//! The Engine actor: one OS thread owns the `Engine`; everyone else talks to it over a
//! channel. This is how a `!Sync` Engine serves an async, multi-connection host, and
//! where spec §8's background refresh lives.

use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use singularrag_core::engine::{Engine, FindRequest, FindResponse, MapRequest, MapResponse};
use singularrag_core::index::IndexStats;
use tokio::sync::oneshot;

/// Errors are stringified at the actor boundary: replies must be `Send + 'static`,
/// and the MCP layer only ever renders them as `is_error` text.
pub type Reply<T> = Result<T, String>;

/// The last background `refresh` the actor saw, plus how many chunks have run. `chunks`
/// exists so callers (and tests) can tell "no drain has happened yet" apart from "the
/// drain finished with nothing remaining" — both look like `remaining == 0` otherwise.
///
/// Not on the serve API (spec §5, Task 4): the watcher is the only freshness writer
/// there. `Job::Stats`/`DrainStats` stay for the MCP drain loop, which is not wired up
/// yet — hence `#[allow(dead_code)]`, matching the note on `Job::Stats` below.
#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub struct DrainStats {
    pub last: IndexStats,
    pub chunks: u64,
}

pub enum Job {
    Map(MapRequest, oneshot::Sender<Reply<MapResponse>>),
    Find(FindRequest, oneshot::Sender<Reply<FindResponse>>),
    /// One budgeted refresh with no retrieval recorded and no drain armed: the file
    /// watcher's job, not a tool response. Constructed by `serve::watcher` through
    /// `EngineHandle::refresh`, and exercised directly by the actor's unit tests.
    Refresh(oneshot::Sender<Reply<IndexStats>>),
    // Constructed only by `EngineHandle::set_refresh_budget`/`stats`, which are currently
    // test-only (see the `#[allow(dead_code)]` note on `DrainStats`).
    #[allow(dead_code)]
    SetRefreshBudget(Duration, oneshot::Sender<Reply<()>>),
    #[allow(dead_code)]
    Stats(oneshot::Sender<Reply<DrainStats>>),
    Shutdown,
}

/// Where the `<client>` part of the session key comes from.
#[derive(Clone)]
pub enum SessionKey {
    /// A process that knows its own name (`serve`, tests): the key is used verbatim.
    /// Built by `serve::run` (`SessionKey::Fixed("serve".into())`); also exercised
    /// directly by the actor's unit tests.
    Fixed(String),
    /// The MCP server: filled from `clientInfo.name` during initialize, read at first job.
    FromHandshake(Arc<Mutex<Option<String>>>),
}

#[derive(Clone)]
pub struct EngineConfig {
    pub root: PathBuf,
    pub session_key: SessionKey,
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

    // One budgeted refresh with no retrieval recorded. The watcher's job; see the
    // doc comment on `Job::Refresh`.
    pub async fn refresh(&self) -> Reply<IndexStats> {
        self.ask(Job::Refresh).await
    }

    // Test-only for now; see the `#[allow(dead_code)]` note on `DrainStats`.
    #[allow(dead_code)]
    pub async fn set_refresh_budget(&self, budget: Duration) -> Reply<()> {
        self.ask(|tx| Job::SetRefreshBudget(budget, tx)).await
    }

    #[allow(dead_code)]
    pub async fn stats(&self) -> Reply<DrainStats> {
        self.ask(Job::Stats).await
    }

    pub fn shutdown(&self) {
        let _ = self.tx.send(Job::Shutdown);
    }
}

/// Sends the moment the actor thread leaves `run`, by returning *or* by unwinding out
/// of a panic — `Drop` runs either way. That is the point: spec §2's one fatal path is
/// the actor dying, and a panic is how it dies without anyone asking it to.
struct DiedGuard(Option<oneshot::Sender<()>>);

impl Drop for DiedGuard {
    fn drop(&mut self) {
        if let Some(tx) = self.0.take() {
            let _ = tx.send(());
        }
    }
}

/// Returns the handle, the thread's join handle, and a receiver that resolves when the
/// actor thread is gone for any reason. `mcp::run` watches the third.
pub fn spawn(config: EngineConfig) -> (EngineHandle, JoinHandle<()>, oneshot::Receiver<()>) {
    let (tx, rx) = mpsc::channel::<Job>();
    let (died_tx, died_rx) = oneshot::channel();
    let join = std::thread::Builder::new()
        .name("singularrag-engine".into())
        .spawn(move || {
            let _died = DiedGuard(Some(died_tx));
            run(config, rx)
        })
        .expect("spawn engine thread");
    (EngineHandle { tx }, join, died_rx)
}

struct Actor {
    config: EngineConfig,
    /// When this actor started, not when its first job arrived: spec §2's session key is
    /// `mcp:<client>:<pid>:<start_ms>`, and `<pid>:<start_ms>` is what makes two runs of
    /// the same host in the same repo tell apart in the retrieval rail.
    start_ms: i64,
    engine: Option<Result<Engine, String>>,
    last_stats: IndexStats,
    /// Number of `drain_chunk` calls that have run a `refresh` (incremented even when
    /// that refresh hit `lock_timeout`), so `Job::Stats` can distinguish "no drain chunk
    /// has run yet" from "a drain chunk ran and found nothing left to do".
    drain_chunks: u64,
    /// True after a response reported stale files and the refresh did not lose the lock.
    drain_pending: bool,
    /// Backlog the current drain is working against: the `stale_count` that armed it,
    /// then each chunk's `remaining`. A chunk that does not shrink it made no progress.
    drain_remaining: usize,
}

impl Actor {
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

    fn note(&mut self, stale_count: usize, lock_timeout: bool) {
        // The plan's Global Constraints: arm the drain only after a response with
        // stale_count > 0 whose refresh did not hit lock_timeout. A response that lost
        // the lock says another process is already indexing this repo, and its backlog
        // is not ours to chase: each doomed chunk would spend LOCK_WAIT_MS discovering
        // the holder again. The next response re-arms us once the lock is free.
        self.drain_pending = stale_count > 0 && !lock_timeout;
        self.drain_remaining = if self.drain_pending { stale_count } else { 0 };
    }

    fn handle(&mut self, job: Job) -> bool {
        match job {
            Job::Map(req, reply) => {
                let out = self
                    .engine()
                    .and_then(|e| e.repo_map(&req).map_err(|e| e.to_string()));
                if let Ok(r) = &out {
                    self.note(r.stale_count, r.lock_timeout);
                }
                let _ = reply.send(out);
            }
            Job::Find(req, reply) => {
                let out = self
                    .engine()
                    .and_then(|e| e.find_symbol(&req).map_err(|e| e.to_string()));
                if let Ok(r) = &out {
                    self.note(r.stale_count, r.lock_timeout);
                }
                let _ = reply.send(out);
            }
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
            Job::SetRefreshBudget(budget, reply) => {
                self.config.refresh_budget = budget;
                // A drain that stopped for want of budget deserves another go with the
                // new one; without this, "too small to index a file" would be permanent
                // for the rest of the session.
                if self.drain_remaining > 0 {
                    self.drain_pending = true;
                }
                let out = self.engine().map(|e| e.set_refresh_budget(budget));
                let _ = reply.send(out);
            }
            Job::Stats(reply) => {
                let _ = reply.send(Ok(DrainStats {
                    last: self.last_stats.clone(),
                    chunks: self.drain_chunks,
                }));
            }
            Job::Shutdown => return false,
        }
        true
    }

    /// One drain chunk. Returns true when more work remains, the lock was ours, and
    /// this chunk actually shrank the backlog.
    ///
    /// The progress term is what keeps a budget smaller than one file's parse from
    /// spinning: `Indexer::refresh_with` walks the tree before it looks at the deadline,
    /// so such a chunk indexes nothing, reports the same `remaining` as the last one, and
    /// would re-arm the drain forever — re-walking, rewriting meta and taking the
    /// advisory lock tens of times a second. Progress is measured as the backlog
    /// shrinking rather than as `indexed + skipped + removed > 0`, because the walk
    /// re-reports denylisted files (`.env` and friends) in `skipped` on *every* chunk,
    /// so that sum is never zero on a real repo. `indexed`/`removed` are still accepted
    /// as progress on their own, so a chunk that keeps up with a repo being edited
    /// underneath it is not mistaken for a stalled one.
    fn drain_chunk(&mut self) -> bool {
        let budget = self.config.refresh_budget;
        let Ok(engine) = self.engine() else {
            return false;
        };
        let refreshed = engine.refresh(budget);
        self.drain_chunks += 1;
        match refreshed {
            Ok(stats) => {
                let progressed = stats.indexed > 0
                    || stats.removed > 0
                    || stats.remaining < self.drain_remaining;
                let more = stats.remaining > 0 && !stats.lock_timeout && progressed;
                if stats.remaining > 0 && !stats.lock_timeout && !progressed {
                    tracing::warn!(
                        "refresh budget too small to index a file; background drain stopped"
                    );
                }
                self.drain_remaining = stats.remaining;
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
    let mut actor = Actor {
        config,
        start_ms: singularrag_core::time::now_ms(),
        engine: None,
        last_stats: IndexStats::default(),
        drain_chunks: 0,
        drain_pending: false,
        drain_remaining: 0,
    };
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
        // Nothing to drain: block. `drain_pending` only ever changes inside this loop
        // (in `handle`), so there is nothing a timeout could wake up to notice — the
        // old 20 ms poll just woke the thread fifty times a second to find that out.
        match rx.recv() {
            Ok(job) => {
                if !actor.handle(job) {
                    return;
                }
            }
            Err(mpsc::RecvError) => return,
        }
    }
}

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
            session_key: SessionKey::FromHandshake(Arc::new(Mutex::new(Some(
                "test-client".into(),
            )))),
            refresh_budget: budget,
        }
    }

    #[tokio::test]
    async fn map_and_find_match_a_direct_engine_call() {
        let dir = fixture();
        let (handle, _join, _died) = spawn(config(dir.path(), REFRESH_BUDGET));
        let map = handle
            .map(MapRequest {
                query: Some("session".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(map.text.starts_with("# singularrag · index "));
        assert!(map
            .text
            .lines()
            .any(|l| l.starts_with("src/auth/session.ts:")));
        let find = handle
            .find(FindRequest {
                name: "createSession".into(),
                kind: None,
                limit: 10,
            })
            .await
            .unwrap();
        assert!(find
            .text
            .contains("src/auth/session.ts:3  function  export function createSession"));

        // Same text the engine produces directly for the same inputs (modulo retrieval id).
        let mut e = Engine::open(dir.path(), "direct").unwrap();
        let direct = e
            .repo_map(&MapRequest {
                query: Some("session".into()),
                ..Default::default()
            })
            .unwrap();
        let strip = |s: &str| s.lines().skip(1).collect::<Vec<_>>().join("\n");
        assert_eq!(strip(&map.text), strip(&direct.text));
    }

    #[tokio::test]
    async fn session_key_carries_client_name_pid_and_start() {
        let dir = fixture();
        let (handle, _join, _died) = spawn(config(dir.path(), REFRESH_BUDGET));
        handle.map(MapRequest::default()).await.unwrap();
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        let key: String = store
            .conn()
            .query_row(
                "SELECT session_key FROM retrievals ORDER BY id DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
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
        cfg.session_key = SessionKey::FromHandshake(Arc::new(Mutex::new(None)));
        let (handle, _join, _died) = spawn(cfg);
        handle.map(MapRequest::default()).await.unwrap();
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        let key: String = store
            .conn()
            .query_row(
                "SELECT session_key FROM retrievals ORDER BY id DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(key.starts_with("mcp:unknown:"), "{key}");
    }

    #[tokio::test]
    async fn stale_first_response_is_drained_in_the_background() {
        let dir = fixture();
        // Zero budget: the first call indexes nothing and reports STALE.
        let (handle, _join, _died) = spawn(config(dir.path(), Duration::ZERO));
        let first = handle.map(MapRequest::default()).await.unwrap();
        assert!(first.stale_count > 0, "{first:?}");
        // The drain uses the same (zero) budget, so it cannot make progress by itself;
        // raise the budget through the handle and wait for the drain to finish.
        handle.set_refresh_budget(REFRESH_BUDGET).await.unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let final_stats = loop {
            let stats = handle.stats().await.unwrap();
            if stats.chunks >= 1 && stats.last.remaining == 0 {
                break stats;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "drain never finished: {stats:?}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        assert!(final_stats.chunks >= 1);
        let second = handle.map(MapRequest::default()).await.unwrap();
        assert_eq!(second.stale_count, 0);
        assert!(second.text.contains("· fresh ·"));
    }

    /// A budget too small to index even one file makes every chunk identical: the walk
    /// runs, nothing is indexed, `remaining` does not move. Without a progress term the
    /// actor re-arms forever and burns a core (measured at ~85% on a real repo with
    /// `--refresh-budget-ms 1`). One chunk, then silence.
    #[tokio::test]
    async fn a_chunk_that_makes_no_progress_stops_the_drain() {
        let dir = fixture();
        let (handle, _join, _died) = spawn(config(dir.path(), Duration::ZERO));
        let first = handle.map(MapRequest::default()).await.unwrap();
        assert!(first.stale_count > 0, "{first:?}");
        tokio::time::sleep(Duration::from_millis(300)).await;
        let a = handle.stats().await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let b = handle.stats().await.unwrap();
        assert_eq!(
            a.chunks, b.chunks,
            "the drain kept re-arming with no progress: {a:?} then {b:?}"
        );
        assert!(
            a.chunks <= 1,
            "one no-progress chunk is enough to stop: {a:?}"
        );
    }

    #[tokio::test]
    async fn drain_does_not_run_while_another_process_holds_the_lock() {
        let dir = fixture();
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        let foreign_pid = std::process::id() + 1;
        assert!(lock::try_acquire(&store, foreign_pid, singularrag_core::time::now_ms()).unwrap());
        let (handle, _join, _died) = spawn(config(dir.path(), REFRESH_BUDGET));
        let first = handle.map(MapRequest::default()).await.unwrap();
        assert!(first.stale_count > 0);
        tokio::time::sleep(Duration::from_millis(300)).await;
        let n: i64 = store
            .conn()
            .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            n, 0,
            "drain indexed while the lock was held by another process"
        );
        let stats = handle.stats().await.unwrap();
        assert_eq!(
            stats.chunks, 0,
            "a response whose refresh lost the lock must not arm the drain at all: {stats:?}"
        );
        lock::release(&store, foreign_pid).unwrap();
    }

    #[tokio::test]
    async fn bad_root_yields_errors_and_the_actor_stays_alive() {
        let cfg = EngineConfig {
            root: std::path::PathBuf::from("/nonexistent/singularrag-test-root"),
            session_key: SessionKey::FromHandshake(Arc::new(Mutex::new(None))),
            refresh_budget: REFRESH_BUDGET,
        };
        let (handle, _join, _died) = spawn(cfg);
        let err = handle.map(MapRequest::default()).await.unwrap_err();
        assert!(err.contains("/nonexistent/singularrag-test-root"), "{err}");
        let err2 = handle
            .find(FindRequest {
                name: "x".into(),
                kind: None,
                limit: 5,
            })
            .await
            .unwrap_err();
        assert!(err2.contains("/nonexistent/singularrag-test-root"));
    }

    /// Spec §2's only fatal path: the actor thread dying. `mcp::run` selects on this
    /// receiver and exits non-zero so the host restarts the server instead of every
    /// later tool call answering "engine thread is gone" forever. The signal is a guard
    /// whose `Drop` sends, so it fires on a panic (which unwinds through it) exactly as
    /// it does on a clean return; a clean return is what this test can observe in-process.
    #[tokio::test]
    async fn the_died_signal_fires_when_the_actor_thread_leaves() {
        let dir = fixture();
        let (handle, join, died) = spawn(config(dir.path(), REFRESH_BUDGET));
        handle.map(MapRequest::default()).await.unwrap();
        handle.shutdown();
        join.join().unwrap();
        tokio::time::timeout(Duration::from_secs(5), died)
            .await
            .expect("died signal never fired")
            .expect("died sender was dropped without sending");
    }

    #[tokio::test]
    async fn dropped_engine_thread_reports_gone() {
        let dir = fixture();
        let (handle, join, _died) = spawn(config(dir.path(), REFRESH_BUDGET));
        handle.shutdown();
        join.join().unwrap();
        let err = handle.map(MapRequest::default()).await.unwrap_err();
        assert!(err.contains("engine thread is gone"), "{err}");
    }

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
            .query_row(
                "SELECT session_key FROM retrievals ORDER BY id DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
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
        let n: i64 = store
            .conn()
            .query_row("SELECT COUNT(*) FROM retrievals", [], |r| r.get(0))
            .unwrap();
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
}
