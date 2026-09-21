//! `eval/tier2.toml` (spec §3): the checkout, the pinned commit, the conditions, the caps.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::repo;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Condition {
    pub name: String,
    #[serde(default)]
    pub baseline: bool,
    /// Claude Code `mcpServers` JSON; absolute after `load`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_config: Option<PathBuf>,
    /// Argv run once before this condition's loop, `<checkout>` substituted (spec §3).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warmup: Vec<String>,
    /// Checkout-relative paths deleted before every session of this condition (spec §3).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reset: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunConfig {
    pub repo: PathBuf,
    pub commit: String,
    #[serde(default = "default_questions")]
    pub questions: PathBuf,
    #[serde(default = "default_repeats")]
    pub repeats: u32,
    #[serde(default = "default_max_turns")]
    pub max_turns: u32,
    #[serde(default = "default_max_budget")]
    pub max_budget_usd: f64,
    #[serde(default = "default_tools")]
    pub tools: Vec<String>,
    #[serde(default = "default_answer_max")]
    pub answer_max: usize,
    #[serde(rename = "condition")]
    pub conditions: Vec<Condition>,
}

fn default_questions() -> PathBuf {
    PathBuf::from("questions.toml")
}
fn default_repeats() -> u32 {
    3
}
fn default_max_turns() -> u32 {
    25
}
fn default_max_budget() -> f64 {
    0.5
}
fn default_tools() -> Vec<String> {
    ["Read", "Grep", "Glob"].map(String::from).to_vec()
}
fn default_answer_max() -> usize {
    15
}

fn absolutise(base: &Path, p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        normalise_dots(&base.join(p))
    }
}

/// Collapse `.` and `..` components without touching the filesystem (the checkout may not exist yet).
fn normalise_dots(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

pub fn load(path: &Path) -> Result<RunConfig> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut cfg: RunConfig =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    let base = path.parent().unwrap_or(Path::new("."));
    let base = if base.as_os_str().is_empty() {
        Path::new(".")
    } else {
        base
    };
    let base = std::fs::canonicalize(base).unwrap_or_else(|_| base.to_path_buf());
    cfg.repo = absolutise(&base, &cfg.repo);
    cfg.questions = absolutise(&base, &cfg.questions);
    let baselines = cfg.conditions.iter().filter(|c| c.baseline).count();
    if baselines != 1 {
        bail!("exactly one condition must be the baseline (found {baselines})");
    }
    let mut seen = std::collections::BTreeSet::new();
    for c in &mut cfg.conditions {
        if !seen.insert(c.name.clone()) {
            bail!("duplicate condition name {}", c.name);
        }
        if let Some(m) = &c.mcp_config {
            let abs = absolutise(&base, m);
            if !abs.is_file() {
                bail!(
                    "condition {}: mcp_config not found: {}",
                    c.name,
                    abs.display()
                );
            }
            c.mcp_config = Some(abs);
        }
        for entry in &c.reset {
            // A prefix check alone lets `.singularrag/../../x` through, and `remove_dir_all`
            // would follow it out of the checkout.
            let climbs = Path::new(entry).components().any(|c| {
                !matches!(
                    c,
                    std::path::Component::Normal(_) | std::path::Component::CurDir
                )
            });
            if climbs {
                bail!(
                    "condition {}: reset path {entry} must stay inside the checkout",
                    c.name
                );
            }
            let under_ignored = repo::IGNORED_PREFIXES
                .iter()
                .any(|pre| entry.starts_with(pre) || format!("{entry}/").starts_with(pre));
            if !under_ignored {
                bail!(
                    "condition {}: reset path {entry} is not under an ignored prefix",
                    c.name
                );
            }
        }
    }
    if cfg.repeats == 0 {
        bail!("repeats must be at least 1");
    }
    if cfg.answer_max == 0 {
        bail!("answer_max must be at least 1");
    }
    Ok(cfg)
}

impl RunConfig {
    pub fn baseline(&self) -> &Condition {
        self.conditions
            .iter()
            .find(|c| c.baseline)
            .expect("validated in load")
    }

    /// The conditions to run, in config order; `names` narrows and must all exist.
    pub fn select(&self, names: Option<&[String]>) -> Result<Vec<Condition>> {
        match names {
            None => Ok(self.conditions.clone()),
            Some(names) => {
                for n in names {
                    if !self.conditions.iter().any(|c| &c.name == n) {
                        bail!("unknown condition {n}");
                    }
                }
                Ok(self
                    .conditions
                    .iter()
                    .filter(|c| names.contains(&c.name))
                    .cloned()
                    .collect())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
        let p = dir.join("tier2.toml");
        std::fs::write(&p, body).unwrap();
        p
    }

    const GOOD: &str = r#"
repo = "../hono"
commit = "098e11912ab244c5c33931de007f04dc8e3c2929"
[[condition]]
name = "alone"
baseline = true
[[condition]]
name = "singularrag"
mcp_config = "conditions/singularrag.json"
"#;

    #[test]
    fn paths_resolve_relative_to_the_config_file_and_defaults_apply() {
        let dir = tempfile::tempdir().unwrap();
        // Canonical: `load` canonicalises the config directory, and macOS temp dirs sit behind /private.
        let base = std::fs::canonicalize(dir.path()).unwrap();
        let sub = base.join("eval");
        std::fs::create_dir_all(sub.join("conditions")).unwrap();
        std::fs::write(sub.join("conditions/singularrag.json"), "{}").unwrap();
        let cfg = load(&write(&sub, GOOD)).unwrap();
        assert_eq!(cfg.repo, base.join("hono"));
        assert_eq!(cfg.questions, sub.join("questions.toml"));
        assert_eq!(cfg.repeats, 3);
        assert_eq!(cfg.max_turns, 25);
        assert!((cfg.max_budget_usd - 0.5).abs() < 1e-9);
        assert_eq!(cfg.tools, vec!["Read", "Grep", "Glob"]);
        assert_eq!(cfg.answer_max, 15);
        assert_eq!(
            cfg.conditions[1].mcp_config.as_deref(),
            Some(sub.join("conditions/singularrag.json").as_path())
        );
        assert_eq!(cfg.baseline().name, "alone");
    }

    #[test]
    fn exactly_one_baseline_is_required() {
        let dir = tempfile::tempdir().unwrap();
        let none = GOOD.replace("baseline = true\n", "");
        let e = load(&write(dir.path(), &none)).unwrap_err().to_string();
        assert!(
            e.contains("exactly one condition must be the baseline"),
            "{e}"
        );
        let two = GOOD.replace(
            "mcp_config = \"conditions/singularrag.json\"",
            "baseline = true",
        );
        let e = load(&write(dir.path(), &two)).unwrap_err().to_string();
        assert!(
            e.contains("exactly one condition must be the baseline"),
            "{e}"
        );
    }

    #[test]
    fn duplicate_names_and_missing_mcp_config_files_are_errors() {
        let dir = tempfile::tempdir().unwrap();
        let dup = GOOD.replace("name = \"singularrag\"", "name = \"alone\"");
        let e = load(&write(dir.path(), &dup)).unwrap_err().to_string();
        assert!(e.contains("duplicate condition name alone"), "{e}");
        let e = load(&write(dir.path(), GOOD)).unwrap_err().to_string();
        assert!(e.contains("mcp_config not found"), "{e}");
    }

    #[test]
    fn reset_paths_must_be_under_an_ignored_prefix() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("conditions")).unwrap();
        std::fs::write(dir.path().join("conditions/singularrag.json"), "{}").unwrap();
        let bad = GOOD.replace(
            "mcp_config = \"conditions/singularrag.json\"",
            "mcp_config = \"conditions/singularrag.json\"\nreset = [\"src/other\"]",
        );
        let e = load(&write(dir.path(), &bad)).unwrap_err().to_string();
        assert!(
            e.contains("reset path src/other is not under an ignored prefix"),
            "{e}"
        );
        let good = GOOD.replace(
            "mcp_config = \"conditions/singularrag.json\"",
            "mcp_config = \"conditions/singularrag.json\"\nreset = [\".singularrag/memories\"]",
        );
        assert!(load(&write(dir.path(), &good)).is_ok());
    }

    #[test]
    fn reset_paths_must_not_climb_out_of_the_checkout() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("conditions")).unwrap();
        std::fs::write(dir.path().join("conditions/singularrag.json"), "{}").unwrap();
        for entry in [
            ".singularrag/../../x",
            ".singularrag/./../x",
            "/.singularrag",
        ] {
            let bad = GOOD.replace(
                "mcp_config = \"conditions/singularrag.json\"",
                &format!("mcp_config = \"conditions/singularrag.json\"\nreset = [\"{entry}\"]"),
            );
            let e = load(&write(dir.path(), &bad)).unwrap_err().to_string();
            assert!(
                e.contains(&format!("reset path {entry} must stay inside the checkout")),
                "{entry}: {e}"
            );
        }
    }

    #[test]
    fn select_filters_by_name_and_rejects_unknown() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("conditions")).unwrap();
        std::fs::write(dir.path().join("conditions/singularrag.json"), "{}").unwrap();
        let cfg = load(&write(dir.path(), GOOD)).unwrap();
        let sel = cfg.select(Some(&["singularrag".to_string()])).unwrap();
        assert_eq!(sel.len(), 1);
        assert_eq!(sel[0].name, "singularrag");
        assert_eq!(cfg.select(None).unwrap().len(), 2);
        let e = cfg
            .select(Some(&["nope".to_string()]))
            .unwrap_err()
            .to_string();
        assert!(e.contains("unknown condition nope"), "{e}");
    }
}
