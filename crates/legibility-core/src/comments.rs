//! Comment thread extraction (plan §1.7, defect 3).
//!
//! Turns a detected [`Group`](crate::groups::Group) into an ordered, parented list of items.
//!
//! # Flat, not a tree
//!
//! [`CommentSet`] stores items in document order with a parent index rather than nesting them.
//! Nesting would mean a recursive builder and a recursive consumer, and thread depth is
//! attacker-controlled — a 100 000-deep reply chain is a stack overflow, which cannot be caught.
//! A flat list with parents is walkable iteratively and carries the same information.
//!
//! # Deleted comments are kept
//!
//! An item with no text still becomes a node. Removing it would break the parent chain of its
//! replies, and a thread that cannot be reassembled is worse than one containing a placeholder.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::arena::{Arena, AttrName, NodeId, NodeKind};
use crate::groups::Group;
use crate::tag::TagId;

/// What kind of contribution an item is.
///
/// Q&A sites are two-tier: an answer is not a comment, and putting both in one pool makes
/// `count` meaningless and turns answer bodies into replies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentKind {
    /// A reply in a discussion.
    Reply,
    /// An answer to a question.
    Answer,
}

/// Where an item's depth came from, so a consumer can tell a real tree from a guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepthSource {
    /// A CSS custom property such as `--comment-depth`.
    CssVariable,
    /// Nesting of members within one another.
    DomNesting,
    /// An indentation width (`margin-left`, `padding-left`, a spacer image).
    Indentation,
    /// No depth information; every item is top-level.
    Flat,
}

/// Per-item state. None of these are ever a reason to drop the item.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Flags {
    /// Removed by its author or a moderator.
    pub deleted: bool,
    /// Flagged or killed (Hacker News `[dead]`).
    pub dead: bool,
    /// Collapsed by default, usually for a low score.
    pub collapsed: bool,
    /// Edited after posting.
    pub edited: bool,
    /// Written by the submission's author.
    pub op: bool,
    /// Marked as the accepted answer.
    pub accepted: bool,
}

/// One comment.
#[derive(Debug, Clone)]
pub struct CommentItem {
    /// Node this item was extracted from.
    pub node: NodeId,
    /// Author, verbatim from the document.
    pub author: Option<String>,
    /// Timestamp, verbatim. Not parsed here: relative times need a reference instant, and the core
    /// has no clock.
    pub timestamp: Option<String>,
    /// Prose text, with control and hidden text already excluded.
    pub text: String,
    /// The subtree holding the comment's *content*, as opposed to its byline and controls.
    ///
    /// Named so the serializer can emit formatted HTML for a comment without re-deciding what a
    /// comment is. Comments arrive with structure — lists, code blocks, quotes, emphasis, headings —
    /// and until now only [`Self::text`] existed, so every consumer rendered a flattened paragraph
    /// and the formatting looked stripped. It was never extracted.
    ///
    /// `None` when nothing under the item carries prose outside its byline.
    pub body: Option<NodeId>,
    /// Nodes the author and the timestamp were read from.
    ///
    /// Handed to the serializer so it can leave them out of [`Self::body`] without having to decide
    /// again what a byline is. They are already returned as [`Self::author`] and [`Self::timestamp`],
    /// and when [`Self::body`] is the whole comment — which it is whenever the content is spread over
    /// sibling blocks — the byline sits inside it and would be rendered a second time.
    pub byline: Vec<NodeId>,
    /// Depth in the thread; 0 is top level.
    pub depth: u16,
    /// Index into [`CommentSet::items`] of the parent, if any.
    pub parent: Option<u32>,
    /// Fragment or URL that links to this item.
    pub permalink: Option<String>,
    /// Score or vote count, when the site exposes one.
    pub score: Option<i64>,
    /// Reply or answer.
    pub kind: CommentKind,
    /// Per-item state.
    pub flags: Flags,
}

/// Why a thread is incomplete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TruncationReason {
    /// A "load more comments" stub: the remainder is not in the HTML at all.
    LoadMoreStub,
    /// The thread is paginated.
    Pagination,
    /// Lazily loaded on scroll.
    LazyScroll,
    /// Our own [`crate::Limits::max_comment_items`] was reached.
    LimitHit,
}

/// How much of the thread is actually present.
///
/// Reported because you cannot extract what is not in the HTML, and a caller shown 20 of 400
/// comments with no indication would reasonably believe it had all of them.
#[derive(Debug, Clone, Default)]
pub struct Completeness {
    /// Items extracted.
    pub present: u32,
    /// Total the page claims, when it says.
    pub claimed_total: Option<u32>,
    /// Whether anything is missing.
    pub truncated: bool,
    /// Why, when truncated.
    pub reason: Option<TruncationReason>,
    /// URLs that would yield the rest. Never followed: the core has no network.
    pub continuation: Vec<String>,
}

/// An extracted thread.
#[derive(Debug, Clone, Default)]
pub struct CommentSet {
    /// Items in document order.
    pub items: Vec<CommentItem>,
    /// Where depth came from.
    pub depth_source: Option<DepthSource>,
    /// Presence accounting.
    pub completeness: Completeness,
}

impl CommentSet {
    /// Number of items.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the thread is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// Extract a thread from a group.
#[must_use]
pub fn extract(arena: &Arena, group: &Group, max_items: u32) -> CommentSet {
    let mut set = CommentSet::default();
    let mut hit_limit = false;

    for (idx, &m) in group.members.iter().enumerate() {
        if idx as u32 >= max_items {
            hit_limit = true;
            break;
        }
        set.items.push(item_of(arena, m));
    }

    let (depths, source) = resolve_depths(arena, group, set.items.len());
    for (i, d) in depths.iter().enumerate() {
        if let Some(it) = set.items.get_mut(i) {
            it.depth = *d;
        }
    }
    set.depth_source = Some(source);
    link_parents(&mut set);

    set.completeness.present = set.items.len() as u32;
    if hit_limit {
        set.completeness.truncated = true;
        set.completeness.reason = Some(TruncationReason::LimitHit);
    }
    set
}

fn item_of(arena: &Arena, m: NodeId) -> CommentItem {
    let end = arena.subtree_end.get(m.idx()).copied().unwrap_or(0) as usize;

    let mut author = None;
    let mut timestamp = None;
    let mut permalink = None;
    // The nodes the author and timestamp were read *from*, so their text can be left out of the
    // body. They are already returned as their own fields, and repeating them as a prefix on every
    // item ("hshim 2026-07-24 08:59:37 \u{c800}\u{b294}...") makes the text unusable for anything that
    // measures or displays it.
    let mut author_node: Option<NodeId> = None;
    let mut time_node: Option<NodeId> = None;

    for i in m.idx()..end {
        let id = NodeId(i as u32);
        if author.is_none() && arena.tag.get(i).copied() == Some(TagId::A) {
            let is_author = arena.attr(id, AttrName::HREF).is_some_and(|h| {
                h.contains("/u/")
                    || h.contains("/user/")
                    || h.contains("/@")
                    || h.contains("/profile")
            }) || arena
                .attr(id, AttrName::CLASS)
                .is_some_and(|c| c.contains("author") || c.contains("user"));
            if is_author {
                let t = subtree_prose(arena, id);
                if !t.is_empty() {
                    author = Some(t);
                    author_node = Some(id);
                }
            }
        }
        if timestamp.is_none() {
            if let Some(dt) = arena.attr(id, AttrName::DATETIME) {
                timestamp = Some(dt.to_string());
                // A `datetime` attribute is usually on the <time> whose text is the human-readable
                // copy of the same instant, so that text is a duplicate too.
                if arena.tag.get(i).copied() == Some(TagId::TIME) {
                    time_node = Some(id);
                }
            } else if arena.tag.get(i).copied() == Some(TagId::TIME) {
                let t = subtree_prose(arena, id);
                if !t.is_empty() {
                    timestamp = Some(t);
                    time_node = Some(id);
                }
            }
        }
        if permalink.is_none() {
            if let Some(idv) = arena.attr(id, AttrName::ID) {
                if !idv.is_empty() {
                    permalink = Some(alloc::format!("#{idv}"));
                }
            }
        }
    }

    let body = content_child(arena, m, end, &[author_node, time_node]);
    // One exclusion set for both `text` and `html`, computed once.
    //
    // `text` used to exclude only the author and the timestamp while `html` excluded the whole
    // byline-and-furniture set, so the same item disagreed with itself: GeekNews puts a vote arrow
    // and a collapse toggle in the credit bar, and every comment's `text` opened with `▲ [-]`
    // while its `html` correctly did not. A consumer rendering `text` -- a search index, a
    // notification, a plain-text digest -- got the controls; one rendering `html` did not.
    let byline = byline_and_furniture(arena, m, end, body, &[author_node, time_node]);
    // Over the same node `html` is built from, with the same exclusions, so the two cannot
    // disagree about what the comment says.
    //
    // They did. `html` serializes `body` -- the content child, which on most templates is already
    // inside the credit bar's sibling -- while `text` spanned the whole item and skipped only the
    // author and the timestamp. GeekNews puts a vote arrow and a collapse toggle in that bar, so
    // every comment's `text` opened with `▲ [-]` and its `html` did not. A consumer rendering
    // `text` (a search index, a notification, a plain-text digest) got the controls.
    let text = subtree_prose_excluding(arena, body.unwrap_or(m), &byline);
    let lower_owned = text.to_lowercase();
    let lower = lower_owned.as_str();
    let flags = Flags {
        deleted: text.is_empty() || lower.contains("[deleted]") || lower.contains("[removed]"),
        dead: lower.contains("[dead]") || lower.contains("[flagged]"),
        collapsed: false,
        edited: lower.contains("edited") || lower.contains("수정됨"),
        op: false,
        accepted: false,
    };

    CommentItem {
        node: m,
        author,
        timestamp,
        body,
        byline,
        text,
        depth: 0,
        parent: None,
        permalink,
        score: None,
        kind: CommentKind::Reply,
        flags,
    }
}

/// The child of a comment holding its content rather than its byline or its controls.
///
/// The widest-prose direct child that contains neither the author nor the timestamp. Every template
/// in the corpus separates the two — `GeekNews` has `div.commentinfo` beside `div.commentTD`, Reddit a
/// meta slot beside a body slot — because the byline is laid out differently from the prose. Picking
/// by prose alone would also work on most pages and would pick the byline on a one-line reply, which
/// is exactly where it matters least and is wrong most visibly.
///
/// Returns `None` rather than guessing when no child qualifies.
fn content_child(
    arena: &Arena,
    item: NodeId,
    end: usize,
    meta: &[Option<NodeId>],
) -> Option<NodeId> {
    // Descend through single-child wrappers first. A member is often the row rather than the comment
    // -- `<li class="comment-tree-item"><article>…</article></li>` -- and looking only at the row's
    // own children finds one child that holds the byline, gives up, and falls back to the whole row.
    // The byline is then rendered twice: once as `author`/`timestamp`, once inside `html`.
    let mut item = item;
    let mut end = end;
    loop {
        let kids: alloc::vec::Vec<usize> = element_children_of(arena, item.idx(), end);
        match kids.as_slice() {
            [only] => {
                item = NodeId(*only as u32);
                end = (arena.subtree_end.get(*only).copied().unwrap_or(0) as usize).max(only + 1);
            }
            _ => break,
        }
    }

    // One content child means that child *is* the comment, and returning it drops the wrapper's own
    // furniture. Several means the content is spread across siblings, and the only node that holds
    // all of them is the container — the caller excludes the byline from it.
    let content: alloc::vec::Vec<usize> = element_children_of(arena, item.idx(), end)
        .into_iter()
        .filter(|&c| {
            let child_end = (arena.subtree_end.get(c).copied().unwrap_or(0) as usize).max(c + 1);
            let holds_meta = meta.iter().flatten().any(|n| n.idx() >= c && n.idx() < child_end);
            !holds_meta
                && arena.prose_len.get(c).copied().unwrap_or(0) > 0
                && !is_control_furniture(arena, NodeId(c as u32))
        })
        .collect();
    match content.as_slice() {
        [only] => Some(NodeId(*only as u32)),
        [] => None,
        _ => Some(item),
    }
}

/// Whether a child of a comment is a control strip rather than content.
///
/// A vote arrow and a fold toggle are `<a>` elements — `<a class="upvote">▲</a>`,
/// `<a href="javascript:child_toggle(1)"><span>[-]</span></a>` — so they are prose by role and their
/// text is otherwise indistinguishable from a one-word reply. What gives them away is that *all* of
/// their text sits inside links: nobody writes a comment that is entirely anchor text.
fn is_control_furniture(arena: &Arena, node: NodeId) -> bool {
    arena.link_density(node) > 0.9
}

/// Everything the serializer must leave out of a comment's body.
///
/// The author and timestamp nodes always, because they are returned as their own fields. Plus, when
/// the body is the whole comment rather than one content child, the control strips beside it — the
/// vote arrow and the fold toggle would otherwise open every rendered comment with `▲ [-]`.
fn byline_and_furniture(
    arena: &Arena,
    item: NodeId,
    end: usize,
    body: Option<NodeId>,
    meta: &[Option<NodeId>],
) -> Vec<NodeId> {
    let mut out: Vec<NodeId> = meta.iter().flatten().copied().collect();
    if body != Some(item) {
        return out;
    }
    for c in element_children_of(arena, item.idx(), end) {
        let id = NodeId(c as u32);
        if arena.prose_len.get(c).copied().unwrap_or(0) > 0 && is_control_furniture(arena, id) {
            out.push(id);
        }
    }
    out
}

/// Indices of an element's direct element children.
fn element_children_of(arena: &Arena, parent: usize, end: usize) -> alloc::vec::Vec<usize> {
    let mut out = alloc::vec::Vec::new();
    let mut c = parent + 1;
    while c < end {
        if arena.kind.get(c).copied() == Some(NodeKind::Element) {
            out.push(c);
        }
        c = (arena.subtree_end.get(c).copied().unwrap_or(0) as usize).max(c + 1);
    }
    out
}

/// Prose text of a subtree. Control, hidden and screen-reader text are already excluded by role, so
/// reaction buttons and icon glyphs never appear in a comment body.
fn subtree_prose(arena: &Arena, node: NodeId) -> String {
    let end = arena.subtree_end.get(node.idx()).copied().unwrap_or(0) as usize;
    let mut out = String::new();
    for i in node.idx()..end {
        if arena.kind.get(i).copied() != Some(NodeKind::Text) {
            continue;
        }
        if !arena.text_role.get(i).copied().is_some_and(crate::a11y::TextRole::is_prose) {
            continue;
        }
        for w in arena.own_text(NodeId(i as u32)).split_whitespace() {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(w);
        }
    }
    out
}

/// [`subtree_prose`] with the given subtrees left out.
///
/// Used to keep the author name and timestamp out of a comment's body text. They are returned as
/// their own fields, so including them again meant every item read
/// `"hshim 2026-07-24 08:59:37 저는 …"` — duplication that also skews any length or similarity
/// measure taken on the text, and the quote-duplicate detection planned for M9 is exactly such a
/// measure.
///
/// Only the nodes the values were actually read from are excluded, not a whole metadata container:
/// which element wraps the byline is a per-site question, whereas "the `<a>` I took the author from"
/// is not.
fn subtree_prose_excluding(arena: &Arena, node: NodeId, skip: &[NodeId]) -> String {
    let end = arena.subtree_end.get(node.idx()).copied().unwrap_or(0) as usize;
    let mut out = String::new();
    let mut i = node.idx();
    while i < end {
        // Jump the whole excluded subtree rather than filtering node by node, so nested markup
        // inside a byline (`<a><span>name</span></a>`) cannot leak a fragment through.
        if let Some(s) = skip.iter().find(|s| s.idx() == i) {
            i = arena.subtree_end.get(s.idx()).copied().unwrap_or(0) as usize;
            continue;
        }
        if arena.kind.get(i).copied() == Some(NodeKind::Text)
            && arena.text_role.get(i).copied().is_some_and(crate::a11y::TextRole::is_prose)
        {
            for w in arena.own_text(NodeId(i as u32)).split_whitespace() {
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(w);
            }
        }
        i += 1;
    }
    out
}

/// Depth per item, and which signal produced it.
///
/// Tried in order of trustworthiness. A CSS custom property or real DOM nesting is a statement by
/// the site; an indentation width is an inference from presentation; flat is an admission that
/// there was nothing to go on. `depth_source` is reported so a consumer can tell which it got.
fn resolve_depths(arena: &Arena, group: &Group, n: usize) -> (Vec<u16>, DepthSource) {
    let members = group.members.get(..n).unwrap_or(&group.members);

    // 1. A CSS variable naming a depth, e.g. style="--comment-depth:2".
    let mut from_css = Vec::with_capacity(members.len());
    let mut css_ok = true;
    for &m in members {
        if let Some(v) = css_depth(arena, m) {
            from_css.push(v);
        } else {
            css_ok = false;
            break;
        }
    }
    if css_ok && !from_css.is_empty() {
        return (from_css, DepthSource::CssVariable);
    }

    // 2. Nesting of members inside one another, in one linear pass with a stack of open ancestors.
    //
    // The obvious form asks, for each member, how many other members contain it — a members x
    // members product, and every comparison a `contains` call. On a 30,000-comment thread that was
    // 530 ms of a 559 ms pipeline. Members arrive in document order and containment is a
    // `subtree_end` range test, so an ancestor is simply an earlier member whose subtree has not
    // closed yet: keep those on a stack, pop the ones that have closed, and the depth is the stack
    // height. Same answer, O(members).
    let mut from_dom = Vec::with_capacity(members.len());
    let mut any_nested = false;
    let mut open: Vec<u32> = Vec::new();
    for &m in members {
        while open.last().is_some_and(|&e| m.0 >= e) {
            open.pop();
        }
        let d = open.len();
        if d > 0 {
            any_nested = true;
        }
        from_dom.push(u16::try_from(d).unwrap_or(u16::MAX));
        open.push(arena.subtree_end.get(m.idx()).copied().unwrap_or(m.0 + 1));
    }
    if any_nested {
        return (from_dom, DepthSource::DomNesting);
    }

    // 3. Indentation, with the unit derived from the observed values rather than assumed. A site
    //    using 40px steps and one using 1.5rem steps must both come out as 0,1,2.
    let raw: Vec<u32> = members.iter().map(|&m| indent_px(arena, m)).collect();
    if raw.iter().any(|&v| v > 0) {
        let unit = smallest_positive_gap(&raw).max(1);
        let depths = raw.iter().map(|&v| u16::try_from(v / unit).unwrap_or(u16::MAX)).collect();
        return (depths, DepthSource::Indentation);
    }

    (alloc::vec![0u16; members.len()], DepthSource::Flat)
}

/// Read a depth from a CSS custom property in the `style` attribute.
///
/// A site that writes `style="--comment-depth:2"` has stated the thread structure outright, which
/// is more reliable than inferring it from an indentation width.
fn css_depth(arena: &Arena, node: NodeId) -> Option<u16> {
    // Searches the subtree, not just the member. Sites commonly put the variable on an inner
    // wrapper -- beebs writes `<li class=comment-tree-item><article style="--comment-depth:2">` --
    // so checking only the member found nothing and every thread came back Flat.
    let end = arena.subtree_end.get(node.idx()).copied().unwrap_or(0) as usize;
    for i in node.idx()..end {
        if let Some(style) = arena.attr(NodeId(i as u32), AttrName::STYLE) {
            if let Some(d) = read_depth_var(style) {
                return Some(d);
            }
        }
    }
    None
}

/// Find `--<something>depth: N` in a style declaration.
fn read_depth_var(style: &str) -> Option<u16> {
    let lower = style.to_lowercase();
    let pos = lower.find("depth")?;
    // Must be a custom property, i.e. preceded by `--` somewhere before a `:`.
    let before = lower.get(..pos)?;
    if !before.contains("--") {
        return None;
    }
    let after = lower.get(pos..)?;
    let colon = after.find(':')?;
    let val = after.get(colon + 1..)?;
    let digits: String =
        val.chars().skip_while(|c| c.is_whitespace()).take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Indentation in pixels, from `margin-left` / `padding-left`, or a spacer image's width.
fn indent_px(arena: &Arena, node: NodeId) -> u32 {
    if let Some(px) = arena.attr(node, AttrName::STYLE).and_then(read_left_offset) {
        return px;
    }
    let end = arena.subtree_end.get(node.idx()).copied().unwrap_or(0) as usize;
    for i in node.idx()..end {
        let id = NodeId(i as u32);
        // Hacker News encodes depth as `<td indent="N">`.
        if let Some(n) = arena.attr(id, AttrName::INDENT).and_then(|v| v.trim().parse::<u32>().ok())
        {
            return n;
        }
        // Old forums and archived HN use a spacer image whose width encodes depth.
        if arena.tag.get(i).copied() == Some(TagId::IMG) {
            if let Some(w) =
                arena.attr(id, AttrName::WIDTH).and_then(|v| v.trim().parse::<u32>().ok())
            {
                if w > 0 {
                    return w;
                }
            }
        }
        // A nested member's own style still counts.
        if let Some(px) = arena.attr(id, AttrName::STYLE).and_then(read_left_offset) {
            return px;
        }
    }
    0
}

fn read_left_offset(style: &str) -> Option<u32> {
    let lower = style.to_lowercase();
    for key in ["margin-left", "padding-left", "margin-inline-start"] {
        if let Some(p) = lower.find(key) {
            let rest = lower.get(p + key.len()..)?;
            let colon = rest.find(':')?;
            let val = rest.get(colon + 1..)?;
            let num: String = val
                .chars()
                .skip_while(|c| c.is_whitespace())
                .take_while(char::is_ascii_digit)
                .collect();
            if let Ok(n) = num.parse::<u32>() {
                if n > 0 {
                    return Some(n);
                }
            }
        }
    }
    None
}

/// Smallest non-zero difference between distinct values — the indentation step.
fn smallest_positive_gap(v: &[u32]) -> u32 {
    let mut best = u32::MAX;
    for (i, &a) in v.iter().enumerate() {
        for &b in v.iter().skip(i + 1) {
            let d = a.abs_diff(b);
            if d > 0 && d < best {
                best = d;
            }
        }
    }
    // Fall back to the smallest non-zero value: with a single distinct indent there is no gap to
    // measure, and that value *is* one step.
    if best == u32::MAX {
        v.iter().copied().filter(|&x| x > 0).min().unwrap_or(1)
    } else {
        best
    }
}

/// Assign each item the nearest preceding item of lower depth.
///
/// Iterative with an explicit stack of (depth, index). This is the standard way to rebuild a tree
/// from a depth sequence, and it is why storing depth rather than nesting loses nothing.
fn link_parents(set: &mut CommentSet) {
    let mut stack: Vec<(u16, u32)> = Vec::new();
    for i in 0..set.items.len() {
        let d = set.items.get(i).map_or(0, |it| it.depth);
        while stack.last().is_some_and(|&(sd, _)| sd >= d) {
            stack.pop();
        }
        let parent = stack.last().map(|&(_, pi)| pi);
        if let Some(it) = set.items.get_mut(i) {
            it.parent = parent;
        }
        stack.push((d, u32::try_from(i).unwrap_or(u32::MAX)));
    }
}

/// The comment total the page states about itself, if it states one.
///
/// Scans the whole document rather than guessing where the count sits — it is a heading on one
/// site, a tab label on another, a badge on a third. First match wins; no match means `None`.
///
/// Serves two purposes, and the second is load-bearing: [`Completeness::claimed_total`] compares it
/// against what we extracted, and [`Group::is_comment_thread`] uses it as a **second, independent**
/// source of evidence that a small set of look-alike siblings is a thread. See that method.
#[must_use]
pub fn claimed_total(arena: &Arena) -> Option<u32> {
    (0..arena.len())
        .filter(|&i| arena.kind.get(i).copied() == Some(NodeKind::Text))
        // Script and stylesheet source is `Inert`, and it is usually the largest text in the
        // document. Handing it to the parser below made this scan 72% of the whole pipeline on a
        // measured page — it cannot contain a rendered comment count by definition, since nothing
        // in it is rendered at all.
        .filter(|&i| arena.text_role.get(i).copied() != Some(crate::a11y::TextRole::Inert))
        .find_map(|i| parse_claimed_total(arena.own_text(NodeId(i as u32))))
}

/// Parse a claimed comment total from page text, e.g. "128 comments" or "댓글 3개".
///
/// Returns `None` when the page does not say. Estimating would defeat the purpose: the value
/// exists to be compared against what we extracted.
#[must_use]
pub fn parse_claimed_total(text: &str) -> Option<u32> {
    const MARKERS: [&str; 5] = ["comment", "댓글", "コメント", "评论", "replies"];
    // Cheap reject before the allocation. Only `comment` and `replies` are case-sensitive at all —
    // the CJK markers have no case — so a page with no ASCII `c`/`r` and none of the three CJK
    // markers cannot match, and that is almost every text node on almost every page.
    if !text.bytes().any(|b| matches!(b, b'c' | b'C' | b'r' | b'R'))
        && !MARKERS[1..4].iter().any(|m| text.contains(m))
    {
        return None;
    }
    let lower = text.to_lowercase();
    for marker in MARKERS {
        let mut from = 0usize;
        while let Some(rel) = lower.get(from..).and_then(|s| s.find(marker)) {
            let at = from + rel;
            // Look backwards for digits immediately before the marker.
            let before = lower.get(..at).unwrap_or("");
            let digits: String = before
                .chars()
                .rev()
                .skip_while(|c| c.is_whitespace())
                .take_while(char::is_ascii_digit)
                .collect();
            if !digits.is_empty() {
                let fwd: String = digits.chars().rev().collect();
                if let Ok(n) = fwd.parse::<u32>() {
                    return Some(n);
                }
            }
            // Or forwards, for "댓글 3개".
            let after = lower.get(at + marker.len()..).unwrap_or("");
            let d2: String = after
                .chars()
                .skip_while(|c| c.is_whitespace())
                .take_while(char::is_ascii_digit)
                .collect();
            if !d2.is_empty() {
                if let Ok(n) = d2.parse::<u32>() {
                    return Some(n);
                }
            }
            from = at + marker.len();
        }
    }
    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn depth_variable_is_read_only_from_a_custom_property() {
        assert_eq!(read_depth_var("--comment-depth:2"), Some(2));
        assert_eq!(read_depth_var("--depth: 10; color:red"), Some(10));
        assert_eq!(read_depth_var("color:red;--comment-depth:0"), Some(0));
        // Not a custom property: an ordinary declaration that merely contains the word.
        assert_eq!(read_depth_var("z-depth:3"), None);
        assert_eq!(read_depth_var("color:red"), None);
    }

    #[test]
    fn indentation_unit_is_derived_not_assumed() {
        // A site stepping by 40px and one stepping by 15px must both yield 0,1,2.
        assert_eq!(smallest_positive_gap(&[0, 40, 80, 120]), 40);
        assert_eq!(smallest_positive_gap(&[0, 15, 30]), 15);
        // One distinct indent: that value is the step.
        assert_eq!(smallest_positive_gap(&[0, 0, 25]), 25);
        assert_eq!(smallest_positive_gap(&[0, 0, 0]), 1);
    }

    #[test]
    fn left_offsets_are_read_from_the_usual_properties() {
        assert_eq!(read_left_offset("margin-left:40px"), Some(40));
        assert_eq!(read_left_offset("padding-left: 15px;"), Some(15));
        assert_eq!(read_left_offset("margin-inline-start:80px"), Some(80));
        assert_eq!(read_left_offset("margin-left:0"), None);
        assert_eq!(read_left_offset("color:red"), None);
    }

    #[test]
    fn parents_are_rebuilt_from_a_depth_sequence() {
        let mut set = CommentSet::default();
        for d in [0u16, 1, 2, 1, 0, 1] {
            set.items.push(CommentItem {
                byline: Vec::new(),
                body: None,
                node: NodeId(0),
                author: None,
                timestamp: None,
                text: String::new(),
                depth: d,
                parent: None,
                permalink: None,
                score: None,
                kind: CommentKind::Reply,
                flags: Flags::default(),
            });
        }
        link_parents(&mut set);
        let parents: Vec<Option<u32>> = set.items.iter().map(|i| i.parent).collect();
        assert_eq!(parents, alloc::vec![None, Some(0), Some(1), Some(0), None, Some(4)]);
    }

    #[test]
    fn claimed_total_is_read_but_never_estimated() {
        assert_eq!(parse_claimed_total("128 comments"), Some(128));
        assert_eq!(parse_claimed_total("3 replies"), Some(3));
        assert_eq!(parse_claimed_total("댓글 12개"), Some(12));
        assert_eq!(parse_claimed_total("18 댓글"), Some(18));
        // No number stated: say nothing rather than guess.
        assert_eq!(parse_claimed_total("no comments (yet)"), None);
        assert_eq!(parse_claimed_total("comments"), None);
        assert_eq!(parse_claimed_total(""), None);
    }

    #[test]
    fn a_deleted_item_is_kept_so_the_parent_chain_survives() {
        // If a deleted comment were dropped, every reply beneath it would lose its parent and the
        // thread could not be reassembled.
        let mut set = CommentSet::default();
        for (d, t) in [(0u16, "top"), (1, ""), (2, "reply to deleted")] {
            set.items.push(CommentItem {
                byline: Vec::new(),
                body: None,
                node: NodeId(0),
                author: None,
                timestamp: None,
                text: t.into(),
                depth: d,
                parent: None,
                permalink: None,
                score: None,
                kind: CommentKind::Reply,
                flags: Flags { deleted: t.is_empty(), ..Flags::default() },
            });
        }
        link_parents(&mut set);
        assert_eq!(set.items[2].parent, Some(1), "reply must still point at the deleted node");
        assert!(set.items[1].flags.deleted);
    }
}
