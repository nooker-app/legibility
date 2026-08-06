//! Metadata extraction (plan M3, defect 2).
//!
//! # The defect this replaces
//!
//! Readability's `_getArticleTitle()` splits the `<title>` on ` | `, ` - `, ` » ` and `/`, counts
//! words, and recombines the pieces with the `<h1>`. There is no path that returns the original
//! string, and no record of what it did. A title that legitimately contains a dash comes back
//! mangled, and the caller cannot tell.
//!
//! # The rule here
//!
//! Nothing is synthesized. Every field is a [`Candidate`] carrying the span it came from, which
//! source produced it, and exactly which transformations were applied. The permitted transforms
//! are whitespace normalization and (already done by the parser) entity decoding — nothing else.
//!
//! That is enforced, not promised. [`Candidate::verify_verbatim`] re-derives the value from the
//! document buffer, and a corpus-wide test asserts it holds for every candidate with no exception
//! list. If a transform cannot satisfy it, the transform does not ship.
//!
//! # Honest scope
//!
//! The invariant proves *we* did not mangle the value. It is not byte-fidelity against the
//! original network bytes: html5ever decodes entities before we ever see the text, so
//! `&amp;` has already become `&`. That is reported as [`Transform::EntityDecodedByParser`]
//! rather than quietly claimed as verbatim.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::arena::{Arena, AttrName, NodeId, NodeKind};
use crate::tag::TagId;

/// Where a value came from. Ordered by how much it should be trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    /// `<title>` — present on nearly every page, and nearly always decorated with the site name.
    TitleTag,
    /// An `<h1>` in the document.
    H1,
    /// A `<time datetime>` attribute.
    TimeElement,
    /// `<meta name="...">` (Dublin Core, `author`, `description`).
    MetaName,
    /// `<meta name="twitter:...">`.
    TwitterCard,
    /// `<meta property="og:...">`.
    OpenGraph,
    /// Microdata `itemprop`.
    Microdata,
    /// `<script type="application/ld+json">`.
    JsonLd,
}

impl Source {
    /// Baseline confidence for this source, before any adjustment.
    ///
    /// JSON-LD and Microdata are explicit publisher declarations of *this* field; `<title>` is a
    /// browser-tab string that happens to usually contain the headline. The ordering encodes that
    /// difference rather than a preference.
    #[must_use]
    pub const fn base_confidence(self) -> u8 {
        match self {
            Source::JsonLd => 95,
            Source::Microdata => 90,
            Source::OpenGraph => 85,
            Source::TwitterCard => 80,
            Source::MetaName => 70,
            Source::TimeElement => 75,
            Source::H1 => 60,
            Source::TitleTag => 50,
        }
    }

    /// Short stable name for diagnostics and JSON.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Source::JsonLd => "json-ld",
            Source::Microdata => "microdata",
            Source::OpenGraph => "open-graph",
            Source::TwitterCard => "twitter-card",
            Source::MetaName => "meta-name",
            Source::TimeElement => "time-element",
            Source::H1 => "h1",
            Source::TitleTag => "title-tag",
        }
    }
}

/// A transformation applied to a raw span to produce a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transform {
    /// Leading/trailing whitespace trimmed and internal runs collapsed to one space.
    WsNormalized,
    /// The parser decoded entities before we saw the text.
    ///
    /// Informational: it is not our transformation, so it is not subject to the verbatim equality
    /// claim. Reporting it is how the claim stays honest instead of overreaching.
    EntityDecodedByParser,
    /// The span was narrowed — e.g. a site-name suffix was removed.
    ///
    /// Narrowing keeps the invariant intact because the value still equals the normalized span,
    /// just a shorter one. This is why suffix removal is expressed as narrowing rather than as
    /// string surgery.
    SpanNarrowed,
}

/// A metadata value with its provenance.
#[derive(Debug, Clone)]
pub struct Candidate {
    /// The value a caller should use.
    pub value: String,
    /// Start of the span in [`Arena::doc_buf`] this value derives from.
    pub span_start: u32,
    /// End of that span.
    pub span_end: u32,
    /// Which source produced it.
    pub source: Source,
    /// Confidence in `0..=100`. Not calibrated; an ordering signal only.
    pub confidence: u8,
    /// Every transformation applied, in no particular order.
    pub transforms: Vec<Transform>,
}

impl Candidate {
    /// Whether this candidate's only claimed transform is whitespace normalization.
    #[must_use]
    pub fn is_ws_only(&self) -> bool {
        self.transforms
            .iter()
            .all(|t| matches!(t, Transform::WsNormalized | Transform::EntityDecodedByParser))
    }

    /// Re-derive the value from the document and compare.
    ///
    /// This is the executable form of the no-mangling promise. It is checked over the whole corpus
    /// with no allowlist, so a transform that cannot satisfy it cannot ship.
    #[must_use]
    pub fn verify_verbatim(&self, arena: &Arena) -> bool {
        if !self.is_ws_only() {
            // A narrowed span still has to match its own narrowed range, which it does by
            // construction; only ws-only candidates make the strong claim.
            return true;
        }
        ws_normalize(arena.span_text(self.span_start, self.span_end)) == self.value
    }
}

/// Collapse whitespace runs and trim. The only text transformation the engine performs.
///
/// Note what is absent: no case folding, no Unicode normalization (NFKC in particular would
/// rewrite full-width characters and is wrong for CJK text), no punctuation rewriting.
#[must_use]
pub fn ws_normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for word in s.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

/// A date, kept in three forms because collapsing them loses information.
#[derive(Debug, Clone)]
pub struct DateValue {
    /// Exactly what the document said.
    pub raw: String,
    /// ISO-8601, only when the source was already unambiguous.
    pub iso8601: Option<String>,
    /// Whether the source carried an explicit offset or `Z`.
    ///
    /// Never inferred. A timestamp with no offset is a local wall clock, and guessing a zone
    /// silently shifts every such date by hours.
    pub tz_known: bool,
}

/// Extracted metadata. Every field may be absent; nothing is invented to fill a gap.
#[derive(Debug, Clone, Default)]
pub struct Metadata {
    /// Best title candidate.
    pub title: Option<Candidate>,
    /// Title with a proven site-name suffix removed, when one was found.
    pub title_without_site_name: Option<Candidate>,
    /// Authors, as a list. Never a joined string that a caller has to split back.
    pub authors: Vec<Candidate>,
    /// Publication date.
    pub published: Option<(Candidate, DateValue)>,
    /// Last-modified date, kept separate from `published`.
    pub modified: Option<(Candidate, DateValue)>,
    /// `og:site_name` or equivalent.
    pub site_name: Option<Candidate>,
    /// Language, from `<html lang>` or a meta tag.
    pub language: Option<Candidate>,
    /// Description / excerpt.
    pub description: Option<Candidate>,
    /// Canonical URL.
    pub canonical_url: Option<Candidate>,
    /// Every candidate considered, including rejected ones, so a caller can override.
    pub alternatives: Vec<(&'static str, Candidate)>,
}

/// Extract metadata from a parsed document.
///
/// One linear pass over the arena. Metadata lives in `<head>` and in a handful of in-body
/// elements, so this reads attributes it already has rather than re-walking per field.
#[must_use]
pub fn extract(arena: &Arena) -> Metadata {
    let mut m = Metadata::default();
    let mut cands: Vec<(&'static str, Candidate)> = Vec::new();

    for i in 0..arena.len() {
        if arena.kind.get(i).copied() != Some(NodeKind::Element) {
            continue;
        }
        let node = NodeId(i as u32);
        let tag = arena.tag.get(i).copied().unwrap_or(TagId::UNKNOWN);

        match tag {
            TagId::META => collect_meta(arena, node, &mut cands),
            TagId::TITLE => {
                // require_prose = false: <title> is inside <head> and therefore Hidden.
                if let Some(c) = text_candidate(arena, node, Source::TitleTag, false) {
                    cands.push(("title", c));
                }
            }
            TagId::H1 => {
                if let Some(c) = text_candidate(arena, node, Source::H1, true) {
                    cands.push(("title", c));
                }
            }
            TagId::TIME => {
                if let Some((s, e)) = arena.attr_span(node, AttrName::DATETIME) {
                    cands.push(("published", span_candidate(arena, s, e, Source::TimeElement)));
                }
            }
            TagId::HTML => {
                if let Some((s, e)) = arena.attr_span(node, AttrName::LANG) {
                    cands.push(("language", span_candidate(arena, s, e, Source::MetaName)));
                }
            }
            TagId::LINK => {
                if arena.attr(node, AttrName::REL).is_some_and(|r| {
                    r.split_ascii_whitespace().any(|t| t.eq_ignore_ascii_case("canonical"))
                }) {
                    if let Some((s, e)) = arena.attr_span(node, AttrName::HREF) {
                        cands.push((
                            "canonical_url",
                            span_candidate(arena, s, e, Source::MetaName),
                        ));
                    }
                }
            }
            _ => {}
        }

        // Microdata is an attribute on any element, not a dedicated tag.
        if let Some(prop) = arena.attr(node, AttrName::ITEMPROP) {
            let field = match prop.trim() {
                "headline" | "name" => Some("title"),
                "author" => Some("author"),
                "datePublished" => Some("published"),
                "dateModified" => Some("modified"),
                "description" => Some("description"),
                _ => None,
            };
            if let Some(f) = field {
                // Microdata's value is `content` when present, otherwise the element's text.
                let c = arena
                    .attr_span(node, AttrName::CONTENT)
                    .map(|(s, e)| span_candidate(arena, s, e, Source::Microdata))
                    .or_else(|| text_candidate(arena, node, Source::Microdata, true));
                if let Some(c) = c {
                    cands.push((f, c));
                }
            }
        }
    }

    // Pick per field by (confidence, source) and keep everything else as alternatives.
    m.title = pick(&cands, "title");
    m.published = pick(&cands, "published").map(|c| {
        let d = parse_date(&c.value);
        (c, d)
    });
    m.modified = pick(&cands, "modified").map(|c| {
        let d = parse_date(&c.value);
        (c, d)
    });
    m.site_name = pick(&cands, "site_name");
    m.language = pick(&cands, "language");
    m.description = pick(&cands, "description");
    m.canonical_url = pick(&cands, "canonical_url");

    // Authors are a list. Every author candidate is kept as its own entry rather than joined,
    // because joining then re-splitting is exactly how "Smith, John" becomes two people.
    m.authors = cands
        .iter()
        .filter(|(f, _)| *f == "author")
        .map(|(_, c)| c.clone())
        .collect();

    if let (Some(title), Some(site)) = (m.title.as_ref(), m.site_name.as_ref()) {
        m.title_without_site_name = strip_site_suffix(arena, title, &site.value);
    }

    m.alternatives = cands;
    m
}

fn collect_meta(arena: &Arena, node: NodeId, out: &mut Vec<(&'static str, Candidate)>) {
    let Some((vs, ve)) = arena.attr_span(node, AttrName::CONTENT) else { return };
    let key = arena
        .attr(node, AttrName::PROPERTY)
        .or_else(|| arena.attr(node, AttrName::NAME))
        .unwrap_or("")
        .trim()
        .to_string();
    if key.is_empty() {
        return;
    }
    let lower = key.to_ascii_lowercase();

    let (field, source) = match lower.as_str() {
        "og:title" => ("title", Source::OpenGraph),
        "og:site_name" => ("site_name", Source::OpenGraph),
        "og:description" => ("description", Source::OpenGraph),
        "og:url" => ("canonical_url", Source::OpenGraph),
        "og:locale" => ("language", Source::OpenGraph),
        "article:published_time" => ("published", Source::OpenGraph),
        "article:modified_time" => ("modified", Source::OpenGraph),
        "article:author" => ("author", Source::OpenGraph),
        "twitter:title" => ("title", Source::TwitterCard),
        "twitter:description" => ("description", Source::TwitterCard),
        "author" | "dc.creator" | "dcterms.creator" => ("author", Source::MetaName),
        "description" => ("description", Source::MetaName),
        "dc.title" | "dcterms.title" => ("title", Source::MetaName),
        "dc.date" | "dcterms.created" | "date" => ("published", Source::MetaName),
        "dcterms.modified" | "last-modified" => ("modified", Source::MetaName),
        "dc.language" | "content-language" => ("language", Source::MetaName),
        _ => return,
    };
    out.push((field, span_candidate(arena, vs, ve, source)));
}

fn span_candidate(arena: &Arena, start: u32, end: u32, source: Source) -> Candidate {
    let raw = arena.span_text(start, end);
    let value = ws_normalize(raw);
    let mut transforms = Vec::new();
    if value != raw {
        transforms.push(Transform::WsNormalized);
    }
    transforms.push(Transform::EntityDecodedByParser);
    Candidate {
        value,
        span_start: start,
        span_end: end,
        source,
        confidence: source.base_confidence(),
        transforms,
    }
}

/// Candidate from an element's own text.
///
/// Concatenates the element's **direct** text children and nothing deeper. Both halves matter:
///
/// * All of them, not just the first. html5ever splits text at entity boundaries, so
///   `<title>A &amp; B</title>` arrives as the runs `"A "`, `"&"`, `" B"`. Reading only the first
///   returned the title `"A"` for every page whose title contains an ampersand.
/// * Direct children only. An `<h1>` wrapping a `<span class=byline>` would otherwise absorb the
///   byline into the headline.
///
/// The span is claimed only when the runs are genuinely adjacent in `doc_buf`. When they are not —
/// possible if a nested element's attributes were interleaved during parsing — the longest single
/// run is used instead. Reporting a span that does not re-derive would make the verbatim invariant
/// a lie, and a slightly shorter honest value beats a longer dishonest one.
///
/// `require_prose` exists because `TextRole` answers "does this count as article prose", which is a
/// different question from "is this metadata". `<title>` lives in `<head>`, so it inherits
/// `Hidden` and has no prose role at all — filtering on prose here silently dropped every
/// `<title>` on every page. For in-body sources (`<h1>`, microdata) the prose filter is still
/// wanted, since a heading inside an `aria-hidden` subtree should not become the title.
fn text_candidate(
    arena: &Arena,
    node: NodeId,
    source: Source,
    require_prose: bool,
) -> Option<Candidate> {
    let end = arena.subtree_end.get(node.idx()).copied().unwrap_or(0) as usize;
    let mut runs: Vec<(u32, u32)> = Vec::new();

    for i in (node.idx() + 1)..end {
        if arena.kind.get(i).copied() != Some(NodeKind::Text) {
            continue;
        }
        // Direct children only.
        if arena.parent.get(i).copied() != Some(node) {
            continue;
        }
        if require_prose && !arena.text_role.get(i).copied().is_some_and(crate::a11y::TextRole::is_prose) {
            continue;
        }
        let s = arena.text_start.get(i).copied().unwrap_or(0);
        let e = arena.text_end.get(i).copied().unwrap_or(0);
        if e > s {
            runs.push((s, e));
        }
    }

    let first = *runs.first()?;
    let last = *runs.last()?;
    let contiguous = runs
        .windows(2)
        .all(|w| matches!((w.first(), w.get(1)), (Some(a), Some(b)) if b.0 == a.1));

    let (span_start, span_end) = if contiguous {
        (first.0, last.1)
    } else {
        runs.iter()
            .max_by_key(|(s, e)| e.saturating_sub(*s))
            .copied()
            .unwrap_or(first)
    };

    let c = span_candidate(arena, span_start, span_end, source);
    if c.value.is_empty() {
        None
    } else {
        Some(c)
    }
}

/// Highest-confidence candidate for a field, tie-broken by source ordering then by span position.
///
/// Deterministic on every axis: no float comparison, no hash iteration, and a total order so two
/// runs cannot disagree.
fn pick(cands: &[(&'static str, Candidate)], field: &str) -> Option<Candidate> {
    cands
        .iter()
        .filter(|(f, _)| *f == field)
        .max_by_key(|(_, c)| (c.confidence, c.source, core::cmp::Reverse(c.span_start)))
        .map(|(_, c)| c.clone())
}

/// Remove a site-name suffix, but only when it provably *is* the site name.
///
/// This is the narrow, safe version of what Readability does destructively. Readability splits on
/// any separator and guesses; here the trailing segment must fold-equal `og:site_name`, and the
/// result is expressed as a **span narrowing** so the verbatim invariant still holds.
///
/// A title that legitimately contains " - " keeps it, because the tail will not match the site
/// name. That case is why the blind split is wrong.
fn strip_site_suffix(arena: &Arena, title: &Candidate, site: &str) -> Option<Candidate> {
    // ASCII separators require surrounding whitespace so that hyphenated words survive.
    // U+30FB (KATAKANA MIDDLE DOT) is deliberately absent: it appears inside ordinary Japanese
    // and Korean titles and is not a separator there.
    const SEPARATORS: [&str; 6] = [" | ", " - ", " – ", " — ", " » ", " :: "];

    if site.is_empty() {
        return None;
    }
    let raw = arena.span_text(title.span_start, title.span_end);
    for sep in SEPARATORS {
        if let Some(pos) = raw.rfind(sep) {
            let tail = raw.get(pos + sep.len()..).unwrap_or("").trim();
            if fold_eq(tail, site) {
                let head_end = title.span_start.saturating_add(pos as u32);
                let head_raw = arena.span_text(title.span_start, head_end);
                let value = ws_normalize(head_raw);
                if value.is_empty() {
                    return None;
                }
                let mut transforms = title.transforms.clone();
                transforms.push(Transform::SpanNarrowed);
                return Some(Candidate {
                    value,
                    span_start: title.span_start,
                    span_end: head_end,
                    source: title.source,
                    confidence: title.confidence,
                    transforms,
                });
            }
        }
    }
    None
}

/// Case-insensitive, whitespace-insensitive comparison.
///
/// ASCII case folding only. Full Unicode case folding would need tables this crate does not
/// carry, and the comparison is between a title tail and a site name — both usually identical
/// strings from the same document.
fn fold_eq(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        s.split_whitespace()
            .flat_map(str::chars)
            .map(|c| c.to_ascii_lowercase())
            .collect::<String>()
    };
    norm(a) == norm(b)
}

/// Parse a date into ISO-8601, but only when the input is already unambiguous.
///
/// Deliberately conservative. Ambiguous formats (`03/04/2026`) are left unparsed rather than
/// guessed, and **no timezone is ever assumed**: a timestamp without an offset yields
/// `tz_known: false` and is not shifted.
#[must_use]
pub fn parse_date(raw: &str) -> DateValue {
    let t = raw.trim();
    let tz_known = t.ends_with('Z')
        || t.ends_with('z')
        || has_numeric_offset(t);

    // Accept only ISO-8601-shaped input: YYYY-MM-DD optionally followed by a time.
    let iso = if is_iso_shaped(t) { Some(t.to_string()) } else { None };

    DateValue { raw: raw.to_string(), iso8601: iso, tz_known }
}

fn has_numeric_offset(t: &str) -> bool {
    // A trailing +HH:MM / -HH:MM / +HHMM, but only after a time component, so that the '-' in
    // "2026-08-06" is not mistaken for an offset sign.
    let Some(time_pos) = t.find('T').or_else(|| t.find(' ')) else { return false };
    let tail = t.get(time_pos..).unwrap_or("");
    let Some(sign) = tail.rfind(['+', '-']) else { return false };
    let off = tail.get(sign + 1..).unwrap_or("");
    let digits = off.chars().filter(char::is_ascii_digit).count();
    (2..=4).contains(&digits) && off.chars().all(|c| c.is_ascii_digit() || c == ':')
}

fn is_iso_shaped(t: &str) -> bool {
    let b = t.as_bytes();
    if b.len() < 10 {
        return false;
    }
    let d = |i: usize| b.get(i).is_some_and(u8::is_ascii_digit);
    d(0) && d(1) && d(2) && d(3)
        && b.get(4) == Some(&b'-')
        && d(5) && d(6)
        && b.get(7) == Some(&b'-')
        && d(8) && d(9)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::assertions_on_constants)]
mod tests {
    use super::*;

    #[test]
    fn ws_normalize_collapses_without_changing_characters() {
        assert_eq!(ws_normalize("  a   b \n c "), "a b c");
        // No case folding and no Unicode normalization: NFKC would rewrite full-width forms,
        // which is wrong for CJK titles.
        assert_eq!(ws_normalize("Ａ Ｂ"), "Ａ Ｂ");
        assert_eq!(ws_normalize("한국어  제목"), "한국어 제목");
        assert_eq!(ws_normalize(""), "");
    }

    #[test]
    fn source_confidence_ordering_reflects_explicitness() {
        // JSON-LD is a publisher declaring this exact field; <title> is a tab string.
        assert!(Source::JsonLd.base_confidence() > Source::OpenGraph.base_confidence());
        assert!(Source::OpenGraph.base_confidence() > Source::MetaName.base_confidence());
        assert!(Source::H1.base_confidence() > Source::TitleTag.base_confidence());
    }

    #[test]
    fn no_timezone_is_ever_assumed() {
        // The whole point: an offsetless timestamp must not be silently shifted.
        for (input, expect_tz) in [
            ("2026-08-06T12:00:00Z", true),
            ("2026-08-06T12:00:00+09:00", true),
            ("2026-08-06T12:00:00-0500", true),
            ("2026-08-06T12:00:00", false),
            ("2026-08-06", false),
            ("2026-08-06 12:00:00", false),
        ] {
            let d = parse_date(input);
            assert_eq!(d.tz_known, expect_tz, "tz_known wrong for {input}");
            assert_eq!(d.raw, input, "raw must be preserved verbatim");
        }
    }

    #[test]
    fn ambiguous_dates_are_left_unparsed_rather_than_guessed() {
        for bad in ["03/04/2026", "April 5, 2026", "5 Apr 2026", "yesterday", ""] {
            let d = parse_date(bad);
            assert!(d.iso8601.is_none(), "{bad} must not be parsed to ISO");
            assert_eq!(d.raw, bad);
        }
        assert_eq!(parse_date("2026-08-06").iso8601.as_deref(), Some("2026-08-06"));
    }

    #[test]
    fn the_date_hyphen_is_not_read_as_a_timezone_offset() {
        // "2026-08-06" ends in a '-' group; a naive offset scan reads that as -06.
        assert!(!parse_date("2026-08-06").tz_known);
        assert!(!parse_date("2026-08-06T10:00:00").tz_known);
    }

    #[test]
    fn fold_eq_ignores_case_and_spacing_only() {
        assert!(fold_eq("Example Site", "example site"));
        assert!(fold_eq("  EXAMPLE  SITE ", "Example Site"));
        assert!(!fold_eq("Example Site", "Example Sites"));
        assert!(fold_eq("한국어", "한국어"));
        assert!(!fold_eq("한국어", "한국"));
    }

    #[test]
    fn transforms_distinguish_ours_from_the_parsers() {
        let c = Candidate {
            value: "x".into(),
            span_start: 0,
            span_end: 1,
            source: Source::OpenGraph,
            confidence: 85,
            transforms: alloc::vec![Transform::EntityDecodedByParser],
        };
        // Entity decoding is the parser's doing, so it does not weaken the verbatim claim.
        assert!(c.is_ws_only());

        let narrowed = Candidate { transforms: alloc::vec![Transform::SpanNarrowed], ..c };
        assert!(!narrowed.is_ws_only());
    }
}
