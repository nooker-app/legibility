#!/usr/bin/env bash
# M0 exit gate. Every item is executed, not asserted. Exits non-zero on the first failure so it
# can be wired straight into CI.
set -uo pipefail
fail=0
chk() { if eval "$2" >/dev/null 2>&1; then printf '  PASS  %s\n' "$1"; else printf '  FAIL  %s\n' "$1"; fail=1; fi; }

echo "M0 exit gate"
chk "cargo test --workspace"                     "cargo test --workspace -q"
chk "clippy -D warnings (all targets)"           "cargo clippy --workspace --all-targets -q -- -D warnings"
chk "cargo deny check licenses"                  "cargo deny check licenses"
chk "legibility-core has no html5ever"           "! cargo tree -p legibility-core | grep -q html5ever"
chk "legibility-core builds for wasm32-unknown"  "cargo build -q --release -p legibility-core --target wasm32-unknown-unknown"
chk "legibility-core builds for wasm32-wasip1"   "cargo build -q --release -p legibility-core --target wasm32-wasip1"
chk "iOS reachability: aarch64-apple-ios"        "cargo build -q --release -p legibility-core --target aarch64-apple-ios"
chk "iOS reachability: aarch64-apple-ios-sim"    "cargo build -q --release -p legibility-core --target aarch64-apple-ios-sim"
chk "no direct recursion in core"                "python3 scripts/check-no-recursion.py"
chk "no third-party .html committed (D9)"        "[ \"\$(git ls-files corpus/ | grep -c '\\.html\$')\" = 0 ]"
chk "extern \"C\" bodies wrapped (S1/§1.11.2)"     "python3 scripts/ffi-audit.py"
# The demo's CNAME guard (§1.12.2). Here rather than only in the Pages workflow because the danger
# is a *committed* CNAME: a custom domain belongs to one repository, so one landing on main would
# move www.nooker.app off nooker-web and take production down. Cheap, and needs no bundle.
# An explicitly absent bundle directory, so only the CNAME leg can decide this. Pointed at a real
# `dist/`, any of the six checks could fail it and `chk` swallows the output, so a developer with a
# leftover bundle would read "no CNAME" for an absolute-path problem.
chk "no CNAME anywhere (§1.12.2)"                 "scripts/pages-guard.sh .m0-no-bundle >/dev/null"
chk "corpus submodule pinned to expected SHA"    "[ \"\$(git -C corpus/readability rev-parse HEAD)\" = ab4027a8b37669745016869a37a504727992b2ba ]"
chk "tier_a case count matches manifest"         "[ \"\$(ls corpus/readability/test/test-pages | wc -l | tr -d ' ')\" = 130 ]"

# Byte-identical output across targets is the load-bearing S3 gate.
#
# It used to be one hand-written fixture, and that fixture was a single line -- the one shape of
# document that *cannot* diverge. Meanwhile the real answer was that native and wasm32 disagreed on
# **123 of the 130 corpus pages**, down to different headlines and lost bylines, because html5ever
# chunks text differently per target and the sink turned chunking into arena shape. A gate that tests
# the only safe input is not a gate.
#
# So: every corpus page, both targets, compared byte for byte. It costs about a minute and it is the
# only thing standing between S3 and another silent divergence.
cargo build -q --release -p legibility-cli
cargo build -q --release -p legibility-cli --target wasm32-wasip1
S3TMP=$(mktemp -d); trap 'rm -rf "$S3TMP"' EXIT
mismatch=0; compared=0
for d in corpus/readability/test/test-pages/*/; do
  [ -f "$d/source.html" ] || continue
  cp "$d/source.html" "$S3TMP/src.html"
  n=$(./target/release/lgb extract "$S3TMP/src.html" 2>/dev/null | shasum -a 256 | cut -d' ' -f1)
  w=$(wasmtime run --dir="$S3TMP" target/wasm32-wasip1/release/lgb.wasm extract "$S3TMP/src.html" \
        2>/dev/null | shasum -a 256 | cut -d' ' -f1)
  compared=$((compared + 1))
  if [ -z "$n" ] || [ "$n" != "$w" ]; then
    mismatch=$((mismatch + 1))
    [ "$mismatch" -le 5 ] && printf '        %s: native %s != wasip1 %s\n' \
      "$(basename "$d")" "${n:0:12}" "${w:0:12}"
  fi
done
# A newline *inside* a text run is the shape that used to diverge; keep one explicitly so the check
# still means something if the corpus submodule is ever absent.
printf '<html><body><nav><a href=#>n</a></nav><article><p>alpha beta\ngamma</p></article></body></html>' \
  > "$S3TMP/nl.html"
n=$(./target/release/lgb extract "$S3TMP/nl.html" 2>/dev/null | shasum -a 256 | cut -d' ' -f1)
w=$(wasmtime run --dir="$S3TMP" target/wasm32-wasip1/release/lgb.wasm extract "$S3TMP/nl.html" \
      2>/dev/null | shasum -a 256 | cut -d' ' -f1)
compared=$((compared + 1))
[ -n "$n" ] && [ "$n" = "$w" ] || { mismatch=$((mismatch + 1)); printf '        newline-in-text fixture diverges\n'; }

if [ "$mismatch" = 0 ] && [ "$compared" -gt 1 ]; then
  printf '  PASS  native == wasip1 on all %s inputs (S3)\n' "$compared"
else
  printf '  FAIL  %s of %s inputs differ between native and wasip1 (S3)\n' "$mismatch" "$compared"
  fail=1
fi

echo
[ "$fail" = 0 ] && echo "M0 gate: ALL PASS" || echo "M0 gate: FAILURES PRESENT"
exit $fail
