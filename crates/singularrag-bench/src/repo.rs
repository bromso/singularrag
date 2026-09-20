//! The two git facts the harness needs: HEAD, and whether the tree is clean.

use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};

/// Paths under these never count as dirt: singularrag and Serena keep their caches in the checkout.
#[allow(dead_code)]
pub const IGNORED_PREFIXES: &[&str] = &[".singularrag/", ".serena/"];

fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .with_context(|| format!("running git {} in {}", args.join(" "), repo.display()))?;
    if !out.status.success() {
        bail!(
            "git {} failed in {}: {}",
            args.join(" "),
            repo.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Split a git status line into path(s). Handles renames (` -> `) by returning both old and new paths.
fn split_status_line(line: &str) -> Vec<String> {
    if line.len() <= 3 {
        return vec![];
    }
    let path_part = &line[3..].trim();
    if let Some((old, new)) = path_part.split_once(" -> ") {
        vec![old.trim().to_string(), new.trim().to_string()]
    } else {
        vec![path_part.to_string()]
    }
}

#[allow(dead_code)]
pub fn head_commit(repo: &Path) -> Result<String> {
    Ok(git(repo, &["rev-parse", "HEAD"])?.trim().to_string())
}

#[allow(dead_code)]
pub fn dirty_paths(repo: &Path, ignore_prefixes: &[&str]) -> Result<Vec<String>> {
    let out = git(repo, &["status", "--porcelain", "--untracked-files=all"])?;
    Ok(out
        .lines()
        .flat_map(split_status_line)
        .filter(|p| !ignore_prefixes.iter().any(|pre| p.starts_with(pre)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let st = Command::new("git")
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

    fn repo_with_commit() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        git(d.path(), &["init", "-q"]);
        std::fs::write(d.path().join("a.txt"), "a").unwrap();
        git(d.path(), &["add", "."]);
        git(d.path(), &["commit", "-q", "-m", "init"]);
        d
    }

    #[test]
    fn head_commit_is_forty_hex() {
        let d = repo_with_commit();
        let h = head_commit(d.path()).unwrap();
        assert_eq!(h.len(), 40);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn dirty_paths_lists_modified_and_untracked_minus_ignored_prefixes() {
        let d = repo_with_commit();
        assert!(dirty_paths(d.path(), IGNORED_PREFIXES).unwrap().is_empty());
        std::fs::write(d.path().join("a.txt"), "b").unwrap();
        std::fs::create_dir_all(d.path().join(".singularrag")).unwrap();
        std::fs::write(d.path().join(".singularrag/index.db"), "x").unwrap();
        std::fs::write(d.path().join("new.txt"), "n").unwrap();
        let mut dirty = dirty_paths(d.path(), IGNORED_PREFIXES).unwrap();
        dirty.sort();
        assert_eq!(dirty, vec!["a.txt".to_string(), "new.txt".to_string()]);
    }

    #[test]
    fn not_a_repo_is_an_error() {
        let d = tempfile::tempdir().unwrap();
        assert!(head_commit(d.path()).is_err());
    }

    #[test]
    fn split_status_line_handles_renames_by_emitting_both_paths() {
        // Normal modification
        assert_eq!(split_status_line("M  file.txt"), vec!["file.txt"]);
        // Rename: both old and new paths
        assert_eq!(
            split_status_line("R  old.txt -> new.txt"),
            vec!["old.txt", "new.txt"]
        );
        // Untracked
        assert_eq!(split_status_line("?? untracked.txt"), vec!["untracked.txt"]);
    }

    #[test]
    fn split_status_line_filters_each_renamed_path_independently() {
        // Rename into ignored prefix: new path filtered, old path kept
        let paths = split_status_line("R  a.txt -> .singularrag/a.txt");
        let filtered: Vec<_> = paths
            .iter()
            .filter(|p| !IGNORED_PREFIXES.iter().any(|pre| p.starts_with(pre)))
            .cloned()
            .collect();
        assert_eq!(filtered, vec!["a.txt"]);

        // Rename out of ignored prefix: old path filtered, new path kept
        let paths = split_status_line("R  .serena/b.txt -> b.txt");
        let filtered: Vec<_> = paths
            .iter()
            .filter(|p| !IGNORED_PREFIXES.iter().any(|pre| p.starts_with(pre)))
            .cloned()
            .collect();
        assert_eq!(filtered, vec!["b.txt"]);
    }

    #[test]
    fn dirty_paths_includes_both_sides_of_a_rename() {
        let d = repo_with_commit();
        git(d.path(), &["mv", "a.txt", "b.txt"]);
        let mut dirty = dirty_paths(d.path(), IGNORED_PREFIXES).unwrap();
        dirty.sort();
        assert_eq!(dirty, vec!["a.txt".to_string(), "b.txt".to_string()]);
    }
}
