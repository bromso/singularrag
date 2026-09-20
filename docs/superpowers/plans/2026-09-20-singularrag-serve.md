# singularrag serve Implementation Plan (plan 3a of 4)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship `singularrag serve`: a localhost page where a developer sees every retrieval an agent made (served, cut, and why), browses the repo as an accessible treegrid, pins/excludes/annotates files and symbols into `map.toml`, and sees freshness live.

**Architecture:** `serve` owns its own engine actor (moved to `crate::actor`, with a new `Job::Refresh`) fed by a `notify` watcher; UI reads go through a separate read-only SQLite connection so they never wait behind indexing; a `data_version` poller drives SSE. The UI is a Bun-built React app embedded in the binary via `rust-embed`. The treegrid (react-aria-components) is the canonical view; the map projection is plan 3b.

**Tech Stack:** Rust: axum 0.8, tower-http, rust-embed 8, notify 8 + notify-debouncer-full 0.7, open 5, rand, subtle, tokio-stream, reqwest (dev). UI: React 19, TypeScript, Tailwind v4 via bun-plugin-tailwind, shadcn on Base UI (manual install), react-aria-components, bun test + happy-dom + Testing Library + axe-core. Bun is the only JS toolchain.

**Spec:** `docs/superpowers/specs/2026-09-20-singularrag-serve-design.md` (this plan); parent `docs/superpowers/specs/2026-09-19-singularrag-design.md` §7, §10, §11.

## Global Constraints

- Bind `127.0.0.1` only; `--port` default 0 (ephemeral); print exactly one stdout line `http://127.0.0.1:<port>/#token=<hex>`; open the browser unless `--no-open` (spec §2).
- Every `/api` request needs the per-run token (32 random bytes, hex): `Authorization: Bearer <hex>` or `?token=<hex>` for the EventSource; constant-time compare; missing/wrong → 401 (spec §5).
- `Host` must be exactly `127.0.0.1:<port>` or `localhost:<port>`; else 403 before routing. No CORS headers. `Cache-Control: no-store` on `/api`; hashed assets `public, max-age=31536000, immutable` (spec §5).
- UI reads use `Store::open_read_only` (`SQLITE_OPEN_READ_ONLY`), which refuses a wrong `schema_version`; reads never go through the actor (spec §2).
- The only write is `PUT /api/map` → `.singularrag/map.toml` via temp file + rename; validation: paths repo-relative, no `..`, not absolute, no leading `./`; `deny.extra_patterns` may only grow; invalid → 422 (spec §3).
- SSE: `change { max_retrieval_id }` on `data_version` change (250 ms poll), `freshness { ... }` on snapshot change, keep-alive 15 s (spec §2).
- Watcher: `notify-debouncer-full` 300 ms, recursive, ignores `.git/` and `.singularrag/`; one `Job::Refresh` per batch; `lock_timeout` → `foreign_indexing: true`, retry once after 2 s, never an error (spec §2).
- Actor moves to `crate::actor`; `Job::Refresh`; `SessionKey::{Fixed, FromHandshake}`; serve's key is `"serve"` (spec §2).
- Session labels: `mcp:<slug>:…` → slug title-cased with `-`→space; `cli-<pid>` → "CLI"; `serve` → "UI"; else raw key (spec §3).
- Status encoded as text label + icon shape + colour, never colour alone; WCAG 2.2 AA; treegrid keyboard model per spec §6; polite live region; no animation (spec §4, §6).
- Bun only for the UI: `bun run build` → `ui/dist` (gitignored); `build.rs` fails clearly if `ui/dist/index.html` is missing (spec §7).
- stdout carries only the startup line; logs to stderr via `tracing` (spec §2).
- No "try a query" box; `serve` is an observer (spec §10).

## File structure

```
crates/singularrag-core/src/store/mod.rs         + open_read_only()
crates/singularrag-core/src/config.rs             + validate(), save_atomic(), HEADER
crates/singularrag/src/actor.rs                   moved from mcp/actor.rs; + Job::Refresh, SessionKey
crates/singularrag/src/mcp/mod.rs                 uses crate::actor, SessionKey::FromHandshake
crates/singularrag/src/mcp/server.rs              import path only
crates/singularrag/src/serve/mod.rs               run(root, port, open) → runtime, actor, watcher, router
crates/singularrag/src/serve/auth.rs              token + Host middleware (tests)
crates/singularrag/src/serve/state.rs             AppState { read: Mutex<Connection>, freshness, tx, root, token, port }
crates/singularrag/src/serve/queries.rs           SQL → DTOs for status/retrievals/tree/skipped (tests on fixture)
crates/singularrag/src/serve/routes.rs            axum handlers
crates/singularrag/src/serve/events.rs            SSE + data_version poller
crates/singularrag/src/serve/watcher.rs           notify → Job::Refresh → Freshness
crates/singularrag/src/serve/assets.rs            rust-embed + index/assets handlers
crates/singularrag/build.rs                       ui/dist guard
crates/singularrag/src/main.rs                    + Cmd::Serve
crates/singularrag/tests/serve.rs                 integration via reqwest
ui/                                               Bun project (Tasks 9–14)
```

---

### Task 1: Move the actor to `crate::actor`; add `Job::Refresh` and `SessionKey`

**Files:**
- Move: `crates/singularrag/src/mcp/actor.rs` → `crates/singularrag/src/actor.rs` (`git mv`)
- Modify: `crates/singularrag/src/actor.rs`, `crates/singularrag/src/mcp/mod.rs`, `crates/singularrag/src/mcp/server.rs`, `crates/singularrag/src/main.rs`

**Interfaces:**
- Produces: `crate::actor::{Job, Reply, DrainStats, EngineConfig, EngineHandle, SessionKey, spawn}`; `Job::Refresh(oneshot::Sender<Reply<IndexStats>>)`; `EngineHandle::refresh(&self) -> Reply<IndexStats>` (async); `pub enum SessionKey { Fixed(String), FromHandshake(Arc<Mutex<Option<String>>>) }`; `EngineConfig { root, session_key: SessionKey, refresh_budget }`. The `#[allow(dead_code)]` on `Stats`/`SetRefreshBudget`/`DrainStats` stays until Task 5 uses them.
- Consumes: existing actor internals (`Actor::session_key`, `engine()`, `handle()`).

- [ ] **Step 1: Move the file and fix imports so everything compiles unchanged**

```bash
git mv crates/singularrag/src/mcp/actor.rs crates/singularrag/src/actor.rs
```
In `crates/singularrag/src/main.rs` add `mod actor;` next to `mod mcp;`. In `crates/singularrag/src/mcp/mod.rs` delete `pub mod actor;` and replace `actor::` with `crate::actor::` (three uses: `spawn`, `EngineConfig`). In `crates/singularrag/src/mcp/server.rs` change `use super::actor::EngineHandle;` to `use crate::actor::EngineHandle;`. Run `cargo test --workspace`; expect the same 100 green.

- [ ] **Step 2: Write the failing tests**

Add to the `tests` module at the bottom of `crates/singularrag/src/actor.rs` (the `config()` helper there builds an `EngineConfig`; update it in step 4 to use `SessionKey::FromHandshake`):
```rust
    #[tokio::test]
    async fn fixed_session_key_is_used_verbatim() {
        let dir = fixture();
        let (handle, _join, _died) = spawn(EngineConfig {
            root: dir.path().to_path_buf(),
            session_key: SessionKey::Fixed("serve".into()),
            refresh_budget: REFRESH_BUDGET,
        });
        handle.map(MapRequest::default()).await.unwrap();
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        let key: String = store
            .conn()
            .query_row("SELECT session_key FROM retrievals ORDER BY id DESC LIMIT 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(key, "serve");
    }

    #[tokio::test]
    async fn refresh_job_indexes_and_returns_stats() {
        let dir = fixture();
        let (handle, _join, _died) = spawn(config(dir.path(), REFRESH_BUDGET));
        let stats = handle.refresh().await.unwrap();
        assert!(stats.indexed >= 4, "{stats:?}");
        assert_eq!(stats.remaining, 0);
        assert!(!stats.lock_timeout);
        let again = handle.refresh().await.unwrap();
        assert_eq!(again.indexed, 0);
        assert!(again.unchanged >= 4);
        // A refresh is not a retrieval: nothing recorded.
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        let n: i64 = store.conn().query_row("SELECT COUNT(*) FROM retrievals", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
    }

    #[tokio::test]
    async fn refresh_job_reports_lock_timeout_under_a_foreign_lock() {
        let dir = fixture();
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        let foreign = std::process::id() + 1;
        assert!(lock::try_acquire(&store, foreign, singularrag_core::time::now_ms()).unwrap());
        let (handle, _join, _died) = spawn(config(dir.path(), REFRESH_BUDGET));
        let stats = handle.refresh().await.unwrap();
        assert!(stats.lock_timeout);
        assert_eq!(stats.indexed, 0);
        lock::release(&store, foreign).unwrap();
    }
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p singularrag actor`
Expected: compile errors: `SessionKey` and `EngineHandle::refresh` not found.

- [ ] **Step 4: Implement**

In `crates/singularrag/src/actor.rs`:
```rust
/// Where the `<client>` part of the session key comes from.
#[derive(Clone)]
pub enum SessionKey {
    /// A process that knows its own name (`serve`, tests): the key is used verbatim.
    Fixed(String),
    /// The MCP server: filled from `clientInfo.name` during initialize, read at first job.
    FromHandshake(Arc<Mutex<Option<String>>>),
}
```
Change `EngineConfig.session_key` to `SessionKey`. Add `Refresh(oneshot::Sender<Reply<IndexStats>>)` to `Job`. In `EngineHandle`:
```rust
    /// One budgeted refresh with no retrieval recorded. The watcher's job.
    pub async fn refresh(&self) -> Reply<IndexStats> {
        self.ask(Job::Refresh).await
    }
```
In `Actor::session_key`:
```rust
    fn session_key(&self) -> String {
        match &self.config.session_key {
            SessionKey::Fixed(k) => k.clone(),
            SessionKey::FromHandshake(slot) => {
                let client = slot
                    .lock()
                    .map(|g| g.clone())
                    .unwrap_or(None)
                    .unwrap_or_else(|| "unknown".to_string());
                format!("mcp:{client}:{}:{}", std::process::id(), self.start_ms)
            }
        }
    }
```
(Keep whatever the current code does for `start_ms`; only the `Fixed` arm is new.) In `Actor::handle` add:
```rust
            Job::Refresh(reply) => {
                let budget = self.config.refresh_budget;
                let out = self
                    .engine()
                    .and_then(|e| e.refresh(budget).map_err(|e| e.to_string()));
                if let Ok(stats) = &out {
                    self.last_stats = stats.clone();
                    // A refresh from the watcher is not a tool response; it does not arm
                    // the drain (the caller decides whether to refresh again).
                }
                let _ = reply.send(out);
            }
```
Update `mcp/mod.rs` to build `SessionKey::FromHandshake(Arc::clone(&session_key))` and the actor tests' `config()` helper to `SessionKey::FromHandshake(Arc::new(Mutex::new(Some("test-client".into()))))`; the `unknown_client...` test wraps `None` the same way.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p singularrag actor` then `cargo test --workspace`
Expected: 3 new tests pass; 103 total green; the mcp integration tests still pass (session key format unchanged).

- [ ] **Step 6: Lint and commit**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings`
```bash
git add -A crates/singularrag
git commit -m "refactor(actor): move to crate::actor; add Job::Refresh and SessionKey"
```

---

### Task 2: `Store::open_read_only`

**Files:**
- Modify: `crates/singularrag-core/src/store/mod.rs`

**Interfaces:**
- Produces: `Store::open_read_only(path: &Path) -> Result<Store>`: `OpenFlags::SQLITE_OPEN_READ_ONLY | SQLITE_OPEN_NO_MUTEX`, `busy_timeout` 1000, no DDL, no meta write; returns `Error::Config("index schema is v{found}, this build needs v{SCHEMA_VERSION}; run `singularrag index`")` on mismatch and `Error::Config("no index at {path}; run `singularrag index`")` when the file does not exist. `Store::data_version()` and `conn()` work as before on it.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module of `crates/singularrag-core/src/store/mod.rs`:
```rust
    #[test]
    fn read_only_opens_an_existing_index_and_cannot_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".singularrag/index.db");
        let writer = Store::open(&path).unwrap();
        writer.set_meta("git_head", "abc").unwrap();
        let ro = Store::open_read_only(&path).unwrap();
        assert_eq!(ro.get_meta("git_head").unwrap().as_deref(), Some("abc"));
        assert!(ro.data_version().unwrap() >= 0);
        let err = ro.conn().execute("INSERT INTO meta(key, value) VALUES ('x', 'y')", []).unwrap_err();
        assert!(err.to_string().contains("readonly"), "{err}");
    }

    #[test]
    fn read_only_refuses_a_missing_file_and_a_wrong_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        let err = Store::open_read_only(&path).unwrap_err();
        assert!(err.to_string().contains("no index at"), "{err}");
        let writer = Store::open(&path).unwrap();
        writer.set_meta("schema_version", "1").unwrap();
        drop(writer);
        let err = Store::open_read_only(&path).unwrap_err();
        assert!(err.to_string().contains("schema is v1"), "{err}");
    }

    #[test]
    fn read_only_sees_commits_from_the_writer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        let writer = Store::open(&path).unwrap();
        let ro = Store::open_read_only(&path).unwrap();
        let v1 = ro.data_version().unwrap();
        writer.set_meta("k", "v").unwrap();
        let v2 = ro.data_version().unwrap();
        assert_ne!(v1, v2, "data_version must change when another connection commits");
        assert_eq!(ro.get_meta("k").unwrap().as_deref(), Some("v"));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p singularrag-core read_only`
Expected: compile error, `open_read_only` not found.

- [ ] **Step 3: Implement**

```rust
    /// A second connection for readers (the UI). Read-only at the SQLite level, no DDL,
    /// no meta write; the writer side owns schema rebuilds, so a version mismatch is an
    /// error here rather than a rebuild.
    pub fn open_read_only(path: &Path) -> Result<Store> {
        use rusqlite::OpenFlags;
        if !path.exists() {
            return Err(crate::Error::Config(format!(
                "no index at {}; run `singularrag index`",
                path.display()
            )));
        }
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.pragma_update(None, "busy_timeout", 1000)?;
        let found = stored_version(&conn)?.unwrap_or(0);
        if found != SCHEMA_VERSION {
            return Err(crate::Error::Config(format!(
                "index schema is v{found}, this build needs v{SCHEMA_VERSION}; run `singularrag index`"
            )));
        }
        Ok(Store { conn })
    }
```
`stored_version` already exists (used by `init`). Note for the doc comment: a WAL database needs the `-shm` file writable by this process even for readers; the writer creates it with the DB's permissions, and both run as the same user, so no extra handling.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p singularrag-core store`
Expected: all store tests pass including the 3 new ones.

- [ ] **Step 5: Lint and commit**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test --workspace`
```bash
git add crates/singularrag-core/src/store/mod.rs
git commit -m "feat(core): read-only store connection for UI readers"
```

---

### Task 3: `MapConfig` validation and atomic save

**Files:**
- Modify: `crates/singularrag-core/src/config.rs`

**Interfaces:**
- Produces: `MapConfig::validate(&self, current_extras: &[String]) -> std::result::Result<(), MapConfigError>`; `pub struct MapConfigError { pub field: String, pub message: String }` (`Debug`, `Clone`, `Serialize`); `MapConfig::save_atomic(&self, root: &Path) -> Result<()>` (writes `HEADER` + `toml::to_string_pretty`, to `map.toml.tmp` then rename); `pub const MAP_HEADER: &str = "# Managed by singularrag serve. Hand edits are kept; comments are not.\n"`; `MapConfig::parse` skips the header naturally (it is a TOML comment).

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module of `crates/singularrag-core/src/config.rs`:
```rust
    fn cfg(toml_src: &str) -> MapConfig {
        MapConfig::parse(toml_src).unwrap()
    }

    #[test]
    fn validate_accepts_repo_relative_paths() {
        let c = cfg("[[pin]]\npath = \"src/a.ts\"\n[[exclude]]\npath = \"src/legacy/\"\n[[note]]\npath = \"src/a.ts\"\nsymbol = \"f\"\ntext = \"x\"\n[[boundary]]\nname = \"auth\"\npaths = [\"src/auth/\"]\n");
        assert!(c.validate(&[]).is_ok());
    }

    #[test]
    fn validate_rejects_absolute_dotdot_and_dot_slash() {
        for (bad, field) in [
            ("[[pin]]\npath = \"/etc/passwd\"\n", "pin[0].path"),
            ("[[exclude]]\npath = \"../x\"\n", "exclude[0].path"),
            ("[[note]]\npath = \"src/../../x\"\ntext = \"t\"\n", "note[0].path"),
            ("[[pin]]\npath = \"./src/a.ts\"\n", "pin[0].path"),
            ("[[boundary]]\nname = \"b\"\npaths = [\"src/ok\", \"C:\\\\x\"]\n", "boundary[0].paths[1]"),
            ("[[pin]]\npath = \"src\\\\a.ts\"\n", "pin[0].path"),
        ] {
            let err = cfg(bad).validate(&[]).unwrap_err();
            assert_eq!(err.field, field, "{bad}");
        }
    }

    #[test]
    fn validate_rejects_a_shrunk_deny_list() {
        let c = cfg("[deny]\nextra_patterns = [\"*.snap\"]\n");
        assert!(c.validate(&["*.snap".to_string()]).is_ok());
        assert!(c.validate(&[]).is_ok(), "growing is fine");
        let err = c.validate(&["*.snap".to_string(), "*.lock".to_string()]).unwrap_err();
        assert_eq!(err.field, "deny.extra_patterns");
        assert!(err.message.contains("*.lock"));
    }

    #[test]
    fn save_atomic_round_trips_with_header_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let c = cfg("[[pin]]\npath = \"src/a.ts\"\n[[note]]\npath = \"src/a.ts\"\ntext = \"hello\"\n");
        c.save_atomic(dir.path()).unwrap();
        let written = std::fs::read_to_string(dir.path().join(MAP_FILE)).unwrap();
        assert!(written.starts_with(MAP_HEADER), "{written}");
        assert!(!dir.path().join(".singularrag/map.toml.tmp").exists());
        assert_eq!(MapConfig::load(dir.path()).unwrap(), c);
        // Overwrite works too.
        let c2 = cfg("[[exclude]]\npath = \"src/legacy/\"\n");
        c2.save_atomic(dir.path()).unwrap();
        assert_eq!(MapConfig::load(dir.path()).unwrap(), c2);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p singularrag-core config`
Expected: compile errors, `validate`, `save_atomic`, `MAP_HEADER` not found.

- [ ] **Step 3: Implement**

Add to `crates/singularrag-core/src/config.rs`:
```rust
pub const MAP_HEADER: &str = "# Managed by singularrag serve. Hand edits are kept; comments are not.\n";

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MapConfigError {
    pub field: String,
    pub message: String,
}

fn check_path(field: &str, p: &str) -> std::result::Result<(), MapConfigError> {
    let bad = |message: &str| MapConfigError { field: field.to_string(), message: message.to_string() };
    if p.is_empty() {
        return Err(bad("path is empty"));
    }
    if p.starts_with('/') || p.contains('\\') || p.chars().nth(1) == Some(':') {
        return Err(bad("path must be repo-relative with forward slashes"));
    }
    if p.starts_with("./") {
        return Err(bad("path must not start with ./"));
    }
    if p.split('/').any(|seg| seg == "..") {
        return Err(bad("path must not contain .."));
    }
    Ok(())
}

impl MapConfig {
    /// Spec §3: every path repo-relative; `deny.extra_patterns` may only grow.
    pub fn validate(&self, current_extras: &[String]) -> std::result::Result<(), MapConfigError> {
        for (i, t) in self.pin.iter().enumerate() {
            check_path(&format!("pin[{i}].path"), &t.path)?;
        }
        for (i, t) in self.exclude.iter().enumerate() {
            check_path(&format!("exclude[{i}].path"), &t.path)?;
        }
        for (i, n) in self.note.iter().enumerate() {
            check_path(&format!("note[{i}].path"), &n.path)?;
        }
        for (i, b) in self.boundary.iter().enumerate() {
            for (j, p) in b.paths.iter().enumerate() {
                check_path(&format!("boundary[{i}].paths[{j}]"), p)?;
            }
        }
        if let Some(missing) = current_extras.iter().find(|p| !self.deny.extra_patterns.contains(p)) {
            return Err(MapConfigError {
                field: "deny.extra_patterns".into(),
                message: format!("deny patterns may only be added; {missing} was removed"),
            });
        }
        Ok(())
    }

    /// Write `map.toml` via a temp file and rename so a reader never sees a torn file.
    pub fn save_atomic(&self, root: &Path) -> Result<()> {
        let path = root.join(MAP_FILE);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = toml::to_string_pretty(self).map_err(|e| Error::Config(e.to_string()))?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, format!("{MAP_HEADER}{body}"))?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p singularrag-core config`
Expected: all config tests pass including the 4 new ones.

- [ ] **Step 5: Lint and commit**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test --workspace`
```bash
git add crates/singularrag-core/src/config.rs
git commit -m "feat(core): map.toml validation and atomic save"
```

---
### Task 4: Serve state, auth middleware, and the axum skeleton

**Files:**
- Modify: `Cargo.toml` (workspace deps), `crates/singularrag/Cargo.toml`, `crates/singularrag/src/main.rs`
- Create: `crates/singularrag/src/serve/mod.rs`, `crates/singularrag/src/serve/state.rs`, `crates/singularrag/src/serve/auth.rs`

**Interfaces:**
- Produces: `serve::state::AppState { root: PathBuf, port: u16, token: Arc<str>, read: Arc<Mutex<Store>>, freshness: Arc<RwLock<Freshness>>, events: broadcast::Sender<ServerEvent> }` (`Clone`); `serve::state::Freshness { stale_count, lock_timeout, foreign_indexing, drain: DrainStats, indexed_at_ms: Option<i64> }` (`Default`, `Clone`, `Serialize`); `serve::state::ServerEvent { Change { max_retrieval_id: i64 }, Freshness(Freshness) }` (`Clone`); `serve::auth::require_token(State, Request, Next)` middleware (Bearer header or `?token=`, constant-time, 401 JSON); `serve::auth::check_host(State, Request, Next)` (403 JSON); `serve::auth::generate_token() -> String` (64 hex chars); `serve::router(state) -> Router` with `/api/health` returning `{"ok":true}` and all `/api` routes behind both middlewares plus `Cache-Control: no-store`; `serve::run(root, port, open_browser) -> anyhow::Result<()>` that binds, prints the URL line, serves. (Routes, SSE, watcher, assets arrive in Tasks 5–8; `run` grows with them.)
- Verified rmcp-style facts (2026-09-20): axum 0.8.9 default features include `json`, `query`, `tokio` (which enables `tokio/net`); `middleware::from_fn_with_state(state, f)`; `axum::http::header::{AUTHORIZATION, HOST, CACHE_CONTROL}`; `tower-http` 0.7 `SetResponseHeaderLayer`; `subtle` 2.6 `ConstantTimeEq::ct_eq` on `[u8]`; `rand` 0.10 `rand::rng().fill_bytes(&mut buf)`; `hex::encode`.

- [ ] **Step 1: Add dependencies**

Workspace `Cargo.toml` `[workspace.dependencies]` add:
```toml
axum = "0.8"
tower-http = { version = "0.7", features = ["set-header"] }
tokio-stream = { version = "0.1", features = ["sync"] }
rust-embed = { version = "8", features = ["interpolate-folder-path"] }
mime_guess = "2"
notify = "8"
notify-debouncer-full = "0.7"
open = "5"
rand = "0.10"
hex = "0.4"
subtle = "2"
reqwest = { version = "0.13", default-features = false, features = ["json", "stream"] }
futures-util = "0.3"
```
`crates/singularrag/Cargo.toml` `[dependencies]` add `axum`, `tower-http`, `tokio-stream`, `rust-embed`, `mime_guess`, `notify`, `notify-debouncer-full`, `open`, `rand`, `hex`, `subtle`, `futures-util`, `rusqlite = { workspace = true }`, `toml = { workspace = true }` (all `{ workspace = true }`); `[dev-dependencies]` add `reqwest = { workspace = true }`. Ensure the workspace `tokio` features include `net` (add it if absent).

- [ ] **Step 2: Write the failing tests**

`crates/singularrag/src/serve/auth.rs` bottom:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};
    use tower::ServiceExt;

    async fn app() -> (axum::Router, String) {
        let dir = tempfile::tempdir().unwrap();
        singularrag_core::fixture::write_ts_mini(dir.path());
        singularrag_core::store::Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        let state = crate::serve::state::AppState::new(dir.path().to_path_buf(), 4173).unwrap();
        let token = state.token.to_string();
        std::mem::forget(dir);
        (crate::serve::router(state), token)
    }

    fn req(method: &str, uri: &str, host: &str, auth: Option<&str>) -> Request<Body> {
        let mut b = Request::builder().method(method).uri(uri).header(header::HOST, host);
        if let Some(a) = auth {
            b = b.header(header::AUTHORIZATION, a);
        }
        b.body(Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn token_is_64_hex_chars_and_unique() {
        let a = generate_token();
        let b = generate_token();
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[tokio::test]
    async fn api_requires_bearer_or_query_token() {
        let (app, token) = app().await;
        let r = app.clone().oneshot(req("GET", "/api/health", "127.0.0.1:4173", None)).await.unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        let r = app.clone().oneshot(req("GET", "/api/health", "127.0.0.1:4173", Some("Bearer nope"))).await.unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        let r = app.clone().oneshot(req("GET", "/api/health", "127.0.0.1:4173", Some(&format!("Bearer {token}")))).await.unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(r.headers().get(header::CACHE_CONTROL).unwrap(), "no-store");
        let r = app.oneshot(req("GET", &format!("/api/health?token={token}"), "localhost:4173", None)).await.unwrap();
        assert_eq!(r.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn wrong_host_is_forbidden_before_auth() {
        let (app, token) = app().await;
        for host in ["evil.example:4173", "127.0.0.1:9999", "127.0.0.1", "[::1]:4173"] {
            let r = app.clone().oneshot(req("GET", "/api/health", host, Some(&format!("Bearer {token}")))).await.unwrap();
            assert_eq!(r.status(), StatusCode::FORBIDDEN, "{host}");
        }
    }

    #[tokio::test]
    async fn no_cors_headers_ever() {
        let (app, token) = app().await;
        let r = app.oneshot(req("OPTIONS", "/api/health", "127.0.0.1:4173", Some(&format!("Bearer {token}")))).await.unwrap();
        assert!(r.headers().get("access-control-allow-origin").is_none());
    }
}
```
Add `tower = { version = "0.5", features = ["util"] }` to the bin crate's `[dev-dependencies]` (workspace dep too) for `oneshot`.

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p singularrag serve::auth`
Expected: compile errors, module missing.

- [ ] **Step 4: Implement**

`crates/singularrag/src/serve/state.rs`:
```rust
//! Shared state for `singularrag serve`: one read-only store behind a mutex, the
//! freshness snapshot the watcher maintains, and the broadcast channel SSE fans out.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use serde::Serialize;
use singularrag_core::store::Store;
use tokio::sync::broadcast;

use crate::actor::DrainStats;

#[derive(Debug, Clone, Default, Serialize)]
pub struct Freshness {
    pub stale_count: usize,
    pub lock_timeout: bool,
    pub foreign_indexing: bool,
    pub drain: DrainStatsJson,
    pub indexed_at_ms: Option<i64>,
}

/// `DrainStats` is not `Serialize` in the actor; mirror it here for the API.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DrainStatsJson {
    pub chunks: u64,
    pub last: singularrag_core::index::IndexStats,
}

impl From<DrainStats> for DrainStatsJson {
    fn from(d: DrainStats) -> Self {
        DrainStatsJson { chunks: d.chunks, last: d.last }
    }
}

#[derive(Debug, Clone)]
pub enum ServerEvent {
    Change { max_retrieval_id: i64 },
    Freshness(Freshness),
}

#[derive(Clone)]
pub struct AppState {
    pub root: PathBuf,
    pub port: u16,
    pub token: Arc<str>,
    pub read: Arc<Mutex<Store>>,
    pub freshness: Arc<RwLock<Freshness>>,
    pub events: broadcast::Sender<ServerEvent>,
}

impl AppState {
    pub fn new(root: PathBuf, port: u16) -> anyhow::Result<AppState> {
        let root = root.canonicalize()?;
        let read = Store::open_read_only(&root.join(singularrag_core::engine::DB_FILE))?;
        let (events, _) = broadcast::channel(64);
        Ok(AppState {
            root,
            port,
            token: crate::serve::auth::generate_token().into(),
            read: Arc::new(Mutex::new(read)),
            freshness: Arc::new(RwLock::new(Freshness::default())),
            events,
        })
    }
}
```
`IndexStats` needs `Serialize`: add `serde::Serialize` to its derive in `crates/singularrag-core/src/index.rs` (serde is already a core dependency).

`crates/singularrag/src/serve/auth.rs`:
```rust
//! Per-run bearer token and Host check (parent spec §11): loopback bind alone does not
//! stop a web page on the same machine, or DNS rebinding, from reaching the API.

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use rand::RngCore;
use subtle::ConstantTimeEq;

use super::state::AppState;

pub fn generate_token() -> String {
    let mut buf = [0u8; 32];
    rand::rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

fn reject(status: StatusCode, msg: &str) -> Response {
    (status, Json(serde_json::json!({ "error": msg }))).into_response()
}

fn presented_token(req: &Request<Body>) -> Option<String> {
    if let Some(h) = req.headers().get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) {
        if let Some(t) = h.strip_prefix("Bearer ") {
            return Some(t.trim().to_string());
        }
    }
    req.uri().query().and_then(|q| {
        q.split('&').find_map(|kv| kv.strip_prefix("token=").map(|v| v.to_string()))
    })
}

pub async fn require_token(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let Some(t) = presented_token(&req) else {
        return reject(StatusCode::UNAUTHORIZED, "missing token");
    };
    let ok: bool = t.as_bytes().ct_eq(state.token.as_bytes()).into();
    if !ok {
        return reject(StatusCode::UNAUTHORIZED, "invalid token");
    }
    next.run(req).await
}

pub async fn check_host(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let host = req.headers().get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("");
    let allowed = [format!("127.0.0.1:{}", state.port), format!("localhost:{}", state.port)];
    if !allowed.iter().any(|a| a == host) {
        return reject(StatusCode::FORBIDDEN, "host not allowed");
    }
    next.run(req).await
}
```
`crates/singularrag/src/serve/mod.rs`:
```rust
//! `singularrag serve`: the loop UI. Own actor + watcher for writes, a read-only store for
//! reads, SSE for liveness, an embedded React app for the page.

pub mod auth;
pub mod state;

use std::path::PathBuf;

use axum::http::{header, HeaderValue};
use axum::routing::get;
use axum::{middleware, Json, Router};
use tower_http::set_header::SetResponseHeaderLayer;

use state::AppState;

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/health", get(|| async { Json(serde_json::json!({ "ok": true })) }))
        .layer(SetResponseHeaderLayer::overriding(header::CACHE_CONTROL, HeaderValue::from_static("no-store")))
        .layer(middleware::from_fn_with_state(state.clone(), auth::require_token))
        .layer(middleware::from_fn_with_state(state.clone(), auth::check_host));
    Router::new().nest("/api", api).with_state(state)
}

/// Bind, print the one stdout line, open the browser, serve until Ctrl-C.
pub fn run(root: PathBuf, port: u16, open_browser: bool) -> anyhow::Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    rt.block_on(async move {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
        let port = listener.local_addr()?.port();
        let state = AppState::new(root, port)?;
        let url = format!("http://127.0.0.1:{port}/#token={}", state.token);
        println!("{url}");
        if open_browser {
            if let Err(e) = open::that(&url) {
                tracing::warn!("could not open a browser: {e}");
            }
        }
        axum::serve(listener, router(state)).await?;
        Ok::<(), anyhow::Error>(())
    })
}
```
Layer order note: axum applies layers bottom-up, so `check_host` runs first, then `require_token`, then the cache header wraps the response. `main.rs`: add `mod serve;`, and `Cmd::Serve { port: u16 (default 0), no_open: bool }` with doc "Open the map UI on localhost"; dispatch it before `Engine::open` like `Mcp` (early return `serve::run(root, port, !no_open)`), and add `Cmd::Serve { .. } => unreachable!(..)` to the match.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p singularrag serve`
Expected: 4 passed. Then `cargo test --workspace` green.

- [ ] **Step 6: Lint and commit**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings`
```bash
git add Cargo.toml Cargo.lock crates
git commit -m "feat(serve): axum skeleton with per-run token and host check"
```

---

### Task 5: Read queries and the JSON routes

**Files:**
- Create: `crates/singularrag/src/serve/queries.rs`, `crates/singularrag/src/serve/routes.rs`
- Modify: `crates/singularrag/src/serve/mod.rs` (mount routes)

**Interfaces:**
- Consumes: `AppState.read` (`Store`), `Freshness`, `MapConfig::{load, validate, save_atomic}`, `singularrag_core::config::MAP_FILE`.
- Produces (all `Serialize`, in `queries.rs`): `StatusDto`, `RetrievalSummary`, `RetrievalDetail { #[serde(flatten)] summary, items: Vec<ItemDto> }`, `ItemDto { rank, symbol_id, path, name, line_start, score, served: bool, reasons: serde_json::Value }`, `TreeFile { path, lang, skipped_reason, symbols: Vec<TreeSymbol> }`, `TreeSymbol { id, name, kind, line_start, line_end, signature }`, `SkippedFile { path, reason }`; functions `status(&Store, &Freshness) -> Result<StatusDto>`, `retrievals(&Store, limit: usize, before: Option<i64>) -> Result<Vec<RetrievalSummary>>`, `retrieval(&Store, id) -> Result<Option<RetrievalDetail>>`, `tree(&Store) -> Result<Vec<TreeFile>>`, `skipped(&Store) -> Result<Vec<SkippedFile>>`, `session_label(&str) -> String`, `max_retrieval_id(&Store) -> Result<i64>`. Routes in `routes.rs`: `GET /status`, `GET /retrievals`, `GET /retrievals/{id}`, `GET /tree`, `GET /skipped`, `GET /map`, `PUT /map`.

- [ ] **Step 1: Write the failing tests**

`crates/singularrag/src/serve/queries.rs` bottom:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use singularrag_core::engine::{Engine, FindRequest, MapRequest};
    use singularrag_core::fixture::write_ts_mini;
    use singularrag_core::store::Store;

    fn seeded() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        write_ts_mini(dir.path());
        let mut e = Engine::open(dir.path(), "mcp:claude-code:1:2").unwrap();
        e.repo_map(&MapRequest { query: Some("session".into()), focus_files: vec![], budget_tokens: 256 }).unwrap();
        e.find_symbol(&FindRequest { name: "log".into(), kind: None, limit: 5 }).unwrap();
        drop(e);
        let ro = Store::open_read_only(&dir.path().join(".singularrag/index.db")).unwrap();
        (dir, ro)
    }

    #[test]
    fn labels_sessions() {
        assert_eq!(session_label("mcp:claude-code:123:456"), "Claude Code");
        assert_eq!(session_label("mcp:codex:1:2"), "Codex");
        assert_eq!(session_label("cli-999"), "CLI");
        assert_eq!(session_label("serve"), "UI");
        assert_eq!(session_label("something-else"), "something-else");
    }

    #[test]
    fn retrievals_newest_first_with_counts_and_paging() {
        let (_d, ro) = seeded();
        let all = retrievals(&ro, 50, None).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].tool, "find_symbol");
        assert_eq!(all[0].limit_n, Some(5));
        assert_eq!(all[1].tool, "repo_map");
        assert_eq!(all[1].budget, Some(256));
        assert_eq!(all[1].session_label, "Claude Code");
        assert!(all[1].served > 0);
        assert!(all[1].served + all[1].cut > all[1].served, "a 256-token map must cut something");
        let page = retrievals(&ro, 50, Some(all[0].id)).unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].id, all[1].id);
        assert_eq!(max_retrieval_id(&ro).unwrap(), all[0].id);
    }

    #[test]
    fn retrieval_detail_parses_reasons() {
        let (_d, ro) = seeded();
        let id = retrievals(&ro, 50, None).unwrap()[1].id;
        let d = retrieval(&ro, id).unwrap().unwrap();
        assert_eq!(d.summary.tool, "repo_map");
        assert!(!d.items.is_empty());
        let first = &d.items[0];
        assert_eq!(first.rank, 1);
        assert!(first.served);
        assert!(first.reasons.get("referenced_by").is_some(), "{:?}", first.reasons);
        assert!(d.items.iter().any(|i| !i.served));
        assert!(retrieval(&ro, 999_999).unwrap().is_none());
    }

    #[test]
    fn tree_and_skipped() {
        let (_d, ro) = seeded();
        let t = tree(&ro).unwrap();
        let session = t.iter().find(|f| f.path == "src/auth/session.ts").unwrap();
        assert_eq!(session.lang.as_deref(), Some("typescript"));
        assert!(session.symbols.iter().any(|s| s.name == "createSession" && s.kind == "function"));
        assert!(t.iter().all(|f| f.skipped_reason.is_none()), "tree lists indexed files only");
        let sk = skipped(&ro).unwrap();
        assert!(sk.iter().any(|s| s.path == ".env" && s.reason == "denylisted"));
        assert!(sk.iter().any(|s| s.path == "src/config.ts" && s.reason == "secret-like content"));
    }

    #[test]
    fn status_merges_meta_and_freshness() {
        let (_d, ro) = seeded();
        let f = crate::serve::state::Freshness { stale_count: 3, foreign_indexing: true, ..Default::default() };
        let s = status(&ro, &f).unwrap();
        assert_eq!(s.index_version.len(), 12);
        assert_eq!(s.stale_count, 3);
        assert!(s.foreign_indexing);
        assert!(s.files.indexed >= 4);
        assert!(s.files.skipped >= 2);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p singularrag serve::queries`
Expected: compile errors.

- [ ] **Step 3: Implement the queries**

`crates/singularrag/src/serve/queries.rs`:
```rust
//! Read-only SQL for the UI. Every function takes the read connection and returns
//! serialisable DTOs; nothing here writes.

use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use singularrag_core::store::Store;
use singularrag_core::Result;

use super::state::{DrainStatsJson, Freshness};

#[derive(Debug, Serialize)]
pub struct FileCounts { pub indexed: i64, pub skipped: i64 }

#[derive(Debug, Serialize)]
pub struct StatusDto {
    pub index_version: String,
    pub git_head: Option<String>,
    pub indexed_at_ms: Option<i64>,
    pub stale_count: usize,
    pub lock_timeout: bool,
    pub foreign_indexing: bool,
    pub files: FileCounts,
    pub drain: DrainStatsJson,
}

#[derive(Debug, Clone, Serialize)]
pub struct RetrievalSummary {
    pub id: i64,
    pub session_key: String,
    pub session_label: String,
    pub tool: String,
    pub query: Option<String>,
    pub focus_files: Vec<String>,
    pub budget: Option<i64>,
    pub limit_n: Option<i64>,
    pub index_version: String,
    pub git_head: Option<String>,
    pub stale_count: i64,
    pub created_at_ms: i64,
    pub served: i64,
    pub cut: i64,
}

#[derive(Debug, Serialize)]
pub struct ItemDto {
    pub rank: i64,
    pub symbol_id: i64,
    pub path: String,
    pub name: String,
    pub line_start: i64,
    pub score: f64,
    pub served: bool,
    pub reasons: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct RetrievalDetail {
    #[serde(flatten)]
    pub summary: RetrievalSummary,
    pub items: Vec<ItemDto>,
}

#[derive(Debug, Serialize)]
pub struct TreeSymbol { pub id: i64, pub name: String, pub kind: String, pub line_start: i64, pub line_end: i64, pub signature: String }

#[derive(Debug, Serialize)]
pub struct TreeFile { pub path: String, pub lang: Option<String>, pub skipped_reason: Option<String>, pub symbols: Vec<TreeSymbol> }

#[derive(Debug, Serialize)]
pub struct SkippedFile { pub path: String, pub reason: String }

/// Spec §3: `mcp:<slug>:…` → title-cased slug; `cli-<pid>` → CLI; `serve` → UI; else raw.
pub fn session_label(key: &str) -> String {
    if let Some(rest) = key.strip_prefix("mcp:") {
        let slug = rest.split(':').next().unwrap_or("");
        return slug
            .split('-')
            .filter(|s| !s.is_empty())
            .map(|w| {
                let mut c = w.chars();
                match c.next() {
                    Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
    }
    if key.starts_with("cli-") {
        return "CLI".into();
    }
    if key == "serve" {
        return "UI".into();
    }
    key.to_string()
}

pub fn max_retrieval_id(store: &Store) -> Result<i64> {
    Ok(store.conn().query_row("SELECT COALESCE(MAX(id), 0) FROM retrievals", [], |r| r.get(0))?)
}

pub fn status(store: &Store, f: &Freshness) -> Result<StatusDto> {
    let conn = store.conn();
    let indexed: i64 = conn.query_row("SELECT COUNT(*) FROM files WHERE skipped_reason IS NULL", [], |r| r.get(0))?;
    let skipped: i64 = conn.query_row("SELECT COUNT(*) FROM files WHERE skipped_reason IS NOT NULL", [], |r| r.get(0))?;
    Ok(StatusDto {
        index_version: store.get_meta("index_version")?.unwrap_or_default(),
        git_head: store.get_meta("git_head")?.filter(|h| !h.is_empty()),
        indexed_at_ms: store.get_meta("indexed_at_ms")?.and_then(|s| s.parse().ok()),
        stale_count: f.stale_count,
        lock_timeout: f.lock_timeout,
        foreign_indexing: f.foreign_indexing,
        files: FileCounts { indexed, skipped },
        drain: f.drain.clone(),
    })
}

const SUMMARY_SQL: &str = "SELECT r.id, r.session_key, r.tool, r.query, r.focus_files, r.budget, r.limit_n, r.index_version, r.git_head, r.stale_count, r.created_at_ms,
    (SELECT COUNT(*) FROM retrieval_items i WHERE i.retrieval_id = r.id AND i.served = 1),
    (SELECT COUNT(*) FROM retrieval_items i WHERE i.retrieval_id = r.id AND i.served = 0)
  FROM retrievals r";

fn summary_from_row(r: &rusqlite::Row) -> rusqlite::Result<RetrievalSummary> {
    let key: String = r.get(1)?;
    let focus: String = r.get(4)?;
    Ok(RetrievalSummary {
        id: r.get(0)?,
        session_label: session_label(&key),
        session_key: key,
        tool: r.get(2)?,
        query: r.get(3)?,
        focus_files: serde_json::from_str(&focus).unwrap_or_default(),
        budget: r.get(5)?,
        limit_n: r.get(6)?,
        index_version: r.get(7)?,
        git_head: r.get::<_, Option<String>>(8)?.filter(|h| !h.is_empty()),
        stale_count: r.get(9)?,
        created_at_ms: r.get(10)?,
        served: r.get(11)?,
        cut: r.get(12)?,
    })
}

pub fn retrievals(store: &Store, limit: usize, before: Option<i64>) -> Result<Vec<RetrievalSummary>> {
    let sql = format!("{SUMMARY_SQL} WHERE (?1 IS NULL OR r.id < ?1) ORDER BY r.id DESC LIMIT ?2");
    let mut stmt = store.conn().prepare(&sql)?;
    let rows = stmt.query_map(params![before, limit as i64], summary_from_row)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

pub fn retrieval(store: &Store, id: i64) -> Result<Option<RetrievalDetail>> {
    let sql = format!("{SUMMARY_SQL} WHERE r.id = ?1");
    let Some(summary) = store.conn().query_row(&sql, [id], summary_from_row).optional()? else {
        return Ok(None);
    };
    let mut stmt = store.conn().prepare(
        "SELECT rank, symbol_id, path, name, line_start, score, served, reasons_json FROM retrieval_items WHERE retrieval_id = ?1 ORDER BY rank",
    )?;
    let items = stmt
        .query_map([id], |r| {
            let raw: String = r.get(7)?;
            Ok(ItemDto {
                rank: r.get(0)?,
                symbol_id: r.get(1)?,
                path: r.get(2)?,
                name: r.get(3)?,
                line_start: r.get(4)?,
                score: r.get(5)?,
                served: r.get::<_, i64>(6)? == 1,
                reasons: serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null),
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(Some(RetrievalDetail { summary, items }))
}

pub fn tree(store: &Store) -> Result<Vec<TreeFile>> {
    let conn = store.conn();
    let mut files = conn.prepare("SELECT id, path, lang FROM files WHERE skipped_reason IS NULL ORDER BY path")?;
    let mut syms = conn.prepare("SELECT id, name, kind, line_start, line_end, signature FROM symbols WHERE file_id = ?1 ORDER BY line_start")?;
    let mut out = Vec::new();
    for row in files.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?)))? {
        let (fid, path, lang) = row?;
        let symbols = syms
            .query_map([fid], |r| {
                Ok(TreeSymbol { id: r.get(0)?, name: r.get(1)?, kind: r.get(2)?, line_start: r.get(3)?, line_end: r.get(4)?, signature: r.get(5)? })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        out.push(TreeFile { path, lang, skipped_reason: None, symbols });
    }
    Ok(out)
}

pub fn skipped(store: &Store) -> Result<Vec<SkippedFile>> {
    let mut stmt = store.conn().prepare("SELECT path, skipped_reason FROM files WHERE skipped_reason IS NOT NULL ORDER BY skipped_reason, path")?;
    let rows = stmt.query_map([], |r| Ok(SkippedFile { path: r.get(0)?, reason: r.get(1)? }))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}
```

- [ ] **Step 4: Run the query tests**

Run: `cargo test -p singularrag serve::queries`
Expected: 5 passed.

- [ ] **Step 5: Add the routes and mount them**

`crates/singularrag/src/serve/routes.rs`:
```rust
//! JSON handlers. Each locks the read store for the duration of one query.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use singularrag_core::config::MapConfig;

use super::queries;
use super::state::AppState;

pub struct ApiError(StatusCode, serde_json::Value);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(self.1)).into_response()
    }
}

impl From<singularrag_core::Error> for ApiError {
    fn from(e: singularrag_core::Error) -> Self {
        let busy = matches!(&e, singularrag_core::Error::Sqlite(rusqlite::Error::SqliteFailure(f, _)) if f.code == rusqlite::ErrorCode::DatabaseBusy);
        let status = if busy { StatusCode::SERVICE_UNAVAILABLE } else { StatusCode::INTERNAL_SERVER_ERROR };
        ApiError(status, serde_json::json!({ "error": e.to_string() }))
    }
}

fn locked<T>(state: &AppState, f: impl FnOnce(&singularrag_core::store::Store) -> singularrag_core::Result<T>) -> Result<T, ApiError> {
    let guard = state.read.lock().map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, serde_json::json!({ "error": "read store poisoned" })))?;
    Ok(f(&guard)?)
}

pub async fn status(State(s): State<AppState>) -> Result<Json<queries::StatusDto>, ApiError> {
    let f = s.freshness.read().map(|g| g.clone()).unwrap_or_default();
    Ok(Json(locked(&s, |store| queries::status(store, &f))?))
}

#[derive(Deserialize)]
pub struct Page { pub limit: Option<usize>, pub before: Option<i64> }

pub async fn retrievals(State(s): State<AppState>, Query(p): Query<Page>) -> Result<Json<Vec<queries::RetrievalSummary>>, ApiError> {
    let limit = p.limit.unwrap_or(50).clamp(1, 200);
    Ok(Json(locked(&s, |store| queries::retrievals(store, limit, p.before))?))
}

pub async fn retrieval(State(s): State<AppState>, Path(id): Path<i64>) -> Result<Json<queries::RetrievalDetail>, ApiError> {
    match locked(&s, |store| queries::retrieval(store, id))? {
        Some(d) => Ok(Json(d)),
        None => Err(ApiError(StatusCode::NOT_FOUND, serde_json::json!({ "error": "no such retrieval" }))),
    }
}

pub async fn tree(State(s): State<AppState>) -> Result<Json<Vec<queries::TreeFile>>, ApiError> {
    Ok(Json(locked(&s, queries::tree)?))
}

pub async fn skipped(State(s): State<AppState>) -> Result<Json<Vec<queries::SkippedFile>>, ApiError> {
    Ok(Json(locked(&s, queries::skipped)?))
}

pub async fn get_map(State(s): State<AppState>) -> Result<Json<MapConfig>, ApiError> {
    Ok(Json(MapConfig::load(&s.root)?))
}

pub async fn put_map(State(s): State<AppState>, Json(cfg): Json<MapConfig>) -> Result<Json<MapConfig>, ApiError> {
    let current = MapConfig::load(&s.root)?;
    if let Err(e) = cfg.validate(&current.deny.extra_patterns) {
        return Err(ApiError(StatusCode::UNPROCESSABLE_ENTITY, serde_json::json!({ "error": e.message, "field": e.field })));
    }
    cfg.save_atomic(&s.root)?;
    Ok(Json(MapConfig::load(&s.root)?))
}
```
In `serve/mod.rs` add `pub mod queries; pub mod routes;` and extend the api router:
```rust
        .route("/status", get(routes::status))
        .route("/retrievals", get(routes::retrievals))
        .route("/retrievals/{id}", get(routes::retrieval))
        .route("/tree", get(routes::tree))
        .route("/skipped", get(routes::skipped))
        .route("/map", get(routes::get_map).put(routes::put_map))
```
(axum 0.8 path params use `{id}`.)

- [ ] **Step 6: Run everything, lint, commit**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test --workspace`
```bash
git add crates
git commit -m "feat(serve): read queries and json routes for status, retrievals, tree, skipped and map"
```

---

### Task 6: SSE events and the `data_version` poller

**Files:**
- Create: `crates/singularrag/src/serve/events.rs`
- Modify: `crates/singularrag/src/serve/mod.rs`

**Interfaces:**
- Produces: `events::sse(State) -> Sse<..>` handler on `GET /api/events`; `events::spawn_poller(state: AppState) -> tokio::task::JoinHandle<()>` polling `data_version` every 250 ms and broadcasting `ServerEvent::Change { max_retrieval_id }` on change; `events::to_sse_event(&ServerEvent) -> Event` (`event: change` with `{"max_retrieval_id":N}`, `event: freshness` with the snapshot JSON).
- Verified: `tokio_stream::wrappers::BroadcastStream` (feature `sync`) yields `Result<T, BroadcastStreamRecvError>`; lag errors are dropped with `filter_map(Result::ok)`; `Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))`.

- [ ] **Step 1: Write the failing tests**

`crates/singularrag/src/serve/events.rs` bottom:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::serve::state::{Freshness, ServerEvent};
    use singularrag_core::engine::{Engine, MapRequest};
    use singularrag_core::fixture::write_ts_mini;

    #[test]
    fn events_serialise_with_their_names() {
        let e = to_sse_event(&ServerEvent::Change { max_retrieval_id: 7 });
        let s = format!("{e:?}");
        assert!(s.contains("change"), "{s}");
        let e = to_sse_event(&ServerEvent::Freshness(Freshness { stale_count: 2, ..Default::default() }));
        let s = format!("{e:?}");
        assert!(s.contains("freshness"), "{s}");
    }

    #[tokio::test]
    async fn poller_broadcasts_change_when_another_connection_writes() {
        let dir = tempfile::tempdir().unwrap();
        write_ts_mini(dir.path());
        singularrag_core::store::Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        let state = crate::serve::state::AppState::new(dir.path().to_path_buf(), 1).unwrap();
        let mut rx = state.events.subscribe();
        let _poller = spawn_poller(state.clone());
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        let root = dir.path().to_path_buf();
        tokio::task::spawn_blocking(move || {
            let mut e = Engine::open(&root, "cli-1").unwrap();
            e.repo_map(&MapRequest::default()).unwrap();
        })
        .await
        .unwrap();
        let ev = tokio::time::timeout(std::time::Duration::from_secs(3), rx.recv()).await.expect("event within 3 s").unwrap();
        match ev {
            ServerEvent::Change { max_retrieval_id } => assert_eq!(max_retrieval_id, 1),
            other => panic!("unexpected {other:?}"),
        }
    }
}
```
(`Event` implements `Debug`; the `event:` name appears in it. If the debug output does not include the name, assert via `to_sse_event(..).into_data_or_name()`; read `axum::response::sse::Event`'s methods and adapt the assertion, not the requirement.)

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p singularrag serve::events`
Expected: compile errors.

- [ ] **Step 3: Implement**

`crates/singularrag/src/serve/events.rs`:
```rust
//! Live updates. A poller watches `PRAGMA data_version` on the read connection and
//! broadcasts a `change`; the watcher broadcasts `freshness`. Clients refetch; the
//! stream carries no rows.

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures_util::stream::{Stream, StreamExt};
use tokio_stream::wrappers::BroadcastStream;

use super::queries;
use super::state::{AppState, ServerEvent};

pub const POLL_EVERY: Duration = Duration::from_millis(250);

pub fn to_sse_event(e: &ServerEvent) -> Event {
    match e {
        ServerEvent::Change { max_retrieval_id } => Event::default()
            .event("change")
            .data(serde_json::json!({ "max_retrieval_id": max_retrieval_id }).to_string()),
        ServerEvent::Freshness(f) => Event::default()
            .event("freshness")
            .data(serde_json::to_string(f).unwrap_or_else(|_| "{}".into())),
    }
}

pub async fn sse(State(s): State<AppState>) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let stream = BroadcastStream::new(s.events.subscribe())
        .filter_map(|r| async move { r.ok() })
        .map(|e| Ok(to_sse_event(&e)));
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

pub fn spawn_poller(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut last: Option<i64> = None;
        loop {
            tokio::time::sleep(POLL_EVERY).await;
            let snapshot = {
                let Ok(store) = state.read.lock() else { continue };
                match (store.data_version(), queries::max_retrieval_id(&store)) {
                    (Ok(v), Ok(max)) => Some((v, max)),
                    _ => None,
                }
            };
            let Some((v, max)) = snapshot else { continue };
            if last.is_some_and(|l| l != v) {
                let _ = state.events.send(ServerEvent::Change { max_retrieval_id: max });
            }
            last = Some(v);
        }
    })
}
```
In `serve/mod.rs`: `pub mod events;`, route `.route("/events", get(events::sse))` inside the api router (it sits behind the token middleware; the EventSource passes `?token=`), and in `run` call `events::spawn_poller(state.clone())` before `axum::serve`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p singularrag serve::events`
Expected: 2 passed.

- [ ] **Step 5: Lint and commit**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test --workspace`
```bash
git add crates
git commit -m "feat(serve): sse stream driven by data_version polling"
```

---

### Task 7: Watcher → `Job::Refresh` → freshness

**Files:**
- Create: `crates/singularrag/src/serve/watcher.rs`
- Modify: `crates/singularrag/src/serve/mod.rs`

**Interfaces:**
- Consumes: `crate::actor::{spawn, EngineConfig, EngineHandle, SessionKey}`, `AppState.freshness`, `AppState.events`.
- Produces: `watcher::start(state: AppState, handle: EngineHandle) -> anyhow::Result<notify_debouncer_full::Debouncer<..>>` (must be kept alive by the caller); `watcher::apply(state: &AppState, stats: IndexStats)` updating the snapshot and broadcasting `Freshness`; `watcher::refresh_once(state: &AppState, handle: &EngineHandle)` (async; on `lock_timeout` sets `foreign_indexing`, sleeps 2 s, retries once).
- Verified: `notify_debouncer_full::new_debouncer(Duration, None, handler)` where the handler is `FnMut(DebounceEventResult) + Send + 'static`; `debouncer.watch(path, RecursiveMode::Recursive)` directly on the debouncer in 0.7; `DebouncedEvent` derefs to `notify::Event` with `.paths`.

- [ ] **Step 1: Write the failing tests**

`crates/singularrag/src/serve/watcher.rs` bottom:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::{spawn, EngineConfig, SessionKey};
    use singularrag_core::fixture::write_ts_mini;
    use singularrag_core::store::{lock, Store};
    use std::time::Duration;

    fn setup() -> (tempfile::TempDir, crate::serve::state::AppState, crate::actor::EngineHandle) {
        let dir = tempfile::tempdir().unwrap();
        write_ts_mini(dir.path());
        Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        let state = crate::serve::state::AppState::new(dir.path().to_path_buf(), 1).unwrap();
        let (handle, _join, _died) = spawn(EngineConfig {
            root: dir.path().to_path_buf(),
            session_key: SessionKey::Fixed("serve".into()),
            refresh_budget: singularrag_core::engine::REFRESH_BUDGET,
        });
        (dir, state, handle)
    }

    #[tokio::test]
    async fn refresh_once_updates_freshness_and_broadcasts() {
        let (_dir, state, handle) = setup();
        let mut rx = state.events.subscribe();
        refresh_once(&state, &handle).await;
        let f = state.freshness.read().unwrap().clone();
        assert_eq!(f.stale_count, 0);
        assert!(!f.foreign_indexing);
        assert!(f.indexed_at_ms.is_some());
        assert!(matches!(rx.try_recv().unwrap(), crate::serve::state::ServerEvent::Freshness(_)));
    }

    #[tokio::test]
    async fn foreign_lock_is_reported_not_errored() {
        let (dir, state, handle) = setup();
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        let foreign = std::process::id() + 1;
        assert!(lock::try_acquire(&store, foreign, singularrag_core::time::now_ms()).unwrap());
        let started = std::time::Instant::now();
        refresh_once(&state, &handle).await;
        assert!(started.elapsed() >= Duration::from_secs(2), "must retry once after 2 s");
        let f = state.freshness.read().unwrap().clone();
        assert!(f.foreign_indexing);
        assert!(f.lock_timeout);
        lock::release(&store, foreign).unwrap();
    }

    #[tokio::test]
    async fn file_change_triggers_a_refresh() {
        let (dir, state, handle) = setup();
        refresh_once(&state, &handle).await;
        let _debouncer = start(state.clone(), handle.clone()).unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        std::fs::write(dir.path().join("src/util/log.ts"), "export function logRenamed(): void {}\n").unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let store = Store::open_read_only(&dir.path().join(".singularrag/index.db")).unwrap();
            let n: i64 = store.conn().query_row("SELECT COUNT(*) FROM symbols WHERE name = 'logRenamed'", [], |r| r.get(0)).unwrap();
            if n == 1 { break; }
            assert!(std::time::Instant::now() < deadline, "watcher never reindexed the changed file");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p singularrag serve::watcher`
Expected: compile errors.

- [ ] **Step 3: Implement**

```rust
//! File watcher → one `Job::Refresh` per debounced batch → freshness snapshot + SSE.
//! `lock_timeout` means another process (the MCP server) is indexing: reported, retried
//! once, never an error (spec §2).

use std::path::Path;
use std::time::Duration;

use notify::RecursiveMode;
use notify_debouncer_full::{new_debouncer, DebounceEventResult, Debouncer, RecommendedCache};
use singularrag_core::index::IndexStats;
use singularrag_core::time::now_ms;
use tokio::sync::mpsc;

use crate::actor::EngineHandle;
use super::state::{AppState, ServerEvent};

pub const DEBOUNCE: Duration = Duration::from_millis(300);
pub const LOCK_RETRY_AFTER: Duration = Duration::from_secs(2);

pub fn apply(state: &AppState, stats: IndexStats) {
    let snapshot = {
        let mut f = match state.freshness.write() {
            Ok(g) => g,
            Err(_) => return,
        };
        f.stale_count = stats.remaining;
        f.lock_timeout = stats.lock_timeout;
        f.foreign_indexing = stats.lock_timeout;
        f.drain.last = stats.clone();
        if !stats.lock_timeout {
            f.indexed_at_ms = Some(now_ms());
        }
        f.clone()
    };
    let _ = state.events.send(ServerEvent::Freshness(snapshot));
}

pub async fn refresh_once(state: &AppState, handle: &EngineHandle) {
    for attempt in 0..2 {
        match handle.refresh().await {
            Ok(stats) => {
                let retry = stats.lock_timeout && attempt == 0;
                apply(state, stats);
                if !retry {
                    return;
                }
                tokio::time::sleep(LOCK_RETRY_AFTER).await;
            }
            Err(e) => {
                tracing::warn!("refresh failed: {e}");
                return;
            }
        }
    }
}

fn interesting(root: &Path, p: &Path) -> bool {
    let Ok(rel) = p.strip_prefix(root) else { return false };
    !rel.starts_with(".git") && !rel.starts_with(".singularrag")
}

/// Start watching `state.root`. The returned debouncer must be kept alive.
pub fn start(state: AppState, handle: EngineHandle) -> anyhow::Result<Debouncer<notify::RecommendedWatcher, RecommendedCache>> {
    let (tx, mut rx) = mpsc::channel::<()>(4);
    let root = state.root.clone();
    let root_for_handler = root.clone();
    let mut debouncer = new_debouncer(DEBOUNCE, None, move |res: DebounceEventResult| {
        if let Ok(events) = res {
            if events.iter().any(|e| e.paths.iter().any(|p| interesting(&root_for_handler, p))) {
                let _ = tx.try_send(());
            }
        }
    })?;
    debouncer.watch(&root, RecursiveMode::Recursive)?;
    tokio::spawn(async move {
        while rx.recv().await.is_some() {
            refresh_once(&state, &handle).await;
        }
    });
    Ok(debouncer)
}
```
In `serve/mod.rs`: `pub mod watcher;`; in `run`, after `AppState::new`, spawn the actor with `SessionKey::Fixed("serve".into())` and `REFRESH_BUDGET`, call `watcher::refresh_once(&state, &handle).await` (the startup refresh), then `let _debouncer = watcher::start(state.clone(), handle.clone())?;` kept in scope until `axum::serve` returns; select the actor's `died` receiver against `axum::serve` as `mcp::run` does and exit 2 on death; on normal exit `handle.shutdown()` and join.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p singularrag serve::watcher`
Expected: 3 passed (the foreign-lock test takes ~2.5 s).

- [ ] **Step 5: Lint and commit**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test --workspace`
```bash
git add crates
git commit -m "feat(serve): notify watcher drives refresh jobs and the freshness snapshot"
```

---

### Task 8: Embedded assets, `build.rs` guard, and the integration test

**Files:**
- Create: `crates/singularrag/build.rs`, `crates/singularrag/src/serve/assets.rs`, `crates/singularrag/tests/serve.rs`
- Modify: `crates/singularrag/src/serve/mod.rs`, `crates/singularrag/Cargo.toml` (`build = "build.rs"` is implicit; nothing to add)

**Interfaces:**
- Consumes: `ui/dist` (Task 9 produces it; until then the guard's message is the deliverable, and this task's test suite is gated on `ui/dist/index.html` existing, see Step 1).
- Produces: `assets::index()` handler for `/` and `assets::asset(Path<String>)` for `/assets/{*path}`; `Cache-Control: public, max-age=31536000, immutable` on `/assets`; `Content-Type` from `mime_guess`; 404 for unknown assets. `tests/serve.rs` spawning the binary.
- Verified: rust-embed 8.12 derives `Embed` (`use rust_embed::Embed;`), `#[folder = "$CARGO_MANIFEST_DIR/../../ui/dist"]` needs the `interpolate-folder-path` feature (added in Task 4), `Asset::get(path) -> Option<EmbeddedFile>` with `.data: Cow<[u8]>`; `Metadata` has no mimetype, so use `mime_guess::from_path`. Debug builds read the folder from disk at run time; release embeds.

- [ ] **Step 1: The `build.rs` guard and a placeholder dist for the Rust-only path**

`crates/singularrag/build.rs`:
```rust
use std::path::Path;

fn main() {
    let dist = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui/dist");
    println!("cargo:rerun-if-changed={}", dist.display());
    if !dist.join("index.html").exists() {
        panic!(
            "ui/dist/index.html is missing: run `bun install && bun run build` in ui/ first (the UI is embedded into this binary)"
        );
    }
}
```
Because Tasks 9–12 build the real UI later, this task must not break `cargo build` in the meantime: create `ui/dist/index.html` containing `<!doctype html><title>singularrag</title><p>UI not built</p>` and `ui/dist/.gitkeep`, commit both (they are overwritten by the real build; `ui/dist/` stays gitignored for everything else by adding `!ui/dist/index.html` and `!ui/dist/.gitkeep` to `.gitignore`). Task 12's final commit replaces the placeholder with a real build check in CI; the placeholder never ships because release builds run `bun run build` first.

- [ ] **Step 2: Write the failing tests**

`crates/singularrag/tests/serve.rs`:
```rust
//! Drives the real binary: spawn `serve --no-open --port 0`, parse the URL line, hit the API.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use futures_util::StreamExt;
use singularrag_core::fixture::write_ts_mini;

struct Server { child: Child, url: String, token: String, port: u16 }

impl Drop for Server {
    fn drop(&mut self) { let _ = self.child.kill(); let _ = self.child.wait(); }
}

fn spawn(repo: &std::path::Path) -> Server {
    let mut child = Command::new(env!("CARGO_BIN_EXE_singularrag"))
        .args(["serve", "--no-open", "--port", "0", "--repo", repo.to_str().unwrap()])
        .env("RUST_LOG", "warn")
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    let line = line.trim().to_string();
    let (base, token) = line.split_once("/#token=").expect("url line");
    let port: u16 = base.rsplit(':').next().unwrap().parse().unwrap();
    Server { child, url: base.to_string(), token: token.to_string(), port }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().timeout(Duration::from_secs(5)).build().unwrap()
}

async fn get(s: &Server, path: &str) -> reqwest::Response {
    client().get(format!("{}/api{path}", s.url)).bearer_auth(&s.token).send().await.unwrap()
}

fn cli_query(repo: &std::path::Path, q: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_singularrag")).args(["query", q, "--budget", "4096", "--repo", repo.to_str().unwrap()]).output().unwrap();
    String::from_utf8(out.stdout).unwrap()
}

#[tokio::test]
async fn auth_and_host_controls() {
    let dir = tempfile::tempdir().unwrap();
    write_ts_mini(dir.path());
    let s = spawn(dir.path());
    let r = client().get(format!("{}/api/status", s.url)).send().await.unwrap();
    assert_eq!(r.status(), 401);
    let r = client().get(format!("http://localhost:{}/api/status", s.port)).bearer_auth(&s.token).send().await.unwrap();
    assert_eq!(r.status(), 200, "localhost:<port> is an allowed host");
    let r = client().get(format!("{}/api/status", s.url)).header("Host", "evil.example").bearer_auth(&s.token).send().await.unwrap();
    assert_eq!(r.status(), 403);
    assert!(r.headers().get("access-control-allow-origin").is_none());
    let r = get(&s, "/status").await;
    assert_eq!(r.headers().get("cache-control").unwrap(), "no-store");
}

#[tokio::test]
async fn shell_and_assets_are_served_without_a_token() {
    let dir = tempfile::tempdir().unwrap();
    write_ts_mini(dir.path());
    let s = spawn(dir.path());
    let r = client().get(format!("{}/", s.url)).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.headers().get("content-type").unwrap().to_str().unwrap().starts_with("text/html"));
    assert!(r.text().await.unwrap().contains("singularrag"));
    let r = client().get(format!("{}/assets/nope.js", s.url)).send().await.unwrap();
    assert_eq!(r.status(), 404);
}

#[tokio::test]
async fn routes_have_the_documented_shapes() {
    let dir = tempfile::tempdir().unwrap();
    write_ts_mini(dir.path());
    cli_query(dir.path(), "session");
    let s = spawn(dir.path());
    let status: serde_json::Value = get(&s, "/status").await.json().await.unwrap();
    for k in ["index_version", "git_head", "indexed_at_ms", "stale_count", "lock_timeout", "foreign_indexing", "files", "drain"] {
        assert!(status.get(k).is_some(), "status missing {k}: {status}");
    }
    let rs: serde_json::Value = get(&s, "/retrievals?limit=10").await.json().await.unwrap();
    let first = &rs.as_array().unwrap()[0];
    assert_eq!(first["session_label"], "CLI");
    assert_eq!(first["tool"], "repo_map");
    let id = first["id"].as_i64().unwrap();
    let d: serde_json::Value = get(&s, &format!("/retrievals/{id}")).await.json().await.unwrap();
    assert!(d["items"].as_array().unwrap().iter().any(|i| i["served"] == true));
    assert!(d["items"][0]["reasons"]["referenced_by"].is_array());
    let tree: serde_json::Value = get(&s, "/tree").await.json().await.unwrap();
    assert!(tree.as_array().unwrap().iter().any(|f| f["path"] == "src/auth/session.ts"));
    let sk: serde_json::Value = get(&s, "/skipped").await.json().await.unwrap();
    assert!(sk.as_array().unwrap().iter().any(|f| f["path"] == ".env"));
    let map: serde_json::Value = get(&s, "/map").await.json().await.unwrap();
    assert!(map["pin"].is_array());
    assert_eq!(get(&s, "/retrievals/999999").await.status(), 404);
}

#[tokio::test]
async fn a_cli_query_produces_a_change_event() {
    let dir = tempfile::tempdir().unwrap();
    write_ts_mini(dir.path());
    let s = spawn(dir.path());
    let resp = client().get(format!("{}/api/events?token={}", s.url, s.token)).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let mut stream = resp.bytes_stream();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let repo = dir.path().to_path_buf();
    std::thread::spawn(move || cli_query(&repo, "session"));
    let mut buf = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let chunk = tokio::time::timeout_at(deadline, stream.next()).await.expect("change event within 5 s").unwrap().unwrap();
        buf.push_str(std::str::from_utf8(&chunk).unwrap());
        if buf.contains("event: change") && buf.contains("max_retrieval_id") { break; }
    }
}

#[tokio::test]
async fn exclude_via_put_map_changes_the_next_retrieval() {
    let dir = tempfile::tempdir().unwrap();
    write_ts_mini(dir.path());
    let before = cli_query(dir.path(), "");
    assert!(before.contains("src/util/log.ts:"), "{before}");
    let s = spawn(dir.path());
    let body = serde_json::json!({ "pin": [], "exclude": [{ "path": "src/util/" }], "note": [], "boundary": [], "deny": { "extra_patterns": [] } });
    let r = client().put(format!("{}/api/map", s.url)).bearer_auth(&s.token).json(&body).send().await.unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    let written = std::fs::read_to_string(dir.path().join(".singularrag/map.toml")).unwrap();
    assert!(written.starts_with("# Managed by singularrag serve"));
    let after = cli_query(dir.path(), "");
    assert!(!after.contains("src/util/log.ts:"), "{after}");
    let bad = serde_json::json!({ "pin": [{ "path": "../x" }], "exclude": [], "note": [], "boundary": [], "deny": { "extra_patterns": [] } });
    let r = client().put(format!("{}/api/map", s.url)).bearer_auth(&s.token).json(&bad).send().await.unwrap();
    assert_eq!(r.status(), 422);
    let e: serde_json::Value = r.json().await.unwrap();
    assert_eq!(e["field"], "pin[0].path");
}

#[tokio::test]
async fn touching_a_file_changes_freshness_within_two_seconds() {
    let dir = tempfile::tempdir().unwrap();
    write_ts_mini(dir.path());
    let s = spawn(dir.path());
    tokio::time::sleep(Duration::from_millis(500)).await;
    let before: serde_json::Value = get(&s, "/status").await.json().await.unwrap();
    std::fs::write(dir.path().join("src/util/log.ts"), "export function logAgain(): void {}\n").unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let now: serde_json::Value = get(&s, "/status").await.json().await.unwrap();
        if now["indexed_at_ms"] != before["indexed_at_ms"] { break; }
        assert!(tokio::time::Instant::now() < deadline, "freshness did not change");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let tree: serde_json::Value = get(&s, "/tree").await.json().await.unwrap();
    assert!(tree.to_string().contains("logAgain"));
}
```
Add `serde_json` and `futures-util` to the bin crate's dev-dependencies if not already normal deps (both are).

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p singularrag --test serve`
Expected: `/` returns 404 (assets not mounted) and other failures until Step 4.

- [ ] **Step 4: Implement assets and finish `run`**

`crates/singularrag/src/serve/assets.rs`:
```rust
//! The embedded UI. `ui/dist` is built by Bun and embedded at compile time; nothing is
//! fetched at run time. The shell and assets carry no data, so they need no token.

use axum::extract::Path;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "$CARGO_MANIFEST_DIR/../../ui/dist"]
struct Dist;

pub async fn index() -> Response {
    match Dist::get("index.html") {
        Some(f) => ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], f.data.into_owned()).into_response(),
        None => (StatusCode::NOT_FOUND, "ui not built").into_response(),
    }
}

pub async fn asset(Path(path): Path<String>) -> Response {
    let key = format!("assets/{path}");
    match Dist::get(&key) {
        Some(f) => {
            let mime = mime_guess::from_path(&key).first_or_octet_stream();
            (
                [
                    (header::CONTENT_TYPE, HeaderValue::from_str(mime.as_ref()).unwrap_or(HeaderValue::from_static("application/octet-stream"))),
                    (header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=31536000, immutable")),
                ],
                f.data.into_owned(),
            )
                .into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
```
In `serve/mod.rs`: `pub mod assets;`, and the top-level router becomes `Router::new().route("/", get(assets::index)).route("/assets/{*path}", get(assets::asset)).nest("/api", api).with_state(state)`. Bun emits assets next to `index.html` by default (not under `assets/`); set `Bun.build`'s `naming: { asset: "assets/[name]-[hash].[ext]", chunk: "assets/[name]-[hash].[ext]", entry: "assets/[name]-[hash].[ext]" }` in Task 9's `build.ts` so hashed files land under `dist/assets/`, leaving `index.html` at the root. (Note for Task 9's implementer; this task only serves what is there.)

Complete `run` per Task 7's notes (actor spawn, startup refresh, watcher, poller, `died` select, shutdown).

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p singularrag --test serve`
Expected: 6 passed. `shell_and_assets...` passes against the placeholder `index.html` (it contains "singularrag").

- [ ] **Step 6: Lint and commit**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test --workspace`
```bash
git add -A crates ui/dist/index.html ui/dist/.gitkeep .gitignore
git commit -m "feat(serve): embedded ui assets, build guard, and end-to-end api tests"
```

---
### Task 9: UI project scaffold with Bun, Tailwind v4, shadcn on Base UI

**Files:**
- Create: `ui/package.json`, `ui/tsconfig.json`, `ui/bunfig.toml`, `ui/build.ts`, `ui/dev.ts`, `ui/happydom.ts`, `ui/index.html`, `ui/src/main.tsx`, `ui/src/App.tsx`, `ui/src/index.css`, `ui/src/lib/utils.ts`, `ui/components.json`, `ui/src/components/ui/{button,badge,textarea,sheet,tooltip}.tsx` (installed via the shadcn CLI), `ui/src/App.test.tsx`
- Modify: `.gitignore` (add `ui/dist/`, `ui/node_modules/`)

**Interfaces:**
- Produces: `bun run build` → `ui/dist/index.html` + hashed assets; `bun run dev` → `Bun.serve` on 5173 with `/api/*` proxied to `http://127.0.0.1:4173`; `bun test` with happy-dom preloaded; `@/` alias to `ui/src`; `cn()` helper.
- Verified facts (2026-09-20): Bun 1.4 `Bun.build({ entrypoints: ["./index.html"], outdir, minify, plugins })` rewrites script/link tags to hashed assets (default `--asset-naming '[name]-[hash].[ext]'`); `bun-plugin-tailwind` 0.1.2 is the Bun path for Tailwind 4 (an open issue, oven-sh/bun#29603, reports `@layer` ordering problems; the fallback below is `@tailwindcss/cli`); shadcn CLI 4.x with `--base base`; the Base UI package is `@base-ui/react` (the `@base-ui-components/react` name is deprecated); a plain Bun project needs a hand-written `components.json`.

- [ ] **Step 1: Create the project**

```bash
mkdir -p ui/src/lib ui/src/components/ui && cd ui
bun init -y >/dev/null
bun add react@19 react-dom@19 react-aria-components@1 @base-ui/react class-variance-authority clsx tailwind-merge lucide-react sonner
bun add -d typescript @types/react @types/react-dom tailwindcss@4 bun-plugin-tailwind @happy-dom/global-registrator @testing-library/react@16 @testing-library/user-event@14 @testing-library/jest-dom axe-core @types/bun
```
Replace `ui/package.json`'s `scripts` with:
```json
{
  "scripts": {
    "build": "bun run build.ts",
    "dev": "bun --hot dev.ts",
    "test": "bun test",
    "typecheck": "tsc --noEmit"
  }
}
```
`ui/tsconfig.json`:
```json
{
  "compilerOptions": {
    "target": "ES2022", "lib": ["ES2022", "DOM", "DOM.Iterable"], "jsx": "react-jsx",
    "module": "ESNext", "moduleResolution": "bundler", "strict": true, "noEmit": true,
    "skipLibCheck": true, "types": ["bun-types"],
    "baseUrl": ".", "paths": { "@/*": ["src/*"] }
  },
  "include": ["src", "build.ts", "dev.ts", "happydom.ts"]
}
```
`ui/bunfig.toml`:
```toml
[serve.static]
plugins = ["bun-plugin-tailwind"]

[test]
preload = ["./happydom.ts"]
```
`ui/happydom.ts`:
```ts
import { GlobalRegistrator } from "@happy-dom/global-registrator";
GlobalRegistrator.register();
```
`ui/build.ts`:
```ts
import tailwind from "bun-plugin-tailwind";

const result = await Bun.build({
  entrypoints: ["./index.html"],
  outdir: "./dist",
  minify: true,
  sourcemap: "none",
  plugins: [tailwind],
});
if (!result.success) {
  for (const log of result.logs) console.error(log);
  process.exit(1);
}
console.log(`built ${result.outputs.length} files into ui/dist`);
```
`ui/dev.ts`:
```ts
import homepage from "./index.html";

const API = process.env.SINGULARRAG_API ?? "http://127.0.0.1:4173";

Bun.serve({
  port: 5173,
  routes: { "/": homepage },
  async fetch(req) {
    const url = new URL(req.url);
    if (url.pathname.startsWith("/api/")) {
      return fetch(`${API}${url.pathname}${url.search}`, {
        method: req.method,
        headers: req.headers,
        body: req.body,
      });
    }
    return new Response("Not found", { status: 404 });
  },
});
console.log("ui dev server on http://localhost:5173 (api → " + API + ")");
```
`ui/index.html`:
```html
<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <meta name="color-scheme" content="light dark" />
    <title>singularrag</title>
    <link rel="stylesheet" href="./src/index.css" />
  </head>
  <body>
    <div id="root"></div>
    <script type="module" src="./src/main.tsx"></script>
  </body>
</html>
```
`ui/src/index.css` (shadcn's Tailwind v4 token block, neutral base, both schemes):
```css
@import "tailwindcss";

@custom-variant dark (&:is(.dark *));

:root {
  --background: oklch(1 0 0);
  --foreground: oklch(0.145 0 0);
  --card: oklch(1 0 0);
  --card-foreground: oklch(0.145 0 0);
  --popover: oklch(1 0 0);
  --popover-foreground: oklch(0.145 0 0);
  --primary: oklch(0.205 0 0);
  --primary-foreground: oklch(0.985 0 0);
  --secondary: oklch(0.97 0 0);
  --secondary-foreground: oklch(0.205 0 0);
  --muted: oklch(0.97 0 0);
  --muted-foreground: oklch(0.556 0 0);
  --accent: oklch(0.97 0 0);
  --accent-foreground: oklch(0.205 0 0);
  --destructive: oklch(0.577 0.245 27.325);
  --border: oklch(0.922 0 0);
  --input: oklch(0.922 0 0);
  --ring: oklch(0.708 0 0);
  --radius: 0.5rem;
}

@media (prefers-color-scheme: dark) {
  :root {
    --background: oklch(0.145 0 0);
    --foreground: oklch(0.985 0 0);
    --card: oklch(0.205 0 0);
    --card-foreground: oklch(0.985 0 0);
    --popover: oklch(0.205 0 0);
    --popover-foreground: oklch(0.985 0 0);
    --primary: oklch(0.922 0 0);
    --primary-foreground: oklch(0.205 0 0);
    --secondary: oklch(0.269 0 0);
    --secondary-foreground: oklch(0.985 0 0);
    --muted: oklch(0.269 0 0);
    --muted-foreground: oklch(0.708 0 0);
    --accent: oklch(0.269 0 0);
    --accent-foreground: oklch(0.985 0 0);
    --destructive: oklch(0.704 0.191 22.216);
    --border: oklch(1 0 0 / 10%);
    --input: oklch(1 0 0 / 15%);
    --ring: oklch(0.556 0 0);
  }
}

@theme inline {
  --color-background: var(--background);
  --color-foreground: var(--foreground);
  --color-card: var(--card);
  --color-card-foreground: var(--card-foreground);
  --color-popover: var(--popover);
  --color-popover-foreground: var(--popover-foreground);
  --color-primary: var(--primary);
  --color-primary-foreground: var(--primary-foreground);
  --color-secondary: var(--secondary);
  --color-secondary-foreground: var(--secondary-foreground);
  --color-muted: var(--muted);
  --color-muted-foreground: var(--muted-foreground);
  --color-accent: var(--accent);
  --color-accent-foreground: var(--accent-foreground);
  --color-destructive: var(--destructive);
  --color-border: var(--border);
  --color-input: var(--input);
  --color-ring: var(--ring);
  --radius-sm: calc(var(--radius) - 4px);
  --radius-md: calc(var(--radius) - 2px);
  --radius-lg: var(--radius);
}

@layer base {
  * { @apply border-border outline-ring/50; }
  body { @apply bg-background text-foreground; }
  :focus-visible { @apply outline-2 outline-offset-2 outline-ring; }
}
```
`ui/src/lib/utils.ts`:
```ts
import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";
export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}
```
`ui/components.json`:
```json
{
  "$schema": "https://ui.shadcn.com/schema.json",
  "style": "new-york",
  "rsc": false,
  "tsx": true,
  "tailwind": { "config": "", "css": "src/index.css", "baseColor": "neutral", "cssVariables": true, "prefix": "" },
  "aliases": { "components": "@/components", "utils": "@/lib/utils", "ui": "@/components/ui", "lib": "@/lib", "hooks": "@/hooks" }
}
```
Then install the components with the CLI (Base UI): `bunx shadcn@latest add --base base button badge textarea sheet tooltip`. If the CLI cannot detect a framework and refuses, run `bunx shadcn@latest init --base base --yes` first (it may ask to overwrite `components.json`; keep ours). If `--base base` is rejected, run `bunx shadcn@latest init --help` and use the flag value it lists for Base UI; record it in the report.

`ui/src/main.tsx`:
```tsx
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
```
`ui/src/App.tsx` (a shell; later tasks fill it):
```tsx
export function App() {
  return (
    <main className="min-h-screen p-4">
      <h1 className="text-xl font-semibold">singularrag</h1>
    </main>
  );
}
```
Add to the repo root `.gitignore`: `ui/dist/` and `ui/node_modules/`.

- [ ] **Step 2: Write the smoke test**

`ui/src/App.test.tsx`:
```tsx
import { describe, expect, test } from "bun:test";
import { render, screen } from "@testing-library/react";
import { App } from "./App";

describe("App", () => {
  test("renders the heading", () => {
    render(<App />);
    expect(screen.getByRole("heading", { name: "singularrag" })).toBeTruthy();
  });
});
```

- [ ] **Step 3: Run build, test and typecheck**

Run (from `ui/`): `bun run typecheck && bun test && bun run build && ls dist`
Expected: typecheck clean; 1 test passed; `dist/index.html` plus hashed `.js`/`.css` under `dist/`. Open `dist/index.html`'s text and confirm the script/link paths point at hashed files. If `bun-plugin-tailwind` produces CSS where utility classes are overridden by base styles (the oven-sh/bun#29603 symptom), switch: `bun add -d @tailwindcss/cli`, change the `build` script to `bunx @tailwindcss/cli -i src/index.css -o src/tailwind.out.css --minify && bun run build.ts`, point `index.html` at `./src/tailwind.out.css`, drop the plugin from `build.ts` and `bunfig.toml`, and gitignore `src/tailwind.out.css`; record the switch in the report.

- [ ] **Step 4: Commit**

```bash
git add .gitignore ui
git commit -m "feat(ui): bun-only react scaffold with tailwind v4 and shadcn on base ui"
```

---

### Task 10: API client, types, `reasonsToSentences`, session labels, status encoding

**Files:**
- Create: `ui/src/api/types.ts`, `ui/src/api/client.ts`, `ui/src/lib/reasons.ts`, `ui/src/lib/reasons.test.ts`, `ui/src/lib/status.tsx`, `ui/src/lib/status.test.tsx`, `ui/src/api/events.ts`

**Interfaces:**
- Consumes: the `/api` JSON shapes from Tasks 6–8 (`Status`, `RetrievalSummary`, `RetrievalDetail`, `Item`, `Reasons`, `TreeFile`, `TreeSymbol`, `SkippedFile`, `MapConfig`) and the SSE events `change`/`freshness`.
- Produces: `api.status()`, `api.retrievals(before?)`, `api.retrieval(id)`, `api.tree()`, `api.skipped()`, `api.map()`, `api.saveMap(cfg)` (throws `ApiError { status, message, field? }`); `tokenFromFragment(): string`; `subscribe(onChange, onFreshness): () => void`; `reasonsToSentences(r: Reasons, rank: number, score: number): string[]`; `<StatusMark status="served"|"cut"|"untouched" />` rendering label + icon + colour class; `statusLabel()`.

- [ ] **Step 1: Write the failing tests**

`ui/src/lib/reasons.test.ts`:
```ts
import { describe, expect, test } from "bun:test";
import { reasonsToSentences } from "./reasons";

const base = { score: 0.11, file_rank: 0.2, seeds: [], referenced_by: [], pinned: false, fts_hit: false, query_ident_match: false };

describe("reasonsToSentences", () => {
  test("rank and score always come first", () => {
    expect(reasonsToSentences(base, 3, 0.11)[0]).toBe("Ranked 3rd, score 0.11.");
    expect(reasonsToSentences(base, 1, 0.5)[0]).toBe("Ranked 1st, score 0.50.");
    expect(reasonsToSentences(base, 22, 0.001)[0]).toBe("Ranked 22nd, score 0.00.");
  });
  test("references list up to five files with counts", () => {
    const s = reasonsToSentences({ ...base, referenced_by: [{ path: "src/http/middleware.ts", count: 2 }, { path: "src/cli/login.ts", count: 1 }] }, 1, 0.1);
    expect(s).toContain("Referenced from middleware.ts (2) and login.ts (1).");
  });
  test("query match sentences", () => {
    expect(reasonsToSentences({ ...base, query_ident_match: true, seeds: ["query:session store"] }, 1, 0.1)).toContain("Matched the query on session store.");
    expect(reasonsToSentences({ ...base, fts_hit: true }, 1, 0.1)).toContain("Matched the query in the index.");
  });
  test("focus and pinned", () => {
    const s = reasonsToSentences({ ...base, seeds: ["focus", "pinned"], pinned: true }, 1, 0.1);
    expect(s).toContain("You focused this file.");
    expect(s).toContain("Pinned.");
  });
  test("nothing extra when nothing applies", () => {
    expect(reasonsToSentences(base, 4, 0.02)).toEqual(["Ranked 4th, score 0.02."]);
  });
});
```
`ui/src/lib/status.test.tsx`:
```tsx
import { describe, expect, test } from "bun:test";
import { render } from "@testing-library/react";
import { StatusMark, statusLabel } from "./status";

describe("StatusMark", () => {
  test("encodes status as text, icon and colour", () => {
    for (const s of ["served", "cut", "untouched"] as const) {
      const { container, unmount } = render(<StatusMark status={s} />);
      const el = container.firstElementChild!;
      expect(el.textContent).toContain(statusLabel(s));
      expect(el.querySelector("svg")!.getAttribute("data-shape")).toBe({ served: "filled", cut: "outlined", untouched: "dash" }[s]);
      expect(el.className).toContain(`status-${s}`);
      unmount();
    }
  });
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run (from `ui/`): `bun test`
Expected: two files fail to import `./reasons` / `./status`.

- [ ] **Step 3: Implement**

`ui/src/api/types.ts`:
```ts
export type Reasons = {
  score: number; file_rank: number; seeds: string[];
  referenced_by: { path: string; count: number }[];
  pinned: boolean; fts_hit: boolean; query_ident_match: boolean;
};
export type Item = { rank: number; symbol_id: number; path: string; name: string; line_start: number; score: number; served: boolean; reasons: Reasons };
export type RetrievalSummary = {
  id: number; session_key: string; session_label: string; tool: string; query: string | null;
  focus_files: string[]; budget: number | null; limit_n: number | null; index_version: string;
  git_head: string | null; stale_count: number; created_at_ms: number; served: number; cut: number;
};
export type RetrievalDetail = RetrievalSummary & { items: Item[] };
export type TreeSymbol = { id: number; name: string; kind: string; line_start: number; line_end: number; signature: string };
export type TreeFile = { path: string; lang: string | null; skipped_reason: string | null; symbols: TreeSymbol[] };
export type SkippedFile = { path: string; reason: string };
export type Target = { path: string; symbol?: string };
export type Note = { path: string; symbol?: string; text: string };
export type Boundary = { name: string; paths: string[] };
export type MapConfig = { pin: Target[]; exclude: Target[]; note: Note[]; boundary: Boundary[]; deny: { extra_patterns: string[] } };
export type Status = {
  index_version: string; git_head: string | null; indexed_at_ms: number | null; stale_count: number;
  lock_timeout: boolean; foreign_indexing: boolean; files: { indexed: number; skipped: number };
  drain: { chunks: number; last: { scanned: number; indexed: number; unchanged: number; skipped: number; removed: number; remaining: number; lock_timeout: boolean } };
};
```
`ui/src/api/client.ts`:
```ts
import type { MapConfig, RetrievalDetail, RetrievalSummary, SkippedFile, Status, TreeFile } from "./types";

export class ApiError extends Error {
  constructor(public status: number, message: string, public field?: string) { super(message); }
}

export function tokenFromFragment(): string {
  const m = /(?:^#|&)token=([0-9a-f]+)/.exec(window.location.hash);
  return m?.[1] ?? "";
}

let token = "";
export function setToken(t: string) { token = t; }
export function getToken() { return token; }

async function req<T>(method: string, path: string, body?: unknown): Promise<T> {
  const res = await fetch(`/api${path}`, {
    method,
    headers: { Authorization: `Bearer ${token}`, ...(body ? { "Content-Type": "application/json" } : {}) },
    body: body ? JSON.stringify(body) : undefined,
  });
  if (!res.ok) {
    let msg = res.statusText, field: string | undefined;
    try { const j = await res.json(); msg = j.error ?? msg; field = j.field; } catch {}
    throw new ApiError(res.status, msg, field);
  }
  return (await res.json()) as T;
}

export const api = {
  status: () => req<Status>("GET", "/status"),
  retrievals: (before?: number) => req<RetrievalSummary[]>("GET", `/retrievals?limit=50${before ? `&before=${before}` : ""}`),
  retrieval: (id: number) => req<RetrievalDetail>("GET", `/retrievals/${id}`),
  tree: () => req<TreeFile[]>("GET", "/tree"),
  skipped: () => req<SkippedFile[]>("GET", "/skipped"),
  map: () => req<MapConfig>("GET", "/map"),
  saveMap: (cfg: MapConfig) => req<MapConfig>("PUT", "/map", cfg),
};
```
`ui/src/api/events.ts`:
```ts
import { getToken } from "./client";
import type { Status } from "./types";

export function subscribe(onChange: (maxId: number) => void, onFreshness: (s: Status) => void): () => void {
  const es = new EventSource(`/api/events?token=${getToken()}`);
  es.addEventListener("change", (e) => onChange(JSON.parse((e as MessageEvent).data).max_retrieval_id));
  es.addEventListener("freshness", (e) => onFreshness(JSON.parse((e as MessageEvent).data)));
  return () => es.close();
}
```
`ui/src/lib/reasons.ts`:
```ts
import type { Reasons } from "@/api/types";

function ordinal(n: number): string {
  const s = ["th", "st", "nd", "rd"], v = n % 100;
  return n + (s[(v - 20) % 10] ?? s[v] ?? s[0]);
}
const base = (p: string) => p.split("/").pop() ?? p;
function list(parts: string[]): string {
  return parts.length <= 1 ? parts.join("") : `${parts.slice(0, -1).join(", ")} and ${parts[parts.length - 1]}`;
}

/** Every field of `Reasons` maps to a fixed phrase or nothing; the panel can never show a reason the data does not carry. */
export function reasonsToSentences(r: Reasons, rank: number, score: number): string[] {
  const out = [`Ranked ${ordinal(rank)}, score ${score.toFixed(2)}.`];
  if (r.referenced_by.length) {
    out.push(`Referenced from ${list(r.referenced_by.slice(0, 5).map((x) => `${base(x.path)} (${x.count})`))}.`);
  }
  const q = r.seeds.find((s) => s.startsWith("query:"));
  if (r.query_ident_match && q) out.push(`Matched the query on ${q.slice("query:".length)}.`);
  else if (r.query_ident_match || r.fts_hit) out.push("Matched the query in the index.");
  if (r.seeds.includes("focus")) out.push("You focused this file.");
  if (r.pinned || r.seeds.includes("pinned")) out.push("Pinned.");
  return out;
}
```
`ui/src/lib/status.tsx`:
```tsx
export type ItemStatus = "served" | "cut" | "untouched";

export function statusLabel(s: ItemStatus): string {
  return { served: "Served", cut: "Cut", untouched: "Untouched" }[s];
}

/** Three encodings at once: text, icon shape, colour. Never colour alone (spec §4). */
export function StatusMark({ status }: { status: ItemStatus }) {
  const shape = { served: "filled", cut: "outlined", untouched: "dash" }[status];
  const colour = { served: "text-emerald-700 dark:text-emerald-400", cut: "text-amber-700 dark:text-amber-400", untouched: "text-muted-foreground" }[status];
  return (
    <span className={`status-${status} inline-flex items-center gap-1 ${colour}`}>
      <svg data-shape={shape} aria-hidden="true" width="12" height="12" viewBox="0 0 12 12">
        {shape === "filled" && <circle cx="6" cy="6" r="5" fill="currentColor" />}
        {shape === "outlined" && <circle cx="6" cy="6" r="4.5" fill="none" stroke="currentColor" strokeWidth="1.5" />}
        {shape === "dash" && <rect x="2" y="5" width="8" height="2" fill="currentColor" />}
      </svg>
      <span>{statusLabel(status)}</span>
    </span>
  );
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run (from `ui/`): `bun test && bun run typecheck`
Expected: all pass. The `22nd` ordinal case guards the `-20 % 10` trick.

- [ ] **Step 5: Commit**

```bash
git add ui
git commit -m "feat(ui): api client, reasons as sentences, three-way status mark"
```

---

### Task 11: Treegrid (react-aria-components) with status join, filter and row actions

**Files:**
- Create: `ui/src/components/RepoTree.tsx`, `ui/src/components/RepoTree.test.tsx`, `ui/src/lib/join.ts`, `ui/src/lib/join.test.ts`

**Interfaces:**
- Consumes: `TreeFile`, `TreeSymbol`, `Item`, `StatusMark`, `MapConfig` (for pinned/excluded badges).
- Produces: `joinRetrieval(files: TreeFile[], items: Item[] | null): FileRow[]` where `FileRow = { path, lang, served, cut, expandedByDefault, symbols: SymbolRow[] }`, `SymbolRow = { key, symbol, status, item: Item | null }`; `<RepoTree files rows filter onFocusRow onAction />` rendering `react-aria-components` `<Tree aria-label="Repository">` (it renders `role="treegrid"`), file rows expandable to symbol rows, sortable by status/score/path via header buttons, a row action button ("Actions") that calls `onAction(row)`; `onFocusRow(row)` fires on focus change.
- Verified (2026-09-20): react-aria-components 1.21 exports `Tree`, `TreeItem`, `TreeItemContent`, `Collection`, `Button`; `<Button slot="chevron">` inside `TreeItemContent` toggles expansion; `aria-label` is required on `Tree`; `selectionMode="single"`. `@react-aria/test-utils` is a release candidate (`1.0.0-rc.1`); the tests below use `@testing-library/user-event` keyboard events directly rather than depend on it.

- [ ] **Step 1: Write the failing tests**

`ui/src/lib/join.test.ts`:
```ts
import { describe, expect, test } from "bun:test";
import { joinRetrieval } from "./join";
import type { Item, TreeFile } from "@/api/types";

const files: TreeFile[] = [
  { path: "src/a.ts", lang: "typescript", skipped_reason: null, symbols: [
    { id: 1, name: "f", kind: "function", line_start: 1, line_end: 3, signature: "export function f()" },
    { id: 2, name: "g", kind: "function", line_start: 5, line_end: 7, signature: "export function g()" },
  ] },
  { path: "src/b.ts", lang: "typescript", skipped_reason: null, symbols: [
    { id: 3, name: "h", kind: "function", line_start: 1, line_end: 2, signature: "export function h()" },
  ] },
];
const r = { score: 0, file_rank: 0, seeds: [], referenced_by: [], pinned: false, fts_hit: false, query_ident_match: false };
const items: Item[] = [
  { rank: 1, symbol_id: 1, path: "src/a.ts", name: "f", line_start: 1, score: 0.5, served: true, reasons: r },
  { rank: 2, symbol_id: 2, path: "src/a.ts", name: "g", line_start: 5, score: 0.1, served: false, reasons: r },
];

describe("joinRetrieval", () => {
  test("without a retrieval every symbol is untouched and nothing is expanded", () => {
    const rows = joinRetrieval(files, null);
    expect(rows.map((f) => f.path)).toEqual(["src/a.ts", "src/b.ts"]);
    expect(rows[0].symbols.every((s) => s.status === "untouched")).toBe(true);
    expect(rows.every((f) => !f.expandedByDefault)).toBe(true);
  });
  test("with a retrieval, items join by path+name+line and touched files come first, expanded", () => {
    const rows = joinRetrieval(files, items);
    expect(rows[0].path).toBe("src/a.ts");
    expect(rows[0].served).toBe(1);
    expect(rows[0].cut).toBe(1);
    expect(rows[0].expandedByDefault).toBe(true);
    expect(rows[0].symbols.map((s) => s.status)).toEqual(["served", "cut"]);
    expect(rows[1].expandedByDefault).toBe(false);
    expect(rows[1].symbols[0].status).toBe("untouched");
  });
});
```
`ui/src/components/RepoTree.test.tsx`:
```tsx
import { describe, expect, test } from "bun:test";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { RepoTree } from "./RepoTree";
import { joinRetrieval } from "@/lib/join";
import type { TreeFile } from "@/api/types";

const files: TreeFile[] = [
  { path: "src/a.ts", lang: "typescript", skipped_reason: null, symbols: [{ id: 1, name: "f", kind: "function", line_start: 1, line_end: 3, signature: "export function f()" }] },
  { path: "src/b.ts", lang: "typescript", skipped_reason: null, symbols: [{ id: 2, name: "h", kind: "function", line_start: 1, line_end: 2, signature: "export function h()" }] },
];

describe("RepoTree", () => {
  test("renders a treegrid with file rows, expands with the keyboard, and reaches the action button", async () => {
    const user = userEvent.setup();
    const actions: string[] = [];
    render(<RepoTree rows={joinRetrieval(files, null)} filter="" onFocusRow={() => {}} onAction={(r) => actions.push(r.path)} />);
    const grid = screen.getByRole("treegrid", { name: "Repository" });
    const rows = within(grid).getAllByRole("row");
    expect(rows.length).toBe(2);
    await user.click(within(rows[0]).getByText("src/a.ts"));
    await user.keyboard("{ArrowRight}");
    expect(within(grid).getAllByRole("row").length).toBe(3);
    expect(within(grid).getByText("f")).toBeTruthy();
    await user.keyboard("{ArrowDown}");
    await user.keyboard("{ArrowRight}");
    await user.keyboard("{Enter}");
    expect(actions).toEqual(["src/a.ts"]);
  });
  test("filter narrows by path or symbol name", () => {
    render(<RepoTree rows={joinRetrieval(files, null)} filter="h" onFocusRow={() => {}} onAction={() => {}} />);
    const grid = screen.getByRole("treegrid", { name: "Repository" });
    expect(within(grid).queryByText("src/a.ts")).toBeNull();
    expect(within(grid).getByText("src/b.ts")).toBeTruthy();
  });
  test("has no axe violations", async () => {
    const { container } = render(<RepoTree rows={joinRetrieval(files, null)} filter="" onFocusRow={() => {}} onAction={() => {}} />);
    const results = await axe.run(container);
    expect(results.violations).toEqual([]);
  });
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run (from `ui/`): `bun test`
Expected: import failures for `./join` and `./RepoTree`.

- [ ] **Step 3: Implement**

`ui/src/lib/join.ts`:
```ts
import type { Item, TreeFile, TreeSymbol } from "@/api/types";
import type { ItemStatus } from "./status";

export type SymbolRow = { key: string; symbol: TreeSymbol; status: ItemStatus; item: Item | null };
export type FileRow = { path: string; lang: string | null; served: number; cut: number; expandedByDefault: boolean; symbols: SymbolRow[] };

export function joinRetrieval(files: TreeFile[], items: Item[] | null): FileRow[] {
  const byKey = new Map<string, Item>();
  for (const it of items ?? []) byKey.set(`${it.path}::${it.name}::${it.line_start}`, it);
  const rows = files.map<FileRow>((f) => {
    let served = 0, cut = 0;
    const symbols = f.symbols.map<SymbolRow>((s) => {
      const key = `${f.path}::${s.name}::${s.line_start}`;
      const item = byKey.get(key) ?? null;
      const status: ItemStatus = item ? (item.served ? "served" : "cut") : "untouched";
      if (item) item.served ? served++ : cut++;
      return { key, symbol: s, status, item };
    });
    return { path: f.path, lang: f.lang, served, cut, expandedByDefault: served + cut > 0, symbols };
  });
  if (items) rows.sort((a, b) => Number(b.expandedByDefault) - Number(a.expandedByDefault) || a.path.localeCompare(b.path));
  return rows;
}
```
`ui/src/components/RepoTree.tsx`:
```tsx
import { useMemo, useState } from "react";
import { Button, Collection, Tree, TreeItem, TreeItemContent, type Key } from "react-aria-components";
import { ChevronRight, MoreHorizontal } from "lucide-react";
import { StatusMark } from "@/lib/status";
import type { FileRow, SymbolRow } from "@/lib/join";
import { cn } from "@/lib/utils";

export type TreeRow = { kind: "file"; path: string; file: FileRow } | { kind: "symbol"; path: string; file: FileRow; symbol: SymbolRow };
type SortKey = "path" | "status" | "score";
const statusOrder = { served: 0, cut: 1, untouched: 2 } as const;

export function RepoTree({ rows, filter, onFocusRow, onAction }: {
  rows: FileRow[]; filter: string; onFocusRow: (row: TreeRow) => void; onAction: (row: TreeRow) => void;
}) {
  const [sort, setSort] = useState<SortKey>("path");
  const [expanded, setExpanded] = useState<Set<Key>>(() => new Set(rows.filter((r) => r.expandedByDefault).map((r) => r.path)));

  const visible = useMemo(() => {
    const q = filter.trim().toLowerCase();
    const filtered = q
      ? rows.map((f) => ({ ...f, symbols: f.symbols.filter((s) => s.symbol.name.toLowerCase().includes(q)) }))
          .filter((f) => f.path.toLowerCase().includes(q) || f.symbols.length > 0)
      : rows;
    const sorted = [...filtered];
    if (sort === "path") sorted.sort((a, b) => a.path.localeCompare(b.path));
    if (sort === "status") sorted.sort((a, b) => (b.served + b.cut) - (a.served + a.cut) || a.path.localeCompare(b.path));
    if (sort === "score") sorted.sort((a, b) => Math.max(0, ...b.symbols.map((s) => s.item?.score ?? 0)) - Math.max(0, ...a.symbols.map((s) => s.item?.score ?? 0)));
    return sorted.map((f) => ({
      ...f,
      symbols: sort === "path" ? [...f.symbols].sort((a, b) => a.symbol.line_start - b.symbol.line_start)
        : sort === "status" ? [...f.symbols].sort((a, b) => statusOrder[a.status] - statusOrder[b.status] || a.symbol.line_start - b.symbol.line_start)
        : [...f.symbols].sort((a, b) => (b.item?.score ?? 0) - (a.item?.score ?? 0)),
    }));
  }, [rows, filter, sort]);

  const header = (k: SortKey, label: string) => (
    <button type="button" className={cn("px-2 py-1 text-left text-xs font-medium", sort === k && "underline")} aria-pressed={sort === k} onClick={() => setSort(k)}>
      Sort by {label}
    </button>
  );

  return (
    <div className="flex flex-col">
      <div className="flex gap-2 border-b" role="group" aria-label="Sort">{header("path", "path")}{header("status", "status")}{header("score", "score")}</div>
      <Tree
        aria-label="Repository"
        selectionMode="single"
        expandedKeys={expanded}
        onExpandedChange={(keys) => setExpanded(new Set(keys))}
        items={visible}
        className="outline-none"
      >
        {(file) => (
          <TreeItem id={file.path} textValue={file.path} className="outline-none data-[focused]:bg-accent">
            <TreeItemContent>
              <div className="flex min-h-6 items-center gap-2 px-2 py-1" onFocus={() => onFocusRow({ kind: "file", path: file.path, file })}>
                <Button slot="chevron" className="size-6 rounded data-[focused]:outline-2" aria-label={`Toggle ${file.path}`}>
                  <ChevronRight className="size-4 transition-none data-[expanded]:rotate-90" aria-hidden="true" />
                </Button>
                <span className="font-mono text-sm">{file.path}</span>
                <span className="text-xs text-muted-foreground">{file.lang ?? ""}</span>
                {file.served + file.cut > 0 && <span className="text-xs">{file.served} served · {file.cut} cut</span>}
                <Button className="ml-auto size-6 rounded" aria-label={`Actions for ${file.path}`} onPress={() => onAction({ kind: "file", path: file.path, file })}>
                  <MoreHorizontal className="size-4" aria-hidden="true" />
                </Button>
              </div>
            </TreeItemContent>
            <Collection items={file.symbols}>
              {(s) => (
                <TreeItem id={s.key} textValue={`${s.symbol.name} ${s.symbol.kind}`} className="outline-none data-[focused]:bg-accent">
                  <TreeItemContent>
                    <div className="flex min-h-6 items-center gap-2 py-1 pl-10 pr-2" onFocus={() => onFocusRow({ kind: "symbol", path: file.path, file, symbol: s })}>
                      <span className="font-mono text-sm">{s.symbol.name}</span>
                      <span className="text-xs text-muted-foreground">{s.symbol.kind} · line {s.symbol.line_start}</span>
                      <StatusMark status={s.status} />
                      {s.item && <span className="text-xs tabular-nums">{s.item.score.toFixed(2)}</span>}
                      <span className="truncate text-xs text-muted-foreground" title={s.symbol.signature}>{s.symbol.signature}</span>
                      <Button className="ml-auto size-6 rounded" aria-label={`Actions for ${s.symbol.name}`} onPress={() => onAction({ kind: "symbol", path: file.path, file, symbol: s })}>
                        <MoreHorizontal className="size-4" aria-hidden="true" />
                      </Button>
                    </div>
                  </TreeItemContent>
                </TreeItem>
              )}
            </Collection>
          </TreeItem>
        )}
      </Tree>
    </div>
  );
}
```
Nested `Collection` + `TreeItem` children for symbols is the documented pattern for dynamic trees. If the keyboard test fails because `{ArrowRight}` on an expanded row moves focus to the chevron rather than the next focusable in the row, adjust the test to `{ArrowRight}{ArrowRight}` (chevron → actions button) and note it in the report; the requirement is that the action button is reachable by keyboard from the row, not the exact key count.

- [ ] **Step 4: Run the tests to verify they pass**

Run (from `ui/`): `bun test && bun run typecheck`
Expected: all pass; the axe test is clean. If axe reports `aria-required-children` on the treegrid, react-aria's rendering is correct and the rule is fine; check whether `Collection` rendered `role="row"` children inside `role="rowgroup"` and, if not, upgrade `react-aria-components` to the latest 1.21.x patch.

- [ ] **Step 5: Commit**

```bash
git add ui
git commit -m "feat(ui): accessible repo treegrid with retrieval join, sort, filter and row actions"
```

---

### Task 12: Rail, panel, badge, skipped sheet, live updates, annotation loop; `App` wiring

**Files:**
- Create: `ui/src/components/RetrievalsRail.tsx`, `ui/src/components/DetailPanel.tsx`, `ui/src/components/FreshnessBadge.tsx`, `ui/src/components/SkippedSheet.tsx`, `ui/src/components/LiveRegion.tsx`, `ui/src/lib/mapEdits.ts`, `ui/src/lib/mapEdits.test.ts`, `ui/src/App.test.tsx` (replace)
- Modify: `ui/src/App.tsx`

**Interfaces:**
- Consumes: everything from Tasks 10–11.
- Produces: `togglePin(cfg, path, symbol?)`, `toggleExclude(cfg, path)`, `setNote(cfg, path, symbol, text)` → new `MapConfig` (pure); `<App />` composing rail, treegrid, panel, badge, sheet and the live region; announcements on each new retrieval and freshness change; toast "Saved. Applies to the next retrieval." on save.

- [ ] **Step 1: Write the failing tests**

`ui/src/lib/mapEdits.test.ts`:
```ts
import { describe, expect, test } from "bun:test";
import { isExcluded, isPinned, setNote, togglePin, toggleExclude } from "./mapEdits";
import type { MapConfig } from "@/api/types";

const empty: MapConfig = { pin: [], exclude: [], note: [], boundary: [], deny: { extra_patterns: [] } };

describe("map edits", () => {
  test("pin toggles at file and symbol level", () => {
    let c = togglePin(empty, "src/a.ts");
    expect(c.pin).toEqual([{ path: "src/a.ts" }]);
    expect(isPinned(c, "src/a.ts")).toBe(true);
    c = togglePin(c, "src/a.ts", "f");
    expect(c.pin).toEqual([{ path: "src/a.ts" }, { path: "src/a.ts", symbol: "f" }]);
    c = togglePin(c, "src/a.ts");
    expect(c.pin).toEqual([{ path: "src/a.ts", symbol: "f" }]);
  });
  test("exclude toggles and does not mutate the input", () => {
    const c = toggleExclude(empty, "src/legacy/");
    expect(c.exclude).toEqual([{ path: "src/legacy/" }]);
    expect(isExcluded(c, "src/legacy/")).toBe(true);
    expect(empty.exclude).toEqual([]);
    expect(toggleExclude(c, "src/legacy/").exclude).toEqual([]);
  });
  test("setNote replaces, and an empty text removes", () => {
    let c = setNote(empty, "src/a.ts", undefined, "hello");
    expect(c.note).toEqual([{ path: "src/a.ts", text: "hello" }]);
    c = setNote(c, "src/a.ts", undefined, "again");
    expect(c.note).toEqual([{ path: "src/a.ts", text: "again" }]);
    c = setNote(c, "src/a.ts", undefined, "");
    expect(c.note).toEqual([]);
  });
});
```
`ui/src/App.test.tsx` (replace):
```tsx
import { afterEach, beforeEach, describe, expect, mock, test } from "bun:test";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { App } from "./App";

const status = { index_version: "abc123", git_head: "9b1e0d4f", indexed_at_ms: Date.now(), stale_count: 0, lock_timeout: false, foreign_indexing: false, files: { indexed: 4, skipped: 1 }, drain: { chunks: 0, last: { scanned: 5, indexed: 4, unchanged: 0, skipped: 1, removed: 0, remaining: 0, lock_timeout: false } } };
const r = { score: 0.1, file_rank: 0.1, seeds: ["query:session"], referenced_by: [{ path: "src/http/middleware.ts", count: 2 }], pinned: false, fts_hit: true, query_ident_match: true };
const retrieval = { id: 7, session_key: "mcp:claude-code:1:2", session_label: "Claude Code", tool: "repo_map", query: "session", focus_files: [], budget: 1024, limit_n: null, index_version: "abc123", git_head: "9b1e0d4f", stale_count: 0, created_at_ms: Date.now(), served: 1, cut: 0 };
const tree = [{ path: "src/auth/session.ts", lang: "typescript", skipped_reason: null, symbols: [{ id: 1, name: "createSession", kind: "function", line_start: 3, line_end: 6, signature: "export function createSession(user: User, ttl: number): Session" }] }];
let saved: unknown = null;

beforeEach(() => {
  saved = null;
  (globalThis as any).EventSource = class { addEventListener() {} close() {} };
  window.location.hash = "#token=deadbeef";
  globalThis.fetch = mock(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    const json = (b: unknown) => new Response(JSON.stringify(b), { headers: { "Content-Type": "application/json" } });
    if (init?.headers && (init.headers as Record<string, string>).Authorization !== "Bearer deadbeef") return new Response("{\"error\":\"unauthorized\"}", { status: 401 });
    if (url.endsWith("/api/status")) return json(status);
    if (url.includes("/api/retrievals?")) return json([retrieval]);
    if (url.endsWith("/api/retrievals/7")) return json({ ...retrieval, items: [{ rank: 1, symbol_id: 1, path: "src/auth/session.ts", name: "createSession", line_start: 3, score: 0.1, served: true, reasons: r }] });
    if (url.endsWith("/api/tree")) return json(tree);
    if (url.endsWith("/api/skipped")) return json([{ path: ".env", reason: "denylisted" }]);
    if (url.endsWith("/api/map") && init?.method === "PUT") { saved = JSON.parse(String(init.body)); return json(saved); }
    if (url.endsWith("/api/map")) return json({ pin: [], exclude: [], note: [], boundary: [], deny: { extra_patterns: [] } });
    return new Response("not found", { status: 404 });
  }) as unknown as typeof fetch;
});
afterEach(() => { mock.restore(); });

describe("App", () => {
  test("keyboard-only loop: select retrieval, expand file, focus symbol, exclude, toast, badge, skipped sheet", async () => {
    const user = userEvent.setup();
    render(<App />);
    const rail = await screen.findByRole("region", { name: "Retrievals" });
    expect(within(rail).getByText("Claude Code")).toBeTruthy();
    await user.click(within(rail).getByRole("button", { name: /repo_map.*session/ }));
    const grid = await screen.findByRole("treegrid", { name: "Repository" });
    await waitFor(() => expect(within(grid).getByText("createSession")).toBeTruthy());
    expect(within(grid).getByText("Served")).toBeTruthy();
    await user.click(within(grid).getByText("createSession"));
    const panel = screen.getByRole("region", { name: "Details" });
    await waitFor(() => expect(within(panel).getByText("Ranked 1st, score 0.10.")).toBeTruthy());
    expect(within(panel).getByText("Referenced from middleware.ts (2).")).toBeTruthy();
    await user.click(within(panel).getByRole("button", { name: "Exclude file" }));
    await waitFor(() => expect(saved).not.toBeNull());
    expect((saved as any).exclude).toEqual([{ path: "src/auth/session.ts" }]);
    expect(await screen.findByText("Saved. Applies to the next retrieval.")).toBeTruthy();
    const badge = screen.getByRole("status", { name: "Index freshness" });
    expect(badge.textContent).toContain("fresh");
    expect(badge.textContent).toContain("9b1e0d4");
    await user.click(screen.getByRole("button", { name: /Skipped files/ }));
    expect(await screen.findByText(".env")).toBeTruthy();
    expect(screen.getByText("denylisted")).toBeTruthy();
  });
  test("announces a new retrieval in the live region", async () => {
    render(<App />);
    const live = await screen.findByRole("log", { name: "Announcements" });
    await waitFor(() => expect(live.textContent).toContain("New retrieval from Claude Code: repo_map, 1 served, 0 cut, fresh"));
  });
  test("has no axe violations once loaded", async () => {
    const { container } = render(<App />);
    await screen.findByRole("treegrid", { name: "Repository" });
    const results = await axe.run(container);
    expect(results.violations).toEqual([]);
  });
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run (from `ui/`): `bun test`
Expected: `mapEdits` import fails; `App` tests fail on missing regions.

- [ ] **Step 3: Implement**

`ui/src/lib/mapEdits.ts`:
```ts
import type { MapConfig, Target } from "@/api/types";

const same = (t: Target, path: string, symbol?: string) => t.path === path && (t.symbol ?? undefined) === symbol;

export const isPinned = (c: MapConfig, path: string, symbol?: string) => c.pin.some((t) => same(t, path, symbol));
export const isExcluded = (c: MapConfig, path: string) => c.exclude.some((t) => t.path === path);

export function togglePin(c: MapConfig, path: string, symbol?: string): MapConfig {
  const pin = isPinned(c, path, symbol) ? c.pin.filter((t) => !same(t, path, symbol)) : [...c.pin, symbol ? { path, symbol } : { path }];
  return { ...c, pin };
}
export function toggleExclude(c: MapConfig, path: string): MapConfig {
  const exclude = isExcluded(c, path) ? c.exclude.filter((t) => t.path !== path) : [...c.exclude, { path }];
  return { ...c, exclude };
}
export function setNote(c: MapConfig, path: string, symbol: string | undefined, text: string): MapConfig {
  const note = c.note.filter((n) => !(n.path === path && (n.symbol ?? undefined) === symbol));
  if (text.trim()) note.push(symbol ? { path, symbol, text } : { path, text });
  return { ...c, note };
}
export const noteFor = (c: MapConfig, path: string, symbol?: string) => c.note.find((n) => n.path === path && (n.symbol ?? undefined) === symbol)?.text ?? "";
```
`ui/src/components/LiveRegion.tsx`:
```tsx
export function LiveRegion({ messages }: { messages: string[] }) {
  return (
    <div role="log" aria-label="Announcements" aria-live="polite" className="sr-only">
      {messages.map((m, i) => <p key={i}>{m}</p>)}
    </div>
  );
}
```
`ui/src/components/FreshnessBadge.tsx`:
```tsx
import type { Status } from "@/api/types";

export function freshnessText(s: Status | null): string {
  if (!s) return "loading";
  if (s.foreign_indexing) return "another process indexing";
  if (s.drain.last.remaining > 0 && !s.lock_timeout && s.stale_count > 0) return `indexing, ${s.stale_count} stale`;
  if (s.stale_count > 0) return `${s.stale_count} stale`;
  return "fresh";
}
function age(ms: number | null): string {
  if (!ms) return "";
  const s = Math.max(0, Math.round((Date.now() - ms) / 1000));
  return s < 60 ? `${s}s ago` : s < 3600 ? `${Math.round(s / 60)}m ago` : `${Math.round(s / 3600)}h ago`;
}
export function FreshnessBadge({ status }: { status: Status | null }) {
  return (
    <span role="status" aria-label="Index freshness" className="rounded border px-2 py-1 text-sm">
      <span className="font-medium">{freshnessText(status)}</span>
      {status?.git_head && <span className="ml-2 font-mono text-xs">{status.git_head.slice(0, 7)}</span>}
      <span className="ml-2 text-xs text-muted-foreground">{age(status?.indexed_at_ms ?? null)}</span>
    </span>
  );
}
```
`ui/src/components/RetrievalsRail.tsx`:
```tsx
import type { RetrievalSummary } from "@/api/types";
import { cn } from "@/lib/utils";

function rel(ms: number): string {
  const s = Math.max(0, Math.round((Date.now() - ms) / 1000));
  return s < 60 ? `${s}s ago` : s < 3600 ? `${Math.round(s / 60)}m ago` : `${Math.round(s / 3600)}h ago`;
}
export function RetrievalsRail({ retrievals, selected, onSelect, onMore }: {
  retrievals: RetrievalSummary[]; selected: number | null; onSelect: (id: number) => void; onMore: () => void;
}) {
  const groups = new Map<string, RetrievalSummary[]>();
  for (const r of retrievals) groups.set(r.session_label, [...(groups.get(r.session_label) ?? []), r]);
  return (
    <section aria-label="Retrievals" className="flex h-full flex-col overflow-y-auto border-r">
      <h2 className="px-3 py-2 text-sm font-semibold">Retrievals</h2>
      {retrievals.length === 0 && (
        <div className="px-3 py-2 text-sm text-muted-foreground">
          <p>No retrievals yet. Connect an agent:</p>
          <pre className="mt-2 rounded bg-muted p-2 text-xs">claude mcp add --transport stdio singularrag -- singularrag mcp</pre>
        </div>
      )}
      {[...groups].map(([label, rs]) => (
        <div key={label}>
          <h3 className="px-3 pt-2 text-xs font-medium text-muted-foreground">{label}</h3>
          <ul>
            {rs.map((r) => (
              <li key={r.id}>
                <button type="button" aria-current={selected === r.id ? "true" : undefined} onClick={() => onSelect(r.id)}
                  className={cn("w-full px-3 py-2 text-left text-sm hover:bg-accent focus-visible:bg-accent", selected === r.id && "bg-accent")}>
                  <span className="block">{r.tool} · {r.query ?? "no query"}</span>
                  <span className="block text-xs text-muted-foreground">{rel(r.created_at_ms)} · {r.served} served · {r.cut} cut · {r.stale_count ? `${r.stale_count} stale` : "fresh"}</span>
                </button>
              </li>
            ))}
          </ul>
        </div>
      ))}
      {retrievals.length >= 50 && <button type="button" className="m-3 rounded border px-3 py-1 text-sm" onClick={onMore}>Older</button>}
    </section>
  );
}
```
`ui/src/components/DetailPanel.tsx`:
```tsx
import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import type { MapConfig } from "@/api/types";
import { reasonsToSentences } from "@/lib/reasons";
import { isExcluded, isPinned, noteFor } from "@/lib/mapEdits";
import type { TreeRow } from "./RepoTree";

export function DetailPanel({ row, map, onPin, onExclude, onNote }: {
  row: TreeRow | null; map: MapConfig;
  onPin: (path: string, symbol?: string) => void; onExclude: (path: string) => void; onNote: (path: string, symbol: string | undefined, text: string) => void;
}) {
  const symbol = row?.kind === "symbol" ? row.symbol.symbol.name : undefined;
  const [text, setText] = useState("");
  useEffect(() => { setText(row ? noteFor(map, row.path, symbol) : ""); }, [row, map, symbol]);
  if (!row) return <section aria-label="Details" className="border-l p-3 text-sm text-muted-foreground">Select a file or symbol.</section>;
  const item = row.kind === "symbol" ? row.symbol.item : null;
  return (
    <section aria-label="Details" className="flex flex-col gap-3 border-l p-3">
      <h2 className="font-mono text-sm font-semibold">{symbol ? `${row.path} :: ${symbol}` : row.path}</h2>
      {item ? (
        <ul className="list-disc pl-5 text-sm">{reasonsToSentences(item.reasons, item.rank, item.score).map((s) => <li key={s}>{s}</li>)}</ul>
      ) : (
        <p className="text-sm text-muted-foreground">{row.kind === "symbol" ? "Not part of the selected retrieval." : `${row.file.served} served · ${row.file.cut} cut`}</p>
      )}
      <div className="flex flex-wrap gap-2">
        <Button type="button" variant="outline" aria-pressed={isPinned(map, row.path, symbol)} onClick={() => onPin(row.path, symbol)}>
          {isPinned(map, row.path, symbol) ? "Unpin" : symbol ? "Pin symbol" : "Pin file"}
        </Button>
        <Button type="button" variant="outline" aria-pressed={isExcluded(map, row.path)} onClick={() => onExclude(row.path)}>
          {isExcluded(map, row.path) ? "Include file" : "Exclude file"}
        </Button>
      </div>
      <label className="text-sm">
        Note
        <Textarea value={text} onChange={(e) => setText(e.target.value)} onBlur={() => onNote(row.path, symbol, text)} rows={3} className="mt-1" />
      </label>
      {(isPinned(map, row.path, symbol) || isExcluded(map, row.path)) && (
        <p className="text-xs text-muted-foreground">{isPinned(map, row.path, symbol) ? "Pinned. " : ""}{isExcluded(map, row.path) ? "Excluded." : ""}</p>
      )}
    </section>
  );
}
```
`ui/src/components/SkippedSheet.tsx`:
```tsx
import { Sheet, SheetContent, SheetHeader, SheetTitle, SheetTrigger } from "@/components/ui/sheet";
import { Button } from "@/components/ui/button";
import type { SkippedFile } from "@/api/types";

export function SkippedSheet({ skipped }: { skipped: SkippedFile[] }) {
  const groups = new Map<string, string[]>();
  for (const s of skipped) groups.set(s.reason, [...(groups.get(s.reason) ?? []), s.path]);
  return (
    <Sheet>
      <SheetTrigger render={<Button type="button" variant="outline" />}>Skipped files ({skipped.length})</SheetTrigger>
      <SheetContent>
        <SheetHeader><SheetTitle>Skipped files</SheetTitle></SheetHeader>
        {[...groups].map(([reason, paths]) => (
          <section key={reason} className="mt-3">
            <h3 className="text-sm font-medium">{reason}</h3>
            <ul className="mt-1 font-mono text-xs">{paths.map((p) => <li key={p}>{p}</li>)}</ul>
          </section>
        ))}
      </SheetContent>
    </Sheet>
  );
}
```
If the Base UI sheet's trigger does not accept `render`, use whatever composition the installed `sheet.tsx` documents (`asChild` on Radix-style, `render` on Base UI); the requirement is an accessible button named "Skipped files (N)".

`ui/src/App.tsx`:
```tsx
import { useCallback, useEffect, useMemo, useState } from "react";
import { Toaster, toast } from "sonner";
import { api, setToken, tokenFromFragment } from "@/api/client";
import { subscribe } from "@/api/events";
import type { MapConfig, RetrievalDetail, RetrievalSummary, SkippedFile, Status, TreeFile } from "@/api/types";
import { joinRetrieval } from "@/lib/join";
import { setNote, toggleExclude, togglePin } from "@/lib/mapEdits";
import { DetailPanel } from "@/components/DetailPanel";
import { FreshnessBadge, freshnessText } from "@/components/FreshnessBadge";
import { LiveRegion } from "@/components/LiveRegion";
import { RepoTree, type TreeRow } from "@/components/RepoTree";
import { RetrievalsRail } from "@/components/RetrievalsRail";
import { SkippedSheet } from "@/components/SkippedSheet";

const emptyMap: MapConfig = { pin: [], exclude: [], note: [], boundary: [], deny: { extra_patterns: [] } };

export function App() {
  const [status, setStatus] = useState<Status | null>(null);
  const [retrievals, setRetrievals] = useState<RetrievalSummary[]>([]);
  const [selected, setSelected] = useState<number | null>(null);
  const [detail, setDetail] = useState<RetrievalDetail | null>(null);
  const [tree, setTree] = useState<TreeFile[]>([]);
  const [skipped, setSkipped] = useState<SkippedFile[]>([]);
  const [map, setMap] = useState<MapConfig>(emptyMap);
  const [focused, setFocused] = useState<TreeRow | null>(null);
  const [filter, setFilter] = useState("");
  const [messages, setMessages] = useState<string[]>([]);
  const announce = useCallback((m: string) => setMessages((ms) => [...ms.slice(-9), m]), []);

  useEffect(() => {
    setToken(tokenFromFragment());
    let knownMax = 0;
    const load = async () => {
      const [s, rs, t, sk, m] = await Promise.all([api.status(), api.retrievals(), api.tree(), api.skipped(), api.map()]);
      setStatus(s); setRetrievals(rs); setTree(t); setSkipped(sk); setMap(m);
      for (const r of rs.filter((r) => r.id > knownMax).reverse()) {
        announce(`New retrieval from ${r.session_label}: ${r.tool}, ${r.served} served, ${r.cut} cut, ${r.stale_count ? `${r.stale_count} stale` : "fresh"}`);
      }
      knownMax = Math.max(knownMax, ...rs.map((r) => r.id));
    };
    load().catch((e) => toast.error(String(e.message ?? e)));
    return subscribe(
      () => { load().catch(() => {}); },
      (s) => { setStatus(s); announce(`Index ${freshnessText(s)}`); },
    );
  }, [announce]);

  useEffect(() => {
    if (selected == null) { setDetail(null); return; }
    api.retrieval(selected).then(setDetail).catch((e) => toast.error(String(e.message ?? e)));
  }, [selected]);

  const rows = useMemo(() => joinRetrieval(tree, detail?.items ?? null), [tree, detail]);

  const save = async (next: MapConfig) => {
    try {
      const saved = await api.saveMap(next);
      setMap(saved);
      toast.success("Saved. Applies to the next retrieval.");
    } catch (e: any) {
      toast.error(e.field ? `${e.field}: ${e.message}` : String(e.message ?? e));
    }
  };

  return (
    <div className="grid h-screen grid-cols-[18rem_1fr_22rem] grid-rows-[auto_1fr]">
      <header className="col-span-3 flex items-center gap-3 border-b px-3 py-2">
        <h1 className="text-base font-semibold">singularrag</h1>
        <FreshnessBadge status={status} />
        <SkippedSheet skipped={skipped} />
        <label className="ml-auto text-sm">
          Filter
          <input type="search" value={filter} onChange={(e) => setFilter(e.target.value)} className="ml-2 rounded border bg-background px-2 py-1" placeholder="path or symbol" />
        </label>
      </header>
      <RetrievalsRail retrievals={retrievals} selected={selected} onSelect={setSelected}
        onMore={() => api.retrievals(retrievals[retrievals.length - 1]?.id).then((more) => setRetrievals((rs) => [...rs, ...more]))} />
      <main className="overflow-auto">
        <RepoTree rows={rows} filter={filter} onFocusRow={setFocused} onAction={setFocused} />
      </main>
      <DetailPanel row={focused} map={map}
        onPin={(p, s) => save(togglePin(map, p, s))}
        onExclude={(p) => save(toggleExclude(map, p))}
        onNote={(p, s, t) => { if (t !== (map.note.find((n) => n.path === p && (n.symbol ?? undefined) === s)?.text ?? "")) save(setNote(map, p, s, t)); }} />
      <LiveRegion messages={messages} />
      <Toaster />
    </div>
  );
}
```
The `RepoTree` from Task 11 must re-key its `expanded` state when `rows` changes for a newly selected retrieval; if the App test's expanded file does not appear, add a `useEffect` in `RepoTree` that resets `expanded` to the `expandedByDefault` set whenever the `rows` identity changes.

- [ ] **Step 4: Run the tests to verify they pass**

Run (from `ui/`): `bun test && bun run typecheck && bun run build`
Expected: all pass; build succeeds. If `sonner`'s toast is not found by text in happy-dom, render `<Toaster />` with `expand` and query by role `status`; the requirement is that the success message is announced and visible.

- [ ] **Step 5: Commit**

```bash
git add ui
git commit -m "feat(ui): retrievals rail, detail panel, freshness badge, skipped sheet, live region and the annotation loop"
```

---
### Task 13: Real UI in the binary, CI, README

**Files:**
- Modify: `ui/build.ts` (asset naming under `assets/`), `.gitignore` (drop the placeholder exceptions), `README.md`, `crates/singularrag/tests/serve.rs` (one assertion), `docs/superpowers/specs/2026-09-20-singularrag-serve-design.md` (record any deviation)
- Create: `.github/workflows/ci.yml`
- Delete: `ui/dist/index.html`, `ui/dist/.gitkeep` (the placeholder from Task 8)

**Interfaces:**
- Consumes: everything from Tasks 4–12.
- Produces: `cargo build --release` after `bun run build` embeds the real UI; the integration test asserts the served shell references a hashed `/assets/*.js`; CI runs `bun install --frozen-lockfile && bun run build && bun test` in `ui/` then `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test --workspace`; README gains a "Map UI" section.

- [ ] **Step 1: Point Bun's output at `assets/` and drop the placeholder**

In `ui/build.ts` add to the `Bun.build` options:
```ts
  naming: {
    entry: "[name].[ext]",
    chunk: "assets/[name]-[hash].[ext]",
    asset: "assets/[name]-[hash].[ext]",
  },
```
Run `bun run build` in `ui/` and confirm `dist/index.html` sits at the root and every other file is under `dist/assets/` with a hash in its name. If `entry` naming puts the entry script at the root rather than under `assets/`, set `entry: "assets/[name]-[hash].[ext]"` and confirm `index.html` still lands at `dist/index.html` (HTML entrypoints are always emitted at their own path). Then:
```bash
git rm ui/dist/index.html ui/dist/.gitkeep
```
and remove the `!ui/dist/index.html` and `!ui/dist/.gitkeep` lines from `.gitignore`.

- [ ] **Step 2: Strengthen the integration assertion**

In `crates/singularrag/tests/serve.rs`, `shell_and_assets_are_served_without_a_token`, after the `contains("singularrag")` assertion add:
```rust
    let html = r_text; // rename the earlier `r.text().await.unwrap()` into a variable
    let src = html.split("src=\"").nth(1).and_then(|s| s.split('"').next()).expect("a script tag");
    assert!(src.starts_with("/assets/") || src.starts_with("./assets/") || src.starts_with("assets/"), "{src}");
    let asset_path = src.trim_start_matches('.').trim_start_matches('/');
    let r = client().get(format!("{}/{asset_path}", s.url)).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.headers().get("content-type").unwrap().to_str().unwrap().contains("javascript"));
    assert_eq!(r.headers().get("cache-control").unwrap(), "public, max-age=31536000, immutable");
```

- [ ] **Step 3: Run the whole thing against the real UI**

Run (from `ui/`): `bun run build`; then from the root: `cargo test --workspace` and `cargo build --release`; then `./target/release/singularrag serve --no-open --repo /private/tmp/claude-501/-Users-jonasbroms-Sites-singularrag/e7cfbc54-6490-47fa-af56-276872c58e83/scratchpad/hono` in the background, open the printed URL with `curl`, confirm `/` returns the built shell and `/api/status` with the token returns hono's counts, kill it.
Expected: all green; the release binary serves the real UI.

- [ ] **Step 4: CI**

`.github/workflows/ci.yml`:
```yaml
name: ci
on:
  push:
    branches: [main]
  pull_request:
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: oven-sh/setup-bun@v2
        with:
          bun-version: latest
      - name: Build and test the UI
        working-directory: ui
        run: |
          bun install --frozen-lockfile
          bun run typecheck
          bun test
          bun run build
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@v2
      - run: cargo fmt --all --check
      - run: cargo clippy --all-targets -- -D warnings
      - run: cargo test --workspace --locked
```

- [ ] **Step 5: README**

Add to `README.md` after the "Connect an agent" section:
````markdown
## See what the agent was given

```sh
singularrag serve
```

Opens `http://127.0.0.1:<port>/#token=…` in your browser. The page shows every retrieval an agent made (grouped by host), the repo as a keyboard-navigable treegrid with each symbol marked served, cut or untouched for the selected retrieval, and the reasons in plain sentences. Pin or exclude files and symbols and leave notes; they are written to `.singularrag/map.toml` (commit it) and apply to the agent's next retrieval. The index refreshes as files change; the badge says how fresh it is.

The server binds to localhost only and requires the per-run token in the URL. `--port N` pins a port, `--no-open` skips the browser.

Building from source needs Bun for the UI: `cd ui && bun install && bun run build`, then `cargo build --release`.
````

- [ ] **Step 6: Lint and commit**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test --workspace`
```bash
git add -A
git commit -m "feat: embed the built ui, ci workflow, and serve docs"
```

---

## Self-review notes

- Spec §2 process model: subcommand and dispatch (Task 4), actor move + `Job::Refresh` + `SessionKey` (Task 1), watcher with lock-timeout retry (Task 7), read-only connection (Task 2), `data_version` poller and SSE (Task 6), errors as JSON / 503 on busy / exit 2 on actor death (Tasks 5, 7).
- Spec §3 API: every route (Task 5), `session_label` (Task 5), `PUT /api/map` validation and atomic write (Task 3 + Task 5), SSE names (Task 6). Integration coverage (Task 8).
- Spec §4 UI: stack and Bun build (Task 9), rail/treegrid/top bar/panel (Tasks 11–12), status encoding and reasons as sentences (Task 10), annotation loop with toast (Task 12), badge and skipped sheet (Task 12), empty states (Task 12), theme by `prefers-color-scheme` (Task 9 CSS).
- Spec §5 security: bind, token, Host, no CORS, cache headers (Task 4), assets without token (Task 8), single write path (Task 5), read-only connection (Task 2).
- Spec §6 accessibility: treegrid via react-aria with keyboard test (Task 11), live region (Task 12), status never colour alone (Task 10), no animation (Task 9 CSS has none; `transition-none` on the chevron), axe in component tests (Tasks 11–12), keyboard walkthrough test (Task 12). VoiceOver pass is manual and belongs in the finishing checklist, not a task.
- Spec §7 build: `build.rs` guard and embed (Task 8), CI (Task 13). Spec §8 tests: Rust unit (Tasks 3, 4, 5, 6, 7) and integration (Task 8, 13); UI (Tasks 10–12). Spec §9 crates (Task 4, 9). Spec §10 non-goals: nothing here adds a map, Playwright, a query box, or multi-repo.
- Type consistency: `AppState` fields used identically in Tasks 4–8; `Freshness`/`DrainStatsJson` in 4, 5, 7; DTO names in `queries.rs` match `ui/src/api/types.ts` field-for-field (`session_label`, `limit_n`, `indexed_at_ms`, `drain.last`, `drain.chunks`); `SessionKey::Fixed("serve")` in Tasks 1 and 7; `EngineHandle::refresh` in 1 and 7; `MapConfig::{validate, save_atomic}` in 3 and 5; `ItemStatus` values `served|cut|untouched` in 10–12.
- Known API uncertainties, each with an in-task fallback: shadcn `--base` flag value (Task 9), `bun-plugin-tailwind` layer ordering (Task 9), react-aria `{ArrowRight}` reaching the action button in one press (Task 11), Base UI sheet trigger composition (Task 12), `Event` debug format (Task 6), Bun entry naming under `assets/` (Task 13).
- Placeholder `ui/dist` in Task 8 is a deliberate sequencing device: Rust tasks 4–8 stay green before the UI exists; Task 13 removes it.
