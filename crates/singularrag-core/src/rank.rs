//! Personalised PageRank over the file graph, then distribution of file rank to the
//! symbols it defines. Every score carries a `Reasons` the UI can draw.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::config::MapConfig;
use crate::graph::{build_graph, query_ident_match, FileGraph};
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

/// A test, spec or benchmark file: it ranks as a referrer but is never served as a map
/// row, and `map.toml`'s `exclude` is still the way to drop a file from the graph.
pub fn is_support_file(path: &str) -> bool {
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

pub fn rank_symbols(
    store: &Store,
    config: &MapConfig,
    query: Option<&str>,
    focus_files: &[String],
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
    let fts_names: HashSet<String> = symbols
        .iter()
        .filter(|s| is_fts_hit(s.id))
        .map(|s| s.name.clone())
        .collect();

    let g: FileGraph = build_graph(store, config, &terms, &fts_names)?;
    let n = g.nodes.len();
    if n == 0 {
        return Ok(Vec::new());
    }
    symbols.retain(|s| g.index_of.contains_key(&s.file_id));

    let fts_files: HashSet<i64> = symbols
        .iter()
        .filter(|s| is_fts_hit(s.id))
        .map(|s| s.file_id)
        .collect();

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
    // many FTS hits shares one file rank between them.
    let mut group_size: HashMap<(usize, String), usize> = HashMap::new();
    let mut fts_in_file: HashMap<usize, usize> = HashMap::new();
    for s in &symbols {
        let fi = g.index_of[&s.file_id];
        *group_size.entry((fi, s.name.clone())).or_default() += 1;
        if is_fts_hit(s.id) {
            *fts_in_file.entry(fi).or_default() += 1;
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
            if fts_hit {
                score += fr / *fts_in_file.get(&fi).unwrap_or(&1) as f64;
            }
            let path = &g.nodes[fi].path;
            let has_symbol_note = !note_symbols.is_empty()
                && note_symbols.contains(&(path.clone(), s.name.clone()));
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
mod tests {
    use super::*;
    use crate::config::MapConfig;
    use crate::fixture::write_ts_mini;
    use crate::index::Indexer;
    use crate::store::Store;

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
        Indexer::new(&store, dir.path(), &MapConfig::default())
            .unwrap()
            .refresh(None)
            .unwrap();
        (dir, store)
    }

    #[test]
    fn most_referenced_definition_outranks_unreferenced_ones() {
        let (_dir, store) = indexed();
        let ranked = rank_symbols(&store, &MapConfig::default(), None, &[]).unwrap();
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
        Indexer::new(&store, dir.path(), &MapConfig::default())
            .unwrap()
            .refresh(None)
            .unwrap();
        let ranked = rank_symbols(&store, &MapConfig::default(), None, &[]).unwrap();

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
        let ranked = rank_symbols(&store, &MapConfig::default(), None, &[]).unwrap();
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
        let ranked = rank_symbols(&store, &MapConfig::default(), Some("sessions"), &[]).unwrap();
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
    }

    #[test]
    fn excluded_files_vanish_and_pins_are_recorded() {
        let (_dir, store) = indexed();
        let cfg = MapConfig::parse(
            "[[exclude]]\npath = \"src/util/\"\n[[pin]]\npath = \"src/cli/login.ts\"\n",
        )
        .unwrap();
        let ranked = rank_symbols(&store, &cfg, None, &[]).unwrap();
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
        let plain =
            rank_symbols(&store, &MapConfig::default(), Some("onboarding flow"), &[]).unwrap();
        let noted = rank_symbols(&store, &config, Some("onboarding flow"), &[]).unwrap();
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
}
