//! Output sanitizer. Owned by the engine, never delegated to the consumer.
//!
//! The engine returns HTML that gets injected into a page, so an XSS here is worse than any
//! extraction bug. Leaving sanitization to callers means every caller re-implements it and one
//! of them gets it wrong.
//!
//! # Two profiles, separated by a type
//!
//! [`Article`] and [`UserContent`] are not the same policy with a flag. Comment HTML is
//! **attacker-controlled input** — a site's own sanitizer may have failed, and archived
//! snapshots preserve whatever was there at the time. So `UserContent` is strictly tighter:
//! images off by default, no media, `rel="nofollow noopener noreferrer"` forced on links,
//! `<details open>` neutralized so spoilers stay closed.
//!
//! The profile is a type parameter on [`SanitizedHtml`], so rendering comment HTML through an
//! article-profile path is a compile error rather than a code review finding.
//!
//! # What is deliberately unsupported
//!
//! `style` attributes, `SVG` and `MathML` are dropped. `SVG` in particular is a well-known mXSS
//! vector (`<svg><style>` and foreign-content parsing differences), and supporting it safely
//! costs more than it returns for a reader view.

#![forbid(unsafe_code)]
#![deny(clippy::all)]
#![warn(clippy::pedantic, missing_docs)]
#![allow(clippy::module_name_repetitions)]

use core::marker::PhantomData;

/// Sanitization profile.
pub trait Profile: private::Sealed {
    /// Human-readable name, used in diagnostics.
    const NAME: &'static str;
    /// Whether `<img>` survives by default.
    const IMAGES: bool;
    /// Whether `<video>` / `<audio>` survive.
    const MEDIA: bool;
    /// Value forced onto every `rel` attribute on `<a>`, if any.
    const FORCED_REL: Option<&'static str>;
    /// Prefix for rewritten `id` attributes.
    const ID_PREFIX: &'static str;
}

mod private {
    pub trait Sealed {}
    impl Sealed for super::Article {}
    impl Sealed for super::UserContent {}
}

/// Article body: the site's own content.
#[derive(Debug, Clone, Copy)]
pub struct Article;

/// Comments, quotes, and anything else a third party authored.
#[derive(Debug, Clone, Copy)]
pub struct UserContent;

impl Profile for Article {
    const NAME: &'static str = "article";
    const IMAGES: bool = true;
    const MEDIA: bool = true;
    const FORCED_REL: Option<&'static str> = None;
    const ID_PREFIX: &'static str = "lg-";
}

impl Profile for UserContent {
    const NAME: &'static str = "user-content";
    // Off by default: a comment image is a tracking pixel or a shock image far more often than
    // it is content, and turning it on is one option flag away.
    const IMAGES: bool = false;
    const MEDIA: bool = false;
    const FORCED_REL: Option<&'static str> = Some("nofollow noopener noreferrer");
    const ID_PREFIX: &'static str = "lg-c-";
}

/// HTML that has passed a specific profile.
///
/// The `PhantomData` is the point: `SanitizedHtml<UserContent>` cannot be passed where
/// `SanitizedHtml<Article>` is expected, so the two can never be mixed by accident.
#[derive(Debug, Clone)]
pub struct SanitizedHtml<P: Profile> {
    html: String,
    _profile: PhantomData<P>,
}

impl<P: Profile> SanitizedHtml<P> {
    /// The sanitized markup.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.html
    }

    /// Consume into the underlying `String`.
    #[must_use]
    pub fn into_inner(self) -> String {
        self.html
    }

    /// Which profile produced this.
    #[must_use]
    pub fn profile_name() -> &'static str {
        P::NAME
    }
}

/// Elements permitted in output. Everything not listed is unwrapped (children kept) or dropped.
const ALLOWED: &[&str] = &[
    "a",
    "abbr",
    "article",
    "aside",
    "b",
    "blockquote",
    "br",
    "caption",
    "cite",
    "code",
    "col",
    "colgroup",
    "dd",
    "del",
    "details",
    "dfn",
    "div",
    "dl",
    "dt",
    "em",
    "figcaption",
    "figure",
    // `img` was missing here until a test asked whether an image-only `<figure>` survived
    // serialization. It did not -- and no article had ever carried an image -- while
    // `element_attrs("img")` and the `P::IMAGES` gate below sat as unreachable code stating the
    // opposite intent. Absence is invisible in a text-token F1 score, which is why the corpus
    // ratchet never noticed.
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "i",
    "img",
    "ins",
    "kbd",
    "li",
    "main",
    "mark",
    "ol",
    "p",
    "pre",
    "q",
    "s",
    "samp",
    "section",
    "small",
    "span",
    "strong",
    "sub",
    "summary",
    "sup",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "time",
    "tr",
    "u",
    "ul",
    "var",
    "wbr",
];

/// Elements whose entire subtree is discarded, text included.
///
/// Distinct from "not allowed": an unknown `<foo>` should keep its text, but a `<script>`'s
/// contents are code.
///
/// # Why `<form>` is not here
///
/// It was, on the reasoning that a form's contents are controls. Sites use `<form>` as a *layout
/// wrapper* around prose, and old.reddit.com is the case that proves it: both a self-post's text and
/// every comment's text live inside
///
/// ```text
///   <form class="usertext" action="#" onsubmit="return false;">
///     <div class="usertext-body"><div class="md"><p>…the actual text…</p></div></div>
///   </form>
/// ```
///
/// so dropping the subtree emptied every comment body on the site. A `<form>` is dangerous because
/// it *submits*, and unwrapping it removes that completely — the element vanishes, its text stays,
/// and the controls inside it are still dropped individually by the entries below. Nothing is gained
/// by taking the prose with them.
const DROP_SUBTREE: &[&str] = &[
    "script", "style", "noscript", "template", "iframe", "embed", "object", "applet", "input",
    "button", "select", "textarea", "option", "optgroup", "label", "fieldset", "legend", "svg",
    "math", "canvas", "map", "area", "audio", "video", "source", "track", "dialog", "meta", "link",
    "base", "title", "head",
];

/// Attributes allowed on any element.
const GLOBAL_ATTRS: &[&str] = &["dir", "lang", "title"];

/// Per-element attribute allowlist.
fn element_attrs(tag: &str) -> &'static [&'static str] {
    match tag {
        "a" => &["href", "rel"],
        "img" => &["src", "alt", "width", "height"],
        "time" => &["datetime"],
        "td" | "th" => &["colspan", "rowspan", "scope", "headers"],
        "col" | "colgroup" => &["span"],
        "ol" => &["start", "reversed", "type"],
        "li" => &["value"],
        "blockquote" | "q" => &["cite"],
        "del" | "ins" => &["cite", "datetime"],
        "pre" | "code" => &["class"],
        _ => &[],
    }
}

/// URL schemes permitted in `href` / `src`.
const ALLOWED_SCHEMES: &[&str] = &["http", "https", "mailto", "tel", "ftp"];

/// Outcome of validating a URL-valued attribute.
///
/// Both fields are kept. `url` is `None` when the scheme was rejected, but `raw` survives so a
/// consumer can still show what the page claimed — satisfying verbatim reporting and scheme
/// denial at the same time, rather than trading one for the other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlField {
    /// The URL, if its scheme is allowed.
    pub url: Option<String>,
    /// Exactly what the document contained.
    pub raw: String,
    /// Why it was rejected, if it was.
    pub reject_reason: Option<&'static str>,
}

/// Validate a URL-valued attribute.
///
/// Control characters are stripped **before** the scheme is extracted. This order is the whole
/// defence: `java\tscript:alert(1)` and `java\0script:` both parse as `javascript:` in browsers,
/// so extracting the scheme first and stripping afterwards lets them through.
#[must_use]
pub fn check_url(raw: &str) -> UrlField {
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_control() && *c != '\u{feff}' && !matches!(c, '\u{200b}'..='\u{200f}'))
        .collect();
    let trimmed = cleaned.trim();

    // Relative and fragment URLs carry no scheme and are safe by construction.
    let scheme_end = trimmed.find(':');
    let Some(idx) = scheme_end else {
        return UrlField {
            url: Some(trimmed.to_string()),
            raw: raw.to_string(),
            reject_reason: None,
        };
    };
    // A ':' after a '/' or '?' is part of a path or query, not a scheme.
    let before = trimmed.get(..idx).unwrap_or("");
    if before.contains('/') || before.contains('?') || before.contains('#') {
        return UrlField {
            url: Some(trimmed.to_string()),
            raw: raw.to_string(),
            reject_reason: None,
        };
    }
    let scheme = before.to_ascii_lowercase();
    if ALLOWED_SCHEMES.contains(&scheme.as_str()) {
        UrlField { url: Some(trimmed.to_string()), raw: raw.to_string(), reject_reason: None }
    } else {
        UrlField { url: None, raw: raw.to_string(), reject_reason: Some("scheme not in allowlist") }
    }
}

/// Whether `tag`'s entire subtree must be discarded.
#[must_use]
pub fn drops_subtree(tag: &str) -> bool {
    DROP_SUBTREE.contains(&tag)
}

/// [`drops_subtree`], plus the elements a *profile* refuses.
///
/// `UserContent` turns images off (plan §1.8), and that was implemented by refusing every attribute
/// on `<img>` — which emitted `<img>` with no `src` and no `alt`: a broken-image icon in the reader,
/// and the alt text, the one part that carries meaning, gone. Refusing the element is what "images
/// off" was supposed to mean.
#[must_use]
pub fn drops_subtree_for<P: Profile>(tag: &str) -> bool {
    drops_subtree(tag) || (!P::IMAGES && matches!(tag, "img" | "picture" | "source"))
}

/// Whether `tag` may appear in output.
#[must_use]
pub fn is_allowed_element(tag: &str) -> bool {
    ALLOWED.contains(&tag)
}

/// Whether `attr` may appear on `tag` under profile `P`.
#[must_use]
pub fn is_allowed_attr<P: Profile>(tag: &str, attr: &str) -> bool {
    let lower = attr.to_ascii_lowercase();
    // Event handlers are rejected before any allowlist consultation. `on*` is checked by prefix
    // rather than by name so a novel handler attribute cannot slip through.
    if lower.starts_with("on") {
        return false;
    }
    if lower == "style" {
        return false;
    }
    if lower == "srcdoc" || lower == "ping" || lower == "formaction" {
        return false;
    }
    if tag == "img" && !P::IMAGES {
        return false;
    }
    GLOBAL_ATTRS.contains(&lower.as_str()) || element_attrs(tag).contains(&lower.as_str())
}

/// Whether a `class` value survives on `<pre>` / `<code>`.
///
/// Only `language-*` tokens, so syntax highlighting keeps working without becoming a channel for
/// arbitrary class names that a consumer's CSS might key on.
#[must_use]
pub fn filter_code_class(value: &str) -> Option<String> {
    let kept: Vec<&str> = value
        .split_ascii_whitespace()
        .filter(|t| {
            t.len() <= 29
                && t.starts_with("language-")
                && t.get(9..).is_some_and(|rest| {
                    !rest.is_empty()
                        && rest.chars().all(|c| {
                            c.is_ascii_alphanumeric() || matches!(c, '+' | '#' | '.' | '-')
                        })
                })
        })
        .collect();
    if kept.is_empty() {
        None
    } else {
        Some(kept.join(" "))
    }
}

/// Rewrite an `id` into the profile's namespace.
///
/// DOM clobbering is the reason. An element with `id="attributes"` or `id="children"` shadows
/// those properties on its parent node, so unnamespaced ids from untrusted markup can break a
/// consumer's own DOM traversal. Prefixing keeps permalink anchors working while removing the
/// collision.
#[must_use]
pub fn namespace_id<P: Profile>(id: &str) -> String {
    let safe: String =
        id.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')).collect();
    format!("{}{safe}", P::ID_PREFIX)
}

/// Escape text for an HTML text node.
#[must_use]
pub fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

/// Escape a value for a double-quoted attribute.
#[must_use]
pub fn escape_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Elements that must be emitted self-closing.
#[must_use]
pub fn is_void(tag: &str) -> bool {
    matches!(tag, "br" | "hr" | "img" | "wbr" | "col")
}

/// Wrap already-sanitized markup.
///
/// Only callable from within this crate's serializer contract; the type it returns is the proof
/// that a profile was applied.
#[must_use]
pub fn wrap<P: Profile>(html: String) -> SanitizedHtml<P> {
    SanitizedHtml { html, _profile: PhantomData }
}

#[cfg(test)]
// Assertions on associated consts are regression guards against someone loosening a profile;
// clippy sees them as constant, which is exactly why they are cheap and worth keeping.
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::assertions_on_constants)]
mod tests {
    use super::*;

    #[test]
    fn event_handlers_are_rejected_by_prefix_not_by_name() {
        for attr in ["onclick", "ONCLICK", "onmouseover", "onfocusin", "onsomethingnovel"] {
            assert!(!is_allowed_attr::<Article>("div", attr), "{attr} must be rejected");
            assert!(!is_allowed_attr::<UserContent>("div", attr));
        }
    }

    #[test]
    fn control_characters_are_stripped_before_the_scheme_is_read() {
        // The whole point of the ordering. Browsers parse all of these as `javascript:`.
        for raw in [
            "java\tscript:alert(1)",
            "java\nscript:alert(1)",
            "java\rscript:alert(1)",
            "java\0script:alert(1)",
            "\u{feff}javascript:alert(1)",
            "JaVaScRiPt:alert(1)",
            "  javascript:alert(1)  ",
        ] {
            let f = check_url(raw);
            assert!(f.url.is_none(), "{raw:?} must be rejected, got {:?}", f.url);
            assert_eq!(f.raw, raw, "raw value must be preserved verbatim");
            assert!(f.reject_reason.is_some());
        }
    }

    #[test]
    fn dangerous_schemes_are_rejected_and_safe_ones_pass() {
        for bad in [
            "data:text/html,<script>alert(1)</script>",
            "data:image/svg+xml;base64,PHN2Zz4=",
            "vbscript:msgbox(1)",
            "file:///etc/passwd",
            "blob:https://x/y",
        ] {
            assert!(check_url(bad).url.is_none(), "{bad} must be rejected");
        }
        for good in [
            "https://example.com/a?b=1#c",
            "http://example.com",
            "mailto:a@b.c",
            "tel:+8210",
            "/relative/path",
            "#fragment",
            "?query=1",
            "path/with:colon/after/slash",
        ] {
            assert!(check_url(good).url.is_some(), "{good} must be allowed");
        }
    }

    #[test]
    fn user_content_profile_is_strictly_tighter_than_article() {
        assert!(Article::IMAGES && !UserContent::IMAGES);
        assert!(Article::MEDIA && !UserContent::MEDIA);
        assert!(Article::FORCED_REL.is_none());
        assert_eq!(UserContent::FORCED_REL, Some("nofollow noopener noreferrer"));
        // Nothing UserContent allows may be disallowed by Article.
        for (tag, attr) in [("a", "href"), ("td", "scope"), ("time", "datetime")] {
            if is_allowed_attr::<UserContent>(tag, attr) {
                assert!(
                    is_allowed_attr::<Article>(tag, attr),
                    "UserContent allows {tag}[{attr}] but Article does not — profiles inverted"
                );
            }
        }
        assert!(is_allowed_attr::<Article>("img", "src"));
        assert!(!is_allowed_attr::<UserContent>("img", "src"));
    }

    #[test]
    fn code_class_keeps_only_language_tokens() {
        assert_eq!(filter_code_class("language-rust").as_deref(), Some("language-rust"));
        assert_eq!(
            filter_code_class("hljs language-c++ theme-dark").as_deref(),
            Some("language-c++")
        );
        assert_eq!(filter_code_class("hljs theme-dark"), None);
        assert_eq!(filter_code_class("language-"), None, "empty language is not a language");
        assert_eq!(filter_code_class("language-<script>"), None);
    }

    #[test]
    fn ids_are_namespaced_against_dom_clobbering() {
        // These exact ids shadow real DOM properties on the parent node.
        for hostile in ["attributes", "children", "documentElement", "firstChild", "id"] {
            let out = namespace_id::<UserContent>(hostile);
            assert!(out.starts_with("lg-c-"), "{out} must be namespaced");
            assert_ne!(out, hostile);
        }
        // Namespacing also sanitizes: quotes cannot escape the attribute.
        assert_eq!(namespace_id::<Article>("a\"><script>"), "lg-ascript");
    }

    #[test]
    fn code_and_control_subtrees_are_dropped_but_wrappers_keep_their_text() {
        for t in ["script", "style", "iframe", "svg", "button", "textarea", "template", "dialog"] {
            assert!(drops_subtree(t), "{t} subtree must be dropped");
        }
        // An unknown element should lose its tag but keep its text; dropping the subtree would
        // lose real content on any page using custom elements.
        assert!(!drops_subtree("my-widget"));
        assert!(!is_allowed_element("my-widget"));
        // `<form>` is unwrapped rather than dropped, and it is not a permitted output element
        // either: old.reddit.com wraps every post and comment body in one, so dropping the subtree
        // emptied them. See DROP_SUBTREE's doc comment.
        assert!(!drops_subtree("form"), "a form is a wrapper; its prose must survive");
        assert!(!is_allowed_element("form"), "a form must not reach the output");
    }

    #[test]
    fn style_and_srcdoc_are_never_allowed() {
        for attr in ["style", "srcdoc", "ping", "formaction"] {
            assert!(!is_allowed_attr::<Article>("div", attr));
            assert!(!is_allowed_attr::<Article>("iframe", attr));
        }
    }

    #[test]
    fn escaping_closes_both_text_and_attribute_contexts() {
        assert_eq!(escape_text("<b>&</b>"), "&lt;b&gt;&amp;&lt;/b&gt;");
        assert_eq!(escape_attr("a\"b'c<d"), "a&quot;b&#39;c&lt;d");
        // The classic attribute break-out must not survive.
        assert!(!escape_attr("\"><script>").contains('>'));
    }
}
