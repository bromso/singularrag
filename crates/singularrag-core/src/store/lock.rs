//! Advisory indexer lock: one row, pid + heartbeat. A heartbeat older than
//! `LOCK_STALE_MS` is treated as abandoned and may be taken over.

use rusqlite::params;

use crate::store::Store;
use crate::Result;

pub const LOCK_STALE_MS: i64 = 10_000;

pub fn try_acquire(store: &Store, pid: u32, now_ms: i64) -> Result<bool> {
    let conn = store.conn();
    let changed = conn.execute(
        "INSERT INTO indexer_lock(id, pid, heartbeat_at_ms) VALUES (1, ?1, ?2)
         ON CONFLICT(id) DO UPDATE SET pid = excluded.pid, heartbeat_at_ms = excluded.heartbeat_at_ms
         WHERE indexer_lock.pid = excluded.pid OR indexer_lock.heartbeat_at_ms < ?2 - ?3",
        params![pid, now_ms, LOCK_STALE_MS],
    )?;
    Ok(changed == 1)
}

pub fn heartbeat(store: &Store, pid: u32, now_ms: i64) -> Result<()> {
    store.conn().execute(
        "UPDATE indexer_lock SET heartbeat_at_ms = ?2 WHERE id = 1 AND pid = ?1",
        params![pid, now_ms],
    )?;
    Ok(())
}

/// The pid holding the lock row, stale or not.
pub fn holder(store: &Store) -> Result<Option<u32>> {
    use rusqlite::OptionalExtension;
    Ok(store
        .conn()
        .query_row("SELECT pid FROM indexer_lock WHERE id = 1", [], |r| {
            r.get(0)
        })
        .optional()?)
}

pub fn release(store: &Store, pid: u32) -> Result<()> {
    store
        .conn()
        .execute("DELETE FROM indexer_lock WHERE id = 1 AND pid = ?1", [pid])?;
    Ok(())
}
