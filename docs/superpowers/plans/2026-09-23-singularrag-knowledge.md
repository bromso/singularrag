# singularrag knowledge Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Embed document sections, extract entities and relations from them with a local model in the background, seed `repo_map` with semantic and entity matches, add an `entities` tool, and show the knowledge graph in the UI, all judged by a prose fixture set and Jonas's vault.

**Architecture:** A core `models.rs` speaks Ollama's HTTP API (embeddings, JSON extraction, tags) through `reqwest::blocking`; a `fake_ollama` test server keeps every test offline. Schema version 4 adds entity, mention, relation and queue tables plus `sqlite-vec` `vec0` tables (created at the first embedding, sized to the model's dimension). `knowledge.rs` owns the queue and the extraction tick, which the actor runs between jobs; `rank.rs` takes precomputed seeds (query vector, matched entities, theme vectors) so the ranker itself never calls a model. The `entities` tool, `doctor`, the `/api/entities` route and the map overlay sit on top.

**Tech Stack:** Rust (reqwest 0.13 `blocking` + `json`, `sqlite-vec = "=0.1.6"` via `sqlite3_auto_extension`, rusqlite bundled, blake3, serde_json), React UI (Bun only), Ollama at runtime (`qwen2.5:7b-instruct`, `nomic-embed-text`, 768 dims).

**Spec:** `docs/superpowers/specs/2026-09-23-singularrag-knowledge-design.md` (binding; parent spec `docs/superpowers/specs/2026-09-19-singularrag-design.md` §5/§7/§9/§10/§12 amended in Task 10; documents spec `docs/superpowers/specs/2026-09-23-singularrag-documents-design.md` §5 `hit_shares` reused).

## Global Constraints

- Branch `knowledge-v0` from `main` 5ec066b (spec commit e98f4cf). Worktree `/Users/jonasbroms/Sites/singularrag/.claude/worktrees/ui-v0`; absolute paths; never `cd` into `/Users/jonasbroms/Sites/singularrag`; the Bash guard rejects any command line containing the word `eval` (a directory and a subcommand carry the name: use globs like `e*/` and never type the subcommand; write files under it with the Write tool) and rejects computed command names; never a bare `git stash`; commit with `git add -A`; branch from `origin/main`, never local `main` (stale).
- Gate for every commit: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`; UI tasks also `cd ui && bun run typecheck && bun test && bun run build` (Bun only), then `cargo test --workspace` once more (assets are embedded).
- No model call at query time except one embedding of the query (and one per `themes` entry). No generated answers. Every model failure is `Error::ModelUnavailable(String)` and means "skip", never a crash or a tool error. Nothing in singularrag downloads a model.
- `workspace.toml` `[models]`: `ollama = "http://127.0.0.1:11434"`, `extract = "qwen2.5:7b-instruct"`, `embed = "nomic-embed-text"` (defaults when absent), optional `api = "anthropic"` for extraction only (key from `ANTHROPIC_API_KEY`). Env override `SINGULARRAG_OLLAMA_URL` wins over the file (tests and the CLI use it).
- Model calls: 30 s timeout; one retry on a network error, none on a model error; embedding batches of at most 50 texts; extraction input at most 6,000 characters per part, split at paragraph boundaries; `format: "json"`, temperature 0.
- Entity types, closed: `person`, `organisation`, `system`, `concept`, `event`, `place`, `document`; unknown → `concept`. Name ≤ 80 chars, description ≤ 300, control characters stripped. Dedup by `norm_name` = lowercased, whitespace collapsed, trailing `.,;:!?` dropped. A later different description is appended with `; ` and the whole capped at 300.
- Extraction tick: up to 50 embeddings in one call, up to 5 extractions under a 20 s budget, oldest `queued_at_ms` first, one transaction per section; malformed JSON gets one stricter retry, then `attempts += 1`; three attempts skip the section, shown in the skipped sheet as `extraction failed: <last_error>`. Runs only under `serve` and `mcp` (the actor), never the one-shot CLI. Every 30 s while the queue is non-empty.
- Header segments, exact: ` · entities: N pending` while the queue is non-empty; ` · models: unavailable` while the last model call failed; ` · embeddings: rebuilding` while the vector tables are being rebuilt after a dimension change. Appended after `fresh`/`STALE…` and before ` · retrieval r_…`.
- Seeds (spec §4): 20 nearest sections by cosine seed their files with `FTS_FILE_BOOST × similarity` and score as hits with weight `similarity`; matched entities (whole-word `norm_name`, the agent's `entities` argument, 10 nearest entity vectors above similarity 0.5) seed mentioning files with `NOTE_BOOST` and the sections score as hits with weight 1.0; `themes` match relation vectors (10 nearest). `Reasons` gains `semantic: Option<f64>`, `entities: Vec<String>`, `themes: Vec<String>`. `fts_names` stays name-hits only.
- `entities {query, entities?, limit?}`: limit default 10, max 25; output format in Task 6; one `retrievals` row `tool = "entities"`, served items = cited sections in rank order, `limit_n = limit`.
- Schema version 4; vector tables `section_vec`, `entity_vec`, `relation_vec` are `vec0(embedding float[DIM] distance_metric=cosine)` created when `embed_dim` is first known; a model or dimension change drops and recreates them and re-queues every section.
- Agent-facing strings (`INSTRUCTIONS`, tool descriptions) live in `server.rs` as literals kept byte-identical in the `#[tool]` attribute and `tests/mcp.rs`.
- Secret-like documents are skipped whole before any of this; nothing flagged is ever sent to a model.

## Review Focus

1. Ollama returns an embedding whose length differs from `embed_dim` (a model swap mid-run): the batch must be rejected with `ModelUnavailable`, never inserted. Test in Task 2 (`a_wrong_dimension_is_refused`).
2. A section whose text contains `"` and `\` inside JSON-looking prose: the prompt must embed it as data and the response parser must not be confused by fences (` ```json … ``` `) around the JSON. Test in Task 1 (`fenced_json_and_quotes_are_parsed`).
3. The extractor returns an entity name that is empty, 500 characters, or only punctuation: dropped or truncated, never a `norm_name` of `""`. Test in Task 3 (`degenerate_entity_names_are_dropped_or_truncated`).
4. The same section text appears in two files (a copied note): both sections queue, the second is served from the extraction cache without a model call, and mentions exist for both symbols. Test in Task 3 (`duplicate_section_text_hits_the_cache`).
5. Ollama goes down between the embed step and the extract step of one tick: the tick stops, the queue keeps its rows, the header says `models: unavailable`, and the next tick resumes. Test in Task 4 (`a_model_outage_mid_tick_leaves_the_queue_intact`).

---

## File structure

- `crates/singularrag-core/src/models.rs` (new): `ModelsConfig`, `Models`, `SectionInput`, `Extraction`, prompt, JSON parsing, `ENTITY_TYPES`, `normalise_*`.
- `crates/singularrag-core/src/fake_ollama.rs` (new, always compiled, std only): the test server.
- `crates/singularrag-core/src/error.rs`: `ModelUnavailable(String)`.
- `crates/singularrag-core/src/store/schema.rs`, `store/mod.rs`, `store/vec.rs` (new): version 4, extension loading, vector helpers.
- `crates/singularrag-core/src/workspace.rs`: `[models]`.
- `crates/singularrag-core/src/knowledge.rs` (new): queue, caches, extraction apply, tick, seeds, entity matching and rendering.
- `crates/singularrag-core/src/index.rs`: queue on section write; `delete_symbols_for` clears knowledge rows.
- `crates/singularrag-core/src/rank.rs`: `Seeds`, new reasons.
- `crates/singularrag-core/src/map.rs`: `Extras` in the header.
- `crates/singularrag-core/src/engine.rs`: `models()`, `knowledge_tick`, `entities`, seeds in `repo_map`, `set_models_enabled`.
- `crates/singularrag-core/src/eval.rs`: unchanged API; `--no-models` is an engine setting.
- `crates/singularrag-core/src/fixture.rs` + `crates/singularrag-core/fixtures/prose/{voyage.md,handbook.md,extraction.json}`.
- `crates/singularrag/src/actor.rs` (tick, `Job::Entities`), `main.rs` (`doctor`, `entities`, `query --entity/--theme`, `eval --no-models`), `mcp/server.rs` + `tests/mcp.rs`, `serve/queries.rs`, `serve/routes.rs`, `serve/mod.rs`, `tests/cli.rs`.
- `ui/src/api/{types,client}.ts`, `ui/src/lib/{graph,mapStyle,reasons}.ts`, `ui/src/components/{ViewToggle,MapView,DetailPanel,FreshnessBadge,SkippedSheet}.tsx`, `ui/src/App.tsx`, tests beside each.
- `e*/questions-prose.toml` (new), README, parent spec.

---

### Task 1: `models.rs` and the fake Ollama

**Files:**
- Modify: `Cargo.toml` (workspace `reqwest` features gain `"blocking"`), `crates/singularrag-core/Cargo.toml` (`reqwest = { workspace = true }`)
- Modify: `crates/singularrag-core/src/error.rs`
- Create: `crates/singularrag-core/src/models.rs`, `crates/singularrag-core/src/fake_ollama.rs`
- Modify: `crates/singularrag-core/src/lib.rs` (`pub mod models; pub mod fake_ollama;`)

**Interfaces:**
- Produces: `Error::ModelUnavailable(String)`; `models::ModelsConfig { ollama: String, extract: String, embed: String, api: Option<String> }` with `Default` (the three defaults, `api: None`) and `ModelsConfig::with_env(self) -> Self` (applies `SINGULARRAG_OLLAMA_URL`); `models::Models::new(cfg: ModelsConfig) -> Models`; `Models::embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>`; `Models::extract(&self, input: &SectionInput) -> Result<Extraction>`; `Models::tags(&self) -> Result<Vec<String>>`; `Models::config(&self) -> &ModelsConfig`; `SectionInput<'a> { path: &'a str, heading: &'a str, text: &'a str }`; `Extraction { entities: Vec<ExtractedEntity>, relations: Vec<ExtractedRelation> }` (both `Deserialize + Serialize + Default + Clone`); `ExtractedEntity { name: String, r#type: String, description: String }`; `ExtractedRelation { source: String, target: String, description: String }`; `models::ENTITY_TYPES: [&str; 7]`; `models::normalise_extraction(Extraction) -> Extraction` (truncate, strip controls, type mapping, drop empty names); `models::parse_extraction(text: &str) -> Option<Extraction>` (strips fences, finds the first `{`…last `}`); `models::EXTRACT_PROMPT` (template); `models::MAX_PART_CHARS: usize = 6000`; `models::split_parts(text: &str) -> Vec<String>`.
- `fake_ollama::FakeOllama::spawn(dim: usize) -> FakeOllama` with `url() -> String`, `set_extraction(heading_substring: &str, extraction: serde_json::Value)`, `fail_next_generate(n: usize)` (returns `not json {` bodies), `set_down(bool)` (refuses connections), `calls() -> Vec<String>` (paths hit, in order), `Drop` stops the thread. Embeddings are deterministic bag-of-words: each lowercase word hashed into `dim` buckets (`blake3` first 8 bytes mod dim), counts L2-normalised; so texts that share words are close and a text is identical to itself.

- [ ] **Step 1: Write the failing tests** (`models.rs` tests)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_ollama::FakeOllama;

    fn models(f: &FakeOllama) -> Models {
        Models::new(ModelsConfig { ollama: f.url(), ..ModelsConfig::default() })
    }

    #[test]
    fn embed_batches_and_returns_one_vector_per_text() {
        let f = FakeOllama::spawn(16);
        let m = models(&f);
        let texts: Vec<String> = (0..120).map(|i| format!("text {i}")).collect();
        let out = m.embed(&texts).unwrap();
        assert_eq!(out.len(), 120);
        assert!(out.iter().all(|v| v.len() == 16));
        assert_eq!(f.calls().iter().filter(|p| p.as_str() == "/api/embed").count(), 3, "batches of 50");
        let a = m.embed(&["the login flow".into()]).unwrap();
        let b = m.embed(&["the login flow".into()]).unwrap();
        assert_eq!(a, b, "deterministic");
    }

    #[test]
    fn extract_parses_the_models_json_and_normalises_it() {
        let f = FakeOllama::spawn(8);
        f.set_extraction("Freshness", serde_json::json!({
            "entities": [
                {"name": "  Acme Ltd ", "type": "Company", "description": "a firm\u{0007}"},
                {"name": "", "type": "person", "description": "nobody"},
                {"name": "x".repeat(200), "type": "person", "description": "y".repeat(400)}
            ],
            "relations": [{"source": "Acme Ltd", "target": "Jonas", "description": "employs"}]
        }));
        let m = models(&f);
        let e = m.extract(&SectionInput { path: "docs/a.md", heading: "Freshness", text: "Acme Ltd employs Jonas." }).unwrap();
        assert_eq!(e.entities.len(), 2, "{e:?}");
        assert_eq!(e.entities[0].name, "Acme Ltd");
        assert_eq!(e.entities[0].r#type, "concept", "unknown type maps to concept");
        assert_eq!(e.entities[0].description, "a firm");
        assert_eq!(e.entities[1].name.chars().count(), 80);
        assert_eq!(e.entities[1].description.chars().count(), 300);
        assert_eq!(e.relations[0].source, "Acme Ltd");
    }

    #[test]
    fn fenced_json_and_quotes_are_parsed() {
        let raw = "Here you go:\n```json\n{\"entities\":[{\"name\":\"Q \\\"quoted\\\" \\\\ name\",\"type\":\"person\",\"description\":\"says \\\"hi\\\"\"}],\"relations\":[]}\n```";
        let e = parse_extraction(raw).expect("parsed");
        assert_eq!(e.entities[0].name, "Q \"quoted\" \\ name");
        assert!(parse_extraction("no json here").is_none());
        assert!(parse_extraction("{\"entities\": [").is_none());
    }

    #[test]
    fn malformed_json_retries_once_then_errors() {
        let f = FakeOllama::spawn(8);
        f.fail_next_generate(1);
        f.set_extraction("H", serde_json::json!({"entities": [], "relations": []}));
        let m = models(&f);
        assert!(m.extract(&SectionInput { path: "a.md", heading: "H", text: "t" }).is_ok(), "one retry recovers");
        f.fail_next_generate(2);
        let e = m.extract(&SectionInput { path: "a.md", heading: "H", text: "t" }).unwrap_err();
        assert!(matches!(e, crate::Error::ModelUnavailable(_)), "{e}");
        assert_eq!(f.calls().iter().filter(|p| p.as_str() == "/api/generate").count(), 4);
    }

    #[test]
    fn a_down_server_is_model_unavailable_and_tags_lists_models() {
        let f = FakeOllama::spawn(8);
        let m = models(&f);
        assert_eq!(m.tags().unwrap(), vec!["qwen2.5:7b-instruct".to_string(), "nomic-embed-text".to_string()]);
        f.set_down(true);
        let e = m.embed(&["x".into()]).unwrap_err();
        assert!(matches!(e, crate::Error::ModelUnavailable(_)));
        let e = m.extract(&SectionInput { path: "a.md", heading: "H", text: "t" }).unwrap_err();
        assert!(matches!(e, crate::Error::ModelUnavailable(_)));
    }

    #[test]
    fn long_sections_split_at_paragraphs_and_env_overrides_the_url() {
        let para = "word ".repeat(700).trim().to_string(); // 3,499 chars
        let text = format!("{para}\n\n{para}\n\n{para}");
        let parts = split_parts(&text);
        assert_eq!(parts.len(), 2, "{:?}", parts.iter().map(|p| p.len()).collect::<Vec<_>>());
        assert!(parts.iter().all(|p| p.len() <= MAX_PART_CHARS));
        std::env::set_var("SINGULARRAG_OLLAMA_URL", "http://10.0.0.1:1");
        assert_eq!(ModelsConfig::default().with_env().ollama, "http://10.0.0.1:1");
        std::env::remove_var("SINGULARRAG_OLLAMA_URL");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p singularrag-core --lib models:: 2>&1 | grep -E '^error|test result' | head -3`
Expected: compile errors (`cannot find type Models`).

- [ ] **Step 3: Implement**

`error.rs`: add `#[error("model unavailable: {0}")] ModelUnavailable(String),`. `Cargo.toml` workspace: `reqwest = { version = "0.13", default-features = false, features = ["json", "stream", "blocking"] }`; core crate adds `reqwest = { workspace = true }`.

`models.rs`:

```rust
//! Ollama over HTTP (knowledge spec §2): embeddings and JSON extraction. Blocking client,
//! because the Engine and the actor are synchronous. Every failure is `ModelUnavailable`.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

pub const DEFAULT_OLLAMA: &str = "http://127.0.0.1:11434";
pub const DEFAULT_EXTRACT: &str = "qwen2.5:7b-instruct";
pub const DEFAULT_EMBED: &str = "nomic-embed-text";
pub const EMBED_BATCH: usize = 50;
pub const MAX_PART_CHARS: usize = 6000;
pub const NAME_MAX: usize = 80;
pub const DESCRIPTION_MAX: usize = 300;
pub const ENTITY_TYPES: [&str; 7] = ["person", "organisation", "system", "concept", "event", "place", "document"];
const TIMEOUT: Duration = Duration::from_secs(30);

pub const EXTRACT_PROMPT: &str = "You extract a knowledge graph from one section of a document.\n\
Return ONLY a JSON object of the shape {\"entities\": [{\"name\": string, \"type\": string, \"description\": string}], \"relations\": [{\"source\": string, \"target\": string, \"description\": string}]}.\n\
Types are exactly one of: person, organisation, system, concept, event, place, document.\n\
Descriptions are one short sentence from the text. Relations connect two entity names from your list.\n\
Document: {path}\nSection: {heading}\n\nText:\n{text}\n";
const STRICT_SUFFIX: &str = "\nYour previous answer was not valid JSON. Answer with the JSON object only, no prose, no code fence.";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelsConfig {
    pub ollama: String,
    pub extract: String,
    pub embed: String,
    pub api: Option<String>,
}

impl Default for ModelsConfig {
    fn default() -> Self {
        ModelsConfig { ollama: DEFAULT_OLLAMA.into(), extract: DEFAULT_EXTRACT.into(), embed: DEFAULT_EMBED.into(), api: None }
    }
}

impl ModelsConfig {
    /// `SINGULARRAG_OLLAMA_URL` wins over the file: tests and the CLI point at a fake.
    pub fn with_env(mut self) -> Self {
        if let Ok(u) = std::env::var("SINGULARRAG_OLLAMA_URL") {
            if !u.is_empty() { self.ollama = u; }
        }
        self
    }
}

pub struct SectionInput<'a> { pub path: &'a str, pub heading: &'a str, pub text: &'a str }

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Extraction {
    #[serde(default)] pub entities: Vec<ExtractedEntity>,
    #[serde(default)] pub relations: Vec<ExtractedRelation>,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ExtractedEntity { #[serde(default)] pub name: String, #[serde(default, rename = "type")] pub r#type: String, #[serde(default)] pub description: String }
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ExtractedRelation { #[serde(default)] pub source: String, #[serde(default)] pub target: String, #[serde(default)] pub description: String }

pub struct Models { cfg: ModelsConfig, http: reqwest::blocking::Client }

fn unavailable(e: impl std::fmt::Display) -> Error { Error::ModelUnavailable(e.to_string()) }

/// Truncate to `max` characters, strip control characters, collapse whitespace.
pub fn clean(s: &str, max: usize) -> String {
    let s: String = s.chars().filter(|c| !c.is_control()).collect();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    s.chars().take(max).collect()
}

pub fn normalise_extraction(e: Extraction) -> Extraction {
    let entities: Vec<ExtractedEntity> = e.entities.into_iter().filter_map(|x| {
        let name = clean(&x.name, NAME_MAX);
        if name.chars().all(|c| !c.is_alphanumeric()) { return None; }
        let t = x.r#type.trim().to_lowercase();
        let r#type = if ENTITY_TYPES.contains(&t.as_str()) { t } else { "concept".to_string() };
        Some(ExtractedEntity { name, r#type, description: clean(&x.description, DESCRIPTION_MAX) })
    }).collect();
    let relations = e.relations.into_iter().filter_map(|r| {
        let source = clean(&r.source, NAME_MAX); let target = clean(&r.target, NAME_MAX);
        if source.is_empty() || target.is_empty() || source == target { return None; }
        Some(ExtractedRelation { source, target, description: clean(&r.description, DESCRIPTION_MAX) })
    }).collect();
    Extraction { entities, relations }
}

/// The JSON object inside a model answer: fences and prose around it are dropped.
pub fn parse_extraction(text: &str) -> Option<Extraction> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end <= start { return None; }
    serde_json::from_str::<Extraction>(&text[start..=end]).ok()
}

/// Paragraph-bounded parts of at most `MAX_PART_CHARS`; a single oversized paragraph is cut hard.
pub fn split_parts(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    for para in text.split("\n\n") {
        if !cur.is_empty() && cur.len() + 2 + para.len() > MAX_PART_CHARS { parts.push(std::mem::take(&mut cur)); }
        if para.len() > MAX_PART_CHARS {
            let mut s = para;
            while s.len() > MAX_PART_CHARS { let (h, t) = s.split_at(s.char_indices().nth(MAX_PART_CHARS).map(|(i, _)| i).unwrap_or(s.len())); parts.push(h.to_string()); s = t; }
            cur = s.to_string();
            continue;
        }
        if !cur.is_empty() { cur.push_str("\n\n"); }
        cur.push_str(para);
    }
    if !cur.trim().is_empty() { parts.push(cur); }
    parts
}

impl Models {
    pub fn new(cfg: ModelsConfig) -> Models {
        let http = reqwest::blocking::Client::builder().timeout(TIMEOUT).build().expect("reqwest client");
        Models { cfg, http }
    }
    pub fn config(&self) -> &ModelsConfig { &self.cfg }

    fn post(&self, path: &str, body: &serde_json::Value) -> Result<serde_json::Value> {
        let url = format!("{}{}", self.cfg.ollama.trim_end_matches('/'), path);
        let mut last = None;
        for _ in 0..2 {
            match self.http.post(&url).json(body).send() {
                Ok(resp) => {
                    let status = resp.status();
                    let v: serde_json::Value = resp.json().map_err(unavailable)?;
                    if !status.is_success() { return Err(unavailable(format!("{path}: HTTP {status}: {v}"))); }
                    return Ok(v);
                }
                Err(e) if e.is_connect() || e.is_timeout() => last = Some(e),
                Err(e) => return Err(unavailable(e)),
            }
        }
        Err(unavailable(last.map(|e| e.to_string()).unwrap_or_default()))
    }

    pub fn tags(&self) -> Result<Vec<String>> {
        let url = format!("{}/api/tags", self.cfg.ollama.trim_end_matches('/'));
        let v: serde_json::Value = self.http.get(&url).send().map_err(unavailable)?.json().map_err(unavailable)?;
        Ok(v["models"].as_array().map(|a| a.iter().filter_map(|m| m["name"].as_str().map(str::to_string)).collect()).unwrap_or_default())
    }

    pub fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let mut out = Vec::with_capacity(texts.len());
        for chunk in texts.chunks(EMBED_BATCH) {
            let v = self.post("/api/embed", &serde_json::json!({ "model": self.cfg.embed, "input": chunk }))?;
            let arr = v["embeddings"].as_array().ok_or_else(|| unavailable("embed: no embeddings field"))?;
            if arr.len() != chunk.len() { return Err(unavailable(format!("embed: {} vectors for {} texts", arr.len(), chunk.len()))); }
            for e in arr {
                let vec: Vec<f32> = e.as_array().ok_or_else(|| unavailable("embed: vector is not an array"))?
                    .iter().map(|x| x.as_f64().unwrap_or(0.0) as f32).collect();
                out.push(vec);
            }
        }
        Ok(out)
    }

    fn generate(&self, prompt: &str) -> Result<String> {
        let v = self.post("/api/generate", &serde_json::json!({ "model": self.cfg.extract, "prompt": prompt, "format": "json", "stream": false, "options": { "temperature": 0 } }))?;
        v["response"].as_str().map(str::to_string).ok_or_else(|| unavailable("generate: no response field"))
    }

    /// One prompt per part; parts are merged. A malformed answer gets one stricter retry.
    pub fn extract(&self, input: &SectionInput) -> Result<Extraction> {
        let mut merged = Extraction::default();
        for part in split_parts(input.text) {
            let prompt = EXTRACT_PROMPT.replace("{path}", input.path).replace("{heading}", input.heading).replace("{text}", &part);
            let first = self.generate(&prompt)?;
            let parsed = match parse_extraction(&first) {
                Some(e) => e,
                None => {
                    let second = self.generate(&format!("{prompt}{STRICT_SUFFIX}"))?;
                    parse_extraction(&second).ok_or_else(|| unavailable(format!("extract: not JSON after retry: {}", clean(&second, 120))))?
                }
            };
            let e = normalise_extraction(parsed);
            merged.entities.extend(e.entities);
            merged.relations.extend(e.relations);
        }
        Ok(merged)
    }
}
```

`fake_ollama.rs` (std only; a tiny HTTP/1.1 server on a thread; parses `Content-Length`; routes `/api/tags`, `/api/embed`, `/api/generate`; `set_down` makes the accept loop close connections immediately; embeddings as described; `set_extraction` keys on a heading substring found in the prompt's `Section:` line; `fail_next_generate(n)` returns `not json {` for the next `n` generate calls; records every path in `calls`). The `Drop` sets a stop flag and connects once to unblock `accept`. Write it so a test never needs tokio.

- [ ] **Step 4: Run the tests**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test -p singularrag-core --lib models:: 2>&1 | grep -E 'test result|panicked'`
Expected: 6 passed.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(core): models.rs speaks Ollama (embed, JSON extraction, tags) with a std fake for tests"
```

---

### Task 2: Schema version 4, sqlite-vec, `[models]` in the workspace

**Files:**
- Modify: `Cargo.toml` (workspace `sqlite-vec = "=0.1.6"`), `crates/singularrag-core/Cargo.toml`
- Modify: `crates/singularrag-core/src/store/schema.rs`, `store/mod.rs`; Create: `store/vec.rs`
- Modify: `crates/singularrag-core/src/workspace.rs`

**Interfaces:**
- Produces: `SCHEMA_VERSION = 4`; tables `entities(id INTEGER PRIMARY KEY, name TEXT NOT NULL, norm_name TEXT NOT NULL UNIQUE, type TEXT NOT NULL, description TEXT NOT NULL, mentions INTEGER NOT NULL DEFAULT 0)`, `entity_mentions(entity_id, symbol_id, section_hash TEXT, PRIMARY KEY(entity_id, symbol_id))`, `relations(id INTEGER PRIMARY KEY, src_entity, dst_entity, description, symbol_id, section_hash)`, `extract_queue(symbol_id INTEGER PRIMARY KEY, hash TEXT NOT NULL, attempts INTEGER NOT NULL DEFAULT 0, last_error TEXT, queued_at_ms INTEGER NOT NULL)`, `section_embeddings(symbol_id INTEGER PRIMARY KEY, hash TEXT NOT NULL)`, `extraction_cache(hash TEXT PRIMARY KEY, json TEXT NOT NULL)`, `embedding_cache(hash TEXT PRIMARY KEY, dim INTEGER NOT NULL, blob BLOB NOT NULL)`; indexes on `entity_mentions(symbol_id)`, `relations(symbol_id)`, `relations(src_entity)`, `relations(dst_entity)`.
- `store::vec`: `pub fn to_blob(v: &[f32]) -> Vec<u8>`; `pub fn from_blob(b: &[u8]) -> Vec<f32>`; `pub fn ensure_tables(store: &Store, dim: usize) -> Result<bool>` (creates the three `vec0` tables when absent, records `embed_dim`; returns `true` when it (re)created them: a different stored `embed_dim` drops the three tables, clears `section_embeddings`, `entity_mentions`, `relations`, `embedding_cache`, sets meta `embeddings_rebuilding = "1"`, and re-queues every document section with a body); `pub fn dim(store) -> Result<Option<usize>>`; `pub fn insert(store, table: &str, rowid: i64, v: &[f32]) -> Result<()>` (refuses a wrong length with `ModelUnavailable`); `pub fn delete(store, table, rowid)`; `pub fn knn(store, table, query: &[f32], k: usize) -> Result<Vec<(i64, f64)>>` returning `(rowid, similarity = 1 - cosine distance)` best first; `Store::open` registers the extension once via `std::sync::Once` + `sqlite3_auto_extension`.
- `Workspace.models: ModelsConfig` parsed from `[models]` (`ollama`, `extract`, `embed`, `api`), defaults when absent, `with_env()` applied.

- [ ] **Step 1: Write the failing tests**

`store/vec.rs` tests:

```rust
    #[test]
    fn vec_tables_are_created_once_and_knn_returns_nearest_by_cosine() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(dim(&store).unwrap(), None);
        assert!(ensure_tables(&store, 3).unwrap());
        assert!(!ensure_tables(&store, 3).unwrap(), "same dim: nothing to do");
        insert(&store, "section_vec", 1, &[1.0, 0.0, 0.0]).unwrap();
        insert(&store, "section_vec", 2, &[0.0, 1.0, 0.0]).unwrap();
        insert(&store, "section_vec", 3, &[0.9, 0.1, 0.0]).unwrap();
        let hits = knn(&store, "section_vec", &[1.0, 0.0, 0.0], 2).unwrap();
        assert_eq!(hits.iter().map(|h| h.0).collect::<Vec<_>>(), vec![1, 3]);
        assert!(hits[0].1 > 0.999 && hits[1].1 > 0.98 && hits[1].1 < hits[0].1, "{hits:?}");
        delete(&store, "section_vec", 3).unwrap();
        assert_eq!(knn(&store, "section_vec", &[1.0, 0.0, 0.0], 5).unwrap().len(), 2);
        assert_eq!(from_blob(&to_blob(&[1.5, -2.0])), vec![1.5, -2.0]);
    }

    #[test]
    fn a_wrong_dimension_is_refused() {
        let store = Store::open_in_memory().unwrap();
        ensure_tables(&store, 3).unwrap();
        let e = insert(&store, "section_vec", 1, &[1.0, 0.0]).unwrap_err();
        assert!(matches!(e, crate::Error::ModelUnavailable(_)), "{e}");
    }

    #[test]
    fn a_dimension_change_rebuilds_and_requeues() {
        let dir = tempfile::tempdir().unwrap();
        crate::fixture::write_docs_mini(dir.path());
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        let ws = crate::workspace::Workspace::single(dir.path()).unwrap();
        crate::index::Indexer::new(&store, &ws, &crate::config::MapConfig::default()).unwrap().refresh(None).unwrap();
        ensure_tables(&store, 4).unwrap();
        insert(&store, "section_vec", 1, &[1.0, 0.0, 0.0, 0.0]).unwrap();
        store.conn().execute("INSERT INTO section_embeddings(symbol_id, hash) VALUES (1, 'h')", []).unwrap();
        store.conn().execute("DELETE FROM extract_queue", []).unwrap();
        assert!(ensure_tables(&store, 8).unwrap(), "dim changed");
        assert_eq!(dim(&store).unwrap(), Some(8));
        let n: i64 = store.conn().query_row("SELECT COUNT(*) FROM section_embeddings", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
        let q: i64 = store.conn().query_row("SELECT COUNT(*) FROM extract_queue", [], |r| r.get(0)).unwrap();
        assert!(q >= 3, "every document section with a body is queued again: {q}");
        assert_eq!(store.get_meta("embeddings_rebuilding").unwrap().as_deref(), Some("1"));
    }
```

`workspace.rs` test:

```rust
    #[test]
    fn models_table_is_optional_with_defaults_and_env_override() {
        let d = ws_dir();
        assert_eq!(Workspace::open(d.path()).unwrap().models, crate::models::ModelsConfig::default());
        write_roots(d.path(), "[models]\nextract = \"llama3.2:3b\"\napi = \"anthropic\"\n");
        let ws = Workspace::open(d.path()).unwrap();
        assert_eq!(ws.models.extract, "llama3.2:3b");
        assert_eq!(ws.models.embed, "nomic-embed-text");
        assert_eq!(ws.models.api.as_deref(), Some("anthropic"));
        assert!(!ws.is_named(), "a models table alone does not name roots");
    }
```

`store/mod.rs` test: `all_tables_exist` gains the seven new table names; the schema-version test expects 4.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p singularrag-core --lib 'store::' 2>&1 | grep -E '^error|test result' | head -3`
Expected: `cannot find module vec` / missing tables.

- [ ] **Step 3: Implement**

`store/vec.rs`:

```rust
//! sqlite-vec tables (knowledge spec §3): `section_vec`, `entity_vec`, `relation_vec`,
//! created when the embedding dimension is first known; a dimension change rebuilds.

use rusqlite::params;
use crate::store::Store;
use crate::{Error, Result};

pub const TABLES: [&str; 3] = ["section_vec", "entity_vec", "relation_vec"];

pub fn to_blob(v: &[f32]) -> Vec<u8> { v.iter().flat_map(|f| f.to_le_bytes()).collect() }
pub fn from_blob(b: &[u8]) -> Vec<f32> { b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect() }

pub fn dim(store: &Store) -> Result<Option<usize>> {
    Ok(store.get_meta("embed_dim")?.and_then(|s| s.parse().ok()))
}

pub fn ensure_tables(store: &Store, dim_now: usize) -> Result<bool> {
    let conn = store.conn();
    match dim(store)? {
        Some(d) if d == dim_now => {
            let exists: i64 = conn.query_row("SELECT COUNT(*) FROM sqlite_master WHERE name = 'section_vec'", [], |r| r.get(0))?;
            if exists > 0 { return Ok(false); }
        }
        Some(_) => {
            let tx = conn.unchecked_transaction()?;
            for t in TABLES { tx.execute_batch(&format!("DROP TABLE IF EXISTS {t}"))?; }
            tx.execute_batch("DELETE FROM section_embeddings; DELETE FROM entity_mentions; DELETE FROM relations; DELETE FROM entities; DELETE FROM embedding_cache;")?;
            tx.execute(
                "INSERT OR REPLACE INTO extract_queue(symbol_id, hash, attempts, last_error, queued_at_ms)
                 SELECT s.id, '', 0, NULL, ?1 FROM symbols s JOIN sections_fts f ON f.rowid = s.id",
                [crate::time::now_ms()],
            )?;
            tx.commit()?;
            store.set_meta("embeddings_rebuilding", "1")?;
        }
        None => {}
    }
    for t in TABLES {
        conn.execute_batch(&format!("CREATE VIRTUAL TABLE IF NOT EXISTS {t} USING vec0(embedding float[{dim_now}] distance_metric=cosine)"))?;
    }
    store.set_meta("embed_dim", &dim_now.to_string())?;
    Ok(true)
}

pub fn insert(store: &Store, table: &str, rowid: i64, v: &[f32]) -> Result<()> {
    if dim(store)? != Some(v.len()) {
        return Err(Error::ModelUnavailable(format!("embedding of {} dims for a {:?}-dim index", v.len(), dim(store)?)));
    }
    store.conn().execute(&format!("INSERT OR REPLACE INTO {table}(rowid, embedding) VALUES (?1, ?2)"), params![rowid, to_blob(v)])?;
    Ok(())
}

pub fn delete(store: &Store, table: &str, rowid: i64) -> Result<()> {
    store.conn().execute(&format!("DELETE FROM {table} WHERE rowid = ?1"), [rowid])?;
    Ok(())
}

/// Nearest `k` rows, best first, as `(rowid, similarity)` with similarity = 1 − cosine distance.
pub fn knn(store: &Store, table: &str, query: &[f32], k: usize) -> Result<Vec<(i64, f64)>> {
    if k == 0 || dim(store)?.is_none() { return Ok(Vec::new()); }
    let mut stmt = store.conn().prepare(&format!("SELECT rowid, distance FROM {table} WHERE embedding MATCH ?1 AND k = ?2 ORDER BY distance"))?;
    let rows = stmt.query_map(params![to_blob(query), k as i64], |r| Ok((r.get::<_, i64>(0)?, 1.0 - r.get::<_, f64>(1)?)))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}
```

`table` is always one of `TABLES` (assert in debug builds). `store/mod.rs`: `pub mod vec;` and, at the top of `Store::open`/`open_in_memory`/`open_read_only`, `register_vec()`:

```rust
fn register_vec() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute::<*const (), unsafe extern "C" fn(*mut rusqlite::ffi::sqlite3, *mut *mut i8, *const rusqlite::ffi::sqlite3_api_routines) -> i32>(sqlite_vec::sqlite3_vec_init as *const ())));
    });
}
```

(This exact transmute compiled against `sqlite-vec = "=0.1.6"` and rusqlite 0.40 bundled in a probe on 2026-09-23; if the signature type differs in the crate's `ffi`, use `rusqlite::ffi::sqlite3_auto_extension(Some(unsafe { std::mem::transmute(sqlite_vec::sqlite3_vec_init as *const ()) }))` with the inferred type.) `drop_all_tables` already drops virtual tables first, which covers `vec0` and its shadow tables. `schema.rs`: the seven tables and indexes, `SCHEMA_VERSION = 4`. `workspace.rs`: `WorkspaceFile { root, models: Option<ModelsEntry> }` where `ModelsEntry { ollama, extract, embed, api }` are all `Option<String>`, folded onto `ModelsConfig::default()` then `.with_env()`; `Workspace::single` uses `ModelsConfig::default().with_env()`.

- [ ] **Step 4: Run the tests**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace 2>&1 | grep -E 'test result|panicked|FAILED'`
Expected: all ok (existing indexes rebuild on version 4 in tests automatically).

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(core): schema v4 with knowledge tables, sqlite-vec vector tables sized at first embed, [models] in workspace.toml"
```

---

### Task 3: The queue, the caches and extraction (`knowledge.rs`)

**Files:**
- Create: `crates/singularrag-core/src/knowledge.rs`
- Modify: `crates/singularrag-core/src/index.rs` (queue on section write; `delete_symbols_for` clears knowledge rows), `lib.rs`
- Modify: `crates/singularrag/src/serve/queries.rs` (`skipped` unions extraction failures)

**Interfaces:**
- Produces: `knowledge::section_hash(body: &str) -> String` (blake3 hex); `knowledge::queue_section(conn, symbol_id, body) -> Result<()>`; `knowledge::pending(store) -> Result<usize>`; `knowledge::KnowledgeTick { embedded: usize, extracted: usize, failed: usize, pending: usize, model_error: Option<String> }`; `knowledge::tick(store: &Store, models: &Models, budget: Duration) -> Result<KnowledgeTick>` (never returns `ModelUnavailable`: it lands in `model_error`); `knowledge::apply_extraction(conn, symbol_id, hash, &Extraction) -> Result<(Vec<i64> new_entity_ids, Vec<i64> new_relation_ids)>`; `knowledge::norm_name(&str) -> String`; `knowledge::EXTRACT_PER_TICK = 5`, `knowledge::MAX_ATTEMPTS = 3`; `knowledge::failed_sections(store) -> Result<Vec<(String path, String name, String error)>>`; `knowledge::prune_entities(conn)`.
- Consumes: Task 1 `Models`, Task 2 tables and `store::vec`.

- [ ] **Step 1: Write the failing tests** (`knowledge.rs` tests)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MapConfig;
    use crate::fake_ollama::FakeOllama;
    use crate::index::Indexer;
    use crate::models::{Models, ModelsConfig};
    use crate::store::Store;
    use crate::workspace::Workspace;
    use std::time::Duration;

    fn indexed_docs() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        crate::fixture::write_docs_mini(dir.path());
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        let ws = Workspace::single(dir.path()).unwrap();
        Indexer::new(&store, &ws, &MapConfig::default()).unwrap().refresh(None).unwrap();
        (dir, store)
    }
    fn models(f: &FakeOllama) -> Models { Models::new(ModelsConfig { ollama: f.url(), ..Default::default() }) }
    fn count(store: &Store, sql: &str) -> i64 { store.conn().query_row(sql, [], |r| r.get(0)).unwrap() }

    #[test]
    fn indexing_documents_queues_their_sections_and_code_never() {
        let (_d, store) = indexed_docs();
        let q = count(&store, "SELECT COUNT(*) FROM extract_queue");
        let bodies = count(&store, "SELECT COUNT(*) FROM sections_fts");
        assert_eq!(q, bodies, "one queue row per section with a body");
        let code: i64 = count(&store, "SELECT COUNT(*) FROM extract_queue q JOIN symbols s ON s.id = q.symbol_id JOIN files f ON f.id = s.file_id WHERE f.lang IN ('typescript','rust')");
        assert_eq!(code, 0);
        assert_eq!(pending(&store).unwrap() as i64, q);
    }

    #[test]
    fn a_tick_embeds_then_extracts_and_the_queue_drains() {
        let (_d, store) = indexed_docs();
        let f = FakeOllama::spawn(16);
        f.set_extraction("Freshness", serde_json::json!({
            "entities": [{"name": "STALE header", "type": "concept", "description": "the header line that says files changed"},
                         {"name": "createSession", "type": "system", "description": "creates sessions"}],
            "relations": [{"source": "createSession", "target": "STALE header", "description": "is refreshed before"}]
        }));
        let m = models(&f);
        let before = pending(&store).unwrap();
        let t = tick(&store, &m, Duration::from_secs(20)).unwrap();
        assert!(t.embedded >= 3 && t.extracted >= 1, "{t:?}");
        assert!(t.model_error.is_none());
        let mut total = t.extracted;
        while pending(&store).unwrap() > 0 { total += tick(&store, &m, Duration::from_secs(20)).unwrap().extracted; }
        assert_eq!(total, before, "every queued section was extracted");
        assert_eq!(count(&store, "SELECT COUNT(*) FROM entities WHERE norm_name = 'stale header'"), 1);
        assert_eq!(count(&store, "SELECT COUNT(*) FROM relations"), 1);
        assert_eq!(count(&store, "SELECT mentions FROM entities WHERE norm_name = 'stale header'"), 1);
        assert_eq!(count(&store, "SELECT COUNT(*) FROM entity_vec"), 2);
        assert_eq!(count(&store, "SELECT COUNT(*) FROM relation_vec"), 1);
        assert_eq!(count(&store, "SELECT COUNT(*) FROM section_embeddings"), count(&store, "SELECT COUNT(*) FROM sections_fts"));
    }

    #[test]
    fn duplicate_section_text_hits_the_cache() {
        let (dir, store) = indexed_docs();
        let f = FakeOllama::spawn(8);
        f.set_extraction("Runbook", serde_json::json!({"entities": [{"name": "Runbook", "type": "document", "description": "ops notes"}], "relations": []}));
        let m = models(&f);
        while pending(&store).unwrap() > 0 { tick(&store, &m, Duration::from_secs(20)).unwrap(); }
        let generates = f.calls().iter().filter(|p| p.as_str() == "/api/generate").count();
        std::fs::copy(dir.path().join("docs/runbook.md"), dir.path().join("docs/runbook-copy.md")).unwrap();
        let ws = Workspace::single(dir.path()).unwrap();
        Indexer::new(&store, &ws, &MapConfig::default()).unwrap().refresh(None).unwrap();
        assert!(pending(&store).unwrap() > 0, "the copy queues");
        while pending(&store).unwrap() > 0 { tick(&store, &m, Duration::from_secs(20)).unwrap(); }
        assert_eq!(f.calls().iter().filter(|p| p.as_str() == "/api/generate").count(), generates, "served from the cache");
        assert_eq!(count(&store, "SELECT mentions FROM entities WHERE norm_name = 'runbook'"), 2);
    }

    #[test]
    fn a_changed_section_requeues_and_its_old_rows_vanish_and_a_deleted_file_clears_all() {
        let (dir, store) = indexed_docs();
        let f = FakeOllama::spawn(8);
        f.set_extraction("Storage", serde_json::json!({"entities": [{"name": "SessionStore", "type": "system", "description": "keeps sessions"}], "relations": []}));
        let m = models(&f);
        while pending(&store).unwrap() > 0 { tick(&store, &m, Duration::from_secs(20)).unwrap(); }
        assert_eq!(count(&store, "SELECT COUNT(*) FROM entities WHERE norm_name = 'sessionstore'"), 1);
        let p = dir.path().join("docs/design.md");
        std::fs::write(&p, std::fs::read_to_string(&p).unwrap().replace("`SessionStore` keeps them.", "Nothing keeps them now.")).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        let ws = Workspace::single(dir.path()).unwrap();
        Indexer::new(&store, &ws, &MapConfig::default()).unwrap().refresh(None).unwrap();
        f.set_extraction("Storage", serde_json::json!({"entities": [], "relations": []}));
        while pending(&store).unwrap() > 0 { tick(&store, &m, Duration::from_secs(20)).unwrap(); }
        assert_eq!(count(&store, "SELECT COUNT(*) FROM entities WHERE norm_name = 'sessionstore'"), 0, "no mentions left: pruned");
        std::fs::remove_file(&p).unwrap();
        Indexer::new(&store, &ws, &MapConfig::default()).unwrap().refresh(None).unwrap();
        assert_eq!(count(&store, "SELECT COUNT(*) FROM entity_mentions m JOIN symbols s ON s.id = m.symbol_id JOIN files f ON f.id = s.file_id WHERE f.path = 'docs/design.md'"), 0);
        assert_eq!(count(&store, "SELECT COUNT(*) FROM extract_queue q WHERE NOT EXISTS (SELECT 1 FROM symbols s WHERE s.id = q.symbol_id)"), 0);
    }

    #[test]
    fn malformed_answers_count_attempts_and_three_skip_the_section() {
        let (_d, store) = indexed_docs();
        let f = FakeOllama::spawn(8);
        let m = models(&f);
        f.fail_next_generate(1000);
        for _ in 0..4 { tick(&store, &m, Duration::from_secs(20)).unwrap(); }
        let max_attempts: i64 = count(&store, "SELECT MAX(attempts) FROM extract_queue");
        assert!(max_attempts >= 3, "{max_attempts}");
        let failed = failed_sections(&store).unwrap();
        assert!(!failed.is_empty() && failed[0].2.contains("not JSON"), "{failed:?}");
        let t = tick(&store, &m, Duration::from_secs(20)).unwrap();
        assert!(t.model_error.is_none() || t.extracted == 0);
    }

    #[test]
    fn degenerate_entity_names_are_dropped_or_truncated() {
        let (_d, store) = indexed_docs();
        let f = FakeOllama::spawn(8);
        f.set_extraction("Freshness", serde_json::json!({"entities": [
            {"name": "", "type": "person", "description": "x"},
            {"name": "!!!", "type": "person", "description": "x"},
            {"name": "A".repeat(500), "type": "person", "description": "x"},
            {"name": "Jonas", "type": "person", "description": "first"},
            {"name": "jonas ", "type": "person", "description": "second"}
        ], "relations": [{"source": "Jonas", "target": "Jonas", "description": "self"}]}));
        let m = models(&f);
        while pending(&store).unwrap() > 0 { tick(&store, &m, Duration::from_secs(20)).unwrap(); }
        assert_eq!(count(&store, "SELECT COUNT(*) FROM entities WHERE norm_name = ''"), 0);
        assert_eq!(count(&store, "SELECT COUNT(*) FROM entities WHERE norm_name = 'jonas'"), 1);
        let d: String = store.conn().query_row("SELECT description FROM entities WHERE norm_name = 'jonas'", [], |r| r.get(0)).unwrap();
        assert_eq!(d, "first; second");
        assert_eq!(count(&store, "SELECT MAX(LENGTH(name)) FROM entities"), 80);
        assert_eq!(count(&store, "SELECT COUNT(*) FROM relations"), 0, "self relation dropped");
    }

    #[test]
    fn norm_name_rules() {
        assert_eq!(norm_name("  Acme   Ltd. "), "acme ltd");
        assert_eq!(norm_name("Jonas!"), "jonas");
        assert_eq!(norm_name("créateSession"), "créatesession");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p singularrag-core --lib knowledge:: 2>&1 | grep -E '^error|test result' | head -3`
Expected: compile errors.

- [ ] **Step 3: Implement**

`knowledge.rs` (the shape; every function below is real code the implementer writes in full):

```rust
pub const EXTRACT_PER_TICK: usize = 5;
pub const EMBED_PER_TICK: usize = 50;
pub const MAX_ATTEMPTS: i64 = 3;

pub fn section_hash(body: &str) -> String { blake3::hash(body.as_bytes()).to_hex().to_string() }

pub fn norm_name(s: &str) -> String {
    let collapsed = s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    collapsed.trim_end_matches(['.', ',', ';', ':', '!', '?']).trim().to_string()
}

/// Called by the indexer for every section-kind symbol with a non-blank body, inside its transaction.
pub fn queue_section(conn: &Connection, symbol_id: i64, body: &str) -> Result<()> {
    conn.execute("INSERT OR REPLACE INTO extract_queue(symbol_id, hash, attempts, last_error, queued_at_ms) VALUES (?1, ?2, 0, NULL, ?3)",
        params![symbol_id, section_hash(body), now_ms()])?;
    Ok(())
}

/// Called from `delete_symbols_for` before the symbols go: mentions, relations (+ vec rows),
/// embeddings (+ vec rows), queue rows for the file's symbol ids; then `prune_entities`.
pub fn delete_for_symbols(conn: &Connection, file_id: i64) -> Result<()> { /* DELETE … WHERE symbol_id IN (SELECT id FROM symbols WHERE file_id = ?1), including DELETE FROM section_vec / relation_vec WHERE rowid IN (…); then prune_entities(conn) */ }

/// Entities with no mentions left are deleted with their vector rows; `mentions` is recounted.
pub fn prune_entities(conn: &Connection) -> Result<()> { /* UPDATE entities SET mentions = (SELECT COUNT(*) FROM entity_mentions m WHERE m.entity_id = entities.id); DELETE FROM entity_vec WHERE rowid IN (SELECT id FROM entities WHERE mentions = 0); DELETE FROM entities WHERE mentions = 0 */ }

pub fn pending(store: &Store) -> Result<usize> { /* SELECT COUNT(*) FROM extract_queue WHERE attempts < MAX_ATTEMPTS */ }

pub fn failed_sections(store: &Store) -> Result<Vec<(String, String, String)>> { /* path, symbol name, last_error for attempts >= MAX_ATTEMPTS */ }

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct KnowledgeTick { pub embedded: usize, pub extracted: usize, pub failed: usize, pub pending: usize, pub model_error: Option<String> }

/// Upserts entities by norm_name (extending a differing description), inserts mentions and
/// relations for this section, returns the new entity ids and relation ids that need vectors.
pub fn apply_extraction(conn: &Connection, symbol_id: i64, hash: &str, e: &Extraction) -> Result<(Vec<i64>, Vec<i64>)> { /* … */ }

pub fn tick(store: &Store, models: &Models, budget: Duration) -> Result<KnowledgeTick> {
    let start = Instant::now();
    let mut t = KnowledgeTick::default();
    // 1. Embeddings: queued sections without a section_embeddings row, up to EMBED_PER_TICK.
    //    Bodies come from sections_fts (content by rowid). Cache by hash in embedding_cache;
    //    only the misses go to models.embed. On the first vector, store::vec::ensure_tables(dim).
    //    Insert section_vec + section_embeddings + embedding_cache. On ModelUnavailable: set
    //    meta models_error, t.model_error, return.
    // 2. Extraction: up to EXTRACT_PER_TICK queue rows with attempts < MAX_ATTEMPTS and an
    //    embedding, oldest queued_at_ms first, while start.elapsed() < budget. For each:
    //    extraction_cache by hash, else models.extract (heading = symbol name, path from files).
    //    On ModelUnavailable whose message contains "not JSON": attempts += 1, last_error, continue;
    //    other ModelUnavailable: models_error, stop. On success: one transaction: apply_extraction,
    //    embed the new entity texts (name + ": " + description) and relation texts
    //    (src + " → " + dst + ": " + description) in one models.embed call, insert entity_vec /
    //    relation_vec, cache the extraction json, delete the queue row. t.extracted += 1.
    // 3. t.pending = pending(store); clear meta models_error on a fully successful tick; clear
    //    embeddings_rebuilding when the queue is empty.
}
```

`index.rs`: in `index_file`, after inserting a body row into `sections_fts`, call `crate::knowledge::queue_section(&tx, id, body)?`; in `delete_symbols_for`, call `crate::knowledge::delete_for_symbols(conn, file_id)?` first. `serve/queries.rs` `skipped`: append `knowledge::failed_sections` rows as `SkippedFile { path: format!("{path}::{name}"), reason: format!("extraction failed: {error}") }`.

- [ ] **Step 4: Run the tests**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace 2>&1 | grep -E 'test result|panicked|FAILED'`
Expected: all ok.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(core): the extraction queue, caches and tick; entities, mentions and relations with vectors"
```

---

### Task 4: The actor tick, header extras, status and `doctor`

**Files:**
- Modify: `crates/singularrag-core/src/map.rs` (`Extras`), `engine.rs` (`models()`, `knowledge_tick`, `index_meta` extras, `set_models_enabled`), `crates/singularrag/src/actor.rs`, `serve/queries.rs` (status fields), `serve/state.rs`/`watcher.rs` if the status broadcast needs them, `main.rs` (`doctor`), `tests/cli.rs`
- Modify: `ui/src/api/types.ts` (`Status` gains `entities_pending: number; models_unavailable: boolean; embeddings_rebuilding: boolean`) and the `status` fixtures in `ui/src` tests

**Interfaces:**
- Produces: `map::Extras { pending: usize, models_unavailable: bool, rebuilding: bool }` with `Default`; `map::header(version, head, stale, &Extras, retrieval_id)`; `map::freshness_line(version, head, stale, &Extras)` = the old `freshness_header` plus the segments; `Engine::models(&self) -> Option<&Models>` (`None` when `set_models_enabled(false)`); `Engine::set_models_enabled(&mut self, bool)`; `Engine::knowledge_tick(&mut self) -> Result<KnowledgeTick>` (20 s budget); `Engine::extras(&self) -> Result<Extras>`; actor: `Job::Knowledge(oneshot)` for tests plus the automatic tick: after any job that ran a refresh, `knowledge_pending = pending > 0`; the loop uses `recv_timeout(Duration::from_secs(30))` while `knowledge_pending` and runs `knowledge_tick` on timeout or after the drain finishes; `EngineHandle::knowledge_tick()`; `StatusDto` gains the three fields; CLI `singularrag doctor`.

- [ ] **Step 1: Write the failing tests**

`map.rs`:

```rust
    #[test]
    fn header_extras_render_in_order() {
        let x = Extras { pending: 12, models_unavailable: true, rebuilding: false };
        assert_eq!(freshness_line("abcdef12", Some("9b1e0d4f00"), 0, &x), "# singularrag · index abcdef · HEAD 9b1e0d4 · fresh · entities: 12 pending · models: unavailable");
        let r = Extras { pending: 0, models_unavailable: false, rebuilding: true };
        assert!(freshness_line("abcdef12", None, 3, &r).ends_with("STALE: 3 files changed since index · embeddings: rebuilding"));
        assert_eq!(header("abcdef12", None, 0, &Extras::default(), 7), "# singularrag · index abcdef · HEAD none · fresh · retrieval r_000007");
    }
```

`actor.rs`:

```rust
    #[tokio::test]
    async fn the_actor_ticks_the_queue_between_jobs_and_the_header_counts_down() {
        let dir = tempfile::tempdir().unwrap();
        singularrag_core::fixture::write_docs_mini(dir.path());
        let f = singularrag_core::fake_ollama::FakeOllama::spawn(8);
        std::env::set_var("SINGULARRAG_OLLAMA_URL", f.url());
        let (handle, _join, _died) = spawn(config(dir.path(), REFRESH_BUDGET));
        let first = handle.map(MapRequest { query: Some("stale".into()), ..Default::default() }).await.unwrap();
        assert!(first.text.contains("entities: ") && first.text.contains(" pending"), "{}", first.text.lines().next().unwrap());
        for _ in 0..10 { handle.knowledge_tick().await.unwrap(); }
        let later = handle.map(MapRequest { query: Some("stale".into()), ..Default::default() }).await.unwrap();
        assert!(!later.text.contains("pending"), "{}", later.text.lines().next().unwrap());
        std::env::remove_var("SINGULARRAG_OLLAMA_URL");
    }

    #[tokio::test]
    async fn a_model_outage_mid_tick_leaves_the_queue_intact() {
        let dir = tempfile::tempdir().unwrap();
        singularrag_core::fixture::write_docs_mini(dir.path());
        let f = singularrag_core::fake_ollama::FakeOllama::spawn(8);
        std::env::set_var("SINGULARRAG_OLLAMA_URL", f.url());
        let (handle, _join, _died) = spawn(config(dir.path(), REFRESH_BUDGET));
        handle.map(MapRequest { query: Some("stale".into()), ..Default::default() }).await.unwrap();
        let t = handle.knowledge_tick().await.unwrap();
        assert!(t.embedded > 0);
        f.set_down(true);
        let t = handle.knowledge_tick().await.unwrap();
        assert!(t.model_error.is_some() && t.extracted == 0, "{t:?}");
        let m = handle.map(MapRequest { query: Some("stale".into()), ..Default::default() }).await.unwrap();
        assert!(m.text.contains("models: unavailable") && m.text.contains("pending"), "{}", m.text.lines().next().unwrap());
        f.set_down(false);
        for _ in 0..10 { handle.knowledge_tick().await.unwrap(); }
        let m = handle.map(MapRequest { query: Some("stale".into()), ..Default::default() }).await.unwrap();
        assert!(!m.text.contains("models: unavailable") && !m.text.contains("pending"), "{}", m.text.lines().next().unwrap());
        std::env::remove_var("SINGULARRAG_OLLAMA_URL");
    }
```

(Env vars are process-wide: mark both tests `#[serial_test::serial]` if the crate has `serial_test`, else run the fake on a per-test URL and pass it through `EngineConfig.models_url: Option<String>` instead of the env var. Prefer the config field: add `models_url: Option<String>` to `EngineConfig`, `None` in existing tests, applied by the actor through `engine.set_models_url`.)

`tests/cli.rs`:

```rust
#[test]
fn doctor_reports_ollama_and_the_queue() {
    let dir = tempfile::tempdir().unwrap();
    singularrag_core::fixture::write_docs_mini(dir.path());
    let f = singularrag_core::fake_ollama::FakeOllama::spawn(8);
    Command::cargo_bin("singularrag").unwrap().args(["--repo", dir.path().to_str().unwrap(), "index"]).assert().success();
    Command::cargo_bin("singularrag").unwrap().env("SINGULARRAG_OLLAMA_URL", f.url())
        .args(["--repo", dir.path().to_str().unwrap(), "doctor"]).assert().success()
        .stdout(predicate::str::contains("ollama: ok").and(predicate::str::contains("qwen2.5:7b-instruct: pulled")).and(predicate::str::contains("pending:")));
    Command::cargo_bin("singularrag").unwrap().env("SINGULARRAG_OLLAMA_URL", "http://127.0.0.1:9")
        .args(["--repo", dir.path().to_str().unwrap(), "doctor"]).assert().success()
        .stdout(predicate::str::contains("ollama: unreachable"));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p singularrag-core --lib map:: 2>&1 | grep -E '^error' | head -3`
Expected: `cannot find type Extras`.

- [ ] **Step 3: Implement**

`map.rs`: `Extras` struct; `freshness_line` = the old body plus `if extras.pending > 0 { " · entities: N pending" }`, `if models_unavailable { " · models: unavailable" }`, `if rebuilding { " · embeddings: rebuilding" }`; keep `freshness_header(version, head, stale)` as `freshness_line(…, &Extras::default())` for the existing callers/tests; `header` takes `&Extras`. `engine.rs`: `models: Option<Models>` built in `open` from `ws.models` (`set_models_url` rebuilds it), `models_enabled: bool`; `extras()` reads `knowledge::pending`, meta `models_error`, meta `embeddings_rebuilding`; every `header(...)`/`freshness_header(...)` call site passes `&self.extras()?`; `knowledge_tick` calls `knowledge::tick` with `Duration::from_secs(20)` when models are enabled, else returns a default tick. `actor.rs`: `Job::Knowledge(oneshot::Sender<Reply<KnowledgeTick>>)`, `knowledge_pending` set from `engine.extras()` after Map/Find/Refresh/Trace/Changed/Entities jobs; the loop's blocking `recv()` becomes `recv_timeout(30 s)` when `knowledge_pending`, running a tick on `Timeout` and after each drain chunk returns false; `EngineHandle::knowledge_tick()`. `queries::status` fills the three fields from the store (pending via a count, the two flags via meta). `main.rs`: `Doctor` handled before `Engine::open`? No: it needs the workspace's models config and the store; open the Engine and print:

```
ollama: ok (http://127.0.0.1:11434) | unreachable (<error>)
qwen2.5:7b-instruct: pulled | missing
nomic-embed-text: pulled | missing
embedding dimension: 768 | unknown
pending: N sections · failed: M · last error: <text or none>
```

- [ ] **Step 4: Run the tests**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace 2>&1 | grep -E 'test result|panicked|FAILED'`; `cd ui && bun run typecheck && bun test 2>&1 | tail -2`
Expected: all ok.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: the actor runs the knowledge tick; header extras; status fields; singularrag doctor"
```

---

### Task 5: Seeds in `repo_map`

**Files:**
- Modify: `crates/singularrag-core/src/knowledge.rs` (`Seeds`, `seeds_for`), `rank.rs` (`rank_symbols(…, &Seeds)`, reasons), `engine.rs` (`MapRequest.entities/themes`, seeds in `repo_map`), `find.rs`/`eval.rs`/tests that call `rank_symbols`
- Modify: `crates/singularrag/src/main.rs` (`query --entity/--theme`, repeatable), `mcp/server.rs` + `tests/mcp.rs` (`repo_map` args `entities`, `themes`)
- Modify: `ui/src/api/types.ts`, `ui/src/lib/reasons.ts` (+ test), every `Reasons` literal in `ui/src` tests

**Interfaces:**
- Produces: `knowledge::Seeds { query_vec: Option<Vec<f32>>, entity_ids: Vec<i64>, entity_names: Vec<String>, theme_vecs: Vec<(String, Vec<f32>)>, models_unavailable: bool }` with `Default`; `knowledge::seeds_for(store, models: Option<&Models>, query: Option<&str>, entities: &[String], themes: &[String]) -> Result<Seeds>` (never errors on `ModelUnavailable`; sets the flag); `rank::rank_symbols(store, config, query, focus_files, seeds: &Seeds)`; `Reasons { …, semantic: Option<f64>, entities: Vec<String>, themes: Vec<String> }`; `MapRequest { …, entities: Vec<String>, themes: Vec<String> }`; constants `knowledge::SEMANTIC_K = 20`, `ENTITY_K = 10`, `ENTITY_MIN_SIM = 0.5`, `THEME_K = 10`.
- Seeding rules (spec §4): for each of the `SEMANTIC_K` nearest sections, its file's personalization += `FTS_FILE_BOOST × similarity` (seed label `semantic`), and the section is a hit with body weight `similarity` in `hit_shares`; for each matched entity, every mentioning section's file += `NOTE_BOOST` once per file (seed label `entity:<name>`), and the section is a hit with weight 1.0; themes: the sections stating the nearest relations, seed label `theme`, hit weight `similarity`.

- [ ] **Step 1: Write the failing tests** (`rank.rs` tests)

```rust
    fn knowledge_ready() -> (tempfile::TempDir, Store, crate::fake_ollama::FakeOllama, crate::models::Models) {
        let dir = tempfile::tempdir().unwrap();
        crate::fixture::write_docs_mini(dir.path());
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        let ws = crate::workspace::Workspace::single(dir.path()).unwrap();
        Indexer::new(&store, &ws, &MapConfig::default()).unwrap().refresh(None).unwrap();
        let f = crate::fake_ollama::FakeOllama::spawn(32);
        f.set_extraction("Freshness", serde_json::json!({"entities": [{"name": "STALE header", "type": "concept", "description": "the header when files changed"}], "relations": [{"source": "STALE header", "target": "createSession", "description": "refresh runs before session creation"}]}));
        f.set_extraction("Storage", serde_json::json!({"entities": [{"name": "SessionStore", "type": "system", "description": "keeps sessions"}, {"name": "createSession", "type": "system", "description": "creates a session"}], "relations": []}));
        let m = crate::models::Models::new(crate::models::ModelsConfig { ollama: f.url(), ..Default::default() });
        while crate::knowledge::pending(&store).unwrap() > 0 { crate::knowledge::tick(&store, &m, std::time::Duration::from_secs(20)).unwrap(); }
        (dir, store, f, m)
    }

    #[test]
    fn the_semantic_seed_finds_a_section_with_no_keyword_overlap() {
        let (_d, store, _f, m) = knowledge_ready();
        // The fake embeds by bag of words; "Wait and call again" is the runbook's prose, and
        // the query shares those words but none of the heading's, so FTS on names finds nothing.
        let seeds = crate::knowledge::seeds_for(&store, Some(&m), Some("wait and call again"), &[], &[]).unwrap();
        assert!(seeds.query_vec.is_some() && !seeds.models_unavailable);
        let ranked = rank_symbols(&store, &MapConfig::default(), Some("wait and call again"), &[], &seeds).unwrap();
        let hit = ranked.iter().find(|s| s.name == "When the header says STALE").unwrap();
        assert!(hit.reasons.semantic.is_some_and(|v| v > 0.5), "{:?}", hit.reasons);
        assert!(hit.reasons.seeds.contains(&"semantic".to_string()));
        assert!(ranked.iter().position(|s| s.name == "When the header says STALE").unwrap() < 5, "{:?}", ranked.iter().take(5).map(|s| &s.name).collect::<Vec<_>>());
    }

    #[test]
    fn the_entity_seed_matches_by_term_argument_and_vector() {
        let (_d, store, _f, m) = knowledge_ready();
        let by_term = crate::knowledge::seeds_for(&store, Some(&m), Some("what is the stale header"), &[], &[]).unwrap();
        assert!(by_term.entity_names.iter().any(|n| n == "STALE header"), "{:?}", by_term.entity_names);
        let by_arg = crate::knowledge::seeds_for(&store, None, Some("anything"), &["sessionstore".into()], &[]).unwrap();
        assert_eq!(by_arg.entity_names, vec!["SessionStore".to_string()]);
        let ranked = rank_symbols(&store, &MapConfig::default(), Some("anything"), &[], &by_arg).unwrap();
        let storage = ranked.iter().find(|s| s.name == "Storage").unwrap();
        assert_eq!(storage.reasons.entities, vec!["SessionStore".to_string()]);
        assert!(storage.reasons.seeds.contains(&"entity:SessionStore".to_string()));
    }

    #[test]
    fn themes_match_relations_and_seeds_are_empty_without_models() {
        let (_d, store, f, m) = knowledge_ready();
        let s = crate::knowledge::seeds_for(&store, Some(&m), None, &[], &["refresh before session creation".into()]).unwrap();
        assert_eq!(s.theme_vecs.len(), 1);
        let ranked = rank_symbols(&store, &MapConfig::default(), None, &[], &s).unwrap();
        let fresh = ranked.iter().find(|s| s.name == "Freshness").unwrap();
        assert!(!fresh.reasons.themes.is_empty(), "{:?}", fresh.reasons);
        f.set_down(true);
        let off = crate::knowledge::seeds_for(&store, Some(&m), Some("x"), &[], &["y".into()]).unwrap();
        assert!(off.models_unavailable && off.query_vec.is_none() && off.theme_vecs.is_empty());
        let none = crate::knowledge::seeds_for(&store, None, Some("x"), &[], &[]).unwrap();
        assert!(!none.models_unavailable && none.query_vec.is_none(), "models disabled is not an outage");
    }
```

`engine.rs` test: `repo_map` with `entities: vec!["SessionStore".into()]` renders `docs/design.md:` with the `## Storage` row and the retrieval item's `reasons_json` contains `"entities":["SessionStore"]`; with the fake down, the header carries `models: unavailable` and the map still renders. `reasons.test.ts`: `semantic: 0.83` → "Semantically close to the query (0.83).", `entities: ["Acme"]` → "Mentions Acme.", `themes: ["x"]` → "Matches the theme x.".

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p singularrag-core --lib rank:: 2>&1 | grep -E '^error' | head -3`
Expected: `cannot find Seeds`.

- [ ] **Step 3: Implement**

`knowledge::seeds_for`: query terms via `tokens::query_terms`; `entity_ids` from `SELECT id, name FROM entities WHERE norm_name IN (terms…)` plus `norm_name(arg)` for each argument plus, when `query_vec` is available, `vec::knn(entity_vec, query_vec, ENTITY_K)` filtered `sim >= ENTITY_MIN_SIM`; `query_vec` from `models.embed(&[query])` when a query and models exist (`ModelUnavailable` → flag, none); `theme_vecs` one embed call for all themes. `rank_symbols`: after the FTS/body hits, compute `semantic_hits: HashMap<i64, f64>` from `knn(section_vec, query_vec, SEMANTIC_K)`, `entity_sections: HashMap<i64, Vec<String>>` from `entity_mentions` joined to `entities` for the matched ids, `theme_sections: HashMap<i64, (String, f64)>` from `knn(relation_vec, theme_vec, THEME_K)` joined to `relations.symbol_id`; personalization and seeds per rule; the `hit_shares` input per symbol becomes `(name_hit, body_rank.max(semantic))` with entity and theme hits as name-weight (1.0) hits; reasons filled. `MapRequest` gains the two vectors (Default empty); `repo_map` computes `seeds_for(&self.store, self.models(), req.query.as_deref(), &req.entities, &req.themes)` before ranking and folds `seeds.models_unavailable` into the header extras for that response. CLI `query --entity NAME --theme TEXT` (repeatable), MCP `repo_map` schema: `entities: Option<Vec<String>>`, `themes: Option<Vec<String>>`, both documented in the description as "optional: entity names you already know matter; themes as short phrases".

- [ ] **Step 4: Run the tests**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace 2>&1 | grep -E 'test result|panicked|FAILED'`; `cd ui && bun run typecheck && bun test 2>&1 | tail -2`
Expected: all ok.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(core): semantic, entity and theme seeds in repo_map with drawable reasons; entities/themes arguments"
```

---

### Task 6: The `entities` tool

**Files:**
- Modify: `crates/singularrag-core/src/knowledge.rs` (`EntityHit`, `match_entities`, `render_entities`), `engine.rs` (`EntitiesRequest/Response`, `Engine::entities`), `crates/singularrag/src/actor.rs` (`Job::Entities`), `mcp/server.rs` + `tests/mcp.rs` (tool, `ENTITIES_DESCRIPTION`, `INSTRUCTIONS`), `main.rs` (`entities` subcommand), `tests/cli.rs`, README tool count

**Interfaces:**
- Produces: `knowledge::EntityHit { id, name, r#type, description, mentions: usize, relations: Vec<RelationHit>, sections: Vec<CitedSection { symbol_id, path, name, line_start, line_end }> }`, `RelationHit { src: String, dst: String, description: String, section: CitedSection }`; `knowledge::match_entities(store, models: Option<&Models>, query: &str, extra: &[String], limit: usize) -> Result<(Vec<EntityHit>, bool models_unavailable)>` (matching rule = the entity seed's; entities ordered by term match first, then similarity, then mentions; relations capped at 30 overall; sections deduped in first-seen order); `knowledge::render_entities(hits: &[EntityHit]) -> String`; `EntitiesRequest { query: String, entities: Vec<String>, limit: usize }`, `EntitiesResponse { retrieval_id, text, entities: usize, stale_count, lock_timeout }`; `ENTITIES_LIMIT_DEFAULT = 10`, `ENTITIES_LIMIT_MAX = 25`.
- Output, exact:

```
# singularrag · index … · retrieval r_000009
STALE header (concept): the header when files changed
  ← docs/design.md::Freshness (lines 5-8)
  STALE header → createSession: refresh runs before session creation (docs/design.md::Freshness, lines 5-8)
SessionStore (system): keeps sessions
  ← docs/design.md::Storage (lines 9-11)
# 2 entities · 1 relation · 2 sections
```

- [ ] **Step 1: Write the failing tests** (`knowledge.rs` tests, `engine.rs` test, `tests/mcp.rs`, `tests/cli.rs`)

```rust
    #[test]
    fn match_and_render_entities_with_their_relations_and_sections() {
        let (_d, store, _f, m) = crate::rank::tests::knowledge_ready(); // make that helper pub(crate)
        let (hits, off) = match_entities(&store, Some(&m), "stale header", &[], 10).unwrap();
        assert!(!off);
        assert_eq!(hits[0].name, "STALE header");
        assert_eq!(hits[0].relations.len(), 1);
        assert_eq!(hits[0].sections[0].name, "Freshness");
        let text = render_entities(&hits);
        assert!(text.starts_with("STALE header (concept): the header when files changed\n  ← docs/design.md::Freshness (lines "), "{text}");
        assert!(text.contains("STALE header → createSession: refresh runs before session creation (docs/design.md::Freshness, lines "), "{text}");
        assert!(text.trim_end().ends_with(" sections"), "{text}");
        let (none, _) = match_entities(&store, None, "nothing matches this", &[], 10).unwrap();
        assert!(none.is_empty());
        assert_eq!(render_entities(&none), "# 0 entities · 0 relations · 0 sections\n");
    }
```

`engine.rs`: `Engine::entities(&EntitiesRequest { query: "stale header", entities: vec![], limit: 10 })` records a retrieval with `tool = "entities"`, served items `docs/design.md::Freshness` first, `limit_n = 10`; `limit: 100` is clamped to 25. `tests/mcp.rs`: the tool list has six tools with the spec descriptions; an `entities` round trip over the binary with the fake at `SINGULARRAG_OLLAMA_URL` after a warm-up of `knowledge_tick` calls (through the MCP `repo_map` calls the actor ticks itself; the test waits until the header has no `pending`, polling `repo_map` up to 30 times with a 200 ms sleep). `tests/cli.rs`: `singularrag entities "stale header"` prints the block (the CLI ticks the queue to completion first, explicitly, since the one-shot CLI has no background job: `Cmd::Entities` calls `engine.knowledge_tick()` until pending is 0 or a model error, then answers).

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p singularrag-core --lib knowledge::tests::match_and 2>&1 | grep -E '^error' | head -3`
Expected: `cannot find function match_entities`.

- [ ] **Step 3: Implement**

`ENTITIES_DESCRIPTION`: `"What the corpus knows about a person, system, concept or event, and how it connects: matched entities with type and description, their relations, and the document sections that state them (path::heading, lines). `query` is a name or a question; `entities` are names you already know; `limit` defaults to 10, max 25. Descriptions are extracted text, not verified facts. Use it before reading a document about someone or something."` `INSTRUCTIONS` gains one sentence after the changed sentence: `"Use entities to learn what the corpus says about a person, system or concept and how it connects; prose queries to repo_map work in plain language."` and the closing note `"Entity descriptions are extracted text, not verified facts."` Three copies each. `Cmd::Entities { query, entity: Vec<String>, limit }`.

- [ ] **Step 4: Run the tests**

Run: the full gate.
Expected: all ok; `lists_exactly_the_six_tools_with_spec_descriptions` (renamed) passes.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: the entities tool (core, engine, actor, MCP, CLI); six tools in the agent-facing text"
```

---

### Task 7: The prose fixture, the checked-in extraction, `--no-models`

**Files:**
- Create: `crates/singularrag-core/fixtures/prose/voyage.md`, `handbook.md`, `extraction.json`
- Modify: `crates/singularrag-core/src/fixture.rs` (`write_prose`), `knowledge.rs` (`load_extraction_json`), `engine.rs`/`main.rs` (`eval --no-models`), `tests/cli.rs`
- Create: `e*/questions-prose.toml` (Write tool)

**Interfaces:**
- Produces: `fixture::write_prose(root: &Path)` (writes the two Markdown files under `docs/`); `knowledge::load_extraction_json(store, models: Option<&Models>, json: &str) -> Result<usize>` (applies a checked-in `{ "<path>::<heading>": Extraction }` map to the matching queued sections, embedding through `models` when given, else leaving embeddings absent; returns sections applied); CLI `eval --no-models`.
- `voyage.md` (write it in full in the fixture file; about 600 words, original prose, no external source): a short account of a fictional 1890s research voyage: the ship *Aurelia*, captain Ingrid Halvorsen, naturalist Tomas Berg, the port of Tromsø, the Kolbeinsey islet, the Bergen Museum that funded it, a storm on 14 August, and a report delivered to the Museum. Headings: `# The Aurelia voyage`, `## Departure`, `## The storm`, `## Kolbeinsey`, `## The report`.
- `handbook.md` (about 500 words): a fictional company handbook: `# Northwind handbook`, `## Onboarding` (roles: People team, IT desk, buddy; system: Okta), `## Expense process` (steps: submit in Expensify, manager approval, Finance review within 5 days), `## Incident process` (on-call engineer, PagerDuty, the incident channel, a post-mortem within 3 days), `## Release process` (release captain, staging, changelog).
- `extraction.json`: hand-written entities and relations per section consistent with the prose (at least 12 entities and 10 relations across the nine sections), typed with the closed list.
- `questions-prose.toml`: twelve questions, `paraphrase` (4), `entity` (4), `relation` (4), gold as `docs/voyage.md::The storm`-style sections, authored from the two files.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn the_prose_fixture_loads_its_extraction_without_a_model() {
        let dir = tempfile::tempdir().unwrap();
        crate::fixture::write_prose(dir.path());
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        let ws = Workspace::single(dir.path()).unwrap();
        Indexer::new(&store, &ws, &MapConfig::default()).unwrap().refresh(None).unwrap();
        let n = load_extraction_json(&store, None, include_str!("../fixtures/prose/extraction.json")).unwrap();
        assert!(n >= 9, "{n}");
        assert!(count(&store, "SELECT COUNT(*) FROM entities") >= 12);
        assert!(count(&store, "SELECT COUNT(*) FROM relations") >= 10);
        assert_eq!(pending(&store).unwrap(), 0);
        let (hits, _) = match_entities(&store, None, "who captained the Aurelia", &[], 5).unwrap();
        assert!(hits.iter().any(|h| h.name == "Ingrid Halvorsen"), "{:?}", hits.iter().map(|h| &h.name).collect::<Vec<_>>());
    }
```

`tests/cli.rs`: `eval --questions <abs path to questions-prose.toml> --budget 4096 --no-models` on a tempdir with `write_prose` prints `mean recall` and exits 0; the question file loads with 12 questions in the three categories and every gold exists (same pattern as the docs-gold test, against a tempdir copy).

- [ ] **Step 2: Run** — Expected: compile errors, then file-missing.

- [ ] **Step 3: Implement** — the fixture files, `write_prose`, `load_extraction_json` (looks up each key's symbol by path + name, applies `apply_extraction` with the section's hash, removes its queue row, caches the JSON; embeds sections and new entities/relations through `models` when `Some`), `--no-models` → `engine.set_models_enabled(false)`.

- [ ] **Step 4: Run the gate.**

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: the prose fixture with a checked-in extraction; questions-prose.toml; eval --no-models"
```

---

### Task 8: `GET /api/entities` and status

**Files:**
- Modify: `crates/singularrag/src/serve/queries.rs` (`EntitiesDto`, `entities(store)`), `routes.rs`, `mod.rs` (route), tests in `routes.rs`

**Interfaces:**
- Produces: `GET /api/entities` → `{ "entities": [{ "id", "name", "type", "description", "mentions" }], "relations": [{ "id", "src", "dst", "description", "symbol_id", "path", "name" }], "mentions": [{ "entity_id", "symbol_id", "path", "name" }] }`, entities ordered by mentions desc then name, capped at 500 entities and 2000 relations/mentions (the cap is stated in the response as `"truncated": bool`).

- [ ] **Step 1: Write the failing test** — a route test on a `write_prose` tempdir with the extraction loaded (`load_extraction_json`) asserting the three arrays, the ordering, and `truncated: false`.
- [ ] **Step 2: Run** — 404.
- [ ] **Step 3: Implement** — queries + route behind the token layer like the others.
- [ ] **Step 4: Run the gate.**
- [ ] **Step 5: Commit** — `feat(serve): GET /api/entities for the knowledge overlay`.

---

### Task 9: UI: the entity overlay, detail panel, status and reasons

**Files:**
- Modify: `ui/src/api/types.ts` (`EntitiesPayload`, `Entity`, `Relation`, `Mention`), `client.ts` (`entities: () => req<EntitiesPayload>("GET", "/entities")`)
- Modify: `ui/src/components/ViewToggle.tsx` (an `Overlay` radio group component `OverlayToggle` with `files` | `entities`, exported from the same file, shown only in map view), `ui/src/lib/graph.ts` (`buildGraph(payload, entities?: EntitiesPayload)` adds `entity:<id>` nodes with `{ kind: "entity", type, mentions, description }`, dashed mention edges to file nodes, solid relation edges), `ui/src/lib/mapStyle.ts` (`NodeCtx.entityType?: string`; entity colour per type from palette tokens `--map-entity-person` … seven tokens in `index.css` light and dark; size from mentions), `ui/src/components/MapView.tsx` (entity node reducer, accessible name `<name> (<type>), N mentions`, click → `onSelectEntity(id)`), `ui/src/components/DetailPanel.tsx` (a section row lists its entities and relations; an `EntityRow` view when an entity is focused: description, mentions with "Go to section" buttons that focus the treegrid row, relations), `ui/src/components/FreshnessBadge.tsx` (` · entities: N pending`, ` · models unavailable`, ` · embeddings rebuilding`), `ui/src/components/SkippedSheet.tsx` (no change: reasons flow through), `ui/src/App.tsx` (`overlay` state, `entities` fetched on load and on change events, `focusedEntity`), tests: `graph.test.ts`, `mapStyle.test.ts`, `App.test.tsx`, `DetailPanel.test.tsx` if present else in `App.test.tsx`

**Interfaces:** `TreeRow` gains a variant `{ kind: "entity"; entity: Entity }`; `Reasons` sentences from Task 5 already exist.

- [ ] **Step 1: Write the failing tests** — `graph.test.ts`: `buildGraph(payload, entities)` adds `entity:1` with `kind: "entity"`, a mention edge to `docs/a.md` and a relation edge to `entity:2`; without `entities` the graph is unchanged. `mapStyle.test.ts`: an entity node with `entityType: "person"` gets `pal.entityPerson` and a size that grows with mentions. `App.test.tsx`: in map view the `Overlay` radio group appears; choosing `Files + entities` fetches `/api/entities` (mock) and the map's accessible label says `… and 2 entities`; clicking the mocked entity node path is not testable in jsdom, so focus an entity through a new "Entities" list in the detail panel's empty state (a list of the top 10 entities by mentions, each a button) and assert the panel shows its description, one mention with a "Go to section" button, and one relation; the freshness badge renders `entities: 3 pending`; axe over the panel with an entity focused.
- [ ] **Step 2: Run** — failures.
- [ ] **Step 3: Implement** — as listed; the top-entities list in the panel's empty state is the keyboard path to an entity (map clicks are the mouse path).
- [ ] **Step 4: Run** — `cd ui && bun run typecheck && bun test && bun run build`, then `cargo test --workspace`.
- [ ] **Step 5: Commit** — `feat(ui): entity overlay on the map, entity focus in the detail panel, status lines`.

---

### Task 10: Docs, the vault set, the gate record

**Files:**
- Modify: `README.md` (install Ollama + pull the two models; `[models]`; `doctor`; the `entities` tool in "What the agent sees"; the vault question file location and how to author it with `singularrag find`), `docs/superpowers/specs/2026-09-19-singularrag-design.md` (§5, §7, §9, §10, §12 amendments marked *Amended 2026-09-23 (knowledge design)*), `e*/README.md` (a "Tier one on knowledge" section with the prose set seeds-off and, when Ollama is present, seeds-on; the vault set numbers when Jonas runs it), `tests/cli.rs` (README tool count six)

- [ ] **Step 1: Write the failing test** — `readme_lists_six_tools_and_the_models_setup`: README contains "Six tools", "ollama pull nomic-embed-text", "singularrag doctor".
- [ ] **Step 2: Run** — fails.
- [ ] **Step 3: Implement** — the docs; run the prose set with `--no-models` and, if `singularrag doctor` reports Ollama ok on this machine, with seeds on; record both (or "seeds-on not run: Ollama not installed") in the tier-one README; write the vault instructions (`[[root]] name = "vault"`, `~/.singularrag/questions-vault.toml`, `singularrag eval --questions ~/.singularrag/questions-vault.toml --budget 4096`).
- [ ] **Step 4: Run the gate.**
- [ ] **Step 5: Commit** — `docs+eval: knowledge setup, six tools, parent spec amended; prose set recorded`.

---

## Self-review

- Spec §2 → Tasks 1, 2, 4 (`doctor`); §3 → Tasks 2, 3, 4 (tick placement); §4 → Tasks 5, 6; §5 → Task 9; §6 → Tasks 7, 10; §7 → Tasks 1 (data cleaning), 3 (nothing sent for skipped files: the queue is fed only from `sections_fts`, which secret-like files never reach), 4 (lock per transaction); §8 → Task 10; §9 → each task; §10 holds.
- Names: `Models::{embed, extract, tags}`, `Extraction`, `SectionInput` (T1) used in T3, T5, T6, T7; `store::vec::{ensure_tables, insert, delete, knn, dim}` (T2) in T3, T5; `knowledge::{tick, pending, seeds_for, match_entities, render_entities, load_extraction_json, failed_sections}` consistent across T3–T8; `map::Extras`/`freshness_line`/`header` (T4) in T5, T6; `EntitiesPayload` (T8) in T9.
- Recorded choices: extraction results are cached by section hash in `extraction_cache`/`embedding_cache` so a re-indexed file re-queues cheaply (the spec keys rows by hash; the plan keys rows by symbol id and caches by hash, same effect); the fake Ollama lives in core as a std server so every crate's tests share it; `EngineConfig.models_url` carries the fake's URL into actor tests instead of a process-wide env var.
- Review Focus 1–5 have named tests in Tasks 2, 1, 3, 3, 4.
