pub mod lock;
pub mod schema;

use std::path::Path;

use rusqlite::{Connection, OptionalExtension};

use crate::Result;
pub use schema::SCHEMA_VERSION;

/// One SQLite connection to a repo's `.singularrag/index.db`.
#[derive(Debug)]
pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Store> {
        Self::init(Connection::open_in_memory()?)
    }

    /// A second connection for readers (the UI). Read-only at the SQLite level, no DDL,
    /// no meta write; the writer side owns schema rebuilds, so a version mismatch is an
    /// error here rather than a rebuild.
    pub fn open_read_only(path: &Path) -> Result<Store> {
        use rusqlite::OpenFlags;
        if !path.exists() {
            return Err(crate::Error::Config(format!(
                "no index at {}; run `singularrag index`",
                path.display()
            )));
        }
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.pragma_update(None, "busy_timeout", 1000)?;
        let found = stored_version(&conn)?.unwrap_or(0);
        if found != SCHEMA_VERSION {
            return Err(crate::Error::Config(format!(
                "index schema is v{found}, this build needs v{SCHEMA_VERSION}; run `singularrag index`"
            )));
        }
        Ok(Store { conn })
    }

    fn init(conn: Connection) -> Result<Store> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "busy_timeout", 1000)?;
        if stored_version(&conn)?.is_some_and(|v| v != SCHEMA_VERSION) {
            drop_all_tables(&conn)?;
        }
        conn.execute_batch(schema::DDL)?;
        let store = Store { conn };
        store.set_meta("schema_version", &SCHEMA_VERSION.to_string())?;
        Ok(store)
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn schema_version(&self) -> Result<i64> {
        Ok(self
            .get_meta("schema_version")?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0))
    }

    /// SQLite's `PRAGMA data_version`: changes when another connection commits.
    pub fn data_version(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("PRAGMA data_version", [], |r| r.get(0))?)
    }

    pub fn get_meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [key, value],
        )?;
        Ok(())
    }
}

/// The `schema_version` already in the file, or `None` when this is a fresh database.
fn stored_version(conn: &Connection) -> Result<Option<i64>> {
    let has_meta: bool = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'meta'",
        [],
        |r| Ok(r.get::<_, i64>(0)? > 0),
    )?;
    if !has_meta {
        return Ok(None);
    }
    Ok(conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .and_then(|v| v.parse().ok()))
}

/// The index is derived from the working tree, so a schema change is resolved by
/// throwing the file's contents away and rebuilding — never by migrating.
/// Virtual tables go first so their shadow tables disappear with them.
fn drop_all_tables(conn: &Connection) -> Result<()> {
    for virtual_first in [true, false] {
        let mut stmt = conn.prepare(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
             AND (sql LIKE 'CREATE VIRTUAL%') = ?1",
        )?;
        let names: Vec<String> = stmt
            .query_map([virtual_first], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<_, _>>()?;
        for name in names {
            conn.execute_batch(&format!("DROP TABLE IF EXISTS \"{name}\""))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_creates_db_with_schema_and_wal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".singularrag").join("index.db");
        let store = Store::open(&path).unwrap();
        assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
        let mode: String = store
            .conn()
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perm = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(perm, 0o600);
        }
    }

    #[test]
    fn reopen_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        Store::open(&path).unwrap();
        let store = Store::open(&path).unwrap();
        assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn version_mismatch_drops_and_recreates_every_table() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        {
            let store = Store::open(&path).unwrap();
            store
                .conn()
                .execute(
                    "INSERT INTO files(path, lang, content_hash, mtime_ms, size, indexed_at_ms, skipped_reason)
                     VALUES ('src/a.ts', 'typescript', 'h', 1, 2, 3, NULL)",
                    [],
                )
                .unwrap();
            store.set_meta("schema_version", "1").unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
        let files: i64 = store
            .conn()
            .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
            .unwrap();
        assert_eq!(files, 0, "a stale-schema index must be rebuilt, not kept");
        // Same version: nothing is dropped.
        store
            .conn()
            .execute(
                "INSERT INTO files(path, lang, content_hash, mtime_ms, size, indexed_at_ms, skipped_reason)
                 VALUES ('src/b.ts', 'typescript', 'h', 1, 2, 3, NULL)",
                [],
            )
            .unwrap();
        drop(store);
        let store = Store::open(&path).unwrap();
        let files: i64 = store
            .conn()
            .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
            .unwrap();
        assert_eq!(files, 1);
    }

    #[test]
    fn fts_is_stemmed_so_plurals_and_inflections_match() {
        let store = Store::open_in_memory().unwrap();
        store
            .conn()
            .execute(
                "INSERT INTO symbols_fts(rowid, name, name_tokens, signature, path)
                 VALUES (1, 'createSession', 'create session', 'sig', 'src/a.ts')",
                [],
            )
            .unwrap();
        let n: i64 = store
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM symbols_fts WHERE symbols_fts MATCH 'name_tokens:sessions'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "porter stemming should match 'sessions' to 'session'");
    }

    #[test]
    fn meta_roundtrip_and_data_version_changes_on_write() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.get_meta("git_head").unwrap(), None);
        store.set_meta("git_head", "abc").unwrap();
        assert_eq!(store.get_meta("git_head").unwrap(), Some("abc".to_string()));
        // data_version only moves for writes from *other* connections; assert it is readable.
        assert!(store.data_version().unwrap() >= 0);
    }

    #[test]
    fn all_tables_exist() {
        let store = Store::open_in_memory().unwrap();
        for t in [
            "meta",
            "files",
            "symbols",
            "refs",
            "symbols_fts",
            "retrievals",
            "retrieval_items",
            "indexer_lock",
        ] {
            let n: i64 = store
                .conn()
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE name = ?1",
                    [t],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "missing table {t}");
        }
    }

    #[test]
    fn read_only_opens_an_existing_index_and_cannot_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".singularrag/index.db");
        let writer = Store::open(&path).unwrap();
        writer.set_meta("git_head", "abc").unwrap();
        let ro = Store::open_read_only(&path).unwrap();
        assert_eq!(ro.get_meta("git_head").unwrap().as_deref(), Some("abc"));
        assert!(ro.data_version().unwrap() >= 0);
        let err = ro
            .conn()
            .execute("INSERT INTO meta(key, value) VALUES ('x', 'y')", [])
            .unwrap_err();
        assert!(err.to_string().contains("readonly"), "{err}");
    }

    #[test]
    fn read_only_refuses_a_missing_file_and_a_wrong_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        let err = Store::open_read_only(&path).unwrap_err();
        assert!(err.to_string().contains("no index at"), "{err}");
        let writer = Store::open(&path).unwrap();
        writer.set_meta("schema_version", "1").unwrap();
        drop(writer);
        let err = Store::open_read_only(&path).unwrap_err();
        assert!(err.to_string().contains("schema is v1"), "{err}");
    }

    #[test]
    fn read_only_sees_commits_from_the_writer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        let writer = Store::open(&path).unwrap();
        let ro = Store::open_read_only(&path).unwrap();
        let v1 = ro.data_version().unwrap();
        writer.set_meta("k", "v").unwrap();
        let v2 = ro.data_version().unwrap();
        assert_ne!(
            v1, v2,
            "data_version must change when another connection commits"
        );
        assert_eq!(ro.get_meta("k").unwrap().as_deref(), Some("v"));
    }
}
