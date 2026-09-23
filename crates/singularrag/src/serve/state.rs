//! Shared state for `singularrag serve`: one read-only store behind a mutex, the
//! freshness snapshot the watcher maintains, and the broadcast channel SSE fans out.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use serde::Serialize;
use singularrag_core::store::Store;
use singularrag_core::workspace::Workspace;
use tokio::sync::broadcast;

#[derive(Debug, Clone, Default, Serialize)]
pub struct Freshness {
    pub stale_count: usize,
    pub lock_timeout: bool,
    pub foreign_indexing: bool,
    /// True while this process is inside a refresh. `foreign_indexing` is the same
    /// condition for *another* process holding the lock (I8).
    pub indexing: bool,
}

/// Events broadcast to SSE clients for live updates.
#[derive(Debug, Clone)]
pub enum ServerEvent {
    Change { max_retrieval_id: i64 },
    Freshness(super::queries::StatusDto),
}

#[derive(Clone)]
pub struct AppState {
    pub root: PathBuf,
    pub ws: Workspace,
    pub port: u16,
    pub token: Arc<str>,
    pub read: Arc<Mutex<Store>>,
    pub freshness: Arc<RwLock<Freshness>>,
    pub events: broadcast::Sender<ServerEvent>,
}

impl AppState {
    pub fn new(root: PathBuf, port: u16) -> anyhow::Result<AppState> {
        let root = root.canonicalize()?;
        let ws = Workspace::open(&root)?;
        let read = Store::open_read_only(&root.join(singularrag_core::engine::DB_FILE))?;
        let (events, _) = broadcast::channel(64);
        Ok(AppState {
            root,
            ws,
            port,
            token: crate::serve::auth::generate_token().into(),
            read: Arc::new(Mutex::new(read)),
            freshness: Arc::new(RwLock::new(Freshness::default())),
            events,
        })
    }

    /// The read store, recovered if a panic elsewhere poisoned the mutex: the SQLite
    /// connection is still usable, and refusing it would turn every route into a 500
    /// and freeze the freshness badge until restart.
    pub fn store(&self) -> std::sync::MutexGuard<'_, Store> {
        self.read.lock().unwrap_or_else(|e| e.into_inner())
    }
}
