//! `lgb` — the development driver.
//!
//! M0 wires the whole pipeline together — bytes in, canonical JSON out — on top of the
//! placeholder scorer, so the seam is exercisable before any real heuristic exists. The point
//! is not extraction quality; it is that `parse -> arena -> flatten -> accumulate -> score ->
//! serialize` runs end to end and produces byte-identical output on every target.

use std::io::Read;

use legibility_core::{Arena, Limits, LimitsHit, NodeId, NodeKind, TagId};
use legibility_dom::BuildArena;

fn main() {
    let mut args = std::env::args().skip(1);
    let sub = args.next().unwrap_or_default();
    let rest: Vec<String> = args.collect();

    match sub.as_str() {
        "extract" => match read_input(rest.first().map(String::as_str)) {
            Ok(html) => {
                let (arena, hit) = BuildArena::parse_to_arena(&html, Limits::DEFAULT);
                println!("{}", to_json(&arena, hit));
            }
            Err(e) => {
                eprintln!("lgb: {e}");
                std::process::exit(1);
            }
        },
        "text" => match read_input(rest.first().map(String::as_str)) {
            Ok(html) => {
                let (arena, _) = BuildArena::parse_to_arena(&html, Limits::DEFAULT);
                match best_region(&arena) {
                    Some(n) => print!("{}", prose_text(&arena, n)),
                    None => eprintln!("lgb: no region accepted"),
                }
            }
            Err(e) => {
                eprintln!("lgb: {e}");
                std::process::exit(1);
            }
        },
        "" => {
            eprintln!("usage: lgb <extract|text> [file|-]");
            std::process::exit(2);
        }
        other => {
            eprintln!("lgb: unknown subcommand `{other}`");
            std::process::exit(2);
        }
    }
}

fn read_input(path: Option<&str>) -> Result<String, String> {
    match path {
        None | Some("-") => {
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .map_err(|e| format!("reading stdin: {e}"))?;
            Ok(buf)
        }
        Some(p) => std::fs::read(p)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .map_err(|e| format!("reading {p}: {e}")),
    }
}

/// Highest-scoring element region under the placeholder scorer.
///
/// Ranking uses the integer key from `legibility_core::num::rank_key`, so ties break by document
/// order rather than by whatever the sort happened to do — see that function for why an
/// intermittently-different winner is worse than a consistently mediocre one.
fn best_region(arena: &Arena) -> Option<NodeId> {
    let mut best: Option<((core::cmp::Reverse<u16>, u32), NodeId)> = None;
    for i in 0..arena.len() {
        if arena.kind.get(i).copied() != Some(NodeKind::Element) {
            continue;
        }
        let tag = arena.tag.get(i).copied().unwrap_or(TagId::UNKNOWN);
        // Only containers are candidates. <html> and <body> are excluded: picking <body> is the
        // silent fallback that defect 1 exists to remove.
        if !matches!(
            tag,
            TagId::DIV | TagId::ARTICLE | TagId::MAIN | TagId::SECTION | TagId::TD | TagId::LI
        ) {
            continue;
        }
        let id = NodeId(i as u32);
        let score = arena.placeholder_evidence(id);
        if score <= 0.0 {
            continue;
        }
        let key = legibility_core::num::rank_key(score / 64.0, i as u32);
        if best.as_ref().is_none_or(|(bk, _)| key < *bk) {
            best = Some((key, id));
        }
    }
    best.map(|(_, id)| id)
}

/// Concatenate the prose text of a subtree, collapsing whitespace runs.
///
/// Reads `text_role` per node, so control labels, hidden subtrees and screen-reader-only text are
/// absent by construction rather than filtered afterwards.
fn prose_text(arena: &Arena, region: NodeId) -> String {
    let end = arena.subtree_end.get(region.idx()).copied().unwrap_or(0) as usize;
    let mut out = String::new();
    for i in region.idx()..end {
        if arena.kind.get(i).copied() != Some(NodeKind::Text) {
            continue;
        }
        if arena.text_role.get(i).copied().is_none_or(|r| !r.is_prose()) {
            continue;
        }
        for word in arena.own_text(NodeId(i as u32)).split_whitespace() {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(word);
        }
    }
    out.push('\n');
    out
}

/// Hand-written canonical JSON.
///
/// No serde. `schema_version` and key order are part of the determinism contract — S3 compares
/// `blake3` of this output across native, wasip1 and headless Chrome — and a derive macro would
/// put that contract at the mercy of a dependency's field ordering.
fn to_json(arena: &Arena, hit: LimitsHit) -> String {
    let region = best_region(arena);
    let mut s = String::from("{\"schema_version\":1,\"article\":");
    match region {
        Some(n) => {
            let text = prose_text(arena, n).trim_end().to_string();
            s.push_str("{\"text\":");
            push_json_string(&mut s, &text);
            s.push_str(",\"prose_len\":");
            s.push_str(&arena.prose_len.get(n.idx()).copied().unwrap_or(0).to_string());
            s.push_str(",\"tag\":");
            push_json_string(
                &mut s,
                arena.tag.get(n.idx()).copied().and_then(TagId::known_name).unwrap_or("?"),
            );
            // `calibrated` is false permanently in v1, not pending. Calibration needs a labelled
            // corpus; claiming it without one would be worse than admitting its absence.
            s.push_str(",\"calibrated\":false}");
        }
        None => s.push_str("null"),
    }
    if region.is_none() {
        s.push_str(",\"no_article\":\"NoTextContent\"");
    }
    // Present-and-empty from day one. A key that appears in a later version is a schema break
    // for every consumer that already shipped against its absence.
    s.push_str(",\"comments\":{\"count\":0,\"items\":[]}");
    s.push_str(",\"diagnostics\":{\"limits_hit\":[");
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
            push_json_string(&mut s, name);
            first = false;
        }
    }
    s.push_str("],\"node_count\":");
    s.push_str(&arena.len().to_string());
    s.push_str(",\"page_prose_len\":");
    s.push_str(&arena.prose_len.first().copied().unwrap_or(0).to_string());
    s.push_str(",\"page_control_len\":");
    s.push_str(&arena.control_len.first().copied().unwrap_or(0).to_string());
    s.push_str(",\"page_hidden_len\":");
    s.push_str(&arena.hidden_len.first().copied().unwrap_or(0).to_string());
    s.push_str(",\"page_alt_len\":");
    s.push_str(&arena.alt_len.first().copied().unwrap_or(0).to_string());
    s.push_str("}}");
    s
}

fn push_json_string(out: &mut String, s: &str) {
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
