#![forbid(unsafe_code)]

use std::path::PathBuf;

use clap::{Parser, Subcommand};

mod config;
mod score;
mod stream;

#[derive(Parser, Debug)]
#[command(
    name = "singularrag-bench",
    version,
    about = "Tier-two eval: headless Claude Code sessions scored against the gold sets"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Print the parsed record of one stream-json transcript
    Parse { stream: PathBuf },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Parse { stream } => {
            let text = std::fs::read_to_string(&stream)?;
            let parsed = stream::parse_stream(&text);
            println!("{}", serde_json::to_string_pretty(&parsed)?);
        }
    }
    Ok(())
}
