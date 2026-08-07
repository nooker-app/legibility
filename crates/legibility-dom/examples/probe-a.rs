//! PROBE-A — html5ever throughput, measured twice.
//!
//! The plan's M0 gate insists on two numbers rather than one, because the difference between
//! them is the only honest way to budget anything downstream:
//!
//! * **(a) null sink** — html5ever's tokenizer and tree builder with a sink that discards
//!   everything. This is the ceiling: no arena can be faster than the parser feeding it.
//! * **(b) full-featurization sink** — the real [`BuildArena`], doing tag interning, a11y
//!   classification, `doc_buf` copying, then `flatten()` and `accumulate_subtrees()`.
//!
//! Every performance claim in the plan derives from **(b)**, not (a). Quoting (a) would be
//! quoting a number no user can ever observe.
//!
//! Run with:
//! ```text
//! cargo run --release -p legibility-dom --example probe-a
//! ```

// `Instant::now` is banned workspace-wide by clippy.toml so that the *engine* cannot read a
// clock — output must depend only on input bytes (guarantee S3). A benchmark whose entire job is
// to measure elapsed time is the one legitimate exception, and it lives outside the library.
#![allow(clippy::disallowed_methods)]

use std::borrow::Cow;
use std::cell::RefCell;
use std::time::Instant;

use html5ever::interface::{ElementFlags, NodeOrText, QuirksMode, TreeSink};
use html5ever::tendril::{StrTendril, TendrilSink};
use html5ever::{local_name, ns, Attribute, QualName};

use legibility_core::Limits;
use legibility_dom::BuildArena;

/// A sink that allocates ids and nothing else. Establishes the parser-only ceiling.
struct NullSink {
    names: RefCell<Vec<QualName>>,
}

impl NullSink {
    fn new() -> Self {
        Self { names: RefCell::new(vec![QualName::new(None, ns!(), local_name!(""))]) }
    }
}

impl TreeSink for NullSink {
    type Handle = usize;
    type Output = usize;
    type ElemName<'a>
        = std::cell::Ref<'a, QualName>
    where
        Self: 'a;

    fn finish(self) -> usize {
        self.names.borrow().len()
    }
    fn parse_error(&self, _: Cow<'static, str>) {}
    fn get_document(&self) -> usize {
        0
    }
    fn elem_name<'a>(&'a self, target: &'a usize) -> std::cell::Ref<'a, QualName> {
        let i = *target;
        std::cell::Ref::map(self.names.borrow(), move |v| v.get(i).unwrap_or(&v[0]))
    }
    fn create_element(&self, name: QualName, _: Vec<Attribute>, _: ElementFlags) -> usize {
        let mut n = self.names.borrow_mut();
        n.push(name);
        n.len() - 1
    }
    fn create_comment(&self, _: StrTendril) -> usize {
        0
    }
    fn create_pi(&self, _: StrTendril, _: StrTendril) -> usize {
        0
    }
    fn append(&self, _: &usize, _: NodeOrText<usize>) {}
    fn append_based_on_parent_node(&self, _: &usize, _: &usize, _: NodeOrText<usize>) {}
    fn append_doctype_to_document(&self, _: StrTendril, _: StrTendril, _: StrTendril) {}
    fn get_template_contents(&self, _: &usize) -> usize {
        0
    }
    fn same_node(&self, x: &usize, y: &usize) -> bool {
        x == y
    }
    fn set_quirks_mode(&self, _: QuirksMode) {}
    fn append_before_sibling(&self, _: &usize, _: NodeOrText<usize>) {}
    fn add_attrs_if_missing(&self, _: &usize, _: Vec<Attribute>) {}
    fn remove_from_parent(&self, _: &usize) {}
    fn reparent_children(&self, _: &usize, _: &usize) {}
}

fn corpus() -> Vec<(String, String)> {
    let dir = std::path::Path::new("corpus/readability/test/test-pages");
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        let mut names: Vec<_> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
        names.sort();
        for p in names {
            let src = p.join("source.html");
            if let Ok(bytes) = std::fs::read(&src) {
                let name =
                    p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                out.push((name, String::from_utf8_lossy(&bytes).into_owned()));
            }
        }
    }
    out
}

fn main() {
    let docs = corpus();
    if docs.is_empty() {
        eprintln!(
            "PROBE-A: no corpus found. Run from the repository root with the submodule checked out:\n  \
             git submodule update --init corpus/readability"
        );
        std::process::exit(1);
    }

    let total_bytes: usize = docs.iter().map(|(_, h)| h.len()).sum();
    let reps = 3usize;

    // (a) parser ceiling.
    let mut best_a = f64::MAX;
    for _ in 0..reps {
        let t = Instant::now();
        let mut sink_nodes = 0usize;
        for (_, html) in &docs {
            sink_nodes +=
                html5ever::parse_document(NullSink::new(), html5ever::ParseOpts::default())
                    .from_utf8()
                    .one(html.as_bytes());
        }
        std::hint::black_box(sink_nodes);
        best_a = best_a.min(t.elapsed().as_secs_f64());
    }

    // (b) what a caller actually pays for.
    let mut best_b = f64::MAX;
    let mut nodes_total = 0usize;
    for _ in 0..reps {
        let t = Instant::now();
        let mut n = 0usize;
        for (_, html) in &docs {
            let (arena, _) = BuildArena::parse_to_arena(html, Limits::DEFAULT);
            n += arena.len();
        }
        std::hint::black_box(n);
        nodes_total = n;
        best_b = best_b.min(t.elapsed().as_secs_f64());
    }

    let mib = total_bytes as f64 / (1024.0 * 1024.0);
    println!("PROBE-A  (best of {reps}, release)");
    println!(
        "  corpus              {} docs, {:.2} MiB, {} arena nodes",
        docs.len(),
        mib,
        nodes_total
    );
    println!(
        "  (a) null sink       {:>8.1} ms   {:>7.1} MiB/s   <- parser ceiling, not observable",
        best_a * 1e3,
        mib / best_a
    );
    println!(
        "  (b) full featurize  {:>8.1} ms   {:>7.1} MiB/s   <- BUDGET DERIVES FROM THIS",
        best_b * 1e3,
        mib / best_b
    );
    println!(
        "  our overhead        {:>8.1} ms   {:>7.1}x parse",
        (best_b - best_a) * 1e3,
        best_b / best_a
    );
    println!();
    println!("  Kill criterion (plan M0): if (b) is under ~40 MiB/s and profiling blames the");
    println!("  tokenizer state machine, the optimization ladder must be scheduled before M6.");
    let verdict = if mib / best_b >= 40.0 { "PASS" } else { "TRIGGERED" };
    println!("  verdict: {verdict}");
}
