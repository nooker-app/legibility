# `rjs-baseline` — run Readability.js and see what it produced

```sh
node run.js --corpus --out corpus.json            # mozilla's 130 test pages
node run.js --out fixtures.json path/to/*.html    # any pages you have
```

No `npm install`, and no lockfile to keep current. `Readability.js` and `JSDOMParser.js` are read
straight out of the pinned `corpus/readability` submodule, so this runs the exact version the corpus
was generated with, and CI never needs Node.

The pipeline mirrors `corpus/readability/test/generate-testcase.js` line for line — the same
`http://fakehost/test/page.html` base URI, the same `{ classesToPreserve: ["caption"] }`, and
`JSDOMParser` rather than jsdom, which is what mozilla parses with. Deviating would turn a
comparison of engines into a comparison of harnesses.

Output is one JSON object per page: `{ok, error, title, byline, siteName, publishedTime, lang, text,
length}`. `text`, not `content` — the metric scores text, and committing every article's HTML would
add megabytes to a permanent history for bytes nothing reads.

**Read [`../../docs/parity-gate.md`](../../docs/parity-gate.md) before drawing a conclusion from
this.** On mozilla's corpus, `expected.html` *is* this tool's output, so Readability scores 1.000
there by construction and the comparison is circular. The pages where it is not are the
self-authored fixtures:

```sh
cargo test -p legibility-dom --test snapshot -- --ignored dump_fixtures
node run.js --out fixtures.json "$TMPDIR/legibility-fixtures"/*.html
```
