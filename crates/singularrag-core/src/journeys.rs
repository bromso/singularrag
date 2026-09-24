//! Processes, steps and roles (spec: docs/superpowers/specs/2026-09-24-singularrag-journeys-design.md).
use crate::knowledge::norm_name;
use crate::models::StepsAnswer;
use crate::Result;
use rusqlite::{params, Connection, OptionalExtension};

#[derive(Debug, Clone, PartialEq)]
pub struct StepRow {
    pub id: i64,
    pub process_id: i64,
    pub ordinal: i64,
    pub text: String,
    pub role_id: Option<i64>,
    pub role_text: String,
    pub symbol_id: i64,
}

fn section_entity(conn: &Connection, symbol_id: i64, ty: &str, norm: &str) -> Result<Option<i64>> {
    Ok(conn
        .query_row(
            "SELECT e.id FROM entities e JOIN entity_mentions m ON m.entity_id = e.id WHERE m.symbol_id = ?1 AND e.type = ?2 AND e.norm_name = ?3 LIMIT 1",
            params![symbol_id, ty, norm],
            |r| r.get(0),
        )
        .optional()?)
}

pub fn delete_steps_for_symbol(conn: &Connection, symbol_id: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM step_systems WHERE step_id IN (SELECT id FROM steps WHERE symbol_id = ?1)",
        [symbol_id],
    )?;
    conn.execute("DELETE FROM steps WHERE symbol_id = ?1", [symbol_id])?;
    Ok(())
}

pub fn delete_steps_for_symbols(conn: &Connection, symbol_ids_sql: &str, param: i64) -> Result<()> {
    conn.execute(
        &format!("DELETE FROM step_systems WHERE step_id IN (SELECT id FROM steps WHERE symbol_id IN ({symbol_ids_sql}))"),
        [param],
    )?;
    conn.execute(
        &format!("DELETE FROM steps WHERE symbol_id IN ({symbol_ids_sql})"),
        [param],
    )?;
    Ok(())
}

/// Replaces this section's steps. `Ok(None)` when no `process` entity of this section
/// matches `a.process` by `norm_name`; otherwise the prior steps for this section are
/// dropped and replaced, so applying twice is harmless.
pub fn apply_steps(
    conn: &Connection,
    symbol_id: i64,
    hash: &str,
    a: &StepsAnswer,
) -> Result<Option<usize>> {
    let Some(process_id) = section_entity(conn, symbol_id, "process", &norm_name(&a.process))?
    else {
        return Ok(None);
    };
    delete_steps_for_symbol(conn, symbol_id)?;
    let mut n = 0;
    for (i, s) in a.steps.iter().enumerate() {
        let role_id = if s.role.is_empty() {
            None
        } else {
            section_entity(conn, symbol_id, "role", &norm_name(&s.role))?
        };
        conn.execute(
            "INSERT INTO steps(process_id, ordinal, text, role_id, role_text, symbol_id, section_hash) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![process_id, i as i64 + 1, s.text, role_id, s.role, symbol_id, hash],
        )?;
        let step_id = conn.last_insert_rowid();
        for sys in &s.systems {
            if let Some(eid) = section_entity(conn, symbol_id, "system", &norm_name(sys))? {
                conn.execute(
                    "INSERT OR IGNORE INTO step_systems(step_id, entity_id) VALUES (?1, ?2)",
                    [step_id, eid],
                )?;
            }
        }
        n += 1;
    }
    Ok(Some(n))
}

pub fn steps_for_process(conn: &Connection, process_id: i64) -> Result<Vec<StepRow>> {
    let mut stmt = conn.prepare(
        "SELECT st.id, st.process_id, st.ordinal, st.text, st.role_id, st.role_text, st.symbol_id FROM steps st JOIN symbols s ON s.id = st.symbol_id WHERE st.process_id = ?1 ORDER BY s.line_start, st.ordinal",
    )?;
    let rows = stmt.query_map([process_id], |r| {
        Ok(StepRow {
            id: r.get(0)?,
            process_id: r.get(1)?,
            ordinal: r.get(2)?,
            text: r.get(3)?,
            role_id: r.get(4)?,
            role_text: r.get(5)?,
            symbol_id: r.get(6)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

pub fn systems_for_step(conn: &Connection, step_id: i64) -> Result<Vec<(i64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT e.id, e.name FROM step_systems ss JOIN entities e ON e.id = ss.entity_id WHERE ss.step_id = ?1 ORDER BY e.name",
    )?;
    let rows = stmt.query_map([step_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

pub fn role_name(conn: &Connection, step: &StepRow) -> Result<String> {
    match step.role_id {
        Some(id) => Ok(conn
            .query_row("SELECT name FROM entities WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .optional()?
            .unwrap_or_else(|| step.role_text.clone())),
        None => Ok(step.role_text.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ExtractedStep;
    use crate::store::Store;

    fn temp_store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        (dir, store)
    }

    fn seed_section(conn: &Connection, path: &str, name: &str, line: i64) -> i64 {
        conn.execute(
            "INSERT INTO files(path, lang, content_hash, mtime_ms, size, indexed_at_ms, skipped_reason) VALUES (?1, NULL, 'h', 0, 1, 0, NULL) ON CONFLICT(path) DO NOTHING",
            [path],
        )
        .unwrap();
        let file_id: i64 = conn
            .query_row("SELECT id FROM files WHERE path = ?1", [path], |r| r.get(0))
            .unwrap();
        conn.execute(
            "INSERT INTO symbols(file_id, name, kind, line_start, line_end, signature) VALUES (?1, ?2, 'section', ?3, ?3, '')",
            params![file_id, name, line],
        )
        .unwrap();
        let symbol_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO sections_fts(rowid, path, name, content) VALUES (?1, ?2, ?3, 'body')",
            params![symbol_id, path, name],
        )
        .unwrap();
        symbol_id
    }

    fn seed_entity(conn: &Connection, symbol_id: i64, name: &str, ty: &str) -> i64 {
        conn.execute(
            "INSERT INTO entities(name, norm_name, type, description, mentions) VALUES (?1, ?2, ?3, '', 1)",
            params![name, norm_name(name), ty],
        )
        .unwrap();
        let id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO entity_mentions(entity_id, symbol_id, section_hash) VALUES (?1, ?2, 'h')",
            params![id, symbol_id],
        )
        .unwrap();
        id
    }

    #[test]
    fn apply_steps_resolves_process_role_and_systems_from_the_sections_own_entities() {
        let (_dir, store) = temp_store();
        let conn = store.conn();
        let sec = seed_section(conn, "docs/handbook.md", "Expense process", 10);
        let process = seed_entity(conn, sec, "Expense process", "process");
        let manager = seed_entity(conn, sec, "Manager", "role");
        let expensify = seed_entity(conn, sec, "Expensify", "system");
        let other_sec = seed_section(conn, "docs/other.md", "Elsewhere", 1);
        let _okta_elsewhere = seed_entity(conn, other_sec, "Okta", "system");
        let a = StepsAnswer {
            process: "expense process".into(),
            steps: vec![
                ExtractedStep {
                    text: "Submit in Expensify".into(),
                    role: "Employee".into(),
                    systems: vec!["Expensify".into(), "Okta".into()],
                },
                ExtractedStep {
                    text: "Manager approves".into(),
                    role: "manager".into(),
                    systems: vec![],
                },
            ],
        };
        assert_eq!(apply_steps(conn, sec, "h", &a).unwrap(), Some(2));
        let rows = steps_for_process(conn, process).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            (rows[0].ordinal, rows[0].role_id, rows[0].role_text.as_str()),
            (1, None, "Employee")
        );
        assert_eq!((rows[1].ordinal, rows[1].role_id), (2, Some(manager)));
        assert_eq!(
            systems_for_step(conn, rows[0].id).unwrap(),
            vec![(expensify, "Expensify".to_string())],
            "Okta belongs to another section and is dropped"
        );
        assert_eq!(role_name(conn, &rows[1]).unwrap(), "Manager");
        // re-applying replaces, never duplicates
        assert_eq!(apply_steps(conn, sec, "h", &a).unwrap(), Some(2));
        assert_eq!(steps_for_process(conn, process).unwrap().len(), 2);
    }

    #[test]
    fn an_unmatched_process_name_returns_none_and_writes_nothing() {
        let (_dir, store) = temp_store();
        let conn = store.conn();
        let sec = seed_section(conn, "docs/handbook.md", "Expense process", 10);
        let process = seed_entity(conn, sec, "Expense process", "process");
        let a = StepsAnswer {
            process: "Expenses".into(),
            steps: vec![ExtractedStep {
                text: "x".into(),
                ..Default::default()
            }],
        };
        assert_eq!(apply_steps(conn, sec, "h", &a).unwrap(), None);
        assert!(steps_for_process(conn, process).unwrap().is_empty());
    }

    #[test]
    fn steps_merge_across_sections_in_document_order() {
        let (_dir, store) = temp_store();
        let conn = store.conn();
        let later = seed_section(conn, "docs/handbook.md", "Travel", 40);
        let earlier = seed_section(conn, "docs/handbook.md", "Expense process", 10);
        let p = seed_entity(conn, earlier, "Expense process", "process");
        conn.execute(
            "INSERT INTO entity_mentions(entity_id, symbol_id, section_hash) VALUES (?1, ?2, 'h')",
            [p, later],
        )
        .unwrap();
        let a = |t: &str| StepsAnswer {
            process: "Expense process".into(),
            steps: vec![ExtractedStep {
                text: t.into(),
                ..Default::default()
            }],
        };
        apply_steps(conn, later, "h2", &a("book travel first")).unwrap();
        apply_steps(conn, earlier, "h1", &a("submit expense")).unwrap();
        let texts: Vec<String> = steps_for_process(conn, p)
            .unwrap()
            .into_iter()
            .map(|r| r.text)
            .collect();
        assert_eq!(texts, vec!["submit expense", "book travel first"]);
    }

    #[test]
    fn deleting_a_file_deletes_its_steps_and_systems() {
        let (_dir, store) = temp_store();
        let conn = store.conn();
        let sec = seed_section(conn, "docs/handbook.md", "Expense process", 10);
        let p = seed_entity(conn, sec, "Expense process", "process");
        let s = seed_entity(conn, sec, "Expensify", "system");
        apply_steps(
            conn,
            sec,
            "h",
            &StepsAnswer {
                process: "Expense process".into(),
                steps: vec![ExtractedStep {
                    text: "x".into(),
                    systems: vec!["Expensify".into()],
                    ..Default::default()
                }],
            },
        )
        .unwrap();
        let file_id: i64 = conn
            .query_row("SELECT file_id FROM symbols WHERE id = ?1", [sec], |r| {
                r.get(0)
            })
            .unwrap();
        crate::knowledge::delete_for_symbols(conn, file_id).unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM steps", [], |r| r.get(0))
            .unwrap();
        let m: i64 = conn
            .query_row("SELECT COUNT(*) FROM step_systems", [], |r| r.get(0))
            .unwrap();
        assert_eq!((n, m), (0, 0));
        let _ = (p, s);
    }
}
