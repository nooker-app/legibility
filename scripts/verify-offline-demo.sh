#!/usr/bin/env bash
# Verify the single-file demo really works with no server and no network.
#
# "Offline" is a claim about behaviour, so it is checked by loading the file in a browser from
# file:// rather than by inspecting the source. Two independent checks: the file contains no
# loadable external reference, and a headless run produces a correct extraction.
set -uo pipefail
cd "$(dirname "$0")/.."
FILE="js/testbed/legibility-offline.html"
fail=0

[ -f "$FILE" ] || { echo "FAIL  $FILE missing — run scripts/build-offline-demo.py"; exit 1; }

echo "offline demo verification"

# 1. No loadable external reference. Prose may mention a URL; a src/href/import may not fetch one.
ext=$(grep -oE '(src|href)="https?://[^"]*"|@import[^;]*|importScripts\(' "$FILE" | head -5)
if [ -n "$ext" ]; then
  printf '  FAIL  external references present:\n%s\n' "$ext"; fail=1
else
  printf '  PASS  no external src/href/import\n'
fi

# 2. The module is embedded, not referenced. Checked in Python, not grep: the base64 payload is one
#    ~800 KB line and BSD grep gives up on lines that long, which made this report a false failure
#    while every behavioural check passed.
if python3 scripts/check-embed.py "$FILE"; then
  printf '  PASS  wasm embedded as base64 (no fetch of a sibling .wasm)\n'
else
  printf '  FAIL  wasm is not embedded\n'; fail=1
fi

# 3. Headless run from file://, with the extraction asserted.
CHROME=$(ls -d "$HOME"/.cache/puppeteer/chrome/*/chrome-mac*/Google\ Chrome\ for\ Testing.app/Contents/MacOS/* 2>/dev/null | head -1)
[ -n "$CHROME" ] || CHROME="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
if [ ! -x "$CHROME" ]; then
  printf '  SKIP  no Chrome found; behavioural check not run\n'
else
  "$CHROME" --headless --disable-gpu --no-sandbox --allow-file-access-from-files \
    --virtual-time-budget=8000 --dump-dom "file://$PWD/$FILE#autorun" >/tmp/lgb-offline-dom.html 2>/dev/null
  if python3 - <<'PY'
import re, json, html, sys
s = open('/tmp/lgb-offline-dom.html', encoding='utf-8', errors='replace').read()
m = re.search(r'<pre class="json" id="json">(.*?)</pre>', s, re.S)
if not m:
    print('        no JSON produced — the module did not run'); sys.exit(1)
d = json.loads(html.unescape(m.group(1)))
art = re.search(r'<div id="article">(.*?)</div>\s*<h2', s, re.S)
body = art.group(1) if art else ''
checks = [
    ('schema_version is 1',        d.get('schema_version') == 1),
    ('article selected',           d.get('article') is not None),
    ('article is <article>',       (d.get('article') or {}).get('tag') == 'article'),
    ('title extracted',            bool(d['metadata'].get('title'))),
    ('title is verbatim',          (d['metadata'].get('title') or {}).get('verbatim_ok') is True),
    ('author extracted',           bool(d['metadata'].get('authors'))),
    ('tz_known from +09:00',       (d['metadata'].get('published') or {}).get('tz_known') is True),
    ('3 comments found',           d['comments']['count'] == 3),
    ('claimed total matches',      d['comments']['completeness']['claimed_total'] == 3),
    ('not reported truncated',     d['comments']['completeness']['truncated'] is False),
    ('button label not in body',   '코드 복사' not in body),
    ('hidden promo not in body',   '프로모션' not in body),
    ('script content not in body', 'tracking' not in body),
]
bad = [n for n, ok in checks if not ok]
for n, ok in checks:
    print(f'        {"ok  " if ok else "FAIL"}  {n}')
sys.exit(1 if bad else 0)
PY
  then printf '  PASS  headless run from file:// produced a correct extraction\n'
  else printf '  FAIL  headless run from file:// did not\n'; fail=1
  fi
fi

# 4. The two ways of opening the demo must be the same demo. Same input, same JSON, or the claim
#    that `lgb serve` and the single file agree is only a claim. They share one template and one
#    WebAssembly module, so this checks that the wiring did not diverge, not the engine.
PORT=8898
if [ ! -x ./target/release/lgb ]; then
  printf '  SKIP  lgb not built; two-mode equivalence not checked\n'
elif [ ! -x "$CHROME" ]; then
  printf '  SKIP  no Chrome found; two-mode equivalence not checked\n'
else
  ./target/release/lgb serve --port "$PORT" >/tmp/lgb-verify-serve.log 2>&1 &
  serve_pid=$!
  sleep 1
  "$CHROME" --headless --disable-gpu --no-sandbox --virtual-time-budget=8000 \
    --dump-dom "http://127.0.0.1:$PORT/#autorun" >/tmp/lgb-served-dom.html 2>/dev/null
  kill "$serve_pid" 2>/dev/null
  wait "$serve_pid" 2>/dev/null
  if python3 - <<'PY2'
import html, re, sys
def payload(path):
    s = open(path, encoding='utf-8', errors='replace').read()
    m = re.search(r'<pre class="json" id="json">(.*?)</pre>', s, re.S)
    return html.unescape(m.group(1)) if m else None
a, b = payload('/tmp/lgb-offline-dom.html'), payload('/tmp/lgb-served-dom.html')
if a is None: print('        the single-file run produced no JSON'); sys.exit(1)
if b is None: print('        the served run produced no JSON'); sys.exit(1)
if a != b:
    print('        the two modes disagree on the same input')
    sys.exit(1)
print(f'        identical output, {len(a)} bytes')
PY2
  then
    printf '  PASS  file:// and lgb serve produce identical output\n'
  else
    printf '  FAIL  file:// and lgb serve disagree\n'; fail=1
  fi
fi

# 5. URL input from file://. "Offline" means nothing but this machine is involved, not that the
#    network is unreachable -- so the single file may ask a helper on localhost to fetch a page.
#    Whether that hop works cannot be read off the source: a file:// document has origin `null`, so
#    it depends on the helper's CORS header being right.
#
#    The page fetched is served by a second local server, not by the helper itself: the helper takes
#    one connection at a time, so asking it to fetch its own URL deadlocks it against itself.
if [ ! -x ./target/release/lgb ] || [ ! -x "$CHROME" ]; then
  printf '  SKIP  url-from-file:// not checked\n'
else
  ORIGIN_PORT=8897
  tmp=$(mktemp -d)
  cat > "$tmp/page.html" <<'HTML'
<!doctype html><html><head><title>Fetched through the helper</title></head><body>
<main><article>
<h1>Fetched through the helper</h1>
<p>This page was served by a local origin, pulled in by lgb serve, and extracted in the browser.</p>
<p>If you can read this sentence in the article pane, the file:// to helper hop works.</p>
</article></main></body></html>
HTML
  ( cd "$tmp" && python3 -m http.server "$ORIGIN_PORT" >/dev/null 2>&1 & echo $! > "$tmp/origin.pid" )
  ./target/release/lgb serve --port "$PORT" >/tmp/lgb-verify-serve2.log 2>&1 &
  serve_pid=$!
  sleep 1.2
  enc=$(python3 -c "import urllib.parse,sys;print(urllib.parse.quote('http://127.0.0.1:'+sys.argv[1]+'/page.html',safe=''))" "$ORIGIN_PORT")
  "$CHROME" --headless --disable-gpu --no-sandbox --allow-file-access-from-files \
    --virtual-time-budget=12000 \
    --dump-dom "file://$PWD/$FILE#url=$enc&port=$PORT" >/tmp/lgb-url-dom.html 2>/dev/null
  kill "$serve_pid" 2>/dev/null; wait "$serve_pid" 2>/dev/null
  kill "$(cat "$tmp/origin.pid")" 2>/dev/null
  rm -rf "$tmp"
  if python3 - <<'PY2'
import re, sys
s = open('/tmp/lgb-url-dom.html', encoding='utf-8', errors='replace').read()
err = re.search(r'<div id="article">\s*<p class="err">(.*?)</p>', s, re.S)
if err:
    print('        ' + re.sub(r'<[^>]+>', '', err.group(1)).strip()[:200]); sys.exit(1)
size = re.search(r'<p class="note" id="insize">(.*?)</p>', s, re.S)
if not size or 'received' not in size.group(1):
    print('        the fetched page never reached the engine'); sys.exit(1)
if 'helper hop works' not in s:
    print('        the fetched body is not in the article pane'); sys.exit(1)
print('        ' + size.group(1).strip())
PY2
  then
    printf '  PASS  file:// fetched a URL through the local helper\n'
  else
    printf '  FAIL  file:// could not fetch through the local helper\n'; fail=1
  fi
fi

echo
[ "$fail" = 0 ] && echo "offline demo: ALL PASS" || echo "offline demo: FAILURES PRESENT"
exit $fail
