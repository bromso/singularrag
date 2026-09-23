//! `singularrag hook read|map`: the query-first gate (workflow spec §2). Reads the
//! Claude Code hook JSON on stdin, prints a decision, exits 0 whatever happens.

use std::path::{Path, PathBuf};

use serde_json::Value;

pub const DENY_JSON: &str = r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"singularrag: this repository has a map. Call repo_map with your task first; it lists the relevant symbols and who references them. Then read what it points at."}}"#;
pub const ALLOW_JSON: &str =
    r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow"}}"#;

#[derive(Debug, Clone, Copy, PartialEq, clap::ValueEnum)]
pub enum HookEvent {
    /// PreToolUse on Read
    Read,
    /// PreToolUse on mcp__singularrag__repo_map
    Map,
}

pub fn marker_dir(session_id: &str) -> PathBuf {
    let safe: String = session_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    std::env::temp_dir()
        .join("singularrag-hook")
        .join(if safe.is_empty() {
            "unknown".to_string()
        } else {
            safe
        })
}

fn touch(dir: &Path, name: &str) {
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(dir.join(name), b"");
}

/// The decision JSON. Never errors: anything unexpected is an allow.
pub fn run(event: HookEvent, repo: Option<PathBuf>, input: &str) -> String {
    let Ok(v) = serde_json::from_str::<Value>(input) else {
        return ALLOW_JSON.to_string();
    };
    let session = v.get("session_id").and_then(Value::as_str).unwrap_or("");
    if session.is_empty() {
        return ALLOW_JSON.to_string();
    }
    let dir = marker_dir(session);
    match event {
        HookEvent::Map => {
            touch(&dir, "mapped");
            ALLOW_JSON.to_string()
        }
        HookEvent::Read => {
            if dir.join("mapped").exists() || dir.join("denied").exists() {
                return ALLOW_JSON.to_string();
            }
            let root = repo.or_else(|| v.get("cwd").and_then(Value::as_str).map(PathBuf::from));
            let Some(root) = root else {
                return ALLOW_JSON.to_string();
            };
            let Ok(ws) = singularrag_core::workspace::Workspace::open(&root) else {
                return ALLOW_JSON.to_string();
            };
            let file = v
                .pointer("/tool_input/file_path")
                .and_then(Value::as_str)
                .unwrap_or("");
            if file.is_empty() {
                return ALLOW_JSON.to_string();
            }
            let Ok(abs) = Path::new(file).canonicalize() else {
                return ALLOW_JSON.to_string();
            };
            let Some(rel) = ws.rel_of(&abs) else {
                return ALLOW_JSON.to_string();
            };
            let db = ws.dir.join(singularrag_core::engine::DB_FILE);
            let Ok(store) = singularrag_core::store::Store::open_read_only(&db) else {
                return ALLOW_JSON.to_string();
            };
            let indexed: bool = store
                .conn()
                .query_row(
                    "SELECT count(*) FROM files WHERE path = ?1 AND skipped_reason IS NULL",
                    [rel.as_str()],
                    |r| r.get::<_, i64>(0),
                )
                .map(|n| n > 0)
                .unwrap_or(false);
            if !indexed {
                return ALLOW_JSON.to_string();
            }
            touch(&dir, "denied");
            DENY_JSON.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use singularrag_core::fixture::write_ts_mini;

    fn indexed_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        write_ts_mini(dir.path());
        let mut e = singularrag_core::engine::Engine::open(dir.path(), "hook-test").unwrap();
        e.refresh(std::time::Duration::from_secs(60)).unwrap();
        dir
    }

    fn input(session: &str, cwd: &std::path::Path, file: &str) -> String {
        serde_json::json!({ "session_id": session, "cwd": cwd, "hook_event_name": "PreToolUse", "tool_name": "Read", "tool_input": { "file_path": file } }).to_string()
    }

    #[test]
    fn denies_once_then_allows_and_allows_after_a_map() {
        let dir = indexed_repo();
        let session = format!("test-{}", std::process::id());
        let _ = std::fs::remove_dir_all(marker_dir(&session));
        let file = dir.path().join("src/auth/session.ts").display().to_string();
        assert_eq!(
            run(HookEvent::Read, None, &input(&session, dir.path(), &file)),
            DENY_JSON
        );
        assert_eq!(
            run(HookEvent::Read, None, &input(&session, dir.path(), &file)),
            ALLOW_JSON,
            "second read passes"
        );
        let s2 = format!("{session}-b");
        let _ = std::fs::remove_dir_all(marker_dir(&s2));
        assert_eq!(
            run(HookEvent::Map, None, &input(&s2, dir.path(), "")),
            ALLOW_JSON
        );
        assert_eq!(
            run(HookEvent::Read, None, &input(&s2, dir.path(), &file)),
            ALLOW_JSON,
            "a map call lifts the gate"
        );
        let _ = std::fs::remove_dir_all(marker_dir(&session));
        let _ = std::fs::remove_dir_all(marker_dir(&s2));
    }

    #[test]
    fn allows_outside_the_repo_unindexed_files_and_garbage() {
        let dir = indexed_repo();
        let session = format!("test-{}-c", std::process::id());
        let _ = std::fs::remove_dir_all(marker_dir(&session));
        assert_eq!(
            run(
                HookEvent::Read,
                None,
                &input(&session, dir.path(), "/etc/hosts")
            ),
            ALLOW_JSON
        );
        assert_eq!(
            run(
                HookEvent::Read,
                None,
                &input(
                    &session,
                    dir.path(),
                    &dir.path().join("README.md").display().to_string()
                )
            ),
            ALLOW_JSON,
            "not indexed"
        );
        assert_eq!(run(HookEvent::Read, None, "not json"), ALLOW_JSON);
        assert_eq!(
            run(HookEvent::Read, None, &input(&session, dir.path(), "")),
            ALLOW_JSON
        );
        // still un-denied: an indexed file is refused now
        assert_eq!(
            run(
                HookEvent::Read,
                None,
                &input(
                    &session,
                    dir.path(),
                    &dir.path().join("src/cli/login.ts").display().to_string()
                )
            ),
            DENY_JSON
        );
        let _ = std::fs::remove_dir_all(marker_dir(&session));
    }

    #[test]
    fn hook_denies_a_read_in_a_second_root() {
        let d = tempfile::tempdir().unwrap();
        let (app, _notes) = singularrag_core::fixture::write_workspace(d.path());
        let mut e = singularrag_core::engine::Engine::open(d.path(), "hook-test").unwrap();
        e.refresh(std::time::Duration::from_secs(60)).unwrap();
        let session = format!("ws-{}", std::process::id());
        let _ = std::fs::remove_dir_all(marker_dir(&session));
        // Task 5 switches this to `notes.join("README.md")` once Markdown is indexed;
        // until then README.md has no `files` row with skipped_reason NULL, so a read
        // of it would never be denied.
        let file = app.join("src/auth/session.ts").display().to_string();
        let first = run(
            HookEvent::Read,
            Some(d.path().to_path_buf()),
            &input(&session, d.path(), &file),
        );
        assert_eq!(first, DENY_JSON, "a file in the second root is indexed");
        let _ = std::fs::remove_dir_all(marker_dir(&session));
    }
}
