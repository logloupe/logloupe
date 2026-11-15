//! The combined log format, shared by default nginx and Apache (and so by
//! cPanel domlogs):
//!
//! ```text
//! ip ident user [10/Oct/2026:13:55:36 +0000] "GET /path?q=1 HTTP/1.1" 200 2326 "referer" "user-agent"
//! ```
//!
//! Hand-written, no regex: this is the hot path.

use super::{LogFormat, ParseError, Parsed, RawRecord, Request, timestamp};

/// Plain combined. Anything after the user agent is returned as `rest`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Combined;

impl LogFormat for Combined {
    fn name(&self) -> &str {
        "combined"
    }

    fn parse<'a>(&self, line: &'a str) -> Result<Parsed<'a>, ParseError> {
        parse_combined(line)
    }
}

pub fn parse_combined(line: &str) -> Result<Parsed<'_>, ParseError> {
    let line = line.trim_end_matches(['\r', '\n']);

    let (ip, rest) = line.split_once(' ').ok_or(ParseError::Ip)?;
    let ip = ip.parse().map_err(|_| ParseError::Ip)?;

    // `ident user ` sits between the address and the opening bracket.
    let open = rest.find('[').ok_or(ParseError::Timestamp)?;
    let user = rest[..open]
        .trim_end()
        .split_once(' ')
        .map(|(_ident, user)| user)
        .and_then(dash_to_none);

    let rest = &rest[open + 1..];
    let close = rest.find(']').ok_or(ParseError::Timestamp)?;
    let ts = timestamp::parse_clf(&rest[..close]).ok_or(ParseError::Timestamp)?;

    let rest = rest[close + 1..]
        .strip_prefix(" \"")
        .ok_or(ParseError::Request)?;
    let (request, rest) = take_quoted(rest).ok_or(ParseError::Request)?;

    let rest = rest.strip_prefix(' ').ok_or(ParseError::Status)?;
    let (status, rest) = rest.split_once(' ').ok_or(ParseError::Status)?;
    if status.len() != 3 {
        return Err(ParseError::Status);
    }
    let status = status.parse().map_err(|_| ParseError::Status)?;

    let (bytes, rest) = rest.split_once(' ').ok_or(ParseError::Referer)?;
    // Apache writes `-` when no body was sent.
    let bytes = match bytes {
        "-" => 0,
        n => n.parse().map_err(|_| ParseError::Bytes)?,
    };

    let rest = rest.strip_prefix('"').ok_or(ParseError::Referer)?;
    let (referer, rest) = take_quoted(rest).ok_or(ParseError::Referer)?;

    let rest = rest.strip_prefix(" \"").ok_or(ParseError::UserAgent)?;
    let (user_agent, rest) = take_quoted(rest).ok_or(ParseError::UserAgent)?;

    Ok(Parsed {
        record: RawRecord {
            ip,
            user,
            ts,
            request: parse_request(request),
            status,
            bytes,
            referer: dash_to_none(referer),
            user_agent: dash_to_none(user_agent),
            request_ms: None,
            vhost: None,
            forwarded_for: None,
        },
        rest: rest.trim(),
    })
}

fn dash_to_none(s: &str) -> Option<&str> {
    match s {
        "" | "-" => None,
        s => Some(s),
    }
}

/// Reads a quoted field whose opening quote has already been consumed.
/// Returns the contents and the text after the closing quote.
///
/// Apache escapes an embedded quote as `\"` and a backslash as `\\`; nginx
/// writes both as `\xXX`. Skipping the byte after any backslash handles both.
pub(crate) fn take_quoted(s: &str) -> Option<(&str, &str)> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            // A quote is ASCII, so slicing either side of it is always valid.
            b'"' => return Some((&s[..i], &s[i + 1..])),
            _ => i += 1,
        }
    }
    None
}

/// Splits `GET /path?q=1 HTTP/1.1`. Anything that does not look like an HTTP
/// request line is kept whole as [`Request::Malformed`].
pub(crate) fn parse_request(s: &str) -> Request<'_> {
    let Some((method, tail)) = s.split_once(' ') else {
        return Request::Malformed(s);
    };
    let method_ok = !method.is_empty()
        && method.len() <= 20
        && method
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b == b'-' || b == b'_');
    if !method_ok {
        return Request::Malformed(s);
    }

    let (target, protocol) = match tail.rsplit_once(' ') {
        Some((target, protocol)) if protocol.starts_with("HTTP/") => (target, Some(protocol)),
        // No protocol token: only accept a bare target (HTTP/0.9 style).
        _ if !tail.contains(' ') => (tail, None),
        _ => return Request::Malformed(s),
    };
    if target.is_empty() {
        return Request::Malformed(s);
    }

    let (path, query) = match target.split_once('?') {
        Some((path, query)) => (path, Some(query).filter(|q| !q.is_empty())),
        None => (target, None),
    };
    Request::Parsed {
        method,
        path,
        query,
        protocol,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;

    fn ok(line: &str) -> Parsed<'_> {
        parse_combined(line).unwrap_or_else(|e| panic!("{e}: {line}"))
    }

    #[test]
    fn nginx_default() {
        let p = ok(
            r#"203.0.113.7 - - [10/Oct/2026:13:55:36 +0000] "GET /path?q=1 HTTP/1.1" 200 2326 "https://example.com/" "Mozilla/5.0 (X11; Linux x86_64)""#,
        );
        let r = p.record;
        assert_eq!(r.ip, "203.0.113.7".parse::<IpAddr>().unwrap());
        assert_eq!(r.user, None);
        assert_eq!(
            r.ts,
            timestamp::parse_clf("10/Oct/2026:13:55:36 +0000").unwrap()
        );
        assert_eq!(
            r.request,
            Request::Parsed {
                method: "GET",
                path: "/path",
                query: Some("q=1"),
                protocol: Some("HTTP/1.1"),
            }
        );
        assert_eq!(r.status, 200);
        assert_eq!(r.bytes, 2326);
        assert_eq!(r.referer, Some("https://example.com/"));
        assert_eq!(r.user_agent, Some("Mozilla/5.0 (X11; Linux x86_64)"));
        assert_eq!(p.rest, "");
    }

    #[test]
    fn apache_dash_bytes_and_user() {
        let p = ok(
            "198.51.100.9 - rob [10/Oct/2026:13:55:36 +0100] \"POST /wp-login.php HTTP/1.0\" 304 - \"-\" \"-\"\n",
        );
        assert_eq!(p.record.user, Some("rob"));
        assert_eq!(p.record.bytes, 0);
        assert_eq!(p.record.referer, None);
        assert_eq!(p.record.user_agent, None);
    }

    #[test]
    fn ipv6() {
        let p = ok(
            r#"2001:db8::1 - - [10/Oct/2026:13:55:36 +0000] "GET / HTTP/2.0" 200 10 "-" "curl/8.5.0""#,
        );
        assert_eq!(p.record.ip, "2001:db8::1".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn trailing_fields_are_returned_as_rest() {
        let p = ok(
            r#"203.0.113.7 - - [10/Oct/2026:13:55:36 +0000] "GET / HTTP/1.1" 200 10 "-" "curl/8.5.0" 0.123 "example.com""#,
        );
        assert_eq!(p.rest, r#"0.123 "example.com""#);
    }

    #[test]
    fn escaped_quotes_in_fields() {
        // Apache style.
        let p = ok(
            r#"203.0.113.7 - - [10/Oct/2026:13:55:36 +0000] "GET /a HTTP/1.1" 200 10 "-" "evil \"agent\" \\""#,
        );
        assert_eq!(p.record.user_agent, Some(r#"evil \"agent\" \\"#));
        // nginx style.
        let p = ok(
            r#"203.0.113.7 - - [10/Oct/2026:13:55:36 +0000] "GET /a HTTP/1.1" 200 10 "-" "evil \x22agent\x22""#,
        );
        assert_eq!(p.record.user_agent, Some(r"evil \x22agent\x22"));
    }

    #[test]
    fn malformed_requests_are_kept() {
        for raw in [r"\x16\x03\x01\x00\xEE", "", "-", "get / HTTP/1.1", "GET"] {
            let line =
                format!(r#"203.0.113.7 - - [10/Oct/2026:13:55:36 +0000] "{raw}" 400 166 "-" "-""#);
            let p = ok(&line);
            assert_eq!(p.record.request, Request::Malformed(raw), "{raw:?}");
            assert_eq!(p.record.status, 400);
        }
    }

    #[test]
    fn request_shapes() {
        assert_eq!(
            parse_request("GET /a?"),
            Request::Parsed {
                method: "GET",
                path: "/a",
                query: None,
                protocol: None
            }
        );
        assert_eq!(
            parse_request("GET /a b?x=1?y HTTP/1.1"),
            Request::Parsed {
                method: "GET",
                path: "/a b",
                query: Some("x=1?y"),
                protocol: Some("HTTP/1.1")
            }
        );
        assert_eq!(
            parse_request("CONNECT example.com:443 HTTP/1.1"),
            Request::Parsed {
                method: "CONNECT",
                path: "example.com:443",
                query: None,
                protocol: Some("HTTP/1.1")
            }
        );
        assert_eq!(
            parse_request("GET /a b c"),
            Request::Malformed("GET /a b c")
        );
    }

    #[test]
    fn very_long_line() {
        let path = "a".repeat(100_000);
        let line = format!(
            r#"203.0.113.7 - - [10/Oct/2026:13:55:36 +0000] "GET /{path} HTTP/1.1" 414 0 "-" "x""#
        );
        assert!(matches!(
            ok(&line).record.request,
            Request::Parsed { path: p, .. } if p.len() == 100_001
        ));
    }

    #[test]
    fn errors_name_the_field() {
        let cases = [
            ("", ParseError::Ip),
            (
                "not-an-ip - - [10/Oct/2026:13:55:36 +0000] \"GET / HTTP/1.1\" 200 1 \"-\" \"-\"",
                ParseError::Ip,
            ),
            (
                "203.0.113.7 - - 10/Oct/2026 \"GET / HTTP/1.1\" 200 1 \"-\" \"-\"",
                ParseError::Timestamp,
            ),
            (
                "203.0.113.7 - - [10/Oct/2026:13:55:36] \"GET / HTTP/1.1\" 200 1 \"-\" \"-\"",
                ParseError::Timestamp,
            ),
            (
                "203.0.113.7 - - [10/Oct/2026:13:55:36 +0000] GET / HTTP/1.1 200 1",
                ParseError::Request,
            ),
            (
                "203.0.113.7 - - [10/Oct/2026:13:55:36 +0000] \"GET / HTTP/1.1\" OK 1 \"-\" \"-\"",
                ParseError::Status,
            ),
            (
                "203.0.113.7 - - [10/Oct/2026:13:55:36 +0000] \"GET / HTTP/1.1\" 200 x \"-\" \"-\"",
                ParseError::Bytes,
            ),
            (
                "203.0.113.7 - - [10/Oct/2026:13:55:36 +0000] \"GET / HTTP/1.1\" 200 1",
                ParseError::Referer,
            ),
            (
                "203.0.113.7 - - [10/Oct/2026:13:55:36 +0000] \"GET / HTTP/1.1\" 200 1 \"-\"",
                ParseError::UserAgent,
            ),
        ];
        for (line, expected) in cases {
            assert_eq!(parse_combined(line).unwrap_err(), expected, "{line}");
        }
    }
}
