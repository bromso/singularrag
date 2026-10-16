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
                        let (col, ty) =
                            piece.split_once(char::is_whitespace).unwrap_or((piece, ""));
                        if let Some(r) = re_refs.captures(ty) {
                            refs.push(fk(unquote(&r[1]), line));
                        }
                        columns.push(def(
                            format!("{name}.{}", unquote(col)),
                            "column",
                            line,
                            line,
                            truncate(ty),
                        ));
                    }
                }
            }
            let start_line = line_of(source, name_at);
            let end_line = line_of(source, off + s.trim_end().len().saturating_sub(1));
            out.tags.push(def(
                name.clone(),
                "table",
                start_line,
                end_line.max(start_line),
                truncate(&format!("table {name} ({} columns)", columns.len())),
            ));
            out.tags.extend(columns);
            out.tags.extend(refs);
            if seen.insert(name.clone()) {
                out.aliases.push((name.clone(), name));
            }
        } else if let Some(m) = re_view.captures(s) {
            let name = unquote(&m[1]);
            let line = line_of(source, off + m.get(1).expect("group 1").start());
            out.tags.push(def(
                name.clone(),
                "table",
                line,
                line,
                truncate(&format!("view {name}")),
            ));
            if seen.insert(name.clone()) {
                out.aliases.push((name.clone(), name));
            }
        } else if let Some(m) = re_alter.captures(s) {
            let table = unquote(&m[1]);
            let actions_off = m.get(2).expect("group 2").start();
            for (po, action) in pieces(&m[2]) {
                let line = line_of(source, off + actions_off + po);
                let Some(rest) = strip_prefix_ci(action, "ADD") else {
                    continue;
                };
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
                let rest = strip_prefix_ci(rest, "COLUMN")
                    .map(str::trim_start)
                    .unwrap_or(rest);
                let rest = strip_prefix_ci(rest, "IF NOT EXISTS")
                    .map(str::trim_start)
                    .unwrap_or(rest);
                let (col, ty) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                if let Some(r) = re_refs.captures(ty) {
                    out.tags.push(fk(unquote(&r[1]), line));
                }
                out.tags.push(def(
                    format!("{table}.{}", unquote(col)),
                    "column",
                    line,
                    line,
                    truncate(ty),
                ));
            }
        }
    }
    out
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
            truncate(&sig),
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
            for byte in blanked.iter_mut().take(eol).skip(i) {
                *byte = b' ';
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
                for byte in blanked.iter_mut().take(eol).skip(i) {
                    *byte = b' ';
                }
                i = eol;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let end = source[i + 2..]
                    .find("*/")
                    .map_or(b.len(), |n| i + 2 + n + 2);
                for byte in blanked.iter_mut().take(end.min(b.len())).skip(i) {
                    if *byte != b'\n' {
                        *byte = b' ';
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
                let end = source[i + 2..]
                    .find("$$")
                    .map_or(b.len(), |n| i + 2 + n + 2);
                i = end;
            }
            b';' => {
                let text = String::from_utf8_lossy(&blanked[start..i]).into_owned();
                let head = text.trim_start();
                let is_copy = head
                    .get(..4)
                    .is_some_and(|h| h.eq_ignore_ascii_case("copy"))
                    && text.to_ascii_uppercase().contains("FROM STDIN");
                out.push((start, text));
                i += 1;
                start = i;
                if is_copy {
                    // the data begins on the next line
                    let eol = source[i.min(b.len())..]
                        .find('\n')
                        .map_or(b.len(), |n| i + n);
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
    last.trim_matches(|c| c == '"' || c == '`' || c == '[' || c == ']')
        .to_string()
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
    "CONSTRAINT",
    "PRIMARY",
    "UNIQUE",
    "INDEX",
    "KEY",
    "FOREIGN",
    "CHECK",
    "EXCLUDE",
    "FULLTEXT",
    "SPATIAL",
    "PROJECTION",
    "LIKE",
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
            Regex::new(r"(?is)\bREFERENCES\s+([^\s(,;]+)").unwrap(),
        )
    })
}

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
        Language::TypeScript => (
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            &["property_identifier"],
            &["string_fragment"],
        ),
        Language::Tsx => (
            tree_sitter_typescript::LANGUAGE_TSX.into(),
            &["property_identifier"],
            &["string_fragment"],
        ),
        Language::JavaScript => (
            tree_sitter_javascript::LANGUAGE.into(),
            &["property_identifier"],
            &["string_fragment"],
        ),
        Language::Rust => (
            tree_sitter_rust::LANGUAGE.into(),
            &["field_identifier"],
            &["string_content"],
        ),
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
                for (i, tok) in text
                    .split(|c: char| !c.is_alphanumeric() && c != '_')
                    .enumerate()
                {
                    if tok.is_empty() {
                        continue;
                    }
                    if let Some(table) = vocab.get(tok) {
                        // a multi-line template: the token's own line
                        let offset = text
                            .split(|c: char| !c.is_alphanumeric() && c != '_')
                            .take(i)
                            .map(|s| s.len() + 1)
                            .sum::<usize>();
                        let extra = text.as_bytes()[..offset.min(text.len())]
                            .iter()
                            .filter(|&&b| b == b'\n')
                            .count() as u32;
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

    #[test]
    fn a_long_table_signature_is_cut_at_the_cap() {
        let model = "A".repeat(200);
        let table = "B".repeat(200);
        let src = format!("model {model} {{\n  id String @id\n\n  @@map(\"{table}\")\n}}\n");
        let x = extract(Language::Prisma, &src).unwrap();
        let t = x.tags.iter().find(|t| t.name == table).unwrap();
        assert_eq!(t.signature.chars().count(), SIGNATURE_MAX);
        assert!(t.signature.ends_with('…'));
    }

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
                "user.user_id",
                "user.username",
                "user.created_at",
                "website.website_id",
                "website.name",
                "website.user_id",
                "website.labels",
                "website.domain",
                "website.share_id",
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
        assert_eq!(
            (domain.signature.as_str(), domain.line_start),
            ("VARCHAR(500)", 27)
        );
        let share = x
            .tags
            .iter()
            .find(|t| t.name == "website.share_id")
            .unwrap();
        assert_eq!(
            (share.signature.as_str(), share.line_start),
            ("VARCHAR(50)", 28)
        );
        assert_eq!(
            x.aliases,
            vec![
                ("user".to_string(), "user".to_string()),
                ("website".to_string(), "website".to_string())
            ]
        );
    }

    #[test]
    fn inline_and_constraint_foreign_keys_become_references_at_their_lines() {
        let x = extract(Language::Sql, MIGRATION).unwrap();
        let fks: Vec<(String, u32)> = x
            .tags
            .iter()
            .filter(|t| !t.is_definition)
            .map(|t| (t.name.clone(), t.line_start))
            .collect();
        assert_eq!(
            fks,
            vec![("user".to_string(), 17), ("user".to_string(), 20)]
        );
    }

    #[test]
    fn a_word_ending_in_references_is_not_a_foreign_key() {
        let src = "CREATE TABLE t (\n  \"notes\" TEXT DEFAULT 'set your preferences here',\n  \"owner\" UUID REFERENCES \"user\"(\"user_id\")\n);\n";
        let x = extract(Language::Sql, src).unwrap();
        let fks: Vec<&str> = x
            .tags
            .iter()
            .filter(|t| !t.is_definition)
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(fks, vec!["user"]);
    }

    #[test]
    fn quoted_backticked_and_schema_prefixed_names_are_unquoted() {
        let src = "CREATE TABLE `team_user` (\n  `id` INT,\n  KEY `k` (`id`)\n) ENGINE=InnoDB;\nCREATE TABLE umami.website_event\n(\n    website_id UUID,\n    hostname LowCardinality(String),\n    INDEX idx hostname TYPE bloom_filter GRANULARITY 1\n)\nENGINE = MergeTree\nORDER BY (website_id);\nCREATE TABLE \"public\".\"report\" (report_id uuid);\n";
        let x = extract(Language::Sql, src).unwrap();
        assert_eq!(
            names(&x.tags, "table"),
            vec!["team_user", "website_event", "report"]
        );
        assert_eq!(
            names(&x.tags, "column"),
            vec![
                "team_user.id",
                "website_event.website_id",
                "website_event.hostname",
                "report.report_id"
            ]
        );
        let host = x
            .tags
            .iter()
            .find(|t| t.name == "website_event.hostname")
            .unwrap();
        assert_eq!(host.signature, "LowCardinality(String)");
    }

    #[test]
    fn views_are_tables_whose_signature_says_view() {
        let src = "CREATE MATERIALIZED VIEW umami.website_event_stats_hourly_mv\nTO umami.website_event_stats_hourly\nAS SELECT website_id FROM umami.website_event;\nCREATE OR REPLACE VIEW v_sessions AS SELECT 1;\n";
        let x = extract(Language::Sql, src).unwrap();
        let v: Vec<(String, String)> = x
            .tags
            .iter()
            .map(|t| (t.name.clone(), t.signature.clone()))
            .collect();
        assert_eq!(
            v,
            vec![
                (
                    "website_event_stats_hourly_mv".to_string(),
                    "view website_event_stats_hourly_mv".to_string()
                ),
                ("v_sessions".to_string(), "view v_sessions".to_string()),
            ]
        );
    }

    #[test]
    fn a_copy_data_block_and_inserts_are_dropped() {
        let src = "CREATE TABLE public.report (\n    report_id uuid NOT NULL\n);\n\nCOPY public.report (report_id) FROM stdin;\n0b1c\tCREATE TABLE phantom (x int);\n\\.\n\nINSERT INTO public.report VALUES ('CREATE TABLE phantom2 (y int);');\nALTER TABLE ONLY public.report\n    ADD CONSTRAINT report_website_fk FOREIGN KEY (website_id) REFERENCES public.website(website_id);\n";
        let x = extract(Language::Sql, src).unwrap();
        assert_eq!(names(&x.tags, "table"), vec!["report"]);
        assert_eq!(names(&x.tags, "column"), vec!["report.report_id"]);
        let fks: Vec<(String, u32)> = x
            .tags
            .iter()
            .filter(|t| !t.is_definition)
            .map(|t| (t.name.clone(), t.line_start))
            .collect();
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

    #[test]
    fn a_long_sql_table_name_is_cut_in_its_signature() {
        let table = "A".repeat(200);
        let src = format!("CREATE TABLE {table} (id int);\n");
        let x = extract(Language::Sql, &src).unwrap();
        let t = x.tags.iter().find(|t| t.name == table).unwrap();
        assert_eq!(t.signature.chars().count(), SIGNATURE_MAX);
        assert!(t.signature.ends_with('…'));
    }

    fn vocab() -> HashMap<String, String> {
        [
            ("website", "website"),
            ("Website", "website"),
            ("websiteEvent", "website_event"),
            ("website_event", "website_event"),
            ("session", "session"),
            ("user", "user"),
        ]
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
        assert_eq!(
            m,
            vec![("session".to_string(), 3), ("website".to_string(), 4)]
        );
    }

    #[test]
    fn documents_and_schema_files_get_no_table_mentions_and_an_empty_vocabulary_yields_nothing() {
        assert!(table_mentions(Language::Markdown, "`website`", &vocab())
            .unwrap()
            .is_empty());
        assert!(
            table_mentions(Language::Sql, "select * from website", &vocab())
                .unwrap()
                .is_empty()
        );
        assert!(
            table_mentions(Language::TypeScript, "x.website", &HashMap::new())
                .unwrap()
                .is_empty()
        );
    }
}
