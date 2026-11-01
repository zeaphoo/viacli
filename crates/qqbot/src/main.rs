//! `viacli-qqbot` 独立 bin：QQ Bot API 命令行工具。

use clap::Parser;

use qqbot::cli::{run, Cli};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();
    run(Cli::parse()).await
}
