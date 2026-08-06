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
    /// Plan D4's `lead.byline_span`: the dateline that precedes the headline, when there is one.
    ///
    /// Unlike [`Self::body`] this *is* acted on — the caller excludes it. See
    /// [`submission_byline`] for why removing this is safe where narrowing to `body` was not.
    pub byline: Option<NodeId>,
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
    let region_end =
        (arena.subtree_end.get(region.idx()).copied().unwrap_or(0) as usize).min(arena.len());
    let title = submission_heading(arena, region, region_end)?;
    let title_prose = arena.prose_len.get(title.idx()).copied().unwrap_or(0);
    let title_end = arena.subtree_end.get(title.idx()).copied().unwrap_or(0) as usize;
    // Look for the body inside the submission, not across the whole region. The region is
    // whatever the scorer accepted -- on a Reddit post that is `<main>`, which also holds the
    // comment furniture and the ad rail, and both of those are wide, pure, link-free prose that
    // out-masses a headline. Measured on the real page: the empty comment section ("Be the first
    // to comment. Nobody's responded to this post yet.") came to 190 bytes and the house ad to
    // 183, against a 108-byte title, so a link submission with no body at all reported two
    // separate bodies before the first one won.
    let container = submission_container(arena, region, region_end, title);
    let (start, end) = match container {
        Some(c) => (
            c.idx(),
            (arena.subtree_end.get(c.idx()).copied().unwrap_or(0) as usize).min(region_end),
        ),
        None => (region.idx() + 1, region_end),
    };

    // Direct children of whatever the search is scoped to. `start` is the container node itself
    // in the narrowed case, so its children begin one past it.
    let children_start = container.map_or(region.idx() + 1, |c| c.idx() + 1);

    let mut body: Option<NodeId> = None;
    let mut body_prose = 0u32;
    let mut link: Option<NodeId> = None;
    let mut link_prose = 0u32;
    let mut i = start;
    while i < end {
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
        if arena.tag.get(i).copied() == Some(TagId::A) && arena.attr(node, AttrName::HREF).is_some()
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
            let prose = net_prose(arena, i, exclusions);
            if prose > body_prose && is_prose_block(arena, node) {
                body_prose = prose;
                body = Some(node);
            }
        }
        i += 1;
    }

    // A body has to have *passed* to count as one. Reporting the widest candidate regardless was
    // wrong in a way that stayed hidden while `body` was only ever reported: on a page whose title
    // sits in its own `<header>`, the widest candidate inside that header is the meta line, and the
    // `WithBody` verdict is reached through `link.is_none()` rather than through the mass test. The
    // field then named the byline as the body. Harmless until the byline rule started excluding
    // whatever `body` names, at which point it hid the very node it was looking for.
    let qualified = (body_prose > title_prose).then_some(body).flatten();

    // Computed after the body is known, because the body is the one thing the byline must never be.
    let byline = submission_byline(arena, children_start, end, title_prose, qualified, exclusions);

    // A body says more than its own headline. Everything that fails this is furniture: the
    // credit bar, the score, a timestamp line, a caption -- and on a link submission, the
    // pasted URL with its brackets.
    //
    // `LinkOnly` additionally requires somewhere to point. Without that clause a page whose
    // heading sits alone in a header block reports a pointer with no destination, and the
    // serializer's answer to a pointer is an empty body -- so a misjudgement here does not
    // degrade the article, it deletes it. A pointer that points nowhere is not a pointer.
    if qualified.is_some() || link.is_none() {
        Some(Shape {
            kind: DiscussionShape::WithBody,
            title,
            body: qualified,
            link,
            byline,
        })
    } else {
        Some(Shape {
            kind: DiscussionShape::LinkOnly,
            title,
            body: None,
            link,
            byline,
        })
    }
}

/// The dateline sitting in front of the headline: plan D4's `lead.byline_span`.
///
/// On every discussion site the submission opens with a credit bar — subreddit or site link,
/// author, score, permalink, and a timestamp — and it rides in front of the body because it is
/// inside the same container. It is not prose; every one of its fields is already returned as a
/// metadata candidate with provenance.
///
/// # Which side of the headline
///
/// Either. Two sites, same construct, opposite order:
///
/// ```text
///   reddit    <div>r/rss · <time>12d ago</time> · author</div>   <h1>…</h1>   <div>body</div>
///   beebs     <div class=title-line><h1>…</h1></div>   <div>author · views · <time>…</time></div>
/// ```
///
/// The first version looked only before the headline and left the whole meta line on every beebs
/// thread. Position is not the signal; the `<time>` is.
///
/// # Why this is allowed to remove text when narrowing to `body` was not
///
/// The reverted approach (see [`decide`]) *chose a new region* by comparing prose mass, so it
/// could and did pick an ad rail or a comment list over the article. This removes a **single
/// direct child** identified by a structural fact instead of a size comparison: it carries a
/// `<time>`. Article prose does not.
///
/// Four guards keep it away from an article's text:
///
/// - it runs only when `is_discussion`, so the 130-page corpus cannot enter it at all;
/// - the qualified `body` is excluded, along with anything containing it — the block that beat the
///   headline is not the dateline, and that is what makes searching *after* the headline safe;
/// - anything holding a comment is excluded, so a reply — which also carries a `<time>` and no
///   heading — can never be mistaken for the dateline;
/// - a child holding a heading of its own is refused, which also skips the headline's own wrapper;
/// - a child out-massing the headline is refused, reusing the comparison the shape rule already
///   trusts. Something bigger than the title means this module has misread the page, and keeping
///   the text is the safe move.
fn submission_byline(
    arena: &Arena,
    children_start: usize,
    end: usize,
    title_prose: u32,
    body: Option<NodeId>,
    exclusions: &[NodeId],
) -> Option<NodeId> {
    let mut c = children_start;
    while c < end {
        let child_end = (arena.subtree_end.get(c).copied().unwrap_or(0) as usize).max(c + 1);
        let holds = |n: NodeId| n.idx() >= c && n.idx() < child_end;
        if arena.kind.get(c).copied() == Some(NodeKind::Element)
            && !body.is_some_and(holds)
            && !exclusions.iter().copied().any(holds)
            && arena.prose_len.get(c).copied().unwrap_or(0) <= title_prose
            && subtree_has(arena, c, child_end, TagId::TIME)
            && !subtree_has_heading(arena, c, child_end)
        {
            return Some(NodeId(c as u32));
        }
        c = child_end;
    }
    None
}

/// Whether `tag` appears anywhere in `[from, to)`.
fn subtree_has(arena: &Arena, from: usize, to: usize, tag: TagId) -> bool {
    (from..to.min(arena.len())).any(|i| arena.tag.get(i).copied() == Some(tag))
}

/// Whether any heading appears in `[from, to)`.
fn subtree_has_heading(arena: &Arena, from: usize, to: usize) -> bool {
    (from..to.min(arena.len())).any(|i| is_heading(arena, NodeId(i as u32)))
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

/// The part of `region` that holds the submission: the region's own child containing the title.
///
/// The region is chosen by the scorer to be generous — it is the block that best explains the
/// page — so on a discussion page it routinely contains the comment section, the composer, the
/// sort control and an ad slot alongside the post. Every one of those is prose a headline can
/// lose to, and the shape rule's whole question is "does this submission say more than its own
/// headline". Asking it of the region answers a different question.
///
/// Returns the whole region when the title is a direct child of it, which is the case where the
/// region already *is* the submission and there is nothing tighter to narrow to.
///
/// This narrows only the *search*, never the extracted region — see the note on [`decide`] for
/// why narrowing the output was tried and reverted.
fn submission_container(
    arena: &Arena,
    region: NodeId,
    region_end: usize,
    title: NodeId,
) -> Option<NodeId> {
    let mut c = region.idx() + 1;
    while c < region_end {
        let child_end = (arena.subtree_end.get(c).copied().unwrap_or(0) as usize).max(c + 1);
        if title.idx() >= c && title.idx() < child_end {
            // The title being the child itself means the submission is spread across the region's
            // children rather than gathered into one, so there is no container to narrow to.
            return (c != title.idx()).then_some(NodeId(c as u32));
        }
        c = child_end;
    }
    None
}

/// Prose of `node`'s subtree with the excluded subtrees inside it subtracted.
///
/// The scan skips excluded subtrees, but skipping them does not stop their bytes from being
/// counted in every ancestor's `prose_len` — an ancestor is visited before the descendant it
/// would have skipped. On the page that prompted this, the `<div>` wrapping the comment tree
/// scored 190 bytes of which all 190 were comment-section furniture, and it beat the title
/// on prose that the extraction had already agreed to throw away.
///
/// Nested exclusions are subtracted once: a comment item inside an excluded section would
/// otherwise be counted twice and drive the result negative.
fn net_prose(arena: &Arena, node: usize, exclusions: &[NodeId]) -> u32 {
    let total = arena.prose_len.get(node).copied().unwrap_or(0);
    let end = arena.subtree_end.get(node).copied().unwrap_or(0) as usize;
    let mut removed = 0u32;
    for (k, ex) in exclusions.iter().enumerate() {
        let e = ex.idx();
        if e <= node || e >= end {
            continue;
        }
        // Outermost only. `enumerate` rather than a sort because `exclusions` is short and the
        // order it arrives in is not something this function should depend on.
        let nested = exclusions.iter().enumerate().any(|(j, other)| {
            j != k
                && other.idx() < e
                && (arena.subtree_end.get(other.idx()).copied().unwrap_or(0) as usize) > e
        });
        if !nested {
            removed = removed.saturating_add(arena.prose_len.get(e).copied().unwrap_or(0));
        }
    }
    total.saturating_sub(removed)
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
