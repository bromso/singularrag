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

/// The last background `refresh` the actor saw, plus how many chunks have run. `chunks`
/// exists so callers (and tests) can tell "no drain has happened yet" apart from "the
/// drain finished with nothing remaining" — both look like `remaining == 0` otherwise.
#[derive(Debug, Clone, Default)]
pub struct DrainStats {
    pub last: IndexStats,
    pub chunks: u64,
}

pub enum Job {
    Map(MapRequest, oneshot::Sender<Reply<MapResponse>>),
    Find(FindRequest, oneshot::Sender<Reply<FindResponse>>),
    SetRefreshBudget(Duration, oneshot::Sender<Reply<()>>),
    Stats(oneshot::Sender<Reply<DrainStats>>),
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

    pub async fn stats(&self) -> Reply<DrainStats> {
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
    /// Number of `drain_chunk` calls that have run a `refresh` (incremented even when
    /// that refresh hit `lock_timeout`), so `Job::Stats` can distinguish "no drain chunk
    /// has run yet" from "a drain chunk ran and found nothing left to do".
    drain_chunks: u64,
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
        format!(
            "mcp:{client}:{}:{}",
            std::process::id(),
            singularrag_core::time::now_ms()
        )
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
        // drain_pending is derived from stale_count only; a foreign lock is detected one
        // chunk later by drain_chunk (refresh reports lock_timeout and indexes nothing).
        self.drain_pending = stale_count > 0;
    }

    fn handle(&mut self, job: Job) -> bool {
        match job {
            Job::Map(req, reply) => {
                let out = self
                    .engine()
                    .and_then(|e| e.repo_map(&req).map_err(|e| e.to_string()));
                if let Ok(r) = &out {
                    self.note(r.stale_count);
                }
                let _ = reply.send(out);
            }
            Job::Find(req, reply) => {
                let out = self
                    .engine()
                    .and_then(|e| e.find_symbol(&req).map_err(|e| e.to_string()));
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
                let _ = reply.send(Ok(DrainStats {
                    last: self.last_stats.clone(),
                    chunks: self.drain_chunks,
                }));
            }
            Job::Shutdown => return false,
        }
        true
    }

    /// One drain chunk. Returns true when more work remains and the lock was ours.
    fn drain_chunk(&mut self) -> bool {
        self.drain_chunks += 1;
        let budget = self.config.refresh_budget;
        let Ok(engine) = self.engine() else {
            return false;
        };
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
    let mut actor = Actor {
        config,
        engine: None,
        last_stats: IndexStats::default(),
        drain_chunks: 0,
        drain_pending: false,
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
        let map = handle
            .map(MapRequest {
                query: Some("session".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(map.text.starts_with("# singularrag · index "));
        assert!(map.text.contains("src/auth/session.ts:\n"));
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
        let (handle, _join) = spawn(config(dir.path(), REFRESH_BUDGET));
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
        cfg.session_key = Arc::new(Mutex::new(None));
        let (handle, _join) = spawn(cfg);
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
        let (handle, _join) = spawn(config(dir.path(), Duration::ZERO));
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
            stats.chunks, 1,
            "exactly one drain chunk should have run and hit lock_timeout: {stats:?}"
        );
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
