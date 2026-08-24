//! Redirector pages: redirects expressed in HTML rather than in a `3xx`.
//!
//! A `Location` header is not the only way the web forwards a request. Referrer
//! strippers and link filters — `href.li`, `anonym.to`, `nullrefer.com`,
//! `steamcommunity.com/linkfilter`, and every "you are leaving our site" page —
//! answer `200 OK` with a short HTML document whose only content is a
//! `<meta http-equiv="refresh">` and a `window.location` assignment. A browser
//! follows it; a downloader that only understands `3xx` does not, and saves the
//! forwarding page instead:
//!
//! ```text
//! https://href.li/?https://example.org/setup.exe   ->  index.html   (1 KB of HTML)
//! ```
//!
//! That is exactly the failure this module removes. The resolution is
//! EVIDENCE-BASED rather than heuristic: nothing is unwrapped from the query
//! string and no host list is consulted, because `?url=…` is also how signed
//! CDN links and legitimate fetch proxies are spelled, and rewriting those
//! would break downloads that work today. A hop is taken only when the server
//! actually served a page that says "go here instead".
//!
//! Two directives are recognised, which between them cover what redirectors
//! emit — the meta refresh for browsers with scripting off, and the script for
//! everyone else:
//!
//! * `<meta http-equiv="refresh" content="0; url=TARGET">`
//! * `location = "TARGET"`, `location.href = "TARGET"`,
//!   `location.replace("TARGET")`, `location.assign("TARGET")`
//!
//! The caller owns the hop budget and the URL joining: this module reports the
//! target verbatim (it may be relative), and the same redirect budget that
//! bounds a `3xx` chain must bound these hops too — a pair of pages pointing at
//! each other is a loop like any other.

use crate::http::fetch_small;
use crate::{Connector, Target};

/// Largest page examined for a redirect directive.
///
/// A redirector page is a few hundred bytes; this is three orders of magnitude
/// of headroom and still small enough that fetching one to find out costs less
/// than a TLS handshake. The cap matters because the alternative — reading an
/// arbitrary HTML response whole — would pull a multi-megabyte page into memory
/// to answer a question about its first kilobyte.
pub const MAX_REDIRECTOR_PAGE: u64 = 64 * 1024;

/// Fetch `t` and report the redirect it forwards to, if it is a redirector page.
///
/// Returns the target verbatim, exactly as the page spells it: absolute,
/// root-relative, protocol-relative, or path-relative. Resolving it against the
/// current URL is the caller's job, since only the caller knows what that is.
///
/// Failure is silent and is reported as "no redirect" rather than as an error:
/// this runs speculatively against a response that is probably just a web page,
/// and a page that cannot be fetched is not a redirect the caller can follow.
pub async fn html_redirect<C: Connector>(c: &C, t: &Target) -> Option<String> {
    let body = fetch_small(c, t, MAX_REDIRECTOR_PAGE as usize).await.ok()?;
    html_redirect_target(&String::from_utf8_lossy(&body))
}

/// The redirect target declared by an HTML document, if it declares one.
///
/// Pure: the parsing half of [`html_redirect`], separated so the recognition
/// rules can be tested against real redirector markup without a socket.
pub fn html_redirect_target(body: &str) -> Option<String> {
    // ASCII-lowercased for case-insensitive matching. `to_ascii_lowercase`
    // maps only `A-Z`, so it is byte-length preserving: an index found in the
    // lowercase copy addresses the same position in the original, and every
    // index used below comes from matching ASCII text, so it lands on a char
    // boundary even when the page contains UTF-8 elsewhere.
    let lower = body.to_ascii_lowercase();
    meta_refresh(body, &lower)
        .or_else(|| script_location(body, &lower))
        .filter(|u| followable(u))
}

/// A target worth handing back to the caller.
///
/// Restricted to what a downloader can actually fetch. `javascript:`, `data:`
/// and friends appear in the same markup and would otherwise be returned as if
/// they were addresses.
fn followable(u: &str) -> bool {
    let u = u.trim();
    if u.is_empty() {
        return false;
    }
    let lower = u.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return true;
    }
    // Relative forms: `/path`, `//host/path`, `path`. Anything carrying a
    // scheme this client cannot fetch is refused — a colon before the first
    // slash is what distinguishes `mailto:x` from `dir/file?a:b`.
    let head = u.split('/').next().unwrap_or("");
    !head.contains(':')
}

// ------------------------------------------------------------- meta refresh

/// `<meta http-equiv="refresh" content="0; url=TARGET">`.
fn meta_refresh(body: &str, lower: &str) -> Option<String> {
    let mut i = 0usize;
    while let Some(rel) = lower[i..].find("<meta") {
        let start = i + rel;
        let end = lower[start..]
            .find('>')
            .map(|e| start + e)
            .unwrap_or(lower.len());
        // Always past the `<meta` just examined, so a tag with no `>` (a
        // truncated page — this body is a capped prefix) cannot spin here.
        i = end.max(start + 5);

        let tag = &body[start..end];
        let tag_lower = &lower[start..end];
        // The refresh spelling, not just any `http-equiv`: `content-type` and
        // `x-ua-compatible` sit in the same head and carry a `content` too.
        if attr(tag_lower, tag_lower, "http-equiv")
            .as_deref()
            .map(str::trim)
            != Some("refresh")
        {
            continue;
        }
        let Some(content) = attr(tag, tag_lower, "content") else {
            continue;
        };
        if let Some(u) = refresh_url(&content) {
            return Some(u);
        }
    }
    None
}

/// The URL out of a refresh directive: `0; url=X`, `0;URL='X'`, `0,url=X`.
fn refresh_url(content: &str) -> Option<String> {
    // The delay and the URL are separated by `;` (or `,`, which browsers also
    // accept). A directive with no separator is a bare delay — a genuine
    // self-refresh, not a redirect.
    let (_, rest) = content.split_once([';', ','])?;
    let rest = rest.trim();
    let rest_lower = rest.to_ascii_lowercase();
    let value = match rest_lower.find("url") {
        Some(k) => rest[k + 3..].trim_start().strip_prefix('=')?.trim(),
        // `content="0; https://…"` — no `url=` key, just the address.
        None => rest,
    };
    Some(unentity(unquote(value)))
}

/// The value of attribute `name` in a tag, quoted or bare.
///
/// `tag` and `tag_lower` are the same bytes, the second ASCII-lowercased: the
/// name is matched in the lowercase copy and the value is taken from the
/// original, because a URL's case is significant and its attribute's is not.
fn attr(tag: &str, tag_lower: &str, name: &str) -> Option<String> {
    let b = tag_lower.as_bytes();
    let mut i = 0usize;
    while let Some(rel) = tag_lower[i..].find(name) {
        let at = i + rel;
        i = at + name.len();
        // A name match must be the whole attribute name, not a suffix of a
        // longer one: `data-content` must not answer for `content`.
        let boundary_before = at == 0 || !is_name_char(b[at - 1]);
        let mut k = skip_ws(b, i);
        if !boundary_before || b.get(k) != Some(&b'=') {
            continue;
        }
        k = skip_ws(b, k + 1);
        return Some(match b.get(k) {
            Some(&q) if q == b'"' || q == b'\'' => {
                let end = tag_lower[k + 1..].find(q as char)? + k + 1;
                tag[k + 1..end].to_string()
            }
            // Bare value: runs to whitespace or the tag's end.
            Some(_) => {
                let end = tag_lower[k..]
                    .find(|c: char| c.is_ascii_whitespace())
                    .map(|e| k + e)
                    .unwrap_or(tag.len());
                tag[k..end].to_string()
            }
            None => return None,
        });
    }
    None
}

// ---------------------------------------------------------------- scripting

/// `location = "X"`, `location.href = "X"`, `location.replace("X")`,
/// `location.assign("X")` — with any `window.`/`document.`/`top.` prefix, which
/// this does not need to see: matching on `location` alone covers every spelling.
fn script_location(body: &str, lower: &str) -> Option<String> {
    let b = lower.as_bytes();
    let raw = body.as_bytes();
    let mut i = 0usize;
    while let Some(rel) = lower[i..].find("location") {
        let after = i + rel + "location".len();
        i = after;
        let mut k = skip_ws(b, after);
        if b.get(k) == Some(&b'.') {
            k = skip_ws(b, k + 1);
            let (ident, next) = ident_at(b, k);
            k = skip_ws(b, next);
            match ident {
                // `= "X"`, but not `== "X"`: a comparison is not a jump.
                "href" if b.get(k) == Some(&b'=') && b.get(k + 1) != Some(&b'=') => k += 1,
                "replace" | "assign" if b.get(k) == Some(&b'(') => k += 1,
                _ => continue,
            }
        } else if b.get(k) == Some(&b'=') && b.get(k + 1) != Some(&b'=') {
            k += 1;
        } else {
            continue;
        }
        k = skip_ws(b, k);
        let Some(&quote) = b.get(k) else { continue };
        if quote != b'"' && quote != b'\'' {
            continue;
        }
        if let Some(s) = js_string(raw, k + 1, quote) {
            return Some(unescape_js(&s));
        }
    }
    None
}

/// The contents of a JavaScript string literal starting at `from`, up to the
/// first unescaped `quote`. Returned still escaped.
fn js_string(b: &[u8], from: usize, quote: u8) -> Option<String> {
    let mut out = Vec::new();
    let mut i = from;
    while i < b.len() {
        match b[i] {
            b'\\' if i + 1 < b.len() => {
                out.push(b[i]);
                out.push(b[i + 1]);
                i += 2;
            }
            c if c == quote => return String::from_utf8(out).ok(),
            // A literal newline inside a string is not a string: this is
            // markup that happens to contain the word `location`.
            b'\n' | b'\r' => return None,
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    None
}

/// Undo the escaping a redirector applies to a URL inside a script literal.
///
/// `\/` matters most: the WordPress-family strippers emit
/// `window.location.replace( "https:\/\/example.org\/setup.exe" )`, and a target
/// carrying literal backslashes parses as neither a host nor a path.
fn unescape_js(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut c = s.chars();
    while let Some(ch) = c.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match c.next() {
            Some('u') => {
                // `\uXXXX`. Anything malformed is dropped rather than guessed:
                // a mangled escape in a URL is not something to repair.
                let hex: String = c.by_ref().take(4).collect();
                if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                    out.push(ch);
                }
            }
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some(other) => out.push(other),
            None => break,
        }
    }
    out
}

// ------------------------------------------------------------------- bits

fn is_name_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'-' || c == b'_' || c == b':'
}

fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && b[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

/// The identifier at `from`, and the index just past it.
fn ident_at(b: &[u8], from: usize) -> (&str, usize) {
    let mut end = from;
    while end < b.len() && (b[end].is_ascii_alphanumeric() || b[end] == b'_') {
        end += 1;
    }
    (
        std::str::from_utf8(&b[from..end]).unwrap_or(""),
        end.max(from),
    )
}

fn unquote(s: &str) -> &str {
    let s = s.trim();
    for q in ['"', '\''] {
        if let Some(inner) = s.strip_prefix(q) {
            return inner.split(q).next().unwrap_or("").trim();
        }
    }
    s
}

/// Decode the entities an attribute value can carry. A query string is where
/// this shows up: `content="0; url=/go?a=1&amp;b=2"` addresses `&`, not `&amp;`.
fn unentity(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&#38;", "&")
        .replace("&#x26;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The page that started this: a referrer stripper, verbatim.
    #[test]
    fn href_li_style_page_resolves_to_the_real_download() {
        let body = r#"<!DOCTYPE html>
<html><head>
<title>href.li</title>
<meta http-equiv="Refresh" content="0; url=https://www.xmedia-recode.de/download/XMediaRecode3640_x64_setup.exe" />
<meta name="referrer" content="no-referrer" />
<script type="text/javascript">
window.location.replace( "https:\/\/www.xmedia-recode.de\/download\/XMediaRecode3640_x64_setup.exe" + window.location.hash );
</script>
</head><body><p>Redirecting..</p></body></html>"#;
        assert_eq!(
            html_redirect_target(body).as_deref(),
            Some("https://www.xmedia-recode.de/download/XMediaRecode3640_x64_setup.exe")
        );
    }

    /// With the meta tag absent, the script is the only statement of intent.
    #[test]
    fn script_only_redirector_is_followed() {
        for js in [
            r#"<script>window.location.replace("https://a.org/f.zip")</script>"#,
            r#"<script>location.href = 'https://a.org/f.zip';</script>"#,
            r#"<script>window.location = "https://a.org/f.zip";</script>"#,
            r#"<script>document.location.assign( "https:\/\/a.org\/f.zip" )</script>"#,
        ] {
            assert_eq!(
                html_redirect_target(js).as_deref(),
                Some("https://a.org/f.zip"),
                "{js}"
            );
        }
    }

    #[test]
    fn meta_refresh_spellings_all_parse() {
        for tag in [
            r#"<meta http-equiv="refresh" content="0; url=/dl/f.zip">"#,
            r#"<META HTTP-EQUIV="REFRESH" CONTENT="5;URL='/dl/f.zip'">"#,
            r#"<meta http-equiv=refresh content="0,url=/dl/f.zip">"#,
            r#"<meta http-equiv="refresh" content="0; /dl/f.zip">"#,
        ] {
            assert_eq!(
                html_redirect_target(tag).as_deref(),
                Some("/dl/f.zip"),
                "{tag}"
            );
        }
    }

    #[test]
    fn query_entities_are_decoded() {
        let tag = r#"<meta http-equiv="refresh" content="0; url=https://a.org/get?id=1&amp;k=2">"#;
        assert_eq!(
            html_redirect_target(tag).as_deref(),
            Some("https://a.org/get?id=1&k=2")
        );
    }

    /// An ordinary page must be downloaded, not chased.
    #[test]
    fn a_page_without_a_redirect_directive_is_not_one() {
        for body in [
            "<html><body><a href=\"https://a.org/f.zip\">download</a></body></html>",
            r#"<meta http-equiv="content-type" content="text/html; charset=utf-8">"#,
            r#"<meta name="refresh" content="0; url=https://a.org/f.zip">"#,
            r#"<script>if (location.href == "https://a.org/x") { go(); }</script>"#,
            r#"<meta http-equiv="refresh" content="30">"#,
            "<html><body>plain</body></html>",
        ] {
            assert_eq!(html_redirect_target(body), None, "{body}");
        }
    }

    /// A `data:`/`javascript:` target is markup, not an address to fetch.
    #[test]
    fn unfetchable_schemes_are_refused() {
        for body in [
            r#"<script>location.href = "javascript:void(0)"</script>"#,
            r#"<meta http-equiv="refresh" content="0; url=data:text/html,hi">"#,
            r#"<script>location.replace("mailto:x@y.org")</script>"#,
        ] {
            assert_eq!(html_redirect_target(body), None, "{body}");
        }
    }

    /// A truncated page (the body is a capped prefix) must not hang the scan.
    #[test]
    fn a_truncated_page_terminates() {
        assert_eq!(html_redirect_target("<meta http-equiv=\"refre"), None);
        assert_eq!(
            html_redirect_target("<script>location.replace(\"http"),
            None
        );
        assert_eq!(html_redirect_target("<meta<meta<meta"), None);
    }

    /// Non-ASCII text elsewhere on the page must not disturb the byte indices
    /// the scan works with.
    #[test]
    fn utf8_body_does_not_break_indexing() {
        let body = "<html><body><p>Перенаправление… 转向中</p>\
                    <meta http-equiv=\"refresh\" content=\"0; url=https://a.org/файл.zip\"></body></html>";
        assert_eq!(
            html_redirect_target(body).as_deref(),
            Some("https://a.org/файл.zip")
        );
    }
}
