//! File watcher → one `Job::Refresh` per debounced batch → freshness snapshot + SSE.
//! `lock_timeout` means another process (the MCP server) is indexing: reported, retried
//! once, never an error (spec §2).

use std::path::Path;
use std::time::Duration;

use notify::RecursiveMode;
use notify_debouncer_full::{new_debouncer, DebounceEventResult, Debouncer, RecommendedCache};
use singularrag_core::index::IndexStats;
use singularrag_core::workspace::Workspace;
use tokio::sync::mpsc;

use super::state::{AppState, ServerEvent};
use crate::actor::EngineHandle;

pub const DEBOUNCE: Duration = Duration::from_millis(300);
pub const LOCK_RETRY_AFTER: Duration = Duration::from_secs(2);

fn broadcast_status(state: &AppState) {
    let f = state
        .freshness
        .read()
        .map(|g| g.clone())
        .unwrap_or_default();
    let dto = super::queries::status(&state.store(), &f, &state.ws);
    match dto {
        Ok(dto) => {
            let _ = state.events.send(ServerEvent::Freshness(dto));
        }
        Err(e) => tracing::warn!("status for freshness event failed: {e}"),
    }
}

/// A refresh is starting: the badge may say "indexing" for the seconds it takes (I8).
pub fn apply_started(state: &AppState) {
    if let Ok(mut f) = state.freshness.write() {
        f.indexing = true;
    }
    broadcast_status(state);
}

pub fn apply(state: &AppState, stats: IndexStats) {
    if let Ok(mut f) = state.freshness.write() {
        f.stale_count = stats.remaining;
        f.lock_timeout = stats.lock_timeout;
        f.foreign_indexing = stats.lock_timeout;
        f.indexing = false;
    }
    broadcast_status(state);
}

fn apply_failed(state: &AppState) {
    if let Ok(mut f) = state.freshness.write() {
        f.indexing = false;
    }
    broadcast_status(state);
}

pub async fn refresh_once(state: &AppState, handle: &EngineHandle) {
    for attempt in 0..2 {
        apply_started(state);
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
                apply_failed(state);
                return;
            }
        }
    }
}

/// `p` is worth a refresh: it is under a workspace root (any repo's `.git` aside), or it
/// is the workspace's own `map.toml` (a hand edit there must refresh — the Engine reloads
/// the config by mtime — and bump `data_version` so the UI refetches the map it is
/// editing). The rest of `.singularrag` (the index and its WAL) is our own writes, and
/// watching them would loop.
fn interesting(ws: &Workspace, p: &Path) -> bool {
    if p == ws.dir.join(singularrag_core::config::MAP_FILE) {
        return true;
    }
    let Some(rel) = ws.rel_of(p) else {
        return false;
    };
    let inner = rel
        .split_once('/')
        .map(|(_, r)| r)
        .filter(|_| ws.is_named())
        .unwrap_or(&rel);
    !inner.starts_with(".git") && !inner.starts_with(".singularrag")
}

/// Start watching every root in `state.ws`, plus (for a named workspace) the
/// `.singularrag` directory non-recursively so a hand-edited `map.toml` is seen. The
/// returned debouncer must be kept alive.
pub fn start(
    state: AppState,
    handle: EngineHandle,
) -> anyhow::Result<Debouncer<notify::RecommendedWatcher, RecommendedCache>> {
    let (tx, mut rx) = mpsc::channel::<()>(4);
    let ws = state.ws.clone();
    let ws_for_handler = ws.clone();
    let mut debouncer = new_debouncer(DEBOUNCE, None, move |res: DebounceEventResult| {
        if let Ok(events) = res {
            if events
                .iter()
                .any(|e| e.paths.iter().any(|p| interesting(&ws_for_handler, p)))
            {
                let _ = tx.try_send(());
            }
        }
    })?;
    for r in &ws.roots {
        debouncer.watch(&r.path, RecursiveMode::Recursive)?;
    }
    if ws.is_named() {
        let dot_dir = ws.dir.join(".singularrag");
        std::fs::create_dir_all(&dot_dir)?;
        debouncer.watch(&dot_dir, RecursiveMode::NonRecursive)?;
    }
    tokio::spawn(async move {
        while rx.recv().await.is_some() {
            refresh_once(&state, &handle).await;
        }
    });
    Ok(debouncer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::{spawn, EngineConfig, SessionKey};
    use singularrag_core::fixture::write_ts_mini;
    use singularrag_core::store::{lock, Store};
    use std::time::Duration;

    fn setup() -> (
        tempfile::TempDir,
        crate::serve::state::AppState,
        crate::actor::EngineHandle,
    ) {
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

    #[test]
    fn interesting_paths_follow_the_roots_and_the_workspace_map_file() {
        let d = tempfile::tempdir().unwrap();
        let (app, notes) = singularrag_core::fixture::write_workspace(d.path());
        // Canonicalize: `interesting` compares against `ws.roots`, which `Workspace::open`
        // canonicalizes, and on macOS `TMPDIR` is itself a symlink (`/var` -> `/private/var`).
        let app = app.canonicalize().unwrap();
        let notes = notes.canonicalize().unwrap();
        let ws = singularrag_core::workspace::Workspace::open(d.path()).unwrap();
        assert!(interesting(&ws, &app.join("src/a.ts")));
        assert!(interesting(&ws, &notes.join("n.md")));
        assert!(!interesting(&ws, &app.join(".git/HEAD")));
        assert!(interesting(&ws, &ws.dir.join(".singularrag/map.toml")));
        assert!(!interesting(&ws, &ws.dir.join(".singularrag/index.db")));
        assert!(
            !interesting(&ws, &ws.dir.join("unrelated.txt")),
            "the workspace dir is not a root"
        );
    }

    #[tokio::test]
    async fn a_poisoned_store_mutex_still_serves_status_and_freshness() {
        let (_dir, state, _handle) = setup();
        let poisoner = state.clone();
        let _ = std::thread::spawn(move || {
            let _guard = poisoner.read.lock().unwrap();
            panic!("poison the read store on purpose");
        })
        .join();
        assert!(state.read.lock().is_err(), "the mutex should be poisoned");
        let mut rx = state.events.subscribe();
        broadcast_status(&state);
        match rx.try_recv() {
            Ok(ServerEvent::Freshness(dto)) => assert_eq!(dto.stale_count, 0),
            other => panic!("expected a freshness event, got {other:?}"),
        }
        match crate::serve::routes::status(axum::extract::State(state)).await {
            Ok(status) => assert_eq!(status.0.stale_count, 0),
            Err(_) => panic!("status must answer after a poisoned lock"),
        }
    }

    #[tokio::test]
    async fn refresh_once_broadcasts_exactly_two_status_events() {
        let (_dir, state, handle) = setup();
        let mut rx = state.events.subscribe();
        refresh_once(&state, &handle).await;
        let f = state.freshness.read().unwrap().clone();
        assert_eq!(f.stale_count, 0);
        assert!(!f.foreign_indexing);
        assert!(!f.indexing);
        let mut seen = Vec::new();
        while let Ok(crate::serve::state::ServerEvent::Freshness(s)) = rx.try_recv() {
            seen.push(s);
        }
        assert_eq!(seen.len(), 2, "one 'started', one result: {seen:?}");
        assert!(seen[0].indexing);
        assert!(!seen[1].indexing);
        assert!(
            !seen[1].index_version.is_empty(),
            "the payload is a full status"
        );
        assert_eq!(seen[1].files.indexed, 5, "four .ts files and README.md");
        assert!(seen[1].indexed_at_ms.is_some());
    }

    #[tokio::test]
    async fn foreign_lock_is_reported_not_errored() {
        let (dir, state, handle) = setup();
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        let foreign = std::process::id() + 1;
        assert!(lock::try_acquire(&store, foreign, singularrag_core::time::now_ms()).unwrap());
        let started = std::time::Instant::now();
        refresh_once(&state, &handle).await;
        assert!(
            started.elapsed() >= Duration::from_secs(2),
            "must retry once after 2 s"
        );
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
        std::fs::write(
            dir.path().join("src/util/log.ts"),
            "export function logRenamed(): void {}\n",
        )
        .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let store = Store::open_read_only(&dir.path().join(".singularrag/index.db")).unwrap();
            let n: i64 = store
                .conn()
                .query_row(
                    "SELECT COUNT(*) FROM symbols WHERE name = 'logRenamed'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            if n == 1 {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "watcher never reindexed the changed file"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}
