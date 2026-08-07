//! Arena subtree → sanitized HTML.
//!
//! Lives here rather than in `legibility-core` because it needs element *names*, and names are
//! html5ever's `QualName` — which core deliberately cannot see. Core keeps interned `TagId`s.
//!
//! Sanitization is not a pass over the output. It happens **while** serializing, so there is no
//! window in which unsanitized markup exists as a string that something could accidentally use.
//! That ordering also gives the reparse-fixpoint property its meaning: what we emit is what the
//! policy allowed, not what a filter later removed.

use legibility_core::a11y::TextRole;
use legibility_core::{Arena, NodeId, NodeKind};
use legibility_sanitize::{
    escape_attr, escape_text, filter_code_class, is_void, wrap, Profile, SanitizedHtml,
};

/// Options affecting what reaches the output.
#[derive(Debug, Clone, Copy)]
pub struct SerializeOptions {
    /// Emit screen-reader-only text, marked with `data-lg-sr-only`.
    ///
    /// Off by default for a reader view, but the text is never *deleted* — see
    /// [`TextRole::AltOnly`] for why suppressing assistive-technology content while citing
    /// accessibility as the reason would be incoherent.
    pub include_sr_only: bool,
    /// Nesting depth beyond which output is flattened, reported as `depth_clamped`.
    ///
    /// A ten-thousand-deep comment thread will hang a consumer's renderer long before it
    /// troubles us, so the cap protects them, not us.
    pub max_render_depth: u16,
    /// Byte cap. Truncation happens at an **element boundary**: cutting mid-tag would emit
    /// markup that reparses differently, which is a sanitizer bypass in waiting.
    pub max_output_bytes: u32,
}

impl Default for SerializeOptions {
    fn default() -> Self {
        Self { include_sr_only: false, max_render_depth: 64, max_output_bytes: 16 * 1024 * 1024 }
    }
}

/// What was dropped or changed while serializing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SerializeReport {
    /// Output hit [`SerializeOptions::max_output_bytes`].
    pub truncated: bool,
    /// At least one subtree was flattened at [`SerializeOptions::max_render_depth`].
    pub depth_clamped: bool,
    /// Count of elements whose tag was not on the allowlist and were unwrapped.
    pub unwrapped_elements: u32,
    /// Count of subtrees discarded whole (`<script>`, `<form>`, …).
    pub dropped_subtrees: u32,
    /// Count of URL-valued attributes rejected for their scheme.
    pub rejected_urls: u32,
}

/// Serialize `region`'s subtree under profile `P`.
///
/// Iterative with an explicit stack: recursion here would be a remote stack-overflow primitive,
/// since nesting depth comes straight from the input.
#[must_use]
pub fn serialize_region<P: Profile>(
    arena: &Arena,
    region: NodeId,
    opts: SerializeOptions,
) -> (SanitizedHtml<P>, SerializeReport) {
    serialize_region_excluding(arena, region, opts, &[])
}

/// [`serialize_region`] with subtrees that must not appear in the output.
///
/// The article region can *contain* the comment thread — Reddit's `<main>` holds the post, the
/// composer and the whole discussion — and scoring alone cannot prevent that: masking keeps comment
/// prose out of the *statistics*, but the region that wins is still an ancestor of the thread, and
/// walking it emits every comment. Comments in `article.html` is the failure the plan makes a
/// hard-fail gate, so the region walk is told which subtrees to skip rather than left to rediscover
/// them from class names.
#[must_use]
pub fn serialize_region_excluding<P: Profile>(
    arena: &Arena,
    region: NodeId,
    opts: SerializeOptions,
    exclude: &[NodeId],
) -> (SanitizedHtml<P>, SerializeReport) {
    let mut out = String::new();
    let mut report = SerializeReport::default();
    let end = arena.subtree_end.get(region.idx()).copied().unwrap_or(0) as usize;
    let start = region.idx();

    // (arena index, close-tag to emit on exit)
    // `Close` borrows the element name from the arena rather than owning it. A `&'static str`
    // would have been enough while only the interned table was consulted, and that restriction is
    // precisely what silently unwrapped every dynamically interned element.
    enum Step<'a> {
        Open(usize),
        Close(&'a str),
    }
    let mut stack: Vec<Step<'_>> = vec![Step::Open(start)];
    // Depth of currently open emitted elements, for the render cap.
    let mut open_depth: u16 = 0;

    while let Some(step) = stack.pop() {
        if out.len() as u32 >= opts.max_output_bytes {
            report.truncated = true;
            // Close what is still open before leaving, innermost first — the stack already holds the
            // pending `Close` steps in exactly that order. Breaking outright dropped the step just
            // popped and abandoned the rest, so a truncated article ended mid-element with no
            // `</div>`, `</article>` or `</p>`: the fragment did not reparse to the tree it was a
            // prefix of, and a consumer inserting it into a page would have its own markup adopted
            // into the hole.
            //
            // These closes may carry the output a few bytes past the cap. That is the right trade:
            // the cap exists to bound memory, and well-formedness is what the bytes are *for*.
            let mut pending = Some(step);
            while let Some(s) = pending.take().or_else(|| stack.pop()) {
                if let Step::Close(t) = s {
                    out.push_str("</");
                    out.push_str(t);
                    out.push('>');
                }
            }
            break;
        }
        match step {
            Step::Close(t) => {
                out.push_str("</");
                out.push_str(t);
                out.push('>');
                open_depth = open_depth.saturating_sub(1);
            }
            Step::Open(i) => {
                if i >= end {
                    continue;
                }
                let kind = arena.kind.get(i).copied().unwrap_or(NodeKind::Text);
                let role = arena.text_role.get(i).copied().unwrap_or(TextRole::Prose);

                match kind {
                    NodeKind::Text => {
                        match role {
                            TextRole::Prose => out.push_str(&escape_text(arena.own_text(NodeId(i as u32)))),
                            TextRole::AltOnly if opts.include_sr_only => {
                                out.push_str("<span data-lg-sr-only=\"\">");
                                out.push_str(&escape_text(arena.own_text(NodeId(i as u32))));
                                out.push_str("</span>");
                            }
                            // Control, Hidden, Inert, and AltOnly-when-disabled contribute
                            // nothing.
                            _ => {}
                        }
                    }
                    NodeKind::Element => {
                        // `tag_name`, not `TagId::known_name`. The interned table holds only the
                        // elements the *scorer* needs to compare as integers; everything else is
                        // interned dynamically and `known_name` returns `None` for it. Reading the
                        // narrower one gave an empty name to `em`, `strong`, `b`, `i`, `mark`,
                        // `abbr`, `sub`, `sup` and `figure`, so all of them were unwrapped out of
                        // every article. It looked correct because the same path is what unwraps
                        // custom elements, which genuinely should go, and because emphasis is
                        // invisible to a text-token F1 score.
                        let name = arena.tag_name(NodeId(i as u32)).unwrap_or("");

                        // Excluded subtrees (comments, when the region encloses them) go first:
                        // they are a caller's decision about content, not a property of the node,
                        // so no later rule can be relied on to catch them.
                        if i != start && exclude.iter().any(|n| n.idx() == i) {
                            report.dropped_subtrees = report.dropped_subtrees.saturating_add(1);
                            continue;
                        }
                        // Hidden and inert subtrees never reach output at all. Inert is the
                        // stronger case of the two: a `<script>` body must not be emitted even
                        // as escaped text, and the sanitizer's raw-text rule depends on it.
                        if role == TextRole::Hidden || role == TextRole::Inert {
                            report.dropped_subtrees = report.dropped_subtrees.saturating_add(1);
                            continue;
                        }
                        // Control subtrees are what the a11y work exists to remove: button
                        // labels, icon ligatures, toolbars.
                        if role == TextRole::Control {
                            report.dropped_subtrees = report.dropped_subtrees.saturating_add(1);
                            continue;
                        }
                        if !name.is_empty() && legibility_sanitize::drops_subtree_for::<P>(name) {
                            report.dropped_subtrees = report.dropped_subtrees.saturating_add(1);
                            continue;
                        }

                        let kids = child_range(arena, i, end);
                        let emit = !name.is_empty()
                            && legibility_sanitize::is_allowed_element(name)
                            && open_depth < opts.max_render_depth;

                        if !emit {
                            if open_depth >= opts.max_render_depth {
                                report.depth_clamped = true;
                            } else if !name.is_empty() {
                                report.unwrapped_elements =
                                    report.unwrapped_elements.saturating_add(1);
                            }
                            // Unwrap: keep the children, lose the tag.
                            for k in kids.into_iter().rev() {
                                stack.push(Step::Open(k));
                            }
                            continue;
                        }

                        out.push('<');
                        out.push_str(name);
                        write_attrs::<P>(arena, i, name, &mut out, &mut report);
                        out.push('>');

                        if is_void(name) {
                            continue;
                        }
                        open_depth = open_depth.saturating_add(1);
                        stack.push(Step::Close(name));
                        for k in kids.into_iter().rev() {
                            stack.push(Step::Open(k));
                        }
                    }
                    // Comments, doctype, PIs and orphans never render.
                    _ => {}
                }
            }
        }
    }

    // Any elements still open when the byte cap hit must be closed, or the output reparses as a
    // different tree than we serialized.
    if report.truncated {
        let mut pending: Vec<&str> = Vec::new();
        for s in &stack {
            if let Step::Close(t) = s {
                pending.push(t);
            }
        }
        for t in pending {
            out.push_str("</");
            out.push_str(t);
            out.push('>');
        }
    }

    collapse_empty_wrappers(&mut out, &mut report);
    collapse_blank_runs(&mut out);
    (wrap::<P>(out), report)
}

/// Delete elements that ended up holding nothing.
///
/// Every removal rule above works on one node: this text is `Control`, that subtree is `Hidden`,
/// this element is not on the allowlist. None of them can see that a `<div>` is left with nothing
/// inside once its children are gone — so a page built out of custom elements and slots, where
/// almost all the text is button labels, serializes as a skeleton of empty wrappers. A Reddit post
/// detail page produced ~90 nested empty `<div>`/`<span>` pairs and one paragraph.
///
/// Done on the finished string rather than in the walk, because "did this element contribute
/// anything?" is only answerable after its subtree has been emitted, and buffering per element to
/// find out would make the serializer's memory proportional to depth × output.
///
/// # What survives
///
/// Only a tag pair with *nothing but whitespace* between it goes. `<div><img src=…></div>` has
/// content between the tags and is kept — which is the case that makes the cheaper "subtree has
/// zero prose" test wrong, since an image-only figure has no prose and must not be dropped.
/// Table structure is exempt outright: an empty `<td>` is not noise, it is a column.
fn collapse_empty_wrappers(out: &mut String, report: &mut SerializeReport) {
    /// Removing one of these would change a table's shape, not just its clutter.
    const KEEP_EMPTY: [&str; 9] = [
        "td", "th", "tr", "table", "thead", "tbody", "tfoot", "col", "colgroup",
    ];

    // Nested empties need more than one sweep: `<div><span></span></div>` only becomes an empty
    // `<div>` after the `<span>` goes. Bounded so a pathological input cannot spin here.
    for _ in 0..MAX_COLLAPSE_PASSES {
        let mut changed = false;
        let mut result = String::with_capacity(out.len());
        let mut rest: &str = out;
        while let Some(lt) = rest.find('<') {
            let (before, from_lt) = rest.split_at(lt);
            result.push_str(before);
            let Some(gt) = from_lt.find('>') else {
                result.push_str(from_lt);
                rest = "";
                break;
            };
            let open = &from_lt[..=gt];
            let name: &str = open
                .trim_start_matches('<')
                .split([' ', '>', '/'])
                .next()
                .unwrap_or("");
            let after = &from_lt[gt + 1..];
            let close = format!("</{name}>");
            let empty = !name.is_empty()
                && !name.starts_with('/')
                && !KEEP_EMPTY.contains(&name)
                && after.trim_start().starts_with(close.as_str());
            if empty {
                // Drop both tags and the whitespace between them.
                rest = &after.trim_start()[close.len()..];
                report.dropped_subtrees = report.dropped_subtrees.saturating_add(1);
                changed = true;
            } else {
                result.push_str(open);
                rest = after;
            }
        }
        result.push_str(rest);
        *out = result;
        if !changed {
            break;
        }
    }
}

/// Enough to unwind the deepest wrapper nest seen on a real page, with room to spare. A cap rather
/// than a fixpoint loop so that adversarial input cannot make this quadratic.
const MAX_COLLAPSE_PASSES: usize = 32;

/// Reduce every whitespace run that spans a line break to a single newline.
///
/// Removing an element takes its tags and the whitespace *between* them, but not the indentation
/// that sat on either side — so each dropped wrapper leaves its own two-space-and-a-newline
/// behind, and a page built from deeply nested custom elements serializes as a few paragraphs
/// adrift in fifty blank lines. The output was correct and looked broken, which for a reader view
/// is the same thing.
///
/// A run *without* a line break is left exactly as it is. That is the whole safety argument: the
/// only whitespace that separates two inline elements on one line is a plain space, and collapsing
/// that could weld two words together. Anything spanning a newline is pretty-printer output, which
/// HTML itself already renders as a single space.
///
/// # Except inside `<pre>`
///
/// There the whitespace *is* the content. Running this over a code block reindents every line to
/// column zero and deletes the blank lines between functions:
///
/// ```text
///   def f(n):            def f(n):
///       if n < 2:   ->   if n < 2:
///           return n     return n
/// ```
///
/// which is not a formatting nit — it is the difference between runnable Python and not. `<pre>` is
/// already [`crate::TagId::is_opaque`] to cleaning for the same reason; it was not opaque here.
fn collapse_blank_runs(out: &mut String) {
    let mut result = String::with_capacity(out.len());
    let mut rest: &str = out;
    loop {
        // Copy verbatim up to the next `<pre`, collapse that stretch, then hand the `<pre>` element
        // through untouched. Nesting is counted rather than assumed: `<pre>` cannot legally contain
        // another, but the serializer must not depend on the page being legal.
        let cut = rest.find("<pre").unwrap_or(rest.len());
        let (before, after) = rest.split_at(cut);
        collapse_into(&mut result, before);
        if after.is_empty() {
            break;
        }
        let mut depth = 0usize;
        let mut at = 0usize;
        let end = loop {
            let Some(next) = after[at..].find('<') else { break after.len() };
            let i = at + next;
            if after[i..].starts_with("<pre") {
                depth += 1;
            } else if after[i..].starts_with("</pre") {
                depth -= 1;
                if depth == 0 {
                    // Include the close tag itself.
                    break after[i..].find('>').map_or(after.len(), |g| i + g + 1);
                }
            }
            at = i + 1;
        };
        result.push_str(&after[..end]);
        rest = &after[end..];
    }
    *out = result;
}

/// Collapse every newline-spanning whitespace run in `chunk` into a single newline.
fn collapse_into(result: &mut String, chunk: &str) {
    let mut rest = chunk;
    while let Some(pos) = rest.find('\n') {
        // Back up over the whitespace preceding the newline, forward over what follows it.
        let start = rest[..pos].trim_end_matches([' ', '\t', '\r']).len();
        let (head, tail) = rest.split_at(start);
        result.push_str(head);
        result.push('\n');
        rest = tail.trim_start_matches([' ', '\t', '\r', '\n']);
    }
    result.push_str(rest);
}

/// Direct children of `i` within `[i+1, end)`.
///
/// Uses `subtree_end` to skip whole subtrees, so this is O(children) rather than O(descendants).
fn child_range(arena: &Arena, i: usize, end: usize) -> Vec<usize> {
    let mut kids = Vec::new();
    let sub_end = arena.subtree_end.get(i).copied().unwrap_or(0) as usize;
    let limit = sub_end.min(end);
    let mut c = i + 1;
    while c < limit {
        kids.push(c);
        c = arena.subtree_end.get(c).copied().unwrap_or((c + 1) as u32) as usize;
    }
    kids
}

/// Emit the attributes the profile allows, from the arena's interned set.
///
/// This was a stub until the demo made its absence visible: an `<a>` with no `href` and an `<img>`
/// with no `src` render as dead text and a broken-image icon. The corpus gate could not see it,
/// because token-multiset F1 is computed on *text* and an attribute is not text.
///
/// Only interned names ([`AttrName`]) can be emitted — the arena does not keep a span for the name
/// of an attribute the engine never consults, so `data-*` and friends are structurally
/// unreachable here rather than deliberately filtered. That is a narrower output than the
/// allowlist describes, and it is the safe direction to be wrong in.
fn write_attrs<P: Profile>(
    arena: &Arena,
    node: usize,
    tag: &str,
    out: &mut String,
    report: &mut SerializeReport,
) {
    let mut wrote_rel = false;
    for a in arena.attrs_of(NodeId(node as u32)) {
        let name = a.name.as_str();
        if name.is_empty() || !legibility_sanitize::is_allowed_attr::<P>(tag, name) {
            continue;
        }
        let Some(value) = arena.attr(NodeId(node as u32), a.name) else {
            continue;
        };
        // A URL-valued attribute is the one place a string can become code, so the scheme check
        // happens here rather than being left to the consumer.
        let emitted = match name {
            "href" | "src" => match legibility_sanitize::check_url(value).url {
                Some(u) => u,
                None => {
                    report.rejected_urls = report.rejected_urls.saturating_add(1);
                    continue;
                }
            },
            // Syntax highlighting survives; `class` as a general channel does not.
            "class" if tag == "pre" || tag == "code" => match filter_code_class(value) {
                Some(c) => c,
                None => continue,
            },
            "class" => continue,
            // Namespaced so that a page-authored `id` cannot collide with the host document's,
            // which is how DOM clobbering starts.
            "id" => legibility_sanitize::namespace_id::<P>(value),
            "rel" if P::FORCED_REL.is_some() => continue,
            _ => value.to_string(),
        };
        if name == "rel" {
            wrote_rel = true;
        }
        out.push(' ');
        out.push_str(name);
        out.push_str("=\"");
        out.push_str(&escape_attr(&emitted));
        out.push('"');
    }
    // `rel` on user content is forced, not merely allowed: a comment must not be able to opt out
    // of `nofollow noopener noreferrer` by supplying its own.
    if tag == "a" && !wrote_rel {
        if let Some(rel) = P::FORCED_REL {
            out.push_str(" rel=\"");
            out.push_str(&escape_attr(rel));
            out.push('"');
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::BuildArena;
    use legibility_core::{Limits, TagId};
    use legibility_sanitize::{Article, UserContent};

    fn html_of<P: Profile>(src: &str) -> String {
        let (arena, _) = BuildArena::parse_to_arena(src, Limits::DEFAULT);
        let body = (0..arena.len())
            .find(|&i| arena.tag.get(i).copied() == Some(TagId::BODY))
            .unwrap();
        let (h, _) = serialize_region::<P>(&arena, NodeId(body as u32), SerializeOptions::default());
        h.into_inner()
    }

    #[test]
    fn script_and_style_never_appear_in_output() {
        let out = html_of::<Article>(
            "<html><body><p>keep</p><script>alert(1)</script><style>p{}</style></body></html>",
        );
        assert!(out.contains("keep"));
        assert!(!out.contains("alert"), "script content leaked: {out}");
        assert!(!out.contains("<script"), "script tag leaked: {out}");
        assert!(!out.contains("p{}"));
    }

    #[test]
    fn event_handlers_and_inline_styles_do_not_survive() {
        let out = html_of::<Article>(
            "<html><body><div onclick=\"steal()\" style=\"display:block\"><p>text</p></div></body></html>",
        );
        assert!(!out.contains("onclick"), "{out}");
        assert!(!out.contains("steal"), "{out}");
        assert!(!out.contains("style="), "{out}");
        assert!(out.contains("text"));
    }

    #[test]
    fn control_and_hidden_subtrees_are_absent_from_html_not_merely_from_text() {
        let out = html_of::<Article>(
            "<html><body><article><p>body</p>\
             <button><span>Copy</span></button>\
             <div hidden><p>promo</p></div></article></body></html>",
        );
        assert!(out.contains("body"));
        assert!(!out.contains("Copy"), "button label leaked into HTML: {out}");
        assert!(!out.contains("promo"), "hidden block leaked into HTML: {out}");
    }

    #[test]
    fn user_content_forces_rel_on_links_but_article_does_not() {
        let src = "<html><body><p><a href=\"https://x.test\">l</a></p></body></html>";
        let ugc = html_of::<UserContent>(src);
        assert!(ugc.contains("nofollow"), "UserContent must force rel: {ugc}");
        assert!(ugc.contains("noopener"));
        let art = html_of::<Article>(src);
        assert!(!art.contains("nofollow"), "Article must not force rel: {art}");
    }

    #[test]
    fn unknown_elements_are_unwrapped_and_keep_their_text() {
        let out = html_of::<Article>("<html><body><my-widget><p>inner</p></my-widget></body></html>");
        assert!(out.contains("inner"), "custom element content lost: {out}");
        assert!(!out.contains("my-widget"), "unknown tag emitted: {out}");
    }

    #[test]
    fn output_reparses_to_the_same_visible_text_fixpoint() {
        // The property that makes mXSS hard: serialize, reparse, serialize again, and the result
        // must stabilize rather than drift into something the policy would have rejected.
        let src = "<html><body><article><p>a &amp; b &lt;c&gt;</p><blockquote>q</blockquote></article></body></html>";
        let once = html_of::<Article>(src);
        let twice = html_of::<Article>(&format!("<html><body>{once}</body></html>"));
        let thrice = html_of::<Article>(&format!("<html><body>{twice}</body></html>"));
        assert_eq!(twice, thrice, "serialization is not a fixpoint:\n{twice}\n{thrice}");
        assert!(!twice.contains("<script"));
    }

    #[test]
    fn deep_nesting_is_clamped_and_reported_rather_than_emitted() {
        let deep = "<div>".repeat(500);
        let (arena, _) = BuildArena::parse_to_arena(
            &format!("<html><body>{deep}<p>x</p></body></html>"),
            Limits::DEFAULT,
        );
        let body = (0..arena.len())
            .find(|&i| arena.tag.get(i).copied() == Some(TagId::BODY))
            .unwrap();
        let (h, rep) = serialize_region::<Article>(
            &arena,
            NodeId(body as u32),
            SerializeOptions { max_render_depth: 16, ..SerializeOptions::default() },
        );
        assert!(rep.depth_clamped, "the render cap must be reported");
        assert!(h.as_str().contains('x'), "content past the cap must still appear");
    }

    #[test]
    fn truncation_closes_every_open_element() {
        let long = "<p>aaaaaaaaaa</p>".repeat(200);
        let (arena, _) = BuildArena::parse_to_arena(
            &format!("<html><body><article>{long}</article></body></html>"),
            Limits::DEFAULT,
        );
        let body = (0..arena.len())
            .find(|&i| arena.tag.get(i).copied() == Some(TagId::BODY))
            .unwrap();
        let (h, rep) = serialize_region::<Article>(
            &arena,
            NodeId(body as u32),
            SerializeOptions { max_output_bytes: 200, ..SerializeOptions::default() },
        );
        assert!(rep.truncated);
        let s = h.as_str();
        // Balanced tags, or a reparse yields a different tree than we serialized.
        assert_eq!(
            s.matches("<article").count(),
            s.matches("</article>").count(),
            "unbalanced after truncation: {s}"
        );
    }

    #[test]
    fn sr_only_is_omitted_by_default_and_recoverable_on_request() {
        let src = "<html><body><div><span class=sr-only>skip</span><p>real</p></div></body></html>";
        let (arena, _) = BuildArena::parse_to_arena(src, Limits::DEFAULT);
        let body = (0..arena.len())
            .find(|&i| arena.tag.get(i).copied() == Some(TagId::BODY))
            .unwrap();

        let (off, _) = serialize_region::<Article>(&arena, NodeId(body as u32), SerializeOptions::default());
        assert!(!off.as_str().contains("skip"), "sr-only must be off by default");

        let (on, _) = serialize_region::<Article>(
            &arena,
            NodeId(body as u32),
            SerializeOptions { include_sr_only: true, ..SerializeOptions::default() },
        );
        assert!(on.as_str().contains("skip"), "sr-only must be recoverable, not deleted");
        assert!(on.as_str().contains("data-lg-sr-only"));
    }
}
