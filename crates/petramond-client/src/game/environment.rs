use std::sync::Arc;

use petramond::world::environment::ShaderParamMap;
use petramond::world::ReplicaWorld;
use petramond_math::math::IVec3;
use petramond_render::EyeFluid;
use petramond_world::biome::{
    blended_fog_color, blended_water_clarity, blended_water_color, Biome,
};
use petramond_world::block::Block;
use petramond_world::chunk::{SectionPos, SECTION_SIZE, WORLD_MAX_Y, WORLD_MIN_Y};
use petramond_world::section::SectionSummary;
use petramond_world::world::{SectionCursor, WorldData};

/// How far beside a covered eye its fluid's open surface is looked for, nearest first.
const SIDE_REACH: [i32; 10] = [1, 2, 3, 4, 6, 8, 12, 16, 24, 32];
const SIDE_DIRECTIONS: [(i32, i32); 8] = [
    (1, 0),
    (-1, 0),
    (0, 1),
    (0, -1),
    (1, 1),
    (1, -1),
    (-1, 1),
    (-1, -1),
];
/// The depth of an eye whose fluid shows no open surface anywhere near: deep enough to be dark.
const UNSEEN_SURFACE_DEPTH: f32 = 128.0;

use super::Game;

#[derive(Clone, Debug, PartialEq)]
pub struct GameEnvironment {
    pub fog: [f32; 3],
    pub time: f32,
    pub eye: Option<EyeFluid>,
    pub shader_params: Arc<ShaderParamMap>,
}

impl Game {
    pub(super) fn environment(&self, now: f64) -> GameEnvironment {
        let eye = self.render_camera().pos;
        let (fog, eye) = camera_fog(&self.replica.world, eye, |wx, wz| {
            if let Some(id) = self.replica.world.data().column_biome(wx, wz) {
                return Biome::from_id(id);
            }

            self.replica.fallback_world.biome_at(wx, wz)
        });

        GameEnvironment {
            fog,
            eye,
            time: (self.world_clock(now) % 3600.0) as f32,
            shader_params: self
                .replica
                .world
                .data()
                .environment()
                .shader_params()
                .clone(),
        }
    }
}

pub fn camera_fog(
    world: &ReplicaWorld,
    eye: petramond_math::world_pos::WorldPos,
    mut biome_at: impl FnMut(i32, i32) -> Biome,
) -> ([f32; 3], Option<EyeFluid>) {
    let data = world.data();
    let inside = data.eye_fluid(eye);
    // The view's near plane reaches past the eye, so water whose surface the eye is just over
    // (or just under, short of counting as inside) still fills the rays that start below it.
    let surface = data
        .eye_surface(eye)
        .filter(|(fluid, _)| inside.is_none_or(|inside| inside == *fluid))
        .filter(|(fluid, _)| has_optics(*fluid));
    let Some(block) = inside.or(surface.map(|(fluid, _)| fluid)) else {
        return (blended_fog_color(eye.x, eye.z, biome_at), None);
    };
    // Water with optics is seen through its own lanes; its fog stays the air's, which what lies
    // above the surface is hazed by.
    let fog = match block.fluid_def().map(|def| &def.medium) {
        Some(medium) if inside.is_some() && medium.optics.is_none() => medium.fog_color,
        _ => blended_fog_color(eye.x, eye.z, &mut biome_at),
    };
    let surface = surface.map(|(_, y)| f64::from(y));
    let depth = depth_below_surface(world, eye, block, surface);
    let fluid = EyeFluid {
        block,
        inside: inside.is_some(),
        depth,
        surface,
        water: blended_water_color(eye.x, eye.z, &mut biome_at),
        clarity: blended_water_clarity(eye.x, eye.z, &mut biome_at),
    };
    (fog, Some(fluid))
}

fn has_optics(fluid: Block) -> bool {
    fluid
        .fluid_def()
        .is_some_and(|def| def.medium.optics.is_some())
}

/// Blocks of water between the eye and its fluid's open surface: the highest cell of the fluid
/// that touches open space above it, straight up from the eye, measured to `drawn` (the surface
/// drawn at the eye) when that is the one. Anything solid is looked through (an overhang, a
/// ledge, ice), and so is every lower surface (an air pocket sealed inside the water), open space
/// with no fluid right under it (an air pocket inside the rock) or a section only known as mixed,
/// so deep water under an overhang sees the same surface as the water beside it. A column covered
/// all the way up (a sea cave, a notch under a cliff) takes the nearest open surface beside it,
/// plus the way across to it.
fn depth_below_surface(
    world: &ReplicaWorld,
    eye: petramond_math::world_pos::WorldPos,
    fluid: Block,
    drawn: Option<f64>,
) -> f32 {
    let data = world.data();
    let cur = data.cursor();
    let cell = eye.block();
    let below = |top: f64| (top - eye.y).max(0.0) as f32;
    // From the cell under the eye, so an eye just over its surface finds it.
    if let Some(top) = open_surface_above(data, &cur, cell - IVec3::Y, fluid) {
        let top = f64::from(top);
        return match drawn {
            Some(y) if top - y < 1.0 => below(y),
            _ => below(top),
        };
    }
    for reach in SIDE_REACH {
        let nearest = SIDE_DIRECTIONS
            .iter()
            .filter_map(|&(dx, dz)| {
                let column = IVec3::new(cell.x + dx * reach, cell.y, cell.z + dz * reach);
                let top = open_surface_above(data, &cur, column, fluid)?;
                let across = ((dx * dx + dz * dz) as f32).sqrt() * reach as f32;
                Some(below(f64::from(top)) + across)
            })
            .reduce(f32::min);
        if let Some(depth) = nearest {
            return depth;
        }
    }
    UNSEEN_SURFACE_DEPTH
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum ColumnCell {
    Fluid,
    Open,
    Covered,
}

/// The top of the highest cell of `fluid` in `start`'s column, from `start` up, with open space
/// directly above it.
fn open_surface_above(
    data: &WorldData,
    cur: &SectionCursor<'_>,
    start: IVec3,
    fluid: Block,
) -> Option<i32> {
    let class = |block: Block, cell_fluid: Option<Block>| {
        if cell_fluid == Some(fluid) {
            ColumnCell::Fluid
        } else if block.is_solid() {
            ColumnCell::Covered
        } else {
            ColumnCell::Open
        }
    };
    let mut on_fluid = false;
    let mut highest = None;
    let mut y = start.y.max(WORLD_MIN_Y);
    while y < WORLD_MAX_Y {
        let at = IVec3::new(start.x, y, start.z);
        let (cell, next) = match cur.section_at(at) {
            Some((section, lx, ly, lz)) => (
                class(section.block(lx, ly, lz), section.cell_fluid(lx, ly, lz)),
                y + 1,
            ),
            None => {
                let pos = SectionPos::from_world(at.x, at.y, at.z)?;
                let summary = data.section_summary(pos);
                let cell = match summary {
                    SectionSummary::Empty
                    | SectionSummary::FullWater
                    | SectionSummary::FullOpaque => {
                        let block = summary.virtual_block();
                        class(block, block.fluid())
                    }
                    SectionSummary::Mixed | SectionSummary::Unknown => ColumnCell::Covered,
                };
                (cell, (pos.cy + 1) * SECTION_SIZE as i32)
            }
        };
        if cell == ColumnCell::Open && on_fluid {
            highest = Some(y);
        }
        on_fluid = cell == ColumnCell::Fluid;
        y = next;
    }
    highest
}

#[cfg(test)]
mod tests {
    use crate::game::Game;
    use petramond_math::math::IVec3;
    use petramond_math::world_pos::WorldPos;
    use petramond_render::camera::Camera;
    use petramond_world::block::Block;
    use petramond_world::chunk::ChunkPos;
    use petramond_world::fluid_math::fluid_height;

    #[test]
    fn the_environment_reports_the_eye_fluid() {
        let mut game = Game::new(
            Camera::new(WorldPos::new(0.0, 80.0, 0.0), 16.0 / 9.0),
            "",
            1,
            1,
        )
        .unwrap();
        game.replica.world.clear_world();
        game.replica
            .world
            .insert_empty_column_for_test(ChunkPos::new(0, 0));
        let p = IVec3::new(8, 64, 8);
        for pos in [p, p + IVec3::Y] {
            game.replica
                .world
                .section_at_world_mut_for_test(pos.x, pos.y, pos.z)
                .expect("test section must be installed")
                .set_fluid(
                    (pos.x & 0x0F) as usize,
                    pos.y.rem_euclid(16) as usize,
                    (pos.z & 0x0F) as usize,
                    Block::Water,
                    0,
                );
        }
        game.local.cam.pos = WorldPos::new(p.x as f64 + 0.5, p.y as f64 + 0.5, p.z as f64 + 0.5);
        let eye = game.environment(0.0).eye.expect("the eye is in the water");
        assert_eq!(eye.block, Block::Water);
        assert!(eye.inside);
        let surface = f64::from(65.0 + fluid_height(0, None, Block::Water));
        assert_eq!(eye.surface, Some(surface), "the top cell's drawn surface");
        assert!(
            (f64::from(eye.depth) - (surface - 64.5)).abs() < 1e-4,
            "the eye sits under that surface, got {}",
            eye.depth
        );

        game.local.cam.pos = WorldPos::new(p.x as f64 + 0.5, surface + 0.02, p.z as f64 + 0.5);
        let over = game
            .environment(0.0)
            .eye
            .expect("the water the eye is just over");
        assert!(!over.inside, "an eye over the surface is in the air");
        assert_eq!((over.surface, over.depth), (Some(surface), 0.0));
    }

    fn empty_column_game() -> Game {
        let mut game = Game::new(
            Camera::new(WorldPos::new(0.0, 80.0, 0.0), 16.0 / 9.0),
            "",
            1,
            1,
        )
        .unwrap();
        game.replica.world.clear_world();
        game.replica
            .world
            .insert_empty_column_for_test(ChunkPos::new(0, 0));
        game
    }

    fn put(game: &mut Game, x: i32, ys: std::ops::RangeInclusive<i32>, block: Block) {
        for y in ys {
            let section = game
                .replica
                .world
                .section_at_world_mut_for_test(x, y, 8)
                .expect("test section must be installed");
            let (lx, ly) = ((x & 0x0F) as usize, y.rem_euclid(16) as usize);
            if block == Block::Water {
                section.set_fluid(lx, ly, 8, Block::Water, 0);
            } else {
                section.set_block(lx, ly, 8, block);
            }
        }
    }

    fn depth_at(game: &mut Game, x: i32, y: f64) -> f32 {
        game.local.cam.pos = WorldPos::new(f64::from(x) + 0.5, y, 8.5);
        game.environment(0.0)
            .eye
            .expect("the eye is in the water")
            .depth
    }

    /// Under an overhang the eye still sees the sea's surface, not the rock's underside: the
    /// underwater look must not change when swimming under a ledge.
    #[test]
    fn a_roof_over_the_eye_keeps_the_depth_to_the_open_surface() {
        let mut game = empty_column_game();
        put(&mut game, 8, 60..=63, Block::Water);
        put(&mut game, 8, 64..=65, Block::Stone);
        put(&mut game, 8, 66..=69, Block::Water);
        let depth = depth_at(&mut game, 8, 61.5);
        assert!(
            (depth - 8.5).abs() < 1e-4,
            "the eye sits 8.5 below the water over the rock, got {depth}"
        );

        put(&mut game, 8, 64..=64, Block::Air);
        put(&mut game, 8, 63..=63, Block::Stone);
        let depth = depth_at(&mut game, 8, 61.5);
        assert!(
            (depth - 8.5).abs() < 1e-4,
            "an air pocket inside the rock is no surface, got {depth}"
        );
    }

    /// An air pocket walled off inside the sea has a drawn floor but is no surface: the depth runs
    /// to the sea's, or everything over the pocket's height reads as dry land in shallow water.
    #[test]
    fn an_air_pocket_sealed_inside_the_water_is_no_surface() {
        let mut game = empty_column_game();
        put(&mut game, 8, 60..=63, Block::Water);
        put(&mut game, 8, 64..=64, Block::Air);
        put(&mut game, 8, 65..=65, Block::Stone);
        put(&mut game, 8, 66..=69, Block::Water);
        for eye_y in [61.5, 63.5] {
            let depth = depth_at(&mut game, 8, eye_y);
            assert!(
                (depth - (70.0 - eye_y) as f32).abs() < 1e-4,
                "an eye at {eye_y} sits under the sea's surface at 70, got {depth}"
            );
        }

        let floor = f64::from(63.0 + fluid_height(0, None, Block::Water));
        game.local.cam.pos = WorldPos::new(8.5, floor + 0.02, 8.5);
        let over = game.environment(0.0).eye.expect("the pocket's floor");
        assert_eq!(
            over.surface,
            Some(floor),
            "the waterline stays the pocket's floor"
        );
        assert!(
            (f64::from(over.depth) - (70.0 - (floor + 0.02))).abs() < 1e-4,
            "looking down from the pocket sees the sea's depth, got {}",
            over.depth
        );
    }

    /// A notch under a cliff has rock all the way up; its water's surface is the open sea beside
    /// it, so swimming in does not plunge the look into the deep.
    #[test]
    fn a_covered_eye_takes_the_open_surface_beside_it() {
        let mut game = empty_column_game();
        put(&mut game, 8, 60..=63, Block::Water);
        put(&mut game, 8, 64..=70, Block::Stone);
        put(&mut game, 9, 60..=63, Block::Water);
        let depth = depth_at(&mut game, 8, 61.5);
        assert!(
            (depth - 3.5).abs() < 1e-4,
            "2.5 below the sea beside the notch and one across, got {depth}"
        );
    }
}
