use crate::entity::DroppedItem;
use crate::mob::SavedMob;
use petramond_world::chunk::SectionPos;
use petramond_world::section::Section;

mod poll;
mod requests;
mod settle;
mod shape;
mod unload;
mod water_kick;

#[cfg(any(test, feature = "test-support"))]
#[cfg(test)]
mod tests;

#[cfg(any(test, feature = "test-support"))]
pub use petramond_world::column_split::split_generated_column;

/// A saved section read back from disk, awaiting overlay over its generated column:
/// the decoded `Section` plus the item entities and mobs that rode in its record.
pub(super) type LoadedOverlay = (Section, Vec<DroppedItem>, Vec<SavedMob>);

/// A section install the per-frame streamer performed, buffered for the tick-side
/// event bus (`section_generated` / `section_loaded`): handlers must never run
/// from per-frame code, so `poll` only records and the next game tick dispatches.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum StreamEvent {
    /// A freshly generated section was installed.
    Generated(SectionPos),
    /// A saved (player-modified) section read from disk was overlaid over its
    /// generated base.
    Loaded(SectionPos),
}
