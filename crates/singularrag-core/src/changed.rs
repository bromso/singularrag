//! `changed`: the symbols a diff touches and who references them. `git diff --unified=0`
//! against HEAD (or a ref) plus untracked files, mapped onto symbol line ranges.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use rusqlite::params;

use crate::blast::referencing_files;
use crate::config::MapConfig;
use crate::store::Store;
use crate::{Error, Result};

pub const MAX_CHANGED: usize = 50;

#[derive(Debug, Clone)]
pub struct ChangedSymbol {
    pub symbol_id: i64,
    pub path: String,
    pub name: String,
    pub kind: String,
    pub signature: String,
    pub line_start: u32,
    pub line_end: u32,
    /// Other files referencing the symbol's name, path order.
    pub referenced_by: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Changed {
    pub base: String,
    pub symbols: Vec<ChangedSymbol>,
    pub files_without_symbols: Vec<String>,
    pub truncated: bool,
}

fn git(root: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| Error::Config(format!("running git: {e}")))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        if err.to_lowercase().contains("not a git repository") {
            return Err(Error::Config("not a git checkout".into()));
        }
        return Err(Error::Config(format!(
            "git {}: {}",
            args.join(" "),
            err.trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `(path, first, last)` new-side line ranges of every hunk in a `--unified=0` diff.
pub fn parse_unified0(diff: &str) -> Vec<(String, u32, u32)> {
    let mut out = Vec::new();
    let mut path = String::new();
    for line in diff.lines() {
        if let Some(p) = line.strip_prefix("+++ b/") {
            path = p.to_string();
        } else if line.starts_with("+++ ") {
            path.clear(); // deleted file: no new side
        } else if let Some(rest) = line.strip_prefix("@@ ") {
            if path.is_empty() {
                continue;
            }
            let Some(plus) = rest.split(' ').find(|s| s.starts_with('+')) else {
                continue;
            };
            let spec = &plus[1..];
            let (start, count) = match spec.split_once(',') {
                Some((s, c)) => (s.parse::<u32>().unwrap_or(0), c.parse::<u32>().unwrap_or(0)),
                None => (spec.parse::<u32>().unwrap_or(0), 1),
            };
            let last = if count == 0 { start } else { start + count - 1 };
            out.push((path.clone(), start.max(1), last.max(1)));
        }
    }
    out
}

pub fn changed(
    store: &Store,
    config: &MapConfig,
    root: &Path,
    base: Option<&str>,
) -> Result<Changed> {
    if let Some(b) = base {
        if b.starts_with('-') || b.is_empty() {
            return Err(Error::Config(format!("base: {b:?} is not a git ref")));
        }
    }
    // Fixed prefixes and no external diff, so a user's `diff.noprefix`,
    // `diff.mnemonicPrefix` or `diff.external` cannot change the format we parse;
    // `core.quotepath=off` keeps non-ASCII paths literal.
    let mut args = vec![
        "-c",
        "core.quotepath=off",
        "diff",
        "--unified=0",
        "--no-color",
        "--no-ext-diff",
        "--src-prefix=a/",
        "--dst-prefix=b/",
    ];
    match base {
        Some(b) => args.push(b),
        // No revision means index vs working tree, which hides staged edits; compare
        // against HEAD when there is one. An unborn HEAD leaves every file untracked.
        None if git(root, &["rev-parse", "--verify", "-q", "HEAD"]).is_ok() => args.push("HEAD"),
        None => {}
    }
    let diff = git(root, &args)?;
    let untracked = git(root, &["ls-files", "--others", "--exclude-standard"])?;
    let mut ranges = parse_unified0(&diff);
    for p in untracked.lines().filter(|l| !l.is_empty()) {
        ranges.push((p.to_string(), 1, u32::MAX));
    }
    let base_name = base.unwrap_or("HEAD").to_string();

    let mut symbols: Vec<ChangedSymbol> = Vec::new();
    let mut files_without: BTreeSet<String> = BTreeSet::new();
    let mut hit_paths: BTreeSet<String> = BTreeSet::new();
    let mut seen: BTreeSet<i64> = BTreeSet::new();
    let mut truncated = false;
    let mut stmt = store.conn().prepare(
        "SELECT s.id, s.name, s.kind, s.signature, s.line_start, s.line_end
         FROM symbols s JOIN files f ON f.id = s.file_id
         WHERE f.path = ?1 AND f.skipped_reason IS NULL AND s.line_start <= ?3 AND s.line_end >= ?2
         ORDER BY s.line_start",
    )?;
    let indexed: Vec<String> = {
        let mut q = store
            .conn()
            .prepare("SELECT path FROM files WHERE skipped_reason IS NULL")?;
        let rows = q
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows
    };
    for (path, first, last) in ranges {
        if !indexed.contains(&path) || config.is_excluded(&path) {
            continue;
        }
        let mut hit = false;
        for row in stmt.query_map(params![path, first, last], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, u32>(4)?,
                r.get::<_, u32>(5)?,
            ))
        })? {
            let (id, name, kind, signature, line_start, line_end) = row?;
            hit = true;
            hit_paths.insert(path.clone());
            if !seen.insert(id) {
                continue;
            }
            if symbols.len() == MAX_CHANGED {
                truncated = true;
                break;
            }
            let referenced_by: Vec<String> = referencing_files(store, &name)?
                .into_iter()
                .map(|(_, p)| p)
                .filter(|p| p != &path && !config.is_excluded(p))
                .collect();
            symbols.push(ChangedSymbol {
                symbol_id: id,
                path: path.clone(),
                name,
                kind,
                signature,
                line_start,
                line_end,
                referenced_by,
            });
        }
        if !hit {
            files_without.insert(path);
        }
    }
    // A file with one hunk inside a symbol and one outside is a changed file, not also
    // a file with no symbol touched.
    files_without.retain(|p| !hit_paths.contains(p));
    symbols.sort_by(|a, b| a.path.cmp(&b.path).then(a.line_start.cmp(&b.line_start)));
    Ok(Changed {
        base: base_name,
        symbols,
        files_without_symbols: files_without.into_iter().collect(),
        truncated,
    })
}

pub fn render_changed(c: &Changed) -> String {
    let mut out = String::new();
    let mut files: BTreeSet<&str> = BTreeSet::new();
    let mut referrers: BTreeSet<&str> = BTreeSet::new();
    for s in &c.symbols {
        files.insert(&s.path);
        let refs = if s.referenced_by.is_empty() {
            String::new()
        } else {
            for r in &s.referenced_by {
                referrers.insert(r);
            }
            format!("  ← {}", s.referenced_by.join(", "))
        };
        out.push_str(&format!(
            "{}::{} (lines {}-{}){refs}\n",
            s.path, s.name, s.line_start, s.line_end
        ));
    }
    for f in &c.files_without_symbols {
        files.insert(f);
        out.push_str(&format!("{f} (no symbol touched)\n"));
    }
    let n = c.symbols.len();
    let more = if c.truncated {
        format!(" · more than {MAX_CHANGED}, showing the first")
    } else {
        String::new()
    };
    out.push_str(&format!(
        "# {n} symbol{} in {} file{} changed since {} · {} referencing file{}{more}\n",
        if n == 1 { "" } else { "s" },
        files.len(),
        if files.len() == 1 { "" } else { "s" },
        c.base,
        referrers.len(),
        if referrers.len() == 1 { "" } else { "s" },
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MapConfig;
    use crate::fixture::write_ts_mini;
    use crate::index::Indexer;
    use crate::store::Store;
    use std::process::Command;

    fn git(root: &std::path::Path, args: &[&str]) {
        let st = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .status()
            .unwrap();
        assert!(st.success(), "git {args:?}");
    }

    /// The fixture as a git repo with one commit, indexed.
    fn repo() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        write_ts_mini(dir.path());
        git(dir.path(), &["init", "-q"]);
        git(
            dir.path(),
            &["-c", "user.email=t@t", "-c", "user.name=t", "add", "."],
        );
        git(
            dir.path(),
            &[
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-q",
                "-m",
                "base",
            ],
        );
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        Indexer::new(&store, dir.path(), &MapConfig::default())
            .unwrap()
            .refresh(None)
            .unwrap();
        (dir, store)
    }

    #[test]
    fn parses_unified_zero_hunks() {
        let diff = "diff --git a/src/a.ts b/src/a.ts\n--- a/src/a.ts\n+++ b/src/a.ts\n@@ -3,0 +4,2 @@\n+x\n+y\n@@ -10 +12 @@\n-z\n+w\n@@ -20,2 +22,0 @@\n-p\n-q\n";
        assert_eq!(
            parse_unified0(diff),
            vec![
                ("src/a.ts".into(), 4, 5),
                ("src/a.ts".into(), 12, 12),
                ("src/a.ts".into(), 22, 22)
            ]
        );
    }

    #[test]
    fn a_modified_symbol_and_an_untracked_file_are_reported_with_referrers() {
        let (dir, store) = repo();
        // Touch createSession's body (line 4 of session.ts in the fixture: inside the function).
        let p = dir.path().join("src/auth/session.ts");
        let text = std::fs::read_to_string(&p).unwrap().replacen(
            "const store = new SessionStore();",
            "// changed\n  const store = new SessionStore();",
            1,
        );
        std::fs::write(&p, text).unwrap();
        std::fs::write(
            dir.path().join("src/new.ts"),
            "export function fresh(): number { return 1; }\n",
        )
        .unwrap();
        Indexer::new(&store, dir.path(), &MapConfig::default())
            .unwrap()
            .refresh(None)
            .unwrap();
        let c = changed(&store, &MapConfig::default(), dir.path(), None).unwrap();
        assert_eq!(c.base, "HEAD");
        let cs = c
            .symbols
            .iter()
            .find(|s| s.name == "createSession")
            .expect("createSession changed");
        assert_eq!(cs.path, "src/auth/session.ts");
        assert!(
            cs.referenced_by
                .contains(&"src/http/middleware.ts".to_string())
                && cs.referenced_by.contains(&"src/cli/login.ts".to_string()),
            "{:?}",
            cs.referenced_by
        );
        assert!(
            c.symbols
                .iter()
                .any(|s| s.path == "src/new.ts" && s.name == "fresh"),
            "untracked file counts as fully changed: {:?}",
            c.symbols
        );
        let text = render_changed(&c);
        assert!(
            text.contains("src/auth/session.ts::createSession (lines "),
            "{text}"
        );
        assert!(
            text.contains("← src/cli/login.ts, src/http/middleware.ts"),
            "{text}"
        );
        assert!(
            text.ends_with("changed since HEAD · 2 referencing files\n"),
            "{text}"
        );
    }

    #[test]
    fn a_change_outside_any_symbol_is_a_file_line_and_base_is_honoured() {
        let (dir, store) = repo();
        let p = dir.path().join("src/cli/login.ts");
        let text = format!("// top comment\n{}", std::fs::read_to_string(&p).unwrap());
        std::fs::write(&p, text).unwrap();
        git(
            dir.path(),
            &[
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-qam",
                "comment",
            ],
        );
        Indexer::new(&store, dir.path(), &MapConfig::default())
            .unwrap()
            .refresh(None)
            .unwrap();
        let now = changed(&store, &MapConfig::default(), dir.path(), None).unwrap();
        assert!(
            now.symbols.is_empty() && now.files_without_symbols.is_empty(),
            "clean tree: {now:?}"
        );
        let vs_base = changed(&store, &MapConfig::default(), dir.path(), Some("HEAD~1")).unwrap();
        assert_eq!(vs_base.base, "HEAD~1");
        assert_eq!(
            vs_base.files_without_symbols,
            vec!["src/cli/login.ts".to_string()]
        );
        assert!(render_changed(&vs_base).contains("src/cli/login.ts (no symbol touched)"));
        let e = changed(
            &store,
            &MapConfig::default(),
            dir.path(),
            Some("--output=/tmp/x"),
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("base"), "{e}");
    }

    #[test]
    fn a_non_git_directory_errors() {
        let dir = tempfile::tempdir().unwrap();
        write_ts_mini(dir.path());
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        Indexer::new(&store, dir.path(), &MapConfig::default())
            .unwrap()
            .refresh(None)
            .unwrap();
        let e = changed(&store, &MapConfig::default(), dir.path(), None)
            .unwrap_err()
            .to_string();
        assert!(e.contains("not a git checkout"), "{e}");
    }

    fn touch_create_session(dir: &std::path::Path) {
        let p = dir.join("src/auth/session.ts");
        let text = std::fs::read_to_string(&p).unwrap().replacen(
            "const store = new SessionStore();",
            "// changed\n  const store = new SessionStore();",
            1,
        );
        std::fs::write(&p, text).unwrap();
    }

    fn refresh(store: &Store, dir: &std::path::Path) {
        Indexer::new(store, dir, &MapConfig::default())
            .unwrap()
            .refresh(None)
            .unwrap();
    }

    fn names(c: &Changed) -> Vec<String> {
        c.symbols.iter().map(|s| s.name.clone()).collect()
    }

    #[test]
    fn staged_edits_count_against_head() {
        let (dir, store) = repo();
        touch_create_session(dir.path());
        git(dir.path(), &["add", "."]);
        refresh(&store, dir.path());
        let c = changed(&store, &MapConfig::default(), dir.path(), None).unwrap();
        assert!(
            names(&c).contains(&"createSession".to_string()),
            "a staged edit is still a change against HEAD: {c:?}"
        );
    }

    #[test]
    fn diff_noprefix_config_does_not_hide_hunks() {
        let (dir, store) = repo();
        git(dir.path(), &["config", "diff.noprefix", "true"]);
        git(dir.path(), &["config", "diff.mnemonicPrefix", "true"]);
        touch_create_session(dir.path());
        refresh(&store, dir.path());
        let c = changed(&store, &MapConfig::default(), dir.path(), None).unwrap();
        assert!(
            names(&c).contains(&"createSession".to_string()),
            "hunks parse whatever the user's prefix config: {c:?}"
        );
    }

    #[test]
    fn a_file_with_a_symbol_hit_and_an_outside_hunk_is_not_listed_twice() {
        let (dir, store) = repo();
        let p = dir.path().join("src/cli/login.ts");
        let text = std::fs::read_to_string(&p).unwrap();
        let text = format!("// top comment\n{}", text).replacen(
            "const s = createSession",
            "// body\n  const s = createSession",
            1,
        );
        std::fs::write(&p, text).unwrap();
        refresh(&store, dir.path());
        let c = changed(&store, &MapConfig::default(), dir.path(), None).unwrap();
        assert!(names(&c).contains(&"login".to_string()), "{c:?}");
        assert!(
            c.files_without_symbols.is_empty(),
            "the file has a symbol hit, so it is not also 'no symbol touched': {c:?}"
        );
        assert!(!render_changed(&c).contains("(no symbol touched)"));
    }

    #[test]
    fn a_repo_with_no_commits_reports_every_file_as_new() {
        let dir = tempfile::tempdir().unwrap();
        write_ts_mini(dir.path());
        git(dir.path(), &["init", "-q"]);
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        refresh(&store, dir.path());
        let c = changed(&store, &MapConfig::default(), dir.path(), None).unwrap();
        assert!(
            names(&c).contains(&"createSession".to_string()),
            "unborn HEAD: the tree is all untracked: {c:?}"
        );
    }
}
