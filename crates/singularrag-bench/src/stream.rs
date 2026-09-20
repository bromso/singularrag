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
    /// Tool names denied by permission on the result line, deduplicated, first-seen order.
    pub permission_denials: Vec<String>,
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
        match (
            v.get("type").and_then(Value::as_str),
            v.get("subtype").and_then(Value::as_str),
        ) {
            (Some("system"), Some("init")) => {
                p.model = v.get("model").and_then(Value::as_str).map(str::to_string);
                p.claude_version = v
                    .get("claude_code_version")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                if let Some(servers) = v.get("mcp_servers").and_then(Value::as_array) {
                    p.mcp_servers = servers
                        .iter()
                        .map(|s| McpServer {
                            name: s
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string(),
                            status: s
                                .get("status")
                                .and_then(Value::as_str)
                                .unwrap_or("unknown")
                                .to_string(),
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
                if let Some(denials) = v.get("permission_denials").and_then(Value::as_array) {
                    for d in denials {
                        if let Some(name) = d.get("tool_name").and_then(Value::as_str) {
                            if !p.permission_denials.iter().any(|n| n == name) {
                                p.permission_denials.push(name.to_string());
                            }
                        }
                    }
                }
                p.result = Some(ResultLine {
                    subtype: subtype.unwrap_or("unknown").to_string(),
                    is_error: v.get("is_error").and_then(Value::as_bool).unwrap_or(false),
                    num_turns: u64_of(&v, "num_turns"),
                    duration_ms: u64_of(&v, "duration_ms"),
                    total_cost_usd: v
                        .get("total_cost_usd")
                        .and_then(Value::as_f64)
                        .unwrap_or(0.0),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/").to_string() + name,
        )
        .unwrap()
    }

    #[test]
    fn success_stream_yields_model_servers_tool_counts_and_result() {
        let p = parse_stream(&fixture("success.jsonl"));
        assert!(p.permission_denials.is_empty());
        assert_eq!(p.model.as_deref(), Some("claude-fable-5-1"));
        assert_eq!(p.claude_version.as_deref(), Some("2.1.261"));
        assert_eq!(p.mcp_servers.len(), 1);
        assert_eq!(p.mcp_servers[0].name, "singularrag");
        assert_eq!(p.mcp_servers[0].status, "connected");
        assert_eq!(p.tool_calls.get("Grep"), Some(&2));
        assert_eq!(p.tool_calls.get("mcp__singularrag__repo_map"), Some(&1));
        assert!(
            !p.tool_calls.contains_key("StructuredOutput"),
            "the answer call is not a tool call"
        );
        let r = p.result.expect("result line");
        assert_eq!(r.subtype, "success");
        assert!(!r.is_error);
        assert_eq!(r.num_turns, 3);
        assert_eq!(r.duration_ms, 11419);
        assert!((r.total_cost_usd - 0.1928845).abs() < 1e-9);
        assert_eq!(
            r.tokens,
            Tokens {
                input: 34,
                output: 776,
                cache_creation: 7606,
                cache_read: 6498
            }
        );
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
        assert!(p.permission_denials.is_empty());
    }

    #[test]
    fn no_answer_stream_is_success_without_structured_output() {
        let p = parse_stream(&fixture("no_answer.jsonl"));
        let r = p.result.expect("result");
        assert_eq!(r.subtype, "success");
        assert!(r.structured_output.is_none());
        assert!(p.tool_calls.is_empty());
        assert!(p.permission_denials.is_empty());
    }

    #[test]
    fn mcp_failed_stream_reports_server_status() {
        let p = parse_stream(&fixture("mcp_failed.jsonl"));
        assert_eq!(p.mcp_servers[0].status, "failed");
    }

    #[test]
    fn garbage_lines_are_counted_not_fatal() {
        let text = format!(
            "not json\n{}\n\n",
            fixture("no_answer.jsonl").lines().last().unwrap()
        );
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

    #[test]
    fn recorded_smoke_stream_parses_to_a_result_with_an_answer() {
        let p = parse_stream(&fixture("smoke.jsonl"));
        assert_eq!(p.bad_lines, 0);
        let r = p.result.expect("result");
        assert_eq!(r.subtype, "success");
        assert!(r.structured_output.is_some());
        assert!(p.model.is_some());
        assert_eq!(p.mcp_servers[0].status, "connected");
        assert_eq!(p.tool_calls.get("mcp__singularrag__repo_map"), Some(&1));
        assert_eq!(p.permission_denials.len(), 2);
    }

    #[test]
    fn denied_stream_lists_each_denied_tool_once() {
        let p = parse_stream(&fixture("denied.jsonl"));
        assert_eq!(
            p.permission_denials,
            vec!["mcp__singularrag__repo_map".to_string()]
        );
        assert_eq!(p.tool_calls.get("mcp__singularrag__repo_map"), Some(&1));
    }
}
