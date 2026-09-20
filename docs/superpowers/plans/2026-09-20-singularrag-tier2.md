# singularrag-bench (plan 4, tier-two eval harness) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A second binary, `singularrag-bench`, that runs headless Claude Code sessions over the tier-one questions under named conditions, scores each answer by gold recall, records tokens, tool calls, cost and wall time, and prints the parent spec's §12 verdict.

**Architecture:** One new crate `crates/singularrag-bench` in the existing workspace. Pure modules (`stream` parser, `score`, `summary`, `config`) are tested from fixtures; `session` builds the `claude -p` command and streams its stdout to a file; `run` is the loop with preflight, resume, dirty check and condition abort. Tests replace `claude` with a fake script through an environment variable; CI never calls a model.

**Tech Stack:** Rust 2021 (rust-version 1.85), clap 4, serde, serde_json, toml, chrono, `singularrag-core::eval::{load_questions, Question}`; assert_cmd + tempfile for tests.

**Spec:** `docs/superpowers/specs/2026-09-20-singularrag-tier2-design.md` (this plan argues from it; read it first). Parent: `docs/superpowers/specs/2026-09-19-singularrag-design.md` §12.

## Global Constraints

- Branch `bench-v0` from `main` (a94594b). Run everything from the worktree root. Commit after every task.
- `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace` must be green at the end of every task. `#![forbid(unsafe_code)]` in `main.rs`.
- Nothing in `cargo test` spawns the real `claude`. The binary name comes from `SINGULARRAG_BENCH_CLAUDE` (default `claude`); tests set it to a fake script. Likewise `SINGULARRAG_BENCH_SINGULARRAG` (default `singularrag`).
- The `claude` command line is exactly the spec §4 list: `-p <prompt> --output-format stream-json --verbose --json-schema <schema> --tools <tools joined by ","> --strict-mcp-config [--mcp-config <path>] --setting-sources "" --disable-slash-commands --permission-mode dontAsk --permission-prompts none --no-session-persistence --max-turns <n> --max-budget-usd <x>`.
- Tool-call counts exclude the tool named `StructuredOutput` (spec §4).
- Tokens are recorded as four numbers exactly as reported (`input_tokens`, `output_tokens`, `cache_creation_input_tokens`, `cache_read_input_tokens`); "tokens" for comparison is their sum (spec §5).
- Scoring (spec §5): normalise = trim, strip leading `./`, drop entries without `::`, truncate to `answer_max`; recall = |gold ∩ answer| / |gold| (1.0 when gold is empty); precision = |gold ∩ answer| / |answer| (0.0 when answer is empty). A session with `is_error: true`, `subtype != "success"`, or no `structured_output` is `failed` with recall 0 and counts in the mean.
- Verdict (spec §7): correctness holds when `cond.mean_recall >= base.mean_recall - 0.02`; efficiency holds when `cond.mean_tokens <= 0.75 * base.mean_tokens` or `cond.mean_tool_calls <= 0.75 * base.mean_tool_calls`. Compare with a 1e-9 tolerance so boundary values pass.
- Dirty check ignores paths under `.singularrag/` and `.serena/` (spec §2, §4).
- Run directory: `eval/runs/<YYYYMMDDTHHMMSSZ>-<label>/`, label default `run` (spec §6). `eval/runs/` is gitignored except `summary.md`.
- Copy: prompt and summary wording as given in the tasks, verbatim.
- No async runtime, no daemon, no concurrency.

---

## File structure

```
crates/singularrag-bench/
  Cargo.toml
  src/main.rs        # clap: run | score | parse; wires modules; nothing else
  src/stream.rs      # parse_stream(&str) -> Parsed; Tokens; ResultLine; McpServer
  src/score.rs       # normalise, score(...) -> Record; Record is the on-disk JSON
  src/config.rs      # RunConfig, Condition; load(path) resolves + validates
  src/summary.rs     # ConditionStats, verdict, render(...) -> markdown
  src/session.rs     # prompt_for, schema_for, command_args, run_session
  src/repo.rs        # head_commit, dirty_paths (git status --porcelain)
  src/run.rs         # RunOpts, run(): preflight, run dir, index, loop, resume, summary
  tests/fixtures/
    success.jsonl
    max_turns.jsonl
    no_answer.jsonl
    mcp_failed.jsonl
    fake-claude.sh   # replays $FAKE_CLAUDE_FIXTURE; answers --version
  tests/cli.rs       # parse/score subcommands against fixtures
  tests/run.rs       # end-to-end with fake claude in a temp git checkout
eval/tier2.toml
eval/conditions/singularrag.json
eval/conditions/serena.json
.gitignore           # + eval/runs/* ; !eval/runs/*/summary.md
Cargo.toml           # workspace member + chrono
README.md            # "Tier-two eval" section
```

---

### Task 1: Crate scaffold and the stream parser

**Files:**
- Modify: `Cargo.toml` (workspace members + `chrono` dependency)
- Create: `crates/singularrag-bench/Cargo.toml`, `crates/singularrag-bench/src/main.rs`, `crates/singularrag-bench/src/stream.rs`
- Create: `crates/singularrag-bench/tests/fixtures/{success,max_turns,no_answer,mcp_failed}.jsonl`
- Test: unit tests in `stream.rs`; `crates/singularrag-bench/tests/cli.rs` (`parse` subcommand)

**Interfaces:**
- Produces: `stream::Tokens { input, output, cache_creation, cache_read: u64 }` with `total()`, `Serialize + Deserialize + Default + Clone`; `stream::ResultLine { subtype: String, is_error: bool, num_turns: u64, duration_ms: u64, total_cost_usd: f64, tokens: Tokens, structured_output: Option<serde_json::Value> }`; `stream::McpServer { name: String, status: String }`; `stream::Parsed { model: Option<String>, claude_version: Option<String>, mcp_servers: Vec<McpServer>, tool_calls: BTreeMap<String, u64>, result: Option<ResultLine>, lines: usize, bad_lines: usize }`; `stream::parse_stream(text: &str) -> Parsed`.
- CLI: `singularrag-bench parse <stream.jsonl>` prints `Parsed` as pretty JSON.

- [ ] **Step 1: Workspace and crate manifests**

In the root `Cargo.toml`, change `members` to `["crates/singularrag-core", "crates/singularrag", "crates/singularrag-bench"]` and add to `[workspace.dependencies]`:
```toml
chrono = { version = "0.4", default-features = false, features = ["clock", "std"] }
```
Create `crates/singularrag-bench/Cargo.toml`:
```toml
[package]
name = "singularrag-bench"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
publish = false
description = "Tier-two eval harness: headless Claude Code sessions scored against the tier-one gold sets"

[[bin]]
name = "singularrag-bench"
path = "src/main.rs"

[dependencies]
singularrag-core = { path = "../singularrag-core" }
clap = { workspace = true }
anyhow = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
toml = { workspace = true }
chrono = { workspace = true }

[dev-dependencies]
assert_cmd = { workspace = true }
predicates = { workspace = true }
tempfile = { workspace = true }
```

- [ ] **Step 2: Fixtures**

Create `crates/singularrag-bench/tests/fixtures/success.jsonl` (one JSON object per line; these mirror a real `claude -p --output-format stream-json --verbose` session recorded on 2026-09-20 with identifiers replaced):
```jsonl
{"type":"system","subtype":"init","cwd":"/tmp/hono","session_id":"00000000-0000-0000-0000-000000000000","tools":["Read","Grep","Glob","mcp__singularrag__repo_map","mcp__singularrag__find_symbol","StructuredOutput"],"mcp_servers":[{"name":"singularrag","status":"connected"}],"model":"claude-fable-5-1","permissionMode":"dontAsk","claude_code_version":"2.1.261"}
{"type":"system","subtype":"thinking_tokens","estimated_tokens":12,"session_id":"00000000-0000-0000-0000-000000000000"}
{"type":"assistant","message":{"model":"claude-fable-5-1","id":"msg_01","type":"message","role":"assistant","content":[{"type":"thinking","thinking":"","signature":"x"}]},"session_id":"00000000-0000-0000-0000-000000000000"}
{"type":"assistant","message":{"model":"claude-fable-5-1","id":"msg_02","type":"message","role":"assistant","content":[{"type":"tool_use","id":"toolu_01","name":"Grep","input":{"pattern":"class Router","glob":"src/**/*.ts"}}],"stop_reason":"tool_use"},"session_id":"00000000-0000-0000-0000-000000000000"}
{"type":"user","message":{"role":"user","content":[{"tool_use_id":"toolu_01","type":"tool_result","content":"src/router.ts:10:export interface Router<T>"}]},"session_id":"00000000-0000-0000-0000-000000000000"}
{"type":"assistant","message":{"model":"claude-fable-5-1","id":"msg_03","type":"message","role":"assistant","content":[{"type":"tool_use","id":"toolu_02","name":"mcp__singularrag__repo_map","input":{"query":"where is request routing decided","budget_tokens":1024}},{"type":"tool_use","id":"toolu_03","name":"Grep","input":{"pattern":"match("}}],"stop_reason":"tool_use"},"session_id":"00000000-0000-0000-0000-000000000000"}
{"type":"user","message":{"role":"user","content":[{"tool_use_id":"toolu_02","type":"tool_result","content":"# singularrag · index b94252 · HEAD 098e119 · fresh · retrieval r_000001\nsrc/router.ts:\n   10  export interface Router<T>\n"},{"tool_use_id":"toolu_03","type":"tool_result","content":"src/router.ts:40"}]},"session_id":"00000000-0000-0000-0000-000000000000"}
{"type":"rate_limit_event","rate_limit_info":{"status":"allowed"},"session_id":"00000000-0000-0000-0000-000000000000"}
{"type":"assistant","message":{"model":"claude-fable-5-1","id":"msg_04","type":"message","role":"assistant","content":[{"type":"tool_use","id":"toolu_04","name":"StructuredOutput","input":{"symbols":["src/router.ts::Router"," ./src/router.ts::match ","src/hono-base.ts::Hono","not-a-symbol"]}}],"stop_reason":"tool_use"},"session_id":"00000000-0000-0000-0000-000000000000"}
{"type":"result","subtype":"success","is_error":false,"duration_ms":11419,"duration_api_ms":7754,"num_turns":3,"result":"{\"symbols\":[\"src/router.ts::Router\",\" ./src/router.ts::match \",\"src/hono-base.ts::Hono\",\"not-a-symbol\"]}","structured_output":{"symbols":["src/router.ts::Router"," ./src/router.ts::match ","src/hono-base.ts::Hono","not-a-symbol"]},"session_id":"00000000-0000-0000-0000-000000000000","total_cost_usd":0.1928845,"usage":{"input_tokens":34,"cache_creation_input_tokens":7606,"cache_read_input_tokens":6498,"output_tokens":776},"modelUsage":{"claude-fable-5-1":{"inputTokens":34,"outputTokens":776,"cacheReadInputTokens":6498,"cacheCreationInputTokens":7606,"costUSD":0.1928845}},"permission_denials":[]}
```

`max_turns.jsonl`:
```jsonl
{"type":"system","subtype":"init","cwd":"/tmp/hono","session_id":"1","tools":["Read","Grep","Glob","StructuredOutput"],"mcp_servers":[],"model":"claude-fable-5-1","permissionMode":"dontAsk","claude_code_version":"2.1.261"}
{"type":"assistant","message":{"model":"claude-fable-5-1","id":"m1","type":"message","role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"/tmp/hono/src/router.ts"}}]},"session_id":"1"}
{"type":"user","message":{"role":"user","content":[{"tool_use_id":"t1","type":"tool_result","content":"..."}]},"session_id":"1"}
{"type":"assistant","message":{"model":"claude-fable-5-1","id":"m2","type":"message","role":"assistant","content":[{"type":"tool_use","id":"t2","name":"Read","input":{"file_path":"/tmp/hono/src/compose.ts"}}]},"session_id":"1"}
{"type":"user","message":{"role":"user","content":[{"tool_use_id":"t2","type":"tool_result","content":"..."}]},"session_id":"1"}
{"type":"result","subtype":"error_max_turns","is_error":true,"duration_ms":30000,"duration_api_ms":25000,"num_turns":2,"session_id":"1","total_cost_usd":0.31,"usage":{"input_tokens":10,"cache_creation_input_tokens":4000,"cache_read_input_tokens":9000,"output_tokens":300},"permission_denials":[]}
```

`no_answer.jsonl`:
```jsonl
{"type":"system","subtype":"init","cwd":"/tmp/hono","session_id":"2","tools":["Read","Grep","Glob","StructuredOutput"],"mcp_servers":[],"model":"claude-fable-5-1","permissionMode":"dontAsk","claude_code_version":"2.1.261"}
{"type":"assistant","message":{"model":"claude-fable-5-1","id":"m1","type":"message","role":"assistant","content":[{"type":"text","text":"Routing is decided in src/router.ts."}]},"session_id":"2"}
{"type":"result","subtype":"success","is_error":false,"duration_ms":5000,"duration_api_ms":4000,"num_turns":1,"result":"Routing is decided in src/router.ts.","session_id":"2","total_cost_usd":0.05,"usage":{"input_tokens":5,"cache_creation_input_tokens":4000,"cache_read_input_tokens":0,"output_tokens":40},"permission_denials":[]}
```

`mcp_failed.jsonl`:
```jsonl
{"type":"system","subtype":"init","cwd":"/tmp/hono","session_id":"3","tools":["Read","Grep","Glob","StructuredOutput"],"mcp_servers":[{"name":"serena","status":"failed"}],"model":"claude-fable-5-1","permissionMode":"dontAsk","claude_code_version":"2.1.261"}
{"type":"assistant","message":{"model":"claude-fable-5-1","id":"m1","type":"message","role":"assistant","content":[{"type":"tool_use","id":"t1","name":"StructuredOutput","input":{"symbols":[]}}]},"session_id":"3"}
{"type":"result","subtype":"success","is_error":false,"duration_ms":4000,"duration_api_ms":3000,"num_turns":1,"result":"{\"symbols\":[]}","structured_output":{"symbols":[]},"session_id":"3","total_cost_usd":0.04,"usage":{"input_tokens":5,"cache_creation_input_tokens":4000,"cache_read_input_tokens":0,"output_tokens":30},"permission_denials":[]}
```

- [ ] **Step 3: Failing parser tests**

`crates/singularrag-bench/src/stream.rs`, tests at the bottom:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/").to_string() + name).unwrap()
    }

    #[test]
    fn success_stream_yields_model_servers_tool_counts_and_result() {
        let p = parse_stream(&fixture("success.jsonl"));
        assert_eq!(p.model.as_deref(), Some("claude-fable-5-1"));
        assert_eq!(p.claude_version.as_deref(), Some("2.1.261"));
        assert_eq!(p.mcp_servers.len(), 1);
        assert_eq!(p.mcp_servers[0].name, "singularrag");
        assert_eq!(p.mcp_servers[0].status, "connected");
        assert_eq!(p.tool_calls.get("Grep"), Some(&2));
        assert_eq!(p.tool_calls.get("mcp__singularrag__repo_map"), Some(&1));
        assert!(!p.tool_calls.contains_key("StructuredOutput"), "the answer call is not a tool call");
        let r = p.result.expect("result line");
        assert_eq!(r.subtype, "success");
        assert!(!r.is_error);
        assert_eq!(r.num_turns, 3);
        assert_eq!(r.duration_ms, 11419);
        assert!((r.total_cost_usd - 0.1928845).abs() < 1e-9);
        assert_eq!(r.tokens, Tokens { input: 34, output: 776, cache_creation: 7606, cache_read: 6498 });
        assert_eq!(r.tokens.total(), 34 + 776 + 7606 + 6498);
        let so = r.structured_output.expect("structured output");
        assert_eq!(so["symbols"].as_array().unwrap().len(), 4);
        assert_eq!(p.lines, 10);
        assert_eq!(p.bad_lines, 0);
    }

    #[test]
    fn max_turns_stream_has_error_result_without_answer() {
        let p = parse_stream(&fixture("max_turns.jsonl"));
        let r = p.result.expect("result");
        assert_eq!(r.subtype, "error_max_turns");
        assert!(r.is_error);
        assert!(r.structured_output.is_none());
        assert_eq!(p.tool_calls.get("Read"), Some(&2));
    }

    #[test]
    fn no_answer_stream_is_success_without_structured_output() {
        let p = parse_stream(&fixture("no_answer.jsonl"));
        let r = p.result.expect("result");
        assert_eq!(r.subtype, "success");
        assert!(r.structured_output.is_none());
        assert!(p.tool_calls.is_empty());
    }

    #[test]
    fn mcp_failed_stream_reports_server_status() {
        let p = parse_stream(&fixture("mcp_failed.jsonl"));
        assert_eq!(p.mcp_servers[0].status, "failed");
    }

    #[test]
    fn garbage_lines_are_counted_not_fatal() {
        let text = format!("not json\n{}\n\n", fixture("no_answer.jsonl").lines().last().unwrap());
        let p = parse_stream(&text);
        assert_eq!(p.bad_lines, 1);
        assert!(p.result.is_some());
        assert_eq!(p.lines, 2, "blank lines are skipped, not counted");
    }

    #[test]
    fn empty_input_has_no_result() {
        let p = parse_stream("");
        assert!(p.result.is_none());
        assert_eq!(p.lines, 0);
    }
}
```

- [ ] **Step 4: Run to see them fail**

Run: `cargo test -p singularrag-bench` — expected: compile error (module missing). Create `src/main.rs` with the module declaration and a placeholder CLI so it compiles, then confirm the tests fail on missing items.

- [ ] **Step 5: Implement `stream.rs`**

```rust
//! Parse a `claude -p --output-format stream-json --verbose` transcript (one JSON object per line).
//! Only three line kinds matter (spec §5): the init message, assistant tool calls, the result.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The four token counts exactly as Claude Code reports them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_creation: u64,
    pub cache_read: u64,
}

impl Tokens {
    /// The comparison metric: input + output + cache creation + cache read.
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_creation + self.cache_read
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultLine {
    pub subtype: String,
    pub is_error: bool,
    pub num_turns: u64,
    pub duration_ms: u64,
    pub total_cost_usd: f64,
    pub tokens: Tokens,
    pub structured_output: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpServer {
    pub name: String,
    pub status: String,
}

#[derive(Debug, Default, Serialize)]
pub struct Parsed {
    pub model: Option<String>,
    pub claude_version: Option<String>,
    pub mcp_servers: Vec<McpServer>,
    /// Tool calls by tool name. `StructuredOutput` is the answer mechanism and is never counted.
    pub tool_calls: BTreeMap<String, u64>,
    pub result: Option<ResultLine>,
    /// Non-blank lines seen, and how many of them were not JSON objects.
    pub lines: usize,
    pub bad_lines: usize,
}

const ANSWER_TOOL: &str = "StructuredOutput";

fn u64_of(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

pub fn parse_stream(text: &str) -> Parsed {
    let mut p = Parsed::default();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        p.lines += 1;
        let v: Value = match serde_json::from_str(line) {
            Ok(Value::Object(m)) => Value::Object(m),
            _ => {
                p.bad_lines += 1;
                continue;
            }
        };
        match (v.get("type").and_then(Value::as_str), v.get("subtype").and_then(Value::as_str)) {
            (Some("system"), Some("init")) => {
                p.model = v.get("model").and_then(Value::as_str).map(str::to_string);
                p.claude_version = v.get("claude_code_version").and_then(Value::as_str).map(str::to_string);
                if let Some(servers) = v.get("mcp_servers").and_then(Value::as_array) {
                    p.mcp_servers = servers
                        .iter()
                        .map(|s| McpServer {
                            name: s.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                            status: s.get("status").and_then(Value::as_str).unwrap_or("unknown").to_string(),
                        })
                        .collect();
                }
            }
            (Some("assistant"), _) => {
                let blocks = v.pointer("/message/content").and_then(Value::as_array);
                for b in blocks.into_iter().flatten() {
                    if b.get("type").and_then(Value::as_str) != Some("tool_use") {
                        continue;
                    }
                    let name = b.get("name").and_then(Value::as_str).unwrap_or("");
                    if name.is_empty() || name == ANSWER_TOOL {
                        continue;
                    }
                    *p.tool_calls.entry(name.to_string()).or_insert(0) += 1;
                }
            }
            (Some("result"), subtype) => {
                let usage = v.get("usage").cloned().unwrap_or(Value::Null);
                p.result = Some(ResultLine {
                    subtype: subtype.unwrap_or("unknown").to_string(),
                    is_error: v.get("is_error").and_then(Value::as_bool).unwrap_or(false),
                    num_turns: u64_of(&v, "num_turns"),
                    duration_ms: u64_of(&v, "duration_ms"),
                    total_cost_usd: v.get("total_cost_usd").and_then(Value::as_f64).unwrap_or(0.0),
                    tokens: Tokens {
                        input: u64_of(&usage, "input_tokens"),
                        output: u64_of(&usage, "output_tokens"),
                        cache_creation: u64_of(&usage, "cache_creation_input_tokens"),
                        cache_read: u64_of(&usage, "cache_read_input_tokens"),
                    },
                    structured_output: v.get("structured_output").cloned(),
                });
            }
            _ => {}
        }
    }
    p
}
```

- [ ] **Step 6: `main.rs` with the `parse` subcommand**

```rust
#![forbid(unsafe_code)]

use std::path::PathBuf;

use clap::{Parser, Subcommand};

mod stream;

#[derive(Parser, Debug)]
#[command(name = "singularrag-bench", version, about = "Tier-two eval: headless Claude Code sessions scored against the gold sets")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Print the parsed record of one stream-json transcript
    Parse { stream: PathBuf },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Parse { stream } => {
            let text = std::fs::read_to_string(&stream)?;
            let parsed = stream::parse_stream(&text);
            println!("{}", serde_json::to_string_pretty(&parsed)?);
        }
    }
    Ok(())
}
```

`crates/singularrag-bench/tests/cli.rs`:
```rust
use assert_cmd::Command;
use predicates::prelude::*;

fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn parse_prints_tool_counts_and_result() {
    Command::cargo_bin("singularrag-bench")
        .unwrap()
        .args(["parse", &fixture("success.jsonl")])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"mcp__singularrag__repo_map\": 1"))
        .stdout(predicate::str::contains("\"subtype\": \"success\""));
}

#[test]
fn parse_missing_file_is_an_error() {
    Command::cargo_bin("singularrag-bench")
        .unwrap()
        .args(["parse", "/nonexistent/stream.jsonl"])
        .assert()
        .failure();
}
```

- [ ] **Step 7: Run tests, lint, commit**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test -p singularrag-bench`
Expected: 6 unit + 2 integration tests pass.
```bash
git add -A
git commit -m "feat(bench): crate scaffold and stream-json parser with recorded fixtures"
```

---

### Task 2: Scoring

**Files:**
- Create: `crates/singularrag-bench/src/score.rs`
- Modify: `crates/singularrag-bench/src/main.rs` (`mod score;`)
- Test: unit tests in `score.rs`

**Interfaces:**
- Consumes: `stream::{Parsed, Tokens}`; `singularrag_core::eval::Question { id, category, query, gold: Vec<String> }`.
- Produces: `score::Record` (the on-disk `<qid>-<repeat>.json`, `Serialize + Deserialize`): `question, condition: String, repeat: u32, model: Option<String>, recall: f64, precision: f64, hit: Vec<String>, miss: Vec<String>, answer: Vec<String>, tool_calls: BTreeMap<String, u64>, tokens: Tokens, cost_usd: f64, turns: u64, duration_ms: u64, failed: bool, reason: Option<String>`; `score::normalise(symbols: &[String], answer_max: usize) -> Vec<String>`; `score::score(q: &Question, condition: &str, repeat: u32, parsed: &Parsed, answer_max: usize, spawn_error: Option<&str>) -> Record`; `impl Record { pub fn tool_call_total(&self) -> u64 }`.

- [ ] **Step 1: Failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::parse_stream;
    use singularrag_core::eval::Question;

    fn q(gold: &[&str]) -> Question {
        Question { id: "L1".into(), category: "locate".into(), query: "where is request routing decided".into(), gold: gold.iter().map(|s| s.to_string()).collect() }
    }
    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
    }
    fn s(v: &[&str]) -> Vec<String> { v.iter().map(|x| x.to_string()).collect() }

    #[test]
    fn normalise_trims_strips_dot_slash_drops_non_symbols_and_caps() {
        let out = normalise(&s(&[" src/a.ts::A ", "./src/b.ts::B", "src/c.ts", "", "src/d.ts::D"]), 2);
        assert_eq!(out, s(&["src/a.ts::A", "src/b.ts::B"]));
    }

    #[test]
    fn success_fixture_scores_recall_and_precision() {
        let p = parse_stream(&fixture("success.jsonl"));
        let r = score(&q(&["src/router.ts::Router", "src/router.ts::match", "src/hono-base.ts::Hono", "src/router/reg-exp-router/router.ts::RegExpRouter"]), "singularrag", 1, &p, 15, None);
        assert!(!r.failed);
        assert_eq!(r.answer, s(&["src/router.ts::Router", "src/router.ts::match", "src/hono-base.ts::Hono"]));
        assert_eq!(r.hit.len(), 3);
        assert_eq!(r.miss, s(&["src/router/reg-exp-router/router.ts::RegExpRouter"]));
        assert!((r.recall - 0.75).abs() < 1e-9);
        assert!((r.precision - 1.0).abs() < 1e-9);
        assert_eq!(r.tool_call_total(), 3);
        assert_eq!(r.turns, 3);
        assert_eq!(r.tokens.total(), 14914);
        assert_eq!(r.model.as_deref(), Some("claude-fable-5-1"));
        assert_eq!(r.condition, "singularrag");
        assert_eq!(r.repeat, 1);
    }

    #[test]
    fn max_turns_is_failed_with_zero_recall_and_keeps_costs() {
        let p = parse_stream(&fixture("max_turns.jsonl"));
        let r = score(&q(&["src/router.ts::Router"]), "alone", 1, &p, 15, None);
        assert!(r.failed);
        assert_eq!(r.reason.as_deref(), Some("result subtype error_max_turns"));
        assert_eq!(r.recall, 0.0);
        assert_eq!(r.miss, s(&["src/router.ts::Router"]));
        assert!((r.cost_usd - 0.31).abs() < 1e-9);
        assert_eq!(r.tool_call_total(), 2);
    }

    #[test]
    fn no_structured_output_is_failed() {
        let p = parse_stream(&fixture("no_answer.jsonl"));
        let r = score(&q(&["src/router.ts::Router"]), "alone", 1, &p, 15, None);
        assert!(r.failed);
        assert_eq!(r.reason.as_deref(), Some("no structured output"));
    }

    #[test]
    fn spawn_error_is_failed_with_that_reason() {
        let p = parse_stream("");
        let r = score(&q(&["src/router.ts::Router"]), "alone", 2, &p, 15, Some("exit status 1"));
        assert!(r.failed);
        assert_eq!(r.reason.as_deref(), Some("exit status 1"));
        assert_eq!(r.tokens, Tokens::default());
    }

    #[test]
    fn empty_gold_is_recall_one_and_empty_answer_is_precision_zero() {
        let p = parse_stream(&fixture("mcp_failed.jsonl"));
        let r = score(&q(&[]), "serena", 1, &p, 15, None);
        assert_eq!(r.recall, 1.0);
        assert_eq!(r.precision, 0.0);
        assert!(!r.failed);
    }

    #[test]
    fn record_round_trips_through_json() {
        let p = parse_stream(&fixture("success.jsonl"));
        let r = score(&q(&["src/router.ts::Router"]), "singularrag", 1, &p, 15, None);
        let text = serde_json::to_string(&r).unwrap();
        let back: Record = serde_json::from_str(&text).unwrap();
        assert_eq!(back.hit, r.hit);
        assert_eq!(back.tokens, r.tokens);
    }
}
```

- [ ] **Step 2: Run to see them fail** — `cargo test -p singularrag-bench score::` fails with unresolved items.

- [ ] **Step 3: Implement**

```rust
//! Score one session against its question's gold set (spec §5).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use singularrag_core::eval::Question;

use crate::stream::{Parsed, Tokens};

/// One session's record: what `<qid>-<repeat>.json` holds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub question: String,
    pub condition: String,
    pub repeat: u32,
    pub model: Option<String>,
    pub recall: f64,
    pub precision: f64,
    pub hit: Vec<String>,
    pub miss: Vec<String>,
    pub answer: Vec<String>,
    pub tool_calls: BTreeMap<String, u64>,
    pub tokens: Tokens,
    pub cost_usd: f64,
    pub turns: u64,
    pub duration_ms: u64,
    pub failed: bool,
    pub reason: Option<String>,
}

impl Record {
    pub fn tool_call_total(&self) -> u64 {
        self.tool_calls.values().sum()
    }
}

/// Trim, strip a leading `./`, drop entries without `::`, keep the first `answer_max`.
pub fn normalise(symbols: &[String], answer_max: usize) -> Vec<String> {
    symbols
        .iter()
        .map(|s| s.trim())
        .map(|s| s.strip_prefix("./").unwrap_or(s))
        .filter(|s| s.contains("::"))
        .map(str::to_string)
        .take(answer_max)
        .collect()
}

fn answer_of(parsed: &Parsed, answer_max: usize) -> Option<Vec<String>> {
    let so = parsed.result.as_ref()?.structured_output.as_ref()?;
    let raw: Vec<String> = so
        .get("symbols")?
        .as_array()?
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    Some(normalise(&raw, answer_max))
}

pub fn score(q: &Question, condition: &str, repeat: u32, parsed: &Parsed, answer_max: usize, spawn_error: Option<&str>) -> Record {
    let result = parsed.result.as_ref();
    let reason: Option<String> = if let Some(e) = spawn_error {
        Some(e.to_string())
    } else if let Some(r) = result {
        if r.is_error || r.subtype != "success" {
            Some(format!("result subtype {}", r.subtype))
        } else if r.structured_output.is_none() {
            Some("no structured output".to_string())
        } else {
            None
        }
    } else {
        Some("no result line".to_string())
    };
    let answer = if reason.is_none() { answer_of(parsed, answer_max).unwrap_or_default() } else { Vec::new() };
    let (hit, miss): (Vec<String>, Vec<String>) = q.gold.iter().cloned().partition(|g| answer.contains(g));
    let recall = if reason.is_some() {
        0.0
    } else if q.gold.is_empty() {
        1.0
    } else {
        hit.len() as f64 / q.gold.len() as f64
    };
    let precision = if answer.is_empty() { 0.0 } else { hit.len() as f64 / answer.len() as f64 };
    Record {
        question: q.id.clone(),
        condition: condition.to_string(),
        repeat,
        model: parsed.model.clone(),
        recall,
        precision,
        hit,
        miss,
        answer,
        tool_calls: parsed.tool_calls.clone(),
        tokens: result.map(|r| r.tokens.clone()).unwrap_or_default(),
        cost_usd: result.map(|r| r.total_cost_usd).unwrap_or(0.0),
        turns: result.map(|r| r.num_turns).unwrap_or(0),
        duration_ms: result.map(|r| r.duration_ms).unwrap_or(0),
        failed: reason.is_some(),
        reason,
    }
}
```
Note: when `reason` is set, `miss` must still list all gold (the partition against an empty answer does that).

- [ ] **Step 4: Tests green, lint, commit**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test -p singularrag-bench`
```bash
git add -A
git commit -m "feat(bench): score a session by gold recall and precision"
```

---
### Task 3: Run config

**Files:**
- Create: `crates/singularrag-bench/src/config.rs`
- Modify: `crates/singularrag-bench/src/main.rs` (`mod config;`)
- Test: unit tests in `config.rs` (tempfile)

**Interfaces:**
- Produces: `config::Condition { name: String, baseline: bool, mcp_config: Option<PathBuf> }` (absolute path after load); `config::RunConfig { repo: PathBuf, commit: String, questions: PathBuf, repeats: u32, max_turns: u32, max_budget_usd: f64, tools: Vec<String>, answer_max: usize, conditions: Vec<Condition> }` (`Serialize + Deserialize + Clone`; `repo` and `questions` absolute after load); `config::load(path: &Path) -> anyhow::Result<RunConfig>`; `impl RunConfig { pub fn baseline(&self) -> &Condition; pub fn select(&self, names: Option<&[String]>) -> anyhow::Result<Vec<Condition>> }`.
- Defaults when absent in the file: `repeats = 3`, `max_turns = 25`, `max_budget_usd = 0.5`, `tools = ["Read","Grep","Glob"]`, `answer_max = 15`, `questions = "questions.toml"`, `baseline = false`.

- [ ] **Step 1: Failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
        let p = dir.join("tier2.toml");
        std::fs::write(&p, body).unwrap();
        p
    }

    const GOOD: &str = r#"
repo = "../hono"
commit = "098e11912ab244c5c33931de007f04dc8e3c2929"
[[condition]]
name = "alone"
baseline = true
[[condition]]
name = "singularrag"
mcp_config = "conditions/singularrag.json"
"#;

    #[test]
    fn paths_resolve_relative_to_the_config_file_and_defaults_apply() {
        let dir = tempfile::tempdir().unwrap();
        // Canonical: `load` canonicalises the config directory, and macOS temp dirs sit behind /private.
        let base = std::fs::canonicalize(dir.path()).unwrap();
        let sub = base.join("eval");
        std::fs::create_dir_all(sub.join("conditions")).unwrap();
        std::fs::write(sub.join("conditions/singularrag.json"), "{}").unwrap();
        let cfg = load(&write(&sub, GOOD)).unwrap();
        assert_eq!(cfg.repo, base.join("hono"));
        assert_eq!(cfg.questions, sub.join("questions.toml"));
        assert_eq!(cfg.repeats, 3);
        assert_eq!(cfg.max_turns, 25);
        assert!((cfg.max_budget_usd - 0.5).abs() < 1e-9);
        assert_eq!(cfg.tools, vec!["Read", "Grep", "Glob"]);
        assert_eq!(cfg.answer_max, 15);
        assert_eq!(cfg.conditions[1].mcp_config.as_deref(), Some(sub.join("conditions/singularrag.json").as_path()));
        assert_eq!(cfg.baseline().name, "alone");
    }

    #[test]
    fn exactly_one_baseline_is_required() {
        let dir = tempfile::tempdir().unwrap();
        let none = GOOD.replace("baseline = true\n", "");
        let e = load(&write(dir.path(), &none)).unwrap_err().to_string();
        assert!(e.contains("exactly one condition must be the baseline"), "{e}");
        let two = GOOD.replace("mcp_config = \"conditions/singularrag.json\"", "baseline = true");
        let e = load(&write(dir.path(), &two)).unwrap_err().to_string();
        assert!(e.contains("exactly one condition must be the baseline"), "{e}");
    }

    #[test]
    fn duplicate_names_and_missing_mcp_config_files_are_errors() {
        let dir = tempfile::tempdir().unwrap();
        let dup = GOOD.replace("name = \"singularrag\"", "name = \"alone\"");
        let e = load(&write(dir.path(), &dup)).unwrap_err().to_string();
        assert!(e.contains("duplicate condition name alone"), "{e}");
        let e = load(&write(dir.path(), GOOD)).unwrap_err().to_string();
        assert!(e.contains("mcp_config not found"), "{e}");
    }

    #[test]
    fn select_filters_by_name_and_rejects_unknown() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("conditions")).unwrap();
        std::fs::write(dir.path().join("conditions/singularrag.json"), "{}").unwrap();
        let cfg = load(&write(dir.path(), GOOD)).unwrap();
        let sel = cfg.select(Some(&["singularrag".to_string()])).unwrap();
        assert_eq!(sel.len(), 1);
        assert_eq!(sel[0].name, "singularrag");
        assert_eq!(cfg.select(None).unwrap().len(), 2);
        let e = cfg.select(Some(&["nope".to_string()])).unwrap_err().to_string();
        assert!(e.contains("unknown condition nope"), "{e}");
    }
}
```

- [ ] **Step 2: Run to see them fail.**

- [ ] **Step 3: Implement**

```rust
//! `eval/tier2.toml` (spec §3): the checkout, the pinned commit, the conditions, the caps.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Condition {
    pub name: String,
    #[serde(default)]
    pub baseline: bool,
    /// Claude Code `mcpServers` JSON; absolute after `load`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_config: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunConfig {
    pub repo: PathBuf,
    pub commit: String,
    #[serde(default = "default_questions")]
    pub questions: PathBuf,
    #[serde(default = "default_repeats")]
    pub repeats: u32,
    #[serde(default = "default_max_turns")]
    pub max_turns: u32,
    #[serde(default = "default_max_budget")]
    pub max_budget_usd: f64,
    #[serde(default = "default_tools")]
    pub tools: Vec<String>,
    #[serde(default = "default_answer_max")]
    pub answer_max: usize,
    #[serde(rename = "condition")]
    pub conditions: Vec<Condition>,
}

fn default_questions() -> PathBuf { PathBuf::from("questions.toml") }
fn default_repeats() -> u32 { 3 }
fn default_max_turns() -> u32 { 25 }
fn default_max_budget() -> f64 { 0.5 }
fn default_tools() -> Vec<String> { ["Read", "Grep", "Glob"].map(String::from).to_vec() }
fn default_answer_max() -> usize { 15 }

fn absolutise(base: &Path, p: &Path) -> PathBuf {
    if p.is_absolute() { p.to_path_buf() } else { normalise_dots(&base.join(p)) }
}

/// Collapse `.` and `..` components without touching the filesystem (the checkout may not exist yet).
fn normalise_dots(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => { out.pop(); }
            other => out.push(other),
        }
    }
    out
}

pub fn load(path: &Path) -> Result<RunConfig> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut cfg: RunConfig = toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    let base = path.parent().unwrap_or(Path::new("."));
    let base = if base.as_os_str().is_empty() { Path::new(".") } else { base };
    let base = std::fs::canonicalize(base).unwrap_or_else(|_| base.to_path_buf());
    cfg.repo = absolutise(&base, &cfg.repo);
    cfg.questions = absolutise(&base, &cfg.questions);
    let baselines = cfg.conditions.iter().filter(|c| c.baseline).count();
    if baselines != 1 {
        bail!("exactly one condition must be the baseline (found {baselines})");
    }
    let mut seen = std::collections::BTreeSet::new();
    for c in &mut cfg.conditions {
        if !seen.insert(c.name.clone()) {
            bail!("duplicate condition name {}", c.name);
        }
        if let Some(m) = &c.mcp_config {
            let abs = absolutise(&base, m);
            if !abs.is_file() {
                bail!("condition {}: mcp_config not found: {}", c.name, abs.display());
            }
            c.mcp_config = Some(abs);
        }
    }
    if cfg.repeats == 0 { bail!("repeats must be at least 1"); }
    if cfg.answer_max == 0 { bail!("answer_max must be at least 1"); }
    Ok(cfg)
}

impl RunConfig {
    pub fn baseline(&self) -> &Condition {
        self.conditions.iter().find(|c| c.baseline).expect("validated in load")
    }

    /// The conditions to run, in config order; `names` narrows and must all exist.
    pub fn select(&self, names: Option<&[String]>) -> Result<Vec<Condition>> {
        match names {
            None => Ok(self.conditions.clone()),
            Some(names) => {
                for n in names {
                    if !self.conditions.iter().any(|c| &c.name == n) {
                        bail!("unknown condition {n}");
                    }
                }
                Ok(self.conditions.iter().filter(|c| names.contains(&c.name)).cloned().collect())
            }
        }
    }
}
```

- [ ] **Step 4: Tests green, lint, commit**

```bash
git add -A
git commit -m "feat(bench): run config with resolved paths and validation"
```

---

### Task 4: Summary, verdict, and the `score` subcommand

**Files:**
- Create: `crates/singularrag-bench/src/summary.rs`
- Modify: `crates/singularrag-bench/src/main.rs` (`mod summary;`, `Score { run_dir }` subcommand)
- Test: unit tests in `summary.rs`; `tests/cli.rs` gains a `score` test over a hand-built run directory

**Interfaces:**
- Consumes: `score::Record`.
- Produces: `summary::ConditionStats { name: String, sessions: usize, failed: usize, mean_recall: f64, mean_tokens: f64, median_tokens: f64, mean_tool_calls: f64, mean_wall_s: f64, total_cost: f64 }`; `summary::stats(name: &str, records: &[Record]) -> ConditionStats`; `summary::Verdict { correctness: Result<(), String>, efficiency: Result<(), String> }` with `fn earns(&self) -> bool`; `summary::verdict(base: &ConditionStats, cond: &ConditionStats) -> Verdict`; `summary::RunMeta { run_dir: String, commit: String, claude_version: String, singularrag_version: Option<String>, aborted: BTreeMap<String, String> }`; `summary::render(meta: &RunMeta, baseline: &str, conditions: &[(String, Vec<Record>)], question_ids: &[String]) -> String`; `summary::load_run(run_dir: &Path) -> anyhow::Result<(RunMeta, String, Vec<(String, Vec<Record>)>, Vec<String>)>` (reads `run.toml` and every `<condition>/*.json`; the `String` is the baseline name).
- `run.toml` shape (written by Task 6, read here):
  ```toml
  [run]
  created = "20260920T153000Z"
  label = "run"
  commit = "098e…"
  claude_version = "2.1.261"
  singularrag_version = "0.1.0"     # absent when no condition used singularrag
  baseline = "alone"
  conditions = ["alone", "singularrag"]
  questions = ["L1", "L2"]
  [run.aborted]                     # condition name -> reason; may be empty
  serena = "mcp server serena status failed"
  [config]
  # RunConfig as resolved (repo, commit, questions, repeats, max_turns, max_budget_usd, tools, answer_max, condition = [...])
  ```
  Define `summary::RunFile { run: RunMeta2, config: crate::config::RunConfig }` where `RunMeta2 { created, label, commit, claude_version, singularrag_version: Option<String>, baseline, conditions: Vec<String>, questions: Vec<String>, #[serde(default)] aborted: BTreeMap<String,String> }` (`Serialize + Deserialize`). Task 6 writes it with `toml::to_string_pretty`.

- [ ] **Step 1: Failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::score::Record;
    use crate::stream::Tokens;
    use std::collections::BTreeMap;

    fn rec(q: &str, cond: &str, recall: f64, tokens: u64, calls: u64, failed: bool) -> Record {
        let mut tool_calls = BTreeMap::new();
        if calls > 0 { tool_calls.insert("Grep".to_string(), calls); }
        Record {
            question: q.into(), condition: cond.into(), repeat: 1, model: Some("m".into()),
            recall, precision: 0.5, hit: vec![], miss: vec![], answer: vec![], tool_calls,
            tokens: Tokens { input: tokens, output: 0, cache_creation: 0, cache_read: 0 },
            cost_usd: 0.1, turns: 3, duration_ms: 2000, failed, reason: None,
        }
    }

    #[test]
    fn stats_are_means_medians_and_counts() {
        let rs = vec![rec("L1", "alone", 1.0, 100, 4, false), rec("L1", "alone", 0.5, 300, 2, false), rec("L2", "alone", 0.0, 800, 0, true)];
        let s = stats("alone", &rs);
        assert_eq!(s.sessions, 3);
        assert_eq!(s.failed, 1);
        assert!((s.mean_recall - 0.5).abs() < 1e-9);
        assert!((s.mean_tokens - 400.0).abs() < 1e-9);
        assert!((s.median_tokens - 300.0).abs() < 1e-9);
        assert!((s.mean_tool_calls - 2.0).abs() < 1e-9);
        assert!((s.mean_wall_s - 2.0).abs() < 1e-9);
        assert!((s.total_cost - 0.3).abs() < 1e-9);
    }

    #[test]
    fn median_of_even_count_averages_the_middle_pair() {
        let rs = vec![rec("L1", "a", 1.0, 100, 0, false), rec("L1", "a", 1.0, 200, 0, false), rec("L1", "a", 1.0, 1000, 0, false), rec("L1", "a", 1.0, 5000, 0, false)];
        assert!((stats("a", &rs).median_tokens - 600.0).abs() < 1e-9);
    }

    #[test]
    fn empty_stats_are_zero_not_nan() {
        let s = stats("x", &[]);
        assert_eq!(s.sessions, 0);
        assert_eq!(s.mean_recall, 0.0);
        assert_eq!(s.median_tokens, 0.0);
    }

    fn st(recall: f64, tokens: f64, calls: f64) -> ConditionStats {
        ConditionStats { name: "c".into(), sessions: 1, failed: 0, mean_recall: recall, mean_tokens: tokens, median_tokens: tokens, mean_tool_calls: calls, mean_wall_s: 1.0, total_cost: 0.1 }
    }

    #[test]
    fn verdict_boundaries_pass_and_just_beyond_fail() {
        let base = st(0.60, 1000.0, 10.0);
        assert!(verdict(&base, &st(0.58, 750.0, 10.0)).earns(), "recall at -0.02 and tokens at -25% pass");
        assert!(verdict(&base, &st(0.58, 1000.0, 7.5)).earns(), "tool calls at -25% pass");
        let v = verdict(&base, &st(0.579, 750.0, 10.0));
        assert!(v.correctness.is_err() && v.efficiency.is_ok() && !v.earns());
        let v = verdict(&base, &st(0.60, 751.0, 7.6));
        assert!(v.correctness.is_ok() && v.efficiency.is_err() && !v.earns());
        assert_eq!(v.efficiency.unwrap_err(), "tokens 751 vs 1000 (-24.9%), tool calls 7.6 vs 10.0 (-24.0%); need -25% on either");
    }

    #[test]
    fn render_has_header_tables_and_verdicts() {
        let meta = RunMeta { run_dir: "eval/runs/20260920T000000Z-run".into(), commit: "098e119".into(), claude_version: "2.1.261".into(), singularrag_version: Some("0.1.0".into()), aborted: BTreeMap::new() };
        let conds = vec![
            ("alone".to_string(), vec![rec("L1", "alone", 0.5, 1000, 10, false), rec("L2", "alone", 0.5, 1000, 10, false)]),
            ("singularrag".to_string(), vec![rec("L1", "singularrag", 1.0, 500, 4, false), rec("L2", "singularrag", 0.0, 500, 4, true)]),
        ];
        let md = render(&meta, "alone", &conds, &["L1".into(), "L2".into()]);
        assert!(md.contains("tokens = input + output + cache creation + cache read"));
        assert!(md.contains("| alone | 2 | 0 | 0.50 |"), "{md}");
        assert!(md.contains("| singularrag | 2 | 1 | 0.50 |"), "{md}");
        assert!(md.contains("| L1 | 0.50 | 1.00 |"), "{md}");
        assert!(md.contains("**singularrag earns its place**"), "{md}");
        assert!(md.contains("models: m"), "{md}");
    }

    #[test]
    fn render_names_the_failed_test_and_aborts() {
        let mut aborted = BTreeMap::new();
        aborted.insert("serena".to_string(), "mcp server serena status failed".to_string());
        let meta = RunMeta { run_dir: "r".into(), commit: "c".into(), claude_version: "v".into(), singularrag_version: None, aborted };
        let conds = vec![
            ("alone".to_string(), vec![rec("L1", "alone", 0.5, 1000, 10, false)]),
            ("serena".to_string(), vec![rec("L1", "serena", 0.2, 900, 9, false)]),
        ];
        let md = render(&meta, "alone", &conds, &["L1".into()]);
        assert!(md.contains("serena does not earn its place: correctness: recall 0.20 vs 0.50 (need >= 0.48); efficiency: tokens 900 vs 1000 (-10.0%), tool calls 9.0 vs 10.0 (-10.0%); need -25% on either"), "{md}");
        assert!(md.contains("aborted: serena (mcp server serena status failed)"), "{md}");
    }
}
```

- [ ] **Step 2: Run to see them fail.**

- [ ] **Step 3: Implement**

```rust
//! Aggregate records into per-condition stats, the §12 verdict, and `summary.md` (spec §7).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::RunConfig;
use crate::score::Record;

pub const RECALL_TOLERANCE: f64 = 0.02;
pub const EFFICIENCY_FACTOR: f64 = 0.75;
const EPS: f64 = 1e-9;

#[derive(Debug, Clone)]
pub struct ConditionStats {
    pub name: String,
    pub sessions: usize,
    pub failed: usize,
    pub mean_recall: f64,
    pub mean_tokens: f64,
    pub median_tokens: f64,
    pub mean_tool_calls: f64,
    pub mean_wall_s: f64,
    pub total_cost: f64,
}

fn mean(xs: impl Iterator<Item = f64>, n: usize) -> f64 {
    if n == 0 { 0.0 } else { xs.sum::<f64>() / n as f64 }
}

fn median(mut xs: Vec<f64>) -> f64 {
    if xs.is_empty() { return 0.0; }
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = xs.len();
    if n % 2 == 1 { xs[n / 2] } else { (xs[n / 2 - 1] + xs[n / 2]) / 2.0 }
}

pub fn stats(name: &str, records: &[Record]) -> ConditionStats {
    let n = records.len();
    ConditionStats {
        name: name.to_string(),
        sessions: n,
        failed: records.iter().filter(|r| r.failed).count(),
        mean_recall: mean(records.iter().map(|r| r.recall), n),
        mean_tokens: mean(records.iter().map(|r| r.tokens.total() as f64), n),
        median_tokens: median(records.iter().map(|r| r.tokens.total() as f64).collect()),
        mean_tool_calls: mean(records.iter().map(|r| r.tool_call_total() as f64), n),
        mean_wall_s: mean(records.iter().map(|r| r.duration_ms as f64 / 1000.0), n),
        total_cost: records.iter().map(|r| r.cost_usd).sum(),
    }
}

#[derive(Debug, Clone)]
pub struct Verdict {
    pub correctness: std::result::Result<(), String>,
    pub efficiency: std::result::Result<(), String>,
}

impl Verdict {
    pub fn earns(&self) -> bool {
        self.correctness.is_ok() && self.efficiency.is_ok()
    }
}

fn pct(cond: f64, base: f64) -> f64 {
    if base == 0.0 { 0.0 } else { (cond - base) / base * 100.0 }
}

pub fn verdict(base: &ConditionStats, cond: &ConditionStats) -> Verdict {
    let floor = base.mean_recall - RECALL_TOLERANCE;
    let correctness = if cond.mean_recall + EPS >= floor {
        Ok(())
    } else {
        Err(format!("recall {:.2} vs {:.2} (need >= {:.2})", cond.mean_recall, base.mean_recall, floor))
    };
    let tokens_ok = cond.mean_tokens <= EFFICIENCY_FACTOR * base.mean_tokens + EPS;
    let calls_ok = cond.mean_tool_calls <= EFFICIENCY_FACTOR * base.mean_tool_calls + EPS;
    let efficiency = if tokens_ok || calls_ok {
        Ok(())
    } else {
        Err(format!(
            "tokens {:.0} vs {:.0} ({:+.1}%), tool calls {:.1} vs {:.1} ({:+.1}%); need -25% on either",
            cond.mean_tokens, base.mean_tokens, pct(cond.mean_tokens, base.mean_tokens),
            cond.mean_tool_calls, base.mean_tool_calls, pct(cond.mean_tool_calls, base.mean_tool_calls)
        ))
    };
    Verdict { correctness, efficiency }
}

#[derive(Debug, Clone)]
pub struct RunMeta {
    pub run_dir: String,
    pub commit: String,
    pub claude_version: String,
    pub singularrag_version: Option<String>,
    pub aborted: BTreeMap<String, String>,
}

/// `run.toml` on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunFile {
    pub run: RunHeader,
    pub config: RunConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunHeader {
    pub created: String,
    pub label: String,
    pub commit: String,
    pub claude_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub singularrag_version: Option<String>,
    pub baseline: String,
    pub conditions: Vec<String>,
    pub questions: Vec<String>,
    #[serde(default)]
    pub aborted: BTreeMap<String, String>,
}

pub fn render(meta: &RunMeta, baseline: &str, conditions: &[(String, Vec<Record>)], question_ids: &[String]) -> String {
    let mut out = String::new();
    let mut models: Vec<String> = conditions.iter().flat_map(|(_, rs)| rs.iter().filter_map(|r| r.model.clone())).collect();
    models.sort();
    models.dedup();
    let _ = writeln!(out, "# Tier-two run {}\n", meta.run_dir);
    let _ = writeln!(out, "commit {} · claude {} · singularrag {} · models: {}", meta.commit, meta.claude_version, meta.singularrag_version.as_deref().unwrap_or("n/a"), if models.is_empty() { "none".to_string() } else { models.join(", ") });
    let _ = writeln!(out, "tokens = input + output + cache creation + cache read\n");
    for (name, reason) in &meta.aborted {
        let _ = writeln!(out, "aborted: {name} ({reason})");
    }
    if !meta.aborted.is_empty() { out.push('\n'); }

    let all: Vec<ConditionStats> = conditions.iter().map(|(n, rs)| stats(n, rs)).collect();
    let _ = writeln!(out, "| condition | sessions | failed | mean recall | mean tokens | median tokens | mean tool calls | mean wall s | total cost |");
    let _ = writeln!(out, "|---|---:|---:|---:|---:|---:|---:|---:|---:|");
    for s in &all {
        let _ = writeln!(out, "| {} | {} | {} | {:.2} | {:.0} | {:.0} | {:.1} | {:.1} | ${:.2} |", s.name, s.sessions, s.failed, s.mean_recall, s.mean_tokens, s.median_tokens, s.mean_tool_calls, s.mean_wall_s, s.total_cost);
    }
    out.push('\n');

    let _ = writeln!(out, "| question | {} |", conditions.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(" | "));
    let _ = writeln!(out, "|---|{}", "---:|".repeat(conditions.len()));
    for q in question_ids {
        let cells: Vec<String> = conditions
            .iter()
            .map(|(_, rs)| {
                let qs: Vec<&Record> = rs.iter().filter(|r| &r.question == q).collect();
                if qs.is_empty() { "–".to_string() } else { format!("{:.2}", mean(qs.iter().map(|r| r.recall), qs.len())) }
            })
            .collect();
        let _ = writeln!(out, "| {} | {} |", q, cells.join(" | "));
    }
    out.push('\n');

    match all.iter().find(|s| s.name == baseline) {
        None => { let _ = writeln!(out, "No verdict: baseline condition {baseline} has no records."); }
        Some(base) => {
            for s in all.iter().filter(|s| s.name != baseline) {
                let v = verdict(base, s);
                if v.earns() {
                    let _ = writeln!(out, "**{} earns its place** against {}.", s.name, baseline);
                } else {
                    let mut parts = Vec::new();
                    if let Err(e) = &v.correctness { parts.push(format!("correctness: {e}")); }
                    if let Err(e) = &v.efficiency { parts.push(format!("efficiency: {e}")); }
                    let _ = writeln!(out, "{} does not earn its place: {}", s.name, parts.join("; "));
                }
            }
        }
    }
    out
}

/// Read a run directory back: `run.toml` plus every `<condition>/<qid>-<n>.json`.
pub fn load_run(run_dir: &Path) -> Result<(RunMeta, String, Vec<(String, Vec<Record>)>, Vec<String>)> {
    let text = std::fs::read_to_string(run_dir.join("run.toml")).with_context(|| format!("reading {}", run_dir.join("run.toml").display()))?;
    let rf: RunFile = toml::from_str(&text).context("parsing run.toml")?;
    let mut conditions = Vec::new();
    for name in &rf.run.conditions {
        let dir = run_dir.join(name);
        let mut records = Vec::new();
        if dir.is_dir() {
            let mut entries: Vec<_> = std::fs::read_dir(&dir)?.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
            entries.sort();
            for p in entries {
                let r: Record = serde_json::from_str(&std::fs::read_to_string(&p)?).with_context(|| format!("parsing {}", p.display()))?;
                records.push(r);
            }
        }
        conditions.push((name.clone(), records));
    }
    let meta = RunMeta {
        run_dir: run_dir.display().to_string(),
        commit: rf.run.commit.clone(),
        claude_version: rf.run.claude_version.clone(),
        singularrag_version: rf.run.singularrag_version.clone(),
        aborted: rf.run.aborted.clone(),
    };
    Ok((meta, rf.run.baseline.clone(), conditions, rf.run.questions.clone()))
}
```

- [ ] **Step 4: `score` subcommand and its CLI test**

In `main.rs` add `Score { run_dir: PathBuf }` ("Rewrite summary.md from the records in a run directory") and:
```rust
Cmd::Score { run_dir } => {
    let (meta, baseline, conditions, questions) = summary::load_run(&run_dir)?;
    let md = summary::render(&meta, &baseline, &conditions, &questions);
    std::fs::write(run_dir.join("summary.md"), &md)?;
    print!("{md}");
}
```
Add to `tests/cli.rs`:
```rust
#[test]
fn score_rewrites_summary_from_records() {
    let dir = tempfile::tempdir().unwrap();
    let run = dir.path().join("20260920T000000Z-run");
    std::fs::create_dir_all(run.join("alone")).unwrap();
    std::fs::create_dir_all(run.join("singularrag")).unwrap();
    std::fs::write(run.join("run.toml"), r#"
[run]
created = "20260920T000000Z"
label = "run"
commit = "098e119"
claude_version = "2.1.261"
baseline = "alone"
conditions = ["alone", "singularrag"]
questions = ["L1"]
[config]
repo = "/tmp/hono"
commit = "098e119"
questions = "/tmp/q.toml"
[[config.condition]]
name = "alone"
baseline = true
[[config.condition]]
name = "singularrag"
"#).unwrap();
    let rec = |cond: &str, recall: f64, tokens: u64| format!(r#"{{"question":"L1","condition":"{cond}","repeat":1,"model":"m","recall":{recall},"precision":1.0,"hit":[],"miss":[],"answer":[],"tool_calls":{{"Grep":2}},"tokens":{{"input":{tokens},"output":0,"cache_creation":0,"cache_read":0}},"cost_usd":0.1,"turns":2,"duration_ms":1000,"failed":false,"reason":null}}"#);
    std::fs::write(run.join("alone/L1-1.json"), rec("alone", 0.5, 1000)).unwrap();
    std::fs::write(run.join("singularrag/L1-1.json"), rec("singularrag", 0.5, 500)).unwrap();
    Command::cargo_bin("singularrag-bench").unwrap().args(["score", run.to_str().unwrap()]).assert().success()
        .stdout(predicate::str::contains("**singularrag earns its place**"));
    assert!(run.join("summary.md").exists());
}
```
(`tempfile` is already a dev-dependency.)

- [ ] **Step 5: Tests green, lint, commit**

```bash
git add -A
git commit -m "feat(bench): per-condition stats, the section 12 verdict, summary.md, and score"
```

---

### Task 5: Session command and the git helpers

**Files:**
- Create: `crates/singularrag-bench/src/session.rs`, `crates/singularrag-bench/src/repo.rs`, `crates/singularrag-bench/tests/fixtures/fake-claude.sh`
- Modify: `crates/singularrag-bench/src/main.rs` (`mod session; mod repo;`)
- Test: unit tests in both files (the session spawn test uses the fake script; the repo tests use a temp git repo)

**Interfaces:**
- Produces: `session::prompt_for(query: &str, answer_max: usize) -> String`; `session::schema_for(answer_max: usize) -> String`; `session::SessionSpec { prompt: String, schema: String, tools: Vec<String>, mcp_config: Option<PathBuf>, max_turns: u32, max_budget_usd: f64 }`; `session::command_args(spec: &SessionSpec) -> Vec<String>` (everything after the program name); `session::claude_bin() -> String` (env `SINGULARRAG_BENCH_CLAUDE` or `claude`); `session::claude_version() -> anyhow::Result<String>`; `session::SessionOutcome { exit_code: Option<i32>, stderr_written: bool }`; `session::run_session(cwd: &Path, spec: &SessionSpec, stream_path: &Path, stderr_path: &Path) -> anyhow::Result<SessionOutcome>` (stdout streamed to `stream_path` as it arrives; stderr to `stderr_path` only if non-empty, else no file).
- `repo::head_commit(repo: &Path) -> anyhow::Result<String>`; `repo::dirty_paths(repo: &Path, ignore_prefixes: &[&str]) -> anyhow::Result<Vec<String>>` (from `git status --porcelain --untracked-files=all`, paths relative to the repo, excluding those starting with any prefix); `repo::IGNORED_PREFIXES: &[&str] = &[".singularrag/", ".serena/"]`.

- [ ] **Step 1: The fake claude script**

`crates/singularrag-bench/tests/fixtures/fake-claude.sh` (make it executable: `chmod +x`):
```sh
#!/bin/sh
# Stand-in for `claude` in tests. Replays $FAKE_CLAUDE_FIXTURE to stdout.
# $FAKE_CLAUDE_STDERR, when set, is written to stderr. $FAKE_CLAUDE_EXIT overrides the exit code.
# $FAKE_CLAUDE_TOUCH, when set, creates that file in the working directory (dirty-tree test).
# $FAKE_CLAUDE_ARGS_FILE, when set, receives the argument list one per line.
if [ "$1" = "--version" ]; then echo "9.9.9 (Claude Code)"; exit 0; fi
if [ -n "$FAKE_CLAUDE_ARGS_FILE" ]; then printf '%s\n' "$@" > "$FAKE_CLAUDE_ARGS_FILE"; fi
if [ -n "$FAKE_CLAUDE_TOUCH" ]; then : > "$FAKE_CLAUDE_TOUCH"; fi
if [ -n "$FAKE_CLAUDE_STDERR" ]; then echo "$FAKE_CLAUDE_STDERR" >&2; fi
if [ -n "$FAKE_CLAUDE_FIXTURE" ]; then cat "$FAKE_CLAUDE_FIXTURE"; fi
exit "${FAKE_CLAUDE_EXIT:-0}"
```

- [ ] **Step 2: Failing tests**

`session.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> SessionSpec {
        SessionSpec { prompt: prompt_for("where is request routing decided", 15), schema: schema_for(15), tools: vec!["Read".into(), "Grep".into(), "Glob".into()], mcp_config: Some(PathBuf::from("/tmp/mcp.json")), max_turns: 25, max_budget_usd: 0.5 }
    }

    #[test]
    fn prompt_carries_the_query_and_the_answer_contract() {
        let p = prompt_for("where is request routing decided", 15);
        assert!(p.starts_with("where is request routing decided\n\n"));
        assert!(p.contains("as `path::name`, where `path` is relative to the repository root"));
        assert!(p.contains("List at most 15, most important first."));
        assert!(p.ends_with("Use the tools available to you as you see fit."));
    }

    #[test]
    fn schema_is_an_object_with_a_capped_symbols_array() {
        let v: serde_json::Value = serde_json::from_str(&schema_for(7)).unwrap();
        assert_eq!(v["properties"]["symbols"]["maxItems"], 7);
        assert_eq!(v["required"][0], "symbols");
    }

    #[test]
    fn command_args_match_the_spec_list_in_order() {
        let args = command_args(&spec());
        let expected: Vec<String> = [
            "-p", &spec().prompt, "--output-format", "stream-json", "--verbose",
            "--json-schema", &spec().schema, "--tools", "Read,Grep,Glob",
            "--strict-mcp-config", "--mcp-config", "/tmp/mcp.json",
            "--setting-sources", "", "--disable-slash-commands",
            "--permission-mode", "dontAsk", "--permission-prompts", "none",
            "--no-session-persistence", "--max-turns", "25", "--max-budget-usd", "0.5",
        ].iter().map(|s| s.to_string()).collect();
        assert_eq!(args, expected);
    }

    #[test]
    fn no_mcp_config_means_no_mcp_flag() {
        let mut s = spec();
        s.mcp_config = None;
        let args = command_args(&s);
        assert!(!args.iter().any(|a| a == "--mcp-config"));
        assert!(args.iter().any(|a| a == "--strict-mcp-config"));
    }

}
```
`run_session` and `claude_version` read `SINGULARRAG_BENCH_CLAUDE`, which is process-global, so they are not unit-tested in-process (parallel test threads would race on the variable). Task 6's integration tests cover them through the `singularrag-bench` binary with the variable set per child process: the stream file line count, the stderr file, the non-zero exit, and the version string all get asserted there.

`repo.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let st = Command::new("git").args(args).current_dir(dir).env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t").env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t").status().unwrap();
        assert!(st.success());
    }

    fn repo_with_commit() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        git(d.path(), &["init", "-q"]);
        std::fs::write(d.path().join("a.txt"), "a").unwrap();
        git(d.path(), &["add", "."]);
        git(d.path(), &["commit", "-q", "-m", "init"]);
        d
    }

    #[test]
    fn head_commit_is_forty_hex() {
        let d = repo_with_commit();
        let h = head_commit(d.path()).unwrap();
        assert_eq!(h.len(), 40);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn dirty_paths_lists_modified_and_untracked_minus_ignored_prefixes() {
        let d = repo_with_commit();
        assert!(dirty_paths(d.path(), IGNORED_PREFIXES).unwrap().is_empty());
        std::fs::write(d.path().join("a.txt"), "b").unwrap();
        std::fs::create_dir_all(d.path().join(".singularrag")).unwrap();
        std::fs::write(d.path().join(".singularrag/index.db"), "x").unwrap();
        std::fs::write(d.path().join("new.txt"), "n").unwrap();
        let mut dirty = dirty_paths(d.path(), IGNORED_PREFIXES).unwrap();
        dirty.sort();
        assert_eq!(dirty, vec!["a.txt".to_string(), "new.txt".to_string()]);
    }

    #[test]
    fn not_a_repo_is_an_error() {
        let d = tempfile::tempdir().unwrap();
        assert!(head_commit(d.path()).is_err());
    }
}
```

- [ ] **Step 3: Run to see them fail.**

- [ ] **Step 4: Implement `session.rs`**

```rust
//! One headless Claude Code session (spec §4): the prompt, the schema, the exact command line,
//! and streaming its stdout to disk as it arrives.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};

pub const CLAUDE_ENV: &str = "SINGULARRAG_BENCH_CLAUDE";

pub fn claude_bin() -> String {
    std::env::var(CLAUDE_ENV).ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "claude".to_string())
}

pub fn claude_version() -> Result<String> {
    let out = Command::new(claude_bin()).arg("--version").output().with_context(|| format!("running {} --version", claude_bin()))?;
    anyhow::ensure!(out.status.success(), "{} --version failed", claude_bin());
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Identical across conditions (spec §4).
pub fn prompt_for(query: &str, answer_max: usize) -> String {
    format!(
        "{query}\n\nAnswer by listing the symbols that answer the question, as `path::name`, where `path` is relative to the repository root and `name` is the symbol's declared name. List at most {answer_max}, most important first. Use the tools available to you as you see fit."
    )
}

pub fn schema_for(answer_max: usize) -> String {
    serde_json::json!({
        "type": "object",
        "properties": { "symbols": { "type": "array", "items": { "type": "string" }, "maxItems": answer_max } },
        "required": ["symbols"]
    })
    .to_string()
}

#[derive(Debug, Clone)]
pub struct SessionSpec {
    pub prompt: String,
    pub schema: String,
    pub tools: Vec<String>,
    pub mcp_config: Option<PathBuf>,
    pub max_turns: u32,
    pub max_budget_usd: f64,
}

/// Everything after the program name, in the spec's order.
pub fn command_args(spec: &SessionSpec) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "-p".into(), spec.prompt.clone(),
        "--output-format".into(), "stream-json".into(), "--verbose".into(),
        "--json-schema".into(), spec.schema.clone(),
        "--tools".into(), spec.tools.join(","),
        "--strict-mcp-config".into(),
    ];
    if let Some(m) = &spec.mcp_config {
        a.push("--mcp-config".into());
        a.push(m.display().to_string());
    }
    a.extend([
        "--setting-sources".to_string(), String::new(), "--disable-slash-commands".to_string(),
        "--permission-mode".to_string(), "dontAsk".to_string(), "--permission-prompts".to_string(), "none".to_string(),
        "--no-session-persistence".to_string(),
        "--max-turns".to_string(), spec.max_turns.to_string(),
        "--max-budget-usd".to_string(), spec.max_budget_usd.to_string(),
    ]);
    a
}

#[derive(Debug)]
pub struct SessionOutcome {
    pub exit_code: Option<i32>,
    pub stderr_written: bool,
}

pub fn run_session(cwd: &Path, spec: &SessionSpec, stream_path: &Path, stderr_path: &Path) -> Result<SessionOutcome> {
    let mut child = Command::new(claude_bin())
        .args(command_args(spec))
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawning {}", claude_bin()))?;
    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");
    let mut file = std::fs::File::create(stream_path).with_context(|| format!("creating {}", stream_path.display()))?;
    let err_thread = std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = std::io::Read::read_to_string(&mut BufReader::new(stderr), &mut buf);
        buf
    });
    for line in BufReader::new(stdout).lines() {
        let line = line?;
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
        file.flush()?;
    }
    let status = child.wait()?;
    let err = err_thread.join().unwrap_or_default();
    let stderr_written = !err.trim().is_empty();
    if stderr_written {
        std::fs::write(stderr_path, err)?;
    }
    Ok(SessionOutcome { exit_code: status.code(), stderr_written })
}
```

- [ ] **Step 5: Implement `repo.rs`**

```rust
//! The two git facts the harness needs: HEAD, and whether the tree is clean.

use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};

/// Paths under these never count as dirt: singularrag and Serena keep their caches in the checkout.
pub const IGNORED_PREFIXES: &[&str] = &[".singularrag/", ".serena/"];

fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git").args(args).current_dir(repo).output().with_context(|| format!("running git {} in {}", args.join(" "), repo.display()))?;
    if !out.status.success() {
        bail!("git {} failed in {}: {}", args.join(" "), repo.display(), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

pub fn head_commit(repo: &Path) -> Result<String> {
    Ok(git(repo, &["rev-parse", "HEAD"])?.trim().to_string())
}

pub fn dirty_paths(repo: &Path, ignore_prefixes: &[&str]) -> Result<Vec<String>> {
    let out = git(repo, &["status", "--porcelain", "--untracked-files=all"])?;
    Ok(out
        .lines()
        .filter(|l| l.len() > 3)
        .map(|l| l[3..].trim().to_string())
        .filter(|p| !ignore_prefixes.iter().any(|pre| p.starts_with(pre)))
        .collect())
}
```

- [ ] **Step 6: Tests green, lint, commit**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test -p singularrag-bench`
```bash
git add -A
git commit -m "feat(bench): session command line and streaming spawn, git helpers, fake claude"
```

---
### Task 6: The run loop and the `run` subcommand

**Files:**
- Create: `crates/singularrag-bench/src/run.rs`, `crates/singularrag-bench/tests/run.rs`, `crates/singularrag-bench/tests/fixtures/fake-singularrag.sh`
- Modify: `crates/singularrag-bench/src/main.rs` (`mod run;`, the `Run` subcommand)
- Test: `tests/run.rs` end to end through the binary with the fake scripts

**Interfaces:**
- Consumes: `config::{load, RunConfig, Condition}`, `session::*`, `repo::*`, `stream::parse_stream`, `score::score`, `summary::{RunFile, RunHeader, load_run, render}`, `singularrag_core::eval::load_questions`.
- Produces: `run::RunOpts { config: PathBuf, label: String, conditions: Option<Vec<String>>, questions: Option<Vec<String>>, repeats: Option<u32>, dry_run: bool, resume: Option<PathBuf> }`; `run::run(opts: &RunOpts) -> anyhow::Result<Option<PathBuf>>` (`None` for a dry run, else the run directory); `run::SINGULARRAG_ENV = "SINGULARRAG_BENCH_SINGULARRAG"`.
- CLI: `singularrag-bench run [--config eval/tier2.toml] [--label run] [--conditions a,b] [--questions L1,T2] [--repeats N] [--dry-run] [--resume <run-dir>]` (`--conditions` and `--questions` are comma-separated; `value_delimiter = ','`).

- [ ] **Step 1: The fake singularrag script**

`crates/singularrag-bench/tests/fixtures/fake-singularrag.sh` (`chmod +x`):
```sh
#!/bin/sh
# Stand-in for `singularrag` in tests: answers --version, records an `index` call.
if [ "$1" = "--version" ]; then echo "singularrag 0.1.0-fake"; exit 0; fi
if [ "$1" = "index" ] && [ -n "$FAKE_SINGULARRAG_MARK" ]; then echo "$@" > "$FAKE_SINGULARRAG_MARK"; fi
exit 0
```

- [ ] **Step 2: Failing integration tests**

`crates/singularrag-bench/tests/run.rs`:
```rust
use std::path::{Path, PathBuf};
use std::process::Command as Std;

use assert_cmd::Command;
use predicates::prelude::*;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn git(dir: &Path, args: &[&str]) {
    let st = Std::new("git").args(args).current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t")
        .status().unwrap();
    assert!(st.success());
}

/// A workspace: `eval/tier2.toml`, two questions, one condition file, and a `hono/` checkout with one commit.
struct Ws {
    _dir: tempfile::TempDir,
    root: PathBuf,
    commit: String,
}

fn workspace() -> Ws {
    let dir = tempfile::tempdir().unwrap();
    // Canonical, because config::load canonicalises the config directory and macOS temp
    // paths go through a /private symlink.
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let hono = root.join("hono");
    std::fs::create_dir_all(hono.join("src")).unwrap();
    std::fs::write(hono.join("src/router.ts"), "export class Router {}\n").unwrap();
    git(&hono, &["init", "-q"]);
    git(&hono, &["add", "."]);
    git(&hono, &["commit", "-q", "-m", "init"]);
    let commit = String::from_utf8(Std::new("git").args(["rev-parse", "HEAD"]).current_dir(&hono).output().unwrap().stdout).unwrap().trim().to_string();
    let ev = root.join("eval");
    std::fs::create_dir_all(ev.join("conditions")).unwrap();
    std::fs::write(ev.join("questions.toml"), r#"
[[question]]
id = "L1"
category = "locate"
query = "where is request routing decided"
gold = ["src/router.ts::Router", "src/router.ts::match", "src/compose.ts::compose", "src/compose.ts::dispatch"]
[[question]]
id = "L2"
category = "locate"
query = "where are middleware chains composed"
gold = ["src/compose.ts::compose"]
"#).unwrap();
    std::fs::write(ev.join("conditions/singularrag.json"), r#"{ "mcpServers": { "singularrag": { "command": "singularrag", "args": ["mcp", "--repo", "<checkout>"] } } }"#).unwrap();
    std::fs::write(ev.join("tier2.toml"), format!(r#"
repo = "../hono"
commit = "{commit}"
repeats = 2
max_turns = 5
max_budget_usd = 0.25
[[condition]]
name = "alone"
baseline = true
[[condition]]
name = "singularrag"
mcp_config = "conditions/singularrag.json"
"#)).unwrap();
    Ws { _dir: dir, root, commit }
}

fn bench(ws: &Ws) -> Command {
    let mut c = Command::cargo_bin("singularrag-bench").unwrap();
    c.current_dir(&ws.root)
        .env("SINGULARRAG_BENCH_CLAUDE", fixtures().join("fake-claude.sh"))
        .env("SINGULARRAG_BENCH_SINGULARRAG", fixtures().join("fake-singularrag.sh"))
        .env("FAKE_CLAUDE_FIXTURE", fixtures().join("success.jsonl"))
        .env_remove("FAKE_CLAUDE_EXIT").env_remove("FAKE_CLAUDE_STDERR").env_remove("FAKE_CLAUDE_TOUCH");
    c
}

fn run_dirs(ws: &Ws) -> Vec<PathBuf> {
    let runs = ws.root.join("eval/runs");
    if !runs.exists() { return vec![]; }
    let mut v: Vec<PathBuf> = std::fs::read_dir(runs).unwrap().map(|e| e.unwrap().path()).collect();
    v.sort();
    v
}

#[test]
fn dry_run_prints_every_command_and_creates_nothing() {
    let ws = workspace();
    bench(&ws).args(["run", "--config", "eval/tier2.toml", "--dry-run", "--repeats", "1"]).assert().success()
        .stdout(predicate::str::contains("--strict-mcp-config").count(4))
        .stdout(predicate::str::contains("where is request routing decided"))
        .stdout(predicate::str::contains("dry run: 4 sessions"));
    assert!(run_dirs(&ws).is_empty());
}

#[test]
fn run_writes_records_run_toml_and_summary_and_indexes_once() {
    let ws = workspace();
    let mark = ws.root.join("index.mark");
    bench(&ws).env("FAKE_SINGULARRAG_MARK", &mark)
        .args(["run", "--config", "eval/tier2.toml", "--label", "t", "--repeats", "1"]).assert().success()
        .stdout(predicate::str::contains("| alone | 2 | 0 |"))
        .stdout(predicate::str::contains("singularrag"));
    let dirs = run_dirs(&ws);
    assert_eq!(dirs.len(), 1);
    let run = &dirs[0];
    assert!(run.file_name().unwrap().to_str().unwrap().ends_with("-t"));
    for c in ["alone", "singularrag"] {
        for q in ["L1", "L2"] {
            assert!(run.join(c).join(format!("{q}-1.json")).exists(), "{c}/{q}-1.json");
            let stream = std::fs::read_to_string(run.join(c).join(format!("{q}-1.stream.jsonl"))).unwrap();
            assert_eq!(stream.lines().count(), 10);
            assert!(!run.join(c).join(format!("{q}-1.stderr")).exists());
        }
    }
    let rec: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(run.join("alone/L1-1.json")).unwrap()).unwrap();
    assert_eq!(rec["recall"], 0.5);
    assert_eq!(rec["failed"], false);
    let toml = std::fs::read_to_string(run.join("run.toml")).unwrap();
    assert!(toml.contains("claude_version = \"9.9.9 (Claude Code)\""));
    assert!(toml.contains("singularrag_version = \"singularrag 0.1.0-fake\""));
    assert!(toml.contains(&format!("commit = \"{}\"", ws.commit)));
    assert!(toml.contains("conditions = [\"alone\", \"singularrag\"]"));
    assert!(run.join("summary.md").exists());
    let index_args = std::fs::read_to_string(&mark).expect("singularrag index was run");
    assert!(index_args.contains("index --repo"));
    let mcp = std::fs::read_to_string(run.join("singularrag/mcp.json")).unwrap();
    assert!(!mcp.contains("<checkout>"));
    assert!(mcp.contains(ws.root.join("hono").to_str().unwrap()));
}

#[test]
fn alone_only_run_does_not_index() {
    let ws = workspace();
    let mark = ws.root.join("index.mark");
    bench(&ws).env("FAKE_SINGULARRAG_MARK", &mark)
        .args(["run", "--config", "eval/tier2.toml", "--conditions", "alone", "--questions", "L1", "--repeats", "1"]).assert().success()
        .stdout(predicate::str::contains("No verdict").not());
    assert!(!mark.exists());
    let toml = std::fs::read_to_string(run_dirs(&ws)[0].join("run.toml")).unwrap();
    assert!(!toml.contains("singularrag_version"));
}

#[test]
fn wrong_commit_and_dirty_tree_are_hard_errors_before_any_session() {
    let ws = workspace();
    let cfg = ws.root.join("eval/tier2.toml");
    let text = std::fs::read_to_string(&cfg).unwrap();
    std::fs::write(&cfg, text.replace(&ws.commit, "0000000000000000000000000000000000000000")).unwrap();
    bench(&ws).args(["run", "--config", "eval/tier2.toml"]).assert().failure()
        .stderr(predicate::str::contains("HEAD is"));
    std::fs::write(&cfg, text).unwrap();
    std::fs::write(ws.root.join("hono/dirty.txt"), "x").unwrap();
    bench(&ws).args(["run", "--config", "eval/tier2.toml"]).assert().failure()
        .stderr(predicate::str::contains("dirty.txt"));
    assert!(run_dirs(&ws).is_empty());
}

#[test]
fn unknown_question_id_is_an_error() {
    let ws = workspace();
    bench(&ws).args(["run", "--config", "eval/tier2.toml", "--questions", "Z9"]).assert().failure()
        .stderr(predicate::str::contains("unknown question Z9"));
}

#[test]
fn a_session_that_dirties_the_tree_aborts_the_run_after_recording_it() {
    let ws = workspace();
    bench(&ws).env("FAKE_CLAUDE_TOUCH", "left-behind.txt")
        .args(["run", "--config", "eval/tier2.toml", "--repeats", "1"]).assert().failure()
        .stderr(predicate::str::contains("left-behind.txt"));
    let run = &run_dirs(&ws)[0];
    assert!(run.join("alone/L1-1.json").exists());
    assert!(!run.join("alone/L2-1.json").exists());
}

#[test]
fn nonzero_exit_and_stderr_make_a_failed_session_and_the_run_continues() {
    let ws = workspace();
    bench(&ws).env("FAKE_CLAUDE_EXIT", "3").env("FAKE_CLAUDE_STDERR", "boom")
        .args(["run", "--config", "eval/tier2.toml", "--conditions", "alone", "--repeats", "1"]).assert().success();
    let run = &run_dirs(&ws)[0];
    let rec: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(run.join("alone/L1-1.json")).unwrap()).unwrap();
    assert_eq!(rec["failed"], true);
    assert_eq!(rec["reason"], "exit status 3");
    assert_eq!(std::fs::read_to_string(run.join("alone/L1-1.stderr")).unwrap().trim(), "boom");
    assert!(run.join("alone/L2-1.json").exists());
}

#[test]
fn a_disconnected_mcp_server_aborts_that_condition() {
    let ws = workspace();
    bench(&ws).env("FAKE_CLAUDE_FIXTURE", fixtures().join("mcp_failed.jsonl"))
        .args(["run", "--config", "eval/tier2.toml", "--conditions", "singularrag", "--repeats", "1"]).assert().success()
        .stdout(predicate::str::contains("aborted: singularrag (mcp server serena status failed)"))
        .stdout(predicate::str::contains("No verdict"));
    let run = &run_dirs(&ws)[0];
    assert!(run.join("singularrag/L1-1.json").exists());
    assert!(!run.join("singularrag/L2-1.json").exists());
    let toml = std::fs::read_to_string(run.join("run.toml")).unwrap();
    assert!(toml.contains("[run.aborted]"));
}

#[test]
fn resume_skips_sessions_that_have_records() {
    let ws = workspace();
    bench(&ws).args(["run", "--config", "eval/tier2.toml", "--conditions", "alone", "--questions", "L1", "--repeats", "1"]).assert().success();
    let run = run_dirs(&ws).remove(0);
    let before = std::fs::read_to_string(run.join("alone/L1-1.json")).unwrap();
    bench(&ws).env("FAKE_CLAUDE_EXIT", "3")
        .args(["run", "--config", "eval/tier2.toml", "--conditions", "alone", "--repeats", "2", "--resume", run.to_str().unwrap()]).assert().success();
    assert_eq!(std::fs::read_to_string(run.join("alone/L1-1.json")).unwrap(), before, "existing record untouched");
    let rec: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(run.join("alone/L1-2.json")).unwrap()).unwrap();
    assert_eq!(rec["failed"], true, "the new session ran with the failing fake");
    assert_eq!(run_dirs(&ws).len(), 1, "resume creates no second directory");
}
```
Add `serde_json = { workspace = true }` under `[dev-dependencies]` if it is not already a regular dependency (it is; no change needed).

- [ ] **Step 3: Run to see them fail** (`cargo test -p singularrag-bench --test run` fails: no `run` subcommand).

- [ ] **Step 4: Implement `run.rs`**

```rust
//! One run (spec §2): preflight, run directory, optional index, the session loop with resume,
//! the dirty check after every session, condition abort on a disconnected MCP server, summary.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use singularrag_core::eval::{load_questions, Question};

use crate::config::{self, Condition, RunConfig};
use crate::repo;
use crate::score;
use crate::session::{self, SessionSpec};
use crate::stream;
use crate::summary::{self, RunFile, RunHeader};

pub const SINGULARRAG_ENV: &str = "SINGULARRAG_BENCH_SINGULARRAG";

#[derive(Debug, Clone)]
pub struct RunOpts {
    pub config: PathBuf,
    pub label: String,
    pub conditions: Option<Vec<String>>,
    pub questions: Option<Vec<String>>,
    pub repeats: Option<u32>,
    pub dry_run: bool,
    pub resume: Option<PathBuf>,
}

fn singularrag_bin() -> String {
    std::env::var(SINGULARRAG_ENV).ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "singularrag".to_string())
}

/// Does this condition's MCP config launch the `singularrag` command?
fn uses_singularrag(c: &Condition) -> Result<bool> {
    let Some(path) = &c.mcp_config else { return Ok(false) };
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path)?).with_context(|| format!("parsing {}", path.display()))?;
    let servers = v.get("mcpServers").and_then(|s| s.as_object());
    Ok(servers.into_iter().flatten().any(|(_, s)| {
        s.get("command").and_then(|c| c.as_str()).map(|c| Path::new(c).file_name().is_some_and(|f| f == "singularrag")).unwrap_or(false)
    }))
}

fn select_questions(all: Vec<Question>, ids: Option<&[String]>) -> Result<Vec<Question>> {
    match ids {
        None => Ok(all),
        Some(ids) => {
            for id in ids {
                if !all.iter().any(|q| &q.id == id) { bail!("unknown question {id}"); }
            }
            Ok(all.into_iter().filter(|q| ids.contains(&q.id)).collect())
        }
    }
}

fn spec_for(cfg: &RunConfig, q: &Question, mcp: Option<&Path>) -> SessionSpec {
    SessionSpec {
        prompt: session::prompt_for(&q.query, cfg.answer_max),
        schema: session::schema_for(cfg.answer_max),
        tools: cfg.tools.clone(),
        mcp_config: mcp.map(Path::to_path_buf),
        max_turns: cfg.max_turns,
        max_budget_usd: cfg.max_budget_usd,
    }
}

fn shell_quote(s: &str) -> String {
    if s.is_empty() { return "''".into(); }
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./,=:".contains(c)) { s.to_string() } else { format!("'{}'", s.replace('\'', "'\\''")) }
}

fn preflight(cfg: &RunConfig) -> Result<String> {
    if !cfg.repo.is_dir() { bail!("checkout not found: {}", cfg.repo.display()); }
    let head = repo::head_commit(&cfg.repo)?;
    if head != cfg.commit { bail!("checkout HEAD is {head}, config pins {}", cfg.commit); }
    let dirty = repo::dirty_paths(&cfg.repo, repo::IGNORED_PREFIXES)?;
    if !dirty.is_empty() { bail!("checkout is dirty: {}", dirty.join(", ")); }
    session::claude_version()
}

fn version_of_singularrag() -> Result<String> {
    let out = Command::new(singularrag_bin()).arg("--version").output().with_context(|| format!("running {} --version", singularrag_bin()))?;
    anyhow::ensure!(out.status.success(), "{} --version failed", singularrag_bin());
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn write_run_file(run_dir: &Path, rf: &RunFile) -> Result<()> {
    std::fs::write(run_dir.join("run.toml"), toml::to_string_pretty(rf)?)?;
    Ok(())
}

fn dirty_after(cfg: &RunConfig, what: &str) -> Result<()> {
    let dirty = repo::dirty_paths(&cfg.repo, repo::IGNORED_PREFIXES)?;
    if !dirty.is_empty() { bail!("checkout is dirty after {what}: {}; aborting the run", dirty.join(", ")); }
    Ok(())
}

pub fn run(opts: &RunOpts) -> Result<Option<PathBuf>> {
    let mut cfg = config::load(&opts.config)?;
    if let Some(r) = opts.repeats { cfg.repeats = r.max(1); }
    let questions = select_questions(load_questions(&cfg.questions).with_context(|| format!("reading {}", cfg.questions.display()))?, opts.questions.as_deref())?;
    let conditions = cfg.select(opts.conditions.as_deref())?;
    let baseline = cfg.baseline().name.clone();

    if opts.dry_run {
        let mut n = 0;
        for c in &conditions {
            for q in &questions {
                for _ in 1..=cfg.repeats {
                    let spec = spec_for(&cfg, q, c.mcp_config.as_deref());
                    let args: Vec<String> = session::command_args(&spec).iter().map(|a| shell_quote(a)).collect();
                    println!("[{}/{}] (cd {} && {} {})", c.name, q.id, shell_quote(&cfg.repo.display().to_string()), session::claude_bin(), args.join(" "));
                    n += 1;
                }
            }
        }
        println!("dry run: {n} sessions");
        return Ok(None);
    }

    let claude_version = preflight(&cfg)?;
    let needs_index = conditions.iter().map(uses_singularrag).collect::<Result<Vec<_>>>()?.into_iter().any(|b| b);
    let singularrag_version = if needs_index { Some(version_of_singularrag()?) } else { None };

    let (run_dir, mut rf) = match &opts.resume {
        Some(dir) => {
            let text = std::fs::read_to_string(dir.join("run.toml")).with_context(|| format!("resume: reading {}", dir.join("run.toml").display()))?;
            let rf: RunFile = toml::from_str(&text).context("resume: parsing run.toml")?;
            (dir.clone(), rf)
        }
        None => {
            let created = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
            let runs_root = opts.config.parent().map(Path::to_path_buf).unwrap_or_default().join("runs");
            let run_dir = runs_root.join(format!("{created}-{}", opts.label));
            std::fs::create_dir_all(&run_dir).with_context(|| format!("creating {}", run_dir.display()))?;
            let rf = RunFile {
                run: RunHeader {
                    created,
                    label: opts.label.clone(),
                    commit: cfg.commit.clone(),
                    claude_version,
                    singularrag_version,
                    baseline: baseline.clone(),
                    conditions: conditions.iter().map(|c| c.name.clone()).collect(),
                    questions: questions.iter().map(|q| q.id.clone()).collect(),
                    aborted: BTreeMap::new(),
                },
                config: cfg.clone(),
            };
            write_run_file(&run_dir, &rf)?;
            (run_dir, rf)
        }
    };

    if needs_index {
        let st = Command::new(singularrag_bin()).args(["index", "--repo"]).arg(&cfg.repo).status().with_context(|| format!("running {} index", singularrag_bin()))?;
        if !st.success() { bail!("{} index failed with {st}", singularrag_bin()); }
        dirty_after(&cfg, "singularrag index")?;
    }

    let checkout = cfg.repo.display().to_string();
    for c in &conditions {
        if rf.run.aborted.contains_key(&c.name) { continue; }
        let cdir = run_dir.join(&c.name);
        std::fs::create_dir_all(&cdir)?;
        let mcp = match &c.mcp_config {
            Some(src) => {
                let text = std::fs::read_to_string(src)?.replace("<checkout>", &checkout);
                let dst = cdir.join("mcp.json");
                std::fs::write(&dst, text)?;
                Some(dst)
            }
            None => None,
        };
        'condition: for q in &questions {
            for repeat in 1..=cfg.repeats {
                let base = cdir.join(format!("{}-{repeat}", q.id));
                let record_path = base.with_extension("json");
                if record_path.exists() { continue; }
                let stream_path = cdir.join(format!("{}-{repeat}.stream.jsonl", q.id));
                let stderr_path = cdir.join(format!("{}-{repeat}.stderr", q.id));
                eprintln!("[{}/{}#{repeat}] running", c.name, q.id);
                let spec = spec_for(&cfg, q, mcp.as_deref());
                let outcome = session::run_session(&cfg.repo, &spec, &stream_path, &stderr_path)?;
                let parsed = stream::parse_stream(&std::fs::read_to_string(&stream_path)?);
                let spawn_error = match outcome.exit_code {
                    Some(0) => None,
                    Some(code) => Some(format!("exit status {code}")),
                    None => Some("killed by signal".to_string()),
                };
                let record = score::score(q, &c.name, repeat, &parsed, cfg.answer_max, spawn_error.as_deref());
                std::fs::write(&record_path, serde_json::to_string_pretty(&record)?)?;
                eprintln!("[{}/{}#{repeat}] recall {:.2} · {} tokens · {} tool calls{}", c.name, q.id, record.recall, record.tokens.total(), record.tool_call_total(), if record.failed { " · FAILED" } else { "" });
                dirty_after(&cfg, &format!("{}/{}#{repeat}", c.name, q.id))?;
                if let Some(bad) = parsed.mcp_servers.iter().find(|s| s.status != "connected") {
                    let reason = format!("mcp server {} status {}", bad.name, bad.status);
                    eprintln!("[{}] aborted: {reason}", c.name);
                    rf.run.aborted.insert(c.name.clone(), reason);
                    write_run_file(&run_dir, &rf)?;
                    break 'condition;
                }
            }
        }
    }

    let (meta, baseline, conds, qids) = summary::load_run(&run_dir)?;
    let md = summary::render(&meta, &baseline, &conds, &qids);
    std::fs::write(run_dir.join("summary.md"), &md)?;
    print!("{md}");
    Ok(Some(run_dir))
}
```

- [ ] **Step 5: The `run` subcommand**

In `main.rs`:
```rust
/// Run the tier-two eval: one headless Claude Code session per condition, question and repeat
Run {
    #[arg(long, default_value = "eval/tier2.toml")]
    config: PathBuf,
    #[arg(long, default_value = "run")]
    label: String,
    /// Comma-separated condition names (default: all in the config)
    #[arg(long, value_delimiter = ',')]
    conditions: Option<Vec<String>>,
    /// Comma-separated question ids (default: all)
    #[arg(long, value_delimiter = ',')]
    questions: Option<Vec<String>>,
    #[arg(long)]
    repeats: Option<u32>,
    /// Print every command line and exit without touching the checkout
    #[arg(long)]
    dry_run: bool,
    /// Continue a run directory, skipping sessions that already have a record
    #[arg(long, value_name = "RUN_DIR")]
    resume: Option<PathBuf>,
},
```
and the arm:
```rust
Cmd::Run { config, label, conditions, questions, repeats, dry_run, resume } => {
    let opts = run::RunOpts { config, label, conditions, questions, repeats, dry_run, resume };
    if let Some(dir) = run::run(&opts)? {
        eprintln!("run written to {}", dir.display());
    }
}
```

- [ ] **Step 6: Tests green, lint, commit**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test --workspace`
Expected: all previous tests plus 9 in `tests/run.rs` pass.
```bash
git add -A
git commit -m "feat(bench): the run loop with preflight, resume, dirty check, and condition abort"
```

---

### Task 7: Config files, gitignore, docs, and the smoke run

**Files:**
- Create: `eval/tier2.toml`, `eval/conditions/singularrag.json`, `eval/conditions/serena.json`
- Modify: `.gitignore`, `README.md`, `eval/README.md`
- Create: `crates/singularrag-bench/tests/fixtures/smoke.jsonl` (recorded, sanitised) and one parser test over it

- [ ] **Step 1: The committed config**

`eval/tier2.toml`:
```toml
# Tier-two eval (spec docs/superpowers/specs/2026-09-20-singularrag-tier2-design.md).
# Run: cargo run -p singularrag-bench -- run --config eval/tier2.toml
repo = "../../hono"                # a honojs/hono checkout at `commit`, next to this repo
commit = "098e11912ab244c5c33931de007f04dc8e3c2929"
questions = "questions.toml"
repeats = 3
max_turns = 25
max_budget_usd = 0.50
tools = ["Read", "Grep", "Glob"]
answer_max = 15

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
`eval/conditions/singularrag.json`:
```json
{ "mcpServers": { "singularrag": { "command": "singularrag", "args": ["mcp", "--repo", "<checkout>"] } } }
```
`eval/conditions/serena.json`:
```json
{ "mcpServers": { "serena": { "command": "uvx", "args": ["--from", "git+https://github.com/oraios/serena", "serena", "start-mcp-server", "--context", "claude-code", "--project", "<checkout>"] } } }
```

- [ ] **Step 2: gitignore**

Append to `.gitignore`:
```
eval/runs/*
!eval/runs/*/
eval/runs/*/*
!eval/runs/*/summary.md
```
Verify: `mkdir -p eval/runs/x && touch eval/runs/x/summary.md eval/runs/x/a.json && git status --short eval/` shows only `eval/runs/x/summary.md`; then `rm -r eval/runs/x`.

- [ ] **Step 3: Docs**

Append to `README.md` after the "See what the agent was given" section:
````markdown
## Does it earn its place? (tier-two eval)

`singularrag-bench` runs headless Claude Code over the tier-one questions under named conditions and prints the verdict from the design spec: correctness not worse than Claude Code alone, and tokens or tool calls down by at least 25%.

```sh
cargo install --path crates/singularrag            # `singularrag` must be on PATH
git clone https://github.com/honojs/hono ../hono && git -C ../hono checkout 098e11912ab244c5c33931de007f04dc8e3c2929
cargo run -p singularrag-bench -- run --questions L1 --repeats 1 --conditions alone,singularrag   # smoke, a few dollars
cargo run -p singularrag-bench -- run                                                            # full: 12 × 3 × 3 = 108 sessions
```

Results land in `eval/runs/<timestamp>-<label>/` with one record and raw stream per session and a `summary.md` (the only file committed). `--dry-run` prints the commands, `--resume <dir>` continues an interrupted run, `score <dir>` rewrites the summary. The Serena condition needs `uvx`; a condition whose MCP server does not connect is aborted and reported, not run degraded. Sessions use `--setting-sources ""` and `--strict-mcp-config`, so your own hooks, plugins and MCP servers stay out of every condition.
````
Append to `eval/README.md` a "## Tier two" section with the smoke run's summary table pasted verbatim (Step 5) and the sentence "Fixtures under `crates/singularrag-bench/tests/fixtures/` are recorded streams with identifiers removed."

- [ ] **Step 4: Build the real binaries and dry-run**

```bash
cargo install --path crates/singularrag --locked
ls ../../hono/.git >/dev/null && git -C ../../hono rev-parse HEAD     # must print 098e119…; clone/checkout if not
cargo run -p singularrag-bench -- run --dry-run --questions L1 --repeats 1
```
Expected: three command lines (one per condition) with the L1 query, `--strict-mcp-config`, `--setting-sources ''`; "dry run: 3 sessions".

- [ ] **Step 5: The smoke run (costs money; ask before running if you are not the session owner)**

```bash
cargo run -p singularrag-bench -- run --label smoke --questions L1 --repeats 1 --conditions alone,singularrag
```
Expected: two sessions, both recorded, `summary.md` printed with a verdict line. Check `eval/runs/*-smoke/singularrag/L1-1.json`: `tool_calls` should show whether `mcp__singularrag__repo_map` was called; note it in the finish message either way (the spec deliberately does not steer the agent). If the singularrag condition reports its MCP server as not connected, check `singularrag mcp --repo ../../hono` starts by hand before anything else.

Then record the fixture: copy `eval/runs/*-smoke/singularrag/L1-1.stream.jsonl` to `crates/singularrag-bench/tests/fixtures/smoke.jsonl` and sanitise with:
```bash
sed -E -i '' -e 's/"session_id":"[^"]*"/"session_id":"0"/g' -e 's/"uuid":"[^"]*"/"uuid":"0"/g' -e 's#"cwd":"[^"]*"#"cwd":"/tmp/hono"#g' -e 's/"signature":"[^"]*"/"signature":"x"/g' crates/singularrag-bench/tests/fixtures/smoke.jsonl
grep -c "$HOME" crates/singularrag-bench/tests/fixtures/smoke.jsonl   # must be 0; if not, replace the remaining paths by hand
```
Add to `stream.rs` tests:
```rust
#[test]
fn recorded_smoke_stream_parses_to_a_result_with_an_answer() {
    let p = parse_stream(&fixture("smoke.jsonl"));
    assert_eq!(p.bad_lines, 0);
    let r = p.result.expect("result");
    assert_eq!(r.subtype, "success");
    assert!(r.structured_output.is_some());
    assert!(p.model.is_some());
}
```
Paste the smoke `summary.md` tables into `eval/README.md` under "## Tier two".

- [ ] **Step 6: Lint, full tests, commit**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test --workspace`
```bash
git add -A
git commit -m "feat(bench): tier-two config, conditions, docs, and the recorded smoke fixture"
```

---

## Self-review notes

- Spec §2 process model: preflight, run dir, conditional index, sequential loop, incremental records, resume (Task 6). §3 config and both condition files with `<checkout>` substitution (Tasks 3, 6, 7). §4 prompt, schema, exact command line incl. `--setting-sources ""` and `--disable-slash-commands`, stream to file, stderr only when non-empty, dirty check after each session, StructuredOutput excluded from counts (Tasks 1, 5, 6). §5 parsing of the three line kinds, four token numbers, normalisation, recall, precision, failed rules (Tasks 1, 2). §6 layout, `run.toml`, gitignore (Tasks 4, 6, 7). §7 summary tables, verdict thresholds with tolerance, `score` subcommand (Task 4). §8 failures: hard errors before any session, per-condition abort, per-session failure, no verdict without baseline (Tasks 4, 6). §9 CLI: all three subcommands and every flag (Tasks 1, 4, 6). §10 non-goals: nothing here adds concurrency, a grader, other CLIs, or a prompt hint. §11 testing: fixtures, scorer tables, fake claude end to end, CI unchanged (Tasks 1–7). §12 crates (Task 1). §13 cost note in README (Task 7).
- Placeholder scan: none. Every code step has its code; the only content produced at execution time is the recorded smoke fixture and the pasted summary, both with exact commands.
- Type consistency: `Tokens`/`ResultLine`/`Parsed` (Task 1) used by `score` (2) and `summary` (4); `Record` field names in Task 2 match the JSON literal in Task 4's CLI test and Task 6's assertions (`recall`, `failed`, `reason`, `tokens.input`); `RunFile`/`RunHeader` (Task 4) written by Task 6 and asserted textually in `tests/run.rs` (`claude_version`, `singularrag_version`, `conditions`, `[run.aborted]`); `select` (Task 3) and `select_questions` (Task 6) error strings match their tests; `command_args` order (Task 5) matches the Global Constraints list; `run()` returns `Option<PathBuf>` and `main.rs` matches.
- Known uncertainties with in-task fallbacks: `toml::to_string_pretty` of a `BTreeMap` inside `[run]` renders as `[run.aborted]` (asserted in Task 6; if the crate renders it inline, change the assertion to `aborted = {` and keep the semantics); the real stream may carry line kinds not in the fixtures (the parser ignores unknown kinds by design); whether `--tools` leaves MCP tools available is verified by the smoke run's init `tools` list (fallback: `--disallowedTools` for every built-in except Read, Grep, Glob).
