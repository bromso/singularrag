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
                .and(predicate::str::contains("\nsrc/auth/session.ts:"))
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
fn eval_runs_on_fixture_questions() {
    let dir = fixture();
    let q = dir.path().join("q.toml");
    std::fs::write(&q, "[[question]]\nid = \"L1\"\ncategory = \"locate\"\nquery = \"where are sessions created\"\ngold = [\"src/auth/session.ts::createSession\"]\n").unwrap();
    Command::cargo_bin("singularrag")
        .unwrap()
        .args([
            "eval",
            "--questions",
            q.to_str().unwrap(),
            "--repo",
            dir.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "mean recall 1.000 over 1 questions",
        ));
}

#[test]
fn missing_repo_says_which_path_failed() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("no-such-repo");
    Command::cargo_bin("singularrag")
        .unwrap()
        .args(["index", "--repo", missing.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains(format!(
            "opening repo at {}",
            missing.display()
        )));
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

#[test]
fn help_lists_the_mcp_subcommand() {
    Command::cargo_bin("singularrag")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("mcp").and(predicate::str::contains("stdio")));
}

#[test]
fn mcp_exits_cleanly_when_stdin_closes() {
    let dir = fixture();
    let mut cmd = Command::cargo_bin("singularrag").unwrap();
    cmd.args(["mcp", "--repo", dir.path().to_str().unwrap()]);
    cmd.write_stdin("");
    cmd.timeout(std::time::Duration::from_secs(10))
        .assert()
        .success();
}

#[test]
fn path_prints_the_chain_and_changed_needs_git() {
    let dir = fixture();
    Command::cargo_bin("singularrag")
        .unwrap()
        .args([
            "--repo",
            dir.path().to_str().unwrap(),
            "path",
            "src/cli/login.ts::login",
            "src/auth/session.ts::createSession",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("# 1 hop\n"));
    Command::cargo_bin("singularrag")
        .unwrap()
        .args([
            "--repo",
            dir.path().to_str().unwrap(),
            "path",
            "nope.ts::x",
            "src/auth/session.ts::createSession",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "from: nope.ts::x is not in the index",
        ));
    Command::cargo_bin("singularrag")
        .unwrap()
        .args(["--repo", dir.path().to_str().unwrap(), "changed"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not a git checkout"));
}

#[test]
fn hook_read_reads_stdin_and_prints_a_decision() {
    let dir = fixture();
    Command::cargo_bin("singularrag")
        .unwrap()
        .args(["--repo", dir.path().to_str().unwrap(), "index"])
        .assert()
        .success();
    let session = format!("cli-{}", std::process::id());
    let input = format!(
        r#"{{"session_id":"{session}","cwd":"{}","tool_name":"Read","tool_input":{{"file_path":"{}"}}}}"#,
        dir.path().display(),
        dir.path().join("src/auth/session.ts").display()
    );
    let mut cmd = Command::cargo_bin("singularrag").unwrap();
    cmd.args(["--repo", dir.path().to_str().unwrap(), "hook", "read"])
        .write_stdin(input.clone());
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("\"permissionDecision\":\"deny\""));
    let mut again = Command::cargo_bin("singularrag").unwrap();
    again
        .args(["--repo", dir.path().to_str().unwrap(), "hook", "read"])
        .write_stdin(input);
    again
        .assert()
        .success()
        .stdout(predicate::str::contains("\"permissionDecision\":\"allow\""));
    let _ = std::fs::remove_dir_all(std::env::temp_dir().join("singularrag-hook").join(&session));
}

#[test]
fn readme_mcp_json_snippet_is_valid_and_points_at_the_mcp_subcommand() {
    let readme =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../README.md")).unwrap();
    // Anchored on the heading, not on the first json fence in the file: this test is
    // about Claude Code's `.mcp.json`, and reordering the README or adding a snippet
    // above it must not silently point the assertions at someone else's config.
    let section = readme
        .find("### Claude Code")
        .expect("a Claude Code section");
    let start =
        readme[section..].find("```json").expect("json snippet") + section + "```json".len();
    let end = readme[start..].find("```").unwrap() + start;
    let v: serde_json::Value = serde_json::from_str(readme[start..end].trim()).unwrap();
    assert_eq!(v["mcpServers"]["singularrag"]["command"], "singularrag");
    assert_eq!(
        v["mcpServers"]["singularrag"]["args"],
        serde_json::json!(["mcp"])
    );
}

#[test]
fn init_writes_the_claude_files() {
    let dir = fixture();
    Command::cargo_bin("singularrag")
        .unwrap()
        .args([
            "--repo",
            dir.path().to_str().unwrap(),
            "init",
            "--host",
            "claude",
        ])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("wrote .claude/settings.local.json")
                .and(predicate::str::contains("wrote .mcp.json")),
        );
    let s = std::fs::read_to_string(dir.path().join(".claude/settings.local.json")).unwrap();
    assert!(
        s.contains("hook read") && s.contains("mcp__singularrag__repo_map"),
        "{s}"
    );
}
