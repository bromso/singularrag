# singularrag workspace and text documents Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Index Markdown, HTML, CSS, JSON, YAML, TOML and plain-text documents from one or several roots as sections in the existing symbol model, rank them with code in one map, link them to code by mention, and give the UI a Documents view and a query panel.

**Architecture:** A `Workspace` (directory plus named roots) replaces the single repo root everywhere a root is consumed; paths gain a `<name>/` prefix only when roots are declared. A new core module `doc.rs` walks tree-sitter trees for the six document grammars and yields the same `Tag` shape as code, plus section bodies (a new `sections_fts` table keyed by symbol id) and mentions (ordinary `refs` rows). The ranker treats a body hit like a symbol FTS hit. `serve` gains `POST /api/query` through the actor; the UI gains a query panel, a root filter and document colours on the map.

**Tech Stack:** Rust (tree-sitter 0.27 with `tree-sitter-md 0.5`, `tree-sitter-html 0.23`, `tree-sitter-css 0.25`, `tree-sitter-json 0.24`, `tree-sitter-yaml 0.7`, `tree-sitter-toml-ng 0.7`; rusqlite FTS5; regex; toml), React UI in `ui/` (Bun only), axum in `serve`.

**Spec:** `docs/superpowers/specs/2026-09-23-singularrag-documents-design.md` (binding; parent spec `docs/superpowers/specs/2026-09-19-singularrag-design.md` §5/§6/§7/§9/§10/§12 are amended in Task 10).

## Global Constraints

- Branch `documents-v0` from `workflow-v0` (0b769e3). Worktree `/Users/jonasbroms/Sites/singularrag/.claude/worktrees/ui-v0`; use absolute paths, never `cd` into the main checkout; the Bash guard rejects any command containing the substring `eval`, so refer to that directory as `e*/` in shell globs and type the word only inside file content written through the Write tool; never a bare `git stash`; commit with `git add -A`.
- No model, no embeddings, no new MCP tool, no new UI view component beyond `QueryPanel`; no per-kind budget share (spec §11).
- Workspace: `.singularrag/workspace.toml`, `[[root]]` with `name` matching `^[A-Za-z0-9_-]{1,64}$` and unique, `path` relative to the workspace directory or absolute; roots canonicalised, must be directories, none inside another; no file or no entry means one unnamed root and bare paths; declared roots mean every path is `<name>/<relative>`; the file is read, never written (spec §2, §8).
- Document `lang` values: `markdown`, `html`, `css`, `json`, `yaml`, `toml`, `text`. Extensions: `.md .markdown`, `.html .htm`, `.css`, `.json`, `.yaml .yml`, `.toml`, `.txt` (spec §3).
- Kinds: `section`, `document`, `element`, `rule`, `key`. Every document gets one `document` symbol named by its file stem spanning the whole file. Markdown h4 and deeper fold into their parent. Keys to depth 2. Values in JSON, YAML and TOML never enter symbols, FTS or signatures beyond the type word (`object`, `array`, `string`, `number`, `bool`) (spec §3).
- `sections_fts(path UNINDEXED, name UNINDEXED, content, tokenize='porter unicode61')`, rowid = the symbol's id, rows for kinds `section`, `document`, `element` only (spec §3).
- Document skips, exact `skipped_reason` strings: `too-large` over `DOC_MAX_BYTES = 256 * 1024`; `lockfile` for `package-lock.json`, `yarn.lock`, `pnpm-lock.yaml`, `Cargo.lock`, `bun.lock`, `bun.lockb`, `composer.lock`, `Gemfile.lock`, `poetry.lock`; `minified` when average line length exceeds 500 characters; `secret-like content` as today (spec §3).
- Mentions (spec §4): inline code spans and `<code>` text that are one identifier `^[A-Za-z_][A-Za-z0-9_]*(\(\))?$`; wiki links `[[name]]` / `[[name|label]]` (name before `#` or `|`); relative Markdown link targets and `href` values by file stem, fragments dropped, absolute URLs (`scheme:`) ignored; nothing from fenced code blocks.
- Ranking (spec §5): a `sections_fts` hit seeds the file with `FTS_FILE_BOOST` and the matching symbol scores like an FTS hit; `Reasons.body_hit: bool`; documents are never support files; no ranker change beyond that.
- Freshness header: single root `HEAD <7 chars>` as today; several roots `HEAD app:9b1e0d4 vault:none` in root order (spec §2).
- `POST /api/query {query, budget}`: query 1 to 2000 characters, budget clamped by `map::clamp_budget`, runs `repo_map` under session key `serve` (label `UI`), returns `{retrieval_id, served, cut}` (spec §6; the session key ruling is in Task 8).
- Agent-facing text (`INSTRUCTIONS`, `REPO_MAP_DESCRIPTION`) is a string literal in `server.rs`, kept byte-identical in the `#[tool]` attribute and `tests/mcp.rs` (existing constraint).
- Eval gate (spec §7): hono tier-one recall at 4096 within 0.02 of its value at 0b769e3; the docs set at or above 0.6 at 4096.

## Review Focus

1. A Markdown file whose first heading is `##` (no h1) or that mixes setext (`===`) and atx headings: sections must still nest by level and the document symbol must still exist. Test in Task 4 (`setext_and_missing_h1_still_nest`).
2. A JSON file that is an array at the top level, or a YAML file with several `---` documents: no panic, a `document` symbol and no keys, or keys from each document. Test in Task 4 (`top_level_array_and_multi_document_yaml`).
3. A root declared with a trailing slash, a symlink, or `~`: the path must canonicalise or fail with the file and field named, never index the wrong directory. Test in Task 1 (`root_paths_normalise_or_name_the_field`).
4. A hook Read of a file in a second root when `--repo` names the workspace: the longest-prefix mapping must find it and deny once. Test in Task 3 (`hook_denies_a_read_in_a_second_root`).
5. A query submitted from the UI while the index is stale or the lock is held: the panel must show the header's STALE state through the rail badge and not hang; a 422 on an empty query must be announced. Test in Task 9 (`query_panel_announces_errors`).

---

## File structure

- `crates/singularrag-core/src/workspace.rs` (new): `Workspace`, `Root`, `WORKSPACE_FILE`, loading and validation, `rel_of`, `abs_of`, `prefix`.
- `crates/singularrag-core/src/walk.rs`: `walk_root(root, prefix, deny)` and `walk_workspace(ws, deny)`; `walk(root, deny)` stays as the single-root wrapper.
- `crates/singularrag-core/src/index.rs`: `Indexer::new(store, &Workspace, config)`, per-root `git_head:<name>` meta, document skips, `sections_fts` and mention writes.
- `crates/singularrag-core/src/config.rs`: `MapConfig::check_roots(&[&str])`.
- `crates/singularrag-core/src/engine.rs`: `Engine::open(dir)` opens a `Workspace`; `workspace()`; composed multi-root head.
- `crates/singularrag-core/src/lang.rs`: seven new `Language` variants; `extract_tags` dispatches document languages to `doc::extract`.
- `crates/singularrag-core/src/doc.rs` (new): `DocExtract`, `extract(lang, source, stem)`, per-format walkers, mention scanner, skip helpers.
- `crates/singularrag-core/src/store/schema.rs`: `sections_fts`, `SCHEMA_VERSION = 3`.
- `crates/singularrag-core/src/rank.rs`: `fts_body_ids`, `Reasons.body_hit`.
- `crates/singularrag-core/src/map.rs`: `freshness_header` keeps a composed multi-root head whole.
- `crates/singularrag-core/src/changed.rs`: `changed(store, config, &Workspace, base)` per git root.
- `crates/singularrag-core/src/fixture.rs`: `write_docs_mini`, `write_workspace`.
- `crates/singularrag/src/hook.rs`, `serve/state.rs`, `serve/watcher.rs`, `serve/queries.rs`, `serve/routes.rs`, `serve/mod.rs`, `mcp/server.rs`, `tests/mcp.rs`, `tests/cli.rs`.
- `ui/src/api/types.ts`, `ui/src/api/client.ts`, `ui/src/lib/reasons.ts`, `ui/src/lib/mapStyle.ts`, `ui/src/lib/langGroup.ts` (new), `ui/src/components/QueryPanel.tsx` (new), `ui/src/App.tsx`, tests beside each.
- `e*/questions-docs.toml` (new), README, parent spec.

---

### Task 1: The workspace model

**Files:**
- Create: `crates/singularrag-core/src/workspace.rs`
- Modify: `crates/singularrag-core/src/lib.rs` (add `pub mod workspace;`)
- Modify: `crates/singularrag-core/src/config.rs` (add `check_roots`)

**Interfaces:**
- Produces: `pub const WORKSPACE_FILE: &str = ".singularrag/workspace.toml"`; `pub struct Root { pub name: String, pub path: PathBuf }`; `pub struct Workspace { pub dir: PathBuf, pub roots: Vec<Root> }`; `Workspace::open(dir: &Path) -> Result<Workspace>`; `Workspace::single(dir: &Path) -> Result<Workspace>` (test helper, no file read); `fn is_named(&self) -> bool`; `fn prefix(&self, root: &Root) -> String` (`""` or `"<name>/"`); `fn rel_of(&self, abs: &Path) -> Option<String>`; `fn abs_of(&self, rel: &str) -> Option<PathBuf>`; `fn root_of(&self, rel: &str) -> Option<(&Root, String)>`; `fn names(&self) -> Vec<&str>` (empty when unnamed); `MapConfig::check_roots(&self, names: &[&str]) -> Result<()>`.

- [ ] **Step 1: Write the failing tests**

`crates/singularrag-core/src/workspace.rs` (tests only for now, plus the `use` lines the implementation will need):

```rust
//! A workspace: the directory that holds `.singularrag/`, and the roots it indexes
//! (documents spec §2). No file, or no `[[root]]`, means the directory is its only root
//! and every path is bare; declared roots prefix every path with `<name>/`.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::{Error, Result};

pub const WORKSPACE_FILE: &str = ".singularrag/workspace.toml";

#[cfg(test)]
mod tests {
    use super::*;

    fn ws_dir() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".singularrag")).unwrap();
        d
    }

    fn write_roots(d: &Path, body: &str) {
        std::fs::write(d.join(WORKSPACE_FILE), body).unwrap();
    }

    #[test]
    fn no_file_means_one_unnamed_root_with_bare_paths() {
        let d = ws_dir();
        std::fs::create_dir_all(d.path().join("src")).unwrap();
        std::fs::write(d.path().join("src/a.ts"), "").unwrap();
        let ws = Workspace::open(d.path()).unwrap();
        assert_eq!(ws.roots.len(), 1);
        assert_eq!(ws.roots[0].name, "");
        assert!(!ws.is_named());
        assert_eq!(ws.prefix(&ws.roots[0]), "");
        let abs = d.path().join("src/a.ts").canonicalize().unwrap();
        assert_eq!(ws.rel_of(&abs).as_deref(), Some("src/a.ts"));
        assert_eq!(ws.abs_of("src/a.ts"), Some(abs));
        assert!(ws.names().is_empty());
    }

    #[test]
    fn declared_roots_prefix_paths_and_map_back_by_longest_prefix() {
        let d = ws_dir();
        let app = tempfile::tempdir().unwrap();
        let vault = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(app.path().join("src")).unwrap();
        std::fs::write(app.path().join("src/a.ts"), "").unwrap();
        std::fs::write(vault.path().join("note.md"), "").unwrap();
        write_roots(
            d.path(),
            &format!(
                "[[root]]\nname = \"app\"\npath = \"{}\"\n[[root]]\nname = \"vault\"\npath = \"{}\"\n",
                app.path().display(),
                vault.path().display()
            ),
        );
        let ws = Workspace::open(d.path()).unwrap();
        assert!(ws.is_named());
        assert_eq!(ws.names(), vec!["app", "vault"]);
        assert_eq!(ws.prefix(&ws.roots[0]), "app/");
        let abs = app.path().join("src/a.ts").canonicalize().unwrap();
        assert_eq!(ws.rel_of(&abs).as_deref(), Some("app/src/a.ts"));
        assert_eq!(ws.abs_of("vault/note.md"), Some(vault.path().canonicalize().unwrap().join("note.md")));
        assert!(ws.abs_of("nope/x.md").is_none(), "unknown prefix");
        assert!(ws.rel_of(d.path()).is_none(), "the workspace dir itself is not a root");
        let (root, rest) = ws.root_of("vault/a/b.md").unwrap();
        assert_eq!((root.name.as_str(), rest.as_str()), ("vault", "a/b.md"));
    }

    #[test]
    fn relative_root_paths_resolve_against_the_workspace_dir() {
        let d = ws_dir();
        std::fs::create_dir_all(d.path().join("sub/app")).unwrap();
        write_roots(d.path(), "[[root]]\nname = \"app\"\npath = \"sub/app\"\n");
        let ws = Workspace::open(d.path()).unwrap();
        assert_eq!(ws.roots[0].path, d.path().join("sub/app").canonicalize().unwrap());
    }

    #[test]
    fn root_paths_normalise_or_name_the_field() {
        let d = ws_dir();
        std::fs::create_dir_all(d.path().join("app")).unwrap();
        write_roots(d.path(), "[[root]]\nname = \"app\"\npath = \"app/\"\n");
        let ws = Workspace::open(d.path()).unwrap();
        assert_eq!(ws.roots[0].path, d.path().join("app").canonicalize().unwrap(), "trailing slash");
        write_roots(d.path(), "[[root]]\nname = \"app\"\npath = \"missing\"\n");
        let e = Workspace::open(d.path()).unwrap_err().to_string();
        assert!(e.contains("workspace.toml") && e.contains("root[0].path"), "{e}");
        std::fs::write(d.path().join("file.txt"), "").unwrap();
        write_roots(d.path(), "[[root]]\nname = \"f\"\npath = \"file.txt\"\n");
        let e = Workspace::open(d.path()).unwrap_err().to_string();
        assert!(e.contains("not a directory"), "{e}");
    }

    #[test]
    fn bad_names_duplicates_and_nesting_are_errors() {
        let d = ws_dir();
        std::fs::create_dir_all(d.path().join("a/b")).unwrap();
        write_roots(d.path(), "[[root]]\nname = \"bad name\"\npath = \"a\"\n");
        let e = Workspace::open(d.path()).unwrap_err().to_string();
        assert!(e.contains("root[0].name"), "{e}");
        write_roots(d.path(), "[[root]]\nname = \"x\"\npath = \"a\"\n[[root]]\nname = \"x\"\npath = \"a/b\"\n");
        let e = Workspace::open(d.path()).unwrap_err().to_string();
        assert!(e.contains("duplicate root name x"), "{e}");
        write_roots(d.path(), "[[root]]\nname = \"a\"\npath = \"a\"\n[[root]]\nname = \"b\"\npath = \"a/b\"\n");
        let e = Workspace::open(d.path()).unwrap_err().to_string();
        assert!(e.contains("root a contains root b"), "{e}");
    }

    #[test]
    fn malformed_file_names_the_file() {
        let d = ws_dir();
        write_roots(d.path(), "[[root]\nname = 1\n");
        let e = Workspace::open(d.path()).unwrap_err().to_string();
        assert!(e.contains("workspace.toml"), "{e}");
    }
}
```

`crates/singularrag-core/src/config.rs`, inside the existing `mod tests`:

```rust
    #[test]
    fn check_roots_accepts_root_prefixes_and_rejects_unknown_ones() {
        let cfg = MapConfig::parse("[[pin]]\npath = \"app/src/a.ts\"\n[[exclude]]\npath = \"vault/old/\"\n").unwrap();
        cfg.check_roots(&["app", "vault"]).unwrap();
        cfg.check_roots(&[]).unwrap();
        let bad = MapConfig::parse("[[pin]]\npath = \"src/a.ts\"\n").unwrap();
        let e = bad.check_roots(&["app", "vault"]).unwrap_err().to_string();
        assert!(e.contains("pin[0].path") && e.contains("app, vault"), "{e}");
        let note = MapConfig::parse("[[note]]\npath = \"x/a.md\"\ntext = \"t\"\n").unwrap();
        assert!(note.check_roots(&["app"]).is_err());
        let boundary = MapConfig::parse("[[boundary]]\nname = \"b\"\npaths = [\"app/src/\", \"nope/\"]\n").unwrap();
        let e = boundary.check_roots(&["app"]).unwrap_err().to_string();
        assert!(e.contains("boundary[0].paths[1]"), "{e}");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p singularrag-core --lib workspace:: 2>&1 | grep -E '^error|test result' | head -3`
Expected: compile errors, `cannot find type Workspace`.

- [ ] **Step 3: Implement**

Add to `workspace.rs` above the tests:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct Root {
    /// Empty for the single unnamed root.
    pub name: String,
    /// Canonical.
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct Workspace {
    /// Canonical; holds `.singularrag/`.
    pub dir: PathBuf,
    /// In file order; one unnamed root when no file declares any.
    pub roots: Vec<Root>,
}

#[derive(Deserialize)]
struct WorkspaceFile {
    #[serde(default)]
    root: Vec<RootEntry>,
}

#[derive(Deserialize)]
struct RootEntry {
    name: String,
    path: PathBuf,
}

fn valid_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 64
        && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

impl Workspace {
    /// The directory as its own single root; no file is read.
    pub fn single(dir: &Path) -> Result<Workspace> {
        let dir = dir
            .canonicalize()
            .map_err(|e| Error::Config(format!("opening workspace at {}: {e}", dir.display())))?;
        Ok(Workspace {
            roots: vec![Root { name: String::new(), path: dir.clone() }],
            dir,
        })
    }

    pub fn open(dir: &Path) -> Result<Workspace> {
        let ws = Workspace::single(dir)?;
        let file = ws.dir.join(WORKSPACE_FILE);
        let text = match std::fs::read_to_string(&file) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(ws),
            Err(e) => return Err(Error::Config(format!("{}: {e}", file.display()))),
        };
        let parsed: WorkspaceFile = toml::from_str(&text)
            .map_err(|e| Error::Config(format!("{}: {e}", file.display())))?;
        if parsed.root.is_empty() {
            return Ok(ws);
        }
        let mut roots: Vec<Root> = Vec::with_capacity(parsed.root.len());
        for (i, r) in parsed.root.iter().enumerate() {
            let field = |f: &str| format!("{}: root[{i}].{f}", file.display());
            if !valid_name(&r.name) {
                return Err(Error::Config(format!(
                    "{}: {:?} must match [A-Za-z0-9_-]{{1,64}}",
                    field("name"),
                    r.name
                )));
            }
            if roots.iter().any(|x| x.name == r.name) {
                return Err(Error::Config(format!("{}: duplicate root name {}", file.display(), r.name)));
            }
            let raw = if r.path.is_absolute() { r.path.clone() } else { ws.dir.join(&r.path) };
            let path = raw
                .canonicalize()
                .map_err(|e| Error::Config(format!("{}: {}: {e}", field("path"), raw.display())))?;
            if !path.is_dir() {
                return Err(Error::Config(format!("{}: {} is not a directory", field("path"), path.display())));
            }
            roots.push(Root { name: r.name.clone(), path });
        }
        for a in &roots {
            for b in &roots {
                if a.name != b.name && b.path.starts_with(&a.path) {
                    return Err(Error::Config(format!(
                        "{}: root {} contains root {}",
                        file.display(),
                        a.name,
                        b.name
                    )));
                }
            }
        }
        Ok(Workspace { dir: ws.dir, roots })
    }

    pub fn is_named(&self) -> bool {
        self.roots.iter().any(|r| !r.name.is_empty())
    }

    pub fn names(&self) -> Vec<&str> {
        self.roots.iter().filter(|r| !r.name.is_empty()).map(|r| r.name.as_str()).collect()
    }

    pub fn prefix(&self, root: &Root) -> String {
        if root.name.is_empty() {
            String::new()
        } else {
            format!("{}/", root.name)
        }
    }

    /// `<name>/<relative>` (or bare) for a canonical absolute path under a root: the
    /// longest root prefix wins, which matters only when roots are nested, which `open`
    /// refuses; it is still the correct rule.
    pub fn rel_of(&self, abs: &Path) -> Option<String> {
        let mut best: Option<(usize, String)> = None;
        for r in &self.roots {
            if let Ok(rest) = abs.strip_prefix(&r.path) {
                let rest = rest.to_string_lossy().replace('\\', "/");
                if rest.is_empty() {
                    continue;
                }
                let len = r.path.as_os_str().len();
                if best.as_ref().is_none_or(|(l, _)| len > *l) {
                    best = Some((len, format!("{}{rest}", self.prefix(r))));
                }
            }
        }
        best.map(|(_, s)| s)
    }

    pub fn root_of(&self, rel: &str) -> Option<(&Root, String)> {
        if !self.is_named() {
            return Some((&self.roots[0], rel.to_string()));
        }
        let (name, rest) = rel.split_once('/')?;
        let root = self.roots.iter().find(|r| r.name == name)?;
        Some((root, rest.to_string()))
    }

    pub fn abs_of(&self, rel: &str) -> Option<PathBuf> {
        let (root, rest) = self.root_of(rel)?;
        Some(root.path.join(rest))
    }
}
```

`lib.rs`: add `pub mod workspace;` in alphabetical position. `config.rs`, in `impl MapConfig`:

```rust
    /// In a named workspace every authored path starts with a root name (documents spec
    /// §2). `names` empty means an unnamed workspace, where any relative path is fine.
    pub fn check_roots(&self, names: &[&str]) -> Result<()> {
        if names.is_empty() {
            return Ok(());
        }
        let check = |field: String, p: &str| -> Result<()> {
            let first = p.split('/').next().unwrap_or("");
            if names.contains(&first) {
                Ok(())
            } else {
                Err(Error::Config(format!(
                    "map.toml: {field} {p:?} does not start with a root name ({})",
                    names.join(", ")
                )))
            }
        };
        for (i, t) in self.pin.iter().enumerate() {
            check(format!("pin[{i}].path"), &t.path)?;
        }
        for (i, t) in self.exclude.iter().enumerate() {
            check(format!("exclude[{i}].path"), &t.path)?;
        }
        for (i, n) in self.note.iter().enumerate() {
            check(format!("note[{i}].path"), &n.path)?;
        }
        for (i, b) in self.boundary.iter().enumerate() {
            for (j, p) in b.paths.iter().enumerate() {
                check(format!("boundary[{i}].paths[{j}]"), p)?;
            }
        }
        Ok(())
    }
```

- [ ] **Step 4: Run the tests**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test -p singularrag-core --lib 2>&1 | grep -E 'test result|panicked'`
Expected: `test result: ok` with 6 new workspace tests and 1 new config test.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(core): the workspace model, named roots with prefixed paths"
```

---

### Task 2: Walk and index a workspace

**Files:**
- Modify: `crates/singularrag-core/src/walk.rs` (`walk_root`, `walk_workspace`)
- Modify: `crates/singularrag-core/src/index.rs` (`Indexer::new` takes `&Workspace`; per-root heads)
- Modify: `crates/singularrag-core/src/engine.rs` (`Engine::open` opens the workspace; `workspace()`; composed head)
- Modify: `crates/singularrag-core/src/map.rs` (`freshness_header` keeps a composed head)
- Modify: `crates/singularrag-core/src/fixture.rs` (`write_workspace`)
- Modify: every caller of `Indexer::new(&store, dir.path(), …)` in core tests (`changed.rs`, `trace.rs`, `blast.rs`, `rank.rs`, `find.rs`, `eval.rs`, `index.rs`): replace `dir.path()` with `&Workspace::single(dir.path()).unwrap()`; grep `Indexer::new(` to find them all.

**Interfaces:**
- Consumes: Task 1's `Workspace`.
- Produces: `walk::walk_root(root: &Path, prefix: &str, deny: &[String]) -> Result<WalkResult>`; `walk::walk_workspace(ws: &Workspace, deny: &[String]) -> Result<WalkResult>`; `Indexer::new(store, ws: &Workspace, config)`; meta keys `git_head` (single root: the head; named: the composed `app:9b1e0d4 vault:none` string) and `git_head:<name>` per named root; `Engine::workspace(&self) -> &Workspace`; `fixture::write_workspace(dir: &Path) -> (PathBuf, PathBuf)` returning the `app` and `notes` root paths (the notes root content arrives in Task 5; here it holds one `README.md`).

- [ ] **Step 1: Write the failing tests**

`walk.rs` tests:

```rust
    #[test]
    fn walk_workspace_prefixes_named_roots_and_sorts() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".singularrag")).unwrap();
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        write(a.path(), "src/z.ts", "");
        write(b.path(), "n.md", "");
        std::fs::write(
            d.path().join(crate::workspace::WORKSPACE_FILE),
            format!("[[root]]\nname = \"app\"\npath = \"{}\"\n[[root]]\nname = \"vault\"\npath = \"{}\"\n", a.path().display(), b.path().display()),
        )
        .unwrap();
        let ws = crate::workspace::Workspace::open(d.path()).unwrap();
        let r = walk_workspace(&ws, &[]).unwrap();
        let paths: Vec<&str> = r.entries.iter().map(|e| e.rel_path.as_str()).collect();
        assert_eq!(paths, vec!["app/src/z.ts", "vault/n.md"]);
        let single = walk_workspace(&crate::workspace::Workspace::single(a.path()).unwrap(), &[]).unwrap();
        assert_eq!(single.entries[0].rel_path, "src/z.ts");
    }
```

`index.rs` tests (add next to the existing ones; use the `Workspace` import):

```rust
    #[test]
    fn a_named_workspace_indexes_both_roots_and_records_a_head_per_root() {
        let d = tempfile::tempdir().unwrap();
        let (app, _notes) = crate::fixture::write_workspace(d.path());
        // Make `app` a git checkout so it has a head; `notes` stays plain.
        let git = |args: &[&str]| {
            assert!(std::process::Command::new("git").arg("-C").arg(&app).args(args).status().unwrap().success());
        };
        git(&["init", "-q"]);
        git(&["-c", "user.email=t@t", "-c", "user.name=t", "add", "."]);
        git(&["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "base"]);
        let ws = Workspace::open(d.path()).unwrap();
        let store = Store::open(&d.path().join(crate::engine::DB_FILE)).unwrap();
        let s = Indexer::new(&store, &ws, &MapConfig::default()).unwrap().refresh(None).unwrap();
        assert!(s.indexed >= 5, "{s:?}");
        let n: i64 = store
            .conn()
            .query_row("SELECT COUNT(*) FROM files WHERE path LIKE 'app/%' AND skipped_reason IS NULL", [], |r| r.get(0))
            .unwrap();
        assert!(n >= 4, "{n}");
        let head = store.get_meta("git_head:app").unwrap().unwrap();
        assert_eq!(head.len(), 40);
        assert_eq!(store.get_meta("git_head:notes").unwrap().as_deref(), Some(""));
        let composed = store.get_meta("git_head").unwrap().unwrap();
        assert_eq!(composed, format!("app:{} notes:none", &head[..7]));
    }
```

`map.rs` tests:

```rust
    #[test]
    fn freshness_header_keeps_a_composed_multi_root_head_whole() {
        let h = freshness_header("abcdef123", Some("app:9b1e0d4 notes:none"), 0);
        assert_eq!(h, "# singularrag · index abcdef · HEAD app:9b1e0d4 notes:none · fresh");
        let single = freshness_header("abcdef123", Some("9b1e0d4f00"), 2);
        assert!(single.contains("HEAD 9b1e0d4 ·"), "{single}");
    }
```

`engine.rs` tests:

```rust
    #[test]
    fn open_on_a_named_workspace_prefixes_map_paths_and_rejects_unknown_prefixes() {
        let d = tempfile::tempdir().unwrap();
        crate::fixture::write_workspace(d.path());
        std::fs::write(d.path().join(crate::config::MAP_FILE), "[[pin]]\npath = \"app/src/auth/session.ts\"\n").unwrap();
        let mut e = Engine::open(d.path(), "t").unwrap();
        assert_eq!(e.workspace().names(), vec!["app", "notes"]);
        let r = e.repo_map(&MapRequest { query: Some("session".into()), ..Default::default() }).unwrap();
        assert!(r.text.contains("app/src/auth/session.ts:"), "{}", r.text);
        assert!(r.text.contains("HEAD app:none notes:none"), "{}", r.text);
        std::fs::write(d.path().join(crate::config::MAP_FILE), "[[pin]]\npath = \"src/auth/session.ts\"\n").unwrap();
        let err = Engine::open(d.path(), "t").unwrap_err().to_string();
        assert!(err.contains("does not start with a root name"), "{err}");
    }
```

`fixture.rs`:

```rust
/// A named workspace: `app` is the TypeScript mini repo, `notes` holds one README that
/// mentions `createSession` in a code span. Returns the two root paths. The workspace
/// directory itself holds only `.singularrag/workspace.toml`.
pub fn write_workspace(dir: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let app = dir.join("roots/app");
    let notes = dir.join("roots/notes");
    write_ts_mini(&app);
    w(&notes, "README.md", "# Notes\n\nSessions come from `createSession`; see [[design]].\n");
    w(
        dir,
        crate::workspace::WORKSPACE_FILE,
        "[[root]]\nname = \"app\"\npath = \"roots/app\"\n[[root]]\nname = \"notes\"\npath = \"roots/notes\"\n",
    );
    (app, notes)
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p singularrag-core --lib 2>&1 | grep -E '^error' | head -5`
Expected: `cannot find function walk_workspace`, `write_workspace`, mismatched `Indexer::new` arguments.

- [ ] **Step 3: Implement**

`walk.rs`: rename the body of `walk` into `walk_root` and add the prefix:

```rust
/// Walk one root; `prefix` (`""` or `"<name>/"`) is prepended to every relative path.
pub fn walk_root(root: &Path, prefix: &str, deny: &[String]) -> Result<WalkResult> {
    // … the existing body, with the two `rel_path: rel` constructions changed to
    // `rel_path: format!("{prefix}{rel}")` and the denyset matched on `rel` (unprefixed).
}

/// The single-root walk, kept for callers and tests that have only a directory.
pub fn walk(root: &Path, deny: &[String]) -> Result<WalkResult> {
    walk_root(root, "", deny)
}

/// Every root of the workspace, concatenated and sorted by prefixed path.
pub fn walk_workspace(ws: &crate::workspace::Workspace, deny: &[String]) -> Result<WalkResult> {
    let mut out = WalkResult::default();
    for r in &ws.roots {
        let one = walk_root(&r.path, &ws.prefix(r), deny)?;
        out.entries.extend(one.entries);
        out.skipped.extend(one.skipped);
    }
    out.entries.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(out)
}
```

`index.rs`: `Indexer { store, ws: Workspace, config }`; `new(store, ws: &Workspace, config)` stores `ws.clone()`; both `walk(&self.root, …)` calls become `walk_workspace(&self.ws, …)`; `write_meta`:

```rust
        if self.ws.is_named() {
            let mut parts = Vec::new();
            for r in &self.ws.roots {
                let head = git_head(&r.path).unwrap_or_default();
                self.store.set_meta(&format!("git_head:{}", r.name), &head)?;
                let short = if head.is_empty() { "none".to_string() } else { head.chars().take(7).collect() };
                parts.push(format!("{}:{short}", r.name));
            }
            self.store.set_meta("git_head", &parts.join(" "))?;
        } else {
            match git_head(&self.ws.roots[0].path) {
                Some(h) => self.store.set_meta("git_head", &h)?,
                None => self.store.set_meta("git_head", "")?,
            }
        }
```

`map.rs` `freshness_header`: a head containing `':'` is already composed and rendered whole:

```rust
    let head = match git_head {
        Some(h) if h.contains(':') => h.to_string(),
        Some(h) if !h.is_empty() => h.chars().take(7).collect::<String>(),
        _ => "none".to_string(),
    };
```

`engine.rs`: field `ws: Workspace` beside `root` (keep `root` = `ws.dir` so `root()` and `map_toml_mtime` are unchanged); `open`:

```rust
    pub fn open(dir: &Path, session_key: &str) -> Result<Engine> {
        let ws = crate::workspace::Workspace::open(dir)?;
        let root = ws.dir.clone();
        let store = Store::open(&root.join(DB_FILE))?;
        let config = MapConfig::load(&root)?;
        config.check_roots(&ws.names())?;
        …
    }
    pub fn workspace(&self) -> &crate::workspace::Workspace { &self.ws }
```

`reload_config_if_changed` also calls `config.check_roots(&self.ws.names())?` after a reload; `refresh` builds `Indexer::new(&self.store, &self.ws, &self.config)`. The composed head reaches `index_meta` through the `git_head` meta unchanged.

- [ ] **Step 4: Run the tests**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace 2>&1 | grep -E 'test result|panicked|FAILED'`
Expected: every binary `ok`; the `singularrag` crate compiles because `Engine::open(root, …)` keeps its signature.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(core): walk and index a workspace of named roots; a head per root"
```

---

### Task 3: Workspace consumers: hook, changed, watcher, serve status

**Files:**
- Modify: `crates/singularrag-core/src/changed.rs` (`changed(store, config, ws, base)`)
- Modify: `crates/singularrag-core/src/engine.rs` (`Engine::changed` passes `&self.ws`)
- Modify: `crates/singularrag/src/hook.rs` (map through `Workspace`)
- Modify: `crates/singularrag/src/serve/watcher.rs` (watch every root; `interesting(ws, p)`)
- Modify: `crates/singularrag/src/serve/state.rs` (`AppState.ws: Workspace`)
- Modify: `crates/singularrag/src/serve/queries.rs` (`StatusDto.roots`)
- Modify: `crates/singularrag/tests/cli.rs`

**Interfaces:**
- Consumes: `Workspace` (Task 1), `write_workspace` (Task 2).
- Produces: `changed::changed(store, config, ws: &Workspace, base: Option<&str>) -> Result<Changed>`; `queries::RootDto { name: String, path: String, git_head: Option<String> }`; `StatusDto.roots: Vec<RootDto>` (one entry with `name: ""` for an unnamed workspace); `watcher::interesting(ws: &Workspace, p: &Path) -> bool`.

- [ ] **Step 1: Write the failing tests**

`changed.rs` tests:

```rust
    #[test]
    fn a_named_workspace_diffs_each_git_root_and_prefixes_paths() {
        let d = tempfile::tempdir().unwrap();
        let (app, notes) = crate::fixture::write_workspace(d.path());
        for root in [&app, &notes] {
            git(root, &["init", "-q"]);
            git(root, &["-c", "user.email=t@t", "-c", "user.name=t", "add", "."]);
            git(root, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "base"]);
        }
        touch_create_session(&app);
        std::fs::write(notes.join("new.md"), "# New\n").unwrap();
        let ws = Workspace::open(d.path()).unwrap();
        let store = Store::open(&d.path().join(crate::engine::DB_FILE)).unwrap();
        Indexer::new(&store, &ws, &MapConfig::default()).unwrap().refresh(None).unwrap();
        let c = changed(&store, &MapConfig::default(), &ws, None).unwrap();
        assert!(c.symbols.iter().any(|s| s.path == "app/src/auth/session.ts" && s.name == "createSession"), "{c:?}");
        assert!(c.symbols.iter().any(|s| s.path == "notes/new.md"), "untracked in the second root: {c:?}");
    }

    #[test]
    fn a_workspace_with_no_git_root_is_not_a_checkout() {
        let d = tempfile::tempdir().unwrap();
        crate::fixture::write_workspace(d.path());
        let ws = Workspace::open(d.path()).unwrap();
        let store = Store::open(&d.path().join(crate::engine::DB_FILE)).unwrap();
        Indexer::new(&store, &ws, &MapConfig::default()).unwrap().refresh(None).unwrap();
        let e = changed(&store, &MapConfig::default(), &ws, None).unwrap_err().to_string();
        assert!(e.contains("not a git checkout"), "{e}");
    }
```

(The `notes/new.md` symbol assertion needs Task 5's document extraction to produce a `document` symbol; until then assert `c.files_without_symbols.contains(&"notes/new.md".to_string())` instead and switch the assertion in Task 5.)

`hook.rs` tests:

```rust
    #[test]
    fn hook_denies_a_read_in_a_second_root() {
        let d = tempfile::tempdir().unwrap();
        let (_app, notes) = singularrag_core::fixture::write_workspace(d.path());
        let mut e = singularrag_core::engine::Engine::open(d.path(), "hook-test").unwrap();
        e.refresh(std::time::Duration::from_secs(60)).unwrap();
        let session = format!("ws-{}", std::process::id());
        let _ = std::fs::remove_dir_all(marker_dir(&session));
        let file = notes.join("README.md").display().to_string();
        let first = run(HookEvent::Read, Some(d.path().to_path_buf()), &input(&session, d.path(), &file));
        assert_eq!(first, DENY_JSON, "a file in the second root is indexed");
        let _ = std::fs::remove_dir_all(marker_dir(&session));
    }
```

(`README.md` is indexed only after Task 5 adds Markdown; until then use `app.join("src/auth/session.ts")` in this test and switch the path in Task 5.)

`serve/watcher.rs` tests, replacing the `interesting` assertions:

```rust
    #[test]
    fn interesting_paths_follow_the_roots_and_the_workspace_map_file() {
        let d = tempfile::tempdir().unwrap();
        let (app, notes) = singularrag_core::fixture::write_workspace(d.path());
        let ws = singularrag_core::workspace::Workspace::open(d.path()).unwrap();
        assert!(interesting(&ws, &app.join("src/a.ts")));
        assert!(interesting(&ws, &notes.join("n.md")));
        assert!(!interesting(&ws, &app.join(".git/HEAD")));
        assert!(interesting(&ws, &ws.dir.join(".singularrag/map.toml")));
        assert!(!interesting(&ws, &ws.dir.join(".singularrag/index.db")));
        assert!(!interesting(&ws, &ws.dir.join("unrelated.txt")), "the workspace dir is not a root");
    }
```

`serve/queries.rs` test:

```rust
    #[test]
    fn status_lists_the_roots() {
        let d = tempfile::tempdir().unwrap();
        singularrag_core::fixture::write_workspace(d.path());
        let mut e = singularrag_core::engine::Engine::open(d.path(), "t").unwrap();
        e.refresh(std::time::Duration::from_secs(60)).unwrap();
        let ws = e.workspace().clone();
        let s = status(e.store(), &crate::serve::state::Freshness::default(), &ws).unwrap();
        assert_eq!(s.roots.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), vec!["app", "notes"]);
        assert!(s.roots[0].git_head.is_none());
    }
```

`tests/cli.rs`:

```rust
#[test]
fn changed_in_a_named_workspace_prefixes_paths() {
    let dir = tempfile::tempdir().unwrap();
    let (app, _) = singularrag_core::fixture::write_workspace(dir.path());
    let git = |args: &[&str]| {
        assert!(std::process::Command::new("git").arg("-C").arg(&app).args(args).status().unwrap().success());
    };
    git(&["init", "-q"]);
    git(&["-c", "user.email=t@t", "-c", "user.name=t", "add", "."]);
    git(&["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "base"]);
    std::fs::write(app.join("src/util/log.ts"), "export function log(msg: string): void {\n  // changed\n  console.log(msg);\n}\n").unwrap();
    Command::cargo_bin("singularrag")
        .unwrap()
        .args(["--repo", dir.path().to_str().unwrap(), "changed"])
        .assert()
        .success()
        .stdout(predicate::str::contains("app/src/util/log.ts::log"));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --workspace 2>&1 | grep -E '^error' | head -5`
Expected: signature mismatches on `changed`, `interesting`, `status`; missing `roots` field.

- [ ] **Step 3: Implement**

`changed.rs`: signature `pub fn changed(store, config, ws: &Workspace, base: Option<&str>) -> Result<Changed>`. Replace the two `git(root, …)` calls with a loop:

```rust
    let mut ranges: Vec<(String, u32, u32)> = Vec::new();
    let mut git_roots = 0usize;
    for r in &ws.roots {
        if !r.path.join(".git").exists() {
            continue;
        }
        git_roots += 1;
        let prefix = ws.prefix(r);
        let mut args = vec![/* the fixed diff args from the current code */];
        match base {
            Some(b) => args.push(b),
            None if git(&r.path, &["rev-parse", "--verify", "-q", "HEAD"]).is_ok() => args.push("HEAD"),
            None => {}
        }
        let diff = git(&r.path, &args)?;
        let untracked = git(&r.path, &["ls-files", "--others", "--exclude-standard"])?;
        ranges.extend(parse_unified0(&diff).into_iter().map(|(p, a, b)| (format!("{prefix}{p}"), a, b)));
        for p in untracked.lines().filter(|l| !l.is_empty()) {
            ranges.push((format!("{prefix}{p}"), 1, u32::MAX));
        }
    }
    if git_roots == 0 {
        return Err(Error::Config("not a git checkout".into()));
    }
```

The rest of the function is unchanged. `Engine::changed` passes `self.workspace()`. Existing tests pass `&Workspace::single(dir.path()).unwrap()`.

`hook.rs`: replace the root and strip_prefix block:

```rust
            let root = repo.or_else(|| v.get("cwd").and_then(Value::as_str).map(PathBuf::from));
            let Some(root) = root else { return ALLOW_JSON.to_string() };
            let Ok(ws) = singularrag_core::workspace::Workspace::open(&root) else {
                return ALLOW_JSON.to_string();
            };
            …
            let Ok(abs) = Path::new(file).canonicalize() else { return ALLOW_JSON.to_string() };
            let Some(rel) = ws.rel_of(&abs) else { return ALLOW_JSON.to_string() };
            let db = ws.dir.join(singularrag_core::engine::DB_FILE);
```

`serve/state.rs`: `pub ws: Workspace` filled in `AppState::new` by `Workspace::open(&root)?` (keep `root`). `serve/watcher.rs`:

```rust
fn interesting(ws: &Workspace, p: &Path) -> bool {
    if p == ws.dir.join(singularrag_core::config::MAP_FILE) {
        return true;
    }
    let Some(rel) = ws.rel_of(p) else { return false };
    let inner = rel.split_once('/').map(|(_, r)| r).filter(|_| ws.is_named()).unwrap_or(&rel);
    !inner.starts_with(".git") && !inner.starts_with(".singularrag")
}
```

and in `start`: `for r in &state.ws.roots { debouncer.watch(&r.path, RecursiveMode::Recursive)?; }` plus, when `ws.is_named()`, `debouncer.watch(&state.ws.dir.join(".singularrag"), RecursiveMode::NonRecursive)?;` (create the directory first with `create_dir_all`; it exists because the index lives there). The handler closure captures `ws.clone()`.

`serve/queries.rs`:

```rust
#[derive(Debug, Clone, Serialize)]
pub struct RootDto {
    pub name: String,
    pub path: String,
    pub git_head: Option<String>,
}
```

`StatusDto.roots: Vec<RootDto>`; `status(store, f, ws)` fills it: for each root, `git_head` from `git_head:<name>` (named) or `git_head` (unnamed), empty → `None`. The route passes `&s.ws`. The UI `Status` type gains `roots: { name: string; path: string; git_head: string | null }[]` (`ui/src/api/types.ts`) and the App test's `status` fixture gains `roots: [{ name: "", path: "/repo", git_head: "9b1e0d4f" }]`.

- [ ] **Step 4: Run the tests**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace 2>&1 | grep -E 'test result|panicked|FAILED'`; then `cd /Users/jonasbroms/Sites/singularrag/.claude/worktrees/ui-v0/ui && bun run typecheck && bun test 2>&1 | tail -3`
Expected: all `ok`; UI typecheck clean after the `Status` fixture change.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: hook, changed, the watcher and serve status follow the workspace roots"
```

---

### Task 4: Document extraction (`doc.rs`)

**Files:**
- Modify: `Cargo.toml` (workspace deps) and `crates/singularrag-core/Cargo.toml`: `tree-sitter-md = "0.5"`, `tree-sitter-html = "0.23"`, `tree-sitter-css = "0.25"`, `tree-sitter-json = "0.24"`, `tree-sitter-yaml = "0.7"`, `tree-sitter-toml-ng = "0.7"`
- Create: `crates/singularrag-core/src/doc.rs`
- Modify: `crates/singularrag-core/src/lang.rs` (variants, `from_path`, `as_str`, `is_document`, dispatch in `extract_tags`)
- Modify: `crates/singularrag-core/src/lib.rs` (`pub mod doc;`)

**Interfaces:**
- Consumes: `lang::Tag`.
- Produces: `Language::{Markdown, Html, Css, Json, Yaml, Toml, Text}`; `Language::is_document(self) -> bool`; `doc::DocExtract { pub tags: Vec<Tag>, pub bodies: Vec<(usize, String)>, pub mentions: Vec<(String, u32)> }` where `bodies` pairs an index into `tags` with that section's text and `mentions` are `(name, line)`; `doc::extract(lang: Language, source: &str, stem: &str) -> Result<DocExtract>`; `doc::DOC_MAX_BYTES: u64`; `doc::skip_reason(rel_path: &str, size: u64, source: &str) -> Option<&'static str>` (`lockfile`, `too-large`, `minified`); `doc::is_lockfile(basename: &str) -> bool`. `lang::extract_tags` for a document language returns `extract(...).tags` (bodies and mentions are only reachable through `doc::extract`, which the indexer calls for documents).

Node names, verified on 2026-09-23 against these crate versions with a scratch parse: Markdown `document > section > atx_heading (atx_h1_marker … atx_h6_marker, heading_content: inline)`, `setext_heading`, `fenced_code_block`, `paragraph`; HTML `document > element > start_tag (tag_name, attribute (attribute_name, quoted_attribute_value (attribute_value)))`, `text`, `end_tag`; CSS `stylesheet > rule_set (selectors, block)`, `media_statement (… block (rule_set))`; JSON `document > object > pair (key: string (string_content), value: …)` with value kinds `object`, `array`, `string`, `number`, `true`, `false`, `null`; YAML `stream > document > block_node > block_mapping > block_mapping_pair (key: flow_node (plain_scalar (string_scalar)), value: flow_node | block_node (block_mapping | block_sequence))`; TOML `document > pair (bare_key, value)`, `table (bare_key | dotted_key (bare_key …), pair …)`, `table_array_element (bare_key, pair …)`. If a grammar's crate exposes `LANGUAGE` under a different name, `cargo doc` for that crate says which; the Markdown crate exposes `tree_sitter_md::LANGUAGE` (block grammar) and `INLINE_LANGUAGE`, and only the block grammar is used.

- [ ] **Step 1: Write the failing tests**

`doc.rs` tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const MD: &str = "# Title\n\nIntro `createSession` and [[design|the design]] and [x](../a/b.md#f) and [ext](https://x.io/y.md).\n\n```ts\nconst hidden = 1;\n```\n\n## Sub\n\nsub text mentions `SessionStore()`.\n\n#### Deep\n\nfolded into Sub.\n\n## Sub2\n\n<code>inline</code> with `not an identifier here`.\n";

    fn find<'a>(d: &'a DocExtract, kind: &str, name: &str) -> &'a Tag {
        d.tags.iter().find(|t| t.kind == kind && t.name == name).unwrap_or_else(|| panic!("{kind} {name} in {:?}", d.tags))
    }

    #[test]
    fn markdown_sections_fold_h4_and_carry_bodies() {
        let d = extract(Language::Markdown, MD, "readme").unwrap();
        let title = find(&d, "section", "Title");
        assert_eq!((title.line_start, title.line_end), (1, 21));
        assert_eq!(title.signature, "# Title");
        let sub = find(&d, "section", "Sub");
        assert_eq!((sub.line_start, sub.line_end), (11, 17), "h4 folds into its parent");
        assert!(d.tags.iter().all(|t| t.name != "Deep"));
        let doc = find(&d, "document", "readme");
        assert_eq!((doc.line_start, doc.line_end), (1, 21));
        let body = |name: &str| {
            let i = d.tags.iter().position(|t| t.name == name).unwrap();
            d.bodies.iter().find(|(j, _)| *j == i).map(|(_, b)| b.clone()).unwrap_or_default()
        };
        assert!(body("Sub").contains("sub text") && body("Sub").contains("folded into Sub"));
        assert!(!body("Title").contains("sub text"), "a child section's text is not repeated in the parent");
        assert!(!body("Title").contains("hidden"), "fenced code is stripped");
        assert!(d.tags.iter().all(|t| t.is_definition));
    }

    #[test]
    fn markdown_mentions_are_code_spans_wiki_links_and_relative_link_stems() {
        let d = extract(Language::Markdown, MD, "readme").unwrap();
        let names: Vec<&str> = d.mentions.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"createSession"), "{names:?}");
        assert!(names.contains(&"design"), "wiki link before |: {names:?}");
        assert!(names.contains(&"b"), "relative link stem: {names:?}");
        assert!(names.contains(&"SessionStore"), "trailing () dropped: {names:?}");
        assert!(names.contains(&"inline"), "<code> in markdown: {names:?}");
        assert!(!names.contains(&"y"), "absolute URL ignored: {names:?}");
        assert!(!names.contains(&"hidden"), "fenced code ignored: {names:?}");
        assert!(!names.iter().any(|n| n.contains(' ')), "{names:?}");
        assert_eq!(d.mentions.iter().find(|(n, _)| n == "createSession").unwrap().1, 3);
    }

    #[test]
    fn setext_and_missing_h1_still_nest() {
        let src = "## Second level first\n\ntext\n\nSetext\n======\n\nmore\n";
        let d = extract(Language::Markdown, src, "odd").unwrap();
        assert!(d.tags.iter().any(|t| t.kind == "section" && t.name == "Second level first"));
        assert!(d.tags.iter().any(|t| t.kind == "section" && t.name == "Setext"));
        assert!(d.tags.iter().any(|t| t.kind == "document" && t.name == "odd"));
        let none = extract(Language::Markdown, "just a paragraph\n", "plain").unwrap();
        assert_eq!(none.tags.len(), 1);
        assert_eq!(none.tags[0].kind, "document");
        assert_eq!(none.tags[0].signature, "just a paragraph");
    }

    #[test]
    fn html_headings_ids_code_and_hrefs() {
        let src = "<html><body><h1>Title</h1><div id=\"main\"><p>Hi <code>foo</code> <a href=\"../docs/x.md\">x</a> <a href=\"https://e.io/z.md\">z</a></p></div><h2 id=\"s\">Sec</h2></body></html>";
        let d = extract(Language::Html, src, "page").unwrap();
        assert_eq!(find(&d, "section", "Title").signature, "Title");
        assert_eq!(find(&d, "element", "main").signature, "<div id=\"main\">");
        assert!(d.tags.iter().any(|t| t.kind == "section" && t.name == "Sec"));
        assert!(d.tags.iter().any(|t| t.kind == "element" && t.name == "s"));
        let names: Vec<&str> = d.mentions.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["foo", "x"]);
        let i = d.tags.iter().position(|t| t.kind == "document").unwrap();
        let body = &d.bodies.iter().find(|(j, _)| *j == i).unwrap().1;
        assert!(body.contains("Hi") && body.contains("Sec") && !body.contains("<"), "{body}");
    }

    #[test]
    fn css_rules_and_media_fold() {
        let src = ".a, .b > p { color: red }\n@media (max-width: 1px) { .c { x: y } }\n#id:hover { a: b }\n";
        let d = extract(Language::Css, src, "style").unwrap();
        let names: Vec<(&str, &str)> = d.tags.iter().map(|t| (t.kind.as_str(), t.name.as_str())).collect();
        assert!(names.contains(&("rule", ".a, .b > p")), "{names:?}");
        assert!(names.contains(&("rule", "@media (max-width: 1px)")), "{names:?}");
        assert!(names.contains(&("rule", "#id:hover")), "{names:?}");
        assert!(!names.contains(&("rule", ".c")), "folded: {names:?}");
        assert!(d.bodies.is_empty() || d.bodies.iter().all(|(i, _)| d.tags[*i].kind == "document"));
    }

    #[test]
    fn json_yaml_toml_keys_to_depth_two_without_values() {
        let json = "{\"name\": \"x\", \"scripts\": {\"build\": \"tsc\", \"deep\": {\"more\": 1}}, \"arr\": [1], \"n\": 3, \"b\": true, \"token\": \"sk-secret\"}";
        let d = extract(Language::Json, json, "package").unwrap();
        let sigs: Vec<(&str, &str)> = d.tags.iter().filter(|t| t.kind == "key").map(|t| (t.name.as_str(), t.signature.as_str())).collect();
        assert!(sigs.contains(&("name", "name: string")), "{sigs:?}");
        assert!(sigs.contains(&("scripts", "scripts: object")), "{sigs:?}");
        assert!(sigs.contains(&("scripts.build", "scripts.build: string")), "{sigs:?}");
        assert!(sigs.contains(&("scripts.deep", "scripts.deep: object")), "{sigs:?}");
        assert!(!sigs.iter().any(|(n, _)| n.contains("more")), "depth 3 dropped: {sigs:?}");
        assert!(sigs.contains(&("arr", "arr: array")) && sigs.contains(&("n", "n: number")) && sigs.contains(&("b", "b: bool")), "{sigs:?}");
        let all = format!("{:?}{:?}", d.tags, d.bodies);
        assert!(!all.contains("sk-secret") && !all.contains("tsc"), "values never leave the parser: {all}");
        assert!(d.mentions.is_empty());

        let yaml = "name: x\nscripts:\n  build: tsc\n  deep:\n    more: 1\narr:\n  - 1\n";
        let d = extract(Language::Yaml, yaml, "ci").unwrap();
        let names: Vec<&str> = d.tags.iter().filter(|t| t.kind == "key").map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["name", "scripts", "scripts.build", "scripts.deep", "arr"]);
        assert_eq!(find(&d, "key", "scripts.deep").signature, "scripts.deep: object");

        let toml = "name = \"x\"\n[scripts]\nbuild = \"tsc\"\n[scripts.deep]\nmore = 1\n[[bin]]\nname = \"a\"\n";
        let d = extract(Language::Toml, toml, "Cargo").unwrap();
        let names: Vec<&str> = d.tags.iter().filter(|t| t.kind == "key").map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["name", "scripts", "scripts.build", "scripts.deep", "bin", "bin.name"]);
        assert_eq!(find(&d, "key", "scripts").signature, "scripts: object");
        assert_eq!(find(&d, "key", "bin").signature, "bin: array");
    }

    #[test]
    fn top_level_array_and_multi_document_yaml() {
        let d = extract(Language::Json, "[1, 2, {\"a\": 1}]", "list").unwrap();
        assert_eq!(d.tags.len(), 1);
        assert_eq!(d.tags[0].kind, "document");
        let d = extract(Language::Yaml, "a: 1\n---\nb: 2\n", "multi").unwrap();
        let names: Vec<&str> = d.tags.iter().filter(|t| t.kind == "key").map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[test]
    fn plain_text_is_one_document_with_a_body() {
        let d = extract(Language::Text, "First line here.\nSecond `ident` line.\n", "notes").unwrap();
        assert_eq!(d.tags.len(), 1);
        assert_eq!(d.tags[0].signature, "First line here.");
        assert_eq!(d.bodies[0].1.trim(), "First line here.\nSecond `ident` line.");
        assert_eq!(d.mentions, vec![("ident".to_string(), 2)]);
    }

    #[test]
    fn skip_reasons() {
        assert!(is_lockfile("package-lock.json") && is_lockfile("Cargo.lock") && is_lockfile("bun.lockb"));
        assert!(!is_lockfile("lock.md"));
        assert_eq!(skip_reason("a/pnpm-lock.yaml", 10, "x"), Some("lockfile"));
        assert_eq!(skip_reason("a/b.json", DOC_MAX_BYTES + 1, "x"), Some("too-large"));
        let minified = "a".repeat(600);
        assert_eq!(skip_reason("a/b.css", 600, &minified), Some("minified"));
        assert_eq!(skip_reason("a/b.md", 10, "# ok\n"), None);
    }
}
```

`lang.rs` tests: extend `detects_language_from_extension` with `assert_eq!(Language::from_path("README.md"), Some(Language::Markdown));` (replacing the current `None` assertion), `("a/b.yml", Some(Language::Yaml))`, `("x.htm", Some(Language::Html))`, `("notes.txt", Some(Language::Text))`, and `assert!(Language::Markdown.is_document() && !Language::Rust.is_document());`.

- [ ] **Step 2: Add the crates and run the tests to verify they fail**

Add the six crates to both `Cargo.toml` files (workspace `[workspace.dependencies]` with versions, crate with `{ workspace = true }`).

Run: `cargo build -p singularrag-core 2>&1 | tail -3 && cargo test -p singularrag-core --lib doc:: 2>&1 | grep -E '^error|test result' | head -5`
Expected: the build resolves the six crates (if a version does not exist against tree-sitter 0.27, `cargo build` says so and the next patch version is used; record the pinned versions in the commit); tests fail with `cannot find function extract`.

- [ ] **Step 3: Implement**

`lang.rs`:

```rust
pub enum Language { TypeScript, Tsx, JavaScript, Rust, Markdown, Html, Css, Json, Yaml, Toml, Text }
// from_path: "md" | "markdown" => Markdown, "html" | "htm" => Html, "css" => Css, "json" => Json,
// "yaml" | "yml" => Yaml, "toml" => Toml, "txt" => Text
// as_str: "markdown", "html", "css", "json", "yaml", "toml", "text"
impl Language {
    pub fn is_document(self) -> bool {
        matches!(self, Language::Markdown | Language::Html | Language::Css | Language::Json | Language::Yaml | Language::Toml | Language::Text)
    }
}
```

In `extract_tags`, first line: `if lang.is_document() { return Ok(crate::doc::extract(lang, source, "")?.tags); }` (the stem is only meaningful through `doc::extract`; the indexer calls that directly). `make_config` keeps its four code arms.

`doc.rs`:

```rust
//! Text documents as sections (documents spec §3, §4): tree-sitter trees walked
//! directly, one `Tag` per section, key or rule, the section text for FTS, and the
//! mentions that become `refs`. Nothing here needs a model.

use regex::Regex;
use std::sync::OnceLock;
use tree_sitter::{Node, Parser};

use crate::lang::{Language, Tag, SIGNATURE_MAX};
use crate::{Error, Result};

pub const DOC_MAX_BYTES: u64 = 256 * 1024;
const MAX_HEADING_LEVEL: usize = 3;
const MAX_KEY_DEPTH: usize = 2;
const MINIFIED_AVG_LINE: usize = 500;
const LOCKFILES: [&str; 9] = ["package-lock.json", "yarn.lock", "pnpm-lock.yaml", "Cargo.lock", "bun.lock", "bun.lockb", "composer.lock", "Gemfile.lock", "poetry.lock"];

#[derive(Debug, Default)]
pub struct DocExtract {
    pub tags: Vec<Tag>,
    /// `(index into tags, section text)` for `section`, `document` and `element` tags.
    pub bodies: Vec<(usize, String)>,
    /// `(name, 1-based line)`.
    pub mentions: Vec<(String, u32)>,
}

pub fn is_lockfile(basename: &str) -> bool {
    LOCKFILES.contains(&basename)
}

pub fn skip_reason(rel_path: &str, size: u64, source: &str) -> Option<&'static str> {
    let base = rel_path.rsplit('/').next().unwrap_or(rel_path);
    if is_lockfile(base) {
        return Some("lockfile");
    }
    if size > DOC_MAX_BYTES {
        return Some("too-large");
    }
    let lines = source.lines().count().max(1);
    if source.len() / lines > MINIFIED_AVG_LINE {
        return Some("minified");
    }
    None
}

fn line_of(source: &str, byte: usize) -> u32 {
    source.as_bytes()[..byte.min(source.len())].iter().filter(|&&b| b == b'\n').count() as u32 + 1
}

fn end_line_of(source: &str, end_byte: usize) -> u32 {
    line_of(source, end_byte.saturating_sub(1))
}

fn truncate(s: &str) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() > SIGNATURE_MAX {
        let mut t: String = s.chars().take(SIGNATURE_MAX - 1).collect();
        t.push('…');
        t
    } else {
        s
    }
}

fn first_line(source: &str) -> String {
    truncate(source.lines().find(|l| !l.trim().is_empty()).unwrap_or(""))
}

fn parse(lang: tree_sitter::Language, source: &str) -> Result<tree_sitter::Tree> {
    let mut p = Parser::new();
    p.set_language(&lang).map_err(|e| Error::Tags(format!("{e:?}")))?;
    p.parse(source, None).ok_or_else(|| Error::Tags("parse returned nothing".into()))
}

fn tag(name: &str, kind: &str, source: &str, start: usize, end: usize, signature: String) -> Tag {
    Tag {
        name: name.to_string(),
        kind: kind.to_string(),
        is_definition: true,
        line_start: line_of(source, start),
        line_end: end_line_of(source, end).max(line_of(source, start)),
        signature,
    }
}

fn document_tag(stem: &str, source: &str) -> Tag {
    tag(stem, "document", source, 0, source.len().max(1), first_line(source))
}

/// Mentions in prose (spec §4). `offset` is the byte offset of `text` inside the file,
/// for line numbers.
fn mentions_in(text: &str, offset: usize, source: &str, out: &mut Vec<(String, u32)>) {
    static CODE: OnceLock<Regex> = OnceLock::new();
    static WIKI: OnceLock<Regex> = OnceLock::new();
    static LINK: OnceLock<Regex> = OnceLock::new();
    static IDENT: OnceLock<Regex> = OnceLock::new();
    let code = CODE.get_or_init(|| Regex::new(r"`([^`\n]+)`|<code>([^<\n]+)</code>").unwrap());
    let wiki = WIKI.get_or_init(|| Regex::new(r"\[\[([^\]\|#]+)(?:[#|][^\]]*)?\]\]").unwrap());
    let link = LINK.get_or_init(|| Regex::new(r"\]\(([^)\s]+)\)|href=\x22([^\x22]+)\x22").unwrap());
    let ident = IDENT.get_or_init(|| Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*(\(\))?$").unwrap());
    for m in code.captures_iter(text) {
        let g = m.get(1).or_else(|| m.get(2)).unwrap();
        let t = g.as_str().trim();
        if ident.is_match(t) {
            out.push((t.trim_end_matches("()").to_string(), line_of(source, offset + g.start())));
        }
    }
    for m in wiki.captures_iter(text) {
        let g = m.get(1).unwrap();
        out.push((g.as_str().trim().to_string(), line_of(source, offset + g.start())));
    }
    for m in link.captures_iter(text) {
        let g = m.get(1).or_else(|| m.get(2)).unwrap();
        let target = g.as_str();
        if target.contains("://") || target.starts_with("mailto:") || target.starts_with('#') {
            continue;
        }
        let path = target.split('#').next().unwrap_or("");
        let file = path.rsplit('/').next().unwrap_or(path);
        let stem = file.rsplit_once('.').map(|(s, _)| s).unwrap_or(file);
        if !stem.is_empty() {
            out.push((stem.to_string(), line_of(source, offset + g.start())));
        }
    }
}

/// `text` with the byte ranges in `cuts` blanked (kept the same length so offsets hold).
fn blank(text: &str, base: usize, cuts: &[(usize, usize)]) -> String {
    let mut bytes = text.as_bytes().to_vec();
    for &(s, e) in cuts {
        let (s, e) = (s.saturating_sub(base).min(bytes.len()), e.saturating_sub(base).min(bytes.len()));
        for b in &mut bytes[s..e] {
            if *b != b'\n' {
                *b = b' ';
            }
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

pub fn extract(lang: Language, source: &str, stem: &str) -> Result<DocExtract> {
    match lang {
        Language::Markdown => markdown(source, stem),
        Language::Html => html(source, stem),
        Language::Css => css(source, stem),
        Language::Json => json(source, stem),
        Language::Yaml => yaml(source, stem),
        Language::Toml => toml(source, stem),
        Language::Text => {
            let mut d = DocExtract::default();
            d.tags.push(document_tag(stem, source));
            d.bodies.push((0, source.to_string()));
            mentions_in(source, 0, source, &mut d.mentions);
            Ok(d)
        }
        _ => Err(Error::Tags(format!("{lang:?} is not a document language"))),
    }
}

// ---------- Markdown ----------

fn heading_level(h: Node) -> usize {
    let mut c = h.walk();
    for child in h.children(&mut c) {
        let k = child.kind();
        if let Some(n) = k.strip_prefix("atx_h").and_then(|r| r.strip_suffix("_marker")) {
            return n.parse().unwrap_or(1);
        }
        if k == "setext_h1_underline" { return 1; }
        if k == "setext_h2_underline" { return 2; }
    }
    1
}

fn heading_text(h: Node, source: &str) -> String {
    let mut c = h.walk();
    for child in h.children(&mut c) {
        if child.kind() == "inline" || child.kind() == "paragraph" {
            return truncate(&source[child.byte_range()]);
        }
    }
    truncate(source[h.byte_range()].trim_start_matches('#'))
}

fn markdown(source: &str, stem: &str) -> Result<DocExtract> {
    let tree = parse(tree_sitter_md::LANGUAGE.into(), source)?;
    let mut d = DocExtract::default();
    let mut fences: Vec<(usize, usize)> = Vec::new();
    collect_kind(tree.root_node(), "fenced_code_block", &mut fences);
    d.tags.push(document_tag(stem, source));
    // The document body is the whole prose minus fences; sections get their own bodies
    // below and the document row keeps only what no section owns (the preamble).
    let mut own_sections: Vec<(usize, usize)> = Vec::new();
    walk_sections(tree.root_node(), source, &fences, &mut d, &mut own_sections);
    let mut cuts = fences.clone();
    cuts.extend(own_sections.iter().copied());
    d.bodies.insert(0, (0, blank(source, 0, &cuts)));
    let prose = blank(source, 0, &fences);
    mentions_in(&prose, 0, source, &mut d.mentions);
    Ok(d)
}

fn collect_kind(n: Node, kind: &str, out: &mut Vec<(usize, usize)>) {
    if n.kind() == kind {
        out.push((n.start_byte(), n.end_byte()));
        return;
    }
    let mut c = n.walk();
    for child in n.children(&mut c) {
        collect_kind(child, kind, out);
    }
}

fn walk_sections(n: Node, source: &str, fences: &[(usize, usize)], d: &mut DocExtract, own: &mut Vec<(usize, usize)>) {
    let mut c = n.walk();
    for child in n.children(&mut c) {
        if child.kind() != "section" {
            continue;
        }
        let heading = child.child(0).filter(|h| h.kind() == "atx_heading" || h.kind() == "setext_heading");
        let Some(h) = heading else {
            walk_sections(child, source, fences, d, own);
            continue;
        };
        let level = heading_level(h);
        if level > MAX_HEADING_LEVEL {
            continue; // folded into the parent, which already spans it
        }
        let name = heading_text(h, source);
        if name.is_empty() {
            continue;
        }
        let sig = truncate(&source[h.byte_range()]);
        let t = tag(&name, "section", source, child.start_byte(), child.end_byte(), sig);
        let idx = d.tags.len();
        d.tags.push(t);
        own.push((child.start_byte(), child.end_byte()));
        // Body: this section minus fences minus child sections that get their own tag.
        let mut child_own: Vec<(usize, usize)> = Vec::new();
        walk_sections(child, source, fences, d, &mut child_own);
        let mut cuts: Vec<(usize, usize)> = fences.to_vec();
        cuts.extend(child_own);
        cuts.push((h.start_byte(), h.end_byte()));
        d.bodies.push((idx, blank(&source[child.byte_range()], child.start_byte(), &cuts)));
    }
}
```

The `d.bodies.insert(0, …)` for the document row must be done after the sections so `own` is complete; the index `0` refers to the document tag pushed first. HTML, CSS, JSON, YAML, TOML follow the same shape:

```rust
// ---------- HTML ----------

fn attr(start_tag: Node, name: &str, source: &str) -> Option<String> {
    let mut c = start_tag.walk();
    for a in start_tag.children(&mut c) {
        if a.kind() != "attribute" { continue; }
        let mut ac = a.walk();
        let kids: Vec<Node> = a.children(&mut ac).collect();
        let an = kids.iter().find(|k| k.kind() == "attribute_name")?;
        if &source[an.byte_range()] != name { continue; }
        let val = kids.iter().find(|k| k.kind() == "quoted_attribute_value" || k.kind() == "attribute_value")?;
        return Some(source[val.byte_range()].trim_matches('"').trim_matches('\'').to_string());
    }
    None
}

fn text_of(n: Node, source: &str, out: &mut String) {
    if n.kind() == "text" {
        out.push_str(source[n.byte_range()].trim());
        out.push(' ');
        return;
    }
    let mut c = n.walk();
    for child in n.children(&mut c) {
        text_of(child, source, out);
    }
}

fn html(source: &str, stem: &str) -> Result<DocExtract> {
    let tree = parse(tree_sitter_html::LANGUAGE.into(), source)?;
    let mut d = DocExtract::default();
    d.tags.push(document_tag(stem, source));
    let mut body = String::new();
    text_of(tree.root_node(), source, &mut body);
    d.bodies.push((0, body));
    let mut stack = vec![tree.root_node()];
    while let Some(n) = stack.pop() {
        if n.kind() == "element" {
            if let Some(st) = n.child(0).filter(|c| c.kind() == "start_tag") {
                let tag_name = st.child(1).map(|t| source[t.byte_range()].to_string()).unwrap_or_default();
                let mut txt = String::new();
                text_of(n, source, &mut txt);
                let txt = truncate(&txt);
                if matches!(tag_name.as_str(), "h1" | "h2" | "h3" | "h4" | "h5" | "h6") && !txt.is_empty() {
                    let idx = d.tags.len();
                    d.tags.push(tag(&txt, "section", source, n.start_byte(), n.end_byte(), txt.clone()));
                    d.bodies.push((idx, txt.clone()));
                }
                if let Some(id) = attr(st, "id", source) {
                    let idx = d.tags.len();
                    d.tags.push(tag(&id, "element", source, n.start_byte(), n.end_byte(), format!("<{tag_name} id=\"{id}\">")));
                    d.bodies.push((idx, txt));
                }
            }
        }
        let mut c = n.walk();
        let kids: Vec<Node> = n.children(&mut c).collect();
        stack.extend(kids.into_iter().rev());
    }
    mentions_in(source, 0, source, &mut d.mentions);
    Ok(d)
}

// ---------- CSS ----------

fn css(source: &str, stem: &str) -> Result<DocExtract> {
    let tree = parse(tree_sitter_css::LANGUAGE.into(), source)?;
    let mut d = DocExtract::default();
    d.tags.push(document_tag(stem, source));
    let root = tree.root_node();
    let mut c = root.walk();
    for n in root.children(&mut c) {
        match n.kind() {
            "rule_set" => {
                if let Some(sel) = n.child(0).filter(|s| s.kind() == "selectors") {
                    let name = truncate(&source[sel.byte_range()]);
                    d.tags.push(tag(&name, "rule", source, n.start_byte(), n.end_byte(), name.clone()));
                }
            }
            "media_statement" | "supports_statement" | "keyframes_statement" => {
                let block = n.child_by_field_name("body").or_else(|| { let mut cc = n.walk(); n.children(&mut cc).find(|k| k.kind() == "block" || k.kind() == "keyframe_block_list") });
                let prelude_end = block.map(|b| b.start_byte()).unwrap_or(n.end_byte());
                let name = truncate(&source[n.start_byte()..prelude_end]);
                d.tags.push(tag(&name, "rule", source, n.start_byte(), n.end_byte(), name.clone()));
            }
            _ => {}
        }
    }
    Ok(d)
}

// ---------- JSON / YAML / TOML ----------

fn type_word(kind: &str) -> &'static str {
    match kind {
        "object" | "block_mapping" | "flow_mapping" | "table" | "inline_table" => "object",
        "array" | "block_sequence" | "flow_sequence" | "table_array_element" => "array",
        "string" | "string_scalar" | "double_quote_scalar" | "single_quote_scalar" | "block_scalar" => "string",
        "number" | "integer" | "float" | "integer_scalar" | "float_scalar" => "number",
        "true" | "false" | "boolean_scalar" | "boolean" => "bool",
        _ => "string",
    }
}

fn push_key(d: &mut DocExtract, source: &str, path: &str, value_kind: &str, start: usize, end: usize) {
    d.tags.push(tag(path, "key", source, start, end, format!("{path}: {}", type_word(value_kind))));
}

/// Unquoted key text of a YAML/JSON key node.
fn key_text(n: Node, source: &str) -> String {
    source[n.byte_range()].trim().trim_matches('"').trim_matches('\'').to_string()
}

/// The innermost value node: YAML wraps values in `flow_node`/`block_node`.
fn unwrap_value(n: Node) -> Node {
    let mut v = n;
    while matches!(v.kind(), "flow_node" | "block_node") {
        match v.child(0) { Some(c) => v = c, None => break }
    }
    v
}

fn json_object(obj: Node, source: &str, prefix: &str, depth: usize, d: &mut DocExtract) {
    let mut c = obj.walk();
    for pair in obj.children(&mut c) {
        if pair.kind() != "pair" { continue; }
        let (Some(k), Some(v)) = (pair.child_by_field_name("key"), pair.child_by_field_name("value")) else { continue };
        let path = if prefix.is_empty() { key_text(k, source) } else { format!("{prefix}.{}", key_text(k, source)) };
        push_key(d, source, &path, v.kind(), pair.start_byte(), pair.end_byte());
        if v.kind() == "object" && depth < MAX_KEY_DEPTH {
            json_object(v, source, &path, depth + 1, d);
        }
    }
}

fn json(source: &str, stem: &str) -> Result<DocExtract> {
    let tree = parse(tree_sitter_json::LANGUAGE.into(), source)?;
    let mut d = DocExtract::default();
    d.tags.push(document_tag(stem, source));
    if let Some(obj) = tree.root_node().child(0).filter(|n| n.kind() == "object") {
        json_object(obj, source, "", 1, &mut d);
    }
    Ok(d)
}

fn yaml_mapping(map: Node, source: &str, prefix: &str, depth: usize, d: &mut DocExtract) {
    let mut c = map.walk();
    for pair in map.children(&mut c) {
        if pair.kind() != "block_mapping_pair" && pair.kind() != "flow_pair" { continue; }
        let Some(k) = pair.child_by_field_name("key") else { continue };
        let path = if prefix.is_empty() { key_text(k, source) } else { format!("{prefix}.{}", key_text(k, source)) };
        let v = pair.child_by_field_name("value").map(unwrap_value);
        push_key(d, source, &path, v.map(|v| v.kind()).unwrap_or("string"), pair.start_byte(), pair.end_byte());
        if let Some(v) = v {
            if v.kind() == "block_mapping" && depth < MAX_KEY_DEPTH {
                yaml_mapping(v, source, &path, depth + 1, d);
            }
        }
    }
}

fn yaml(source: &str, stem: &str) -> Result<DocExtract> {
    let tree = parse(tree_sitter_yaml::LANGUAGE.into(), source)?;
    let mut d = DocExtract::default();
    d.tags.push(document_tag(stem, source));
    let mut c = tree.root_node().walk();
    for doc in tree.root_node().children(&mut c) {
        if doc.kind() != "document" { continue; }
        let mut dc = doc.walk();
        for n in doc.children(&mut dc) {
            let v = unwrap_value(n);
            if v.kind() == "block_mapping" {
                yaml_mapping(v, source, "", 1, &mut d);
            }
        }
    }
    Ok(d)
}

fn toml_key_path(n: Node, source: &str) -> String {
    match n.kind() {
        "dotted_key" => {
            let mut c = n.walk();
            n.children(&mut c).filter(|k| k.kind() != ".").map(|k| key_text(k, source)).collect::<Vec<_>>().join(".")
        }
        _ => key_text(n, source),
    }
}

fn toml_pairs(container: Node, source: &str, prefix: &str, depth: usize, d: &mut DocExtract) {
    let mut c = container.walk();
    for pair in container.children(&mut c) {
        if pair.kind() != "pair" { continue; }
        let mut pc = pair.walk();
        let kids: Vec<Node> = pair.children(&mut pc).collect();
        let Some(k) = kids.iter().find(|k| matches!(k.kind(), "bare_key" | "quoted_key" | "dotted_key")) else { continue };
        let v = kids.iter().rev().find(|k| !matches!(k.kind(), "bare_key" | "quoted_key" | "dotted_key" | "="));
        let path = if prefix.is_empty() { toml_key_path(*k, source) } else { format!("{prefix}.{}", toml_key_path(*k, source)) };
        if path.matches('.').count() >= MAX_KEY_DEPTH { continue; }
        push_key(d, source, &path, v.map(|v| v.kind()).unwrap_or("string"), pair.start_byte(), pair.end_byte());
        let _ = depth;
    }
}

fn toml(source: &str, stem: &str) -> Result<DocExtract> {
    let tree = parse(tree_sitter_toml_ng::LANGUAGE.into(), source)?;
    let mut d = DocExtract::default();
    d.tags.push(document_tag(stem, source));
    let root = tree.root_node();
    toml_pairs(root, source, "", 1, &mut d);
    let mut seen_tables: Vec<String> = Vec::new();
    let mut c = root.walk();
    for n in root.children(&mut c) {
        if n.kind() != "table" && n.kind() != "table_array_element" { continue; }
        let Some(k) = n.child(1).filter(|k| matches!(k.kind(), "bare_key" | "quoted_key" | "dotted_key")) else { continue };
        let path = toml_key_path(k, source);
        if path.matches('.').count() >= MAX_KEY_DEPTH { continue; }
        if !seen_tables.contains(&path) {
            push_key(&mut d, source, &path, n.kind(), n.start_byte(), n.end_byte());
            seen_tables.push(path.clone());
        }
        toml_pairs(n, source, &path, 2, &mut d);
    }
    Ok(d)
}
```

If a child index (`n.child(1)` for the TOML table key, `st.child(1)` for the HTML tag name) does not hold the expected node, print `n.to_sexp()` in the failing test and adjust the index; the sexp shapes above are the ones observed. `lib.rs`: `pub mod doc;`.

- [ ] **Step 4: Run the tests**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test -p singularrag-core --lib doc:: lang:: 2>&1 | grep -E 'test result|panicked|FAILED'`
Expected: all 9 doc tests and the lang tests pass. Then `cargo test --workspace 2>&1 | grep -E 'FAILED|panicked'` must print nothing: `README.md` in the TS fixture is now a Markdown file and will be indexed from Task 5 on, which changes no existing count until then because `index_file` still routes documents through `extract_tags` with an empty stem (the `document` symbol is named `""` and skipped by the `name.is_empty()` guard the indexer applies in Task 5; until then a document symbol named `""` may appear: if any existing test counts symbols on the TS fixture and fails, add `.filter(|t| !t.name.is_empty())` in `index_file`'s tag loop now).

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(core): document extraction for markdown, html, css, json, yaml, toml and text"
```

---

### Task 5: The indexer writes documents

**Files:**
- Modify: `crates/singularrag-core/src/store/schema.rs` (`sections_fts`, `SCHEMA_VERSION = 3`)
- Modify: `crates/singularrag-core/src/index.rs` (document path in `index_file`; `delete_symbols_for`)
- Modify: `crates/singularrag-core/src/fixture.rs` (`write_docs_mini`; `write_workspace` notes root grows)
- Modify: `crates/singularrag-core/src/changed.rs` and `crates/singularrag/src/hook.rs` tests (switch the two deferred assertions from Task 3)

**Interfaces:**
- Consumes: `doc::extract`, `doc::skip_reason`, `Language::is_document`.
- Produces: `sections_fts` rows with `rowid = symbols.id`; `refs` rows from mentions; `fixture::write_docs_mini(root: &Path)` writing `docs/design.md`, `docs/runbook.md`, `site/index.html`, `site/app.css`, `package.json`, `ci.yml`, `Cargo.toml`, `notes.txt`, `package-lock.json`, `big.min.css`.

- [ ] **Step 1: Write the failing tests**

`fixture.rs`:

```rust
/// Documents next to the TS mini repo: two Markdown files that mention its symbols and
/// each other, a page, a stylesheet, three config files, a text note, and two files
/// that must be skipped.
pub fn write_docs_mini(root: &Path) {
    write_ts_mini(root);
    w(root, "docs/design.md", "# Design\n\nSessions are created by `createSession` in [session](../src/auth/session.ts).\n\n## Freshness\n\nEvery call refreshes; see [[runbook]] for the STALE header.\n\n## Storage\n\n`SessionStore` keeps them.\n");
    w(root, "docs/runbook.md", "# Runbook\n\n## When the header says STALE\n\nWait and call again. The login flow (`login`) is unaffected.\n");
    w(root, "site/index.html", "<html><body><h1>Sessions</h1><div id=\"app\"><p>Uses <code>createSession</code>.</p></div></body></html>");
    w(root, "site/app.css", ".login { color: red }\n@media (max-width: 600px) { .login { color: blue } }\n");
    w(root, "package.json", "{\"name\": \"mini\", \"scripts\": {\"build\": \"tsc\"}, \"token\": \"ghp_notreallyasecretbutlong1234567890\"}\n");
    w(root, "ci.yml", "name: ci\njobs:\n  build:\n    steps: []\n");
    w(root, "Cargo.toml", "[package]\nname = \"mini\"\n[dependencies]\nserde = \"1\"\n");
    w(root, "notes.txt", "Remember: `log` is the only logger.\n");
    w(root, "package-lock.json", "{\"lockfileVersion\": 3}\n");
    w(root, "big.min.css", &format!(".a{{x:y}}{}", "a".repeat(2000)));
}
```

And in `write_workspace`, the notes root gains `w(&notes, "design.md", "# Design notes\n\nSee `createSession`.\n");` so `[[design]]` in its README resolves.

`index.rs` tests:

```rust
    #[test]
    fn documents_become_sections_bodies_mentions_and_skips() {
        let dir = tempfile::tempdir().unwrap();
        crate::fixture::write_docs_mini(dir.path());
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        let ws = Workspace::single(dir.path()).unwrap();
        Indexer::new(&store, &ws, &MapConfig::default()).unwrap().refresh(None).unwrap();
        let conn = store.conn();
        let kind = |path: &str, name: &str| -> Option<String> {
            conn.query_row(
                "SELECT s.kind FROM symbols s JOIN files f ON f.id = s.file_id WHERE f.path = ?1 AND s.name = ?2",
                [path, name], |r| r.get(0)).ok()
        };
        assert_eq!(kind("docs/design.md", "Freshness").as_deref(), Some("section"));
        assert_eq!(kind("docs/design.md", "design").as_deref(), Some("document"));
        assert_eq!(kind("site/index.html", "app").as_deref(), Some("element"));
        assert_eq!(kind("site/app.css", ".login").as_deref(), Some("rule"));
        assert_eq!(kind("package.json", "scripts.build").as_deref(), Some("key"));
        assert_eq!(kind("ci.yml", "jobs.build").as_deref(), Some("key"));
        assert_eq!(kind("Cargo.toml", "dependencies.serde").as_deref(), Some("key"));
        assert_eq!(kind("notes.txt", "notes").as_deref(), Some("document"));
        let lang: String = conn.query_row("SELECT lang FROM files WHERE path = 'docs/design.md'", [], |r| r.get(0)).unwrap();
        assert_eq!(lang, "markdown");
        let body_hits: i64 = conn
            .query_row("SELECT COUNT(*) FROM sections_fts WHERE sections_fts MATCH 'stale'", [], |r| r.get(0))
            .unwrap();
        assert!(body_hits >= 2, "both STALE sections: {body_hits}");
        let no_values: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbols_fts WHERE symbols_fts MATCH 'tsc'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(no_values, 0);
        let mentions: Vec<String> = {
            let mut st = conn.prepare("SELECT r.name FROM refs r JOIN files f ON f.id = r.file_id WHERE f.path = 'docs/design.md' ORDER BY r.name").unwrap();
            st.query_map([], |r| r.get(0)).unwrap().collect::<std::result::Result<_, _>>().unwrap()
        };
        assert_eq!(mentions, vec!["SessionStore", "createSession", "runbook", "session"]);
        let skipped = |path: &str| -> Option<String> {
            conn.query_row("SELECT skipped_reason FROM files WHERE path = ?1", [path], |r| r.get(0)).unwrap()
        };
        assert_eq!(skipped("package-lock.json").as_deref(), Some("lockfile"));
        assert_eq!(skipped("big.min.css").as_deref(), Some("minified"));
        assert_eq!(skipped("package.json").as_deref(), Some("secret-like content"));
    }

    #[test]
    fn reindexing_a_document_replaces_its_fts_rows() {
        let dir = tempfile::tempdir().unwrap();
        crate::fixture::write_docs_mini(dir.path());
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        let ws = Workspace::single(dir.path()).unwrap();
        let cfg = MapConfig::default();
        Indexer::new(&store, &ws, &cfg).unwrap().refresh(None).unwrap();
        std::fs::write(dir.path().join("docs/runbook.md"), "# Runbook\n\nNothing stale here any more.\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        Indexer::new(&store, &ws, &cfg).unwrap().refresh(None).unwrap();
        let n: i64 = store.conn()
            .query_row("SELECT COUNT(*) FROM sections_fts WHERE path = 'docs/runbook.md'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "one document row, the old section row is gone");
        std::fs::remove_file(dir.path().join("docs/runbook.md")).unwrap();
        Indexer::new(&store, &ws, &cfg).unwrap().refresh(None).unwrap();
        let n: i64 = store.conn()
            .query_row("SELECT COUNT(*) FROM sections_fts WHERE path = 'docs/runbook.md'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }
```

Note the `package.json` fixture value looks like a GitHub token so `looks_secret` fires; that is deliberate and doubles as the "values never leak" guard. Switch the two Task 3 assertions now: in `changed.rs` assert `notes/new.md` appears as a symbol (`document` named `new`), and in `hook.rs` read `notes.join("README.md")`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p singularrag-core --lib index:: 2>&1 | grep -E 'panicked|test result' | head -5`
Expected: `no such table: sections_fts` or the `kind(...)` assertion for `Freshness` failing.

- [ ] **Step 3: Implement**

`schema.rs`: after `symbols_fts`:

```sql
CREATE VIRTUAL TABLE IF NOT EXISTS sections_fts USING fts5(
  path UNINDEXED, name UNINDEXED, content, tokenize='porter unicode61'
);
```

and `pub const SCHEMA_VERSION: i64 = 3;` (the store drops and rebuilds on mismatch; the CLI message already says to run `singularrag index`).

`index.rs` `index_file`, after the UTF-8 check and before `looks_secret`:

```rust
        if lang.is_document() {
            if let Some(reason) = crate::doc::skip_reason(&e.rel_path, e.size, source) {
                self.upsert_skipped(&e.rel_path, e.mtime_ms, e.size, reason, now)?;
                return Ok(Outcome::Skipped);
            }
        }
```

Replace `let tags = extract_tags(lang, source)?;` and the tag loop with:

```rust
        let stem = e.rel_path.rsplit('/').next().unwrap_or(&e.rel_path);
        let stem = stem.rsplit_once('.').map(|(s, _)| s).unwrap_or(stem).to_string();
        let (tags, bodies, mentions) = if lang.is_document() {
            let d = crate::doc::extract(lang, source, &stem)?;
            (d.tags, d.bodies, d.mentions)
        } else {
            (extract_tags(lang, source)?, Vec::new(), Vec::new())
        };
        …
        let mut ids: Vec<i64> = Vec::with_capacity(tags.len());
        for t in &tags {
            if t.name.is_empty() { ids.push(0); continue; }
            if t.is_definition {
                tx.execute("INSERT INTO symbols(…) VALUES (…)", params![…])?;
                let id = tx.last_insert_rowid();
                ids.push(id);
                tx.execute("INSERT INTO symbols_fts(…)", params![…])?;
            } else {
                ids.push(0);
                tx.execute("INSERT INTO refs(file_id, name, line) VALUES (?1, ?2, ?3)", params![file_id, t.name, t.line_start])?;
            }
        }
        for (i, body) in &bodies {
            let id = ids[*i];
            if id != 0 && matches!(tags[*i].kind.as_str(), "section" | "document" | "element") {
                tx.execute(
                    "INSERT INTO sections_fts(rowid, path, name, content) VALUES (?1, ?2, ?3, ?4)",
                    params![id, e.rel_path, tags[*i].name, body],
                )?;
            }
        }
        for (name, line) in &mentions {
            tx.execute("INSERT INTO refs(file_id, name, line) VALUES (?1, ?2, ?3)", params![file_id, name, line])?;
        }
```

`delete_symbols_for` gains, before the `symbols` delete: `conn.execute("DELETE FROM sections_fts WHERE rowid IN (SELECT id FROM symbols WHERE file_id = ?1)", [file_id])?;`.

- [ ] **Step 4: Run the tests**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace 2>&1 | grep -E 'test result|panicked|FAILED'`
Expected: all `ok`. Existing tests that count files or symbols on `write_ts_mini` now also see `README.md` as a Markdown document with one `document` symbol named `README` (and `Cargo.toml` in `write_rust_mini` with keys); adjust those counts where a test asserts an exact number, and say so in the commit.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(core): index documents as sections with bodies in sections_fts and mentions as refs"
```

---

### Task 6: Ranking, map and find over documents

**Files:**
- Modify: `crates/singularrag-core/src/rank.rs` (`fts_body_ids`, `Reasons.body_hit`, scoring)
- Modify: `crates/singularrag-core/src/map.rs` tests only (a section row renders)
- Modify: `crates/singularrag-core/src/engine.rs` tests (find over a heading; trace note to code)
- Modify: `ui/src/api/types.ts` (`body_hit: boolean`), `ui/src/lib/reasons.ts`, `ui/src/lib/reasons.test.ts`, every `Reasons` literal in `ui/src` tests gains `body_hit: false`

**Interfaces:**
- Consumes: `sections_fts` (Task 5).
- Produces: `Reasons.body_hit: bool` (serialised in `reasons_json`); `rank::fts_body_ids(store, terms) -> Result<Vec<i64>>` (sorted symbol ids).

- [ ] **Step 1: Write the failing tests**

`rank.rs` tests:

```rust
    #[test]
    fn a_body_hit_seeds_the_file_and_marks_the_section() {
        let dir = tempfile::tempdir().unwrap();
        crate::fixture::write_docs_mini(dir.path());
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        let ws = crate::workspace::Workspace::single(dir.path()).unwrap();
        Indexer::new(&store, &ws, &MapConfig::default()).unwrap().refresh(None).unwrap();
        let ranked = rank_symbols(&store, &MapConfig::default(), Some("what does the STALE header mean"), &[]).unwrap();
        let top: Vec<String> = ranked.iter().take(3).map(|s| format!("{}::{}", s.path, s.name)).collect();
        assert!(top.contains(&"docs/runbook.md::When the header says STALE".to_string()), "{top:?}");
        let hit = ranked.iter().find(|s| s.name == "When the header says STALE").unwrap();
        assert!(hit.reasons.body_hit);
        assert!(hit.reasons.seeds.iter().any(|s| s.starts_with("query:")));
        let sess = ranked.iter().find(|s| s.name == "createSession").unwrap();
        assert!(!sess.reasons.body_hit);
    }

    #[test]
    fn a_mention_gives_the_document_an_edge_to_the_code() {
        let dir = tempfile::tempdir().unwrap();
        crate::fixture::write_docs_mini(dir.path());
        let store = Store::open(&dir.path().join(".singularrag/index.db")).unwrap();
        let ws = crate::workspace::Workspace::single(dir.path()).unwrap();
        Indexer::new(&store, &ws, &MapConfig::default()).unwrap().refresh(None).unwrap();
        let ranked = rank_symbols(&store, &MapConfig::default(), Some("createSession"), &[]).unwrap();
        let sess = ranked.iter().find(|s| s.path == "src/auth/session.ts" && s.name == "createSession").unwrap();
        assert!(sess.reasons.referenced_by.iter().any(|r| r.path == "docs/design.md"), "{:?}", sess.reasons.referenced_by);
    }
```

`map.rs` tests:

```rust
    #[test]
    fn a_section_renders_under_its_document_with_its_heading_line() {
        let s = ScoredSymbol {
            symbol_id: 1, file_id: 1, path: "docs/design.md".into(), name: "Freshness".into(), kind: "section".into(),
            line_start: 5, line_end: 8, signature: "## Freshness".into(), score: 1.0, reasons: Reasons::default(),
        };
        assert_eq!(render(&[s], 1, &[]), "docs/design.md:\n    5  ## Freshness\n");
    }
```

`engine.rs` tests:

```rust
    #[test]
    fn find_symbol_finds_a_heading_and_trace_path_walks_a_mention() {
        let dir = tempfile::tempdir().unwrap();
        crate::fixture::write_docs_mini(dir.path());
        let mut e = Engine::open(dir.path(), "t").unwrap();
        let f = e.find_symbol(&FindRequest { name: "freshness".into(), kind: None, limit: 5 }).unwrap();
        assert!(f.text.contains("docs/design.md") && f.text.contains("## Freshness"), "{}", f.text);
        let t = e.trace_path(&TraceRequest {
            from_path: "docs/design.md".into(), from_symbol: "design".into(),
            to_path: "src/auth/session.ts".into(), to_symbol: "createSession".into(),
        }).unwrap();
        assert!(t.text.contains("# 1 hop"), "{}", t.text);
    }
```

`ui/src/lib/reasons.test.ts`: a case where `body_hit: true` yields `"Matches the text of the section."`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p singularrag-core --lib rank:: 2>&1 | grep -E '^error|panicked' | head -3`
Expected: `no field body_hit`.

- [ ] **Step 3: Implement**

`rank.rs`: `Reasons` gains `pub body_hit: bool`. Add:

```rust
/// Symbol ids whose section text matches a query term (prefix, porter-stemmed).
pub fn fts_body_ids(store: &Store, terms: &[String]) -> Result<Vec<i64>> {
    let mut ids = Vec::new();
    let mut stmt = store.conn().prepare("SELECT rowid FROM sections_fts WHERE sections_fts MATCH ?1")?;
    for t in terms {
        let q = format!("content: \"{}\"*", t.replace('"', "\"\""));
        for row in stmt.query_map([q], |r| r.get::<_, i64>(0))? {
            ids.push(row?);
        }
    }
    ids.sort_unstable();
    ids.dedup();
    Ok(ids)
}
```

In `rank_symbols`, after `fts_ids`: `let body_ids = fts_body_ids(store, &terms)?; let is_body_hit = |id: i64| body_ids.binary_search(&id).is_ok();`. `fts_files` includes files with a body hit; `fts_in_file` counts `is_fts_hit(s.id) || is_body_hit(s.id)`; in the scoring closure `let body_hit = is_body_hit(s.id); if fts_hit || body_hit { score += fr / hits_in_file }` and `reasons.body_hit = body_hit`. `fts_names` stays symbol-name hits only (a body hit must not turn the query multiplier on for an edge name).

`ui/src/api/types.ts`: `body_hit: boolean` in `Reasons`; `reasons.ts`: after the note line, `if (r.body_hit) out.push("Matches the text of the section.");`; add `body_hit: false` to every `Reasons` literal in `ui/src` tests (`grep -rn "note_hit:" ui/src` lists them).

- [ ] **Step 4: Run the tests**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace 2>&1 | grep -E 'test result|panicked|FAILED'`; `cd /Users/jonasbroms/Sites/singularrag/.claude/worktrees/ui-v0/ui && bun run typecheck && bun test 2>&1 | tail -3`
Expected: all `ok`.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(core): section bodies seed ranking; body_hit reason; find and trace over documents"
```

---

### Task 7: Agent-facing text and document integration tests

**Files:**
- Modify: `crates/singularrag/src/mcp/server.rs` (`INSTRUCTIONS`, `REPO_MAP_DESCRIPTION`, the `#[tool]` literal)
- Modify: `crates/singularrag/tests/mcp.rs` (the copied literals and a document round trip)
- Modify: `crates/singularrag/tests/cli.rs` (query and changed over documents)

**Interfaces:** none new. The three copies of each string stay byte-identical (existing test guards it).

- [ ] **Step 1: Write the failing tests**

`tests/mcp.rs`: the assertion on `INSTRUCTIONS` starts with `"singularrag gives you a ranked map"`; extend the copied `REPO_MAP_DESCRIPTION` constant in the test file to the new text (below), and add:

```rust
#[tokio::test]
async fn repo_map_serves_a_document_section() {
    let dir = tempfile::tempdir().unwrap();
    singularrag_core::fixture::write_docs_mini(dir.path());
    // … the crate's existing helper that starts the server over `dir` and calls repo_map;
    // copy the body of the existing `repo_map` round-trip test and change the query:
    let text = call_repo_map(&client, "what does the STALE header mean", 2048).await;
    assert!(text.contains("docs/runbook.md:"), "{text}");
    assert!(text.contains("## When the header says STALE"), "{text}");
}
```

(`call_repo_map` is whatever name the existing round-trip test in `tests/mcp.rs` uses for its tool call; reuse it, do not add a second server helper.)

`tests/cli.rs`:

```rust
#[test]
fn query_and_changed_cover_documents() {
    let dir = tempfile::tempdir().unwrap();
    singularrag_core::fixture::write_docs_mini(dir.path());
    Command::cargo_bin("singularrag").unwrap()
        .args(["--repo", dir.path().to_str().unwrap(), "query", "STALE header", "--budget", "2048"])
        .assert().success()
        .stdout(predicate::str::contains("docs/runbook.md:").and(predicate::str::contains("## When the header says STALE")));
    let git = |args: &[&str]| {
        assert!(std::process::Command::new("git").arg("-C").arg(dir.path()).args(args).status().unwrap().success());
    };
    git(&["init", "-q"]);
    git(&["-c", "user.email=t@t", "-c", "user.name=t", "add", "."]);
    git(&["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "base"]);
    std::fs::write(dir.path().join("docs/runbook.md"), "# Runbook\n\n## When the header says STALE\n\nWait longer.\n").unwrap();
    Command::cargo_bin("singularrag").unwrap()
        .args(["--repo", dir.path().to_str().unwrap(), "changed"])
        .assert().success()
        .stdout(predicate::str::contains("docs/runbook.md::When the header says STALE"));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p singularrag --test mcp 2>&1 | grep -E 'panicked|test result' | head -3`
Expected: the description equality test fails once the test-file copy changes (RED for the text); the document round trip fails only if the map lacks the section.

- [ ] **Step 3: Implement**

New texts, byte-identical in the three places:

`INSTRUCTIONS`: `"singularrag gives you a ranked map of this workspace: code symbols and document sections (specs, notes, READMEs, config keys) together. Call repo_map first with your task as the query and answer from it; read only to confirm a detail the map does not show, and read the section the map points at rather than the whole file. Use find_symbol to locate a name or a heading, trace_path to see how two symbols connect (a note that mentions a symbol counts), and changed to see what a diff touches and who references it. When you learn something about a file that its signatures do not say, record it with annotate so the next session starts from it. Only annotate writes, and only a note into .singularrag/map.toml. A STALE header means files changed since indexing; the index catches up in the background."`

`REPO_MAP_DESCRIPTION`: `"Token-budgeted map of the code symbols and document sections most relevant to a task, each file with the files that reference it. Call this first and answer locate, trace, blast-radius and placement questions from it; read a file only to confirm a detail the map does not show. `query` is a question or identifiers; `focus_files` are workspace-relative paths you already know matter; `budget_tokens` defaults to 1024, up to 8192 for trace and blast-radius questions. Each file header ends with `← ` and the files that reference it; rows are `line  signature` for code and `line  ## heading` for document sections, never bodies. The first line says how fresh the index is; if it says STALE, call again after a moment."`

Update the `#[tool(description = "…")]` literal on `repo_map`, the two constants, and the copies in `tests/mcp.rs`.

- [ ] **Step 4: Run the tests**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test -p singularrag 2>&1 | grep -E 'test result|panicked|FAILED'`
Expected: all `ok`.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(mcp): the agent-facing text names document sections; document round trips over the binary"
```

---

### Task 8: `POST /api/query`

**Files:**
- Modify: `crates/singularrag/src/serve/state.rs` (`AppState.handle: EngineHandle`; `AppState::new(root, port, handle)`)
- Modify: `crates/singularrag/src/serve/routes.rs` (`query`)
- Modify: `crates/singularrag/src/serve/mod.rs` (route; pass the handle)
- Modify: `crates/singularrag/src/serve/{auth,events,watcher}.rs` tests (spawn an actor for `AppState::new`)
- Modify: `crates/singularrag/tests/serve.rs` if it exists, else add the route test to `routes.rs`

**Interfaces:**
- Consumes: `actor::EngineHandle::map(MapRequest) -> Reply<MapResponse>` (`Reply<T> = Result<T, String>`).
- Produces: `POST /api/query` body `{ "query": string, "budget"?: number }` → `200 { "retrieval_id": i64, "served": usize, "cut": usize }`; `422 { "error": "query must be 1 to 2000 characters", "field": "query" }`; `503 { "error": <engine error> }` when the actor replies `Err`.

Ruling (executor, per spec §6): the retrieval is recorded under the actor's existing session key `serve`, whose label is already `UI`, instead of a new key `ui` labelled `This UI`; one session key per process is the actor's contract and the rail already shows `UI`. Cost if wrong: a label.

- [ ] **Step 1: Write the failing test**

In `routes.rs` tests (or the serve test file the crate already has for routes; follow the `auth.rs` test's way of building a router):

```rust
    #[tokio::test]
    async fn query_records_a_ui_retrieval_and_validates_input() {
        let dir = tempfile::tempdir().unwrap();
        singularrag_core::fixture::write_docs_mini(dir.path());
        let (handle, _join, _died) = crate::actor::spawn(crate::actor::EngineConfig {
            root: dir.path().to_path_buf(),
            session_key: crate::actor::SessionKey::Fixed("serve".into()),
            refresh_budget: std::time::Duration::from_secs(5),
        });
        handle.refresh().await.unwrap();
        let state = crate::serve::state::AppState::new(dir.path().to_path_buf(), 1, handle.clone()).unwrap();
        let token = state.token.to_string();
        let app = crate::serve::router(state);
        let post = |body: &str| {
            axum::http::Request::builder()
                .method("POST").uri("/api/query").header("host", "127.0.0.1:1")
                .header("authorization", format!("Bearer {token}")).header("content-type", "application/json")
                .body(axum::body::Body::from(body.to_string())).unwrap()
        };
        let res = tower::ServiceExt::oneshot(app.clone(), post(r#"{"query":"STALE header","budget":2048}"#)).await.unwrap();
        assert_eq!(res.status(), 200);
        let v: serde_json::Value = serde_json::from_slice(&axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap()).unwrap();
        assert!(v["retrieval_id"].as_i64().unwrap() >= 1 && v["served"].as_u64().unwrap() >= 1, "{v}");
        let res = tower::ServiceExt::oneshot(app.clone(), post(r#"{"query":""}"#)).await.unwrap();
        assert_eq!(res.status(), 422);
        let list = tower::ServiceExt::oneshot(app, axum::http::Request::builder().uri("/api/retrievals").header("host", "127.0.0.1:1").header("authorization", format!("Bearer {token}")).body(axum::body::Body::empty()).unwrap()).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&axum::body::to_bytes(list.into_body(), 1 << 20).await.unwrap()).unwrap();
        assert_eq!(v[0]["session_label"], "UI");
        assert_eq!(v[0]["query"], "STALE header");
        handle.shutdown();
    }
```

(The header names and the way `auth.rs` tests pass the token are the pattern to copy; if that file uses a query-string token for a route, use the bearer form here as the `require_token` layer accepts.)

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p singularrag --bin singularrag query_records 2>&1 | grep -E '^error|panicked' | head -3`
Expected: `AppState::new` takes 2 arguments, or 404 on the route.

- [ ] **Step 3: Implement**

`state.rs`: `pub handle: crate::actor::EngineHandle`; `new(root, port, handle)`. `serve/mod.rs` passes `handle.clone()` into `AppState::new` and adds `.route("/query", post(routes::query))` (import `axum::routing::post`). `routes.rs`:

```rust
#[derive(Deserialize)]
pub struct QueryBody {
    pub query: String,
    pub budget: Option<usize>,
}

#[derive(Serialize)]
pub struct QueryDto {
    pub retrieval_id: i64,
    pub served: usize,
    pub cut: usize,
}

pub async fn query(State(s): State<AppState>, Json(b): Json<QueryBody>) -> Result<Json<QueryDto>, ApiError> {
    let q = b.query.trim();
    if q.is_empty() || q.chars().count() > 2000 {
        return Err(ApiError(StatusCode::UNPROCESSABLE_ENTITY, serde_json::json!({ "error": "query must be 1 to 2000 characters", "field": "query" })));
    }
    let req = singularrag_core::engine::MapRequest {
        query: Some(q.to_string()),
        focus_files: vec![],
        budget_tokens: singularrag_core::map::clamp_budget(b.budget.unwrap_or(singularrag_core::map::DEFAULT_BUDGET)),
    };
    match s.handle.map(req).await {
        Ok(r) => Ok(Json(QueryDto { retrieval_id: r.retrieval_id, served: r.served, cut: r.total.saturating_sub(r.served) })),
        Err(e) => Err(ApiError(StatusCode::SERVICE_UNAVAILABLE, serde_json::json!({ "error": e }))),
    }
}
```

Tests in `auth.rs`, `events.rs`, `watcher.rs` that call `AppState::new(dir, port)` spawn an actor the same way the test above does and pass its handle (a small `fn test_state(dir: &Path) -> (AppState, EngineHandle)` helper in `state.rs` under `#[cfg(test)]` keeps it to one place).

- [ ] **Step 4: Run the tests**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test -p singularrag 2>&1 | grep -E 'test result|panicked|FAILED'`
Expected: all `ok`.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(serve): POST /api/query runs repo_map through the actor and records a UI retrieval"
```

---

### Task 9: UI: query panel, root filter, document colours

**Files:**
- Create: `ui/src/components/QueryPanel.tsx`, `ui/src/components/QueryPanel.test.tsx`
- Create: `ui/src/lib/langGroup.ts`, `ui/src/lib/langGroup.test.ts`
- Modify: `ui/src/api/client.ts` (`query`), `ui/src/api/types.ts` (`QueryResult`, `GraphNode.lang` already exists)
- Modify: `ui/src/lib/mapStyle.ts` (`NodeCtx.group`, colour per group), `ui/src/lib/mapStyle.test.ts`
- Modify: `ui/src/components/MapView.tsx` (pass `group: langGroup(node.lang)`)
- Modify: `ui/src/App.tsx` (panel above the rail, root filter in the header, announcements), `ui/src/App.test.tsx`

**Interfaces:**
- Consumes: `POST /api/query` (Task 8), `Status.roots` (Task 3), `Reasons.body_hit` (Task 6).
- Produces: `api.query(query: string, budget: number) => Promise<QueryResult>` with `QueryResult = { retrieval_id: number; served: number; cut: number }`; `langGroup(lang: string | null): "code" | "docs" | "config" | "styles"` (`markdown`, `html`, `text` → docs; `json`, `yaml`, `toml` → config; `css` → styles; else code); `QueryPanel({ onDone, announce })`.

- [ ] **Step 1: Write the failing tests**

`ui/src/lib/langGroup.test.ts`:

```ts
import { describe, expect, test } from "bun:test";
import { langGroup } from "./langGroup";

describe("langGroup", () => {
  test("groups languages", () => {
    expect(langGroup("typescript")).toBe("code");
    expect(langGroup("markdown")).toBe("docs");
    expect(langGroup("text")).toBe("docs");
    expect(langGroup("json")).toBe("config");
    expect(langGroup("css")).toBe("styles");
    expect(langGroup(null)).toBe("code");
  });
});
```

`ui/src/lib/mapStyle.test.ts`: `test("untouched document nodes use the docs colour", () => { const s = nodeStyle("docs/a.md", { ...base, status: "untouched", group: "docs" }, pal); expect(s.color).toBe(pal.docs); });` and `base` gains `group: "code"`; `Palette` gains `docs`, `config`, `styles`.

`ui/src/components/QueryPanel.test.tsx`:

```tsx
import { afterEach, describe, expect, mock, test } from "bun:test";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { QueryPanel } from "./QueryPanel";

const calls: unknown[] = [];
mock.module("@/api/client", () => ({
  api: {
    query: async (query: string, budget: number) => {
      calls.push({ query, budget });
      if (query === "boom") throw new Error("query must be 1 to 2000 characters");
      return { retrieval_id: 9, served: 3, cut: 2 };
    },
  },
}));

afterEach(() => { calls.length = 0; });

describe("QueryPanel", () => {
  test("submits the query and budget, reports the result", async () => {
    const done: number[] = [];
    const said: string[] = [];
    render(<QueryPanel onDone={(id) => done.push(id)} announce={(m) => said.push(m)} />);
    const user = userEvent.setup();
    await user.type(screen.getByLabelText("Query"), "where is routing");
    await user.selectOptions(screen.getByLabelText("Budget"), "2048");
    await user.click(screen.getByRole("button", { name: "Run query" }));
    await waitFor(() => expect(done).toEqual([9]));
    expect(calls).toEqual([{ query: "where is routing", budget: 2048 }]);
    expect(said).toContain("3 served, 2 cut");
  });

  test("query_panel_announces_errors", async () => {
    const said: string[] = [];
    render(<QueryPanel onDone={() => {}} announce={(m) => said.push(m)} />);
    const user = userEvent.setup();
    await user.type(screen.getByLabelText("Query"), "boom");
    await user.click(screen.getByRole("button", { name: "Run query" }));
    await waitFor(() => expect(screen.getByRole("alert").textContent).toContain("1 to 2000"));
    expect(said.some((m) => m.includes("1 to 2000"))).toBe(true);
    const results = await axe.run(document.body);
    expect(results.violations).toEqual([]);
  });
});
```

`ui/src/App.test.tsx`: add a fetch mock branch for `POST /api/query` returning `{ retrieval_id: 8, served: 1, cut: 0 }` and make the `/api/retrievals` mock return `retrievalList` (it already does); a test `"running a query from the panel selects the new retrieval"` that types in the panel, submits, pushes a retrieval with id 8 into `retrievalList` before the refetch, and asserts `within(rail).getByRole("button", { name: /^Map · where/ })` has `aria-current="true"`. A test `"a multi-root status shows a root filter that narrows the tree"` with `status.roots = [{ name: "app", … }, { name: "notes", … }]` and a tree containing `app/src/a.ts` and `notes/n.md`: selecting `notes` in the `Root` select leaves only `notes/n.md` rows.

- [ ] **Step 2: Run them**

Run: `cd /Users/jonasbroms/Sites/singularrag/.claude/worktrees/ui-v0/ui && bun test langGroup QueryPanel 2>&1 | tail -3`
Expected: module-not-found failures.

- [ ] **Step 3: Implement**

`langGroup.ts`:

```ts
export type LangGroup = "code" | "docs" | "config" | "styles";
const GROUPS: Record<string, LangGroup> = { markdown: "docs", html: "docs", text: "docs", json: "config", yaml: "config", toml: "config", css: "styles" };
export const langGroup = (lang: string | null): LangGroup => (lang ? GROUPS[lang] : undefined) ?? "code";
```

`mapStyle.ts`: `Palette` gains `docs: string; config: string; styles: string` (defaults `#7c3aed`, `#0e7490`, `#be185d`; `readPalette` reads `--map-docs` etc. like the others); `NodeCtx` gains `group: LangGroup`; in `nodeStyle` the untouched branch uses `pal[ctx.group === "code" ? "untouched" : ctx.group]` as its colour and the served/cut branches keep their status colours. `MapView.tsx` passes `group: langGroup(node lang from the payload)`; the payload's `GraphNode.lang` already exists.

`client.ts`: `query: (query: string, budget: number) => req<QueryResult>("POST", "/query", { query, budget })`. `types.ts`: `export type QueryResult = { retrieval_id: number; served: number; cut: number };`.

`QueryPanel.tsx`:

```tsx
import { useState } from "react";
import { api } from "@/api/client";

export function QueryPanel({ onDone, announce }: { onDone: (id: number) => void; announce: (m: string) => void }) {
  const [query, setQuery] = useState("");
  const [budget, setBudget] = useState("1024");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      const r = await api.query(query, Number(budget));
      announce(`${r.served} served, ${r.cut} cut`);
      onDone(r.retrieval_id);
    } catch (err) {
      const m = err instanceof Error ? err.message : String(err);
      setError(m);
      announce(`Query failed: ${m}`);
    } finally {
      setBusy(false);
    }
  };
  return (
    <form onSubmit={submit} aria-label="Run a query" className="flex flex-col gap-2 border-b px-3 py-2">
      <label className="text-sm">Query
        <input type="text" value={query} onChange={(e) => setQuery(e.target.value)} className="mt-1 w-full rounded border bg-background px-2 py-1" />
      </label>
      <div className="flex items-end gap-2">
        <label className="text-sm">Budget
          <select value={budget} onChange={(e) => setBudget(e.target.value)} className="ml-2 rounded border bg-background px-2 py-1">
            <option value="1024">1024</option><option value="2048">2048</option><option value="4096">4096</option>
          </select>
        </label>
        <button type="submit" disabled={busy} className="rounded border px-3 py-1 text-sm">Run query</button>
      </div>
      {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
    </form>
  );
}
```

`App.tsx`: render `<QueryPanel onDone={onQueryDone} announce={announce} />` inside the rail's grid cell above `RetrievalsRail` (wrap both in a `<div className="flex min-h-0 flex-col">`); `onQueryDone` refetches `api.retrievals()`, sets the list, and `setSelected(id)`. Root filter: `const [root, setRoot] = useState("")`; when `status?.roots.length > 1`, a `<label>Root <select>` in the header with an `All roots` option and one per root name; `rows` are filtered by `path.startsWith(root + "/")` when `root` is set (apply to the tree rows and the graph nodes alike through the same `rows`). `RetrievalsRail` is unchanged. The `Status` fixture in tests already carries `roots` from Task 3.

- [ ] **Step 4: Run**

Run: `cd /Users/jonasbroms/Sites/singularrag/.claude/worktrees/ui-v0/ui && bun run typecheck && bun test 2>&1 | tail -3 && bun run build 2>&1 | tail -1`, then `cargo test --workspace 2>&1 | grep -E 'FAILED|panicked'` (embedded assets rebuilt).
Expected: typecheck clean, all UI tests pass including the two axe runs, build ok, Rust green.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(ui): query panel records a UI retrieval; root filter; document colours on the map; body_hit sentence"
```

---

### Task 10: The docs eval set, the gate, and the docs

**Files:**
- Create: `e*/questions-docs.toml` (write it with the Write tool; the shell guard rejects the directory name)
- Modify: `README.md` (`## What the agent sees`, `## CLI`, a new `## Workspaces and documents` section), `docs/superpowers/specs/2026-09-19-singularrag-design.md` (§5, §6, §7, §9, §10, §12 amendments), `e*/README.md` (the gate)
- Modify: `crates/singularrag/tests/cli.rs` (both question files load and run)

**Interfaces:** none new. The `eval` subcommand already takes `--questions <path>`.

- [ ] **Step 1: Write the failing test**

`tests/cli.rs`:

```rust
#[test]
fn the_docs_question_file_loads_and_names_sections_that_exist() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../e", "val/questions-docs.toml");
    let qs = singularrag_core::eval::load_questions(std::path::Path::new(path)).unwrap();
    assert_eq!(qs.len(), 12);
    let cats: std::collections::BTreeSet<&str> = qs.iter().map(|q| q.category.as_str()).collect();
    assert_eq!(cats.into_iter().collect::<Vec<_>>(), vec!["code-to-doc", "doc-to-code", "locate-doc"]);
    // Every gold section must exist in this repository's own index.
    let repo = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
    let mut e = singularrag_core::engine::Engine::open(repo, "docs-gold-check").unwrap();
    e.refresh(std::time::Duration::from_secs(120)).unwrap();
    for q in &qs {
        for g in &q.gold {
            let (path, name) = g.split_once("::").unwrap();
            let n: i64 = e.store().conn().query_row(
                "SELECT COUNT(*) FROM symbols s JOIN files f ON f.id = s.file_id WHERE f.path = ?1 AND s.name = ?2",
                [path, name], |r| r.get(0)).unwrap();
            assert!(n > 0, "{} gold {g} is not in the index", q.id);
        }
    }
}
```

(This test indexes the repository itself into its own `.singularrag/index.db`, which is gitignored and already exists for the CLI; it runs in seconds.)

- [ ] **Step 2: Run it**

Run: `cargo test -p singularrag --test cli docs_question 2>&1 | grep -E 'panicked|test result' | head -3`
Expected: FAIL, file not found.

- [ ] **Step 3: Write the question file, check every gold with `singularrag find`, then the docs**

`questions-docs.toml` (twelve questions; the gold below was authored from the headings and symbols in this repository on 2026-09-23; run `cargo run -q -p singularrag -- find "<name>"` for every entry and correct any heading whose exact text differs, since the section name is the heading text without `#`):

```toml
# Fixed on first run. Never edit an existing question; add new ids instead.
# Authored against this repository (singularrag) at branch documents-v0. Every gold entry is
# `path::name` where `name` is a heading's text (a `section`) or a code symbol, as printed
# by `singularrag find <name>` on this repo.

[[question]]
id = "D1"
category = "locate-doc"
query = "how fresh is the index and what does the STALE header promise"
gold = ["docs/superpowers/specs/2026-09-19-singularrag-design.md::8. Freshness"]

[[question]]
id = "D2"
category = "locate-doc"
query = "what does the query-first hook deny and when does it allow"
gold = ["docs/superpowers/specs/2026-09-21-singularrag-workflow-design.md::2. The query-first hook"]

[[question]]
id = "D3"
category = "locate-doc"
query = "which conditions does the tier-two run compare and how is a session launched"
gold = ["docs/superpowers/specs/2026-09-20-singularrag-tier2-design.md::3. Run config", "docs/superpowers/specs/2026-09-20-singularrag-tier2-design.md::4. Session"]

[[question]]
id = "D4"
category = "locate-doc"
query = "when do embeddings or LSP get built, what is the gate"
gold = ["docs/superpowers/specs/2026-09-19-singularrag-design.md::Gates for deferred features (eval-driven, §12)"]

[[question]]
id = "D5"
category = "doc-to-code"
query = "the code that implements the freshness refresh described in the design spec"
gold = ["docs/superpowers/specs/2026-09-19-singularrag-design.md::8. Freshness", "crates/singularrag-core/src/index.rs::refresh_with", "crates/singularrag-core/src/engine.rs::refresh"]

[[question]]
id = "D6"
category = "doc-to-code"
query = "where is the hook decision implemented that the workflow spec describes"
gold = ["docs/superpowers/specs/2026-09-21-singularrag-workflow-design.md::2. The query-first hook", "crates/singularrag/src/hook.rs::run"]

[[question]]
id = "D7"
category = "doc-to-code"
query = "the harness code for parsing the stream and scoring a session"
gold = ["docs/superpowers/specs/2026-09-20-singularrag-tier2-design.md::5. Parsing and scoring", "crates/singularrag-bench/src/stream.rs::parse_stream", "crates/singularrag-bench/src/score.rs::score"]

[[question]]
id = "D8"
category = "doc-to-code"
query = "how the map fits a token budget as the design describes"
gold = ["crates/singularrag-core/src/map.rs::fit", "crates/singularrag-core/src/map.rs::render"]

[[question]]
id = "D9"
category = "code-to-doc"
query = "which spec section describes what singularrag init installs"
gold = ["docs/superpowers/specs/2026-09-21-singularrag-workflow-design.md::3. `singularrag init`", "crates/singularrag/src/init.rs::run"]

[[question]]
id = "D10"
category = "code-to-doc"
query = "where is the verdict rule for whether singularrag earns its place written down"
gold = ["docs/superpowers/specs/2026-09-20-singularrag-tier2-design.md::7. Summary and verdict", "crates/singularrag-bench/src/summary.rs::render"]

[[question]]
id = "D11"
category = "code-to-doc"
query = "the README section that tells a user how to connect Claude Code"
gold = ["README.md::Claude Code", "README.md::Connect an agent"]

[[question]]
id = "D12"
category = "code-to-doc"
query = "what the agent sees in a repo_map response"
gold = ["README.md::What the agent sees", "crates/singularrag-core/src/engine.rs::repo_map"]
```

Run both evals and record the numbers in `e*/README.md` under a new `## Tier one on documents` heading: `cargo run -q -p singularrag -- --repo /Users/jonasbroms/Sites/hono e*/... ` is not needed for hono; for hono run exactly what the existing README says (the hono checkout at `/Users/jonasbroms/Sites/hono`, `--questions <abs path to questions.toml> --budget 4096`) before and after this branch (before: check out 0b769e3 in a scratch worktree or read the number recorded in `e*/README.md`), and for docs run `cargo run -q -p singularrag -- --repo /Users/jonasbroms/Sites/singularrag/.claude/worktrees/ui-v0 eval --questions <abs path>/questions-docs.toml --budget 4096`. The gate: hono within 0.02 of its recorded value, docs at or above 0.6. If docs is below 0.6, the plan's fix is in ranking, not in the questions: raise `FTS_FILE_BOOST` for body hits is not allowed (spec §5 says the same boost), so inspect the misses with `singularrag query "<query>" --budget 4096` and adjust heading extraction or mention rules only where a gold section is provably present and unranked; record what was found in `e*/README.md`.

Docs: README `## What the agent sees` gains a `docs/design.md:` block with a `## Freshness` row in the example and a sentence on documents; `## CLI` mentions `--repo` may be a workspace; new `## Workspaces and documents` section with the `workspace.toml` example from spec §2 and the list of document formats and skips; parent spec amendments per spec §9 (one paragraph each, marked *Amended 2026-09-23 (documents design)*).

- [ ] **Step 4: Run the tests and the evals**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace 2>&1 | grep -E 'test result|panicked|FAILED'`; `cd ui && bun run typecheck && bun test && bun run build`; the two eval commands above.
Expected: all green; both eval outputs pasted into `e*/README.md`.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "docs+eval: the documents question set and gate; README and parent spec amended for workspaces and documents"
```

---

## Self-review

- Spec §2 → Tasks 1, 2, 3; §3 → Tasks 4, 5; §4 → Tasks 4, 5, 6; §5 → Tasks 6, 7; §6 → Tasks 8, 9; §7 → Task 10; §8 → Tasks 1, 5, 8; §9 → Task 10; §10 → each task's tests; §11 holds (no office, no models, no new tool, no per-kind share).
- Names: `Workspace::{open, single, is_named, names, prefix, rel_of, abs_of, root_of}` are used identically in Tasks 2, 3, 5, 6; `doc::{extract, DocExtract, skip_reason, is_lockfile, DOC_MAX_BYTES}` in Tasks 4 and 5; `Reasons.body_hit` in Tasks 6 and 9; `AppState::new(root, port, handle)` in Task 8 and its test callers; `api.query` and `QueryResult` in Task 9.
- Deviation from the spec recorded: §3 says "a tags query per format"; Task 4 walks the trees directly because heading-level folding and key depth cannot be expressed in a tags query. Same output shape, so no spec change is needed beyond a sentence in Task 10's amendment.
- Review Focus lines 1 to 5 each have a named test in Tasks 4, 4, 1, 3 and 9.
