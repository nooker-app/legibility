//! End-to-end region selection on shapes that broke, kept as the smallest reproductions.
//!
//! Each case here came from a real page and cost a diagnosis cycle. The fixtures are self-authored
//! (plan D9 forbids committing third-party HTML) but the structure is the site's.

use legibility_core::{Limits, TagId};
use legibility_dom::BuildArena;

/// Tag of the selected region, or `None`.
fn selected(html: &str) -> Option<&'static str> {
    let (arena, _) = BuildArena::parse_to_arena(html, Limits::DEFAULT);
    let sel = legibility_core::select_article(&arena);
    sel.article.and_then(|n| {
        arena
            .tag
            .get(n.idx())
            .copied()
            .unwrap_or(TagId::UNKNOWN)
            .known_name()
    })
}

/// A Reddit post: a title, a body that is one bare link, an inlined module script, and an
/// empty-comment-section banner that is short but very dense.
fn reddit_shaped(script_bytes: usize) -> String {
    let js = "function t(e){return e&&e.x?e.x:0}".repeat(script_bytes / 34 + 1);
    format!(
        "<html><body>\
         <main>\
           <div><a href=\"/r/rss/\">r/rss</a><a href=\"/user/x/\">someone</a></div>\
           <h1>Free news APIs differ in historical coverage, languages and search filters</h1>\
           <div><p>[<a href=\"https://example.test/repo\">https://example.test/repo</a>]</p></div>\
           <section><div>\
             <p>Be the first to comment</p>\
             <p>Nobody's responded to this post yet. Add your thoughts and get the conversation \
                going.</p>\
           </div></section>\
           <script>{js}</script>\
         </main>\
         <aside><p>A community for discussing feeds and syndication tooling of every kind.</p>\
         </aside>\
         </body></html>"
    )
}

#[test]
fn an_inlined_script_does_not_disqualify_the_container_holding_it() {
    // `purity` counted `<script>` bytes in its denominator, so <main> scored 0.04 with a 10 KB
    // bundle inside it and was thrown out by the floor. Reddit inlines exactly such a bundle.
    for bytes in [0usize, 1_000, 10_000, 200_000] {
        assert_eq!(
            selected(&reddit_shaped(bytes)),
            Some("main"),
            "a {bytes}-byte inline script changed which region was selected"
        );
    }
}

#[test]
fn the_empty_comment_banner_does_not_beat_the_post() {
    // Without the semantic-anchor rung, "Be the first to comment" won on density: 8.7% of the
    // page's prose at 2.7x the density of <main>, which is a 2% lead in evidence. Every site with
    // an empty comment section has this shape.
    let picked = selected(&reddit_shaped(10_000));
    assert_eq!(picked, Some("main"));
}

#[test]
fn a_nested_article_is_preferred_over_the_main_that_wraps_it() {
    // The narrower claim wins: <main> may hold the rail and the comment form too.
    let html = "<html><body><main>\
        <nav><a href=/a>one</a><a href=/b>two</a><a href=/c>three</a></nav>\
        <article><p>The first paragraph of the actual body, long enough to matter here.</p>\
        <p>A second paragraph so the article is not a single short block of text.</p></article>\
        <aside><p>Related links and other furniture that is not the body.</p></aside>\
        </main></body></html>";
    assert_eq!(selected(html), Some("article"));
}

#[test]
fn many_sibling_articles_are_ambiguous_so_the_statistics_decide() {
    // A listing of cards has one innermost anchor per card. Picking between them is how a front
    // page gets returned as a post, so the anchor rung must decline to choose.
    let card = "<article><h2><a href=/x>A story headline goes here</a></h2>\
                <p>Two sentences of standfirst text for this card in the list.</p></article>";
    let html = format!("<html><body><main>{}</main></body></html>", card.repeat(10));
    let (arena, _) = BuildArena::parse_to_arena(&html, Limits::DEFAULT);
    let sel = legibility_core::select_article(&arena);
    assert!(
        !sel.region_from_semantic_anchor,
        "ten equal anchors is not an anchor signal"
    );
}

#[test]
fn a_page_whose_only_dense_block_is_link_only_still_finds_its_prose() {
    // The viability floor used to be applied to the winner, so a link-only block that happened to
    // be the argmax reported the whole page as having no article.
    let html = "<html><body>\
        <div><p><a href=/1>x</a> <a href=/2>y</a> <a href=/3>z</a></p></div>\
        <div><p>Real prose that should be found even though the link block scores higher on \
        raw density than this paragraph does.</p></div>\
        </body></html>";
    assert!(selected(html).is_some(), "a link-dense argmax must not sink the page");
}
