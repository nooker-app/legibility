//! Does a discussion's submission carry a body? — plan §1.7, the S-1/S-2 axis.
//!
//! # The defect this closes
//!
//! A discussion page has an invariant three-part shape:
//!
//! ```text
//!   [submission header]   title, byline, score, permalink — often an outbound link
//!   [submission body]     zero or one prose block
//!   [comment section]     zero or more replies
//! ```
//!
//! The scorer picks the container that maximises evidence. When the body is absent, or is
//! *nothing but a link*, that container is the **header region**, and we hand back a
//! pseudo-article made of the credit bar, the title and a bare URL — every byte of which is
//! already returned as a metadata field. Two real Reddit posts, same site, opposite shapes:
//!
//! | post | body | what we returned before |
//! |---|---|---|
//! | `1vezc5f` | one `<p>` holding one `<a>` | `"r/rss · 2d ago rangeva [Github] Free news APIs… [ https://… ] 0"` |
//! | `1v5n91g` | three `<p>`, real prose | the prose, correctly |
//!
//! The first is not a smaller article. It is a *different kind of thing*: a pointer. The fix
//! is to say so — [`DiscussionShape::LinkOnly`] yields a `DiscussionRoot` whose payload is
//! the title (already in metadata) plus the outbound URL, and an empty body.
//!
//! # Why the rule is structural rather than a length test
//!
//! "Short body ⇒ pointer" would be an absolute character threshold, which is defect ① wearing
//! a different hat. The signal that actually separates the two posts is not size — it is that
//! the link-only body's prose *is* its anchor text. So the test is
//! [`crate::score::Candidate::is_viable`], the same purity and link-density floor the scorer
//! already applies to candidacy. One definition, two uses.
//!
//! # Why the byline does not win
//!
//! The first attempt walked the title's following siblings, on the theory that the header's
//! furniture precedes the title and the body follows it. Two things broke it. The credit bar
//! (`r/rss` + author link + a timestamp) is *itself* a viable prose block, so any scan that
//! reaches it can call the byline a body — and when the title is wrapped in a `<header>`, as
//! it is on `GeekNews`, the "following siblings" are the byline and nothing else.
//!
//! What separates a body from a byline without a length constant is a comparison the page
//! supplies itself: **a body says more than its own headline.** A credit bar, a timestamp
//! line, a score, a caption — all of them are shorter than the title they annotate. So the
//! rule is a search over the whole region for the widest viable prose block that neither
//! contains nor sits inside the title, and the accept test is that it out-masses the title.
//! Both sides are measured on the same page, so duplication and truncation move them
//! together (plan §M6 scale invariance).
//!
//! This decision runs **after** comment masking, for the reason plan §1.7 gives: before
//! masking, the first reply is a candidate body and every link submission with replies reads
//! as S-2.

use crate::arena::{Arena, AttrName};
use crate::num::guarded_div;
use crate::score::Candidate;
use crate::{NodeId, NodeKind, TagId};

/// Which of the two submission shapes a discussion page has.
///
/// Only the axis that changes the *output shape* is modelled here. The rest of plan §1.7's
/// taxonomy (S-3…S-6) is about how depth is encoded, which is [`crate::comments`]' problem
/// and does not change what `article` means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscussionShape {
    /// S-1: the submission is a pointer. Title plus outbound URL is the entire payload.
    LinkOnly,
    /// S-2: the submission carries prose. That prose is the article.
    WithBody,
}

impl DiscussionShape {
    /// Stable name for JSON and diagnostics.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::LinkOnly => "link-only",
            Self::WithBody => "with-body",
        }
    }
}

/// The decision, with the nodes it was derived from so a caller can act on it.
#[derive(Debug, Clone, Copy)]
pub struct Shape {
    /// Which shape.
    pub kind: DiscussionShape,
    /// The submission heading the walk anchored on.
    pub title: NodeId,
    /// The widest prose block found outside the title, and the reason the shape came out as it
    /// did. Reported, **not** used to re-pick the region — see the note in [`decide`] on why
    /// narrowing to it is a separate problem.
    pub body: Option<NodeId>,
    /// For [`DiscussionShape::LinkOnly`]: the outbound anchor, if the submission has one.
    pub link: Option<NodeId>,
}

/// Decide the shape of a discussion page's submission.
///
/// `exclusions` is [`crate::Outcome::article_exclusions`] — comments and the section furniture
/// around them. Returns `None` when there is nothing to decide: the page is not a discussion,
/// or the region carries no heading to anchor on. `None` leaves the existing selection alone,
/// which is the right default for every non-discussion page on the web.
///
/// # What this deliberately does not do
///
/// It does not re-pick the region for [`DiscussionShape::WithBody`]. Narrowing to `body` looks
/// free — it would drop the credit bar and the duplicated title that currently ride in front of
/// every discussion body — and it was implemented and reverted, because "widest viable prose
/// block" is a cruder rule than the scorer's and overrides it on real pages:
///
/// | page | narrowed to | F1 |
/// |---|---|---|
/// | ehow-2 | one paragraph — the `<h1>` is *inside* the content div, so that div is skipped as a title wrapper | 0.979 → 0.286 |
/// | cnet | ditto | 0.927 → 0.000 |
/// | breitbart | the `<aside>` ad rail | 0.332 → 0.113 |
/// | wordpress | the comment list, which `comment_section_nodes` had missed | 0.430 → 0.298 |
///
/// A majority guard rescued the first two families and left the last two, so the approach was
/// wrong rather than mistuned: the byline prefix is plan D4's `lead { byline_span, heading_span }`
/// and needs byline detection to be safe. Removing prose from a region is not a thing to do with
/// a length comparison.
#[must_use]
pub fn decide(
    arena: &Arena,
    region: NodeId,
    exclusions: &[NodeId],
    is_discussion: bool,
) -> Option<Shape> {
    if !is_discussion {
        return None;
    }
    let region_end = (arena.subtree_end.get(region.idx()).copied().unwrap_or(0) as usize)
        .min(arena.len());
    let title = submission_heading(arena, region, region_end)?;
    let title_prose = arena.prose_len.get(title.idx()).copied().unwrap_or(0);
    let title_end = arena.subtree_end.get(title.idx()).copied().unwrap_or(0) as usize;

    let mut body: Option<NodeId> = None;
    let mut body_prose = 0u32;
    let mut link: Option<NodeId> = None;
    let mut link_prose = 0u32;
    let mut i = region.idx() + 1;
    while i < region_end {
        let node = NodeId(i as u32);
        // Skip whole comment subtrees. Without this a reply's inner markdown block is a
        // perfectly good "body" and the widest one wins -- which is defect ③ arriving by a
        // side door after masking already shut the front one.
        if exclusions.iter().any(|n| n.idx() == i) {
            i = (arena.subtree_end.get(i).copied().unwrap_or(0) as usize).max(i + 1);
            continue;
        }
        if arena.kind.get(i).copied() != Some(NodeKind::Element) {
            i += 1;
            continue;
        }
        // The outbound link is the widest anchor anywhere in the submission. Including the
        // title's own anchors is deliberate: Hacker News puts the destination *on* the title,
        // and Reddit puts it in the body, and "widest" picks the right one in both.
        if arena.tag.get(i).copied() == Some(TagId::A)
            && arena.attr(node, AttrName::HREF).is_some()
        {
            let n = arena.prose_len.get(i).copied().unwrap_or(0);
            if n > link_prose {
                link_prose = n;
                link = Some(node);
            }
        }
        // Neither the title nor anything wrapping it. Excluding wrappers is what keeps the
        // submission container -- which holds title *and* body -- from being its own body.
        let inside_title = i >= title.idx() && i < title_end;
        let wraps_title = i < title.idx()
            && (arena.subtree_end.get(i).copied().unwrap_or(0) as usize) > title.idx();
        if !inside_title && !wraps_title && !is_heading(arena, node) {
            let prose = arena.prose_len.get(i).copied().unwrap_or(0);
            if prose > body_prose && is_prose_block(arena, node) {
                body_prose = prose;
                body = Some(node);
            }
        }
        i += 1;
    }

    // A body says more than its own headline. Everything that fails this is furniture: the
    // credit bar, the score, a timestamp line, a caption -- and on a link submission, the
    // pasted URL with its brackets.
    if body_prose > title_prose {
        Some(Shape {
            kind: DiscussionShape::WithBody,
            title,
            body,
            link,
        })
    } else {
        Some(Shape {
            kind: DiscussionShape::LinkOnly,
            title,
            body: None,
            link,
        })
    }
}

/// The submission's title: the first heading in the region that carries prose.
///
/// Level-agnostic on purpose. Reddit and Hacker News both use `h1`, but a discussion rendered
/// inside a wider page may demote it, and requiring `h1` would silently switch such a page to
/// the whole-region scan this module exists to avoid.
fn submission_heading(arena: &Arena, region: NodeId, region_end: usize) -> Option<NodeId> {
    for i in (region.idx() + 1)..region_end.min(arena.len()) {
        let node = NodeId(i as u32);
        if arena.kind.get(i).copied() == Some(NodeKind::Element)
            && is_heading(arena, node)
            && arena.prose_len.get(i).copied().unwrap_or(0) > 0
        {
            return Some(node);
        }
    }
    None
}

fn is_heading(arena: &Arena, node: NodeId) -> bool {
    matches!(
        arena.tag.get(node.idx()).copied(),
        Some(TagId::H1 | TagId::H2 | TagId::H3 | TagId::H4 | TagId::H5 | TagId::H6)
    )
}

/// Whether a node reads as prose rather than as a link or as furniture.
///
/// Deliberately the scorer's own candidacy floor rather than a second opinion: if a block
/// would not be allowed to compete for the article, it cannot be the submission body either.
fn is_prose_block(arena: &Arena, node: NodeId) -> bool {
    let i = node.idx();
    let prose = arena.prose_len.get(i).copied().unwrap_or(0);
    let control = arena.control_len.get(i).copied().unwrap_or(0);
    let hidden = arena.hidden_len.get(i).copied().unwrap_or(0);
    let alt = arena.alt_len.get(i).copied().unwrap_or(0);
    let all = prose
        .saturating_add(control)
        .saturating_add(hidden)
        .saturating_add(alt);
    let purity = guarded_div(prose as f32, all as f32);
    let link_density = arena.link_density(node).clamp(0.0, 1.0);
    purity >= Candidate::MIN_PURITY && link_density <= Candidate::MAX_LINK_DENSITY
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_non_discussion_page_is_never_reshaped() {
        // The blast radius. Every ordinary article on the web reaches this function, and the
        // only correct answer for all of them is "no opinion".
        let arena = Arena::default();
        assert!(decide(&arena, NodeId(0), &[], false).is_none());
    }
}
