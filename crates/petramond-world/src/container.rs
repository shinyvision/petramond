use crate::item::{ItemStack, ItemTag};

pub const MAX_CONTAINER_SLOTS: usize = 54;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Container {
    pub slots: Vec<Option<ItemStack>>,
}

impl Container {
    pub fn with_len(len: usize) -> Container {
        Container {
            slots: vec![None; len.min(MAX_CONTAINER_SLOTS)],
        }
    }

    pub fn ensure_len(&mut self, len: usize) {
        let len = len.min(MAX_CONTAINER_SLOTS);
        if self.slots.len() < len {
            self.slots.resize(len, None);
        }
    }

    pub fn count_like(&self, like: ItemStack) -> u32 {
        self.slots
            .iter()
            .flatten()
            .filter(|s| s.can_stack_with(&like))
            .map(|s| u32::from(s.count))
            .sum()
    }

    pub fn holds_all(&self, cost: &[ItemStack]) -> bool {
        cost.iter().all(|want| {
            let needed: u32 = cost
                .iter()
                .filter(|c| c.can_stack_with(want))
                .map(|c| u32::from(c.count))
                .sum();
            self.count_like(*want) >= needed
        })
    }

    pub fn take_all(&mut self, cost: &[ItemStack]) -> bool {
        if !self.holds_all(cost) {
            return false;
        }
        for want in cost {
            let mut left = want.count;
            for slot in self.slots.iter_mut().rev() {
                if left == 0 {
                    break;
                }
                let Some(stack) = slot else {
                    continue;
                };
                if !stack.can_stack_with(want) {
                    continue;
                }
                let n = stack.count.min(left);
                left -= n;
                *slot = (stack.count > n).then(|| stack.restack(stack.count - n));
            }
        }
        true
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SlotFilter {
    Tag(ItemTag),
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

pub const FULL_MASK: u32 = u32::MAX;

pub const MAX_SLOT_FILTERS: usize = u32::BITS as usize;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SlotSpec {
    pub accepts: Vec<SlotFilter>,
    pub take_only: bool,
    pub accepts_bind: Option<&'static str>,
}

impl SlotSpec {
    pub fn accepts_mask(&self, gui_state: Option<&crate::gui_state::GuiStateMap>) -> u32 {
        match self.accepts_bind.and_then(|key| gui_state?.get(key)) {
            Some(crate::gui_state::GuiValue::I32(m)) => *m as u32,
            _ => FULL_MASK,
        }
    }

    pub fn routes(&self, item: crate::item::ItemType, mask: u32) -> bool {
        if self.take_only {
            return false;
        }
        self.accepts.is_empty() || self.matches_active(item, mask)
    }

    pub fn admits(&self, item: crate::item::ItemType, mask: u32) -> bool {
        !self.take_only && self.routes(item, mask)
    }

    pub fn routes_by_filter(&self, item: crate::item::ItemType, mask: u32) -> bool {
        !self.take_only && self.matches_active(item, mask)
    }

    fn matches_active(&self, item: crate::item::ItemType, mask: u32) -> bool {
        self.accepts
            .iter()
            .enumerate()
            .any(|(i, &f)| i < MAX_SLOT_FILTERS && mask & (1 << i) != 0 && f.matches(item))
    }
}

/// Whether container slot `i` of `specs` accepts `held` on a DELIBERATE
/// placement — a click, or one leg of a drag. Nothing held admits nothing.
/// `gui_state` is the deciding session's map (see [`SlotSpec::accepts_mask`]).
///
/// The one question every placement path asks: the server's transport, the
/// server's drag capacity, and the client's prediction of both. It is a free
/// function over the whole list rather than a `SlotSpec` method because
/// ASKING IT is the part that matters — a path reaching for `take_only`
/// directly is a filter the two mirrors disagree about, and a drag splits by
/// the NUMBER of admitting slots, so one mirror counting one slot differently
/// changes the amount landing in every other slot of the gesture too.
pub fn slot_admits(
    specs: &[SlotSpec],
    i: usize,
    held: Option<crate::item::ItemType>,
    gui_state: Option<&crate::gui_state::GuiStateMap>,
) -> bool {
    match (specs.get(i), held) {
        (Some(spec), Some(item)) => spec.admits(item, spec.accepts_mask(gui_state)),
        _ => false,
    }
}

pub fn route_into(
    src: &mut Option<ItemStack>,
    slots: &mut [Option<ItemStack>],
    specs: &[SlotSpec],
    gui: Option<&crate::gui_state::GuiStateMap>,
) {
    let Some(item) = src.map(|s| s.item) else {
        return;
    };
    let by_filter = (0..slots.len()).filter(|&s| {
        specs
            .get(s)
            .is_some_and(|spec| spec.routes_by_filter(item, spec.accepts_mask(gui)))
    });
    let open = (0..slots.len()).filter(|&s| {
        specs.get(s).is_some_and(|spec| {
            let mask = spec.accepts_mask(gui);
            !spec.routes_by_filter(item, mask) && spec.routes(item, mask)
        })
    });
    let routed: Vec<usize> = by_filter.chain(open).collect();
    for &s in &routed {
        if src.is_none() {
            break;
        }
        if slots[s].is_some() {
            crate::inventory::merge_stack(src, &mut slots[s]);
        }
    }
    for &s in &routed {
        if src.is_none() {
            break;
        }
        if slots[s].is_none() {
            crate::inventory::merge_stack(src, &mut slots[s]);
        }
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
            accepts_bind: None,
        };
        let output = SlotSpec {
            accepts: Vec::new(),
            take_only: true,
            accepts_bind: None,
        };
        let open = SlotSpec::default();
        assert!(fuel_slot.routes(ItemType::Coal, FULL_MASK));
        assert!(fuel_slot.routes_by_filter(ItemType::Coal, FULL_MASK));
        assert!(!fuel_slot.routes(ItemType::Stone, FULL_MASK));
        assert!(!output.routes(ItemType::Coal, FULL_MASK));
        assert!(open.routes(ItemType::Stone, FULL_MASK));
        assert!(!open.routes_by_filter(ItemType::Stone, FULL_MASK));
    }

    #[test]
    fn a_bound_accepts_mask_narrows_the_authored_filters() {
        let cell = SlotSpec {
            accepts: vec![
                SlotFilter::Tag(ItemTag::FUEL),
                SlotFilter::Tag(ItemTag::SMELTABLE),
            ],
            take_only: false,
            accepts_bind: Some("m:cell0"),
        };
        assert!(cell.routes(ItemType::Coal, 0b01));
        assert!(!cell.routes(ItemType::Coal, 0b10));
        assert!(!cell.routes(ItemType::RawIron, 0b01));
        assert!(cell.routes(ItemType::RawIron, 0b10));
        assert!(!cell.routes(ItemType::Coal, 0), "mask 0 admits nothing");
        assert!(!cell.routes_by_filter(ItemType::Coal, 0b10));
        let mut map = crate::gui_state::GuiStateMap::new();
        assert_eq!(cell.accepts_mask(Some(&map)), FULL_MASK);
        map.insert("m:cell0".into(), crate::gui_state::GuiValue::I32(0b10));
        assert_eq!(cell.accepts_mask(Some(&map)), 0b10);
        assert_eq!(cell.accepts_mask(None), FULL_MASK);
        let open = SlotSpec::default();
        assert!(open.routes(ItemType::Stone, 0));
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
