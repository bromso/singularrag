//! `singularrag init`: install the query-first hook and the MCP entry (workflow spec §3).

use std::path::Path;

use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, clap::ValueEnum)]
pub enum Host {
    Claude,
    Codex,
    Copilot,
    All,
}

pub const AGENTS_SNIPPET: &str = "This repository has a singularrag map. Call repo_map with your task before reading files; use find_symbol for a name, trace_path for how two symbols connect, changed for what a diff touches, and entities for what the documents say about a person, system or concept.";

fn hook_entry(matcher: &str, command: String) -> Value {
    json!({ "matcher": matcher, "hooks": [ { "type": "command", "command": command } ] })
}

/// Is one of our hook entries already present for this matcher?
fn has_ours(entries: &[Value], matcher: &str) -> bool {
    entries.iter().any(|e| {
        e["matcher"] == matcher
            && e["hooks"].as_array().is_some_and(|hs| {
                hs.iter().any(|h| {
                    h["command"]
                        .as_str()
                        .is_some_and(|c| c.contains("singularrag hook"))
                })
            })
    })
}

/// Merge the two PreToolUse hooks into a Claude Code settings document, keeping every
/// other key and every other hook.
pub fn merge_hooks(mut existing: Value, bin: &Path) -> Value {
    if !existing.is_object() {
        existing = json!({});
    }
    let bin = bin.display().to_string();
    let hooks = existing
        .as_object_mut()
        .unwrap()
        .entry("hooks")
        .or_insert_with(|| json!({}));
    if !hooks.is_object() {
        *hooks = json!({});
    }
    let pre = hooks
        .as_object_mut()
        .unwrap()
        .entry("PreToolUse")
        .or_insert_with(|| json!([]));
    if !pre.is_array() {
        *pre = json!([]);
    }
    let arr = pre.as_array_mut().unwrap();
    for (matcher, event) in [("Read", "read"), ("mcp__singularrag__repo_map", "map")] {
        if !has_ours(arr, matcher) {
            arr.push(hook_entry(matcher, format!("{bin} hook {event}")));
        }
    }
    existing
}

/// Merge the singularrag server into a `.mcp.json` document, keeping other servers.
pub fn merge_mcp(mut existing: Value) -> Value {
    if !existing.is_object() {
        existing = json!({});
    }
    let servers = existing
        .as_object_mut()
        .unwrap()
        .entry("mcpServers")
        .or_insert_with(|| json!({}));
    if !servers.is_object() {
        *servers = json!({});
    }
    servers
        .as_object_mut()
        .unwrap()
        .entry("singularrag")
        .or_insert_with(|| json!({ "command": "singularrag", "args": ["mcp"] }));
    existing
}

fn read_json(p: &Path) -> anyhow::Result<Value> {
    match std::fs::read_to_string(p) {
        Ok(s) if !s.trim().is_empty() => Ok(serde_json::from_str(&s)?),
        Ok(_) => Ok(json!({})),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(e.into()),
    }
}

/// Write the document; `Ok(false)` when the file already holds exactly this text.
fn write_json(p: &Path, v: &Value) -> anyhow::Result<bool> {
    let text = format!("{}\n", serde_json::to_string_pretty(v)?);
    if std::fs::read_to_string(p).ok().as_deref() == Some(text.as_str()) {
        return Ok(false);
    }
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(p, text)?;
    Ok(true)
}

/// Install for the given host(s); returns the report text the CLI prints.
pub fn run(root: &Path, project: bool, host: Host, bin: &Path) -> anyhow::Result<String> {
    let mut out = String::new();
    if matches!(host, Host::Claude | Host::All) {
        let rel = if project {
            ".claude/settings.json"
        } else {
            ".claude/settings.local.json"
        };
        let settings = root.join(rel);
        let merged = merge_hooks(read_json(&settings)?, bin);
        let verb = if write_json(&settings, &merged)? {
            "wrote"
        } else {
            "unchanged"
        };
        out.push_str(&format!("{verb} {rel}\n"));
        let mcp = root.join(".mcp.json");
        let merged = merge_mcp(read_json(&mcp)?);
        let verb = if write_json(&mcp, &merged)? {
            "wrote"
        } else {
            "unchanged"
        };
        out.push_str(&format!("{verb} .mcp.json\n"));
    }
    if matches!(host, Host::Codex | Host::All) {
        out.push_str(
            "Codex CLI, in ~/.codex/config.toml:\n[mcp_servers.singularrag]\ncommand = \"singularrag\"\nargs = [\"mcp\"]\n\n",
        );
    }
    if matches!(host, Host::Copilot | Host::All) {
        out.push_str(
            "Copilot CLI, in ~/.copilot/mcp-config.json:\n{ \"mcpServers\": { \"singularrag\": { \"type\": \"stdio\", \"command\": \"singularrag\", \"args\": [\"mcp\"] } } }\n\n",
        );
    }
    if matches!(host, Host::Codex | Host::Copilot | Host::All) {
        out.push_str(&format!(
            "Add to AGENTS.md (Codex and Copilot have no hooks):\n{AGENTS_SNIPPET}\n"
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn merge_hooks_adds_ours_keeps_theirs_and_is_idempotent() {
        let bin = std::path::Path::new("/opt/singularrag");
        let existing = json!({ "permissions": { "allow": ["Bash(ls:*)"] }, "hooks": { "PreToolUse": [ { "matcher": "Bash", "hooks": [ { "type": "command", "command": "echo hi" } ] } ] } });
        let once = merge_hooks(existing.clone(), bin);
        assert_eq!(once["permissions"], existing["permissions"]);
        let pre = once["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre.len(), 3);
        assert_eq!(pre[0]["matcher"], "Bash");
        assert_eq!(pre[1]["matcher"], "Read");
        assert_eq!(pre[1]["hooks"][0]["command"], "/opt/singularrag hook read");
        assert_eq!(pre[2]["matcher"], "mcp__singularrag__repo_map");
        assert_eq!(pre[2]["hooks"][0]["command"], "/opt/singularrag hook map");
        assert_eq!(merge_hooks(once.clone(), bin), once, "idempotent");
        assert_eq!(
            merge_hooks(json!({}), bin)["hooks"]["PreToolUse"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn merge_mcp_adds_singularrag_and_keeps_others() {
        let existing = json!({ "mcpServers": { "other": { "command": "x" } } });
        let m = merge_mcp(existing);
        assert_eq!(m["mcpServers"]["other"]["command"], "x");
        assert_eq!(
            m["mcpServers"]["singularrag"],
            json!({ "command": "singularrag", "args": ["mcp"] })
        );
        assert_eq!(merge_mcp(m.clone()), m);
    }

    #[test]
    fn run_writes_local_settings_and_mcp_json_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let bin = std::path::Path::new("/opt/singularrag");
        let report = run(dir.path(), false, Host::All, bin).unwrap();
        let settings = dir.path().join(".claude/settings.local.json");
        let mcp = dir.path().join(".mcp.json");
        assert!(settings.exists() && mcp.exists(), "{report}");
        assert!(
            report.contains(".claude/settings.local.json")
                && report.contains(".mcp.json")
                && report.contains("AGENTS.md"),
            "{report}"
        );
        let a = std::fs::read_to_string(&settings).unwrap();
        run(dir.path(), false, Host::All, bin).unwrap();
        assert_eq!(
            std::fs::read_to_string(&settings).unwrap(),
            a,
            "byte-identical on the second run"
        );
        run(dir.path(), true, Host::Claude, bin).unwrap();
        assert!(dir.path().join(".claude/settings.json").exists());
        let codex_only = run(dir.path(), false, Host::Codex, bin).unwrap();
        assert!(
            codex_only.contains("[mcp_servers.singularrag]") && codex_only.contains(AGENTS_SNIPPET)
        );
    }
}
