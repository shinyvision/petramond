//! The host-supplied dynamic state a document's bindings read.
//!
//! `UiState` is a flat string-keyed map plus change tracking: a per-state
//! `origin`, a revision counter, and the revision each key last changed at.
//! The runtime's frame cache keys on `(origin, revision)` — equal means
//! identical contents — and re-expands only the subtrees that read a key
//! [`changed_since`](UiState::changed_since) its cached revision. List-bound
//! widgets read a `UiValue::List` of per-item maps; inside a list template,
//! bindings resolve against the item map first and fall back to the global
//! map.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// One bound value. `List` items are maps so row templates can bind several
/// fields (name, version, enabled…) from one item.
#[derive(Clone, Debug, PartialEq)]
pub enum UiValue {
    F32(f32),
    I32(i32),
    Bool(bool),
    Str(String),
    List(Arc<Vec<UiMap>>),
}

impl UiValue {
    /// Whether replacing `self` with `other` changes nothing. A list is
    /// first compared by pointer, so a host republishing the same `Arc`
    /// every frame never pays for a deep row-by-row comparison.
    fn same_as(&self, other: &UiValue) -> bool {
        match (self, other) {
            (UiValue::List(a), UiValue::List(b)) => Arc::ptr_eq(a, b) || a == b,
            _ => self == other,
        }
    }
}

/// A string-keyed value map. `BTreeMap` for deterministic iteration.
pub type UiMap = BTreeMap<String, UiValue>;

/// Distinct origins: a state's revisions only compare against itself.
static NEXT_ORIGIN: AtomicU64 = AtomicU64::new(1);

/// The per-frame read model of everything a document binds. Mutate through the
/// setters so the revision advances.
#[derive(Debug)]
pub struct UiState {
    /// Each value with the revision it last changed at.
    values: BTreeMap<String, (UiValue, u64)>,
    /// Keys removed, with the revision they were removed at.
    removed: BTreeMap<String, u64>,
    revision: u64,
    origin: u64,
}

impl Default for UiState {
    fn default() -> UiState {
        UiState {
            values: BTreeMap::new(),
            removed: BTreeMap::new(),
            revision: 0,
            origin: NEXT_ORIGIN.fetch_add(1, Ordering::Relaxed),
        }
    }
}

/// A clone is a NEW origin: it can diverge from its source at matching
/// revision numbers, so a cache must never mistake one for the other.
impl Clone for UiState {
    fn clone(&self) -> UiState {
        UiState {
            values: self.values.clone(),
            removed: self.removed.clone(),
            revision: self.revision,
            origin: NEXT_ORIGIN.fetch_add(1, Ordering::Relaxed),
        }
    }
}

impl UiState {
    pub fn new() -> UiState {
        UiState::default()
    }

    /// Monotonic change counter: within one [`origin`](Self::origin), equal
    /// revisions mean identical contents.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// This state's identity for revision comparisons (unique per state and
    /// per clone).
    pub fn origin(&self) -> u64 {
        self.origin
    }

    /// Every key set, changed or removed after `revision`.
    pub fn changed_since(&self, revision: u64) -> impl Iterator<Item = &str> {
        let set = self
            .values
            .iter()
            .filter(move |(_, (_, at))| *at > revision)
            .map(|(k, _)| k.as_str());
        let removed = self
            .removed
            .iter()
            .filter(move |(_, at)| **at > revision)
            .map(|(k, _)| k.as_str());
        set.chain(removed)
    }

    pub fn set(&mut self, key: impl Into<String>, value: UiValue) {
        let key = key.into();
        if self
            .values
            .get(&key)
            .is_some_and(|(old, _)| old.same_as(&value))
        {
            return;
        }
        self.revision += 1;
        self.removed.remove(&key);
        self.values.insert(key, (value, self.revision));
    }

    pub fn remove(&mut self, key: &str) {
        if let Some((key, _)) = self.values.remove_entry(key) {
            self.revision += 1;
            self.removed.insert(key, self.revision);
        }
    }

    pub fn clear(&mut self) {
        if !self.values.is_empty() {
            self.revision += 1;
            let revision = self.revision;
            let values = std::mem::take(&mut self.values);
            self.removed
                .extend(values.into_keys().map(|key| (key, revision)));
        }
    }

    pub fn get(&self, key: &str) -> Option<&UiValue> {
        self.values.get(key).map(|(value, _)| value)
    }

    /// Resolve `key` against an optional list-item map first, then the global
    /// map — the template-binding rule.
    pub fn resolve<'a>(&'a self, item: Option<&'a UiMap>, key: &str) -> Option<&'a UiValue> {
        item.and_then(|m| m.get(key)).or_else(|| self.get(key))
    }

    pub fn get_str(&self, key: &str) -> Option<&str> {
        match self.get(key) {
            Some(UiValue::Str(s)) => Some(s),
            _ => None,
        }
    }

    pub fn get_f32(&self, key: &str) -> Option<f32> {
        match self.get(key) {
            Some(UiValue::F32(v)) => Some(*v),
            Some(UiValue::I32(v)) => Some(*v as f32),
            _ => None,
        }
    }

    pub fn get_i32(&self, key: &str) -> Option<i32> {
        match self.get(key) {
            Some(UiValue::I32(v)) => Some(*v),
            _ => None,
        }
    }

    pub fn get_bool(&self, key: &str) -> Option<bool> {
        match self.get(key) {
            Some(UiValue::Bool(v)) => Some(*v),
            _ => None,
        }
    }

    pub fn get_list(&self, key: &str) -> Option<&Arc<Vec<UiMap>>> {
        match self.get(key) {
            Some(UiValue::List(items)) => Some(items),
            _ => None,
        }
    }
}

/// Coercions bindings apply at read time (widgets want one shape per binding).
impl UiValue {
    pub fn as_display_text(&self) -> Option<String> {
        match self {
            UiValue::Str(s) => Some(s.clone()),
            UiValue::I32(v) => Some(v.to_string()),
            UiValue::F32(v) => Some(format!("{v}")),
            UiValue::Bool(_) | UiValue::List(_) => None,
        }
    }

    pub fn as_f32(&self) -> Option<f32> {
        match self {
            UiValue::F32(v) => Some(*v),
            UiValue::I32(v) => Some(*v as f32),
            _ => None,
        }
    }

    /// A number counts as a bool, and that is what lets a value published from
    /// outside the UI drive `visible`, `enabled` and a checkbox/toggle's face.
    /// Without it those bindings silently resolve to their default (`true`)
    /// and the node just never changes. A STRING counts too — non-empty is
    /// true — so one published value (a tooltip's text) can be both the
    /// content and the condition, with no parallel `has_*` key.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            UiValue::Bool(v) => Some(*v),
            UiValue::I32(v) => Some(*v != 0),
            UiValue::F32(v) => Some(*v != 0.0),
            UiValue::Str(s) => Some(!s.is_empty()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revision_advances_only_on_change() {
        let mut s = UiState::new();
        let r0 = s.revision();
        s.set("a", UiValue::I32(1));
        let r1 = s.revision();
        assert!(r1 > r0);
        s.set("a", UiValue::I32(1)); // same value: no change
        assert_eq!(s.revision(), r1);
        s.set("a", UiValue::I32(2));
        assert!(s.revision() > r1);
        s.remove("missing");
        let r3 = s.revision();
        s.remove("a");
        assert!(s.revision() > r3);
    }

    #[test]
    fn changed_since_names_exactly_the_keys_touched_after_a_revision() {
        let mut s = UiState::new();
        s.set("a", UiValue::I32(1));
        s.set("b", UiValue::I32(1));
        let mark = s.revision();
        assert_eq!(s.changed_since(mark).count(), 0);
        s.set("b", UiValue::I32(1)); // unchanged value: not a change
        s.set("c", UiValue::Bool(true));
        s.remove("a");
        let mut changed: Vec<&str> = s.changed_since(mark).collect();
        changed.sort_unstable();
        assert_eq!(changed, ["a", "c"]);
        // Re-setting a removed key is a change again, and it stops being
        // reported as removed.
        let mark = s.revision();
        s.set("a", UiValue::I32(5));
        assert_eq!(s.changed_since(mark).collect::<Vec<_>>(), ["a"]);
        let mark = s.revision();
        s.clear();
        let mut changed: Vec<&str> = s.changed_since(mark).collect();
        changed.sort_unstable();
        assert_eq!(changed, ["a", "b", "c"]);
    }

    #[test]
    fn a_republished_list_arc_is_not_a_change_and_clones_are_new_origins() {
        let rows = Arc::new(vec![UiMap::new()]);
        let mut s = UiState::new();
        s.set("rows", UiValue::List(rows.clone()));
        let r = s.revision();
        s.set("rows", UiValue::List(rows));
        assert_eq!(s.revision(), r);
        let copy = s.clone();
        assert_ne!(copy.origin(), s.origin());
        assert_eq!(copy.get("rows"), s.get("rows"));
    }

    #[test]
    fn resolve_prefers_item_map() {
        let mut s = UiState::new();
        s.set("name", UiValue::Str("global".into()));
        let mut item = UiMap::new();
        item.insert("name".into(), UiValue::Str("row".into()));
        assert_eq!(
            s.resolve(Some(&item), "name"),
            Some(&UiValue::Str("row".into()))
        );
        assert_eq!(
            s.resolve(None, "name"),
            Some(&UiValue::Str("global".into()))
        );
        assert_eq!(s.resolve(Some(&item), "absent"), None);
    }
}
