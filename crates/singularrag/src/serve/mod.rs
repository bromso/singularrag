//! `singularrag serve`: the loop UI. Own actor + watcher for writes, a read-only store for
//! reads, SSE for liveness, an embedded React app for the page.

pub mod auth;
pub mod events;
pub mod queries;
pub mod routes;
pub mod state;

use std::path::PathBuf;

use axum::http::{header, HeaderValue};
use axum::routing::get;
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
        .route("/map", get(routes::get_map).put(routes::put_map))
        .route("/events", get(events::sse))
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
    Router::new().nest("/api", api).with_state(state)
}

/// Bind, print the one stdout line, open the browser, serve until Ctrl-C.
pub fn run(root: PathBuf, port: u16, open_browser: bool) -> anyhow::Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
        let port = listener.local_addr()?.port();
        let state = AppState::new(root, port)?;
        let url = format!("http://127.0.0.1:{port}/#token={}", state.token);
        println!("{url}");
        if open_browser {
            if let Err(e) = open::that(&url) {
                tracing::warn!("could not open a browser: {e}");
            }
        }
        let _poller = events::spawn_poller(state.clone());
        axum::serve(listener, router(state)).await?;
        Ok::<(), anyhow::Error>(())
    })
}
