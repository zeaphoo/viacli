//! `viacli-search` 独立 bin：无头 Chrome 网页搜索（Bing / Brave / DuckDuckGo）。

use clap::Parser;

use web_search::{run, WebSearchArgs};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = WebSearchArgs::parse();
    run(&args).await
}
