//! Tier-one eval: does the budgeted map contain the gold symbols? No LLM involved.

use std::path::Path;

use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::engine::{Engine, EntitiesRequest, MapRequest, ENTITIES_LIMIT_DEFAULT};
use crate::{Error, Result};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Question {
    pub id: String,
    pub category: String,
    pub query: String,
    /// `path::name` entries.
    pub gold: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct QuestionFile {
    pub question: Vec<Question>,
}

/// Categories whose questions are also put to the `entities` tool, for the `cited` column.
pub const CITED_CATEGORIES: [&str; 3] = ["entity", "relation", "process"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalResult {
    pub id: String,
    pub category: String,
    pub recall: f64,
    /// Approximate size of the served map text, so a budget's recall has its cost beside it.
    pub tokens: usize,
    pub hit: Vec<String>,
    pub miss: Vec<String>,
    /// For `CITED_CATEGORIES`: whether the `entities` tool, given the query, served any
    /// gold section. `None` for the other categories.
    #[serde(default)]
    pub cited: Option<bool>,
}

pub fn load_questions(path: &Path) -> Result<Vec<Question>> {
    let s = std::fs::read_to_string(path)?;
    let f: QuestionFile = toml::from_str(&s).map_err(|e| Error::Config(e.to_string()))?;
    Ok(f.question)
}

fn served_keys(engine: &Engine, retrieval_id: i64) -> Result<Vec<String>> {
    // Straight off the recorded row: `symbol_id` is not stable across a reindex.
    let mut stmt = engine.store().conn().prepare(
        "SELECT path || '::' || name FROM retrieval_items
         WHERE retrieval_id = ?1 AND served = 1",
    )?;
    let keys = stmt
        .query_map(params![retrieval_id], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(keys)
}

pub fn run(engine: &mut Engine, questions: &[Question], budget: usize) -> Result<Vec<EvalResult>> {
    let mut out = Vec::new();
    for q in questions {
        let resp = engine.repo_map(&MapRequest {
            query: Some(q.query.clone()),
            focus_files: vec![],
            budget_tokens: budget,
            ..Default::default()
        })?;
        let served = served_keys(engine, resp.retrieval_id)?;
        let (hit, miss): (Vec<String>, Vec<String>) =
            q.gold.iter().cloned().partition(|g| served.contains(g));
        let cited = if CITED_CATEGORIES.contains(&q.category.as_str()) {
            let r = engine.entities(&EntitiesRequest {
                query: q.query.clone(),
                entities: vec![],
                limit: ENTITIES_LIMIT_DEFAULT,
            })?;
            let served = served_keys(engine, r.retrieval_id)?;
            Some(q.gold.iter().any(|g| served.contains(g)))
        } else {
            None
        };
        let recall = if q.gold.is_empty() {
            1.0
        } else {
            hit.len() as f64 / q.gold.len() as f64
        };
        out.push(EvalResult {
            id: q.id.clone(),
            category: q.category.clone(),
            recall,
            tokens: crate::tokens::approx_tokens(&resp.text),
            hit,
            miss,
            cited,
        });
    }
    Ok(out)
}

pub fn mean_recall(results: &[EvalResult]) -> f64 {
    if results.is_empty() {
        return 0.0;
    }
    results.iter().map(|r| r.recall).sum::<f64>() / results.len() as f64
}

pub fn render_report(results: &[EvalResult]) -> String {
    let mut out = String::from("id    category   recall  tokens  cited  missed\n");
    for r in results {
        let cited = match r.cited {
            Some(true) => "yes",
            Some(false) => "no",
            None => "-",
        };
        out.push_str(&format!(
            "{:<5} {:<10} {:>5.2}  {:>6}  {:<5}  {}\n",
            r.id,
            r.category,
            r.recall,
            r.tokens,
            cited,
            r.miss.join(", ")
        ));
    }
    out.push_str(&format!(
        "mean recall {:.3} over {} questions\n",
        mean_recall(results),
        results.len()
    ));
    // One line per category with cited values, in the order the categories first appear.
    let mut categories: Vec<&str> = Vec::new();
    for r in results.iter().filter(|r| r.cited.is_some()) {
        if !categories.contains(&r.category.as_str()) {
            categories.push(&r.category);
        }
    }
    for c in categories {
        let of: Vec<bool> = results
            .iter()
            .filter(|r| r.category == c)
            .filter_map(|r| r.cited)
            .collect();
        let yes = of.iter().filter(|c| **c).count();
        out.push_str(&format!("cited {yes}/{} {c}\n", of.len()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use crate::fixture::write_ts_mini;

    const Q: &str = r#"
[[question]]
id = "L1"
category = "locate"
query = "where are sessions created"
gold = ["src/auth/session.ts::createSession"]

[[question]]
id = "B1"
category = "blast"
query = "what calls createSession"
gold = ["src/http/middleware.ts::requireSession", "src/http/middleware.ts::attachSession", "src/cli/login.ts::login", "src/nope.ts::missing"]
"#;

    #[test]
    fn loads_and_scores_recall_at_budget() {
        let dir = tempfile::tempdir().unwrap();
        write_ts_mini(dir.path());
        let qpath = dir.path().join("questions.toml");
        std::fs::write(&qpath, Q).unwrap();
        let qs = load_questions(&qpath).unwrap();
        assert_eq!(qs.len(), 2);

        let mut e = Engine::open(dir.path(), "eval").unwrap();
        let results = run(&mut e, &qs, 2048).unwrap();
        assert_eq!(results[0].id, "L1");
        assert!((results[0].recall - 1.0).abs() < 1e-9, "{:?}", results[0]);
        assert!((results[1].recall - 0.75).abs() < 1e-9, "{:?}", results[1]);
        assert_eq!(results[1].miss, vec!["src/nope.ts::missing"]);
        assert!(
            results[0].tokens > 0 && results[0].tokens <= 2048,
            "{:?}",
            results[0]
        );
        let report = render_report(&results);
        assert!(
            report.starts_with("id    category   recall  tokens  cited  missed\n"),
            "{report}"
        );
        assert!(report.contains("L1"));
        assert!(report.contains("mean recall"));
        assert!(
            results.iter().all(|r| r.cited.is_none()),
            "locate and blast questions are not put to entities"
        );
        assert!(!report.contains("\ncited "), "{report}");
        assert!((mean_recall(&results) - 0.875).abs() < 1e-9);
    }

    #[test]
    fn the_report_has_a_cited_column_and_a_line_per_cited_category() {
        let r = |id: &str, category: &str, cited: Option<bool>| EvalResult {
            id: id.into(),
            category: category.into(),
            recall: 1.0,
            tokens: 10,
            hit: vec![],
            miss: vec![],
            cited,
        };
        let report = render_report(&[
            r("P1", "paraphrase", None),
            r("S1", "process", Some(true)),
            r("E1", "entity", Some(false)),
            r("S2", "process", Some(true)),
        ]);
        assert!(
            report.contains("\nS1    process     1.00      10  yes    \n"),
            "{report}"
        );
        assert!(
            report.contains("\nE1    entity      1.00      10  no     \n"),
            "{report}"
        );
        assert!(
            report.contains("\nP1    paraphrase  1.00      10  -      \n"),
            "{report}"
        );
        assert!(
            report.ends_with("questions\ncited 2/2 process\ncited 0/1 entity\n"),
            "{report}"
        );
        // A JSON report written before the column existed still reads back.
        let old: EvalResult = serde_json::from_str(
            r#"{"id":"P1","category":"paraphrase","recall":1.0,"tokens":1,"hit":[],"miss":[]}"#,
        )
        .unwrap();
        assert_eq!(old.cited, None);
    }

    #[test]
    fn tiny_budget_lowers_recall() {
        let dir = tempfile::tempdir().unwrap();
        write_ts_mini(dir.path());
        let qpath = dir.path().join("questions.toml");
        std::fs::write(&qpath, Q).unwrap();
        let qs = load_questions(&qpath).unwrap();
        let mut e = Engine::open(dir.path(), "eval").unwrap();
        let big = mean_recall(&run(&mut e, &qs, 4096).unwrap());
        let small = mean_recall(&run(&mut e, &qs, 64).unwrap());
        assert!(small <= big);
    }
}
