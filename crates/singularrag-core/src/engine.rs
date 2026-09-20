//! The Engine: what the CLI, the MCP server and the UI call. Owns the store,
//! the authored config and the session key; every retrieval is recorded.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rusqlite::params;

use crate::config::MapConfig;
use crate::index::{IndexStats, Indexer};
use crate::map::{self, CUT_RECORDED};
use crate::rank::{rank_symbols, ScoredSymbol};
use crate::store::{lock, Store};
use crate::time::now_ms;
use crate::Result;

pub const DB_FILE: &str = ".singularrag/index.db";
/// Spec §8: a refresh that exceeds this answers with the stale count in the header.
/// The other half of §8, finishing the remaining files in the background, is the MCP
/// server's drain loop (plan 2).
pub const REFRESH_BUDGET: Duration = Duration::from_secs(2);
pub const LOCK_WAIT_MS: u64 = 500;
pub const HEARTBEAT_EVERY: Duration = Duration::from_secs(2);

/// One `Engine` per connection: it owns a `rusqlite::Connection`, which is `Send` but
/// not `Sync`, so an `Engine` cannot be shared between threads. Plan 2 (the MCP server)
/// decides between `Mutex<Connection>` and one `Engine` per request; nothing here
/// assumes either.
pub struct Engine {
    root: PathBuf,
    store: Store,
    config: MapConfig,
    /// mtime of `map.toml` when the config was last read (`None` when absent).
    config_mtime: Option<std::time::SystemTime>,
    session_key: String,
    heartbeat_every: Duration,
    heartbeats: std::cell::Cell<usize>,
    last_heartbeat_ms: std::cell::Cell<i64>,
    refresh_budget: Duration,
}

#[derive(Debug, Clone)]
pub struct MapRequest {
    pub query: Option<String>,
    pub focus_files: Vec<String>,
    pub budget_tokens: usize,
}

impl Default for MapRequest {
    /// `budget_tokens: 0` would be clamped up to `MIN_BUDGET` (64 tokens), which is not
    /// what "unspecified" means: the documented default is `DEFAULT_BUDGET`.
    fn default() -> Self {
        MapRequest {
            query: None,
            focus_files: Vec::new(),
            budget_tokens: map::DEFAULT_BUDGET,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MapResponse {
    pub retrieval_id: i64,
    pub text: String,
    pub served: usize,
    pub total: usize,
    pub cut_recorded: usize,
    pub stale_count: usize,
}

#[derive(Debug, Clone)]
pub struct FindRequest {
    pub name: String,
    pub kind: Option<String>,
    pub limit: usize,
}

#[derive(Debug, Clone)]
pub struct FindResponse {
    pub retrieval_id: i64,
    pub text: String,
    pub hits: usize,
    pub stale_count: usize,
}

impl Engine {
    pub fn open(root: &Path, session_key: &str) -> Result<Engine> {
        let root = root.canonicalize().map_err(|e| {
            crate::Error::Config(format!("opening repo at {}: {e}", root.display()))
        })?;
        let store = Store::open(&root.join(DB_FILE))?;
        let config = MapConfig::load(&root)?;
        let config_mtime = map_toml_mtime(&root);
        Ok(Engine {
            root,
            store,
            config,
            config_mtime,
            session_key: session_key.to_string(),
            heartbeat_every: HEARTBEAT_EVERY,
            heartbeats: std::cell::Cell::new(0),
            last_heartbeat_ms: std::cell::Cell::new(0),
            refresh_budget: REFRESH_BUDGET,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn store(&self) -> &Store {
        &self.store
    }
    pub fn config(&self) -> &MapConfig {
        &self.config
    }
    pub fn session_key(&self) -> &str {
        &self.session_key
    }

    /// How often the advisory lock's heartbeat is written during a long refresh.
    /// Injectable so a test does not have to spend `HEARTBEAT_EVERY` to observe one.
    pub fn set_heartbeat_every(&mut self, every: Duration) {
        self.heartbeat_every = every;
    }
    /// Heartbeats written since this Engine was opened.
    pub fn heartbeats(&self) -> usize {
        self.heartbeats.get()
    }
    /// Timestamp of the last heartbeat written, or 0 if none.
    pub fn last_heartbeat_ms(&self) -> i64 {
        self.last_heartbeat_ms.get()
    }

    /// Inline refresh budget used by `repo_map` and `find_symbol` (spec §8). The MCP
    /// server (plan 2) uses the same value for its background drain chunks; tests set
    /// it to zero to force a stale response without holding the lock.
    pub fn set_refresh_budget(&mut self, budget: Duration) {
        self.refresh_budget = budget;
    }

    pub fn refresh_budget(&self) -> Duration {
        self.refresh_budget
    }

    /// `map.toml` is authored while this process is alive (the UI writes it), so its
    /// mtime is checked before every refresh and the config re-read when it moved.
    /// An absent file is a state too: creating or deleting it counts as a change.
    fn reload_config_if_changed(&mut self) -> Result<()> {
        let mtime = map_toml_mtime(&self.root);
        if mtime != self.config_mtime {
            self.config = MapConfig::load(&self.root)?;
            self.config_mtime = mtime;
        }
        Ok(())
    }

    /// Refresh under the advisory lock. If the lock is held by a live process for
    /// longer than `LOCK_WAIT_MS`, index nothing and report how many files are stale.
    pub fn refresh(&mut self, budget: Duration) -> Result<IndexStats> {
        self.reload_config_if_changed()?;
        let pid = std::process::id();
        let indexer = Indexer::new(&self.store, &self.root, &self.config)?;
        let wait_until = Instant::now() + Duration::from_millis(LOCK_WAIT_MS);
        loop {
            if lock::try_acquire(&self.store, pid, now_ms())? {
                break;
            }
            if Instant::now() >= wait_until {
                let remaining = indexer.stale_count()?;
                return Ok(IndexStats {
                    remaining,
                    lock_timeout: true,
                    ..IndexStats::default()
                });
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let mut last_beat = Instant::now();
        let every = self.heartbeat_every;
        let result = indexer.refresh_with(Some(Instant::now() + budget), || {
            if last_beat.elapsed() >= every {
                let now = now_ms();
                let _ = lock::heartbeat(&self.store, pid, now);
                self.heartbeats.set(self.heartbeats.get() + 1);
                self.last_heartbeat_ms.set(now);
                last_beat = Instant::now();
            }
        });
        let released = lock::release(&self.store, pid);
        let stats = result?;
        released?;
        Ok(stats)
    }

    fn index_meta(&self) -> Result<(String, Option<String>)> {
        let version = self
            .store
            .get_meta("index_version")?
            .unwrap_or_else(|| "000000".to_string());
        let head = self.store.get_meta("git_head")?.filter(|h| !h.is_empty());
        Ok((version, head))
    }

    pub fn repo_map(&mut self, req: &MapRequest) -> Result<MapResponse> {
        let stats = self.refresh(self.refresh_budget)?;
        let budget = map::clamp_budget(req.budget_tokens);
        let ranked = rank_symbols(
            &self.store,
            &self.config,
            req.query.as_deref(),
            &req.focus_files,
        )?;
        let served = map::fit(&ranked, budget);
        let body = map::render(&ranked, served);
        let cut_recorded = (ranked.len() - served).min(CUT_RECORDED);
        let (version, head) = self.index_meta()?;

        let retrieval_id = self.record_retrieval(
            "repo_map",
            req.query.as_deref(),
            &req.focus_files,
            Some(budget),
            None,
            &version,
            head.as_deref(),
            stats.remaining,
            &ranked,
            served,
        )?;
        let text = format!(
            "{}\n{}{}\n",
            map::header(&version, head.as_deref(), stats.remaining, retrieval_id),
            body,
            map::footer(served, ranked.len(), cut_recorded)
        );
        Ok(MapResponse {
            retrieval_id,
            text,
            served,
            total: ranked.len(),
            cut_recorded,
            stale_count: stats.remaining,
        })
    }

    pub fn find_symbol(&mut self, req: &FindRequest) -> Result<FindResponse> {
        let stats = self.refresh(self.refresh_budget)?;
        // `find` shares `repo_map`'s cut semantics: look up the served hits plus the
        // candidates just below the limit, serve the first `limit` and record the rest
        // as `served = 0` so the provenance rows of the two tools are comparable.
        let limit = req.limit.clamp(1, crate::find::MAX_LIMIT);
        let candidates = crate::find::find_symbol(
            &self.store,
            &self.config,
            &req.name,
            req.kind.as_deref(),
            limit + CUT_RECORDED,
        )?;
        let served = candidates.len().min(limit);
        let hits = &candidates[..served];
        let (version, head) = self.index_meta()?;
        // The score is the FTS rank order (1, 1/2, 1/3 …): `find` does not run PageRank,
        // and the reasons say so (`fts_hit`, `find:<name>` seed).
        let ranked: Vec<ScoredSymbol> = candidates
            .iter()
            .enumerate()
            .map(|(i, h)| ScoredSymbol {
                symbol_id: h.symbol_id,
                file_id: 0,
                path: h.path.clone(),
                name: h.name.clone(),
                kind: h.kind.clone(),
                line_start: h.line,
                line_end: h.line,
                signature: h.signature.clone(),
                score: 1.0 / (i as f64 + 1.0),
                reasons: crate::rank::Reasons {
                    fts_hit: true,
                    query_ident_match: h.name.eq_ignore_ascii_case(&req.name),
                    referenced_by: h.referenced_from.clone(),
                    seeds: vec![format!("find:{}", req.name)],
                    ..Default::default()
                },
            })
            .collect();
        let retrieval_id = self.record_retrieval(
            "find_symbol",
            Some(&req.name),
            &[],
            None,
            Some(limit),
            &version,
            head.as_deref(),
            stats.remaining,
            &ranked,
            served,
        )?;
        let text = format!(
            "{}\n{}",
            map::header(&version, head.as_deref(), stats.remaining, retrieval_id),
            crate::find::render_find(hits)
        );
        Ok(FindResponse {
            retrieval_id,
            text,
            hits: served,
            stale_count: stats.remaining,
        })
    }

    /// Writes one `retrievals` row plus its served items and the `CUT_RECORDED`
    /// candidates below the line. `budget` is the token budget (`repo_map`) and
    /// `limit_n` the item limit (`find_symbol`); exactly one is set, so the two
    /// tools' rows stay comparable. Items carry `path`/`name`/`line_start` because
    /// `symbols.id` is reused after a reindex and would silently re-point.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record_retrieval(
        &self,
        tool: &str,
        query: Option<&str>,
        focus_files: &[String],
        budget: Option<usize>,
        limit_n: Option<usize>,
        index_version: &str,
        git_head: Option<&str>,
        stale: usize,
        ranked: &[ScoredSymbol],
        served: usize,
    ) -> Result<i64> {
        let conn = self.store.conn();
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO retrievals(session_key, tool, query, focus_files, budget, limit_n, index_version, git_head, stale_count, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                self.session_key,
                tool,
                query,
                serde_json::to_string(focus_files).unwrap_or_else(|_| "[]".into()),
                budget.map(|b| b as i64),
                limit_n.map(|l| l as i64),
                index_version,
                git_head,
                stale as i64,
                now_ms()
            ],
        )?;
        let id = tx.last_insert_rowid();
        {
            let mut stmt = tx.prepare(
                "INSERT INTO retrieval_items(retrieval_id, symbol_id, path, name, line_start, rank, score, served, reasons_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for (i, s) in ranked.iter().take(served + CUT_RECORDED).enumerate() {
                stmt.execute(params![
                    id,
                    s.symbol_id,
                    s.path,
                    s.name,
                    s.line_start,
                    (i + 1) as i64,
                    s.score,
                    i < served,
                    serde_json::to_string(&s.reasons).unwrap_or_else(|_| "{}".into())
                ])?;
            }
        }
        tx.commit()?;
        Ok(id)
    }
}

/// mtime of the repo's `map.toml`, or `None` when it does not exist (or cannot be
/// stat'ed). Filesystem mtime granularity bounds this: two writes inside one tick
/// look identical, as they do to the indexer's own stat walk.
fn map_toml_mtime(root: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(root.join(crate::config::MAP_FILE))
        .and_then(|m| m.modified())
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::write_ts_mini;
    use crate::store::lock;

    fn engine() -> (tempfile::TempDir, Engine) {
        let dir = tempfile::tempdir().unwrap();
        write_ts_mini(dir.path());
        let e = Engine::open(dir.path(), "test-session").unwrap();
        (dir, e)
    }

    fn count(e: &Engine, sql: &str) -> i64 {
        e.store().conn().query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn repo_map_indexes_renders_and_records_provenance() {
        let (_dir, mut e) = engine();
        let resp = e
            .repo_map(&MapRequest {
                query: Some("session".into()),
                focus_files: vec![],
                budget_tokens: 1024,
            })
            .unwrap();
        assert!(
            resp.text.starts_with("# singularrag · index "),
            "{}",
            resp.text
        );
        assert!(resp.text.contains("· fresh ·"));
        assert!(resp.text.contains("src/auth/session.ts:\n"));
        assert!(resp
            .text
            .contains("export function createSession(user: User, ttl: number): Session\n"));
        assert!(resp.text.ends_with(&format!(
            "{}\n",
            crate::map::footer(resp.served, resp.total, resp.cut_recorded)
        )));
        assert_eq!(resp.stale_count, 0);
        assert!(
            !resp.text.contains("console.log"),
            "bodies must never be served"
        );

        assert_eq!(count(&e, "SELECT COUNT(*) FROM retrievals"), 1);
        let served = count(&e, "SELECT COUNT(*) FROM retrieval_items WHERE served = 1");
        let cut = count(&e, "SELECT COUNT(*) FROM retrieval_items WHERE served = 0");
        assert_eq!(served as usize, resp.served);
        assert_eq!(cut as usize, resp.cut_recorded);
        assert!(cut as usize <= crate::map::CUT_RECORDED);
        let reasons: String = e
            .store()
            .conn()
            .query_row(
                "SELECT reasons_json FROM retrieval_items WHERE rank = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(reasons.contains("\"referenced_by\""));
        let tool: String = e
            .store()
            .conn()
            .query_row("SELECT tool FROM retrievals", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tool, "repo_map");
    }

    /// `symbols.id` is reused after a reindex (SQLite hands out `max(rowid)+1`), so a
    /// recorded item identified only by `symbol_id` would silently re-point at another
    /// symbol. The denormalised `path`/`name`/`line_start` columns are the identity.
    #[test]
    fn recorded_items_keep_their_identity_across_a_reindex() {
        let (dir, mut e) = engine();
        let resp = e
            .repo_map(&MapRequest {
                query: Some("log".into()),
                focus_files: vec![],
                budget_tokens: 1024,
            })
            .unwrap();
        let item = |e: &Engine| -> (i64, String, String, i64) {
            e.store()
                .conn()
                .query_row(
                    "SELECT symbol_id, path, name, line_start FROM retrieval_items
                     WHERE retrieval_id = ?1 AND rank = 1",
                    params![resp.retrieval_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .unwrap()
        };
        let before = item(&e);
        assert!(!before.1.is_empty() && !before.2.is_empty());

        // Rewrite two files so their symbols are deleted and reinserted with new ids.
        for (rel, body) in [
            ("src/util/log.ts", "export function log(msg: string): void {\n  emit(msg);\n}\nexport function logTwice(msg: string): void {\n  emit(msg);\n}\n"),
            ("src/cli/login.ts", "export function login(userId: string): void {}\n"),
        ] {
            let p = dir.path().join(rel);
            std::fs::write(&p, body).unwrap();
            let t = std::time::SystemTime::now() + Duration::from_secs(2);
            std::fs::File::open(&p).unwrap().set_modified(t).unwrap();
        }
        e.refresh(Duration::from_secs(5)).unwrap();

        let after = item(&e);
        assert_eq!(before, after, "recorded provenance must not move");
        // The tools are comparable: one fills `budget`, the other `limit_n`.
        let (budget, limit_n): (Option<i64>, Option<i64>) = e
            .store()
            .conn()
            .query_row(
                "SELECT budget, limit_n FROM retrievals WHERE id = ?1",
                params![resp.retrieval_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((budget, limit_n), (Some(1024), None));
    }

    /// Spec §9: rendered rows carry identifiers, signatures and paths only. One-line
    /// arrow definitions used to arrive with their whole body, quotes and trailing comment.
    #[test]
    fn rendered_rows_never_carry_bodies_comments_or_strings() {
        let dir = tempfile::tempdir().unwrap();
        write_ts_mini(dir.path());
        std::fs::write(
            dir.path().join("src/oneline.ts"),
            "export const onNotFound = (c: Context) => c.text('NotFound', 404)\nexport const isRawRequest = (request: Request): request is Request => 'headers' in request // 'headers' exists only on Request\n",
        )
        .unwrap();
        let mut e = Engine::open(dir.path(), "test-session").unwrap();
        let resp = e
            .repo_map(&MapRequest {
                query: None,
                focus_files: vec![],
                budget_tokens: 4096,
            })
            .unwrap();
        for line in resp.text.lines().filter(|l| l.starts_with(' ')) {
            assert!(
                !line.contains("//") && !line.contains('\'') && !line.contains('"'),
                "leaked: {line}\n{}",
                resp.text
            );
        }
        assert!(
            resp.text
                .contains("export const onNotFound = (c: Context) =>\n"),
            "{}",
            resp.text
        );
    }

    #[test]
    fn small_budget_cuts_and_records_up_to_25() {
        let (_dir, mut e) = engine();
        let resp = e
            .repo_map(&MapRequest {
                query: None,
                focus_files: vec![],
                budget_tokens: 64,
            })
            .unwrap();
        assert!(resp.served < resp.total);
        assert_eq!(
            resp.cut_recorded,
            (resp.total - resp.served).min(crate::map::CUT_RECORDED)
        );
        assert!(resp.text.contains(&format!(
            "{} more ranked below budget · {} recorded ·",
            resp.total - resp.served,
            resp.cut_recorded
        )));
    }

    /// `map.toml` used to be read once at `Engine::open`, so a pin or exclude authored
    /// while an MCP process was alive never applied. The same Engine must pick it up.
    #[test]
    fn excludes_from_map_toml_change_the_next_retrieval() {
        let (dir, mut e) = engine();
        let before = e
            .repo_map(&MapRequest {
                query: None,
                focus_files: vec![],
                budget_tokens: 4096,
            })
            .unwrap();
        assert!(before.text.contains("src/util/log.ts:"));
        std::fs::create_dir_all(dir.path().join(".singularrag")).unwrap();
        std::fs::write(
            dir.path().join(".singularrag/map.toml"),
            "[[exclude]]\npath = \"src/util/\"\n",
        )
        .unwrap();
        let after = e
            .repo_map(&MapRequest {
                query: None,
                focus_files: vec![],
                budget_tokens: 4096,
            })
            .unwrap();
        assert!(!after.text.contains("src/util/log.ts:"), "{}", after.text);
        assert!(e.config().is_excluded("src/util/log.ts"));

        // Deleting it is a change too.
        std::fs::remove_file(dir.path().join(".singularrag/map.toml")).unwrap();
        let back = e
            .repo_map(&MapRequest {
                query: None,
                focus_files: vec![],
                budget_tokens: 4096,
            })
            .unwrap();
        assert!(back.text.contains("src/util/log.ts:"));
    }

    #[test]
    fn map_request_default_is_the_documented_budget() {
        assert_eq!(MapRequest::default().budget_tokens, map::DEFAULT_BUDGET);
        assert!(MapRequest::default().query.is_none());
    }

    #[test]
    fn held_lock_yields_stale_header_without_indexing() {
        let (_dir, mut e) = engine();
        let other_pid = std::process::id() + 1;
        assert!(lock::try_acquire(e.store(), other_pid, crate::time::now_ms()).unwrap());
        let started = std::time::Instant::now();
        let resp = e
            .repo_map(&MapRequest {
                query: None,
                focus_files: vec![],
                budget_tokens: 1024,
            })
            .unwrap();
        assert!(started.elapsed() >= Duration::from_millis(LOCK_WAIT_MS));
        assert!(resp.stale_count > 0);
        let stats = e.refresh(REFRESH_BUDGET).unwrap();
        assert!(stats.lock_timeout, "{stats:?}");
        assert_eq!(stats.indexed, 0);
        assert!(resp.text.contains(&format!(
            "STALE: {} files changed since index",
            resp.stale_count
        )));
        assert_eq!(count(&e, "SELECT COUNT(*) FROM symbols"), 0);
        // A stale heartbeat is reclaimable.
        assert!(lock::try_acquire(
            e.store(),
            std::process::id(),
            crate::time::now_ms() + lock::LOCK_STALE_MS + 1
        )
        .unwrap());
    }

    #[test]
    fn refresh_heartbeats_the_lock() {
        let (_dir, mut e) = engine();
        // The default interval is longer than a fixture refresh takes, so nothing beats.
        let stats = e.refresh(Duration::from_secs(5)).unwrap();
        assert_eq!(stats.remaining, 0);
        assert_eq!(e.heartbeats(), 0);
        assert_eq!(count(&e, "SELECT COUNT(*) FROM indexer_lock"), 0);

        let t0 = crate::time::now_ms();
        e.set_heartbeat_every(Duration::ZERO);
        std::fs::write(
            _dir.path().join("src/extra.ts"),
            "export function extra(): void {}\n",
        )
        .unwrap();
        e.refresh(Duration::from_secs(5)).unwrap();
        assert!(e.heartbeats() > 0, "a long refresh must heartbeat");
        assert!(e.last_heartbeat_ms() >= t0);
        // The lock row itself moves with each heartbeat, and is released afterwards.
        assert_eq!(count(&e, "SELECT COUNT(*) FROM indexer_lock"), 0);
        assert!(lock::try_acquire(e.store(), 7, 1_000).unwrap());
        lock::heartbeat(e.store(), 7, 2_000).unwrap();
        assert_eq!(
            count(&e, "SELECT heartbeat_at_ms FROM indexer_lock WHERE pid = 7"),
            2_000
        );
    }

    #[test]
    fn lock_release_frees_it() {
        let (_dir, e) = engine();
        let now = crate::time::now_ms();
        assert!(lock::try_acquire(e.store(), 1, now).unwrap());
        assert!(!lock::try_acquire(e.store(), 2, now).unwrap());
        lock::release(e.store(), 1).unwrap();
        assert!(lock::try_acquire(e.store(), 2, now).unwrap());
    }

    #[test]
    fn zero_refresh_budget_makes_the_first_call_stale_and_default_catches_up() {
        let (_dir, mut e) = engine();
        assert_eq!(e.refresh_budget(), REFRESH_BUDGET);
        e.set_refresh_budget(Duration::ZERO);
        let first = e
            .repo_map(&MapRequest {
                query: None,
                focus_files: vec![],
                budget_tokens: 1024,
            })
            .unwrap();
        assert!(first.stale_count > 0, "{first:?}");
        assert!(first.text.contains("STALE:"));
        e.set_refresh_budget(REFRESH_BUDGET);
        let second = e
            .repo_map(&MapRequest {
                query: None,
                focus_files: vec![],
                budget_tokens: 1024,
            })
            .unwrap();
        assert_eq!(second.stale_count, 0);
        assert!(second.text.contains("· fresh ·"));
    }
}
