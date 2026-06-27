//! Slot-identity enums shared between the render-side hit-test and the game-side
//! container menu. They name *which* slot a click resolved to — a crafting input
//! cell or result, a furnace role — independent of where that slot sits on screen.
//!
//! They live in this neutral module (not in a per-screen layout file, and not in
//! the geometry-owning [`super::gui_def`]) so the deterministic mutation layer
//! ([`crate::game::container`]) can key on them without depending on any GUI
//! layout: the App resolves a pixel to one of these via [`super::gui_def`], and
//! `ContainerMenu::click` decodes it on the game tick.

/// A hit-tested crafting slot: an input cell index (`0..cols*cols`, row-major) or
/// the single output result slot.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CraftHit {
    Input(usize),
    Result,
}

/// A hit-tested furnace role: the smeltable input, the fuel, or the take-only
/// output. One slot each, so these are identified by role, never by position.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FurnaceHit {
    Input,
    Fuel,
    Output,
}

/// A hit-tested furniture-workbench slot: the single input block, or one of the
/// take-only result cells (`0..` row-major, indexing the recipes the placed block
/// offers — see [`crate::crafting::Recipes::furniture_for`]).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum WorkbenchHit {
    Input,
    Result(usize),
}
