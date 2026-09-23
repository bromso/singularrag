# singularrag — agent workflow parity design (query-first hook, `init`, `trace_path`, `changed`)

Date: 2026-09-21. Status: approved in brainstorming; awaiting written-spec review.
Parent spec: `docs/superpowers/specs/2026-09-19-singularrag-design.md` (§9 tools, §11 controls). Tier-two spec: `docs/superpowers/specs/2026-09-20-singularrag-tier2-design.md` (§3 conditions, §4 session, §5 records). This work lives on `workflow-v0`, branched from `main` at 2b6c07f (PRs #14 and #15 merged).

## 1. Goal

Close the workflow gaps the competitor survey (2026-09-21) found and that two tier-two runs point at: the agent reads as many files with the map as without it. Three deliverables: a Claude Code hook that denies the first `Read` of a session until the map has been consulted; `singularrag init`, which installs the hook and the MCP entry; and two graph tools the competitors all have, `trace_path` between two symbols and `changed`, the symbols a diff touches and who references them. No new views; the tools record retrievals like the existing ones, so the rail and the treegrid show them through the existing join.

Facts probed on 2026-09-21 with Claude Code 2.1.261: a `PreToolUse` hook can only allow, ask, or deny; a denial's `permissionDecisionReason` reaches the model; `additionalContext` exists only on `SessionStart` and `UserPromptSubmit`; a `--settings <file>` with a `hooks` block is loaded even under `--setting-sources ""`, and a hook denial appears in the result's `permission_denials`. MCP servers cannot register hooks.

## 2. The query-first hook

Two `PreToolUse` hooks, both `command` hooks running the binary, which reads the hook's JSON from stdin (`session_id`, `cwd`, `hook_event_name`, `tool_name`, `tool_input`) and writes the decision JSON to stdout:

- Matcher `mcp__singularrag__repo_map`: `singularrag hook map` writes the marker `<tmp>/singularrag-hook/<session_id>/mapped` and allows.
- Matcher `Read`: `singularrag hook read` denies exactly once per session, when all of these hold: no `mapped` marker for the session; no `denied` marker for the session; `tool_input.file_path` resolves under the repo root (the hook's `cwd` or `--repo`) and is an indexed, non-skipped file in `.singularrag/index.db`. On denial it writes the `denied` marker and returns

  ```json
  {"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"singularrag: this repository has a map. Call repo_map with your task first; it lists the relevant symbols and who references them. Then read what it points at."}}
  ```

  Every other case returns `{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow"}}`.

`<tmp>` is `std::env::temp_dir()`. Markers are empty files; the directory is per session id, never inside the repo, and never cleaned by singularrag (the OS temp dir is). A hook that cannot read its stdin, cannot open the index, or hits any error allows and exits 0: the hook must never break a session. The reason text names the tool so the model can act on it and reads as a policy line, not an instruction inside data; the probe showed the model treats denial reasons with suspicion, so the wording stays factual.

Codex CLI and Copilot CLI have no hook mechanism; they get the instruction snippet from §3.

## 3. `singularrag init`

`singularrag init [--repo PATH] [--project] [--host claude|codex|copilot|all]` (default `all`):

- Claude Code: merges the two hooks into `.claude/settings.local.json` (`--project`: `.claude/settings.json`), keeping every other key and every other hook, and skipping a hook entry whose `command` already contains `singularrag hook`; merges `{"mcpServers":{"singularrag":{"command":"singularrag","args":["mcp"]}}}` into `.mcp.json` the same way. The command in the hook is the absolute path of the running binary, so a `PATH` that differs inside Claude Code does not matter.
- Codex CLI and Copilot CLI: prints the `~/.codex/config.toml` and `mcp-config.json` entries the README already documents, plus an AGENTS.md paragraph: "This repository has a singularrag map. Call `repo_map` with your task before reading files; use `find_symbol` for a name, `trace_path` for how two symbols connect, and `changed` for what a diff touches."

`init` is idempotent: running it twice leaves both files byte-identical. It writes nothing outside `.claude/` and `.mcp.json` in the repo. It prints what it wrote.

## 4. `trace_path`

Inputs: `from` and `to`, each `path::symbol` (repo-relative path, declared name). Both must exist in the index; otherwise an error naming the input.

Search: breadth-first over the file reference graph the ranker already builds (`graph::build_graph` with no query), starting at `from`'s file, following edges in both directions, at most `MAX_PATH_DEPTH = 6` hops, first path found wins (BFS gives the shortest). Excluded files are absent from the graph as everywhere else. Each hop carries the symbol name of the edge.

Output (plain text after the freshness header):

```
src/cli/login.ts::login → src/auth/session.ts::createSession (login calls createSession)
src/auth/session.ts::createSession → src/auth/store.ts::SessionStore (createSession references SessionStore)
# 2 hops
```

or `# no path within 6 hops`. A `from` equal to `to` is `# 0 hops`. Provenance: one `retrievals` row with `tool = "trace_path"`, `query = "<from> -> <to>"`, and one served item per symbol on the path in hop order; `limit_n = MAX_PATH_DEPTH`. CLI: `singularrag path FROM TO`.

## 5. `changed`

Inputs: `base` (optional git ref; default the working tree against `HEAD`, which is uncommitted changes; `base = "main"` is the branch's whole diff against main via `git diff --unified=0 main...`). Runs `git -C <root> diff --unified=0 [<base>]` and `git -C <root> ls-files --others --exclude-standard` (untracked files count as fully changed) as read-only subprocesses; the parent spec's "no exec" rule is about agent-exposed execution and §9 says so. A repo that is not a git checkout returns an error.

Mapping: each hunk's new-side line range, per file; a symbol counts as changed when its `line_start..=line_end` intersects a range; a file with changes outside any symbol counts as a changed file with no symbol. Only indexed, non-skipped files count. For each changed symbol, the files that reference its name (the depth-one blast walk), at most `MAX_CHANGED = 50` symbols, ordered by file then line.

Output:

```
src/router.ts::match (lines 52-70) ← src/hono-base.ts, src/router/smart-router/router.ts +3
src/router.ts::add (lines 88-95) ← src/hono-base.ts
src/utils/url.ts (no symbol touched)
# 2 symbols in 2 files changed since HEAD · 5 referencing files
```

Provenance: one `retrievals` row with `tool = "changed"`, `query = base` (or `"HEAD"`), served items = the changed symbols, `limit_n = MAX_CHANGED`. CLI: `singularrag changed [--base REF]`.

Both tools share `repo_map`'s refresh-before-answer and freshness header, and go through the actor like the others.

## 6. Tool descriptions

- `trace_path`: "How two symbols connect: the shortest chain of references between `from` and `to`, each `path::name`, up to 6 hops, with the symbol each hop goes through. Use it for trace questions before reading files."
- `changed`: "What a change touches: the symbols whose lines a diff modifies and the files that reference each. `base` is a git ref; omitted means the working tree against HEAD. Use it before editing to see the blast radius and after editing to check it."

`INSTRUCTIONS` names all five tools in one sentence each.

## 7. UI

No new views. The rail lists `trace_path` and `changed` retrievals with their tool name and query; the treegrid and the detail panel show their served items through the existing join. `RetrievalSummary.tool` already carries the name; the rail's tool label maps the two new names to "Trace" and "Changed".

## 8. Eval

- The tier-two config gains an optional per-condition `settings` path (relative to the config file), passed as `--settings <absolute path>`; the file is committed under `eval/conditions/`. A condition's hooks file uses `<bin>`, the same `singularrag` command the harness warms up with (`SINGULARRAG_BIN` when set, else the bare name on `PATH`), substituted like `<checkout>`.
- The result line's `permission_denials` entries carry `tool_name`, `tool_use_id` and `tool_input` but no reason (probed 2026-09-21). A denial is a hook denial when its `tool_name` is `Read` and the stream's `tool_result` for that `tool_use_id` contains `singularrag:`; it is recorded in `Record.hook_denials` (count), not an abort. Any other denial still aborts the condition.
- New condition `singularrag+hook`: the singularrag MCP config plus `conditions/singularrag-hook.json` (the two hooks). The next run is `alone,singularrag,singularrag+hook`, same questions, repeats and model.
- Summary: `hook_denials` per condition in the table; the paired verdict is unchanged.

## 9. Security

- The hook subcommands read stdin and the index and write only marker files under the OS temp dir. They allow on any error.
- `init` writes only `.claude/settings.local.json` (or `settings.json` with `--project`) and `.mcp.json`, merging, never removing.
- `changed` executes `git diff` and `git ls-files` with fixed arguments; `base` is passed as one argument and rejected if it starts with `-`.
- Both tools return identifiers, signatures and paths only.

## 10. Parent spec amendments

- §9: five tools; `trace_path` and `changed` documented as in §4 and §5; `init` and `hook` as CLI subcommands.
- §11 controls: "no exec" becomes "no agent-exposed execution; `changed` runs `git diff` and `git ls-files` read-only with fixed arguments".
- Tier-two spec §3: the `settings` field and the `singularrag+hook` condition; §5: `hook_denials`.

## 11. Testing

Core: `trace_path` on the fixture (login → createSession is one hop; a symbol with no path; from equal to to; unknown symbol errors); `changed` on a temp git repo built in the test (a modified symbol, a change outside symbols, an untracked file, `base` pointing at a commit, a non-git dir errors). Hook: `singularrag hook read` with fixture stdin JSON (deny once, then allow; allow after a `map` marker; allow for a path outside the repo; allow for an unindexed file; allow on garbage stdin). `init`: writes both files, merges into an existing hooks block and an existing `.mcp.json` without losing keys, is idempotent, `--project` targets `settings.json`. MCP: five tools with spec descriptions; `trace_path` and `changed` over the real binary. Bench: a condition with `settings` passes `--settings`; a `singularrag:` Read denial becomes `hook_denials = 1` and the session completes; another denial aborts. UI: the rail labels the two tool names.

## 12. Non-goals

SessionStart context; denying more than once; hooks for Codex or Copilot; resolved calls; clusters; docs as a source type; any change to the map's budget or references.
