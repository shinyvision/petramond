//! The open container menu: the craft grid plus the block-entity the GUI is
//! currently editing.
//!
//! `App::AppScreen` remains the single authority for *which* screen is open;
//! `ContainerMenu` owns only the persistent *edit target* — the block-entity (or
//! the inventory-side craft grid) the open GUI mutates — and the slot-interaction
//! behaviour for it. The one-shot *open request* is a `GameEvents` field consumed
//! by the app shell, never persisted here.

use crate::chest::Chest;
use crate::controls::PointerButton;
use crate::crafting::{CraftGrid, Recipes};
use crate::furnace::Furnace;
use crate::inventory::{Inventory, SlotGrid};
use crate::item::{ItemStack, ItemType};
use crate::mathh::IVec3;
use crate::render::{ChestView, CraftHit, FurnaceHit, FurnaceView, WorkbenchHit, WorkbenchView};
use crate::world::World;

/// What the open GUI is acting on — named for the thing being edited, not for the
/// screen. The app's `AppScreen` decides which screen is up; this decides which
/// block-entity (or the inventory-side craft grid) that screen reads and mutates.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum ContainerTarget {
    /// No container GUI is editing anything (gameplay, or a screen that owns no
    /// block-entity yet).
    #[default]
    None,
    /// The 2×2 crafting grid embedded in the inventory screen.
    Inventory,
    /// The 3×3 crafting grid at a placed crafting table.
    Table,
    /// The furnace at this world position.
    Furnace(IVec3),
    /// The chest at this world position.
    Chest(IVec3),
    /// A furniture workbench. Like the crafting grid it owns no persistent block-entity
    /// — the single input block lives transiently on the menu and is returned to the
    /// inventory on close — so no position is needed.
    FurnitureWorkbench,
}

impl ContainerTarget {
    /// The world position of the open chest, if a chest is the current target. The
    /// lid animation on `Game` reads this to know which chest to ease open.
    #[inline]
    pub fn open_chest(self) -> Option<IVec3> {
        match self {
            ContainerTarget::Chest(pos) => Some(pos),
            _ => None,
        }
    }
}

/// A click hit-tested to a concrete slot identity, the unit the App routes through
/// [`ContainerMenu::click`]. The App's per-layout hit-testers resolve a pixel to one
/// of these (a role for the furnace, an index for chest/craft/inventory); the menu
/// then decodes the (slot × button × shift) taxonomy in ONE place keyed on its
/// [`ContainerTarget`], instead of the App carrying a router per container type.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MenuSlot {
    /// A main inventory/hotbar slot (the 36-slot grid drawn under every panel).
    /// What a shift-click does depends on the open target — the furnace tag-routes
    /// it to fuel/input, the chest dumps it in, otherwise it shuffles hotbar↔grid.
    Inventory(usize),
    /// A crafting input cell or the result slot of the open craft grid.
    Craft(CraftHit),
    /// A furnace role slot (smeltable input, fuel, or take-only output).
    Furnace(FurnaceHit),
    /// A chest storage slot index.
    Chest(usize),
    /// A furniture-workbench slot: the input block, or a take-only result cell.
    Workbench(WorkbenchHit),
}

/// A block-entity container the open GUI can edit in place: the one accessor that
/// the byte-identical `edit_open_furnace` / `edit_open_chest` helpers differed on.
/// [`with_open_container`] pairs it with the (type-independent) chunk-modified mark
/// so an otherwise-idle container — which no tick would re-flag — persists its edit.
trait BlockEntityContainer: Sized {
    /// Mutable handle to this container at `pos`, or `None` if it has unloaded.
    fn at_mut(world: &mut World, pos: IVec3) -> Option<&mut Self>;
}

impl BlockEntityContainer for Furnace {
    #[inline]
    fn at_mut(world: &mut World, pos: IVec3) -> Option<&mut Self> {
        world.furnace_at_mut(pos)
    }
}

impl BlockEntityContainer for Chest {
    #[inline]
    fn at_mut(world: &mut World, pos: IVec3) -> Option<&mut Self> {
        world.chest_at_mut(pos)
    }
}

/// The active container menu: the crafting grid (2×2 in the inventory, 3×3 at a
/// table) plus the edit target the open GUI mutates.
///
/// Recipes are NOT owned here — the furnace *smelting* tick on `Game` also needs
/// them, so they stay on `Game` and are borrowed into the craft methods per call.
/// That keeps the smelt tick free of a self-referential borrow.
#[derive(Default)]
pub struct ContainerMenu {
    /// What the open GUI is currently editing (a block-entity or the craft grid).
    target: ContainerTarget,
    /// The active crafting grid + its cached result. Empty whenever no crafting
    /// screen is open.
    craft: CraftGrid,
    /// The furniture workbench's single input block while its screen is open. Transient
    /// (returned to the inventory on close, like the craft grid), so the workbench owns
    /// no block-entity. `None` whenever no workbench screen is open.
    workbench_input: Option<ItemStack>,
}

impl ContainerMenu {
    pub fn new() -> Self {
        Self::default()
    }

    /// What the open GUI is editing (read by the lid animation + render views).
    #[inline]
    pub fn target(&self) -> ContainerTarget {
        self.target
    }

    /// The active crafting grid (for the UI to read cells + result preview).
    #[inline]
    pub fn craft_grid(&self) -> &CraftGrid {
        &self.craft
    }

    // --- Open / close ------------------------------------------------------

    /// Configure the crafting grid for a screen of `cols×cols` (2 = inventory,
    /// 3 = table) and clear it. Called when a crafting screen opens.
    pub fn open_crafting(&mut self, cols: usize, recipes: &Recipes) {
        self.target = if cols >= 3 {
            ContainerTarget::Table
        } else {
            ContainerTarget::Inventory
        };
        self.craft.reset(cols);
        self.craft.recompute(recipes);
    }

    /// Begin a furnace-screen session at `pos`: remember which furnace the GUI
    /// reads and edits. Defensively creates an empty entity if the block lacks one
    /// (placement always inserts one, so this is belt-and-braces).
    pub fn open_furnace_screen(&mut self, world: &mut World, pos: IVec3) {
        if world.furnace_at(pos).is_none() {
            world.insert_furnace(pos, crate::furnace::Facing::default());
        }
        self.target = ContainerTarget::Furnace(pos);
    }

    /// End the furnace-screen session. The furnace keeps its contents (unlike the
    /// crafting grid, which empties back into the inventory on close).
    pub fn close_furnace(&mut self) {
        if matches!(self.target, ContainerTarget::Furnace(_)) {
            self.target = ContainerTarget::None;
        }
    }

    /// Begin a chest-screen session at `pos`: remember which chest the GUI reads and
    /// edits. Defensively creates an empty chest if the block lacks one (placement
    /// always inserts one, so this is belt-and-braces).
    pub fn open_chest_screen(&mut self, world: &mut World, pos: IVec3) {
        if world.chest_at(pos).is_none() {
            world.insert_chest(pos, crate::furnace::Facing::default());
        }
        self.target = ContainerTarget::Chest(pos);
    }

    /// End the chest-screen session. The chest keeps its contents (like the furnace,
    /// unlike the crafting grid which empties back into the inventory on close).
    pub fn close_chest(&mut self) {
        if matches!(self.target, ContainerTarget::Chest(_)) {
            self.target = ContainerTarget::None;
        }
    }

    /// Begin a furniture-workbench session: the input slot starts empty.
    pub fn open_workbench(&mut self) {
        self.target = ContainerTarget::FurnitureWorkbench;
        self.workbench_input = None;
    }

    /// End the workbench session: return the input block to the inventory (overflow
    /// thrown into the world via `overflow`), then drop the target — like the crafting
    /// grid, the workbench is a station that holds nothing once closed.
    pub fn close_workbench(&mut self, inv: &mut Inventory, mut overflow: impl FnMut(ItemStack)) {
        if let Some(stack) = self.workbench_input.take() {
            if let Some(leftover) = inv.add(stack) {
                overflow(leftover);
            }
        }
        if matches!(self.target, ContainerTarget::FurnitureWorkbench) {
            self.target = ContainerTarget::None;
        }
    }

    /// Close the crafting grid: return every input item to the inventory (any
    /// overflow is thrown into the world via `overflow`), then clear the result and
    /// drop the craft target.
    pub fn close_crafting(
        &mut self,
        inv: &mut Inventory,
        recipes: &Recipes,
        mut overflow: impl FnMut(ItemStack),
    ) {
        for i in 0..self.craft.capacity() {
            if let Some(stack) = self.craft.take_cell(i) {
                if let Some(leftover) = inv.add(stack) {
                    overflow(leftover);
                }
            }
        }
        self.craft.recompute(recipes);
        if matches!(
            self.target,
            ContainerTarget::Inventory | ContainerTarget::Table
        ) {
            self.target = ContainerTarget::None;
        }
    }

    // --- Unified click dispatch -------------------------------------------

    /// Route a hit-tested click to the open container. This is the single decision
    /// point that replaced the App's four parallel `route_*` routers and its
    /// `is_furnace()/is_chest()` ladder: the App resolves the pixel to a [`MenuSlot`]
    /// (a role for the furnace, an index elsewhere) and hands it here with the button
    /// and the physical Shift state, plus `gather` — the App-owned double-click
    /// verdict that turns a left-click-with-a-held-stack into a same-item collect.
    ///
    /// The per-slot decoding (shift quick-move vs left/right place-split-swap, the
    /// furnace's take-only output, the inventory shift-click routed by the open
    /// target) all lives below, matched on `self.target` so adding a container type
    /// touches this one method, not a ladder repeated per interaction.
    #[allow(clippy::too_many_arguments)]
    pub fn click(
        &mut self,
        world: &mut World,
        inv: &mut Inventory,
        recipes: &Recipes,
        slot: MenuSlot,
        button: PointerButton,
        shift: bool,
        gather: bool,
    ) {
        match slot {
            MenuSlot::Inventory(i) => {
                if shift {
                    // Shift-click of an inventory slot is routed by the open target:
                    // the furnace tag-routes fuel/smeltable into its slots, the chest
                    // dumps the stack in, and otherwise it shuffles hotbar↔main-grid.
                    match self.target {
                        ContainerTarget::Furnace(_) => {
                            self.furnace_shift_from_inventory(world, inv, i)
                        }
                        ContainerTarget::Chest(_) => self.chest_shift_from_inventory(world, inv, i),
                        ContainerTarget::FurnitureWorkbench => {
                            self.workbench_shift_from_inventory(inv, recipes, i)
                        }
                        _ => inv.shift_move_slot(i),
                    }
                } else if gather {
                    // A double-click while dragging gathers matching items; with the
                    // chest open it sweeps the chest too (matching MC), else just the
                    // inventory. (The App gates this on the cursor holding a stack.)
                    self.collect_to_cursor(world, inv);
                } else {
                    match button {
                        PointerButton::Primary => inv.click_slot(i),
                        PointerButton::Secondary => inv.right_click_slot(i),
                    }
                }
            }
            MenuSlot::Craft(hit) => match hit {
                CraftHit::Input(i) => {
                    if shift {
                        self.craft_shift_slot(inv, recipes, i);
                    } else {
                        match button {
                            PointerButton::Primary => self.craft_click_slot(inv, recipes, i),
                            PointerButton::Secondary => {
                                self.craft_right_click_slot(inv, recipes, i)
                            }
                        }
                    }
                }
                CraftHit::Result => {
                    if shift {
                        self.craft_shift_result(inv, recipes);
                    } else {
                        self.craft_take_result(inv, recipes);
                    }
                }
            },
            MenuSlot::Furnace(hit) => match hit {
                FurnaceHit::Input => {
                    if shift {
                        self.furnace_shift_input(world, inv);
                    } else {
                        match button {
                            PointerButton::Primary => self.furnace_click_input(world, inv),
                            PointerButton::Secondary => self.furnace_right_click_input(world, inv),
                        }
                    }
                }
                FurnaceHit::Fuel => {
                    if shift {
                        self.furnace_shift_fuel(world, inv);
                    } else {
                        match button {
                            PointerButton::Primary => self.furnace_click_fuel(world, inv),
                            PointerButton::Secondary => self.furnace_right_click_fuel(world, inv),
                        }
                    }
                }
                // Output is take-only: any click moves the product out (shift -> inv).
                FurnaceHit::Output => {
                    if shift {
                        self.furnace_shift_output(world, inv);
                    } else {
                        self.furnace_take_output(world, inv);
                    }
                }
            },
            MenuSlot::Chest(i) => {
                if shift {
                    self.chest_shift_slot(world, inv, i);
                } else if gather {
                    // Double-click in the chest gathers from the chest AND inventory.
                    self.collect_to_cursor_in_chest(world, inv);
                } else {
                    match button {
                        PointerButton::Primary => self.chest_click_slot(world, inv, i),
                        PointerButton::Secondary => self.chest_right_click_slot(world, inv, i),
                    }
                }
            }
            MenuSlot::Workbench(hit) => match hit {
                WorkbenchHit::Input => {
                    if shift {
                        self.workbench_shift_input(inv);
                    } else {
                        match button {
                            PointerButton::Primary => {
                                inv.click_external_slot(&mut self.workbench_input)
                            }
                            PointerButton::Secondary => {
                                inv.right_click_external_slot(&mut self.workbench_input)
                            }
                        }
                    }
                }
                // Results are take-only: craft the i-th offered recipe (shift -> inv).
                WorkbenchHit::Result(i) => self.workbench_take_result(inv, recipes, i, shift),
            },
        }
    }

    /// The gather a double-click on an inventory slot performs: with a chest open it
    /// sweeps the chest's slots too (matching MC), otherwise only the inventory.
    fn collect_to_cursor(&self, world: &mut World, inv: &mut Inventory) {
        if matches!(self.target, ContainerTarget::Chest(_)) {
            self.collect_to_cursor_in_chest(world, inv);
        } else {
            inv.collect_to_cursor();
        }
    }

    // --- Crafting slot interactions ---------------------------------------

    /// Left-click a crafting input cell (cursor pick/drop/merge/swap), then
    /// refresh the result preview.
    pub fn craft_click_slot(&mut self, inv: &mut Inventory, recipes: &Recipes, i: usize) {
        if i >= self.craft.capacity() {
            return;
        }
        inv.click_external_slot(self.craft.cell_mut(i));
        self.craft.recompute(recipes);
    }

    /// Right-click a crafting input cell (split / place-one), then refresh.
    pub fn craft_right_click_slot(&mut self, inv: &mut Inventory, recipes: &Recipes, i: usize) {
        if i >= self.craft.capacity() {
            return;
        }
        inv.right_click_external_slot(self.craft.cell_mut(i));
        self.craft.recompute(recipes);
    }

    /// Shift-click a crafting input cell: move its whole stack to the inventory
    /// (whatever doesn't fit stays in the cell), then refresh.
    pub fn craft_shift_slot(&mut self, inv: &mut Inventory, recipes: &Recipes, i: usize) {
        if i >= self.craft.capacity() {
            return;
        }
        if let Some(stack) = self.craft.take_cell(i) {
            if let Some(leftover) = inv.add(stack) {
                *self.craft.cell_mut(i) = Some(leftover);
            }
        }
        self.craft.recompute(recipes);
    }

    /// Take one craft from the result slot onto the cursor: places the result on
    /// the cursor (stacking onto a matching held stack with room) and consumes one
    /// item from every occupied input cell. No-op if there's no result or the
    /// cursor can't accept the whole result.
    pub fn craft_take_result(&mut self, inv: &mut Inventory, recipes: &Recipes) {
        self.craft.take_result(recipes, inv.cursor_mut());
    }

    /// Shift-click the result: craft as many times as possible straight into the
    /// inventory, stopping when an ingredient runs out or the next result won't
    /// fully fit. The hotbar/main grid both receive results (via `add`).
    pub fn craft_shift_result(&mut self, inv: &mut Inventory, recipes: &Recipes) {
        // Bounded by the grid contents: each craft consumes ≥1 from every cell.
        for _ in 0..(64 * crate::crafting::MAX_CELLS) {
            let Some(result) = self.craft.result().copied() else {
                break;
            };
            if !inv.can_add(result) {
                break;
            }
            inv.add(result);
            self.craft.consume_one();
            self.craft.recompute(recipes);
        }
    }

    // --- Furnace slot interactions ----------------------------------------

    /// The view of the currently-open furnace for the UI (its slots + the two
    /// progress gauges), or `None` if no furnace screen is up or it has unloaded.
    pub fn open_furnace_view(&self, world: &World) -> Option<FurnaceView> {
        let ContainerTarget::Furnace(pos) = self.target else {
            return None;
        };
        Some(world.furnace_at(pos)?.view())
    }

    /// Run `edit` on the open furnace's contents, then mark its chunk modified so the
    /// change persists (an idle furnace wouldn't otherwise be re-saved). No-op when
    /// no furnace screen is open or the furnace has unloaded.
    fn edit_open_furnace(
        &self,
        world: &mut World,
        inv: &mut Inventory,
        edit: impl FnOnce(&mut Inventory, &mut Furnace),
    ) {
        let ContainerTarget::Furnace(pos) = self.target else {
            return;
        };
        with_open_container(world, pos, |f: &mut Furnace| edit(inv, f));
    }

    /// Left-click the furnace input (smeltable) slot: cursor pick/drop/merge/swap.
    pub fn furnace_click_input(&self, world: &mut World, inv: &mut Inventory) {
        self.edit_open_furnace(world, inv, |inv, f| inv.click_external_slot(f.input_slot()));
    }

    /// Right-click the furnace input slot: split / place-one.
    pub fn furnace_right_click_input(&self, world: &mut World, inv: &mut Inventory) {
        self.edit_open_furnace(world, inv, |inv, f| {
            inv.right_click_external_slot(f.input_slot())
        });
    }

    /// Left-click the furnace fuel slot: cursor pick/drop/merge/swap.
    pub fn furnace_click_fuel(&self, world: &mut World, inv: &mut Inventory) {
        self.edit_open_furnace(world, inv, |inv, f| inv.click_external_slot(f.fuel_slot()));
    }

    /// Right-click the furnace fuel slot: split / place-one.
    pub fn furnace_right_click_fuel(&self, world: &mut World, inv: &mut Inventory) {
        self.edit_open_furnace(world, inv, |inv, f| {
            inv.right_click_external_slot(f.fuel_slot())
        });
    }

    /// Click the furnace output: take-only — move the whole product onto the cursor
    /// if it fits. The take-only rule lives in [`Furnace::take_output`].
    pub fn furnace_take_output(&self, world: &mut World, inv: &mut Inventory) {
        self.edit_open_furnace(world, inv, |inv, f| {
            f.take_output(inv.cursor_mut());
        });
    }

    /// Shift-click the furnace input slot: move its stack to the inventory (whatever
    /// doesn't fit stays put).
    pub fn furnace_shift_input(&self, world: &mut World, inv: &mut Inventory) {
        self.edit_open_furnace(world, inv, |inv, f| inv.pull_from(f.input_slot()));
    }

    /// Shift-click the furnace fuel slot: move its stack to the inventory.
    pub fn furnace_shift_fuel(&self, world: &mut World, inv: &mut Inventory) {
        self.edit_open_furnace(world, inv, |inv, f| inv.pull_from(f.fuel_slot()));
    }

    /// Shift-click the furnace output slot: move the product to the inventory
    /// (take-only out — never a deposit). See [`Furnace::shift_output_into`].
    pub fn furnace_shift_output(&self, world: &mut World, inv: &mut Inventory) {
        self.edit_open_furnace(world, inv, |inv, f| f.shift_output_into(inv));
    }

    /// Shift-click inventory slot `i` while the furnace screen is open: routed by the
    /// furnace via [`Furnace::fill_slot_for`] — a fuel stack goes to the fuel slot and
    /// a smeltable stack to the input slot (leftover stays in the inventory). Items
    /// that are neither fall back to the normal hotbar↔grid move, so shift-click still
    /// does something sensible for them.
    pub fn furnace_shift_from_inventory(&self, world: &mut World, inv: &mut Inventory, i: usize) {
        let ContainerTarget::Furnace(pos) = self.target else {
            return;
        };
        let Some(stack) = inv.slot(i).copied() else {
            return;
        };
        // The fuel-vs-smeltable routing is furnace behavior (it reads item tags).
        let Some(role) = Furnace::fill_slot_for(stack.item) else {
            // Neither fuel nor smeltable: fall back to the ordinary hotbar↔grid move.
            inv.shift_move_slot(i);
            return;
        };
        // `world` and the inventory slot are disjoint borrows, so the furnace and
        // the inventory slot can be borrowed together for the move.
        with_open_container(world, pos, |furnace: &mut Furnace| {
            if let Some(src) = inv.slot_mut(i) {
                furnace.shift_in(role, src);
            }
        });
    }

    // --- Chest slot interactions ------------------------------------------

    /// The view of the currently-open chest for the UI (its 27 storage slots), or
    /// `None` if no chest screen is up or it has unloaded.
    pub fn open_chest_view(&self, world: &World) -> Option<ChestView> {
        let ContainerTarget::Chest(pos) = self.target else {
            return None;
        };
        Some(world.chest_at(pos)?.view())
    }

    /// Run `edit` on the open chest's contents, then mark its chunk modified so the
    /// change persists (an idle chest wouldn't otherwise be re-saved). No-op when no
    /// chest screen is open or the chest has unloaded.
    fn edit_open_chest(
        &self,
        world: &mut World,
        inv: &mut Inventory,
        edit: impl FnOnce(&mut Inventory, &mut Chest),
    ) {
        let ContainerTarget::Chest(pos) = self.target else {
            return;
        };
        with_open_container(world, pos, |chest: &mut Chest| edit(inv, chest));
    }

    /// Left-click a chest storage slot: cursor pick/drop/merge/swap.
    pub fn chest_click_slot(&self, world: &mut World, inv: &mut Inventory, i: usize) {
        self.edit_open_chest(world, inv, |inv, chest| {
            if let Some(slot) = chest.slots_mut().get_mut(i) {
                inv.click_external_slot(slot);
            }
        });
    }

    /// Right-click a chest storage slot: split / place-one.
    pub fn chest_right_click_slot(&self, world: &mut World, inv: &mut Inventory, i: usize) {
        self.edit_open_chest(world, inv, |inv, chest| {
            if let Some(slot) = chest.slots_mut().get_mut(i) {
                inv.right_click_external_slot(slot);
            }
        });
    }

    /// Shift-click a chest storage slot: move its stack to the inventory (whatever
    /// doesn't fit stays put).
    pub fn chest_shift_slot(&self, world: &mut World, inv: &mut Inventory, i: usize) {
        self.edit_open_chest(world, inv, |inv, chest| {
            if let Some(slot) = chest.slots_mut().get_mut(i) {
                inv.pull_from(slot);
            }
        });
    }

    /// Shift-click inventory slot `i` while the chest screen is open: move its whole
    /// stack into the chest (merging into matching stacks, then the first empty slot;
    /// leftover stays in the inventory).
    pub fn chest_shift_from_inventory(&self, world: &mut World, inv: &mut Inventory, i: usize) {
        let ContainerTarget::Chest(pos) = self.target else {
            return;
        };
        if inv.slot(i).is_none() {
            return;
        }
        // `world` and the inventory slot are disjoint borrows, so the chest slots
        // and the inventory slot can be borrowed together for the move.
        with_open_container(world, pos, |chest: &mut Chest| {
            let Some(src) = inv.slot_mut(i) else {
                return;
            };
            // First-fit the whole source stack into the chest; whatever didn't fit
            // (a single source stack is ≤ one max stack, so the general insert lands
            // it in one empty slot just like the old single-slot fill) stays behind.
            if let Some(stack) = src.take() {
                *src = chest.insert(stack);
            }
        });
    }

    /// Double-click gather in the open chest screen: top up the cursor-held stack
    /// with matching items from BOTH the chest and the inventory.
    pub fn collect_to_cursor_in_chest(&self, world: &mut World, inv: &mut Inventory) {
        self.edit_open_chest(world, inv, |inv, chest| {
            inv.collect_to_cursor_including(chest.slots_mut())
        });
    }

    // --- Furniture-workbench slot interactions ----------------------------

    /// The view of the open furniture workbench for the UI: its input block plus the
    /// results that block offers, each flagged craftable (enough input). `None` if no
    /// workbench screen is up. Recomputed from the recipes each call — cheap (≤21
    /// entries) and keeps the result list a pure function of the input.
    pub fn open_workbench_view(&self, recipes: &Recipes) -> Option<WorkbenchView> {
        if !matches!(self.target, ContainerTarget::FurnitureWorkbench) {
            return None;
        }
        Some(WorkbenchView {
            input: self.workbench_input,
            results: self.workbench_results(recipes),
        })
    }

    /// The results the placed input block offers: `(result item, craftable now)` per
    /// furniture recipe whose input matches, row-major. Empty when the input is empty.
    fn workbench_results(&self, recipes: &Recipes) -> Vec<(ItemType, bool)> {
        match self.workbench_input {
            Some(stack) => recipes
                .furniture_for(stack.item)
                .map(|r| (r.result.item, stack.count >= r.cost))
                .collect(),
            None => Vec::new(),
        }
    }

    /// Shift-click inventory slot `i` while the workbench screen is open: move the stack
    /// INTO the single input slot (merging onto a matching block, filling it if empty).
    /// Only a block that actually feeds a furniture recipe is routed there; anything else
    /// falls back to the ordinary hotbar↔grid move so shift-click still does something —
    /// mirroring the furnace's tag-routed shift-in. The reverse direction (input → inv)
    /// is [`workbench_shift_input`](Self::workbench_shift_input).
    pub fn workbench_shift_from_inventory(&mut self, inv: &mut Inventory, recipes: &Recipes, i: usize) {
        let Some(stack) = inv.slot(i).copied() else {
            return;
        };
        if recipes.furniture_for(stack.item).next().is_none() {
            inv.shift_move_slot(i);
            return;
        }
        // `self.workbench_input` and the inventory slot are disjoint borrows.
        if let Some(src) = inv.slot_mut(i) {
            crate::furnace::merge_stack(src, &mut self.workbench_input);
        }
    }

    /// Shift-click the workbench input: move the whole input stack to the inventory
    /// (whatever doesn't fit stays put).
    pub fn workbench_shift_input(&mut self, inv: &mut Inventory) {
        if let Some(stack) = self.workbench_input.take() {
            if let Some(leftover) = inv.add(stack) {
                self.workbench_input = Some(leftover);
            }
        }
    }

    /// Take (craft) the `i`-th offered result: consume `cost` of the input block and
    /// yield the result. A left-click places one craft on the cursor (only if it fits);
    /// shift-click crafts as many as the input + inventory allow, straight into the
    /// inventory. No-op when the input is empty, the i-th recipe doesn't exist, or there
    /// isn't enough input (a greyed result).
    pub fn workbench_take_result(
        &mut self,
        inv: &mut Inventory,
        recipes: &Recipes,
        i: usize,
        shift: bool,
    ) {
        let Some(input) = self.workbench_input else {
            return;
        };
        let Some(recipe) = recipes.furniture_for(input.item).nth(i).copied() else {
            return;
        };
        if input.count < recipe.cost {
            return; // greyed: not enough of the input block
        }
        if shift {
            // Craft repeatedly into the inventory until the input runs out or it won't fit.
            for _ in 0..(64 * 64) {
                let Some(have) = self.workbench_input else {
                    break;
                };
                if have.count < recipe.cost || !inv.can_add(recipe.result) {
                    break;
                }
                inv.add(recipe.result);
                self.consume_workbench_input(recipe.cost);
            }
        } else {
            // Place one craft onto the cursor (only if the whole result fits), then
            // consume the input cost — exactly the crafting-result take rule.
            let cursor = inv.cursor_mut();
            let placed = match cursor {
                None => {
                    *cursor = Some(recipe.result);
                    true
                }
                Some(cur)
                    if cur.can_stack_with(&recipe.result)
                        && cur.space_left() >= recipe.result.count =>
                {
                    cur.count += recipe.result.count;
                    true
                }
                _ => false,
            };
            if placed {
                self.consume_workbench_input(recipe.cost);
            }
        }
    }

    /// Remove `cost` items from the workbench input, clearing it when emptied.
    fn consume_workbench_input(&mut self, cost: u8) {
        if let Some(stack) = &mut self.workbench_input {
            stack.count = stack.count.saturating_sub(cost);
            if stack.count == 0 {
                self.workbench_input = None;
            }
        }
    }
}

/// Run `edit` on the block-entity container `C` at `pos`, then mark its chunk
/// modified so an otherwise-idle container persists the edit. No-op (but still
/// marks, matching the prior behaviour) when the container has unloaded.
///
/// This is the single helper that replaced the byte-identical `edit_open_furnace`
/// / `edit_open_chest` twins — they differed only in which `at_mut` accessor they
/// called, now selected by the `C: BlockEntityContainer` bound.
fn with_open_container<C: BlockEntityContainer>(
    world: &mut World,
    pos: IVec3,
    edit: impl FnOnce(&mut C),
) {
    if let Some(container) = C::at_mut(world, pos) {
        edit(container);
    }
    world.mark_chunk_modified(pos);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::Block;
    use crate::furnace::Facing;
    use crate::item::ItemType;
    use crate::world::World;

    /// A bare in-memory world with an empty chunk at (0,0) installed, so a
    /// block-entity can be placed and edited without touching disk or worldgen.
    fn world_with_empty_chunk() -> World {
        let mut world = World::new(1, 1);
        let pos = crate::chunk::ChunkPos::new(0, 0);
        world.clear_world();
        world.insert_chunk_for_test(pos, crate::chunk::Chunk::new(0, 0));
        world
    }

    fn recipes() -> Recipes {
        crate::crafting::load_recipes()
    }

    fn count_item(inv: &Inventory, item: ItemType) -> u32 {
        (0..crate::inventory::TOTAL_SLOTS)
            .filter_map(|i| inv.slot(i))
            .filter(|s| s.item == item)
            .map(|s| s.count as u32)
            .sum()
    }

    /// Put `stack` into the first craft cell by routing it through the cursor
    /// (inventory slot 0 → cursor → craft cell), as the UI clicks would.
    fn place_in_craft_cell(
        menu: &mut ContainerMenu,
        inv: &mut Inventory,
        recipes: &Recipes,
        cell: usize,
        stack: ItemStack,
    ) {
        inv.add(stack);
        inv.click_slot(0); // pick the stack onto the cursor
        menu.craft_click_slot(inv, recipes, cell); // drop it into the craft cell
    }

    #[test]
    fn crafting_planks_from_log_via_result_slot() {
        let recipes = recipes();
        let mut menu = ContainerMenu::new();
        let mut inv = Inventory::new();
        menu.open_crafting(2, &recipes);
        place_in_craft_cell(
            &mut menu,
            &mut inv,
            &recipes,
            0,
            ItemStack::new(ItemType::OakLog, 1),
        );
        assert_eq!(
            menu.craft_grid().result().map(|s| (s.item, s.count)),
            Some((ItemType::OakPlanks, 4))
        );
        // Take the result: 4 planks onto the cursor, the log consumed, no result.
        menu.craft_take_result(&mut inv, &recipes);
        assert_eq!(
            inv.cursor().map(|s| (s.item, s.count)),
            Some((ItemType::OakPlanks, 4))
        );
        assert!(menu.craft_grid().result().is_none());
        assert!(menu.craft_grid().is_empty());
    }

    #[test]
    fn shift_crafting_consumes_every_log_in_the_cell() {
        let recipes = recipes();
        let mut menu = ContainerMenu::new();
        let mut inv = Inventory::new();
        menu.open_crafting(2, &recipes);
        // A cell holding 3 logs shift-crafts three times (one log per craft).
        place_in_craft_cell(
            &mut menu,
            &mut inv,
            &recipes,
            0,
            ItemStack::new(ItemType::OakLog, 3),
        );
        menu.craft_shift_result(&mut inv, &recipes);
        assert!(menu.craft_grid().is_empty(), "all logs consumed");
        assert_eq!(count_item(&inv, ItemType::OakPlanks), 12);
    }

    #[test]
    fn closing_crafting_returns_grid_items_to_inventory() {
        let recipes = recipes();
        let mut menu = ContainerMenu::new();
        let mut inv = Inventory::new();
        menu.open_crafting(3, &recipes);
        place_in_craft_cell(
            &mut menu,
            &mut inv,
            &recipes,
            4,
            ItemStack::new(ItemType::OakLog, 5),
        );
        assert!(inv.cursor().is_none());
        menu.close_crafting(&mut inv, &recipes, |_| panic!("nothing should overflow"));
        assert_eq!(count_item(&inv, ItemType::OakLog), 5);
        assert!(menu.craft_grid().cell(4).is_none());
    }

    #[test]
    fn furnace_shift_routes_fuel_and_smeltable_to_their_slots() {
        let mut world = world_with_empty_chunk();
        let mut menu = ContainerMenu::new();
        let pos = IVec3::new(2, 64, 2);
        world.set_block_world(pos.x, pos.y, pos.z, Block::Furnace);
        world.insert_furnace(pos, Facing::North);
        menu.open_furnace_screen(&mut world, pos);

        // Hotbar: coal (slot 0), raw iron (slot 1), oak planks (slot 2 — neither tag).
        let mut inv = Inventory::new();
        inv.add(ItemStack::new(ItemType::Coal, 5));
        inv.add(ItemStack::new(ItemType::RawIron, 3));
        inv.add(ItemStack::new(ItemType::OakPlanks, 4));

        // Coal -> fuel slot.
        menu.furnace_shift_from_inventory(&mut world, &mut inv, 0);
        assert!(inv.slot(0).is_none(), "coal left the inventory");
        assert_eq!(
            world.furnace_at(pos).unwrap().fuel,
            Some(ItemStack::new(ItemType::Coal, 5)),
            "coal went to the fuel slot"
        );

        // Raw iron -> input slot.
        menu.furnace_shift_from_inventory(&mut world, &mut inv, 1);
        assert!(inv.slot(1).is_none(), "raw iron left the inventory");
        assert_eq!(
            world.furnace_at(pos).unwrap().input,
            Some(ItemStack::new(ItemType::RawIron, 3)),
            "raw iron went to the input slot"
        );

        // A non-fuel, non-smeltable item is not pulled into the furnace; it falls
        // back to the ordinary hotbar->main-grid shuffle.
        menu.furnace_shift_from_inventory(&mut world, &mut inv, 2);
        assert!(inv.slot(2).is_none(), "plank moved out of the hotbar slot");
        let f = world.furnace_at(pos).unwrap();
        assert_ne!(f.input.map(|s| s.item), Some(ItemType::OakPlanks));
        assert_ne!(f.fuel.map(|s| s.item), Some(ItemType::OakPlanks));
        // It landed in the main grid (first slot of the 27-slot region).
        assert_eq!(
            inv.slot(crate::inventory::HOTBAR_LEN).map(|s| s.item),
            Some(ItemType::OakPlanks),
        );
    }

    #[test]
    fn furnace_shift_merges_into_a_partly_filled_slot() {
        let mut world = world_with_empty_chunk();
        let mut menu = ContainerMenu::new();
        let pos = IVec3::new(3, 64, 3);
        world.set_block_world(pos.x, pos.y, pos.z, Block::Furnace);
        world.insert_furnace(pos, Facing::North);
        // Seed the fuel slot with some coal already.
        world.furnace_at_mut(pos).unwrap().fuel = Some(ItemStack::new(ItemType::Coal, 60));
        menu.open_furnace_screen(&mut world, pos);

        let mut inv = Inventory::new();
        inv.add(ItemStack::new(ItemType::Coal, 10));
        menu.furnace_shift_from_inventory(&mut world, &mut inv, 0);

        // 4 top up the fuel slot to 64; the remaining 6 stay in the inventory.
        assert_eq!(world.furnace_at(pos).unwrap().fuel.unwrap().count, 64);
        assert_eq!(inv.slot(0).map(|s| s.count), Some(6));
    }

    /// Place a block in the workbench input by routing it through the cursor (inventory
    /// slot 0 → cursor → input), as the UI clicks would. The click path needs a world,
    /// though the workbench slots don't touch it.
    fn place_in_workbench_input(
        menu: &mut ContainerMenu,
        world: &mut World,
        inv: &mut Inventory,
        recipes: &Recipes,
        stack: ItemStack,
    ) {
        inv.add(stack);
        inv.click_slot(0); // pick onto cursor
        menu.click(
            world,
            inv,
            recipes,
            MenuSlot::Workbench(WorkbenchHit::Input),
            PointerButton::Primary,
            false,
            false,
        );
    }

    #[test]
    fn workbench_offers_a_door_for_planks_and_crafting_consumes_input() {
        let recipes = recipes(); // the shipped recipes incl. plank → door
        let mut world = world_with_empty_chunk();
        let mut menu = ContainerMenu::new();
        let mut inv = Inventory::new();
        menu.open_workbench();
        // Empty input offers nothing.
        assert!(menu.open_workbench_view(&recipes).unwrap().results.is_empty());

        // Three oak planks in → oak door offered and craftable (cost 1).
        place_in_workbench_input(
            &mut menu,
            &mut world,
            &mut inv,
            &recipes,
            ItemStack::new(ItemType::OakPlanks, 3),
        );
        let view = menu.open_workbench_view(&recipes).unwrap();
        assert_eq!(view.input.map(|s| s.count), Some(3));
        assert!(
            view.results
                .iter()
                .any(|&(it, ok)| it == ItemType::OakDoor && ok),
            "oak door offered + craftable"
        );

        // Craft the first result: a door onto the cursor, one plank consumed.
        menu.click(
            &mut world,
            &mut inv,
            &recipes,
            MenuSlot::Workbench(WorkbenchHit::Result(0)),
            PointerButton::Primary,
            false,
            false,
        );
        assert_eq!(
            inv.cursor().map(|s| (s.item, s.count)),
            Some((ItemType::OakDoor, 1))
        );
        assert_eq!(menu.open_workbench_view(&recipes).unwrap().input.unwrap().count, 2);
    }

    #[test]
    fn shift_clicking_inventory_planks_fills_the_workbench_input() {
        let recipes = recipes();
        let mut world = world_with_empty_chunk();
        let mut menu = ContainerMenu::new();
        let mut inv = Inventory::new();
        menu.open_workbench();
        inv.add(ItemStack::new(ItemType::OakPlanks, 5));

        // Shift-click the inventory slot: the planks (a furniture input) jump to the input.
        menu.click(
            &mut world,
            &mut inv,
            &recipes,
            MenuSlot::Inventory(0),
            PointerButton::Primary,
            true,
            false,
        );
        assert_eq!(
            menu.open_workbench_view(&recipes).unwrap().input.map(|s| (s.item, s.count)),
            Some((ItemType::OakPlanks, 5)),
            "the whole plank stack moved into the input",
        );
        assert!(inv.slot(0).is_none(), "the inventory slot emptied");
    }

    #[test]
    fn shift_clicking_a_non_input_item_does_not_fill_the_workbench() {
        // A stick has no furniture recipe, so shift-click must NOT dump it in the input —
        // it falls back to the ordinary hotbar↔grid move instead.
        let recipes = recipes();
        let mut world = world_with_empty_chunk();
        let mut menu = ContainerMenu::new();
        let mut inv = Inventory::new();
        menu.open_workbench();
        inv.add(ItemStack::new(ItemType::Stick, 4));
        menu.click(
            &mut world,
            &mut inv,
            &recipes,
            MenuSlot::Inventory(0),
            PointerButton::Primary,
            true,
            false,
        );
        assert!(
            menu.open_workbench_view(&recipes).unwrap().input.is_none(),
            "a non-input item never lands in the workbench input",
        );
    }

    #[test]
    fn workbench_greys_out_and_refuses_a_result_below_its_cost() {
        // A recipe that needs 6 planks: with fewer it shows greyed and won't craft.
        let recipes = Recipes::new(
            Vec::new(),
            Vec::new(),
            vec![crate::crafting::FurnitureRecipe {
                input: ItemType::OakPlanks,
                result: ItemStack::new(ItemType::OakDoor, 1),
                cost: 6,
            }],
        );
        let mut world = world_with_empty_chunk();
        let mut menu = ContainerMenu::new();
        let mut inv = Inventory::new();
        menu.open_workbench();
        place_in_workbench_input(
            &mut menu,
            &mut world,
            &mut inv,
            &recipes,
            ItemStack::new(ItemType::OakPlanks, 3),
        );
        // Offered but greyed (3 < 6).
        let view = menu.open_workbench_view(&recipes).unwrap();
        assert_eq!(view.results, vec![(ItemType::OakDoor, false)]);
        // Clicking a greyed result does nothing: no door, input untouched.
        menu.click(
            &mut world,
            &mut inv,
            &recipes,
            MenuSlot::Workbench(WorkbenchHit::Result(0)),
            PointerButton::Primary,
            false,
            false,
        );
        assert!(inv.cursor().is_none(), "no craft below cost");
        assert_eq!(menu.open_workbench_view(&recipes).unwrap().input.unwrap().count, 3);
    }
}
