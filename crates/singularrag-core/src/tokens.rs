//! Identifier splitting (for FTS and query matching) and the token approximation.

const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "the", "of", "to", "in", "is", "it", "for", "on", "how", "does", "do",
    "what", "where", "which", "when", "why", "this", "that", "with", "by", "at", "be", "or", "as",
    "from", "into", "i", "we", "you", "my", "our",
];

/// "createSession" -> ["create","session"]; "XMLHttpRequest" -> ["xml","http","request"].
pub fn split_identifier(s: &str) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let mut parts = Vec::new();
    let mut cur = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if !c.is_alphanumeric() {
            if !cur.is_empty() {
                parts.push(std::mem::take(&mut cur));
            }
            continue;
        }
        if c.is_uppercase() && !cur.is_empty() {
            let prev = chars[i - 1];
            let next_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
            // Boundary at lower->Upper, or at the last capital of an acronym run (XMLHttp -> XML|Http).
            if prev.is_lowercase() || prev.is_numeric() || (prev.is_uppercase() && next_lower) {
                parts.push(std::mem::take(&mut cur));
            }
        }
        cur.push(c);
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    parts.into_iter().map(|p| p.to_lowercase()).collect()
}

/// Spec §7: tokens ≈ chars / 4, rounded up. Budget is soft.
pub fn approx_tokens(s: &str) -> usize {
    s.chars().count().div_ceil(4)
}

/// Extracts query terms: splits on identifiers, lowercases, filters stopwords and short terms (< 2 chars), deduplicates.
pub fn query_terms(q: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in q.split(|c: char| !c.is_alphanumeric() && c != '_') {
        for part in split_identifier(word) {
            if part.chars().count() >= 2
                && !STOPWORDS.contains(&part.as_str())
                && !out.contains(&part)
            {
                out.push(part);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_camel_snake_and_acronyms() {
        assert_eq!(split_identifier("createSession"), vec!["create", "session"]);
        assert_eq!(split_identifier("HTTP_Server2"), vec!["http", "server2"]);
        assert_eq!(
            split_identifier("XMLHttpRequest"),
            vec!["xml", "http", "request"]
        );
        assert_eq!(split_identifier("session"), vec!["session"]);
        assert_eq!(split_identifier("__init__"), vec!["init"]);
    }

    #[test]
    fn approx_tokens_is_chars_over_four_rounded_up() {
        assert_eq!(approx_tokens(""), 0);
        assert_eq!(approx_tokens("abcd"), 1);
        assert_eq!(approx_tokens("abcde"), 2);
    }

    #[test]
    fn query_terms_lowercases_splits_and_drops_stopwords() {
        assert_eq!(
            query_terms("Where is createSession used in the HTTP layer?"),
            vec!["create", "session", "used", "http", "layer"]
        );
        assert_eq!(
            query_terms("SessionStore SessionStore"),
            vec!["session", "store"]
        );
        // Lone non-ASCII character (1 char, 2 bytes) should be filtered; longer terms kept
        assert_eq!(query_terms("ä session"), vec!["session"]);
    }
}
