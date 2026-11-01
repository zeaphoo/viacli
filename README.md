# viacli

多功能命令行工具集：天气查询、金融数据、微信 iLink、QQ Bot、网页抓取与网页搜索，
以及一组「采集器（collector）」——灾害、预测市场、卫星、RSS、本机系统信息。

每个能力都是**独立的 bin**（独立的 cargo package），可以单独编译、单独安装、单独分发。

## 独立 bin

| bin | 功能 | 配置 | crate |
|-----|------|------|-------|
| `viacli-weather` | 查询城市天气（无需 API Token） | 否 | `crates/weather` |
| `viacli-finance` | 查询股票、加密货币行情 | 否 | `crates/finance` |
| `viacli-weixin` | 微信 iLink API（登录、收发消息、媒体） | 是 | `crates/weixin` |
| `viacli-qqbot` | QQ Bot API（网关、消息、媒体） | 是 | `crates/qqbot` |
| `viacli-fetch` | 用无头 Chrome 抓取网页内容 | 否 | `crates/chrome_fetch` |
| `viacli-search` | 网页搜索（Bing / Brave / DuckDuckGo） | 否 | `crates/web_search` |
| `viacli-hazards` | 灾害采集器：USGS 地震 / NWS 预警 / NOAA 空间天气 | 否 | `crates/hazards` |
| `viacli-markets` | 预测市场采集器：Polymarket / Kalshi | 否 | `crates/markets` |
| `viacli-space` | 太空采集器：CelesTrak TLE + SGP4 / 地面站 | 否 | `crates/space` |
| `viacli-rss` | RSS / Atom 订阅采集器 | 否 | `crates/rss` |
| `viacli-sys` | 本机系统信息采集器（CPU/内存/磁盘/网络/温度/进程） | 否 | `crates/system` |
| `viacli-networkdevice` | 局域网设备发现采集器（网段扫描 / 端口 / 厂商 / 增量清点库） | 否 | `crates/networkdevice` |
| `viacli` | **兼容入口**：等价于过去的多合一命令，只保留 6 个子命令，不再扩充 | — | `crates/viacli` |

> `viacli` 仅用于向后兼容（`viacli weather ...` 等），新功能一律以独立 bin 交付。
> 采集器类 bin 遵循 [keendeck 采集器协议](docs/07-collector-protocol.md)，
> 既可直接运行，也可作为第三方采集器挂进 keendeck 宿主。

## 安装

### 单个 bin（推荐）

```bash
git clone https://github.com/zeaphoo/viacli.git
cd viacli

# 只装需要的
cargo install --path crates/weather      # 产出 viacli-weather
cargo install --path crates/hazards      # 产出 viacli-hazards

# 全部
./scripts/install.sh
```

### 多架构二进制（Docker）

需要 `docker buildx`：

```bash
./build.sh
# dist/amd64/viacli*   dist/amd64 -> dist/viacli-amd64.tar.gz
# dist/arm64/viacli*   dist/arm64 -> dist/viacli-arm64.tar.gz
```

Docker 构建使用 `rust:1.85-alpine` + musl 静态链接，产物为静态可执行文件。

## 基本用法

```bash
# 天气 / 金融
viacli-weather Beijing --detailed
viacli-finance BTC-USD --outformat json

# 微信 / QQ Bot
viacli-weixin login
viacli-weixin monitor
viacli-qqbot --help

# 网页抓取 / 搜索
viacli-fetch https://example.com
viacli-search "rust async"

# 兼容入口（等价）
viacli weather Beijing --detailed
```

交互类命令（weather / finance / weixin / qqbot / fetch / search）支持 `--outformat text|json`（默认 `text`）。

采集器类 bin 使用的是采集器协议（NDJSON），见 [采集器文档](docs/07-collector-protocol.md)：

```bash
viacli-hazards --describe | jq .          # 自描述（能力/配置 schema/记录类型）
echo '{"protocol":1,"runId":"t","now":0,"mode":"oneshot","config":{}}' | viacli-hazards
```

## 项目结构

```
crates/
├── viacli/        # 兼容入口 bin `viacli`（冻结的 6 个子命令）
├── weather/       # lib + bin viacli-weather
├── finance/       # lib + bin viacli-finance
├── weixin/        # lib + bin viacli-weixin
├── qqbot/         # lib + bin viacli-qqbot
├── chrome_fetch/  # lib + bin viacli-fetch
├── web_search/    # lib + bin viacli-search
├── base-http/     # 采集器共享库：协议 Sink + 阻塞 HTTP
├── hazards/       # bin viacli-hazards
├── markets/       # bin viacli-markets
├── space/         # bin viacli-space
├── rss/           # bin viacli-rss
├── system/        # bin viacli-sys
└── networkdevice/ # bin viacli-networkdevice
docs/              # 使用文档
scripts/           # install.sh 等
build.sh           # Docker 多架构构建脚本
Dockerfile         # musl 静态构建
```

每个 bin 所在 crate 都是独立 package，依赖只声明自己用到的部分；
整个仓库共用一个 cargo workspace（一份 `Cargo.lock`、一份 release profile、共享编译产物）。

## 文档

- [快速入门](docs/01-quickstart.md)
- [Weather 命令](docs/02-weather.md)
- [Finance 命令](docs/03-finance.md)
- [QQBot 命令](docs/04-qqbot.md)
- [Weixin 命令](docs/05-weixin.md)
- [采集器（hazards / markets / space / rss / sys / networkdevice）](docs/07-collector-protocol.md)
- [局域网扫描（viacli-networkdevice）](docs/08-networkdevice.md)

## 许可证

MIT
