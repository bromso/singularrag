//! Tier-one eval: does the budgeted map contain the gold symbols? No LLM involved.

use std::path::Path;

use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::engine::{Engine, MapRequest};
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

#[derive(Debug, Clone, Serialize)]
pub struct EvalResult {
    pub id: String,
    pub category: String,
    pub recall: f64,
    /// Approximate size of the served map text, so a budget's recall has its cost beside it.
    pub tokens: usize,
    pub hit: Vec<String>,
    pub miss: Vec<String>,
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
        })?;
        let served = served_keys(engine, resp.retrieval_id)?;
        let (hit, miss): (Vec<String>, Vec<String>) =
            q.gold.iter().cloned().partition(|g| served.contains(g));
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
    let mut out = String::from("id    category  recall  tokens  missed\n");
    for r in results {
        out.push_str(&format!(
            "{:<5} {:<9} {:>5.2}  {:>6}  {}\n",
            r.id,
            r.category,
            r.recall,
            r.tokens,
            r.miss.join(", ")
        ));
    }
    out.push_str(&format!(
        "mean recall {:.3} over {} questions\n",
        mean_recall(results),
        results.len()
    ));
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
            report.starts_with("id    category  recall  tokens  missed\n"),
            "{report}"
        );
        assert!(report.contains("L1"));
        assert!(report.contains("mean recall"));
        assert!((mean_recall(&results) - 0.875).abs() < 1e-9);
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
