# Tier-two run /private/tmp/claude-501/-Users-jonasbroms-Sites-singularrag/83926a0e-4b8b-4d58-996f-9cfd64eefec6/scratchpad/runs/20260923T175126Z-docs-hook

commit 098e11912ab244c5c33931de007f04dc8e3c2929 · claude 2.1.280 (Claude Code) · singularrag 0.1.0 · models: claude-opus-5-5[1m]
tokens = input + output + cache creation + cache read

| condition | sessions | failed | hook denials | mean recall | mean tokens | median tokens | mean tool calls | mean wall s | total cost |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| alone | 36 | 0 | 0 | 0.50 | 24917 | 21194 | 3.4 | 13.4 | $1.82 |
| singularrag | 36 | 0 | 0 | 0.63 | 46704 | 44018 | 4.0 | 15.2 | $2.96 |
| singularrag+hook | 36 | 0 | 1 | 0.60 | 45432 | 43846 | 3.7 | 13.5 | $2.77 |

| question | alone | singularrag | singularrag+hook |
|---|---:|---:|---:|
| L1 | 0.50 | 0.58 | 0.50 |
| L2 | 0.44 | 1.00 | 0.89 |
| L3 | 0.42 | 0.50 | 0.50 |
| T1 | 0.50 | 0.92 | 1.00 |
| T2 | 0.50 | 0.58 | 0.58 |
| T3 | 1.00 | 0.92 | 1.00 |
| B1 | 0.22 | 0.28 | 0.28 |
| B2 | 0.33 | 0.53 | 0.33 |
| B3 | 0.20 | 0.20 | 0.20 |
| P1 | 0.25 | 0.25 | 0.08 |
| P2 | 1.00 | 1.00 | 1.00 |
| P3 | 0.67 | 0.83 | 0.83 |

singularrag does not earn its place: efficiency: tool calls +0.6 [-0.2, +1.3]; the interval must lie below 0, tokens +87.4%; at most +10%
- recall +0.13 [+0.04, +0.22] over 36 pairs
- tool calls +0.6 [-0.2, +1.3]
- tokens +21787 (+87.4%) [+64.6%, +110.3%]
singularrag+hook does not earn its place: efficiency: tool calls +0.2 [-0.5, +0.9]; the interval must lie below 0, tokens +82.3%; at most +10%
- recall +0.10 [+0.00, +0.19] over 36 pairs
- tool calls +0.2 [-0.5, +0.9]
- tokens +20515 (+82.3%) [+60.2%, +104.5%]
