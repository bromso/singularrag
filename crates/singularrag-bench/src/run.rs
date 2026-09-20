//! One run (spec §2): preflight, run directory, optional index, the session loop with resume,
//! the dirty check after every session, condition abort on a disconnected MCP server, summary.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use singularrag_core::eval::{load_questions, Question};

use crate::config::{self, Condition, RunConfig};
use crate::repo;
use crate::score;
use crate::session::{self, SessionSpec};
use crate::stream;
use crate::summary::{self, RunFile, RunHeader};

pub const SINGULARRAG_ENV: &str = "SINGULARRAG_BENCH_SINGULARRAG";

#[derive(Debug, Clone)]
pub struct RunOpts {
    pub config: PathBuf,
    pub label: String,
    pub conditions: Option<Vec<String>>,
    pub questions: Option<Vec<String>>,
    pub repeats: Option<u32>,
    pub dry_run: bool,
    pub resume: Option<PathBuf>,
}

fn singularrag_bin() -> String {
    std::env::var(SINGULARRAG_ENV)
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "singularrag".to_string())
}

/// Does this condition's MCP config launch the `singularrag` command?
fn uses_singularrag(c: &Condition) -> Result<bool> {
    let Some(path) = &c.mcp_config else {
        return Ok(false);
    };
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path)?)
        .with_context(|| format!("parsing {}", path.display()))?;
    let servers = v.get("mcpServers").and_then(|s| s.as_object());
    Ok(servers.into_iter().flatten().any(|(_, s)| {
        s.get("command")
            .and_then(|c| c.as_str())
            .map(|c| Path::new(c).file_name().is_some_and(|f| f == "singularrag"))
            .unwrap_or(false)
    }))
}

fn select_questions(all: Vec<Question>, ids: Option<&[String]>) -> Result<Vec<Question>> {
    match ids {
        None => Ok(all),
        Some(ids) => {
            for id in ids {
                if !all.iter().any(|q| &q.id == id) {
                    bail!("unknown question {id}");
                }
            }
            Ok(all.into_iter().filter(|q| ids.contains(&q.id)).collect())
        }
    }
}

fn spec_for(cfg: &RunConfig, q: &Question, mcp: Option<&Path>) -> SessionSpec {
    SessionSpec {
        prompt: session::prompt_for(&q.query, cfg.answer_max),
        schema: session::schema_for(cfg.answer_max),
        tools: cfg.tools.clone(),
        mcp_config: mcp.map(Path::to_path_buf),
        max_turns: cfg.max_turns,
        max_budget_usd: cfg.max_budget_usd,
    }
}

fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".into();
    }
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_./,=:".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

fn preflight(cfg: &RunConfig) -> Result<String> {
    if !cfg.repo.is_dir() {
        bail!("checkout not found: {}", cfg.repo.display());
    }
    let head = repo::head_commit(&cfg.repo)?;
    if head != cfg.commit {
        bail!("checkout HEAD is {head}, config pins {}", cfg.commit);
    }
    let dirty = repo::dirty_paths(&cfg.repo, repo::IGNORED_PREFIXES)?;
    if !dirty.is_empty() {
        bail!("checkout is dirty: {}", dirty.join(", "));
    }
    session::claude_version()
}

fn version_of_singularrag() -> Result<String> {
    let out = Command::new(singularrag_bin())
        .arg("--version")
        .output()
        .with_context(|| format!("running {} --version", singularrag_bin()))?;
    anyhow::ensure!(
        out.status.success(),
        "{} --version failed",
        singularrag_bin()
    );
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn write_run_file(run_dir: &Path, rf: &RunFile) -> Result<()> {
    std::fs::write(run_dir.join("run.toml"), toml::to_string(rf)?)?;
    Ok(())
}

fn dirty_after(cfg: &RunConfig, what: &str) -> Result<()> {
    let dirty = repo::dirty_paths(&cfg.repo, repo::IGNORED_PREFIXES)?;
    if !dirty.is_empty() {
        bail!(
            "checkout is dirty after {what}: {}; aborting the run",
            dirty.join(", ")
        );
    }
    Ok(())
}

pub fn run(opts: &RunOpts) -> Result<Option<PathBuf>> {
    let mut cfg = config::load(&opts.config)?;
    if let Some(r) = opts.repeats {
        cfg.repeats = r.max(1);
    }
    let questions = select_questions(
        load_questions(&cfg.questions)
            .with_context(|| format!("reading {}", cfg.questions.display()))?,
        opts.questions.as_deref(),
    )?;
    let conditions = cfg.select(opts.conditions.as_deref())?;
    let baseline = cfg.baseline().name.clone();

    if opts.dry_run {
        let mut n = 0;
        for c in &conditions {
            for q in &questions {
                for _ in 1..=cfg.repeats {
                    let spec = spec_for(&cfg, q, c.mcp_config.as_deref());
                    let args: Vec<String> = session::command_args(&spec)
                        .iter()
                        .map(|a| shell_quote(a))
                        .collect();
                    println!(
                        "[{}/{}] (cd {} && {} {})",
                        c.name,
                        q.id,
                        shell_quote(&cfg.repo.display().to_string()),
                        session::claude_bin(),
                        args.join(" ")
                    );
                    n += 1;
                }
            }
        }
        println!("dry run: {n} sessions");
        return Ok(None);
    }

    let claude_version = preflight(&cfg)?;
    let needs_index = conditions
        .iter()
        .map(uses_singularrag)
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .any(|b| b);
    let singularrag_version = if needs_index {
        Some(version_of_singularrag()?)
    } else {
        None
    };

    let (run_dir, mut rf) = match &opts.resume {
        Some(dir) => {
            let text = std::fs::read_to_string(dir.join("run.toml"))
                .with_context(|| format!("resume: reading {}", dir.join("run.toml").display()))?;
            let rf: RunFile = toml::from_str(&text).context("resume: parsing run.toml")?;
            (dir.clone(), rf)
        }
        None => {
            let created = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
            let runs_root = opts
                .config
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default()
                .join("runs");
            let run_dir = runs_root.join(format!("{created}-{}", opts.label));
            std::fs::create_dir_all(&run_dir)
                .with_context(|| format!("creating {}", run_dir.display()))?;
            let rf = RunFile {
                run: RunHeader {
                    created,
                    label: opts.label.clone(),
                    commit: cfg.commit.clone(),
                    claude_version,
                    singularrag_version,
                    baseline: baseline.clone(),
                    conditions: conditions.iter().map(|c| c.name.clone()).collect(),
                    questions: questions.iter().map(|q| q.id.clone()).collect(),
                    aborted: BTreeMap::new(),
                },
                config: cfg.clone(),
            };
            write_run_file(&run_dir, &rf)?;
            (run_dir, rf)
        }
    };

    if needs_index {
        let st = Command::new(singularrag_bin())
            .args(["index", "--repo"])
            .arg(&cfg.repo)
            .status()
            .with_context(|| format!("running {} index", singularrag_bin()))?;
        if !st.success() {
            bail!("{} index failed with {st}", singularrag_bin());
        }
        dirty_after(&cfg, "singularrag index")?;
    }

    let checkout = cfg.repo.display().to_string();
    for c in &conditions {
        if rf.run.aborted.contains_key(&c.name) {
            continue;
        }
        let cdir = run_dir.join(&c.name);
        std::fs::create_dir_all(&cdir)?;
        let mcp = match &c.mcp_config {
            Some(src) => {
                let text = std::fs::read_to_string(src)?.replace("<checkout>", &checkout);
                let dst = cdir.join("mcp.json");
                std::fs::write(&dst, text)?;
                Some(dst)
            }
            None => None,
        };
        'condition: for q in &questions {
            for repeat in 1..=cfg.repeats {
                let base = cdir.join(format!("{}-{repeat}", q.id));
                let record_path = base.with_extension("json");
                if record_path.exists() {
                    continue;
                }
                let stream_path = cdir.join(format!("{}-{repeat}.stream.jsonl", q.id));
                let stderr_path = cdir.join(format!("{}-{repeat}.stderr", q.id));
                eprintln!("[{}/{}#{repeat}] running", c.name, q.id);
                let spec = spec_for(&cfg, q, mcp.as_deref());
                let outcome = session::run_session(&cfg.repo, &spec, &stream_path, &stderr_path)?;
                let parsed = stream::parse_stream(&std::fs::read_to_string(&stream_path)?);
                let spawn_error = match outcome.exit_code {
                    Some(0) => None,
                    Some(code) => Some(format!("exit status {code}")),
                    None => Some("killed by signal".to_string()),
                };
                let record = score::score(
                    q,
                    &c.name,
                    repeat,
                    &parsed,
                    cfg.answer_max,
                    spawn_error.as_deref(),
                );
                std::fs::write(&record_path, serde_json::to_string_pretty(&record)?)?;
                eprintln!(
                    "[{}/{}#{repeat}] recall {:.2} · {} tokens · {} tool calls{}",
                    c.name,
                    q.id,
                    record.recall,
                    record.tokens.total(),
                    record.tool_call_total(),
                    if record.failed { " · FAILED" } else { "" }
                );
                dirty_after(&cfg, &format!("{}/{}#{repeat}", c.name, q.id))?;
                if let Some(bad) = parsed.mcp_servers.iter().find(|s| s.status != "connected") {
                    let reason = format!("mcp server {} status {}", bad.name, bad.status);
                    eprintln!("[{}] aborted: {reason}", c.name);
                    rf.run.aborted.insert(c.name.clone(), reason);
                    write_run_file(&run_dir, &rf)?;
                    break 'condition;
                }
            }
        }
    }

    let (meta, baseline, conds, qids) = summary::load_run(&run_dir)?;
    let md = summary::render(&meta, &baseline, &conds, &qids);
    std::fs::write(run_dir.join("summary.md"), &md)?;
    print!("{md}");
    Ok(Some(run_dir))
}
