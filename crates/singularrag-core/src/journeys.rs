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

pub const DOC_KINDS: [&str; 5] = ["section", "document", "element", "rule", "key"];
/// `key` rows are config keys and do count as code for "implemented by"; only the four prose
/// kinds are excluded there.
const NON_CODE_KINDS: [&str; 4] = ["section", "document", "element", "rule"];
pub const LINK_LIMIT: usize = 3;
pub const MIN_SYSTEM_CHARS: usize = 3;

pub fn normalise_ident(s: &str) -> String {
    s.chars()
        .filter(|c| *c != '-' && *c != '_')
        .flat_map(|c| c.to_lowercase())
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Via {
    Mention,
    System(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeLink {
    pub symbol_id: i64,
    pub path: String,
    pub name: String,
    pub line: u32,
    pub via: Via,
}

struct CodeSym {
    symbol_id: i64,
    file_id: i64,
    path: String,
    name: String,
    line: u32,
    norm_name: String,
    norm_stem: String,
    referrers: i64,
}

pub struct CodeIndex {
    syms: Vec<CodeSym>,
}

pub fn load_code_index(conn: &Connection) -> Result<CodeIndex> {
    let kinds = NON_CODE_KINDS
        .iter()
        .map(|k| format!("'{k}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut stmt = conn.prepare(&format!(
        "SELECT s.id, s.file_id, f.path, s.name, s.line_start,
                (SELECT COUNT(DISTINCT r.file_id) FROM refs r WHERE r.name = s.name AND r.file_id != s.file_id)
         FROM symbols s JOIN files f ON f.id = s.file_id
         WHERE s.kind NOT IN ({kinds})
         ORDER BY s.id"
    ))?;
    let syms = stmt
        .query_map([], |r| {
            let path: String = r.get(2)?;
            let name: String = r.get(3)?;
            let stem = std::path::Path::new(&path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            Ok(CodeSym {
                symbol_id: r.get(0)?,
                file_id: r.get(1)?,
                norm_name: normalise_ident(&name),
                norm_stem: normalise_ident(&stem),
                path,
                name,
                line: r.get::<_, i64>(4)? as u32,
                referrers: r.get(5)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(CodeIndex { syms })
}

fn section_mentions(conn: &Connection, index: &CodeIndex, symbol_id: i64) -> Result<Vec<CodeLink>> {
    let (file_id, a, b): (i64, i64, i64) = conn.query_row(
        "SELECT file_id, line_start, line_end FROM symbols WHERE id = ?1",
        [symbol_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let mut stmt = conn
        .prepare("SELECT DISTINCT name FROM refs WHERE file_id = ?1 AND line BETWEEN ?2 AND ?3")?;
    let names: Vec<String> = stmt
        .query_map(params![file_id, a, b], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    let mut out = Vec::new();
    for s in &index.syms {
        if s.file_id != file_id && names.iter().any(|n| n == &s.name) {
            out.push(CodeLink {
                symbol_id: s.symbol_id,
                path: s.path.clone(),
                name: s.name.clone(),
                line: s.line,
                via: Via::Mention,
            });
        }
    }
    out.sort_by(|x, y| (&x.path, &x.name).cmp(&(&y.path, &y.name)));
    Ok(out)
}

pub fn implemented_by(
    conn: &Connection,
    index: &CodeIndex,
    step: &StepRow,
    systems: &[(i64, String)],
    limit: usize,
) -> Result<(Vec<CodeLink>, usize)> {
    let mut links = section_mentions(conn, index, step.symbol_id)?;
    let mut by_system: Vec<(&CodeSym, String)> = Vec::new();
    for (_, name) in systems {
        let norm = normalise_ident(&norm_name(name));
        if norm.chars().count() < MIN_SYSTEM_CHARS {
            continue;
        }
        for s in &index.syms {
            if (s.norm_name.contains(&norm) || s.norm_stem.contains(&norm))
                && !links.iter().any(|l| l.symbol_id == s.symbol_id)
                && !by_system.iter().any(|(x, _)| x.symbol_id == s.symbol_id)
            {
                by_system.push((s, name.clone()));
            }
        }
    }
    // Ranked by incoming-reference count, then path, then name — the spec's explicit tie-break.
    by_system.sort_by_key(|(s, _)| {
        (
            std::cmp::Reverse(s.referrers),
            s.path.clone(),
            s.name.clone(),
        )
    });
    links.extend(by_system.into_iter().map(|(s, sys)| CodeLink {
        symbol_id: s.symbol_id,
        path: s.path.clone(),
        name: s.name.clone(),
        line: s.line,
        via: Via::System(sys),
    }));
    let more = links.len().saturating_sub(limit);
    links.truncate(limit);
    Ok((links, more))
}

/// One step of a process as the `entities` tool and the journeys read model show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepView {
    /// 1-based, renumbered across all the process's sections in document order.
    pub ordinal: usize,
    pub text: String,
    /// The role entity's name, else the extracted role text; empty when none.
    pub role: String,
    pub systems: Vec<String>,
    /// Entity ids parallel to `systems` (same order — both come from `systems_for_step`).
    pub system_ids: Vec<i64>,
    pub section: crate::knowledge::CitedSection,
    /// At most `LINK_LIMIT` "implemented by" links.
    pub code: Vec<CodeLink>,
    /// Links beyond `code` that were cut by `LINK_LIMIT`.
    pub more_code: usize,
    /// The underlying `steps` row id (for callers, e.g. the serve DTO, that need it).
    pub step_id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessSteps {
    pub steps: Vec<StepView>,
}

/// The process's steps in document order, each with its role, systems, documenting
/// section and implementing code.
pub fn process_steps(
    conn: &Connection,
    index: &CodeIndex,
    process_id: i64,
) -> Result<ProcessSteps> {
    let mut steps = Vec::new();
    for (i, row) in steps_for_process(conn, process_id)?.into_iter().enumerate() {
        let systems = systems_for_step(conn, row.id)?;
        let (code, more_code) = implemented_by(conn, index, &row, &systems, LINK_LIMIT)?;
        let section = crate::knowledge::cited_section(conn, row.symbol_id)?.ok_or_else(|| {
            crate::Error::Config(format!(
                "step {} cites a missing section {}",
                row.id, row.symbol_id
            ))
        })?;
        let step_id = row.id;
        let role = role_name(conn, &row)?;
        steps.push(StepView {
            ordinal: i + 1,
            role,
            text: row.text,
            system_ids: systems.iter().map(|(id, _)| *id).collect(),
            systems: systems.into_iter().map(|(_, n)| n).collect(),
            section,
            code,
            more_code,
            step_id,
        });
    }
    Ok(ProcessSteps { steps })
}

/// One line per step: `  N. text — role: r — systems: a, b — path::heading` then
/// ` — code: path::name (+M)` for the first link, `M` counting the rest.
pub fn render_steps(p: &ProcessSteps, out: &mut String) {
    for s in &p.steps {
        out.push_str(&format!("  {}. {}", s.ordinal, s.text));
        if !s.role.is_empty() {
            out.push_str(&format!(" — role: {}", s.role));
        }
        if !s.systems.is_empty() {
            out.push_str(&format!(" — systems: {}", s.systems.join(", ")));
        }
        out.push_str(&format!(" — {}::{}", s.section.path, s.section.name));
        if let Some(first) = s.code.first() {
            let rest = s.code.len() - 1 + s.more_code;
            out.push_str(&format!(" — code: {}::{}", first.path, first.name));
            if rest > 0 {
                out.push_str(&format!(" (+{rest})"));
            }
        }
        out.push('\n');
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
        seed_section_range(conn, path, name, line, line)
    }

    fn seed_section_range(
        conn: &Connection,
        path: &str,
        name: &str,
        line: i64,
        line_end: i64,
    ) -> i64 {
        conn.execute(
            "INSERT INTO files(path, lang, content_hash, mtime_ms, size, indexed_at_ms, skipped_reason) VALUES (?1, NULL, 'h', 0, 1, 0, NULL) ON CONFLICT(path) DO NOTHING",
            [path],
        )
        .unwrap();
        let file_id: i64 = conn
            .query_row("SELECT id FROM files WHERE path = ?1", [path], |r| r.get(0))
            .unwrap();
        conn.execute(
            "INSERT INTO symbols(file_id, name, kind, line_start, line_end, signature) VALUES (?1, ?2, 'section', ?3, ?4, '')",
            params![file_id, name, line, line_end],
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

    fn seed_code(conn: &Connection, path: &str, name: &str, kind: &str, line: i64) -> i64 {
        conn.execute(
            "INSERT INTO files(path, lang, content_hash, mtime_ms, size, indexed_at_ms, skipped_reason) VALUES (?1, 'typescript', 'h', 0, 1, 0, NULL) ON CONFLICT(path) DO NOTHING",
            [path],
        )
        .unwrap();
        let file_id: i64 = conn
            .query_row("SELECT id FROM files WHERE path = ?1", [path], |r| r.get(0))
            .unwrap();
        conn.execute(
            "INSERT INTO symbols(file_id, name, kind, line_start, line_end, signature) VALUES (?1, ?2, ?3, ?4, ?4, '')",
            params![file_id, name, kind, line],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn seed_ref(conn: &Connection, from_path: &str, name: &str, line: i64) {
        conn.execute(
            "INSERT INTO files(path, lang, content_hash, mtime_ms, size, indexed_at_ms, skipped_reason) VALUES (?1, 'typescript', 'h', 0, 1, 0, NULL) ON CONFLICT(path) DO NOTHING",
            [from_path],
        )
        .unwrap();
        let file_id: i64 = conn
            .query_row("SELECT id FROM files WHERE path = ?1", [from_path], |r| {
                r.get(0)
            })
            .unwrap();
        conn.execute(
            "INSERT INTO refs(file_id, name, line) VALUES (?1, ?2, ?3)",
            params![file_id, name, line],
        )
        .unwrap();
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

    #[test]
    fn a_sections_own_code_mentions_come_first_then_system_name_matches() {
        let (_dir, store) = temp_store();
        let conn = store.conn();
        let sec = seed_section_range(conn, "docs/handbook.md", "Expense process", 10, 20);
        let approve = seed_code(
            conn,
            "src/payroll/expense.ts",
            "approveClaim",
            "function",
            3,
        );
        seed_ref(conn, "docs/handbook.md", "approveClaim", 12);
        let client = seed_code(
            conn,
            "src/vendors/expensify-client.ts",
            "postClaim",
            "function",
            1,
        );
        let cfg = seed_code(
            conn,
            "config/app.json",
            "integrations.expensify.token",
            "key",
            4,
        );
        let _unrelated = seed_code(conn, "src/auth/okta.ts", "oktaLogin", "function", 1);
        let p = seed_entity(conn, sec, "Expense process", "process");
        let expensify = seed_entity(conn, sec, "Expensify", "system");
        apply_steps(
            conn,
            sec,
            "h",
            &StepsAnswer {
                process: "Expense process".into(),
                steps: vec![ExtractedStep {
                    text: "Submit".into(),
                    systems: vec!["Expensify".into()],
                    ..Default::default()
                }],
            },
        )
        .unwrap();
        let step = &steps_for_process(conn, p).unwrap()[0];
        let index = load_code_index(conn).unwrap();
        let (links, more) = implemented_by(
            conn,
            &index,
            step,
            &[(expensify, "Expensify".into())],
            LINK_LIMIT,
        )
        .unwrap();
        assert_eq!(
            links.iter().map(|l| l.symbol_id).collect::<Vec<_>>(),
            vec![approve, cfg, client],
            "system matches tie-break on path: config/app.json before src/vendors/..."
        );
        assert!(matches!(links[0].via, Via::Mention));
        assert!(matches!(&links[1].via, Via::System(s) if s == "Expensify"));
        assert_eq!(more, 0);
        let (two, rest) =
            implemented_by(conn, &index, step, &[(expensify, "Expensify".into())], 2).unwrap();
        assert_eq!((two.len(), rest), (2, 1));
    }

    #[test]
    fn matching_ignores_case_hyphens_and_underscores_but_not_document_kinds() {
        let (_dir, store) = temp_store();
        let conn = store.conn();
        let sec = seed_section(conn, "docs/handbook.md", "Incidents", 5);
        let page_fn = seed_code(
            conn,
            "src/incidents/page.ts",
            "pager_duty_page",
            "function",
            2,
        );
        let hook_fn = seed_code(conn, "src/lib/PagerDuty-hooks.ts", "hooks", "function", 1);
        let runbook_sec = seed_section(conn, "docs/pagerduty.md", "PagerDuty runbook", 1);
        let p = seed_entity(conn, sec, "Incidents", "process");
        let pagerduty = seed_entity(conn, sec, "PagerDuty", "system");
        apply_steps(
            conn,
            sec,
            "h",
            &StepsAnswer {
                process: "Incidents".into(),
                steps: vec![ExtractedStep {
                    text: "Page on-call".into(),
                    systems: vec!["PagerDuty".into()],
                    ..Default::default()
                }],
            },
        )
        .unwrap();
        let step = &steps_for_process(conn, p).unwrap()[0];
        let index = load_code_index(conn).unwrap();
        let (links, more) = implemented_by(
            conn,
            &index,
            step,
            &[(pagerduty, "PagerDuty".into())],
            LINK_LIMIT,
        )
        .unwrap();
        let ids: Vec<i64> = links.iter().map(|l| l.symbol_id).collect();
        assert!(
            ids.contains(&page_fn),
            "norm_name match should link: {ids:?}"
        );
        assert!(
            ids.contains(&hook_fn),
            "file-stem match should link: {ids:?}"
        );
        assert!(
            !ids.contains(&runbook_sec),
            "section kind must never appear as a code link: {ids:?}"
        );
        assert_eq!(links.len(), 2);
        assert_eq!(more, 0);
    }

    #[test]
    fn a_two_character_system_matches_no_code() {
        let (_dir, store) = temp_store();
        let conn = store.conn();
        let sec = seed_section(conn, "docs/handbook.md", "IT process", 5);
        let _item = seed_code(conn, "src/inventory/item.ts", "item", "function", 1);
        let _commit = seed_code(conn, "src/git/commit.ts", "commit", "function", 1);
        let p = seed_entity(conn, sec, "IT process", "process");
        let it = seed_entity(conn, sec, "IT", "system");
        apply_steps(
            conn,
            sec,
            "h",
            &StepsAnswer {
                process: "IT process".into(),
                steps: vec![ExtractedStep {
                    text: "Open ticket".into(),
                    systems: vec!["IT".into()],
                    ..Default::default()
                }],
            },
        )
        .unwrap();
        let step = &steps_for_process(conn, p).unwrap()[0];
        let index = load_code_index(conn).unwrap();
        let (links, more) =
            implemented_by(conn, &index, step, &[(it, "IT".into())], LINK_LIMIT).unwrap();
        assert!(
            links.is_empty(),
            "2-char system must match no code: {links:?}"
        );
        assert_eq!(more, 0);
    }

    #[test]
    fn system_matches_rank_by_incoming_references_then_path() {
        let (_dir, store) = temp_store();
        let conn = store.conn();
        let sec = seed_section(conn, "docs/handbook.md", "Billing process", 5);
        let popular = seed_code(
            conn,
            "src/services/stripe_billing.ts",
            "stripeBilling",
            "function",
            5,
        );
        let quiet = seed_code(conn, "src/legacy/stripe_old.ts", "stripeOld", "function", 5);
        seed_ref(conn, "src/a.ts", "stripeBilling", 1);
        seed_ref(conn, "src/b.ts", "stripeBilling", 2);
        let p = seed_entity(conn, sec, "Billing process", "process");
        let stripe = seed_entity(conn, sec, "Stripe", "system");
        apply_steps(
            conn,
            sec,
            "h",
            &StepsAnswer {
                process: "Billing process".into(),
                steps: vec![ExtractedStep {
                    text: "Charge card".into(),
                    systems: vec!["Stripe".into()],
                    ..Default::default()
                }],
            },
        )
        .unwrap();
        let step = &steps_for_process(conn, p).unwrap()[0];
        let index = load_code_index(conn).unwrap();
        let (links, more) =
            implemented_by(conn, &index, step, &[(stripe, "Stripe".into())], LINK_LIMIT).unwrap();
        assert_eq!(
            links.iter().map(|l| l.symbol_id).collect::<Vec<_>>(),
            vec![popular, quiet],
            "higher incoming-reference count sorts first"
        );
        assert_eq!(more, 0);

        // Explicit path tie-break: two symbols with equal (zero) referrers, seeded in reverse
        // path order ("b.ts" before "a.ts") so insertion order alone would get this wrong.
        let widget_b = seed_code(conn, "b.ts", "acmeWidgetB", "function", 1);
        let widget_a = seed_code(conn, "a.ts", "acmeWidgetA", "function", 1);
        let acme = seed_entity(conn, sec, "Acme", "system");
        let index2 = load_code_index(conn).unwrap();
        let (tie_links, tie_more) =
            implemented_by(conn, &index2, step, &[(acme, "Acme".into())], LINK_LIMIT).unwrap();
        assert_eq!(
            tie_links.iter().map(|l| l.symbol_id).collect::<Vec<_>>(),
            vec![widget_a, widget_b],
            "equal referrers tie-break ascending by path: a.ts before b.ts"
        );
        assert_eq!(tie_more, 0);
    }

    fn sect(path: &str, name: &str, a: u32, b: u32) -> crate::knowledge::CitedSection {
        crate::knowledge::CitedSection {
            symbol_id: 1,
            path: path.into(),
            name: name.into(),
            line_start: a,
            line_end: b,
        }
    }

    fn link(path: &str, name: &str, line: u32) -> CodeLink {
        CodeLink {
            symbol_id: line as i64,
            path: path.into(),
            name: name.into(),
            line,
            via: Via::Mention,
        }
    }

    #[test]
    fn render_steps_prints_role_systems_section_and_the_first_code_link_with_a_count() {
        let p = ProcessSteps {
            steps: vec![
                StepView {
                    ordinal: 1,
                    text: "Submit in Expensify".into(),
                    role: "employee".into(),
                    systems: vec!["Expensify".into()],
                    system_ids: vec![1],
                    section: sect("docs/handbook.md", "Expense process", 10, 20),
                    code: vec![],
                    more_code: 0,
                    step_id: 1,
                },
                StepView {
                    ordinal: 2,
                    text: "Finance reviews".into(),
                    role: "".into(),
                    systems: vec![],
                    system_ids: vec![],
                    section: sect("docs/handbook.md", "Expense process", 10, 20),
                    code: vec![
                        link("src/payroll/expense.ts", "approveClaim", 3),
                        link("src/payroll/expense.ts", "review", 9),
                    ],
                    more_code: 1,
                    step_id: 2,
                },
            ],
        };
        let mut out = String::new();
        render_steps(&p, &mut out);
        assert_eq!(out, "  1. Submit in Expensify — role: employee — systems: Expensify — docs/handbook.md::Expense process\n  2. Finance reviews — docs/handbook.md::Expense process — code: src/payroll/expense.ts::approveClaim (+2)\n");
    }
}
