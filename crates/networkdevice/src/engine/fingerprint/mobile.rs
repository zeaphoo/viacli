//! Recognising phones, tablets and the things they announce.
//!
//! A phone is the hardest thing on a LAN to name. It opens no ports, answers no
//! SNMP, usually randomises its MAC, and often has no reverse DNS. What it *does*
//! do is announce itself over mDNS — and Apple devices put their model identifier
//! in the announcement, which is as precise as a fingerprint gets:
//!
//! ```text
//! _airplay._tcp.local.  model=iPhone14,2
//! _raop._tcp.local.     am=AppleTV6,2
//! _companion-link._tcp  (iOS/iPadOS/watchOS Continuity — no model)
//! ```
//!
//! Android is chattier about *services* than about itself: `_googlecast._tcp`
//! (Chromecast, Android TV) carries `md=<model>`, and `_adb-tls-connect._tcp`
//! means a developer-enabled phone. HarmonyOS announces almost nothing, so it is
//! recognised from HONOR/HUAWEI naming and, failing that, from the vendor.
//!
//! Everything here is a *hint*: the inventory weighs it against the rest of the
//! evidence, and several of these mappings are deliberately the closest kind
//! rather than a perfect one (there is no `Watch` or `Speaker` in
//! [`DeviceKind`]).

use crate::engine::model::DeviceKind;

/// The kind an Apple model identifier implies.
///
/// The identifiers are the ones Apple itself puts in mDNS TXT records and in
/// `_device-info`: `iPhone14,2`, `iPad13,8`, `Watch7,3`, `AppleTV6,2`,
/// `AudioAccessory1,1` (HomePod), `MacBookPro18,3`, `RealityDevice14,1`.
pub fn apple_model_kind(model: &str) -> Option<DeviceKind> {
    let model = model.trim().to_ascii_lowercase();
    if model.is_empty() {
        return None;
    }
    let starts = |prefix: &str| model.starts_with(prefix);

    // Order matters: `ipad` before `ipod`, `appleTV` before `app…`.
    if starts("iphone") || starts("ipod") {
        return Some(DeviceKind::Phone);
    }
    if starts("ipad") {
        return Some(DeviceKind::Tablet);
    }
    // A watch is worn, not carried: with no `Wearable` kind, `Iot` is the honest
    // bucket rather than claiming it is a phone.
    if starts("watch") {
        return Some(DeviceKind::Iot);
    }
    if starts("appletv") {
        return Some(DeviceKind::Tv);
    }
    // HomePod: a speaker, so an appliance rather than a television.
    if starts("audioaccessory") || starts("airpods") || starts("airpod") {
        return Some(DeviceKind::Iot);
    }
    // Vision Pro runs a full desktop-class OS.
    if starts("realitydevice") {
        return Some(DeviceKind::Computer);
    }
    if starts("macbook")
        || starts("imac")
        || starts("macmini")
        || starts("macpro")
        || starts("macstudio")
        || starts("mac")
    {
        return Some(DeviceKind::Computer);
    }

    None
}

/// The kind an mDNS/DNS-SD service type implies.
///
/// Service types are lower-cased and may carry a domain suffix
/// (`_googlecast._tcp.local`), so these are substring tests.
pub fn service_kind(service: &str) -> Option<DeviceKind> {
    let service = service.trim().to_ascii_lowercase();
    let has = |needle: &str| service.contains(needle);

    // Apple Continuity: iOS, iPadOS and watchOS only — macOS does not speak it.
    if has("_companion-link") || has("_remotepairing") || has("_apple-mobdev") || has("_rdlink") {
        return Some(DeviceKind::Phone);
    }
    if has("_appletv") || has("_mediaremotetv") {
        return Some(DeviceKind::Tv);
    }
    // AirPlay receivers are televisions, speakers and Macs alike; the model
    // identifier settles it when the device sends one, which is why callers ask
    // `apple_model_kind` first.
    if has("_airplay") {
        return Some(DeviceKind::Tv);
    }
    // Remote Audio Output: HomePod, AirPort Express, third-party speakers.
    if has("_raop") {
        return Some(DeviceKind::Iot);
    }
    // Chromecast and Android TV / Google TV.
    if has("_googlecast") || has("_androidtvremote") {
        return Some(DeviceKind::Tv);
    }
    // ADB over the network: a developer-enabled phone or tablet, and rarely
    // anything else on a home LAN.
    if has("_adb") || has("_android") {
        return Some(DeviceKind::Phone);
    }
    if has("_sonos") || has("_spotify-connect") || has("_googlezone") || has("_hap") {
        return Some(DeviceKind::Iot);
    }
    if has("_ipp") || has("_ipps") || has("_printer") || has("_pdl-datastream") {
        return Some(DeviceKind::Printer);
    }
    // AirPort base station.
    if has("_airport") {
        return Some(DeviceKind::Router);
    }

    None
}

/// The kind a vendor model string implies, for the non-Apple world.
///
/// These arrive as UPnP `modelName`, mDNS TXT `md=`, or a hostname that leaked
/// into the model field. They are weaker than an Apple identifier, so the
/// inventory only reaches for them when nothing better was found.
pub fn model_kind(model: &str) -> Option<DeviceKind> {
    let model = model.trim().to_ascii_lowercase();
    if model.is_empty() {
        return None;
    }
    let has = |needle: &str| model.contains(needle);

    // A ThinkPad is a laptop; check it before the tablet patterns below, which
    // would otherwise see "pad".
    if has("thinkpad") {
        return Some(DeviceKind::Computer);
    }
    // HarmonyOS: HONOR and Huawei name their devices this way. A Huawei *router*
    // does not, which is why plain "huawei" is deliberately not a rule.
    if has("harmonyos") || has("harmony os") || has("honor") {
        return Some(DeviceKind::Phone);
    }

    // Tablets before phones: every tablet pattern below is unambiguous, and a
    // "Galaxy Tab" must not be read as a "Galaxy" phone.
    if has("ipad")
        || has("galaxy tab")
        || has("galaxytab")
        || has("matepad")
        || has("mediapad")
        || has("tablet")
        || has("sm-t")
        || has("lenovo tab")
        || has("tab ")
    {
        return Some(DeviceKind::Tablet);
    }

    // Android families, by the names their settings screens show.
    if has("android")
        || has("galaxy")
        || has("sm-")
        || has("pixel")
        || has("redmi")
        || has("poco")
        || has("oneplus")
        || has("oppo")
        || has("vivo")
        || has("realme")
        || has("xperia")
        || has("moto ")
        || has("nothing phone")
        || has("mi ")
        || has("xiaomi")
    {
        return Some(DeviceKind::Phone);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apple_identifiers_map_to_the_closest_kind() {
        assert_eq!(apple_model_kind("iPhone14,2"), Some(DeviceKind::Phone));
        assert_eq!(apple_model_kind("iPad13,8"), Some(DeviceKind::Tablet));
        assert_eq!(apple_model_kind("Watch7,3"), Some(DeviceKind::Iot));
        assert_eq!(apple_model_kind("AppleTV6,2"), Some(DeviceKind::Tv));
        assert_eq!(
            apple_model_kind("AudioAccessory5,1"),
            Some(DeviceKind::Iot),
            "HomePod is an appliance, not a television"
        );
        assert_eq!(
            apple_model_kind("MacBookPro18,3"),
            Some(DeviceKind::Computer)
        );
        assert_eq!(apple_model_kind(" iMac21,1 "), Some(DeviceKind::Computer));
        assert_eq!(
            apple_model_kind("RealityDevice14,1"),
            Some(DeviceKind::Computer)
        );
        assert_eq!(apple_model_kind("Samsung Galaxy S23"), None);
        assert_eq!(apple_model_kind(""), None);
    }

    #[test]
    fn a_tablet_is_never_reported_as_a_phone() {
        // The bug this replaced: every Apple mobile identifier was `Phone`.
        for model in ["iPad13,8", "iPad Pro", "Galaxy Tab S8", "HUAWEI MatePad 11"] {
            assert_eq!(
                apple_model_kind(model).or_else(|| model_kind(model)),
                Some(DeviceKind::Tablet),
                "{model}"
            );
        }
    }

    #[test]
    fn continuity_services_mean_an_apple_mobile_device() {
        for service in ["_companion-link._tcp.local", "_remotepairing._tcp"] {
            assert_eq!(service_kind(service), Some(DeviceKind::Phone), "{service}");
        }
        // And the media side, which is not a phone.
        assert_eq!(service_kind("_googlecast._tcp.local"), Some(DeviceKind::Tv));
        assert_eq!(service_kind("_androidtvremote2._tcp"), Some(DeviceKind::Tv));
        assert_eq!(service_kind("_raop._tcp.local"), Some(DeviceKind::Iot));
        assert_eq!(
            service_kind("_adb-tls-connect._tcp"),
            Some(DeviceKind::Phone)
        );
        assert_eq!(service_kind("_hap._tcp.local"), Some(DeviceKind::Iot));
    }

    #[test]
    fn airplay_is_not_a_router() {
        // It used to be classified as one, which is what a Mac or an Apple TV
        // sitting in the inventory as a router looks like.
        assert_ne!(
            service_kind("_airplay._tcp.local"),
            Some(DeviceKind::Router)
        );
        assert_eq!(
            service_kind("_airport._tcp.local"),
            Some(DeviceKind::Router)
        );
    }

    #[test]
    fn android_and_harmony_families_are_recognised_from_model_strings() {
        for phone in [
            "SM-G991B",
            "Pixel 7",
            "Redmi Note 12",
            "OnePlus 9",
            "HONOR 50",
        ] {
            assert_eq!(model_kind(phone), Some(DeviceKind::Phone), "{phone}");
        }
        assert_eq!(model_kind("HarmonyOS"), Some(DeviceKind::Phone));
        assert_eq!(model_kind("Lenovo Tab P11"), Some(DeviceKind::Tablet));
        assert_eq!(model_kind("ThinkPad X1 Carbon"), Some(DeviceKind::Computer));
        // A Huawei *router* must not be called a phone.
        assert_eq!(model_kind("HUAWEI WiFi AX3"), None);
        assert_eq!(model_kind(""), None);
    }
}
