#!/usr/bin/env python3
"""Report absolute asset paths in a demo page's markup.

A GitHub project page is served under a path prefix -- `https://nooker-app.github.io/legibility/` --
so `src="/foo"` resolves to the *user* site and 404s. Relative paths are the only ones that work,
which is a mistake that is invisible locally and total once deployed.

Only the markup is examined. Everything from the first `<script` onward is data, and the demo's
built-in sample is a page we wrote which legitimately contains `href="/u/aaa"` and `href="/about"`;
a plain grep over the whole file reports those as broken assets and the guard cries wolf.

Exit 0 when clean, 1 when something absolute is loadable.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

# `//host/x` is protocol-relative and points at another origin: caught by the external-reference
# check instead, and excluded here so one fault is not reported by two guards.
ABSOLUTE = re.compile(r'(?:src|href)="(/[^/"][^"]*)"')


def main() -> int:
    if len(sys.argv) < 2:
        print("usage: check-abs-paths.py <html>", file=sys.stderr)
        return 2
    bad: list[tuple[str, str]] = []
    for arg in sys.argv[1:]:
        text = Path(arg).read_text(encoding="utf-8", errors="replace")
        cut = text.find("<script")
        markup = text if cut < 0 else text[:cut]
        bad += [(arg, hit) for hit in ABSOLUTE.findall(markup)]
    for arg, hit in bad[:10]:
        print(f"        {arg}: {hit}")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
