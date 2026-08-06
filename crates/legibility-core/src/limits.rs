//! Resource limits (guarantee S2).
//!
//! Two properties make this more than a config struct.
//!
//! **Every limit degrades; none of them fails.** Hitting a limit produces a valid, smaller
//! [`crate::Extraction`] with the corresponding flag set in [`LimitsHit`]. There is no `Err`
//! path from a limit. A caller that hits `max_nodes` on a hostile page gets the first
//! `max_nodes` worth of document, not an error it has to decide what to do with.
//!
//! **The values are per-host constants, never computed at call time.** An adaptive cap —
//! "use 25% of available memory" — makes the output depend on machine state, which destroys
//! S3 (byte-identical output for identical input). So each embedding commits to a constant:
//! see [`Limits::IOS_APP_EXTENSION`] for the tightest case, where an iOS Share Extension has
//! far less headroom than its host app.

/// Bounds on the work a single extraction may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Reject input larger than this. Degradation: truncate at a UTF-8 boundary and set
    /// [`LimitsHit::input_bytes`]; extraction proceeds on the prefix.
    pub max_input_bytes: u32,
    /// Stop building the arena past this many nodes. Degradation: the remainder of the
    /// document is not parsed.
    pub max_nodes: u32,
    /// Nodes deeper than this become [`crate::NodeKind::DepthCappedOrphan`] — allocated and
    /// recorded, never dropped. See that variant for why dropping corrupts the arena.
    pub max_depth: u16,
    /// Attributes retained per element. Degradation: extra attributes are ignored.
    pub max_attrs_per_node: u16,
    /// Bytes retained per attribute value. Degradation: the value is truncated.
    pub max_attr_bytes: u32,
    /// Comment items collected. Degradation: reported through the completeness record as
    /// `TruncationReason::LimitHit`, so a caller cannot mistake our cap for the site's own
    /// pagination.
    pub max_comment_items: u32,
    /// Cap on serialized output. Degradation: cut at an **element boundary**, never mid-tag,
    /// and report the truncation. Cutting mid-tag would emit HTML that reparses differently,
    /// which is a sanitizer bypass waiting to happen.
    pub max_output_bytes: u32,
    /// Coarse work budget, counted in arena operations rather than time.
    ///
    /// Deliberately not a wall-clock budget: a time budget makes output depend on machine
    /// speed and therefore breaks S3. A step budget is reproducible.
    pub step_budget: u64,
}

impl Limits {
    /// Default for CLI and server use, where memory is plentiful.
    pub const DEFAULT: Limits = Limits {
        max_input_bytes: 32 * 1024 * 1024,
        max_nodes: 10_000_000,
        max_depth: 512,
        max_attrs_per_node: 256,
        max_attr_bytes: 64 * 1024,
        max_comment_items: 100_000,
        max_output_bytes: 16 * 1024 * 1024,
        step_budget: 2_000_000_000,
    };

    /// Browser / WASM profile. Linear memory never shrinks once grown, so a page that briefly
    /// needs 200 MB keeps costing 200 MB for the lifetime of the instance.
    pub const BROWSER: Limits = Limits {
        max_input_bytes: 8 * 1024 * 1024,
        max_nodes: 2_000_000,
        max_output_bytes: 4 * 1024 * 1024,
        ..Limits::DEFAULT
    };

    /// iOS app-extension profile — the tightest embedding we target.
    ///
    /// A Share Extension is killed for memory far sooner than its host app, and being killed
    /// is indistinguishable to the user from a crash. Degrading to a partial article is
    /// strictly better than that, which is the whole argument for S2 returning degraded
    /// results instead of errors.
    pub const IOS_APP_EXTENSION: Limits = Limits {
        max_input_bytes: 4 * 1024 * 1024,
        max_nodes: 500_000,
        max_comment_items: 5_000,
        max_output_bytes: 2 * 1024 * 1024,
        ..Limits::DEFAULT
    };
}

impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Which limits were reached during an extraction.
///
/// Every field corresponds one-to-one with a field of [`Limits`] and with a documented
/// degradation. An M10 exit gate asserts that each has a test proving the degradation happens
/// and that the result is still a valid `Extraction`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LimitsHit {
    /// Input was truncated at a UTF-8 boundary.
    pub input_bytes: bool,
    /// Arena construction stopped early.
    pub nodes: bool,
    /// At least one node became a depth-capped orphan.
    pub depth: bool,
    /// At least one element had attributes dropped.
    pub attrs_per_node: bool,
    /// At least one attribute value was truncated.
    pub attr_bytes: bool,
    /// Comment collection stopped early.
    pub comment_items: bool,
    /// Output was cut at an element boundary.
    pub output_bytes: bool,
    /// The step budget was exhausted.
    pub step_budget: bool,
}

impl LimitsHit {
    /// Whether any limit was reached.
    #[must_use]
    pub const fn any(self) -> bool {
        self.input_bytes
            || self.nodes
            || self.depth
            || self.attrs_per_node
            || self.attr_bytes
            || self.comment_items
            || self.output_bytes
            || self.step_budget
    }
}

#[cfg(test)]
// Tests may unwrap and may assert on const-evaluable predicates: these are regression
// guards against someone changing a const fn, not runtime checks.
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::assertions_on_constants)]
mod tests {
    use super::*;

    #[test]
    fn profiles_are_ordered_by_tightness() {
        // The iOS extension profile must never be more permissive than the browser profile,
        // which must never exceed the default. A copy-paste error here shows up on a device
        // as an extension kill, which is the hardest failure to diagnose.
        assert!(Limits::IOS_APP_EXTENSION.max_input_bytes <= Limits::BROWSER.max_input_bytes);
        assert!(Limits::BROWSER.max_input_bytes <= Limits::DEFAULT.max_input_bytes);
        assert!(Limits::IOS_APP_EXTENSION.max_nodes <= Limits::BROWSER.max_nodes);
        assert!(Limits::BROWSER.max_nodes <= Limits::DEFAULT.max_nodes);
        assert!(Limits::IOS_APP_EXTENSION.max_output_bytes <= Limits::BROWSER.max_output_bytes);
    }

    #[test]
    fn depth_cap_is_uniform_across_profiles() {
        // max_depth is a correctness parameter, not a memory one: it exists so the iterative
        // walks have a bounded explicit stack. Varying it per host would make the same document
        // produce different trees on different platforms, breaking S3.
        assert_eq!(Limits::DEFAULT.max_depth, 512);
        assert_eq!(Limits::BROWSER.max_depth, 512);
        assert_eq!(Limits::IOS_APP_EXTENSION.max_depth, 512);
    }

    #[test]
    fn no_limits_hit_by_default() {
        assert!(!LimitsHit::default().any());
        assert!(LimitsHit { depth: true, ..LimitsHit::default() }.any());
    }
}
