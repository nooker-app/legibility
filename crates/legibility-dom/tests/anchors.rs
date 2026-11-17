//! Self-authored reductions of https://nooker.app/@tim/hello-nook and its RSS body.
//! Only its anchor vocabulary and TOC/ref/backlink structure are retained, not the article prose.

use std::collections::BTreeSet;

use legibility_core::arena::AttrName;
use legibility_core::{Arena, Limits, NodeId, TagId};
use legibility_dom::serialize::{serialize_region, serialize_region_excluding, SerializeOptions};
use legibility_dom::BuildArena;
use legibility_sanitize::{Article, Profile, UserContent};

fn sanitize<P: Profile>(html: &str) -> String {
    let (arena, _) = BuildArena::parse_to_arena(html, Limits::DEFAULT);
    let body = (0..arena.len()).find(|&i| arena.tag[i] == TagId::BODY).unwrap();
    serialize_region::<P>(&arena, NodeId(body as u32), SerializeOptions::default()).0.into_inner()
}

fn nodes_with(arena: &Arena, attr: AttrName) -> Vec<(NodeId, &str)> {
    (0..arena.len())
        .filter_map(|i| {
            let id = NodeId(i as u32);
            arena.attr(id, attr).map(|value| (id, value))
        })
        .collect()
}

fn assert_links_resolve(html: &str, count: usize) {
    let (arena, _) = BuildArena::parse_to_arena(html, Limits::DEFAULT);
    let ids = nodes_with(&arena, AttrName::ID);
    let unique: BTreeSet<_> = ids.iter().map(|(_, id)| *id).collect();
    assert_eq!(unique.len(), ids.len(), "duplicate output IDs: {html}");
    let links = nodes_with(&arena, AttrName::HREF);
    assert_eq!(links.iter().filter(|(_, href)| href.starts_with('#')).count(), count);
    for (_, href) in links {
        if let Some(fragment) = href.strip_prefix('#') {
            assert!(unique.contains(fragment), "missing target for {href}: {html}");
        }
    }
}

// A deliberately independent URL encoding helper for the fixture, not the sanitizer's decoder.
fn fragment(id: &str) -> String {
    id.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b':') {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

fn hello_nook(prefix: &str) -> String {
    let headings = [
        "만들게-된-이유",
        "첫-여정",
        "편의-기능",
        "실-사용",
        "nook-plus",
        "plus-기능은-아직-초대-코드가-있어야-합니다",
        "앞으로의-방향",
        "마치며",
        "다운로드",
    ];
    let mut html = String::from("<html><body><main><article><div><ul>");
    for heading in headings {
        html.push_str(&format!(
            "<li><a href=\"#{}\">{heading}</a></li>",
            fragment(&format!("{prefix}{heading}"))
        ));
    }
    html.push_str("</ul></div>");
    for (i, heading) in headings.iter().enumerate() {
        let tag = if i == 5 || i == 8 { "h3" } else { "h2" };
        html.push_str(&format!("<{tag} id=\"{prefix}{heading}\">{heading}</{tag}><p>\
            This section explains a reading feature, with enough original prose to select the article. \
            Its references lead to supporting notes and return to this paragraph.</p>"));
        let notes: &[usize] = match i {
            0 => &[1],
            2 => &[2, 3, 4],
            3 => &[5],
            4 => &[6],
            _ => &[],
        };
        for n in notes {
            html.push_str(&format!(
                "<p>Read the supporting note<sup id=\"{prefix}fnref:{n}\">\
                <a href=\"#{prefix}fn:{n}\">{n}</a></sup>.</p>"
            ));
        }
    }
    html.push_str("<div class=footnotes><hr><ol>");
    for n in 1..=6 {
        html.push_str(&format!(
            "<li id=\"{prefix}fn:{n}\"><p>Supporting detail for reference {n}. \
            <a href=\"#{prefix}fnref:{n}\">↩</a></p></li>"
        ));
    }
    html.push_str("</ol></div></article></main></body></html>");
    html
}

fn extracted_hello_nook(prefix: &str) {
    let source = hello_nook(prefix);
    let (arena, hit) = BuildArena::parse_to_arena(&source, Limits::DEFAULT);
    let outcome = legibility_core::extract_all(&arena, Limits::DEFAULT);
    let article = outcome.selection.article.expect("article selected");
    let (html, report) = serialize_region_excluding::<Article>(
        &arena,
        article,
        SerializeOptions::default(),
        &outcome.article_exclusions,
    );
    assert!(!report.truncated);
    assert_links_resolve(html.as_str(), 21);
    let (result, _) = BuildArena::parse_to_arena(html.as_str(), Limits::DEFAULT);
    let targets = nodes_with(&result, AttrName::ID);
    assert_eq!(targets.len(), 21);
    for (tag, count) in [("h2", 7), ("h3", 2), ("sup", 6), ("li", 6)] {
        assert_eq!(targets.iter().filter(|(n, _)| result.tag_name(*n) == Some(tag)).count(), count);
    }
    // Verify the edges, not only that they terminate somewhere: TOC -> heading, ref -> note,
    // note -> the very same ref. Each of the 21 destinations is used exactly once.
    let mut destinations = BTreeSet::new();
    for (link, href) in nodes_with(&result, AttrName::HREF) {
        let target = targets.iter().find(|(_, id)| Some(*id) == href.strip_prefix('#')).unwrap().0;
        assert!(destinations.insert(target));
        let label = result.own_text(NodeId(link.0 + 1));
        match result.tag_name(target).unwrap() {
            "h2" | "h3" => assert_eq!(result.own_text(NodeId(target.0 + 1)), label),
            "li" => {
                let end = result.subtree_end[target.idx()] as usize;
                let back = (target.idx()..end)
                    .find_map(|i| result.attr(NodeId(i as u32), AttrName::HREF))
                    .unwrap();
                let reference = result.parent[link.idx()];
                assert_eq!(result.attr(reference, AttrName::ID), back.strip_prefix('#'));
            }
            "sup" => assert_eq!(label, "↩"),
            other => panic!("unexpected target {other}"),
        }
    }
    // Exercise the same final JSON path used by native CLI and WASM.
    let json = legibility_dom::json::extraction_json(&arena, &outcome, hit, None);
    assert_eq!(json.matches(" href=\\\"#lg-").count(), 21);
    assert_eq!(json.matches(" id=\\\"lg-").count(), 21);
    assert_eq!(sanitize::<Article>(html.as_str()), html.as_str());
}

#[test]
fn hello_nook_toc_and_six_footnotes_round_trip_after_extraction() {
    extracted_hello_nook("");
}

#[test]
fn hello_nook_rss_prefixed_anchors_round_trip_after_extraction() {
    extracted_hello_nook("hello-nook-");
}

#[test]
fn legacy_empty_targets_duplicates_and_prefixed_ids_survive_resanitizing() {
    let source = "<a href='#old'>legacy</a><a href='#dup'>first</a><a href='#lg-1'>prefixed</a>\
        <a name=old></a><p id=dup>first</p><p id=dup>second</p><p id=lg-1>third</p>";
    for html in [sanitize::<Article>(source), sanitize::<UserContent>(source)] {
        assert_links_resolve(&html, 3);
        assert!(!html.contains(" name="));
        assert!(html.contains("</a>"));
    }
    let article = sanitize::<Article>(source);
    assert!(article.contains("<a id=\"lg-1\"></a>"));
    assert!(article.contains("href=\"#lg-2\">first"));
    assert!(article.contains("href=\"#lg-4\">prefixed"));
    assert_eq!(sanitize::<Article>(&article), article);
    let ugc = sanitize::<UserContent>(source);
    assert_eq!(sanitize::<UserContent>(&ugc), ugc);
}

#[test]
fn href_rewriting_preserves_external_fragments_and_security_policy() {
    let html = sanitize::<Article>(
        "<a href='#%ED%95%9C'>local</a>\
        <p id='한' onclick='alert(1)' style='color:red'>target</p>\
        <a href='https://example.test/#%ED%95%9C'>external</a>\
        <a href='/elsewhere#한'>relative</a><a href='?page=2#한'>query</a>\
        <a href='//example.test/#한'>network</a>\
        <a href='java&#9;script:alert(1)'>unsafe</a>\
        <a name='attributes'></a><script id='code'>alert(1)</script>\
        <style id='css'>p{}</style><svg id='svg'></svg><form name='host'><input></form>",
    );
    assert_links_resolve(&html, 1);
    for href in
        ["https://example.test/#%ED%95%9C", "/elsewhere#한", "?page=2#한", "//example.test/#한"]
    {
        assert!(html.contains(&format!("href=\"{href}\"")), "{html}");
    }
    for forbidden in [
        "onclick",
        "style=",
        "alert(1)",
        "<script",
        "<style",
        "<svg",
        "<form",
        "<input",
        "name=",
        "id=\"attributes\"",
    ] {
        assert!(!html.contains(forbidden), "{forbidden}: {html}");
    }
}

#[test]
fn dropped_excluded_flattened_and_truncated_targets_cannot_capture_links() {
    let source = "<article><p><a href='#hidden'>hidden</a><a href='#excluded'>excluded</a>\
        <a href='#deep'>deep</a><a href='#later'>later</a><a href='#lg-1'>unresolved</a></p>\
        <p id=hidden hidden>hidden</p><p id=excluded>excluded</p>\
        <div><div><p id=deep>deep</p></div></div><p id=later>later</p></article>";
    let (arena, _) = BuildArena::parse_to_arena(source, Limits::DEFAULT);
    let article = (0..arena.len()).find(|&i| arena.tag[i] == TagId::ARTICLE).unwrap();
    let excluded =
        nodes_with(&arena, AttrName::ID).into_iter().find(|(_, id)| *id == "excluded").unwrap().0;
    let (html, report) = serialize_region_excluding::<Article>(
        &arena,
        NodeId(article as u32),
        SerializeOptions { max_render_depth: 3, ..SerializeOptions::default() },
        &[excluded],
    );
    assert!(report.depth_clamped);
    assert_links_resolve(html.as_str(), 1);
    assert!(html.as_str().contains("href=\"#lg-1\">later"));
    let (short, report) = serialize_region_excluding::<Article>(
        &arena,
        NodeId(article as u32),
        SerializeOptions { max_output_bytes: 140, ..SerializeOptions::default() },
        &[excluded],
    );
    assert!(report.truncated);
    assert_links_resolve(short.as_str(), 0);
}

#[test]
fn expanding_short_fragments_respects_the_output_budget() {
    let source =
        format!("<article><p id=x>target</p>{}</article>", "<a href=#x>x</a>".repeat(1000));
    let (arena, _) = BuildArena::parse_to_arena(&source, Limits::DEFAULT);
    let article = (0..arena.len()).find(|&i| arena.tag[i] == TagId::ARTICLE).unwrap();
    let (html, report) = serialize_region::<Article>(
        &arena,
        NodeId(article as u32),
        SerializeOptions { max_output_bytes: 512, ..SerializeOptions::default() },
    );
    assert!(report.truncated);
    // As elsewhere in the serializer, the last element and closing tags may exceed the cap;
    // rewriting all preceding links must not add unbounded extra bytes after that decision.
    assert!(html.as_str().len() <= 512 + 32, "{} bytes", html.as_str().len());
    assert!(!html.as_str().contains(&format!("#lg-{}", arena.len())), "placeholder leaked");
    assert_eq!(sanitize::<Article>(html.as_str()), html.as_str());
}
