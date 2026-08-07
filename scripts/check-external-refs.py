#!/usr/bin/env python3
"""Report references that would make a demo page *load* something from another host.

The offline and gh-pages builds both promise that opening the page involves nothing but the file
itself. That promise is about **loads**, not about links: a `<link rel=stylesheet href=…>`, a
`<script src=…>`, an `<img src=…>`, an `@import` or an `importScripts()` fetches from the network
before the user does anything, and any of those breaks the claim.

An `<a href="https://github.com/…">` does not. It is navigation the reader chooses, it fetches
nothing until clicked, and the demo carries one on purpose so the source is findable. An earlier
version of this check grepped for `href="http` across the whole file and failed the build on that
link — a guard that cannot tell a load from a link will eventually be switched off, which is worse
than not having it.

Exit 0 when clean, 1 when something would load from off-host.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

# `src` on any element is always a load. `href` counts only on the elements that fetch: `<link>`
# and `<base>`. Anchors and `<area>` are navigation.
PATTERNS = [
    ("src", re.compile(r'\bsrc\s*=\s*"(https?:)?//[^"]*"', re.I)),
    ("link/base href", re.compile(r'<(?:link|base)\b[^>]*\bhref\s*=\s*"(?:https?:)?//[^"]*"', re.I)),
    ("@import", re.compile(r"@import\s+(?:url\()?['\"]?(?:https?:)?//", re.I)),
    ("importScripts", re.compile(r"\bimportScripts\s*\(", re.I)),
    ("fetch of an absolute URL", re.compile(r"""\bfetch\s*\(\s*['"`](?:https?:)?//""", re.I)),
    ("XHR to an absolute URL", re.compile(r"""\.open\s*\(\s*['"][A-Z]+['"]\s*,\s*['"`](?:https?:)?//""", re.I)),
]


def main() -> int:
    if len(sys.argv) < 2:
        print("usage: check-external-refs.py <html>...", file=sys.stderr)
        return 2
    hits: list[str] = []
    for arg in sys.argv[1:]:
        text = Path(arg).read_text(encoding="utf-8", errors="replace")
        for label, pat in PATTERNS:
            for m in pat.finditer(text):
                hits.append(f"        {arg}: {label}: {m.group(0)[:110]}")
    for h in hits[:10]:
        print(h)
    return 1 if hits else 0


if __name__ == "__main__":
    sys.exit(main())
