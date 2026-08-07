//! Guarantee S3, differentially: the same bytes must produce the same output every time.
//!
//! Not a cross-*target* check — that is the M0 gate's job and needs two runtimes. This is the other
//! half: within one process, extraction must not depend on anything but the document. Allocator
//! addresses, iteration order over a map, a `sort_by` that is not a total order, an `f32`
//! accumulation whose order varies — all of them produce a result that is stable when you run it
//! once and different when you run it twice, and none of them is visible in a single run.
#![no_main]

use legibility_core::Limits;
use libfuzzer_sys::fuzz_target;

fn run(html: &str) -> String {
    let limits = Limits::BROWSER;
    let (arena, hit) = legibility_dom::BuildArena::parse_to_arena(html, limits);
    let out = legibility_core::extract_all(&arena, limits);
    legibility_dom::json::extraction_json_limited(&arena, &out, hit, None, limits)
}

fuzz_target!(|data: &[u8]| {
    let html = String::from_utf8_lossy(data);
    let first = run(&html);

    // Allocate between the runs so the second sees a different heap. A result that depends on an
    // address survives a back-to-back comparison and dies here.
    let churn: Vec<String> = (0..16).map(|i| "x".repeat(i * 97)).collect();
    core::hint::black_box(&churn);

    let second = run(&html);
    assert_eq!(first, second, "the same input produced two different outputs");
});
