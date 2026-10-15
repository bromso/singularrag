# singularrag schema sources Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prisma schemas and SQL DDL (migrations, dumps) become `table` and `column` symbols in the index, linked to the code that names them and to the processes that name them, shown in the detail panel, and judged by walks on a fixture and on pinned umami.

**Architecture:** Two new `Language`s (`Prisma`, `Sql`) form a third category next to code and documents. A new core module `schema.rs` parses both by hand into the same `Tag`s the indexer already writes (tables, `table.column` columns, foreign keys as non-definition tags, i.e. refs) and contributes an alias vocabulary (`table_names`). The indexer runs schema files first, then code files, where a tree-sitter walk turns property names and string contents that match the vocabulary into refs; a vocabulary change forces one code re-index. Serving adds one field (`refs` on column symbols); the UI adds a Columns section and a `schema` language group; the walk harness gains `implemented_by` on process walks.

**Tech Stack:** Rust (rusqlite, tree-sitter 0.27, regex, blake3), React + TypeScript (Bun only, bun test, axe-core), TOML walk files, git patches.

**Spec:** `docs/superpowers/specs/2026-10-15-singularrag-schema-design.md` (the authority; the walks spec `docs/superpowers/specs/2026-10-12-singularrag-walks-design.md` §2–§5 for the harness).

## Global Constraints

- Worktree: `/Users/jonasbroms/Sites/singularrag/.claude/worktrees/ui-v0`, branch `schema-v0` from `main` at 7cdf16b (the spec is committed at 5ddbb47). Run every command from the worktree; the Bash guard rejects any command containing the substring `eval` (write `e*/README.md`, `e*/walks-ci.toml`; the cli test already does this with `concat!`).
- Gate before every commit: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`; for UI tasks also `cd ui && bun run typecheck && bun test && bun run build` (assets are embedded in the Rust binary; run `cargo test --workspace` again after a UI build).
- Commit with `git add -A <paths>` and end every commit message with `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`.
- Symbol kinds are exactly `table` and `column`; a column's symbol name is `<table>.<column>`; table names are database names (`@@map` else the model name; `@map` else the field name); schema identifiers are unquoted and schema-prefixed names drop the prefix (spec §2).
- A schema file emits refs for foreign keys only, named after the referenced table, at the column's line; it never references code (spec §3).
- Code files get a table ref only from a property identifier after a dot or from string/template contents, never from a plain identifier (spec §3).
- Aliases: the table name; for Prisma also the model name and its lower-camel form; all resolve to the table name (spec §3). Store table `table_names(alias, table_name, file_id)` with `PRIMARY KEY (alias, file_id)`; `SCHEMA_VERSION` becomes 7.
- Caps unchanged: `MAX_FILE_BYTES` (1 MiB) applies to schema files; `SIGNATURE_MAX` (160) to every signature. Tables are never queued for extraction. No new route, no MCP change, no change to `INSTRUCTIONS` or tool descriptions.
- Tier one on hono, docs, prose and handbook sets must be byte-identical before and after this branch (those corpora have no schema files).
- Never run change walks against a working repository; the umami corpus is a pinned clone with `.singularrag/` in `.git/info/exclude`.

## Review Focus

1. A `pg_dump` with a `COPY ... FROM stdin` block whose data lines contain `CREATE TABLE` text: the scanner must drop the data block and emit nothing for it (Task 2 test `a_copy_data_block_and_inserts_are_dropped`).
2. A table named `user` or `session` with code that uses `session` as a variable name: no ref from the plain identifier, a ref from `prisma.client.session` (Task 4 test `properties_and_strings_reference_tables_but_plain_identifiers_do_not`).
3. A schema file added to a corpus that was already indexed, with code files whose mtimes do not change: the code files must be re-indexed once in that run and not again (Task 3 test `a_vocabulary_change_reindexes_code_files_once`).
4. A Prisma model whose closing brace is missing: the models before it parse, the open one runs to EOF, and nothing panics (Task 1 test `an_unterminated_model_runs_to_the_end_of_the_file`).
5. A column whose foreign key points at a table defined in another file: the detail panel's button must still land on that table (Task 6, `App.focusSection` fallback test `a_foreign_key_button_reaches_a_table_defined_in_another_file`).

---

### Task 1: The languages and the Prisma parser

**Files:**
- Modify: `crates/singularrag-core/src/lang.rs` (enum, `from_path`, `as_str`, new `is_schema`, `extract_tags`)
- Modify: `crates/singularrag-core/src/lib.rs` (add `pub mod schema;`)
- Create: `crates/singularrag-core/src/schema.rs`
- Test: unit tests in both files

**Interfaces:**
- Produces: `Language::Prisma`, `Language::Sql`, `Language::is_schema(self) -> bool`; `schema::SchemaExtract { tags: Vec<Tag>, aliases: Vec<(String, String)> }`; `schema::extract(lang: Language, source: &str) -> Result<SchemaExtract>`; `schema::lower_camel(&str) -> String`. Foreign keys are `Tag { is_definition: false, kind: "reference", name: <referenced table> }` at the column's line.

- [ ] **Step 1: Write the failing language tests** in `lang.rs`'s `mod tests`, next to the existing `from_path` test (line ~327):

```rust
    #[test]
    fn schema_files_are_a_third_category() {
        assert_eq!(Language::from_path("prisma/schema.prisma"), Some(Language::Prisma));
        assert_eq!(Language::from_path("db/dump.sql"), Some(Language::Sql));
        assert!(Language::Prisma.is_schema() && Language::Sql.is_schema());
        assert!(!Language::Prisma.is_document() && !Language::Sql.is_document());
        assert!(!Language::TypeScript.is_schema() && !Language::Markdown.is_schema());
        assert_eq!(Language::Prisma.as_str(), "prisma");
        assert_eq!(Language::Sql.as_str(), "sql");
    }
```

- [ ] **Step 2: Run it**: `cargo test -p singularrag-core schema_files_are_a_third_category` — Expected: FAIL to compile (`no variant named Prisma`).

- [ ] **Step 3: Add the variants** in `lang.rs`:

```rust
pub enum Language {
    TypeScript, Tsx, JavaScript, Rust, Markdown, Html, Css, Json, Yaml, Toml, Text,
    Prisma,
    Sql,
}
```
In `from_path` add `"prisma" => Some(Language::Prisma),` and `"sql" => Some(Language::Sql),` before `_ => None`. In `as_str` add `Language::Prisma => "prisma",` and `Language::Sql => "sql",`. After `is_document` add:

```rust
    /// Database schemas (schema spec §2): Prisma schemas and SQL DDL, parsed by hand in
    /// `schema.rs` into tables and columns.
    pub fn is_schema(self) -> bool {
        matches!(self, Language::Prisma | Language::Sql)
    }
```
In `extract_tags`, after the `is_document` early return:

```rust
    if lang.is_schema() {
        return Ok(crate::schema::extract(lang, source)?.tags);
    }
```
Add `pub mod schema;` to `lib.rs` (alphabetical, after `rank`).

- [ ] **Step 4: Write the failing Prisma tests.** Create `crates/singularrag-core/src/schema.rs` with only the module doc, the types and the tests for now:

```rust
//! Database schemas as tables and columns (schema spec §2–§3): a Prisma schema and SQL
//! DDL (migrations, dumps) parsed by hand into the same `Tag`s the indexer writes for
//! code, plus the alias vocabulary and the code-side table mentions. No model and no
//! connection: nothing here executes anything.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use regex::Regex;
use tree_sitter::Parser;

use crate::lang::{Language, Tag, SIGNATURE_MAX};
use crate::{Error, Result};

/// Tables and columns of one schema file, and the alias vocabulary it contributes:
/// `(alias, table_name)` pairs, where an alias is the table name itself, a Prisma model
/// name, or that model name in lower-camel form (`WebsiteEvent` → `websiteEvent`).
#[derive(Debug, Default, PartialEq)]
pub struct SchemaExtract {
    pub tags: Vec<Tag>,
    pub aliases: Vec<(String, String)>,
}

pub fn extract(lang: Language, source: &str) -> Result<SchemaExtract> {
    match lang {
        Language::Prisma => Ok(prisma(source)),
        Language::Sql => Ok(sql(source)),
        _ => Err(Error::Tags(format!("{lang:?} is not a schema language"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRISMA: &str = r#"generator client {
  provider = "prisma-client-js"
}

datasource db {
  provider = "postgresql"
}

model User {
  id        String    @id @map("user_id") @db.Uuid
  username  String    @unique @db.VarChar(255)
  // a comment line
  logoUrl   String?   @map("logo_url") @db.VarChar(2183)
  role      Role      @default(user)
  websites  Website[] @relation("user")

  @@map("user")
}

model Website {
  id        String   @id @map("website_id") @db.Uuid
  name      String   @db.VarChar(100)
  userId    String?  @map("user_id") @db.Uuid
  user      User?    @relation("user", fields: [userId], references: [id])
  createdBy String?  @map("created_by") @db.Uuid
  creator   User?    @relation("creator", fields: [createdBy], references: [id])

  @@index([userId])
  @@map("website")
}

model Team {
  id   String @id @db.Uuid
  name String
}

enum Role {
  admin
  user
}
"#;

    fn names(tags: &[Tag], kind: &str) -> Vec<String> {
        tags.iter().filter(|t| t.kind == kind && t.is_definition).map(|t| t.name.clone()).collect()
    }

    #[test]
    fn prisma_models_become_tables_named_by_map_and_fields_become_columns() {
        let x = extract(Language::Prisma, PRISMA).unwrap();
        assert_eq!(names(&x.tags, "table"), vec!["user", "website", "Team"]);
        assert_eq!(
            names(&x.tags, "column"),
            vec![
                "user.user_id", "user.username", "user.logo_url", "user.role",
                "website.website_id", "website.name", "website.user_id", "website.created_by",
                "Team.id", "Team.name",
            ]
        );
        let user = x.tags.iter().find(|t| t.name == "user").unwrap();
        assert_eq!((user.line_start, user.line_end), (9, 18));
        assert_eq!(user.signature, "table user (4 columns, model User)");
        let team = x.tags.iter().find(|t| t.name == "Team").unwrap();
        assert_eq!(team.signature, "table Team (2 columns)");
        let logo = x.tags.iter().find(|t| t.name == "user.logo_url").unwrap();
        assert_eq!(logo.signature, "String? @map(\"logo_url\") @db.VarChar(2183)");
        assert_eq!((logo.line_start, logo.line_end), (13, 13));
    }

    #[test]
    fn prisma_relations_are_not_columns_and_foreign_keys_sit_on_the_scalar_column() {
        let x = extract(Language::Prisma, PRISMA).unwrap();
        assert!(!x.tags.iter().any(|t| t.name.ends_with(".websites") || t.name.ends_with(".user") || t.name.ends_with(".creator")));
        let fks: Vec<(String, u32)> = x.tags.iter().filter(|t| !t.is_definition).map(|t| (t.name.clone(), t.line_start)).collect();
        // userId is line 23, createdBy line 25: both reference the `user` table
        assert_eq!(fks, vec![("user".to_string(), 23), ("user".to_string(), 25)]);
        assert!(x.tags.iter().filter(|t| !t.is_definition).all(|t| t.kind == "reference"));
    }

    #[test]
    fn prisma_aliases_are_the_table_the_model_and_its_lower_camel_form() {
        let x = extract(Language::Prisma, PRISMA).unwrap();
        assert_eq!(
            x.aliases,
            vec![
                ("user".to_string(), "user".to_string()),
                ("User".to_string(), "user".to_string()),
                ("website".to_string(), "website".to_string()),
                ("Website".to_string(), "website".to_string()),
                ("Team".to_string(), "Team".to_string()),
                ("team".to_string(), "Team".to_string()),
            ]
        );
        assert_eq!(lower_camel("WebsiteEvent"), "websiteEvent");
        assert_eq!(lower_camel(""), "");
    }

    #[test]
    fn prisma_enum_generator_and_datasource_produce_nothing() {
        let x = extract(Language::Prisma, "generator client {\n  provider = \"x\"\n}\ndatasource db {\n  provider = \"postgresql\"\n}\nenum Role {\n  admin\n}\n").unwrap();
        assert!(x.tags.is_empty() && x.aliases.is_empty());
    }

    #[test]
    fn an_unterminated_model_runs_to_the_end_of_the_file() {
        let src = "model A {\n  id String @id\n}\nmodel B {\n  id String @id\n  name String\n";
        let x = extract(Language::Prisma, src).unwrap();
        assert_eq!(names(&x.tags, "table"), vec!["A", "B"]);
        let b = x.tags.iter().find(|t| t.name == "B").unwrap();
        assert_eq!((b.line_start, b.line_end), (4, 6));
        assert_eq!(names(&x.tags, "column"), vec!["A.id", "B.id", "B.name"]);
    }

    #[test]
    fn a_long_column_signature_is_cut_at_the_cap() {
        let attrs = "@db.VarChar(1) ".repeat(20);
        let src = format!("model A {{\n  id String {attrs}\n}}\n");
        let x = extract(Language::Prisma, &src).unwrap();
        let col = x.tags.iter().find(|t| t.name == "A.id").unwrap();
        assert_eq!(col.signature.chars().count(), SIGNATURE_MAX);
        assert!(col.signature.ends_with('…'));
    }
}
```
Add a temporary `fn prisma(_: &str) -> SchemaExtract { SchemaExtract::default() }` and `fn sql(_: &str) -> SchemaExtract { SchemaExtract::default() }` and `pub fn lower_camel(_: &str) -> String { String::new() }` so it compiles (the `HashSet`, `OnceLock`, `Regex`, `Parser` imports are used from Task 2 and Task 4; add `#[allow(unused_imports)]` on them for now and remove it in Task 4).

- [ ] **Step 5: Run them**: `cargo test -p singularrag-core schema::tests` — Expected: the six tests FAIL on assertions (empty tags).

- [ ] **Step 6: Implement the Prisma parser** in `schema.rs` (replace the stubs for `prisma` and `lower_camel`; keep the `sql` stub for Task 2):

```rust
/// Whitespace-collapsed and cut at `SIGNATURE_MAX` with an ellipsis, like `lang::signature_for`.
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

/// 1-based line containing `byte`.
fn line_of(source: &str, byte: usize) -> u32 {
    source.as_bytes()[..byte.min(source.len())]
        .iter()
        .filter(|&&b| b == b'\n')
        .count() as u32
        + 1
}

fn def(name: String, kind: &str, line_start: u32, line_end: u32, signature: String) -> Tag {
    Tag { name, kind: kind.into(), is_definition: true, line_start, line_end, signature }
}

/// A foreign key: a reference tag named after the referenced table, at the column's line.
/// The indexer writes non-definition tags as `refs` rows (spec §3).
fn fk(table: String, line: u32) -> Tag {
    Tag { name: table, kind: "reference".into(), is_definition: false, line_start: line, line_end: line, signature: String::new() }
}

/// `WebsiteEvent` → `websiteEvent`: the Prisma client's property for a model.
pub fn lower_camel(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_lowercase().chain(c).collect(),
        None => String::new(),
    }
}

// ---- Prisma ----

/// The model name of a `model Name {` line.
fn model_header(line: &str) -> Option<String> {
    let t = line.trim();
    let rest = t.strip_prefix("model ")?;
    let name: String = rest.trim_start().chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
    (!name.is_empty() && t.contains('{')).then_some(name)
}

/// The string argument of `prefix` in `s`: `attr_string("x @map(\"logo_url\")", "@map(")` is `logo_url`.
fn attr_string(s: &str, prefix: &str) -> Option<String> {
    let i = s.find(prefix)? + prefix.len();
    let rest = &s[i..];
    let q = rest.find('"')?;
    let after = &rest[q + 1..];
    let end = after.find('"')?;
    Some(after[..end].to_string())
}

/// `(field name, rest)` of a field line; `None` for anything that is not `name Type ...`.
fn split_field(t: &str) -> Option<(&str, &str)> {
    let (name, rest) = t.split_once(char::is_whitespace)?;
    if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    let rest = rest.trim();
    (!rest.is_empty()).then_some((name, rest))
}

/// The first field named in `@relation(fields: [a, b], ...)`.
fn relation_field(attrs: &str) -> Option<String> {
    let i = attrs.find("fields:")? + "fields:".len();
    let rest = attrs[i..].trim_start().strip_prefix('[')?;
    let end = rest.find(']')?;
    rest[..end].split(',').next().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn prisma(source: &str) -> SchemaExtract {
    let lines: Vec<&str> = source.lines().collect();
    // (model, table, first line index, last line index); an unterminated model runs to EOF
    let mut blocks: Vec<(String, String, usize, usize)> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let Some(model) = model_header(lines[i]) else {
            i += 1;
            continue;
        };
        let mut end = lines.len().saturating_sub(1);
        let mut table = model.clone();
        for (j, l) in lines.iter().enumerate().skip(i + 1) {
            let t = l.trim();
            if t == "}" {
                end = j;
                break;
            }
            if t.starts_with("@@map(") {
                if let Some(m) = attr_string(t, "@@map(") {
                    table = m;
                }
            }
        }
        blocks.push((model, table, i, end));
        i = end + 1;
    }
    let tables: HashMap<&str, &str> = blocks.iter().map(|(m, t, _, _)| (m.as_str(), t.as_str())).collect();
    let mut out = SchemaExtract::default();
    for (model, table, start, end) in &blocks {
        let mut columns: Vec<Tag> = Vec::new();
        let mut scalar_lines: HashMap<String, u32> = HashMap::new();
        let mut fks: Vec<(String, String, u32)> = Vec::new(); // (local field, referenced table, relation line)
        for j in start + 1..*end {
            let t = lines[j].trim();
            if t.is_empty() || t.starts_with("//") || t.starts_with("@@") {
                continue;
            }
            let Some((name, rest)) = split_field(t) else { continue };
            let (ty, attrs) = rest
                .split_once(char::is_whitespace)
                .map(|(a, b)| (a, b.trim()))
                .unwrap_or((rest, ""));
            let base = ty.trim_end_matches("[]").trim_end_matches('?');
            if let Some(target) = tables.get(base) {
                if let Some(f) = relation_field(attrs) {
                    fks.push((f, target.to_string(), j as u32 + 1));
                }
                continue;
            }
            if attrs.contains("@relation(") {
                continue;
            }
            let col = attr_string(attrs, "@map(").unwrap_or_else(|| name.to_string());
            scalar_lines.insert(name.to_string(), j as u32 + 1);
            columns.push(def(
                format!("{table}.{col}"),
                "column",
                j as u32 + 1,
                j as u32 + 1,
                truncate(&format!("{ty} {attrs}")),
            ));
        }
        let sig = if model == table {
            format!("table {table} ({} columns)", columns.len())
        } else {
            format!("table {table} ({} columns, model {model})", columns.len())
        };
        out.tags.push(def(table.clone(), "table", *start as u32 + 1, *end as u32 + 1, sig));
        out.tags.extend(columns);
        for (field, target, rel_line) in fks {
            let line = scalar_lines.get(&field).copied().unwrap_or(rel_line);
            out.tags.push(fk(target, line));
        }
        out.aliases.push((table.clone(), table.clone()));
        if model != table {
            out.aliases.push((model.clone(), table.clone()));
        }
        let lc = lower_camel(model);
        if lc != *table && lc != *model {
            out.aliases.push((lc, table.clone()));
        }
    }
    out
}
```

- [ ] **Step 7: Run the tests**: `cargo test -p singularrag-core schema::tests` and `cargo test -p singularrag-core lang::` — Expected: all PASS. Check the line-number assertions against the `PRISMA` constant by counting lines if one fails: `model User {` is line 9, its `}` line 18, `logoUrl` line 13, `userId` line 23, `createdBy` line 25.

- [ ] **Step 8: Gate and commit**

```bash
cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add -A crates/singularrag-core/src/lang.rs crates/singularrag-core/src/lib.rs crates/singularrag-core/src/schema.rs
git commit -m "feat(core): Prisma and SQL as schema languages; Prisma models as table and column tags

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 2: The SQL DDL scanner

**Files:**
- Modify: `crates/singularrag-core/src/schema.rs` (replace the `sql` stub)
- Test: unit tests in `schema.rs`

**Interfaces:**
- Consumes: `truncate`, `line_of`, `def`, `fk`, `SchemaExtract` from Task 1.
- Produces: `fn sql(source: &str) -> SchemaExtract` (private, reached through `extract(Language::Sql, ..)`).

- [ ] **Step 1: Write the failing tests** in `schema.rs`'s `mod tests`:

```rust
    const MIGRATION: &str = r#"-- CreateExtension
CREATE EXTENSION IF NOT EXISTS "pgcrypto";

-- CreateTable
CREATE TABLE "user" (
    "user_id" UUID NOT NULL,
    "username" VARCHAR(255) NOT NULL, -- unique below
    "created_at" TIMESTAMPTZ(6) DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT "user_pkey" PRIMARY KEY ("user_id")
);

-- CreateTable
CREATE TABLE IF NOT EXISTS "website" (
    "website_id" UUID NOT NULL,
    "name" VARCHAR(100) NOT NULL,
    "user_id" UUID REFERENCES "user"("user_id"),
    "labels" TEXT DEFAULT 'a,b',
    CONSTRAINT "website_pkey" PRIMARY KEY ("website_id"),
    CONSTRAINT "website_user_fk" FOREIGN KEY ("user_id") REFERENCES "user"("user_id")
);

-- CreateIndex
CREATE INDEX "website_user_id_idx" ON "website"("user_id");

-- AlterTable
ALTER TABLE "website" ADD COLUMN "domain" VARCHAR(500), ADD CONSTRAINT "x" UNIQUE ("name");
ALTER TABLE ONLY "website" ADD "share_id" VARCHAR(50);
ALTER TABLE "website" DROP COLUMN "labels";
"#;

    #[test]
    fn create_table_statements_become_tables_with_columns_and_constraints_left_out() {
        let x = extract(Language::Sql, MIGRATION).unwrap();
        assert_eq!(names(&x.tags, "table"), vec!["user", "website"]);
        assert_eq!(
            names(&x.tags, "column"),
            vec![
                "user.user_id", "user.username", "user.created_at",
                "website.website_id", "website.name", "website.user_id", "website.labels",
                "website.domain", "website.share_id",
            ]
        );
        let user = x.tags.iter().find(|t| t.name == "user").unwrap();
        assert_eq!((user.line_start, user.line_end), (5, 11));
        assert_eq!(user.signature, "table user (3 columns)");
        let username = x.tags.iter().find(|t| t.name == "user.username").unwrap();
        assert_eq!(username.signature, "VARCHAR(255) NOT NULL");
        assert_eq!(username.line_start, 7);
        let labels = x.tags.iter().find(|t| t.name == "website.labels").unwrap();
        assert_eq!(labels.signature, "TEXT DEFAULT 'a,b'");
        let domain = x.tags.iter().find(|t| t.name == "website.domain").unwrap();
        assert_eq!((domain.signature.as_str(), domain.line_start), ("VARCHAR(500)", 27));
        let share = x.tags.iter().find(|t| t.name == "website.share_id").unwrap();
        assert_eq!((share.signature.as_str(), share.line_start), ("VARCHAR(50)", 28));
        assert_eq!(x.aliases, vec![("user".to_string(), "user".to_string()), ("website".to_string(), "website".to_string())]);
    }

    #[test]
    fn inline_and_constraint_foreign_keys_become_references_at_their_lines() {
        let x = extract(Language::Sql, MIGRATION).unwrap();
        let fks: Vec<(String, u32)> = x.tags.iter().filter(|t| !t.is_definition).map(|t| (t.name.clone(), t.line_start)).collect();
        assert_eq!(fks, vec![("user".to_string(), 17), ("user".to_string(), 20)]);
    }

    #[test]
    fn quoted_backticked_and_schema_prefixed_names_are_unquoted() {
        let src = "CREATE TABLE `team_user` (\n  `id` INT,\n  KEY `k` (`id`)\n) ENGINE=InnoDB;\nCREATE TABLE umami.website_event\n(\n    website_id UUID,\n    hostname LowCardinality(String),\n    INDEX idx hostname TYPE bloom_filter GRANULARITY 1\n)\nENGINE = MergeTree\nORDER BY (website_id);\nCREATE TABLE \"public\".\"report\" (report_id uuid);\n";
        let x = extract(Language::Sql, src).unwrap();
        assert_eq!(names(&x.tags, "table"), vec!["team_user", "website_event", "report"]);
        assert_eq!(names(&x.tags, "column"), vec!["team_user.id", "website_event.website_id", "website_event.hostname", "report.report_id"]);
        let host = x.tags.iter().find(|t| t.name == "website_event.hostname").unwrap();
        assert_eq!(host.signature, "LowCardinality(String)");
    }

    #[test]
    fn views_are_tables_whose_signature_says_view() {
        let src = "CREATE MATERIALIZED VIEW umami.website_event_stats_hourly_mv\nTO umami.website_event_stats_hourly\nAS SELECT website_id FROM umami.website_event;\nCREATE OR REPLACE VIEW v_sessions AS SELECT 1;\n";
        let x = extract(Language::Sql, src).unwrap();
        let v: Vec<(String, String)> = x.tags.iter().map(|t| (t.name.clone(), t.signature.clone())).collect();
        assert_eq!(v, vec![
            ("website_event_stats_hourly_mv".to_string(), "view website_event_stats_hourly_mv".to_string()),
            ("v_sessions".to_string(), "view v_sessions".to_string()),
        ]);
    }

    #[test]
    fn a_copy_data_block_and_inserts_are_dropped() {
        let src = "CREATE TABLE public.report (\n    report_id uuid NOT NULL\n);\n\nCOPY public.report (report_id) FROM stdin;\n0b1c\tCREATE TABLE phantom (x int);\n\\.\n\nINSERT INTO public.report VALUES ('CREATE TABLE phantom2 (y int);');\nALTER TABLE ONLY public.report\n    ADD CONSTRAINT report_website_fk FOREIGN KEY (website_id) REFERENCES public.website(website_id);\n";
        let x = extract(Language::Sql, src).unwrap();
        assert_eq!(names(&x.tags, "table"), vec!["report"]);
        assert_eq!(names(&x.tags, "column"), vec!["report.report_id"]);
        let fks: Vec<(String, u32)> = x.tags.iter().filter(|t| !t.is_definition).map(|t| (t.name.clone(), t.line_start)).collect();
        assert_eq!(fks, vec![("website".to_string(), 11)]);
    }

    #[test]
    fn a_malformed_statement_costs_itself_not_the_file() {
        let src = "CREATE TABLE broken (\n  id int\n-- no closing paren, no semicolon until the next statement\nCREATE TABLE good (id int);\nCREATE TABLE also_good (id int);\n";
        let x = extract(Language::Sql, src).unwrap();
        // `broken` swallows `good` (one statement up to the first `;`) and yields no columns it can trust;
        // `also_good` parses.
        assert!(names(&x.tags, "table").contains(&"also_good".to_string()));
        assert!(names(&x.tags, "column").contains(&"also_good.id".to_string()));
    }

    #[test]
    fn dollar_quoted_bodies_and_block_comments_hide_their_semicolons() {
        let src = "CREATE FUNCTION f() RETURNS void AS $$ BEGIN PERFORM 1; END; $$ LANGUAGE plpgsql;\n/* CREATE TABLE commented (x int); */\nCREATE TABLE real_one (id int);\n";
        let x = extract(Language::Sql, src).unwrap();
        assert_eq!(names(&x.tags, "table"), vec!["real_one"]);
    }
```

- [ ] **Step 2: Run them**: `cargo test -p singularrag-core schema::tests` — Expected: the seven new tests FAIL (the stub returns nothing).

- [ ] **Step 3: Implement the scanner** (replace the `sql` stub):

```rust
// ---- SQL DDL ----

/// Statements as `(byte offset, text)`, split at semicolons outside quotes, comments and
/// `$$` bodies. Comments are blanked to spaces (newlines kept) so byte offsets into the
/// text still map to source lines. A `COPY ... FROM stdin` statement's data block, up to
/// the `\.` line, is dropped with it.
fn statements(source: &str) -> Vec<(usize, String)> {
    let b = source.as_bytes();
    let mut blanked: Vec<u8> = b.to_vec();
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    let mut skip_data = false;
    while i < b.len() {
        if skip_data {
            let eol = source[i..].find('\n').map_or(b.len(), |n| i + n);
            let line = source[i..eol].trim();
            for k in i..eol {
                blanked[k] = b' ';
            }
            i = (eol + 1).min(b.len());
            if line == "\\." {
                skip_data = false;
                start = i;
            }
            continue;
        }
        let c = b[i];
        match c {
            b'-' if b.get(i + 1) == Some(&b'-') => {
                let eol = source[i..].find('\n').map_or(b.len(), |n| i + n);
                for k in i..eol {
                    blanked[k] = b' ';
                }
                i = eol;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let end = source[i + 2..].find("*/").map_or(b.len(), |n| i + 2 + n + 2);
                for k in i..end.min(b.len()) {
                    if blanked[k] != b'\n' {
                        blanked[k] = b' ';
                    }
                }
                i = end;
            }
            b'\'' | b'"' | b'`' => {
                i += 1;
                while i < b.len() && b[i] != c {
                    i += 1;
                }
                i += 1;
            }
            b'$' if b.get(i + 1) == Some(&b'$') => {
                let end = source[i + 2..].find("$$").map_or(b.len(), |n| i + 2 + n + 2);
                i = end;
            }
            b';' => {
                let text = String::from_utf8_lossy(&blanked[start..i]).into_owned();
                let head = text.trim_start();
                let is_copy = head.get(..4).is_some_and(|h| h.eq_ignore_ascii_case("copy"))
                    && text.to_ascii_uppercase().contains("FROM STDIN");
                out.push((start, text));
                i += 1;
                start = i;
                if is_copy {
                    // the data begins on the next line
                    let eol = source[i.min(b.len())..].find('\n').map_or(b.len(), |n| i + n);
                    i = (eol + 1).min(b.len());
                    start = i;
                    skip_data = true;
                }
            }
            _ => i += 1,
        }
    }
    if start < b.len() {
        let text = String::from_utf8_lossy(&blanked[start..]).into_owned();
        if !text.trim().is_empty() {
            out.push((start, text));
        }
    }
    out
}

/// `"public"."user"` → `user`; `` `team_user` `` → `team_user`; `umami.website_event` → `website_event`.
fn unquote(ident: &str) -> String {
    let t = ident.trim().trim_end_matches('(').trim();
    let last = t.rsplit('.').next().unwrap_or(t);
    last.trim_matches(|c| c == '"' || c == '`' || c == '[' || c == ']').to_string()
}

/// `(offset of the first `(`, offset of its matching `)`)` in `s`, quotes respected.
fn paren_span(s: &str) -> Option<(usize, usize)> {
    let open = s.find('(')?;
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    for (i, &c) in s.as_bytes().iter().enumerate().skip(open) {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
            }
            None => match c {
                b'\'' | b'"' | b'`' => quote = Some(c),
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some((open, i));
                    }
                }
                _ => {}
            },
        }
    }
    None
}

/// Top-level comma-separated pieces of a body as `(offset within body, trimmed text)`.
fn pieces(body: &str) -> Vec<(usize, &str)> {
    let mut raw = Vec::new();
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    let mut start = 0usize;
    for (i, &c) in body.as_bytes().iter().enumerate() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
            }
            None => match c {
                b'\'' | b'"' | b'`' => quote = Some(c),
                b'(' => depth += 1,
                b')' => depth -= 1,
                b',' if depth == 0 => {
                    raw.push((start, &body[start..i]));
                    start = i + 1;
                }
                _ => {}
            },
        }
    }
    raw.push((start, &body[start..]));
    raw.into_iter()
        .filter(|(_, p)| !p.trim().is_empty())
        .map(|(o, p)| {
            let lead = p.len() - p.trim_start().len();
            (o + lead, p.trim())
        })
        .collect()
}

const CONSTRAINT_WORDS: [&str; 12] = [
    "CONSTRAINT", "PRIMARY", "UNIQUE", "INDEX", "KEY", "FOREIGN", "CHECK", "EXCLUDE",
    "FULLTEXT", "SPATIAL", "PROJECTION", "LIKE",
];

fn first_word_upper(s: &str) -> String {
    s.split_whitespace()
        .next()
        .unwrap_or("")
        .trim_matches(|c| c == '(' || c == ',')
        .to_ascii_uppercase()
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let p = s.get(..prefix.len())?;
    p.eq_ignore_ascii_case(prefix).then(|| &s[prefix.len()..])
}

fn sql_regexes() -> &'static (Regex, Regex, Regex, Regex) {
    static R: OnceLock<(Regex, Regex, Regex, Regex)> = OnceLock::new();
    R.get_or_init(|| {
        (
            Regex::new(r"(?is)^\s*CREATE\s+(?:OR\s+REPLACE\s+)?(?:(?:GLOBAL|LOCAL)\s+)?(?:(?:TEMP|TEMPORARY|UNLOGGED)\s+)?TABLE\s+(?:IF\s+NOT\s+EXISTS\s+)?([^\s(;]+)").unwrap(),
            Regex::new(r"(?is)^\s*CREATE\s+(?:OR\s+REPLACE\s+)?(?:MATERIALIZED\s+)?VIEW\s+(?:IF\s+NOT\s+EXISTS\s+)?([^\s(;]+)").unwrap(),
            Regex::new(r"(?is)^\s*ALTER\s+TABLE\s+(?:ONLY\s+)?(?:IF\s+EXISTS\s+)?([^\s(;]+)\s+(.*)$").unwrap(),
            Regex::new(r"(?is)REFERENCES\s+([^\s(,;]+)").unwrap(),
        )
    })
}

fn sql(source: &str) -> SchemaExtract {
    let (re_table, re_view, re_alter, re_refs) = sql_regexes();
    let mut out = SchemaExtract::default();
    let mut seen: HashSet<String> = HashSet::new();
    for (off, stmt) in statements(source) {
        let s = stmt.as_str();
        if let Some(m) = re_table.captures(s) {
            let name = unquote(&m[1]);
            let name_at = off + m.get(1).expect("group 1").start();
            let after = m.get(0).expect("group 0").end();
            let mut columns = Vec::new();
            let mut refs = Vec::new();
            if s[after..].trim_start().starts_with('(') {
                if let Some((open, close)) = paren_span(&s[after..]) {
                    let body_off = after + open + 1;
                    let body = &s[body_off..after + close];
                    for (po, piece) in pieces(body) {
                        let line = line_of(source, off + body_off + po);
                        let first = first_word_upper(piece);
                        if CONSTRAINT_WORDS.contains(&first.as_str()) {
                            if let Some(r) = re_refs.captures(piece) {
                                refs.push(fk(unquote(&r[1]), line));
                            }
                            continue;
                        }
                        let (col, ty) = piece.split_once(char::is_whitespace).unwrap_or((piece, ""));
                        if let Some(r) = re_refs.captures(ty) {
                            refs.push(fk(unquote(&r[1]), line));
                        }
                        columns.push(def(format!("{name}.{}", unquote(col)), "column", line, line, truncate(ty)));
                    }
                }
            }
            let start_line = line_of(source, name_at);
            let end_line = line_of(source, off + s.trim_end().len().saturating_sub(1));
            out.tags.push(def(name.clone(), "table", start_line, end_line.max(start_line), format!("table {name} ({} columns)", columns.len())));
            out.tags.extend(columns);
            out.tags.extend(refs);
            if seen.insert(name.clone()) {
                out.aliases.push((name.clone(), name));
            }
        } else if let Some(m) = re_view.captures(s) {
            let name = unquote(&m[1]);
            let line = line_of(source, off + m.get(1).expect("group 1").start());
            out.tags.push(def(name.clone(), "table", line, line, format!("view {name}")));
            if seen.insert(name.clone()) {
                out.aliases.push((name.clone(), name));
            }
        } else if let Some(m) = re_alter.captures(s) {
            let table = unquote(&m[1]);
            let actions_off = m.get(2).expect("group 2").start();
            for (po, action) in pieces(&m[2]) {
                let line = line_of(source, off + actions_off + po);
                let Some(rest) = strip_prefix_ci(action, "ADD") else { continue };
                if !rest.starts_with(char::is_whitespace) {
                    continue;
                }
                let rest = rest.trim_start();
                if CONSTRAINT_WORDS.contains(&first_word_upper(rest).as_str()) {
                    if let Some(r) = re_refs.captures(action) {
                        out.tags.push(fk(unquote(&r[1]), line));
                    }
                    continue;
                }
                let rest = strip_prefix_ci(rest, "COLUMN").map(str::trim_start).unwrap_or(rest);
                let rest = strip_prefix_ci(rest, "IF NOT EXISTS").map(str::trim_start).unwrap_or(rest);
                let (col, ty) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                if let Some(r) = re_refs.captures(ty) {
                    out.tags.push(fk(unquote(&r[1]), line));
                }
                out.tags.push(def(format!("{table}.{}", unquote(col)), "column", line, line, truncate(ty)));
            }
        }
    }
    out
}
```

- [ ] **Step 4: Run the tests**: `cargo test -p singularrag-core schema::tests` — Expected: all PASS. If a line assertion is off by one, the offending offset is a `pieces` lead or the `COPY` skip; fix the code, not the test, after re-counting the constant's lines (`CREATE TABLE "user"` is line 5 of `MIGRATION` and its `);` line 11, `"username"` line 7, the inline `REFERENCES` line 17, the `FOREIGN KEY` constraint line 20, the `ADD COLUMN "domain"` line 27, the `ADD "share_id"` line 28; in the COPY test the `ADD CONSTRAINT` line is 11).

- [ ] **Step 5: Gate and commit**

```bash
cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add -A crates/singularrag-core/src/schema.rs
git commit -m "feat(core): SQL DDL scanner: tables, columns, foreign keys; data blocks dropped

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 3: The store, the indexer's two phases and the schema fixture

**Files:**
- Modify: `crates/singularrag-core/src/store/schema.rs` (`SCHEMA_VERSION` 7, `table_names`)
- Modify: `crates/singularrag-core/src/index.rs` (`refresh_with`, new `process`, `index_file`, `delete_symbols_for`, `load_vocab`, `vocab_hash`)
- Modify: `crates/singularrag-core/src/fixture.rs` (`write_schema_mini`)
- Test: `index.rs` `mod tests`

**Interfaces:**
- Consumes: `Language::is_schema`, `schema::extract` (Tasks 1–2).
- Produces: `pub type Vocab = HashMap<String, String>` (alias → table name); `pub fn load_vocab(conn: &Connection) -> Result<Vocab>`; meta key `table_vocab`; `fixture::write_schema_mini(root: &Path)`. `index_file` takes `vocab: &Vocab, force: bool`; Task 4 adds the call to `schema::table_mentions` inside it.

- [ ] **Step 1: Add the fixture** to `fixture.rs` (after `write_prose`). The schema's `model Website {` must be line 18 and its `domain` field line 21: Task 7's patch depends on it.

```rust
/// A codebase with its database (schema spec §5): a Prisma schema with three models, two
/// migrations, a dump with a data block, one query module that names tables the two ways
/// code does (a client property, a SQL string) and the way the rule ignores (a variable
/// called `session`), and a document whose process step names a table in a code span.
pub fn write_schema_mini(root: &Path) {
    w(root, ".gitignore", "dist/\n.singularrag/index.db\n");
    w(
        root,
        "prisma/schema.prisma",
        r#"datasource db {
  provider = "postgresql"
}

generator client {
  provider = "prisma-client-js"
}

model User {
  id        String    @id @map("user_id") @db.Uuid
  username  String    @unique @db.VarChar(255)
  createdAt DateTime? @default(now()) @map("created_at")
  websites  Website[]

  @@map("user")
}

model Website {
  id        String         @id @map("website_id") @db.Uuid
  name      String         @db.VarChar(100)
  domain    String?        @db.VarChar(500)
  userId    String?        @map("user_id") @db.Uuid
  user      User?          @relation(fields: [userId], references: [id])
  events    WebsiteEvent[]

  @@index([userId])
  @@map("website")
}

model WebsiteEvent {
  id        String   @id @map("event_id") @db.Uuid
  websiteId String   @map("website_id") @db.Uuid
  urlPath   String   @map("url_path") @db.VarChar(500)
  eventName String?  @map("event_name") @db.VarChar(50)
  createdAt DateTime @default(now()) @map("created_at")
  website   Website  @relation(fields: [websiteId], references: [id])

  @@map("website_event")
}

enum Role {
  admin
  user
}
"#,
    );
    w(
        root,
        "prisma/migrations/01_init/migration.sql",
        r#"-- CreateTable
CREATE TABLE "user" (
    "user_id" UUID NOT NULL,
    "username" VARCHAR(255) NOT NULL,
    "created_at" TIMESTAMPTZ(6) DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT "user_pkey" PRIMARY KEY ("user_id")
);

-- CreateTable
CREATE TABLE "website" (
    "website_id" UUID NOT NULL,
    "name" VARCHAR(100) NOT NULL,
    "domain" VARCHAR(500),
    "user_id" UUID REFERENCES "user"("user_id"),

    CONSTRAINT "website_pkey" PRIMARY KEY ("website_id")
);

-- CreateTable
CREATE TABLE "website_event" (
    "event_id" UUID NOT NULL,
    "website_id" UUID NOT NULL,
    "url_path" VARCHAR(500) NOT NULL,
    "created_at" TIMESTAMPTZ(6) NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT "website_event_pkey" PRIMARY KEY ("event_id"),
    CONSTRAINT "website_event_website_fk" FOREIGN KEY ("website_id") REFERENCES "website"("website_id")
);

-- CreateIndex
CREATE INDEX "website_event_website_id_idx" ON "website_event"("website_id");
"#,
    );
    w(
        root,
        "prisma/migrations/02_add_event_name/migration.sql",
        "-- AlterTable\nALTER TABLE \"website_event\" ADD COLUMN \"event_name\" VARCHAR(50);\n",
    );
    w(
        root,
        "db/dump.sql",
        r#"--
-- PostgreSQL database dump (schema and data)
--
SET statement_timeout = 0;

CREATE TABLE public.report (
    report_id uuid NOT NULL,
    website_id uuid NOT NULL,
    name character varying(200) NOT NULL
);

ALTER TABLE ONLY public.report
    ADD CONSTRAINT report_website_fk FOREIGN KEY (website_id) REFERENCES public.website(website_id);

COPY public.report (report_id, website_id, name) FROM stdin;
0b1c	9f3a	Weekly (CREATE TABLE in data, not DDL)
\.

INSERT INTO public.report VALUES ('0b1d', '9f3a', 'CREATE TABLE phantom (x int)');
"#,
    );
    w(
        root,
        "src/queries/website.ts",
        r#"import { prisma } from "../lib/prisma";
import { rawQuery } from "../lib/db";

export function getWebsite(websiteId: string) {
  return prisma.client.website.findUnique({ where: { id: websiteId } });
}

export function getWebsiteDateRange(websiteId: string) {
  return rawQuery(`
    select min(created_at) as mindate, max(created_at) as maxdate
    from website_event
    where website_id = {{websiteId::uuid}}
  `, { websiteId });
}

export function touchSession(id: string) {
  const session = { id, seenAt: Date.now() };
  return session;
}
"#,
    );
    w(
        root,
        "src/lib/prisma.ts",
        "export const prisma = { client: {} as Record<string, { findUnique: (q: unknown) => unknown }> };\n",
    );
    w(
        root,
        "src/lib/db.ts",
        "export function rawQuery(sql: string, params: Record<string, string>) {\n  return { sql, params };\n}\n",
    );
    w(
        root,
        "docs/data-model.md",
        "# Data model\n\n## Recording a visit\n\nWhen a visitor opens a page, the tracker script posts to the collect endpoint. The collector looks up the website row by its tracking id, then writes one row to the `website_event` table with the URL path and the event name. Reports read `website_event` through `getWebsiteDateRange`.\n",
    );
}
```

- [ ] **Step 2: Write the failing indexer tests** in `index.rs`'s `mod tests` (the module has `setup`, `count`, and uses `write_ts_mini`; add `write_schema_mini` to the `use crate::fixture::{...}` line):

```rust
    fn index_dir(dir: &Path) -> Store {
        Store::open(&dir.join(".singularrag/index.db")).unwrap()
    }

    fn refresh(store: &Store, dir: &Path) -> IndexStats {
        let cfg = MapConfig::default();
        Indexer::new(store, &Workspace::single(dir).unwrap(), &cfg)
            .unwrap()
            .refresh(None)
            .unwrap()
    }

    fn symbols_of(store: &Store, path: &str, kind: &str) -> Vec<String> {
        let mut stmt = store
            .conn()
            .prepare("SELECT s.name FROM symbols s JOIN files f ON f.id = s.file_id WHERE f.path = ?1 AND s.kind = ?2 ORDER BY s.line_start")
            .unwrap();
        stmt.query_map([path, kind], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect()
    }

    fn refs_named(store: &Store, name: &str) -> Vec<String> {
        let mut stmt = store
            .conn()
            .prepare("SELECT DISTINCT f.path FROM refs r JOIN files f ON f.id = r.file_id WHERE r.name = ?1 ORDER BY f.path")
            .unwrap();
        stmt.query_map([name], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect()
    }

    #[test]
    fn schema_files_index_tables_columns_foreign_keys_and_aliases() {
        let dir = tempfile::tempdir().unwrap();
        write_schema_mini(dir.path());
        let store = index_dir(dir.path());
        let stats = refresh(&store, dir.path());
        assert_eq!(stats.remaining, 0);
        assert_eq!(symbols_of(&store, "prisma/schema.prisma", "table"), vec!["user", "website", "website_event"]);
        assert!(symbols_of(&store, "prisma/schema.prisma", "column").contains(&"website_event.website_id".to_string()));
        assert_eq!(symbols_of(&store, "prisma/migrations/02_add_event_name/migration.sql", "column"), vec!["website_event.event_name"]);
        assert_eq!(symbols_of(&store, "db/dump.sql", "table"), vec!["report"]);
        // foreign keys: the schema, the init migration and the dump all reference `website`
        // (`contains`, not equality: Task 4 adds the code file to this list)
        let website_refs = refs_named(&store, "website");
        for p in ["db/dump.sql", "prisma/migrations/01_init/migration.sql", "prisma/schema.prisma"] {
            assert!(website_refs.contains(&p.to_string()), "{p} missing from {website_refs:?}");
        }
        let lang: String = store
            .conn()
            .query_row("SELECT lang FROM files WHERE path = 'prisma/schema.prisma'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(lang, "prisma");
        let vocab = load_vocab(store.conn()).unwrap();
        assert_eq!(vocab.get("websiteEvent").map(String::as_str), Some("website_event"));
        assert_eq!(vocab.get("WebsiteEvent").map(String::as_str), Some("website_event"));
        assert_eq!(vocab.get("report").map(String::as_str), Some("report"));
        assert_eq!(
            count(&store, "SELECT COUNT(*) FROM extract_queue q JOIN symbols s ON s.id = q.symbol_id WHERE s.kind IN ('table', 'column')"),
            0,
            "a table is never queued for extraction"
        );
        assert!(count(&store, "SELECT COUNT(*) FROM extract_queue") >= 1, "the document section is queued");
    }

    #[test]
    fn a_vocabulary_change_reindexes_code_files_once() {
        let dir = tempfile::tempdir().unwrap();
        write_schema_mini(dir.path());
        std::fs::write(
            dir.path().join("src/queries/team.ts"),
            "import { prisma } from \"../lib/prisma\";\nexport function getTeams() {\n  return prisma.client.team.findUnique({});\n}\n",
        )
        .unwrap();
        let store = index_dir(dir.path());
        refresh(&store, dir.path());
        assert!(refs_named(&store, "team").is_empty(), "no `team` table yet, so no ref");
        let second = refresh(&store, dir.path());
        assert_eq!(second.indexed, 0, "nothing changed, nothing re-indexed");

        // a new model appears; the code files' mtimes do not change
        let schema = dir.path().join("prisma/schema.prisma");
        let mut src = std::fs::read_to_string(&schema).unwrap();
        src.push_str("\nmodel Team {\n  id String @id\n\n  @@map(\"team\")\n}\n");
        std::fs::write(&schema, src).unwrap();
        let third = refresh(&store, dir.path());
        assert_eq!(refs_named(&store, "team"), vec!["src/queries/team.ts"]);
        // the schema file plus every code file (website.ts, prisma.ts, db.ts, team.ts), not the documents
        assert_eq!(third.indexed, 5, "{third:?}");
        let fourth = refresh(&store, dir.path());
        assert_eq!(fourth.indexed, 0, "the forced re-index happens once: {fourth:?}");
    }

    #[test]
    fn an_empty_vocabulary_forces_nothing_and_emits_nothing() {
        let (dir, store) = setup();
        let first = refresh(&store, dir.path());
        assert!(first.indexed > 0);
        assert_eq!(count(&store, "SELECT COUNT(*) FROM table_names"), 0);
        let second = refresh(&store, dir.path());
        assert_eq!(second.indexed, 0, "{second:?}");
        assert_eq!(store.get_meta("table_vocab").unwrap().as_deref(), Some(vocab_hash(&Vocab::new()).as_str()));
    }

    #[test]
    fn removing_a_schema_file_removes_its_aliases() {
        let dir = tempfile::tempdir().unwrap();
        write_schema_mini(dir.path());
        let store = index_dir(dir.path());
        refresh(&store, dir.path());
        assert!(load_vocab(store.conn()).unwrap().contains_key("report"));
        std::fs::remove_file(dir.path().join("db/dump.sql")).unwrap();
        refresh(&store, dir.path());
        assert!(!load_vocab(store.conn()).unwrap().contains_key("report"));
        assert!(load_vocab(store.conn()).unwrap().contains_key("website"));
    }
```
Add `use std::path::Path;` inside the tests module if `Path` is not already in scope there (it is imported at the top of `index.rs`).

- [ ] **Step 3: Run them**: `cargo test -p singularrag-core index::tests` — Expected: FAIL to compile (`load_vocab`, `vocab_hash`, `Vocab`, `write_schema_mini` missing).

- [ ] **Step 4: Bump the store schema** in `store/schema.rs`: `pub const SCHEMA_VERSION: i64 = 7;` and append to `DDL` after `steps_cache`:

```sql
CREATE TABLE IF NOT EXISTS table_names (
  alias      TEXT NOT NULL,
  table_name TEXT NOT NULL,
  file_id    INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  PRIMARY KEY (alias, file_id)
);
CREATE INDEX IF NOT EXISTS table_names_file ON table_names(file_id);
```

- [ ] **Step 5: Implement the vocabulary and the two phases** in `index.rs`. Add after `MAX_FILE_BYTES`:

```rust
/// Alias → table name (schema spec §3): every table name, plus a Prisma model's name and
/// its lower-camel form. Rebuilt from `table_names` after the schema files of a run.
pub type Vocab = HashMap<String, String>;

pub fn load_vocab(conn: &Connection) -> Result<Vocab> {
    let mut stmt = conn.prepare("SELECT alias, table_name FROM table_names ORDER BY alias, table_name")?;
    let mut v = Vocab::new();
    for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (alias, table) = row?;
        v.entry(alias).or_insert(table);
    }
    Ok(v)
}

/// A stable digest of the vocabulary, kept in `meta` as `table_vocab`; a run whose
/// vocabulary differs from the stored one re-indexes every code file once.
pub fn vocab_hash(v: &Vocab) -> String {
    let mut pairs: Vec<(&String, &String)> = v.iter().collect();
    pairs.sort();
    let mut h = blake3::Hasher::new();
    for (a, t) in pairs {
        h.update(a.as_bytes());
        h.update(b"\t");
        h.update(t.as_bytes());
        h.update(b"\n");
    }
    h.finalize().to_hex()[..12].to_string()
}

fn is_schema_path(p: &str) -> bool {
    Language::from_path(p).is_some_and(|l| l.is_schema())
}

fn is_code_path(p: &str) -> bool {
    Language::from_path(p).is_some_and(|l| !l.is_schema() && !l.is_document())
}
```
Replace the `for e in &w.entries { ... }` loop in `refresh_with` with:

```rust
        // Schema files first (spec §3): the vocabulary the code pass needs comes from them.
        // Then everything else; when the vocabulary differs from the previous run's, every
        // code file is re-indexed once, content hash or not.
        let (schema, rest): (Vec<&WalkEntry>, Vec<&WalkEntry>) =
            w.entries.iter().partition(|e| is_schema_path(&e.rel_path));
        let mut seen: Vec<&str> = Vec::with_capacity(w.entries.len());
        let empty = Vocab::new();
        for e in schema {
            seen.push(&e.rel_path);
            self.process(e, &known, &empty, false, deadline, now, &mut stats)?;
            on_file();
        }
        let vocab = load_vocab(self.store.conn())?;
        let hash = vocab_hash(&vocab);
        let force = self.store.get_meta("table_vocab")?.as_deref() != Some(hash.as_str());
        for e in rest {
            seen.push(&e.rel_path);
            let force_this = force && is_code_path(&e.rel_path);
            self.process(e, &known, &vocab, force_this, deadline, now, &mut stats)?;
            on_file();
        }
        if stats.remaining == 0 {
            self.store.set_meta("table_vocab", &hash)?;
        }
```
and add the method (the body is the old loop body, moved):

```rust
    #[allow(clippy::too_many_arguments)]
    fn process(
        &self,
        e: &WalkEntry,
        known: &HashMap<String, Known>,
        vocab: &Vocab,
        force: bool,
        deadline: Option<Instant>,
        now: i64,
        stats: &mut IndexStats,
    ) -> Result<()> {
        if !force && !Self::changed(known, e) {
            stats.unchanged += 1;
            return Ok(());
        }
        if deadline.is_some_and(|d| Instant::now() >= d) {
            stats.remaining += 1;
            return Ok(());
        }
        let prior = known.get(&e.rel_path);
        // One unreadable or unparseable file must never abort the refresh: it becomes
        // a skip row like any other skipped file, and the walk carries on. Only store
        // errors (which mean the index itself is unusable) propagate.
        match self.index_file(e, prior, now, vocab, force) {
            Ok(Outcome::Indexed) => stats.indexed += 1,
            Ok(Outcome::Unchanged) => stats.unchanged += 1,
            Ok(Outcome::Skipped) => stats.skipped += 1,
            Err(Error::Io(_)) => {
                self.upsert_skipped(&e.rel_path, e.mtime_ms, e.size, "unreadable", now)?;
                stats.skipped += 1;
            }
            Err(Error::Tags(_)) => {
                self.upsert_skipped(&e.rel_path, e.mtime_ms, e.size, "parse-error", now)?;
                stats.skipped += 1;
            }
            Err(other) => return Err(other),
        }
        Ok(())
    }
```
In `index_file`, change the signature to `fn index_file(&self, e: &WalkEntry, prior: Option<&Known>, now: i64, vocab: &Vocab, force: bool) -> Result<Outcome>`, guard the content-hash short-circuit with `if !force && prior.and_then(|p| p.content_hash.as_deref()) == Some(hash.as_str())`, and replace the extraction branch with:

```rust
        let (tags, bodies, mentions, aliases) = if lang.is_document() {
            let d = crate::doc::extract(lang, source, &stem)?;
            (d.tags, d.bodies, d.mentions, Vec::new())
        } else if lang.is_schema() {
            let s = crate::schema::extract(lang, source)?;
            (s.tags, Vec::new(), Vec::new(), s.aliases)
        } else {
            let _ = vocab; // Task 4 turns the vocabulary into table mentions here
            (extract_tags(lang, source)?, Vec::new(), Vec::new(), Vec::new())
        };
```
After `delete_symbols_for(&tx, file_id)?;` add:

```rust
        for (alias, table) in &aliases {
            tx.execute(
                "INSERT OR REPLACE INTO table_names(alias, table_name, file_id) VALUES (?1, ?2, ?3)",
                params![alias, table, file_id],
            )?;
        }
```
In `delete_symbols_for` add `conn.execute("DELETE FROM table_names WHERE file_id = ?1", [file_id])?;` before the `symbols` delete. Where `refresh_with` previously declared `seen` before the loop, remove the old declaration (it moves into the block above).

- [ ] **Step 6: Run the tests**: `cargo test -p singularrag-core index::` — Expected: the four new tests PASS and the existing ones still PASS. Note `a_vocabulary_change_reindexes_code_files_once` expects `third.indexed == 5`: the schema file and four code files; the Markdown file and the SQL files are not code (`is_code_path`) and stay unchanged.

- [ ] **Step 7: Gate and commit**

```bash
cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add -A crates/singularrag-core/src/store/schema.rs crates/singularrag-core/src/index.rs crates/singularrag-core/src/fixture.rs
git commit -m "feat(core): schema files indexed first; table_names vocabulary; code re-indexed once when it changes; schema fixture

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 4: Code mentions of tables

**Files:**
- Modify: `crates/singularrag-core/src/schema.rs` (`table_mentions`)
- Modify: `crates/singularrag-core/src/index.rs` (call it in `index_file`)
- Test: `schema.rs` and `index.rs` tests

**Interfaces:**
- Consumes: `Vocab` (Task 3).
- Produces: `pub fn table_mentions(lang: Language, source: &str, vocab: &HashMap<String, String>) -> Result<Vec<(String, u32)>>` — `(table name, 1-based line)`, deduplicated, line order.

- [ ] **Step 1: Write the failing tests** in `schema.rs`'s `mod tests`:

```rust
    fn vocab() -> HashMap<String, String> {
        [("website", "website"), ("Website", "website"), ("websiteEvent", "website_event"), ("website_event", "website_event"), ("session", "session"), ("user", "user")]
            .into_iter()
            .map(|(a, t)| (a.to_string(), t.to_string()))
            .collect()
    }

    #[test]
    fn properties_and_strings_reference_tables_but_plain_identifiers_do_not() {
        let src = "import { Website } from './types';\nconst session = create();\nexport function f(websiteId: string) {\n  prisma.client.websiteEvent.findMany();\n  return db.raw(`select 1 from website_event where id = ${websiteId}`);\n}\nconst s = \"session\";\nconst u = user;\n";
        let m = table_mentions(Language::TypeScript, src, &vocab()).unwrap();
        assert_eq!(
            m,
            vec![
                ("website_event".to_string(), 4),
                ("website_event".to_string(), 5),
                ("session".to_string(), 7),
            ]
        );
    }

    #[test]
    fn rust_field_names_and_string_contents_count_too() {
        let src = "fn q(conn: &Conn) {\n    let user = 1;\n    conn.execute(\"INSERT INTO session(id) VALUES (1)\", []);\n    let n = row.website;\n}\n";
        let m = table_mentions(Language::Rust, src, &vocab()).unwrap();
        assert_eq!(m, vec![("session".to_string(), 3), ("website".to_string(), 4)]);
    }

    #[test]
    fn documents_and_schema_files_get_no_table_mentions_and_an_empty_vocabulary_yields_nothing() {
        assert!(table_mentions(Language::Markdown, "`website`", &vocab()).unwrap().is_empty());
        assert!(table_mentions(Language::Sql, "select * from website", &vocab()).unwrap().is_empty());
        assert!(table_mentions(Language::TypeScript, "x.website", &HashMap::new()).unwrap().is_empty());
    }
```
And in `index.rs` tests:

```rust
    #[test]
    fn code_references_tables_through_properties_and_strings_only() {
        let dir = tempfile::tempdir().unwrap();
        write_schema_mini(dir.path());
        let store = index_dir(dir.path());
        refresh(&store, dir.path());
        assert!(refs_named(&store, "website").contains(&"src/queries/website.ts".to_string()), "prisma.client.website");
        assert!(refs_named(&store, "website_event").contains(&"src/queries/website.ts".to_string()), "from website_event in a template string");
        assert!(!refs_named(&store, "user").contains(&"src/queries/website.ts".to_string()));
        // `const session = ...` is a plain identifier, and there is no `session` table anyway;
        // the negative case that matters is `user`, which is a table and appears nowhere as a property or string
        let from_ts: i64 = store
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM refs r JOIN files f ON f.id = r.file_id WHERE f.path = 'src/queries/website.ts' AND r.name IN ('user', 'session')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(from_ts, 0);
    }
```

- [ ] **Step 2: Run them**: `cargo test -p singularrag-core table_mentions` and `cargo test -p singularrag-core code_references_tables` — Expected: FAIL to compile (`table_mentions` missing), then the index test fails on the `website` assertion.

- [ ] **Step 3: Implement `table_mentions`** in `schema.rs` and remove the `#[allow(unused_imports)]` from Task 1:

```rust
// ---- Code mentions of tables (spec §3) ----

/// Where code names a table: a property identifier after a dot (`prisma.client.website`)
/// or a token inside a string or template literal (`from website_event`). Plain
/// identifiers never count. Returns `(table name, 1-based line)`, deduplicated, in order.
pub fn table_mentions(
    lang: Language,
    source: &str,
    vocab: &HashMap<String, String>,
) -> Result<Vec<(String, u32)>> {
    if vocab.is_empty() {
        return Ok(Vec::new());
    }
    let (language, props, strings): (tree_sitter::Language, &[&str], &[&str]) = match lang {
        Language::TypeScript => (tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(), &["property_identifier"], &["string_fragment"]),
        Language::Tsx => (tree_sitter_typescript::LANGUAGE_TSX.into(), &["property_identifier"], &["string_fragment"]),
        Language::JavaScript => (tree_sitter_javascript::LANGUAGE.into(), &["property_identifier"], &["string_fragment"]),
        Language::Rust => (tree_sitter_rust::LANGUAGE.into(), &["field_identifier"], &["string_content"]),
        _ => return Ok(Vec::new()),
    };
    let mut parser = Parser::new();
    parser
        .set_language(&language)
        .map_err(|e| Error::Tags(format!("{e:?}")))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| Error::Tags("parse returned nothing".into()))?;
    let mut out: Vec<(String, u32)> = Vec::new();
    let mut seen: HashSet<(String, u32)> = HashSet::new();
    let mut cursor = tree.walk();
    let mut push = |table: &str, line: u32, out: &mut Vec<(String, u32)>| {
        if seen.insert((table.to_string(), line)) {
            out.push((table.to_string(), line));
        }
    };
    // Pre-order walk without recursion.
    loop {
        let node = cursor.node();
        let kind = node.kind();
        let line = node.start_position().row as u32 + 1;
        if props.contains(&kind) {
            if let Ok(text) = node.utf8_text(source.as_bytes()) {
                if let Some(table) = vocab.get(text) {
                    push(table, line, &mut out);
                }
            }
        } else if strings.contains(&kind) {
            if let Ok(text) = node.utf8_text(source.as_bytes()) {
                for (i, tok) in text.split(|c: char| !c.is_alphanumeric() && c != '_').enumerate() {
                    if tok.is_empty() {
                        continue;
                    }
                    if let Some(table) = vocab.get(tok) {
                        // a multi-line template: the token's own line
                        let offset = text.split(|c: char| !c.is_alphanumeric() && c != '_').take(i).map(|s| s.len() + 1).sum::<usize>();
                        let extra = text.as_bytes()[..offset.min(text.len())].iter().filter(|&&b| b == b'\n').count() as u32;
                        push(table, line + extra, &mut out);
                    }
                }
            }
        }
        if cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                out.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
                return Ok(out);
            }
        }
    }
}
```
In `index.rs` `index_file`, replace the `let _ = vocab;` branch with:

```rust
            let mentions = if vocab.is_empty() {
                Vec::new()
            } else {
                crate::schema::table_mentions(lang, source, vocab)?
            };
            (extract_tags(lang, source)?, Vec::new(), mentions, Vec::new())
```
(`mentions` are written to `refs` by the existing loop at the end of `index_file`.)

- [ ] **Step 4: Run the tests**: `cargo test -p singularrag-core schema::` and `cargo test -p singularrag-core index::` — Expected: PASS. If the TypeScript test reports an extra mention at line 1 (`import { Website }`), the grammar emitted `identifier`, not `property_identifier`, and the walk is right; if it reports a missing line-5 mention, check that `template_string` children are `string_fragment` in the pinned `tree-sitter-javascript` by printing `node.kind()` in the test once, then remove the print.

- [ ] **Step 5: Gate and commit**

```bash
cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add -A crates/singularrag-core/src/schema.rs crates/singularrag-core/src/index.rs
git commit -m "feat(core): code references a table where it names it as a property or inside a string

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 5: The tree payload's `refs` for columns

**Files:**
- Modify: `crates/singularrag/src/serve/queries.rs` (`TreeSymbol`, `tree`)
- Modify: `ui/src/api/types.ts` (`TreeSymbol.refs?`)
- Test: `crates/singularrag/tests/serve.rs`

**Interfaces:**
- Produces: `TreeSymbol.refs: Vec<String>` serialised only when non-empty, filled for `column` symbols with the distinct ref names on the column's lines (its foreign-key targets).

- [ ] **Step 1: Write the failing serve test** in `tests/serve.rs` (copy the `spawn`/`get` usage of the test around line 296):

```rust
#[tokio::test]
async fn the_tree_carries_a_columns_foreign_key_targets_and_nothing_for_tables() {
    let dir = tempfile::tempdir().unwrap();
    singularrag_core::fixture::write_schema_mini(dir.path());
    let s = spawn(dir.path());
    let tree: serde_json::Value = get(&s, "/tree").await.json().await.unwrap();
    let schema = tree
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"] == "prisma/schema.prisma")
        .expect("schema file in the tree");
    assert_eq!(schema["lang"], "prisma");
    let syms = schema["symbols"].as_array().unwrap();
    let col = syms.iter().find(|s| s["name"] == "website_event.website_id").unwrap();
    assert_eq!(col["kind"], "column");
    assert_eq!(col["refs"], serde_json::json!(["website"]));
    let plain = syms.iter().find(|s| s["name"] == "website_event.url_path").unwrap();
    assert!(plain.get("refs").is_none(), "no key when there is nothing to show: {plain}");
    let table = syms.iter().find(|s| s["name"] == "website_event").unwrap();
    assert_eq!(table["kind"], "table");
    assert!(table.get("refs").is_none());
}
```

- [ ] **Step 2: Run it**: `cargo test -p singularrag --test serve the_tree_carries` — Expected: FAIL (`refs` missing).

- [ ] **Step 3: Implement.** In `queries.rs`:

```rust
#[derive(Debug, Serialize)]
pub struct TreeSymbol {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub line_start: i64,
    pub line_end: i64,
    pub signature: String,
    /// For `column` symbols: the tables its foreign keys reference (the distinct ref names
    /// on the column's lines). Absent otherwise (schema spec §4).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<String>,
}
```
In `tree`, prepare a third statement and fill `refs`:

```rust
    let mut refs = conn.prepare(
        "SELECT DISTINCT name FROM refs WHERE file_id = ?1 AND line BETWEEN ?2 AND ?3 ORDER BY name",
    )?;
    ...
        let mut symbols = syms
            .query_map([fid], |r| {
                Ok(TreeSymbol {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    kind: r.get(2)?,
                    line_start: r.get(3)?,
                    line_end: r.get(4)?,
                    signature: r.get(5)?,
                    refs: Vec::new(),
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        for s in symbols.iter_mut().filter(|s| s.kind == "column") {
            s.refs = refs
                .query_map(params![fid, s.line_start, s.line_end], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
        }
```
Every other constructor of `TreeSymbol` in the crate (grep `TreeSymbol {`) gets `refs: Vec::new()`; add `params` to the file's `use rusqlite::{...}` line if it is not already imported. In `ui/src/api/types.ts`: `export type TreeSymbol = { id: number; name: string; kind: string; line_start: number; line_end: number; signature: string; refs?: string[] };`.

- [ ] **Step 4: Run**: `cargo test -p singularrag --test serve` — Expected: PASS.

- [ ] **Step 5: Gate and commit**

```bash
cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
cd ui && bun run typecheck && cd ..
git add -A crates/singularrag/src/serve/queries.rs crates/singularrag/tests/serve.rs ui/src/api/types.ts
git commit -m "feat(serve): column symbols carry their foreign-key targets in the tree payload

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 6: The UI: Columns in the detail panel, the schema language group

**Files:**
- Modify: `ui/src/lib/langGroup.ts`, `ui/src/lib/langGroup.test.ts`
- Modify: `ui/src/lib/mapStyle.ts`, `ui/src/index.css`
- Modify: `ui/src/components/DetailPanel.tsx`
- Modify: `ui/src/App.tsx` (`focusSection` fallback)
- Create: `ui/src/components/DetailPanel.test.tsx`
- Modify: `ui/src/App.test.tsx` (one test)

**Interfaces:**
- Consumes: `TreeSymbol.refs?` (Task 5), `FocusSection` from `EntityView.tsx`, `GridRow` from `RepoTree.tsx`.

- [ ] **Step 1: Write the failing tests.** In `langGroup.test.ts` add inside the `describe`:

```ts
  test("schema files form their own group", () => {
    expect(langGroup("prisma")).toBe("schema");
    expect(langGroup("sql")).toBe("schema");
  });
```
Create `ui/src/components/DetailPanel.test.tsx`:

```tsx
import { afterEach, expect, mock, test } from "bun:test";
import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import type { MapConfig, TreeSymbol } from "../api/types";
import type { FileRow, SymbolRow } from "../lib/join";
import { DetailPanel } from "./DetailPanel";

afterEach(cleanup);

const map: MapConfig = { pin: [], exclude: [], note: [], boundary: [], deny: { extra_patterns: [] } };
const sym = (id: number, name: string, kind: string, line: number, signature: string, refs?: string[]): SymbolRow => ({
  key: `prisma/schema.prisma::${name}`, status: "untouched", item: null, moved: false,
  symbol: { id, name, kind, line_start: line, line_end: line, signature, ...(refs ? { refs } : {}) } as TreeSymbol,
});
const file: FileRow = {
  path: "prisma/schema.prisma", lang: "prisma", served: 0, cut: 0, expandedByDefault: false,
  symbols: [
    sym(4, "user", "table", 1, "table user (1 columns)"),
    sym(1, "website", "table", 10, "table website (2 columns, model Website)"),
    sym(2, "website.website_id", "column", 11, "String @id @map(\"website_id\") @db.Uuid"),
    sym(3, "website.user_id", "column", 12, "String? @map(\"user_id\") @db.Uuid", ["user"]),
  ],
};
const noop = () => {};

function renderPanel(row: SymbolRow) {
  const onFocusSection = mock((_path: string, _name: string, _id?: number) => {});
  const r = render(
    <DetailPanel row={{ kind: "symbol", path: file.path, file, symbol: row }} map={map} entities={null}
      onFocusEntity={noop} onFocusSection={onFocusSection} onPin={noop} onExclude={noop} onNote={noop} onRemoveAgentNote={noop}
      blast={null} blastLoading={false} onToggleBlast={noop} expandedPath={null} onToggleExpand={noop}
      onAddBoundary={noop} onRemoveBoundary={noop} />,
  );
  return { ...r, onFocusSection };
}

test("a table lists its columns with a button to each foreign key's table", async () => {
  const { container, onFocusSection } = renderPanel(file.symbols[1]);
  const list = screen.getByRole("list", { name: "Columns" });
  const items = within(list).getAllByRole("listitem");
  expect(items.map((li) => li.querySelector("code")?.textContent)).toEqual(["website_id", "user_id"]);
  expect(items[1].textContent).toContain("String? @map(\"user_id\") @db.Uuid");
  await userEvent.click(within(list).getByRole("button", { name: "References user" }));
  expect(onFocusSection).toHaveBeenCalledWith("prisma/schema.prisma", "user");
  expect((await axe.run(container)).violations).toEqual([]);
});

test("a column links back to its table and to the table it references", async () => {
  const { container, onFocusSection } = renderPanel(file.symbols[3]);
  await userEvent.click(screen.getByRole("button", { name: "Table website" }));
  expect(onFocusSection).toHaveBeenCalledWith("prisma/schema.prisma", "website");
  expect(screen.getByRole("button", { name: "References user" })).toBeTruthy();
  expect(screen.queryByRole("list", { name: "Columns" })).toBeNull();
  expect((await axe.run(container)).violations).toEqual([]);
});

test("a table with no columns says so", () => {
  renderPanel(file.symbols[0]);
  expect(screen.getByRole("heading", { name: "Columns" })).toBeTruthy();
  expect(screen.getByText("No columns.")).toBeTruthy();
});

test("a function shows neither section", () => {
  renderPanel(sym(9, "getWebsite", "function", 3, "export function getWebsite(websiteId: string)"));
  expect(screen.queryByRole("heading", { name: "Columns" })).toBeNull();
  expect(screen.queryByRole("button", { name: /^Table / })).toBeNull();
});
```
In `App.test.tsx`, find the test that clicks a "Go to section" button and lands on a row (search `Go to section`); add a sibling test named `a_foreign_key_button_reaches_a_table_defined_in_another_file` that mocks `/api/tree` with two files, `prisma/schema.prisma` (a `column` symbol `report.website_id` with `refs: ["website"]`, no `website` table) and `prisma/migrations/01_init/migration.sql` (a `table` symbol `website`), focuses the column row, clicks `References website`, and asserts the migration file's `website` row is the focused tree row (the same assertion the "Go to section" test makes about its target row). Use that test's mocking helpers verbatim; the only new data is the two files.

- [ ] **Step 2: Run them**: `cd ui && bun test` — Expected: the new tests FAIL (`schema` group missing, no Columns list, no fallback).

- [ ] **Step 3: Implement.** `langGroup.ts`:

```ts
export type LangGroup = "code" | "docs" | "config" | "styles" | "schema";
const GROUPS: Record<string, LangGroup> = { markdown: "docs", html: "docs", text: "docs", json: "config", yaml: "config", toml: "config", css: "styles", prisma: "schema", sql: "schema" };
export const langGroup = (lang: string | null): LangGroup => (lang ? GROUPS[lang] : undefined) ?? "code";
```
`mapStyle.ts`: add `schema: string;` to `Palette` after `styles`, `schema: "#0369a1"` to `DEFAULTS` after `styles`, and `schema: v("--map-schema", DEFAULTS.schema),` after the `styles` entry of the palette reader. `index.css`: add `--map-schema: #0369a1;` to the light block's `--map-docs` line and `--map-schema: #38bdf8;` to the dark block's.

`DetailPanel.tsx`: change the `RepoTree` import to `import type { GridRow, TreeRow } from "./RepoTree";`, add after the `SymbolKnowledge` block:

```tsx
      {row.kind === "symbol" && row.symbol.symbol.kind === "table" && <TableColumns row={row} onFocusSection={onFocusSection} />}
      {row.kind === "symbol" && row.symbol.symbol.kind === "column" && <ColumnParent row={row} onFocusSection={onFocusSection} />}
```
and the two components at the bottom of the file:

```tsx
type SymbolGridRow = Extract<GridRow, { kind: "symbol" }>;

/** A table's columns (the file's `table.column` symbols) with their types; a column whose
 *  line carries a foreign key gets a button to the referenced table (schema spec §4). */
function TableColumns({ row, onFocusSection }: { row: SymbolGridRow; onFocusSection: FocusSection }) {
  const table = row.symbol.symbol.name;
  const columns = row.file.symbols.filter((s) => s.symbol.kind === "column" && s.symbol.name.startsWith(`${table}.`));
  return (
    <div>
      <h3 id="columns-heading" className="text-sm font-semibold">Columns</h3>
      {columns.length === 0 ? <p className="text-sm text-muted-foreground">No columns.</p> : (
        <ul aria-labelledby="columns-heading" className="list-disc pl-5 text-sm">
          {columns.map((c) => {
            const where = `column-${c.symbol.id}-where`;
            return (
              <li key={c.symbol.id}>
                <code>{c.symbol.name.slice(table.length + 1)}</code> <span className="text-muted-foreground">{c.symbol.signature}</span>
                {(c.symbol.refs ?? []).map((t) => (
                  <Button key={t} type="button" variant="outline" size="sm" className="ml-2" aria-describedby={where} onClick={() => onFocusSection(row.path, t)}>
                    References {t}
                  </Button>
                ))}
                <span id={where} className="sr-only">{row.path} line {c.symbol.line_start}</span>
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}

/** A column's table, and the table its foreign key references. */
function ColumnParent({ row, onFocusSection }: { row: SymbolGridRow; onFocusSection: FocusSection }) {
  const name = row.symbol.symbol.name;
  const table = name.slice(0, name.lastIndexOf("."));
  const where = `column-${row.symbol.symbol.id}-table`;
  if (!table) return null;
  return (
    <div className="flex flex-wrap items-center gap-2 text-sm">
      <Button type="button" variant="outline" size="sm" aria-describedby={where} onClick={() => onFocusSection(row.path, table)}>Table {table}</Button>
      {(row.symbol.symbol.refs ?? []).map((t) => (
        <Button key={t} type="button" variant="outline" size="sm" aria-describedby={where} onClick={() => onFocusSection(row.path, t)}>References {t}</Button>
      ))}
      <span id={where} className="sr-only">{row.path} line {row.symbol.symbol.line_start}</span>
    </div>
  );
}
```
`App.tsx` `focusSection` (line ~260):

```ts
  const focusSection = (path: string, name: string, symbolId?: number) => {
    let file = filteredRows.find((r) => r.path === path);
    let sym = file && (file.symbols.find((s) => s.symbol.id === symbolId) ?? file.symbols.find((s) => s.symbol.name === name));
    if (!sym && symbolId === undefined) {
      // A foreign key names a table that may be defined in another file (schema spec §4).
      file = filteredRows.find((r) => r.symbols.some((s) => s.symbol.kind === "table" && s.symbol.name === name));
      sym = file?.symbols.find((s) => s.symbol.kind === "table" && s.symbol.name === name);
    }
    if (!file || !sym) { toast.error(`${path} :: ${name} is not in the tree`); return; }
    if (view !== "tree") changeView("tree");
    setReveal({ path: file.path, key: sym.key, seq: ++revealSeq.current });
  };
```

- [ ] **Step 4: Run**: `cd ui && bun run typecheck && bun test && bun run build`, then `cargo test --workspace` from the worktree root (embedded assets). Expected: all PASS, including the existing `App.test.tsx` and `RepoTree.test.tsx`.

- [ ] **Step 5: Gate and commit**

```bash
cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add -A ui/src/lib/langGroup.ts ui/src/lib/langGroup.test.ts ui/src/lib/mapStyle.ts ui/src/index.css ui/src/components/DetailPanel.tsx ui/src/components/DetailPanel.test.tsx ui/src/App.tsx ui/src/App.test.tsx
git commit -m "feat(ui): a table's columns and foreign keys in the detail panel; schema files get their own map group

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 7: Walks: `implemented_by`, the CI schema walks, the extraction fixture

**Files:**
- Modify: `crates/singularrag-core/src/walks.rs` (`Walk.implemented_by`, validation)
- Modify: `crates/singularrag/src/walk.rs` (`grade_process`)
- Modify: `crates/singularrag-core/fixtures/prose/extraction.json` (one new section)
- Modify: `eval/walks-ci.toml` (four walks), create `eval/patches/ci-schema.patch`
- Modify: `crates/singularrag/tests/cli.rs` (`walk_runs_all_four_kinds_on_the_fixtures_without_a_model`)
- Test: `walks.rs`, `walk.rs`, `cli.rs`

**Interfaces:**
- Consumes: `StepDto.code: Vec<CodeLinkDto { path, name, .. }>` (existing), `fixture::write_schema_mini` (Task 3).
- Produces: `Walk.implemented_by: Option<String>` (process only; `path::name`).

- [ ] **Step 1: Write the failing tests.** In `walks.rs` tests (find the existing validation tests and add):

```rust
    #[test]
    fn implemented_by_belongs_to_process_walks_and_is_path_name() {
        let ok: WalkFile = toml::from_str("[[walk]]\nid = \"P\"\nkind = \"process\"\nquery = \"q\"\nimplemented_by = \"prisma/schema.prisma::website_event\"\ngold = [\"docs/a.md::S\"]\n").unwrap();
        validate(&ok).unwrap();
        let wrong_kind: WalkFile = toml::from_str("[[walk]]\nid = \"L\"\nkind = \"locate\"\nquery = \"q\"\nimplemented_by = \"a.ts::f\"\ngold = [\"docs/a.md::S\"]\n").unwrap();
        assert!(validate(&wrong_kind).unwrap_err().to_string().contains("implemented_by belongs to process walks only"));
        let bare: WalkFile = toml::from_str("[[walk]]\nid = \"P\"\nkind = \"process\"\nquery = \"q\"\nimplemented_by = \"website_event\"\ngold = [\"docs/a.md::S\"]\n").unwrap();
        assert!(validate(&bare).unwrap_err().to_string().contains("is not path::name"));
    }
```
In `walk.rs` tests (add `CodeLinkDto` to the `use crate::serve::queries::{...}` line):

```rust
    #[test]
    fn a_process_walk_can_require_a_step_to_be_implemented_by_a_symbol() {
        let mut d = dto(vec![(
            "Recording a visit",
            vec![
                ("The tracker posts to the collector", "", "docs/data-model.md::Recording a visit"),
                ("The collector writes one row to website_event", "", "docs/data-model.md::Recording a visit"),
            ],
        )]);
        d.processes[0].steps[1].code.push(CodeLinkDto {
            symbol_id: 5,
            path: "prisma/schema.prisma".into(),
            name: "website_event".into(),
            line: 30,
            via: serde_json::json!("mention"),
        });
        let mut w = walk(WalkKind::Process, &["docs/data-model.md::Recording a visit"]);
        w.step_text = Some("writes one row".into());
        w.implemented_by = Some("prisma/schema.prisma::website_event".into());
        assert!(grade_process(&w, &d).0);
        w.implemented_by = Some("prisma/schema.prisma::website".into());
        let (pass, _, reason) = grade_process(&w, &d);
        assert!(!pass);
        assert_eq!(reason.as_deref(), Some("prisma/schema.prisma::website not among the steps' implemented-by links"));
        // the text failing still reports the text, as before
        w.step_text = Some("rolls back".into());
        assert_eq!(grade_process(&w, &d).2.as_deref(), Some("step text \"rolls back\" not found"));
    }
```
Also add `implemented_by: None,` to the test helper `walk()` in `walk.rs` and to every other `Walk { .. }` literal in the workspace (grep `patch: None`).

- [ ] **Step 2: Run them**: `cargo test -p singularrag-core walks::` and `cargo test -p singularrag walk::tests` — Expected: FAIL to compile (no field `implemented_by`).

- [ ] **Step 3: Implement.** `walks.rs`: add to `Walk` after `step_text`:

```rust
    #[serde(default)]
    pub implemented_by: Option<String>, // process only: a path::name among a step's implemented-by links
```
In `validate`, extend the role/step_text rule and add the shape check:

```rust
        if (w.role.is_some() || w.step_text.is_some()) && w.kind != WalkKind::Process {
            ...unchanged...
        }
        if w.implemented_by.is_some() && w.kind != WalkKind::Process {
            return Err(Error::Config(format!(
                "{}: implemented_by belongs to process walks only",
                w.id
            )));
        }
        if let Some(key) = &w.implemented_by {
            if !key.contains("::") {
                return Err(Error::Config(format!(
                    "{}: implemented_by \"{key}\" is not path::name",
                    w.id
                )));
            }
        }
```
`walk.rs` `grade_process`: add after `text_ok`:

```rust
    let impl_ok = |p: &ProcessDto| {
        w.implemented_by.as_ref().is_none_or(|key| {
            p.steps
                .iter()
                .any(|s| s.code.iter().any(|c| format!("{}::{}", c.path, c.name) == *key))
        })
    };
    if citing.iter().any(|p| role_ok(p) && text_ok(p) && impl_ok(p)) {
        return (true, reached, None);
    }
    let reason = if w.role.is_some() && !citing.iter().any(|p| role_ok(p)) {
        format!("step with role {} not found", w.role.as_deref().unwrap_or_default())
    } else if w.implemented_by.is_some() && citing.iter().any(|p| role_ok(p) && text_ok(p)) {
        format!(
            "{} not among the steps' implemented-by links",
            w.implemented_by.as_deref().unwrap_or_default()
        )
    } else {
        format!(
            "step text \"{}\" not found",
            w.step_text.as_deref().unwrap_or_default()
        )
    };
    (false, reached, Some(reason))
```
(replacing the old `if citing.iter().any(|p| role_ok(p) && text_ok(p))` block and the old `reason` match).

- [ ] **Step 4: Run**: both test commands from Step 2 — Expected: PASS, including the existing process-grader tests.

- [ ] **Step 5: The extraction fixture.** Add to `crates/singularrag-core/fixtures/prose/extraction.json` a new top-level key (keep the file valid JSON; the existing keys use `source`/`target` for relations and `steps.process`/`steps.steps[].{text,role,systems}`):

```json
  "docs/data-model.md::Recording a visit": {
    "entities": [
      {"name": "Recording a visit", "type": "process", "description": "How a page view becomes a row in the website_event table."},
      {"name": "Collector", "type": "system", "description": "The endpoint the tracker script posts to."},
      {"name": "website_event", "type": "system", "description": "The table that stores page views and events."}
    ],
    "relations": [
      {"source": "Collector", "target": "website_event", "description": "writes one row per page view to"}
    ],
    "steps": {
      "process": "Recording a visit",
      "steps": [
        {"text": "The tracker script posts to the collect endpoint when a visitor opens a page.", "role": "", "systems": ["Collector"]},
        {"text": "The collector looks up the website row by its tracking id and writes one row to the website_event table with the URL path and the event name.", "role": "", "systems": ["website_event"]},
        {"text": "Reports read website_event through getWebsiteDateRange.", "role": "", "systems": ["website_event"]}
      ]
    }
  }
```
Run `cargo test -p singularrag-core knowledge::` afterwards: the tests that load this file count sections or entities; if one asserts an exact count, raise it by one and say so in the report.

- [ ] **Step 6: The CI walks.** Append to `eval/walks-ci.toml` (write the path as `e*/walks-ci.toml` in commands):

```toml

# The schema fixture (`fixture::write_schema_mini`): tables and columns as symbols (schema spec §5).

[[walk]]
id = "C6"
kind = "locate"
query = "which table holds each event a website records"
gold = ["prisma/schema.prisma::website_event", "prisma/migrations/01_init/migration.sql::website_event"]

[[walk]]
id = "C7"
kind = "process"
query = "what happens when a visitor opens a page"
step_text = "writes one row"
implemented_by = "prisma/schema.prisma::website_event"
gold = ["docs/data-model.md::Recording a visit"]

[[walk]]
id = "C8"
kind = "connection"
from = "prisma/schema.prisma::website"
hops = 1
gold = ["src/queries/website.ts::getWebsite"]

[[walk]]
id = "C9"
kind = "change"
patch = "patches/ci-schema.patch"
gold = ["prisma/schema.prisma::website"]
```
Create `eval/patches/ci-schema.patch` (the fixture's `model Website {` is line 18; `domain` is line 21):

```
diff --git a/prisma/schema.prisma b/prisma/schema.prisma
--- a/prisma/schema.prisma
+++ b/prisma/schema.prisma
@@ -19,6 +19,7 @@ model Website {
   id        String         @id @map("website_id") @db.Uuid
   name      String         @db.VarChar(100)
   domain    String?        @db.VarChar(500)
+  notes     String?        @db.VarChar(500)
   userId    String?        @map("user_id") @db.Uuid
   user      User?          @relation(fields: [userId], references: [id])
   events    WebsiteEvent[]
```
Update the header comment of `walks-ci.toml` to say "plus the schema fixture (`fixture::write_schema_mini`)" and "two for connection, ... nine walks".

- [ ] **Step 7: Update the CLI test** `walk_runs_all_four_kinds_on_the_fixtures_without_a_model` in `tests/cli.rs`: add `singularrag_core::fixture::write_schema_mini(dir.path());` after `write_prose`, and change the assertions to `walks 2/2 locate`, `walks 2/2 process`, `walks 3/3 connection`, `walks 2/2 change`, `walks 9/9\n`. Add, before the footer assertions, a check that the process walk reached its links and not by accident:

```rust
    assert!(!stdout.contains("not among the steps' implemented-by links"), "{stdout}");
```

- [ ] **Step 8: Run**: `cargo test -p singularrag --test cli walk_` — Expected: PASS. If C7 fails with the implemented-by reason, print the `--json` output of the walk and check the step's `code` links: the section's code-span mentions are `website_event` (twice) and `getWebsiteDateRange`, sorted by path then name, which puts `prisma/migrations/01_init/migration.sql::website_event`, `prisma/schema.prisma::website_event` and `src/queries/website.ts::getWebsiteDateRange` inside `LINK_LIMIT` (3). If C6 fails, raise the test's `--budget` is not an option (it is 4096 already); check that the `symbols_fts` row for the table carries `name_tokens` `website event`. If C9 fails with `patch does not apply`, re-count the fixture's lines against the hunk header.

- [ ] **Step 9: Gate and commit**

```bash
cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add -A crates/singularrag-core/src/walks.rs crates/singularrag/src/walk.rs crates/singularrag-core/fixtures/prose/extraction.json e*/walks-ci.toml e*/patches/ci-schema.patch crates/singularrag/tests/cli.rs
git commit -m "feat(walk): process walks can require an implemented-by link; schema walks on the CI fixture

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 8: umami: the pinned corpus, its walk file, the baseline and the record

**Files:**
- Create: `eval/walks-umami.toml`, `eval/patches/umami-{1,2,3,4}.patch`
- Modify: `eval/README.md` ("Walks" section: Recipe list, a new subsection), `docs/superpowers/specs/2026-09-19-singularrag-design.md` §5 (one line), `docs/superpowers/specs/2026-09-23-singularrag-documents-design.md` decomposition item 4 (one line)
- External: `~/Sites/umami-corpus` (a clone, never committed)

**Interfaces:**
- Consumes: everything above, through the built binary.

- [ ] **Step 1: Pin the corpus**

```bash
git clone -q https://github.com/umami-software/umami.git ~/Sites/umami-corpus
git -C ~/Sites/umami-corpus checkout -q ec0ff50388c264ed8ce46f00967e92f7e71476ae
echo ".singularrag/" >> ~/Sites/umami-corpus/.git/info/exclude
git -C ~/Sites/umami-corpus status --porcelain | wc -l   # Expected: 0
cargo build -p singularrag --release
./target/release/singularrag --repo ~/Sites/umami-corpus index
```
Expected: the index completes; `git -C ~/Sites/umami-corpus status --porcelain` is still empty. Then list what the index saw, to author the walks against real names:

```bash
sqlite3 ~/Sites/umami-corpus/.singularrag/index.db "SELECT f.path, s.name, s.kind, s.line_start FROM symbols s JOIN files f ON f.id = s.file_id WHERE s.kind = 'table' ORDER BY f.path, s.line_start"
sqlite3 ~/Sites/umami-corpus/.singularrag/index.db "SELECT f.path, s.name FROM symbols s JOIN files f ON f.id = s.file_id WHERE f.path IN ('src/queries/prisma/website.ts','src/queries/prisma/session.ts','src/queries/prisma/team.ts','src/queries/sql/getWebsiteDateRange.ts') ORDER BY f.path, s.line_start"
sqlite3 ~/Sites/umami-corpus/.singularrag/index.db "SELECT DISTINCT f.path FROM refs r JOIN files f ON f.id = r.file_id WHERE r.name = 'website' AND f.path LIKE 'src/%' ORDER BY f.path LIMIT 20"
sqlite3 ~/Sites/umami-corpus/.singularrag/index.db "SELECT COUNT(*) FROM table_names"
```
Expected: 26 Prisma tables in `prisma/schema.prisma` plus the migrations' and ClickHouse tables; the three query files with their function names; `src/queries/prisma/website.ts` among the `website` referrers.

- [ ] **Step 2: Write `eval/walks-umami.toml`** with twelve walks. The drafts below name the symbols umami's code is expected to have; **verify every `gold` and `from` against the Step 1 listings** and replace a name that does not exist with one that does in the same file, recording each substitution in your report. Header comment in the style of `walks-hono.toml`: corpus `~/Sites/umami-corpus` at `ec0ff50`, indexed, not extracted, `.singularrag/` excluded; run `singularrag --repo ~/Sites/umami-corpus walk --questions eval/walks-umami.toml --budget 4096`.

```toml
# locate: everyday words, not the table's own name

[[walk]]
id = "U1"
kind = "locate"
query = "which table stores the page views and events a tracked site sends in"
gold = ["prisma/schema.prisma::website_event", "db/clickhouse/schema.sql::website_event"]

[[walk]]
id = "U2"
kind = "locate"
query = "where are a visitor's browser, operating system, device and screen size kept"
gold = ["prisma/schema.prisma::session", "db/clickhouse/schema.sql::website_event"]

[[walk]]
id = "U3"
kind = "locate"
query = "which table says which people belong to a team and with what role"
gold = ["prisma/schema.prisma::team_user"]

[[walk]]
id = "U4"
kind = "locate"
query = "where do the custom properties attached to an event end up"
gold = ["prisma/schema.prisma::event_data", "db/clickhouse/schema.sql::event_data"]

# connection: from a table to the code that reads or writes it (symbol starts)

[[walk]]
id = "U5"
kind = "connection"
from = "prisma/schema.prisma::website"
hops = 1
gold = ["src/queries/prisma/website.ts::getWebsite"]

[[walk]]
id = "U6"
kind = "connection"
from = "prisma/schema.prisma::website_event"
hops = 1
gold = ["src/queries/sql/getWebsiteDateRange.ts::getWebsiteDateRange"]

[[walk]]
id = "U7"
kind = "connection"
from = "prisma/schema.prisma::session"
hops = 1
gold = ["src/queries/prisma/session.ts::getSession"]

[[walk]]
id = "U8"
kind = "connection"
from = "prisma/schema.prisma::team"
hops = 1
gold = ["src/queries/prisma/team.ts::getTeam"]

# change: a schema edit or a new migration; patches/umami-*.patch

[[walk]]
id = "U9"
kind = "change"
patch = "patches/umami-1.patch"
gold = ["prisma/schema.prisma::website"]

[[walk]]
id = "U10"
kind = "change"
patch = "patches/umami-2.patch"
gold = ["prisma/schema.prisma::session"]

[[walk]]
id = "U11"
kind = "change"
patch = "patches/umami-3.patch"
gold = ["prisma/migrations/28_add_website_notes/migration.sql::website.notes"]

[[walk]]
id = "U12"
kind = "change"
patch = "patches/umami-4.patch"
gold = ["db/clickhouse/schema.sql::website_event"]
```
Patches: `umami-1` adds a `notes String? @db.VarChar(500)` field inside `model Website` in `prisma/schema.prisma`; `umami-2` adds `notes String? @db.VarChar(100)` inside `model Session`; `umami-3` adds a new file `prisma/migrations/28_add_website_notes/migration.sql` containing `-- AlterTable\nALTER TABLE "website" ADD COLUMN "notes" VARCHAR(500);\n`; `umami-4` adds a column line `notes String,` inside `CREATE TABLE umami.website_event` in `db/clickhouse/schema.sql`. Generate each with `git -C ~/Sites/umami-corpus diff` (for the new file, `git add -N` first, then `git diff`), copy to `eval/patches/`, and `git -C ~/Sites/umami-corpus checkout -- . && git -C ~/Sites/umami-corpus clean -fd` so the corpus is clean again. Confirm `.gitignore` in the corpus does not ignore `prisma/migrations/` (the walk refuses to revert an ignored path).

- [ ] **Step 3: Run the baseline** from the worktree root (the binary from Step 1):

```bash
for b in 2048 4096; do
  ./target/release/singularrag --repo ~/Sites/umami-corpus walk --questions e*/walks-umami.toml --budget $b | tail -6
  ./target/release/singularrag --repo ~/Sites/umami-corpus walk --questions e*/walks-umami.toml --budget $b --no-models | tail -6
done
git -C ~/Sites/umami-corpus status --porcelain | wc -l   # Expected: 0 after every run
```
Record every row with its reason. The baseline may be poor (spec §5); a failing walk is a lead, not a thing to fix here. A `change` walk that fails with `patch does not apply` or leaves the tree dirty is a harness problem and must be fixed before recording. Also run the three existing walk files (`walks-repo.toml` on `../singularrag-pinned`, `walks-handbook.toml` on `../../handbook-corpus`, `walks-hono.toml` on `../hono`, absolute paths from the worktree) at 4096 seeds on, and compare with the "After the caps fix" table in the README: no walk that passed may fail now.

- [ ] **Step 4: Tier one must not move.** Run tier one on hono and the pinned docs corpus at 2048 and 4096 with `--no-models`, exactly as the README's "Tier one on journeys" recipe does, before (on `main`, in a scratch build: `git worktree add /tmp/sr-main main && cargo build --release --manifest-path /tmp/sr-main/Cargo.toml`) and after. Expected: identical numbers. Remove the scratch worktree afterwards (`git worktree remove /tmp/sr-main`).

- [ ] **Step 5: Record.** In `eval/README.md` "Walks": add umami to the Recipe list (`~/Sites/umami-corpus` at `ec0ff50`, indexed, not extracted; 4 locate, 4 connection, 4 change) and a subsection:

```markdown
### Schema sources: umami, 2026-10-16 (schema-v0)

`walks-umami.toml` on `umami-corpus` (26 Prisma models, 27 migrations, the ClickHouse DDL; schema spec):

| | 2048 seeds on | 2048 seeds off | 4096 seeds on | 4096 seeds off |
|---|---|---|---|---|
| `walks N/4 locate` | | | | |
| `walks N/4 connection` | | | | |
| `walks N/4 change` | | | | |
| overall | | | | |

Failing walks and why: <one line per failing id>.

No process walks: umami has ten Markdown files and no process documentation; Journeys over tables is judged on the CI fixture (`walks-ci.toml` C7, `implemented_by`). The three earlier walk files at 4096 seeds on: <same as the caps-fix table / list any change>. Tier one on hono and docs: identical before and after (no schema files there).
```
Fill in the numbers. In `docs/superpowers/specs/2026-09-19-singularrag-design.md` §5 change the source-types sentence to "code, text documents and database schemas (Prisma, SQL DDL)" with a pointer to this spec; in `docs/superpowers/specs/2026-09-23-singularrag-documents-design.md` item 4 append "(built as `2026-10-15-singularrag-schema-design.md`; Drizzle deferred; dumps as DDL only)". Add `ignore = ["umami-corpus"]`? No: the corpus lives outside the repository; nothing to ignore.

- [ ] **Step 6: Gate and commit**

```bash
cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add -A e*/walks-umami.toml e*/patches/umami-1.patch e*/patches/umami-2.patch e*/patches/umami-3.patch e*/patches/umami-4.patch e*/README.md docs/superpowers/specs/2026-09-19-singularrag-design.md docs/superpowers/specs/2026-09-23-singularrag-documents-design.md
git commit -m "docs(walks): umami walk file and baseline for schema sources; parent spec amendments

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

## Self-review

**Spec coverage.** §2 languages and both parsers: Tasks 1–2; caps and the secret check: unchanged code, exercised by Task 3's fixture (`db/dump.sql` is small; the cap is the existing `MAX_FILE_BYTES` path). §3 foreign keys: Tasks 1–2 (non-definition tags) and 3 (written as refs); code mentions and the noise rule: Task 4; vocabulary, order, forced re-index once, deadline rule: Task 3; documents and Journeys unchanged: Task 7's C7 proves the step reaches the table. §4 no new route, `refs` on columns: Task 5; Columns section, parent button, provenance, `schema` group: Task 6; cross-file foreign key: Task 6's App test. §5 fixture, `implemented_by`, CI walks, umami, gate and tier one identical: Tasks 7–8. §6 errors: malformed statement and unterminated model tests (Tasks 1–2); parse failure handling unchanged. §7 security: COPY and INSERT dropped (Task 2). §8 testing list: every item maps to a named test above except "ClickHouse types", covered in `quoted_backticked_and_schema_prefixed_names_are_unquoted` by `LowCardinality(String)`. §9 amendments: Task 8.

**Placeholder scan.** The App test in Task 6 Step 1 describes its data and reuses the file's existing mocking helpers rather than reprinting them; that is the one place an implementer must read a neighbouring test, and the test's name, data and assertion are given. Task 8's walk drafts are explicitly to be verified against the index, with the verification queries given.

**Type consistency.** `SchemaExtract.aliases: Vec<(String, String)>` (Task 1) is what Task 3 inserts; `Vocab = HashMap<String, String>` (Task 3) is what Task 4's `table_mentions` takes; `TreeSymbol.refs: Vec<String>` (Task 5) is `refs?: string[]` in TypeScript (Task 6); `Walk.implemented_by: Option<String>` (Task 7) is read by `grade_process` as a `path::name` compared with `CodeLinkDto.path`/`name`. `index_file(e, prior, now, vocab, force)` is called only from `process`.

**Review Focus.** Each line names its test: 1 → Task 2 `a_copy_data_block_and_inserts_are_dropped`; 2 → Task 4 `properties_and_strings_reference_tables_but_plain_identifiers_do_not`; 3 → Task 3 `a_vocabulary_change_reindexes_code_files_once`; 4 → Task 1 `an_unterminated_model_runs_to_the_end_of_the_file`; 5 → Task 6 `a_foreign_key_button_reaches_a_table_defined_in_another_file`.
