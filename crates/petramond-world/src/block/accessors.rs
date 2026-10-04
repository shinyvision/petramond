use crate::facing::Facing;
use crate::item::{DropSpec, ItemType, ToolKind};
use crate::sound_registry::Sound;
use crate::tile::Tile;

use super::{
    data, definition, sounds, Aabb, Block, BlockBehavior, BlockFlags, BlockInteraction,
    BlockLightShape, BlockMaterial, BlockShapeKind, BlockSoundAction, BlockTag, MeshEmitter,
    ParticleEmitter, ShapeFamily, ShapeKindDef, ShapeState, SupportDir, ENGINE_BLOCK_NAMES,
};

impl Block {
    pub fn all() -> &'static [Block] {
        data::all()
    }

    #[inline]
    pub fn shape_kind(self) -> BlockShapeKind {
        self.def().shape_kind
    }

    #[inline]
    pub fn shape_kind_def(self) -> &'static ShapeKindDef {
        self.shape_kind().def()
    }

    #[inline]
    pub fn shape_family(self) -> ShapeFamily {
        self.shape_kind_def().family
    }

    #[inline]
    pub fn mesh_emitter(self) -> MeshEmitter {
        if self.animated_model().is_some() {
            MeshEmitter::Nothing
        } else {
            self.shape_kind_def().mesh_emitter
        }
    }

    #[inline]
    pub fn is_cube_shaped(self) -> bool {
        self.shape_kind_def().mesh_emitter == MeshEmitter::Cube
    }

    #[inline]
    pub fn is_custom_shape(self) -> bool {
        data::shape_custom(self.id())
    }

    #[inline]
    pub fn animated_model(self) -> Option<&'static crate::animated_model::AnimatedModelDef> {
        self.def().animated_model
    }

    pub fn animated_pose(
        self,
        state: ShapeState,
    ) -> Option<(
        &'static crate::animated_model::AnimatedModelDef,
        crate::animated_model::AnimatedPose,
    )> {
        let model = self.animated_model()?;
        let k = self.shape_kind_def();
        Some((model, k.render.animated_pose(&k.params, self, state)?))
    }

    pub fn compound_members(
        self,
        pos: crate::mathh::IVec3,
        state: ShapeState,
    ) -> Option<Vec<(crate::mathh::IVec3, ShapeState)>> {
        let k = self.shape_kind_def();
        k.sim.compound_members(&k.params, self, pos, state)
    }

    #[inline]
    pub fn nav_follows_row(self) -> bool {
        self.shape_kind_def().sim.nav_follows_row()
    }

    #[inline]
    pub fn shape_refines(self) -> bool {
        Self::id_refines_shape(self.id())
    }

    #[inline]
    pub fn id_refines_shape(id: u16) -> bool {
        data::shape_refines(id)
    }

    #[inline]
    pub fn model_kind(self) -> Option<crate::block_model::BlockModelKind> {
        self.shape_kind_def().params.model_kind()
    }

    #[allow(dead_code)]
    #[inline]
    pub fn default_light_apertures(self) -> u32 {
        data::default_light_apertures(self.id())
    }

    #[inline]
    pub fn light_shape(self) -> BlockLightShape {
        if self.is_opaque() {
            return BlockLightShape::OpaqueCube;
        }
        let k = self.shape_kind_def();
        k.sim.light_shape(&k.params, self)
    }

    #[inline]
    pub fn transmits_direct_skylight(self) -> bool {
        self == Block::Air
            || (self.light_shape() == BlockLightShape::Open
                && self.is_transparent()
                && !self.is_fluid()
                && !self.is_leaves())
    }

    pub fn has_same_light_behavior(self, other: Block) -> bool {
        let shape = self.light_shape();
        shape == other.light_shape()
            && shape != BlockLightShape::Shaped
            && self.transmits_direct_skylight() == other.transmits_direct_skylight()
            && self.light_emission_rgb() == other.light_emission_rgb()
    }

    #[inline]
    pub fn row_collision(self) -> &'static [Aabb] {
        self.def().collision
    }

    #[inline]
    pub fn collision_boxes(self) -> &'static [Aabb] {
        if let Some(kind) = self.model_kind() {
            return crate::block_model::collision_boxes(kind, [0, 0, 0]);
        }
        let k = self.shape_kind_def();
        k.sim.default_boxes(&k.params, self)
    }

    #[inline]
    pub fn static_collision_boxes(self) -> Option<&'static [Aabb]> {
        data::static_collision_boxes(self.0)
    }

    #[inline]
    pub fn nav_reads_solid(self) -> bool {
        data::nav_reads_solid(self.0)
    }

    #[inline]
    pub fn blocks_movement(self) -> bool {
        !self.collision_boxes().is_empty()
    }

    #[inline]
    pub fn visual_aabb(self) -> Option<([f32; 3], [f32; 3])> {
        let k = self.shape_kind_def();
        k.render.default_selection_box(&k.params, self)
    }

    #[inline]
    pub const fn id(self) -> u16 {
        self.0
    }

    #[inline]
    pub fn from_id(id: u16) -> Block {
        data::from_id(id)
    }

    #[inline]
    pub fn is_engine(self) -> bool {
        (self.0 as usize) < ENGINE_BLOCK_NAMES.len()
    }

    #[inline]
    pub fn is_solid(self) -> bool {
        data::flags(self.id()).is_solid()
    }

    #[inline]
    pub fn has_tag(self, tag: BlockTag) -> bool {
        data::has_tag(self.id(), tag)
    }

    #[inline]
    pub fn is_terrain_solid(self) -> bool {
        self.has_tag(BlockTag::TERRAIN)
    }

    #[inline]
    pub fn is_leaves(self) -> bool {
        self.has_tag(BlockTag::LEAVES)
    }

    #[inline]
    pub fn is_canopy(self) -> bool {
        self.has_tag(BlockTag::CANOPY)
    }

    #[inline]
    pub fn merges_with_self(self) -> bool {
        self.has_tag(BlockTag::MERGES_WITH_SELF)
    }

    #[inline]
    pub fn is_log(self) -> bool {
        self.has_tag(BlockTag::LOG)
    }

    #[inline]
    pub fn is_axial(self) -> bool {
        self.is_log() || self.has_tag(BlockTag::AXIAL)
    }

    #[inline]
    pub fn is_fluid(self) -> bool {
        self.flags().fluid()
    }

    #[inline]
    pub fn contained_fluid(self) -> Option<Block> {
        if !self.flags().contains_fluid() {
            return None;
        }
        self.def().contained_fluid
    }

    #[inline]
    pub fn fluid(self) -> Option<Block> {
        data::BlockTable::current().fluid(self.id())
    }

    #[inline]
    pub fn fluid_def(self) -> Option<&'static crate::fluid::FluidDef> {
        self.def().fluid
    }

    #[inline]
    pub fn behavior(self) -> &'static dyn BlockBehavior {
        self.def().behavior
    }

    #[inline]
    pub fn interaction(self) -> BlockInteraction {
        self.def().interaction
    }

    #[inline]
    pub fn next_stage(self) -> Option<Block> {
        self.def().next_stage
    }

    #[inline]
    pub fn grows_into(self) -> &'static [(&'static str, f32)] {
        self.def().grows_into
    }

    #[inline]
    pub fn has_random_tick(self) -> bool {
        self.behavior().has_random_tick()
    }

    #[inline]
    pub fn is_opaque(self) -> bool {
        data::flags(self.id()).is_opaque()
    }

    #[inline]
    pub fn flags(self) -> BlockFlags {
        data::flags(self.id())
    }

    #[inline]
    pub fn is_slab(self) -> bool {
        data::flags(self.id()).is_slab()
    }

    #[inline]
    pub fn has_box_shape(self) -> bool {
        data::flags(self.id()).has_box_shape()
    }

    #[inline]
    pub fn occludes_ao(self) -> bool {
        data::flags(self.id()).occludes_ao()
    }

    #[inline]
    pub fn is_transparent(self) -> bool {
        data::flags(self.id()).is_transparent()
    }

    #[inline]
    pub fn is_translucent(self) -> bool {
        data::flags(self.id()).is_translucent()
    }

    #[inline]
    pub fn light_emission(self) -> u8 {
        data::emission(self.id())
    }

    #[inline]
    pub fn light_emission_rgb(self) -> [u8; 3] {
        data::emission_rgb(self.id())
    }

    #[inline]
    pub fn front_tile(self) -> Option<Tile> {
        self.def().front
    }

    #[inline]
    pub fn side_overlay(self) -> Option<definition::SideOverlay> {
        self.def().side_overlay
    }

    #[inline]
    pub fn covered_side(self) -> Option<Tile> {
        self.def().covered_side
    }

    #[inline]
    pub fn particle_emitter(self) -> Option<&'static [ParticleEmitter]> {
        self.def().particle_emitter
    }

    #[inline]
    pub fn cloth(self) -> Option<&'static crate::cloth::ClothDef> {
        crate::cloth::def(self.def().cloth?)
    }

    #[inline]
    pub fn is_replaceable(self) -> bool {
        self.has_tag(BlockTag::REPLACEABLE)
    }

    #[inline]
    pub fn is_snow_cover(self) -> bool {
        self.has_tag(BlockTag::SNOW_COVER)
    }

    #[inline]
    pub fn is_snow_bedded(self) -> bool {
        self.has_tag(BlockTag::SNOW_BEDDED)
    }

    #[inline]
    pub fn is_fragile(self) -> bool {
        self.has_tag(BlockTag::FRAGILE)
    }

    #[inline]
    pub fn is_climbable(self) -> bool {
        data::flags(self.id()).is_climbable()
    }

    #[inline]
    pub fn panel_facing(self) -> Facing {
        self.def().panel_facing.unwrap_or_default()
    }

    #[inline]
    pub fn declared_panel_facing(self) -> Option<Facing> {
        self.def().panel_facing
    }

    #[inline]
    pub fn ladder_dims(self) -> (f32, f32) {
        self.shape_kind_def()
            .params
            .dimensions()
            .map_or((crate::ladder::THICKNESS, 1.0), |d| (d.thickness, d.height))
    }

    #[inline]
    pub fn rotated_wall_panel(self, facing: Facing) -> Block {
        Block::all()
            .iter()
            .copied()
            .find_map(|b| {
                b.def()
                    .facing_rows
                    .filter(|rows| rows.contains(&self))
                    .map(|rows| rows[facing.to_u8() as usize])
            })
            .unwrap_or(self)
    }

    pub fn facing_rows(self) -> Option<&'static [Block; 4]> {
        self.def().facing_rows
    }

    pub fn wall_panel_row(self, facing: Facing) -> Block {
        match self.def().facing_rows {
            Some(rows) => rows[facing.to_u8() as usize],
            None => self,
        }
    }

    #[inline]
    pub fn flipped_row(self) -> Option<Block> {
        self.def().flipped_row
    }

    #[inline]
    pub fn is_slippery(self) -> bool {
        data::flags(self.id()).is_slippery()
    }

    #[inline]
    pub fn melts_to(self) -> Option<Block> {
        self.def().melts_to
    }

    #[inline]
    pub fn break_residue(self, below: Block) -> Block {
        match self.melts_to() {
            Some(fluid) if below.is_solid() || below == fluid => fluid,
            _ => Block::Air,
        }
    }

    pub fn can_root_on(self, ground: Block) -> bool {
        let soil = self.has_tag(BlockTag::ROOTS_IN_SOIL);
        let sand = self.has_tag(BlockTag::ROOTS_IN_SAND);
        let stone = self.has_tag(BlockTag::ROOTS_IN_STONE);
        let named = self.def().roots_on;
        if !(soil || sand || stone) && named.is_empty() {
            return true;
        }
        (soil && ground.has_tag(BlockTag::SOIL))
            || (sand && ground.has_tag(BlockTag::SAND))
            || (stone && ground.material() == BlockMaterial::Stone)
            || named.iter().any(|t| ground.has_tag(*t))
    }

    #[inline]
    pub fn support_dir(self) -> SupportDir {
        self.def().support
    }

    #[inline]
    pub fn roots_face(self) -> crate::block::RootsFace {
        self.def().roots_face
    }

    pub fn rotated_row(self) -> Option<Block> {
        self.def().rotate_y
    }

    pub fn construction(self) -> Option<super::Construction> {
        self.def().construction
    }

    #[inline]
    pub fn directional_view(self) -> bool {
        data::flags(self.id()).is_directional_view()
    }

    #[inline]
    pub fn tiles(self) -> [Tile; 3] {
        self.def().tiles
    }

    #[inline]
    pub fn uv_turns(self) -> [u8; 3] {
        self.def().uv_turns
    }

    #[inline]
    pub fn fluid_still_tile(self) -> Tile {
        self.def().tiles[0]
    }

    #[inline]
    pub fn fluid_flow_tile(self) -> Tile {
        self.def().flow_tile.unwrap_or_else(|| self.def().tiles[0])
    }

    #[inline]
    pub fn material(self) -> BlockMaterial {
        self.def().material
    }

    #[inline]
    pub fn hardness(self) -> f32 {
        self.def().hardness
    }

    #[inline]
    pub fn drop_spec(self) -> DropSpec {
        self.def().drop
    }

    #[inline]
    pub fn to_item(self) -> ItemType {
        ItemType::from_block(self)
    }

    #[inline]
    pub fn requires_tool(self) -> bool {
        self.harvest_tier() >= 1
    }

    #[inline]
    pub fn preferred_tool(self) -> Option<ToolKind> {
        match self.material() {
            BlockMaterial::Stone | BlockMaterial::Ore | BlockMaterial::Ice => {
                Some(ToolKind::Pickaxe)
            }
            BlockMaterial::Wood => Some(ToolKind::Axe),
            BlockMaterial::Dirt | BlockMaterial::Sand | BlockMaterial::Snow => {
                Some(ToolKind::Shovel)
            }
            BlockMaterial::Wool | BlockMaterial::Plant | BlockMaterial::Foliage => {
                Some(ToolKind::Shears)
            }
            _ => None,
        }
    }

    #[inline]
    pub fn cut_by_preferred_tool(self) -> bool {
        matches!(self.material(), BlockMaterial::Foliage)
    }

    #[inline]
    pub fn harvest_tier(self) -> u8 {
        self.def().harvest_tier
    }

    #[inline]
    pub fn picks_by_boxes(self) -> bool {
        let k = self.shape_kind_def();
        k.render.picks_by_boxes(&k.params)
    }

    #[inline]
    pub fn precise_pick(self) -> bool {
        let k = self.shape_kind_def();
        k.render.precise_pick(&k.params)
    }

    #[inline]
    pub fn carry(self) -> &'static [&'static str] {
        self.def().carry
    }

    #[inline]
    pub fn data_value(self, key: &str) -> Option<&'static str> {
        let data = self.def().data;
        data.binary_search_by(|(k, _)| (*k).cmp(key))
            .ok()
            .map(|i| data[i].1)
    }

    #[inline]
    pub fn sound(self, action: BlockSoundAction) -> Option<Sound> {
        self.sound_set().get(action)
    }

    #[inline]
    fn sound_set(self) -> &'static sounds::BlockSoundSet {
        match self.material() {
            BlockMaterial::Wood => &sounds::WOOD,
            BlockMaterial::Stone | BlockMaterial::Ore => &sounds::STONE,
            BlockMaterial::Dirt => &sounds::DIRT,
            BlockMaterial::Sand => &sounds::SAND,
            BlockMaterial::Snow => &sounds::SNOW,
            BlockMaterial::Plant | BlockMaterial::Foliage => &sounds::LEAF,
            BlockMaterial::Glass | BlockMaterial::Ice => &sounds::GLASS,
            _ => &sounds::SILENT,
        }
    }

    #[inline]
    fn def(self) -> &'static definition::BlockDef {
        data::def(self)
    }
}
