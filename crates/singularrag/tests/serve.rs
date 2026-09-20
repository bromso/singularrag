//! Drives the real binary: spawn `serve --no-open --port 0`, parse the URL line, hit the API.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use futures_util::StreamExt;
use singularrag_core::fixture::write_ts_mini;

struct Server {
    child: Child,
    url: String,
    token: String,
    port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn spawn(repo: &std::path::Path) -> Server {
    let mut child = Command::new(env!("CARGO_BIN_EXE_singularrag"))
        .args([
            "serve",
            "--no-open",
            "--port",
            "0",
            "--repo",
            repo.to_str().unwrap(),
        ])
        .env("RUST_LOG", "warn")
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let line = line.trim().to_string();
    let (base, token) = line.split_once("/#token=").expect("url line");
    let port: u16 = base.rsplit(':').next().unwrap().parse().unwrap();
    Server {
        child,
        url: base.to_string(),
        token: token.to_string(),
        port,
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
}

async fn get(s: &Server, path: &str) -> reqwest::Response {
    client()
        .get(format!("{}/api{path}", s.url))
        .bearer_auth(&s.token)
        .send()
        .await
        .unwrap()
}

fn cli_query(repo: &std::path::Path, q: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_singularrag"))
        .args([
            "query",
            q,
            "--budget",
            "4096",
            "--repo",
            repo.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap()
}

#[tokio::test]
async fn auth_and_host_controls() {
    let dir = tempfile::tempdir().unwrap();
    write_ts_mini(dir.path());
    let s = spawn(dir.path());
    let r = client()
        .get(format!("{}/api/status", s.url))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    let r = client()
        .get(format!("http://localhost:{}/api/status", s.port))
        .bearer_auth(&s.token)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "localhost:<port> is an allowed host");
    let r = client()
        .get(format!("{}/api/status", s.url))
        .header("Host", "evil.example")
        .bearer_auth(&s.token)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    assert!(r.headers().get("access-control-allow-origin").is_none());
    let r = get(&s, "/status").await;
    assert_eq!(r.headers().get("cache-control").unwrap(), "no-store");
}

/// I6: an unknown `/api` path must be answered inside the api router's layers, not fall
/// out to the outer router's asset fallback (which has no token check and no no-store).
#[tokio::test]
async fn an_unknown_api_path_is_guarded_and_not_cached() {
    let dir = tempfile::tempdir().unwrap();
    write_ts_mini(dir.path());
    let s = spawn(dir.path());
    let r = client()
        .get(format!("{}/api/nope", s.url))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    assert_eq!(r.headers().get("cache-control").unwrap(), "no-store");
    let r = get(&s, "/nope").await;
    assert_eq!(r.status(), 404);
    assert_eq!(r.headers().get("cache-control").unwrap(), "no-store");
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!(body["error"], "not found");
}

/// The query-string token exists for `EventSource`, which cannot send headers; every
/// other route takes the bearer header only.
#[tokio::test]
async fn a_query_token_works_only_on_events() {
    let dir = tempfile::tempdir().unwrap();
    write_ts_mini(dir.path());
    let s = spawn(dir.path());
    let r = client()
        .get(format!("{}/api/status?token={}", s.url, s.token))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    let r = client()
        .get(format!("{}/api/events?token={}", s.url, s.token))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn shell_and_assets_are_served_without_a_token() {
    let dir = tempfile::tempdir().unwrap();
    write_ts_mini(dir.path());
    let s = spawn(dir.path());
    let r = client().get(format!("{}/", s.url)).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert!(r
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("text/html"));
    let r_text = r.text().await.unwrap();
    assert!(r_text.contains("singularrag"));
    // Verify the HTML references a hashed asset under /assets/
    let src = r_text
        .split("src=\"")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .expect("a script tag");
    assert!(
        src.starts_with("/assets/") || src.starts_with("./assets/") || src.starts_with("assets/"),
        "{src}"
    );
    let asset_path = src.trim_start_matches('.').trim_start_matches('/');
    let r = client()
        .get(format!("{}/{asset_path}", s.url))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert!(r
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("javascript"));
    assert_eq!(
        r.headers().get("cache-control").unwrap(),
        "public, max-age=31536000, immutable"
    );
    // Verify a non-existent asset returns 404
    let r = client()
        .get(format!("{}/assets/nope.js", s.url))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
}

/// `/assets/{*path}` is an exact lookup into the embedded bundle, so a traversal attempt
/// cannot reach anything outside it. The `..` is percent-encoded because the client would
/// otherwise normalise it away before the request left the process.
#[tokio::test]
async fn assets_traversal_is_404() {
    let dir = tempfile::tempdir().unwrap();
    write_ts_mini(dir.path());
    let s = spawn(dir.path());
    for path in [
        "/assets/%2e%2e/index.html",
        "/assets/..%2f..%2fetc%2fpasswd",
        "/assets/%2e%2e%2f%2e%2e%2fCargo.toml",
    ] {
        let r = client()
            .get(format!("{}{path}", s.url))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 404, "{path}");
    }
}

/// A foreign process holding the indexer lock is reported, never an error (spec §2).
#[tokio::test]
async fn a_foreign_lock_shows_up_as_foreign_indexing() {
    use singularrag_core::store::{lock, Store};

    let dir = tempfile::tempdir().unwrap();
    write_ts_mini(dir.path());
    let s = spawn(dir.path());
    tokio::time::sleep(Duration::from_millis(500)).await;

    let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
    // A pid no live process can have. `std::process::id() + 1` would not do: pids are
    // handed out in order, so the server this test just spawned is very likely to hold
    // it, and the lock lets its own pid take the row over.
    let foreign = u32::MAX - 7;
    assert_ne!(foreign, s.child.id());
    assert!(lock::try_acquire(&store, foreign, singularrag_core::time::now_ms()).unwrap());

    // A file change makes the watcher try to refresh; it cannot take the lock.
    std::fs::write(
        dir.path().join("src/util/log.ts"),
        "export function logWhileLocked(): void {}\n",
    )
    .unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        // Keep the lock alive: a heartbeat older than LOCK_STALE_MS may be taken over.
        lock::heartbeat(&store, foreign, singularrag_core::time::now_ms()).unwrap();
        let now: serde_json::Value = get(&s, "/status").await.json().await.unwrap();
        if now["foreign_indexing"] == true {
            assert_eq!(now["lock_timeout"], true);
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "foreign_indexing never went true: {now}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    lock::release(&store, foreign).unwrap();
}

#[tokio::test]
async fn routes_have_the_documented_shapes() {
    let dir = tempfile::tempdir().unwrap();
    write_ts_mini(dir.path());
    cli_query(dir.path(), "session");
    let s = spawn(dir.path());
    let status: serde_json::Value = get(&s, "/status").await.json().await.unwrap();
    for k in [
        "index_version",
        "git_head",
        "indexed_at_ms",
        "stale_count",
        "lock_timeout",
        "foreign_indexing",
        "indexing",
        "files",
        "drain",
    ] {
        assert!(status.get(k).is_some(), "status missing {k}: {status}");
    }
    let rs: serde_json::Value = get(&s, "/retrievals?limit=10").await.json().await.unwrap();
    let first = &rs.as_array().unwrap()[0];
    assert_eq!(first["session_label"], "CLI");
    assert_eq!(first["tool"], "repo_map");
    let id = first["id"].as_i64().unwrap();
    let d: serde_json::Value = get(&s, &format!("/retrievals/{id}"))
        .await
        .json()
        .await
        .unwrap();
    assert!(d["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["served"] == true));
    assert!(d["items"][0]["reasons"]["referenced_by"].is_array());
    let tree: serde_json::Value = get(&s, "/tree").await.json().await.unwrap();
    assert!(tree
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["path"] == "src/auth/session.ts"));
    let sk: serde_json::Value = get(&s, "/skipped").await.json().await.unwrap();
    assert!(sk.as_array().unwrap().iter().any(|f| f["path"] == ".env"));
    let map: serde_json::Value = get(&s, "/map").await.json().await.unwrap();
    assert!(map["pin"].is_array());
    assert_eq!(get(&s, "/retrievals/999999").await.status(), 404);
}

#[tokio::test]
async fn a_cli_query_produces_a_change_event() {
    let dir = tempfile::tempdir().unwrap();
    write_ts_mini(dir.path());
    let s = spawn(dir.path());
    let resp = client()
        .get(format!("{}/api/events?token={}", s.url, s.token))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let mut stream = resp.bytes_stream();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let repo = dir.path().to_path_buf();
    std::thread::spawn(move || cli_query(&repo, "session"));
    let mut buf = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let chunk = tokio::time::timeout_at(deadline, stream.next())
            .await
            .expect("change event within 5 s")
            .unwrap()
            .unwrap();
        buf.push_str(std::str::from_utf8(&chunk).unwrap());
        if buf.contains("event: change") && buf.contains("max_retrieval_id") {
            break;
        }
    }
}

#[tokio::test]
async fn exclude_via_put_map_changes_the_next_retrieval() {
    let dir = tempfile::tempdir().unwrap();
    write_ts_mini(dir.path());
    let before = cli_query(dir.path(), "");
    assert!(before.contains("src/util/log.ts:"), "{before}");
    let s = spawn(dir.path());
    let body = serde_json::json!({ "pin": [], "exclude": [{ "path": "src/util/" }], "note": [], "boundary": [], "deny": { "extra_patterns": [] } });
    let r = client()
        .put(format!("{}/api/map", s.url))
        .bearer_auth(&s.token)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    let written = std::fs::read_to_string(dir.path().join(".singularrag/map.toml")).unwrap();
    assert!(written.starts_with("# Managed by singularrag serve"));
    let after = cli_query(dir.path(), "");
    assert!(!after.contains("src/util/log.ts:"), "{after}");
    let bad = serde_json::json!({ "pin": [{ "path": "../x" }], "exclude": [], "note": [], "boundary": [], "deny": { "extra_patterns": [] } });
    let r = client()
        .put(format!("{}/api/map", s.url))
        .bearer_auth(&s.token)
        .json(&bad)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 422);
    let e: serde_json::Value = r.json().await.unwrap();
    assert_eq!(e["field"], "pin[0].path");
}

/// C1: `PUT /api/map` is compare-and-swap. A hand edit to `map.toml` between a GET and a
/// PUT must not be silently overwritten: the stale `expected_version` is refused with 409
/// and the caller is handed the current document to rebuild its edit on.
#[tokio::test]
async fn put_map_with_stale_version_is_409() {
    let dir = tempfile::tempdir().unwrap();
    write_ts_mini(dir.path());
    let s = spawn(dir.path());
    let doc: serde_json::Value = get(&s, "/map").await.json().await.unwrap();
    let stale_version = doc["version"].as_i64().expect("version in GET /api/map");

    // A hand edit behind the server's back.
    std::fs::create_dir_all(dir.path().join(".singularrag")).unwrap();
    std::fs::write(
        dir.path().join(".singularrag/map.toml"),
        "[[pin]]\npath = \"src/auth/session.ts\"\n",
    )
    .unwrap();

    let put = |body: serde_json::Value| {
        let url = format!("{}/api/map", s.url);
        let token = s.token.clone();
        async move {
            client()
                .put(url)
                .bearer_auth(token)
                .json(&body)
                .send()
                .await
                .unwrap()
        }
    };
    let edit = |version: i64| {
        serde_json::json!({
            "pin": [{ "path": "src/util/log.ts" }],
            "exclude": [], "note": [], "boundary": [],
            "deny": { "extra_patterns": [] },
            "expected_version": version,
        })
    };

    let r = put(edit(stale_version)).await;
    assert_eq!(r.status(), 409);
    let e: serde_json::Value = r.json().await.unwrap();
    assert_eq!(e["field"], "expected_version");
    assert_eq!(e["error"], "map.toml changed on disk");
    assert_eq!(e["current"]["pin"][0]["path"], "src/auth/session.ts");
    let fresh_version = e["current"]["version"].as_i64().unwrap();
    assert_ne!(fresh_version, stale_version);
    // The refused write left the hand edit alone.
    let on_disk = std::fs::read_to_string(dir.path().join(".singularrag/map.toml")).unwrap();
    assert!(on_disk.contains("src/auth/session.ts"), "{on_disk}");

    // Retrying with the version the 409 handed back succeeds.
    let r = put(edit(fresh_version)).await;
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    let saved: serde_json::Value = get(&s, "/map").await.json().await.unwrap();
    assert_eq!(saved["pin"][0]["path"], "src/util/log.ts");
    assert_ne!(saved["version"].as_i64().unwrap(), fresh_version);
}

#[tokio::test]
async fn touching_a_file_changes_freshness_within_two_seconds() {
    let dir = tempfile::tempdir().unwrap();
    write_ts_mini(dir.path());
    let s = spawn(dir.path());
    tokio::time::sleep(Duration::from_millis(500)).await;
    let before: serde_json::Value = get(&s, "/status").await.json().await.unwrap();
    std::fs::write(
        dir.path().join("src/util/log.ts"),
        "export function logAgain(): void {}\n",
    )
    .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let now: serde_json::Value = get(&s, "/status").await.json().await.unwrap();
        if now["indexed_at_ms"] != before["indexed_at_ms"] {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "freshness did not change"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let tree: serde_json::Value = get(&s, "/tree").await.json().await.unwrap();
    assert!(tree.to_string().contains("logAgain"));
}

#[tokio::test]
async fn graph_and_blast_are_served_behind_the_token() {
    let dir = tempfile::tempdir().unwrap();
    write_ts_mini(dir.path());
    let s = spawn(dir.path());
    let r = client()
        .get(format!("{}/api/graph", s.url))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    let g: serde_json::Value = get(&s, "/graph").await.json().await.unwrap();
    let tree: serde_json::Value = get(&s, "/tree").await.json().await.unwrap();
    assert_eq!(
        g["nodes"].as_array().unwrap().len(),
        tree.as_array().unwrap().len()
    );
    assert!(g["edges"].as_array().unwrap().len() >= 2);
    let b: serde_json::Value = get(&s, "/blast?path=src/auth/session.ts&symbol=createSession")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(b["root"]["symbol"], "createSession");
    assert_eq!(b["files"][0]["depth"], 1);
    assert!(b["truncated"].is_null());
    let r = get(&s, "/blast?path=src/auth/session.ts&symbol=nope").await;
    assert_eq!(r.status(), 404);
    assert_eq!(r.headers().get("cache-control").unwrap(), "no-store");
    let r = get(&s, "/blast?path=src/auth/session.ts").await;
    assert_eq!(r.status(), 400);
}

#[tokio::test]
async fn put_map_rejects_duplicate_boundary_names() {
    let dir = tempfile::tempdir().unwrap();
    write_ts_mini(dir.path());
    let s = spawn(dir.path());
    let body = serde_json::json!({ "pin": [], "exclude": [], "note": [], "boundary": [ { "name": "x", "paths": ["src/a.ts"] }, { "name": "x", "paths": ["src/b.ts"] } ], "deny": { "extra_patterns": [] } });
    let r = client()
        .put(format!("{}/api/map", s.url))
        .bearer_auth(&s.token)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 422);
    let e: serde_json::Value = r.json().await.unwrap();
    assert_eq!(e["field"], "boundary[1].name");
}
