# Weixin 命令 - 微信 iLink API 工具

微信 iLink API 的命令行管理工具。

## 全局选项

```bash
    --outformat <FORMAT>   输出格式: text 或 json
    --base-url <URL>       API 基础 URL
-a, --account <ACCOUNT>    账号 ID（部分命令需要）
```

## 命令列表

| 命令 | 需要 `-a` | 说明 |
|------|----------|------|
| `login` | ❌ | 登录微信（扫码） |
| `list` | ❌ | 列出已登录账号 |
| `logout` | ❌ | 登出账号（参数直接指定） |
| `get-updates` | ✅ | 获取消息更新 |
| `send` | ✅ | 发送文本消息 |
| `typing` | ✅ | 发送输入状态 |
| `get-upload-url` | ✅ | 获取上传 URL |
| `monitor` | ✅ | 监控模式 |
| `send-image` | ✅ | 发送图片消息 |
| `send-video` | ✅ | 发送视频消息 |
| `send-file` | ✅ | 发送文件消息 |
| `get-config` | ✅ | 获取配置 |

## 子命令

### login - 登录微信（扫码）

```bash
viacli weixin login [选项]
```

选项：
```bash
-t, --timeout <MS>       登录超时时间（毫秒，默认: 480000）
    --bot-type <TYPE>    机器人类型（默认: 3）
```

示例：
```bash
# 默认登录
viacli weixin login

# 指定超时时间
viacli weixin login --timeout 60000
```

### list - 列出已登录账号

```bash
viacli weixin list
```

### logout - 登出账号

```bash
viacli weixin logout <account_id>
```

### get-updates - 获取消息更新

**需要 `-a` 参数**

```bash
viacli weixin get-updates [选项] -a <account_id>
```

选项：
```bash
-a, --account <ID>       账号 ID（必需）
-b, --buf <CACHE>        同步缓存
-t, --timeout <MS>       长轮询超时（毫秒，默认: 35000）
```

示例：
```bash
viacli weixin get-updates -a <account_id>
viacli weixin get-updates -a <account_id> --timeout 60000
```

### send - 发送文本消息

**需要 `-a` 参数**

```bash
viacli weixin send [选项] -a <account_id> <目标用户ID> <消息内容>
```

选项：
```bash
-a, --account <ID>       账号 ID（必需）
-c, --context <TOKEN>    上下文 Token（用于回复）
```

示例：
```bash
# 发送消息
viacli weixin send -a <account_id> "user_id" "你好"

# 引用回复
viacli weixin send -a <account_id> "user_id" "回复内容" --context "context_token"
```

### typing - 发送输入状态

**需要 `-a` 参数**

```bash
viacli weixin typing -a <account_id> <目标用户ID> <status>
```

状态值：
- `typing` - 正在输入
- `cancel` - 取消输入

示例：
```bash
viacli weixin typing -a <account_id> "user_id" typing
viacli weixin typing -a <account_id> "user_id" cancel
```

### get-upload-url - 获取上传 URL

**需要 `-a` 参数**

```bash
viacli weixin get-upload-url -a <account_id> <文件路径> <媒体类型> <目标用户ID>
```

媒体类型：
- `image` - 图片
- `video` - 视频
- `file` - 文件
- `voice` - 语音

示例：
```bash
viacli weixin get-upload-url -a <account_id> "/path/to/image.png" image "user_id"
```

### monitor - 监控模式（持续获取消息）

**需要 `-a` 参数**

```bash
viacli weixin monitor [选项] -a <account_id>
```

选项：
```bash
-a, --account <ID>        账号 ID（必需）
-i, --interval <SECONDS>  轮询间隔（秒，默认: 1）
```

示例：
```bash
# 启动监控
viacli weixin monitor -a <account_id>

# 指定轮询间隔
viacli weixin monitor -a <account_id> --interval 5
```

### send-image - 发送图片消息

**需要 `-a` 参数**

```bash
viacli weixin send-image [选项] -a <account_id> <目标用户ID> <图片文件路径>
```

选项：
```bash
-a, --account <ID>       账号 ID（必需）
-t, --text <TEXT>        可选的文本说明
-c, --context <TOKEN>    上下文 Token
```

示例：
```bash
viacli weixin send-image -a <account_id> "user_id" "/path/to/image.png"
viacli weixin send-image -a <account_id> "user_id" "/path/to/image.png" --text "图片说明"
```

### send-video - 发送视频消息

**需要 `-a` 参数**

```bash
viacli weixin send-video [选项] -a <account_id> <目标用户ID> <视频文件路径>
```

选项：
```bash
-a, --account <ID>       账号 ID（必需）
-t, --text <TEXT>        可选的文本说明
-c, --context <TOKEN>    上下文 Token
```

示例：
```bash
viacli weixin send-video -a <account_id> "user_id" "/path/to/video.mp4"
```

### send-file - 发送文件消息

**需要 `-a` 参数**

```bash
viacli weixin send-file [选项] -a <account_id> <目标用户ID> <文件路径>
```

选项：
```bash
-a, --account <ID>       账号 ID（必需）
-t, --text <TEXT>        可选的文本说明
-c, --context <TOKEN>    上下文 Token
```

示例：
```bash
viacli weixin send-file -a <account_id> "user_id" "/path/to/document.pdf"
```

### get-config - 获取配置

**需要 `-a` 参数**

```bash
viacli weixin get-config [选项] -a <account_id> <用户ID>
```

选项：
```bash
-a, --account <ID>       账号 ID（必需）
-c, --context <TOKEN>    上下文 Token
```

## 完整示例

```bash
# 1. 登录微信（扫码）
viacli weixin login

# 2. 查看已登录账号
viacli weixin list

# 3. 发送测试消息（需要 -a 指定账号）
viacli weixin send -a <account_id> "user_id" "Hello Weixin!"

# 4. 发送图片（需要 -a 指定账号）
viacli weixin send-image -a <account_id> "user_id" "/path/to/image.png"

# 5. 启动监控接收消息（需要 -a 指定账号）
viacli weixin monitor -a <account_id>

# 6. 登出
viacli weixin logout <account_id>
```

## 注意事项

- 需要先登录才能发送消息
- 二维码有效期为一定时间，过期需重新获取
- 监控模式需要保持运行
- 发送媒体文件前需要先获取上传 URL
