//! Drives the real binary over stdio with an rmcp client.

use std::path::Path;

use rmcp::model::{CallToolRequestParams, ClientCapabilities, ClientConfig, Implementation};
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use rmcp::{object, ClientHandler, ServiceExt};
use singularrag_core::fixture::write_ts_mini;
use singularrag_core::store::Store;
use tokio::process::Command;

const REPO_MAP_DESCRIPTION: &str = "Token-budgeted map of the code symbols and document sections most relevant to a task, each file with the files that reference it. Call this first and answer locate, trace, blast-radius and placement questions from it; read a file only to confirm a detail the map does not show. `query` is a question or identifiers; `focus_files` are workspace-relative paths you already know matter; `budget_tokens` defaults to 1024, up to 8192 for trace and blast-radius questions. Each file header ends with `← ` and the files that reference it; rows are `line  signature` for code and `line  ## heading` for document sections, never bodies. The first line says how fresh the index is; if it says STALE, call again after a moment. `entities` and `themes` are optional: entity names you already know matter; themes as short phrases.";
const FIND_SYMBOL_DESCRIPTION: &str = "Look up a symbol by name: exact, prefix, or split words (`create session` finds `createSession`). Returns the definition's path, line and signature and which files reference it. Optional `kind` filter: function, class, method, type, const, module, section, document, element, rule, key. `limit` defaults to 10, max 50.";
const ANNOTATE_DESCRIPTION: &str = "Record what you learned about a file or symbol that its signatures do not say: what it is for, an entry point, a trap, a convention. One or two sentences; the next session and the developer will see it in the map. `path` is workspace-relative; `symbol` narrows the note to one definition in that file. Empty `text` removes your note. You can replace your own note on a target; a note the developer wrote is theirs.";
const TRACE_PATH_DESCRIPTION: &str = "How two symbols connect: the shortest chain of references between `from` and `to`, each `path::name`, up to 6 hops, with the symbol each hop goes through. Use it for trace questions before reading files.";
const ENTITIES_DESCRIPTION: &str = "What the corpus knows about a person, system, concept or event, and how it connects: matched entities with type and description, their relations, and the document sections that state them (path::heading, lines). `query` is a name or a question; `entities` are names you already know; `limit` defaults to 10, max 25. Descriptions are extracted text, not verified facts. Use it before reading a document about someone or something.";
const CHANGED_DESCRIPTION: &str = "What a change touches: the symbols whose lines a diff modifies and the files that reference each. `base` is a git ref; omitted means the working tree against HEAD. Use it before editing to see the blast radius and after editing to check it.";

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
    connect_with_log(repo, extra, "warn").await
}

/// `log` is the child's `RUST_LOG`. It matters: stdout is the protocol and nothing else
/// (spec §2), and rmcp's client fails to parse the moment a non-JSON line lands there,
/// so running a test at `debug` is how stdout purity is checked.
async fn connect_with_log(
    repo: &Path,
    extra: &[&str],
    log: &str,
) -> rmcp::service::RunningService<rmcp::RoleClient, TestClient> {
    connect_with_env(repo, extra, log, &[]).await
}

/// `env` is set on the child on top of `RUST_LOG`.
async fn connect_with_env(
    repo: &Path,
    extra: &[&str],
    log: &str,
    env: &[(&str, &str)],
) -> rmcp::service::RunningService<rmcp::RoleClient, TestClient> {
    let env: Vec<(String, String)> = env
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let bin = env!("CARGO_BIN_EXE_singularrag");
    let repo = repo.to_path_buf();
    let extra: Vec<String> = extra.iter().map(|s| s.to_string()).collect();
    let log = log.to_string();
    let transport = TokioChildProcess::new(Command::new(bin).configure(move |c| {
        c.arg("mcp").arg("--repo").arg(&repo);
        for e in &extra {
            c.arg(e);
        }
        c.env("RUST_LOG", &log);
        for (k, v) in &env {
            c.env(k, v);
        }
        // A panic before `client.cancel()` must not leave a `singularrag mcp` child
        // running for the rest of the machine's day.
        c.kill_on_drop(true);
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

/// Runs at `RUST_LOG=debug` on purpose: every assertion below is also an assertion that
/// nothing but JSON-RPC reached stdout, because the client could not have parsed a single
/// message otherwise.
#[tokio::test]
async fn lists_exactly_the_six_tools_with_spec_descriptions() {
    let dir = tempfile::tempdir().unwrap();
    write_ts_mini(dir.path());
    let client = connect_with_log(dir.path(), &[], "debug").await;
    let info = client.peer_info().expect("server info");
    let server_info = info.server_info.as_ref().expect("server_info present");
    assert_eq!(server_info.name, "singularrag");
    let instructions = info.instructions.as_deref().unwrap_or("");
    assert!(instructions.starts_with("singularrag gives you a ranked map"));
    assert!(
        instructions.contains("Use entities to learn what the corpus says about a person, system or concept and how it connects; prose queries to repo_map work in plain language.")
            && instructions.ends_with("Entity descriptions are extracted text, not verified facts."),
        "{instructions}"
    );
    let mut tools = client.list_all_tools().await.unwrap();
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(tools.len(), 6);
    assert_eq!(tools[0].name, "annotate");
    assert_eq!(tools[0].description.as_deref(), Some(ANNOTATE_DESCRIPTION));
    assert_eq!(tools[1].name, "changed");
    assert_eq!(tools[1].description.as_deref(), Some(CHANGED_DESCRIPTION));
    assert_eq!(tools[2].name, "entities");
    assert_eq!(tools[2].description.as_deref(), Some(ENTITIES_DESCRIPTION));
    assert_eq!(tools[3].name, "find_symbol");
    assert_eq!(
        tools[3].description.as_deref(),
        Some(FIND_SYMBOL_DESCRIPTION)
    );
    assert_eq!(tools[4].name, "repo_map");
    assert_eq!(tools[4].description.as_deref(), Some(REPO_MAP_DESCRIPTION));
    assert_eq!(tools[5].name, "trace_path");
    assert_eq!(
        tools[5].description.as_deref(),
        Some(TRACE_PATH_DESCRIPTION)
    );
    client.cancel().await.unwrap();
}

/// The knowledge queue is drained in-process first (warm-up ticks against the fake), so
/// the server only has to embed the query; the test still waits until `repo_map`'s
/// header shows nothing pending before asking.
#[tokio::test]
async fn entities_answers_over_the_binary() {
    let dir = tempfile::tempdir().unwrap();
    singularrag_core::fixture::write_docs_mini(dir.path());
    let f = singularrag_core::fake_ollama::FakeOllama::spawn(32);
    f.set_extraction("Freshness", serde_json::json!({"entities": [{"name": "STALE header", "type": "concept", "description": "the header when files changed"}, {"name": "createSession", "type": "system", "description": "creates a session"}], "relations": [{"source": "STALE header", "target": "createSession", "description": "refresh runs before session creation"}]}));
    f.set_extraction("Storage", serde_json::json!({"entities": [{"name": "SessionStore", "type": "system", "description": "keeps sessions"}, {"name": "createSession", "type": "system", "description": "creates a session"}], "relations": []}));
    // On its own thread: the Engine's blocking HTTP client must not be dropped inside
    // the async runtime.
    let (root, url) = (dir.path().to_path_buf(), f.url());
    std::thread::spawn(move || {
        let mut e = singularrag_core::engine::Engine::open(&root, "warm-up").unwrap();
        e.set_models_url(&url);
        e.refresh(std::time::Duration::from_secs(60)).unwrap();
        for _ in 0..20 {
            if e.knowledge_tick(&|| false).unwrap().pending == 0 {
                break;
            }
        }
    })
    .join()
    .unwrap();
    let url = f.url();
    let client = connect_with_env(
        dir.path(),
        &[],
        "warn",
        &[("SINGULARRAG_OLLAMA_URL", url.as_str())],
    )
    .await;
    let mut header = String::new();
    for _ in 0..30 {
        let map = client
            .call_tool(CallToolRequestParams::new("repo_map").with_arguments(object!({})))
            .await
            .unwrap();
        header = text_of(&map).lines().next().unwrap_or("").to_string();
        if !header.contains("pending") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    assert!(!header.contains("pending"), "{header}");

    let r = client
        .call_tool(
            CallToolRequestParams::new("entities")
                .with_arguments(object!({ "query": "stale header" })),
        )
        .await
        .unwrap();
    assert_ne!(r.is_error, Some(true), "{}", text_of(&r));
    let t = text_of(&r);
    assert!(t.starts_with("# singularrag · index "), "{t}");
    assert!(
        t.contains("\nSTALE header (concept): the header when files changed\n  ← docs/design.md::Freshness (lines "),
        "{t}"
    );
    assert!(
        t.contains("  STALE header → createSession: refresh runs before session creation (docs/design.md::Freshness, lines "),
        "{t}"
    );
    let named = client
        .call_tool(CallToolRequestParams::new("entities").with_arguments(
            object!({ "query": "who keeps things", "entities": ["SessionStore"], "limit": 1 }),
        ))
        .await
        .unwrap();
    let t = text_of(&named);
    assert!(
        t.contains("\nSessionStore (system): keeps sessions\n  ← docs/design.md::Storage (lines "),
        "{t}"
    );
    assert!(t.ends_with("# 1 entity · 0 relations · 1 section\n"), "{t}");
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn trace_path_and_changed_answer_over_the_binary() {
    let dir = tempfile::tempdir().unwrap();
    write_ts_mini(dir.path());
    let client = connect(dir.path(), &[]).await;
    let r = client
        .call_tool(CallToolRequestParams::new("trace_path").with_arguments(object!({ "from": "src/cli/login.ts::login", "to": "src/auth/session.ts::createSession" })))
        .await
        .unwrap();
    assert_ne!(r.is_error, Some(true), "{}", text_of(&r));
    assert!(text_of(&r).contains("# 1 hop"), "{}", text_of(&r));
    let bad = client
        .call_tool(CallToolRequestParams::new("trace_path").with_arguments(
            object!({ "from": "login", "to": "src/auth/session.ts::createSession" }),
        ))
        .await
        .unwrap();
    assert_eq!(bad.is_error, Some(true));
    assert!(text_of(&bad).contains("path::name"), "{}", text_of(&bad));
    let c = client
        .call_tool(CallToolRequestParams::new("changed").with_arguments(object!({})))
        .await
        .unwrap();
    assert_eq!(c.is_error, Some(true), "the fixture is not a git checkout");
    assert!(
        text_of(&c).contains("not a git checkout"),
        "{}",
        text_of(&c)
    );
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn annotate_writes_a_note_the_next_map_shows() {
    let dir = tempfile::tempdir().unwrap();
    write_ts_mini(dir.path());
    let client = connect(dir.path(), &[]).await;
    let r = client
        .call_tool(
            CallToolRequestParams::new("annotate").with_arguments(object!({
                "path": "src/auth/session.ts",
                "symbol": "createSession",
                "text": "Sessions are minted here; the CLI and the middleware both call it."
            })),
        )
        .await
        .unwrap();
    assert_ne!(r.is_error, Some(true), "{}", text_of(&r));
    let t = text_of(&r);
    assert!(
        t.starts_with("# singularrag · index ")
            && t.contains("noted src/auth/session.ts::createSession (1 note on this file)"),
        "{t}"
    );
    let on_disk = std::fs::read_to_string(dir.path().join(".singularrag/map.toml")).unwrap();
    assert!(
        on_disk.contains("by = \"agent\"") && on_disk.contains("session = \"mcp:"),
        "{on_disk}"
    );

    let map = client
        .call_tool(
            CallToolRequestParams::new("repo_map")
                .with_arguments(object!({ "query": "minted", "budget_tokens": 1024 })),
        )
        .await
        .unwrap();
    let map_text = text_of(&map);
    assert!(
        map_text.contains("        note (agent): Sessions are minted here"),
        "{map_text}"
    );

    let bad = client
        .call_tool(
            CallToolRequestParams::new("annotate")
                .with_arguments(object!({ "path": "src/nope.ts", "text": "x" })),
        )
        .await
        .unwrap();
    assert_eq!(bad.is_error, Some(true));
    assert!(
        text_of(&bad).contains("not an indexed file"),
        "{}",
        text_of(&bad)
    );
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
    assert!(map_text
        .lines()
        .any(|l| l.starts_with("src/auth/session.ts:")));
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
async fn repo_map_serves_a_document_section() {
    let dir = tempfile::tempdir().unwrap();
    singularrag_core::fixture::write_docs_mini(dir.path());
    let client = connect(dir.path(), &[]).await;

    let map = client
        .call_tool(CallToolRequestParams::new("repo_map").with_arguments(
            object!({ "query": "what does the STALE header mean", "budget_tokens": 2048 }),
        ))
        .await
        .unwrap();
    assert_ne!(map.is_error, Some(true));
    let text = text_of(&map);
    assert!(text.contains("docs/runbook.md:"), "{text}");
    assert!(text.contains("## When the header says STALE"), "{text}");
    client.cancel().await.unwrap();
}

/// What this pins is the flag, not the drain: a zero budget cannot index a file, so the
/// first call is STALE and the drain stops on its first no-progress chunk. A server with
/// the default budget indexes inline and answers fresh. The drain proper is
/// `same_session_drain_makes_the_next_call_fresh` below.
#[tokio::test]
async fn refresh_budget_flag_controls_first_call_freshness() {
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

/// Spec §8's second half, end to end in one session: a budget too small to finish the
/// backlog inline leaves the first call STALE, the actor drains the rest between calls,
/// and a later call on the *same* connection says `fresh` — no restart, no extra tool
/// call driving the work.
#[tokio::test]
async fn same_session_drain_makes_the_next_call_fresh() {
    let dir = tempfile::tempdir().unwrap();
    write_ts_mini(dir.path());
    // A backlog far larger than one 20 ms chunk, so freshness cannot come from the
    // inline refreshes the polling calls do: that would take one chunk per call, and
    // the loop below only ever makes a handful of calls.
    std::fs::create_dir_all(dir.path().join("src/gen")).unwrap();
    for i in 0..400 {
        std::fs::write(
            dir.path().join(format!("src/gen/g{i}.ts")),
            format!(
                "export function gen{i}(a: string): string {{\n  return a + \"{i}\";\n}}\nexport class Gen{i} {{\n  run(a: string): string {{ return gen{i}(a); }}\n}}\n"
            ),
        )
        .unwrap();
    }
    let client = connect(dir.path(), &["--refresh-budget-ms", "20"]).await;
    let first = text_of(
        &client
            .call_tool(CallToolRequestParams::new("repo_map").with_arguments(object!({})))
            .await
            .unwrap(),
    );
    assert!(first.contains("STALE:"), "{first}");

    let mut last = first;
    for _ in 0..15 {
        // A quiet second: with no jobs queued, only the drain can move the backlog.
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        last = text_of(
            &client
                .call_tool(CallToolRequestParams::new("repo_map").with_arguments(object!({})))
                .await
                .unwrap(),
        );
        if last.contains("· fresh ·") {
            client.cancel().await.unwrap();
            return;
        }
    }
    panic!("the drain never caught up in 15 s: {last}");
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
