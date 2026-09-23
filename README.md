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

Five tools. `repo_map` returns something like:

```
# singularrag · index 7f3a2c · HEAD 9b1e0d4 · fresh · retrieval r_000123
src/auth/session.ts:  ← src/http/middleware.ts, src/cli/login.ts
    3  export function createSession(user: User, ttl: number): Session
    7  export class SessionStore
src/http/middleware.ts:  ← src/http/routes.ts
        note (agent): Wraps every request; the token check lives here.
    2  export function requireSession(token: string): Session
docs/design.md:
  108  ## 8. Freshness
# 42 of 310 symbols shown · 268 more ranked below budget · 25 recorded · widen with a larger budget or a focus file
```

Each file header ends with the files that reference it, strongest first, and a note (yours or the agent's) sits under the header or under its symbol's row, so locate, trace, blast-radius and placement questions can be answered from the map without opening files. `find_symbol` looks a name up and lists which files reference it. Neither tool ever returns function bodies, comments or string literals.

Documents rank alongside code: a Markdown or HTML heading is a `section` row showing its heading line, a config key is a `key` row showing its type (`scripts.build: string`), and the agent reads the section the map points at rather than the whole file.

`annotate` lets the agent leave a one-paragraph note on a file or symbol; it lands in `map.toml`, shows in the next map with an `(agent)` tag, and the developer can delete it from the page.

`trace_path` answers how two symbols connect; `changed` lists what your uncommitted (or branch) changes touch and who references each symbol. `singularrag init` installs a Claude Code hook that refuses the first file read of a session until the map has been consulted, once per session.

## CLI

```
singularrag index            # build or refresh the index
singularrag query "text"     # print the map the agent would get
singularrag find NAME        # look a symbol up
singularrag path FROM TO     # the shortest reference chain between two path::symbol
singularrag changed          # the symbols a diff touches and who references them (--base REF)
singularrag init             # install the query-first hook and the MCP entry for this repo
singularrag eval             # tier-one recall against eval/questions.toml
singularrag serve            # open the map UI on localhost
singularrag mcp              # serve over stdio
```

Every command takes `--repo <dir>` (default: the current directory). The directory may be a workspace: see below.

Logs go to stderr; set `RUST_LOG=debug` for more.

## Workspaces and documents

A workspace is any directory holding `.singularrag/`. With no `.singularrag/workspace.toml` the directory is its own only root and paths are relative to it, as always. To index several directories together, say a repo and a notes vault, declare roots:

```toml
[[root]]
name = "app"
path = "../app"            # relative to the workspace directory, or absolute

[[root]]
name = "vault"
path = "/Users/jonas/Notes"
```

Every path is then `<name>/<relative>` (`app/src/auth/session.ts`), in the map, in `map.toml` and in the UI. Names are `[A-Za-z0-9_-]`, unique, and no root may contain another. Each root keeps its own `.gitignore`; the deny list and `map.toml` excludes apply to all of them. singularrag reads this file and never writes it. It is read when `serve` or `mcp` starts: edit it, then restart. The hook and the `.mcp.json` entry that `init` writes take the workspace from the process's working directory, so run the agent from the workspace directory or pass `--repo` in the hook command.

Documents are indexed next to code:

| format | extensions | symbols |
|---|---|---|
| Markdown | `.md` `.markdown` | a `section` per heading (h4 and deeper fold into their parent) |
| HTML | `.html` `.htm` | a `section` per heading, an `element` per `id` |
| CSS | `.css` | a `rule` per selector; rules inside `@media` fold into one |
| JSON, YAML, TOML | `.json` `.yaml` `.yml` `.toml` | a `key` per key path to depth 2, with the value's type, never the value |
| text | `.txt` | one `document` |

Every document also gets a `document` symbol named by its file stem, so `[[design]]` and `../specs/design.md` links resolve to it. Section text is searchable; code spans that name one identifier, wiki links and relative links become references; fenced code blocks and front matter do not.

Skipped, with the reason shown in the UI's skipped sheet: documents over 256 KB, lockfiles (`package-lock.json`, `yarn.lock`, `pnpm-lock.yaml`, `Cargo.lock`, `bun.lock`, `bun.lockb`, `composer.lock`, `Gemfile.lock`, `poetry.lock`), minified files (average line over 500 characters) and anything that looks like a secret.

## Design

`docs/superpowers/specs/2026-09-19-singularrag-design.md` is the product and architecture spec; `docs/superpowers/specs/2026-09-20-singularrag-mcp-design.md` covers the server.
