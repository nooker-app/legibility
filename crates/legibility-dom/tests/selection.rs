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

/// Full pipeline: the region, its text, and the comments.
fn extract(html: &str) -> (String, String, usize) {
    let (arena, _) = BuildArena::parse_to_arena(html, Limits::DEFAULT);
    let out = legibility_core::extract_all(&arena, Limits::DEFAULT);
    let tag = out
        .selection
        .article
        .and_then(|n| arena.tag_name(n))
        .unwrap_or("none")
        .to_string();
    let text = out.selection.article.map_or(String::new(), |n| {
        legibility_dom::json::prose_text_excluding(&arena, n, &out.article_exclusions)
    });
    (tag, text, out.comments.items.len())
}

/// A GeekNews/BeeBS thread: a short submission body, then eighteen comments, each its own
/// `<article>` — nineteen `<article>` elements on the page in total.
fn discussion_shaped(comments: usize) -> String {
    let mut s = String::from(
        "<html><body><main class=\"thread-view\">\
         <article class=\"thread-detail\">\
           <header><h1>다들 체력관리는 어떻게 하시나요?</h1>\
             <div><span>hshim</span><span>조회 448</span><time datetime=\"2026-07-23T23:57:24Z\">\
             2026-07-24 08:57:24</time></div></header>\
           <div class=\"markdown-body thread-body\"><p>어떤 운동을 얼만큼 하시는지 궁금합니다</p></div>\
         </article>\
         <section class=\"comment-section\"><ol>",
    );
    for i in 0..comments {
        s.push_str(&format!(
            "<li class=\"comment-tree-item\"><article id=\"comment-{i}\" class=\"comment-row\" \
             style=\"--comment-depth:0\">\
             <div class=\"comment-meta\"><a class=\"author-profile-link\" href=\"/u/u{i}\">사용자{i}</a>\
             <time datetime=\"2026-07-24T0{}:00:00Z\">2026-07-24 0{}:00:00</time></div>\
             <div class=\"markdown-body\"><p>답변 {i} 입니다. 이 댓글은 본문보다 훨씬 길고 밀도도 높아서 \
             통계만으로는 본문을 이깁니다. 실제로 그런 일이 일어났습니다.</p></div>\
             </article></li>",
            i % 9,
            i % 9,
        ));
    }
    s.push_str("</ol></section></main></body></html>");
    s
}

#[test]
fn the_submission_body_wins_over_eighteen_longer_comments() {
    // The comments outweigh the body many times over, so no statistic can find it. `<article
    // class="thread-detail">` can -- but only once the comment `<article>`s stop counting as
    // semantic anchors themselves, which is what masking their whole subtrees achieves.
    let (tag, text, n) = extract(&discussion_shaped(18));
    assert_eq!(tag, "article");
    assert!(text.contains("어떤 운동을 얼만큼"), "submission body missing from: {text}");
    assert!(!text.contains("답변 3"), "comment text leaked into the body: {text}");
    assert_eq!(n, 18);
}

#[test]
fn masking_covers_the_inside_of_a_comment_not_just_its_container() {
    // The bug this pins: masking marked the matched container and rolled the total up to its
    // ancestors, leaving each comment's inner `<div class="markdown-body">` a full-strength
    // candidate. One of them then won, and the page came back with a reply as its article.
    let (_, text, _) = extract(&discussion_shaped(18));
    for i in 0..18 {
        assert!(!text.contains(&format!("답변 {i} ")), "comment {i} survived masking");
    }
}

#[test]
fn a_comment_body_does_not_repeat_its_own_author_and_timestamp() {
    // Both are returned as their own fields. Repeating them as a prefix on every item also skews
    // any length or similarity measure taken on the text.
    let html = discussion_shaped(4);
    let (arena, _) = BuildArena::parse_to_arena(&html, Limits::DEFAULT);
    let out = legibility_core::extract_all(&arena, Limits::DEFAULT);
    let first = out.comments.items.first().expect("a comment");
    assert_eq!(first.author.as_deref(), Some("사용자0"));
    assert!(first.text.starts_with("답변 0"), "text starts with metadata: {:?}", first.text);
    assert!(!first.text.contains("사용자0"), "author repeated in text: {:?}", first.text);
    assert!(!first.text.contains("2026-07-24 0"), "timestamp repeated: {:?}", first.text);
}

#[test]
fn an_empty_comment_section_is_not_body_text() {
    // A post with no replies still carries the empty state and the composer inside the <main> that
    // wins, and there is nothing to mask because there are no comments. Reddit's shape exactly.
    let html = "<html><body><main>\
        <h1>A post with no replies yet</h1>\
        <div><p>The body of the post, which is what a reader came for and what should be \
        returned here rather than the furniture around it.</p></div>\
        <shreddit-comment-tree id=\"comment-tree\"><section>\
          <p>Be the first to comment</p>\
          <p>Nobody's responded to this post yet. Add your thoughts and get the conversation \
          going, because this block is longer and denser than the post itself.</p>\
        </section></shreddit-comment-tree>\
        </main></body></html>";
    let (_, text, n) = extract(html);
    assert_eq!(n, 0, "there are no comments to find");
    assert!(text.contains("what a reader came for"), "the post body is missing: {text}");
    assert!(!text.contains("Be the first to comment"), "empty comment state in body: {text}");
    assert!(!text.contains("Nobody's responded"), "empty comment state in body: {text}");
}

#[test]
fn a_comment_shaped_class_on_article_prose_is_not_removed_wholesale() {
    // The guard. `class="comment-policy"` on an article *about* moderation must not take the
    // article with it -- and if a match would remove more than half the region, nothing goes.
    let html = "<html><body><main><div class=\"comments\">\
        <h1>Our comment policy</h1>\
        <p>This page is entirely about how comments are moderated, so the only container that \
        matches the comment-section rule is also the whole article.</p>\
        <p>Removing it would leave a reader with nothing at all, which is worse than leaving \
        some furniture behind.</p></div></main></body></html>";
    let (_, text, _) = extract(html);
    assert!(text.contains("entirely about how comments"), "the guard did not hold: {text}");
}
