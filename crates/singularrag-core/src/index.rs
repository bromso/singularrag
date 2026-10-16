//! Indexer: walk, diff against the `files` table, parse changed files, write symbols,
//! refs and FTS rows. Runs until an optional deadline; whatever is left is reported
//! as `remaining` so callers can say "STALE: N files changed since index".

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use rusqlite::{params, Connection};
use serde::Serialize;

use crate::config::MapConfig;
use crate::lang::{extract_tags, Language};
use crate::secrets::looks_secret;
use crate::store::Store;
use crate::time::now_ms;
use crate::tokens::split_identifier;
use crate::walk::{walk_workspace, WalkEntry};
use crate::workspace::Workspace;
use crate::{Error, Result};

pub const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// Alias → table name (schema spec §3): every table name, plus a Prisma model's name and
/// its lower-camel form. Rebuilt from `table_names` after the schema files of a run.
pub type Vocab = HashMap<String, String>;

pub fn load_vocab(conn: &Connection) -> Result<Vocab> {
    let mut stmt =
        conn.prepare("SELECT alias, table_name FROM table_names ORDER BY alias, table_name")?;
    let mut v = Vocab::new();
    for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (alias, table) = row?;
        v.entry(alias).or_insert(table);
    }
    Ok(v)
}

/// A stable digest of the vocabulary, stamped onto each code file's `files.vocab` row
/// (spec §3) as it's indexed; a code file whose stamp differs from the vocabulary just
/// loaded is re-indexed once, content hash or not.
pub fn vocab_hash(v: &Vocab) -> String {
    let mut pairs: Vec<(&String, &String)> = v.iter().collect();
    pairs.sort();
    let mut h = blake3::Hasher::new();
    for (a, t) in pairs {
        h.update(a.as_bytes());
        h.update(b"\t");
        h.update(t.as_bytes());
        h.update(b"\n");
    }
    h.finalize().to_hex()[..12].to_string()
}

fn is_schema_path(p: &str) -> bool {
    Language::from_path(p).is_some_and(|l| l.is_schema())
}

fn is_code_path(p: &str) -> bool {
    Language::from_path(p).is_some_and(|l| !l.is_schema() && !l.is_document())
}

#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub struct IndexStats {
    pub scanned: usize,
    pub indexed: usize,
    pub unchanged: usize,
    pub skipped: usize,
    pub removed: usize,
    pub remaining: usize,
    /// True when the advisory lock was held by another live process for longer than the
    /// caller was willing to wait, so nothing was indexed and `remaining` is the backlog.
    pub lock_timeout: bool,
}

pub struct Indexer<'a> {
    store: &'a Store,
    ws: Workspace,
    config: &'a MapConfig,
}

#[derive(Clone)]
struct Known {
    id: i64,
    mtime_ms: i64,
    size: u64,
    content_hash: Option<String>,
    /// The `vocab_hash` of the table-name vocabulary this file was last indexed under
    /// (spec §3); `None` for a skipped file or one indexed before this column existed.
    vocab: Option<String>,
}

/// What `index_file` did with one changed file.
enum Outcome {
    Indexed,
    Unchanged,
    Skipped,
}

impl<'a> Indexer<'a> {
    pub fn new(store: &'a Store, ws: &Workspace, config: &'a MapConfig) -> Result<Self> {
        Ok(Indexer {
            store,
            ws: ws.clone(),
            config,
        })
    }

    fn known_files(&self) -> Result<HashMap<String, Known>> {
        let mut stmt = self
            .store
            .conn()
            .prepare("SELECT id, path, mtime_ms, size, content_hash, vocab FROM files")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(1)?,
                Known {
                    id: r.get(0)?,
                    mtime_ms: r.get(2)?,
                    size: r.get::<_, i64>(3)? as u64,
                    content_hash: r.get(4)?,
                    vocab: r.get(5)?,
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
        let w = walk_workspace(&self.ws, &self.config.deny_patterns())?;
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
        self.refresh_with(deadline, || {})
    }

    /// Same as `refresh`, but calls `on_file` once after every entry in the walk's
    /// entry list is processed (indexed, unchanged, skipped-by-content or cut off by
    /// the deadline). Callers use this to send a lock heartbeat during long refreshes.
    pub fn refresh_with(
        &self,
        deadline: Option<Instant>,
        mut on_file: impl FnMut(),
    ) -> Result<IndexStats> {
        let mut stats = IndexStats::default();
        let known = self.known_files()?;
        let w = walk_workspace(&self.ws, &self.config.deny_patterns())?;
        stats.scanned = w.entries.len() + w.skipped.len();
        let now = now_ms();

        // Skipped-by-walk files become rows with a reason and no symbols.
        for s in &w.skipped {
            self.upsert_skipped(&s.rel_path, 0, 0, s.reason, now)?;
            stats.skipped += 1;
        }

        // Schema files first (spec §3): the vocabulary the code pass needs comes from them.
        // Then everything else: a code file whose own `files.vocab` stamp doesn't match the
        // vocabulary just loaded is re-indexed once, content hash or not. The stamp lives on
        // the file's own row rather than in a run-level meta, so a deadline cutoff mid-run
        // never loses track of which files still need the new vocabulary: whatever got
        // stamped stays stamped, and whatever didn't is still behind next time, with no
        // window where every code file looks forced just because no run has finished yet.
        let (schema, rest): (Vec<&WalkEntry>, Vec<&WalkEntry>) =
            w.entries.iter().partition(|e| is_schema_path(&e.rel_path));
        let mut seen: Vec<&str> = Vec::with_capacity(w.entries.len());
        let empty = Vocab::new();
        let empty_hash = vocab_hash(&empty);
        for e in schema {
            seen.push(&e.rel_path);
            self.process(e, &known, &empty, &empty_hash, deadline, now, &mut stats)?;
            on_file();
        }
        let vocab = load_vocab(self.store.conn())?;
        let hash = vocab_hash(&vocab);
        for e in rest {
            seen.push(&e.rel_path);
            self.process(e, &known, &vocab, &hash, deadline, now, &mut stats)?;
            on_file();
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

    #[allow(clippy::too_many_arguments)]
    fn process(
        &self,
        e: &WalkEntry,
        known: &HashMap<String, Known>,
        vocab: &Vocab,
        hash: &str,
        deadline: Option<Instant>,
        now: i64,
        stats: &mut IndexStats,
    ) -> Result<()> {
        let prior = known.get(&e.rel_path);
        // Only a code file's own stamp can force it, and only when it was actually indexed
        // before (`content_hash` is set): a skipped file (secret-like, binary, too-large,
        // parse-error, unreadable) has no stamp to be stale, and forcing it anyway would
        // re-read and re-scan it on every single run forever, inflating `skipped` and, under
        // a deadline, `remaining`/STALE on a corpus where nothing changed. A skipped file that
        // later becomes indexable changes content, so the ordinary mtime/size check catches
        // it. Documents and schema files are only ever reindexed by that same check (a schema
        // file's vocabulary stamp is irrelevant and never compared).
        let force = is_code_path(&e.rel_path)
            && prior.is_some_and(|p| p.content_hash.is_some() && p.vocab.as_deref() != Some(hash));
        if !force && !Self::changed(known, e) {
            stats.unchanged += 1;
            return Ok(());
        }
        if deadline.is_some_and(|d| Instant::now() >= d) {
            stats.remaining += 1;
            return Ok(());
        }
        // One unreadable or unparseable file must never abort the refresh: it becomes
        // a skip row like any other skipped file, and the walk carries on. Only store
        // errors (which mean the index itself is unusable) propagate.
        match self.index_file(e, prior, now, vocab, hash, force) {
            Ok(Outcome::Indexed) => stats.indexed += 1,
            Ok(Outcome::Unchanged) => stats.unchanged += 1,
            Ok(Outcome::Skipped) => stats.skipped += 1,
            Err(Error::Io(_)) => {
                self.upsert_skipped(&e.rel_path, e.mtime_ms, e.size, "unreadable", now)?;
                stats.skipped += 1;
            }
            Err(Error::Tags(_)) => {
                self.upsert_skipped(&e.rel_path, e.mtime_ms, e.size, "parse-error", now)?;
                stats.skipped += 1;
            }
            Err(other) => return Err(other),
        }
        Ok(())
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
            "INSERT INTO files(path, lang, content_hash, mtime_ms, size, indexed_at_ms, skipped_reason, vocab)
             VALUES (?1, NULL, NULL, ?2, ?3, ?4, ?5, NULL)
             ON CONFLICT(path) DO UPDATE SET lang = NULL, content_hash = NULL, mtime_ms = excluded.mtime_ms,
               size = excluded.size, indexed_at_ms = excluded.indexed_at_ms, skipped_reason = excluded.skipped_reason,
               vocab = NULL",
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
    fn index_file(
        &self,
        e: &WalkEntry,
        prior: Option<&Known>,
        now: i64,
        vocab: &Vocab,
        vocab_tag: &str,
        force: bool,
    ) -> Result<Outcome> {
        if e.size > MAX_FILE_BYTES {
            self.upsert_skipped(&e.rel_path, e.mtime_ms, e.size, "too-large", now)?;
            return Ok(Outcome::Skipped);
        }
        let base = e.rel_path.rsplit('/').next().unwrap_or(&e.rel_path);
        if crate::doc::is_lockfile(base) {
            self.upsert_skipped(&e.rel_path, e.mtime_ms, e.size, "lockfile", now)?;
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
        if lang.is_document() {
            if let Some(reason) = crate::doc::skip_reason(&e.rel_path, e.size, source) {
                self.upsert_skipped(&e.rel_path, e.mtime_ms, e.size, reason, now)?;
                return Ok(Outcome::Skipped);
            }
        }
        if looks_secret(source).is_some() {
            self.upsert_skipped(&e.rel_path, e.mtime_ms, e.size, "secret-like content", now)?;
            return Ok(Outcome::Skipped);
        }
        let hash = blake3::hash(&bytes).to_hex().to_string();
        let conn = self.store.conn();

        if !force && prior.and_then(|p| p.content_hash.as_deref()) == Some(hash.as_str()) {
            let tx = conn.unchecked_transaction()?;
            tx.execute(
                "UPDATE files SET mtime_ms = ?2, size = ?3 WHERE path = ?1",
                params![e.rel_path, e.mtime_ms, e.size as i64],
            )?;
            tx.commit()?;
            return Ok(Outcome::Unchanged);
        }

        let stem = e.rel_path.rsplit('/').next().unwrap_or(&e.rel_path);
        let stem = stem
            .rsplit_once('.')
            .map(|(s, _)| s)
            .unwrap_or(stem)
            .to_string();
        let (tags, bodies, mentions, aliases) = if lang.is_document() {
            let d = crate::doc::extract(lang, source, &stem)?;
            (d.tags, d.bodies, d.mentions, Vec::new())
        } else if lang.is_schema() {
            let s = crate::schema::extract(lang, source)?;
            (s.tags, Vec::new(), Vec::new(), s.aliases)
        } else {
            let _ = vocab; // Task 4 turns the vocabulary into table mentions here
            (
                extract_tags(lang, source)?,
                Vec::new(),
                Vec::new(),
                Vec::new(),
            )
        };
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO files(path, lang, content_hash, mtime_ms, size, indexed_at_ms, skipped_reason, vocab)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7)
             ON CONFLICT(path) DO UPDATE SET lang = excluded.lang, content_hash = excluded.content_hash,
               mtime_ms = excluded.mtime_ms, size = excluded.size, indexed_at_ms = excluded.indexed_at_ms, skipped_reason = NULL,
               vocab = excluded.vocab",
            params![
                e.rel_path,
                lang.as_str(),
                hash,
                e.mtime_ms,
                e.size as i64,
                now,
                vocab_tag
            ],
        )?;
        let file_id: i64 =
            tx.query_row("SELECT id FROM files WHERE path = ?1", [&e.rel_path], |r| {
                r.get(0)
            })?;
        delete_symbols_for(&tx, file_id)?;

        for (alias, table) in &aliases {
            tx.execute(
                "INSERT OR REPLACE INTO table_names(alias, table_name, file_id) VALUES (?1, ?2, ?3)",
                params![alias, table, file_id],
            )?;
        }

        let mut ids: Vec<i64> = Vec::with_capacity(tags.len());
        for t in &tags {
            if t.name.is_empty() {
                ids.push(0);
                continue;
            }
            if t.is_definition {
                tx.execute(
                    "INSERT INTO symbols(file_id, name, kind, line_start, line_end, signature) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![file_id, t.name, t.kind, t.line_start, t.line_end, t.signature],
                )?;
                let id = tx.last_insert_rowid();
                ids.push(id);
                tx.execute(
                    "INSERT INTO symbols_fts(rowid, name, name_tokens, signature, path) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![id, t.name, split_identifier(&t.name).join(" "), t.signature, e.rel_path],
                )?;
            } else {
                ids.push(0);
                tx.execute(
                    "INSERT INTO refs(file_id, name, line) VALUES (?1, ?2, ?3)",
                    params![file_id, t.name, t.line_start],
                )?;
            }
        }
        for (i, body) in &bodies {
            let id = ids[*i];
            if id == 0 || !matches!(tags[*i].kind.as_str(), "section" | "document" | "element") {
                continue;
            }
            // A wrapper whose whole content lives in child sections (e.g. the
            // document row of a file that opens straight into an H1, or a section
            // that holds nothing but a nested heading) blanks down to whitespace;
            // skip it rather than writing a noise row.
            if body.trim().is_empty() {
                continue;
            }
            tx.execute(
                "INSERT INTO sections_fts(rowid, path, name, content) VALUES (?1, ?2, ?3, ?4)",
                params![id, e.rel_path, tags[*i].name, body],
            )?;
            crate::knowledge::queue_section(&tx, id, body)?;
        }
        for (name, line) in &mentions {
            tx.execute(
                "INSERT INTO refs(file_id, name, line) VALUES (?1, ?2, ?3)",
                params![file_id, name, line],
            )?;
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
        if self.ws.is_named() {
            let mut parts = Vec::new();
            for r in &self.ws.roots {
                let head = git_head(&r.path).unwrap_or_default();
                self.store
                    .set_meta(&format!("git_head:{}", r.name), &head)?;
                let short = if head.is_empty() {
                    "none".to_string()
                } else {
                    head.chars().take(7).collect()
                };
                parts.push(format!("{}:{short}", r.name));
            }
            self.store.set_meta("git_head", &parts.join(" "))?;
        } else {
            match git_head(&self.ws.roots[0].path) {
                Some(h) => self.store.set_meta("git_head", &h)?,
                None => self.store.set_meta("git_head", "")?,
            }
        }
        Ok(())
    }
}

/// Deletes a file's symbols, refs, FTS rows and knowledge rows (but not the `files` row
/// itself), so the caller can either re-insert fresh ones or leave the file skipped.
fn delete_symbols_for(conn: &Connection, file_id: i64) -> Result<()> {
    crate::knowledge::delete_for_symbols(conn, file_id)?;
    conn.execute("DELETE FROM table_names WHERE file_id = ?1", [file_id])?;
    conn.execute(
        "DELETE FROM symbols_fts WHERE rowid IN (SELECT id FROM symbols WHERE file_id = ?1)",
        [file_id],
    )?;
    conn.execute(
        "DELETE FROM sections_fts WHERE rowid IN (SELECT id FROM symbols WHERE file_id = ?1)",
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
    use crate::fixture::{write_rust_mini, write_schema_mini, write_ts_mini};
    use crate::store::Store;
    use crate::workspace::Workspace;

    fn setup() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        write_ts_mini(dir.path());
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        (dir, store)
    }

    fn count(store: &Store, sql: &str) -> i64 {
        store.conn().query_row(sql, [], |r| r.get(0)).unwrap()
    }

    fn index_dir(dir: &Path) -> Store {
        Store::open(&dir.join(".singularrag/index.db")).unwrap()
    }

    fn refresh(store: &Store, dir: &Path) -> IndexStats {
        let cfg = MapConfig::default();
        Indexer::new(store, &Workspace::single(dir).unwrap(), &cfg)
            .unwrap()
            .refresh(None)
            .unwrap()
    }

    fn symbols_of(store: &Store, path: &str, kind: &str) -> Vec<String> {
        let mut stmt = store
            .conn()
            .prepare("SELECT s.name FROM symbols s JOIN files f ON f.id = s.file_id WHERE f.path = ?1 AND s.kind = ?2 ORDER BY s.line_start")
            .unwrap();
        stmt.query_map([path, kind], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    }

    fn refs_named(store: &Store, name: &str) -> Vec<String> {
        let mut stmt = store
            .conn()
            .prepare("SELECT DISTINCT f.path FROM refs r JOIN files f ON f.id = r.file_id WHERE r.name = ?1 ORDER BY f.path")
            .unwrap();
        stmt.query_map([name], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    }

    #[test]
    fn schema_files_index_tables_columns_foreign_keys_and_aliases() {
        let dir = tempfile::tempdir().unwrap();
        write_schema_mini(dir.path());
        let store = index_dir(dir.path());
        let stats = refresh(&store, dir.path());
        assert_eq!(stats.remaining, 0);
        assert_eq!(
            symbols_of(&store, "prisma/schema.prisma", "table"),
            vec!["user", "website", "website_event"]
        );
        assert!(symbols_of(&store, "prisma/schema.prisma", "column")
            .contains(&"website_event.website_id".to_string()));
        assert_eq!(
            symbols_of(
                &store,
                "prisma/migrations/02_add_event_name/migration.sql",
                "column"
            ),
            vec!["website_event.event_name"]
        );
        assert_eq!(symbols_of(&store, "db/dump.sql", "table"), vec!["report"]);
        // foreign keys: the schema, the init migration and the dump all reference `website`
        // (`contains`, not equality: Task 4 adds the code file to this list)
        let website_refs = refs_named(&store, "website");
        for p in [
            "db/dump.sql",
            "prisma/migrations/01_init/migration.sql",
            "prisma/schema.prisma",
        ] {
            assert!(
                website_refs.contains(&p.to_string()),
                "{p} missing from {website_refs:?}"
            );
        }
        let lang: String = store
            .conn()
            .query_row(
                "SELECT lang FROM files WHERE path = 'prisma/schema.prisma'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(lang, "prisma");
        let vocab = load_vocab(store.conn()).unwrap();
        assert_eq!(
            vocab.get("websiteEvent").map(String::as_str),
            Some("website_event")
        );
        assert_eq!(
            vocab.get("WebsiteEvent").map(String::as_str),
            Some("website_event")
        );
        assert_eq!(vocab.get("report").map(String::as_str), Some("report"));
        assert_eq!(
            count(&store, "SELECT COUNT(*) FROM extract_queue q JOIN symbols s ON s.id = q.symbol_id WHERE s.kind IN ('table', 'column')"),
            0,
            "a table is never queued for extraction"
        );
        assert!(
            count(&store, "SELECT COUNT(*) FROM extract_queue") >= 1,
            "the document section is queued"
        );
    }

    #[test]
    fn a_vocabulary_change_reindexes_code_files_once() {
        let dir = tempfile::tempdir().unwrap();
        write_schema_mini(dir.path());
        std::fs::write(
            dir.path().join("src/queries/team.ts"),
            "import { prisma } from \"../lib/prisma\";\nexport function getTeams() {\n  return prisma.client.team.findUnique({});\n}\n",
        )
        .unwrap();
        let store = index_dir(dir.path());
        refresh(&store, dir.path());
        assert!(
            refs_named(&store, "team").is_empty(),
            "no `team` table yet, so no ref"
        );
        let second = refresh(&store, dir.path());
        assert_eq!(second.indexed, 0, "nothing changed, nothing re-indexed");

        // a new model appears; the code files' mtimes do not change
        let schema = dir.path().join("prisma/schema.prisma");
        let mut src = std::fs::read_to_string(&schema).unwrap();
        src.push_str("\nmodel Team {\n  id String @id\n\n  @@map(\"team\")\n}\n");
        std::fs::write(&schema, src).unwrap();
        let third = refresh(&store, dir.path());
        // `team.ts` is reindexed under the new vocabulary, but table mentions (Task 4's
        // `schema::table_mentions` call) are not wired into `index_file` yet, so no ref
        // is written from its `prisma.client.team` use.
        assert!(
            refs_named(&store, "team").is_empty(),
            "table mentions land in Task 4"
        );
        // the schema file plus every code file (website.ts, prisma.ts, db.ts, team.ts), not the documents
        assert_eq!(third.indexed, 5, "{third:?}");
        let fourth = refresh(&store, dir.path());
        assert_eq!(
            fourth.indexed, 0,
            "the forced re-index happens once: {fourth:?}"
        );
    }

    #[test]
    fn an_empty_vocabulary_forces_nothing_and_emits_nothing() {
        let (dir, store) = setup();
        let first = refresh(&store, dir.path());
        assert!(first.indexed > 0);
        assert_eq!(count(&store, "SELECT COUNT(*) FROM table_names"), 0);
        let empty_hash = vocab_hash(&Vocab::new());
        let mut stmt = store
            .conn()
            .prepare("SELECT path, vocab FROM files WHERE skipped_reason IS NULL")
            .unwrap();
        let rows: Vec<(String, Option<String>)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        for (path, vocab) in &rows {
            if is_code_path(path) {
                assert_eq!(vocab.as_deref(), Some(empty_hash.as_str()), "{path}");
            }
        }
        let second = refresh(&store, dir.path());
        assert_eq!(second.indexed, 0, "{second:?}");
        // Only the walk-level `.env` denylist row: `src/config.ts` (secret-like, so never
        // indexed and never stamped) must not be force-rechecked every run just because its
        // `vocab` column is NULL — it now counts as unchanged, like any other skipped file
        // whose mtime/size haven't moved.
        assert_eq!(
            second.skipped, 1,
            "only the walk-level .env denylist row: {second:?}"
        );
        // An already-expired deadline must not make an unchanged corpus look STALE: there is
        // nothing left for the deadline check to even see, because every file either hit the
        // walk-level skip (unconditional, not deadline-gated) or the cheap unchanged shortcut.
        let third = Indexer::new(
            &store,
            &Workspace::single(dir.path()).unwrap(),
            &MapConfig::default(),
        )
        .unwrap()
        .refresh(Some(std::time::Instant::now()))
        .unwrap();
        assert_eq!(third.remaining, 0, "{third:?}");
    }

    #[test]
    fn removing_a_schema_file_removes_its_aliases() {
        let dir = tempfile::tempdir().unwrap();
        write_schema_mini(dir.path());
        let store = index_dir(dir.path());
        refresh(&store, dir.path());
        assert!(load_vocab(store.conn()).unwrap().contains_key("report"));
        std::fs::remove_file(dir.path().join("db/dump.sql")).unwrap();
        refresh(&store, dir.path());
        assert!(!load_vocab(store.conn()).unwrap().contains_key("report"));
        assert!(load_vocab(store.conn()).unwrap().contains_key("website"));
    }

    /// A deadline that expires partway through phase 2 (the code/document pass, after the
    /// vocabulary has already changed) must not corrupt or lose track of which code files
    /// still carry the old stamp: whatever the cut run didn't reach keeps its old stamp and
    /// is still owed a reindex; a later uninterrupted run finishes the job exactly once per
    /// file, no more and no less.
    #[test]
    fn a_deadline_mid_run_leaves_no_code_file_with_a_stale_stamp() {
        let dir = tempfile::tempdir().unwrap();
        write_schema_mini(dir.path());
        std::fs::write(
            dir.path().join("src/queries/team.ts"),
            "import { prisma } from \"../lib/prisma\";\nexport function getTeams() {\n  return prisma.client.team.findUnique({});\n}\n",
        )
        .unwrap();
        let store = index_dir(dir.path());
        refresh(&store, dir.path());
        let old_hash = vocab_hash(&load_vocab(store.conn()).unwrap());

        let schema = dir.path().join("prisma/schema.prisma");
        let mut src = std::fs::read_to_string(&schema).unwrap();
        src.push_str("\nmodel Team {\n  id String @id\n\n  @@map(\"team\")\n}\n");
        std::fs::write(&schema, src).unwrap();

        let cfg = MapConfig::default();
        let ix = Indexer::new(&store, &Workspace::single(dir.path()).unwrap(), &cfg).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(300);
        let mut calls = 0usize;
        let cut = ix
            .refresh_with(Some(deadline), || {
                calls += 1;
                // The fixture has one Prisma file and three SQL files: once all four
                // schema entries have been processed (and the vocabulary has changed),
                // sleep past the deadline so the cut lands inside phase 2, not before
                // the vocabulary even changes.
                if calls == 4 {
                    std::thread::sleep(std::time::Duration::from_millis(400));
                }
            })
            .unwrap();
        assert!(cut.remaining > 0, "{cut:?}");

        let code_stamps = |store: &Store| -> Vec<(String, Option<String>)> {
            let mut stmt = store
                .conn()
                .prepare("SELECT path, vocab FROM files WHERE skipped_reason IS NULL")
                .unwrap();
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .map(|r| r.unwrap())
                .filter(|(path, _): &(String, Option<String>)| is_code_path(path))
                .collect()
        };
        assert!(
            code_stamps(&store)
                .iter()
                .any(|(_, vocab)| vocab.as_deref() == Some(old_hash.as_str())),
            "at least one code file must still carry the old stamp: {:?}",
            code_stamps(&store)
        );

        let caught_up = refresh(&store, dir.path());
        assert_eq!(caught_up.remaining, 0, "{caught_up:?}");
        assert_eq!(
            cut.indexed + caught_up.indexed,
            5,
            "the schema file plus four code files, split across the two runs: cut={cut:?} caught_up={caught_up:?}"
        );

        let new_hash = vocab_hash(&load_vocab(store.conn()).unwrap());
        assert_ne!(new_hash, old_hash);
        for (path, vocab) in code_stamps(&store) {
            assert_eq!(vocab.as_deref(), Some(new_hash.as_str()), "{path}");
        }

        let fourth = refresh(&store, dir.path());
        assert_eq!(fourth.indexed, 0, "{fourth:?}");
    }

    #[test]
    fn full_index_records_files_symbols_refs_and_skips() {
        let (dir, store) = setup();
        let cfg = MapConfig::default();
        let ix = Indexer::new(&store, &Workspace::single(dir.path()).unwrap(), &cfg).unwrap();
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
        assert_eq!(reason("README.md"), None, "Markdown is indexed");
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

    /// A file the process cannot read (or that fails to parse) must not abort the
    /// whole refresh: it becomes a skip row and every later file still indexes.
    #[cfg(unix)]
    #[test]
    fn unreadable_file_becomes_a_skip_row_without_aborting() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, store) = setup();
        let bad = dir.path().join("src/aaa_unreadable.ts");
        std::fs::write(&bad, "export function nope(): void {}\n").unwrap();
        std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read(&bad).is_ok() {
            return; // running as root: the file is readable anyway
        }
        let cfg = MapConfig::default();
        let ix = Indexer::new(&store, &Workspace::single(dir.path()).unwrap(), &cfg).unwrap();
        let stats = ix.refresh(None).unwrap();
        let reason: Option<String> = store
            .conn()
            .query_row(
                "SELECT skipped_reason FROM files WHERE path = ?1",
                ["src/aaa_unreadable.ts"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(reason.as_deref(), Some("unreadable"));
        // Files sorted after the unreadable one still made it in.
        assert!(stats.indexed >= 4, "{stats:?}");
        assert_eq!(
            count(
                &store,
                "SELECT COUNT(*) FROM symbols WHERE name = 'createSession'"
            ),
            1
        );
        std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o644)).unwrap();
    }

    /// The mirror of `full_index_records_files_symbols_refs_and_skips` for Rust, which
    /// was never exercised end to end (`write_rust_mini` was dead code).
    #[test]
    fn full_index_of_a_rust_repo_records_files_symbols_refs_and_skips() {
        let dir = tempfile::tempdir().unwrap();
        write_rust_mini(dir.path());
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        let cfg = MapConfig::default();
        let ix = Indexer::new(&store, &Workspace::single(dir.path()).unwrap(), &cfg).unwrap();
        let stats = ix.refresh(None).unwrap();
        assert_eq!(stats.indexed, 3, "two .rs files and Cargo.toml: {stats:?}");
        assert_eq!(stats.remaining, 0);

        let reason: Option<String> = store
            .conn()
            .query_row(
                "SELECT skipped_reason FROM files WHERE path = ?1",
                ["Cargo.toml"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(reason, None, "TOML is indexed");
        assert_eq!(
            count(
                &store,
                "SELECT COUNT(*) FROM files WHERE lang = 'rust' AND skipped_reason IS NULL"
            ),
            2
        );
        assert_eq!(
            count(
                &store,
                "SELECT COUNT(*) FROM symbols WHERE name = 'parse' AND kind = 'function'"
            ),
            1
        );
        assert_eq!(
            count(&store, "SELECT COUNT(*) FROM symbols WHERE name = 'helper'"),
            1
        );
        // `helper(s)` from lib.rs and the scoped `mini::parse(..)` call from main.rs.
        assert_eq!(
            count(&store, "SELECT COUNT(*) FROM refs r JOIN files f ON f.id = r.file_id WHERE r.name = 'parse' AND f.path = 'src/main.rs'"),
            1
        );
        assert_eq!(
            count(&store, "SELECT COUNT(*) FROM refs WHERE name = 'helper'"),
            1
        );
        let sig: String = store
            .conn()
            .query_row(
                "SELECT signature FROM symbols WHERE name = 'parse'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(sig, "pub fn parse(s: &str) -> u32");
        assert!(store.get_meta("index_version").unwrap().is_some());
    }

    #[test]
    fn incremental_refresh_touches_only_changed_files() {
        let (dir, store) = setup();
        let cfg = MapConfig::default();
        let ix = Indexer::new(&store, &Workspace::single(dir.path()).unwrap(), &cfg).unwrap();
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
        let ix = Indexer::new(&store, &Workspace::single(dir.path()).unwrap(), &cfg).unwrap();
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
        let ix = Indexer::new(&store, &Workspace::single(dir.path()).unwrap(), &cfg).unwrap();
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
        let ix = Indexer::new(&store, &Workspace::single(dir.path()).unwrap(), &cfg).unwrap();
        let stats = ix.refresh(Some(std::time::Instant::now())).unwrap();
        assert_eq!(stats.indexed, 0);
        assert!(stats.remaining >= 4, "{stats:?}");
        assert_eq!(ix.stale_count().unwrap(), stats.remaining);
    }

    #[test]
    fn refresh_with_calls_on_file_once_per_walked_entry() {
        let (dir, store) = setup();
        let cfg = MapConfig::default();
        let ix = Indexer::new(&store, &Workspace::single(dir.path()).unwrap(), &cfg).unwrap();
        let w =
            crate::walk::walk(&dir.path().canonicalize().unwrap(), &cfg.deny_patterns()).unwrap();
        let mut count = 0usize;
        let stats = ix.refresh_with(None, || count += 1).unwrap();
        assert_eq!(count, w.entries.len());
        assert_eq!(count, stats.scanned - w.skipped.len());
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

    /// A packed repository has no loose `refs/heads/<branch>` file; HEAD then resolves
    /// through `.git/packed-refs`.
    #[test]
    fn git_head_resolves_through_packed_refs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(
            dir.path().join(".git/packed-refs"),
            "# pack-refs with: peeled fully-peeled sorted\n\
             1111111111111111111111111111111111111111 refs/heads/other\n\
             2222222222222222222222222222222222222222 refs/heads/main\n\
             ^3333333333333333333333333333333333333333\n",
        )
        .unwrap();
        assert_eq!(
            git_head(dir.path()).as_deref(),
            Some("2222222222222222222222222222222222222222")
        );
        // A loose ref still wins over packed-refs.
        std::fs::create_dir_all(dir.path().join(".git/refs/heads")).unwrap();
        std::fs::write(
            dir.path().join(".git/refs/heads/main"),
            "4444444444444444444444444444444444444444\n",
        )
        .unwrap();
        assert_eq!(
            git_head(dir.path()).as_deref(),
            Some("4444444444444444444444444444444444444444")
        );
    }

    #[test]
    fn documents_become_sections_bodies_mentions_and_skips() {
        let dir = tempfile::tempdir().unwrap();
        crate::fixture::write_docs_mini(dir.path());
        // A lockfile no `Language` claims still skips as a lockfile.
        std::fs::write(dir.path().join("yarn.lock"), "# yarn lockfile v1\n").unwrap();
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        let ws = Workspace::single(dir.path()).unwrap();
        Indexer::new(&store, &ws, &MapConfig::default())
            .unwrap()
            .refresh(None)
            .unwrap();
        let conn = store.conn();
        let kind = |path: &str, name: &str| -> Option<String> {
            conn.query_row(
                "SELECT s.kind FROM symbols s JOIN files f ON f.id = s.file_id WHERE f.path = ?1 AND s.name = ?2",
                [path, name], |r| r.get(0)).ok()
        };
        assert_eq!(
            kind("docs/design.md", "Freshness").as_deref(),
            Some("section")
        );
        assert_eq!(
            kind("docs/design.md", "design").as_deref(),
            Some("document")
        );
        assert_eq!(kind("site/index.html", "app").as_deref(), Some("element"));
        assert_eq!(kind("site/app.css", ".login").as_deref(), Some("rule"));
        assert_eq!(
            kind("package.json", "scripts.build").as_deref(),
            Some("key")
        );
        assert_eq!(kind("ci.yml", "jobs.build").as_deref(), Some("key"));
        assert_eq!(
            kind("Cargo.toml", "dependencies.serde").as_deref(),
            Some("key")
        );
        assert_eq!(kind("notes.txt", "notes").as_deref(), Some("document"));
        let lang: String = conn
            .query_row(
                "SELECT lang FROM files WHERE path = 'docs/design.md'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(lang, "markdown");
        let body_hits: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sections_fts WHERE sections_fts MATCH 'stale'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(body_hits >= 1, "design.md's Freshness prose: {body_hits}");
        let no_values: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM symbols_fts WHERE symbols_fts MATCH 'tsc'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(no_values, 0);
        let mentions: Vec<String> = {
            let mut st = conn.prepare("SELECT r.name FROM refs r JOIN files f ON f.id = r.file_id WHERE f.path = 'docs/design.md' ORDER BY r.name").unwrap();
            st.query_map([], |r| r.get(0))
                .unwrap()
                .collect::<std::result::Result<_, _>>()
                .unwrap()
        };
        assert_eq!(
            mentions,
            vec!["SessionStore", "createSession", "runbook", "session"]
        );
        let skipped = |path: &str| -> Option<String> {
            conn.query_row(
                "SELECT skipped_reason FROM files WHERE path = ?1",
                [path],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(skipped("package-lock.json").as_deref(), Some("lockfile"));
        assert_eq!(skipped("yarn.lock").as_deref(), Some("lockfile"));
        assert_eq!(skipped("big.min.css").as_deref(), Some("minified"));
        assert_eq!(
            skipped("secrets.json").as_deref(),
            Some("secret-like content")
        );
        let secrets_symbols: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM symbols s JOIN files f ON f.id = s.file_id WHERE f.path = 'secrets.json'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(secrets_symbols, 0, "a secret-like file is skipped whole");
    }

    #[test]
    fn reindexing_a_document_replaces_its_fts_rows() {
        let dir = tempfile::tempdir().unwrap();
        crate::fixture::write_docs_mini(dir.path());
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        let ws = Workspace::single(dir.path()).unwrap();
        let cfg = MapConfig::default();
        Indexer::new(&store, &ws, &cfg)
            .unwrap()
            .refresh(None)
            .unwrap();
        std::fs::write(
            dir.path().join("docs/runbook.md"),
            "# Runbook\n\nNothing stale here any more.\n",
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        Indexer::new(&store, &ws, &cfg)
            .unwrap()
            .refresh(None)
            .unwrap();
        let n: i64 = store
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM sections_fts WHERE path = 'docs/runbook.md'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "one document row, the old section row is gone");
        std::fs::remove_file(dir.path().join("docs/runbook.md")).unwrap();
        Indexer::new(&store, &ws, &cfg)
            .unwrap()
            .refresh(None)
            .unwrap();
        let n: i64 = store
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM sections_fts WHERE path = 'docs/runbook.md'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn a_named_workspace_indexes_both_roots_and_records_a_head_per_root() {
        let d = tempfile::tempdir().unwrap();
        let (app, _notes) = crate::fixture::write_workspace(d.path());
        // Make `app` a git checkout so it has a head; `notes` stays plain.
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
        let ws = Workspace::open(d.path()).unwrap();
        let store = Store::open(&d.path().join(crate::engine::DB_FILE)).unwrap();
        let s = Indexer::new(&store, &ws, &MapConfig::default())
            .unwrap()
            .refresh(None)
            .unwrap();
        assert!(s.indexed >= 4, "{s:?}");
        let n: i64 = store
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM files WHERE path LIKE 'app/%' AND skipped_reason IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(n >= 4, "{n}");
        let head = store.get_meta("git_head:app").unwrap().unwrap();
        assert_eq!(head.len(), 40);
        assert_eq!(
            store.get_meta("git_head:notes").unwrap().as_deref(),
            Some("")
        );
        let composed = store.get_meta("git_head").unwrap().unwrap();
        assert_eq!(composed, format!("app:{} notes:none", &head[..7]));
    }
}
