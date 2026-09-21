# Tier-one eval

Repo: honojs/hono at commit `098e11912ab244c5c33931de007f04dc8e3c2929`.
Run: `singularrag index --repo <hono checkout> && singularrag eval --repo <hono checkout> --questions eval/questions.toml`

Questions are fixed. Never edit an existing id; add new ids for new questions.

## Baseline (first run)

```
id    category  recall  missed
L1    locate     0.00  src/router.ts::Router, src/router.ts::match, src/router/reg-exp-router/router.ts::RegExpRouter, src/hono-base.ts::Hono
L2    locate     0.00  src/compose.ts::compose, src/compose.ts::dispatch, src/hono-base.ts::use
L3    locate     1.00  
T1    trace      0.00  src/hono-base.ts::use, src/router.ts::add, src/compose.ts::compose, src/compose.ts::dispatch
T2    trace      0.25  src/hono-base.ts::Hono, src/router.ts::match, src/request.ts::param
T3    trace      0.25  src/compose.ts::dispatch, src/http-exception.ts::HTTPException, src/http-exception.ts::getResponse
B1    blast      0.83  src/hono-base.ts::Hono
B2    blast      0.20  src/hono-base.ts::Hono, src/compose.ts::compose, src/compose.ts::dispatch, src/request.ts::HonoRequest
B3    blast      0.00  src/compose.ts::dispatch, src/hono-base.ts::use, src/middleware/logger/index.ts::logger, src/middleware/cors/index.ts::cors, src/middleware/jwt/jwt.ts::jwt
P1    placement  0.00  src/hono-base.ts::use, src/middleware/powered-by/index.ts::poweredBy, src/middleware/logger/index.ts::logger, src/middleware/etag/index.ts::etag
P2    placement  1.00  
P3    placement  0.75  src/context.ts::JSONRespond
mean recall 0.357 over 12 questions
```

Known weakness: test files with many identically named local constants (e.g. many `const request` helpers, or the standalone `class Context` in `src/middleware/cache/index.test.ts`) can flood the top of the ranked map and crowd out the `src/` definitions these questions are actually asking about.

## Baseline (after ranking fixes, 2026-09-20)

```
id    category  recall  missed
L1    locate     0.50  src/router.ts::match, src/hono-base.ts::Hono
L2    locate     1.00  
L3    locate     0.50  src/context.ts::JSONRespond, src/context.ts::NewResponse
T1    trace      0.50  src/router.ts::add, src/compose.ts::compose
T2    trace      0.50  src/hono-base.ts::Hono, src/request.ts::param
T3    trace      0.50  src/compose.ts::dispatch, src/http-exception.ts::HTTPException
B1    blast      0.67  src/router/smart-router/router.ts::match, src/hono-base.ts::Hono
B2    blast      0.20  src/hono-base.ts::Hono, src/compose.ts::compose, src/compose.ts::dispatch, src/request.ts::HonoRequest
B3    blast      0.20  src/compose.ts::dispatch, src/middleware/logger/index.ts::logger, src/middleware/cors/index.ts::cors, src/middleware/jwt/jwt.ts::jwt
P1    placement  0.00  src/hono-base.ts::use, src/middleware/powered-by/index.ts::poweredBy, src/middleware/logger/index.ts::logger, src/middleware/etag/index.ts::etag
P2    placement  1.00  
P3    placement  0.75  src/context.ts::JSONRespond
mean recall 0.526 over 12 questions
```

Mean recall 0.357 → 0.526 on the same questions and the same commit, from four
changes: a file's rank is now divided among the symbols sharing a name instead of
replicated to each of them, an FTS bonus is divided among a file's hits, references
from a symbol's own file count for its score and its reasons, and `symbols_fts` is
porter-stemmed (so "composed" reaches `compose` and carries the ×10 query-identifier
multiplier with it). The schema bump to v2 means the first run after this change
rebuilds the index.

Remaining weakness: the question set's hardest misses are all one symbol —
`src/hono-base.ts::Hono`, a class defined in a file that mostly *makes* references
rather than receiving them, so PageRank never lifts it. P1 ("where should a new
middleware go") is still 0.00: the middleware packages it asks for are leaves that
nothing in `src/` references.

## Baseline (tests never served, header references, 2026-09-21)

```
id    category  recall  tokens  missed
L1    locate     0.50    1071  src/router.ts::match, src/hono-base.ts::Hono
L2    locate     1.00    1047  
L3    locate     0.50    1053  src/context.ts::JSONRespond, src/context.ts::NewResponse
T1    trace      0.50    1046  src/router.ts::add, src/compose.ts::compose
T2    trace      0.50    1071  src/hono-base.ts::Hono, src/request.ts::param
T3    trace      0.50    1064  src/compose.ts::dispatch, src/http-exception.ts::HTTPException
B1    blast      0.83    1059  src/hono-base.ts::Hono
B2    blast      0.20    1057  src/hono-base.ts::Hono, src/compose.ts::compose, src/compose.ts::dispatch, src/request.ts::HonoRequest
B3    blast      0.20    1065  src/compose.ts::dispatch, src/middleware/logger/index.ts::logger, src/middleware/cors/index.ts::cors, src/middleware/jwt/jwt.ts::jwt
P1    placement  0.00    1058  src/hono-base.ts::use, src/middleware/powered-by/index.ts::poweredBy, src/middleware/logger/index.ts::logger, src/middleware/etag/index.ts::etag
P2    placement  1.00    1039  
P3    placement  0.50    1072  src/context.ts::NewResponse, src/context.ts::JSONRespond
mean recall 0.519 over 12 questions
```

Two changes after the first full tier-two run (below): symbols in test, spec and benchmark
files are never served (their files stay in the graph as referrers), and each file header
ends with the files that reference it (`src/router.ts:  ← src/hono-base.ts, src/router/smart-router/router.ts +3`).
The report gained a `tokens` column, the map's size, so a budget's recall carries its cost.

Mean recall by budget, before → after: 1024 0.526 → 0.519; 2048 0.640 → 0.658; 4096 0.750 → 0.792.
The steps on the way there, all at 4096: references on every row cost a third of the
symbols (0.62); one file per row still 0.69; references on the header alone 0.69, because
hono's groups average two symbols per file; excluding tests from the graph as well fell to
0.63, because tests are the strongest referrers of the public API; tests as referrers but
never rows gave 0.79. Of the 387 indexed files, 134 are tests and 48 benchmarks; before the
filter they were 72 of the 168 rows a 4096-token map served for L1, led by
`src/jsx/dom/index.test.tsx`.

## Tier two

Fixtures under `crates/singularrag-bench/tests/fixtures/` are recorded streams with identifiers removed.

Smoke run 2026-09-20 (one question, one repeat, two conditions; the run directory was outside the repo):

#### Tier-two run smoke

commit 098e11912ab244c5c33931de007f04dc8e3c2929 · claude 2.1.261 (Claude Code) · singularrag singularrag 0.1.0 · models: claude-opus-5[1m]
tokens = input + output + cache creation + cache read

| condition | sessions | failed | mean recall | mean tokens | median tokens | mean tool calls | mean wall s | total cost |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| alone | 1 | 0 | 0.50 | 108254 | 108254 | 9.0 | 24.4 | $0.24 |
| singularrag | 1 | 0 | 0.50 | 63849 | 63849 | 6.0 | 22.1 | $0.22 |

| question | alone | singularrag |
|---|---:|---:|
| L1 | 0.50 | 0.50 |

**singularrag earns its place** against alone.

The first smoke attempt ran without `--allowedTools` and both singularrag tool calls were denied by permission, which is why the harness now allows the condition's MCP servers and aborts a condition on any denial.

Serena's warm-up (`serena project index`) prompts interactively when the checkout has no `.serena/project.yml`, so a fresh checkout needs a one-time `uvx --from git+https://github.com/oraios/serena serena project create --language typescript <checkout>` first (`create` refuses to run on an existing project, which is why it is not the warm-up).

### Full run 2026-09-21 (12 questions × 3 conditions × 3 repeats = 108 sessions)

Summary: `runs/20260921T065656Z-full/summary.md`. Model `claude-opus-5[1m]`, Claude Code 2.1.261, singularrag 0.1.0 at main 195d5fe.

| condition | sessions | failed | mean recall | mean tokens | mean tool calls | total cost |
|---|---:|---:|---:|---:|---:|---:|
| alone | 36 | 0 | 0.46 | 85500 | 10.4 | $8.13 |
| singularrag | 36 | 0 | 0.53 | 102299 | 9.6 | $9.45 |
| serena | 36 | 1 | 0.41 | 129488 | 10.0 | $7.72 |

**Verdict: singularrag does not earn its place.** Recall is up (0.53 vs 0.46, and it beats or ties the baseline on 10 of 12 questions), but tokens are up 19.6% and tool calls down only 8.3%; the rule needs 25% down on either. Serena fails on both axes and one Serena session (B1#3) hit the $0.50 budget cap.

Where the tokens went, from the records:

- The agent does not read less with the map. Reads are flat (155 with singularrag, 154 alone); the map replaced Grep (111 vs 176) and Glob (7 vs 45), which are the cheap calls. Trace, blast and placement questions want bodies, and a map of signatures does not remove that.
- The map is large and paid on every later turn. `repo_map` was called 34 times with agent-chosen budgets of 2048 to 4096 tokens; the mean result is 12.2k characters (about 3k tokens), and each of the roughly ten turns that follow re-reads it as cache-read input. Mean cache-read input per session: 84k with singularrag against 71k alone. That gap is the whole token loss.
- `find_symbol` (37 calls, mean 1.3k characters) is the cheap tool and is used as often as `repo_map`.

Question-level: the largest gains are P1 (0.08 → 0.33), B3 (0.07 → 0.20), P2 and T1; the one loss is L1 (0.42 → 0.17), where all three singularrag sessions missed `src/hono-base.ts::Hono` and `src/router/reg-exp-router/router.ts::RegExpRouter`, and two of three missed `src/router.ts::match`; `Hono` and `match` are the same symbols tier one misses on L1.

What the records said on a second look, and what the next run tests:

- The default budget was never used. It is already 1024; the agent chose 2048, 3000 or 4096 in every one of its 34 map calls, one call per session. Tier one says wider maps are better maps (0.53 at 1024, 0.75 at 4096), so the choice was rational. Shrinking the map cannot pass the rule: removing it entirely only returns to parity, because the token gap is the map itself being re-read on every later turn.
- The map already held the answer. Per session it served 0.76 of the gold and the agent's answer kept 0.55: of 108 gold symbols handed to it, the agent dropped 44, then read files to confirm what it had been told. On B1 the map served all six `match` implementations and both conditions answered with the router classes instead (gold granularity, not retrieval).
- The grader was strict. 15 to 21% of answers were `Class.method`, `Node/search` or `#name`; scored as declared names, alone is 0.485, singularrag 0.564, serena 0.461. Same order, slightly wider gap. The grader now reduces qualified names (the committed summary above is the strict one).

So the second run changes what the agent is told, not how much: file headers name who references the file, test and benchmark rows are gone, and the tool description says to answer from the map and read only to confirm. The rerun is `alone` and `singularrag` only, same config and model; Serena's numbers stand.
