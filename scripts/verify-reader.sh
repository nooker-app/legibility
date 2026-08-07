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

TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT
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

"$CHROME" --headless --disable-gpu --no-sandbox --allow-file-access-from-files \
  --virtual-time-budget=25000 --dump-dom "file://$TMP/drive.html" >"$TMP/dom.html" 2>/dev/null

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
else say FAIL "see the per-check list above"; fail=1
fi

echo
[ "$fail" = 0 ] && echo "reader view: ALL PASS" || echo "reader view: FAILED"
exit "$fail"
