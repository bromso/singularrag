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

singularrag does not earn its place: correctness: recall +0.07 [-0.01, +0.14]; the interval must lie above 0; efficiency: tool calls -0.9 [-1.7, +0.0]; the interval must lie below 0, tokens +19.6%; at most +10%
- recall +0.07 [-0.01, +0.14] over 36 pairs
- tool calls -0.9 [-1.7, +0.0]
- tokens +16799 (+19.6%) [+6.0%, +33.3%]
serena does not earn its place: correctness: recall -0.05 [-0.13, +0.02]; the interval must lie above 0; efficiency: tool calls -0.4 [-1.6, +0.8]; the interval must lie below 0, tokens +51.4%; at most +10%
- recall -0.05 [-0.13, +0.02] over 36 pairs
- tool calls -0.4 [-1.6, +0.8]
- tokens +43988 (+51.4%) [+33.3%, +69.6%]

(Under the rule in force when the run was made, tokens or tool calls 25% below the baseline, both conditions also failed: singularrag on tokens +19.6% and tool calls -8.3%, serena on recall 0.41 vs 0.46 and tokens +51.4%. Re-scored 2026-09-21 under the amended §12 rule with the strict grader's recalls.)
