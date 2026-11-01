# QQBot 命令 - QQ Bot API 工具

QQ Bot 的命令行管理工具，支持消息发送、Webhook 管理等功能。

## 全局选项

```bash
-a, --account <ACCOUNT>  指定账号 ID
    --outformat <FORMAT>   输出格式: text 或 json
```

## 子命令

### config - 配置账号

```bash
viacli qqbot config [选项]
```

选项：
```bash
    --app-id <APP_ID>      App ID
    --secret <SECRET>      Client Secret
    --token <TOKEN>        Token (格式: app_id:secret)
    --name <NAME>          账号名称
    --use-env              使用环境变量
```

示例：
```bash
# 使用 token 配置
viacli qqbot config --token "102146862:your-secret"

# 使用环境变量
viacli qqbot config --use-env

# 环境变量名：
# QQBOT_APP_ID
# QQBOT_CLIENT_SECRET
```

### list - 列出已配置账号

```bash
viacli qqbot list
```

### remove - 删除账号配置

```bash
viacli qqbot remove <account_id>
```

### send - 发送消息

```bash
viacli qqbot send [选项] <目标ID> <消息内容>
```

选项：
```bash
-t, --target-type <TYPE>  目标类型: c2c, group, channel (默认: c2c)
-r, --reply-to <MSG_ID>   回复的消息 ID
```

示例：
```bash
# 发送私聊消息
viacli qqbot send --account default "openid" "你好"

# 发送群消息
viacli qqbot send --account default --target-type group "group_openid" "大家好"

# 引用回复
viacli qqbot send --account default "openid" "回复内容" --reply-to "msg_id"
```

### send-markdown - 发送 Markdown 消息

```bash
viacli qqbot send-markdown [选项] <目标ID> <Markdown内容>
```

示例：
```bash
viacli qqbot send-markdown --account default "openid" "**粗体** 和 *斜体*"
```

### send-image - 发送图片

```bash
viacli qqbot send-image [选项] <目标ID> <图片路径或URL>
```

选项：
```bash
-t, --target-type <TYPE>  目标类型 (默认: c2c)
-c, --caption <TEXT>      图片说明
```

示例：
```bash
# 发送本地图片
viacli qqbot send-image --account default "openid" "/path/to/image.png"

# 发送网络图片
viacli qqbot send-image --account default "openid" "https://example.com/image.png"
```

### send-file - 发送文件

```bash
viacli qqbot send-file [选项] <目标ID> <文件路径>
```

示例：
```bash
viacli qqbot send-file --account default "openid" "/path/to/document.pdf"
```

### typing - 发送输入状态

```bash
viacli qqbot typing [选项] <目标ID>
```

选项：
```bash
-s, --seconds <SECONDS>  持续时间（默认: 60）
```

示例：
```bash
viacli qqbot typing --account default "openid"
viacli qqbot typing --account default --seconds 30 "openid"
```

### gateway - 获取 Gateway URL

```bash
viacli qqbot gateway --account <account_id>
```

### monitor - 监控模式（接收消息）

```bash
viacli qqbot monitor [选项] --account <account_id>
```

选项：
```bash
    --auto-reply <TEXT>  自动回复内容
```

示例：
```bash
# 启动监控
viacli qqbot monitor --account default

# 带自动回复
viacli qqbot monitor --account default --auto-reply "收到"
```

### ping - 测试连接

```bash
viacli qqbot ping --account <account_id>
```

## 消息类型说明

| 类型 | 说明 | 目标ID |
|------|------|--------|
| c2c | 私聊消息 | 用户 openid |
| group | 群聊消息 | 群组 openid |
| channel | 频道消息 | 频道 ID |

## 媒体标签

发送消息时支持使用媒体标签：

```bash
# 发送图片
viacli qqbot send --account default "openid" "查看图片 <image src='/path/to/image.png'>"

# 发送语音
viacli qqbot send --account default "openid" "<voice src='/path/to/audio.mp3'>"

# 发送视频
viacli qqbot send --account default "openid" "<video src='/path/to/video.mp4'>"

# 发送文件
viacli qqbot send --account default "openid" "<file src='/path/to/file.pdf'>"
```

## 引用回复

支持引用回复功能：

```bash
# 先获取消息 ID，然后回复
viacli qqbot send --account default "openid" "回复内容" --reply-to "原消息ID"
```

## 完整示例

```bash
# 1. 配置账号
viacli qqbot config --token "102146862:your-secret-here"

# 2. 查看配置
viacli qqbot list

# 3. 发送测试消息
viacli qqbot send --account default "user_openid" "Hello QQ Bot!"

# 4. 启动监控接收消息
viacli qqbot monitor --account default
```
