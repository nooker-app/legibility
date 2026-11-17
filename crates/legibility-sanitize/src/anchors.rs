//! Region-local fragment targets. Source identifiers never become output identifiers.

use std::collections::BTreeMap;

use super::{PhantomData, Profile};

/// Maps surviving elements to unique, deterministic IDs in the profile's namespace.
///
/// Register only elements actually emitted, in tree order, then resolve links (including forward
/// references). IDs are numbered rather than filtered: filtering Unicode or punctuation merges
/// distinct targets. Even already-prefixed source IDs are untrusted input. Numbering also makes
/// repeated sanitization stable and keeps Article's `lg-<number>` disjoint from `UserContent`'s
/// `lg-c-<number>`. The host must reserve these namespaces and isolate separately rendered regions.
///
/// `id` / `name` deliberately stay off the ordinary attribute allowlist. A legacy `<a name>` is
/// registered as an alias for its element's generated ID; no `name` attribute is emitted.
#[derive(Debug)]
pub struct FragmentTargets<P: Profile> {
    ids: BTreeMap<String, usize>,
    names: BTreeMap<String, usize>,
    count: usize,
    _profile: PhantomData<P>,
}

impl<P: Profile> Default for FragmentTargets<P> {
    fn default() -> Self {
        Self { ids: BTreeMap::new(), names: BTreeMap::new(), count: 0, _profile: PhantomData }
    }
}

impl<P: Profile> FragmentTargets<P> {
    /// Register one emitted element's ID and, for `<a>` only, legacy name.
    ///
    /// Empty identifiers are not targets. Duplicate source identifiers resolve to the first
    /// element, but every emitted element receives a different output ID.
    pub fn register(&mut self, id: Option<&str>, name: Option<&str>) -> Option<String> {
        let id = id.filter(|s| !s.is_empty());
        let name = name.filter(|s| !s.is_empty());
        if id.is_none() && name.is_none() {
            return None;
        }
        self.count += 1;
        if let Some(id) = id {
            self.ids.entry(id.to_owned()).or_insert(self.count);
        }
        if let Some(name) = name {
            self.names.entry(name.to_owned()).or_insert(self.count);
        }
        Some(format!("{}{}", P::ID_PREFIX, self.count))
    }

    /// Resolve a fragment (without `#`) after every surviving target has been registered.
    ///
    /// HTML's indicated-part algorithm checks the literal fragment before percent-decoding once,
    /// and IDs before legacy names at each step. Invalid percent escapes stay literal; invalid
    /// UTF-8 uses replacement characters. `+` is not form-URL-decoded to a space.
    /// See <https://html.spec.whatwg.org/multipage/browsing-the-web.html#the-indicated-part-of-the-document>.
    /// Empty / unmatched `top` fragments become `#`; other unresolved links lose their href so
    /// they cannot accidentally address the host document or a newly generated identifier.
    #[must_use]
    pub fn resolve(&self, fragment: &str) -> Option<String> {
        if fragment.is_empty() {
            return Some("#".to_owned());
        }
        let decoded = decode_fragment(fragment);
        let target = self.lookup(fragment).or_else(|| self.lookup(&decoded));
        target
            .map(|n| format!("#{}{n}", P::ID_PREFIX))
            .or_else(|| decoded.eq_ignore_ascii_case("top").then(|| "#".to_owned()))
    }

    fn lookup(&self, fragment: &str) -> Option<usize> {
        self.ids.get(fragment).or_else(|| self.names.get(fragment)).copied()
    }
}

fn decode_fragment(fragment: &str) -> String {
    let bytes = fragment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(a), Some(b)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(a * 16 + b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Article, UserContent};

    #[test]
    fn unicode_colons_and_literal_percent_resolve_without_collisions() {
        let mut targets = FragmentTargets::<Article>::default();
        for id in ["한글", "fn:1", "fn1", "100%", "%ed%95%9c", "한", "a+b", "a b", "%FF", "�"]
        {
            targets.register(Some(id), None);
        }
        for (fragment, n) in [
            ("%ED%95%9C%EA%B8%80", 1),
            ("한글", 1),
            ("fn%3a1", 2),
            ("fn1", 3),
            ("100%25", 4),
            ("100%", 4),
            ("%ed%95%9c", 5),
            ("%25ed%2595%259c", 5),
            ("%ED%95%9C", 6),
            ("a+b", 7),
            ("a%20b", 8),
            ("%FF", 9),
            ("%ff", 10),
        ] {
            assert_eq!(targets.resolve(fragment), Some(format!("#lg-{n}")), "{fragment}");
        }
    }

    #[test]
    fn duplicate_and_already_prefixed_ids_are_untrusted_and_unique() {
        let mut targets = FragmentTargets::<Article>::default();
        for (i, id) in ["x", "x", "lg-1", "lg-c-1", "attributes", "a\"><script>"].iter().enumerate()
        {
            assert_eq!(targets.register(Some(id), None), Some(format!("lg-{}", i + 1)));
        }
        assert_eq!(targets.resolve("x").as_deref(), Some("#lg-1"));
        assert_eq!(targets.resolve("lg-1").as_deref(), Some("#lg-3"));
        assert_eq!(targets.resolve("lg-c-1").as_deref(), Some("#lg-4"));
    }

    #[test]
    fn ids_take_precedence_over_legacy_names_in_tree_order() {
        let mut targets = FragmentTargets::<Article>::default();
        targets.register(None, Some("same"));
        targets.register(Some("same"), Some("alias"));
        targets.register(Some("same"), None);
        assert_eq!(targets.resolve("same").as_deref(), Some("#lg-2"));
        assert_eq!(targets.resolve("alias").as_deref(), Some("#lg-2"));
    }

    #[test]
    fn profiles_have_disjoint_namespaces() {
        let mut article = FragmentTargets::<Article>::default();
        let mut comment = FragmentTargets::<UserContent>::default();
        assert_eq!(article.register(Some("c-1"), None).as_deref(), Some("lg-1"));
        assert_eq!(comment.register(Some("1"), None).as_deref(), Some("lg-c-1"));
    }

    #[test]
    fn unresolved_fragments_cannot_bind_to_generated_or_host_ids() {
        let mut targets = FragmentTargets::<Article>::default();
        assert_eq!(targets.register(Some(""), Some("")), None);
        targets.register(Some("x"), None);
        assert_eq!(targets.resolve("lg-1"), None);
        assert_eq!(targets.resolve("host"), None);
        for top in ["", "top", "TOP", "%74op"] {
            assert_eq!(targets.resolve(top).as_deref(), Some("#"));
        }
        targets.register(Some("top"), None);
        assert_eq!(targets.resolve("top").as_deref(), Some("#lg-2"));
    }
}
