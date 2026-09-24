//! Personalised PageRank over the file graph, then distribution of file rank to the
//! symbols it defines. Every score carries a `Reasons` the UI can draw.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::config::MapConfig;
use crate::graph::{build_graph, query_ident_match, FileGraph};
use crate::knowledge::{Seeds, SEMANTIC_K, THEME_K};
use crate::store::Store;
use crate::tokens::query_terms;
use crate::Result;

pub const DAMPING: f64 = 0.85;
pub const ITERATIONS: usize = 50;
pub const FOCUS_BOOST: f64 = 10.0;
pub const PIN_BOOST: f64 = 10.0;
pub const FTS_FILE_BOOST: f64 = 5.0;
pub const NOTE_BOOST: f64 = 5.0;
/// Symbols nobody references still get a sliver of their file's rank so they stay orderable.
pub const UNREFERENCED_FRACTION: f64 = 0.001;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RefBy {
    pub path: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct Reasons {
    /// The symbol's final score. (`file_rank` below is the PageRank value itself.)
    pub score: f64,
    pub file_rank: f64,
    pub seeds: Vec<String>,
    pub referenced_by: Vec<RefBy>,
    pub pinned: bool,
    pub fts_hit: bool,
    pub query_ident_match: bool,
    pub note_hit: bool,
    pub body_hit: bool,
    /// Similarity of the section to the query's vector, when it was among the nearest.
    pub semantic: Option<f64>,
    /// Entities the section mentions that the query or the caller named.
    pub entities: Vec<String>,
    /// Themes whose nearest relations this section states.
    pub themes: Vec<String>,
    /// Processes whose steps this code symbol implements.
    #[serde(default)]
    pub implements: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScoredSymbol {
    pub symbol_id: i64,
    pub file_id: i64,
    pub path: String,
    pub name: String,
    pub kind: String,
    pub line_start: u32,
    pub line_end: u32,
    pub signature: String,
    pub score: f64,
    pub reasons: Reasons,
}

/// Power iteration. `personalization` is normalised; all-zero means uniform.
/// Dangling mass is redistributed by the personalization vector.
pub fn pagerank(n: usize, edges: &[(usize, usize, f64)], personalization: &[f64]) -> Vec<f64> {
    if n == 0 {
        return Vec::new();
    }
    let psum: f64 = personalization.iter().sum();
    let p: Vec<f64> = if psum > 0.0 {
        personalization.iter().map(|v| v / psum).collect()
    } else {
        vec![1.0 / n as f64; n]
    };
    let mut out_w = vec![0.0; n];
    for &(s, _, w) in edges {
        out_w[s] += w;
    }
    let mut r = p.clone();
    for _ in 0..ITERATIONS {
        let mut next = vec![0.0; n];
        let mut dangling = 0.0;
        for i in 0..n {
            if out_w[i] == 0.0 {
                dangling += r[i];
            }
        }
        for &(s, d, w) in edges {
            next[d] += r[s] * w / out_w[s];
        }
        for i in 0..n {
            next[i] = (1.0 - DAMPING) * p[i] + DAMPING * (next[i] + dangling * p[i]);
        }
        r = next;
    }
    let total: f64 = r.iter().sum();
    if total > 0.0 {
        for v in &mut r {
            *v /= total;
        }
    }
    r
}

struct SymbolRow {
    id: i64,
    file_id: i64,
    name: String,
    kind: String,
    line_start: u32,
    line_end: u32,
    signature: String,
}

fn fts_symbol_ids(store: &Store, terms: &[String]) -> Result<Vec<i64>> {
    let mut ids = Vec::new();
    let mut stmt = store
        .conn()
        .prepare("SELECT rowid FROM symbols_fts WHERE symbols_fts MATCH ?1")?;
    for t in terms {
        let q = format!("{{name name_tokens}}: \"{}\"*", t.replace('"', "\"\""));
        for row in stmt.query_map([q], |r| r.get::<_, i64>(0))? {
            ids.push(row?);
        }
    }
    ids.sort_unstable();
    ids.dedup();
    Ok(ids)
}

/// Symbol ids whose section text matches a query term (prefix, porter-stemmed), with
/// the strength of the match: FTS5's `bm25()` summed over the terms, negated so a
/// stronger match is a larger number. Sorted by id.
pub fn fts_body_hits(store: &Store, terms: &[String]) -> Result<Vec<(i64, f64)>> {
    let mut hits: HashMap<i64, f64> = HashMap::new();
    let mut stmt = store.conn().prepare(
        "SELECT rowid, -bm25(sections_fts) FROM sections_fts WHERE sections_fts MATCH ?1",
    )?;
    for t in terms {
        let q = format!("content: \"{}\"*", t.replace('"', "\"\""));
        for row in stmt.query_map([q], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?)))? {
            let (id, w) = row?;
            *hits.entry(id).or_default() += w.max(0.0);
        }
    }
    let mut out: Vec<(i64, f64)> = hits.into_iter().collect();
    out.sort_unstable_by_key(|(id, _)| *id);
    Ok(out)
}

/// One hit for `hit_shares`: `(name hit, body rank, semantic similarity)`.
pub type Hit = (bool, Option<f64>, Option<f64>);

/// How a file's FTS bonus is shared between its hits (documents spec §5, amended;
/// knowledge spec §4). Each hit is `(name hit, body rank, semantic similarity)`: a name
/// hit weighs 1.0; a body rank is normalised so the strongest body hit in the file
/// weighs 1.0 (similarities never enter that maximum); the symbol then takes the larger
/// of its normalised rank and its similarity, and adds the name weight. Returns each
/// hit's share of the bonus, summing to one. An even split falls out when every hit is
/// a name hit, which is what code files had before.
pub fn hit_shares(hits: &[Hit]) -> Vec<f64> {
    let max_body = hits
        .iter()
        .filter_map(|(_, b, _)| *b)
        .fold(0.0_f64, f64::max);
    let weights: Vec<f64> = hits
        .iter()
        .map(|(name, body, semantic)| {
            let n = if *name { 1.0 } else { 0.0 };
            let b = match body {
                Some(r) if max_body > 0.0 => r / max_body,
                Some(_) => 1.0,
                None => 0.0,
            };
            n + b.max(semantic.unwrap_or(0.0))
        })
        .collect();
    let total: f64 = weights.iter().sum();
    if total <= 0.0 {
        return vec![0.0; hits.len()];
    }
    weights.into_iter().map(|w| w / total).collect()
}

/// Directories whose files are tests or benchmarks, whatever the language.
const SUPPORT_DIRS: [&str; 7] = [
    "__tests__",
    "__mocks__",
    "test",
    "tests",
    "bench",
    "benches",
    "benchmarks",
];

/// A test, spec or benchmark source file (never a document): it ranks as a referrer but is never served as a map
/// row, and `map.toml`'s `exclude` is still the way to drop a file from the graph.
pub fn is_support_file(path: &str) -> bool {
    // Documents are never support files (documents spec §5).
    if crate::lang::Language::from_path(path).is_some_and(|l| l.is_document()) {
        return false;
    }
    let file = path.rsplit('/').next().unwrap_or(path);
    if file.contains(".test.") || file.contains(".spec.") {
        return true;
    }
    path.split('/')
        .rev()
        .skip(1)
        .any(|dir| SUPPORT_DIRS.contains(&dir))
}

/// A query term appears in the note as a whole word, after the same splitting the
/// query gets (`tokens::query_terms`), so `createSession` in a note matches `session`.
pub fn note_matches(text: &str, terms: &[String]) -> bool {
    if terms.is_empty() {
        return false;
    }
    let words = crate::tokens::query_terms(text);
    terms.iter().any(|t| words.contains(t))
}

/// The sections the knowledge seeds reach, by symbol id (knowledge spec §4). Computed
/// from vectors already in `Seeds`: ranking never calls a model.
#[derive(Default)]
struct KnowledgeHits {
    /// Nearest sections to the query vector, with their similarity.
    semantic: HashMap<i64, f64>,
    /// Sections mentioning a matched entity, with the entities' names.
    entities: HashMap<i64, Vec<String>>,
    /// Sections stating a relation near a theme: `(theme, similarity)`, best per theme.
    themes: HashMap<i64, Vec<(String, f64)>>,
    /// Code symbols a matched process's systems name, with the system names.
    implements: HashMap<i64, Vec<String>>,
}

fn knowledge_hits(store: &Store, seeds: &Seeds) -> Result<KnowledgeHits> {
    let mut hits = KnowledgeHits::default();
    let conn = store.conn();
    if let Some(qv) = &seeds.query_vec {
        for (id, sim) in crate::store::vec::knn(store, "section_vec", qv, SEMANTIC_K)? {
            if sim > 0.0 {
                hits.semantic.insert(id, sim);
            }
        }
    }
    if !seeds.entity_ids.is_empty() {
        let mut stmt =
            conn.prepare("SELECT symbol_id FROM entity_mentions WHERE entity_id = ?1")?;
        for (id, name) in seeds.entity_ids.iter().zip(&seeds.entity_names) {
            for row in stmt.query_map([id], |r| r.get::<_, i64>(0))? {
                let names = hits.entities.entry(row?).or_default();
                if !names.contains(name) {
                    names.push(name.clone());
                }
            }
        }
    }
    for (id, name) in &seeds.implements {
        let names = hits.implements.entry(*id).or_default();
        if !names.contains(name) {
            names.push(name.clone());
        }
    }
    if !seeds.theme_vecs.is_empty() {
        let mut stmt = conn.prepare("SELECT symbol_id FROM relations WHERE id = ?1")?;
        for (theme, tv) in &seeds.theme_vecs {
            for (rel, sim) in crate::store::vec::knn(store, "relation_vec", tv, THEME_K)? {
                if sim <= 0.0 {
                    continue;
                }
                for row in stmt.query_map([rel], |r| r.get::<_, i64>(0))? {
                    let list = hits.themes.entry(row?).or_default();
                    match list.iter_mut().find(|(t, _)| t == theme) {
                        Some(best) => best.1 = best.1.max(sim),
                        None => list.push((theme.clone(), sim)),
                    }
                }
            }
        }
    }
    Ok(hits)
}

pub fn rank_symbols(
    store: &Store,
    config: &MapConfig,
    query: Option<&str>,
    focus_files: &[String],
    seeds: &Seeds,
) -> Result<Vec<ScoredSymbol>> {
    let terms = query.map(query_terms).unwrap_or_default();
    let conn = store.conn();

    // Notes whose text matches the query: files get a seed, named symbols a bonus.
    let mut note_files: HashSet<String> = HashSet::new();
    let mut note_symbols: HashSet<(String, String)> = HashSet::new();
    for n in &config.note {
        if note_matches(&n.text, &terms) {
            note_files.insert(n.path.clone());
            if let Some(s) = &n.symbol {
                note_symbols.insert((n.path.clone(), s.clone()));
            }
        }
    }

    // Symbols come first: which names the FTS matched decides which edges the query
    // multiplier applies to, so the graph cannot be built before they are known.
    let mut symbols: Vec<SymbolRow> = Vec::new();
    let mut stmt = conn
        .prepare("SELECT id, file_id, name, kind, line_start, line_end, signature FROM symbols")?;
    for row in stmt.query_map([], |r| {
        Ok(SymbolRow {
            id: r.get(0)?,
            file_id: r.get(1)?,
            name: r.get(2)?,
            kind: r.get(3)?,
            line_start: r.get(4)?,
            line_end: r.get(5)?,
            signature: r.get(6)?,
        })
    })? {
        symbols.push(row?);
    }

    let fts_ids = fts_symbol_ids(store, &terms)?;
    let is_fts_hit = |id: i64| fts_ids.binary_search(&id).is_ok();
    // `fts_names` stays symbol-name hits only: a body hit must not turn the query
    // multiplier on for an edge name.
    let fts_names: HashSet<String> = symbols
        .iter()
        .filter(|s| is_fts_hit(s.id))
        .map(|s| s.name.clone())
        .collect();
    let body_hits = fts_body_hits(store, &terms)?;
    let body_rank = |id: i64| -> Option<f64> {
        body_hits
            .binary_search_by_key(&id, |(i, _)| *i)
            .ok()
            .map(|i| body_hits[i].1)
    };
    let is_body_hit = |id: i64| body_rank(id).is_some();
    let kh = knowledge_hits(store, seeds)?;

    let g: FileGraph = build_graph(store, config, &terms, &fts_names)?;
    let n = g.nodes.len();
    if n == 0 {
        return Ok(Vec::new());
    }
    symbols.retain(|s| g.index_of.contains_key(&s.file_id));

    let fts_files: HashSet<i64> = symbols
        .iter()
        .filter(|s| is_fts_hit(s.id) || is_body_hit(s.id))
        .map(|s| s.file_id)
        .collect();
    // Per file: the best semantic similarity, the matched entity names in seed order,
    // and the best theme similarity.
    let mut semantic_files: HashMap<i64, f64> = HashMap::new();
    let mut entity_files: HashMap<i64, Vec<String>> = HashMap::new();
    let mut theme_files: HashMap<i64, f64> = HashMap::new();
    let mut implements_files: HashMap<i64, Vec<String>> = HashMap::new();
    for s in &symbols {
        if let Some(&sim) = kh.semantic.get(&s.id) {
            let best = semantic_files.entry(s.file_id).or_default();
            *best = best.max(sim);
        }
        if let Some(names) = kh.entities.get(&s.id) {
            let file_names = entity_files.entry(s.file_id).or_default();
            for n in names {
                if !file_names.contains(n) {
                    file_names.push(n.clone());
                }
            }
        }
        if let Some(names) = kh.implements.get(&s.id) {
            let file_names = implements_files.entry(s.file_id).or_default();
            for n in names {
                if !file_names.contains(n) {
                    file_names.push(n.clone());
                }
            }
        }
        if let Some(themes) = kh.themes.get(&s.id) {
            let best = theme_files.entry(s.file_id).or_default();
            *best = themes.iter().map(|(_, sim)| *sim).fold(*best, f64::max);
        }
    }
    for names in entity_files.values_mut() {
        names.sort_by_key(|n| seeds.entity_names.iter().position(|e| e == n));
    }

    // Personalization + seeds per file.
    let mut personalization = vec![1.0; n];
    let mut seeds: Vec<Vec<String>> = vec![Vec::new(); n];
    for (i, node) in g.nodes.iter().enumerate() {
        if focus_files.iter().any(|f| f == &node.path) {
            personalization[i] += FOCUS_BOOST;
            seeds[i].push("focus".to_string());
        }
        if config.is_pinned(&node.path) {
            personalization[i] += PIN_BOOST;
            seeds[i].push("pinned".to_string());
        }
        if fts_files.contains(&node.id) {
            personalization[i] += FTS_FILE_BOOST;
            seeds[i].push(format!("query:{}", terms.join(" ")));
        }
        if note_files.contains(&node.path) {
            personalization[i] += NOTE_BOOST;
            seeds[i].push("note".to_string());
        }
        if let Some(sim) = semantic_files.get(&node.id) {
            personalization[i] += FTS_FILE_BOOST * sim;
            seeds[i].push("semantic".to_string());
        }
        if let Some(names) = entity_files.get(&node.id) {
            personalization[i] += NOTE_BOOST;
            seeds[i].extend(names.iter().map(|n| format!("entity:{n}")));
        }
        if let Some(names) = implements_files.get(&node.id) {
            personalization[i] += NOTE_BOOST;
            seeds[i].extend(names.iter().map(|n| format!("implements:{n}")));
        }
        if let Some(sim) = theme_files.get(&node.id) {
            personalization[i] += FTS_FILE_BOOST * sim;
            seeds[i].push("theme".to_string());
        }
    }
    let edge_triples: Vec<(usize, usize, f64)> =
        g.edges.iter().map(|e| (e.src, e.dst, e.weight)).collect();
    let file_rank = pagerank(n, &edge_triples, &personalization);

    // Out-weight per file for distribution. Self-edges are excluded from PageRank but
    // included here: a symbol used only inside its own file is still used, and both the
    // score and the `referenced_by` list have to say so.
    let all_edges = || g.edges.iter().chain(g.self_edges.iter());
    let mut out_w = vec![0.0; n];
    for e in all_edges() {
        out_w[e.src] += e.weight;
    }
    // (dst file, name) -> distributed score, and referenced_by lists.
    let mut dist: HashMap<(usize, String), f64> = HashMap::new();
    let mut ref_by: HashMap<(usize, String), HashMap<usize, i64>> = HashMap::new();
    for e in all_edges() {
        *dist.entry((e.dst, e.name.clone())).or_default() +=
            file_rank[e.src] * e.weight / out_w[e.src];
        // raw ref count for the reasons list (undo the ambiguity split and query multiplier)
        let multiplier = if query_ident_match(&e.name, &terms, &fts_names) {
            crate::graph::QUERY_IDENT_MULTIPLIER
        } else {
            1.0
        };
        let definer_count = *g.definer_count.get(&e.name).unwrap_or(&1) as f64;
        let raw = (e.weight * definer_count / multiplier).round() as i64;
        *ref_by
            .entry((e.dst, e.name.clone()))
            .or_default()
            .entry(e.src)
            .or_default() = raw;
    }

    // A file's rank is *distributed*, not replicated: eight `const Child = …` in one file
    // share one name's incoming weight instead of each taking all of it, and a file with
    // many FTS hits shares one file rank between them, in proportion to each hit's
    // strength (`hit_shares`): a spec section that really answers the query keeps most
    // of its document's bonus instead of a twentieth of it.
    let mut group_size: HashMap<(usize, String), usize> = HashMap::new();
    let mut hits_by_file: HashMap<usize, Vec<(i64, Hit)>> = HashMap::new();
    for s in &symbols {
        let fi = g.index_of[&s.file_id];
        *group_size.entry((fi, s.name.clone())).or_default() += 1;
        // Entity, theme and implements hits weigh like a name hit; a semantic hit weighs
        // its similarity against the file's normalised body ranks (`hit_shares`).
        let name_hit = is_fts_hit(s.id)
            || kh.entities.contains_key(&s.id)
            || kh.themes.contains_key(&s.id)
            || kh.implements.contains_key(&s.id);
        let body = body_rank(s.id);
        let semantic = kh.semantic.get(&s.id).copied();
        if name_hit || body.is_some() || semantic.is_some() {
            hits_by_file
                .entry(fi)
                .or_default()
                .push((s.id, (name_hit, body, semantic)));
        }
    }
    let mut share_of: HashMap<i64, f64> = HashMap::new();
    for hits in hits_by_file.values() {
        let kinds: Vec<Hit> = hits.iter().map(|(_, h)| *h).collect();
        for ((id, _), share) in hits.iter().zip(hit_shares(&kinds)) {
            share_of.insert(*id, share);
        }
    }

    let mut out: Vec<ScoredSymbol> = symbols
        .into_iter()
        // Tests and benchmarks keep their edges (on hono they are the strongest referrers
        // of the public API, and dropping them lowered recall) but are never candidates:
        // they were 43% of the rows a 4096-token map served, none of them an answer.
        .filter(|s| !is_support_file(&g.nodes[g.index_of[&s.file_id]].path))
        .map(|s| {
            let fi = g.index_of[&s.file_id];
            let key = (fi, s.name.clone());
            let fr = file_rank[fi];
            let siblings = *group_size.get(&key).unwrap_or(&1) as f64;
            let mut score = dist
                .get(&key)
                .map(|d| d / siblings)
                .unwrap_or(fr * UNREFERENCED_FRACTION);
            let fts_hit = is_fts_hit(s.id);
            let body_hit = is_body_hit(s.id);
            if let Some(share) = share_of.get(&s.id) {
                score += fr * share;
            }
            let path = &g.nodes[fi].path;
            let has_symbol_note =
                !note_symbols.is_empty() && note_symbols.contains(&(path.clone(), s.name.clone()));
            let note_hit = note_files.contains(path);
            if has_symbol_note {
                score += fr;
            }
            let mut referenced_by: Vec<RefBy> = ref_by
                .get(&key)
                .map(|m| {
                    m.iter()
                        .map(|(&src, &c)| RefBy {
                            path: g.nodes[src].path.clone(),
                            count: c,
                        })
                        .collect()
                })
                .unwrap_or_default();
            referenced_by.sort_by(|a, b| b.count.cmp(&a.count).then(a.path.cmp(&b.path)));
            referenced_by.truncate(5);
            ScoredSymbol {
                symbol_id: s.id,
                file_id: s.file_id,
                path: g.nodes[fi].path.clone(),
                name: s.name.clone(),
                kind: s.kind,
                line_start: s.line_start,
                line_end: s.line_end,
                signature: s.signature,
                score,
                reasons: Reasons {
                    score,
                    file_rank: fr,
                    seeds: seeds[fi].clone(),
                    referenced_by,
                    pinned: config.is_pinned(&g.nodes[fi].path),
                    fts_hit,
                    query_ident_match: query_ident_match(&s.name, &terms, &fts_names),
                    note_hit,
                    body_hit,
                    semantic: kh.semantic.get(&s.id).copied(),
                    entities: kh.entities.get(&s.id).cloned().unwrap_or_default(),
                    themes: kh
                        .themes
                        .get(&s.id)
                        .map(|t| t.iter().map(|(theme, _)| theme.clone()).collect())
                        .unwrap_or_default(),
                    implements: kh.implements.get(&s.id).cloned().unwrap_or_default(),
                },
            }
        })
        .collect();
    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.path.cmp(&b.path))
            .then(a.line_start.cmp(&b.line_start))
    });
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::config::MapConfig;
    use crate::fixture::write_ts_mini;
    use crate::index::Indexer;
    use crate::store::Store;
    use crate::workspace::Workspace;

    #[test]
    fn pagerank_sums_to_one_and_prefers_sinks() {
        // A -> B -> C, uniform personalization: C > B > A.
        let r = pagerank(3, &[(0, 1, 1.0), (1, 2, 1.0)], &[1.0, 1.0, 1.0]);
        assert!((r.iter().sum::<f64>() - 1.0).abs() < 1e-9);
        assert!(r[2] > r[1] && r[1] > r[0], "{r:?}");
    }

    #[test]
    fn pagerank_symmetric_pair_is_equal_and_personalization_boosts() {
        let r = pagerank(2, &[(0, 1, 1.0), (1, 0, 1.0)], &[1.0, 1.0]);
        assert!((r[0] - r[1]).abs() < 1e-9);
        let b = pagerank(2, &[(0, 1, 1.0), (1, 0, 1.0)], &[10.0, 1.0]);
        assert!(b[0] > b[1]);
    }

    #[test]
    fn pagerank_handles_no_edges_and_zero_personalization() {
        let r = pagerank(3, &[], &[0.0, 0.0, 0.0]);
        assert!((r.iter().sum::<f64>() - 1.0).abs() < 1e-9);
        assert!((r[0] - r[2]).abs() < 1e-9);
        assert!(pagerank(0, &[], &[]).is_empty());
    }

    fn indexed() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        write_ts_mini(dir.path());
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        Indexer::new(
            &store,
            &Workspace::single(dir.path()).unwrap(),
            &MapConfig::default(),
        )
        .unwrap()
        .refresh(None)
        .unwrap();
        (dir, store)
    }

    #[test]
    fn most_referenced_definition_outranks_unreferenced_ones() {
        let (_dir, store) = indexed();
        let ranked =
            rank_symbols(&store, &MapConfig::default(), None, &[], &Seeds::default()).unwrap();
        let names = || ranked.iter().map(|s| &s.name).collect::<Vec<_>>();
        let pos = |n: &str| ranked.iter().position(|s| s.name == n).unwrap();
        // Referenced from two other files; `attachSession` and `login` are referenced
        // from nowhere. (`log` can sit above it: `console.log` is a self-reference the
        // name-based ref model cannot tell from a real one, and its file has one symbol.)
        assert!(pos("createSession") < pos("attachSession"), "{:?}", names());
        assert!(pos("createSession") < pos("login"), "{:?}", names());
        let cs = &ranked[pos("createSession")];
        let rb = &cs.reasons.referenced_by;
        assert!(
            rb.iter()
                .any(|r| r.path == "src/http/middleware.ts" && r.count == 2),
            "{rb:?}"
        );
        assert!(rb
            .iter()
            .any(|r| r.path == "src/cli/login.ts" && r.count == 1));
        assert!(ranked.iter().all(|s| s.score >= 0.0));
    }

    /// Spec §7 step 3 says *distribute* the file rank to its symbols. Handing the whole
    /// group's score to every symbol sharing a name replicated it instead, so eight
    /// `const Child = …` in one file took eight consecutive slots of the budget.
    #[test]
    fn same_named_symbols_split_their_share_instead_of_replicating_it() {
        let dir = tempfile::tempdir().unwrap();
        let w = |rel: &str, body: &str| {
            let p = dir.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        };
        w(
            "src/dup.ts",
            &"const Child = (p: string) => render(p)\n".repeat(4),
        );
        w("src/one.ts", "const Only = (p: string) => render(p)\n");
        w(
            "src/use.ts",
            "export function page(): void {\n  Child(\"a\");\n  Only(\"b\");\n}\n",
        );
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        Indexer::new(
            &store,
            &Workspace::single(dir.path()).unwrap(),
            &MapConfig::default(),
        )
        .unwrap()
        .refresh(None)
        .unwrap();
        let ranked =
            rank_symbols(&store, &MapConfig::default(), None, &[], &Seeds::default()).unwrap();

        let children: Vec<&ScoredSymbol> = ranked.iter().filter(|s| s.name == "Child").collect();
        assert_eq!(
            children.len(),
            4,
            "{:?}",
            ranked.iter().map(|s| &s.name).collect::<Vec<_>>()
        );
        let only = ranked.iter().find(|s| s.name == "Only").unwrap();
        for c in &children {
            assert!((c.score - children[0].score).abs() < 1e-15, "{c:?}");
        }
        // `Child` and `Only` receive the same incoming weight; four definitions share it.
        assert!(
            (only.score - 4.0 * children[0].score).abs() < 1e-12,
            "only {} vs child {}",
            only.score,
            children[0].score
        );
        assert_eq!(
            ranked[0].name, "Only",
            "the group must not crowd out the single definition"
        );
    }

    /// Ruling 1 (drop self-edges) is right for PageRank and wrong here: a symbol used
    /// only inside its own file is still used, and its reasons must say where.
    #[test]
    fn self_references_count_for_the_score_and_appear_in_reasons() {
        let (_dir, store) = indexed();
        let ranked =
            rank_symbols(&store, &MapConfig::default(), None, &[], &Seeds::default()).unwrap();
        // `SessionStore` is only ever referenced from src/auth/session.ts (`new SessionStore()`).
        let store_sym = ranked.iter().find(|s| s.name == "SessionStore").unwrap();
        assert!(
            store_sym.score > store_sym.reasons.file_rank * UNREFERENCED_FRACTION,
            "{store_sym:?}"
        );
        assert!(
            store_sym
                .reasons
                .referenced_by
                .iter()
                .any(|r| r.path == "src/auth/session.ts" && r.count == 1),
            "{:?}",
            store_sym.reasons.referenced_by
        );
        let unreferenced = ranked.iter().find(|s| s.name == "attachSession").unwrap();
        assert!(store_sym.score > unreferenced.score);
    }

    /// The FTS index is stemmed, so the ×10 query-identifier multiplier has to follow the
    /// stemmer rather than compare raw terms, and a file's FTS bonus is shared by its hits.
    #[test]
    fn stemmed_fts_hits_count_as_query_identifier_matches() {
        let (_dir, store) = indexed();
        let ranked = rank_symbols(
            &store,
            &MapConfig::default(),
            Some("sessions"),
            &[],
            &Seeds::default(),
        )
        .unwrap();
        let cs = ranked.iter().find(|s| s.name == "createSession").unwrap();
        assert!(cs.reasons.fts_hit, "porter stemming should match sessions");
        assert!(
            cs.reasons.query_ident_match,
            "an FTS hit is a query identifier match even when the raw term differs"
        );
        // src/http/middleware.ts has two hits and no incoming references: each takes
        // half of the file's rank, not all of it.
        for name in ["requireSession", "attachSession"] {
            let s = ranked.iter().find(|s| s.name == name).unwrap();
            let fr = s.reasons.file_rank;
            assert!(
                (s.score - (fr * UNREFERENCED_FRACTION + fr / 2.0)).abs() < 1e-12,
                "{name}: {s:?}"
            );
        }
    }

    #[test]
    fn query_terms_boost_matching_symbols_and_record_reasons() {
        let (_dir, store) = indexed();
        let ranked = rank_symbols(
            &store,
            &MapConfig::default(),
            Some("where is log written"),
            &[],
            &Seeds::default(),
        )
        .unwrap();
        let log = ranked.iter().find(|s| s.name == "log").unwrap();
        assert!(log.reasons.fts_hit);
        assert!(log.reasons.query_ident_match);
        assert!(
            log.reasons.seeds.iter().any(|s| s.starts_with("query:")),
            "{:?}",
            log.reasons.seeds
        );
        let pos_log = ranked.iter().position(|s| s.name == "log").unwrap();
        let pos_attach = ranked
            .iter()
            .position(|s| s.name == "attachSession")
            .unwrap();
        assert!(pos_log < pos_attach);
    }

    #[test]
    fn support_files_are_tests_specs_and_benchmarks_in_any_language() {
        for p in [
            "src/hono.test.ts",
            "src/jsx/dom/index.test.tsx",
            "src/router.spec.js",
            "src/__tests__/router.ts",
            "test/helpers.ts",
            "tests/serve.rs",
            "benches/rank.rs",
            "benchmarks/routers/src/tool.mts",
        ] {
            assert!(is_support_file(p), "{p}");
        }
        for p in [
            "src/router.ts",
            "src/testing.ts",
            "src/contest/index.ts",
            "src/bench-utils.ts",
            "crates/singularrag/src/serve/routes.rs",
        ] {
            assert!(!is_support_file(p), "{p}");
        }
        // Documents are never support files (documents spec §5).
        assert!(!is_support_file("docs/test/guide.md"));
        assert!(!is_support_file("api.spec.md"));
        assert!(is_support_file("src/a.test.ts"));
    }

    #[test]
    fn excluded_files_vanish_and_pins_are_recorded() {
        let (_dir, store) = indexed();
        let cfg = MapConfig::parse(
            "[[exclude]]\npath = \"src/util/\"\n[[pin]]\npath = \"src/cli/login.ts\"\n",
        )
        .unwrap();
        let ranked = rank_symbols(&store, &cfg, None, &[], &Seeds::default()).unwrap();
        assert!(ranked.iter().all(|s| s.path != "src/util/log.ts"));
        let login = ranked.iter().find(|s| s.name == "login").unwrap();
        assert!(login.reasons.pinned);
        assert!(login.reasons.seeds.contains(&"pinned".to_string()));
    }

    #[test]
    fn focus_files_seed_personalization() {
        let (_dir, store) = indexed();
        let ranked = rank_symbols(
            &store,
            &MapConfig::default(),
            None,
            &["src/cli/login.ts".to_string()],
            &Seeds::default(),
        )
        .unwrap();
        let login = ranked.iter().find(|s| s.name == "login").unwrap();
        assert!(login.reasons.seeds.contains(&"focus".to_string()));
    }

    #[test]
    fn a_note_matching_the_query_seeds_its_file_and_its_symbol() {
        let (_dir, store) = indexed();
        let config = MapConfig::parse(
            "[[note]]\npath = \"src/cli/login.ts\"\nsymbol = \"login\"\ntext = \"the onboarding flow starts here\"\nby = \"agent\"\nsession = \"s\"\nat = \"t\"\n",
        )
        .unwrap();
        let plain = rank_symbols(
            &store,
            &MapConfig::default(),
            Some("onboarding flow"),
            &[],
            &Seeds::default(),
        )
        .unwrap();
        let noted = rank_symbols(
            &store,
            &config,
            Some("onboarding flow"),
            &[],
            &Seeds::default(),
        )
        .unwrap();
        let pos = |v: &[ScoredSymbol]| v.iter().position(|s| s.name == "login").unwrap();
        assert!(
            pos(&noted) < pos(&plain),
            "the note lifts login: {} vs {}",
            pos(&noted),
            pos(&plain)
        );
        let login = noted.iter().find(|s| s.name == "login").unwrap();
        assert!(login.reasons.note_hit);
        assert!(
            login.reasons.seeds.iter().any(|s| s == "note"),
            "{:?}",
            login.reasons.seeds
        );
        let other = noted.iter().find(|s| s.name == "createSession").unwrap();
        assert!(!other.reasons.note_hit);
        assert!(note_matches("The Onboarding flow", &["onboarding".into()]));
        assert!(
            !note_matches("onboard", &["onboarding".into()]),
            "whole words only"
        );
    }

    #[test]
    fn a_body_hit_seeds_the_file_and_marks_the_section() {
        let dir = tempfile::tempdir().unwrap();
        crate::fixture::write_docs_mini(dir.path());
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        let ws = crate::workspace::Workspace::single(dir.path()).unwrap();
        Indexer::new(&store, &ws, &MapConfig::default())
            .unwrap()
            .refresh(None)
            .unwrap();
        let ranked = rank_symbols(
            &store,
            &MapConfig::default(),
            Some("what does the STALE header mean"),
            &[],
            &Seeds::default(),
        )
        .unwrap();
        let top: Vec<String> = ranked
            .iter()
            .take(3)
            .map(|s| format!("{}::{}", s.path, s.name))
            .collect();
        assert!(
            top.contains(&"docs/runbook.md::When the header says STALE".to_string()),
            "{top:?}"
        );
        // The runbook's heading matches `symbols_fts` (its name), not `sections_fts`:
        // the sentence with "STALE" and "header" is the *runbook's* prose, but under a
        // different heading it would live in that section's body. Here it is the
        // heading itself that carries the words, so this hit is `fts_hit`, not
        // `body_hit`; `docs/design.md::Freshness`'s body is the one that says "the
        // STALE header" in prose, so that is where `body_hit` is asserted.
        let hit = ranked
            .iter()
            .find(|s| s.name == "When the header says STALE")
            .unwrap();
        assert!(hit.reasons.fts_hit);
        assert!(hit.reasons.seeds.iter().any(|s| s.starts_with("query:")));
        let fresh = ranked
            .iter()
            .find(|s| s.path == "docs/design.md" && s.name == "Freshness")
            .unwrap();
        assert!(fresh.reasons.body_hit, "{fresh:?}");
        // No symbol in docs/design.md has a name-based FTS hit for this query, so this
        // seed can only come from the body hit: a body-only hit still seeds its file.
        assert!(
            fresh.reasons.seeds.iter().any(|s| s.starts_with("query:")),
            "{:?}",
            fresh.reasons.seeds
        );
        let sess = ranked.iter().find(|s| s.name == "createSession").unwrap();
        assert!(!sess.reasons.body_hit);
    }

    #[test]
    fn a_mention_gives_the_document_an_edge_to_the_code() {
        let dir = tempfile::tempdir().unwrap();
        crate::fixture::write_docs_mini(dir.path());
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        let ws = crate::workspace::Workspace::single(dir.path()).unwrap();
        Indexer::new(&store, &ws, &MapConfig::default())
            .unwrap()
            .refresh(None)
            .unwrap();
        let ranked = rank_symbols(
            &store,
            &MapConfig::default(),
            Some("createSession"),
            &[],
            &Seeds::default(),
        )
        .unwrap();
        let sess = ranked
            .iter()
            .find(|s| s.path == "src/auth/session.ts" && s.name == "createSession")
            .unwrap();
        assert!(
            sess.reasons
                .referenced_by
                .iter()
                .any(|r| r.path == "docs/design.md"),
            "{:?}",
            sess.reasons.referenced_by
        );
    }

    #[test]
    fn hit_shares_weigh_body_hits_by_rank_and_name_hits_as_one() {
        // Two name hits: even split, as before.
        let even = hit_shares(&[(true, None, None), (true, None, None)]);
        assert!((even[0] - 0.5).abs() < 1e-9 && (even[1] - 0.5).abs() < 1e-9);
        // One strong body hit and three weak ones: the strong one weighs 1.0, the weak
        // ones 0.1 each, so the strong section takes 1 / 1.3 of the file's bonus.
        let s = hit_shares(&[
            (false, Some(10.0), None),
            (false, Some(1.0), None),
            (false, Some(1.0), None),
            (false, Some(1.0), None),
        ]);
        assert!((s[0] - 1.0 / 1.3).abs() < 1e-9, "{s:?}");
        assert!((s[1] - 0.1 / 1.3).abs() < 1e-9, "{s:?}");
        // A name hit beside body hits weighs 1.0, the same as the strongest body hit.
        let m = hit_shares(&[
            (true, None, None),
            (false, Some(4.0), None),
            (false, Some(2.0), None),
        ]);
        assert!(
            (m[0] - 1.0 / 2.5).abs() < 1e-9
                && (m[1] - 1.0 / 2.5).abs() < 1e-9
                && (m[2] - 0.5 / 2.5).abs() < 1e-9,
            "{m:?}"
        );
        // Shares always sum to one; a symbol with both a name and a body hit adds them.
        let both = hit_shares(&[(true, Some(3.0), None), (false, Some(3.0), None)]);
        assert!(
            (both.iter().sum::<f64>() - 1.0).abs() < 1e-9 && both[0] > both[1],
            "{both:?}"
        );
        assert!(hit_shares(&[]).is_empty());
    }

    #[test]
    fn a_semantic_hit_weighs_against_normalised_body_ranks() {
        // Body ranks are raw bm25 sums; they are normalised before the similarity is
        // compared, and a similarity never enters the body maximum.
        let s = hit_shares(&[(false, Some(8.0), None), (false, None, Some(0.85))]);
        assert!((s[0] - 1.0 / 1.85).abs() < 1e-9, "{s:?}");
        assert!((s[1] - 0.85 / 1.85).abs() < 1e-9, "{s:?}");
        // A symbol with both takes the larger of its normalised rank and its similarity.
        let b = hit_shares(&[(false, Some(8.0), None), (false, Some(2.0), Some(0.6))]);
        assert!((b[1] - 0.6 / 1.6).abs() < 1e-9, "{b:?}");
    }

    #[test]
    fn a_strong_section_takes_most_of_its_documents_bonus() {
        let dir = tempfile::tempdir().unwrap();
        crate::fixture::write_ts_mini(dir.path());
        let mut md = String::from("# Notes\n\n## Strong\n\nrouting routing routing routing routing routing decides the route.\n");
        for i in 1..=5 {
            md.push_str(&format!("\n## Weak{i}\n\nan unrelated paragraph that mentions routing once among many other words about nothing in particular.\n"));
        }
        std::fs::create_dir_all(dir.path().join("docs")).unwrap();
        std::fs::write(dir.path().join("docs/notes.md"), md).unwrap();
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        let ws = crate::workspace::Workspace::single(dir.path()).unwrap();
        Indexer::new(&store, &ws, &MapConfig::default())
            .unwrap()
            .refresh(None)
            .unwrap();
        let ranked = rank_symbols(
            &store,
            &MapConfig::default(),
            Some("routing"),
            &[],
            &Seeds::default(),
        )
        .unwrap();
        let score = |name: &str| {
            ranked
                .iter()
                .find(|s| s.name == name)
                .unwrap_or_else(|| panic!("{name}"))
                .score
        };
        let strong = score("Strong");
        let weak = score("Weak1");
        assert!(
            strong > 2.0 * weak,
            "strong {strong} vs weak {weak}: an even split would make them equal"
        );
        let pos = |name: &str| ranked.iter().position(|s| s.name == name).unwrap();
        assert!(
            (1..=5).all(|i| pos("Strong") < pos(&format!("Weak{i}"))),
            "{:?}",
            ranked.iter().map(|s| &s.name).collect::<Vec<_>>()
        );
    }

    /// A docs fixture with the knowledge layer drained against the fake: section, entity
    /// and relation vectors exist. Task 6 reuses it.
    pub(crate) fn knowledge_ready() -> (
        tempfile::TempDir,
        Store,
        crate::fake_ollama::FakeOllama,
        crate::models::Models,
    ) {
        let dir = tempfile::tempdir().unwrap();
        crate::fixture::write_docs_mini(dir.path());
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        let ws = crate::workspace::Workspace::single(dir.path()).unwrap();
        Indexer::new(&store, &ws, &MapConfig::default())
            .unwrap()
            .refresh(None)
            .unwrap();
        let f = crate::fake_ollama::FakeOllama::spawn(32);
        // A relation only lands when both ends are known entities, and Freshness is
        // extracted before Storage, so Freshness names `createSession` itself.
        f.set_extraction("Freshness", serde_json::json!({"entities": [{"name": "STALE header", "type": "concept", "description": "the header when files changed"}, {"name": "createSession", "type": "system", "description": "creates a session"}], "relations": [{"source": "STALE header", "target": "createSession", "description": "refresh runs before session creation"}]}));
        f.set_extraction("Storage", serde_json::json!({"entities": [{"name": "SessionStore", "type": "system", "description": "keeps sessions"}, {"name": "createSession", "type": "system", "description": "creates a session"}], "relations": []}));
        let m = crate::models::Models::new(crate::models::ModelsConfig {
            ollama: f.url(),
            ..Default::default()
        });
        while crate::knowledge::pending(&store).unwrap() > 0 {
            crate::knowledge::tick(&store, &m, std::time::Duration::from_secs(20), &|| false)
                .unwrap();
        }
        (dir, store, f, m)
    }

    #[test]
    fn the_semantic_seed_finds_a_section_with_no_keyword_overlap() {
        let (_d, store, _f, m) = knowledge_ready();
        // The fake embeds by bag of words; "Wait and call again" is the runbook's prose, and
        // the query shares those words but none of the heading's, so FTS on names finds nothing.
        let seeds =
            crate::knowledge::seeds_for(&store, Some(&m), Some("wait and call again"), &[], &[])
                .unwrap();
        assert!(seeds.query_vec.is_some() && !seeds.models_unavailable);
        let ranked = rank_symbols(
            &store,
            &MapConfig::default(),
            Some("wait and call again"),
            &[],
            &seeds,
        )
        .unwrap();
        let hit = ranked
            .iter()
            .find(|s| s.name == "When the header says STALE")
            .unwrap();
        assert!(
            hit.reasons.semantic.is_some_and(|v| v > 0.5),
            "{:?}",
            hit.reasons
        );
        assert!(hit.reasons.seeds.contains(&"semantic".to_string()));
        assert!(
            ranked
                .iter()
                .position(|s| s.name == "When the header says STALE")
                .unwrap()
                < 5,
            "{:?}",
            ranked.iter().take(5).map(|s| &s.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn the_entity_seed_matches_by_term_argument_and_vector() {
        let (_d, store, _f, m) = knowledge_ready();
        let by_term = crate::knowledge::seeds_for(
            &store,
            Some(&m),
            Some("what is the stale header"),
            &[],
            &[],
        )
        .unwrap();
        assert!(
            by_term.entity_names.iter().any(|n| n == "STALE header"),
            "{:?}",
            by_term.entity_names
        );
        let by_arg = crate::knowledge::seeds_for(
            &store,
            None,
            Some("anything"),
            &["sessionstore".into()],
            &[],
        )
        .unwrap();
        assert_eq!(by_arg.entity_names, vec!["SessionStore".to_string()]);
        let ranked = rank_symbols(
            &store,
            &MapConfig::default(),
            Some("anything"),
            &[],
            &by_arg,
        )
        .unwrap();
        let storage = ranked.iter().find(|s| s.name == "Storage").unwrap();
        assert_eq!(storage.reasons.entities, vec!["SessionStore".to_string()]);
        assert!(storage
            .reasons
            .seeds
            .contains(&"entity:SessionStore".to_string()));
    }

    #[test]
    fn themes_match_relations_and_seeds_are_empty_without_models() {
        let (_d, store, f, m) = knowledge_ready();
        let s = crate::knowledge::seeds_for(
            &store,
            Some(&m),
            None,
            &[],
            &["refresh before session creation".into()],
        )
        .unwrap();
        assert_eq!(s.theme_vecs.len(), 1);
        let ranked = rank_symbols(&store, &MapConfig::default(), None, &[], &s).unwrap();
        let fresh = ranked.iter().find(|s| s.name == "Freshness").unwrap();
        assert!(!fresh.reasons.themes.is_empty(), "{:?}", fresh.reasons);
        f.set_down(true);
        let off =
            crate::knowledge::seeds_for(&store, Some(&m), Some("x"), &[], &["y".into()]).unwrap();
        assert!(off.models_unavailable && off.query_vec.is_none() && off.theme_vecs.is_empty());
        let none = crate::knowledge::seeds_for(&store, None, Some("x"), &[], &[]).unwrap();
        assert!(
            !none.models_unavailable && none.query_vec.is_none(),
            "models disabled is not an outage"
        );
    }
}
