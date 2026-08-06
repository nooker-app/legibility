//! `lgb serve` — the local helper for the demo page.
//!
//! # What it does, and what it deliberately does not
//!
//! It serves `js/testbed/demo.html`, hands over the WebAssembly module next to it, and fetches URLs
//! on request. **It does not extract.** Extraction happens in the browser, in the same module the
//! single-file build embeds, so the two ways of opening the demo cannot disagree — there is only
//! one engine to disagree with. This used to be a second extraction path in Rust; keeping it meant
//! keeping a copy of the pipeline whose only job was to drift.
//!
//! # Why the URL fetch lives here
//!
//! A browser cannot fetch a third-party page from a document it did not come from — CORS forbids
//! it, on `http://localhost` exactly as much as on `file://`. So something outside the browser has
//! to do it. The two options are a public proxy or a process on the user's own machine, and only
//! one of those keeps the promise the offline build makes. "Offline" here means *nothing but your
//! own device is involved*, which is not the same as *no network*: this helper is on your machine,
//! so using it to pull a page down does not break that.
//!
//! # Why the fetch is a subprocess
//!
//! `curl` rather than an HTTP client crate. Two reasons, and the second is the real one: it keeps
//! TLS out of the dependency tree, and it keeps the network *outside the engine*.
//! `legibility-core` promises no clock, no filesystem and no sockets — that promise is what makes
//! byte-identical cross-target output (S3) structural. A dev tool shelling out is honest; linking a
//! client into the workspace would blur the boundary the guarantee depends on.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;

/// The demo page. The same file the offline build inlines the module into, unsubstituted here so
/// the browser fetches the module from `/legibility.wasm` instead.
const INDEX: &str = include_str!("../../../js/testbed/demo.html");

/// Run the helper until the process is killed.
pub fn serve(port: u16) -> std::io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    eprintln!("legibility demo → http://127.0.0.1:{port}");
    match wasm_path() {
        Some(p) => eprintln!("  module: {}", p.display()),
        None => eprintln!(
            "  module: NOT FOUND — the page will not run. Build it with:\n    \
             cargo build --release -p legibility-wasm --target wasm32-unknown-unknown"
        ),
    }
    eprintln!("  paste HTML, choose a file, or enter a URL (fetched here, so no CORS)");
    for stream in listener.incoming() {
        match stream {
            // One connection at a time. A dev tool does not need concurrency, and a thread pool
            // would be the largest untested surface in the binary.
            //
            // One consequence worth knowing: asking the helper to fetch *its own* URL deadlocks it
            // against itself until curl times out, because it cannot answer while it is waiting.
            // Any other URL is fine, since only one request is ever in flight from the page.
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

/// Where the WebAssembly module is, preferring the `wasm-opt` output when it exists.
fn wasm_path() -> Option<PathBuf> {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/wasm32-unknown-unknown/release");
    for name in ["legibility_wasm.opt.wasm", "legibility_wasm.wasm"] {
        let p = base.join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

fn handle(mut stream: TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("/").to_string();

    // Drain the headers. Nothing here needs them, but the request must be consumed before the
    // response or the client sees a reset instead of a reply.
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 || h.trim().is_empty() {
            break;
        }
    }

    let (path, query) = target.split_once('?').unwrap_or((target.as_str(), ""));
    match (method.as_str(), path) {
        ("GET", "/") => respond(&mut stream, 200, "text/html; charset=utf-8", INDEX.as_bytes()),
        ("GET", "/legibility.wasm") => match wasm_path().and_then(|p| std::fs::read(p).ok()) {
            Some(bytes) => respond(&mut stream, 200, "application/wasm", &bytes),
            None => respond(
                &mut stream,
                503,
                "text/plain; charset=utf-8",
                b"the wasm module is not built; run:\n  \
                  cargo build --release -p legibility-wasm --target wasm32-unknown-unknown\n",
            ),
        },
        ("GET", "/fetch") => {
            let url = query_param(query, "url").unwrap_or_default();
            match fetch(&url) {
                Ok(html) => respond(&mut stream, 200, "text/plain; charset=utf-8", html.as_bytes()),
                Err(e) => respond(&mut stream, 502, "text/plain; charset=utf-8", e.as_bytes()),
            }
        }
        _ => respond(&mut stream, 404, "text/plain; charset=utf-8", b"not found"),
    }
}

fn respond(stream: &mut TcpStream, status: u16, ctype: &str, body: &[u8]) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
        502 => "Bad Gateway",
        _ => "Service Unavailable",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\n\
         Access-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    // `Access-Control-Allow-Origin: *` is what lets the single-file page, whose origin is `null`
    // because it came from disk, talk to this helper at all. A plain GET with no custom headers
    // also avoids a preflight, so no OPTIONS handler is needed.
    stream.write_all(body)?;
    stream.flush()
}

/// Value of `name` in a query string, percent-decoded.
fn query_param(query: &str, name: &str) -> Option<String> {
    query
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| percent_decode(v))
}

/// Percent-decode, treating `+` as a space and leaving malformed escapes alone.
///
/// Byte-level, then one UTF-8 conversion at the end: decoding per character would split multi-byte
/// sequences, which is how a URL with a Korean path turns into replacement characters.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                let hex = |c: u8| (c as char).to_digit(16);
                match (hex(b[i + 1]), hex(b[i + 2])) {
                    (Some(h), Some(l)) => {
                        out.push((h * 16 + l) as u8);
                        i += 3;
                    }
                    _ => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Fetch a page with `curl`.
///
/// A browser User-Agent because a great many sites serve a different (often empty) document to
/// unknown clients, and extracting from that would measure the wrong thing.
fn fetch(url: &str) -> Result<String, String> {
    let url = url.trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("only http and https URLs are accepted".into());
    }
    let out = std::process::Command::new("curl")
        .args([
            "-sSL",
            // Print the final status on its own trailing line so an error page cannot be mistaken
            // for a page we failed to extract from. Without this the demo happily reports
            // "no article" for a 404 body, which sends you debugging the wrong thing.
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn only_http_urls_are_fetchable() {
        // file:// would turn the helper into a local file read primitive for any page that can
        // reach it, and `Access-Control-Allow-Origin: *` means every page can.
        for bad in ["file:///etc/passwd", "ftp://x/y", "javascript:1", "/etc/passwd", ""] {
            assert!(fetch(bad).is_err(), "{bad} must be refused");
        }
    }

    #[test]
    fn query_params_survive_non_ascii_and_escaped_delimiters() {
        // The predecessor of this function decoded `\uXXXX` per character and replaced every
        // non-ASCII codepoint with U+FFFD, which went unnoticed because browsers send raw UTF-8.
        // Percent-encoding always splits non-ASCII into bytes, so the same mistake here would be
        // unconditional rather than occasional.
        assert_eq!(
            query_param("url=https%3A%2F%2Fko.wikipedia.org%2Fwiki%2F%ED%95%9C%EA%B5%AD", "url")
                .as_deref(),
            Some("https://ko.wikipedia.org/wiki/한국")
        );
        // A query string inside the target URL must not end the parameter early.
        assert_eq!(
            query_param("url=https%3A%2F%2Fa.test%2Fx%3Fid%3D1%26p%3D2", "url").as_deref(),
            Some("https://a.test/x?id=1&p=2")
        );
        assert_eq!(query_param("other=1", "url"), None);
        // Malformed escapes are left as-is rather than swallowed.
        assert_eq!(query_param("url=100%zz", "url").as_deref(), Some("100%zz"));
    }

    #[test]
    fn the_served_page_is_the_same_template_the_offline_build_uses() {
        // If these ever diverge, the two ways of opening the demo stop being the same demo. The
        // placeholder must still be unsubstituted here: that is what makes the browser fetch the
        // module from /legibility.wasm rather than decode an embedded copy.
        assert!(INDEX.contains("__WASM_BASE64__"), "template already substituted");
        assert!(INDEX.contains("legibility.wasm"), "no fallback fetch of the module");
    }
}
