//! The knowledge layer's write side (knowledge spec §3): the extraction queue the indexer
//! fills, the embedding and extraction caches, and the tick that drains the queue into
//! section vectors, entities, mentions and relations.
//!
//! Model calls happen outside any transaction; every write for one section happens in one.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use rusqlite::{params, Connection, OptionalExtension};

use crate::models::{ExtractedEntity, Extraction, Models, SectionInput, DESCRIPTION_MAX};
use crate::store::{lock, vec, Store};
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
    /// Another process held the indexer lock, so this tick did nothing.
    pub skipped_locked: bool,
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
fn dimension_changed(store: &Store, models: &Models, vectors: &[Vec<f32>]) -> Result<bool> {
    let Some(len) = vectors.first().map(Vec::len) else {
        return Ok(false);
    };
    match vec::dim(store)? {
        Some(d) if d != len => {
            vec::ensure_tables(store, len, &models.config().embed)?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// The index's dimension when its vectors came from another embedding model than `models` uses.
fn swapped_model_dim(store: &Store, models: &Models) -> Result<Option<usize>> {
    if vec::model(store)?.is_some_and(|m| m != models.config().embed) {
        return vec::dim(store);
    }
    Ok(None)
}

fn clear_meta(store: &Store, key: &str) -> Result<()> {
    store
        .conn()
        .execute("DELETE FROM meta WHERE key = ?1", [key])?;
    Ok(())
}

/// Holds the indexer lock for one tick and releases it on every exit path, unless this
/// process already held it when the tick began (then its owner releases it).
struct TickLock<'a> {
    store: &'a Store,
    pid: u32,
    release: bool,
}

impl<'a> TickLock<'a> {
    /// `None` when another live process holds the lock.
    fn acquire(store: &'a Store) -> Result<Option<TickLock<'a>>> {
        let pid = std::process::id();
        let already = lock::holder(store)? == Some(pid);
        if !lock::try_acquire(store, pid, now_ms())? {
            return Ok(None);
        }
        Ok(Some(TickLock {
            store,
            pid,
            release: !already,
        }))
    }
}

impl Drop for TickLock<'_> {
    fn drop(&mut self) {
        if self.release {
            let _ = lock::release(self.store, self.pid);
        }
    }
}

/// One background step: embed queued sections, then extract some under `budget`.
/// A model outage never escapes as an error: it lands in `model_error` and in meta
/// `models_error`, and the tick stops there. `should_yield` is asked before the embed
/// batch and before each extraction: `true` (a job is waiting) ends the tick there.
/// The tick runs under the indexer lock, so two processes on one index never do the
/// same model work; when another process holds it the tick does nothing and says so
/// in `skipped_locked`.
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
    let Some(_lock) = TickLock::acquire(store)? else {
        t.skipped_locked = true;
        t.pending = pending(store)?;
        return Ok(t);
    };
    // No retry on a timeout: one slow section holds the actor for one timeout at most.
    let models = &models.tick_client();
    if let Some(d) = swapped_model_dim(store, models)? {
        // A different embedding model at any dimension: the same rebuild a dimension change
        // does, before anything is embedded. The next ticks re-embed with the new model.
        vec::ensure_tables(store, d, &models.config().embed)?;
    } else if embed_step(store, models, &mut t)? {
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
}

/// What a model answering in a different dimension (or a different embedding model) than
/// the index holds means to the caller.
#[derive(Clone, Copy, PartialEq, Eq)]
enum OnNewDim {
    /// The tick: rebuild (clear the vectors, re-queue every section) and end the tick.
    Rebuild,
    /// A load: stop with `ModelUnavailable` and write nothing, so what earlier sections
    /// wrote survives.
    Refuse,
}

/// The section-embedding write the tick and the extraction loader share. Vectors for
/// `items` come from the embedding cache (by hash and model, at the index's dimension); the
/// misses and the `extra` texts go to the model in one call. The first vectors ever create
/// the tables. A different dimension or embedding model is handled as `on_new_dim` says; otherwise `section_vec`,
/// `section_embeddings` and `embedding_cache` are written for every item in one
/// transaction (and a queue row's empty rebuild hash is filled in). Returns the vectors
/// for `extra`, in order, or `None` when a rebuild ended the step. A model outage is an
/// `Err(ModelUnavailable)` with nothing written.
fn embed_sections(
    store: &Store,
    models: &Models,
    items: &[ToEmbed],
    extra: &[String],
    on_new_dim: OnNewDim,
) -> Result<Option<Vec<Vec<f32>>>> {
    let conn = store.conn();
    let model = models.config().embed.as_str();
    let dim_before = vec::dim(store)?;
    if let Some(d) = swapped_model_dim(store, models)? {
        return match on_new_dim {
            OnNewDim::Rebuild => {
                vec::ensure_tables(store, d, model)?;
                Ok(None)
            }
            OnNewDim::Refuse => Err(Error::ModelUnavailable(format!(
                "embeddings from {model} for an index built with {}",
                vec::model(store)?.unwrap_or_default()
            ))),
        };
    }
    let mut vectors: Vec<Option<Vec<f32>>> = Vec::with_capacity(items.len());
    for it in items {
        let cached: Option<(i64, Vec<u8>)> = conn
            .query_row(
                "SELECT dim, blob FROM embedding_cache WHERE hash = ?1 AND model = ?2",
                params![it.hash, model],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        vectors.push(
            cached
                .filter(|(d, _)| dim_before == Some(*d as usize))
                .map(|(_, b)| vec::from_blob(&b)),
        );
    }

    let misses: Vec<usize> = (0..items.len()).filter(|&i| vectors[i].is_none()).collect();
    let mut extra_vectors = Vec::new();
    if misses.is_empty() && extra.is_empty() {
        if let Some(d) = dim_before {
            vec::ensure_tables(store, d, model)?;
        }
    } else {
        let texts: Vec<String> = misses
            .iter()
            .map(|&i| items[i].body.clone())
            .chain(extra.iter().cloned())
            .collect();
        let mut answered = models.embed(&texts)?;
        if answered.len() != texts.len() {
            return Err(Error::ModelUnavailable(format!(
                "embed: {} vectors for {} texts",
                answered.len(),
                texts.len()
            )));
        }
        let d = answered.first().map_or(0, Vec::len);
        if d == 0 {
            return Err(Error::ModelUnavailable("embed: empty vector".to_string()));
        }
        if let Some(before) = dim_before.filter(|&b| b != d) {
            return match on_new_dim {
                OnNewDim::Rebuild => {
                    vec::ensure_tables(store, d, model)?;
                    Ok(None)
                }
                OnNewDim::Refuse => Err(Error::ModelUnavailable(format!(
                    "embedding of {d} dims for a {before}-dim index"
                ))),
            };
        }
        // First vectors ever: this creates the tables; otherwise a cheap no-op.
        vec::ensure_tables(store, d, model)?;
        extra_vectors = answered.split_off(misses.len());
        for (i, v) in misses.into_iter().zip(answered) {
            vectors[i] = Some(v);
        }
    }

    let tx = conn.unchecked_transaction()?;
    for (it, v) in items.iter().zip(&vectors) {
        let Some(v) = v else { continue };
        vec::insert(store, "section_vec", it.symbol_id, v)?;
        tx.execute(
            "INSERT OR REPLACE INTO section_embeddings(symbol_id, hash) VALUES (?1, ?2)",
            params![it.symbol_id, it.hash],
        )?;
        tx.execute(
            "INSERT OR REPLACE INTO embedding_cache(hash, model, dim, blob) VALUES (?1, ?2, ?3, ?4)",
            params![it.hash, model, v.len() as i64, vec::to_blob(v)],
        )?;
        tx.execute(
            "UPDATE extract_queue SET hash = ?2 WHERE symbol_id = ?1 AND hash = ''",
            params![it.symbol_id, it.hash],
        )?;
    }
    tx.commit()?;
    Ok(Some(extra_vectors))
}

/// Queued sections without a vector, up to `EMBED_PER_TICK`, through `embed_sections`.
/// Returns `false` when a model outage or a dimension rebuild ended the tick.
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
        items.push(ToEmbed {
            symbol_id,
            hash,
            body,
        });
    }
    match embed_sections(store, models, &items, &[], OnNewDim::Rebuild) {
        Ok(Some(_)) => {
            t.embedded += items.len();
            Ok(true)
        }
        Ok(None) => Ok(false),
        Err(Error::ModelUnavailable(m)) => {
            record_outage(store, t, m)?;
            Ok(false)
        }
        Err(e) => Err(e),
    }
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
        // One extraction can outlast `LOCK_STALE_MS`; keep the tick's lock fresh between them.
        lock::heartbeat(store, std::process::id(), now_ms())?;
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
                    if dimension_changed(store, models, &v)? {
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

/// Applies a checked-in `{ "<path>::<heading>": Extraction }` map to the indexed sections it
/// names, as the tick would have: the extraction (normalised) goes in with the section's hash,
/// the extraction cache remembers it, and the queue row goes. With `models`, the section and
/// the new entities and relations are embedded as well; without, they stay unembedded.
/// Keys naming no section with a body are skipped with a warning. Returns sections applied.
pub fn load_extraction_json(store: &Store, models: Option<&Models>, json: &str) -> Result<usize> {
    let map: std::collections::BTreeMap<String, Extraction> =
        serde_json::from_str(json).map_err(|e| Error::Config(format!("extraction json: {e}")))?;
    let conn = store.conn();
    let mut applied = 0;
    let mut unknown = Vec::new();
    for (key, x) in map {
        let found = match key.split_once("::") {
            Some((path, name)) => conn
                .query_row(
                    "SELECT s.id, t.content FROM symbols s JOIN files f ON f.id = s.file_id
                     JOIN sections_fts t ON t.rowid = s.id
                     WHERE f.path = ?1 AND s.name = ?2 ORDER BY s.id LIMIT 1",
                    [path, name],
                    |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
                )
                .optional()?,
            None => None,
        };
        let Some((symbol_id, body)) = found else {
            unknown.push(key);
            continue;
        };
        let x = crate::models::normalise_extraction(x);
        let hash = section_hash(&body);
        let vectors = match models {
            Some(m) => embed_loaded_section(store, m, symbol_id, &hash, &body, &x)?,
            None => HashMap::new(),
        };
        conn.execute(
            "INSERT OR REPLACE INTO extraction_cache(hash, json) VALUES (?1, ?2)",
            params![hash, extraction_json(&x)?],
        )?;
        write_section(store, symbol_id, &hash, &x, &vectors)?;
        applied += 1;
    }
    if !unknown.is_empty() {
        tracing::warn!(
            skipped = unknown.len(),
            keys = ?unknown,
            "extraction keys name no indexed section"
        );
    }
    Ok(applied)
}

/// A loaded section's vector and those of its new entities and relations, through the
/// tick's `embed_sections`; a dimension change stops the load instead of rebuilding.
fn embed_loaded_section(
    store: &Store,
    models: &Models,
    symbol_id: i64,
    hash: &str,
    body: &str,
    x: &Extraction,
) -> Result<HashMap<String, Vec<f32>>> {
    let texts = vector_texts(store.conn(), x)?;
    let item = ToEmbed {
        symbol_id,
        hash: hash.to_string(),
        body: body.to_string(),
    };
    let vectors = embed_sections(store, models, &[item], &texts, OnNewDim::Refuse)?
        .expect("Refuse never rebuilds");
    Ok(texts.into_iter().zip(vectors).collect())
}

/// Where `needle` first occurs in `hay` with no word character directly before or after it.
fn phrase_position(hay: &str, needle: &str) -> Option<usize> {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    hay.match_indices(needle)
        .find(|(i, m)| {
            !hay[..*i].chars().next_back().is_some_and(is_word)
                && !hay[i + m.len()..].chars().next().is_some_and(is_word)
        })
        .map(|(i, _)| i)
}

/// A model outage is an answer (`models_unavailable`), anything else an error.
fn only_unavailable(e: Error) -> Result<()> {
    match e {
        Error::ModelUnavailable(_) => Ok(()),
        e => Err(e),
    }
}

#[derive(Debug, Clone)]
struct MatchedEntity {
    id: i64,
    name: String,
}

/// The entities a query and a list of names match, plus the query's vector.
#[derive(Debug, Default)]
struct EntityMatches {
    entities: Vec<MatchedEntity>,
    query_vec: Option<Vec<f32>>,
    models_unavailable: bool,
}

/// The entity matching rule `repo_map`'s seeds and the `entities` tool share, with at
/// most one model call (the query's embedding). In order: the names in `entities`
/// (exact `norm_name`, argument order); then entities the query names, as a whole-word
/// phrase of `norm_name(query)` or as one of its whitespace tokens, by where they occur
/// in the query; then, with a query vector, the `ENTITY_K` nearest entities at least
/// `ENTITY_MIN_SIM` close, most similar first. Ties go to the entity with more mentions.
/// Name matching needs no model; with no vectors in the index the model is not asked.
fn matching_entities(
    store: &Store,
    models: Option<&Models>,
    query: Option<&str>,
    entities: &[String],
) -> Result<EntityMatches> {
    let mut out = EntityMatches::default();
    let conn = store.conn();
    let push = |out: &mut EntityMatches, id: i64, name: &str| {
        if !out.entities.iter().any(|m| m.id == id) {
            out.entities.push(MatchedEntity {
                id,
                name: name.to_string(),
            });
        }
    };

    // Candidates in one query, matched in Rust: the arguments by exact `norm_name`,
    // the query by whole-word phrase or by one of its whitespace tokens.
    let candidates: Vec<(i64, String, String, i64)> = {
        let mut stmt = conn.prepare(
            "SELECT id, name, norm_name, mentions FROM entities ORDER BY mentions DESC, id",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
        rows.collect::<std::result::Result<_, _>>()?
    };
    for arg in entities
        .iter()
        .map(|e| norm_name(e))
        .filter(|e| !e.is_empty())
    {
        if let Some((id, name, _, _)) = candidates.iter().find(|(_, _, n, _)| *n == arg) {
            push(&mut out, *id, name);
        }
    }
    if let Some(q) = query {
        let phrase = norm_name(q);
        let tokens: Vec<String> = q.split_whitespace().map(norm_name).collect();
        // (position in the query, candidate index): candidates are already by mentions.
        let mut named: Vec<(usize, usize)> = candidates
            .iter()
            .enumerate()
            .filter(|(_, (_, _, n, _))| !n.is_empty())
            .filter_map(|(i, (_, _, n, _))| {
                phrase_position(&phrase, n)
                    .or_else(|| {
                        tokens
                            .contains(n)
                            .then(|| phrase.find(n.as_str()).unwrap_or(usize::MAX))
                    })
                    .map(|pos| (pos, i))
            })
            .collect();
        named.sort();
        for (_, i) in named {
            let (id, name, _, _) = &candidates[i];
            push(&mut out, *id, name);
        }
    }

    let Some(models) = models else {
        return Ok(out);
    };
    if vec::dim(store)?.is_none() {
        return Ok(out);
    }
    if let Some(q) = query.filter(|q| !q.trim().is_empty()) {
        match models.embed(&[q.to_string()]) {
            Ok(mut v) if v.len() == 1 => out.query_vec = v.pop(),
            Ok(_) => out.models_unavailable = true,
            Err(e) => {
                only_unavailable(e)?;
                out.models_unavailable = true;
            }
        }
    }
    if let Some(qv) = &out.query_vec {
        let mentions = |id: i64| candidates.iter().find(|c| c.0 == id).map_or(0, |c| c.3);
        let mut near: Vec<(i64, f64)> = vec::knn(store, "entity_vec", qv, ENTITY_K)?
            .into_iter()
            .filter(|(_, sim)| *sim >= ENTITY_MIN_SIM)
            .collect();
        near.sort_by(|a, b| {
            b.1.total_cmp(&a.1)
                .then_with(|| mentions(b.0).cmp(&mentions(a.0)))
        });
        for (id, _) in near {
            if let Some((_, name, _, _)) = candidates.iter().find(|c| c.0 == id) {
                push(&mut out, id, name);
            }
        }
    }
    Ok(out)
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
/// the query, one batch for all themes (skipped once the first has failed). A model
/// outage never escapes: it sets `models_unavailable` and leaves the vectors empty;
/// entities already matched, by name or by vector before the failure, are kept.
/// Entity names match when an argument equals one (after `norm_name`) or when the
/// query contains one as a whole-word phrase; that needs no model, so it also works
/// with models disabled. With no vectors in the index yet nothing could be matched by
/// vector, so the models are not asked.
pub fn seeds_for(
    store: &Store,
    models: Option<&Models>,
    query: Option<&str>,
    entities: &[String],
    themes: &[String],
) -> Result<Seeds> {
    let mut seeds = Seeds::default();
    let matched = matching_entities(store, models, query, entities)?;
    for m in matched.entities {
        seeds.add_entity(m.id, m.name);
    }
    seeds.query_vec = matched.query_vec;
    seeds.models_unavailable = matched.models_unavailable;

    let Some(models) = models else {
        return Ok(seeds);
    };
    if vec::dim(store)?.is_none() {
        return Ok(seeds);
    }

    let themes: Vec<String> = themes
        .iter()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();
    // After a failed query embed a second call would only wait out another timeout.
    if !themes.is_empty() && !seeds.models_unavailable {
        match models.embed(&themes) {
            Ok(v) if v.len() == themes.len() => {
                seeds.theme_vecs = themes.into_iter().zip(v).collect();
            }
            Ok(_) => seeds.models_unavailable = true,
            Err(e) => {
                only_unavailable(e)?;
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

/// Relations the `entities` tool shows, over all matched entities together.
pub const ENTITY_RELATIONS_MAX: usize = 30;

/// A document section that states something about an entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CitedSection {
    pub symbol_id: i64,
    pub path: String,
    pub name: String,
    pub line_start: u32,
    pub line_end: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationHit {
    pub src: String,
    pub dst: String,
    pub description: String,
    pub section: CitedSection,
}

/// One entity the `entities` tool answers with: its relations (either end) and the
/// sections that mention it or state one of its relations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityHit {
    pub id: i64,
    pub name: String,
    pub r#type: String,
    pub description: String,
    pub mentions: usize,
    pub relations: Vec<RelationHit>,
    pub sections: Vec<CitedSection>,
}

fn cited_section(conn: &Connection, symbol_id: i64) -> Result<Option<CitedSection>> {
    Ok(conn
        .query_row(
            "SELECT f.path, s.name, s.line_start, s.line_end FROM symbols s
             JOIN files f ON f.id = s.file_id WHERE s.id = ?1",
            [symbol_id],
            |r| {
                Ok(CitedSection {
                    symbol_id,
                    path: r.get(0)?,
                    name: r.get(1)?,
                    line_start: r.get(2)?,
                    line_end: r.get(3)?,
                })
            },
        )
        .optional()?)
}

/// The `entities` tool's answer: up to `limit` entities matched by the rule
/// `repo_map`'s entity seeds use (`extra` are exact names), each with its relations
/// (at most `ENTITY_RELATIONS_MAX` over all entities; a relation between two matched
/// entities is shown once, under the first) and its sections, mentions first in
/// document order, deduped. The only model call is the query's embedding; the bool is
/// true when that call failed, and name matches still answer.
pub fn match_entities(
    store: &Store,
    models: Option<&Models>,
    query: &str,
    extra: &[String],
    limit: usize,
) -> Result<(Vec<EntityHit>, bool)> {
    let matched = matching_entities(store, models, Some(query), extra)?;
    let conn = store.conn();
    let mut entity =
        conn.prepare("SELECT name, type, description, mentions FROM entities WHERE id = ?1")?;
    let mut mentioned = conn.prepare(
        "SELECT m.symbol_id FROM entity_mentions m JOIN symbols s ON s.id = m.symbol_id
         JOIN files f ON f.id = s.file_id WHERE m.entity_id = ?1 ORDER BY f.path, s.line_start",
    )?;
    let mut related = conn.prepare(
        "SELECT r.id, se.name, de.name, r.description, r.symbol_id FROM relations r
         JOIN entities se ON se.id = r.src_entity JOIN entities de ON de.id = r.dst_entity
         WHERE r.src_entity = ?1 OR r.dst_entity = ?1 ORDER BY r.id",
    )?;
    let mut shown_relations: HashSet<i64> = HashSet::new();
    let mut hits = Vec::new();
    for m in matched.entities.into_iter().take(limit) {
        let Some((name, r#type, description, mentions)) = entity
            .query_row([m.id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            })
            .optional()?
        else {
            continue;
        };
        let mut sections: Vec<CitedSection> = Vec::new();
        let ids: Vec<i64> = mentioned
            .query_map([m.id], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        for id in ids {
            if let Some(c) = cited_section(conn, id)? {
                if !sections.contains(&c) {
                    sections.push(c);
                }
            }
        }
        let rows: Vec<(i64, String, String, String, i64)> = related
            .query_map([m.id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })?
            .collect::<std::result::Result<_, _>>()?;
        let mut relations = Vec::new();
        for (rid, src, dst, desc, symbol_id) in rows {
            if shown_relations.len() >= ENTITY_RELATIONS_MAX {
                break;
            }
            if shown_relations.contains(&rid) {
                continue;
            }
            let Some(section) = cited_section(conn, symbol_id)? else {
                continue;
            };
            shown_relations.insert(rid);
            if !sections.contains(&section) {
                sections.push(section.clone());
            }
            relations.push(RelationHit {
                src,
                dst,
                description: desc,
                section,
            });
        }
        hits.push(EntityHit {
            id: m.id,
            name,
            r#type,
            description,
            mentions: mentions.max(0) as usize,
            relations,
            sections,
        });
    }
    Ok((hits, matched.models_unavailable))
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The `entities` block: per entity a `name (type): description` line, its sections as
/// `  ← path::heading (lines a-b)` and its relations as
/// `  src → dst: description (path::heading, lines a-b)`, then a count footer.
pub fn render_entities(hits: &[EntityHit]) -> String {
    let mut out = String::new();
    let mut relations = 0;
    let mut sections: Vec<i64> = Vec::new();
    for h in hits {
        out.push_str(&format!("{} ({})", h.name, h.r#type));
        if !h.description.is_empty() {
            out.push_str(&format!(": {}", h.description));
        }
        out.push('\n');
        for s in &h.sections {
            out.push_str(&format!(
                "  ← {}::{} (lines {}-{})\n",
                s.path, s.name, s.line_start, s.line_end
            ));
            if !sections.contains(&s.symbol_id) {
                sections.push(s.symbol_id);
            }
        }
        for r in &h.relations {
            let s = &r.section;
            out.push_str(&format!(
                "  {} → {}: {} ({}::{}, lines {}-{})\n",
                r.src, r.dst, r.description, s.path, s.name, s.line_start, s.line_end
            ));
        }
        relations += h.relations.len();
    }
    out.push_str(&format!(
        "# {} · {} · {}\n",
        plural(hits.len(), "entity", "entities"),
        plural(relations, "relation", "relations"),
        plural(sections.len(), "section", "sections")
    ));
    out
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
    fn a_tick_makes_one_generate_attempt_when_it_times_out() {
        let (_d, store) = indexed_docs();
        let f = FakeOllama::spawn(8);
        let m = Models::with_timeout(
            ModelsConfig {
                ollama: f.url(),
                ..Default::default()
            },
            Duration::from_millis(300),
        );
        f.set_generate_delay(Duration::from_millis(800));
        let t = tick(&store, &m, Duration::from_secs(20), &|| false).unwrap();
        assert!(t.model_error.is_some(), "a timeout is an outage: {t:?}");
        // The fake answers one request at a time: a retry would be read once the first
        // delayed answer is written, so wait past that before counting.
        std::thread::sleep(Duration::from_millis(2000));
        let generate = f
            .calls()
            .iter()
            .filter(|p| p.as_str() == "/api/generate")
            .count();
        assert_eq!(generate, 1, "no retry on a timeout inside a tick");
    }

    #[test]
    fn a_tick_does_nothing_while_another_process_holds_the_lock() {
        use crate::store::lock;
        let (_d, store) = indexed_docs();
        let f = FakeOllama::spawn(8);
        let m = models(&f);
        let before = pending(&store).unwrap();
        let other = std::process::id().wrapping_add(1);
        assert!(lock::try_acquire(&store, other, now_ms()).unwrap());
        let t = tick(&store, &m, Duration::from_secs(20), &|| false).unwrap();
        assert!(f.calls().is_empty(), "no model call: {:?}", f.calls());
        assert_eq!(t.pending, before, "{t:?}");
        assert_eq!(t.embedded + t.extracted, 0, "{t:?}");
        assert!(t.skipped_locked, "{t:?}");
        lock::release(&store, other).unwrap();
        let t = tick(&store, &m, Duration::from_secs(20), &|| false).unwrap();
        assert!(t.embedded > 0 && t.extracted > 0, "{t:?}");
        assert!(
            lock::try_acquire(&store, other, now_ms()).unwrap(),
            "the tick released the lock"
        );
    }

    #[test]
    fn a_model_swap_at_the_same_dimension_rebuilds_and_misses_the_cache() {
        let (_d, store) = indexed_docs();
        let sections = count(&store, "SELECT COUNT(*) FROM sections_fts");
        let f = FakeOllama::spawn(8);
        let t = tick(&store, &models(&f), Duration::ZERO, &|| false).unwrap();
        assert_eq!(t.embedded as i64, sections);
        let embeds = |f: &FakeOllama| {
            f.calls()
                .iter()
                .filter(|p| p.as_str() == "/api/embed")
                .count()
        };
        let before = embeds(&f);
        let swapped = Models::new(ModelsConfig {
            ollama: f.url(),
            embed: "other-embed".into(),
            ..Default::default()
        });
        tick(&store, &swapped, Duration::ZERO, &|| false).unwrap();
        assert_eq!(
            crate::store::vec::dim(&store).unwrap(),
            Some(8),
            "same dimension"
        );
        assert_eq!(
            store.get_meta("embed_model").unwrap().as_deref(),
            Some("other-embed")
        );
        assert_eq!(
            store.get_meta("embeddings_rebuilding").unwrap().as_deref(),
            Some("1")
        );
        while count(&store, "SELECT COUNT(*) FROM section_embeddings") < sections {
            let t = tick(&store, &swapped, Duration::ZERO, &|| false).unwrap();
            assert!(t.model_error.is_none(), "{t:?}");
            assert!(
                embeds(&f) > before,
                "the old model's vectors are a cache miss"
            );
        }
        assert_eq!(count(&store, "SELECT COUNT(*) FROM section_vec"), sections);
        assert_eq!(
            count(
                &store,
                "SELECT COUNT(*) FROM embedding_cache WHERE model = 'other-embed'"
            ),
            count(&store, "SELECT COUNT(*) FROM embedding_cache")
        );
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
    fn a_failed_query_embed_skips_the_themes_call() {
        let (_d, store, f, m) = crate::rank::tests::knowledge_ready();
        let embeds = || f.calls().iter().filter(|c| *c == "/api/embed").count();
        let before = embeds();
        f.corrupt_next_embed(1);
        let s = seeds_for(&store, Some(&m), Some("stale header"), &[], &["y".into()]).unwrap();
        assert!(s.models_unavailable && s.query_vec.is_none() && s.theme_vecs.is_empty());
        assert_eq!(embeds() - before, 1, "one failed call, no second timeout");
        assert!(
            s.entity_names.contains(&"STALE header".to_string()),
            "name matches are kept"
        );
    }

    #[test]
    fn entity_names_match_as_whole_phrases_in_the_query_without_models() {
        let (_d, store, _f, _m) = crate::rank::tests::knowledge_ready();
        let names = |q: &str| {
            seeds_for(&store, None, Some(q), &[], &[])
                .unwrap()
                .entity_names
        };
        assert!(names("where is SessionStore created").contains(&"SessionStore".to_string()));
        assert!(names("what is the STALE header").contains(&"STALE header".to_string()));
        let session = names("session");
        assert!(
            !session
                .iter()
                .any(|n| n == "SessionStore" || n == "STALE header"),
            "{session:?}"
        );
        // "headers" is not the word "header".
        assert!(!names("stale headers").contains(&"STALE header".to_string()));
    }

    #[test]
    fn match_and_render_entities_with_their_relations_and_sections() {
        let (_d, store, _f, m) = crate::rank::tests::knowledge_ready(); // make that helper pub(crate)
        let (hits, off) = match_entities(&store, Some(&m), "stale header", &[], 10).unwrap();
        assert!(!off);
        assert_eq!(hits[0].name, "STALE header");
        assert_eq!(hits[0].relations.len(), 1);
        assert_eq!(hits[0].sections[0].name, "Freshness");
        let text = render_entities(&hits);
        assert!(text.starts_with("STALE header (concept): the header when files changed\n  ← docs/design.md::Freshness (lines "), "{text}");
        assert!(text.contains("STALE header → createSession: refresh runs before session creation (docs/design.md::Freshness, lines "), "{text}");
        // One entity citing one section here, so the footer is singular throughout.
        assert!(
            text.ends_with("\n# 1 entity · 1 relation · 1 section\n"),
            "{text}"
        );
        let (none, _) = match_entities(&store, None, "nothing matches this", &[], 10).unwrap();
        assert!(none.is_empty());
        assert_eq!(
            render_entities(&none),
            "# 0 entities · 0 relations · 0 sections\n"
        );
    }

    #[test]
    fn norm_name_rules() {
        assert_eq!(norm_name("  Acme   Ltd. "), "acme ltd");
        assert_eq!(norm_name("Jonas!"), "jonas");
        assert_eq!(norm_name("créateSession"), "créatesession");
    }

    #[test]
    fn the_prose_fixture_loads_its_extraction_without_a_model() {
        let dir = tempfile::tempdir().unwrap();
        crate::fixture::write_prose(dir.path());
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        let ws = Workspace::single(dir.path()).unwrap();
        Indexer::new(&store, &ws, &MapConfig::default())
            .unwrap()
            .refresh(None)
            .unwrap();
        let n = load_extraction_json(
            &store,
            None,
            include_str!("../fixtures/prose/extraction.json"),
        )
        .unwrap();
        assert!(n >= 9, "{n}");
        assert!(count(&store, "SELECT COUNT(*) FROM entities") >= 12);
        assert!(count(&store, "SELECT COUNT(*) FROM relations") >= 10);
        assert_eq!(pending(&store).unwrap(), 0);
        // Without a model only names match, so the query finds the ship, and the captain
        // comes with it as a relation.
        let (hits, _) = match_entities(&store, None, "who captained the Aurelia", &[], 5).unwrap();
        assert!(
            hits.iter().any(|h| h.name == "Ingrid Halvorsen"
                || h.relations.iter().any(|r| r.src == "Ingrid Halvorsen"
                    && r.dst == "Aurelia"
                    && r.description == "captain of the Aurelia")),
            "{hits:?}"
        );
        assert_eq!(count(&store, "SELECT COUNT(*) FROM extraction_cache"), 10);
        assert_eq!(
            count(&store, "SELECT COUNT(*) FROM section_embeddings"),
            0,
            "no model, no vectors"
        );
        // Keys naming no section are skipped, not an error.
        let n = load_extraction_json(
            &store,
            None,
            r#"{"docs/voyage.md::No such heading": {"entities": [], "relations": []}, "nonsense": {}}"#,
        )
        .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn a_dimension_change_mid_load_stops_the_load_and_keeps_what_was_loaded() {
        let dir = tempfile::tempdir().unwrap();
        crate::fixture::write_prose(dir.path());
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        let ws = Workspace::single(dir.path()).unwrap();
        Indexer::new(&store, &ws, &MapConfig::default())
            .unwrap()
            .refresh(None)
            .unwrap();
        let all: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(include_str!("../fixtures/prose/extraction.json")).unwrap();
        let (voyage, handbook): (Vec<_>, Vec<_>) = all
            .into_iter()
            .partition(|(k, _)| k.starts_with("docs/voyage.md::"));
        let json = |part: Vec<(String, serde_json::Value)>| {
            serde_json::Value::Object(part.into_iter().collect()).to_string()
        };

        let f8 = FakeOllama::spawn(8);
        let n = load_extraction_json(&store, Some(&models(&f8)), &json(voyage)).unwrap();
        assert_eq!(n, 5);
        let entities = count(&store, "SELECT COUNT(*) FROM entities");
        let relations = count(&store, "SELECT COUNT(*) FROM relations");
        assert!(entities > 0 && relations > 0);

        let f16 = FakeOllama::spawn(16);
        let err = load_extraction_json(&store, Some(&models(&f16)), &json(handbook)).unwrap_err();
        assert!(matches!(err, Error::ModelUnavailable(_)), "{err:?}");
        assert_eq!(count(&store, "SELECT COUNT(*) FROM entities"), entities);
        assert_eq!(count(&store, "SELECT COUNT(*) FROM relations"), relations);
        assert_eq!(count(&store, "SELECT COUNT(*) FROM section_vec"), 5);
        assert_eq!(vec::dim(&store).unwrap(), Some(8), "no rebuild");
        assert_eq!(
            pending(&store).unwrap(),
            5,
            "the handbook sections stay queued"
        );
        assert!(store.get_meta("embeddings_rebuilding").unwrap().is_none());
    }

    #[test]
    fn loading_the_prose_extraction_with_a_model_embeds_sections_entities_and_relations() {
        let dir = tempfile::tempdir().unwrap();
        crate::fixture::write_prose(dir.path());
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        let ws = Workspace::single(dir.path()).unwrap();
        Indexer::new(&store, &ws, &MapConfig::default())
            .unwrap()
            .refresh(None)
            .unwrap();
        let f = FakeOllama::spawn(8);
        let m = models(&f);
        let n = load_extraction_json(
            &store,
            Some(&m),
            include_str!("../fixtures/prose/extraction.json"),
        )
        .unwrap();
        assert_eq!(n, 10);
        assert_eq!(pending(&store).unwrap(), 0);
        assert_eq!(count(&store, "SELECT COUNT(*) FROM section_embeddings"), 10);
        assert_eq!(count(&store, "SELECT COUNT(*) FROM section_vec"), 10);
        assert_eq!(
            count(&store, "SELECT COUNT(*) FROM entity_vec"),
            count(&store, "SELECT COUNT(*) FROM entities")
        );
        assert_eq!(
            count(&store, "SELECT COUNT(*) FROM relation_vec"),
            count(&store, "SELECT COUNT(*) FROM relations")
        );
    }
}
