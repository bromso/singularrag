# singularrag walks design (the product reframe and its gate)

**Status:** v0 design, approved in brainstorm 2026-10-12.
**Parent:** `docs/superpowers/specs/2026-09-19-singularrag-design.md`, as amended by the documents (2026-09-23), knowledge (2026-09-23) and journeys (2026-09-24) designs. This spec amends the parent's §2 and §12 (see §8).

## Context

Five tier-two runs on three corpora (hono, this repository with its documents, the GitLab handbook's `people-group` pages; `eval/README.md` "Tier two") say the same thing: a coding agent with grep and a few turns does not need a ranked map or a knowledge graph to answer the questions a person would bring to a codebase or a document set, however hard those questions are for a single ranked map. On the handbook every one of sixteen questions scored 1.00 alone and with singularrag, at +46% cost with the tools. On the same set tier one, the retrieval gate, went from 0.688 to 0.938 at 4096 with the knowledge layer: the retrieval is good; the agent does not need it. The thesis the parent spec was built on, a shared map that pays for itself in an agent's context, is therefore retired as the gate. What has never been judged is the other half of the product, built across the last three sub-projects: the map, the knowledge graph, Journeys and the retrieval panel, for a person.

The brainstorm settled three things: singularrag is for a person understanding a corpus; it is judged by scripted walks through its UI's own data paths, deterministic and tracked per release, with tier one kept as the retrieval gate underneath; and the next source is a codebase with its database, scoped against the walks. This spec is the gate and the reframe. Sub-project 4 gets its own spec afterwards.

## 1. Product statement

singularrag is a local tool for a person understanding a corpus: a repository, a handbook, a notes folder, or several together in one workspace. It indexes code and documents, extracts people, systems, concepts, processes and their steps with a local model, and shows the result as a ranked map, a knowledge graph, Journeys, and a retrieval panel with reasons. Everything served has provenance: a section or symbol with lines, and a reason the ranking can draw. The person's questions are the unit of value; the gate is whether the UI surfaces the answer.

Retired as a gate: tier two. Its runs stay as the record and its harness stays in the repository; no release is judged by it and no work is scheduled to improve it. Frozen, not removed: the six MCP tools, the hook and `init`. They keep working and keep their tests, get bug fixes and no features, and their descriptions are shortened once (§7). Kept, unchanged: tier one, because it measures the retrieval the walks depend on.

## 2. What a walk is

A walk is one question a person brings, the path the UI offers for it, and a deterministic check that the path ends at the answer. Gold is always a section or symbol as `path::name` (the tier-one convention), never text. Four kinds, one per perspective:

| kind | the question | the path | the check | the bound |
|---|---|---|---|---|
| `locate` | where is X described or defined | the retrieval panel: the question at a budget | a gold item is among the served items | the budget (`budget_tokens`, the walk file's default or the walk's own) |
| `process` | how does X work; who does what; what comes after Y | Journeys: filter by the role the walk names, if any; select the process | a process exists whose steps cite a gold section; when the walk names a `role` or a `step_text` substring, a step of that process has it | the processes the UI lists (the route's cap) |
| `connection` | what does A have to do with B; what touches X | the entity or symbol in the detail panel | the gold neighbour is among the entity's relations or mentions, or among the symbol's referrers or blast radius | `hops` (default 2) |
| `change` | what would this diff touch | the change view, with a prepared patch applied | the gold symbols and sections are in the changed list, with their referrers | the patch |

A walk fails when the path cannot reach the answer within its bound, never because of wording. A `process` walk passes through a process the model named differently from the document, as long as its steps cite the gold section. A `connection` walk names its start (`from`, an entity name or a `path::name` symbol) and its gold neighbours; the check follows relations and mentions for an entity, references and blast radius for a symbol, up to `hops`. A `change` walk names a patch file beside the walk file; the runner applies it to the corpus checkout, runs the change view's query, and reverts the checkout (§3).

Walk files are TOML like the tier-one sets, one per corpus, with the same rule: never edit an existing id, add new ones.

```toml
budget = 2048            # default for locate walks

[[walk]]
id = "W1"
kind = "locate"
query = "how do I get reimbursed after paying for something myself"
gold = ["docs/handbook.md::Expense process"]

[[walk]]
id = "W2"
kind = "process"
query = "what happens after my manager approves an expense claim"
role = "Finance"
step_text = "within 5 days"
gold = ["docs/handbook.md::Expense process"]

[[walk]]
id = "W3"
kind = "connection"
from = "Okta"
hops = 2
gold = ["docs/handbook.md::Onboarding"]

[[walk]]
id = "W4"
kind = "change"
patch = "patches/W4.patch"
gold = ["src/auth/session.ts::createSession", "docs/design.md::Freshness"]
```

## 3. The harness and the grader

`singularrag walk --questions <file> [--budget <N>] [--no-models] [--extraction <json>] [--json]`, in the CLI crate, so it runs exactly what the UI runs (`--extraction` loads a checked-in extraction first, as `eval` does, so the CI file runs with no model):

- `locate`: the retrieval panel's query path (the engine's `repo_map` under the session key the panel uses, recorded like a panel query), served items compared to gold.
- `process`: the processes query behind `GET /api/processes` (`queries::processes`), the same cap; the step and role checks run on its DTO.
- `connection`: for an entity start, the entities query behind `GET /api/entities` (relations and mentions); for a symbol start, the references and blast-radius queries behind the detail panel; breadth-first to `hops`.
- `change`: `git apply --check` then `git apply` of the patch in the corpus checkout, the engine's `changed` call with no base (working tree against HEAD), then `git checkout -- <paths>` and `git clean -fd -- <paths>` for the paths the patch names. A dirty checkout before the walk is a refusal, not a run. Change walks therefore run only on a pinned clone of a corpus, never on a working repository.

No DOM and no browser: a walk that passes means the data the UI would render contains the answer. Output: one row per walk (`id`, `kind`, pass or fail, what the path reached: the served items, the process matched, the neighbours found, the changed list), a reason on every failing row (`gold not served at 2048`, `no process cites docs/handbook.md::Expense process`, `step with role Finance not found`, `Onboarding not within 2 hops of Okta`, `src/auth/session.ts::createSession not in the changed list`), and a footer `walks N/M` per kind and overall. `--json` writes the same for the record.

Models: `locate` embeds the query once when seeds are on; a `connection` walk's entity start resolves by name only and makes no model call (amended below); `--no-models` gives the seeds-off row as tier one does and so affects `locate` only. `process` and `change` never call a model. The walks run against an extracted index and never trigger extraction; `doctor` says whether the index is extracted.

*Amended 2026-10-15 (plan execution).* A connection walk's entity start resolves by `norm_name` only and makes no model call, because the entity panel the walk models is reached by a person choosing an entity by name, and a vector fallback would pass a walk through an entity the person could not find. `--no-models` therefore affects `locate` only.

*Amended 2026-10-16 (caps).* The `process` grader pages through the processes query (`queries::processes_page`, 200 a page, until `offset >= total`), so no cap hides a process; the entity start resolves through the entity search and detail queries (`GET /api/entities/search`, exact `norm_name` matches only, then `GET /api/entities/{id}` per hop), the panel's own path, instead of the capped overlay behind `GET /api/entities`.

Lives in `crates/singularrag/src/walk.rs` (runner, grader, report) with the walk file types in `crates/singularrag-core/src/walks.rs` (parse, validate: ids unique, gold non-empty, kinds known, `from` present for `connection`, `patch` present for `change`).

## 4. Corpora and the baseline

Three walk files at the start, twelve walks each, three per kind where the corpus allows it:

- `eval/walks-repo.toml`: this repository at the documents pin: `../singularrag-pinned` (`corpus` branch, commit `0c2ed54`, the `5ec066b` tree plus the two prose fixture files; paths relative to the main checkout root), code and documents together; the file with `change` walks on code and documents, patches under `eval/patches/`.
- `eval/walks-handbook.toml`: the handbook `people-group` corpus (`../../handbook-corpus`, extracted): `locate`, `process`, `connection`; no `change`.
- `eval/walks-hono.toml`: hono at its pin: `locate`, `connection`, `change`; it has no extraction and shows what the walks say about a plain codebase.

The baseline is the first full run of all three, seeds on and off, recorded in `eval/README.md` under "Walks" like the tier-one rounds, and it is allowed to be poor: a walk that fails at the baseline is a lead for the roadmap, not a bug to hide. A CI walk file on the prose fixture plus `write_docs_mini` (`eval/walks-ci.toml`, all four kinds, the checked-in extraction loaded with `--extraction`, one patch) runs in `cargo test` with no model and pins the harness itself.

## 5. The gate rule

Per release: no walk that passed still fails; tier one holds its recorded values within 0.02 (hono, docs, prose, handbook sets); a sub-project ships with walks of its own, on a fixture and on a real corpus, that pass. Extraction is a background cost (the handbook's 983 sections took about three hours) and is acceptable for a corpus a person works with for weeks.

What the gate does not promise: that the UI is pleasant or fast. The walks say the answer is reachable; use says whether it is findable. Both go in the record, the second as dated notes in the eval README.

## 6. UI

No change in this spec. The walks measure the data behind the perspectives; a walk that fails at the baseline because the path exists in the data but not in the UI (a neighbour the panel does not list, a process Journeys cannot reach by role) is a finding for the next plan.

## 7. The agent tools and tier two

The six MCP tools, the hook and `init` stay as they are. One change: `INSTRUCTIONS` and the six tool descriptions are shortened to the minimum that names each tool, its arguments and when to call it (a sentence or two each; today they are about 1.8k tokens a session), with the byte-identical test copies updated in the same commit. The tier-two harness keeps building; `eval/README.md`'s "Tier two" section opens with one paragraph saying it is no longer a release gate and why, pointing at the handbook run.

## 8. Parent spec amendments

- §2 (target user and job): the person understanding a corpus is the user; the agent is a consumer of the same index, no longer the job.
- §3 (verdict on the shared-map hypothesis): the hypothesis is recorded as not holding for an agent with grep on corpora up to a thousand sections, with the five runs as the evidence.
- §12 (eval): tier one plus walks; tier two retired as a gate.

## 9. Security

Nothing new leaves the machine: the walks call the local engine and queries; `locate` and `connection` make the one query embedding the panel makes. The `change` runner writes to a corpus checkout only after `git apply --check` passes and only on a clean tree, and reverts exactly the paths the patch names; it refuses a dirty tree and never runs on the repository that holds the walk file.

## 10. Testing

Walk file parsing and validation (unique ids, kinds, required fields per kind); each kind's grader against hand-built DTOs (served list with and without gold; a process whose steps cite gold with and without the named role; neighbours at one and three hops against a bound of two; a changed list with and without the gold); the runner on `walks-ci.toml` with the prose extraction loaded (all four kinds pass; a deliberately wrong gold fails with the stated reason); the `change` runner's refusal on a dirty tree and its revert on success; the report's row and footer format; the CLI test that `singularrag walk --json` on the CI file prints `walks 4/4`. The shortened tool descriptions keep the byte-identical tests.

## 11. Non-goals

A browser-driven UI test; timing or usability measures; new perspectives or UI changes; new sources (sub-project 4 follows); re-running tier two; removing the MCP tools; a model in the grader.
