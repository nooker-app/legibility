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

## Try it

Two demos, because one of them cannot do what the other can.

**Offline, single file, no server.** The WebAssembly module is embedded in the HTML as base64, so it
runs from `file://` — double-click it, mail it, use it on a plane.

```sh
python3 scripts/build-offline-demo.py     # -> js/testbed/legibility-offline.html
open js/testbed/legibility-offline.html
```

Paste HTML or drop a `.html` file. There is **no URL input**: a page opened from disk cannot fetch a
third-party site, because CORS forbids it and no client-side code changes that. Verify the offline
claim rather than taking it — `scripts/verify-offline-demo.sh` loads the file in headless Chrome from
`file://` and asserts the extraction is correct.

**Local server, with URL fetching.** The fetch happens server-side, where CORS does not apply.

```sh
cargo run --release -p legibility-cli -- serve   # http://127.0.0.1:8080
```

Both go through the same serializer (`legibility_dom::json`), so their output is byte-identical to
`lgb extract` — the determinism gate compares them.

## Status

Pre-alpha. M0 (skeleton, disciplines, probes) is in progress; nothing here extracts anything
useful yet. See `docs/adr/` for the decisions already settled, and `docs/limits.md` for what v1
deliberately does not do.

## Layout

| crate | role |
|---|---|
| `legibility` | umbrella; the only crate consumers depend on |
| `legibility-core` | engine. `no_std + alloc`, no parser, no clock, no I/O |
| `legibility-dom` | the sole html5ever consumer; owns the SoA `TreeSink` |
| `legibility-sanitize` | output sanitizer, two profiles (article vs user content) |
| `legibility-adapters` | site adapters; may only adjust confidence, never override |
| `legibility-metrics` | quality metrics, pure |
| `legibility-legacy` | Readability.js `parse()` port, used as an oracle |
| `legibility-ffi` | C ABI for the iOS static library |
| `legibility-wasm` | wasm-bindgen wrapper |
| `legibility-cli` | the `lgb` binary |

## License

Apache-2.0. See `NOTICE` for third-party attributions — in particular mozilla/readability,
of which `legibility-legacy` is a derivative work.
