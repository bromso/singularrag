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
