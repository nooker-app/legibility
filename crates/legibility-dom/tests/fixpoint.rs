//! Repeated sanitization must settle within the two rounds the contract allows (plan M1).
//!
//! # Why this is a property of the *serializer*, not of the sanitizer's rules
//!
//! Sanitizing means parse, then serialize. The output only settles if the tree we write down is a
//! tree that parsing can produce — otherwise the next parse builds something else, and the round
//! after that writes something else again. Everything here is a shape our arena legitimately holds
//! and HTML cannot express.

use legibility_core::{Limits, NodeId, NodeKind, TagId};
use legibility_dom::serialize::{serialize_region, SerializeOptions};
use legibility_dom::BuildArena;
use legibility_sanitize::{Article, Profile, UserContent};

fn once<P: Profile>(html: &str) -> String {
    let (arena, _) = BuildArena::parse_to_arena(html, Limits::BROWSER);
    let body = (0..arena.len())
        .find(|&i| {
            arena.kind.get(i).copied() == Some(NodeKind::Element)
                && arena.tag.get(i).copied() == Some(TagId::BODY)
        })
        .expect("every document has a body");
    let (out, _) = serialize_region::<P>(&arena, NodeId(body as u32), SerializeOptions::default());
    out.as_str().to_string()
}

/// `(rounds to settle, settled output)`, giving up at six.
fn settle<P: Profile>(html: &str) -> (usize, String) {
    let mut cur = html.to_string();
    for round in 1..=6 {
        let next = once::<P>(&cur);
        if next == cur {
            return (round, next);
        }
        cur = next;
    }
    (usize::MAX, cur)
}

/// A heading inside a heading is unwritable, so it must never be written.
///
/// Tree construction pops an open `h1`-`h6` when another heading starts, so `<h5><h5>` always comes
/// back as siblings — but the adoption agency algorithm *creates* that nesting in our arena when it
/// reparents around a formatting element. Serializing it faithfully cost one round every time:
/// once and the nesting is there, twice and the parser has split it, three times to settle.
///
/// Both of these are reduced from real nightly fuzz failures — `sanitize_roundtrip_ugc` and
/// `sanitize_roundtrip_article`, which failed every night from 2026-08-07. The two inputs looked
/// nothing alike and shared only this.
#[test]
fn a_heading_inside_a_heading_settles_within_two_rounds() {
    let ugc = "<h5>a<a>b<h5>c<a>d</a></h5><table></table></a></h5>";
    let (rounds, out) = settle::<UserContent>(ugc);
    assert!(rounds <= 2, "took {rounds} rounds to settle: {out}");
    assert!(out.contains('c') && out.contains('d'), "text was lost: {out}");

    let article = "<h3><a>x</a><h3><a>y</a></h3></h3>";
    let (rounds, out) = settle::<Article>(article);
    assert!(rounds <= 2, "took {rounds} rounds to settle: {out}");
    assert!(out.contains('x') && out.contains('y'), "text was lost: {out}");
}

/// The narrow case only. A heading nested inside a heading *via another element* is writable and
/// stable, and must keep its tag — `<h5>a<a>b<h5>c</h5></a></h5>` round-trips unchanged, because a
/// heading start tag only pops an open heading when that heading is the current node.
///
/// This is the half that a blunter rule gets wrong: counting open headings instead of asking what
/// the immediate parent is deletes a heading the page can legitimately contain.
#[test]
fn a_heading_below_some_other_element_keeps_its_tag() {
    let html = "<h5>a<a>b<h5>c</h5></a></h5>";
    let (rounds, out) = settle::<UserContent>(html);
    assert!(rounds <= 2, "took {rounds} rounds to settle: {out}");
    assert!(out.contains("<h5>c</h5>"), "the inner heading lost its tag: {out}");
}

/// Every input a nightly fuzz run has ever rejected, replayed.
///
/// The bytes are the fuzzer's own minimized cases, not hand-written markup, and that is the point:
/// each of these took real fuzzing to reach and none of them survives being simplified into
/// something readable. `<ul><a>x<a>y</a></a></ul>` looks like `ugc-anchor-in-anchor` and proves
/// nothing, because the first parse splits it and the arena never holds the shape at all.
///
/// Not valid UTF-8 and not meant to be — they arrive as bytes, which is how the module is called.
///
/// Counted from the *first* sanitization, exactly as the fuzz target does. Round zero is the raw
/// input, which is arbitrary tag soup and is allowed to move once: whitespace before `<html>` is
/// dropped during tree construction, so a document beginning with a tab emits it once and never
/// again. The contract is that sanitized output is stable, not that unparsed bytes are.
#[test]
fn every_minimized_fuzz_case_settles_within_two_rounds() {
    for (name, bytes) in [
        (
            "ugc-heading-in-heading",
            &include_bytes!("../../../tests/regressions/ugc-heading-in-heading.bin")[..],
        ),
        (
            "ugc-anchor-in-anchor",
            &include_bytes!("../../../tests/regressions/ugc-anchor-in-anchor.bin")[..],
        ),
    ] {
        let first = once::<UserContent>(&String::from_utf8_lossy(bytes));
        let (rounds, out) = settle::<UserContent>(&first);
        assert!(rounds <= 2, "{name} took {rounds} rounds to settle:\n{out}");
    }

    let bytes = &include_bytes!("../../../tests/regressions/article-heading-in-heading.bin")[..];
    let first = once::<Article>(&String::from_utf8_lossy(bytes));
    let (rounds, out) = settle::<Article>(&first);
    assert!(rounds <= 2, "article-heading-in-heading took {rounds} rounds to settle:\n{out}");
}

/// Two links in two table cells are not nested, whatever the ancestor walk says.
///
/// A marker goes on the list of active formatting elements when the parser enters `td`, `th` or
/// `caption`, so formatting does not reach across one. Without that bound the ancestor search would
/// strip the second link on any table of links.
#[test]
fn links_in_separate_table_cells_both_survive() {
    let html = "<table><tr><td><a href=\"/one\">one</a></td>\
        <td><a href=\"/two\">two</a></td></tr></table>";
    let (rounds, out) = settle::<UserContent>(html);
    assert!(rounds <= 2, "took {rounds} rounds to settle: {out}");
    assert_eq!(out.matches("<a ").count(), 2, "a link was unwrapped: {out}");
}

/// A `<pre>` eats a newline immediately after its start tag on every parse, so emitting the content
/// verbatim loses one per round and it never settles. Regression-guarded here because it is the
/// other member of this family and was found the same way.
#[test]
fn a_pre_block_keeps_its_leading_newlines() {
    let html = "<pre>\n\n\nkept</pre>";
    let (rounds, out) = settle::<Article>(html);
    assert!(rounds <= 2, "took {rounds} rounds to settle: {out:?}");
    let (_, twice) = settle::<Article>(&out);
    assert_eq!(twice, out, "a second pass moved it again");
}
