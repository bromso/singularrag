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
            "opening workspace at {}",
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
fn query_takes_repeatable_entities_and_themes() {
    let dir = fixture();
    Command::cargo_bin("singularrag")
        .unwrap()
        .args([
            "query",
            "session",
            "--entity",
            "SessionStore",
            "--entity",
            "createSession",
            "--theme",
            "refresh first",
            "--theme",
            "login flow",
            "--repo",
            dir.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\nsrc/auth/session.ts:"));
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

#[test]
fn mcp_help_names_all_six_tools() {
    Command::cargo_bin("singularrag")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "repo_map, find_symbol, trace_path, changed, annotate and entities",
        ));
}

#[test]
fn changed_in_a_named_workspace_prefixes_paths() {
    let dir = tempfile::tempdir().unwrap();
    let (app, _) = singularrag_core::fixture::write_workspace(dir.path());
    let git = |args: &[&str]| {
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(&app)
            .args(args)
            .status()
            .unwrap()
            .success());
    };
    git(&["init", "-q"]);
    git(&["-c", "user.email=t@t", "-c", "user.name=t", "add", "."]);
    git(&[
        "-c",
        "user.email=t@t",
        "-c",
        "user.name=t",
        "commit",
        "-qm",
        "base",
    ]);
    std::fs::write(
        app.join("src/util/log.ts"),
        "export function log(msg: string): void {\n  // changed\n  console.log(msg);\n}\n",
    )
    .unwrap();
    Command::cargo_bin("singularrag")
        .unwrap()
        .args(["--repo", dir.path().to_str().unwrap(), "changed"])
        .assert()
        .success()
        .stdout(predicate::str::contains("app/src/util/log.ts::log"));
}

#[test]
fn query_and_changed_cover_documents() {
    let dir = tempfile::tempdir().unwrap();
    singularrag_core::fixture::write_docs_mini(dir.path());
    Command::cargo_bin("singularrag")
        .unwrap()
        .args([
            "--repo",
            dir.path().to_str().unwrap(),
            "query",
            "STALE header",
            "--budget",
            "2048",
        ])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("docs/runbook.md:")
                .and(predicate::str::contains("## When the header says STALE")),
        );
    let git = |args: &[&str]| {
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(args)
            .status()
            .unwrap()
            .success());
    };
    git(&["init", "-q"]);
    git(&["-c", "user.email=t@t", "-c", "user.name=t", "add", "."]);
    git(&[
        "-c",
        "user.email=t@t",
        "-c",
        "user.name=t",
        "commit",
        "-qm",
        "base",
    ]);
    std::fs::write(
        dir.path().join("docs/runbook.md"),
        "# Runbook\n\n## When the header says STALE\n\nWait longer.\n",
    )
    .unwrap();
    Command::cargo_bin("singularrag")
        .unwrap()
        .args(["--repo", dir.path().to_str().unwrap(), "changed"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "docs/runbook.md::When the header says STALE",
        ));
}

#[test]
fn readme_cli_block_lists_the_new_subcommands() {
    let readme =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../README.md")).unwrap();
    let cli = readme.find("## CLI").expect("a CLI section");
    let block = &readme[cli..readme[cli + 1..].find("\n## ").unwrap() + cli + 1];
    for sub in [
        "singularrag path ",
        "singularrag changed",
        "singularrag init",
        "singularrag entities ",
    ] {
        assert!(
            block.contains(sub),
            "README CLI block lacks `{sub}`:\n{block}"
        );
    }
}

#[test]
fn readme_lists_six_tools_and_the_models_setup() {
    let readme =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../README.md")).unwrap();
    for needle in [
        "Six tools",
        "ollama pull nomic-embed-text",
        "singularrag doctor",
    ] {
        assert!(readme.contains(needle), "README lacks `{needle}`");
    }
}

#[test]
fn the_docs_question_file_loads_and_names_sections_that_exist() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../e",
        "val/questions-docs.toml"
    );
    let qs = singularrag_core::eval::load_questions(std::path::Path::new(path)).unwrap();
    assert_eq!(qs.len(), 12);
    let cats: std::collections::BTreeSet<&str> = qs.iter().map(|q| q.category.as_str()).collect();
    assert_eq!(
        cats.into_iter().collect::<Vec<_>>(),
        vec!["code-to-doc", "doc-to-code", "locate-doc"]
    );
    // Every gold section must exist in an index of this repository's corpus. The corpus is
    // copied into a tempdir so the test never writes (or waits on the lock of) the
    // checkout's own `.singularrag/index.db`.
    let repo = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
    let copy = tempfile::tempdir().unwrap();
    std::fs::copy(repo.join("README.md"), copy.path().join("README.md")).unwrap();
    copy_tree(&repo.join("docs"), &copy.path().join("docs"));
    for krate in ["singularrag", "singularrag-core", "singularrag-bench"] {
        copy_tree(
            &repo.join("crates").join(krate).join("src"),
            &copy.path().join("crates").join(krate).join("src"),
        );
    }
    let mut e = singularrag_core::engine::Engine::open(copy.path(), "docs-gold-check").unwrap();
    e.refresh(std::time::Duration::from_secs(120)).unwrap();
    for q in &qs {
        for g in &q.gold {
            let (path, name) = g.split_once("::").unwrap();
            let n: i64 = e
                .store()
                .conn()
                .query_row(
                    "SELECT COUNT(*) FROM symbols s JOIN files f ON f.id = s.file_id WHERE f.path = ?1 AND s.name = ?2",
                    [path, name],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(n > 0, "{} gold {g} is not in the index", q.id);
        }
    }
}

fn prose_questions_path() -> &'static str {
    concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../e",
        "val/questions-prose.toml"
    )
}

#[test]
fn the_prose_question_file_loads_and_names_sections_that_exist() {
    let qs = singularrag_core::eval::load_questions(std::path::Path::new(prose_questions_path()))
        .unwrap();
    assert_eq!(qs.len(), 12);
    let mut cats: std::collections::BTreeMap<&str, usize> = Default::default();
    for q in &qs {
        *cats.entry(q.category.as_str()).or_default() += 1;
    }
    assert_eq!(
        cats.into_iter().collect::<Vec<_>>(),
        vec![("entity", 4), ("paraphrase", 4), ("relation", 4)]
    );
    // Gold is checked against an index of the fixture in a tempdir, the corpus the questions
    // were authored from.
    let dir = tempfile::tempdir().unwrap();
    singularrag_core::fixture::write_prose(dir.path());
    let mut e = singularrag_core::engine::Engine::open(dir.path(), "prose-gold-check").unwrap();
    e.set_models_enabled(false);
    e.refresh(std::time::Duration::from_secs(120)).unwrap();
    for q in &qs {
        assert!(!q.gold.is_empty(), "{} has no gold", q.id);
        for g in &q.gold {
            let (path, name) = g.split_once("::").unwrap();
            let n: i64 = e
                .store()
                .conn()
                .query_row(
                    "SELECT COUNT(*) FROM symbols s JOIN files f ON f.id = s.file_id WHERE f.path = ?1 AND s.name = ?2",
                    [path, name],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(n > 0, "{} gold {g} is not in the index", q.id);
        }
    }
}

#[test]
fn eval_no_models_runs_the_prose_questions() {
    let dir = tempfile::tempdir().unwrap();
    singularrag_core::fixture::write_prose(dir.path());
    Command::cargo_bin("singularrag")
        .unwrap()
        // Nothing listens here: with --no-models the run must never ask.
        .env("SINGULARRAG_OLLAMA_URL", "http://127.0.0.1:9")
        .args([
            "eval",
            "--questions",
            prose_questions_path(),
            "--budget",
            "4096",
            "--no-models",
            "--repo",
            dir.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("mean recall"));
}

/// Recursively copy `from` into `to`, skipping build output, dependencies, VCS and index
/// directories.
fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if [".git", ".singularrag", "target", "node_modules"]
            .iter()
            .any(|skip| name == *skip)
        {
            continue;
        }
        let ty = entry.file_type().unwrap();
        if ty.is_dir() {
            copy_tree(&entry.path(), &to.join(&name));
        } else if ty.is_file() {
            std::fs::copy(entry.path(), to.join(&name)).unwrap();
        }
    }
}

#[test]
fn doctor_reports_ollama_and_the_queue() {
    let dir = tempfile::tempdir().unwrap();
    singularrag_core::fixture::write_docs_mini(dir.path());
    let f = singularrag_core::fake_ollama::FakeOllama::spawn(8);
    Command::cargo_bin("singularrag")
        .unwrap()
        .args(["--repo", dir.path().to_str().unwrap(), "index"])
        .assert()
        .success();
    Command::cargo_bin("singularrag")
        .unwrap()
        .env("SINGULARRAG_OLLAMA_URL", f.url())
        .args(["--repo", dir.path().to_str().unwrap(), "doctor"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("ollama: ok")
                .and(predicate::str::contains("qwen2.5:7b-instruct: pulled"))
                .and(predicate::str::contains("pending:")),
        );
    Command::cargo_bin("singularrag")
        .unwrap()
        .env("SINGULARRAG_OLLAMA_URL", "http://127.0.0.1:9")
        .args(["--repo", dir.path().to_str().unwrap(), "doctor"])
        .assert()
        .success()
        .stdout(predicate::str::contains("ollama: unreachable"));
}

#[test]
fn entities_drains_the_queue_then_prints_the_block() {
    let dir = tempfile::tempdir().unwrap();
    singularrag_core::fixture::write_docs_mini(dir.path());
    let f = singularrag_core::fake_ollama::FakeOllama::spawn(32);
    f.set_extraction("Freshness", serde_json::json!({"entities": [{"name": "STALE header", "type": "concept", "description": "the header when files changed"}, {"name": "createSession", "type": "system", "description": "creates a session"}], "relations": [{"source": "STALE header", "target": "createSession", "description": "refresh runs before session creation"}]}));
    f.set_extraction("Storage", serde_json::json!({"entities": [{"name": "SessionStore", "type": "system", "description": "keeps sessions"}, {"name": "createSession", "type": "system", "description": "creates a session"}], "relations": []}));
    Command::cargo_bin("singularrag")
        .unwrap()
        .env("SINGULARRAG_OLLAMA_URL", f.url())
        .args([
            "entities",
            "stale header",
            "--entity",
            "SessionStore",
            "--limit",
            "5",
            "--repo",
            dir.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(
            predicate::str::starts_with("# singularrag · index ")
                .and(predicate::str::contains("\nSessionStore (system): keeps sessions\n  ← docs/design.md::Storage (lines "))
                .and(predicate::str::contains("\nSTALE header (concept): the header when files changed\n  ← docs/design.md::Freshness (lines "))
                .and(predicate::str::contains("  STALE header → createSession: refresh runs before session creation (docs/design.md::Freshness, lines "))
                .and(predicate::str::contains("pending").not()),
        );
}
