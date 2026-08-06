//! `lgb serve` — the local demo server.
//!
//! This exists so extraction can be judged by looking at it. A JSON blob does not tell you
//! whether the right region was picked; a rendered page does.
//!
//! # Why local rather than the gh-pages demo
//!
//! The published demo (plan §1.12) is a static page, and a static page cannot fetch an arbitrary
//! URL — CORS forbids it, and no amount of client-side code changes that. So the public demo
//! accepts pasted HTML only. Here the fetch happens **server-side**, where CORS does not apply,
//! which is the only way "type a URL and see the result" can work at all.
//!
//! # Why the fetch is a subprocess
//!
//! `curl` rather than an HTTP client crate. Two reasons, and the second is the real one:
//! it keeps TLS out of the dependency tree, and it keeps the network *outside the engine*.
//! `legibility-core` promises no clock, no filesystem and no sockets — that promise is what makes
//! byte-identical cross-target output (S3) structural. A dev tool shelling out is honest; linking
//! a client into the workspace would blur the boundary the guarantee depends on.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};

use legibility_core::Limits;
use legibility_dom::BuildArena;

const INDEX: &str = include_str!("demo.html");

/// Run the demo server until the process is killed.
pub fn serve(port: u16) -> std::io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    eprintln!("legibility demo → http://127.0.0.1:{port}");
    eprintln!("  paste HTML, or enter a URL (fetched server-side, so no CORS)");
    for stream in listener.incoming() {
        match stream {
            // One connection at a time. A dev tool does not need concurrency, and a thread pool
            // would be the largest untested surface in the binary.
            Ok(s) => {
                if let Err(e) = handle(s) {
                    eprintln!("lgb serve: {e}");
                }
            }
            Err(e) => eprintln!("lgb serve: accept: {e}"),
        }
    }
    Ok(())
}

fn handle(mut stream: TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("/").to_string();

    let mut len = 0usize;
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 || h.trim().is_empty() {
            break;
        }
        if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
            len = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; len];
    if len > 0 {
        reader.read_exact(&mut body)?;
    }
    let body = String::from_utf8_lossy(&body).into_owned();

    match (method.as_str(), path.as_str()) {
        ("GET", "/") => respond(&mut stream, 200, "text/html; charset=utf-8", INDEX),
        ("POST", "/extract") => {
            let out = match extract_request(&body) {
                Ok(json) => json,
                Err(e) => format!("{{\"error\":{}}}", json_string(&e)),
            };
            respond(&mut stream, 200, "application/json; charset=utf-8", &out)
        }
        _ => respond(&mut stream, 404, "text/plain; charset=utf-8", "not found"),
    }
}

fn respond(stream: &mut TcpStream, status: u16, ctype: &str, body: &str) -> std::io::Result<()> {
    let reason = if status == 200 { "OK" } else { "Not Found" };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body.as_bytes())?;
    stream.flush()
}

/// Body is `{"html": "..."}` or `{"url": "..."}`. Hand-parsed for the same reason the output is:
/// no serde, so the wire format cannot drift with a dependency.
fn extract_request(body: &str) -> Result<String, String> {
    if let Some(url) = json_field(body, "url") {
        let url = url.trim().to_string();
        if url.is_empty() {
            return Err("empty url".into());
        }
        let html = fetch(&url)?;
        Ok(extract_json(&html, Some(&url)))
    } else if let Some(html) = json_field(body, "html") {
        Ok(extract_json(&html, None))
    } else {
        Err("expected {\"html\":...} or {\"url\":...}".into())
    }
}

/// Fetch a page with `curl`.
///
/// A browser User-Agent because a great many sites serve a different (often empty) document to
/// unknown clients, and extracting from that would measure the wrong thing.
fn fetch(url: &str) -> Result<String, String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("only http and https URLs are accepted".into());
    }
    let out = std::process::Command::new("curl")
        .args([
            "-sSL",
            // Print the final status on its own trailing line so an error page cannot be
            // mistaken for a page we failed to extract from. Without this the demo happily
            // reports "no article" for a 404 body, which sends you debugging the wrong thing.
            "-w",
            "\n__LGB_HTTP__%{http_code}",
            "--max-time",
            "20",
            "--max-filesize",
            "33554432",
            "--compressed",
            "-A",
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/124.0 Safari/537.36",
            url,
        ])
        .output()
        .map_err(|e| format!("curl not available: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "fetch failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let raw = String::from_utf8_lossy(&out.stdout).into_owned();
    let (body, status) = match raw.rsplit_once("\n__LGB_HTTP__") {
        Some((b, s)) => (b.to_string(), s.trim().parse::<u16>().unwrap_or(0)),
        None => (raw, 0),
    };
    if !(200..300).contains(&status) && status != 0 {
        return Err(format!("HTTP {status} from {url} — nothing to extract from"));
    }
    Ok(body)
}

fn extract_json(html: &str, url: Option<&str>) -> String {
    let (arena, hit) = BuildArena::parse_to_arena(html, Limits::DEFAULT);
    let out = legibility_core::extract_all(&arena, Limits::DEFAULT);
    // One serializer for every host (legibility_dom::json). Two would mean the cross-target
    // determinism gate fails on a field-ordering difference rather than on anything real.
    legibility_dom::json::extraction_json(&arena, &out, hit, url)
}



/// Extract a string field from a flat JSON object.
fn json_field(body: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let start = body.find(&needle)? + needle.len();
    let rest = body.get(start..)?;
    let colon = rest.find(':')? + 1;
    let rest = rest.get(colon..)?;
    let mut chars = rest.char_indices().skip_while(|(_, c)| c.is_whitespace());
    let (open_idx, open) = chars.next()?;
    if open != '"' {
        return None;
    }
    let mut out = String::new();
    let mut chars = rest.get(open_idx + 1..)?.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'b' => out.push('\u{8}'),
                'f' => out.push('\u{c}'),
                'u' => {
                    // Proper \uXXXX decoding, surrogate pairs included.
                    //
                    // This used to push U+FFFD and move on, with a comment claiming the demo never
                    // needed it. The browser hid the mistake: JSON.stringify emits raw UTF-8, so
                    // pasted Korean arrived intact. Any client that escapes non-ASCII -- Python's
                    // json.dumps does by default, and so do many HTTP libraries -- had every
                    // non-Latin character replaced and its four hex digits left behind as text.
                    let hi = take_hex4(&mut chars)?;
                    let cp = if (0xD800..0xDC00).contains(&hi) {
                        // High surrogate: a low surrogate must follow, as \uXXXX again.
                        if chars.next()? != '\\' || chars.next()? != 'u' {
                            return None;
                        }
                        let lo = take_hex4(&mut chars)?;
                        if !(0xDC00..0xE000).contains(&lo) {
                            return None;
                        }
                        0x1_0000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                    } else {
                        hi
                    };
                    out.push(char::from_u32(cp).unwrap_or('\u{fffd}'));
                }
                other => out.push(other),
            },
            c => out.push(c),
        }
    }
    None
}

/// Read exactly four hex digits.
fn take_hex4(chars: &mut core::iter::Peekable<core::str::Chars<'_>>) -> Option<u32> {
    let mut v = 0u32;
    for _ in 0..4 {
        v = v * 16 + chars.next()?.to_digit(16)?;
    }
    Some(v)
}

fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str("\\u");
                for shift in [12, 8, 4, 0] {
                    let nibble = ((c as u32) >> shift) & 0xF;
                    out.push(char::from_digit(nibble, 16).unwrap_or('0'));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn json_field_reads_html_and_url() {
        assert_eq!(json_field(r#"{"url":"https://a.test"}"#, "url").as_deref(), Some("https://a.test"));
        assert_eq!(json_field(r#"{"html":"<p>hi</p>"}"#, "html").as_deref(), Some("<p>hi</p>"));
        assert_eq!(json_field(r#"{"html":"a\"b"}"#, "html").as_deref(), Some("a\"b"));
        assert_eq!(json_field(r#"{"html":"a\nb"}"#, "html").as_deref(), Some("a\nb"));
        assert_eq!(json_field(r#"{"other":1}"#, "html"), None);
    }

    #[test]
    fn unicode_escapes_survive_including_korean_and_surrogate_pairs() {
        // The browser hid this: JSON.stringify emits raw UTF-8, so pasted Korean always worked.
        // Python's json.dumps escapes non-ASCII by default, and every such character was being
        // replaced with U+FFFD while its four hex digits stayed behind as literal text.
        assert_eq!(
            json_field(r#"{"html":"\ud55c\uad6d\uc5b4"}"#, "html").as_deref(),
            Some("한국어")
        );
        // Astral plane, as a surrogate pair.
        assert_eq!(
            json_field(r#"{"html":"\ud83d\ude00"}"#, "html").as_deref(),
            Some("\u{1f600}")
        );
        // Mixed with ordinary text and other escapes.
        assert_eq!(
            json_field(r#"{"html":"<p>\ud55c\n\uae00</p>"}"#, "html").as_deref(),
            Some("<p>한\n글</p>")
        );
        // A lone high surrogate is malformed input, not something to guess at.
        assert_eq!(json_field(r#"{"html":"\ud83d"}"#, "html"), None);
    }

    #[test]
    fn only_http_urls_are_fetchable() {
        // file:// would turn the demo server into a local file read primitive.
        for bad in ["file:///etc/passwd", "ftp://x/y", "javascript:1", "/etc/passwd"] {
            assert!(fetch(bad).is_err(), "{bad} must be refused");
        }
    }

    #[test]
    fn extract_json_is_wellformed_for_empty_and_hostile_input() {
        for src in ["", "<p>", "<script>alert(1)</script>", "<html><body><div><p>x</p></div>"] {
            let out = extract_json(src, None);
            assert!(out.starts_with('{') && out.ends_with('}'), "malformed for {src:?}: {out}");
            assert!(out.contains("\"schema_version\":1"));
            assert!(!out.contains("alert(1)"), "script content leaked: {out}");
        }
    }
}
