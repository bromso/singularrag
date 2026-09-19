//! Authored map: `.singularrag/map.toml`. Committed to the repo; the UI edits it.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

pub const MAP_FILE: &str = ".singularrag/map.toml";

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
}
