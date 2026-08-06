#!/usr/bin/env python3
"""Render an `lgb extract` result next to the source page, for eyeballing.

Not a product surface -- the real testbed lands in M2. This exists because a JSON blob does not
tell you whether the extraction is *right*, and M0 needs someone to look at the output before it
is trusted.
"""
import html
import json
import subprocess
import sys
from pathlib import Path

src = Path(sys.argv[1])
out = Path(sys.argv[2])

raw = subprocess.run(
    ["cargo", "run", "-q", "-p", "legibility-cli", "--", "extract", str(src)],
    capture_output=True, text=True, check=True,
).stdout
data = json.loads(raw)
art = data.get("article")
diag = data["diagnostics"]

total = max(1, diag["page_prose_len"] + diag["page_control_len"]
            + diag["page_hidden_len"] + diag["page_alt_len"])
bars = [
    ("prose", diag["page_prose_len"], "#2f6f4f"),
    ("control", diag["page_control_len"], "#8a5a2b"),
    ("hidden", diag["page_hidden_len"], "#6b3f7a"),
    ("alt (sr-only)", diag["page_alt_len"], "#2b5f8a"),
]

rows = "".join(
    f'<tr><td>{html.escape(n)}</td><td class=num>{v}</td>'
    f'<td class=barcell><span class=bar style="width:{v*100/total:.1f}%;background:{c}"></span></td>'
    f'<td class=num>{v*100/total:.1f}%</td></tr>'
    for n, v, c in bars
)

article_html = (
    f'<p class=meta>region <code>&lt;{html.escape(art["tag"])}&gt;</code> · '
    f'prose_len {art["prose_len"]} · calibrated {str(art["calibrated"]).lower()}</p>'
    f'<div class=extracted>{html.escape(art["text"])}</div>'
) if art else f'<p class=none>no article — {html.escape(data.get("no_article","?"))}</p>'

out.write_text(f"""<!doctype html><meta charset=utf-8>
<title>legibility — extraction of {html.escape(src.name)}</title>
<style>
 :root {{ color-scheme: light dark; }}
 body {{ font: 15px/1.6 ui-sans-serif,-apple-system,'Apple SD Gothic Neo',sans-serif;
        max-width: 60rem; margin: 2rem auto; padding: 0 1rem; }}
 h1 {{ font-size: 1.3rem; }}
 h2 {{ font-size: 1rem; margin-top: 2rem; border-bottom: 1px solid #8884; padding-bottom: .3rem; }}
 .extracted {{ border-left: 3px solid #2f6f4f; padding: .6rem 0 .6rem 1rem; white-space: pre-wrap; }}
 .none {{ color: #a33; }}
 .meta, .note {{ color: #6668; font-size: .85rem; }}
 table {{ border-collapse: collapse; width: 100%; }}
 td {{ padding: .25rem .5rem; vertical-align: middle; }}
 .num {{ text-align: right; font-variant-numeric: tabular-nums; white-space: nowrap; }}
 .barcell {{ width: 60%; }}
 .bar {{ display: inline-block; height: .8rem; border-radius: 2px; }}
 pre {{ overflow-x: auto; background: #8881; padding: .6rem; border-radius: 4px; font-size: .8rem; }}
</style>
<h1>legibility — extraction of <code>{html.escape(src.name)}</code></h1>
<p class=note>M0 placeholder scorer (<code>text_density × (1 − link_density)</code>). Quality is not
the point yet; the point is that the pipeline runs and the four text roles are separated
<em>before</em> scoring rather than after.</p>

<h2>Extracted article</h2>
{article_html}

<h2>Where the page's text went</h2>
<table>{rows}</table>
<p class=note>Only <strong>prose</strong> reaches any statistic. Readability counts all of it,
which is why icon ligatures, "copy code" labels and <code>hidden</code> promo blocks distort its
scores. <code>alt (sr-only)</code> is excluded from the reader output but preserved, not deleted —
it is legitimate content for assistive technology.</p>

<h2>Raw output</h2>
<pre>{html.escape(json.dumps(data, indent=2, ensure_ascii=False))}</pre>
""", encoding="utf-8")
print(f"wrote {out}")
