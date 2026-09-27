use std::collections::VecDeque;

use crate::inventory::{Inventory, TOTAL_SLOTS};
use crate::item::{ItemStack, ItemType};

use super::{CraftingRecipe, IngredientSelector, IngredientUse};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CraftFailure {
    OutputOccupied,
    MissingIngredients,
}

#[derive(Debug)]
pub struct CraftPlan {
    takes: Vec<(usize, u8)>,
    remainders: Vec<(ItemType, u16)>,
    output_variant: crate::item::VariantId,
}

#[derive(Copy, Clone)]
struct InventoryStack {
    slot: usize,
    item: ItemType,
    count: u8,
}

impl InventoryStack {
    fn matches(self, selector: IngredientSelector) -> bool {
        selector.matches(self.item)
    }
}

#[derive(Copy, Clone, Debug)]
struct FlowEdge {
    to: usize,
    reverse: usize,
    residual: u32,
    initial: u32,
}

struct FlowNetwork {
    edges: Vec<Vec<FlowEdge>>,
}

impl FlowNetwork {
    fn new(nodes: usize) -> Self {
        Self {
            edges: vec![Vec::new(); nodes],
        }
    }

    fn add_edge(&mut self, from: usize, to: usize, capacity: u32) -> usize {
        let forward = self.edges[from].len();
        let reverse = self.edges[to].len();
        self.edges[from].push(FlowEdge {
            to,
            reverse,
            residual: capacity,
            initial: capacity,
        });
        self.edges[to].push(FlowEdge {
            to: from,
            reverse: forward,
            residual: 0,
            initial: 0,
        });
        forward
    }

    fn max_flow(&mut self, source: usize, sink: usize, limit: u32) -> u32 {
        let mut total = 0;
        while total < limit {
            let levels = self.levels_from(source);
            if levels[sink] == usize::MAX {
                break;
            }
            let mut next_edge = vec![0; self.edges.len()];
            loop {
                let pushed = self.push(source, sink, limit - total, &levels, &mut next_edge);
                if pushed == 0 {
                    break;
                }
                total += pushed;
            }
        }
        total
    }

    fn levels_from(&self, source: usize) -> Vec<usize> {
        let mut levels = vec![usize::MAX; self.edges.len()];
        let mut queue = VecDeque::from([source]);
        levels[source] = 0;
        while let Some(node) = queue.pop_front() {
            for edge in &self.edges[node] {
                if edge.residual > 0 && levels[edge.to] == usize::MAX {
                    levels[edge.to] = levels[node] + 1;
                    queue.push_back(edge.to);
                }
            }
        }
        levels
    }

    fn push(
        &mut self,
        node: usize,
        sink: usize,
        limit: u32,
        levels: &[usize],
        next_edge: &mut [usize],
    ) -> u32 {
        if node == sink {
            return limit;
        }
        while next_edge[node] < self.edges[node].len() {
            let edge_index = next_edge[node];
            let edge = self.edges[node][edge_index];
            if edge.residual > 0 && levels[edge.to] == levels[node] + 1 {
                let pushed = self.push(edge.to, sink, limit.min(edge.residual), levels, next_edge);
                if pushed > 0 {
                    self.edges[node][edge_index].residual -= pushed;
                    self.edges[edge.to][edge.reverse].residual += pushed;
                    return pushed;
                }
            }
            next_edge[node] += 1;
        }
        0
    }

    fn flow_on(&self, from: usize, edge: usize) -> u32 {
        let edge = self.edges[from][edge];
        edge.initial - edge.residual
    }
}

pub fn plan(recipe: &CraftingRecipe, inventory: &Inventory) -> Option<CraftPlan> {
    let required = recipe
        .ingredients()
        .iter()
        .map(|ingredient| u64::from(ingredient.count))
        .sum::<u64>();
    if required == 0 {
        return None;
    }

    let mut available = Vec::with_capacity(TOTAL_SLOTS);
    for (slot, stack) in inventory.raw_slots().iter().enumerate() {
        let Some(stack) = stack else { continue };
        if stack.count > 0 {
            available.push(InventoryStack {
                slot,
                item: stack.item,
                count: stack.count,
            });
        }
    }
    let available_count = available
        .iter()
        .map(|stack| u32::from(stack.count))
        .sum::<u32>();
    if required > u64::from(available_count) {
        return None;
    }

    let required = required as u32;
    let ingredient_start = 1;
    let stack_start = ingredient_start + recipe.ingredients().len();
    let sink = stack_start + available.len();
    let source = 0;
    let mut network = FlowNetwork::new(sink + 1);
    let mut match_edges = vec![Vec::new(); recipe.ingredients().len()];

    for (ingredient_index, ingredient) in recipe.ingredients().iter().enumerate() {
        let ingredient_node = ingredient_start + ingredient_index;
        network.add_edge(source, ingredient_node, u32::from(ingredient.count));
        for (stack_index, stack) in available.iter().enumerate() {
            if stack.matches(ingredient.selector) {
                let edge = network.add_edge(
                    ingredient_node,
                    stack_start + stack_index,
                    u32::from(ingredient.count.min(u16::from(stack.count))),
                );
                match_edges[ingredient_index].push((stack_index, edge));
            }
        }
    }
    for (stack_index, stack) in available.iter().enumerate() {
        network.add_edge(stack_start + stack_index, sink, u32::from(stack.count));
    }
    if network.max_flow(source, sink, required) != required {
        return None;
    }

    let mut per_slot = [0u16; TOTAL_SLOTS];
    let mut remainders: Vec<(ItemType, u16)> = Vec::new();
    for (ingredient_index, ingredient) in recipe.ingredients().iter().enumerate() {
        let ingredient_node = ingredient_start + ingredient_index;
        for &(stack_index, edge) in &match_edges[ingredient_index] {
            let assigned = network.flow_on(ingredient_node, edge) as u16;
            if assigned == 0 {
                continue;
            }
            match ingredient.use_mode {
                IngredientUse::Keep => {}
                IngredientUse::Consume => per_slot[available[stack_index].slot] += assigned,
                IngredientUse::Remainder(item) => {
                    per_slot[available[stack_index].slot] += assigned;
                    if let Some((_, count)) = remainders.iter_mut().find(|(it, _)| *it == item) {
                        *count += assigned;
                    } else {
                        remainders.push((item, assigned));
                    }
                }
            }
        }
    }
    let takes: Vec<(usize, u8)> = per_slot
        .into_iter()
        .enumerate()
        .filter_map(|(slot, count)| {
            (count > 0).then_some((
                slot,
                u8::try_from(count).expect("assigned consumption fits its inventory stack"),
            ))
        })
        .collect();

    let output_variant = if recipe.inherit().is_empty() {
        crate::item::VariantId::NONE
    } else {
        let mut out = crate::item::variant::VariantMap::new();
        for key in recipe.inherit() {
            let mut chosen: Option<Vec<u8>> = None;
            for &(slot, _) in &takes {
                let Some(stack) = inventory.raw_slots()[slot] else {
                    continue;
                };
                let Some(value) = crate::item::variant::value(stack.variant, key) else {
                    continue;
                };
                match &chosen {
                    None => chosen = Some(value),
                    Some(prev) if *prev == value => {}
                    Some(_) => return None,
                }
            }
            if let Some(value) = chosen {
                out.insert(key.clone(), value);
            }
        }
        if out.is_empty() {
            crate::item::VariantId::NONE
        } else {
            crate::item::variant::intern(&out).unwrap_or_else(|e| {
                log::warn!(
                    "craft '{}': {e} — output loses inherited data",
                    recipe.key()
                );
                crate::item::VariantId::NONE
            })
        }
    };
    Some(CraftPlan {
        takes,
        remainders,
        output_variant,
    })
}

pub fn output_accepts(recipe: &CraftingRecipe, output: Option<ItemStack>) -> bool {
    let result = recipe.result();
    match output {
        None => true,
        Some(stack) => {
            stack.item == result.item
                && u16::from(stack.count) + u16::from(result.count)
                    <= u16::from(result.item.max_stack_size())
        }
    }
}

pub fn craft(
    recipe: &CraftingRecipe,
    inventory: &mut Inventory,
    output: &mut Option<ItemStack>,
) -> Result<Vec<ItemStack>, CraftFailure> {
    if !output_accepts(recipe, *output) {
        return Err(CraftFailure::OutputOccupied);
    }
    let plan = plan(recipe, inventory).ok_or(CraftFailure::MissingIngredients)?;
    if output
        .as_ref()
        .is_some_and(|stack| stack.variant != plan.output_variant)
    {
        return Err(CraftFailure::OutputOccupied);
    }
    if !inventory.consume_slots(&plan.takes) {
        return Err(CraftFailure::MissingIngredients);
    }

    let mut overflow = Vec::new();
    for (item, mut count) in plan.remainders {
        while count > 0 {
            let put = count.min(u16::from(item.max_stack_size())) as u8;
            if let Some(left) = inventory.add(ItemStack::new(item, put)) {
                overflow.push(left);
            }
            count -= u16::from(put);
        }
    }
    let mut result = recipe.result();
    result.variant = plan.output_variant;
    *output = Some(match *output {
        Some(stack) => stack.restack(stack.count + result.count),
        None => result,
    });
    Ok(overflow)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crafting::{CraftingIngredient, CraftingStation, IngredientSelector, IngredientUse};
    use crate::item::ItemTag;

    fn recipe(ingredients: Vec<CraftingIngredient>) -> CraftingRecipe {
        CraftingRecipe::new(
            "test:recipe".into(),
            CraftingStation::Inventory,
            ingredients,
            ItemStack::new(ItemType::Stick, 1),
        )
    }

    fn ingredient(
        selector: IngredientSelector,
        count: u16,
        use_mode: IngredientUse,
    ) -> CraftingIngredient {
        CraftingIngredient {
            selector,
            count,
            use_mode,
        }
    }

    fn count(inventory: &Inventory, item: ItemType) -> u16 {
        inventory
            .raw_slots()
            .iter()
            .flatten()
            .filter(|stack| stack.item == item)
            .map(|stack| u16::from(stack.count))
            .sum()
    }

    #[test]
    fn quantities_span_stacks_and_commit_atomically() {
        let mut inventory = Inventory::new();
        inventory.add(ItemStack::new(ItemType::OakPlanks, 40));
        inventory.add(ItemStack::new(ItemType::OakPlanks, 30));
        let recipe = recipe(vec![ingredient(
            IngredientSelector::Item(ItemType::OakPlanks),
            65,
            IngredientUse::Consume,
        )]);
        let mut output = None;
        assert!(craft(&recipe, &mut inventory, &mut output).is_ok());
        assert_eq!(count(&inventory, ItemType::OakPlanks), 5);
        assert_eq!(output, Some(ItemStack::new(ItemType::Stick, 1)));

        let before = *inventory.raw_slots();
        output = None;
        assert_eq!(
            craft(&recipe, &mut inventory, &mut output),
            Err(CraftFailure::MissingIngredients)
        );
        assert_eq!(inventory.raw_slots(), &before);
        assert!(output.is_none());
    }

    #[test]
    fn augmenting_path_preserves_exact_items_from_an_overlapping_tag() {
        let mut inventory = Inventory::new();
        inventory.add(ItemStack::new(ItemType::OakPlanks, 1));
        inventory.add(ItemStack::new(ItemType::SprucePlanks, 1));
        let recipe = recipe(vec![
            ingredient(
                IngredientSelector::Tag(ItemTag::PLANKS),
                1,
                IngredientUse::Consume,
            ),
            ingredient(
                IngredientSelector::Item(ItemType::OakPlanks),
                1,
                IngredientUse::Consume,
            ),
        ]);
        assert!(plan(&recipe, &inventory).is_some());
    }

    #[test]
    fn overlapping_keep_and_remainder_produce_the_right_slot_plan() {
        let mut inventory = Inventory::new();
        inventory.add(ItemStack::new(ItemType::OakPlanks, 1));
        inventory.add(ItemStack::new(ItemType::SprucePlanks, 1));
        let recipe = recipe(vec![
            ingredient(
                IngredientSelector::Tag(ItemTag::PLANKS),
                1,
                IngredientUse::Keep,
            ),
            ingredient(
                IngredientSelector::Item(ItemType::OakPlanks),
                1,
                IngredientUse::Remainder(ItemType::WoodenBucket),
            ),
        ]);

        let plan = plan(&recipe, &inventory).expect("broad catalyst moves to spruce");
        assert_eq!(plan.takes, vec![(0, 1)]);
        assert_eq!(plan.remainders, vec![(ItemType::WoodenBucket, 1)]);
    }

    #[test]
    fn capacity_matcher_handles_full_stack_counts_without_expanding_units() {
        let mut inventory = Inventory::new();
        let oak_slots = TOTAL_SLOTS / 2;
        let spruce_slots = TOTAL_SLOTS - oak_slots;
        let oak_stack = ItemType::OakPlanks.max_stack_size();
        let spruce_stack = ItemType::SprucePlanks.max_stack_size();
        for _ in 0..oak_slots {
            assert!(inventory
                .add(ItemStack::new(ItemType::OakPlanks, oak_stack))
                .is_none());
        }
        for _ in 0..spruce_slots {
            assert!(inventory
                .add(ItemStack::new(ItemType::SprucePlanks, spruce_stack))
                .is_none());
        }

        let oak_count = u16::try_from(oak_slots * usize::from(oak_stack)).unwrap();
        let spruce_count = u16::try_from(spruce_slots * usize::from(spruce_stack)).unwrap();
        let recipe = recipe(vec![
            ingredient(
                IngredientSelector::Tag(ItemTag::PLANKS),
                spruce_count,
                IngredientUse::Consume,
            ),
            ingredient(
                IngredientSelector::Item(ItemType::OakPlanks),
                oak_count,
                IngredientUse::Consume,
            ),
        ]);

        let plan = plan(&recipe, &inventory).expect("all aggregate capacity is assignable");
        let expected = inventory
            .raw_slots()
            .iter()
            .enumerate()
            .filter_map(|(slot, stack)| stack.map(|stack| (slot, stack.count)))
            .collect::<Vec<_>>();
        assert_eq!(plan.takes, expected);
    }

    #[test]
    fn catalyst_is_reserved_and_remainder_is_never_deleted() {
        let mut inventory = Inventory::new();
        inventory.add(ItemStack::new(ItemType::Coal, 1));
        inventory.add(ItemStack::new(ItemType::StoneShovel, 1));
        inventory.add(ItemStack::new(ItemType::WaterBucket, 1));
        let recipe = recipe(vec![
            ingredient(
                IngredientSelector::Item(ItemType::Coal),
                1,
                IngredientUse::Consume,
            ),
            ingredient(
                IngredientSelector::Tag(ItemTag::SHOVELS),
                1,
                IngredientUse::Keep,
            ),
            ingredient(
                IngredientSelector::Item(ItemType::WaterBucket),
                1,
                IngredientUse::Remainder(ItemType::WoodenBucket),
            ),
        ]);
        let mut output = None;
        let overflow = craft(&recipe, &mut inventory, &mut output).expect("craft");
        assert!(overflow.is_empty());
        assert_eq!(count(&inventory, ItemType::Coal), 0);
        assert_eq!(count(&inventory, ItemType::StoneShovel), 1);
        assert_eq!(count(&inventory, ItemType::WaterBucket), 0);
        assert_eq!(count(&inventory, ItemType::WoodenBucket), 1);
    }

    #[test]
    fn inherit_copies_agreeing_variants_and_refuses_disagreement() {
        use crate::item::{variant, VariantId};
        let tinted = |rgb: [u8; 3]| {
            let mut m = variant::VariantMap::new();
            m.insert("petramond:tint".into(), rgb.to_vec());
            variant::intern(&m).unwrap()
        };
        let red = tinted([200, 10, 10]);
        let mut recipe = recipe(vec![ingredient(
            IngredientSelector::Item(ItemType::OakPlanks),
            2,
            IngredientUse::Consume,
        )]);
        recipe
            .set_data(vec![(
                "petramond:inherit".into(),
                "[\"petramond:tint\"]".into(),
            )])
            .unwrap();

        let mut inventory = Inventory::new();
        inventory.add(ItemStack::with_variant(ItemType::OakPlanks, 1, red));
        inventory.add(ItemStack::with_variant(ItemType::OakPlanks, 1, red));
        let mut output = None;
        craft(&recipe, &mut inventory, &mut output).expect("agreeing craft");
        assert_eq!(output.unwrap().variant, red, "output inherits the tint");

        let mut inventory = Inventory::new();
        inventory.add(ItemStack::with_variant(ItemType::OakPlanks, 1, red));
        inventory.add(ItemStack::with_variant(
            ItemType::OakPlanks,
            1,
            tinted([10, 10, 200]),
        ));
        assert!(plan(&recipe, &inventory).is_none(), "disagreement refuses");

        let mut inventory = Inventory::new();
        inventory.add(ItemStack::new(ItemType::OakPlanks, 2));
        let mut output = None;
        craft(&recipe, &mut inventory, &mut output).expect("plain craft");
        assert_eq!(output.unwrap().variant, VariantId::NONE);
    }

    #[test]
    fn occupied_output_refuses_without_consuming() {
        let mut inventory = Inventory::new();
        inventory.add(ItemStack::new(ItemType::Coal, 1));
        let recipe = recipe(vec![ingredient(
            IngredientSelector::Item(ItemType::Coal),
            1,
            IngredientUse::Consume,
        )]);
        let before = *inventory.raw_slots();
        let mut output = Some(ItemStack::new(ItemType::Dirt, 1));
        assert_eq!(
            craft(&recipe, &mut inventory, &mut output),
            Err(CraftFailure::OutputOccupied)
        );
        assert_eq!(inventory.raw_slots(), &before);
    }
}
