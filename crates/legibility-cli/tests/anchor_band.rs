//! Corpus measurement for region selection, against mozilla's `expected.html`.
//!
//! # What this is and is not
//!
//! Not the M1 parity gate — that needs the R.js oracle, a committed threshold and a triage
//! taxonomy, and `legibility-legacy` is still a stub. This is the smaller thing that was missing
//! and that let two changes land blind: a number saying whether a change to selection made the
//! corpus better or worse.
//!
//! It calls the **real** [`legibility_core::select_article`]. An earlier version reimplemented the
//! decision so it could sweep a constant, and promptly disagreed with production — 1 regression
//! against the real binary's 8 — which is how a constant gets tuned against a fiction. Duplicating
//! the decision is the same mistake as duplicating the serializer, so the sweep now happens by
//! editing the constant and rebuilding, and what lives here is the measurement and the guard.
//!
//! # What was measured (130 pages, mozilla corpus)
//!
//! | change | mean F1 |
//! |---|---|
//! | before | 0.742 |
//! | `Inert` split + viability filter | 0.745 |
//! | + semantic anchor rung | **0.806** |
//!
//! Sweeping an upper edge on the anchor trust band: 1.0 → 0.784, 2.0 → 0.787, 2.5 → 0.799,
//! 3.0 → 0.806, unbounded → 0.806. It buys nothing, so there is no upper edge.

// BTreeMap rather than HashMap: clippy.toml bans HashMap workspace-wide so that no output can
// depend on hash iteration order (guarantee S3). A test is not an output path, but weakening the
// rule per file is how the rule stops meaning anything.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use legibility_core::{Arena, Limits, NodeId};
use legibility_dom::BuildArena;

/// Per-page floor, committed alongside the code that produces it.
///
/// A ratchet, not a target: selection may improve freely, but a page that falls below what it
/// scored here fails the build. An absolute threshold was tried first and was useless — plenty of
/// pages have always scored low for reasons this work never touched, and a test that cannot tell
/// "was already bad" from "just broke" reports noise.
const BASELINE: &str = include_str!("selection-baseline.tsv");

/// How far a single page may fall before it counts as a regression.
///
/// Not zero: serializer and whitespace changes move F1 in the third decimal without changing which
/// region was picked, and a gate that fires on those gets switched off.
const PER_PAGE_TOLERANCE: f32 = 0.02;

/// Pages this work knowingly made worse. They are already at their new, lower value in the
/// baseline; this list records *why*, so the numbers are not mistaken for something anyone chose.
///
/// All four have the same cause: `<script>` bytes used to land in `hidden_len`, which pushed the
/// purity of big outer wrappers below the floor and accidentally forced selection inward. Removing
/// that was a correctness fix — script source is not text — but it took away a brake the scorer
/// was leaning on, and the region that now wins carries the headline and byline into the body.
///
/// Two planned pieces close this: region cleaning (M6) strips page furniture from the chosen
/// region, and plan D4's `lead { byline_span, heading_span }` moves the headline and byline out of
/// `article.html` into their own fields — precisely the systematic offset being measured here.
#[allow(dead_code)]
const KNOWN_REGRESSIONS: [&str; 4] = ["aktualne", "webmd-1", "webmd-2", "youth"];

fn corpus() -> Vec<PathBuf> {
    let root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/readability/test/test-pages");
    let Ok(rd) = std::fs::read_dir(root) else { return Vec::new() };
    let mut v: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    v.sort();
    v
}

/// Strip tags and collapse whitespace. Crude on purpose: both sides get the same treatment.
///
/// Byte-level throughout. An earlier version lowercased into a `String` and sliced it by byte
/// index, which panics the moment a page contains `…` — and most of them do.
fn text_of(markup: &str) -> String {
    let b = markup.as_bytes();
    let lower: Vec<u8> = b.iter().map(u8::to_ascii_lowercase).collect();
    let starts = |at: usize, pat: &[u8]| lower.get(at..at + pat.len()) == Some(pat);
    let find_from = |at: usize, pat: &[u8]| {
        (at..lower.len().saturating_sub(pat.len().saturating_sub(1)))
            .find(|&j| starts(j, pat))
            .unwrap_or(lower.len())
    };

    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut depth = 0usize;
    let mut i = 0usize;
    while i < b.len() {
        if depth == 0 && starts(i, b"<script") {
            i = find_from(i, b"</script");
            continue;
        }
        if depth == 0 && starts(i, b"<style") {
            i = find_from(i, b"</style");
            continue;
        }
        match b[i] {
            b'<' => depth += 1,
            b'>' if depth > 0 => {
                depth -= 1;
                out.push(b' ');
            }
            c if depth == 0 => out.push(c),
            _ => {}
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Token-multiset F1 (plan D8, ASCII half).
fn f1(a: &str, b: &str) -> f32 {
    let count = |s: &str| {
        let mut m: BTreeMap<String, i32> = BTreeMap::new();
        for t in s.split_whitespace() {
            *m.entry(t.to_string()).or_default() += 1;
        }
        m
    };
    let (ca, cb) = (count(a), count(b));
    let overlap: i32 =
        ca.iter().map(|(k, &v)| v.min(cb.get(k).copied().unwrap_or(0))).sum();
    if overlap == 0 {
        return 0.0;
    }
    let (na, nb): (i32, i32) = (ca.values().sum(), cb.values().sum());
    let (p, r) = (overlap as f32 / na as f32, overlap as f32 / nb as f32);
    2.0 * p * r / (p + r)
}

#[allow(dead_code)]
fn region_text(arena: &Arena, node: NodeId) -> String {
    let end = arena.subtree_end.get(node.idx()).copied().unwrap_or(0) as usize;
    let mut out = String::new();
    for i in node.idx()..end {
        if arena.kind.get(i).copied() != Some(legibility_core::NodeKind::Text) {
            continue;
        }
        if arena.text_role.get(i).copied().is_none_or(|r| !r.is_prose()) {
            continue;
        }
        for w in arena.own_text(NodeId(i as u32)).split_whitespace() {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(w);
        }
    }
    out
}

/// `(name, f1)` for every corpus page, through the **whole pipeline**.
///
/// `extract_all`, not `select_article`: comment masking, the anchor rung and the article exclusions
/// all sit between the two, and this measured none of them until a masking bug that changed 130
/// pages' output slipped past it untouched. A gate that watches half the pipeline reports on the
/// half that is not moving.
fn measure() -> Vec<(String, f32)> {
    corpus()
        .into_iter()
        .filter_map(|d| {
            let src = std::fs::read(d.join("source.html")).ok()?;
            let exp = std::fs::read(d.join("expected.html")).ok()?;
            let html = String::from_utf8_lossy(&src).into_owned();
            let (arena, _) = BuildArena::parse_to_arena(&html, Limits::DEFAULT);
            let out = legibility_core::extract_all(&arena, Limits::DEFAULT);
            let ours = out.selection.article.map_or(String::new(), |n| {
                legibility_dom::json::prose_text_excluding(&arena, n, &out.article_exclusions)
            });
            let want = text_of(&String::from_utf8_lossy(&exp));
            Some((d.file_name()?.to_string_lossy().into_owned(), f1(&ours, &want)))
        })
        .collect()
}

#[test]
fn corpus_mean_f1_does_not_fall_below_what_was_measured() {
    let rows = measure();
    if rows.is_empty() {
        eprintln!("corpus submodule absent; skipping");
        return;
    }
    // Floor, not a target. Set below the 0.806 measured when the anchor rung landed, with room for
    // the noise a serializer change can introduce, so it catches a real fall rather than a wobble.
    const FLOOR: f32 = 0.78;
    let mean = rows.iter().map(|(_, s)| *s).sum::<f32>() / rows.len() as f32;
    assert!(
        mean >= FLOOR,
        "corpus mean F1 fell to {mean:.4}, below the committed floor {FLOOR}; \
         run `lgb explain` on the worst pages before adjusting this number"
    );
}

#[test]
fn no_page_falls_below_its_committed_baseline() {
    let rows = measure();
    if rows.is_empty() {
        eprintln!("corpus submodule absent; skipping");
        return;
    }
    let base: BTreeMap<&str, f32> = BASELINE
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .filter_map(|(n, v)| Some((n, v.trim().parse().ok()?)))
        .collect();

    let mut fell: Vec<String> = Vec::new();
    let mut missing: Vec<&str> = Vec::new();
    for (name, got) in &rows {
        match base.get(name.as_str()) {
            Some(&want) if *got + PER_PAGE_TOLERANCE < want => {
                fell.push(format!("{name}: {want:.3} -> {got:.3}"));
            }
            None => missing.push(name.as_str()),
            _ => {}
        }
    }
    assert!(
        missing.is_empty(),
        "corpus pages with no baseline entry: {missing:?}\n\
         run `cargo test -p legibility-cli --test anchor_band -- --ignored bless` to record them"
    );
    assert!(
        fell.is_empty(),
        "region selection regressed on {} page(s):\n  {}\n\
         run `lgb explain <source.html>` on each before touching the baseline",
        fell.len(),
        fell.join("\n  ")
    );
}

#[test]
#[ignore = "writes the baseline; run deliberately after reviewing what changed"]
fn bless_baseline() {
    let rows = measure();
    if rows.is_empty() {
        eprintln!("corpus submodule absent; nothing to record");
        return;
    }
    let mut out = String::from(
        "# Per-page token-F1 against mozilla expected.html, through legibility_core::extract_all.\n\
         # A ratchet: see no_page_falls_below_its_committed_baseline. Regenerate deliberately.\n",
    );
    let mut rows = rows;
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    for (n, s) in &rows {
        out.push_str(&format!("{n}\t{s:.4}\n"));
    }
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/selection-baseline.tsv");
    std::fs::write(&path, out).expect("writing baseline");
    let mean = rows.iter().map(|(_, s)| *s).sum::<f32>() / rows.len() as f32;
    println!("wrote {} rows to {}, mean F1 {mean:.4}", rows.len(), path.display());
}
