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
pub const ENTITY_TYPES: [&str; 9] = [
    "person",
    "organisation",
    "system",
    "concept",
    "event",
    "place",
    "document",
    "process",
    "role",
];
pub const MAX_STEPS: usize = 30;
pub const STEP_TEXT_MAX: usize = 300;
pub const ROLE_MAX: usize = 80;
const TIMEOUT: Duration = Duration::from_secs(30);

pub const EXTRACT_PROMPT: &str = "You extract a knowledge graph from one section of a document.\n\
Return ONLY a JSON object of the shape {\"entities\": [{\"name\": string, \"type\": string, \"description\": string}], \"relations\": [{\"source\": string, \"target\": string, \"description\": string}]}.\n\
Types are exactly one of: person, organisation, system, concept, event, place, document, process (a named sequence of steps people follow), role (a job or position that acts in a process).\n\
Descriptions are one short sentence from the text. Relations connect two entity names from your list.\n\
If the section describes a process, include one process entity named as the section names it (usually its heading), and its roles.\n\
Document: {path}\nSection: {heading}\n\nText:\n{text}\n";
const STRICT_SUFFIX: &str = "\nYour previous answer was not valid JSON. Answer with the JSON object only, no prose, no code fence.";

pub const STEPS_PROMPT_FIRST_LINE: &str = "You list the steps of a process";
pub const STEPS_PROMPT: &str = "You list the steps of a process described in one section of a document.\n\
Return ONLY a JSON object of the shape {\"process\": string, \"steps\": [{\"text\": string, \"role\": string, \"systems\": [string]}]}, steps in reading order.\n\
\"process\" is the name of the process the section describes, as the text names it. Each step's \"text\" is one short sentence from the text; \"role\" is who acts, or an empty string; \"systems\" are the tools or systems the step touches, by the names the text uses.\n\
Document: {path}\nSection: {heading}\n\nText:\n{text}\n";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelsConfig {
    pub ollama: String,
    pub extract: String,
    pub embed: String,
    pub api: Option<String>,
}

impl Default for ModelsConfig {
    fn default() -> Self {
        ModelsConfig {
            ollama: DEFAULT_OLLAMA.into(),
            extract: DEFAULT_EXTRACT.into(),
            embed: DEFAULT_EMBED.into(),
            api: None,
        }
    }
}

impl ModelsConfig {
    /// `SINGULARRAG_OLLAMA_URL` wins over the file: tests and the CLI point at a fake.
    pub fn with_env(mut self) -> Self {
        if let Ok(u) = std::env::var("SINGULARRAG_OLLAMA_URL") {
            if !u.is_empty() {
                self.ollama = u;
            }
        }
        self
    }
}

pub struct SectionInput<'a> {
    pub path: &'a str,
    pub heading: &'a str,
    pub text: &'a str,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Extraction {
    #[serde(default)]
    pub entities: Vec<ExtractedEntity>,
    #[serde(default)]
    pub relations: Vec<ExtractedRelation>,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ExtractedEntity {
    #[serde(default)]
    pub name: String,
    #[serde(default, rename = "type")]
    pub r#type: String,
    #[serde(default)]
    pub description: String,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ExtractedRelation {
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StepsAnswer {
    #[serde(default)]
    pub process: String,
    #[serde(default)]
    pub steps: Vec<ExtractedStep>,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ExtractedStep {
    pub text: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub systems: Vec<String>,
}

#[derive(Debug)]
pub struct Models {
    cfg: ModelsConfig,
    http: reqwest::blocking::Client,
    /// A timed-out request is sent once more. Off for the background tick, which runs on
    /// the actor thread: a retry would hold a waiting tool call for a second timeout.
    retry_on_timeout: bool,
}

fn unavailable(e: impl std::fmt::Display) -> Error {
    Error::ModelUnavailable(e.to_string())
}

/// Truncate to `max` characters, strip control characters, collapse whitespace.
pub fn clean(s: &str, max: usize) -> String {
    let s: String = s.chars().filter(|c| !c.is_control()).collect();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    s.chars().take(max).collect()
}

pub fn normalise_extraction(e: Extraction) -> Extraction {
    let entities: Vec<ExtractedEntity> = e
        .entities
        .into_iter()
        .filter_map(|x| {
            let name = clean(&x.name, NAME_MAX);
            if name.chars().all(|c| !c.is_alphanumeric()) {
                return None;
            }
            let t = x.r#type.trim().to_lowercase();
            let r#type = if ENTITY_TYPES.contains(&t.as_str()) {
                t
            } else {
                "concept".to_string()
            };
            Some(ExtractedEntity {
                name,
                r#type,
                description: clean(&x.description, DESCRIPTION_MAX),
            })
        })
        .collect();
    let relations = e
        .relations
        .into_iter()
        .filter_map(|r| {
            let source = clean(&r.source, NAME_MAX);
            let target = clean(&r.target, NAME_MAX);
            if source.is_empty() || target.is_empty() || source == target {
                return None;
            }
            Some(ExtractedRelation {
                source,
                target,
                description: clean(&r.description, DESCRIPTION_MAX),
            })
        })
        .collect();
    Extraction {
        entities,
        relations,
    }
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
            let systems = s
                .systems
                .iter()
                .map(|x| clean(x, NAME_MAX))
                .filter(|x| !x.is_empty())
                .collect();
            Some(ExtractedStep {
                text,
                role: clean(&s.role, ROLE_MAX),
                systems,
            })
        })
        .take(MAX_STEPS)
        .collect();
    StepsAnswer {
        process: clean(&a.process, NAME_MAX),
        steps,
    }
}

/// The JSON object inside a model answer: fences and prose around it are dropped.
fn parse_json<T: serde::de::DeserializeOwned>(text: &str) -> Option<T> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str::<T>(&text[start..=end]).ok()
}

/// The JSON object inside a model answer: fences and prose around it are dropped.
pub fn parse_extraction(text: &str) -> Option<Extraction> {
    parse_json(text)
}

/// The JSON object inside a model answer: fences and prose around it are dropped.
pub fn parse_steps(text: &str) -> Option<StepsAnswer> {
    parse_json(text)
}

/// Paragraph-bounded parts of at most `MAX_PART_CHARS`; a single oversized paragraph is cut hard.
pub fn split_parts(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    for para in text.split("\n\n") {
        if !cur.is_empty() && cur.len() + 2 + para.len() > MAX_PART_CHARS {
            parts.push(std::mem::take(&mut cur));
        }
        if para.len() > MAX_PART_CHARS {
            let mut s = para;
            while s.len() > MAX_PART_CHARS {
                let (h, t) = s.split_at(
                    s.char_indices()
                        .nth(MAX_PART_CHARS)
                        .map(|(i, _)| i)
                        .unwrap_or(s.len()),
                );
                parts.push(h.to_string());
                s = t;
            }
            cur = s.to_string();
            continue;
        }
        if !cur.is_empty() {
            cur.push_str("\n\n");
        }
        cur.push_str(para);
    }
    if !cur.trim().is_empty() {
        parts.push(cur);
    }
    parts
}

impl Models {
    pub fn new(cfg: ModelsConfig) -> Models {
        Models::with_timeout(cfg, TIMEOUT)
    }

    /// `new` with another per-request timeout (tests use a short one).
    pub fn with_timeout(cfg: ModelsConfig, timeout: Duration) -> Models {
        let http = reqwest::blocking::Client::builder()
            .timeout(timeout)
            .build()
            .expect("reqwest client");
        Models {
            cfg,
            http,
            retry_on_timeout: true,
        }
    }

    /// The same client without the retry on a timeout, for the background tick.
    pub fn tick_client(&self) -> Models {
        Models {
            cfg: self.cfg.clone(),
            http: self.http.clone(),
            retry_on_timeout: false,
        }
    }
    pub fn config(&self) -> &ModelsConfig {
        &self.cfg
    }

    fn get(&self, path: &str) -> Result<serde_json::Value> {
        self.send(path, None)
    }

    fn post(&self, path: &str, body: &serde_json::Value) -> Result<serde_json::Value> {
        self.send(path, Some(body))
    }

    /// One retry on connect errors, and on timeouts unless `retry_on_timeout` is off; a
    /// non-2xx status or a non-JSON body is unavailable.
    fn send(&self, path: &str, body: Option<&serde_json::Value>) -> Result<serde_json::Value> {
        let url = format!("{}{}", self.cfg.ollama.trim_end_matches('/'), path);
        let mut last = None;
        for _ in 0..2 {
            let req = match body {
                Some(b) => self.http.post(&url).json(b),
                None => self.http.get(&url),
            };
            match req.send() {
                Ok(resp) => {
                    let status = resp.status();
                    if !status.is_success() {
                        let text = resp.text().unwrap_or_default();
                        return Err(unavailable(format!(
                            "{path}: HTTP {status}: {}",
                            clean(&text, 200)
                        )));
                    }
                    return resp.json().map_err(|e| unavailable(format!("{path}: {e}")));
                }
                Err(e) if e.is_connect() || (e.is_timeout() && self.retry_on_timeout) => {
                    last = Some(e)
                }
                Err(e) => return Err(unavailable(format!("{path}: {e}"))),
            }
        }
        Err(unavailable(format!(
            "{path}: {}",
            last.map(|e| e.to_string()).unwrap_or_default()
        )))
    }

    pub fn tags(&self) -> Result<Vec<String>> {
        let v = self.get("/api/tags")?;
        let models = v["models"]
            .as_array()
            .ok_or_else(|| unavailable("tags: no models field"))?;
        Ok(models
            .iter()
            .filter_map(|m| m["name"].as_str().map(str::to_string))
            .collect())
    }

    pub fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let mut out = Vec::with_capacity(texts.len());
        for chunk in texts.chunks(EMBED_BATCH) {
            let v = self.post(
                "/api/embed",
                &serde_json::json!({ "model": self.cfg.embed, "input": chunk }),
            )?;
            let arr = v["embeddings"]
                .as_array()
                .ok_or_else(|| unavailable("embed: no embeddings field"))?;
            if arr.len() != chunk.len() {
                return Err(unavailable(format!(
                    "embed: {} vectors for {} texts",
                    arr.len(),
                    chunk.len()
                )));
            }
            for e in arr {
                let vec = e
                    .as_array()
                    .ok_or_else(|| unavailable("embed: vector is not an array"))?
                    .iter()
                    .map(|x| {
                        x.as_f64()
                            .map(|f| f as f32)
                            .ok_or_else(|| unavailable("embed: vector element is not a number"))
                    })
                    .collect::<Result<Vec<f32>>>()?;
                out.push(vec);
            }
        }
        Ok(out)
    }

    fn generate(&self, prompt: &str) -> Result<String> {
        let v = self.post("/api/generate", &serde_json::json!({ "model": self.cfg.extract, "prompt": prompt, "format": "json", "stream": false, "options": { "temperature": 0 } }))?;
        v["response"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| unavailable("generate: no response field"))
    }

    /// One prompt per part; parts are merged. A malformed answer gets one stricter retry.
    pub fn extract(&self, input: &SectionInput) -> Result<Extraction> {
        let mut merged = Extraction::default();
        for part in split_parts(input.text) {
            let prompt = EXTRACT_PROMPT
                .replace("{path}", input.path)
                .replace("{heading}", input.heading)
                .replace("{text}", &part);
            let first = self.generate(&prompt)?;
            let parsed = match parse_extraction(&first) {
                Some(e) => e,
                None => {
                    let second = self.generate(&format!("{prompt}{STRICT_SUFFIX}"))?;
                    parse_extraction(&second).ok_or_else(|| {
                        unavailable(format!(
                            "extract: not JSON after retry: {}",
                            clean(&second, 120)
                        ))
                    })?
                }
            };
            let e = normalise_extraction(parsed);
            merged.entities.extend(e.entities);
            merged.relations.extend(e.relations);
        }
        Ok(merged)
    }

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
                    parse_steps(&second).ok_or_else(|| {
                        unavailable(format!(
                            "steps: not JSON after retry: {}",
                            clean(&second, 120)
                        ))
                    })?
                }
            };
            if merged.process.is_empty() {
                merged.process = parsed.process;
            }
            merged.steps.extend(parsed.steps);
        }
        Ok(normalise_steps(merged))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_ollama::FakeOllama;

    fn models(f: &FakeOllama) -> Models {
        Models::new(ModelsConfig {
            ollama: f.url(),
            ..ModelsConfig::default()
        })
    }

    #[test]
    fn embed_batches_and_returns_one_vector_per_text() {
        let f = FakeOllama::spawn(16);
        let m = models(&f);
        let texts: Vec<String> = (0..120).map(|i| format!("text {i}")).collect();
        let out = m.embed(&texts).unwrap();
        assert_eq!(out.len(), 120);
        assert!(out.iter().all(|v| v.len() == 16));
        assert_eq!(
            f.calls()
                .iter()
                .filter(|p| p.as_str() == "/api/embed")
                .count(),
            3,
            "batches of 50"
        );
        let a = m.embed(&["the login flow".into()]).unwrap();
        let b = m.embed(&["the login flow".into()]).unwrap();
        assert_eq!(a, b, "deterministic");
    }

    #[test]
    fn extract_parses_the_models_json_and_normalises_it() {
        let f = FakeOllama::spawn(8);
        f.set_extraction(
            "Freshness",
            serde_json::json!({
                "entities": [
                    {"name": "  Acme Ltd ", "type": "Company", "description": "a firm\u{0007}"},
                    {"name": "", "type": "person", "description": "nobody"},
                    {"name": "x".repeat(200), "type": "person", "description": "y".repeat(400)}
                ],
                "relations": [{"source": "Acme Ltd", "target": "Jonas", "description": "employs"}]
            }),
        );
        let m = models(&f);
        let e = m
            .extract(&SectionInput {
                path: "docs/a.md",
                heading: "Freshness",
                text: "Acme Ltd employs Jonas.",
            })
            .unwrap();
        assert_eq!(e.entities.len(), 2, "{e:?}");
        assert_eq!(e.entities[0].name, "Acme Ltd");
        assert_eq!(
            e.entities[0].r#type, "concept",
            "unknown type maps to concept"
        );
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
        assert!(
            m.extract(&SectionInput {
                path: "a.md",
                heading: "H",
                text: "t"
            })
            .is_ok(),
            "one retry recovers"
        );
        f.fail_next_generate(2);
        let e = m
            .extract(&SectionInput {
                path: "a.md",
                heading: "H",
                text: "t",
            })
            .unwrap_err();
        assert!(matches!(e, crate::Error::ModelUnavailable(_)), "{e}");
        assert_eq!(
            f.calls()
                .iter()
                .filter(|p| p.as_str() == "/api/generate")
                .count(),
            4
        );
    }

    #[test]
    fn a_down_server_is_model_unavailable_and_tags_lists_models() {
        let f = FakeOllama::spawn(8);
        let m = models(&f);
        assert_eq!(
            m.tags().unwrap(),
            vec![
                "qwen2.5:7b-instruct".to_string(),
                "nomic-embed-text".to_string()
            ]
        );
        f.set_down(true);
        let e = m.tags().unwrap_err();
        assert!(matches!(e, crate::Error::ModelUnavailable(_)), "{e}");
        let e = m.embed(&["x".into()]).unwrap_err();
        assert!(matches!(e, crate::Error::ModelUnavailable(_)));
        let e = m
            .extract(&SectionInput {
                path: "a.md",
                heading: "H",
                text: "t",
            })
            .unwrap_err();
        assert!(matches!(e, crate::Error::ModelUnavailable(_)));
    }

    #[test]
    fn long_sections_split_at_paragraphs_and_env_overrides_the_url() {
        let para = "word ".repeat(700).trim().to_string(); // 3,499 chars
        let text = format!("{para}\n\n{para}\n\n{para}");
        let parts = split_parts(&text);
        assert_eq!(
            parts.len(),
            3,
            "{:?}",
            parts.iter().map(|p| p.len()).collect::<Vec<_>>()
        );
        assert!(parts.iter().all(|p| p.chars().count() <= MAX_PART_CHARS));
        std::env::set_var("SINGULARRAG_OLLAMA_URL", "http://10.0.0.1:1");
        assert_eq!(
            ModelsConfig::default().with_env().ollama,
            "http://10.0.0.1:1"
        );
        std::env::remove_var("SINGULARRAG_OLLAMA_URL");
    }

    #[test]
    fn a_corrupt_embedding_is_model_unavailable() {
        let f = FakeOllama::spawn(8);
        let m = models(&f);
        f.corrupt_next_embed(1);
        let e = m.embed(&["a".into(), "b".into()]).unwrap_err();
        match e {
            crate::Error::ModelUnavailable(msg) => {
                assert!(msg.contains("vector element is not a number"), "{msg}")
            }
            other => panic!("expected ModelUnavailable, got {other}"),
        }
        assert!(
            m.embed(&["a".into()]).is_ok(),
            "only the next response is corrupt"
        );
    }

    #[test]
    fn a_refused_port_is_model_unavailable_quickly() {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let m = Models::new(ModelsConfig {
            ollama: format!("http://127.0.0.1:{port}"),
            ..ModelsConfig::default()
        });
        let t = std::time::Instant::now();
        let e = m.embed(&["x".into()]).unwrap_err();
        assert!(matches!(e, crate::Error::ModelUnavailable(_)), "{e}");
        assert!(
            t.elapsed() < std::time::Duration::from_secs(5),
            "{:?}",
            t.elapsed()
        );
    }

    #[test]
    fn the_type_list_has_process_and_role_and_the_prompt_defines_them() {
        assert!(ENTITY_TYPES.contains(&"process") && ENTITY_TYPES.contains(&"role"));
        assert!(EXTRACT_PROMPT.contains("process (a named sequence of steps people follow)"));
        assert!(EXTRACT_PROMPT.contains("role (a job or position that acts in a process)"));
        assert!(STEPS_PROMPT.starts_with(STEPS_PROMPT_FIRST_LINE));
        assert!(EXTRACT_PROMPT.contains("If the section describes a process, include one process entity named as the section names it"));
    }

    #[test]
    fn normalise_steps_cleans_truncates_and_drops_empty_steps() {
        let mut steps: Vec<ExtractedStep> = (0..40)
            .map(|i| ExtractedStep {
                text: format!("step {i}"),
                role: " Manager\u{0} ".into(),
                systems: vec!["Okta".into(), "".into()],
            })
            .collect();
        steps.push(ExtractedStep {
            text: "   ".into(),
            role: "".into(),
            systems: vec![],
        });
        let a = normalise_steps(StepsAnswer {
            process: " Expense process ".into(),
            steps,
        });
        assert_eq!(a.process, "Expense process");
        assert_eq!(a.steps.len(), MAX_STEPS);
        assert_eq!(a.steps[0].role, "Manager");
        assert_eq!(a.steps[0].systems, vec!["Okta".to_string()]);
        let long = normalise_steps(StepsAnswer {
            process: "p".into(),
            steps: vec![ExtractedStep {
                text: "x".repeat(400),
                role: "r".repeat(100),
                systems: vec![],
            }],
        });
        assert_eq!(long.steps[0].text.chars().count(), STEP_TEXT_MAX);
        assert_eq!(long.steps[0].role.chars().count(), ROLE_MAX);
    }

    #[test]
    fn parse_steps_accepts_fences_and_missing_optional_fields() {
        let a = parse_steps(
            "```json\n{\"process\": \"Onboarding\", \"steps\": [{\"text\": \"Activate Okta\"}]}\n```",
        )
        .unwrap();
        assert_eq!(a.process, "Onboarding");
        assert_eq!(a.steps[0].role, "");
        assert!(a.steps[0].systems.is_empty());
        assert!(parse_steps("not json").is_none());
    }

    #[test]
    fn steps_asks_once_per_part_and_retries_strictly_on_malformed_json() {
        let fake = FakeOllama::spawn(8);
        fake.set_steps(
            "Expense",
            serde_json::json!({"process": "Expense process", "steps": [{"text": "Submit in Expensify", "role": "employee", "systems": ["Expensify"]}]}),
        );
        let m = Models::new(ModelsConfig {
            ollama: fake.url(),
            ..ModelsConfig::default()
        });
        let input = SectionInput {
            path: "docs/handbook.md",
            heading: "Expense process",
            text: "Submit each expense in Expensify.",
        };
        let a = m.steps(&input).unwrap();
        assert_eq!(a.steps.len(), 1);
        assert_eq!(
            fake.calls()
                .iter()
                .filter(|c| *c == "/api/generate:steps")
                .count(),
            1
        );
        assert_eq!(
            fake.calls()
                .iter()
                .filter(|c| *c == "/api/generate")
                .count(),
            0
        );
        fake.fail_next_generate(1);
        let a = m.steps(&input).unwrap();
        assert_eq!(a.steps.len(), 1, "one strict retry recovers");
        assert_eq!(
            fake.calls()
                .iter()
                .filter(|c| *c == "/api/generate:steps")
                .count(),
            3
        );
        fake.fail_next_generate(2);
        assert!(matches!(
            m.steps(&input),
            Err(crate::Error::ModelUnavailable(_))
        ));
    }

    #[test]
    fn a_dropped_connection_is_retried_only_when_it_is_a_connect_error() {
        let f = FakeOllama::spawn(8);
        let m = models(&f);
        f.drop_next_n_connections(1);
        let before = f.accepted();
        let r = m.embed(&["x".into()]);
        let accepted = f.accepted() - before;
        match r {
            Ok(_) => assert_eq!(
                accepted, 2,
                "success means the drop was a connect error and was retried"
            ),
            Err(e) => {
                assert!(matches!(e, crate::Error::ModelUnavailable(_)), "{e}");
                assert_eq!(
                    accepted, 1,
                    "failure means no retry: the drop was not a connect error"
                );
            }
        }
    }
}
