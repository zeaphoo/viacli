//! toposight 引擎（逐字移植自用户自己的 `.links/toposight` 项目，MIT/Apache-2.0）。
//!
//! 这一层是**扫描与清点引擎**：发现源（neigh / L3 sweep / mDNS / SSDP）、
//! 端点→设备的归并、反向 DNS 与端口指纹探测、OUI 厂商库。
//! 上层（`crate::collect`）负责把它接到 keendeck 的采集器协议上。
//!
//! 与上游的差异只有两处，都是按需求裁剪：
//! * **不含 raw ARP（`pnet`）与外部 `nmap` 源**——这两个上游模块没有移植；
//! * **不含独立服务的 API/UI/登录/调度与 sqlx 持久化**——这些由 keendeck 宿主承担。
//!
//! 其余逻辑（含单元测试）逐字保留，以保证行为一致。
//!
//! 这是**逐字移植的第三方（用户自有）代码**：为保证与上游一致，不对其做风格级
//! 改写，因此整体关闭 dead_code/unused_imports/clippy 的告警。
#![allow(dead_code, unused_imports)]
#![allow(clippy::all)]

pub mod config;
pub mod connect;
pub mod discovery;
pub mod error;
pub mod fingerprint;
pub mod identify;
pub mod ids;
pub mod inventory;
pub mod model;
pub mod netinfo;
pub mod oui;
pub mod ports;
pub mod probe;
pub mod segments;
pub mod sources;

pub use config::{Config, DEFAULT_INTENSITY, MAX_INTENSITY, MIN_INTENSITY, default_sources};
pub use discovery::{Discovery, ScanOutcome, SourceKind};
pub use error::{Error, Result};
pub use identify::{Identification, Identifier, PortProbe};
pub use inventory::{Device, Endpoint, Event, EventKind, Fingerprint, Inventory, MergedBy};
pub use model::{
    DeviceKind, DiscoverySource, Label, MacAddr, Observation, ObservationInfo, PortInfo,
};
pub use oui::OuiDb;
pub use ports::PortSpec;
pub use sources::load_oui;
