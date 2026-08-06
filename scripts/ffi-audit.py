#!/usr/bin/env python3
"""Assert every `#[no_mangle] extern "C"` body is wrapped in `catch_unwind` (plan §1.11.2).

Unwinding out of an `extern "C"` function is undefined behaviour, and `panic = abort` is not an
acceptable alternative for a library: aborting inside a static library embedded in an iOS app kills
the host app, which is a worse failure than any wrong answer. So the boundary converts a panic into
a null or an error value instead.

The wasm build *is* `panic = abort` (`.cargo/config.toml`), where the wrapper cannot catch anything —
a wasm panic traps the instance. It is still required there, because the same source is compiled for
native targets where it does catch, and because a boundary that is sometimes wrapped is a boundary
nobody can reason about.

An earlier version of this check simply asserted that no `#[no_mangle]` existed anywhere. That
passed while `legibility-ffi` was an empty stub and started failing the moment real FFI arrived —
counting zero is not the same as checking a property.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
NO_MANGLE = re.compile(r'#\[no_mangle\][\s\S]{0,80}?\bfn\s+(\w+)\s*\(')


def body_of(src: str, header_end: int) -> str:
    """Slice a function body from its opening brace to the matching close."""
    open_idx = src.find("{", header_end)
    if open_idx < 0:
        return ""
    depth = 0
    for i in range(open_idx, len(src)):
        if src[i] == "{":
            depth += 1
        elif src[i] == "}":
            depth -= 1
            if depth == 0:
                return src[open_idx : i + 1]
    return src[open_idx:]


def main() -> int:
    unwrapped: list[str] = []
    checked = 0

    for path in sorted((ROOT / "crates").rglob("*.rs")):
        if "/tests/" in str(path) or path.name.startswith("test"):
            continue
        src = path.read_text(encoding="utf-8")
        for m in NO_MANGLE.finditer(src):
            name = m.group(1)
            body = body_of(src, m.end())
            checked += 1
            # A trivial constant-returning export cannot panic and needs no wrapper, but proving
            # that automatically is not worth the machinery: require the wrapper everywhere and let
            # the cost be one line.
            if "catch_unwind" not in body:
                rel = path.relative_to(ROOT)
                unwrapped.append(f"{rel}: {name}")

    if unwrapped:
        print('FAIL: `extern "C"` bodies not wrapped in catch_unwind')
        for u in unwrapped:
            print(f"  {u}")
        return 1

    print(f'PASS: all {checked} #[no_mangle] functions wrap their body in catch_unwind')
    return 0


if __name__ == "__main__":
    sys.exit(main())
