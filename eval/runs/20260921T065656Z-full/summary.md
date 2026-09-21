# Tier-two run /Users/jonasbroms/Sites/singularrag-bench-runs/runs/20260921T065656Z-full

commit 098e11912ab244c5c33931de007f04dc8e3c2929 · claude 2.1.261 (Claude Code) · singularrag 0.1.0 · models: claude-opus-5[1m]
tokens = input + output + cache creation + cache read

| condition | sessions | failed | mean recall | mean tokens | median tokens | mean tool calls | mean wall s | total cost |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| alone | 36 | 0 | 0.46 | 85500 | 77418 | 10.4 | 32.5 | $8.13 |
| singularrag | 36 | 0 | 0.53 | 102299 | 92336 | 9.6 | 32.3 | $9.45 |
| serena | 36 | 1 | 0.41 | 129488 | 118236 | 10.0 | 30.6 | $7.72 |

| question | alone | singularrag | serena |
|---|---:|---:|---:|
| L1 | 0.42 | 0.17 | 0.33 |
| L2 | 0.89 | 1.00 | 0.67 |
| L3 | 0.33 | 0.42 | 0.42 |
| T1 | 0.67 | 0.83 | 0.67 |
| T2 | 0.42 | 0.50 | 0.08 |
| T3 | 0.92 | 0.83 | 0.67 |
| B1 | 0.06 | 0.06 | 0.06 |
| B2 | 0.53 | 0.60 | 0.53 |
| B3 | 0.07 | 0.20 | 0.00 |
| P1 | 0.08 | 0.33 | 0.08 |
| P2 | 0.58 | 0.75 | 0.58 |
| P3 | 0.58 | 0.67 | 0.83 |

singularrag does not earn its place: efficiency: tokens 102299 vs 85500 (+19.6%), tool calls 9.6 vs 10.4 (-8.3%); need -25% on either
serena does not earn its place: correctness: recall 0.41 vs 0.46 (need >= 0.44); efficiency: tokens 129488 vs 85500 (+51.4%), tool calls 10.0 vs 10.4 (-3.7%); need -25% on either
