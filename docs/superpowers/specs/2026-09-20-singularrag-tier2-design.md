# singularrag-bench — plan 4 design (the tier-two eval harness)

Date: 2026-09-20. Status: approved in brainstorming; awaiting written-spec review.
Parent spec: `docs/superpowers/specs/2026-09-19-singularrag-design.md` (§12 eval set, §14.6 host coverage bind this plan). Builds on plans 1 to 3a, all merged on `main` (a94594b); this work lives on `bench-v0`, branched from `main`. Plan 3b (the Sigma.js map) is not started and is out of scope here.

## 1. Goal

Answer the parent spec's judging question with numbers: does singularrag earn its place in the agent's context? `singularrag-bench` runs headless Claude Code sessions over the twelve tier-one questions under named conditions (Claude Code alone, plus singularrag, plus Serena), scores each session's answer against the same gold sets tier one uses, records tokens, tool calls, cost and wall time, and prints the §12 verdict. It is a weekly instrument, not a product feature; nothing in it ships in the `singularrag` binary.

## 2. Process model

`singularrag-bench` is a second binary crate in the workspace, `crates/singularrag-bench`. It depends on `singularrag-core` only for `eval::load_questions` and the `Question` type.

One invocation of `singularrag-bench run` is one run. In order:

1. Resolve the run config (§3). Verify: the checkout exists, `git rev-parse HEAD` equals the pinned commit, the tree is clean ignoring `.singularrag/` and `.serena/`, `claude` is on `PATH`, every condition's MCP config file exists, exactly one condition is the baseline. Any failure is a hard error before a run directory is created.
2. Create `eval/runs/<UTC timestamp>-<label>/` and write `run.toml` (§6).
3. If any selected condition's MCP config runs the `singularrag` command, run `singularrag index --repo <checkout>` once, so that condition starts fresh rather than paying the first index inside a timed session.
4. Loop conditions, then questions, then repeats, one session at a time (§4). Each session's record is written as soon as it ends, so an interrupted run keeps every finished session; `--resume <run-dir>` skips sessions whose record exists.
5. Write `summary.md` (§7) and print it.

Sessions are sequential. There is no concurrency and no daemon.

## 3. Run config

`eval/tier2.toml`, committed:

```toml
repo = "../hono"                  # relative to this file, or absolute
commit = "098e11912ab244c5c33931de007f04dc8e3c2929"
questions = "questions.toml"      # relative to this file
repeats = 3
max_turns = 25
max_budget_usd = 0.50
tools = ["Read", "Grep", "Glob"]  # Claude Code built-ins; MCP tools come from the condition
answer_max = 15                   # cap on symbols an answer may list

[[condition]]
name = "alone"
baseline = true

[[condition]]
name = "singularrag"
mcp_config = "conditions/singularrag.json"

[[condition]]
name = "serena"
mcp_config = "conditions/serena.json"
```

`mcp_config` paths are relative to the config file. The MCP config files are ordinary Claude Code `mcpServers` JSON, committed under `eval/conditions/`:

- `singularrag.json`: `{ "mcpServers": { "singularrag": { "command": "singularrag", "args": ["mcp", "--repo", "<checkout>"] } } }`. The harness substitutes `<checkout>` with the resolved absolute path when writing the temporary config it actually passes, so the committed file stays machine-independent. `singularrag` must be on `PATH` (`cargo install --path crates/singularrag`); the harness records `singularrag --version` in `run.toml`.
- `serena.json`: `{ "mcpServers": { "serena": { "command": "uvx", "args": ["--from", "git+https://github.com/oraios/serena", "serena", "start-mcp-server", "--context", "claude-code", "--project", "<checkout>"] } } }`, with the same substitution.

The model is not configured. The CLI's default is used, and the model name each session reports is stored in its record, so a change of default between weekly runs is visible rather than silent.

## 4. Session

The prompt is identical across conditions. It is the question's `query`, then the answer contract:

> Answer by listing the symbols that answer the question, as `path::name`, where `path` is relative to the repository root and `name` is the symbol's declared name. List at most `<answer_max>`, most important first. Use the tools available to you as you see fit.

The structured-output schema is `{ "type": "object", "properties": { "symbols": { "type": "array", "items": { "type": "string" }, "maxItems": <answer_max> } }, "required": ["symbols"] }`.

The command, run with the checkout as working directory:

```
claude -p <prompt>
  --output-format stream-json --verbose
  --json-schema <schema>
  --tools Read,Grep,Glob            # from `tools`
  --strict-mcp-config
  [--mcp-config <temp condition json>]   # absent for a condition with no mcp_config
  --setting-sources "" --disable-slash-commands
  --permission-mode dontAsk --permission-prompts none
  --no-session-persistence
  --max-turns 25 --max-budget-usd 0.50
```

`--strict-mcp-config` is what keeps the developer's own MCP servers out of every condition; `--tools` names the built-ins explicitly so the alone condition is Read, Grep and Glob and nothing else. `--setting-sources ""` loads no user, project or local settings, which is what keeps the developer's hooks and plugins out: probed on 2026-09-20, a session without it ran the SessionStart hooks of the superpowers plugin and cached a 22k-token system prompt; with it and `--disable-slash-commands` the init message reports no plugins and no skills, the explicit MCP config is still loaded, and the cached prompt is 4k tokens. Claude Code's minimal mode (`--bare`) is not used because it authenticates only with an API key, and `--safe-mode` is not used because it also drops the explicit MCP config.

The structured answer is delivered as a tool call named `StructuredOutput`; it is excluded from tool-call counts because it is the answer mechanism, not retrieval.

Stdout is streamed to `<qid>-<repeat>.stream.jsonl` in the condition's directory as it arrives; stderr is captured to `<qid>-<repeat>.stderr` only when non-empty. After the child exits the harness re-checks the tree is clean (same exclusions as §2); a dirty tree aborts the run, because later sessions would see a different repository.

## 5. Parsing and scoring

The stream is one JSON object per line. The parser keeps three kinds and ignores the rest:

- `system` with `subtype: "init"`: `model`, `tools`, and `mcp_servers` (name and status per server).
- `assistant`: each `tool_use` content block is counted by its `name`.
- `result`: `usage.input_tokens`, `usage.output_tokens`, `usage.cache_creation_input_tokens`, `usage.cache_read_input_tokens`, `total_cost_usd`, `num_turns`, `duration_ms`, `is_error`, `subtype`, `structured_output`.

Tokens are recorded as those four numbers, unchanged. The comparison metric "tokens" is their sum, and the summary header says so.

Scoring: `structured_output.symbols` is normalised (trim; strip a leading `./`; drop entries without `::`; truncate to `answer_max`). Recall is the share of the question's gold set present in the normalised list, the same arithmetic as tier one. Precision, the share of the list that is gold, is recorded so a condition that answers by listing everything is visible. A session with `is_error: true`, a `subtype` other than `success`, or no `structured_output` scores recall 0 and is flagged `failed` with the reason. Failed sessions count in the mean: an agent that runs out of turns has failed the question.

**Deviation from the parent spec §12:** placement questions are scored by gold recall like the others, not by a 0 to 2 rubric. Their gold sets already exist in `eval/questions.toml` and tier one already scores them that way; a rubric would need a grader, which is out of scope (§10).

## 6. Storage

The run directory is `eval/runs/<timestamp>-<label>/` where the timestamp is UTC in the form `20260920T153000Z` and the label defaults to `run`.

```
eval/runs/<timestamp>-<label>/
  run.toml               # resolved config, commit, claude version, singularrag version
  <condition>/
    <qid>-<repeat>.stream.jsonl
    <qid>-<repeat>.stderr    # only when non-empty
    <qid>-<repeat>.json      # the parsed record
  summary.md
```

The record: `question`, `condition`, `repeat`, `model`, `recall`, `precision`, `hit`, `miss`, `answer` (normalised list), `tool_calls` (map name → count), `tokens` (the four numbers), `cost_usd`, `turns`, `duration_ms`, `failed`, `reason`.

`eval/runs/` is gitignored except `summary.md` files, so the numbers travel with the repo and the streams do not.

## 7. Summary and verdict

`summary.md`:

1. Header: run directory, commit, claude version, singularrag version, the models seen, and the sentence "tokens = input + output + cache creation + cache read".
2. One row per condition: sessions, failed, mean recall, mean and median tokens, mean tool calls, mean wall seconds, total cost.
3. One row per question and condition with mean recall over repeats, so a regression on one question is visible.
4. For each non-baseline condition, the §12 verdict. Correctness holds when the condition's mean recall is at least the baseline's minus 0.02. Efficiency holds when the condition's mean tokens or mean tool calls is at least 25% below the baseline's. Both hold: "earns its place". Otherwise the line names the failed test and the margin.

The same text prints to stdout. `singularrag-bench score <run-dir>` rewrites `summary.md` from the records on disk, so a change to the verdict rules never needs a rerun.

## 8. Failures

- Before any session (§2 step 1): hard error, no run directory.
- Per condition: if the first session's init message reports any MCP server whose status is not `connected`, the condition is aborted after that session; its record is kept, the abort and the reported status go in the summary, and the run continues with the remaining conditions.
- Per session: non-zero exit, an unparseable stream, or no `result` line is a failed session with the reason; the run continues.
- A run whose baseline condition was aborted prints the tables but no verdict.

## 9. CLI

```
singularrag-bench run   [--config eval/tier2.toml] [--label <name>]
                        [--conditions a,b] [--questions L1,T2]
                        [--repeats N] [--dry-run] [--resume <run-dir>]
singularrag-bench score <run-dir>
singularrag-bench parse <stream.jsonl>
```

`--conditions`, `--questions` and `--repeats` narrow a run; a smoke run is `--questions L1 --repeats 1`. `--dry-run` prints every command line it would spawn, prompt included, and exits without touching the checkout or creating a run directory. `parse` prints the record for one stream and is the debugging seam.

## 10. Non-goals

Codex and Copilot CLI conditions (neither is installed here; §14.6 resolves to Claude Code only for v0). Concurrent sessions. An LLM grader. A UI over runs. Statistics beyond mean and median over three repeats. Cost tracking across runs. A system-prompt hint steering the agent toward `repo_map`: the prompt is identical across conditions because the question is whether the tool earns its place under the agent's own judgement.

## 11. Testing

- Parser and scorer are pure functions over text, tested against recorded fixtures under `crates/singularrag-bench/tests/fixtures/`: a successful session, a max-turns failure, a session with no structured answer, and an init message with a disconnected MCP server. The fixtures come from the first smoke run, with session ids and any absolute paths removed.
- Scorer table tests: normalisation cases, the `answer_max` cut, empty gold, precision, and the verdict thresholds at their boundaries (recall exactly baseline minus 0.02; tokens exactly 25% below).
- Run loop: an integration test puts a fake `claude` script first on `PATH` that replays a fixture, so config resolution, spawning, the dirty check, resume and summary run end to end in `cargo test` against a temporary git checkout. Nothing in CI calls Claude.
- CI: the existing workflow's `cargo fmt`, `clippy -D warnings` and `cargo test --workspace` cover the new crate without changes.

## 12. Crates

`clap`, `serde`, `serde_json`, `toml`, `chrono` (timestamp), `tempfile` and `assert_cmd` for tests, and `singularrag-core`. No async runtime: the child's stdout is read on a thread with a blocking reader.

## 13. Cost note

A trivial headless call on the current default model cost about $0.46 (system-prompt cache creation dominates). A full run of 12 questions × 3 conditions × 3 repeats is 108 sessions and can reach a few hundred dollars; the per-session cap bounds it at 108 × `max_budget_usd`. Use `--questions` and `--repeats 1` for smoke runs.
