//! sqlite-vec tables (knowledge spec §3): `section_vec`, `entity_vec`, `relation_vec`,
//! created when the embedding dimension is first known; a different dimension or embedding
//! model rebuilds.

use crate::store::Store;
use crate::{Error, Result};
use rusqlite::params;

pub const TABLES: [&str; 3] = ["section_vec", "entity_vec", "relation_vec"];

pub fn to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

pub fn from_blob(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

pub fn dim(store: &Store) -> Result<Option<usize>> {
    Ok(store.get_meta("embed_dim")?.and_then(|s| s.parse().ok()))
}

/// The embedding model the stored vectors came from, written beside `embed_dim`.
pub fn model(store: &Store) -> Result<Option<String>> {
    store.get_meta("embed_model")
}

/// Creates the three `vec0` tables for `dim_now` from `model_now` when absent. Returns
/// `true` when it (re)created them. A different stored dimension or model drops them,
/// clears everything derived from embeddings or extraction, marks `embeddings_rebuilding`
/// and re-queues every document section with a body.
pub fn ensure_tables(store: &Store, dim_now: usize, model_now: &str) -> Result<bool> {
    let conn = store.conn();
    let same_model = model(store)?.as_deref() == Some(model_now);
    match dim(store)? {
        Some(d) if d == dim_now && same_model => {
            let exists: i64 = conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN (?1, ?2, ?3)",
                TABLES,
                |r| r.get(0),
            )?;
            if exists == TABLES.len() as i64 {
                return Ok(false);
            }
        }
        Some(_) => {
            let tx = conn.unchecked_transaction()?;
            for t in TABLES {
                tx.execute_batch(&format!("DROP TABLE IF EXISTS {t}"))?;
            }
            tx.execute_batch(
                "DELETE FROM section_embeddings; DELETE FROM entity_mentions; DELETE FROM relations; DELETE FROM entities; DELETE FROM embedding_cache;",
            )?;
            tx.execute(
                "INSERT OR REPLACE INTO extract_queue(symbol_id, hash, attempts, last_error, queued_at_ms)
                 SELECT s.id, '', 0, NULL, ?1 FROM symbols s JOIN sections_fts f ON f.rowid = s.id",
                [crate::time::now_ms()],
            )?;
            tx.commit()?;
            store.set_meta("embeddings_rebuilding", "1")?;
        }
        None => {}
    }
    for t in TABLES {
        conn.execute_batch(&format!(
            "CREATE VIRTUAL TABLE IF NOT EXISTS {t} USING vec0(embedding float[{dim_now}] distance_metric=cosine)"
        ))?;
    }
    store.set_meta("embed_dim", &dim_now.to_string())?;
    store.set_meta("embed_model", model_now)?;
    Ok(true)
}

/// Refuses a vector whose length is not the index's dimension, before any write.
pub fn insert(store: &Store, table: &str, rowid: i64, v: &[f32]) -> Result<()> {
    debug_assert!(TABLES.contains(&table), "unknown vector table {table}");
    let d = dim(store)?;
    if d != Some(v.len()) {
        return Err(Error::ModelUnavailable(format!(
            "embedding of {} dims for a {d:?}-dim index",
            v.len()
        )));
    }
    store.conn().execute(
        &format!("INSERT OR REPLACE INTO {table}(rowid, embedding) VALUES (?1, ?2)"),
        params![rowid, to_blob(v)],
    )?;
    Ok(())
}

pub fn delete(store: &Store, table: &str, rowid: i64) -> Result<()> {
    debug_assert!(TABLES.contains(&table), "unknown vector table {table}");
    store
        .conn()
        .execute(&format!("DELETE FROM {table} WHERE rowid = ?1"), [rowid])?;
    Ok(())
}

/// Nearest `k` rows, best first, as `(rowid, similarity)` with similarity = 1 − cosine distance.
pub fn knn(store: &Store, table: &str, query: &[f32], k: usize) -> Result<Vec<(i64, f64)>> {
    debug_assert!(TABLES.contains(&table), "unknown vector table {table}");
    if k == 0 || dim(store)?.is_none() {
        return Ok(Vec::new());
    }
    let mut stmt = store.conn().prepare(&format!(
        "SELECT rowid, distance FROM {table} WHERE embedding MATCH ?1 AND k = ?2 ORDER BY distance"
    ))?;
    let rows = stmt.query_map(params![to_blob(query), k as i64], |r| {
        Ok((r.get::<_, i64>(0)?, 1.0 - r.get::<_, f64>(1)?))
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vec_tables_are_created_once_and_knn_returns_nearest_by_cosine() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(dim(&store).unwrap(), None);
        assert!(ensure_tables(&store, 3, "m").unwrap());
        assert!(
            !ensure_tables(&store, 3, "m").unwrap(),
            "same dim: nothing to do"
        );
        insert(&store, "section_vec", 1, &[1.0, 0.0, 0.0]).unwrap();
        insert(&store, "section_vec", 2, &[0.0, 1.0, 0.0]).unwrap();
        insert(&store, "section_vec", 3, &[0.9, 0.1, 0.0]).unwrap();
        let hits = knn(&store, "section_vec", &[1.0, 0.0, 0.0], 2).unwrap();
        assert_eq!(hits.iter().map(|h| h.0).collect::<Vec<_>>(), vec![1, 3]);
        assert!(
            hits[0].1 > 0.999 && hits[1].1 > 0.98 && hits[1].1 < hits[0].1,
            "{hits:?}"
        );
        delete(&store, "section_vec", 3).unwrap();
        assert_eq!(
            knn(&store, "section_vec", &[1.0, 0.0, 0.0], 5)
                .unwrap()
                .len(),
            2
        );
        assert_eq!(from_blob(&to_blob(&[1.5, -2.0])), vec![1.5, -2.0]);
    }

    #[test]
    fn missing_vector_tables_are_recreated_at_the_same_dimension() {
        let store = Store::open_in_memory().unwrap();
        assert!(ensure_tables(&store, 3, "m").unwrap());
        insert(&store, "section_vec", 1, &[1.0, 0.0, 0.0]).unwrap();
        store.conn().execute_batch("DROP TABLE entity_vec").unwrap();
        assert!(
            ensure_tables(&store, 3, "m").unwrap(),
            "a missing table is recreated"
        );
        let n: i64 = store
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'entity_vec'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
        assert_eq!(
            knn(&store, "section_vec", &[1.0, 0.0, 0.0], 5)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn a_model_change_at_the_same_dimension_rebuilds() {
        let store = Store::open_in_memory().unwrap();
        assert!(ensure_tables(&store, 3, "old-embed").unwrap());
        assert!(!ensure_tables(&store, 3, "old-embed").unwrap());
        insert(&store, "section_vec", 1, &[1.0, 0.0, 0.0]).unwrap();
        store
            .conn()
            .execute(
                "INSERT INTO embedding_cache(hash, model, dim, blob) VALUES ('h', 'old-embed', 3, x'00')",
                [],
            )
            .unwrap();
        assert!(
            ensure_tables(&store, 3, "new-embed").unwrap(),
            "model changed"
        );
        assert_eq!(dim(&store).unwrap(), Some(3), "dimension unchanged");
        assert_eq!(model(&store).unwrap().as_deref(), Some("new-embed"));
        assert!(
            knn(&store, "section_vec", &[1.0, 0.0, 0.0], 5)
                .unwrap()
                .is_empty(),
            "the old model's vectors are gone"
        );
        let cached: i64 = store
            .conn()
            .query_row("SELECT COUNT(*) FROM embedding_cache", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cached, 0);
        assert_eq!(
            store.get_meta("embeddings_rebuilding").unwrap().as_deref(),
            Some("1")
        );
    }

    #[test]
    fn a_wrong_dimension_is_refused() {
        let store = Store::open_in_memory().unwrap();
        ensure_tables(&store, 3, "m").unwrap();
        let e = insert(&store, "section_vec", 1, &[1.0, 0.0]).unwrap_err();
        assert!(matches!(e, crate::Error::ModelUnavailable(_)), "{e}");
    }

    #[test]
    fn a_dimension_change_rebuilds_and_requeues() {
        let dir = tempfile::tempdir().unwrap();
        crate::fixture::write_docs_mini(dir.path());
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        let ws = crate::workspace::Workspace::single(dir.path()).unwrap();
        crate::index::Indexer::new(&store, &ws, &crate::config::MapConfig::default())
            .unwrap()
            .refresh(None)
            .unwrap();
        ensure_tables(&store, 4, "m").unwrap();
        insert(&store, "section_vec", 1, &[1.0, 0.0, 0.0, 0.0]).unwrap();
        store
            .conn()
            .execute(
                "INSERT INTO section_embeddings(symbol_id, hash) VALUES (1, 'h')",
                [],
            )
            .unwrap();
        store
            .conn()
            .execute("DELETE FROM extract_queue", [])
            .unwrap();
        assert!(ensure_tables(&store, 8, "m").unwrap(), "dim changed");
        assert_eq!(dim(&store).unwrap(), Some(8));
        let n: i64 = store
            .conn()
            .query_row("SELECT COUNT(*) FROM section_embeddings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
        let q: i64 = store
            .conn()
            .query_row("SELECT COUNT(*) FROM extract_queue", [], |r| r.get(0))
            .unwrap();
        assert!(
            q >= 3,
            "every document section with a body is queued again: {q}"
        );
        assert_eq!(
            store.get_meta("embeddings_rebuilding").unwrap().as_deref(),
            Some("1")
        );
    }
}
