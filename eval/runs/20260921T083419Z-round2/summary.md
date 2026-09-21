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

singularrag does not earn its place: efficiency: tokens 92695 vs 87134 (+6.4%), tool calls 8.6 vs 10.7 (-19.8%); need -25% on either
