//! Canonical JSON output. The single serializer for every host.
//!
//! The CLI, the local demo server and the WASM module all call [`extraction_json`]. That is not
//! tidiness: guarantee S3 compares `blake3` of this output across native, `wasm32-wasip1` and
//! headless Chrome, so two serializers would mean the determinism gate fails on a field ordering
//! difference rather than on anything real.
//!
//! Hand-written rather than derived. `schema_version` and key order are part of the contract, and a
//! derive macro would put them at the mercy of a dependency's field ordering.

use legibility_core::comments::CommentSet;
use legibility_core::meta::{Candidate, DateValue, Metadata};
use legibility_core::{Arena, LimitsHit, NodeId, NodeKind, Outcome, TagId};
use legibility_sanitize::{Article, UserContent};

use crate::serialize::{serialize_region_excluding, SerializeOptions};

/// Serialize a complete extraction.
///
/// `url` is echoed when the caller fetched one, so a saved result records where it came from.
#[must_use]
pub fn extraction_json(
    arena: &Arena,
    out: &Outcome,
    hit: LimitsHit,
    url: Option<&str>,
) -> String {
    extraction_json_limited(arena, out, hit, url, legibility_core::Limits::DEFAULT)
}

/// [`extraction_json`] honouring a caller's [`Limits`].
///
/// `max_output_bytes` was declared, documented per host profile — 4 MiB in a browser, 2 MiB in an
/// iOS share extension, chosen because "a Share Extension is killed for memory far sooner than its
/// host app" — and never read by anything. Every serialization used
/// [`SerializeOptions::default`]'s 16 MiB, so both host caps were decoration and
/// `LimitsHit::output_bytes` could never become true. A limit nobody enforces is worse than no
/// limit: a caller reads it and plans around a bound that is not there.
#[must_use]
pub fn extraction_json_limited(
    arena: &Arena,
    out: &Outcome,
    hit: LimitsHit,
    url: Option<&str>,
    limits: legibility_core::Limits,
) -> String {
    let opts = SerializeOptions {
        max_output_bytes: limits.max_output_bytes,
        ..SerializeOptions::default()
    };
    let mut s = String::from("{\"schema_version\":1");
    s.push_str(",\"url\":");
    match url {
        Some(u) => push_str(&mut s, u),
        None => s.push_str("null"),
    }

    s.push_str(",\"article\":");
    match out.selection.article {
        Some(n) => {
            // Comments and the section furniture around them, decided in the pipeline: the
            // serializer is told what to skip rather than deciding for itself.
            let comment_nodes: &[NodeId] = &out.article_exclusions;
            // A link submission has no body. The region around it is a credit bar, the title and
            // the URL -- and all three are already returned as fields, which is exactly how a
            // pointer came back looking like a broken article:
            //
            //   "r/rss · 2d ago rangeva  [Github] Free news APIs differ ...  [ https://... ]  0"
            //
            // So the body is empty by construction and the payload is `url`. Nothing is lost:
            // the title is `metadata.title` and the destination is right there.
            let root = out
                .shape
                .is_some_and(|sh| sh.kind == legibility_core::DiscussionShape::LinkOnly);
            let ser = if root {
                None
            } else {
                Some(serialize_region_excluding::<Article>(arena, n, opts, comment_nodes))
            };
            let rep = ser.as_ref().map_or_else(Default::default, |(_, r)| *r);
            s.push_str("{\"html\":");
            push_str(&mut s, ser.as_ref().map_or("", |(h, _)| h.as_str()));
            s.push_str(",\"text\":");
            push_str(
                &mut s,
                &if root {
                    String::new()
                } else {
                    prose_text_excluding(arena, n, comment_nodes)
                },
            );
            s.push_str(",\"tag\":");
            push_str(
                &mut s,
                arena.tag.get(n.idx()).copied().and_then(TagId::known_name).unwrap_or("?"),
            );
            // What kind of thing this is, not merely where it was found. A reader renders a
            // `discussion-root` as a headline that links out and an `article` as prose;
            // inferring that from an empty `html` would be a guess.
            s.push_str(",\"kind\":");
            push_str(&mut s, if root { "discussion-root" } else { "article" });
            s.push_str(",\"url\":");
            match out
                .shape
                .and_then(|sh| sh.link)
                .and_then(|a| arena.attr(a, legibility_core::arena::AttrName::HREF))
            {
                Some(href) => push_str(&mut s, href),
                None => s.push_str("null"),
            }
            s.push_str(",\"prose_len\":");
            // Net of the exclusions, so `prose_len`, `text` and `html` describe the same thing. The
            // raw subtree total would say 441 bytes next to a 280-byte `text` and leave a caller to
            // guess which is wrong.
            // Only the exclusions actually inside the region. `article_exclusions` lists every
            // comment on the page, and most pages put the thread *outside* the chosen region --
            // subtracting those reported a 432-byte submission as 0 bytes.
            let region_end = arena.subtree_end.get(n.idx()).copied().unwrap_or(0) as usize;
            let excluded: u32 = comment_nodes
                .iter()
                .filter(|c| c.idx() > n.idx() && c.idx() < region_end)
                .map(|c| arena.prose_len.get(c.idx()).copied().unwrap_or(0))
                .sum();
            let net = arena
                .prose_len
                .get(n.idx())
                .copied()
                .unwrap_or(0)
                .saturating_sub(excluded);
            // Zero for a link submission, because `text` and `html` are empty. The invariant this
            // field exists to keep is that all three describe the same thing.
            s.push_str(&if root { 0 } else { net }.to_string());
            s.push_str(",\"confidence\":");
            s.push_str(&out.selection.confidence.to_string());
            s.push_str(",\"dropped_subtrees\":");
            s.push_str(&rep.dropped_subtrees.to_string());
            s.push_str(",\"unwrapped\":");
            s.push_str(&rep.unwrapped_elements.to_string());
            s.push_str(",\"truncated\":");
            s.push_str(if rep.truncated { "true" } else { "false" });
            s.push_str(",\"dispersion_floor\":");
            s.push_str(if out.selection.dispersion_floor_used { "true" } else { "false" });
            // How the region was chosen, not just which one. "<main> said so" and "it won the
            // statistics" are different claims and a caller weighing our answer needs to tell them
            // apart -- as does anyone reading a bug report about the wrong region.
            s.push_str(",\"from_semantic_anchor\":");
            s.push_str(if out.selection.region_from_semantic_anchor { "true" } else { "false" });
            s.push_str(",\"signal_conflict\":");
            s.push_str(if out.selection.signal_conflict { "true" } else { "false" });
            // Permanently false in v1, and part of the contract rather than a TODO: calibrating a
            // confidence needs a labelled corpus, and claiming it without one is worse than
            // admitting its absence.
            s.push_str(",\"calibrated\":false}");
        }
        None => s.push_str("null"),
    }
    // Always emitted, `null` when absent. A key that appears only on some outcomes makes the object
    // a different shape per page, which every strict consumer -- and the snapshot suite -- has to
    // special-case. `article` was already unconditional; its counterpart was not.
    s.push_str(",\"no_article\":");
    match out.selection.no_article {
        Some(r) => push_str(&mut s, &alloc_fmt(r)),
        None => s.push_str("null"),
    }

    s.push_str(",\"comments\":");
    s.push_str(&comments_json(arena, &out.comments, opts));
    s.push_str(",\"metadata\":");
    s.push_str(&metadata_json(arena, &out.metadata));

    s.push_str(",\"page_kind\":");
    push_str(&mut s, if out.is_listing { "listing" } else { "article-or-discussion" });
    s.push_str(",\"comment_mask_reverted\":");
    s.push_str(if out.comment_mask_reverted { "true" } else { "false" });
    s.push_str(",\"group_count\":");
    s.push_str(&out.group_count.to_string());

    s.push_str(",\"diagnostics\":{\"node_count\":");
    s.push_str(&arena.len().to_string());
    for (k, v) in [
        ("page_prose_len", arena.prose_len.first().copied().unwrap_or(0)),
        ("page_control_len", arena.control_len.first().copied().unwrap_or(0)),
        ("page_hidden_len", arena.hidden_len.first().copied().unwrap_or(0)),
        ("page_alt_len", arena.alt_len.first().copied().unwrap_or(0)),
        // Reported but read by no feature: script/style source, template contents, comments.
        // Split out of page_hidden_len because counting it as hidden *text* made any container
        // that inlines a JS bundle look impure. See legibility_core::a11y::TextRole::Inert.
        ("page_inert_len", arena.inert_len.first().copied().unwrap_or(0)),
    ] {
        s.push_str(",\"");
        s.push_str(k);
        s.push_str("\":");
        s.push_str(&v.to_string());
    }
    // Which submission shape was found, or null on the vast majority of pages that are not
    // discussions. Reported because "we judged this a pointer" is a claim a bug report needs to
    // be able to name.
    s.push_str(",\"discussion_shape\":");
    match out.shape {
        Some(sh) => push_str(&mut s, sh.kind.name()),
        None => s.push_str("null"),
    }
    s.push_str(",\"limits_hit\":[");
    let mut first = true;
    for (name, on) in [
        ("input_bytes", hit.input_bytes),
        ("nodes", hit.nodes),
        ("depth", hit.depth),
        ("attrs_per_node", hit.attrs_per_node),
        ("attr_bytes", hit.attr_bytes),
        ("comment_items", hit.comment_items),
        ("output_bytes", hit.output_bytes),
        ("step_budget", hit.step_budget),
    ] {
        if on {
            if !first {
                s.push(',');
            }
            push_str(&mut s, name);
            first = false;
        }
    }
    s.push_str("]}}");
    s
}

fn comments_json(arena: &Arena, set: &CommentSet, opts: SerializeOptions) -> String {
    let mut s = String::from("{\"count\":");
    s.push_str(&set.items.len().to_string());
    s.push_str(",\"depth_source\":");
    match set.depth_source {
        Some(d) => push_str(&mut s, &alloc_fmt(d)),
        None => s.push_str("null"),
    }

    let c = &set.completeness;
    s.push_str(",\"completeness\":{\"present\":");
    s.push_str(&c.present.to_string());
    s.push_str(",\"claimed_total\":");
    match c.claimed_total {
        Some(v) => s.push_str(&v.to_string()),
        None => s.push_str("null"),
    }
    s.push_str(",\"truncated\":");
    s.push_str(if c.truncated { "true" } else { "false" });
    s.push_str(",\"reason\":");
    match c.reason {
        Some(r) => push_str(&mut s, &alloc_fmt(r)),
        None => s.push_str("null"),
    }
    s.push_str(",\"continuation\":[");
    for (i, u) in c.continuation.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        push_str(&mut s, u);
    }
    s.push_str("]}");

    s.push_str(",\"items\":[");
    for (i, it) in set.items.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str("{\"author\":");
        match &it.author {
            Some(a) => push_str(&mut s, a),
            None => s.push_str("null"),
        }
        s.push_str(",\"timestamp\":");
        match &it.timestamp {
            Some(t) => push_str(&mut s, t),
            None => s.push_str("null"),
        }
        s.push_str(",\"depth\":");
        s.push_str(&it.depth.to_string());
        s.push_str(",\"parent\":");
        match it.parent {
            Some(p) => s.push_str(&p.to_string()),
            None => s.push_str("null"),
        }
        s.push_str(",\"permalink\":");
        match &it.permalink {
            Some(p) => push_str(&mut s, p),
            None => s.push_str("null"),
        }
        s.push_str(",\"deleted\":");
        s.push_str(if it.flags.deleted { "true" } else { "false" });
        s.push_str(",\"text\":");
        push_str(&mut s, &it.text);
        // Formatted HTML, through the *UserContent* profile. Comments arrive with lists, quotes,
        // code blocks and emphasis, and only `text` existed until now -- so every consumer rendered
        // a flattened paragraph and the formatting read as stripped when it had never been emitted.
        //
        // `UserContent` and not `Article`: this is attacker-controlled input (plan §1.8), and the
        // phantom type on `SanitizedHtml<P>` is what stops it being rendered down an article path.
        // Images are off, media is dropped, and every surviving link gets
        // `rel="nofollow noopener noreferrer"`.
        s.push_str(",\"html\":");
        match it.body.or(Some(it.node)) {
            Some(n) => {
                // The byline is excluded, not because it is unsafe but because it is already
                // `author` and `timestamp`: when `body` is the whole comment it would render twice.
                let (h, _) = serialize_region_excluding::<UserContent>(arena, n, opts, &it.byline);
                push_str(&mut s, h.as_str());
            }
            None => s.push_str("null"),
        }
        s.push('}');
    }
    s.push(']');
    s.push('}');
    s
}

fn metadata_json(arena: &Arena, m: &Metadata) -> String {
    let mut s = String::from("{");
    for (i, (key, c)) in [
        ("title", m.title.as_ref()),
        ("title_without_site_name", m.title_without_site_name.as_ref()),
        ("site_name", m.site_name.as_ref()),
        ("language", m.language.as_ref()),
        ("description", m.description.as_ref()),
        ("canonical_url", m.canonical_url.as_ref()),
    ]
    .into_iter()
    .enumerate()
    {
        if i > 0 {
            s.push(',');
        }
        s.push('"');
        s.push_str(key);
        s.push_str("\":");
        match c {
            Some(c) => s.push_str(&candidate_json(arena, c)),
            None => s.push_str("null"),
        }
    }

    s.push_str(",\"authors\":[");
    for (i, a) in m.authors.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&candidate_json(arena, a));
    }
    s.push(']');

    for (key, d) in [("published", m.published.as_ref()), ("modified", m.modified.as_ref())] {
        s.push_str(",\"");
        s.push_str(key);
        s.push_str("\":");
        match d {
            Some((c, dv)) => s.push_str(&date_json(arena, c, dv)),
            None => s.push_str("null"),
        }
    }

    s.push_str(",\"candidate_count\":");
    s.push_str(&m.alternatives.len().to_string());
    s.push('}');
    s
}

fn candidate_json(arena: &Arena, c: &Candidate) -> String {
    let mut s = String::from("{\"value\":");
    push_str(&mut s, &c.value);
    s.push_str(",\"source\":");
    push_str(&mut s, c.source.as_str());
    s.push_str(",\"confidence\":");
    s.push_str(&c.confidence.to_string());
    s.push_str(",\"span\":[");
    s.push_str(&c.span_start.to_string());
    s.push(',');
    s.push_str(&c.span_end.to_string());
    s.push_str("],\"transforms\":[");
    for (i, t) in c.transforms.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        push_str(&mut s, &alloc_fmt(*t));
    }
    // The executable form of the no-mangling promise, re-checked at serialization time so a
    // consumer can see it rather than take it on faith.
    s.push_str("],\"verbatim_ok\":");
    s.push_str(if c.verify_verbatim(arena) { "true" } else { "false" });
    s.push('}');
    s
}

fn date_json(arena: &Arena, c: &Candidate, d: &DateValue) -> String {
    let mut s = String::from("{\"candidate\":");
    s.push_str(&candidate_json(arena, c));
    s.push_str(",\"raw\":");
    push_str(&mut s, &d.raw);
    s.push_str(",\"iso8601\":");
    match &d.iso8601 {
        Some(v) => push_str(&mut s, v),
        None => s.push_str("null"),
    }
    // Never inferred: an offsetless timestamp is a local wall clock, and guessing shifts it.
    s.push_str(",\"tz_known\":");
    s.push_str(if d.tz_known { "true" } else { "false" });
    s.push('}');
    s
}

/// Prose-only text of a region, whitespace collapsed.
#[must_use]
pub fn prose_text(arena: &Arena, region: NodeId) -> String {
    prose_text_excluding(arena, region, &[])
}

/// [`prose_text`] with subtrees left out.
///
/// Must take the same exclusions as the HTML. `article.html` without the comments and
/// `article.text` with them is worse than either on its own: anything measuring the text -- an F1
/// score, a reading-time estimate, a search index -- would then disagree with what a reader sees.
#[must_use]
pub fn prose_text_excluding(arena: &Arena, region: NodeId, exclude: &[NodeId]) -> String {
    let end = arena.subtree_end.get(region.idx()).copied().unwrap_or(0) as usize;
    let mut out = String::new();
    let mut i = region.idx();
    while i < end {
        if i != region.idx() {
            if let Some(skip) = exclude.iter().find(|n| n.idx() == i) {
                i = arena.subtree_end.get(skip.idx()).copied().unwrap_or(0) as usize;
                continue;
            }
        }
        if arena.kind.get(i).copied() == Some(NodeKind::Text)
            && arena
                .text_role
                .get(i)
                .copied()
                .is_some_and(legibility_core::a11y::TextRole::is_prose)
        {
            for w in arena.own_text(NodeId(i as u32)).split_whitespace() {
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(w);
            }
        }
        i += 1;
    }
    out
}

fn alloc_fmt<T: core::fmt::Debug>(v: T) -> String {
    format!("{v:?}")
}

/// Append `s` as a JSON string literal.
fn push_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str("\\u");
                for shift in [12, 8, 4, 0] {
                    let nibble = ((c as u32) >> shift) & 0xF;
                    out.push(char::from_digit(nibble, 16).unwrap_or('0'));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use legibility_core::Limits;

    fn json_of(html: &str) -> String {
        let (arena, hit) = crate::BuildArena::parse_to_arena(html, Limits::DEFAULT);
        let out = legibility_core::extract_all(&arena, Limits::DEFAULT);
        extraction_json(&arena, &out, hit, None)
    }

    #[test]
    fn control_characters_are_escaped_so_the_output_always_parses() {
        let j = json_of("<html><body><article><p>a\u{1}b\tc</p></article></body></html>");
        assert!(j.contains("\\u0001") || !j.contains('\u{1}'), "raw control char in JSON: {j}");
        assert!(j.starts_with('{') && j.ends_with('}'));
    }

    #[test]
    fn keys_are_present_and_stable_even_when_everything_is_absent() {
        // A key that appears only in later versions is a schema break for consumers that already
        // shipped against its absence, so the shape is fixed regardless of content.
        let j = json_of("");
        for key in [
            "\"schema_version\":1",
            "\"article\":",
            "\"comments\":",
            "\"metadata\":",
            "\"page_kind\":",
            "\"diagnostics\":",
            "\"count\":0",
            "\"candidate_count\":",
        ] {
            assert!(j.contains(key), "missing {key} in {j}");
        }
    }

    #[test]
    fn serialization_is_byte_stable_across_repeated_runs() {
        // The cheap in-process half of guarantee S3. The cross-target half is xtask determinism.
        let html = "<html><head><title>T</title></head><body><article><p>x y z</p></article></body></html>";
        let first = json_of(html);
        for _ in 0..20 {
            assert_eq!(json_of(html), first, "output is not stable across runs");
        }
    }
}
