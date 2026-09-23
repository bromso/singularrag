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
    Ok(f(&state.store())?)
}

pub async fn status(State(s): State<AppState>) -> Result<Json<queries::StatusDto>, ApiError> {
    let f = s.freshness.read().map(|g| g.clone()).unwrap_or_default();
    Ok(Json(locked(&s, |store| queries::status(store, &f, &s.ws))?))
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

pub async fn entities(State(s): State<AppState>) -> Result<Json<queries::EntitiesDto>, ApiError> {
    Ok(Json(locked(&s, queries::entities)?))
}

/// `MapConfig` plus the token a client must hand back to write: `map.toml`'s mtime in
/// milliseconds, 0 when the file does not exist. Kept here rather than in core so
/// `MapConfig` stays the on-disk shape (spec §3).
///
/// Deliberately not `#[serde(flatten)]` over `MapConfig`: that struct's list fields skip
/// serialization when empty (needed so `save_atomic`'s `toml_edit` pretty-printer doesn't choke
/// on an empty array of tables — see the comment on `MapConfig`), which would leak into this
/// JSON response as a missing `pin`/`exclude`/`note`/`boundary` key instead of `[]`. The HTTP
/// shape and the on-disk shape have different constraints, so they get independent `Serialize`
/// impls even though the data is the same.
pub struct MapDoc {
    pub config: MapConfig,
    pub version: i64,
}

impl Serialize for MapDoc {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(6))?;
        map.serialize_entry("pin", &self.config.pin)?;
        map.serialize_entry("exclude", &self.config.exclude)?;
        map.serialize_entry("note", &self.config.note)?;
        map.serialize_entry("boundary", &self.config.boundary)?;
        map.serialize_entry("deny", &self.config.deny)?;
        map.serialize_entry("version", &self.version)?;
        map.end()
    }
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

pub async fn graph(State(s): State<AppState>) -> Result<Json<queries::GraphDto>, ApiError> {
    let config = MapConfig::load(&s.root)?;
    Ok(Json(locked(&s, |store| queries::graph(store, &config))?))
}

#[derive(Deserialize)]
pub struct BlastQuery {
    pub path: Option<String>,
    pub symbol: Option<String>,
}

#[derive(Serialize)]
pub struct BlastRoot {
    pub path: String,
    pub symbol: String,
}

#[derive(Serialize)]
pub struct BlastDto {
    pub root: BlastRoot,
    pub files: Vec<singularrag_core::blast::BlastFile>,
    pub truncated: Option<&'static str>,
}

#[derive(Deserialize)]
pub struct QueryBody {
    pub query: String,
    pub budget: Option<usize>,
}

#[derive(Serialize)]
pub struct QueryDto {
    pub retrieval_id: i64,
    pub served: usize,
    pub cut: usize,
}

/// Runs `repo_map` through the process's own actor, so the retrieval is recorded under
/// the actor's existing session key (`serve`, labelled `UI` — spec §6) rather than a
/// second key.
pub async fn query(
    State(s): State<AppState>,
    Json(b): Json<QueryBody>,
) -> Result<Json<QueryDto>, ApiError> {
    let q = b.query.trim();
    if q.is_empty() || q.chars().count() > 2000 {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            serde_json::json!({ "error": "query must be 1 to 2000 characters", "field": "query" }),
        ));
    }
    let req = singularrag_core::engine::MapRequest {
        query: Some(q.to_string()),
        focus_files: vec![],
        budget_tokens: singularrag_core::map::clamp_budget(
            b.budget.unwrap_or(singularrag_core::map::DEFAULT_BUDGET),
        ),
        ..Default::default()
    };
    match s.handle.map(req).await {
        Ok(r) => Ok(Json(QueryDto {
            retrieval_id: r.retrieval_id,
            served: r.served,
            cut: r.total.saturating_sub(r.served),
        })),
        Err(e) => Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            serde_json::json!({ "error": e }),
        )),
    }
}

pub async fn blast(
    State(s): State<AppState>,
    Query(q): Query<BlastQuery>,
) -> Result<Json<BlastDto>, ApiError> {
    let (Some(path), Some(symbol)) = (q.path, q.symbol) else {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            serde_json::json!({ "error": "path and symbol are required" }),
        ));
    };
    let config = MapConfig::load(&s.root)?;
    let b = locked(&s, |store| {
        singularrag_core::blast::blast_radius(
            store,
            &config,
            &path,
            &symbol,
            singularrag_core::blast::MAX_DEPTH,
            singularrag_core::blast::MAX_FILES,
        )
    })?;
    match b {
        None => Err(ApiError(
            StatusCode::NOT_FOUND,
            serde_json::json!({ "error": "symbol not found" }),
        )),
        Some(b) => Ok(Json(BlastDto {
            root: BlastRoot {
                path: b.root_path,
                symbol: b.root_symbol,
            },
            files: b.files,
            truncated: b.truncated,
        })),
    }
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn entities_lists_entities_relations_and_mentions_from_the_prose_fixture() {
        let dir = tempfile::tempdir().unwrap();
        singularrag_core::fixture::write_prose(dir.path());
        let mut e = singularrag_core::engine::Engine::open(dir.path(), "t").unwrap();
        e.refresh(std::time::Duration::from_secs(120)).unwrap();
        drop(e);
        let store = singularrag_core::store::Store::open(&dir.path().join(".singularrag/index.db"))
            .unwrap();
        let json = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../singularrag-core/fixtures/prose/extraction.json"
        ))
        .unwrap();
        singularrag_core::knowledge::load_extraction_json(&store, None, &json).unwrap();
        drop(store);

        let (state, handle) = crate::serve::state::test_state(dir.path(), 1);
        let token = state.token.to_string();
        let app = crate::serve::router(state);
        let res = tower::ServiceExt::oneshot(
            app,
            axum::http::Request::builder()
                .uri("/api/entities")
                .header("host", "127.0.0.1:1")
                .header("authorization", format!("Bearer {token}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(res.status(), 200);
        let v: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(res.into_body(), 1 << 20)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(v["truncated"], false, "{v}");

        let entities = v["entities"].as_array().unwrap();
        assert!(entities.len() >= 12, "{v}");
        assert!(entities
            .windows(2)
            .all(
                |w| w[0]["mentions"].as_i64().unwrap() > w[1]["mentions"].as_i64().unwrap()
                    || (w[0]["mentions"].as_i64().unwrap() == w[1]["mentions"].as_i64().unwrap()
                        && w[0]["name"].as_str().unwrap() <= w[1]["name"].as_str().unwrap())
            ));
        for f in ["id", "name", "type", "description", "mentions"] {
            assert!(entities[0].get(f).is_some(), "entity missing {f}: {v}");
        }

        let relations = v["relations"].as_array().unwrap();
        assert!(relations.len() >= 10, "{v}");
        for f in [
            "id",
            "src",
            "dst",
            "description",
            "symbol_id",
            "path",
            "name",
        ] {
            assert!(relations[0].get(f).is_some(), "relation missing {f}: {v}");
        }
        assert!(relations[0]["path"].as_str().unwrap().starts_with("docs/"));

        let mentions = v["mentions"].as_array().unwrap();
        assert!(!mentions.is_empty(), "{v}");
        for f in ["entity_id", "symbol_id", "path", "name"] {
            assert!(mentions[0].get(f).is_some(), "mention missing {f}: {v}");
        }
        assert!(mentions[0]["path"].as_str().unwrap().starts_with("docs/"));

        handle.shutdown();
    }

    #[tokio::test]
    async fn query_records_a_ui_retrieval_and_validates_input() {
        let dir = tempfile::tempdir().unwrap();
        singularrag_core::fixture::write_docs_mini(dir.path());
        let (handle, _join, _died) = crate::actor::spawn(crate::actor::EngineConfig {
            root: dir.path().to_path_buf(),
            session_key: crate::actor::SessionKey::Fixed("serve".into()),
            refresh_budget: std::time::Duration::from_secs(5),
            models_url: None,
            knowledge_idle: None,
        });
        handle.refresh().await.unwrap();
        let state = crate::serve::state::AppState::new(dir.path().to_path_buf(), 1, handle.clone())
            .unwrap();
        let token = state.token.to_string();
        let app = crate::serve::router(state);
        let post = |body: &str| {
            axum::http::Request::builder()
                .method("POST")
                .uri("/api/query")
                .header("host", "127.0.0.1:1")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body.to_string()))
                .unwrap()
        };
        let res = tower::ServiceExt::oneshot(
            app.clone(),
            post(r#"{"query":"STALE header","budget":2048}"#),
        )
        .await
        .unwrap();
        assert_eq!(res.status(), 200);
        let v: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(res.into_body(), 1 << 20)
                .await
                .unwrap(),
        )
        .unwrap();
        assert!(
            v["retrieval_id"].as_i64().unwrap() >= 1 && v["served"].as_u64().unwrap() >= 1,
            "{v}"
        );
        let res = tower::ServiceExt::oneshot(app.clone(), post(r#"{"query":""}"#))
            .await
            .unwrap();
        assert_eq!(res.status(), 422);
        let list = tower::ServiceExt::oneshot(
            app,
            axum::http::Request::builder()
                .uri("/api/retrievals")
                .header("host", "127.0.0.1:1")
                .header("authorization", format!("Bearer {token}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        let v: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(list.into_body(), 1 << 20)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(v[0]["session_label"], "UI");
        assert_eq!(v[0]["query"], "STALE header");
        handle.shutdown();
    }
}
