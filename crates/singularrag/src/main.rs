#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, Subcommand};
use singularrag_core::engine::{Engine, FindRequest, MapRequest};
use singularrag_core::map::DEFAULT_BUDGET;

#[allow(dead_code)]
mod mcp;

#[derive(Parser, Debug)]
#[command(
    name = "singularrag",
    version,
    about = "Repo map with retrieval provenance for coding agents"
)]
struct Cli {
    /// Repo root. Defaults to the current directory.
    #[arg(long, global = true, value_name = "PATH")]
    repo: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Build or refresh .singularrag/index.db
    Index,
    /// Print a token-budgeted repo map (what the repo_map tool returns)
    Query {
        /// Identifiers or a natural-language question
        text: Option<String>,
        #[arg(long, default_value_t = DEFAULT_BUDGET)]
        budget: usize,
        /// Repo-relative files to seed the ranking; repeatable
        #[arg(long = "focus", value_name = "PATH")]
        focus: Vec<String>,
    },
    /// Look up symbols by name (what the find_symbol tool returns)
    Find {
        name: String,
        #[arg(long)]
        kind: Option<String>,
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// Tier-one eval: recall of gold symbols inside the budgeted map
    Eval {
        #[arg(long, default_value = "eval/questions.toml")]
        questions: PathBuf,
        #[arg(long, default_value_t = DEFAULT_BUDGET)]
        budget: usize,
        #[arg(long)]
        json: bool,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let root = cli.repo.unwrap_or(std::env::current_dir()?);
    let mut engine = Engine::open(&root, &format!("cli-{}", std::process::id()))?;
    match cli.cmd {
        Cmd::Index => {
            let s = engine.refresh(Duration::from_secs(600))?;
            let held = if s.lock_timeout {
                " · lock held by another process"
            } else {
                ""
            };
            println!(
                "scanned {} · indexed {} · unchanged {} · skipped {} · removed {} · remaining {}{held}",
                s.scanned, s.indexed, s.unchanged, s.skipped, s.removed, s.remaining
            );
        }
        Cmd::Query {
            text,
            budget,
            focus,
        } => {
            let r = engine.repo_map(&MapRequest {
                query: text,
                focus_files: focus,
                budget_tokens: budget,
            })?;
            print!("{}", r.text);
        }
        Cmd::Find { name, kind, limit } => {
            let r = engine.find_symbol(&FindRequest { name, kind, limit })?;
            print!("{}", r.text);
        }
        Cmd::Eval {
            questions,
            budget,
            json,
        } => {
            let qs = singularrag_core::eval::load_questions(&questions)?;
            let results = singularrag_core::eval::run(&mut engine, &qs, budget)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&results)?);
            } else {
                print!("{}", singularrag_core::eval::render_report(&results));
            }
        }
    }
    Ok(())
}
