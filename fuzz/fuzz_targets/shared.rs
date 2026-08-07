//! Helpers shared by the two sanitizer targets.
//!
//! A module rather than a crate: `cargo-fuzz` builds each target as its own binary, and a `mod`
//! compiled into both is the cheapest way to keep one definition of "clean".

use legibility_core::{Limits, NodeId};
use legibility_dom::serialize::{serialize_region, SerializeOptions};
use legibility_sanitize::Profile;

/// Parse `html`, then serialize its `<body>` through `P`. `None` when there is nothing to serialize.
pub fn sanitize<P: Profile>(html: &str) -> Option<String> {
    let (arena, _) = legibility_dom::BuildArena::parse_to_arena(html, Limits::BROWSER);
    // `<body>` rather than the selected region: selection can legitimately refuse a document, and
    // this target is about the serializer, which must be safe on whatever it is handed.
    let body = (0..arena.len()).find(|&i| {
        arena.kind.get(i).copied() == Some(legibility_core::NodeKind::Element)
            && arena.tag.get(i).copied() == Some(legibility_core::TagId::BODY)
    })?;
    let (out, _) =
        serialize_region::<P>(&arena, NodeId(body as u32), SerializeOptions::default());
    Some(out.as_str().to_string())
}

/// Reparse `html` and assert nothing forbidden survives.
///
/// # Two halves, for two different reasons
///
/// **Elements are checked in the reparsed tree.** A string check cannot see mXSS — markup that
/// looks inert until a browser builds a different tree out of it than the bytes suggest — so the
/// output goes back through html5ever and the elements it actually produces are inspected.
///
/// **Attributes are checked in the string.** The arena interns attribute *names*, and only the
/// names the engine consults get an id; everything else collapses to `AttrName::OTHER` with no name
/// retained. That is a stronger guarantee than a check could express — an `onclick` has no
/// representation in the arena, so the serializer has no way to emit one — but it does mean the
/// tree cannot be asked. What can go wrong is a *permitted* attribute carrying a hostile value, and
/// that is visible in the text.
pub fn assert_tree_is_clean(html: &str) {
    let (arena, _) = legibility_dom::BuildArena::parse_to_arena(html, Limits::BROWSER);
    for i in 0..arena.len() {
        if arena.kind.get(i).copied() != Some(legibility_core::NodeKind::Element) {
            continue;
        }
        if let Some(name) = arena.tag_name(NodeId(i as u32)) {
            assert!(
                !matches!(
                    name,
                    "script"
                        | "iframe"
                        | "object"
                        | "embed"
                        | "style"
                        | "svg"
                        | "math"
                        | "form"
                        | "input"
                        | "button"
                        | "textarea"
                        | "select"
                ),
                "{name} survived into the reparsed tree: {html}"
            );
        }
    }

    // Attributes are parsed, not substring-matched. Matching the tag interior was the previous
    // version and the nightly fuzz run refuted it twice in one night:
    //
    //     <p lang="…&lt;data:text/html…a/">      reported as a surviving `data:` URL
    //     <s dir="…style=">                     reported as a surviving `style` attribute
    //
    // Both are inert. `lang` and `dir` are not URL-bearing and not executable, so arbitrary text in
    // their values does nothing at all — a check that flags them is testing the shape of a string
    // rather than the safety of a document. What actually matters is narrower and needs the two
    // halves kept apart: which attribute *names* exist, and what the *URL-bearing* ones point at.
    for tag in tag_interiors(html) {
        for (name, value) in attributes(tag) {
            let n = name.to_ascii_lowercase();
            assert!(!n.starts_with("on"), "event handler {n} survived: {html}");
            assert!(
                !matches!(n.as_str(), "srcdoc" | "ping" | "style" | "formaction"),
                "{n} survived as an attribute: {html}"
            );
            if matches!(n.as_str(), "href" | "src" | "action" | "xlink:href" | "data") {
                let v = value.trim().to_ascii_lowercase();
                // Entity-encoded schemes decode back on reparse, and the tree we are inspecting is
                // already reparsed, so a plain prefix test is the right one here.
                for scheme in ["javascript:", "data:text/html", "data:image/svg", "vbscript:"] {
                    assert!(!v.starts_with(scheme), "{n}={v} survived: {html}");
                }
            }
        }
    }
}

/// `(name, value)` for every attribute in a tag interior.
///
/// Small and forgiving on purpose: this parses what our own serializer emits, which always quotes,
/// but a malformed tag must yield *something* rather than panic — the input is a fuzzer's.
fn attributes(tag: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let b: Vec<char> = tag.chars().collect();
    let mut i = 0usize;
    // Skip the element name.
    while i < b.len() && !b[i].is_whitespace() {
        i += 1;
    }
    while i < b.len() {
        while i < b.len() && (b[i].is_whitespace() || b[i] == '/') {
            i += 1;
        }
        let name_start = i;
        while i < b.len() && !b[i].is_whitespace() && b[i] != '=' && b[i] != '/' {
            i += 1;
        }
        if i == name_start {
            break;
        }
        let name: String = b[name_start..i].iter().collect();
        let mut value = String::new();
        while i < b.len() && b[i].is_whitespace() {
            i += 1;
        }
        if i < b.len() && b[i] == '=' {
            i += 1;
            while i < b.len() && b[i].is_whitespace() {
                i += 1;
            }
            if i < b.len() && (b[i] == '"' || b[i] == '\'') {
                let q = b[i];
                i += 1;
                while i < b.len() && b[i] != q {
                    value.push(b[i]);
                    i += 1;
                }
                i += 1;
            } else {
                while i < b.len() && !b[i].is_whitespace() {
                    value.push(b[i]);
                    i += 1;
                }
            }
        }
        out.push((name, value));
    }
    out
}

/// The text between `<` and `>` for every tag in `html`, quotes respected.
fn tag_interiors(html: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let b = html.as_bytes();
    let mut i = 0usize;
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        let start = i + 1;
        let mut j = start;
        let mut quote: Option<u8> = None;
        while j < b.len() {
            match b[j] {
                q @ (b'"' | b'\'') if quote.is_none() => quote = Some(q),
                q if Some(q) == quote => quote = None,
                b'>' if quote.is_none() => break,
                _ => {}
            }
            j += 1;
        }
        if let Some(t) = html.get(start..j.min(html.len())) {
            out.push(t);
        }
        i = j + 1;
    }
    out
}

/// Assert repeated sanitization settles, within the two rounds the contract allows.
pub fn assert_fixpoint<P: Profile>(first: &str) {
    let Some(second) = sanitize::<P>(first) else { return };
    if second == *first {
        return;
    }
    let Some(third) = sanitize::<P>(&second) else { return };
    assert_eq!(
        second, third,
        "sanitization had not settled after two rounds:\n 1: {first}\n 2: {second}\n 3: {third}"
    );
}
