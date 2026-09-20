//! Authored map: `.singularrag/map.toml`. Committed to the repo; the UI edits it.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

pub const MAP_FILE: &str = ".singularrag/map.toml";

pub const MAP_HEADER: &str =
    "# Managed by singularrag serve. Hand edits are kept; comments are not.\n";

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
    #[serde(default)]
    pub pin: Vec<Target>,
    #[serde(default)]
    pub exclude: Vec<Target>,
    #[serde(default)]
    pub note: Vec<Note>,
    #[serde(default)]
    pub boundary: Vec<Boundary>,
    #[serde(default)]
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

    /// Write `map.toml` via a temp file and rename so a reader never sees a torn file.
    pub fn save_atomic(&self, root: &Path) -> Result<()> {
        let path = root.join(MAP_FILE);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = toml::to_string_pretty(self).map_err(|e| Error::Config(e.to_string()))?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, format!("{MAP_HEADER}{body}"))?;
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
