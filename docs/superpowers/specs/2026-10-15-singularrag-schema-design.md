# singularrag schema design (sub-project 4: a codebase with its database)

**Status:** v0 design, approved in brainstorm 2026-10-15.
**Parent:** `docs/superpowers/specs/2026-09-19-singularrag-design.md`, as amended by the documents (2026-09-23), knowledge (2026-09-23), journeys (2026-09-24) and walks (2026-10-12) designs. This spec implements item 4 of the documents design's decomposition and amends the parent's §5 (see §9).

## Context

The walks design made singularrag a tool for a person understanding a corpus, judged by scripted walks through the UI's own data paths. The next source the documents design scheduled is the database: schemas, migrations, dumps and ORM models as tables and columns in the graph, linked to the code that reads and writes them and to the documents and process steps that name them. The caps fix (PR #29) removed the route caps the walks baseline had exposed, so the perspectives can carry a new node kind without a known hiding place.

Brainstorm decisions (Jonas, 2026-10-15): the first corpus is a pinned open-source Prisma app, umami, chosen for its size (30 MB, MIT), one 534-line `schema.prisma` with 26 models, 27 SQL migration folders, ClickHouse DDL under `db/`, and code that names tables two ways, through the Prisma client by model name and through raw SQL strings by table name. Drizzle is deferred to a bounded follow-up once a corpus exists. Tables and columns enter the index as symbols (approach A), not as knowledge-graph entities (B) or both (C), because every perspective already renders symbols, the walks already grade them, and one representation cannot drift from itself.

## 1. Goal

A person opening a codebase with its database sees the tables and columns in the same treegrid, map and detail panel as the code and documents, and can answer four questions through the UI: where is this table defined, which code reads or writes it, what does this table have to do with that table or that process, and what would this migration touch. No model call per table, no connection to a database, no new MCP tool.

## 2. Sources and parsing

Two new languages, `Prisma` (`.prisma`) and `Sql` (`.sql`), form a third category next to code and documents (`Language::is_schema`). Neither has a tree-sitter grammar; each gets a small hand-written parser in a new core module, `crates/singularrag-core/src/schema.rs`, producing the same `Tag` list the indexer writes for every other language. A schema file goes through the same files, symbols, FTS and refs tables; `symbols_fts` gets the table and column names, so `locate` and ranking see them as they see functions.

**Prisma.** Each `model` block becomes a symbol of kind `table`, named by its database name: the `@@map` value, else the model name. Each scalar field becomes a symbol of kind `column` named `table.column`, the column name being the field's `@map` value, else the field name. Relation fields (a type that is another model of the same file, or `Model[]`) are not columns. A column's signature is its type plus its attributes, trimmed to `SIGNATURE_MAX`; a table's signature is `table website (12 columns, model Website)`. `enum`, `generator`, `datasource` and `type` blocks produce nothing. A model without a closing brace ends the file's models at that point.

**SQL DDL.** A statement scanner, not a SQL parser. It splits the file into statements at top-level semicolons (quotes, parentheses and `$$` bodies respected; a `COPY ... FROM stdin;` block is consumed up to its `\.` terminator and dropped) and reads:

- `CREATE TABLE [IF NOT EXISTS] name (...)` and `CREATE [MATERIALIZED] VIEW name`: kind `table`; a view's signature says `view name`.
- Column definitions inside `CREATE TABLE`: kind `column`, named `table.column`, type the text up to the next top-level comma, trimmed to `SIGNATURE_MAX`. Lines starting with `CONSTRAINT`, `PRIMARY KEY`, `UNIQUE`, `INDEX`, `KEY`, `FOREIGN KEY` and `CHECK` are constraints, not columns.
- `ALTER TABLE [ONLY] name ADD [COLUMN] col type ...`: a `column` symbol under that table name, in the altering file.
- Foreign keys from an inline `REFERENCES other(col)` on a column and from `[ADD] CONSTRAINT ... FOREIGN KEY (...) REFERENCES other(...)`: a ref (§3).

Identifiers are unquoted: backticks and double quotes stripped, schema prefixes (`public.`, `umami.`) dropped. Everything else is skipped: `INSERT`, data, indexes, functions, triggers, `SET`, comments. This covers Prisma migrations (Postgres and MySQL dialects), umami's ClickHouse DDL (`LowCardinality(String)` is just a type string) and a `pg_dump` or `mysqldump` output in one scanner. A malformed statement costs itself, never the file.

**Caps.** Schema files obey the existing `MAX_FILE_BYTES` (1 MiB) and are skipped with the existing `too-large` reason above it; a dump with data usually exceeds it, and the guidance (README) is a schema-only export. No larger cap for dumps: a dump can carry data, and the secret-like-content check runs on the whole file before any parsing.

*Amended 2026-10-16 (final review).* A `schema_version` change no longer drops the content-hash caches (`extraction_cache`, `embedding_cache`, `steps_cache`): they carry their own `cache_version` meta key (a store from schema v5 to v8 without the key counts as cache version 1), and only a change to that drops them, so a rebuild re-drains extraction without a model call.

**Names collide on purpose.** The `session` table is a `table` symbol in `schema.prisma` and again in `01_init/migration.sql`. The index already treats a function defined twice this way: locate and change walks use path plus name, and referrers match by name, so both definitions share the same code links.

## 3. Links

**Foreign keys.** A foreign key is a ref row in the schema file at the column's line, named after the referenced table. `session.website_id` referencing `website` makes the schema file a referrer of `website`, and blast radius from `website` reaches `session` the way it reaches a caller today. Foreign keys are the only refs a schema file emits; it never references code.

**Code that reads and writes a table.** The bundled `tags.scm` captures calls and identifiers, never property names or string contents, so `prisma.client.website.findMany` and `from website_event` inside a template string produce no ref today. The indexer gains one deterministic pass for code files (`Language` not a document and not a schema): walk the tree-sitter tree it already builds and collect two node kinds, property identifiers after a dot (TypeScript and JavaScript `property_identifier`, Rust `field_identifier`) and the contents of string and template literals (`string_fragment`, Rust `string_content`). A token there (split on non-identifier characters) that is in the table vocabulary becomes a ref named after the table, at that line. Plain identifiers do not count: `session` as a variable name is not a reference to the `session` table. This is the noise rule; umami, with tables named `user`, `session`, `link` and `share`, is the corpus that tests it.

**The vocabulary** is every table name plus, for Prisma tables, the model name and its lower-camel form (`WebsiteEvent`, `websiteEvent`, `website_event` all resolve to `website_event`). It lives in a new store table `table_names(alias PRIMARY KEY, table_name)`, rebuilt from the schema files' symbols at the start of each run and after each schema file is indexed. Within a run the indexer processes schema files before every other file (a sort by category, then path). When a run's schema files change the vocabulary (the alias set differs from the previous run's, stored as a hash in `meta`), every code file is re-indexed in that run regardless of its content hash, once, after the schema files; the re-index cannot change the vocabulary, so there is no loop. Columns get no code links: `created_at` appears in every table and nearly every file.

*Amended 2026-10-16 (plan execution).* The implementation stores the vocabulary hash per file (`files.vocab`, the hash a file was indexed under; a code file is re-indexed when its stamp differs from the current hash, documents and schema files never by stamp) instead of a run-level `table_vocab` meta, because the meta was either quadratic on deadline-bounded drains or unsafe when a deadline interrupted the forced re-index.

**Documents and processes** need nothing new. A document that names `website_event` in a code span already emits a mention ref. A process step whose system is "the website event table" reaches the `website_event` symbol through the existing implemented-by token matching, because the table's name tokens are `website event`. Tables are never queued for extraction; they are not sections.

**Agent tools.** `repo_map`, `find_symbol`, `changed` and `trace_path` serve tables and columns as symbols without change. Descriptions and `INSTRUCTIONS` do not change.

## 4. UI and serving

No new route. The tree payload carries every symbol with kind, line and signature; the detail panel fetches blast radius for a symbol. Tables and columns arrive through both as they are.

- **Treegrid and query panel.** A schema file lists its `table` and `table.column` rows by line with the kind shown as for functions and sections. A served table reads `website_event · table` with the reasons the ranking draws.
- **Detail panel, table.** When the focused symbol's kind is `table`, the panel adds a Columns section: the file's `column` symbols whose name starts with `<table>.`, each with its signature, and for a column whose line carries a foreign-key ref the target table as a button that focuses that table's row in the same file, or the first file defining it. The existing files-that-reference section (blast radius) is the code-that-reads-and-writes answer. The foreign-key targets come from the tree payload's new per-symbol `refs` field for `column` symbols only (names referenced on the column's line), the one payload change in this spec.
- **Detail panel, column.** The parent table as a button, and the foreign-key target when present.
- **Accessibility.** Both lists are real lists with a heading; buttons carry `aria-describedby` naming the file and line, the Journeys pattern. Axe runs on the panel in the UI tests.
- **Map.** File-level as today: a schema file is one node, its edges to query files come from the new refs. A new language group `schema` covers `prisma` and `sql` for hue and hull. Per-table map nodes are not in this spec; a umami walk that fails because the file node hides the answer is a lead.

## 5. Walks, corpora and the gate

**Fixture.** `write_schema_mini(root)` in `fixture.rs`: `prisma/schema.prisma` with three models (`User` → `user`, `Website` → `website`, `WebsiteEvent` → `website_event`, with `@@map`, `@map` and a relation), `prisma/migrations/01_init/migration.sql` with inline foreign keys, `prisma/migrations/02_add_visit/migration.sql` altering a table, `db/dump.sql` with a `COPY` block, `src/queries/website.ts` using `prisma.client.website.findUnique`, a raw `from website_event` template string and a `const session = ...` variable (the negative case), and `docs/handbook.md` with a process section whose step names the website event table and a code span naming `website`. The CI walk file `eval/walks-ci.toml` gains one walk per kind against it (five with the `implemented_by` walk), run in `cargo test` with no model as today.

**One grader extension.** A `process` walk gains an optional `implemented_by`, a `path::name` that must be among the matched step's implemented-by links (the links the `entities` tool and Journeys render). Without it the fixture could not show that a step reaches a table. The harness is otherwise unchanged: a table is a symbol, so `locate`, `connection` with a symbol start and `change` already grade it.

**Corpus.** umami pinned at master `ec0ff50388c264ed8ce46f00967e92f7e71476ae` (2026-09-17), cloned to `~/Sites/umami-corpus` like hono, with `eval/walks-umami.toml`: twelve walks, four `locate`, four `connection`, four `change`, patches under `eval/patches/`. No `process` walks: umami has ten Markdown files and no process documentation, so Journeys over tables is judged on the fixture only; the handbook corpus has processes and no tables. The gap is stated in the eval README. umami needs no extraction; the baseline costs an index and a walk run, seeds on and off.

**Gate.** The walks rule, unchanged: the new walks pass on the fixture; the umami baseline is recorded under "Walks" in `eval/README.md` and may be poor; no walk that passed still fails; tier one holds within 0.02 on its four sets. Those sets contain no schema files, so the vocabulary is empty there and the code pass must emit nothing: the tier-one numbers must be identical, not merely within tolerance.

## 6. Errors

A schema file that is not UTF-8, binary, secret-like or over the cap is skipped with the existing reasons and shows in the skipped sheet. A malformed statement never fails the file. A foreign key to a table the index has not seen is still a ref and resolves when the table arrives. The vocabulary rebuild and re-index are one pass per run with no retry. A code file whose tree-sitter parse fails today is handled as today; the table pass runs only on a tree the tagger produced.

## 7. Security

Nothing new leaves the machine. The scanner never executes SQL and never opens a connection. Only DDL names and types reach the index or FTS; `INSERT` statements and `COPY` blocks are dropped before anything is stored, and the secret-like-content check precedes parsing. Schema files are read with the same path rules as every other file in a root.

## 8. Testing

Parser: `@@map` and `@map`; relation fields excluded; `enum`, `generator`, `datasource` ignored; an unterminated model; quoted and schema-prefixed identifiers; `IF NOT EXISTS`; constraints not columns; `ALTER TABLE ADD COLUMN` with and without `COLUMN`; inline and constraint foreign keys; a `COPY` block skipped; `INSERT` skipped; a view; ClickHouse types; a malformed statement followed by a good one. Indexer: the three alias forms resolve to the table name; the property and string rule with a plain identifier as the negative case; schema-before-code order; re-index on vocabulary change exactly once; no refs and no re-index with an empty vocabulary; a table defined in two files shares referrers. Serving: the column `refs` field present only for columns. UI: Columns list, foreign-key button, parent-table button, axe on the panel, the `schema` language group. Walks: the `implemented_by` grader with a matching and a non-matching link; the CI walk file passes all five; a wrong gold fails with the stated reason. Tier one: the four sets byte-identical before and after.

## 9. Parent spec amendments

- §5 (source types): "code and text documents" becomes "code, text documents and database schemas (Prisma, SQL DDL)".
- Documents design decomposition, item 4: Drizzle deferred; SQL dumps covered as DDL only.

## 10. Non-goals

Drizzle (deferred by decision). Row data, connections, query plans, dump sizes above the file cap. Per-table nodes on the map. Column-to-code links. A SQL grammar beyond DDL. Model extraction over tables. New MCP tools or description changes.
