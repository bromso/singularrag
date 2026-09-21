# singularrag serve cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the four items deferred from plan 3a: comment-preserving `map.toml` writes, a retrieval-to-symbol join that survives a reindex and names moved symbols, a tree/graph refetch only when the index changes, and one freshness writer with one payload shape.

**Architecture:** Four independent, bounded changes on `cleanup-v0` (from `map-v0`, b2ad303). Core: `save_atomic` edits a `toml_edit` document instead of serialising. UI: `joinRetrieval` gains a three-tier match with a `moved` flag; `App.load()` gates the tree and graph fetches on `index_version`. Serve: `Freshness` drops `drain`, the SSE `freshness` event carries a full `StatusDto`, the watcher broadcasts exactly twice per refresh, the live region de-duplicates.

**Tech Stack:** Rust (`toml_edit` 0.25 with `serde`), TypeScript/React, bun test.

**Spec:** `docs/superpowers/specs/2026-09-20-singularrag-serve-cleanup-design.md` (binding). Serve spec `docs/superpowers/specs/2026-09-20-singularrag-serve-design.md` §3, §5, §6.

## Global Constraints

- Branch `cleanup-v0` from `map-v0` (b2ad303). Commit after every task. Gates at the end of every task: `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace`; in `ui/`: `bun test`, `bun run typecheck`, `bun run build`.
- No new routes, no new write path; `PUT /api/map` remains the only write (serve spec §5).
- `MAP_HEADER` becomes exactly: `# Managed by singularrag serve. Hand edits and comments between sections are kept; a comment inside an entry the page rewrites is not.\n`, written only when the file did not exist.
- `save_atomic` replaces only the top-level items `pin`, `exclude`, `note`, `boundary`, `deny`; everything else in the document is untouched.
- Join tiers in order: `symbol_id` (and the tree symbol's path and name equal the item's), then `(path, name, line_start)`, then `(path, name)` with the nearest `line_start` → `moved: true`. Each item joins at most one symbol; each symbol takes at most one item, by rank.
- Copy: treegrid moved marker text `moved`; panel line `Moved since this retrieval (was line N)`.
- `App.load()` fetches status, retrievals, skipped and the map document on every change; tree and graph only when `index_version` differs from the last loaded, tracked in one ref.
- `Freshness` fields: `stale_count`, `lock_timeout`, `foreign_indexing`, `indexing`, `indexed_at_ms`. No `drain` anywhere on the serve side or in the UI `Status` type.
- The `freshness` SSE event carries `StatusDto` (same shape as `GET /api/status`). Exactly two events per refresh attempt: `indexing: true` before, the result with `indexing: false` after.
- The live region announces `Index <freshness text>` only when the text differs from the last announced freshness text.

---

## File structure

```
Cargo.toml, crates/singularrag-core/Cargo.toml   # toml_edit 0.25 (serde)
crates/singularrag-core/src/config.rs             # save_atomic via toml_edit; MAP_HEADER; tests
ui/src/lib/join.ts (+ .test.ts)                   # three-tier join, moved flag
ui/src/components/RepoTree.tsx, DetailPanel.tsx   # moved marker and panel line
ui/src/App.tsx (+ App.test.tsx)                   # refetch gate; freshness announce de-dup
crates/singularrag/src/serve/state.rs             # Freshness without drain; ServerEvent::Freshness(StatusDto)
crates/singularrag/src/serve/queries.rs           # StatusDto without drain (+ Clone, Default)
crates/singularrag/src/serve/watcher.rs           # apply_started/apply broadcast StatusDto; two events
crates/singularrag/src/serve/events.rs            # serialise StatusDto; test
crates/singularrag/tests/serve.rs                 # status keys without drain
ui/src/api/types.ts, FreshnessBadge.tsx (+ .test), client.test.ts   # Status without drain
docs/superpowers/specs/2026-09-20-singularrag-serve-design.md      # §3 status row, freshness paragraph
```

---

### Task 1: Comment-preserving `save_atomic`

**Files:**
- Modify: `Cargo.toml` (workspace dependency), `crates/singularrag-core/Cargo.toml`, `crates/singularrag-core/src/config.rs`
- Test: `config.rs` unit tests

**Interfaces:**
- `MapConfig::save_atomic(&self, root: &Path) -> Result<()>` signature unchanged. `MAP_HEADER` text changes per Global Constraints.

- [ ] **Step 1: Dependency**

Root `Cargo.toml` `[workspace.dependencies]`: `toml_edit = { version = "0.25", features = ["serde"] }`. Core `Cargo.toml` `[dependencies]`: `toml_edit = { workspace = true }`. Run `cargo build -p singularrag-core` to lock it.

- [ ] **Step 2: Failing tests** (append to `config.rs` `mod tests`; `cfg()` exists)

```rust
    #[test]
    fn save_atomic_keeps_comments_between_sections_and_unknown_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(MAP_FILE);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "# mine, at the top\npin = []\n\n# between sections\n[[exclude]]\npath = \"src/legacy/\"\n\ncustom = \"keep me\"\n\n[deny]\nextra_patterns = []\n# at the end\n").unwrap();
        let c = cfg("[[pin]]\npath = \"src/a.ts\"\n[[exclude]]\npath = \"src/legacy/\"\n");
        c.save_atomic(dir.path()).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.starts_with("# mine, at the top"), "{written}");
        assert!(!written.contains("Managed by singularrag"), "the header is only written to a new file");
        assert!(written.contains("# between sections"), "{written}");
        assert!(written.contains("custom = \"keep me\""), "{written}");
        assert!(written.contains("# at the end"), "{written}");
        assert!(written.contains("[[pin]]"), "{written}");
        assert_eq!(MapConfig::load(dir.path()).unwrap(), c);
        assert!(!dir.path().join(".singularrag/map.toml.tmp").exists());
    }

    #[test]
    fn save_atomic_writes_the_header_only_to_a_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let c = cfg("[[pin]]\npath = \"src/a.ts\"\n");
        c.save_atomic(dir.path()).unwrap();
        let written = std::fs::read_to_string(dir.path().join(MAP_FILE)).unwrap();
        assert!(written.starts_with(MAP_HEADER), "{written}");
        assert_eq!(MapConfig::load(dir.path()).unwrap(), c);
        // A second save must not duplicate the header.
        c.save_atomic(dir.path()).unwrap();
        let again = std::fs::read_to_string(dir.path().join(MAP_FILE)).unwrap();
        assert_eq!(again.matches("Managed by singularrag").count(), 1, "{again}");
    }

    #[test]
    fn save_atomic_replaces_every_owned_section_even_when_emptied() {
        let dir = tempfile::tempdir().unwrap();
        cfg("[[pin]]\npath = \"src/a.ts\"\n[[note]]\npath = \"src/a.ts\"\ntext = \"n\"\n").save_atomic(dir.path()).unwrap();
        MapConfig::default().save_atomic(dir.path()).unwrap();
        let loaded = MapConfig::load(dir.path()).unwrap();
        assert!(loaded.pin.is_empty() && loaded.note.is_empty(), "{loaded:?}");
    }
```
Update the existing `save_atomic_round_trips_with_header_and_leaves_no_temp_file` if it asserts the old header text (it uses the constant, so it should pass unchanged).

- [ ] **Step 3: Run to see them fail.**

- [ ] **Step 4: Implement**

Replace `MAP_HEADER` and `save_atomic`:
```rust
pub const MAP_HEADER: &str = "# Managed by singularrag serve. Hand edits and comments between sections are kept; a comment inside an entry the page rewrites is not.\n";

const OWNED_KEYS: [&str; 5] = ["pin", "exclude", "note", "boundary", "deny"];

    /// Edit the existing document in place so comments, blank lines, key order and keys
    /// the page does not own survive; only the owned sections are replaced. Written via a
    /// temp file and rename so a reader never sees a torn file.
    pub fn save_atomic(&self, root: &Path) -> Result<()> {
        let path = root.join(MAP_FILE);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let existing = match std::fs::read_to_string(&path) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        let mut doc: toml_edit::DocumentMut = existing
            .as_deref()
            .unwrap_or(MAP_HEADER)
            .parse()
            .map_err(|e: toml_edit::TomlError| Error::Config(e.to_string()))?;
        let fresh = toml_edit::ser::to_document(self).map_err(|e| Error::Config(e.to_string()))?;
        for key in OWNED_KEYS {
            match fresh.get(key) {
                Some(item) => {
                    doc[key] = item.clone();
                }
                None => {
                    doc.remove(key);
                }
            }
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, doc.to_string())?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }
```
If `toml_edit::ser::to_document` is not exported under that path in 0.25, use `toml_edit::ser::to_string_pretty(self)` and parse the result into a `DocumentMut`; the rest is identical. If assigning an array-of-tables item over an existing plain array (`pin = []`) leaves a stray inline value, `doc.remove(key)` before `doc[key] = …`.

- [ ] **Step 5: Gates and commit**

`cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test --workspace`
```bash
git add -A
git commit -m "feat(core): map.toml writes keep comments and unknown keys (toml_edit)"
```

---

### Task 2: The join chain with a moved state

**Files:**
- Modify: `ui/src/lib/join.ts`, `ui/src/lib/join.test.ts`, `ui/src/components/RepoTree.tsx`, `ui/src/components/DetailPanel.tsx`
- Test: `join.test.ts`; one assertion each in `RepoTree.test.tsx` and `App.test.tsx`

**Interfaces:**
- `SymbolRow` gains `moved: boolean`. `joinRetrieval(files, items)` signature unchanged.

- [ ] **Step 1: Failing tests** (append to `join.test.ts`; `files`, `r` exist there)

```ts
describe("join tiers", () => {
  const item = (o: Partial<Item>): Item => ({ rank: 1, symbol_id: 0, path: "src/a.ts", name: "f", line_start: 1, score: 0.5, served: true, reasons: r, ...o });

  test("tier 1: symbol_id wins even when the line differs, if path and name agree", () => {
    const rows = joinRetrieval(files, [item({ symbol_id: 2, name: "g", line_start: 99 })]);
    const g = rows[0].symbols.find((s) => s.symbol.name === "g")!;
    expect(g.status).toBe("served");
    expect(g.moved).toBe(false);
  });

  test("a reused id that points at a different symbol does not match", () => {
    const rows = joinRetrieval(files, [item({ symbol_id: 3, path: "src/a.ts", name: "f", line_start: 1 })]);
    const f = rows.find((x) => x.path === "src/a.ts")!.symbols.find((s) => s.symbol.name === "f")!;
    expect(f.status).toBe("served");
    expect(f.moved).toBe(false);
    const h = rows.find((x) => x.path === "src/b.ts")!.symbols[0];
    expect(h.status).toBe("untouched");
  });

  test("tier 2: a new id with the same path, name and line joins without moved", () => {
    const rows = joinRetrieval(files, [item({ symbol_id: 777, name: "f", line_start: 1 })]);
    const f = rows[0].symbols.find((s) => s.symbol.name === "f")!;
    expect(f.status).toBe("served");
    expect(f.moved).toBe(false);
  });

  test("tier 3: same path and name at another line joins as moved, nearest line wins", () => {
    const twoF: TreeFile[] = [{ path: "src/a.ts", lang: null, skipped_reason: null, symbols: [
      { id: 10, name: "f", kind: "function", line_start: 3, line_end: 4, signature: "f" },
      { id: 11, name: "f", kind: "function", line_start: 40, line_end: 41, signature: "f" },
    ] }];
    const rows = joinRetrieval(twoF, [item({ symbol_id: 777, name: "f", line_start: 8 })]);
    const [near, far] = rows[0].symbols;
    expect(near.status).toBe("served");
    expect(near.moved).toBe(true);
    expect(far.status).toBe("untouched");
  });

  test("a name that no longer exists in the file stays unjoined", () => {
    const rows = joinRetrieval(files, [item({ symbol_id: 777, name: "gone", line_start: 1 })]);
    expect(rows[0].symbols.every((s) => s.status === "untouched")).toBe(true);
    expect(rows[0].served).toBe(0);
  });

  test("each symbol takes at most one item, by rank", () => {
    const rows = joinRetrieval(files, [item({ rank: 2, symbol_id: 777, name: "f", line_start: 1, served: false }), item({ rank: 1, symbol_id: 1, name: "f", line_start: 1, served: true })]);
    const f = rows[0].symbols.find((s) => s.symbol.name === "f")!;
    expect(f.item?.rank).toBe(1);
    expect(f.status).toBe("served");
  });
});
```
Add `import type { TreeFile } from "@/api/types"` if missing (it is imported already).

`RepoTree.test.tsx`: in the existing test that renders rows with items, give one symbol row `moved: true` and assert `screen.getByText("moved")` is in that row. `App.test.tsx`: extend the mocked `/api/retrievals/7` item to `line_start: 9` (the tree symbol is at line 3) and assert the panel, after focusing `createSession`, shows `Moved since this retrieval (was line 9)`; keep the other tests' expectations by leaving `symbol_id: 1` so tier 1 still matches — then this test must instead use `symbol_id: 999` to force tier 3. Write it as its own test that overrides the retrieval detail through a `let detailItems` variable in the mock.

- [ ] **Step 2: Run to see them fail.**

- [ ] **Step 3: Implement `join.ts`**

```ts
import type { Item, TreeFile, TreeSymbol } from "@/api/types";
import type { ItemStatus } from "./status";

export type SymbolRow = { key: string; symbol: TreeSymbol; status: ItemStatus; item: Item | null; moved: boolean };
export type FileRow = { path: string; lang: string | null; served: number; cut: number; expandedByDefault: boolean; symbols: SymbolRow[] };

type Hit = { item: Item; moved: boolean };

/** Match items to symbols by id, then by exact position, then by name at another line (moved). */
function assign(files: TreeFile[], items: Item[]): Map<number, Hit> {
  const byId = new Map<number, { path: string; symbol: TreeSymbol }>();
  const byPathName = new Map<string, TreeSymbol[]>();
  for (const f of files) {
    for (const s of f.symbols) {
      byId.set(s.id, { path: f.path, symbol: s });
      const k = `${f.path}::${s.name}`;
      const list = byPathName.get(k);
      if (list) list.push(s); else byPathName.set(k, [s]);
    }
  }
  const hits = new Map<number, Hit>();
  const taken = new Set<number>();
  for (const it of [...items].sort((a, b) => a.rank - b.rank)) {
    const idHit = byId.get(it.symbol_id);
    if (idHit && idHit.path === it.path && idHit.symbol.name === it.name && !taken.has(idHit.symbol.id)) {
      hits.set(idHit.symbol.id, { item: it, moved: false });
      taken.add(idHit.symbol.id);
      continue;
    }
    const candidates = (byPathName.get(`${it.path}::${it.name}`) ?? []).filter((s) => !taken.has(s.id));
    if (candidates.length === 0) continue;
    const exact = candidates.find((s) => s.line_start === it.line_start);
    const chosen = exact ?? candidates.reduce((best, s) => (Math.abs(s.line_start - it.line_start) < Math.abs(best.line_start - it.line_start) ? s : best));
    hits.set(chosen.id, { item: it, moved: !exact });
    taken.add(chosen.id);
  }
  return hits;
}

export function joinRetrieval(files: TreeFile[], items: Item[] | null): FileRow[] {
  const hits = items ? assign(files, items) : new Map<number, Hit>();
  const rows = files.map<FileRow>((f) => {
    let served = 0, cut = 0;
    const symbols = f.symbols.map<SymbolRow>((s) => {
      const hit = hits.get(s.id) ?? null;
      const item = hit?.item ?? null;
      const status: ItemStatus = item ? (item.served ? "served" : "cut") : "untouched";
      if (item) item.served ? served++ : cut++;
      return { key: `${f.path}::${s.name}::${s.line_start}`, symbol: s, status, item, moved: hit?.moved ?? false };
    });
    return { path: f.path, lang: f.lang, served, cut, expandedByDefault: served + cut > 0, symbols };
  });
  if (items) rows.sort((a, b) => Number(b.expandedByDefault) - Number(a.expandedByDefault) || a.path.localeCompare(b.path));
  return rows;
}
```

`RepoTree.tsx` `SymbolRowView`: after `<StatusMark status={symbol.status} />` add `{symbol.moved && <span className="text-xs text-muted-foreground">moved</span>}`. `DetailPanel.tsx`: inside the `item ? (…)` branch, after the reasons list, add `{row.kind === "symbol" && row.symbol.moved && <p className="text-sm text-muted-foreground">Moved since this retrieval (was line {item.line_start})</p>}`. Any test fixture that builds `SymbolRow` literals (e.g. `MapView.test.tsx`'s `sym()` helper) gains `moved: false`.

- [ ] **Step 4: Gates and commit**

From `ui/`: `bun test && bun run typecheck && bun run build`
```bash
git add -A
git commit -m "feat(ui): join items by id, position, then name with a moved state"
```

---

### Task 3: The refetch gate

**Files:**
- Modify: `ui/src/App.tsx`, `ui/src/App.test.tsx`

**Interfaces:** none new. `graphRef` becomes `indexRef` and gates both `tree` and `graph`.

- [ ] **Step 1: Failing tests** (`App.test.tsx`; the mock's `status` object and `changeHandler` exist; add `let statusVersion = "abc123"` reset in `beforeEach`, and make the status branch return `json({ ...status, index_version: statusVersion })`; add a helper `const calls = (frag: string) => (globalThis.fetch as any).mock.calls.filter((c: any[]) => String(c[0]).includes(frag)).length`)

```ts
test("a change event with the same index version refetches retrievals and the map but not the tree or graph", async () => {
  render(<App />);
  await screen.findByText("src/auth/session.ts");
  const tree0 = calls("/api/tree"), graph0 = calls("/api/graph"), rs0 = calls("/api/retrievals?");
  await act(async () => { changeHandler!({ data: JSON.stringify({ max_retrieval_id: 7 }) }); });
  await waitFor(() => expect(calls("/api/retrievals?")).toBe(rs0 + 1));
  expect(calls("/api/tree")).toBe(tree0);
  expect(calls("/api/graph")).toBe(graph0);
});

test("a change event with a new index version refetches the tree and the graph", async () => {
  render(<App />);
  await screen.findByText("src/auth/session.ts");
  const tree0 = calls("/api/tree"), graph0 = calls("/api/graph");
  statusVersion = "def456";
  await act(async () => { changeHandler!({ data: JSON.stringify({ max_retrieval_id: 7 }) }); });
  await waitFor(() => expect(calls("/api/tree")).toBe(tree0 + 1));
  expect(calls("/api/graph")).toBe(graph0 + 1);
});
```

- [ ] **Step 2: Run to see them fail** (the first fails: the tree is refetched today).

- [ ] **Step 3: Implement**

In `App.tsx`, rename `graphRef` to `indexRef` and rewrite `load`:
```ts
    const load = async () => {
      const [s, rs, sk, m] = await Promise.all([api.status(), api.retrievals(), api.skipped(), api.map()]);
      setStatus(s); setRetrievals(rs); setSkipped(sk); applyMapDoc(m);
      // The tree and the graph are functions of the index: refetch them only when it changed.
      if (indexRef.current !== s.index_version) {
        const [t, g] = await Promise.all([api.tree(), api.graph()]);
        indexRef.current = s.index_version;
        setTree(t); setGraph(g);
      }
      …announcements unchanged…
    };
```
The exclusion-change path from plan 3b (a `PUT /api/map` whose `exclude` changed) keeps refetching the graph directly; it must not reset `indexRef`.

- [ ] **Step 4: Gates and commit**

From `ui/`: `bun test && bun run typecheck && bun run build`
```bash
git add -A
git commit -m "feat(ui): refetch the tree and graph only when the index version changes"
```

---

### Task 4: One freshness writer

**Files:**
- Modify: `crates/singularrag/src/serve/state.rs`, `queries.rs`, `watcher.rs`, `events.rs`, `crates/singularrag/tests/serve.rs`, `ui/src/api/types.ts`, `ui/src/components/FreshnessBadge.tsx`, `FreshnessBadge.test.tsx`, `ui/src/api/client.test.ts`, `ui/src/App.tsx`, `ui/src/App.test.tsx`, `docs/superpowers/specs/2026-09-20-singularrag-serve-design.md`
- Test: watcher and events unit tests; `tests/serve.rs`; `App.test.tsx`

**Interfaces:**
- `state::Freshness { stale_count, lock_timeout, foreign_indexing, indexing, indexed_at_ms }`; `DrainStatsJson` removed. `ServerEvent::Freshness(queries::StatusDto)`; `StatusDto` derives `Clone` and `Default` (and `FileCounts` `Default`), loses `drain`.
- `watcher::apply_started(state: &AppState)` and `watcher::apply(state: &AppState, stats: IndexStats)` both broadcast a `StatusDto` built by `queries::status` under the read lock; `refresh_once` calls `apply_started` before each attempt and `apply` (or `apply_failed`, which clears `indexing` and broadcasts) after.
- UI `Status` loses `drain`; `subscribe`'s freshness callback receives a `Status`; App announces only on text change.

- [ ] **Step 1: Failing Rust tests**

`watcher.rs`: replace `refresh_once_updates_freshness_and_broadcasts` with:
```rust
    #[tokio::test]
    async fn refresh_once_broadcasts_exactly_two_status_events() {
        let (_dir, state, handle) = setup();
        let mut rx = state.events.subscribe();
        refresh_once(&state, &handle).await;
        let f = state.freshness.read().unwrap().clone();
        assert_eq!(f.stale_count, 0);
        assert!(!f.foreign_indexing);
        assert!(!f.indexing);
        assert!(f.indexed_at_ms.is_some());
        let mut seen = Vec::new();
        while let Ok(crate::serve::state::ServerEvent::Freshness(s)) = rx.try_recv() {
            seen.push(s);
        }
        assert_eq!(seen.len(), 2, "one 'started', one result: {seen:?}");
        assert!(seen[0].indexing);
        assert!(!seen[1].indexing);
        assert!(!seen[1].index_version.is_empty(), "the payload is a full status");
        assert_eq!(seen[1].files.indexed, 4);
    }
```
`events.rs`: change `events_serialise_with_their_names` to build `ServerEvent::Freshness(StatusDto { stale_count: 2, git_head: Some("abc".into()), ..Default::default() })` and assert the debug string contains `freshness`, `git_head` and `abc`.
`tests/serve.rs`: remove `"drain"` from the status key list.

- [ ] **Step 2: Failing UI tests**

`App.test.tsx`: extend the `EventSource` stub to capture the `freshness` listener too (`let freshnessHandler: ((e: { data: string }) => void) | null = null;` set when `type === "freshness"`), then:
```ts
test("identical freshness payloads announce once; a change announces again", async () => {
  render(<App />);
  await screen.findByText("src/auth/session.ts");
  const log = () => screen.getByRole("log", { name: "Announcements" }).textContent ?? "";
  const payload = { ...status, indexing: true };
  await act(async () => { freshnessHandler!({ data: JSON.stringify(payload) }); });
  await act(async () => { freshnessHandler!({ data: JSON.stringify(payload) }); });
  expect(log().split("Index indexing").length - 1).toBe(1);
  await act(async () => { freshnessHandler!({ data: JSON.stringify({ ...status, indexing: false }) }); });
  expect(log()).toContain("Index fresh");
  expect(screen.getByLabelText("Index freshness").textContent).toContain("9b1e0d4");
});
```
Remove `drain` from the `status` literal in `App.test.tsx`, from `client.test.ts`'s `mockStatus`, and from `FreshnessBadge.test.tsx` (delete the "ignores drain" test).

- [ ] **Step 3: Run to see them fail.**

- [ ] **Step 4: Implement Rust**

`state.rs`: delete `DrainStatsJson`, its `From`, the `use crate::actor::DrainStats;`, and the `drain` field; `ServerEvent::Freshness(super::queries::StatusDto)`. `queries.rs`: `#[derive(Debug, Clone, Default, Serialize)]` on `StatusDto` and `FileCounts`; remove `drain`. `watcher.rs`:
```rust
fn broadcast_status(state: &AppState) {
    let f = state.freshness.read().map(|g| g.clone()).unwrap_or_default();
    let dto = {
        let Ok(store) = state.read.lock() else { return };
        super::queries::status(&store, &f)
    };
    match dto {
        Ok(dto) => { let _ = state.events.send(ServerEvent::Freshness(dto)); }
        Err(e) => tracing::warn!("status for freshness event failed: {e}"),
    }
}

/// A refresh is starting: the badge may say "indexing" for the seconds it takes (I8).
pub fn apply_started(state: &AppState) {
    if let Ok(mut f) = state.freshness.write() { f.indexing = true; }
    broadcast_status(state);
}

pub fn apply(state: &AppState, stats: IndexStats) {
    if let Ok(mut f) = state.freshness.write() {
        f.stale_count = stats.remaining;
        f.lock_timeout = stats.lock_timeout;
        f.foreign_indexing = stats.lock_timeout;
        f.indexing = false;
        if !stats.lock_timeout { f.indexed_at_ms = Some(now_ms()); }
    }
    broadcast_status(state);
}

fn apply_failed(state: &AppState) {
    if let Ok(mut f) = state.freshness.write() { f.indexing = false; }
    broadcast_status(state);
}

pub async fn refresh_once(state: &AppState, handle: &EngineHandle) {
    for attempt in 0..2 {
        apply_started(state);
        match handle.refresh().await {
            Ok(stats) => {
                let retry = stats.lock_timeout && attempt == 0;
                apply(state, stats);
                if !retry { return; }
                tokio::time::sleep(LOCK_RETRY_AFTER).await;
            }
            Err(e) => { tracing::warn!("refresh failed: {e}"); apply_failed(state); return; }
        }
    }
}
```
Delete `set_indexing`. `events.rs` `to_sse_event`: `ServerEvent::Freshness(s) => Event::default().event("freshness").data(serde_json::to_string(s).unwrap_or_else(|_| "{}".into()))`. Note `queries::status` reads `indexed_at_ms` from the store's meta, not from `Freshness`; keep it that way (the watcher's `indexed_at_ms` stays as the in-process timestamp, unused by the DTO).

- [ ] **Step 5: Implement UI**

`types.ts`: drop `drain` from `Status`. `FreshnessBadge.tsx`: drop the `drain` sentence from the comment. `App.tsx`: `const lastFreshnessRef = useRef<string | null>(null);` and the subscribe freshness callback becomes:
```ts
      (s) => {
        setStatus(s);
        const text = `Index ${freshnessText(s)}`;
        if (text !== lastFreshnessRef.current) { lastFreshnessRef.current = text; announce(text); }
      },
```

- [ ] **Step 6: Spec amendment**

In the serve spec, in the §3 `/api/status` row remove `drain` and add `indexing`; in the freshness paragraph (§2 or wherever `DrainStats` is named as the freshness feed) replace it with: "The watcher is the only freshness writer. The `freshness` SSE event carries the same payload as `GET /api/status`; a refresh emits exactly two: `indexing: true` before, the result after. The actor's drain statistics are not on the API."

- [ ] **Step 7: Gates and commit**

`cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test --workspace`; from `ui/`: `bun test && bun run typecheck && bun run build`
```bash
git add -A
git commit -m "feat(serve): one freshness writer; the freshness event carries the full status; drain leaves the API"
```

---

## Self-review notes

- Spec §2 (Task 1: document edit, owned keys, header only for new files, temp+rename, three tests); §3 (Task 2: three tiers, id guarded by path and name, nearest line, one item per symbol by rank, `moved`, treegrid marker and panel line); §4 (Task 3: gate on `index_version`, one ref for tree and graph, exclusion path untouched); §5 (Task 4: fields, `StatusDto` payload built under the read lock, two events per attempt, `apply_failed`, de-duplicated announcement, spec amendment, tests); §6 non-goals honoured; §8 dependency (Task 1).
- Placeholders: none. Every code step carries code; the two toml_edit API fallbacks are stated.
- Type consistency: `SymbolRow.moved` (Task 2) is read by `RepoTree`, `DetailPanel` and fixtures; `indexRef` (Task 3) replaces `graphRef` everywhere in `App.tsx`; `ServerEvent::Freshness(StatusDto)` (Task 4) is matched in `events.rs` and the watcher tests; `Status` without `drain` matches `StatusDto` without `drain`.
- Known uncertainty: `toml_edit::ser::to_document` availability in 0.25 (fallback in Task 1 Step 4); happy-dom `act` around synchronous handler calls in the App tests (pattern already used in this file).
