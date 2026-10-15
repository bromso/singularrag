//! Database schemas as tables and columns (schema spec §2–§3): a Prisma schema and SQL
//! DDL (migrations, dumps) parsed by hand into the same `Tag`s the indexer writes for
//! code, plus the alias vocabulary and the code-side table mentions. No model and no
//! connection: nothing here executes anything.

#[allow(unused_imports)]
use std::collections::{HashMap, HashSet};
#[allow(unused_imports)]
use std::sync::OnceLock;

#[allow(unused_imports)]
use regex::Regex;
#[allow(unused_imports)]
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

fn sql(_: &str) -> SchemaExtract {
    SchemaExtract::default()
}

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
#[allow(dead_code)]
fn line_of(source: &str, byte: usize) -> u32 {
    source.as_bytes()[..byte.min(source.len())]
        .iter()
        .filter(|&&b| b == b'\n')
        .count() as u32
        + 1
}

fn def(name: String, kind: &str, line_start: u32, line_end: u32, signature: String) -> Tag {
    Tag {
        name,
        kind: kind.into(),
        is_definition: true,
        line_start,
        line_end,
        signature,
    }
}

/// A foreign key: a reference tag named after the referenced table, at the column's line.
/// The indexer writes non-definition tags as `refs` rows (spec §3).
fn fk(table: String, line: u32) -> Tag {
    Tag {
        name: table,
        kind: "reference".into(),
        is_definition: false,
        line_start: line,
        line_end: line,
        signature: String::new(),
    }
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
    let name: String = rest
        .trim_start()
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
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
    rest[..end]
        .split(',')
        .next()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn prisma(source: &str) -> SchemaExtract {
    let lines: Vec<&str> = source.lines().collect();
    // (model, table, first line index, last line index, closing brace found); an
    // unterminated model runs to EOF, where the last line index is content, not a
    // delimiter, so the field scan below must include it.
    let mut blocks: Vec<(String, String, usize, usize, bool)> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let Some(model) = model_header(lines[i]) else {
            i += 1;
            continue;
        };
        let mut end = lines.len().saturating_sub(1);
        let mut terminated = false;
        let mut table = model.clone();
        for (j, l) in lines.iter().enumerate().skip(i + 1) {
            let t = l.trim();
            if t == "}" {
                end = j;
                terminated = true;
                break;
            }
            if t.starts_with("@@map(") {
                if let Some(m) = attr_string(t, "@@map(") {
                    table = m;
                }
            }
        }
        blocks.push((model, table, i, end, terminated));
        i = end + 1;
    }
    let tables: HashMap<&str, &str> = blocks
        .iter()
        .map(|(m, t, _, _, _)| (m.as_str(), t.as_str()))
        .collect();
    let mut out = SchemaExtract::default();
    for (model, table, start, end, terminated) in &blocks {
        let mut columns: Vec<Tag> = Vec::new();
        let mut scalar_lines: HashMap<String, u32> = HashMap::new();
        let mut fks: Vec<(String, String, u32)> = Vec::new(); // (local field, referenced table, relation line)
        let field_end = if *terminated { *end } else { end + 1 };
        #[allow(clippy::needless_range_loop)]
        for j in start + 1..field_end {
            let t = lines[j].trim();
            if t.is_empty() || t.starts_with("//") || t.starts_with("@@") {
                continue;
            }
            let Some((name, rest)) = split_field(t) else {
                continue;
            };
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
        out.tags.push(def(
            table.clone(),
            "table",
            *start as u32 + 1,
            *end as u32 + 1,
            sig,
        ));
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
        tags.iter()
            .filter(|t| t.kind == kind && t.is_definition)
            .map(|t| t.name.clone())
            .collect()
    }

    #[test]
    fn prisma_models_become_tables_named_by_map_and_fields_become_columns() {
        let x = extract(Language::Prisma, PRISMA).unwrap();
        assert_eq!(names(&x.tags, "table"), vec!["user", "website", "Team"]);
        assert_eq!(
            names(&x.tags, "column"),
            vec![
                "user.user_id",
                "user.username",
                "user.logo_url",
                "user.role",
                "website.website_id",
                "website.name",
                "website.user_id",
                "website.created_by",
                "Team.id",
                "Team.name",
            ]
        );
        let user = x.tags.iter().find(|t| t.name == "user").unwrap();
        assert_eq!((user.line_start, user.line_end), (9, 18));
        assert_eq!(user.signature, "table user (4 columns, model User)");
        let team = x.tags.iter().find(|t| t.name == "Team").unwrap();
        assert_eq!(team.signature, "table Team (2 columns)");
        let logo = x.tags.iter().find(|t| t.name == "user.logo_url").unwrap();
        assert_eq!(
            logo.signature,
            "String? @map(\"logo_url\") @db.VarChar(2183)"
        );
        assert_eq!((logo.line_start, logo.line_end), (13, 13));
    }

    #[test]
    fn prisma_relations_are_not_columns_and_foreign_keys_sit_on_the_scalar_column() {
        let x = extract(Language::Prisma, PRISMA).unwrap();
        assert!(!x.tags.iter().any(|t| t.name.ends_with(".websites")
            || t.name.ends_with(".user")
            || t.name.ends_with(".creator")));
        let fks: Vec<(String, u32)> = x
            .tags
            .iter()
            .filter(|t| !t.is_definition)
            .map(|t| (t.name.clone(), t.line_start))
            .collect();
        // userId is line 23, createdBy line 25: both reference the `user` table
        assert_eq!(
            fks,
            vec![("user".to_string(), 23), ("user".to_string(), 25)]
        );
        assert!(x
            .tags
            .iter()
            .filter(|t| !t.is_definition)
            .all(|t| t.kind == "reference"));
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
