# singularrag MCP server — plan 2 design

Date: 2026-09-20. Status: approved in brainstorming; awaiting written-spec review.
Parent spec: `docs/superpowers/specs/2026-09-19-singularrag-design.md` (§7–§9, §11 bind this plan). Builds on plan 1 (branch `engine-v0`, PR #1); this work lives on `mcp-v0`, stacked on `engine-v0`.

## 1. Goal

Expose the plan-1 `Engine` to Claude Code, Codex CLI and Copilot CLI as a stdio MCP server with exactly the two tools in parent spec §9, complete the second half of parent spec §8 (finish an interrupted refresh in the background), and document host configuration. After this plan an agent in any of the three hosts can call `repo_map` and `find_symbol` and every call is recorded as provenance for plan 3's UI.

## 2. Process model

`singularrag mcp [--repo PATH]` is a subcommand of the existing binary.

- **Runtime.** A tokio runtime drives rmcp over stdio. stdout carries the protocol and nothing else; stderr carries `tracing` logs, level `warn` by default, `RUST_LOG` overrides.
- **Engine actor.** One plain OS thread owns the `Engine`. The rmcp handler holds only an `std::sync::mpsc::Sender<Job>` (wrapped so the handler is `Send + Sync + Clone`). `Job` is an enum: `Map(MapRequest, oneshot::Sender<Result<MapResponse>>)`, `Find(FindRequest, oneshot::Sender<Result<FindResponse>>)`. Tool handlers `await` the one-shot; nothing is held across an await, and the async runtime is never blocked. Concurrent tool calls queue in arrival order. The `Engine` API from plan 1 is used unchanged.
- **Lazy open.** The Engine is opened on the actor thread at the first job, after the MCP initialize handshake has delivered `clientInfo`, so the session key can include the client name. If open fails (bad `--repo`), every job returns that error and the server stays up.
- **Repo root.** `--repo` if given, else the process's current directory. All three hosts spawn stdio servers in the project directory (Codex additionally has a per-server `cwd` field). Claude Code's `roots/list` is not consulted in v0 (open question 1).
- **Session key.** `mcp:<client>:<pid>:<start_ms>`, where `<client>` is `clientInfo.name` lower-cased with whitespace replaced by `-`, or `unknown` if initialize has not run. No host passes a session id to a stdio server (MCP has none for stdio; the 2026-07-28 spec removed protocol sessions from HTTP too).
- **Background refresh (parent §8, second half).** The actor loop: block for a job; run it; if the response carried `stale_count > 0` and the refresh did not time out on the lock, then while `try_recv` finds no job, call `engine.refresh(budget)` and loop until `remaining == 0` or `lock_timeout` is set (another process is indexing; its work makes the next inline refresh cheap) or a job arrives. Each chunk re-walks the tree in tens of milliseconds and parses only leftovers, so the backlog drains between tool calls and the next call reports `fresh`. A chunk never exceeds the budget, so a job arriving mid-drain waits at most one chunk.
- **One Engine API addition.** `Engine::set_refresh_budget(Duration)` (default `REFRESH_BUDGET`, 2 s) so `repo_map`/`find_symbol` use a configurable inline budget; the actor uses the same value for drain chunks. Tests set it to zero to force a stale first response without touching the lock.
- **Errors.** An `Engine` error becomes a tool result with `is_error = true` and the error text as the content; never a JSON-RPC error and never a panic. The only fatal path is the actor thread dying (channel closed): the process logs to stderr and exits non-zero so the host restarts it.

## 3. Tool surface (parent §9, unchanged contracts)

Two tools via rmcp's `#[tool_router]`, inputs as `Parameters<T>` with `schemars` derives, output one text content block containing the engine's text verbatim.

### `repo_map`
- Description (agent-facing): "Token-budgeted map of the symbols most relevant to a task, each with the files that reference it. Call this first and answer locate, trace, blast-radius and placement questions from it; read a file only to confirm a detail the map does not show. `query` is a question or identifiers; `focus_files` are repo-relative paths you already know matter; `budget_tokens` defaults to 1024, up to 8192 for trace and blast-radius questions. Rows are `line  signature  ← referencing files`, never bodies. The first line says how fresh the index is; if it says STALE, call again after a moment."
- Input: `query: Option<String>`, `focus_files: Option<Vec<String>>`, `budget_tokens: Option<u32>`. Missing values take `MapRequest::default()`. Out-of-range budgets are clamped by the engine, never rejected.

### `find_symbol`
- Description: "Look up a symbol by name: exact, prefix, or split words (`create session` finds `createSession`). Returns the definition's path, line and signature and which files reference it. Optional `kind` filter: function, class, method, type, const, module. `limit` defaults to 10, max 50."
- Input: `name: String`, `kind: Option<String>`, `limit: Option<u32>`.

### Server info
Name `singularrag`, version from `CARGO_PKG_VERSION`, capabilities: tools only. Instructions string: "singularrag gives you a ranked map of this repository. Call repo_map first with your task as the query, then read only the files it points at. Use find_symbol to locate a name. Both tools are read-only. A STALE header means files changed since indexing; the index catches up in the background."
No resources, no prompts, no notifications.

## 4. Host configuration (README)

A root `README.md` with: three sentences on what singularrag is; build/install (`cargo install --path crates/singularrag` for now); and one snippet per host, verified against the hosts' current docs on 2026-09-20:
- Claude Code: `claude mcp add --transport stdio singularrag -- singularrag mcp`, and the project-scoped `.mcp.json` form `{"mcpServers":{"singularrag":{"command":"singularrag","args":["mcp"]}}}`.
- Codex CLI: `~/.codex/config.toml`, `[mcp_servers.singularrag]` with `command = "singularrag"`, `args = ["mcp"]`; note the optional `cwd` field.
- Copilot CLI: `~/.copilot/mcp-config.json` or repo-level `.copilot/mcp-config.json`, `{"mcpServers":{"singularrag":{"type":"stdio","command":"singularrag","args":["mcp"]}}}`.
Plus a short "what the agent sees" section showing one `repo_map` output. No install subcommand in v0.

## 5. Testing

- **Actor unit tests** (in the mcp module, real Engine on the `ts_mini` fixture): enqueue a Map and a Find job and assert responses match `Engine` called directly; set the refresh budget to zero so the first response has `stale_count > 0`, restore the default, then assert the background drain reaches `remaining == 0` with no further jobs (observable via the next response's `fresh` header and a `files` table with no stale rows); hold the advisory lock from the test during a first call and assert the drain does not run while it is held; a bad root yields `is_error` results and the actor stays alive for the next job.
- **Child-process integration test** (`crates/singularrag/tests/mcp.rs`, `#[tokio::test]`): an rmcp client spawns the built binary with `mcp --repo <fixture>` over stdio, completes initialize with `clientInfo.name = "singularrag-test"`, lists tools and asserts exactly `repo_map` and `find_symbol` with the descriptions above, calls both tools and asserts the header line and that the text equals the CLI's `query`/`find` output for the same inputs, and asserts a `retrievals` row whose `session_key` starts with `mcp:singularrag-test:`.
- **Failure test:** `--repo /nonexistent` starts, and the first tool call returns `is_error` containing the path.
- Tier one eval unaffected; tier two is plan 4.

## 6. Crates added

`rmcp` 3.4 (`server`, `transport-io`; `transport-child-process` as a dev-dependency for the test client), `schemars` 1.2 (re-exported by rmcp), `tokio` 1.53 (`rt-multi-thread`, `macros`, `io-std`, `sync`), `tracing` 0.1, `tracing-subscriber` 0.3 (`env-filter`). Exact versions pinned by `cargo add` at implementation time.

## 7. Non-goals

The `serve` UI (plan 3); `roots/list`; an install subcommand; HTTP transport; per-host plugins; multi-repo; the tier-two harness (plan 4); any change to ranking or the eval questions.

## 8. Open questions

1. Multi-root sessions: Claude Code answers `roots/list` with the launch directory plus `--add-dir` entries. v0 is single-repo; revisit with plan 3's multi-repo question.
2. Whether the drain loop should also run when a process starts with a large backlog but no tool call has arrived yet (pre-warm). v0: no, refresh happens on first call.
3. Codex and Copilot `clientInfo.name` values are unverified; the README asks users to report what their rail shows.
