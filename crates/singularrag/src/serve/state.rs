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
        DrainStatsJson {
            chunks: d.chunks,
            last: d.last,
        }
    }
}

/// Events broadcast to SSE clients for live updates.
#[derive(Debug, Clone)]
pub enum ServerEvent {
    Change { max_retrieval_id: i64 },
    #[allow(dead_code)]
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
