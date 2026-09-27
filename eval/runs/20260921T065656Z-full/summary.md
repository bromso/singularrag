# Tier-two run /Users/jonasbroms/Sites/singularrag-bench-runs/runs/20260921T065656Z-full

commit 098e11912ab244c5c33931de007f04dc8e3c2929 · claude 2.1.261 (Claude Code) · singularrag 0.1.0 · models: claude-opus-5[1m]
tokens = input + output + cache creation + cache read; cost = input + 1.25 × cache creation + 0.1 × cache read + 5 × output (price-weighted tokens; the efficiency test)

| condition | sessions | failed | hook denials | mean recall | mean tokens | median tokens | mean cost | mean tool calls | mean wall s | total cost |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| alone | 36 | 0 | 0 | 0.46 | 85500 | 77418 | 35766 | 10.4 | 32.5 | $8.13 |
| singularrag | 36 | 0 | 0 | 0.53 | 102299 | 92336 | 40709 | 9.6 | 32.3 | $9.45 |
| serena | 36 | 1 | 0 | 0.41 | 129488 | 118236 | 35565 | 10.0 | 30.6 | $7.72 |

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

singularrag does not earn its place: correctness: recall +0.07 [-0.01, +0.14]; the interval must lie above 0; efficiency: tool calls -0.9 [-1.7, +0.0]; the interval must lie below 0, cost +13.8%; at most +10%
- recall +0.07 [-0.01, +0.14] over 36 pairs
- tool calls -0.9 [-1.7, +0.0]
- cost +4943 (+13.8%) [+6.0%, +21.7%]
- tokens +16799 (+19.6%) [+6.0%, +33.3%]
serena does not earn its place: correctness: recall -0.05 [-0.13, +0.02]; the interval must lie above 0; efficiency: tool calls -0.4 [-1.6, +0.8]; the interval must lie below 0
- recall -0.05 [-0.13, +0.02] over 36 pairs
- tool calls -0.4 [-1.6, +0.8]
- cost -201 (-0.6%) [-9.6%, +8.5%]
- tokens +43988 (+51.4%) [+33.3%, +69.6%]
