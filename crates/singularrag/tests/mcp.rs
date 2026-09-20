//! Drives the real binary over stdio with an rmcp client.

use std::path::Path;

use rmcp::model::{CallToolRequestParams, ClientCapabilities, ClientConfig, Implementation};
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use rmcp::{object, ClientHandler, ServiceExt};
use singularrag_core::fixture::write_ts_mini;
use singularrag_core::store::Store;
use tokio::process::Command;

const REPO_MAP_DESCRIPTION: &str = "Token-budgeted map of the symbols most relevant to a task. Call this before reading files. `query` is a question or identifiers; `focus_files` are repo-relative paths you already know matter; `budget_tokens` defaults to 1024, max 8192. Returns paths, line numbers and signatures only, never bodies. The first line says how fresh the index is; if it says STALE, call again after a moment.";
const FIND_SYMBOL_DESCRIPTION: &str = "Look up a symbol by name: exact, prefix, or split words (`create session` finds `createSession`). Returns the definition's path, line and signature and which files reference it. Optional `kind` filter: function, class, method, type, const, module. `limit` defaults to 10, max 50.";

#[derive(Clone)]
struct TestClient;

impl ClientHandler for TestClient {
    fn get_info(&self) -> ClientConfig {
        ClientConfig::new(
            ClientCapabilities::default(),
            Implementation::new("Singularrag Test", "0.0.0"),
        )
    }
}

async fn connect(
    repo: &Path,
    extra: &[&str],
) -> rmcp::service::RunningService<rmcp::RoleClient, TestClient> {
    let bin = env!("CARGO_BIN_EXE_singularrag");
    let repo = repo.to_path_buf();
    let extra: Vec<String> = extra.iter().map(|s| s.to_string()).collect();
    let transport = TokioChildProcess::new(Command::new(bin).configure(move |c| {
        c.arg("mcp").arg("--repo").arg(&repo);
        for e in &extra {
            c.arg(e);
        }
        c.env("RUST_LOG", "warn");
    }))
    .expect("spawn singularrag mcp");
    TestClient.serve(transport).await.expect("initialize")
}

fn text_of(r: &rmcp::model::CallToolResult) -> String {
    r.content
        .first()
        .and_then(|c| c.as_text())
        .map(|t| t.text.clone())
        .unwrap_or_default()
}

#[tokio::test]
async fn lists_exactly_the_two_tools_with_spec_descriptions() {
    let dir = tempfile::tempdir().unwrap();
    write_ts_mini(dir.path());
    let client = connect(dir.path(), &[]).await;
    let info = client.peer_info().expect("server info");
    let server_info = info.server_info.as_ref().expect("server_info present");
    assert_eq!(server_info.name, "singularrag");
    assert!(info
        .instructions
        .as_deref()
        .unwrap_or("")
        .starts_with("singularrag gives you a ranked map"));
    let mut tools = client.list_all_tools().await.unwrap();
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(tools.len(), 2);
    assert_eq!(tools[0].name, "find_symbol");
    assert_eq!(
        tools[0].description.as_deref(),
        Some(FIND_SYMBOL_DESCRIPTION)
    );
    assert_eq!(tools[1].name, "repo_map");
    assert_eq!(tools[1].description.as_deref(), Some(REPO_MAP_DESCRIPTION));
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn repo_map_and_find_symbol_match_the_cli_and_record_provenance() {
    let dir = tempfile::tempdir().unwrap();
    write_ts_mini(dir.path());
    let client = connect(dir.path(), &[]).await;

    let map = client
        .call_tool(
            CallToolRequestParams::new("repo_map")
                .with_arguments(object!({ "query": "session", "budget_tokens": 512 })),
        )
        .await
        .unwrap();
    assert_ne!(map.is_error, Some(true));
    let map_text = text_of(&map);
    assert!(map_text.starts_with("# singularrag · index "), "{map_text}");
    assert!(map_text.contains("src/auth/session.ts:\n"));
    assert!(!map_text.contains("console.log"));

    let find = client
        .call_tool(
            CallToolRequestParams::new("find_symbol")
                .with_arguments(object!({ "name": "createSession" })),
        )
        .await
        .unwrap();
    let find_text = text_of(&find);
    assert!(
        find_text.contains("src/auth/session.ts:3  function  export function createSession"),
        "{find_text}"
    );

    // Same body as the CLI's `query` for the same inputs (header carries a different retrieval id).
    let cli = std::process::Command::new(env!("CARGO_BIN_EXE_singularrag"))
        .args([
            "query",
            "session",
            "--budget",
            "512",
            "--repo",
            dir.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();
    let cli_text = String::from_utf8(cli.stdout).unwrap();
    let body = |s: &str| s.lines().skip(1).collect::<Vec<_>>().join("\n");
    assert_eq!(body(&map_text), body(&cli_text));

    let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
    let keys: Vec<String> = store
        .conn()
        .prepare("SELECT session_key FROM retrievals WHERE tool IN ('repo_map','find_symbol') ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(
        keys.iter().any(|k| k.starts_with("mcp:singularrag-test:")),
        "{keys:?}"
    );
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn stale_first_call_is_fresh_after_background_drain() {
    let dir = tempfile::tempdir().unwrap();
    write_ts_mini(dir.path());
    let client = connect(dir.path(), &["--refresh-budget-ms", "0"]).await;
    let first = text_of(
        &client
            .call_tool(CallToolRequestParams::new("repo_map").with_arguments(object!({})))
            .await
            .unwrap(),
    );
    assert!(first.contains("STALE:"), "{first}");
    // With a zero budget the drain cannot progress; a normal-budget server would. Restart with the default.
    client.cancel().await.unwrap();
    let client = connect(dir.path(), &[]).await;
    let second = text_of(
        &client
            .call_tool(CallToolRequestParams::new("repo_map").with_arguments(object!({})))
            .await
            .unwrap(),
    );
    assert!(second.contains("· fresh ·"), "{second}");
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn bad_repo_is_a_tool_error_not_a_crash() {
    let missing = Path::new("/nonexistent/singularrag-mcp-test");
    let client = connect(missing, &[]).await;
    let r = client
        .call_tool(CallToolRequestParams::new("repo_map").with_arguments(object!({})))
        .await
        .unwrap();
    assert_eq!(r.is_error, Some(true));
    assert!(text_of(&r).contains("/nonexistent/singularrag-mcp-test"));
    let again = client
        .call_tool(
            CallToolRequestParams::new("find_symbol").with_arguments(object!({ "name": "x" })),
        )
        .await
        .unwrap();
    assert_eq!(
        again.is_error,
        Some(true),
        "server must keep answering after an error"
    );
    client.cancel().await.unwrap();
}
