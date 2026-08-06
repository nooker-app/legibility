# Known limits of v1

Recorded here rather than left implicit. Each entry is a deliberate scope decision with a
named consequence, not an oversight.

## External CSS is not parsed

The a11y classifier (plan §1.10) sees the `style` **attribute** only. A node hidden solely by
an external stylesheet — `.promo { display: none }` — is not detected as `Hidden`, so its text
still counts toward `prose_len`.

Mitigations in place: the `sr-only` / `visually-hidden` / `screen-reader-text` class lexicon
covers the common cases a stylesheet would have revealed, and `aria-hidden` / `hidden` /
`inert` / `<template>` cover the rest of the usual patterns.

Consequence: some visually-hidden text is scored as prose. Corpus cases exercising this are
named explicitly so the gap is measured rather than assumed away. Parsing external CSS is a
v1.1 candidate and will only be taken up if a measured quality gap tracks to it.

## `continuation` URLs are not followed

When a discussion thread is truncated (Reddit's `load more`, Discourse's lazy scroll, HN's
`More` link) the engine reports `completeness.continuation` but does not fetch it. The core has
no network access at all, which is what makes guarantee S3 (byte-identical output for identical
input) structural rather than aspirational.

## No timezone is ever assumed

A date with no offset in the source yields `tz_known: false` and a local wall-clock value. It is
not shifted to UTC. Date-only values are not expanded to midnight UTC. Relative times
("3 hours ago") resolve only against a caller-supplied `reference_instant`.

## `confidence` is not calibrated

`Article::calibrated` is `false`, permanently in v1. Calibrating requires a labelled corpus
large enough to fit a mapping; claiming calibration without one would be worse than admitting
its absence. Treat `confidence` as an ordering signal, not a probability.

## SVG and MathML are unsupported

Both are dropped by the sanitizer. The structural metric measures what this costs; v1 does not
reverse it.
