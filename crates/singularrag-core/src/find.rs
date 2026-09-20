//! `find_symbol`: FTS5 lookup by name (exact, prefix, split tokens) with reference counts.

use rusqlite::params;

use crate::config::MapConfig;
use crate::rank::RefBy;
use crate::store::Store;
use crate::Result;

pub const MAX_LIMIT: usize = 50;

#[derive(Debug, Clone)]
pub struct FindHit {
    pub symbol_id: i64,
    pub path: String,
    pub line: u32,
    pub kind: String,
    pub name: String,
    pub signature: String,
    pub referenced_from: Vec<RefBy>,
    pub total_ref_files: usize,
    /// References from the defining file itself. Counted separately because a symbol
    /// used only inside its own file is used, and the "referenced from N files" line
    /// would otherwise read as zero.
    pub in_file_refs: i64,
}

pub fn find_symbol(
    store: &Store,
    config: &MapConfig,
    name: &str,
    kind: Option<&str>,
    limit: usize,
) -> Result<Vec<FindHit>> {
    let limit = limit.clamp(1, MAX_LIMIT + crate::map::CUT_RECORDED);
    let conn = store.conn();
    let term = name.replace('"', "\"\"");
    let parts = crate::tokens::split_identifier(name);
    // Exact name first, then prefix, then all split parts (only when there are any).
    let q = if parts.is_empty() {
        format!("{{name name_tokens}}: (\"{term}\" OR \"{term}\"*)")
    } else {
        let and_parts = parts
            .iter()
            .map(|p| format!("\"{p}\""))
            .collect::<Vec<_>>()
            .join(" AND ");
        format!("{{name name_tokens}}: (\"{term}\" OR \"{term}\"* OR ({and_parts}))")
    };
    let mut stmt = conn.prepare(
        "SELECT s.id, f.path, s.line_start, s.kind, s.name, s.signature, f.id
         FROM symbols_fts JOIN symbols s ON s.id = symbols_fts.rowid JOIN files f ON f.id = s.file_id
         WHERE symbols_fts MATCH ?1
         ORDER BY (lower(s.name) = lower(?2)) DESC, bm25(symbols_fts), f.path, s.line_start",
    )?;
    let rows = stmt.query_map(params![q, name], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, u32>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, i64>(6)?,
        ))
    })?;

    let mut rs = conn.prepare(
        "SELECT f.path, COUNT(*) FROM refs r JOIN files f ON f.id = r.file_id
         WHERE r.name = ?1 AND r.file_id != ?2 GROUP BY f.path ORDER BY COUNT(*) DESC, f.path",
    )?;
    let mut own = conn.prepare("SELECT COUNT(*) FROM refs WHERE name = ?1 AND file_id = ?2")?;

    let mut hits = Vec::new();
    for row in rows {
        let (symbol_id, path, line, k, sym_name, signature, file_id) = row?;
        if config.is_excluded(&path) {
            continue;
        }
        if kind.is_some_and(|want| want != k) {
            continue;
        }
        let all: Vec<RefBy> = rs
            .query_map(params![sym_name, file_id], |r| {
                Ok(RefBy {
                    path: r.get(0)?,
                    count: r.get(1)?,
                })
            })?
            .collect::<std::result::Result<_, _>>()?;
        // Excluded files must never appear in reference lists or counts (graph.rs never lets them in either).
        let all: Vec<RefBy> = all
            .into_iter()
            .filter(|r| !config.is_excluded(&r.path))
            .collect();
        let total_ref_files = all.len();
        let mut referenced_from = all;
        referenced_from.truncate(5);
        let in_file_refs: i64 = own.query_row(params![sym_name, file_id], |r| r.get(0))?;
        hits.push(FindHit {
            symbol_id,
            path,
            line,
            kind: k,
            name: sym_name,
            signature,
            referenced_from,
            total_ref_files,
            in_file_refs,
        });
        if hits.len() >= limit {
            break;
        }
    }
    Ok(hits)
}

pub fn render_find(hits: &[FindHit]) -> String {
    if hits.is_empty() {
        return "no symbols matched\n".to_string();
    }
    let mut out = String::new();
    for h in hits {
        out.push_str(&format!(
            "{}:{}  {}  {}\n",
            h.path, h.line, h.kind, h.signature
        ));
        let in_file = if h.in_file_refs > 0 {
            format!(", {} in this file", h.in_file_refs)
        } else {
            String::new()
        };
        if h.total_ref_files == 0 {
            out.push_str(&format!("   referenced from 0 files{in_file}\n"));
        } else {
            let list = h
                .referenced_from
                .iter()
                .map(|r| format!("{} ({})", r.path, r.count))
                .collect::<Vec<_>>()
                .join(", ");
            let more = if h.total_ref_files > h.referenced_from.len() {
                ", …"
            } else {
                ""
            };
            out.push_str(&format!(
                "   referenced from {} files: {list}{more}{in_file}\n",
                h.total_ref_files
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MapConfig;
    use crate::engine::{Engine, FindRequest};
    use crate::fixture::write_ts_mini;

    fn engine() -> (tempfile::TempDir, Engine) {
        let dir = tempfile::tempdir().unwrap();
        write_ts_mini(dir.path());
        let mut e = Engine::open(dir.path(), "t").unwrap();
        e.refresh(std::time::Duration::from_secs(5)).unwrap();
        (dir, e)
    }

    #[test]
    fn exact_and_prefix_lookup_with_references() {
        let (_dir, e) = engine();
        let hits = find_symbol(e.store(), &MapConfig::default(), "createSess", None, 10).unwrap();
        assert_eq!(hits[0].name, "createSession");
        assert_eq!(hits[0].path, "src/auth/session.ts");
        assert_eq!(hits[0].line, 3);
        assert_eq!(hits[0].kind, "function");
        assert_eq!(hits[0].total_ref_files, 2);
        assert_eq!(hits[0].referenced_from[0].path, "src/http/middleware.ts");
        assert_eq!(hits[0].referenced_from[0].count, 2);
    }

    #[test]
    fn excluded_files_are_dropped_from_references() {
        let (_dir, e) = engine();
        let cfg = MapConfig::parse("[[exclude]]\npath = \"src/cli/\"\n").unwrap();
        let hits = find_symbol(e.store(), &cfg, "createSession", None, 10).unwrap();
        assert_eq!(hits[0].total_ref_files, 1);
        assert!(hits[0]
            .referenced_from
            .iter()
            .all(|r| !r.path.starts_with("src/cli/")));
    }

    /// `r.file_id != ?2` hides references from the defining file, so a symbol used only
    /// inside its own file used to read "referenced from 0 files" with nothing else said.
    #[test]
    fn in_file_references_are_counted_and_rendered() {
        let (_dir, e) = engine();
        let hits = find_symbol(e.store(), &MapConfig::default(), "SessionStore", None, 10).unwrap();
        assert_eq!(hits[0].name, "SessionStore");
        assert_eq!(hits[0].total_ref_files, 0);
        assert_eq!(
            hits[0].in_file_refs, 1,
            "new SessionStore() in its own file"
        );
        assert!(
            render_find(&hits[..1]).contains("   referenced from 0 files, 1 in this file\n"),
            "{}",
            render_find(&hits[..1])
        );
    }

    #[test]
    fn split_token_lookup_and_kind_filter() {
        let (_dir, e) = engine();
        let hits = find_symbol(
            e.store(),
            &MapConfig::default(),
            "session",
            Some("class"),
            10,
        )
        .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "SessionStore");
        let none = find_symbol(e.store(), &MapConfig::default(), "nope_zzz", None, 10).unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn render_format() {
        let (_dir, e) = engine();
        let hits = find_symbol(e.store(), &MapConfig::default(), "createSession", None, 1).unwrap();
        let text = render_find(&hits);
        assert!(text.starts_with("src/auth/session.ts:3  function  export function createSession(user: User, ttl: number): Session\n   referenced from 2 files: src/http/middleware.ts (2), src/cli/login.ts (1)\n"), "{text}");
        assert_eq!(render_find(&[]), "no symbols matched\n");
    }

    #[test]
    fn engine_find_records_retrieval_with_header() {
        let (_dir, mut e) = engine();
        let resp = e
            .find_symbol(&FindRequest {
                name: "log".into(),
                kind: None,
                limit: 10,
            })
            .unwrap();
        assert!(resp.text.starts_with("# singularrag · index "));
        assert!(resp
            .text
            .contains("src/util/log.ts:1  function  export function log(msg: string): void"));
        let tool: String = e
            .store()
            .conn()
            .query_row(
                "SELECT tool FROM retrievals ORDER BY id DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tool, "find_symbol");
        let (budget, limit_n): (Option<i64>, Option<i64>) = e
            .store()
            .conn()
            .query_row(
                "SELECT budget, limit_n FROM retrievals ORDER BY id DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((budget, limit_n), (None, Some(10)));
    }

    /// `find` shares `repo_map`'s cut semantics: the candidates below the limit are
    /// recorded as `served = 0` instead of everything being recorded as served.
    #[test]
    fn candidates_below_the_limit_are_recorded_as_cut() {
        let (_dir, mut e) = engine();
        let resp = e
            .find_symbol(&FindRequest {
                name: "session".into(),
                kind: None,
                limit: 1,
            })
            .unwrap();
        assert_eq!(resp.hits, 1);
        assert_eq!(resp.text.matches("   referenced from").count(), 1);
        let counts = |served: i64| -> i64 {
            e.store()
                .conn()
                .query_row(
                    "SELECT COUNT(*) FROM retrieval_items WHERE retrieval_id = ?1 AND served = ?2",
                    rusqlite::params![resp.retrieval_id, served],
                    |r| r.get(0),
                )
                .unwrap()
        };
        assert_eq!(counts(1), 1);
        assert!(counts(0) > 0, "cut candidates must be recorded");
        assert!(counts(0) <= crate::map::CUT_RECORDED as i64);
    }
}
