//! Language detection and tag extraction. Uses each grammar's bundled `tags.scm`
//! through `tree-sitter-tags`; nothing here parses code by hand.

use std::cell::RefCell;
use std::collections::HashMap;

use tree_sitter_tags::{TagsConfiguration, TagsContext};

use crate::{Error, Result};

pub const SIGNATURE_MAX: usize = 160;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    TypeScript,
    Tsx,
    JavaScript,
    Rust,
}

impl Language {
    pub fn from_path(path: &str) -> Option<Language> {
        let ext = path.rsplit('.').next()?;
        match ext {
            "ts" | "mts" | "cts" => Some(Language::TypeScript),
            "tsx" => Some(Language::Tsx),
            "js" | "mjs" | "cjs" | "jsx" => Some(Language::JavaScript),
            "rs" => Some(Language::Rust),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Language::TypeScript => "typescript",
            Language::Tsx => "tsx",
            Language::JavaScript => "javascript",
            Language::Rust => "rust",
        }
    }
}

/// The bundled Rust `tags.scm` captures `foo(...)` and `self.foo(...)` calls but not calls
/// through a path like `util::len(...)` (function: `scoped_identifier`). This supplement adds
/// that case so scoped calls still produce a reference tag.
const RUST_CALLS: &str = r#"
(call_expression
    function: (scoped_identifier
        name: (identifier) @name)) @reference.call
"#;

#[derive(Debug, Clone, PartialEq)]
pub struct Tag {
    pub name: String,
    pub kind: String,
    pub is_definition: bool,
    pub line_start: u32,
    pub line_end: u32,
    pub signature: String,
}

fn make_config(lang: Language) -> Result<TagsConfiguration> {
    let (language, tags, locals) = match lang {
        Language::TypeScript => (
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            format!(
                "{}\n{}",
                tree_sitter_javascript::TAGS_QUERY,
                tree_sitter_typescript::TAGS_QUERY
            ),
            tree_sitter_typescript::LOCALS_QUERY,
        ),
        Language::Tsx => (
            tree_sitter_typescript::LANGUAGE_TSX.into(),
            format!(
                "{}\n{}",
                tree_sitter_javascript::TAGS_QUERY,
                tree_sitter_typescript::TAGS_QUERY
            ),
            tree_sitter_typescript::LOCALS_QUERY,
        ),
        Language::JavaScript => (
            tree_sitter_javascript::LANGUAGE.into(),
            tree_sitter_javascript::TAGS_QUERY.to_string(),
            tree_sitter_javascript::LOCALS_QUERY,
        ),
        Language::Rust => (
            tree_sitter_rust::LANGUAGE.into(),
            format!("{}\n{}", tree_sitter_rust::TAGS_QUERY, RUST_CALLS),
            "",
        ),
    };
    TagsConfiguration::new(language, &tags, locals)
        .map_err(|e| Error::Tags(format!("{lang:?}: {e:?}")))
}

thread_local! {
    static CONFIGS: RefCell<HashMap<Language, TagsConfiguration>> = RefCell::new(HashMap::new());
    static CONTEXT: RefCell<TagsContext> = RefCell::new(TagsContext::new());
}

fn signature_for(source: &str, byte_start: usize) -> String {
    let line_start = source[..byte_start].rfind('\n').map_or(0, |i| i + 1);
    let line_end = source[byte_start..]
        .find('\n')
        .map_or(source.len(), |i| byte_start + i);
    let line = source[line_start..line_end].trim();
    if line.chars().count() > SIGNATURE_MAX {
        let mut s: String = line.chars().take(SIGNATURE_MAX - 1).collect();
        s.push('…');
        s
    } else {
        line.to_string()
    }
}

/// 1-based line number containing `byte_offset`. `tree_sitter_tags::Tag::span` only covers
/// the *name* node, so multi-line definitions (e.g. a function's closing brace) need this
/// computed from `Tag::range` (the full definition node's byte range) instead.
fn line_of(source: &str, byte_offset: usize) -> u32 {
    source.as_bytes()[..byte_offset]
        .iter()
        .filter(|&&b| b == b'\n')
        .count() as u32
        + 1
}

pub fn extract_tags(lang: Language, source: &str) -> Result<Vec<Tag>> {
    CONFIGS.with(|configs| {
        let mut configs = configs.borrow_mut();
        let config = match configs.entry(lang) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => e.insert(make_config(lang)?),
        };
        CONTEXT.with(|ctx| {
            let mut ctx = ctx.borrow_mut();
            let (iter, _has_error) = ctx
                .generate_tags(config, source.as_bytes(), None)
                .map_err(|e| Error::Tags(format!("{e:?}")))?;
            let mut out = Vec::new();
            for tag in iter {
                let tag = tag.map_err(|e| Error::Tags(format!("{e:?}")))?;
                let name = source[tag.name_range.clone()].to_string();
                if name.is_empty() {
                    continue;
                }
                out.push(Tag {
                    name,
                    kind: config.syntax_type_name(tag.syntax_type_id).to_string(),
                    is_definition: tag.is_definition,
                    line_start: line_of(source, tag.range.start),
                    line_end: line_of(source, tag.range.end.saturating_sub(1).max(tag.range.start)),
                    signature: signature_for(source, tag.range.start),
                });
            }
            Ok(out)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TS: &str = r#"export interface Session { id: string }
export function createSession(user: User, ttl: number): Session {
  const store = new SessionStore();
  return store.create(user, ttl);
}
export class SessionStore {
  create(user: User, ttl: number): Session {
    return { id: "s1" };
  }
}
"#;

    fn names(tags: &[Tag], def: bool) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = tags
            .iter()
            .filter(|t| t.is_definition == def)
            .map(|t| (t.name.clone(), t.kind.clone()))
            .collect();
        v.sort();
        v.dedup();
        v
    }

    #[test]
    fn detects_language_from_extension() {
        assert_eq!(Language::from_path("src/a.ts"), Some(Language::TypeScript));
        assert_eq!(Language::from_path("src/a.tsx"), Some(Language::Tsx));
        assert_eq!(Language::from_path("src/a.js"), Some(Language::JavaScript));
        assert_eq!(Language::from_path("src/a.mjs"), Some(Language::JavaScript));
        assert_eq!(Language::from_path("src/a.rs"), Some(Language::Rust));
        assert_eq!(Language::from_path("README.md"), None);
    }

    #[test]
    fn typescript_definitions_and_references() {
        let tags = extract_tags(Language::TypeScript, TS).unwrap();
        let defs = names(&tags, true);
        assert!(defs.contains(&("createSession".into(), "function".into())));
        assert!(defs.contains(&("SessionStore".into(), "class".into())));
        assert!(defs.contains(&("create".into(), "method".into())));
        assert!(defs.contains(&("Session".into(), "interface".into())));
        let refs = names(&tags, false);
        assert!(
            refs.iter().any(|(n, _)| n == "create"),
            "call ref missing: {refs:?}"
        );
        assert!(
            refs.iter().any(|(n, _)| n == "SessionStore"),
            "new ref missing: {refs:?}"
        );
        let f = tags
            .iter()
            .find(|t| t.name == "createSession" && t.is_definition)
            .unwrap();
        assert_eq!(f.line_start, 2);
        assert_eq!(f.line_end, 5);
        assert_eq!(
            f.signature,
            "export function createSession(user: User, ttl: number): Session {"
        );
    }

    #[test]
    fn rust_definitions_and_scoped_call_references() {
        let src = "pub fn parse(s: &str) -> u32 { helper(s) + util::len(s) }\nfn helper(s: &str) -> u32 { 0 }\n";
        let tags = extract_tags(Language::Rust, src).unwrap();
        let defs = names(&tags, true);
        assert!(defs.contains(&("parse".into(), "function".into())));
        assert!(defs.contains(&("helper".into(), "function".into())));
        let refs = names(&tags, false);
        assert!(refs.iter().any(|(n, _)| n == "helper"));
        assert!(refs.iter().any(|(n, _)| n == "len"));
    }

    #[test]
    fn signature_is_capped() {
        let long = format!("export function f({}) {{}}", "a: number, ".repeat(40));
        let tags = extract_tags(Language::TypeScript, &long).unwrap();
        let f = tags.iter().find(|t| t.name == "f").unwrap();
        assert!(f.signature.chars().count() <= SIGNATURE_MAX);
        assert!(f.signature.ends_with('…'));
    }
}
