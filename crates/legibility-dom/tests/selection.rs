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

/// A Reddit post detail page, reduced to the structure that decides the shape: a credit bar
/// *before* the title, the title, the flair slot, a body, and the comment tree.
///
/// `body` is the inner HTML of `<shreddit-post-text-body>`. That one parameter is the entire
/// difference between the two real posts this pair came from, so the two tests below can only
/// both pass if the rule reads the body rather than the site.
fn shreddit_post(body: &str, comments: &str) -> String {
    format!(
        "<html><body><main>\
         <shreddit-post permalink=\"/r/rss/comments/x/\" comment-count=\"0\">\
           <div slot=\"credit-bar\"><a href=\"/r/rss/\">r/rss</a>\
             <span>•</span><time datetime=\"2026-08-03T00:00:00Z\">2d ago</time>\
             <a href=\"/user/rangeva/\">rangeva</a></div>\
           <h1 slot=\"title\">Free news APIs differ in historical coverage and search filters</h1>\
           <shreddit-post-flair slot=\"post-flair\"></shreddit-post-flair>\
           <shreddit-post-text-body slot=\"text-body\">{body}</shreddit-post-text-body>\
           <div slot=\"action-row\"><button>Upvote</button><span>1</span>\
             <button>Downvote</button><button>Reply</button><button>Share</button></div>\
         </shreddit-post>\
         <shreddit-comment-tree id=\"comment-tree\">{comments}</shreddit-comment-tree>\
         </main></body></html>"
    )
}

/// `(kind, url, text)` as a consumer sees them.
///
/// Read out of the serialized JSON rather than off `Outcome`, because for a link submission the
/// emptying of `text` *is* the contract and it lives in the serializer. Field slicing is naive
/// on purpose — these fixtures contain no quotes or backslashes — and a hand-rolled reader here
/// is preferable to giving the test crate a JSON dependency the engine does not have.
fn extract_shape(html: &str) -> (String, String, String) {
    let (arena, hit) = BuildArena::parse_to_arena(html, Limits::DEFAULT);
    let out = legibility_core::extract_all(&arena, Limits::DEFAULT);
    let json = legibility_dom::json::extraction_json(&arena, &out, hit, None);
    let field = |key: &str| -> String {
        let pat = format!("\"{key}\":\"");
        match json.find(&pat) {
            Some(at) => {
                let rest = &json[at + pat.len()..];
                rest[..rest.find('"').unwrap_or(0)].to_string()
            }
            None => String::new(),
        }
    };
    (field("kind"), field("url"), field("text"))
}

#[test]
fn a_submission_whose_body_is_one_link_is_a_pointer_not_an_article() {
    // The reported defect. The body is a single anchor -- the poster pasted markdown wrongly, so
    // the brackets survive as prose -- and the region that won was the whole submission: credit
    // bar, title, URL and a stray score. Every byte of that is already a metadata field.
    //
    // The discriminator is not length. It is that this block's prose *is* its anchor text, which
    // is the scorer's own candidacy floor (`Candidate::is_viable`), so no character threshold
    // enters the decision.
    let html = shreddit_post(
        "<div class=\"md\"><p>[<a href=\"https://github.com/free-news-api/news-api\">\
         https://github.com/free-news-api/news-api</a>]</p></div>",
        "<section><p>Be the first to comment</p>\
         <p>Nobody's responded to this post yet. Add your thoughts and get the conversation \
         going, because this block is longer and denser than the submission itself.</p></section>",
    );
    let (kind, url, text) = extract_shape(&html);
    assert_eq!(kind, "discussion-root", "a pointer must not be served as an article");
    assert_eq!(url, "https://github.com/free-news-api/news-api");
    // No body at all, rather than a body made of the byline and the URL. Those are already
    // `metadata.byline` and `article.url`; returning them twice, as prose, is the defect.
    assert_eq!(text, "", "a pointer must have an empty body, got: {text}");
}

#[test]
fn a_submission_with_prose_keeps_its_prose_and_loses_its_furniture() {
    // Same site, same template, opposite shape -- and the same code has to reach the other
    // answer. Applying the link-only rule here would throw the body away; applying this one to
    // the post above would return the credit bar. That is why the pair is one test each.
    let html = shreddit_post(
        "<div class=\"md\"><p>I'm very new to RSS and just started setting up apps (Feeder). \
         I follow football closely and wanted to make a list, but I can't seem to add this \
         website for some reason. Any help would be great.</p>\
         <p><a href=\"https://www.sport.es/es/\">https://www.sport.es/es/</a></p>\
         <p>This is the website.</p></div>",
        "<shreddit-comment thingid=\"t1_a\" depth=\"0\"><div class=\"md\">\
         <p>Try this https://www.sport.es/es/rss/</p></div></shreddit-comment>\
         <shreddit-comment thingid=\"t1_b\" depth=\"1\" parentid=\"t1_a\"><div class=\"md\">\
         <p>Tried it, nothing happened. I am still stuck on this and cannot work it out.</p>\
         </div></shreddit-comment>",
    );
    let (kind, _, text) = extract_shape(&html);
    assert_eq!(kind, "article");
    assert!(text.contains("very new to RSS"), "the body is missing: {text}");
    assert!(text.contains("This is the website"), "the body was truncated: {text}");
    assert!(!text.contains("Try this"), "a comment leaked into the body: {text}");
    // The title and byline still prefix this body. That is plan D4's `lead` and not fixed here:
    // removing them needs byline detection, and the length-comparison version of it cost four
    // corpus families at once (see `shape::decide`).
}

#[test]
fn an_ordinary_article_with_an_empty_comment_section_is_not_reshaped() {
    // The blast radius, pinned. This page *is* a discussion by the same test -- it has comment
    // furniture and no replies -- so the rule runs on it. It must find the body and leave
    // everything alone, because the alternative is that a news site with a dead Disqus embed
    // starts returning `discussion-root` and no text at all.
    let html = "<html><body><main>\
        <div class=\"byline\"><a href=\"/staff/j\">Jane Roe</a><time>2026-08-01</time></div>\
        <h1>A perfectly ordinary news article</h1>\
        <div class=\"body\"><p>The first paragraph of a story that has nothing to do with \
        link aggregators, long enough that no length rule could confuse it for a pointer.</p>\
        <p>A second paragraph, because one is not a body.</p></div>\
        <div id=\"disqus_thread\"><p>Comments are closed for this article.</p></div>\
        </main></body></html>";
    let (kind, _, text) = extract_shape(html);
    assert_eq!(kind, "article", "a page with prose is never a pointer");
    assert!(text.contains("first paragraph"), "the body was lost: {text}");
    assert!(!text.contains("Comments are closed"), "comment furniture in body: {text}");
}

#[test]
fn a_page_with_no_comment_furniture_gets_no_opinion_at_all() {
    // Most of the web. `shape` must be `None`, not `WithBody`: the two are different claims and
    // only one of them is honest about a page we never examined for a submission.
    let html = "<html><body><main><article>\
        <h1>Just an article</h1>\
        <p>Prose that stands alone with no discussion attached to it whatsoever, and so the \
        shape decision has nothing to say about this page.</p>\
        <p>A second paragraph to keep the region from being a single block.</p>\
        </article></main></body></html>";
    let (arena, _) = BuildArena::parse_to_arena(html, Limits::DEFAULT);
    let out = legibility_core::extract_all(&arena, Limits::DEFAULT);
    assert!(out.shape.is_none(), "shape decided on a page with no discussion");
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

#[test]
fn empty_wrappers_are_collapsed_but_content_carrying_ones_are_not() {
    // A page built from custom elements and slots, where nearly all the text is button labels,
    // used to serialize as a skeleton: ~90 nested empty <div>/<span> pairs around one paragraph.
    // Every per-node rule was right; none of them can see that a container ended up empty.
    let html = "<html><body><main><div><span></span><div><span>   </span></div>\
        <p>The one paragraph that actually says something on this page.</p></div></main></body></html>";
    let (arena, _) = BuildArena::parse_to_arena(html, Limits::DEFAULT);
    let out = legibility_core::extract_all(&arena, Limits::DEFAULT);
    let n = out.selection.article.expect("a region");
    let (h, _) = legibility_dom::serialize::serialize_region::<legibility_sanitize::Article>(
        &arena,
        n,
        legibility_dom::serialize::SerializeOptions::default(),
    );
    assert!(!h.as_str().contains("<span"), "empty spans survived: {}", h.as_str());
    assert!(h.as_str().contains("actually says something"), "content lost: {}", h.as_str());
}

#[test]
fn an_image_only_container_and_an_empty_table_cell_both_survive() {
    // The two cases that make the cheaper rule -- "drop any subtree with zero prose" -- wrong.
    // An image-only figure has no prose and is content; an empty <td> is a column, not clutter.
    let html = "<html><body><main><article>\
        <p>Prose long enough that this region is chosen over anything else on the page here.</p>\
        <figure><img src=\"/a.png\" alt=\"\"></figure>\
        <table><tr><td>x</td><td></td></tr></table>\
        </article></main></body></html>";
    let (arena, _) = BuildArena::parse_to_arena(html, Limits::DEFAULT);
    let out = legibility_core::extract_all(&arena, Limits::DEFAULT);
    let n = out.selection.article.expect("a region");
    let (h, _) = legibility_dom::serialize::serialize_region::<legibility_sanitize::Article>(
        &arena,
        n,
        legibility_dom::serialize::SerializeOptions::default(),
    );
    let s = h.as_str();
    assert!(s.contains("<img"), "an image-only container was dropped: {s}");
    assert!(s.contains("src=\"/a.png\""), "the image lost its src: {s}");
    assert!(s.contains("<td></td>"), "an empty table cell was dropped: {s}");
}

#[test]
fn links_keep_their_href_and_lose_it_when_the_scheme_is_not_allowed() {
    // `write_attrs` was a stub, so every link in every article came out as dead text and every
    // image as a broken icon. Invisible to the corpus gate: token-multiset F1 scores *text*, and
    // an attribute is not text -- which is why the demo found this and 130 corpus pages did not.
    let html = "<html><body><main><article>\
        <p>Body prose long enough to be chosen as the region on this small test page here.</p>\
        <p><a href=\"https://ok.test/x\">good</a><a href=\"javascript:alert(1)\">bad</a>\
        <a href=\"data:text/html,hi\">worse</a></p>\
        </article></main></body></html>";
    let (arena, _) = BuildArena::parse_to_arena(html, Limits::DEFAULT);
    let out = legibility_core::extract_all(&arena, Limits::DEFAULT);
    let n = out.selection.article.expect("a region");
    let (h, rep) = legibility_dom::serialize::serialize_region::<legibility_sanitize::Article>(
        &arena,
        n,
        legibility_dom::serialize::SerializeOptions::default(),
    );
    let s = h.as_str();
    assert!(s.contains("href=\"https://ok.test/x\""), "an allowed href was dropped: {s}");
    assert!(!s.contains("javascript:"), "a javascript: URL survived: {s}");
    assert!(!s.contains("data:"), "a data: URL survived: {s}");
    assert_eq!(rep.rejected_urls, 2, "rejections must be reported, not silent");
    // The anchor text stays either way: refusing the destination is not a reason to lose the words.
    assert!(s.contains("bad") && s.contains("worse"), "text lost with the URL: {s}");
}

#[test]
fn indentation_between_tags_does_not_dilute_link_density() {
    // The mechanism behind a link submission reading as a body. The anchor is wrapped in four
    // nested divs and the markup is pretty-printed, so every level adds a newline and spaces.
    // Counting those as prose grew link_density's denominator while its numerator -- the anchor
    // text -- stayed fixed: 0.87 at the <p>, 0.74 three levels up, under the 0.75 viability
    // floor. A bare URL then out-massed the title and the page reported a body it does not have.
    let html = "<html><body><main>\n  <div slot=\"credit-bar\">\n    <a href=\"/r/rss/\">r/rss</a>\n \
        <time>2d ago</time>\n  </div>\n  <h1>[Github] Free news APIs differ significantly in \
        historical coverage, languages and search filters</h1>\n  <shreddit-post-text-body>\n    \
        <div>\n      <div>\n        <div>\n          <p>\n            [<a \
        href=\"https://github.com/free-news-api/news-api\">\
        https://github.com/free-news-api/news-api</a>]\n          </p>\n        </div>\n      \
        </div>\n    </div>\n  </shreddit-post-text-body>\n  <shreddit-comment-tree>\n    \
        <p>Be the first to comment</p>\n    <p>Nobody's responded to this post yet.</p>\n  \
        </shreddit-comment-tree>\n</main></body></html>";
    let (kind, url, text) = extract_shape(html);
    assert_eq!(kind, "discussion-root", "wrapper indentation made a URL look like a body");
    assert_eq!(url, "https://github.com/free-news-api/news-api");
    assert_eq!(text, "", "a pointer must have an empty body, got: {text}");
}

#[test]
fn comment_furniture_and_an_ad_do_not_become_the_submission_body() {
    // Reconstructed from the page that took four rounds to diagnose. `<main>` is the accepted
    // region, and it holds three things wider than the headline: a house ad, the empty comment
    // section's own prose, and a wrapper <div> whose length is the sum of what is inside it. On
    // the real page those measured 183, 147 and 190 bytes against a 108-byte title, so a link
    // submission with no body at all was reported as having one.
    //
    // Two separate causes, both live here. The ad is outside the submission and was never the
    // submission's business. The wrapper's prose is entirely comment-section prose that the
    // extraction had already agreed to discard -- excluded subtrees are skipped by the scan but
    // still counted in every ancestor's `prose_len`, and an ancestor is visited before the
    // descendant it would have skipped.
    let html = "<html><body><main>\
        <shreddit-post permalink=\"/r/rss/comments/x/\">\
          <div slot=\"credit-bar\"><a href=\"/r/rss/\">r/rss</a>\
            <time datetime=\"2026-08-03T00:00:00Z\">2d ago</time>\
            <a href=\"/user/rangeva/\">rangeva</a></div>\
          <h1 slot=\"title\">Free news APIs differ in coverage and search filters</h1>\
          <shreddit-post-text-body><div><div><p>\
            <a href=\"https://github.com/free-news-api/news-api\">\
            https://github.com/free-news-api/news-api</a></p></div></div>\
          </shreddit-post-text-body>\
        </shreddit-post>\
        <shreddit-comments-page-ad><div>Sponsored. Ship faster with the tools that teams \
          everywhere already trust for their day to day work.</div></shreddit-comments-page-ad>\
        <div id=\"tree-wrapper\">\
          <shreddit-comment-tree><section><h1>Comments Section</h1>\
            <comment-forest-empty-state><p>Be the first to comment</p>\
            <p>Nobody's responded to this post yet. Add your thoughts and get the conversation \
            going.</p></comment-forest-empty-state></section></shreddit-comment-tree>\
        </div></main></body></html>";
    let (kind, url, text) = extract_shape(html);
    assert_eq!(kind, "discussion-root", "a pointer was reported as an article: {text}");
    assert_eq!(url, "https://github.com/free-news-api/news-api");
    assert_eq!(text, "", "a link submission must not carry a body");
}

#[test]
fn a_heading_that_only_repeats_the_title_is_dropped_but_a_differing_one_is_kept() {
    // Readability drops a heading that restates the title, so the corpus `expected.html` files
    // were produced without it -- keeping it was scored as if we had invented text. It is also
    // the duplicated headline in front of every discussion body.
    let dup = "<html><head><title>Free news APIs differ in coverage</title></head><body><main>\
        <article><h1>Free news APIs differ in coverage</h1>\
        <p>The body itself, long enough to carry this region past anything else here.</p>\
        </article></main></body></html>";
    let (_, text, _) = extract(dup);
    assert!(!text.contains("Free news APIs"), "the restated title survived: {text}");
    assert!(text.contains("The body itself"), "the body was lost: {text}");

    // The guard that matters: when `<title>` and `<h1>` disagree the heading is real content.
    // Comparing a heading against a title *harvested from that heading* always matches, which is
    // how the corpus page named for this discrepancy lost its `<h1>`.
    let differs = "<html><head><title>Site name — section</title></head><body><main>\
        <article><h1>A headline the title tag never mentions</h1>\
        <p>The body itself, long enough to carry this region past anything else here.</p>\
        </article></main></body></html>";
    let (_, text2, _) = extract(differs);
    assert!(text2.contains("never mentions"), "a real heading was dropped: {text2}");
}

#[test]
fn blank_line_soup_is_collapsed_without_welding_inline_words_together() {
    // Dropping an element takes its tags and the whitespace between them, but not the indentation
    // on either side, so each removed wrapper left a newline behind. A Reddit post came out as
    // three paragraphs adrift in fifty blank lines: correct, and indistinguishable from broken.
    //
    // The second half is the safety property. Only runs containing a line break are collapsed,
    // because the whitespace separating two inline elements on one line is a plain space, and
    // collapsing that would join two words into one.
    let html = "<html><body><main><article>\
        <div>\n   <span></span>\n   <span>  </span>\n   </div>\
        <p>A paragraph long enough to carry this region past everything else on the page.</p>\
        <p><em>one</em> <em>two</em></p>\
        </article></main></body></html>";
    let (arena, _) = BuildArena::parse_to_arena(html, Limits::DEFAULT);
    let out = legibility_core::extract_all(&arena, Limits::DEFAULT);
    let n = out.selection.article.expect("a region");
    let (h, _) = legibility_dom::serialize::serialize_region::<legibility_sanitize::Article>(
        &arena,
        n,
        legibility_dom::serialize::SerializeOptions::default(),
    );
    let s = h.as_str();
    assert!(!s.contains("\n\n"), "blank runs survived: {s:?}");
    assert!(s.contains("<em>one</em> <em>two</em>"), "inline words were welded: {s:?}");
}

#[test]
fn inline_emphasis_survives_and_custom_elements_still_do_not() {
    // The serializer read `TagId::known_name`, which resolves only the elements the *scorer*
    // interns as integers. Everything else -- em, strong, b, i, mark, abbr, sub, sup, figure --
    // came back nameless and was unwrapped out of every article on the web. It read as correct
    // because that same path is what unwraps custom elements, which genuinely should go, and
    // because emphasis is invisible to a token-multiset F1 score.
    let html = "<html><body><main><article>\
        <p>A paragraph long enough to carry this region past everything else on this page.</p>\
        <p>With <em>emphasis</em>, <strong>weight</strong>, <mark>a mark</mark> and \
          <abbr title=\"HyperText\">HT</abbr>.</p>\
        <my-widget><p>Inside a custom element, which keeps its text and loses its tag.</p>\
          </my-widget>\
        </article></main></body></html>";
    let (arena, _) = BuildArena::parse_to_arena(html, Limits::DEFAULT);
    let out = legibility_core::extract_all(&arena, Limits::DEFAULT);
    let n = out.selection.article.expect("a region");
    let (h, _) = legibility_dom::serialize::serialize_region::<legibility_sanitize::Article>(
        &arena,
        n,
        legibility_dom::serialize::SerializeOptions::default(),
    );
    let s = h.as_str();
    for tag in ["<em>", "<strong>", "<mark>", "<abbr"] {
        assert!(s.contains(tag), "{tag} was unwrapped: {s}");
    }
    assert!(!s.contains("my-widget"), "a custom element kept its tag: {s}");
    assert!(s.contains("loses its tag"), "custom element text was dropped: {s}");
}
