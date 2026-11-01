use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "viacli")]
#[command(about = "多功能命令行工具集")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(ValueEnum, Clone, Debug)]
pub enum OutputFormat {
    /// 文本格式输出
    Text,
    /// JSON 格式输出
    Json,
}

#[derive(Subcommand)]
enum Commands {
    /// 查询城市天气 - 无需 API Token
    Weather {
        /// 城市名称 (如: Beijing, Shanghai, Tokyo, London)
        city: String,

        /// 显示详细天气信息
        #[arg(short, long)]
        detailed: bool,

        /// 输出格式
        #[arg(long, value_enum, default_value = "text")]
        outformat: OutputFormat,
    },

    /// 查询金融数据 - 股票、加密货币
    Finance {
        /// 股票/加密货币代码 (如: AAPL, BTC-USD, TSLA)
        symbol: String,

        /// 输出格式
        #[arg(long, value_enum, default_value = "text")]
        outformat: OutputFormat,
    },

    /// 微信 iLink API 工具
    #[command(name = "weixin")]
    Weixin {
        #[command(subcommand)]
        command: WeixinCommands,

        /// 输出格式
        #[arg(long, value_enum, global = true, default_value = "text")]
        outformat: OutputFormat,

        /// API 基础 URL
        #[arg(long, global = true)]
        base_url: Option<String>,

        /// 账号 ID
        #[arg(short, long, global = true)]
        account: Option<String>,

        /// 状态数据目录（用于存储账号信息）
        #[arg(long, global = true)]
        state_dir: Option<String>,
    },

    /// QQ Bot API 工具
    #[command(name = "qqbot")]
    QQBot {
        #[command(subcommand)]
        command: QQBotCommands,

        /// 输出格式
        #[arg(long, value_enum, global = true, default_value = "text")]
        outformat: OutputFormat,

        /// 账号 ID
        #[arg(short, long, global = true)]
        account: Option<String>,
    },

    /// 使用 Chrome 无头浏览器抓取网页内容
    #[command(name = "chrome_fetch")]
    ChromeFetch {
        /// 要抓取的网页 URL
        url: String,

        /// 输出格式
        #[arg(long, value_enum, default_value = "text")]
        outformat: OutputFormat,

        /// 等待时间（毫秒），用于等待页面加载完成
        #[arg(long, default_value = "1000")]
        wait_time: u64,

        /// 是否启用 JavaScript（默认禁用）
        #[arg(long)]
        enable_js: bool,

        /// User-Agent 字符串
        #[arg(long)]
        user_agent: Option<String>,

        /// 视口宽度
        #[arg(long, default_value = "1920")]
        viewport_width: u32,

        /// 视口高度
        #[arg(long, default_value = "1080")]
        viewport_height: u32,
    },

    /// 网页搜索 - 支持 Bing, Brave, DuckDuckGo
    #[command(name = "web_search")]
    WebSearch {
        /// 搜索关键词
        query: String,

        /// 搜索引擎
        #[arg(short, long, value_enum, default_value = "bing")]
        engine: WebSearchEngineArg,

        /// 输出格式
        #[arg(long, value_enum, default_value = "text")]
        outformat: OutputFormat,

        /// 等待时间（毫秒），用于等待页面加载完成
        #[arg(long, default_value = "2000")]
        wait_time: u64,

        /// 是否启用 JavaScript（默认启用）
        #[arg(long)]
        enable_js: bool,

        /// User-Agent 字符串
        #[arg(long)]
        user_agent: Option<String>,

        /// 视口宽度
        #[arg(long, default_value = "1920")]
        viewport_width: u32,

        /// 视口高度
        #[arg(long, default_value = "1080")]
        viewport_height: u32,

        /// 结果数量（最多显示的搜索结果数）
        #[arg(long, default_value = "10")]
        limit: usize,
    },
}

#[derive(Subcommand)]
enum WeixinCommands {
    /// 登录微信（扫码）
    Login {
        /// 登录超时时间（毫秒）
        #[arg(short, long, default_value = "480000")]
        timeout: u64,

        /// 机器人类型
        #[arg(long, default_value = "3")]
        bot_type: String,
    },

    /// 列出已登录账号
    List,

    /// 登出账号
    Logout {
        /// 账号 ID
        account_id: String,
    },

    /// 获取消息更新
    GetUpdates {
        /// 同步缓存（如果不提供则使用账号缓存）
        #[arg(short, long)]
        buf: Option<String>,

        /// 长轮询超时（毫秒）
        #[arg(short, long, default_value = "35000")]
        timeout: u64,
    },

    /// 发送消息
    Send {
        /// 目标用户 ID
        to: String,

        /// 消息内容
        text: String,

        /// 上下文 Token（用于回复）
        #[arg(short, long)]
        context: Option<String>,
    },

    /// 获取配置
    GetConfig {
        /// 用户 ID
        user_id: String,

        /// 上下文 Token
        #[arg(short, long)]
        context: Option<String>,
    },

    /// 发送输入状态
    Typing {
        /// 目标用户 ID
        to: String,

        /// 状态：typing 或 cancel
        #[arg(value_enum)]
        status: weixin::cli::TypingStatusArg,
    },

    /// 获取上传 URL（用于发送媒体）
    GetUploadUrl {
        /// 文件路径
        file: String,

        /// 媒体类型
        #[arg(value_enum)]
        media_type: weixin::cli::MediaTypeArg,

        /// 目标用户 ID
        to: String,
    },

    /// 监控模式（持续获取消息）
    Monitor {
        /// 轮询间隔（秒）
        #[arg(short, long, default_value = "1")]
        interval: u64,
    },

    /// 发送图片消息
    SendImage {
        /// 目标用户 ID
        to: String,
        /// 图片文件路径
        file: String,
        /// 可选的文本说明
        #[arg(short, long)]
        text: Option<String>,
        /// 上下文 Token
        #[arg(short, long)]
        context: Option<String>,
    },

    /// 发送视频消息
    SendVideo {
        /// 目标用户 ID
        to: String,
        /// 视频文件路径
        file: String,
        /// 可选的文本说明
        #[arg(short, long)]
        text: Option<String>,
        /// 上下文 Token
        #[arg(short, long)]
        context: Option<String>,
    },

    /// 发送文件消息
    SendFile {
        /// 目标用户 ID
        to: String,
        /// 文件路径
        file: String,
        /// 可选的文本说明
        #[arg(short, long)]
        text: Option<String>,
        /// 上下文 Token
        #[arg(short, long)]
        context: Option<String>,
    },
}

#[derive(Subcommand)]
enum QQBotCommands {
    /// 配置账号
    Config {
        /// App ID
        #[arg(short, long)]
        app_id: Option<String>,
        /// Client Secret
        #[arg(short, long)]
        secret: Option<String>,
        /// Token (格式: app_id:client_secret)
        #[arg(short, long)]
        token: Option<String>,
        /// 账号名称
        #[arg(short, long)]
        name: Option<String>,
        /// 使用环境变量
        #[arg(long)]
        use_env: bool,
    },
    /// 列出已配置账号
    List,
    /// 删除账号配置
    Remove {
        /// 账号 ID
        account_id: String,
    },
    /// 发送消息
    Send {
        /// 目标 ID
        to: String,
        /// 消息内容
        text: String,
        /// 目标类型
        #[arg(short, long, value_enum, default_value = "c2c")]
        target_type: QQBotTargetTypeArg,
        /// 回复的消息 ID
        #[arg(short, long)]
        reply_to: Option<String>,
    },
    /// 发送 Markdown 消息
    SendMarkdown {
        /// 目标 ID
        to: String,
        /// Markdown 内容
        text: String,
        /// 目标类型
        #[arg(short, long, value_enum, default_value = "c2c")]
        target_type: QQBotTargetTypeArg,
    },
    /// 发送图片
    SendImage {
        /// 目标 ID
        to: String,
        /// 图片路径或 URL
        image: String,
        /// 目标类型
        #[arg(short, long, value_enum, default_value = "c2c")]
        target_type: QQBotTargetTypeArg,
        /// 图片说明
        #[arg(short, long)]
        caption: Option<String>,
    },
    /// 发送文件
    SendFile {
        /// 目标 ID
        to: String,
        /// 文件路径
        file: String,
        /// 目标类型
        #[arg(short, long, value_enum, default_value = "c2c")]
        target_type: QQBotTargetTypeArg,
    },
    /// 发送输入状态
    Typing {
        /// 目标 ID
        to: String,
        /// 输入状态持续时间（秒）
        #[arg(short, long, default_value = "60")]
        seconds: i32,
    },
    /// 获取 Gateway URL
    Gateway,
    /// 监控模式（接收消息）
    Monitor {
        /// 自动回复内容
        #[arg(short, long)]
        auto_reply: Option<String>,
    },
    /// 测试连接
    Ping,
}

#[derive(ValueEnum, Clone, Debug)]
enum QQBotTargetTypeArg {
    C2C,
    Group,
    Channel,
}

#[derive(ValueEnum, Clone, Debug)]
enum WebSearchEngineArg {
    Bing,
    Brave,
    DuckDuckGo,
}

#[tokio::main]
async fn main() -> Result<()> {
    // 初始化日志系统
    env_logger::init();
    
    let cli = Cli::parse();

    match cli.command {
        Commands::Weather { city, detailed, outformat } => {
            let json_output = matches!(outformat, OutputFormat::Json);
            weather::run(&city, detailed, json_output).await?;
        }
        Commands::Finance { symbol, outformat } => {
            let json_output = matches!(outformat, OutputFormat::Json);
            finance::run(&symbol, json_output).await?;
        }
        Commands::Weixin { command, outformat, base_url, account, state_dir } => {
            let cli = weixin::cli::Cli {
                command: match command {
                    WeixinCommands::Login { timeout, bot_type } => {
                        weixin::cli::Commands::Login { timeout, bot_type }
                    }
                    WeixinCommands::List => weixin::cli::Commands::List,
                    WeixinCommands::Logout { account_id } => {
                        weixin::cli::Commands::Logout { account_id }
                    }
                    WeixinCommands::GetUpdates { buf, timeout } => {
                        weixin::cli::Commands::GetUpdates { buf, timeout }
                    }
                    WeixinCommands::Send { to, text, context } => {
                        weixin::cli::Commands::Send { to, text, context }
                    }
                    WeixinCommands::GetConfig { user_id, context } => {
                        weixin::cli::Commands::GetConfig { user_id, context }
                    }
                    WeixinCommands::Typing { to, status } => {
                        weixin::cli::Commands::Typing { to, status }
                    }
                    WeixinCommands::GetUploadUrl { file, media_type, to } => {
                        weixin::cli::Commands::GetUploadUrl { file, media_type, to }
                    }
                    WeixinCommands::Monitor { interval } => {
                        weixin::cli::Commands::Monitor { interval }
                    }
                    WeixinCommands::SendImage { to, file, text, context } => {
                        weixin::cli::Commands::SendImage { to, file, text, context }
                    }
                    WeixinCommands::SendVideo { to, file, text, context } => {
                        weixin::cli::Commands::SendVideo { to, file, text, context }
                    }
                    WeixinCommands::SendFile { to, file, text, context } => {
                        weixin::cli::Commands::SendFile { to, file, text, context }
                    }
                },
                outformat: match outformat {
                    OutputFormat::Text => weixin::cli::OutputFormat::Text,
                    OutputFormat::Json => weixin::cli::OutputFormat::Json,
                },
                base_url,
                account,
                state_dir,
            };
            weixin::cli::run(cli).await?;
        }
        Commands::QQBot { command, outformat, account } => {
            let cli = qqbot::cli::Cli {
                command: match command {
                    QQBotCommands::Config { app_id, secret, token, name, use_env } => {
                        qqbot::cli::Commands::Config { app_id, secret, token, name, use_env }
                    }
                    QQBotCommands::List => qqbot::cli::Commands::List,
                    QQBotCommands::Remove { account_id } => {
                        qqbot::cli::Commands::Remove { account_id }
                    }
                    QQBotCommands::Send { to, text, target_type, reply_to } => {
                        qqbot::cli::Commands::Send {
                            to,
                            text,
                            target_type: match target_type {
                                QQBotTargetTypeArg::C2C => qqbot::cli::TargetTypeArg::C2C,
                                QQBotTargetTypeArg::Group => qqbot::cli::TargetTypeArg::Group,
                                QQBotTargetTypeArg::Channel => qqbot::cli::TargetTypeArg::Channel,
                            },
                            reply_to,
                        }
                    }
                    QQBotCommands::SendMarkdown { to, text, target_type } => {
                        qqbot::cli::Commands::SendMarkdown {
                            to,
                            text,
                            target_type: match target_type {
                                QQBotTargetTypeArg::C2C => qqbot::cli::TargetTypeArg::C2C,
                                QQBotTargetTypeArg::Group => qqbot::cli::TargetTypeArg::Group,
                                QQBotTargetTypeArg::Channel => qqbot::cli::TargetTypeArg::Channel,
                            },
                        }
                    }
                    QQBotCommands::SendImage { to, image, target_type, caption } => {
                        qqbot::cli::Commands::SendImage {
                            to,
                            image,
                            target_type: match target_type {
                                QQBotTargetTypeArg::C2C => qqbot::cli::TargetTypeArg::C2C,
                                QQBotTargetTypeArg::Group => qqbot::cli::TargetTypeArg::Group,
                                QQBotTargetTypeArg::Channel => qqbot::cli::TargetTypeArg::Channel,
                            },
                            caption,
                        }
                    }
                    QQBotCommands::SendFile { to, file, target_type } => {
                        qqbot::cli::Commands::SendFile {
                            to,
                            file,
                            target_type: match target_type {
                                QQBotTargetTypeArg::C2C => qqbot::cli::TargetTypeArg::C2C,
                                QQBotTargetTypeArg::Group => qqbot::cli::TargetTypeArg::Group,
                                QQBotTargetTypeArg::Channel => qqbot::cli::TargetTypeArg::Channel,
                            },
                        }
                    }
                    QQBotCommands::Typing { to, seconds } => {
                        qqbot::cli::Commands::Typing { to, seconds }
                    }
                    QQBotCommands::Gateway => qqbot::cli::Commands::Gateway,
                    QQBotCommands::Monitor { auto_reply } => qqbot::cli::Commands::Monitor { auto_reply },
                    QQBotCommands::Ping => qqbot::cli::Commands::Ping,
                },
                outformat: match outformat {
                    OutputFormat::Text => qqbot::cli::OutputFormat::Text,
                    OutputFormat::Json => qqbot::cli::OutputFormat::Json,
                },
                account,
            };
            qqbot::cli::run(cli).await?;
        }
        Commands::ChromeFetch { url, outformat, wait_time, enable_js, user_agent, viewport_width, viewport_height } => {
            let args = chrome_fetch::ChromeFetchArgs {
                url,
                outformat: match outformat {
                    OutputFormat::Text => chrome_fetch::OutputFormat::Text,
                    OutputFormat::Json => chrome_fetch::OutputFormat::Json,
                },
                wait_time,
                enable_js,
                user_agent,
                viewport_width,
                viewport_height,
            };
            chrome_fetch::run(&args).await?;
        }
        Commands::WebSearch { query, engine, outformat, wait_time, enable_js, user_agent, viewport_width, viewport_height, limit } => {
            let args = web_search::WebSearchArgs {
                query,
                engine: match engine {
                    WebSearchEngineArg::Bing => web_search::SearchEngine::Bing,
                    WebSearchEngineArg::Brave => web_search::SearchEngine::Brave,
                    WebSearchEngineArg::DuckDuckGo => web_search::SearchEngine::DuckDuckGo,
                },
                outformat: match outformat {
                    OutputFormat::Text => web_search::OutputFormat::Text,
                    OutputFormat::Json => web_search::OutputFormat::Json,
                },
                wait_time,
                enable_js,
                user_agent,
                viewport_width,
                viewport_height,
                limit,
            };
            web_search::run(&args).await?;
        }
    }

    Ok(())
}
