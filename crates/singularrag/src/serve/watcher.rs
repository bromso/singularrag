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

use super::state::{AppState, ServerEvent};
use crate::actor::EngineHandle;

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

/// Flip the "this process is indexing" flag and broadcast it, so the badge can say
/// "indexing" for the seconds a refresh takes (I8).
fn set_indexing(state: &AppState, on: bool) {
    let snapshot = {
        let Ok(mut f) = state.freshness.write() else {
            return;
        };
        if f.indexing == on {
            return;
        }
        f.indexing = on;
        f.clone()
    };
    let _ = state.events.send(ServerEvent::Freshness(snapshot));
}

pub async fn refresh_once(state: &AppState, handle: &EngineHandle) {
    for attempt in 0..2 {
        set_indexing(state, true);
        let result = handle.refresh().await;
        set_indexing(state, false);
        match result {
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
    let Ok(rel) = p.strip_prefix(root) else {
        return false;
    };
    if rel.starts_with(".git") {
        return false;
    }
    if rel.starts_with(".singularrag") {
        // The index and its WAL are our own writes — watching them would loop. `map.toml`
        // is the exception: a hand edit there must refresh (the Engine reloads the config
        // by mtime) and bump `data_version` so the UI refetches the map it is editing.
        return rel == Path::new(singularrag_core::config::MAP_FILE);
    }
    true
}

/// Start watching `state.root`. The returned debouncer must be kept alive.
pub fn start(
    state: AppState,
    handle: EngineHandle,
) -> anyhow::Result<Debouncer<notify::RecommendedWatcher, RecommendedCache>> {
    let (tx, mut rx) = mpsc::channel::<()>(4);
    let root = state.root.clone();
    let root_for_handler = root.clone();
    let mut debouncer = new_debouncer(DEBOUNCE, None, move |res: DebounceEventResult| {
        if let Ok(events) = res {
            if events
                .iter()
                .any(|e| e.paths.iter().any(|p| interesting(&root_for_handler, p)))
            {
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
    fn map_toml_is_watched_but_the_rest_of_dot_singularrag_is_not() {
        let root = Path::new("/repo");
        assert!(interesting(root, &root.join("src/a.ts")));
        assert!(
            interesting(root, &root.join(".singularrag/map.toml")),
            "a hand edit to map.toml must produce a refresh (C1)"
        );
        assert!(!interesting(root, &root.join(".singularrag/index.db")));
        assert!(!interesting(root, &root.join(".singularrag/index.db-wal")));
        assert!(!interesting(root, &root.join(".singularrag/map.toml.tmp")));
        assert!(!interesting(root, &root.join(".git/HEAD")));
    }

    #[tokio::test]
    async fn refresh_once_updates_freshness_and_broadcasts() {
        let (_dir, state, handle) = setup();
        let mut rx = state.events.subscribe();
        refresh_once(&state, &handle).await;
        let f = state.freshness.read().unwrap().clone();
        assert_eq!(f.stale_count, 0);
        assert!(!f.foreign_indexing);
        assert!(!f.indexing, "the flag is cleared when the refresh returns");
        assert!(f.indexed_at_ms.is_some());
        let mut seen = Vec::new();
        while let Ok(crate::serve::state::ServerEvent::Freshness(f)) = rx.try_recv() {
            seen.push(f);
        }
        assert!(!seen.is_empty());
        assert!(
            seen.iter().any(|f| f.indexing),
            "the badge must be told a refresh started"
        );
        assert!(!seen.last().unwrap().indexing);
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
