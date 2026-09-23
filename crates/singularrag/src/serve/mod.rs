//! `singularrag serve`: the loop UI. Own actor + watcher for writes, a read-only store for
//! reads, SSE for liveness, an embedded React app for the page.

pub mod assets;
pub mod auth;
pub mod events;
pub mod queries;
pub mod routes;
pub mod state;
pub mod watcher;

use std::path::PathBuf;

use axum::http::{header, HeaderValue};
use axum::routing::{get, post};
use axum::{middleware, Json, Router};
use tower_http::set_header::SetResponseHeaderLayer;

use state::AppState;

pub fn router(state: AppState) -> Router {
    // axum nests layers outward (the last `.layer` added is outermost), so this list reads
    // innermost-first: `require_token` wraps the handler, `check_host` wraps that (so the
    // host check runs before the token check on the way in), and the cache-control layer
    // wraps everything (so it stamps the header on 401/403 rejections too, not just 200s).
    let api = Router::new()
        .route(
            "/health",
            get(|| async { Json(serde_json::json!({ "ok": true })) }),
        )
        .route("/status", get(routes::status))
        .route("/retrievals", get(routes::retrievals))
        .route("/retrievals/{id}", get(routes::retrieval))
        .route("/tree", get(routes::tree))
        .route("/skipped", get(routes::skipped))
        .route("/entities", get(routes::entities))
        .route("/map", get(routes::get_map).put(routes::put_map))
        .route("/graph", get(routes::graph))
        .route("/blast", get(routes::blast))
        .route("/query", post(routes::query))
        .route("/events", get(events::sse))
        // Before the layers, so an unknown /api path is answered *inside* them: it gets
        // the host and token checks and the no-store header, instead of falling out to
        // the outer router's asset fallback (I6).
        .fallback(|| async {
            (
                axum::http::StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "not found" })),
            )
        })
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_token,
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::check_host,
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ));
    Router::new()
        .route("/", get(assets::index))
        .route("/assets/{*path}", get(assets::asset))
        .nest("/api", api)
        .with_state(state)
}

/// Bind, spawn the actor and run its startup refresh (creating the index if this is a
/// fresh repo), open the read-only store, print the one stdout line, open the browser,
/// serve until Ctrl-C or the actor dies.
///
/// The actor is spawned and its startup refresh run *before* `AppState::new`, not after:
/// `AppState::new` opens `.singularrag/index.db` read-only and fails if it does not exist
/// yet, so on a fresh repo the index must be created first. `watcher::refresh_once` takes
/// `&AppState` (it updates `freshness` and broadcasts), which does not exist yet at that
/// point, so the startup refresh calls `handle.refresh()` directly and `watcher::apply`s
/// the resulting stats once `AppState` exists.
pub fn run(root: PathBuf, port: u16, open_browser: bool) -> anyhow::Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    // Spawned (and joined below) outside `block_on`, as `mcp::run` does: `actor::spawn`
    // is synchronous (an OS thread, not a task), and joining it is a blocking call that
    // has no business running on a tokio worker thread.
    let (handle, join, mut died) = crate::actor::spawn(crate::actor::EngineConfig {
        root: root.clone(),
        session_key: crate::actor::SessionKey::Fixed("serve".into()),
        refresh_budget: singularrag_core::engine::REFRESH_BUDGET,
        models_url: None,
        knowledge_idle: None,
    });

    let result = rt.block_on(async {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
        let port = listener.local_addr()?.port();

        let startup_stats = handle
            .refresh()
            .await
            .map_err(|e| anyhow::anyhow!("startup refresh failed: {e}"))?;

        let state = AppState::new(root, port, handle.clone())?;
        watcher::apply(&state, startup_stats);

        let url = format!("http://127.0.0.1:{port}/#token={}", state.token);
        println!("{url}");
        if open_browser {
            if let Err(e) = open::that(&url) {
                tracing::warn!("could not open a browser: {e}");
            }
        }
        let _poller = events::spawn_poller(state.clone());
        let _debouncer = watcher::start(state.clone(), handle.clone())?;

        // Spec §2: the one fatal path is the actor thread dying (mirrors `mcp::run`).
        tokio::select! {
            served = axum::serve(listener, router(state)) => {
                served?;
            }
            _ = &mut died => {
                tracing::error!("engine thread exited unexpectedly");
                std::process::exit(2);
            }
        }
        Ok::<(), anyhow::Error>(())
    });

    handle.shutdown();
    if join.join().is_err() {
        tracing::error!("engine thread panicked");
        std::process::exit(2);
    }
    result
}
