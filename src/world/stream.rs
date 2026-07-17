use crate::chunk::SectionPos;
use crate::entity::DroppedItem;
use crate::mob::SavedMob;
use crate::section::Section;

mod poll;
mod requests;
mod settle;
mod shape;
mod unload;
mod water_kick;

#[cfg(test)]
mod column_split;
#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use column_split::split_generated_column;

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
