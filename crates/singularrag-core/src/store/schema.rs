pub const SCHEMA_VERSION: i64 = 1;

pub const DDL: &str = r#"
CREATE TABLE IF NOT EXISTS meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS files (
  id             INTEGER PRIMARY KEY,
  path           TEXT NOT NULL UNIQUE,
  lang           TEXT,
  content_hash   TEXT,
  mtime_ms       INTEGER NOT NULL,
  size           INTEGER NOT NULL,
  indexed_at_ms  INTEGER NOT NULL,
  skipped_reason TEXT
);
CREATE TABLE IF NOT EXISTS symbols (
  id         INTEGER PRIMARY KEY,
  file_id    INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  name       TEXT NOT NULL,
  kind       TEXT NOT NULL,
  line_start INTEGER NOT NULL,
  line_end   INTEGER NOT NULL,
  signature  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS symbols_name ON symbols(name);
CREATE INDEX IF NOT EXISTS symbols_file ON symbols(file_id);
CREATE TABLE IF NOT EXISTS refs (
  id      INTEGER PRIMARY KEY,
  file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  name    TEXT NOT NULL,
  line    INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS refs_name ON refs(name);
CREATE INDEX IF NOT EXISTS refs_file ON refs(file_id);
CREATE VIRTUAL TABLE IF NOT EXISTS symbols_fts USING fts5(
  name, name_tokens, signature, path, tokenize='unicode61'
);
CREATE TABLE IF NOT EXISTS retrievals (
  id            INTEGER PRIMARY KEY,
  session_key   TEXT NOT NULL,
  tool          TEXT NOT NULL,
  query         TEXT,
  focus_files   TEXT NOT NULL,
  budget        INTEGER NOT NULL,
  index_version TEXT NOT NULL,
  git_head      TEXT,
  stale_count   INTEGER NOT NULL,
  created_at_ms INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS retrieval_items (
  retrieval_id INTEGER NOT NULL REFERENCES retrievals(id) ON DELETE CASCADE,
  symbol_id    INTEGER NOT NULL,
  rank         INTEGER NOT NULL,
  score        REAL NOT NULL,
  served       INTEGER NOT NULL,
  reasons_json TEXT NOT NULL,
  PRIMARY KEY (retrieval_id, rank)
);
CREATE TABLE IF NOT EXISTS indexer_lock (
  id              INTEGER PRIMARY KEY CHECK (id = 1),
  pid             INTEGER NOT NULL,
  heartbeat_at_ms INTEGER NOT NULL
);
"#;
