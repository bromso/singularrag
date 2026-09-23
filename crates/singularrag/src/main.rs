#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, Subcommand};
use singularrag_core::engine::{ChangedRequest, Engine, FindRequest, MapRequest, TraceRequest};
use singularrag_core::map::DEFAULT_BUDGET;

mod actor;
mod hook;
mod init;
mod mcp;
mod serve;

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
        /// Entity names that matter; repeatable
        #[arg(long = "entity", value_name = "NAME")]
        entity: Vec<String>,
        /// Themes as short phrases; repeatable
        #[arg(long = "theme", value_name = "TEXT")]
        theme: Vec<String>,
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
    /// The shortest chain of references between two symbols (what the trace_path tool returns)
    Path {
        /// `path::symbol`
        from: String,
        /// `path::symbol`
        to: String,
    },
    /// The symbols a diff touches and who references them (what the changed tool returns)
    Changed {
        /// A git ref; default is the working tree against HEAD
        #[arg(long)]
        base: Option<String>,
    },
    /// Serve the repo_map, find_symbol, trace_path, changed and annotate tools to an agent over stdio (MCP)
    Mcp {
        /// Inline refresh budget in milliseconds (spec §8). Tests lower it.
        #[arg(long, default_value_t = 2000, hide = true)]
        refresh_budget_ms: u64,
    },
    /// Open the map UI on localhost
    Serve {
        #[arg(long, default_value_t = 0)]
        port: u16,
        #[arg(long)]
        no_open: bool,
    },
    /// Claude Code PreToolUse hook (installed by `init`): reads the hook JSON on stdin
    Hook {
        #[arg(value_enum)]
        event: hook::HookEvent,
    },
    /// Check Ollama, the configured models and the knowledge queue
    Doctor,
    /// Install the query-first hook and the MCP entry for this repo
    Init {
        /// Write .claude/settings.json (shared) instead of .claude/settings.local.json
        #[arg(long)]
        project: bool,
        #[arg(long, value_enum, default_value_t = init::Host::All)]
        host: init::Host,
    },
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    if let Cmd::Hook { event } = cli.cmd {
        let mut input = String::new();
        let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut input);
        println!("{}", hook::run(event, cli.repo.clone(), &input));
        return Ok(());
    }
    let root = cli.repo.unwrap_or(std::env::current_dir()?);
    if let Cmd::Init { project, host } = cli.cmd {
        let bin = std::env::current_exe()?;
        print!("{}", init::run(&root, project, host, &bin)?);
        return Ok(());
    }
    // `mcp` returns before the `Engine::open` below, and must: the Engine is `!Sync` and
    // belongs to the actor thread, which opens it lazily at the first job with a session
    // key derived from the client name that MCP `initialize` delivers (spec §2). Opening
    // one here would give the actor a second connection under a `cli-<pid>` key, and
    // would turn a bad `--repo` into a startup failure instead of the `is_error` tool
    // result the spec asks for.
    if let Cmd::Mcp { refresh_budget_ms } = cli.cmd {
        return mcp::run(root, Duration::from_millis(refresh_budget_ms));
    }
    if let Cmd::Serve { port, no_open } = cli.cmd {
        return serve::run(root, port, !no_open);
    }
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
            entity,
            theme,
        } => {
            let r = engine.repo_map(&MapRequest {
                query: text,
                focus_files: focus,
                budget_tokens: budget,
                entities: entity,
                themes: theme,
            })?;
            print!("{}", r.text);
        }
        Cmd::Find { name, kind, limit } => {
            let r = engine.find_symbol(&FindRequest { name, kind, limit })?;
            print!("{}", r.text);
        }
        Cmd::Path { from, to } => {
            let split = |s: &str| -> anyhow::Result<(String, String)> {
                s.split_once("::")
                    .map(|(p, n)| (p.to_string(), n.to_string()))
                    .ok_or_else(|| anyhow::anyhow!("expected path::symbol, got {s}"))
            };
            let (from_path, from_symbol) = split(&from)?;
            let (to_path, to_symbol) = split(&to)?;
            let r = engine.trace_path(&TraceRequest {
                from_path,
                from_symbol,
                to_path,
                to_symbol,
            })?;
            print!("{}", r.text);
        }
        Cmd::Changed { base } => {
            let r = engine.changed(&ChangedRequest { base })?;
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
        Cmd::Doctor => print!("{}", doctor(&engine)?),
        Cmd::Mcp { .. } => unreachable!("handled above before Engine::open"),
        Cmd::Serve { .. } => unreachable!("handled above before Engine::open"),
        Cmd::Hook { .. } => unreachable!("handled above before Engine::open"),
        Cmd::Init { .. } => unreachable!("handled above before Engine::open"),
    }
    Ok(())
}

/// `singularrag doctor`: five lines on Ollama, the two models, the embedding dimension
/// and the knowledge queue. An unreachable Ollama is a line of output, not a failure.
fn doctor(engine: &Engine) -> anyhow::Result<String> {
    use singularrag_core::knowledge;
    let cfg = &engine.workspace().models;
    let tags = match engine.models() {
        Some(m) => m.tags().map_err(|e| e.to_string()),
        None => Err("no Ollama URL configured".to_string()),
    };
    let url = &cfg.ollama;
    let mut out = match &tags {
        Ok(_) => format!("ollama: ok ({url})\n"),
        Err(e) => format!("ollama: unreachable ({e})\n"),
    };
    for name in [&cfg.extract, &cfg.embed] {
        let pulled = tags.as_ref().is_ok_and(|t| is_pulled(t, name));
        let state = if pulled { "pulled" } else { "missing" };
        out.push_str(&format!("{name}: {state}\n"));
    }
    let store = engine.store();
    let dim =
        singularrag_core::store::vec::dim(store)?.map_or("unknown".to_string(), |d| d.to_string());
    out.push_str(&format!("embedding dimension: {dim}\n"));
    let failed = knowledge::failed_sections(store)?;
    let last_error = store
        .get_meta("models_error")?
        .filter(|e| !e.is_empty())
        .or_else(|| {
            failed
                .last()
                .map(|(_, _, e)| e.clone())
                .filter(|e| !e.is_empty())
        })
        .unwrap_or_else(|| "none".to_string());
    out.push_str(&format!(
        "pending: {} sections · failed: {} · last error: {last_error}\n",
        knowledge::pending(store)?,
        failed.len()
    ));
    Ok(out)
}

/// Ollama lists `nomic-embed-text` as `nomic-embed-text:latest`.
fn is_pulled(tags: &[String], name: &str) -> bool {
    tags.iter()
        .any(|t| t == name || (!name.contains(':') && t.strip_prefix(name) == Some(":latest")))
}
