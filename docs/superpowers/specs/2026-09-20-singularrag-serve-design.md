# singularrag serve — plan 3a design (the loop UI)

Date: 2026-09-20. Status: approved in brainstorming; awaiting written-spec review.
Parent spec: `docs/superpowers/specs/2026-09-19-singularrag-design.md` (§7 architecture A, §10 UI, §11 security and accessibility bind this plan). Builds on plan 2 (`mcp-v0`, PR #2); this work lives on `ui-v0`, stacked on `mcp-v0`. Plan 3b (the Sigma.js map projection and boundaries) follows and is out of scope here.

## 1. Goal

Ship the retrieval provenance loop as a human-facing tool: `singularrag serve` opens a localhost page where the developer sees every retrieval the agent made (served, cut, and why), browses the repo as a treegrid, pins or excludes files and symbols and leaves notes, and the next retrieval reflects it. The treegrid is the canonical, accessible view; the map in 3b is a projection of the same data. After this plan the parent spec's "done when" for the loop holds end to end in a browser.

## 2. Process model

`singularrag serve [--repo PATH] [--port N] [--no-open]` is a subcommand of the existing binary, dispatched before the shared `Engine::open` (like `mcp`) because it owns an actor.

- **Startup.** Bind `127.0.0.1:<port>` (`--port` default 0, ephemeral), generate a per-run token (32 random bytes, hex), print one line `http://127.0.0.1:<port>/#token=<hex>`, open the browser via the `open` crate unless `--no-open`, run one refresh, then serve.
- **Actor.** The plan-2 actor moves from `crate::mcp::actor` to `crate::actor` (two callers). It gains `Job::Refresh(oneshot<Reply<IndexStats>>)`, which runs one refresh with the configured budget and returns its stats. `EngineConfig.session_key` becomes `SessionKey::Fixed(String)` (serve uses `"serve"`) or `SessionKey::FromHandshake(Arc<Mutex<Option<String>>>)` (mcp); key format unchanged. `serve` spawns its own actor; the MCP process, if running, has its own. The advisory lock arbitrates.
- **Watcher.** `notify-debouncer-full` at 300 ms, recursive on the repo root, ignoring `.git/` and `.singularrag/`. Each debounced batch sends one `Job::Refresh`. The returned `IndexStats` update a shared `Freshness` snapshot (`Arc<RwLock<Freshness>>`) read by `/api/status` and broadcast on SSE. On `lock_timeout` the snapshot reports "another process is indexing" and the watcher retries once after 2 s; it is never an error.
- **Read path.** `Store::open_read_only(path)` is added to core: `SQLITE_OPEN_READ_ONLY | SQLITE_OPEN_NO_MUTEX`, `busy_timeout` 1000 ms, no DDL, and it fails with a clear error if the file's `schema_version` differs from the current one (the writer side owns rebuilds). The axum state holds that connection behind a `std::sync::Mutex`; handlers lock for the milliseconds a query takes. UI reads never wait behind an indexing chunk.
- **Live updates.** One tokio task polls `PRAGMA data_version` on the read connection every 250 ms; on change it broadcasts `change` with the newest retrieval id. Freshness snapshot changes broadcast `freshness`. Clients refetch what they display; the stream carries no rows. `KeepAlive` every 15 s. Fan-out is a `tokio::sync::broadcast` channel.
- **Errors.** API errors are JSON `{"error": "..."}` with 4xx/5xx; a busy read connection is 503 and the client retries with backoff; nothing panics; the actor dying exits the process with code 2 as in `mcp`. Logs go to stderr via `tracing` (`RUST_LOG`, default `warn`); stdout carries only the startup line.

## 3. HTTP API

All under `/api`, JSON, token-guarded (§5). Timestamps are Unix milliseconds.

| Route | Returns |
|---|---|
| `GET /api/status` | `{ index_version, git_head, indexed_at_ms, stale_count, lock_timeout, foreign_indexing: bool, files: { indexed, skipped }, drain: { chunks, last: IndexStats } }` |
| `GET /api/retrievals?limit=50&before=<id>` | newest first: `[{ id, session_key, session_label, tool, query, focus_files: [..], budget, limit_n, index_version, git_head, stale_count, created_at_ms, served, cut }]` |
| `GET /api/retrievals/{id}` | the retrieval plus `items: [{ rank, symbol_id, path, name, line_start, score, served: bool, reasons: Reasons }]` with `reasons_json` parsed |
| `GET /api/tree` | `[{ path, lang, skipped_reason, symbols: [{ id, name, kind, line_start, line_end, signature }] }]`, sorted by path; the client joins a selected retrieval's items onto symbol rows by `(path, name, line_start)` |
| `GET /api/skipped` | `[{ path, reason }]` |
| `GET /api/map` | the `MapConfig` as JSON, plus `version` |
| `PUT /api/map` | body: `MapConfig` JSON, optionally with `expected_version`; validates and writes `.singularrag/map.toml` atomically; returns the saved config with its new `version` |
| `GET /api/events?token=<hex>` | SSE: `change { max_retrieval_id }`, `freshness { <status snapshot> }` |

`session_label` is derived server-side from the key: `mcp:<client>:…` → the client slug title-cased with `-` → space (`claude-code` → "Claude Code"); `cli-<pid>` → "CLI"; `serve` → "UI"; anything else → the raw key.

**`PUT /api/map` validation.** Every `path` in `pin`, `exclude`, `note`, `boundary.paths` must be repo-relative: not absolute, no `..` segment, no leading `./`, forward slashes. `deny.extra_patterns` may only be a superset of the current file's extras (the built-ins are never in the file). Invalid → 422 with the offending field. Valid → serialise with `toml::to_string_pretty`, prepend the header comment `# Managed by singularrag serve. Hand edits are kept; comments are not.`, write to `map.toml.tmp` in the same directory, `rename` over `map.toml`. The Engine in the actor reloads the config on its next refresh (plan 1's mtime check).

**Compare-and-swap** (amended 2026-09-20). `version` is `map.toml`'s mtime in milliseconds, 0 when the file does not exist. When a `PUT` carries `expected_version` and it differs from the file's current version, the write is refused with 409 `{ error, field: "expected_version", current: <config + version> }` and nothing is written; the client reloads from `current` and the user redoes the edit. Without `expected_version` the write proceeds unconditionally. The watcher therefore does not filter `.singularrag/map.toml` (it still filters everything else under `.singularrag/`), so a hand edit refreshes and the page refetches.

## 4. UI

**Stack.** React 19 + TypeScript, Tailwind v4, shadcn on Base UI (components added via the manual install path: `components.json` plus copied files), react-aria-components for the tree, all built by Bun only: `bun run build` uses `bun build` with `bun-plugin-tailwind` to emit `ui/dist`; `bun run dev` runs `bun --hot` on `index.html` with a small `Bun.serve` script proxying `/api` to a `serve --no-open --port 4173` process. No Vite. `ui/` at the repo root has its own `package.json` and lockfile.

**Layout.** Three regions and a panel:
1. **Retrievals rail** (left): grouped by `session_label`, newest first; each row shows relative time, tool, query (or "no query"), served/cut counts, and freshness at that moment (fresh or N stale). Selecting a row drives the treegrid and panel. Pages backwards with `before`.
2. **Treegrid** (main): rows are files, expandable to their symbols. File rows: path, language, served and cut counts for the selected retrieval. Symbol rows: name, kind, line, signature, status, score, one-line reason summary. Sortable by status, score, path. A filter box above it narrows rows by path or symbol name, client-side. Without a retrieval selected: the whole repo with an empty status column. With one: files that had any item are expanded and sorted first; others collapsed below.
3. **Top bar**: freshness badge (fresh / N stale / indexing / another process indexing, with git HEAD short and index age) and the skipped-files trigger.
4. **Detail panel** (right): the focused row's reasons as sentences; pin, exclude and note actions; existing note text in a textarea.

**Status encoding.** Three states, served / cut / untouched, shown as a text label, a distinct icon shape (filled circle, outlined circle, dash), and a colour. Never colour alone.

**Reasons as sentences.** A pure function maps `Reasons` to fixed phrases: rank and score ("Ranked 3rd, score 0.11."); `referenced_by` ("Referenced from middleware.ts (2) and login.ts (1)."); `query_ident_match` / `fts_hit` ("Matched the query on <query terms>." or "Matched the query in the index."); `seeds` (`focus` → "You focused this file."; `pinned` → "Pinned."; `query:<terms>` → covered by the match sentence); `pinned` badge. Fields absent or false produce no sentence.

**Annotation loop.** Pin, exclude and note act on the focused row (file or symbol; symbol-level pins and notes use `path::symbol` targets). Each action sends the whole `MapConfig` to `PUT /api/map`; on success a toast reads "Saved. Applies to the next retrieval." and the row shows a pinned or excluded badge. The `deny` list appears read-only with the built-ins marked fixed. Boundaries are not editable in 3a.

**Freshness and skips.** Badge fed by `/api/status` and `freshness` events. Skipped-files sheet lists path and reason grouped by reason.

**Empty states.** No retrievals yet: "Connect an agent" with the README's Claude Code snippet. Repo not indexed: handled by the startup refresh; the badge shows "indexing" meanwhile.

**Theme.** Follows `prefers-color-scheme` via shadcn's tokens; no toggle.

## 5. Security (parent §11)

- Bind `127.0.0.1` only.
- Every `/api` request requires the per-run token: `Authorization: Bearer <hex>` on fetches, `?token=<hex>` on the EventSource. Compared in constant time. The page reads it from the URL fragment (which never reaches the server) into memory; never `localStorage`.
- `Host` must be exactly `127.0.0.1:<port>` or `localhost:<port>`; anything else is 403 before routing (DNS-rebinding control).
- No CORS headers at all. `Cache-Control: no-store` on `/api`; hashed assets `immutable`.
- The static shell and assets are served without the token; they contain no data.
- `PUT /api/map` is the only write; it is validated as in §3 and can only touch `.singularrag/map.toml`.
- The read connection is opened read-only at the SQLite level.

## 6. Accessibility (parent §11, WCAG 2.2 AA)

- The treegrid is the canonical view. It uses react-aria-components' tree primitives. Keyboard model (amended 2026-09-20; see §12 Q1): Up/Down move between rows; Right on a collapsed row expands it; Right on an expanded row moves focus into the row's controls (chevron, then the row action button); Left mirrors (back to the row, then collapse); Enter activates the focused row or control; Home/End jump to the first/last row. Rows are single-cell (react-aria-components renders one gridcell per row), so there is no cell-to-cell navigation. Type-ahead is the filter box. Every row action (pin, exclude, note) is reachable from the keyboard through a row action button that opens the panel actions.
- A polite `aria-live` region announces each new retrieval ("New retrieval from Claude Code: repo_map, 42 served, 25 cut, fresh") and each freshness change.
- Status never by colour alone (§4). Contrast ≥ 4.5:1 in both themes. Focus always visible; no keyboard traps; interactive targets ≥ 24 px.
- Nothing animates in 3a and the badge does not pulse, so reduced motion is honoured by default.
- Acceptance: axe-core clean over every rendered view in component tests; the keyboard-only walkthrough (select a retrieval, expand a file, focus a symbol, exclude it, confirm the toast, tab to the badge and skipped sheet) is an automated test; a VoiceOver pass by Jonas before the branch finishes.

## 7. Build and embedding

- `ui/dist` is gitignored. `crates/singularrag/build.rs` checks for `ui/dist/index.html` and fails with "run `bun run build` in ui/ first" if absent; `cargo:rerun-if-changed=../../ui/dist`.
- `#[derive(RustEmbed)] #[folder = "../../ui/dist"]`; axum serves `/` as `index.html` and `/assets/*` with `mime_guess` types and `Cache-Control: public, max-age=31536000, immutable`. One route; no SPA fallback.
- CI runs `bun install --frozen-lockfile && bun run build` in `ui/` before `cargo`.

## 8. Testing

**Rust.** Unit: constant-time token compare; Host check; `map.toml` validation (absolute path, `..`, shrunk deny → 422); atomic write leaves no temp file. Integration (`crates/singularrag/tests/serve.rs`, `reqwest` dev-dependency): spawn `serve --no-open --port 0` on the ts-mini fixture, parse the printed URL; no token → 401; wrong Host → 403; each route's shape; a CLI `query` against the fixture produces a `change` event; `PUT /api/map` with an exclude followed by a CLI `query` yields a retrieval without that file (the loop, end to end); touching a file changes freshness within 2 s; a held advisory lock reports `foreign_indexing: true`, not an error.

**UI.** `bun test` with happy-dom and Testing Library: `reasonsToSentences` for every field; status encoding renders label, icon and colour class; the tree's keyboard model with react-aria's test utilities; axe-core over each view; the keyboard walkthrough of §6.

**Deferred.** A Playwright end-to-end test (3b); the API integration test covers the loop and component tests cover the browser side.

## 9. Crates and packages added

Rust: `axum` 0.8, `tower-http` 0.7 (set-header), `rust-embed` 8, `mime_guess`, `notify` 8 + `notify-debouncer-full` 0.7, `open` 5, `rand` (token), `subtle` (constant-time compare), `tokio` gains `net`; dev: `reqwest` with the `stream` feature; the SSE test reader is a small hand-rolled parser over the response byte stream (no extra crate). UI: `react`, `react-dom`, `react-aria-components`, `@base-ui/react` (via shadcn), `tailwindcss` 4, `bun-plugin-tailwind`, `axe-core`, `@testing-library/react`, `happy-dom`. Exact versions pinned by `cargo add` / `bun add` at implementation time.

## 10. Non-goals (3a)

The Sigma.js map and boundaries (3b); Playwright; multi-repo; remote access, HTTPS, or auth beyond the token; a theme toggle; editing the deny list from the UI; server-side search; a "try a query" box (a human-issued query would pollute the provenance rail; `serve` is an observer); comment-preserving `map.toml` writes.

## 11. Decisions recorded

- `PUT /api/map` writes through serde and drops hand-written comments; the file's header says so. `toml_edit` is open for 3b.
- Sessions are labelled from the key (§3). The rail has no total count.
- One read connection behind a mutex, not a pool: queries are milliseconds and the process is single-user.
- The UI holds the whole `MapConfig` and sends it whole; there are no per-annotation endpoints.

## 12. Open questions

1. ~~Which react-aria-components export supplies the treegrid role in the current release (`Tree` with row cells vs a `TreeGrid` primitive); the plan verifies against the installed version.~~ **Answered 2026-09-20:** `Tree`/`TreeItem`/`TreeItemContent` (react-aria-components 1.21). `Tree` renders `role="treegrid"`; there is no `TreeGrid` primitive, and each `TreeItem` renders exactly one `role="gridcell"`, so rows are single-cell. §6's keyboard paragraph was amended to the model that follows from that.
2. Whether to adopt `toml_edit` in 3b so comments survive.
3. Whether `/api/tree` needs pagination on repos far larger than hono (2k symbols, a few hundred KB today).
4. Whether the MCP process's drain budget should be reduced while `serve` is running, to shorten the watcher's `lock_timeout` stretches (plan-2 handoff).
