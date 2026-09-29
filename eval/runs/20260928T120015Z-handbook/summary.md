# Tier-two run /private/tmp/claude-501/-Users-jonasbroms-Sites-singularrag/83926a0e-4b8b-4d58-996f-9cfd64eefec6/scratchpad/runs/20260928T120015Z-handbook

commit 7cf88f91987b1bd43c878e2cadfec797ce1ad9c5 · claude 2.1.283 (Claude Code) · singularrag 0.1.0 · models: claude-opus-5-5
tokens = input + output + cache creation + cache read; cost = input + 1.25 × cache creation + 0.1 × cache read + 5 × output (price-weighted tokens; the efficiency test)

| condition | sessions | failed | hook denials | mean recall | mean tokens | median tokens | mean cost | mean tool calls | mean wall s | total cost |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| alone | 48 | 0 | 0 | 1.00 | 27401 | 27288 | 12826 | 2.9 | 13.3 | $2.96 |
| singularrag | 48 | 0 | 0 | 1.00 | 46377 | 46153 | 18690 | 3.2 | 14.5 | $4.39 |

| question | alone | singularrag |
|---|---:|---:|
| H2 | 1.00 | 1.00 |
| H3 | 1.00 | 1.00 |
| H4 | 1.00 | 1.00 |
| H5 | 1.00 | 1.00 |
| H6 | 1.00 | 1.00 |
| H7 | 1.00 | 1.00 |
| H8 | 1.00 | 1.00 |
| H10 | 1.00 | 1.00 |
| H11 | 1.00 | 1.00 |
| H12 | 1.00 | 1.00 |
| H13 | 1.00 | 1.00 |
| H14 | 1.00 | 1.00 |
| H16 | 1.00 | 1.00 |
| H17 | 1.00 | 1.00 |
| H18 | 1.00 | 1.00 |
| H19 | 1.00 | 1.00 |

singularrag does not earn its place: correctness: recall +0.00 [+0.00, +0.00]; the interval must lie above 0; efficiency: tool calls +0.3 [+0.0, +0.6]; the interval must lie below 0, cost +45.7%; at most +10%
- recall +0.00 [+0.00, +0.00] over 48 pairs
- tool calls +0.3 [+0.0, +0.6]
- cost +5864 (+45.7%) [+38.3%, +53.1%]
- tokens +18976 (+69.3%) [+57.9%, +80.6%]
