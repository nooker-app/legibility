//! WebAssembly entry points over a raw linear-memory ABI.
//!
//! # Why not wasm-bindgen
//!
//! The published demo must run from `file://` with no server, which means the module has to be
//! inlined into the HTML as base64 — a `fetch()` for a sibling `.wasm` is blocked by CORS on
//! `file://`, and no client-side code changes that. wasm-bindgen's generated glue expects to load
//! the module itself and pulls in a JS file whose shape is tied to the CLI version. Forty lines of
//! hand-written glue over four exported functions has no such coupling and inlines trivially.
//!
//! The plan reserves wasm-bindgen for an ergonomic npm wrapper later. This is the hot path.
//!
//! # ABI
//!
//! ```text
//!   ptr = lgb_alloc(n)            reserve n bytes
//!   <caller writes UTF-8 HTML into memory[ptr .. ptr+n]>
//!   res = lgb_extract(ptr, n)     returns a pointer to a 4-byte little-endian length,
//!                                 followed by that many bytes of UTF-8 JSON
//!   lgb_free_result()             release it
//! ```
//!
//! Length-prefixing the result rather than returning a pair avoids multi-value returns, which are
//! not universally available, and avoids a second call to ask how long the answer was.
//!
//! # Panics
//!
//! Every entry point is wrapped in `catch_unwind`. On wasm the profile is `panic = abort`
//! (`.cargo/config.toml`), so that wrapper cannot actually catch anything — a wasm panic traps the
//! instance. It is there because the same code is compiled for native targets where it does catch,
//! and because `xtask ffi-audit` requires every `extern "C"` body to be wrapped. For wasm the real
//! defence is guarantee S1: not panicking in the first place.

#![forbid(unsafe_op_in_unsafe_fn)]
#![deny(clippy::all)]
#![warn(clippy::pedantic, missing_docs)]

use std::panic::{catch_unwind, AssertUnwindSafe};

use legibility_core::{Arena, Limits};

// Result buffer, kept alive between `lgb_extract` and `lgb_free_result`.
//
// A thread-local rather than a `static mut`: wasm is single-threaded, but a `static mut` would need
// `unsafe` on every touch and this crate forbids unchecked unsafe.
thread_local! {
    static RESULT: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Reserve `len` bytes for the caller to write input into.
///
/// # Safety
/// The returned pointer is valid for `len` bytes until passed to [`lgb_extract`].
#[no_mangle]
pub extern "C" fn lgb_alloc(len: usize) -> *mut u8 {
    catch_unwind(|| {
        let mut buf = Vec::<u8>::new();
        if buf.try_reserve_exact(len).is_err() {
            return core::ptr::null_mut();
        }
        buf.resize(len, 0);
        let ptr = buf.as_mut_ptr();
        core::mem::forget(buf);
        ptr
    })
    .unwrap_or(core::ptr::null_mut())
}

/// Release a buffer obtained from [`lgb_alloc`] without running an extraction.
///
/// # Safety
/// `ptr` must come from [`lgb_alloc`] with the same `len`, and must not be used afterwards.
#[no_mangle]
pub unsafe extern "C" fn lgb_dealloc(ptr: *mut u8, len: usize) {
    if ptr.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: contract of this function -- ptr/len came from lgb_alloc.
        drop(unsafe { Vec::from_raw_parts(ptr, len, len) });
    }));
}

/// Extract from UTF-8 HTML at `ptr[..len]`.
///
/// Returns a pointer to a 4-byte little-endian length followed by that many bytes of UTF-8 JSON, or
/// null on failure. Consumes the input buffer.
///
/// # Safety
/// `ptr` must come from [`lgb_alloc`] with the same `len`.
#[no_mangle]
pub unsafe extern "C" fn lgb_extract(ptr: *mut u8, len: usize) -> *const u8 {
    if ptr.is_null() {
        return core::ptr::null();
    }
    // SAFETY: contract of this function.
    let input = unsafe { Vec::from_raw_parts(ptr, len, len) };

    let json = catch_unwind(AssertUnwindSafe(|| {
        // Lossy rather than rejecting: a page served with a broken encoding declaration is a page
        // a reader still wants to read, and guarantee S2 says degrade rather than fail.
        let html = String::from_utf8_lossy(&input);
        // BROWSER limits, not DEFAULT: wasm linear memory never shrinks once grown, so a single
        // huge page would keep costing that memory for the instance's whole life.
        run(&html, Limits::BROWSER)
    }))
    .unwrap_or_else(|_| {
        String::from("{\"error\":\"internal panic; see the stability contract in docs/\"}")
    });

    RESULT.with(|cell| {
        let mut out = cell.borrow_mut();
        out.clear();
        let bytes = json.as_bytes();
        let n = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
        out.extend_from_slice(&n.to_le_bytes());
        out.extend_from_slice(bytes);
        out.as_ptr()
    })
}

/// Release the buffer returned by [`lgb_extract`].
#[no_mangle]
pub extern "C" fn lgb_free_result() {
    let _ = catch_unwind(|| {
        RESULT.with(|cell| {
            let mut out = cell.borrow_mut();
            out.clear();
            out.shrink_to_fit();
        });
    });
}

/// Schema version of the JSON this module emits.
///
/// Wrapped like every other export even though returning a constant cannot panic. `xtask ffi-audit`
/// requires it without exception, because a boundary that is wrapped only where someone judged it
/// necessary is a boundary nobody can reason about, and the judgement is the part that rots.
#[no_mangle]
pub extern "C" fn lgb_schema_version() -> u32 {
    catch_unwind(|| 1).unwrap_or(0)
}

/// Run the pipeline and serialize.
///
/// Shared with the CLI through `legibility_dom::json`, so the WASM demo and `lgb extract` cannot
/// drift apart — the determinism gate compares their output byte for byte.
fn run(html: &str, limits: Limits) -> String {
    let (arena, hit) = legibility_dom::BuildArena::parse_to_arena(html, limits);
    let out = legibility_core::extract_all(&arena, limits);
    legibility_dom::json::extraction_json_limited(&arena, &out, hit, None, limits)
}

/// Extract from a `&str`, for native tests of the same path the browser takes.
#[must_use]
pub fn extract_to_json(html: &str) -> String {
    run(html, Limits::BROWSER)
}

/// The arena type, re-exported so consumers need not depend on core directly.
pub type Document = Arena;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_is_wellformed_json_for_hostile_input() {
        for html in ["", "<p>", "<script>alert(1)</script>", "<div><div><div>", "&#x1F600;"] {
            let j = extract_to_json(html);
            assert!(j.starts_with('{') && j.ends_with('}'), "malformed for {html:?}: {j}");
            assert!(j.contains("\"schema_version\":1"));
            assert!(!j.contains("alert(1)"), "script content leaked: {j}");
        }
    }

    #[test]
    fn the_wasm_path_and_the_cli_path_produce_the_same_bytes() {
        // Both go through legibility_dom::json, so this is a guard against someone adding a field
        // to one serializer and not the other -- which would break the cross-target determinism
        // gate in a way that is tedious to diagnose from a hash mismatch.
        let html = "<html><body><article><p>alpha beta gamma</p></article></body></html>";
        let (arena, hit) =
            legibility_dom::BuildArena::parse_to_arena(html, Limits::BROWSER);
        let out = legibility_core::extract_all(&arena, Limits::BROWSER);
        let direct =
            legibility_dom::json::extraction_json_limited(&arena, &out, hit, None, Limits::BROWSER);
        assert_eq!(extract_to_json(html), direct);
    }
}
