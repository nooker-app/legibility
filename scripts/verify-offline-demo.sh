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

echo
[ "$fail" = 0 ] && echo "offline demo: ALL PASS" || echo "offline demo: FAILURES PRESENT"
exit $fail
