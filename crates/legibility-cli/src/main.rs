//! `lgb` — the development driver.
//!
//! M0 wires the whole pipeline together — bytes in, canonical JSON out — on top of the
//! placeholder scorer, so the seam is exercisable before any real heuristic exists. The point
//! is not extraction quality; it is that `parse -> arena -> flatten -> accumulate -> score ->
//! serialize` runs end to end and produces byte-identical output on every target.

mod explain;
mod serve;

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
                match legibility_core::select_article(&arena).article {
                    Some(n) => print!("{}", prose_text(&arena, n)),
                    None => eprintln!("lgb: no region accepted"),
                }
            }
            Err(e) => {
                eprintln!("lgb: {e}");
                std::process::exit(1);
            }
        },
        "explain" => match read_input(rest.first().map(String::as_str)) {
            Ok(html) => {
                let (arena, _) = BuildArena::parse_to_arena(&html, Limits::DEFAULT);
                let top = rest
                    .iter()
                    .position(|a| a == "--top")
                    .and_then(|i| rest.get(i + 1))
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(12usize);
                let node = rest
                    .iter()
                    .position(|a| a == "--node")
                    .and_then(|i| rest.get(i + 1))
                    .and_then(|n| n.parse::<u32>().ok());
                if rest.iter().any(|a| a == "--region") {
                    // The candidate table says why *that* region won the page. This says what is
                    // inside it, which is the separate question the shape rule answers.
                    print!("{}", explain::region_map(&arena, Limits::DEFAULT, 1));
                } else if let Some(n) = node {
                    // --node prints what a specific candidate actually holds, which is the next
                    // question after "why did that win".
                    println!("{}", explain::node_text(&arena, NodeId(n)));
                } else {
                    print!("{}", explain::explain(&arena, Limits::DEFAULT, top));
                }
            }
            Err(e) => {
                eprintln!("lgb: {e}");
                std::process::exit(1);
            }
        },
        "serve" => {
            let port = rest
                .iter()
                .position(|a| a == "--port")
                .and_then(|i| rest.get(i + 1))
                .and_then(|p| p.parse().ok())
                .unwrap_or(8899u16);
            if let Err(e) = serve::serve(port) {
                eprintln!("lgb serve: {e}");
                std::process::exit(1);
            }
        }
        "" => {
            eprintln!(
                "usage: lgb <extract|text|explain|serve> [file|-] \
                 [--port N] [--top N] [--node ID] [--region]"
            );
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
#[allow(dead_code)]
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

/// Serialize through the shared serializer so `lgb extract`, `lgb serve` and the WASM module all
/// emit identical bytes -- the cross-target determinism gate compares them.
fn to_json(arena: &Arena, hit: LimitsHit) -> String {
    let out = legibility_core::extract_all(arena, Limits::DEFAULT);
    legibility_dom::json::extraction_json_limited(arena, &out, hit, None, Limits::DEFAULT)
}
