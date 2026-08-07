//! Guarantee S1 against arbitrary bytes: no panic, no abort, no stack overflow, ever.
//!
//! The whole input is the document. HTML has no invalid input — the spec defines a parse for every
//! byte sequence — so anything this finds is ours.
#![no_main]

use legibility_core::Limits;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Lossy rather than skipping non-UTF-8: a real page can arrive as mis-decoded bytes, and the
    // replacement characters that produces are exactly the input we want under test.
    let html = String::from_utf8_lossy(data);

    // The browser profile rather than DEFAULT: its caps are the tightest that ship, so the limit
    // paths — which are the ones with no other coverage — are actually exercised.
    let limits = Limits::BROWSER;
    let (arena, _hit) = legibility_dom::BuildArena::parse_to_arena(&html, limits);
    let out = legibility_core::extract_all(&arena, limits);

    // A limit must degrade, never fail: every run answers with a region or with a reason.
    assert!(
        out.selection.article.is_some() || out.selection.no_article.is_some(),
        "extraction produced neither a region nor a reason"
    );

    // Serialization is part of the promise; a panic here is as bad as one in the engine.
    let _ = legibility_dom::json::extraction_json_limited(&arena, &out, _hit, None, limits);
});
