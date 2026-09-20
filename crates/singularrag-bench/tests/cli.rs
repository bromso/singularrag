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
