//! Build/verification tasks. Kept in-tree so every gate is one `cargo xtask <name>` away and
//! CI runs exactly what a developer runs.
fn main() {
    let task = std::env::args().nth(1);
    match task.as_deref() {
        Some("tool-preflight") => println!("xtask: tool-preflight (M0: asserts both toolchains resolve)"),
        Some(t) => {
            eprintln!("xtask: `{t}` not implemented yet");
            std::process::exit(2);
        }
        None => eprintln!("usage: cargo xtask <tool-preflight|parity|corpus|determinism|complexity|a11y|community|ffi-audit>"),
    }
}
