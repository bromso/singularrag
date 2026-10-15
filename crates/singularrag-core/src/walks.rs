//! Walk files: scripted question/answer "walks" of four kinds (locate, process,
//! connection, change), parsed from TOML and validated before use.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WalkKind {
    Locate,
    Process,
    Connection,
    Change,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Walk {
    pub id: String,
    pub kind: WalkKind,
    #[serde(default)]
    pub query: Option<String>,
    pub gold: Vec<String>,
    #[serde(default)]
    pub budget: Option<usize>, // locate only
    #[serde(default)]
    pub role: Option<String>, // process only
    #[serde(default)]
    pub step_text: Option<String>, // process only
    #[serde(default)]
    pub from: Option<String>, // connection only
    #[serde(default)]
    pub hops: Option<u32>, // connection only
    #[serde(default)]
    pub patch: Option<String>, // change only, relative to the walk file
}

#[derive(Debug, Clone, Deserialize)]
pub struct WalkFile {
    #[serde(default)]
    pub budget: Option<usize>,
    pub walk: Vec<Walk>,
}

pub const DEFAULT_HOPS: u32 = 2;

impl Walk {
    pub fn hops(&self) -> u32 {
        self.hops.unwrap_or(DEFAULT_HOPS)
    }

    pub fn budget_or(&self, default: usize) -> usize {
        self.budget.unwrap_or(default)
    }
}

/// Validate the rules a `WalkFile` must satisfy beyond what TOML deserialization
/// already enforces: unique ids, non-empty `path::name` gold, the field required
/// by each walk's kind, and that no walk carries a field belonging to another kind.
pub fn validate(file: &WalkFile) -> Result<()> {
    let mut seen: Vec<&str> = Vec::new();
    for w in &file.walk {
        if seen.contains(&w.id.as_str()) {
            return Err(Error::Config(format!("duplicate id {}", w.id)));
        }
        seen.push(&w.id);

        if w.gold.is_empty() {
            return Err(Error::Config(format!("{}: gold is empty", w.id)));
        }
        for g in &w.gold {
            if !g.contains("::") {
                return Err(Error::Config(format!(
                    "{}: gold \"{g}\" is not path::name",
                    w.id
                )));
            }
        }

        match w.kind {
            WalkKind::Locate => {
                if w.query.is_none() {
                    return Err(Error::Config(format!("{}: locate needs query", w.id)));
                }
            }
            WalkKind::Process => {
                if w.query.is_none() {
                    return Err(Error::Config(format!("{}: process needs query", w.id)));
                }
            }
            WalkKind::Connection => {
                if w.from.is_none() {
                    return Err(Error::Config(format!("{}: connection needs from", w.id)));
                }
            }
            WalkKind::Change => {
                if w.patch.is_none() {
                    return Err(Error::Config(format!("{}: change needs patch", w.id)));
                }
            }
        }

        if w.budget.is_some() && w.kind != WalkKind::Locate {
            return Err(Error::Config(format!(
                "{}: budget belongs to locate walks only",
                w.id
            )));
        }
        if (w.role.is_some() || w.step_text.is_some()) && w.kind != WalkKind::Process {
            return Err(Error::Config(format!(
                "{}: role/step_text belongs to process walks only",
                w.id
            )));
        }
        if (w.from.is_some() || w.hops.is_some()) && w.kind != WalkKind::Connection {
            return Err(Error::Config(format!(
                "{}: hops belongs to connection walks only",
                w.id
            )));
        }
        if w.patch.is_some() && w.kind != WalkKind::Change {
            return Err(Error::Config(format!(
                "{}: patch belongs to change walks only",
                w.id
            )));
        }
    }
    Ok(())
}

/// Read a walk file from `path`, parse it, and validate it.
pub fn load_walks(path: &Path) -> Result<WalkFile> {
    let s = std::fs::read_to_string(path)?;
    let f: WalkFile =
        toml::from_str(&s).map_err(|e| Error::Config(format!("{}: {e}", path.display())))?;
    validate(&f)?;
    Ok(f)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Result<WalkFile> {
        let f: WalkFile = toml::from_str(s).map_err(|e| Error::Config(e.to_string()))?;
        validate(&f)?;
        Ok(f)
    }

    #[test]
    fn a_valid_file_of_all_four_kinds_parses() {
        let f = parse(
            r#"
budget = 2048
[[walk]]
id = "W1"
kind = "locate"
query = "how do I get reimbursed"
gold = ["docs/handbook.md::Expense process"]
[[walk]]
id = "W2"
kind = "process"
query = "what happens after approval"
role = "Finance"
step_text = "within 5 days"
gold = ["docs/handbook.md::Expense process"]
[[walk]]
id = "W3"
kind = "connection"
from = "Okta"
gold = ["docs/handbook.md::Onboarding"]
[[walk]]
id = "W4"
kind = "change"
patch = "patches/W4.patch"
gold = ["src/auth/session.ts::createSession"]
"#,
        )
        .unwrap();
        assert_eq!(f.walk.len(), 4);
        assert_eq!(f.walk[0].budget_or(f.budget.unwrap()), 2048);
        assert_eq!(f.walk[2].hops(), DEFAULT_HOPS);
        assert_eq!(f.walk[3].kind, WalkKind::Change);
    }

    #[test]
    fn missing_required_fields_are_errors() {
        for (body, msg) in [
            (
                "id = \"A\"\nkind = \"locate\"\ngold = [\"x::y\"]",
                "A: locate needs query",
            ),
            (
                "id = \"A\"\nkind = \"process\"\ngold = [\"x::y\"]",
                "A: process needs query",
            ),
            (
                "id = \"A\"\nkind = \"connection\"\ngold = [\"x::y\"]",
                "A: connection needs from",
            ),
            (
                "id = \"A\"\nkind = \"change\"\ngold = [\"x::y\"]",
                "A: change needs patch",
            ),
            (
                "id = \"A\"\nkind = \"locate\"\nquery = \"q\"\ngold = []",
                "A: gold is empty",
            ),
            (
                "id = \"A\"\nkind = \"locate\"\nquery = \"q\"\ngold = [\"no-separator\"]",
                "A: gold \"no-separator\" is not path::name",
            ),
        ] {
            let e = parse(&format!("[[walk]]\n{body}\n"))
                .unwrap_err()
                .to_string();
            assert!(e.contains(msg), "{e}");
        }
    }

    #[test]
    fn duplicate_ids_and_unknown_kinds_are_errors() {
        let e = parse("[[walk]]\nid = \"A\"\nkind = \"locate\"\nquery = \"q\"\ngold = [\"x::y\"]\n[[walk]]\nid = \"A\"\nkind = \"locate\"\nquery = \"q\"\ngold = [\"x::y\"]\n").unwrap_err().to_string();
        assert!(e.contains("duplicate id A"), "{e}");
        let e =
            parse("[[walk]]\nid = \"A\"\nkind = \"browse\"\nquery = \"q\"\ngold = [\"x::y\"]\n")
                .unwrap_err()
                .to_string();
        assert!(e.contains("browse"), "{e}");
    }

    #[test]
    fn fields_that_do_not_belong_to_the_kind_are_errors() {
        let e = parse("[[walk]]\nid = \"A\"\nkind = \"process\"\nquery = \"q\"\nbudget = 512\ngold = [\"x::y\"]\n").unwrap_err().to_string();
        assert!(e.contains("A: budget belongs to locate walks only"), "{e}");
        let e = parse(
            "[[walk]]\nid = \"A\"\nkind = \"locate\"\nquery = \"q\"\nhops = 3\ngold = [\"x::y\"]\n",
        )
        .unwrap_err()
        .to_string();
        assert!(
            e.contains("A: hops belongs to connection walks only"),
            "{e}"
        );
    }
}
