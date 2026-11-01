# 局域网设备发现（viacli-networkdevice）

`viacli-networkdevice` 扫描本机所在网段，识别在线端点、开放端口、设备类型与厂商，
并维护一份 SQLite 清点库做增量扫描、差分与设备归并（离线判定、变更事件）。

> 它是采集器：从 stdin 读一行 JSON 请求、向 stdout 写 NDJSON，协议见
> [07-collector-protocol.md](07-collector-protocol.md)。人用的话按下面的例子拼一行请求即可。

## 快速开始

```bash
# 自描述（能力 / 配置 schema / 记录类型）
viacli-networkdevice --describe | jq .

# 一次快速扫描（neigh + TCP 预扫，不主动探端口）
now=$(date +%s%3N)
echo "{\"protocol\":1,\"now\":$now,\"mode\":\"oneshot\",\"config\":{\"intensity\":2,\"sources\":[\"neigh\",\"sweep\"],\"portProbe\":false}}" \
  | viacli-networkdevice | jq -c 'select(.type=="record") | {kind, key}'

# 指定网卡 / 网段 / 清点库位置
echo '{"protocol":1,"now":0,"mode":"oneshot","config":{"sources":["sweep"],"extraSubnets":["192.168.1.0/24"]}}' \
  | viacli-networkdevice --ifaces eth0 --db ~/.cache/net.db
```

耗时参考：默认强度（`intensity=2`，neigh+sweep，不探端口）约 **4 秒**；
开 `probe`（端口探测 + TLS 指纹）或调高强度会更久，建议先 `--sources sweep` 摸清网段规模。

## 权限与平台

| 项 | 说明 |
|---|---|
| root / `CAP_NET_RAW` | **不需要**。默认源为 `neigh`（读 `ip neigh`）、`sweep`（TCP connect）、`mdns`、`ssdp`。raw-socket 的 `pnet` 与外部 `nmap` 都是显式 opt-in |
| 平台 | **Linux 优先**：网段/网卡信息靠解析 `ip -o -4 addr show`、`ip -4 route show default`。缺少 `ip` 时会退化为「没有自动扫描目标」，需用 `--extra-subnets` 手工指定 |
| 外部命令 | `ip`（建议装 `iproute2`）；`nmap` 仅在启用 Nmap 源时调用 |

## 常用参数

```
--config <path>           从文件读取请求（含 config 字段）或配置对象；stdin 优先
--db <path>               清点数据库位置（默认 $VIACLI_DATA_ROOT/networkdevice/<数据源名>.db）
--ifaces <列表>           扫描网卡，逗号分隔；留空 = 默认路由那张
--extra-subnets <列表>    L3 sweep 的网段（可跨路由器）
--sources <列表>          启用的发现源：neigh,mdns,ssdp,sweep,probe
--intensity <1-5>         扫描强度（统一缩放各源周期，默认 3）
--probe-ports <列表>      端口清单（数字或分组 common/web/db）
--no-port-probe           只被动识别（反向 DNS + 厂商）
--no-dns / --no-mdns / --no-ssdp / --no-neigh-sweep
--offline-after-secs <s>  多久没见到判离线
--oui-file <path>         自定义 IEEE OUI CSV
--probes-file <path>      自定义 nmap-service-probes
```

## 状态与数据目录

清点库默认位置：

```
$VIACLI_DATA_ROOT/networkdevice/<数据源名>.db      # 设置了 VIACLI_DATA_ROOT
$XDG_DATA_HOME/viacli/networkdevice/<数据源名>.db  # 否则
~/.local/share/viacli/networkdevice/<数据源名>.db  # 否则
```

`<数据源名>` 来自请求里的 `datasource`（缺省 `default`）。用 `--db` 可以完全覆盖。
库内保留历史、设备归并与变更事件；`historyRetainHours` / `trimHistHours` 控制保留窗口。

## 记录类型

| `kind` | 内容 |
|---|---|
| `networkdevice.endpoint` | 一个在线端点（IP/MAC/厂商/开放端口/指纹） |
| `networkdevice.device` | 归并后的设备（多个端点 → 一台设备） |
| `networkdevice.change` | 与上一轮相比的变更（上线/下线/端口变化） |
| `networkdevice.scan` | 本轮扫描元信息（源、耗时、命中数） |

## 作为第三方采集器使用

协议与 keendeck 采集器一致，可直接放进宿主的 `collectors/` 目录：

```bash
cp target/release/viacli-networkdevice ~/.local/share/keendeck/collectors/networkdevice
```
