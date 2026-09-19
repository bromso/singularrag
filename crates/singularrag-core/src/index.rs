//! Indexer: walk, diff against the `files` table, parse changed files, write symbols,
//! refs and FTS rows. Runs until an optional deadline; whatever is left is reported
//! as `remaining` so callers can say "STALE: N files changed since index".

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use rusqlite::{params, Connection};

use crate::config::MapConfig;
use crate::lang::{extract_tags, Language};
use crate::secrets::looks_secret;
use crate::store::Store;
use crate::time::now_ms;
use crate::tokens::split_identifier;
use crate::walk::{walk, WalkEntry};
use crate::Result;

pub const MAX_FILE_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct IndexStats {
    pub scanned: usize,
    pub indexed: usize,
    pub unchanged: usize,
    pub skipped: usize,
    pub removed: usize,
    pub remaining: usize,
}

pub struct Indexer<'a> {
    store: &'a Store,
    root: PathBuf,
    config: &'a MapConfig,
}

#[derive(Clone)]
struct Known {
    id: i64,
    mtime_ms: i64,
    size: u64,
    content_hash: Option<String>,
}

/// What `index_file` did with one changed file.
enum Outcome {
    Indexed,
    Unchanged,
    Skipped,
}

impl<'a> Indexer<'a> {
    pub fn new(store: &'a Store, root: &Path, config: &'a MapConfig) -> Result<Self> {
        Ok(Indexer {
            store,
            root: root.canonicalize()?,
            config,
        })
    }

    fn known_files(&self) -> Result<HashMap<String, Known>> {
        let mut stmt = self
            .store
            .conn()
            .prepare("SELECT id, path, mtime_ms, size, content_hash FROM files")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(1)?,
                Known {
                    id: r.get(0)?,
                    mtime_ms: r.get(2)?,
                    size: r.get::<_, i64>(3)? as u64,
                    content_hash: r.get(4)?,
                },
            ))
        })?;
        let mut m = HashMap::new();
        for row in rows {
            let (p, k) = row?;
            m.insert(p, k);
        }
        Ok(m)
    }

    /// Files whose mtime or size differ from the table, or that are not in it yet.
    pub fn stale_count(&self) -> Result<usize> {
        let known = self.known_files()?;
        let w = walk(&self.root, &self.config.deny_patterns())?;
        Ok(w.entries
            .iter()
            .filter(|e| Self::changed(&known, e))
            .count())
    }

    fn changed(known: &HashMap<String, Known>, e: &WalkEntry) -> bool {
        match known.get(&e.rel_path) {
            Some(k) => k.mtime_ms != e.mtime_ms || k.size != e.size,
            None => true,
        }
    }

    pub fn refresh(&self, deadline: Option<Instant>) -> Result<IndexStats> {
        let mut stats = IndexStats::default();
        let known = self.known_files()?;
        let w = walk(&self.root, &self.config.deny_patterns())?;
        stats.scanned = w.entries.len() + w.skipped.len();
        let now = now_ms();

        // Skipped-by-walk files become rows with a reason and no symbols.
        for s in &w.skipped {
            self.upsert_skipped(&s.rel_path, 0, 0, s.reason, now)?;
            stats.skipped += 1;
        }

        let mut seen: Vec<&str> = Vec::with_capacity(w.entries.len());
        for e in &w.entries {
            seen.push(&e.rel_path);
            if !Self::changed(&known, e) {
                stats.unchanged += 1;
                continue;
            }
            if deadline.is_some_and(|d| Instant::now() >= d) {
                stats.remaining += 1;
                continue;
            }
            let prior = known.get(&e.rel_path);
            match self.index_file(e, prior, now)? {
                Outcome::Indexed => stats.indexed += 1,
                Outcome::Unchanged => stats.unchanged += 1,
                Outcome::Skipped => stats.skipped += 1,
            }
        }

        // Remove rows for files that no longer exist (or are now gitignored), and write
        // meta, atomically: either both land or neither does.
        let present: std::collections::HashSet<&str> = seen
            .iter()
            .copied()
            .chain(w.skipped.iter().map(|s| s.rel_path.as_str()))
            .collect();
        let conn = self.store.conn();
        let tx = conn.unchecked_transaction()?;
        for (path, k) in &known {
            if !present.contains(path.as_str()) {
                delete_symbols_for(&tx, k.id)?;
                tx.execute("DELETE FROM files WHERE id = ?1", [k.id])?;
                stats.removed += 1;
            }
        }
        self.write_meta(now)?;
        tx.commit()?;
        Ok(stats)
    }

    fn upsert_skipped(
        &self,
        path: &str,
        mtime_ms: i64,
        size: u64,
        reason: &str,
        now: i64,
    ) -> Result<i64> {
        let conn = self.store.conn();
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO files(path, lang, content_hash, mtime_ms, size, indexed_at_ms, skipped_reason)
             VALUES (?1, NULL, NULL, ?2, ?3, ?4, ?5)
             ON CONFLICT(path) DO UPDATE SET lang = NULL, content_hash = NULL, mtime_ms = excluded.mtime_ms,
               size = excluded.size, indexed_at_ms = excluded.indexed_at_ms, skipped_reason = excluded.skipped_reason",
            params![path, mtime_ms, size as i64, now, reason],
        )?;
        let id: i64 = tx.query_row("SELECT id FROM files WHERE path = ?1", [path], |r| r.get(0))?;
        delete_symbols_for(&tx, id)?;
        tx.commit()?;
        Ok(id)
    }

    /// Parses, hashes and (re)writes one changed file's row, symbols, refs and FTS rows,
    /// all atomically. `Outcome::Unchanged` means the content hash matched what was already
    /// stored (only mtime/size moved) so nothing besides those two columns was touched;
    /// `Outcome::Skipped` means the file was written to `files` with a `skipped_reason` and
    /// has no symbols; `Outcome::Indexed` means symbols/refs/FTS rows were (re)written.
    fn index_file(&self, e: &WalkEntry, prior: Option<&Known>, now: i64) -> Result<Outcome> {
        if e.size > MAX_FILE_BYTES {
            self.upsert_skipped(&e.rel_path, e.mtime_ms, e.size, "too-large", now)?;
            return Ok(Outcome::Skipped);
        }
        let Some(lang) = Language::from_path(&e.rel_path) else {
            self.upsert_skipped(&e.rel_path, e.mtime_ms, e.size, "unsupported-language", now)?;
            return Ok(Outcome::Skipped);
        };
        let bytes = std::fs::read(&e.abs_path)?;
        if bytes.iter().take(8192).any(|&b| b == 0) {
            self.upsert_skipped(&e.rel_path, e.mtime_ms, e.size, "binary", now)?;
            return Ok(Outcome::Skipped);
        }
        let Ok(source) = std::str::from_utf8(&bytes) else {
            self.upsert_skipped(&e.rel_path, e.mtime_ms, e.size, "not-utf8", now)?;
            return Ok(Outcome::Skipped);
        };
        if looks_secret(source).is_some() {
            self.upsert_skipped(&e.rel_path, e.mtime_ms, e.size, "secret-like content", now)?;
            return Ok(Outcome::Skipped);
        }
        let hash = blake3::hash(&bytes).to_hex().to_string();
        let conn = self.store.conn();

        if prior.and_then(|p| p.content_hash.as_deref()) == Some(hash.as_str()) {
            let tx = conn.unchecked_transaction()?;
            tx.execute(
                "UPDATE files SET mtime_ms = ?2, size = ?3 WHERE path = ?1",
                params![e.rel_path, e.mtime_ms, e.size as i64],
            )?;
            tx.commit()?;
            return Ok(Outcome::Unchanged);
        }

        let tags = extract_tags(lang, source)?;
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO files(path, lang, content_hash, mtime_ms, size, indexed_at_ms, skipped_reason)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL)
             ON CONFLICT(path) DO UPDATE SET lang = excluded.lang, content_hash = excluded.content_hash,
               mtime_ms = excluded.mtime_ms, size = excluded.size, indexed_at_ms = excluded.indexed_at_ms, skipped_reason = NULL",
            params![e.rel_path, lang.as_str(), hash, e.mtime_ms, e.size as i64, now],
        )?;
        let file_id: i64 =
            tx.query_row("SELECT id FROM files WHERE path = ?1", [&e.rel_path], |r| {
                r.get(0)
            })?;
        delete_symbols_for(&tx, file_id)?;

        for t in &tags {
            if t.is_definition {
                tx.execute(
                    "INSERT INTO symbols(file_id, name, kind, line_start, line_end, signature) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![file_id, t.name, t.kind, t.line_start, t.line_end, t.signature],
                )?;
                let id = tx.last_insert_rowid();
                tx.execute(
                    "INSERT INTO symbols_fts(rowid, name, name_tokens, signature, path) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![id, t.name, split_identifier(&t.name).join(" "), t.signature, e.rel_path],
                )?;
            } else {
                tx.execute(
                    "INSERT INTO refs(file_id, name, line) VALUES (?1, ?2, ?3)",
                    params![file_id, t.name, t.line_start],
                )?;
            }
        }
        tx.commit()?;
        Ok(Outcome::Indexed)
    }

    fn write_meta(&self, now: i64) -> Result<()> {
        let mut stmt = self.store.conn().prepare(
            "SELECT path, content_hash FROM files WHERE content_hash IS NOT NULL ORDER BY path",
        )?;
        let mut hasher = blake3::Hasher::new();
        for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (p, h) = row?;
            hasher.update(p.as_bytes());
            hasher.update(b"\t");
            hasher.update(h.as_bytes());
            hasher.update(b"\n");
        }
        let version = hasher.finalize().to_hex()[..12].to_string();
        self.store.set_meta("index_version", &version)?;
        self.store.set_meta("indexed_at_ms", &now.to_string())?;
        match git_head(&self.root) {
            Some(h) => self.store.set_meta("git_head", &h)?,
            None => self.store.set_meta("git_head", "")?,
        }
        Ok(())
    }
}

/// Deletes a file's symbols, refs and FTS rows (but not the `files` row itself),
/// so the caller can either re-insert fresh ones or leave the file skipped.
fn delete_symbols_for(conn: &Connection, file_id: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM symbols_fts WHERE rowid IN (SELECT id FROM symbols WHERE file_id = ?1)",
        [file_id],
    )?;
    conn.execute("DELETE FROM symbols WHERE file_id = ?1", [file_id])?;
    conn.execute("DELETE FROM refs WHERE file_id = ?1", [file_id])?;
    Ok(())
}

/// Resolve `.git/HEAD` without a git dependency. Handles symbolic refs, loose refs
/// and `packed-refs`. Returns `None` when there is no readable repository.
pub fn git_head(root: &Path) -> Option<String> {
    let head = std::fs::read_to_string(root.join(".git/HEAD")).ok()?;
    let head = head.trim();
    if let Some(r) = head.strip_prefix("ref: ") {
        if let Ok(s) = std::fs::read_to_string(root.join(".git").join(r)) {
            return Some(s.trim().to_string());
        }
        let packed = std::fs::read_to_string(root.join(".git/packed-refs")).ok()?;
        return packed
            .lines()
            .filter(|l| !l.starts_with('#') && !l.starts_with('^'))
            .find_map(|l| {
                l.split_once(' ')
                    .filter(|(_, name)| *name == r)
                    .map(|(sha, _)| sha.to_string())
            });
    }
    if head.len() >= 40 && head.chars().all(|c| c.is_ascii_hexdigit()) {
        return Some(head.to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MapConfig;
    use crate::fixture::write_ts_mini;
    use crate::store::Store;

    fn setup() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        write_ts_mini(dir.path());
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        (dir, store)
    }

    fn count(store: &Store, sql: &str) -> i64 {
        store.conn().query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn full_index_records_files_symbols_refs_and_skips() {
        let (dir, store) = setup();
        let cfg = MapConfig::default();
        let ix = Indexer::new(&store, dir.path(), &cfg).unwrap();
        let stats = ix.refresh(None).unwrap();
        assert_eq!(stats.remaining, 0);
        assert!(stats.indexed >= 4, "{stats:?}");

        // .env denylisted, config.ts secret-like, README unsupported, dist/ absent.
        let reason = |p: &str| -> Option<String> {
            store
                .conn()
                .query_row(
                    "SELECT skipped_reason FROM files WHERE path = ?1",
                    [p],
                    |r| r.get(0),
                )
                .unwrap()
        };
        assert_eq!(reason(".env").as_deref(), Some("denylisted"));
        assert_eq!(
            reason("src/config.ts").as_deref(),
            Some("secret-like content")
        );
        assert_eq!(reason("README.md").as_deref(), Some("unsupported-language"));
        assert_eq!(reason("src/auth/session.ts"), None);
        assert_eq!(
            count(
                &store,
                "SELECT COUNT(*) FROM files WHERE path LIKE 'dist/%'"
            ),
            0
        );

        assert_eq!(
            count(
                &store,
                "SELECT COUNT(*) FROM symbols WHERE name = 'createSession' AND kind = 'function'"
            ),
            1
        );
        assert_eq!(
            count(&store, "SELECT COUNT(DISTINCT f.path) FROM refs r JOIN files f ON f.id = r.file_id WHERE r.name = 'createSession' AND f.path != 'src/auth/session.ts'"),
            2
        );
        assert_eq!(
            count(
                &store,
                "SELECT COUNT(*) FROM symbols_fts WHERE symbols_fts MATCH 'name_tokens:session'"
            ),
            5
        );
        assert!(store.get_meta("index_version").unwrap().is_some());
    }

    #[test]
    fn incremental_refresh_touches_only_changed_files() {
        let (dir, store) = setup();
        let cfg = MapConfig::default();
        let ix = Indexer::new(&store, dir.path(), &cfg).unwrap();
        ix.refresh(None).unwrap();
        let before: i64 = store
            .conn()
            .query_row(
                "SELECT indexed_at_ms FROM files WHERE path = 'src/util/log.ts'",
                [],
                |r| r.get(0),
            )
            .unwrap();

        std::thread::sleep(std::time::Duration::from_millis(20));
        let p = dir.path().join("src/cli/login.ts");
        std::fs::write(&p, "export function loginRenamed(): void {}\n").unwrap();
        // Force a distinct mtime even on coarse filesystems.
        let t = std::time::SystemTime::now() + std::time::Duration::from_secs(2);
        std::fs::File::open(&p).unwrap().set_modified(t).unwrap();

        let stats = ix.refresh(None).unwrap();
        assert_eq!(stats.indexed, 1, "{stats:?}");
        assert_eq!(
            count(&store, "SELECT COUNT(*) FROM symbols WHERE name = 'login'"),
            0
        );
        assert_eq!(
            count(
                &store,
                "SELECT COUNT(*) FROM symbols WHERE name = 'loginRenamed'"
            ),
            1
        );
        let after: i64 = store
            .conn()
            .query_row(
                "SELECT indexed_at_ms FROM files WHERE path = 'src/util/log.ts'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn same_content_new_mtime_is_unchanged_not_indexed() {
        let (dir, store) = setup();
        let cfg = MapConfig::default();
        let ix = Indexer::new(&store, dir.path(), &cfg).unwrap();
        ix.refresh(None).unwrap();

        let p = dir.path().join("src/util/log.ts");
        let content = std::fs::read_to_string(&p).unwrap();
        std::fs::write(&p, &content).unwrap();
        // Force a distinct mtime even on coarse filesystems; content is byte-identical.
        let t = std::time::SystemTime::now() + std::time::Duration::from_secs(2);
        std::fs::File::open(&p).unwrap().set_modified(t).unwrap();

        let stats = ix.refresh(None).unwrap();
        assert_eq!(stats.indexed, 0, "{stats:?}");
        assert!(stats.unchanged >= 1, "{stats:?}");
        assert_eq!(
            count(&store, "SELECT COUNT(*) FROM symbols WHERE name = 'log'"),
            1
        );
    }

    #[test]
    fn deleted_files_are_removed_with_their_symbols() {
        let (dir, store) = setup();
        let cfg = MapConfig::default();
        let ix = Indexer::new(&store, dir.path(), &cfg).unwrap();
        ix.refresh(None).unwrap();
        std::fs::remove_file(dir.path().join("src/util/log.ts")).unwrap();
        let stats = ix.refresh(None).unwrap();
        assert_eq!(stats.removed, 1);
        assert_eq!(
            count(&store, "SELECT COUNT(*) FROM symbols WHERE name = 'log'"),
            0
        );
        assert_eq!(
            count(
                &store,
                "SELECT COUNT(*) FROM symbols_fts WHERE symbols_fts MATCH 'name:log'"
            ),
            0
        );
    }

    #[test]
    fn expired_deadline_indexes_nothing_and_reports_remaining() {
        let (dir, store) = setup();
        let cfg = MapConfig::default();
        let ix = Indexer::new(&store, dir.path(), &cfg).unwrap();
        let stats = ix.refresh(Some(std::time::Instant::now())).unwrap();
        assert_eq!(stats.indexed, 0);
        assert!(stats.remaining >= 4, "{stats:?}");
        assert_eq!(ix.stale_count().unwrap(), stats.remaining);
    }

    #[test]
    fn git_head_reads_ref_or_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(git_head(dir.path()), None);
        std::fs::create_dir_all(dir.path().join(".git/refs/heads")).unwrap();
        std::fs::write(dir.path().join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(
            dir.path().join(".git/refs/heads/main"),
            "0123456789abcdef0123456789abcdef01234567\n",
        )
        .unwrap();
        assert_eq!(
            git_head(dir.path()).as_deref(),
            Some("0123456789abcdef0123456789abcdef01234567")
        );
        std::fs::write(
            dir.path().join(".git/HEAD"),
            "fedcba9876543210fedcba9876543210fedcba98\n",
        )
        .unwrap();
        assert_eq!(
            git_head(dir.path()).as_deref(),
            Some("fedcba9876543210fedcba9876543210fedcba98")
        );
    }
}
