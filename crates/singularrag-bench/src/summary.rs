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
    if n == 0 {
        0.0
    } else {
        xs.sum::<f64>() / n as f64
    }
}

fn median(mut xs: Vec<f64>) -> f64 {
    if xs.is_empty() {
        return 0.0;
    }
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = xs.len();
    if n % 2 == 1 {
        xs[n / 2]
    } else {
        (xs[n / 2 - 1] + xs[n / 2]) / 2.0
    }
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
    if base == 0.0 {
        0.0
    } else {
        (cond - base) / base * 100.0
    }
}

pub fn verdict(base: &ConditionStats, cond: &ConditionStats) -> Verdict {
    let floor = base.mean_recall - RECALL_TOLERANCE;
    let correctness = if cond.mean_recall + EPS >= floor {
        Ok(())
    } else {
        Err(format!(
            "recall {:.2} vs {:.2} (need >= {:.2})",
            cond.mean_recall, base.mean_recall, floor
        ))
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
    Verdict {
        correctness,
        efficiency,
    }
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

pub fn render(
    meta: &RunMeta,
    baseline: &str,
    conditions: &[(String, Vec<Record>)],
    question_ids: &[String],
) -> String {
    let mut out = String::new();
    let mut models: Vec<String> = conditions
        .iter()
        .flat_map(|(_, rs)| rs.iter().filter_map(|r| r.model.clone()))
        .collect();
    models.sort();
    models.dedup();
    let _ = writeln!(out, "# Tier-two run {}\n", meta.run_dir);
    let _ = writeln!(
        out,
        "commit {} · claude {} · singularrag {} · models: {}",
        meta.commit,
        meta.claude_version,
        meta.singularrag_version.as_deref().unwrap_or("n/a"),
        if models.is_empty() {
            "none".to_string()
        } else {
            models.join(", ")
        }
    );
    let _ = writeln!(
        out,
        "tokens = input + output + cache creation + cache read\n"
    );
    for (name, reason) in &meta.aborted {
        let _ = writeln!(out, "aborted: {name} ({reason})");
    }
    if !meta.aborted.is_empty() {
        out.push('\n');
    }

    let all: Vec<ConditionStats> = conditions.iter().map(|(n, rs)| stats(n, rs)).collect();
    let _ = writeln!(out, "| condition | sessions | failed | mean recall | mean tokens | median tokens | mean tool calls | mean wall s | total cost |");
    let _ = writeln!(out, "|---|---:|---:|---:|---:|---:|---:|---:|---:|");
    for s in &all {
        let _ = writeln!(
            out,
            "| {} | {} | {} | {:.2} | {:.0} | {:.0} | {:.1} | {:.1} | ${:.2} |",
            s.name,
            s.sessions,
            s.failed,
            s.mean_recall,
            s.mean_tokens,
            s.median_tokens,
            s.mean_tool_calls,
            s.mean_wall_s,
            s.total_cost
        );
    }
    out.push('\n');

    let _ = writeln!(
        out,
        "| question | {} |",
        conditions
            .iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<_>>()
            .join(" | ")
    );
    let _ = writeln!(out, "|---|{}", "---:|".repeat(conditions.len()));
    for q in question_ids {
        let cells: Vec<String> = conditions
            .iter()
            .map(|(_, rs)| {
                let qs: Vec<&Record> = rs.iter().filter(|r| &r.question == q).collect();
                if qs.is_empty() {
                    "–".to_string()
                } else {
                    format!("{:.2}", mean(qs.iter().map(|r| r.recall), qs.len()))
                }
            })
            .collect();
        let _ = writeln!(out, "| {} | {} |", q, cells.join(" | "));
    }
    out.push('\n');

    match all.iter().find(|s| s.name == baseline) {
        None => {
            let _ = writeln!(
                out,
                "No verdict: baseline condition {baseline} has no records."
            );
        }
        Some(base) => {
            for s in all.iter().filter(|s| s.name != baseline) {
                let v = verdict(base, s);
                if v.earns() {
                    let _ = writeln!(out, "**{} earns its place** against {}.", s.name, baseline);
                } else {
                    let mut parts = Vec::new();
                    if let Err(e) = &v.correctness {
                        parts.push(format!("correctness: {e}"));
                    }
                    if let Err(e) = &v.efficiency {
                        parts.push(format!("efficiency: {e}"));
                    }
                    let _ = writeln!(
                        out,
                        "{} does not earn its place: {}",
                        s.name,
                        parts.join("; ")
                    );
                }
            }
        }
    }
    out
}

/// Read a run directory back: `run.toml` plus every `<condition>/<qid>-<n>.json`.
#[allow(clippy::type_complexity)]
pub fn load_run(
    run_dir: &Path,
) -> Result<(RunMeta, String, Vec<(String, Vec<Record>)>, Vec<String>)> {
    let text = std::fs::read_to_string(run_dir.join("run.toml"))
        .with_context(|| format!("reading {}", run_dir.join("run.toml").display()))?;
    let rf: RunFile = toml::from_str(&text).context("parsing run.toml")?;
    let mut conditions = Vec::new();
    for name in &rf.run.conditions {
        let dir = run_dir.join(name);
        let mut records = Vec::new();
        if dir.is_dir() {
            // Record files are named `<qid>-<repeat>.json`; a condition directory can also hold
            // `mcp.json` (the materialised MCP config), which is not a record.
            let mut entries: Vec<_> = std::fs::read_dir(&dir)?
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.extension().is_some_and(|x| x == "json")
                        && p.file_stem()
                            .and_then(|s| s.to_str())
                            .and_then(|s| s.rsplit_once('-'))
                            .is_some_and(|(_, n)| n.parse::<u32>().is_ok())
                })
                .collect();
            entries.sort();
            for p in entries {
                let r: Record = serde_json::from_str(&std::fs::read_to_string(&p)?)
                    .with_context(|| format!("parsing {}", p.display()))?;
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
    Ok((
        meta,
        rf.run.baseline.clone(),
        conditions,
        rf.run.questions.clone(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::score::Record;
    use crate::stream::Tokens;
    use std::collections::BTreeMap;

    fn rec(q: &str, cond: &str, recall: f64, tokens: u64, calls: u64, failed: bool) -> Record {
        let mut tool_calls = BTreeMap::new();
        if calls > 0 {
            tool_calls.insert("Grep".to_string(), calls);
        }
        Record {
            question: q.into(),
            condition: cond.into(),
            repeat: 1,
            model: Some("m".into()),
            recall,
            precision: 0.5,
            hit: vec![],
            miss: vec![],
            answer: vec![],
            tool_calls,
            tokens: Tokens {
                input: tokens,
                output: 0,
                cache_creation: 0,
                cache_read: 0,
            },
            cost_usd: 0.1,
            turns: 3,
            duration_ms: 2000,
            failed,
            reason: None,
            denied: vec![],
        }
    }

    #[test]
    fn stats_are_means_medians_and_counts() {
        let rs = vec![
            rec("L1", "alone", 1.0, 100, 4, false),
            rec("L1", "alone", 0.5, 300, 2, false),
            rec("L2", "alone", 0.0, 800, 0, true),
        ];
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
        let rs = vec![
            rec("L1", "a", 1.0, 100, 0, false),
            rec("L1", "a", 1.0, 200, 0, false),
            rec("L1", "a", 1.0, 1000, 0, false),
            rec("L1", "a", 1.0, 5000, 0, false),
        ];
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
        ConditionStats {
            name: "c".into(),
            sessions: 1,
            failed: 0,
            mean_recall: recall,
            mean_tokens: tokens,
            median_tokens: tokens,
            mean_tool_calls: calls,
            mean_wall_s: 1.0,
            total_cost: 0.1,
        }
    }

    #[test]
    fn verdict_boundaries_pass_and_just_beyond_fail() {
        let base = st(0.60, 1000.0, 10.0);
        assert!(
            verdict(&base, &st(0.58, 750.0, 10.0)).earns(),
            "recall at -0.02 and tokens at -25% pass"
        );
        assert!(
            verdict(&base, &st(0.58, 1000.0, 7.5)).earns(),
            "tool calls at -25% pass"
        );
        let v = verdict(&base, &st(0.579, 750.0, 10.0));
        assert!(v.correctness.is_err() && v.efficiency.is_ok() && !v.earns());
        let v = verdict(&base, &st(0.60, 751.0, 7.6));
        assert!(v.correctness.is_ok() && v.efficiency.is_err() && !v.earns());
        assert_eq!(
            v.efficiency.unwrap_err(),
            "tokens 751 vs 1000 (-24.9%), tool calls 7.6 vs 10.0 (-24.0%); need -25% on either"
        );
    }

    #[test]
    fn render_has_header_tables_and_verdicts() {
        let meta = RunMeta {
            run_dir: "eval/runs/20260920T000000Z-run".into(),
            commit: "098e119".into(),
            claude_version: "2.1.261".into(),
            singularrag_version: Some("0.1.0".into()),
            aborted: BTreeMap::new(),
        };
        let conds = vec![
            (
                "alone".to_string(),
                vec![
                    rec("L1", "alone", 0.5, 1000, 10, false),
                    rec("L2", "alone", 0.5, 1000, 10, false),
                ],
            ),
            (
                "singularrag".to_string(),
                vec![
                    rec("L1", "singularrag", 1.0, 500, 4, false),
                    rec("L2", "singularrag", 0.0, 500, 4, true),
                ],
            ),
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
        aborted.insert(
            "serena".to_string(),
            "mcp server serena status failed".to_string(),
        );
        let meta = RunMeta {
            run_dir: "r".into(),
            commit: "c".into(),
            claude_version: "v".into(),
            singularrag_version: None,
            aborted,
        };
        let conds = vec![
            (
                "alone".to_string(),
                vec![rec("L1", "alone", 0.5, 1000, 10, false)],
            ),
            (
                "serena".to_string(),
                vec![rec("L1", "serena", 0.2, 900, 9, false)],
            ),
        ];
        let md = render(&meta, "alone", &conds, &["L1".into()]);
        assert!(md.contains("serena does not earn its place: correctness: recall 0.20 vs 0.50 (need >= 0.48); efficiency: tokens 900 vs 1000 (-10.0%), tool calls 9.0 vs 10.0 (-10.0%); need -25% on either"), "{md}");
        assert!(
            md.contains("aborted: serena (mcp server serena status failed)"),
            "{md}"
        );
    }
}
