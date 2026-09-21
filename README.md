# singularrag

A repo map with retrieval provenance for coding agents. It indexes a codebase with tree-sitter into SQLite, ranks symbols with personalised PageRank, and serves a token-budgeted map to Claude Code, Codex CLI or Copilot CLI over MCP. Every retrieval is recorded, served and cut, with reasons, so a human can later see what the agent was given and what it missed.

Status: v0, engine and MCP server. The map UI (`singularrag serve`) is next.

## Install

```sh
cargo install --path crates/singularrag
```

The UI is embedded in the binary at compile time, so build it first or the install fails: `cd ui && bun install && bun run build`.

Requires a Rust toolchain (1.85+) and Bun. The binary is self-contained; the index lives in `.singularrag/index.db` inside each repo (add it to `.gitignore`; `map.toml` next to it is meant to be committed).

## Connect an agent

The server reads the repo from its working directory, or `--repo PATH`.

If your host shows up as `unknown` in the retrieval rail, tell us what `clientInfo.name` it sends.

### Claude Code

```sh
claude mcp add --transport stdio singularrag -- singularrag mcp
```

Or project-scoped, in `.mcp.json` at the repo root:

```json
{
  "mcpServers": {
    "singularrag": {
      "command": "singularrag",
      "args": ["mcp"]
    }
  }
}
```

### Codex CLI

In `~/.codex/config.toml`:

```toml
[mcp_servers.singularrag]
command = "singularrag"
args = ["mcp"]
# cwd = "/path/to/repo"   # optional; defaults to Codex's working directory
```

### Copilot CLI

In `~/.copilot/mcp-config.json` (or `.copilot/mcp-config.json` in the repo):

```json5
{
  "mcpServers": {
    "singularrag": { "type": "stdio", "command": "singularrag", "args": ["mcp"] }
  }
}
```

## See what the agent was given

```sh
singularrag serve
```

Opens `http://127.0.0.1:<port>/#token=…` in your browser. The page shows every retrieval an agent made (grouped by host), the repo as a keyboard-navigable treegrid with each symbol marked served, cut or untouched for the selected retrieval, and the reasons in plain sentences. Pin or exclude files and symbols and leave notes; they are written to `.singularrag/map.toml` (commit it) and apply to the agent's next retrieval. The index refreshes as files change; the badge says how fresh it is.

Switch the main pane to **Map** for a Sigma.js projection of the same data: every indexed file is a node and every reference an edge, laid out once per index version; the selected retrieval paints files as served (filled), cut (ringed) or untouched (dimmed), a symbol's blast radius can be shown from the detail panel, and boundaries — named groups of files kept in `map.toml` — are drawn as labelled regions and edited from the panel. The treegrid stays the canonical view: the map has a summary label and a "Switch to table" control, and everything the map shows is also in the panel.

The server binds to localhost only and requires the per-run token in the URL. `--port N` pins a port, `--no-open` skips the browser.

Building from source needs Bun for the UI: `cd ui && bun install && bun run build`, then `cargo build --release`.

## Does it earn its place? (tier-two eval)

`singularrag-bench` runs headless Claude Code over the tier-one questions under named conditions and prints the verdict from the design spec: correctness not worse than Claude Code alone, and tokens or tool calls down by at least 25%.

```sh
cargo install --path crates/singularrag            # `singularrag` must be on PATH
git clone https://github.com/honojs/hono ../hono && git -C ../hono checkout 098e11912ab244c5c33931de007f04dc8e3c2929
cargo run -p singularrag-bench -- run --questions L1 --repeats 1 --conditions alone,singularrag   # smoke, a few dollars
cargo run -p singularrag-bench -- run                                                            # full: 12 × 3 × 3 = 108 sessions, about $25 at the smoke's $0.22 per session, capped at 108 × max_budget_usd
```

Results land in `eval/runs/<timestamp>-<label>/` with one record and raw stream per session and a `summary.md` (the only file committed). A CLAUDE.md inside the checkout is still loaded as project memory; it is identical across conditions, so it does not bias the comparison. `--dry-run` prints the commands, `--resume <dir>` continues an interrupted run, `score <dir>` rewrites the summary. The Serena condition needs `uvx`; a condition whose MCP server does not connect is aborted and reported, not run degraded. Sessions use `--setting-sources ""` and `--strict-mcp-config`, so your own hooks, plugins and MCP servers stay out of every condition.

## What the agent sees

Two tools. `repo_map` returns something like:

```
# singularrag · index 7f3a2c · HEAD 9b1e0d4 · fresh · retrieval r_000123
src/auth/session.ts:
    3  export function createSession(user: User, ttl: number): Session  ← src/http/middleware.ts, src/cli/login.ts
    7  export class SessionStore  ← src/http/middleware.ts
src/http/middleware.ts:
    2  export function requireSession(token: string): Session  ← src/http/routes.ts
# 42 of 310 symbols shown · 268 more ranked below budget · 25 recorded · widen with a larger budget or a focus file
```

Each row ends with the files that reference the symbol, strongest first, so locate, trace, blast-radius and placement questions can be answered from the map without opening files. `find_symbol` looks a name up and lists which files reference it. Neither tool ever returns function bodies, comments or string literals.

## CLI

```
singularrag index            # build or refresh the index
singularrag query "text"     # print the map the agent would get
singularrag find NAME        # look a symbol up
singularrag eval             # tier-one recall against eval/questions.toml
singularrag serve            # open the map UI on localhost
singularrag mcp              # serve over stdio
```

Logs go to stderr; set `RUST_LOG=debug` for more.

## Design

`docs/superpowers/specs/2026-09-19-singularrag-design.md` is the product and architecture spec; `docs/superpowers/specs/2026-09-20-singularrag-mcp-design.md` covers the server.
