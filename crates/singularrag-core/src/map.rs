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
        out.push_str(path);
        out.push_str(":\n");
        let mut rows: Vec<&ScoredSymbol> = top.iter().filter(|s| s.path == path).collect();
        rows.sort_by_key(|s| s.line_start);
        for s in rows {
            out.push_str(&format!("{:>5}  {}\n", s.line_start, s.signature));
        }
    }
    out
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

pub fn header(
    index_version: &str,
    git_head: Option<&str>,
    stale: usize,
    retrieval_id: i64,
) -> String {
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
    format!("# singularrag · index {idx} · HEAD {head} · {fresh} · retrieval r_{retrieval_id:06}")
}

pub fn footer(served: usize, total: usize) -> String {
    if served >= total {
        format!("# {served} of {total} symbols shown")
    } else {
        format!(
            "# {served} of {total} symbols shown · {} more ranked below budget · widen with a larger budget or a focus file",
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
            footer(42, 310),
            "# 42 of 310 symbols shown · 268 more ranked below budget · widen with a larger budget or a focus file"
        );
        assert_eq!(footer(5, 5), "# 5 of 5 symbols shown");
    }

    #[test]
    fn budget_is_clamped() {
        assert_eq!(clamp_budget(0), MIN_BUDGET);
        assert_eq!(clamp_budget(1024), 1024);
        assert_eq!(clamp_budget(1_000_000), MAX_BUDGET);
    }
}
