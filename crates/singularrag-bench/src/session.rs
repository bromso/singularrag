//! One headless Claude Code session (spec §4): the prompt, the schema, the exact command line,
//! and streaming its stdout to disk as it arrives.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};

pub const CLAUDE_ENV: &str = "SINGULARRAG_BENCH_CLAUDE";

pub fn claude_bin() -> String {
    std::env::var(CLAUDE_ENV)
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "claude".to_string())
}

pub fn claude_version() -> Result<String> {
    let out = Command::new(claude_bin())
        .arg("--version")
        .output()
        .with_context(|| format!("running {} --version", claude_bin()))?;
    anyhow::ensure!(out.status.success(), "{} --version failed", claude_bin());
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Identical across conditions (spec §4).
pub fn prompt_for(query: &str, answer_max: usize) -> String {
    format!(
        "{query}\n\nAnswer by listing the symbols that answer the question, as `path::name`, where `path` is relative to the repository root and `name` is the symbol's declared name. List at most {answer_max}, most important first. Use the tools available to you as you see fit."
    )
}

pub fn schema_for(answer_max: usize) -> String {
    serde_json::json!({
        "type": "object",
        "properties": { "symbols": { "type": "array", "items": { "type": "string" }, "maxItems": answer_max } },
        "required": ["symbols"]
    })
    .to_string()
}

#[derive(Debug, Clone)]
pub struct SessionSpec {
    pub prompt: String,
    pub schema: String,
    pub tools: Vec<String>,
    pub mcp_config: Option<PathBuf>,
    pub allowed_tools: Vec<String>,
    /// A settings file with the condition's hooks; loaded even under `--setting-sources ""`.
    pub settings: Option<PathBuf>,
    pub max_turns: u32,
    pub max_budget_usd: f64,
}

/// Everything after the program name, in the spec's order.
pub fn command_args(spec: &SessionSpec) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "-p".into(),
        spec.prompt.clone(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        "--json-schema".into(),
        spec.schema.clone(),
        "--tools".into(),
        spec.tools.join(","),
        "--strict-mcp-config".into(),
    ];
    if let Some(m) = &spec.mcp_config {
        a.push("--mcp-config".into());
        a.push(m.display().to_string());
        if !spec.allowed_tools.is_empty() {
            a.push("--allowedTools".into());
            a.push(spec.allowed_tools.join(","));
        }
    }
    if let Some(s) = &spec.settings {
        a.push("--settings".into());
        a.push(s.display().to_string());
    }
    a.extend([
        "--setting-sources".to_string(),
        String::new(),
        "--disable-slash-commands".to_string(),
        "--permission-mode".to_string(),
        "dontAsk".to_string(),
        "--permission-prompts".to_string(),
        "none".to_string(),
        "--no-session-persistence".to_string(),
        "--max-turns".to_string(),
        spec.max_turns.to_string(),
        "--max-budget-usd".to_string(),
        spec.max_budget_usd.to_string(),
    ]);
    a
}

#[derive(Debug)]
pub struct SessionOutcome {
    pub exit_code: Option<i32>,
    #[allow(dead_code)]
    pub stderr_written: bool,
}

pub fn run_session(
    cwd: &Path,
    spec: &SessionSpec,
    stream_path: &Path,
    stderr_path: &Path,
) -> Result<SessionOutcome> {
    let mut child = Command::new(claude_bin())
        .args(command_args(spec))
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawning {}", claude_bin()))?;
    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");
    let mut file = std::fs::File::create(stream_path)
        .with_context(|| format!("creating {}", stream_path.display()))?;
    let err_thread = std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = std::io::Read::read_to_string(&mut BufReader::new(stderr), &mut buf);
        buf
    });
    for line in BufReader::new(stdout).lines() {
        let line = line?;
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
        file.flush()?;
    }
    let status = child.wait()?;
    let err = err_thread.join().unwrap_or_default();
    let stderr_written = !err.trim().is_empty();
    if stderr_written {
        std::fs::write(stderr_path, err)?;
    }
    Ok(SessionOutcome {
        exit_code: status.code(),
        stderr_written,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> SessionSpec {
        SessionSpec {
            prompt: prompt_for("where is request routing decided", 15),
            schema: schema_for(15),
            tools: vec!["Read".into(), "Grep".into(), "Glob".into()],
            mcp_config: Some(PathBuf::from("/tmp/mcp.json")),
            allowed_tools: vec!["mcp__singularrag".into()],
            settings: None,
            max_turns: 25,
            max_budget_usd: 0.5,
        }
    }

    #[test]
    fn prompt_carries_the_query_and_the_answer_contract() {
        let p = prompt_for("where is request routing decided", 15);
        assert!(p.starts_with("where is request routing decided\n\n"));
        assert!(p.contains("as `path::name`, where `path` is relative to the repository root"));
        assert!(p.contains("List at most 15, most important first."));
        assert!(p.ends_with("Use the tools available to you as you see fit."));
    }

    #[test]
    fn schema_is_an_object_with_a_capped_symbols_array() {
        let v: serde_json::Value = serde_json::from_str(&schema_for(7)).unwrap();
        assert_eq!(v["properties"]["symbols"]["maxItems"], 7);
        assert_eq!(v["required"][0], "symbols");
    }

    #[test]
    fn command_args_match_the_spec_list_in_order() {
        let args = command_args(&spec());
        let expected: Vec<String> = [
            "-p",
            &spec().prompt,
            "--output-format",
            "stream-json",
            "--verbose",
            "--json-schema",
            &spec().schema,
            "--tools",
            "Read,Grep,Glob",
            "--strict-mcp-config",
            "--mcp-config",
            "/tmp/mcp.json",
            "--allowedTools",
            "mcp__singularrag",
            "--setting-sources",
            "",
            "--disable-slash-commands",
            "--permission-mode",
            "dontAsk",
            "--permission-prompts",
            "none",
            "--no-session-persistence",
            "--max-turns",
            "25",
            "--max-budget-usd",
            "0.5",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(args, expected);
    }

    #[test]
    fn no_mcp_config_means_no_mcp_flag() {
        let mut s = spec();
        s.mcp_config = None;
        s.allowed_tools = vec![];
        let args = command_args(&s);
        assert!(!args.iter().any(|a| a == "--mcp-config"));
        assert!(!args.iter().any(|a| a == "--allowedTools"));
        assert!(args.iter().any(|a| a == "--strict-mcp-config"));
    }

    #[test]
    fn allowed_tools_are_only_emitted_when_present() {
        let mut s = spec();
        s.allowed_tools = vec![];
        let args = command_args(&s);
        assert!(!args.iter().any(|a| a == "--allowedTools"));
    }

    #[test]
    fn settings_flag_follows_allowed_tools_and_precedes_setting_sources() {
        let mut s = spec();
        s.settings = Some(PathBuf::from("/x/settings.json"));
        let args = command_args(&s);
        let at = args
            .iter()
            .position(|a| a == "--settings")
            .expect("--settings");
        assert_eq!(args[at + 1], "/x/settings.json");
        assert_eq!(args[at - 2], "--allowedTools");
        assert_eq!(args[at + 2], "--setting-sources");
        let none = command_args(&spec());
        assert!(!none.iter().any(|a| a == "--settings"));
    }
}
