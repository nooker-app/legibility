"""Assert the built single-file demo carries the module rather than fetching it.

Checked in Python, not grep: the base64 payload is one ~800 KB line and BSD grep gives up on lines
that long, which once made this report a false failure while every behavioural check passed.

The template now has two delivery routes — embedded base64 for `file://`, a sibling `.wasm` for
`lgb serve` — so "contains no fetch of a .wasm" is no longer the right question; the template always
contains one. The right question is whether this *build* can reach it, which is decided entirely by
whether the placeholder was substituted.
"""

import re
import sys

src = open(sys.argv[1], encoding="utf-8").read()

m = re.search(r'WASM_BASE64 = "([^"]*)"', src)
if not m:
    print("        no WASM_BASE64 assignment found")
    sys.exit(1)
payload = m.group(1)

if payload.startswith("/*"):
    print("        placeholder never substituted — this build would fetch the module")
    sys.exit(1)
if not re.fullmatch(r"[A-Za-z0-9+/=]*", payload) or len(payload) < 100_000:
    print(f"        payload absent, malformed, or implausibly small ({len(payload)} chars)")
    sys.exit(1)

# What makes the fetch unreachable once the payload is present. Two page shapes satisfy this, and
# the check is about the property rather than about either of them:
#
#   * the demo has two delivery routes and picks the payload first, so the guard is the branch;
#   * the reader view has no network route at all, which is the stronger form — there is no fetch
#     of a module to be preferred over.
#
# Requiring the demo's exact line failed the reader page while every behavioural check passed, which
# is a guard describing one implementation rather than the thing being guarded.
prefers_payload = "if (EMBEDDED) return b64ToBytes(WASM_BASE64);" in src
fetches_module = re.search(r'fetch\(\s*["\'`][^"\'`]*\.wasm', src) is not None
if fetches_module and not prefers_payload:
    print("        the embedded payload is not preferred over the network route")
    sys.exit(1)

print(
    f"        {len(payload) // 1024} KB of base64 embedded, "
    + ("network route unreachable" if fetches_module else "no network route in the page")
)
