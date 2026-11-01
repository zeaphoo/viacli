//! `viacli-weather` 独立 bin：查询城市天气（无需 API Token）。

use clap::Parser;

use weather::{run_with_args, WeatherArgs};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = WeatherArgs::parse();
    run_with_args(args).await
}
