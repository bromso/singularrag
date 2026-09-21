# Tier-two run /Users/jonasbroms/Sites/singularrag-bench-runs/runs/20260921T083419Z-round2

commit 098e11912ab244c5c33931de007f04dc8e3c2929 · claude 2.1.261 (Claude Code) · singularrag 0.1.0 · models: claude-opus-5[1m]
tokens = input + output + cache creation + cache read

| condition | sessions | failed | mean recall | mean tokens | median tokens | mean tool calls | mean wall s | total cost |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| alone | 36 | 0 | 0.52 | 87134 | 86121 | 10.7 | 32.6 | $8.12 |
| singularrag | 36 | 0 | 0.60 | 92695 | 94746 | 8.6 | 29.4 | $8.79 |

| question | alone | singularrag |
|---|---:|---:|
| L1 | 0.42 | 0.50 |
| L2 | 1.00 | 1.00 |
| L3 | 0.25 | 0.42 |
| T1 | 0.75 | 0.92 |
| T2 | 0.58 | 0.75 |
| T3 | 0.92 | 0.92 |
| B1 | 0.00 | 0.11 |
| B2 | 0.60 | 0.53 |
| B3 | 0.07 | 0.20 |
| P1 | 0.17 | 0.25 |
| P2 | 0.75 | 0.92 |
| P3 | 0.75 | 0.67 |

**singularrag earns its place** against alone.
- recall +0.08 [+0.02, +0.14] over 36 pairs
- tool calls -2.1 [-3.3, -0.9]
- tokens +5561 (+6.4%) [-6.7%, +19.5%]

(Under the rule in force when the run was made, tokens or tool calls 25% below the baseline: "does not earn its place: tokens +6.4%, tool calls -19.8%". Re-scored 2026-09-21 under the amended §12 rule.)
