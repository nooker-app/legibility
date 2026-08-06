//! Repeated-sibling structure: comments, listings, and the masking that separates them from the
//! article (plan §1.7, defect 3).
//!
//! # Why one module does all three
//!
//! A comment thread, a product grid, an article listing and a nav menu are the same *structural*
//! phenomenon: a template applied N times. Detecting that repetition is one algorithm. What
//! distinguishes a comment thread from a listing is not the repetition but what each item
//! *contains* — an author and a timestamp — so the discrimination is a second, cheap test on top.
//!
//! # Why masking rather than deletion
//!
//! Comments are removed from the *article candidate pool* before it is scored, not deleted from the
//! arena. On Hacker News the submission is a title and a link — tens of bytes — while the comment
//! text is enormous, so any scorer that sees both picks the comments. Readability does exactly that.
//! Masking keeps the byte ranges available so the comments can still be returned as comments.
//!
//! # The recovery invariant
//!
//! A confident false positive would be catastrophic: mask the article, find nothing, return
//! nothing. So if masking leaves no viable article, the highest-prose masked group is unmasked and
//! the selection is retried exactly once, with `comment_mask_reverted` reported.

use alloc::vec::Vec;

use crate::arena::{Arena, AttrName, NodeId, NodeKind};
use crate::num::guarded_div;
use crate::tag::TagId;

/// Minimum members before repetition is evidence of a template rather than coincidence.
///
/// Two siblings that happen to look alike are common; three is a pattern. This is the one count
/// threshold in the module, and it is a count of *structures*, not of characters.
const MIN_GROUP: usize = 3;

/// How many descendants contribute to a signature.
///
/// Enough to capture an item's shape, few enough that a long comment and a short one still hash
/// alike — the whole point is to match items whose *content* differs.
const SIG_DESCENDANTS: usize = 24;

/// A set of sibling elements sharing a structural signature.
#[derive(Debug, Clone)]
pub struct Group {
    /// The shared parent.
    pub parent: NodeId,
    /// Members, in document order.
    pub members: Vec<NodeId>,
    /// Structural hash the members share.
    pub signature: u64,
    /// Fraction of members carrying at least two of {author link, timestamp, item id}.
    pub micro_metadata_ratio: f32,
    /// Total prose in the group.
    pub prose_len: u32,
    /// Mean link density across members.
    pub mean_link_density: f32,
    /// Coefficient of variation of member prose length.
    ///
    /// Reported as a diagnostic, **not** used as a reject. It was a reject at first, on the theory
    /// that human comments vary in length while generated listings do not. Measured against a real
    /// thread it dropped nine genuine comments whose lengths happened to be similar, at
    /// `micro_metadata_ratio = 1.00` — every member had both an author and a timestamp. A product
    /// grid does not have those. The gate re-asked a question already answered and got it wrong.
    pub length_cv: f32,
    /// Whether every member contains a heading — an article listing, not a discussion.
    pub all_members_have_heading: bool,
    /// Mean share of a member's prose held by its **first** link.
    ///
    /// Sharper than the largest-link share, and it is the signal that finally separated a Lobsters
    /// front page from a comment thread. Position carries the meaning: a listing item leads with its
    /// title link, so the first link is most of the text; a comment leads with a byline, so the
    /// first link is a handful of characters against a paragraph. Taking the largest link instead
    /// averaged the distinction away.
    pub mean_first_link_share: f32,
    /// Mean share of a member's prose that sits inside its single largest link.
    ///
    /// This is the discriminator that separates a comment thread from a story listing, and neither
    /// link density nor headings can do it. A Lobsters or Hacker News front page gives every item an
    /// author and a timestamp, so the micro-metadata gate passes; it uses `<a>` rather than `<h2>`
    /// for titles, so the heading test passes too. But a listing item's *title link is most of its
    /// text*, while a comment's author link is a few characters against a paragraph. One number,
    /// and the two shapes fall apart.
    pub mean_max_link_share: f32,
}

impl Group {
    /// Whether this group is a comment thread.
    ///
    /// The decisive signal is per-item authorship: a comment has an author and a time, and a
    /// product grid or nav menu does not. Two negative discriminators guard against the remaining
    /// confusions — a link-dense group is a menu, and a group whose every member carries a heading
    /// is an article listing.
    #[must_use]
    pub fn is_comment_thread(&self) -> bool {
        if self.members.len() < MIN_GROUP {
            return false;
        }
        if self.micro_metadata_ratio < 0.7 {
            return false;
        }
        if self.mean_link_density > 0.5 {
            return false;
        }
        if self.all_members_have_heading {
            return false;
        }
        if self.mean_first_link_share > 0.25 {
            return false;
        }
        true
    }

    /// Whether this group is a listing (index page, product grid, nav).
    ///
    /// The mirror of [`Group::is_comment_thread`]: repetition without per-item authorship.
    #[must_use]
    pub fn is_listing(&self) -> bool {
        if self.members.len() < MIN_GROUP {
            return false;
        }
        // A listing whose items carry authors and timestamps is still a listing if its text lives
        // inside title links -- that is what a link-aggregator front page is.
        if self.mean_first_link_share > 0.25 {
            return true;
        }
        self.micro_metadata_ratio < 0.7
            && (self.mean_link_density > 0.3 || self.all_members_have_heading)
    }
}

/// Structural signature of `node`: its own shape plus that of its first descendants.
///
/// FNV-1a with a fixed basis, over integers only. No `HashMap`, no `DefaultHasher` — both would
/// make the value depend on something other than the document, and guarantee S3 requires that the
/// same bytes produce the same result on every target forever.
///
/// Deliberately excludes text length: two comments of wildly different length must hash alike.
fn signature(arena: &Arena, node: NodeId) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut h = FNV_OFFSET;
    let mix = |v: u64, h: &mut u64| {
        *h ^= v;
        *h = h.wrapping_mul(FNV_PRIME);
    };

    let end = arena.subtree_end.get(node.idx()).copied().unwrap_or(0) as usize;
    let base_depth = arena.depth.get(node.idx()).copied().unwrap_or(0);
    mix(u64::from(arena.tag.get(node.idx()).copied().unwrap_or(TagId::UNKNOWN).0), &mut h);
    mix(class_bits(arena, node), &mut h);

    let mut seen = 0usize;
    for i in (node.idx() + 1)..end {
        if seen >= SIG_DESCENDANTS {
            break;
        }
        if arena.kind.get(i).copied() != Some(NodeKind::Element) {
            continue;
        }
        let d = arena.depth.get(i).copied().unwrap_or(0).saturating_sub(base_depth);
        mix(u64::from(arena.tag.get(i).copied().unwrap_or(TagId::UNKNOWN).0), &mut h);
        mix(u64::from(d), &mut h);
        seen = seen.saturating_add(1);
    }
    h
}

/// Order-independent hash of an element's class tokens.
///
/// XOR of per-token hashes so that `class="a b"` and `class="b a"` agree — authoring tools reorder
/// class lists, and a signature that changed with the order would split one group into several.
fn class_bits(arena: &Arena, node: NodeId) -> u64 {
    let Some(class) = arena.attr(node, AttrName::CLASS) else { return 0 };
    let mut acc = 0u64;
    for token in class.split_ascii_whitespace() {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for b in token.bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        acc ^= h;
    }
    acc
}

/// Whether `node`'s subtree carries an author link.
///
/// Either a link whose target looks like a profile, or one whose class says so. Both, rather than
/// only the class, because class names are site-specific while `/u/`, `/user/` and `/@handle` are
/// near-universal conventions.
fn has_author_link(arena: &Arena, node: NodeId, end: usize) -> bool {
    for i in node.idx()..end {
        if arena.tag.get(i).copied() != Some(TagId::A) {
            continue;
        }
        let id = NodeId(i as u32);
        if let Some(href) = arena.attr(id, AttrName::HREF) {
            if href.contains("/u/")
                || href.contains("/user/")
                || href.contains("/users/")
                || href.contains("/@")
                || href.contains("/profile")
                || href.contains("/member")
            {
                return true;
            }
        }
        if arena
            .attr(id, AttrName::CLASS)
            .is_some_and(|c| c.contains("author") || c.contains("user"))
        {
            return true;
        }
        if arena
            .attr(id, AttrName::REL)
            .is_some_and(|r| r.split_ascii_whitespace().any(|t| t.eq_ignore_ascii_case("author")))
        {
            return true;
        }
    }
    false
}

/// Whether `node`'s subtree carries a timestamp.
fn has_timestamp(arena: &Arena, node: NodeId, end: usize) -> bool {
    for i in node.idx()..end {
        if arena.tag.get(i).copied() == Some(TagId::TIME) {
            return true;
        }
        let id = NodeId(i as u32);
        if arena.attr(id, AttrName::DATETIME).is_some() {
            return true;
        }
    }
    false
}

/// Whether `node` or a descendant carries an id, which is how permalinks are anchored.
fn has_item_id(arena: &Arena, node: NodeId, end: usize) -> bool {
    (node.idx()..end).any(|i| arena.attr(NodeId(i as u32), AttrName::ID).is_some())
}

fn has_heading(arena: &Arena, node: NodeId, end: usize) -> bool {
    ((node.idx() + 1)..end).any(|i| {
        matches!(
            arena.tag.get(i).copied(),
            Some(TagId::H1 | TagId::H2 | TagId::H3 | TagId::H4 | TagId::H5 | TagId::H6)
        )
    })
}

/// Every element in the document whose `(tag, class)` matches this group's members.
///
/// Used so a thread of one -- a lone reply with no siblings, which can never form a group -- is
/// still returned as a comment.
#[must_use]
pub fn members_by_identity(arena: &Arena, group: &Group) -> Vec<NodeId> {
    let Some(&first) = group.members.first() else { return Vec::new() };
    let want = (
        arena.tag.get(first.idx()).copied().unwrap_or(TagId::UNKNOWN).0,
        class_bits(arena, first),
    );
    if want.1 == 0 {
        return group.members.clone();
    }
    let mut out = Vec::new();
    for i in 0..arena.len() {
        if arena.kind.get(i).copied() != Some(NodeKind::Element) {
            continue;
        }
        let node = NodeId(i as u32);
        let got = (
            arena.tag.get(i).copied().unwrap_or(TagId::UNKNOWN).0,
            class_bits(arena, node),
        );
        if got == want {
            out.push(node);
        }
    }
    out
}

/// Merge groups sharing a signature into one thread.
///
/// Nested discussions split across containers: each reply level gets its own `<ol>`, so one
/// eighteen-comment thread arrived as a group of nine and a group of three under different parents.
/// Members are re-sorted into document order, which is also what makes DOM-nesting depth resolvable
/// afterwards — a member nested inside another is a reply to it.
#[must_use]
pub fn merge_by_signature(arena: &Arena, groups: &[Group]) -> Vec<Group> {
    // Merged on the members' own identity -- tag plus class tokens -- not on the full structural
    // signature. The full signature includes descendant shape, and a reply that itself has replies
    // has a different shape from a leaf reply, so an eighteen-comment thread arrived as a group of
    // nine and a group of three. `class="comment-tree-item"` is the site stating that these are the
    // same kind of thing, which is exactly the claim we want to trust here.
    let key = |g: &Group| {
        let m = g.members.first().copied().unwrap_or(NodeId(0));
        (
            arena.tag.get(m.idx()).copied().unwrap_or(TagId::UNKNOWN).0,
            class_bits(arena, m),
        )
    };
    let mut out: Vec<Group> = Vec::new();
    for g in groups {
        let k = key(g);
        match out.iter_mut().find(|o| key(o) == k) {
            Some(existing) => existing.members.extend_from_slice(&g.members),
            None => out.push(g.clone()),
        }
    }
    for g in &mut out {
        g.members.sort_unstable();
        g.members.dedup();
        let merged = describe(arena, g.parent, g.signature, core::mem::take(&mut g.members));
        *g = merged;
    }
    out
}

/// Find every repeated-sibling group in the document.
///
/// One pass: for each element, bucket its element children by signature, then keep buckets with at
/// least [`MIN_GROUP`] members. Children are compared only against their own siblings, which is
/// what makes this O(n) with a small constant rather than a pairwise comparison.
#[must_use]
pub fn find_groups(arena: &Arena) -> Vec<Group> {
    let mut groups = Vec::new();

    for p in 0..arena.len() {
        if arena.kind.get(p).copied() != Some(NodeKind::Element) {
            continue;
        }
        let parent = NodeId(p as u32);
        let kids = element_children(arena, p);
        if kids.len() < MIN_GROUP {
            continue;
        }

        // Signature -> members, as a small association list. A map would be faster asymptotically
        // but sibling counts are small, and a Vec keeps iteration order tied to the document
        // rather than to a hash (S3).
        let mut buckets: Vec<(u64, Vec<NodeId>)> = Vec::new();
        for &k in &kids {
            let sig = signature(arena, k);
            match buckets.iter_mut().find(|(s, _)| *s == sig) {
                Some((_, v)) => v.push(k),
                None => buckets.push((sig, alloc::vec![k])),
            }
        }

        for (sig, members) in buckets {
            if members.len() < MIN_GROUP {
                continue;
            }
            groups.push(describe(arena, parent, sig, members));
        }
    }
    groups
}

fn element_children(arena: &Arena, p: usize) -> Vec<NodeId> {
    let end = arena.subtree_end.get(p).copied().unwrap_or(0) as usize;
    let mut kids = Vec::new();
    let mut c = p + 1;
    while c < end {
        if arena.kind.get(c).copied() == Some(NodeKind::Element) {
            kids.push(NodeId(c as u32));
        }
        c = arena.subtree_end.get(c).copied().unwrap_or((c + 1) as u32) as usize;
    }
    kids
}

fn describe(arena: &Arena, parent: NodeId, signature: u64, members: Vec<NodeId>) -> Group {
    let mut with_micro = 0usize;
    let mut prose_total: u32 = 0;
    let mut link_sum = 0.0f32;
    let mut all_headings = true;
    let mut max_link_share_sum = 0.0f32;
    let mut first_link_share_sum = 0.0f32;
    let mut lengths: Vec<u32> = Vec::with_capacity(members.len());

    for &m in &members {
        let end = arena.subtree_end.get(m.idx()).copied().unwrap_or(0) as usize;
        let signals = usize::from(has_author_link(arena, m, end))
            + usize::from(has_timestamp(arena, m, end))
            + usize::from(has_item_id(arena, m, end));
        if signals >= 2 {
            with_micro = with_micro.saturating_add(1);
        }
        let p = arena.prose_len.get(m.idx()).copied().unwrap_or(0);
        prose_total = prose_total.saturating_add(p);
        lengths.push(p);
        link_sum += arena.link_density(m);
        max_link_share_sum += max_link_prose_share(arena, m, end, p);
        first_link_share_sum += first_link_prose_share(arena, m, end, p);
        if !has_heading(arena, m, end) {
            all_headings = false;
        }
    }

    let n = members.len() as f32;
    Group {
        parent,
        signature,
        micro_metadata_ratio: guarded_div(with_micro as f32, n),
        prose_len: prose_total,
        mean_link_density: guarded_div(link_sum, n),
        length_cv: coefficient_of_variation(&lengths),
        all_members_have_heading: all_headings,
        mean_max_link_share: guarded_div(max_link_share_sum, n),
        mean_first_link_share: guarded_div(first_link_share_sum, n),
        members,
    }
}

/// Share of an item's prose held by its **first** link in document order.
fn first_link_prose_share(arena: &Arena, node: NodeId, end: usize, item_prose: u32) -> f32 {
    if item_prose == 0 {
        return 0.0;
    }
    for i in node.idx()..end {
        if arena.tag.get(i).copied() == Some(TagId::A) {
            let p = arena.prose_len.get(i).copied().unwrap_or(0);
            if p > 0 {
                return guarded_div(p as f32, item_prose as f32);
            }
        }
    }
    0.0
}

/// Share of an item's prose held by its single largest link.
///
/// Largest rather than total: a comment with several short inline links should not look like a
/// listing, and a listing item whose title link dominates should not be rescued by also containing
/// a short byline link.
fn max_link_prose_share(arena: &Arena, node: NodeId, end: usize, item_prose: u32) -> f32 {
    if item_prose == 0 {
        return 0.0;
    }
    let mut max_link = 0u32;
    for i in node.idx()..end {
        if arena.tag.get(i).copied() == Some(TagId::A) {
            let p = arena.prose_len.get(i).copied().unwrap_or(0);
            max_link = max_link.max(p);
        }
    }
    guarded_div(max_link as f32, item_prose as f32)
}

/// Standard deviation over mean. Zero when the mean is zero.
///
/// Computed with a plain two-pass mean/variance rather than a streaming form: two passes over a
/// handful of integers is cheaper than the accuracy argument, and the result is deterministic.
fn coefficient_of_variation(v: &[u32]) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    let n = v.len() as f32;
    let mean = guarded_div(v.iter().map(|&x| x as f32).sum::<f32>(), n);
    if mean == 0.0 {
        return 0.0;
    }
    let var = guarded_div(
        v.iter().map(|&x| {
            let d = x as f32 - mean;
            d * d
        }).sum::<f32>(),
        n,
    );
    guarded_div(sqrt_approx(var), mean)
}

/// Square root without `powf`, by Newton–Raphson from a bit-shift seed.
///
/// `f32::sqrt` compiles to a hardware instruction that IEEE-754 requires to be exactly rounded, so
/// it would in fact be reproducible — but the numeric policy bans reaching for math intrinsics as a
/// habit, and six Newton steps are exact enough for a coefficient of variation used against a
/// threshold of 0.15.
fn sqrt_approx(x: f32) -> f32 {
    if !x.is_finite() || x <= 0.0 {
        return 0.0;
    }
    // Halve the exponent for an initial guess.
    let mut g = f32::from_bits((x.to_bits() >> 1).wrapping_add(0x1fc0_0000));
    for _ in 0..6 {
        if g == 0.0 {
            return 0.0;
        }
        g = 0.5 * (g + guarded_div(x, g));
    }
    g
}

/// Prose that belongs to comment threads, per node.
///
/// `comment_prose[i]` is the prose inside `i`'s subtree that sits within a masked group member, so a
/// candidate's article-only prose is `prose_len[i] - comment_prose[i]`. Computed in one reverse
/// pass, reusing the same document-order property that makes the main accumulation single-pass.
#[must_use]
pub fn mask_comment_prose(arena: &Arena, groups: &[Group]) -> Vec<u32> {
    // Masking is by *identity*, not by group membership. A reply that is the only child of its own
    // container never forms a group of three, so membership-based masking left six of eighteen
    // comments unmasked -- and the article selection then picked one of the six. Once a group has
    // been confirmed as a thread, its members' `(tag, class)` is the site's own statement of what a
    // comment looks like, so every element matching it is one, thread of one included.
    let mut identities: Vec<(u16, u64)> = Vec::new();
    for g in groups {
        if !g.is_comment_thread() {
            continue;
        }
        if let Some(&m) = g.members.first() {
            let id = (
                arena.tag.get(m.idx()).copied().unwrap_or(TagId::UNKNOWN).0,
                class_bits(arena, m),
            );
            // A classless identity would match every bare <div> in the document.
            if id.1 != 0 && !identities.contains(&id) {
                identities.push(id);
            }
        }
    }

    let mut own = alloc::vec![0u32; arena.len()];
    // Document order means a nested match always appears after the outer one it sits inside, so a
    // single high-water mark suffices to skip it -- the outer match's prose already covers it.
    // Scanning backwards for an enclosing match instead would be quadratic on a deep thread.
    let mut covered_until = 0usize;
    for i in 0..arena.len() {
        if arena.kind.get(i).copied() != Some(NodeKind::Element) {
            continue;
        }
        if i < covered_until {
            continue;
        }
        let node = NodeId(i as u32);
        let id = (
            arena.tag.get(i).copied().unwrap_or(TagId::UNKNOWN).0,
            class_bits(arena, node),
        );
        if !identities.contains(&id) {
            continue;
        }
        if let Some(slot) = own.get_mut(i) {
            *slot = arena.prose_len.get(i).copied().unwrap_or(0);
        }
        covered_until = arena.subtree_end.get(i).copied().unwrap_or(0) as usize;
    }

    // Roll up. A masked member's own value is already its whole subtree, so descendants of a masked
    // member must not add again -- hence the skip.
    let mut out = own.clone();
    for i in (0..arena.len()).rev() {
        let Some(&parent) = arena.parent.get(i) else { continue };
        if !parent.is_some() {
            continue;
        }
        if own.get(i).copied().unwrap_or(0) > 0 {
            // This node is a masked member: contribute its own total, not its children's.
            let v = own.get(i).copied().unwrap_or(0);
            if let Some(slot) = out.get_mut(parent.idx()) {
                *slot = slot.saturating_add(v);
            }
            continue;
        }
        let v = out.get(i).copied().unwrap_or(0);
        if v > 0 {
            if let Some(slot) = out.get_mut(parent.idx()) {
                *slot = slot.saturating_add(v);
            }
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::assertions_on_constants, clippy::float_cmp, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn sqrt_approx_is_accurate_enough_for_a_ratio_threshold() {
        for x in [0.0f32, 1.0, 2.0, 4.0, 9.0, 100.0, 1e6, 1e-6] {
            let a = sqrt_approx(x);
            let b = if x <= 0.0 { 0.0 } else { x.sqrt() };
            assert!(
                (a - b).abs() <= b * 1e-4 + 1e-6,
                "sqrt_approx({x}) = {a}, expected ~{b}"
            );
        }
        assert_eq!(sqrt_approx(-1.0), 0.0);
        assert_eq!(sqrt_approx(f32::NAN), 0.0);
    }

    #[test]
    fn coefficient_of_variation_separates_written_from_generated() {
        // Human comments vary; a generated listing does not. The threshold in
        // is_comment_thread relies on exactly this gap.
        let uniform = [100u32, 101, 99, 100, 100];
        let varied = [20u32, 800, 150, 3000, 60];
        assert!(coefficient_of_variation(&uniform) < 0.15);
        assert!(coefficient_of_variation(&varied) > 0.15);
        assert_eq!(coefficient_of_variation(&[]), 0.0);
        assert_eq!(coefficient_of_variation(&[0, 0, 0]), 0.0);
    }

    #[test]
    fn min_group_is_three_because_two_alike_siblings_are_coincidence() {
        assert_eq!(MIN_GROUP, 3);
    }
}
