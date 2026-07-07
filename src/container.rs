//! Generic item-slot storage for MOD container blocks.
//!
//! A mod GUI document (kind `mod_id:name`) may declare `container` role slots;
//! the engine backs them with one `Container` per opening block position —
//! section-owned like [`crate::chest::Chest`]/[`crate::furnace::Furnace`], but
//! with no engine behavior at all: the engine stores, renders, click-routes,
//! persists, and scatters the slots; what the contents MEAN (cooking, burning,
//! filtering machines…) is the owning mod's tick logic, reached through the
//! `ContainerGet`/`ContainerSet` host calls.
//!
//! Slot SEMANTICS (which item groups shift-clicks route into a slot, which
//! slots are take-only outputs) are declared on the document's slot nodes and
//! resolved at document load — see `gui::documents` and [`SlotSpec`].

use crate::item::{ItemStack, ItemTag};

/// The most `container` role slots one mod document may declare (a double
/// chest's worth). Bounds both the click surface and the per-record save size.
pub const MAX_CONTAINER_SLOTS: usize = 54;

/// One mod container block-entity: a flat row-major slot list sized by the
/// owning GUI document when the session first opens (or grown by a mod write).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Container {
    pub slots: Vec<Option<ItemStack>>,
}

impl Container {
    /// An empty container with `len` slots.
    pub fn with_len(len: usize) -> Container {
        Container {
            slots: vec![None; len.min(MAX_CONTAINER_SLOTS)],
        }
    }

    /// Grow (never shrink) to at least `len` slots, clamped to the cap —
    /// a re-authored document with more slots must not drop stored items.
    pub fn ensure_len(&mut self, len: usize) {
        let len = len.min(MAX_CONTAINER_SLOTS);
        if self.slots.len() < len {
            self.slots.resize(len, None);
        }
    }
}

/// One document slot's host-interpreted semantics, resolved from the document's
/// `accepts` tag names / `take_only` flag at load. In-role index order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SlotSpec {
    /// Item tags shift-clicks may route into this slot; empty = accepts any
    /// item on shift-routing (a plain storage cell).
    pub accepts: Vec<ItemTag>,
    /// A take-only output: clicks only ever remove from it, and shift-routing
    /// never targets it.
    pub take_only: bool,
}

impl SlotSpec {
    /// Whether shift-routing may move `item` into this slot: never for an
    /// output, tag-matched when filters are declared, always otherwise.
    pub fn routes(&self, item: crate::item::ItemType) -> bool {
        if self.take_only {
            return false;
        }
        self.accepts.is_empty() || self.accepts.iter().any(|&t| item.has_tag(t))
    }

    /// Whether a shift-route of `item` should PREFER this slot: it names a
    /// matching filter (the furnace's fuel→fuel-slot read), beating unfiltered
    /// storage cells.
    pub fn routes_by_filter(&self, item: crate::item::ItemType) -> bool {
        !self.take_only && self.accepts.iter().any(|&t| item.has_tag(t))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::ItemType;

    #[test]
    fn slot_specs_route_by_tag_and_never_into_outputs() {
        let fuel_slot = SlotSpec {
            accepts: vec![ItemTag::FUEL],
            take_only: false,
        };
        let output = SlotSpec {
            accepts: Vec::new(),
            take_only: true,
        };
        let open = SlotSpec::default();
        assert!(fuel_slot.routes(ItemType::Coal));
        assert!(fuel_slot.routes_by_filter(ItemType::Coal));
        assert!(!fuel_slot.routes(ItemType::Stone));
        assert!(!output.routes(ItemType::Coal));
        assert!(open.routes(ItemType::Stone));
        assert!(!open.routes_by_filter(ItemType::Stone));
    }

    #[test]
    fn containers_grow_to_spec_but_never_shrink_or_pass_the_cap() {
        let mut c = Container::with_len(3);
        c.slots[2] = Some(ItemStack::new(ItemType::Coal, 5));
        c.ensure_len(2);
        assert_eq!(c.slots.len(), 3, "ensure_len never shrinks");
        c.ensure_len(9);
        assert_eq!(c.slots.len(), 9);
        assert!(c.slots[2].is_some(), "stored stacks survive growth");
        c.ensure_len(500);
        assert_eq!(c.slots.len(), MAX_CONTAINER_SLOTS);
    }
}
