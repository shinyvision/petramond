//! The creative building tools a session carries: the held world tools
//! (selection, fill…), the schematic paste preview, the on-disk schematic
//! library, the capture/paste share with the server, and the placed-schematic
//! ghosts. Presentation and request state only — every world edit they make
//! travels to the server as a message.

use super::ghosts::Ghosts;
use super::schematic_library::SchematicLibrary;
use super::schematic_preview::SchematicPreview;
use super::schematics::SchematicShare;
use super::world_tool::WorldTools;

#[derive(Default)]
pub struct Tools {
    /// Every world tool by the name item rows use.
    pub world: WorldTools,
    /// The schematic held up for pasting, and its aimed placement.
    pub preview: SchematicPreview,
    /// The player's saved schematics on disk, loaded and thumbnailed off the
    /// frame thread.
    pub library: SchematicLibrary,
    /// Captures arriving from, and pastes leaving for, the server.
    pub share: SchematicShare,
    /// Meshed ghosts of the placed schematics the server tracks.
    pub(super) ghosts: Ghosts,
    /// A paste the player asked for went up (an edge the menu closes on).
    pub(super) paste_preview_ready: bool,
}
