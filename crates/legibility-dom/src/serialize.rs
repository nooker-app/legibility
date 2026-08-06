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
use legibility_core::{Arena, NodeId, NodeKind, TagId};
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
    let mut out = String::new();
    let mut report = SerializeReport::default();
    let end = arena.subtree_end.get(region.idx()).copied().unwrap_or(0) as usize;
    let start = region.idx();

    // (arena index, close-tag to emit on exit)
    enum Step {
        Open(usize),
        Close(&'static str),
    }
    let mut stack: Vec<Step> = vec![Step::Open(start)];
    // Depth of currently open emitted elements, for the render cap.
    let mut open_depth: u16 = 0;

    while let Some(step) = stack.pop() {
        if out.len() as u32 >= opts.max_output_bytes {
            report.truncated = true;
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
                        let tag = arena.tag.get(i).copied().unwrap_or(TagId::UNKNOWN);
                        let name = tag.known_name().unwrap_or("");

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
                        if !name.is_empty() && legibility_sanitize::drops_subtree(name) {
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

    (wrap::<P>(out), report)
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

fn write_attrs<P: Profile>(
    arena: &Arena,
    _node: usize,
    tag: &str,
    out: &mut String,
    _report: &mut SerializeReport,
) {
    // Attribute storage lands with the metadata subsystem in M3; until then the only attributes
    // emitted are the ones the profile *forces*, which is the security-relevant half. Emitting a
    // half-implemented attribute path would be worse than emitting none: it would look done.
    let _ = arena;
    if tag == "a" {
        if let Some(rel) = P::FORCED_REL {
            out.push_str(" rel=\"");
            out.push_str(&escape_attr(rel));
            out.push('"');
        }
    }
    if tag == "pre" || tag == "code" {
        // Placeholder for the language class, which needs attribute storage to exist.
        let _ = filter_code_class("");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::BuildArena;
    use legibility_core::Limits;
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
