#!/usr/bin/env bash
# Guards for the gh-pages demo bundle (plan §1.12.2, §1.12.3).
#
# Every check here exists because of a specific way a static project page fails, and the first one
# is the only *dangerous* one: GitHub allows one repository per custom domain, so a CNAME file in
# this repo would take `www.nooker.app` away from `nooker-web` and the production site would go
# dark. That is why it is a hard failure and not a warning.
#
# Usage: scripts/pages-guard.sh [dist-dir]
set -uo pipefail
cd "$(dirname "$0")/.."
DIST="${1:-dist}"
fail=0
say() { printf '  %-5s %s\n' "$1" "$2"; }

echo "gh-pages guards ($DIST)"

# 1. CNAME. The one that can break production.
#
# Checked in three places because there are three ways one arrives: committed to the worktree,
# staged into the bundle by a build step, or added to the deployed branch by hand.
found_cname=0
[ -e CNAME ] && found_cname=1
[ -e "$DIST/CNAME" ] && found_cname=1
if git rev-parse --verify --quiet gh-pages >/dev/null 2>&1; then
  git ls-tree -r --name-only gh-pages | grep -qx 'CNAME' && found_cname=1
fi
if [ "$found_cname" = 1 ]; then
  say FAIL "a CNAME exists. This repo must never have one: GitHub binds a custom domain to exactly"
  printf '        one repository, so www.nooker.app would move here and nooker-web would go dark.\n'
  printf '        Serve at https://nooker-app.github.io/legibility/ instead.\n'
  fail=1
else
  say PASS "no CNAME anywhere (worktree, bundle, gh-pages branch)"
fi

# The CNAME check needs no bundle, and it is the one that protects production — so this script is
# runnable, and is run by `m0-gate.sh`, without anything having been built. The bundle checks then
# report SKIP rather than failing, because "not built" is not "built wrong".
if [ ! -f "$DIST/index.html" ]; then
  say SKIP "no $DIST/index.html — bundle checks not run"
  printf '        build it with: python3 scripts/build-offline-demo.py %s/index.html\n' "$DIST"
  echo
  [ "$fail" = 0 ] && echo "gh-pages guards: CNAME PASS, bundle SKIPPED" \
                  || echo "gh-pages guards: FAILED"
  exit "$fail"
fi

# 2. Absolute asset paths. A project page is served under /legibility/, so `/assets/x` is a 404.
#    Markup only -- see scripts/check-abs-paths.py for why the sample must be excluded.
if python3 scripts/check-abs-paths.py "$DIST/index.html"; then
  say PASS "no absolute asset paths"
else
  say FAIL "absolute asset paths (404 under the /legibility/ prefix), listed above"
  fail=1
fi

# 3. Nothing *loaded* from another host. Links are fine -- see scripts/check-external-refs.py for
#    why the distinction matters and how an earlier version of this failed on the repo link.
if python3 scripts/check-external-refs.py "$DIST/index.html"; then
  say PASS "nothing loads from another host"
else
  say FAIL "off-host loads present, listed above"
  fail=1
fi

# 4. The module travels inside the file. Pages cannot set Content-Type reliably for .wasm and
#    `instantiateStreaming` is fussy about it; embedding removes the question rather than answering
#    it. It is also what makes STATIC true in the page, which is what hides URL input.
if python3 scripts/check-embed.py "$DIST/index.html"; then
  say PASS "wasm embedded (no sibling .wasm to mislabel)"
else
  say FAIL "wasm is not embedded — the page would fetch ./legibility.wasm"
  fail=1
fi

# 5. No third-party corpus HTML in the bundle (D9). The repo may not distribute it and the demo has
#    no need of it: its samples are self-authored.
#
#    Written twice before this. `find -newer /dev/null` matches nothing on darwin, because
#    /dev/null's mtime is continuously refreshed -- so the check passed with a planted corpus page
#    in the bundle. And `grep -l … | head -1` under `set -o pipefail` makes a *match* exit nonzero
#    via SIGPIPE, so finding something read as finding nothing. No pipe, no `find` predicate.
if grep -rlq 'corpus/readability\|test-pages' "$DIST"; then
  say FAIL "a bundle file references corpus test-pages"
  grep -rl 'corpus/readability\|test-pages' "$DIST" | sed 's/^/        /'
  fail=1
else
  say PASS "no corpus HTML in the bundle (D9)"
fi

# 6. URL input must be absent on a static deploy, and the page must say why.
#
#    Presence of the mechanism only. The *behaviour* is asserted by scripts/verify-pages-demo.sh,
#    which serves the bundle under a path prefix and drives a real browser -- a grep cannot tell
#    whether a branch runs, and an earlier version of this comment cited that script before it
#    existed, which made a tautology look like it was backed by something.
if grep -q 'const STATIC' "$DIST/index.html" && grep -q 'id="urlrow"' "$DIST/index.html"; then
  say PASS "static-deploy detection and the hideable URL row are both present"
else
  say FAIL "the STATIC branch or #urlrow is gone; URL input would call Pages itself"
  fail=1
fi

echo
[ "$fail" = 0 ] && echo "gh-pages guards: ALL PASS" || echo "gh-pages guards: FAILED"
exit "$fail"
