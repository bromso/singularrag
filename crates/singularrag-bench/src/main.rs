#![forbid(unsafe_code)]

use std::path::PathBuf;

use clap::{Parser, Subcommand};

mod config;
mod repo;
mod run;
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
    /// Run the tier-two eval: one headless Claude Code session per condition, question and repeat
    Run {
        #[arg(long, default_value = "eval/tier2.toml")]
        config: PathBuf,
        #[arg(long, default_value = "run")]
        label: String,
        /// Comma-separated condition names (default: all in the config)
        #[arg(long, value_delimiter = ',')]
        conditions: Option<Vec<String>>,
        /// Comma-separated question ids (default: all)
        #[arg(long, value_delimiter = ',')]
        questions: Option<Vec<String>>,
        #[arg(long)]
        repeats: Option<u32>,
        /// Print every command line and exit without touching the checkout
        #[arg(long)]
        dry_run: bool,
        /// Continue a run directory, skipping sessions that already have a record
        #[arg(long, value_name = "RUN_DIR")]
        resume: Option<PathBuf>,
    },
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
        Cmd::Run {
            config,
            label,
            conditions,
            questions,
            repeats,
            dry_run,
            resume,
        } => {
            let opts = run::RunOpts {
                config,
                label,
                conditions,
                questions,
                repeats,
                dry_run,
                resume,
            };
            if let Some(dir) = run::run(&opts)? {
                eprintln!("run written to {}", dir.display());
            }
        }
    }
    Ok(())
}
