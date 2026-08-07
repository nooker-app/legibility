//! The only html5ever consumer.
//!
//! Phase 1 of the two-phase arena (plan §1.1). html5ever's `TreeSink` takes `&self` and calls
//! `remove_from_parent`, `reparent_children` and `append_before_sibling` *while parsing* — the
//! adoption agency algorithm and table foster parenting both do — so **allocation order is not
//! document order** and a single-pass document-order arena is impossible.
//!
//! So this builds a mutable doubly-linked arena where those three operations are O(1), then
//! [`BuildArena::flatten`] walks it once with an explicit stack to produce the immutable
//! document-order [`Arena`] that `legibility-core` scores.
//!
//! Everything downstream of `flatten` is index arithmetic on contiguous slices. That is the
//! whole performance argument: Readability recomputes `getInnerText` per candidate per
//! ancestor, which is quadratic on deep documents.

#![forbid(unsafe_code)]
#![deny(clippy::all)]
#![warn(missing_docs)]

pub mod json;
pub mod serialize;

use std::cell::{Ref, RefCell};

use html5ever::interface::{ElementFlags, NodeOrText, QuirksMode, TreeSink};
use html5ever::tendril::{StrTendril, TendrilSink};
use html5ever::{local_name, ns, Attribute, QualName};

use legibility_core::a11y::{
    inline_style_hides, is_hidden_attr_value, is_private_use_only, HiddenReason, TextRole,
};
use legibility_core::{Arena, Limits, LimitsHit, NodeId, NodeKind, TagId};

const NONE: u32 = u32::MAX;

/// Class tokens that mark screen-reader-only content.
///
/// This lexicon is doing the job external CSS would do. v1 does not parse stylesheets (see
/// `docs/limits.md`), and these five class names cover the overwhelming majority of
/// visually-hidden content in the wild.
const SR_ONLY_CLASSES: [&str; 5] =
    ["sr-only", "visually-hidden", "visuallyhidden", "screen-reader-text", "assistive-text"];

/// ARIA roles whose subtree text is control text rather than prose.
const CONTROL_ROLES: [&str; 13] = [
    "button",
    "tab",
    "menuitem",
    "menuitemcheckbox",
    "menuitemradio",
    "switch",
    "checkbox",
    "radio",
    "slider",
    "spinbutton",
    "option",
    "toolbar",
    "tablist",
];

/// ARIA roles that are live regions: dynamic status text, never article prose.
const LIVE_ROLES: [&str; 5] = ["log", "status", "alert", "marquee", "timer"];

/// One node in the mutable build tree.
struct BuildNode {
    parent: u32,
    first_child: u32,
    last_child: u32,
    prev_sib: u32,
    next_sib: u32,
    kind: NodeKind,
    name: QualName,
    tag: TagId,
    text: (u32, u32),
    /// Role declared by this node itself, before inheritance.
    own_role: TextRole,
    /// Why this node is hidden, if it is. Kept so the recovery pass can revert only the
    /// site-authored reasons (`aria-hidden`, inline style) and never the factual ones.
    hidden_reason: Option<HiddenReason>,
    /// Depth from the document node, tracked as the tree is linked.
    depth: u16,
    /// A `<template>`'s content fragment.
    template_contents: u32,
    /// Range into `Build::attrs`.
    attr_start: u32,
    attr_len: u16,
}

impl BuildNode {
    fn new(kind: NodeKind, name: QualName) -> Self {
        Self {
            parent: NONE,
            first_child: NONE,
            last_child: NONE,
            prev_sib: NONE,
            next_sib: NONE,
            kind,
            name,
            tag: TagId::UNKNOWN,
            text: (0, 0),
            own_role: TextRole::Prose,
            hidden_reason: None,
            depth: 0,
            template_contents: NONE,
            attr_start: 0,
            attr_len: 0,
        }
    }
}

struct Build {
    nodes: Vec<BuildNode>,
    doc_buf: String,
    quirks: QuirksMode,
    limits_hit: LimitsHit,
    /// Flat attribute table shared by every element.
    attrs: Vec<legibility_core::arena::Attr>,
    /// Dynamically interned names for custom/unknown elements.
    dynamic_tags: Vec<String>,
    errors: usize,
    /// First build index past [`Limits::max_nodes`]; nothing from here on is recorded.
    ///
    /// `None` until the cap is reached. See [`Build::new_node`] for why the cap cannot be enforced
    /// at allocation time.
    overflow_sink: Option<u32>,
}

/// Phase-1 arena and html5ever sink.
///
/// `TreeSink` methods take `&self`, hence the `RefCell`. This is not a design choice we get to
/// make — it is the trait's shape.
pub struct BuildArena {
    inner: RefCell<Build>,
    limits: Limits,
}

impl BuildArena {
    /// Create an empty arena containing only the document node.
    #[must_use]
    pub fn new(limits: Limits) -> Self {
        let doc = BuildNode::new(NodeKind::Document, QualName::new(None, ns!(), local_name!("")));
        Self {
            inner: RefCell::new(Build {
                nodes: vec![doc],
                doc_buf: String::new(),
                attrs: Vec::new(),
                quirks: QuirksMode::NoQuirks,
                limits_hit: LimitsHit::default(),
                dynamic_tags: Vec::new(),
                errors: 0,
                overflow_sink: None,
            }),
            limits,
        }
    }

    /// Parse `html` and flatten straight to an immutable document-order [`Arena`].
    ///
    /// Oversized input is truncated at a UTF-8 boundary rather than rejected: guarantee S2 says
    /// every limit degrades and none of them fails.
    #[must_use]
    pub fn parse_to_arena(html: &str, limits: Limits) -> (Arena, LimitsHit) {
        let cap = limits.max_input_bytes as usize;
        let (input, truncated) = if html.len() > cap {
            let mut end = cap;
            while end > 0 && !html.is_char_boundary(end) {
                end -= 1;
            }
            (html.get(..end).unwrap_or(""), true)
        } else {
            (html, false)
        };

        let sink = BuildArena::new(limits);
        let parsed: BuildArena = html5ever::parse_document(sink, html5ever::ParseOpts::default())
            .from_utf8()
            .one(input.as_bytes());

        let (mut arena, mut hit) = parsed.flatten();
        if truncated {
            hit.input_bytes = true;
        }
        arena.accumulate_subtrees(TagId::A);
        (arena, hit)
    }

    /// Phase 2: walk the mutable tree once, in document order, into the immutable arena.
    ///
    /// Iterative with an explicit stack. Recursion here would be a remote stack-overflow
    /// primitive — nesting depth is attacker-controlled and a stack overflow cannot be caught.
    ///
    /// `subtree_end` is filled on the way *out* of each node, which is why the stack holds an
    /// "exit" marker per entry rather than just a node id.
    #[must_use]
    pub fn flatten(self) -> (Arena, LimitsHit) {
        let build = self.inner.into_inner();
        // doc_buf moves over wholesale: it was accumulated during parsing and every text/attr
        // range already indexes into it. Copying it again would be the single largest
        // avoidable allocation in the pipeline.
        let mut arena = Arena {
            doc_buf: build.doc_buf,
            attrs: build.attrs,
            dynamic_tags: build.dynamic_tags,
            ..Arena::default()
        };

        // (build index, inherited role, exit-marker for already-emitted node)
        enum Step {
            Enter(u32, TextRole),
            Exit(u32),
        }

        // Everything at or past this build index was allocated after `max_nodes` was reached, and is
        // not recorded — the documented degradation for that limit. Enforced here rather than at
        // allocation because html5ever needs unique handles to keep its own stack coherent.
        let cut = build.overflow_sink.unwrap_or(u32::MAX);

        let mut stack: Vec<Step> = vec![Step::Enter(0, TextRole::Prose)];
        // Maps build index -> emitted arena index, needed to patch subtree_end on exit.
        let mut emitted: Vec<u32> = vec![NONE; build.nodes.len()];
        let mut parent_stack: Vec<u32> = Vec::new();

        while let Some(step) = stack.pop() {
            match step {
                Step::Exit(bi) => {
                    let ai = emitted.get(bi as usize).copied().unwrap_or(NONE);
                    if ai != NONE {
                        if let Some(slot) = arena.subtree_end.get_mut(ai as usize) {
                            *slot = arena.kind.len() as u32;
                        }
                    }
                    parent_stack.pop();
                }
                Step::Enter(bi, inherited) => {
                    if bi >= cut {
                        continue;
                    }
                    let Some(node) = build.nodes.get(bi as usize) else { continue };

                    let role = TextRole::inherit(inherited, node.own_role);
                    let ai = arena.kind.len() as u32;
                    if let Some(slot) = emitted.get_mut(bi as usize) {
                        *slot = ai;
                    }

                    let parent = parent_stack.last().copied().map_or(NodeId::NONE, NodeId);

                    arena.parent.push(parent);
                    arena.subtree_end.push(ai + 1);
                    arena.depth.push(node.depth);
                    arena.kind.push(node.kind);
                    arena.tag.push(node.tag);
                    arena.text_start.push(node.text.0);
                    arena.text_end.push(node.text.1);
                    arena.text_role.push(role);
                    arena.attr_start.push(node.attr_start);
                    arena.attr_len.push(node.attr_len);
                    arena.prose_len.push(0);
                    arena.control_len.push(0);
                    arena.hidden_len.push(0);
                    arena.alt_len.push(0);
                    arena.inert_len.push(0);
                    arena.link_prose_len.push(0);
                    arena.element_count.push(0);

                    parent_stack.push(ai);
                    stack.push(Step::Exit(bi));

                    // Push children in reverse so they pop in document order.
                    let mut kids: Vec<u32> = Vec::new();
                    let mut c = node.first_child;
                    while c != NONE {
                        kids.push(c);
                        c = build.nodes.get(c as usize).map_or(NONE, |n| n.next_sib);
                    }
                    for &k in kids.iter().rev() {
                        stack.push(Step::Enter(k, role));
                    }
                }
            }
        }

        (arena, build.limits_hit)
    }
}

impl Build {
    fn intern(&mut self, name: &QualName) -> TagId {
        let local = name.local.as_ref();
        if let Some(id) = TagId::from_known(local) {
            return id;
        }
        if let Some(pos) = self.dynamic_tags.iter().position(|t| t == local) {
            return TagId(TagId::FIRST_DYNAMIC.saturating_add(pos as u16));
        }
        self.dynamic_tags.push(local.to_string());
        let idx = self.dynamic_tags.len().saturating_sub(1);
        TagId(TagId::FIRST_DYNAMIC.saturating_add(idx as u16))
    }

    fn push_text(&mut self, s: &str, limits: &Limits) -> (u32, u32) {
        // Text is bounded by `max_input_bytes`, not by `max_attr_bytes`. The two used to share this
        // function *and* its cap, so any text node over 64 KiB was silently cut and the loss was
        // reported as `attr_bytes` -- a long article with one big paragraph lost its ending and said
        // it had hit an attribute limit. `doc_buf` cannot exceed the document anyway, so this is a
        // ceiling rather than a policy.
        self.push_capped(s, limits.max_input_bytes as usize, false)
    }

    /// Copy an attribute value, capped at [`Limits::max_attr_bytes`].
    fn push_attr_value(&mut self, s: &str, limits: &Limits) -> (u32, u32) {
        self.push_capped(s, limits.max_attr_bytes as usize, true)
    }

    fn push_capped(&mut self, s: &str, room: usize, is_attr: bool) -> (u32, u32) {
        let start = self.doc_buf.len() as u32;
        if s.len() > room {
            let mut end = room;
            while end > 0 && !s.is_char_boundary(end) {
                end -= 1;
            }
            self.doc_buf.push_str(s.get(..end).unwrap_or(""));
            if is_attr {
                self.limits_hit.attr_bytes = true;
            } else {
                self.limits_hit.input_bytes = true;
            }
        } else {
            self.doc_buf.push_str(s);
        }
        (start, self.doc_buf.len() as u32)
    }

    /// Copy attribute values into `doc_buf` and record their spans.
    ///
    /// The value is copied rather than referenced because html5ever hands over an
    /// entity-decoded `StrTendril` and offers no source span. Owning the bytes is what makes the
    /// verbatim invariant checkable at all -- see `Arena::doc_buf`.
    fn push_attrs(&mut self, attrs: &[Attribute], limits: &Limits) -> (u32, u16) {
        let start = self.attrs.len() as u32;
        let mut n = 0u16;
        for a in attrs {
            let name = legibility_core::arena::AttrName::from_name(a.name.local.as_ref());
            let (vs, ve) = self.push_attr_value(a.value.as_ref(), limits);
            self.attrs.push(legibility_core::arena::Attr { name, value_start: vs, value_end: ve });
            n = n.saturating_add(1);
        }
        (start, n)
    }

    /// Classify an element from its attributes.
    ///
    /// Returns the role its subtree inherits and, if hidden, why. The *reason* matters: the
    /// recovery pass reverts only `aria-hidden` and inline styles, because those are things a
    /// site can be wrong about, whereas `<template>` contents and `<script>` text are not
    /// opinions.
    fn classify(tag: TagId, attrs: &[Attribute]) -> (TextRole, Option<HiddenReason>) {
        if tag.is_non_rendered() {
            // Inert, not Hidden: these bytes are program or stylesheet source, and a length
            // column that feeds `purity` must not see them. See a11y::TextRole::Inert.
            return (TextRole::Inert, Some(HiddenReason::NonRendered));
        }

        let mut role = TextRole::Prose;
        let mut hidden: Option<HiddenReason> = None;
        let mut dialog_open = false;

        for a in attrs {
            let name = a.name.local.as_ref();
            let value = a.value.as_ref();
            match name {
                "hidden" if is_hidden_attr_value(value) => {
                    hidden = Some(HiddenReason::HiddenAttr);
                }
                "aria-hidden" if value.eq_ignore_ascii_case("true") => {
                    hidden = hidden.or(Some(HiddenReason::AriaHidden));
                }
                "inert" => hidden = hidden.or(Some(HiddenReason::Inert)),
                "open" => dialog_open = true,
                "style" if inline_style_hides(value) => {
                    hidden = hidden.or(Some(HiddenReason::InlineStyle));
                }
                "role" => {
                    let v = value.trim();
                    if CONTROL_ROLES.iter().any(|r| v.eq_ignore_ascii_case(r))
                        || LIVE_ROLES.iter().any(|r| v.eq_ignore_ascii_case(r))
                    {
                        role = TextRole::inherit(role, TextRole::Control);
                    }
                    // Deliberately NOT handled: role="presentation" / role="none". They strip
                    // semantics but leave the text in the accessibility tree. Treating them as
                    // hidden silently deletes real article text — a table laid out with
                    // role="presentation" is still full of content.
                }
                "aria-live" => role = TextRole::inherit(role, TextRole::Control),
                "class" => {
                    if value
                        .split_ascii_whitespace()
                        .any(|t| SR_ONLY_CLASSES.iter().any(|s| t.eq_ignore_ascii_case(s)))
                    {
                        role = TextRole::inherit(role, TextRole::AltOnly);
                    }
                }
                _ => {}
            }
        }

        if tag == TagId::DIALOG && !dialog_open {
            hidden = hidden.or(Some(HiddenReason::ClosedDialog));
        }
        if tag.is_control_element() {
            role = TextRole::inherit(role, TextRole::Control);
        }

        if hidden.is_some() {
            role = TextRole::Hidden;
        }
        (role, hidden)
    }

    fn unlink(&mut self, target: u32) {
        let (parent, prev, next) = match self.nodes.get(target as usize) {
            Some(n) => (n.parent, n.prev_sib, n.next_sib),
            None => return,
        };
        if parent == NONE {
            return;
        }
        if prev != NONE {
            if let Some(p) = self.nodes.get_mut(prev as usize) {
                p.next_sib = next;
            }
        } else if let Some(p) = self.nodes.get_mut(parent as usize) {
            p.first_child = next;
        }
        if next != NONE {
            if let Some(n) = self.nodes.get_mut(next as usize) {
                n.prev_sib = prev;
            }
        } else if let Some(p) = self.nodes.get_mut(parent as usize) {
            p.last_child = prev;
        }
        if let Some(t) = self.nodes.get_mut(target as usize) {
            t.parent = NONE;
            t.prev_sib = NONE;
            t.next_sib = NONE;
        }
    }

    fn link_last(&mut self, parent: u32, child: u32, max_depth: u16) {
        self.unlink(child);
        let pd = self.nodes.get(parent as usize).map_or(0, |n| n.depth);
        if pd >= max_depth {
            // Allocate-and-orphan (plan §1.1). The node keeps existing and its handle stays
            // valid, so any later remove_from_parent / reparent_children / append_before_sibling
            // for it is a well-defined no-op. Dropping it instead would leave html5ever's
            // open-elements stack holding a handle the arena never stored.
            if let Some(c) = self.nodes.get_mut(child as usize) {
                c.kind = NodeKind::DepthCappedOrphan;
            }
            self.limits_hit.depth = true;
            return;
        }
        let last = self.nodes.get(parent as usize).map_or(NONE, |n| n.last_child);
        if let Some(c) = self.nodes.get_mut(child as usize) {
            c.parent = parent;
            c.prev_sib = last;
            c.next_sib = NONE;
            c.depth = pd.saturating_add(1);
        }
        if last == NONE {
            if let Some(p) = self.nodes.get_mut(parent as usize) {
                p.first_child = child;
            }
        } else if let Some(l) = self.nodes.get_mut(last as usize) {
            l.next_sib = child;
        }
        if let Some(p) = self.nodes.get_mut(parent as usize) {
            p.last_child = child;
        }
    }

    fn link_before(&mut self, sibling: u32, new_node: u32) {
        self.unlink(new_node);
        let (parent, prev, depth) = match self.nodes.get(sibling as usize) {
            Some(n) => (n.parent, n.prev_sib, n.depth),
            None => return,
        };
        if parent == NONE {
            return;
        }
        if let Some(n) = self.nodes.get_mut(new_node as usize) {
            n.parent = parent;
            n.prev_sib = prev;
            n.next_sib = sibling;
            n.depth = depth;
        }
        if prev == NONE {
            if let Some(p) = self.nodes.get_mut(parent as usize) {
                p.first_child = new_node;
            }
        } else if let Some(p) = self.nodes.get_mut(prev as usize) {
            p.next_sib = new_node;
        }
        if let Some(s) = self.nodes.get_mut(sibling as usize) {
            s.prev_sib = new_node;
        }
    }

    /// Allocate a node, marking where [`Limits::max_nodes`] was crossed.
    ///
    /// # Why the cap cannot be applied here
    ///
    /// It used to return handle `0` — the *document* — as a sink for everything past the cap, on the
    /// reasoning that "every later operation on it is structurally harmless". It is not. The tree
    /// builder puts whatever `create_element` returns on its open-elements stack, compares handles
    /// for identity, and pops them; handing back one shared handle made two different elements
    /// indistinguishable, the stack unbalanced, and html5ever panicked with `no current element`.
    /// Inside a dependency, so `#![forbid(unsafe_code)]` and every discipline in this crate were
    /// irrelevant. **Any document past the cap crashed the library** — exactly what S1 and S2 exist
    /// to rule out — and no test covered the path. Returning a single *element* instead was tried
    /// next and panicked identically, because the problem is aliasing rather than kind.
    ///
    /// A `TreeSink` has no way to say "stop". So the cap moves to where we do have a choice:
    /// allocation continues with unique handles, and [`BuildArena::flatten`] refuses to record
    /// anything at or past `overflow_sink`. That is the degradation the documentation already
    /// promised — "the remainder of the document is not parsed" — delivered one phase later, and the
    /// build arena is still bounded, because a node cannot exist without input bytes to spell it and
    /// `max_input_bytes` bounds those.
    fn new_node(&mut self, kind: NodeKind, name: QualName, limits: &Limits) -> u32 {
        if self.nodes.len() as u32 >= limits.max_nodes && self.overflow_sink.is_none() {
            self.limits_hit.nodes = true;
            self.overflow_sink = Some(self.nodes.len() as u32);
        }
        self.nodes.push(BuildNode::new(kind, name));
        (self.nodes.len() - 1) as u32
    }
}

impl TreeSink for BuildArena {
    type Handle = u32;
    type Output = Self;
    type ElemName<'a>
        = Ref<'a, QualName>
    where
        Self: 'a;

    fn finish(self) -> Self::Output {
        self
    }

    fn parse_error(&self, _msg: std::borrow::Cow<'static, str>) {
        self.inner.borrow_mut().errors += 1;
    }

    fn get_document(&self) -> u32 {
        0
    }

    fn elem_name<'a>(&'a self, target: &'a u32) -> Ref<'a, QualName> {
        let idx = *target as usize;
        Ref::map(self.inner.borrow(), move |b| match b.nodes.get(idx) {
            Some(n) => &n.name,
            // Unreachable in practice: every handle we hand out indexes a node we allocated.
            // Returning a shared empty name keeps this total rather than panicking, because the
            // trait docs invite `panic!` here and S1 does not permit it.
            None => &*EMPTY_NAME,
        })
    }

    fn create_element(&self, name: QualName, attrs: Vec<Attribute>, _f: ElementFlags) -> u32 {
        let mut b = self.inner.borrow_mut();
        let id = b.new_node(NodeKind::Element, name.clone(), &self.limits);
        let tag = b.intern(&name);

        let kept = if attrs.len() > self.limits.max_attrs_per_node as usize {
            b.limits_hit.attrs_per_node = true;
            attrs.get(..self.limits.max_attrs_per_node as usize).unwrap_or(&attrs)
        } else {
            attrs.as_slice()
        };
        let (role, hidden) = Build::classify(tag, kept);
        let kept: Vec<Attribute> = kept.to_vec();
        let (attr_start, attr_len) = b.push_attrs(&kept, &self.limits);

        if let Some(n) = b.nodes.get_mut(id as usize) {
            n.tag = tag;
            n.own_role = role;
            n.hidden_reason = hidden;
            n.attr_start = attr_start;
            n.attr_len = attr_len;
        }
        id
    }

    fn create_comment(&self, text: StrTendril) -> u32 {
        let mut b = self.inner.borrow_mut();
        let id = b.new_node(NodeKind::Comment, (*EMPTY_NAME).clone(), &self.limits);
        let range = b.push_text(&text, &self.limits);
        if let Some(n) = b.nodes.get_mut(id as usize) {
            n.text = range;
            // Comment text never renders, so it must not reach any statistic.
            n.own_role = TextRole::Inert;
            n.hidden_reason = Some(HiddenReason::NonRendered);
        }
        id
    }

    fn create_pi(&self, target: StrTendril, _data: StrTendril) -> u32 {
        let mut b = self.inner.borrow_mut();
        let id = b.new_node(NodeKind::ProcessingInstruction, (*EMPTY_NAME).clone(), &self.limits);
        let range = b.push_text(&target, &self.limits);
        if let Some(n) = b.nodes.get_mut(id as usize) {
            n.text = range;
            n.own_role = TextRole::Inert;
            n.hidden_reason = Some(HiddenReason::NonRendered);
        }
        id
    }

    fn append(&self, parent: &u32, child: NodeOrText<u32>) {
        let mut b = self.inner.borrow_mut();
        match child {
            NodeOrText::AppendNode(id) => b.link_last(*parent, id, self.limits.max_depth),
            NodeOrText::AppendText(text) => self.append_text(&mut b, *parent, &text),
        }
    }

    fn append_based_on_parent_node(
        &self,
        element: &u32,
        prev_element: &u32,
        child: NodeOrText<u32>,
    ) {
        let has_parent =
            self.inner.borrow().nodes.get(*element as usize).is_some_and(|n| n.parent != NONE);
        if has_parent {
            self.append_before_sibling(element, child);
        } else {
            self.append(prev_element, child);
        }
    }

    fn append_doctype_to_document(&self, name: StrTendril, _p: StrTendril, _s: StrTendril) {
        let mut b = self.inner.borrow_mut();
        let id = b.new_node(NodeKind::Doctype, (*EMPTY_NAME).clone(), &self.limits);
        let range = b.push_text(&name, &self.limits);
        if let Some(n) = b.nodes.get_mut(id as usize) {
            n.text = range;
            n.own_role = TextRole::Inert;
            n.hidden_reason = Some(HiddenReason::NonRendered);
        }
        b.link_last(0, id, self.limits.max_depth);
    }

    fn get_template_contents(&self, target: &u32) -> u32 {
        let mut b = self.inner.borrow_mut();
        let existing = b.nodes.get(*target as usize).map_or(NONE, |n| n.template_contents);
        if existing != NONE {
            return existing;
        }
        let frag = b.new_node(NodeKind::Element, (*EMPTY_NAME).clone(), &self.limits);
        if let Some(n) = b.nodes.get_mut(frag as usize) {
            // Template contents never render. Marking the fragment Inert means the whole
            // subtree inherits it in flatten and none of it reaches a statistic.
            n.own_role = TextRole::Inert;
            n.hidden_reason = Some(HiddenReason::Template);
            n.tag = TagId::TEMPLATE;
        }
        if let Some(t) = b.nodes.get_mut(*target as usize) {
            t.template_contents = frag;
        }
        frag
    }

    fn same_node(&self, x: &u32, y: &u32) -> bool {
        x == y
    }

    fn set_quirks_mode(&self, mode: QuirksMode) {
        self.inner.borrow_mut().quirks = mode;
    }

    fn append_before_sibling(&self, sibling: &u32, new_node: NodeOrText<u32>) {
        let mut b = self.inner.borrow_mut();
        match new_node {
            NodeOrText::AppendNode(id) => b.link_before(*sibling, id),
            NodeOrText::AppendText(text) => self.insert_text_before(&mut b, *sibling, &text),
        }
    }

    fn add_attrs_if_missing(&self, target: &u32, attrs: Vec<Attribute>) {
        // Re-classify: a late `aria-hidden` or `hidden` still has to take effect.
        let mut b = self.inner.borrow_mut();
        let tag = b.nodes.get(*target as usize).map_or(TagId::UNKNOWN, |n| n.tag);
        let (role, hidden) = Build::classify(tag, &attrs);
        if let Some(n) = b.nodes.get_mut(*target as usize) {
            n.own_role = TextRole::inherit(n.own_role, role);
            n.hidden_reason = n.hidden_reason.or(hidden);
        }
    }

    fn remove_from_parent(&self, target: &u32) {
        self.inner.borrow_mut().unlink(*target);
    }

    fn reparent_children(&self, node: &u32, new_parent: &u32) {
        let mut b = self.inner.borrow_mut();
        let max_depth = self.limits.max_depth;
        loop {
            let first = b.nodes.get(*node as usize).map_or(NONE, |n| n.first_child);
            if first == NONE {
                break;
            }
            b.link_last(*new_parent, first, max_depth);
        }
    }
}

impl BuildArena {
    /// Append text to `parent`, merging into its last child when that child is already text.
    ///
    /// # Why merging is not optional
    ///
    /// This used to allocate a fresh node per `AppendText` call, with a comment claiming it was
    /// "equivalent for every statistic we compute". It is not, and the consequence was that
    /// **guarantee S3 did not hold on 123 of the 130 corpus pages.**
    ///
    /// html5ever gates its data-state scanner on `target_arch`. On x86_64 and aarch64 a SIMD fast
    /// path returns one long character run spanning newlines; on `wasm32` the scalar fallback breaks
    /// the run at every `\n` and emits each newline as its own token. Identical documents therefore
    /// arrived as different token streams — and with a node per token, the token stream *became* the
    /// arena shape. [`Arena::accumulate_subtrees`] zeroes a text node only when the whole node is
    /// whitespace, so the same document scored a different `prose_len` per target, and every ratio
    /// built on it followed: `ars-1` reported a different headline, `lwn-1` selected a different
    /// region, `iab-1` lost its byline.
    ///
    /// Merging makes the arena a function of the document rather than of how the tokenizer happened
    /// to slice it, which is what a `TreeSink` is expected to do and what the fix had to be.
    ///
    /// The existing gate could not see any of this: it compares one hand-written fixture that is a
    /// single line, and a document with no newline inside a text run is precisely the shape that
    /// cannot diverge.
    fn append_text(&self, b: &mut std::cell::RefMut<'_, Build>, parent: u32, text: &str) {
        let last = b.nodes.get(parent as usize).map_or(NONE, |n| n.last_child);
        if last != NONE && self.extend_text(b, last, text) {
            return;
        }
        let id = self.new_text(b, text);
        b.link_last(parent, id, self.limits.max_depth);
    }

    /// Insert text before `sibling`, merging into `sibling`'s previous node when that is text.
    fn insert_text_before(&self, b: &mut std::cell::RefMut<'_, Build>, sibling: u32, text: &str) {
        let prev = b.nodes.get(sibling as usize).map_or(NONE, |n| n.prev_sib);
        if prev != NONE && self.extend_text(b, prev, text) {
            return;
        }
        let id = self.new_text(b, text);
        b.link_before(sibling, id);
    }

    /// Extend an existing text node with `text`, or report that it cannot be done.
    ///
    /// Refused unless the node is text whose bytes end exactly at the tip of `doc_buf`, because the
    /// span has to stay a single contiguous range — the verbatim invariant (plan §1.4) is stated
    /// over `doc_buf[span]` and a stitched-together range would not be one. In practice text arrives
    /// in a run, so the tip is where it is; when it is not, a fresh node is correct and merely
    /// misses one merge.
    fn extend_text(&self, b: &mut std::cell::RefMut<'_, Build>, id: u32, text: &str) -> bool {
        let Some(n) = b.nodes.get(id as usize) else { return false };
        if n.kind != NodeKind::Text || n.text.1 as usize != b.doc_buf.len() {
            return false;
        }
        let (_, end) = b.push_text(text, &self.limits);
        let start = match b.nodes.get_mut(id as usize) {
            Some(n) => {
                n.text.1 = end;
                n.text.0
            }
            None => return false,
        };
        // The icon-glyph signal is a property of the whole node, so it has to be re-decided against
        // the merged text: a private-use run followed by a real word is not an icon.
        let pua = is_private_use_only(b.doc_buf.get(start as usize..end as usize).unwrap_or(""));
        if let Some(n) = b.nodes.get_mut(id as usize) {
            n.own_role = if pua { TextRole::Control } else { TextRole::Prose };
        }
        true
    }

    /// Create a fresh text node.
    fn new_text(&self, b: &mut std::cell::RefMut<'_, Build>, text: &str) -> u32 {
        let id = b.new_node(NodeKind::Text, (*EMPTY_NAME).clone(), &self.limits);
        let range = b.push_text(text, &self.limits);
        if let Some(n) = b.nodes.get_mut(id as usize) {
            n.text = range;
            // Icon-font glyphs are text nodes made entirely of private-use codepoints. This is
            // the cheapest high-precision icon signal there is, and it catches Font Awesome
            // where a class-name heuristic would not.
            if is_private_use_only(text) {
                n.own_role = TextRole::Control;
            }
        }
        id
    }
}

/// A single shared empty name for nodes that have none (text, comments, doctype).
///
/// `QualName` is not `const`-constructible, hence a lazily-initialized static. Borrowing it as
/// `&'static QualName` is what lets `elem_name` return a `Ref` without a fallible index.
static EMPTY_NAME: std::sync::LazyLock<QualName> =
    std::sync::LazyLock::new(|| QualName::new(None, ns!(), local_name!("")));

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn parse(html: &str) -> (Arena, LimitsHit) {
        BuildArena::parse_to_arena(html, Limits::DEFAULT)
    }

    fn prose_of(arena: &Arena, n: usize) -> u32 {
        arena.prose_len.get(n).copied().unwrap_or(0)
    }

    #[test]
    fn document_order_and_subtree_containment_hold() {
        let (a, _) = parse("<html><body><div id=o><p>one</p><p>two</p></div></body></html>");
        // Invariant 1: pre-order. Invariant 2: descendants are a contiguous slice.
        for i in 0..a.len() {
            let end = a.subtree_end[i] as usize;
            assert!(end > i, "subtree_end must be exclusive-after for node {i}");
            assert!(end <= a.len());
            for d in (i + 1)..end {
                // Every node in the range must have an ancestor chain reaching i.
                let mut cur = a.parent[d];
                let mut found = false;
                let mut guard = 0;
                while cur.is_some() && guard < 1000 {
                    if cur.idx() == i {
                        found = true;
                        break;
                    }
                    cur = a.parent[cur.idx()];
                    guard += 1;
                }
                assert!(found, "node {d} is inside subtree of {i} but not a descendant");
            }
        }
    }

    #[test]
    fn subtree_sums_fold_in_one_reverse_pass() {
        let (a, _) = parse("<html><body><div><p>abcd</p><p>ef</p></div></body></html>");
        // The div must hold the sum of both paragraphs' prose.
        let div = a.tag.iter().position(|&t| t == TagId::DIV).unwrap();
        assert_eq!(prose_of(&a, div), 6, "abcd + ef");
    }

    #[test]
    fn script_and_style_text_is_inert_not_hidden() {
        // This test used to assert `hidden_len[body] > 0`, which was asserting a bug. Script and
        // stylesheet source landing in `hidden_len` put it into purity's denominator, so a
        // container holding an inlined bundle looked like a container padded with hidden text.
        // Reddit inlines a ~10 KB module script inside <main>: purity 0.04, <main> disqualified.
        let (a, _) = parse(
            "<html><head><style>.x{color:red}</style></head>\
             <body><p>real</p><script>var evil='aaaaaaaaaaaaaaaa';</script></body></html>",
        );
        let body = a.tag.iter().position(|&t| t == TagId::BODY).unwrap();
        assert_eq!(prose_of(&a, body), 4, "only `real` counts; script text must not");
        assert!(a.inert_len[body] > 0, "script text is accounted, in its own column");
        assert_eq!(a.hidden_len[body], 0, "script text is not hidden *text*");

        // The property that actually matters: purity is unmoved by inlined script bytes.
        let purity = |i: usize| {
            let p = a.prose_len[i] as f32;
            p / (p + a.control_len[i] as f32 + a.hidden_len[i] as f32 + a.alt_len[i] as f32)
        };
        assert!(
            (purity(body) - 1.0).abs() < f32::EPSILON,
            "a body of pure prose plus a script is pure prose, got {}",
            purity(body)
        );
    }

    #[test]
    fn copy_code_button_label_is_not_prose() {
        // The motivating case for defect a11y: the label is two levels below the <button>.
        let (a, _) = parse(
            "<html><body><article><p>body text</p>\
             <button class=copy><span class=icon>content_copy</span><span>Copy</span></button>\
             </article></body></html>",
        );
        let art = a.tag.iter().position(|&t| t == TagId::ARTICLE).unwrap();
        assert_eq!(prose_of(&a, art), 9, "`body text` only");
        assert!(a.control_len[art] > 0, "button labels are counted as control text");
    }

    #[test]
    fn aria_hidden_and_sr_only_are_separated_from_prose() {
        let (a, _) = parse(
            "<html><body><div id=w>\
             <span aria-hidden=true>\u{f0c5}</span>\
             <span class=sr-only>Skip to content</span>\
             <p>visible</p></div></body></html>",
        );
        let w = a.tag.iter().position(|&t| t == TagId::DIV).unwrap();
        assert_eq!(prose_of(&a, w), 7, "`visible` only");
        assert!(a.alt_len[w] > 0, "sr-only text is preserved as AltOnly, not deleted");
        assert!(a.hidden_len[w] > 0, "aria-hidden subtree is hidden");
    }

    #[test]
    fn role_presentation_does_not_hide_text() {
        // The easy mistake. A table laid out with role=presentation is still full of content;
        // treating the role as a hiding signal silently deletes it.
        let (a, _) = parse(
            "<html><body><table role=presentation><tr><td>real content here</td></tr></table></body></html>",
        );
        let td = a.tag.iter().position(|&t| t == TagId::TD).unwrap();
        assert_eq!(prose_of(&a, td), 17, "role=presentation must not hide text");
    }

    #[test]
    fn closed_details_content_survives() {
        let (a, _) = parse("<html><body><details><summary>Title</summary><p>hidden body</p></details></body></html>");
        let det = a.tag.iter().position(|&t| t == TagId::DETAILS).unwrap();
        // Both the summary heading and the collapsed body are real content.
        assert_eq!(prose_of(&a, det), 5 + 11);
    }

    #[test]
    fn hidden_until_found_is_not_hidden() {
        let (a, _) =
            parse("<html><body><div hidden=until-found><p>findable</p></div></body></html>");
        let d = a.tag.iter().position(|&t| t == TagId::DIV).unwrap();
        assert_eq!(prose_of(&a, d), 8, "until-found content is ordinary collapsed content");
    }

    #[test]
    fn link_density_uses_prose_only() {
        let (a, _) = parse("<html><body><div><a href=#>link</a><p>text</p></div></body></html>");
        let d = a.tag.iter().position(|&t| t == TagId::DIV).unwrap();
        let density = a.link_density(NodeId(d as u32));
        assert!((density - 0.5).abs() < 1e-6, "4 link bytes of 8 prose bytes, got {density}");
    }

    #[test]
    fn deeply_nested_markup_does_not_overflow_and_reports_the_cap() {
        // 5000 levels: well past max_depth, and a stack overflow here would be unrecoverable.
        let deep = "<div>".repeat(5000);
        let (a, hit) = parse(&format!("<html><body>{deep}<p>x</p></body></html>"));
        assert!(hit.depth, "the depth cap must be reported, not silently applied");
        assert!(!a.is_empty());
    }

    #[test]
    fn malformed_markup_still_yields_a_usable_arena() {
        for html in [
            "",
            "<p>unclosed",
            "<b><i>mis</b>nested</i>",
            "<table><td>foster</td></table>",
            "<!DOCTYPE html><html><body><p>ok</p>",
            "<div><div><div>",
            "&amp;&#x1F600;&notanentity;",
        ] {
            let (a, _) = parse(html);
            // The contract is "never panic, always return something walkable".
            for i in 0..a.len() {
                assert!(a.subtree_end[i] as usize <= a.len());
            }
        }
    }

    /// Placeholder evidence for every `<div>`, in document order.
    fn div_scores(a: &Arena) -> Vec<f32> {
        (0..a.len())
            .filter(|&i| a.tag.get(i).copied() == Some(TagId::DIV))
            .map(|i| a.placeholder_evidence(NodeId(i as u32)))
            .collect()
    }

    #[test]
    fn scale_invariance_is_about_the_decision_not_the_value() {
        // An earlier version of this test asserted that text_density is numerically unchanged
        // when text is duplicated. That is false, and usefully so: tripling the text inside a
        // structure triples prose_len while element_count is unchanged, so the value triples.
        //
        // The property that actually matters for defect 1 — and the one the plan's M6 gate
        // states — is that the *decision* is invariant: scaling all text by the same factor
        // must not change which region wins. Readability fails this because its score contains
        // an absolute `min(floor(len/100), 3)` term that saturates, so a long boilerplate block
        // and a short article block can swap places purely on length.
        let mk = |unit: &str| {
            format!(
                "<html><body>\
                 <div id=prose><p>{unit}</p><p>{unit}</p></div>\
                 <div id=links><a href=#>{unit}</a><a href=#>{unit}</a><a href=#>{unit}</a>\
                 <a href=#>{unit}</a><a href=#>{unit}</a><a href=#>{unit}</a></div>\
                 </body></html>"
            )
        };

        let (a1, _) = parse(&mk("alpha beta"));
        let (a5, _) = parse(&mk(&"alpha beta ".repeat(5)));

        let d1 = div_scores(&a1);
        let d5 = div_scores(&a5);
        assert_eq!(d1.len(), 2, "expected exactly two candidate divs");
        assert_eq!(d5.len(), 2);

        // The winner is the same region at both scales.
        let win1 = usize::from(d1[1] > d1[0]);
        let win5 = usize::from(d5[1] > d5[0]);
        assert_eq!(win1, win5, "argmax moved when text was scaled: {d1:?} vs {d5:?}");
        assert_eq!(win1, 0, "the prose-dense region must win, not the link-dense one");
    }

    #[test]
    fn scores_of_competing_prose_regions_scale_by_the_same_factor() {
        // Argmax stability is only meaningful if it is structural rather than lucky. With two
        // regions that both score above zero, uniform text scaling must multiply both scores by
        // the same factor -- that is what makes the ordering scale-free rather than coincidental.
        //
        // Two prose regions with deliberately different densities: one paragraph of text versus
        // the same text spread over four paragraphs (more tags, same characters).
        let mk = |unit: &str| {
            format!(
                "<html><body>\
                 <div id=dense><p>{unit}{unit}{unit}{unit}</p></div>\
                 <div id=sparse><p>{unit}</p><p>{unit}</p><p>{unit}</p><p>{unit}</p></div>\
                 </body></html>"
            )
        };
        let (a1, _) = parse(&mk("alpha beta "));
        let (a5, _) = parse(&mk(&"alpha beta ".repeat(5)));

        let d1 = div_scores(&a1);
        let d5 = div_scores(&a5);
        assert!(d1.iter().all(|s| *s > 0.0), "both regions must score above zero: {d1:?}");

        let f0 = d5[0] / d1[0];
        let f1 = d5[1] / d1[1];
        assert!(
            (f0 - f1).abs() < 1e-3,
            "scores did not scale uniformly: factors {f0} vs {f1} ({d1:?} -> {d5:?})"
        );
        assert!(d1[0] > d1[1], "the denser region must win at 1x: {d1:?}");
        assert!(d5[0] > d5[1], "and still at 5x: {d5:?}");
    }

    #[test]
    fn a_pure_link_block_scores_zero() {
        // link_density == 1.0 collapses the placeholder score to exactly zero, which is why the
        // ratio between an article and a nav block is not a usable comparison -- the nav block is
        // not merely worse, it is out of the running.
        let (a, _) = parse(
            "<html><body><div id=nav><a href=#>one</a><a href=#>two</a></div>\
             <div id=art><p>real prose here</p></div></body></html>",
        );
        let d = div_scores(&a);
        assert_eq!(d.len(), 2);
        assert!((d[0] - 0.0).abs() < f32::EPSILON, "a pure link block must score 0, got {}", d[0]);
        assert!(d[1] > 0.0, "the article must score above zero, got {}", d[1]);
    }

    #[test]
    fn no_decision_reads_an_absolute_character_count() {
        // The structural half of defect 1: a region that is objectively better must stay better
        // when the whole document shrinks below Readability's 500-character threshold, which is
        // the exact point where its retry ladder starts disabling flags and eventually falls
        // back to <body>.
        let tiny = "<html><body><div id=prose><p>ok</p></div>\
                    <div id=nav><a href=#>a</a><a href=#>b</a></div></body></html>";
        let (a, _) = parse(tiny);
        let d = div_scores(&a);
        assert_eq!(d.len(), 2);
        assert!(d[0] > d[1], "a 2-character article must still beat a nav block: {d:?}");
    }
}
