#![forbid(unsafe_code)]

use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "singularrag",
    version,
    about = "Repo map with retrieval provenance for coding agents"
)]
struct Cli {}

fn main() -> anyhow::Result<()> {
    let _cli = Cli::parse();
    Ok(())
}
