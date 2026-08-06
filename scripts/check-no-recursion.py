#!/usr/bin/env python3
"""Assert that legibility-core contains no direct recursion (guarantee S1).

Deeply nested markup is a one-line remote input, and a stack overflow cannot be caught --
`catch_unwind` does not see it and there is no `try_grow_stack`. So every tree walk in the
core must be iterative with an explicit `Vec` stack, bounded by `Limits::max_depth`.

This is a lint, not a proof. It catches direct self-calls, which is the mistake people
actually make when translating a recursive algorithm. Mutual recursion across two functions
is not detected; that is a known limit, recorded here rather than left implicit. The
256 KiB-stack survival test on the deepest adversarial fixture is what actually covers the
general case.

Exits non-zero on the first violation so it can be wired straight into CI.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

FN_RE = re.compile(r"\bfn\s+(\w+)\s*(?:<[^>]*>)?\s*\(")

# A call to `name(` that is NOT `.name(` or `::name(`. Without this the checker reports
# `Arena::len` as recursive because its body contains `self.kind.len()`, which is a call to
# a *different* type's method that merely shares the name.
def self_call_re(name: str) -> re.Pattern[str]:
    return re.compile(r"(?<![.\w:])" + re.escape(name) + r"\s*\(")


def strip_noise(src: str) -> str:
    """Remove test modules and comments, which legitimately mention function names."""
    src = re.split(r"^#\[cfg\(test\)\]", src, maxsplit=1, flags=re.M)[0]
    keep = []
    for line in src.splitlines():
        stripped = line.lstrip()
        if stripped.startswith(("///", "//!", "//")):
            continue
        keep.append(line)
    return "\n".join(keep)


def balanced_body(src: str, open_idx: int) -> str:
    """Slice from `{` at open_idx to its matching `}`."""
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
    root = Path(sys.argv[1] if len(sys.argv) > 1 else "crates/legibility-core/src")
    violations: list[str] = []
    scanned = 0

    for path in sorted(root.rglob("*.rs")):
        body = strip_noise(path.read_text(encoding="utf-8"))
        for match in FN_RE.finditer(body):
            name = match.group(1)
            brace = body.find("{", match.end())
            if brace < 0:
                continue
            scanned += 1
            fn_body = balanced_body(body, brace)
            if self_call_re(name).search(fn_body):
                violations.append(f"{path}: fn {name} calls itself")

    if violations:
        print("FAIL: direct recursion found (S1 requires iterative walks)")
        for v in violations:
            print(f"  {v}")
        return 1

    print(f"PASS: no direct recursion in {scanned} functions under {root}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
