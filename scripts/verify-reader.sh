#!/usr/bin/env bash
# Verify the reader bundle renders an article, driven the way a host drives it.
#
# The reader page is what an iOS or macOS app loads into a WKWebView, and the only thing that can
# tell whether it works is a browser: its first version was refused by its own Content-Security-
# Policy, because compiling WebAssembly counts as evaluation, and nothing but a real load would have
# said so.
set -uo pipefail
cd "$(dirname "$0")/.."
FILE="js/reader/reader-bundled.html"
fail=0
say() { printf '  %-5s %s\n' "$1" "$2"; }

echo "reader view verification"
[ -f "$FILE" ] || {
  say FAIL "$FILE missing — build it with:"
  printf '        python3 scripts/build-offline-demo.py --template reader %s\n' "$FILE"
  exit 1
}

if python3 scripts/check-external-refs.py "$FILE"; then
  say PASS "nothing loads from another host"
else
  say FAIL "off-host loads present, listed above"; fail=1
fi

if python3 scripts/check-embed.py "$FILE" >/dev/null; then
  say PASS "wasm embedded (one file, no sibling to ship)"
else
  say FAIL "wasm is not embedded"; fail=1
fi

CHROME=$(ls -d "$HOME"/.cache/puppeteer/chrome/*/chrome-mac*/Google\ Chrome\ for\ Testing.app/Contents/MacOS/* 2>/dev/null | head -1)
[ -n "$CHROME" ] || CHROME="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
if [ ! -x "$CHROME" ]; then
  for c in google-chrome-stable google-chrome chromium chromium-browser; do
    command -v "$c" >/dev/null 2>&1 && { CHROME=$(command -v "$c"); break; }
  done
fi
if [ ! -x "$CHROME" ]; then
  say SKIP "no Chrome found; render not checked"
  echo; [ "$fail" = 0 ] && echo "reader view: contents PASS, render SKIPPED" || echo "reader view: FAILED"
  exit "$fail"
fi

# Inside the repository, not /tmp. A snap-confined Chromium — which is what `chromium-browser`
# resolves to on some Linux images — cannot read outside the user's home, so a driver in /tmp loads
# as a blank page and every assertion below fails at once with no hint as to why.
TMP=$(mktemp -d "$PWD/.reader-verify-XXXXXX"); trap 'rm -rf "$TMP"' EXIT
# A self-authored article, so this needs no corpus and commits no third-party HTML (D9).
python3 - "$FILE" "$TMP/drive.html" <<'PY'
import json, sys, pathlib
page = pathlib.Path(sys.argv[1]).read_text(encoding="utf-8")
article = (
    "<html><head><title>Neon lighting — Field Guide</title>"
    "<meta name='author' content='Ann Lee'></head><body><main><article>"
    "<h1>Why neon is fading</h1>"
    "<p>Neon lighting was invented in 1910 and spread through every city centre before the "
    "cheaper alternatives arrived to replace it.</p>"
    "<pre><code>def f(n):\n    return n</code></pre>"
    "<p>The craft survives in a few workshops, mostly doing restoration work.</p>"
    "</article>"
    "<ul class='comments'>"
    "<li class='c'><div class='meta'><a class='author' href='/u/bo'>bo</a>"
    "<time datetime='2026-08-01T10:00:00Z'>Aug 1</time></div>"
    "<p>My grandfather bent tubes for forty years.</p></li>"
    "<li class='c'><div class='meta'><a class='author' href='/u/cy'>cy</a>"
    "<time datetime='2026-08-02T10:00:00Z'>Aug 2</time></div>"
    "<p>There is a workshop in Osaka still doing this by hand.</p></li>"
    "<li class='c'><div class='meta'><a class='author' href='/u/di'>di</a>"
    "<time datetime='2026-08-03T10:00:00Z'>Aug 3</time></div>"
    "<p>LED retrofits look nothing like the real thing up close.</p></li>"
    "</ul></main></body></html>"
)
driver = (
    "\n<script>\n(async () => {\n  const r = await window.legibility.render("
    + json.dumps(article).replace("</", "<\\/")
    + ");\n  document.title = 'RESULT ' + JSON.stringify(r);\n})();\n</script>\n"
)
pathlib.Path(sys.argv[2]).write_text(page + driver, encoding="utf-8")
PY

# Two transports, because they answer different questions and only one of them is reliable to
# snapshot.
#
# `--dump-dom` takes a single snapshot, and `--virtual-time-budget` advances *virtual* time — it
# does not wait on real CPU work, and compiling 667 KB of WebAssembly is real CPU work. On a fast
# machine the render lands first; on a CI runner it does not, and the snapshot catches an empty
# page. That produced eight identical failures with nothing in the console to explain them.
#
# So the *behaviour* is asserted over http, served locally, which is what the gh-pages verifier
# already does reliably in CI. And `file://` — the transport a WKWebView actually uses, via
# `loadFileURL` — is asserted separately and narrowly: that the page initializes there at all, which
# is the only part that can differ by origin. The CSP refusal that broke this page originally would
# still be caught, because it stops initialization.
# Headless Chrome can wedge -- waiting on a socket, a GPU probe, a profile lock -- and
# `--virtual-time-budget` bounds the page's clock, not the process's. Without a wall-clock bound a
# wedged browser hangs the whole run: one CI job sat on this step for thirty-two minutes against a
# 4.5-second local time before anyone looked. `timeout` is GNU coreutils and is not on every macOS,
# so fall back to running unguarded rather than failing to run at all.
if command -v timeout >/dev/null 2>&1; then
  cap() { timeout "$@"; }
elif command -v gtimeout >/dev/null 2>&1; then
  cap() { gtimeout "$@"; }
else
  cap() { shift; "$@"; }
fi

PORT="${2:-8931}"
python3 -m http.server "$PORT" --directory "$TMP" >/dev/null 2>&1 &
server=$!
trap 'kill $server 2>/dev/null; rm -rf "$TMP"' EXIT
for _ in $(seq 1 40); do
  curl -fsS -o /dev/null "http://127.0.0.1:$PORT/drive.html" 2>/dev/null && break
  sleep 0.25
done

cap 120 "$CHROME" --headless --disable-gpu --no-sandbox --enable-logging=stderr --log-level=0 \
  --virtual-time-budget=25000 --dump-dom "http://127.0.0.1:$PORT/drive.html" \
  >"$TMP/dom.html" 2>"$TMP/console.log"

# `file://` only has to get as far as exposing the API; the CSP is what could stop it.
cat > "$TMP/init.html" <<'INIT'
<script>window.addEventListener("load", () => {
  document.title = "INIT " + (typeof window.legibility === "object" ? "ok" : "missing");
});</script>
INIT
cat "$FILE" "$TMP/init.html" > "$TMP/fileprobe.html"
cap 120 "$CHROME" --headless --disable-gpu --no-sandbox --allow-file-access-from-files \
  --virtual-time-budget=15000 --dump-dom "file://$TMP/fileprobe.html" \
  >"$TMP/filedom.html" 2>/dev/null
if grep -q "INIT ok" "$TMP/filedom.html"; then
  say PASS "initializes from file:// (the transport a WKWebView loads)"
else
  say FAIL "does not initialize from file://; a WKWebView would show nothing"
  grep -iE 'refused|denied|csp|security' "$TMP/console.log" 2>/dev/null | head -3 | sed 's/^/        /'
  fail=1
fi

if python3 - "$TMP/dom.html" <<'PY'
import json, re, sys
s = open(sys.argv[1], encoding="utf-8", errors="replace").read()
title = (re.search(r"<title>(.*?)</title>", s, re.S) or ["", ""])[1]
root = (re.search(r'<main id="root">(.*?)</main>', s, re.S) or ["", ""])[1]
res = {}
if title.startswith("RESULT "):
    try:
        res = json.loads(title[len("RESULT "):])
    except Exception:
        pass
checks = [
    ("the host call resolved ok", res.get("ok") is True),
    ("an article was found", res.get("kind") == "article"),
    ("three comments were found", res.get("comments") == 3),
    ("a headline is rendered", "<h1 class=\"title\">" in root),
    ("the body is rendered", "<article>" in root and "invented in 1910" in root),
    ("the code block kept its indentation", "    return n" in root),
    ("comments are rendered", 'ol class="comments"' in root),
    ("no error panel", 'class="err"' not in root),
]
for name, ok in checks:
    print(f"        {'ok  ' if ok else 'FAIL'}  {name}")
sys.exit(1 if [n for n, ok in checks if not ok] else 0)
PY
then say PASS "renders a full reader view in a browser"
else
  say FAIL "see the per-check list above"
  # The browser's own account of it, which is the difference between "the page is wrong" and "the
  # page never loaded".
  grep -iE 'error|refused|denied|csp|security' "$TMP/console.log" 2>/dev/null | head -5 | sed 's/^/        /'
  fail=1
fi

echo
[ "$fail" = 0 ] && echo "reader view: ALL PASS" || echo "reader view: FAILED"
exit "$fail"
