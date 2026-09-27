# Tier-two run /private/tmp/claude-501/-Users-jonasbroms-Sites-singularrag/83926a0e-4b8b-4d58-996f-9cfd64eefec6/scratchpad/runs/20260927T150030Z-docs

commit 0c2ed54eceeea1327a3822cb6c0644ec0da0b2e5 · claude 2.1.280 (Claude Code) · singularrag 0.1.0 · models: claude-opus-5-5[1m]
tokens = input + output + cache creation + cache read; cost = input + 1.25 × cache creation + 0.1 × cache read + 5 × output (price-weighted tokens; the efficiency test)

| condition | sessions | failed | hook denials | mean recall | mean tokens | median tokens | mean cost | mean tool calls | mean wall s | total cost |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| alone | 84 | 0 | 0 | 0.94 | 28416 | 19118 | 10921 | 3.4 | 11.7 | $4.20 |
| singularrag | 84 | 0 | 0 | 0.94 | 47808 | 41082 | 18262 | 3.3 | 11.4 | $7.65 |

| question | alone | singularrag |
|---|---:|---:|
| D1 | 1.00 | 1.00 |
| D2 | 1.00 | 1.00 |
| D3 | 1.00 | 1.00 |
| D4 | 1.00 | 1.00 |
| D5 | 1.00 | 0.78 |
| D6 | 1.00 | 1.00 |
| D7 | 0.67 | 0.67 |
| D8 | 1.00 | 1.00 |
| D9 | 0.50 | 0.67 |
| D10 | 0.67 | 0.67 |
| D11 | 1.00 | 1.00 |
| D12 | 0.50 | 0.67 |
| P1 | 1.00 | 1.00 |
| P2 | 1.00 | 1.00 |
| P3 | 1.00 | 1.00 |
| P4 | 1.00 | 1.00 |
| E1 | 1.00 | 1.00 |
| E2 | 1.00 | 1.00 |
| E3 | 1.00 | 1.00 |
| E4 | 1.00 | 1.00 |
| R1 | 1.00 | 1.00 |
| R2 | 1.00 | 1.00 |
| R3 | 1.00 | 1.00 |
| R4 | 1.00 | 1.00 |
| S1 | 1.00 | 1.00 |
| S2 | 1.00 | 1.00 |
| S3 | 1.00 | 1.00 |
| S4 | 1.00 | 1.00 |

singularrag does not earn its place: correctness: recall +0.00 [-0.02, +0.03]; the interval must lie above 0; efficiency: tool calls -0.1 [-0.5, +0.3]; the interval must lie below 0, cost +67.2%; at most +10%
- recall +0.00 [-0.02, +0.03] over 84 pairs
- tool calls -0.1 [-0.5, +0.3]
- cost +7340 (+67.2%) [+51.9%, +82.5%]
- tokens +19392 (+68.2%) [+53.2%, +83.3%]
