//! Aggregate records into per-condition stats, the §12 verdict, and `summary.md` (spec §7).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::RunConfig;
use crate::score::Record;

/// Spec §12 (amended 2026-09-21): the condition's mean tokens may exceed the baseline's
/// by at most this share; the other two tests are paired intervals.
pub const TOKEN_TOLERANCE: f64 = 0.10;
/// Two-sided 95% normal interval. The design is 12 questions × 3 repeats = 36 pairs,
/// enough for the normal approximation; a handful of pairs gives a wide interval and
/// an honest "no" rather than a false "yes".
const Z: f64 = 1.96;
pub const MIN_PAIRS: usize = 2;
const EPS: f64 = 1e-9;

#[derive(Debug, Clone)]
pub struct ConditionStats {
    pub name: String,
    pub sessions: usize,
    pub failed: usize,
    pub hook_denials: u64,
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
        hook_denials: records.iter().map(|r| r.hook_denials).sum(),
        mean_recall: mean(records.iter().map(|r| r.recall), n),
        mean_tokens: mean(records.iter().map(|r| r.tokens.total() as f64), n),
        median_tokens: median(records.iter().map(|r| r.tokens.total() as f64).collect()),
        mean_tool_calls: mean(records.iter().map(|r| r.tool_call_total() as f64), n),
        mean_wall_s: mean(records.iter().map(|r| r.duration_ms as f64 / 1000.0), n),
        total_cost: records.iter().map(|r| r.cost_usd).sum(),
    }
}

/// A paired difference (condition minus baseline) with its 95% interval.
#[derive(Debug, Clone, PartialEq)]
pub struct Paired {
    pub n: usize,
    pub mean: f64,
    pub lo: f64,
    pub hi: f64,
}

/// `pairs` are `(baseline, condition)` values for the same question and repeat.
pub fn paired(pairs: &[(f64, f64)]) -> Option<Paired> {
    let n = pairs.len();
    if n < MIN_PAIRS {
        return None;
    }
    let d: Vec<f64> = pairs.iter().map(|(b, c)| c - b).collect();
    let mean = d.iter().sum::<f64>() / n as f64;
    let var = d.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1) as f64;
    let se = (var / n as f64).sqrt();
    Some(Paired {
        n,
        mean,
        lo: mean - Z * se,
        hi: mean + Z * se,
    })
}

/// Baseline and condition records matched on question and repeat; a session missing on
/// either side drops the pair. A failed session keeps its recorded recall (0) and cost.
fn pairs(base: &[Record], cond: &[Record], f: impl Fn(&Record) -> f64) -> Vec<(f64, f64)> {
    base.iter()
        .filter_map(|b| {
            cond.iter()
                .find(|c| c.question == b.question && c.repeat == b.repeat)
                .map(|c| (f(b), f(c)))
        })
        .collect()
}

#[derive(Debug, Clone)]
pub struct Verdict {
    pub correctness: std::result::Result<(), String>,
    pub efficiency: std::result::Result<(), String>,
    /// The three measurements, shown under the verdict line whichever way it went.
    pub lines: Vec<String>,
}

impl Verdict {
    pub fn earns(&self) -> bool {
        self.correctness.is_ok() && self.efficiency.is_ok()
    }
}

/// Spec §12: correctness when the paired recall interval lies above zero; efficiency
/// when the paired tool-call interval lies below zero and mean tokens exceed the
/// baseline's by at most `TOKEN_TOLERANCE`. `Err` names why no verdict is possible.
pub fn verdict(base: &[Record], cond: &[Record]) -> std::result::Result<Verdict, String> {
    let recall = paired(&pairs(base, cond, |r| r.recall));
    let calls = paired(&pairs(base, cond, |r| r.tool_call_total() as f64));
    let tokens = paired(&pairs(base, cond, |r| r.tokens.total() as f64));
    let (Some(recall), Some(calls), Some(tokens)) = (recall, calls, tokens) else {
        return Err(format!("fewer than {MIN_PAIRS} paired sessions"));
    };
    let base_tokens = mean(base.iter().map(|r| r.tokens.total() as f64), base.len());
    let token_pct = |x: f64| {
        if base_tokens == 0.0 {
            0.0
        } else {
            x / base_tokens * 100.0
        }
    };
    let lines = vec![
        format!(
            "recall {:+.2} [{:+.2}, {:+.2}] over {} pairs",
            recall.mean, recall.lo, recall.hi, recall.n
        ),
        format!(
            "tool calls {:+.1} [{:+.1}, {:+.1}]",
            calls.mean, calls.lo, calls.hi
        ),
        format!(
            "tokens {:+.0} ({:+.1}%) [{:+.1}%, {:+.1}%]",
            tokens.mean,
            token_pct(tokens.mean),
            token_pct(tokens.lo),
            token_pct(tokens.hi)
        ),
    ];
    let correctness = if recall.lo > EPS {
        Ok(())
    } else {
        Err(format!(
            "recall {:+.2} [{:+.2}, {:+.2}]; the interval must lie above 0",
            recall.mean, recall.lo, recall.hi
        ))
    };
    let mut why = Vec::new();
    if calls.hi >= -EPS {
        why.push(format!(
            "tool calls {:+.1} [{:+.1}, {:+.1}]; the interval must lie below 0",
            calls.mean, calls.lo, calls.hi
        ));
    }
    if tokens.mean > TOKEN_TOLERANCE * base_tokens + EPS {
        why.push(format!(
            "tokens {:+.1}%; at most {:+.0}%",
            token_pct(tokens.mean),
            TOKEN_TOLERANCE * 100.0
        ));
    }
    let efficiency = if why.is_empty() {
        Ok(())
    } else {
        Err(why.join(", "))
    };
    Ok(Verdict {
        correctness,
        efficiency,
        lines,
    })
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
    let singularrag_field = meta
        .singularrag_version
        .as_deref()
        .map(|v| v.strip_prefix("singularrag ").unwrap_or(v))
        .unwrap_or("n/a");
    let _ = writeln!(
        out,
        "commit {} · claude {} · singularrag {} · models: {}",
        meta.commit,
        meta.claude_version,
        singularrag_field,
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
    let _ = writeln!(out, "| condition | sessions | failed | hook denials | mean recall | mean tokens | median tokens | mean tool calls | mean wall s | total cost |");
    let _ = writeln!(out, "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|");
    for s in &all {
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {:.2} | {:.0} | {:.0} | {:.1} | {:.1} | ${:.2} |",
            s.name,
            s.sessions,
            s.failed,
            s.hook_denials,
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

    if let Some(reason) = meta.aborted.get(baseline) {
        let _ = writeln!(
            out,
            "No verdict: baseline condition {baseline} was aborted ({reason})."
        );
    } else {
        match all.iter().find(|s| s.name == baseline) {
            None => {
                let _ = writeln!(
                    out,
                    "No verdict: baseline condition {baseline} has no records."
                );
            }
            Some(_) => {
                let base_records: &[Record] = conditions
                    .iter()
                    .find(|(n, _)| n == baseline)
                    .map(|(_, rs)| rs.as_slice())
                    .unwrap_or(&[]);
                for s in all.iter().filter(|s| s.name != baseline) {
                    if let Some(reason) = meta.aborted.get(&s.name) {
                        let _ = writeln!(out, "{}: no verdict (aborted: {reason})", s.name);
                        continue;
                    }
                    if s.sessions == 0 {
                        let _ = writeln!(out, "{}: no verdict (no records)", s.name);
                        continue;
                    }
                    let records: &[Record] = conditions
                        .iter()
                        .find(|(n, _)| *n == s.name)
                        .map(|(_, rs)| rs.as_slice())
                        .unwrap_or(&[]);
                    match verdict(base_records, records) {
                        Err(e) => {
                            let _ = writeln!(out, "{}: no verdict ({e})", s.name);
                        }
                        Ok(v) => {
                            if v.earns() {
                                let _ = writeln!(
                                    out,
                                    "**{} earns its place** against {}.",
                                    s.name, baseline
                                );
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
                            for l in &v.lines {
                                let _ = writeln!(out, "- {l}");
                            }
                        }
                    }
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
            hook_denials: 0,
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

    fn recr(q: &str, repeat: u32, cond: &str, recall: f64, tokens: u64, calls: u64) -> Record {
        let mut r = rec(q, cond, recall, tokens, calls, false);
        r.repeat = repeat;
        r
    }

    /// Four pairs of the same question: baseline (0.5, 1000 tokens, 10 calls) against
    /// the given condition values, one per repeat.
    fn four(cond: &[(f64, u64, u64)]) -> (Vec<Record>, Vec<Record>) {
        let base = (1..=4)
            .map(|i| recr("L1", i, "alone", 0.5, 1000, 10))
            .collect();
        let cond = cond
            .iter()
            .enumerate()
            .map(|(i, (r, t, c))| recr("L1", i as u32 + 1, "x", *r, *t, *c))
            .collect();
        (base, cond)
    }

    #[test]
    fn paired_interval_matches_a_hand_computation() {
        // differences -2, -3, -1, -2: mean -2, sample variance 2/3, se 0.4082
        let p = paired(&[(10.0, 8.0), (10.0, 7.0), (10.0, 9.0), (10.0, 8.0)]).unwrap();
        assert_eq!(p.n, 4);
        assert!((p.mean + 2.0).abs() < 1e-9);
        assert!((p.lo + 2.8).abs() < 1e-3, "{p:?}");
        assert!((p.hi + 1.2).abs() < 1e-3, "{p:?}");
        assert!(paired(&[(1.0, 2.0)]).is_none(), "one pair has no interval");
        assert!(paired(&[]).is_none());
    }

    #[test]
    fn verdict_needs_recall_up_calls_down_and_tokens_near_parity() {
        // Constant improvements: zero variance, the intervals are points.
        let (b, c) = four(&[
            (0.7, 1050, 8),
            (0.7, 1050, 8),
            (0.7, 1050, 8),
            (0.7, 1050, 8),
        ]);
        let v = verdict(&b, &c).unwrap();
        assert!(v.earns(), "{v:?}");
        assert_eq!(v.lines[0], "recall +0.20 [+0.20, +0.20] over 4 pairs");
        assert_eq!(v.lines[1], "tool calls -2.0 [-2.0, -2.0]");
        assert_eq!(v.lines[2], "tokens +50 (+5.0%) [+5.0%, +5.0%]");

        // Recall up on average but not beyond noise: +0.5, +0.5, -0.4, +0.2.
        let (b, c) = four(&[
            (1.0, 1000, 8),
            (1.0, 1000, 8),
            (0.1, 1000, 8),
            (0.7, 1000, 8),
        ]);
        let v = verdict(&b, &c).unwrap();
        assert!(v.correctness.is_err() && v.efficiency.is_ok(), "{v:?}");
        assert!(v
            .correctness
            .unwrap_err()
            .ends_with("the interval must lie above 0"));

        // Tokens over the tolerance: +11%.
        let (b, c) = four(&[
            (0.7, 1110, 8),
            (0.7, 1110, 8),
            (0.7, 1110, 8),
            (0.7, 1110, 8),
        ]);
        let v = verdict(&b, &c).unwrap();
        assert_eq!(v.efficiency.unwrap_err(), "tokens +11.0%; at most +10%");

        // Tool calls down on average but not beyond noise: -6, -6, +4, -1.
        let (b, c) = four(&[
            (0.7, 1000, 4),
            (0.7, 1000, 4),
            (0.7, 1000, 14),
            (0.7, 1000, 9),
        ]);
        let v = verdict(&b, &c).unwrap();
        assert!(v
            .efficiency
            .unwrap_err()
            .ends_with("the interval must lie below 0"));

        // Both efficiency reasons are named.
        let (b, c) = four(&[
            (0.7, 1200, 4),
            (0.7, 1200, 4),
            (0.7, 1200, 14),
            (0.7, 1200, 9),
        ]);
        let e = verdict(&b, &c).unwrap().efficiency.unwrap_err();
        assert!(
            e.contains("tool calls") && e.contains("tokens +20.0%"),
            "{e}"
        );
    }

    #[test]
    fn verdict_needs_two_paired_sessions() {
        let b = vec![recr("L1", 1, "alone", 0.5, 1000, 10)];
        let c = vec![recr("L1", 1, "x", 1.0, 500, 4)];
        assert_eq!(verdict(&b, &c).unwrap_err(), "fewer than 2 paired sessions");
        // A repeat the condition never ran is not a pair.
        let b2 = vec![
            recr("L1", 1, "alone", 0.5, 1000, 10),
            recr("L1", 2, "alone", 0.5, 1000, 10),
        ];
        assert!(verdict(&b2, &c).is_err());
    }

    #[test]
    fn render_has_header_tables_and_verdicts() {
        let meta = RunMeta {
            run_dir: "eval/runs/20260920T000000Z-run".into(),
            commit: "098e119".into(),
            claude_version: "2.1.261".into(),
            singularrag_version: Some("singularrag 0.1.0".into()),
            aborted: BTreeMap::new(),
        };
        // Two questions × two repeats. The failed singularrag session scores 0 against
        // a baseline 0, a zero difference that narrows the recall interval but keeps it
        // above 0.
        let mut failed = rec("L2", "singularrag", 0.0, 500, 4, true);
        failed.repeat = 2;
        let conds = vec![
            (
                "alone".to_string(),
                vec![
                    recr("L1", 1, "alone", 0.5, 1000, 10),
                    recr("L1", 2, "alone", 0.5, 1000, 10),
                    recr("L2", 1, "alone", 0.5, 1000, 10),
                    recr("L2", 2, "alone", 0.0, 1000, 10),
                ],
            ),
            (
                "singularrag".to_string(),
                vec![
                    recr("L1", 1, "singularrag", 1.0, 500, 4),
                    recr("L1", 2, "singularrag", 1.0, 500, 4),
                    recr("L2", 1, "singularrag", 1.0, 500, 4),
                    failed,
                ],
            ),
        ];
        let md = render(&meta, "alone", &conds, &["L1".into(), "L2".into()]);
        assert!(md.contains("tokens = input + output + cache creation + cache read"));
        assert!(md.contains("| alone | 4 | 0 | 0 | 0.38 |"), "{md}");
        assert!(md.contains("| singularrag | 4 | 1 | 0 | 0.75 |"), "{md}");
        assert!(md.contains("| L1 | 0.50 | 1.00 |"), "{md}");
        assert!(md.contains("**singularrag earns its place**"), "{md}");
        assert!(
            md.contains("- recall +0.38 [+0.13, +0.62] over 4 pairs"),
            "{md}"
        );
        assert!(md.contains("- tool calls -6.0 [-6.0, -6.0]"), "{md}");
        assert!(md.contains("- tokens -500 (-50.0%)"), "{md}");
        assert!(md.contains("models: m"), "{md}");
        assert!(md.contains("· singularrag 0.1.0 ·"), "{md}");
        assert!(!md.contains("singularrag singularrag"), "{md}");
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
        assert!(
            md.contains("serena: no verdict (aborted: mcp server serena status failed)"),
            "{md}"
        );
        assert!(!md.contains("serena does not earn"), "{md}");
        assert!(
            md.contains("aborted: serena (mcp server serena status failed)"),
            "{md}"
        );
    }

    #[test]
    fn aborted_baseline_yields_no_verdict_at_all() {
        let mut aborted = BTreeMap::new();
        aborted.insert("alone".to_string(), "checkout dirty".to_string());
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
                "singularrag".to_string(),
                vec![rec("L1", "singularrag", 1.0, 500, 4, false)],
            ),
        ];
        let md = render(&meta, "alone", &conds, &["L1".into()]);
        assert!(
            md.contains("No verdict: baseline condition alone was aborted (checkout dirty)."),
            "{md}"
        );
        assert!(!md.contains("earns its place"), "{md}");
        assert!(!md.contains("does not earn"), "{md}");
    }

    #[test]
    fn a_condition_with_no_records_is_skipped_in_the_verdict() {
        let meta = RunMeta {
            run_dir: "r".into(),
            commit: "c".into(),
            claude_version: "v".into(),
            singularrag_version: None,
            aborted: BTreeMap::new(),
        };
        let conds = vec![
            (
                "alone".to_string(),
                vec![rec("L1", "alone", 0.5, 1000, 10, false)],
            ),
            ("singularrag".to_string(), vec![]),
        ];
        let md = render(&meta, "alone", &conds, &["L1".into()]);
        assert!(md.contains("singularrag: no verdict (no records)"), "{md}");
        assert!(!md.contains("singularrag earns"), "{md}");
        assert!(!md.contains("singularrag does not earn"), "{md}");
    }

    #[test]
    fn hook_denials_are_summed_into_their_own_column() {
        let meta = RunMeta {
            run_dir: "eval/runs/20260921T000000Z-run".into(),
            commit: "098e119".into(),
            claude_version: "2.1.261".into(),
            singularrag_version: None,
            aborted: BTreeMap::new(),
        };
        let mut a = rec("L1", "singularrag+hook", 1.0, 500, 4, false);
        a.hook_denials = 2;
        let conds = vec![
            (
                "alone".to_string(),
                vec![rec("L1", "alone", 0.5, 1000, 10, false)],
            ),
            ("singularrag+hook".to_string(), vec![a]),
        ];
        let md = render(&meta, "alone", &conds, &["L1".into()]);
        assert!(
            md.contains("| condition | sessions | failed | hook denials |"),
            "{md}"
        );
        assert!(
            md.contains("| singularrag+hook | 1 | 0 | 2 | 1.00 |"),
            "{md}"
        );
        assert!(md.contains("| alone | 1 | 0 | 0 | 0.50 |"), "{md}");
    }
}
