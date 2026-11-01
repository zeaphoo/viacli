//! Async TCP connect scanning: the hot loop a large scan runs in.
//!
//! A /16 over a couple of ports is tens of thousands of connections, and one
//! blocking thread per connection does not scale. Everything funnels through
//! here instead: a bounded number of connects in flight on Tokio, which the
//! operating system multiplexes. A connect scan needs no privileges, unlike a
//! SYN scan, so it stays the default.
//!
//! The entry point is synchronous on purpose — the [`crate::engine::discovery::Discovery`]
//! trait, the port probe and the neighbour sweep are all blocking — so a
//! current-thread runtime is built, driven to completion, and dropped per call.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;
use tokio::runtime::Builder;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

/// The ceiling on connects in flight at once when nothing else is configured.
///
/// High enough that a whole port list goes out in one wave, low enough that a
/// scan does not exhaust the host's file descriptors.
pub const DEFAULT_CONCURRENCY: usize = 256;

/// How much of a greeting is kept when a connection is read on the spot.
const READ_LIMIT: usize = 512;

/// A completed handshake and whatever the service said without being asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connected {
    pub ip: Ipv4Addr,
    pub port: u16,
    /// The bytes read from the connection, empty when the service waits for a
    /// prompt or the read timed out.
    pub banner: Vec<u8>,
}

/// Every `(ip, port)` that completed a TCP handshake, in completion order.
///
/// Returns nothing when the runtime cannot be built, which keeps a broken
/// environment from aborting a scan that could still try the other sources.
pub fn open_ports(
    targets: &[(Ipv4Addr, u16)],
    timeout: Duration,
    concurrency: usize,
) -> Vec<(Ipv4Addr, u16)> {
    if targets.is_empty() {
        return Vec::new();
    }
    let Ok(runtime) = Builder::new_current_thread().enable_all().build() else {
        return Vec::new();
    };

    runtime.block_on(async {
        let semaphore = Arc::new(Semaphore::new(concurrency.max(1)));
        let mut tasks = JoinSet::new();

        for &(ip, port) in targets {
            // Take the permit before spawning, so what is bounded is the
            // connections in flight, not the number of tasks created.
            let Ok(permit) = semaphore.clone().acquire_owned().await else {
                break;
            };
            tasks.spawn(async move {
                let _permit = permit;
                let addr = SocketAddr::from((ip, port));
                match tokio::time::timeout(timeout, TcpStream::connect(addr)).await {
                    Ok(Ok(_stream)) => Some((ip, port)),
                    _ => None,
                }
            });
        }

        let mut open = Vec::new();
        while let Some(result) = tasks.join_next().await {
            if let Ok(Some(pair)) = result {
                open.push(pair);
            }
        }
        open
    })
}

/// Connect to every target and read once on the same connection, keeping what
/// the service sends unprompted.
///
/// Reading on the connection the handshake already opened is what lets a banner
/// be captured from a server that accepts a single connection. Every port that
/// answered comes back, banner or not; only the read waits out the timeout.
pub fn connect_and_read(
    targets: &[(Ipv4Addr, u16)],
    timeout: Duration,
    concurrency: usize,
) -> Vec<Connected> {
    if targets.is_empty() {
        return Vec::new();
    }
    let Ok(runtime) = Builder::new_current_thread().enable_all().build() else {
        return Vec::new();
    };

    runtime.block_on(async {
        let semaphore = Arc::new(Semaphore::new(concurrency.max(1)));
        let mut tasks = JoinSet::new();

        for &(ip, port) in targets {
            let Ok(permit) = semaphore.clone().acquire_owned().await else {
                break;
            };
            tasks.spawn(async move {
                let _permit = permit;
                let addr = SocketAddr::from((ip, port));
                let stream = tokio::time::timeout(timeout, TcpStream::connect(addr))
                    .await
                    .ok()?
                    .ok()?;
                let mut stream = stream;
                let mut buffer = vec![0u8; READ_LIMIT];
                let banner = match tokio::time::timeout(timeout, stream.read(&mut buffer)).await {
                    Ok(Ok(read)) => buffer[..read].to_vec(),
                    // A timeout with nothing, or an error, is an open port with
                    // no greeting: still a result.
                    _ => Vec::new(),
                };
                Some(Connected { ip, port, banner })
            });
        }

        let mut connected = Vec::new();
        while let Some(result) = tasks.join_next().await {
            if let Ok(Some(found)) = result {
                connected.push(found);
            }
        }
        connected
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn an_open_listener_is_found_and_a_closed_port_is_not() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();

        // A port that was just bound then dropped is very likely free.
        let closed = TcpListener::bind("127.0.0.1:0")
            .expect("bind")
            .local_addr()
            .unwrap()
            .port();

        let targets = vec![
            (Ipv4Addr::LOCALHOST, port),
            (Ipv4Addr::LOCALHOST, closed),
        ];
        let open = open_ports(&targets, Duration::from_millis(500), 16);

        assert_eq!(open, vec![(Ipv4Addr::LOCALHOST, port)]);
    }

    #[test]
    fn every_open_port_of_one_host_comes_back() {
        let first = TcpListener::bind("127.0.0.1:0").expect("bind");
        let second = TcpListener::bind("127.0.0.1:0").expect("bind");
        let targets = vec![
            (Ipv4Addr::LOCALHOST, first.local_addr().unwrap().port()),
            (Ipv4Addr::LOCALHOST, second.local_addr().unwrap().port()),
        ];

        let mut open = open_ports(&targets, Duration::from_millis(500), 1);
        open.sort();
        assert_eq!(open.len(), 2);
    }

    #[test]
    fn an_empty_target_list_is_not_an_error() {
        assert!(open_ports(&[], Duration::from_millis(10), 4).is_empty());
    }

    #[test]
    fn a_greeting_is_read_on_the_same_connection() {
        use std::io::Write;

        // A server that accepts exactly one connection, like the test doubles
        // in `identify`. Reconnecting for the banner would miss it.
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let _ = stream.write_all(b"SSH-2.0-OpenSSH_9.0p1\r\n");
            }
        });

        let found = connect_and_read(
            &[(Ipv4Addr::LOCALHOST, port)],
            Duration::from_millis(500),
            4,
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].banner, b"SSH-2.0-OpenSSH_9.0p1\r\n");
    }

    #[test]
    fn a_quiet_open_port_still_comes_back() {
        // A listener that accepts and says nothing: the port is open, the
        // banner is empty, and the read has to time out rather than hang.
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let _ = listener.accept();
            std::thread::sleep(Duration::from_millis(400));
        });

        let found = connect_and_read(
            &[(Ipv4Addr::LOCALHOST, port)],
            Duration::from_millis(120),
            4,
        );
        assert_eq!(found.len(), 1);
        assert!(found[0].banner.is_empty());
    }
}
