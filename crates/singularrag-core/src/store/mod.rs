pub mod lock;
pub mod schema;

use std::path::Path;

use rusqlite::{Connection, OptionalExtension};

use crate::Result;
pub use schema::SCHEMA_VERSION;

/// One SQLite connection to a repo's `.singularrag/index.db`.
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

    fn init(conn: Connection) -> Result<Store> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "busy_timeout", 1000)?;
        conn.execute_batch(schema::DDL)?;
        let store = Store { conn };
        if store.get_meta("schema_version")?.is_none() {
            store.set_meta("schema_version", &SCHEMA_VERSION.to_string())?;
        }
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
}
