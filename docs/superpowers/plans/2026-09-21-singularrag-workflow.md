# singularrag agent workflow parity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A query-first Claude Code hook installed by `singularrag init`, two graph tools (`trace_path`, `changed`) as MCP tools and CLI subcommands, and a tier-two condition that measures the hook.

**Architecture:** Two core modules (`trace.rs`, `changed.rs`) compute over the existing store and file graph; the engine wraps them with refresh, header and provenance like `repo_map`; the actor and MCP server expose them; the binary gains `hook`, `init`, `path` and `changed` subcommands. The bench gains a per-condition `settings` file passed as `--settings`, and classifies hook denials from the stream's tool results.

**Tech Stack:** Rust (rusqlite, rmcp 3.x, clap, serde_json, std::process for git), React UI in `ui/` (Bun only), bench crate.

**Spec:** `docs/superpowers/specs/2026-09-21-singularrag-workflow-design.md` (binding; parent spec `docs/superpowers/specs/2026-09-19-singularrag-design.md` §9/§11 and tier-two spec §3/§5 are amended in Tasks 4 and 7).

## Global Constraints

- The hook subcommands read stdin and the index and write only empty marker files under `std::env::temp_dir().join("singularrag-hook").join(<session_id>)`; on any error they print the allow decision and exit 0. A session is denied at most once.
- Denial JSON, byte for byte: `{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"singularrag: this repository has a map. Call repo_map with your task first; it lists the relevant symbols and who references them. Then read what it points at."}}`. Allow JSON: `{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow"}}`.
- `init` writes only `.claude/settings.local.json` (or `.claude/settings.json` with `--project`) and `.mcp.json`, merging, never removing, idempotent; the hook command is the absolute path of the running binary.
- `trace_path`: breadth-first over `graph::build_graph` edges in both directions, `MAX_PATH_DEPTH = 6`; `changed`: `git diff --unified=0 [base]` plus untracked files, `MAX_CHANGED = 50`; `base` starting with `-` is rejected. Both tools return identifiers, signatures and paths only, refresh before answering, print the freshness header, and record a `retrievals` row (`tool = "trace_path"` / `"changed"`).
- Tool descriptions and `INSTRUCTIONS` are string literals in `server.rs` kept byte-identical to the `*_DESCRIPTION` constants and the copies in `tests/mcp.rs`.
- Bench: a denial is a hook denial when its `tool_name` is `Read` and the stream's `tool_result` for its `tool_use_id` contains `singularrag:`; hook denials are counted in `Record.hook_denials` and never abort; any other denial aborts as before.
- Sandbox: never a bare `git stash`; the worktree's Bash guard rejects any command containing the substring `eval`, so run `cargo test --workspace` and `git add -A`. Use absolute paths.
- YAGNI: no SessionStart hook, no hooks for other hosts, no new UI views.

---

## File structure

- `crates/singularrag-core/src/trace.rs` (new): `trace_path`, `Hop`, `Trace`, `MAX_PATH_DEPTH`, `render_trace`.
- `crates/singularrag-core/src/changed.rs` (new): `changed`, `ChangedSymbol`, `Changed`, `MAX_CHANGED`, `render_changed`, git subprocess and hunk parsing.
- `crates/singularrag-core/src/blast.rs`: `referencing_files` becomes `pub(crate)`.
- `crates/singularrag-core/src/lib.rs`: `pub mod trace; pub mod changed;`.
- `crates/singularrag-core/src/engine.rs`: `TraceRequest/Response`, `ChangedRequest/Response`, `Engine::trace_path`, `Engine::changed`.
- `crates/singularrag/src/main.rs`: `Path`, `Changed`, `Hook`, `Init` subcommands; `crates/singularrag/src/hook.rs` (new); `crates/singularrag/src/init.rs` (new).
- `crates/singularrag/src/actor.rs`, `crates/singularrag/src/mcp/server.rs`, `crates/singularrag/tests/mcp.rs`, `crates/singularrag/tests/cli.rs`.
- `crates/singularrag-bench/src/{config.rs,session.rs,stream.rs,score.rs,run.rs,summary.rs}`, `eval/conditions/singularrag-hook.json`, `eval/tier2.toml`.
- `ui/src/components/RetrievalsRail.tsx` + test.
- Docs: parent spec §9, §11; tier-two spec §3, §5; README.

---

### Task 1: `trace_path` in core

**Files:**
- Create: `crates/singularrag-core/src/trace.rs`
- Modify: `crates/singularrag-core/src/lib.rs` (add `pub mod trace;`)

**Interfaces:**
- Consumes: `graph::build_graph(store, config, &[], &HashSet::new()) -> FileGraph { nodes: Vec<FileNode{id,path}>, index_of: HashMap<i64,usize>, edges: Vec<Edge{src,dst,name,weight}>, self_edges }`.
- Produces: `pub const MAX_PATH_DEPTH: usize = 6`; `pub struct Hop { pub from: String, pub to: String, pub name: String, pub forward: bool }`; `pub struct Trace { pub from: (String, String), pub to: (String, String), pub hops: Vec<Hop> }`; `pub fn trace_path(store, config, from_path, from_symbol, to_path, to_symbol) -> Result<Option<Trace>>` (`None` = no path within the depth; `Err(Error::Config(..))` for an unknown symbol); `pub fn render_trace(t: &Trace) -> String`; `pub fn path_symbols(store, t: &Trace) -> Result<Vec<(i64, String, String, u32, String, String)>>` = `(symbol_id, path, name, line_start, kind, signature)` for provenance in hop order, deduplicated.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MapConfig;
    use crate::fixture::write_ts_mini;
    use crate::index::Indexer;
    use crate::store::Store;

    fn indexed() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        write_ts_mini(dir.path());
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        Indexer::new(&store, dir.path(), &MapConfig::default()).unwrap().refresh(None).unwrap();
        (dir, store)
    }

    #[test]
    fn one_hop_between_login_and_create_session() {
        let (_d, store) = indexed();
        let t = trace_path(&store, &MapConfig::default(), "src/cli/login.ts", "login", "src/auth/session.ts", "createSession")
            .unwrap()
            .expect("a path");
        assert_eq!(t.hops.len(), 1);
        assert_eq!(t.hops[0].from, "src/cli/login.ts");
        assert_eq!(t.hops[0].to, "src/auth/session.ts");
        assert_eq!(t.hops[0].name, "createSession");
        assert!(t.hops[0].forward);
        let text = render_trace(&t);
        assert_eq!(
            text,
            "src/cli/login.ts::login → src/auth/session.ts::createSession (references createSession)\n# 1 hop\n"
        );
        let syms = path_symbols(&store, &t).unwrap();
        assert_eq!(syms.iter().map(|s| format!("{}::{}", s.1, s.2)).collect::<Vec<_>>(), vec!["src/cli/login.ts::login", "src/auth/session.ts::createSession"]);
    }

    #[test]
    fn reverse_edges_count_and_same_file_is_zero_hops() {
        let (_d, store) = indexed();
        // session.ts does not reference login.ts; the path goes backwards along the same edge.
        let t = trace_path(&store, &MapConfig::default(), "src/auth/session.ts", "createSession", "src/cli/login.ts", "login")
            .unwrap()
            .expect("a path");
        assert_eq!(t.hops.len(), 1);
        assert!(!t.hops[0].forward);
        assert!(render_trace(&t).contains("(referenced through createSession)"), "{}", render_trace(&t));
        let same = trace_path(&store, &MapConfig::default(), "src/auth/session.ts", "createSession", "src/auth/session.ts", "SessionStore").unwrap().unwrap();
        assert!(same.hops.is_empty());
        assert_eq!(render_trace(&same), "src/auth/session.ts::createSession → src/auth/session.ts::SessionStore (same file)\n# 0 hops\n");
    }

    #[test]
    fn unknown_symbols_error_and_an_unreachable_file_is_none() {
        let (_d, store) = indexed();
        let e = trace_path(&store, &MapConfig::default(), "src/nope.ts", "x", "src/auth/session.ts", "createSession").unwrap_err().to_string();
        assert!(e.contains("from: src/nope.ts::x is not in the index"), "{e}");
        let e = trace_path(&store, &MapConfig::default(), "src/auth/session.ts", "createSession", "src/auth/session.ts", "nope").unwrap_err().to_string();
        assert!(e.contains("to: src/auth/session.ts::nope is not in the index"), "{e}");
        // Every fixture file is connected (login.ts imports log.ts and session.ts), so an
        // unreachable pair needs an exclude: without src/cli/, log.ts hangs alone.
        let cfg = MapConfig::parse("[[exclude]]\npath = \"src/cli/\"\n").unwrap();
        let none = trace_path(&store, &cfg, "src/util/log.ts", "log", "src/auth/session.ts", "createSession").unwrap();
        assert!(none.is_none());
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p singularrag-core --lib trace:: 2>&1 | grep -E '^error|test result' | head -3`
Expected: compile errors (module missing).

- [ ] **Step 3: Implement**

```rust
//! `trace_path`: the shortest chain of references between two symbols, breadth-first
//! over the ranking's file graph in both directions. Name-based, like everything else.

use std::collections::{HashMap, HashSet, VecDeque};

use rusqlite::{params, OptionalExtension};

use crate::config::MapConfig;
use crate::graph::build_graph;
use crate::store::Store;
use crate::{Error, Result};

pub const MAX_PATH_DEPTH: usize = 6;

#[derive(Debug, Clone, PartialEq)]
pub struct Hop {
    pub from: String,
    pub to: String,
    /// The symbol name the edge carries.
    pub name: String,
    /// `true` when `from` references `name` defined in `to`; `false` when the edge is
    /// walked backwards (`to` references `name` defined in `from`).
    pub forward: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Trace {
    pub from: (String, String),
    pub to: (String, String),
    pub hops: Vec<Hop>,
}

fn symbol_exists(store: &Store, path: &str, name: &str) -> Result<bool> {
    let n: Option<i64> = store
        .conn()
        .query_row(
            "SELECT s.id FROM symbols s JOIN files f ON f.id = s.file_id WHERE f.path = ?1 AND s.name = ?2 LIMIT 1",
            params![path, name],
            |r| r.get(0),
        )
        .optional()?;
    Ok(n.is_some())
}

pub fn trace_path(
    store: &Store,
    config: &MapConfig,
    from_path: &str,
    from_symbol: &str,
    to_path: &str,
    to_symbol: &str,
) -> Result<Option<Trace>> {
    if !symbol_exists(store, from_path, from_symbol)? {
        return Err(Error::Config(format!("from: {from_path}::{from_symbol} is not in the index")));
    }
    if !symbol_exists(store, to_path, to_symbol)? {
        return Err(Error::Config(format!("to: {to_path}::{to_symbol} is not in the index")));
    }
    let endpoints = (
        (from_path.to_string(), from_symbol.to_string()),
        (to_path.to_string(), to_symbol.to_string()),
    );
    if from_path == to_path {
        return Ok(Some(Trace { from: endpoints.0, to: endpoints.1, hops: Vec::new() }));
    }
    let g = build_graph(store, config, &[], &HashSet::new())?;
    let idx = |p: &str| g.nodes.iter().position(|n| n.path == p);
    let (Some(start), Some(goal)) = (idx(from_path), idx(to_path)) else {
        return Ok(None); // excluded from the graph
    };
    // adjacency: node -> (neighbour, name, forward)
    let mut adj: HashMap<usize, Vec<(usize, &str, bool)>> = HashMap::new();
    for e in &g.edges {
        adj.entry(e.src).or_default().push((e.dst, &e.name, true));
        adj.entry(e.dst).or_default().push((e.src, &e.name, false));
    }
    for v in adj.values_mut() {
        v.sort_by(|a, b| a.1.cmp(b.1).then(a.0.cmp(&b.0)));
    }
    let mut prev: HashMap<usize, (usize, String, bool)> = HashMap::new();
    let mut depth: HashMap<usize, usize> = HashMap::from([(start, 0)]);
    let mut queue = VecDeque::from([start]);
    while let Some(u) = queue.pop_front() {
        if u == goal {
            break;
        }
        let d = depth[&u];
        if d == MAX_PATH_DEPTH {
            continue;
        }
        for (v, name, forward) in adj.get(&u).into_iter().flatten() {
            if depth.contains_key(v) {
                continue;
            }
            depth.insert(*v, d + 1);
            prev.insert(*v, (u, name.to_string(), *forward));
            queue.push_back(*v);
        }
    }
    if !depth.contains_key(&goal) {
        return Ok(None);
    }
    let mut hops = Vec::new();
    let mut cur = goal;
    while cur != start {
        let (p, name, forward) = prev[&cur].clone();
        hops.push(Hop {
            from: g.nodes[p].path.clone(),
            to: g.nodes[cur].path.clone(),
            name,
            forward,
        });
        cur = p;
    }
    hops.reverse();
    Ok(Some(Trace { from: endpoints.0, to: endpoints.1, hops }))
}

/// One line per hop, then `# N hops`. The left symbol of a hop is the symbol the walk
/// arrived at (the previous hop's name, or `from` for the first); the right symbol is the
/// hop's name, or `to`'s symbol on the last hop when they differ.
pub fn render_trace(t: &Trace) -> String {
    let mut out = String::new();
    if t.hops.is_empty() {
        out.push_str(&format!("{}::{} → {}::{} (same file)\n# 0 hops\n", t.from.0, t.from.1, t.to.0, t.to.1));
        return out;
    }
    let mut left = t.from.1.clone();
    let last = t.hops.len() - 1;
    for (i, h) in t.hops.iter().enumerate() {
        let right = if i == last { t.to.1.clone() } else { h.name.clone() };
        let via = if h.forward {
            format!("references {}", h.name)
        } else {
            format!("referenced through {}", h.name)
        };
        out.push_str(&format!("{}::{} → {}::{} ({via})\n", h.from, left, h.to, right));
        left = right;
    }
    let n = t.hops.len();
    out.push_str(&format!("# {n} hop{}\n", if n == 1 { "" } else { "s" }));
    out
}

/// The symbols along the path for provenance: `from`, each hop's named symbol where it is
/// defined, then `to`; deduplicated, in order. `(symbol_id, path, name, line_start, kind, signature)`.
pub fn path_symbols(store: &Store, t: &Trace) -> Result<Vec<(i64, String, String, u32, String, String)>> {
    let mut wanted: Vec<(String, String)> = vec![t.from.clone()];
    for h in &t.hops {
        let defined_in = if h.forward { &h.to } else { &h.from };
        wanted.push((defined_in.clone(), h.name.clone()));
    }
    wanted.push(t.to.clone());
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut stmt = store.conn().prepare(
        "SELECT s.id, s.line_start, s.kind, s.signature FROM symbols s JOIN files f ON f.id = s.file_id
         WHERE f.path = ?1 AND s.name = ?2 ORDER BY s.line_start LIMIT 1",
    )?;
    for (path, name) in wanted {
        if !seen.insert((path.clone(), name.clone())) {
            continue;
        }
        let row: Option<(i64, u32, String, String)> = stmt
            .query_row(params![path, name], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .optional()?;
        if let Some((id, line, kind, sig)) = row {
            out.push((id, path, name, line, kind, sig));
        }
    }
    Ok(out)
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p singularrag-core --lib trace:: 2>&1 | grep -E 'test result|panicked'`
Expected: 3 passed.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(core): trace_path, the shortest reference chain between two symbols"
```

---

### Task 2: `changed` in core

**Files:**
- Create: `crates/singularrag-core/src/changed.rs`
- Modify: `crates/singularrag-core/src/blast.rs` (`fn referencing_files` → `pub(crate) fn referencing_files`), `crates/singularrag-core/src/lib.rs` (`pub mod changed;`)

**Interfaces:**
- Consumes: `blast::referencing_files(store, name) -> Result<Vec<(i64, String)>>`.
- Produces: `pub const MAX_CHANGED: usize = 50`; `pub struct ChangedSymbol { pub symbol_id: i64, pub path: String, pub name: String, pub kind: String, pub signature: String, pub line_start: u32, pub line_end: u32, pub referenced_by: Vec<String> }`; `pub struct Changed { pub base: String, pub symbols: Vec<ChangedSymbol>, pub files_without_symbols: Vec<String>, pub truncated: bool }`; `pub fn changed(store, config, root: &Path, base: Option<&str>) -> Result<Changed>`; `pub fn render_changed(c: &Changed) -> String`; `pub fn parse_unified0(diff: &str) -> Vec<(String, u32, u32)>` (path, first line, last line of each new-side hunk; a pure-deletion hunk is `(line, line)`).

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MapConfig;
    use crate::fixture::write_ts_mini;
    use crate::index::Indexer;
    use crate::store::Store;
    use std::process::Command;

    fn git(root: &std::path::Path, args: &[&str]) {
        let st = Command::new("git").arg("-C").arg(root).args(args).status().unwrap();
        assert!(st.success(), "git {args:?}");
    }

    /// The fixture as a git repo with one commit, indexed.
    fn repo() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        write_ts_mini(dir.path());
        git(dir.path(), &["init", "-q"]);
        git(dir.path(), &["-c", "user.email=t@t", "-c", "user.name=t", "add", "."]);
        git(dir.path(), &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "-m", "base"]);
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        Indexer::new(&store, dir.path(), &MapConfig::default()).unwrap().refresh(None).unwrap();
        (dir, store)
    }

    #[test]
    fn parses_unified_zero_hunks() {
        let diff = "diff --git a/src/a.ts b/src/a.ts\n--- a/src/a.ts\n+++ b/src/a.ts\n@@ -3,0 +4,2 @@\n+x\n+y\n@@ -10 +12 @@\n-z\n+w\n@@ -20,2 +22,0 @@\n-p\n-q\n";
        assert_eq!(
            parse_unified0(diff),
            vec![("src/a.ts".into(), 4, 5), ("src/a.ts".into(), 12, 12), ("src/a.ts".into(), 22, 22)]
        );
    }

    #[test]
    fn a_modified_symbol_and_an_untracked_file_are_reported_with_referrers() {
        let (dir, store) = repo();
        // Touch createSession's body (line 4 of session.ts in the fixture: inside the function).
        let p = dir.path().join("src/auth/session.ts");
        let text = std::fs::read_to_string(&p).unwrap().replacen("return {", "// changed\n  return {", 1);
        std::fs::write(&p, text).unwrap();
        std::fs::write(dir.path().join("src/new.ts"), "export const fresh = 1;\n").unwrap();
        Indexer::new(&store, dir.path(), &MapConfig::default()).unwrap().refresh(None).unwrap();
        let c = changed(&store, &MapConfig::default(), dir.path(), None).unwrap();
        assert_eq!(c.base, "HEAD");
        let cs = c.symbols.iter().find(|s| s.name == "createSession").expect("createSession changed");
        assert_eq!(cs.path, "src/auth/session.ts");
        assert!(cs.referenced_by.contains(&"src/http/middleware.ts".to_string()) && cs.referenced_by.contains(&"src/cli/login.ts".to_string()), "{:?}", cs.referenced_by);
        assert!(c.symbols.iter().any(|s| s.path == "src/new.ts" && s.name == "fresh"), "untracked file counts as fully changed: {:?}", c.symbols);
        let text = render_changed(&c);
        assert!(text.contains("src/auth/session.ts::createSession (lines "), "{text}");
        assert!(text.contains("← src/cli/login.ts, src/http/middleware.ts"), "{text}");
        assert!(text.ends_with("changed since HEAD · 2 referencing files\n"), "{text}");
    }

    #[test]
    fn a_change_outside_any_symbol_is_a_file_line_and_base_is_honoured() {
        let (dir, store) = repo();
        let p = dir.path().join("src/cli/login.ts");
        let text = format!("// top comment\n{}", std::fs::read_to_string(&p).unwrap());
        std::fs::write(&p, text).unwrap();
        git(dir.path(), &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qam", "comment"]);
        Indexer::new(&store, dir.path(), &MapConfig::default()).unwrap().refresh(None).unwrap();
        let now = changed(&store, &MapConfig::default(), dir.path(), None).unwrap();
        assert!(now.symbols.is_empty() && now.files_without_symbols.is_empty(), "clean tree: {now:?}");
        let vs_base = changed(&store, &MapConfig::default(), dir.path(), Some("HEAD~1")).unwrap();
        assert_eq!(vs_base.base, "HEAD~1");
        assert_eq!(vs_base.files_without_symbols, vec!["src/cli/login.ts".to_string()]);
        assert!(render_changed(&vs_base).contains("src/cli/login.ts (no symbol touched)"));
        let e = changed(&store, &MapConfig::default(), dir.path(), Some("--output=/tmp/x")).unwrap_err().to_string();
        assert!(e.contains("base"), "{e}");
    }

    #[test]
    fn a_non_git_directory_errors() {
        let dir = tempfile::tempdir().unwrap();
        write_ts_mini(dir.path());
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        Indexer::new(&store, dir.path(), &MapConfig::default()).unwrap().refresh(None).unwrap();
        let e = changed(&store, &MapConfig::default(), dir.path(), None).unwrap_err().to_string();
        assert!(e.contains("not a git checkout"), "{e}");
    }
}
```

(If the fixture's `createSession` body does not contain `return {`, read `fixture.rs` and pick a line inside the function to edit; the assertion is that the symbol containing the edited line is reported. Line numbers in `render_changed` are the symbol's `line_start-line_end`. The referencing-file count in the footer counts distinct files across all changed symbols.)

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p singularrag-core --lib changed:: 2>&1 | grep -E '^error|test result' | head -3`
Expected: compile errors.

- [ ] **Step 3: Implement**

In `blast.rs`: `pub(crate) fn referencing_files(...)`.

`changed.rs`:

```rust
//! `changed`: the symbols a diff touches and who references them. `git diff --unified=0`
//! against HEAD (or a ref) plus untracked files, mapped onto symbol line ranges.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use rusqlite::params;

use crate::blast::referencing_files;
use crate::config::MapConfig;
use crate::store::Store;
use crate::{Error, Result};

pub const MAX_CHANGED: usize = 50;

#[derive(Debug, Clone)]
pub struct ChangedSymbol {
    pub symbol_id: i64,
    pub path: String,
    pub name: String,
    pub kind: String,
    pub signature: String,
    pub line_start: u32,
    pub line_end: u32,
    /// Other files referencing the symbol's name, path order.
    pub referenced_by: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Changed {
    pub base: String,
    pub symbols: Vec<ChangedSymbol>,
    pub files_without_symbols: Vec<String>,
    pub truncated: bool,
}

fn git(root: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| Error::Config(format!("running git: {e}")))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        if err.contains("not a git repository") {
            return Err(Error::Config("not a git checkout".into()));
        }
        return Err(Error::Config(format!("git {}: {}", args.join(" "), err.trim())));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `(path, first, last)` new-side line ranges of every hunk in a `--unified=0` diff.
pub fn parse_unified0(diff: &str) -> Vec<(String, u32, u32)> {
    let mut out = Vec::new();
    let mut path = String::new();
    for line in diff.lines() {
        if let Some(p) = line.strip_prefix("+++ b/") {
            path = p.to_string();
        } else if line.starts_with("+++ ") {
            path.clear(); // deleted file: no new side
        } else if let Some(rest) = line.strip_prefix("@@ ") {
            if path.is_empty() {
                continue;
            }
            let Some(plus) = rest.split(' ').find(|s| s.starts_with('+')) else { continue };
            let spec = &plus[1..];
            let (start, count) = match spec.split_once(',') {
                Some((s, c)) => (s.parse::<u32>().unwrap_or(0), c.parse::<u32>().unwrap_or(0)),
                None => (spec.parse::<u32>().unwrap_or(0), 1),
            };
            let last = if count == 0 { start } else { start + count - 1 };
            out.push((path.clone(), start.max(1), last.max(1)));
        }
    }
    out
}

pub fn changed(store: &Store, config: &MapConfig, root: &Path, base: Option<&str>) -> Result<Changed> {
    if let Some(b) = base {
        if b.starts_with('-') || b.is_empty() {
            return Err(Error::Config(format!("base: {b:?} is not a git ref")));
        }
    }
    let mut args = vec!["diff", "--unified=0", "--no-color"];
    if let Some(b) = base {
        args.push(b);
    }
    let diff = git(root, &args)?;
    let untracked = git(root, &["ls-files", "--others", "--exclude-standard"])?;
    let mut ranges = parse_unified0(&diff);
    for p in untracked.lines().filter(|l| !l.is_empty()) {
        ranges.push((p.to_string(), 1, u32::MAX));
    }
    let base_name = base.unwrap_or("HEAD").to_string();

    let mut symbols: Vec<ChangedSymbol> = Vec::new();
    let mut files_without: BTreeSet<String> = BTreeSet::new();
    let mut seen: BTreeSet<i64> = BTreeSet::new();
    let mut truncated = false;
    let mut stmt = store.conn().prepare(
        "SELECT s.id, s.name, s.kind, s.signature, s.line_start, s.line_end
         FROM symbols s JOIN files f ON f.id = s.file_id
         WHERE f.path = ?1 AND f.skipped_reason IS NULL AND s.line_start <= ?3 AND s.line_end >= ?2
         ORDER BY s.line_start",
    )?;
    let indexed: Vec<String> = {
        let mut q = store.conn().prepare("SELECT path FROM files WHERE skipped_reason IS NULL")?;
        q.query_map([], |r| r.get::<_, String>(0))?.collect::<std::result::Result<Vec<_>, _>>()?
    };
    for (path, first, last) in ranges {
        if !indexed.contains(&path) || config.is_excluded(&path) {
            continue;
        }
        let mut hit = false;
        for row in stmt.query_map(params![path, first, last], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, u32>(4)?, r.get::<_, u32>(5)?))
        })? {
            let (id, name, kind, signature, line_start, line_end) = row?;
            hit = true;
            if !seen.insert(id) {
                continue;
            }
            if symbols.len() == MAX_CHANGED {
                truncated = true;
                break;
            }
            let referenced_by: Vec<String> = referencing_files(store, &name)?
                .into_iter()
                .map(|(_, p)| p)
                .filter(|p| p != &path && !config.is_excluded(p))
                .collect();
            symbols.push(ChangedSymbol { symbol_id: id, path: path.clone(), name, kind, signature, line_start, line_end, referenced_by });
        }
        if !hit {
            files_without.insert(path);
        }
    }
    symbols.sort_by(|a, b| a.path.cmp(&b.path).then(a.line_start.cmp(&b.line_start)));
    Ok(Changed { base: base_name, symbols, files_without_symbols: files_without.into_iter().collect(), truncated })
}

pub fn render_changed(c: &Changed) -> String {
    let mut out = String::new();
    let mut files: BTreeSet<&str> = BTreeSet::new();
    let mut referrers: BTreeSet<&str> = BTreeSet::new();
    for s in &c.symbols {
        files.insert(&s.path);
        let refs = if s.referenced_by.is_empty() {
            String::new()
        } else {
            for r in &s.referenced_by {
                referrers.insert(r);
            }
            format!("  ← {}", s.referenced_by.join(", "))
        };
        out.push_str(&format!("{}::{} (lines {}-{}){refs}\n", s.path, s.name, s.line_start, s.line_end));
    }
    for f in &c.files_without_symbols {
        files.insert(f);
        out.push_str(&format!("{f} (no symbol touched)\n"));
    }
    let n = c.symbols.len();
    let more = if c.truncated { format!(" · more than {MAX_CHANGED}, showing the first") } else { String::new() };
    out.push_str(&format!(
        "# {n} symbol{} in {} file{} changed since {} · {} referencing file{}{more}\n",
        if n == 1 { "" } else { "s" },
        files.len(),
        if files.len() == 1 { "" } else { "s" },
        c.base,
        referrers.len(),
        if referrers.len() == 1 { "" } else { "s" },
    ));
    out
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p singularrag-core --lib changed:: 2>&1 | grep -E 'test result|panicked'`
Expected: 4 passed. If the untracked-file test fails because the indexer skipped `src/new.ts`, check the indexer's language detection expects `.ts` (it does for the fixture) and that `refresh` ran after the file was written.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(core): changed, the symbols a diff touches and who references them"
```

---

### Task 3: Engine wrappers and CLI subcommands

**Files:**
- Modify: `crates/singularrag-core/src/engine.rs` (types after `AnnotateResponse`; methods after `annotate`; tests)
- Modify: `crates/singularrag/src/main.rs` (`Path`, `Changed` subcommands)
- Modify: `crates/singularrag/tests/cli.rs`

**Interfaces:**
- Consumes: Task 1 and Task 2; `map::header`, `record_retrieval(tool, query, focus_files, budget, limit_n, index_version, git_head, stale, ranked, served)`, `map::footer` is not used.
- Produces: `TraceRequest { from_path, from_symbol, to_path, to_symbol }`, `TraceResponse { retrieval_id: i64, text: String, hops: usize, found: bool, stale_count, lock_timeout }`, `ChangedRequest { base: Option<String> }`, `ChangedResponse { retrieval_id, text, symbols: usize, stale_count, lock_timeout }`, `Engine::trace_path(&TraceRequest) -> Result<TraceResponse>`, `Engine::changed(&ChangedRequest) -> Result<ChangedResponse>`; CLI `singularrag path FROM TO` and `singularrag changed [--base REF]`.

- [ ] **Step 1: Write the failing tests**

`engine.rs` tests:

```rust
    #[test]
    fn trace_path_records_a_retrieval_with_the_path_symbols() {
        let dir = tempfile::tempdir().unwrap();
        write_ts_mini(dir.path());
        let mut e = Engine::open(dir.path(), "trace").unwrap();
        let r = e.trace_path(&TraceRequest { from_path: "src/cli/login.ts".into(), from_symbol: "login".into(), to_path: "src/auth/session.ts".into(), to_symbol: "createSession".into() }).unwrap();
        assert!(r.found && r.hops == 1);
        assert!(r.text.starts_with("# singularrag · index ") && r.text.contains("retrieval r_"), "{}", r.text);
        assert!(r.text.contains("src/cli/login.ts::login → src/auth/session.ts::createSession"), "{}", r.text);
        let (tool, query, limit): (String, String, i64) = e.store().conn().query_row("SELECT tool, query, limit_n FROM retrievals WHERE id = ?1", params![r.retrieval_id], |x| Ok((x.get(0)?, x.get(1)?, x.get(2)?))).unwrap();
        assert_eq!((tool.as_str(), query.as_str(), limit), ("trace_path", "src/cli/login.ts::login -> src/auth/session.ts::createSession", 6));
        let served: i64 = e.store().conn().query_row("SELECT count(*) FROM retrieval_items WHERE retrieval_id = ?1 AND served = 1", params![r.retrieval_id], |x| x.get(0)).unwrap();
        assert_eq!(served, 2);
        // Exclude src/cli/ so log.ts is unreachable (see the trace.rs test); the engine
        // re-reads map.toml on its next refresh.
        std::fs::create_dir_all(dir.path().join(".singularrag")).unwrap();
        std::fs::write(dir.path().join(".singularrag/map.toml"), "[[exclude]]\npath = \"src/cli/\"\n").unwrap();
        let none = e.trace_path(&TraceRequest { from_path: "src/util/log.ts".into(), from_symbol: "log".into(), to_path: "src/auth/session.ts".into(), to_symbol: "createSession".into() }).unwrap();
        assert!(!none.found && none.text.contains("# no path within 6 hops"), "{}", none.text);
    }

    #[test]
    fn changed_records_a_retrieval_and_errors_outside_git() {
        let dir = tempfile::tempdir().unwrap();
        write_ts_mini(dir.path());
        let mut e = Engine::open(dir.path(), "changed").unwrap();
        let err = e.changed(&ChangedRequest { base: None }).unwrap_err().to_string();
        assert!(err.contains("not a git checkout"), "{err}");
        // make it a repo with everything committed, then touch one symbol
        let git = |args: &[&str]| assert!(std::process::Command::new("git").arg("-C").arg(dir.path()).args(["-c", "user.email=t@t", "-c", "user.name=t"]).args(args).status().unwrap().success());
        git(&["init", "-q"]);
        git(&["add", "."]);
        git(&["commit", "-qm", "base"]);
        let p = dir.path().join("src/cli/login.ts");
        std::fs::write(&p, std::fs::read_to_string(&p).unwrap() + "\n// tail\n").unwrap();
        let r = e.changed(&ChangedRequest { base: None }).unwrap();
        assert!(r.text.starts_with("# singularrag · index "), "{}", r.text);
        assert!(r.text.contains("changed since HEAD"), "{}", r.text);
        let tool: String = e.store().conn().query_row("SELECT tool FROM retrievals WHERE id = ?1", params![r.retrieval_id], |x| x.get(0)).unwrap();
        assert_eq!(tool, "changed");
    }
```

(A trailing comment after the last symbol may or may not fall inside a symbol's range; the test asserts only the header, footer and provenance row. `e.store()` exists.)

`tests/cli.rs` (the `fixture()` helper and `Command::cargo_bin("singularrag")` pattern exist):

```rust
#[test]
fn path_prints_the_chain_and_changed_needs_git() {
    let dir = fixture();
    Command::cargo_bin("singularrag").unwrap().args(["--repo", dir.path().to_str().unwrap(), "path", "src/cli/login.ts::login", "src/auth/session.ts::createSession"])
        .assert().success()
        .stdout(predicate::str::contains("# 1 hop\n"));
    Command::cargo_bin("singularrag").unwrap().args(["--repo", dir.path().to_str().unwrap(), "path", "nope.ts::x", "src/auth/session.ts::createSession"])
        .assert().failure()
        .stderr(predicate::str::contains("from: nope.ts::x is not in the index"));
    Command::cargo_bin("singularrag").unwrap().args(["--repo", dir.path().to_str().unwrap(), "changed"])
        .assert().failure()
        .stderr(predicate::str::contains("not a git checkout"));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p singularrag-core --lib engine::tests 2>&1 | grep -E '^error|test result' | head -3`
Expected: compile errors.

- [ ] **Step 3: Implement**

`engine.rs`, after `AnnotateResponse`:

```rust
#[derive(Debug, Clone)]
pub struct TraceRequest {
    pub from_path: String,
    pub from_symbol: String,
    pub to_path: String,
    pub to_symbol: String,
}

#[derive(Debug, Clone)]
pub struct TraceResponse {
    pub retrieval_id: i64,
    pub text: String,
    pub hops: usize,
    pub found: bool,
    pub stale_count: usize,
    pub lock_timeout: bool,
}

#[derive(Debug, Clone)]
pub struct ChangedRequest {
    /// A git ref; `None` is the working tree against HEAD.
    pub base: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ChangedResponse {
    pub retrieval_id: i64,
    pub text: String,
    pub symbols: usize,
    pub stale_count: usize,
    pub lock_timeout: bool,
}
```

In `impl Engine`, after `annotate`:

```rust
    fn provenance_symbol(symbol_id: i64, path: &str, name: &str, kind: &str, line: u32, signature: &str, i: usize, seed: &str) -> ScoredSymbol {
        ScoredSymbol {
            symbol_id,
            file_id: 0,
            path: path.to_string(),
            name: name.to_string(),
            kind: kind.to_string(),
            line_start: line,
            line_end: line,
            signature: signature.to_string(),
            score: 1.0 / (i as f64 + 1.0),
            reasons: crate::rank::Reasons {
                seeds: vec![seed.to_string()],
                ..Default::default()
            },
        }
    }

    pub fn trace_path(&mut self, req: &TraceRequest) -> Result<TraceResponse> {
        let stats = self.refresh(self.refresh_budget)?;
        let trace = crate::trace::trace_path(&self.store, &self.config, &req.from_path, &req.from_symbol, &req.to_path, &req.to_symbol)?;
        let query = format!("{}::{} -> {}::{}", req.from_path, req.from_symbol, req.to_path, req.to_symbol);
        let (body, ranked) = match &trace {
            Some(t) => {
                let syms = crate::trace::path_symbols(&self.store, t)?;
                let ranked: Vec<ScoredSymbol> = syms
                    .iter()
                    .enumerate()
                    .map(|(i, (id, path, name, line, kind, sig))| Self::provenance_symbol(*id, path, name, kind, *line, sig, i, &format!("trace:{query}")))
                    .collect();
                (crate::trace::render_trace(t), ranked)
            }
            None => (format!("# no path within {} hops\n", crate::trace::MAX_PATH_DEPTH), Vec::new()),
        };
        let (version, head) = self.index_meta()?;
        let served = ranked.len();
        let retrieval_id = self.record_retrieval("trace_path", Some(&query), &[], None, Some(crate::trace::MAX_PATH_DEPTH), &version, head.as_deref(), stats.remaining, &ranked, served)?;
        Ok(TraceResponse {
            retrieval_id,
            text: format!("{}\n{body}", map::header(&version, head.as_deref(), stats.remaining, retrieval_id)),
            hops: trace.as_ref().map(|t| t.hops.len()).unwrap_or(0),
            found: trace.is_some(),
            stale_count: stats.remaining,
            lock_timeout: stats.lock_timeout,
        })
    }

    pub fn changed(&mut self, req: &ChangedRequest) -> Result<ChangedResponse> {
        let stats = self.refresh(self.refresh_budget)?;
        let c = crate::changed::changed(&self.store, &self.config, &self.root, req.base.as_deref())?;
        let ranked: Vec<ScoredSymbol> = c
            .symbols
            .iter()
            .enumerate()
            .map(|(i, s)| Self::provenance_symbol(s.symbol_id, &s.path, &s.name, &s.kind, s.line_start, &s.signature, i, &format!("changed:{}", c.base)))
            .collect();
        let (version, head) = self.index_meta()?;
        let served = ranked.len();
        let retrieval_id = self.record_retrieval("changed", Some(&c.base), &[], None, Some(crate::changed::MAX_CHANGED), &version, head.as_deref(), stats.remaining, &ranked, served)?;
        Ok(ChangedResponse {
            retrieval_id,
            text: format!("{}\n{}", map::header(&version, head.as_deref(), stats.remaining, retrieval_id), crate::changed::render_changed(&c)),
            symbols: c.symbols.len(),
            stale_count: stats.remaining,
            lock_timeout: stats.lock_timeout,
        })
    }
```

(`record_retrieval` records `CUT_RECORDED` candidates below `served`; with `served == ranked.len()` there are none. If it asserts `ranked.len() >= served` or reads `limit_n` as `usize`, keep the types it expects.)

`main.rs`: add to `Cmd`:

```rust
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
```

and handlers:

```rust
        Cmd::Path { from, to } => {
            let split = |s: &str| -> anyhow::Result<(String, String)> {
                s.split_once("::")
                    .map(|(p, n)| (p.to_string(), n.to_string()))
                    .ok_or_else(|| anyhow::anyhow!("expected path::symbol, got {s}"))
            };
            let (from_path, from_symbol) = split(&from)?;
            let (to_path, to_symbol) = split(&to)?;
            let r = engine.trace_path(&TraceRequest { from_path, from_symbol, to_path, to_symbol })?;
            print!("{}", r.text);
        }
        Cmd::Changed { base } => {
            let r = engine.changed(&ChangedRequest { base })?;
            print!("{}", r.text);
        }
```

(import `TraceRequest, ChangedRequest` from `singularrag_core::engine`; the `Path` variant name shadows nothing since `std::path::Path` is not imported in `main.rs`; if it is, name the variant `TracePath` with `#[command(name = "path")]`.)

- [ ] **Step 4: Run the tests**

Run: `cargo test --workspace 2>&1 | grep -E 'test result|FAILED|panicked'`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: trace_path and changed through the engine and the CLI"
```

---

### Task 4: MCP tools, actor jobs, descriptions, docs

**Files:**
- Modify: `crates/singularrag/src/actor.rs`, `crates/singularrag/src/mcp/server.rs`, `crates/singularrag/tests/mcp.rs`
- Modify: `docs/superpowers/specs/2026-09-19-singularrag-design.md` §9, §11; `README.md` "What the agent sees"

**Interfaces:**
- Consumes: Task 3's requests/responses.
- Produces: `Job::Trace(TraceRequest, oneshot::Sender<Reply<TraceResponse>>)`, `Job::Changed(ChangedRequest, oneshot::Sender<Reply<ChangedResponse>>)`, `EngineHandle::trace_path`, `EngineHandle::changed`; `TRACE_PATH_DESCRIPTION`, `CHANGED_DESCRIPTION`; tools `trace_path {from, to}` and `changed {base?}`.

Descriptions, byte for byte:

- `TRACE_PATH_DESCRIPTION`: `How two symbols connect: the shortest chain of references between `from` and `to`, each `path::name`, up to 6 hops, with the symbol each hop goes through. Use it for trace questions before reading files.`
- `CHANGED_DESCRIPTION`: `What a change touches: the symbols whose lines a diff modifies and the files that reference each. `base` is a git ref; omitted means the working tree against HEAD. Use it before editing to see the blast radius and after editing to check it.`

New `INSTRUCTIONS`, byte for byte:

`singularrag gives you a ranked map of this repository. Call repo_map first with your task as the query and answer from it; read only to confirm a detail the map does not show. Use find_symbol to locate a name, trace_path to see how two symbols connect, and changed to see what a diff touches and who references it. When you learn something about a file that its signatures do not say, record it with annotate so the next session starts from it. Only annotate writes, and only a note into .singularrag/map.toml. A STALE header means files changed since indexing; the index catches up in the background.`

- [ ] **Step 1: Write the failing tests**

`server.rs` unit test: expect `vec!["annotate", "changed", "find_symbol", "repo_map", "trace_path"]` and the two descriptions equal to the constants; `trace_path`'s schema requires `from` and `to`; `changed`'s has `base` optional.

`tests/mcp.rs`: add the two constants (copies), expect five tools in `lists_exactly_the_three_tools_with_spec_descriptions` (rename to `…five_tools…`), the new instructions prefix still `"singularrag gives you a ranked map"`, and:

```rust
#[tokio::test]
async fn trace_path_and_changed_answer_over_the_binary() {
    let dir = tempfile::tempdir().unwrap();
    write_ts_mini(dir.path());
    let client = connect(dir.path(), &[]).await;
    let r = client.call_tool(CallToolRequestParams::new("trace_path").with_arguments(object!({ "from": "src/cli/login.ts::login", "to": "src/auth/session.ts::createSession" }))).await.unwrap();
    assert_ne!(r.is_error, Some(true), "{}", text_of(&r));
    assert!(text_of(&r).contains("# 1 hop"), "{}", text_of(&r));
    let bad = client.call_tool(CallToolRequestParams::new("trace_path").with_arguments(object!({ "from": "login", "to": "src/auth/session.ts::createSession" }))).await.unwrap();
    assert_eq!(bad.is_error, Some(true));
    assert!(text_of(&bad).contains("path::name"), "{}", text_of(&bad));
    let c = client.call_tool(CallToolRequestParams::new("changed").with_arguments(object!({}))).await.unwrap();
    assert_eq!(c.is_error, Some(true), "the fixture is not a git checkout");
    assert!(text_of(&c).contains("not a git checkout"), "{}", text_of(&c));
    client.cancel().await.unwrap();
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p singularrag 2>&1 | grep -E '^error|test result' | head -5`

- [ ] **Step 3: Implement**

`actor.rs`: import the four types; variants `Trace(TraceRequest, oneshot::Sender<Reply<TraceResponse>>)` and `Changed(ChangedRequest, oneshot::Sender<Reply<ChangedResponse>>)`; handle methods `trace_path`/`changed` mirroring `annotate`; `handle` arms calling `e.trace_path(&req)` / `e.changed(&req)` and `self.note(r.stale_count, r.lock_timeout)`.

`server.rs`:

```rust
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct TraceArgs {
    /// `path::name` of the starting symbol.
    pub from: String,
    /// `path::name` of the target symbol.
    pub to: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ChangedArgs {
    /// A git ref to diff against; omit for the working tree against HEAD.
    pub base: Option<String>,
}

fn split_symbol(s: &str) -> Result<(String, String), String> {
    s.split_once("::")
        .filter(|(p, n)| !p.is_empty() && !n.is_empty())
        .map(|(p, n)| (p.to_string(), n.to_string()))
        .ok_or_else(|| format!("expected path::name, got {s:?}"))
}
```

tools:

```rust
    #[tool(name = "trace_path", description = "<TRACE_PATH_DESCRIPTION>")]
    async fn trace_path(&self, Parameters(args): Parameters<TraceArgs>) -> Result<CallToolResult, ErrorData> {
        let req = match (split_symbol(&args.from), split_symbol(&args.to)) {
            (Ok((from_path, from_symbol)), Ok((to_path, to_symbol))) => TraceRequest { from_path, from_symbol, to_path, to_symbol },
            (Err(e), _) | (_, Err(e)) => return text_result(Err(e)),
        };
        text_result(self.handle.trace_path(req).await.map(|r| r.text))
    }

    #[tool(name = "changed", description = "<CHANGED_DESCRIPTION>")]
    async fn changed(&self, Parameters(args): Parameters<ChangedArgs>) -> Result<CallToolResult, ErrorData> {
        text_result(self.handle.changed(ChangedRequest { base: args.base.filter(|b| !b.trim().is_empty()) }).await.map(|r| r.text))
    }
```

(with the literal descriptions pasted in place of the placeholders). Add the two `pub const` descriptions with `#[allow(dead_code)]`, replace `INSTRUCTIONS`.

Docs: parent spec §9: after the annotate sentence, "Two graph tools (2026-09-21, `docs/superpowers/specs/2026-09-21-singularrag-workflow-design.md` §4 and §5): `trace_path` returns the shortest reference chain between two `path::name` symbols, up to 6 hops; `changed` returns the symbols a git diff touches and the files that reference each. Five tools in all; `init` and `hook` are CLI subcommands, not tools." §11 controls: the "no exec" wording becomes "no agent-exposed execution; `changed` runs `git diff` and `git ls-files` read-only with fixed arguments and a validated ref". README "What the agent sees": after the annotate paragraph, "`trace_path` answers how two symbols connect; `changed` lists what your uncommitted (or branch) changes touch and who references each symbol. `singularrag init` installs a Claude Code hook that refuses the first file read of a session until the map has been consulted, once per session." and fix "Three tools" to "Five tools".

- [ ] **Step 4: Run the tests**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace 2>&1 | grep -E 'test result|FAILED'`

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(mcp): trace_path and changed tools; instructions name all five; specs and README amended"
```

---

### Task 5: The `hook` subcommand

**Files:**
- Create: `crates/singularrag/src/hook.rs`
- Modify: `crates/singularrag/src/main.rs` (`mod hook;`, `Hook { event }` subcommand handled before `Engine::open`)
- Modify: `crates/singularrag/tests/cli.rs`

**Interfaces:**
- Consumes: `singularrag_core::store::Store::open_read_only(&path)`; hook stdin JSON `{ "session_id", "cwd", "tool_name", "tool_input": { "file_path" } }`.
- Produces: `pub fn run(event: HookEvent, repo: Option<PathBuf>, input: &str) -> String` (the JSON to print) and `pub enum HookEvent { Read, Map }`; constants `DENY_JSON`, `ALLOW_JSON`, `pub fn marker_dir(session_id: &str) -> PathBuf`.

- [ ] **Step 1: Write the failing tests**

`hook.rs` unit tests:

```rust
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
        assert_eq!(run(HookEvent::Read, None, &input(&session, dir.path(), &file)), DENY_JSON);
        assert_eq!(run(HookEvent::Read, None, &input(&session, dir.path(), &file)), ALLOW_JSON, "second read passes");
        let s2 = format!("{session}-b");
        let _ = std::fs::remove_dir_all(marker_dir(&s2));
        assert_eq!(run(HookEvent::Map, None, &input(&s2, dir.path(), "")), ALLOW_JSON);
        assert_eq!(run(HookEvent::Read, None, &input(&s2, dir.path(), &file)), ALLOW_JSON, "a map call lifts the gate");
        let _ = std::fs::remove_dir_all(marker_dir(&session));
        let _ = std::fs::remove_dir_all(marker_dir(&s2));
    }

    #[test]
    fn allows_outside_the_repo_unindexed_files_and_garbage() {
        let dir = indexed_repo();
        let session = format!("test-{}-c", std::process::id());
        let _ = std::fs::remove_dir_all(marker_dir(&session));
        assert_eq!(run(HookEvent::Read, None, &input(&session, dir.path(), "/etc/hosts")), ALLOW_JSON);
        assert_eq!(run(HookEvent::Read, None, &input(&session, dir.path(), &dir.path().join("README.md").display().to_string())), ALLOW_JSON, "not indexed");
        assert_eq!(run(HookEvent::Read, None, "not json"), ALLOW_JSON);
        assert_eq!(run(HookEvent::Read, None, &input(&session, dir.path(), "")), ALLOW_JSON);
        // still un-denied: an indexed file is refused now
        assert_eq!(run(HookEvent::Read, None, &input(&session, dir.path(), &dir.path().join("src/cli/login.ts").display().to_string())), DENY_JSON);
        let _ = std::fs::remove_dir_all(marker_dir(&session));
    }
}
```

`tests/cli.rs`:

```rust
#[test]
fn hook_read_reads_stdin_and_prints_a_decision() {
    use std::io::Write;
    let dir = fixture();
    Command::cargo_bin("singularrag").unwrap().args(["--repo", dir.path().to_str().unwrap(), "index"]).assert().success();
    let session = format!("cli-{}", std::process::id());
    let input = format!(r#"{{"session_id":"{session}","cwd":"{}","tool_name":"Read","tool_input":{{"file_path":"{}"}}}}"#, dir.path().display(), dir.path().join("src/auth/session.ts").display());
    let mut cmd = Command::cargo_bin("singularrag").unwrap();
    cmd.args(["--repo", dir.path().to_str().unwrap(), "hook", "read"]).write_stdin(input.clone());
    cmd.assert().success().stdout(predicate::str::contains("\"permissionDecision\":\"deny\""));
    let mut again = Command::cargo_bin("singularrag").unwrap();
    again.args(["--repo", dir.path().to_str().unwrap(), "hook", "read"]).write_stdin(input);
    again.assert().success().stdout(predicate::str::contains("\"permissionDecision\":\"allow\""));
    let _ = std::fs::remove_dir_all(std::env::temp_dir().join("singularrag-hook").join(&session));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p singularrag --lib hook:: 2>&1 | grep -E '^error|test result' | head -3`

- [ ] **Step 3: Implement**

```rust
//! `singularrag hook read|map`: the query-first gate (workflow spec §2). Reads the
//! Claude Code hook JSON on stdin, prints a decision, exits 0 whatever happens.

use std::path::{Path, PathBuf};

use serde_json::Value;

pub const DENY_JSON: &str = r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"singularrag: this repository has a map. Call repo_map with your task first; it lists the relevant symbols and who references them. Then read what it points at."}}"#;
pub const ALLOW_JSON: &str = r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow"}}"#;

#[derive(Debug, Clone, Copy, PartialEq, clap::ValueEnum)]
pub enum HookEvent {
    /// PreToolUse on Read
    Read,
    /// PreToolUse on mcp__singularrag__repo_map
    Map,
}

pub fn marker_dir(session_id: &str) -> PathBuf {
    let safe: String = session_id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    std::env::temp_dir().join("singularrag-hook").join(if safe.is_empty() { "unknown".to_string() } else { safe })
}

fn touch(dir: &Path, name: &str) {
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(dir.join(name), b"");
}

/// The decision JSON. Never errors: anything unexpected is an allow.
pub fn run(event: HookEvent, repo: Option<PathBuf>, input: &str) -> String {
    let Ok(v) = serde_json::from_str::<Value>(input) else { return ALLOW_JSON.to_string() };
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
            let Some(root) = root.and_then(|r| r.canonicalize().ok()) else { return ALLOW_JSON.to_string() };
            let file = v.pointer("/tool_input/file_path").and_then(Value::as_str).unwrap_or("");
            if file.is_empty() {
                return ALLOW_JSON.to_string();
            }
            let Ok(abs) = Path::new(file).canonicalize() else { return ALLOW_JSON.to_string() };
            let Ok(rel) = abs.strip_prefix(&root) else { return ALLOW_JSON.to_string() };
            let rel = rel.to_string_lossy().replace('\\', "/");
            let db = root.join(singularrag_core::engine::DB_FILE);
            let Ok(store) = singularrag_core::store::Store::open_read_only(&db) else { return ALLOW_JSON.to_string() };
            let indexed: bool = store
                .conn()
                .query_row("SELECT count(*) FROM files WHERE path = ?1 AND skipped_reason IS NULL", [rel.as_str()], |r| r.get::<_, i64>(0))
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
```

`main.rs`: `mod hook;`; variant

```rust
    /// Claude Code PreToolUse hook (installed by `init`): reads the hook JSON on stdin
    Hook {
        #[arg(value_enum)]
        event: hook::HookEvent,
    },
```

handled before `Engine::open` (next to `Mcp`/`Serve`):

```rust
    if let Cmd::Hook { event } = cli.cmd {
        let mut input = String::new();
        let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut input);
        println!("{}", hook::run(event, cli.repo.clone(), &input));
        return Ok(());
    }
```

(`cli.repo` is moved into `root` earlier; compute `root` after this branch or clone before. `clap` is a dependency of the bin crate already; `ValueEnum` derive needs the `derive` feature, which `Parser` already uses.)

- [ ] **Step 4: Run the tests**

Run: `cargo test -p singularrag 2>&1 | grep -E 'test result|panicked'`

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(cli): the query-first hook, one denied Read per session until repo_map is called"
```

---

### Task 6: `singularrag init`

**Files:**
- Create: `crates/singularrag/src/init.rs`
- Modify: `crates/singularrag/src/main.rs` (`mod init;`, `Init { project, host }` handled before `Engine::open`)
- Modify: `crates/singularrag/tests/cli.rs`

**Interfaces:**
- Produces: `pub enum Host { Claude, Codex, Copilot, All }`; `pub fn run(root: &Path, project: bool, host: Host, bin: &Path) -> anyhow::Result<String>` (returns the report text it prints); `pub fn merge_hooks(existing: Value, bin: &Path) -> Value`; `pub fn merge_mcp(existing: Value) -> Value`; `pub const AGENTS_SNIPPET: &str`.

Hook entries written (with `<bin>` the absolute binary path):

```json
{"matcher":"Read","hooks":[{"type":"command","command":"<bin> hook read"}]}
{"matcher":"mcp__singularrag__repo_map","hooks":[{"type":"command","command":"<bin> hook map"}]}
```

`AGENTS_SNIPPET`: `This repository has a singularrag map. Call repo_map with your task before reading files; use find_symbol for a name, trace_path for how two symbols connect, and changed for what a diff touches.`

- [ ] **Step 1: Write the failing tests**

`init.rs` unit tests:

```rust
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
        assert_eq!(merge_hooks(json!({}), bin)["hooks"]["PreToolUse"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn merge_mcp_adds_singularrag_and_keeps_others() {
        let existing = json!({ "mcpServers": { "other": { "command": "x" } } });
        let m = merge_mcp(existing);
        assert_eq!(m["mcpServers"]["other"]["command"], "x");
        assert_eq!(m["mcpServers"]["singularrag"], json!({ "command": "singularrag", "args": ["mcp"] }));
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
        assert!(report.contains(".claude/settings.local.json") && report.contains(".mcp.json") && report.contains("AGENTS.md"), "{report}");
        let a = std::fs::read_to_string(&settings).unwrap();
        run(dir.path(), false, Host::All, bin).unwrap();
        assert_eq!(std::fs::read_to_string(&settings).unwrap(), a, "byte-identical on the second run");
        run(dir.path(), true, Host::Claude, bin).unwrap();
        assert!(dir.path().join(".claude/settings.json").exists());
        let codex_only = run(dir.path(), false, Host::Codex, bin).unwrap();
        assert!(codex_only.contains("[mcp_servers.singularrag]") && codex_only.contains(AGENTS_SNIPPET));
    }
}
```

`tests/cli.rs`:

```rust
#[test]
fn init_writes_the_claude_files() {
    let dir = fixture();
    Command::cargo_bin("singularrag").unwrap().args(["--repo", dir.path().to_str().unwrap(), "init", "--host", "claude"])
        .assert().success()
        .stdout(predicate::str::contains("wrote .claude/settings.local.json").and(predicate::str::contains("wrote .mcp.json")));
    let s = std::fs::read_to_string(dir.path().join(".claude/settings.local.json")).unwrap();
    assert!(s.contains("hook read") && s.contains("mcp__singularrag__repo_map"), "{s}");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p singularrag --lib init:: 2>&1 | grep -E '^error|test result' | head -3`

- [ ] **Step 3: Implement**

```rust
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

pub const AGENTS_SNIPPET: &str = "This repository has a singularrag map. Call repo_map with your task before reading files; use find_symbol for a name, trace_path for how two symbols connect, and changed for what a diff touches.";

fn hook_entry(matcher: &str, command: String) -> Value {
    json!({ "matcher": matcher, "hooks": [ { "type": "command", "command": command } ] })
}

fn has_ours(entries: &[Value], matcher: &str) -> bool {
    entries.iter().any(|e| {
        e["matcher"] == matcher
            && e["hooks"].as_array().is_some_and(|hs| hs.iter().any(|h| h["command"].as_str().is_some_and(|c| c.contains("singularrag hook"))))
    })
}

pub fn merge_hooks(mut existing: Value, bin: &Path) -> Value {
    if !existing.is_object() {
        existing = json!({});
    }
    let bin = bin.display().to_string();
    let hooks = existing.as_object_mut().unwrap().entry("hooks").or_insert_with(|| json!({}));
    if !hooks.is_object() {
        *hooks = json!({});
    }
    let pre = hooks.as_object_mut().unwrap().entry("PreToolUse").or_insert_with(|| json!([]));
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

pub fn merge_mcp(mut existing: Value) -> Value {
    if !existing.is_object() {
        existing = json!({});
    }
    let servers = existing.as_object_mut().unwrap().entry("mcpServers").or_insert_with(|| json!({}));
    if !servers.is_object() {
        *servers = json!({});
    }
    servers.as_object_mut().unwrap().entry("singularrag").or_insert_with(|| json!({ "command": "singularrag", "args": ["mcp"] }));
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

pub fn run(root: &Path, project: bool, host: Host, bin: &Path) -> anyhow::Result<String> {
    let mut out = String::new();
    if matches!(host, Host::Claude | Host::All) {
        let rel = if project { ".claude/settings.json" } else { ".claude/settings.local.json" };
        let settings = root.join(rel);
        let merged = merge_hooks(read_json(&settings)?, bin);
        out.push_str(&format!("{} {rel}\n", if write_json(&settings, &merged)? { "wrote" } else { "unchanged" }));
        let mcp = root.join(".mcp.json");
        let merged = merge_mcp(read_json(&mcp)?);
        out.push_str(&format!("{} .mcp.json\n", if write_json(&mcp, &merged)? { "wrote" } else { "unchanged" }));
    }
    if matches!(host, Host::Codex | Host::All) {
        out.push_str("Codex CLI, in ~/.codex/config.toml:\n[mcp_servers.singularrag]\ncommand = \"singularrag\"\nargs = [\"mcp\"]\n\n");
    }
    if matches!(host, Host::Copilot | Host::All) {
        out.push_str("Copilot CLI, in ~/.copilot/mcp-config.json:\n{ \"mcpServers\": { \"singularrag\": { \"type\": \"stdio\", \"command\": \"singularrag\", \"args\": [\"mcp\"] } } }\n\n");
    }
    if matches!(host, Host::Codex | Host::Copilot | Host::All) {
        out.push_str(&format!("Add to AGENTS.md (Codex and Copilot have no hooks):\n{AGENTS_SNIPPET}\n"));
    }
    Ok(out)
}
```

`main.rs`: `mod init;`; variant

```rust
    /// Install the query-first hook and the MCP entry for this repo
    Init {
        /// Write .claude/settings.json (shared) instead of .claude/settings.local.json
        #[arg(long)]
        project: bool,
        #[arg(long, value_enum, default_value_t = init::Host::All)]
        host: init::Host,
    },
```

handled before `Engine::open`: `let bin = std::env::current_exe()?; print!("{}", init::run(&root, project, host, &bin)?); return Ok(());`.

- [ ] **Step 4: Run the tests**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test -p singularrag 2>&1 | grep -E 'test result|panicked'`

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(cli): singularrag init installs the hook and the MCP entry, idempotently"
```

---

### Task 7: The bench measures the hook

**Files:**
- Modify: `crates/singularrag-bench/src/config.rs` (`Condition.settings: Option<PathBuf>`, absolutised and existence-checked like `mcp_config`)
- Modify: `crates/singularrag-bench/src/session.rs` (`SessionSpec.settings: Option<PathBuf>`; `--settings <path>` appended after `--allowedTools`)
- Modify: `crates/singularrag-bench/src/stream.rs` (`permission_denial_ids: Vec<(String, String)>`, `tool_results: BTreeMap<String, String>`)
- Modify: `crates/singularrag-bench/src/score.rs` (`Record.hook_denials: u64`, `denied` excludes hook denials)
- Modify: `crates/singularrag-bench/src/run.rs` (write the condition's settings file with `<checkout>` and `<bin>` substituted into `<run>/<condition>/settings.json`; abort only on `!record.denied.is_empty()`)
- Modify: `crates/singularrag-bench/src/summary.rs` (`ConditionStats.hook_denials: u64` total, a `hook denials` column after `failed`)
- Create: `eval/conditions/singularrag-hook.json`; modify `eval/tier2.toml` (condition `singularrag+hook`)
- Modify: `docs/superpowers/specs/2026-09-20-singularrag-tier2-design.md` §3 (the `settings` field and the condition), §5 (`hook_denials`)

**Interfaces:**
- Consumes: Task 5's `hook` subcommand; the hook JSON shapes.
- Produces: `Parsed.permission_denial_ids`, `Parsed.tool_results`, `Record.hook_denials`, `SessionSpec.settings`.

`eval/conditions/singularrag-hook.json`:

```json
{
  "hooks": {
    "PreToolUse": [
      { "matcher": "Read", "hooks": [ { "type": "command", "command": "<bin> --repo <checkout> hook read" } ] },
      { "matcher": "mcp__singularrag__repo_map", "hooks": [ { "type": "command", "command": "<bin> --repo <checkout> hook map" } ] }
    ]
  }
}
```

`eval/tier2.toml` gains:

```toml
[[condition]]
name = "singularrag+hook"
mcp_config = "conditions/singularrag.json"
settings = "conditions/singularrag-hook.json"
warmup = ["singularrag", "index", "--repo", "<checkout>"]
```

- [ ] **Step 1: Write the failing tests**

`stream.rs`: add a test that parses a three-line stream: an assistant `tool_use` (`id: "tu1"`, name `Read`), a user `tool_result` for `tu1` with content `"singularrag: this repository has a map. …"`, and a result line with `permission_denials: [{"tool_name":"Read","tool_use_id":"tu1"}]`; assert `permission_denial_ids == vec![("Read","tu1")]` and `tool_results["tu1"]` contains `singularrag:`. Also a `tool_result` whose `content` is an array of `{type: text, text}` blocks is joined.

`score.rs`: a test building such a `Parsed` (via `parse_stream` on the same text) and asserting `record.hook_denials == 1`, `record.denied.is_empty()`, `record.failed == false`; and one where the denied tool is `mcp__singularrag__repo_map` giving `hook_denials == 0` and `denied == ["mcp__singularrag__repo_map"]`.

`session.rs`: `command_args` with `settings: Some("/x/settings.json")` contains `--settings` followed by the path, after `--allowedTools` and before `--setting-sources`; `None` omits it.

`config.rs`: a config with `settings = "conditions/hook.json"` loads with the absolute path and fails with "settings not found" when the file is missing.

`summary.rs`: a record with `hook_denials: 2` shows in the table row (`| 2 |` in the new column) and the header contains `hook denials`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p singularrag-bench 2>&1 | grep -E '^error|test result' | head -5`

- [ ] **Step 3: Implement**

`stream.rs`: in the `assistant` arm, keep recording tool calls; add a `(Some("user"), _)` arm:

```rust
            (Some("user"), _) => {
                let blocks = v.pointer("/message/content").and_then(Value::as_array);
                for b in blocks.into_iter().flatten() {
                    if b.get("type").and_then(Value::as_str) != Some("tool_result") {
                        continue;
                    }
                    let Some(id) = b.get("tool_use_id").and_then(Value::as_str) else { continue };
                    let text = match b.get("content") {
                        Some(Value::String(s)) => s.clone(),
                        Some(Value::Array(items)) => items.iter().filter_map(|i| i.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n"),
                        _ => String::new(),
                    };
                    p.tool_results.insert(id.to_string(), text.chars().take(400).collect());
                }
            }
```

and in the result arm push `(tool_name, tool_use_id)` pairs into `permission_denial_ids` (keep `permission_denials` as the deduplicated names for existing tests).

`score.rs`:

```rust
    let is_hook = |tool: &str, id: &str| tool == "Read" && parsed.tool_results.get(id).is_some_and(|t| t.contains("singularrag:"));
    let hook_denials = parsed.permission_denial_ids.iter().filter(|(t, id)| is_hook(t, id)).count() as u64;
    let mut denied: Vec<String> = Vec::new();
    for (t, id) in &parsed.permission_denial_ids {
        if !is_hook(t, id) && !denied.contains(t) {
            denied.push(t.clone());
        }
    }
```

with `#[serde(default)] pub hook_denials: u64` on `Record`. `run.rs`: replace the abort check with `if !record.denied.is_empty() { let reason = format!("tool {} denied by permission", record.denied[0]); … }`; write the settings file:

```rust
        let settings = match &c.settings {
            Some(src) => {
                let text = std::fs::read_to_string(src)?.replace("<checkout>", &checkout).replace("<bin>", &singularrag_bin());
                let dst = cdir.join("settings.json");
                std::fs::write(&dst, text)?;
                Some(dst)
            }
            None => None,
        };
```

and pass it into `spec_for` → `SessionSpec.settings` (the dry run passes the source path). `session.rs`: after the `--allowedTools` block, `if let Some(s) = &spec.settings { a.push("--settings".into()); a.push(s.display().to_string()); }`. `summary.rs`: `hook_denials: records.iter().map(|r| r.hook_denials).sum()` in `stats`, header `| condition | sessions | failed | hook denials | mean recall | …` and the row `| {} | {} | {} | {} | {:.2} | …`; fix the separator row and existing tests' expected strings (`| alone | 4 | 0 | 0 | 0.38 |`).

Tier-two spec §3: after the `reset` sentence, "`settings` is a Claude Code settings JSON (relative to the config file) passed as `--settings`; it is loaded even under `--setting-sources ""` (probed 2026-09-21). `<checkout>` and `<bin>` (the resolved `singularrag` binary) are substituted. The `singularrag+hook` condition uses `conditions/singularrag-hook.json`." §5: "`hook_denials`: Reads refused by the singularrag hook (a `Read` denial whose tool result contains `singularrag:`); counted, never an abort; `denied` lists the other denied tools."

- [ ] **Step 4: Run the tests**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace 2>&1 | grep -E 'test result|FAILED'`; then a dry run: write a scratch copy of `eval/tier2.toml` with absolute paths outside the repo if the committed one cannot be used from the worktree (see the plan-4 handoff), and `cargo run -q -p singularrag-bench -- run --config <scratch> --conditions singularrag+hook --questions L1 --repeats 1 --dry-run` must print `--settings`.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "bench: per-condition settings, hook denials counted not aborted, the singularrag+hook condition"
```

---

### Task 8: The rail labels the two tools

**Files:**
- Modify: `ui/src/components/RetrievalsRail.tsx` (line ~30: `{r.tool} · {r.query ?? "no query"}`), `ui/src/App.test.tsx` (or a new `RetrievalsRail.test.tsx` if the rail is testable standalone; check how `RetrievalsRail` is rendered in `App.test.tsx`)

**Interfaces:**
- Produces: `export const toolLabel = (tool: string) => ({ repo_map: "Map", find_symbol: "Find", trace_path: "Trace", changed: "Changed" } as Record<string, string>)[tool] ?? tool;` in `ui/src/lib/toolLabel.ts`, used by the rail.

- [ ] **Step 1: Write the failing test** — `ui/src/lib/toolLabel.test.ts`: the four known names map, an unknown name passes through.
- [ ] **Step 2: Run it** — `cd ui && bun test toolLabel` fails (module missing).
- [ ] **Step 3: Implement** — the helper and `{toolLabel(r.tool)} · {r.query ?? "no query"}` in the rail; any App test asserting the rail's raw `repo_map` text is updated to `Map`.
- [ ] **Step 4: Run** — `cd ui && bun run typecheck && bun test && bun run build`, then `cargo test --workspace` (embedded assets).
- [ ] **Step 5: Commit** — `git add -A && git commit -m "feat(ui): the rail labels trace_path and changed retrievals"`.

---

## Self-review

- Spec §2 → Task 5; §3 → Task 6; §4 → Tasks 1, 3, 4; §5 → Tasks 2, 3, 4; §6 → Task 4; §7 → Task 8; §8 → Task 7; §9 → Tasks 2, 5, 6 (git args fixed, marker files only, merge-only writes); §10 → Tasks 4 and 7; §11 → each task's tests.
- Names: `TraceRequest/TraceResponse/ChangedRequest/ChangedResponse` are identical in Tasks 3 and 4; `hook::HookEvent`/`init::Host` are `clap::ValueEnum`; `DENY_JSON`/`ALLOW_JSON` match the Global Constraints; the bench's `<bin>` substitution matches `singularrag_bin()`.
- Placeholders: Task 4's `<TRACE_PATH_DESCRIPTION>` / `<CHANGED_DESCRIPTION>` in the `#[tool]` attributes mean "paste the literal given above", since the macro needs a literal.
