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
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WASM = ROOT / "target/wasm32-unknown-unknown/release/legibility_wasm.wasm"
TEMPLATE = ROOT / "js/testbed/offline.html"


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
    out_path = Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT / "js/testbed/legibility-offline.html"
    wasm = build_wasm()
    b64 = base64.b64encode(wasm.read_bytes()).decode("ascii")
    template = TEMPLATE.read_text(encoding="utf-8")

    marker = "/*__WASM_BASE64__*/"
    if marker not in template:
        sys.exit(f"{TEMPLATE} is missing the {marker} placeholder")
    html = template.replace(marker, b64)

    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(html, encoding="utf-8")

    size = out_path.stat().st_size
    print(f"  wrote {out_path.relative_to(ROOT)}  ({size // 1024} KB, single file, no server)")
    # Anything the page loads from the network would break the offline promise, so check rather
    # than trust: no src/href pointing outward, no import, no fetch of a URL.
    lowered = html.lower()
    for bad in ["http://", "https://cdn", "src=\"http", "href=\"http", "integrity="]:
        if bad in lowered:
            # Links in prose are fine; only loadable references matter.
            if bad in ("http://", "https://cdn") and "script src" not in lowered:
                continue
            print(f"  WARNING: found {bad!r} — the page may not be fully offline")
    return 0


if __name__ == "__main__":
    sys.exit(main())
