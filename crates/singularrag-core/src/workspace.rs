//! A workspace: the directory that holds `.singularrag/`, and the roots it indexes
//! (documents spec §2). No file, or no `[[root]]`, means the directory is its only root
//! and every path is bare; declared roots prefix every path with `<name>/`.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::{Error, Result};

pub const WORKSPACE_FILE: &str = ".singularrag/workspace.toml";

#[derive(Debug, Clone, PartialEq)]
pub struct Root {
    /// Empty for the single unnamed root.
    pub name: String,
    /// Canonical.
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct Workspace {
    /// Canonical; holds `.singularrag/`.
    pub dir: PathBuf,
    /// In file order; one unnamed root when no file declares any.
    pub roots: Vec<Root>,
}

#[derive(Deserialize)]
struct WorkspaceFile {
    #[serde(default)]
    root: Vec<RootEntry>,
}

#[derive(Deserialize)]
struct RootEntry {
    name: String,
    path: PathBuf,
}

fn valid_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 64
        && n.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

impl Workspace {
    /// The directory as its own single root; no file is read.
    pub fn single(dir: &Path) -> Result<Workspace> {
        let dir = dir
            .canonicalize()
            .map_err(|e| Error::Config(format!("opening workspace at {}: {e}", dir.display())))?;
        Ok(Workspace {
            roots: vec![Root {
                name: String::new(),
                path: dir.clone(),
            }],
            dir,
        })
    }

    pub fn open(dir: &Path) -> Result<Workspace> {
        let ws = Workspace::single(dir)?;
        let file = ws.dir.join(WORKSPACE_FILE);
        let text = match std::fs::read_to_string(&file) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(ws),
            Err(e) => return Err(Error::Config(format!("{}: {e}", file.display()))),
        };
        let parsed: WorkspaceFile =
            toml::from_str(&text).map_err(|e| Error::Config(format!("{}: {e}", file.display())))?;
        if parsed.root.is_empty() {
            return Ok(ws);
        }
        let mut roots: Vec<Root> = Vec::with_capacity(parsed.root.len());
        for (i, r) in parsed.root.iter().enumerate() {
            let field = |f: &str| format!("{}: root[{i}].{f}", file.display());
            if !valid_name(&r.name) {
                return Err(Error::Config(format!(
                    "{}: {:?} must match [A-Za-z0-9_-]{{1,64}}",
                    field("name"),
                    r.name
                )));
            }
            if roots.iter().any(|x| x.name == r.name) {
                return Err(Error::Config(format!(
                    "{}: duplicate root name {}",
                    file.display(),
                    r.name
                )));
            }
            let raw = if r.path.is_absolute() {
                r.path.clone()
            } else {
                ws.dir.join(&r.path)
            };
            let path = raw
                .canonicalize()
                .map_err(|e| Error::Config(format!("{}: {}: {e}", field("path"), raw.display())))?;
            if !path.is_dir() {
                return Err(Error::Config(format!(
                    "{}: {} is not a directory",
                    field("path"),
                    path.display()
                )));
            }
            roots.push(Root {
                name: r.name.clone(),
                path,
            });
        }
        for a in &roots {
            for b in &roots {
                if a.name != b.name && b.path.starts_with(&a.path) {
                    return Err(Error::Config(format!(
                        "{}: root {} contains root {}",
                        file.display(),
                        a.name,
                        b.name
                    )));
                }
            }
        }
        Ok(Workspace { dir: ws.dir, roots })
    }

    pub fn is_named(&self) -> bool {
        self.roots.iter().any(|r| !r.name.is_empty())
    }

    pub fn names(&self) -> Vec<&str> {
        self.roots
            .iter()
            .filter(|r| !r.name.is_empty())
            .map(|r| r.name.as_str())
            .collect()
    }

    pub fn prefix(&self, root: &Root) -> String {
        if root.name.is_empty() {
            String::new()
        } else {
            format!("{}/", root.name)
        }
    }

    /// `<name>/<relative>` (or bare) for a canonical absolute path under a root: the
    /// longest root prefix wins, which matters only when roots are nested, which `open`
    /// refuses; it is still the correct rule.
    pub fn rel_of(&self, abs: &Path) -> Option<String> {
        let mut best: Option<(usize, String)> = None;
        for r in &self.roots {
            if let Ok(rest) = abs.strip_prefix(&r.path) {
                let rest = rest.to_string_lossy().replace('\\', "/");
                if rest.is_empty() {
                    continue;
                }
                let len = r.path.as_os_str().len();
                if best.as_ref().is_none_or(|(l, _)| len > *l) {
                    best = Some((len, format!("{}{rest}", self.prefix(r))));
                }
            }
        }
        best.map(|(_, s)| s)
    }

    pub fn root_of(&self, rel: &str) -> Option<(&Root, String)> {
        if !self.is_named() {
            return Some((&self.roots[0], rel.to_string()));
        }
        let (name, rest) = rel.split_once('/')?;
        let root = self.roots.iter().find(|r| r.name == name)?;
        Some((root, rest.to_string()))
    }

    pub fn abs_of(&self, rel: &str) -> Option<PathBuf> {
        let (root, rest) = self.root_of(rel)?;
        Some(root.path.join(rest))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws_dir() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".singularrag")).unwrap();
        d
    }

    fn write_roots(d: &Path, body: &str) {
        std::fs::write(d.join(WORKSPACE_FILE), body).unwrap();
    }

    #[test]
    fn no_file_means_one_unnamed_root_with_bare_paths() {
        let d = ws_dir();
        std::fs::create_dir_all(d.path().join("src")).unwrap();
        std::fs::write(d.path().join("src/a.ts"), "").unwrap();
        let ws = Workspace::open(d.path()).unwrap();
        assert_eq!(ws.roots.len(), 1);
        assert_eq!(ws.roots[0].name, "");
        assert!(!ws.is_named());
        assert_eq!(ws.prefix(&ws.roots[0]), "");
        let abs = d.path().join("src/a.ts").canonicalize().unwrap();
        assert_eq!(ws.rel_of(&abs).as_deref(), Some("src/a.ts"));
        assert_eq!(ws.abs_of("src/a.ts"), Some(abs));
        assert!(ws.names().is_empty());
    }

    #[test]
    fn declared_roots_prefix_paths_and_map_back_by_longest_prefix() {
        let d = ws_dir();
        let app = tempfile::tempdir().unwrap();
        let vault = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(app.path().join("src")).unwrap();
        std::fs::write(app.path().join("src/a.ts"), "").unwrap();
        std::fs::write(vault.path().join("note.md"), "").unwrap();
        write_roots(
            d.path(),
            &format!(
                "[[root]]\nname = \"app\"\npath = \"{}\"\n[[root]]\nname = \"vault\"\npath = \"{}\"\n",
                app.path().display(),
                vault.path().display()
            ),
        );
        let ws = Workspace::open(d.path()).unwrap();
        assert!(ws.is_named());
        assert_eq!(ws.names(), vec!["app", "vault"]);
        assert_eq!(ws.prefix(&ws.roots[0]), "app/");
        let abs = app.path().join("src/a.ts").canonicalize().unwrap();
        assert_eq!(ws.rel_of(&abs).as_deref(), Some("app/src/a.ts"));
        assert_eq!(
            ws.abs_of("vault/note.md"),
            Some(vault.path().canonicalize().unwrap().join("note.md"))
        );
        assert!(ws.abs_of("nope/x.md").is_none(), "unknown prefix");
        assert!(
            ws.rel_of(d.path()).is_none(),
            "the workspace dir itself is not a root"
        );
        let (root, rest) = ws.root_of("vault/a/b.md").unwrap();
        assert_eq!((root.name.as_str(), rest.as_str()), ("vault", "a/b.md"));
    }

    #[test]
    fn relative_root_paths_resolve_against_the_workspace_dir() {
        let d = ws_dir();
        std::fs::create_dir_all(d.path().join("sub/app")).unwrap();
        write_roots(d.path(), "[[root]]\nname = \"app\"\npath = \"sub/app\"\n");
        let ws = Workspace::open(d.path()).unwrap();
        assert_eq!(
            ws.roots[0].path,
            d.path().join("sub/app").canonicalize().unwrap()
        );
    }

    #[test]
    fn root_paths_normalise_or_name_the_field() {
        let d = ws_dir();
        std::fs::create_dir_all(d.path().join("app")).unwrap();
        write_roots(d.path(), "[[root]]\nname = \"app\"\npath = \"app/\"\n");
        let ws = Workspace::open(d.path()).unwrap();
        assert_eq!(
            ws.roots[0].path,
            d.path().join("app").canonicalize().unwrap(),
            "trailing slash"
        );
        write_roots(d.path(), "[[root]]\nname = \"app\"\npath = \"missing\"\n");
        let e = Workspace::open(d.path()).unwrap_err().to_string();
        assert!(
            e.contains("workspace.toml") && e.contains("root[0].path"),
            "{e}"
        );
        std::fs::write(d.path().join("file.txt"), "").unwrap();
        write_roots(d.path(), "[[root]]\nname = \"f\"\npath = \"file.txt\"\n");
        let e = Workspace::open(d.path()).unwrap_err().to_string();
        assert!(e.contains("not a directory"), "{e}");
    }

    #[test]
    fn bad_names_duplicates_and_nesting_are_errors() {
        let d = ws_dir();
        std::fs::create_dir_all(d.path().join("a/b")).unwrap();
        write_roots(d.path(), "[[root]]\nname = \"bad name\"\npath = \"a\"\n");
        let e = Workspace::open(d.path()).unwrap_err().to_string();
        assert!(e.contains("root[0].name"), "{e}");
        write_roots(
            d.path(),
            "[[root]]\nname = \"x\"\npath = \"a\"\n[[root]]\nname = \"x\"\npath = \"a/b\"\n",
        );
        let e = Workspace::open(d.path()).unwrap_err().to_string();
        assert!(e.contains("duplicate root name x"), "{e}");
        write_roots(
            d.path(),
            "[[root]]\nname = \"a\"\npath = \"a\"\n[[root]]\nname = \"b\"\npath = \"a/b\"\n",
        );
        let e = Workspace::open(d.path()).unwrap_err().to_string();
        assert!(e.contains("root a contains root b"), "{e}");
    }

    #[test]
    fn malformed_file_names_the_file() {
        let d = ws_dir();
        write_roots(d.path(), "[[root]\nname = 1\n");
        let e = Workspace::open(d.path()).unwrap_err().to_string();
        assert!(e.contains("workspace.toml"), "{e}");
    }
}
