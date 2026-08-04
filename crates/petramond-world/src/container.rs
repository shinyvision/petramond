//! THE generic item-slot storage: one section-owned `Container` per block
//! position backs chests, furnaces, and mod container blocks alike — there
//! are NO parallel slot stores.
//!
//! The engine stores, renders, click-routes, persists, and scatters the
//! slots; what the contents MEAN is the container's owner — engine machine
//! state ([`crate::furnace::Furnace`]) or the owning mod's tick logic,
//! reached through the `ContainerGet`/`ContainerSet` host calls.
//!
//! Slot SEMANTICS (which item groups shift-clicks route into a slot, which
//! slots are take-only outputs) are [`SlotSpec`]s: engine-owned sets for the
//! chest/furnace (`game::container::generic`), document slot nodes resolved
//! at load for mod GUIs (`gui::documents`).

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

/// One item GROUP a slot admits, in whichever of the content layer's two
/// vocabularies names it.
///
/// A TAG is a closed vocabulary a row opts into; a DATA key is the vocabulary a
/// CONSUMING system already speaks about the items it understands. Both were
/// always here — this type is only the admission that a slot filter is a
/// question about item membership, not specifically about tags.
///
/// The data form is what lets a slot filter and a machine's own whitelist be
/// ONE statement: the mod enumerates `ItemsWithData("forge:metal")` to learn
/// what it can melt, and the slot admits exactly that set. It also reaches rows
/// the naming pack does not own — a `{"patch"}` row attaches a data key to
/// ANYONE's item (deliberately cross-namespace) and cannot attach a tag, so
/// tags alone can never describe "this pack's notion of metal, including the
/// engine's ingots".
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SlotFilter {
    Tag(ItemTag),
    /// Any item whose row carries this namespaced `data` key, whatever its
    /// value — membership is the question, and the value belongs to the
    /// consumer that parses it.
    Data(&'static str),
}

impl SlotFilter {
    pub fn matches(self, item: crate::item::ItemType) -> bool {
        match self {
            SlotFilter::Tag(tag) => item.has_tag(tag),
            SlotFilter::Data(key) => item.data_value(key).is_some(),
        }
    }
}

/// One document slot's host-interpreted semantics, resolved from the document's
/// `accepts` filters / `take_only` flag at load. In-role index order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SlotSpec {
    /// Item groups shift-clicks may route into this slot; empty = accepts any
    /// item on shift-routing (a plain storage cell).
    pub accepts: Vec<SlotFilter>,
    /// A take-only output: clicks only ever remove from it, and shift-routing
    /// never targets it.
    pub take_only: bool,
}

impl SlotSpec {
    /// Whether shift-routing may move `item` into this slot: never for an
    /// output, filter-matched when filters are declared, always otherwise.
    pub fn routes(&self, item: crate::item::ItemType) -> bool {
        if self.take_only {
            return false;
        }
        self.accepts.is_empty() || self.accepts.iter().any(|&f| f.matches(item))
    }

    /// Whether a manual click may PUT `item` into this slot. The predicted
    /// click and the authoritative one both ask THIS — a filter the client
    /// mirrors differently from the server is a click that visibly succeeds
    /// and then snaps back.
    pub fn admits(&self, item: crate::item::ItemType) -> bool {
        !self.take_only && self.routes(item)
    }

    /// Whether a shift-route of `item` should PREFER this slot: it names a
    /// matching filter (the furnace's fuel→fuel-slot read), beating unfiltered
    /// storage cells.
    pub fn routes_by_filter(&self, item: crate::item::ItemType) -> bool {
        !self.take_only && self.accepts.iter().any(|&f| f.matches(item))
    }
}

/// Whether container slot `i` of `specs` accepts `held` on a DELIBERATE
/// placement — a click, or one leg of a drag. Nothing held admits nothing.
///
/// The one question every placement path asks: the server's transport, the
/// server's drag capacity, and the client's prediction of both. It is a free
/// function over the whole list rather than a `SlotSpec` method because
/// ASKING IT is the part that matters — a path reaching for `take_only`
/// directly is a filter the two mirrors disagree about, and a drag splits by
/// the NUMBER of admitting slots, so one mirror counting one slot differently
/// changes the amount landing in every other slot of the gesture too.
pub fn slot_admits(specs: &[SlotSpec], i: usize, held: Option<crate::item::ItemType>) -> bool {
    match (specs.get(i), held) {
        (Some(spec), Some(item)) => spec.admits(item),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::ItemType;

    #[test]
    fn slot_specs_route_by_tag_and_never_into_outputs() {
        let fuel_slot = SlotSpec {
            accepts: vec![SlotFilter::Tag(ItemTag::FUEL)],
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
