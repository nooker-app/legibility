#!/usr/bin/env python3
"""Build a single self-contained HTML file that runs the engine with no server.

Why one file with the module inlined as base64, rather than an HTML plus a sibling `.wasm`:
opening a page from `file://` and `fetch()`ing a neighbouring file is blocked by CORS in every
browser, so the usual "load the wasm next door" layout only works behind a web server. Inlining
costs 33% in size and buys a demo you can double-click, mail to someone, or open on a plane.

Usage:
    python3 scripts/build-offline-demo.py [output.html]
"""

from __future__ import annotations

import base64
import hashlib
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WASM = ROOT / "target/wasm32-unknown-unknown/release/legibility_wasm.wasm"

# Which page to inline the module into. The demo is the default; `--template` selects another, so
# the reader view is built by the same code rather than by a second script that would drift from it.
TEMPLATES = {
    "demo": ROOT / "js/testbed/demo.html",
    "reader": ROOT / "js/reader/reader.html",
}
TEMPLATE = TEMPLATES["demo"]


def build_wasm() -> Path:
    subprocess.run(
        ["cargo", "build", "--release", "-p", "legibility-wasm",
         "--target", "wasm32-unknown-unknown"],
        cwd=ROOT, check=True,
    )
    if not WASM.exists():
        sys.exit(f"expected {WASM} to exist after the build")

    # wasm-opt is optional. Its absence must not silently produce a bigger artifact with no
    # explanation, so say so either way.
    if shutil.which("wasm-opt"):
        opt = WASM.with_name("legibility_wasm.opt.wasm")
        subprocess.run(
            ["wasm-opt", "-O3", "--enable-bulk-memory", str(WASM), "-o", str(opt)],
            check=True,
        )
        before, after = WASM.stat().st_size, opt.stat().st_size
        print(f"  wasm-opt: {before // 1024} KB -> {after // 1024} KB")
        return opt
    print("  wasm-opt not found (brew install binaryen) — shipping the unoptimized module")
    return WASM


def main() -> int:
    global TEMPLATE
    argv = sys.argv[1:]
    if "--template" in argv:
        at = argv.index("--template")
        TEMPLATE = TEMPLATES[argv[at + 1]]
        del argv[at : at + 2]
        sys.argv = [sys.argv[0], *argv]

    # Resolved, so a relative argument works. `dist/index.html` is how the gh-pages bundle is built,
    # and the `relative_to(ROOT)` in the report below raises on a path that is not already absolute.
    out_path = (
        Path(sys.argv[1]).resolve()
        if len(sys.argv) > 1
        else ROOT / "js/testbed/legibility-offline.html"
    )
    wasm = build_wasm()
    b64 = base64.b64encode(wasm.read_bytes()).decode("ascii")
    template = TEMPLATE.read_text(encoding="utf-8")

    marker = "/*__WASM_BASE64__*/"
    if marker not in template:
        sys.exit(f"{TEMPLATE} is missing the {marker} placeholder")
    html = template.replace(marker, b64)

    # Identify the build on the page itself. Which commit produced this module cannot be recovered
    # from inside a browser, and a demo running a stale binary is indistinguishable from a fix that
    # did not work -- a confusion that has already cost two rounds of diagnosis.
    stamp_marker = "/*__BUILD_STAMP__*/"
    if stamp_marker not in template:
        sys.exit(f"{TEMPLATE} is missing the {stamp_marker} placeholder")
    head = subprocess.run(
        ["git", "rev-parse", "--short", "HEAD"],
        cwd=ROOT, capture_output=True, text=True, check=False,
    ).stdout.strip() or "nogit"
    dirty = subprocess.run(
        ["git", "status", "--porcelain"], cwd=ROOT, capture_output=True, text=True, check=False,
    ).stdout.strip()
    # The wasm digest is the part that cannot lie: the commit says what the tree was, this says
    # what was actually compiled.
    digest = hashlib.sha256(wasm.read_bytes()).hexdigest()[:8]
    html = html.replace(stamp_marker, f"{head}{'+dirty' if dirty else ''} · wasm {digest}")

    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(html, encoding="utf-8")

    size = out_path.stat().st_size
    shown = out_path.relative_to(ROOT) if out_path.is_relative_to(ROOT) else out_path
    print(f"  wrote {shown}  ({size // 1024} KB, single file, no server)")
    # Anything the page loads from the network would break the offline promise, so check rather
    # than trust: no src/href pointing outward, no import, no fetch of a URL.
    # One implementation of "would this load from off-host", shared with pages-guard.sh and
    # verify-offline-demo.sh. The substring sweep this replaces warned on the repo link in the
    # header -- an `<a href>` fetches nothing until clicked -- and a guard that cries wolf gets
    # ignored, which is worse than not having one.
    if subprocess.run(
        [sys.executable, str(ROOT / "scripts/check-external-refs.py"), str(out_path)],
        check=False,
    ).returncode != 0:
        print("  WARNING: the page would load something from another host (listed above)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
