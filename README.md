# viacli

多功能命令行工具集：天气查询、金融数据、微信 iLink、QQ Bot、网页抓取与网页搜索。

## 特性

| 命令 | 功能描述 | 是否需要配置 |
|------|----------|--------------|
| `weather` | 查询城市天气（无需 API Token） | 否 |
| `finance` | 查询股票、加密货币等金融数据 | 否 |
| `weixin` | 微信 iLink API 工具（登录、收发消息、媒体） | 是 |
| `qqbot` | QQ Bot API 工具 | 是 |
| `chrome_fetch` | 使用 Chrome 无头浏览器抓取网页内容 | 否（需本机 Chrome/Chromium） |
| `web_search` | 网页搜索（Bing / Brave / DuckDuckGo） | 否（需本机 Chrome/Chromium） |

## 安装

### 从源码编译

```bash
git clone https://github.com/zeaphoo/viacli.git
cd viacli
cargo build --release
# 二进制输出：target/release/viacli
```

### 多架构编译（Docker）

需要 `docker buildx`：

```bash
./build.sh
# 输出：dist/amd64/viacli、dist/arm64/viacli
```

Docker 构建使用 `rust:1.85-alpine` + musl 静态链接，产物为静态可执行文件。

## 基本用法

```bash
viacli --help
viacli --version

# 天气
viacli weather Beijing
viacli weather Shanghai --detailed

# 金融数据
viacli finance AAPL
viacli finance BTC-USD --outformat json

# 微信
viacli weixin login
viacli weixin accounts
viacli weixin monitor

# QQ Bot
viacli qqbot --help

# 网页抓取 / 搜索
viacli chrome_fetch https://example.com
viacli web_search "rust async"
```

部分子命令支持 `--outformat text|json`（默认 `text`）。

## 项目结构

```
crates/
├── viacli/        # CLI 入口（clap 子命令聚合）
├── weather/       # 天气查询
├── finance/       # 金融数据
├── weixin/        # 微信 iLink 客户端
├── qqbot/         # QQ Bot 客户端
├── chrome_fetch/  # 无头浏览器抓取
└── web_search/    # 网页搜索
docs/              # 使用文档
build.sh           # Docker 多架构构建脚本
Dockerfile         # musl 静态构建
```

## 文档

- [快速入门](docs/01-quickstart.md)
- [Weather 命令](docs/02-weather.md)
- [Finance 命令](docs/03-finance.md)
- [QQBot 命令](docs/04-qqbot.md)
- [Weixin 命令](docs/05-weixin.md)

## 许可证

MIT
