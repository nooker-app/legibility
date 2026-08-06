//! Accessibility semantics used as extraction signals (plan §1.10).
//!
//! Readability handles this with an `unlikelyCandidates` regex and some style cleaning, which
//! means Material Icons' `content_copy`, Font Awesome's private-use glyphs, "Skip to content"
//! and every "copy code" button label land in the text length that scoring depends on.
//!
//! The fix is an ordering fix, not a filter. Classification happens **during the parse**, and
//! the arena keeps four separate length columns, so polluted text never enters a statistic in
//! the first place. Removing it at serialization time — which is where a cleaning pass
//! naturally sits — is too late: the region has already been chosen.

/// Role of a single text node's bytes.
///
/// Exactly one applies. `Hidden` is inherited by a whole subtree; the others are per-node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum TextRole {
    /// Real content. The only role counted in `prose_len`, and therefore the only role any
    /// feature or z-score sees.
    #[default]
    Prose = 0,
    /// Control and icon text: `<button>`, `role=button`, icon-font ligatures, live regions.
    /// Excluded from statistics and from output text; the element is dropped from output HTML
    /// by default.
    Control = 1,
    /// Not in the accessibility tree, or not rendered at all. Excluded from everything and
    /// removed from output.
    Hidden = 2,
    /// Screen-reader-only text (`sr-only`, `visually-hidden`, …).
    ///
    /// Deliberately **not** deleted. For assistive technology this is legitimate content;
    /// suppressing it silently while citing accessibility as the justification would be
    /// self-contradictory. It is excluded from the default reader output and from statistics,
    /// preserved in output HTML behind a `data-lg-sr-only` marker, and recoverable with
    /// `include_sr_only`.
    AltOnly = 3,
    /// Bytes that are not text at all: `<script>` and `<style>` source, `<template>` contents,
    /// comments, doctype, processing instructions, `<head>`.
    ///
    /// Counted in **no** length column, which is the difference between this and [`Hidden`].
    ///
    /// These used to be `Hidden`, and that was a real bug rather than a naming quibble. `purity`
    /// is `prose / (prose + control + hidden + alt)`, so a container holding 10 KB of minified
    /// JavaScript and 450 bytes of article scored 0.04 and was thrown out by the purity floor.
    /// Modern pages inline their bundles: Reddit puts a ~10 KB module script inside `<main>`,
    /// which made `<main>` — the correct answer — unselectable. The intent of `purity` was to
    /// catch regions padded with *visible-ish* noise (button labels, `display:none` promo copy),
    /// and script source is neither. Invisible markup overhead must not be evidence about
    /// anything.
    ///
    /// [`Hidden`]: TextRole::Hidden
    Inert = 4,
}

impl TextRole {
    /// Whether this role contributes to `prose_len` and thus to every feature.
    #[must_use]
    pub const fn is_prose(self) -> bool {
        matches!(self, TextRole::Prose)
    }

    /// Strength of this role's claim during inheritance. Higher wins.
    ///
    /// `Hidden` outranks `Control` and `AltOnly` because it is a statement about rendering, not
    /// about the kind of text: a prose paragraph inside an `aria-hidden` subtree is still not
    /// rendered. `Inert` outranks even that — bytes inside a `<script>` are not text, so no claim
    /// about what kind of text they are can apply to them.
    const fn precedence(self) -> u8 {
        match self {
            TextRole::Prose => 0,
            TextRole::AltOnly => 1,
            TextRole::Control => 2,
            TextRole::Hidden => 3,
            TextRole::Inert => 4,
        }
    }

    /// Combine an inherited role with a node's own classification.
    ///
    /// **Every non-prose role inherits**, not only `Hidden`. This is required by the motivating
    /// case rather than being a tidiness refinement: control text is almost never a direct child
    /// of the control.
    ///
    /// ```text
    /// <button class="copy"><span class="icon">content_copy</span><span>Copy</span></button>
    /// ```
    ///
    /// Both text nodes sit two levels below the `<button>`. If `Control` stopped at the element
    /// that declared it, "Copy" would count as prose and the copy-code-button case would not
    /// actually be fixed. The same argument applies to a `sr-only` wrapper whose text sits in a
    /// nested `<span>`.
    #[must_use]
    pub const fn inherit(parent: TextRole, own: TextRole) -> TextRole {
        if parent.precedence() >= own.precedence() {
            parent
        } else {
            own
        }
    }
}

/// Signals that mark a subtree [`TextRole::Hidden`].
///
/// Kept as an explicit enum rather than a bool so diagnostics can say *why* something was
/// excluded, and so the recovery pass in §1.10.5 can revert only the untrustworthy reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HiddenReason {
    /// The `hidden` attribute — but see [`is_hidden_attr_value`]: `until-found` does not count.
    HiddenAttr,
    /// `aria-hidden="true"`. Frequently misapplied by sites, hence revertible.
    AriaHidden,
    /// The `inert` attribute.
    Inert,
    /// Inside `<template>`.
    Template,
    /// A `<dialog>` without `open`.
    ClosedDialog,
    /// `<script>`, `<style>`, `<noscript>`, or `<head>` text.
    NonRendered,
    /// An inline `style` declaring the node invisible. Also revertible.
    InlineStyle,
    /// Text made entirely of private-use codepoints, i.e. icon-font glyphs.
    PrivateUseGlyph,
}

impl HiddenReason {
    /// Whether the a11y recovery pass may undo this reason.
    ///
    /// Only site-authored assertions are revertible. `aria-hidden` and inline styles are
    /// regularly wrong — sites put `aria-hidden="true"` on article containers — so if applying
    /// them removes almost all prose, they are undone and reported. `<script>` text and
    /// `<template>` contents are not opinions, so they are never undone.
    #[must_use]
    pub const fn is_revertible(self) -> bool {
        matches!(self, HiddenReason::AriaHidden | HiddenReason::InlineStyle)
    }
}

/// Whether a `hidden` attribute value actually hides content.
///
/// `hidden="until-found"` does **not**: the content participates in find-in-page and is
/// revealed when matched, so it is ordinary collapsed content. Treating it as hidden silently
/// deletes real article text, and it is an easy mistake because every other value hides.
#[must_use]
pub fn is_hidden_attr_value(value: &str) -> bool {
    !value.eq_ignore_ascii_case("until-found")
}

/// Whether a string consists solely of private-use codepoints.
///
/// This is a cheap, high-precision icon detector: Font Awesome and similar ship glyphs at
/// `U+F000`-ish codepoints, and no natural-language content is written in the private use
/// areas. Text mixing PUA with real characters is left alone — only wholly-PUA nodes are icons.
#[must_use]
pub fn is_private_use_only(s: &str) -> bool {
    let mut saw_any = false;
    for c in s.chars() {
        if c.is_whitespace() {
            continue;
        }
        saw_any = true;
        let cp = c as u32;
        let in_pua = (0xE000..=0xF8FF).contains(&cp)
            || (0xF_0000..=0xF_FFFD).contains(&cp)
            || (0x10_0000..=0x10_FFFD).contains(&cp);
        if !in_pua {
            return false;
        }
    }
    saw_any
}

/// Whether an inline `style` value declares the element invisible.
///
/// Substring matching on a normalized copy, not CSS parsing. This only ever sees the `style`
/// attribute; **external stylesheets are out of scope for v1** and that limitation is recorded
/// in `docs/limits.md` rather than hidden, since it means some visually-hidden nodes are
/// missed. The `sr-only` class lexicon covers the common cases that CSS would have caught.
#[must_use]
pub fn inline_style_hides(style: &str) -> bool {
    const NEEDLES: [&str; 7] = [
        "display:none",
        "visibility:hidden",
        "opacity:0",
        "font-size:0",
        "clip:rect(0,0,0,0)",
        "clip:rect(1px,1px,1px,1px)",
        "clip-path:inset(100%)",
    ];

    let mut norm = alloc::string::String::with_capacity(style.len());
    for c in style.chars() {
        if !c.is_whitespace() {
            norm.push(c.to_ascii_lowercase());
        }
    }
    if NEEDLES.iter().any(|n| norm.contains(n)) {
        return true;
    }
    // The 1x1-plus-overflow-hidden trick, which no single declaration reveals.
    (norm.contains("width:1px") || norm.contains("width:0"))
        && (norm.contains("height:1px") || norm.contains("height:0"))
        && norm.contains("overflow:hidden")
}

#[cfg(test)]
// Tests may unwrap and may assert on const-evaluable predicates: these are regression
// guards against someone changing a const fn, not runtime checks.
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::assertions_on_constants)]
mod tests {
    use super::*;

    #[test]
    fn until_found_is_not_hidden() {
        // The whole point: every other value hides, this one does not.
        assert!(is_hidden_attr_value(""));
        assert!(is_hidden_attr_value("hidden"));
        assert!(!is_hidden_attr_value("until-found"));
        assert!(!is_hidden_attr_value("UNTIL-FOUND"));
    }

    #[test]
    fn private_use_detection_is_precise() {
        assert!(is_private_use_only("\u{f0c5}")); // Font Awesome copy glyph
        assert!(is_private_use_only(" \u{e800}\u{e801} "));
        assert!(!is_private_use_only("")); // nothing is not an icon
        assert!(!is_private_use_only("   "));
        assert!(!is_private_use_only("content_copy")); // ligature text, caught elsewhere
        assert!(!is_private_use_only("\u{f0c5} Copy")); // mixed: leave it alone
        assert!(!is_private_use_only("본문")); // Korean prose must never be an icon
    }

    #[test]
    fn inline_style_detection_covers_the_1px_trick() {
        assert!(inline_style_hides("display:none"));
        assert!(inline_style_hides("DISPLAY : NONE"));
        assert!(inline_style_hides("visibility:hidden"));
        assert!(inline_style_hides("clip-path: inset(100%)"));
        assert!(inline_style_hides("width:1px;height:1px;overflow:hidden"));
        assert!(!inline_style_hides("display:block"));
        assert!(!inline_style_hides("width:1px"));
        assert!(!inline_style_hides("color:red;font-weight:700"));
    }

    #[test]
    fn every_non_prose_role_inherits() {
        // The copy-code-button case: <button><span>Copy</span></button>. If Control did not
        // inherit, "Copy" would be scored as prose and the whole point would be missed.
        assert_eq!(TextRole::inherit(TextRole::Control, TextRole::Prose), TextRole::Control);
        assert_eq!(TextRole::inherit(TextRole::AltOnly, TextRole::Prose), TextRole::AltOnly);
        assert_eq!(TextRole::inherit(TextRole::Hidden, TextRole::Prose), TextRole::Hidden);
    }

    #[test]
    fn hidden_outranks_every_other_role() {
        // Rendering beats kind-of-text: prose inside an aria-hidden subtree is still not shown.
        for own in [TextRole::Prose, TextRole::Control, TextRole::AltOnly, TextRole::Hidden] {
            assert_eq!(TextRole::inherit(TextRole::Hidden, own), TextRole::Hidden);
        }
        // ... and a Hidden child inside a Control parent is still Hidden.
        assert_eq!(TextRole::inherit(TextRole::Control, TextRole::Hidden), TextRole::Hidden);
    }

    #[test]
    fn a_more_specific_child_role_still_wins_over_prose_parent() {
        assert_eq!(TextRole::inherit(TextRole::Prose, TextRole::Control), TextRole::Control);
        assert_eq!(TextRole::inherit(TextRole::Prose, TextRole::AltOnly), TextRole::AltOnly);
        assert_eq!(TextRole::inherit(TextRole::Prose, TextRole::Prose), TextRole::Prose);
    }

    #[test]
    fn inherit_is_idempotent_and_associative_enough_to_fold_down_a_tree() {
        // flatten() folds this pairwise down each root-to-leaf path, so a inherit b inherit c
        // must not depend on grouping.
        let roles = [
            TextRole::Prose,
            TextRole::AltOnly,
            TextRole::Control,
            TextRole::Hidden,
            TextRole::Inert,
        ];
        for a in roles {
            assert_eq!(TextRole::inherit(a, a), a, "not idempotent for {a:?}");
            for b in roles {
                for c in roles {
                    let left = TextRole::inherit(TextRole::inherit(a, b), c);
                    let right = TextRole::inherit(a, TextRole::inherit(b, c));
                    assert_eq!(left, right, "grouping matters for {a:?},{b:?},{c:?}");
                }
            }
        }
    }

    #[test]
    fn only_site_opinions_are_revertible() {
        assert!(HiddenReason::AriaHidden.is_revertible());
        assert!(HiddenReason::InlineStyle.is_revertible());
        // These are facts about rendering, not claims a site can get wrong.
        assert!(!HiddenReason::Template.is_revertible());
        assert!(!HiddenReason::NonRendered.is_revertible());
        assert!(!HiddenReason::HiddenAttr.is_revertible());
    }

    #[test]
    fn prose_is_the_only_counted_role() {
        assert!(TextRole::Prose.is_prose());
        for r in [TextRole::Control, TextRole::Hidden, TextRole::AltOnly, TextRole::Inert] {
            assert!(!r.is_prose(), "{r:?} must not count toward prose_len");
        }
    }

    #[test]
    fn inert_outranks_hidden_so_script_text_is_never_merely_hidden() {
        // This ordering is what keeps `<script>` bytes out of the purity denominator. If Hidden
        // won here, a page that inlines its JS bundle inside <main> would disqualify <main>.
        assert_eq!(TextRole::inherit(TextRole::Hidden, TextRole::Inert), TextRole::Inert);
        assert_eq!(TextRole::inherit(TextRole::Inert, TextRole::Hidden), TextRole::Inert);
        // And a script inside a visually-hidden wrapper is still inert, not hidden prose.
        for own in [TextRole::Prose, TextRole::Control, TextRole::AltOnly, TextRole::Hidden] {
            assert_eq!(TextRole::inherit(TextRole::Inert, own), TextRole::Inert);
        }
    }
}
