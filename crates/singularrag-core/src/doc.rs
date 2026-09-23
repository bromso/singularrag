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
const LOCKFILES: [&str; 9] = [
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "Cargo.lock",
    "bun.lock",
    "bun.lockb",
    "composer.lock",
    "Gemfile.lock",
    "poetry.lock",
];

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
    source.as_bytes()[..byte.min(source.len())]
        .iter()
        .filter(|&&b| b == b'\n')
        .count() as u32
        + 1
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
    p.set_language(&lang)
        .map_err(|e| Error::Tags(format!("{e:?}")))?;
    p.parse(source, None)
        .ok_or_else(|| Error::Tags("parse returned nothing".into()))
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
    tag(
        stem,
        "document",
        source,
        0,
        source.len().max(1),
        first_line(source),
    )
}

/// A config file's document tag: its signature names the format, never a value.
fn config_document_tag(stem: &str, source: &str, lang: Language) -> Tag {
    tag(
        stem,
        "document",
        source,
        0,
        source.len().max(1),
        lang.as_str().to_string(),
    )
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
            out.push((
                t.trim_end_matches("()").to_string(),
                line_of(source, offset + g.start()),
            ));
        }
    }
    for m in wiki.captures_iter(text) {
        let g = m.get(1).unwrap();
        out.push((
            g.as_str().trim().to_string(),
            line_of(source, offset + g.start()),
        ));
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
        let (s, e) = (
            s.saturating_sub(base).min(bytes.len()),
            e.saturating_sub(base).min(bytes.len()),
        );
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
        if let Some(n) = k
            .strip_prefix("atx_h")
            .and_then(|r| r.strip_suffix("_marker"))
        {
            return n.parse().unwrap_or(1);
        }
        if k == "setext_h1_underline" {
            return 1;
        }
        if k == "setext_h2_underline" {
            return 2;
        }
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

fn walk_sections(
    n: Node,
    source: &str,
    fences: &[(usize, usize)],
    d: &mut DocExtract,
    own: &mut Vec<(usize, usize)>,
) {
    let mut c = n.walk();
    let kids: Vec<Node> = n.children(&mut c).collect();
    for (i, &child) in kids.iter().enumerate() {
        // tree-sitter-md opens no `section` for a setext heading that follows other
        // content: it sits among its section's children, so it spans to the next
        // heading sibling or the end of the enclosing node.
        if child.kind() == "setext_heading" && !(i == 0 && n.kind() == "section") {
            let end = kids[i + 1..]
                .iter()
                .find(|k| matches!(k.kind(), "setext_heading" | "section"))
                .map_or(n.end_byte(), |k| k.start_byte());
            let name = heading_text(child, source);
            if name.is_empty() {
                continue;
            }
            let sig = truncate(source[child.byte_range()].lines().next().unwrap_or(""));
            let idx = d.tags.len();
            d.tags
                .push(tag(&name, "section", source, child.start_byte(), end, sig));
            own.push((child.start_byte(), end));
            let mut cuts: Vec<(usize, usize)> = fences.to_vec();
            cuts.push((child.start_byte(), child.end_byte()));
            d.bodies.push((
                idx,
                blank(&source[child.start_byte()..end], child.start_byte(), &cuts),
            ));
            continue;
        }
        if child.kind() != "section" {
            continue;
        }
        let heading = child
            .child(0)
            .filter(|h| h.kind() == "atx_heading" || h.kind() == "setext_heading");
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
        let sig = truncate(source[h.byte_range()].lines().next().unwrap_or(""));
        let t = tag(
            &name,
            "section",
            source,
            child.start_byte(),
            child.end_byte(),
            sig,
        );
        let idx = d.tags.len();
        d.tags.push(t);
        own.push((child.start_byte(), child.end_byte()));
        // Body: this section minus fences minus child sections that get their own tag.
        let mut child_own: Vec<(usize, usize)> = Vec::new();
        walk_sections(child, source, fences, d, &mut child_own);
        let mut cuts: Vec<(usize, usize)> = fences.to_vec();
        cuts.extend(child_own);
        cuts.push((h.start_byte(), h.end_byte()));
        d.bodies.push((
            idx,
            blank(&source[child.byte_range()], child.start_byte(), &cuts),
        ));
    }
}

// ---------- HTML ----------

fn attr(start_tag: Node, name: &str, source: &str) -> Option<String> {
    let mut c = start_tag.walk();
    for a in start_tag.children(&mut c) {
        if a.kind() != "attribute" {
            continue;
        }
        let mut ac = a.walk();
        let kids: Vec<Node> = a.children(&mut ac).collect();
        let an = kids.iter().find(|k| k.kind() == "attribute_name")?;
        if &source[an.byte_range()] != name {
            continue;
        }
        let val = kids
            .iter()
            .find(|k| k.kind() == "quoted_attribute_value" || k.kind() == "attribute_value")?;
        return Some(
            source[val.byte_range()]
                .trim_matches('"')
                .trim_matches('\'')
                .to_string(),
        );
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
                let tag_name = st
                    .child(1)
                    .map(|t| source[t.byte_range()].to_string())
                    .unwrap_or_default();
                let mut txt = String::new();
                text_of(n, source, &mut txt);
                let txt = truncate(&txt);
                if matches!(tag_name.as_str(), "h1" | "h2" | "h3" | "h4" | "h5" | "h6")
                    && !txt.is_empty()
                {
                    let idx = d.tags.len();
                    d.tags.push(tag(
                        &txt,
                        "section",
                        source,
                        n.start_byte(),
                        n.end_byte(),
                        txt.clone(),
                    ));
                    d.bodies.push((idx, txt.clone()));
                }
                if let Some(id) = attr(st, "id", source) {
                    let idx = d.tags.len();
                    d.tags.push(tag(
                        &id,
                        "element",
                        source,
                        n.start_byte(),
                        n.end_byte(),
                        format!("<{tag_name} id=\"{id}\">"),
                    ));
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
                    d.tags.push(tag(
                        &name,
                        "rule",
                        source,
                        n.start_byte(),
                        n.end_byte(),
                        name.clone(),
                    ));
                }
            }
            "media_statement" | "supports_statement" | "keyframes_statement" => {
                let block = n.child_by_field_name("body").or_else(|| {
                    let mut cc = n.walk();
                    let found = n
                        .children(&mut cc)
                        .find(|k| k.kind() == "block" || k.kind() == "keyframe_block_list");
                    found
                });
                let prelude_end = block.map(|b| b.start_byte()).unwrap_or(n.end_byte());
                let name = truncate(&source[n.start_byte()..prelude_end]);
                d.tags.push(tag(
                    &name,
                    "rule",
                    source,
                    n.start_byte(),
                    n.end_byte(),
                    name.clone(),
                ));
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
        "string"
        | "string_scalar"
        | "double_quote_scalar"
        | "single_quote_scalar"
        | "block_scalar" => "string",
        "number" | "integer" | "float" | "integer_scalar" | "float_scalar" => "number",
        "true" | "false" | "boolean_scalar" | "boolean" => "bool",
        _ => "string",
    }
}

fn push_key(
    d: &mut DocExtract,
    source: &str,
    path: &str,
    value_kind: &str,
    start: usize,
    end: usize,
) {
    d.tags.push(tag(
        path,
        "key",
        source,
        start,
        end,
        format!("{path}: {}", type_word(value_kind)),
    ));
}

/// Unquoted key text of a YAML/JSON key node.
fn key_text(n: Node, source: &str) -> String {
    source[n.byte_range()]
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_string()
}

/// The innermost value node: YAML wraps values in `flow_node`/`block_node`.
fn unwrap_value(n: Node) -> Node {
    let mut v = n;
    while matches!(v.kind(), "flow_node" | "block_node") {
        match v.child(0) {
            Some(c) => v = c,
            None => break,
        }
    }
    v
}

fn json_object(obj: Node, source: &str, prefix: &str, depth: usize, d: &mut DocExtract) {
    let mut c = obj.walk();
    for pair in obj.children(&mut c) {
        if pair.kind() != "pair" {
            continue;
        }
        let (Some(k), Some(v)) = (
            pair.child_by_field_name("key"),
            pair.child_by_field_name("value"),
        ) else {
            continue;
        };
        let path = if prefix.is_empty() {
            key_text(k, source)
        } else {
            format!("{prefix}.{}", key_text(k, source))
        };
        push_key(
            d,
            source,
            &path,
            v.kind(),
            pair.start_byte(),
            pair.end_byte(),
        );
        if v.kind() == "object" && depth < MAX_KEY_DEPTH {
            json_object(v, source, &path, depth + 1, d);
        }
    }
}

fn json(source: &str, stem: &str) -> Result<DocExtract> {
    let tree = parse(tree_sitter_json::LANGUAGE.into(), source)?;
    let mut d = DocExtract::default();
    d.tags
        .push(config_document_tag(stem, source, Language::Json));
    if let Some(obj) = tree.root_node().child(0).filter(|n| n.kind() == "object") {
        json_object(obj, source, "", 1, &mut d);
    }
    Ok(d)
}

fn yaml_mapping(map: Node, source: &str, prefix: &str, depth: usize, d: &mut DocExtract) {
    let mut c = map.walk();
    for pair in map.children(&mut c) {
        if pair.kind() != "block_mapping_pair" && pair.kind() != "flow_pair" {
            continue;
        }
        let Some(k) = pair.child_by_field_name("key") else {
            continue;
        };
        let path = if prefix.is_empty() {
            key_text(k, source)
        } else {
            format!("{prefix}.{}", key_text(k, source))
        };
        let v = pair.child_by_field_name("value").map(unwrap_value);
        push_key(
            d,
            source,
            &path,
            v.map(|v| v.kind()).unwrap_or("string"),
            pair.start_byte(),
            pair.end_byte(),
        );
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
    d.tags
        .push(config_document_tag(stem, source, Language::Yaml));
    let mut c = tree.root_node().walk();
    for doc in tree.root_node().children(&mut c) {
        if doc.kind() != "document" {
            continue;
        }
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
            n.children(&mut c)
                .filter(|k| k.kind() != ".")
                .map(|k| key_text(k, source))
                .collect::<Vec<_>>()
                .join(".")
        }
        _ => key_text(n, source),
    }
}

fn toml_pairs(container: Node, source: &str, prefix: &str, depth: usize, d: &mut DocExtract) {
    let mut c = container.walk();
    for pair in container.children(&mut c) {
        if pair.kind() != "pair" {
            continue;
        }
        let mut pc = pair.walk();
        let kids: Vec<Node> = pair.children(&mut pc).collect();
        let Some(k) = kids
            .iter()
            .find(|k| matches!(k.kind(), "bare_key" | "quoted_key" | "dotted_key"))
        else {
            continue;
        };
        let v = kids
            .iter()
            .rev()
            .find(|k| !matches!(k.kind(), "bare_key" | "quoted_key" | "dotted_key" | "="));
        let path = if prefix.is_empty() {
            toml_key_path(*k, source)
        } else {
            format!("{prefix}.{}", toml_key_path(*k, source))
        };
        if path.matches('.').count() >= MAX_KEY_DEPTH {
            continue;
        }
        push_key(
            d,
            source,
            &path,
            v.map(|v| v.kind()).unwrap_or("string"),
            pair.start_byte(),
            pair.end_byte(),
        );
        let _ = depth;
    }
}

fn toml(source: &str, stem: &str) -> Result<DocExtract> {
    let tree = parse(tree_sitter_toml_ng::LANGUAGE.into(), source)?;
    let mut d = DocExtract::default();
    d.tags
        .push(config_document_tag(stem, source, Language::Toml));
    let root = tree.root_node();
    toml_pairs(root, source, "", 1, &mut d);
    let mut seen_tables: Vec<String> = Vec::new();
    let mut c = root.walk();
    for n in root.children(&mut c) {
        if n.kind() != "table" && n.kind() != "table_array_element" {
            continue;
        }
        let Some(k) = n
            .child(1)
            .filter(|k| matches!(k.kind(), "bare_key" | "quoted_key" | "dotted_key"))
        else {
            continue;
        };
        let path = toml_key_path(k, source);
        if path.matches('.').count() >= MAX_KEY_DEPTH {
            continue;
        }
        if !seen_tables.contains(&path) {
            push_key(
                &mut d,
                source,
                &path,
                n.kind(),
                n.start_byte(),
                n.end_byte(),
            );
            seen_tables.push(path.clone());
        }
        toml_pairs(n, source, &path, 2, &mut d);
    }
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MD: &str = "# Title\n\nIntro `createSession` and [[design|the design]] and [x](../a/b.md#f) and [ext](https://x.io/y.md).\n\n```ts\nconst hidden = 1;\n```\n\n## Sub\n\nsub text mentions `SessionStore()`.\n\n#### Deep\n\nfolded into Sub.\n\n## Sub2\n\n<code>inline</code> with `not an identifier here`.\n";

    fn find<'a>(d: &'a DocExtract, kind: &str, name: &str) -> &'a Tag {
        d.tags
            .iter()
            .find(|t| t.kind == kind && t.name == name)
            .unwrap_or_else(|| panic!("{kind} {name} in {:?}", d.tags))
    }

    #[test]
    fn markdown_sections_fold_h4_and_carry_bodies() {
        let d = extract(Language::Markdown, MD, "readme").unwrap();
        let title = find(&d, "section", "Title");
        assert_eq!((title.line_start, title.line_end), (1, 19));
        assert_eq!(title.signature, "# Title");
        let sub = find(&d, "section", "Sub");
        assert_eq!(
            (sub.line_start, sub.line_end),
            (9, 16),
            "h4 folds into its parent"
        );
        assert!(d.tags.iter().all(|t| t.name != "Deep"));
        let doc = find(&d, "document", "readme");
        assert_eq!((doc.line_start, doc.line_end), (1, 19));
        let body = |name: &str| {
            let i = d.tags.iter().position(|t| t.name == name).unwrap();
            d.bodies
                .iter()
                .find(|(j, _)| *j == i)
                .map(|(_, b)| b.clone())
                .unwrap_or_default()
        };
        assert!(body("Sub").contains("sub text") && body("Sub").contains("folded into Sub"));
        assert!(
            !body("Title").contains("sub text"),
            "a child section's text is not repeated in the parent"
        );
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
        assert!(
            names.contains(&"SessionStore"),
            "trailing () dropped: {names:?}"
        );
        assert!(names.contains(&"inline"), "<code> in markdown: {names:?}");
        assert!(!names.contains(&"y"), "absolute URL ignored: {names:?}");
        assert!(!names.contains(&"hidden"), "fenced code ignored: {names:?}");
        assert!(!names.iter().any(|n| n.contains(' ')), "{names:?}");
        assert_eq!(
            d.mentions
                .iter()
                .find(|(n, _)| n == "createSession")
                .unwrap()
                .1,
            3
        );
    }

    #[test]
    fn setext_and_missing_h1_still_nest() {
        let src = "## Second level first\n\ntext\n\nSetext\n======\n\nmore\n";
        let d = extract(Language::Markdown, src, "odd").unwrap();
        assert!(d
            .tags
            .iter()
            .any(|t| t.kind == "section" && t.name == "Second level first"));
        assert!(d
            .tags
            .iter()
            .any(|t| t.kind == "section" && t.name == "Setext"));
        assert!(d
            .tags
            .iter()
            .any(|t| t.kind == "document" && t.name == "odd"));
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
        assert!(d
            .tags
            .iter()
            .any(|t| t.kind == "section" && t.name == "Sec"));
        assert!(d.tags.iter().any(|t| t.kind == "element" && t.name == "s"));
        let names: Vec<&str> = d.mentions.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["foo", "x"]);
        let i = d.tags.iter().position(|t| t.kind == "document").unwrap();
        let body = &d.bodies.iter().find(|(j, _)| *j == i).unwrap().1;
        assert!(
            body.contains("Hi") && body.contains("Sec") && !body.contains("<"),
            "{body}"
        );
    }

    #[test]
    fn css_rules_and_media_fold() {
        let src = ".a, .b > p { color: red }\n@media (max-width: 1px) { .c { x: y } }\n#id:hover { a: b }\n";
        let d = extract(Language::Css, src, "style").unwrap();
        let names: Vec<(&str, &str)> = d
            .tags
            .iter()
            .map(|t| (t.kind.as_str(), t.name.as_str()))
            .collect();
        assert!(names.contains(&("rule", ".a, .b > p")), "{names:?}");
        assert!(
            names.contains(&("rule", "@media (max-width: 1px)")),
            "{names:?}"
        );
        assert!(names.contains(&("rule", "#id:hover")), "{names:?}");
        assert!(!names.contains(&("rule", ".c")), "folded: {names:?}");
        assert!(d.bodies.is_empty() || d.bodies.iter().all(|(i, _)| d.tags[*i].kind == "document"));
    }

    #[test]
    fn json_yaml_toml_keys_to_depth_two_without_values() {
        let json = "{\"name\": \"x\", \"scripts\": {\"build\": \"tsc\", \"deep\": {\"more\": 1}}, \"arr\": [1], \"n\": 3, \"b\": true, \"token\": \"sk-secret\"}";
        let d = extract(Language::Json, json, "package").unwrap();
        let sigs: Vec<(&str, &str)> = d
            .tags
            .iter()
            .filter(|t| t.kind == "key")
            .map(|t| (t.name.as_str(), t.signature.as_str()))
            .collect();
        assert!(sigs.contains(&("name", "name: string")), "{sigs:?}");
        assert!(sigs.contains(&("scripts", "scripts: object")), "{sigs:?}");
        assert!(
            sigs.contains(&("scripts.build", "scripts.build: string")),
            "{sigs:?}"
        );
        assert!(
            sigs.contains(&("scripts.deep", "scripts.deep: object")),
            "{sigs:?}"
        );
        assert!(
            !sigs.iter().any(|(n, _)| n.contains("more")),
            "depth 3 dropped: {sigs:?}"
        );
        assert!(
            sigs.contains(&("arr", "arr: array"))
                && sigs.contains(&("n", "n: number"))
                && sigs.contains(&("b", "b: bool")),
            "{sigs:?}"
        );
        let all = format!("{:?}{:?}", d.tags, d.bodies);
        assert!(
            !all.contains("sk-secret") && !all.contains("tsc"),
            "values never leave the parser: {all}"
        );
        assert!(d.mentions.is_empty());

        let yaml = "name: x\nscripts:\n  build: tsc\n  deep:\n    more: 1\narr:\n  - 1\n";
        let d = extract(Language::Yaml, yaml, "ci").unwrap();
        let names: Vec<&str> = d
            .tags
            .iter()
            .filter(|t| t.kind == "key")
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec!["name", "scripts", "scripts.build", "scripts.deep", "arr"]
        );
        assert_eq!(
            find(&d, "key", "scripts.deep").signature,
            "scripts.deep: object"
        );

        let toml = "name = \"x\"\n[scripts]\nbuild = \"tsc\"\n[scripts.deep]\nmore = 1\n[[bin]]\nname = \"a\"\n";
        let d = extract(Language::Toml, toml, "Cargo").unwrap();
        let names: Vec<&str> = d
            .tags
            .iter()
            .filter(|t| t.kind == "key")
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec![
                "name",
                "scripts",
                "scripts.build",
                "scripts.deep",
                "bin",
                "bin.name"
            ]
        );
        assert_eq!(find(&d, "key", "scripts").signature, "scripts: object");
        assert_eq!(find(&d, "key", "bin").signature, "bin: array");
    }

    #[test]
    fn top_level_array_and_multi_document_yaml() {
        let d = extract(Language::Json, "[1, 2, {\"a\": 1}]", "list").unwrap();
        assert_eq!(d.tags.len(), 1);
        assert_eq!(d.tags[0].kind, "document");
        let d = extract(Language::Yaml, "a: 1\n---\nb: 2\n", "multi").unwrap();
        let names: Vec<&str> = d
            .tags
            .iter()
            .filter(|t| t.kind == "key")
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[test]
    fn plain_text_is_one_document_with_a_body() {
        let d = extract(
            Language::Text,
            "First line here.\nSecond `ident` line.\n",
            "notes",
        )
        .unwrap();
        assert_eq!(d.tags.len(), 1);
        assert_eq!(d.tags[0].signature, "First line here.");
        assert_eq!(
            d.bodies[0].1.trim(),
            "First line here.\nSecond `ident` line."
        );
        assert_eq!(d.mentions, vec![("ident".to_string(), 2)]);
    }

    #[test]
    fn skip_reasons() {
        assert!(
            is_lockfile("package-lock.json")
                && is_lockfile("Cargo.lock")
                && is_lockfile("bun.lockb")
        );
        assert!(!is_lockfile("lock.md"));
        assert_eq!(skip_reason("a/pnpm-lock.yaml", 10, "x"), Some("lockfile"));
        assert_eq!(
            skip_reason("a/b.json", DOC_MAX_BYTES + 1, "x"),
            Some("too-large")
        );
        let minified = "a".repeat(600);
        assert_eq!(skip_reason("a/b.css", 600, &minified), Some("minified"));
        assert_eq!(skip_reason("a/b.md", 10, "# ok\n"), None);
    }
}
