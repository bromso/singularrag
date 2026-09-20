# singularrag serve — plan 3b design (the map projection and boundaries)

Date: 2026-09-20. Status: approved in brainstorming; awaiting written-spec review.
Parent spec: `docs/superpowers/specs/2026-09-19-singularrag-design.md` (§10 view 3, §11 accessibility bind this plan). Serve spec: `docs/superpowers/specs/2026-09-20-singularrag-serve-design.md` (plan 3a; its §3 API, §5 security, §6 keyboard model and single write path bind this plan). This work lives on `map-v0`, branched from `main` (5978e88, plans 1 to 4 merged).

## 1. Goal

Add the third view the parent spec promised: a Sigma.js projection of the same rows the treegrid shows, over the same file-level reference graph the ranking uses. With a retrieval selected, the map shows what was served, what was cut and what was untouched as a shape over the repository; a symbol's blast radius can be shown; and boundaries, named regions of files, are drawn from `map.toml` and edited from the detail panel. The treegrid stays the canonical, accessible view; the map is a projection with a summary label and a way back to the table.

## 2. Scope

In: `GET /api/graph`, `GET /api/blast`, the Tree/Map toggle, the map view, status encoding, boundary hulls, symbol expansion, blast-radius display, selection sync with the treegrid and panel, boundary editing in the panel through `PUT /api/map`, tests.

Out (own bounded cleanup after this plan): `toml_edit` comment preservation; the join fallback to `symbol_id` with a "moved since this retrieval" state; `/api/tree` payload size and the full refetch on change; the freshness model's two writers. Also out: any ranking effect from boundaries; editing boundaries by dragging; drag-to-move nodes; edge labels; clustering; a query box; multi-repo; Playwright.

## 3. Routes

Both are read-only, live under the existing `/api` router with the token, Host and `no-store` layers, and return JSON errors like the others. No new write route: boundaries go through `PUT /api/map`.

### `GET /api/graph`

Calls core `graph::build_graph(store, &config, &[], &HashSet::new())`: no query terms, empty FTS set, so weights are the plain ranking weights. Response:

```json
{ "index_version": "b94252",
  "nodes": [ { "path": "src/router.ts", "symbols": 7, "lang": "typescript" } ],
  "edges": [ { "src": 3, "dst": 12, "weight": 1.5, "names": 2 } ] }
```

- `nodes` are the graph's files in path order; excluded files are absent (the builder omits them). `symbols` is the count of symbols in the file; `lang` may be null.
- `edges`: one per ordered `(src, dst)` pair, `weight` the sum of the multigraph's weights for that pair, `names` the number of distinct symbol names behind it. Self-edges are dropped.
- `index_version` is the store's current index version so the browser can key its layout cache.

### `GET /api/blast?path=<repo-relative>&symbol=<name>`

A breadth-first walk over `refs`, `symbols` and `files` in a new core module (`blast::blast_radius(store, config, path, symbol, max_depth, max_files)`), depth cap 3, file cap 200. *(Amended at plan time from "one recursive CTE": a walk with two prepared statements per level gives the same result and is testable level by level.)*

- Depth 0: the file `path`, which must define `symbol` (else 404 `{ "error": "symbol not found" }`).
- Depth n+1: every non-excluded, non-skipped file with a `refs` row whose `name` equals any symbol defined in a depth-n file, and that has not been reached at a shallower depth. A file is reported once at its minimum depth, with `via` the name that reached it first (lowest depth, then alphabetical).
- Stops at depth 3 or when 200 files are reached; `truncated` says which cap hit, if any.

Response: `{ "root": { "path", "symbol" }, "files": [ { "path", "depth", "via" } ], "truncated": null }`, files ordered by depth then path; `truncated` is `null`, `"depth"` or `"files"`. Missing `path` or `symbol` is 400 `{ "error": "path and symbol are required" }`. Depth 1 answers "what references this symbol"; deeper levels are what the parent spec calls blast radius.

### `PUT /api/map` validation addition

Boundary names must be non-empty and unique (422 with `field: "boundary[i].name"`). Existing path checks stay.

## 4. Data model in the browser

`ui/src/lib/graph.ts`, pure over the `/api/graph` payload:

- `buildGraph(payload) -> graphology.Graph`: nodes keyed by path with `symbols`, `lang`; undirected edges with `weight` and `names` (a pair present in both directions merges into one edge with summed weight).
- `layout(graph, seed?) -> Record<path, {x, y}>`: circular seed (or `seed` positions for paths that still exist, new files placed at the centroid of their neighbours or the origin), then ForceAtlas2 with settings inferred from the graph plus a fixed iteration count (300), then a no-overlap pass. Deterministic for a given graph and seed.
- `cacheKey(index_version)`, `loadLayout`, `saveLayout`: localStorage per viewer, wrapped in try/catch, keyed by index version. On a version change the previous positions seed the new layout, so the picture drifts rather than jumps.

`ui/src/lib/hull.ts`: `convexHull(points) -> points`, `padHull(points, padding)`, degenerate cases (1 point → circle, 2 → pill) return a polygon approximation.

`ui/src/lib/mapEdits.ts` gains `addToBoundary(cfg, name, path)` (creates the boundary when absent, no duplicate paths, trims the name) and `removeFromBoundary(cfg, name, path)` (drops the boundary when its last path goes).

`ui/src/lib/mapSummary.ts`: `summaryLabel(counts, retrieval, boundaries) -> string`, the text used for the `role="img"` label.

## 5. The map view

**Toggle.** The top bar gains a radiogroup "View" with "Tree" and "Map", before the filter. Tree is the default. The choice persists in localStorage. Focused row, selected retrieval and map document live in App state and are shared by both views.

**Rendering, `ui/src/components/MapView.tsx`.** Sigma 3 (`sigma` 3.0.3) mounted in an effect on a container div, destroyed on unmount. Painting is by node and edge reducers over three inputs: the selected retrieval's items joined to paths (the existing join), the focused row, and the boundaries.

- Status encoding, never colour alone: served is a filled node with a solid label; cut is a ring (hollow node with a border) whose label carries ` · cut`; untouched is a small grey node with no label until hovered or zoomed in. Node size scales with symbol count, capped.
- The focused file gets a highlight ring; the camera pans to it with no easing. Its edges are drawn full, others dimmed.
- Boundaries are drawn on a canvas layer above Sigma using Sigma's camera to map graph coordinates to the viewport: for each boundary the padded convex hull of its members' positions, stroked and lightly filled with a stable hue derived from the name, with the name as a text label at the hull's top. Redrawn on every camera or data change.
- Symbol expansion: at most one file expanded at a time, via a "Show symbols" button in the detail panel or a double-click on the node. Its symbols appear as small satellite nodes on a circle around it (local positions, no relayout), each labelled with its name and carrying the item status for the selected retrieval. Edges stay file-level. Collapsing removes them.
- Blast radius: the detail panel gains "Show blast radius" for a symbol row. The map dims everything except the returned files, encodes depth as ring thickness with the depth in the label (`· depth 2`), and the panel lists the files by depth with their `via` names. The panel list is the canonical form; the map echoes it. It clears when the focus changes.
- Nothing animates: no layout animation, no camera easing, no hover transitions. `prefers-reduced-motion` has nothing extra to disable.
- Theme: node, ring, edge and hull colours are read from CSS custom properties at mount and on theme change so dark mode keeps contrast ≥ 4.5:1 for labels and ≥ 3:1 for node marks.

**Selection sync.** Clicking a node calls the same `setFocused` the treegrid uses, so the detail panel is shared. Treegrid focus highlights and centres the node. Clicking empty canvas keeps the focus.

## 6. Accessibility

- The map container is `role="img"` with `aria-label` from `summaryLabel`: "Map of 486 files. Retrieval 17: 30 served, 25 cut, 431 untouched. 2 boundaries." (without a retrieval: "Map of 486 files. No retrieval selected. 2 boundaries."). Updated on every change.
- Immediately before the container in focus order: a "Switch to table" button that sets the toggle to Tree and moves focus to the treegrid. The canvas is not in the tab order.
- The live region announces "Map layout ready" once per index version and "Blast radius: N files to depth D" when shown.
- Every map-only affordance has a panel equivalent: expansion, blast radius and boundary membership are all in the detail panel, so the treegrid + panel path is complete without the canvas.
- axe runs over the map container and the panel in component tests. Jonas's VoiceOver pass covers the toggle, the summary label and the switch control.

## 7. Boundary editing

In the detail panel, for a focused file or a symbol's file, a "Boundaries" line: the names the file belongs to as chips with a "Remove from <name>" button each, and an "Add to boundary" control: a text input with a `<datalist>` of existing names, so typing a new name creates one, and an "Add" button. Both go through the existing save queue as edit functions, with the existing toast and 409 reload. The hull appears when the map document is re-read after the save; no graph refetch.

## 8. Testing

- Rust unit: graph projection (aggregation, self-edges dropped, excluded files omitted, path order, `names` count); blast CTE on a fixture (three-file chain gives depths 1 and 2; a cycle terminates; both caps; unknown symbol; excluded file omitted); boundary name validation.
- Rust integration (`tests/serve.rs`): both routes 401 without the token, 200 with; `blast` 404 for an unknown symbol; the graph's node count equals the tree's non-skipped, non-excluded file count.
- UI (bun test, happy-dom): `graph.ts` (build, merge of both-direction edges, deterministic layout for a fixed seed, cache round trip, seeding from old positions), `hull.ts`, `mapEdits.ts` additions, `summaryLabel`, the toggle and its persistence, boundary editing through the mocked API (add creates, remove drops when empty, 409 path), `MapView` with `sigma` replaced by a module mock so the reducers, the aria label, the switch button and the hull layer's inputs are exercised without WebGL; axe on the map container and panel.
- Deferred: Playwright. The live canvas is checked by hand in the browser.

## 9. Dependencies

`sigma` 3.0.3 with `@sigma/node-border` 3.0.0 (the ring rendering for cut and focused nodes), `graphology` 0.26, `graphology-layout-forceatlas2` 0.10, `graphology-layout-noverlap` 0.4, `graphology-layout` 0.6. No React wrapper. No new Rust crates.

## 10. Decisions recorded

- Whole-repo map, status painted over it, rather than a retrieval neighbourhood: a stable picture the eye can learn.
- Layout on the main thread, cached per index version; a worker only if a repo proves the need.
- Convex hulls: cheap and legible at ten to fifty files per boundary.
- Blast depth cap 3, file cap 200: deeper approaches the whole repo on a framework and stops meaning "blast".
- The blast list, symbol expansion and boundary membership live in the panel, so the screen-reader path is complete without the canvas.
