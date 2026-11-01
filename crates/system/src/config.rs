//! 请求与配置（见 `docs/07-collector-protocol.md` §2.2、§11.5）。

use serde::Deserialize;

/// 宿主发来的请求。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub protocol: u32,
    /// 仅用于日志关联：宿主会把它和自己的记录对上，采集器不需要据此改行为。
    #[serde(default)]
    pub run_id: Option<String>,
    /// 宿主当前时间（Unix 毫秒）。采集器不要自己造时间（§2.5）。
    #[serde(default)]
    pub now: Option<i64>,
    /// `oneshot` | `stream`（§4）
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default)]
    pub interval_ms: Option<u64>,
    #[serde(default)]
    pub config: Option<Config>,
    /// 上次成功运行保存的状态（§5）。`system` 不用它，但必须能收下不报错。
    #[serde(default)]
    #[allow(dead_code)]
    pub state: Option<serde_json::Value>,
    #[serde(default)]
    pub limits: Option<Limits>,
}

fn default_mode() -> String {
    "oneshot".to_string()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub max_output_bytes: Option<u64>,
    #[serde(default)]
    pub max_lines: Option<u64>,
}

impl Request {
    /// 日志前缀，便于把 stderr 与某次运行对上。
    pub fn tag(&self) -> String {
        match &self.run_id {
            Some(id) => format!("run={id}"),
            None => "run=-".to_string(),
        }
    }
}

pub const ALL_CATEGORIES: &[&str] = &[
    "cpu",
    "memory",
    "disk",
    "network",
    "temperature",
    "gpu",
    "system",
    "processes",
];

/// 采集器配置（对应 `--describe` 里的 `configSchema`）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    pub interval_ms: u64,
    pub categories: Vec<String>,
    pub top_processes: usize,
    pub include_interfaces: Vec<String>,
    pub exclude_interfaces: Vec<String>,
    pub include_disks: Vec<String>,
    pub exclude_disks: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            // 2 秒一次：这是它选 stream 的原因之一（见 DESIGN.md §6.2.13.6）——
            // 间隔这么短，每轮 spawn 一个进程的固定开销（含 250ms 双采样窗口）就不可忽略了。
            interval_ms: 2000,
            categories: ALL_CATEGORIES.iter().map(|s| s.to_string()).collect(),
            top_processes: 5,
            include_interfaces: Vec::new(),
            // 回环 + 容器/虚拟机/隧道网卡默认排除。
            //
            // 这不是"顺手清一下"：实测一台普通开发机上 **31 个网卡里有 27 个是 Docker
            // 的 `br-*` 网桥**，每个贡献 8 条序列 → 335 条序列里 248 条来自网卡，
            // 而其中 216 条是容器网桥的噪声。默认保留它们会让库在几天内被撑满，
            // 且真正要看的物理网卡被淹没。用户要监控网桥时用 `includeInterfaces` 打开即可。
            //
            // 用子串匹配（见 `interface_allowed`），所以 `br-` 能盖住 Docker 的
            // `br-<12位ID>`；`docker0` 是老版本的默认网桥，一并留着。
            exclude_interfaces: vec![
                "lo".into(),
                "docker".into(),
                "veth".into(),
                "br-".into(),   // Docker 自定义网络 / libvirt 网桥
                "virbr".into(), // libvirt 默认网桥
                "tun".into(),
                "tap".into(),
                "wg".into(),    // WireGuard
                "zt".into(),    // ZeroTier
            ],
            include_disks: Vec::new(),
            exclude_disks: Vec::new(),
        }
    }
}

impl Config {
    /// 是否采集某类别。`categories` 为空视为"全采"。
    pub fn has(&self, category: &str) -> bool {
        self.categories.is_empty() || self.categories.iter().any(|c| c == category)
    }

    /// 过滤后的接口列表（先 include 后 exclude；include 为空表示全选）。
    pub fn interface_allowed(&self, name: &str) -> bool {
        let included = self.include_interfaces.is_empty()
            || self.include_interfaces.iter().any(|p| name.contains(p.as_str()));
        let excluded = self
            .exclude_interfaces
            .iter()
            .any(|p| !p.is_empty() && name.contains(p.as_str()));
        included && !excluded
    }

    pub fn disk_allowed(&self, name: &str, mount: &str) -> bool {
        let included = self.include_disks.is_empty()
            || self
                .include_disks
                .iter()
                .any(|p| name.contains(p.as_str()) || mount.contains(p.as_str()));
        let excluded = self.exclude_disks.iter().any(|p| {
            !p.is_empty() && (name.contains(p.as_str()) || mount.contains(p.as_str()))
        });
        included && !excluded
    }

    /// 校验配置；返回人类可读的错误（宿主会按退出码 2 处理，§3.4）。
    pub fn validate(&self) -> Result<(), String> {
        if self.interval_ms < 250 {
            return Err(format!(
                "intervalMs 不能小于 250（当前 {}）——低于最小更新间隔无法得到有效的 CPU 使用率",
                self.interval_ms
            ));
        }
        for c in &self.categories {
            if !ALL_CATEGORIES.contains(&c.as_str()) {
                return Err(format!(
                    "未知类别 {c:?}；可用值：{}",
                    ALL_CATEGORIES.join(", ")
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let c = Config::default();
        assert!(c.validate().is_ok());
        assert!(c.has("cpu"));
        assert!(c.interval_ms >= 250, "默认间隔必须满足最小更新间隔");
        assert_eq!(c.top_processes, 5);
    }

    #[test]
    fn camel_case_request_parses() {
        let raw = r#"{
            "protocol": 1, "runId": "r1", "now": 123, "mode": "stream",
            "intervalMs": 1000,
            "config": {"intervalMs": 1000, "categories": ["cpu"], "topProcesses": 3},
            "limits": {"timeoutMs": 5000, "maxOutputBytes": 100, "maxLines": 10}
        }"#;
        let req: Request = serde_json::from_str(raw).unwrap();
        assert_eq!(req.protocol, 1);
        assert_eq!(req.mode, "stream");
        assert_eq!(req.interval_ms, Some(1000));
        let cfg = req.config.unwrap();
        assert_eq!(cfg.categories, vec!["cpu"]);
        assert_eq!(cfg.top_processes, 3);
        assert_eq!(req.limits.unwrap().max_lines, Some(10));
    }

    #[test]
    fn invalid_config_is_rejected_with_readable_message() {
        let c = Config {
            interval_ms: 10,
            ..Default::default()
        };
        let err = c.validate().unwrap_err();
        assert!(err.contains("intervalMs"), "错误信息要说清是哪个字段: {err}");

        let c = Config {
            categories: vec!["nope".into()],
            ..Default::default()
        };
        assert!(c.validate().unwrap_err().contains("未知类别"));
    }

    #[test]
    fn interface_filtering() {
        let c = Config::default();
        assert!(!c.interface_allowed("lo"), "回环默认被排除");
        assert!(c.interface_allowed("eth0"));
        let c = Config {
            include_interfaces: vec!["eth".into()],
            ..Default::default()
        };
        assert!(c.interface_allowed("eth0"));
        assert!(!c.interface_allowed("wlan0"), "include 非空时只选匹配项");
    }
}
