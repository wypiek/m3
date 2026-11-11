//! Minimal glob matching (`*` and `?`) used by window rules.

/// Match `text` against `pattern`, where `*` matches any run of characters and
/// `?` matches exactly one. Matching is case-sensitive.
pub fn matches(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut mark = 0usize;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::matches;

    #[test]
    fn globbing() {
        assert!(matches("firefox", "firefox"));
        assert!(!matches("firefox", "firefox-esr"));
        assert!(matches("firefox*", "firefox-esr"));
        assert!(matches("*.Nautilus", "org.gnome.Nautilus"));
        assert!(matches("org.?nome.*", "org.gnome.Nautilus"));
        assert!(matches("*", ""));
        assert!(!matches("?", ""));
        assert!(matches("a*b*c", "aXXbYYc"));
        assert!(!matches("a*b*c", "aXXbYY"));
    }
}
