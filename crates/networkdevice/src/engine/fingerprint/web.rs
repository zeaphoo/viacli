//! Web fingerprint rules: what is running behind that web interface.
//!
//! A LAN is full of small web servers that say exactly what they are — a router,
//! a NAS, a printer, a camera — and a handful of words on the front page is
//! usually enough to name the product. The rules below are data, not code: each
//! one lists what to look for and what it means.
//!
//! Nothing here is inferred from a *port*: the caller has already decided that
//! something on this port speaks HTTP. What is left is the fun part.

use crate::engine::model::DeviceKind;

/// What the probe saw when it asked for the front page.
#[derive(Debug, Default, Clone, Copy)]
pub struct Page<'a> {
    /// Response headers as they arrived; [`evaluate`] lower-cases them.
    pub headers: &'a [(String, String)],
    /// Cookie names from `Set-Cookie`.
    pub cookies: &'a [String],
    /// Title and body. The rules are written in lower case and the engine
    /// normalises, so callers do not have to remember to.
    pub text: &'a str,
    /// The front page's favicon hash, from [`favicon_hash`].
    pub favicon_hash: Option<u32>,
}

/// One rule: what to look for, and what it means.
pub struct Rule {
    pub name: &'static str,
    pub vendor: Option<&'static str>,
    /// The kind this product implies, when it implies one.
    pub kind: Option<DeviceKind>,
    /// `(header name, substring of its value)`, both lower-case.
    pub headers: &'static [(&'static str, &'static str)],
    /// Cookie names.
    pub cookies: &'static [&'static str],
    /// Substrings of the title or body, lower-case.
    pub body: &'static [&'static str],
    /// Favicon hashes, in the convention [`favicon_hash`] implements.
    pub favicon: &'static [u32],
}

impl Rule {
    /// How much this rule matched. Headers and cookies are deliberate; body text
    /// can appear by accident, so it counts for less.
    fn score(&self, page: &Page) -> u32 {
        let mut score = 0;
        for (name, needle) in self.headers {
            if page
                .headers
                .iter()
                .any(|(key, value)| key == name && value.contains(needle))
            {
                score += 3;
            }
        }
        for cookie in self.cookies {
            if page.cookies.iter().any(|name| name == cookie) {
                score += 3;
            }
        }
        if let Some(hash) = page.favicon_hash
            && self.favicon.contains(&hash)
        {
            score += 6;
        }
        for needle in self.body {
            if page.text.contains(needle) {
                score += 1;
            }
        }
        score
    }
}

/// A rule that matched, with how strongly. The rule comes from the table, so
/// the match outlives the page it was found on.
pub struct Match {
    pub rule: &'static Rule,
    pub score: u32,
}

/// The best-matching rule for a page, if any.
pub fn evaluate(page: &Page<'_>) -> Option<Match> {
    // Normalise once, here: the rules are lower case, and asking every caller
    // to lower-case headers and body is asking for a silent miss.
    let text = page.text.to_lowercase();
    let headers: Vec<(String, String)> = page
        .headers
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), value.to_ascii_lowercase()))
        .collect();
    let cookies: Vec<String> = page
        .cookies
        .iter()
        .map(|name| name.to_ascii_lowercase())
        .collect();
    let normalised = Page {
        headers: &headers,
        cookies: &cookies,
        text: &text,
        favicon_hash: page.favicon_hash,
    };

    RULES
        .iter()
        .filter_map(|rule| {
            let score = rule.score(&normalised);
            (score > 0).then_some(Match { rule, score })
        })
        // Highest score wins; a tie keeps the earlier rule, and the table is
        // ordered specific-before-generic on purpose.
        .max_by_key(|found| found.score)
}

/// The rules, most specific first.
///
/// `body` needles are chosen to be distinctive: "cisco" alone would match any
/// page that mentions Cisco, while "cisco systems" appears in the device's own
/// footer. Where a product's name is the only signal there is, that is what the
/// rule uses.
#[rustfmt::skip]
pub static RULES: &[Rule] = &[
    // ---------------------------------------------------------- network gear
    Rule { name: "RouterOS", vendor: Some("MikroTik"), kind: Some(DeviceKind::Router),
        headers: &[], cookies: &[], favicon: &[], body: &["routeros", "mikrotik"] },
    Rule { name: "OpenWrt (LuCI)", vendor: None, kind: Some(DeviceKind::Router),
        headers: &[], cookies: &[], favicon: &[], body: &["luci", "openwrt"] },
    Rule { name: "DD-WRT", vendor: None, kind: Some(DeviceKind::Router),
        headers: &[], cookies: &[], favicon: &[], body: &["dd-wrt"] },
    Rule { name: "pfSense", vendor: None, kind: Some(DeviceKind::Router),
        headers: &[], cookies: &[], favicon: &[], body: &["pfsense"] },
    Rule { name: "OPNsense", vendor: None, kind: Some(DeviceKind::Router),
        headers: &[], cookies: &[], favicon: &[], body: &["opnsense"] },
    Rule { name: "EdgeOS", vendor: Some("Ubiquiti"), kind: Some(DeviceKind::Router),
        headers: &[], cookies: &[], favicon: &[], body: &["edgeos", "edgerouter"] },
    Rule { name: "UniFi", vendor: Some("Ubiquiti"), kind: Some(DeviceKind::AccessPoint),
        headers: &[], cookies: &[], favicon: &[], body: &["unifi", "ubiquiti networks"] },
    Rule { name: "airOS", vendor: Some("Ubiquiti"), kind: Some(DeviceKind::AccessPoint),
        headers: &[], cookies: &[], favicon: &[], body: &["airos"] },
    Rule { name: "FRITZ!Box", vendor: Some("AVM"), kind: Some(DeviceKind::Router),
        headers: &[], cookies: &[], favicon: &[], body: &["fritz!box", "avm.de"] },
    Rule { name: "AsusWRT", vendor: Some("ASUS"), kind: Some(DeviceKind::Router),
        headers: &[], cookies: &[], favicon: &[], body: &["asuswrt", "asus wireless router"] },
    Rule { name: "TP-Link", vendor: Some("TP-Link"), kind: Some(DeviceKind::Router),
        headers: &[], cookies: &[], favicon: &[], body: &["tp-link", "tplink"] },
    Rule { name: "D-Link", vendor: Some("D-Link"), kind: Some(DeviceKind::Router),
        headers: &[], cookies: &[], favicon: &[], body: &["d-link", "dlink"] },
    Rule { name: "NETGEAR", vendor: Some("NETGEAR"), kind: Some(DeviceKind::Router),
        headers: &[], cookies: &[], favicon: &[], body: &["netgear"] },
    Rule { name: "Tenda", vendor: Some("Tenda"), kind: Some(DeviceKind::Router),
        headers: &[], cookies: &[], favicon: &[], body: &["tenda"] },
    Rule { name: "Zyxel", vendor: Some("Zyxel"), kind: Some(DeviceKind::Router),
        headers: &[], cookies: &[], favicon: &[], body: &["zyxel"] },
    Rule { name: "H3C", vendor: Some("H3C"), kind: Some(DeviceKind::Router),
        headers: &[], cookies: &[], favicon: &[], body: &["h3c"] },
    Rule { name: "Huawei HiLink", vendor: Some("Huawei"), kind: Some(DeviceKind::Router),
        headers: &[], cookies: &[], favicon: &[], body: &["hilink"] },
    Rule { name: "Xiaomi MiWiFi", vendor: Some("Xiaomi"), kind: Some(DeviceKind::Router),
        headers: &[], cookies: &[], favicon: &[], body: &["miwifi", "小米路由器"] },
    Rule { name: "iKuai", vendor: None, kind: Some(DeviceKind::Router),
        headers: &[], cookies: &[], favicon: &[], body: &["ikuai", "爱快"] },
    Rule { name: "MERCURY", vendor: Some("Mercury"), kind: Some(DeviceKind::Router),
        headers: &[], cookies: &[], favicon: &[], body: &["mercury router", "melogin"] },

    // ------------------------------------------------------------- storage
    Rule { name: "Synology DSM", vendor: Some("Synology"), kind: Some(DeviceKind::Nas),
        headers: &[], cookies: &["id"], favicon: &[], body: &["synology", "diskstation"] },
    Rule { name: "QNAP QTS", vendor: Some("QNAP"), kind: Some(DeviceKind::Nas),
        headers: &[], cookies: &[], favicon: &[], body: &["qnap", "qts "] },
    Rule { name: "TrueNAS", vendor: Some("iXsystems"), kind: Some(DeviceKind::Nas),
        headers: &[], cookies: &[], favicon: &[], body: &["truenas"] },
    Rule { name: "FreeNAS", vendor: Some("iXsystems"), kind: Some(DeviceKind::Nas),
        headers: &[], cookies: &[], favicon: &[], body: &["freenas"] },
    Rule { name: "OpenMediaVault", vendor: None, kind: Some(DeviceKind::Nas),
        headers: &[], cookies: &[], favicon: &[], body: &["openmediavault"] },
    Rule { name: "WD My Cloud", vendor: Some("Western Digital"), kind: Some(DeviceKind::Nas),
        headers: &[], cookies: &[], favicon: &[], body: &["my cloud", "wdcloud"] },
    Rule { name: "ASUSTOR", vendor: Some("ASUSTOR"), kind: Some(DeviceKind::Nas),
        headers: &[], cookies: &[], favicon: &[], body: &["asustor"] },
    Rule { name: "TerraMaster", vendor: Some("TerraMaster"), kind: Some(DeviceKind::Nas),
        headers: &[], cookies: &[], favicon: &[], body: &["terramaster"] },

    // ------------------------------------------------------------ printing
    Rule { name: "HP Embedded Web Server", vendor: Some("HP"), kind: Some(DeviceKind::Printer),
        headers: &[], cookies: &[], favicon: &[], body: &["hp laserjet", "hewlett-packard", "hp.com"] },
    Rule { name: "Epson", vendor: Some("Seiko Epson"), kind: Some(DeviceKind::Printer),
        headers: &[], cookies: &[], favicon: &[], body: &["epson", "seiko epson"] },
    Rule { name: "Brother", vendor: Some("Brother"), kind: Some(DeviceKind::Printer),
        headers: &[], cookies: &[], favicon: &[], body: &["brother industries", "brother"] },
    Rule { name: "Canon", vendor: Some("Canon"), kind: Some(DeviceKind::Printer),
        headers: &[], cookies: &[], favicon: &[], body: &["canon inc", "canon"] },
    Rule { name: "Xerox", vendor: Some("Xerox"), kind: Some(DeviceKind::Printer),
        headers: &[], cookies: &[], favicon: &[], body: &["xerox"] },
    Rule { name: "Kyocera", vendor: Some("Kyocera"), kind: Some(DeviceKind::Printer),
        headers: &[], cookies: &[], favicon: &[], body: &["kyocera"] },
    Rule { name: "Ricoh", vendor: Some("Ricoh"), kind: Some(DeviceKind::Printer),
        headers: &[], cookies: &[], favicon: &[], body: &["ricoh"] },
    Rule { name: "Lexmark", vendor: Some("Lexmark"), kind: Some(DeviceKind::Printer),
        headers: &[], cookies: &[], favicon: &[], body: &["lexmark"] },
    Rule { name: "Pantum", vendor: Some("Pantum"), kind: Some(DeviceKind::Printer),
        headers: &[], cookies: &[], favicon: &[], body: &["pantum"] },

    // ------------------------------------------------------------- cameras
    Rule { name: "Hikvision", vendor: Some("Hikvision"), kind: Some(DeviceKind::Camera),
        headers: &[], cookies: &[], favicon: &[], body: &["hikvision"] },
    Rule { name: "Dahua", vendor: Some("Dahua"), kind: Some(DeviceKind::Camera),
        headers: &[], cookies: &[], favicon: &[], body: &["dahua"] },
    Rule { name: "Reolink", vendor: Some("Reolink"), kind: Some(DeviceKind::Camera),
        headers: &[], cookies: &[], favicon: &[], body: &["reolink"] },
    Rule { name: "Axis", vendor: Some("Axis"), kind: Some(DeviceKind::Camera),
        headers: &[], cookies: &[], favicon: &[], body: &["axis communications"] },
    Rule { name: "Foscam", vendor: Some("Foscam"), kind: Some(DeviceKind::Camera),
        headers: &[], cookies: &[], favicon: &[], body: &["foscam"] },
    Rule { name: "Amcrest", vendor: Some("Amcrest"), kind: Some(DeviceKind::Camera),
        headers: &[], cookies: &[], favicon: &[], body: &["amcrest"] },
    Rule { name: "Uniview", vendor: Some("Uniview"), kind: Some(DeviceKind::Camera),
        headers: &[], cookies: &[], favicon: &[], body: &["uniview"] },

    // ----------------------------------------------------------- home lab
    Rule { name: "Proxmox VE", vendor: Some("Proxmox"), kind: Some(DeviceKind::Server),
        headers: &[("server", "pve-api-daemon")], cookies: &["pveauth"], favicon: &[],
        body: &["proxmox"] },
    Rule { name: "VMware ESXi", vendor: Some("VMware"), kind: Some(DeviceKind::Server),
        headers: &[], cookies: &[], favicon: &[], body: &["vmware esxi", "vsphere"] },
    Rule { name: "Home Assistant", vendor: Some("Home Assistant"), kind: Some(DeviceKind::Server),
        headers: &[], cookies: &[], favicon: &[], body: &["home assistant"] },
    Rule { name: "Pi-hole", vendor: None, kind: Some(DeviceKind::Server),
        headers: &[], cookies: &[], favicon: &[], body: &["pi-hole", "pihole"] },
    Rule { name: "AdGuard Home", vendor: Some("AdGuard"), kind: Some(DeviceKind::Server),
        headers: &[], cookies: &[], favicon: &[], body: &["adguard"] },
    Rule { name: "Grafana", vendor: Some("Grafana Labs"), kind: Some(DeviceKind::Server),
        headers: &[], cookies: &["grafana_session"], favicon: &[], body: &["grafana"] },
    Rule { name: "Portainer", vendor: Some("Portainer"), kind: Some(DeviceKind::Server),
        headers: &[], cookies: &[], favicon: &[], body: &["portainer"] },
    Rule { name: "Plex", vendor: Some("Plex"), kind: Some(DeviceKind::Server),
        headers: &[], cookies: &[], favicon: &[], body: &["plex"] },
    Rule { name: "Jellyfin", vendor: None, kind: Some(DeviceKind::Server),
        headers: &[], cookies: &[], favicon: &[], body: &["jellyfin"] },
    Rule { name: "Emby", vendor: Some("Emby"), kind: Some(DeviceKind::Server),
        headers: &[], cookies: &[], favicon: &[], body: &["emby"] },

    // ---------------------------------------------------------- appliances
    Rule { name: "OctoPrint", vendor: None, kind: Some(DeviceKind::Iot),
        headers: &[], cookies: &[], favicon: &[], body: &["octoprint"] },
    Rule { name: "Homebridge", vendor: None, kind: Some(DeviceKind::Iot),
        headers: &[], cookies: &[], favicon: &[], body: &["homebridge"] },
    Rule { name: "Tasmota", vendor: None, kind: Some(DeviceKind::Iot),
        headers: &[], cookies: &[], favicon: &[], body: &["tasmota"] },
    Rule { name: "ESPHome", vendor: None, kind: Some(DeviceKind::Iot),
        headers: &[], cookies: &[], favicon: &[], body: &["esphome"] },
    Rule { name: "Shelly", vendor: Some("Shelly"), kind: Some(DeviceKind::Iot),
        headers: &[], cookies: &[], favicon: &[], body: &["shelly"] },
    Rule { name: "Philips Hue", vendor: Some("Signify"), kind: Some(DeviceKind::Iot),
        headers: &[], cookies: &[], favicon: &[], body: &["philips hue", "philips-hue"] },
    Rule { name: "Sonos", vendor: Some("Sonos"), kind: Some(DeviceKind::Iot),
        headers: &[], cookies: &[], favicon: &[], body: &["sonos"] },

    // ---------------------------------------------------------- self-hosted services
    Rule { name: "Grafana", vendor: None, kind: Some(DeviceKind::Server),
        headers: &[], cookies: &[], favicon: &[], body: &["grafana"] },
    Rule { name: "Prometheus", vendor: None, kind: Some(DeviceKind::Server),
        headers: &[], cookies: &[], favicon: &[], body: &["prometheus time series", "prometheus.io"] },
    Rule { name: "Kibana", vendor: Some("Elastic"), kind: Some(DeviceKind::Server),
        headers: &[], cookies: &[], favicon: &[], body: &["kibana"] },
    Rule { name: "Elasticsearch", vendor: Some("Elastic"), kind: Some(DeviceKind::Server),
        headers: &[], cookies: &[], favicon: &[], body: &["you know, for search", "\"cluster_name\""] },
    Rule { name: "RabbitMQ Management", vendor: Some("Broadcom"), kind: Some(DeviceKind::Server),
        headers: &[], cookies: &[], favicon: &[], body: &["rabbitmq management"] },
    Rule { name: "Jenkins", vendor: None, kind: Some(DeviceKind::Server),
        headers: &[("x-jenkins", "")], cookies: &[], favicon: &[], body: &["jenkins"] },
    Rule { name: "Portainer", vendor: None, kind: Some(DeviceKind::Server),
        headers: &[], cookies: &[], favicon: &[], body: &["portainer"] },
    Rule { name: "MinIO", vendor: None, kind: Some(DeviceKind::Server),
        headers: &[("server", "minio")], cookies: &[], favicon: &[], body: &["minio"] },
    Rule { name: "Nexus Repository", vendor: Some("Sonatype"), kind: Some(DeviceKind::Server),
        headers: &[], cookies: &[], favicon: &[], body: &["nexus repository"] },
    Rule { name: "Proxmox VE", vendor: Some("Proxmox"), kind: Some(DeviceKind::Server),
        headers: &[], cookies: &[], favicon: &[], body: &["proxmox"] },
    Rule { name: "Home Assistant", vendor: None, kind: Some(DeviceKind::Iot),
        headers: &[], cookies: &[], favicon: &[], body: &["home assistant"] },
    Rule { name: "Node-RED", vendor: None, kind: Some(DeviceKind::Iot),
        headers: &[], cookies: &[], favicon: &[], body: &["node-red"] },
    Rule { name: "Pi-hole", vendor: None, kind: Some(DeviceKind::Server),
        headers: &[], cookies: &[], favicon: &[], body: &["pi-hole", "pihole"] },
    Rule { name: "AdGuard Home", vendor: Some("AdGuard"), kind: Some(DeviceKind::Server),
        headers: &[], cookies: &[], favicon: &[], body: &["adguard home"] },
    Rule { name: "phpMyAdmin", vendor: None, kind: Some(DeviceKind::Server),
        headers: &[], cookies: &[], favicon: &[], body: &["phpmyadmin"] },
    Rule { name: "Adminer", vendor: None, kind: Some(DeviceKind::Server),
        headers: &[], cookies: &[], favicon: &[], body: &["adminer"] },
    Rule { name: "Gitea", vendor: None, kind: Some(DeviceKind::Server),
        headers: &[], cookies: &[], favicon: &[], body: &["gitea"] },
    Rule { name: "GitLab", vendor: Some("GitLab"), kind: Some(DeviceKind::Server),
        headers: &[], cookies: &[], favicon: &[], body: &["gitlab"] },
    Rule { name: "Jupyter", vendor: None, kind: Some(DeviceKind::Server),
        headers: &[], cookies: &[], favicon: &[], body: &["jupyter"] },
    Rule { name: "qBittorrent", vendor: None, kind: Some(DeviceKind::Server),
        headers: &[], cookies: &[], favicon: &[], body: &["qbittorrent"] },
    Rule { name: "Transmission", vendor: None, kind: Some(DeviceKind::Server),
        headers: &[], cookies: &[], favicon: &[], body: &["transmission"] },
];

// ------------------------------------------------------------- favicon

/// murmur3 (x86, 32-bit) — the hash family browsers' favicon databases use.
pub fn murmur3_32(data: &[u8], seed: u32) -> u32 {
    const C1: u32 = 0xcc9e_2d51;
    const C2: u32 = 0x1b87_3593;

    let mut hash = seed;
    let mut chunks = data.chunks_exact(4);
    for chunk in &mut chunks {
        let mut k = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        k = k.wrapping_mul(C1).rotate_left(15).wrapping_mul(C2);
        hash ^= k;
        hash = hash
            .rotate_left(13)
            .wrapping_mul(5)
            .wrapping_add(0xe654_6b64);
    }

    let tail = chunks.remainder();
    if !tail.is_empty() {
        let mut k = 0u32;
        for (index, byte) in tail.iter().enumerate() {
            k |= u32::from(*byte) << (8 * index);
        }
        hash ^= k.wrapping_mul(C1).rotate_left(15).wrapping_mul(C2);
    }

    hash ^= data.len() as u32;
    hash ^= hash >> 16;
    hash = hash.wrapping_mul(0x85eb_ca6b);
    hash ^= hash >> 13;
    hash = hash.wrapping_mul(0xc2b2_ae35);
    hash ^= hash >> 16;
    hash
}

/// A favicon's hash, in the convention the public databases use: murmur3 over
/// the base64 of the icon, *including* the line breaks CPython's
/// `base64.encodebytes` inserts (one per 57 input bytes).
///
/// Following that convention exactly is the point: a hash printed by this
/// scanner can be looked up in Shodan, FOFA or any favicon fingerprint list.
pub fn favicon_hash(icon: &[u8]) -> u32 {
    use base64::Engine as _;
    const BINSIZE: usize = 57;

    let mut text = String::with_capacity(icon.len() * 4 / 3 + 8);
    for chunk in icon.chunks(BINSIZE) {
        text.push_str(&base64::engine::general_purpose::STANDARD.encode(chunk));
        text.push('\n');
    }
    murmur3_32(text.as_bytes(), 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn murmur3_matches_the_published_vectors() {
        assert_eq!(murmur3_32(b"", 0), 0);
        assert_eq!(murmur3_32(b"foo", 0), 0xF6A5_C420);
        assert_eq!(murmur3_32(b"hello", 0), 0x248B_FA47);
        assert_eq!(murmur3_32(&[b'a'; 100], 0), 0x8EEF_A468);
    }

    #[test]
    fn the_favicon_hash_follows_the_public_convention() {
        // 57 bytes per line, a newline after each: the same shape Python's
        // `base64.encodebytes` produces, which is what the public databases hash.
        let icon: Vec<u8> = (0..120).map(|byte| byte as u8).collect();
        assert_eq!(favicon_hash(&icon), 2_070_964_735);
        // Empty input hashes to the empty string's hash, not to a panic.
        assert_eq!(favicon_hash(&[]), 0);
    }

    /// Empty header/cookie slices with a `'static` lifetime, so a page built
    /// from them is not borrowing a temporary.
    const NO_HEADERS: &[(String, String)] = &[];
    const NO_COOKIES: &[String] = &[];

    fn page<'a>(
        headers: &'a [(String, String)],
        cookies: &'a [String],
        text: &'a str,
        favicon_hash: Option<u32>,
    ) -> Page<'a> {
        Page {
            headers,
            cookies,
            text,
            favicon_hash,
        }
    }

    fn matched(input: &Page) -> Option<&'static str> {
        evaluate(input).map(|found| found.rule.name)
    }

    #[test]
    fn a_router_page_names_the_router() {
        let input = page(
            NO_HEADERS,
            NO_COOKIES,
            "<html><title>RouterOS</title>",
            None,
        );
        assert_eq!(matched(&input), Some("RouterOS"));
        let input = page(NO_HEADERS, NO_COOKIES, "<html>Welcome to <b>LuCI</b>", None);
        assert_eq!(matched(&input), Some("OpenWrt (LuCI)"));
    }

    #[test]
    fn a_printer_and_a_camera_are_told_apart() {
        let printer = page(
            NO_HEADERS,
            NO_COOKIES,
            "<html>HP LaserJet 1020</html>",
            None,
        );
        assert_eq!(matched(&printer), Some("HP Embedded Web Server"));
        let camera = page(
            NO_HEADERS,
            NO_COOKIES,
            "<html>Hikvision Digital Technology</html>",
            None,
        );
        assert_eq!(matched(&camera), Some("Hikvision"));
    }

    #[test]
    fn headers_and_cookies_outweigh_page_text() {
        // A page that merely mentions Grafana loses to the product that set its
        // own session cookie.
        let cookies = ["grafana_session".to_string()];
        let both = page(NO_HEADERS, &cookies, "<html>nothing to see", None);
        assert_eq!(matched(&both), Some("Grafana"));

        let grafana_text = page(NO_HEADERS, NO_COOKIES, "<html>grafana", None);
        let synology = page(NO_HEADERS, NO_COOKIES, "<html>Synology DiskStation", None);
        assert_eq!(matched(&grafana_text), Some("Grafana"));
        assert_eq!(matched(&synology), Some("Synology DSM"));
    }

    /// A rule whose only signal is a favicon. The hash is the one
    /// [`favicon_hash`] gives for `(0..120).map(|b| b as u8)`, which the test
    /// above pins to its own independent value.
    static FAVICON_RULE: Rule = Rule {
        name: "Test Product",
        vendor: None,
        kind: Some(DeviceKind::Nas),
        headers: &[],
        cookies: &[],
        body: &[],
        favicon: &[2_070_964_735],
    };

    #[test]
    fn a_favicon_can_identify_a_product_on_its_own() {
        let input = page(NO_HEADERS, NO_COOKIES, "<html>login", Some(2_070_964_735));
        assert_eq!(
            FAVICON_RULE.score(&input),
            6,
            "a favicon match alone is enough"
        );

        // Without an icon there is nothing left to match.
        let without = page(NO_HEADERS, NO_COOKIES, "<html>login", None);
        assert_eq!(FAVICON_RULE.score(&without), 0);

        // A different icon does not match either.
        let other = page(NO_HEADERS, NO_COOKIES, "<html>login", Some(1_234_567));
        assert_eq!(FAVICON_RULE.score(&other), 0);
    }

    #[test]
    fn an_unknown_page_matches_nothing() {
        let headers = [("server".to_string(), "nginx".to_string())];
        let input = page(
            &headers,
            NO_COOKIES,
            "<html><head><title>It works!</title></head></html>",
            None,
        );
        assert_eq!(matched(&input), None);
    }
}
