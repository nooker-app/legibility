# Output schema (`schema_version: 1`)

One JSON object per document, from every host — `lgb extract`, the WebAssembly module, the reader
view. The same bytes on every target: guarantee S3 compares `blake3` of this string across native,
`wasm32-wasip1` and headless Chrome over the whole corpus on every CI run.

**Every key is always present.** A field with nothing to say is `null`, never absent. Key order is
part of the contract, so a diff of two outputs is a diff of two documents.

```jsonc
{
  "schema_version": 1,
  "url": null,                    // echoed if the caller supplied one
  "article": { … } | null,
  "no_article": null | "IndexPage" | "CommentsOnly" | "NoTextContent" | "LowConfidence"
                     | "TooFewCompetitors",
  "comments": { … },
  "metadata": { … },
  "page_kind": "article-or-discussion" | "listing",
  "comment_mask_reverted": false,
  "group_count": 19,
  "diagnostics": { … }
}
```

## The two questions to ask first

```js
if (d.article && d.article.kind === "article") { /* render d.article.html */ }
else if (d.article) { /* kind === "discussion-root": a pointer, see below */ }
else { /* nothing extractable; d.no_article says why */ }
```

`article` and `no_article` are exclusive: exactly one is non-null.

## `article`

```jsonc
{
  "html": "<article>…</article>",  // sanitized, Profile::Article
  "text": "…",                     // prose only, whitespace-normalized
  "tag": "div",                    // the element that was selected
  "kind": "article" | "discussion-root",
  "url": null,                     // the outbound link, on a submission
  "prose_len": 12375,              // bytes of `text`, net of exclusions
  "confidence": 8151,              // 0–10000
  "calibrated": false,             // permanently: see below
  "dropped_subtrees": 8,
  "unwrapped": 3,
  "truncated": false,              // `max_output_bytes` was reached
  "dispersion_floor": false,       // too few competitors to compare; priors used
  "from_semantic_anchor": true,    // <main>/<article>/role/itemprop said so
  "signal_conflict": false         // an anchor existed and was not believed
}
```

**`kind: "discussion-root"` means the page is a pointer**, not that extraction failed. A Hacker News
or Reddit link submission is a headline and an outbound URL with no body of its own, so `html` and
`text` are empty **by construction** and the payload is `metadata.title` plus `article.url`. Render
it as a link, not as an empty article. Returning nothing here would throw away the whole page.

**`confidence` is not a probability.** `calibrated: false` is permanent in v1 (plan D3): calibrating
one needs a labelled corpus, and claiming it without one is worse than admitting its absence. Use it
to *rank* or to gate at a threshold you pick yourself; do not read 8151 as 81.5% correct.

`html` is already sanitized. Insert it as markup — that is what it is for. Do not re-escape it, and
do not concatenate it into an attribute.

## `comments`

```jsonc
{
  "count": 2,
  "depth_source": "CssVariable" | "DomNesting" | "Indentation" | "Flat" | null,
  "completeness": {
    "present": 2,
    "claimed_total": 2,           // what the page said about itself, or null
    "truncated": false,
    "reason": null | "LoadMoreStub" | "Pagination" | "LazyScroll" | "LimitHit",
    "continuation": []            // URLs that would yield the rest; never followed
  },
  "items": [
    {
      "author": "구루",
      "timestamp": "2026-07-24T03:41:55.634Z",  // verbatim; not parsed, there is no clock
      "depth": 0,
      "parent": null,             // index into `items`
      "permalink": "#comment-1",
      "deleted": false,
      "text": "…",
      "html": "<div><p>…</p></div>"   // sanitized, Profile::UserContent
    }
  ]
}
```

**Check `completeness.truncated` before telling a user they have the whole thread.** You cannot
extract what is not in the HTML: Reddit ships a `load more` stub with the replies absent, Discourse
loads on scroll, forums paginate. `truncated: true` with `reason: null` means we know the thread is
short of `claimed_total` and cannot say why — which includes *we failed to detect the rest*, stated
rather than hidden.

`items` is flat with a `parent` index, not nested. Thread depth is attacker-controlled and a
recursive consumer is a stack overflow; walk it iteratively.

**`items[].html` is `UserContent`, not `Article`.** Stricter, because a comment is attacker-controlled
in a way an article body usually is not: images off, media dropped, `rel="nofollow noopener
noreferrer"` forced on every link. Do not render it through a path that assumes the article profile.

A `deleted` item is kept with empty text on purpose — removing it breaks the `parent` chain of its
replies, and a thread you cannot reassemble is worse than one containing a placeholder.

## `metadata`

Every field is a **candidate** carrying where it came from, never a synthesized value:

```jsonc
{
  "value": "자기 개선을 위한 하네스 엔지니어링 | GeekNews",
  "source": "open-graph",         // json-ld | open-graph | meta-name | dom | itemprop | title
  "confidence": 85,               // 0–100
  "span": [2378, 2470],           // byte range in the source document
  "transforms": ["EntityDecodedByParser"],
  "verbatim_ok": true
}
```

`title`, `title_without_site_name`, `site_name`, `language`, `description`, `canonical_url` are
candidates or `null`. `authors` is an array of them. `published` and `modified` wrap one:

```jsonc
"published": {
  "candidate": { … },
  "raw": "2026-08-05T23:36:25+09:00",
  "iso8601": "2026-08-05T23:36:25+09:00",
  "tz_known": true                // false when the source carried no offset
}
```

**`verbatim_ok` is an executable guarantee, not a hint.** When `transforms` is exactly
`{WS_NORMALIZED}`, `ws_normalize(source[span]) == value` holds for the whole corpus and under fuzz,
with no exceptions and no allowlist (plan §1.4). `EntityDecodedByParser` is reported for information:
the parser did it, not us, so it is not part of the equality claim.

**`tz_known: false` means the document did not say.** The instant is a local wall clock. Do not
convert it to UTC and present the result as certain; that is the bug this field exists to prevent.
Relative times (`3 hours ago`) resolve only against a host-supplied reference instant, because the
core has no clock.

`title_without_site_name` is populated only when the suffix was confirmed against `og:site_name`. It
is `null` far more often than not, and that is deliberate — guessing costs more than it saves.

## `page_kind` and `diagnostics`

`page_kind: "listing"` is an index or front page: its text is dominated by a repeated template whose
items have no author or timestamp. Paired with `no_article: "IndexPage"`. **Not a failure** — it is
the answer.

```jsonc
"diagnostics": {
  "node_count": 2520,
  "page_prose_len": 25518, "page_control_len": 1380,
  "page_hidden_len": 4403, "page_alt_len": 0, "page_inert_len": 43217,
  "discussion_shape": "with-body" | "link-only" | null,
  "limits_hit": []                // e.g. ["input_bytes", "nodes", "attr_bytes"]
}
```

The five length columns are where the page's text went, by role. Only `prose` reaches any statistic —
that is what keeps icon ligatures, "copy code" labels and `hidden` blocks out of the scoring, and it
is the thing Readability counts and we do not.

`limits_hit` being non-empty means you got a **valid but smaller** result. There is no error path
from a limit (guarantee S2); every one degrades and says so.

## Stability

`schema_version` is `1`. Fields may be **added** within version 1; existing keys keep their names,
positions and meanings. `diagnostics` is the exception — treat it as observational, not as an API.

`no_article`, `article.url`, and every metadata field can be `null`. Code that assumes otherwise will
meet a page that disagrees.
