//! Repo walk: gitignore-aware, denylist-aware, sandboxed to the repo root.

use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;

use crate::{Error, Result};

#[derive(Debug, Clone)]
pub struct WalkEntry {
    pub rel_path: String,
    pub abs_path: PathBuf,
    pub mtime_ms: i64,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Skipped {
    pub rel_path: String,
    pub reason: &'static str,
}

#[derive(Debug, Default)]
pub struct WalkResult {
    pub entries: Vec<WalkEntry>,
    pub skipped: Vec<Skipped>,
}

fn build_denyset(deny: &[String]) -> Result<GlobSet> {
    let mut b = GlobSetBuilder::new();
    for p in deny {
        let glob = if let Some(dir) = p.strip_suffix('/') {
            format!("**/{dir}/**")
        } else {
            format!("**/{p}")
        };
        b.add(Glob::new(&glob).map_err(|e| Error::Config(format!("deny pattern {p}: {e}")))?);
    }
    b.build().map_err(|e| Error::Config(e.to_string()))
}

/// Walk one root; `prefix` (`""` or `"<name>/"`) is prepended to every relative path.
/// Files ignored by `.gitignore` are silently absent; files hit by the denylist or
/// resolving outside `root` are returned in `skipped` with a reason.
pub fn walk_root(root: &Path, prefix: &str, deny: &[String]) -> Result<WalkResult> {
    let root = root.canonicalize()?;
    let denyset = build_denyset(deny)?;
    let mut out = WalkResult::default();

    let walker = WalkBuilder::new(&root)
        .hidden(false)
        .require_git(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(true)
        .follow_links(false)
        .sort_by_file_path(|a, b| a.cmp(b))
        .filter_entry(|e| {
            let n = e.file_name();
            n != ".git" && n != ".singularrag"
        })
        .build();

    for entry in walker {
        let entry = entry.map_err(|e| Error::Io(std::io::Error::other(e)))?;
        let path = entry.path();
        if path == root {
            continue;
        }
        let rel = path
            .strip_prefix(&root)
            .map_err(|_| Error::PathEscape(path.display().to_string()))?
            .to_string_lossy()
            .replace('\\', "/");

        // Resolve symlinks and sandbox.
        let resolved = match path.canonicalize() {
            Ok(p) => p,
            Err(_) => continue, // dangling symlink: nothing to index
        };
        if !resolved.starts_with(&root) {
            out.skipped.push(Skipped {
                rel_path: format!("{prefix}{rel}"),
                reason: "outside-root",
            });
            continue;
        }
        let meta = match std::fs::metadata(&resolved) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if !meta.is_file() {
            continue;
        }
        if denyset.is_match(&rel) {
            out.skipped.push(Skipped {
                rel_path: format!("{prefix}{rel}"),
                reason: "denylisted",
            });
            continue;
        }
        let mtime_ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        out.entries.push(WalkEntry {
            rel_path: format!("{prefix}{rel}"),
            abs_path: resolved,
            mtime_ms,
            size: meta.len(),
        });
    }
    out.entries.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(out)
}

/// The single-root walk, kept for callers and tests that have only a directory.
pub fn walk(root: &Path, deny: &[String]) -> Result<WalkResult> {
    walk_root(root, "", deny)
}

/// Every root of the workspace, concatenated and sorted by prefixed path.
pub fn walk_workspace(ws: &crate::workspace::Workspace, deny: &[String]) -> Result<WalkResult> {
    let mut out = WalkResult::default();
    for r in &ws.roots {
        let one = walk_root(&r.path, &ws.prefix(r), deny)?;
        out.entries.extend(one.entries);
        out.skipped.extend(one.skipped);
    }
    out.entries.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(root: &Path, rel: &str, content: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    #[test]
    fn honours_gitignore_denylist_and_sandbox() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, ".gitignore", "dist/\n");
        write(root, "src/a.ts", "export const a = 1;");
        write(root, "dist/bundle.js", "var x;");
        write(root, ".env", "SECRET=1");
        write(root, "secrets/k.pem", "key");
        write(root, ".git/HEAD", "ref: refs/heads/main");
        write(root, ".singularrag/index.db", "");
        let outside = tempfile::tempdir().unwrap();
        write(outside.path(), "evil.ts", "export const e = 1;");
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path().join("evil.ts"), root.join("src/link.ts"))
            .unwrap();

        let deny: Vec<String> = crate::config::BUILTIN_DENY
            .iter()
            .map(|s| s.to_string())
            .collect();
        let r = walk(root, &deny).unwrap();

        let entries: Vec<&str> = r.entries.iter().map(|e| e.rel_path.as_str()).collect();
        assert_eq!(entries, vec![".gitignore", "src/a.ts"]);
        assert!(r.entries[1].size > 0);
        assert!(r.entries[1].mtime_ms > 0);

        let mut skipped: Vec<(String, &str)> = r
            .skipped
            .iter()
            .map(|s| (s.rel_path.clone(), s.reason))
            .collect();
        skipped.sort();
        let mut expected = vec![
            (".env".to_string(), "denylisted"),
            ("secrets/k.pem".to_string(), "denylisted"),
        ];
        #[cfg(unix)]
        expected.push(("src/link.ts".to_string(), "outside-root"));
        expected.sort();
        assert_eq!(skipped, expected);
    }

    #[test]
    fn extra_deny_pattern_applies() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.snap", "x");
        write(dir.path(), "b.ts", "x");
        let r = walk(dir.path(), &["*.snap".to_string()]).unwrap();
        assert_eq!(r.entries.len(), 1);
        assert_eq!(r.skipped[0].rel_path, "a.snap");
    }

    #[test]
    fn walk_workspace_prefixes_named_roots_and_sorts() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".singularrag")).unwrap();
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        write(a.path(), "src/z.ts", "");
        write(b.path(), "n.md", "");
        std::fs::write(
            d.path().join(crate::workspace::WORKSPACE_FILE),
            format!("[[root]]\nname = \"app\"\npath = \"{}\"\n[[root]]\nname = \"vault\"\npath = \"{}\"\n", a.path().display(), b.path().display()),
        )
        .unwrap();
        let ws = crate::workspace::Workspace::open(d.path()).unwrap();
        let r = walk_workspace(&ws, &[]).unwrap();
        let paths: Vec<&str> = r.entries.iter().map(|e| e.rel_path.as_str()).collect();
        assert_eq!(paths, vec!["app/src/z.ts", "vault/n.md"]);
        let single =
            walk_workspace(&crate::workspace::Workspace::single(a.path()).unwrap(), &[]).unwrap();
        assert_eq!(single.entries[0].rel_path, "src/z.ts");
    }
}
