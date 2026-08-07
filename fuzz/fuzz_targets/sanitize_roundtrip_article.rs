//! Guarantee S5 for `Profile::Article`: what the sanitizer emits must survive being reparsed.
//!
//! Three properties, and the reparse is what makes them meaningful. Checking the output string for
//! `<script` proves nothing about what a browser will build from it — mXSS is precisely the case
//! where a benign-looking string parses into something else. So the output goes back through
//! html5ever and the *tree* is inspected.
#![no_main]

use legibility_core::Limits;
use legibility_dom::serialize::{serialize_region, SerializeOptions};
use legibility_sanitize::Article;
use libfuzzer_sys::fuzz_target;

mod shared;

fuzz_target!(|data: &[u8]| {
    let html = String::from_utf8_lossy(data);
    let Some(first) = shared::sanitize::<Article>(&html) else { return };

    // 1. Nothing forbidden survives a reparse of our own output.
    shared::assert_tree_is_clean(&first);

    // 2. A fixpoint within two rounds, which is what the contract commits to (plan M1) — not
    //    within one, which is what a first draft of this asserted and which the HTML spec forbids.
    //
    //    Whitespace before `<html>` is dropped during tree construction, so a document whose first
    //    text is a tab emits that tab once and never again: round 0 gives "\t\u{fffd}~", round 1
    //    gives "\u{fffd}~", round 2 gives "\u{fffd}~". The sequence converges, which is the property
    //    that matters — a consumer re-sanitizing on the way in must not be handed something that
    //    keeps changing — but it converges at one, not zero.
    shared::assert_fixpoint::<Article>(&first);
});

// Keeps the unused-import warning away when `serialize_region` is only used inside `shared`.
#[allow(dead_code)]
fn _uses(a: &legibility_core::Arena, n: legibility_core::NodeId) {
    let _ = serialize_region::<Article>(a, n, SerializeOptions::default());
    let _ = Limits::DEFAULT;
}
