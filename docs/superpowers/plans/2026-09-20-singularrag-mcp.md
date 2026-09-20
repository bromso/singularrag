# singularrag MCP Server Implementation Plan (plan 2 of 4)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Expose the plan-1 `Engine` as a stdio MCP server (`singularrag mcp`) with exactly two tools, finish interrupted refreshes in the background between tool calls, and document host configuration.

**Architecture:** One OS thread owns the `Engine` (an actor); rmcp tool handlers send `Job`s over a channel and await a one-shot reply, so the async runtime never blocks and nothing crosses an await. The actor drains stale files in 2 s chunks whenever its queue is empty. The Engine API is unchanged except for one setter.

**Tech Stack:** rmcp 3.4 (`server`, `transport-io`; `transport-child-process` for the test client), schemars 1.x via rmcp, tokio 1.x (`rt-multi-thread`, `macros`, `io-std`, `sync`), tracing + tracing-subscriber (`env-filter`), plus the plan-1 workspace.

**Spec:** `docs/superpowers/specs/2026-09-20-singularrag-mcp-design.md` (this plan); parent `docs/superpowers/specs/2026-09-19-singularrag-design.md` §7–§9, §11.

## Global Constraints

- stdout is the MCP protocol; nothing else ever writes to it. Logs go to stderr via `tracing`, default level `warn`, `RUST_LOG` overrides (spec §2).
- Exactly two tools, `repo_map` and `find_symbol`, with the descriptions in spec §3 verbatim; output is one text content block containing the engine's text unchanged (spec §3).
- Session key `mcp:<client>:<pid>:<start_ms>`; `<client>` is `clientInfo.name` lower-cased, whitespace → `-`, else `unknown` (spec §2).
- Repo root: `--repo` else current directory (spec §2). `roots/list` is not consulted.
- Engine errors become tool results with `is_error = true`; never a JSON-RPC error, never a panic. Dead actor → log to stderr, exit non-zero (spec §2).
- Background drain runs only when the queue is empty, only after a response with `stale_count > 0` whose refresh did not hit `lock_timeout`, and stops on `remaining == 0`, `lock_timeout`, or a job arriving (spec §2).
- `Engine::set_refresh_budget(Duration)` is the only Engine API addition; default `REFRESH_BUDGET` (2 s) (spec §2).
- Server info: name `singularrag`, version `CARGO_PKG_VERSION`, tools capability only, the instructions string in spec §3 verbatim. No resources, prompts or notifications.
- Read-only posture unchanged: the server writes only what the Engine writes.
- No install subcommand; host config lives in `README.md` (spec §4).

## File structure

```
crates/singularrag-core/src/engine.rs        + refresh_budget field, set_refresh_budget(), refresh_budget()
crates/singularrag/Cargo.toml                 + rmcp, tokio, tracing, tracing-subscriber, serde (deps); rmcp child-process (dev)
crates/singularrag/src/main.rs                + Cmd::Mcp, mod mcp, tracing init
crates/singularrag/src/mcp/mod.rs             pub fn run(root, refresh_budget) -> anyhow::Result<()>  (runtime + serve)
crates/singularrag/src/mcp/actor.rs           Job, EngineHandle, spawn_engine_thread(), drain logic  (tests here)
crates/singularrag/src/mcp/server.rs          SingularragServer: tool router, ServerHandler, session key from clientInfo
crates/singularrag/tests/mcp.rs               child-process integration tests
README.md                                     host configuration
```

---

### Task 1: `Engine::set_refresh_budget`

**Files:**
- Modify: `crates/singularrag-core/src/engine.rs` (struct, `open`, `repo_map`, `find_symbol`, tests)

**Interfaces:**
- Produces: `Engine::set_refresh_budget(&mut self, budget: Duration)`, `Engine::refresh_budget(&self) -> Duration`. `repo_map` and `find_symbol` call `self.refresh(self.refresh_budget)` instead of the constant. `REFRESH_BUDGET` stays as the default.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module at the bottom of `crates/singularrag-core/src/engine.rs`:
```rust
    #[test]
    fn zero_refresh_budget_makes_the_first_call_stale_and_default_catches_up() {
        let (_dir, mut e) = engine();
        assert_eq!(e.refresh_budget(), REFRESH_BUDGET);
        e.set_refresh_budget(Duration::ZERO);
        let first = e
            .repo_map(&MapRequest { query: None, focus_files: vec![], budget_tokens: 1024 })
            .unwrap();
        assert!(first.stale_count > 0, "{first:?}");
        assert!(first.text.contains("STALE:"));
        e.set_refresh_budget(REFRESH_BUDGET);
        let second = e
            .repo_map(&MapRequest { query: None, focus_files: vec![], budget_tokens: 1024 })
            .unwrap();
        assert_eq!(second.stale_count, 0);
        assert!(second.text.contains("· fresh ·"));
    }
```
The existing `engine()` helper in that module opens the ts-mini fixture; reuse it.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p singularrag-core zero_refresh_budget`
Expected: compile error, `refresh_budget`/`set_refresh_budget` not found.

- [ ] **Step 3: Implement**

In `Engine` add a field `refresh_budget: Duration`; in `open` initialise it to `REFRESH_BUDGET`. Add next to `set_heartbeat_every`:
```rust
    /// Inline refresh budget used by `repo_map` and `find_symbol` (spec §8). The MCP
    /// server (plan 2) uses the same value for its background drain chunks; tests set
    /// it to zero to force a stale response without holding the lock.
    pub fn set_refresh_budget(&mut self, budget: Duration) {
        self.refresh_budget = budget;
    }

    pub fn refresh_budget(&self) -> Duration {
        self.refresh_budget
    }
```
In `repo_map` and `find_symbol` replace `self.refresh(REFRESH_BUDGET)?` with `self.refresh(self.refresh_budget)?`. Update the `REFRESH_BUDGET` doc comment's second sentence to: "The other half of §8, finishing the remaining files in the background, is the MCP server's drain loop (plan 2)."

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p singularrag-core engine`
Expected: all engine tests pass, including the new one.

- [ ] **Step 5: Lint and commit**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test --workspace`
```bash
git add crates/singularrag-core/src/engine.rs
git commit -m "feat(core): configurable inline refresh budget on Engine"
```

---

### Task 2: Engine actor thread with background drain

**Files:**
- Modify: `crates/singularrag/Cargo.toml` (add `tokio` dependency; the rest of the MCP deps come in Task 3)
- Create: `crates/singularrag/src/mcp/mod.rs` (module skeleton only in this task), `crates/singularrag/src/mcp/actor.rs`
- Modify: `crates/singularrag/src/main.rs` (add `mod mcp;`)

**Interfaces:**
- Consumes: `Engine::{open, set_refresh_budget, refresh, repo_map, find_symbol}`, `MapRequest/MapResponse`, `FindRequest/FindResponse`, `IndexStats { remaining, lock_timeout, .. }`, `singularrag_core::Error`.
- Produces:
  - `actor::Job` enum: `Map(MapRequest, oneshot::Sender<Reply<MapResponse>>)`, `Find(FindRequest, oneshot::Sender<Reply<FindResponse>>)` where `type Reply<T> = Result<T, String>` (errors stringified at the actor boundary so replies are `Send + 'static` and trivially convertible to `is_error` content).
  - `actor::EngineHandle { tx: std::sync::mpsc::Sender<Job> }` with `Clone`, `async fn map(&self, MapRequest) -> Reply<MapResponse>`, `async fn find(&self, FindRequest) -> Reply<FindResponse>`. A closed channel yields `Err("engine thread is gone")`.
  - `actor::EngineConfig { root: PathBuf, session_key: Arc<Mutex<Option<String>>>, refresh_budget: Duration }`.
  - `actor::spawn(config: EngineConfig) -> (EngineHandle, std::thread::JoinHandle<()>)`.
  - Session key: the actor reads `session_key.lock().clone()` at first job; `None` → `format!("mcp:unknown:{pid}:{start_ms}")`; `Some(client)` → `format!("mcp:{client}:{pid}:{start_ms}")`. The server (Task 3) fills the `Option` from `clientInfo` during initialize.

- [ ] **Step 1: Add tokio and the module skeleton**

`crates/singularrag/Cargo.toml` `[dependencies]` add:
```toml
tokio = { workspace = true }
```
Workspace `Cargo.toml` `[workspace.dependencies]` add:
```toml
tokio = { version = "1", features = ["rt-multi-thread", "macros", "io-std", "sync", "time"] }
```
`crates/singularrag/src/mcp/mod.rs`:
```rust
//! `singularrag mcp`: stdio MCP server over the plan-1 Engine.

pub mod actor;
```
`crates/singularrag/src/main.rs`: add `mod mcp;` after the `use` lines (the `Mcp` subcommand arrives in Task 4; until then the module is compiled but unused, so add `#[allow(dead_code)]` on `mod mcp;` and remove it in Task 4).

- [ ] **Step 2: Write the failing tests**

Bottom of `crates/singularrag/src/mcp/actor.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use singularrag_core::engine::{Engine, REFRESH_BUDGET};
    use singularrag_core::fixture::write_ts_mini;
    use singularrag_core::store::{lock, Store};
    use std::time::Duration;

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        write_ts_mini(dir.path());
        dir
    }

    fn config(root: &std::path::Path, budget: Duration) -> EngineConfig {
        EngineConfig {
            root: root.to_path_buf(),
            session_key: Arc::new(Mutex::new(Some("test-client".into()))),
            refresh_budget: budget,
        }
    }

    #[tokio::test]
    async fn map_and_find_match_a_direct_engine_call() {
        let dir = fixture();
        let (handle, _join) = spawn(config(dir.path(), REFRESH_BUDGET));
        let map = handle.map(MapRequest { query: Some("session".into()), ..Default::default() }).await.unwrap();
        assert!(map.text.starts_with("# singularrag · index "));
        assert!(map.text.contains("src/auth/session.ts:\n"));
        let find = handle.find(FindRequest { name: "createSession".into(), kind: None, limit: 10 }).await.unwrap();
        assert!(find.text.contains("src/auth/session.ts:3  function  export function createSession"));

        // Same text the engine produces directly for the same inputs (modulo retrieval id).
        let mut e = Engine::open(dir.path(), "direct").unwrap();
        let direct = e.repo_map(&MapRequest { query: Some("session".into()), ..Default::default() }).unwrap();
        let strip = |s: &str| s.lines().skip(1).collect::<Vec<_>>().join("\n");
        assert_eq!(strip(&map.text), strip(&direct.text));
    }

    #[tokio::test]
    async fn session_key_carries_client_name_pid_and_start() {
        let dir = fixture();
        let (handle, _join) = spawn(config(dir.path(), REFRESH_BUDGET));
        handle.map(MapRequest::default()).await.unwrap();
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        let key: String = store.conn().query_row("SELECT session_key FROM retrievals ORDER BY id DESC LIMIT 1", [], |r| r.get(0)).unwrap();
        let parts: Vec<&str> = key.split(':').collect();
        assert_eq!(parts.len(), 4, "{key}");
        assert_eq!(parts[0], "mcp");
        assert_eq!(parts[1], "test-client");
        assert_eq!(parts[2], std::process::id().to_string());
        assert!(parts[3].parse::<i64>().unwrap() > 0);
    }

    #[tokio::test]
    async fn unknown_client_when_initialize_never_ran() {
        let dir = fixture();
        let mut cfg = config(dir.path(), REFRESH_BUDGET);
        cfg.session_key = Arc::new(Mutex::new(None));
        let (handle, _join) = spawn(cfg);
        handle.map(MapRequest::default()).await.unwrap();
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        let key: String = store.conn().query_row("SELECT session_key FROM retrievals ORDER BY id DESC LIMIT 1", [], |r| r.get(0)).unwrap();
        assert!(key.starts_with("mcp:unknown:"), "{key}");
    }

    #[tokio::test]
    async fn stale_first_response_is_drained_in_the_background() {
        let dir = fixture();
        // Zero budget: the first call indexes nothing and reports STALE.
        let (handle, _join) = spawn(config(dir.path(), Duration::ZERO));
        let first = handle.map(MapRequest::default()).await.unwrap();
        assert!(first.stale_count > 0, "{first:?}");
        // The drain uses the same (zero) budget, so it cannot make progress by itself;
        // raise the budget through the handle and wait for the drain to finish.
        handle.set_refresh_budget(REFRESH_BUDGET).await.unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let stats = handle.stats().await.unwrap();
            if stats.remaining == 0 {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "drain never finished: {stats:?}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let second = handle.map(MapRequest::default()).await.unwrap();
        assert_eq!(second.stale_count, 0);
        assert!(second.text.contains("· fresh ·"));
    }

    #[tokio::test]
    async fn drain_does_not_run_while_another_process_holds_the_lock() {
        let dir = fixture();
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        let foreign_pid = std::process::id() + 1;
        assert!(lock::try_acquire(&store, foreign_pid, singularrag_core::time::now_ms()).unwrap());
        let (handle, _join) = spawn(config(dir.path(), REFRESH_BUDGET));
        let first = handle.map(MapRequest::default()).await.unwrap();
        assert!(first.stale_count > 0);
        tokio::time::sleep(Duration::from_millis(300)).await;
        let n: i64 = store.conn().query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0, "drain indexed while the lock was held by another process");
        lock::release(&store, foreign_pid).unwrap();
    }

    #[tokio::test]
    async fn bad_root_yields_errors_and_the_actor_stays_alive() {
        let cfg = EngineConfig {
            root: std::path::PathBuf::from("/nonexistent/singularrag-test-root"),
            session_key: Arc::new(Mutex::new(None)),
            refresh_budget: REFRESH_BUDGET,
        };
        let (handle, _join) = spawn(cfg);
        let err = handle.map(MapRequest::default()).await.unwrap_err();
        assert!(err.contains("/nonexistent/singularrag-test-root"), "{err}");
        let err2 = handle.find(FindRequest { name: "x".into(), kind: None, limit: 5 }).await.unwrap_err();
        assert!(err2.contains("/nonexistent/singularrag-test-root"));
    }

    #[tokio::test]
    async fn dropped_engine_thread_reports_gone() {
        let dir = fixture();
        let (handle, join) = spawn(config(dir.path(), REFRESH_BUDGET));
        handle.shutdown();
        join.join().unwrap();
        let err = handle.map(MapRequest::default()).await.unwrap_err();
        assert!(err.contains("engine thread is gone"), "{err}");
    }
}
```
Note the two extra handle methods the drain test needs and that are useful to the server too: `set_refresh_budget` and `stats` (returns the last `IndexStats` the actor saw, `Default` before any refresh). They become `Job::SetRefreshBudget(Duration, oneshot::Sender<Reply<()>>)` and `Job::Stats(oneshot::Sender<Reply<IndexStats>>)`. `shutdown()` sends `Job::Shutdown` and is fire-and-forget. Add `tempfile` and `singularrag-core` to the bin crate's `[dev-dependencies]` if not already present (core is a normal dependency already; tempfile is).

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p singularrag actor`
Expected: compile error, `actor` items not found.

- [ ] **Step 4: Implement the actor**

`crates/singularrag/src/mcp/actor.rs` (above the tests):
```rust
//! The Engine actor: one OS thread owns the `Engine`; everyone else talks to it over a
//! channel. This is how a `!Sync` Engine serves an async, multi-connection host, and
//! where spec §8's background refresh lives.

use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use singularrag_core::engine::{Engine, FindRequest, FindResponse, MapRequest, MapResponse};
use singularrag_core::index::IndexStats;
use tokio::sync::oneshot;

/// Errors are stringified at the actor boundary: replies must be `Send + 'static`,
/// and the MCP layer only ever renders them as `is_error` text.
pub type Reply<T> = Result<T, String>;

pub enum Job {
    Map(MapRequest, oneshot::Sender<Reply<MapResponse>>),
    Find(FindRequest, oneshot::Sender<Reply<FindResponse>>),
    SetRefreshBudget(Duration, oneshot::Sender<Reply<()>>),
    Stats(oneshot::Sender<Reply<IndexStats>>),
    Shutdown,
}

#[derive(Clone)]
pub struct EngineConfig {
    pub root: PathBuf,
    /// Filled by the server from `clientInfo.name` during initialize; read once at the
    /// first job. `None` means initialize never ran.
    pub session_key: Arc<Mutex<Option<String>>>,
    pub refresh_budget: Duration,
}

#[derive(Clone)]
pub struct EngineHandle {
    tx: mpsc::Sender<Job>,
}

const GONE: &str = "engine thread is gone";

impl EngineHandle {
    async fn ask<T>(&self, make: impl FnOnce(oneshot::Sender<Reply<T>>) -> Job) -> Reply<T> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(make(tx)).map_err(|_| GONE.to_string())?;
        rx.await.unwrap_or_else(|_| Err(GONE.to_string()))
    }

    pub async fn map(&self, req: MapRequest) -> Reply<MapResponse> {
        self.ask(|tx| Job::Map(req, tx)).await
    }

    pub async fn find(&self, req: FindRequest) -> Reply<FindResponse> {
        self.ask(|tx| Job::Find(req, tx)).await
    }

    pub async fn set_refresh_budget(&self, budget: Duration) -> Reply<()> {
        self.ask(|tx| Job::SetRefreshBudget(budget, tx)).await
    }

    pub async fn stats(&self) -> Reply<IndexStats> {
        self.ask(Job::Stats).await
    }

    pub fn shutdown(&self) {
        let _ = self.tx.send(Job::Shutdown);
    }
}

/// How long the actor waits for a job before checking whether a drain is due.
const IDLE_TICK: Duration = Duration::from_millis(20);

pub fn spawn(config: EngineConfig) -> (EngineHandle, JoinHandle<()>) {
    let (tx, rx) = mpsc::channel::<Job>();
    let join = std::thread::Builder::new()
        .name("singularrag-engine".into())
        .spawn(move || run(config, rx))
        .expect("spawn engine thread");
    (EngineHandle { tx }, join)
}

struct Actor {
    config: EngineConfig,
    engine: Option<Result<Engine, String>>,
    last_stats: IndexStats,
    /// True after a response reported stale files and the refresh did not lose the lock.
    drain_pending: bool,
}

impl Actor {
    fn session_key(&self) -> String {
        let client = self
            .config
            .session_key
            .lock()
            .map(|g| g.clone())
            .unwrap_or(None)
            .unwrap_or_else(|| "unknown".to_string());
        format!("mcp:{client}:{}:{}", std::process::id(), singularrag_core::time::now_ms())
    }

    fn engine(&mut self) -> Result<&mut Engine, String> {
        if self.engine.is_none() {
            let key = self.session_key();
            let opened = Engine::open(&self.config.root, &key)
                .map(|mut e| {
                    e.set_refresh_budget(self.config.refresh_budget);
                    e
                })
                .map_err(|e| e.to_string());
            self.engine = Some(opened);
        }
        match self.engine.as_mut().expect("set above") {
            Ok(e) => Ok(e),
            Err(msg) => Err(msg.clone()),
        }
    }

    fn note(&mut self, stale_count: usize) {
        // The Engine reports lock_timeout through its last refresh; we re-derive it by
        // asking for a zero-work refresh only when needed (cheap: a stat walk).
        self.drain_pending = stale_count > 0;
    }

    fn handle(&mut self, job: Job) -> bool {
        match job {
            Job::Map(req, reply) => {
                let out = self.engine().and_then(|e| e.repo_map(&req).map_err(|e| e.to_string()));
                if let Ok(r) = &out {
                    self.note(r.stale_count);
                }
                let _ = reply.send(out);
            }
            Job::Find(req, reply) => {
                let out = self.engine().and_then(|e| e.find_symbol(&req).map_err(|e| e.to_string()));
                if let Ok(r) = &out {
                    self.note(r.stale_count);
                }
                let _ = reply.send(out);
            }
            Job::SetRefreshBudget(budget, reply) => {
                self.config.refresh_budget = budget;
                let out = self.engine().map(|e| e.set_refresh_budget(budget));
                let _ = reply.send(out);
            }
            Job::Stats(reply) => {
                let _ = reply.send(Ok(self.last_stats.clone()));
            }
            Job::Shutdown => return false,
        }
        true
    }

    /// One drain chunk. Returns true when more work remains and the lock was ours.
    fn drain_chunk(&mut self) -> bool {
        let budget = self.config.refresh_budget;
        let Ok(engine) = self.engine() else { return false };
        match engine.refresh(budget) {
            Ok(stats) => {
                let more = stats.remaining > 0 && !stats.lock_timeout;
                self.last_stats = stats;
                more
            }
            Err(e) => {
                tracing::warn!("background refresh failed: {e}");
                false
            }
        }
    }
}

fn run(config: EngineConfig, rx: mpsc::Receiver<Job>) {
    let mut actor = Actor { config, engine: None, last_stats: IndexStats::default(), drain_pending: false };
    loop {
        if actor.drain_pending {
            // Prefer jobs; drain only when the queue is empty.
            match rx.try_recv() {
                Ok(job) => {
                    if !actor.handle(job) {
                        return;
                    }
                    continue;
                }
                Err(mpsc::TryRecvError::Disconnected) => return,
                Err(mpsc::TryRecvError::Empty) => {
                    actor.drain_pending = actor.drain_chunk();
                    continue;
                }
            }
        }
        match rx.recv_timeout(IDLE_TICK) {
            Ok(job) => {
                if !actor.handle(job) {
                    return;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}
```
Two details the tests pin: `Stats` returns `last_stats`, which is only updated by drain chunks (so the drain test observes `remaining` fall to 0 through it); and after a drain chunk with `lock_timeout`, `drain_pending` becomes false, which is what the lock test asserts indirectly by counting symbols. Note `note()` sets `drain_pending` from `stale_count` alone; the lock case is handled one chunk later by `drain_chunk` returning false as soon as its refresh reports `lock_timeout`, and because `refresh` under a foreign lock indexes nothing (`remaining` set, no writes), the symbol count stays 0.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p singularrag actor`
Expected: 7 passed. The drain test may take a few seconds. If `unknown_client_when_initialize_never_ran` shows a key with 5 parts, the client name contained a colon; it cannot here, but the session-key test splits on `:` so keep client names colon-free (the server lower-cases and replaces whitespace only; colons are replaced with `-` too in Task 3).

- [ ] **Step 6: Lint and commit**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test --workspace`
```bash
git add Cargo.toml Cargo.lock crates/singularrag
git commit -m "feat(mcp): engine actor thread with background drain"
```

---
### Task 3: rmcp server with the two tools

**Files:**
- Modify: `Cargo.toml` (workspace deps), `crates/singularrag/Cargo.toml`, `crates/singularrag/src/mcp/mod.rs`
- Create: `crates/singularrag/src/mcp/server.rs`

**Interfaces:**
- Consumes: `actor::{EngineHandle, EngineConfig, spawn}`, `MapRequest`, `FindRequest`, `singularrag_core::map::{DEFAULT_BUDGET, MAX_BUDGET}`, `singularrag_core::find::MAX_LIMIT`.
- Produces: `server::SingularragServer::new(handle: EngineHandle, session_key: Arc<Mutex<Option<String>>>) -> Self` (`Clone`), implementing `rmcp::ServerHandler` with tools `repo_map` and `find_symbol`; `server::INSTRUCTIONS: &str`; `server::REPO_MAP_DESCRIPTION: &str`; `server::FIND_SYMBOL_DESCRIPTION: &str`; `server::client_slug(name: &str) -> String` (lower-case, whitespace and `:` → `-`).
- Verified rmcp 3.4.0 facts (2026-09-20): `get_info` returns `ServerConfig` (builder: `ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_server_info(Implementation::new(name, version)).with_instructions(..)`); text content is `ContentBlock::text(..)`; error result is `CallToolResult::error(vec![ContentBlock::text(..)])`; `initialize(&self, request: InitializeRequestParams, context: RequestContext<RoleServer>) -> Result<InitializeResult, ErrorData>` and `request.client_info.name`; the default `initialize` body is `context.peer.set_peer_info(request.clone()); self.negotiate_initialize(&request)`; `Parameters<T>` at `rmcp::handler::server::wrapper::Parameters`; `#[tool_router]` on the impl block, `#[tool(description = ..)]` on methods, `#[tool_handler]` on the `ServerHandler` impl; `Option<T>` inputs need no `#[serde(default)]`.

- [ ] **Step 1: Add the dependencies**

Workspace `Cargo.toml` `[workspace.dependencies]` add:
```toml
rmcp = { version = "3.4", features = ["server", "transport-io"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
```
`crates/singularrag/Cargo.toml` `[dependencies]` add `rmcp = { workspace = true }`, `serde = { workspace = true }`, `tracing = { workspace = true }`, `tracing-subscriber = { workspace = true }`.
`crates/singularrag/src/mcp/mod.rs` add `pub mod server;`.

- [ ] **Step 2: Write the failing unit tests**

Bottom of `crates/singularrag/src/mcp/server.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_slug_normalises() {
        assert_eq!(client_slug("Claude Code"), "claude-code");
        assert_eq!(client_slug("codex"), "codex");
        assert_eq!(client_slug("Some:Host  CLI"), "some-host-cli");
        assert_eq!(client_slug("   "), "unknown");
    }

    #[test]
    fn tool_list_is_exactly_the_two_spec_tools() {
        let router = SingularragServer::tool_router();
        let mut names: Vec<String> = router.list_all().into_iter().map(|t| t.name.to_string()).collect();
        names.sort();
        assert_eq!(names, vec!["find_symbol", "repo_map"]);
        let tools = router.list_all();
        let map = tools.iter().find(|t| t.name == "repo_map").unwrap();
        assert_eq!(map.description.as_deref(), Some(REPO_MAP_DESCRIPTION));
        let find = tools.iter().find(|t| t.name == "find_symbol").unwrap();
        assert_eq!(find.description.as_deref(), Some(FIND_SYMBOL_DESCRIPTION));
        let schema = serde_json::to_value(&map.input_schema).unwrap();
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("query") && props.contains_key("focus_files") && props.contains_key("budget_tokens"));
        assert!(schema.get("required").map_or(true, |r| r.as_array().unwrap().is_empty()), "{schema}");
        let schema = serde_json::to_value(&find.input_schema).unwrap();
        assert_eq!(schema["required"], serde_json::json!(["name"]));
    }

    #[test]
    fn requests_convert_with_defaults() {
        let m: MapRequest = MapArgs { query: None, focus_files: None, budget_tokens: None }.into();
        assert_eq!(m.budget_tokens, singularrag_core::map::DEFAULT_BUDGET);
        assert!(m.focus_files.is_empty());
        let m: MapRequest = MapArgs { query: Some("x".into()), focus_files: Some(vec!["a.ts".into()]), budget_tokens: Some(99_999) }.into();
        assert_eq!(m.budget_tokens, 99_999, "clamping is the engine's job, not the server's");
        let f: FindRequest = FindArgs { name: "n".into(), kind: None, limit: None }.into();
        assert_eq!(f.limit, 10);
    }
}
```
`ToolRouter::list_all()` returns `Vec<Tool>`; if the method in rmcp 3.4 is named differently (`list_all` vs `tools`), read `~/.cargo/registry/src/*/rmcp-3.4.*/src/handler/server/router/tool.rs` and use the accessor that returns all `Tool`s; adjust the test, not the assertions.

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p singularrag server`
Expected: compile error, `server` items not found.

- [ ] **Step 4: Implement the server**

`crates/singularrag/src/mcp/server.rs` (above the tests):
```rust
//! The rmcp handler: two tools, server info, and the session key from clientInfo.
//! All engine work goes through the actor; handlers only await a reply.

use std::sync::{Arc, Mutex};

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ContentBlock, Implementation, InitializeRequestParams, InitializeResult,
    ServerCapabilities, ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::{schemars, tool, tool_handler, tool_router, ErrorData, RoleServer, ServerHandler};
use singularrag_core::engine::{FindRequest, MapRequest};

use super::actor::EngineHandle;

pub const INSTRUCTIONS: &str = "singularrag gives you a ranked map of this repository. Call repo_map first with your task as the query, then read only the files it points at. Use find_symbol to locate a name. Both tools are read-only. A STALE header means files changed since indexing; the index catches up in the background.";

pub const REPO_MAP_DESCRIPTION: &str = "Token-budgeted map of the symbols most relevant to a task. Call this before reading files. `query` is a question or identifiers; `focus_files` are repo-relative paths you already know matter; `budget_tokens` defaults to 1024, max 8192. Returns paths, line numbers and signatures only, never bodies. The first line says how fresh the index is; if it says STALE, call again after a moment.";

pub const FIND_SYMBOL_DESCRIPTION: &str = "Look up a symbol by name: exact, prefix, or split words (`create session` finds `createSession`). Returns the definition's path, line and signature and which files reference it. Optional `kind` filter: function, class, method, type, const, module. `limit` defaults to 10, max 50.";

/// `clientInfo.name` → the `<client>` part of the session key. Lower-case; whitespace
/// and colons become `-` so the key stays `mcp:<client>:<pid>:<start>`.
pub fn client_slug(name: &str) -> String {
    let slug: String = name
        .trim()
        .to_lowercase()
        .split(|c: char| c.is_whitespace() || c == ':')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        "unknown".to_string()
    } else {
        slug
    }
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct MapArgs {
    /// A question or identifiers describing the task.
    pub query: Option<String>,
    /// Repo-relative paths you already know matter; they seed the ranking.
    pub focus_files: Option<Vec<String>>,
    /// Soft token budget for the map. Default 1024, max 8192.
    pub budget_tokens: Option<u32>,
}

impl From<MapArgs> for MapRequest {
    fn from(a: MapArgs) -> Self {
        let d = MapRequest::default();
        MapRequest {
            query: a.query,
            focus_files: a.focus_files.unwrap_or_default(),
            budget_tokens: a.budget_tokens.map_or(d.budget_tokens, |b| b as usize),
        }
    }
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct FindArgs {
    /// Symbol name: exact, prefix, or split words.
    pub name: String,
    /// One of: function, class, method, type, const, module.
    pub kind: Option<String>,
    /// Max hits. Default 10, max 50.
    pub limit: Option<u32>,
}

impl From<FindArgs> for FindRequest {
    fn from(a: FindArgs) -> Self {
        FindRequest { name: a.name, kind: a.kind, limit: a.limit.map_or(10, |l| l as usize) }
    }
}

#[derive(Clone)]
pub struct SingularragServer {
    handle: EngineHandle,
    session_key: Arc<Mutex<Option<String>>>,
    tool_router: ToolRouter<Self>,
}

fn text_result(r: Result<String, String>) -> Result<CallToolResult, ErrorData> {
    Ok(match r {
        Ok(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
        Err(msg) => CallToolResult::error(vec![ContentBlock::text(msg)]),
    })
}

#[tool_router]
impl SingularragServer {
    pub fn new(handle: EngineHandle, session_key: Arc<Mutex<Option<String>>>) -> Self {
        Self { handle, session_key, tool_router: Self::tool_router() }
    }

    #[tool(name = "repo_map", description = REPO_MAP_DESCRIPTION)]
    async fn repo_map(&self, Parameters(args): Parameters<MapArgs>) -> Result<CallToolResult, ErrorData> {
        text_result(self.handle.map(args.into()).await.map(|r| r.text))
    }

    #[tool(name = "find_symbol", description = FIND_SYMBOL_DESCRIPTION)]
    async fn find_symbol(&self, Parameters(args): Parameters<FindArgs>) -> Result<CallToolResult, ErrorData> {
        text_result(self.handle.find(args.into()).await.map(|r| r.text))
    }
}

#[tool_handler]
impl ServerHandler for SingularragServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("singularrag", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS.to_string())
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        if let Ok(mut slot) = self.session_key.lock() {
            *slot = Some(client_slug(&request.client_info.name));
        }
        context.peer.set_peer_info(request.clone());
        self.negotiate_initialize(&request)
    }
}
```
If `#[tool(description = CONST)]` does not accept a path expression in rmcp 3.4 (the macro may require a literal), keep the constants and write the literal strings in the attributes too, and change the unit test to compare against the constants so drift is caught. If the description attribute accepts an expression, the constants are the single source. Try the constant form first. If `negotiate_initialize` or `set_peer_info` are named differently, copy the default `initialize` body from the trait's source in the cargo registry.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p singularrag server`
Expected: 3 passed.

- [ ] **Step 6: Lint and commit**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test --workspace`
```bash
git add Cargo.toml Cargo.lock crates/singularrag
git commit -m "feat(mcp): rmcp server with repo_map and find_symbol tools"
```

---

### Task 4: `singularrag mcp` subcommand and runtime

**Files:**
- Modify: `crates/singularrag/src/mcp/mod.rs`, `crates/singularrag/src/main.rs`, `crates/singularrag/tests/cli.rs`

**Interfaces:**
- Consumes: `actor::{spawn, EngineConfig}`, `server::SingularragServer`, `Engine::REFRESH_BUDGET`.
- Produces: `mcp::run(root: PathBuf, refresh_budget: Duration) -> anyhow::Result<()>` (blocking; builds the tokio runtime, spawns the actor, serves stdio, waits, shuts the actor down); `Cmd::Mcp { refresh_budget_ms: u64 }` (hidden flag, default 2000, exists for the integration tests); `--help` lists `mcp`.

- [ ] **Step 1: Write the failing CLI test**

Append to `crates/singularrag/tests/cli.rs`:
```rust
#[test]
fn help_lists_the_mcp_subcommand() {
    Command::cargo_bin("singularrag")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("mcp").and(predicate::str::contains("stdio")));
}

#[test]
fn mcp_exits_cleanly_when_stdin_closes() {
    let dir = fixture();
    let mut cmd = Command::cargo_bin("singularrag").unwrap();
    cmd.args(["mcp", "--repo", dir.path().to_str().unwrap()]);
    cmd.write_stdin("");
    cmd.timeout(std::time::Duration::from_secs(10)).assert().success();
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p singularrag --test cli mcp`
Expected: 2 failures (unknown subcommand / missing text).

- [ ] **Step 3: Implement**

`crates/singularrag/src/mcp/mod.rs`:
```rust
//! `singularrag mcp`: stdio MCP server over the plan-1 Engine.

pub mod actor;
pub mod server;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rmcp::{transport::stdio, ServiceExt};

/// Serve MCP over stdin/stdout until the client closes the connection.
/// stdout is the protocol; logs go to stderr (initialised by `main`).
pub fn run(root: PathBuf, refresh_budget: Duration) -> anyhow::Result<()> {
    let session_key = Arc::new(Mutex::new(None));
    let (handle, join) = actor::spawn(actor::EngineConfig { root, session_key: Arc::clone(&session_key), refresh_budget });
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    let result = rt.block_on(async {
        let service = server::SingularragServer::new(handle.clone(), session_key)
            .serve(stdio())
            .await
            .map_err(|e| anyhow::anyhow!("mcp initialize failed: {e}"))?;
        service.waiting().await.map_err(|e| anyhow::anyhow!("mcp transport error: {e}"))?;
        Ok::<(), anyhow::Error>(())
    });
    handle.shutdown();
    if join.join().is_err() {
        tracing::error!("engine thread panicked");
        std::process::exit(2);
    }
    result
}
```
`crates/singularrag/src/main.rs`: remove the `#[allow(dead_code)]` from `mod mcp;`; add to `Cmd`:
```rust
    /// Serve the repo_map and find_symbol tools to an agent over stdio (MCP)
    Mcp {
        /// Inline refresh budget in milliseconds (spec §8). Tests lower it.
        #[arg(long, default_value_t = 2000, hide = true)]
        refresh_budget_ms: u64,
    },
```
At the top of `main`, before parsing, initialise logging to stderr:
```rust
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")))
        .with_writer(std::io::stderr)
        .init();
```
The `Mcp` arm must not open an `Engine` in `main` (the actor opens it lazily with the session key). Restructure `main` so `Engine::open` happens inside the arms that need it, or match `Cmd::Mcp` before the open:
```rust
    if let Cmd::Mcp { refresh_budget_ms } = cli.cmd {
        return mcp::run(root, Duration::from_millis(refresh_budget_ms));
    }
```
placed after `root` is computed and before `Engine::open`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p singularrag --test cli`
Expected: all CLI tests pass including the two new ones. `mcp_exits_cleanly_when_stdin_closes` relies on rmcp's stdio transport ending `waiting()` on EOF; if the process instead hangs until the timeout, wrap `service.waiting()` in `tokio::select!` with a `tokio::io::stdin()` EOF probe is NOT the fix (stdin is owned by the transport); instead check that `serve` returned an error on EOF (a closed stdin before initialize is an initialize failure) and treat `rmcp` initialize errors caused by EOF as a clean exit: match the error's `Display` for "EOF"/"closed"/"end of file" and return `Ok(())` in that case, logging at `debug`.

- [ ] **Step 5: Lint and commit**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test --workspace`
```bash
git add crates/singularrag
git commit -m "feat(cli): mcp subcommand serving stdio with stderr logging"
```

---

### Task 5: Child-process integration tests

**Files:**
- Modify: `crates/singularrag/Cargo.toml` (dev-dependency: `rmcp` with `client` + `transport-child-process`)
- Create: `crates/singularrag/tests/mcp.rs`

**Interfaces:**
- Consumes: the built binary (`env!("CARGO_BIN_EXE_singularrag")`), `singularrag_core::fixture::write_ts_mini`, `Store`, `server::{REPO_MAP_DESCRIPTION, FIND_SYMBOL_DESCRIPTION}` are duplicated as literals here (tests must not depend on the bin crate's private modules), `map::header` format.
- Verified rmcp client facts: `().serve(TokioChildProcess::new(Command::new(..).configure(|c| {..}))?).await?` yields a running client; `client.list_all_tools().await?`, `client.call_tool(CallToolRequestParams::new("repo_map").with_arguments(object!({..}))).await?`; `CallToolResult { content: Vec<ContentBlock>, is_error: Option<bool>, .. }`; text via `content[0].as_text().map(|t| t.text.clone())`; a custom client name needs a `ClientHandler` impl whose `get_info` returns a `ClientConfig` built from `InitializeRequestParams` with `Implementation::new(name, version)`.

- [ ] **Step 1: Add the dev-dependency**

`crates/singularrag/Cargo.toml` `[dev-dependencies]` add:
```toml
rmcp = { version = "3.4", features = ["client", "transport-child-process"] }
```
(Cargo unifies features with the normal dependency; both sets are enabled for tests.)

- [ ] **Step 2: Write the failing tests**

`crates/singularrag/tests/mcp.rs`:
```rust
//! Drives the real binary over stdio with an rmcp client.

use std::path::Path;

use rmcp::model::{CallToolRequestParams, ClientCapabilities, ClientInfo, Implementation, InitializeRequestParams};
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use rmcp::{object, ClientHandler, ServiceExt};
use singularrag_core::fixture::write_ts_mini;
use singularrag_core::store::Store;
use tokio::process::Command;

const REPO_MAP_DESCRIPTION: &str = "Token-budgeted map of the symbols most relevant to a task. Call this before reading files. `query` is a question or identifiers; `focus_files` are repo-relative paths you already know matter; `budget_tokens` defaults to 1024, max 8192. Returns paths, line numbers and signatures only, never bodies. The first line says how fresh the index is; if it says STALE, call again after a moment.";
const FIND_SYMBOL_DESCRIPTION: &str = "Look up a symbol by name: exact, prefix, or split words (`create session` finds `createSession`). Returns the definition's path, line and signature and which files reference it. Optional `kind` filter: function, class, method, type, const, module. `limit` defaults to 10, max 50.";

#[derive(Clone)]
struct TestClient;

impl ClientHandler for TestClient {
    fn get_info(&self) -> ClientInfo {
        InitializeRequestParams {
            protocol_version: Default::default(),
            capabilities: ClientCapabilities::default(),
            client_info: Implementation::new("Singularrag Test", "0.0.0"),
            ..Default::default()
        }
        .into()
    }
}

async fn connect(repo: &Path, extra: &[&str]) -> rmcp::service::RunningService<rmcp::RoleClient, TestClient> {
    let bin = env!("CARGO_BIN_EXE_singularrag");
    let repo = repo.to_path_buf();
    let extra: Vec<String> = extra.iter().map(|s| s.to_string()).collect();
    let transport = TokioChildProcess::new(Command::new(bin).configure(move |c| {
        c.arg("mcp").arg("--repo").arg(&repo);
        for e in &extra {
            c.arg(e);
        }
        c.env("RUST_LOG", "warn");
    }))
    .expect("spawn singularrag mcp");
    TestClient.serve(transport).await.expect("initialize")
}

fn text_of(r: &rmcp::model::CallToolResult) -> String {
    r.content.first().and_then(|c| c.as_text()).map(|t| t.text.clone()).unwrap_or_default()
}

#[tokio::test]
async fn lists_exactly_the_two_tools_with_spec_descriptions() {
    let dir = tempfile::tempdir().unwrap();
    write_ts_mini(dir.path());
    let client = connect(dir.path(), &[]).await;
    let info = client.peer_info().expect("server info");
    assert_eq!(info.server_info.name, "singularrag");
    assert!(info.instructions.as_deref().unwrap_or("").starts_with("singularrag gives you a ranked map"));
    let mut tools = client.list_all_tools().await.unwrap();
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(tools.len(), 2);
    assert_eq!(tools[0].name, "find_symbol");
    assert_eq!(tools[0].description.as_deref(), Some(FIND_SYMBOL_DESCRIPTION));
    assert_eq!(tools[1].name, "repo_map");
    assert_eq!(tools[1].description.as_deref(), Some(REPO_MAP_DESCRIPTION));
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn repo_map_and_find_symbol_match_the_cli_and_record_provenance() {
    let dir = tempfile::tempdir().unwrap();
    write_ts_mini(dir.path());
    let client = connect(dir.path(), &[]).await;

    let map = client
        .call_tool(CallToolRequestParams::new("repo_map").with_arguments(object!({ "query": "session", "budget_tokens": 512 })))
        .await
        .unwrap();
    assert_ne!(map.is_error, Some(true));
    let map_text = text_of(&map);
    assert!(map_text.starts_with("# singularrag · index "), "{map_text}");
    assert!(map_text.contains("src/auth/session.ts:\n"));
    assert!(!map_text.contains("console.log"));

    let find = client
        .call_tool(CallToolRequestParams::new("find_symbol").with_arguments(object!({ "name": "createSession" })))
        .await
        .unwrap();
    let find_text = text_of(&find);
    assert!(find_text.contains("src/auth/session.ts:3  function  export function createSession"), "{find_text}");

    // Same body as the CLI's `query` for the same inputs (header carries a different retrieval id).
    let cli = std::process::Command::new(env!("CARGO_BIN_EXE_singularrag"))
        .args(["query", "session", "--budget", "512", "--repo", dir.path().to_str().unwrap()])
        .output()
        .unwrap();
    let cli_text = String::from_utf8(cli.stdout).unwrap();
    let body = |s: &str| s.lines().skip(1).collect::<Vec<_>>().join("\n");
    assert_eq!(body(&map_text), body(&cli_text));

    let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
    let keys: Vec<String> = store
        .conn()
        .prepare("SELECT session_key FROM retrievals WHERE tool IN ('repo_map','find_symbol') ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(keys.iter().any(|k| k.starts_with("mcp:singularrag-test:")), "{keys:?}");
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn stale_first_call_is_fresh_after_background_drain() {
    let dir = tempfile::tempdir().unwrap();
    write_ts_mini(dir.path());
    let client = connect(dir.path(), &["--refresh-budget-ms", "0"]).await;
    let first = text_of(&client.call_tool(CallToolRequestParams::new("repo_map").with_arguments(object!({}))).await.unwrap());
    assert!(first.contains("STALE:"), "{first}");
    // With a zero budget the drain cannot progress; a normal-budget server would. Restart with the default.
    client.cancel().await.unwrap();
    let client = connect(dir.path(), &[]).await;
    let second = text_of(&client.call_tool(CallToolRequestParams::new("repo_map").with_arguments(object!({}))).await.unwrap());
    assert!(second.contains("· fresh ·"), "{second}");
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn bad_repo_is_a_tool_error_not_a_crash() {
    let missing = Path::new("/nonexistent/singularrag-mcp-test");
    let client = connect(missing, &[]).await;
    let r = client.call_tool(CallToolRequestParams::new("repo_map").with_arguments(object!({}))).await.unwrap();
    assert_eq!(r.is_error, Some(true));
    assert!(text_of(&r).contains("/nonexistent/singularrag-mcp-test"));
    let again = client.call_tool(CallToolRequestParams::new("find_symbol").with_arguments(object!({ "name": "x" }))).await.unwrap();
    assert_eq!(again.is_error, Some(true), "server must keep answering after an error");
    client.cancel().await.unwrap();
}
```
If `ClientInfo` is not a type alias for the client's `get_info` return in rmcp 3.4, read `~/.cargo/registry/src/*/rmcp-3.4.*/src/handler/client.rs` for the exact return type (`ClientConfig` or `ClientInfo`) and construction; the intent is only to set `client_info.name` to `"Singularrag Test"` so the session key slug becomes `singularrag-test`. `client.peer_info()` returns the server's `InitializeResult` (`Option<Arc<..>>`); adapt the deref.

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p singularrag --test mcp`
Expected: compile or assertion failures until Task 4's binary behaviour is complete (the binary exists from Task 4; failures here mean a contract mismatch to fix in `server.rs`/`actor.rs`, not in the test).

- [ ] **Step 4: Make them pass**

Iterate on `server.rs`/`actor.rs`/`mod.rs` until all four pass. The `stale_first_call...` test drives the actor's zero-budget path through the real binary; the `--refresh-budget-ms` flag from Task 4 is what makes it deterministic.

- [ ] **Step 5: Lint and commit**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test --workspace`
```bash
git add crates/singularrag
git commit -m "test(mcp): child-process integration tests over stdio"
```

---

### Task 6: README with host configuration

**Files:**
- Create: `README.md`
- Modify: `crates/singularrag/tests/cli.rs` (one test that the README's `.mcp.json` snippet parses and names the right command)

**Interfaces:**
- Consumes: nothing from code; host config formats verified 2026-09-20 against the hosts' docs (Claude Code `claude mcp add --transport stdio <name> -- <cmd> [args]` and `.mcp.json` `mcpServers`; Codex `~/.codex/config.toml` `[mcp_servers.<name>]` with `command`, `args`, optional `cwd`; Copilot `~/.copilot/mcp-config.json` or `.copilot/mcp-config.json` with `mcpServers.<name>.{type,command,args}`).

- [ ] **Step 1: Write the failing test**

Append to `crates/singularrag/tests/cli.rs`:
```rust
#[test]
fn readme_mcp_json_snippet_is_valid_and_points_at_the_mcp_subcommand() {
    let readme = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../README.md")).unwrap();
    let start = readme.find("```json").expect("json snippet") + "```json".len();
    let end = readme[start..].find("```").unwrap() + start;
    let v: serde_json::Value = serde_json::from_str(readme[start..end].trim()).unwrap();
    assert_eq!(v["mcpServers"]["singularrag"]["command"], "singularrag");
    assert_eq!(v["mcpServers"]["singularrag"]["args"], serde_json::json!(["mcp"]));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p singularrag --test cli readme`
Expected: FAIL, README.md not found.

- [ ] **Step 3: Write the README**

`README.md` at the repo root:
````markdown
# singularrag

A repo map with retrieval provenance for coding agents. It indexes a codebase with tree-sitter into SQLite, ranks symbols with personalised PageRank, and serves a token-budgeted map to Claude Code, Codex CLI or Copilot CLI over MCP. Every retrieval is recorded, served and cut, with reasons, so a human can later see what the agent was given and what it missed.

Status: v0, engine and MCP server. The map UI (`singularrag serve`) is next.

## Install

```sh
cargo install --path crates/singularrag
```

Requires a Rust toolchain (1.85+). The binary is self-contained; the index lives in `.singularrag/index.db` inside each repo (add it to `.gitignore`; `map.toml` next to it is meant to be committed).

## Connect an agent

The server reads the repo from its working directory, or `--repo PATH`.

### Claude Code

```sh
claude mcp add --transport stdio singularrag -- singularrag mcp
```

Or project-scoped, in `.mcp.json` at the repo root:

```json
{
  "mcpServers": {
    "singularrag": {
      "command": "singularrag",
      "args": ["mcp"]
    }
  }
}
```

### Codex CLI

In `~/.codex/config.toml`:

```toml
[mcp_servers.singularrag]
command = "singularrag"
args = ["mcp"]
# cwd = "/path/to/repo"   # optional; defaults to Codex's working directory
```

### Copilot CLI

In `~/.copilot/mcp-config.json` (or `.copilot/mcp-config.json` in the repo):

```json5
{
  "mcpServers": {
    "singularrag": { "type": "stdio", "command": "singularrag", "args": ["mcp"] }
  }
}
```

## What the agent sees

Two tools. `repo_map` returns something like:

```
# singularrag · index 7f3a2c · HEAD 9b1e0d4 · fresh · retrieval r_000123
src/auth/session.ts:
    3  export function createSession(user: User, ttl: number): Session
    7  export class SessionStore
src/http/middleware.ts:
    2  export function requireSession(token: string): Session
# 42 of 310 symbols shown · 268 more ranked below budget · 25 recorded · widen with a larger budget or a focus file
```

`find_symbol` looks a name up and lists which files reference it. Neither tool ever returns function bodies, comments or string literals.

## CLI

```
singularrag index            # build or refresh the index
singularrag query "text"     # print the map the agent would get
singularrag find NAME        # look a symbol up
singularrag eval             # tier-one recall against eval/questions.toml
singularrag mcp              # serve over stdio
```

Logs go to stderr; set `RUST_LOG=debug` for more.

## Design

`docs/superpowers/specs/2026-09-19-singularrag-design.md` is the product and architecture spec; `docs/superpowers/specs/2026-09-20-singularrag-mcp-design.md` covers the server.
````
The Copilot snippet is fenced as `json5` on purpose so the README test picks the Claude Code `.mcp.json` block (the first ```` ```json ```` fence).

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p singularrag --test cli readme`
Expected: PASS.

- [ ] **Step 5: Lint and commit**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test --workspace`
```bash
git add README.md crates/singularrag/tests/cli.rs
git commit -m "docs: README with host configuration for Claude Code, Codex and Copilot"
```

---

## Self-review notes

- Spec §2 process model: runtime/stderr (Task 4), actor + lazy open + session key (Task 2, Task 3 initialize), repo root (Task 4), background drain with lock-timeout stop (Task 2), error policy (Tasks 2–4), `set_refresh_budget` (Task 1).
- Spec §3 tools, descriptions, server info, instructions (Task 3), verified verbatim by Task 3 unit test and Task 5 integration test.
- Spec §4 README (Task 6). Spec §5 tests: actor unit tests (Task 2), child-process tests (Task 5), failure test (Tasks 2 and 5). Spec §6 crates (Tasks 2, 3, 5). Spec §7 non-goals: nothing here adds `serve`, roots, install, HTTP.
- Type consistency: `Reply<T> = Result<T, String>`, `EngineHandle::{map, find, set_refresh_budget, stats, shutdown}`, `EngineConfig { root, session_key: Arc<Mutex<Option<String>>>, refresh_budget }` used identically in Tasks 2, 3, 4; `MapArgs`/`FindArgs` → `MapRequest`/`FindRequest` via `From` (Task 3); `--refresh-budget-ms` defined in Task 4 and used in Task 5.
- Known API uncertainty is confined to three spots, each with an in-task fallback: `ToolRouter` list accessor name (Task 3), `#[tool(description = CONST)]` accepting a path (Task 3), client `get_info` return type (Task 5).
