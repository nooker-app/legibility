//! `lgb` — the development driver. M0 ships `extract` against a placeholder engine so the
//! whole pipeline (bytes in, canonical JSON out) is exercisable before any heuristic exists.
fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("extract") => println!("{}", legibility_cli_placeholder_json()),
        Some(other) => {
            eprintln!("lgb: unknown subcommand `{other}`");
            std::process::exit(2);
        }
        None => {
            eprintln!("usage: lgb <extract|corpus|diff|explain|read>");
            std::process::exit(2);
        }
    }
}

/// Hand-written canonical JSON. No serde: `schema_version` and key order are part of the
/// determinism contract (S3 compares blake3 of this output across targets), and a derive macro
/// would put that contract at the mercy of a dependency's field ordering.
fn legibility_cli_placeholder_json() -> String {
    // `comments` is present-and-empty from day one rather than absent. A key that appears later
    // is a schema break for every consumer that already shipped.
    concat!(
        "{\"schema_version\":1,",
        "\"article\":null,",
        "\"no_article\":\"NoTextContent\",",
        "\"comments\":{\"count\":0,\"items\":[]},",
        "\"diagnostics\":{\"limits_hit\":[],\"node_count\":0,\"page_prose_len\":0}}"
    )
    .to_string()
}
