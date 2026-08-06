//! Metadata extraction against real documents, including the verbatim invariant.
//!
//! These live here rather than in `legibility-core` because they need a parser, and core has none.

use legibility_core::meta::{extract, ws_normalize, Source, Transform};
use legibility_core::Limits;
use legibility_dom::BuildArena;

fn parse(html: &str) -> legibility_core::Arena {
    BuildArena::parse_to_arena(html, Limits::DEFAULT).0
}

/// The executable form of the no-mangling promise (plan §1.4).
///
/// For every candidate whose only transforms are ours-that-preserve plus the parser's entity
/// decoding, the value must equal the whitespace-normalized span it came from. No allowlist and no
/// exceptions — a transform that cannot satisfy this does not ship.
fn assert_verbatim(arena: &legibility_core::Arena, m: &legibility_core::Metadata) {
    let mut checked = 0usize;
    for (field, c) in &m.alternatives {
        if c.is_ws_only() {
            let re = ws_normalize(arena.span_text(c.span_start, c.span_end));
            assert_eq!(
                re, c.value,
                "VERBATIM INVARIANT violated for field `{field}` from {}: \
                 span re-derives to {re:?} but value is {:?}",
                c.source.as_str(),
                c.value
            );
            checked += 1;
        }
        assert!(c.verify_verbatim(arena), "verify_verbatim disagreed for `{field}`");
    }
    assert!(checked > 0, "no ws-only candidates were checked — the test proved nothing");
}

#[test]
fn open_graph_and_title_are_both_kept_with_provenance() {
    let arena = parse(
        r#"<html><head>
        <title>Real Headline | Example Site</title>
        <meta property="og:title" content="Real Headline">
        <meta property="og:site_name" content="Example Site">
        <meta name="author" content="Kim Minji">
        <meta property="article:published_time" content="2026-08-06T09:00:00+09:00">
        </head><body><h1>Real Headline</h1><p>body</p></body></html>"#,
    );
    let m = extract(&arena);
    assert_verbatim(&arena, &m);

    let t = m.title.expect("a title");
    // og:title outranks <title> because it is a declaration of this field, not a tab string.
    assert_eq!(t.source, Source::OpenGraph);
    assert_eq!(t.value, "Real Headline");

    assert_eq!(m.authors.len(), 1);
    assert_eq!(m.authors[0].value, "Kim Minji");

    let (_, d) = m.published.expect("a date");
    assert!(d.tz_known, "an explicit +09:00 offset must be recognized");
    assert_eq!(d.raw, "2026-08-06T09:00:00+09:00");
}

#[test]
fn a_title_containing_a_dash_is_not_mangled() {
    // This is the exact case Readability gets wrong: it splits on " - " and guesses.
    let arena = parse(
        r#"<html><head>
        <title>Rust 1.75 - what changed and why</title>
        <meta property="og:site_name" content="Example Site">
        </head><body><p>x</p></body></html>"#,
    );
    let m = extract(&arena);
    assert_verbatim(&arena, &m);

    let t = m.title.expect("a title");
    assert_eq!(
        t.value, "Rust 1.75 - what changed and why",
        "the separator is part of the headline and must survive"
    );
    // The tail is not the site name, so no narrowing is offered at all.
    assert!(
        m.title_without_site_name.is_none(),
        "suffix removal must not fire when the tail is not the site name"
    );
}

#[test]
fn a_proven_site_suffix_is_offered_separately_never_in_place() {
    let arena = parse(
        r#"<html><head>
        <title>Real Headline | Example Site</title>
        <meta property="og:site_name" content="Example Site">
        </head><body><p>x</p></body></html>"#,
    );
    let m = extract(&arena);
    assert_verbatim(&arena, &m);

    // The primary title is untouched. A caller that wants the raw string still has it.
    assert_eq!(m.title.as_ref().unwrap().value, "Real Headline | Example Site");

    let stripped = m.title_without_site_name.expect("a narrowed title");
    assert_eq!(stripped.value, "Real Headline");
    assert!(stripped.transforms.contains(&Transform::SpanNarrowed));
    // Narrowing means the span shrank rather than the string being rewritten.
    assert!(stripped.span_end < m.title.as_ref().unwrap().span_end);
}

#[test]
fn korean_titles_with_middle_dot_are_not_split() {
    // U+30FB appears inside ordinary Korean and Japanese titles. Treating it as a separator
    // would cut real headlines in half.
    let arena = parse(
        r#"<html><head>
        <title>설계・구현 노트</title>
        <meta property="og:site_name" content="노트">
        </head><body><p>x</p></body></html>"#,
    );
    let m = extract(&arena);
    assert_verbatim(&arena, &m);
    assert_eq!(m.title.unwrap().value, "설계・구현 노트");
    assert!(m.title_without_site_name.is_none());
}

#[test]
fn microdata_and_time_elements_are_read() {
    let arena = parse(
        r#"<html><body>
        <article itemscope>
          <h2 itemprop="headline">Microdata Headline</h2>
          <span itemprop="author">Park Jisoo</span>
          <time datetime="2026-08-06">Aug 6</time>
          <p>body text here</p>
        </article></body></html>"#,
    );
    let m = extract(&arena);
    assert_verbatim(&arena, &m);
    assert_eq!(m.title.expect("title").value, "Microdata Headline");
    assert!(m.authors.iter().any(|a| a.value == "Park Jisoo"));
    let (_, d) = m.published.expect("date");
    assert_eq!(d.iso8601.as_deref(), Some("2026-08-06"));
    assert!(!d.tz_known, "a date-only value has no timezone and must not claim one");
}

#[test]
fn authors_stay_a_list_and_are_never_joined_then_resplit() {
    let arena = parse(
        r#"<html><head>
        <meta property="article:author" content="Smith, John">
        <meta name="author" content="Lee Hana">
        </head><body><p>x</p></body></html>"#,
    );
    let m = extract(&arena);
    assert_verbatim(&arena, &m);
    // "Smith, John" is one person. Joining authors into a string and splitting on commas is
    // precisely how that becomes two.
    assert!(m.authors.iter().any(|a| a.value == "Smith, John"));
    assert!(m.authors.iter().any(|a| a.value == "Lee Hana"));
    for a in &m.authors {
        assert!(!a.value.is_empty());
    }
}

#[test]
fn absent_metadata_is_absent_rather_than_invented() {
    let arena = parse("<html><body><p>just text, no metadata at all</p></body></html>");
    let m = extract(&arena);
    assert!(m.title.is_none(), "no title element and no meta: must be None");
    assert!(m.authors.is_empty());
    assert!(m.published.is_none());
    assert!(m.site_name.is_none());
}

#[test]
fn entity_decoding_is_reported_not_claimed_as_verbatim() {
    let arena = parse(
        r#"<html><head><title>A &amp; B &lt;tag&gt;</title></head><body><p>x</p></body></html>"#,
    );
    let m = extract(&arena);
    let t = m.title.clone().expect("title");
    // The parser already decoded these before we saw them, so the value differs from the source
    // bytes. That is disclosed as a transform rather than passed off as untouched.
    assert_eq!(t.value, "A & B <tag>");
    assert!(t.transforms.contains(&Transform::EntityDecodedByParser));
    assert_verbatim(&arena, &m);
}

#[test]
fn whitespace_in_attributes_is_normalized_and_the_span_still_re_derives() {
    let arena = parse(
        "<html><head><meta property=\"og:title\" content=\"  spaced   out \n title  \">\
         </head><body><p>x</p></body></html>",
    );
    let m = extract(&arena);
    assert_verbatim(&arena, &m);
    let t = m.title.expect("title");
    assert_eq!(t.value, "spaced out title");
    assert!(t.transforms.contains(&Transform::WsNormalized));
}

#[test]
fn published_and_modified_are_never_conflated() {
    let arena = parse(
        r#"<html><head>
        <meta property="article:published_time" content="2026-01-01T00:00:00Z">
        <meta property="article:modified_time" content="2026-08-06T00:00:00Z">
        </head><body><p>x</p></body></html>"#,
    );
    let m = extract(&arena);
    assert_verbatim(&arena, &m);
    assert_eq!(m.published.as_ref().unwrap().1.raw, "2026-01-01T00:00:00Z");
    assert_eq!(m.modified.as_ref().unwrap().1.raw, "2026-08-06T00:00:00Z");
}

#[test]
fn hostile_and_malformed_documents_do_not_break_the_invariant() {
    for html in [
        "",
        "<title>",
        "<meta property=og:title>",
        "<meta property=\"og:title\" content=\"\">",
        "<html><head><title>a</title><title>b</title></head>",
        "<meta itemprop=headline content=\"x\"><meta itemprop=headline content=\"y\">",
        "<time datetime>x</time>",
        "<html lang=\"\"><body><h1></h1></body></html>",
    ] {
        let arena = parse(html);
        let m = extract(&arena);
        // Not every fixture yields a candidate, so this checks the invariant directly rather
        // than through the helper's "at least one" assertion.
        for (field, c) in &m.alternatives {
            assert!(
                c.verify_verbatim(&arena),
                "invariant broken for `{field}` on {html:?}"
            );
        }
    }
}
