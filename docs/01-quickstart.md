# viacli 快速入门

## 简介

viacli 是一个多功能命令行工具集，集成天气查询、金融数据、微信和 QQ Bot 等功能。

## 安装

### 从源码编译

```bash
# 克隆仓库
git clone <repository-url>
cd viacli

# 编译 Release 版本
cargo build --release

# 编译后的二进制在 target/release/viacli
```

### 多架构编译（Docker）

```bash
# 编译 AMD64 和 ARM64
./build.sh

# 输出目录
dist/amd64/viacli
dist/arm64/viacli
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

| 命令 | 功能描述 |
|------|----------|
| `weather` | 查询城市天气（无需 API Token） |
| `finance` | 查询股票、加密货币等金融数据 |
| `weixin` | 微信 iLink API 工具 |
| `qqbot` | QQ Bot API 工具 |

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
