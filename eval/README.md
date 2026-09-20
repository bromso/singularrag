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

## Tier two

Fixtures under `crates/singularrag-bench/tests/fixtures/` are recorded streams with identifiers removed.

Smoke run 2026-09-20 (one question, one repeat, two conditions; the run directory was outside the repo):

# Tier-two run smoke

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
