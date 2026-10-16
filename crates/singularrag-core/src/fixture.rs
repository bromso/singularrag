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
    // `.singularrag/index.db` is the tool's own on-disk index, not corpus content (the
    // project's own top-level .gitignore makes the same exclusion); a change walk's git
    // checkout must stay clean after indexing, not just after the patch it applies.
    w(root, ".gitignore", "dist/\n.singularrag/index.db\n");
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
        // Assembled at runtime: the literal would make this source file look secret-like.
        &format!(
            "export const awsKey = \"{}{}\";\n",
            "AKIA", "IOSFODNN7EXAMPLE"
        ),
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
        &format!(
            "{{\"token\": \"{}{}\", \"name\": \"mini\"}}\n",
            "ghp_", "notreallyasecretbutlongenough1234567890"
        ),
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

/// Two prose documents with no code around them, for the knowledge layer: a fictional
/// 1890s research voyage and a fictional company handbook. `fixtures/prose/extraction.json`
/// holds a hand-written extraction for every section, keyed `docs/<file>::<heading>`.
pub fn write_prose(root: &Path) {
    w(
        root,
        "docs/voyage.md",
        include_str!("../fixtures/prose/voyage.md"),
    );
    w(
        root,
        "docs/handbook.md",
        include_str!("../fixtures/prose/handbook.md"),
    );
}

/// A codebase with its database (schema spec §5): a Prisma schema with three models, two
/// migrations, a dump with a data block, one query module that names tables the two ways
/// code does (a client property, a SQL string) and the way the rule ignores (a variable
/// called `session`), and a document whose process step names a table in a code span.
pub fn write_schema_mini(root: &Path) {
    w(root, ".gitignore", "dist/\n.singularrag/index.db\n");
    w(
        root,
        "prisma/schema.prisma",
        r#"datasource db {
  provider = "postgresql"
}

generator client {
  provider = "prisma-client-js"
}

model User {
  id        String    @id @map("user_id") @db.Uuid
  username  String    @unique @db.VarChar(255)
  createdAt DateTime? @default(now()) @map("created_at")
  websites  Website[]

  @@map("user")
}

model Website {
  id        String         @id @map("website_id") @db.Uuid
  name      String         @db.VarChar(100)
  domain    String?        @db.VarChar(500)
  userId    String?        @map("user_id") @db.Uuid
  user      User?          @relation(fields: [userId], references: [id])
  events    WebsiteEvent[]

  @@index([userId])
  @@map("website")
}

model WebsiteEvent {
  id        String   @id @map("event_id") @db.Uuid
  websiteId String   @map("website_id") @db.Uuid
  urlPath   String   @map("url_path") @db.VarChar(500)
  eventName String?  @map("event_name") @db.VarChar(50)
  createdAt DateTime @default(now()) @map("created_at")
  website   Website  @relation(fields: [websiteId], references: [id])

  @@map("website_event")
}

enum Role {
  admin
  user
}
"#,
    );
    w(
        root,
        "prisma/migrations/01_init/migration.sql",
        r#"-- CreateTable
CREATE TABLE "user" (
    "user_id" UUID NOT NULL,
    "username" VARCHAR(255) NOT NULL,
    "created_at" TIMESTAMPTZ(6) DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT "user_pkey" PRIMARY KEY ("user_id")
);

-- CreateTable
CREATE TABLE "website" (
    "website_id" UUID NOT NULL,
    "name" VARCHAR(100) NOT NULL,
    "domain" VARCHAR(500),
    "user_id" UUID REFERENCES "user"("user_id"),

    CONSTRAINT "website_pkey" PRIMARY KEY ("website_id")
);

-- CreateTable
CREATE TABLE "website_event" (
    "event_id" UUID NOT NULL,
    "website_id" UUID NOT NULL,
    "url_path" VARCHAR(500) NOT NULL,
    "created_at" TIMESTAMPTZ(6) NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT "website_event_pkey" PRIMARY KEY ("event_id"),
    CONSTRAINT "website_event_website_fk" FOREIGN KEY ("website_id") REFERENCES "website"("website_id")
);

-- CreateIndex
CREATE INDEX "website_event_website_id_idx" ON "website_event"("website_id");
"#,
    );
    w(
        root,
        "prisma/migrations/02_add_event_name/migration.sql",
        "-- AlterTable\nALTER TABLE \"website_event\" ADD COLUMN \"event_name\" VARCHAR(50);\n",
    );
    w(
        root,
        "db/dump.sql",
        r#"--
-- PostgreSQL database dump (schema and data)
--
SET statement_timeout = 0;

CREATE TABLE public.report (
    report_id uuid NOT NULL,
    website_id uuid NOT NULL,
    name character varying(200) NOT NULL
);

ALTER TABLE ONLY public.report
    ADD CONSTRAINT report_website_fk FOREIGN KEY (website_id) REFERENCES public.website(website_id);

COPY public.report (report_id, website_id, name) FROM stdin;
0b1c	9f3a	Weekly (CREATE TABLE in data, not DDL)
\.

INSERT INTO public.report VALUES ('0b1d', '9f3a', 'CREATE TABLE phantom (x int)');
"#,
    );
    w(
        root,
        "src/queries/website.ts",
        r#"import { prisma } from "../lib/prisma";
import { rawQuery } from "../lib/db";

export function getWebsite(websiteId: string) {
  return prisma.client.website.findUnique({ where: { id: websiteId } });
}

export function getWebsiteDateRange(websiteId: string) {
  return rawQuery(`
    select min(created_at) as mindate, max(created_at) as maxdate
    from website_event
    where website_id = {{websiteId::uuid}}
  `, { websiteId });
}

export function touchSession(id: string) {
  const session = { id, seenAt: Date.now() };
  return session;
}
"#,
    );
    w(
        root,
        "src/lib/prisma.ts",
        "export const prisma = { client: {} as Record<string, { findUnique: (q: unknown) => unknown }> };\n",
    );
    w(
        root,
        "src/lib/db.ts",
        "export function rawQuery(sql: string, params: Record<string, string>) {\n  return { sql, params };\n}\n",
    );
    w(
        root,
        "docs/data-model.md",
        "# Data model\n\n## Recording a visit\n\nWhen a visitor opens a page, the tracker script posts to the collect endpoint. The collector looks up the website row by its tracking id, then writes one row to the `website_event` table with the URL path and the event name. Reports read `website_event` through `getWebsiteDateRange`.\n",
    );
}
