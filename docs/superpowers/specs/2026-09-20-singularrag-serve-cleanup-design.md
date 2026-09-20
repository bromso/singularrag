# singularrag serve — cleanup design (the four items deferred from plan 3a)

Date: 2026-09-20. Status: approved in brainstorming; awaiting written-spec review.
Serve spec: `docs/superpowers/specs/2026-09-20-singularrag-serve-design.md` (§3 API, §5 single write path, §6 accessibility bind this). Map spec: `docs/superpowers/specs/2026-09-20-singularrag-map-design.md` (the join is shared with the map). This work lives on `cleanup-v0`, branched from `map-v0` (b2ad303, PR #8); the PR targets `map-v0` and is retargeted to `main` when #8 lands.

## 1. Goal

Close the four items the serve spec deferred: hand-written comments in `map.toml` survive page writes; retrieval items join to tree symbols by a chain that survives a reindex and names a moved symbol; the tree is refetched only when the index changes; freshness has one writer, one payload shape, and no duplicate events. No new views, no new routes, no new write path.

## 2. Comment-preserving map writes

`MapConfig::save_atomic` (core `config.rs`) becomes a document edit rather than a serialisation:

- Read the existing file into a `toml_edit::DocumentMut`. When the file is absent, start from an empty document preceded by `MAP_HEADER`.
- Replace the top-level items `pin`, `exclude`, `note`, `boundary` (arrays of tables) and the `deny` table with values serialised from the config (`toml_edit::ser::to_document` on the config, then item by item), leaving every other item of the document untouched.
- Write through the existing temp-file-and-rename path.

What survives: comments before and between top-level sections, blank lines, key order, and any top-level keys the page does not own. What does not: a comment inside an entry the page rewrote (the arrays are replaced wholesale). `MAP_HEADER` changes to `# Managed by singularrag serve. Hand edits and comments between sections are kept; a comment inside an entry the page rewrites is not.` and is written only when the file did not exist. `MapConfig::parse` and `load` are unchanged (serde over the same text).

Tests: a file with a leading comment, a comment between two `[[pin]]` tables, a trailing comment and an unknown top-level key is written after a pin toggle and all four survive byte-for-byte outside the replaced items; `load` after `save_atomic` equals the config; a missing file gets the header and parses; the existing validation and atomicity tests keep passing.

## 3. The join chain

`joinRetrieval` (`ui/src/lib/join.ts`) matches each retrieval item to a tree symbol in order:

1. `symbol_id` equals the tree symbol's `id` (the file has not been reindexed since the retrieval).
2. `(path, name, line_start)` equal (reindexed, symbol unchanged).
3. `(path, name)` equal with a different line: the symbol **moved**; among several same-named symbols in the file, the nearest `line_start` wins.

A `SymbolRow` gains `moved: boolean` (true only for tier 3). Each item joins at most one symbol and each symbol takes at most one item (first by rank). The treegrid renders the existing status mark plus the text `moved` after the status label for tier-3 rows; the detail panel shows `Moved since this retrieval (was line N)` where N is the item's recorded `line_start`. The map's file status (`fileStatusOf`) and the served/cut counts count moved symbols like any other.

Tests: one fixture per tier; a symbol with a changed id and unchanged line joins by tier 2 with `moved: false`; a same-named symbol at another line joins with `moved: true` and the nearest line wins when two candidates exist; an item whose name no longer exists in the file stays unjoined (untouched).

## 4. The refetch gate

`App.load()` fetches `status`, `retrievals` and the map document on every change event. `tree` and `graph` are fetched only when the status's `index_version` differs from the version last loaded, tracked in one ref shared by both, and on the first load. A hand edit to `map.toml` still produces a change event (the watcher watches it) and therefore a map-document refetch. No server change.

Tests (`App.test.tsx`): a change event with an unchanged `index_version` does not call `/api/tree` or `/api/graph` again; a change event whose status carries a new `index_version` does.

## 5. One freshness writer

- `Freshness` (serve `state.rs`) keeps `stale_count`, `lock_timeout`, `foreign_indexing`, `indexing`. The `drain` field and `DrainStatsJson` are removed from `Freshness`, `StatusDto`, the UI `Status` type and the badge. The actor's `Job::Stats` stays for the MCP drain loop; nothing on the serve side reads it. `indexed_at_ms` comes from the store's meta, written by every successful refresh.
- The SSE `freshness` event carries a full `StatusDto`, built by `queries::status(store, &freshness)` under the read lock at send time, so it always has `git_head`, `index_version` and file counts. The App assigns it to `status` as is.
- Exactly two events per refresh: `apply_started` sets `indexing = true` and broadcasts; `apply(stats)` sets the result with `indexing = false` and broadcasts. `set_indexing(false)` no longer exists as a broadcaster. The lock-retry path may add one more pair.
- The live region announces `Index <freshness text>` only when the text differs from the last freshness text it announced (the badge itself shows every payload).

Tests: the watcher test asserts exactly two `Freshness` events for one refresh, the first with `indexing: true`, the second with `indexing: false` and the refreshed `indexed_at_ms`; an events test asserts the serialised payload contains `git_head` and `index_version`; the App test asserts that two freshness payloads with the same text announce once; the serve integration test for `/api/status` no longer expects `drain`. The serve spec's §3 `/api/status` row and its freshness paragraph are amended to this model.

## 6. Non-goals

The Playwright test; the tree endpoint's shape; server-side ETags; any change to the MCP drain loop; Jonas's VoiceOver pass (still his).

## 7. Testing summary

Rust: `config.rs` round-trip tests; watcher and events tests as above; `tests/serve.rs` status and freshness assertions. UI: `join.test.ts` tiers; `App.test.tsx` refetch gate and announcement de-duplication; `FreshnessBadge.test.tsx` without `drain`. Gates as before.

## 8. Dependencies

`toml_edit` 0.25 (workspace dependency, `features = ["serde"]`; `toml` 1.x no longer pulls it in, so it is a new entry in the lockfile). No UI dependencies.
