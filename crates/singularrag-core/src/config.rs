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

fn check_path(field: &str, p: &str) -> std::result::Result<(), MapConfigError> {
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pin: Vec<Target>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<Target>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub note: Vec<Note>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub boundary: Vec<Boundary>,
    #[serde(default, skip_serializing_if = "Deny::is_empty")]
    pub deny: Deny,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Target {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    pub text: String,
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

impl Deny {
    fn is_empty(&self) -> bool {
        self.extra_patterns.is_empty()
    }
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
        for (i, n) in self.note.iter().enumerate() {
            check_path(&format!("note[{i}].path"), &n.path)?;
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

        // Preserve leading comments/blank lines from existing file
        let leading_content = existing.as_ref().and_then(|text| {
            let mut end = 0;
            for line in text.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    end += line.len() + 1; // +1 for newline
                } else {
                    break;
                }
            }
            if end > 0 {
                Some(text[..end].to_string())
            } else {
                None
            }
        });

        let mut doc: toml_edit::DocumentMut = if let Some(text) = &existing {
            text.parse()
                .map_err(|e: toml_edit::TomlError| Error::Config(e.to_string()))?
        } else {
            toml_edit::DocumentMut::new()
        };

        // Determine the first owned key in the document to avoid removing its prefix
        let first_owned_key = OWNED_KEYS.iter().find(|k| doc.get(k).is_some()).copied();

        let fresh = toml_edit::ser::to_string_pretty(self)
            .map_err(|e| Error::Config(e.to_string()))?
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e: toml_edit::TomlError| Error::Config(e.to_string()))?;

        for key in OWNED_KEYS {
            match fresh.get(key) {
                Some(item) => {
                    // Only remove if there's a type conflict (e.g., inline array vs array-of-tables)
                    let needs_remove = if let Some(existing) = doc.get(key) {
                        (existing.is_array() && item.is_array_of_tables())
                            || (existing.is_array_of_tables() && !item.is_array_of_tables())
                    } else {
                        false
                    };

                    // Preserve the leading decorator (comments before the section),
                    // but only for keys after the first owned key (leading_content handles the first key)
                    let old_prefix = if Some(key) != first_owned_key {
                        doc.get(key).and_then(leading_prefix)
                    } else {
                        None
                    };

                    if needs_remove {
                        doc.remove(key);
                    }
                    doc[key] = item.clone();

                    // Apply the old prefix to the new item
                    if let (Some(p), Some(new_item)) = (old_prefix, doc.get_mut(key)) {
                        set_leading_prefix(new_item, p);
                    }
                }
                None => {
                    doc.remove(key);
                }
            }
        }

        let mut output = doc.to_string();

        // Restore leading content if it exists, otherwise add header for new files
        if let Some(leading) = leading_content {
            if !output.starts_with(&leading) {
                output = format!("{}{}", leading, output);
            }
        } else if existing.is_none() && !output.starts_with(MAP_HEADER) {
            output = format!("{}{}", MAP_HEADER, output);
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, output)?;
        std::fs::rename(&tmp, &path)?;
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
        // Verify that the unknown top-level key stays before the owned sections
        let custom_pos = written.find("custom = \"keep me\"").expect("custom key");
        let pin_pos = written.find("[[pin]]").expect("pin section");
        assert!(
            custom_pos < pin_pos,
            "custom key should appear before [[pin]]"
        );
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
}
