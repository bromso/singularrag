//! The knowledge layer's write side (knowledge spec §3): the extraction queue the indexer
//! fills, the embedding and extraction caches, and the tick that drains the queue into
//! section vectors, entities, mentions and relations.
//!
//! Model calls happen outside any transaction; every write for one section happens in one.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use rusqlite::{params, Connection, OptionalExtension};

use crate::models::{ExtractedEntity, Extraction, Models, SectionInput, DESCRIPTION_MAX};
use crate::store::{vec, Store};
use crate::time::now_ms;
use crate::{Error, Result};

pub const EXTRACT_PER_TICK: usize = 5;
pub const EMBED_PER_TICK: usize = 50;
pub const MAX_ATTEMPTS: i64 = 3;

pub fn section_hash(body: &str) -> String {
    blake3::hash(body.as_bytes()).to_hex().to_string()
}

/// Lowercased, whitespace collapsed, trailing punctuation dropped.
pub fn norm_name(s: &str) -> String {
    let collapsed = s
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    collapsed
        .trim_end_matches(['.', ',', ';', ':', '!', '?'])
        .trim()
        .to_string()
}

/// Called by the indexer for every section-kind symbol with a non-blank body, inside its transaction.
pub fn queue_section(conn: &Connection, symbol_id: i64, body: &str) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO extract_queue(symbol_id, hash, attempts, last_error, queued_at_ms) VALUES (?1, ?2, 0, NULL, ?3)",
        params![symbol_id, section_hash(body), now_ms()],
    )?;
    Ok(())
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [name],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// Called from `delete_symbols_for` before the symbols go: mentions, relations (+ vector rows),
/// section embeddings (+ vector rows) and queue rows for the file's symbols; then the
/// entities those mentions pointed at are recounted and dropped when nothing mentions them.
pub fn delete_for_symbols(conn: &Connection, file_id: i64) -> Result<()> {
    const SYMS: &str = "SELECT id FROM symbols WHERE file_id = ?1";
    let affected: Vec<i64> = {
        let mut stmt = conn.prepare(&format!(
            "SELECT DISTINCT entity_id FROM entity_mentions WHERE symbol_id IN ({SYMS})"
        ))?;
        let rows = stmt.query_map([file_id], |r| r.get(0))?;
        rows.collect::<std::result::Result<_, _>>()?
    };
    if table_exists(conn, "relation_vec")? {
        conn.execute(
            &format!("DELETE FROM relation_vec WHERE rowid IN (SELECT id FROM relations WHERE symbol_id IN ({SYMS}))"),
            [file_id],
        )?;
    }
    if table_exists(conn, "section_vec")? {
        conn.execute(
            &format!("DELETE FROM section_vec WHERE rowid IN ({SYMS})"),
            [file_id],
        )?;
    }
    for table in [
        "relations",
        "entity_mentions",
        "section_embeddings",
        "extract_queue",
    ] {
        conn.execute(
            &format!("DELETE FROM {table} WHERE symbol_id IN ({SYMS})"),
            [file_id],
        )?;
    }
    if !affected.is_empty() {
        recount(conn, &affected)?;
        drop_unmentioned(conn)?;
    }
    Ok(())
}

/// Entities with no mentions left are deleted with their vector rows (and the relations
/// that pointed at them); `mentions` is recounted for every entity.
pub fn prune_entities(conn: &Connection) -> Result<()> {
    conn.execute(
        "UPDATE entities SET mentions = (SELECT COUNT(*) FROM entity_mentions m WHERE m.entity_id = entities.id)",
        [],
    )?;
    drop_unmentioned(conn)
}

fn recount(conn: &Connection, ids: &[i64]) -> Result<()> {
    let mut stmt = conn.prepare(
        "UPDATE entities SET mentions = (SELECT COUNT(*) FROM entity_mentions m WHERE m.entity_id = ?1) WHERE id = ?1",
    )?;
    for id in ids {
        stmt.execute([id])?;
    }
    Ok(())
}

fn drop_unmentioned(conn: &Connection) -> Result<()> {
    const GONE: &str = "SELECT id FROM entities WHERE mentions = 0";
    if table_exists(conn, "relation_vec")? {
        conn.execute(
            &format!("DELETE FROM relation_vec WHERE rowid IN (SELECT id FROM relations WHERE src_entity IN ({GONE}) OR dst_entity IN ({GONE}))"),
            [],
        )?;
    }
    conn.execute(
        &format!("DELETE FROM relations WHERE src_entity IN ({GONE}) OR dst_entity IN ({GONE})"),
        [],
    )?;
    if table_exists(conn, "entity_vec")? {
        conn.execute(
            &format!("DELETE FROM entity_vec WHERE rowid IN ({GONE})"),
            [],
        )?;
    }
    conn.execute("DELETE FROM entities WHERE mentions = 0", [])?;
    Ok(())
}

/// Sections still waiting for extraction; one that failed `MAX_ATTEMPTS` times is not pending.
pub fn pending(store: &Store) -> Result<usize> {
    let n: i64 = store.conn().query_row(
        "SELECT COUNT(*) FROM extract_queue WHERE attempts < ?1",
        [MAX_ATTEMPTS],
        |r| r.get(0),
    )?;
    Ok(n as usize)
}

/// `(path, section name, last error)` for every section whose extraction failed `MAX_ATTEMPTS` times.
pub fn failed_sections(store: &Store) -> Result<Vec<(String, String, String)>> {
    let mut stmt = store.conn().prepare(
        "SELECT f.path, s.name, COALESCE(q.last_error, '') FROM extract_queue q
         JOIN symbols s ON s.id = q.symbol_id JOIN files f ON f.id = s.file_id
         WHERE q.attempts >= ?1 ORDER BY f.path, s.line_start",
    )?;
    let rows = stmt.query_map([MAX_ATTEMPTS], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct KnowledgeTick {
    /// Sections that gained a vector this tick.
    pub embedded: usize,
    /// Sections whose extraction was written this tick (cache hits included).
    pub extracted: usize,
    /// Extraction attempts that failed on a malformed answer this tick.
    pub failed: usize,
    /// `pending()` after the tick.
    pub pending: usize,
    /// The model outage that stopped the tick, if any.
    pub model_error: Option<String>,
}

/// `old` extended with `add` (joined with `; `, capped at `DESCRIPTION_MAX` characters)
/// unless `add` is empty or already one of its parts.
fn extend_description(old: &str, add: &str) -> String {
    if add.is_empty() || old.split("; ").any(|p| p == add) {
        return old.to_string();
    }
    if old.is_empty() {
        return add.to_string();
    }
    format!("{old}; {add}")
        .chars()
        .take(DESCRIPTION_MAX)
        .collect()
}

/// The extraction's entities merged by `norm_name` (first name and type win, descriptions
/// extended); entities whose name normalises to nothing are dropped.
fn merge_entities(e: &Extraction) -> Vec<(String, ExtractedEntity)> {
    let mut out: Vec<(String, ExtractedEntity)> = Vec::new();
    for ent in &e.entities {
        let n = norm_name(&ent.name);
        if n.is_empty() {
            continue;
        }
        match out.iter_mut().find(|(k, _)| *k == n) {
            Some((_, first)) => {
                first.description = extend_description(&first.description, &ent.description)
            }
            None => out.push((n, ent.clone())),
        }
    }
    out
}

fn entity_text(name: &str, description: &str) -> String {
    format!("{name}: {description}")
}

fn relation_text(src: &str, dst: &str, description: &str) -> String {
    format!("{src} → {dst}: {description}")
}

fn entity_by_norm(conn: &Connection, norm: &str) -> Result<Option<(i64, String, String)>> {
    Ok(conn
        .query_row(
            "SELECT id, name, description FROM entities WHERE norm_name = ?1",
            [norm],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?)
}

/// Upserts entities by norm_name (extending a differing description), inserts mentions and
/// relations for this section, returns the new entity ids and relation ids that need vectors.
/// Whatever this section said before is replaced, so applying twice is harmless.
pub fn apply_extraction(
    conn: &Connection,
    symbol_id: i64,
    hash: &str,
    e: &Extraction,
) -> Result<(Vec<i64>, Vec<i64>)> {
    let prior: Vec<i64> = {
        let mut stmt =
            conn.prepare("SELECT DISTINCT entity_id FROM entity_mentions WHERE symbol_id = ?1")?;
        let rows = stmt.query_map([symbol_id], |r| r.get(0))?;
        rows.collect::<std::result::Result<_, _>>()?
    };
    if table_exists(conn, "relation_vec")? {
        conn.execute(
            "DELETE FROM relation_vec WHERE rowid IN (SELECT id FROM relations WHERE symbol_id = ?1)",
            [symbol_id],
        )?;
    }
    conn.execute("DELETE FROM relations WHERE symbol_id = ?1", [symbol_id])?;
    conn.execute(
        "DELETE FROM entity_mentions WHERE symbol_id = ?1",
        [symbol_id],
    )?;

    let mut new_entities = Vec::new();
    let mut by_norm: HashMap<String, i64> = HashMap::new();
    for (n, ent) in merge_entities(e) {
        let id = match entity_by_norm(conn, &n)? {
            Some((id, _, old)) => {
                let d = extend_description(&old, &ent.description);
                if d != old {
                    conn.execute(
                        "UPDATE entities SET description = ?2 WHERE id = ?1",
                        params![id, d],
                    )?;
                }
                id
            }
            None => {
                conn.execute(
                    "INSERT INTO entities(name, norm_name, type, description, mentions) VALUES (?1, ?2, ?3, ?4, 0)",
                    params![ent.name, n, ent.r#type, ent.description],
                )?;
                let id = conn.last_insert_rowid();
                new_entities.push(id);
                id
            }
        };
        conn.execute(
            "INSERT OR IGNORE INTO entity_mentions(entity_id, symbol_id, section_hash) VALUES (?1, ?2, ?3)",
            params![id, symbol_id, hash],
        )?;
        by_norm.insert(n, id);
    }

    let mut new_relations = Vec::new();
    for r in &e.relations {
        let resolve = |name: &str| -> Result<Option<i64>> {
            let n = norm_name(name);
            if let Some(id) = by_norm.get(&n) {
                return Ok(Some(*id));
            }
            Ok(entity_by_norm(conn, &n)?.map(|(id, _, _)| id))
        };
        let (Some(src), Some(dst)) = (resolve(&r.source)?, resolve(&r.target)?) else {
            continue;
        };
        if src == dst {
            continue;
        }
        conn.execute(
            "INSERT INTO relations(src_entity, dst_entity, description, symbol_id, section_hash) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![src, dst, r.description, symbol_id, hash],
        )?;
        new_relations.push(conn.last_insert_rowid());
    }

    let touched: Vec<i64> = by_norm
        .values()
        .copied()
        .chain(prior.iter().copied())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    recount(conn, &touched)?;
    if !prior.is_empty() {
        drop_unmentioned(conn)?;
    }
    Ok((new_entities, new_relations))
}

/// The texts `apply_extraction` will need vectors for, computed before it runs so the model
/// call stays outside the transaction: new entities (`name: description`) and relations
/// (`src → dst: description`), named as they will be stored.
fn vector_texts(conn: &Connection, e: &Extraction) -> Result<Vec<String>> {
    let mut names: HashMap<String, String> = HashMap::new();
    let mut texts = Vec::new();
    for (n, ent) in merge_entities(e) {
        match entity_by_norm(conn, &n)? {
            Some((_, name, _)) => {
                names.insert(n, name);
            }
            None => {
                texts.push(entity_text(&ent.name, &ent.description));
                names.insert(n, ent.name);
            }
        }
    }
    for r in &e.relations {
        let (s, d) = (norm_name(&r.source), norm_name(&r.target));
        if s == d {
            continue;
        }
        let name_of = |n: &str| -> Result<Option<String>> {
            if let Some(name) = names.get(n) {
                return Ok(Some(name.clone()));
            }
            Ok(entity_by_norm(conn, n)?.map(|(_, name, _)| name))
        };
        if let (Some(src), Some(dst)) = (name_of(&s)?, name_of(&d)?) {
            texts.push(relation_text(&src, &dst, &r.description));
        }
    }
    let mut seen = HashSet::new();
    texts.retain(|t| seen.insert(t.clone()));
    Ok(texts)
}

fn section_body(conn: &Connection, symbol_id: i64) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT content FROM sections_fts WHERE rowid = ?1",
            [symbol_id],
            |r| r.get(0),
        )
        .optional()?)
}

fn cached_extraction(conn: &Connection, hash: &str) -> Result<Option<Extraction>> {
    let json: Option<String> = conn
        .query_row(
            "SELECT json FROM extraction_cache WHERE hash = ?1",
            [hash],
            |r| r.get(0),
        )
        .optional()?;
    Ok(json.and_then(|j| serde_json::from_str(&j).ok()))
}

fn record_outage(store: &Store, t: &mut KnowledgeTick, message: String) -> Result<()> {
    store.set_meta("models_error", &message)?;
    t.model_error = Some(message);
    Ok(())
}

/// When the model now answers in a different dimension than the index holds, rebuild
/// (which clears the vectors and re-queues every section) and tell the caller to end
/// the tick; the next ticks re-embed at the new dimension. Not an outage.
fn dimension_changed(store: &Store, vectors: &[Vec<f32>]) -> Result<bool> {
    let Some(len) = vectors.first().map(Vec::len) else {
        return Ok(false);
    };
    match vec::dim(store)? {
        Some(d) if d != len => {
            vec::ensure_tables(store, len)?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

fn clear_meta(store: &Store, key: &str) -> Result<()> {
    store
        .conn()
        .execute("DELETE FROM meta WHERE key = ?1", [key])?;
    Ok(())
}

/// One background step: embed queued sections, then extract some under `budget`.
/// A model outage never escapes as an error: it lands in `model_error` and in meta
/// `models_error`, and the tick stops there. `should_yield` is asked before the embed
/// batch and before each extraction: `true` (a job is waiting) ends the tick there.
pub fn tick(
    store: &Store,
    models: &Models,
    budget: Duration,
    should_yield: &dyn Fn() -> bool,
) -> Result<KnowledgeTick> {
    let start = Instant::now();
    let mut t = KnowledgeTick::default();
    if should_yield() {
        // Nothing was asked of the models, so an earlier outage flag stays as it is.
        t.pending = pending(store)?;
        return Ok(t);
    }
    if embed_step(store, models, &mut t)? {
        extract_step(store, models, &mut t, start, budget, should_yield)?;
    }
    t.pending = pending(store)?;
    if t.model_error.is_none() {
        clear_meta(store, "models_error")?;
    }
    if t.pending == 0 {
        clear_meta(store, "embeddings_rebuilding")?;
    }
    Ok(t)
}

struct ToEmbed {
    symbol_id: i64,
    hash: String,
    body: String,
    vector: Option<Vec<f32>>,
}

/// Queued sections without a vector, up to `EMBED_PER_TICK`: cache hits by hash, the misses
/// in one model call. Returns `false` when a model outage or a dimension rebuild ended the tick.
fn embed_step(store: &Store, models: &Models, t: &mut KnowledgeTick) -> Result<bool> {
    let conn = store.conn();
    let rows: Vec<(i64, String)> = {
        let mut stmt = conn.prepare(
            "SELECT q.symbol_id, q.hash FROM extract_queue q
             WHERE NOT EXISTS (SELECT 1 FROM section_embeddings e WHERE e.symbol_id = q.symbol_id)
             ORDER BY q.queued_at_ms, q.symbol_id LIMIT ?1",
        )?;
        let rows = stmt.query_map([EMBED_PER_TICK as i64], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<std::result::Result<_, _>>()?
    };
    if rows.is_empty() {
        return Ok(true);
    }
    let dim_before = vec::dim(store)?;
    let mut items = Vec::with_capacity(rows.len());
    for (symbol_id, hash) in rows {
        let Some(body) = section_body(conn, symbol_id)? else {
            conn.execute(
                "DELETE FROM extract_queue WHERE symbol_id = ?1",
                [symbol_id],
            )?;
            continue;
        };
        // An empty hash was written by a dimension rebuild: recompute it before any cache lookup.
        let hash = if hash.is_empty() {
            section_hash(&body)
        } else {
            hash
        };
        let cached: Option<(i64, Vec<u8>)> = conn
            .query_row(
                "SELECT dim, blob FROM embedding_cache WHERE hash = ?1",
                [&hash],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let vector = cached
            .filter(|(d, _)| dim_before == Some(*d as usize))
            .map(|(_, b)| vec::from_blob(&b));
        items.push(ToEmbed {
            symbol_id,
            hash,
            body,
            vector,
        });
    }

    let misses: Vec<usize> = (0..items.len())
        .filter(|&i| items[i].vector.is_none())
        .collect();
    if !misses.is_empty() {
        let texts: Vec<String> = misses.iter().map(|&i| items[i].body.clone()).collect();
        let vectors = match models.embed(&texts) {
            Ok(v) => v,
            Err(Error::ModelUnavailable(m)) => {
                record_outage(store, t, m)?;
                return Ok(false);
            }
            Err(e) => return Err(e),
        };
        let d = vectors.first().map_or(0, Vec::len);
        if d == 0 {
            record_outage(store, t, "embed: empty vector".to_string())?;
            return Ok(false);
        }
        if dimension_changed(store, &vectors)? {
            return Ok(false);
        }
        // First vectors ever: this creates the tables; otherwise a cheap no-op.
        vec::ensure_tables(store, d)?;
        for (i, v) in misses.into_iter().zip(vectors) {
            items[i].vector = Some(v);
        }
    } else if let Some(d) = dim_before {
        vec::ensure_tables(store, d)?;
    }

    let tx = conn.unchecked_transaction()?;
    let mut embedded = 0;
    for it in &items {
        let Some(v) = &it.vector else { continue };
        match vec::insert(store, "section_vec", it.symbol_id, v) {
            Ok(()) => {}
            Err(Error::ModelUnavailable(m)) => {
                drop(tx);
                record_outage(store, t, m)?;
                return Ok(false);
            }
            Err(e) => return Err(e),
        }
        tx.execute(
            "INSERT OR REPLACE INTO section_embeddings(symbol_id, hash) VALUES (?1, ?2)",
            params![it.symbol_id, it.hash],
        )?;
        tx.execute(
            "INSERT OR REPLACE INTO embedding_cache(hash, dim, blob) VALUES (?1, ?2, ?3)",
            params![it.hash, v.len() as i64, vec::to_blob(v)],
        )?;
        tx.execute(
            "UPDATE extract_queue SET hash = ?2 WHERE symbol_id = ?1 AND hash = ''",
            params![it.symbol_id, it.hash],
        )?;
        embedded += 1;
    }
    tx.commit()?;
    t.embedded += embedded;
    Ok(true)
}

/// Up to `EXTRACT_PER_TICK` embedded queue rows, oldest first, while `budget` lasts.
fn extract_step(
    store: &Store,
    models: &Models,
    t: &mut KnowledgeTick,
    start: Instant,
    budget: Duration,
    should_yield: &dyn Fn() -> bool,
) -> Result<()> {
    let conn = store.conn();
    let rows: Vec<(i64, String, String, String)> = {
        let mut stmt = conn.prepare(
            "SELECT q.symbol_id, q.hash, s.name, f.path FROM extract_queue q
             JOIN section_embeddings e ON e.symbol_id = q.symbol_id
             JOIN symbols s ON s.id = q.symbol_id JOIN files f ON f.id = s.file_id
             WHERE q.attempts < ?1 ORDER BY q.queued_at_ms, q.symbol_id LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![MAX_ATTEMPTS, EXTRACT_PER_TICK as i64], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?;
        rows.collect::<std::result::Result<_, _>>()?
    };
    for (symbol_id, hash, heading, path) in rows {
        if start.elapsed() >= budget || should_yield() {
            break;
        }
        let Some(body) = section_body(conn, symbol_id)? else {
            conn.execute(
                "DELETE FROM extract_queue WHERE symbol_id = ?1",
                [symbol_id],
            )?;
            continue;
        };
        let hash = if hash.is_empty() {
            let h = section_hash(&body);
            conn.execute(
                "UPDATE extract_queue SET hash = ?2 WHERE symbol_id = ?1",
                params![symbol_id, h],
            )?;
            h
        } else {
            hash
        };
        let extraction = match cached_extraction(conn, &hash)? {
            Some(x) => x,
            None => match models.extract(&SectionInput {
                path: &path,
                heading: &heading,
                text: &body,
            }) {
                Ok(x) => {
                    // A memo keyed by hash, not section state: kept even when a later
                    // step fails, so that failure never costs another model call.
                    conn.execute(
                        "INSERT OR REPLACE INTO extraction_cache(hash, json) VALUES (?1, ?2)",
                        params![hash, extraction_json(&x)?],
                    )?;
                    x
                }
                Err(Error::ModelUnavailable(m)) if m.contains("not JSON") => {
                    conn.execute(
                        "UPDATE extract_queue SET attempts = attempts + 1, last_error = ?2 WHERE symbol_id = ?1",
                        params![symbol_id, m],
                    )?;
                    t.failed += 1;
                    continue;
                }
                Err(Error::ModelUnavailable(m)) => return record_outage(store, t, m),
                Err(e) => return Err(e),
            },
        };
        let texts = vector_texts(conn, &extraction)?;
        let vectors: HashMap<String, Vec<f32>> = if texts.is_empty() {
            HashMap::new()
        } else {
            match models.embed(&texts) {
                Ok(v) => {
                    if dimension_changed(store, &v)? {
                        return Ok(());
                    }
                    texts.into_iter().zip(v).collect()
                }
                Err(Error::ModelUnavailable(m)) => return record_outage(store, t, m),
                Err(e) => return Err(e),
            }
        };
        match write_section(store, symbol_id, &hash, &extraction, &vectors) {
            Ok(()) => t.extracted += 1,
            Err(Error::ModelUnavailable(m)) => return record_outage(store, t, m),
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn extraction_json(x: &Extraction) -> Result<String> {
    serde_json::to_string(x).map_err(|e| Error::Config(e.to_string()))
}

/// One transaction: the extraction applied, vectors for what is new, the queue row gone. Any error rolls all of it back.
fn write_section(
    store: &Store,
    symbol_id: i64,
    hash: &str,
    x: &Extraction,
    vectors: &HashMap<String, Vec<f32>>,
) -> Result<()> {
    let tx = store.conn().unchecked_transaction()?;
    let (entities, relations) = apply_extraction(&tx, symbol_id, hash, x)?;
    for id in entities {
        let text: Option<String> = tx
            .query_row(
                "SELECT name, description FROM entities WHERE id = ?1",
                [id],
                |r| {
                    Ok(entity_text(
                        &r.get::<_, String>(0)?,
                        &r.get::<_, String>(1)?,
                    ))
                },
            )
            .optional()?;
        if let Some(v) = text.and_then(|t| vectors.get(&t)) {
            vec::insert(store, "entity_vec", id, v)?;
        }
    }
    for id in relations {
        let text: Option<String> = tx
            .query_row(
                "SELECT s.name, d.name, r.description FROM relations r
                 JOIN entities s ON s.id = r.src_entity JOIN entities d ON d.id = r.dst_entity
                 WHERE r.id = ?1",
                [id],
                |r| {
                    Ok(relation_text(
                        &r.get::<_, String>(0)?,
                        &r.get::<_, String>(1)?,
                        &r.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;
        if let Some(v) = text.and_then(|t| vectors.get(&t)) {
            vec::insert(store, "relation_vec", id, v)?;
        }
    }
    tx.execute(
        "DELETE FROM extract_queue WHERE symbol_id = ?1",
        [symbol_id],
    )?;
    tx.commit()?;
    Ok(())
}

/// Nearest sections a query vector seeds (knowledge spec §4).
pub const SEMANTIC_K: usize = 20;
/// Nearest entities a query vector may match.
pub const ENTITY_K: usize = 10;
/// An entity matched by vector must be at least this close to the query.
pub const ENTITY_MIN_SIM: f64 = 0.5;
/// Nearest relations one theme seeds.
pub const THEME_K: usize = 10;

/// What the knowledge layer adds to one ranking: the query's vector, the entities the
/// query or the caller named, and one vector per theme. `entity_ids` and `entity_names`
/// are parallel. `models_unavailable` means a model call failed, so the vectors are empty.
#[derive(Debug, Clone, Default)]
pub struct Seeds {
    pub query_vec: Option<Vec<f32>>,
    pub entity_ids: Vec<i64>,
    pub entity_names: Vec<String>,
    pub theme_vecs: Vec<(String, Vec<f32>)>,
    pub models_unavailable: bool,
}

impl Seeds {
    fn add_entity(&mut self, id: i64, name: String) {
        if !self.entity_ids.contains(&id) {
            self.entity_ids.push(id);
            self.entity_names.push(name);
        }
    }
}

/// The seeds for one `repo_map` call, with at most two model calls: one embedding for
/// the query, one batch for all themes. A model outage never escapes: it sets
/// `models_unavailable` and leaves the vectors empty. Without models (disabled) only the
/// entity names the query terms or the arguments spell out match. With no vectors in
/// the index yet nothing could be matched by vector, so the models are not asked.
pub fn seeds_for(
    store: &Store,
    models: Option<&Models>,
    query: Option<&str>,
    entities: &[String],
    themes: &[String],
) -> Result<Seeds> {
    let mut seeds = Seeds::default();
    let conn = store.conn();

    let names: Vec<String> = query
        .map(crate::tokens::query_terms)
        .unwrap_or_default()
        .iter()
        .chain(entities)
        .map(|n| norm_name(n))
        .filter(|n| !n.is_empty())
        .collect();
    {
        let mut stmt = conn.prepare("SELECT id, name FROM entities WHERE norm_name = ?1")?;
        for n in &names {
            if let Some((id, name)) = stmt
                .query_row([n], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
                .optional()?
            {
                seeds.add_entity(id, name);
            }
        }
    }

    let Some(models) = models else {
        return Ok(seeds);
    };
    if vec::dim(store)?.is_none() {
        return Ok(seeds);
    }
    let unavailable = |e: Error| match e {
        Error::ModelUnavailable(_) => Ok(()),
        e => Err(e),
    };

    if let Some(q) = query.filter(|q| !q.trim().is_empty()) {
        match models.embed(&[q.to_string()]) {
            Ok(mut v) if v.len() == 1 => seeds.query_vec = v.pop(),
            Ok(_) => seeds.models_unavailable = true,
            Err(e) => {
                unavailable(e)?;
                seeds.models_unavailable = true;
            }
        }
    }
    if let Some(qv) = &seeds.query_vec {
        let mut stmt = conn.prepare("SELECT name FROM entities WHERE id = ?1")?;
        for (id, sim) in vec::knn(store, "entity_vec", qv, ENTITY_K)? {
            if sim < ENTITY_MIN_SIM {
                continue;
            }
            if let Some(name) = stmt.query_row([id], |r| r.get::<_, String>(0)).optional()? {
                seeds.add_entity(id, name);
            }
        }
    }

    let themes: Vec<String> = themes
        .iter()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();
    if !themes.is_empty() {
        match models.embed(&themes) {
            Ok(v) if v.len() == themes.len() => {
                seeds.theme_vecs = themes.into_iter().zip(v).collect();
            }
            Ok(_) => seeds.models_unavailable = true,
            Err(e) => {
                unavailable(e)?;
                seeds.models_unavailable = true;
            }
        }
    }
    if seeds.models_unavailable {
        seeds.query_vec = None;
        seeds.theme_vecs.clear();
    }
    Ok(seeds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MapConfig;
    use crate::fake_ollama::FakeOllama;
    use crate::index::Indexer;
    use crate::models::{Models, ModelsConfig};
    use crate::store::Store;
    use crate::workspace::Workspace;
    use std::time::Duration;

    fn indexed_docs() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        crate::fixture::write_docs_mini(dir.path());
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        let ws = Workspace::single(dir.path()).unwrap();
        Indexer::new(&store, &ws, &MapConfig::default())
            .unwrap()
            .refresh(None)
            .unwrap();
        (dir, store)
    }
    fn models(f: &FakeOllama) -> Models {
        Models::new(ModelsConfig {
            ollama: f.url(),
            ..Default::default()
        })
    }
    fn count(store: &Store, sql: &str) -> i64 {
        store.conn().query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn indexing_documents_queues_their_sections_and_code_never() {
        let (_d, store) = indexed_docs();
        let q = count(&store, "SELECT COUNT(*) FROM extract_queue");
        let bodies = count(&store, "SELECT COUNT(*) FROM sections_fts");
        assert_eq!(q, bodies, "one queue row per section with a body");
        let code: i64 = count(&store, "SELECT COUNT(*) FROM extract_queue q JOIN symbols s ON s.id = q.symbol_id JOIN files f ON f.id = s.file_id WHERE f.lang IN ('typescript','rust')");
        assert_eq!(code, 0);
        assert_eq!(pending(&store).unwrap() as i64, q);
    }

    #[test]
    fn a_tick_embeds_then_extracts_and_the_queue_drains() {
        let (_d, store) = indexed_docs();
        let f = FakeOllama::spawn(16);
        f.set_extraction("Freshness", serde_json::json!({
            "entities": [{"name": "STALE header", "type": "concept", "description": "the header line that says files changed"},
                         {"name": "createSession", "type": "system", "description": "creates sessions"}],
            "relations": [{"source": "createSession", "target": "STALE header", "description": "is refreshed before"}]
        }));
        let m = models(&f);
        let before = pending(&store).unwrap();
        let t = tick(&store, &m, Duration::from_secs(20), &|| false).unwrap();
        assert!(t.embedded >= 3 && t.extracted >= 1, "{t:?}");
        assert!(t.model_error.is_none());
        let mut total = t.extracted;
        while pending(&store).unwrap() > 0 {
            total += tick(&store, &m, Duration::from_secs(20), &|| false)
                .unwrap()
                .extracted;
        }
        assert_eq!(total, before, "every queued section was extracted");
        assert_eq!(
            count(
                &store,
                "SELECT COUNT(*) FROM entities WHERE norm_name = 'stale header'"
            ),
            1
        );
        assert_eq!(count(&store, "SELECT COUNT(*) FROM relations"), 1);
        assert_eq!(
            count(
                &store,
                "SELECT mentions FROM entities WHERE norm_name = 'stale header'"
            ),
            1
        );
        assert_eq!(count(&store, "SELECT COUNT(*) FROM entity_vec"), 2);
        assert_eq!(count(&store, "SELECT COUNT(*) FROM relation_vec"), 1);
        assert_eq!(
            count(&store, "SELECT COUNT(*) FROM section_embeddings"),
            count(&store, "SELECT COUNT(*) FROM sections_fts")
        );
    }

    #[test]
    fn duplicate_section_text_hits_the_cache() {
        let (dir, store) = indexed_docs();
        let f = FakeOllama::spawn(8);
        // The H1 "Runbook" has a blank body and is never queued; its one section is this heading.
        f.set_extraction("says STALE", serde_json::json!({"entities": [{"name": "Runbook", "type": "document", "description": "ops notes"}], "relations": []}));
        let m = models(&f);
        while pending(&store).unwrap() > 0 {
            tick(&store, &m, Duration::from_secs(20), &|| false).unwrap();
        }
        let generates = f
            .calls()
            .iter()
            .filter(|p| p.as_str() == "/api/generate")
            .count();
        std::fs::copy(
            dir.path().join("docs/runbook.md"),
            dir.path().join("docs/runbook-copy.md"),
        )
        .unwrap();
        let ws = Workspace::single(dir.path()).unwrap();
        Indexer::new(&store, &ws, &MapConfig::default())
            .unwrap()
            .refresh(None)
            .unwrap();
        assert!(pending(&store).unwrap() > 0, "the copy queues");
        while pending(&store).unwrap() > 0 {
            tick(&store, &m, Duration::from_secs(20), &|| false).unwrap();
        }
        assert_eq!(
            f.calls()
                .iter()
                .filter(|p| p.as_str() == "/api/generate")
                .count(),
            generates,
            "served from the cache"
        );
        assert_eq!(
            count(
                &store,
                "SELECT mentions FROM entities WHERE norm_name = 'runbook'"
            ),
            2
        );
    }

    #[test]
    fn a_changed_section_requeues_and_its_old_rows_vanish_and_a_deleted_file_clears_all() {
        let (dir, store) = indexed_docs();
        let f = FakeOllama::spawn(8);
        f.set_extraction("Storage", serde_json::json!({"entities": [{"name": "SessionStore", "type": "system", "description": "keeps sessions"}], "relations": []}));
        let m = models(&f);
        while pending(&store).unwrap() > 0 {
            tick(&store, &m, Duration::from_secs(20), &|| false).unwrap();
        }
        assert_eq!(
            count(
                &store,
                "SELECT COUNT(*) FROM entities WHERE norm_name = 'sessionstore'"
            ),
            1
        );
        let p = dir.path().join("docs/design.md");
        std::fs::write(
            &p,
            std::fs::read_to_string(&p)
                .unwrap()
                .replace("`SessionStore` keeps them.", "Nothing keeps them now."),
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(20));
        let ws = Workspace::single(dir.path()).unwrap();
        Indexer::new(&store, &ws, &MapConfig::default())
            .unwrap()
            .refresh(None)
            .unwrap();
        f.set_extraction(
            "Storage",
            serde_json::json!({"entities": [], "relations": []}),
        );
        while pending(&store).unwrap() > 0 {
            tick(&store, &m, Duration::from_secs(20), &|| false).unwrap();
        }
        assert_eq!(
            count(
                &store,
                "SELECT COUNT(*) FROM entities WHERE norm_name = 'sessionstore'"
            ),
            0,
            "no mentions left: pruned"
        );
        std::fs::remove_file(&p).unwrap();
        Indexer::new(&store, &ws, &MapConfig::default())
            .unwrap()
            .refresh(None)
            .unwrap();
        assert_eq!(count(&store, "SELECT COUNT(*) FROM entity_mentions m JOIN symbols s ON s.id = m.symbol_id JOIN files f ON f.id = s.file_id WHERE f.path = 'docs/design.md'"), 0);
        assert_eq!(count(&store, "SELECT COUNT(*) FROM extract_queue q WHERE NOT EXISTS (SELECT 1 FROM symbols s WHERE s.id = q.symbol_id)"), 0);
    }

    #[test]
    fn malformed_answers_count_attempts_and_three_skip_the_section() {
        let (_d, store) = indexed_docs();
        let f = FakeOllama::spawn(8);
        let m = models(&f);
        f.fail_next_generate(1000);
        for _ in 0..4 {
            tick(&store, &m, Duration::from_secs(20), &|| false).unwrap();
        }
        let max_attempts: i64 = count(&store, "SELECT MAX(attempts) FROM extract_queue");
        assert!(max_attempts >= 3, "{max_attempts}");
        let failed = failed_sections(&store).unwrap();
        assert!(
            !failed.is_empty() && failed[0].2.contains("not JSON"),
            "{failed:?}"
        );
        let t = tick(&store, &m, Duration::from_secs(20), &|| false).unwrap();
        assert!(t.model_error.is_none() || t.extracted == 0);
    }

    #[test]
    fn degenerate_entity_names_are_dropped_or_truncated() {
        let (_d, store) = indexed_docs();
        let f = FakeOllama::spawn(8);
        f.set_extraction(
            "Freshness",
            serde_json::json!({"entities": [
            {"name": "", "type": "person", "description": "x"},
            {"name": "!!!", "type": "person", "description": "x"},
            {"name": "A".repeat(500), "type": "person", "description": "x"},
            {"name": "Jonas", "type": "person", "description": "first"},
            {"name": "jonas ", "type": "person", "description": "second"}
        ], "relations": [{"source": "Jonas", "target": "Jonas", "description": "self"}]}),
        );
        let m = models(&f);
        while pending(&store).unwrap() > 0 {
            tick(&store, &m, Duration::from_secs(20), &|| false).unwrap();
        }
        assert_eq!(
            count(&store, "SELECT COUNT(*) FROM entities WHERE norm_name = ''"),
            0
        );
        assert_eq!(
            count(
                &store,
                "SELECT COUNT(*) FROM entities WHERE norm_name = 'jonas'"
            ),
            1
        );
        let d: String = store
            .conn()
            .query_row(
                "SELECT description FROM entities WHERE norm_name = 'jonas'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(d, "first; second");
        assert_eq!(count(&store, "SELECT MAX(LENGTH(name)) FROM entities"), 80);
        assert_eq!(
            count(&store, "SELECT COUNT(*) FROM relations"),
            0,
            "self relation dropped"
        );
    }

    #[test]
    fn a_dimension_change_rederives_entities_from_the_extraction_cache() {
        let (dir, store) = indexed_docs();
        let f = FakeOllama::spawn(8);
        f.set_extraction("Storage", serde_json::json!({"entities": [{"name": "SessionStore", "type": "system", "description": "keeps sessions"}], "relations": []}));
        let m = models(&f);
        while pending(&store).unwrap() > 0 {
            tick(&store, &m, Duration::from_secs(20), &|| false).unwrap();
        }
        // The new dimension is only seen at the next embedding, so give the queue something.
        std::fs::write(
            dir.path().join("docs/new.md"),
            "# New\n\nA fresh section.\n",
        )
        .unwrap();
        let ws = Workspace::single(dir.path()).unwrap();
        Indexer::new(&store, &ws, &MapConfig::default())
            .unwrap()
            .refresh(None)
            .unwrap();
        let g = FakeOllama::spawn(16);
        let m16 = models(&g);
        let t = tick(&store, &m16, Duration::from_secs(20), &|| false).unwrap();
        assert!(t.model_error.is_none(), "{t:?}");
        assert_eq!(
            store.get_meta("embeddings_rebuilding").unwrap().as_deref(),
            Some("1")
        );
        assert!(
            count(&store, "SELECT COUNT(*) FROM extract_queue WHERE hash = ''") > 0,
            "the rebuild re-queued without hashes"
        );
        tick(&store, &m16, Duration::from_secs(20), &|| false).unwrap();
        assert_eq!(
            count(&store, "SELECT COUNT(*) FROM extract_queue WHERE hash = ''"),
            0,
            "hashes recomputed"
        );
        while pending(&store).unwrap() > 0 {
            tick(&store, &m16, Duration::from_secs(20), &|| false).unwrap();
        }
        assert_eq!(
            g.calls()
                .iter()
                .filter(|p| p.as_str() == "/api/generate")
                .count(),
            1,
            "only the new section went to the model"
        );
        assert_eq!(
            count(
                &store,
                "SELECT COUNT(*) FROM entities WHERE norm_name = 'sessionstore'"
            ),
            1
        );
        assert_eq!(count(&store, "SELECT COUNT(*) FROM entity_vec"), 1);
        assert_eq!(
            count(&store, "SELECT COUNT(*) FROM section_embeddings"),
            count(&store, "SELECT COUNT(*) FROM sections_fts")
        );
        assert_eq!(store.get_meta("embeddings_rebuilding").unwrap(), None);
    }

    #[test]
    fn an_outage_lands_in_model_error_and_writes_nothing() {
        let (_d, store) = indexed_docs();
        let f = FakeOllama::spawn(8);
        f.set_down(true);
        let m = models(&f);
        let before = pending(&store).unwrap();
        let t = tick(&store, &m, Duration::from_secs(20), &|| false).unwrap();
        assert!(
            t.model_error.is_some() && t.embedded == 0 && t.pending == before,
            "{t:?}"
        );
        assert!(store.get_meta("models_error").unwrap().is_some());
        f.set_down(false);
        let t = tick(&store, &m, Duration::from_secs(20), &|| false).unwrap();
        assert!(t.model_error.is_none() && t.embedded > 0, "{t:?}");
        assert_eq!(store.get_meta("models_error").unwrap(), None);
    }

    #[test]
    fn a_tick_that_must_yield_touches_nothing_and_keeps_the_outage_flag() {
        let (_d, store) = indexed_docs();
        let f = FakeOllama::spawn(8);
        let m = models(&f);
        store.set_meta("models_error", "earlier outage").unwrap();
        let before = pending(&store).unwrap();
        let t = tick(&store, &m, Duration::from_secs(20), &|| true).unwrap();
        assert!(
            t.embedded == 0 && t.extracted == 0 && t.pending == before,
            "{t:?}"
        );
        assert!(f.calls().is_empty(), "{:?}", f.calls());
        assert!(store.get_meta("models_error").unwrap().is_some());
        // Yielding after the first extraction stops the tick there.
        let asked = std::cell::Cell::new(0);
        let t = tick(&store, &m, Duration::from_secs(20), &|| {
            asked.set(asked.get() + 1);
            asked.get() > 2
        })
        .unwrap();
        assert!(t.embedded > 0 && t.extracted == 1, "{t:?}");
    }

    #[test]
    fn a_dimension_change_seen_at_extraction_rebuilds_instead_of_stalling() {
        let (_d, store) = indexed_docs();
        let sections = count(&store, "SELECT COUNT(*) FROM sections_fts");
        let f = FakeOllama::spawn(8);
        let t = tick(&store, &models(&f), Duration::ZERO, &|| false).unwrap();
        assert_eq!(
            t.embedded as i64, sections,
            "every section embedded at 8 dims"
        );
        assert_eq!(
            t.extracted, 0,
            "a zero budget leaves the whole extraction backlog"
        );
        drop(f);
        let g = FakeOllama::spawn(16);
        g.set_extraction("", serde_json::json!({"entities": [{"name": "Thing", "type": "concept", "description": "in every section"}], "relations": []}));
        let m = models(&g);
        let t = tick(&store, &m, Duration::from_secs(20), &|| false).unwrap();
        assert!(t.model_error.is_none(), "{t:?}");
        assert_eq!(crate::store::vec::dim(&store).unwrap(), Some(16));
        assert_eq!(
            store.get_meta("embeddings_rebuilding").unwrap().as_deref(),
            Some("1")
        );
        assert_eq!(
            count(&store, "SELECT COUNT(*) FROM extract_queue"),
            sections,
            "re-queued"
        );
        while pending(&store).unwrap() > 0 {
            let t = tick(&store, &m, Duration::from_secs(20), &|| false).unwrap();
            assert!(t.model_error.is_none(), "{t:?}");
        }
        let mut q = vec![0.0f32; 16];
        q[0] = 1.0;
        assert_eq!(
            crate::store::vec::knn(&store, "entity_vec", &q, 1)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            count(
                &store,
                "SELECT mentions FROM entities WHERE norm_name = 'thing'"
            ),
            sections
        );
        let generates = g
            .calls()
            .iter()
            .filter(|p| p.as_str() == "/api/generate")
            .count();
        assert_eq!(
            generates as i64, sections,
            "the extraction before the rebuild was not asked again"
        );
    }

    #[test]
    fn an_extraction_is_cached_even_when_its_entity_embedding_fails() {
        let (_d, store) = indexed_docs();
        let sections = count(&store, "SELECT COUNT(*) FROM sections_fts");
        let f = FakeOllama::spawn(8);
        let m = models(&f);
        tick(&store, &m, Duration::ZERO, &|| false).unwrap();
        f.set_extraction("Freshness", serde_json::json!({"entities": [{"name": "STALE header", "type": "concept", "description": "says files changed"}], "relations": []}));
        f.corrupt_next_embed(1);
        let t = loop {
            let t = tick(&store, &m, Duration::from_secs(20), &|| false).unwrap();
            if t.model_error.is_some() {
                break t;
            }
            assert!(
                pending(&store).unwrap() > 0,
                "the corrupt embed was never hit"
            );
        };
        assert!(t.model_error.is_some(), "{t:?}");
        let fresh: i64 = count(&store, "SELECT s.id FROM symbols s JOIN files f ON f.id = s.file_id WHERE f.path = 'docs/design.md' AND s.name = 'Freshness'");
        assert_eq!(
            count(
                &store,
                &format!("SELECT COUNT(*) FROM extract_queue WHERE symbol_id = {fresh}")
            ),
            1,
            "the queue row survived"
        );
        assert_eq!(count(&store, &format!("SELECT COUNT(*) FROM extraction_cache c JOIN extract_queue q ON q.hash = c.hash WHERE q.symbol_id = {fresh}")), 1, "cached anyway");
        while pending(&store).unwrap() > 0 {
            tick(&store, &m, Duration::from_secs(20), &|| false).unwrap();
        }
        let generates = f
            .calls()
            .iter()
            .filter(|p| p.as_str() == "/api/generate")
            .count();
        assert_eq!(
            generates as i64, sections,
            "no section went to the model twice"
        );
        assert_eq!(count(&store, "SELECT COUNT(*) FROM entity_vec"), 1);
    }

    #[test]
    fn norm_name_rules() {
        assert_eq!(norm_name("  Acme   Ltd. "), "acme ltd");
        assert_eq!(norm_name("Jonas!"), "jonas");
        assert_eq!(norm_name("créateSession"), "créatesession");
    }
}
