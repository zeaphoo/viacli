# viacli 文档目录

## 文档列表

| 文件名 | 说明 |
|--------|------|
| [01-quickstart.md](01-quickstart.md) | 快速入门指南 |
| [02-weather.md](02-weather.md) | Weather 命令 - 天气查询 |
| [03-finance.md](03-finance.md) | Finance 命令 - 金融数据 |
| [04-qqbot.md](04-qqbot.md) | QQBot 命令 - QQ Bot API |
| [05-weixin.md](05-weixin.md) | Weixin 命令 - 微信 iLink API |
| [07-collector-protocol.md](07-collector-protocol.md) | 采集器协议 + 内置采集器（hazards/markets/space/rss/sys） |

## 快速导航

### 新用户

1. 先阅读 [快速入门](01-quickstart.md)
2. 根据需要查看各命令详细文档

## 命令分类

### 工具类
- `viacli-weather` / `viacli weather` - 天气查询（无需配置）
- `viacli-finance` / `viacli finance` - 金融数据（无需配置）
- `viacli-fetch` / `viacli chrome_fetch` - 无头 Chrome 抓取
- `viacli-search` / `viacli web_search` - 网页搜索

### 消息平台
- `viacli-qqbot` / `viacli qqbot` - QQ Bot API（需要配置 AppID/Secret）
- `viacli-weixin` / `viacli weixin` - 微信 iLink API（需要配置 AppID/Secret）

### 采集器（独立 bin，遵循采集器协议）
- `viacli-hazards` - USGS 地震 / NWS 预警 / NOAA 空间天气
- `viacli-markets` - Polymarket / Kalshi 预测市场
- `viacli-space` - CelesTrak 卫星 + SGP4 / 地面站
- `viacli-rss` - RSS / Atom 订阅
- `viacli-sys` - 本机系统信息
