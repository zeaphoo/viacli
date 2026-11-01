//! 采集器共享基础库（遵循 `docs/07-collector-protocol.md`）。
//!
//! 把每个采集器都要重复写的东西收在一处：
//!
//! * [`protocol`]：`meta` / `series` / `sample` / `record` / `state` / `log` / `error`
//!   七种消息的发送端（[`Sink`]），以及序列命名、保留声明、错误类别与退出码。
//! * [`http`]：带超时、UA、响应体积上限、重试退避的阻塞 HTTP 客户端。
//!
//! 两条硬约束（协议 §3.1、§3.3）由 [`Sink`] 保证：**stdout 只放协议消息**、
//! **每条消息立即 flush**。采集器作者不必再各自踩这两个坑。

pub mod http;
pub mod protocol;

pub use http::{HttpClient, HttpConfig, HttpError};
pub use protocol::{ErrorKind, Kind, Retention, Sink, Unit, name_is_legal};
