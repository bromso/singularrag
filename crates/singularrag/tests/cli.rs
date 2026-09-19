use assert_cmd::Command;
use predicates::prelude::*;

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    singularrag_core::fixture::write_ts_mini(dir.path());
    dir
}

#[test]
fn version_prints_name_and_version() {
    Command::cargo_bin("singularrag")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::starts_with("singularrag 0.1.0"));
}

#[test]
fn index_reports_stats() {
    let dir = fixture();
    Command::cargo_bin("singularrag")
        .unwrap()
        .args(["index", "--repo", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("indexed ").and(predicate::str::contains("skipped ")));
    assert!(dir.path().join(".singularrag/index.db").exists());
}

#[test]
fn query_prints_header_map_and_footer() {
    let dir = fixture();
    Command::cargo_bin("singularrag")
        .unwrap()
        .args([
            "query",
            "session",
            "--budget",
            "512",
            "--repo",
            dir.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(
            predicate::str::starts_with("# singularrag · index ")
                .and(predicate::str::contains("src/auth/session.ts:\n"))
                .and(predicate::str::contains("symbols shown")),
        );
}

#[test]
fn find_prints_hits() {
    let dir = fixture();
    Command::cargo_bin("singularrag")
        .unwrap()
        .args([
            "find",
            "createSession",
            "--repo",
            dir.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "src/auth/session.ts:3  function  export function createSession",
        ));
}

#[test]
fn budget_over_cap_is_clamped_not_rejected() {
    let dir = fixture();
    Command::cargo_bin("singularrag")
        .unwrap()
        .args([
            "query",
            "--budget",
            "999999",
            "--repo",
            dir.path().to_str().unwrap(),
        ])
        .assert()
        .success();
}
