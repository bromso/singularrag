//! `singularrag walk`: runs a walk file against the data paths the UI renders (walks spec §3).
//! A pass means the data the UI would render contains the answer; no DOM, no browser.

use std::path::PathBuf;

use anyhow::Result;
use rusqlite::params;
use serde::Serialize;
use singularrag_core::engine::{Engine, MapRequest};
use singularrag_core::map;
use singularrag_core::walks::{Walk, WalkFile, WalkKind};

use crate::serve::queries::{self, ProcessDto, ProcessesDto};

#[derive(Debug, Clone, Serialize)]
pub struct WalkResult {
    pub id: String,
    pub kind: WalkKind,
    pub pass: bool,
    /// What the path reached: served keys, the process name, the neighbours, the changed keys.
    pub reached: Vec<String>,
    pub reason: Option<String>,
}

pub struct WalkOptions {
    pub budget: usize,
    /// The walk file's directory: `change` patches are relative to it.
    #[allow(dead_code)] // read by the change walks (Task 4)
    pub walk_dir: PathBuf,
}

pub fn grade_locate(
    gold: &[String],
    served: &[String],
    budget: usize,
) -> (bool, Vec<String>, Option<String>) {
    let pass = gold.iter().any(|g| served.contains(g));
    (
        pass,
        served.to_vec(),
        (!pass).then(|| format!("gold not served at {budget}")),
    )
}

fn eq_ci(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

fn contains_ci(hay: &str, needle: &str) -> bool {
    hay.to_lowercase().contains(&needle.trim().to_lowercase())
}

pub fn grade_process(w: &Walk, dto: &ProcessesDto) -> (bool, Vec<String>, Option<String>) {
    let cites = |p: &ProcessDto| {
        p.steps.iter().any(|s| {
            w.gold
                .iter()
                .any(|g| format!("{}::{}", s.section.path, s.section.name) == *g)
        })
    };
    let citing: Vec<&ProcessDto> = dto.processes.iter().filter(|p| cites(p)).collect();
    if citing.is_empty() {
        return (
            false,
            vec![],
            Some(format!("no process cites {}", w.gold.join(", "))),
        );
    }
    let reached: Vec<String> = citing.iter().map(|p| p.name.clone()).collect();
    if let Some(role) = &w.role {
        if !citing
            .iter()
            .any(|p| p.steps.iter().any(|s| eq_ci(&s.role, role)))
        {
            return (
                false,
                reached,
                Some(format!("step with role {role} not found")),
            );
        }
    }
    if let Some(text) = &w.step_text {
        if !citing
            .iter()
            .any(|p| p.steps.iter().any(|s| contains_ci(&s.text, text)))
        {
            return (
                false,
                reached,
                Some(format!("step text \"{text}\" not found")),
            );
        }
    }
    (true, reached, None)
}

/// The served `path::name` keys of a recorded retrieval, straight off the row (as tier one
/// reads them: `symbol_id` is not stable across a reindex).
fn served_keys(engine: &Engine, retrieval_id: i64) -> Result<Vec<String>> {
    let mut stmt = engine.store().conn().prepare(
        "SELECT path || '::' || name FROM retrieval_items
         WHERE retrieval_id = ?1 AND served = 1",
    )?;
    let keys = stmt
        .query_map(params![retrieval_id], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(keys)
}

pub fn run_walks(
    engine: &mut Engine,
    file: &WalkFile,
    opts: &WalkOptions,
) -> Result<Vec<WalkResult>> {
    // The walk's own budget, else the file's default, else the command's (spec §2).
    let default_budget = file.budget.unwrap_or(opts.budget);
    let mut processes: Option<ProcessesDto> = None;
    let mut results = Vec::with_capacity(file.walk.len());
    for w in &file.walk {
        let (pass, reached, reason) = match w.kind {
            WalkKind::Locate => {
                let budget = map::clamp_budget(w.budget_or(default_budget));
                let r = engine.repo_map(&MapRequest {
                    query: w.query.clone(),
                    budget_tokens: budget,
                    ..Default::default()
                })?;
                let served = served_keys(engine, r.retrieval_id)?;
                grade_locate(&w.gold, &served, budget)
            }
            WalkKind::Process => {
                if processes.is_none() {
                    processes = Some(queries::processes(engine.store())?);
                }
                grade_process(w, processes.as_ref().expect("loaded above"))
            }
            WalkKind::Connection | WalkKind::Change => {
                (false, vec![], Some("not implemented".to_string()))
            }
        };
        results.push(WalkResult {
            id: w.id.clone(),
            kind: w.kind,
            pass,
            reached,
            reason,
        });
    }
    Ok(results)
}

fn kind_name(k: WalkKind) -> &'static str {
    match k {
        WalkKind::Locate => "locate",
        WalkKind::Process => "process",
        WalkKind::Connection => "connection",
        WalkKind::Change => "change",
    }
}

pub fn render_report(results: &[WalkResult]) -> String {
    let mut out = String::from("id    kind        pass  reached / reason\n");
    for r in results {
        let tail = if r.pass {
            r.reached.join(", ")
        } else {
            r.reason.clone().unwrap_or_default()
        };
        out.push_str(&format!(
            "{:<5} {:<11} {:<5} {}\n",
            r.id,
            kind_name(r.kind),
            if r.pass { "yes" } else { "no" },
            tail
        ));
    }
    for k in [
        WalkKind::Locate,
        WalkKind::Process,
        WalkKind::Connection,
        WalkKind::Change,
    ] {
        let of_kind: Vec<&WalkResult> = results.iter().filter(|r| r.kind == k).collect();
        if !of_kind.is_empty() {
            let passed = of_kind.iter().filter(|r| r.pass).count();
            out.push_str(&format!(
                "walks {passed}/{} {}\n",
                of_kind.len(),
                kind_name(k)
            ));
        }
    }
    let passed = results.iter().filter(|r| r.pass).count();
    out.push_str(&format!("walks {passed}/{}\n", results.len()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serve::queries::{SectionRefDto, StepDto};

    #[test]
    fn locate_passes_when_any_gold_is_served_and_names_the_budget_when_not() {
        let gold = vec!["docs/a.md::One".to_string(), "docs/b.md::Two".to_string()];
        let (pass, reached, reason) = grade_locate(
            &gold,
            &["docs/x.md::X".into(), "docs/b.md::Two".into()],
            2048,
        );
        assert!(pass && reason.is_none());
        assert_eq!(reached, vec!["docs/x.md::X", "docs/b.md::Two"]);
        let (pass, _, reason) = grade_locate(&gold, &["docs/x.md::X".into()], 2048);
        assert!(!pass);
        assert_eq!(reason.as_deref(), Some("gold not served at 2048"));
    }

    #[allow(clippy::type_complexity)] // the brief's fixture shape, kept verbatim
    fn dto(processes: Vec<(&str, Vec<(&str, &str, &str)>)>) -> ProcessesDto {
        // (process name, [(step text, role, section path::name)])
        ProcessesDto {
            processes: processes
                .into_iter()
                .enumerate()
                .map(|(i, (name, steps))| ProcessDto {
                    id: i as i64 + 1,
                    name: name.into(),
                    description: String::new(),
                    roles: steps
                        .iter()
                        .map(|s| s.1.to_string())
                        .filter(|r| !r.is_empty())
                        .collect(),
                    steps: steps
                        .into_iter()
                        .enumerate()
                        .map(|(j, (text, role, sec))| {
                            let (path, name) = sec.split_once("::").unwrap();
                            StepDto {
                                ordinal: j + 1,
                                text: text.into(),
                                role: role.into(),
                                systems: vec![],
                                section: SectionRefDto {
                                    symbol_id: 10 + j as i64,
                                    path: path.into(),
                                    name: name.into(),
                                    line: 1,
                                },
                                code: vec![],
                                more_code: 0,
                            }
                        })
                        .collect(),
                })
                .collect(),
            truncated: false,
        }
    }

    fn walk(kind: WalkKind, gold: &[&str]) -> Walk {
        Walk {
            id: "W".into(),
            kind,
            query: Some("q".into()),
            gold: gold.iter().map(|g| g.to_string()).collect(),
            budget: None,
            role: None,
            step_text: None,
            from: None,
            hops: None,
            patch: None,
        }
    }

    #[test]
    fn a_process_walk_passes_through_a_process_named_differently() {
        let d = dto(vec![(
            "First day",
            vec![(
                "Activate Okta",
                "new colleague",
                "docs/handbook.md::Onboarding",
            )],
        )]);
        let (pass, reached, _) = grade_process(
            &walk(WalkKind::Process, &["docs/handbook.md::Onboarding"]),
            &d,
        );
        assert!(pass);
        assert_eq!(reached, vec!["First day"]);
    }

    #[test]
    fn a_process_walk_checks_role_and_step_text_within_the_citing_process() {
        let d = dto(vec![
            (
                "Expense process",
                vec![
                    (
                        "Submit in Expensify",
                        "you",
                        "docs/handbook.md::Expense process",
                    ),
                    (
                        "Finance reviews within 5 days",
                        "Finance",
                        "docs/handbook.md::Expense process",
                    ),
                ],
            ),
            (
                "Release process",
                vec![(
                    "Cut the branch",
                    "Finance",
                    "docs/handbook.md::Release process",
                )],
            ),
        ]);
        let mut w = walk(WalkKind::Process, &["docs/handbook.md::Expense process"]);
        w.role = Some("finance".into());
        w.step_text = Some("WITHIN 5 DAYS".into());
        assert!(
            grade_process(&w, &d).0,
            "role and step text match case-insensitively"
        );
        w.role = Some("Manager".into());
        let (pass, _, reason) = grade_process(&w, &d);
        assert!(!pass);
        assert_eq!(reason.as_deref(), Some("step with role Manager not found"));
        w.role = None;
        w.step_text = Some("rolls back".into());
        assert_eq!(
            grade_process(&w, &d).2.as_deref(),
            Some("step text \"rolls back\" not found")
        );
        let w = walk(WalkKind::Process, &["docs/handbook.md::Incident process"]);
        assert_eq!(
            grade_process(&w, &d).2.as_deref(),
            Some("no process cites docs/handbook.md::Incident process")
        );
    }

    #[test]
    fn the_report_has_one_row_per_walk_and_a_footer_per_kind() {
        let rs = vec![
            WalkResult {
                id: "W1".into(),
                kind: WalkKind::Locate,
                pass: true,
                reached: vec!["a::b".into()],
                reason: None,
            },
            WalkResult {
                id: "W2".into(),
                kind: WalkKind::Process,
                pass: false,
                reached: vec![],
                reason: Some("no process cites x::y".into()),
            },
        ];
        let out = render_report(&rs);
        assert!(
            out.starts_with("id    kind        pass  reached / reason\n"),
            "{out}"
        );
        assert!(out.contains("W1    locate      yes   a::b\n"), "{out}");
        assert!(
            out.contains("W2    process     no    no process cites x::y\n"),
            "{out}"
        );
        assert!(
            out.ends_with("walks 1/1 locate\nwalks 0/1 process\nwalks 1/2\n"),
            "{out}"
        );
    }
}
