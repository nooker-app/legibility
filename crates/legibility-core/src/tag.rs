//! Interned element names.
//!
//! Scoring must do zero string work. Readability compares `node.tagName` and runs regexes over
//! `className`/`id` per node per pass; here names are resolved to a `u16` once during parsing
//! and every later comparison is an integer compare.

/// An interned element name.
///
/// Known HTML elements get a stable, `const`-usable id so that match arms in the scoring hot
/// path compile to integer comparisons. Unknown and custom elements are interned dynamically
/// above [`TagId::FIRST_DYNAMIC`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct TagId(pub u16);

macro_rules! known_tags {
    ($($konst:ident = $idx:expr, $name:literal;)*) => {
        impl TagId {
            $(
                #[doc = concat!("`<", $name, ">`")]
                pub const $konst: TagId = TagId($idx);
            )*

            /// First id available for dynamically interned (custom / unknown) elements.
            pub const FIRST_DYNAMIC: u16 = 1000;

            /// Resolve a lowercase element name to a known id.
            #[must_use]
            pub fn from_known(name: &str) -> Option<TagId> {
                match name {
                    $($name => Some(TagId::$konst),)*
                    _ => None,
                }
            }

            /// Name of a known id.
            #[must_use]
            pub fn known_name(self) -> Option<&'static str> {
                match self {
                    $(TagId::$konst => Some($name),)*
                    _ => None,
                }
            }
        }
    };
}

known_tags! {
    UNKNOWN = 0, "";
    A = 1, "a";
    ARTICLE = 2, "article";
    ASIDE = 3, "aside";
    BLOCKQUOTE = 4, "blockquote";
    BODY = 5, "body";
    BR = 6, "br";
    BUTTON = 7, "button";
    CODE = 8, "code";
    DD = 9, "dd";
    DETAILS = 10, "details";
    DIALOG = 11, "dialog";
    DIV = 12, "div";
    DL = 13, "dl";
    DT = 14, "dt";
    EMBED = 15, "embed";
    FIGCAPTION = 16, "figcaption";
    FIGURE = 17, "figure";
    FOOTER = 18, "footer";
    FORM = 19, "form";
    H1 = 20, "h1";
    H2 = 21, "h2";
    H3 = 22, "h3";
    H4 = 23, "h4";
    H5 = 24, "h5";
    H6 = 25, "h6";
    HEAD = 26, "head";
    HEADER = 27, "header";
    HTML = 28, "html";
    IFRAME = 29, "iframe";
    IMG = 30, "img";
    INPUT = 31, "input";
    LABEL = 32, "label";
    LI = 33, "li";
    LINK = 34, "link";
    MAIN = 35, "main";
    META = 36, "meta";
    NAV = 37, "nav";
    NOSCRIPT = 38, "noscript";
    OBJECT = 39, "object";
    OL = 40, "ol";
    OPTION = 41, "option";
    P = 42, "p";
    PRE = 43, "pre";
    SCRIPT = 44, "script";
    SECTION = 45, "section";
    SELECT = 46, "select";
    SPAN = 47, "span";
    STYLE = 48, "style";
    SUMMARY = 49, "summary";
    TABLE = 50, "table";
    TBODY = 51, "tbody";
    TD = 52, "td";
    TEMPLATE = 53, "template";
    TH = 54, "th";
    THEAD = 55, "thead";
    TIME = 56, "time";
    TITLE = 57, "title";
    TR = 58, "tr";
    UL = 59, "ul";
    VIDEO = 60, "video";
    AUDIO = 61, "audio";
}

impl TagId {
    /// Elements whose text is never prose.
    ///
    /// `<summary>` is deliberately absent: the heading of a collapsed section is content, and
    /// treating it as a control label loses it.
    #[must_use]
    pub const fn is_control_element(self) -> bool {
        matches!(self, TagId::BUTTON | TagId::LABEL | TagId::OPTION | TagId::SELECT | TagId::INPUT)
    }

    /// Elements that never render text into the document.
    #[must_use]
    pub const fn is_non_rendered(self) -> bool {
        matches!(
            self,
            TagId::SCRIPT | TagId::STYLE | TagId::NOSCRIPT | TagId::TEMPLATE | TagId::HEAD
        )
    }

    /// Elements that are positive semantic anchors for the article region.
    #[must_use]
    pub const fn is_positive_landmark(self) -> bool {
        matches!(self, TagId::ARTICLE | TagId::MAIN)
    }

    /// Elements that are negative (boilerplate) landmarks.
    #[must_use]
    pub const fn is_negative_landmark(self) -> bool {
        matches!(self, TagId::NAV | TagId::HEADER | TagId::FOOTER | TagId::ASIDE | TagId::FORM)
    }

    /// Elements whose content must survive verbatim, and which cleaning must never enter.
    #[must_use]
    pub const fn is_opaque(self) -> bool {
        matches!(self, TagId::PRE | TagId::CODE)
    }
}

#[cfg(test)]
// Tests may unwrap and may assert on const-evaluable predicates: these are regression
// guards against someone changing a const fn, not runtime checks.
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::assertions_on_constants)]
mod tests {
    use super::*;

    #[test]
    fn known_names_round_trip() {
        for name in ["a", "div", "p", "article", "summary", "template", "pre"] {
            let id = TagId::from_known(name).expect("known tag");
            assert_eq!(id.known_name(), Some(name));
        }
        assert!(TagId::from_known("my-custom-element").is_none());
        assert!(TagId::from_known("DIV").is_none(), "callers must lowercase first");
    }

    #[test]
    fn summary_is_prose_not_control() {
        // A collapsed section's heading is content. This is the one exception to
        // "interactive element labels are not prose" and it is easy to get wrong.
        assert!(!TagId::SUMMARY.is_control_element());
        assert!(TagId::BUTTON.is_control_element());
        assert!(TagId::LABEL.is_control_element());
    }

    #[test]
    fn landmark_classes_are_disjoint() {
        let all = [
            TagId::A,
            TagId::ARTICLE,
            TagId::MAIN,
            TagId::NAV,
            TagId::HEADER,
            TagId::FOOTER,
            TagId::ASIDE,
            TagId::FORM,
            TagId::DIV,
            TagId::P,
        ];
        for t in all {
            assert!(
                !(t.is_positive_landmark() && t.is_negative_landmark()),
                "{t:?} cannot be both a positive and a negative landmark"
            );
        }
    }

    #[test]
    fn known_ids_stay_below_first_dynamic() {
        // Dynamic interning starts above every known id; an overlap would silently alias a
        // custom element onto <div> and make scoring nonsense.
        for id in [TagId::AUDIO, TagId::VIDEO, TagId::UL] {
            assert!(id.0 < TagId::FIRST_DYNAMIC);
        }
    }

    #[test]
    fn non_rendered_and_opaque_sets_are_right() {
        assert!(TagId::SCRIPT.is_non_rendered());
        assert!(TagId::TEMPLATE.is_non_rendered());
        assert!(!TagId::DIV.is_non_rendered());
        assert!(TagId::PRE.is_opaque());
        assert!(!TagId::DIV.is_opaque());
    }
}
