//! Authored map: `.singularrag/map.toml`. Committed to the repo; the UI edits it.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

pub const MAP_FILE: &str = ".singularrag/map.toml";

pub const MAP_HEADER: &str = "# Managed by singularrag serve. Hand edits and comments between sections are kept; a comment inside an entry the page rewrites is not.\n";

const OWNED_KEYS: [&str; 5] = ["pin", "exclude", "note", "boundary", "deny"];

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MapConfigError {
    pub field: String,
    pub message: String,
}

/// Never shrinkable. `map.toml` may only add patterns.
pub const BUILTIN_DENY: &[&str] = &[
    ".env*",
    "*.pem",
    "*.key",
    "id_rsa*",
    "*.p12",
    "*.pfx",
    ".npmrc",
    ".netrc",
    "*.tfstate",
    "secrets/",
    "credentials*",
];

pub(crate) fn check_path(field: &str, p: &str) -> std::result::Result<(), MapConfigError> {
    let bad = |message: &str| MapConfigError {
        field: field.to_string(),
        message: message.to_string(),
    };
    if p.is_empty() {
        return Err(bad("path is empty"));
    }
    // `nth(1) == ':'` catches a Windows drive letter (`C:\x`, `C:/x`). It over-rejects —
    // any path whose second character is a colon goes with it — but no repo-relative path
    // looks like that in practice, and the failure mode is a clear 422 rather than a write
    // outside the repo. Ruled acceptable in Task 3.
    if p.starts_with('/') || p.contains('\\') || p.chars().nth(1) == Some(':') {
        return Err(bad("path must be repo-relative with forward slashes"));
    }
    if p.starts_with("./") {
        return Err(bad("path must not start with ./"));
    }
    if p.split('/').any(|seg| seg == "..") {
        return Err(bad("path must not contain .."));
    }
    Ok(())
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct MapConfig {
    // `skip_serializing_if` stays on the four list fields for a `toml_edit::ser::to_string_pretty`
    // reason, not a JSON one: pretty-printing an empty `Vec` of structs as an array of tables
    // renders a bare `pin=` with no value (a toml_edit pretty-serializer bug), which then fails
    // to parse back. Omitting the key for an empty list sidesteps it. `save_atomic` treats an
    // owned key's absence from the freshly serialized document as "remove this section" either
    // way, so the two are consistent for a section that's genuinely empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pin: Vec<Target>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<Target>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub note: Vec<Note>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub boundary: Vec<Boundary>,
    // Deny is a plain struct (not a list of structs), so it does not hit the pretty-printer bug
    // above; keeping it unconditional means `[deny]` — the never-shrinkable security section —
    // always renders once map.toml owns it, even with an empty `extra_patterns`.
    #[serde(default)]
    pub deny: Deny,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Target {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
}

/// Who may write a note besides a human: the MCP `annotate` tool.
pub const AGENT: &str = "agent";
/// One paragraph, at most this many characters after normalisation.
pub const NOTE_MAX_CHARS: usize = 300;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    pub text: String,
    /// Absent for a human (the UI or a hand edit); `"agent"` for the MCP tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    /// The MCP session key that wrote it; present exactly when `by = "agent"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// RFC 3339 UTC; present exactly when `by = "agent"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
}

impl Note {
    pub fn is_agent(&self) -> bool {
        self.by.as_deref() == Some(AGENT)
    }
}

/// Control characters and whitespace runs become one space; the ends are trimmed.
pub fn normalise_note_text(text: &str) -> String {
    text.split(|c: char| c.is_control() || c.is_whitespace())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Boundary {
    pub name: String,
    pub paths: Vec<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Deny {
    #[serde(default)]
    pub extra_patterns: Vec<String>,
}

/// Extract the leading decorator (prefix) from a toml_edit::Item, which holds comments that
/// precede the item.
fn leading_prefix(item: &toml_edit::Item) -> Option<toml_edit::RawString> {
    match item {
        toml_edit::Item::ArrayOfTables(a) => a.get(0).and_then(|t| t.decor().prefix().cloned()),
        toml_edit::Item::Table(t) => t.decor().prefix().cloned(),
        toml_edit::Item::Value(v) => v.decor().prefix().cloned(),
        _ => None,
    }
}

/// Set the leading decorator (prefix) on a toml_edit::Item.
fn set_leading_prefix(item: &mut toml_edit::Item, prefix: toml_edit::RawString) {
    match item {
        toml_edit::Item::ArrayOfTables(a) => {
            if let Some(t) = a.get_mut(0) {
                t.decor_mut().set_prefix(prefix);
            }
        }
        toml_edit::Item::Table(t) => t.decor_mut().set_prefix(prefix),
        toml_edit::Item::Value(v) => v.decor_mut().set_prefix(prefix),
        _ => {}
    }
}

/// The document position of an owned item: a table's own position, or (for an array of
/// tables) its first table's position. `None` for a plain value (e.g. `pin = []`) or an
/// item that was never parsed from text (so has no recorded position).
fn item_position(item: &toml_edit::Item) -> Option<isize> {
    match item {
        toml_edit::Item::Table(t) => t.position(),
        toml_edit::Item::ArrayOfTables(a) => a.get(0).and_then(|t| t.position()),
        _ => None,
    }
}

/// Stamp `pos` onto every table within an owned item so it sorts at a known place among the
/// document's other tables, regardless of where toml_edit's serializer or IndexMap happened
/// to put it.
fn set_item_position(item: &mut toml_edit::Item, pos: isize) {
    match item {
        toml_edit::Item::Table(t) => t.set_position(Some(pos)),
        toml_edit::Item::ArrayOfTables(a) => {
            for t in a.iter_mut() {
                t.set_position(Some(pos));
            }
        }
        _ => {}
    }
}

/// The smallest position among the document's top-level tables (bare tables and every table
/// inside an array of tables). `None` when no top-level table carries a position.
fn min_table_position(doc: &toml_edit::DocumentMut) -> Option<isize> {
    doc.iter().flat_map(|(_, item)| table_positions(item)).min()
}

/// The largest position among the document's top-level tables. See `min_table_position`.
fn max_table_position(doc: &toml_edit::DocumentMut) -> Option<isize> {
    doc.iter().flat_map(|(_, item)| table_positions(item)).max()
}

fn table_positions(item: &toml_edit::Item) -> Vec<isize> {
    match item {
        toml_edit::Item::Table(t) => t.position().into_iter().collect(),
        toml_edit::Item::ArrayOfTables(a) => a.iter().filter_map(|t| t.position()).collect(),
        _ => Vec::new(),
    }
}

/// A removed owned section's leading comment (`prefix`) must not vanish with it. Prepend it
/// to the leading prefix of the next owned key (in `OWNED_KEYS` order) that still exists in
/// `doc`, so the comment now precedes whatever section follows on disk; if none of the later
/// owned keys survived, the comment has nothing left to precede, so it is appended to the
/// document's trailing content instead, which always renders last.
fn carry_removed_comment(doc: &mut toml_edit::DocumentMut, removed: &str, prefix: &str) {
    let start = OWNED_KEYS
        .iter()
        .position(|k| *k == removed)
        .map(|i| i + 1)
        .unwrap_or(OWNED_KEYS.len());
    let next_key = OWNED_KEYS[start..].iter().find(|k| doc.contains_key(k));
    match next_key {
        Some(key) => {
            if let Some(item) = doc.get_mut(key) {
                let existing = leading_prefix(item)
                    .and_then(|p| p.as_str().map(str::to_string))
                    .unwrap_or_default();
                set_leading_prefix(item, format!("{prefix}{existing}").into());
            }
        }
        None => {
            let existing = doc.trailing().as_str().unwrap_or("").to_string();
            doc.set_trailing(format!("{existing}{prefix}"));
        }
    }
}

impl MapConfig {
    pub fn load(root: &Path) -> Result<MapConfig> {
        let path = root.join(MAP_FILE);
        match std::fs::read_to_string(&path) {
            Ok(s) => Self::parse(&s),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(MapConfig::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn parse(s: &str) -> Result<MapConfig> {
        toml::from_str(s).map_err(|e| Error::Config(e.to_string()))
    }

    /// Pinned at file level (a pin with a symbol also pins the file).
    pub fn is_pinned(&self, path: &str) -> bool {
        self.pin.iter().any(|t| t.path == path)
    }

    /// The human (`agent == false`) or agent note on `path` / `path::symbol`.
    pub fn note_on(&self, path: &str, symbol: Option<&str>, agent: bool) -> Option<&Note> {
        self.note
            .iter()
            .find(|n| n.path == path && n.symbol.as_deref() == symbol && n.is_agent() == agent)
    }

    /// Exact path, or a directory prefix when the exclude path ends with '/'.
    pub fn is_excluded(&self, path: &str) -> bool {
        self.exclude.iter().any(|t| {
            if t.path.ends_with('/') {
                path.starts_with(&t.path)
            } else {
                t.path == path
            }
        })
    }

    pub fn deny_patterns(&self) -> Vec<String> {
        BUILTIN_DENY
            .iter()
            .map(|s| s.to_string())
            .chain(self.deny.extra_patterns.iter().cloned())
            .collect()
    }

    /// Spec §3: every path repo-relative; `deny.extra_patterns` may only grow.
    pub fn validate(&self, current_extras: &[String]) -> std::result::Result<(), MapConfigError> {
        for (i, t) in self.pin.iter().enumerate() {
            check_path(&format!("pin[{i}].path"), &t.path)?;
        }
        for (i, t) in self.exclude.iter().enumerate() {
            check_path(&format!("exclude[{i}].path"), &t.path)?;
        }
        let mut seen_notes: std::collections::HashSet<(String, Option<String>, bool)> =
            std::collections::HashSet::new();
        for (i, n) in self.note.iter().enumerate() {
            check_path(&format!("note[{i}].path"), &n.path)?;
            let err = |f: &str, message: String| MapConfigError {
                field: format!("note[{i}].{f}"),
                message,
            };
            match n.by.as_deref() {
                None | Some(AGENT) => {}
                Some(other) => {
                    return Err(err(
                        "by",
                        format!("by must be absent or \"{AGENT}\", not {other:?}"),
                    ))
                }
            }
            if n.session.is_some() != n.is_agent() || n.at.is_some() != n.is_agent() {
                return Err(err(
                    "by",
                    format!("session and at are present exactly when by = \"{AGENT}\""),
                ));
            }
            if n.text.chars().any(char::is_control) {
                return Err(err(
                    "text",
                    "text is one paragraph: no line breaks or control characters".into(),
                ));
            }
            if n.text.chars().count() > NOTE_MAX_CHARS {
                return Err(err(
                    "text",
                    format!("text is longer than {NOTE_MAX_CHARS} characters"),
                ));
            }
            if !seen_notes.insert((n.path.clone(), n.symbol.clone(), n.is_agent())) {
                let who = if n.is_agent() { "agent" } else { "human" };
                let target = match &n.symbol {
                    Some(s) => format!("{}::{s}", n.path),
                    None => n.path.clone(),
                };
                return Err(err("path", format!("a second {who} note on {target}")));
            }
        }
        for (i, b) in self.boundary.iter().enumerate() {
            for (j, p) in b.paths.iter().enumerate() {
                check_path(&format!("boundary[{i}].paths[{j}]"), p)?;
            }
        }
        let mut seen_names = std::collections::HashSet::new();
        for (i, b) in self.boundary.iter().enumerate() {
            let name = b.name.trim();
            if name.is_empty() {
                return Err(MapConfigError {
                    field: format!("boundary[{i}].name"),
                    message: "boundary name is empty".into(),
                });
            }
            if !seen_names.insert(name.to_string()) {
                return Err(MapConfigError {
                    field: format!("boundary[{i}].name"),
                    message: format!("duplicate boundary name {name}"),
                });
            }
        }
        if let Some(missing) = current_extras
            .iter()
            .find(|p| !self.deny.extra_patterns.contains(p))
        {
            return Err(MapConfigError {
                field: "deny.extra_patterns".into(),
                message: format!("deny patterns may only be added; {missing} was removed"),
            });
        }
        Ok(())
    }

    /// Edit the existing document in place so comments, blank lines, key order and keys
    /// the page does not own survive; only the owned sections are replaced. Written via a
    /// temp file and rename so a reader never sees a torn file.
    pub fn save_atomic(&self, root: &Path) -> Result<()> {
        let path = root.join(MAP_FILE);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let existing = match std::fs::read_to_string(&path) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        let mut doc: toml_edit::DocumentMut = existing
            .as_deref()
            .unwrap_or(MAP_HEADER)
            .parse()
            .map_err(|e: toml_edit::TomlError| Error::Config(e.to_string()))?;
        let fresh = toml_edit::ser::to_string_pretty(self)
            .map_err(|e| Error::Config(e.to_string()))?
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e: toml_edit::TomlError| Error::Config(e.to_string()))?;

        for key in OWNED_KEYS {
            let (old_prefix, old_pos, old_was_value) = {
                let old = doc.get(key);
                (
                    old.and_then(leading_prefix),
                    old.and_then(item_position),
                    matches!(old, Some(toml_edit::Item::Value(_))),
                )
            };

            match fresh.get(key) {
                Some(item) => {
                    let mut item = item.clone();
                    if let Some(p) = old_prefix {
                        set_leading_prefix(&mut item, p);
                    }
                    let pos = match old_pos {
                        Some(p) => p,
                        None if old_was_value => min_table_position(&doc)
                            .map(|m| m.saturating_sub(1))
                            .unwrap_or(0),
                        None => max_table_position(&doc).map(|m| m + 1).unwrap_or(1),
                    };
                    set_item_position(&mut item, pos);
                    // A key that used to hold a plain value (`pin = []`) carries key-level
                    // decor shaped for `key = value` formatting. Reusing that slot for a table
                    // or array-of-tables leaves stray formatting around the new `[[key]]`
                    // header, so drop the old key first and let the assignment recreate it.
                    if old_was_value && !item.is_value() {
                        doc.remove(key);
                    }
                    doc[key] = item;
                }
                None => {
                    doc.remove(key);
                    if let Some(prefix) = old_prefix.and_then(|p| p.as_str().map(str::to_string)) {
                        if !prefix.trim().is_empty() {
                            carry_removed_comment(&mut doc, key, &prefix);
                        }
                    }
                }
            }
        }

        // A brand-new file was seeded by parsing MAP_HEADER alone (no key follows it), so
        // toml_edit has nothing to attach the comment to as a leading prefix and stores it as
        // the document's trailing content instead — which always renders last. Move it back to
        // the document's own leading decor so it renders first, ahead of every table just added.
        if existing.is_none() && doc.trailing().as_str() == Some(MAP_HEADER) {
            doc.set_trailing("");
            doc.decor_mut().set_prefix(MAP_HEADER);
        }

        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, doc.to_string())?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

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
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[[pin]]
path = "src/auth/session.ts"

[[pin]]
path = "src/http/middleware.ts"
symbol = "requireSession"

[[exclude]]
path = "src/legacy/"

[[note]]
path = "src/auth/session.ts"
text = "Auth boundary. Sessions are created here only."

[[boundary]]
name = "auth"
paths = ["src/auth/", "src/http/middleware.ts"]

[deny]
extra_patterns = ["*.snap"]
"#;

    #[test]
    fn parses_sample() {
        let c = MapConfig::parse(SAMPLE).unwrap();
        assert_eq!(c.pin.len(), 2);
        assert_eq!(c.pin[1].symbol.as_deref(), Some("requireSession"));
        assert_eq!(c.exclude[0].path, "src/legacy/");
        assert_eq!(c.boundary[0].paths.len(), 2);
        assert!(c.is_pinned("src/auth/session.ts"));
        assert!(!c.is_pinned("src/util/log.ts"));
    }

    #[test]
    fn excluded_matches_exact_and_directory_prefix() {
        let c = MapConfig::parse(SAMPLE).unwrap();
        assert!(c.is_excluded("src/legacy/old.ts"));
        assert!(!c.is_excluded("src/legacyish.ts"));
        assert!(!c.is_excluded("src/auth/session.ts"));
    }

    #[test]
    fn deny_patterns_include_builtins_plus_extras() {
        let c = MapConfig::parse(SAMPLE).unwrap();
        let d = c.deny_patterns();
        for b in BUILTIN_DENY {
            assert!(d.iter().any(|p| p == b), "builtin {b} missing");
        }
        assert!(d.iter().any(|p| p == "*.snap"));
    }

    #[test]
    fn missing_file_is_default() {
        let dir = tempfile::tempdir().unwrap();
        let c = MapConfig::load(dir.path()).unwrap();
        assert_eq!(c, MapConfig::default());
        assert_eq!(c.deny_patterns().len(), BUILTIN_DENY.len());
    }

    #[test]
    fn invalid_toml_is_config_error() {
        let err = MapConfig::parse("[[pin]\npath = 1").unwrap_err();
        assert!(matches!(err, crate::Error::Config(_)));
    }

    fn cfg(toml_src: &str) -> MapConfig {
        MapConfig::parse(toml_src).unwrap()
    }

    #[test]
    fn validate_accepts_repo_relative_paths() {
        let c = cfg("[[pin]]\npath = \"src/a.ts\"\n[[exclude]]\npath = \"src/legacy/\"\n[[note]]\npath = \"src/a.ts\"\nsymbol = \"f\"\ntext = \"x\"\n[[boundary]]\nname = \"auth\"\npaths = [\"src/auth/\"]\n");
        assert!(c.validate(&[]).is_ok());
    }

    #[test]
    fn validate_rejects_absolute_dotdot_and_dot_slash() {
        for (bad, field) in [
            ("[[pin]]\npath = \"/etc/passwd\"\n", "pin[0].path"),
            ("[[exclude]]\npath = \"../x\"\n", "exclude[0].path"),
            (
                "[[note]]\npath = \"src/../../x\"\ntext = \"t\"\n",
                "note[0].path",
            ),
            ("[[pin]]\npath = \"./src/a.ts\"\n", "pin[0].path"),
            (
                "[[boundary]]\nname = \"b\"\npaths = [\"src/ok\", \"C:\\\\x\"]\n",
                "boundary[0].paths[1]",
            ),
            ("[[pin]]\npath = \"src\\\\a.ts\"\n", "pin[0].path"),
        ] {
            let err = cfg(bad).validate(&[]).unwrap_err();
            assert_eq!(err.field, field, "{bad}");
        }
    }

    #[test]
    fn validate_rejects_a_shrunk_deny_list() {
        let c = cfg("[deny]\nextra_patterns = [\"*.snap\"]\n");
        assert!(c.validate(&["*.snap".to_string()]).is_ok());
        assert!(c.validate(&[]).is_ok(), "growing is fine");
        let err = c
            .validate(&["*.snap".to_string(), "*.lock".to_string()])
            .unwrap_err();
        assert_eq!(err.field, "deny.extra_patterns");
        assert!(err.message.contains("*.lock"));
    }

    #[test]
    fn check_roots_accepts_root_prefixes_and_rejects_unknown_ones() {
        let cfg = MapConfig::parse(
            "[[pin]]\npath = \"app/src/a.ts\"\n[[exclude]]\npath = \"vault/old/\"\n",
        )
        .unwrap();
        cfg.check_roots(&["app", "vault"]).unwrap();
        cfg.check_roots(&[]).unwrap();
        let bad = MapConfig::parse("[[pin]]\npath = \"src/a.ts\"\n").unwrap();
        let e = bad.check_roots(&["app", "vault"]).unwrap_err().to_string();
        assert!(e.contains("pin[0].path") && e.contains("app, vault"), "{e}");
        let note = MapConfig::parse("[[note]]\npath = \"x/a.md\"\ntext = \"t\"\n").unwrap();
        assert!(note.check_roots(&["app"]).is_err());
        let boundary =
            MapConfig::parse("[[boundary]]\nname = \"b\"\npaths = [\"app/src/\", \"nope/\"]\n")
                .unwrap();
        let e = boundary.check_roots(&["app"]).unwrap_err().to_string();
        assert!(e.contains("boundary[0].paths[1]"), "{e}");
    }

    #[test]
    fn save_atomic_keeps_comments_between_sections_and_unknown_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(MAP_FILE);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "# mine, at the top\ncustom = \"keep me\"\npin = []\n\n# between sections\n[[exclude]]\npath = \"src/legacy/\"\n\n[deny]\nextra_patterns = []\n# at the end\n").unwrap();
        let c = cfg("[[pin]]\npath = \"src/a.ts\"\n[[exclude]]\npath = \"src/legacy/\"\n");
        c.save_atomic(dir.path()).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.starts_with("# mine, at the top"), "{written}");
        assert!(
            !written.contains("Managed by singularrag"),
            "the header is only written to a new file"
        );
        assert!(written.contains("# between sections"), "{written}");
        assert!(written.contains("custom = \"keep me\""), "{written}");
        assert!(written.contains("# at the end"), "{written}");
        assert!(written.contains("[[pin]]"), "{written}");
        assert_eq!(MapConfig::load(dir.path()).unwrap(), c);
        assert!(!dir.path().join(".singularrag/map.toml.tmp").exists());
    }

    #[test]
    fn save_atomic_writes_the_header_only_to_a_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let c = cfg("[[pin]]\npath = \"src/a.ts\"\n");
        c.save_atomic(dir.path()).unwrap();
        let written = std::fs::read_to_string(dir.path().join(MAP_FILE)).unwrap();
        assert!(written.starts_with(MAP_HEADER), "{written}");
        assert_eq!(MapConfig::load(dir.path()).unwrap(), c);
        // A second save must not duplicate the header.
        c.save_atomic(dir.path()).unwrap();
        let again = std::fs::read_to_string(dir.path().join(MAP_FILE)).unwrap();
        assert_eq!(
            again.matches("Managed by singularrag").count(),
            1,
            "{again}"
        );
    }

    #[test]
    fn save_atomic_replaces_every_owned_section_even_when_emptied() {
        let dir = tempfile::tempdir().unwrap();
        cfg("[[pin]]\npath = \"src/a.ts\"\n[[note]]\npath = \"src/a.ts\"\ntext = \"n\"\n")
            .save_atomic(dir.path())
            .unwrap();
        MapConfig::default().save_atomic(dir.path()).unwrap();
        let loaded = MapConfig::load(dir.path()).unwrap();
        assert!(
            loaded.pin.is_empty() && loaded.note.is_empty(),
            "{loaded:?}"
        );
    }

    #[test]
    fn save_atomic_round_trips_with_header_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let c =
            cfg("[[pin]]\npath = \"src/a.ts\"\n[[note]]\npath = \"src/a.ts\"\ntext = \"hello\"\n");
        c.save_atomic(dir.path()).unwrap();
        let written = std::fs::read_to_string(dir.path().join(MAP_FILE)).unwrap();
        assert!(written.starts_with(MAP_HEADER), "{written}");
        assert!(!dir.path().join(".singularrag/map.toml.tmp").exists());
        assert_eq!(MapConfig::load(dir.path()).unwrap(), c);
        // Overwrite works too.
        let c2 = cfg("[[exclude]]\npath = \"src/legacy/\"\n");
        c2.save_atomic(dir.path()).unwrap();
        assert_eq!(MapConfig::load(dir.path()).unwrap(), c2);
    }

    #[test]
    fn save_atomic_keeps_file_order_and_comment_when_sections_precede_the_owned_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(MAP_FILE);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // Note: `pin` is written as a pre-existing `[[pin]]` array-of-tables, not a bare
        // `pin = []` value. TOML has no way to place a bare root key like `pin = []` after a
        // `[deny]` header (all bare root keys must precede every table header), so a bare-value
        // form of this fixture would actually define `deny.pin`, not a top-level `pin` at all.
        // `[[pin]]` is what the Critical finding's second cause is really about: a pre-existing
        // array-of-tables whose position must survive the rewrite.
        std::fs::write(
            &path,
            "[deny]\nextra_patterns = [\"*.snap\"]\n\n# user comment before pin\n[[pin]]\npath = \"src/old.ts\"\n",
        )
        .unwrap();
        let c = cfg("[[pin]]\npath = \"src/a.ts\"\n[deny]\nextra_patterns = [\"*.snap\"]\n");
        c.save_atomic(dir.path()).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("# user comment before pin"), "{written}");
        let i_deny = written.find("[deny]").expect("deny table");
        let i_pin = written.find("[[pin]]").expect("pin table");
        assert!(i_deny < i_pin, "{written}");
        assert_eq!(MapConfig::load(dir.path()).unwrap(), c);
    }

    #[test]
    fn save_atomic_keeps_a_value_section_before_later_tables_when_it_becomes_tables() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(MAP_FILE);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "custom = \"x\"\npin = []\n\n[deny]\nextra_patterns = []\n",
        )
        .unwrap();
        let c = cfg("[[pin]]\npath = \"src/a.ts\"\n");
        c.save_atomic(dir.path()).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        let i_custom = written.find("custom = \"x\"").expect("custom key");
        let i_pin = written.find("[[pin]]").expect("pin table");
        let i_deny = written.find("[deny]").expect("deny table");
        assert!(i_custom < i_pin, "{written}");
        assert!(i_pin < i_deny, "{written}");
        assert_eq!(MapConfig::load(dir.path()).unwrap(), c);
    }

    #[test]
    fn save_atomic_appends_a_brand_new_section_after_existing_tables() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(MAP_FILE);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "[deny]\nextra_patterns = []\n").unwrap();
        let c = cfg("[[pin]]\npath = \"src/a.ts\"\n");
        c.save_atomic(dir.path()).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        let i_deny = written.find("[deny]").expect("deny table");
        let i_pin = written.find("[[pin]]").expect("pin table");
        assert!(i_deny < i_pin, "{written}");
        assert_eq!(MapConfig::load(dir.path()).unwrap(), c);
    }

    #[test]
    fn save_atomic_keeps_the_leading_comment_of_a_removed_section() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(MAP_FILE);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "# keep me\n[[pin]]\npath = \"src/a.ts\"\n\n[deny]\nextra_patterns = []\n",
        )
        .unwrap();
        MapConfig::default().save_atomic(dir.path()).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("# keep me"), "{written}");
        assert_eq!(MapConfig::load(dir.path()).unwrap(), MapConfig::default());
    }

    #[test]
    fn boundary_names_must_be_unique_and_non_empty() {
        let c = cfg("[[boundary]]\nname = \"auth\"\npaths = [\"src/a\"]\n[[boundary]]\nname = \"auth\"\npaths = [\"src/b\"]\n");
        let e = c.validate(&[]).unwrap_err();
        assert_eq!(e.field, "boundary[1].name");
        assert!(e.message.contains("duplicate"));
        let c = cfg("[[boundary]]\nname = \"  \"\npaths = [\"src/a\"]\n");
        let e = c.validate(&[]).unwrap_err();
        assert_eq!(e.field, "boundary[0].name");
        assert!(e.message.contains("empty"));
    }

    #[test]
    fn a_note_round_trips_its_author_keys() {
        let text = "[[note]]\npath = \"src/a.ts\"\nsymbol = \"f\"\ntext = \"entry point\"\nby = \"agent\"\nsession = \"mcp:claude-code:1:2\"\nat = \"2026-09-21T09:14:02Z\"\n[[note]]\npath = \"src/a.ts\"\ntext = \"human\"\n";
        let c = cfg(text);
        assert!(c.validate(&[]).is_ok());
        assert!(c.note[0].is_agent());
        assert!(!c.note[1].is_agent());
        assert_eq!(c.note[0].session.as_deref(), Some("mcp:claude-code:1:2"));
        assert_eq!(c.note[0].at.as_deref(), Some("2026-09-21T09:14:02Z"));
        assert_eq!(
            c.note_on("src/a.ts", Some("f"), true)
                .map(|n| n.text.as_str()),
            Some("entry point")
        );
        assert_eq!(
            c.note_on("src/a.ts", None, false).map(|n| n.text.as_str()),
            Some("human")
        );
        assert!(c.note_on("src/a.ts", None, true).is_none());
        let out = toml_edit::ser::to_string_pretty(&c).unwrap();
        assert!(out.contains("by = \"agent\""), "{out}");
        assert!(
            !out.contains("by = \"\""),
            "a human note has no by key: {out}"
        );
        assert_eq!(cfg(&out), c);
    }

    #[test]
    fn validate_enforces_the_note_rules() {
        let base = "[[note]]\npath = \"src/a.ts\"\ntext = \"one\"\n";
        let field = |t: &str| cfg(t).validate(&[]).unwrap_err().field;
        // a second human note on the same target
        assert_eq!(
            field(&format!(
                "{base}[[note]]\npath = \"src/a.ts\"\ntext = \"two\"\n"
            )),
            "note[1].path"
        );
        // an agent note beside a human one is fine; a second agent note is not
        let agent = "[[note]]\npath = \"src/a.ts\"\ntext = \"a\"\nby = \"agent\"\nsession = \"s\"\nat = \"t\"\n";
        assert!(cfg(&format!("{base}{agent}")).validate(&[]).is_ok());
        assert_eq!(field(&format!("{base}{agent}{agent}")), "note[2].path");
        // by must be absent or "agent"; session/at go with by
        assert_eq!(
            field("[[note]]\npath = \"src/a.ts\"\ntext = \"x\"\nby = \"bot\"\n"),
            "note[0].by"
        );
        assert_eq!(
            field("[[note]]\npath = \"src/a.ts\"\ntext = \"x\"\nby = \"agent\"\n"),
            "note[0].by"
        );
        assert_eq!(
            field("[[note]]\npath = \"src/a.ts\"\ntext = \"x\"\nsession = \"s\"\nat = \"t\"\n"),
            "note[0].by"
        );
        // a human note with only one of session/at is still wrong, not just both
        assert_eq!(
            field("[[note]]\npath = \"src/a.ts\"\ntext = \"x\"\nsession = \"s\"\n"),
            "note[0].by"
        );
        assert_eq!(
            field("[[note]]\npath = \"src/a.ts\"\ntext = \"x\"\nat = \"t\"\n"),
            "note[0].by"
        );
        // an agent note missing just one of session/at is also wrong
        assert_eq!(
            field("[[note]]\npath = \"src/a.ts\"\ntext = \"x\"\nby = \"agent\"\nsession = \"s\"\n"),
            "note[0].by"
        );
        assert_eq!(
            field("[[note]]\npath = \"src/a.ts\"\ntext = \"x\"\nby = \"agent\"\nat = \"t\"\n"),
            "note[0].by"
        );
        // one paragraph, at most 300 characters
        assert_eq!(
            field("[[note]]\npath = \"src/a.ts\"\ntext = \"two\\nlines\"\n"),
            "note[0].text"
        );
        let long = "x".repeat(301);
        assert_eq!(
            field(&format!(
                "[[note]]\npath = \"src/a.ts\"\ntext = \"{long}\"\n"
            )),
            "note[0].text"
        );
        let ok = "x".repeat(300);
        assert!(
            cfg(&format!("[[note]]\npath = \"src/a.ts\"\ntext = \"{ok}\"\n"))
                .validate(&[])
                .is_ok()
        );
    }

    #[test]
    fn note_text_is_normalised_to_one_paragraph() {
        assert_eq!(normalise_note_text("  a\n\n b\t c  "), "a b c");
        assert_eq!(normalise_note_text("\u{0}x\r\ny"), "x y");
        assert_eq!(normalise_note_text("   "), "");
    }
}
