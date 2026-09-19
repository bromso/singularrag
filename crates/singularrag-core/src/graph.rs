//! File-level multigraph: one edge per (referencing file, defining file, name),
//! weight = ref count / number of files defining that name. Aider's construction.

use std::collections::{HashMap, HashSet};

use crate::config::MapConfig;
use crate::store::Store;
use crate::Result;

#[derive(Debug, Clone)]
pub struct FileNode {
    pub id: i64,
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct Edge {
    pub src: usize,
    pub dst: usize,
    pub name: String,
    pub weight: f64,
}

#[derive(Debug, Default)]
pub struct FileGraph {
    pub nodes: Vec<FileNode>,
    pub index_of: HashMap<i64, usize>,
    /// PageRank input: edges between *different* files.
    pub edges: Vec<Edge>,
    /// A file referencing a symbol it defines itself. Not a dependency between files,
    /// so it is kept out of PageRank, but it is a real use of the symbol and so counts
    /// for score distribution and for `referenced_by`.
    pub self_edges: Vec<Edge>,
    /// name -> number of distinct (included) files that define it.
    pub definer_count: HashMap<String, usize>,
}

/// Identifiers mentioned in the query get their edges weighted ×10 (Aider's rule).
pub const QUERY_IDENT_MULTIPLIER: f64 = 10.0;

pub fn build_graph(
    store: &Store,
    config: &MapConfig,
    query_terms: &[String],
    fts_names: &HashSet<String>,
) -> Result<FileGraph> {
    let conn = store.conn();
    let mut g = FileGraph::default();

    let mut stmt =
        conn.prepare("SELECT id, path FROM files WHERE skipped_reason IS NULL ORDER BY path")?;
    for row in stmt.query_map([], |r| {
        Ok(FileNode {
            id: r.get(0)?,
            path: r.get(1)?,
        })
    })? {
        let n = row?;
        if config.is_excluded(&n.path) {
            continue;
        }
        g.index_of.insert(n.id, g.nodes.len());
        g.nodes.push(n);
    }

    // name -> defining file ids (deduped)
    let mut definers: HashMap<String, Vec<usize>> = HashMap::new();
    let mut stmt = conn.prepare("SELECT DISTINCT name, file_id FROM symbols")?;
    for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
        let (name, fid) = row?;
        if let Some(&i) = g.index_of.get(&fid) {
            definers.entry(name).or_default().push(i);
        }
    }
    g.definer_count = definers
        .iter()
        .map(|(name, ids)| (name.clone(), ids.len()))
        .collect();

    let mut stmt =
        conn.prepare("SELECT file_id, name, COUNT(*) FROM refs GROUP BY file_id, name")?;
    for row in stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
        ))
    })? {
        let (fid, name, count) = row?;
        let Some(&src) = g.index_of.get(&fid) else {
            continue;
        };
        let Some(dsts) = definers.get(&name) else {
            continue;
        };
        let mut w = count as f64 / dsts.len() as f64;
        if query_ident_match(&name, query_terms, fts_names) {
            w *= QUERY_IDENT_MULTIPLIER;
        }
        for &dst in dsts {
            // Self-edges would distort PageRank (a file cannot make itself important),
            // so they are collected separately. The 1/N weight above is still computed
            // from the full definer count, including this file.
            let edge = Edge {
                src,
                dst,
                name: name.clone(),
                weight: w,
            };
            if dst == src {
                g.self_edges.push(edge);
            } else {
                g.edges.push(edge);
            }
        }
    }
    Ok(g)
}

/// A name counts as "named by the query" when the split-token comparison matches it or
/// when FTS matched it. The FTS arm is what makes stemming symmetric: "composed" reaches
/// `compose` through the porter tokenizer, and the term comparison alone never would.
pub fn query_ident_match(name: &str, terms: &[String], fts_names: &HashSet<String>) -> bool {
    name_matches_query(name, terms) || fts_names.contains(name)
}

/// A symbol name matches the query when every one of its split parts is a query term,
/// or the whole lowercased name is a query term.
pub fn name_matches_query(name: &str, terms: &[String]) -> bool {
    if terms.is_empty() {
        return false;
    }
    let lower = name.to_lowercase();
    if terms.contains(&lower) {
        return true;
    }
    let parts = crate::tokens::split_identifier(name);
    !parts.is_empty() && parts.iter().all(|p| terms.contains(p))
}
