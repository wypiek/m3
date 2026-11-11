//! Finding links under the pointer, and regex helpers.

use vte4::prelude::*;

/// URL patterns highlighted in the terminal (PCRE2).
pub const URL_REGEX: &str = r#"(?:https?|ftp|file)://[^\s<>"'`]*[^\s<>"'`.,;:!?)\]}]"#;

// PCRE2 compile flags used by VTE regexes.
pub const PCRE2_CASELESS: u32 = 0x0000_0008;
pub const PCRE2_MULTILINE: u32 = 0x0000_0400;
pub const PCRE2_UCP: u32 = 0x0002_0000;
pub const PCRE2_UTF: u32 = 0x0008_0000;
pub const PCRE2_NO_UTF_CHECK: u32 = 0x4000_0000;

/// The link at widget coordinates: an OSC 8 hyperlink, else a matched URL.
pub fn link_at(term: &vte4::Terminal, x: f64, y: f64) -> Option<String> {
    term.check_hyperlink_at(x, y).or_else(|| term.check_match_at(x, y).0).map(|s| s.to_string())
}

/// Escape `text` for use as a literal in a PCRE2 pattern.
pub fn escape_regex(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    for c in text.chars() {
        if "\\^$.|?*+()[]{}/-".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Only open links with schemes that are safe to hand to the desktop.
pub fn is_openable(uri: &str) -> bool {
    let lower = uri.to_ascii_lowercase();
    ["http://", "https://", "ftp://", "file://", "mailto:"].iter().any(|s| lower.starts_with(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escaping() {
        assert_eq!(escape_regex("a.b*c"), r"a\.b\*c");
        assert_eq!(escape_regex("(x)[y]{z}"), r"\(x\)\[y\]\{z\}");
        assert_eq!(escape_regex("plain"), "plain");
    }

    #[test]
    fn openable_schemes() {
        assert!(is_openable("https://example.org/x"));
        assert!(is_openable("FILE:///tmp"));
        assert!(!is_openable("javascript:alert(1)"));
        assert!(!is_openable("ssh://host"));
    }
}
