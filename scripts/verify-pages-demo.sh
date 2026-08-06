#!/usr/bin/env bash
# Verify the gh-pages bundle behaves correctly as a *static* project page.
#
# `pages-guard.sh` checks the bundle's contents. This checks what it does, which is a different
# question and the one that matters: the page must extract with no server capable of helping it, and
# it must not offer a control that would call the static host looking for a fetch endpoint.
#
# Served under a `/legibility/` prefix by `python3 -m http.server` -- a server with no `/fetch`
# route, no wasm Content-Type handling worth relying on, and no relationship to `lgb serve`. If the
# page works against that, it works against GitHub Pages.
set -uo pipefail
cd "$(dirname "$0")/.."
DIST="${1:-dist}"
PORT="${2:-8908}"
fail=0
say() { printf '  %-5s %s\n' "$1" "$2"; }

echo "gh-pages behavioural verification ($DIST)"

if [ ! -f "$DIST/index.html" ]; then
  say FAIL "no $DIST/index.html — run: python3 scripts/build-offline-demo.py $DIST/index.html"
  exit 1
fi

# macOS first, then the Linux names GitHub's runners carry, so this is the same check locally and in
# CI. A behavioural check that only ever runs on one developer's laptop is a comment.
CHROME=$(ls -d "$HOME"/.cache/puppeteer/chrome/*/chrome-mac*/Google\ Chrome\ for\ Testing.app/Contents/MacOS/* 2>/dev/null | head -1)
[ -n "$CHROME" ] || CHROME="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
if [ ! -x "$CHROME" ]; then
  for c in google-chrome-stable google-chrome chromium chromium-browser; do
    if command -v "$c" >/dev/null 2>&1; then CHROME=$(command -v "$c"); break; fi
  done
fi
if [ ! -x "$CHROME" ]; then
  say SKIP "no Chrome found; behavioural check not run"
  exit 0
fi

# The path prefix is the point: a project page is never served from the root, and an absolute asset
# path is invisible until it is.
ROOTDIR=$(mktemp -d)
trap 'rm -rf "$ROOTDIR"' EXIT
mkdir -p "$ROOTDIR/legibility"
cp "$DIST/index.html" "$ROOTDIR/legibility/"

python3 -m http.server "$PORT" --directory "$ROOTDIR" >/dev/null 2>&1 &
server=$!
trap 'kill $server 2>/dev/null; rm -rf "$ROOTDIR"' EXIT
for _ in $(seq 1 40); do
  curl -fsS -o /dev/null "http://127.0.0.1:$PORT/legibility/" 2>/dev/null && break
  sleep 0.25
done

DOM=$(mktemp)
"$CHROME" --headless --disable-gpu --no-sandbox --virtual-time-budget=20000 \
  --dump-dom "http://127.0.0.1:$PORT/legibility/#autorun" >"$DOM" 2>/dev/null

if python3 - "$DOM" <<'PY'
import html, json, re, sys

s = open(sys.argv[1], encoding="utf-8", errors="replace").read()


def attr_hidden(el_id: str) -> bool:
    m = re.search(r'id="%s"([^>]*)>' % el_id, s)
    return bool(m) and " hidden" in m.group(1)


m = re.search(r'<pre class="json" id="json">(.*?)</pre>', s, re.S)
raw = html.unescape(m.group(1)) if m else ""
ran = raw.strip() not in ("", "—")
d = json.loads(raw) if ran else {}
note = re.sub(r"<[^>]+>", "", (re.search(r'id="urlnote"[^>]*>(.*?)</p>', s, re.S) or [""," "])[1])

checks = [
    ("extraction ran with no helper reachable", ran),
    ("schema_version is 1", d.get("schema_version") == 1),
    ("an article was selected", d.get("article") is not None),
    ("comments were found", (d.get("comments") or {}).get("count", 0) > 0),
    ("stage reports static", 'id="stage"' in s and "static" in (re.search(r'id="stage"[^>]*>(.*?)<', s) or ["", ""])[1]),
    ("URL row hidden", attr_hidden("urlrow")),
    ("helper-port row hidden", attr_hidden("portrow")),
    ("the reason is stated on the page", "URL input is not offered" in note),
    # The rendered panel, not the file: the phrase "could not load" appears in the inlined script's
    # own error string, so searching the whole DOM for it fails on a page that worked perfectly.
    ("the article panel holds no error", 'class="err"' not in (re.search(r'<div id="article">(.*?)</div>\s*<h2', s, re.S) or ["", ""])[1]),
]
bad = [name for name, ok in checks if not ok]
for name, ok in checks:
    print(f"        {'ok  ' if ok else 'FAIL'}  {name}")
sys.exit(1 if bad else 0)
PY
then
  say PASS "static deploy extracts, and offers no control that needs a server"
else
  say FAIL "see the per-check list above"
  fail=1
fi
rm -f "$DOM"

echo
[ "$fail" = 0 ] && echo "gh-pages behaviour: ALL PASS" || echo "gh-pages behaviour: FAILED"
exit "$fail"
