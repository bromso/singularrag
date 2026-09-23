# singularrag — knowledge design: embeddings, entity extraction and the `entities` tool (sub-project 2 of the document RAG)

Date: 2026-09-23. Status: approved in brainstorming; awaiting written-spec review.
Parent spec: `docs/superpowers/specs/2026-09-19-singularrag-design.md` (§5 non-goals, §7 index model and gates, §9 tools, §10 UI, §11 controls, §12 eval). Documents spec: `docs/superpowers/specs/2026-09-23-singularrag-documents-design.md` (workspace, sections, `sections_fts`, `hit_shares`). This work lives on `knowledge-v0`, branched from `main` at 5ec066b.

## Context

Sub-project 2 of the decomposition recorded in the documents spec: singularrag becomes a general local RAG with LightRAG's retrieval approach as the quality reference. LightRAG builds a graph of entities and relations with an LLM, embeds chunks and entities, and answers with dual-level retrieval: low-level (entities named by the query) and high-level (themes, matched against relations). This spec brings that to singularrag without a query-time model call and without an answer generator: the host agent is the model, singularrag serves context with provenance.

Decisions from the 2026-09-23 brainstorm, binding here:

- **Corpus:** Jonas's notes vault as a workspace root for the eval that matters (question file outside the repo), plus a small public prose fixture committed for tests and a reproducible question set.
- **Runtime:** Ollama for both extraction and embeddings, over HTTP with `reqwest`; no ONNX runtime or llama.cpp in the binary; nothing in singularrag downloads a model. An opt-in API route for extraction only.
- **Agent view:** `repo_map` gains two silent seeds (semantic, entity) with drawable reasons; one new tool, `entities`, returns LightRAG's context block (entities, relations, source sections). The agent may pass `entities` and `themes` itself; singularrag makes no model call at query time except one embedding of the query.
- **Extraction:** per section, cached by the section's content hash, run by a background job in the actor under a time budget, with a pending count in the freshness header. Failures skip, never crash.

This spec overturns the parent's "singularrag never talks to a model" (§5) and retires the embedding gate (§7); §9 amendments record it.

## 1. Goal

Prose queries in plain language find the right sections even with no keyword overlap; questions about a person, system or concept return what the corpus says about it and how it connects, with the sections that say so; the human sees entities and relations on the map with the same provenance loop as everything else. Ollama down means today's behaviour plus a header line, never an error.

## 2. Models and configuration

`workspace.toml` gains an optional table:

```toml
[models]
ollama = "http://127.0.0.1:11434"   # default
extract = "qwen2.5:7b-instruct"     # default
embed = "nomic-embed-text"          # default
# api = "anthropic"                 # opt-in: extraction through the API, key from ANTHROPIC_API_KEY; embeddings stay local
```

- Core module `models.rs`: `embed(texts: &[String]) -> Result<Vec<Vec<f32>>>` (Ollama `/api/embed`, batches of at most 50) and `extract(section: &SectionInput) -> Result<Extraction>` (Ollama `/api/generate` with `format: "json"`, or the API when configured). 30 s timeout per call, one retry on a network error, none on a model error. Every failure is `Error::ModelUnavailable(String)`; callers treat it as "skip for now".
- The embedding dimension is read from the first successful call and stored in `meta` (`embed_dim`, `embed_model`). A later call with a different model or dimension drops and rebuilds the vector tables and re-queues every section; the header says `embeddings: rebuilding`.
- `singularrag doctor`: prints whether Ollama answers at the configured URL, whether both models are pulled (`/api/tags`), the embedding dimension, the queue length and the last extraction error. Read-only. README's install section tells the user to install Ollama and pull the two models.

## 3. Extraction and the knowledge stores

Schema version 4 adds, in `index.db`:

| table | columns | notes |
|---|---|---|
| `section_embeddings` | `symbol_id INTEGER PRIMARY KEY`, `hash TEXT`, plus a `sqlite-vec` virtual table `section_vec(embedding float[dim])` keyed by the same rowid | one row per document section with a non-blank body |
| `entities` | `id`, `name`, `norm_name UNIQUE`, `type`, `description`, `mentions INTEGER` | `norm_name` = lowercased, whitespace collapsed, trailing punctuation dropped; `description` is the first extractor's, extended (joined with `; `, capped at 300 chars) when a later section adds a different one |
| `entity_vec` | `sqlite-vec` virtual table keyed by `entities.id` | embedding of `name + ": " + description` |
| `entity_mentions` | `entity_id`, `symbol_id`, `section_hash`, primary key `(entity_id, symbol_id)` | deleted with the section's hash |
| `relations` | `id`, `src_entity`, `dst_entity`, `description`, `symbol_id`, `section_hash` | one row per statement per section; `relation_vec` keyed by `id` embeds `src → dst: description` |
| `extract_queue` | `symbol_id PRIMARY KEY`, `hash`, `attempts`, `last_error`, `queued_at_ms` | what still needs the model |

- The indexer, when it writes a document section with a body (kinds `section`, `document`, `element`), upserts `extract_queue (symbol_id, hash)` and deletes embedding, mention and relation rows whose `section_hash` differs from the new hash. Deleting a file deletes its rows through the same `delete_symbols_for` path. Code symbols are never queued. Entities with zero mentions left are deleted.
- Extraction input: the section's text (at most 6,000 characters; longer sections are split at paragraph boundaries into parts that share the symbol id), its heading, and its path. The prompt asks for JSON `{ "entities": [{name, type, description}], "relations": [{source, target, description}] }` with the closed type list `person`, `organisation`, `system`, `concept`, `event`, `place`, `document`; unknown types map to `concept`. Names are truncated to 80 characters, descriptions to 300, control characters stripped. Malformed JSON gets one retry with a stricter instruction; then `attempts` increments and the row waits for the next tick; after three attempts the section is skipped and listed in the UI's skipped sheet as `extraction failed: <last_error>`.
- The background job runs in the actor beside the refresh drain, only under `serve` and `mcp` (never the one-shot CLI): after every refresh, and every 30 s while the queue is non-empty, it embeds up to 50 queued sections in one call, then extracts up to 5 sections under a 20 s budget, oldest `queued_at_ms` first, one transaction per section. The freshness header gains ` · entities: N pending` while the queue is non-empty and ` · models: unavailable` while the last model call failed; the status endpoint carries both.
- Deduplication is by `norm_name` only. Two different people with the same name merge; aliases do not. Both are non-goals here.

## 4. Retrieval

- **Semantic seed.** `repo_map` with a query embeds the query once. The 20 nearest sections by cosine (a `section_vec` KNN) seed their files with `FTS_FILE_BOOST × similarity`, and each matched section is a hit with weight `similarity` through `rank::hit_shares` (documents spec §5, amended). `Reasons` gains `semantic: Option<f64>`, rendered "Semantically close to the query (0.83)". Embedding failure skips the seed; the header says `models: unavailable`.
- **Entity seed.** Query terms are matched whole-word against `entities.norm_name`, the agent's optional `entities: ["…"]` argument is matched the same way, and the query embedding is matched against `entity_vec` (10 nearest above similarity 0.5). Every section that mentions a matched entity seeds its file with `NOTE_BOOST` and is a hit with weight 1.0. `Reasons` gains `entities: Vec<String>` (the matched names), rendered "Mentions Acme Ltd". This is LightRAG's low-level path.
- **Themes.** The agent's optional `themes: ["…"]` argument is embedded and matched against `relation_vec` (10 nearest); the sections that state those relations seed their files as above, reason `theme: <description>`. The high-level path; without the argument, the query embedding covers it.
- **`entities {query, entities?, limit?}`** (limit default 10, max 25): after the freshness header, matched entities as `name (type): description`, then relations as `src → dst: description (path::heading, lines a-b)`, then `# N entities · M relations · K sections`. Matching is the entity seed's rule. One `retrievals` row with `tool = "entities"`, `query`, served items = the sections cited in rank order, `limit_n = limit`; reasons carry `entities`. No write.
- `MapRequest` gains `entities: Vec<String>` and `themes: Vec<String>`; the MCP `repo_map` schema and the CLI `query --entity/--theme` flags expose them.
- `INSTRUCTIONS` and `REPO_MAP_DESCRIPTION` say prose queries work in plain language, that `entities` answers "what do we know about X and how is it connected", and that entity descriptions are extracted text, not verified facts. `find_symbol`, `trace_path`, `changed`, `annotate` and the hook are unchanged.

## 5. UI: the knowledge-graph perspective

- The Sigma map gains an entity overlay toggled from the view toggle (`Files`, `Files + entities`): entity nodes coloured by type and sized by mention count, edges from an entity to the sections that mention it (dashed) and between related entities (solid), file and section nodes as today. Hover shows the description; click focuses the entity and the detail panel lists its mentions and relations with links that focus the section row in the treegrid.
- The detail panel for a section lists the entities it mentions and the relations it states. `reasonsToSentences` gains the semantic, entity and theme sentences.
- The status line shows `entities: N pending`, `models: unavailable` and `embeddings: rebuilding` from the status endpoint; the skipped sheet lists extraction failures with their reason.
- The query panel is unchanged and now exercises the seeds.
- Accessibility: the overlay toggle is a labelled radio group like the view toggle; entity nodes have accessible names `<name> (<type>), N mentions`; axe over the panel with an entity focused.

## 6. Eval

- **Vault set:** Jonas's vault as a named root; `~/.singularrag/questions-vault.toml` (outside the repo, passed with `--questions`), twelve questions: `paraphrase` (no keyword overlap with the gold section), `entity`, `relation`. Gold are sections. Tier one scores recall at 4096 as today; `entity` and `relation` questions are also run through the `entities` tool and count as cited when a gold section is among the served items.
- **Prose fixture:** `crates/singularrag-core/fixtures/prose/`: a few thousand words of public-domain narrative with people, places and events, and a handbook-style Markdown file with processes and roles; `fixtures/prose/extraction.json` holds a checked-in extraction so unit tests and the committed `eval/questions-prose.toml` (twelve questions, same categories) never call a model. A test behind `SINGULARRAG_OLLAMA=1` runs the real extractor when Ollama answers.
- **Gate:** the hono and docs sets stay within 0.02 of their `main` values at 4096; the prose set reaches 0.6 with seeds on and reports its seeds-off value; the vault set reaches 0.6 with seeds on; the `entities` tool cites a gold section for at least 8 of the 12 entity and relation questions on each set. `singularrag eval --no-models` gives the seeds-off number in one command. Tier two is out of scope; the efficiency-rule question from run 3 stays open.

## 7. Security

- Section text leaves the process only to the configured Ollama URL, local by default. The API route is opt-in per workspace; README states what is sent (section text, heading, path) and to whom. Secret-like documents are already skipped whole before any of this, so nothing flagged is ever sent.
- Model output is data: names and descriptions truncated and control-character-stripped, rendered as text, never as links or instructions; `INSTRUCTIONS` says so to the agent.
- Vector tables live in `index.db`; no new file. `workspace.toml` stays read-only; `doctor` reads only. The background job holds the indexer lock per section transaction, never across a model call.

## 8. Parent spec amendments

- §5: "Any LLM call" becomes "no query-time model call beyond one embedding; extraction and embeddings run through a local model runtime, opt-in API for extraction".
- §7: the embedding gate is retired, citing run 3 and the docs set; the index model gains the six tables.
- §9: the sixth tool, `entities`; `repo_map`'s `entities` and `themes` arguments.
- §10: the entity overlay and status lines.
- §12: the vault and prose question sets and their gate.

## 9. Testing

Models: a fake Ollama (in-test axum server) serving `/api/embed`, `/api/generate`, `/api/tags`; each caller's `ModelUnavailable` path. Extraction: queue rows follow section hashes; a changed section re-queues and its old mentions and relations vanish; a deleted file clears everything; malformed JSON retries once then counts an attempt; three attempts skip and show in the skipped sheet; `norm_name` dedup; description extension capped; type mapping; long sections split at paragraphs. Vectors: KNN returns the nearest; a dimension change rebuilds and re-queues. Retrieval: semantic seed and reason; entity seed from a term, from the argument, from the embedding; themes against relations; seeds off with the model unavailable and the header says so; `hit_shares` with similarity weights. Tool: `entities` over the binary with the fixture extraction loaded; provenance row. CLI: `doctor`, `query --entity`, `eval --no-models`. UI: overlay toggle, entity focus, detail panel mentions, status lines, axe. Eval: both question files load; the prose set runs under the gate with the checked-in extraction.

## 10. Non-goals

Alias merging beyond `norm_name`; query-time model calls beyond one embedding; a generated answer; reranking models; API embeddings; processes and journeys (sub-project 3); office, PDF and media sources (5, 6); extraction of code symbols; a chat UI; cross-workspace entities; tier-two changes.
