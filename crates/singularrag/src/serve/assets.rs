//! The embedded UI. `ui/dist` is built by Bun and embedded at compile time; nothing is
//! fetched at run time. The shell and assets carry no data, so they need no token.

use axum::extract::Path;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "$CARGO_MANIFEST_DIR/../../ui/dist"]
struct Dist;

pub async fn index() -> Response {
    match Dist::get("index.html") {
        Some(f) => (
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            f.data.into_owned(),
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "ui not built").into_response(),
    }
}

pub async fn asset(Path(path): Path<String>) -> Response {
    let key = format!("assets/{path}");
    match Dist::get(&key) {
        Some(f) => {
            let mime = mime_guess::from_path(&key).first_or_octet_stream();
            (
                [
                    (
                        header::CONTENT_TYPE,
                        HeaderValue::from_str(mime.as_ref())
                            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
                    ),
                    (
                        header::CACHE_CONTROL,
                        HeaderValue::from_static("public, max-age=31536000, immutable"),
                    ),
                ],
                f.data.into_owned(),
            )
                .into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
