//! JSON handlers. Each locks the read store for the duration of one query.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
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
        let status = if busy {
            StatusCode::SERVICE_UNAVAILABLE
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        };
        ApiError(status, serde_json::json!({ "error": e.to_string() }))
    }
}

fn locked<T>(
    state: &AppState,
    f: impl FnOnce(&singularrag_core::store::Store) -> singularrag_core::Result<T>,
) -> Result<T, ApiError> {
    let guard = state.read.lock().map_err(|_| {
        ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({ "error": "read store poisoned" }),
        )
    })?;
    Ok(f(&guard)?)
}

pub async fn status(State(s): State<AppState>) -> Result<Json<queries::StatusDto>, ApiError> {
    let f = s.freshness.read().map(|g| g.clone()).unwrap_or_default();
    Ok(Json(locked(&s, |store| queries::status(store, &f))?))
}

#[derive(Deserialize)]
pub struct Page {
    pub limit: Option<usize>,
    pub before: Option<i64>,
}

pub async fn retrievals(
    State(s): State<AppState>,
    Query(p): Query<Page>,
) -> Result<Json<Vec<queries::RetrievalSummary>>, ApiError> {
    let limit = p.limit.unwrap_or(50).clamp(1, 200);
    Ok(Json(locked(&s, |store| {
        queries::retrievals(store, limit, p.before)
    })?))
}

pub async fn retrieval(
    State(s): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<queries::RetrievalDetail>, ApiError> {
    match locked(&s, |store| queries::retrieval(store, id))? {
        Some(d) => Ok(Json(d)),
        None => Err(ApiError(
            StatusCode::NOT_FOUND,
            serde_json::json!({ "error": "no such retrieval" }),
        )),
    }
}

pub async fn tree(State(s): State<AppState>) -> Result<Json<Vec<queries::TreeFile>>, ApiError> {
    Ok(Json(locked(&s, queries::tree)?))
}

pub async fn skipped(
    State(s): State<AppState>,
) -> Result<Json<Vec<queries::SkippedFile>>, ApiError> {
    Ok(Json(locked(&s, queries::skipped)?))
}

/// `MapConfig` plus the token a client must hand back to write: `map.toml`'s mtime in
/// milliseconds, 0 when the file does not exist. Kept here rather than in core so
/// `MapConfig` stays the on-disk shape (spec §3).
#[derive(Serialize)]
pub struct MapDoc {
    #[serde(flatten)]
    pub config: MapConfig,
    pub version: i64,
}

/// A `PUT /api/map` body: the config, plus an optional `expected_version` that turns the
/// write into a compare-and-swap. Absent means "write regardless" (the pre-C1 behaviour,
/// kept so `curl` stays usable).
#[derive(Deserialize)]
pub struct MapPut {
    #[serde(flatten)]
    pub config: MapConfig,
    #[serde(default)]
    pub expected_version: Option<i64>,
}

fn map_version(root: &std::path::Path) -> i64 {
    std::fs::metadata(root.join(singularrag_core::config::MAP_FILE))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn map_doc(root: &std::path::Path) -> Result<MapDoc, ApiError> {
    // Read the version first: if a writer lands between the two reads we would rather
    // report a version older than the config (the next CAS fails and the client reloads)
    // than newer (which would let a stale write through).
    let version = map_version(root);
    Ok(MapDoc {
        config: MapConfig::load(root)?,
        version,
    })
}

pub async fn get_map(State(s): State<AppState>) -> Result<Json<MapDoc>, ApiError> {
    Ok(Json(map_doc(&s.root)?))
}

pub async fn put_map(
    State(s): State<AppState>,
    Json(body): Json<MapPut>,
) -> Result<Json<MapDoc>, ApiError> {
    let current = map_doc(&s.root)?;
    if body.expected_version.is_some_and(|v| v != current.version) {
        return Err(ApiError(
            StatusCode::CONFLICT,
            serde_json::json!({
                "error": "map.toml changed on disk",
                "field": "expected_version",
                "current": current,
            }),
        ));
    }
    if let Err(e) = body.config.validate(&current.config.deny.extra_patterns) {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            serde_json::json!({ "error": e.message, "field": e.field }),
        ));
    }
    body.config.save_atomic(&s.root)?;
    Ok(Json(map_doc(&s.root)?))
}
