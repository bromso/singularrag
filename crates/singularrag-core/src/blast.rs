//! Blast radius: which files break if a symbol changes. A breadth-first walk over
//! `refs`: depth 1 is every file referencing the symbol's name, depth n+1 every file
//! referencing any symbol defined in a depth-n file. Name-based, like the ranking graph.

use std::collections::{BTreeMap, HashSet};

use rusqlite::{params, OptionalExtension};
use serde::Serialize;

use crate::config::MapConfig;
use crate::store::Store;
use crate::Result;

pub const MAX_DEPTH: u32 = 3;
pub const MAX_FILES: usize = 200;

#[derive(Debug, Clone, Serialize)]
pub struct BlastFile {
    pub path: String,
    pub depth: u32,
    /// The symbol name through which this file was first reached.
    pub via: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Blast {
    pub root_path: String,
    pub root_symbol: String,
    pub files: Vec<BlastFile>,
    /// `None`, or which cap stopped the walk: `"depth"` or `"files"`.
    pub truncated: Option<&'static str>,
}

fn defined_names(store: &Store, file_ids: &[i64]) -> Result<Vec<String>> {
    let mut names = Vec::new();
    let mut stmt = store
        .conn()
        .prepare("SELECT DISTINCT name FROM symbols WHERE file_id = ?1")?;
    for fid in file_ids {
        for n in stmt.query_map(params![fid], |r| r.get::<_, String>(0))? {
            names.push(n?);
        }
    }
    names.sort();
    names.dedup();
    Ok(names)
}

/// Files (id, path) that reference `name`, non-skipped, in path order.
pub(crate) fn referencing_files(store: &Store, name: &str) -> Result<Vec<(i64, String)>> {
    let mut stmt = store.conn().prepare(
        "SELECT DISTINCT f.id, f.path FROM refs r JOIN files f ON f.id = r.file_id
         WHERE r.name = ?1 AND f.skipped_reason IS NULL ORDER BY f.path",
    )?;
    let rows = stmt
        .query_map(params![name], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn blast_radius(
    store: &Store,
    config: &MapConfig,
    path: &str,
    symbol: &str,
    max_depth: u32,
    max_files: usize,
) -> Result<Option<Blast>> {
    let root: Option<i64> = store
        .conn()
        .query_row(
            "SELECT f.id FROM symbols s JOIN files f ON f.id = s.file_id WHERE f.path = ?1 AND s.name = ?2 LIMIT 1",
            params![path, symbol],
            |r| r.get(0),
        )
        .optional()?;
    let Some(root_id) = root else { return Ok(None) };

    let mut seen: HashSet<i64> = HashSet::from([root_id]);
    let mut files: Vec<BlastFile> = Vec::new();
    let mut truncated = None;
    let mut frontier: Vec<i64> = vec![root_id];
    // Depth 1 follows the root symbol's own name only; deeper levels follow every
    // symbol the frontier files define.
    let mut names: Vec<String> = vec![symbol.to_string()];
    let mut depth = 0u32;
    'walk: while !frontier.is_empty() && !names.is_empty() {
        if depth == max_depth {
            // Something is still reachable: report the depth cap only if a next level
            // would have added a file we have not seen.
            for n in &names {
                if referencing_files(store, n)?
                    .iter()
                    .any(|(id, p)| !seen.contains(id) && !config.is_excluded(p))
                {
                    truncated = Some("depth");
                    break;
                }
            }
            break;
        }
        depth += 1;
        // path -> (id, via): a file reached by several names keeps the alphabetically
        // first name, which `names` is sorted for.
        let mut next: BTreeMap<String, (i64, String)> = BTreeMap::new();
        for n in &names {
            for (id, p) in referencing_files(store, n)? {
                if seen.contains(&id) || config.is_excluded(&p) || next.contains_key(&p) {
                    continue;
                }
                next.insert(p, (id, n.clone()));
            }
        }
        frontier.clear();
        for (p, (id, via)) in next {
            if files.len() == max_files {
                truncated = Some("files");
                break 'walk;
            }
            seen.insert(id);
            frontier.push(id);
            files.push(BlastFile {
                path: p,
                depth,
                via,
            });
        }
        names = defined_names(store, &frontier)?;
    }
    Ok(Some(Blast {
        root_path: path.to_string(),
        root_symbol: symbol.to_string(),
        files,
        truncated,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MapConfig;
    use crate::engine::Engine;
    use crate::fixture::write_ts_mini;
    use crate::store::Store;

    fn engine() -> (tempfile::TempDir, Engine) {
        let dir = tempfile::tempdir().unwrap();
        write_ts_mini(dir.path());
        let mut e = Engine::open(dir.path(), "t").unwrap();
        e.refresh(std::time::Duration::from_secs(5)).unwrap();
        (dir, e)
    }

    #[test]
    fn direct_referencers_are_depth_one_with_the_reaching_name() {
        let (_d, e) = engine();
        let b = blast_radius(
            e.store(),
            &MapConfig::default(),
            "src/auth/session.ts",
            "createSession",
            MAX_DEPTH,
            MAX_FILES,
        )
        .unwrap()
        .unwrap();
        assert_eq!(b.root_path, "src/auth/session.ts");
        assert_eq!(b.root_symbol, "createSession");
        let paths: Vec<(&str, u32, &str)> = b
            .files
            .iter()
            .map(|f| (f.path.as_str(), f.depth, f.via.as_str()))
            .collect();
        assert_eq!(
            paths,
            vec![
                ("src/cli/login.ts", 1, "createSession"),
                ("src/http/middleware.ts", 1, "createSession")
            ]
        );
        assert_eq!(b.truncated, None);
    }

    #[test]
    fn unknown_symbol_or_path_is_none() {
        let (_d, e) = engine();
        assert!(blast_radius(
            e.store(),
            &MapConfig::default(),
            "src/auth/session.ts",
            "nope",
            MAX_DEPTH,
            MAX_FILES
        )
        .unwrap()
        .is_none());
        assert!(blast_radius(
            e.store(),
            &MapConfig::default(),
            "src/nope.ts",
            "createSession",
            MAX_DEPTH,
            MAX_FILES
        )
        .unwrap()
        .is_none());
    }

    #[test]
    fn excluded_files_are_omitted() {
        let (_d, e) = engine();
        let cfg = MapConfig::parse("[[exclude]]\npath = \"src/cli/\"\n").unwrap();
        let b = blast_radius(
            e.store(),
            &cfg,
            "src/auth/session.ts",
            "createSession",
            MAX_DEPTH,
            MAX_FILES,
        )
        .unwrap()
        .unwrap();
        assert_eq!(b.files.len(), 1);
        assert_eq!(b.files[0].path, "src/http/middleware.ts");
    }

    /// Three files in a cycle: a.ts defines `a`, b.ts defines `b` and references `a`,
    /// c.ts defines `c` and references `b`, a.ts references `c`.
    fn cycle_store() -> Store {
        let s = Store::open_in_memory().unwrap();
        let c = s.conn();
        for (id, path) in [(1, "a.ts"), (2, "b.ts"), (3, "c.ts")] {
            c.execute("INSERT INTO files (id, path, lang, content_hash, mtime_ms, size, indexed_at_ms) VALUES (?1, ?2, 'typescript', 'h', 0, 1, 0)", rusqlite::params![id, path]).unwrap();
        }
        for (fid, name) in [(1, "a"), (2, "b"), (3, "c")] {
            c.execute("INSERT INTO symbols (file_id, name, kind, line_start, line_end, signature) VALUES (?1, ?2, 'function', 1, 1, ?2)", rusqlite::params![fid, name]).unwrap();
        }
        for (fid, name) in [(2, "a"), (3, "b"), (1, "c")] {
            c.execute(
                "INSERT INTO refs (file_id, name, line) VALUES (?1, ?2, 1)",
                rusqlite::params![fid, name],
            )
            .unwrap();
        }
        s
    }

    #[test]
    fn walk_follows_depths_and_a_cycle_terminates() {
        let s = cycle_store();
        let b = blast_radius(&s, &MapConfig::default(), "a.ts", "a", MAX_DEPTH, MAX_FILES)
            .unwrap()
            .unwrap();
        let got: Vec<(&str, u32, &str)> = b
            .files
            .iter()
            .map(|f| (f.path.as_str(), f.depth, f.via.as_str()))
            .collect();
        assert_eq!(got, vec![("b.ts", 1, "a"), ("c.ts", 2, "b")]);
        assert_eq!(b.truncated, None, "a.ts is depth 0 and never re-added");
    }

    #[test]
    fn depth_cap_reports_truncation_only_when_more_was_reachable() {
        let s = cycle_store();
        let b = blast_radius(&s, &MapConfig::default(), "a.ts", "a", 1, MAX_FILES)
            .unwrap()
            .unwrap();
        assert_eq!(b.files.len(), 1);
        assert_eq!(b.truncated, Some("depth"));
        let b = blast_radius(&s, &MapConfig::default(), "a.ts", "a", 2, MAX_FILES)
            .unwrap()
            .unwrap();
        assert_eq!(b.truncated, None, "depth 3 would add nothing new");
    }

    #[test]
    fn file_cap_stops_the_walk() {
        let s = cycle_store();
        let b = blast_radius(&s, &MapConfig::default(), "a.ts", "a", MAX_DEPTH, 1)
            .unwrap()
            .unwrap();
        assert_eq!(b.files.len(), 1);
        assert_eq!(b.truncated, Some("files"));
    }
}
