# singularrag

A repo map with retrieval provenance for coding agents. It indexes a codebase with tree-sitter into SQLite, ranks symbols with personalised PageRank, and serves a token-budgeted map to Claude Code, Codex CLI or Copilot CLI over MCP. Every retrieval is recorded, served and cut, with reasons, so a human can later see what the agent was given and what it missed.

Status: v0, engine and MCP server. The map UI (`singularrag serve`) is next.

## Install

```sh
cargo install --path crates/singularrag
```

Requires a Rust toolchain (1.85+). The binary is self-contained; the index lives in `.singularrag/index.db` inside each repo (add it to `.gitignore`; `map.toml` next to it is meant to be committed).

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

The server binds to localhost only and requires the per-run token in the URL. `--port N` pins a port, `--no-open` skips the browser.

Building from source needs Bun for the UI: `cd ui && bun install && bun run build`, then `cargo build --release`.

## What the agent sees

Two tools. `repo_map` returns something like:

```
# singularrag · index 7f3a2c · HEAD 9b1e0d4 · fresh · retrieval r_000123
src/auth/session.ts:
    3  export function createSession(user: User, ttl: number): Session
    7  export class SessionStore
src/http/middleware.ts:
    2  export function requireSession(token: string): Session
# 42 of 310 symbols shown · 268 more ranked below budget · 25 recorded · widen with a larger budget or a focus file
```

`find_symbol` looks a name up and lists which files reference it. Neither tool ever returns function bodies, comments or string literals.

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
