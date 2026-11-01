//! `viacli-weixin` 独立 bin：微信 iLink API 命令行工具。

use clap::Parser;

use weixin::cli::{run, Cli};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();
    run(Cli::parse()).await
}
