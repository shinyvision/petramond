//! The host-supplied dynamic state a document's bindings read.
//!
//! `UiState` is a flat string-keyed map plus a revision counter (so hosts and
//! the layout cache can cheaply detect "nothing changed"). List-bound widgets
//! read a `UiValue::List` of per-item maps; inside a list template, bindings
//! resolve against the item map first and fall back to the global map.

use std::collections::BTreeMap;
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

/// A string-keyed value map. `BTreeMap` for deterministic iteration.
pub type UiMap = BTreeMap<String, UiValue>;

/// The per-frame read model of everything a document binds. Mutate through the
/// setters so the revision advances.
#[derive(Clone, Debug, Default)]
pub struct UiState {
    values: UiMap,
    revision: u64,
}

impl UiState {
    pub fn new() -> UiState {
        UiState::default()
    }

    /// Monotonic change counter: equal revisions mean identical contents.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn set(&mut self, key: impl Into<String>, value: UiValue) {
        let key = key.into();
        if self.values.get(&key) != Some(&value) {
            self.values.insert(key, value);
            self.revision += 1;
        }
    }

    pub fn remove(&mut self, key: &str) {
        if self.values.remove(key).is_some() {
            self.revision += 1;
        }
    }

    pub fn clear(&mut self) {
        if !self.values.is_empty() {
            self.values.clear();
            self.revision += 1;
        }
    }

    pub fn get(&self, key: &str) -> Option<&UiValue> {
        self.values.get(key)
    }

    /// Resolve `key` against an optional list-item map first, then the global
    /// map — the template-binding rule.
    pub fn resolve<'a>(&'a self, item: Option<&'a UiMap>, key: &str) -> Option<&'a UiValue> {
        item.and_then(|m| m.get(key)).or_else(|| self.values.get(key))
    }

    pub fn get_str(&self, key: &str) -> Option<&str> {
        match self.values.get(key) {
            Some(UiValue::Str(s)) => Some(s),
            _ => None,
        }
    }

    pub fn get_f32(&self, key: &str) -> Option<f32> {
        match self.values.get(key) {
            Some(UiValue::F32(v)) => Some(*v),
            Some(UiValue::I32(v)) => Some(*v as f32),
            _ => None,
        }
    }

    pub fn get_i32(&self, key: &str) -> Option<i32> {
        match self.values.get(key) {
            Some(UiValue::I32(v)) => Some(*v),
            _ => None,
        }
    }

    pub fn get_bool(&self, key: &str) -> Option<bool> {
        match self.values.get(key) {
            Some(UiValue::Bool(v)) => Some(*v),
            _ => None,
        }
    }

    pub fn get_list(&self, key: &str) -> Option<&Arc<Vec<UiMap>>> {
        match self.values.get(key) {
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

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            UiValue::Bool(v) => Some(*v),
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
