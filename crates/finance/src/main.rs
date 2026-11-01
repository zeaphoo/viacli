//! `viacli-finance` 独立 bin：查询股票 / 加密货币行情。

use clap::Parser;

use finance::{run_with_args, FinanceArgs};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = FinanceArgs::parse();
    run_with_args(args).await
}
