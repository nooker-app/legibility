//! The whole extraction, in the order the ordering actually matters.
//!
//! Sequence, and why it cannot be rearranged:
//!
//! 1. **Find repeated groups.** Position-independent, so it does not depend on any later step.
//! 2. **Mask comment prose.** Must happen *before* article scoring. Once a long comment thread is
//!    in the candidate pool it wins — on Hacker News the submission is tens of bytes against
//!    kilobytes of discussion — and no post-filter recovers what it displaced.
//! 3. **Score the article** against masked prose.
//! 4. **Recover if masking was wrong.** Exactly one retry, reported.
//! 5. **Classify what is left**: a page whose content is a non-comment repeated group is a listing.
//!
//! Step 4 exists because a confident false positive is otherwise catastrophic: mask the article,
//! find nothing, return nothing. Step 5 is why a listing page is not silently returned as an
//! article, which is what a scorer alone does.

use alloc::vec::Vec;

use crate::arena::Arena;
use crate::comments::{self, CommentSet};
use crate::groups::{self, Group};
use crate::meta::{self, Metadata};
use crate::num::guarded_div;
use crate::score::{self, Selection};
use crate::shape;
use crate::{Limits, NoArticle};

/// Everything the engine concluded about a document.
#[derive(Debug, Clone)]
pub struct Outcome {
    /// Article region and its accept/reject reasoning.
    pub selection: Selection,
    /// The comment thread, if one was found.
    pub comments: CommentSet,
    /// Metadata candidates with provenance.
    pub metadata: Metadata,
    /// Whether the page is a listing rather than an article.
    pub is_listing: bool,
    /// Set when comment masking was undone because it left no viable article.
    pub comment_mask_reverted: bool,
    /// Repeated groups found, for diagnostics.
    pub group_count: u32,
    /// Share of page prose held by the detected comment thread.
    pub comment_prose_share: f32,
    /// Subtrees inside the article region that must not be serialized as body: every detected
    /// comment, plus the comment-section furniture around them.
    ///
    /// Computed here rather than in the serializer because it is a judgement about content, and the
    /// serializer should only ever be told what to skip. See [`comment_section_nodes`].
    pub article_exclusions: Vec<crate::NodeId>,
    /// On a discussion page: whether the submission carries a body, and the nodes that say so.
    ///
    /// `None` on every page that is not a discussion, which is the overwhelming majority — see
    /// [`crate::shape::decide`] for why the decision is confined that way.
    pub shape: Option<crate::shape::Shape>,
}

/// Run the full pipeline.
#[must_use]
pub fn run(arena: &Arena, limits: Limits) -> Outcome {
    let groups = groups::merge_by_signature(arena, &groups::find_groups(arena));
    let page_prose = arena.prose_len.first().copied().unwrap_or(0);

    // The comment thread is the largest qualifying group. Largest rather than first because a page
    // can contain a small "related discussion" widget alongside the real thread.
    let thread = groups
        .iter()
        .filter(|g| g.is_comment_thread())
        .max_by_key(|g| g.prose_len);

    let masked = if thread.is_some() {
        groups::mask_comment_prose(arena, &groups)
    } else {
        alloc::vec![0u32; arena.len()]
    };

    let mut selection = score::select_article_masked(arena, &masked);
    let mut reverted = false;

    // Recovery: masking must never be able to turn a page with an article into a page without one.
    if selection.article.is_none() && thread.is_some() {
        let unmasked = score::select_article(arena);
        if unmasked.article.is_some() {
            selection = unmasked;
            reverted = true;
        }
    }

    let comments = match thread {
        Some(g) if !reverted => {
            // Extract by identity so a lone reply, which cannot form a group, is still a comment.
            let expanded = Group {
                members: groups::members_by_identity(arena, g),
                ..g.clone()
            };
            let mut set = comments::extract(arena, &expanded, limits.max_comment_items);
            fill_completeness(arena, &mut set);
            set
        }
        _ => CommentSet::default(),
    };

    // A listing is a page whose prose is dominated by a repeated group that is *not* a discussion.
    // Checked after selection so that a normal article containing a small related-links list is not
    // mistaken for one.
    let listing = dominant_listing(&groups, page_prose);
    if listing && comments.is_empty() {
        selection = Selection {
            article: None,
            no_article: Some(NoArticle::IndexPage),
            ..selection
        };
    }

    let mut sections: Vec<crate::NodeId> = Vec::new();
    let article_exclusions = match selection.article {
        Some(region) => {
            let mut v: Vec<crate::NodeId> = comments.items.iter().map(|c| c.node).collect();
            sections = comment_section_nodes(arena, region, &v);
            v.extend(sections.iter().copied());
            v
        }
        None => Vec::new(),
    };

    let metadata = meta::extract(arena);

    // A discussion is a page with replies *or* with the furniture that would hold them. The
    // second half matters: a link submission with zero replies still has a comment section, and
    // that is exactly the page whose "article" was a credit bar and a URL.
    let is_discussion = !comments.items.is_empty() || !sections.is_empty();
    let shape = selection
        .article
        .and_then(|region| shape::decide(arena, region, &article_exclusions, is_discussion));

    // A heading that only repeats the page title is not content -- and it is added *after* the
    // shape decision on purpose. `shape::decide` finds the submission title by scanning for the
    // first heading and skips excluded subtrees, so excluding the heading first would make it
    // anchor on the next one down and change the decision it is supposed to be reporting.
    let mut article_exclusions = article_exclusions;
    if let Some(region) = selection.article {
        // Compare against a title the *document* declared, never one harvested from a heading.
        // An `<h1>` candidate outranks `<title>` for the metadata field, so comparing the heading
        // against the winning candidate is circular and matches every time -- which is how the
        // corpus page named `title-and-h1-discrepancy` lost the `<h1>` that Readability keeps.
        // Readability compares against `document.title` for the same reason.
        if let Some(declared) = declared_title(&metadata) {
            if let Some(h) = duplicate_heading(arena, region, declared) {
                article_exclusions.push(h);
            }
        }
    }

    Outcome {
        shape,
        selection,
        comment_prose_share: thread
            .map_or(0.0, |g| guarded_div(g.prose_len as f32, page_prose as f32)),
        comments,
        metadata,
        is_listing: listing,
        comment_mask_reverted: reverted,
        group_count: u32::try_from(groups.len()).unwrap_or(u32::MAX),
        article_exclusions,
    }
}

/// Comment-section containers inside `region` that hold no detected comment.
///
/// Masking removes comments from the statistics and [`Outcome::article_exclusions`] removes them
/// from the output, but neither helps when the section is *empty*: a Reddit post with no replies
/// still carries "Be the first to comment", the composer and the sort controls inside the `<main>`
/// that wins, and all of it reads as body text. There is nothing to mask because there is nothing
/// there.
///
/// Two matchers, deliberately of different strictness:
///
/// - **Element name** containing `comment`. Custom elements are named by the site for what they
///   are — `<shreddit-comment-tree>`, `<comment-body-header>` — and a made-up tag name with
///   "comment" in it is not plausibly article prose.
/// - **`class`/`id`** matching a *section-like* token only (`comments`, `comment-list`,
///   `comment-tree`, `disqus_thread`, `respond`, `댓글`…). A bare `comment` token is too weak:
///   `class="comment-policy"` on an article *about* moderation would take the article with it.
///
/// Guarded by an invariant rather than a threshold: **removal must leave some prose behind**. That
/// is what separates the two cases exactly. On a post with an empty comment section the tree is a
/// sibling of the body, so removing it leaves the body — even when the empty state is longer than
/// the post, which it often is. On an article *about* moderation, `<div class="comments">` holds
/// everything, so removing it leaves nothing and the match is refused.
///
/// A share threshold was tried first and got this backwards: at "no more than half the region" the
/// genuine case failed whenever the boilerplate outweighed a short post, which is the case that
/// motivated the rule.
#[must_use]
pub fn comment_section_nodes(
    arena: &Arena,
    region: crate::NodeId,
    already: &[crate::NodeId],
) -> Vec<crate::NodeId> {
    /// Section-like `class`/`id` tokens. Plural or structural forms only; see above.
    const SECTION_TOKENS: [&str; 11] = [
        "comments",
        "comment-list",
        "commentlist",
        "comment-tree",
        "comment-section",
        "comment-thread",
        "commentthread",
        "disqus_thread",
        "disqus-thread",
        "respond",
        "댓글",
    ];

    let region_prose = arena.prose_len.get(region.idx()).copied().unwrap_or(0);
    let end = arena.subtree_end.get(region.idx()).copied().unwrap_or(0) as usize;

    let mut found: Vec<crate::NodeId> = Vec::new();
    let mut covered_until = 0usize;
    for i in (region.idx() + 1)..end {
        if arena.kind.get(i).copied() != Some(crate::NodeKind::Element) {
            continue;
        }
        // Outermost match wins; anything inside it goes with it anyway.
        if i < covered_until {
            continue;
        }
        let node = crate::NodeId(i as u32);
        if already.iter().any(|n| n.idx() == i) {
            continue;
        }
        let by_name = arena.tag_name(node).is_some_and(|n| n.contains("comment"));
        let by_attr = [crate::arena::AttrName::CLASS, crate::arena::AttrName::ID]
            .iter()
            .filter_map(|&a| arena.attr(node, a))
            .any(|v| {
                v.split(|c: char| c.is_whitespace())
                    .any(|t| SECTION_TOKENS.iter().any(|s| t.eq_ignore_ascii_case(s)))
            });
        if by_name || by_attr {
            found.push(node);
            covered_until = arena.subtree_end.get(i).copied().unwrap_or(0) as usize;
        }
    }

    let removed: u32 = found
        .iter()
        .map(|n| arena.prose_len.get(n.idx()).copied().unwrap_or(0))
        .sum();
    if removed >= region_prose {
        return Vec::new();
    }
    found
}

/// Whether a non-comment repeated group holds most of the page's prose.
///
/// The threshold is a *share*, not a count: a front page is mostly its list of items, while an
/// article that happens to end with a "related posts" block is not.
fn dominant_listing(groups: &[Group], page_prose: u32) -> bool {
    const DOMINANT_SHARE: f32 = 0.55;
    groups
        .iter()
        .filter(|g| g.is_listing() && !g.is_comment_thread())
        .any(|g| guarded_div(g.prose_len as f32, page_prose as f32) >= DOMINANT_SHARE)
}

/// Fill in how much of the thread is present, and how we know.
fn fill_completeness(arena: &Arena, set: &mut CommentSet) {
    // The claimed total is stated in page text near the thread; scan the whole document rather than
    // guess where. Cheap: one pass over prose text nodes.
    let mut claimed = None;
    for i in 0..arena.len() {
        if arena.kind.get(i).copied() != Some(crate::NodeKind::Text) {
            continue;
        }
        let t = arena.own_text(crate::NodeId(i as u32));
        if let Some(n) = comments::parse_claimed_total(t) {
            claimed = Some(n);
            break;
        }
    }
    set.completeness.claimed_total = claimed;

    // Continuation links: anchors whose text offers more of the thread.
    let mut continuation = Vec::new();
    for i in 0..arena.len() {
        if arena.tag.get(i).copied() != Some(crate::TagId::A) {
            continue;
        }
        let id = crate::NodeId(i as u32);
        let text = subtree_text_lower(arena, id);
        let offers_more = text.contains("more comment")
            || text.contains("load more")
            || text.trim() == "more"
            || text.contains("다음")
            || text.contains("더 보기");
        if offers_more {
            if let Some(href) = arena.attr(id, crate::arena::AttrName::HREF) {
                continuation.push(alloc::string::String::from(href));
            }
        }
    }

    // Invariant: present != claimed_total must imply truncated. Asserting it here rather than
    // hoping, because a silently short thread is the failure this whole struct exists to prevent.
    let short = claimed.is_some_and(|c| c > set.completeness.present);
    if short || !continuation.is_empty() {
        set.completeness.truncated = true;
        if set.completeness.reason.is_none() {
            set.completeness.reason = Some(if continuation.is_empty() {
                comments::TruncationReason::LoadMoreStub
            } else {
                comments::TruncationReason::Pagination
            });
        }
    }
    set.completeness.continuation = continuation;
    debug_assert!(
        claimed.is_none_or(|c| c == set.completeness.present) || set.completeness.truncated,
        "present != claimed_total while truncated is false"
    );
}

fn subtree_text_lower(arena: &Arena, node: crate::NodeId) -> alloc::string::String {
    let end = arena.subtree_end.get(node.idx()).copied().unwrap_or(0) as usize;
    let mut out = alloc::string::String::new();
    for i in node.idx()..end {
        if arena.kind.get(i).copied() == Some(crate::NodeKind::Text) {
            for c in arena.own_text(crate::NodeId(i as u32)).chars() {
                out.extend(c.to_lowercase());
            }
        }
    }
    out
}

/// The best title the document states about itself, excluding any taken from a heading.
///
/// `<title>`, `og:title`, JSON-LD and microdata are the publisher declaring a title. An `<h1>` is
/// the page *displaying* one, so it cannot serve as the yardstick for deciding whether a heading is
/// redundant.
fn declared_title(m: &Metadata) -> Option<&str> {
    core::iter::once(m.title.as_ref())
        .flatten()
        .chain(
            m.alternatives
                .iter()
                .filter(|(field, _)| *field == "title")
                .map(|(_, c)| c),
        )
        .filter(|c| c.source != meta::Source::H1)
        .max_by_key(|c| c.confidence)
        .map(|c| c.value.as_str())
}

/// The region's leading heading, when it says nothing the page title has not already said.
///
/// Readability drops such a heading (`_headerDuplicatesTitle`), so the corpus `expected.html`
/// files were produced without it and every page where we keep it is scored as if we invented
/// text. It also accounts for the duplicated headline in front of every discussion body: the
/// title is already a metadata field with provenance, and repeating it inside the article is not
/// an extra fact.
///
/// Deliberately narrow. Only the first `h1`/`h2` carrying prose is considered, and only exact
/// equality after folding — Readability uses a 75%-similarity test, which would let a heading go
/// that merely resembles the title, and losing a real heading is worse than keeping a redundant
/// one. Plan D4 reports the span as `lead.heading_span`; this is the removal half only.
fn duplicate_heading(arena: &Arena, region: crate::NodeId, title: &str) -> Option<crate::NodeId> {
    let end = (arena.subtree_end.get(region.idx()).copied().unwrap_or(0) as usize).min(arena.len());
    let want = fold(title);
    if want.is_empty() {
        return None;
    }
    for i in (region.idx() + 1)..end {
        if arena.kind.get(i).copied() != Some(crate::NodeKind::Element) {
            continue;
        }
        if !matches!(
            arena.tag.get(i).copied(),
            Some(crate::TagId::H1 | crate::TagId::H2)
        ) {
            continue;
        }
        if arena.prose_len.get(i).copied().unwrap_or(0) == 0 {
            continue;
        }
        return (fold(&node_prose(arena, i)) == want).then_some(crate::NodeId(i as u32));
    }
    None
}

/// Prose text of a subtree, whitespace-collapsed.
fn node_prose(arena: &Arena, node: usize) -> alloc::string::String {
    let end = (arena.subtree_end.get(node).copied().unwrap_or(0) as usize).min(arena.len());
    let mut out = alloc::string::String::new();
    for i in node..end {
        if arena.kind.get(i).copied() != Some(crate::NodeKind::Text) {
            continue;
        }
        if arena.text_role.get(i).copied().is_none_or(|r| !r.is_prose()) {
            continue;
        }
        for w in arena.own_text(crate::NodeId(i as u32)).split_whitespace() {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(w);
        }
    }
    out
}

/// Case- and whitespace-insensitive form for comparing a heading against a title.
///
/// No NFKC: plan §M3 forbids it, because folding full-width to ASCII changes CJK text rather than
/// normalising it. Lowercasing is inside the comparator only, never applied to a returned value.
fn fold(s: &str) -> alloc::string::String {
    let mut out = alloc::string::String::new();
    for w in s.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        for c in w.chars().flat_map(char::to_lowercase) {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::float_cmp, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn a_dominant_listing_needs_a_share_not_a_count() {
        // An article ending in a five-item "related posts" block must not be called a listing, no
        // matter how many items that block has.
        let small = Group {
            parent: crate::NodeId(0),
            members: alloc::vec![crate::NodeId(1), crate::NodeId(2), crate::NodeId(3)],
            signature: 1,
            micro_metadata_ratio: 0.0,
            prose_len: 200,
            mean_link_density: 0.9,
            length_cv: 0.05,
            all_members_have_heading: true,
            mean_max_link_share: 0.6,
            mean_first_link_share: 0.6,
        };
        assert!(small.is_listing());
        assert!(!dominant_listing(core::slice::from_ref(&small), 10_000), "5% of the page is not dominant");

        let big = Group { prose_len: 9_000, ..small };
        assert!(dominant_listing(&[big], 10_000), "90% of the page is a listing");
    }
}
