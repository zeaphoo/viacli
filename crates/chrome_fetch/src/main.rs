//! `viacli-fetch` 独立 bin：用无头 Chrome 抓取网页内容。

use clap::Parser;

use chrome_fetch::{run, ChromeFetchArgs};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = ChromeFetchArgs::parse();
    run(&args).await
}
