//! Read-only SQL for the UI. Every function takes the read connection and returns
//! serialisable DTOs; nothing here writes.

use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use singularrag_core::journeys;
use singularrag_core::store::Store;
use singularrag_core::workspace::Workspace;
use singularrag_core::Result;

use super::state::Freshness;

#[derive(Debug, Clone, Default, Serialize)]
pub struct FileCounts {
    pub indexed: i64,
    pub skipped: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RootDto {
    pub name: String,
    pub path: String,
    pub git_head: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct StatusDto {
    pub index_version: String,
    pub git_head: Option<String>,
    pub indexed_at_ms: Option<i64>,
    pub stale_count: usize,
    pub lock_timeout: bool,
    pub foreign_indexing: bool,
    pub indexing: bool,
    pub files: FileCounts,
    pub roots: Vec<RootDto>,
    /// Sections waiting for the knowledge tick.
    pub entities_pending: usize,
    /// The last knowledge tick hit a model outage.
    pub models_unavailable: bool,
    /// A dimension change is re-embedding every section.
    pub embeddings_rebuilding: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RetrievalSummary {
    pub id: i64,
    pub session_key: String,
    pub session_label: String,
    pub tool: String,
    pub query: Option<String>,
    pub focus_files: Vec<String>,
    pub budget: Option<i64>,
    pub limit_n: Option<i64>,
    pub index_version: String,
    pub git_head: Option<String>,
    pub stale_count: i64,
    pub created_at_ms: i64,
    pub served: i64,
    pub cut: i64,
}

#[derive(Debug, Serialize)]
pub struct ItemDto {
    pub rank: i64,
    pub symbol_id: i64,
    pub path: String,
    pub name: String,
    pub line_start: i64,
    pub score: f64,
    pub served: bool,
    pub reasons: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct RetrievalDetail {
    #[serde(flatten)]
    pub summary: RetrievalSummary,
    pub items: Vec<ItemDto>,
}

#[derive(Debug, Serialize)]
pub struct TreeSymbol {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub line_start: i64,
    pub line_end: i64,
    pub signature: String,
}

#[derive(Debug, Serialize)]
pub struct TreeFile {
    pub path: String,
    pub lang: Option<String>,
    pub skipped_reason: Option<String>,
    pub symbols: Vec<TreeSymbol>,
}

#[derive(Debug, Serialize)]
pub struct SkippedFile {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Serialize)]
pub struct GraphNode {
    pub path: String,
    pub symbols: usize,
    pub lang: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct GraphEdge {
    pub src: usize,
    pub dst: usize,
    pub weight: f64,
    pub names: usize,
}

#[derive(Debug, Serialize)]
pub struct GraphDto {
    pub index_version: String,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EntityDto {
    pub id: i64,
    pub name: String,
    pub r#type: String,
    pub description: String,
    pub mentions: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct EntityRelationDto {
    pub id: i64,
    pub src: i64,
    pub dst: i64,
    pub description: String,
    pub symbol_id: i64,
    pub path: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct EntityMentionDto {
    pub entity_id: i64,
    pub symbol_id: i64,
    pub path: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct EntitiesDto {
    pub entities: Vec<EntityDto>,
    pub relations: Vec<EntityRelationDto>,
    pub mentions: Vec<EntityMentionDto>,
    /// Set when any of the three caps below cut a list short.
    pub truncated: bool,
}

/// Spec §3: `mcp:<slug>:…` → title-cased slug; `cli-<pid>` → CLI; `serve` → UI; else raw.
pub fn session_label(key: &str) -> String {
    if let Some(rest) = key.strip_prefix("mcp:") {
        let slug = rest.split(':').next().unwrap_or("");
        return slug
            .split('-')
            .filter(|s| !s.is_empty())
            .map(|w| {
                let mut c = w.chars();
                match c.next() {
                    Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
    }
    if key.starts_with("cli-") {
        return "CLI".into();
    }
    if key == "serve" {
        return "UI".into();
    }
    key.to_string()
}

pub fn max_retrieval_id(store: &Store) -> Result<i64> {
    Ok(store
        .conn()
        .query_row("SELECT COALESCE(MAX(id), 0) FROM retrievals", [], |r| {
            r.get(0)
        })?)
}

pub fn status(store: &Store, f: &Freshness, ws: &Workspace) -> Result<StatusDto> {
    let conn = store.conn();
    let indexed: i64 = conn.query_row(
        "SELECT COUNT(*) FROM files WHERE skipped_reason IS NULL",
        [],
        |r| r.get(0),
    )?;
    let skipped: i64 = conn.query_row(
        "SELECT COUNT(*) FROM files WHERE skipped_reason IS NOT NULL",
        [],
        |r| r.get(0),
    )?;
    let mut roots = Vec::with_capacity(ws.roots.len());
    for r in &ws.roots {
        let key = if r.name.is_empty() {
            "git_head".to_string()
        } else {
            format!("git_head:{}", r.name)
        };
        roots.push(RootDto {
            name: r.name.clone(),
            path: r.path.display().to_string(),
            git_head: store.get_meta(&key)?.filter(|h| !h.is_empty()),
        });
    }
    let entities_pending = singularrag_core::knowledge::pending(store)?;
    Ok(StatusDto {
        index_version: store.get_meta("index_version")?.unwrap_or_default(),
        git_head: store.get_meta("git_head")?.filter(|h| !h.is_empty()),
        indexed_at_ms: store
            .get_meta("indexed_at_ms")?
            .and_then(|s| s.parse().ok()),
        stale_count: f.stale_count,
        lock_timeout: f.lock_timeout,
        foreign_indexing: f.foreign_indexing,
        indexing: f.indexing,
        files: FileCounts { indexed, skipped },
        roots,
        entities_pending,
        // Same rule as the header: an outage with nothing pending is not reported.
        models_unavailable: entities_pending > 0
            && store
                .get_meta("models_error")?
                .is_some_and(|e| !e.is_empty()),
        embeddings_rebuilding: store.get_meta("embeddings_rebuilding")?.as_deref() == Some("1"),
    })
}

const SUMMARY_SQL: &str = "SELECT r.id, r.session_key, r.tool, r.query, r.focus_files, r.budget, r.limit_n, r.index_version, r.git_head, r.stale_count, r.created_at_ms,
    (SELECT COUNT(*) FROM retrieval_items i WHERE i.retrieval_id = r.id AND i.served = 1),
    (SELECT COUNT(*) FROM retrieval_items i WHERE i.retrieval_id = r.id AND i.served = 0)
  FROM retrievals r";

fn summary_from_row(r: &rusqlite::Row) -> rusqlite::Result<RetrievalSummary> {
    let id: i64 = r.get(0)?;
    let key: String = r.get(1)?;
    let focus: String = r.get(4)?;
    // A row the writer produced should always parse; if one does not, the UI degrades to
    // an empty list rather than failing the request, so say so on stderr.
    let focus_files = serde_json::from_str(&focus).unwrap_or_else(|e| {
        tracing::warn!("retrieval {id}: focus_files is not valid JSON ({e}); reporting none");
        Vec::new()
    });
    Ok(RetrievalSummary {
        id,
        session_label: session_label(&key),
        session_key: key,
        tool: r.get(2)?,
        query: r.get(3)?,
        focus_files,
        budget: r.get(5)?,
        limit_n: r.get(6)?,
        index_version: r.get(7)?,
        git_head: r.get::<_, Option<String>>(8)?.filter(|h| !h.is_empty()),
        stale_count: r.get(9)?,
        created_at_ms: r.get(10)?,
        served: r.get(11)?,
        cut: r.get(12)?,
    })
}

pub fn retrievals(
    store: &Store,
    limit: usize,
    before: Option<i64>,
) -> Result<Vec<RetrievalSummary>> {
    let sql = format!("{SUMMARY_SQL} WHERE (?1 IS NULL OR r.id < ?1) ORDER BY r.id DESC LIMIT ?2");
    let mut stmt = store.conn().prepare(&sql)?;
    let rows = stmt.query_map(params![before, limit as i64], summary_from_row)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

pub fn retrieval(store: &Store, id: i64) -> Result<Option<RetrievalDetail>> {
    let sql = format!("{SUMMARY_SQL} WHERE r.id = ?1");
    let Some(summary) = store
        .conn()
        .query_row(&sql, [id], summary_from_row)
        .optional()?
    else {
        return Ok(None);
    };
    let mut stmt = store.conn().prepare(
        "SELECT rank, symbol_id, path, name, line_start, score, served, reasons_json FROM retrieval_items WHERE retrieval_id = ?1 ORDER BY rank",
    )?;
    let items = stmt
        .query_map([id], |r| {
            let rank: i64 = r.get(0)?;
            let raw: String = r.get(7)?;
            let reasons = serde_json::from_str(&raw).unwrap_or_else(|e| {
                tracing::warn!(
                    "retrieval {id} rank {rank}: reasons_json is not valid JSON ({e}); \
                     reporting null reasons"
                );
                serde_json::Value::Null
            });
            Ok(ItemDto {
                rank,
                symbol_id: r.get(1)?,
                path: r.get(2)?,
                name: r.get(3)?,
                line_start: r.get(4)?,
                score: r.get(5)?,
                served: r.get::<_, i64>(6)? == 1,
                reasons,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(Some(RetrievalDetail { summary, items }))
}

pub fn tree(store: &Store) -> Result<Vec<TreeFile>> {
    let conn = store.conn();
    let mut files = conn
        .prepare("SELECT id, path, lang FROM files WHERE skipped_reason IS NULL ORDER BY path")?;
    let mut syms = conn.prepare(
        "SELECT id, name, kind, line_start, line_end, signature FROM symbols WHERE file_id = ?1 ORDER BY line_start",
    )?;
    let mut out = Vec::new();
    for row in files.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
        ))
    })? {
        let (fid, path, lang) = row?;
        let symbols = syms
            .query_map([fid], |r| {
                Ok(TreeSymbol {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    kind: r.get(2)?,
                    line_start: r.get(3)?,
                    line_end: r.get(4)?,
                    signature: r.get(5)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        out.push(TreeFile {
            path,
            lang,
            skipped_reason: None,
            symbols,
        });
    }
    Ok(out)
}

pub fn skipped(store: &Store) -> Result<Vec<SkippedFile>> {
    let mut stmt = store
        .conn()
        .prepare("SELECT path, skipped_reason FROM files WHERE skipped_reason IS NOT NULL ORDER BY skipped_reason, path")?;
    let rows = stmt.query_map([], |r| {
        Ok(SkippedFile {
            path: r.get(0)?,
            reason: r.get(1)?,
        })
    })?;
    let mut out: Vec<SkippedFile> = rows.collect::<std::result::Result<_, _>>()?;
    out.extend(
        singularrag_core::knowledge::failed_sections(store)?
            .into_iter()
            .map(|(path, name, error)| SkippedFile {
                path: format!("{path}::{name}"),
                reason: format!("extraction failed: {error}"),
            }),
    );
    Ok(out)
}

/// The ranking's file graph, projected for drawing: excluded files absent, self-edges
/// dropped, one edge per ordered pair (spec §3).
pub fn graph(store: &Store, config: &singularrag_core::config::MapConfig) -> Result<GraphDto> {
    let g = singularrag_core::graph::build_graph(
        store,
        config,
        &[],
        &std::collections::HashSet::new(),
    )?;
    let conn = store.conn();
    let mut counts = std::collections::HashMap::<i64, usize>::new();
    let mut stmt = conn.prepare("SELECT file_id, COUNT(*) FROM symbols GROUP BY file_id")?;
    for row in stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))? {
        let (fid, n) = row?;
        counts.insert(fid, n as usize);
    }
    let mut langs = std::collections::HashMap::<i64, Option<String>>::new();
    let mut stmt = conn.prepare("SELECT id, lang FROM files")?;
    for row in stmt.query_map([], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?))
    })? {
        let (fid, l) = row?;
        langs.insert(fid, l);
    }
    let nodes = g
        .nodes
        .iter()
        .map(|n| GraphNode {
            path: n.path.clone(),
            symbols: counts.get(&n.id).copied().unwrap_or(0),
            lang: langs.get(&n.id).cloned().flatten(),
        })
        .collect();
    // (src, dst) -> (weight sum, distinct names)
    let mut agg = std::collections::BTreeMap::<
        (usize, usize),
        (f64, std::collections::BTreeSet<String>),
    >::new();
    for e in &g.edges {
        if e.src == e.dst {
            continue;
        }
        let entry = agg
            .entry((e.src, e.dst))
            .or_insert((0.0, Default::default()));
        entry.0 += e.weight;
        entry.1.insert(e.name.clone());
    }
    let edges = agg
        .into_iter()
        .map(|((src, dst), (weight, names))| GraphEdge {
            src,
            dst,
            weight,
            names: names.len(),
        })
        .collect();
    Ok(GraphDto {
        index_version: store.get_meta("index_version")?.unwrap_or_default(),
        nodes,
        edges,
    })
}

/// Caps for the knowledge overlay (task brief): entities ordered by `mentions` desc then
/// `name`, relations and mentions ordered by id so paging (if ever added) is stable. Each
/// cap is checked by asking for one row past the limit; three queries total, no N+1.
const ENTITY_CAP: i64 = 500;
const REL_MENTION_CAP: i64 = 2000;

pub fn entities(store: &Store) -> Result<EntitiesDto> {
    let conn = store.conn();
    let mut truncated = false;

    let mut stmt = conn.prepare(
        "SELECT id, name, type, description, mentions FROM entities ORDER BY mentions DESC, name LIMIT ?1",
    )?;
    let mut entities: Vec<EntityDto> = stmt
        .query_map([ENTITY_CAP + 1], |r| {
            Ok(EntityDto {
                id: r.get(0)?,
                name: r.get(1)?,
                r#type: r.get(2)?,
                description: r.get(3)?,
                mentions: r.get(4)?,
            })
        })?
        .collect::<std::result::Result<_, _>>()?;
    if entities.len() as i64 > ENTITY_CAP {
        entities.truncate(ENTITY_CAP as usize);
        truncated = true;
    }

    let mut stmt = conn.prepare(
        "SELECT r.id, r.src_entity, r.dst_entity, r.description, r.symbol_id, f.path, s.name
           FROM relations r
           JOIN symbols s ON s.id = r.symbol_id
           JOIN files f ON f.id = s.file_id
          ORDER BY r.id
          LIMIT ?1",
    )?;
    let mut relations: Vec<EntityRelationDto> = stmt
        .query_map([REL_MENTION_CAP + 1], |r| {
            Ok(EntityRelationDto {
                id: r.get(0)?,
                src: r.get(1)?,
                dst: r.get(2)?,
                description: r.get(3)?,
                symbol_id: r.get(4)?,
                path: r.get(5)?,
                name: r.get(6)?,
            })
        })?
        .collect::<std::result::Result<_, _>>()?;
    if relations.len() as i64 > REL_MENTION_CAP {
        relations.truncate(REL_MENTION_CAP as usize);
        truncated = true;
    }

    let mut stmt = conn.prepare(
        "SELECT m.entity_id, m.symbol_id, f.path, s.name
           FROM entity_mentions m
           JOIN symbols s ON s.id = m.symbol_id
           JOIN files f ON f.id = s.file_id
          ORDER BY m.entity_id, m.symbol_id
          LIMIT ?1",
    )?;
    let mut mentions: Vec<EntityMentionDto> = stmt
        .query_map([REL_MENTION_CAP + 1], |r| {
            Ok(EntityMentionDto {
                entity_id: r.get(0)?,
                symbol_id: r.get(1)?,
                path: r.get(2)?,
                name: r.get(3)?,
            })
        })?
        .collect::<std::result::Result<_, _>>()?;
    if mentions.len() as i64 > REL_MENTION_CAP {
        mentions.truncate(REL_MENTION_CAP as usize);
        truncated = true;
    }

    Ok(EntitiesDto {
        entities,
        relations,
        mentions,
        truncated,
    })
}

/// A `{id, name}` pair — the shape the UI wants for a step's systems, distinct from
/// `EntityDto` (which carries the full row).
#[derive(Debug, Clone, Serialize)]
pub struct EntityRefDto {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SectionRefDto {
    pub symbol_id: i64,
    pub path: String,
    pub name: String,
    pub line: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct CodeLinkDto {
    pub symbol_id: i64,
    pub path: String,
    pub name: String,
    pub line: u32,
    /// `"mention"` for a section's own code mentions, else `{"system": "<name>"}` for a
    /// system-name match.
    pub via: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct StepDto {
    pub ordinal: usize,
    pub text: String,
    pub role: String,
    pub systems: Vec<EntityRefDto>,
    pub section: SectionRefDto,
    pub code: Vec<CodeLinkDto>,
    pub more_code: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProcessDto {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub roles: Vec<String>,
    pub steps: Vec<StepDto>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProcessesDto {
    pub processes: Vec<ProcessDto>,
    pub truncated: bool,
}

const PROCESS_CAP: i64 = 200;
const STEP_CAP: usize = 30;

fn code_link_dto(c: journeys::CodeLink) -> CodeLinkDto {
    let via = match c.via {
        journeys::Via::Mention => serde_json::json!("mention"),
        journeys::Via::System(s) => serde_json::json!({ "system": s }),
    };
    CodeLinkDto {
        symbol_id: c.symbol_id,
        path: c.path,
        name: c.name,
        line: c.line,
        via,
    }
}

fn step_dto(s: journeys::StepView) -> StepDto {
    let systems = s
        .systems
        .into_iter()
        .zip(s.system_ids)
        .map(|(name, id)| EntityRefDto { id, name })
        .collect();
    StepDto {
        ordinal: s.ordinal,
        text: s.text,
        role: s.role,
        systems,
        section: SectionRefDto {
            symbol_id: s.section.symbol_id,
            path: s.section.path,
            name: s.section.name,
            line: s.section.line_start,
        },
        code: s.code.into_iter().map(code_link_dto).collect(),
        more_code: s.more_code,
    }
}

/// Every `process` entity (name order), each with its steps, the distinct non-empty
/// roles its steps use, and each step's derived "implemented by" code links.
pub fn processes(store: &Store) -> Result<ProcessesDto> {
    processes_with(store, journeys::load_code_index)
}

/// `processes` with the code index builder passed in (tests count its calls).
fn processes_with(
    store: &Store,
    build_index: impl FnOnce(&rusqlite::Connection) -> Result<journeys::CodeIndex>,
) -> Result<ProcessesDto> {
    let conn = store.conn();
    // Built on the first process only: an index without processes never scans the code.
    let mut build_index = Some(build_index);
    let mut index: Option<journeys::CodeIndex> = None;
    let mut stmt = conn.prepare(
        "SELECT id, name, description FROM entities WHERE type = 'process' ORDER BY name LIMIT ?1",
    )?;
    let rows: Vec<(i64, String, String)> = stmt
        .query_map([PROCESS_CAP + 1], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<std::result::Result<_, _>>()?;
    let truncated = rows.len() as i64 > PROCESS_CAP;
    let mut processes = Vec::new();
    for (id, name, description) in rows.into_iter().take(PROCESS_CAP as usize) {
        if let Some(build) = build_index.take() {
            index = Some(build(conn)?);
        }
        let p = journeys::process_steps(conn, index.as_ref().expect("built above"), id)?;
        let mut roles: Vec<String> = p
            .steps
            .iter()
            .map(|s| s.role.clone())
            .filter(|r| !r.is_empty())
            .collect();
        roles.sort();
        roles.dedup();
        let steps = p.steps.into_iter().take(STEP_CAP).map(step_dto).collect();
        processes.push(ProcessDto {
            id,
            name,
            description,
            roles,
            steps,
        });
    }
    Ok(ProcessesDto {
        processes,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use singularrag_core::engine::{Engine, FindRequest, MapRequest};
    use singularrag_core::fixture::write_ts_mini;
    use singularrag_core::store::Store;

    #[test]
    fn processes_on_an_index_without_processes_builds_no_code_index() {
        let (_dir, store) = seeded();
        let dto = processes_with(&store, |_| panic!("the code index was built")).unwrap();
        assert!(dto.processes.is_empty() && !dto.truncated);
    }

    fn seeded() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        write_ts_mini(dir.path());
        let mut e = Engine::open(dir.path(), "mcp:claude-code:1:2").unwrap();
        e.repo_map(&MapRequest {
            query: Some("session".into()),
            focus_files: vec![],
            // MIN_BUDGET (64): the smallest legal budget, chosen so the 9-symbol
            // ts_mini fixture is guaranteed to cut something (at 256 tokens, every
            // symbol fits and nothing is cut, which the assertions below require).
            budget_tokens: 64,
            ..Default::default()
        })
        .unwrap();
        e.find_symbol(&FindRequest {
            name: "log".into(),
            kind: None,
            limit: 5,
        })
        .unwrap();
        drop(e);
        let ro = Store::open_read_only(&dir.path().join(".singularrag/index.db")).unwrap();
        (dir, ro)
    }

    #[test]
    fn labels_sessions() {
        assert_eq!(session_label("mcp:claude-code:123:456"), "Claude Code");
        assert_eq!(session_label("mcp:codex:1:2"), "Codex");
        assert_eq!(session_label("cli-999"), "CLI");
        assert_eq!(session_label("serve"), "UI");
        assert_eq!(session_label("something-else"), "something-else");
    }

    #[test]
    fn retrievals_newest_first_with_counts_and_paging() {
        let (_d, ro) = seeded();
        let all = retrievals(&ro, 50, None).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].tool, "find_symbol");
        assert_eq!(all[0].limit_n, Some(5));
        assert_eq!(all[1].tool, "repo_map");
        assert_eq!(all[1].budget, Some(64));
        assert_eq!(all[1].session_label, "Claude Code");
        assert!(all[1].served > 0);
        assert!(
            all[1].served + all[1].cut > all[1].served,
            "a 64-token map must cut something"
        );
        let page = retrievals(&ro, 50, Some(all[0].id)).unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].id, all[1].id);
        assert_eq!(max_retrieval_id(&ro).unwrap(), all[0].id);
    }

    #[test]
    fn retrieval_detail_parses_reasons() {
        let (_d, ro) = seeded();
        let id = retrievals(&ro, 50, None).unwrap()[1].id;
        let d = retrieval(&ro, id).unwrap().unwrap();
        assert_eq!(d.summary.tool, "repo_map");
        assert!(!d.items.is_empty());
        let first = &d.items[0];
        assert_eq!(first.rank, 1);
        assert!(first.served);
        assert!(
            first.reasons.get("referenced_by").is_some(),
            "{:?}",
            first.reasons
        );
        assert!(d.items.iter().any(|i| !i.served));
        assert!(retrieval(&ro, 999_999).unwrap().is_none());
    }

    #[test]
    fn tree_and_skipped() {
        let (_d, ro) = seeded();
        let t = tree(&ro).unwrap();
        let session = t.iter().find(|f| f.path == "src/auth/session.ts").unwrap();
        assert_eq!(session.lang.as_deref(), Some("typescript"));
        assert!(session
            .symbols
            .iter()
            .any(|s| s.name == "createSession" && s.kind == "function"));
        assert!(
            t.iter().all(|f| f.skipped_reason.is_none()),
            "tree lists indexed files only"
        );
        let sk = skipped(&ro).unwrap();
        assert!(sk
            .iter()
            .any(|s| s.path == ".env" && s.reason == "denylisted"));
        assert!(sk
            .iter()
            .any(|s| s.path == "src/config.ts" && s.reason == "secret-like content"));
    }

    #[test]
    fn status_merges_meta_and_freshness() {
        let (d, ro) = seeded();
        let f = crate::serve::state::Freshness {
            stale_count: 3,
            foreign_indexing: true,
            ..Default::default()
        };
        let ws = Workspace::single(d.path()).unwrap();
        let s = status(&ro, &f, &ws).unwrap();
        assert_eq!(s.index_version.len(), 12);
        assert_eq!(s.stale_count, 3);
        assert!(s.foreign_indexing);
        assert!(s.files.indexed >= 4);
        assert!(s.files.skipped >= 2);
    }

    #[test]
    fn status_lists_the_roots() {
        let d = tempfile::tempdir().unwrap();
        singularrag_core::fixture::write_workspace(d.path());
        let mut e = singularrag_core::engine::Engine::open(d.path(), "t").unwrap();
        e.refresh(std::time::Duration::from_secs(60)).unwrap();
        let ws = e.workspace().clone();
        let s = status(e.store(), &crate::serve::state::Freshness::default(), &ws).unwrap();
        assert_eq!(
            s.roots.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            vec!["app", "notes"]
        );
        assert!(s.roots[0].git_head.is_none());
    }

    #[test]
    fn status_reports_the_knowledge_queue() {
        let d = tempfile::tempdir().unwrap();
        singularrag_core::fixture::write_docs_mini(d.path());
        let mut e = singularrag_core::engine::Engine::open(d.path(), "t").unwrap();
        e.refresh(std::time::Duration::from_secs(60)).unwrap();
        let ws = e.workspace().clone();
        let f = crate::serve::state::Freshness::default();
        let s = status(e.store(), &f, &ws).unwrap();
        assert!(s.entities_pending > 0);
        assert!(!s.models_unavailable && !s.embeddings_rebuilding);
        e.store()
            .set_meta("models_error", "connection refused")
            .unwrap();
        e.store().set_meta("embeddings_rebuilding", "1").unwrap();
        let s = status(e.store(), &f, &ws).unwrap();
        assert!(s.models_unavailable && s.embeddings_rebuilding);
    }

    #[test]
    fn graph_projects_files_and_aggregated_edges() {
        let (_d, store) = seeded();
        let g = graph(&store, &singularrag_core::config::MapConfig::default()).unwrap();
        assert!(!g.index_version.is_empty());
        let paths: Vec<&str> = g.nodes.iter().map(|n| n.path.as_str()).collect();
        // Same files as the tree (non-skipped; `src/config.ts` is skipped by the secret scan), in path order.
        assert_eq!(g.nodes.len(), tree(&store).unwrap().len());
        assert!(
            paths.windows(2).all(|w| w[0] < w[1]),
            "path order: {paths:?}"
        );
        for p in [
            "src/auth/session.ts",
            "src/cli/login.ts",
            "src/http/middleware.ts",
            "src/util/log.ts",
        ] {
            assert!(paths.contains(&p), "{p} missing from {paths:?}");
        }
        let session = paths
            .iter()
            .position(|p| *p == "src/auth/session.ts")
            .unwrap();
        let middleware = paths
            .iter()
            .position(|p| *p == "src/http/middleware.ts")
            .unwrap();
        assert!(g.nodes[session].symbols >= 4);
        assert_eq!(g.nodes[session].lang.as_deref(), Some("typescript"));
        let e = g
            .edges
            .iter()
            .find(|e| e.src == middleware && e.dst == session)
            .expect("middleware -> session edge");
        assert!(e.weight > 0.0);
        assert_eq!(
            e.names, 2,
            "createSession and Session are distinct names behind one edge"
        );
        assert!(g.edges.iter().all(|e| e.src != e.dst), "self-edges dropped");
        assert_eq!(
            g.edges
                .iter()
                .filter(|e| e.src == middleware && e.dst == session)
                .count(),
            1,
            "one edge per pair"
        );
    }

    #[test]
    fn graph_omits_excluded_files() {
        let (_d, store) = seeded();
        let cfg = singularrag_core::config::MapConfig::parse("[[exclude]]\npath = \"src/cli/\"\n")
            .unwrap();
        let g = graph(&store, &cfg).unwrap();
        assert!(g.nodes.iter().all(|n| !n.path.starts_with("src/cli/")));
    }
}
