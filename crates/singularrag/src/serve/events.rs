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
                let Ok(store) = state.read.lock() else {
                    continue;
                };
                match (store.data_version(), queries::max_retrieval_id(&store)) {
                    (Ok(v), Ok(max)) => Some((v, max)),
                    _ => None,
                }
            };
            let Some((v, max)) = snapshot else { continue };
            if last.is_some_and(|l| l != v) {
                let _ = state.events.send(ServerEvent::Change {
                    max_retrieval_id: max,
                });
            }
            last = Some(v);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serve::state::{Freshness, ServerEvent};
    use singularrag_core::engine::{Engine, MapRequest};
    use singularrag_core::fixture::write_ts_mini;

    #[test]
    fn events_serialise_with_their_names() {
        let e = to_sse_event(&ServerEvent::Change {
            max_retrieval_id: 7,
        });
        let s = format!("{e:?}");
        assert!(s.contains("change"), "{s}");
        let e = to_sse_event(&ServerEvent::Freshness(Freshness {
            stale_count: 2,
            ..Default::default()
        }));
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
        // repo_map runs a refresh before recording the retrieval, so the first commit is an index
        // write, and the retrieval row arrives in a later commit. Loop until we get the event
        // with max_retrieval_id=1.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                panic!("timeout waiting for max_retrieval_id=1");
            }
            let ev = tokio::time::timeout(remaining, rx.recv())
                .await
                .expect("event within 3 s")
                .unwrap();
            match ev {
                ServerEvent::Change { max_retrieval_id } => {
                    if max_retrieval_id == 1 {
                        break;
                    }
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn poller_broadcasts_change_for_a_write_that_adds_no_retrieval() {
        let dir = tempfile::tempdir().unwrap();
        write_ts_mini(dir.path());
        singularrag_core::store::Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        let state = crate::serve::state::AppState::new(dir.path().to_path_buf(), 1).unwrap();
        let mut rx = state.events.subscribe();
        let _poller = spawn_poller(state.clone());
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        let db_path = dir.path().join(".singularrag/index.db");
        tokio::task::spawn_blocking(move || {
            let store = singularrag_core::store::Store::open(&db_path).unwrap();
            store.set_meta("git_head", "deadbeef").unwrap();
        })
        .await
        .unwrap();
        let ev = tokio::time::timeout(std::time::Duration::from_secs(3), rx.recv())
            .await
            .expect("event within 3 s")
            .unwrap();
        match ev {
            ServerEvent::Change {
                max_retrieval_id: _,
            } => {
                // Event received; max_retrieval_id value doesn't matter for this test
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
