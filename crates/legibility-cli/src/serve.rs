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
    let st = stamp();
    eprintln!("  build:  {}", st.text);
    if let Some(f) = &st.stale {
        // Loud on purpose. A stale module is indistinguishable from a fix that did not work, and
        // reading it as the latter is what wastes a diagnosis round.
        eprintln!(
            "\n  ┌─ STALE MODULE ─────────────────────────────────────────────────\n  \
             │ {f} is newer than the .wasm being served.\n  \
             │ The page will run the OLD engine. Rebuild:\n  \
             │   python3 scripts/build-offline-demo.py\n  \
             └────────────────────────────────────────────────────────────────\n"
        );
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

/// Where the WebAssembly module is: whichever of the two artifacts is **newer**.
///
/// Preferring `.opt.wasm` unconditionally was a trap. `cargo build -p legibility-wasm` writes
/// `legibility_wasm.wasm`; only `scripts/build-offline-demo.py` runs `wasm-opt` and writes
/// `legibility_wasm.opt.wasm`. So once an optimised module existed the helper served it forever, and
/// the rebuild command the STALE banner printed could not change what the page ran — a warning that
/// is true and whose stated remedy does nothing, which is worse than no warning at all, because the
/// only way out of it is to stop believing it.
fn wasm_path() -> Option<PathBuf> {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/wasm32-unknown-unknown/release");
    ["legibility_wasm.opt.wasm", "legibility_wasm.wasm"]
        .iter()
        .map(|n| base.join(n))
        .filter(|p| p.is_file())
        .max_by_key(|p| p.metadata().and_then(|m| m.modified()).ok())
}

/// Repository root, derived from this crate's location.
fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The crates the WebAssembly module is actually built from, per `legibility-wasm`'s manifest.
///
/// Walking all of `crates/` reported STALE for edits to this very file and to
/// `legibility-cli/tests/`, neither of which is an input to the module. A staleness warning that
/// fires when nothing relevant changed gets ignored, and then it is not a warning.
const MODULE_CRATES: [&str; 4] =
    ["legibility-core", "legibility-dom", "legibility-sanitize", "legibility-wasm"];

/// Files outside `crates/` that also change the compiled module.
///
/// A dependency bump or a rustflags change rebuilds the module with no `.rs` touched, and reporting
/// FRESH then is the same failure in the other direction.
const MODULE_CONFIG: [&str; 4] =
    ["Cargo.toml", "Cargo.lock", ".cargo/config.toml", "rust-toolchain.toml"];

/// Newest modification time among the module's inputs, and which file it was.
///
/// Recursion is banned in the engine (S1) and there is no reason to make a dev tool the exception,
/// so the walk carries its own stack.
fn newest_source() -> Option<(std::time::SystemTime, PathBuf)> {
    let mut stack: Vec<PathBuf> =
        MODULE_CRATES.iter().map(|c| root().join("crates").join(c).join("src")).collect();
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    let mut consider = |p: PathBuf, m: std::time::SystemTime| {
        if best.as_ref().is_none_or(|(b, _)| m > *b) {
            best = Some((m, p));
        }
    };
    for name in MODULE_CONFIG {
        let p = root().join(name);
        if let Ok(m) = p.metadata().and_then(|m| m.modified()) {
            consider(p, m);
        }
    }
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for entry in rd.flatten() {
            let p = entry.path();
            match entry.file_type() {
                Ok(t) if t.is_dir() => stack.push(p),
                Ok(t) if t.is_file() && p.extension().is_some_and(|e| e == "rs") => {
                    if let Ok(m) = entry.metadata().and_then(|m| m.modified()) {
                        consider(p, m);
                    }
                }
                _ => {}
            }
        }
    }
    best
}

/// What the page should say about the module it is about to run.
///
/// # Why this is not cosmetic
///
/// `lgb serve` hands over whatever module happens to be sitting in `target/`, and a module compiled
/// before a fix behaves exactly like a fix that did not work. That confusion has now cost four
/// rounds of diagnosis on one page: the engine was correct, the CLI proved it, and the demo kept
/// showing the old answer because the `.wasm` was 37 minutes older than the commit that fixed it.
///
/// The page carried a `__BUILD_STAMP__` placeholder for exactly this, and only the single-file build
/// ever substituted it — so under `lgb serve` the build row displayed the literal text
/// `/*__BUILD_STAMP__*/`, which is worse than showing nothing, because it looks like a value.
struct Stamp {
    text: String,
    /// Set when a source file is newer than the module, naming the file.
    stale: Option<String>,
}

fn stamp() -> Stamp {
    let Some(wasm) = wasm_path() else {
        return Stamp { text: "no module built".into(), stale: None };
    };
    let built = wasm.metadata().and_then(|m| m.modified()).ok();
    let digest = digest_of(&wasm);
    let head = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "nogit".into());

    // Whether anything the module is compiled from has changed since it was compiled. mtime rather
    // than a content hash of the sources: cargo's own staleness check is mtime-based too, so this
    // agrees with whether `cargo build` would do work.
    let stale = match (built, newest_source()) {
        (Some(b), Some((src, path))) if src > b => Some(short(&path)),
        _ => None,
    };
    let text = match &stale {
        Some(f) => format!("STALE — {head} · wasm {digest} · older than {f}"),
        None => format!("{head} · wasm {digest}"),
    };
    Stamp { text, stale }
}

/// First eight hex digits of the module's SHA-256, or `"unhashed"`.
///
/// A subprocess for the same reason `fetch` is one: it keeps a crypto dependency out of a workspace
/// whose engine has none, and this is a dev tool. Matching what `scripts/build-offline-demo.py`
/// prints is the point — the two delivery routes must be comparable by eye.
fn digest_of(p: &std::path::Path) -> String {
    let out = std::process::Command::new("shasum")
        .args(["-a", "256"])
        .arg(p)
        .output()
        .ok()
        .filter(|o| o.status.success());
    out.and_then(|o| {
        String::from_utf8_lossy(&o.stdout)
            .split_whitespace()
            .next()
            .filter(|h| h.len() >= 8)
            .map(|h| h[..8].to_string())
    })
    .unwrap_or_else(|| "unhashed".into())
}

/// A repo-relative-looking path for a message. `root()` is `<crate>/../..`, so it is not a textual
/// prefix of anything `read_dir` produced and `strip_prefix` silently does nothing. `rfind` rather
/// than `find`: the unnormalised root contains `/crates/` itself, so the first match is the wrong
/// one and yields `crates/legibility-cli/../../crates/...`.
fn short(p: &std::path::Path) -> String {
    let s = p.display().to_string();
    match s.rfind("/crates/") {
        Some(at) => s[at + 1..].to_string(),
        None => s,
    }
}

fn git(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(root())
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// The page, with its placeholders filled in.
///
/// Substituted per request rather than once at startup: the module can be rebuilt while the helper
/// is running, and a reload should then show the new stamp. That is the whole point of it.
fn index() -> String {
    INDEX.replace("/*__BUILD_STAMP__*/", &stamp().text)
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
        ("GET", "/") => respond(&mut stream, 200, "text/html; charset=utf-8", index().as_bytes()),
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
        return Err(format!("fetch failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
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
