# singularrag walks (the product reframe and its gate) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A deterministic `singularrag walk` command that checks, for a file of questions a person would bring, that the UI's own data paths reach the answer, plus the product reframe it serves: tier two retired as a gate, the agent tools frozen and trimmed, three corpus walk files with a recorded baseline.

**Architecture:** Walk files are TOML in core (`walks.rs`: types, parse, validate). The runner lives in the CLI crate (`walk.rs`) because it calls the same functions the serve routes call: the engine's `repo_map` for `locate`, `queries::processes` for `process`, `queries::entities` and core `blast_radius` for `connection`, core `changed` for `change`. Each kind has a pure grader over the DTO it reads, so graders are unit-tested on hand-built data and the runner is integration-tested on the fixtures with the checked-in extraction. The report mirrors tier one's table.

**Tech Stack:** Rust (clap, serde, toml, rusqlite), `git` through `std::process::Command` for change walks, `assert_cmd` for CLI tests.

**Spec:** `docs/superpowers/specs/2026-10-12-singularrag-walks-design.md`

## Global Constraints

- Gold is always `path::name` (a section's heading text or a symbol's declared name), never text; a walk fails only when its path cannot reach the answer within its bound.
- Four kinds exactly: `locate` (bound: the budget), `process` (bound: the processes query's cap), `connection` (bound: `hops`, default 2), `change` (bound: the patch). Unknown kinds are a validation error.
- The runner calls what the routes call: `Engine::repo_map` with `map::clamp_budget` (locate), `serve::queries::processes` (process), `serve::queries::entities` and `singularrag_core::blast::blast_radius` with `MAX_DEPTH`/`MAX_FILES` (connection), `singularrag_core::changed::changed` (change). No DOM, no browser, no model in the grader.
- `locate` and `connection` make at most the one query embedding the panel makes; `--no-models` turns it off; `process` and `change` never call a model; the walk never triggers extraction.
- `change` walks: `git apply --check` then `git apply`, then `git checkout -- <paths>` and `git clean -fd -- <paths>` for the paths the patch names; a dirty tree (`git status --porcelain --untracked-files=all` non-empty) before the walk is a refusal with a message, not a run; never on the repository that holds the walk file (the runner refuses when the corpus root contains the walk file's directory).
- Every failing row carries a reason in the exact forms: `gold not served at <budget>`, `no process cites <gold>`, `step with role <role> not found`, `step text "<text>" not found`, `<gold> not within <hops> hops of <from>`, `<gold> not in the changed list`, `patch does not apply: <git message>`, `checkout is dirty: refused`.
- Footer: `walks N/M` overall, preceded by one `walks N/M <kind>` line per kind present, in the order locate, process, connection, change.
- Walk files: never edit an existing id, add new ones; ids unique; `gold` non-empty; `from` required for `connection`; `patch` required for `change`; `query` required for `locate` and `process`.
- `INSTRUCTIONS` and the six tool descriptions in `crates/singularrag/src/mcp/server.rs` stay byte-identical to their copies in `crates/singularrag/tests/mcp.rs` and the `#[tool(description = …)]` literals; the tool count stays 6.
- Gate before every commit: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`. No UI change in this plan.
- The Bash guard rejects command lines containing the substring `eval`: use globs (`e*/README.md`, `e*/walks-ci.toml`) or a script file in the scratchpad.

## Review Focus

1. A `process` walk whose gold section is cited by a process the model named differently from the document (`First day` for Onboarding): must pass, because the check is on the cited section, not the name. Test in Task 2 (`a_process_walk_passes_through_a_process_named_differently`).
2. A `connection` walk from an entity that exists under two norm-names in different sections (`Finance` as organisation and as role): the start must resolve to the entity whose neighbourhood reaches gold, not the first by id. Test in Task 3 (`an_entity_start_tries_every_entity_of_that_name`).
3. A `change` patch that applies cleanly but touches a file outside the index (a binary, a secret-like file): the walk must still revert every path it touched, including new files. Test in Task 4 (`a_patch_adding_a_new_file_is_reverted_with_clean`).
4. A walk file that names a budget on a `process` walk or `hops` on a `locate` walk: ignored silently would hide a typo; it is a validation error. Test in Task 1 (`fields_that_do_not_belong_to_the_kind_are_errors`).
5. `--no-models` on a `connection` walk whose entity start matches only by vector: the walk must fail with `<from> not found`, not panic or match nothing silently. Test in Task 3 (`an_unknown_entity_start_fails_with_not_found`).

---

## File structure

- `crates/singularrag-core/src/walks.rs` (new): `WalkKind`, `Walk`, `WalkFile`, `load_walks`, validation.
- `crates/singularrag-core/src/lib.rs`: `pub mod walks;`.
- `crates/singularrag/src/walk.rs` (new): `WalkResult`, graders (`grade_locate`, `grade_process`, `grade_connection`, `grade_change`), `run_walks`, `render_report`, the change runner's git helpers.
- `crates/singularrag/src/main.rs`: `Cmd::Walk { questions, budget, json, no_models, extraction }` and its arm; `mod walk;`.
- `crates/singularrag/src/serve/queries.rs`: no change (its `processes` and `entities` are already `pub`); `crates/singularrag/src/serve/mod.rs` must expose `queries` to the crate (`pub(crate) mod queries` if it is private).
- `crates/singularrag/tests/cli.rs`: the walk CLI tests; `crates/singularrag-core/fixtures/prose/extraction.json` unchanged; `eval/walks-ci.toml` and `eval/patches/ci-session.patch` (new).
- `crates/singularrag/src/mcp/server.rs`, `crates/singularrag/tests/mcp.rs`: shortened `INSTRUCTIONS` and descriptions.
- `eval/walks-repo.toml`, `eval/walks-handbook.toml`, `eval/walks-hono.toml`, `eval/patches/*.patch`, `eval/README.md`, `docs/superpowers/specs/2026-09-19-singularrag-design.md` (§2, §3, §12 amendments), `README.md`.

---

### Task 1: Walk file types, parsing and validation

**Files:**
- Create: `crates/singularrag-core/src/walks.rs`
- Modify: `crates/singularrag-core/src/lib.rs` (`pub mod walks;`)
- Test: `walks.rs` `mod tests`

**Interfaces:**
- Consumes: `crate::{Error, Result}`, `toml`, `serde`.
- Produces:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WalkKind { Locate, Process, Connection, Change }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Walk {
    pub id: String,
    pub kind: WalkKind,
    #[serde(default)] pub query: Option<String>,
    pub gold: Vec<String>,
    #[serde(default)] pub budget: Option<usize>,   // locate only
    #[serde(default)] pub role: Option<String>,    // process only
    #[serde(default)] pub step_text: Option<String>, // process only
    #[serde(default)] pub from: Option<String>,    // connection only
    #[serde(default)] pub hops: Option<u32>,       // connection only
    #[serde(default)] pub patch: Option<String>,   // change only, relative to the walk file
}

#[derive(Debug, Clone, Deserialize)]
pub struct WalkFile {
    #[serde(default)] pub budget: Option<usize>,
    pub walk: Vec<Walk>,
}

pub const DEFAULT_HOPS: u32 = 2;
pub fn load_walks(path: &Path) -> Result<WalkFile>;      // parse + validate
pub fn validate(file: &WalkFile) -> Result<()>;          // the rules below
impl Walk { pub fn hops(&self) -> u32; pub fn budget_or(&self, default: usize) -> usize; }
```

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Result<WalkFile> {
        let f: WalkFile = toml::from_str(s).map_err(|e| Error::Config(e.to_string()))?;
        validate(&f)?;
        Ok(f)
    }

    #[test]
    fn a_valid_file_of_all_four_kinds_parses() {
        let f = parse(r#"
budget = 2048
[[walk]]
id = "W1"
kind = "locate"
query = "how do I get reimbursed"
gold = ["docs/handbook.md::Expense process"]
[[walk]]
id = "W2"
kind = "process"
query = "what happens after approval"
role = "Finance"
step_text = "within 5 days"
gold = ["docs/handbook.md::Expense process"]
[[walk]]
id = "W3"
kind = "connection"
from = "Okta"
gold = ["docs/handbook.md::Onboarding"]
[[walk]]
id = "W4"
kind = "change"
patch = "patches/W4.patch"
gold = ["src/auth/session.ts::createSession"]
"#).unwrap();
        assert_eq!(f.walk.len(), 4);
        assert_eq!(f.walk[0].budget_or(f.budget.unwrap()), 2048);
        assert_eq!(f.walk[2].hops(), DEFAULT_HOPS);
        assert_eq!(f.walk[3].kind, WalkKind::Change);
    }

    #[test]
    fn missing_required_fields_are_errors() {
        for (body, msg) in [
            ("id = \"A\"\nkind = \"locate\"\ngold = [\"x::y\"]", "A: locate needs query"),
            ("id = \"A\"\nkind = \"process\"\ngold = [\"x::y\"]", "A: process needs query"),
            ("id = \"A\"\nkind = \"connection\"\ngold = [\"x::y\"]", "A: connection needs from"),
            ("id = \"A\"\nkind = \"change\"\ngold = [\"x::y\"]", "A: change needs patch"),
            ("id = \"A\"\nkind = \"locate\"\nquery = \"q\"\ngold = []", "A: gold is empty"),
            ("id = \"A\"\nkind = \"locate\"\nquery = \"q\"\ngold = [\"no-separator\"]", "A: gold \"no-separator\" is not path::name"),
        ] {
            let e = parse(&format!("[[walk]]\n{body}\n")).unwrap_err().to_string();
            assert!(e.contains(msg), "{e}");
        }
    }

    #[test]
    fn duplicate_ids_and_unknown_kinds_are_errors() {
        let e = parse("[[walk]]\nid = \"A\"\nkind = \"locate\"\nquery = \"q\"\ngold = [\"x::y\"]\n[[walk]]\nid = \"A\"\nkind = \"locate\"\nquery = \"q\"\ngold = [\"x::y\"]\n").unwrap_err().to_string();
        assert!(e.contains("duplicate id A"), "{e}");
        let e = parse("[[walk]]\nid = \"A\"\nkind = \"browse\"\nquery = \"q\"\ngold = [\"x::y\"]\n").unwrap_err().to_string();
        assert!(e.contains("browse"), "{e}");
    }

    #[test]
    fn fields_that_do_not_belong_to_the_kind_are_errors() {
        let e = parse("[[walk]]\nid = \"A\"\nkind = \"process\"\nquery = \"q\"\nbudget = 512\ngold = [\"x::y\"]\n").unwrap_err().to_string();
        assert!(e.contains("A: budget belongs to locate walks only"), "{e}");
        let e = parse("[[walk]]\nid = \"A\"\nkind = \"locate\"\nquery = \"q\"\nhops = 3\ngold = [\"x::y\"]\n").unwrap_err().to_string();
        assert!(e.contains("A: hops belongs to connection walks only"), "{e}");
    }
}
```

- [ ] **Step 2: Run** — `cargo test -p singularrag-core walks` → compile errors (no module).

- [ ] **Step 3: Implement** `walks.rs`: the types above; `validate` checks in order per walk: unique id (`duplicate id {id}`), `gold` non-empty, every gold contains `::`, the required field for the kind, and the field-ownership rules (`budget` only on locate; `role`/`step_text` only on process; `from`/`hops` only on connection; `patch` only on change), each an `Error::Config` with the messages the tests name. `load_walks` reads the file, parses (`Error::Config(format!("{}: {e}", path.display()))`), validates. `hops()` = `self.hops.unwrap_or(DEFAULT_HOPS)`; `budget_or(d)` = `self.budget.unwrap_or(d)`.

- [ ] **Step 4: Run** — `cargo test -p singularrag-core walks` → pass.

- [ ] **Step 5: Commit** — `feat(core): walk files: four kinds, parse and validate`

---

### Task 2: The runner, the `locate` and `process` graders, the report, the `walk` command

**Files:**
- Create: `crates/singularrag/src/walk.rs`
- Modify: `crates/singularrag/src/main.rs` (`mod walk;`, `Cmd::Walk`, its arm), `crates/singularrag/src/serve/mod.rs` (expose `queries` to the crate if private)
- Test: `walk.rs` `mod tests`

**Interfaces:**
- Consumes: `singularrag_core::walks::{Walk, WalkKind, WalkFile, load_walks}`, `Engine::{repo_map, store}`, `map::clamp_budget`, `serve::queries::{processes, ProcessesDto}`, `eval`'s `served_keys` pattern (reimplemented here: `SELECT path || '::' || name FROM retrieval_items WHERE retrieval_id = ?1 AND served = 1`).
- Produces:

```rust
#[derive(Debug, Clone, Serialize)]
pub struct WalkResult {
    pub id: String,
    pub kind: WalkKind,
    pub pass: bool,
    /// What the path reached: served keys, the process name, the neighbours, the changed keys.
    pub reached: Vec<String>,
    pub reason: Option<String>,
}
pub fn grade_locate(gold: &[String], served: &[String], budget: usize) -> WalkResult-ish (pass, reached, reason);
pub fn grade_process(w: &Walk, dto: &ProcessesDto) -> (bool, Vec<String>, Option<String>);
pub struct WalkOptions { pub budget: usize, pub walk_dir: PathBuf }   // walk_dir: the walk file's directory, for patches
pub fn run_walks(engine: &mut Engine, file: &WalkFile, opts: &WalkOptions) -> Result<Vec<WalkResult>>;
pub fn render_report(results: &[WalkResult]) -> String;
```

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn locate_passes_when_any_gold_is_served_and_names_the_budget_when_not() {
    let gold = vec!["docs/a.md::One".to_string(), "docs/b.md::Two".to_string()];
    let (pass, reached, reason) = grade_locate(&gold, &["docs/x.md::X".into(), "docs/b.md::Two".into()], 2048);
    assert!(pass && reason.is_none());
    assert_eq!(reached, vec!["docs/x.md::X", "docs/b.md::Two"]);
    let (pass, _, reason) = grade_locate(&gold, &["docs/x.md::X".into()], 2048);
    assert!(!pass);
    assert_eq!(reason.as_deref(), Some("gold not served at 2048"));
}

fn dto(processes: Vec<(&str, Vec<(&str, &str, &str)>)>) -> ProcessesDto {
    // (process name, [(step text, role, section path::name)])
    ProcessesDto { processes: processes.into_iter().enumerate().map(|(i, (name, steps))| ProcessDto {
        id: i as i64 + 1, name: name.into(), description: String::new(),
        roles: steps.iter().map(|s| s.1.to_string()).filter(|r| !r.is_empty()).collect(),
        steps: steps.into_iter().enumerate().map(|(j, (text, role, sec))| {
            let (path, name) = sec.split_once("::").unwrap();
            StepDto { ordinal: j + 1, text: text.into(), role: role.into(), systems: vec![],
                      section: SectionRefDto { symbol_id: 10 + j as i64, path: path.into(), name: name.into(), line: 1 },
                      code: vec![], more_code: 0 }
        }).collect(),
    }).collect(), truncated: false }
}

fn walk(kind: WalkKind, gold: &[&str]) -> Walk {
    Walk { id: "W".into(), kind, query: Some("q".into()), gold: gold.iter().map(|g| g.to_string()).collect(),
           budget: None, role: None, step_text: None, from: None, hops: None, patch: None }
}

#[test]
fn a_process_walk_passes_through_a_process_named_differently() {
    let d = dto(vec![("First day", vec![("Activate Okta", "new colleague", "docs/handbook.md::Onboarding")])]);
    let (pass, reached, _) = grade_process(&walk(WalkKind::Process, &["docs/handbook.md::Onboarding"]), &d);
    assert!(pass);
    assert_eq!(reached, vec!["First day"]);
}

#[test]
fn a_process_walk_checks_role_and_step_text_within_the_citing_process() {
    let d = dto(vec![
        ("Expense process", vec![("Submit in Expensify", "you", "docs/handbook.md::Expense process"),
                                 ("Finance reviews within 5 days", "Finance", "docs/handbook.md::Expense process")]),
        ("Release process", vec![("Cut the branch", "Finance", "docs/handbook.md::Release process")]),
    ]);
    let mut w = walk(WalkKind::Process, &["docs/handbook.md::Expense process"]);
    w.role = Some("finance".into());
    w.step_text = Some("WITHIN 5 DAYS".into());
    assert!(grade_process(&w, &d).0, "role and step text match case-insensitively");
    w.role = Some("Manager".into());
    let (pass, _, reason) = grade_process(&w, &d);
    assert!(!pass);
    assert_eq!(reason.as_deref(), Some("step with role Manager not found"));
    w.role = None;
    w.step_text = Some("rolls back".into());
    assert_eq!(grade_process(&w, &d).2.as_deref(), Some("step text \"rolls back\" not found"));
    let w = walk(WalkKind::Process, &["docs/handbook.md::Incident process"]);
    assert_eq!(grade_process(&w, &d).2.as_deref(), Some("no process cites docs/handbook.md::Incident process"));
}

#[test]
fn the_report_has_one_row_per_walk_and_a_footer_per_kind() {
    let rs = vec![
        WalkResult { id: "W1".into(), kind: WalkKind::Locate, pass: true, reached: vec!["a::b".into()], reason: None },
        WalkResult { id: "W2".into(), kind: WalkKind::Process, pass: false, reached: vec![], reason: Some("no process cites x::y".into()) },
    ];
    let out = render_report(&rs);
    assert!(out.starts_with("id    kind        pass  reached / reason\n"), "{out}");
    assert!(out.contains("W1    locate      yes   a::b\n"), "{out}");
    assert!(out.contains("W2    process     no    no process cites x::y\n"), "{out}");
    assert!(out.ends_with("walks 1/1 locate\nwalks 0/1 process\nwalks 1/2\n"), "{out}");
}
```

- [ ] **Step 2: Run** — compile errors.

- [ ] **Step 3: Implement**

`walk.rs`:

```rust
pub fn grade_locate(gold: &[String], served: &[String], budget: usize) -> (bool, Vec<String>, Option<String>) {
    let pass = gold.iter().any(|g| served.contains(g));
    (pass, served.to_vec(), (!pass).then(|| format!("gold not served at {budget}")))
}

fn eq_ci(a: &str, b: &str) -> bool { a.trim().eq_ignore_ascii_case(b.trim()) }
fn contains_ci(hay: &str, needle: &str) -> bool { hay.to_lowercase().contains(&needle.trim().to_lowercase()) }

pub fn grade_process(w: &Walk, dto: &ProcessesDto) -> (bool, Vec<String>, Option<String>) {
    let cites = |p: &ProcessDto| p.steps.iter().any(|s| w.gold.iter().any(|g| format!("{}::{}", s.section.path, s.section.name) == *g));
    let citing: Vec<&ProcessDto> = dto.processes.iter().filter(|p| cites(p)).collect();
    if citing.is_empty() {
        return (false, vec![], Some(format!("no process cites {}", w.gold.join(", "))));
    }
    let reached: Vec<String> = citing.iter().map(|p| p.name.clone()).collect();
    if let Some(role) = &w.role {
        if !citing.iter().any(|p| p.steps.iter().any(|s| eq_ci(&s.role, role))) {
            return (false, reached, Some(format!("step with role {role} not found")));
        }
    }
    if let Some(text) = &w.step_text {
        if !citing.iter().any(|p| p.steps.iter().any(|s| contains_ci(&s.text, text))) {
            return (false, reached, Some(format!("step text \"{text}\" not found")));
        }
    }
    (true, reached, None)
}
```

`run_walks`: for each walk by kind: `Locate` → `engine.repo_map(&MapRequest { query: w.query.clone(), budget_tokens: map::clamp_budget(w.budget_or(opts.budget)), ..Default::default() })`, served keys from `retrieval_items` (the `eval` SQL), `grade_locate`; `Process` → `queries::processes(engine.store())`, `grade_process`; `Connection` and `Change` → Task 3 and 4 (until then return `pass: false, reason: Some("not implemented")`). Build `WalkResult { id, kind, pass, reached, reason }`.

`render_report`: header `id    kind        pass  reached / reason`; row `format!("{:<5} {:<11} {:<5} {}\n", id, kind, if pass {"yes"} else {"no"}, if pass { reached.join(", ") } else { reason })` with `kind` lowercase; footer one `walks N/M <kind>` per kind present in the order locate, process, connection, change, then `walks N/M`.

`main.rs`:

```rust
/// Walk the UI's own data paths for a file of questions a person would bring (spec walks §3)
Walk {
    #[arg(long)] questions: PathBuf,
    #[arg(long, default_value_t = DEFAULT_BUDGET)] budget: usize,
    #[arg(long)] json: bool,
    #[arg(long)] no_models: bool,
    #[arg(long, value_name = "PATH")] extraction: Option<PathBuf>,
},
```

Arm: mirror `Cmd::Eval` (models off, extraction load after a refresh, then `let file = load_walks(&questions)?; let opts = WalkOptions { budget, walk_dir: questions.parent().map(Path::to_path_buf).unwrap_or_default() }; let results = walk::run_walks(&mut engine, &file, &opts)?;` then JSON or report). The engine is opened as the CLI opens it today (`Engine::open(&root, "walk")`: change the session key for this command only to the literal `"walk"`, so the rail labels these retrievals `walk`; the other commands keep `cli-<pid>`).

- [ ] **Step 4: Run** — `cargo test -p singularrag --bin singularrag walk` → pass; then the gate.

- [ ] **Step 5: Commit** — `feat(cli): singularrag walk — locate and process walks, the report`

---

### Task 3: The `connection` grader

**Files:**
- Modify: `crates/singularrag/src/walk.rs`
- Test: `walk.rs` `mod tests`

**Interfaces:**
- Consumes: `serve::queries::{entities, EntitiesDto}`, `singularrag_core::blast::{blast_radius, MAX_FILES, Blast}`, `MapConfig::load`, `knowledge::norm_name`.
- Produces: `pub fn grade_connection_entity(w: &Walk, dto: &EntitiesDto) -> (bool, Vec<String>, Option<String>)` and `pub fn grade_connection_symbol(w: &Walk, start: (&str, &str), blast: Option<&Blast>) -> (bool, Vec<String>, Option<String>)`, plus the dispatch in `run_walks`.

Rule: `from` containing `::` is a symbol start `path::name`; otherwise an entity start. Entity start: every entity whose `norm_name(name) == norm_name(from)` is tried (Review Focus 2); breadth-first over the DTO: from an entity, hop 1 reaches its mention sections (`path::name` of each mention) and its related entities (either end of a relation); hop 2 reaches those entities' mentions and relations; and so on to `hops`. Pass when a gold key is reached. `reached` lists the keys reached in BFS order (cap 50). Symbol start: `blast_radius(store, &config, path, name, hops.min(MAX_DEPTH), MAX_FILES)`; pass when a gold key's path equals the start's path or is among `blast.files` with `depth <= hops`; `reached` = the files' paths. Reasons: `<from> not found` when no entity of that name / `blast_radius` is `None`; `<gold> not within <hops> hops of <from>` otherwise (first gold).

- [ ] **Step 1: Write the failing tests**

```rust
fn edto(entities: &[(i64, &str, &str)], relations: &[(i64, i64)], mentions: &[(i64, &str)]) -> EntitiesDto {
    EntitiesDto {
        entities: entities.iter().map(|(id, n, t)| EntityDto { id: *id, name: n.to_string(), r#type: t.to_string(), description: String::new(), mentions: 1 }).collect(),
        relations: relations.iter().enumerate().map(|(i, (s, d))| EntityRelationDto { id: i as i64, src: *s, dst: *d, description: String::new(), symbol_id: 0, path: "p.md".into(), name: "s".into() }).collect(),
        mentions: mentions.iter().map(|(e, key)| { let (p, n) = key.split_once("::").unwrap(); EntityMentionDto { entity_id: *e, symbol_id: 0, path: p.into(), name: n.into() } }).collect(),
        truncated: false,
    }
}

#[test]
fn an_entity_reaches_its_mentions_at_one_hop_and_its_relations_mentions_at_two() {
    let d = edto(&[(1, "Okta", "system"), (2, "IT desk", "organisation")], &[(2, 1)],
                 &[(1, "docs/handbook.md::Onboarding"), (2, "docs/it.md::Desk")]);
    let mut w = walk(WalkKind::Connection, &["docs/it.md::Desk"]);
    w.from = Some("okta".into());
    w.hops = Some(1);
    let (pass, _, reason) = grade_connection_entity(&w, &d);
    assert!(!pass);
    assert_eq!(reason.as_deref(), Some("docs/it.md::Desk not within 1 hops of okta"));
    w.hops = Some(2);
    let (pass, reached, _) = grade_connection_entity(&w, &d);
    assert!(pass, "{reached:?}");
}

#[test]
fn an_entity_start_tries_every_entity_of_that_name() {
    let d = edto(&[(1, "Finance", "organisation"), (2, "Finance", "role")], &[],
                 &[(1, "docs/a.md::Budget"), (2, "docs/handbook.md::Expense process")]);
    let mut w = walk(WalkKind::Connection, &["docs/handbook.md::Expense process"]);
    w.from = Some("Finance".into());
    assert!(grade_connection_entity(&w, &d).0);
}

#[test]
fn an_unknown_entity_start_fails_with_not_found() {
    let d = edto(&[(1, "Okta", "system")], &[], &[(1, "docs/handbook.md::Onboarding")]);
    let mut w = walk(WalkKind::Connection, &["docs/handbook.md::Onboarding"]);
    w.from = Some("Workday".into());
    assert_eq!(grade_connection_entity(&w, &d).2.as_deref(), Some("Workday not found"));
}

#[test]
fn a_symbol_start_uses_the_blast_radius_within_hops() {
    use singularrag_core::blast::{Blast, BlastFile};
    let b = Blast { root_path: "src/auth/session.ts".into(), root_symbol: "createSession".into(),
                    files: vec![BlastFile { path: "src/http/middleware.ts".into(), depth: 1, via: "createSession".into() },
                                BlastFile { path: "src/http/routes.ts".into(), depth: 2, via: "requireSession".into() }],
                    truncated: None };
    let mut w = walk(WalkKind::Connection, &["src/http/routes.ts::route"]);
    w.from = Some("src/auth/session.ts::createSession".into());
    w.hops = Some(1);
    assert!(!grade_connection_symbol(&w, ("src/auth/session.ts", "createSession"), Some(&b)).0);
    w.hops = Some(2);
    assert!(grade_connection_symbol(&w, ("src/auth/session.ts", "createSession"), Some(&b)).0);
    assert_eq!(grade_connection_symbol(&w, ("src/x.ts", "nope"), None).2.as_deref(), Some("src/x.ts::nope not found"));
}
```

- [ ] **Step 2: Run** — compile errors.

- [ ] **Step 3: Implement** the two graders as specified (BFS with a `HashSet<i64>` of visited entities and a `Vec<String>` of reached keys; a mention key is `format!("{}::{}", m.path, m.name)`), and the dispatch in `run_walks`: entity start → `queries::entities(engine.store())`; symbol start → `MapConfig::load(root)` and `blast_radius(engine.store(), &config, path, name, w.hops().min(MAX_DEPTH), MAX_FILES)`. `--no-models` changes nothing here (matching is by name); note that in the module doc.

- [ ] **Step 4: Run** — tests and the gate.

- [ ] **Step 5: Commit** — `feat(cli): connection walks over entities, mentions and the blast radius`

---

### Task 4: The `change` runner

**Files:**
- Modify: `crates/singularrag/src/walk.rs`
- Test: `walk.rs` `mod tests` (git helpers on a temp repo) and `crates/singularrag/tests/cli.rs`

**Interfaces:**
- Consumes: `singularrag_core::changed::{changed, Changed}`, `MapConfig`, `Workspace`, `std::process::Command`.
- Produces: `pub fn grade_change(gold: &[String], c: &Changed) -> (bool, Vec<String>, Option<String>)`; `pub fn patch_paths(patch: &str) -> Vec<String>` (every `+++ b/<path>` and `--- a/<path>`, deduped, without `/dev/null`); `fn git(root, args) -> Result<String>`; `fn is_clean(root) -> Result<bool>`; `fn apply_patch(root, patch_file) -> Result<()>` (`--check` then apply); `fn revert(root, paths)`.

Rule: `reached` = every `format!("{}::{}", s.path, s.name)` in `c.symbols`; pass when every gold key is among them (a change walk asks "does the list contain all of these", unlike locate's "any"). Reason: `<first missing gold> not in the changed list`. Refusals before any write: the corpus root must not contain `opts.walk_dir` (`walk file lives inside the corpus: refused`), and `is_clean` must hold (`checkout is dirty: refused`); a patch that fails `--check` gives `patch does not apply: <git stderr first line>`. Revert always runs after `changed`, even when it errs (use a guard struct whose `Drop` runs `revert`).

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn patch_paths_lists_touched_files_once_including_new_ones() {
    let p = "--- a/src/a.ts\n+++ b/src/a.ts\n@@ -1 +1 @@\n-x\n+y\n--- /dev/null\n+++ b/src/new.ts\n@@ -0,0 +1 @@\n+z\n";
    assert_eq!(patch_paths(p), vec!["src/a.ts", "src/new.ts"]);
}

#[test]
fn grade_change_needs_every_gold_in_the_list() {
    use singularrag_core::changed::{Changed, ChangedSymbol};
    let sym = |p: &str, n: &str| ChangedSymbol { symbol_id: 1, path: p.into(), name: n.into(), kind: "function".into(), signature: String::new(), line_start: 1, line_end: 2, referenced_by: vec![] };
    let c = Changed { base: "HEAD".into(), symbols: vec![sym("src/a.ts", "a"), sym("src/b.ts", "b")], files_without_symbols: vec![], truncated: false };
    assert!(grade_change(&["src/a.ts::a".into()], &c).0);
    let (pass, _, reason) = grade_change(&["src/a.ts::a".into(), "src/c.ts::c".into()], &c);
    assert!(!pass);
    assert_eq!(reason.as_deref(), Some("src/c.ts::c not in the changed list"));
}

#[test]
fn a_patch_adding_a_new_file_is_reverted_with_clean() {
    // temp git repo with one committed file; a patch that edits it and adds a new file;
    // after apply_patch + revert the tree is clean and the new file is gone.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("a.txt"), "x\n").unwrap();
    for args in [vec!["init", "-q"], vec!["add", "."], vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "one"]] {
        git(root, &args).unwrap();
    }
    let patch = root.join("p.patch");
    std::fs::write(&patch, "--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-x\n+y\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1 @@\n+z\n").unwrap();
    assert!(is_clean(root).unwrap());
    apply_patch(root, &patch).unwrap();
    assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "y\n");
    assert!(root.join("new.txt").exists());
    revert(root, &["a.txt".into(), "new.txt".into()]).unwrap();
    assert!(is_clean(root).unwrap());
    assert!(!root.join("new.txt").exists());
}

#[test]
fn a_dirty_checkout_is_refused_before_any_write() {
    // same temp repo, an uncommitted edit; run_change (the per-walk fn) returns the refusal reason and leaves the edit alone.
}
```

- [ ] **Step 2: Run** — compile errors.

- [ ] **Step 3: Implement**

```rust
fn git(root: &Path, args: &[&str]) -> Result<String> {
    let out = std::process::Command::new("git").arg("-C").arg(root).args(args).output()?;
    if !out.status.success() {
        return Err(Error::Config(format!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).lines().next().unwrap_or("").trim())));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}
fn is_clean(root: &Path) -> Result<bool> { Ok(git(root, &["status", "--porcelain", "--untracked-files=all"])?.trim().is_empty()) }
fn apply_patch(root: &Path, patch: &Path) -> Result<()> {
    let p = patch.to_string_lossy();
    git(root, &["apply", "--check", &p]).map_err(|e| Error::Config(format!("patch does not apply: {e}")))?;
    git(root, &["apply", &p]).map(|_| ())
}
/// Tracked edits go back with `checkout`; a file the patch added has nothing to check out,
/// so that error is ignored and `clean` removes it.
fn revert(root: &Path, paths: &[String]) -> Result<()> {
    let mut args: Vec<&str> = vec!["checkout", "--"];
    args.extend(paths.iter().map(String::as_str));
    let _ = git(root, &args);
    let mut args: Vec<&str> = vec!["clean", "-fd", "--"];
    args.extend(paths.iter().map(String::as_str));
    git(root, &args).map(|_| ())
}
``` `patch_paths`: lines starting with `+++ b/` or `--- a/`, strip the prefix, skip `/dev/null`, dedup preserving order. The per-walk `run_change(engine, root, w, opts)`: refusals → `WalkResult` with the reason; else `apply_patch`, a `Revert` guard holding `(root, paths)`, `let c = changed(engine.store(), &MapConfig::load(root)?, engine.workspace(), None)?` (`Engine::workspace()` and `Engine::store()` exist; call `engine.refresh(Duration::from_secs(600))` first so the patched files are indexed, then the core `changed(...)` directly), `grade_change`. Also refuse when `opts.walk_dir.starts_with(root)` (`walk file lives inside the corpus: refused`).

- [ ] **Step 4: Run** — tests and the gate.

- [ ] **Step 5: Commit** — `feat(cli): change walks apply a patch, read the changed list, and revert`

---

### Task 5: The CI walk file and the CLI test

**Files:**
- Create: `eval/walks-ci.toml`, `eval/patches/ci-session.patch`
- Modify: `crates/singularrag/tests/cli.rs`
- Test: `cli.rs`

**Interfaces:**
- Consumes: `fixture::write_docs_mini` (code + `docs/design.md` with `## Freshness`, `## Storage`), `fixture::write_prose`, the prose `extraction.json`; the `walk` command.

- [ ] **Step 1: Write the failing test** (`cli.rs`)

```rust
fn walks_ci_path() -> &'static str { concat!(env!("CARGO_MANIFEST_DIR"), "/../../e", "val/walks-ci.toml") }
fn prose_extraction_path() -> &'static str { concat!(env!("CARGO_MANIFEST_DIR"), "/../singularrag-core/fixtures/prose/extraction.json") }

#[test]
fn walk_runs_all_four_kinds_on_the_fixtures_without_a_model() {
    let dir = tempfile::tempdir().unwrap();
    singularrag_core::fixture::write_docs_mini(dir.path());
    singularrag_core::fixture::write_prose(dir.path());
    // the change walk needs a git checkout: init and commit the fixture
    for args in [vec!["init", "-q"], vec!["add", "."], vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "fixture"]] {
        assert!(std::process::Command::new("git").arg("-C").arg(dir.path()).args(&args).status().unwrap().success());
    }
    let out = Command::cargo_bin("singularrag").unwrap()
        .env("SINGULARRAG_OLLAMA_URL", "http://127.0.0.1:9")
        .args(["walk", "--questions", walks_ci_path(), "--budget", "4096", "--no-models",
               "--extraction", prose_extraction_path(), "--repo", dir.path().to_str().unwrap()])
        .assert().success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(stdout.lines().any(|l| l == "walks 1/1 locate"), "{stdout}");
    assert!(stdout.lines().any(|l| l == "walks 1/1 process"), "{stdout}");
    assert!(stdout.lines().any(|l| l == "walks 2/2 connection"), "{stdout}");
    assert!(stdout.lines().any(|l| l == "walks 1/1 change"), "{stdout}");
    assert!(stdout.ends_with("walks 5/5\n"), "{stdout}");
    // the checkout is clean again after the change walk
    let st = std::process::Command::new("git").arg("-C").arg(dir.path()).args(["status", "--porcelain", "--untracked-files=all"]).output().unwrap();
    assert!(st.stdout.is_empty());
}

#[test]
fn walk_names_the_reason_when_gold_is_wrong() {
    // a scratch walk file in the tempdir's parent with one locate walk whose gold is docs/handbook.md::Nowhere → "gold not served at 4096" and "walks 0/1"
}
```

- [ ] **Step 2: Run** — fails (no walk file).

- [ ] **Step 3: Implement** `eval/walks-ci.toml`:

```toml
# The harness's own walk file: the prose fixture (`fixture::write_prose`) plus the code and docs
# mini fixture (`fixture::write_docs_mini`), the checked-in prose extraction loaded with
# --extraction, no model. One walk per kind (two for connection: an entity and a symbol start).
budget = 4096

[[walk]]
id = "C1"
kind = "locate"
query = "how do I get reimbursed after paying for something myself on the job"
gold = ["docs/handbook.md::Expense process"]

[[walk]]
id = "C2"
kind = "process"
query = "what happens after my manager approves an expense claim"
role = "Finance"
step_text = "within 5 days"
gold = ["docs/handbook.md::Expense process"]

[[walk]]
id = "C3"
kind = "connection"
from = "Okta"
gold = ["docs/handbook.md::Onboarding"]

[[walk]]
id = "C4"
kind = "connection"
from = "src/auth/session.ts::createSession"
hops = 1
gold = ["src/http/middleware.ts::requireSession"]

[[walk]]
id = "C5"
kind = "change"
patch = "patches/ci-session.patch"
gold = ["src/auth/session.ts::createSession"]
```

`eval/patches/ci-session.patch`: a unified diff that changes one line inside `createSession` in `src/auth/session.ts` as `write_ts_mini` writes it (read `fixture.rs` for the exact text and generate the patch with `git diff` on a scratch checkout of the fixture, so it applies). Check the role name the prose extraction gives the Finance step (`extraction.json`, `docs/handbook.md::Expense process` → `steps`), and adjust `role`/`step_text` in C2 to what the fixture holds, not the other way round.

- [ ] **Step 4: Run** — `cargo test -p singularrag --test cli walk` → pass; the gate.

- [ ] **Step 5: Commit** — `test: the CI walk file runs all four kinds on the fixtures without a model`

---

### Task 6: Shortened tool descriptions, tier two retired as a gate, parent spec amendments

**Files:**
- Modify: `crates/singularrag/src/mcp/server.rs` (`INSTRUCTIONS`, six constants, six `#[tool(description = …)]` literals), `crates/singularrag/tests/mcp.rs` (the copies), `eval/README.md` ("Tier two" opening paragraph), `docs/superpowers/specs/2026-09-19-singularrag-design.md` (§2, §3, §12), `README.md` (one paragraph under "Does it earn its place?")
- Test: `tests/mcp.rs` (the byte-identical test, updated copies), `tests/cli.rs` README tests

- [ ] **Step 1: Write the failing test** — update the `tests/mcp.rs` copies to the new texts below so `lists_exactly_the_six_tools_with_spec_descriptions` fails until `server.rs` matches; add to `tests/cli.rs` a `readme_says_who_the_product_is_for` asserting README contains "for a person understanding a corpus" and "`singularrag walk`".

- [ ] **Step 2: Run** — fails.

- [ ] **Step 3: Implement** — the new texts, verbatim in all copies:

```
INSTRUCTIONS = "singularrag indexes this workspace's code and documents. Call repo_map with your task before reading files; find_symbol looks a name up; trace_path shows how two symbols connect; changed lists what a diff touches; entities answers what the documents say about a person, system, concept or process (a process comes back as ordered steps); annotate leaves a note on a file or symbol. The first line of every answer says how fresh the index is."
REPO_MAP_DESCRIPTION = "Token-budgeted map of the code symbols and document sections most relevant to a task, each file with the files that reference it. `query`: a question or identifiers; `focus_files`: workspace-relative paths; `budget_tokens`: default 1024, up to 8192; `entities`, `themes`: optional names and phrases. Rows are `line  signature` or `line  ## heading`, never bodies."
FIND_SYMBOL_DESCRIPTION = "Looks a symbol or section name up and lists its definitions and the files that reference it. `name`: the exact or partial name."
ANNOTATE_DESCRIPTION = "Leaves a one-paragraph note on a file or symbol in map.toml; it shows in the next map with an (agent) tag. `path`, optional `symbol`, `note`."
TRACE_PATH_DESCRIPTION = "How two symbols connect through references. `from`, `to`: `path::name`."
ENTITIES_DESCRIPTION = "What the documents say about a person, system, concept, event or process, with the sections that say it. A process comes back as ordered steps with role, systems and implementing code. `query`: a name or a question; `entities`: names you know; `limit`: default 10, max 25. Descriptions are extracted text, not verified."
CHANGED_DESCRIPTION = "What your uncommitted or branch changes touch: each changed symbol or section with the files that reference it. `base`: a git ref, default the working tree against HEAD."
```

Check each existing `tests/mcp.rs` assertion on `INSTRUCTIONS` (`starts_with`/`ends_with`) and update them. `eval/README.md` "Tier two" section opens with: *Retired as a release gate 2026-10-12 (walks design): five runs on three corpora, the last on the GitLab handbook with both conditions at 1.00, showed that an agent with grep does not need the map; the runs below stay as the record, the harness stays for anyone measuring an agent, and no release is judged by it. The gate is tier one plus the walks.* Parent spec: §2 gains *Amended 2026-10-12 (walks design, `docs/superpowers/specs/2026-10-12-singularrag-walks-design.md`).* The user is a person understanding a corpus; the agent is a consumer of the same index, no longer the job. §3 gains the same header plus: the shared-map hypothesis did not hold for an agent with grep on corpora up to a thousand sections (five tier-two runs, `eval/README.md`); it is retired as the gate. §12 gains: the eval is tier one plus the walks (`singularrag walk`, one file per corpus, the per-release rule in the walks spec §5); tier two is retired as a gate. `README.md`: under "Does it earn its place?", replace the tier-two framing with one paragraph: who it is for, that tier one and `singularrag walk --questions eval/walks-<corpus>.toml` are the gates, and a pointer to the eval README for the tier-two record.

- [ ] **Step 4: Run** — the gate.

- [ ] **Step 5: Commit** — `docs+mcp: the product is for a person understanding a corpus; tool text trimmed; tier two retired as a gate`

---

### Task 7: The three corpus walk files, the baseline, the record

**Files:**
- Create: `eval/walks-repo.toml`, `eval/walks-handbook.toml`, `eval/walks-hono.toml`, `eval/patches/repo-*.patch` (three), `eval/patches/hono-*.patch` (three)
- Modify: `eval/README.md` (new section "Walks" before "Tier two")
- Test: `crates/singularrag/tests/cli.rs` (`the_corpus_walk_files_parse`: each file loads with `load_walks`, has 12 walks, ids unique across the file, and the kinds per file are as below)

**Corpora** (all pinned, all outside this repository): `../../singularrag-pinned` at `0c2ed54` (`corpus` branch; index from the real-model extraction, `.singularrag/` excluded), `../../handbook-corpus` at `7cf88f9` (extracted), `/Users/jonasbroms/Sites/hono` at `098e119` (indexed, not extracted; its `.singularrag/` must be ignored or excluded before change walks run: add it to `.git/info/exclude` there).

**Authoring rule per file** (twelve walks, three per kind where the corpus allows): write each walk from a page or file you read, gold verified with `singularrag --repo <corpus> find "<name>"` (sections print as `path:line  section  ## Heading`; the gold is `path::Heading`), `locate` queries in everyday words, `process` walks naming a role and a step phrase the page states, `connection` walks from an entity the extraction holds (check with `singularrag --repo <corpus> entities "<name>"`) or from a code symbol, `change` patches made with `git diff` from a real one-line edit inside a function or section of the pinned checkout (then reverted), gold = the symbol or section the edit sits in plus one referrer the `changed` output lists. Kinds per file: repo: 3 locate, 3 process, 3 connection, 3 change; handbook: 4 locate, 4 process, 4 connection; hono: 4 locate, 4 connection (symbol starts), 4 change. Three example walks to start from (verify before keeping):

```toml
# walks-repo.toml
[[walk]]
id = "R1"
kind = "locate"
query = "how fresh is the index and what does the STALE header promise"
gold = ["docs/superpowers/specs/2026-09-19-singularrag-design.md::8. Freshness"]

[[walk]]
id = "R4"
kind = "process"
query = "what happens after my manager approves an expense claim"
role = "Finance"
step_text = "5 days"
gold = ["docs/handbook.md::Expense process"]

[[walk]]
id = "R10"
kind = "change"
patch = "patches/repo-hook-run.patch"
gold = ["crates/singularrag/src/hook.rs::run"]
```

- [ ] **Step 1: Write the failing test** (`the_corpus_walk_files_parse`, kinds per file as above).
- [ ] **Step 2: Run** — fails (files missing).
- [ ] **Step 3: Author the three files and six patches** per the rule; then run each file seeds on and off: `singularrag --repo <corpus> walk --questions e*/walks-<corpus>.toml --budget 2048` and `--budget 4096`, and the same with `--no-models` (write the commands into a scratchpad script; the hono change walks need its tree clean). Record in `eval/README.md` under a new `## Walks` section placed before `## Tier two`: what a walk is (two sentences and a pointer to the spec), the recipe, one table per corpus with rows `walks N/M <kind>` at 2048 and 4096, seeds on and off, and one line per failing walk at 4096 seeds on with its reason verbatim. A failing walk is recorded, not fixed, in this plan (spec §4: the baseline is allowed to be poor).
- [ ] **Step 4: Run** — the gate; the new test passes.
- [ ] **Step 5: Commit** — `eval: the three corpus walk files and the walks baseline`

---

## Self-review

- Spec coverage: §1 → Task 6; §2 → Task 1 (file), Tasks 2–4 (kinds); §3 → Tasks 2–5 (runner, reasons, footer, `--extraction`); §4 → Tasks 5 (CI file) and 7 (corpora, baseline); §5 → Task 7's README text states the rule; §6 holds (no UI change); §7 → Task 6; §8 → Task 6; §9 → Task 4 (refusals, revert); §10 → every task's tests; §11 holds.
- Names: `WalkKind`, `Walk` (fields incl. `hops()`, `budget_or`), `WalkFile`, `load_walks`, `validate`, `DEFAULT_HOPS` (T1) in T2–T7; `WalkResult`, `WalkOptions`, `grade_locate`, `grade_process`, `run_walks`, `render_report` (T2) in T3–T5; `grade_connection_entity`, `grade_connection_symbol` (T3); `grade_change`, `patch_paths`, `git`, `is_clean`, `apply_patch`, `revert` (T4); DTO names `ProcessesDto`, `ProcessDto`, `StepDto`, `SectionRefDto`, `EntitiesDto`, `EntityDto`, `EntityRelationDto`, `EntityMentionDto` exist in `serve/queries.rs`; `Blast`, `BlastFile`, `blast_radius`, `MAX_DEPTH`, `MAX_FILES` in `singularrag_core::blast`; `Changed`, `ChangedSymbol`, `changed` in `singularrag_core::changed`.
- Recorded deviations: locate walks run under the session key `walk` (label `walk` in the rail), not the panel's `serve`, so a person can tell walk retrievals from their own; the path (`repo_map` with `clamp_budget`) is the same. The CI walk file has five walks (two connection starts) so both connection paths are exercised.
- Review Focus 1–5 have named tests in Tasks 2, 3, 4, 1, 3.
