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
H=/tmp/m0-gate-fixture.html
printf '<html><body><nav><a href=#>n</a></nav><article><p>alpha beta gamma</p></article></body></html>' > "$H"
NAT=$(cargo run -q -p legibility-cli -- extract "$H" | shasum -a 256 | cut -d' ' -f1)
cargo build -q --release -p legibility-cli --target wasm32-wasip1
WAS=$(wasmtime run --dir=/tmp target/wasm32-wasip1/release/lgb.wasm extract "$H" 2>/dev/null | shasum -a 256 | cut -d' ' -f1)
if [ -n "$NAT" ] && [ "$NAT" = "$WAS" ]; then
  printf '  PASS  native == wasip1 byte-identical output (S3)\n'
else
  printf '  FAIL  native(%s) != wasip1(%s)\n' "${NAT:0:12}" "${WAS:0:12}"; fail=1
fi

echo
[ "$fail" = 0 ] && echo "M0 gate: ALL PASS" || echo "M0 gate: FAILURES PRESENT"
exit $fail
