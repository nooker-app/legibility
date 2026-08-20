//! Whole-output snapshots for every page shape that has been diagnosed by hand.
//!
//! # Why this exists alongside `selection.rs`
//!
//! `selection.rs` asserts the one fact each case was about — this tag was chosen, that string is
//! absent. Those assertions are precise and they are also narrow, and four defects in a row reached
//! a user through code that every targeted assertion passed: a `<h1>` duplicated because a title
//! carried a site suffix, a dateline that survived on one site and not another, blank-line soup
//! between paragraphs, emphasis that never reached the page. The 130-page corpus ratchet did not
//! see them either — it scores a **token multiset over text**, so whitespace, attributes, images,
//! tags and the shape verdict are all invisible to it.
//!
//! So this pins the *entire* output for each shape. A change anywhere in the pipeline that alters
//! anything a consumer receives shows up here as a diff, whether or not anyone thought to assert
//! on it.
//!
//! # Why the snapshot is the canonical JSON
//!
//! Not a report written for this test: [`legibility_dom::json::extraction_json`] is the single
//! serializer every host calls, and guarantee S3 compares `blake3` of exactly this string across
//! native, `wasm32-wasip1` and headless Chrome. Snapshotting it means the fixtures below are also
//! a determinism corpus, and there is no second output format to drift from the real one. It is
//! re-indented for the diff only — never reordered, never re-encoded.
//!
//! # Fixtures
//!
//! Self-authored (plan D9 forbids committing third-party HTML) but the structure is the site's,
//! reduced to the smallest thing that still reproduces. Each is named for the page it came from.
//!
//! # Updating
//!
//! ```text
//! cargo test -p legibility-dom --test snapshot -- --ignored bless
//! ```
//!
//! Read the diff before committing it. A snapshot that is blessed without being read is a test
//! that agrees with whatever the code does, which is no test at all.

use std::path::{Path, PathBuf};

use legibility_core::Limits;
use legibility_dom::BuildArena;

/// Every case, as `(snapshot name, page source)`.
///
/// One list, walked by both the assertion and the bless run, so the two cannot disagree about what
/// the corpus is.
fn cases() -> Vec<(&'static str, String)> {
    vec![
        ("reddit-link-submission", reddit_link_submission()),
        ("reddit-text-post", reddit_text_post()),
        ("beebs-two-comments", beebs_thread(2)),
        ("beebs-eighteen-comments", beebs_thread(18)),
        ("news-article-with-dateline", news_article_with_dateline()),
        ("cards-are-not-a-listing", cards_are_not_a_listing()),
        ("serialization-oddities", serialization_oddities()),
        ("empty-comment-section", empty_comment_section()),
        ("geeknews-topic-one-comment", geeknews_topic(1)),
        ("geeknews-topic-two-comments", geeknews_topic(2)),
        ("timeline-events-are-not-comments", timeline_events()),
        ("nav-inside-main-is-not-body", nav_inside_main()),
        ("reddit-credit-bar-without-comments", reddit_credit_bar_only()),
        ("reddit-nested-thread", reddit_nested_thread()),
    ]
}

// ---------------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------------

/// Reddit, S-2 with a **nested** thread: five roots, one carrying two replies and a reply to one of
/// them.
///
/// Reported against `reddit.com/r/rss/comments/1vsw2th`: replies were not separated and a comment
/// carried its descendants' content. Two defects, and the first hid the second.
///
/// Rendered Reddit builds a thread from `<shreddit-comment>` elements with **no class**, and
/// identity expansion required one — so nested replies could not be recognised at all and neither
/// could the root that held them. Before the fix this fixture returned four of its eight comments,
/// `depth_source: Flat`, every depth 0: the four childless roots, with the whole nested branch and
/// its parent missing.
///
/// With them found, the reported symptom appeared: a `<shreddit-comment>` with replies *contains*
/// them, so the root's `text` and `html` carried its children's authors, timestamps and bodies.
fn reddit_nested_thread() -> String {
    fn comment(id: &str, author: &str, depth: u8, text: &str, children: &str) -> String {
        format!(
            // Attributes and child order copied from the live page: a class every comment
            // shares, a `<details>` collapse control, the meta slot, the body, then nested
            // replies as *direct children* alongside them.
            "<shreddit-comment class=\"relative [contain:style] col-start-2\" \
             thingid=\"t1_{id}\" author=\"{author}\" depth=\"{depth}\" \
             permalink=\"/r/rss/comments/x/_/{id}/\">\
             <details><summary>More replies</summary></details>\
             <div slot=\"commentMeta\"><a href=\"/user/{author}/\">{author}</a>\
               <time datetime=\"2026-08-1{depth}T10:00:00Z\">2d ago</time></div>\
             <div><div><p>{text}</p></div></div>\
             {children}</shreddit-comment>"
        )
    }

    let grandchild =
        comment("g1", "carol", 2, "I switched to Feedly years ago and never looked back.", "");
    let child_one = comment(
        "k1",
        "bob",
        1,
        "Yes, I still use RSS every single day for tech news.",
        &grandchild,
    );
    let child_two =
        comment("k2", "erin", 1, "Same here, though I moved to a self-hosted reader.", "");
    let roots = [
        comment(
            "r1",
            "alice",
            0,
            "Do you still use RSS in 2026? I certainly do, every day.",
            &format!("{child_one}{child_two}"),
        ),
        comment("r2", "dave", 0, "RSS is the last good protocol on the open web today.", ""),
        comment("r3", "fay", 0, "I keep about forty feeds and read them each morning.", ""),
        comment("r4", "gus", 0, "Newsletters replaced it for me, honestly speaking here.", ""),
        comment("r5", "hal", 0, "Feed readers are niche now but the niche is healthy.", ""),
    ]
    .concat();

    format!(
        "<html><head><title>Do you still use RSS and feed reader apps in 2026? : r/rss</title>\
         <meta property=\"og:site_name\" content=\"Reddit\"></head><body><main>\
         <shreddit-post permalink=\"/r/rss/comments/x/\" comment-count=\"8\">\
           <div slot=\"credit-bar\"><a href=\"/r/rss/\">r/rss</a>\
             <time datetime=\"2026-08-10T20:36:22Z\">2d ago</time>\
             <a href=\"/user/asker/\">asker</a></div>\
           <h1 slot=\"title\">Do you still use RSS and feed reader apps in 2026?</h1>\
           <shreddit-post-text-body slot=\"text-body\"><div><p>I am curious whether people still \
             keep a feed reader in their daily rotation, or whether it has all moved to \
             newsletters.</p></div></shreddit-post-text-body>\
         </shreddit-post>\
         <shreddit-comment-tree id=\"comment-tree\">{roots}</shreddit-comment-tree>\
         </main></body></html>"
    )
}

/// Reddit, S-1: the submission is a title and an outbound link, and its "body" is one bare anchor
/// the poster pasted wrongly. Reported as a defect — the pointer was being served as an article.
fn reddit_link_submission() -> String {
    format!(
        "<html><head><title>Free news APIs compared : r/rss</title>\
         <meta property=\"og:site_name\" content=\"Reddit\"></head><body><main>\
         {}\
         <shreddit-comment-tree id=\"comment-tree\"></shreddit-comment-tree>\
         </main></body></html>",
        shreddit_post(
            "<div><p>[<a href=\"https://example.test/news-apis\">\
             https://example.test/news-apis</a>]</p></div>"
        )
    )
}

/// Reddit, S-2: a text post. Three things happen here at once, and each of them shipped broken
/// once — the `<title>` carries ` : r/rss` so the duplicated `<h1>` only matches directionally,
/// the credit bar precedes the headline, and the body is prose rather than a pointer.
fn reddit_text_post() -> String {
    format!(
        "<html><head>\
         <title>New to RSS here, why can't i use this website to add to my feed? : r/rss</title>\
         <meta property=\"og:site_name\" content=\"Reddit\"></head><body><main>\
         {}\
         <shreddit-comment-tree id=\"comment-tree\"></shreddit-comment-tree>\
         </main></body></html>",
        shreddit_post(
            "<div><p>I'm very new to RSS and just started setting up apps (Feeder)</p>\
             <p>I follow football closely and wanted to make a list, but I can't seem to add this \
             website for some reason. Any help would be great.</p>\
             <p><a href=\"https://www.example.test/es/\">https://www.example.test/es/</a></p></div>"
        )
    )
}

/// The shell both Reddit fixtures share: a credit bar, a headline that restates the `<title>`
/// minus its site suffix, the body, and an action row of buttons.
fn shreddit_post(body: &str) -> String {
    format!(
        "<shreddit-post permalink=\"/r/rss/comments/x/\" comment-count=\"0\">\
           <div slot=\"credit-bar\"><a href=\"/r/rss/\">r/rss</a><span>•</span>\
             <time datetime=\"2026-07-24T20:36:22Z\">12d ago</time>\
             <a href=\"/user/awesome5ftw/\">awesome5ftw</a></div>\
           <h1 slot=\"title\">New to RSS here, why can't i use this website to add to my feed?</h1>\
           <shreddit-post-text-body slot=\"text-body\">{body}</shreddit-post-text-body>\
           <div slot=\"action-row\"><button>Upvote<span>1</span></button>\
             <button>Downvote</button><button>Reply</button><button>Share</button></div>\
         </shreddit-post>"
    )
}

/// beebs.hada.io, S-2. Two calls, two defects.
///
/// `n = 18` is `t/60`, which worked. `n = 2` is `t/61`, which returned its first comment as the
/// article: with two members no group formed, nothing was masked, and the widest prose block on
/// the page won. The pair is admitted because the page states `댓글 2` and the pair is exactly two.
///
/// Three details are load-bearing and easy to lose in a tidy-up.
///
/// The dateline **follows** the headline here where Reddit puts it in front, which is why the
/// byline search looks both ways. The submission's author is a plain `<span>` and not an anchor,
/// so there is no outbound link inside the header — with one there, it gets picked as the
/// submission's link and the page is read as a pointer with no body at all. And the two comments
/// differ *structurally*, not cosmetically: the first hangs its reactions off a third child
/// `<div class="comment-actions">`, the second has no such child and nests them inside its meta
/// line instead. That is what pushes their signatures apart inside the first 24 descendants and
/// leaves their shared `(tag, class)` as the only thing that groups them.
fn beebs_thread(n: usize) -> String {
    let mut s = format!(
        "<html><head><title>인앱 브라우저에서 패스키를 지원할 방법은 없는걸까요? - GeekNews BeeBS</title>\
         <meta property=\"og:site_name\" content=\"BeeBS\"></head>\
         <body><main class=\"thread-view\">\
         <article class=\"thread-detail\">\
           <header class=\"thread-detail-head\">\
             <div class=\"thread-detail-title-line\">\
               <span class=\"category-chip thread-category\">질문</span>\
               <h1>인앱 브라우저에서 패스키를 지원할 방법은 없는걸까요?</h1></div>\
             <div class=\"thread-detail-meta\" aria-label=\"글 정보\">\
               <span class=\"thread-detail-author\">댕댕</span>\
               <span class=\"thread-detail-view\">조회 199</span>\
               <span class=\"thread-detail-comments\">댓글 {n}</span>\
               <time class=\"thread-detail-created\" datetime=\"2026-07-24T02:39:52.607Z\">\
                 2026-07-24 11:39:52</time></div></header>\
           <div class=\"markdown-body thread-body\"><p>어쩌다가 카톡의 인앱 브라우저로 BeeBS에 \
             로그인을 시도했는데 패스키는 동작을 안하더라고요</p>\
             <p>RSS 리더앱으로도 똑같이 동작해서 저는 그냥 안되나보다 하고 있지만 혹시 다른분들은 \
             잘 되시는지 궁금하네요</p></div>\
           <div class=\"thread-actions\"><div class=\"reaction-bar\" aria-label=\"리액션\">\
             <div class=\"reaction-picker-wrap\"><button class=\"emoji-reaction-add\" type=\"button\" \
               aria-label=\"리액션 추가\"><span class=\"emoji-reaction-add-text\">반응</span>\
               </button></div></div></div>\
         </article>\
         <section class=\"comments-section\"><h2>댓글</h2><ol class=\"comment-list\">"
    );
    for i in 0..n {
        // The reaction count lives *inside* a `<button>`, so it is a control label rather than
        // prose -- it must not reach the comment's text.
        let reactions = "<div class=\"comment-reaction-bar\" aria-label=\"댓글 리액션\">\
             <button class=\"emoji-reaction-button\" type=\"button\" title=\"❤️\" aria-label=\"❤️ 1\">\
             <span class=\"reaction-emoji\" aria-hidden=\"true\">❤️</span><span>1</span></button>\
             </div>";
        // The structural divergence between members, verbatim from the page: same class, different
        // shape. See the doc comment.
        let (in_meta, after_body) = if i % 2 == 0 {
            ("", format!("<div class=\"comment-actions\">{reactions}</div>"))
        } else {
            ("<span class=\"comment-meta-actions\"></span>", String::new())
        };
        s.push_str(&format!(
            "<li class=\"comment-tree-item\">\
             <article id=\"comment-{i}\" class=\"comment-row\" style=\"--comment-depth:0\" \
               tabindex=\"-1\">\
             <div class=\"comment-meta\"><a class=\"author-profile-link\" href=\"/u/u{i}\">사용자{i}</a>\
             <time datetime=\"2026-07-24T0{h}:00:00.000Z\">2026-07-24 0{h}:00:00</time>\
             {in_meta}</div>\
             <div class=\"markdown-body\"><p>답변 {i} 입니다. 이 댓글은 본문보다 훨씬 길고 밀도도 \
             높아서 통계만으로는 본문을 이깁니다. 실제로 그런 일이 일어났습니다.</p></div>\
             {after_body}</article></li>",
            h = i % 9,
        ));
    }
    s.push_str(
        "</ol><form class=\"comment-form\"><button type=\"button\">댓글을 남겨보세요.</button>\
                </form></section></main></body></html>",
    );
    s
}

/// A news page with a dateline in the same position a discussion page puts its credit bar.
///
/// The negative half of the byline rule, and the reason the 130-page corpus cannot regress on it:
/// this page has no comment furniture, so `discussion_shape` is absent and the rule is
/// unreachable. If that gate ever softens, this snapshot loses its dateline and says so.
fn news_article_with_dateline() -> String {
    "<html><head><title>Markets close higher — Daily Ledger</title>\
     <meta property=\"og:site_name\" content=\"Daily Ledger\"></head><body><main><article>\
     <div class=\"dateline\">By A Reporter \
       <time datetime=\"2026-07-24T20:36:22Z\">24 July 2026</time></div>\
     <h1>Markets close higher as industrials rally</h1>\
     <p>Equities finished the session up across the board, with the broadest gains in industrials \
       and a late rally in energy that carried the index past its opening level.</p>\
     <p>Traders pointed to the morning's inventory numbers, which came in well under the range \
       most desks had been working from, as the reason the afternoon turned.</p>\
     <figure><img src=\"/img/floor.jpg\" alt=\"Traders on the exchange floor\">\
       <figcaption>The floor at close.</figcaption></figure>\
     </article></main></body></html>"
        .to_string()
}

/// Pairs of look-alike siblings that must not cost the page its article.
///
/// The corpus pages `mozilla-2` and `mercurial` are this shape, and both went to **F1 0.000** —
/// article withheld as an index page — when identity-gathered groups became indistinguishable
/// from structurally-matched ones on their way out of `merge_by_signature`. Grouping a pair on
/// shared `(tag, class)` is weak evidence; it is enough to mask a two-comment thread and not
/// enough to withhold an article, and that asymmetry is what this pins.
fn cards_are_not_a_listing() -> String {
    let mut s = String::from(
        "<html><head><title>Release notes for version 6.9 — Project</title></head>\
         <body><main><article>\
         <h1>Release notes for version 6.9</h1>\
         <p>This release closes the long-standing report about revision ordering, and changes the \
           default for two configuration keys that almost nobody had set deliberately.</p>\
         <p>Upgrading in place is supported from any 6.x release. The repository format is \
           unchanged, so a downgrade remains possible for as long as no new revisions land.</p>\
         </article>\
         <aside class=\"rail\">",
    );
    // Pairs, not triples, and each pair's members differ internally so no structural signature
    // matches -- exactly the shape `identity_group` picks up.
    for (i, extra) in ["<span>new</span>", "<em>pinned</em>"].iter().enumerate() {
        s.push_str(&format!(
            "<div class=\"card\"><a href=\"/p/{i}\">Another page worth reading, number {i}</a>\
             {extra}</div>"
        ));
    }
    for i in 0..2 {
        s.push_str(&format!("<div class=\"tag-pill\"><a href=\"/t/{i}\">topic {i}</a></div>"));
    }
    s.push_str("</aside></main></body></html>");
    s
}

/// Everything the serializer has got wrong, on one page.
///
/// Blank-line soup between paragraphs; emphasis that has to survive inline without welding words
/// together; a custom element that must be unwrapped rather than emitted; a cell holding only an
/// image; a link whose scheme is not allowed and must lose its `href` while keeping its text.
fn serialization_oddities() -> String {
    "<html><head><title>Notes on serialization — Field Guide</title></head><body><main><article>\
     <h1>Notes on serialization</h1>\n\n\n\
     <p>The first paragraph, which is <em>emphasised</em> in the middle and must not come out \
       as <b>onewordwelded</b> either side of it.</p>\n\n   \n\n\
     <p>A second paragraph, far enough from the first that a naive collapse would join them into \
       a single run of text with no break at all between the two.</p>\n\n\
     <custom-callout><p>Inside a custom element, which is not a tag any consumer knows, so the \
       wrapper goes and the paragraph stays.</p></custom-callout>\
     <table><tr><td><img src=\"/img/diagram.png\" alt=\"A diagram of the pipeline\"></td>\
       <td></td></tr></table>\
     <p>A link with <a href=\"javascript:alert(1)\">a scheme that is refused</a> and one with \
       <a href=\"https://example.test/ok\">a scheme that is not</a>.</p>\
     </article></main></body></html>"
        .to_string()
}

/// An ordinary article whose comment section is present and empty.
///
/// "Be the first to comment" is short and very dense, and it used to win on density — every site
/// with an empty comment section has this shape. The page must come out unreshaped: an article,
/// no discussion verdict, no comments.
fn empty_comment_section() -> String {
    "<html><head><title>How the cache is warmed — Field Guide</title></head><body><main>\
     <article><h1>How the cache is warmed</h1>\
     <p>The warmer runs from the same queue as everything else, which is why it is bounded by the \
       same concurrency limit and why a cold start is slower than the numbers suggest.</p>\
     <p>Each entry is fetched once and shared, so a burst of readers for the same key costs one \
       fetch rather than one per reader.</p></article>\
     <section class=\"comments\"><div><p>Be the first to comment</p>\
       <p>Nobody's responded to this post yet. Add your thoughts and get the conversation \
       going.</p></div></section>\
     </main></body></html>"
        .to_string()
}

/// news.hada.io, S-2. One `<article>` holds everything; only the body is marked.
///
/// Reported three times over, at `topic?id=32176`, `32177`, `32178`, `32179` and `32183`: the
/// extracted body opened with the credit bar and ran on into the comments —
///
/// ```text
///   (warp.dev) 1 P by GN⁺ 20시간전 | ★ favorite | 댓글 1개  Warp Terminal의 …
/// ```
///
/// `<article>` is a genuine semantic anchor and the wrong one, and it is also the widest viable
/// candidate, so no statistic rescues it. `itemprop="articleBody"` is the only thing on the page that
/// makes the narrower claim; plan §1.10.4 listed it from the start and nothing read it.
///
/// `n = 1` is the additional case: a lone comment cannot form a repeated group, so the page states
/// `댓글 1개` and we still report none. The body must be right regardless, which is what this pins.
fn geeknews_topic(comments: usize) -> String {
    let mut s = format!(
        "<html><head><title>Warp Agent CLI | GeekNews</title>\
         <meta property=\"og:site_name\" content=\"GeekNews\"></head><body>\
         <header><nav><a href=\"/\">GeekNews</a><a href=\"/new\">최신글</a>\
           <a href=\"/comments\">댓글</a><a href=\"/ask\">Ask</a></nav></header>\
         <main><article><div class=\"topic-table\">\
           <div class=\"topic\">\
             <div class=\"vote\"><a href=\"javascript:vote(1)\">▲</a></div>\
             <div class=\"topictitle link\"><a href=\"https://warp.dev/blog/agent-cli\">\
               <h1>Warp Agent CLI</h1></a> <span class=\"topicurl\">(warp.dev)</span></div>\
             <div class=\"topicinfo\"><span>1</span>P by <a href=\"/@neo\">GN⁺</a> \
               <time datetime=\"2026-08-05T22:05:33+09:00\">20시간전</time> | \
               <a href=\"javascript:fav(1)\">★ favorite</a> | \
               <a href=\"topic?id=1\">댓글 {comments}개</a></div>\
             <div class=\"topic_contents\"><div>\
               <section id=\"topic_contents\" class=\"article-content\" itemprop=\"articleBody\">\
                 <ul><li>Warp Terminal의 다중 모델 코딩 에이전트를 <strong>독립형 CLI</strong>로 \
                   제공해 다른 터미널에서도 사용할 수 있음</li>\
                 <li>tmux와 유사한 <strong>멀티플렉싱 구조</strong>로 PTY 연결을 관리함</li></ul>\
                 <h2>선호하는 터미널에서 실행하는 독립형 에이전트</h2>\
                 <p>전문 개발자를 위한 비용 최적화형 다중 모델 하네스로, 작업 복잡도 기반 자동 \
                   라우팅을 기본 제공함</p>\
               </section></div></div></div></div>\
           <div class=\"related-topics\"><h2>함께 보면 좋은 글</h2><ul>\
             <li><div><a href=\"/topic?id=2\">Herdr - AI Agent 시대를 위한 터미널 워크스페이스</a>\
               </div></li>\
             <li><div><a href=\"/topic?id=3\">telepty — 여러 머신의 에이전트 세션 컨트롤 플레인</a>\
               </div></li></ul></div>\
           <div class=\"comment_form\"><form class=\"write_comment\">\
             <textarea placeholder=\"친절하고 점잖은 댓글을 남겨주세요\"></textarea>\
             <input type=\"submit\" value=\"댓글 달기\"></form></div>\
           <div id=\"comment_thread\" class=\"comment_thread descendant\">"
    );
    for i in 0..comments {
        s.push_str(&format!(
            "<div class=\"comment_row\" id=\"cid{i}\" style=\"--depth:0\">\
             <div class=\"vote\"><a href=\"javascript:votec({i})\">▲</a></div>\
             <div class=\"commentinfo\"><a href=\"/@u{i}\">사용자{i}</a> \
               <a href=\"/topic?id=1#cid{i}\"><time datetime=\"2026-08-05T23:0{i}:00+09:00\">\
                 19시간전</time></a>\
               <a href=\"javascript:child_toggle({i})\"><span class=\"commentfold\">[-]</span></a>\
               </div>\
             <div class=\"commentTD\"><span class=\"comment_contents\">\
               <p>이 기능은 <strong>정말</strong> 기대되네요. 터미널을 떠나지 않고 작업할 수 있다는 \
                 점이 특히 좋습니다.</p>\
               <ul><li>첫째 이유</li><li>둘째 이유</li></ul></span></div></div>"
        ));
    }
    s.push_str("</div></article></main></body></html>");
    s
}

/// A GitHub pull request's timeline: events that carry an author and a time but no authored prose.
///
/// `added 3 commits` and `mentioned this pull request` cleared every gate a comment clears — an event
/// has a byline exactly as a comment does — and three of them outnumbered the real conversation, so
/// commit records were reported as comments. Plan §1.7 S-6 calls them `Boilerplate`; the signal that
/// separates them is that nobody wrote a paragraph.
fn timeline_events() -> String {
    let mut s = String::from(
        "<html><head><title>perf(hub): reuse datasource connections by selenehyun · Pull Request \
         #39</title></head><body><main>\
         <div class=\"comment-body\"><h1>perf(hub): reuse datasource connections</h1>\
         <p>queryDatabase builds a datasource and closes it again on every sync, which costs a \
           connection handshake per source per interval.</p>\
         <p>This reuses a pooled datasource keyed by type and config, and closes it when the last \
           borrower returns it.</p></div>\
         <div id=\"timeline\">",
    );
    for (i, (verb, hash)) in
        [("added 3 commits", "8918818"), ("added 2 commits", "8b85ec7")].iter().enumerate()
    {
        s.push_str(&format!(
            "<div class=\"TimelineItem\" id=\"event-{i}\">\
             <a href=\"/selenehyun\">selenehyun</a> <span>and others</span> {verb} \
             <a href=\"#commits-pushed-{hash}\"><time datetime=\"2026-08-05T12:4{i}:50+09:00\">\
               August 5, 2026</time></a>\
             <div class=\"commits\"><pre><code>{hash}</code></pre>\
               <a href=\"/commit/{hash}\">perf(lynqhub): reuse datasource connections across \
                 syncs</a></div></div>"
        ));
    }
    s.push_str(
        "<div class=\"TimelineItem\" id=\"event-2\">\
         <a href=\"/selenehyun\">selenehyun</a> mentioned this pull request \
         <a href=\"#ref-1\"><time datetime=\"2026-08-05T07:09:53Z\">Aug 5, 2026</time></a>\
         <div><a href=\"/pull/41\">fix(lynqhub): decide rollout skew on API state</a>\
           <span>#41</span><span>Merged</span></div></div>\
         </div></main></body></html>",
    );
    s
}

/// Navigation *inside* `<main>`, which is where an application-shaped page puts it.
///
/// GitHub's repository bar and tab strip live under the same `<main>` as the pull request, so the
/// extracted body opened with `Code Issues 4 Pull requests 3 … Conversation Commits Checks`. A
/// `<nav>` is the page saying it is navigation. The repository bar carries no landmark at all and is
/// caught by name plus structure instead — its id contains `header`, it holds no paragraph, and it is
/// almost entirely links.
fn nav_inside_main() -> String {
    "<html><head><title>Reuse datasource connections · Pull Request #39</title></head><body>\
     <main>\
     <div id=\"repository-container-header\"><ul>\
       <li><a href=\"/notifications\">Notifications</a></li>\
       <li><a href=\"/fork\">Fork 6</a></li>\
       <li><a href=\"/stargazers\">Star 78</a></li></ul></div>\
     <nav aria-label=\"Repository\"><a href=\"/code\">Code</a><a href=\"/issues\">Issues 4</a>\
       <a href=\"/pulls\">Pull requests 3</a><a href=\"/actions\">Actions</a>\
       <a href=\"/insights\">Insights</a></nav>\
     <nav aria-label=\"Pull request tabs\"><a href=\"#conv\">Conversation</a>\
       <a href=\"#commits\">Commits</a><a href=\"#checks\">Checks</a>\
       <a href=\"#files\">Files changed</a></nav>\
     <div class=\"comment-body\">\
       <p>queryDatabase builds a datasource and closes it again on every sync, which costs a full \
         connection handshake per source per interval.</p>\
       <p>This change reuses a pooled datasource keyed by type and config, and closes it only when \
         the last borrower gives it back.</p></div>\
     <footer><a href=\"/terms\">Terms</a><a href=\"/privacy\">Privacy</a></footer>\
     </main></body></html>"
        .to_string()
}

/// The same Reddit post as [`reddit_text_post`], captured with **neither** of the two signals the
/// credit-bar rule used to depend on.
///
/// Reported a fourth time, on `r/rss/comments/1v5n91g`, with the bar back at the top of the body:
///
/// ```text
///   rss 커뮤니티로 이동  r/rss  13일 전  awesome5ftw  I'm very new to RSS and just started …
/// ```
///
/// Both exclusions failed at once, which is what made it look like the earlier fix had been undone.
/// The byline rule is gated on the page being recognised as a discussion, and this capture carries
/// no comment furniture at all — a post with zero replies, saved from a rendered DOM — so `shape`
/// came back `None` and the rule was unreachable. The duplicate-heading rule needs the document's
/// declared title, and it did not match either, so the `<h1>` stayed too.
///
/// Neither signal is present here on purpose: no `<title>`, no comment tree, no custom elements —
/// only the structure, which is the one thing that was true both times. A credit bar is a block
/// holding a `<time>` whose text is almost entirely links; a news dateline is plain text with at
/// most a byline link, and [`news_article_with_dateline`] pins that it survives.
fn reddit_credit_bar_only() -> String {
    "<html lang=\"ko\"><body>\
     <main dir=\"ltr\">\
     <div><span>\
       <span><a href=\"https://www.reddit.com/r/rss/\">rss 커뮤니티로 이동</a></span>\
       <div>\
         <span><span><a href=\"https://www.reddit.com/r/rss/\">r/rss</a></span>\
           <time datetime=\"2026-07-24T20:36:22.796Z\" title=\"2026년 7월 24일 금요일\">13일 전</time>\
           </span>\
         <div><span><div><a href=\"https://www.reddit.com/user/awesome5ftw/\">\
           <span dir=\"auto\">awesome5ftw</span></a></div></span></div>\
       </div>\
     </span></div>\
     <h1 dir=\"auto\">New to RSS here, why can't i use this website to add to my feed?</h1>\
     <div><div><div dir=\"auto\">\
     <p>I'm very new to RSS and just started setting up apps (Feeder) I follow football closely and \
       wanted to make a list, but I can't seem to add this website for some reason. Any help would \
       be great.</p>\
     <p><a href=\"https://www.sport.es/es/\" rel=\"noopener nofollow ugc\">\
       https://www.sport.es/es/</a></p>\
     <p>This is the website.</p>\
     </div></div></div>\
     </main></body></html>"
        .to_string()
}

// ---------------------------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------------------------

/// The canonical JSON for one page, re-indented.
fn snapshot_of(html: &str) -> String {
    let (arena, hit) = BuildArena::parse_to_arena(html, Limits::DEFAULT);
    let out = legibility_core::extract_all(&arena, Limits::DEFAULT);
    indent(&legibility_dom::json::extraction_json(&arena, &out, hit, None))
}

/// Put one JSON value per line, indented by nesting depth.
///
/// Purely presentational: characters are copied through in order and only whitespace is inserted,
/// so nothing about the snapshot can differ from the bytes the determinism gate hashes. String
/// contents are copied verbatim — a `{` inside a title must not indent anything.
fn indent(json: &str) -> String {
    let mut out = String::with_capacity(json.len() * 2);
    let mut depth = 0usize;
    let mut in_str = false;
    let mut escaped = false;
    let newline = |out: &mut String, depth: usize| {
        out.push('\n');
        for _ in 0..depth {
            out.push_str("  ");
        }
    };
    for c in json.chars() {
        if in_str {
            out.push(c);
            escaped = c == '\\' && !escaped;
            if c == '"' && !escaped {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_str = true;
                escaped = false;
                out.push(c);
            }
            '{' | '[' => {
                depth += 1;
                out.push(c);
                newline(&mut out, depth);
            }
            '}' | ']' => {
                depth = depth.saturating_sub(1);
                newline(&mut out, depth);
                out.push(c);
            }
            ',' => {
                out.push(c);
                newline(&mut out, depth);
            }
            _ => out.push(c),
        }
    }
    out.push('\n');
    out
}

fn snapshot_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/snapshots/{name}.json"))
}

#[test]
fn every_shape_produces_the_output_it_was_committed_with() {
    let mut differ: Vec<String> = Vec::new();
    let mut missing: Vec<&str> = Vec::new();

    for (name, html) in cases() {
        let got = snapshot_of(&html);
        match std::fs::read_to_string(snapshot_path(name)) {
            Ok(want) if want == got => {}
            Ok(want) => differ.push(format!("{name}:\n{}", first_difference(&want, &got))),
            Err(_) => missing.push(name),
        }
    }

    assert!(
        missing.is_empty(),
        "no committed snapshot for: {missing:?}\n\
         run `cargo test -p legibility-dom --test snapshot -- --ignored bless` to record them"
    );
    assert!(
        differ.is_empty(),
        "output changed for {} shape(s):\n\n{}\n\
         If the new output is correct, bless it -- and read the diff first, because these fixtures \
         exist to catch changes nobody asserted on.",
        differ.len(),
        differ.join("\n\n")
    );
}

/// The first line that differs, with a little context. Enough to recognise the change from the
/// test output; `git diff` after blessing is where the whole picture is.
fn first_difference(want: &str, got: &str) -> String {
    let (w, g): (Vec<&str>, Vec<&str>) = (want.lines().collect(), got.lines().collect());
    let at = (0..w.len().max(g.len())).find(|&i| w.get(i) != g.get(i)).unwrap_or(0);
    let from = at.saturating_sub(2);
    let mut s = String::new();
    for i in from..at {
        s.push_str(&format!("   {}\n", w.get(i).copied().unwrap_or("")));
    }
    s.push_str(&format!("  -{}\n", w.get(at).copied().unwrap_or("<end of file>")));
    s.push_str(&format!("  +{}", g.get(at).copied().unwrap_or("<end of file>")));
    s
}

#[test]
#[ignore = "writes the snapshots; run deliberately and read the diff"]
fn bless() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots");
    std::fs::create_dir_all(&dir).expect("creating the snapshot directory");
    for (name, html) in cases() {
        let path = snapshot_path(name);
        let got = snapshot_of(&html);
        let changed = std::fs::read_to_string(&path).map_or(true, |old| old != got);
        std::fs::write(&path, &got).expect("writing a snapshot");
        println!("{} {name}", if changed { "wrote  " } else { "unchanged" });
    }
}

/// Write each fixture's source HTML where `tools/rjs-baseline` can read it.
///
/// The fixtures are the only pages in this repo whose correct extraction is stated by *us* rather
/// than by an engine, which makes them the only place a comparison against Readability.js means
/// anything. On mozilla's 130 pages `expected.html` is R.js's own output, so R.js scores 1.000
/// there by construction and "we agree with R.js 91% of the time" is the whole of what that
/// corpus can say.
///
/// Ignored, and writes outside the repo: plan D9 keeps HTML out of the history, and while these
/// bytes are self-authored, the generated copies are derived files with a source of truth ten
/// lines up.
///
/// ```text
/// cargo test -p legibility-dom --test snapshot -- --ignored dump_fixtures
/// node tools/rjs-baseline/run.js --out rjs.json /tmp/legibility-fixtures/*.html
/// ```
#[test]
#[ignore = "writes fixture HTML to /tmp for the Readability.js comparison"]
fn dump_fixtures() {
    let dir = std::env::temp_dir().join("legibility-fixtures");
    std::fs::create_dir_all(&dir).expect("creating the fixture directory");
    for (name, html) in cases() {
        let path = dir.join(format!("{name}.html"));
        std::fs::write(&path, &html).expect("writing a fixture");
        println!("wrote {}", path.display());
    }
}
