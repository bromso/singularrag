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

pub async fn get_map(State(s): State<AppState>) -> Result<Json<MapConfig>, ApiError> {
    Ok(Json(MapConfig::load(&s.root)?))
}

pub async fn put_map(
    State(s): State<AppState>,
    Json(cfg): Json<MapConfig>,
) -> Result<Json<MapConfig>, ApiError> {
    let current = MapConfig::load(&s.root)?;
    if let Err(e) = cfg.validate(&current.deny.extra_patterns) {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            serde_json::json!({ "error": e.message, "field": e.field }),
        ));
    }
    cfg.save_atomic(&s.root)?;
    Ok(Json(MapConfig::load(&s.root)?))
}
