//! Secret-like content detection. A hit skips the whole file (spec §11).
//! Deliberately a short hand-maintained list; no third-party scanner is standard in Rust.

use std::sync::OnceLock;

use regex::Regex;

struct Rule {
    name: &'static str,
    re: Regex,
}

fn rules() -> &'static [Rule] {
    static RULES: OnceLock<Vec<Rule>> = OnceLock::new();
    RULES.get_or_init(|| {
        let mk = |name, pat| Rule {
            name,
            re: Regex::new(pat).expect("valid secret regex"),
        };
        vec![
            mk(
                "private-key",
                r"-----BEGIN (?:RSA |EC |DSA |OPENSSH |PGP )?PRIVATE KEY(?: BLOCK)?-----",
            ),
            mk("aws-access-key", r"\bAKIA[0-9A-Z]{16}\b"),
            mk("github-token", r"\bgh[pousr]_[A-Za-z0-9]{36,}\b"),
            mk("slack-token", r"\bxox[baprs]-[0-9A-Za-z-]{10,}\b"),
            mk(
                "jwt",
                r"\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\b",
            ),
        ]
    })
}

fn assignment_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)\b(?:api[_-]?key|secret|token|password|passwd|auth)\w*\s*[:=]\s*['"]([A-Za-z0-9_\-/+=]{32,})['"]"#)
            .expect("valid assignment regex")
    })
}

/// Shannon entropy in bits per character.
fn entropy(s: &str) -> f64 {
    let mut counts = [0usize; 256];
    for b in s.bytes() {
        counts[b as usize] += 1;
    }
    let n = s.len() as f64;
    counts
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = c as f64 / n;
            -p * p.log2()
        })
        .sum()
}

pub fn looks_secret(text: &str) -> Option<&'static str> {
    for r in rules() {
        if r.re.is_match(text) {
            return Some(r.name);
        }
    }
    for cap in assignment_re().captures_iter(text) {
        if entropy(&cap[1]) > 3.5 {
            return Some("secret-assignment");
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_private_key_header() {
        assert_eq!(
            looks_secret("-----BEGIN RSA PRIVATE KEY-----\nMIIE"),
            Some("private-key")
        );
        assert_eq!(
            looks_secret("-----BEGIN OPENSSH PRIVATE KEY-----"),
            Some("private-key")
        );
    }

    #[test]
    fn detects_aws_github_slack_jwt() {
        assert_eq!(
            looks_secret("key = AKIAIOSFODNN7EXAMPLE"),
            Some("aws-access-key")
        );
        assert_eq!(
            looks_secret("ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghij"),
            Some("github-token")
        );
        assert_eq!(
            looks_secret("xoxb-123456789012-abcdefghijk"),
            Some("slack-token")
        );
        assert_eq!(
            looks_secret("eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c"),
            Some("jwt")
        );
    }

    #[test]
    fn detects_high_entropy_assignment() {
        let s = r#"const apiKey = "9aB3xQ7mZ2pL8kR4tY6wE1uI0oP5sD7fXk2";"#;
        assert_eq!(looks_secret(s), Some("secret-assignment"));
    }

    #[test]
    fn ignores_ordinary_code_and_low_entropy() {
        assert_eq!(
            looks_secret("export function createSession(user: User) {}"),
            None
        );
        assert_eq!(
            looks_secret(r#"const password = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";"#),
            None
        );
        assert_eq!(looks_secret(r#"const token = getToken();"#), None);
        assert_eq!(
            looks_secret(r#""integrity": "sha512-abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJ""#),
            None
        );
    }

    #[test]
    fn own_sources_that_carry_secret_fixtures_are_not_secret_like() {
        // These files hold detector fixtures for their tests; the fixture strings are
        // assembled at runtime so the source files themselves index normally.
        assert_eq!(looks_secret(include_str!("engine.rs")), None, "engine.rs");
        assert_eq!(looks_secret(include_str!("fixture.rs")), None, "fixture.rs");
    }
}
