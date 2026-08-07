# Using it in a browser

The engine is a WebAssembly module with five exports and no dependencies. There is no npm package
yet; you take the `.wasm` and about forty lines of glue.

## Build the module

```sh
cargo build --release -p legibility-wasm --target wasm32-unknown-unknown
wasm-opt -O3 --enable-bulk-memory \
  target/wasm32-unknown-unknown/release/legibility_wasm.wasm \
  -o legibility.wasm            # ~790 KB -> ~670 KB
```

`wasm-opt` is optional (`brew install binaryen`); without it the module is 120 KB larger and behaves
identically.

## The ABI

Not `wasm-bindgen`. Five `extern "C"` functions over one linear-memory buffer, so the module has no
generated JavaScript that could be version-coupled to the CLI that produced it.

| export | signature | does |
|---|---|---|
| `lgb_alloc` | `(len: usize) -> *mut u8` | reserve `len` bytes to write the document into; `0` on failure |
| `lgb_extract` | `(ptr: *mut u8, len: usize) -> *const u8` | extract; returns a result pointer, `0` on failure |
| `lgb_free_result` | `()` | release the result buffer |
| `lgb_dealloc` | `(ptr: *mut u8, len: usize)` | release an input buffer that was never handed to `lgb_extract` |
| `lgb_schema_version` | `() -> u32` | the `schema_version` this module emits |

`lgb_extract` takes ownership of the input buffer, so the glue below never calls `lgb_dealloc`. It
exists for the caller that allocates and then bails out — a document that turns out to be empty, an
aborted fetch — and would otherwise leak that reservation for the life of the instance.

`lgb_extract` returns a **4-byte little-endian length followed by that many bytes of UTF-8 JSON**.
Copy it out before calling `lgb_free_result`: the view is into the module's own memory, and any later
allocation may move it.

```js
let wasm;

async function init(url = "./legibility.wasm") {
  // `instantiate`, not `instantiateStreaming`: this also works from a byte array, and it sidesteps
  // servers that label `.wasm` as `application/octet-stream`.
  const bytes = new Uint8Array(await (await fetch(url)).arrayBuffer());
  ({ instance: { exports: wasm } } = await WebAssembly.instantiate(bytes, {}));
}

function extract(html) {
  const enc = new TextEncoder().encode(html);
  const ptr = wasm.lgb_alloc(enc.length);
  if (ptr === 0) throw new Error("document too large for the module's limits");
  new Uint8Array(wasm.memory.buffer, ptr, enc.length).set(enc);

  const res = wasm.lgb_extract(ptr, enc.length);
  if (res === 0) throw new Error("extraction returned no result");
  const len = new DataView(wasm.memory.buffer, res, 4).getUint32(0, true);
  const json = new TextDecoder("utf-8").decode(
    new Uint8Array(wasm.memory.buffer, res + 4, len)
  );
  wasm.lgb_free_result();
  return JSON.parse(json);
}
```

Instantiate **once** and reuse. Compiling ~670 KB per article is the difference between instant and
noticeable.

See [`output-schema.md`](output-schema.md) for what you get back. `article.html` is already
sanitized; insert it as markup.

## Content-Security-Policy

Compiling a WebAssembly module counts as evaluation, so a page with a CSP needs
`script-src … 'wasm-unsafe-eval'`. Without it `WebAssembly.instantiate` is refused outright and the
page renders nothing but the browser's own error — which is how the reader view first shipped.

`'wasm-unsafe-eval'` permits the module and still forbids `eval()` on JavaScript. Chrome 97+,
Safari 16.4+. `'unsafe-eval'` also works and gives away more than you need.

## If you want the whole reader view instead

`js/reader/reader.html` is a complete reader-mode page: it embeds the module, renders a styled
article and thread, and exposes `window.legibility.render(html)`. Built with

```sh
python3 scripts/build-offline-demo.py --template reader js/reader/reader-bundled.html
```

One ~900 KB file, no network, its own CSP. That is also the iOS and macOS path — see
[`embedding-ios.md`](embedding-ios.md).

## Fetching the document

**A page cannot read another origin unless that origin allows it,** and news sites do not. This is a
browser rule, not a network limitation: `curl` has no such restriction, which is why the CLI and
`lgb serve` work where a page does not.

Your options, in the order they cost you something:

1. **Already have the HTML.** A feed reader that fetched the page server-side, a browser extension
   with host permissions, an app passing in a document. Nothing to solve.
2. **An extension content script.** Runs in the page's own origin, so `document.documentElement
   .outerHTML` is simply available. This is the intended shape for a browser extension.
3. **A CORS proxy.** Works, and means the proxy operator sees every address you read. The hosted demo
   does this with the proxy named on the page and editable, because a static page has no other
   option — see `fetchWithoutHelper` in `js/testbed/demo.html`. Do not do this silently.
4. **`lgb serve`.** A helper on the user's own machine fetches for the page. Nothing but their device
   is involved.

Many sites answer a data-centre address with a 403 while serving a browser normally — `news.hada.io`
and Reddit both do. No proxy gets past that, and the demo detects the resulting block page rather
than extracting it as an article.

## Limits

The module compiles with `Limits::BROWSER`: 8 MiB input, 2M nodes, 4 MiB output, depth 512. Hitting
one produces a **valid, smaller** result with the cause in `diagnostics.limits_hit` — there is no
error path from a limit (guarantee S2).

A wasm panic traps the instance and cannot be caught, so every export wraps its body in
`catch_unwind` and CI refuses any that does not. After a trap, discard the instance and make a new
one.
