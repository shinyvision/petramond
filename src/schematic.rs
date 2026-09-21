//! Portable, sparse structures and complete cell snapshots for creative edits.

mod cell;
mod sections;
pub use sections::{CellRef, SchematicBuilder, SchematicSection, SectionCell};
mod scene;
pub use scene::Scene;
pub mod archive;
pub mod library;
mod outline;
mod selection;
pub mod share;
pub mod store;
#[cfg(test)]
mod tests;
mod transform;

pub use cell::{CellData, ResolvedCell, SavedStack};
pub use selection::{FaceRect, Selection, SelectionBox, SelectionFace, SelectionSurface};
pub use transform::{rotate_position, rotate_record};

use serde::{Deserialize, Serialize};

pub const PLACEMENT_REACH: f32 = 128.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SchematicCell {
    pub pos: [i32; 3],
    pub data: CellData,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Schematic {
    pub name: String,
    pub size: [i32; 3],
    pub sections: Vec<SchematicSection>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CreativeAction {
    Capture {
        name: String,
        regions: Vec<SelectionBox>,
        include_air: bool,
    },
    /// Paste the design `digest` names; the server asks for its archive
    /// (`SchematicNotice::Want`) when the world does not hold it.
    Place {
        digest: store::Digest,
        origin: [i32; 3],
        turns: u8,
    },
    Undo,
    Redo,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CreativeReply {
    /// The capture asked for is arriving as the blob `digest`.
    Captured {
        digest: store::Digest,
    },
    Message(String),
}

impl Schematic {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() || self.name.len() > 128 {
            return Err("Use a name of 1–128 bytes".into());
        }
        if self.sections.is_empty() || self.size.iter().any(|s| *s < 1) {
            return Err("Invalid schematic dimensions".into());
        }
        let mut previous = None;
        for section in &self.sections {
            if previous.is_some_and(|p| p >= section.pos) {
                return Err("Duplicate or unsorted schematic section".into());
            }
            section.validate(self.size)?;
            previous = Some(section.pos);
        }
        Ok(())
    }
}
