# singularrag map (plan 3b: the Sigma projection and boundaries) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the map view to `singularrag serve`: a Sigma.js projection of the ranking's file-level reference graph with the selected retrieval painted over it, a symbol's blast radius, and boundaries drawn from and edited into `map.toml`.

**Architecture:** Two new read routes on the existing axum router (`GET /api/graph` from the core's `build_graph`, `GET /api/blast` from a new core breadth-first walk over `refs`). In the browser, pure modules build a graphology graph, lay it out once per index version (cached per viewer) and compute hulls; `MapView` mounts Sigma 3 in an effect and paints through reducers; the detail panel gains boundary editing, blast list and symbol expansion so every map affordance has a keyboard twin; a Tree/Map toggle in the top bar. No new write route: boundaries go through `PUT /api/map`.

**Tech Stack:** Rust (axum 0.8, rusqlite, serde) in `crates/singularrag-core` and `crates/singularrag`; TypeScript, React 19, `sigma` 3.0.3 + `@sigma/node-border` 3.0.0, `graphology` 0.26, `graphology-layout-forceatlas2` 0.10, `graphology-layout-noverlap` 0.4, `graphology-layout` 0.6; bun test + happy-dom + Testing Library + axe-core.

**Spec:** `docs/superpowers/specs/2026-09-20-singularrag-map-design.md` (read it first; it binds this plan). Parent: `docs/superpowers/specs/2026-09-19-singularrag-design.md` §10, §11; serve spec `docs/superpowers/specs/2026-09-20-singularrag-serve-design.md` §3, §5, §6.

## Global Constraints

- Branch `map-v0` from `main` (5978e88). Commit after every task. `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace`, and in `ui/`: `bun run typecheck`, `bun test`, `bun run build` all green at the end of every task that touches that side.
- Security (serve spec §5): both new routes sit inside the `/api` router's existing layers (Host check, token, `no-store`); no new write route; `PUT /api/map` stays the single write path.
- `GET /api/graph` payload: `{ index_version, nodes: [{ path, symbols, lang }], edges: [{ src, dst, weight, names }] }`; nodes in path order, excluded files absent, self-edges dropped, one edge per ordered `(src, dst)` pair with summed `weight` and distinct-name count `names`.
- `GET /api/blast?path=&symbol=`: depth 0 the defining file; depth n+1 every non-excluded, non-skipped file with a `refs` row naming any symbol defined in a depth-n file, reported once at its minimum depth with `via` the reaching name (lowest depth, then alphabetical); caps depth 3 and 200 files; `truncated` is `null`, `"depth"` or `"files"`; 404 `{ "error": "symbol not found" }` for an unknown path/symbol. Implemented as a breadth-first walk in Rust over `refs`/`symbols`/`files` (the spec's "recursive CTE" wording is amended to this; same result).
- `PUT /api/map`: boundary names non-empty and unique → 422 with `field: "boundary[i].name"`.
- Status encoding on the map, never colour alone: served = filled node + solid label; cut = ring (hollow node with border) + label suffix ` · cut`; untouched = small grey node, no label until hovered/zoomed. Node size scales with symbol count, capped. Focused file: highlight ring, camera pans with no easing.
- Nothing animates: no layout animation, no camera easing, no transitions.
- The map container is `role="img"` with `aria-label` = `summaryLabel(...)`: `Map of N files. Retrieval R: S served, C cut, U untouched. B boundaries.` / `Map of N files. No retrieval selected. B boundaries.` (B = 1 → `1 boundary`). A "Switch to table" button precedes it in focus order; the canvas is not tabbable.
- Live region copy: `Map layout ready` once per index version; `Blast radius: N files to depth D`.
- Toggle: a radiogroup labelled `View` with `Tree` and `Map`; Tree default; persisted in localStorage key `singularrag.view`.
- Layout cache: localStorage key `singularrag.layout.<index_version>`, wrapped in try/catch; a version change seeds from the previous positions.
- Boundary editing copy: chips `Remove from <name>`; input labelled `Add to boundary` with a `<datalist>` of existing names; button `Add`. Toast on save is the existing `Saved. Applies to the next retrieval.`
- Panel copy: buttons `Show symbols` / `Hide symbols` (file), `Show blast radius` / `Hide blast radius` (symbol); blast list heading `Blast radius`.
- Tests never touch WebGL: `MapView` tests mock the `sigma` and `@sigma/node-border` modules.

---

## File structure

```
crates/singularrag-core/src/blast.rs          # blast_radius(): BFS over refs; Blast/BlastFile types; tests
crates/singularrag-core/src/lib.rs            # + pub mod blast
crates/singularrag-core/src/config.rs         # validate(): boundary names non-empty, unique
crates/singularrag/src/serve/queries.rs       # + graph(store, config) -> GraphDto; DTOs
crates/singularrag/src/serve/routes.rs        # + graph, blast handlers
crates/singularrag/src/serve/mod.rs           # + two routes
crates/singularrag/tests/serve.rs             # + graph/blast integration tests
ui/package.json                               # + sigma, @sigma/node-border, graphology, layouts
ui/src/api/types.ts                           # + GraphPayload, GraphNode, GraphEdge, BlastResult, BlastFile
ui/src/api/client.ts                          # + api.graph(), api.blast(path, symbol)
ui/src/lib/graph.ts (+ .test.ts)              # buildGraph, layoutGraph, layout cache
ui/src/lib/hull.ts (+ .test.ts)               # convexHull, padHull, hullFor
ui/src/lib/mapEdits.ts (+ .test.ts)           # + boundariesOf, addToBoundary, removeFromBoundary
ui/src/lib/mapSummary.ts (+ .test.ts)         # summaryLabel
ui/src/lib/mapStyle.ts (+ .test.ts)           # pure node/edge style decisions used by the reducers
ui/src/components/MapView.tsx (+ .test.tsx)   # Sigma mount, reducers, hull layer, expansion, blast paint
ui/src/components/ViewToggle.tsx (+ .test.tsx)# radiogroup Tree/Map, persisted
ui/src/components/DetailPanel.tsx             # + boundaries line, Show symbols, Show blast radius, blast list
ui/src/App.tsx (+ App.test.tsx)               # toggle, graph fetch, blast fetch, expansion state, sync, announcements
README.md                                     # map paragraph
```

---

### Task 1: Blast radius in core

**Files:**
- Create: `crates/singularrag-core/src/blast.rs`
- Modify: `crates/singularrag-core/src/lib.rs` (add `pub mod blast;` after `pub mod graph;`)
- Test: unit tests in `blast.rs`

**Interfaces:**
- Produces: `blast::BlastFile { path: String, depth: u32, via: String }`, `blast::Blast { root_path: String, root_symbol: String, files: Vec<BlastFile>, truncated: Option<&'static str> }` (both `Serialize`), `blast::blast_radius(store: &Store, config: &MapConfig, path: &str, symbol: &str, max_depth: u32, max_files: usize) -> Result<Option<Blast>>` (`None` when `path` does not define `symbol`), constants `blast::MAX_DEPTH: u32 = 3`, `blast::MAX_FILES: usize = 200`.
- Consumes: `Store::conn()`, `MapConfig::is_excluded(&str)`.

- [ ] **Step 1: Failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MapConfig;
    use crate::engine::Engine;
    use crate::fixture::write_ts_mini;
    use crate::store::Store;

    fn engine() -> (tempfile::TempDir, Engine) {
        let dir = tempfile::tempdir().unwrap();
        write_ts_mini(dir.path());
        let mut e = Engine::open(dir.path(), "t").unwrap();
        e.refresh(std::time::Duration::from_secs(5)).unwrap();
        (dir, e)
    }

    #[test]
    fn direct_referencers_are_depth_one_with_the_reaching_name() {
        let (_d, e) = engine();
        let b = blast_radius(e.store(), &MapConfig::default(), "src/auth/session.ts", "createSession", MAX_DEPTH, MAX_FILES).unwrap().unwrap();
        assert_eq!(b.root_path, "src/auth/session.ts");
        assert_eq!(b.root_symbol, "createSession");
        let paths: Vec<(&str, u32, &str)> = b.files.iter().map(|f| (f.path.as_str(), f.depth, f.via.as_str())).collect();
        assert_eq!(paths, vec![("src/cli/login.ts", 1, "createSession"), ("src/http/middleware.ts", 1, "createSession")]);
        assert_eq!(b.truncated, None);
    }

    #[test]
    fn unknown_symbol_or_path_is_none() {
        let (_d, e) = engine();
        assert!(blast_radius(e.store(), &MapConfig::default(), "src/auth/session.ts", "nope", MAX_DEPTH, MAX_FILES).unwrap().is_none());
        assert!(blast_radius(e.store(), &MapConfig::default(), "src/nope.ts", "createSession", MAX_DEPTH, MAX_FILES).unwrap().is_none());
    }

    #[test]
    fn excluded_files_are_omitted() {
        let (_d, e) = engine();
        let cfg = MapConfig::parse("[[exclude]]\npath = \"src/cli/\"\n").unwrap();
        let b = blast_radius(e.store(), &cfg, "src/auth/session.ts", "createSession", MAX_DEPTH, MAX_FILES).unwrap().unwrap();
        assert_eq!(b.files.len(), 1);
        assert_eq!(b.files[0].path, "src/http/middleware.ts");
    }

    /// Three files in a cycle: a.ts defines `a`, b.ts defines `b` and references `a`,
    /// c.ts defines `c` and references `b`, a.ts references `c`.
    fn cycle_store() -> Store {
        let s = Store::open_in_memory().unwrap();
        let c = s.conn();
        for (id, path) in [(1, "a.ts"), (2, "b.ts"), (3, "c.ts")] {
            c.execute("INSERT INTO files (id, path, lang, content_hash, mtime_ms, size, indexed_at_ms) VALUES (?1, ?2, 'typescript', 'h', 0, 1, 0)", rusqlite::params![id, path]).unwrap();
        }
        for (fid, name) in [(1, "a"), (2, "b"), (3, "c")] {
            c.execute("INSERT INTO symbols (file_id, name, kind, line_start, line_end, signature) VALUES (?1, ?2, 'function', 1, 1, ?2)", rusqlite::params![fid, name]).unwrap();
        }
        for (fid, name) in [(2, "a"), (3, "b"), (1, "c")] {
            c.execute("INSERT INTO refs (file_id, name, line) VALUES (?1, ?2, 1)", rusqlite::params![fid, name]).unwrap();
        }
        s
    }

    #[test]
    fn walk_follows_depths_and_a_cycle_terminates() {
        let s = cycle_store();
        let b = blast_radius(&s, &MapConfig::default(), "a.ts", "a", MAX_DEPTH, MAX_FILES).unwrap().unwrap();
        let got: Vec<(&str, u32, &str)> = b.files.iter().map(|f| (f.path.as_str(), f.depth, f.via.as_str())).collect();
        assert_eq!(got, vec![("b.ts", 1, "a"), ("c.ts", 2, "b")]);
        assert_eq!(b.truncated, None, "a.ts is depth 0 and never re-added");
    }

    #[test]
    fn depth_cap_reports_truncation_only_when_more_was_reachable() {
        let s = cycle_store();
        let b = blast_radius(&s, &MapConfig::default(), "a.ts", "a", 1, MAX_FILES).unwrap().unwrap();
        assert_eq!(b.files.len(), 1);
        assert_eq!(b.truncated, Some("depth"));
        let b = blast_radius(&s, &MapConfig::default(), "a.ts", "a", 2, MAX_FILES).unwrap().unwrap();
        assert_eq!(b.truncated, None, "depth 3 would add nothing new");
    }

    #[test]
    fn file_cap_stops_the_walk() {
        let s = cycle_store();
        let b = blast_radius(&s, &MapConfig::default(), "a.ts", "a", MAX_DEPTH, 1).unwrap().unwrap();
        assert_eq!(b.files.len(), 1);
        assert_eq!(b.truncated, Some("files"));
    }
}
```

- [ ] **Step 2: Run to see them fail** — `cargo test -p singularrag-core blast::` fails to compile (module missing). Add `pub mod blast;` to `lib.rs` and an empty module with the constants, confirm the tests fail on missing items.

- [ ] **Step 3: Implement**

```rust
//! Blast radius: which files break if a symbol changes. A breadth-first walk over
//! `refs`: depth 1 is every file referencing the symbol's name, depth n+1 every file
//! referencing any symbol defined in a depth-n file. Name-based, like the ranking graph.

use std::collections::{BTreeMap, HashSet};

use rusqlite::{params, OptionalExtension};
use serde::Serialize;

use crate::config::MapConfig;
use crate::store::Store;
use crate::Result;

pub const MAX_DEPTH: u32 = 3;
pub const MAX_FILES: usize = 200;

#[derive(Debug, Clone, Serialize)]
pub struct BlastFile {
    pub path: String,
    pub depth: u32,
    /// The symbol name through which this file was first reached.
    pub via: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Blast {
    pub root_path: String,
    pub root_symbol: String,
    pub files: Vec<BlastFile>,
    /// `None`, or which cap stopped the walk: `"depth"` or `"files"`.
    pub truncated: Option<&'static str>,
}

fn defined_names(store: &Store, file_ids: &[i64]) -> Result<Vec<String>> {
    let mut names = Vec::new();
    let mut stmt = store.conn().prepare("SELECT DISTINCT name FROM symbols WHERE file_id = ?1")?;
    for fid in file_ids {
        for n in stmt.query_map(params![fid], |r| r.get::<_, String>(0))? {
            names.push(n?);
        }
    }
    names.sort();
    names.dedup();
    Ok(names)
}

/// Files (id, path) that reference `name`, non-skipped, in path order.
fn referencing_files(store: &Store, name: &str) -> Result<Vec<(i64, String)>> {
    let mut stmt = store.conn().prepare(
        "SELECT DISTINCT f.id, f.path FROM refs r JOIN files f ON f.id = r.file_id
         WHERE r.name = ?1 AND f.skipped_reason IS NULL ORDER BY f.path",
    )?;
    let rows = stmt
        .query_map(params![name], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn blast_radius(
    store: &Store,
    config: &MapConfig,
    path: &str,
    symbol: &str,
    max_depth: u32,
    max_files: usize,
) -> Result<Option<Blast>> {
    let root: Option<i64> = store
        .conn()
        .query_row(
            "SELECT f.id FROM symbols s JOIN files f ON f.id = s.file_id WHERE f.path = ?1 AND s.name = ?2 LIMIT 1",
            params![path, symbol],
            |r| r.get(0),
        )
        .optional()?;
    let Some(root_id) = root else { return Ok(None) };

    let mut seen: HashSet<i64> = HashSet::from([root_id]);
    let mut files: Vec<BlastFile> = Vec::new();
    let mut truncated = None;
    let mut frontier: Vec<i64> = vec![root_id];
    // Depth 1 follows the root symbol's own name only; deeper levels follow every
    // symbol the frontier files define.
    let mut names: Vec<String> = vec![symbol.to_string()];
    let mut depth = 0u32;
    'walk: while !frontier.is_empty() && !names.is_empty() {
        if depth == max_depth {
            // Something is still reachable: report the depth cap only if a next level
            // would have added a file we have not seen.
            for n in &names {
                if referencing_files(store, n)?.iter().any(|(id, p)| !seen.contains(id) && !config.is_excluded(p)) {
                    truncated = Some("depth");
                    break;
                }
            }
            break;
        }
        depth += 1;
        // path -> (id, via): a file reached by several names keeps the alphabetically
        // first name, which `names` is sorted for.
        let mut next: BTreeMap<String, (i64, String)> = BTreeMap::new();
        for n in &names {
            for (id, p) in referencing_files(store, n)? {
                if seen.contains(&id) || config.is_excluded(&p) || next.contains_key(&p) {
                    continue;
                }
                next.insert(p, (id, n.clone()));
            }
        }
        frontier.clear();
        for (p, (id, via)) in next {
            if files.len() == max_files {
                truncated = Some("files");
                break 'walk;
            }
            seen.insert(id);
            frontier.push(id);
            files.push(BlastFile { path: p, depth, via });
        }
        names = defined_names(store, &frontier)?;
    }
    Ok(Some(Blast { root_path: path.to_string(), root_symbol: symbol.to_string(), files, truncated }))
}
```

- [ ] **Step 4: Tests green, lint, commit**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test -p singularrag-core blast::`
```bash
git add -A
git commit -m "feat(core): blast radius as a breadth-first walk over refs"
```

---

### Task 2: `/api/graph`, `/api/blast`, boundary name validation

**Files:**
- Modify: `crates/singularrag-core/src/config.rs` (validate), `crates/singularrag/src/serve/queries.rs` (graph DTOs + query), `crates/singularrag/src/serve/routes.rs` (two handlers), `crates/singularrag/src/serve/mod.rs` (two routes)
- Test: unit tests in `config.rs` and `queries.rs`; integration tests in `crates/singularrag/tests/serve.rs`

**Interfaces:**
- Produces: `queries::GraphNode { path: String, symbols: usize, lang: Option<String> }`, `queries::GraphEdge { src: usize, dst: usize, weight: f64, names: usize }`, `queries::GraphDto { index_version: String, nodes: Vec<GraphNode>, edges: Vec<GraphEdge> }`, `queries::graph(store: &Store, config: &MapConfig) -> Result<GraphDto>`; routes `GET /api/graph` → `GraphDto`, `GET /api/blast?path&symbol` → `blast::Blast` as `{ "root": { "path", "symbol" }, "files": [...], "truncated": null|"depth"|"files" }` (a small `BlastDto` in routes.rs maps `Blast` to that shape), 404 JSON when `None`, 400 JSON `{ "error": "path and symbol are required" }` when either query param is missing.
- Consumes: `singularrag_core::graph::build_graph`, `singularrag_core::blast::{blast_radius, MAX_DEPTH, MAX_FILES}`, `MapConfig::load(&root)`.

- [ ] **Step 1: Failing tests**

`config.rs` tests (append to the existing `mod tests`):
```rust
    #[test]
    fn boundary_names_must_be_unique_and_non_empty() {
        let c = cfg("[[boundary]]\nname = \"auth\"\npaths = [\"src/a\"]\n[[boundary]]\nname = \"auth\"\npaths = [\"src/b\"]\n");
        let e = c.validate(&[]).unwrap_err();
        assert_eq!(e.field, "boundary[1].name");
        assert!(e.message.contains("duplicate"));
        let c = cfg("[[boundary]]\nname = \"  \"\npaths = [\"src/a\"]\n");
        let e = c.validate(&[]).unwrap_err();
        assert_eq!(e.field, "boundary[0].name");
        assert!(e.message.contains("empty"));
    }
```
(`cfg` is the existing helper in that module.)

`queries.rs` tests (append; the module already has `seeded()` returning `(TempDir, Store)` over `write_ts_mini` after a `repo_map`):
```rust
    #[test]
    fn graph_projects_files_and_aggregated_edges() {
        let (_d, store) = seeded();
        let g = graph(&store, &MapConfig::default()).unwrap();
        assert!(!g.index_version.is_empty());
        let paths: Vec<&str> = g.nodes.iter().map(|n| n.path.as_str()).collect();
        // Same files as the tree (non-skipped; `src/config.ts` is skipped by the secret scan), in path order.
        assert_eq!(g.nodes.len(), tree(&store).unwrap().len());
        assert!(paths.windows(2).all(|w| w[0] < w[1]), "path order: {paths:?}");
        for p in ["src/auth/session.ts", "src/cli/login.ts", "src/http/middleware.ts", "src/util/log.ts"] {
            assert!(paths.contains(&p), "{p} missing from {paths:?}");
        }
        let session = paths.iter().position(|p| *p == "src/auth/session.ts").unwrap();
        let middleware = paths.iter().position(|p| *p == "src/http/middleware.ts").unwrap();
        assert!(g.nodes[session].symbols >= 4);
        assert_eq!(g.nodes[session].lang.as_deref(), Some("typescript"));
        let e = g.edges.iter().find(|e| e.src == middleware && e.dst == session).expect("middleware -> session edge");
        assert!(e.weight > 0.0);
        assert!(e.names >= 1, "createSession and Session are distinct names behind one edge");
        assert!(g.edges.iter().all(|e| e.src != e.dst), "self-edges dropped");
        assert_eq!(g.edges.iter().filter(|e| e.src == middleware && e.dst == session).count(), 1, "one edge per pair");
    }

    #[test]
    fn graph_omits_excluded_files() {
        let (_d, store) = seeded();
        let cfg = MapConfig::parse("[[exclude]]\npath = \"src/cli/\"\n").unwrap();
        let g = graph(&store, &cfg).unwrap();
        assert!(g.nodes.iter().all(|n| !n.path.starts_with("src/cli/")));
    }
```
Add `use singularrag_core::config::MapConfig;` to that test module if absent.

`tests/serve.rs` (append; helpers `spawn`, `get`, `client` exist):
```rust
#[tokio::test]
async fn graph_and_blast_are_served_behind_the_token() {
    let dir = tempfile::tempdir().unwrap();
    write_ts_mini(dir.path());
    let s = spawn(dir.path());
    let r = client().get(format!("{}/api/graph", s.url)).send().await.unwrap();
    assert_eq!(r.status(), 401);
    let g: serde_json::Value = get(&s, "/graph").await.json().await.unwrap();
    let tree: serde_json::Value = get(&s, "/tree").await.json().await.unwrap();
    assert_eq!(g["nodes"].as_array().unwrap().len(), tree.as_array().unwrap().len());
    assert!(g["edges"].as_array().unwrap().len() >= 2);
    let b: serde_json::Value = get(&s, "/blast?path=src/auth/session.ts&symbol=createSession").await.json().await.unwrap();
    assert_eq!(b["root"]["symbol"], "createSession");
    assert_eq!(b["files"][0]["depth"], 1);
    assert!(b["truncated"].is_null());
    let r = get(&s, "/blast?path=src/auth/session.ts&symbol=nope").await;
    assert_eq!(r.status(), 404);
    assert_eq!(r.headers().get("cache-control").unwrap(), "no-store");
    let r = get(&s, "/blast?path=src/auth/session.ts").await;
    assert_eq!(r.status(), 400);
}

#[tokio::test]
async fn put_map_rejects_duplicate_boundary_names() {
    let dir = tempfile::tempdir().unwrap();
    write_ts_mini(dir.path());
    let s = spawn(dir.path());
    let body = serde_json::json!({ "pin": [], "exclude": [], "note": [], "boundary": [ { "name": "x", "paths": ["src/a.ts"] }, { "name": "x", "paths": ["src/b.ts"] } ], "deny": { "extra_patterns": [] } });
    let r = client().put(format!("{}/api/map", s.url)).bearer_auth(&s.token).json(&body).send().await.unwrap();
    assert_eq!(r.status(), 422);
    let e: serde_json::Value = r.json().await.unwrap();
    assert_eq!(e["field"], "boundary[1].name");
}
```

- [ ] **Step 2: Run to see them fail.**

- [ ] **Step 3: Implement**

`config.rs`, inside `validate` after the boundary path loop:
```rust
        let mut seen_names = std::collections::HashSet::new();
        for (i, b) in self.boundary.iter().enumerate() {
            let name = b.name.trim();
            if name.is_empty() {
                return Err(MapConfigError { field: format!("boundary[{i}].name"), message: "boundary name is empty".into() });
            }
            if !seen_names.insert(name.to_string()) {
                return Err(MapConfigError { field: format!("boundary[{i}].name"), message: format!("duplicate boundary name {name}") });
            }
        }
```

`queries.rs`:
```rust
#[derive(Debug, Serialize)]
pub struct GraphNode {
    pub path: String,
    pub symbols: usize,
    pub lang: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct GraphEdge {
    pub src: usize,
    pub dst: usize,
    pub weight: f64,
    pub names: usize,
}

#[derive(Debug, Serialize)]
pub struct GraphDto {
    pub index_version: String,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

/// The ranking's file graph, projected for drawing: excluded files absent, self-edges
/// dropped, one edge per ordered pair (spec §3).
pub fn graph(store: &Store, config: &MapConfig) -> Result<GraphDto> {
    let g = singularrag_core::graph::build_graph(store, config, &[], &std::collections::HashSet::new())?;
    let conn = store.conn();
    let mut counts = std::collections::HashMap::<i64, usize>::new();
    let mut stmt = conn.prepare("SELECT file_id, COUNT(*) FROM symbols GROUP BY file_id")?;
    for row in stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, usize>(1)?)))? {
        let (fid, n) = row?;
        counts.insert(fid, n);
    }
    let mut langs = std::collections::HashMap::<i64, Option<String>>::new();
    let mut stmt = conn.prepare("SELECT id, lang FROM files")?;
    for row in stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?)))? {
        let (fid, l) = row?;
        langs.insert(fid, l);
    }
    let nodes = g
        .nodes
        .iter()
        .map(|n| GraphNode { path: n.path.clone(), symbols: counts.get(&n.id).copied().unwrap_or(0), lang: langs.get(&n.id).cloned().flatten() })
        .collect();
    // (src, dst) -> (weight sum, distinct names)
    let mut agg = std::collections::BTreeMap::<(usize, usize), (f64, std::collections::BTreeSet<String>)>::new();
    for e in &g.edges {
        if e.src == e.dst {
            continue;
        }
        let entry = agg.entry((e.src, e.dst)).or_insert((0.0, Default::default()));
        entry.0 += e.weight;
        entry.1.insert(e.name.clone());
    }
    let edges = agg.into_iter().map(|((src, dst), (weight, names))| GraphEdge { src, dst, weight, names: names.len() }).collect();
    Ok(GraphDto { index_version: store.get_meta("index_version")?.unwrap_or_default(), nodes, edges })
}
```
(`use singularrag_core::config::MapConfig;` at the top if not already imported.)

`routes.rs`:
```rust
pub async fn graph(State(s): State<AppState>) -> Result<Json<queries::GraphDto>, ApiError> {
    let config = MapConfig::load(&s.root)?;
    Ok(Json(locked(&s, |store| queries::graph(store, &config))?))
}

#[derive(Deserialize)]
pub struct BlastQuery {
    pub path: Option<String>,
    pub symbol: Option<String>,
}

#[derive(Serialize)]
pub struct BlastRoot {
    pub path: String,
    pub symbol: String,
}

#[derive(Serialize)]
pub struct BlastDto {
    pub root: BlastRoot,
    pub files: Vec<singularrag_core::blast::BlastFile>,
    pub truncated: Option<&'static str>,
}

pub async fn blast(State(s): State<AppState>, Query(q): Query<BlastQuery>) -> Result<Json<BlastDto>, ApiError> {
    let (Some(path), Some(symbol)) = (q.path, q.symbol) else {
        return Err(ApiError(StatusCode::BAD_REQUEST, serde_json::json!({ "error": "path and symbol are required" })));
    };
    let config = MapConfig::load(&s.root)?;
    let b = locked(&s, |store| {
        singularrag_core::blast::blast_radius(store, &config, &path, &symbol, singularrag_core::blast::MAX_DEPTH, singularrag_core::blast::MAX_FILES)
    })?;
    match b {
        None => Err(ApiError(StatusCode::NOT_FOUND, serde_json::json!({ "error": "symbol not found" }))),
        Some(b) => Ok(Json(BlastDto { root: BlastRoot { path: b.root_path, symbol: b.root_symbol }, files: b.files, truncated: b.truncated })),
    }
}
```
`mod.rs`: add `.route("/graph", get(routes::graph))` and `.route("/blast", get(routes::blast))` next to `/tree`.

- [ ] **Step 4: Tests green, lint, commit**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test --workspace`
```bash
git add -A
git commit -m "feat(serve): graph and blast read routes, boundary name validation"
```

---
### Task 3: Browser graph model: dependencies, types, client, layout, hulls, style, summary

**Files:**
- Modify: `ui/package.json` (dependencies), `ui/src/api/types.ts`, `ui/src/api/client.ts`
- Create: `ui/src/lib/graph.ts`, `ui/src/lib/graph.test.ts`, `ui/src/lib/hull.ts`, `ui/src/lib/hull.test.ts`, `ui/src/lib/mapStyle.ts`, `ui/src/lib/mapStyle.test.ts`, `ui/src/lib/mapSummary.ts`, `ui/src/lib/mapSummary.test.ts`
- Test: the five `.test.ts` files; `client.test.ts` gains two cases

**Interfaces:**
- Produces (types.ts): `GraphNode { path: string; symbols: number; lang: string | null }`, `GraphEdge { src: number; dst: number; weight: number; names: number }`, `GraphPayload { index_version: string; nodes: GraphNode[]; edges: GraphEdge[] }`, `BlastFile { path: string; depth: number; via: string }`, `BlastResult { root: { path: string; symbol: string }; files: BlastFile[]; truncated: null | "depth" | "files" }`.
- Produces (client.ts): `api.graph(): Promise<GraphPayload>`, `api.blast(path: string, symbol: string): Promise<BlastResult>` (query-string encoded with `encodeURIComponent`).
- Produces (graph.ts): `type Positions = Record<string, { x: number; y: number }>`; `buildGraph(payload: GraphPayload): Graph` (graphology, undirected, non-multi; node attrs `symbols`, `lang`; edge attrs `weight`, `names`; both directions of a pair merge with summed weight and max names); `layoutGraph(graph: Graph, seed?: Positions): Positions` (deterministic); `layoutCacheKey(v: string) => \`singularrag.layout.${v}\``; `loadLayout(v: string): Positions | null`; `saveLayout(v: string, p: Positions): void`; `applyPositions(graph, positions)` sets `x`/`y` on every node.
- Produces (hull.ts): `type Pt = { x: number; y: number }`; `convexHull(points: Pt[]): Pt[]` (counter-clockwise, no duplicates; 0 → [], 1 → [p], 2 → [a, b]); `padHull(points: Pt[], padding: number): Pt[]` (1 point → 12-gon circle of radius `padding`; 2 points → 16-point pill; ≥3 → each vertex pushed outward from the centroid by `padding`).
- Produces (mapStyle.ts): `type NodeCtx = { status: "served" | "cut" | "untouched" | null; focused: boolean; blastDepth: number | null; blastActive: boolean; symbols: number; hovered: boolean; zoomRatio: number }`; `nodeStyle(path: string, ctx: NodeCtx, palette: Palette): { size: number; color: string; borderColor?: string; type: "circle" | "border"; label: string | null; zIndex: number }`; `type EdgeCtx = { touchesFocused: boolean; blastActive: boolean; touchesBlast: boolean }`; `edgeStyle(ctx: EdgeCtx, palette: Palette): { color: string; size: number; hidden: boolean }`; `type Palette = { served: string; cut: string; untouched: string; focus: string; edge: string; edgeDim: string; label: string; background: string }`; `readPalette(el: HTMLElement): Palette` reads CSS custom properties `--map-served`, `--map-cut`, `--map-untouched`, `--map-focus`, `--map-edge`, `--map-edge-dim`, `--foreground`, `--background` via `getComputedStyle`, falling back to fixed hex values; `nodeSize(symbols: number) = Math.min(4 + Math.sqrt(symbols) * 1.5, 14)`.
- Produces (mapSummary.ts): `summaryLabel(files: number, retrieval: { id: number; served: number; cut: number } | null, boundaries: number): string` per Global Constraints.

- [ ] **Step 1: Dependencies**

From `ui/`: `bun add sigma@3.0.3 @sigma/node-border@3.0.0 graphology@0.26.0 graphology-layout-forceatlas2@0.10.1 graphology-layout-noverlap@0.4.2 graphology-layout@0.6.1`. Then `bun install` and confirm `bun run typecheck` still passes (graphology ships types; if `graphology-types` is required by the compiler, add it as a devDependency with `bun add -d graphology-types`).

- [ ] **Step 2: Types and client**

Append the five types above to `ui/src/api/types.ts`. In `client.ts` add to `api`:
```ts
  graph: () => req<GraphPayload>("GET", "/graph"),
  blast: (path: string, symbol: string) => req<BlastResult>("GET", `/blast?path=${encodeURIComponent(path)}&symbol=${encodeURIComponent(symbol)}`),
```
and the imports. `client.test.ts` gains, inside a new `describe("api.graph() and api.blast()")` using the file's existing mocked-fetch pattern: `blast` encodes `src/a b.ts` and `c::d` into the query string; `graph` hits `/api/graph` with the bearer header.

- [ ] **Step 3: Failing tests for the pure modules**

`ui/src/lib/graph.test.ts`:
```ts
import { describe, expect, test, beforeEach } from "bun:test";
import { applyPositions, buildGraph, layoutCacheKey, layoutGraph, loadLayout, saveLayout } from "./graph";
import type { GraphPayload } from "@/api/types";

const payload: GraphPayload = {
  index_version: "v1",
  nodes: [{ path: "a.ts", symbols: 3, lang: "typescript" }, { path: "b.ts", symbols: 1, lang: null }, { path: "c.ts", symbols: 9, lang: "typescript" }],
  edges: [{ src: 1, dst: 0, weight: 1, names: 1 }, { src: 0, dst: 1, weight: 0.5, names: 2 }, { src: 2, dst: 0, weight: 2, names: 1 }],
};

describe("buildGraph", () => {
  test("nodes carry symbols and lang; a pair present in both directions is one undirected edge", () => {
    const g = buildGraph(payload);
    expect(g.order).toBe(3);
    expect(g.size).toBe(2);
    expect(g.getNodeAttribute("c.ts", "symbols")).toBe(9);
    expect(g.getNodeAttribute("b.ts", "lang")).toBeNull();
    expect(g.getEdgeAttribute(g.edge("a.ts", "b.ts")!, "weight")).toBeCloseTo(1.5);
    expect(g.getEdgeAttribute(g.edge("a.ts", "b.ts")!, "names")).toBe(2);
  });
});

describe("layoutGraph", () => {
  test("is deterministic and places every node", () => {
    const p1 = layoutGraph(buildGraph(payload));
    const p2 = layoutGraph(buildGraph(payload));
    expect(Object.keys(p1).sort()).toEqual(["a.ts", "b.ts", "c.ts"]);
    expect(p1).toEqual(p2);
    expect(Number.isFinite(p1["a.ts"].x)).toBe(true);
  });

  test("a seed keeps surviving nodes near their old positions", () => {
    const seed = { "a.ts": { x: 100, y: 100 }, "b.ts": { x: 110, y: 100 }, "zombie.ts": { x: 0, y: 0 } };
    const p = layoutGraph(buildGraph(payload), seed);
    expect(p["zombie.ts"]).toBeUndefined();
    // Seeded nodes stay in the same neighbourhood; the unseeded node is placed too.
    expect(Math.abs(p["a.ts"].x - 100)).toBeLessThan(60);
    expect(p["c.ts"]).toBeDefined();
  });
});

describe("layout cache", () => {
  beforeEach(() => localStorage.clear());
  test("round-trips per index version and tolerates garbage", () => {
    expect(loadLayout("v1")).toBeNull();
    saveLayout("v1", { "a.ts": { x: 1, y: 2 } });
    expect(loadLayout("v1")).toEqual({ "a.ts": { x: 1, y: 2 } });
    expect(loadLayout("v2")).toBeNull();
    localStorage.setItem(layoutCacheKey("v3"), "{not json");
    expect(loadLayout("v3")).toBeNull();
  });
});

describe("applyPositions", () => {
  test("sets x and y on every node, origin for unknown ones", () => {
    const g = buildGraph(payload);
    applyPositions(g, { "a.ts": { x: 5, y: 6 } });
    expect(g.getNodeAttributes("a.ts")).toMatchObject({ x: 5, y: 6 });
    expect(g.getNodeAttributes("b.ts")).toMatchObject({ x: 0, y: 0 });
  });
});
```

`ui/src/lib/hull.test.ts`:
```ts
import { describe, expect, test } from "bun:test";
import { convexHull, padHull } from "./hull";

describe("convexHull", () => {
  test("degenerate inputs", () => {
    expect(convexHull([])).toEqual([]);
    expect(convexHull([{ x: 1, y: 1 }])).toEqual([{ x: 1, y: 1 }]);
    expect(convexHull([{ x: 0, y: 0 }, { x: 2, y: 0 }])).toEqual([{ x: 0, y: 0 }, { x: 2, y: 0 }]);
  });
  test("drops interior points and returns counter-clockwise vertices", () => {
    const h = convexHull([{ x: 0, y: 0 }, { x: 4, y: 0 }, { x: 4, y: 4 }, { x: 0, y: 4 }, { x: 2, y: 2 }, { x: 1, y: 1 }]);
    expect(h).toHaveLength(4);
    expect(h).not.toContainEqual({ x: 2, y: 2 });
    // Shoelace area positive => counter-clockwise.
    let area = 0;
    for (let i = 0; i < h.length; i++) { const a = h[i], b = h[(i + 1) % h.length]; area += a.x * b.y - b.x * a.y; }
    expect(area).toBeGreaterThan(0);
  });
});

describe("padHull", () => {
  test("one point becomes a circle, two a pill, three or more grow outward", () => {
    expect(padHull([{ x: 0, y: 0 }], 10)).toHaveLength(12);
    expect(padHull([{ x: 0, y: 0 }, { x: 10, y: 0 }], 5)).toHaveLength(16);
    const tri = padHull([{ x: 0, y: 0 }, { x: 10, y: 0 }, { x: 0, y: 10 }], 2);
    expect(tri).toHaveLength(3);
    expect(tri[0].x).toBeLessThan(0);
    expect(tri[0].y).toBeLessThan(0);
  });
});
```

`ui/src/lib/mapStyle.test.ts`:
```ts
import { describe, expect, test } from "bun:test";
import { edgeStyle, nodeSize, nodeStyle, readPalette, type NodeCtx, type Palette } from "./mapStyle";

const pal: Palette = { served: "#0a0", cut: "#a60", untouched: "#888", focus: "#00f", edge: "#666", edgeDim: "#ddd", label: "#000", background: "#fff" };
const base: NodeCtx = { status: null, focused: false, blastDepth: null, blastActive: false, symbols: 4, hovered: false, zoomRatio: 1 };

describe("nodeStyle", () => {
  test("served is filled with a plain label", () => {
    const s = nodeStyle("src/a.ts", { ...base, status: "served" }, pal);
    expect(s).toMatchObject({ type: "circle", color: pal.served, label: "src/a.ts" });
  });
  test("cut is a ring whose label says cut", () => {
    const s = nodeStyle("src/a.ts", { ...base, status: "cut" }, pal);
    expect(s.type).toBe("border");
    expect(s.color).toBe(pal.background);
    expect(s.borderColor).toBe(pal.cut);
    expect(s.label).toBe("src/a.ts · cut");
  });
  test("untouched is small, grey and unlabelled unless hovered or zoomed in", () => {
    const s = nodeStyle("src/a.ts", { ...base, status: "untouched" }, pal);
    expect(s.color).toBe(pal.untouched);
    expect(s.label).toBeNull();
    expect(s.size).toBeLessThan(nodeStyle("src/a.ts", { ...base, status: "served" }, pal).size);
    expect(nodeStyle("src/a.ts", { ...base, status: "untouched", hovered: true }, pal).label).toBe("src/a.ts");
    expect(nodeStyle("src/a.ts", { ...base, status: "untouched", zoomRatio: 0.3 }, pal).label).toBe("src/a.ts");
  });
  test("no retrieval means neutral nodes with labels", () => {
    expect(nodeStyle("src/a.ts", base, pal)).toMatchObject({ type: "circle", label: "src/a.ts" });
  });
  test("focus adds a ring on top of any status and raises zIndex", () => {
    const s = nodeStyle("src/a.ts", { ...base, status: "served", focused: true }, pal);
    expect(s.type).toBe("border");
    expect(s.borderColor).toBe(pal.focus);
    expect(s.color).toBe(pal.served);
    expect(s.zIndex).toBeGreaterThan(nodeStyle("src/a.ts", { ...base, status: "served" }, pal).zIndex);
  });
  test("blast mode dims everything outside the radius and labels depth", () => {
    const inside = nodeStyle("src/a.ts", { ...base, status: "served", blastActive: true, blastDepth: 2 }, pal);
    expect(inside.label).toBe("src/a.ts · depth 2");
    expect(inside.type).toBe("border");
    const outside = nodeStyle("src/b.ts", { ...base, status: "served", blastActive: true, blastDepth: null }, pal);
    expect(outside.color).toBe(pal.untouched);
    expect(outside.label).toBeNull();
  });
  test("nodeSize grows with symbols and caps", () => {
    expect(nodeSize(0)).toBe(4);
    expect(nodeSize(100)).toBe(14);
    expect(nodeSize(9)).toBeCloseTo(8.5);
  });
});

describe("edgeStyle", () => {
  test("edges of the focused file are full, others dim; blast mode hides edges outside the radius", () => {
    expect(edgeStyle({ touchesFocused: true, blastActive: false, touchesBlast: false }, pal)).toMatchObject({ color: pal.edge, hidden: false });
    expect(edgeStyle({ touchesFocused: false, blastActive: false, touchesBlast: false }, pal)).toMatchObject({ color: pal.edgeDim, hidden: false });
    expect(edgeStyle({ touchesFocused: false, blastActive: true, touchesBlast: false }, pal).hidden).toBe(true);
    expect(edgeStyle({ touchesFocused: false, blastActive: true, touchesBlast: true }, pal).hidden).toBe(false);
  });
});

describe("readPalette", () => {
  test("falls back to defaults when the variables are unset", () => {
    const el = document.createElement("div");
    document.body.appendChild(el);
    const p = readPalette(el);
    expect(p.served).toMatch(/^#|^oklch|^rgb/);
    expect(p.background).toBeTruthy();
  });
});
```

`ui/src/lib/mapSummary.test.ts`:
```ts
import { describe, expect, test } from "bun:test";
import { summaryLabel } from "./mapSummary";

describe("summaryLabel", () => {
  test("with a retrieval", () => {
    expect(summaryLabel(486, { id: 17, served: 30, cut: 25 }, 2)).toBe("Map of 486 files. Retrieval 17: 30 served, 25 cut, 431 untouched. 2 boundaries.");
  });
  test("without a retrieval, singular boundary", () => {
    expect(summaryLabel(10, null, 1)).toBe("Map of 10 files. No retrieval selected. 1 boundary.");
  });
  test("zero boundaries", () => {
    expect(summaryLabel(0, null, 0)).toBe("Map of 0 files. No retrieval selected. 0 boundaries.");
  });
});
```
(`served + cut` counts files, not items: the caller passes file-level counts from the joined rows.)

- [ ] **Step 4: Run to see them fail.**

- [ ] **Step 5: Implement**

`ui/src/lib/graph.ts`:
```ts
import Graph from "graphology";
import { circular } from "graphology-layout";
import forceAtlas2 from "graphology-layout-forceatlas2";
import noverlap from "graphology-layout-noverlap";
import type { GraphPayload } from "@/api/types";

export type Positions = Record<string, { x: number; y: number }>;

export function buildGraph(payload: GraphPayload): Graph {
  const g = new Graph({ type: "undirected", multi: false, allowSelfLoops: false });
  for (const n of payload.nodes) g.addNode(n.path, { symbols: n.symbols, lang: n.lang, x: 0, y: 0, size: 1 });
  for (const e of payload.edges) {
    const a = payload.nodes[e.src]?.path, b = payload.nodes[e.dst]?.path;
    if (!a || !b || a === b) continue;
    const existing = g.edge(a, b);
    if (existing) {
      g.updateEdgeAttribute(existing, "weight", (w) => (w as number) + e.weight);
      g.updateEdgeAttribute(existing, "names", (n) => Math.max(n as number, e.names));
    } else {
      g.addEdge(a, b, { weight: e.weight, names: e.names });
    }
  }
  return g;
}

const ITERATIONS = 300;

/** Circular seed (or the previous positions for files that still exist), ForceAtlas2, then
 *  a no-overlap pass. Deterministic: no randomness anywhere. */
export function layoutGraph(graph: Graph, seed?: Positions): Positions {
  const g = graph.copy();
  circular.assign(g, { scale: 100 });
  if (seed) {
    let cx = 0, cy = 0, n = 0;
    g.forEachNode((node) => {
      const s = seed[node];
      if (s) { g.mergeNodeAttributes(node, { x: s.x, y: s.y }); cx += s.x; cy += s.y; n += 1; }
    });
    if (n > 0) {
      // Newcomers start at the centroid of their seeded neighbours, else at the seed centroid.
      g.forEachNode((node) => {
        if (seed[node]) return;
        let nx = 0, ny = 0, k = 0;
        g.forEachNeighbor(node, (nb) => { const s = seed[nb]; if (s) { nx += s.x; ny += s.y; k += 1; } });
        g.mergeNodeAttributes(node, k > 0 ? { x: nx / k, y: ny / k } : { x: cx / n, y: cy / n });
      });
    }
  }
  if (g.order > 1) {
    forceAtlas2.assign(g, { iterations: ITERATIONS, settings: { ...forceAtlas2.inferSettings(g), gravity: 1, scalingRatio: 10 } });
    noverlap.assign(g, { maxIterations: 50, settings: { margin: 4 } });
  }
  const out: Positions = {};
  g.forEachNode((node, attrs) => { out[node] = { x: attrs.x, y: attrs.y }; });
  return out;
}

export function applyPositions(graph: Graph, positions: Positions): void {
  graph.forEachNode((node) => {
    const p = positions[node] ?? { x: 0, y: 0 };
    graph.mergeNodeAttributes(node, { x: p.x, y: p.y });
  });
}

export const layoutCacheKey = (indexVersion: string) => `singularrag.layout.${indexVersion}`;

export function loadLayout(indexVersion: string): Positions | null {
  try {
    const raw = localStorage.getItem(layoutCacheKey(indexVersion));
    if (!raw) return null;
    const parsed = JSON.parse(raw);
    return parsed && typeof parsed === "object" ? (parsed as Positions) : null;
  } catch { return null; }
}

export function saveLayout(indexVersion: string, positions: Positions): void {
  try { localStorage.setItem(layoutCacheKey(indexVersion), JSON.stringify(positions)); } catch {}
}
```
If `forceAtlas2.inferSettings` turns out non-deterministic across runs (it is not; it reads the order only), the determinism test would fail loudly.

`ui/src/lib/hull.ts`:
```ts
export type Pt = { x: number; y: number };

const cross = (o: Pt, a: Pt, b: Pt) => (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);

/** Andrew's monotone chain; counter-clockwise; collinear points dropped. */
export function convexHull(points: Pt[]): Pt[] {
  const pts = [...points].sort((a, b) => a.x - b.x || a.y - b.y);
  if (pts.length < 3) return pts;
  const lower: Pt[] = [];
  for (const p of pts) { while (lower.length >= 2 && cross(lower[lower.length - 2], lower[lower.length - 1], p) <= 0) lower.pop(); lower.push(p); }
  const upper: Pt[] = [];
  for (const p of [...pts].reverse()) { while (upper.length >= 2 && cross(upper[upper.length - 2], upper[upper.length - 1], p) <= 0) upper.pop(); upper.push(p); }
  upper.pop(); lower.pop();
  return [...lower, ...upper];
}

export function padHull(points: Pt[], padding: number): Pt[] {
  if (points.length === 0) return [];
  if (points.length === 1) {
    const c = points[0];
    return Array.from({ length: 12 }, (_, i) => { const t = (i / 12) * Math.PI * 2; return { x: c.x + Math.cos(t) * padding, y: c.y + Math.sin(t) * padding }; });
  }
  if (points.length === 2) {
    const [a, b] = points;
    const out: Pt[] = [];
    const ang = Math.atan2(b.y - a.y, b.x - a.x);
    for (let i = 0; i <= 7; i++) { const t = ang + Math.PI / 2 + (i / 7) * Math.PI; out.push({ x: a.x + Math.cos(t) * padding, y: a.y + Math.sin(t) * padding }); }
    for (let i = 0; i <= 7; i++) { const t = ang - Math.PI / 2 + (i / 7) * Math.PI; out.push({ x: b.x + Math.cos(t) * padding, y: b.y + Math.sin(t) * padding }); }
    return out;
  }
  const cx = points.reduce((s, p) => s + p.x, 0) / points.length;
  const cy = points.reduce((s, p) => s + p.y, 0) / points.length;
  return points.map((p) => {
    const dx = p.x - cx, dy = p.y - cy, d = Math.hypot(dx, dy) || 1;
    return { x: p.x + (dx / d) * padding, y: p.y + (dy / d) * padding };
  });
}
```

`ui/src/lib/mapStyle.ts`:
```ts
import type { ItemStatus } from "./status";

export type Palette = { served: string; cut: string; untouched: string; focus: string; edge: string; edgeDim: string; label: string; background: string };
export type NodeCtx = { status: ItemStatus | null; focused: boolean; blastDepth: number | null; blastActive: boolean; symbols: number; hovered: boolean; zoomRatio: number };
export type NodeStyle = { size: number; color: string; borderColor?: string; type: "circle" | "border"; label: string | null; zIndex: number };
export type EdgeCtx = { touchesFocused: boolean; blastActive: boolean; touchesBlast: boolean };

const DEFAULTS: Palette = { served: "#047857", cut: "#b45309", untouched: "#9ca3af", focus: "#2563eb", edge: "#6b7280", edgeDim: "#e5e7eb", label: "#111827", background: "#ffffff" };

export const nodeSize = (symbols: number) => Math.min(4 + Math.sqrt(Math.max(0, symbols)) * 1.5, 14);

/** Labels for untouched nodes appear only when hovered or zoomed in past ratio 0.5. */
export function nodeStyle(path: string, ctx: NodeCtx, pal: Palette): NodeStyle {
  const base = nodeSize(ctx.symbols);
  let s: NodeStyle;
  if (ctx.blastActive && ctx.blastDepth === null) {
    s = { size: Math.max(2, base * 0.5), color: pal.untouched, type: "circle", label: null, zIndex: 0 };
  } else if (ctx.blastActive) {
    const ring = Math.max(1, 4 - ctx.blastDepth!);
    s = { size: base + ring, color: pal.background, borderColor: ctx.status === "served" ? pal.served : ctx.status === "cut" ? pal.cut : pal.focus, type: "border", label: `${path} · depth ${ctx.blastDepth}`, zIndex: 2 };
  } else if (ctx.status === "served") {
    s = { size: base, color: pal.served, type: "circle", label: path, zIndex: 2 };
  } else if (ctx.status === "cut") {
    s = { size: base, color: pal.background, borderColor: pal.cut, type: "border", label: `${path} · cut`, zIndex: 2 };
  } else if (ctx.status === "untouched") {
    const show = ctx.hovered || ctx.zoomRatio < 0.5;
    s = { size: Math.max(2, base * 0.6), color: pal.untouched, type: "circle", label: show ? path : null, zIndex: 0 };
  } else {
    s = { size: base, color: pal.edge, type: "circle", label: path, zIndex: 1 };
  }
  if (ctx.focused) {
    s = { ...s, type: "border", borderColor: pal.focus, size: s.size + 3, zIndex: 3, label: s.label ?? path };
  }
  return s;
}

export function edgeStyle(ctx: EdgeCtx, pal: Palette): { color: string; size: number; hidden: boolean } {
  if (ctx.blastActive) return { color: ctx.touchesBlast ? pal.edge : pal.edgeDim, size: ctx.touchesBlast ? 1.5 : 0.5, hidden: !ctx.touchesBlast };
  return ctx.touchesFocused ? { color: pal.edge, size: 1.5, hidden: false } : { color: pal.edgeDim, size: 0.5, hidden: false };
}

export function readPalette(el: HTMLElement): Palette {
  const cs = getComputedStyle(el);
  const v = (name: string, fallback: string) => { const x = cs.getPropertyValue(name).trim(); return x || fallback; };
  return {
    served: v("--map-served", DEFAULTS.served), cut: v("--map-cut", DEFAULTS.cut), untouched: v("--map-untouched", DEFAULTS.untouched),
    focus: v("--map-focus", DEFAULTS.focus), edge: v("--map-edge", DEFAULTS.edge), edgeDim: v("--map-edge-dim", DEFAULTS.edgeDim),
    label: v("--foreground", DEFAULTS.label), background: v("--background", DEFAULTS.background),
  };
}
```
Add to `ui/src/index.css` under `:root` (light) and the dark `:root` block:
```css
  --map-served: #047857; --map-cut: #b45309; --map-untouched: #9ca3af; --map-focus: #2563eb; --map-edge: #6b7280; --map-edge-dim: #e5e7eb;
```
and in dark: `--map-served: #34d399; --map-cut: #fbbf24; --map-untouched: #6b7280; --map-focus: #60a5fa; --map-edge: #9ca3af; --map-edge-dim: #374151;` (labels use `--foreground`, ≥ 4.5:1 on `--background` in both themes; node marks ≥ 3:1).

`ui/src/lib/mapSummary.ts`:
```ts
export function summaryLabel(files: number, retrieval: { id: number; served: number; cut: number } | null, boundaries: number): string {
  const b = `${boundaries} ${boundaries === 1 ? "boundary" : "boundaries"}`;
  if (!retrieval) return `Map of ${files} files. No retrieval selected. ${b}.`;
  const untouched = Math.max(0, files - retrieval.served - retrieval.cut);
  return `Map of ${files} files. Retrieval ${retrieval.id}: ${retrieval.served} served, ${retrieval.cut} cut, ${untouched} untouched. ${b}.`;
}
```

- [ ] **Step 6: Tests green, typecheck, commit**

Run from `ui/`: `bun test && bun run typecheck && bun run build`
```bash
git add -A
git commit -m "feat(ui): graph model, layout cache, hulls, map style and summary label"
```

---

### Task 4: Boundary editing and the panel twins

**Files:**
- Modify: `ui/src/lib/mapEdits.ts`, `ui/src/lib/mapEdits.test.ts`, `ui/src/components/DetailPanel.tsx`, `ui/src/App.tsx`, `ui/src/App.test.tsx`
- Test: `mapEdits.test.ts` additions; `App.test.tsx` additions (boundary add/remove through the mocked API; the panel buttons)

**Interfaces:**
- Produces (mapEdits.ts): `boundariesOf(c: MapConfig, path: string): string[]` (names, config order); `boundaryNames(c: MapConfig): string[]`; `addToBoundary(c: MapConfig, name: string, path: string): MapConfig` (trims `name`; no-op on empty name or duplicate path; creates the boundary when absent, appended); `removeFromBoundary(c: MapConfig, name: string, path: string): MapConfig` (drops the boundary when its last path goes).
- Produces (DetailPanel props, added): `blast: BlastResult | null`, `blastLoading: boolean`, `onToggleBlast: (path: string, symbol: string) => void`, `expandedPath: string | null`, `onToggleExpand: (path: string) => void`, `onAddBoundary: (name: string, path: string) => void`, `onRemoveBoundary: (name: string, path: string) => void`. The panel renders: a "Boundaries" line (chips + add control) for any row; `Show symbols`/`Hide symbols` for a file row; `Show blast radius`/`Hide blast radius` for a symbol row, and when `blast` matches the row, a `Blast radius` heading with an ordered list `<path> (depth N, via <name>)` and, if truncated, the line `Stopped at the depth cap` / `Stopped at 200 files`.
- App (this task) holds `blast`, `blastLoading`, `expandedPath` state and the two boundary saves; the map itself arrives in Task 5/6.

- [ ] **Step 1: Failing tests**

`mapEdits.test.ts` additions:
```ts
import { addToBoundary, boundariesOf, boundaryNames, removeFromBoundary } from "./mapEdits";
const base: MapConfig = { pin: [], exclude: [], note: [], boundary: [{ name: "auth", paths: ["src/a.ts"] }], deny: { extra_patterns: [] } };

describe("boundaries", () => {
  test("add creates or extends, trims, ignores empty and duplicates", () => {
    expect(addToBoundary(base, " http ", "src/h.ts").boundary).toEqual([{ name: "auth", paths: ["src/a.ts"] }, { name: "http", paths: ["src/h.ts"] }]);
    expect(addToBoundary(base, "auth", "src/b.ts").boundary[0].paths).toEqual(["src/a.ts", "src/b.ts"]);
    expect(addToBoundary(base, "auth", "src/a.ts")).toEqual(base);
    expect(addToBoundary(base, "   ", "src/a.ts")).toEqual(base);
  });
  test("remove drops the boundary when empty and is a no-op otherwise", () => {
    expect(removeFromBoundary(base, "auth", "src/a.ts").boundary).toEqual([]);
    const two = addToBoundary(base, "auth", "src/b.ts");
    expect(removeFromBoundary(two, "auth", "src/a.ts").boundary).toEqual([{ name: "auth", paths: ["src/b.ts"] }]);
    expect(removeFromBoundary(base, "nope", "src/a.ts")).toEqual(base);
  });
  test("boundariesOf and boundaryNames", () => {
    expect(boundariesOf(base, "src/a.ts")).toEqual(["auth"]);
    expect(boundariesOf(base, "src/z.ts")).toEqual([]);
    expect(boundaryNames(addToBoundary(base, "http", "src/h.ts"))).toEqual(["auth", "http"]);
  });
});
```

`App.test.tsx` additions (the file's mocked API persists `mapState` across PUTs; follow its existing patterns for selecting the retrieval and focusing a row; blast requests go to `/api/blast?...` — add to the fetch mock: `if (url.includes("/api/blast?")) return json({ root: { path: "src/auth/session.ts", symbol: "createSession" }, files: [{ path: "src/http/middleware.ts", depth: 1, via: "createSession" }], truncated: null });` and `if (url.endsWith("/api/graph")) return json({ index_version: "abc123", nodes: tree.map((f) => ({ path: f.path, symbols: f.symbols.length, lang: f.lang })), edges: [] });`):
```ts
test("a file can be added to a new boundary and removed again", async () => {
  const user = userEvent.setup();
  render(<App />);
  await screen.findByText("src/auth/session.ts");
  await user.click(screen.getAllByRole("button", { name: /Actions for src\/auth\/session.ts/ })[0]);
  const input = await screen.findByLabelText("Add to boundary");
  await user.type(input, "auth");
  await user.click(screen.getByRole("button", { name: "Add" }));
  await waitFor(() => expect(saved?.boundary).toEqual([{ name: "auth", paths: ["src/auth/session.ts"] }]));
  await user.click(await screen.findByRole("button", { name: "Remove from auth" }));
  await waitFor(() => expect(saved?.boundary).toEqual([]));
});

test("a symbol row can show its blast radius in the panel", async () => {
  const user = userEvent.setup();
  render(<App />);
  await user.click(await screen.findByRole("button", { name: /repo_map/ }));
  await user.click(await screen.findByRole("button", { name: "Actions for createSession" }));
  await user.click(await screen.findByRole("button", { name: "Show blast radius" }));
  const list = await screen.findByRole("list", { name: "Blast radius" });
  expect(within(list).getByText(/src\/http\/middleware.ts \(depth 1, via createSession\)/)).toBeTruthy();
  expect(screen.getByRole("button", { name: "Hide blast radius" })).toBeTruthy();
});

test("a file row offers Show symbols and toggles to Hide symbols", async () => {
  const user = userEvent.setup();
  render(<App />);
  await screen.findByText("src/auth/session.ts");
  await user.click(screen.getAllByRole("button", { name: /Actions for src\/auth\/session.ts/ })[0]);
  await user.click(await screen.findByRole("button", { name: "Show symbols" }));
  expect(await screen.findByRole("button", { name: "Hide symbols" })).toBeTruthy();
});
```

- [ ] **Step 2: Run to see them fail.**

- [ ] **Step 3: Implement `mapEdits.ts` additions**

```ts
export const boundaryNames = (c: MapConfig) => c.boundary.map((b) => b.name);
export const boundariesOf = (c: MapConfig, path: string) => c.boundary.filter((b) => b.paths.includes(path)).map((b) => b.name);
export function addToBoundary(c: MapConfig, name: string, path: string): MapConfig {
  const n = name.trim();
  if (!n) return c;
  const existing = c.boundary.find((b) => b.name === n);
  if (existing?.paths.includes(path)) return c;
  const boundary = existing
    ? c.boundary.map((b) => (b.name === n ? { ...b, paths: [...b.paths, path] } : b))
    : [...c.boundary, { name: n, paths: [path] }];
  return { ...c, boundary };
}
export function removeFromBoundary(c: MapConfig, name: string, path: string): MapConfig {
  if (!c.boundary.some((b) => b.name === name && b.paths.includes(path))) return c;
  const boundary = c.boundary
    .map((b) => (b.name === name ? { ...b, paths: b.paths.filter((p) => p !== path) } : b))
    .filter((b) => b.paths.length > 0);
  return { ...c, boundary };
}
```

- [ ] **Step 4: Implement the panel**

In `DetailPanel.tsx`, extend the props with the seven additions above and render, after the pin/exclude buttons and before the note:
```tsx
      <div className="flex flex-wrap items-center gap-2 text-sm">
        <span id="boundaries-label">Boundaries</span>
        {boundariesOf(map, row.path).map((name) => (
          <span key={name} className="inline-flex items-center gap-1 rounded border px-2 py-0.5">
            {name}
            <Button type="button" variant="ghost" size="sm" aria-label={`Remove from ${name}`} onClick={() => onRemoveBoundary(name, row.path)}>×</Button>
          </span>
        ))}
        <form className="inline-flex items-center gap-1" onSubmit={(e) => { e.preventDefault(); onAddBoundary(newBoundary, row.path); setNewBoundary(""); }}>
          <label className="sr-only" htmlFor="boundary-input">Add to boundary</label>
          <input id="boundary-input" list="boundary-names" value={newBoundary} onChange={(e) => setNewBoundary(e.target.value)} className="w-32 rounded border bg-background px-2 py-1" placeholder="boundary name" />
          <datalist id="boundary-names">{boundaryNames(map).map((n) => <option key={n} value={n} />)}</datalist>
          <Button type="submit" variant="outline" size="sm">Add</Button>
        </form>
      </div>
      {row.kind === "file" && (
        <Button type="button" variant="outline" aria-pressed={expandedPath === row.path} onClick={() => onToggleExpand(row.path)}>
          {expandedPath === row.path ? "Hide symbols" : "Show symbols"}
        </Button>
      )}
      {row.kind === "symbol" && (
        <Button type="button" variant="outline" aria-pressed={blastShown} onClick={() => onToggleBlast(row.path, row.symbol.symbol.name)} disabled={blastLoading}>
          {blastShown ? "Hide blast radius" : "Show blast radius"}
        </Button>
      )}
      {blastShown && blast && (
        <div>
          <h3 id="blast-heading" className="text-sm font-semibold">Blast radius</h3>
          {blast.files.length === 0 ? <p className="text-sm text-muted-foreground">Nothing references this symbol.</p> : (
            <ol aria-labelledby="blast-heading" className="list-decimal pl-5 text-sm">
              {blast.files.map((f) => <li key={f.path}>{f.path} (depth {f.depth}, via {f.via})</li>)}
            </ol>
          )}
          {blast.truncated && <p className="text-xs text-muted-foreground">{blast.truncated === "depth" ? "Stopped at the depth cap" : "Stopped at 200 files"}</p>}
        </div>
      )}
```
with `const [newBoundary, setNewBoundary] = useState("");` and `const blastShown = row.kind === "symbol" && !!blast && blast.root.path === row.path && blast.root.symbol === row.symbol.symbol.name;`. The `sr-only` label plus `htmlFor` gives the input the accessible name "Add to boundary"; `aria-labelledby="blast-heading"` names the list. (`ol` has role `list` for Testing Library.) If `Button` has no `size="sm"` variant, drop the prop.

- [ ] **Step 5: Wire App**

In `App.tsx`: state `const [blast, setBlast] = useState<BlastResult | null>(null); const [blastLoading, setBlastLoading] = useState(false); const [expandedPath, setExpandedPath] = useState<string | null>(null);`. `toggleBlast(path, symbol)`: if `blast` already matches, `setBlast(null)`; else `setBlastLoading(true); api.blast(path, symbol).then((b) => { setBlast(b); announce(\`Blast radius: ${b.files.length} files to depth ${Math.max(0, ...b.files.map((f) => f.depth))}\`); }).catch(toastError).finally(() => setBlastLoading(false))`. Clear `blast` whenever `focused` changes to a different row (an effect keyed on `focused?.path`/symbol name). `toggleExpand(path)`: `setExpandedPath((p) => (p === path ? null : path))`. Pass `onAddBoundary={(n, p) => save((c) => addToBoundary(c, n, p))}` and `onRemoveBoundary={(n, p) => save((c) => removeFromBoundary(c, n, p))}` plus the new props to `DetailPanel`.

- [ ] **Step 6: Tests green, typecheck, commit**

Run from `ui/`: `bun test && bun run typecheck && bun run build`
```bash
git add -A
git commit -m "feat(ui): boundary editing, blast list and symbol expansion in the detail panel"
```

---
### Task 5: `MapView`: Sigma mount, reducers, hull layer, expansion, blast paint

**Files:**
- Create: `ui/src/components/MapView.tsx`, `ui/src/components/MapView.test.tsx`, `ui/sigma-mock.ts`
- Modify: `ui/bunfig.toml` (`preload = ["./happydom.ts", "./sigma-mock.ts"]`), `ui/src/lib/mapStyle.ts` (+ `fileStatusOf`)
- Test: `MapView.test.tsx`

**Interfaces:**
- Consumes: `graph.ts`, `hull.ts`, `mapStyle.ts`, `join.ts` (`FileRow`, `SymbolRow`), `types.ts` (`GraphPayload`, `BlastResult`, `Boundary`).
- Produces: `MapView` props: `{ payload: GraphPayload | null; rows: FileRow[]; hasRetrieval: boolean; focusedPath: string | null; focusedSymbol: string | null; expandedPath: string | null; blast: BlastResult | null; boundaries: Boundary[]; ariaLabel: string; onSelectNode: (path: string, symbol?: string) => void; onToggleExpand: (path: string) => void; onSwitchToTable: () => void; onLayoutReady: (indexVersion: string) => void }`. Also `mapStyle.fileStatusOf(row: FileRow, hasRetrieval: boolean): ItemStatus | null` (served if any served symbol, else cut if any cut, else untouched; `null` when `!hasRetrieval`) and `mapStyle.satelliteKey(path, name, line) = \`sym:${path}::${name}::${line}\``.
- The test preload `ui/sigma-mock.ts` replaces `sigma` and `@sigma/node-border` for every test file and records instances on `globalThis.__sigma`.

- [ ] **Step 1: The Sigma mock preload**

`ui/sigma-mock.ts`:
```ts
import { mock } from "bun:test";

type Handler = (payload: unknown) => void;
export class FakeSigma {
  static instances: FakeSigma[] = [];
  handlers = new Map<string, Handler[]>();
  settings: Record<string, unknown>;
  killed = false;
  refreshes = 0;
  camera = { state: { x: 0.5, y: 0.5, ratio: 1 }, getState() { return this.state; }, setState(s: Partial<{ x: number; y: number; ratio: number }>) { this.state = { ...this.state, ...s }; } };
  constructor(public graph: any, public container: HTMLElement, settings: Record<string, unknown> = {}) {
    this.settings = settings;
    FakeSigma.instances.push(this);
  }
  on(name: string, h: Handler) { const hs = this.handlers.get(name) ?? []; hs.push(h); this.handlers.set(name, hs); return this; }
  emit(name: string, payload: unknown) { for (const h of this.handlers.get(name) ?? []) h(payload); }
  getCamera() { return this.camera; }
  getGraph() { return this.graph; }
  getNodeDisplayData(node: string) { const a = this.graph.getNodeAttributes(node); return { x: a.x ?? 0, y: a.y ?? 0 }; }
  graphToViewport(p: { x: number; y: number }) { return { x: p.x, y: p.y }; }
  getDimensions() { return { width: 400, height: 300 }; }
  setSetting(k: string, v: unknown) { this.settings[k] = v; }
  refresh() { this.refreshes += 1; }
  kill() { this.killed = true; }
}
(globalThis as any).__sigma = FakeSigma;
mock.module("sigma", () => ({ default: FakeSigma, Sigma: FakeSigma }));
mock.module("@sigma/node-border", () => ({ createNodeBorderProgram: () => class {} }));
```
Add it to `bunfig.toml`'s `preload` after `happydom.ts`.

- [ ] **Step 2: Failing tests**

`ui/src/components/MapView.test.tsx`:
```ts
import { beforeEach, describe, expect, mock, test } from "bun:test";
import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { MapView } from "./MapView";
import type { GraphPayload } from "@/api/types";
import type { FileRow } from "@/lib/join";

const Fake = (globalThis as any).__sigma;
const payload: GraphPayload = {
  index_version: "v9",
  nodes: [{ path: "src/a.ts", symbols: 2, lang: "typescript" }, { path: "src/b.ts", symbols: 1, lang: null }, { path: "src/c.ts", symbols: 5, lang: null }],
  edges: [{ src: 1, dst: 0, weight: 1, names: 1 }],
};
const sym = (name: string, line: number, status: "served" | "cut" | "untouched") => ({ key: `k${line}`, symbol: { id: line, name, kind: "function", line_start: line, line_end: line, signature: name }, status, item: null });
const rows: FileRow[] = [
  { path: "src/a.ts", lang: "typescript", served: 1, cut: 0, expandedByDefault: true, symbols: [sym("x", 1, "served"), sym("y", 2, "untouched")] },
  { path: "src/b.ts", lang: null, served: 0, cut: 1, expandedByDefault: true, symbols: [sym("z", 3, "cut")] },
  { path: "src/c.ts", lang: null, served: 0, cut: 0, expandedByDefault: false, symbols: [] },
];
const props = () => ({
  payload, rows, hasRetrieval: true, focusedPath: null as string | null, focusedSymbol: null as string | null, expandedPath: null as string | null, blast: null, boundaries: [],
  ariaLabel: "Map of 3 files.", onSelectNode: mock(), onToggleExpand: mock(), onSwitchToTable: mock(), onLayoutReady: mock(),
});

beforeEach(() => { Fake.instances = []; localStorage.clear(); });

describe("MapView", () => {
  test("renders the summary label, the switch control before the canvas, and mounts Sigma once", async () => {
    const p = props();
    render(<MapView {...p} />);
    const img = screen.getByRole("img", { name: "Map of 3 files." });
    const btn = screen.getByRole("button", { name: "Switch to table" });
    expect(btn.compareDocumentPosition(img) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(img.getAttribute("tabindex")).toBeNull();
    await act(async () => {});
    expect(Fake.instances).toHaveLength(1);
    expect(p.onLayoutReady).toHaveBeenCalledWith("v9");
    expect(localStorage.getItem("singularrag.layout.v9")).toBeTruthy();
    await userEvent.setup().click(btn);
    expect(p.onSwitchToTable).toHaveBeenCalled();
  });

  test("reducers paint status from the rows: served filled, cut ringed, untouched dim", async () => {
    render(<MapView {...props()} />);
    await act(async () => {});
    const s = Fake.instances[0];
    const reduce = s.settings.nodeReducer as (n: string, d: Record<string, unknown>) => Record<string, unknown>;
    const attrs = (n: string) => s.graph.getNodeAttributes(n);
    expect(reduce("src/a.ts", attrs("src/a.ts"))).toMatchObject({ type: "circle", label: "src/a.ts" });
    expect(reduce("src/b.ts", attrs("src/b.ts"))).toMatchObject({ type: "border", label: "src/b.ts · cut" });
    expect(reduce("src/c.ts", attrs("src/c.ts")).label).toBeNull();
  });

  test("clicking a node selects it; double-click asks to expand; focus pans the camera", async () => {
    const p = props();
    const { rerender } = render(<MapView {...p} />);
    await act(async () => {});
    const s = Fake.instances[0];
    act(() => s.emit("clickNode", { node: "src/b.ts" }));
    expect(p.onSelectNode).toHaveBeenCalledWith("src/b.ts", undefined);
    act(() => s.emit("doubleClickNode", { node: "src/b.ts" }));
    expect(p.onToggleExpand).toHaveBeenCalledWith("src/b.ts");
    s.graph.mergeNodeAttributes("src/c.ts", { x: 0.9, y: 0.1 });
    rerender(<MapView {...p} focusedPath="src/c.ts" />);
    await act(async () => {});
    expect(s.camera.state).toMatchObject({ x: 0.9, y: 0.1 });
    expect((s.settings.nodeReducer as any)("src/c.ts", s.graph.getNodeAttributes("src/c.ts")).type).toBe("border");
  });

  test("expanding a file adds satellite symbol nodes and collapsing removes them", async () => {
    const p = props();
    const { rerender } = render(<MapView {...p} />);
    await act(async () => {});
    const s = Fake.instances[0];
    expect(s.graph.order).toBe(3);
    rerender(<MapView {...p} expandedPath="src/a.ts" />);
    await act(async () => {});
    expect(s.graph.order).toBe(5);
    const reduce = s.settings.nodeReducer as any;
    const key = "sym:src/a.ts::x::1";
    expect(reduce(key, s.graph.getNodeAttributes(key))).toMatchObject({ type: "circle", label: "x" });
    act(() => s.emit("clickNode", { node: key }));
    expect(p.onSelectNode).toHaveBeenCalledWith("src/a.ts", "x");
    rerender(<MapView {...p} expandedPath={null} />);
    await act(async () => {});
    expect(s.graph.order).toBe(3);
  });

  test("a blast result dims everything outside it and labels depth", async () => {
    const p = props();
    const { rerender } = render(<MapView {...p} />);
    await act(async () => {});
    const s = Fake.instances[0];
    rerender(<MapView {...p} blast={{ root: { path: "src/a.ts", symbol: "x" }, files: [{ path: "src/b.ts", depth: 1, via: "x" }], truncated: null }} />);
    await act(async () => {});
    const reduce = s.settings.nodeReducer as any;
    expect(reduce("src/b.ts", s.graph.getNodeAttributes("src/b.ts")).label).toBe("src/b.ts · depth 1");
    expect(reduce("src/c.ts", s.graph.getNodeAttributes("src/c.ts")).label).toBeNull();
    expect(reduce("src/a.ts", s.graph.getNodeAttributes("src/a.ts")).label).toBe("src/a.ts · depth 0");
  });

  test("boundary hulls are drawn for members present in the graph", async () => {
    const p = props();
    render(<MapView {...p} boundaries={[{ name: "auth", paths: ["src/a.ts", "src/b.ts", "src/nope.ts"] }]} />);
    await act(async () => {});
    const s = Fake.instances[0];
    const canvas = screen.getByTestId("hull-layer") as HTMLCanvasElement;
    const calls: string[] = [];
    (canvas as any).getContext = () => new Proxy({}, { get: (_t, k) => (k === "canvas" ? canvas : (...a: unknown[]) => { calls.push(String(k)); return undefined; }) });
    act(() => s.emit("afterRender", {}));
    expect(calls).toContain("fill");
    expect(calls).toContain("fillText");
  });

  test("unmount kills sigma; no payload renders an empty state", async () => {
    const p = props();
    const { unmount } = render(<MapView {...p} />);
    await act(async () => {});
    unmount();
    expect(Fake.instances[0].killed).toBe(true);
    render(<MapView {...p} payload={null} />);
    expect(screen.getByText("Loading the map…")).toBeTruthy();
  });

  test("axe finds nothing", async () => {
    const { container } = render(<MapView {...props()} />);
    await act(async () => {});
    const r = await axe.run(container);
    expect(r.violations).toEqual([]);
  });
});
```
happy-dom has no canvas 2D context, so the hull test stubs `getContext` on the layer canvas before emitting `afterRender`; the component must read `getContext("2d")` at draw time, not at mount, and tolerate `null`.

- [ ] **Step 3: Run to see them fail.**

- [ ] **Step 4: Implement**

`mapStyle.ts` additions:
```ts
import type { FileRow } from "./join";
export function fileStatusOf(row: FileRow, hasRetrieval: boolean): ItemStatus | null {
  if (!hasRetrieval) return null;
  return row.served > 0 ? "served" : row.cut > 0 ? "cut" : "untouched";
}
export const satelliteKey = (path: string, name: string, line: number) => `sym:${path}::${name}::${line}`;
```

`ui/src/components/MapView.tsx`:
```tsx
import { useEffect, useMemo, useRef, useState } from "react";
import Sigma from "sigma";
import { createNodeBorderProgram } from "@sigma/node-border";
import type Graph from "graphology";
import { Button } from "@/components/ui/button";
import type { BlastResult, Boundary, GraphPayload } from "@/api/types";
import type { FileRow } from "@/lib/join";
import { applyPositions, buildGraph, layoutGraph, loadLayout, saveLayout, type Positions } from "@/lib/graph";
import { convexHull, padHull } from "@/lib/hull";
import { edgeStyle, fileStatusOf, nodeStyle, readPalette, satelliteKey, type Palette } from "@/lib/mapStyle";

export type MapViewProps = {
  payload: GraphPayload | null; rows: FileRow[]; hasRetrieval: boolean;
  focusedPath: string | null; focusedSymbol: string | null; expandedPath: string | null;
  blast: BlastResult | null; boundaries: Boundary[]; ariaLabel: string;
  onSelectNode: (path: string, symbol?: string) => void; onToggleExpand: (path: string) => void;
  onSwitchToTable: () => void; onLayoutReady: (indexVersion: string) => void;
};

const HULL_PADDING = 18;
const hue = (name: string) => { let h = 0; for (const ch of name) h = (h * 31 + ch.charCodeAt(0)) % 360; return h; };

/** Latest props in a ref so the reducers (installed once) read current state. */
function useLatest<T>(v: T) { const r = useRef(v); r.current = v; return r; }

export function MapView(props: MapViewProps) {
  const { payload, expandedPath, focusedPath, onLayoutReady, onSelectNode, onToggleExpand, onSwitchToTable, ariaLabel } = props;
  const containerRef = useRef<HTMLDivElement>(null);
  const hullRef = useRef<HTMLCanvasElement>(null);
  const sigmaRef = useRef<Sigma | null>(null);
  const graphRef = useRef<Graph | null>(null);
  const lastPositions = useRef<Positions | null>(null);
  const hovered = useRef<string | null>(null);
  const [palette, setPalette] = useState<Palette | null>(null);
  const latest = useLatest(props);

  const statusByPath = useMemo(() => new Map(props.rows.map((r) => [r.path, fileStatusOf(r, props.hasRetrieval)])), [props.rows, props.hasRetrieval]);
  const blastDepth = useMemo(() => {
    if (!props.blast) return null;
    const m = new Map<string, number>([[props.blast.root.path, 0]]);
    for (const f of props.blast.files) m.set(f.path, f.depth);
    return m;
  }, [props.blast]);
  const latestDerived = useLatest({ statusByPath, blastDepth });

  // Mount once per payload: build, lay out (cached per index version), install reducers.
  useEffect(() => {
    const el = containerRef.current;
    if (!payload || !el) return;
    const graph = buildGraph(payload);
    const cached = loadLayout(payload.index_version);
    const positions = cached ?? layoutGraph(graph, lastPositions.current ?? undefined);
    if (!cached) saveLayout(payload.index_version, positions);
    lastPositions.current = positions;
    applyPositions(graph, positions);
    graphRef.current = graph;
    const pal = readPalette(el);
    setPalette(pal);
    const sigma = new Sigma(graph, el, {
      allowInvalidContainer: true,
      renderLabels: true,
      labelRenderedSizeThreshold: 0,
      labelColor: { color: pal.label },
      defaultNodeType: "circle",
      zIndex: true,
      nodeProgramClasses: {
        border: createNodeBorderProgram({ borders: [{ size: { value: 0.25, mode: "relative" }, color: { attribute: "borderColor" } }, { size: { fill: true }, color: { attribute: "color" } }] }),
      },
      nodeReducer: (node, data) => {
        const p = latest.current;
        const d = latestDerived.current;
        const ratio = sigma.getCamera().getState().ratio;
        const sat = data.symbolOf as string | undefined;
        if (sat) {
          const s = nodeStyle(data.symbolName as string, { status: (data.symbolStatus as never) ?? null, focused: p.focusedPath === sat && p.focusedSymbol === data.symbolName, blastDepth: null, blastActive: false, symbols: 0, hovered: false, zoomRatio: ratio }, pal);
          return { ...data, ...s, size: 3 + (s.type === "border" ? 2 : 0), borderColor: s.borderColor ?? pal.focus, label: s.label ?? (data.symbolName as string) };
        }
        const s = nodeStyle(node, {
          status: d.statusByPath.get(node) ?? null,
          focused: p.focusedPath === node,
          blastDepth: d.blastDepth?.get(node) ?? null,
          blastActive: d.blastDepth !== null,
          symbols: (data.symbols as number) ?? 0,
          hovered: hovered.current === node,
          zoomRatio: ratio,
        }, pal);
        return { ...data, size: s.size, color: s.color, borderColor: s.borderColor ?? pal.focus, type: s.type, label: s.label, zIndex: s.zIndex };
      },
      edgeReducer: (edge, data) => {
        const p = latest.current;
        const d = latestDerived.current;
        const [a, b] = graph.extremities(edge);
        const touchesFocused = p.focusedPath === a || p.focusedPath === b;
        const touchesBlast = !!d.blastDepth && d.blastDepth.has(a) && d.blastDepth.has(b);
        const s = edgeStyle({ touchesFocused, blastActive: d.blastDepth !== null, touchesBlast }, pal);
        return { ...data, color: s.color, size: data.satellite ? 0.5 : s.size, hidden: s.hidden && !data.satellite };
      },
    });
    sigma.on("clickNode", ({ node }) => {
      const a = graph.getNodeAttributes(node);
      if (a.symbolOf) latest.current.onSelectNode(a.symbolOf as string, a.symbolName as string);
      else latest.current.onSelectNode(node, undefined);
    });
    sigma.on("doubleClickNode", ({ node }) => { if (!graph.getNodeAttribute(node, "symbolOf")) latest.current.onToggleExpand(node); });
    sigma.on("enterNode", ({ node }) => { hovered.current = node; sigma.refresh(); });
    sigma.on("leaveNode", () => { hovered.current = null; sigma.refresh(); });
    sigma.on("afterRender", () => drawHulls(sigma, graph, hullRef.current, latest.current.boundaries, pal));
    sigmaRef.current = sigma;
    onLayoutReady(payload.index_version);
    return () => { sigma.kill(); sigmaRef.current = null; graphRef.current = null; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [payload]);

  // Repaint when the inputs the reducers read change.
  useEffect(() => { sigmaRef.current?.refresh(); }, [statusByPath, blastDepth, props.focusedPath, props.focusedSymbol, props.boundaries]);

  // Camera follows the focused file, no easing.
  useEffect(() => {
    const sigma = sigmaRef.current, graph = graphRef.current;
    if (!sigma || !graph || !focusedPath || !graph.hasNode(focusedPath)) return;
    const d = sigma.getNodeDisplayData(focusedPath);
    if (d) sigma.getCamera().setState({ x: d.x, y: d.y });
  }, [focusedPath, payload]);

  // Symbol satellites for the expanded file.
  useEffect(() => {
    const graph = graphRef.current;
    if (!graph) return;
    for (const n of graph.nodes()) if (graph.getNodeAttribute(n, "symbolOf")) graph.dropNode(n);
    const row = expandedPath ? props.rows.find((r) => r.path === expandedPath) : null;
    if (row && graph.hasNode(row.path)) {
      const { x, y } = graph.getNodeAttributes(row.path);
      const n = row.symbols.length, radius = n > 8 ? 9 : 6;
      row.symbols.forEach((s, i) => {
        const t = (i / Math.max(1, n)) * Math.PI * 2;
        const key = satelliteKey(row.path, s.symbol.name, s.symbol.line_start);
        graph.addNode(key, { x: x + Math.cos(t) * radius, y: y + Math.sin(t) * radius, size: 3, symbolOf: row.path, symbolName: s.symbol.name, symbolStatus: props.hasRetrieval ? s.status : null });
        graph.addEdge(row.path, key, { weight: 0, names: 0, satellite: true });
      });
    }
    sigmaRef.current?.refresh();
  }, [expandedPath, props.rows, props.hasRetrieval, payload]);

  if (!payload) return <div className="p-3 text-sm text-muted-foreground">Loading the map…</div>;
  return (
    <div className="relative h-full">
      <Button type="button" variant="outline" className="absolute left-2 top-2 z-10" onClick={onSwitchToTable}>Switch to table</Button>
      <div ref={containerRef} role="img" aria-label={ariaLabel} className="h-full w-full" style={{ background: palette?.background }} />
      <canvas ref={hullRef} data-testid="hull-layer" aria-hidden="true" className="pointer-events-none absolute inset-0 h-full w-full" />
    </div>
  );
}

function drawHulls(sigma: Sigma, graph: Graph, canvas: HTMLCanvasElement | null, boundaries: Boundary[], pal: Palette) {
  if (!canvas) return;
  const { width, height } = sigma.getDimensions();
  if (canvas.width !== width || canvas.height !== height) { canvas.width = width; canvas.height = height; }
  const ctx = canvas.getContext("2d");
  if (!ctx) return;
  ctx.clearRect(0, 0, width, height);
  for (const b of boundaries) {
    const pts = b.paths.filter((p) => graph.hasNode(p)).map((p) => { const a = graph.getNodeAttributes(p); return sigma.graphToViewport({ x: a.x, y: a.y }); });
    if (pts.length === 0) continue;
    const hull = padHull(convexHull(pts), HULL_PADDING);
    const h = hue(b.name);
    ctx.beginPath();
    hull.forEach((p, i) => (i === 0 ? ctx.moveTo(p.x, p.y) : ctx.lineTo(p.x, p.y)));
    ctx.closePath();
    ctx.fillStyle = `hsl(${h} 60% 50% / 0.12)`;
    ctx.strokeStyle = `hsl(${h} 60% 45% / 0.7)`;
    ctx.lineWidth = 1.5;
    ctx.fill();
    ctx.stroke();
    const top = hull.reduce((m, p) => (p.y < m.y ? p : m), hull[0]);
    ctx.fillStyle = pal.label;
    ctx.font = "12px system-ui, sans-serif";
    ctx.fillText(b.name, top.x, top.y - 4);
  }
}
```
Notes for the implementer: `readPalette` runs after mount so CSS variables resolve; if Sigma 3's `labelColor` setting shape differs (`{ color }` vs `{ attribute }`), follow the installed type definitions and keep the label colour from the palette. `sigma.getNodeDisplayData` returns framed coordinates, which is what `camera.setState` expects. If `graph.extremities` is not on the installed graphology type, use `graph.source(edge)`/`graph.target(edge)`. Satellite node labels are the symbol name; the blast test expects depth 0 on the root file, which `blastDepth` seeds.

- [ ] **Step 5: Tests green, typecheck, build, commit**

Run from `ui/`: `bun test && bun run typecheck && bun run build`
```bash
git add -A
git commit -m "feat(ui): MapView with status reducers, boundary hulls, symbol satellites, blast paint"
```

---

### Task 6: The view toggle and App integration

**Files:**
- Create: `ui/src/components/ViewToggle.tsx`, `ui/src/components/ViewToggle.test.tsx`
- Modify: `ui/src/App.tsx`, `ui/src/App.test.tsx`
- Test: `ViewToggle.test.tsx`; `App.test.tsx` additions

**Interfaces:**
- Produces: `ViewToggle` props `{ value: "tree" | "map"; onChange: (v: "tree" | "map") => void }`: `role="radiogroup"` labelled `View`, two `role="radio"` buttons `Tree` and `Map` with `aria-checked`, Left/Right/Up/Down arrows move and select, roving tabindex; `export const VIEW_KEY = "singularrag.view"`; `loadView(): "tree" | "map"`, `saveView(v)` (localStorage, try/catch).
- App: state `view`; `graph: GraphPayload | null` fetched on first load and whenever `status.index_version` changes (compare to the loaded payload's `index_version`, not on every change event); `main` renders `RepoTree` or `MapView`; `onSelectNode(path, symbol?)` resolves the row from `rows` and calls `setFocused`; `onSwitchToTable` sets the view to tree and focuses the treegrid element; `onLayoutReady` announces `Map layout ready` once per index version (a ref of announced versions); `ariaLabel` from `summaryLabel(graph.nodes.length, detail ? { id: detail.id, served: files with served>0, cut: files with cut>0 and served===0 } : null, map.boundary.length)`.

- [ ] **Step 1: Failing tests**

`ViewToggle.test.tsx`:
```ts
import { beforeEach, describe, expect, mock, test } from "bun:test";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { ViewToggle, loadView, saveView } from "./ViewToggle";

beforeEach(() => localStorage.clear());

describe("ViewToggle", () => {
  test("is a labelled radiogroup with the current value checked", () => {
    render(<ViewToggle value="tree" onChange={mock()} />);
    const group = screen.getByRole("radiogroup", { name: "View" });
    const radios = screen.getAllByRole("radio");
    expect(radios.map((r) => r.textContent)).toEqual(["Tree", "Map"]);
    expect(radios[0].getAttribute("aria-checked")).toBe("true");
    expect(radios[1].getAttribute("tabindex")).toBe("-1");
    expect(group).toBeTruthy();
  });
  test("arrow keys move selection and click selects", async () => {
    const onChange = mock();
    const user = userEvent.setup();
    render(<ViewToggle value="tree" onChange={onChange} />);
    screen.getByRole("radio", { name: "Tree" }).focus();
    await user.keyboard("{ArrowRight}");
    expect(onChange).toHaveBeenCalledWith("map");
    await user.click(screen.getByRole("radio", { name: "Map" }));
    expect(onChange).toHaveBeenLastCalledWith("map");
  });
  test("persists and reloads, defaulting to tree", () => {
    expect(loadView()).toBe("tree");
    saveView("map");
    expect(loadView()).toBe("map");
    localStorage.setItem("singularrag.view", "bogus");
    expect(loadView()).toBe("tree");
  });
  test("axe clean", async () => {
    const { container } = render(<ViewToggle value="map" onChange={mock()} />);
    expect((await axe.run(container)).violations).toEqual([]);
  });
});
```

`App.test.tsx` additions (the sigma mock preload applies here too; `/api/graph` is already mocked from Task 4):
```ts
test("the map view shows the summary label, switch-to-table returns focus to the tree, and the choice persists", async () => {
  const user = userEvent.setup();
  render(<App />);
  await user.click(await screen.findByRole("button", { name: /repo_map/ }));
  await user.click(screen.getByRole("radio", { name: "Map" }));
  const img = await screen.findByRole("img", { name: /Map of 1 files\. Retrieval 7: 1 served, 0 cut, 0 untouched\. 0 boundaries\./ });
  expect(img).toBeTruthy();
  expect(localStorage.getItem("singularrag.view")).toBe("map");
  await waitFor(() => expect(screen.getByRole("log", { name: "Announcements" }).textContent).toContain("Map layout ready"));
  await user.click(screen.getByRole("button", { name: "Switch to table" }));
  expect(await screen.findByRole("treegrid")).toBeTruthy();
  expect(document.activeElement?.getAttribute("role")).toBe("treegrid");
  expect(localStorage.getItem("singularrag.view")).toBe("tree");
});

test("selecting a node on the map focuses the same row in the panel", async () => {
  const user = userEvent.setup();
  render(<App />);
  await user.click(await screen.findByRole("button", { name: /repo_map/ }));
  await user.click(screen.getByRole("radio", { name: "Map" }));
  await screen.findByRole("img");
  const s = (globalThis as any).__sigma.instances.at(-1);
  await act(async () => { s.emit("clickNode", { node: "src/auth/session.ts" }); });
  expect(screen.getByRole("heading", { name: "src/auth/session.ts" })).toBeTruthy();
});

test("the map view is axe clean", async () => {
  const user = userEvent.setup();
  const { container } = render(<App />);
  await screen.findByText("src/auth/session.ts");
  await user.click(screen.getByRole("radio", { name: "Map" }));
  await screen.findByRole("img");
  expect((await axe.run(container)).violations).toEqual([]);
});
```
(`act` from `@testing-library/react`.)

- [ ] **Step 2: Run to see them fail.**

- [ ] **Step 3: Implement `ViewToggle.tsx`**

```tsx
import { useRef } from "react";

export type View = "tree" | "map";
export const VIEW_KEY = "singularrag.view";
export function loadView(): View {
  try { const v = localStorage.getItem(VIEW_KEY); return v === "map" ? "map" : "tree"; } catch { return "tree"; }
}
export function saveView(v: View) { try { localStorage.setItem(VIEW_KEY, v); } catch {} }

const OPTIONS: { value: View; label: string }[] = [{ value: "tree", label: "Tree" }, { value: "map", label: "Map" }];

export function ViewToggle({ value, onChange }: { value: View; onChange: (v: View) => void }) {
  const refs = useRef<(HTMLButtonElement | null)[]>([]);
  const move = (from: number, delta: number) => {
    const to = (from + delta + OPTIONS.length) % OPTIONS.length;
    onChange(OPTIONS[to].value);
    refs.current[to]?.focus();
  };
  return (
    <div role="radiogroup" aria-label="View" className="inline-flex rounded border">
      {OPTIONS.map((o, i) => (
        <button key={o.value} ref={(el) => { refs.current[i] = el; }} type="button" role="radio" aria-checked={value === o.value} tabIndex={value === o.value ? 0 : -1}
          className="px-2 py-1 text-sm aria-checked:bg-accent focus-visible:outline-2 focus-visible:outline-ring"
          onClick={() => onChange(o.value)}
          onKeyDown={(e) => {
            if (e.key === "ArrowRight" || e.key === "ArrowDown") { e.preventDefault(); move(i, 1); }
            if (e.key === "ArrowLeft" || e.key === "ArrowUp") { e.preventDefault(); move(i, -1); }
          }}>
          {o.label}
        </button>
      ))}
    </div>
  );
}
```

- [ ] **Step 4: Wire App**

In `App.tsx`:
- `const [view, setView] = useState<View>(loadView); const changeView = (v: View) => { setView(v); saveView(v); };`
- `const [graph, setGraph] = useState<GraphPayload | null>(null);` In `load()`, after `setStatus(s)`: `if (!graphRef.current || graphRef.current !== s.index_version) { const g = await api.graph(); graphRef.current = g.index_version; setGraph(g); }` with `const graphRef = useRef<string | null>(null)`.
- `const announcedLayouts = useRef(new Set<string>()); const onLayoutReady = useCallback((v: string) => { if (!announcedLayouts.current.has(v)) { announcedLayouts.current.add(v); announce("Map layout ready"); } }, [announce]);`
- `const onSelectNode = useCallback((path: string, symbol?: string) => { const file = rows.find((r) => r.path === path); if (!file) return; if (symbol) { const s = file.symbols.find((x) => x.symbol.name === symbol); if (s) { setFocused({ kind: "symbol", path, file, symbol: s }); return; } } setFocused({ kind: "file", path, file }); }, [rows]);`
- `const switchToTable = () => { changeView("tree"); requestAnimationFrame(() => (document.querySelector('[role="treegrid"]') as HTMLElement | null)?.focus()); };` (use `setTimeout(…, 0)` instead of rAF: the earlier session found rAF never fires in a hidden tab; `setTimeout` is safe under happy-dom too.)
- `const fileCounts = useMemo(() => ({ served: rows.filter((r) => r.served > 0).length, cut: rows.filter((r) => r.served === 0 && r.cut > 0).length }), [rows]);` and `const mapLabel = summaryLabel(graph?.nodes.length ?? 0, detail ? { id: detail.id, ...fileCounts } : null, map.boundary.length);`
- Header: `<ViewToggle value={view} onChange={changeView} />` before the filter label.
- Main: `{view === "tree" ? <RepoTree … /> : <MapView payload={graph} rows={rows} hasRetrieval={detail !== null} focusedPath={focused?.path ?? null} focusedSymbol={focused?.kind === "symbol" ? focused.symbol.symbol.name : null} expandedPath={expandedPath} blast={blast} boundaries={map.boundary} ariaLabel={mapLabel} onSelectNode={onSelectNode} onToggleExpand={toggleExpand} onSwitchToTable={switchToTable} onLayoutReady={onLayoutReady} />}`. Give `main` `className="relative overflow-hidden"` when the map is shown (Sigma needs a sized, non-scrolling container) and keep `overflow-auto` for the tree.

- [ ] **Step 5: Tests green, typecheck, build, commit**

Run from `ui/`: `bun test && bun run typecheck && bun run build`
```bash
git add -A
git commit -m "feat(ui): Tree/Map toggle, map wired to selection, blast and boundaries"
```

---

### Task 7: Docs, embedded build, and the browser check

**Files:**
- Modify: `README.md`, `docs/superpowers/specs/2026-09-20-singularrag-serve-design.md` (§10/§12 "deferred to 3b" lines get a pointer to the map spec), `ui/dist` (rebuilt; not committed), `crates/singularrag/tests/serve.rs` (no change expected)

- [ ] **Step 1: README**

In the "See what the agent was given" section, after the paragraph that starts "Opens", add:
````markdown
Switch the main pane to **Map** for a Sigma.js projection of the same data: every indexed file is a node and every reference an edge, laid out once per index version; the selected retrieval paints files as served (filled), cut (ringed) or untouched (dimmed), a symbol's blast radius can be shown from the detail panel, and boundaries — named groups of files kept in `map.toml` — are drawn as labelled regions and edited from the panel. The treegrid stays the canonical view: the map has a summary label and a "Switch to table" control, and everything the map shows is also in the panel.
````

- [ ] **Step 2: Serve spec pointer**

In the serve spec, where §10 lists "The Sigma.js map and boundaries (3b)" and §12 asks about `toml_edit`, append `— see docs/superpowers/specs/2026-09-20-singularrag-map-design.md` to the map line only. (The other deferrals stay open.)

- [ ] **Step 3: Build and check in a browser**

```bash
cd ui && bun run build && cd .. && cargo build --release
./target/release/singularrag serve --no-open --repo <a hono checkout with an index>
```
Open the printed URL. Check by hand and record the outcome in the report: the Map radio shows the graph within a second on hono; a retrieval selection paints served/cut/untouched with the label suffixes; clicking a node updates the panel; "Show symbols" adds satellites and "Hide symbols" removes them; "Show blast radius" on a symbol dims the map and lists files in the panel; adding a file to a new boundary draws a labelled hull; "Switch to table" lands focus on the treegrid; nothing animates. Note anything that differs from the spec as a concern.

- [ ] **Step 4: Full gates and commit**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test --workspace` and, in `ui/`, `bun test && bun run typecheck && bun run build`.
```bash
git add -A
git commit -m "docs: map view in the README, serve spec pointer to the map spec"
```

---

## Self-review notes

- Spec §3 routes: graph DTO shape and rules (Task 2 `graph()` + tests), blast definition and caps (Task 1), 404/400 (Task 2), boundary name validation (Task 2). §4 browser model: `graph.ts`, `hull.ts`, `mapEdits.ts` additions, `mapSummary.ts` (Tasks 3, 4). §5 view: toggle (Task 6), rendering/status encoding/focus/hulls/satellites/blast/no animation/theme (Task 5 with `mapStyle.ts` in Task 3 and CSS variables), selection sync (Task 6). §6 accessibility: `role="img"` label, switch button before the canvas, canvas not tabbable, live-region copy, panel twins, axe (Tasks 4, 5, 6). §7 boundary editing (Task 4). §8 testing: every listed test exists in Tasks 1–6; Playwright stays deferred (Task 7 browser check). §9 dependencies (Task 3). §10 decisions honoured (layout on the main thread with cache, convex hulls, caps).
- Placeholder scan: none. Every code step carries its code; Task 7's browser check is a manual step with explicit expectations.
- Type consistency: `GraphPayload`/`BlastResult` (Task 3) match the Rust DTOs (Task 2: `root.{path,symbol}`, `files[].{path,depth,via}`, `truncated: null|"depth"|"files"`); `fileStatusOf`/`satelliteKey` (Task 5) live in `mapStyle.ts` created in Task 3; `MapView` props (Task 5) match App's wiring (Task 6); DetailPanel props (Task 4) match App; `summaryLabel` signature (Task 3) matches Task 6's call; `ViewToggle` exports match the test.
- Known API uncertainties with in-task fallbacks: Sigma 3 `labelColor` setting shape and `graph.extremities` typing (Task 5 notes); `Button size="sm"` (Task 4 note); `graphology-types` devDependency (Task 3 Step 1); happy-dom canvas (`getContext` stubbed in the hull test, tolerated `null` in code).
- Spec amendments made while writing this plan (in the spec file): blast implemented as a breadth-first walk rather than a single CTE; `truncated` is `null | "depth" | "files"`; `@sigma/node-border` added to §9.
