//! Small repos used by tests across crates. Written into a directory at test time
//! so nested `.gitignore` files never interact with this repo's git.

use std::path::Path;

fn w(root: &Path, rel: &str, content: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}

/// Four TS modules with a clear reference structure, one gitignored bundle,
/// one denylisted `.env`, one secret-like config.
pub fn write_ts_mini(root: &Path) {
    w(root, ".gitignore", "dist/\n");
    w(
        root,
        "src/auth/session.ts",
        r#"export interface Session { id: string; userId: string; expiresAt: number }
export interface User { id: string }
export function createSession(user: User, ttl: number): Session {
  const store = new SessionStore();
  return store.create(user, ttl);
}
export class SessionStore {
  create(user: User, ttl: number): Session {
    return { id: "s1", userId: user.id, expiresAt: Date.now() + ttl };
  }
}
"#,
    );
    w(
        root,
        "src/http/middleware.ts",
        r#"import { createSession, Session } from "../auth/session";
export function requireSession(token: string): Session {
  return createSession({ id: token }, 3600);
}
export function attachSession(token: string): Session {
  return createSession({ id: token }, 60);
}
"#,
    );
    w(
        root,
        "src/cli/login.ts",
        r#"import { createSession } from "../auth/session";
import { log } from "../util/log";
export function login(userId: string): void {
  const s = createSession({ id: userId }, 3600);
  log(s.id);
}
"#,
    );
    w(
        root,
        "src/util/log.ts",
        "export function log(msg: string): void {\n  console.log(msg);\n}\n",
    );
    w(
        root,
        "src/config.ts",
        "export const awsKey = \"AKIAIOSFODNN7EXAMPLE\";\n",
    );
    w(root, "dist/bundle.js", "function bundled() {}\n");
    w(root, ".env", "SECRET=1\n");
    w(root, "README.md", "# ts-mini\n");
}

/// A named workspace: `app` is the TypeScript mini repo, `notes` holds one README that
/// mentions `createSession` in a code span and a second document `[[design]]` resolves
/// to. Returns the two root paths. The workspace directory itself holds only
/// `.singularrag/workspace.toml`.
pub fn write_workspace(dir: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let app = dir.join("roots/app");
    let notes = dir.join("roots/notes");
    write_ts_mini(&app);
    w(
        &notes,
        "README.md",
        "# Notes\n\nSessions come from `createSession`; see [[design]].\n",
    );
    w(
        &notes,
        "design.md",
        "# Design notes\n\nSee `createSession`.\n",
    );
    w(
        dir,
        crate::workspace::WORKSPACE_FILE,
        "[[root]]\nname = \"app\"\npath = \"roots/app\"\n[[root]]\nname = \"notes\"\npath = \"roots/notes\"\n",
    );
    (app, notes)
}

/// Documents next to the TS mini repo: two Markdown files that mention its symbols and
/// each other, a page, a stylesheet, four config files (one of them secret-like), a
/// text note, and two files that must be skipped for other reasons.
pub fn write_docs_mini(root: &Path) {
    write_ts_mini(root);
    w(root, "docs/design.md", "# Design\n\nSessions are created by `createSession` in [session](../src/auth/session.ts).\n\n## Freshness\n\nEvery call refreshes; see [[runbook]] for the STALE header.\n\n## Storage\n\n`SessionStore` keeps them.\n");
    w(root, "docs/runbook.md", "# Runbook\n\n## When the header says STALE\n\nWait and call again. The login flow (`login`) is unaffected.\n");
    w(root, "site/index.html", "<html><body><h1>Sessions</h1><div id=\"app\"><p>Uses <code>createSession</code>.</p></div></body></html>");
    w(
        root,
        "site/app.css",
        ".login { color: red }\n@media (max-width: 600px) { .login { color: blue } }\n",
    );
    w(
        root,
        "package.json",
        "{\"name\": \"mini\", \"scripts\": {\"build\": \"tsc\"}, \"private\": true}\n",
    );
    w(
        root,
        "secrets.json",
        "{\"token\": \"ghp_notreallyasecretbutlongenough1234567890\", \"name\": \"mini\"}\n",
    );
    w(root, "ci.yml", "name: ci\njobs:\n  build:\n    steps: []\n");
    w(
        root,
        "Cargo.toml",
        "[package]\nname = \"mini\"\n[dependencies]\nserde = \"1\"\n",
    );
    w(root, "notes.txt", "Remember: `log` is the only logger.\n");
    w(root, "package-lock.json", "{\"lockfileVersion\": 3}\n");
    w(
        root,
        "big.min.css",
        &format!(".a{{x:y}}{}", "a".repeat(2000)),
    );
}

pub fn write_rust_mini(root: &Path) {
    w(
        root,
        "Cargo.toml",
        "[package]\nname = \"mini\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    w(
        root,
        "src/lib.rs",
        "pub fn parse(s: &str) -> u32 {\n    helper(s)\n}\n\nfn helper(s: &str) -> u32 {\n    s.len() as u32\n}\n",
    );
    w(
        root,
        "src/main.rs",
        "fn main() {\n    let n = mini::parse(\"x\");\n    println!(\"{n}\");\n}\n",
    );
}
