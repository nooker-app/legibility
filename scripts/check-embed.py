import re, sys
src = open(sys.argv[1], encoding='utf-8').read()
m = re.search(r'WASM_BASE64 = "([A-Za-z0-9+/=]*)"', src)
if not m or len(m.group(1)) < 100_000:
    print('        base64 payload absent or implausibly small'); sys.exit(1)
if re.search(r'fetch\([^)]*\.wasm', src):
    print('        the page fetches a sibling .wasm'); sys.exit(1)
print(f'        {len(m.group(1)) // 1024} KB of base64 embedded')
