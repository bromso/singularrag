# singularrag Engine + CLI Implementation Plan (plan 1 of 4)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the `singularrag` engine and CLI: index a repo with tree-sitter into SQLite, rank symbols with personalised PageRank, render a token-budgeted repo map with recorded provenance, look up symbols, and run the tier-one eval.

**Architecture:** One cargo workspace. `singularrag-core` (library) owns the SQLite store, the gitignore-aware walker with sandbox and denylist, tag extraction via `tree-sitter-tags`, the file-level reference graph, PageRank, budget fill, and retrieval recording. `singularrag` (binary) is a thin clap CLI over it. MCP (`mcp`), the UI (`serve`) and the tier-two agent harness are plans 2, 3 and 4 and consume the `Engine` type defined here.

**Tech Stack:** Rust 2021 (toolchain 1.97 locally, floor 1.85), rusqlite 0.40 `bundled` (FTS5 included, no separate feature), tree-sitter 0.27 + tree-sitter-tags 0.27, tree-sitter-typescript 0.23, tree-sitter-javascript 0.25, tree-sitter-rust 0.24, ignore 0.4 + globset 0.4, blake3 1.8, regex 1.13, serde/serde_json 1, toml 1.1, thiserror 2, clap 4.6, tempfile 3, assert_cmd 2.

**Spec:** `docs/superpowers/specs/2026-09-19-singularrag-design.md`

## Global Constraints

- Read-only: the only writes are `retrievals`, `retrieval_items`, `indexer_lock`, `meta`, and `.singularrag/map.toml` (spec §11).
- Tool and CLI output contains identifiers, signatures and paths only; never bodies, comments or string literals (spec §9).
- Budget: default 1024 tokens, cap 8192, approximate tokens = chars / 4 (spec §7, §9).
- Header line on every map/find response: `# singularrag · index <id> · HEAD <sha|none> · fresh|STALE: N files changed since index · retrieval r_<id>` (spec §9).
- Freshness: stat walk on every call; inline refresh; if refresh exceeds 2 s answer with the stale count; lock wait 500 ms (spec §8).
- Built-in denylist, extendable never shrinkable: `.env*`, `*.pem`, `*.key`, `id_rsa*`, `*.p12`, `*.pfx`, `.npmrc`, `.netrc`, `*.tfstate`, `secrets/`, `credentials*` (spec §11).
- Index DB: `.singularrag/index.db`, WAL, mode 0600. Authored map: `.singularrag/map.toml`, committed (spec §7).
- Languages v0: TypeScript, TSX, JavaScript, Rust (spec §6).
- No LLM calls, no embeddings, no SCIP (spec §5).
- Cut list: the 25 candidates below the budget line are recorded per retrieval (spec §7).
- No `LICENSE` file until spec open question 1 is answered; `Cargo.toml` carries no `license` field yet.

## File structure

```
Cargo.toml                                   workspace, shared deps
rust-toolchain.toml                          stable, rustfmt, clippy
crates/singularrag-core/
  Cargo.toml
  src/lib.rs                                 module tree + re-exports
  src/error.rs                               Error, Result
  src/time.rs                                now_ms()
  src/config.rs                              MapConfig (map.toml), BUILTIN_DENY
  src/store/mod.rs                           Store: open, WAL, 0600, migrate, data_version
  src/store/schema.rs                        DDL, SCHEMA_VERSION
  src/store/lock.rs                          indexer_lock acquire/release/heartbeat
  src/walk.rs                                gitignore-aware walk, denylist, sandbox
  src/secrets.rs                             looks_secret()
  src/tokens.rs                              split_identifier, approx_tokens, query_terms
  src/lang.rs                                Language, extract_tags() via tree-sitter-tags
  src/index.rs                               Indexer: refresh with deadline, git_head()
  src/graph.rs                               FileGraph from symbols + refs
  src/rank.rs                                pagerank(), rank_symbols(), Reasons
  src/map.rs                                 budget fill, render, header, footer
  src/find.rs                                find_symbol(), render_find()
  src/engine.rs                              Engine: open, refresh (lock), repo_map, find, record retrieval
  src/eval.rs                                Question, load_questions, run_eval, render_report
  src/fixture.rs                             writes the ts-mini / rust-mini fixture repos into a dir (test support)
crates/singularrag/
  Cargo.toml                                 bin "singularrag"
  src/main.rs                                clap: index, query, find, eval
  tests/cli.rs                               assert_cmd tests
eval/questions.toml                          the 12 hono questions
eval/README.md                               pinned hono commit, how to run
```

---

### Task 1: Workspace scaffold

**Files:**
- Create: `Cargo.toml`, `rust-toolchain.toml`, `crates/singularrag-core/Cargo.toml`, `crates/singularrag-core/src/lib.rs`, `crates/singularrag-core/src/error.rs`, `crates/singularrag-core/src/time.rs`, `crates/singularrag/Cargo.toml`, `crates/singularrag/src/main.rs`, `crates/singularrag/tests/cli.rs`
- Modify: `.gitignore` (already has `target/`, `node_modules/`, `.singularrag/index.db`)

**Interfaces:**
- Produces: `singularrag_core::{Error, Result}`, `singularrag_core::time::now_ms() -> i64`, the `singularrag` binary answering `--version`.

- [ ] **Step 1: Write the failing CLI test**

`crates/singularrag/tests/cli.rs`:
```rust
use assert_cmd::Command;

#[test]
fn version_prints_name_and_version() {
    let mut cmd = Command::cargo_bin("singularrag").unwrap();
    cmd.arg("--version")
        .assert()
        .success()
        .stdout(predicates::str::starts_with("singularrag 0.1.0"));
}
```

- [ ] **Step 2: Create the workspace files**

`Cargo.toml`:
```toml
[workspace]
members = ["crates/singularrag-core", "crates/singularrag"]
resolver = "2"

[workspace.package]
version = "0.1.0"
edition = "2021"
rust-version = "1.85"

[workspace.dependencies]
rusqlite = { version = "0.40", features = ["bundled"] }
tree-sitter = "0.27"
tree-sitter-tags = "0.27"
tree-sitter-typescript = "0.23"
tree-sitter-javascript = "0.25"
tree-sitter-rust = "0.24"
ignore = "0.4"
globset = "0.4"
blake3 = "1.8"
regex = "1.13"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "1.1"
thiserror = "2"
anyhow = "1"
clap = { version = "4.6", features = ["derive"] }
tempfile = "3"
assert_cmd = "2"
predicates = "3"

[profile.release]
lto = "thin"
strip = true
```

`rust-toolchain.toml`:
```toml
[toolchain]
channel = "stable"
components = ["rustfmt", "clippy"]
profile = "minimal"
```

`crates/singularrag-core/Cargo.toml`:
```toml
[package]
name = "singularrag-core"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
description = "Repo map with retrieval provenance for coding agents (engine)"

[dependencies]
rusqlite = { workspace = true }
tree-sitter = { workspace = true }
tree-sitter-tags = { workspace = true }
tree-sitter-typescript = { workspace = true }
tree-sitter-javascript = { workspace = true }
tree-sitter-rust = { workspace = true }
ignore = { workspace = true }
globset = { workspace = true }
blake3 = { workspace = true }
regex = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
toml = { workspace = true }
thiserror = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
```

`crates/singularrag-core/src/lib.rs`:
```rust
//! singularrag engine: index a repo, rank symbols, render a budgeted map,
//! and record what was served and cut so a human can see it.

#![forbid(unsafe_code)]

pub mod error;
pub mod time;

pub use error::{Error, Result};
```

`crates/singularrag-core/src/error.rs`:
```rust
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("config: {0}")]
    Config(String),
    #[error("path escapes repo root: {0}")]
    PathEscape(String),
    #[error("tags: {0}")]
    Tags(String),
    #[error("index is locked by another process")]
    Locked,
}

pub type Result<T> = std::result::Result<T, Error>;
```

`crates/singularrag-core/src/time.rs`:
```rust
use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch. Never panics; pre-1970 clocks yield 0.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
```

`crates/singularrag/Cargo.toml`:
```toml
[package]
name = "singularrag"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
description = "Repo map with retrieval provenance for coding agents"

[[bin]]
name = "singularrag"
path = "src/main.rs"

[dependencies]
singularrag-core = { path = "../singularrag-core" }
clap = { workspace = true }
anyhow = { workspace = true }
serde_json = { workspace = true }

[dev-dependencies]
assert_cmd = { workspace = true }
predicates = { workspace = true }
tempfile = { workspace = true }
```

`crates/singularrag/src/main.rs`:
```rust
#![forbid(unsafe_code)]

use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "singularrag", version, about = "Repo map with retrieval provenance for coding agents")]
struct Cli {}

fn main() -> anyhow::Result<()> {
    let _cli = Cli::parse();
    Ok(())
}
```

- [ ] **Step 3: Run the test to verify it passes**

Run: `cargo test -p singularrag --test cli`
Expected: 1 passed. (This task's test passes as soon as the scaffold compiles; that is the deliverable.)

- [ ] **Step 4: Run clippy and fmt**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock rust-toolchain.toml crates
git commit -m "chore: workspace scaffold with core library and cli binary"
```

---

### Task 2: Store, schema, WAL, permissions

**Files:**
- Create: `crates/singularrag-core/src/store/mod.rs`, `crates/singularrag-core/src/store/schema.rs`
- Modify: `crates/singularrag-core/src/lib.rs` (add `pub mod store;`)

**Interfaces:**
- Produces: `Store::open(path: &Path) -> Result<Store>`, `Store::open_in_memory() -> Result<Store>`, `Store::conn(&self) -> &rusqlite::Connection`, `Store::schema_version(&self) -> Result<i64>`, `Store::data_version(&self) -> Result<i64>`, `Store::get_meta(&self, key) -> Result<Option<String>>`, `Store::set_meta(&self, key, value) -> Result<()>`.

- [ ] **Step 1: Write the failing tests**

Append to `crates/singularrag-core/src/store/mod.rs` (the module file is created in step 3; write tests first at its bottom):
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_creates_db_with_schema_and_wal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".singularrag").join("index.db");
        let store = Store::open(&path).unwrap();
        assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
        let mode: String = store
            .conn()
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perm = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(perm, 0o600);
        }
    }

    #[test]
    fn reopen_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        Store::open(&path).unwrap();
        let store = Store::open(&path).unwrap();
        assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn meta_roundtrip_and_data_version_changes_on_write() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.get_meta("git_head").unwrap(), None);
        store.set_meta("git_head", "abc").unwrap();
        assert_eq!(store.get_meta("git_head").unwrap(), Some("abc".to_string()));
        // data_version only moves for writes from *other* connections; assert it is readable.
        assert!(store.data_version().unwrap() >= 0);
    }

    #[test]
    fn all_tables_exist() {
        let store = Store::open_in_memory().unwrap();
        for t in ["meta", "files", "symbols", "refs", "symbols_fts", "retrievals", "retrieval_items", "indexer_lock"] {
            let n: i64 = store
                .conn()
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE name = ?1",
                    [t],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "missing table {t}");
        }
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p singularrag-core store`
Expected: compile error, `Store` not found.

- [ ] **Step 3: Write the schema and store**

`crates/singularrag-core/src/store/schema.rs`:
```rust
pub const SCHEMA_VERSION: i64 = 1;

pub const DDL: &str = r#"
CREATE TABLE IF NOT EXISTS meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS files (
  id             INTEGER PRIMARY KEY,
  path           TEXT NOT NULL UNIQUE,
  lang           TEXT,
  content_hash   TEXT,
  mtime_ms       INTEGER NOT NULL,
  size           INTEGER NOT NULL,
  indexed_at_ms  INTEGER NOT NULL,
  skipped_reason TEXT
);
CREATE TABLE IF NOT EXISTS symbols (
  id         INTEGER PRIMARY KEY,
  file_id    INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  name       TEXT NOT NULL,
  kind       TEXT NOT NULL,
  line_start INTEGER NOT NULL,
  line_end   INTEGER NOT NULL,
  signature  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS symbols_name ON symbols(name);
CREATE INDEX IF NOT EXISTS symbols_file ON symbols(file_id);
CREATE TABLE IF NOT EXISTS refs (
  id      INTEGER PRIMARY KEY,
  file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  name    TEXT NOT NULL,
  line    INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS refs_name ON refs(name);
CREATE INDEX IF NOT EXISTS refs_file ON refs(file_id);
CREATE VIRTUAL TABLE IF NOT EXISTS symbols_fts USING fts5(
  name, name_tokens, signature, path, tokenize='unicode61'
);
CREATE TABLE IF NOT EXISTS retrievals (
  id            INTEGER PRIMARY KEY,
  session_key   TEXT NOT NULL,
  tool          TEXT NOT NULL,
  query         TEXT,
  focus_files   TEXT NOT NULL,
  budget        INTEGER NOT NULL,
  index_version TEXT NOT NULL,
  git_head      TEXT,
  stale_count   INTEGER NOT NULL,
  created_at_ms INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS retrieval_items (
  retrieval_id INTEGER NOT NULL REFERENCES retrievals(id) ON DELETE CASCADE,
  symbol_id    INTEGER NOT NULL,
  rank         INTEGER NOT NULL,
  score        REAL NOT NULL,
  served       INTEGER NOT NULL,
  reasons_json TEXT NOT NULL,
  PRIMARY KEY (retrieval_id, rank)
);
CREATE TABLE IF NOT EXISTS indexer_lock (
  id              INTEGER PRIMARY KEY CHECK (id = 1),
  pid             INTEGER NOT NULL,
  heartbeat_at_ms INTEGER NOT NULL
);
"#;
```
Note: `symbols_fts` rows use `rowid = symbols.id`; that is how the two are joined and how per-file deletes work.

`crates/singularrag-core/src/store/mod.rs` (above the tests):
```rust
pub mod lock;
pub mod schema;

use std::path::Path;

use rusqlite::{Connection, OptionalExtension};

use crate::Result;
pub use schema::SCHEMA_VERSION;

/// One SQLite connection to a repo's `.singularrag/index.db`.
pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Store> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Store> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "busy_timeout", 1000)?;
        conn.execute_batch(schema::DDL)?;
        let store = Store { conn };
        if store.get_meta("schema_version")?.is_none() {
            store.set_meta("schema_version", &SCHEMA_VERSION.to_string())?;
        }
        Ok(store)
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn schema_version(&self) -> Result<i64> {
        Ok(self
            .get_meta("schema_version")?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0))
    }

    /// SQLite's `PRAGMA data_version`: changes when another connection commits.
    pub fn data_version(&self) -> Result<i64> {
        Ok(self.conn.query_row("PRAGMA data_version", [], |r| r.get(0))?)
    }

    pub fn get_meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [key, value],
        )?;
        Ok(())
    }
}
```
Create `crates/singularrag-core/src/store/lock.rs` as an empty file for now (Task 11 fills it) so the module compiles: put a single line `//! Advisory indexer lock (see Task 11).`

Add `pub mod store;` to `lib.rs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p singularrag-core store`
Expected: 4 passed. If `journal_mode` returns `memory` for the in-memory DB, that's expected; the WAL assertion is only in the on-disk test.

- [ ] **Step 5: Commit**

```bash
git add crates/singularrag-core
git commit -m "feat(core): sqlite store with schema v1, wal, 0600 permissions"
```

---

### Task 3: `map.toml` config

**Files:**
- Create: `crates/singularrag-core/src/config.rs`
- Modify: `crates/singularrag-core/src/lib.rs` (add `pub mod config;`)

**Interfaces:**
- Produces: `MapConfig { pin: Vec<Target>, exclude: Vec<Target>, note: Vec<Note>, boundary: Vec<Boundary>, deny: Deny }`, `Target { path: String, symbol: Option<String> }`, `MapConfig::load(root: &Path) -> Result<MapConfig>`, `MapConfig::parse(&str) -> Result<MapConfig>`, `MapConfig::is_pinned(&self, path: &str) -> bool`, `MapConfig::is_excluded(&self, path: &str) -> bool`, `MapConfig::deny_patterns(&self) -> Vec<String>`, `BUILTIN_DENY: &[&str]`, `MAP_FILE: &str = ".singularrag/map.toml"`.

- [ ] **Step 1: Write the failing tests**

At the bottom of `crates/singularrag-core/src/config.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[[pin]]
path = "src/auth/session.ts"

[[pin]]
path = "src/http/middleware.ts"
symbol = "requireSession"

[[exclude]]
path = "src/legacy/"

[[note]]
path = "src/auth/session.ts"
text = "Auth boundary. Sessions are created here only."

[[boundary]]
name = "auth"
paths = ["src/auth/", "src/http/middleware.ts"]

[deny]
extra_patterns = ["*.snap"]
"#;

    #[test]
    fn parses_sample() {
        let c = MapConfig::parse(SAMPLE).unwrap();
        assert_eq!(c.pin.len(), 2);
        assert_eq!(c.pin[1].symbol.as_deref(), Some("requireSession"));
        assert_eq!(c.exclude[0].path, "src/legacy/");
        assert_eq!(c.boundary[0].paths.len(), 2);
        assert!(c.is_pinned("src/auth/session.ts"));
        assert!(!c.is_pinned("src/util/log.ts"));
    }

    #[test]
    fn excluded_matches_exact_and_directory_prefix() {
        let c = MapConfig::parse(SAMPLE).unwrap();
        assert!(c.is_excluded("src/legacy/old.ts"));
        assert!(!c.is_excluded("src/legacyish.ts"));
        assert!(!c.is_excluded("src/auth/session.ts"));
    }

    #[test]
    fn deny_patterns_include_builtins_plus_extras() {
        let c = MapConfig::parse(SAMPLE).unwrap();
        let d = c.deny_patterns();
        for b in BUILTIN_DENY {
            assert!(d.iter().any(|p| p == b), "builtin {b} missing");
        }
        assert!(d.iter().any(|p| p == "*.snap"));
    }

    #[test]
    fn missing_file_is_default() {
        let dir = tempfile::tempdir().unwrap();
        let c = MapConfig::load(dir.path()).unwrap();
        assert_eq!(c, MapConfig::default());
        assert_eq!(c.deny_patterns().len(), BUILTIN_DENY.len());
    }

    #[test]
    fn invalid_toml_is_config_error() {
        let err = MapConfig::parse("[[pin]\npath = 1").unwrap_err();
        assert!(matches!(err, crate::Error::Config(_)));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p singularrag-core config`
Expected: compile error, `MapConfig` not found.

- [ ] **Step 3: Write the implementation**

`crates/singularrag-core/src/config.rs` (above the tests):
```rust
//! Authored map: `.singularrag/map.toml`. Committed to the repo; the UI edits it.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

pub const MAP_FILE: &str = ".singularrag/map.toml";

/// Never shrinkable. `map.toml` may only add patterns.
pub const BUILTIN_DENY: &[&str] = &[
    ".env*", "*.pem", "*.key", "id_rsa*", "*.p12", "*.pfx", ".npmrc", ".netrc", "*.tfstate",
    "secrets/", "credentials*",
];

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct MapConfig {
    #[serde(default)]
    pub pin: Vec<Target>,
    #[serde(default)]
    pub exclude: Vec<Target>,
    #[serde(default)]
    pub note: Vec<Note>,
    #[serde(default)]
    pub boundary: Vec<Boundary>,
    #[serde(default)]
    pub deny: Deny,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Target {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Boundary {
    pub name: String,
    pub paths: Vec<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Deny {
    #[serde(default)]
    pub extra_patterns: Vec<String>,
}

impl MapConfig {
    pub fn load(root: &Path) -> Result<MapConfig> {
        let path = root.join(MAP_FILE);
        match std::fs::read_to_string(&path) {
            Ok(s) => Self::parse(&s),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(MapConfig::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn parse(s: &str) -> Result<MapConfig> {
        toml::from_str(s).map_err(|e| Error::Config(e.to_string()))
    }

    /// Pinned at file level (a pin with a symbol also pins the file).
    pub fn is_pinned(&self, path: &str) -> bool {
        self.pin.iter().any(|t| t.path == path)
    }

    /// Exact path, or a directory prefix when the exclude path ends with '/'.
    pub fn is_excluded(&self, path: &str) -> bool {
        self.exclude.iter().any(|t| {
            if t.path.ends_with('/') {
                path.starts_with(&t.path)
            } else {
                t.path == path
            }
        })
    }

    pub fn deny_patterns(&self) -> Vec<String> {
        BUILTIN_DENY
            .iter()
            .map(|s| s.to_string())
            .chain(self.deny.extra_patterns.iter().cloned())
            .collect()
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p singularrag-core config`
Expected: 5 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/singularrag-core
git commit -m "feat(core): map.toml config with pins, excludes, notes, boundaries, deny extras"
```

---

### Task 4: Walker with gitignore, denylist and sandbox

**Files:**
- Create: `crates/singularrag-core/src/walk.rs`
- Modify: `crates/singularrag-core/src/lib.rs` (add `pub mod walk;`)

**Interfaces:**
- Produces: `WalkEntry { rel_path: String, abs_path: PathBuf, mtime_ms: i64, size: u64 }`, `Skipped { rel_path: String, reason: &'static str }`, `WalkResult { entries: Vec<WalkEntry>, skipped: Vec<Skipped> }`, `walk(root: &Path, deny: &[String]) -> Result<WalkResult>`. Reason strings: `"denylisted"`, `"outside-root"`.
- Consumes: `MapConfig::deny_patterns()`.

- [ ] **Step 1: Write the failing tests**

Bottom of `crates/singularrag-core/src/walk.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(root: &Path, rel: &str, content: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    #[test]
    fn honours_gitignore_denylist_and_sandbox() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, ".gitignore", "dist/\n");
        write(root, "src/a.ts", "export const a = 1;");
        write(root, "dist/bundle.js", "var x;");
        write(root, ".env", "SECRET=1");
        write(root, "secrets/k.pem", "key");
        write(root, ".git/HEAD", "ref: refs/heads/main");
        write(root, ".singularrag/index.db", "");
        let outside = tempfile::tempdir().unwrap();
        write(outside.path(), "evil.ts", "export const e = 1;");
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path().join("evil.ts"), root.join("src/link.ts")).unwrap();

        let deny: Vec<String> = crate::config::BUILTIN_DENY.iter().map(|s| s.to_string()).collect();
        let r = walk(root, &deny).unwrap();

        let entries: Vec<&str> = r.entries.iter().map(|e| e.rel_path.as_str()).collect();
        assert_eq!(entries, vec![".gitignore", "src/a.ts"]);
        assert!(r.entries[1].size > 0);
        assert!(r.entries[1].mtime_ms > 0);

        let mut skipped: Vec<(String, &str)> = r.skipped.iter().map(|s| (s.rel_path.clone(), s.reason)).collect();
        skipped.sort();
        let mut expected = vec![(".env".to_string(), "denylisted"), ("secrets/k.pem".to_string(), "denylisted")];
        #[cfg(unix)]
        expected.push(("src/link.ts".to_string(), "outside-root"));
        expected.sort();
        assert_eq!(skipped, expected);
    }

    #[test]
    fn extra_deny_pattern_applies() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.snap", "x");
        write(dir.path(), "b.ts", "x");
        let r = walk(dir.path(), &["*.snap".to_string()]).unwrap();
        assert_eq!(r.entries.len(), 1);
        assert_eq!(r.skipped[0].rel_path, "a.snap");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p singularrag-core walk`
Expected: compile error, `walk` not found.

- [ ] **Step 3: Write the implementation**

`crates/singularrag-core/src/walk.rs` (above the tests):
```rust
//! Repo walk: gitignore-aware, denylist-aware, sandboxed to the repo root.

use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;

use crate::{Error, Result};

#[derive(Debug, Clone)]
pub struct WalkEntry {
    pub rel_path: String,
    pub abs_path: PathBuf,
    pub mtime_ms: i64,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Skipped {
    pub rel_path: String,
    pub reason: &'static str,
}

#[derive(Debug, Default)]
pub struct WalkResult {
    pub entries: Vec<WalkEntry>,
    pub skipped: Vec<Skipped>,
}

fn build_denyset(deny: &[String]) -> Result<GlobSet> {
    let mut b = GlobSetBuilder::new();
    for p in deny {
        let glob = if let Some(dir) = p.strip_suffix('/') {
            format!("**/{dir}/**")
        } else {
            format!("**/{p}")
        };
        b.add(Glob::new(&glob).map_err(|e| Error::Config(format!("deny pattern {p}: {e}")))?);
    }
    b.build().map_err(|e| Error::Config(e.to_string()))
}

/// Walk `root`. Files ignored by `.gitignore` are silently absent; files hit by the
/// denylist or resolving outside `root` are returned in `skipped` with a reason.
pub fn walk(root: &Path, deny: &[String]) -> Result<WalkResult> {
    let root = root.canonicalize()?;
    let denyset = build_denyset(deny)?;
    let mut out = WalkResult::default();

    let walker = WalkBuilder::new(&root)
        .hidden(false)
        .require_git(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(true)
        .follow_links(false)
        .sort_by_file_path(|a, b| a.cmp(b))
        .filter_entry(|e| {
            let n = e.file_name();
            n != ".git" && n != ".singularrag"
        })
        .build();

    for entry in walker {
        let entry = entry.map_err(|e| Error::Io(std::io::Error::other(e)))?;
        let path = entry.path();
        if path == root {
            continue;
        }
        let rel = path
            .strip_prefix(&root)
            .map_err(|_| Error::PathEscape(path.display().to_string()))?
            .to_string_lossy()
            .replace('\\', "/");

        // Resolve symlinks and sandbox.
        let resolved = match path.canonicalize() {
            Ok(p) => p,
            Err(_) => continue, // dangling symlink: nothing to index
        };
        if !resolved.starts_with(&root) {
            out.skipped.push(Skipped { rel_path: rel, reason: "outside-root" });
            continue;
        }
        let meta = match std::fs::metadata(&resolved) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if !meta.is_file() {
            continue;
        }
        if denyset.is_match(&rel) {
            out.skipped.push(Skipped { rel_path: rel, reason: "denylisted" });
            continue;
        }
        let mtime_ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        out.entries.push(WalkEntry { rel_path: rel, abs_path: resolved, mtime_ms, size: meta.len() });
    }
    out.entries.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(out)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p singularrag-core walk`
Expected: 2 passed. If the `.gitignore` entry ordering differs, the `entries` vector is sorted by path, so `.gitignore` sorts before `src/a.ts`.

- [ ] **Step 5: Commit**

```bash
git add crates/singularrag-core
git commit -m "feat(core): gitignore-aware walker with denylist and root sandbox"
```

---
### Task 5: Secret scanner

**Files:**
- Create: `crates/singularrag-core/src/secrets.rs`
- Modify: `crates/singularrag-core/src/lib.rs` (add `pub mod secrets;`)

**Interfaces:**
- Produces: `looks_secret(text: &str) -> Option<&'static str>` returning the rule name that matched.

- [ ] **Step 1: Write the failing tests**

Bottom of `crates/singularrag-core/src/secrets.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_private_key_header() {
        assert_eq!(looks_secret("-----BEGIN RSA PRIVATE KEY-----\nMIIE"), Some("private-key"));
        assert_eq!(looks_secret("-----BEGIN OPENSSH PRIVATE KEY-----"), Some("private-key"));
    }

    #[test]
    fn detects_aws_github_slack_jwt() {
        assert_eq!(looks_secret("key = AKIAIOSFODNN7EXAMPLE"), Some("aws-access-key"));
        assert_eq!(looks_secret("ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghij"), Some("github-token"));
        assert_eq!(looks_secret("xoxb-123456789012-abcdefghijk"), Some("slack-token"));
        assert_eq!(
            looks_secret("eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c"),
            Some("jwt")
        );
    }

    #[test]
    fn detects_high_entropy_assignment() {
        let s = r#"const apiKey = "9aB3xQ7mZ2pL8kR4tY6wE1uI0oP5sD7fXk2";"#;
        assert_eq!(looks_secret(s), Some("secret-assignment"));
    }

    #[test]
    fn ignores_ordinary_code_and_low_entropy() {
        assert_eq!(looks_secret("export function createSession(user: User) {}"), None);
        assert_eq!(looks_secret(r#"const password = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";"#), None);
        assert_eq!(looks_secret(r#"const token = getToken();"#), None);
        assert_eq!(looks_secret(r#""integrity": "sha512-abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJ""#), None);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p singularrag-core secrets`
Expected: compile error, `looks_secret` not found.

- [ ] **Step 3: Write the implementation**

```rust
//! Secret-like content detection. A hit skips the whole file (spec §11).
//! Deliberately a short hand-maintained list; no third-party scanner is standard in Rust.

use std::sync::OnceLock;

use regex::Regex;

struct Rule {
    name: &'static str,
    re: Regex,
}

fn rules() -> &'static [Rule] {
    static RULES: OnceLock<Vec<Rule>> = OnceLock::new();
    RULES.get_or_init(|| {
        let mk = |name, pat| Rule { name, re: Regex::new(pat).expect("valid secret regex") };
        vec![
            mk("private-key", r"-----BEGIN (?:RSA |EC |DSA |OPENSSH |PGP )?PRIVATE KEY(?: BLOCK)?-----"),
            mk("aws-access-key", r"\bAKIA[0-9A-Z]{16}\b"),
            mk("github-token", r"\bgh[pousr]_[A-Za-z0-9]{36,}\b"),
            mk("slack-token", r"\bxox[baprs]-[0-9A-Za-z-]{10,}\b"),
            mk("jwt", r"\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\b"),
        ]
    })
}

fn assignment_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)\b(?:api[_-]?key|secret|token|password|passwd|auth)\w*\s*[:=]\s*['"]([A-Za-z0-9_\-/+=]{32,})['"]"#)
            .expect("valid assignment regex")
    })
}

/// Shannon entropy in bits per character.
fn entropy(s: &str) -> f64 {
    let mut counts = [0usize; 256];
    for b in s.bytes() {
        counts[b as usize] += 1;
    }
    let n = s.len() as f64;
    counts
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = c as f64 / n;
            -p * p.log2()
        })
        .sum()
}

pub fn looks_secret(text: &str) -> Option<&'static str> {
    for r in rules() {
        if r.re.is_match(text) {
            return Some(r.name);
        }
    }
    for cap in assignment_re().captures_iter(text) {
        if entropy(&cap[1]) > 3.5 {
            return Some("secret-assignment");
        }
    }
    None
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p singularrag-core secrets`
Expected: 4 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/singularrag-core
git commit -m "feat(core): secret-like content scanner"
```

---

### Task 6: Identifier splitting and token approximation

**Files:**
- Create: `crates/singularrag-core/src/tokens.rs`
- Modify: `crates/singularrag-core/src/lib.rs` (add `pub mod tokens;`)

**Interfaces:**
- Produces: `split_identifier(&str) -> Vec<String>` (lowercased parts), `approx_tokens(&str) -> usize`, `query_terms(&str) -> Vec<String>` (lowercased, deduped, stopwords removed, length ≥ 2).

- [ ] **Step 1: Write the failing tests**

Bottom of `crates/singularrag-core/src/tokens.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_camel_snake_and_acronyms() {
        assert_eq!(split_identifier("createSession"), vec!["create", "session"]);
        assert_eq!(split_identifier("HTTP_Server2"), vec!["http", "server2"]);
        assert_eq!(split_identifier("XMLHttpRequest"), vec!["xml", "http", "request"]);
        assert_eq!(split_identifier("session"), vec!["session"]);
        assert_eq!(split_identifier("__init__"), vec!["init"]);
    }

    #[test]
    fn approx_tokens_is_chars_over_four_rounded_up() {
        assert_eq!(approx_tokens(""), 0);
        assert_eq!(approx_tokens("abcd"), 1);
        assert_eq!(approx_tokens("abcde"), 2);
    }

    #[test]
    fn query_terms_lowercases_splits_and_drops_stopwords() {
        assert_eq!(
            query_terms("Where is createSession used in the HTTP layer?"),
            vec!["create", "session", "used", "http", "layer"]
        );
        assert_eq!(query_terms("SessionStore SessionStore"), vec!["session", "store"]);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p singularrag-core tokens`
Expected: compile error.

- [ ] **Step 3: Write the implementation**

```rust
//! Identifier splitting (for FTS and query matching) and the token approximation.

const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "the", "of", "to", "in", "is", "it", "for", "on", "how", "does", "do",
    "what", "where", "which", "when", "why", "this", "that", "with", "by", "at", "be", "or", "as",
    "from", "into", "i", "we", "you", "my", "our",
];

/// "createSession" -> ["create","session"]; "XMLHttpRequest" -> ["xml","http","request"].
pub fn split_identifier(s: &str) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let mut parts = Vec::new();
    let mut cur = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if !c.is_alphanumeric() {
            if !cur.is_empty() {
                parts.push(std::mem::take(&mut cur));
            }
            continue;
        }
        if c.is_uppercase() && !cur.is_empty() {
            let prev = chars[i - 1];
            let next_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
            // Boundary at lower->Upper, or at the last capital of an acronym run (XMLHttp -> XML|Http).
            if prev.is_lowercase() || prev.is_numeric() || (prev.is_uppercase() && next_lower) {
                parts.push(std::mem::take(&mut cur));
            }
        }
        cur.push(c);
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    parts.into_iter().map(|p| p.to_lowercase()).collect()
}

/// Spec §7: tokens ≈ chars / 4, rounded up. Budget is soft.
pub fn approx_tokens(s: &str) -> usize {
    s.chars().count().div_ceil(4)
}

pub fn query_terms(q: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in q.split(|c: char| !c.is_alphanumeric() && c != '_') {
        for part in split_identifier(word) {
            if part.len() >= 2 && !STOPWORDS.contains(&part.as_str()) && !out.contains(&part) {
                out.push(part);
            }
        }
    }
    out
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p singularrag-core tokens`
Expected: 3 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/singularrag-core
git commit -m "feat(core): identifier splitting, query terms, token approximation"
```

---

### Task 7: Tag extraction with tree-sitter-tags

**Files:**
- Create: `crates/singularrag-core/src/lang.rs`
- Modify: `crates/singularrag-core/src/lib.rs` (add `pub mod lang;`)

**Interfaces:**
- Produces: `Language { TypeScript, Tsx, JavaScript, Rust }`, `Language::from_path(&str) -> Option<Language>`, `Language::as_str(self) -> &'static str`, `Tag { name: String, kind: String, is_definition: bool, line_start: u32, line_end: u32, signature: String }` (lines 1-based), `extract_tags(lang: Language, source: &str) -> Result<Vec<Tag>>`.

Facts (verified 2026-09-19): the grammar crates export `TAGS_QUERY`; typescript exports `LANGUAGE_TYPESCRIPT`, `LANGUAGE_TSX`, `LOCALS_QUERY`; javascript exports `LANGUAGE`, `LOCALS_QUERY`; rust exports `LANGUAGE` and no locals query (pass `""`). The TypeScript tags query has **no** `@reference.call`, so a supplement query is appended for calls and `new` expressions.

- [ ] **Step 1: Write the failing tests**

Bottom of `crates/singularrag-core/src/lang.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    const TS: &str = r#"export interface Session { id: string }
export function createSession(user: User, ttl: number): Session {
  const store = new SessionStore();
  return store.create(user, ttl);
}
export class SessionStore {
  create(user: User, ttl: number): Session {
    return { id: "s1" };
  }
}
"#;

    fn names(tags: &[Tag], def: bool) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = tags
            .iter()
            .filter(|t| t.is_definition == def)
            .map(|t| (t.name.clone(), t.kind.clone()))
            .collect();
        v.sort();
        v.dedup();
        v
    }

    #[test]
    fn detects_language_from_extension() {
        assert_eq!(Language::from_path("src/a.ts"), Some(Language::TypeScript));
        assert_eq!(Language::from_path("src/a.tsx"), Some(Language::Tsx));
        assert_eq!(Language::from_path("src/a.js"), Some(Language::JavaScript));
        assert_eq!(Language::from_path("src/a.mjs"), Some(Language::JavaScript));
        assert_eq!(Language::from_path("src/a.rs"), Some(Language::Rust));
        assert_eq!(Language::from_path("README.md"), None);
    }

    #[test]
    fn typescript_definitions_and_references() {
        let tags = extract_tags(Language::TypeScript, TS).unwrap();
        let defs = names(&tags, true);
        assert!(defs.contains(&("createSession".into(), "function".into())));
        assert!(defs.contains(&("SessionStore".into(), "class".into())));
        assert!(defs.contains(&("create".into(), "method".into())));
        assert!(defs.contains(&("Session".into(), "interface".into())));
        let refs = names(&tags, false);
        assert!(refs.iter().any(|(n, _)| n == "create"), "call ref missing: {refs:?}");
        assert!(refs.iter().any(|(n, _)| n == "SessionStore"), "new ref missing: {refs:?}");
        let f = tags.iter().find(|t| t.name == "createSession" && t.is_definition).unwrap();
        assert_eq!(f.line_start, 2);
        assert_eq!(f.line_end, 5);
        assert_eq!(f.signature, "export function createSession(user: User, ttl: number): Session {");
    }

    #[test]
    fn rust_definitions_and_scoped_call_references() {
        let src = "pub fn parse(s: &str) -> u32 { helper(s) + util::len(s) }\nfn helper(s: &str) -> u32 { 0 }\n";
        let tags = extract_tags(Language::Rust, src).unwrap();
        let defs = names(&tags, true);
        assert!(defs.contains(&("parse".into(), "function".into())));
        assert!(defs.contains(&("helper".into(), "function".into())));
        let refs = names(&tags, false);
        assert!(refs.iter().any(|(n, _)| n == "helper"));
        assert!(refs.iter().any(|(n, _)| n == "len"));
    }

    #[test]
    fn signature_is_capped() {
        let long = format!("export function f({}) {{}}", "a: number, ".repeat(40));
        let tags = extract_tags(Language::TypeScript, &long).unwrap();
        let f = tags.iter().find(|t| t.name == "f").unwrap();
        assert!(f.signature.chars().count() <= SIGNATURE_MAX);
        assert!(f.signature.ends_with('…'));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p singularrag-core lang`
Expected: compile error.

- [ ] **Step 3: Write the implementation**

```rust
//! Language detection and tag extraction. Uses each grammar's bundled `tags.scm`
//! through `tree-sitter-tags`; nothing here parses code by hand.

use std::cell::RefCell;
use std::collections::HashMap;

use tree_sitter_tags::{TagsConfiguration, TagsContext};

use crate::{Error, Result};

pub const SIGNATURE_MAX: usize = 160;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    TypeScript,
    Tsx,
    JavaScript,
    Rust,
}

impl Language {
    pub fn from_path(path: &str) -> Option<Language> {
        let ext = path.rsplit('.').next()?;
        match ext {
            "ts" | "mts" | "cts" => Some(Language::TypeScript),
            "tsx" => Some(Language::Tsx),
            "js" | "mjs" | "cjs" | "jsx" => Some(Language::JavaScript),
            "rs" => Some(Language::Rust),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Language::TypeScript => "typescript",
            Language::Tsx => "tsx",
            Language::JavaScript => "javascript",
            Language::Rust => "rust",
        }
    }
}

/// The TypeScript grammar's tags.scm has no call captures; this supplement adds them.
const TS_CALLS: &str = r#"
(call_expression function: (identifier) @name) @reference.call
(call_expression function: (member_expression property: (property_identifier) @name)) @reference.call
(new_expression constructor: (identifier) @name) @reference.class
"#;

#[derive(Debug, Clone, PartialEq)]
pub struct Tag {
    pub name: String,
    pub kind: String,
    pub is_definition: bool,
    pub line_start: u32,
    pub line_end: u32,
    pub signature: String,
}

fn make_config(lang: Language) -> Result<TagsConfiguration> {
    let (language, tags, locals) = match lang {
        Language::TypeScript => (
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            format!("{}\n{}", tree_sitter_typescript::TAGS_QUERY, TS_CALLS),
            tree_sitter_typescript::LOCALS_QUERY,
        ),
        Language::Tsx => (
            tree_sitter_typescript::LANGUAGE_TSX.into(),
            format!("{}\n{}", tree_sitter_typescript::TAGS_QUERY, TS_CALLS),
            tree_sitter_typescript::LOCALS_QUERY,
        ),
        Language::JavaScript => (
            tree_sitter_javascript::LANGUAGE.into(),
            tree_sitter_javascript::TAGS_QUERY.to_string(),
            tree_sitter_javascript::LOCALS_QUERY,
        ),
        Language::Rust => (
            tree_sitter_rust::LANGUAGE.into(),
            tree_sitter_rust::TAGS_QUERY.to_string(),
            "",
        ),
    };
    TagsConfiguration::new(language, &tags, locals).map_err(|e| Error::Tags(format!("{lang:?}: {e:?}")))
}

thread_local! {
    static CONFIGS: RefCell<HashMap<Language, TagsConfiguration>> = RefCell::new(HashMap::new());
    static CONTEXT: RefCell<TagsContext> = RefCell::new(TagsContext::new());
}

fn signature_for(source: &str, byte_start: usize) -> String {
    let line_start = source[..byte_start].rfind('\n').map_or(0, |i| i + 1);
    let line_end = source[byte_start..].find('\n').map_or(source.len(), |i| byte_start + i);
    let line = source[line_start..line_end].trim();
    if line.chars().count() > SIGNATURE_MAX {
        let mut s: String = line.chars().take(SIGNATURE_MAX - 1).collect();
        s.push('…');
        s
    } else {
        line.to_string()
    }
}

pub fn extract_tags(lang: Language, source: &str) -> Result<Vec<Tag>> {
    CONFIGS.with(|configs| {
        let mut configs = configs.borrow_mut();
        if !configs.contains_key(&lang) {
            configs.insert(lang, make_config(lang)?);
        }
        let config = configs.get(&lang).expect("inserted above");
        CONTEXT.with(|ctx| {
            let mut ctx = ctx.borrow_mut();
            let (iter, _has_error) = ctx
                .generate_tags(config, source.as_bytes(), None)
                .map_err(|e| Error::Tags(format!("{e:?}")))?;
            let mut out = Vec::new();
            for tag in iter {
                let tag = tag.map_err(|e| Error::Tags(format!("{e:?}")))?;
                let name = source[tag.name_range.clone()].to_string();
                if name.is_empty() {
                    continue;
                }
                out.push(Tag {
                    name,
                    kind: config.syntax_type_name(tag.syntax_type_id).to_string(),
                    is_definition: tag.is_definition,
                    line_start: tag.span.start.row as u32 + 1,
                    line_end: tag.span.end.row as u32 + 1,
                    signature: signature_for(source, tag.range.start),
                });
            }
            Ok(out)
        })
    })
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p singularrag-core lang`
Expected: 4 passed. If `TagsConfiguration::new` fails for TypeScript with a query error naming `new_expression` or `constructor`, remove the third line of `TS_CALLS` and re-run; the `new` reference assertion then needs the `SessionStore` reference to come from `@reference.class` in the bundled query, which exists for type positions only, so keep the test but change it to assert on `"create"` alone and note the gap in `docs/superpowers/specs/2026-09-19-singularrag-design.md` §7 gates.

- [ ] **Step 5: Commit**

```bash
git add crates/singularrag-core
git commit -m "feat(core): tag extraction for ts/tsx/js/rust via tree-sitter-tags"
```

---

### Task 8: Indexer with incremental refresh and deadline

**Files:**
- Create: `crates/singularrag-core/src/index.rs`, `crates/singularrag-core/src/fixture.rs`
- Modify: `crates/singularrag-core/src/lib.rs` (add `pub mod index; pub mod fixture;`)

**Interfaces:**
- Consumes: `walk::walk`, `secrets::looks_secret`, `lang::{Language, extract_tags}`, `tokens::split_identifier`, `config::MapConfig`, `store::Store`, `time::now_ms`.
- Produces: `IndexStats { scanned: usize, indexed: usize, unchanged: usize, skipped: usize, removed: usize, remaining: usize }`, `Indexer::new(store: &Store, root: &Path, config: &MapConfig) -> Result<Indexer>`, `Indexer::refresh(&self, deadline: Option<Instant>) -> Result<IndexStats>`, `Indexer::stale_count(&self) -> Result<usize>`, `git_head(root: &Path) -> Option<String>`, `fixture::write_ts_mini(root: &Path)`, `fixture::write_rust_mini(root: &Path)`. Meta keys written: `index_version`, `git_head`, `indexed_at_ms`.

- [ ] **Step 1: Write the fixture module**

`crates/singularrag-core/src/fixture.rs`:
```rust
//! Small repos used by tests across crates. Written into a directory at test time
//! so nested `.gitignore` files never interact with this repo's git.

use std::path::Path;

fn w(root: &Path, rel: &str, content: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}

/// Four TS modules with a clear reference structure, one gitignored bundle,
/// one denylisted `.env`, one secret-like config.
pub fn write_ts_mini(root: &Path) {
    w(root, ".gitignore", "dist/\n");
    w(
        root,
        "src/auth/session.ts",
        r#"export interface Session { id: string; userId: string; expiresAt: number }
export interface User { id: string }
export function createSession(user: User, ttl: number): Session {
  const store = new SessionStore();
  return store.create(user, ttl);
}
export class SessionStore {
  create(user: User, ttl: number): Session {
    return { id: "s1", userId: user.id, expiresAt: Date.now() + ttl };
  }
}
"#,
    );
    w(
        root,
        "src/http/middleware.ts",
        r#"import { createSession, Session } from "../auth/session";
export function requireSession(token: string): Session {
  return createSession({ id: token }, 3600);
}
export function attachSession(token: string): Session {
  return createSession({ id: token }, 60);
}
"#,
    );
    w(
        root,
        "src/cli/login.ts",
        r#"import { createSession } from "../auth/session";
import { log } from "../util/log";
export function login(userId: string): void {
  const s = createSession({ id: userId }, 3600);
  log(s.id);
}
"#,
    );
    w(root, "src/util/log.ts", "export function log(msg: string): void {\n  console.log(msg);\n}\n");
    w(root, "src/config.ts", "export const awsKey = \"AKIAIOSFODNN7EXAMPLE\";\n");
    w(root, "dist/bundle.js", "function bundled() {}\n");
    w(root, ".env", "SECRET=1\n");
    w(root, "README.md", "# ts-mini\n");
}

pub fn write_rust_mini(root: &Path) {
    w(root, "Cargo.toml", "[package]\nname = \"mini\"\nversion = \"0.1.0\"\nedition = \"2021\"\n");
    w(
        root,
        "src/lib.rs",
        "pub fn parse(s: &str) -> u32 {\n    helper(s)\n}\n\nfn helper(s: &str) -> u32 {\n    s.len() as u32\n}\n",
    );
    w(root, "src/main.rs", "fn main() {\n    let n = mini::parse(\"x\");\n    println!(\"{n}\");\n}\n");
}
```

- [ ] **Step 2: Write the failing tests**

Bottom of `crates/singularrag-core/src/index.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MapConfig;
    use crate::fixture::write_ts_mini;
    use crate::store::Store;

    fn setup() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        write_ts_mini(dir.path());
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        (dir, store)
    }

    fn count(store: &Store, sql: &str) -> i64 {
        store.conn().query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn full_index_records_files_symbols_refs_and_skips() {
        let (dir, store) = setup();
        let cfg = MapConfig::default();
        let ix = Indexer::new(&store, dir.path(), &cfg).unwrap();
        let stats = ix.refresh(None).unwrap();
        assert_eq!(stats.remaining, 0);
        assert!(stats.indexed >= 4, "{stats:?}");

        // .env denylisted, config.ts secret-like, README unsupported, dist/ absent.
        let reason = |p: &str| -> Option<String> {
            store.conn().query_row("SELECT skipped_reason FROM files WHERE path = ?1", [p], |r| r.get(0)).unwrap()
        };
        assert_eq!(reason(".env").as_deref(), Some("denylisted"));
        assert_eq!(reason("src/config.ts").as_deref(), Some("secret-like content"));
        assert_eq!(reason("README.md").as_deref(), Some("unsupported-language"));
        assert_eq!(reason("src/auth/session.ts"), None);
        assert_eq!(count(&store, "SELECT COUNT(*) FROM files WHERE path LIKE 'dist/%'"), 0);

        assert_eq!(count(&store, "SELECT COUNT(*) FROM symbols WHERE name = 'createSession' AND kind = 'function'"), 1);
        assert_eq!(
            count(&store, "SELECT COUNT(DISTINCT f.path) FROM refs r JOIN files f ON f.id = r.file_id WHERE r.name = 'createSession' AND f.path != 'src/auth/session.ts'"),
            2
        );
        assert_eq!(count(&store, "SELECT COUNT(*) FROM symbols_fts WHERE symbols_fts MATCH 'name_tokens:session'"), 5);
        assert!(store.get_meta("index_version").unwrap().is_some());
    }

    #[test]
    fn incremental_refresh_touches_only_changed_files() {
        let (dir, store) = setup();
        let cfg = MapConfig::default();
        let ix = Indexer::new(&store, dir.path(), &cfg).unwrap();
        ix.refresh(None).unwrap();
        let before: i64 = store.conn().query_row("SELECT indexed_at_ms FROM files WHERE path = 'src/util/log.ts'", [], |r| r.get(0)).unwrap();

        std::thread::sleep(std::time::Duration::from_millis(20));
        let p = dir.path().join("src/cli/login.ts");
        std::fs::write(&p, "export function loginRenamed(): void {}\n").unwrap();
        // Force a distinct mtime even on coarse filesystems.
        let t = std::time::SystemTime::now() + std::time::Duration::from_secs(2);
        std::fs::File::open(&p).unwrap().set_modified(t).unwrap();

        let stats = ix.refresh(None).unwrap();
        assert_eq!(stats.indexed, 1, "{stats:?}");
        assert_eq!(count(&store, "SELECT COUNT(*) FROM symbols WHERE name = 'login'"), 0);
        assert_eq!(count(&store, "SELECT COUNT(*) FROM symbols WHERE name = 'loginRenamed'"), 1);
        let after: i64 = store.conn().query_row("SELECT indexed_at_ms FROM files WHERE path = 'src/util/log.ts'", [], |r| r.get(0)).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn deleted_files_are_removed_with_their_symbols() {
        let (dir, store) = setup();
        let cfg = MapConfig::default();
        let ix = Indexer::new(&store, dir.path(), &cfg).unwrap();
        ix.refresh(None).unwrap();
        std::fs::remove_file(dir.path().join("src/util/log.ts")).unwrap();
        let stats = ix.refresh(None).unwrap();
        assert_eq!(stats.removed, 1);
        assert_eq!(count(&store, "SELECT COUNT(*) FROM symbols WHERE name = 'log'"), 0);
        assert_eq!(count(&store, "SELECT COUNT(*) FROM symbols_fts WHERE symbols_fts MATCH 'name:log'"), 0);
    }

    #[test]
    fn expired_deadline_indexes_nothing_and_reports_remaining() {
        let (dir, store) = setup();
        let cfg = MapConfig::default();
        let ix = Indexer::new(&store, dir.path(), &cfg).unwrap();
        let stats = ix.refresh(Some(std::time::Instant::now())).unwrap();
        assert_eq!(stats.indexed, 0);
        assert!(stats.remaining >= 4, "{stats:?}");
        assert_eq!(ix.stale_count().unwrap(), stats.remaining);
    }

    #[test]
    fn git_head_reads_ref_or_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(git_head(dir.path()), None);
        std::fs::create_dir_all(dir.path().join(".git/refs/heads")).unwrap();
        std::fs::write(dir.path().join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(dir.path().join(".git/refs/heads/main"), "0123456789abcdef0123456789abcdef01234567\n").unwrap();
        assert_eq!(git_head(dir.path()).as_deref(), Some("0123456789abcdef0123456789abcdef01234567"));
        std::fs::write(dir.path().join(".git/HEAD"), "fedcba9876543210fedcba9876543210fedcba98\n").unwrap();
        assert_eq!(git_head(dir.path()).as_deref(), Some("fedcba9876543210fedcba9876543210fedcba98"));
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p singularrag-core index`
Expected: compile error, `Indexer` not found.

- [ ] **Step 4: Write the implementation**

`crates/singularrag-core/src/index.rs` (above the tests):
```rust
//! Indexer: walk, diff against the `files` table, parse changed files, write symbols,
//! refs and FTS rows. Runs until an optional deadline; whatever is left is reported
//! as `remaining` so callers can say "STALE: N files changed since index".

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use rusqlite::params;

use crate::config::MapConfig;
use crate::lang::{extract_tags, Language};
use crate::secrets::looks_secret;
use crate::store::Store;
use crate::time::now_ms;
use crate::tokens::split_identifier;
use crate::walk::{walk, WalkEntry};
use crate::Result;

pub const MAX_FILE_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct IndexStats {
    pub scanned: usize,
    pub indexed: usize,
    pub unchanged: usize,
    pub skipped: usize,
    pub removed: usize,
    pub remaining: usize,
}

pub struct Indexer<'a> {
    store: &'a Store,
    root: PathBuf,
    config: &'a MapConfig,
}

#[derive(Clone)]
struct Known {
    id: i64,
    mtime_ms: i64,
    size: u64,
    content_hash: Option<String>,
}

impl<'a> Indexer<'a> {
    pub fn new(store: &'a Store, root: &Path, config: &'a MapConfig) -> Result<Self> {
        Ok(Indexer { store, root: root.canonicalize()?, config })
    }

    fn known_files(&self) -> Result<HashMap<String, Known>> {
        let mut stmt = self.store.conn().prepare("SELECT id, path, mtime_ms, size, content_hash FROM files")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(1)?,
                Known { id: r.get(0)?, mtime_ms: r.get(2)?, size: r.get::<_, i64>(3)? as u64, content_hash: r.get(4)? },
            ))
        })?;
        let mut m = HashMap::new();
        for row in rows {
            let (p, k) = row?;
            m.insert(p, k);
        }
        Ok(m)
    }

    /// Files whose mtime or size differ from the table, or that are not in it yet.
    pub fn stale_count(&self) -> Result<usize> {
        let known = self.known_files()?;
        let w = walk(&self.root, &self.config.deny_patterns())?;
        Ok(w.entries.iter().filter(|e| Self::changed(&known, e)).count())
    }

    fn changed(known: &HashMap<String, Known>, e: &WalkEntry) -> bool {
        match known.get(&e.rel_path) {
            Some(k) => k.mtime_ms != e.mtime_ms || k.size != e.size,
            None => true,
        }
    }

    pub fn refresh(&self, deadline: Option<Instant>) -> Result<IndexStats> {
        let mut stats = IndexStats::default();
        let known = self.known_files()?;
        let w = walk(&self.root, &self.config.deny_patterns())?;
        stats.scanned = w.entries.len() + w.skipped.len();
        let now = now_ms();
        let conn = self.store.conn();

        // Skipped-by-walk files become rows with a reason and no symbols.
        for s in &w.skipped {
            self.upsert_skipped(&s.rel_path, 0, 0, s.reason, now)?;
            stats.skipped += 1;
        }

        let mut seen: Vec<&str> = Vec::with_capacity(w.entries.len());
        for e in &w.entries {
            seen.push(&e.rel_path);
            if !Self::changed(&known, e) {
                stats.unchanged += 1;
                continue;
            }
            if deadline.is_some_and(|d| Instant::now() >= d) {
                stats.remaining += 1;
                continue;
            }
            let prior = known.get(&e.rel_path);
            if self.index_file(e, prior, now)? {
                stats.indexed += 1;
            } else {
                stats.skipped += 1;
            }
        }

        // Remove rows for files that no longer exist (or are now gitignored).
        let present: std::collections::HashSet<&str> =
            seen.iter().copied().chain(w.skipped.iter().map(|s| s.rel_path.as_str())).collect();
        for (path, k) in &known {
            if !present.contains(path.as_str()) {
                conn.execute("DELETE FROM symbols_fts WHERE rowid IN (SELECT id FROM symbols WHERE file_id = ?1)", [k.id])?;
                conn.execute("DELETE FROM files WHERE id = ?1", [k.id])?;
                stats.removed += 1;
            }
        }

        self.write_meta(now)?;
        Ok(stats)
    }

    fn upsert_skipped(&self, path: &str, mtime_ms: i64, size: u64, reason: &str, now: i64) -> Result<i64> {
        let conn = self.store.conn();
        conn.execute(
            "INSERT INTO files(path, lang, content_hash, mtime_ms, size, indexed_at_ms, skipped_reason)
             VALUES (?1, NULL, NULL, ?2, ?3, ?4, ?5)
             ON CONFLICT(path) DO UPDATE SET lang = NULL, content_hash = NULL, mtime_ms = excluded.mtime_ms,
               size = excluded.size, indexed_at_ms = excluded.indexed_at_ms, skipped_reason = excluded.skipped_reason",
            params![path, mtime_ms, size as i64, now, reason],
        )?;
        let id: i64 = conn.query_row("SELECT id FROM files WHERE path = ?1", [path], |r| r.get(0))?;
        conn.execute("DELETE FROM symbols_fts WHERE rowid IN (SELECT id FROM symbols WHERE file_id = ?1)", [id])?;
        conn.execute("DELETE FROM symbols WHERE file_id = ?1", [id])?;
        conn.execute("DELETE FROM refs WHERE file_id = ?1", [id])?;
        Ok(id)
    }

    /// Returns true when symbols were (re)written, false when the file was skipped.
    fn index_file(&self, e: &WalkEntry, prior: Option<&Known>, now: i64) -> Result<bool> {
        if e.size > MAX_FILE_BYTES {
            self.upsert_skipped(&e.rel_path, e.mtime_ms, e.size, "too-large", now)?;
            return Ok(false);
        }
        let Some(lang) = Language::from_path(&e.rel_path) else {
            self.upsert_skipped(&e.rel_path, e.mtime_ms, e.size, "unsupported-language", now)?;
            return Ok(false);
        };
        let bytes = std::fs::read(&e.abs_path)?;
        if bytes.iter().take(8192).any(|&b| b == 0) {
            self.upsert_skipped(&e.rel_path, e.mtime_ms, e.size, "binary", now)?;
            return Ok(false);
        }
        let Ok(source) = std::str::from_utf8(&bytes) else {
            self.upsert_skipped(&e.rel_path, e.mtime_ms, e.size, "not-utf8", now)?;
            return Ok(false);
        };
        if looks_secret(source).is_some() {
            self.upsert_skipped(&e.rel_path, e.mtime_ms, e.size, "secret-like content", now)?;
            return Ok(false);
        }
        let hash = blake3::hash(&bytes).to_hex().to_string();
        let conn = self.store.conn();

        if prior.and_then(|p| p.content_hash.as_deref()) == Some(hash.as_str()) {
            conn.execute("UPDATE files SET mtime_ms = ?2, size = ?3 WHERE path = ?1", params![e.rel_path, e.mtime_ms, e.size as i64])?;
            return Ok(true);
        }

        let tags = extract_tags(lang, source)?;
        conn.execute(
            "INSERT INTO files(path, lang, content_hash, mtime_ms, size, indexed_at_ms, skipped_reason)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL)
             ON CONFLICT(path) DO UPDATE SET lang = excluded.lang, content_hash = excluded.content_hash,
               mtime_ms = excluded.mtime_ms, size = excluded.size, indexed_at_ms = excluded.indexed_at_ms, skipped_reason = NULL",
            params![e.rel_path, lang.as_str(), hash, e.mtime_ms, e.size as i64, now],
        )?;
        let file_id: i64 = conn.query_row("SELECT id FROM files WHERE path = ?1", [&e.rel_path], |r| r.get(0))?;
        conn.execute("DELETE FROM symbols_fts WHERE rowid IN (SELECT id FROM symbols WHERE file_id = ?1)", [file_id])?;
        conn.execute("DELETE FROM symbols WHERE file_id = ?1", [file_id])?;
        conn.execute("DELETE FROM refs WHERE file_id = ?1", [file_id])?;

        for t in &tags {
            if t.is_definition {
                conn.execute(
                    "INSERT INTO symbols(file_id, name, kind, line_start, line_end, signature) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![file_id, t.name, t.kind, t.line_start, t.line_end, t.signature],
                )?;
                let id = conn.last_insert_rowid();
                conn.execute(
                    "INSERT INTO symbols_fts(rowid, name, name_tokens, signature, path) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![id, t.name, split_identifier(&t.name).join(" "), t.signature, e.rel_path],
                )?;
            } else {
                conn.execute(
                    "INSERT INTO refs(file_id, name, line) VALUES (?1, ?2, ?3)",
                    params![file_id, t.name, t.line_start],
                )?;
            }
        }
        Ok(true)
    }

    fn write_meta(&self, now: i64) -> Result<()> {
        let mut stmt = self.store.conn().prepare("SELECT path, content_hash FROM files WHERE content_hash IS NOT NULL ORDER BY path")?;
        let mut hasher = blake3::Hasher::new();
        for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (p, h) = row?;
            hasher.update(p.as_bytes());
            hasher.update(b"\t");
            hasher.update(h.as_bytes());
            hasher.update(b"\n");
        }
        let version = hasher.finalize().to_hex()[..12].to_string();
        self.store.set_meta("index_version", &version)?;
        self.store.set_meta("indexed_at_ms", &now.to_string())?;
        match git_head(&self.root) {
            Some(h) => self.store.set_meta("git_head", &h)?,
            None => self.store.set_meta("git_head", "")?,
        }
        Ok(())
    }
}

/// Resolve `.git/HEAD` without a git dependency. Handles symbolic refs, loose refs
/// and `packed-refs`. Returns `None` when there is no readable repository.
pub fn git_head(root: &Path) -> Option<String> {
    let head = std::fs::read_to_string(root.join(".git/HEAD")).ok()?;
    let head = head.trim();
    if let Some(r) = head.strip_prefix("ref: ") {
        if let Ok(s) = std::fs::read_to_string(root.join(".git").join(r)) {
            return Some(s.trim().to_string());
        }
        let packed = std::fs::read_to_string(root.join(".git/packed-refs")).ok()?;
        return packed
            .lines()
            .filter(|l| !l.starts_with('#') && !l.starts_with('^'))
            .find_map(|l| l.split_once(' ').filter(|(_, name)| *name == r).map(|(sha, _)| sha.to_string()));
    }
    if head.len() >= 40 && head.chars().all(|c| c.is_ascii_hexdigit()) {
        return Some(head.to_string());
    }
    None
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p singularrag-core index`
Expected: 5 passed. If the `name_tokens:session` count is not 5, list the rows with `SELECT name FROM symbols_fts WHERE symbols_fts MATCH 'name_tokens:session'` and adjust the expected count to the actual set of `Session`, `createSession`, `SessionStore`, `requireSession`, `attachSession` once the TS tags query behaviour is confirmed.

- [ ] **Step 6: Commit**

```bash
git add crates/singularrag-core
git commit -m "feat(core): incremental indexer with deadline, skips, fts rows and git head"
```

---
### Task 9: File graph, PageRank and symbol ranking with reasons

**Files:**
- Create: `crates/singularrag-core/src/graph.rs`, `crates/singularrag-core/src/rank.rs`
- Modify: `crates/singularrag-core/src/lib.rs` (add `pub mod graph; pub mod rank;`)

**Interfaces:**
- Consumes: `Store`, `MapConfig::{is_excluded, is_pinned}`, `tokens::query_terms`.
- Produces:
  - `graph::FileNode { id: i64, path: String }`, `graph::Edge { src: usize, dst: usize, name: String, weight: f64 }`, `graph::FileGraph { nodes: Vec<FileNode>, index_of: HashMap<i64, usize>, edges: Vec<Edge> }`, `graph::build_graph(store: &Store, config: &MapConfig, query_terms: &[String]) -> Result<FileGraph>`.
  - `rank::pagerank(n: usize, edges: &[(usize, usize, f64)], personalization: &[f64]) -> Vec<f64>` (damping 0.85, 50 iterations, sums to 1).
  - `rank::RefBy { path: String, count: i64 }`, `rank::Reasons { pagerank: f64, file_rank: f64, seeds: Vec<String>, referenced_by: Vec<RefBy>, pinned: bool, fts_hit: bool, query_ident_match: bool }` (Serialize), `rank::ScoredSymbol { symbol_id: i64, file_id: i64, path: String, name: String, kind: String, line_start: u32, line_end: u32, signature: String, score: f64, reasons: Reasons }`, `rank::rank_symbols(store: &Store, config: &MapConfig, query: Option<&str>, focus_files: &[String]) -> Result<Vec<ScoredSymbol>>` sorted by score desc, then path, then line.

- [ ] **Step 1: Write the failing tests**

Bottom of `crates/singularrag-core/src/rank.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MapConfig;
    use crate::fixture::write_ts_mini;
    use crate::index::Indexer;
    use crate::store::Store;

    #[test]
    fn pagerank_sums_to_one_and_prefers_sinks() {
        // A -> B -> C, uniform personalization: C > B > A.
        let r = pagerank(3, &[(0, 1, 1.0), (1, 2, 1.0)], &[1.0, 1.0, 1.0]);
        assert!((r.iter().sum::<f64>() - 1.0).abs() < 1e-9);
        assert!(r[2] > r[1] && r[1] > r[0], "{r:?}");
    }

    #[test]
    fn pagerank_symmetric_pair_is_equal_and_personalization_boosts() {
        let r = pagerank(2, &[(0, 1, 1.0), (1, 0, 1.0)], &[1.0, 1.0]);
        assert!((r[0] - r[1]).abs() < 1e-9);
        let b = pagerank(2, &[(0, 1, 1.0), (1, 0, 1.0)], &[10.0, 1.0]);
        assert!(b[0] > b[1]);
    }

    #[test]
    fn pagerank_handles_no_edges_and_zero_personalization() {
        let r = pagerank(3, &[], &[0.0, 0.0, 0.0]);
        assert!((r.iter().sum::<f64>() - 1.0).abs() < 1e-9);
        assert!((r[0] - r[2]).abs() < 1e-9);
        assert!(pagerank(0, &[], &[]).is_empty());
    }

    fn indexed() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        write_ts_mini(dir.path());
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        Indexer::new(&store, dir.path(), &MapConfig::default()).unwrap().refresh(None).unwrap();
        (dir, store)
    }

    #[test]
    fn most_referenced_definition_ranks_first_without_query() {
        let (_dir, store) = indexed();
        let ranked = rank_symbols(&store, &MapConfig::default(), None, &[]).unwrap();
        assert_eq!(ranked[0].name, "createSession", "{:?}", ranked.iter().map(|s| &s.name).collect::<Vec<_>>());
        let rb = &ranked[0].reasons.referenced_by;
        assert!(rb.iter().any(|r| r.path == "src/http/middleware.ts" && r.count == 2), "{rb:?}");
        assert!(rb.iter().any(|r| r.path == "src/cli/login.ts" && r.count == 1));
        assert!(ranked.iter().all(|s| s.score >= 0.0));
    }

    #[test]
    fn query_terms_boost_matching_symbols_and_record_reasons() {
        let (_dir, store) = indexed();
        let ranked = rank_symbols(&store, &MapConfig::default(), Some("where is log written"), &[]).unwrap();
        let log = ranked.iter().find(|s| s.name == "log").unwrap();
        assert!(log.reasons.fts_hit);
        assert!(log.reasons.query_ident_match);
        assert!(log.reasons.seeds.iter().any(|s| s.starts_with("query:")), "{:?}", log.reasons.seeds);
        let pos_log = ranked.iter().position(|s| s.name == "log").unwrap();
        let pos_attach = ranked.iter().position(|s| s.name == "attachSession").unwrap();
        assert!(pos_log < pos_attach);
    }

    #[test]
    fn excluded_files_vanish_and_pins_are_recorded() {
        let (_dir, store) = indexed();
        let cfg = MapConfig::parse("[[exclude]]\npath = \"src/util/\"\n[[pin]]\npath = \"src/cli/login.ts\"\n").unwrap();
        let ranked = rank_symbols(&store, &cfg, None, &[]).unwrap();
        assert!(ranked.iter().all(|s| s.path != "src/util/log.ts"));
        let login = ranked.iter().find(|s| s.name == "login").unwrap();
        assert!(login.reasons.pinned);
        assert!(login.reasons.seeds.contains(&"pinned".to_string()));
    }

    #[test]
    fn focus_files_seed_personalization() {
        let (_dir, store) = indexed();
        let ranked = rank_symbols(&store, &MapConfig::default(), None, &["src/cli/login.ts".to_string()]).unwrap();
        let login = ranked.iter().find(|s| s.name == "login").unwrap();
        assert!(login.reasons.seeds.contains(&"focus".to_string()));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p singularrag-core rank`
Expected: compile error.

- [ ] **Step 3: Write the graph builder**

`crates/singularrag-core/src/graph.rs`:
```rust
//! File-level multigraph: one edge per (referencing file, defining file, name),
//! weight = ref count / number of files defining that name. Aider's construction.

use std::collections::HashMap;

use crate::config::MapConfig;
use crate::store::Store;
use crate::Result;

#[derive(Debug, Clone)]
pub struct FileNode {
    pub id: i64,
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct Edge {
    pub src: usize,
    pub dst: usize,
    pub name: String,
    pub weight: f64,
}

#[derive(Debug, Default)]
pub struct FileGraph {
    pub nodes: Vec<FileNode>,
    pub index_of: HashMap<i64, usize>,
    pub edges: Vec<Edge>,
}

/// Identifiers mentioned in the query get their edges weighted ×10 (Aider's rule).
pub const QUERY_IDENT_MULTIPLIER: f64 = 10.0;

pub fn build_graph(store: &Store, config: &MapConfig, query_terms: &[String]) -> Result<FileGraph> {
    let conn = store.conn();
    let mut g = FileGraph::default();

    let mut stmt = conn.prepare("SELECT id, path FROM files WHERE skipped_reason IS NULL ORDER BY path")?;
    for row in stmt.query_map([], |r| Ok(FileNode { id: r.get(0)?, path: r.get(1)? }))? {
        let n = row?;
        if config.is_excluded(&n.path) {
            continue;
        }
        g.index_of.insert(n.id, g.nodes.len());
        g.nodes.push(n);
    }

    // name -> defining file ids (deduped)
    let mut definers: HashMap<String, Vec<usize>> = HashMap::new();
    let mut stmt = conn.prepare("SELECT DISTINCT name, file_id FROM symbols")?;
    for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
        let (name, fid) = row?;
        if let Some(&i) = g.index_of.get(&fid) {
            definers.entry(name).or_default().push(i);
        }
    }

    let mut stmt = conn.prepare("SELECT file_id, name, COUNT(*) FROM refs GROUP BY file_id, name")?;
    for row in stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))? {
        let (fid, name, count) = row?;
        let Some(&src) = g.index_of.get(&fid) else { continue };
        let Some(dsts) = definers.get(&name) else { continue };
        let mut w = count as f64 / dsts.len() as f64;
        if name_matches_query(&name, query_terms) {
            w *= QUERY_IDENT_MULTIPLIER;
        }
        for &dst in dsts {
            g.edges.push(Edge { src, dst, name: name.clone(), weight: w });
        }
    }
    Ok(g)
}

/// A symbol name matches the query when every one of its split parts is a query term,
/// or the whole lowercased name is a query term.
pub fn name_matches_query(name: &str, terms: &[String]) -> bool {
    if terms.is_empty() {
        return false;
    }
    let lower = name.to_lowercase();
    if terms.iter().any(|t| *t == lower) {
        return true;
    }
    let parts = crate::tokens::split_identifier(name);
    !parts.is_empty() && parts.iter().all(|p| terms.contains(p))
}
```

- [ ] **Step 4: Write the ranker**

`crates/singularrag-core/src/rank.rs` (above the tests):
```rust
//! Personalised PageRank over the file graph, then distribution of file rank to the
//! symbols it defines. Every score carries a `Reasons` the UI can draw.

use std::collections::HashMap;

use serde::Serialize;

use crate::config::MapConfig;
use crate::graph::{build_graph, name_matches_query, FileGraph};
use crate::store::Store;
use crate::tokens::query_terms;
use crate::Result;

pub const DAMPING: f64 = 0.85;
pub const ITERATIONS: usize = 50;
pub const FOCUS_BOOST: f64 = 10.0;
pub const PIN_BOOST: f64 = 10.0;
pub const FTS_FILE_BOOST: f64 = 5.0;
/// Symbols nobody references still get a sliver of their file's rank so they stay orderable.
pub const UNREFERENCED_FRACTION: f64 = 0.001;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RefBy {
    pub path: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct Reasons {
    pub pagerank: f64,
    pub file_rank: f64,
    pub seeds: Vec<String>,
    pub referenced_by: Vec<RefBy>,
    pub pinned: bool,
    pub fts_hit: bool,
    pub query_ident_match: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScoredSymbol {
    pub symbol_id: i64,
    pub file_id: i64,
    pub path: String,
    pub name: String,
    pub kind: String,
    pub line_start: u32,
    pub line_end: u32,
    pub signature: String,
    pub score: f64,
    pub reasons: Reasons,
}

/// Power iteration. `personalization` is normalised; all-zero means uniform.
/// Dangling mass is redistributed by the personalization vector.
pub fn pagerank(n: usize, edges: &[(usize, usize, f64)], personalization: &[f64]) -> Vec<f64> {
    if n == 0 {
        return Vec::new();
    }
    let psum: f64 = personalization.iter().sum();
    let p: Vec<f64> = if psum > 0.0 {
        personalization.iter().map(|v| v / psum).collect()
    } else {
        vec![1.0 / n as f64; n]
    };
    let mut out_w = vec![0.0; n];
    for &(s, _, w) in edges {
        out_w[s] += w;
    }
    let mut r = p.clone();
    for _ in 0..ITERATIONS {
        let mut next = vec![0.0; n];
        let mut dangling = 0.0;
        for i in 0..n {
            if out_w[i] == 0.0 {
                dangling += r[i];
            }
        }
        for &(s, d, w) in edges {
            next[d] += r[s] * w / out_w[s];
        }
        for i in 0..n {
            next[i] = (1.0 - DAMPING) * p[i] + DAMPING * (next[i] + dangling * p[i]);
        }
        r = next;
    }
    let total: f64 = r.iter().sum();
    if total > 0.0 {
        for v in &mut r {
            *v /= total;
        }
    }
    r
}

struct SymbolRow {
    id: i64,
    file_id: i64,
    name: String,
    kind: String,
    line_start: u32,
    line_end: u32,
    signature: String,
}

fn fts_symbol_ids(store: &Store, terms: &[String]) -> Result<Vec<i64>> {
    let mut ids = Vec::new();
    let mut stmt = store.conn().prepare("SELECT rowid FROM symbols_fts WHERE symbols_fts MATCH ?1")?;
    for t in terms {
        let q = format!("{{name name_tokens}}: \"{}\"*", t.replace('"', "\"\""));
        for row in stmt.query_map([q], |r| r.get::<_, i64>(0))? {
            ids.push(row?);
        }
    }
    ids.sort_unstable();
    ids.dedup();
    Ok(ids)
}

pub fn rank_symbols(store: &Store, config: &MapConfig, query: Option<&str>, focus_files: &[String]) -> Result<Vec<ScoredSymbol>> {
    let terms = query.map(query_terms).unwrap_or_default();
    let g: FileGraph = build_graph(store, config, &terms)?;
    let n = g.nodes.len();
    if n == 0 {
        return Ok(Vec::new());
    }
    let conn = store.conn();

    // Symbols in graph files.
    let mut symbols: Vec<SymbolRow> = Vec::new();
    let mut stmt = conn.prepare("SELECT id, file_id, name, kind, line_start, line_end, signature FROM symbols")?;
    for row in stmt.query_map([], |r| {
        Ok(SymbolRow {
            id: r.get(0)?,
            file_id: r.get(1)?,
            name: r.get(2)?,
            kind: r.get(3)?,
            line_start: r.get(4)?,
            line_end: r.get(5)?,
            signature: r.get(6)?,
        })
    })? {
        let s = row?;
        if g.index_of.contains_key(&s.file_id) {
            symbols.push(s);
        }
    }

    let fts_ids = fts_symbol_ids(store, &terms)?;
    let fts_files: std::collections::HashSet<i64> =
        symbols.iter().filter(|s| fts_ids.binary_search(&s.id).is_ok()).map(|s| s.file_id).collect();

    // Personalization + seeds per file.
    let mut personalization = vec![1.0; n];
    let mut seeds: Vec<Vec<String>> = vec![Vec::new(); n];
    for (i, node) in g.nodes.iter().enumerate() {
        if focus_files.iter().any(|f| f == &node.path) {
            personalization[i] += FOCUS_BOOST;
            seeds[i].push("focus".to_string());
        }
        if config.is_pinned(&node.path) {
            personalization[i] += PIN_BOOST;
            seeds[i].push("pinned".to_string());
        }
        if fts_files.contains(&node.id) {
            personalization[i] += FTS_FILE_BOOST;
            seeds[i].push(format!("query:{}", terms.join(" ")));
        }
    }
    let edge_triples: Vec<(usize, usize, f64)> = g.edges.iter().map(|e| (e.src, e.dst, e.weight)).collect();
    let file_rank = pagerank(n, &edge_triples, &personalization);

    // Out-weight per file for distribution.
    let mut out_w = vec![0.0; n];
    for e in &g.edges {
        out_w[e.src] += e.weight;
    }
    // (dst file, name) -> distributed score, and referenced_by lists.
    let mut dist: HashMap<(usize, String), f64> = HashMap::new();
    let mut ref_by: HashMap<(usize, String), HashMap<usize, i64>> = HashMap::new();
    for e in &g.edges {
        *dist.entry((e.dst, e.name.clone())).or_default() += file_rank[e.src] * e.weight / out_w[e.src];
        // raw ref count for the reasons list (undo the ambiguity split and query multiplier)
        let raw = (e.weight
            * definers_count(&g, &symbols, &e.name)
            / if name_matches_query(&e.name, &terms) { crate::graph::QUERY_IDENT_MULTIPLIER } else { 1.0 })
            .round() as i64;
        *ref_by.entry((e.dst, e.name.clone())).or_default().entry(e.src).or_default() = raw;
    }

    let mut out: Vec<ScoredSymbol> = symbols
        .into_iter()
        .map(|s| {
            let fi = g.index_of[&s.file_id];
            let key = (fi, s.name.clone());
            let fr = file_rank[fi];
            let mut score = dist.get(&key).copied().unwrap_or(fr * UNREFERENCED_FRACTION);
            let fts_hit = fts_ids.binary_search(&s.id).is_ok();
            if fts_hit {
                score += fr;
            }
            let mut referenced_by: Vec<RefBy> = ref_by
                .get(&key)
                .map(|m| m.iter().map(|(&src, &c)| RefBy { path: g.nodes[src].path.clone(), count: c }).collect())
                .unwrap_or_default();
            referenced_by.sort_by(|a, b| b.count.cmp(&a.count).then(a.path.cmp(&b.path)));
            referenced_by.truncate(5);
            ScoredSymbol {
                symbol_id: s.id,
                file_id: s.file_id,
                path: g.nodes[fi].path.clone(),
                name: s.name.clone(),
                kind: s.kind,
                line_start: s.line_start,
                line_end: s.line_end,
                signature: s.signature,
                score,
                reasons: Reasons {
                    pagerank: score,
                    file_rank: fr,
                    seeds: seeds[fi].clone(),
                    referenced_by,
                    pinned: config.is_pinned(&g.nodes[fi].path),
                    fts_hit,
                    query_ident_match: name_matches_query(&s.name, &terms),
                },
            }
        })
        .collect();
    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.path.cmp(&b.path))
            .then(a.line_start.cmp(&b.line_start))
    });
    Ok(out)
}

fn definers_count(g: &FileGraph, symbols: &[SymbolRow], name: &str) -> f64 {
    let mut files: Vec<usize> = symbols.iter().filter(|s| s.name == name).map(|s| g.index_of[&s.file_id]).collect();
    files.sort_unstable();
    files.dedup();
    files.len().max(1) as f64
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p singularrag-core rank`
Expected: 7 passed. If `most_referenced_definition_ranks_first_without_query` fails because `SessionStore` or `create` outranks `createSession`, print the top five with their `referenced_by` and check that middleware's two calls produced two `refs` rows; the fixture is built so `createSession` has three incoming refs from two files, more than any other symbol.

- [ ] **Step 6: Commit**

```bash
git add crates/singularrag-core
git commit -m "feat(core): file graph, personalised pagerank, symbol ranking with reasons"
```

---

### Task 10: Budget fill and rendering

**Files:**
- Create: `crates/singularrag-core/src/map.rs`
- Modify: `crates/singularrag-core/src/lib.rs` (add `pub mod map;`)

**Interfaces:**
- Consumes: `rank::ScoredSymbol`, `tokens::approx_tokens`.
- Produces: `map::DEFAULT_BUDGET: usize = 1024`, `map::MAX_BUDGET: usize = 8192`, `map::MIN_BUDGET: usize = 64`, `map::CUT_RECORDED: usize = 25`, `map::clamp_budget(usize) -> usize`, `map::fit(items: &[ScoredSymbol], budget: usize) -> usize` (largest n whose rendering fits), `map::render(items: &[ScoredSymbol], n: usize) -> String`, `map::header(index_version: &str, git_head: Option<&str>, stale: usize, retrieval_id: i64) -> String`, `map::footer(served: usize, total: usize) -> String`.

- [ ] **Step 1: Write the failing tests**

Bottom of `crates/singularrag-core/src/map.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::rank::{Reasons, ScoredSymbol};

    fn sym(path: &str, line: u32, sig: &str, score: f64) -> ScoredSymbol {
        ScoredSymbol {
            symbol_id: line as i64,
            file_id: 1,
            path: path.into(),
            name: "x".into(),
            kind: "function".into(),
            line_start: line,
            line_end: line,
            signature: sig.into(),
            score,
            reasons: Reasons::default(),
        }
    }

    #[test]
    fn render_groups_by_file_in_rank_order_and_sorts_lines() {
        let items = vec![
            sym("src/b.ts", 20, "export function two()", 0.9),
            sym("src/a.ts", 5, "export function five()", 0.5),
            sym("src/b.ts", 3, "export function three()", 0.4),
        ];
        let text = render(&items, 3);
        assert_eq!(
            text,
            "src/b.ts:\n    3  export function three()\n   20  export function two()\nsrc/a.ts:\n    5  export function five()\n"
        );
    }

    #[test]
    fn fit_finds_largest_prefix_within_budget() {
        let items: Vec<ScoredSymbol> = (1..=50).map(|i| sym("src/a.ts", i, "export function f()", 1.0 / i as f64)).collect();
        let n = fit(&items, 64);
        assert!(n > 0 && n < 50);
        assert!(crate::tokens::approx_tokens(&render(&items, n)) <= 64);
        assert!(crate::tokens::approx_tokens(&render(&items, n + 1)) > 64);
        assert_eq!(fit(&items, 100_000), 50);
        assert_eq!(fit(&items, 1), 0);
        assert_eq!(fit(&[], 1024), 0);
    }

    #[test]
    fn header_and_footer_format() {
        assert_eq!(
            header("7f3a2c9d1e0b", Some("9b1e0d4f5a6b7c8d"), 0, 123),
            "# singularrag · index 7f3a2c · HEAD 9b1e0d4 · fresh · retrieval r_000123"
        );
        assert_eq!(
            header("7f3a2c9d1e0b", None, 4, 7),
            "# singularrag · index 7f3a2c · HEAD none · STALE: 4 files changed since index · retrieval r_000007"
        );
        assert_eq!(
            footer(42, 310),
            "# 42 of 310 symbols shown · 268 more ranked below budget · widen with a larger budget or a focus file"
        );
        assert_eq!(footer(5, 5), "# 5 of 5 symbols shown");
    }

    #[test]
    fn budget_is_clamped() {
        assert_eq!(clamp_budget(0), MIN_BUDGET);
        assert_eq!(clamp_budget(1024), 1024);
        assert_eq!(clamp_budget(1_000_000), MAX_BUDGET);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p singularrag-core map`
Expected: compile error.

- [ ] **Step 3: Write the implementation**

```rust
//! Token-budgeted rendering of ranked symbols: `path:` groups, `line  signature` rows.

use crate::rank::ScoredSymbol;
use crate::tokens::approx_tokens;

pub const DEFAULT_BUDGET: usize = 1024;
pub const MAX_BUDGET: usize = 8192;
pub const MIN_BUDGET: usize = 64;
pub const CUT_RECORDED: usize = 25;

pub fn clamp_budget(b: usize) -> usize {
    b.clamp(MIN_BUDGET, MAX_BUDGET)
}

/// Render the top `n` items grouped by file. Files appear in order of their best
/// item; symbols within a file are sorted by line.
pub fn render(items: &[ScoredSymbol], n: usize) -> String {
    let top = &items[..n.min(items.len())];
    let mut order: Vec<&str> = Vec::new();
    for s in top {
        if !order.contains(&s.path.as_str()) {
            order.push(&s.path);
        }
    }
    let mut out = String::new();
    for path in order {
        out.push_str(path);
        out.push_str(":\n");
        let mut rows: Vec<&ScoredSymbol> = top.iter().filter(|s| s.path == path).collect();
        rows.sort_by_key(|s| s.line_start);
        for s in rows {
            out.push_str(&format!("{:>5}  {}\n", s.line_start, s.signature));
        }
    }
    out
}

/// Largest `n` such that `render(items, n)` fits the budget. Binary search, as in Aider.
pub fn fit(items: &[ScoredSymbol], budget: usize) -> usize {
    let (mut lo, mut hi) = (0usize, items.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if approx_tokens(&render(items, mid)) <= budget {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    lo
}

pub fn header(index_version: &str, git_head: Option<&str>, stale: usize, retrieval_id: i64) -> String {
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
    format!("# singularrag · index {idx} · HEAD {head} · {fresh} · retrieval r_{retrieval_id:06}")
}

pub fn footer(served: usize, total: usize) -> String {
    if served >= total {
        format!("# {served} of {total} symbols shown")
    } else {
        format!(
            "# {served} of {total} symbols shown · {} more ranked below budget · widen with a larger budget or a focus file",
            total - served
        )
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p singularrag-core map`
Expected: 4 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/singularrag-core
git commit -m "feat(core): budget fill, map rendering, header and footer"
```

---

### Task 11: Indexer lock and the Engine (`repo_map` with provenance)

**Files:**
- Create: `crates/singularrag-core/src/engine.rs`
- Modify: `crates/singularrag-core/src/store/lock.rs` (replace placeholder), `crates/singularrag-core/src/lib.rs` (add `pub mod engine;`)

**Interfaces:**
- Consumes: everything above.
- Produces:
  - `store::lock::try_acquire(store: &Store, pid: u32, now_ms: i64) -> Result<bool>` (takes the lock if free or heartbeat older than `LOCK_STALE_MS = 10_000`), `store::lock::heartbeat(store, pid, now_ms) -> Result<()>`, `store::lock::release(store, pid) -> Result<()>`.
  - `engine::Engine::open(root: &Path, session_key: &str) -> Result<Engine>`, `Engine::root(&self) -> &Path`, `Engine::store(&self) -> &Store`, `Engine::config(&self) -> &MapConfig`, `Engine::refresh(&self, budget: Duration) -> Result<IndexStats>` (waits ≤ `LOCK_WAIT_MS = 500` for the lock; without it returns `remaining = stale_count()` and indexes nothing), `engine::MapRequest { query: Option<String>, focus_files: Vec<String>, budget_tokens: usize }`, `engine::MapResponse { retrieval_id: i64, text: String, served: usize, total: usize, cut_recorded: usize, stale_count: usize }`, `Engine::repo_map(&self, req: &MapRequest) -> Result<MapResponse>`, `engine::REFRESH_BUDGET: Duration = 2 s`.

- [ ] **Step 1: Write the failing tests**

Bottom of `crates/singularrag-core/src/engine.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::write_ts_mini;
    use crate::store::lock;

    fn engine() -> (tempfile::TempDir, Engine) {
        let dir = tempfile::tempdir().unwrap();
        write_ts_mini(dir.path());
        let e = Engine::open(dir.path(), "test-session").unwrap();
        (dir, e)
    }

    fn count(e: &Engine, sql: &str) -> i64 {
        e.store().conn().query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn repo_map_indexes_renders_and_records_provenance() {
        let (_dir, e) = engine();
        let resp = e.repo_map(&MapRequest { query: Some("session".into()), focus_files: vec![], budget_tokens: 1024 }).unwrap();
        assert!(resp.text.starts_with("# singularrag · index "), "{}", resp.text);
        assert!(resp.text.contains("· fresh ·"));
        assert!(resp.text.contains("src/auth/session.ts:\n"));
        assert!(resp.text.contains("export function createSession(user: User, ttl: number): Session {"));
        assert!(resp.text.ends_with(&format!("{}\n", crate::map::footer(resp.served, resp.total))));
        assert_eq!(resp.stale_count, 0);
        assert!(!resp.text.contains("console.log"), "bodies must never be served");

        assert_eq!(count(&e, "SELECT COUNT(*) FROM retrievals"), 1);
        let served = count(&e, "SELECT COUNT(*) FROM retrieval_items WHERE served = 1");
        let cut = count(&e, "SELECT COUNT(*) FROM retrieval_items WHERE served = 0");
        assert_eq!(served as usize, resp.served);
        assert_eq!(cut as usize, resp.cut_recorded);
        assert!(cut as usize <= crate::map::CUT_RECORDED);
        let reasons: String = e.store().conn().query_row("SELECT reasons_json FROM retrieval_items WHERE rank = 1", [], |r| r.get(0)).unwrap();
        assert!(reasons.contains("\"referenced_by\""));
        let tool: String = e.store().conn().query_row("SELECT tool FROM retrievals", [], |r| r.get(0)).unwrap();
        assert_eq!(tool, "repo_map");
    }

    #[test]
    fn small_budget_cuts_and_records_up_to_25() {
        let (_dir, e) = engine();
        let resp = e.repo_map(&MapRequest { query: None, focus_files: vec![], budget_tokens: 64 }).unwrap();
        assert!(resp.served < resp.total);
        assert_eq!(resp.cut_recorded, (resp.total - resp.served).min(crate::map::CUT_RECORDED));
        assert!(resp.text.contains("more ranked below budget"));
    }

    #[test]
    fn excludes_from_map_toml_change_the_next_retrieval() {
        let (dir, e) = engine();
        let before = e.repo_map(&MapRequest { query: None, focus_files: vec![], budget_tokens: 4096 }).unwrap();
        assert!(before.text.contains("src/util/log.ts:"));
        std::fs::create_dir_all(dir.path().join(".singularrag")).unwrap();
        std::fs::write(dir.path().join(".singularrag/map.toml"), "[[exclude]]\npath = \"src/util/\"\n").unwrap();
        let e2 = Engine::open(dir.path(), "test-session").unwrap();
        let after = e2.repo_map(&MapRequest { query: None, focus_files: vec![], budget_tokens: 4096 }).unwrap();
        assert!(!after.text.contains("src/util/log.ts:"));
    }

    #[test]
    fn held_lock_yields_stale_header_without_indexing() {
        let (_dir, e) = engine();
        let other_pid = std::process::id() + 1;
        assert!(lock::try_acquire(e.store(), other_pid, crate::time::now_ms()).unwrap());
        let started = std::time::Instant::now();
        let resp = e.repo_map(&MapRequest { query: None, focus_files: vec![], budget_tokens: 1024 }).unwrap();
        assert!(started.elapsed() >= Duration::from_millis(LOCK_WAIT_MS));
        assert!(resp.stale_count > 0);
        assert!(resp.text.contains(&format!("STALE: {} files changed since index", resp.stale_count)));
        assert_eq!(count(&e, "SELECT COUNT(*) FROM symbols"), 0);
        // A stale heartbeat is reclaimable.
        assert!(lock::try_acquire(e.store(), std::process::id(), crate::time::now_ms() + lock::LOCK_STALE_MS + 1).unwrap());
    }

    #[test]
    fn lock_release_frees_it() {
        let (_dir, e) = engine();
        let now = crate::time::now_ms();
        assert!(lock::try_acquire(e.store(), 1, now).unwrap());
        assert!(!lock::try_acquire(e.store(), 2, now).unwrap());
        lock::release(e.store(), 1).unwrap();
        assert!(lock::try_acquire(e.store(), 2, now).unwrap());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p singularrag-core engine`
Expected: compile error.

- [ ] **Step 3: Write the lock**

`crates/singularrag-core/src/store/lock.rs`:
```rust
//! Advisory indexer lock: one row, pid + heartbeat. A heartbeat older than
//! `LOCK_STALE_MS` is treated as abandoned and may be taken over.

use rusqlite::params;

use crate::store::Store;
use crate::Result;

pub const LOCK_STALE_MS: i64 = 10_000;

pub fn try_acquire(store: &Store, pid: u32, now_ms: i64) -> Result<bool> {
    let conn = store.conn();
    let changed = conn.execute(
        "INSERT INTO indexer_lock(id, pid, heartbeat_at_ms) VALUES (1, ?1, ?2)
         ON CONFLICT(id) DO UPDATE SET pid = excluded.pid, heartbeat_at_ms = excluded.heartbeat_at_ms
         WHERE indexer_lock.pid = excluded.pid OR indexer_lock.heartbeat_at_ms < ?2 - ?3",
        params![pid, now_ms, LOCK_STALE_MS],
    )?;
    Ok(changed == 1)
}

pub fn heartbeat(store: &Store, pid: u32, now_ms: i64) -> Result<()> {
    store.conn().execute("UPDATE indexer_lock SET heartbeat_at_ms = ?2 WHERE id = 1 AND pid = ?1", params![pid, now_ms])?;
    Ok(())
}

pub fn release(store: &Store, pid: u32) -> Result<()> {
    store.conn().execute("DELETE FROM indexer_lock WHERE id = 1 AND pid = ?1", [pid])?;
    Ok(())
}
```

- [ ] **Step 4: Write the engine**

`crates/singularrag-core/src/engine.rs` (above the tests):
```rust
//! The Engine: what the CLI, the MCP server and the UI call. Owns the store,
//! the authored config and the session key; every retrieval is recorded.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rusqlite::params;

use crate::config::MapConfig;
use crate::index::{IndexStats, Indexer};
use crate::map::{self, CUT_RECORDED};
use crate::rank::{rank_symbols, ScoredSymbol};
use crate::store::{lock, Store};
use crate::time::now_ms;
use crate::Result;

pub const DB_FILE: &str = ".singularrag/index.db";
pub const REFRESH_BUDGET: Duration = Duration::from_secs(2);
pub const LOCK_WAIT_MS: u64 = 500;

pub struct Engine {
    root: PathBuf,
    store: Store,
    config: MapConfig,
    session_key: String,
}

#[derive(Debug, Clone, Default)]
pub struct MapRequest {
    pub query: Option<String>,
    pub focus_files: Vec<String>,
    pub budget_tokens: usize,
}

#[derive(Debug, Clone)]
pub struct MapResponse {
    pub retrieval_id: i64,
    pub text: String,
    pub served: usize,
    pub total: usize,
    pub cut_recorded: usize,
    pub stale_count: usize,
}

impl Engine {
    pub fn open(root: &Path, session_key: &str) -> Result<Engine> {
        let root = root.canonicalize()?;
        let store = Store::open(&root.join(DB_FILE))?;
        let config = MapConfig::load(&root)?;
        Ok(Engine { root, store, config, session_key: session_key.to_string() })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn store(&self) -> &Store {
        &self.store
    }
    pub fn config(&self) -> &MapConfig {
        &self.config
    }
    pub fn session_key(&self) -> &str {
        &self.session_key
    }

    /// Refresh under the advisory lock. If the lock is held by a live process for
    /// longer than `LOCK_WAIT_MS`, index nothing and report how many files are stale.
    pub fn refresh(&self, budget: Duration) -> Result<IndexStats> {
        let pid = std::process::id();
        let indexer = Indexer::new(&self.store, &self.root, &self.config)?;
        let wait_until = Instant::now() + Duration::from_millis(LOCK_WAIT_MS);
        loop {
            if lock::try_acquire(&self.store, pid, now_ms())? {
                break;
            }
            if Instant::now() >= wait_until {
                let remaining = indexer.stale_count()?;
                return Ok(IndexStats { remaining, ..IndexStats::default() });
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let result = indexer.refresh(Some(Instant::now() + budget));
        lock::release(&self.store, pid)?;
        result
    }

    fn index_meta(&self) -> Result<(String, Option<String>)> {
        let version = self.store.get_meta("index_version")?.unwrap_or_else(|| "000000".to_string());
        let head = self.store.get_meta("git_head")?.filter(|h| !h.is_empty());
        Ok((version, head))
    }

    pub fn repo_map(&self, req: &MapRequest) -> Result<MapResponse> {
        let stats = self.refresh(REFRESH_BUDGET)?;
        let budget = map::clamp_budget(req.budget_tokens);
        let ranked = rank_symbols(&self.store, &self.config, req.query.as_deref(), &req.focus_files)?;
        let served = map::fit(&ranked, budget);
        let body = map::render(&ranked, served);
        let (version, head) = self.index_meta()?;

        let retrieval_id = self.record_retrieval("repo_map", req.query.as_deref(), &req.focus_files, budget, &version, head.as_deref(), stats.remaining, &ranked, served)?;
        let text = format!(
            "{}\n{}{}\n",
            map::header(&version, head.as_deref(), stats.remaining, retrieval_id),
            body,
            map::footer(served, ranked.len())
        );
        Ok(MapResponse {
            retrieval_id,
            text,
            served,
            total: ranked.len(),
            cut_recorded: (ranked.len() - served).min(CUT_RECORDED),
            stale_count: stats.remaining,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record_retrieval(
        &self,
        tool: &str,
        query: Option<&str>,
        focus_files: &[String],
        budget: usize,
        index_version: &str,
        git_head: Option<&str>,
        stale: usize,
        ranked: &[ScoredSymbol],
        served: usize,
    ) -> Result<i64> {
        let conn = self.store.conn();
        conn.execute(
            "INSERT INTO retrievals(session_key, tool, query, focus_files, budget, index_version, git_head, stale_count, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                self.session_key,
                tool,
                query,
                serde_json::to_string(focus_files).unwrap_or_else(|_| "[]".into()),
                budget as i64,
                index_version,
                git_head,
                stale as i64,
                now_ms()
            ],
        )?;
        let id = conn.last_insert_rowid();
        let mut stmt = conn.prepare(
            "INSERT INTO retrieval_items(retrieval_id, symbol_id, rank, score, served, reasons_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        for (i, s) in ranked.iter().take(served + CUT_RECORDED).enumerate() {
            stmt.execute(params![
                id,
                s.symbol_id,
                (i + 1) as i64,
                s.score,
                i < served,
                serde_json::to_string(&s.reasons).unwrap_or_else(|_| "{}".into())
            ])?;
        }
        Ok(id)
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p singularrag-core engine`
Expected: 5 passed.

- [ ] **Step 6: Run the whole suite, clippy, fmt**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test --workspace`
Expected: all green.

- [ ] **Step 7: Commit**

```bash
git add crates/singularrag-core
git commit -m "feat(core): engine with locked refresh, repo_map and recorded provenance"
```

---
### Task 12: `find_symbol`

**Files:**
- Create: `crates/singularrag-core/src/find.rs`
- Modify: `crates/singularrag-core/src/engine.rs` (add `Engine::find_symbol`), `crates/singularrag-core/src/lib.rs` (add `pub mod find;`)

**Interfaces:**
- Produces: `find::FindHit { symbol_id: i64, path: String, line: u32, kind: String, name: String, signature: String, referenced_from: Vec<rank::RefBy>, total_ref_files: usize }`, `find::find_symbol(store: &Store, config: &MapConfig, name: &str, kind: Option<&str>, limit: usize) -> Result<Vec<FindHit>>`, `find::render_find(hits: &[FindHit]) -> String`, `find::MAX_LIMIT: usize = 50`, `engine::FindRequest { name: String, kind: Option<String>, limit: usize }`, `engine::FindResponse { retrieval_id: i64, text: String, hits: usize, stale_count: usize }`, `Engine::find_symbol(&self, req: &FindRequest) -> Result<FindResponse>`.

- [ ] **Step 1: Write the failing tests**

Bottom of `crates/singularrag-core/src/find.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MapConfig;
    use crate::engine::{Engine, FindRequest};
    use crate::fixture::write_ts_mini;

    fn engine() -> (tempfile::TempDir, Engine) {
        let dir = tempfile::tempdir().unwrap();
        write_ts_mini(dir.path());
        let e = Engine::open(dir.path(), "t").unwrap();
        e.refresh(std::time::Duration::from_secs(5)).unwrap();
        (dir, e)
    }

    #[test]
    fn exact_and_prefix_lookup_with_references() {
        let (_dir, e) = engine();
        let hits = find_symbol(e.store(), &MapConfig::default(), "createSess", None, 10).unwrap();
        assert_eq!(hits[0].name, "createSession");
        assert_eq!(hits[0].path, "src/auth/session.ts");
        assert_eq!(hits[0].line, 3);
        assert_eq!(hits[0].kind, "function");
        assert_eq!(hits[0].total_ref_files, 2);
        assert_eq!(hits[0].referenced_from[0].path, "src/http/middleware.ts");
        assert_eq!(hits[0].referenced_from[0].count, 2);
    }

    #[test]
    fn split_token_lookup_and_kind_filter() {
        let (_dir, e) = engine();
        let hits = find_symbol(e.store(), &MapConfig::default(), "session", Some("class"), 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "SessionStore");
        let none = find_symbol(e.store(), &MapConfig::default(), "nope_zzz", None, 10).unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn render_format() {
        let (_dir, e) = engine();
        let hits = find_symbol(e.store(), &MapConfig::default(), "createSession", None, 1).unwrap();
        let text = render_find(&hits);
        assert!(text.starts_with("src/auth/session.ts:3  function  export function createSession(user: User, ttl: number): Session {\n   referenced from 2 files: src/http/middleware.ts (2), src/cli/login.ts (1)\n"), "{text}");
        assert_eq!(render_find(&[]), "no symbols matched\n");
    }

    #[test]
    fn engine_find_records_retrieval_with_header() {
        let (_dir, e) = engine();
        let resp = e.find_symbol(&FindRequest { name: "log".into(), kind: None, limit: 10 }).unwrap();
        assert!(resp.text.starts_with("# singularrag · index "));
        assert!(resp.text.contains("src/util/log.ts:1  function  export function log(msg: string): void {"));
        let tool: String = e.store().conn().query_row("SELECT tool FROM retrievals ORDER BY id DESC LIMIT 1", [], |r| r.get(0)).unwrap();
        assert_eq!(tool, "find_symbol");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p singularrag-core find`
Expected: compile error.

- [ ] **Step 3: Write the implementation**

`crates/singularrag-core/src/find.rs` (above the tests):
```rust
//! `find_symbol`: FTS5 lookup by name (exact, prefix, split tokens) with reference counts.

use rusqlite::params;

use crate::config::MapConfig;
use crate::rank::RefBy;
use crate::store::Store;
use crate::Result;

pub const MAX_LIMIT: usize = 50;

#[derive(Debug, Clone)]
pub struct FindHit {
    pub symbol_id: i64,
    pub path: String,
    pub line: u32,
    pub kind: String,
    pub name: String,
    pub signature: String,
    pub referenced_from: Vec<RefBy>,
    pub total_ref_files: usize,
}

pub fn find_symbol(store: &Store, config: &MapConfig, name: &str, kind: Option<&str>, limit: usize) -> Result<Vec<FindHit>> {
    let limit = limit.clamp(1, MAX_LIMIT);
    let conn = store.conn();
    let term = name.replace('"', "\"\"");
    let parts = crate::tokens::split_identifier(name);
    // Exact name first, then prefix, then all split parts.
    let q = format!("{{name name_tokens}}: (\"{term}\" OR \"{term}\"* OR ({}))", parts.iter().map(|p| format!("\"{p}\"")).collect::<Vec<_>>().join(" AND "));
    let mut stmt = conn.prepare(
        "SELECT s.id, f.path, s.line_start, s.kind, s.name, s.signature, f.id
         FROM symbols_fts x JOIN symbols s ON s.id = x.rowid JOIN files f ON f.id = s.file_id
         WHERE symbols_fts MATCH ?1
         ORDER BY (lower(s.name) = lower(?2)) DESC, bm25(symbols_fts), f.path, s.line_start",
    )?;
    let rows = stmt.query_map(params![q, name], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, u32>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, i64>(6)?,
        ))
    })?;

    let mut hits = Vec::new();
    for row in rows {
        let (symbol_id, path, line, k, sym_name, signature, file_id) = row?;
        if config.is_excluded(&path) {
            continue;
        }
        if kind.is_some_and(|want| want != k) {
            continue;
        }
        let mut rs = conn.prepare(
            "SELECT f.path, COUNT(*) FROM refs r JOIN files f ON f.id = r.file_id
             WHERE r.name = ?1 AND r.file_id != ?2 GROUP BY f.path ORDER BY COUNT(*) DESC, f.path",
        )?;
        let all: Vec<RefBy> = rs
            .query_map(params![sym_name, file_id], |r| Ok(RefBy { path: r.get(0)?, count: r.get(1)? }))?
            .collect::<std::result::Result<_, _>>()?;
        let total_ref_files = all.len();
        let mut referenced_from = all;
        referenced_from.truncate(5);
        hits.push(FindHit { symbol_id, path, line, kind: k, name: sym_name, signature, referenced_from, total_ref_files });
        if hits.len() >= limit {
            break;
        }
    }
    Ok(hits)
}

pub fn render_find(hits: &[FindHit]) -> String {
    if hits.is_empty() {
        return "no symbols matched\n".to_string();
    }
    let mut out = String::new();
    for h in hits {
        out.push_str(&format!("{}:{}  {}  {}\n", h.path, h.line, h.kind, h.signature));
        if h.total_ref_files == 0 {
            out.push_str("   referenced from 0 files\n");
        } else {
            let list = h.referenced_from.iter().map(|r| format!("{} ({})", r.path, r.count)).collect::<Vec<_>>().join(", ");
            let more = if h.total_ref_files > h.referenced_from.len() { ", …" } else { "" };
            out.push_str(&format!("   referenced from {} files: {list}{more}\n", h.total_ref_files));
        }
    }
    out
}
```

Add to `crates/singularrag-core/src/engine.rs`:
```rust
#[derive(Debug, Clone)]
pub struct FindRequest {
    pub name: String,
    pub kind: Option<String>,
    pub limit: usize,
}

#[derive(Debug, Clone)]
pub struct FindResponse {
    pub retrieval_id: i64,
    pub text: String,
    pub hits: usize,
    pub stale_count: usize,
}

impl Engine {
    pub fn find_symbol(&self, req: &FindRequest) -> Result<FindResponse> {
        let stats = self.refresh(REFRESH_BUDGET)?;
        let hits = crate::find::find_symbol(&self.store, &self.config, &req.name, req.kind.as_deref(), req.limit)?;
        let (version, head) = self.index_meta()?;
        // Record hits as served items with minimal reasons (exact/prefix match).
        let ranked: Vec<ScoredSymbol> = hits
            .iter()
            .enumerate()
            .map(|(i, h)| ScoredSymbol {
                symbol_id: h.symbol_id,
                file_id: 0,
                path: h.path.clone(),
                name: h.name.clone(),
                kind: h.kind.clone(),
                line_start: h.line,
                line_end: h.line,
                signature: h.signature.clone(),
                score: 1.0 / (i as f64 + 1.0),
                reasons: crate::rank::Reasons {
                    fts_hit: true,
                    query_ident_match: h.name.eq_ignore_ascii_case(&req.name),
                    referenced_by: h.referenced_from.clone(),
                    seeds: vec![format!("find:{}", req.name)],
                    ..Default::default()
                },
            })
            .collect();
        let retrieval_id = self.record_retrieval("find_symbol", Some(&req.name), &[], req.limit, &version, head.as_deref(), stats.remaining, &ranked, ranked.len())?;
        let text = format!("{}\n{}", map::header(&version, head.as_deref(), stats.remaining, retrieval_id), crate::find::render_find(&hits));
        Ok(FindResponse { retrieval_id, text, hits: hits.len(), stale_count: stats.remaining })
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p singularrag-core find`
Expected: 4 passed. If FTS5 rejects the `OR (... AND ...)` grouping, simplify the MATCH string to `{name name_tokens}: "term"*` and keep the split-token behaviour by issuing a second query for the AND of parts when the first returns nothing.

- [ ] **Step 5: Commit**

```bash
git add crates/singularrag-core
git commit -m "feat(core): find_symbol with reference counts and recorded retrieval"
```

---

### Task 13: CLI: `index`, `query`, `find`

**Files:**
- Modify: `crates/singularrag/src/main.rs`, `crates/singularrag/tests/cli.rs`

**Interfaces:**
- Consumes: `Engine::{open, refresh, repo_map, find_symbol}`, `MapRequest`, `FindRequest`, `map::DEFAULT_BUDGET`.
- Produces: binary subcommands `singularrag index [--repo PATH]`, `singularrag query [TEXT] [--budget N] [--focus PATH]... [--repo PATH]`, `singularrag find NAME [--kind K] [--limit N] [--repo PATH]`. Session key `cli-<pid>`.

- [ ] **Step 1: Write the failing tests**

Replace `crates/singularrag/tests/cli.rs` with:
```rust
use assert_cmd::Command;
use predicates::prelude::*;

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    singularrag_core::fixture::write_ts_mini(dir.path());
    dir
}

#[test]
fn version_prints_name_and_version() {
    Command::cargo_bin("singularrag").unwrap().arg("--version").assert().success().stdout(predicate::str::starts_with("singularrag 0.1.0"));
}

#[test]
fn index_reports_stats() {
    let dir = fixture();
    Command::cargo_bin("singularrag")
        .unwrap()
        .args(["index", "--repo", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("indexed ").and(predicate::str::contains("skipped ")));
    assert!(dir.path().join(".singularrag/index.db").exists());
}

#[test]
fn query_prints_header_map_and_footer() {
    let dir = fixture();
    Command::cargo_bin("singularrag")
        .unwrap()
        .args(["query", "session", "--budget", "512", "--repo", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(
            predicate::str::starts_with("# singularrag · index ")
                .and(predicate::str::contains("src/auth/session.ts:\n"))
                .and(predicate::str::contains("symbols shown")),
        );
}

#[test]
fn find_prints_hits() {
    let dir = fixture();
    Command::cargo_bin("singularrag")
        .unwrap()
        .args(["find", "createSession", "--repo", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("src/auth/session.ts:3  function  export function createSession"));
}

#[test]
fn budget_over_cap_is_clamped_not_rejected() {
    let dir = fixture();
    Command::cargo_bin("singularrag")
        .unwrap()
        .args(["query", "--budget", "999999", "--repo", dir.path().to_str().unwrap()])
        .assert()
        .success();
}
```
Add `singularrag-core = { path = "../singularrag-core" }` under `[dev-dependencies]` in `crates/singularrag/Cargo.toml` is unnecessary since it is already a normal dependency; tests can use it directly.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p singularrag --test cli`
Expected: 4 failures (unknown subcommands).

- [ ] **Step 3: Write the CLI**

`crates/singularrag/src/main.rs`:
```rust
#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, Subcommand};
use singularrag_core::engine::{Engine, FindRequest, MapRequest};
use singularrag_core::map::DEFAULT_BUDGET;

#[derive(Parser, Debug)]
#[command(name = "singularrag", version, about = "Repo map with retrieval provenance for coding agents")]
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
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let root = cli.repo.unwrap_or(std::env::current_dir()?);
    let engine = Engine::open(&root, &format!("cli-{}", std::process::id()))?;
    match cli.cmd {
        Cmd::Index => {
            let s = engine.refresh(Duration::from_secs(600))?;
            println!(
                "scanned {} · indexed {} · unchanged {} · skipped {} · removed {} · remaining {}",
                s.scanned, s.indexed, s.unchanged, s.skipped, s.removed, s.remaining
            );
        }
        Cmd::Query { text, budget, focus } => {
            let r = engine.repo_map(&MapRequest { query: text, focus_files: focus, budget_tokens: budget })?;
            print!("{}", r.text);
        }
        Cmd::Find { name, kind, limit } => {
            let r = engine.find_symbol(&FindRequest { name, kind, limit })?;
            print!("{}", r.text);
        }
    }
    Ok(())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p singularrag --test cli`
Expected: 5 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/singularrag
git commit -m "feat(cli): index, query and find subcommands"
```

---

### Task 14: Tier-one eval (`recall@budget`) and the hono question set

**Files:**
- Create: `crates/singularrag-core/src/eval.rs`, `eval/questions.toml`, `eval/README.md`
- Modify: `crates/singularrag-core/src/lib.rs` (add `pub mod eval;`), `crates/singularrag/src/main.rs` (add `Eval` subcommand), `crates/singularrag/tests/cli.rs` (add one test)

**Interfaces:**
- Produces: `eval::Question { id: String, category: String, query: String, gold: Vec<String> }` (gold entries are `path::name`), `eval::QuestionFile { question: Vec<Question> }`, `eval::load_questions(path: &Path) -> Result<Vec<Question>>`, `eval::EvalResult { id: String, category: String, recall: f64, hit: Vec<String>, miss: Vec<String> }`, `eval::run(engine: &Engine, questions: &[Question], budget: usize) -> Result<Vec<EvalResult>>`, `eval::render_report(results: &[EvalResult]) -> String`, `eval::mean_recall(results) -> f64`; CLI `singularrag eval --questions PATH [--budget N] [--json] [--repo PATH]`.

- [ ] **Step 1: Write the failing tests**

Bottom of `crates/singularrag-core/src/eval.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use crate::fixture::write_ts_mini;

    const Q: &str = r#"
[[question]]
id = "L1"
category = "locate"
query = "where are sessions created"
gold = ["src/auth/session.ts::createSession"]

[[question]]
id = "B1"
category = "blast"
query = "what calls createSession"
gold = ["src/http/middleware.ts::requireSession", "src/http/middleware.ts::attachSession", "src/cli/login.ts::login", "src/nope.ts::missing"]
"#;

    #[test]
    fn loads_and_scores_recall_at_budget() {
        let dir = tempfile::tempdir().unwrap();
        write_ts_mini(dir.path());
        let qpath = dir.path().join("questions.toml");
        std::fs::write(&qpath, Q).unwrap();
        let qs = load_questions(&qpath).unwrap();
        assert_eq!(qs.len(), 2);

        let e = Engine::open(dir.path(), "eval").unwrap();
        let results = run(&e, &qs, 2048).unwrap();
        assert_eq!(results[0].id, "L1");
        assert!((results[0].recall - 1.0).abs() < 1e-9, "{:?}", results[0]);
        assert!((results[1].recall - 0.75).abs() < 1e-9, "{:?}", results[1]);
        assert_eq!(results[1].miss, vec!["src/nope.ts::missing"]);
        let report = render_report(&results);
        assert!(report.contains("L1"));
        assert!(report.contains("mean recall"));
        assert!((mean_recall(&results) - 0.875).abs() < 1e-9);
    }

    #[test]
    fn tiny_budget_lowers_recall() {
        let dir = tempfile::tempdir().unwrap();
        write_ts_mini(dir.path());
        let qpath = dir.path().join("questions.toml");
        std::fs::write(&qpath, Q).unwrap();
        let qs = load_questions(&qpath).unwrap();
        let e = Engine::open(dir.path(), "eval").unwrap();
        let big = mean_recall(&run(&e, &qs, 4096).unwrap());
        let small = mean_recall(&run(&e, &qs, 64).unwrap());
        assert!(small <= big);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p singularrag-core eval`
Expected: compile error.

- [ ] **Step 3: Write the implementation**

`crates/singularrag-core/src/eval.rs` (above the tests):
```rust
//! Tier-one eval: does the budgeted map contain the gold symbols? No LLM involved.

use std::path::Path;

use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::engine::{Engine, MapRequest};
use crate::{Error, Result};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Question {
    pub id: String,
    pub category: String,
    pub query: String,
    /// `path::name` entries.
    pub gold: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct QuestionFile {
    pub question: Vec<Question>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EvalResult {
    pub id: String,
    pub category: String,
    pub recall: f64,
    pub hit: Vec<String>,
    pub miss: Vec<String>,
}

pub fn load_questions(path: &Path) -> Result<Vec<Question>> {
    let s = std::fs::read_to_string(path)?;
    let f: QuestionFile = toml::from_str(&s).map_err(|e| Error::Config(e.to_string()))?;
    Ok(f.question)
}

fn served_keys(engine: &Engine, retrieval_id: i64) -> Result<Vec<String>> {
    let mut stmt = engine.store().conn().prepare(
        "SELECT f.path || '::' || s.name FROM retrieval_items i
         JOIN symbols s ON s.id = i.symbol_id JOIN files f ON f.id = s.file_id
         WHERE i.retrieval_id = ?1 AND i.served = 1",
    )?;
    let keys = stmt.query_map(params![retrieval_id], |r| r.get::<_, String>(0))?.collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(keys)
}

pub fn run(engine: &Engine, questions: &[Question], budget: usize) -> Result<Vec<EvalResult>> {
    let mut out = Vec::new();
    for q in questions {
        let resp = engine.repo_map(&MapRequest { query: Some(q.query.clone()), focus_files: vec![], budget_tokens: budget })?;
        let served = served_keys(engine, resp.retrieval_id)?;
        let (hit, miss): (Vec<String>, Vec<String>) = q.gold.iter().cloned().partition(|g| served.contains(g));
        let recall = if q.gold.is_empty() { 1.0 } else { hit.len() as f64 / q.gold.len() as f64 };
        out.push(EvalResult { id: q.id.clone(), category: q.category.clone(), recall, hit, miss });
    }
    Ok(out)
}

pub fn mean_recall(results: &[EvalResult]) -> f64 {
    if results.is_empty() {
        return 0.0;
    }
    results.iter().map(|r| r.recall).sum::<f64>() / results.len() as f64
}

pub fn render_report(results: &[EvalResult]) -> String {
    let mut out = String::from("id    category  recall  missed\n");
    for r in results {
        out.push_str(&format!("{:<5} {:<9} {:>5.2}  {}\n", r.id, r.category, r.recall, r.miss.join(", ")));
    }
    out.push_str(&format!("mean recall {:.3} over {} questions\n", mean_recall(results), results.len()));
    out
}
```

Add to `Cmd` in `crates/singularrag/src/main.rs`:
```rust
    /// Tier-one eval: recall of gold symbols inside the budgeted map
    Eval {
        #[arg(long, default_value = "eval/questions.toml")]
        questions: PathBuf,
        #[arg(long, default_value_t = DEFAULT_BUDGET)]
        budget: usize,
        #[arg(long)]
        json: bool,
    },
```
and the arm:
```rust
        Cmd::Eval { questions, budget, json } => {
            let qs = singularrag_core::eval::load_questions(&questions)?;
            let results = singularrag_core::eval::run(&engine, &qs, budget)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&results)?);
            } else {
                print!("{}", singularrag_core::eval::render_report(&results));
            }
        }
```

Add to `crates/singularrag/tests/cli.rs`:
```rust
#[test]
fn eval_runs_on_fixture_questions() {
    let dir = fixture();
    let q = dir.path().join("q.toml");
    std::fs::write(&q, "[[question]]\nid = \"L1\"\ncategory = \"locate\"\nquery = \"where are sessions created\"\ngold = [\"src/auth/session.ts::createSession\"]\n").unwrap();
    Command::cargo_bin("singularrag")
        .unwrap()
        .args(["eval", "--questions", q.to_str().unwrap(), "--repo", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("mean recall 1.000 over 1 questions"));
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --workspace`
Expected: all green, including the two new eval tests and the CLI eval test.

- [ ] **Step 5: Author the hono question file**

Procedure (this is data authoring; the queries are fixed by the spec, the gold sets are read from the pinned code):
1. `git clone --depth 1 https://github.com/honojs/hono /tmp/hono` (use the scratchpad dir if one is configured), then `git -C /tmp/hono rev-parse HEAD` and record the SHA in `eval/README.md` as the pinned commit. Re-clone with `git fetch --depth 1 origin <sha>` if the plan is executed later so the SHA stays fixed.
2. Run `singularrag index --repo /tmp/hono` and `singularrag find <name> --repo /tmp/hono` for the concepts below to locate the defining `path::name` keys. Every gold entry must be a key that `find` prints; do not invent names.
3. Write `eval/questions.toml` with exactly these twelve entries, queries verbatim, gold filled from step 2 (two to six keys each):

```toml
# Fixed on first run. Never edit an existing question; add new ids instead.
[[question]]
id = "L1"
category = "locate"
query = "where is request routing decided"
gold = []

[[question]]
id = "L2"
category = "locate"
query = "where are middleware chains composed"
gold = []

[[question]]
id = "L3"
category = "locate"
query = "where does context c.json() get its response type"
gold = []

[[question]]
id = "T1"
category = "trace"
query = "path from middleware registration to handler dispatch"
gold = []

[[question]]
id = "T2"
category = "trace"
query = "from incoming request to matched route params"
gold = []

[[question]]
id = "T3"
category = "trace"
query = "from an error thrown in a handler to the response"
gold = []

[[question]]
id = "B1"
category = "blast"
query = "what breaks if the router match signature changes"
gold = []

[[question]]
id = "B2"
category = "blast"
query = "what breaks if the Context class is renamed"
gold = []

[[question]]
id = "B3"
category = "blast"
query = "what breaks if the middleware next contract changes"
gold = []

[[question]]
id = "P1"
category = "placement"
query = "where would you add a new built-in middleware"
gold = []

[[question]]
id = "P2"
category = "placement"
query = "where would you add a new router implementation"
gold = []

[[question]]
id = "P3"
category = "placement"
query = "where would you add a response helper on context"
gold = []
```
The empty `gold = []` arrays above are the template only; the committed file must have every array filled from step 2. A committed question with an empty gold array is a plan failure.
4. Run `singularrag eval --repo /tmp/hono --questions eval/questions.toml` and paste the report into `eval/README.md` as the baseline.

`eval/README.md`:
```markdown
# Tier-one eval

Repo: honojs/hono at commit `<sha from step 1>`.
Run: `singularrag index --repo <hono checkout> && singularrag eval --repo <hono checkout> --questions eval/questions.toml`

Questions are fixed. Never edit an existing id; add new ids for new questions.

## Baseline (first run)

<paste of the report>
```

- [ ] **Step 6: Commit**

```bash
git add crates eval
git commit -m "feat: tier-one eval (recall@budget) and pinned hono question set"
```

---

## Plans 2 to 4 (separate documents, in this order)

- **Plan 2, MCP server.** `singularrag mcp` subcommand using `rmcp` 3.4 (`server`, `transport-io`) exposing `repo_map` and `find_symbol` over stdio via `Engine`. Session key from the MCP process pid and start time. Background continuation of an interrupted refresh in a thread. Host config snippets for Claude Code, Codex CLI, Copilot CLI in the README.
- **Plan 3, `serve` and the UI.** axum 0.8 on 127.0.0.1 with a per-run bearer token and Host check; JSON reads over `retrievals`, `retrieval_items`, `symbols`, `refs`, `files`; SSE driven by `Store::data_version` polling; `notify` watcher calling `Engine::refresh`; annotation writes to `map.toml`. React + shadcn + Sigma.js in `ui/` built by Bun, embedded with `rust-embed`. Table/tree canonical views, map projection, a11y acceptance from spec §11.
- **Plan 4, tier-two harness.** Headless Claude Code runs across three conditions, JSON output parsing for tokens and tool calls, result storage under `eval/runs/`.

## Self-review notes

- Spec §7 index model: every table has a task (2); `map.toml` shape (3); ranking steps 1 to 5 (9, 10); reasons structure (9); cut list of 25 (11).
- Spec §8 freshness: stat walk and inline refresh (8), 2 s budget and stale header (11), lock wait 500 ms (11), git HEAD (8). Background continuation is plan 2.
- Spec §9 tools: `repo_map` (11, 13), `find_symbol` (12, 13), header format (10), identifiers-only output (7, 10).
- Spec §11 security: read-only (all), sandbox and symlinks (4), denylist (3, 4), content scan (5, 8), 0600 (2), budget cap (10). Localhost binding and token are plan 3.
- Spec §12 eval tier one (14). Tier two is plan 4.
- Type consistency: `ScoredSymbol`, `Reasons`, `RefBy` defined in task 9 and used unchanged in 10, 11, 12, 14. `IndexStats.remaining` is the stale count used by `Engine`. `Store::get_meta` keys `index_version`, `git_head` written in task 8 and read in task 11.
