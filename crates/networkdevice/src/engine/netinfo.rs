//! Host-local network information: interfaces, IPv4 subnets and the kernel
//! neighbour table.
//!
//! These helpers deliberately parse the output of standard Linux tools
//! (`ip -o -4 addr show`) and `/proc/net/arp` rather than pulling in a
//! netlink dependency. The parsers are pure functions so they can be tested
//! against captured fixture text.

use std::net::Ipv4Addr;

use crate::engine::error::{Error, Result};

/// An IPv4 network attached to a local interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalSubnet {
    pub iface: String,
    pub addr: Ipv4Addr,
    pub prefix_len: u8,
}

impl LocalSubnet {
    /// Network address (all host bits cleared).
    pub fn network(&self) -> Ipv4Addr {
        let mask = self.mask();
        Ipv4Addr::from(u32::from(self.addr) & u32::from(mask))
    }

    pub fn mask(&self) -> Ipv4Addr {
        let bits = if self.prefix_len == 0 {
            0u32
        } else {
            u32::MAX << (32 - self.prefix_len as u32)
        };
        Ipv4Addr::from(bits)
    }

    pub fn broadcast(&self) -> Ipv4Addr {
        Ipv4Addr::from(u32::from(self.network()) | !u32::from(self.mask()))
    }

    /// True when `ip` falls inside this subnet.
    pub fn contains(&self, ip: Ipv4Addr) -> bool {
        let mask = u32::from(self.mask());
        u32::from(ip) & mask == u32::from(self.network())
    }

    /// Every usable host address in the subnet.
    ///
    /// Callers are expected to sanity-check `prefix_len` first: enumerating a
    /// /8 would allocate 16M addresses.
    pub fn hosts(&self) -> impl Iterator<Item = Ipv4Addr> {
        let network = u32::from(self.network());
        let broadcast = u32::from(self.broadcast());
        ((network + 1)..broadcast).map(Ipv4Addr::from)
    }

    /// Number of usable host addresses.
    /// Addresses this subnet can hold, network and broadcast excluded.
    ///
    /// Delegates to [`host_count`], which is also what the sweep expansions
    /// follow — a /31 must not read as one thing here and another there.
    pub fn host_count(&self) -> u64 {
        host_count(self.prefix_len.min(32))
    }

    /// Guard rail: refuse to sweep ranges larger than a /16 by default.
    pub fn is_reasonable_to_sweep(&self) -> bool {
        self.prefix_len >= 16
    }
}

/// Enumerate local IPv4 subnets by parsing `ip -o -4 addr show`.
///
/// Returns an empty list when `ip` is missing; callers treat that as "no
/// automatic sweep targets".
pub fn local_subnets() -> Result<Vec<LocalSubnet>> {
    let output = match std::process::Command::new("ip")
        .args(["-o", "-4", "addr", "show"])
        .output()
    {
        Ok(output) => output,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(Error::PlainIo(e)),
    };

    if !output.status.success() {
        return Err(Error::unavailable(
            "ip",
            format!("`ip -o -4 addr show` exited with {}", output.status),
        ));
    }

    Ok(parse_ip_addr_show(&String::from_utf8_lossy(&output.stdout)))
}

/// Non-loopback interface names, in `ip` order and de-duplicated.
///
/// Multiple IPv4 addresses on one interface yield one name: an ARP sweep is
/// per interface, not per address.
pub fn iface_names(subnets: &[LocalSubnet]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for subnet in subnets {
        if subnet.addr.is_loopback() || names.contains(&subnet.iface) {
            continue;
        }
        names.push(subnet.iface.clone());
    }
    names
}

/// Names of the local interfaces that have a non-loopback IPv4 address.
///
/// Empty when `ip` is missing, exactly like [`local_subnets`].
pub fn local_iface_names() -> Result<Vec<String>> {
    Ok(iface_names(&local_subnets()?))
}

/// Interfaces that carry a default route, in routing-table order.
///
/// This is where the machine's own network lives, which is the LAN a scanner
/// normally cares about.
pub fn default_route_ifaces() -> Result<Vec<String>> {
    let output = match std::process::Command::new("ip")
        .args(["-4", "route", "show", "default"])
        .output()
    {
        Ok(output) => output,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(Error::PlainIo(e)),
    };

    if !output.status.success() {
        return Err(Error::unavailable(
            "ip",
            format!("`ip -4 route show default` exited with {}", output.status),
        ));
    }

    Ok(parse_default_route_ifaces(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

/// Parse the output of `ip -4 route show default`.
///
/// Example line:
/// `default via 192.168.3.1 dev enp5s0 proto dhcp src 192.168.3.128 metric 100`
/// A gateway-less route is shorter: `default dev tun0 scope link`.
pub fn parse_default_route_ifaces(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in text.lines() {
        let mut tokens = line.split_whitespace();
        if tokens.next() != Some("default") {
            continue;
        }
        while let Some(token) = tokens.next() {
            if token == "dev" {
                if let Some(iface) = tokens.next() {
                    push_unique(&mut names, iface);
                }
                break;
            }
        }
    }
    names
}

/// Interfaces a scan should cover when none are configured.
///
/// Prefers the default-route interface, so a host with dozens of container
/// bridges does not scan all of them. With no default route at all — an
/// isolated LAN — every non-loopback IPv4 interface is used.
pub fn auto_ifaces() -> Result<Vec<String>> {
    prefer_default_routes(default_route_ifaces()?, local_iface_names)
}

/// The auto rule itself: default routes when there are any, else the fallback.
fn prefer_default_routes(
    routes: Vec<String>,
    fallback: impl FnOnce() -> Result<Vec<String>>,
) -> Result<Vec<String>> {
    if routes.is_empty() {
        fallback()
    } else {
        Ok(routes)
    }
}

/// The interfaces a source should use: the configured names, else [`auto_ifaces`].
pub fn allowed_ifaces(configured: &[String]) -> Result<Vec<String>> {
    if configured.is_empty() {
        auto_ifaces()
    } else {
        Ok(configured.to_vec())
    }
}

/// Local subnets on the allowed interfaces, loopback excluded.
///
/// The single definition of "which networks does a scan cover", shared by the
/// neighbour, mDNS and SSDP sources so `ifaces` cannot mean four different
/// things.
pub fn scan_subnets(configured: &[String]) -> Result<Vec<LocalSubnet>> {
    let wanted = allowed_ifaces(configured)?;
    let subnets = local_subnets()?;
    Ok(subnets
        .into_iter()
        .filter(|s| !s.addr.is_loopback() && wanted.contains(&s.iface))
        .collect())
}

/// Append `value` unless an equal one is already present, preserving order.
fn push_unique(names: &mut Vec<String>, value: &str) {
    if !names.iter().any(|name| name == value) {
        names.push(value.to_string());
    }
}

/// Parse `a.b.c.d[/prefix]` into its network address and prefix length.
///
/// A bare address means `/32`. The network address is returned (host bits
/// cleared), so `192.168.3.77/24` and `192.168.3.0/24` describe one segment.
pub fn parse_cidr(spec: &str) -> Result<(Ipv4Addr, u8)> {
    let spec = spec.trim();
    let (addr, prefix) = match spec.split_once('/') {
        Some((addr, prefix)) => {
            let prefix: u8 = prefix
                .trim()
                .parse()
                .map_err(|_| Error::parse("subnet", format!("`{spec}` has a bad prefix")))?;
            (addr.trim(), prefix)
        }
        None => (spec, 32),
    };

    if prefix > 32 {
        return Err(Error::parse(
            "subnet",
            format!("`{spec}` has a prefix longer than /32"),
        ));
    }

    let ip: Ipv4Addr = addr
        .parse()
        .map_err(|_| Error::parse("subnet", format!("`{addr}` is not an IPv4 address")))?;

    let mask = if prefix == 0 {
        0u32
    } else {
        u32::MAX << (32 - prefix)
    };
    Ok((Ipv4Addr::from(u32::from(ip) & mask), prefix))
}

/// How many addresses a segment can hold, excluding network and broadcast
/// (RFC 3021 keeps both for /31 and /32).
pub fn host_count(prefix_len: u8) -> u64 {
    match prefix_len {
        0 => (1u64 << 32) - 2,
        1..=30 => (1u64 << (32 - prefix_len as u64)) - 2,
        31 => 2, // RFC 3021: both addresses are usable, none reserved
        _ => 1,
    }
}

/// Parse the `-o` (one-line) output of `ip -4 addr show`.
///
/// Example line:
/// `2: enp5s0    inet 192.168.3.128/24 brd 192.168.3.255 scope global dynamic enp5s0\...`
pub fn parse_ip_addr_show(text: &str) -> Vec<LocalSubnet> {
    let mut subnets = Vec::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Interface name is the token after "<index>:".
        let Some((_, rest)) = line.split_once(':') else {
            continue;
        };
        let Some(iface) = rest.split_whitespace().next() else {
            continue;
        };

        // Find an `inet <addr>/<len>` pair anywhere in the line.
        let tokens: Vec<&str> = line.split_whitespace().collect();
        let Some(pos) = tokens.iter().position(|t| *t == "inet") else {
            continue;
        };
        let Some(cidr) = tokens.get(pos + 1) else {
            continue;
        };
        let Some((addr, len)) = cidr.split_once('/') else {
            continue;
        };
        let (Ok(addr), Ok(prefix_len)) = (addr.parse::<Ipv4Addr>(), len.parse::<u8>()) else {
            continue;
        };
        if prefix_len > 32 {
            continue;
        }

        subnets.push(LocalSubnet {
            iface: iface.to_string(),
            addr,
            prefix_len,
        });
    }

    subnets
}

/// One row of `/proc/net/arp`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArpEntry {
    pub ip: Ipv4Addr,
    pub mac: String,
    pub iface: String,
    /// `ATF_COM` (0x02) means the entry is complete and usable.
    pub complete: bool,
}

/// Parse `/proc/net/arp`.
///
/// Example:
/// ```text
/// IP address       HW type     Flags       HW address            Mask     Device
/// 192.168.3.1      0x1         0x2         2c:a0:42:30:3c:7e     *        enp5s0
/// ```
pub fn parse_proc_net_arp(text: &str) -> Vec<ArpEntry> {
    let mut entries = Vec::new();

    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 6 {
            continue;
        }
        let Ok(ip) = f[0].parse::<Ipv4Addr>() else {
            continue;
        };
        let flags = u32::from_str_radix(f[2].trim_start_matches("0x"), 16).unwrap_or(0);

        entries.push(ArpEntry {
            ip,
            mac: f[3].to_ascii_uppercase(),
            iface: f[5].to_string(),
            complete: flags & 0x2 != 0,
        });
    }

    entries
}

/// Read and parse `/proc/net/arp`.
pub fn read_proc_net_arp() -> Result<Vec<ArpEntry>> {
    let path = "/proc/net/arp";
    let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
    Ok(parse_proc_net_arp(&text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_default_route_ifaces() {
        let fixture = "\
default via 192.168.3.1 dev enp5s0 proto dhcp src 192.168.3.128 metric 100 
default dev tun0 scope link 
default via 192.168.3.1 dev enp5s0 metric 200 
";
        assert_eq!(parse_default_route_ifaces(fixture), vec!["enp5s0", "tun0"]);
        assert!(parse_default_route_ifaces("").is_empty());
        assert!(
            parse_default_route_ifaces(
                "192.168.3.0/24 dev enp5s0 proto kernel scope link src 192.168.3.128"
            )
            .is_empty(),
            "only default routes count"
        );
    }

    #[test]
    fn auto_prefers_default_routes_and_falls_back() {
        assert_eq!(
            prefer_default_routes(vec!["enp5s0".into()], || Ok(vec!["wlan0".into()])).unwrap(),
            vec!["enp5s0"]
        );
        assert_eq!(
            prefer_default_routes(vec![], || Ok(vec!["wlan0".into(), "enp5s0".into()])).unwrap(),
            vec!["wlan0", "enp5s0"]
        );
    }

    #[test]
    fn configured_interfaces_are_used_verbatim() {
        let configured = vec!["eth0".to_string()];
        assert_eq!(allowed_ifaces(&configured).unwrap(), vec!["eth0"]);
    }

    #[test]
    fn parse_cidr_returns_the_network_address() {
        assert_eq!(
            parse_cidr("192.168.3.77/24").unwrap(),
            ("192.168.3.0".parse().unwrap(), 24)
        );
        assert_eq!(
            parse_cidr("10.0.0.7").unwrap(),
            ("10.0.0.7".parse().unwrap(), 32)
        );
        assert_eq!(
            parse_cidr(" 10.0.0.0/8 ").unwrap(),
            ("10.0.0.0".parse().unwrap(), 8)
        );
        assert!(parse_cidr("10.0.0.0/33").is_err());
        assert!(parse_cidr("not-an-ip/24").is_err());
        assert!(parse_cidr("10.0.0.0/abc").is_err());
    }

    #[test]
    fn host_count_excludes_network_and_broadcast() {
        assert_eq!(host_count(24), 254);
        assert_eq!(host_count(16), 65534);
        assert_eq!(host_count(30), 2);
        assert_eq!(
            host_count(31),
            2,
            "RFC 3021: both addresses are usable, none reserved"
        );
        assert_eq!(host_count(32), 1);
    }

    #[test]
    fn iface_names_skip_loopback_and_duplicates_but_keep_order() {
        let subnets = parse_ip_addr_show(
            "\
1: lo    inet 127.0.0.1/8 scope host lo
2: enp5s0    inet 192.168.3.128/24 brd 192.168.3.255 scope global enp5s0
3: enp5s0    inet 10.1.2.3/16 brd 10.1.255.255 scope global enp5s0
4: wlp4s0    inet 192.168.1.21/24 brd 192.168.1.255 scope global wlp4s0\n",
        );
        assert_eq!(iface_names(&subnets), vec!["enp5s0", "wlp4s0"]);
        assert!(iface_names(&[]).is_empty());
    }

    #[test]
    fn parses_ip_addr_show_output() {
        let fixture = "\
1: lo    inet 127.0.0.1/8 scope host lo\\       valid_lft forever preferred_lft forever
2: enp5s0    inet 192.168.3.128/24 brd 192.168.3.255 scope global dynamic enp5s0\\       valid_lft 604686sec preferred_lft 604686sec
3: virbr0    inet 192.168.122.1/24 brd 192.168.122.255 scope global virbr0\\       valid_lft forever preferred_lft forever
4: wlp4s0    inet6 fe80::1/64 scope link \\       valid_lft forever preferred_lft forever
";
        let subnets = parse_ip_addr_show(fixture);
        assert_eq!(subnets.len(), 3, "IPv6-only lines must be skipped");
        assert_eq!(subnets[0].iface, "lo");
        assert_eq!(subnets[0].addr, Ipv4Addr::new(127, 0, 0, 1));
        assert_eq!(subnets[1].iface, "enp5s0");
        assert_eq!(subnets[1].prefix_len, 24);
        assert_eq!(subnets[2].iface, "virbr0");
    }

    #[test]
    fn computes_subnet_bounds() {
        let subnet = LocalSubnet {
            iface: "eth0".into(),
            addr: Ipv4Addr::new(192, 168, 3, 128),
            prefix_len: 24,
        };
        assert_eq!(subnet.network(), Ipv4Addr::new(192, 168, 3, 0));
        assert_eq!(subnet.broadcast(), Ipv4Addr::new(192, 168, 3, 255));
        assert_eq!(subnet.host_count(), 254);

        let hosts: Vec<_> = subnet.hosts().collect();
        assert_eq!(hosts.len(), 254);
        assert_eq!(hosts[0], Ipv4Addr::new(192, 168, 3, 1));
        assert_eq!(hosts[253], Ipv4Addr::new(192, 168, 3, 254));
    }

    #[test]
    fn containment_uses_the_network_mask() {
        let subnet = LocalSubnet {
            iface: "eth0".into(),
            addr: Ipv4Addr::new(192, 168, 3, 128),
            prefix_len: 24,
        };
        assert!(subnet.contains(Ipv4Addr::new(192, 168, 3, 1)));
        assert!(subnet.contains(Ipv4Addr::new(192, 168, 3, 255)));
        assert!(!subnet.contains(Ipv4Addr::new(192, 168, 4, 1)));
        assert!(!subnet.contains(Ipv4Addr::new(10, 0, 0, 1)));
    }

    #[test]
    fn flags_large_subnets() {
        let big = LocalSubnet {
            iface: "eth0".into(),
            addr: Ipv4Addr::new(10, 0, 0, 1),
            prefix_len: 8,
        };
        assert!(!big.is_reasonable_to_sweep());
        let ok = LocalSubnet {
            iface: "eth0".into(),
            addr: Ipv4Addr::new(10, 0, 0, 1),
            prefix_len: 16,
        };
        assert!(ok.is_reasonable_to_sweep());
    }

    #[test]
    fn parses_proc_net_arp() {
        let fixture = "\
IP address       HW type     Flags       HW address            Mask     Device
192.168.3.96     0x1         0x2         d0:57:7e:35:f5:2f     *        enp5s0
192.168.3.171    0x1         0x0         00:00:00:00:00:00     *        enp5s0
10.0.0.5         0x1         0x2         AA:BB:CC:DD:EE:FF     *        eth0
garbage
";
        let entries = parse_proc_net_arp(fixture);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].ip, Ipv4Addr::new(192, 168, 3, 96));
        assert_eq!(entries[0].mac, "D0:57:7E:35:F5:2F");
        assert!(entries[0].complete);
        assert!(!entries[1].complete, "flags 0x0 is an incomplete entry");
        assert_eq!(entries[2].iface, "eth0");
    }
}
