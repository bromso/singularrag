//! Token-budgeted rendering of ranked symbols: `path:` groups, `line  signature` rows.

use crate::rank::ScoredSymbol;
use crate::tokens::approx_tokens;

pub const DEFAULT_BUDGET: usize = 1024;
pub const MAX_BUDGET: usize = 8192;
pub const MIN_BUDGET: usize = 64;
pub const CUT_RECORDED: usize = 25;

pub fn clamp_budget(b: usize) -> usize {
    b.clamp(MIN_BUDGET, MAX_BUDGET)
}

/// Render the top `n` items grouped by file. Files appear in order of their best
/// item; symbols within a file are sorted by line.
pub fn render(items: &[ScoredSymbol], n: usize) -> String {
    let top = &items[..n.min(items.len())];
    let mut order: Vec<&str> = Vec::new();
    for s in top {
        if !order.contains(&s.path.as_str()) {
            order.push(&s.path);
        }
    }
    let mut out = String::new();
    for path in order {
        let mut rows: Vec<&ScoredSymbol> = top.iter().filter(|s| s.path == path).collect();
        rows.sort_by_key(|s| s.line_start);
        out.push_str(path);
        out.push(':');
        out.push_str(&referenced_by(path, &rows));
        out.push('\n');
        for s in rows {
            out.push_str(&format!("{:>5}  {}\n", s.line_start, s.signature));
        }
    }
    out
}

/// How many referencing files a group header names before `+N`.
const REFS_SHOWN: usize = 2;

/// `  ← a.ts, b.ts +2` on a file's header: the other files that reference any served
/// symbol of this file, strongest first, so a blast or trace question can be answered
/// from the map and `find_symbol` gives a symbol's own list. The suffix sits on the
/// header, not the rows: on hono a per-row suffix cost a budget a third of its
/// symbols, a per-file one costs a few. References from the file itself say nothing
/// about blast radius and are left out. Empty when nothing else references the file.
fn referenced_by(path: &str, rows: &[&ScoredSymbol]) -> String {
    let mut by_file: Vec<(&str, i64)> = Vec::new();
    for r in rows.iter().flat_map(|s| s.reasons.referenced_by.iter()) {
        if r.path == path {
            continue;
        }
        match by_file.iter_mut().find(|(p, _)| *p == r.path) {
            Some(entry) => entry.1 += r.count,
            None => by_file.push((&r.path, r.count)),
        }
    }
    if by_file.is_empty() {
        return String::new();
    }
    by_file.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let shown: Vec<&str> = by_file.iter().take(REFS_SHOWN).map(|(p, _)| *p).collect();
    let more = by_file.len().saturating_sub(REFS_SHOWN);
    if more > 0 {
        format!("  ← {} +{more}", shown.join(", "))
    } else {
        format!("  ← {}", shown.join(", "))
    }
}

/// Largest `n` such that `render(items, n)` fits the budget. Binary search, as in Aider.
pub fn fit(items: &[ScoredSymbol], budget: usize) -> usize {
    let (mut lo, mut hi) = (0usize, items.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if approx_tokens(&render(items, mid)) <= budget {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    lo
}

/// `# singularrag · index 7f3a2c · HEAD 9b1e0d4 · fresh`: the first line of every tool
/// response, with or without a retrieval id.
pub fn freshness_header(index_version: &str, git_head: Option<&str>, stale: usize) -> String {
    let idx: String = index_version.chars().take(6).collect();
    let head = match git_head {
        Some(h) if !h.is_empty() => h.chars().take(7).collect::<String>(),
        _ => "none".to_string(),
    };
    let fresh = if stale == 0 {
        "fresh".to_string()
    } else {
        format!("STALE: {stale} files changed since index")
    };
    format!("# singularrag · index {idx} · HEAD {head} · {fresh}")
}

pub fn header(
    index_version: &str,
    git_head: Option<&str>,
    stale: usize,
    retrieval_id: i64,
) -> String {
    format!(
        "{} · retrieval r_{retrieval_id:06}",
        freshness_header(index_version, git_head, stale)
    )
}

/// `recorded` is how many of the cut candidates were written to `retrieval_items`
/// (at most `CUT_RECORDED`), which is a different number from the remainder: the
/// footer states both so the arithmetic on the line is consistent.
pub fn footer(served: usize, total: usize, recorded: usize) -> String {
    if served >= total {
        format!("# {served} of {total} symbols shown")
    } else {
        format!(
            "# {served} of {total} symbols shown · {} more ranked below budget · {recorded} recorded · widen with a larger budget or a focus file",
            total - served
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rank::{Reasons, ScoredSymbol};

    fn sym(path: &str, line: u32, sig: &str, score: f64) -> ScoredSymbol {
        ScoredSymbol {
            symbol_id: line as i64,
            file_id: 1,
            path: path.into(),
            name: "x".into(),
            kind: "function".into(),
            line_start: line,
            line_end: line,
            signature: sig.into(),
            score,
            reasons: Reasons::default(),
        }
    }

    #[test]
    fn render_groups_by_file_in_rank_order_and_sorts_lines() {
        let items = vec![
            sym("src/b.ts", 20, "export function two()", 0.9),
            sym("src/a.ts", 5, "export function five()", 0.5),
            sym("src/b.ts", 3, "export function three()", 0.4),
        ];
        let text = render(&items, 3);
        assert_eq!(
            text,
            "src/b.ts:\n    3  export function three()\n   20  export function two()\nsrc/a.ts:\n    5  export function five()\n"
        );
    }

    #[test]
    fn render_names_the_files_that_reference_a_symbol() {
        use crate::rank::RefBy;
        let rb = |pairs: &[(&str, i64)]| {
            pairs
                .iter()
                .map(|(p, c)| RefBy {
                    path: p.to_string(),
                    count: *c,
                })
                .collect::<Vec<_>>()
        };
        // src/a.ts: b.ts references it 9 + 1 times across two symbols, d.ts 2, e.ts 1;
        // its own references and a symbol nobody references add nothing.
        let mut five = sym("src/a.ts", 5, "export function five()", 0.9);
        five.reasons.referenced_by = rb(&[("src/d.ts", 2), ("src/b.ts", 1)]);
        let mut nine = sym("src/a.ts", 9, "export class Nine", 0.8);
        nine.reasons.referenced_by = rb(&[("src/b.ts", 9), ("src/a.ts", 4), ("src/e.ts", 1)]);
        let mut only_self = sym("src/a.ts", 12, "export const twelve = 12", 0.7);
        only_self.reasons.referenced_by = rb(&[("src/a.ts", 2)]);
        let none = sym("src/z.ts", 1, "export const z = 1", 0.6);
        let text = render(&[five, nine, only_self, none], 4);
        assert_eq!(
            text,
            "src/a.ts:  ← src/b.ts, src/d.ts +1\n\
             \x20   5  export function five()\n\
             \x20   9  export class Nine\n\
             \x20  12  export const twelve = 12\n\
             src/z.ts:\n\
             \x20   1  export const z = 1\n"
        );
    }

    #[test]
    fn fit_finds_largest_prefix_within_budget() {
        let items: Vec<ScoredSymbol> = (1..=50)
            .map(|i| sym("src/a.ts", i, "export function f()", 1.0 / i as f64))
            .collect();
        let n = fit(&items, 64);
        assert!(n > 0 && n < 50);
        assert!(crate::tokens::approx_tokens(&render(&items, n)) <= 64);
        assert!(crate::tokens::approx_tokens(&render(&items, n + 1)) > 64);
        assert_eq!(fit(&items, 100_000), 50);
        assert_eq!(fit(&items, 1), 0);
        assert_eq!(fit(&[], 1024), 0);
    }

    #[test]
    fn header_and_footer_format() {
        assert_eq!(
            header("7f3a2c9d1e0b", Some("9b1e0d4f5a6b7c8d"), 0, 123),
            "# singularrag · index 7f3a2c · HEAD 9b1e0d4 · fresh · retrieval r_000123"
        );
        assert_eq!(
            header("7f3a2c9d1e0b", None, 4, 7),
            "# singularrag · index 7f3a2c · HEAD none · STALE: 4 files changed since index · retrieval r_000007"
        );
        assert_eq!(
            freshness_header("7f3a2c9d1e0b", Some("9b1e0d4f5a6b7c8d"), 0),
            "# singularrag · index 7f3a2c · HEAD 9b1e0d4 · fresh"
        );
        assert_eq!(
            footer(42, 310, 25),
            "# 42 of 310 symbols shown · 268 more ranked below budget · 25 recorded · widen with a larger budget or a focus file"
        );
        assert_eq!(footer(5, 5, 0), "# 5 of 5 symbols shown");
    }

    #[test]
    fn budget_is_clamped() {
        assert_eq!(clamp_budget(0), MIN_BUDGET);
        assert_eq!(clamp_budget(1024), 1024);
        assert_eq!(clamp_budget(1_000_000), MAX_BUDGET);
    }
}
