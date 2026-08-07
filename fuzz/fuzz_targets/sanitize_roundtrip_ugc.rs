//! Guarantee S5 for `Profile::UserContent` — the profile that handles attacker-controlled input.
//!
//! Separate from the article target rather than parameterised, because plan §1.8 requires the two
//! profiles to pass *independently*: a shared harness that happened to exercise only one of them
//! would satisfy the letter of "both are fuzzed" and none of the intent.
#![no_main]

use legibility_sanitize::UserContent;
use libfuzzer_sys::fuzz_target;

mod shared;

fuzz_target!(|data: &[u8]| {
    let html = String::from_utf8_lossy(data);
    let Some(first) = shared::sanitize::<UserContent>(&html) else { return };

    shared::assert_tree_is_clean(&first);

    // Everything the article profile forbids, plus the two this profile exists to add: no images
    // (plan §1.8 turns them off by default) and no media.
    let lower = first.to_lowercase();
    for banned in ["<img", "<picture", "<source", "<video", "<audio"] {
        assert!(!lower.contains(banned), "{banned} survived UserContent: {first}");
    }

    // 2. A fixpoint within two rounds, which is what the contract commits to (plan M1) — not
    //    within one, which is what a first draft of this asserted and which the HTML spec forbids.
    //
    //    Whitespace before `<html>` is dropped during tree construction, so a document whose first
    //    text is a tab emits that tab once and never again: round 0 gives "\t\u{fffd}~", round 1
    //    gives "\u{fffd}~", round 2 gives "\u{fffd}~". The sequence converges, which is the property
    //    that matters — a consumer re-sanitizing on the way in must not be handed something that
    //    keeps changing — but it converges at one, not zero.
    shared::assert_fixpoint::<UserContent>(&first);
});
