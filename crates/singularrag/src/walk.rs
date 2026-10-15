//! `singularrag walk`: runs a walk file against the data paths the UI renders (walks spec §3).
//! A pass means the data the UI would render contains the answer; no DOM, no browser.
//!
//! The connection grader matches by name (`norm_name` for entities, the exact
//! `path::name` for a symbol's blast radius): `--no-models` changes nothing here, since
//! that flag only withholds freshly-extracted entities, not the names already indexed.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, Result};
use rusqlite::params;
use serde::Serialize;
use singularrag_core::blast::{blast_radius, Blast, MAX_DEPTH, MAX_FILES};
use singularrag_core::changed::{changed, Changed};
use singularrag_core::config::MapConfig;
use singularrag_core::engine::{Engine, MapRequest};
use singularrag_core::knowledge::norm_name;
use singularrag_core::map;
use singularrag_core::walks::{Walk, WalkFile, WalkKind};

use crate::serve::queries::{self, EntitiesDto, ProcessDto, ProcessesDto};

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
    /// An explicit `--budget`; `None` when the flag was absent.
    pub budget: Option<usize>,
    /// The walk file's directory: `change` patches are relative to it.
    pub walk_dir: PathBuf,
}

/// A locate walk's budget: the walk's own, then an explicit `--budget`, then the walk
/// file's default, then `DEFAULT_BUDGET`. The flag beats the file so a budget sweep
/// (1024/2048/4096, as tier one is run) works over one file. Clamped.
pub fn effective_budget(w: &Walk, cli: Option<usize>, file: Option<usize>) -> usize {
    map::clamp_budget(w.budget.or(cli).or(file).unwrap_or(map::DEFAULT_BUDGET))
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

/// An entity-start connection walk: every entity whose normalised name matches `w.from`
/// is a BFS root over the entities DTO. Hop 1 from an entity reaches its mention sections
/// (`path::name`) and its related entities (either end of a relation); hop 2 reaches those
/// entities' mentions and relations; and so on to `w.hops()`.
pub fn grade_connection_entity(w: &Walk, dto: &EntitiesDto) -> (bool, Vec<String>, Option<String>) {
    let from = w.from.as_deref().unwrap_or_default();
    let norm_from = norm_name(from);
    let starts: Vec<i64> = dto
        .entities
        .iter()
        .filter(|e| norm_name(&e.name) == norm_from)
        .map(|e| e.id)
        .collect();
    if starts.is_empty() {
        return (false, vec![], Some(format!("{from} not found")));
    }

    let mut visited: HashSet<i64> = starts.iter().copied().collect();
    let mut frontier: Vec<i64> = starts;
    let mut reached: Vec<String> = Vec::new();
    for _ in 0..w.hops() {
        if frontier.is_empty() {
            break;
        }
        let mut next_frontier: Vec<i64> = Vec::new();
        for eid in &frontier {
            for m in dto.mentions.iter().filter(|m| m.entity_id == *eid) {
                if reached.len() < 50 {
                    reached.push(format!("{}::{}", m.path, m.name));
                }
            }
            for rel in &dto.relations {
                let other = if rel.src == *eid {
                    Some(rel.dst)
                } else if rel.dst == *eid {
                    Some(rel.src)
                } else {
                    None
                };
                if let Some(other) = other {
                    if visited.insert(other) {
                        next_frontier.push(other);
                    }
                }
            }
        }
        frontier = next_frontier;
    }

    let pass = w.gold.iter().any(|g| reached.contains(g));
    if pass {
        (true, reached, None)
    } else {
        let first_gold = w.gold.first().cloned().unwrap_or_default();
        let hops = w.hops();
        (
            false,
            reached,
            Some(format!("{first_gold} not within {hops} hops of {from}")),
        )
    }
}

/// A symbol-start connection walk: the start's blast radius, within `w.hops()`.
/// `start` is `(path, symbol)`; `blast` is `None` when the symbol was not found.
pub fn grade_connection_symbol(
    w: &Walk,
    start: (&str, &str),
    blast: Option<&Blast>,
) -> (bool, Vec<String>, Option<String>) {
    let from = format!("{}::{}", start.0, start.1);
    let Some(blast) = blast else {
        return (false, vec![], Some(format!("{from} not found")));
    };
    let hops = w.hops();
    let reached: Vec<String> = blast.files.iter().map(|f| f.path.clone()).collect();
    let pass = w.gold.iter().any(|g| {
        let Some((path, _)) = g.split_once("::") else {
            return false;
        };
        path == start.0
            || blast
                .files
                .iter()
                .any(|f| f.path == path && f.depth <= hops)
    });
    if pass {
        (true, reached, None)
    } else {
        let first_gold = w.gold.first().cloned().unwrap_or_default();
        (
            false,
            reached,
            Some(format!("{first_gold} not within {hops} hops of {from}")),
        )
    }
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

/// A change walk passes when the changed list holds every gold key (unlike locate's "any":
/// a change walk asks "does the list contain all of these").
pub fn grade_change(gold: &[String], c: &Changed) -> (bool, Vec<String>, Option<String>) {
    let reached: Vec<String> = c
        .symbols
        .iter()
        .map(|s| format!("{}::{}", s.path, s.name))
        .collect();
    match gold.iter().find(|g| !reached.contains(g)) {
        Some(missing) => (
            false,
            reached,
            Some(format!("{missing} not in the changed list")),
        ),
        None => (true, reached, None),
    }
}

/// Every file a patch touches (`--- a/<path>` and `+++ b/<path>`), once each, in order;
/// `/dev/null` (the old side of a new file, the new side of a deleted one) is skipped.
pub fn patch_paths(patch: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in patch.lines() {
        let Some(p) = line
            .strip_prefix("+++ b/")
            .or_else(|| line.strip_prefix("--- a/"))
        else {
            continue;
        };
        let p = p.trim_end();
        if p != "/dev/null" && !out.iter().any(|o| o == p) {
            out.push(p.to_string());
        }
    }
    out
}

/// `git -C root <args>`: `Ok(stdout)`, or `Err(first stderr line)` when git fails.
fn git_run(root: &Path, args: &[&str]) -> Result<std::result::Result<String, String>> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| anyhow!("running git: {e}"))?;
    if out.status.success() {
        Ok(Ok(String::from_utf8_lossy(&out.stdout).into_owned()))
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Ok(Err(err.lines().next().unwrap_or("").trim().to_string()))
    }
}

fn git(root: &Path, args: &[&str]) -> Result<String> {
    git_run(root, args)?.map_err(|e| anyhow!("git {}: {e}", args.join(" ")))
}

/// Clean means no tracked edit and no untracked file, ignoring our own index directory
/// (`.singularrag/`, which opening the engine creates inside the corpus root).
fn is_clean(root: &Path) -> Result<bool> {
    Ok(git(
        root,
        &[
            "status",
            "--porcelain",
            "--untracked-files=all",
            "--",
            ".",
            ":(exclude).singularrag",
        ],
    )?
    .trim()
    .is_empty())
}

/// `git apply --check`, then `git apply`; a failed check is `patch does not apply: <why>`.
fn apply_patch(root: &Path, patch: &Path) -> Result<()> {
    let p = patch.to_string_lossy();
    if let Err(why) = git_run(root, &["apply", "--check", &p])? {
        return Err(anyhow!("patch does not apply: {why}"));
    }
    git(root, &["apply", &p]).map(|_| ())
}

/// Tracked edits go back with `checkout`; a file the patch added has nothing to check out,
/// so that error is ignored and `clean` removes it. One `checkout` per path: git refuses
/// the whole call when any one pathspec matches nothing, which would leave every edit.
fn revert(root: &Path, paths: &[String]) -> Result<()> {
    for p in paths {
        let _ = git(root, &["checkout", "--", p]);
    }
    let mut args: Vec<&str> = vec!["clean", "-fd", "--"];
    args.extend(paths.iter().map(String::as_str));
    git(root, &args).map(|_| ())
}

/// Reverts the patched paths when dropped, so an error between apply and grade (a failed
/// refresh, a failed `changed`) still leaves the checkout as it was.
struct Revert {
    root: PathBuf,
    paths: Vec<String>,
}

impl Drop for Revert {
    fn drop(&mut self) {
        if let Err(e) = revert(&self.root, &self.paths) {
            eprintln!(
                "walk: reverting {} in {} failed: {e}",
                self.paths.join(", "),
                self.root.display()
            );
        }
    }
}

/// A change walk: refuse unless the walk file lives outside the corpus and the checkout is
/// clean; apply the walk's patch, index, read the changed list, grade, and revert.
fn run_change(
    engine: &mut Engine,
    w: &Walk,
    opts: &WalkOptions,
) -> Result<(bool, Vec<String>, Option<String>)> {
    let refuse = |why: String| Ok((false, vec![], Some(why)));
    let root = engine.workspace().dir.clone();
    let walk_dir = opts
        .walk_dir
        .canonicalize()
        .unwrap_or_else(|_| opts.walk_dir.clone());
    if walk_dir.starts_with(&root) {
        return refuse("walk file lives inside the corpus: refused".into());
    }
    match is_clean(&root) {
        Ok(true) => {}
        Ok(false) => return refuse("checkout is dirty: refused".into()),
        Err(e) => return refuse(format!("{e}: refused")),
    }
    let rel = w.patch.as_deref().unwrap_or_default();
    let patch_file = match walk_dir.join(rel).canonicalize() {
        Ok(p) => p,
        Err(e) => return refuse(format!("patch {rel}: {e}")),
    };
    let paths = patch_paths(&std::fs::read_to_string(&patch_file)?);
    if let Err(e) = apply_patch(&root, &patch_file) {
        return refuse(e.to_string());
    }
    let guard = Revert {
        root: root.clone(),
        paths,
    };
    engine.refresh(Duration::from_secs(600))?;
    let config = MapConfig::load(&root)?;
    let c = changed(engine.store(), &config, engine.workspace(), None)?;
    drop(guard);
    // Re-index the reverted tree, so later walks in the file see the corpus as it is.
    engine.refresh(Duration::from_secs(600))?;
    Ok(grade_change(&w.gold, &c))
}

pub fn run_walks(
    engine: &mut Engine,
    file: &WalkFile,
    opts: &WalkOptions,
) -> Result<Vec<WalkResult>> {
    let mut processes: Option<ProcessesDto> = None;
    let mut entities: Option<EntitiesDto> = None;
    let mut results = Vec::with_capacity(file.walk.len());
    for w in &file.walk {
        let (pass, reached, reason) = match w.kind {
            WalkKind::Locate => {
                let budget = effective_budget(w, opts.budget, file.budget);
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
            WalkKind::Connection => {
                let from = w.from.clone().unwrap_or_default();
                if let Some((path, symbol)) = from.split_once("::") {
                    let config = MapConfig::load(&engine.workspace().dir)?;
                    let blast = blast_radius(
                        engine.store(),
                        &config,
                        path,
                        symbol,
                        w.hops().min(MAX_DEPTH),
                        MAX_FILES,
                    )?;
                    grade_connection_symbol(w, (path, symbol), blast.as_ref())
                } else {
                    if entities.is_none() {
                        entities = Some(queries::entities(engine.store())?);
                    }
                    grade_connection_entity(w, entities.as_ref().expect("loaded above"))
                }
            }
            WalkKind::Change => run_change(engine, w, opts)?,
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
    use crate::serve::queries::{
        EntityDto, EntityMentionDto, EntityRelationDto, SectionRefDto, StepDto,
    };

    #[test]
    fn the_budget_is_the_walks_then_the_flags_then_the_files_then_the_default() {
        let mut w = walk(WalkKind::Locate, &["a::b"]);
        assert_eq!(effective_budget(&w, None, None), map::DEFAULT_BUDGET);
        assert_eq!(effective_budget(&w, None, Some(2048)), 2048);
        assert_eq!(effective_budget(&w, Some(4096), Some(2048)), 4096);
        w.budget = Some(512);
        assert_eq!(effective_budget(&w, Some(4096), Some(2048)), 512);
    }

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

    fn edto(
        entities: &[(i64, &str, &str)],
        relations: &[(i64, i64)],
        mentions: &[(i64, &str)],
    ) -> EntitiesDto {
        EntitiesDto {
            entities: entities
                .iter()
                .map(|(id, n, t)| EntityDto {
                    id: *id,
                    name: n.to_string(),
                    r#type: t.to_string(),
                    description: String::new(),
                    mentions: 1,
                })
                .collect(),
            relations: relations
                .iter()
                .enumerate()
                .map(|(i, (s, d))| EntityRelationDto {
                    id: i as i64,
                    src: *s,
                    dst: *d,
                    description: String::new(),
                    symbol_id: 0,
                    path: "p.md".into(),
                    name: "s".into(),
                })
                .collect(),
            mentions: mentions
                .iter()
                .map(|(e, key)| {
                    let (p, n) = key.split_once("::").unwrap();
                    EntityMentionDto {
                        entity_id: *e,
                        symbol_id: 0,
                        path: p.into(),
                        name: n.into(),
                    }
                })
                .collect(),
            truncated: false,
        }
    }

    #[test]
    fn an_entity_reaches_its_mentions_at_one_hop_and_its_relations_mentions_at_two() {
        let d = edto(
            &[(1, "Okta", "system"), (2, "IT desk", "organisation")],
            &[(2, 1)],
            &[(1, "docs/handbook.md::Onboarding"), (2, "docs/it.md::Desk")],
        );
        let mut w = walk(WalkKind::Connection, &["docs/it.md::Desk"]);
        w.from = Some("okta".into());
        w.hops = Some(1);
        let (pass, _, reason) = grade_connection_entity(&w, &d);
        assert!(!pass);
        assert_eq!(
            reason.as_deref(),
            Some("docs/it.md::Desk not within 1 hops of okta")
        );
        w.hops = Some(2);
        let (pass, reached, _) = grade_connection_entity(&w, &d);
        assert!(pass, "{reached:?}");
    }

    #[test]
    fn an_entity_start_tries_every_entity_of_that_name() {
        let d = edto(
            &[(1, "Finance", "organisation"), (2, "Finance", "role")],
            &[],
            &[
                (1, "docs/a.md::Budget"),
                (2, "docs/handbook.md::Expense process"),
            ],
        );
        let mut w = walk(WalkKind::Connection, &["docs/handbook.md::Expense process"]);
        w.from = Some("Finance".into());
        assert!(grade_connection_entity(&w, &d).0);
    }

    #[test]
    fn an_unknown_entity_start_fails_with_not_found() {
        let d = edto(
            &[(1, "Okta", "system")],
            &[],
            &[(1, "docs/handbook.md::Onboarding")],
        );
        let mut w = walk(WalkKind::Connection, &["docs/handbook.md::Onboarding"]);
        w.from = Some("Workday".into());
        assert_eq!(
            grade_connection_entity(&w, &d).2.as_deref(),
            Some("Workday not found")
        );
    }

    #[test]
    fn a_symbol_start_uses_the_blast_radius_within_hops() {
        use singularrag_core::blast::BlastFile;
        let b = Blast {
            root_path: "src/auth/session.ts".into(),
            root_symbol: "createSession".into(),
            files: vec![
                BlastFile {
                    path: "src/http/middleware.ts".into(),
                    depth: 1,
                    via: "createSession".into(),
                },
                BlastFile {
                    path: "src/http/routes.ts".into(),
                    depth: 2,
                    via: "requireSession".into(),
                },
            ],
            truncated: None,
        };
        let mut w = walk(WalkKind::Connection, &["src/http/routes.ts::route"]);
        w.from = Some("src/auth/session.ts::createSession".into());
        w.hops = Some(1);
        assert!(!grade_connection_symbol(&w, ("src/auth/session.ts", "createSession"), Some(&b)).0);
        w.hops = Some(2);
        assert!(grade_connection_symbol(&w, ("src/auth/session.ts", "createSession"), Some(&b)).0);
        assert_eq!(
            grade_connection_symbol(&w, ("src/x.ts", "nope"), None)
                .2
                .as_deref(),
            Some("src/x.ts::nope not found")
        );
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

    #[test]
    fn patch_paths_lists_touched_files_once_including_new_ones() {
        let p = "--- a/src/a.ts\n+++ b/src/a.ts\n@@ -1 +1 @@\n-x\n+y\n--- /dev/null\n+++ b/src/new.ts\n@@ -0,0 +1 @@\n+z\n";
        assert_eq!(patch_paths(p), vec!["src/a.ts", "src/new.ts"]);
    }

    #[test]
    fn grade_change_needs_every_gold_in_the_list() {
        use singularrag_core::changed::{Changed, ChangedSymbol};
        let sym = |p: &str, n: &str| ChangedSymbol {
            symbol_id: 1,
            path: p.into(),
            name: n.into(),
            kind: "function".into(),
            signature: String::new(),
            line_start: 1,
            line_end: 2,
            referenced_by: vec![],
        };
        let c = Changed {
            base: "HEAD".into(),
            symbols: vec![sym("src/a.ts", "a"), sym("src/b.ts", "b")],
            files_without_symbols: vec![],
            truncated: false,
        };
        assert!(grade_change(&["src/a.ts::a".into()], &c).0);
        let (pass, _, reason) = grade_change(&["src/a.ts::a".into(), "src/c.ts::c".into()], &c);
        assert!(!pass);
        assert_eq!(
            reason.as_deref(),
            Some("src/c.ts::c not in the changed list")
        );
    }

    /// A temp git repo with `files` committed.
    fn repo(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for (p, body) in files {
            let f = root.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, body).unwrap();
        }
        for args in [
            vec!["init", "-q"],
            vec!["add", "."],
            vec![
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "-m",
                "one",
            ],
        ] {
            git(root, &args).unwrap();
        }
        dir
    }

    #[test]
    fn a_patch_adding_a_new_file_is_reverted_with_clean() {
        let dir = repo(&[("a.txt", "x\n")]);
        let root = dir.path();
        let outside = tempfile::tempdir().unwrap();
        let patch = outside.path().join("p.patch");
        std::fs::write(&patch, "--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-x\n+y\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1 @@\n+z\n").unwrap();
        assert!(is_clean(root).unwrap());
        apply_patch(root, &patch).unwrap();
        assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "y\n");
        assert!(root.join("new.txt").exists());
        revert(root, &["a.txt".into(), "new.txt".into()]).unwrap();
        assert!(is_clean(root).unwrap());
        assert!(!root.join("new.txt").exists());
    }

    const SESSION_TS: &str = "export function createSession() {\n  return 1;\n}\n";
    const SESSION_PATCH: &str = "--- a/src/session.ts\n+++ b/src/session.ts\n@@ -1,3 +1,3 @@\n export function createSession() {\n-  return 1;\n+  return 2;\n }\n";

    fn change_walk(gold: &[&str]) -> Walk {
        let mut w = walk(WalkKind::Change, gold);
        w.patch = Some("W.patch".into());
        w
    }

    #[test]
    fn a_dirty_checkout_is_refused_before_any_write() {
        let dir = repo(&[("src/session.ts", SESSION_TS)]);
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("W.patch"), SESSION_PATCH).unwrap();
        let edited = "export function createSession() {\n  return 9;\n}\n";
        std::fs::write(dir.path().join("src/session.ts"), edited).unwrap();
        let mut engine = Engine::open(dir.path(), "walk-test").unwrap();
        let opts = WalkOptions {
            budget: None,
            walk_dir: outside.path().to_path_buf(),
        };
        let w = change_walk(&["src/session.ts::createSession"]);
        let (pass, reached, reason) = run_change(&mut engine, &w, &opts).unwrap();
        assert!(!pass && reached.is_empty());
        assert_eq!(reason.as_deref(), Some("checkout is dirty: refused"));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("src/session.ts")).unwrap(),
            edited,
            "the uncommitted edit is untouched"
        );
    }

    #[test]
    fn a_walk_file_inside_the_corpus_is_refused() {
        let dir = repo(&[("src/session.ts", SESSION_TS)]);
        std::fs::create_dir_all(dir.path().join("walks")).unwrap();
        let mut engine = Engine::open(dir.path(), "walk-test").unwrap();
        let opts = WalkOptions {
            budget: None,
            walk_dir: dir.path().join("walks"),
        };
        let w = change_walk(&["src/session.ts::createSession"]);
        assert_eq!(
            run_change(&mut engine, &w, &opts).unwrap().2.as_deref(),
            Some("walk file lives inside the corpus: refused")
        );
    }

    #[test]
    fn a_change_walk_applies_grades_and_leaves_the_checkout_clean() {
        let dir = repo(&[("src/session.ts", SESSION_TS)]);
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("W.patch"), SESSION_PATCH).unwrap();
        let mut engine = Engine::open(dir.path(), "walk-test").unwrap();
        engine.refresh(std::time::Duration::from_secs(600)).unwrap();
        let opts = WalkOptions {
            budget: None,
            walk_dir: outside.path().to_path_buf(),
        };
        let w = change_walk(&["src/session.ts::createSession"]);
        let (pass, reached, reason) = run_change(&mut engine, &w, &opts).unwrap();
        assert!(pass, "{reached:?} {reason:?}");
        assert_eq!(reached, vec!["src/session.ts::createSession"]);
        assert!(is_clean(dir.path()).unwrap());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("src/session.ts")).unwrap(),
            SESSION_TS
        );
        // A patch that does not apply names git's reason and writes nothing.
        std::fs::write(outside.path().join("W.patch"), "--- a/src/session.ts\n+++ b/src/session.ts\n@@ -1,3 +1,3 @@\n export function createSession() {\n-  return 7;\n+  return 8;\n }\n").unwrap();
        let reason = run_change(&mut engine, &w, &opts).unwrap().2.unwrap();
        assert!(reason.starts_with("patch does not apply: "), "{reason}");
        assert!(is_clean(dir.path()).unwrap());
    }
}
