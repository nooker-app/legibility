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

    // Attribute checks apply to **tag interiors only**. Scanning the whole string was the first
    // version and it is wrong twice over: an article may legitimately contain the text
    // `javascript:alert(1)` inside a code sample, and the fuzzer found a document whose *text* read
    // ` oN#…='`, which a naive " on"-then-"=" scan reads as an event handler. Text is escaped for
    // `<` and `&` but is otherwise passed through verbatim, which is the whole point of it.
    for tag in tag_interiors(html) {
        let lower = tag.to_ascii_lowercase();
        for banned in ["javascript:", "data:text/html", "data:image/svg", "srcdoc", "ping=", "style="]
        {
            assert!(!lower.contains(banned), "{banned} survived in <{tag}>: {html}");
        }
        // Any `on…=` pair, which inside a tag can only be an event handler.
        let mut rest = lower.as_str();
        while let Some(at) = rest.find(" on") {
            rest = &rest[at + 1..];
            let name_end = rest.find(['=', ' ']).unwrap_or(rest.len());
            assert!(
                !rest[name_end..].starts_with('='),
                "an event handler survived in <{tag}>: {html}"
            );
        }
    }
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
