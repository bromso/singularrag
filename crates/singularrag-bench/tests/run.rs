use std::path::{Path, PathBuf};
use std::process::Command as Std;

use assert_cmd::Command;
use predicates::prelude::*;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn git(dir: &Path, args: &[&str]) {
    let st = Std::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .status()
        .unwrap();
    assert!(st.success());
}

/// A workspace: `eval/tier2.toml`, two questions, one condition file, and a `hono/` checkout with one commit.
struct Ws {
    _dir: tempfile::TempDir,
    root: PathBuf,
    commit: String,
}

fn workspace() -> Ws {
    let dir = tempfile::tempdir().unwrap();
    // Canonical, because config::load canonicalises the config directory and macOS temp
    // paths go through a /private symlink.
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let hono = root.join("hono");
    std::fs::create_dir_all(hono.join("src")).unwrap();
    std::fs::write(hono.join("src/router.ts"), "export class Router {}\n").unwrap();
    git(&hono, &["init", "-q"]);
    git(&hono, &["add", "."]);
    git(&hono, &["commit", "-q", "-m", "init"]);
    let commit = String::from_utf8(
        Std::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&hono)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    let ev = root.join("eval");
    std::fs::create_dir_all(ev.join("conditions")).unwrap();
    std::fs::write(ev.join("questions.toml"), r#"
[[question]]
id = "L1"
category = "locate"
query = "where is request routing decided"
gold = ["src/router.ts::Router", "src/router.ts::match", "src/compose.ts::compose", "src/compose.ts::dispatch"]
[[question]]
id = "L2"
category = "locate"
query = "where are middleware chains composed"
gold = ["src/compose.ts::compose"]
"#).unwrap();
    std::fs::write(ev.join("conditions/singularrag.json"), r#"{ "mcpServers": { "singularrag": { "command": "singularrag", "args": ["mcp", "--repo", "<checkout>"] } } }"#).unwrap();
    std::fs::write(
        ev.join("tier2.toml"),
        format!(
            r#"
repo = "../hono"
commit = "{commit}"
repeats = 2
max_turns = 5
max_budget_usd = 0.25
[[condition]]
name = "alone"
baseline = true
[[condition]]
name = "singularrag"
mcp_config = "conditions/singularrag.json"
"#
        ),
    )
    .unwrap();
    Ws {
        _dir: dir,
        root,
        commit,
    }
}

fn bench(ws: &Ws) -> Command {
    let mut c = Command::cargo_bin("singularrag-bench").unwrap();
    c.current_dir(&ws.root)
        .env(
            "SINGULARRAG_BENCH_CLAUDE",
            fixtures().join("fake-claude.sh"),
        )
        .env(
            "SINGULARRAG_BENCH_SINGULARRAG",
            fixtures().join("fake-singularrag.sh"),
        )
        .env("FAKE_CLAUDE_FIXTURE", fixtures().join("success.jsonl"))
        .env_remove("FAKE_CLAUDE_EXIT")
        .env_remove("FAKE_CLAUDE_STDERR")
        .env_remove("FAKE_CLAUDE_TOUCH");
    c
}

fn run_dirs(ws: &Ws) -> Vec<PathBuf> {
    let runs = ws.root.join("eval/runs");
    if !runs.exists() {
        return vec![];
    }
    let mut v: Vec<PathBuf> = std::fs::read_dir(runs)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    v.sort();
    v
}

#[test]
fn dry_run_prints_every_command_and_creates_nothing() {
    let ws = workspace();
    bench(&ws)
        .args([
            "run",
            "--config",
            "eval/tier2.toml",
            "--dry-run",
            "--repeats",
            "1",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("--strict-mcp-config").count(4))
        .stdout(predicate::str::contains("where is request routing decided"))
        .stdout(predicate::str::contains("dry run: 4 sessions"));
    assert!(run_dirs(&ws).is_empty());
}

#[test]
fn run_writes_records_run_toml_and_summary_and_indexes_once() {
    let ws = workspace();
    let mark = ws.root.join("index.mark");
    bench(&ws)
        .env("FAKE_SINGULARRAG_MARK", &mark)
        .args([
            "run",
            "--config",
            "eval/tier2.toml",
            "--label",
            "t",
            "--repeats",
            "1",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("| alone | 2 | 0 |"))
        .stdout(predicate::str::contains("singularrag"));
    let dirs = run_dirs(&ws);
    assert_eq!(dirs.len(), 1);
    let run = &dirs[0];
    assert!(run.file_name().unwrap().to_str().unwrap().ends_with("-t"));
    for c in ["alone", "singularrag"] {
        for q in ["L1", "L2"] {
            assert!(
                run.join(c).join(format!("{q}-1.json")).exists(),
                "{c}/{q}-1.json"
            );
            let stream =
                std::fs::read_to_string(run.join(c).join(format!("{q}-1.stream.jsonl"))).unwrap();
            assert_eq!(stream.lines().count(), 10);
            assert!(!run.join(c).join(format!("{q}-1.stderr")).exists());
        }
    }
    let rec: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(run.join("alone/L1-1.json")).unwrap())
            .unwrap();
    assert_eq!(rec["recall"], 0.5);
    assert_eq!(rec["failed"], false);
    let toml = std::fs::read_to_string(run.join("run.toml")).unwrap();
    assert!(toml.contains("claude_version = \"9.9.9 (Claude Code)\""));
    assert!(toml.contains("singularrag_version = \"singularrag 0.1.0-fake\""));
    assert!(toml.contains(&format!("commit = \"{}\"", ws.commit)));
    assert!(toml.contains("conditions = [\"alone\", \"singularrag\"]"));
    assert!(run.join("summary.md").exists());
    let index_args = std::fs::read_to_string(&mark).expect("singularrag index was run");
    assert!(index_args.contains("index --repo"));
    let mcp = std::fs::read_to_string(run.join("singularrag/mcp.json")).unwrap();
    assert!(!mcp.contains("<checkout>"));
    assert!(mcp.contains(ws.root.join("hono").to_str().unwrap()));
}

#[test]
fn alone_only_run_does_not_index() {
    let ws = workspace();
    let mark = ws.root.join("index.mark");
    bench(&ws)
        .env("FAKE_SINGULARRAG_MARK", &mark)
        .args([
            "run",
            "--config",
            "eval/tier2.toml",
            "--conditions",
            "alone",
            "--questions",
            "L1",
            "--repeats",
            "1",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("No verdict").not());
    assert!(!mark.exists());
    let toml = std::fs::read_to_string(run_dirs(&ws)[0].join("run.toml")).unwrap();
    assert!(!toml.contains("singularrag_version"));
}

#[test]
fn wrong_commit_and_dirty_tree_are_hard_errors_before_any_session() {
    let ws = workspace();
    let cfg = ws.root.join("eval/tier2.toml");
    let text = std::fs::read_to_string(&cfg).unwrap();
    std::fs::write(
        &cfg,
        text.replace(&ws.commit, "0000000000000000000000000000000000000000"),
    )
    .unwrap();
    bench(&ws)
        .args(["run", "--config", "eval/tier2.toml"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("HEAD is"));
    std::fs::write(&cfg, text).unwrap();
    std::fs::write(ws.root.join("hono/dirty.txt"), "x").unwrap();
    bench(&ws)
        .args(["run", "--config", "eval/tier2.toml"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("dirty.txt"));
    assert!(run_dirs(&ws).is_empty());
}

#[test]
fn unknown_question_id_is_an_error() {
    let ws = workspace();
    bench(&ws)
        .args(["run", "--config", "eval/tier2.toml", "--questions", "Z9"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown question Z9"));
}

#[test]
#[cfg(unix)]
fn unreadable_mcp_config_names_the_file_in_the_error() {
    use std::os::unix::fs::PermissionsExt;
    let ws = workspace();
    let path = ws.root.join("eval/conditions/singularrag.json");
    let original = std::fs::metadata(&path).unwrap().permissions();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read_to_string(&path).is_ok() {
        // Running as root (or on a filesystem that ignores the mode bits): nothing to pin here.
        std::fs::set_permissions(&path, original).unwrap();
        return;
    }
    bench(&ws)
        .args([
            "run",
            "--config",
            "eval/tier2.toml",
            "--conditions",
            "singularrag",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(path.to_str().unwrap()))
        .stderr(predicate::str::contains("reading"));
    std::fs::set_permissions(&path, original).unwrap();
}

#[test]
fn a_session_that_dirties_the_tree_aborts_the_run_after_recording_it() {
    let ws = workspace();
    bench(&ws)
        .env("FAKE_CLAUDE_TOUCH", "left-behind.txt")
        .args(["run", "--config", "eval/tier2.toml", "--repeats", "1"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("left-behind.txt"));
    let run = &run_dirs(&ws)[0];
    assert!(run.join("alone/L1-1.json").exists());
    assert!(!run.join("alone/L2-1.json").exists());
}

#[test]
fn nonzero_exit_and_stderr_make_a_failed_session_and_the_run_continues() {
    let ws = workspace();
    bench(&ws)
        .env("FAKE_CLAUDE_EXIT", "3")
        .env("FAKE_CLAUDE_STDERR", "boom")
        .args([
            "run",
            "--config",
            "eval/tier2.toml",
            "--conditions",
            "alone",
            "--repeats",
            "1",
        ])
        .assert()
        .success();
    let run = &run_dirs(&ws)[0];
    let rec: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(run.join("alone/L1-1.json")).unwrap())
            .unwrap();
    assert_eq!(rec["failed"], true);
    assert_eq!(rec["reason"], "exit status 3");
    assert_eq!(
        std::fs::read_to_string(run.join("alone/L1-1.stderr"))
            .unwrap()
            .trim(),
        "boom"
    );
    assert!(run.join("alone/L2-1.json").exists());
}

#[test]
fn a_disconnected_mcp_server_aborts_that_condition() {
    let ws = workspace();
    bench(&ws)
        .env("FAKE_CLAUDE_FIXTURE", fixtures().join("mcp_failed.jsonl"))
        .args([
            "run",
            "--config",
            "eval/tier2.toml",
            "--conditions",
            "singularrag",
            "--repeats",
            "1",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "aborted: singularrag (mcp server serena status failed)",
        ))
        .stdout(predicate::str::contains("No verdict"));
    let run = &run_dirs(&ws)[0];
    assert!(run.join("singularrag/L1-1.json").exists());
    assert!(!run.join("singularrag/L2-1.json").exists());
    let toml = std::fs::read_to_string(run.join("run.toml")).unwrap();
    assert!(toml.contains("[run.aborted]"));
}

#[test]
fn resume_skips_sessions_that_have_records() {
    let ws = workspace();
    bench(&ws)
        .args([
            "run",
            "--config",
            "eval/tier2.toml",
            "--conditions",
            "alone",
            "--questions",
            "L1",
            "--repeats",
            "1",
        ])
        .assert()
        .success();
    let run = run_dirs(&ws).remove(0);
    let before = std::fs::read_to_string(run.join("alone/L1-1.json")).unwrap();
    bench(&ws)
        .env("FAKE_CLAUDE_EXIT", "3")
        .args([
            "run",
            "--config",
            "eval/tier2.toml",
            "--conditions",
            "alone",
            "--repeats",
            "2",
            "--resume",
            run.to_str().unwrap(),
        ])
        .assert()
        .success();
    assert_eq!(
        std::fs::read_to_string(run.join("alone/L1-1.json")).unwrap(),
        before,
        "existing record untouched"
    );
    let rec: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(run.join("alone/L1-2.json")).unwrap())
            .unwrap();
    assert_eq!(
        rec["failed"], true,
        "the new session ran with the failing fake"
    );
    assert_eq!(run_dirs(&ws).len(), 1, "resume creates no second directory");
}
