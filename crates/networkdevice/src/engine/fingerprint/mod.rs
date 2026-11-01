//! Fingerprinting: what a device says about itself, and how to read it.
//!
//! Three layers, each answering a different question:
//!
//! * [`mobile`] — *what kind of thing is this?* Model identifiers, mDNS service
//!   types and vendor naming. Phones and tablets are the hard case: they open no
//!   ports and usually randomise their MAC, so the announcement is all there is;
//! * [`web`] — *what is running behind that web interface?* Rules over headers,
//!   cookies, titles, meta tags and the favicon;
//! * [`tcp`] — *what service is this port?* Probes and regex templates in the
//!   `nmap-service-probes` style, which turn a banner into a product, a version
//!   and a CPE.
pub mod mobile;
pub mod tcp;

use crate::engine::model::DeviceKind;

/// A device type as a probe file names it (`d/printer/`, `d/router/`) read as
/// one of ours. The probe table is where nmap-style fingerprints state what a
/// product *is*, which is stronger evidence than any keyword.
pub fn device_type_kind(name: &str) -> Option<DeviceKind> {
    let name = name.trim().to_ascii_lowercase();
    let has = |needle: &str| name.contains(needle);
    if has("printer") || has("print server") {
        return Some(DeviceKind::Printer);
    }
    if has("router") || has("gateway") || has("firewall") {
        return Some(DeviceKind::Router);
    }
    if has("switch") {
        return Some(DeviceKind::Switch);
    }
    if has("access point") || has("wireless") {
        return Some(DeviceKind::AccessPoint);
    }
    if has("camera") {
        return Some(DeviceKind::Camera);
    }
    if has("nas") || has("storage") {
        return Some(DeviceKind::Nas);
    }
    if has("phone") {
        return Some(DeviceKind::Phone);
    }
    if has("tablet") {
        return Some(DeviceKind::Tablet);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_file_device_types_become_kinds() {
        assert_eq!(device_type_kind("printer"), Some(DeviceKind::Printer));
        assert_eq!(device_type_kind("Router"), Some(DeviceKind::Router));
        assert_eq!(
            device_type_kind("access point"),
            Some(DeviceKind::AccessPoint)
        );
        assert_eq!(device_type_kind("NAS"), Some(DeviceKind::Nas));
        assert_eq!(device_type_kind("general purpose"), None);
    }
}
pub mod web;
