# viacli 快速入门

## 简介

viacli 是一个多功能命令行工具集，集成天气查询、金融数据、微信和 QQ Bot 等功能。

## 安装

每个能力都是一个独立的 bin，按需安装即可（推荐）：

```bash
git clone https://github.com/zeaphoo/viacli.git
cd viacli

cargo install --path crates/weather     # 产出 viacli-weather
cargo install --path crates/hazards     # 产出 viacli-hazards

./scripts/install.sh                    # 一次装全部 12 个 bin
```

也可以整仓库编译（产物在 `target/release/`）：

```bash
cargo build --release --bins
```

### 多架构编译（Docker）

```bash
# 编译 AMD64 和 ARM64
./build.sh

# 输出目录（每个架构一整套 bin + 一个 tar 包）
dist/amd64/viacli dist/amd64/viacli-*
dist/arm64/viacli dist/arm64/viacli-*
dist/viacli-amd64.tar.gz  dist/viacli-arm64.tar.gz
```

## 基本用法

```bash
# 查看帮助
viacli --help

# 查看版本
viacli --version

# 查看子命令帮助
viacli <command> --help
```

## 可用命令

每个命令对应一个独立 bin（`viacli` 是兼容入口，等价于 `viacli <命令>`）：

| bin | 兼容用法 | 功能描述 |
|-----|----------|----------|
| `viacli-weather` | `viacli weather` | 查询城市天气（无需 API Token） |
| `viacli-finance` | `viacli finance` | 查询股票、加密货币等金融数据 |
| `viacli-weixin` | `viacli weixin` | 微信 iLink API 工具 |
| `viacli-qqbot` | `viacli qqbot` | QQ Bot API 工具 |
| `viacli-fetch` | `viacli chrome_fetch` | 无头 Chrome 抓取网页 |
| `viacli-search` | `viacli web_search` | 网页搜索（Bing/Brave/DDG） |
| `viacli-hazards` | — | 采集器：地震 / 天气预警 / 空间天气 |
| `viacli-markets` | — | 采集器：预测市场（Polymarket/Kalshi） |
| `viacli-space` | — | 采集器：卫星星下点 / 地面站 |
| `viacli-rss` | — | 采集器：RSS / Atom 订阅 |
| `viacli-sys` | — | 采集器：本机系统信息 |

采集器类的 5 个 bin 使用采集器协议（`--describe` + stdin NDJSON），
见 [采集器文档](07-collector-protocol.md)。

## 全局选项

```bash
-h, --help     显示帮助信息
-V, --version  显示版本号
```

## 输出格式

部分子命令支持 `--outformat` 选项：

- `text` - 文本格式（默认）
- `json` - JSON 格式

示例：
```bash
viacli qqbot list --outformat json
```
