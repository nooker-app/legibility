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
/// All six have one cause: whitespace-only text nodes stopped counting as prose. Indentation is
/// not content, and counting it gave deeply nested, pretty-printed regions a size bonus
/// proportional to their own nesting. Removing it is a correctness fix — it is what stopped a
/// bare URL inside four wrapper `<div>`s from reading as a submission body — but it also took
/// away a thumb on the scale that these six were winning by.
///
/// The trade is recorded rather than argued: **25 pages improved, 6 fell, mean F1 0.8085 →
/// 0.8285.** Several of the gains are total (msn 0.446 → 1.000, youth 0.621 → 1.000,
/// yahoo-2 0.685 → 1.000), and the four pages this list used to name — aktualne, webmd-1,
/// webmd-2, youth — are all healed by the same change, because inflated wrapper lengths were
/// their cause too.
///
/// `bug-1255978` is the one refusal: it now returns `IndexPage` rather than a low-scoring
/// region. One page in 130 is 0.8%, well under the 5% `refusal_rate` cap, but it is a refusal
/// and not a near miss, so it is named here.
/// `nytimes-3` joined for a different reason: `itemprop="articleBody"` became a semantic anchor
/// (plan §1.10.4, listed there from the start and never implemented). That is the narrowest claim a
/// page can make about where its body is, and honouring it moved **24 pages up and 3 down, mean F1
/// 0.8374 → 0.8432** — folha +0.162, heise +0.124, liberation-1 +0.100, medicalnewstoday +0.090.
///
/// The NYT puts its dek in a `<p>` *outside* the marked body, so choosing the narrower region drops
/// one sentence that `expected.html` keeps: 0.988 → 0.952. Region growth (plan M6) is the real fix —
/// the marked body plus its adjacent dek — and until that exists this is a sentence lost against
/// four sites' worth of credit bars and comment threads removed. Named rather than absorbed.
#[allow(dead_code)]
const KNOWN_REGRESSIONS: [&str; 7] = [
    "bug-1255978",
    "nytimes-3",
    "ehow-1",
    "hukumusume",
    "quanta-1",
    "simplyfound-1",
    "wordpress",
];

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
    // Floor, not a target. Set below the 0.8285 measured once whitespace stopped counting as
    // prose, with room for the noise a serializer change can introduce, so it catches a real fall
    // rather than a wobble. Raised from 0.78 with that measurement: a floor that never moves stops
    // being a ratchet.
    const FLOOR: f32 = 0.80;
    let mean = rows.iter().map(|(_, s)| *s).sum::<f32>() / rows.len() as f32;
    eprintln!("MEAN {mean:.4} over {} pages", rows.len());
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
    eprintln!("MEAN {mean:.4} over {} pages", rows.len());
    println!("wrote {} rows to {}, mean F1 {mean:.4}", rows.len(), path.display());
}
