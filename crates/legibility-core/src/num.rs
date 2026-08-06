//! Numeric policy. Guarantee S3 is "same input bytes produce the same output bytes on every
//! target, forever", and floating point is one of only two ways that breaks (the other is
//! hash iteration order, excluded by `no_std`).
//!
//! The rules, enforced by `clippy.toml` rather than by review:
//!
//! * **No transcendental ops.** `exp`, `ln`, `log`, `powf` and `mul_add` are not guaranteed
//!   bit-identical across targets — they come from the platform libm, and wasm, aarch64 and
//!   `x86_64` do not agree to the last bit. The one place a sigmoid was genuinely wanted
//!   (`p_prose`) uses [`piecewise_squash`] instead.
//! * **No NaN reachable.** Every division goes through [`guarded_div`]. NaN poisons
//!   comparisons silently: `NaN < x` and `NaN > x` are both false, so a single NaN turns a
//!   ranking into whatever order the sort happened to visit.
//! * **Ranking never compares floats.** Scores are quantized to integers and tie-broken by
//!   document order — see [`rank_key`]. `partial_cmp().unwrap()` is denied.
//!
//! `f64` is used rather than fixed point. Fixed point (Q16.16) is the documented escape
//! hatch if the cross-target determinism gate ever actually flakes, but adopting it now would
//! mean fixing the dynamic range of every derived feature before a single one has been
//! measured, and that tax lands on every later milestone.

/// Division that cannot produce NaN or infinity.
///
/// A zero denominator yields `0.0`, which is the meaningful answer for every ratio in this
/// engine: a node with no prose has no link density, and no text density. Returning `0.0`
/// keeps such nodes comparable and losing, rather than incomparable.
#[must_use]
#[inline]
pub fn guarded_div(numerator: f32, denominator: f32) -> f32 {
    if denominator == 0.0 || !denominator.is_finite() || !numerator.is_finite() {
        0.0
    } else {
        let q = numerator / denominator;
        if q.is_finite() {
            q
        } else {
            0.0
        }
    }
}

/// Monotone map from an unbounded weighted sum into `[0, 1]`, replacing a logistic sigmoid.
///
/// Three linear segments, chosen so the shape is sigmoid-like where it matters (a soft knee
/// near the decision boundary, saturation at the extremes) while using only multiply, add and
/// compare — all exactly reproducible on every target.
///
/// The segment boundaries are deliberately round numbers rather than fitted constants: they
/// are guesses awaiting a corpus, and are annotated as such because an unannotated magic
/// number is indistinguishable from a measured one six months later.
#[must_use]
#[inline]
pub fn piecewise_squash(x: f32) -> f32 {
    // GUESS (no corpus yet): knee at |x| = 1.0, saturation at |x| = 4.0.
    const KNEE: f32 = 1.0;
    const SAT: f32 = 4.0;
    // Slope inside the knee, and outside it, so the pieces meet continuously at 0.5 +/- 0.25.
    const INNER_SLOPE: f32 = 0.25;
    const OUTER_SLOPE: f32 = 0.083_333_336;

    if x <= -SAT {
        0.0
    } else if x >= SAT {
        1.0
    } else if x < -KNEE {
        // From 0.0 at -SAT up to 0.25 at -KNEE.
        (x + SAT) * OUTER_SLOPE
    } else if x > KNEE {
        // From 0.75 at KNEE up to 1.0 at SAT.
        0.75 + (x - KNEE) * OUTER_SLOPE
    } else {
        // Linear through (0, 0.5).
        0.5 + x * INNER_SLOPE
    }
}

/// Quantize a `[0, 1]` score to `u16` over `[0, 10_000]`.
///
/// Every threshold comparison uses the quantized value. Comparing the pre-quantization float
/// on one target and the quantized value on another is how two builds disagree about a
/// borderline page while both "compute the same score".
#[must_use]
#[inline]
pub fn quantize(score: f32) -> u16 {
    // NaN first, and on its own. Folding NaN together with the infinities under a single
    // `!is_finite()` check sends `+INFINITY` to 0 — turning the strongest possible candidate
    // into the weakest. NaN has no defensible position in an ordering, so it gets the losing
    // end; the infinities saturate in the direction they point.
    if score.is_nan() {
        return 0;
    }
    if score <= 0.0 {
        return 0;
    }
    if score >= 1.0 {
        return 10_000;
    }
    // Truncation, not rounding: rounding modes are another place targets can differ.
    (score * 10_000.0) as u16
}

/// Total-order ranking key: quantized score descending, then document order ascending.
///
/// Returned as a tuple of integers so callers sort with `Ord` and never touch `partial_cmp`.
/// The document-order tie-break is not cosmetic — without it, two equally-scored candidates
/// are ordered by whatever the sort algorithm did, which differs between targets and between
/// element counts, and the parity gate then fails intermittently for no visible reason.
#[must_use]
#[inline]
pub fn rank_key(score: f32, node_index: u32) -> (core::cmp::Reverse<u16>, u32) {
    (core::cmp::Reverse(quantize(score)), node_index)
}

#[cfg(test)]
// Tests may unwrap and may assert on const-evaluable predicates: these are regression
// guards against someone changing a const fn, not runtime checks.
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::assertions_on_constants)]
mod tests {
    use super::*;

    #[test]
    fn guarded_div_never_yields_nan_or_inf() {
        for &(n, d) in &[
            (1.0_f32, 0.0_f32),
            (0.0, 0.0),
            (f32::INFINITY, 1.0),
            (1.0, f32::INFINITY),
            (f32::NAN, 1.0),
            (1.0, f32::NAN),
            (-1.0, 0.0),
        ] {
            let q = guarded_div(n, d);
            assert!(q.is_finite(), "guarded_div({n}, {d}) = {q}");
        }
        assert!((guarded_div(1.0, 4.0) - 0.25).abs() < f32::EPSILON);
    }

    #[test]
    fn piecewise_squash_is_monotone_and_bounded() {
        let mut prev = -1.0_f32;
        let mut x = -6.0_f32;
        while x <= 6.0 {
            let y = piecewise_squash(x);
            assert!((0.0..=1.0).contains(&y), "squash({x}) = {y} out of range");
            assert!(y >= prev, "squash not monotone at {x}: {y} < {prev}");
            prev = y;
            x += 0.05;
        }
        assert!((piecewise_squash(0.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn quantize_saturates_and_never_wraps() {
        assert_eq!(quantize(f32::NAN), 0);
        assert_eq!(quantize(-1.0), 0);
        assert_eq!(quantize(0.0), 0);
        assert_eq!(quantize(1.0), 10_000);
        assert_eq!(quantize(2.0), 10_000);
        assert_eq!(quantize(f32::INFINITY), 10_000);
    }

    #[test]
    fn rank_key_breaks_ties_by_document_order() {
        let mut v = [rank_key(0.5, 7), rank_key(0.5, 3), rank_key(0.9, 9)];
        v.sort_unstable();
        // Highest score first, then earliest document position among equals.
        assert_eq!(v[0], rank_key(0.9, 9));
        assert_eq!(v[1], rank_key(0.5, 3));
        assert_eq!(v[2], rank_key(0.5, 7));
    }
}
