//! Port numbers, and the named groups a scan can ask for.
//!
//! Port lists in the settings are mixed: a literal number, or a group name such
//! as `web` or `top`. Keeping them in one place means the presets, the defaults
//! and the documentation cannot drift apart.

use serde::{Deserialize, Serialize};

/// Widely used service ports. The probe's default: broad enough to find the
/// services a LAN actually runs, not the full 65k a SYN scan would sweep.
pub const COMMON_PORTS: &[u16] = &[
    21, 22, 23, 25, 53, 69, 79, 80, 81, 88, 110, 111, 113, 119, 123, 135, 137, 138, 139, 143, 161,
    162, 179, 194, 389, 443, 445, 464, 465, 500, 514, 515, 520, 548, 554, 587, 623, 631, 636, 646,
    873, 902, 989, 990, 993, 995, 1025, 1080, 1099, 1194, 1433, 1434, 1521, 1701, 1723, 1883, 1900,
    2049, 2082, 2083, 2181, 2222, 2375, 2376, 3000, 3128, 3260, 3306, 3389, 3478, 4443, 4505, 4506,
    5000, 5001, 5060, 5061, 5222, 5269, 5353, 5357, 5432, 5555, 5601, 5672, 5683, 5900, 5901, 5984,
    5985, 5986, 6000, 6379, 6443, 6667, 7001, 7077, 8000, 8008, 8009, 8080, 8081, 8086, 8088, 8090,
    8123, 8161, 8200, 8443, 8500, 8834, 8880, 8883, 8888, 8983, 9000, 9001, 9042, 9090, 9092, 9100,
    9200, 9300, 9418, 9443, 10000, 10250, 11211, 15672, 16379, 27017, 27018, 32400, 50000, 50070,
    61616,
];

/// Web interfaces, plain and TLS.
pub const WEB_PORTS: &[u16] = &[
    80, 81, 88, 443, 591, 3000, 5000, 5001, 8000, 8008, 8080, 8081, 8088, 8090, 8123, 8161, 8443,
    8888, 9000, 9080, 9200, 9443, 10000, 15672,
];

/// Databases and the caches that behave like one.
pub const DATABASE_PORTS: &[u16] = &[
    1433, 1521, 2181, 3306, 5432, 5984, 6379, 9042, 9200, 9300, 11211, 27017, 27018,
];

/// A port list entry: a number, or one of the named groups.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PortSpec {
    /// A single port number.
    Number(u16),
    /// A group name: `web`, `db`, `top`, `all`, or a numeric string.
    Group(String),
}

impl PortSpec {
    /// The concrete ports this entry stands for. An unknown group is empty
    /// rather than an error, so one typo cannot stop a scan.
    pub fn ports(&self) -> Vec<u16> {
        match self {
            PortSpec::Number(port) => vec![*port],
            PortSpec::Group(name) => group_ports(name),
        }
    }
}

/// Expand one group name. Numeric strings count, so a hand-written setting can
/// mix `"8080"` with group names.
pub fn group_ports(name: &str) -> Vec<u16> {
    let name = name.trim().to_ascii_lowercase();
    if let Ok(port) = name.parse::<u16>() {
        return vec![port];
    }
    match name.as_str() {
        "web" | "http" | "https" => WEB_PORTS.to_vec(),
        "db" | "database" => DATABASE_PORTS.to_vec(),
        "top" | "common" | "service" | "services" => COMMON_PORTS.to_vec(),
        "all" | "full" => (1..=u16::MAX).collect(),
        _ => Vec::new(),
    }
}

/// Every group a setting may name, for docs and validation.
pub const GROUPS: [&str; 6] = ["web", "db", "top", "common", "service", "all"];

/// Expand a mixed list into a deduplicated, ascending port list.
pub fn expand(specs: &[PortSpec]) -> Vec<u16> {
    let mut ports: Vec<u16> = specs.iter().flat_map(PortSpec::ports).collect();
    ports.sort_unstable();
    ports.dedup();
    ports
}

/// Wrap a plain port list as specs, for the defaults.
pub fn numbers(ports: &[u16]) -> Vec<PortSpec> {
    ports.iter().copied().map(PortSpec::Number).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(text: &str) -> PortSpec {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn the_groups_are_non_empty_and_sorted() {
        for group in ["web", "db", "top"] {
            let ports = group_ports(group);
            assert!(!ports.is_empty(), "{group}");
            let mut sorted = ports.clone();
            sorted.sort_unstable();
            assert_eq!(ports, sorted, "{group} should be sorted");
        }
        assert_eq!(group_ports("all").len(), 65535);
    }

    #[test]
    fn unknown_groups_and_numeric_strings() {
        assert!(group_ports("nope").is_empty());
        assert_eq!(group_ports("8080"), vec![8080]);
        assert_eq!(group_ports(" Web "), WEB_PORTS.to_vec());
    }

    #[test]
    fn a_mixed_spec_list_expands_and_dedupes() {
        let specs = vec![spec("80"), spec("\"web\""), spec("\"db\"")];
        let ports = expand(&specs);
        assert!(ports.contains(&80));
        assert!(ports.contains(&3306));
        // 80 is in web and also listed first: it must appear once.
        assert_eq!(ports.iter().filter(|port| **port == 80).count(), 1);
        let mut sorted = ports.clone();
        sorted.sort_unstable();
        assert_eq!(ports, sorted);
    }
}
