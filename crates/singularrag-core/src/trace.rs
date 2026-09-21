//! `trace_path`: the shortest chain of references between two symbols, breadth-first
//! over the ranking's file graph in both directions. Name-based, like everything else.

use std::collections::{HashMap, HashSet, VecDeque};

use rusqlite::{params, OptionalExtension};

use crate::config::MapConfig;
use crate::graph::build_graph;
use crate::store::Store;
use crate::{Error, Result};

pub const MAX_PATH_DEPTH: usize = 6;

#[derive(Debug, Clone, PartialEq)]
pub struct Hop {
    pub from: String,
    pub to: String,
    /// The symbol name the edge carries.
    pub name: String,
    /// `true` when `from` references `name` defined in `to`; `false` when the edge is
    /// walked backwards (`to` references `name` defined in `from`).
    pub forward: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Trace {
    pub from: (String, String),
    pub to: (String, String),
    pub hops: Vec<Hop>,
}

fn symbol_exists(store: &Store, path: &str, name: &str) -> Result<bool> {
    let n: Option<i64> = store
        .conn()
        .query_row(
            "SELECT s.id FROM symbols s JOIN files f ON f.id = s.file_id WHERE f.path = ?1 AND s.name = ?2 LIMIT 1",
            params![path, name],
            |r| r.get(0),
        )
        .optional()?;
    Ok(n.is_some())
}

pub fn trace_path(
    store: &Store,
    config: &MapConfig,
    from_path: &str,
    from_symbol: &str,
    to_path: &str,
    to_symbol: &str,
) -> Result<Option<Trace>> {
    if !symbol_exists(store, from_path, from_symbol)? {
        return Err(Error::Config(format!(
            "from: {from_path}::{from_symbol} is not in the index"
        )));
    }
    if !symbol_exists(store, to_path, to_symbol)? {
        return Err(Error::Config(format!(
            "to: {to_path}::{to_symbol} is not in the index"
        )));
    }
    let endpoints = (
        (from_path.to_string(), from_symbol.to_string()),
        (to_path.to_string(), to_symbol.to_string()),
    );
    if from_path == to_path {
        return Ok(Some(Trace {
            from: endpoints.0,
            to: endpoints.1,
            hops: Vec::new(),
        }));
    }
    let g = build_graph(store, config, &[], &HashSet::new())?;
    let idx = |p: &str| g.nodes.iter().position(|n| n.path == p);
    let (Some(start), Some(goal)) = (idx(from_path), idx(to_path)) else {
        return Ok(None); // excluded from the graph
    };
    // adjacency: node -> (neighbour, name, forward)
    let mut adj: HashMap<usize, Vec<(usize, &str, bool)>> = HashMap::new();
    for e in &g.edges {
        adj.entry(e.src).or_default().push((e.dst, &e.name, true));
        adj.entry(e.dst).or_default().push((e.src, &e.name, false));
    }
    for v in adj.values_mut() {
        v.sort_by(|a, b| a.1.cmp(b.1).then(a.0.cmp(&b.0)));
    }
    let mut prev: HashMap<usize, (usize, String, bool)> = HashMap::new();
    let mut depth: HashMap<usize, usize> = HashMap::from([(start, 0)]);
    let mut queue = VecDeque::from([start]);
    while let Some(u) = queue.pop_front() {
        if u == goal {
            break;
        }
        let d = depth[&u];
        if d == MAX_PATH_DEPTH {
            continue;
        }
        for (v, name, forward) in adj.get(&u).into_iter().flatten() {
            if depth.contains_key(v) {
                continue;
            }
            depth.insert(*v, d + 1);
            prev.insert(*v, (u, name.to_string(), *forward));
            queue.push_back(*v);
        }
    }
    if !depth.contains_key(&goal) {
        return Ok(None);
    }
    let mut hops = Vec::new();
    let mut cur = goal;
    while cur != start {
        let (p, name, forward) = prev[&cur].clone();
        hops.push(Hop {
            from: g.nodes[p].path.clone(),
            to: g.nodes[cur].path.clone(),
            name,
            forward,
        });
        cur = p;
    }
    hops.reverse();
    Ok(Some(Trace {
        from: endpoints.0,
        to: endpoints.1,
        hops,
    }))
}

/// One line per hop, then `# N hops`. The left symbol of a hop is the symbol the walk
/// arrived at (the previous hop's name, or `from` for the first); the right symbol is the
/// hop's name, or `to`'s symbol on the last hop when they differ.
pub fn render_trace(t: &Trace) -> String {
    let mut out = String::new();
    if t.hops.is_empty() {
        out.push_str(&format!(
            "{}::{} → {}::{} (same file)\n# 0 hops\n",
            t.from.0, t.from.1, t.to.0, t.to.1
        ));
        return out;
    }
    let mut left = t.from.1.clone();
    let last = t.hops.len() - 1;
    for (i, h) in t.hops.iter().enumerate() {
        let right = if i == last {
            t.to.1.clone()
        } else {
            h.name.clone()
        };
        let via = if h.forward {
            format!("references {}", h.name)
        } else {
            format!("referenced through {}", h.name)
        };
        out.push_str(&format!(
            "{}::{} → {}::{} ({via})\n",
            h.from, left, h.to, right
        ));
        left = right;
    }
    let n = t.hops.len();
    out.push_str(&format!("# {n} hop{}\n", if n == 1 { "" } else { "s" }));
    out
}

/// The symbols along the path for provenance: `from`, each hop's named symbol where it is
/// defined, then `to`; deduplicated, in order. `(symbol_id, path, name, line_start, kind, signature)`.
#[allow(clippy::type_complexity)]
pub fn path_symbols(
    store: &Store,
    t: &Trace,
) -> Result<Vec<(i64, String, String, u32, String, String)>> {
    let mut wanted: Vec<(String, String)> = vec![t.from.clone()];
    for h in &t.hops {
        let defined_in = if h.forward { &h.to } else { &h.from };
        wanted.push((defined_in.clone(), h.name.clone()));
    }
    wanted.push(t.to.clone());
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut stmt = store.conn().prepare(
        "SELECT s.id, s.line_start, s.kind, s.signature FROM symbols s JOIN files f ON f.id = s.file_id
         WHERE f.path = ?1 AND s.name = ?2 ORDER BY s.line_start LIMIT 1",
    )?;
    for (path, name) in wanted {
        if !seen.insert((path.clone(), name.clone())) {
            continue;
        }
        let row: Option<(i64, u32, String, String)> = stmt
            .query_row(params![path, name], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .optional()?;
        if let Some((id, line, kind, sig)) = row {
            out.push((id, path, name, line, kind, sig));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MapConfig;
    use crate::fixture::write_ts_mini;
    use crate::index::Indexer;
    use crate::store::Store;

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
    fn one_hop_between_login_and_create_session() {
        let (_d, store) = indexed();
        let t = trace_path(
            &store,
            &MapConfig::default(),
            "src/cli/login.ts",
            "login",
            "src/auth/session.ts",
            "createSession",
        )
        .unwrap()
        .expect("a path");
        assert_eq!(t.hops.len(), 1);
        assert_eq!(t.hops[0].from, "src/cli/login.ts");
        assert_eq!(t.hops[0].to, "src/auth/session.ts");
        assert_eq!(t.hops[0].name, "createSession");
        assert!(t.hops[0].forward);
        let text = render_trace(&t);
        assert_eq!(
            text,
            "src/cli/login.ts::login → src/auth/session.ts::createSession (references createSession)\n# 1 hop\n"
        );
        let syms = path_symbols(&store, &t).unwrap();
        assert_eq!(
            syms.iter()
                .map(|s| format!("{}::{}", s.1, s.2))
                .collect::<Vec<_>>(),
            vec![
                "src/cli/login.ts::login",
                "src/auth/session.ts::createSession"
            ]
        );
    }

    #[test]
    fn reverse_edges_count_and_same_file_is_zero_hops() {
        let (_d, store) = indexed();
        // session.ts does not reference login.ts; the path goes backwards along the same edge.
        let t = trace_path(
            &store,
            &MapConfig::default(),
            "src/auth/session.ts",
            "createSession",
            "src/cli/login.ts",
            "login",
        )
        .unwrap()
        .expect("a path");
        assert_eq!(t.hops.len(), 1);
        assert!(!t.hops[0].forward);
        assert!(
            render_trace(&t).contains("(referenced through createSession)"),
            "{}",
            render_trace(&t)
        );
        let same = trace_path(
            &store,
            &MapConfig::default(),
            "src/auth/session.ts",
            "createSession",
            "src/auth/session.ts",
            "SessionStore",
        )
        .unwrap()
        .unwrap();
        assert!(same.hops.is_empty());
        assert_eq!(render_trace(&same), "src/auth/session.ts::createSession → src/auth/session.ts::SessionStore (same file)\n# 0 hops\n");
    }

    #[test]
    fn unknown_symbols_error_and_an_unreachable_file_is_none() {
        let (_d, store) = indexed();
        let e = trace_path(
            &store,
            &MapConfig::default(),
            "src/nope.ts",
            "x",
            "src/auth/session.ts",
            "createSession",
        )
        .unwrap_err()
        .to_string();
        assert!(
            e.contains("from: src/nope.ts::x is not in the index"),
            "{e}"
        );
        let e = trace_path(
            &store,
            &MapConfig::default(),
            "src/auth/session.ts",
            "createSession",
            "src/auth/session.ts",
            "nope",
        )
        .unwrap_err()
        .to_string();
        assert!(
            e.contains("to: src/auth/session.ts::nope is not in the index"),
            "{e}"
        );
        // Every fixture file is connected (login.ts imports log.ts and session.ts), so an
        // unreachable pair needs an exclude: without src/cli/, log.ts hangs alone.
        let cfg = MapConfig::parse("[[exclude]]\npath = \"src/cli/\"\n").unwrap();
        let none = trace_path(
            &store,
            &cfg,
            "src/util/log.ts",
            "log",
            "src/auth/session.ts",
            "createSession",
        )
        .unwrap();
        assert!(none.is_none());
    }
}
