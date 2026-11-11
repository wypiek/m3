//! What the address field does with typed text (pure and unit-tested).

/// Search engines the browser offers, `(id, label, query-URL prefix)`. The
/// query is appended percent-encoded.
pub const ENGINES: &[(&str, &str, &str)] = &[
    ("duckduckgo", "DuckDuckGo", "https://duckduckgo.com/?q="),
    ("startpage", "Startpage", "https://www.startpage.com/do/search?q="),
    ("brave", "Brave Search", "https://search.brave.com/search?q="),
    ("ecosia", "Ecosia", "https://www.ecosia.org/search?q="),
    ("google", "Google", "https://www.google.com/search?q="),
    ("bing", "Bing", "https://www.bing.com/search?q="),
];

pub const DEFAULT_ENGINE: &str = "duckduckgo";

/// The query-URL prefix for engine `id` (falls back to the default).
pub fn engine_prefix(id: &str) -> &'static str {
    ENGINES
        .iter()
        .find(|(e, _, _)| *e == id)
        .or_else(|| ENGINES.iter().find(|(e, _, _)| *e == DEFAULT_ENGINE))
        .map(|(_, _, p)| *p)
        .unwrap_or("https://duckduckgo.com/?q=")
}

/// Percent-encode a search query (RFC 3986 unreserved characters kept,
/// spaces as `+`).
pub fn encode_query(q: &str) -> String {
    let mut out = String::with_capacity(q.len() * 3);
    for b in q.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Schemes the address field navigates to directly.
const SCHEMES: &[&str] = &["http", "https", "file", "about"];

fn is_ipv4(host: &str) -> bool {
    let parts: Vec<&str> = host.split('.').collect();
    parts.len() == 4 && parts.iter().all(|p| !p.is_empty() && p.len() <= 3 && p.parse::<u8>().is_ok())
}

/// Whether `s` (no scheme, no spaces) looks like a host with optional port
/// and path: `example.com`, `localhost:8080/x`, `192.168.1.1`, `[::1]:80`.
fn looks_like_host(s: &str) -> bool {
    let end = s.find(['/', '?', '#']).unwrap_or(s.len());
    let authority = &s[..end];
    if authority.is_empty() {
        return false;
    }
    if authority.starts_with('[') {
        return authority.contains(']');
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (authority, None),
    };
    if let Some(p) = port
        && (p.is_empty() || p.parse::<u16>().is_err())
    {
        return false;
    }
    if host.eq_ignore_ascii_case("localhost") || is_ipv4(host) {
        return true;
    }
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() < 2 {
        return false;
    }
    let valid_label = |l: &str| {
        !l.is_empty()
            && l.len() <= 63
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.chars().all(|c| c.is_alphanumeric() || c == '-')
    };
    let tld = labels.last().copied().unwrap_or("");
    labels.iter().all(|l| valid_label(l)) && tld.chars().all(char::is_alphabetic) && tld.chars().count() >= 2
}

/// Turn address-field input into a URL to load, or `None` if nothing should
/// happen (empty input, or a `javascript:` URL — never run from the field).
pub fn resolve(input: &str, engine: &str) -> Option<String> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }
    let lower = s.to_ascii_lowercase();
    if lower.starts_with("javascript:") || lower.starts_with("data:") {
        return None;
    }
    if let Some((scheme, rest)) = s.split_once(':') {
        let scheme_l = scheme.to_ascii_lowercase();
        if SCHEMES.contains(&scheme_l.as_str()) && !s.contains(char::is_whitespace) {
            // `about:` pages and `scheme://…`; `http:foo` is treated as a search.
            if scheme_l == "about" || rest.starts_with("//") {
                return Some(s.to_owned());
            }
        }
    }
    if s.starts_with('/') && !s.contains(char::is_whitespace) {
        return Some(format!("file://{s}"));
    }
    if !s.contains(char::is_whitespace) && looks_like_host(s) {
        return Some(format!("https://{s}"));
    }
    Some(format!("{}{}", engine_prefix(engine), encode_query(s)))
}

/// The host of a URL, for display and dialogs (`https://a.b/c` → `a.b`).
pub fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    let authority = authority.rsplit_once('@').map(|(_, h)| h).unwrap_or(authority);
    (!authority.is_empty()).then(|| authority.to_owned())
}

/// Text for the address field: the URL without a trailing slash on bare
/// hosts; empty for internal pages.
pub fn display_url(url: &str) -> String {
    if url.is_empty() || url == "about:blank" {
        return String::new();
    }
    match url.split_once("://") {
        Some((_, rest)) if rest.matches('/').count() == 1 && rest.ends_with('/') => {
            url.trim_end_matches('/').to_owned()
        }
        _ => url.to_owned(),
    }
}

/// Escape text for inclusion in HTML.
pub fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(s: &str) -> Option<String> {
        resolve(s, "duckduckgo")
    }

    #[test]
    fn explicit_schemes_are_kept() {
        assert_eq!(r("https://example.com/a?b=c").as_deref(), Some("https://example.com/a?b=c"));
        assert_eq!(r("http://localhost:3000").as_deref(), Some("http://localhost:3000"));
        assert_eq!(r("file:///etc/hostname").as_deref(), Some("file:///etc/hostname"));
        assert_eq!(r("about:blank").as_deref(), Some("about:blank"));
        assert_eq!(r("  HTTPS://Example.com  ").as_deref(), Some("HTTPS://Example.com"));
    }

    #[test]
    fn hosts_get_https() {
        assert_eq!(r("example.com").as_deref(), Some("https://example.com"));
        assert_eq!(r("sub.example.co.uk/path?q=1").as_deref(), Some("https://sub.example.co.uk/path?q=1"));
        assert_eq!(r("localhost:8080").as_deref(), Some("https://localhost:8080"));
        assert_eq!(r("localhost").as_deref(), Some("https://localhost"));
        assert_eq!(r("192.168.1.1").as_deref(), Some("https://192.168.1.1"));
        assert_eq!(r("10.0.0.2:8443/x").as_deref(), Some("https://10.0.0.2:8443/x"));
        assert_eq!(r("[::1]:8080").as_deref(), Some("https://[::1]:8080"));
        assert_eq!(r("/home/me/page.html").as_deref(), Some("file:///home/me/page.html"));
    }

    #[test]
    fn everything_else_is_a_search() {
        assert_eq!(r("rust gtk4").as_deref(), Some("https://duckduckgo.com/?q=rust+gtk4"));
        assert_eq!(r("hello").as_deref(), Some("https://duckduckgo.com/?q=hello"));
        assert_eq!(r("c++ & rust?").as_deref(), Some("https://duckduckgo.com/?q=c%2B%2B+%26+rust%3F"));
        assert_eq!(r("example.com is down").as_deref(), Some("https://duckduckgo.com/?q=example.com+is+down"));
        assert_eq!(r("1.5").as_deref(), Some("https://duckduckgo.com/?q=1.5"));
        assert_eq!(r("999.1.1.1").as_deref(), Some("https://duckduckgo.com/?q=999.1.1.1"));
        assert_eq!(r("localhost:notaport").as_deref(), Some("https://duckduckgo.com/?q=localhost%3Anotaport"));
        assert_eq!(resolve("hi", "google").as_deref(), Some("https://www.google.com/search?q=hi"));
        assert_eq!(resolve("hi", "nonsense").as_deref(), Some("https://duckduckgo.com/?q=hi"));
    }

    #[test]
    fn javascript_and_empty_do_nothing() {
        assert_eq!(r(""), None);
        assert_eq!(r("   "), None);
        assert_eq!(r("javascript:alert(1)"), None);
        assert_eq!(r("JavaScript:alert(1)"), None);
        assert_eq!(r("data:text/html,<script>x</script>"), None);
    }

    #[test]
    fn hosts_and_display() {
        assert_eq!(host_of("https://user@example.com:8080/x").as_deref(), Some("example.com:8080"));
        assert_eq!(host_of("about:blank"), None);
        assert_eq!(display_url("https://example.com/"), "https://example.com");
        assert_eq!(display_url("https://example.com/a/"), "https://example.com/a/");
        assert_eq!(display_url("about:blank"), "");
    }

    #[test]
    fn escaping() {
        assert_eq!(html_escape("<a href=\"x\">'&'</a>"), "&lt;a href=&quot;x&quot;&gt;&#39;&amp;&#39;&lt;/a&gt;");
    }
}
