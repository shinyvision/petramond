use super::ghosts::Ghosts;
use super::schematic_library::SchematicLibrary;
use super::schematic_preview::SchematicPreview;
use super::schematics::SchematicShare;
use super::world_tool::WorldTools;

#[derive(Default)]
pub struct Tools {
    pub world: WorldTools,
    pub preview: SchematicPreview,
    pub library: SchematicLibrary,
    pub share: SchematicShare,
    pub(super) ghosts: Ghosts,
    pub(super) paste_preview_ready: bool,
}
