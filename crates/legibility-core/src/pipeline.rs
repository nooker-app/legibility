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

    // What the page says its comment count is. Read before classification, not after, because a
    // thread of two is only recognisable *with* it -- see `Group::is_comment_thread`.
    let stated_total = comments::claimed_total(arena);

    // The comment thread is the largest qualifying group. Largest rather than first because a page
    // can contain a small "related discussion" widget alongside the real thread.
    // Largest, and on a tie the one that appears first. `max_by_key` alone returns the *last*
    // maximum, which is the opposite of the document-order rule stated everywhere else here and
    // would hand back a different thread for two groups of equal prose. Reversing the first member's
    // index inside the key makes the whole key a total order, so the choice cannot depend on
    // iteration order or on which of two equal groups was seen last.
    let thread = groups
        .iter()
        .filter(|g| g.is_comment_thread(stated_total))
        .max_by_key(|g| {
            (
                g.prose_len,
                core::cmp::Reverse(g.members.first().map_or(u32::MAX, |m| m.0)),
            )
        });

    let masked = if thread.is_some() {
        groups::mask_comment_prose(arena, &groups, stated_total)
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
            fill_completeness(arena, &mut set, stated_total);
            set
        }
        // No thread found, but the page may still say it has comments -- a topic with a single reply
        // states `댓글 1개` and a lone element cannot form a repeated group, so detection finds
        // nothing. Reporting `count: 0` and `claimed_total: null` there is the silent omission plan
        // §1.9 exists to make impossible: the caller cannot tell "no comments" from "we missed them".
        // Filling completeness anyway turns it into `0 of 1, truncated`, which is what actually
        // happened.
        _ => {
            let mut set = CommentSet::default();
            fill_completeness(arena, &mut set, stated_total);
            set
        }
    };

    // A listing is a page whose prose is dominated by a repeated group that is *not* a discussion.
    // Checked after selection so that a normal article containing a small related-links list is not
    // mistaken for one.
    let listing = dominant_listing(&groups, page_prose, stated_total);
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
            v.extend(furniture_landmarks(arena, region));
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
                article_exclusions.extend(headline_furniture(arena, region, h));
            }
        }
    }
    // The dateline in front of the headline, on discussion pages only. `shape` is `None` for
    // every other page on the web, so no article can reach this.
    if let Some(b) = shape.and_then(|s| s.byline) {
        article_exclusions.push(b);
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
///
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

/// Boilerplate tokens in `class` or `id`, **corroborated by structure**.
///
/// This is the Readability rule that our engine had no equivalent of, and the difference showed on a
/// GitHub pull request: the repository bar — `Notifications  Fork 6  Star 78` — sits in a `<div>`
/// inside `<main>`, carries no landmark role, and reads as body text. Readability removes it because
/// its id is `repository-container-header` and `header` is one of the words in its
/// `unlikelyCandidates` list.
///
/// Trusting the name alone is also how Readability eats articles, so the name is necessary here and
/// not sufficient. A subtree is furniture when it is *named* like furniture **and**
///
/// - contains no paragraph or blockquote carrying prose — an authored block is content whatever the
///   wrapper is called, which is what protects an `<div class="social-share">` wrapped around a pull
///   quote, and
/// - is mostly links, so a navigation bar qualifies and an article section named `related-notes`
///   with real sentences does not.
///
/// That makes it strictly more conservative than the upstream rule while removing the same thing on
/// the page that motivated it.
fn named_furniture(arena: &Arena, node: crate::NodeId) -> bool {
    /// Whole `-`/`_`-separated tokens only: `header` must not match `headline`, and `ad` must never
    /// be a substring test at all.
    const TOKENS: [&str; 14] = [
        "header", "footer", "nav", "navigation", "menu", "sidebar", "banner", "breadcrumb",
        "breadcrumbs", "toolbar", "pagination", "pager", "social", "share",
    ];
    let named = [crate::arena::AttrName::CLASS, crate::arena::AttrName::ID]
        .into_iter()
        .filter_map(|a| arena.attr(node, a))
        .any(|v| {
            v.split([' ', '-', '_'])
                .any(|t| TOKENS.iter().any(|k| t.eq_ignore_ascii_case(k)))
        });
    if !named {
        return false;
    }
    !holds_authored_block(arena, node) && arena.link_density(node) > 0.5
}

/// Share of a credit bar's prose that sits inside links, above which it is not a dateline.
///
/// A news dateline is plain text with at most one byline link; a submission credit bar is a row of
/// links with a timestamp among them. Measured on the corpus rather than chosen: see
/// [`furniture_landmarks`].
const CREDIT_BAR_LINK_SHARE: f32 = 0.5;

/// Whether a subtree contains a `<time>`.
fn has_time(arena: &Arena, node: crate::NodeId) -> bool {
    let end = (arena.subtree_end.get(node.idx()).copied().unwrap_or(0) as usize).min(arena.len());
    (node.idx()..end).any(|i| arena.tag.get(i).copied() == Some(crate::TagId::TIME))
}

/// Whether a subtree contains a form control — something that could actually be submitted.
fn holds_control(arena: &Arena, node: crate::NodeId) -> bool {
    let end = (arena.subtree_end.get(node.idx()).copied().unwrap_or(0) as usize).min(arena.len());
    (node.idx()..end).any(|i| {
        arena.kind.get(i).copied() == Some(crate::NodeKind::Element)
            && matches!(
                arena.tag.get(i).copied(),
                // No `textarea` in the interned set; `input`, `button` and `select` are enough
                // to recognise anything that submits, and a bare `<textarea>` with no button is
                // not a form anyone can send.
                Some(crate::TagId::INPUT | crate::TagId::BUTTON | crate::TagId::SELECT)
            )
    })
}

/// Whether a subtree contains a paragraph or a blockquote carrying prose.
///
/// The one test that separates "a wrapper someone wrote into" from "a strip of controls", and the
/// reason both the named match above and the `<form>` case can be trusted at all.
fn holds_authored_block(arena: &Arena, node: crate::NodeId) -> bool {
    let end = (arena.subtree_end.get(node.idx()).copied().unwrap_or(0) as usize).min(arena.len());
    (node.idx()..end).any(|i| {
        arena.kind.get(i).copied() == Some(crate::NodeKind::Element)
            && matches!(arena.tag.get(i).copied(), Some(crate::TagId::P | crate::TagId::BLOCKQUOTE))
            && arena.prose_len.get(i).copied().unwrap_or(0) > 0
    })
}

/// Page furniture inside the chosen region, by its own declared role.
///
/// Plan §1.10.4 lists these as negative landmarks and [`crate::TagId::is_negative_landmark`] has
/// existed unused since. The gap shows on any application-shaped page: GitHub puts its repository
/// navigation *inside* `<main>`, so the extracted body of a pull request opened with
///
/// ```text
///   Code  Issues 4  Pull requests 3  Discussions  Actions  Security  Insights
///   Conversation  Commits (5)  Checks  Files changed
/// ```
///
/// Readability drops all of that, and a `<nav>` is the page telling us it is navigation, which is
/// better evidence than any statistic we could compute about it.
///
/// # What is excluded, and what is deliberately not
///
/// `<nav>`, `<footer>` and `<form>` only. Two of the five negative landmarks are left in, each for a
/// measured reason:
///
/// - **`<aside>`** was excluded first and cost `nytimes-3` 0.988 → 0.952. The NYT wraps body
///   paragraphs in it — *"In the late 1800s, many of the city's overhead utilities were buried…"* is
///   article prose inside an `<aside>`, as are most of the article's images. The element is widely
///   used to mean "a floated block", not "not the article", so the claim is not worth trusting.
/// - **`<header>`** inside an article usually holds the headline and the dateline, which are wanted —
///   plan D4 routes those to `lead` rather than deleting them — and on a pull request it holds the
///   title. Excluding it would be a second, much larger change wearing this one's clothes.
///
/// `<nav>` and `<footer>` survive that objection because their meaning is unambiguous in the spec
/// and sites do not reach for them to lay out prose. `<form>` carries the comment composer, which is
/// the same furniture problem that motivated [`comment_section_nodes`].
///
/// The same invariant as comment sections: removal must leave prose behind. A page whose entire
/// body sits inside a `<form>` exists (a wiki edit preview, a search-results article), and losing
/// it to tidy up a toolbar is the worse error.
fn furniture_landmarks(arena: &Arena, region: crate::NodeId) -> alloc::vec::Vec<crate::NodeId> {
    let end = (arena.subtree_end.get(region.idx()).copied().unwrap_or(0) as usize).min(arena.len());
    let mut out = alloc::vec::Vec::new();
    let mut removed = 0u32;
    // Ancestor stack of open `<blockquote>`/`<figure>` subtree ends: inside either, a `<footer>` is
    // the *attribution* and the HTML spec says so explicitly. Excluding it deletes the name of the
    // person being quoted, which is content by any reading.
    let mut quoting: alloc::vec::Vec<usize> = alloc::vec::Vec::new();
    let mut i = region.idx() + 1;
    while i < end {
        while quoting.last().is_some_and(|&e| i >= e) {
            quoting.pop();
        }
        let tag = arena.tag.get(i).copied().unwrap_or(crate::TagId::UNKNOWN);
        let is_element = arena.kind.get(i).copied() == Some(crate::NodeKind::Element);
        let node = crate::NodeId(i as u32);
        if is_element && matches!(tag, crate::TagId::BLOCKQUOTE | crate::TagId::FIGURE) {
            quoting.push((arena.subtree_end.get(i).copied().unwrap_or(0) as usize).max(i + 1));
        }
        let semantic = match tag {
            crate::TagId::NAV => true,
            // A credit bar: a block carrying a `<time>` whose text is almost all links.
            //
            // Every discussion site opens a submission with one — community, author, age, score —
            // and it rides inside the article region. There is already a rule for it, but it is
            // gated on the page being recognised as a discussion, which needs the comment section to
            // be present and detected. A Reddit post captured with no comment furniture, or one
            // whose `<title>` does not match its `<h1>`, satisfies neither gate, and the credit bar
            // came back at the top of the body:
            //
            //   rss 커뮤니티로 이동  r/rss  13일 전  awesome5ftw  I'm very new to RSS and …
            //
            // What is true regardless of comments and of the title is the shape. A news dateline
            // looks the same at a glance — `By A Reporter <time>24 July 2026</time>` — and must be
            // kept, because `expected.html` keeps it. The two come apart on links: a credit bar is
            // links almost end to end (community, author, permalink), a dateline is plain text with
            // at most a byline link. That is the whole discriminator, and it needs nothing else
            // about the page to be known.
            _ if has_time(arena, node) && !holds_authored_block(arena, node) => {
                arena.link_density(node) > CREDIT_BAR_LINK_SHARE
            }
            crate::TagId::FOOTER => quoting.is_empty(),
            // A `<form>` is a wrapper as often as it is a toolbar. old.reddit.com puts every post
            // body *and* every comment body inside `<form class="usertext" action="#"
            // onsubmit="return false;">`, so excluding it on the tag alone deleted the site. The
            // page-level "leaves prose behind" invariant below does not save that: the comment
            // bodies are not the region's only prose.
            //
            // The corroboration is whether it has anything to submit. A newsletter box or a search
            // bar contains an `<input>` or a `<button>`; a form used purely for layout contains
            // none, and is a `<div>` wearing the wrong tag. Requiring "no authored paragraph"
            // instead was tried and cost `theverge` 0.906 → 0.880, because a signup box legitimately
            // carries one line of prose telling you what you are signing up for.
            crate::TagId::FORM => holds_control(arena, node),
            _ => false,
        };
        if is_element && (semantic || named_furniture(arena, node)) {
            out.push(crate::NodeId(i as u32));
            removed = removed.saturating_add(arena.prose_len.get(i).copied().unwrap_or(0));
            // Skip the subtree: a `<nav>` inside an excluded `<aside>` adds nothing, and nested
            // entries would double-count `removed`.
            i = (arena.subtree_end.get(i).copied().unwrap_or(0) as usize).max(i + 1);
            continue;
        }
        i += 1;
    }
    if arena.prose_len.get(region.idx()).copied().unwrap_or(0) <= removed {
        out.clear();
    }
    out
}

/// Whether a non-comment repeated group holds most of the page's prose.
///
/// The threshold is a *share*, not a count: a front page is mostly its list of items, while an
/// article that happens to end with a "related posts" block is not.
fn dominant_listing(groups: &[Group], page_prose: u32, stated_total: Option<u32>) -> bool {
    const DOMINANT_SHARE: f32 = 0.55;
    groups
        .iter()
        .filter(|g| g.is_listing() && !g.is_comment_thread(stated_total))
        .any(|g| guarded_div(g.prose_len as f32, page_prose as f32) >= DOMINANT_SHARE)
}

/// Fill in how much of the thread is present, and how we know.
fn fill_completeness(arena: &Arena, set: &mut CommentSet, stated_total: Option<u32>) {
    // Passed in rather than re-scanned: the same number decided whether this group counted as a
    // thread at all, and reading it twice invites the two answers to differ.
    set.completeness.claimed_total = stated_total;

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
    let short = stated_total.is_some_and(|c| c > set.completeness.present);
    if short || !continuation.is_empty() {
        set.completeness.truncated = true;
        // A reason only where there is evidence for one. `LoadMoreStub` used to be the fallback for
        // "short and no continuation link", which reads as a finding about the page when it is
        // really an absence of one: the thread can also be short because *we* failed to detect it,
        // as happens on a topic with a single reply, where one element cannot form a repeated group.
        // `truncated: true, reason: null` says "incomplete, cause unknown", which is the truth.
        if set.completeness.reason.is_none() && !continuation.is_empty() {
            set.completeness.reason = Some(comments::TruncationReason::Pagination);
        }
    }
    set.completeness.continuation = continuation;
    debug_assert!(
        stated_total.is_none_or(|c| c == set.completeness.present) || set.completeness.truncated,
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
/// # Why the comparison is directional rather than an equality test
///
/// The first attempt required exact equality after whitespace and case folding, on the theory
/// that anything looser risks dropping a real heading. It does not fire on the pages it exists
/// for, because a `<title>` nearly always carries the site name and an `<h1>` nearly never does:
///
/// ```text
///   <title>  New to RSS here, why can't i use this website to add to my feed? : r/rss
///   <h1>     New to RSS here, why can't i use this website to add to my feed?
/// ```
///
/// The measurement that separates "restates the title" from "says something new" is asymmetric:
/// **how much of the heading is already in the title.** Suffixes the title adds are then free,
/// and a heading carrying its own words survives, because its own words are exactly what fails to
/// match. This is Readability's `_textSimilarity` in the same direction and at the same threshold,
/// which matters beyond agreeing with it — the corpus `expected.html` files were produced by that
/// rule, so any other threshold reads as a difference from ground truth.
///
/// Only the first `h1`/`h2` carrying prose is considered. Plan D4 reports the span as
/// `lead.heading_span`; this is the removal half only.
fn duplicate_heading(arena: &Arena, region: crate::NodeId, title: &str) -> Option<crate::NodeId> {
    let end = (arena.subtree_end.get(region.idx()).copied().unwrap_or(0) as usize).min(arena.len());
    let want = tokens(title);
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
        let heading = tokens(&node_prose(arena, i));
        return restates(&want, &heading).then_some(crate::NodeId(i as u32));
    }
    None
}

/// Longest a label beside the headline may be and still count as furniture.
///
/// A category chip, a flair, a byline word, a reading-time badge — never a sentence. Eight Korean
/// characters or twenty-four ASCII ones.
const HEADLINE_SLACK: u32 = 24;

/// Micro-labels sharing the headline's parent, which are furniture once the headline is gone.
///
/// Excluding an `<h1>` leaves its *siblings* in the body, and on a discussion page a sibling is a
/// label. beebs.hada.io renders
///
/// ```text
///   <div class="thread-detail-title-line">
///     <span class="category-chip">질문</span>
///     <h1>인앱 브라우저에서 패스키를 지원할 방법은 없는걸까요?</h1>
///   </div>
/// ```
///
/// so excluding the `<h1>` alone left `질문` — two characters of category label — as the first thing
/// in the body. Two characters look like a rounding error and are not: they are an artifact of a
/// removal this code performed.
///
/// # Why siblings and not the wrapper
///
/// The first attempt ascended to the outermost ancestor whose prose exceeded the heading's by less
/// than [`HEADLINE_SLACK`] and excluded *that*. It is a much more natural way to say "the title
/// line", and it deletes articles. Review found it in one page:
///
/// ```text
///   <article><div class="post">
///     <h1>Passkeys in in-app browsers are broken</h1>
///     <p>Yes, confirmed here too.</p>
///   </div></article>
/// ```
///
/// A body of twenty-four bytes puts `div.post` inside the bound, `div.post` spans the whole region,
/// and the extraction came back with `html: ""` — an article reported as found, containing nothing,
/// with no diagnostic saying why. A short comment-shaped page is not a rare thing to meet.
///
/// Excluding only siblings cannot do that, whatever the constant is: an ancestor is never a
/// candidate, so no subtree containing the body can be named. The bound is then about precision
/// alone rather than about safety, which is the right thing for a constant to be about.
fn headline_furniture(
    arena: &Arena,
    region: crate::NodeId,
    heading: crate::NodeId,
) -> alloc::vec::Vec<crate::NodeId> {
    let mut out = alloc::vec::Vec::new();
    // The headline's parent: the innermost element that precedes it and whose subtree reaches past
    // it. Document order plus contiguous subtrees make "innermost" simply the largest such index.
    let Some(parent) = (region.idx() + 1..heading.idx())
        .rev()
        .find(|&a| {
            arena.kind.get(a).copied() == Some(crate::NodeKind::Element)
                && (arena.subtree_end.get(a).copied().unwrap_or(0) as usize) > heading.idx()
        })
        .map(|a| crate::NodeId(a as u32))
    else {
        return out;
    };

    let parent_end =
        (arena.subtree_end.get(parent.idx()).copied().unwrap_or(0) as usize).min(arena.len());
    let mut taken = 0u32;
    let mut c = parent.idx() + 1;
    while c < parent_end {
        let child_end = (arena.subtree_end.get(c).copied().unwrap_or(0) as usize).max(c + 1);
        let prose = arena.prose_len.get(c).copied().unwrap_or(0);
        if arena.kind.get(c).copied() == Some(crate::NodeKind::Element)
            && c != heading.idx()
            && prose > 0
            && prose <= HEADLINE_SLACK
            // A paragraph is content by declaration, however short. The author wrote `<p>`; a chip
            // is a `<span>` or a `<div>`, and the distinction is the site's own rather than ours.
            // Without this, `<h1>…</h1><p>Yes, confirmed here too.</p>` -- twenty-four bytes, so
            // inside the bound -- loses the reply that is the entire page.
            && !matches!(arena.tag.get(c).copied(), Some(crate::TagId::P))
        {
            out.push(crate::NodeId(c as u32));
            taken += prose;
        }
        c = child_end;
    }

    // A removal must leave prose behind. The same invariant guards comment-section removal, for the
    // same reason: every rule here is a heuristic, and the cost of one being wrong should be a
    // stray label rather than an empty article reported as a full one. Cheap, and it makes the
    // bound above a question of precision rather than of safety.
    let region_prose = arena.prose_len.get(region.idx()).copied().unwrap_or(0);
    let heading_prose = arena.prose_len.get(heading.idx()).copied().unwrap_or(0);
    if region_prose <= heading_prose.saturating_add(taken) {
        out.clear();
    }
    out
}

/// Share of a heading's text that the title already contains, tested against 0.75.
///
/// Weighted by token length, not token count, so that a heading differing by one long word is
/// judged differently from one differing by an article or a preposition — Readability measures the
/// same way, by joined string length.
fn restates(title: &[alloc::string::String], heading: &[alloc::string::String]) -> bool {
    if title.is_empty() || heading.is_empty() {
        return false;
    }
    let joined = |toks: &[alloc::string::String]| -> usize {
        toks.iter().map(alloc::string::String::len).sum::<usize>() + toks.len().saturating_sub(1)
    };
    let mut absent_len = 0usize;
    let mut absent_n = 0usize;
    for t in heading {
        if !title.iter().any(|x| x == t) {
            absent_len += t.len();
            absent_n += 1;
        }
    }
    let absent = absent_len + absent_n.saturating_sub(1);
    guarded_div(absent as f32, joined(heading) as f32) < 0.25
}

/// Lowercased alphanumeric runs.
///
/// Unicode-aware rather than Readability's `\W+`, which classifies every CJK codepoint as a
/// separator and so produces an empty token list for a Korean or Japanese heading — the
/// comparison would then never fire on exactly the pages this project is being built to read.
/// Here a run of ideographs is one token, which is coarse but conservative: it can only make the
/// test stricter, never looser.
fn tokens(s: &str) -> alloc::vec::Vec<alloc::string::String> {
    let mut out = alloc::vec::Vec::new();
    let mut cur = alloc::string::String::new();
    for c in s.chars() {
        if c.is_alphanumeric() {
            cur.extend(c.to_lowercase());
        } else if !cur.is_empty() {
            out.push(core::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::float_cmp, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn a_dominant_listing_needs_a_share_not_a_count() {
        // An article ending in a five-item "related posts" block must not be called a listing, no
        // matter how many items that block has.
        let small = Group {
            paragraph_ratio: 1.0,
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
            by_identity: false,
        };
        assert!(small.is_listing());
        assert!(!dominant_listing(core::slice::from_ref(&small), 10_000, None), "5% of the page is not dominant");

        let big = Group { prose_len: 9_000, ..small };
        assert!(dominant_listing(&[big], 10_000, None), "90% of the page is a listing");
    }
}
