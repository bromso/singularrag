# singularrag — agent-authored notes design (the `annotate` tool)

Date: 2026-09-21. Status: approved in brainstorming; awaiting written-spec review.
Parent spec: `docs/superpowers/specs/2026-09-19-singularrag-design.md` (§5 non-goals, §7 `map.toml`, §9 tools, §11 controls bind this and are amended by §8 below). Serve spec: `docs/superpowers/specs/2026-09-20-singularrag-serve-design.md` (the note UI and `PUT /api/map` validation). This work lives on `annotate-v0`, branched from `main` after PR #13; PR #14 (the verdict rule) is independent.

## 1. Goal

Let the agent record what it learned about a file or symbol so the next session, and the human, get it without re-reading. A note is one or two sentences in `map.toml`, written through a third MCP tool, shown in the map and in `find_symbol`, and used by the ranking. The LLM work happens in the user's own session; singularrag still never calls a model.

Why this and not more: the comparison with LightRAG (2026-09-21) showed its semantic layer is an LLM at index time, which a subscription-only tool cannot have. The agent already in the session is the model that is allowed. Pins, excludes and boundaries stay human-only: they steer the ranking directly, and an agent pinning its own favourites would eat the loop the product exists for.

## 2. The note

`[[note]]` in `map.toml` keeps `path`, optional `symbol` and `text`, and gains three optional keys:

```toml
[[note]]
path = "src/router.ts"
symbol = "match"
text = "Every router implements this; SmartRouter picks one at first request and delegates."
by = "agent"
session = "mcp:claude-code:48213:1758440000"
at = "2026-09-21T09:14:02Z"
```

- `by` absent means a human wrote it (the UI or a hand edit); every existing file stays valid. The only other value is `agent`.
- `session` is the MCP session key of the writing session; `at` is RFC 3339 UTC. Both are present exactly when `by = "agent"`.
- One human note and one agent note per target, where a target is `path` or `path::symbol`. `MapConfig::validate` rejects a second of either kind on the same target.
- `text` is one paragraph: newlines and other control characters are replaced by a space, runs of spaces collapsed, trimmed, at most 300 characters after that. Longer is rejected, not truncated, so the agent learns the limit.

## 3. The tool

`annotate` joins `repo_map` and `find_symbol`. Inputs: `path` (string, required, repo-relative), `symbol` (string, optional), `text` (string, required; empty removes the agent's note on that target).

Description (agent-facing): "Record what you learned about a file or symbol that its signatures do not say: what it is for, an entry point, a trap, a convention. One or two sentences; the next session and the developer will see it in the map. `path` is repo-relative; `symbol` narrows the note to one definition in that file. Empty `text` removes your note. You can replace your own note on a target; a note the developer wrote is theirs."

Behaviour, in order:

1. `path` passes the serve spec's path rules (not absolute, no `..`, no leading `./`, forward slashes) and names a file in the index that is not skipped. `symbol`, if given, is defined in that file. Otherwise an MCP error naming the field.
2. `text` is normalised as in §2. If it is non-empty and `secrets::looks_secret` matches, the error is "text looks like a secret; notes are committed". A note is committed with the repo and the agent has read file bodies.
3. The engine reloads `map.toml` if its mtime moved (the existing check), then: empty text removes the agent note on the target if there is one; otherwise it replaces the agent note on the target or appends one. A human note on the same target is never touched. `by`, `session` and `at` are set by the engine, not the caller.
4. `MapConfig::save_atomic` writes the file (the toml_edit path from the serve cleanup, so comments and unknown keys survive).
5. The response is the freshness header line, then `noted src/router.ts::match (2 notes on this file)` or `removed your note on src/router.ts::match`.

No retrieval row is recorded: an annotation is not a retrieval, and the file watcher already turns the write into a change event for the UI. The write is counted by the tier-two harness as a tool call like any other.

## 4. Served back

**Map.** A file note renders directly under the file header and a symbol note directly under its row, as one line, eight spaces of indent, `note: text` for a human note and `note (agent): text` for an agent note. `map::fit` measures the rendered text, so notes are inside the budget. A note on a file that is not served is not rendered; §5 makes such a file more likely to be served when the query matches the note.

```
src/router.ts:  ← src/hono-base.ts, src/router/smart-router/router.ts +3
        note: the Router interface; concrete routers live under src/router/*/router.ts
   40  export interface Router<T>
   52  match(method: string, path: string): Result<T>
        note (agent): Every router implements this; SmartRouter picks one at first request and delegates.
```

**`find_symbol`.** The same line after a hit's `referenced from` line, for the symbol's note and then the file's.

**Ranking.** `rank_symbols` reads the config's notes. A query term (the same terms the FTS query uses) that appears in a note's text, case-insensitively and whole-word, makes the note's file a seed like an FTS hit, and its symbol (when the note names one) gets a bonus of the file's full rank, more than an FTS hit's share: a note is a deliberate pointer and a target carries at most one of each kind. `Reasons` gains `note_hit: bool`, and the detail panel renders it as "matches a note". No index change: notes live in the config and are read at rank time.

## 5. The UI

The detail panel shows the agent note under the human's own note textarea, with an "agent" badge, the time, and a delete button. It is not edited in place: a human who wants to keep its words puts them in their own note, so the one-human-one-agent rule of §2 cannot be broken from the panel. Delete goes through the existing `PUT /api/map`. The treegrid's existing note marker covers both kinds. `PUT /api/map` accepts the new keys and enforces §2's limits with the same 422 shape as the other fields. The map view is unchanged.

## 6. Two writers

The UI keeps its `expected_version` check. The MCP process re-reads `map.toml` before every write. A human save and an agent note landing in the same instant can lose one of them; it shows in `git diff`, and v0 accepts it.

## 7. Security

- The only write outside the store stays `map.toml`; the tool cannot name another file because the engine builds the path from the config root.
- `path` must be an indexed file, so the agent cannot annotate paths the index skipped (secrets, symlinks out of the root).
- Note text is scanned with the same secret patterns files are, and rejected on a hit.
- Length and paragraph limits bound what a session can add to a committed file per call; the human sees every note in the panel and in the diff.

## 8. Parent spec amendments

- §5 non-goals: "Write or exec tools. Read-only." becomes "No exec tools. One write tool, `annotate`, bounded to notes in `map.toml`; nothing else outside the store is ever written."
- §7 `map.toml`: notes carry `by`, `session`, `at`; the agent edits its own notes, the UI everything.
- §9: three tools; `annotate` documented as in §3.
- §11 controls: the first bullet becomes "only retrieval rows and `map.toml` are ever written, the latter by the UI and by the agent's own notes; no exec".

## 9. Testing

Core: `MapConfig` parses and round-trips the three keys, rejects two notes of one kind on a target and text over 300 characters; the engine's `annotate` adds, replaces and removes an agent note, leaves a human note alone, rejects an unindexed path, an unknown symbol and secret-like text; `render` places file and symbol notes; `rank_symbols` seeds a file whose note matches a term and sets `note_hit`; the serve integration test `PUT`s a note with the new keys and gets 422 for a second agent note on one target. MCP: the tool list is three with the spec descriptions; an `annotate` call changes the next `repo_map` text. UI: the panel shows the badge, delete clears it, edit turns it human; axe stays clean.

Tier one is unchanged. Tier two is unchanged: sessions are fresh, and a cross-session condition is not in scope.

## 10. Non-goals

Agent pins, excludes or boundaries; notes in SQLite or a promotion flow; notes on directories; a cross-session eval condition; any change to budget rules or to the reference suffix.
