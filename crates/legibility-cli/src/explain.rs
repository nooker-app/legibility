//! `lgb explain` — the decision trace (plan §M2).
//!
//! # Why this exists
//!
//! Without it, a rejection is a single word. Diagnosing "no article" on a real page meant
//! reconstructing a reduced fixture and guessing which of four factors collapsed — three wrong
//! guesses before the actual cause (`<main>` disqualified by an inlined 10 KB script) turned up.
//! Every number needed to see that instantly was already computed and thrown away.
//!
//! So this prints the candidate table: the four factors, whether each candidate clears the
//! viability floor, and which one won. It answers "why that region" and "why nothing" with the
//! same output, because they are the same question asked of the same table.

use legibility_core::score::{self, Candidate};
use legibility_core::{groups, Arena, Limits, NodeId, TagId};

/// Print the candidate table for `arena`, ranked by evidence.
pub fn explain(arena: &Arena, limits: Limits, top: usize) -> String {
    let mut s = String::new();

    // Same masking the pipeline applies, so the table explains the real decision rather than a
    // hypothetical unmasked one. Reproduced rather than shared because `pipeline::run` returns
    // conclusions, not intermediates -- see the note at the end of this function.
    let found = groups::merge_by_signature(arena, &groups::find_groups(arena));
    let thread = found
        .iter()
        .filter(|g| g.is_comment_thread())
        .max_by_key(|g| g.prose_len);
    let masked = if thread.is_some() {
        groups::mask_comment_prose(arena, &found)
    } else {
        vec![0u32; arena.len()]
    };

    let (cands, stats) = score::collect_masked(arena, &masked);
    let outcome = legibility_core::extract_all(arena, limits);

    s.push_str(&format!(
        "page prose {}B  ·  control {}B  ·  hidden {}B  ·  alt {}B  ·  inert {}B (not counted)\n",
        arena.prose_len.first().copied().unwrap_or(0),
        arena.control_len.first().copied().unwrap_or(0),
        arena.hidden_len.first().copied().unwrap_or(0),
        arena.alt_len.first().copied().unwrap_or(0),
        arena.inert_len.first().copied().unwrap_or(0),
    ));
    s.push_str(&format!(
        "candidates {}  ·  median density {:.2}  ·  density IQR {:.2}  ·  groups {}{}\n\n",
        stats.competitors,
        stats.median_density,
        stats.density_iqr,
        found.len(),
        if thread.is_some() { "  ·  comment thread masked" } else { "" },
    ));

    // Rank the same way the scorer does: integer key, document order breaking ties.
    let mut ranked: Vec<(_, usize)> = cands
        .iter()
        .enumerate()
        .map(|(i, c)| (legibility_core::num::rank_key(norm(c.evidence), c.node.0), i))
        .collect();
    ranked.sort_unstable();

    s.push_str(
        "  #  node    tag         evidence  share  density  link_d  purity  viable  why not\n",
    );
    let winner = outcome.selection.article;
    for (rank, &(_, idx)) in ranked.iter().take(top).enumerate() {
        let Some(c) = cands.get(idx) else { continue };
        let tag = arena
            .tag
            .get(c.node.idx())
            .copied()
            .unwrap_or(TagId::UNKNOWN)
            .known_name()
            .unwrap_or("?");
        let mark = if winner == Some(c.node) { "->" } else { "  " };
        s.push_str(&format!(
            "{mark}{:2}  {:<6}  {:<10}  {:>8.3}  {:>5.3}  {:>7.2}  {:>6.3}  {:>6.3}  {:^6}  {}\n",
            rank + 1,
            c.node.0,
            tag,
            c.evidence,
            c.text_share,
            c.density,
            c.link_density,
            c.purity,
            if c.is_viable() { "yes" } else { "NO" },
            why_not(c),
        ));
    }

    s.push('\n');
    match (winner, outcome.selection.no_article) {
        (Some(n), _) => {
            let tag = arena
                .tag
                .get(n.idx())
                .copied()
                .unwrap_or(TagId::UNKNOWN)
                .known_name()
                .unwrap_or("?");
            s.push_str(&format!(
                "accepted: node {} <{tag}>  confidence {}/10000{}\n",
                n.0,
                outcome.selection.confidence,
                if outcome.selection.dispersion_floor_used {
                    "  (dispersion floor: too few competitors to be sure)"
                } else {
                    ""
                },
            ));
        }
        (None, Some(reason)) => {
            s.push_str(&format!("rejected: {reason:?}\n"));
            let viable = cands.iter().filter(|c| c.is_viable()).count();
            s.push_str(&format!(
                "  {} of {} candidates cleared the floor (purity >= {}, link density <= {})\n",
                viable,
                cands.len(),
                Candidate::MIN_PURITY,
                Candidate::MAX_LINK_DENSITY,
            ));
            if outcome.is_listing {
                s.push_str("  a non-comment repeated group holds most of the page's prose\n");
            }
        }
        (None, None) => s.push_str("rejected: no reason recorded -- that is itself a bug\n"),
    }
    if outcome.comment_mask_reverted {
        s.push_str("note: comment masking was undone; it had left no viable article\n");
    }
    s.push_str(&format!(
        "comments: {} item(s){}\n",
        outcome.comments.items.len(),
        if outcome.comments.completeness.truncated { ", truncated" } else { "" },
    ));
    s
}

/// Why a candidate failed the floor, or `""` if it did not.
fn why_not(c: &Candidate) -> &'static str {
    if c.purity < Candidate::MIN_PURITY && c.link_density > Candidate::MAX_LINK_DENSITY {
        "impure and link-dense"
    } else if c.purity < Candidate::MIN_PURITY {
        "mostly non-prose text"
    } else if c.link_density > Candidate::MAX_LINK_DENSITY {
        "mostly links"
    } else {
        ""
    }
}

/// Mirror of the scorer's ranking transform.
fn norm(evidence: f32) -> f32 {
    if !evidence.is_finite() || evidence <= 0.0 {
        return 0.0;
    }
    evidence / (1.0 + evidence)
}

/// Prose text of one node's subtree, for `--node`.
pub fn node_text(arena: &Arena, node: NodeId) -> String {
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
