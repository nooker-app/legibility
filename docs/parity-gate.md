# What the corpus score measures, and what it cannot

The engine scores **0.9144** mean token-F1 over mozilla/readability's 130 test pages. This document
is about what that number is, because it is not what the plan assumed when it designed the gate
around it.

## `expected.html` is Readability's own output

Measured, not inferred. `corpus/readability/test/generate-testcase.js:149` builds every
`expected.html` by running Readability over `source.html`:

```js
var uri = "http://fakehost/test/page.html";
var doc = new JSDOMParser().parse(source, uri);
result = new Readability(doc, { classesToPreserve: ["caption"] }).parse();
fs.writeFile(destPath, prettyPrint(result.content), ...);
```

`tools/rjs-baseline/run.js` reproduces that pipeline exactly — same fake base URI, same
`classesToPreserve`, and `JSDOMParser` rather than jsdom, which is what they parse with. Running it
over all 130 pages and comparing whitespace-collapsed text:

| | |
|---|---|
| byte-identical to `expected.html` | **129 / 130** |
| differing | 1 (`wikipedia-2`, by 8 characters — an entity the comparison script decodes and `prettyPrint` does not) |

## Three consequences

**1. Readability scores 1.000 on this corpus by construction.** It is being compared against its own
output. There is no version of this measurement in which it does worse.

**2. `LOSSES` does not mean what plan §2.2 says.** The plan makes
`LOSSES = count(our_f1 < rjs_f1 - 0.05)` the single blocking anchor, on the reasoning that R.js's
output is an external artifact we cannot edit in our favour. That reasoning holds. The arithmetic
does not: with `rjs_f1 = 1.000` everywhere, `LOSSES` is exactly *"pages scoring below 0.95"* — a
threshold on our own number wearing a comparison's clothes. It is not wrong, but it is not evidence
about Readability, and building the R.js baseline does not make it so.

**3. The corpus mean already is the agreement number.** 0.9144 means *"91.4% token agreement with
Readability.js"*, and the 13 pages under 0.80 are pages where the two engines **disagree**. That is
the honest reading. Which engine is right on those pages is a question this corpus cannot answer,
because it has no ground truth independent of one of the answers.

## Where a real comparison is available

`crates/legibility-dom/tests/snapshots/` — thirteen self-authored fixtures, one per page shape that
has been diagnosed by hand. Their correct extraction is stated by us, in the fixture's own doc
comment and its assertions, and it is neither engine's output. That makes them the only pages in
this repo where "better" is a question with an answer.

```sh
cargo test -p legibility-dom --test snapshot -- --ignored dump_fixtures
node tools/rjs-baseline/run.js --out rjs.json /path/printed/above/*.html
```

Measured 2026-08-18, `article.text` length in characters:

| fixture | ours | R.js | what the difference is |
|---|---:|---:|---|
| `beebs-eighteen-comments` | 112 | 1910 | **R.js puts all 18 comments in the body.** We return them as `comments[]` |
| `beebs-two-comments` | 112 | 373 | same, both comments |
| `news-article-with-dateline` | 409 | 389 | we keep the `<figcaption>`; R.js drops it |
| `reddit-credit-bar-without-comments` | 304 → 239 | 239 | **R.js was right and we were not** — see below |
| `reddit-link-submission` | 0 | 127 | we return `kind: "discussion-root"` with the link in `article.url`; R.js returns the credit bar and the pasted URL as prose |
| the other 8 | — | — | identical |

The comment-swallowing on the two `beebs` fixtures is the defect this project was started over,
reported against `news.hada.io` and reproduced here at both thread sizes. It is Readability's, and
it is measured rather than asserted.

The Reddit row went the other way and is now fixed. That page has no `<head>`, so its `<h1>` is the
only thing that ever declared a title — `metadata.title.source` was `h1` — and we then left the same
`<h1>` at the top of `article.html`, so a reader view printed the headline twice. R.js drops it.
`pipeline::headline_that_is_the_title` closes it by node identity rather than text similarity, and
`a_headline_that_is_the_only_title_source_is_not_also_left_in_the_body` guards both directions: the
heading goes when it is the sole title source, and stays when a `<title>` disagrees with it.

## What the disagreements turned out to be

Thirteen agents diagnosed the thirteen pages then below 0.80, one page each, with `lgb explain` and
control experiments. That is the `INFRA` / `PARSER` / `ARITH` decomposition the `legibility-legacy`
port was scoped to produce, obtained by inspection:

| cause | pages |
|---|---:|
| SCORING — both trees fine, we picked a different region | 10 |
| EXCLUSION — same region, different pruning | 1 |
| CORRECT_DIFFERENCE — we differ and ours is better | 2 |
| **INFRA** | **0** |
| **PARSER** | **0** |

The empty rows are the result, and they rest on *positive* controls rather than absence of
evidence: on ten of the thirteen pages the diagnosis names a node in our own arena whose serialized
text scores 0.92–1.00 against `expected.html`. The arena holds the region Readability chose and the
serializer emits it faithfully. What was wrong is the arithmetic that picks which node.

Five fixes followed, each measured on its own: `property="articleBody"` alongside `itemprop`, a
`related` furniture token, `javascript:` anchors around block content no longer counting as links,
a `visible_purity` that keeps hidden bytes out of the viability floor while leaving them in the
ranking, and a `<body>` last-resort gate that fires on prose *coverage* rather than only on an
empty candidate pool. **Corpus mean 0.9144 → 0.9456; pages below 0.80: 13 → 7.**

Two of the thirteen are permanent and should not be recovered. `nytimes-5` (0.5009) is a near-strict
superset of `expected.html` at recall 0.993, because Readability's `cleanConditionally` deleted the
headlines from 16 of the 22 story cards it kept. `hukumusume` (0.5410) omits a nav rail that
Readability's sibling pass swept in. Together they cost 0.0074 of corpus mean, and both proposed
"fixes" are changes the diagnoses themselves argue against.

## What would make the claim provable

Not the R.js baseline, which is now built and says 1.000. **Hand-labelled ground truth** — plan §2.4's
40 hard cases with our own `expected` labels, where both engines are scored against something
neither produced. Until those exist, the defensible claims are:

- 91.4% token agreement with Readability over its own corpus;
- on 13 self-authored shape fixtures, one measured difference in Readability's favour (fixed) and
  three in ours.

Anything stronger is not currently supported by a measurement.

## Is the `legibility-legacy` port still warranted?

**Not as a diagnostic.** Its purpose was the decomposition above, applied to the pages under 0.80,
and that is now done — by a stronger argument than the port makes. The port's INFRA test is negative
("the port disagrees too, so something underneath is broken"); the controls are positive ("node N in
our arena serializes to `expected.html` at F1 0.92–1.00"). Building 3–4k LOC to relabel those pages
`SCORING` would buy an answer already in hand.

What it would still be good for, stated exactly, because it is not nothing:

1. **An oracle for the one mechanism we lack.** The remaining pages need region *growth* —
   Readability's `parentOfTopCandidate` climb and its sibling-append pass — and the risk is that
   widening a region is how index pages and comment threads become article bodies (`groups.rs`
   records `mozilla-2` going 0.991 → 0.000 that way). A port of `grabArticle`'s sibling pass and
   `_cleanConditionally` **only** would print, for all 130 pages, which node the climb reaches and
   what it prunes — turning "sweep an unwritten rule" into a measurement against a reference. That
   is a few hundred lines scoped to two functions, not a port of `parse()`.
2. **Amortising the next thirteen diagnoses.** These cost hours of hand work each and do not
   generalise. That is a standing-infrastructure argument, not a this-week one.

## Bugs 3 and 1: measured, not fixed

Two of the remaining sub-0.80 pages resisted five measured attempts on 2026-08-19. Recording what
was tried, because the next person to look will otherwise try the same things.

**`yahoo-3` (0.3798) — a related-stories rail.** Twenty-odd `<li class="js-stream-content">` items,
each a headline link and a ~300-byte summary of another story. The triage classed this `EXCLUSION`
("same region, different pruning"); it is not. The rail is **58% of the region the scorer picks**,
and the correct region — `div.body yom-art-content`, purity 1.000, density 85 — is a candidate at
rank nine. It is a selection problem.

| attempt | result |
|---|---|
| prune listing groups from the region's output | **−0.004 mean**: Wikipedia's reference and see-also lists are `<li>` runs leading with links and part of the article. `wikipedia-2` 0.950 → 0.772, `wikipedia-4` 0.980 → 0.828, `blogger` 0.999 → 0.898. `yahoo-3` unchanged. |
| ... and require the container to be *named* a rail (`stream`, `related`, …) | fires on **nothing**. Yahoo's container is `class="Bleed Mt-neg-10 Mb-0 ResetChildren Wow-b"` — atomic CSS names no intent. |
| mask listing prose *before* scoring, as comments already are | mask is correct (8397 bytes, the rail included) and selection **does not move**. Removing the rail leaves the region still holding video-player chrome and a comment form, ~2750 bytes more than the real body. |

What it actually needs: the video player's labels (`Closed Caption`, and a menu of quality and font
settings) are `TextRole::Prose` today and are controls. Plan §1.10.3 already specifies the rule —
text under an interactive ARIA role is `Control` — and the roles are implemented; these particular
elements simply do not carry them. That is a11y work (plan M5's tail), not scoring work.

**`bug-1255978` (0.0000) — a fifty-slide gallery.** Its caption list is 65% of the page's prose, over
the dominance share, so the page is refused as an index. Three distinct blockers, each found by
measurement:

1. The argmax is the gallery itself, so `region_from_semantic_anchor` is unset and the containment
   rule added for `news.hada.io/topic?id=32685` cannot disarm the veto.
2. The captions carry no links, so a rail rule keyed on items pointing elsewhere excludes them.
3. `mask_by_identity` keys on `(tag, class_bits)` and requires a non-empty class. The gallery's
   `<li data-gallery-legend="N">` have no class at all, so they can never be masked by identity.

Blocker 3 is the tractable one and is not specific to this page: a repeated group whose members
carry no class is invisible to masking altogether.

**Neither page was left in a worse state, and nothing was shipped for either.** Every attempt above
was reverted; the corpus is at 0.9498 with six pages under 0.80. The reason to write this down is
that all five attempts were plausible, three of them were the triage's own recommendations, and each
cost a measurement to reject.
