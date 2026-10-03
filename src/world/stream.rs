use crate::entity::DroppedItem;
use crate::mob::SavedMob;
use petramond_world::chunk::SectionPos;
use petramond_world::section::Section;

mod fluid_kick;
mod poll;
mod priorities;
mod requests;
mod settle;
mod shape;
mod sky_cavern;
mod unload;

#[cfg(any(test, feature = "test-support"))]
#[cfg(test)]
mod tests;

#[cfg(any(test, feature = "test-support"))]
pub use petramond_world::column_split::split_generated_column;

pub(super) type LoadedOverlay = (Section, Vec<DroppedItem>, Vec<SavedMob>);

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum StreamEvent {
    Generated(SectionPos),
    Loaded(SectionPos),
}
