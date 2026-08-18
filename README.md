# legibility

A content-extraction engine: given an HTML document, it returns the article, the comment
thread, and metadata — as separate, labelled regions with a decision trace.

**This is not a Readability port.** It contains one (`legibility-legacy`), but that crate exists
as a conformance *oracle*: it lets a quality regression be attributed to our arena, to a
parser difference, or to scoring, instead of leaving all three indistinguishable. The engine
itself is built around four things Readability.js structurally cannot do:

1. **Length-independent extraction.** Readability's score contains an absolute text-length term
   and an absolute 500-character accept threshold, and re-runs itself on a document clone up to
   four times when that threshold is missed. Short posts, link posts and Q&A pages fail as a
   result. Here every feature is scale-invariant and every decision is page-relative.
2. **Metadata that is never synthesized.** Title, author and date are candidates carrying their
   source, a confidence, and the transformations applied. The only permitted transforms are
   whitespace normalization and entity decoding, enforced by an executable invariant.
3. **Comments as a first-class region.** Comments are masked out of the article candidate pool
   *before* it is scored, so a long first comment cannot win. Six discussion shapes are
   distinguished, including the one that matters most in practice: a link submission with no
   body (Hacker News) versus a link submission with a summary body (GeekNews).
4. **Speed as a measured property.** One streaming parse, a flat struct-of-arrays arena, and a
   single reverse pass for all subtree sums — which removes the quadratic inner-text
   recomputation. No document cloning, no retry ladder.

## Use it

Three ways, all working today. Start with [`docs/output-schema.md`](docs/output-schema.md) — it is
the contract every one of them returns.

### In a browser, or a browser extension

A WebAssembly module with five exports and forty lines of glue:
[`docs/embedding-web.md`](docs/embedding-web.md).

```sh
cargo build --release -p legibility-wasm --target wasm32-unknown-unknown
```

### In an iOS or macOS app

Inside a `WKWebView`, which is full WebKit and runs WebAssembly normally — so this needs no C ABI, no
`xcframework` and no Swift package. One ~900 KB file in the app bundle and three calls on
`window.legibility`: [`docs/embedding-ios.md`](docs/embedding-ios.md).

```sh
python3 scripts/build-offline-demo.py --template reader js/reader/reader-bundled.html
```

### From the command line

```sh
cargo build --release -p legibility-cli
./target/release/lgb extract page.html          # canonical JSON
./target/release/lgb text page.html             # the article's text
./target/release/lgb explain page.html          # why that region, or why none
./target/release/lgb explain page.html --region # what is inside the chosen region
```

`explain` is the one to reach for when the answer is wrong. It prints the candidate table — the four
factors, which candidates cleared the viability floor, and which won — so "no article" and "the wrong
article" are answered by the same output.

## Try it

<https://nooker-app.github.io/legibility/> — the engine runs in your browser; nothing is uploaded.

Locally, either as a single file with no server at all:

```sh
python3 scripts/build-offline-demo.py     # -> js/testbed/legibility-offline.html
open js/testbed/legibility-offline.html
```

or with a helper that can fetch URLs for you, which a page cannot do for itself:

```sh
cargo run --release -p legibility-cli -- serve   # http://127.0.0.1:8899
```

Both go through the same serializer, so their output is byte-identical to `lgb extract` — the
determinism gate compares them.

## Status

**Extraction works and is measured.** On mozilla/readability's 130-page corpus, token-F1 of the
selected region against their `expected.html`:

| | |
|---|---|
| mean | **0.914** |
| median | 0.989 |
| ≥ 0.95 | 92 pages (71%) |
| < 0.80 | 13 pages (10%) |
| returns no article | 3 pages (2.3%) |

What is proven, and by which gate on every push:

- **No panic, no hang, no unbounded allocation** on any input — five fuzz targets, and every limit
  degrades to a smaller valid result rather than an error.
- **Byte-identical output across targets** — native vs `wasm32-wasip1`, compared over all 130 corpus
  pages plus fixtures on every CI run. This was silently false for 123 of them until the sink stopped
  letting tokenizer chunking become arena shape.
- **Nothing dangerous in the output** — fifteen payload families asserted against real output, and
  both sanitizer profiles fuzzed independently for reparse-fixpoint and mXSS.
- **A per-page quality ratchet.** No page may fall below what it last scored.

What is **not** proven: *"better than Readability.js."* The 0.9144 above is measured against
`expected.html`, and `expected.html` is Readability's own output — reproduced byte-for-byte on
129 of the 130 pages by `tools/rjs-baseline`. So that number is **91.4% agreement with
Readability**, R.js scores 1.000 on its own corpus by construction, and the 13 pages under 0.80 are
disagreements this corpus cannot adjudicate. Where ground truth is ours rather than either
engine's — the thirteen shape fixtures — the count is three differences in our favour and one
against, since fixed. [`docs/parity-gate.md`](docs/parity-gate.md) has the measurement and what
would make the stronger claim provable. Also absent: site adapters, the community and a11y corpora,
and a published package for either npm or crates.io.

Known rough edges are named rather than hidden: `docs/limits.md` for what v1 deliberately does not
do, and `crates/legibility-cli/tests/anchor_band.rs` for the pages this work knowingly made worse and
why.

## Layout

| crate | role |
|---|---|
| `legibility` | umbrella; the only crate consumers depend on |
| `legibility-core` | engine. `no_std + alloc`, no parser, no clock, no I/O |
| `legibility-dom` | the sole html5ever consumer; owns the SoA `TreeSink` |
| `legibility-sanitize` | output sanitizer, two profiles (article vs user content) |
| `legibility-adapters` | site adapters; may only adjust confidence, never override — **stub** |
| `legibility-metrics` | quality metrics, pure — **stub** |
| `legibility-legacy` | Readability.js `parse()` port, used as an oracle — **stub** |
| `legibility-ffi` | C ABI for a native static library — **stub**; the iOS path is the web view |
| `legibility-wasm` | the browser module: five `extern "C"` exports, no wasm-bindgen |
| `legibility-cli` | the `lgb` binary |

## License

Apache-2.0. See `NOTICE` for third-party attributions — in particular mozilla/readability,
of which `legibility-legacy` is a derivative work.
