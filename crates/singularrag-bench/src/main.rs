#![forbid(unsafe_code)]

use std::path::PathBuf;

use clap::{Parser, Subcommand};

mod config;
mod repo;
mod score;
mod session;
mod stream;
mod summary;

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
    /// Rewrite summary.md from the records in a run directory
    Score { run_dir: PathBuf },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Parse { stream } => {
            let text = std::fs::read_to_string(&stream)?;
            let parsed = stream::parse_stream(&text);
            println!("{}", serde_json::to_string_pretty(&parsed)?);
        }
        Cmd::Score { run_dir } => {
            let (meta, baseline, conditions, questions) = summary::load_run(&run_dir)?;
            let md = summary::render(&meta, &baseline, &conditions, &questions);
            std::fs::write(run_dir.join("summary.md"), &md)?;
            print!("{md}");
        }
    }
    Ok(())
}
