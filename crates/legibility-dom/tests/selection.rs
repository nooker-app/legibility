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
    sel.article.and_then(|n| arena.tag.get(n.idx()).copied().unwrap_or(TagId::UNKNOWN).known_name())
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
    assert!(!sel.region_from_semantic_anchor, "ten equal anchors is not an anchor signal");
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
    let tag = out.selection.article.and_then(|n| arena.tag_name(n)).unwrap_or("none").to_string();
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
    let html =
        "<html><body><main>\n  <div slot=\"credit-bar\">\n    <a href=\"/r/rss/\">r/rss</a>\n \
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

    // The case exact equality could not see, and the reason this test exists twice. A `<title>`
    // nearly always carries the site name and an `<h1>` nearly never does, so requiring equality
    // meant the rule never fired on a real page -- the Reddit post it was written for kept its
    // duplicated headline through two rounds of "fixed".
    let suffixed = "<html><head><title>New to RSS here, why can't i add this site? : r/rss\
        </title></head><body><main>\
        <article><h1>New to RSS here, why can't i add this site?</h1>\
        <p>The body itself, long enough to carry this region past anything else here.</p>\
        </article></main></body></html>";
    let (_, text3, _) = extract(suffixed);
    assert!(!text3.contains("New to RSS here"), "a title suffix hid the duplication: {text3}");
    assert!(text3.contains("The body itself"), "the body was lost: {text3}");
}

#[test]
fn the_dateline_before_a_discussion_headline_goes_but_an_articles_stays() {
    // Plan D4's `lead.byline_span`. Every discussion site opens the submission with a credit bar
    // -- community link, author, timestamp -- and it rode in front of the body because it lives in
    // the same container. Every field of it is already a metadata candidate with provenance.
    //
    // Narrowing the region to the body was tried twice for this and reverted twice, because
    // "widest prose block" is a cruder rule than the scorer's and cost three corpus pages ~85% of
    // their text. This removes one direct child on a structural fact instead: it precedes the
    // headline and it carries a `<time>`.
    let mut post = String::from(
        "<html><head><title>Which feed reader do you use? : r/rss</title></head><body><main>\
         <div><a href=\"/r/rss/\">r/rss</a><time datetime=\"2026-07-24T20:36:22Z\">12d ago</time>\
           <a href=\"/user/someone/\">someone</a></div>\
         <h1>Which feed reader do you use?</h1>\
         <div><p>I have been trying a few and cannot decide, so I would like to hear what \
           everyone else has settled on and why.</p></div>",
    );
    // The comment section is what makes this a discussion page at all, and the gate the byline
    // rule sits behind.
    for i in 0..6 {
        post.push_str(&format!(
            "<div class=\"comment\"><a href=\"/user/u{i}/\">u{i}</a>\
             <time datetime=\"2026-07-25T0{i}:00:00Z\">1d</time>\
             <p>Reply number {i}, with enough words in it to read as an actual comment body.</p>\
             </div>"
        ));
    }
    post.push_str("</main></body></html>");
    let (_, text, n) = extract(&post);
    assert!(n >= 5, "the comment group was not found, so the gate never opened: {n}");
    assert!(text.contains("cannot decide"), "the body was lost: {text}");
    assert!(!text.contains("12d ago"), "the dateline survived: {text}");
    assert!(!text.contains("someone"), "the byline survived: {text}");

    // A news article has a dateline in the same position, and it must keep it: this page is not a
    // discussion, so `shape` is `None` and the rule is unreachable. That gate is why the 130-page
    // corpus cannot regress here.
    let news = "<html><head><title>Markets close higher — Daily</title></head><body><main>\
        <article>\
        <div>By A Reporter <time datetime=\"2026-07-24T20:36:22Z\">24 July 2026</time></div>\
        <h1>Markets close higher</h1>\
        <p>Equities finished the session up across the board, with the broadest gains in \
          industrials and a late rally in energy.</p>\
        </article></main></body></html>";
    let (_, news_text, _) = extract(news);
    assert!(
        news_text.contains("A Reporter") && news_text.contains("24 July 2026"),
        "an article lost its dateline: {news_text}"
    );
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

#[test]
fn a_label_beside_the_headline_goes_but_never_the_body_with_it() {
    // Removing a duplicate `<h1>` leaves its siblings behind, and on a discussion page a sibling is
    // a category chip: beebs.hada.io put `질문` at the top of every extracted body. Two characters,
    // which is why it survived several rounds of that page being reported.
    let chip =
        "<html><head><title>인앱 브라우저에서 패스키를 지원할 방법은 없는걸까요?</title></head>\
        <body><main><article>\
        <header><div class=\"title-line\"><span class=\"category-chip\">질문</span>\
          <h1>인앱 브라우저에서 패스키를 지원할 방법은 없는걸까요?</h1></div></header>\
        <div class=\"body\"><p>어쩌다가 카톡의 인앱 브라우저로 로그인을 시도했는데 패스키는 동작을 \
          안하더라고요. 다른 분들은 잘 되시는지 궁금합니다.</p></div>\
        </article></main></body></html>";
    let (_, text, _) = extract(chip);
    assert!(!text.contains('질'), "the category chip survived: {text}");
    assert!(text.contains("동작을"), "the body was lost: {text}");

    // The first version of that rule ascended to the heading's *wrapper* and excluded it, which is a
    // more natural way to say "the title line" and deletes articles: a body of 24 bytes puts the
    // wrapper inside the slack, and the wrapper spans the whole region. This came back with
    // `html: ""` -- an article reported as found, containing nothing.
    let short = "<html><head><title>Passkeys in in-app browsers are broken</title></head><body>\
        <article><div class=\"post\">\
        <h1>Passkeys in in-app browsers are broken</h1>\
        <p>Yes, confirmed here too.</p>\
        </div></article></body></html>";
    let (_, short_text, _) = extract(short);
    assert!(
        short_text.contains("confirmed here too"),
        "a short body was excluded along with the headline: {short_text:?}"
    );

    // And a one-line dek is content, not furniture. It is a `<p>`, which is the author saying so,
    // and it sits beside the heading's wrapper where a chip sits beside the heading itself.
    let dek = "<html><head><title>Why Neon Is Fading</title></head><body><article>\
        <div class=\"outer\">\
          <div class=\"mid\"><header><span class=\"kicker\">Design</span>\
            <h1>Why Neon Is Fading</h1></header></div>\
          <p class=\"dek\">Neon, briefly.</p>\
        </div>\
        <div class=\"body\"><p>Neon lighting was invented in 1910 and spread through every city \
          centre before the cheaper alternatives arrived.</p></div>\
        </article></body></html>";
    let (_, dek_text, _) = extract(dek);
    assert!(dek_text.contains("Neon, briefly"), "the dek was eaten: {dek_text}");
    assert!(!dek_text.contains("Design"), "the kicker survived: {dek_text}");
}

#[test]
fn the_marked_article_body_beats_the_article_that_wraps_the_whole_page() {
    // news.hada.io, reported five times over. One `<article>` holds the vote arrows, the credit bar,
    // the body, the related-links list and the comments, and marks the body alone with
    // `itemprop="articleBody"`. `<article>` is a real anchor and the wrong one; it is also the widest
    // viable candidate, so no statistic finds the narrower claim.
    let html = "<html><head><title>Warp Agent CLI | GeekNews</title></head><body><main>\
        <article>\
        <div class=\"topicinfo\">1P by <a href=\"/@neo\">GN⁺</a> \
          <time datetime=\"2026-08-05T22:05:33+09:00\">20시간전</time> | \
          <a href=\"topic?id=1\">댓글 2개</a></div>\
        <section itemprop=\"articleBody\"><p>독립형 CLI로 제공해 다른 터미널에서도 사용할 수 있음</p>\
          <p>tmux와 유사한 멀티플렉싱 구조로 PTY 연결을 관리함</p></section>\
        <div class=\"related-topics\"><h2>함께 보면 좋은 글</h2>\
          <ul><li><a href=\"/topic?id=2\">Herdr - 터미널 워크스페이스</a></li>\
          <li><a href=\"/topic?id=3\">telepty — 컨트롤 플레인</a></li></ul></div>\
        </article></main></body></html>";
    let (_, text, _) = extract(html);
    assert!(text.contains("PTY 연결"), "the marked body was not chosen: {text}");
    assert!(!text.contains("GN⁺"), "the credit bar survived: {text}");
    assert!(!text.contains("함께 보면"), "the related list survived: {text}");
}

#[test]
fn navigation_inside_main_goes_but_content_in_a_form_stays() {
    // GitHub puts the repository bar and the tab strip inside the same `<main>` as the pull request,
    // so the body opened with `Code Issues 4 Pull requests 3 … Conversation Commits Checks`.
    let gh = "<html><head><title>Reuse datasource connections · Pull Request #39</title></head>\
        <body><main>\
        <div id=\"repository-container-header\"><ul><li><a href=\"/n\">Notifications</a></li>\
          <li><a href=\"/f\">Fork 6</a></li><li><a href=\"/s\">Star 78</a></li></ul></div>\
        <nav aria-label=\"Repository\"><a href=\"/code\">Code</a><a href=\"/issues\">Issues 4</a>\
          <a href=\"/pulls\">Pull requests 3</a><a href=\"/insights\">Insights</a></nav>\
        <div class=\"comment-body\"><p>queryDatabase builds a datasource and closes it again on \
          every sync, which costs a full connection handshake per source per interval.</p>\
          <p>This reuses a pooled datasource keyed by type and config.</p></div>\
        <footer><a href=\"/terms\">Terms</a><a href=\"/privacy\">Privacy</a></footer>\
        </main></body></html>";
    let (_, text, _) = extract(gh);
    assert!(text.contains("pooled datasource"), "the body was lost: {text}");
    for gone in ["Issues 4", "Pull requests 3", "Insights", "Notifications", "Star 78", "Privacy"] {
        assert!(!text.contains(gone), "{gone:?} survived in the body: {text}");
    }

    // The invariant that keeps the rule from being a content shredder: removal must leave prose
    // behind. A wiki edit preview puts the whole article inside a `<form>`, and losing it to tidy up
    // a toolbar is much the worse error.
    let wiki = "<html><head><title>Editing Neon lighting</title></head><body><main>\
        <form action=\"/save\"><div>\
        <p>Neon lighting was invented in 1910 and spread through every city centre before the \
          cheaper alternatives arrived to replace it.</p>\
        <p>The tubes are filled at low pressure and excited by a few thousand volts.</p>\
        </div></form></main></body></html>";
    let (_, wiki_text, _) = extract(wiki);
    assert!(
        wiki_text.contains("invented in 1910"),
        "an article inside a <form> was excluded: {wiki_text:?}"
    );
}

#[test]
fn a_boilerplate_name_alone_does_not_remove_a_block_that_holds_prose() {
    // Readability removes any element whose class or id matches a word list, which is also how it
    // eats articles. Here the name has to agree with the structure: no authored paragraph *and*
    // mostly links. A share widget wrapped around a pull quote keeps its quote.
    let html = "<html><head><title>Why Neon Is Fading</title></head><body><main><article>\
        <div class=\"social-share\"><blockquote>The tubes are hand-bent, one letter at a \
          time.</blockquote><a href=\"/x\">Share</a></div>\
        <p>Neon lighting was invented in 1910 and spread through every city centre before the \
          cheaper alternatives arrived.</p>\
        <p>The craft survives in a few workshops, mostly for restoration work.</p>\
        </article></main></body></html>";
    let (_, text, _) = extract(html);
    assert!(text.contains("hand-bent"), "a named block lost its quote: {text}");
    assert!(text.contains("invented in 1910"), "the body was lost: {text}");
}

#[test]
fn timeline_events_are_not_comments_but_paragraphs_still_are() {
    // A GitHub pull request's `added 3 commits` and `mentioned this pull request` carry an author
    // link and a timestamp exactly as a comment does, so micro-metadata cannot separate them. Three
    // of them outnumbered the real conversation and commit records were reported as comments.
    let mut events = String::from(
        "<html><head><title>perf(hub): reuse datasource connections · Pull Request #39</title>\
         </head><body><main>\
         <div class=\"comment-body\"><p>queryDatabase builds a datasource and closes it again on \
           every sync, which costs a connection handshake per source per interval.</p></div>\
         <div id=\"timeline\">",
    );
    for i in 0..3 {
        events.push_str(&format!(
            "<div class=\"TimelineItem\" id=\"event-{i}\">\
             <a href=\"/selenehyun\">selenehyun</a> added {i} commits \
             <a href=\"#c{i}\"><time datetime=\"2026-08-05T12:4{i}:50+09:00\">August 5</time></a>\
             <div><pre><code>891881{i}</code></pre>\
             <a href=\"/commit/891881{i}\">perf(lynqhub): reuse datasource connections</a></div>\
             </div>"
        ));
    }
    events.push_str("</div></main></body></html>");
    let (_, _, n) = extract(&events);
    assert_eq!(n, 0, "timeline events were reported as {n} comment(s)");

    // And the signal must not cost a real thread: the same shape *with* authored paragraphs is one.
    let (_, _, real) = extract(&discussion_shaped(18));
    assert_eq!(real, 18, "a genuine thread was lost to the paragraph rule");
}

/// A thread of one is found, and a thread we genuinely cannot find still says so.
///
/// These are two halves of the same promise and they used to be one test, because the first half was
/// impossible: every path into group detection needs look-alike siblings, and a single reply has
/// nothing to repeat. The old test asserted that limitation — `items.is_empty() && truncated` — and
/// so the honest reporting was all there was.
///
/// `groups::lone_comment` closes it by trusting the page's own count exactly as
/// `Group::is_comment_thread` already does to admit a *pair* below `MIN_GROUP`. So the first half
/// now asserts the comment is read. The second half is unchanged and still matters: when the
/// replies are not in the HTML at all, as Reddit's `load more comments` stub leaves them, there is
/// nothing to find and saying `count: 0` without a claim is the silent omission plan §1.9 exists to
/// make impossible.
#[test]
fn a_thread_of_one_is_found_and_a_thread_that_is_absent_is_reported() {
    let html = "<html><head><title>Irken, a small IRC client | GeekNews</title></head>\
        <body><main><article>\
        <div class=\"topicinfo\">1P by <a href=\"/@neo\">GN⁺</a> \
          <time datetime=\"2026-08-05T22:05:33+09:00\">19시간전</time> | \
          <a href=\"topic?id=1\">댓글 1개</a></div>\
        <section itemprop=\"articleBody\"><p>Tcl/Tk로 만든 IRC 클라이언트로, 코드를 이해하고 개조할 \
          수 있을 만큼 작은 규모를 지향함</p></section>\
        <div id=\"comment_thread\"><div class=\"comment_row\" id=\"cid1\">\
          <div class=\"commentinfo\"><a href=\"/@u1\">사용자1</a>\
            <time datetime=\"2026-08-05T23:00:00+09:00\">19시간전</time></div>\
          <div><p>재미있네요. 직접 고쳐 쓸 수 있는 크기라는 점이 좋습니다.</p></div></div></div>\
        </article></main></body></html>";
    let (arena, _) = BuildArena::parse_to_arena(html, Limits::DEFAULT);
    let out = legibility_core::extract_all(&arena, Limits::DEFAULT);
    let c = &out.comments.completeness;
    assert_eq!(c.claimed_total, Some(1), "the page's own count was not reported");
    assert_eq!(out.comments.items.len(), 1, "the one comment on the page was not found");
    assert!(!c.truncated, "a complete thread of one was reported as truncated");
    let item = &out.comments.items[0];
    assert_eq!(item.author.as_deref(), Some("사용자1"), "wrong author: {:?}", item.author);
    assert!(item.text.contains("직접 고쳐 쓸 수 있는"), "wrong text: {}", item.text);

    // The other half: the page claims replies that are not in the markup at all.
    let stub = "<html><head><title>A post : r/rss</title></head><body><main>\
        <div><a href=\"/r/rss/\">r/rss</a><time datetime=\"2026-08-05T22:05:33+09:00\">12d</time>\
        <a href=\"/user/someone/\">someone</a></div>\
        <h1>A post with a thread that did not render</h1>\
        <div><p>The body of the post, long enough to be selected as the article region here.</p>\
        </div>\
        <div><a href=\"?comments=1\">42 comments</a></div>\
        <button>load more comments</button></main></body></html>";
    let (arena, _) = BuildArena::parse_to_arena(stub, Limits::DEFAULT);
    let out = legibility_core::extract_all(&arena, Limits::DEFAULT);
    let c = &out.comments.completeness;
    assert_eq!(c.claimed_total, Some(42), "the page's own count was not reported");
    assert!(
        out.comments.items.is_empty() && c.truncated,
        "present {} of a claimed 42 with truncated {} — a missing thread must say so",
        c.present,
        c.truncated
    );
}

#[test]
fn a_truncated_region_still_closes_every_tag_it_opened() {
    // The cap used to `break` mid-walk, dropping the close step it had just popped and abandoning
    // the rest of the stack — so a truncated article ended inside an element. A consumer inserting
    // that fragment has its own markup adopted into the hole, and reparsing it does not give back
    // the tree it was a prefix of.
    let mut html = String::from("<html><head><title>Long</title></head><body><main><article>");
    for i in 0..200 {
        html.push_str(&format!("<div><p>Paragraph number {i} of a very long article.</p></div>"));
    }
    html.push_str("</article></main></body></html>");

    let (arena, _) = BuildArena::parse_to_arena(&html, Limits::DEFAULT);
    let out = legibility_core::extract_all(&arena, Limits::DEFAULT);
    let n = out.selection.article.expect("a region");
    let (frag, report) = legibility_dom::serialize::serialize_region::<legibility_sanitize::Article>(
        &arena,
        n,
        legibility_dom::serialize::SerializeOptions { max_output_bytes: 400, ..Default::default() },
    );
    assert!(report.truncated, "the cap did not fire");

    // Every opened tag is closed, innermost first. Counting is enough to catch the regression and
    // needs no parser: an unbalanced fragment leaves names on the stack.
    let s = frag.as_str();
    let mut open: Vec<&str> = Vec::new();
    let mut rest = s;
    while let Some(at) = rest.find('<') {
        rest = &rest[at + 1..];
        let end = rest.find('>').unwrap_or(rest.len());
        let tag = &rest[..end];
        if let Some(name) = tag.strip_prefix('/') {
            assert_eq!(open.pop(), Some(name), "close tag with no matching open in {s:?}");
        } else if !tag.starts_with('!') && !tag.ends_with('/') {
            let name = tag.split_whitespace().next().unwrap_or(tag);
            // Void elements never close.
            if !matches!(name, "img" | "br" | "hr" | "wbr" | "source" | "track") {
                open.push(name);
            }
        }
        rest = &rest[end.min(rest.len())..];
    }
    assert!(open.is_empty(), "truncated fragment left {open:?} unclosed: {s:?}");
}

#[test]
fn the_host_output_cap_is_actually_enforced() {
    // `Limits::max_output_bytes` was declared, documented per host profile — 4 MiB in a browser,
    // 2 MiB in an iOS share extension because an extension is killed for memory far sooner than its
    // host app — and read by nothing. Every serialization used the serializer's own 16 MiB default,
    // so both host caps were decoration. A limit nobody enforces is worse than no limit: a caller
    // reads it and plans around a bound that is not there.
    let mut html = String::from("<html><head><title>Long</title></head><body><main><article>");
    for i in 0..4000 {
        html.push_str(&format!(
            "<p>Paragraph number {i}, with enough words in it to add up to a document larger than \
             the smallest host profile is willing to hand back in one piece.</p>"
        ));
    }
    html.push_str("</article></main></body></html>");

    let small = Limits { max_output_bytes: 64 * 1024, ..Limits::IOS_APP_EXTENSION };
    let (arena, hit) = BuildArena::parse_to_arena(&html, small);
    let out = legibility_core::extract_all(&arena, small);
    let json = legibility_dom::json::extraction_json_limited(&arena, &out, hit, None, small);
    assert!(
        json.contains("\"truncated\":true"),
        "a {} KB cap did not truncate a {} KB document",
        small.max_output_bytes / 1024,
        html.len() / 1024
    );

    // And the default profile must not truncate the same document, or the cap is just a bug.
    let (arena, hit) = BuildArena::parse_to_arena(&html, Limits::DEFAULT);
    let out = legibility_core::extract_all(&arena, Limits::DEFAULT);
    let json =
        legibility_dom::json::extraction_json_limited(&arena, &out, hit, None, Limits::DEFAULT);
    assert!(json.contains("\"truncated\":false"), "the default profile truncated");
}

#[test]
fn a_long_paragraph_is_not_cut_at_the_attribute_limit() {
    // `push_text` served both attribute values and text nodes, and applied `max_attr_bytes` (64 KiB)
    // to both. Any text node over that was silently truncated and the loss reported as
    // `attr_bytes` — a long article lost its ending and blamed an attribute limit for it. Text is
    // bounded by the document, which `max_input_bytes` already bounds.
    let para = "word ".repeat(30_000); // ~150 KB in one text node
    let html = format!(
        "<html><head><title>Long</title></head><body><main><article><p>{para}</p></article>\
         </main></body></html>"
    );
    let (arena, hit) = BuildArena::parse_to_arena(&html, Limits::DEFAULT);
    let out = legibility_core::extract_all(&arena, Limits::DEFAULT);
    let n = out.selection.article.expect("a region");
    let text = legibility_dom::json::prose_text_excluding(&arena, n, &out.article_exclusions);
    assert!(
        text.len() > 100_000,
        "a {} KB paragraph came back as {} KB",
        para.len() / 1024,
        text.len() / 1024
    );
    assert!(!hit.attr_bytes, "text truncation was reported as an attribute limit");
}

#[test]
fn hitting_the_node_cap_degrades_instead_of_hanging() {
    // `new_node` returns the document handle as a sink once `max_nodes` is reached, and html5ever
    // then appends into it. The claim under test is S2's: every limit yields a *valid degraded*
    // extraction in bounded time, never a hang and never a panic.
    let mut html = String::from("<html><body><main><article>");
    for i in 0..5000 {
        html.push_str(&format!("<div><p>Paragraph {i} of a document with many nodes.</p></div>"));
    }
    html.push_str("</article></main></body></html>");

    let tight = Limits { max_nodes: 500, ..Limits::DEFAULT };
    let (arena, hit) = BuildArena::parse_to_arena(&html, tight);
    assert!(hit.nodes, "the node cap did not report itself");
    assert!(arena.len() <= 512, "arena grew past the cap: {}", arena.len());
    // A degraded extraction is still an extraction: it must answer, either with a region or with a
    // reason, and it must not panic on the way.
    let out = legibility_core::extract_all(&arena, tight);
    assert!(
        out.selection.article.is_some() || out.selection.no_article.is_some(),
        "capped input produced neither a region nor a reason"
    );
}

#[test]
fn no_payload_survives_into_a_comment_body() {
    // `comments[].html` is attacker-controlled input that consumers render, and the demo inserts it
    // as markup. This is the `UserContent` profile's whole reason to exist (plan §1.8), and it is
    // checked against real output rather than against the sanitizer's own unit tests — the two can
    // disagree, and did: `UserContent` "dropped" images by refusing every attribute on `<img>`,
    // which emitted a bare `<img>`.
    //
    // Re-run in particular because `<form>` moved from drop-subtree to unwrap so that old.reddit's
    // comment bodies survive. Unwrapping keeps the prose and still drops every control inside it.
    const PAYLOADS: [&str; 15] = [
        "<script>alert(1)</script>",
        "<img src=x onerror=alert(1)>",
        "<a href=\"javascript:alert(1)\">click</a>",
        "<a href=\"data:text/html,<script>alert(1)</script>\">d</a>",
        "<svg onload=alert(1)></svg>",
        "<iframe src=\"https://evil.test\"></iframe>",
        "<form action=\"https://evil.test\"><input name=\"a\"><button>go</button></form>",
        "<div id=\"attributes\"></div><div name=\"documentElement\"></div>",
        "<style>body{display:none}</style>",
        "<div onclick=\"alert(1)\" style=\"position:fixed\">x</div>",
        "<object data=\"evil\"></object><embed src=\"evil\">",
        "<details open><summary>s</summary>hidden</details>",
        "<a href=\"#x\" ping=\"https://evil.test\">p</a>",
        "<p>unbalanced</div></p><</p",
        "<template><script>alert(1)</script></template>",
    ];
    let mut html = String::from(
        "<html><head><title>XSS thread</title></head><body><main>\
         <article><h1>XSS thread</h1><p>The body of the post, long enough to be the article here \
         on this page.</p></article><ul class=\"comments\">",
    );
    for (i, p) in PAYLOADS.iter().enumerate() {
        html.push_str(&format!(
            "<li class=\"c\"><div class=\"meta\"><a class=\"author\" href=\"/u/a{i}\">a{i}</a> \
             <time datetime=\"2026-08-0{}T10:00:00Z\">Jan</time></div><p>payload {i}:</p>{p}</li>",
            i % 9
        ));
    }
    html.push_str("</ul></main></body></html>");

    let (arena, hit) = BuildArena::parse_to_arena(&html, Limits::DEFAULT);
    let out = legibility_core::extract_all(&arena, Limits::DEFAULT);
    assert_eq!(out.comments.items.len(), PAYLOADS.len(), "not every payload became a comment");
    let json =
        legibility_dom::json::extraction_json_limited(&arena, &out, hit, None, Limits::DEFAULT);
    let lower = json.to_lowercase();
    for banned in [
        "<script",
        "onerror",
        "onload=",
        "onclick",
        "javascript:",
        "data:text",
        "<iframe",
        "<object",
        "<embed",
        "<form",
        "<input",
        "<button",
        "<style",
        "<svg",
        "style=",
        "ping=",
    ] {
        assert!(!lower.contains(banned), "{banned:?} survived into the output");
    }
    // DOM clobbering: no `id`/`name` that would shadow a document property.
    for clobber in ["\"attributes\"", "\"documentelement\"", "\"children\""] {
        assert!(!lower.contains(clobber), "a clobbering identifier survived: {clobber}");
    }
    // Every surviving link is defanged.
    assert!(
        !json.contains("<a ")
            || json.matches("<a ").count()
                == json.matches("rel=\\\"nofollow noopener noreferrer\\\"").count(),
        "a comment link came out without the forced rel"
    );
}

#[test]
fn a_page_whose_article_is_body_itself_is_not_a_refusal() {
    // Ten of the 130 corpus pages returned no article, and eight of them for one reason: the
    // document has no `<div>`, `<article>`, `<main>` or `<section>` at all, so the `<p>` elements
    // are `<body>`'s own children and `<body>` *is* the article. `lgb explain` reported
    // `candidates 0` on pages of 221 bytes and of 202 KB alike — nothing was small, there was
    // nothing to compare against.
    let mut html = String::from("<html><head><title>A Plain Post</title></head><body>");
    html.push_str("<h1>A Plain Post</h1>");
    for i in 0..8 {
        html.push_str(&format!(
            "<p>Paragraph {i}. Under sufficiently extreme conditions quarks may become deconfined \
             and exist as free particles, which is the subject of this post.</p>"
        ));
    }
    html.push_str("</body></html>");
    let (_, text, _) = extract(&html);
    assert!(text.contains("deconfined"), "a body-only article was refused: {text:?}");

    // And it stays a last resort: a page with a real container must still choose the container,
    // never `<body>`, or the silent fallback that defect 1 exists to remove is back.
    let wrapped = "<html><head><title>Wrapped</title></head><body>\
        <nav><a href=/a>one</a><a href=/b>two</a><a href=/c>three</a></nav>\
        <article><p>The article's own paragraph, which is where the prose actually lives on \
          this page and should be chosen over the body.</p>\
        <p>A second paragraph so the region is not a single block.</p></article>\
        </body></html>";
    let (arena, _) = BuildArena::parse_to_arena(wrapped, Limits::DEFAULT);
    let out = legibility_core::extract_all(&arena, Limits::DEFAULT);
    let tag = out.selection.article.and_then(|n| arena.tag_name(n)).unwrap_or("none");
    assert_eq!(tag, "article", "body was chosen over a real container");

    // A page of pure navigation is still refused: the viability floor applies to `<body>` too.
    let navonly = "<html><head><title>Links</title></head><body>\
        <a href=/a>one</a><a href=/b>two</a><a href=/c>three</a><a href=/d>four</a>\
        </body></html>";
    let (arena, _) = BuildArena::parse_to_arena(navonly, Limits::DEFAULT);
    let out = legibility_core::extract_all(&arena, Limits::DEFAULT);
    assert!(out.selection.article.is_none(), "a page of links came back as an article");
}

#[test]
fn a_credit_bar_goes_even_with_no_comments_and_no_title_but_a_dateline_stays() {
    // Reported a fourth time on the same Reddit post. The byline rule is gated on the page being
    // recognised as a discussion, which needs comment furniture to be present; the duplicate-heading
    // rule needs the document's declared title. A capture with neither satisfied neither, and the
    // credit bar came back at the top of the body:
    //
    //   rss 커뮤니티로 이동  r/rss  13일 전  awesome5ftw  I'm very new to RSS and just started …
    //
    // What is true in both captures is the shape, so that is what the rule keys on now.
    let reddit = "<html lang=\"ko\"><body><main dir=\"ltr\">\
        <div><span>\
          <span><a href=\"https://www.reddit.com/r/rss/\">rss 커뮤니티로 이동</a></span>\
          <div><span><span><a href=\"https://www.reddit.com/r/rss/\">r/rss</a></span>\
            <time datetime=\"2026-07-24T20:36:22.796Z\">13일 전</time></span>\
          <div><span><div><a href=\"https://www.reddit.com/user/awesome5ftw/\">awesome5ftw</a>\
            </div></span></div></div>\
        </span></div>\
        <h1 dir=\"auto\">New to RSS here, why can't i use this website to add to my feed?</h1>\
        <div><div><div dir=\"auto\">\
        <p>I'm very new to RSS and just started setting up apps (Feeder) I follow football closely \
          and wanted to make a list, but I can't seem to add this website for some reason.</p>\
        <p>This is the website.</p></div></div></div>\
        </main></body></html>";
    let (_, text, n) = extract(reddit);
    assert_eq!(n, 0, "this capture has no comment furniture at all");
    assert!(text.contains("very new to RSS"), "the body was lost: {text}");
    for gone in ["커뮤니티로 이동", "13일 전", "awesome5ftw", "r/rss"] {
        assert!(!text.contains(gone), "credit bar fragment {gone:?} survived: {text}");
    }

    // The discriminator is links, not the `<time>`: a news dateline looks the same at a glance and
    // `expected.html` keeps it, so it must stay. It is plain text with at most a byline link, where
    // a credit bar is links end to end.
    let news = "<html><head><title>Markets close higher — Daily</title></head><body><main>\
        <article>\
        <div class=\"dateline\">By A Reporter <time datetime=\"2026-07-24T20:36:22Z\">24 July 2026\
          </time></div>\
        <h1>Markets close higher as industrials rally</h1>\
        <p>Equities finished the session up across the board, with the broadest gains in \
          industrials and a late rally in energy that carried the index past its opening level.</p>\
        </article></main></body></html>";
    let (_, news_text, _) = extract(news);
    assert!(
        news_text.contains("A Reporter") && news_text.contains("24 July 2026"),
        "a news dateline was removed as a credit bar: {news_text}"
    );
}

#[test]
fn a_long_run_of_whitespace_merges_into_one_node() {
    // Found by `structure_aware` within a minute of first running it: 50 KB of carriage returns is
    // 50,000 one-character `AppendText` calls, and the merge path re-scanned the whole merged node
    // for private-use codepoints on each one — 905 ms to parse 50 KB, against a 125 MiB/s budget.
    //
    // Asserted as a node count rather than a duration. The count is the *cause*: the quadratic is
    // only reachable if the run fails to merge, so this catches the regression deterministically,
    // with no wall clock to flake and no `Instant::now` — which `clippy.toml` disallows precisely so
    // that no clock creeps anywhere near the engine.
    //
    // Nothing in the 130-page corpus is shaped like this, which is why a fuzzer had to be the one to
    // say so.
    let body = "\r".repeat(50_000);
    let html = format!("<html><head><title>t</title></head><body>{body}</body></html>");
    let (arena, _) = BuildArena::parse_to_arena(&html, Limits::DEFAULT);
    assert!(
        arena.len() < 32,
        "50,000 whitespace tokens became {} nodes instead of one",
        arena.len()
    );
}

#[test]
fn a_pointer_must_be_mostly_headline_and_link_or_it_is_an_article() {
    // `cnet-svg-classes`. A news article with a Disqus section is correctly recognised as a
    // discussion — it has comment furniture — but the first heading inside the chosen region belongs
    // to the "related articles" box rather than to the article, so the submission title named the
    // wrong node and nothing could out-mass it. The verdict was `LinkOnly`, and the serializer's
    // answer to a pointer is an empty body: 1767 bytes of article deleted.
    //
    // The corpus could not see it. The ratchet scores the *region*, not the serialized output, so
    // that page sat at 0.938 while returning `html: ""`.
    let article = "<html><head><title>Twitter Lite llega a 11 países</title></head><body><main>\
        <div id=\"content\">\
        <div class=\"related\"><h3><a href=\"/es/noticias/otro/\">Artículos Relacionados</a></h3>\
        <ul><li><a href=\"/es/a/\">Primer enlace relacionado con el tema</a></li>\
        <li><a href=\"/es/b/\">Segundo enlace relacionado</a></li></ul></div>\
        <p>Twitter ha dado a conocer que su versión ligera estará disponible en más países de \
          América Latina durante las próximas semanas.</p>\
        <p>La aplicación pesa menos de un megabyte y está pensada para conexiones lentas, algo \
          común en varios de los mercados donde se lanza.</p>\
        <p>La compañía no ha detallado si el resto de la región recibirá la aplicación este año.</p>\
        </div>\
        <div class=\"comments collapsible\"><div class=\"commentsContainer disqusContainer\">\
          </div></div>\
        </main></body></html>";
    let (kind, _, text) = extract_shape(article);
    assert_eq!(kind, "article", "an article with a Disqus box was served as a pointer");
    assert!(text.contains("menos de un megabyte"), "the body was emptied: {text:?}");

    // And the guard must not cost a genuine pointer its shape: past the headline and the
    // destination, a link submission has a credit bar and nothing else.
    let pointer = shreddit_post(
        "<div class=\"md\"><p>[<a href=\"https://github.com/free-news-api/news-api\">\
         https://github.com/free-news-api/news-api</a>]</p></div>",
        "<section><p>Be the first to comment</p>\
         <p>Nobody's responded to this post yet. Add your thoughts and get the conversation \
         going, because this block is longer and denser than the submission itself.</p></section>",
    );
    let (kind, url, text) = extract_shape(&pointer);
    assert_eq!(kind, "discussion-root", "a pointer must not be served as an article");
    assert_eq!(url, "https://github.com/free-news-api/news-api");
    assert_eq!(text, "", "a pointer must have an empty body, got: {text}");
}

#[test]
fn a_pre_block_survives_being_serialized_twice() {
    // The tree-construction spec drops a newline that immediately follows `<pre>`, so serializing a
    // code block verbatim loses one newline every time the output is parsed again — `<pre>\n\n\n…`
    // comes back with two, then one, then none, and the sequence never settles. Found by
    // `sanitize_roundtrip_ugc` in CI, not locally: a 45-second run had missed it, a 60-second one
    // did not.
    //
    // The serialization spec's own remedy is to emit an extra newline, which the reparse then eats.
    let html = "<html><head><title>Recursion</title></head><body><main><article>\
        <p>The naive implementation is the standard teaching example, and it is also the standard \
          example of why memoisation exists at all.</p>\
        <pre><code>\n\n\ndef f(n):\n    if n &lt; 2:\n        return n\n</code></pre>\
        <p>Each call spawns two more, so the tree of calls doubles at every level.</p>\
        </article></main></body></html>";

    let once = serialize_article(html);
    let twice = serialize_article(&once);
    assert_eq!(once, twice, "serializing our own output changed the <pre> block");
    assert!(
        once.contains("\n\n\ndef f(n):"),
        "the code block lost its leading blank lines: {once}"
    );
    assert!(once.contains("    if n &lt; 2:"), "the code block lost its indentation: {once}");
}

/// The article region of `html`, serialized through the `Article` profile.
fn serialize_article(html: &str) -> String {
    let (arena, _) = BuildArena::parse_to_arena(html, Limits::DEFAULT);
    let out = legibility_core::extract_all(&arena, Limits::DEFAULT);
    let Some(n) = out.selection.article else { return String::new() };
    let (h, _) =
        legibility_dom::serialize::serialize_region_excluding::<legibility_sanitize::Article>(
            &arena,
            n,
            legibility_dom::serialize::SerializeOptions::default(),
            &out.article_exclusions,
        );
    h.as_str().to_string()
}

#[test]
fn a_written_date_counts_as_a_timestamp_but_prose_about_a_year_does_not() {
    // `pixnet`: every comment byline is `<span class="post-time">於 2013/12/28 10:28</span>`. No
    // `<time>`, no `datetime`, so the micro-metadata gate never reached two signals, no group
    // formed, and all 39 comments were merged into the article — 3.2x its expected length.
    let mut html = String::from(
        "<html><head><title>露營記錄</title></head><body><main>\
         <article><h1>露營記錄</h1>\
         <p>這次的營地在新竹尖石，海拔一千兩百公尺，天氣很好，晚上還看得到星星。</p>\
         <p>營地的設施算是完整，衛浴乾淨，水源穩定，適合帶小孩一起來。</p></article>\
         <div id=\"comment-text\"><ul>",
    );
    for i in 0..4 {
        html.push_str(&format!(
            "<li class=\"post-info\" id=\"comment-588244{i}\"><span class=\"user-name\">\
             <a href=\"http://u{i}.example.test/blog\" rel=\"nofollow\">訪客{i}</a></span>\
             <span class=\"post-time\">於 2013/12/2{i} 10:28</span>\
             <div class=\"content\"><p>謝謝分享，下次也想去這個營地看看，請問開車方便嗎？</p></div>\
             </li>"
        ));
    }
    html.push_str("</ul></div></main></body></html>");
    let (_, text, n) = extract(&html);
    assert_eq!(n, 4, "written-date bylines did not form a thread");
    assert!(text.contains("海拔一千兩百公尺"), "the article was lost: {text}");
    assert!(!text.contains("開車方便嗎"), "a comment leaked into the body: {text}");

    // The bound is what keeps it narrow: a paragraph *about* a year is not a byline. Four digits
    // plus two small numbers only counts inside something byline-sized.
    assert!(!legibility_core::groups::looks_like_a_date_for_test(
        "The company moved in 1998, employing 12 people across 3 sites, and grew steadily after that."
    ));
    assert!(legibility_core::groups::looks_like_a_date_for_test("2013/12/28 10:28"));
    assert!(legibility_core::groups::looks_like_a_date_for_test("2013年12月28日"));
    assert!(!legibility_core::groups::looks_like_a_date_for_test("2013"));
}

#[test]
fn a_per_item_class_does_not_split_one_template_into_thirteen() {
    // WordPress writes `class="comment-215101"` on every comment, so thirteen structurally
    // identical siblings hashed thirteen different ways, no group of three formed, the thread went
    // undetected, and one of its members was selected as the article.
    let mut html = String::from(
        "<html><head><title>On testing</title></head><body><main>\
         <article><h1>On testing</h1>\
         <p>Certification schemes are expensive to run and the value is hard to demonstrate, which \
           is why so few of them survive their first funding round.</p>\
         <p>The alternative most projects reach for is a public test suite anyone can run.</p>\
         </article>\
         <div id=\"comments\">",
    );
    for i in 0..5 {
        html.push_str(&format!(
            "<div class=\"comment-21510{i} even thread-even\">\
             <article class=\"comment-body\"><footer class=\"comment-meta\">\
             <a class=\"url\" href=\"/user/reader{i}\">reader{i}</a>\
             <time datetime=\"2026-08-0{i}T10:00:00Z\">Aug {i}</time></footer>\
             <div class=\"comment-content\"><p>Were you volunteering to help design the tests, or \
               at least set up the means to organize it?</p></div></article></div>"
        ));
    }
    html.push_str("</div></main></body></html>");
    let (_, text, n) = extract(&html);
    assert_eq!(n, 5, "per-item classes split the thread");
    assert!(text.contains("Certification schemes"), "the article was lost: {text}");
    assert!(!text.contains("volunteering to help"), "a comment leaked into the body: {text}");
}

/// A page whose only title is its `<h1>` must not then print that `<h1>` inside the body.
///
/// Found by running Readability.js against the self-authored fixtures — the one comparison in this
/// repo that is not circular, since on mozilla's corpus `expected.html` *is* R.js's output. R.js
/// dropped the heading here and we kept it, so a reader view rendered the headline twice: once
/// from `metadata.title` and again at the top of `article.html`.
///
/// The pair matters more than either case. With a `<title>` present the `<h1>` stays, because the
/// two disagreeing is a fact about the page rather than a duplicate; with no `<title>` at all the
/// `<h1>` is where the title came from, and keeping it is the duplicate.
#[test]
fn a_headline_that_is_the_only_title_source_is_not_also_left_in_the_body() {
    let body = "<div><p>A paragraph with enough words in it to be selected as the body of \
        this page, rather than being thrown out as furniture by the purity floor.</p></div>";

    let untitled = format!(
        "<html><body><main><h1>Why my feed reader will not add this site</h1>\
        {body}</main></body></html>"
    );
    let (_, text, _) = extract(&untitled);
    assert!(
        !text.contains("Why my feed reader will not add this site"),
        "the <h1> the title was harvested from is still in the body: {text}"
    );
    assert!(text.contains("A paragraph with enough words"), "the body itself was lost: {text}");

    // The guard this fix had to stay inside: `title-and-h1-discrepancy` in mozilla's corpus.
    let titled = format!(
        "<html><head><title>Something else entirely</title></head><body><main>\
        <h1>Why my feed reader will not add this site</h1>{body}</main></body></html>"
    );
    let (_, text, _) = extract(&titled);
    assert!(
        text.contains("Why my feed reader will not add this site"),
        "an <h1> that disagrees with the declared <title> was deleted: {text}"
    );
}

/// A page whose prose hangs off `<body>` rather than off any container must still have an article.
///
/// `<body>` is admitted as a candidate only when nothing else is a real answer, because selecting
/// it as a matter of course is the silent fallback defect 1 exists to remove. The gate used to be
/// "no candidates at all", which missed the commoner shape: candidates that exist and are
/// incidental. `dev418` is three `<li>` holding 1.6% of the page each and scored 0.038;
/// `table-style-attributes` spreads its prose across nested tables and scored 0.434.
#[test]
fn prose_that_belongs_to_no_container_is_still_an_article() {
    let paras = (0..6)
        .map(|i| {
            format!(
                "<p>Paragraph {i} of a document whose author never wrapped the body in a \
             container element, which is a thing real pages do.</p>"
            )
        })
        .collect::<String>();

    // Bare prose under <body>, plus a nav list that *is* containerish and holds almost nothing.
    let html = format!(
        "<html><head><title>A plain document</title></head><body>\
         <ul><li><a href=/a>one</a></li><li><a href=/b>two</a></li></ul>\
         {paras}</body></html>"
    );
    let (tag, text, _) = extract(&html);
    assert_eq!(tag, "body", "the incidental <ul> was ranked against itself and won");
    assert!(text.contains("Paragraph 5 of a document"), "the body was lost: {text}");

    // The guard: when a container *does* hold the prose, it still wins and <body> does not.
    let wrapped = format!(
        "<html><head><title>A plain document</title></head><body><main>{paras}</main>\
         <footer><p>Site furniture down here.</p></footer></body></html>"
    );
    let (tag, _, _) = extract(&wrapped);
    assert_eq!(tag, "main", "<body> displaced a container that held the prose");
}

/// An article whose body *is* a bullet list must not be refused as an index page.
///
/// Reported against `news.hada.io/topic?id=32685`, where the whole page came back
/// `no_article: "IndexPage"`. The body is a `<section itemprop="articleBody">` holding fourteen
/// `<li>`, and because roughly a third of their text sits inside links the group scored
/// `mean_link_density` 0.36 — over the 0.3 bar that makes a repeated group a listing. At 61% of the
/// page's prose it also passed the 0.55 dominance share, so the listing veto fired and withheld the
/// article. Nothing was wrong with the region: the semantic anchor had already selected it.
///
/// A summary written as bullet points is the house style of a whole class of site, so the veto has
/// to be able to tell "this page is an index" from "this article is a list".
#[test]
fn an_article_whose_body_is_a_bullet_list_is_not_an_index_page() {
    let bullets: String = (0..14)
        .map(|i| {
            format!(
                "<li><strong>Point {i}</strong>: a sentence of summary long enough to matter, \
                 citing <a href=\"https://example.test/{i}\">a source with a fairly long link \
                 label</a> as most of its text.</li>"
            )
        })
        .collect();
    let html = format!(
        "<html><head><title>A summary in bullets | Example</title></head><body><main>\
         <article><h1>A summary in bullets</h1>\
         <section itemprop=\"articleBody\"><ul>{bullets}</ul></section>\
         </article></main></body></html>"
    );

    let (tag, text, _) = extract(&html);
    assert_ne!(tag, "none", "the page was refused: an article whose body is a list");
    assert!(text.contains("Point 13"), "the last bullet is missing: {text}");
    assert!(text.contains("Point 0"), "the first bullet is missing: {text}");
}

/// The other half: a genuine index page must still be refused.
///
/// The containment test needs both halves — an anchor *and* the group inside it. An anchor alone
/// would disarm the veto on any page with a `<main>`, which is most of them. Here the item list is
/// the page rather than an article's body, so nothing declares it to be article content and the
/// veto still fires.
#[test]
fn a_front_page_of_links_is_still_an_index_page() {
    let items: String = (0..14)
        .map(|i| {
            format!(
                "<li><h2><a href=\"https://example.test/story-{i}\">Story number {i} with a \
                 headline long enough to carry the page's text</a></h2></li>"
            )
        })
        .collect();
    let html = format!(
        "<html><head><title>Front page | Example</title></head><body><main>\
         <ul class=\"stories\">{items}</ul></main></body></html>"
    );

    let (tag, _, _) = extract(&html);
    assert_eq!(tag, "none", "a front page of story links was served as an article");
}

/// A page whose `<body>` is mostly links has no article, and must not fall back to a fragment.
///
/// The `news.hada.io` front page reported `no_article: null` and served a **92-byte** article: one
/// story headline. Every candidate that actually held the page — `<main>`, `<article>` and the
/// wrappers, all at 92% of its prose — was correctly rejected at the link-density floor, and then
/// the argmax was taken over what survived, which was a single story `<div>` that had squeaked
/// under it.
///
/// The test is the floor already applied to every candidate, aimed at `<body>` itself, so there is
/// no new constant. It separates the corpus by a real margin too: its link-heaviest page is `heise`
/// at 0.734, against 0.89 for that front page.
///
/// # Why the shapes below are all different
///
/// The repeated-template veto would otherwise catch this first and the test would pass without the
/// fix — which is what a first version of it did. Ten shapes with two members each stay under
/// `MIN_GROUP`, so no group forms, no veto fires, and the only thing that can refuse the page is
/// the rule under test. The unlinked `<div>` at the end is the fragment that used to win.
///
/// The real front page gets there differently and could not be reduced to this: its items each
/// carry an author and a timestamp, so `micro_metadata_ratio` is 1.00 and the group reads as a
/// comment thread rather than a listing.
#[test]
fn a_page_that_is_mostly_links_has_no_article() {
    const SHAPES: [&str; 10] = [
        "<p>{}</p>",
        "<div><span>{}</span></div>",
        "<h3>{}</h3>",
        "<blockquote>{}</blockquote>",
        "<div><em>{}</em></div>",
        "<section>{}</section>",
        "<div><strong>{}</strong></div>",
        "<h4>{}</h4>",
        "<div><small>{}</small></div>",
        "<figure>{}</figure>",
    ];
    let mut links = String::new();
    let mut n = 0;
    for shape in SHAPES {
        for _ in 0..2 {
            let a = format!(
                "<a href=\"https://example.test/{n}\">Link label number {n} carrying this \
                 page's text</a>"
            );
            links.push_str(&shape.replace("{}", &a));
            n += 1;
        }
    }
    let fragment = "<div><span>A promoted headline with no link on it at all</span></div>";

    let html = format!(
        "<html><head><title>Front | Example</title></head><body><main>{links}{fragment}</main>\
         </body></html>"
    );
    let (tag, text, _) = extract(&html);
    assert_eq!(tag, "none", "a page of nothing but links was served as an article: {text}");

    // The guard: a link-heavy *article* still extracts. `heise` sits at 0.734 in the corpus, so the
    // margin below the floor has to be usable rather than theoretical.
    let prose = "A paragraph of real body text, long enough that the page is predominantly prose \
                 even though it also carries a great many links in its furniture. ";
    let html = format!(
        "<html><head><title>A linky article | Example</title></head><body><main><article>\
         <p>{prose}</p><p>{prose}</p><p>{prose}</p><p>{prose}</p>{links}</article></main>\
         </body></html>"
    );
    let (tag, text, _) = extract(&html);
    assert_ne!(tag, "none", "a link-heavy article was refused");
    assert!(text.contains("A paragraph of real body text"), "the body was lost: {text}");
}
