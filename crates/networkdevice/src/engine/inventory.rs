//! The IP-centric inventory: scan state per address, and the inference that
//! turns addresses into devices.
//!
//! This is the target shape of the core, kept alongside [`crate::engine::registry`]
//! while the service is built out:
//!
//! * [`Endpoint`] — one per IP: reachability **and** everything probed on it,
//!   including the MAC. Scans write here, keyed by address.
//! * [`Device`] — *inferred*: a logical identity that may own several IPs. It is
//!   never written by a scan.
//!
//! Devices are grouped from endpoints by shared evidence, strongest first:
//! a shared MAC, then a shared hostname or model. An IP belongs to at most one
//! device at a time, but a device keeps every address it has been seen at.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::net::Ipv4Addr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::engine::identify::{classify, kind_from_hints, kind_from_hostname};
use crate::engine::model::{
    DeviceKind, DiscoverySource, Label, MacAddr, Observation, ObservationInfo, PortInfo,
    ProbeTarget,
};
use crate::engine::oui::OuiDb;

/// Layer 2 evidence gathered about one address.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Fingerprint {
    /// Hardware address. Part of the fingerprint, not of the device identity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mac: Option<MacAddr>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor: Option<Label>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os: Option<Label>,
    /// Kind judged from everything probed on this address.
    pub kind: DeviceKind,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hostnames: Vec<Label>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<Label>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<PortInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
    pub identified: bool,
    /// When the probe last described this address. `None` means never; the
    /// setting decides whether an old timestamp makes it due again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identified_at: Option<DateTime<Utc>>,
}

impl Fingerprint {
    pub fn hostname(&self) -> Option<&str> {
        self.hostnames.last().map(|label| label.value.as_str())
    }

    pub fn model(&self) -> Option<&str> {
        self.models.last().map(|label| label.value.as_str())
    }

    /// Fold announced hints and the MAC in. Evidence accumulates; nothing is
    /// overwritten except one entry per value.
    /// Judge the kind from everything currently known, and remember it.
    ///
    /// Only ever answers when the kind is still [`DeviceKind::Unknown`]: what a
    /// device announced (or an operator decided) is not overwritten by a later,
    /// weaker guess. Order of evidence:
    ///
    /// 1. what the device announced — `_ipp` is a printer whatever the OUI says;
    /// 2. the hostname, when it names a kind;
    /// 3. the classifier, over MAC, vendor, hostname, interface and ports.
    ///
    /// Returns the kind it settled on, so callers can record an event.
    fn classify_kind(
        &mut self,
        iface: Option<&str>,
        hints: Option<&ObservationInfo>,
    ) -> Option<DeviceKind> {
        if self.kind != DeviceKind::Unknown {
            return None;
        }

        // A rule that recognised the product outright beats every keyword.
        let kind = hints
            .and_then(|info| info.kind)
            .or_else(|| kind_from_hints(hints))
            .or_else(|| {
                hints
                    .and_then(|info| info.hostname.as_deref())
                    .and_then(kind_from_hostname)
            })
            .or_else(|| {
                let ports: Vec<u16> = self.ports.iter().map(|port| port.port).collect();
                let judged = classify(
                    self.mac.as_ref(),
                    self.vendor.as_ref().map(|label| label.value.as_str()),
                    self.hostname(),
                    iface,
                    &ports,
                );
                (judged != DeviceKind::Unknown).then_some(judged)
            });

        if let Some(kind) = kind {
            self.kind = kind;
        }
        kind
    }

    /// Fold one sighting in. Returns what it added, so the caller can record an
    /// event: an empty result means the sighting told us nothing new.
    pub fn absorb(&mut self, obs: &Observation, now: DateTime<Utc>) -> Vec<String> {
        let mut gained = Vec::new();

        if let Some(mac) = obs.mac {
            self.mac = Some(mac);
        }
        if let Some(info) = obs.info.as_ref() {
            if let Some(host) = info
                .hostname
                .as_deref()
                .map(str::trim)
                .filter(|h| !h.is_empty())
            {
                push_label(&mut self.hostnames, host, obs.source, now);
            }
            if let Some(model) = info
                .model
                .as_deref()
                .map(str::trim)
                .filter(|m| !m.is_empty())
            {
                push_label(&mut self.models, model, obs.source, now);
            }
            for service in info.normalized_services() {
                if !self.services.contains(&service) {
                    self.services.push(service);
                }
            }
            if self.vendor.is_none()
                && let Some(vendor) = info
                    .vendor
                    .as_deref()
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
            {
                self.vendor = Some(Label {
                    value: vendor.to_string(),
                    source: obs.source,
                    at: Some(now),
                });
                gained.push(format!("vendor={vendor}"));
            }
            if !info.ports.is_empty() {
                let before: Vec<u16> = self.ports.iter().map(|port| port.port).collect();
                let mut ports = info.ports.clone();
                ports.sort_by_key(|port| port.port);
                let after: Vec<u16> = ports.iter().map(|port| port.port).collect();
                if after != before {
                    gained.push(format!("ports={after:?}"));
                    self.ports = ports;
                }
            }
            if let Some(kind) = self.classify_kind(obs.iface.as_deref(), Some(info)) {
                gained.push(format!("kind={kind}"));
            }
        }

        // The probe is what closes out an address: it is the only source whose
        // job is to describe rather than to find.
        if obs.source == DiscoverySource::Probe {
            self.identified = true;
            self.identified_at = Some(now);
        }

        self.updated_at = Some(now);
        gained
    }
}

/// Layer 1: one address, its liveness, and its fingerprint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Endpoint {
    pub ip: Ipv4Addr,
    pub online: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iface: Option<String>,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    pub sightings: u64,
    pub sources: Vec<DiscoverySource>,
    pub fingerprint: Fingerprint,
}

impl Endpoint {
    fn new(obs: &Observation, now: DateTime<Utc>) -> Self {
        Endpoint {
            ip: obs.ip,
            online: true,
            iface: obs.iface.clone(),
            first_seen: now,
            last_seen: now,
            sightings: 1,
            sources: vec![obs.source],
            fingerprint: Fingerprint::default(),
        }
    }
}

/// How a device's addresses were grouped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MergedBy {
    /// A shared hardware address.
    Mac,
    /// A shared hostname.
    Hostname,
    /// A shared model string.
    Model,
    /// A single address: no merge happened (yet).
    Address,
}

impl MergedBy {
    pub fn label(&self) -> &'static str {
        match self {
            MergedBy::Mac => "mac",
            MergedBy::Hostname => "hostname",
            MergedBy::Model => "model",
            MergedBy::Address => "address",
        }
    }
}

/// Layer 3: a device inferred from one or more addresses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Device {
    /// Six letters + six digits, stable across passes.
    pub id: String,
    pub ips: Vec<Ipv4Addr>,
    pub kind: DeviceKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub known: bool,
    pub online: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merged_by: Option<MergedBy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
}

impl Device {
    pub fn is_new(&self, now: DateTime<Utc>, within: chrono::Duration) -> bool {
        !self.known && now - self.first_seen <= within
    }
}

/// What kind of transition an event records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EventKind {
    /// Seen for the very first time.
    Discovered,
    /// Came back after being offline.
    Online,
    /// Missed by a scan.
    Offline,
    /// Identification produced new information.
    Identified,
    /// Operator edited the record.
    Edited,
}

impl EventKind {
    pub fn label(&self) -> &'static str {
        match self {
            EventKind::Discovered => "discovered",
            EventKind::Online => "online",
            EventKind::Offline => "offline",
            EventKind::Identified => "identified",
            EventKind::Edited => "edited",
        }
    }
}

/// One entry in the change timeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub seq: u64,
    pub at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip: Option<Ipv4Addr>,
    pub kind: EventKind,
    pub detail: String,
}

/// What one observation did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ObserveOutcome {
    pub first_seen: bool,
    pub came_online: bool,
}

/// The IP-centric inventory.
#[derive(Debug, Clone, Default)]
pub struct Inventory {
    endpoints: BTreeMap<Ipv4Addr, Endpoint>,
    devices: BTreeMap<String, Device>,
    events: Vec<Event>,
    next_event_seq: u64,
}

impl Inventory {
    /// Addresses the probe should describe next.
    ///
    /// Only addresses currently seen alive are worth probing, and only those the
    /// probe has not described yet — or described longer ago than `ttl_secs`.
    /// `None` means "probe once": a device that later opens a port is only
    /// revisited when a TTL is configured.
    pub fn probe_targets(
        &self,
        now: DateTime<Utc>,
        ttl_secs: Option<u64>,
        limit: usize,
    ) -> Vec<ProbeTarget> {
        let due = |fingerprint: &Fingerprint| match ttl_secs {
            None => !fingerprint.identified,
            Some(ttl) => fingerprint
                .identified_at
                .is_none_or(|at| (now - at).num_seconds() >= ttl as i64),
        };

        self.endpoints
            .values()
            .filter(|endpoint| endpoint.online && due(&endpoint.fingerprint))
            .take(limit)
            .map(|endpoint| ProbeTarget {
                ip: endpoint.ip,
                mac: endpoint.fingerprint.mac,
                iface: endpoint.iface.clone(),
            })
            .collect()
    }

    pub fn new() -> Self {
        // Event sequences start at 1: `seq > 0` means "everything".
        Inventory {
            next_event_seq: 1,
            ..Default::default()
        }
    }

    /// Resume from persisted state; events keep counting from `event_seq`.
    pub fn resume(endpoints: Vec<Endpoint>, devices: Vec<Device>, event_seq: u64) -> Self {
        Inventory {
            endpoints: endpoints.into_iter().map(|e| (e.ip, e)).collect(),
            devices: devices.into_iter().map(|d| (d.id.clone(), d)).collect(),
            events: Vec::new(),
            next_event_seq: event_seq,
        }
    }

    pub fn endpoints(&self) -> impl Iterator<Item = &Endpoint> {
        self.endpoints.values()
    }

    pub fn endpoint(&self, ip: Ipv4Addr) -> Option<&Endpoint> {
        self.endpoints.get(&ip)
    }

    pub fn devices(&self) -> impl Iterator<Item = &Device> {
        self.devices.values()
    }

    pub fn device(&self, id: &str) -> Option<&Device> {
        self.devices.get(id)
    }

    pub fn device_for_ip(&self, ip: Ipv4Addr) -> Option<&Device> {
        self.devices
            .values()
            .find(|device| device.ips.contains(&ip))
    }

    pub fn events(&self) -> &[Event] {
        &self.events
    }

    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    pub fn event_seq(&self) -> u64 {
        self.next_event_seq
    }

    /// Record a sighting: update the address, then re-infer the devices.
    pub fn observe(&mut self, obs: &Observation, now: DateTime<Utc>) -> ObserveOutcome {
        let first_seen = !self.endpoints.contains_key(&obs.ip);
        let (came_online, gained) = match self.endpoints.get_mut(&obs.ip) {
            Some(endpoint) => {
                let came_online = !endpoint.online;
                endpoint.online = true;
                endpoint.last_seen = now;
                endpoint.sightings += 1;
                if !endpoint.sources.contains(&obs.source) {
                    endpoint.sources.push(obs.source);
                }
                if obs.iface.is_some() {
                    endpoint.iface = obs.iface.clone();
                }
                let gained = endpoint.fingerprint.absorb(obs, now);
                (came_online, gained)
            }
            None => {
                let mut endpoint = Endpoint::new(obs, now);
                let gained = endpoint.fingerprint.absorb(obs, now);
                self.endpoints.insert(obs.ip, endpoint);
                (true, gained)
            }
        };

        self.infer(now);

        if first_seen {
            let detail = match self.device_for_ip(obs.ip) {
                Some(device) => format!("{} {}", obs.ip, device.id),
                None => obs.ip.to_string(),
            };
            let device = self.device_for_ip(obs.ip).map(|device| device.id.clone());
            self.push_event(now, device, Some(obs.ip), EventKind::Discovered, detail);
        } else if came_online {
            let device = self.device_for_ip(obs.ip).map(|device| device.id.clone());
            self.push_event(
                now,
                device,
                Some(obs.ip),
                EventKind::Online,
                obs.ip.to_string(),
            );
        }

        // A sighting that added something is the timeline's "identified" entry.
        // It used to come from `apply_identification`, which wrote to the
        // inventory directly; now every source goes through here, and only a
        // real gain is worth an event — a periodic re-probe that learns nothing
        // new must stay silent.
        if !gained.is_empty() {
            let device = self.device_for_ip(obs.ip).map(|device| device.id.clone());
            self.push_event(
                now,
                device,
                Some(obs.ip),
                EventKind::Identified,
                gained.join(" "),
            );
        }

        ObserveOutcome {
            first_seen,
            came_online,
        }
    }

    /// Name every vendor we can, now that the MAC is known.
    ///
    /// The OUI registry answers from the hardware address alone, so this is a
    /// derivation rather than a scan: it runs for every address that has a MAC,
    /// whether that MAC came from the ARP sweep, the neighbour table, mDNS or a
    /// probe. Doing it here means a new device shows its vendor immediately
    /// instead of waiting for the probe's next visit.
    ///
    /// Returns how many vendors were filled in.
    pub fn apply_oui(&mut self, oui: &OuiDb, now: DateTime<Utc>) -> usize {
        let mut filled = 0;
        for endpoint in self.endpoints.values_mut() {
            let fingerprint = &mut endpoint.fingerprint;
            if fingerprint.vendor.is_some() {
                continue;
            }
            let Some(mac) = fingerprint.mac else {
                continue;
            };
            let Some(vendor) = oui.lookup(&mac).map(str::to_string) else {
                continue;
            };
            fingerprint.vendor = Some(Label {
                value: vendor,
                source: DiscoverySource::Oui,
                at: Some(now),
            });
            // A vendor is exactly what the classifier's vendor buckets want, so
            // an address that was Unknown may not be any more.
            fingerprint.classify_kind(endpoint.iface.as_deref(), None);
            filled += 1;
        }
        filled
    }

    /// Close out a scan: anything not in `seen` is offline.
    pub fn end_scan(&mut self, seen: &BTreeSet<Ipv4Addr>, now: DateTime<Utc>) -> Vec<Ipv4Addr> {
        let mut disappeared = Vec::new();
        for (ip, endpoint) in self.endpoints.iter_mut() {
            if seen.contains(ip) || !endpoint.online {
                continue;
            }
            endpoint.online = false;
            disappeared.push(*ip);
        }
        self.infer(now);
        disappeared
    }

    /// Time-based offline rule for the multi-source scheduler.
    pub fn prune_stale(&mut self, cutoff: DateTime<Utc>, now: DateTime<Utc>) -> Vec<Ipv4Addr> {
        let mut disappeared = Vec::new();
        for (ip, endpoint) in self.endpoints.iter_mut() {
            if !endpoint.online || endpoint.last_seen >= cutoff {
                continue;
            }
            endpoint.online = false;
            disappeared.push(*ip);
        }
        if !disappeared.is_empty() {
            self.infer(now);
        }
        disappeared
    }

    /// Operator action: name a device.
    pub fn set_name(&mut self, id: &str, name: Option<String>) -> bool {
        let Some(device) = self.devices.get_mut(id) else {
            return false;
        };
        let name = name.filter(|n| !n.trim().is_empty());
        if device.name == name {
            return false;
        }
        device.name = name;
        true
    }

    /// Operator action: trust/untrust a device.
    pub fn set_known(&mut self, id: &str, known: bool) -> bool {
        let Some(device) = self.devices.get_mut(id) else {
            return false;
        };
        if device.known == known {
            return false;
        }
        device.known = known;
        true
    }

    /// Remove a device and its addresses from the inventory.
    ///
    /// The endpoints go too: otherwise the very next inference pass would
    /// rebuild the device from the stale address even though it was never seen
    /// again. A device that is still alive is simply rediscovered by the next
    /// scan, as a new device.
    pub fn forget(&mut self, id: &str) -> bool {
        let Some(device) = self.devices.remove(id) else {
            return false;
        };
        for ip in &device.ips {
            self.endpoints.remove(ip);
        }
        true
    }

    // ---------------------------------------------------------------- inference

    /// Recompute the device set from the endpoints.
    ///
    /// Grouping is strongest-first — shared MAC, then shared hostname, then
    /// shared model — and every group keeps the identity of the existing device
    /// it overlaps, so ids, names and the trusted flag survive.
    pub fn infer(&mut self, now: DateTime<Utc>) {
        let ips: Vec<Ipv4Addr> = self.endpoints.keys().copied().collect();
        let mut dsu = Dsu::new(ips.len());

        // 1. shared MAC (strong).
        let mut by_mac: HashMap<MacAddr, usize> = HashMap::new();
        for (i, ip) in ips.iter().enumerate() {
            let Some(mac) = self.endpoints[ip].fingerprint.mac else {
                continue;
            };
            match by_mac.get(&mac) {
                Some(&first) => dsu.union(i, first),
                None => {
                    by_mac.insert(mac, i);
                }
            }
        }

        // 2. shared hostname, then model (weaker).
        for key in [GroupKey::Hostname, GroupKey::Model] {
            let mut seen: HashMap<String, usize> = HashMap::new();
            for (i, ip) in ips.iter().enumerate() {
                let Some(value) = key.value(&self.endpoints[ip].fingerprint) else {
                    continue;
                };
                match seen.get(&value) {
                    Some(&first) => dsu.union(i, first),
                    None => {
                        seen.insert(value, i);
                    }
                }
            }
        }

        // Collect groups, and which merge reason produced each.
        let mut groups: BTreeMap<usize, Vec<Ipv4Addr>> = BTreeMap::new();
        for (i, ip) in ips.iter().enumerate() {
            groups.entry(dsu.find(i)).or_default().push(*ip);
        }

        // Match each group to at most one existing device, by overlap.
        let mut claimed: BTreeMap<String, Vec<Ipv4Addr>> = BTreeMap::new();
        let mut assigned: BTreeSet<String> = BTreeSet::new();
        let existing: Vec<Device> = self.devices.values().cloned().collect();

        for group in groups.values() {
            let members: BTreeSet<Ipv4Addr> = group.iter().copied().collect();
            let mut best: Option<&Device> = None;
            for device in &existing {
                if assigned.contains(&device.id) {
                    continue;
                }
                if device.ips.iter().any(|ip| members.contains(ip)) {
                    // Prefer the oldest device so identity is sticky.
                    match best {
                        Some(current) if current.first_seen <= device.first_seen => {}
                        _ => best = Some(device),
                    }
                }
            }

            match best {
                Some(device) => {
                    assigned.insert(device.id.clone());
                    claimed.insert(device.id.clone(), group.clone());
                }
                None => {
                    // A brand new group: allocate an id below.
                    let id = format!("__new__{}", claimed.len());
                    claimed.insert(id, group.clone());
                }
            }
        }

        let mut rebuilt: BTreeMap<String, Device> = BTreeMap::new();
        for (id, members) in claimed.iter() {
            let previous = self.devices.get(id);
            let mut device = self.promote(members, now);
            if let Some(previous) = previous {
                device.id = previous.id.clone();
                device.name = previous.name.clone();
                device.known = previous.known;
                device.first_seen = previous.first_seen.min(device.first_seen);
            } else {
                device.id = self.fresh_device_id();
            }
            rebuilt.insert(device.id.clone(), device);
        }

        self.devices = rebuilt;
    }

    /// An id no device in the inventory is using.
    fn fresh_device_id(&self) -> String {
        loop {
            let id = crate::engine::ids::device_id();
            if !self.devices.contains_key(&id) {
                return id;
            }
        }
    }

    /// Build the device a set of addresses implies.
    fn promote(&self, members: &[Ipv4Addr], _now: DateTime<Utc>) -> Device {
        let mut ips = members.to_vec();
        ips.sort();

        let first_seen = ips
            .iter()
            .filter_map(|ip| self.endpoints.get(ip))
            .map(|endpoint| endpoint.first_seen)
            .min()
            .unwrap_or_else(Utc::now);
        let last_seen = ips
            .iter()
            .filter_map(|ip| self.endpoints.get(ip))
            .map(|endpoint| endpoint.last_seen)
            .max()
            .unwrap_or(first_seen);
        let online = ips
            .iter()
            .filter_map(|ip| self.endpoints.get(ip))
            .any(|endpoint| endpoint.online);

        let kind = ips
            .iter()
            .filter_map(|ip| self.endpoints.get(ip))
            .map(|endpoint| endpoint_kind(&endpoint.fingerprint))
            .find(|kind| *kind != DeviceKind::Unknown)
            .unwrap_or(DeviceKind::Unknown);

        let merged_by = self.merge_reason(&ips);

        Device {
            id: String::new(),
            ips,
            kind,
            name: None,
            known: false,
            online,
            merged_by,
            confidence: None,
            first_seen,
            last_seen,
        }
    }

    fn merge_reason(&self, ips: &[Ipv4Addr]) -> Option<MergedBy> {
        if ips.len() < 2 {
            return Some(MergedBy::Address);
        }
        let macs: BTreeSet<MacAddr> = ips
            .iter()
            .filter_map(|ip| self.endpoints.get(ip))
            .filter_map(|endpoint| endpoint.fingerprint.mac)
            .collect();
        if macs.len() == 1
            && ips
                .iter()
                .filter_map(|ip| self.endpoints.get(ip))
                .all(|endpoint| endpoint.fingerprint.mac.is_some())
        {
            return Some(MergedBy::Mac);
        }
        let hostnames: BTreeSet<String> = ips
            .iter()
            .filter_map(|ip| {
                self.endpoints
                    .get(ip)
                    .and_then(|e| e.fingerprint.hostname())
            })
            .map(|h| h.to_ascii_lowercase())
            .collect();
        if hostnames.len() == 1 {
            return Some(MergedBy::Hostname);
        }
        let models: BTreeSet<String> = ips
            .iter()
            .filter_map(|ip| self.endpoints.get(ip).and_then(|e| e.fingerprint.model()))
            .map(|m| m.to_ascii_lowercase())
            .collect();
        if models.len() == 1 {
            return Some(MergedBy::Model);
        }
        None
    }

    fn push_event(
        &mut self,
        at: DateTime<Utc>,
        device: Option<String>,
        ip: Option<Ipv4Addr>,
        kind: EventKind,
        detail: String,
    ) {
        self.events.push(Event {
            seq: self.next_event_seq,
            at,
            device,
            ip,
            kind,
            detail,
        });
        self.next_event_seq += 1;
    }
}

/// Which evidence a grouping pass keys on.
#[derive(Clone, Copy)]
enum GroupKey {
    Hostname,
    Model,
}

impl GroupKey {
    fn value(self, fingerprint: &Fingerprint) -> Option<String> {
        let value = match self {
            GroupKey::Hostname => fingerprint.hostname()?,
            GroupKey::Model => fingerprint.model()?,
        };
        Some(value.trim().to_ascii_lowercase())
    }
}

/// The device kind a fingerprint implies on its own.
fn endpoint_kind(fingerprint: &Fingerprint) -> DeviceKind {
    if fingerprint.kind != DeviceKind::Unknown {
        return fingerprint.kind;
    }
    if let Some(kind) = kind_from_hostname(fingerprint.hostname().unwrap_or_default()) {
        return kind;
    }
    let info = ObservationInfo {
        hostname: fingerprint.hostname().map(str::to_string),
        model: fingerprint.model().map(str::to_string),
        services: fingerprint.services.clone(),
        ..Default::default()
    };
    crate::engine::identify::kind_from_hints(Some(&info)).unwrap_or(DeviceKind::Unknown)
}

fn push_label(labels: &mut Vec<Label>, value: &str, source: DiscoverySource, now: DateTime<Utc>) {
    let value = value.trim();
    labels.retain(|label| !label.value.eq_ignore_ascii_case(value));
    labels.push(Label {
        value: value.to_string(),
        source,
        at: Some(now),
    });
}

/// Disjoint-set union over address indices.
struct Dsu {
    parent: Vec<usize>,
}

impl Dsu {
    fn new(len: usize) -> Self {
        Dsu {
            parent: (0..len).collect(),
        }
    }

    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }

    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.parent[ra] = rb;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::model::ObservationInfo;

    fn t(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000 + secs, 0).unwrap()
    }

    fn ip(s: &str) -> Ipv4Addr {
        s.parse().unwrap()
    }

    fn mac(s: &str) -> MacAddr {
        s.parse().unwrap()
    }

    fn arp(ip_s: &str, mac_s: &str) -> Observation {
        Observation::from_mac(mac(mac_s), ip(ip_s), Some("eth0".into()))
            .with_source(DiscoverySource::NeighborTable)
    }

    fn mdns(ip_s: &str, hostname: &str, model: Option<&str>) -> Observation {
        Observation::from_ip(ip(ip_s), Some("eth0".into()))
            .with_source(DiscoverySource::Mdns)
            .with_info(ObservationInfo {
                hostname: Some(hostname.to_string()),
                model: model.map(str::to_string),
                services: Vec::new(),
                ..Default::default()
            })
    }

    /// The mobile case end to end: an address that announces itself over mDNS
    /// and nothing else. This is the whole signal a modern phone gives, so the
    /// classification has to come out of the model identifier.
    #[test]
    fn an_announced_ipad_is_a_tablet_and_an_iphone_a_phone() {
        let mut inv = Inventory::new();
        inv.observe(&mdns("192.168.3.51", "iPhone", Some("iPhone14,2")), t(0));
        inv.observe(&mdns("192.168.3.52", "iPad", Some("iPad13,8")), t(0));
        inv.observe(&mdns("192.168.3.53", "tv", Some("AppleTV6,2")), t(0));
        inv.observe(&mdns("192.168.3.54", "phone", Some("HONOR ELP-NX9")), t(0));

        let kind = |ip_s: &str| {
            inv.device_for_ip(ip(ip_s))
                .map(|device| device.kind)
                .expect("a device")
        };
        assert_eq!(kind("192.168.3.51"), DeviceKind::Phone);
        assert_eq!(
            kind("192.168.3.52"),
            DeviceKind::Tablet,
            "an iPad is not a phone"
        );
        assert_eq!(kind("192.168.3.53"), DeviceKind::Tv);
        assert_eq!(kind("192.168.3.54"), DeviceKind::Phone);
    }

    #[test]
    fn an_address_becomes_a_device() {
        let mut inv = Inventory::new();
        let outcome = inv.observe(&arp("192.168.3.96", "d0:57:7e:35:f5:2f"), t(0));
        assert!(outcome.first_seen);

        let device = inv.device_for_ip(ip("192.168.3.96")).unwrap();
        assert_eq!(device.ips, vec![ip("192.168.3.96")]);
        assert!(crate::engine::ids::is_kind(&device.id, crate::engine::ids::Kind::Device));
        assert_eq!(inv.endpoints().count(), 1);
    }

    #[test]
    fn one_mac_ties_two_addresses_to_one_device() {
        let mut inv = Inventory::new();
        inv.observe(&arp("10.0.0.5", "aa:bb:cc:00:00:01"), t(0));
        inv.observe(&arp("10.0.0.9", "aa:bb:cc:00:00:01"), t(10));

        assert_eq!(inv.devices().count(), 1);
        let device = inv.devices().next().unwrap();
        assert_eq!(device.ips, vec![ip("10.0.0.5"), ip("10.0.0.9")]);
        assert_eq!(device.merged_by, Some(MergedBy::Mac));
    }

    #[test]
    fn a_shared_hostname_ties_addresses_without_macs() {
        let mut inv = Inventory::new();
        inv.observe(&mdns("192.168.3.164", "ChenJieiPad.local", None), t(0));
        inv.observe(&mdns("192.168.3.168", "ChenJieiPad.local", None), t(10));

        assert_eq!(inv.devices().count(), 1);
        let device = inv.devices().next().unwrap();
        assert_eq!(device.ips.len(), 2);
        assert_eq!(device.merged_by, Some(MergedBy::Hostname));
    }

    #[test]
    fn a_dhcp_change_keeps_the_device_id() {
        let mut inv = Inventory::new();
        inv.observe(&arp("10.0.0.5", "aa:bb:cc:00:00:01"), t(0));
        let id = inv.devices().next().unwrap().id.clone();

        // The lease moves; the MAC is the same.
        inv.observe(&arp("10.0.0.9", "aa:bb:cc:00:00:01"), t(10));
        assert_eq!(inv.devices().count(), 1);
        assert_eq!(inv.devices().next().unwrap().id, id);
    }

    #[test]
    fn different_devices_sharing_an_address_stay_separate() {
        let mut inv = Inventory::new();
        inv.observe(&arp("10.0.0.5", "aa:bb:cc:00:00:01"), t(0));
        inv.observe(&arp("10.0.0.9", "aa:bb:cc:00:00:02"), t(10));

        assert_eq!(inv.devices().count(), 2);
    }

    #[test]
    fn devices_get_random_prefixed_ids() {
        let mut inv = Inventory::new();
        inv.observe(&arp("10.0.0.1", "aa:bb:cc:00:00:01"), t(0));
        inv.observe(&arp("10.0.0.2", "aa:bb:cc:00:00:02"), t(0));

        let ids: Vec<&str> = inv.devices().map(|d| d.id.as_str()).collect();
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1]);
        for id in &ids {
            assert!(crate::engine::ids::is_kind(id, crate::engine::ids::Kind::Device), "{id}");
        }
        // Nothing ties id to creation order: they come from the random source
        // (`ids.rs`), not from a counter, so there is no property left to
        // assert here beyond uniqueness and shape.
    }

    #[test]
    fn operator_markings_survive_inference() {
        let mut inv = Inventory::new();
        inv.observe(&arp("10.0.0.5", "aa:bb:cc:00:00:01"), t(0));
        let id = inv.devices().next().unwrap().id.clone();
        assert!(inv.set_name(&id, Some("desk".into())));
        assert!(inv.set_known(&id, true));

        // A second address joins the device, forcing a rebuild.
        inv.observe(&arp("10.0.0.9", "aa:bb:cc:00:00:01"), t(10));

        let device = inv.device(&id).unwrap();
        assert_eq!(device.name.as_deref(), Some("desk"));
        assert!(device.known);
        assert_eq!(device.ips.len(), 2);
    }

    #[test]
    fn going_offline_and_back_is_tracked() {
        let mut inv = Inventory::new();
        inv.observe(&arp("10.0.0.5", "aa:bb:cc:00:00:01"), t(0));
        let seen = BTreeSet::new();
        assert_eq!(inv.end_scan(&seen, t(60)), vec![ip("10.0.0.5")]);
        assert!(!inv.device_for_ip(ip("10.0.0.5")).unwrap().online);

        let outcome = inv.observe(&arp("10.0.0.5", "aa:bb:cc:00:00:01"), t(120));
        assert!(outcome.came_online);
        assert!(!outcome.first_seen);
    }

    /// A probe sighting, exactly as `probe.rs` emits it.
    fn probed(ip_s: &str, mac_s: &str) -> Observation {
        Observation::from_mac(mac(mac_s), ip(ip_s), Some("eth0".into()))
            .with_source(DiscoverySource::Probe)
            .with_info(ObservationInfo {
                ports: vec![PortInfo {
                    port: 80,
                    service: Some("http".into()),
                    banner: None,
                }],
                ..Default::default()
            })
    }

    #[test]
    fn only_live_and_due_addresses_are_probed() {
        let mut inv = Inventory::new();
        inv.observe(&arp("10.0.0.1", "aa:bb:cc:00:00:01"), t(0));
        inv.observe(&arp("10.0.0.2", "aa:bb:cc:00:00:02"), t(0));

        // Nothing probed yet: both are due, and a budget bounds one cycle.
        assert_eq!(inv.probe_targets(t(1), None, 10).len(), 2);
        assert_eq!(inv.probe_targets(t(1), None, 1).len(), 1);

        // An address that is currently offline is not worth a port scan:
        // `end_scan` takes the addresses that *were* seen.
        inv.end_scan(&BTreeSet::from([ip("10.0.0.2")]), t(60));
        let targets = inv.probe_targets(t(61), None, 10);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].ip, ip("10.0.0.2"));

        // Probing it makes it "done"; with no TTL an address is probed once ever.
        inv.observe(&probed("10.0.0.2", "aa:bb:cc:00:00:02"), t(120));
        assert!(inv.probe_targets(t(121), None, 10).is_empty());

        // A TTL brings it back, but only once the stamp is old enough.
        assert!(
            inv.probe_targets(t(200), Some(600), 10).is_empty(),
            "still fresh"
        );
        assert_eq!(inv.probe_targets(t(1000), Some(600), 10).len(), 1, "stale");
    }

    #[test]
    fn identification_lands_on_the_endpoint_and_events_reference_the_device() {
        let mut inv = Inventory::new();
        inv.observe(&arp("10.0.0.5", "aa:bb:cc:00:00:01"), t(0));
        let id = inv.devices().next().unwrap().id.clone();

        // What the probe emits: one observation carrying everything it learned.
        let probed = Observation::from_mac(mac("aa:bb:cc:00:00:01"), ip("10.0.0.5"), None)
            .with_source(DiscoverySource::Probe)
            .with_info(ObservationInfo {
                hostname: Some("desk.lan".into()),
                vendor: Some("Intel Corporate".into()),
                ports: vec![
                    PortInfo {
                        port: 22,
                        service: None,
                        banner: None,
                    },
                    PortInfo {
                        port: 80,
                        service: None,
                        banner: None,
                    },
                ],
                ..Default::default()
            });
        inv.observe(&probed, t(10));

        let endpoint = inv.endpoint(ip("10.0.0.5")).unwrap();
        assert!(
            endpoint.fingerprint.identified_at.is_some(),
            "the probe closes out the address"
        );
        assert_eq!(
            endpoint.fingerprint.vendor.as_ref().unwrap().value,
            "Intel Corporate"
        );
        assert_eq!(endpoint.fingerprint.ports.len(), 2);
        assert_eq!(inv.device(&id).unwrap().kind, DeviceKind::Computer);

        // The identification event names the device, not just the address.
        assert!(
            inv.events()
                .iter()
                .any(|event| event.kind == EventKind::Identified
                    && event.device.as_deref() == Some(id.as_str()))
        );
    }

    #[test]
    fn the_vendor_comes_from_the_mac_without_any_probe() {
        let mut inv = Inventory::new();
        // A neighbour-table sighting: an address and a MAC, nothing else.
        inv.observe(&arp("10.0.0.5", "44:03:2c:af:69:0f"), t(0));

        let endpoint = inv.endpoint(ip("10.0.0.5")).unwrap();
        assert!(
            endpoint.fingerprint.vendor.is_none(),
            "nothing looked it up yet"
        );
        assert_eq!(endpoint.fingerprint.kind, DeviceKind::Unknown);

        // The lookup needs no scan and no probe: only the MAC.
        assert_eq!(inv.apply_oui(OuiDb::embedded(), t(1)), 1);
        let fingerprint = &inv.endpoint(ip("10.0.0.5")).unwrap().fingerprint;
        let vendor = fingerprint.vendor.as_ref().expect("vendor");
        assert_eq!(vendor.source, DiscoverySource::Oui);
        assert!(vendor.value.contains("Intel"), "{}", vendor.value);
        assert_eq!(
            fingerprint.kind,
            DeviceKind::Computer,
            "the vendor bucket decides, now that the vendor is known"
        );

        // Idempotent, and it never overwrites what is already there.
        assert_eq!(inv.apply_oui(OuiDb::embedded(), t(2)), 0);

        // An address with no MAC (an L3 scan across a router) stays unnamed.
        inv.observe(&Observation::new(None, ip("10.9.9.9"), None), t(3));
        inv.apply_oui(OuiDb::embedded(), t(4));
        assert!(
            inv.endpoint(ip("10.9.9.9"))
                .unwrap()
                .fingerprint
                .vendor
                .is_none()
        );
    }

    #[test]
    fn announced_services_beat_the_vendor_bucket() {
        // Shenzhen Bilian sits in the IoT vendor bucket, but a printer that
        // advertises `_ipp` is a printer.
        let mut inv = Inventory::new();
        let obs = Observation::from_mac(
            mac("14:0a:02:20:71:ba"),
            ip("192.168.3.138"),
            Some("eth0".into()),
        )
        .with_source(DiscoverySource::Mdns)
        .with_info(ObservationInfo {
            hostname: Some("Pantum-2071BA.local".into()),
            model: None,
            services: vec!["_ipp._tcp.local".into()],
            ..Default::default()
        });
        inv.observe(&obs, t(0));
        assert_eq!(
            inv.endpoint(ip("192.168.3.138")).unwrap().fingerprint.kind,
            DeviceKind::Printer
        );

        // A vendor-only sighting must not overwrite it, whichever source it
        // comes from.
        let vendor_only =
            Observation::from_mac(mac("14:0a:02:20:71:ba"), ip("192.168.3.138"), None)
                .with_source(DiscoverySource::Probe)
                .with_info(ObservationInfo {
                    vendor: Some("SHENZHEN BILIAN ELECTRONIC CO.,LTD".into()),
                    ..Default::default()
                });
        inv.observe(&vendor_only, t(10));
        assert_eq!(
            inv.endpoint(ip("192.168.3.138")).unwrap().fingerprint.kind,
            DeviceKind::Printer
        );
    }

    #[test]
    fn a_forgotten_device_does_not_resurrect() {
        let mut inv = Inventory::new();
        inv.observe(&arp("10.0.0.5", "aa:bb:cc:00:00:01"), t(0));
        let id = inv.devices().next().unwrap().id.clone();

        assert!(inv.forget(&id));
        assert!(inv.device(&id).is_none());
        assert!(inv.endpoint(ip("10.0.0.5")).is_none());

        // Unrelated traffic must not rebuild it from a stale address.
        inv.observe(&arp("10.0.0.9", "aa:bb:cc:00:00:02"), t(10));
        assert!(inv.device(&id).is_none());
        assert_eq!(inv.devices().count(), 1);
    }

    #[test]
    fn inference_does_not_merge_on_services_alone() {
        // Two TVs exposing the same service must not become one device.
        let mut inv = Inventory::new();
        for (ip_s, host) in [("10.0.0.1", "tv-one"), ("10.0.0.2", "tv-two")] {
            let mut obs = mdns(ip_s, host, None);
            if let Some(info) = obs.info.as_mut() {
                info.services = vec!["_airplay._tcp".into()];
            }
            inv.observe(&obs, t(0));
        }
        assert_eq!(inv.devices().count(), 2);
    }
}
