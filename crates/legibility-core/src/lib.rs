//! `legibility-core` — the extraction engine.
//!
//! This crate is `no_std + alloc` on purpose, and the reason is not portability theatre:
//! it makes `HashMap`/`HashSet` *unreachable by type*, which is one of the two ways output
//! nondeterminism enters an extractor (the other is floating point, handled in [`num`]).
//! It also keeps the clock, filesystem and network out of the engine, so
//! "same input bytes produce the same output bytes, forever" (guarantee S3) is structural
//! rather than aspirational.
//!
//! It contains no HTML parser. `legibility-dom` owns the only dependency on html5ever and
//! feeds this crate a finished [`Arena`]. `cargo tree -p legibility-core` must not mention
//! html5ever — that is an M0 exit gate.
//!
//! # Shape of the pipeline
//!
//! ```text
//!   bytes ──[legibility-dom]──▶ build arena ──▶ flatten() ──▶ one reverse pass ──▶ features
//!                              (mutable,        (document      (subtree sums)
//!                               doubly-linked)   order + SoA)
//! ```
//!
//! Two phases are required rather than elegant. html5ever's `TreeSink` takes `&self` and
//! calls `remove_from_parent` / `reparent_children` / `append_before_sibling` while parsing
//! (the adoption agency algorithm and table foster parenting both do this), so **allocation
//! order is not document order**. A single-pass document-order arena is therefore impossible.
//! Phase 1 builds a mutable doubly-linked arena where those operations are O(1); phase 2
//! flattens it into a document-order column and records [`Arena::subtree_end`], after which
//! every subtree is a contiguous slice and all subtree sums fall out of one reverse loop.
//!
//! That last property is the whole performance story. Readability.js recomputes
//! `getInnerText` per candidate per ancestor, which is quadratic on deep documents.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod a11y;
pub mod arena;
pub mod comments;
pub mod groups;
pub mod limits;
pub mod meta;
pub mod num;
pub mod pipeline;
pub mod score;
pub mod shape;
pub mod tag;

pub use arena::{Arena, NodeId, NodeKind};
pub use limits::{Limits, LimitsHit};
pub use comments::{CommentItem, CommentSet, Completeness, DepthSource};
pub use groups::{find_groups, Group};
pub use pipeline::{run as extract_all, Outcome};
pub use meta::{Candidate as MetaCandidate, Metadata, Source as MetaSource};
pub use score::{select_article, Candidate, PageStats, Selection};
pub use shape::{DiscussionShape, Shape};
pub use tag::TagId;

/// Result of an extraction.
///
/// There is deliberately no `Err` variant reachable from a limit being hit. Every limit in
/// [`Limits`] has a documented degradation and still yields a valid `Extraction` with
/// [`Diagnostics::limits_hit`] populated (guarantee S2). A caller that ignores diagnostics
/// gets a smaller answer, never a failed one.
#[derive(Debug, Clone)]
pub struct Extraction {
    /// The chosen article region, if one was accepted.
    pub article: Option<Article>,
    /// Why no article was returned. Mutually exclusive with `article`.
    pub no_article: Option<NoArticle>,
    /// Machine-readable trace of how this result was reached.
    pub diagnostics: Diagnostics,
}

/// An accepted article region.
#[derive(Debug, Clone)]
pub struct Article {
    /// Sanitized HTML of the region. Sanitization is owned by `legibility-sanitize`, never
    /// left to the consumer — the output is injected into pages.
    pub html: alloc::string::String,
    /// Prose text of the region. Excludes `Control`, `Hidden` and `AltOnly` text
    /// (see [`a11y::TextRole`]).
    pub text: alloc::string::String,
    /// Quantized to `u16` over `[0, 10_000]`. Threshold comparisons use the quantized value
    /// so that they cannot disagree across targets. **Never calibrated** — see `calibrated`.
    pub confidence: u16,
    /// Always `false` in v1, and permanently part of the contract rather than a TODO.
    /// Calibrating a confidence requires a labelled corpus large enough to fit a mapping,
    /// and claiming calibration without one is worse than admitting its absence.
    pub calibrated: bool,
}

/// Why no article was produced. Enumerated rather than a bare `None`, because
/// "this page legitimately has no article" and "we failed" are different facts and a reader
/// UI must be able to tell them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoArticle {
    /// A listing/index page. Not a failure.
    IndexPage,
    /// A discussion page with neither a submission we could identify nor a body.
    ///
    /// Not the same as a link submission: one of those *is* returned, as an article whose
    /// kind is `DiscussionRoot` and whose payload is the title and the outbound URL. See
    /// [`shape::DiscussionShape::LinkOnly`] — throwing a pointer away would be worse than
    /// returning it.
    CommentsOnly,
    /// No text-bearing content at all.
    NoTextContent,
    /// A candidate existed but did not clear the accept bar.
    LowConfidence,
    /// Too few competing regions for page-relative statistics to mean anything.
    TooFewCompetitors,
}

/// Observability payload. Readability.js's opacity is precisely why nobody can fix it, so
/// the decision trace is a product surface here rather than a debug build feature.
#[derive(Debug, Clone, Default)]
pub struct Diagnostics {
    /// Which limits were reached, and thus which degradation applied.
    pub limits_hit: limits::LimitsHit,
    /// Set when a11y exclusion removed so much prose that it was partially reverted.
    pub a11y_exclusion_reverted: bool,
    /// Node count after flattening.
    pub node_count: u32,
    /// Total prose bytes in the document, before region selection.
    pub page_prose_len: u32,
}
