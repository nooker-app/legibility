//! Article region selection (plan M4).
//!
//! # What went wrong with density alone
//!
//! The M0 placeholder scored `text_density × (1 − link_density)`, and on real pages it picked a
//! 65-byte `<li>` out of Wikipedia's 53 KB of prose. The arithmetic explains itself: a list item
//! holding 65 characters across 3 elements has density 21, while the article container holding
//! 40 000 characters across 5 000 elements has density 8. **Density alone always prefers small
//! leaves**, because it measures concentration and says nothing about quantity.
//!
//! The missing signal is [`Candidate::text_share`] — how much of the page's prose a region holds.
//! Neither term works alone: share alone always picks `<body>` (share 1.0 by definition), density
//! alone always picks a leaf. Their product is the smallest thing that is not obviously wrong.
//!
//! # Why this is still not length-dependent
//!
//! Both factors are ratios, so scaling every text node by *k* multiplies every candidate's
//! evidence by the same *k* and the argmax is untouched. Contrast Readability, whose score has an
//! absolute `min(floor(len/100), 3)` term that **saturates** early — past that,
//! additional text stops counting, so a 300-character boilerplate block and a 30 000-character
//! article are indistinguishable on that term.
//!
//! There is no absolute character threshold anywhere in this module. That is checked by a test,
//! not by inspection.

use alloc::vec::Vec;

use crate::arena::{Arena, NodeId, NodeKind};
use crate::num::{guarded_div, quantize, rank_key};
use crate::tag::TagId;
use crate::NoArticle;

/// Page-level statistics that make every decision relative rather than absolute.
#[derive(Debug, Clone, Copy, Default)]
pub struct PageStats {
    /// Total prose bytes in the document.
    pub page_prose_len: u32,
    /// Number of candidate containers considered.
    pub competitors: u32,
    /// Median candidate text density, from a log-spaced histogram.
    pub median_density: f32,
    /// Interquartile range of candidate density, used to detect a degenerate distribution.
    pub density_iqr: f32,
}

/// A scored candidate region.
#[derive(Debug, Clone, Copy)]
pub struct Candidate {
    /// The node.
    pub node: NodeId,
    /// Share of the page's prose held by this subtree, in `[0, 1]`.
    pub text_share: f32,
    /// Prose bytes per descendant element.
    pub density: f32,
    /// Prose inside `<a>` over total prose, in `[0, 1]`.
    pub link_density: f32,
    /// Prose over all text (prose + control + hidden + alt) in the subtree.
    ///
    /// A region full of button labels and hidden promos is impure even if its prose is dense.
    pub purity: f32,
    /// `text_share × density × (1 − link_density) × purity`.
    pub evidence: f32,
}

/// Minimum candidates before page-relative statistics mean anything.
///
/// Below this, a z-score is computed from a sample too small to have a shape — print views, AMP
/// pages and RSS-rendered pages routinely have one or two containers. The dispersion floor
/// (plan §M4) substitutes explicit priors and caps confidence instead of pretending.
const MIN_COMPETITORS: u32 = 3;

/// Container elements that can be an article region.
///
/// `<body>` and `<html>` are absent on purpose: selecting `<body>` is the silent fallback that
/// defect 1 exists to remove. If nothing else qualifies the answer is [`NoArticle`], not the
/// whole page.
fn is_candidate_tag(tag: TagId) -> bool {
    matches!(
        tag,
        TagId::DIV
            | TagId::ARTICLE
            | TagId::MAIN
            | TagId::SECTION
            | TagId::TD
            | TagId::LI
            | TagId::BLOCKQUOTE
            | TagId::DL
            | TagId::TABLE
    )
}

/// Collect and score every candidate, then compute the page-relative statistics.
///
/// `masked_prose` subtracts comment-thread prose per node (see `crate::groups`). Passing all zeros
/// scores the document as if comments were part of the article, which is what Readability does and
/// why a long first comment wins on Hacker News.
#[must_use]
pub fn collect_masked(arena: &Arena, masked_prose: &[u32]) -> (Vec<Candidate>, PageStats) {
    let page_prose = arena
        .prose_len
        .first()
        .copied()
        .unwrap_or(0)
        .saturating_sub(masked_prose.first().copied().unwrap_or(0));
    let mut cands = Vec::new();

    for i in 0..arena.len() {
        if arena.kind.get(i).copied() != Some(NodeKind::Element) {
            continue;
        }
        let tag = arena.tag.get(i).copied().unwrap_or(TagId::UNKNOWN);
        if !is_candidate_tag(tag) {
            continue;
        }
        let node = NodeId(i as u32);
        let masked = masked_prose.get(i).copied().unwrap_or(0);
        let prose = arena
            .prose_len
            .get(i)
            .copied()
            .unwrap_or(0)
            .saturating_sub(masked);
        if prose == 0 {
            continue;
        }

        let control = arena.control_len.get(i).copied().unwrap_or(0);
        let hidden = arena.hidden_len.get(i).copied().unwrap_or(0);
        let alt = arena.alt_len.get(i).copied().unwrap_or(0);
        let all_text = prose
            .saturating_add(control)
            .saturating_add(hidden)
            .saturating_add(alt);

        let text_share = guarded_div(prose as f32, page_prose as f32);
        let density = arena.text_density(node);
        let link_density = arena.link_density(node).clamp(0.0, 1.0);
        let purity = guarded_div(prose as f32, all_text as f32);
        let evidence = text_share * density * (1.0 - link_density) * purity;

        cands.push(Candidate {
            node,
            text_share,
            density,
            link_density,
            purity,
            evidence,
        });
    }

    let stats = page_stats(page_prose, &cands);
    (cands, stats)
}

/// [`collect_masked`] with no mask.
#[must_use]
pub fn collect(arena: &Arena) -> (Vec<Candidate>, PageStats) {
    let zeros = alloc::vec![0u32; arena.len()];
    collect_masked(arena, &zeros)
}

/// Median and IQR of candidate density via a 256-bucket log-spaced histogram.
///
/// A histogram rather than a sort: it is O(n) with a fixed bucket count, so the cost does not
/// grow with candidate count, and — more importantly — it is deterministic without depending on
/// sort stability, which guarantee S3 cares about.
fn page_stats(page_prose: u32, cands: &[Candidate]) -> PageStats {
    const BUCKETS: usize = 256;
    let mut hist = [0u32; BUCKETS];
    for c in cands {
        if let Some(slot) = hist.get_mut(bucket_of(c.density)) {
            *slot = slot.saturating_add(1);
        }
    }
    let total: u32 = hist.iter().sum();
    PageStats {
        page_prose_len: page_prose,
        competitors: cands.len() as u32,
        median_density: percentile(&hist, total, 50),
        density_iqr: percentile(&hist, total, 75) - percentile(&hist, total, 25),
    }
}

/// Log-spaced bucket index for a density value.
///
/// Log spacing because densities span orders of magnitude — a nav block sits near 1 and a dense
/// paragraph near 100 — and linear buckets would put almost everything in bucket 0.
fn bucket_of(density: f32) -> usize {
    const OCTAVE_BIAS: i32 = 8; // lowest represented density is 2^-8
    const SUB_BITS: u32 = 3; // 8 sub-buckets per octave
    const OCTAVES: i32 = 32;

    if !density.is_finite() || density <= 0.0 {
        return 0;
    }
    // Integer log2 from the float exponent: no `ln`, which is banned for cross-target
    // reproducibility (see crate::num). 8 sub-buckets per octave over 32 octaves.
    //
    // Two non-monotonicity bugs lived here, both caused by clamping the OCTAVE while keeping the
    // mantissa sub-index. Clamping low collapsed every density below 1.0 together, so 0.65 landed
    // above 1.0. Clamping high did the mirror image: 208.8 landed above 271.4. A histogram whose
    // index is not monotone in its input makes the median arbitrary, which is worse than having no
    // median at all -- so saturation now applies to the whole index, never to the octave alone.
    let bits = density.to_bits();
    let exp = ((bits >> 23) & 0xFF) as i32 - 127;
    let octave = exp + OCTAVE_BIAS;
    if octave < 0 {
        return 0;
    }
    if octave >= OCTAVES {
        return 255;
    }
    let sub = ((bits >> (23 - SUB_BITS)) & ((1 << SUB_BITS) - 1)) as i32;
    let idx = (octave << SUB_BITS) + sub;
    (idx as usize).min(255)
}

/// Value at `p`th percentile, reconstructed as the low edge of the containing bucket.
fn percentile(hist: &[u32; 256], total: u32, p: u32) -> f32 {
    if total == 0 {
        return 0.0;
    }
    let target = (total.saturating_mul(p)) / 100;
    let mut acc = 0u32;
    for (i, &n) in hist.iter().enumerate() {
        acc = acc.saturating_add(n);
        if acc > target {
            return bucket_low_edge(i);
        }
    }
    bucket_low_edge(255)
}

fn bucket_low_edge(idx: usize) -> f32 {
    // Exact inverse of bucket_of, same bias and same sub-bucket width.
    let exp = (idx >> 3) as i32 - 8;
    let sub = (idx & 0b111) as u32;
    // Reconstruct 2^exp * (1 + sub/8) without powf.
    let base = f32::from_bits((((exp + 127) as u32) << 23) | (sub << 20));
    if base.is_finite() {
        base
    } else {
        0.0
    }
}

/// The chosen region, or why there is none.
#[derive(Debug, Clone, Copy)]
pub struct Selection {
    /// Winning region, if accepted.
    pub article: Option<NodeId>,
    /// Reason, when no region was accepted.
    pub no_article: Option<NoArticle>,
    /// Quantized to `[0, 10_000]`. Threshold comparisons use this, never the float it came from.
    pub confidence: u16,
    /// Set when the candidate distribution was too degenerate for page-relative statistics and
    /// explicit priors were substituted (plan §M4 dispersion floor).
    pub dispersion_floor_used: bool,
    /// Set when the winner is a legitimate argmax that happens to be the outermost container,
    /// as distinct from a fallback. These are different facts and must not share a flag.
    pub region_is_outermost_by_argmax: bool,
}

/// Select the article region.
///
/// Deliberately has no `min_chars` parameter and compares no length against a constant. The
/// accept decision is a margin over the *runner-up* plus a purity floor, both relative.
#[must_use]
pub fn select_article(arena: &Arena) -> Selection {
    let zeros = alloc::vec![0u32; arena.len()];
    select_article_masked(arena, &zeros)
}

/// Select the article region with comment prose masked out.
///
/// The mask is applied *before* scoring rather than filtered afterwards, which is the whole point:
/// once a long comment thread is in the candidate pool it wins, and no amount of post-filtering
/// recovers the submission it displaced.
#[must_use]
pub fn select_article_masked(arena: &Arena, masked_prose: &[u32]) -> Selection {
    let (cands, stats) = collect_masked(arena, masked_prose);

    if cands.is_empty() {
        // Zero candidates means no container held any prose at all -- an empty page, an error
        // page, or one whose content is assembled by script after load. That is NoTextContent.
        //
        // It is emphatically NOT IndexPage: a listing page has many link-dense candidates, so it
        // reaches the purity floor below. An earlier version reported IndexPage here whenever the
        // page held any prose whatsoever, which labelled a 404 body ("404 Not Found", 14 bytes)
        // as a listing page. The two reasons lead a caller to do different things, so conflating
        // them is worse than reporting neither.
        return Selection {
            article: None,
            no_article: Some(NoArticle::NoTextContent),
            confidence: 0,
            dispersion_floor_used: false,
            region_is_outermost_by_argmax: false,
        };
    }

    // Rank by integer key so ties break by document order rather than by sort internals.
    let mut ranked: Vec<(_, usize)> = cands
        .iter()
        .enumerate()
        .map(|(idx, c)| (rank_key(normalize(c.evidence), c.node.0), idx))
        .collect();
    ranked.sort_unstable();

    let Some(&(_, best_idx)) = ranked.first() else {
        return Selection {
            article: None,
            no_article: Some(NoArticle::NoTextContent),
            confidence: 0,
            dispersion_floor_used: false,
            region_is_outermost_by_argmax: false,
        };
    };
    let Some(&best) = cands.get(best_idx) else {
        return Selection {
            article: None,
            no_article: Some(NoArticle::NoTextContent),
            confidence: 0,
            dispersion_floor_used: false,
            region_is_outermost_by_argmax: false,
        };
    };

    // Margin over the runner-up, as a ratio. A ratio rather than a difference so it does not
    // inherit the scale of the evidence values.
    let runner = ranked
        .get(1)
        .and_then(|&(_, i)| cands.get(i))
        .map_or(0.0, |c| c.evidence);
    let margin = 1.0 - guarded_div(runner, best.evidence);

    let degenerate = stats.competitors < MIN_COMPETITORS || stats.density_iqr <= 0.0;

    // Confidence from relative quantities only. `low_evidence` is deliberately excluded from the
    // accept branch: it may move confidence but must never flip Ok into NoArticle, or an absolute
    // size test would have crept back in through the side door.
    let mut conf = 0.25 * best.text_share.clamp(0.0, 1.0)
        + 0.30 * margin.clamp(0.0, 1.0)
        + 0.25 * best.purity.clamp(0.0, 1.0)
        + 0.20 * (1.0 - best.link_density).clamp(0.0, 1.0);
    if degenerate {
        // Cap rather than penalize: with too few competitors we do not know that we are wrong,
        // only that we cannot justify being sure.
        conf = conf.min(0.6);
    }

    // Purity floor is the one hard reject, and it is still relative: a region that is mostly
    // control labels and hidden text is not an article regardless of size.
    if best.purity < 0.5 || best.link_density > 0.75 {
        return Selection {
            article: None,
            no_article: Some(NoArticle::IndexPage),
            confidence: quantize(conf),
            dispersion_floor_used: degenerate,
            region_is_outermost_by_argmax: false,
        };
    }

    let outermost = cands
        .iter()
        .all(|c| c.node == best.node || !contains(arena, c.node, best.node));

    Selection {
        article: Some(best.node),
        no_article: None,
        confidence: quantize(conf),
        dispersion_floor_used: degenerate,
        region_is_outermost_by_argmax: outermost,
    }
}

/// Map unbounded evidence into `[0, 1)` monotonically, for ranking only.
///
/// `x / (1 + x)` — no transcendental op, strictly increasing, so it cannot change an ordering.
fn normalize(evidence: f32) -> f32 {
    if !evidence.is_finite() || evidence <= 0.0 {
        return 0.0;
    }
    guarded_div(evidence, 1.0 + evidence)
}

/// Whether `outer`'s subtree contains `inner`. O(1) thanks to `subtree_end`.
fn contains(arena: &Arena, outer: NodeId, inner: NodeId) -> bool {
    let end = arena.subtree_end.get(outer.idx()).copied().unwrap_or(0) as usize;
    inner.idx() > outer.idx() && inner.idx() < end
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::assertions_on_constants)]
mod tests {
    use super::*;

    #[test]
    fn bucket_indices_are_monotone_in_density() {
        let mut last = 0usize;
        let mut d = 0.5f32;
        while d < 30000.0 {
            let b = bucket_of(d);
            assert!(b >= last, "bucket_of not monotone at {d}: {b} < {last}");
            last = b;
            d *= 1.3;
        }
        assert_eq!(bucket_of(0.0), 0);
        assert_eq!(bucket_of(-1.0), 0);
        assert_eq!(bucket_of(f32::NAN), 0);
    }

    #[test]
    fn normalize_is_strictly_increasing_so_it_cannot_reorder() {
        let mut prev = -1.0f32;
        let mut x = 0.0f32;
        while x < 1000.0 {
            let y = normalize(x);
            assert!(y > prev || (x == 0.0 && y == 0.0), "not increasing at {x}");
            assert!((0.0..1.0).contains(&y));
            prev = y;
            x = x * 1.5 + 0.01;
        }
    }

    #[test]
    fn no_absolute_character_threshold_exists_in_this_module() {
        // Structural guard for defect 1. Readability's charThreshold is 500 and its length term
        // saturates at 300; a constant of that shape appearing here would mean the absolute test
        // came back. Checked as source text because the property is about what the code may say.
        // Comments are stripped first. An earlier version grepped the raw source and flagged
        // the doc comment that *explains* Readability's 500-character threshold -- describing a
        // constant is not using one, and a test that cannot tell the difference would push the
        // explanation out of the file rather than the constant.
        let src = include_str!("score.rs");
        let body = src.split("mod tests").next().unwrap_or("");
        let code: alloc::string::String = body
            .lines()
            .filter(|l| {
                let t = l.trim_start();
                !(t.starts_with("//") || t.starts_with("///") || t.starts_with("//!"))
            })
            .collect::<Vec<_>>()
            .join("\n");
        for forbidden in ["500", "300", "250", "char_threshold", "charThreshold"] {
            assert!(
                !code.contains(forbidden),
                "found `{forbidden}` in code -- an absolute length test has crept in"
            );
        }
    }

    #[test]
    fn percentile_of_empty_histogram_is_zero_not_nan() {
        let hist = [0u32; 256];
        assert!((percentile(&hist, 0, 50) - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn min_competitors_guards_a_meaningless_distribution() {
        // Print views and AMP pages routinely have one or two containers. The floor exists so a
        // z-score is never computed from a sample with no shape.
        assert!(MIN_COMPETITORS >= 3);
    }
}
