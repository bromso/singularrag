//! Per-run bearer token and Host check (parent spec §11): loopback bind alone does not
//! stop a web page on the same machine, or DNS rebinding, from reaching the API.

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use rand::Rng;
use subtle::ConstantTimeEq;

use super::state::AppState;

pub fn generate_token() -> String {
    let mut buf = [0u8; 32];
    rand::rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

fn reject(status: StatusCode, msg: &str) -> Response {
    (status, Json(serde_json::json!({ "error": msg }))).into_response()
}

fn token_from_query(query: &str) -> Option<String> {
    query
        .split('&')
        .find_map(|kv| kv.strip_prefix("token=").map(|v| v.trim().to_string()))
}

/// `?token=` exists only for `EventSource`, which cannot send headers. Every other route
/// takes the bearer header, so the token never lands in a referrer, a proxy log or the
/// address bar for them. `nest("/api", ..)` strips the prefix before this layer runs, so
/// the path seen here is `/events`; the full form is accepted in case it is ever mounted
/// without the nest.
fn accepts_query_token(path: &str) -> bool {
    path == "/events" || path == "/api/events"
}

fn presented_token(req: &Request<Body>) -> Option<String> {
    if let Some(h) = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        if let Some(t) = h.strip_prefix("Bearer ") {
            return Some(t.trim().to_string());
        }
    }
    if !accepts_query_token(req.uri().path()) {
        return None;
    }
    req.uri().query().and_then(token_from_query)
}

pub async fn require_token(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let Some(t) = presented_token(&req) else {
        return reject(StatusCode::UNAUTHORIZED, "missing token");
    };
    let ok: bool = t.as_bytes().ct_eq(state.token.as_bytes()).into();
    if !ok {
        return reject(StatusCode::UNAUTHORIZED, "invalid token");
    }
    next.run(req).await
}

pub async fn check_host(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let allowed = [
        format!("127.0.0.1:{}", state.port),
        format!("localhost:{}", state.port),
    ];
    if !allowed.iter().any(|a| a == host) {
        return reject(StatusCode::FORBIDDEN, "host not allowed");
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};
    use tower::ServiceExt;

    async fn app() -> (axum::Router, String) {
        let dir = tempfile::tempdir().unwrap();
        singularrag_core::fixture::write_ts_mini(dir.path());
        singularrag_core::store::Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        let (state, _handle) = crate::serve::state::test_state(dir.path(), 4173);
        let token = state.token.to_string();
        std::mem::forget(dir);
        (crate::serve::router(state), token)
    }

    fn req(method: &str, uri: &str, host: &str, auth: Option<&str>) -> Request<Body> {
        let mut b = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::HOST, host);
        if let Some(a) = auth {
            b = b.header(header::AUTHORIZATION, a);
        }
        b.body(Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn token_is_64_hex_chars_and_unique() {
        let a = generate_token();
        let b = generate_token();
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[tokio::test]
    async fn api_requires_bearer_or_query_token() {
        let (app, token) = app().await;
        let r = app
            .clone()
            .oneshot(req("GET", "/api/health", "127.0.0.1:4173", None))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(r.headers().get(header::CACHE_CONTROL).unwrap(), "no-store");
        let r = app
            .clone()
            .oneshot(req(
                "GET",
                "/api/health",
                "127.0.0.1:4173",
                Some("Bearer nope"),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        let r = app
            .clone()
            .oneshot(req(
                "GET",
                "/api/health",
                "127.0.0.1:4173",
                Some(&format!("Bearer {token}")),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(r.headers().get(header::CACHE_CONTROL).unwrap(), "no-store");
        // `?token=` is for EventSource only: it is honoured on /api/events and nowhere else.
        let r = app
            .clone()
            .oneshot(req(
                "GET",
                &format!("/api/health?token={token}"),
                "localhost:4173",
                None,
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        let r = app
            .oneshot(req(
                "GET",
                &format!("/api/events?token={token}"),
                "localhost:4173",
                None,
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
    }

    #[test]
    fn query_token_is_trimmed_and_scoped_to_events() {
        assert_eq!(token_from_query("token= abc "), Some("abc".to_string()));
        assert_eq!(token_from_query("x=1&token=abc"), Some("abc".to_string()));
        assert_eq!(token_from_query("x=1"), None);
        assert!(accepts_query_token("/events"));
        assert!(accepts_query_token("/api/events"));
        assert!(!accepts_query_token("/status"));
        assert!(!accepts_query_token("/map"));
    }

    #[tokio::test]
    async fn wrong_host_is_forbidden_before_auth() {
        let (app, token) = app().await;
        for host in [
            "evil.example:4173",
            "127.0.0.1:9999",
            "127.0.0.1",
            "[::1]:4173",
        ] {
            let r = app
                .clone()
                .oneshot(req(
                    "GET",
                    "/api/health",
                    host,
                    Some(&format!("Bearer {token}")),
                ))
                .await
                .unwrap();
            assert_eq!(r.status(), StatusCode::FORBIDDEN, "{host}");
            assert_eq!(
                r.headers().get(header::CACHE_CONTROL).unwrap(),
                "no-store",
                "{host}"
            );
        }
        // No Authorization header at all: if this came back 401 instead of 403, check_host
        // would not be running before require_token.
        let r = app
            .oneshot(req("GET", "/api/health", "evil.example:4173", None))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn no_cors_headers_ever() {
        let (app, token) = app().await;
        let r = app
            .oneshot(req(
                "OPTIONS",
                "/api/health",
                "127.0.0.1:4173",
                Some(&format!("Bearer {token}")),
            ))
            .await
            .unwrap();
        assert!(r.headers().get("access-control-allow-origin").is_none());
    }
}
