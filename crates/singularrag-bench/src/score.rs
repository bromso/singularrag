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
    /// Tools denied by permission, the singularrag hook's Read denials excluded.
    #[serde(default)]
    pub denied: Vec<String>,
    /// Reads refused by the singularrag hook: a `Read` denial whose tool result carries
    /// `singularrag:`. Counted, never an abort.
    #[serde(default)]
    pub hook_denials: u64,
}

impl Record {
    pub fn tool_call_total(&self) -> u64 {
        self.tool_calls.values().sum()
    }
}

/// Trim, strip a leading `./`, drop entries without `::`, reduce a qualified name to the
/// declared one (`Router.match`, `Node/search` and `#dispatch` all score as the gold's
/// `match`, `search`, `dispatch`), drop repeats (a repeated symbol would count twice
/// against precision), keep the first `answer_max`.
pub fn normalise(symbols: &[String], answer_max: usize) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    symbols
        .iter()
        .map(|s| s.trim())
        .map(|s| s.strip_prefix("./").unwrap_or(s))
        .filter(|s| s.contains("::"))
        .map(declared_name)
        .filter(|s| seen.insert(s.clone()))
        .take(answer_max)
        .collect()
}

fn declared_name(sym: &str) -> String {
    let (path, name) = sym.split_once("::").expect("filtered above");
    let last = name.rsplit(['.', '/']).next().unwrap_or(name);
    format!("{path}::{}", last.trim_start_matches('#'))
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

pub fn score(
    q: &Question,
    condition: &str,
    repeat: u32,
    parsed: &Parsed,
    answer_max: usize,
    spawn_error: Option<&str>,
) -> Record {
    let result = parsed.result.as_ref();
    let reason: Option<String> = if let Some(e) = spawn_error {
        Some(e.to_string())
    } else if let Some(r) = result {
        if r.is_error {
            Some("result reported an error".to_string())
        } else if r.subtype != "success" {
            Some(format!("result subtype {}", r.subtype))
        } else if r.structured_output.is_none() {
            Some("no structured output".to_string())
        } else if answer_of(parsed, answer_max).is_none() {
            Some("malformed structured output".to_string())
        } else {
            None
        }
    } else {
        Some("no result line".to_string())
    };
    let is_hook = |tool: &str, id: &str| {
        tool == "Read"
            && parsed
                .tool_results
                .get(id)
                .is_some_and(|t| t.contains("singularrag:"))
    };
    let hook_denials = parsed
        .permission_denial_ids
        .iter()
        .filter(|(t, id)| is_hook(t, id))
        .count() as u64;
    let mut denied: Vec<String> = Vec::new();
    for (t, id) in &parsed.permission_denial_ids {
        if !is_hook(t, id) && !denied.contains(t) {
            denied.push(t.clone());
        }
    }
    let answer = if reason.is_none() {
        answer_of(parsed, answer_max).unwrap_or_default()
    } else {
        Vec::new()
    };
    let (hit, miss): (Vec<String>, Vec<String>) =
        q.gold.iter().cloned().partition(|g| answer.contains(g));
    let recall = if reason.is_some() {
        0.0
    } else if q.gold.is_empty() {
        1.0
    } else {
        hit.len() as f64 / q.gold.len() as f64
    };
    let precision = if answer.is_empty() {
        0.0
    } else {
        hit.len() as f64 / answer.len() as f64
    };
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
        denied,
        hook_denials,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::parse_stream;
    use singularrag_core::eval::Question;

    fn q(gold: &[&str]) -> Question {
        Question {
            id: "L1".into(),
            category: "locate".into(),
            query: "where is request routing decided".into(),
            gold: gold.iter().map(|s| s.to_string()).collect(),
        }
    }
    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!(
            "{}/tests/fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }
    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn normalise_trims_strips_dot_slash_drops_non_symbols_and_caps() {
        let out = normalise(
            &s(&[
                " src/a.ts::A ",
                "./src/b.ts::B",
                "src/c.ts",
                "",
                "src/d.ts::D",
            ]),
            2,
        );
        assert_eq!(out, s(&["src/a.ts::A", "src/b.ts::B"]));
    }

    #[test]
    fn normalise_reduces_qualified_names_to_the_declared_name() {
        let out = normalise(
            &s(&[
                "src/router.ts::Router.match",
                "src/router/trie-router/node.ts::Node/search",
                "src/hono-base.ts::#dispatch",
                "src/router.ts::match",
            ]),
            10,
        );
        assert_eq!(
            out,
            s(&[
                "src/router.ts::match",
                "src/router/trie-router/node.ts::search",
                "src/hono-base.ts::dispatch",
            ])
        );
    }

    #[test]
    fn normalise_dedupes_before_the_cap() {
        let out = normalise(
            &s(&["src/a.ts::A", "./src/a.ts::A", "src/b.ts::B", "src/c.ts::C"]),
            3,
        );
        assert_eq!(out, s(&["src/a.ts::A", "src/b.ts::B", "src/c.ts::C"]));
    }

    #[test]
    fn success_fixture_scores_recall_and_precision() {
        let p = parse_stream(&fixture("success.jsonl"));
        let r = score(
            &q(&[
                "src/router.ts::Router",
                "src/router.ts::match",
                "src/hono-base.ts::Hono",
                "src/router/reg-exp-router/router.ts::RegExpRouter",
            ]),
            "singularrag",
            1,
            &p,
            15,
            None,
        );
        assert!(!r.failed);
        assert_eq!(
            r.answer,
            s(&[
                "src/router.ts::Router",
                "src/router.ts::match",
                "src/hono-base.ts::Hono"
            ])
        );
        assert_eq!(r.hit.len(), 3);
        assert_eq!(
            r.miss,
            s(&["src/router/reg-exp-router/router.ts::RegExpRouter"])
        );
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
        assert_eq!(r.reason.as_deref(), Some("result reported an error"));
        assert_eq!(r.recall, 0.0);
        assert_eq!(r.miss, s(&["src/router.ts::Router"]));
        assert!((r.cost_usd - 0.31).abs() < 1e-9);
        assert_eq!(r.tool_call_total(), 2);
    }

    #[test]
    fn subtype_only_failure_without_is_error_keeps_the_subtype_reason() {
        let stream = r#"{"type":"result","subtype":"error_during_execution","is_error":false,"duration_ms":1,"num_turns":1,"total_cost_usd":0.0,"usage":{},"structured_output":null}"#;
        let p = parse_stream(stream);
        let r = score(&q(&["src/router.ts::Router"]), "alone", 1, &p, 15, None);
        assert!(r.failed);
        assert_eq!(
            r.reason.as_deref(),
            Some("result subtype error_during_execution")
        );
    }

    #[test]
    fn malformed_structured_output_is_failed() {
        let stream = r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":1,"num_turns":1,"total_cost_usd":0.0,"usage":{},"structured_output":{"symbols":"x"}}"#;
        let p = parse_stream(stream);
        let r = score(&q(&["src/router.ts::Router"]), "alone", 1, &p, 15, None);
        assert!(r.failed);
        assert_eq!(r.reason.as_deref(), Some("malformed structured output"));
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
        let r = score(
            &q(&["src/router.ts::Router"]),
            "alone",
            2,
            &p,
            15,
            Some("exit status 1"),
        );
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
    fn denied_tools_are_recorded_but_not_a_scoring_failure() {
        let p = parse_stream(&fixture("denied.jsonl"));
        let r = score(
            &q(&["src/router.ts::Router"]),
            "singularrag",
            1,
            &p,
            15,
            None,
        );
        assert_eq!(r.denied, s(&["mcp__singularrag__repo_map"]));
        assert!(!r.failed);
    }

    #[test]
    fn record_round_trips_through_json() {
        let p = parse_stream(&fixture("success.jsonl"));
        let r = score(
            &q(&["src/router.ts::Router"]),
            "singularrag",
            1,
            &p,
            15,
            None,
        );
        let text = serde_json::to_string(&r).unwrap();
        let back: Record = serde_json::from_str(&text).unwrap();
        assert_eq!(back.hit, r.hit);
        assert_eq!(back.tokens, r.tokens);
    }

    const HOOK_STREAM: &str = concat!(
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"tu1","name":"Read","input":{"file_path":"/tmp/hono/src/router.ts"}}]},"session_id":"5"}"#,
        "\n",
        r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu1","content":"singularrag: this repository has a map. Call repo_map with your task first.","is_error":true}]},"session_id":"5"}"#,
        "\n",
        r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":4000,"num_turns":2,"structured_output":{"symbols":["src/router.ts::Router"]},"session_id":"5","total_cost_usd":0.04,"usage":{"input_tokens":5,"cache_creation_input_tokens":4000,"cache_read_input_tokens":0,"output_tokens":30},"permission_denials":[{"tool_name":"Read","tool_use_id":"tu1","tool_input":{"file_path":"/tmp/hono/src/router.ts"}}]}"#,
        "\n"
    );

    #[test]
    fn a_read_denied_by_the_singularrag_hook_is_counted_not_denied() {
        let p = parse_stream(HOOK_STREAM);
        let r = score(
            &q(&["src/router.ts::Router"]),
            "singularrag+hook",
            1,
            &p,
            15,
            None,
        );
        assert_eq!(r.hook_denials, 1);
        assert!(r.denied.is_empty(), "{:?}", r.denied);
        assert!(!r.failed);
        assert_eq!(r.recall, 1.0);
    }

    #[test]
    fn a_denial_of_another_tool_is_not_a_hook_denial() {
        let p = parse_stream(&fixture("denied.jsonl"));
        let r = score(
            &q(&["src/router.ts::Router"]),
            "singularrag",
            1,
            &p,
            15,
            None,
        );
        assert_eq!(r.hook_denials, 0);
        assert_eq!(r.denied, s(&["mcp__singularrag__repo_map"]));
    }
}
