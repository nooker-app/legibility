//! The document arena: a flat struct-of-arrays store in document order.
//!
//! # Why struct-of-arrays
//!
//! Scoring is a handful of linear sweeps over a few numeric columns. An array-of-structs
//! layout drags an entire node record into cache per touch; `SoA` touches only the columns a
//! pass actually reads. This is also what makes "a parameter variant costs one O(n) pass"
//! true — the arena is immutable during scoring, so there is no reason to clone a document
//! to try different parameters, which is exactly what Readability.js's retry ladder does.
//!
//! # Why five length columns rather than one
//!
//! `prose_len`, `control_len`, `hidden_len`, `alt_len` and `inert_len` are separate (plan
//! §1.10.1). A single `text_len` column is the bug: icon-font ligature text (`content_copy`),
//! "copy code" button labels and `aria-hidden` subtrees all inflate it, and every downstream
//! statistic — link density, text density, the page-relative z-scores — is then computed on
//! polluted lengths. Removing that text at serialization time is far too late; the region
//! has already been chosen wrongly. So the split happens during the parse, and every
//! feature reads `prose_len` only.
//!
//! `inert_len` was split back out of `hidden_len` after the plan was written, because lumping
//! them together reproduced the same class of bug one level down: `purity` divides prose by
//! *all* text, so a `<main>` holding an inlined 10 KB JavaScript bundle scored 0.04 and lost
//! to a "Be the first to comment" banner. Script source is not text competing with the
//! article; it is markup overhead. See [`crate::a11y::TextRole::Inert`].

use alloc::vec::Vec;

use crate::a11y::TextRole;
use crate::tag::TagId;

/// One attribute, with its value recorded as a span rather than a copy.
///
/// The span is what makes the verbatim invariant checkable: for any metadata candidate whose
/// transform set is exactly `{WS_NORMALIZED}`, `ws_normalize(&doc_buf[span]) == value`. Without a
/// span there is nothing to compare against and "we did not mangle this" is only an assertion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attr {
    /// Interned attribute name.
    pub name: AttrName,
    /// Start of the value in [`Arena::doc_buf`].
    pub value_start: u32,
    /// End of the value in [`Arena::doc_buf`].
    pub value_end: u32,
}

/// Interned attribute name.
///
/// Only names the engine actually consults get an id; everything else is [`AttrName::OTHER`].
/// Metadata extraction is a scan over millions of attributes on a large page, and comparing a
/// `u16` beats comparing strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AttrName(pub u16);

macro_rules! attr_names {
    ($($konst:ident = $idx:expr, $name:literal;)*) => {
        impl AttrName {
            $(
                #[doc = concat!("`", $name, "`")]
                pub const $konst: AttrName = AttrName($idx);
            )*
            /// Any attribute the engine does not consult.
            pub const OTHER: AttrName = AttrName(u16::MAX);

            /// Intern a lowercase attribute name.
            #[must_use]
            pub fn from_name(name: &str) -> AttrName {
                match name {
                    $($name => AttrName::$konst,)*
                    _ => AttrName::OTHER,
                }
            }

            /// The name of an interned id.
            #[must_use]
            pub fn as_str(self) -> &'static str {
                match self {
                    $(AttrName::$konst => $name,)*
                    _ => "",
                }
            }
        }
    };
}

attr_names! {
    CONTENT = 0, "content";
    PROPERTY = 1, "property";
    NAME = 2, "name";
    HREF = 3, "href";
    SRC = 4, "src";
    DATETIME = 5, "datetime";
    ITEMPROP = 6, "itemprop";
    ITEMTYPE = 7, "itemtype";
    ITEMSCOPE = 8, "itemscope";
    REL = 9, "rel";
    LANG = 10, "lang";
    CLASS = 11, "class";
    ID = 12, "id";
    ALT = 13, "alt";
    TITLE = 14, "title";
    TYPE = 15, "type";
    DIR = 16, "dir";
    ROLE = 17, "role";
    HTTP_EQUIV = 18, "http-equiv";
    CHARSET = 19, "charset";
    VALUE = 20, "value";
    STYLE = 21, "style";
    WIDTH = 22, "width";
    INDENT = 23, "indent";
}

/// Index into the arena's columns. `u32` rather than `usize`: it halves the index columns on
/// 64-bit targets and caps documents at ~4.29e9 nodes, which [`crate::Limits`] bounds far below.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub u32);

impl NodeId {
    /// Sentinel for "no node". Chosen as `u32::MAX` so that a valid id is never `NONE` and a
    /// forgotten initialization is an out-of-bounds index (caught) rather than node 0 (silent).
    pub const NONE: NodeId = NodeId(u32::MAX);

    /// Whether this id refers to an actual node.
    #[must_use]
    pub const fn is_some(self) -> bool {
        self.0 != u32::MAX
    }

    /// Usable as a slice index.
    #[must_use]
    pub const fn idx(self) -> usize {
        self.0 as usize
    }
}

/// What a node is. Deliberately small: attribute payloads and text live in side buffers, not
/// in this enum, so the kind column stays one byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum NodeKind {
    /// An element. Its name is in the `tag` column.
    Element = 0,
    /// A text node. Its bytes are `[text_start, text_end)` in the document buffer.
    Text = 1,
    /// A comment node. Retained because `<!--[if IE]>` style content affects tree shape.
    Comment = 2,
    /// A doctype.
    Doctype = 3,
    /// A processing instruction.
    ProcessingInstruction = 4,
    /// The document root.
    Document = 5,
    /// A node that exceeded [`crate::Limits::max_depth`].
    ///
    /// Crucially this is *allocated and recorded*, not dropped. html5ever's tree builder keeps
    /// its own open-elements stack holding handles it was given; if we drop a node instead of
    /// returning a usable handle, later `remove_from_parent` / `reparent_children` /
    /// `append_before_sibling` calls arrive for a node the arena never stored and the arena is
    /// corrupted. An orphan makes every such operation a well-defined no-op instead.
    DepthCappedOrphan = 6,
}

/// A finished, immutable, document-order arena.
///
/// Invariants, all established by `legibility-dom`'s flatten step and relied on everywhere:
///
/// 1. Index order is document order (pre-order).
/// 2. `subtree_end[n]` is the exclusive end of `n`'s subtree, so the descendants of `n` are
///    exactly `n+1 .. subtree_end[n]` — a contiguous slice, no pointer chasing.
/// 3. Therefore iterating `(0..len).rev()` lets a child's accumulated totals be folded into
///    its parent in **one** pass, because every descendant has a higher index than its
///    ancestor. This single property is what removes Readability's quadratic inner-text
///    recomputation.
#[derive(Debug, Clone, Default)]
pub struct Arena {
    // ---- structure ----
    /// Parent of each node; [`NodeId::NONE`] for the root.
    pub parent: Vec<NodeId>,
    /// Exclusive end index of each node's subtree. See invariant 2.
    pub subtree_end: Vec<u32>,
    /// Depth from the document root; the root is 0.
    pub depth: Vec<u16>,

    // ---- identity ----
    /// What each node is.
    pub kind: Vec<NodeKind>,
    /// Interned element name. Meaningless unless `kind` is [`NodeKind::Element`].
    pub tag: Vec<TagId>,

    // ---- text, as ranges into `doc_buf` ----
    /// Start offset of this node's own text in [`Arena::doc_buf`].
    pub text_start: Vec<u32>,
    /// End offset of this node's own text in [`Arena::doc_buf`].
    pub text_end: Vec<u32>,

    // ---- a11y classification (plan §1.10) ----
    /// Role of this node's own text. Inherited down the tree during flatten for `Hidden`.
    pub text_role: Vec<TextRole>,

    // ---- accumulated subtree totals, filled by the single reverse pass ----
    /// Bytes of `Prose` text in this subtree. **All features read this, never a total length.**
    pub prose_len: Vec<u32>,
    /// Bytes of `Control` text (button labels, icon ligatures) in this subtree.
    pub control_len: Vec<u32>,
    /// Bytes of `Hidden` text in this subtree — prose a reader cannot see, such as a
    /// `display:none` promo block. Dilutes `purity`, which is what it is for.
    pub hidden_len: Vec<u32>,
    /// Bytes of `AltOnly` text (`sr-only` and friends) in this subtree.
    pub alt_len: Vec<u32>,
    /// Bytes of [`TextRole::Inert`] non-text in this subtree: script and stylesheet source,
    /// template contents, comments, doctype.
    ///
    /// Deliberately read by **no** feature. It is recorded so diagnostics can show where a
    /// document's bytes went, and separated from `hidden_len` so that inlining a JS bundle
    /// inside `<main>` cannot make `<main>` look impure.
    pub inert_len: Vec<u32>,
    /// Bytes of `Prose` text inside `<a>` subtrees. Numerator of link density.
    pub link_prose_len: Vec<u32>,
    /// Count of descendant elements, including this node if it is one. Denominator of text density.
    pub element_count: Vec<u32>,

    // ---- attributes, as ranges into a flat table ----
    /// Index of this node's first attribute in [`Arena::attrs`].
    pub attr_start: Vec<u32>,
    /// Number of attributes this node has.
    pub attr_len: Vec<u16>,
    /// Every attribute of every element, in document order.
    ///
    /// Flat rather than per-node `Vec<Attr>`: one allocation for the document instead of one per
    /// element, and attribute scans stay linear over contiguous memory.
    pub attrs: Vec<Attr>,

    // ---- side buffers ----
    /// Every text node and attribute value, copied here at parse time.
    ///
    /// This exists because html5ever hands over entity-decoded `StrTendril` and offers no
    /// per-node source span (`set_current_line` is the only positional hook). Owning this
    /// buffer is what makes the verbatim invariant checkable: for any metadata candidate whose
    /// transform set is exactly `{WS_NORMALIZED}`, `ws_normalize(&doc_buf[span]) == value`.
    ///
    /// Note the honest scope: this proves *we* did not mangle the value. It is not
    /// byte-fidelity against the original network bytes, which v1 does not offer.
    pub doc_buf: alloc::string::String,

    /// Names of custom and unknown elements, indexed by `TagId - TagId::FIRST_DYNAMIC`.
    ///
    /// Kept because pages are now largely custom elements — Reddit's post, comment tree and
    /// composer are all `<shreddit-*>` — and a name is often the only thing that says what one is.
    /// Dropping them at flatten left core able to see *that* an element was unknown but not
    /// *which*, which is the difference between "some element" and "the comment tree".
    pub dynamic_tags: alloc::vec::Vec<alloc::string::String>,
}

impl Arena {
    /// Number of nodes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.kind.len()
    }

    /// Whether the arena holds no nodes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.kind.is_empty()
    }

    /// Descendant range of `n`, exclusive of `n` itself.
    ///
    /// Returns an empty range for an unknown id rather than panicking: this is called from
    /// scoring paths where `indexing_slicing` is denied and a bad id must degrade, not abort.
    #[must_use]
    pub fn descendants(&self, n: NodeId) -> core::ops::Range<usize> {
        match self.subtree_end.get(n.idx()) {
            Some(&end) => (n.idx() + 1)..(end as usize),
            None => 0..0,
        }
    }

    /// Attributes of `n`.
    #[must_use]
    pub fn attrs_of(&self, n: NodeId) -> &[Attr] {
        let start = self.attr_start.get(n.idx()).copied().unwrap_or(0) as usize;
        let len = self.attr_len.get(n.idx()).copied().unwrap_or(0) as usize;
        self.attrs.get(start..start.saturating_add(len)).unwrap_or(&[])
    }

    /// Value of `n`'s first attribute named `name`, or `None`.
    #[must_use]
    pub fn attr(&self, n: NodeId, name: AttrName) -> Option<&str> {
        self.attrs_of(n)
            .iter()
            .find(|a| a.name == name)
            .and_then(|a| self.doc_buf.get(a.value_start as usize..a.value_end as usize))
    }

    /// Span of `n`'s first attribute named `name`.
    ///
    /// Returned separately from the value so a metadata candidate can carry the span it came
    /// from and be checked against `doc_buf` later.
    #[must_use]
    pub fn attr_span(&self, n: NodeId, name: AttrName) -> Option<(u32, u32)> {
        self.attrs_of(n).iter().find(|a| a.name == name).map(|a| (a.value_start, a.value_end))
    }

    /// Text of a span in [`Arena::doc_buf`].
    #[must_use]
    pub fn span_text(&self, start: u32, end: u32) -> &str {
        self.doc_buf.get(start as usize..end as usize).unwrap_or("")
    }

    /// Element name, whether standard or custom. `None` for non-elements.
    #[must_use]
    pub fn tag_name(&self, n: NodeId) -> Option<&str> {
        let tag = self.tag.get(n.idx()).copied()?;
        if let Some(known) = tag.known_name() {
            return Some(known);
        }
        let idx = tag.0.checked_sub(crate::TagId::FIRST_DYNAMIC)? as usize;
        self.dynamic_tags.get(idx).map(alloc::string::String::as_str)
    }

    /// This node's own text, or `""` if it has none or the id is unknown.
    #[must_use]
    pub fn own_text(&self, n: NodeId) -> &str {
        let (Some(&s), Some(&e)) = (self.text_start.get(n.idx()), self.text_end.get(n.idx()))
        else {
            return "";
        };
        self.doc_buf.get(s as usize..e as usize).unwrap_or("")
    }

    /// Fold every subtree total in **one** reverse pass.
    ///
    /// Correctness rests entirely on arena invariant 1: in pre-order, every descendant has a
    /// strictly higher index than its ancestor. Walking indices downward therefore guarantees a
    /// node's own subtree is already complete when we add it to its parent, with no recursion,
    /// no work queue, and no revisiting.
    ///
    /// Saturating arithmetic throughout: a hostile document can overflow a `u32` byte count,
    /// and saturating is a documented degradation (the node is "very large") whereas wrapping
    /// silently inverts comparisons and a panic violates S1.
    pub fn accumulate_subtrees(&mut self, a_tag: TagId) {
        let n = self.len();
        for i in (0..n).rev() {
            // Seed from this node's own text, classified by role.
            //
            // A text node that is nothing but whitespace contributes zero. Indentation is not
            // content, and counting it is not a rounding error: pretty-printed markup puts a
            // newline and several spaces between every pair of tags, so each wrapper in a chain
            // adds bytes that are prose by type and furniture in fact. On a Reddit link post the
            // body is one anchor inside four nested `<div>`s, and the accumulated indentation
            // diluted `link_density` from 0.87 at the `<p>` to 0.74 three levels up -- under the
            // viability floor -- which made a bare URL read as a submission body.
            let start = self.text_start.get(i).copied().unwrap_or(0) as usize;
            let end = self.text_end.get(i).copied().unwrap_or(0) as usize;
            let raw = end.saturating_sub(start);
            let own = if raw == 0
                || self.doc_buf.get(start..end).is_some_and(|s| s.chars().all(char::is_whitespace))
            {
                0
            } else {
                u32::try_from(raw).unwrap_or(u32::MAX)
            };

            match self.text_role.get(i).copied().unwrap_or(TextRole::Prose) {
                TextRole::Prose => add_at(&mut self.prose_len, i, own),
                TextRole::Control => add_at(&mut self.control_len, i, own),
                TextRole::Hidden => add_at(&mut self.hidden_len, i, own),
                TextRole::AltOnly => add_at(&mut self.alt_len, i, own),
                // Its own column, read by no feature. Script and stylesheet source, template
                // contents and comments are markup overhead rather than text; keeping them out
                // of the other four columns is the whole point, and keeping them in *a* column
                // means the bytes are still reportable instead of vanishing.
                TextRole::Inert => add_at(&mut self.inert_len, i, own),
            }

            if self.kind.get(i).copied() == Some(NodeKind::Element) {
                add_at(&mut self.element_count, i, 1);
                // An <a> contributes its whole prose subtree to the link numerator. Done here,
                // after children have folded up, so the subtree total is already final.
                if self.tag.get(i).copied() == Some(a_tag) {
                    let p = self.prose_len.get(i).copied().unwrap_or(0);
                    set_at(&mut self.link_prose_len, i, p);
                }
            }

            // Fold into the parent.
            let Some(&parent) = self.parent.get(i) else { continue };
            if !parent.is_some() {
                continue;
            }
            let p = parent.idx();
            for col in [
                &mut self.prose_len,
                &mut self.control_len,
                &mut self.hidden_len,
                &mut self.alt_len,
                &mut self.inert_len,
                &mut self.link_prose_len,
                &mut self.element_count,
            ] {
                let v = col.get(i).copied().unwrap_or(0);
                add_at(col, p, v);
            }
        }
    }

    /// Link density over **prose only**.
    ///
    /// `Control` text is excluded from both numerator and denominator by construction, since
    /// neither is counted in `prose_len`. That matters: a toolbar of icon buttons should read
    /// as "no prose", not as "prose with suspiciously high link density", and Readability
    /// reads it as the latter.
    ///
    /// Returns 0.0 for an empty node — see [`crate::num::guarded_div`] for why every division
    /// in this crate goes through a guard.
    #[must_use]
    pub fn link_density(&self, n: NodeId) -> f32 {
        let prose = self.prose_len.get(n.idx()).copied().unwrap_or(0);
        let link = self.link_prose_len.get(n.idx()).copied().unwrap_or(0);
        crate::num::guarded_div(link as f32, prose as f32)
    }

    /// **M0 placeholder scorer.** Replaced wholesale by the modern scorer in M6.
    ///
    /// `text_density * (1 - link_density)`. Two features are the minimum: neither alone
    /// separates an article from a navigation block, and finding that out is worth recording.
    /// A link list is *dense* — six `<a>` elements each holding ten characters beats two
    /// paragraphs on chars-per-tag — so [`Arena::text_density`] alone ranks a nav block above
    /// the article. [`Arena::link_density`] is what discriminates them, and it only works as a
    /// multiplier because it is already a ratio.
    ///
    /// Deliberately unnormalized and uncalibrated. It exists so the pipeline is exercisable
    /// end to end before any real heuristic, and so the M0 gate has something to measure.
    /// It is *not* the design: it has no page-relative statistic, no semantic anchor, no
    /// purity floor and no confidence, which are exactly the things M4 through M6 add.
    #[must_use]
    pub fn placeholder_evidence(&self, n: NodeId) -> f32 {
        let density = self.text_density(n);
        let links = self.link_density(n);
        density * (1.0 - links.clamp(0.0, 1.0))
    }

    /// Prose bytes per descendant element — the scale-invariant replacement for
    /// Readability's absolute `min(floor(len / 100), 3)` term.
    ///
    /// Scale invariance is the entire point of defect 1: duplicating every text node in a
    /// document must not change which region wins, and an absolute length term guarantees
    /// that it does.
    #[must_use]
    pub fn text_density(&self, n: NodeId) -> f32 {
        let prose = self.prose_len.get(n.idx()).copied().unwrap_or(0);
        let elems = self.element_count.get(n.idx()).copied().unwrap_or(0);
        crate::num::guarded_div(prose as f32, elems as f32)
    }
}

#[inline]
fn add_at(col: &mut [u32], i: usize, v: u32) {
    if let Some(slot) = col.get_mut(i) {
        *slot = slot.saturating_add(v);
    }
}

#[inline]
fn set_at(col: &mut [u32], i: usize, v: u32) {
    if let Some(slot) = col.get_mut(i) {
        *slot = v;
    }
}
