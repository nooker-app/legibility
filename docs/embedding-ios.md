# Reader mode in a WKWebView (iOS and macOS)

Plan §1.11 path **X**: the engine runs as WebAssembly *inside* the web view. No `xcframework`, no C
ABI, no Swift package — `js/reader/reader-bundled.html` is one self-contained file and the app hands
it a document.

## Why this path rather than a native library

"iOS has no JIT" is true outside WebKit and false inside it. A `WKWebView` is full WebKit, so
WebAssembly compiles and runs there normally. The native path (a staticlib behind a C ABI, plan
§1.11.1 paths N and W) is faster and is what a background indexer would want; for *rendering one
article a user asked for*, the web view is already on screen and the module is already the thing the
browser demo runs. Choosing it costs nothing and removes the entire packaging chain.

## Build

```sh
python3 scripts/build-offline-demo.py --template reader js/reader/reader-bundled.html
```

Add the result to the app bundle as a resource. It is ~900 KB, of which ~660 KB is the engine as
base64. Nothing else ships.

## Load it

```swift
let config = WKWebViewConfiguration()
// The page's own scripts are inlined and required; a fetched article's are not, and this is what
// stops them running. The article is also sanitized before it reaches the page — this is the
// second line, not the first.
config.defaultWebpagePreferences.allowsContentJavaScript = true

let webView = WKWebView(frame: .zero, configuration: config)
webView.navigationDelegate = self

let url = Bundle.main.url(forResource: "reader-bundled", withExtension: "html")!
webView.loadFileURL(url, allowingReadAccessTo: url.deletingLastPathComponent())
```

`loadFileURL` rather than `loadHTMLString`: the page must be a real file for `allowingReadAccessTo`
to bound what it can reach, and the bound is its own directory.

## Render an article

Wait for `didFinish`, then call in. The page exposes exactly three things on `window.legibility`.

```swift
func render(html: String) async throws -> Summary {
    // JSONSerialization, not string interpolation. The document is attacker-controlled and contains
    // quotes, backslashes and `</script>`; building the call by concatenation is how it escapes.
    let arg = String(
        data: try JSONSerialization.data(withJSONObject: [html], options: []),
        encoding: .utf8
    )!
    let js = "window.legibility.render(\(arg)[0])"
    let result = try await webView.evaluateJavaScript(js)
    // { ok, title, chars, comments, kind }
    return Summary(result)
}
```

| call | does |
|---|---|
| `window.legibility.render(html)` | extract **and** render; resolves with `{ok, title, chars, comments, kind}` |
| `window.legibility.extract(html)` | extract only, resolves with the full canonical JSON |
| `window.legibility.build` | which engine this page carries, e.g. `9140cfa · wasm 18fcdb90` |

The module is instantiated once and reused, so rendering a second article does not recompile
665 KB.

## Keep navigation inside the app

The article's links are real links. Left alone, tapping one replaces the reader with a live web page
inside your web view — with the engine still loaded in it.

```swift
func webView(_ webView: WKWebView,
             decidePolicyFor navigationAction: WKNavigationAction,
             decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
    // The initial file load, and nothing else.
    if navigationAction.navigationType == .other, navigationAction.request.url?.isFileURL == true {
        return decisionHandler(.allow)
    }
    if let url = navigationAction.request.url {
        UIApplication.shared.open(url)   // or NSWorkspace.shared.open(url) on macOS
    }
    decisionHandler(.cancel)
}
```

## What the page can and cannot do

The file carries its own Content-Security-Policy:

```
default-src 'none'; script-src 'unsafe-inline' 'wasm-unsafe-eval';
style-src 'unsafe-inline'; img-src data:; base-uri 'none'; form-action 'none'; frame-ancestors 'none'
```

- **No network.** `default-src 'none'` and no `connect-src`, so the page cannot fetch, XHR or open a
  socket. It has no code that would, either; this makes it structural.
- **No remote images.** `img-src data:` only. An article's `<img src="https://…">` will not load and
  will not phone home — deliberate, and the thing to change first if you want images: add the
  origins you accept, never `*`.
- **`wasm-unsafe-eval`, not `unsafe-eval`.** Compiling a WebAssembly module counts as evaluation, so
  without it `WebAssembly.instantiate` is refused and the page renders nothing but the browser's own
  error — which is how this shipped the first time. The narrow directive permits the module and
  still forbids `eval()` on JavaScript. Chrome 97+, **Safari 16.4+ (iOS 16.4+)**.
- **No frames, no forms, no `<base>`.**

## What the engine has already done

`article.html` is sanitized with `Profile::Article` and `comments[].html` with
`Profile::UserContent` (plan §1.8) — the latter is strictly tighter, because a comment is
attacker-controlled in a way an article body usually is not: images off, media dropped,
`rel="nofollow noopener noreferrer"` forced on every link. Fifteen payload families are asserted
against the real output in `crates/legibility-dom/tests/selection.rs`, and both profiles are fuzzed
independently.

The CSP above is the second line of defence, not the first.

## Memory

`Limits::IOS_APP_EXTENSION` exists for a Share Extension, which is killed for memory far sooner than
its host app. The web view path does not read it — the module compiled for the browser uses
`Limits::BROWSER` (8 MiB input, 2M nodes, 4 MiB output). If you are rendering inside an extension
rather than the app, build a module that selects the tighter profile.

## What this path does not give you

Extraction happens in the web view, so the app does not get the article as data unless it asks:
`window.legibility.extract(html)` returns the canonical JSON, and the app can then store or index
it. If you want extraction *without* a web view on screen — a background indexer, a Spotlight
importer — that is the native path, and it needs the C ABI in `legibility-ffi`, which is a stub
today.
