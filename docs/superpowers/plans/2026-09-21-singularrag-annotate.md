# singularrag annotate (agent-authored notes) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A third MCP tool, `annotate`, lets the agent write a one-paragraph note on a file or symbol into `map.toml`; the map, `find_symbol`, the ranking and the UI all read it back.

**Architecture:** The note keeps living in `MapConfig` (`.singularrag/map.toml`) with three new optional keys (`by`, `session`, `at`). The engine gains `annotate`, which validates, edits the config and writes through the existing `save_atomic`. The map renderer and `find_symbol` print notes; the ranker seeds files whose notes match the query. The MCP server exposes the tool through the actor. Serve only needs its validation to accept the new keys; the UI shows agent notes with a badge and a delete button.

**Tech Stack:** Rust (rusqlite, toml_edit, rmcp 3.x, axum 0.8, chrono), React 19 + Tailwind v4 + react-aria in `ui/` (Bun only), bun test + happy-dom + axe.

**Spec:** `docs/superpowers/specs/2026-09-21-singularrag-annotate-design.md` (this plan argues from it; the parent spec `docs/superpowers/specs/2026-09-19-singularrag-design.md` §5, §7, §9, §11 are amended in Task 5).

## Global Constraints

- The only write outside the SQLite store stays `.singularrag/map.toml`, through `MapConfig::save_atomic`. No exec. No other file is ever written by the tool.
- A note's `by` is absent (human) or exactly `"agent"`; `session` and `at` are present exactly when `by = "agent"`. One human note and one agent note per target (`path` or `path::symbol`).
- Note text is one paragraph: control characters are not allowed, at most 300 characters (`NOTE_MAX_CHARS`). The engine normalises (control characters and whitespace runs become one space, trimmed) before checking; `PUT /api/map` rejects with 422 and the field name.
- The tool sets `by`, `session` and `at` itself; the caller cannot supply them. The agent never edits or removes a human note.
- `path` must be an indexed, non-skipped file; `symbol`, if given, must be defined in that file; text that trips `secrets::looks_secret` is rejected.
- Note lines in the map are eight spaces, then `note: ` or `note (agent): `, then the text; a file note directly under the file header, a symbol note directly under the symbol's row. `map::fit` measures rendered text, so notes count against the budget.
- Tool description and MCP instructions are string literals in `crates/singularrag/src/mcp/server.rs` kept byte-identical to the `*_DESCRIPTION` constants; the unit test compares them.
- Sandbox rule for anyone running commands in this worktree: never a bare `git stash`; the Bash guard rejects any command containing the substring `eval`, so run `cargo test --workspace` rather than a filter that names `eval`, and use `git add -A`.
- WCAG 2.2 AA in the UI: the agent-note badge is text, the delete button has an accessible name, axe stays clean.
- YAGNI: no agent pins, excludes or boundaries; no notes in SQLite; no cross-session eval condition.

---

## File structure

- `crates/singularrag-core/src/config.rs`: `Note` gains `by`, `session`, `at`; `AGENT`, `NOTE_MAX_CHARS`, `normalise_note_text`, `Note::is_agent`, `MapConfig::note_on`; `validate` enforces the note rules; `check_path` becomes `pub(crate)`.
- `crates/singularrag-core/src/time.rs`: `rfc3339_now`. `crates/singularrag-core/Cargo.toml`: `chrono` (already a workspace dependency).
- `crates/singularrag-core/src/map.rs`: `header` split into `freshness_header` + retrieval suffix; `render` and `fit` take the notes.
- `crates/singularrag-core/src/find.rs`: `render_find` takes the notes.
- `crates/singularrag-core/src/engine.rs`: `AnnotateRequest`, `AnnotateResponse`, `Engine::annotate`; `repo_map` and `find_symbol` pass `&self.config.note` to the renderers.
- `crates/singularrag-core/src/rank.rs`: `NOTE_BOOST`, note matching, `Reasons.note_hit`, `note` seed.
- `crates/singularrag/src/actor.rs`: `Job::Annotate`, `EngineHandle::annotate`.
- `crates/singularrag/src/mcp/server.rs`: `AnnotateArgs`, the `annotate` tool, `ANNOTATE_DESCRIPTION`, `INSTRUCTIONS`.
- `crates/singularrag/tests/mcp.rs`, `crates/singularrag/tests/serve.rs`: integration tests.
- `ui/src/api/types.ts`, `ui/src/lib/mapEdits.ts`, `ui/src/components/DetailPanel.tsx`, `ui/src/App.tsx`, `ui/src/App.test.tsx`, `ui/src/lib/mapEdits.test.ts`.
- Docs: parent spec §5, §7, §9, §11; `README.md` tool list.

---

### Task 1: The note shape and its rules in `MapConfig`

**Files:**
- Modify: `crates/singularrag-core/src/config.rs` (the `Note` struct near line 93, `check_path` near line 36, `validate` near line 247, tests at the end)

**Interfaces:**
- Produces: `pub const AGENT: &str = "agent"`, `pub const NOTE_MAX_CHARS: usize = 300`, `pub fn normalise_note_text(text: &str) -> String`, `Note { path, symbol, text, by: Option<String>, session: Option<String>, at: Option<String> }`, `Note::is_agent(&self) -> bool`, `MapConfig::note_on(&self, path: &str, symbol: Option<&str>, agent: bool) -> Option<&Note>`, `pub(crate) fn check_path(field: &str, p: &str) -> Result<(), MapConfigError>`.

- [ ] **Step 1: Write the failing tests**

Append inside `mod tests` in `config.rs` (the `cfg` helper that parses a string into a `MapConfig` already exists there):

```rust
    #[test]
    fn a_note_round_trips_its_author_keys() {
        let text = "[[note]]\npath = \"src/a.ts\"\nsymbol = \"f\"\ntext = \"entry point\"\nby = \"agent\"\nsession = \"mcp:claude-code:1:2\"\nat = \"2026-09-21T09:14:02Z\"\n[[note]]\npath = \"src/a.ts\"\ntext = \"human\"\n";
        let c = cfg(text);
        assert!(c.validate(&[]).is_ok());
        assert!(c.note[0].is_agent());
        assert!(!c.note[1].is_agent());
        assert_eq!(c.note[0].session.as_deref(), Some("mcp:claude-code:1:2"));
        assert_eq!(c.note[0].at.as_deref(), Some("2026-09-21T09:14:02Z"));
        assert_eq!(c.note_on("src/a.ts", Some("f"), true).map(|n| n.text.as_str()), Some("entry point"));
        assert_eq!(c.note_on("src/a.ts", None, false).map(|n| n.text.as_str()), Some("human"));
        assert!(c.note_on("src/a.ts", None, true).is_none());
        let out = toml_edit::ser::to_string_pretty(&c).unwrap();
        assert!(out.contains("by = \"agent\""), "{out}");
        assert!(!out.contains("by = \"\""), "a human note has no by key: {out}");
        assert_eq!(cfg(&out), c);
    }

    #[test]
    fn validate_enforces_the_note_rules() {
        let base = "[[note]]\npath = \"src/a.ts\"\ntext = \"one\"\n";
        let field = |t: &str| cfg(t).validate(&[]).unwrap_err().field;
        // a second human note on the same target
        assert_eq!(field(&format!("{base}[[note]]\npath = \"src/a.ts\"\ntext = \"two\"\n")), "note[1].path");
        // an agent note beside a human one is fine; a second agent note is not
        let agent = "[[note]]\npath = \"src/a.ts\"\ntext = \"a\"\nby = \"agent\"\nsession = \"s\"\nat = \"t\"\n";
        assert!(cfg(&format!("{base}{agent}")).validate(&[]).is_ok());
        assert_eq!(field(&format!("{base}{agent}{agent}")), "note[2].path");
        // by must be absent or "agent"; session/at go with by
        assert_eq!(field("[[note]]\npath = \"src/a.ts\"\ntext = \"x\"\nby = \"bot\"\n"), "note[0].by");
        assert_eq!(field("[[note]]\npath = \"src/a.ts\"\ntext = \"x\"\nby = \"agent\"\n"), "note[0].by");
        assert_eq!(field("[[note]]\npath = \"src/a.ts\"\ntext = \"x\"\nsession = \"s\"\nat = \"t\"\n"), "note[0].by");
        // one paragraph, at most 300 characters
        assert_eq!(field("[[note]]\npath = \"src/a.ts\"\ntext = \"two\\nlines\"\n"), "note[0].text");
        let long = "x".repeat(301);
        assert_eq!(field(&format!("[[note]]\npath = \"src/a.ts\"\ntext = \"{long}\"\n")), "note[0].text");
        let ok = "x".repeat(300);
        assert!(cfg(&format!("[[note]]\npath = \"src/a.ts\"\ntext = \"{ok}\"\n")).validate(&[]).is_ok());
    }

    #[test]
    fn note_text_is_normalised_to_one_paragraph() {
        assert_eq!(normalise_note_text("  a\n\n b\t c  "), "a b c");
        assert_eq!(normalise_note_text("\u{0}x\r\ny"), "x y");
        assert_eq!(normalise_note_text("   "), "");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p singularrag-core --lib config::tests 2>&1 | grep -E 'error|test result'`
Expected: compile errors (`is_agent`, `note_on`, `normalise_note_text`, `by` do not exist).

- [ ] **Step 3: Implement**

Replace the `Note` struct:

```rust
/// Who may write a note besides a human: the MCP `annotate` tool.
pub const AGENT: &str = "agent";
/// One paragraph, at most this many characters after normalisation.
pub const NOTE_MAX_CHARS: usize = 300;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    pub text: String,
    /// Absent for a human (the UI or a hand edit); `"agent"` for the MCP tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    /// The MCP session key that wrote it; present exactly when `by = "agent"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// RFC 3339 UTC; present exactly when `by = "agent"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
}

impl Note {
    pub fn is_agent(&self) -> bool {
        self.by.as_deref() == Some(AGENT)
    }
}

/// Control characters and whitespace runs become one space; the ends are trimmed.
pub fn normalise_note_text(text: &str) -> String {
    text.split(|c: char| c.is_control() || c.is_whitespace())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}
```

Make `check_path` `pub(crate)`. Add to `impl MapConfig` (next to `is_pinned`):

```rust
    /// The human (`agent == false`) or agent note on `path` / `path::symbol`.
    pub fn note_on(&self, path: &str, symbol: Option<&str>, agent: bool) -> Option<&Note> {
        self.note
            .iter()
            .find(|n| n.path == path && n.symbol.as_deref() == symbol && n.is_agent() == agent)
    }
```

In `validate`, replace the existing `for (i, n) in self.note.iter().enumerate()` loop with:

```rust
        let mut seen_notes: std::collections::HashSet<(String, Option<String>, bool)> =
            std::collections::HashSet::new();
        for (i, n) in self.note.iter().enumerate() {
            check_path(&format!("note[{i}].path"), &n.path)?;
            let err = |f: &str, message: String| MapConfigError {
                field: format!("note[{i}].{f}"),
                message,
            };
            match n.by.as_deref() {
                None | Some(AGENT) => {}
                Some(other) => {
                    return Err(err("by", format!("by must be absent or \"{AGENT}\", not {other:?}")))
                }
            }
            if n.is_agent() != (n.session.is_some() && n.at.is_some()) {
                return Err(err("by", format!("session and at are present exactly when by = \"{AGENT}\"")));
            }
            if n.text.chars().any(char::is_control) {
                return Err(err("text", "text is one paragraph: no line breaks or control characters".into()));
            }
            if n.text.chars().count() > NOTE_MAX_CHARS {
                return Err(err("text", format!("text is longer than {NOTE_MAX_CHARS} characters")));
            }
            if !seen_notes.insert((n.path.clone(), n.symbol.clone(), n.is_agent())) {
                let who = if n.is_agent() { "agent" } else { "human" };
                let target = match &n.symbol {
                    Some(s) => format!("{}::{s}", n.path),
                    None => n.path.clone(),
                };
                return Err(err("path", format!("a second {who} note on {target}")));
            }
        }
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p singularrag-core --lib config::tests 2>&1 | grep -E 'test result|panicked'`
Expected: all config tests pass (the existing 17 plus 3).

- [ ] **Step 5: Run the whole workspace and commit**

Run: `cargo test --workspace 2>&1 | grep -E 'test result|FAILED'` (the UI and serve tests construct `Note` values without the new keys through serde defaults, so nothing else should break; if a Rust test constructs `Note { .. }` literally, add `by: None, session: None, at: None`).

```bash
git add -A
git commit -m "feat(core): notes carry by, session and at; one human and one agent note per target"
```

---

### Task 2: `Engine::annotate` and the freshness header

**Files:**
- Modify: `crates/singularrag-core/Cargo.toml` (add `chrono = { workspace = true }` under `[dependencies]`)
- Modify: `crates/singularrag-core/src/time.rs`
- Modify: `crates/singularrag-core/src/map.rs` (`header`, near line 52)
- Modify: `crates/singularrag-core/src/engine.rs` (new request/response types after `FindResponse`; `annotate` after `find_symbol`; tests)

**Interfaces:**
- Consumes: Task 1's `AGENT`, `NOTE_MAX_CHARS`, `normalise_note_text`, `check_path`, `Note`.
- Produces: `pub fn rfc3339_now() -> String` (time.rs); `pub fn freshness_header(index_version: &str, git_head: Option<&str>, stale: usize) -> String` (map.rs; `header` now equals `freshness_header` + `" · retrieval r_{id:06}"`); `AnnotateRequest { path: String, symbol: Option<String>, text: String }`, `AnnotateResponse { text: String, removed: bool, notes_on_file: usize, stale_count: usize, lock_timeout: bool }`, `Engine::annotate(&mut self, req: &AnnotateRequest) -> Result<AnnotateResponse>`.

- [ ] **Step 1: Write the failing tests**

In `map.rs` tests, extend `header_and_footer_format`:

```rust
        assert_eq!(
            freshness_header("7f3a2c9d1e0b", Some("9b1e0d4f5a6b7c8d"), 0),
            "# singularrag · index 7f3a2c · HEAD 9b1e0d4 · fresh"
        );
```

In `time.rs` add a test module:

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn rfc3339_now_is_utc_to_the_second() {
        let s = super::rfc3339_now();
        assert_eq!(s.len(), 20, "{s}");
        assert!(s.ends_with('Z') && s.as_bytes()[10] == b'T', "{s}");
    }
}
```

In `engine.rs` tests (the module already imports `write_ts_mini` and has `Engine::open(dir.path(), "...")` patterns):

```rust
    fn annotate(e: &mut Engine, path: &str, symbol: Option<&str>, text: &str) -> Result<AnnotateResponse> {
        e.annotate(&AnnotateRequest {
            path: path.into(),
            symbol: symbol.map(str::to_string),
            text: text.into(),
        })
    }

    #[test]
    fn annotate_adds_replaces_and_removes_the_agent_note_only() {
        let dir = tempfile::tempdir().unwrap();
        write_ts_mini(dir.path());
        std::fs::create_dir_all(dir.path().join(".singularrag")).unwrap();
        std::fs::write(
            dir.path().join(".singularrag/map.toml"),
            "# mine\n[[note]]\npath = \"src/auth/session.ts\"\nsymbol = \"createSession\"\ntext = \"human says\"\n",
        )
        .unwrap();
        let mut e = Engine::open(dir.path(), "mcp:test:1:2").unwrap();
        let r = annotate(&mut e, "src/auth/session.ts", Some("createSession"), " first\nnote ").unwrap();
        assert!(r.text.starts_with("# singularrag · index "), "{}", r.text);
        assert!(r.text.ends_with("noted src/auth/session.ts::createSession (2 notes on this file)\n"), "{}", r.text);
        assert!(!r.removed);
        let on_disk = std::fs::read_to_string(dir.path().join(".singularrag/map.toml")).unwrap();
        assert!(on_disk.starts_with("# mine\n"), "comments survive: {on_disk}");
        assert!(on_disk.contains("text = \"human says\""), "{on_disk}");
        assert!(on_disk.contains("text = \"first note\"") && on_disk.contains("by = \"agent\"") && on_disk.contains("session = \"mcp:test:1:2\""), "{on_disk}");
        // replace
        let r = annotate(&mut e, "src/auth/session.ts", Some("createSession"), "second").unwrap();
        assert!(r.text.contains("(2 notes on this file)"), "{}", r.text);
        let c = e.config();
        assert_eq!(c.note.len(), 2);
        assert_eq!(c.note_on("src/auth/session.ts", Some("createSession"), true).unwrap().text, "second");
        assert_eq!(c.note_on("src/auth/session.ts", Some("createSession"), false).unwrap().text, "human says");
        // remove
        let r = annotate(&mut e, "src/auth/session.ts", Some("createSession"), "").unwrap();
        assert!(r.removed);
        assert!(r.text.ends_with("removed your note on src/auth/session.ts::createSession\n"), "{}", r.text);
        assert_eq!(e.config().note.len(), 1, "the human note stays");
        // removing again is a no-op that says so
        let r = annotate(&mut e, "src/auth/session.ts", Some("createSession"), "").unwrap();
        assert!(r.text.ends_with("no note of yours on src/auth/session.ts::createSession\n"), "{}", r.text);
        // a file-level note
        let r = annotate(&mut e, "src/cli/login.ts", None, "the CLI entry point").unwrap();
        assert!(r.text.ends_with("noted src/cli/login.ts (1 note on this file)\n"), "{}", r.text);
    }

    #[test]
    fn annotate_rejects_bad_targets_and_secret_like_text() {
        let dir = tempfile::tempdir().unwrap();
        write_ts_mini(dir.path());
        let mut e = Engine::open(dir.path(), "mcp:test:1:2").unwrap();
        let err = |r: Result<AnnotateResponse>| r.unwrap_err().to_string();
        assert!(err(annotate(&mut e, "../x.ts", None, "t")).contains("path"));
        assert!(err(annotate(&mut e, "src/nope.ts", None, "t")).contains("not an indexed file"));
        assert!(err(annotate(&mut e, "src/auth/session.ts", Some("nope"), "t")).contains("not defined in src/auth/session.ts"));
        assert!(err(annotate(&mut e, "src/auth/session.ts", None, &"x".repeat(301))).contains("300"));
        let e2 = err(annotate(&mut e, "src/auth/session.ts", None, "token AKIAIOSFODNN7EXAMPLE1 here"));
        assert!(e2.contains("looks like a secret"), "{e2}");
        assert!(e.config().note.is_empty(), "nothing was written");
        assert!(!dir.path().join(".singularrag/map.toml").exists());
    }
```

(`AKIA` followed by 16 upper-case alphanumerics is the AWS access-key pattern `secrets.rs` already matches; if that rule's name differs, keep the assertion on "looks like a secret".)

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p singularrag-core --lib 2>&1 | grep -E '^error|test result' | head`
Expected: compile errors (`freshness_header`, `rfc3339_now`, `AnnotateRequest`, `annotate` missing).

- [ ] **Step 3: Implement**

`crates/singularrag-core/Cargo.toml`, under `[dependencies]`: `chrono = { workspace = true }`.

`time.rs`, append:

```rust
/// UTC to the second, e.g. `2026-09-21T09:14:02Z`.
pub fn rfc3339_now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}
```

`map.rs`, replace `header`:

```rust
/// `# singularrag · index 7f3a2c · HEAD 9b1e0d4 · fresh`: the first line of every tool
/// response, with or without a retrieval id.
pub fn freshness_header(index_version: &str, git_head: Option<&str>, stale: usize) -> String {
    let idx: String = index_version.chars().take(6).collect();
    let head = match git_head {
        Some(h) if !h.is_empty() => h.chars().take(7).collect::<String>(),
        _ => "none".to_string(),
    };
    let fresh = if stale == 0 {
        "fresh".to_string()
    } else {
        format!("STALE: {stale} files changed since index")
    };
    format!("# singularrag · index {idx} · HEAD {head} · {fresh}")
}

pub fn header(
    index_version: &str,
    git_head: Option<&str>,
    stale: usize,
    retrieval_id: i64,
) -> String {
    format!(
        "{} · retrieval r_{retrieval_id:06}",
        freshness_header(index_version, git_head, stale)
    )
}
```

`engine.rs`, after `FindResponse`:

```rust
#[derive(Debug, Clone)]
pub struct AnnotateRequest {
    pub path: String,
    pub symbol: Option<String>,
    /// Empty removes the agent's note on the target.
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct AnnotateResponse {
    pub text: String,
    pub removed: bool,
    pub notes_on_file: usize,
    pub stale_count: usize,
    /// See `MapResponse::lock_timeout`.
    pub lock_timeout: bool,
}
```

In `impl Engine`, after `find_symbol`:

```rust
    /// Spec (annotate design §3): validate in order, then edit the agent's own note on
    /// the target and write `map.toml`. No retrieval row: an annotation is not a
    /// retrieval, and the file watcher turns the write into a change event.
    pub fn annotate(&mut self, req: &AnnotateRequest) -> Result<AnnotateResponse> {
        use crate::config::{normalise_note_text, Note, AGENT, NOTE_MAX_CHARS};
        use rusqlite::OptionalExtension;
        let stats = self.refresh(self.refresh_budget)?;
        let bad = |field: &str, message: String| crate::Error::Config(format!("{field}: {message}"));
        crate::config::check_path("path", &req.path).map_err(|e| bad(&e.field, e.message))?;
        let file_id: Option<i64> = self
            .store
            .conn()
            .query_row(
                "SELECT id FROM files WHERE path = ?1 AND skipped_reason IS NULL",
                params![req.path],
                |r| r.get(0),
            )
            .optional()?;
        let Some(file_id) = file_id else {
            return Err(bad("path", format!("{} is not an indexed file", req.path)));
        };
        if let Some(sym) = &req.symbol {
            let defined: i64 = self.store.conn().query_row(
                "SELECT count(*) FROM symbols WHERE file_id = ?1 AND name = ?2",
                params![file_id, sym],
                |r| r.get(0),
            )?;
            if defined == 0 {
                return Err(bad("symbol", format!("{sym} is not defined in {}", req.path)));
            }
        }
        let text = normalise_note_text(&req.text);
        if text.chars().count() > NOTE_MAX_CHARS {
            return Err(bad("text", format!("text is longer than {NOTE_MAX_CHARS} characters")));
        }
        if let Some(kind) = crate::secrets::looks_secret(&text) {
            return Err(bad("text", format!("text looks like a secret ({kind}); notes are committed")));
        }
        self.reload_config_if_changed()?;
        let symbol = req.symbol.clone();
        let mut config = self.config.clone();
        let before = config.note.len();
        config
            .note
            .retain(|n| !(n.is_agent() && n.path == req.path && n.symbol == symbol));
        let had_own = config.note.len() < before;
        let removed = text.is_empty();
        if !removed {
            config.note.push(Note {
                path: req.path.clone(),
                symbol: symbol.clone(),
                text,
                by: Some(AGENT.to_string()),
                session: Some(self.session_key.clone()),
                at: Some(crate::time::rfc3339_now()),
            });
        }
        let target = match &symbol {
            Some(s) => format!("{}::{s}", req.path),
            None => req.path.clone(),
        };
        let notes_on_file = config.note.iter().filter(|n| n.path == req.path).count();
        let line = if removed && had_own {
            format!("removed your note on {target}")
        } else if removed {
            format!("no note of yours on {target}")
        } else {
            let plural = if notes_on_file == 1 { "" } else { "s" };
            format!("noted {target} ({notes_on_file} note{plural} on this file)")
        };
        // Write when adding or replacing, and when a removal actually removed something;
        // a no-op removal touches nothing.
        if !removed || had_own {
            config
                .validate(&self.config.deny.extra_patterns)
                .map_err(|e| bad(&e.field, e.message))?;
            config.save_atomic(&self.root)?;
            self.config = config;
            self.config_mtime = map_toml_mtime(&self.root);
        }
        let (version, head) = self.index_meta()?;
        Ok(AnnotateResponse {
            text: format!(
                "{}\n{line}\n",
                map::freshness_header(&version, head.as_deref(), stats.remaining)
            ),
            removed,
            notes_on_file,
            stale_count: stats.remaining,
            lock_timeout: stats.lock_timeout,
        })
    }
```

(`params` is already imported in `engine.rs`. `notes_on_file` is counted on the new config, so a no-op removal reports the current count.)

- [ ] **Step 4: Run the tests**

Run: `cargo test -p singularrag-core --lib 2>&1 | grep -E 'test result|panicked'`
Expected: all pass. If the secret fixture string does not trip a rule, check the rule names in `secrets.rs` `rules()` and use a private-key header (`-----BEGIN RSA PRIVATE KEY-----`) instead; the assertion on "looks like a secret" stays.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(core): Engine::annotate writes the agent's own note through save_atomic; freshness_header"
```

---

### Task 3: Notes in the map and in `find_symbol`

**Files:**
- Modify: `crates/singularrag-core/src/map.rs` (`render`, `fit`, tests)
- Modify: `crates/singularrag-core/src/find.rs` (`render_find`, tests)
- Modify: `crates/singularrag-core/src/engine.rs` (`repo_map`, `find_symbol` call sites; `eval.rs` and any other caller of `render`/`fit`/`render_find`: run `grep -rn 'map::render\|map::fit\|render_find' crates/` and pass `&config.note`)

**Interfaces:**
- Consumes: Task 1's `Note`, `Note::is_agent`.
- Produces: `pub fn render(items: &[ScoredSymbol], n: usize, notes: &[Note]) -> String`, `pub fn fit(items: &[ScoredSymbol], budget: usize, notes: &[Note]) -> usize`, `pub fn render_find(hits: &[FindHit], notes: &[Note]) -> String`, `pub fn note_line(n: &Note) -> String` (map.rs; `        note: text` / `        note (agent): text`).

- [ ] **Step 1: Write the failing tests**

`map.rs` tests (the `sym` helper exists; add a `note` helper):

```rust
    fn note(path: &str, symbol: Option<&str>, text: &str, agent: bool) -> crate::config::Note {
        crate::config::Note {
            path: path.into(),
            symbol: symbol.map(str::to_string),
            text: text.into(),
            by: agent.then(|| crate::config::AGENT.to_string()),
            session: agent.then(|| "s".to_string()),
            at: agent.then(|| "t".to_string()),
        }
    }

    #[test]
    fn render_places_file_notes_under_the_header_and_symbol_notes_under_the_row() {
        let mut five = sym("src/a.ts", 5, "export function five()", 0.9);
        five.name = "five".into();
        let mut nine = sym("src/a.ts", 9, "export class Nine", 0.8);
        nine.name = "Nine".into();
        let notes = vec![
            note("src/a.ts", None, "the a module", false),
            note("src/a.ts", None, "agent on the file", true),
            note("src/a.ts", Some("Nine"), "agent on Nine", true),
            note("src/z.ts", None, "not served", false),
        ];
        assert_eq!(
            render(&[five, nine], 2, &notes),
            "src/a.ts:\n\
             \x20       note: the a module\n\
             \x20       note (agent): agent on the file\n\
             \x20   5  export function five()\n\
             \x20   9  export class Nine\n\
             \x20       note (agent): agent on Nine\n"
        );
    }

    #[test]
    fn fit_counts_notes_against_the_budget() {
        let items: Vec<ScoredSymbol> = (1..=50)
            .map(|i| sym("src/a.ts", i, "export function f()", 1.0 / i as f64))
            .collect();
        let big = note("src/a.ts", None, &"word ".repeat(40), false);
        assert!(fit(&items, 100, &[big]) < fit(&items, 100, &[]));
    }
```

Update the existing `render_*` and `fit_*` tests in `map.rs` to pass `&[]` as the third argument.

`find.rs` tests: in `render_find` tests (the module builds `FindHit`s through the engine; find the test asserting `"   referenced from 2 files: ..."`), add:

```rust
    #[test]
    fn render_find_prints_the_symbol_note_then_the_file_note() {
        let hit = FindHit {
            symbol_id: 1,
            path: "src/a.ts".into(),
            line: 3,
            kind: "function".into(),
            name: "f".into(),
            signature: "export function f()".into(),
            referenced_from: vec![],
            total_ref_files: 0,
            in_file_refs: 0,
        };
        let notes = vec![
            crate::config::Note { path: "src/a.ts".into(), symbol: None, text: "file".into(), by: None, session: None, at: None },
            crate::config::Note { path: "src/a.ts".into(), symbol: Some("f".into()), text: "sym".into(), by: Some("agent".into()), session: Some("s".into()), at: Some("t".into()) },
        ];
        assert_eq!(
            render_find(&[hit], &notes),
            "src/a.ts:3  function  export function f()\n   referenced from 0 files\n        note (agent): sym\n        note: file\n"
        );
    }
```

Update existing `render_find(...)` calls in tests to pass `&[]`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p singularrag-core --lib 2>&1 | grep -E '^error|test result' | head -5`
Expected: compile errors (arity).

- [ ] **Step 3: Implement**

`map.rs`:

```rust
use crate::config::Note;

/// `        note: text` or `        note (agent): text`.
pub fn note_line(n: &Note) -> String {
    if n.is_agent() {
        format!("        note (agent): {}\n", n.text)
    } else {
        format!("        note: {}\n", n.text)
    }
}

/// Human note first, then the agent's, for one target.
fn notes_for<'a>(notes: &'a [Note], path: &str, symbol: Option<&str>) -> impl Iterator<Item = &'a Note> {
    let mut v: Vec<&Note> = notes
        .iter()
        .filter(|n| n.path == path && n.symbol.as_deref() == symbol)
        .collect();
    v.sort_by_key(|n| n.is_agent());
    v.into_iter()
}

pub fn render(items: &[ScoredSymbol], n: usize, notes: &[Note]) -> String {
    let top = &items[..n.min(items.len())];
    let mut order: Vec<&str> = Vec::new();
    for s in top {
        if !order.contains(&s.path.as_str()) {
            order.push(&s.path);
        }
    }
    let mut out = String::new();
    for path in order {
        let mut rows: Vec<&ScoredSymbol> = top.iter().filter(|s| s.path == path).collect();
        rows.sort_by_key(|s| s.line_start);
        out.push_str(path);
        out.push(':');
        out.push_str(&referenced_by(path, &rows));
        out.push('\n');
        for nt in notes_for(notes, path, None) {
            out.push_str(&note_line(nt));
        }
        let mut noted: Vec<&str> = Vec::new();
        for s in rows {
            out.push_str(&format!("{:>5}  {}\n", s.line_start, s.signature));
            // A symbol note goes under the first row with that name.
            if !noted.contains(&s.name.as_str()) {
                for nt in notes_for(notes, path, Some(&s.name)) {
                    out.push_str(&note_line(nt));
                }
                noted.push(&s.name);
            }
        }
    }
    out
}

pub fn fit(items: &[ScoredSymbol], budget: usize, notes: &[Note]) -> usize {
    let (mut lo, mut hi) = (0usize, items.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if approx_tokens(&render(items, mid, notes)) <= budget {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    lo
}
```

`find.rs`: `pub fn render_find(hits: &[FindHit], notes: &[crate::config::Note]) -> String`; after the `referenced from` line of each hit:

```rust
        for n in notes.iter().filter(|n| n.path == h.path && n.symbol.as_deref() == Some(h.name.as_str())) {
            out.push_str(&crate::map::note_line(n));
        }
        for n in notes.iter().filter(|n| n.path == h.path && n.symbol.is_none()) {
            out.push_str(&crate::map::note_line(n));
        }
```

`engine.rs`: `let served = map::fit(&ranked, budget, &self.config.note); let body = map::render(&ranked, served, &self.config.note);` and `crate::find::render_find(hits, &self.config.note)`. Fix any other caller the grep found the same way.

- [ ] **Step 4: Run the tests**

Run: `cargo test --workspace 2>&1 | grep -E 'test result|FAILED|panicked'`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(core): the map and find_symbol print notes; notes count against the budget"
```

---

### Task 4: A matching note seeds the ranking

**Files:**
- Modify: `crates/singularrag-core/src/rank.rs` (`Reasons`, constants near line 16, `rank_symbols`)
- Modify: `ui/src/api/types.ts` (`Reasons` gains `note_hit: boolean`) — one line, folded here so the JSON contract and the type change together

**Interfaces:**
- Consumes: `crate::tokens::query_terms`, Task 1's `Note`.
- Produces: `pub const NOTE_BOOST: f64 = 5.0`, `Reasons.note_hit: bool`, a `"note"` seed string, `pub fn note_matches(text: &str, terms: &[String]) -> bool`.

- [ ] **Step 1: Write the failing test**

`rank.rs` tests (the `indexed()` helper and `MapConfig::parse` exist):

```rust
    #[test]
    fn a_note_matching_the_query_seeds_its_file_and_its_symbol() {
        let (_dir, store) = indexed();
        let config = MapConfig::parse(
            "[[note]]\npath = \"src/cli/login.ts\"\nsymbol = \"login\"\ntext = \"the onboarding flow starts here\"\nby = \"agent\"\nsession = \"s\"\nat = \"t\"\n",
        )
        .unwrap();
        let plain = rank_symbols(&store, &MapConfig::default(), Some("onboarding flow"), &[]).unwrap();
        let noted = rank_symbols(&store, &config, Some("onboarding flow"), &[]).unwrap();
        let pos = |v: &[ScoredSymbol]| v.iter().position(|s| s.name == "login").unwrap();
        assert!(pos(&noted) < pos(&plain), "the note lifts login: {} vs {}", pos(&noted), pos(&plain));
        let login = noted.iter().find(|s| s.name == "login").unwrap();
        assert!(login.reasons.note_hit);
        assert!(login.reasons.seeds.iter().any(|s| s == "note"), "{:?}", login.reasons.seeds);
        let other = noted.iter().find(|s| s.name == "createSession").unwrap();
        assert!(!other.reasons.note_hit);
        assert!(note_matches("The Onboarding flow", &["onboarding".into()]));
        assert!(!note_matches("onboard", &["onboarding".into()]), "whole words only");
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p singularrag-core --lib rank::tests::a_note 2>&1 | grep -E '^error|test result' | head -3`
Expected: compile error (`note_hit`, `note_matches`).

- [ ] **Step 3: Implement**

Add `pub note_hit: bool,` to `Reasons` (after `query_ident_match`) and the constant `pub const NOTE_BOOST: f64 = 5.0;` next to `FTS_FILE_BOOST`. Add:

```rust
/// A query term appears in the note as a whole word, after the same splitting the
/// query gets (`tokens::query_terms`), so `createSession` in a note matches `session`.
pub fn note_matches(text: &str, terms: &[String]) -> bool {
    if terms.is_empty() {
        return false;
    }
    let words = crate::tokens::query_terms(text);
    terms.iter().any(|t| words.contains(t))
}
```

In `rank_symbols`, right after `let terms = ...`:

```rust
    // Notes whose text matches the query: files get a seed, named symbols a bonus.
    let mut note_files: HashSet<String> = HashSet::new();
    let mut note_symbols: HashSet<(String, String)> = HashSet::new();
    for n in &config.note {
        if note_matches(&n.text, &terms) {
            note_files.insert(n.path.clone());
            if let Some(s) = &n.symbol {
                note_symbols.insert((n.path.clone(), s.clone()));
            }
        }
    }
```

In the personalization loop, after the `fts_files` block:

```rust
        if note_files.contains(&node.path) {
            personalization[i] += NOTE_BOOST;
            seeds[i].push("note".to_string());
        }
```

In the scoring closure, after the `fts_hit` bonus:

```rust
            let path = &g.nodes[fi].path;
            let note_hit = note_symbols.contains(&(path.clone(), s.name.clone()))
                || note_files.contains(path);
            if note_symbols.contains(&(path.clone(), s.name.clone())) {
                score += fr;
            }
```

and `note_hit,` in the `Reasons { .. }` literal at `rank.rs` line ~300. The only other literal, in `engine.rs` `find_symbol` (line ~286), ends with `..Default::default()` and needs nothing.

`ui/src/api/types.ts`: `pinned: boolean; fts_hit: boolean; query_ident_match: boolean; note_hit: boolean;`.

- [ ] **Step 4: Run the tests**

Run: `cargo test --workspace 2>&1 | grep -E 'test result|FAILED|panicked'` and `cd ui && bun run typecheck && bun test 2>&1 | tail -3`
Expected: all pass (a UI test fixture that builds `Reasons` literally needs `note_hit: false`).

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(rank): a note matching the query seeds its file and symbol; reasons carry note_hit"
```

---

### Task 5: The `annotate` tool through the actor, plus the spec amendments

**Files:**
- Modify: `crates/singularrag/src/actor.rs` (`Job`, `EngineHandle`, `handle`)
- Modify: `crates/singularrag/src/mcp/server.rs` (`INSTRUCTIONS`, new constant, `AnnotateArgs`, the tool, tests)
- Modify: `crates/singularrag/tests/mcp.rs` (the description constants copied there, the tool-list test, a new call test)
- Modify: `docs/superpowers/specs/2026-09-19-singularrag-design.md` §5, §7, §9, §11; `README.md` "What the agent sees"

**Interfaces:**
- Consumes: Task 2's `AnnotateRequest`, `AnnotateResponse`, `Engine::annotate`.
- Produces: `Job::Annotate(AnnotateRequest, oneshot::Sender<Reply<AnnotateResponse>>)`, `EngineHandle::annotate(&self, req: AnnotateRequest) -> Reply<AnnotateResponse>`, `pub const ANNOTATE_DESCRIPTION: &str`, `AnnotateArgs { path: String, symbol: Option<String>, text: String }`.

The description, byte for byte (used in the constant, the `#[tool]` literal, and the copy in `tests/mcp.rs`):

```
Record what you learned about a file or symbol that its signatures do not say: what it is for, an entry point, a trap, a convention. One or two sentences; the next session and the developer will see it in the map. `path` is repo-relative; `symbol` narrows the note to one definition in that file. Empty `text` removes your note. You can replace your own note on a target; a note the developer wrote is theirs.
```

The new `INSTRUCTIONS`:

```
singularrag gives you a ranked map of this repository. Call repo_map first with your task as the query and answer from it; read only to confirm a detail the map does not show. Use find_symbol to locate a name. When you learn something about a file that its signatures do not say, record it with annotate so the next session starts from it. repo_map and find_symbol are read-only; annotate writes only a note into .singularrag/map.toml. A STALE header means files changed since indexing; the index catches up in the background.
```

- [ ] **Step 1: Write the failing tests**

`server.rs` unit test: rename `tool_list_is_exactly_the_two_spec_tools` to `tool_list_is_exactly_the_three_spec_tools`, expect `vec!["annotate", "find_symbol", "repo_map"]`, and add:

```rust
        let ann = tools.iter().find(|t| t.name == "annotate").unwrap();
        assert_eq!(ann.description.as_deref(), Some(ANNOTATE_DESCRIPTION));
        let schema = serde_json::to_value(&ann.input_schema).unwrap();
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("path") && props.contains_key("symbol") && props.contains_key("text"));
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|r| r == "path") && required.iter().any(|r| r == "text"));
```

`tests/mcp.rs`: add `const ANNOTATE_DESCRIPTION` beside the two existing copies; in `lists_exactly_the_two_tools_with_spec_descriptions` expect three tools (`annotate` sorts first) and its description; add:

```rust
#[tokio::test]
async fn annotate_writes_a_note_the_next_map_shows() {
    let dir = tempfile::tempdir().unwrap();
    write_ts_mini(dir.path());
    let client = connect(dir.path(), &[]).await;
    let r = client
        .call_tool(
            CallToolRequestParams::new("annotate").with_arguments(object!({
                "path": "src/auth/session.ts",
                "symbol": "createSession",
                "text": "Sessions are minted here; the CLI and the middleware both call it."
            })),
        )
        .await
        .unwrap();
    assert_ne!(r.is_error, Some(true), "{}", text_of(&r));
    let t = text_of(&r);
    assert!(t.starts_with("# singularrag · index ") && t.contains("noted src/auth/session.ts::createSession (1 note on this file)"), "{t}");
    let on_disk = std::fs::read_to_string(dir.path().join(".singularrag/map.toml")).unwrap();
    assert!(on_disk.contains("by = \"agent\"") && on_disk.contains("session = \"mcp:"), "{on_disk}");

    let map = client
        .call_tool(CallToolRequestParams::new("repo_map").with_arguments(object!({ "query": "minted", "budget_tokens": 1024 })))
        .await
        .unwrap();
    let map_text = text_of(&map);
    assert!(map_text.contains("        note (agent): Sessions are minted here"), "{map_text}");

    let bad = client
        .call_tool(CallToolRequestParams::new("annotate").with_arguments(object!({ "path": "src/nope.ts", "text": "x" })))
        .await
        .unwrap();
    assert_eq!(bad.is_error, Some(true));
    assert!(text_of(&bad).contains("not an indexed file"), "{}", text_of(&bad));
    client.cancel().await.unwrap();
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p singularrag 2>&1 | grep -E '^error|test result' | head -5`
Expected: compile errors (`ANNOTATE_DESCRIPTION`), then the tool-list assertion.

- [ ] **Step 3: Implement**

`actor.rs`: import `AnnotateRequest, AnnotateResponse` from `singularrag_core::engine`; add the variant `Annotate(AnnotateRequest, oneshot::Sender<Reply<AnnotateResponse>>),` to `Job` after `Find`; add to `EngineHandle`:

```rust
    pub async fn annotate(&self, req: AnnotateRequest) -> Reply<AnnotateResponse> {
        self.ask(|tx| Job::Annotate(req, tx)).await
    }
```

and to `handle`, after the `Job::Find` arm:

```rust
            Job::Annotate(req, reply) => {
                let out = self
                    .engine()
                    .and_then(|e| e.annotate(&req).map_err(|e| e.to_string()));
                if let Ok(r) = &out {
                    self.note(r.stale_count, r.lock_timeout);
                }
                let _ = reply.send(out);
            }
```

`server.rs`: replace `INSTRUCTIONS` with the text above; add `#[allow(dead_code)] pub const ANNOTATE_DESCRIPTION: &str = "…";` (the description above); add:

```rust
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AnnotateArgs {
    /// Repo-relative path of an indexed file.
    pub path: String,
    /// A symbol defined in that file; omit for a note on the file.
    pub symbol: Option<String>,
    /// One or two sentences; empty removes your note on the target.
    pub text: String,
}

impl From<AnnotateArgs> for AnnotateRequest {
    fn from(a: AnnotateArgs) -> Self {
        AnnotateRequest {
            path: a.path,
            symbol: a.symbol.filter(|s| !s.trim().is_empty()),
            text: a.text,
        }
    }
}
```

and the tool in the `#[tool_router]` impl:

```rust
    #[tool(
        name = "annotate",
        description = "Record what you learned about a file or symbol that its signatures do not say: what it is for, an entry point, a trap, a convention. One or two sentences; the next session and the developer will see it in the map. `path` is repo-relative; `symbol` narrows the note to one definition in that file. Empty `text` removes your note. You can replace your own note on a target; a note the developer wrote is theirs."
    )]
    async fn annotate(
        &self,
        Parameters(args): Parameters<AnnotateArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        text_result(self.handle.annotate(args.into()).await.map(|r| r.text))
    }
```

Docs: in the parent spec, §5 first bullet of the non-goals list "Write or exec tools. Read-only." → "No exec tools. One write tool, `annotate`, bounded to notes in `map.toml`; nothing else outside the store is ever written. *Amended 2026-09-21 (annotate design).*"; §7's `map.toml` paragraph gains "Notes carry `by`, `session` and `at`; the agent edits its own notes through `annotate`, the UI everything."; §9 "No other tools, resources or prompts in v0." → "A third tool, `annotate` (2026-09-21, `docs/superpowers/specs/2026-09-21-singularrag-annotate-design.md` §3), writes the agent's note on a file or symbol into `map.toml`. No other tools, resources or prompts."; §11 controls first bullet → "Only retrieval rows and `map.toml` are ever written, the latter by the UI and by the agent's own notes; no exec." `README.md`, after the `find_symbol` sentence: "`annotate` lets the agent leave a one-paragraph note on a file or symbol; it lands in `map.toml`, shows in the next map with an `(agent)` tag, and the developer can delete it from the page."

- [ ] **Step 4: Run the tests**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace 2>&1 | grep -E 'test result|FAILED|panicked'`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(mcp): the annotate tool; specs and README amended"
```

---

### Task 6: Serve accepts the note keys; the page shows agent notes

**Files:**
- Modify: `crates/singularrag/tests/serve.rs` (new test)
- Modify: `ui/src/api/types.ts` (`Note`)
- Modify: `ui/src/lib/mapEdits.ts` and its existing `ui/src/lib/mapEdits.test.ts` (add a `describe("notes", …)` block; the file already imports `setNote` and defines `empty`/`base` configs)
- Modify: `ui/src/lib/reasons.ts` (`reasonsToSentences`) and `ui/src/lib/reasons.test.ts`
- Modify: `ui/src/components/DetailPanel.tsx`, `ui/src/App.tsx`, `ui/src/App.test.tsx`

**Interfaces:**
- Consumes: Task 1's validation (the server side needs no code change: `MapPut` flattens `MapConfig`, and `validate` now enforces the rules), Task 4's `note_hit`.
- Produces: `Note = { path: string; symbol?: string; text: string; by?: "agent"; session?: string; at?: string }`, `agentNoteFor(c, path, symbol?) -> Note | undefined`, `removeAgentNote(c, path, symbol?) -> MapConfig`; `setNote` and `noteFor` touch human notes only and `setNote` normalises whitespace to one paragraph.

- [ ] **Step 1: Write the failing tests**

`tests/serve.rs` (helpers `spawn`, `get`, `client` exist; model the PUT on `put_map_with_stale_version_is_409`):

```rust
#[tokio::test]
async fn put_map_round_trips_agent_notes_and_rejects_a_second_one() {
    let dir = tempfile::tempdir().unwrap();
    write_ts_mini(dir.path());
    let s = spawn(dir.path());
    let put = |body: serde_json::Value| {
        let url = format!("{}/api/map", s.url);
        let token = s.token.clone();
        async move { client().put(url).bearer_auth(token).json(&body).send().await.unwrap() }
    };
    let agent = serde_json::json!({ "path": "src/auth/session.ts", "text": "agent note", "by": "agent", "session": "mcp:x:1:2", "at": "2026-09-21T09:14:02Z" });
    let human = serde_json::json!({ "path": "src/auth/session.ts", "text": "human note" });
    let body = |notes: Vec<serde_json::Value>| serde_json::json!({ "pin": [], "exclude": [], "note": notes, "boundary": [], "deny": { "extra_patterns": [] } });
    let r = put(body(vec![human.clone(), agent.clone()])).await;
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    let doc: serde_json::Value = get(&s, "/map").await.json().await.unwrap();
    assert_eq!(doc["note"][1]["by"], "agent");
    assert_eq!(doc["note"][1]["session"], "mcp:x:1:2");
    assert!(doc["note"][0].get("by").is_none(), "{doc}");
    let r = put(body(vec![human, agent.clone(), agent])).await;
    assert_eq!(r.status(), 422);
    let e: serde_json::Value = r.json().await.unwrap();
    assert_eq!(e["field"], "note[2].path");
}
```

`ui/src/lib/mapEdits.test.ts`, appended (extend the import line with `agentNoteFor, noteFor, removeAgentNote`):

```ts
const noted = (): MapConfig => ({
  pin: [], exclude: [], boundary: [], deny: { extra_patterns: [] },
  note: [
    { path: "src/a.ts", symbol: "f", text: "human" },
    { path: "src/a.ts", symbol: "f", text: "agent", by: "agent", session: "s", at: "t" },
  ],
});

describe("notes", () => {
  test("noteFor and setNote see only the human note", () => {
    expect(noteFor(noted(), "src/a.ts", "f")).toBe("human");
    const c = setNote(noted(), "src/a.ts", "f", "  two\nlines  ");
    expect(c.note).toEqual([
      { path: "src/a.ts", symbol: "f", text: "agent", by: "agent", session: "s", at: "t" },
      { path: "src/a.ts", symbol: "f", text: "two lines" },
    ]);
    expect(setNote(noted(), "src/a.ts", "f", " ").note).toEqual([noted().note[1]]);
  });
  test("agentNoteFor and removeAgentNote touch only the agent note", () => {
    expect(agentNoteFor(noted(), "src/a.ts", "f")?.text).toBe("agent");
    expect(agentNoteFor(noted(), "src/a.ts")).toBeUndefined();
    expect(removeAgentNote(noted(), "src/a.ts", "f").note).toEqual([noted().note[0]]);
  });
});
```

`ui/src/lib/reasons.test.ts`, a new test (the file's existing tests show how a `Reasons` value is built; every literal there gains `note_hit: false`):

```ts
  test("a note match has its own sentence", () => {
    const r = { score: 1, file_rank: 1, seeds: ["note"], referenced_by: [], pinned: false, fts_hit: false, query_ident_match: false, note_hit: true };
    expect(reasonsToSentences(r, 1, 1)).toContain("Matches a note.");
  });
```

`ui/src/App.test.tsx`: next to `a_draft_note_survives_a_live_change_refetch` (reuse its setup: `mapState`, the panel query, `calls`), add:

```tsx
  test("an_agent_note_shows_with_a_badge_and_delete_removes_only_it", async () => {
    mapState = {
      pin: [], exclude: [], boundary: [], deny: { extra_patterns: [] },
      note: [
        { path: "src/auth/session.ts", symbol: "createSession", text: "mine" },
        { path: "src/auth/session.ts", symbol: "createSession", text: "from the agent", by: "agent", session: "mcp:claude-code:1:2", at: "2026-09-21T09:14:02Z" },
      ],
    };
    // (open the createSession row exactly as the draft-note test does)
    const panel = await openDetail("createSession");
    expect(within(panel).getByText("from the agent")).toBeTruthy();
    expect(within(panel).getByText("agent")).toBeTruthy();
    expect((within(panel).getByLabelText("Note") as HTMLTextAreaElement).value).toBe("mine");
    await user.click(within(panel).getByRole("button", { name: "Delete the agent note" }));
    await waitFor(() => expect(calls("/api/map").length).toBe(1));
    const saved = JSON.parse(calls("/api/map")[0].body);
    expect(saved.note).toEqual([{ path: "src/auth/session.ts", symbol: "createSession", text: "mine" }]);
  });
```

(Adapt `openDetail`, `user`, `calls` to the helper names the file actually uses; the draft-note test shows them.)

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p singularrag --test serve put_map_round_trips 2>&1 | grep -E 'test result|panicked'` and `cd ui && bun test 2>&1 | tail -5`
Expected: the serve test fails only if Task 1 is not merged (it should pass already once Task 1 is in; keep it as the contract test); the UI tests fail on missing exports and elements.

- [ ] **Step 3: Implement**

`ui/src/api/types.ts`: `export type Note = { path: string; symbol?: string; text: string; by?: "agent"; session?: string; at?: string };`

`ui/src/lib/mapEdits.ts`:

```ts
const sameTarget = (n: Note, path: string, symbol?: string) => n.path === path && (n.symbol ?? undefined) === symbol;
const isAgent = (n: Note) => n.by === "agent";
/** One paragraph, like the engine: whitespace runs become one space. */
export const oneParagraph = (text: string) => text.split(/\s+/).filter(Boolean).join(" ");

export function setNote(c: MapConfig, path: string, symbol: string | undefined, text: string): MapConfig {
  const note = c.note.filter((n) => !(sameTarget(n, path, symbol) && !isAgent(n)));
  const t = oneParagraph(text);
  if (t) note.push(symbol ? { path, symbol, text: t } : { path, text: t });
  return { ...c, note };
}
export const noteFor = (c: MapConfig, path: string, symbol?: string) =>
  c.note.find((n) => sameTarget(n, path, symbol) && !isAgent(n))?.text ?? "";
export const agentNoteFor = (c: MapConfig, path: string, symbol?: string) =>
  c.note.find((n) => sameTarget(n, path, symbol) && isAgent(n));
export function removeAgentNote(c: MapConfig, path: string, symbol?: string): MapConfig {
  return { ...c, note: c.note.filter((n) => !(sameTarget(n, path, symbol) && isAgent(n))) };
}
```

(import `Note` from `@/api/types`.)

`DetailPanel.tsx`: add a prop `onRemoveAgentNote: (path: string, symbol?: string) => void`; after the `Note` label block:

```tsx
      {agentNote && (
        <div className="rounded border p-2 text-sm">
          <p className="flex items-center gap-2">
            <span className="rounded bg-muted px-1 text-xs font-medium">agent</span>
            <time dateTime={agentNote.at} className="text-xs text-muted-foreground">{agentNote.at}</time>
          </p>
          <p className="mt-1">{agentNote.text}</p>
          <Button type="button" variant="outline" size="sm" className="mt-2" aria-label="Delete the agent note" onClick={() => onRemoveAgentNote(row.path, symbol)}>
            Delete
          </Button>
        </div>
      )}
```

with `const agentNote = row ? agentNoteFor(map, row.path, symbol) : undefined;` next to `saved`, importing `agentNoteFor` from `@/lib/mapEdits` and `Button` from the existing shadcn component (`@/components/ui/button`, check the import other panels use).

`ui/src/lib/reasons.ts`, in `reasonsToSentences` after the query-match lines: `if (r.note_hit) out.push("Matches a note.");`.

`App.tsx`: pass `onRemoveAgentNote={(p, s) => save((c) => removeAgentNote(c, p, s))}` next to `onNote`, importing `removeAgentNote`.

- [ ] **Step 4: Run the tests**

Run: `cd ui && bun run typecheck && bun test 2>&1 | tail -3 && bun run build 2>&1 | tail -1`, then `cargo test --workspace 2>&1 | grep -E 'test result|FAILED'` (the embedded UI is rebuilt into the binary).
Expected: all pass, axe clean (the existing axe test covers the panel).

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(serve,ui): map.toml accepts agent notes; the panel shows them with a badge and a delete"
```

---

## Self-review

- Spec §2 (shape, limits): Task 1. §3 (tool, validation order, response lines): Task 2 (engine) and Task 5 (MCP). §4 (map, find, ranking): Tasks 3 and 4. §5 (UI): Task 6. §6 (two writers): no code, documented in the spec. §7 (security): Task 2's checks. §8 (spec amendments): Task 5. §9 (tests): each task carries its tests; the serve 422 test is in Task 6; the MCP "changes the next repo_map" test is in Task 5.
- Type consistency: `Note` fields (`by`, `session`, `at`) are `Option<String>` everywhere in Rust and optional strings in TS; `AnnotateRequest`/`AnnotateResponse` names match between Tasks 2 and 5; `render`/`fit`/`render_find` arities match between Task 3's definitions and its call sites; `note_hit` matches between Task 4's Rust and TS.
- No placeholders: every code step carries its code; the only "find it yourself" instructions are grep-based locations of existing call sites.
