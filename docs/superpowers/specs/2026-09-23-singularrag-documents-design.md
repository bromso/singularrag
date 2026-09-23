# singularrag — workspace and text documents design (sub-project 1 of the document RAG)

Date: 2026-09-23. Status: approved in brainstorming; awaiting written-spec review.
Parent spec: `docs/superpowers/specs/2026-09-19-singularrag-design.md` (§5 non-goals, §7 index model and ranking, §9 tools, §10 UI, §11 controls, §12 eval). Workflow spec: `docs/superpowers/specs/2026-09-21-singularrag-workflow-design.md` (hook, `changed`). This work lives on `documents-v0`, branched from `workflow-v0` at 0b769e3 (PR #16), because the hook and `changed` are extended here.

## Context: the product decision behind this spec

On 2026-09-23 Jonas decided that singularrag becomes a general local RAG over everything he has: documents, office files, media, database schemas and dumps, more programming languages, with LightRAG's retrieval approach as the reference for quality. Decisions recorded in that session, binding for this and the following specs:

- Models: local by default (Ollama or llama.cpp, a local embedding model, local OCR and transcription), an API key optional and opt-in. This spec uses no model at all.
- Corpus: several named folders on disk. Databases mean schema files and exported dumps, not connections.
- Success: deterministic retrieval recall against gold sources on Jonas's own corpus, at a token budget, no model in the grader.
- Embeddings and hybrid search: built in SQLite (fastembed plus sqlite-vec next to FTS5), the path the parent spec §7 reserved. singularmem stays the separate memory layer.
- What stays the product: the provenance loop (served, cut, reasons, pin, exclude, annotate). The host agent still writes the answer; singularrag serves context; reasons stay drawable.
- Perspectives for the human, after LightRAG's UI: Documents, Knowledge graph, Retrieval, API, and Jonas's own **Journeys**: how a person moves through the organisation's processes, and how each step connects to the knowledge that describes it and the technology that implements it.

The decomposition, in build order, each its own spec, plan and PR:

1. **This spec:** workspace with several roots; Markdown, HTML, CSS, JSON, YAML, TOML and plain text as sources; the Documents and Retrieval perspectives.
2. LightRAG-style retrieval: fastembed plus sqlite-vec, local-model entity and relation extraction into the graph, dual-level (low-level entity, high-level theme) keyword retrieval, the document eval set. Overturns the parent spec's "never talks to a model" rule; that spec records it.
3. Processes and journeys: an ontology (Process, Step, Role, System; "documented in", "implemented by"), extraction prompts for steps and roles, links to code, the Journeys perspective.
4. Database schemas as files: migrations, SQL dumps, Prisma and Drizzle schemas as tables and columns in the graph.
5. Office and PDF: born-digital PDF, Word, PowerPoint and Excel text extraction, OCR for scanned pages, a `read_section` tool.
6. Media: images (OCR, optional local vision caption), video (keyframes, local transcription).
7. More programming languages: one grammar at a time, bounded tasks.

Parent spec §5 amendments made by this spec: "One source type: code" becomes "code and text documents; other sources arrive by later specs". The "no LLM" and embedding-gate lines stay until spec 2 amends them.

## 1. Goal

Index text documents next to code, from one or several roots, so that `repo_map` serves the spec section that describes a function as readily as the function, a vault note that mentions a symbol links to it, and the human can see documents in the treegrid and try a query in the UI. No model, no embeddings, no new MCP tool.

## 2. Workspace and roots

A workspace is any directory holding `.singularrag/`. Optional file `.singularrag/workspace.toml`:

```toml
[[root]]
name = "app"
path = "../app"            # relative to the workspace directory, or absolute

[[root]]
name = "vault"
path = "/Users/jonas/Notes"
```

- No file, or no `[[root]]` entry: the workspace directory is its only root and every indexed path is bare and relative, exactly as today. With roots declared, every indexed path is `<name>/<relative>`; the workspace directory itself is not indexed unless listed as a root.
- `name` is `[A-Za-z0-9_-]{1,64}` and unique. `path` is canonicalised at load; it must exist, be a directory, and no root may contain another (error: `root <a> contains root <b>`). Symlinks are not followed by the walker, as today.
- `--repo` keeps its name and means the workspace directory. `Engine::open` takes the workspace. Every consumer of "the root" (walker, indexer, hook, `changed`, watcher, serve state) takes the root list from a new `Workspace { dir, roots: Vec<Root { name, path }> }` in core; a single-root workspace has one unnamed root.
- Each root is walked with its own `.gitignore` chain. The deny list (`map.toml` `[deny]` plus the builtin list) and `map.toml` excludes apply workspace-wide. `map.toml` lives in the workspace and its paths carry the `<name>/` prefix in a multi-root workspace; `check_path` accepts a first segment equal to a root name and rejects an unknown one, so pins, excludes, notes, boundaries and `annotate` follow without further change.
- Git: `git_head` is recorded per root at index time in `meta` (`git_head:<name>`). The freshness header shows `HEAD <sha>` for a single root as today, and `HEAD app:9b1e0d4 vault:none` for several, in root order. The status endpoint returns a list. `changed` runs its `git` calls per root that is a git checkout, prefixes the paths, and reports the union; a workspace with no git root at all is the existing "not a git checkout" error.
- The `serve` watcher watches every root. The hook maps an absolute `file_path` back to `<name>/<relative>` by the longest matching root prefix after canonicalisation; no match means allow, as today.
- `workspace.toml` is read, never written, by singularrag. The UI shows the roots in the status line; editing them is a file edit.

## 3. Document model: sections are symbols

A document is a `files` row with one of the new `lang` values `markdown`, `html`, `css`, `json`, `yaml`, `toml`, `text`. `Language::from_path` maps `.md .markdown`, `.html .htm`, `.css`, `.json`, `.yaml .yml`, `.toml`, `.txt`. Structure is `symbols` rows produced by `extract_tags` with a tree-sitter grammar and a tags query per format, like TypeScript and Rust today (crates: `tree-sitter-md`, `tree-sitter-html`, `tree-sitter-css`, `tree-sitter-json`, `tree-sitter-yaml`, `tree-sitter-toml-ng`; the plan's first task verifies each builds against tree-sitter 0.27 and pins the versions).

| format | kind | name | span | signature |
|---|---|---|---|---|
| Markdown | `section` | heading text, trimmed, without `#` | from the heading to the line before the next heading of the same or a higher level; h4 and deeper fold into their parent | the heading line |
| Markdown, HTML, text with no headings | `document` | file stem | whole file | first non-empty line, at most 120 chars |
| HTML | `section` | heading text | the heading element's line span | the heading text |
| HTML | `element` | `id` attribute value | the element's span | `<tag id="…">` |
| CSS | `rule` | selector text | the rule's span; rules inside `@media` fold into one `rule` named by the at-rule prelude | the selector |
| JSON, YAML, TOML | `key` | dotted key path to depth 2 (`scripts.build`, `dependencies`) | the key's span | `key: object`, `key: array`, `key: string`, `key: number`, `key: bool` |

Every document additionally gets one `document` symbol named by its file stem spanning the whole file, so links to the file resolve by name (§4). Values in JSON, YAML and TOML are never indexed: not as symbols, not in FTS, not in signatures beyond the type word.

**Anchors.** `line_start` and `line_end` mean "anchor start and end" and are lines for every format in this spec. Later specs use the same columns for pages (PDF), slides (PowerPoint), sheets (Excel) and seconds (video), with `lang` telling the renderer which. The map, the treegrid and `retrieval_items` do not change shape for that; only the number column's label does (§5).

**Body text.** New FTS5 table `sections_fts(path UNINDEXED, name UNINDEXED, content, tokenize='porter unicode61')`, one row per section-kind symbol (`section`, `document`, `element`) holding the section's text with code fences and HTML tags stripped. `rule` and `key` symbols have no body row. Rows are rewritten with the file on refresh and removed with it.

**Skips**, written to `files.skipped_reason` like today: `too large` over 256 KB; `lockfile` for `package-lock.json`, `yarn.lock`, `pnpm-lock.yaml`, `Cargo.lock`, `bun.lock`, `bun.lockb`, `composer.lock`, `Gemfile.lock`, `poetry.lock`; `minified` when the average line length exceeds 500 characters; `looks like a secret` from `secrets::looks_secret` over the whole text, as for code. Skipped files appear in the skipped sheet with the reason.

## 4. Mentions: references from documents

`refs` rows are produced from the document tree:

- Markdown inline code spans and HTML `<code>` text: the span text, when it is one identifier (`[A-Za-z_][A-Za-z0-9_]*`, optionally with a trailing `()`), becomes a ref named by the identifier.
- Wiki links `[[name]]` and `[[name|label]]`: a ref named `name` (before any `#` or `|`).
- Markdown link targets and HTML `href` values that are relative paths: a ref named by the target's file stem (`../specs/design.md` → `design`), fragments dropped. Absolute URLs produce nothing.
- Fenced code blocks produce no refs; their identifiers would swamp the graph.

Refs join to `symbols.name` in `graph::build_graph` exactly as code references do, so a vault note that mentions `createSession` in a code span gets an edge to `app/src/auth/session.ts`, across roots, and a note that links `[[design-notes]]` gets an edge to `design-notes.md` through its `document` symbol. Duplicate names resolve as they do for code (every defining file gets the edge). Code does not reference documents in this spec.

## 5. Ranking, map and tools

- **Seeds.** A query term that hits `sections_fts` seeds the section's file with `FTS_FILE_BOOST`, the same boost as a `symbols_fts` hit today. A `query_ident_match` on a section or document name counts like one on a symbol name. `Reasons` gains `body_hit: bool`, rendered in the UI as "Matches the text of the section". `RefBy`, pins, notes and the unreferenced fraction are unchanged; there is no per-kind budget share.
- **Support files.** Documents are never support files: `is_support_file` keys on `.test.`, `.spec.` and support directories, and `docs/` is not one. Nothing changes there.
- **Map rendering.** A document renders like a file: its header line with referencing files, then one line per served symbol with the anchor and the signature, so `docs/design.md:` then `  108  ## 8. Freshness`. `key` and `rule` rows render their signature. For non-line anchors in later specs the number column renders `p.4` or `s.4`; this spec renders lines only. `fit`, `CUT_RECORDED` and the footer are untouched.
- **Tools.** No new tool. `repo_map` serves mixed results. `find_symbol` matches section names, element ids, selectors, keys and document stems. `trace_path` walks mention edges. `changed` maps document hunks to sections. `annotate` targets a section like a symbol (path plus symbol name). The hook denies a Read of an indexed document like an indexed source file.
- **Agent-facing text.** `INSTRUCTIONS` and `REPO_MAP_DESCRIPTION` say the map also lists document sections (specs, notes, configs) and that the agent should read the section the map points at. This moves the tier-two baseline, recorded in §7.

## 6. UI perspectives

- **Documents.** The treegrid gains a kind badge per symbol row (`section`, `element`, `rule`, `key`, `document`) and, in a multi-root workspace, a root filter over the first path segment. The skipped sheet lists document skips with their reasons. `RepoTree` grows; no new component. The status line shows the roots.
- **Knowledge graph.** The Sigma map draws document nodes in a colour per `lang` group (docs, config, styles) and mention edges like reference edges. Boundaries may include documents. Hover and the detail panel are unchanged.
- **Retrieval.** A query panel above the rail: a labelled text field, a budget select (1024, 2048, 4096), a submit button. Submit calls `POST /api/query {query, budget}`, which runs `repo_map` under the session key `ui` with the label "This UI" and records the retrieval, so it appears in the rail and the detail panel with served, cut and reasons like any agent call. The response is the new retrieval's id; the rail selects it. The live region announces "N served, M cut".
- **API** page: deferred. **Journeys:** spec 3.
- Accessibility: the panel is a `<form>` with labels; errors render inline and are announced; an axe test runs over the panel with a result selected; the kind badge is text, not colour alone.

## 7. Eval

- Tier one gains `eval/questions-docs.toml`, run against this repository as the corpus (it has specs, plans, a README and code that cite each other). Gold entries name sections as `path::name` with the section's heading text as `name`, exactly what `singularrag find` prints. Twelve questions in three categories: `locate-doc` (which section specifies X), `doc-to-code` (which code implements the section), `code-to-doc` (which section describes this symbol). Fixed on first run like the hono set.
- The hono question set does not move. Gate for this spec: hono tier-one recall at 4096 within 0.02 of the value before this branch, and the docs set at or above 0.6 at 4096. Tier two reruns later with the workflow condition set; the agent-facing text change in §5 is a second reason that run's delta is not attributable to one change.
- A multi-root fixture in core (the TypeScript mini repo as root `app`, plus a `notes` root with a Markdown file that mentions `createSession` and links `[[design]]`) covers cross-root indexing, mentions and recall in unit tests.

## 8. Security

- Roots: canonicalised, directories only, none inside another, symlinks not followed. The builtin deny list plus `[deny]` extras apply to every root. `looks_secret` runs over document text. Config values never enter the index. Size, lockfile and minified-file skips bound parse time and index size.
- `POST /api/query` sits behind the existing per-run token and origin check, accepts a query of at most 2,000 characters and a clamped budget, and writes only a retrieval row.
- `workspace.toml` is read, never written. A malformed file is a startup error naming the file and the field.

## 9. Parent spec amendments

- §5: "One source type: code" → code and text documents; later specs add sources.
- §6: the workspace file; documents in the map.
- §7 index model: the new `lang` values, `sections_fts`, anchors, the `body_hit` reason.
- §9: tools serve document sections; `find_symbol` over sections; the agent-facing text.
- §10: the Documents and Retrieval perspectives; `POST /api/query`.
- §12: the docs question set and the gate.

## 10. Testing

Per format, a tags test over a fixture file: Markdown heading folding and a headingless file; HTML headings and `id` elements; CSS selectors and `@media` folding; JSON, YAML and TOML keys to depth 2 with no values anywhere. Mentions: code span, wiki link with a label, relative Markdown link, `href`, fenced block ignored, absolute URL ignored. Workspace: no file means bare paths; two roots prefix paths; a root inside another is rejected; an unknown prefix in `map.toml` is rejected; per-root heads in the header. Ranking: a body hit seeds the file and sets `body_hit`; a mention edge crosses roots. Map: a section row renders under its file with the anchor. Tools: `find_symbol` finds a heading; `trace_path` from a note to a symbol; `changed` on a document edit names the section; the hook denies a Read of an indexed document once. Skips: each reason. UI: kind badges, root filter, query panel round trip with the rail updating, axe over the panel. Eval: both question files run and the gate values are reported.

## 11. Non-goals

PDF, Office, images, video, databases; entity extraction, embeddings, LightRAG retrieval modes; processes and journeys; an API page; a per-kind budget share; more programming languages (each a bounded task); `read_section`; code referencing documents; indexing document values; following symlinks; writing `workspace.toml`.
