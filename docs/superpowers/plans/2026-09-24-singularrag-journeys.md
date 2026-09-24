# singularrag journeys (sub-project 3) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Handbook-style sections become processes with ordered steps (role, systems, documenting section, implementing code), served to the agent through `entities`, seeded into `repo_map`, and shown to the human in a Journeys view.

**Architecture:** Two new entity types (`process`, `role`) ride the existing extraction. A second prompt, gated on a `process` entity and staged on the existing queue, produces steps that a new `journeys` module resolves against the section's own entities and stores in three new tables (schema v6). "Implemented by" is derived at read time from the section's code mentions and system-name matches; nothing about code links is stored. `entities` renders steps, `repo_map` gains an `implements` seed, `serve` gains `/api/processes`, the UI gains a Journeys view, and the eval learns to load the checked-in extraction and run `entities` so the "cites gold" gate is executable.

**Tech Stack:** Rust (rusqlite, serde, reqwest::blocking, the in-crate fake Ollama), axum, React + TypeScript with Bun (bun test, happy-dom, axe-core).

**Spec:** `docs/superpowers/specs/2026-09-24-singularrag-journeys-design.md` (parent chain: `2026-09-19-singularrag-design.md`, `2026-09-23-singularrag-documents-design.md`, `2026-09-23-singularrag-knowledge-design.md`).

## Global Constraints

- Schema version 6; every schema change goes through the existing drop-and-rebuild in `store/mod.rs::init` (no migrations).
- `ENTITY_TYPES` is the closed list `person, organisation, system, concept, event, place, document, process, role`; unknown types still map to `concept`.
- The steps prompt runs only for a section whose own extraction produced at least one `process` entity; never in the one-shot CLI; same 30 s timeout, no timeout retry inside a tick (`tick_client`), one strict retry on malformed JSON, every failure `Error::ModelUnavailable`.
- Steps per section capped at 30; step text at 300 characters; role text at 80; names at `NAME_MAX` (80); all through `models::clean`.
- "Implemented by" is derived, never stored: section mentions first, then system-name matches for systems whose `norm_name` has at least 3 characters; at most 3 shown per step with `(+N)` for the rest.
- No new MCP tool. `INSTRUCTIONS` and every tool description in `crates/singularrag/src/mcp/server.rs` stay byte-identical to their copies in `crates/singularrag/tests/mcp.rs` (the `#[tool(description = …)]` literal, the `pub const`, and the test-local `const` are all edited together).
- `GET /api/processes` sits inside the `api` router before `.fallback` (token + host layers, `no-store`), reads through `locked(&s, …)`, caps at 200 processes and 30 steps each, and never writes.
- UI: the Journeys view is native HTML (`<ul>`, `<ol>`, `<button>`, `<select>`), every control labelled, colour never the only signal, axe clean with a process selected and with a role filter applied; Bun only.
- Gate before every commit: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`; UI tasks also `cd ui && bun run typecheck && bun test && bun run build`.
- The Bash guard rejects command lines containing the substring `eval`: use globs (`e*/README.md`, `e*/questions-prose.toml`) or a script file in the scratchpad.

## Review Focus

1. The steps answer names a process that is not among the section's `process` entities (the model paraphrased "Expense process" as "Expenses"): the answer is discarded, the section is marked done, nothing is retried and nothing is inserted. Test in Task 3 (`an_unmatched_process_name_discards_the_answer_and_finishes_the_section`).
2. Ollama goes down after the entity prompt succeeded and before the steps prompt: the queue row stays at stage 1, the header still says `entities: 1 pending`, and the next tick asks only the steps prompt. Test in Task 3 (`an_outage_between_the_prompts_resumes_at_the_steps_stage`).
3. A system named `IT` or `HR` (two characters) must never turn every `it`-containing symbol into an "implemented by" link. Test in Task 4 (`a_two_character_system_matches_no_code`).
4. A section documenting two processes at once (a heading "Expenses and travel" whose extraction has two `process` entities): the steps answer names one of them; the other keeps no steps and is still an ordinary entity. Test in Task 3 (`a_section_with_two_processes_keeps_steps_only_for_the_named_one`).
5. A workspace with no processes yet and 12 pending sections: the Journeys view says so with the pending count, and the role filter is disabled rather than empty. Test in Task 8 (`the journeys view explains an empty corpus`).

---

## File structure

- `crates/singularrag-core/src/models.rs` — `ENTITY_TYPES` (+2), `EXTRACT_PROMPT` type line, `STEPS_PROMPT`, `StepsAnswer`/`ExtractedStep`, `normalise_steps`, `Models::steps`, shared `parse_json`.
- `crates/singularrag-core/src/fake_ollama.rs` — `set_steps`, steps-prompt routing, `/api/generate:steps` call entries.
- `crates/singularrag-core/src/store/schema.rs` — v6: `stage` on `extract_queue`; `steps`, `step_systems`, `steps_cache`.
- `crates/singularrag-core/src/journeys.rs` (new) — resolution and storage (`apply_steps`, `steps_for_process`, `systems_for_step`, `delete_steps_for_symbols`), the derived rule (`CodeIndex`, `implemented_by`, `CodeLink`, `Via`), `ProcessSteps` read model, `render_steps`.
- `crates/singularrag-core/src/knowledge.rs` — stage handling in `extract_step`/`write_section`, `steps_stage`, `steps_cache`, deletion hooks, loader `steps` block, `render_entities` step lines, `seeds_for` implements seed.
- `crates/singularrag-core/src/rank.rs` — `Reasons.implements`, the `implements` personalization arm.
- `crates/singularrag-core/src/engine.rs` — `entities` serves code links as items; `Engine::load_extraction_json`.
- `crates/singularrag-core/src/eval.rs` — `cited` per question for `entity`/`relation`/`process` categories.
- `crates/singularrag/src/main.rs` — `eval --extraction <path>`.
- `crates/singularrag/src/mcp/server.rs`, `crates/singularrag/tests/mcp.rs` — description and INSTRUCTIONS text.
- `crates/singularrag/src/serve/{mod.rs,routes.rs,queries.rs}` — `/api/processes`.
- `ui/src/api/types.ts`, `ui/src/api/client.ts`, `ui/src/components/ViewToggle.tsx`, `ui/src/components/JourneysView.tsx` (new), `ui/src/App.tsx`, tests.
- `crates/singularrag-core/fixtures/prose/extraction.json`, `eval/questions-prose.toml`, `README.md`, `eval/README.md`, parent spec.

---

### Task 1: Types, the steps prompt, the fake

**Files:**
- Modify: `crates/singularrag-core/src/models.rs`
- Modify: `crates/singularrag-core/src/fake_ollama.rs`
- Test: unit tests in both files

**Interfaces:**
- Consumes: `Models::generate`, `split_parts`, `clean`, `parse_extraction`, `SectionInput`, `FakeOllama::set_extraction`.
- Produces: `pub const STEPS_PROMPT: &str`, `pub const STEPS_PROMPT_FIRST_LINE: &str = "You list the steps of a process"`, `pub const MAX_STEPS: usize = 30`, `pub const STEP_TEXT_MAX: usize = 300`, `pub const ROLE_MAX: usize = 80`, `pub struct StepsAnswer { pub process: String, pub steps: Vec<ExtractedStep> }`, `pub struct ExtractedStep { pub text: String, pub role: String, pub systems: Vec<String> }` (both `Debug, Clone, Default, PartialEq, Serialize, Deserialize`; `role` and `systems` `#[serde(default)]`), `pub fn normalise_steps(a: StepsAnswer) -> StepsAnswer`, `pub fn parse_steps(text: &str) -> Option<StepsAnswer>`, `Models::steps(&self, input: &SectionInput) -> Result<StepsAnswer>`, `FakeOllama::set_steps(&self, heading_substring: &str, answer: Value)`, and `calls()` entries `"/api/generate:steps"` for steps prompts (entity prompts keep `"/api/generate"`).

- [ ] **Step 1: Write the failing tests** (in `models.rs` `mod tests`)

```rust
#[test]
fn the_type_list_has_process_and_role_and_the_prompt_defines_them() {
    assert!(ENTITY_TYPES.contains(&"process") && ENTITY_TYPES.contains(&"role"));
    assert!(EXTRACT_PROMPT.contains("process (a named sequence of steps people follow)"));
    assert!(EXTRACT_PROMPT.contains("role (a job or position that acts in a process)"));
    assert!(STEPS_PROMPT.starts_with(STEPS_PROMPT_FIRST_LINE));
}

#[test]
fn normalise_steps_cleans_truncates_and_drops_empty_steps() {
    let mut steps: Vec<ExtractedStep> = (0..40)
        .map(|i| ExtractedStep { text: format!("step {i}"), role: " Manager\u{0} ".into(), systems: vec!["Okta".into(), "".into()] })
        .collect();
    steps.push(ExtractedStep { text: "   ".into(), role: "".into(), systems: vec![] });
    let a = normalise_steps(StepsAnswer { process: " Expense process ".into(), steps });
    assert_eq!(a.process, "Expense process");
    assert_eq!(a.steps.len(), MAX_STEPS);
    assert_eq!(a.steps[0].role, "Manager");
    assert_eq!(a.steps[0].systems, vec!["Okta".to_string()]);
    let long = normalise_steps(StepsAnswer { process: "p".into(), steps: vec![ExtractedStep { text: "x".repeat(400), role: "r".repeat(100), systems: vec![] }] });
    assert_eq!(long.steps[0].text.chars().count(), STEP_TEXT_MAX);
    assert_eq!(long.steps[0].role.chars().count(), ROLE_MAX);
}

#[test]
fn parse_steps_accepts_fences_and_missing_optional_fields() {
    let a = parse_steps("```json\n{\"process\": \"Onboarding\", \"steps\": [{\"text\": \"Activate Okta\"}]}\n```").unwrap();
    assert_eq!(a.process, "Onboarding");
    assert_eq!(a.steps[0].role, "");
    assert!(a.steps[0].systems.is_empty());
    assert!(parse_steps("not json").is_none());
}

#[test]
fn steps_asks_once_per_part_and_retries_strictly_on_malformed_json() {
    let fake = FakeOllama::spawn(8);
    fake.set_steps("Expense", serde_json::json!({"process": "Expense process", "steps": [{"text": "Submit in Expensify", "role": "employee", "systems": ["Expensify"]}]}));
    let m = Models::new(&ModelsConfig { ollama: fake.url(), ..ModelsConfig::default() }).unwrap();
    let input = SectionInput { path: "docs/handbook.md", heading: "Expense process", text: "Submit each expense in Expensify." };
    let a = m.steps(&input).unwrap();
    assert_eq!(a.steps.len(), 1);
    assert_eq!(fake.calls().iter().filter(|c| *c == "/api/generate:steps").count(), 1);
    assert_eq!(fake.calls().iter().filter(|c| *c == "/api/generate").count(), 0);
    fake.fail_next_generate(1);
    let a = m.steps(&input).unwrap();
    assert_eq!(a.steps.len(), 1, "one strict retry recovers");
    assert_eq!(fake.calls().iter().filter(|c| *c == "/api/generate:steps").count(), 3);
    fake.fail_next_generate(2);
    assert!(matches!(m.steps(&input), Err(Error::ModelUnavailable(_))));
}
```

(Use the same `Models` constructor and `ModelsConfig` the existing `models.rs` tests use; if `ModelsConfig` has no `Default`, build it the way `fenced_json_and_quotes_are_parsed` does.)

- [ ] **Step 2: Run** — `cargo test -p singularrag-core models::tests` → compile errors on the missing constants, types and `set_steps`.

- [ ] **Step 3: Implement**

`models.rs`:

```rust
pub const ENTITY_TYPES: [&str; 9] = [
    "person", "organisation", "system", "concept", "event", "place", "document", "process", "role",
];
pub const MAX_STEPS: usize = 30;
pub const STEP_TEXT_MAX: usize = 300;
pub const ROLE_MAX: usize = 80;
```

In `EXTRACT_PROMPT` replace the type line with:

```
Types are exactly one of: person, organisation, system, concept, event, place, document, process (a named sequence of steps people follow), role (a job or position that acts in a process).\n\
```

Add:

```rust
pub const STEPS_PROMPT_FIRST_LINE: &str = "You list the steps of a process";
pub const STEPS_PROMPT: &str = "You list the steps of a process described in one section of a document.\n\
Return ONLY a JSON object of the shape {\"process\": string, \"steps\": [{\"text\": string, \"role\": string, \"systems\": [string]}]}, steps in reading order.\n\
\"process\" is the name of the process the section describes, as the text names it. Each step's \"text\" is one short sentence from the text; \"role\" is who acts, or an empty string; \"systems\" are the tools or systems the step touches, by the names the text uses.\n\
Document: {path}\nSection: {heading}\n\nText:\n{text}\n";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ExtractedStep {
    pub text: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub systems: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StepsAnswer {
    #[serde(default)]
    pub process: String,
    #[serde(default)]
    pub steps: Vec<ExtractedStep>,
}

pub fn normalise_steps(a: StepsAnswer) -> StepsAnswer {
    let steps = a
        .steps
        .into_iter()
        .filter_map(|s| {
            let text = clean(&s.text, STEP_TEXT_MAX);
            if text.is_empty() {
                return None;
            }
            let systems = s.systems.iter().map(|x| clean(x, NAME_MAX)).filter(|x| !x.is_empty()).collect();
            Some(ExtractedStep { text, role: clean(&s.role, ROLE_MAX), systems })
        })
        .take(MAX_STEPS)
        .collect();
    StepsAnswer { process: clean(&a.process, NAME_MAX), steps }
}

/// Shared by `parse_extraction` and `parse_steps`: strips a code fence, then deserialises.
fn parse_json<T: serde::de::DeserializeOwned>(text: &str) -> Option<T> {
    let t = text.trim();
    let t = t.strip_prefix("```json").or_else(|| t.strip_prefix("```")).unwrap_or(t);
    let t = t.strip_suffix("```").unwrap_or(t).trim();
    serde_json::from_str(t).ok()
}

pub fn parse_steps(text: &str) -> Option<StepsAnswer> {
    parse_json(text)
}
```

Refactor `parse_extraction` to call `parse_json::<Extraction>` (keep its existing behaviour for prose around the JSON if it has any: read it first and move that logic into `parse_json`).

`impl Models`:

```rust
/// The steps prompt for one section: one call per part, one strict retry per part on malformed JSON.
pub fn steps(&self, input: &SectionInput) -> Result<StepsAnswer> {
    let mut merged = StepsAnswer::default();
    for part in split_parts(input.text) {
        let prompt = STEPS_PROMPT
            .replace("{path}", input.path)
            .replace("{heading}", input.heading)
            .replace("{text}", &part);
        let first = self.generate(&prompt)?;
        let parsed = match parse_steps(&first) {
            Some(a) => a,
            None => {
                let second = self.generate(&format!("{prompt}{STRICT_SUFFIX}"))?;
                parse_steps(&second).ok_or_else(|| unavailable(format!("steps: not JSON after retry: {}", clean(&second, 120))))?
            }
        };
        if merged.process.is_empty() {
            merged.process = parsed.process;
        }
        merged.steps.extend(parsed.steps);
    }
    Ok(normalise_steps(merged))
}
```

`fake_ollama.rs`: add `steps: Mutex<Vec<(String, Value)>>` to `State`, `set_steps` shaped exactly like `set_extraction`, and in the `/api/generate` arm:

```rust
let prompt = req["prompt"].as_str().unwrap_or("");
let is_steps = prompt.starts_with(crate::models::STEPS_PROMPT_FIRST_LINE);
lock(&st.calls).push(if is_steps { "/api/generate:steps".into() } else { "/api/generate".into() });
```

(Move the existing `calls` push for generate to this line so a generate call is recorded once.) Then, when not failing: the section line lookup as today, but against `st.steps` when `is_steps`, with the default answer `json!({"process": "", "steps": []})`.

- [ ] **Step 4: Run** — `cargo test -p singularrag-core` → all pass, including every existing `fake_ollama` and `knowledge` test (they count `"/api/generate"` and see no change).

- [ ] **Step 5: Commit** — `feat(core): process and role entity types, the steps prompt, fake steps answers`

---

### Task 2: Schema v6 and the journeys store module

**Files:**
- Modify: `crates/singularrag-core/src/store/schema.rs`
- Create: `crates/singularrag-core/src/journeys.rs`
- Modify: `crates/singularrag-core/src/lib.rs` (`pub mod journeys;`)
- Modify: `crates/singularrag-core/src/knowledge.rs` (`delete_for_symbols` and the hash-mismatch cleanup call the new delete)
- Test: unit tests in `journeys.rs`

**Interfaces:**
- Consumes: `knowledge::norm_name`, `Store::conn()`, `StepsAnswer` (Task 1), the `entities`/`entity_mentions` tables.
- Produces:
  - `pub struct StepRow { pub id: i64, pub process_id: i64, pub ordinal: i64, pub text: String, pub role_id: Option<i64>, pub role_text: String, pub symbol_id: i64 }`
  - `pub fn apply_steps(conn: &Connection, symbol_id: i64, hash: &str, a: &StepsAnswer) -> Result<Option<usize>>` — `Ok(None)` when no `process` entity of this section matches `a.process` by `norm_name`; otherwise replaces this section's steps and returns `Some(count)`.
  - `pub fn steps_for_process(conn: &Connection, process_id: i64) -> Result<Vec<StepRow>>` ordered by the documenting section's `line_start`, then `ordinal`.
  - `pub fn systems_for_step(conn: &Connection, step_id: i64) -> Result<Vec<(i64, String)>>` (entity id, name), ordered by name.
  - `pub fn role_name(conn: &Connection, step: &StepRow) -> Result<String>` — the role entity's name when `role_id` is set, else `role_text`.
  - `pub fn delete_steps_for_symbols(conn: &Connection, symbol_ids_sql: &str, param: i64) -> Result<()>` — deletes `step_systems` then `steps` for `symbol_id IN (<sql>)`.
  - `pub fn delete_steps_for_symbol(conn: &Connection, symbol_id: i64) -> Result<()>`.

- [ ] **Step 1: Write the failing tests** (`journeys.rs` `mod tests`; build a store with `Store::open` in a tempdir, insert a file and two symbols the way `knowledge.rs` tests do — copy its `section_fixture`-style helper if one exists, otherwise insert rows directly)

```rust
fn seed_section(conn: &Connection, path: &str, name: &str, line: i64) -> i64 { /* INSERT files + symbols(kind 'section') and sections_fts row; return symbol id */ }
fn seed_entity(conn: &Connection, symbol_id: i64, name: &str, ty: &str) -> i64 { /* INSERT entities (norm_name via knowledge::norm_name) + entity_mentions(symbol_id, 'h') ; return id */ }

#[test]
fn apply_steps_resolves_process_role_and_systems_from_the_sections_own_entities() {
    let (_dir, store) = temp_store();
    let conn = store.conn();
    let sec = seed_section(conn, "docs/handbook.md", "Expense process", 10);
    let process = seed_entity(conn, sec, "Expense process", "process");
    let manager = seed_entity(conn, sec, "Manager", "role");
    let expensify = seed_entity(conn, sec, "Expensify", "system");
    let other_sec = seed_section(conn, "docs/other.md", "Elsewhere", 1);
    let _okta_elsewhere = seed_entity(conn, other_sec, "Okta", "system");
    let a = StepsAnswer { process: "expense process".into(), steps: vec![
        ExtractedStep { text: "Submit in Expensify".into(), role: "Employee".into(), systems: vec!["Expensify".into(), "Okta".into()] },
        ExtractedStep { text: "Manager approves".into(), role: "manager".into(), systems: vec![] },
    ]};
    assert_eq!(apply_steps(conn, sec, "h", &a).unwrap(), Some(2));
    let rows = steps_for_process(conn, process).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!((rows[0].ordinal, rows[0].role_id, rows[0].role_text.as_str()), (1, None, "Employee"));
    assert_eq!((rows[1].ordinal, rows[1].role_id), (2, Some(manager)));
    assert_eq!(systems_for_step(conn, rows[0].id).unwrap(), vec![(expensify, "Expensify".to_string())], "Okta belongs to another section and is dropped");
    assert_eq!(role_name(conn, &rows[1]).unwrap(), "Manager");
    // re-applying replaces, never duplicates
    assert_eq!(apply_steps(conn, sec, "h", &a).unwrap(), Some(2));
    assert_eq!(steps_for_process(conn, process).unwrap().len(), 2);
}

#[test]
fn an_unmatched_process_name_returns_none_and_writes_nothing() {
    let (_dir, store) = temp_store();
    let conn = store.conn();
    let sec = seed_section(conn, "docs/handbook.md", "Expense process", 10);
    let process = seed_entity(conn, sec, "Expense process", "process");
    let a = StepsAnswer { process: "Expenses".into(), steps: vec![ExtractedStep { text: "x".into(), ..Default::default() }] };
    assert_eq!(apply_steps(conn, sec, "h", &a).unwrap(), None);
    assert!(steps_for_process(conn, process).unwrap().is_empty());
}

#[test]
fn steps_merge_across_sections_in_document_order() {
    let (_dir, store) = temp_store();
    let conn = store.conn();
    let later = seed_section(conn, "docs/handbook.md", "Travel", 40);
    let earlier = seed_section(conn, "docs/handbook.md", "Expense process", 10);
    let p = seed_entity(conn, earlier, "Expense process", "process");
    conn.execute("INSERT INTO entity_mentions(entity_id, symbol_id, section_hash) VALUES (?1, ?2, 'h')", [p, later]).unwrap();
    let a = |t: &str| StepsAnswer { process: "Expense process".into(), steps: vec![ExtractedStep { text: t.into(), ..Default::default() }] };
    apply_steps(conn, later, "h2", &a("book travel first")).unwrap();
    apply_steps(conn, earlier, "h1", &a("submit expense")).unwrap();
    let texts: Vec<String> = steps_for_process(conn, p).unwrap().into_iter().map(|r| r.text).collect();
    assert_eq!(texts, vec!["submit expense", "book travel first"]);
}

#[test]
fn deleting_a_file_deletes_its_steps_and_systems() {
    let (_dir, store) = temp_store();
    let conn = store.conn();
    let sec = seed_section(conn, "docs/handbook.md", "Expense process", 10);
    let p = seed_entity(conn, sec, "Expense process", "process");
    let s = seed_entity(conn, sec, "Expensify", "system");
    apply_steps(conn, sec, "h", &StepsAnswer { process: "Expense process".into(), steps: vec![ExtractedStep { text: "x".into(), systems: vec!["Expensify".into()], ..Default::default() }] }).unwrap();
    let file_id: i64 = conn.query_row("SELECT file_id FROM symbols WHERE id = ?1", [sec], |r| r.get(0)).unwrap();
    crate::knowledge::delete_for_symbols(conn, file_id).unwrap();
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM steps", [], |r| r.get(0)).unwrap();
    let m: i64 = conn.query_row("SELECT COUNT(*) FROM step_systems", [], |r| r.get(0)).unwrap();
    assert_eq!((n, m), (0, 0));
    let _ = (p, s);
}
```

Also in `store/mod.rs` tests (next to the existing version test): `a_v5_index_is_rebuilt_as_v6_with_the_steps_tables` — write `schema_version = 5` into a fresh db's meta, reopen, assert `SCHEMA_VERSION == 6`, `stage` exists on `extract_queue` (`PRAGMA table_info`), and `steps`, `step_systems`, `steps_cache` exist.

- [ ] **Step 2: Run** — `cargo test -p singularrag-core journeys` → compile errors (no module).

- [ ] **Step 3: Implement**

`schema.rs`: `pub const SCHEMA_VERSION: i64 = 6;`; add `stage INTEGER NOT NULL DEFAULT 0` after `claimed_at_ms` in `extract_queue` with the comment `-- 0 = entities pending, 1 = steps pending (journeys)`; append:

```sql
CREATE TABLE IF NOT EXISTS steps (
  id         INTEGER PRIMARY KEY,
  process_id INTEGER NOT NULL,
  ordinal    INTEGER NOT NULL,
  text       TEXT NOT NULL,
  role_id    INTEGER,
  role_text  TEXT NOT NULL DEFAULT '',
  symbol_id  INTEGER NOT NULL,
  section_hash TEXT,
  UNIQUE (process_id, symbol_id, ordinal)
);
CREATE INDEX IF NOT EXISTS steps_symbol ON steps(symbol_id);
CREATE INDEX IF NOT EXISTS steps_process ON steps(process_id);
CREATE TABLE IF NOT EXISTS step_systems (
  step_id   INTEGER NOT NULL,
  entity_id INTEGER NOT NULL,
  PRIMARY KEY (step_id, entity_id)
);
CREATE TABLE IF NOT EXISTS steps_cache (hash TEXT PRIMARY KEY, json TEXT NOT NULL);
```

Check `drop_all_tables` drops by listing `sqlite_master` (it does for the v4/v5 tables); if it uses a fixed list, add the three tables.

`journeys.rs`:

```rust
//! Processes, steps and roles (spec: docs/superpowers/specs/2026-09-24-singularrag-journeys-design.md).
use crate::knowledge::norm_name;
use crate::models::StepsAnswer;
use crate::Result;
use rusqlite::{params, Connection, OptionalExtension};

#[derive(Debug, Clone, PartialEq)]
pub struct StepRow { pub id: i64, pub process_id: i64, pub ordinal: i64, pub text: String, pub role_id: Option<i64>, pub role_text: String, pub symbol_id: i64 }

fn section_entity(conn: &Connection, symbol_id: i64, ty: &str, norm: &str) -> Result<Option<i64>> {
    Ok(conn.query_row(
        "SELECT e.id FROM entities e JOIN entity_mentions m ON m.entity_id = e.id WHERE m.symbol_id = ?1 AND e.type = ?2 AND e.norm_name = ?3 LIMIT 1",
        params![symbol_id, ty, norm], |r| r.get(0)).optional()?)
}

pub fn delete_steps_for_symbol(conn: &Connection, symbol_id: i64) -> Result<()> {
    conn.execute("DELETE FROM step_systems WHERE step_id IN (SELECT id FROM steps WHERE symbol_id = ?1)", [symbol_id])?;
    conn.execute("DELETE FROM steps WHERE symbol_id = ?1", [symbol_id])?;
    Ok(())
}

pub fn delete_steps_for_symbols(conn: &Connection, symbol_ids_sql: &str, param: i64) -> Result<()> {
    conn.execute(&format!("DELETE FROM step_systems WHERE step_id IN (SELECT id FROM steps WHERE symbol_id IN ({symbol_ids_sql}))"), [param])?;
    conn.execute(&format!("DELETE FROM steps WHERE symbol_id IN ({symbol_ids_sql})"), [param])?;
    Ok(())
}

pub fn apply_steps(conn: &Connection, symbol_id: i64, hash: &str, a: &StepsAnswer) -> Result<Option<usize>> {
    let Some(process_id) = section_entity(conn, symbol_id, "process", &norm_name(&a.process))? else { return Ok(None); };
    delete_steps_for_symbol(conn, symbol_id)?;
    let mut n = 0;
    for (i, s) in a.steps.iter().enumerate() {
        let role_id = if s.role.is_empty() { None } else { section_entity(conn, symbol_id, "role", &norm_name(&s.role))? };
        conn.execute(
            "INSERT INTO steps(process_id, ordinal, text, role_id, role_text, symbol_id, section_hash) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![process_id, i as i64 + 1, s.text, role_id, s.role, symbol_id, hash],
        )?;
        let step_id = conn.last_insert_rowid();
        for sys in &s.systems {
            if let Some(eid) = section_entity(conn, symbol_id, "system", &norm_name(sys))? {
                conn.execute("INSERT OR IGNORE INTO step_systems(step_id, entity_id) VALUES (?1, ?2)", [step_id, eid])?;
            }
        }
        n += 1;
    }
    Ok(Some(n))
}

pub fn steps_for_process(conn: &Connection, process_id: i64) -> Result<Vec<StepRow>> {
    let mut stmt = conn.prepare(
        "SELECT st.id, st.process_id, st.ordinal, st.text, st.role_id, st.role_text, st.symbol_id FROM steps st JOIN symbols s ON s.id = st.symbol_id WHERE st.process_id = ?1 ORDER BY s.line_start, st.ordinal")?;
    let rows = stmt.query_map([process_id], |r| Ok(StepRow { id: r.get(0)?, process_id: r.get(1)?, ordinal: r.get(2)?, text: r.get(3)?, role_id: r.get(4)?, role_text: r.get(5)?, symbol_id: r.get(6)? }))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

pub fn systems_for_step(conn: &Connection, step_id: i64) -> Result<Vec<(i64, String)>> {
    let mut stmt = conn.prepare("SELECT e.id, e.name FROM step_systems ss JOIN entities e ON e.id = ss.entity_id WHERE ss.step_id = ?1 ORDER BY e.name")?;
    let rows = stmt.query_map([step_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

pub fn role_name(conn: &Connection, step: &StepRow) -> Result<String> {
    match step.role_id {
        Some(id) => Ok(conn.query_row("SELECT name FROM entities WHERE id = ?1", [id], |r| r.get(0)).optional()?.unwrap_or_else(|| step.role_text.clone())),
        None => Ok(step.role_text.clone()),
    }
}
```

`knowledge.rs::delete_for_symbols`: before the `for table in [...]` loop add `crate::journeys::delete_steps_for_symbols(conn, SYMS, file_id)?;`. Find the hash-mismatch cleanup (grep `section_hash` near the indexer's queue upsert, the code that deletes mentions and relations whose hash differs) and call `delete_steps_for_symbol(conn, symbol_id)` beside it.

- [ ] **Step 4: Run** — `cargo test -p singularrag-core` → pass. Also `cargo test --workspace` (the serve read-only open refuses a version mismatch, so tests that pre-create a store must all open through `Store::open`; fix any that hard-code `5`).

- [ ] **Step 5: Commit** — `feat(core): schema v6 with steps, step_systems, steps_cache and a queue stage; journeys store module`

---

### Task 3: The steps stage of the tick, its cache, the fixture loader

**Files:**
- Modify: `crates/singularrag-core/src/knowledge.rs` (`queue_section`, `KnowledgeTick`, `extract_step`, `write_section`, new `steps_stage`, `load_extraction_json`)
- Test: `knowledge.rs` `mod tests`

**Interfaces:**
- Consumes: `Models::steps`, `normalise_steps`, `journeys::apply_steps`, `FakeOllama::set_steps`, `claim_rows`, `record_outage`, `cached_extraction`-style helpers.
- Produces: `KnowledgeTick.stepped: usize`; `pub fn steps_json(a: &StepsAnswer) -> Result<String>`; `pub fn cached_steps(conn, hash) -> Result<Option<StepsAnswer>>`; `write_section` now returns `Result<bool>` (`true` = row left at stage 1); `load_extraction_json` accepts an optional `steps` block per section; `pub const CLAIM_STEPS: &str = "q.stage = 1 AND q.attempts < 3"`.

- [ ] **Step 1: Write the failing tests** (`knowledge.rs` tests; use the existing tick-test scaffolding that indexes a tempdir with a doc and points `Models` at a `FakeOllama`)

```rust
fn process_extraction() -> serde_json::Value {
    serde_json::json!({"entities": [
        {"name": "Expense process", "type": "process", "description": "how you get money back"},
        {"name": "Manager", "type": "role", "description": "approves claims"},
        {"name": "Expensify", "type": "system", "description": "the expense tool"}
    ], "relations": []})
}
fn expense_steps() -> serde_json::Value {
    serde_json::json!({"process": "Expense process", "steps": [
        {"text": "Submit each expense in Expensify", "role": "employee", "systems": ["Expensify"]},
        {"text": "Your manager approves or rejects the claim", "role": "Manager", "systems": []}
    ]})
}

#[test]
fn a_process_section_gets_a_steps_call_and_a_plain_section_does_not() {
    let (fake, store, models) = tick_fixture(&[("Expense process", "Submit each expense in Expensify. Your manager approves."), ("Buddy", "Every new colleague is given a buddy.")]);
    fake.set_extraction("Expense", process_extraction());
    fake.set_steps("Expense", expense_steps());
    let t = tick(&store, &models, Duration::from_secs(20), &|| false).unwrap();
    assert_eq!((t.extracted, t.stepped, t.pending), (2, 1, 0));
    assert_eq!(fake.calls().iter().filter(|c| *c == "/api/generate:steps").count(), 1);
    let n: i64 = store.conn().query_row("SELECT COUNT(*) FROM steps", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 2);
    let cached: i64 = store.conn().query_row("SELECT COUNT(*) FROM steps_cache", [], |r| r.get(0)).unwrap();
    assert_eq!(cached, 1);
}

#[test]
fn a_cached_steps_answer_is_applied_without_a_model_call() {
    // same fixture; run one tick, then re-queue the section (touch the file with identical text so the hash is equal)
    // and tick again: steps rows = 2, steps calls still 1.
}

#[test]
fn an_outage_between_the_prompts_resumes_at_the_steps_stage() {
    let (fake, store, models) = tick_fixture(&[("Expense process", "Submit each expense in Expensify.")]);
    fake.set_extraction("Expense", process_extraction());
    fake.set_steps("Expense", expense_steps());
    fake.set_down_after(1); // the entity prompt answers; the steps prompt finds the server down
    let t1 = tick(&store, &models, Duration::from_secs(20), &|| false).unwrap();
    assert_eq!((t1.extracted, t1.stepped, t1.pending), (1, 0, 1));
    assert!(t1.model_error.is_some());
    let stage: i64 = store.conn().query_row("SELECT stage FROM extract_queue", [], |r| r.get(0)).unwrap();
    assert_eq!(stage, 1);
    fake.set_down(false);
    let t2 = tick(&store, &models, Duration::from_secs(20), &|| false).unwrap();
    assert_eq!((t2.extracted, t2.stepped, t2.pending), (0, 1, 0));
    assert_eq!(fake.calls().iter().filter(|c| *c == "/api/generate").count(), 1, "the entity prompt is not asked again");
}

#[test]
fn an_unmatched_process_name_discards_the_answer_and_finishes_the_section() {
    // set_steps returns {"process": "Expenses", ...}; after the tick: steps = 0, pending = 0, stepped = 0, no attempts increment.
}

#[test]
fn a_section_with_two_processes_keeps_steps_only_for_the_named_one() {
    // extraction has "Expense process" and "Travel process" typed process; steps name "Travel process";
    // steps_for_process(expense) is empty, steps_for_process(travel) has the rows; both entities exist.
}

#[test]
fn malformed_steps_json_counts_an_attempt_and_leaves_the_stage() {
    // fake.fail_next_generate(2) before the steps prompt (entity prompt already cached from a prior tick):
    // after the tick attempts = 1, stage = 1, t.failed = 1, no steps rows.
}

#[test]
fn the_fixture_loader_applies_a_steps_block() {
    // write_prose tempdir, index, load_extraction_json with a JSON map whose "docs/handbook.md::Expense process"
    // value has entities/relations plus "steps": expense_steps(); assert 2 steps rows, steps_cache 1, pending 0,
    // and a value without a steps block still loads (0 steps rows for it).
}
```

Add to `FakeOllama`: `pub fn set_down_after(&self, n: usize)` — after `n` more `/api/generate` requests have been answered, the server behaves as if `set_down(true)` had been called (one `AtomicUsize` decremented in the generate arm; when it reaches zero the arm sets the `down` flag). `set_down(false)` clears it.

- [ ] **Step 2: Run** — `cargo test -p singularrag-core knowledge::tests` → compile errors on `stepped`, `set_down_after`, `set_steps` usage.

- [ ] **Step 3: Implement**

- `KnowledgeTick` gains `pub stepped: usize`.
- `queue_section`'s INSERT sets `stage` to `0` explicitly (`…, queued_at_ms, stage) VALUES (?1, ?2, 0, NULL, ?3, 0)`), and the hash-mismatch path in the indexer resets `stage = 0` for a changed section (it re-inserts through `queue_section`, so check it does).
- `write_section` (one transaction): after `apply_extraction` and the vector inserts, decide `let has_process = x.entities.iter().any(|e| e.r#type == "process");`; if `has_process` run `UPDATE extract_queue SET stage = 1, attempts = 0, last_error = NULL WHERE symbol_id = ?1` instead of the `DELETE`, else delete as today; return `Ok(has_process)`. Delete the section's old steps here too (`journeys::delete_steps_for_symbol`) so a re-extracted section without a process loses stale steps.
- `steps_json(a)`/`cached_steps(conn, hash)`: `serde_json` to/from `steps_cache`, mirror `extraction_json`/`cached_extraction`.
- New `fn steps_for_row(store: &Store, models: &Models, symbol_id: i64, hash: &str, input: &SectionInput, t: &mut KnowledgeTick) -> Result<StepOutcome>` with `enum StepOutcome { Applied, Discarded, Malformed, Outage }`:
  1. `cached_steps(conn, hash)?` else `models.steps(input)`; on `Err(ModelUnavailable(m)) if m.contains("not JSON")` → `UPDATE extract_queue SET attempts = attempts + 1, last_error = ?2 WHERE symbol_id = ?1`, `t.failed += 1`, return `Malformed`; on other `ModelUnavailable` → `record_outage` and return `Outage` (caller stops the loop); write `steps_cache` the moment the answer parses.
  2. In one transaction: `journeys::apply_steps`; `Some(_)` → `t.stepped += 1`, `Applied`; `None` → `tracing::warn!(symbol_id, process = %a.process, "steps answer names no process entity of the section; discarded")`, `Discarded`. In both cases `DELETE FROM extract_queue WHERE symbol_id = ?1`.
- In `extract_step`, right after a successful `write_section` that returned `true`, call `steps_for_row` for that section (same claim, same budget check before the call: if the budget is spent or `should_yield()` is true, leave the row at stage 1 and return; the next tick picks it up).
- New `fn steps_stage(store, models, t, start, budget, should_yield) -> Result<bool>` run at the start of `extract_step` (before claiming stage-0 rows): `claim_rows` with `CLAIM_STEPS`, then for each row fetch the section input (`sections_fts` content, heading = symbol name, path) and call `steps_for_row`; `Outage` returns `Ok(false)` to stop the tick; budget and yield checks as in the entity loop. Add `const _: () = assert!(MAX_ATTEMPTS == 3, "CLAIM_STEPS spells MAX_ATTEMPTS out");`.
- `pending()` is unchanged (stage-1 rows still count).
- `load_extraction_json`: change the map value type to

```rust
#[derive(Deserialize)]
struct FixtureSection {
    #[serde(flatten)]
    extraction: Extraction,
    #[serde(default)]
    steps: Option<StepsAnswer>,
}
```

  and after `write_section` (whatever it returned): if `steps` is `Some`, `normalise_steps`, insert into `steps_cache`, `journeys::apply_steps` (warn on `None`); then always `DELETE FROM extract_queue WHERE symbol_id = ?1` so a loaded fixture has nothing pending.

- [ ] **Step 4: Run** — `cargo test -p singularrag-core` → pass; then `cargo test --workspace` (actor tests count ticks and pending; `stepped` must not change their expectations since their extractions have no `process` type).

- [ ] **Step 5: Commit** — `feat(core): the steps stage of the extraction tick, its cache, and the fixture loader's steps block`

---

### Task 4: The derived "implemented by" rule

**Files:**
- Modify: `crates/singularrag-core/src/journeys.rs`
- Test: `journeys.rs` `mod tests`

**Interfaces:**
- Consumes: `symbols`, `files`, `refs` tables; `StepRow`, `systems_for_step`.
- Produces:
  - `pub enum Via { Mention, System(String) }`
  - `pub struct CodeLink { pub symbol_id: i64, pub path: String, pub name: String, pub line: u32, pub via: Via }`
  - `pub struct CodeIndex { … }` and `pub fn load_code_index(conn: &Connection) -> Result<CodeIndex>` — every non-document symbol once, with its normalised name, its file's normalised stem, its path, and the number of other files referencing its name.
  - `pub fn implemented_by(conn: &Connection, index: &CodeIndex, step: &StepRow, systems: &[(i64, String)], limit: usize) -> Result<(Vec<CodeLink>, usize)>` — links (at most `limit`) and the count of the rest.
  - `pub const DOC_KINDS: [&str; 5] = ["section", "document", "element", "rule", "key"]` and `pub const LINK_LIMIT: usize = 3`.
  - `pub fn normalise_ident(s: &str) -> String` — lowercase, `-` and `_` removed.

- [ ] **Step 1: Write the failing tests**

```rust
fn seed_code(conn: &Connection, path: &str, name: &str, kind: &str, line: i64) -> i64 { /* INSERT files (lang 'typescript') if missing + symbols; return id */ }
fn seed_ref(conn: &Connection, from_path: &str, name: &str, line: i64) { /* INSERT refs(file_id of from_path, name, line) */ }

#[test]
fn a_sections_own_code_mentions_come_first_then_system_name_matches() {
    let (_dir, store) = temp_store();
    let conn = store.conn();
    let sec = seed_section(conn, "docs/handbook.md", "Expense process", 10); // lines 10-20 (set line_end 20)
    let approve = seed_code(conn, "src/payroll/expense.ts", "approveClaim", "function", 3);
    seed_ref(conn, "docs/handbook.md", "approveClaim", 12);
    let client = seed_code(conn, "src/vendors/expensify-client.ts", "postClaim", "function", 1);
    let cfg = seed_code(conn, "config/app.json", "integrations.expensify.token", "key", 4);
    let _unrelated = seed_code(conn, "src/auth/okta.ts", "oktaLogin", "function", 1);
    let p = seed_entity(conn, sec, "Expense process", "process");
    let expensify = seed_entity(conn, sec, "Expensify", "system");
    apply_steps(conn, sec, "h", &StepsAnswer { process: "Expense process".into(), steps: vec![ExtractedStep { text: "Submit".into(), systems: vec!["Expensify".into()], ..Default::default() }] }).unwrap();
    let step = &steps_for_process(conn, p).unwrap()[0];
    let index = load_code_index(conn).unwrap();
    let (links, more) = implemented_by(conn, &index, step, &[(expensify, "Expensify".into())], LINK_LIMIT).unwrap();
    assert_eq!(links.iter().map(|l| l.symbol_id).collect::<Vec<_>>(), vec![approve, client, cfg]);
    assert!(matches!(links[0].via, Via::Mention));
    assert!(matches!(&links[1].via, Via::System(s) if s == "Expensify"));
    assert_eq!(more, 0);
    let (two, rest) = implemented_by(conn, &index, step, &[(expensify, "Expensify".into())], 2).unwrap();
    assert_eq!((two.len(), rest), (2, 1));
}

#[test]
fn matching_ignores_case_hyphens_and_underscores_but_not_document_kinds() {
    // system "PagerDuty": symbols `pager_duty_page` (function), file stem `PagerDuty-hooks.ts`, and a
    // section heading "PagerDuty runbook" (kind section) → the first two match, the section does not.
}

#[test]
fn a_two_character_system_matches_no_code() {
    // system "IT" with symbols `item`, `commit` present → no System links.
}

#[test]
fn system_matches_rank_by_incoming_references_then_path() {
    // two matching symbols; one referenced from two other files, one from none → the referenced one first.
}
```

- [ ] **Step 2: Run** — compile errors on `Via`, `CodeLink`, `load_code_index`, `implemented_by`.

- [ ] **Step 3: Implement**

```rust
pub const DOC_KINDS: [&str; 5] = ["section", "document", "element", "rule", "key"];
/// `key` rows are config keys and do count as code for "implemented by"; only the four prose kinds are excluded there.
const NON_CODE_KINDS: [&str; 4] = ["section", "document", "element", "rule"];
pub const LINK_LIMIT: usize = 3;
pub const MIN_SYSTEM_CHARS: usize = 3;

pub fn normalise_ident(s: &str) -> String {
    s.chars().filter(|c| *c != '-' && *c != '_').flat_map(|c| c.to_lowercase()).collect()
}

#[derive(Debug, Clone, PartialEq)]
pub enum Via { Mention, System(String) }

#[derive(Debug, Clone, PartialEq)]
pub struct CodeLink { pub symbol_id: i64, pub path: String, pub name: String, pub line: u32, pub via: Via }

struct CodeSym { symbol_id: i64, file_id: i64, path: String, name: String, line: u32, norm_name: String, norm_stem: String, referrers: i64 }
pub struct CodeIndex { syms: Vec<CodeSym> }

pub fn load_code_index(conn: &Connection) -> Result<CodeIndex> {
    let mut stmt = conn.prepare(
        "SELECT s.id, s.file_id, f.path, s.name, s.line_start,
                (SELECT COUNT(DISTINCT r.file_id) FROM refs r WHERE r.name = s.name AND r.file_id != s.file_id)
         FROM symbols s JOIN files f ON f.id = s.file_id
         WHERE s.kind NOT IN ('section', 'document', 'element', 'rule')")?;
    let syms = stmt.query_map([], |r| {
        let path: String = r.get(2)?;
        let name: String = r.get(3)?;
        let stem = std::path::Path::new(&path).file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
        Ok(CodeSym { symbol_id: r.get(0)?, file_id: r.get(1)?, norm_name: normalise_ident(&name), norm_stem: normalise_ident(&stem), path, name, line: r.get::<_, i64>(4)? as u32, referrers: r.get(5)? })
    })?.collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(CodeIndex { syms })
}

fn section_mentions(conn: &Connection, index: &CodeIndex, symbol_id: i64) -> Result<Vec<CodeLink>> {
    let (file_id, a, b): (i64, i64, i64) = conn.query_row("SELECT file_id, line_start, line_end FROM symbols WHERE id = ?1", [symbol_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    let mut stmt = conn.prepare("SELECT DISTINCT name FROM refs WHERE file_id = ?1 AND line BETWEEN ?2 AND ?3")?;
    let names: Vec<String> = stmt.query_map(params![file_id, a, b], |r| r.get(0))?.collect::<std::result::Result<_, _>>()?;
    let mut out = Vec::new();
    for s in &index.syms {
        if s.file_id != file_id && names.iter().any(|n| n == &s.name) {
            out.push(CodeLink { symbol_id: s.symbol_id, path: s.path.clone(), name: s.name.clone(), line: s.line, via: Via::Mention });
        }
    }
    out.sort_by(|x, y| (&x.path, &x.name).cmp(&(&y.path, &y.name)));
    Ok(out)
}

pub fn implemented_by(conn: &Connection, index: &CodeIndex, step: &StepRow, systems: &[(i64, String)], limit: usize) -> Result<(Vec<CodeLink>, usize)> {
    let mut links = section_mentions(conn, index, step.symbol_id)?;
    let mut by_system: Vec<(&CodeSym, String)> = Vec::new();
    for (_, name) in systems {
        let norm = normalise_ident(&norm_name(name));
        if norm.chars().count() < MIN_SYSTEM_CHARS { continue; }
        for s in &index.syms {
            if (s.norm_name.contains(&norm) || s.norm_stem.contains(&norm)) && !links.iter().any(|l| l.symbol_id == s.symbol_id) && !by_system.iter().any(|(x, _)| x.symbol_id == s.symbol_id) {
                by_system.push((s, name.clone()));
            }
        }
    }
    by_system.sort_by(|(a, _), (b, _)| b.referrers.cmp(&a.referrers).then_with(|| (&a.path, &a.name).cmp(&(&b.path, &b.name))));
    links.extend(by_system.into_iter().map(|(s, sys)| CodeLink { symbol_id: s.symbol_id, path: s.path.clone(), name: s.name.clone(), line: s.line, via: Via::System(sys) }));
    let more = links.len().saturating_sub(limit);
    links.truncate(limit);
    Ok((links, more))
}
```

- [ ] **Step 4: Run** — `cargo test -p singularrag-core journeys` → pass.

- [ ] **Step 5: Commit** — `feat(core): derived "implemented by" links for a step (section mentions, then system-name matches)`

---

### Task 5: `entities` renders steps and serves the linked code

**Files:**
- Modify: `crates/singularrag-core/src/journeys.rs` (`ProcessSteps`, `process_steps`, `render_steps`)
- Modify: `crates/singularrag-core/src/knowledge.rs` (`EntityHit` gains `steps: Option<ProcessSteps>`; `match_entities` fills it; `render_entities` prints it)
- Modify: `crates/singularrag-core/src/engine.rs` (`entities` adds code links to the served items with `kind` from the symbol)
- Test: `journeys.rs`, `knowledge.rs`, `engine.rs` tests

**Interfaces:**
- Consumes: Tasks 2 and 4; `EntityHit`, `render_entities`, `Engine::entities`, `record_retrieval`.
- Produces:
  - `pub struct StepView { pub ordinal: usize, pub text: String, pub role: String, pub systems: Vec<String>, pub section: knowledge::CitedSection, pub code: Vec<CodeLink>, pub more_code: usize }` where `SectionRef` is `knowledge::CitedSection` (the type `EntityHit.sections` already uses; make `cited_section` `pub` and reuse both, do not define a second one).
  - `pub struct ProcessSteps { pub steps: Vec<StepView> }`
  - `pub fn process_steps(conn: &Connection, index: &CodeIndex, process_id: i64) -> Result<ProcessSteps>` — steps in document order, `ordinal` renumbered 1.. across sections, each with role name, system names, section ref and `implemented_by(…, LINK_LIMIT)`.
  - `pub fn render_steps(p: &ProcessSteps, out: &mut String)` — one line per step: `  N. <text> — role: <role> — systems: <a, b> — <path>::<name>` with `role:`/`systems:` segments omitted when empty, and ` — code: <path>::<name> (+M)` when links exist (only the first link is printed; `(+M)` counts the remaining links plus `more_code`).
  - `EntityHit.steps: Option<ProcessSteps>` (set for `type == "process"`); the footer becomes `# N entities · M relations · S steps · K sections` (the `steps` term only when S > 0, so existing tests keep passing).

- [ ] **Step 1: Write the failing tests**

```rust
// journeys.rs
#[test]
fn render_steps_prints_role_systems_section_and_the_first_code_link_with_a_count() {
    let p = ProcessSteps { steps: vec![
        StepView { ordinal: 1, text: "Submit in Expensify".into(), role: "employee".into(), systems: vec!["Expensify".into()], section: sect("docs/handbook.md", "Expense process", 10, 20), code: vec![], more_code: 0 },
        StepView { ordinal: 2, text: "Finance reviews".into(), role: "".into(), systems: vec![], section: sect("docs/handbook.md", "Expense process", 10, 20),
                   code: vec![link("src/payroll/expense.ts", "approveClaim", 3), link("src/payroll/expense.ts", "review", 9)], more_code: 1 },
    ]};
    let mut out = String::new();
    render_steps(&p, &mut out);
    assert_eq!(out, "  1. Submit in Expensify — role: employee — systems: Expensify — docs/handbook.md::Expense process\n  2. Finance reviews — docs/handbook.md::Expense process — code: src/payroll/expense.ts::approveClaim (+2)\n");
}

// knowledge.rs
#[test]
fn a_process_hit_renders_its_steps_and_the_footer_counts_them() {
    // index the prose fixture, load the extraction with the expense steps block (Task 3's fixture json),
    // match_entities(&store, None, "expense process", &[], 10) → the hit's steps is Some with 2 steps;
    // render_entities contains "  1. Submit each expense in Expensify" and ends with "· 2 steps · ".
}

// engine.rs
#[test]
fn entities_serves_the_linked_code_after_the_cited_sections() {
    // fixture with a code file `src/payroll/expense.ts` defining `approveClaim`, the handbook section mentioning
    // `approveClaim` in a code span, extraction + steps loaded; engine.entities("expense process") →
    // the retrieval's served items contain docs/handbook.md::Expense process before src/payroll/expense.ts::approveClaim,
    // the code item's kind is "function", and its reasons.seeds contain "implements:Expense process".
}
```

- [ ] **Step 2: Run** — compile errors.

- [ ] **Step 3: Implement**

```rust
// journeys.rs
pub struct StepView { pub ordinal: usize, pub text: String, pub role: String, pub systems: Vec<String>, pub section: crate::knowledge::CitedSection, pub code: Vec<CodeLink>, pub more_code: usize }
pub struct ProcessSteps { pub steps: Vec<StepView> }

pub fn process_steps(conn: &Connection, index: &CodeIndex, process_id: i64) -> Result<ProcessSteps> {
    let mut steps = Vec::new();
    for (i, row) in steps_for_process(conn, process_id)?.into_iter().enumerate() {
        let systems = systems_for_step(conn, row.id)?;
        let (code, more_code) = implemented_by(conn, index, &row, &systems, LINK_LIMIT)?;
        let section = crate::knowledge::cited_section(conn, row.symbol_id)?.ok_or_else(|| crate::Error::Config(format!("step {} cites a missing section {}", row.id, row.symbol_id)))?;
        steps.push(StepView { ordinal: i + 1, text: row.text.clone(), role: role_name(conn, &row)?, systems: systems.into_iter().map(|(_, n)| n).collect(), section, code, more_code });
    }
    Ok(ProcessSteps { steps })
}

pub fn render_steps(p: &ProcessSteps, out: &mut String) {
    for s in &p.steps {
        out.push_str(&format!("  {}. {}", s.ordinal, s.text));
        if !s.role.is_empty() { out.push_str(&format!(" — role: {}", s.role)); }
        if !s.systems.is_empty() { out.push_str(&format!(" — systems: {}", s.systems.join(", "))); }
        out.push_str(&format!(" — {}::{}", s.section.path, s.section.name));
        if let Some(first) = s.code.first() {
            let rest = s.code.len() - 1 + s.more_code;
            out.push_str(&format!(" — code: {}::{}", first.path, first.name));
            if rest > 0 { out.push_str(&format!(" (+{rest})")); }
        }
        out.push('\n');
    }
}
```

`knowledge.rs`: in `match_entities`, after building each `EntityHit`, `if hit.r#type == "process" { hit.steps = Some(journeys::process_steps(conn, &index, hit.id)?) }` with one `load_code_index` per call (build it lazily on the first process hit). `render_entities`: after the entity's `← section` lines and before its relations, `if let Some(p) = &h.steps { journeys::render_steps(p, &mut out); steps += p.steps.len(); }`; footer adds `· S steps` after relations when `steps > 0`.

`engine.rs::entities`: after the section items, for each hit with steps, for each step's `code` links push a `ScoredSymbol` (dedup by `symbol_id`; `kind` from `SELECT kind FROM symbols`; `path`, `name`, `line_start = line`, `score = 1.0 / (ranked.len()+1)`) with `reasons.seeds.push(format!("implements:{}", h.name))` and `reasons.implements.push(h.name.clone())` (the field arrives in Task 6; add the field in this task if the compiler needs it, with the same shape Task 6 specifies).

- [ ] **Step 4: Run** — `cargo test --workspace` → pass (the MCP `entities` tests render the same fixtures; check none asserts the exact footer).

- [ ] **Step 5: Commit** — `feat(core): entities renders a process as ordered steps and serves its implementing code`

---

### Task 6: The `implements` seed and reason; MCP text

**Files:**
- Modify: `crates/singularrag-core/src/rank.rs` (`Reasons.implements`, `KnowledgeHits.implements`, personalization arm)
- Modify: `crates/singularrag-core/src/knowledge.rs` (`Seeds.implements: Vec<(i64, String)>` symbol id + system name; `seeds_for` fills it for matched `process` entities)
- Modify: `crates/singularrag-core/src/map.rs` (reason rendering: "Implements Expensify")
- Modify: `crates/singularrag/src/mcp/server.rs`, `crates/singularrag/tests/mcp.rs`
- Modify: `ui/src/api/types.ts` (`Reasons.implements?: string[]`) and wherever the UI renders reasons (`DetailPanel` reason list) — one line.
- Test: `rank.rs`/`knowledge.rs`/`engine.rs` tests, `tests/mcp.rs`

**Interfaces:**
- Consumes: Task 4 (`load_code_index`, `implemented_by`), `seeds_for`, the rank personalization block.
- Produces: `Reasons.implements: Vec<String>` (system names; `#[serde(default)]` so old retrieval rows deserialise), `Seeds.implements: Vec<(i64, String)>`, the reason text `Implements <system>`.

- [ ] **Step 1: Write the failing tests**

```rust
// knowledge.rs
#[test]
fn a_matched_process_seeds_the_code_its_systems_implement() {
    // fixture: handbook expense section + steps loaded; a code symbol `expensifyClient` in src/vendors/expensify.ts;
    // seeds_for(&store, None, Some("expense process"), &[], &[]) → seeds.implements contains (that symbol id, "Expensify").
}
// engine.rs
#[test]
fn repo_map_for_a_process_query_serves_the_implementing_file_with_an_implements_reason() {
    // same fixture, repo_map { query: "expense process", budget 4096 } → src/vendors/expensify.ts is served and its
    // reasons.implements == ["Expensify"], reasons.seeds contains "implements:Expensify".
}
```

`tests/mcp.rs`: update the copied `ENTITIES_DESCRIPTION` and `INSTRUCTIONS` constants to the new text (below) so `lists_exactly_the_six_tools_with_spec_descriptions` fails first, then passes.

- [ ] **Step 2: Run** — failures.

- [ ] **Step 3: Implement**

- `Seeds` gains `pub implements: Vec<(i64, String)>`. In `seeds_for`, after the entity matches: for each matched entity whose `type == "process"`, `let index = load_code_index(conn)?` (once), for each step (`steps_for_process`) and its systems, `implemented_by(conn, &index, &row, &systems, usize::MAX)` and push every `Via::System(name)` link as `(symbol_id, name)` (mention links are already covered by the section seed).
- `rank.rs`: `Reasons` gains `#[serde(default)] pub implements: Vec<String>`; the knowledge-hit collection maps `seeds.implements` by symbol id into `implements_files: HashMap<file_id, Vec<String>>` the same way `entity_files` is built; the personalization loop adds

```rust
if let Some(names) = implements_files.get(&node.id) {
    personalization[i] += NOTE_BOOST;
    seeds[i].extend(names.iter().map(|n| format!("implements:{n}")));
}
```

  and the per-symbol reasons set `reasons.implements = names.clone()` for symbols in those files (mirror how `reasons.entities` is filled); hit weight for the implementing symbols is 1.0 in the `hit_shares` input (a name hit).
- `map.rs` reason rendering: `Implements <a>, <b>` after the entity line.
- `ENTITIES_DESCRIPTION` becomes (all three copies): `"What the corpus knows about a person, system, concept, event or process, and how it connects: matched entities with type and description, their relations, and the document sections that state them (path::heading, lines). A process comes back as ordered steps, each with its role, systems, documenting section and implementing code. `query` is a name or a question; `entities` are names you already know; `limit` defaults to 10, max 25. Descriptions are extracted text, not verified facts. Use it before reading a document about someone or something, and to learn how a process works."`
- `INSTRUCTIONS`: the sentence `Use entities to learn what the corpus says about a person, system or concept and how it connects;` becomes `Use entities to learn what the corpus says about a person, system or concept and how it connects, and for the ordered steps of a process;` (both copies).
- UI `types.ts` `Reasons` gains `implements?: string[]`; the reason list in `DetailPanel.tsx` renders `Implements ${names.join(", ")}` next to the `Mentions` line (find the `entities` reason rendering and add one line beside it, with a test in `DetailPanel.test.tsx` or `App.test.tsx` mirroring the `Mentions` test).

- [ ] **Step 4: Run** — full Rust gate and `cd ui && bun run typecheck && bun test && bun run build`.

- [ ] **Step 5: Commit** — `feat: repo_map seeds a process's implementing code with an implements reason; entities describes steps`

---

### Task 7: `GET /api/processes`

**Files:**
- Modify: `crates/singularrag/src/serve/queries.rs` (`ProcessesDto` and friends, `processes(store)`)
- Modify: `crates/singularrag/src/serve/routes.rs` (`processes` handler)
- Modify: `crates/singularrag/src/serve/mod.rs` (`.route("/processes", get(routes::processes))` after `/entities`)
- Test: `crates/singularrag/tests/serve.rs` (or wherever the `/api/entities` route test lives)

**Interfaces:**
- Consumes: `journeys::{load_code_index, process_steps, steps_for_process}`, `locked`.
- Produces: `ProcessesDto { processes: Vec<ProcessDto>, truncated: bool }`, `ProcessDto { id, name, description, roles: Vec<String>, steps: Vec<StepDto> }`, `StepDto { ordinal, text, role, systems: Vec<EntityRefDto {id, name}>, section: SectionRefDto {symbol_id, path, name, line}, code: Vec<CodeLinkDto {symbol_id, path, name, line, via}>, more_code }` with `via` serialised as `"mention"` or `{"system": "<name>"}`; caps `PROCESS_CAP = 200`, `STEP_CAP = 30`.

- [ ] **Step 1: Write the failing test** — copy the `/api/entities` route test: create the fixture store with the prose extraction + steps loaded (Task 3's loader), start the server, `GET /api/processes` with the token → 200, `processes[0].name == "Expense process"` (sorted by name: check which of the four comes first alphabetically and assert that), its `steps.len() == 2`, `steps[0].section.path == "docs/handbook.md"`, `roles` contains `"Manager"`; without the token → 401; `truncated == false`.

- [ ] **Step 2: Run** — 404 / compile error.

- [ ] **Step 3: Implement**

```rust
// queries.rs
const PROCESS_CAP: i64 = 200;
const STEP_CAP: usize = 30;
pub fn processes(store: &Store) -> Result<ProcessesDto> {
    let conn = store.conn();
    let index = journeys::load_code_index(conn)?;
    let mut stmt = conn.prepare("SELECT id, name, description FROM entities WHERE type = 'process' ORDER BY name LIMIT ?1")?;
    let rows: Vec<(i64, String, String)> = stmt.query_map([PROCESS_CAP + 1], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<std::result::Result<_, _>>()?;
    let truncated = rows.len() as i64 > PROCESS_CAP;
    let mut processes = Vec::new();
    for (id, name, description) in rows.into_iter().take(PROCESS_CAP as usize) {
        let p = journeys::process_steps(conn, &index, id)?;
        let mut roles: Vec<String> = p.steps.iter().map(|s| s.role.clone()).filter(|r| !r.is_empty()).collect();
        roles.sort(); roles.dedup();
        let steps = p.steps.into_iter().take(STEP_CAP).map(step_dto).collect();
        processes.push(ProcessDto { id, name, description, roles, steps });
    }
    Ok(ProcessesDto { processes, truncated })
}
```

`step_dto` maps `StepView` → `StepDto`, `Via::Mention` → `"mention"`, `Via::System(s)` → `{"system": s}` through a `#[serde(untagged)]` enum or a `serde_json::Value`.

`routes.rs`: `pub async fn processes(State(s): State<AppState>) -> Result<Json<queries::ProcessesDto>, ApiError> { Ok(Json(locked(&s, queries::processes)?)) }`.

- [ ] **Step 4: Run** — `cargo test -p singularrag --test serve` (and the workspace).

- [ ] **Step 5: Commit** — `feat(serve): GET /api/processes with steps, roles and derived code links`

---

### Task 8: The Journeys view

**Files:**
- Modify: `ui/src/api/types.ts` (`ProcessesPayload`, `Process`, `Step`, `CodeLink`; update the entity-type doc comment), `ui/src/api/client.ts` (`processes: () => req<ProcessesPayload>("GET", "/processes")`)
- Modify: `ui/src/components/ViewToggle.tsx` (`View = "tree" | "map" | "journeys"`, option `{ value: "journeys", label: "Journeys" }`, `loadView` accepts it)
- Create: `ui/src/components/JourneysView.tsx`, `ui/src/components/JourneysView.test.tsx`
- Modify: `ui/src/App.tsx` (fetch `/api/processes` in `load()` beside entities with the same JSON dedup; render `JourneysView` when `view === "journeys"`; hide `OverlayToggle` unless `view === "map"`; pass `onFocusSection` and a new `onFocusCode(path, name, symbolId)` that reuses `focusSection`)
- Modify: `ui/src/App.test.tsx`
- Test: both test files

**Interfaces:**
- Consumes: Task 7's JSON; `focusSection(path, name, symbolId?)`; `Status.entities_pending`.
- Produces: `export function JourneysView({ payload, pending, onFocusSection }: { payload: ProcessesPayload | null; pending: number; onFocusSection: FocusSection })`.

- [ ] **Step 1: Write the failing tests** (`JourneysView.test.tsx`)

```tsx
const payload: ProcessesPayload = { processes: [
  { id: 1, name: "Expense process", description: "how you get money back", roles: ["Manager", "employee"], steps: [
    { ordinal: 1, text: "Submit each expense in Expensify", role: "employee", systems: [{ id: 3, name: "Expensify" }], section: { symbol_id: 10, path: "docs/handbook.md", name: "Expense process", line: 12 }, code: [], more_code: 0 },
    { ordinal: 2, text: "Your manager approves", role: "Manager", systems: [], section: { symbol_id: 10, path: "docs/handbook.md", name: "Expense process", line: 12 },
      code: [{ symbol_id: 40, path: "src/payroll/expense.ts", name: "approveClaim", line: 3, via: "mention" }, { symbol_id: 41, path: "src/vendors/expensify.ts", name: "postClaim", line: 1, via: { system: "Expensify" } }], more_code: 2 },
  ]},
  { id: 2, name: "Release process", description: "", roles: ["release captain"], steps: [
    { ordinal: 1, text: "Cut the release branch", role: "release captain", systems: [], section: { symbol_id: 11, path: "docs/handbook.md", name: "Release process", line: 30 }, code: [], more_code: 0 },
  ]},
], truncated: false };

test("lists processes, shows the selected one's steps as an ordered list, and the role filter narrows the list", async () => {
  const user = userEvent.setup();
  const onFocusSection = mock(() => {});
  render(<JourneysView payload={payload} pending={0} onFocusSection={onFocusSection} />);
  const list = screen.getByRole("list", { name: "Processes" });
  expect(within(list).getAllByRole("button")).toHaveLength(2);
  await user.click(within(list).getByRole("button", { name: /Expense process/ }));
  const steps = screen.getByRole("list", { name: "Steps of Expense process" });
  expect(within(steps).getAllByRole("listitem")).toHaveLength(2);
  expect(within(steps).getByText("Manager")).toBeTruthy();
  await user.click(within(steps).getAllByRole("button", { name: /Documented in docs\/handbook\.md::Expense process/ })[0]);
  expect(onFocusSection).toHaveBeenCalledWith("docs/handbook.md", "Expense process", 10);
  await user.click(within(steps).getByRole("button", { name: "Implemented by src/payroll/expense.ts::approveClaim" }));
  expect(onFocusSection).toHaveBeenLastCalledWith("src/payroll/expense.ts", "approveClaim", 40);
  expect(within(steps).getByText("+2 more")).toBeTruthy();
  await user.selectOptions(screen.getByRole("combobox", { name: "Role" }), "release captain");
  expect(within(screen.getByRole("list", { name: "Processes" })).getAllByRole("button")).toHaveLength(1);
});

test("the journeys view explains an empty corpus", () => {
  render(<JourneysView payload={{ processes: [], truncated: false }} pending={12} onFocusSection={() => {}} />);
  expect(screen.getByText("No processes extracted yet (12 sections pending).")).toBeTruthy();
  expect((screen.getByRole("combobox", { name: "Role" }) as HTMLSelectElement).disabled).toBe(true);
});

test("the journeys view is axe clean with a process selected and a role filter applied", async () => {
  const user = userEvent.setup();
  const { container } = render(<JourneysView payload={payload} pending={0} onFocusSection={() => {}} />);
  await user.click(screen.getByRole("button", { name: /Expense process/ }));
  await user.selectOptions(screen.getByRole("combobox", { name: "Role" }), "Manager");
  expect((await axe.run(container)).violations).toEqual([]);
});
```

`App.test.tsx`: the fetch mock answers `/api/processes` with the payload; a test that clicks the `Journeys` radio, sees the process list, and that the `Overlay` radiogroup is absent; a test that a change event refetches `/api/processes` (mirror the entities one); a test that "Documented in" switches to the tree view and reveals the row (mirror the entity "Go to section" test).

- [ ] **Step 2: Run** — `cd ui && bun test` → failures (module missing, radio missing).

- [ ] **Step 3: Implement** `JourneysView.tsx`:

```tsx
import { useMemo, useState } from "react";
import type { ProcessesPayload, Process, Step } from "../api/types";
import type { FocusSection } from "./EntityView";

export function JourneysView({ payload, pending, onFocusSection }: { payload: ProcessesPayload | null; pending: number; onFocusSection: FocusSection }) {
  const [selected, setSelected] = useState<number | null>(null);
  const [role, setRole] = useState("");
  const processes = payload?.processes ?? [];
  const roles = useMemo(() => Array.from(new Set(processes.flatMap((p) => p.roles))).sort(), [processes]);
  const shown = role ? processes.filter((p) => p.roles.includes(role)) : processes;
  const current = shown.find((p) => p.id === selected) ?? shown[0] ?? null;
  if (processes.length === 0) {
    return (
      <section aria-label="Journeys" className="p-4">
        <label className="block text-sm">Role <select aria-label="Role" disabled className="ml-2"><option value="">All roles</option></select></label>
        <p className="mt-4 text-sm">{pending > 0 ? `No processes extracted yet (${pending} sections pending).` : "No processes extracted yet."}</p>
      </section>
    );
  }
  return (
    <section aria-label="Journeys" className="grid h-full min-h-0 grid-cols-[minmax(12rem,1fr)_3fr] gap-4 p-4">
      <div className="min-h-0 overflow-auto">
        <label className="block text-sm">Role
          <select aria-label="Role" className="ml-2" value={role} onChange={(e) => setRole(e.target.value)}>
            <option value="">All roles</option>
            {roles.map((r) => <option key={r} value={r}>{r}</option>)}
          </select>
        </label>
        <ul aria-label="Processes" className="mt-3 space-y-1">
          {shown.map((p) => (
            <li key={p.id}>
              <button type="button" aria-pressed={current?.id === p.id} onClick={() => setSelected(p.id)} className="w-full text-left …">
                <span className="font-medium">{p.name}</span>
                <span className="block text-xs opacity-80">{p.steps.length} steps{p.roles.length ? ` · ${p.roles.join(", ")}` : ""}</span>
              </button>
            </li>
          ))}
        </ul>
      </div>
      <div className="min-h-0 overflow-auto">
        {current && <ProcessSteps process={current} onFocusSection={onFocusSection} />}
      </div>
    </section>
  );
}

function ProcessSteps({ process, onFocusSection }: { process: Process; onFocusSection: FocusSection }) {
  return (
    <>
      <h2 className="text-base font-semibold">{process.name}</h2>
      {process.description && <p className="text-sm">{process.description}</p>}
      <ol aria-label={`Steps of ${process.name}`} className="mt-3 space-y-3 list-decimal pl-6">
        {process.steps.map((s) => <StepItem key={s.ordinal} step={s} onFocusSection={onFocusSection} />)}
      </ol>
    </>
  );
}

function StepItem({ step, onFocusSection }: { step: Step; onFocusSection: FocusSection }) {
  const sec = step.section;
  return (
    <li>
      <p>{step.text}</p>
      <dl className="mt-1 grid grid-cols-[auto_1fr] gap-x-3 text-xs">
        {step.role && <><dt>Role</dt><dd>{step.role}</dd></>}
        {step.systems.length > 0 && <><dt>Systems</dt><dd>{step.systems.map((x) => <span key={x.id} className="mr-2 rounded border px-1">{x.name} <span className="opacity-70">(system)</span></span>)}</dd></>}
        <dt>Documented in</dt>
        <dd><button type="button" className="underline" aria-label={`Documented in ${sec.path}::${sec.name}`} onClick={() => onFocusSection(sec.path, sec.name, sec.symbol_id)}>{sec.path}::{sec.name}</button></dd>
        {(step.code.length > 0 || step.more_code > 0) && <>
          <dt>Implemented by</dt>
          <dd>
            {step.code.map((c) => (
              <button key={c.symbol_id} type="button" className="mr-2 underline" aria-label={`Implemented by ${c.path}::${c.name}`} onClick={() => onFocusSection(c.path, c.name, c.symbol_id)}>
                {c.path}::{c.name} <span className="opacity-70">({typeof c.via === "string" ? "mentioned" : `via ${c.via.system}`})</span>
              </button>
            ))}
            {step.more_code > 0 && <span>+{step.more_code} more</span>}
          </dd>
        </>}
      </dl>
    </li>
  );
}
```

`App.tsx`: add `processes` state and `processesJson` ref, fetch in `load()` exactly like entities; `view === "journeys"` renders `<JourneysView payload={processes} pending={status?.entities_pending ?? 0} onFocusSection={focusSection} />`; `OverlayToggle` renders only when `view === "map"`; `focusSection` already switches to the tree view. `main`'s className: treat `journeys` like `tree` (scrolling).

- [ ] **Step 4: Run** — `cd ui && bun run typecheck && bun test && bun run build`, then `cargo test --workspace` (assets embedded).

- [ ] **Step 5: Commit** — `feat(ui): the Journeys view: processes, ordered steps, role filter, documented-in and implemented-by links`

---

### Task 9: Fixture steps, the eval's `entities` run, docs, the gate record

**Files:**
- Modify: `crates/singularrag-core/fixtures/prose/extraction.json` (handbook sections: `process` and `role` entities, `steps` blocks; voyage sections unchanged)
- Modify: `eval/questions-prose.toml` (four `process` questions, ids `S1`–`S4`)
- Modify: `crates/singularrag-core/src/eval.rs` (`EvalResult.cited: Option<bool>`; `run` calls `engine.entities` for categories `entity`, `relation`, `process`; `render_report` adds a `cited` column and a `cited N/M` line)
- Modify: `crates/singularrag-core/src/engine.rs` (`pub fn load_extraction_json(&mut self, json: &str) -> Result<usize>` wrapping `knowledge::load_extraction_json(&self.store, self.models_if_enabled(), json)`)
- Modify: `crates/singularrag/src/main.rs` (`eval --extraction <path>`: reads the file and calls `engine.load_extraction_json` after `refresh`, before the questions)
- Modify: `README.md`, `eval/README.md`, `docs/superpowers/specs/2026-09-19-singularrag-design.md` (§7, §9, §10, §12 amendments marked *Amended 2026-09-24 (journeys design, `docs/superpowers/specs/2026-09-24-singularrag-journeys-design.md`)*)
- Test: `crates/singularrag/tests/cli.rs`, `eval.rs` tests, `knowledge.rs` fixture test

**Interfaces:**
- Consumes: everything above.
- Produces: the recorded numbers.

- [ ] **Step 1: Write the failing tests**

```rust
// crates/singularrag/tests/cli.rs
#[test]
fn the_prose_question_file_has_four_process_questions_whose_gold_exists() {
    // load e*/questions-prose.toml, assert 16 questions, 4 with category "process", every gold resolves in a write_prose index.
}

#[test]
fn eval_with_the_checked_in_extraction_cites_gold_for_every_process_question() {
    // write_prose tempdir; `singularrag --repo <dir> eval --questions <prose toml> --budget 4096 --no-models --extraction <fixtures/prose/extraction.json>`
    // → stdout has a "cited" column and the line "cited 4/4 process" (and "cited N/8 entity+relation", whatever N is; assert only the process line).
}

// crates/singularrag-core/src/knowledge.rs
#[test]
fn the_checked_in_extraction_has_steps_for_the_four_handbook_processes() {
    // load fixtures/prose/extraction.json into a write_prose index (models None): four process entities, each with ≥ 3 steps,
    // "Expense process" step 2's role resolves to a role entity ("Manager" or as the fixture names it), and pending == 0.
}
```

- [ ] **Step 2: Run** — failures (missing flag, missing questions, missing steps).

- [ ] **Step 3: Implement**

- `extraction.json`: for `docs/handbook.md::Onboarding`, `::Expense process`, `::Incident process`, `::Release process` add a `process` entity named exactly as the heading, `role` entities (`New colleague`, `Buddy`, `IT desk`? — no: `IT desk` and `People team` are organisations already; roles: `Manager`, `Finance`, `On-call engineer`, `Release captain`, `Employee`, `Buddy`), keep systems (`Okta`, `Expensify`, `PagerDuty`, `staging`), and a `steps` block of 3–5 steps per process, each `text` a sentence from the handbook, `role` one of that section's role entities or empty, `systems` names from that section's system entities. Run the fixture test to confirm resolution.
- `questions-prose.toml` (append; never edit existing ids):

```toml
[[question]]
id = "S1"
category = "process"
query = "who approves an expense claim before Finance sees it"
gold = ["docs/handbook.md::Expense process"]

[[question]]
id = "S2"
category = "process"
query = "what does the release captain do once staging looks healthy"
gold = ["docs/handbook.md::Release process"]

[[question]]
id = "S3"
category = "process"
query = "who writes the post-mortem after an incident and by when"
gold = ["docs/handbook.md::Incident process"]

[[question]]
id = "S4"
category = "process"
query = "which single sign-on system opens every other tool on the first day"
gold = ["docs/handbook.md::Onboarding"]
```

- `eval.rs`: `EvalResult` gains `pub cited: Option<bool>` (`#[serde(default)]`); in `run`, for `category` in `["entity", "relation", "process"]`, call `engine.entities(&EntitiesRequest { query: q.query.clone(), entities: vec![], limit: ENTITIES_LIMIT_DEFAULT })`, `served_keys` on its retrieval id, `cited = Some(q.gold.iter().any(|g| served.contains(g)))`; `render_report` prints a `cited` column (`yes`/`no`/`-`) and, after the mean line, one line per category that has cited values: `cited 4/4 process`.
- `main.rs`: `#[arg(long)] extraction: Option<PathBuf>` on `Eval`; after `refresh`, `if let Some(p) = extraction { let n = engine.load_extraction_json(&std::fs::read_to_string(&p)?)?; eprintln!("loaded {n} sections from {}", p.display()); }`.
- Run the eval three ways and record in `eval/README.md` under a new `## Tier one on journeys` section after "Tier one on knowledge": the pinned-corpus recipe from the knowledge section, seeds off, at 1024 / 2048 / 4096 for the 16-question prose set (means and the per-category lines), the `cited` lines with `--extraction`, and hono / docs at 4096 against their pinned checkouts (must be within 0.02 of 0.771 / 0.653). If hono or docs moved by more than 0.02, stop and report NEEDS_CONTEXT with the numbers.
- `README.md`: in "What the agent sees", after the `entities` paragraph: one paragraph that a process comes back as ordered steps with role, systems, documenting section and implementing code, that links are derived (mentions first, then system-name matches) and never stored, and that the Journeys view shows the same per role. In "Local models": the second prompt, when it runs, and that it sends the same section text.
- Parent spec amendments as listed.
- Knowledge spec §6: no change (this plan's gate is in the journeys spec).

- [ ] **Step 4: Run the gate** — Rust and UI gates, plus the three eval runs.

- [ ] **Step 5: Commit** — `docs+eval: process questions, the entities citation run, journeys docs; parent spec amended`

---

## Self-review

- Spec coverage: §2 → Tasks 2 (tables, deletion), 1 (types); §3 → Tasks 1 (prompt), 3 (gate, stage, cache, resolution, loader); §4 → Tasks 4 (implemented by), 5 (entities output, served code), 6 (seed, reason, descriptions); §5 → Tasks 7, 8; §6 → Task 9 (questions, `--extraction`, `cited`, gate record); §7 → Task 7 (route behind auth), Task 3 (same text to the same URL); §8 → Task 9; §9 → each task's tests; §10 holds.
- Names: `StepsAnswer`/`ExtractedStep`/`normalise_steps`/`parse_steps`/`Models::steps` (T1) used in T3, T9; `StepRow`, `apply_steps`, `steps_for_process`, `systems_for_step`, `role_name`, `delete_steps_for_symbol(s)` (T2) in T3–T7; `CodeIndex`, `load_code_index`, `implemented_by`, `CodeLink`, `Via`, `LINK_LIMIT`, `MIN_SYSTEM_CHARS`, `normalise_ident` (T4) in T5–T7; `StepView`, `ProcessSteps`, `process_steps`, `render_steps` (T5) in T7; `Seeds.implements`, `Reasons.implements` (T6); `ProcessesDto` and the TS `ProcessesPayload`/`Process`/`Step`/`CodeLink` (T7, T8); `KnowledgeTick.stepped`, `write_section -> Result<bool>`, `CLAIM_STEPS`, `FakeOllama::set_steps`/`set_down_after` (T1, T3).
- Recorded deviations from the spec: (a) system-name matches are ordered by the count of files referencing the symbol's name, then path and name, because the index stores no static `file_rank` (it is computed per query); the spec's §4 sentence is amended in this plan's commit. (b) `key` symbols count as code for "implemented by" (the spec says "config key"), while `rule` (CSS) does not.
- Review Focus 1–5 have named tests in Tasks 3, 3, 4, 3, 8.
