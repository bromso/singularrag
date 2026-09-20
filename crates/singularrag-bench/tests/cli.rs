use assert_cmd::Command;
use predicates::prelude::*;

fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn parse_prints_tool_counts_and_result() {
    Command::cargo_bin("singularrag-bench")
        .unwrap()
        .args(["parse", &fixture("success.jsonl")])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "\"mcp__singularrag__repo_map\": 1",
        ))
        .stdout(predicate::str::contains("\"subtype\": \"success\""));
}

#[test]
fn parse_missing_file_is_an_error() {
    Command::cargo_bin("singularrag-bench")
        .unwrap()
        .args(["parse", "/nonexistent/stream.jsonl"])
        .assert()
        .failure();
}

#[test]
fn score_rewrites_summary_from_records() {
    let dir = tempfile::tempdir().unwrap();
    let run = dir.path().join("20260920T000000Z-run");
    std::fs::create_dir_all(run.join("alone")).unwrap();
    std::fs::create_dir_all(run.join("singularrag")).unwrap();
    std::fs::write(
        run.join("run.toml"),
        r#"
[run]
created = "20260920T000000Z"
label = "run"
commit = "098e119"
claude_version = "2.1.261"
baseline = "alone"
conditions = ["alone", "singularrag"]
questions = ["L1"]
[config]
repo = "/tmp/hono"
commit = "098e119"
questions = "/tmp/q.toml"
[[config.condition]]
name = "alone"
baseline = true
[[config.condition]]
name = "singularrag"
"#,
    )
    .unwrap();
    let rec = |cond: &str, recall: f64, tokens: u64| {
        format!(
            r#"{{"question":"L1","condition":"{cond}","repeat":1,"model":"m","recall":{recall},"precision":1.0,"hit":[],"miss":[],"answer":[],"tool_calls":{{"Grep":2}},"tokens":{{"input":{tokens},"output":0,"cache_creation":0,"cache_read":0}},"cost_usd":0.1,"turns":2,"duration_ms":1000,"failed":false,"reason":null}}"#
        )
    };
    std::fs::write(run.join("alone/L1-1.json"), rec("alone", 0.5, 1000)).unwrap();
    std::fs::write(
        run.join("singularrag/L1-1.json"),
        rec("singularrag", 0.5, 500),
    )
    .unwrap();
    Command::cargo_bin("singularrag-bench")
        .unwrap()
        .args(["score", run.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("**singularrag earns its place**"));
    assert!(run.join("summary.md").exists());
}
