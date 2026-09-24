# singularrag journeys design (sub-project 3: processes, steps, roles)

**Status:** v0 design, approved in brainstorm 2026-09-24.
**Parent:** `docs/superpowers/specs/2026-09-19-singularrag-design.md`, as amended by the documents design (`2026-09-23-singularrag-documents-design.md`, which fixes this sub-project's outline in its decomposition item 3) and the knowledge design (`2026-09-23-singularrag-knowledge-design.md`, whose extraction job, entity stores, seeds and `entities` tool this spec extends).

## Context

The knowledge design gave the corpus a graph of people, systems and concepts with provenance to sections. What it cannot answer is order: what happens first, who acts at each step, and which code carries the step. Jonas's Journeys perspective is that question for the human; the same data answers it for the agent. The brainstorm settled three forks: a journey is one process extracted from documents (nothing authored by hand); a step links to code deterministically through the systems it names and the mentions its section already has; the agent asks through `entities`, so the tool count stays six.

## 1. Goal

A handbook-style section such as "Expense process" becomes a process with ordered steps, each with the role who acts, the systems touched, the section that documents it and the code that implements it. The agent gets the steps from `entities`; the human gets a Journeys view, filterable by role; every step and every code link is explainable from the index alone.

## 2. Ontology and storage

Two entity types join the closed list: `process` ("a named sequence of steps people follow") and `role` ("a job or position that acts in a process"). `system` already exists. A process is an ordinary entity: description, mentions, relations, vector, map node and `entities` answer come for free. Order is new.

Schema version 6 adds, in `index.db`:

| table | columns | notes |
|---|---|---|
| `steps` | `id`, `process_id` → `entities.id`, `ordinal INTEGER`, `text TEXT`, `role_id` → `entities.id` nullable, `role_text TEXT`, `symbol_id`, `section_hash`, unique `(process_id, symbol_id, ordinal)` | one row per step per documenting section; `text` capped at 300 characters |
| `step_systems` | `step_id`, `entity_id`, primary key `(step_id, entity_id)` | the systems a step touches |
| `steps_cache` | `hash TEXT PRIMARY KEY`, `json TEXT` | the steps prompt's answer per section hash, so a re-indexed section never re-asks |

Rows follow the section: the existing `delete_for_symbols` and the hash-mismatch cleanup delete `steps` and `step_systems` with the mentions and relations of the same section. A process whose last step row is gone stays an entity until its last mention is gone, like any entity.

The two ontology relations are derived, never stored: **documented in** is the step's `symbol_id`; **implemented by** is computed at read time (§4). Derived links cannot rot and always have a reason.

Not built: a journey object above processes (a role filter over processes shows what one role walks); stored code links; editing.

## 3. Extraction

- `ENTITY_TYPES` gains `process` and `role`; `EXTRACT_PROMPT` lists them with the one-line definitions above, and `fixtures/prose/extraction.json` is re-checked so the handbook's four sections each carry a `process` entity and its roles.
- **Gate.** A section is process-like only when its own extraction produced at least one `process` entity. Only then does the tick run a second prompt, `STEPS_PROMPT`, for that section: input the same text, heading and path (same 6,000-character parts rule, parts share the symbol id and their ordinals continue); output JSON `{ "process": string, "steps": [{ "text": string, "role": string, "systems": [string] }] }` in reading order. Same rules as the first prompt: `format: "json"`, one strict retry on malformed JSON, then `attempts` increments on the queue row and the section waits; every failure is `ModelUnavailable`; the same 30 s timeout, no retry on timeout inside a tick.
- **Placement.** The steps call runs inside the same tick, in the same claim, right after `apply_extraction` for that section, and counts against the same 20 s budget; a section is deleted from the queue only after both prompts have been applied (a `stage` column on `extract_queue`, `0` = entities pending, `1` = steps pending, so an outage between the two resumes at the steps prompt). The header's ` · entities: N pending` therefore covers both; no new segment.
- **Cache.** `steps_cache` is keyed by section hash and written the moment an answer parses; a cache hit applies without a model call (the same duplicate-section rule as `extraction_cache`).
- **Resolution.** `process` must match a `process` entity of that section by `norm_name`; otherwise the answer is discarded and the section is marked done (a warn line, no retry). Each `role` resolves to a `role` entity of the section by `norm_name` when one exists, else `role_id` is null and `role_text` keeps the text (capped at 80). Each system resolves to a `system` entity of the section by `norm_name`; unresolvable systems are dropped. Empty step text drops the step; more than 30 steps are truncated to 30.
- **Loader.** `load_extraction_json` applies a `steps` block per section key from the fixture with the same resolution, so unit tests, the eval and the UI tests never call a model.

Cost of the gate being wrong: a process described without the model typing it `process` gets no steps. The fixture's four sections are the test of the prompt wording.

## 4. Retrieval and the agent

- **Implemented by** (derived, read time): for a step, the union of (a) the code symbols the step's section already mentions (the documents design's doc-to-code `refs`) and (b) for each touched system with a `norm_name` of at least three characters, every code symbol or config key whose name, and every file whose stem, contains that name after normalisation (lowercase, hyphens and underscores removed): `okta` matches `oktaClient`, `okta_login.ts`, `auth.okta.issuer`. Ranked (a) first, then (b) by the number of files that reference the symbol's name, then path and name (the index stores no static rank; `file_rank` is computed per query); at most three shown per step, the count of the rest stated. A rule module `journeys::implemented_by(store, step) -> Vec<CodeLink>` with `CodeLink { path, name, line, via: Via::Mention | Via::System(String) }`.
- **`entities` output.** When a matched entity is a `process`, its block is followed by its steps in ordinal order, merged across documenting sections by `(symbol_id line order, ordinal)`:

```
Expense process (process): how you get your own money back
  1. Submit the expense in Expensify within a month — role: employee — systems: Expensify — docs/handbook.md::Expense process
  3. Finance reviews within 5 days — role: Finance — code: src/payroll/expense.ts::approveClaim (+2)
```

  A step line has `text — role: … — systems: … — <path::heading>` and, when links exist, `code: path::name (+N)`. Served items gain the linked code symbols after the cited sections, in rank order, so the retrieval records them and the rail can show them. The footer counts steps: `# N entities · M relations · S steps · K sections`.
- **Seed.** In `repo_map`, a matched `process` entity's touched systems seed their implementing code (rule (b) above) with `NOTE_BOOST`, and the file's hit weight is 1.0; `Reasons` gains `implements: Vec<String>` (the system names), rendered "Implements Expensify". The existing entity seed already covers the process's own sections.
- **Descriptions.** `ENTITIES_DESCRIPTION` gains one sentence: a process comes back as ordered steps with the role, systems, documenting section and implementing code. `INSTRUCTIONS` gains half a line: ask `entities` for how a process works. The MCP schema, `find_symbol`, `trace_path`, `changed`, `annotate` and the hook are unchanged.

## 5. UI: the Journeys perspective

- A third value on the view toggle: `journeys` ("Journeys"), persisted like the others. The overlay toggle is hidden in this view (no canvas).
- **Layout.** Left: a list of processes (`<ul>` of buttons: name, step count, the distinct roles), sorted by name, with a role filter (`<select>`, "All roles" plus every role name across steps) that hides processes no step of which has that role. Right: the selected process as a native `<ol>`; each `<li>` shows the step text, then `role`, `systems` as text with a type badge, a "Documented in" button that focuses the section row in the treegrid (the existing `onFocusSection` path), and up to three "Implemented by" buttons (path::name) that focus the code row, with "+N more" as text. Empty states: no processes yet ("No processes extracted yet" plus the pending count when non-zero); a process with no links.
- **Provenance.** Selecting a step does not record a retrieval; the rail is unchanged. The map is unchanged: processes are already entity nodes.
- **Accessibility.** The list and the `<ol>` carry `aria-label`s; the role filter is a labelled `<select>`; every link is a button with the target in its accessible name; focus moves to the treegrid row on "Documented in"; axe runs over the view with a process selected and with the role filter applied; colour is never the only signal (badges carry text).
- **Serve.** `GET /api/processes` returns `{ processes: [{ id, name, description, roles: [..], steps: [{ ordinal, text, role, systems: [{id,name}], section: { symbol_id, path, name, line }, code: [{ path, name, line, via }], more_code: N }] }], truncated }`, capped at 200 processes and 30 steps each, behind the per-run token and origin check, `Cache-Control: no-store`, `locked(&s, …)` on the read-only store. The UI fetches it on load and on every change event, alongside `/api/entities`.

## 6. Eval

- `eval/questions-prose.toml` gains a fourth category, `process`, four questions on the handbook whose answers are steps: which role approves an expense, what the release captain does after staging looks healthy, who writes the post-mortem and by when, which system opens every other tool. Gold are sections (`docs/handbook.md::Expense process` and so on), exactly as before. The `entities` run counts a question as cited when a gold section is among the served items.
- Corpus: the pinned `5ec066b` tree plus the two fixture files under `docs/` (knowledge spec §6 as amended). The checked-in extraction, with its `steps` block, is loaded for the `entities` run.
- Gate: hono and docs stay within 0.02 of their recorded values at 4096; the prose set's mean does not fall below its recorded seeds-off values (0.250 / 0.653 / 0.722 at 1024 / 2048 / 4096); all four `process` questions cite gold through `entities` with the checked-in steps loaded. Seeds-on stays open until Ollama is installed; `singularrag eval --no-models` remains the one-command seeds-off run.
- Vault: `~/.singularrag/questions-vault.toml` gains a `process` category; the README says how to write one (a gold section per process question, headings as `singularrag find` prints them).

## 7. Security

The steps prompt sends the same section text, heading and path to the same configured Ollama URL as the entity prompt; nothing else leaves the process, and secret-like files never reach the queue (unchanged). Code links are computed from the local index only. `GET /api/processes` sits behind the existing token and origin check and never writes. `annotate` remains the only write outside the store.

## 8. Parent spec amendments

- §7: schema version 6, the three tables, the `stage` column; the second prompt and its gate.
- §9: `entities` renders a process as ordered steps; `Reasons.implements`.
- §10: the Journeys perspective.
- §12: the `process` category and its gate.

## 9. Testing

Fake Ollama gains `set_steps(heading_substring, json)` (keyed like `set_extraction`) and counts `/api/generate` calls per prompt kind (the steps prompt is recognisable by its first line). Core: the gate (a section without a `process` entity makes no steps call; with one, exactly one), the `stage` resume after an outage between the prompts, `steps_cache` hit without a call, resolution (role by `norm_name`, `role_text` fallback, unresolvable system dropped, unmatched process discards the answer), truncation to 30 steps, deletion with the section, `implemented_by` (mention first, then system matches on symbol, config key and file stem; case and separator insensitivity; a two-character system matches nothing; the +N count), `entities` rendering with steps and the footer, the `implements` seed and reason, the fixture loader with the `steps` block. Serve: the route's shape, cap, token check. UI: view toggle with three options, the process list, the role filter, "Documented in" and "Implemented by" focus moves, both empty states, axe over the view. Eval: the four `process` questions resolve and cite gold with the fixture loaded. Global constraint as before: `INSTRUCTIONS` and every tool description in `server.rs` byte-identical to their `tests/mcp.rs` copies.

## 10. Non-goals

A journey object across processes; editing or reordering steps in the UI; model-matched or annotated code links; steps inferred from code (routes, handlers, state machines); process diagrams or swim lanes; cross-workspace processes; a seventh tool; tier-two changes; office, PDF and media sources (sub-projects 5, 6); database schemas (4).
