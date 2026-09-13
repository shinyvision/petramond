use std::sync::Arc;

use petramond::world::environment::ShaderParamMap;
use petramond::world::World;
use petramond_math::math::{lerp, IVec3, Vec3};
use petramond_world::biome::{blended_fog_color, Biome};
use petramond_world::block::Block;

use super::Game;

#[derive(Clone, Debug, PartialEq)]
pub struct GameEnvironment {
    pub fog: [f32; 3],
    pub time: f32,
    /// The fluid the camera eye is inside, if any: its medium row drives the
    /// fog band, the tints and the clear colour.
    pub eye_fluid: Option<Block>,
    /// Named visual shader parameters, written by mods on the tick and mapped
    /// to fixed GPU slots by the active shader pack.
    pub shader_params: Arc<ShaderParamMap>,
}

impl Game {
    pub(super) fn environment(&self, now: f64) -> GameEnvironment {
        // Fog/murk follow the RENDERED camera: a third-person boom dipping into
        // a fluid must show its murk even while the player's eye is dry.
        let eye = self.render_camera().pos;
        let (fog, eye_fluid) = camera_fog(&self.replica, eye, |wx, wz| {
            if let Some(id) = self.replica.column_biome(wx, wz) {
                return Biome::from_id(id);
            }

            self.fallback_world.biome_at(wx, wz)
        });

        GameEnvironment {
            fog,
            eye_fluid,
            time: (now % 3600.0) as f32,
            shader_params: self.replica.environment().shader_params().clone(),
        }
    }
}

/// Fog colour and the eye's fluid for an eye at `eye` in `world` — the two
/// environment inputs a renderer driver hands to `update_uniforms`. `biome_at`
/// is a parameter because the game reads its replica (falling back to the
/// generator for columns it has not received), while a driver holding a plain
/// world reads that world directly.
pub fn camera_fog(
    world: &World,
    eye: petramond_math::world_pos::WorldPos,
    biome_at: impl FnMut(i32, i32) -> Biome,
) -> ([f32; 3], Option<Block>) {
    let eye_fluid = camera_eye_fluid(world, eye);
    let fog = match eye_fluid.and_then(Block::fluid_def) {
        Some(def) => def.medium.fog_color,
        None => blended_fog_color(eye.x, eye.z, biome_at),
    };
    (fog, eye_fluid)
}

/// The fluid the camera eye is inside, judged by its medium's `eye_margin`.
fn camera_eye_fluid(world: &World, eye: petramond_math::world_pos::WorldPos) -> Option<Block> {
    let cell = eye.block();
    let fluid = Block::from_id(world.chunk_block(cell.x, cell.y, cell.z)).fluid()?;
    let margin = fluid.fluid_def()?.medium.eye_margin;
    eye_inside(world, eye, fluid, margin).then_some(fluid)
}

/// Whether an eye in a cell of `fluid` counts as inside it. With a `margin`
/// only an eye that far below the open surface does, so a barely-clipping eye
/// (a shallow flowing film) stays dry; without one, any eye in the cell does.
fn eye_inside(
    world: &World,
    eye: petramond_math::world_pos::WorldPos,
    fluid: Block,
    margin: Option<f32>,
) -> bool {
    let Some(margin) = margin else {
        return true;
    };
    let cell = eye.block();
    // The same fluid above means an interior volume, not the open surface.
    if Block::from_id(world.chunk_block(cell.x, cell.y + 1, cell.z)).fluid() == Some(fluid) {
        return true;
    }
    eye.y < f64::from(surface_y_at(world, cell, eye.relative_to(cell), fluid) - margin)
}

fn surface_y_at(world: &World, cell: IVec3, eye_in_cell: Vec3, fluid: Block) -> f32 {
    if fills_cell_at(world, cell.x, cell.y, cell.z, fluid) {
        return cell.y as f32 + 1.0;
    }

    let mut h = [[1.0f32; 2]; 2];

    // Match the fluid mesher's corner-height rule: each top vertex averages the
    // same-fluid cells meeting that corner, so a flow forms one sloped sheet.
    for cx in 0..2i32 {
        for cz in 0..2i32 {
            let mut sum = 0.0;
            let mut cnt = 0;
            for ox in (cx - 1)..=cx {
                for oz in (cz - 1)..=cz {
                    if let Some(height) =
                        fluid_height_at(world, cell.x + ox, cell.y, cell.z + oz, fluid)
                    {
                        sum += height;
                        cnt += 1;
                    }
                }
            }
            h[cx as usize][cz as usize] = if cnt == 0 { 1.0 } else { sum / cnt as f32 };
        }
    }

    let fx = eye_in_cell.x.clamp(0.0, 1.0);
    let fz = eye_in_cell.z.clamp(0.0, 1.0);
    let z0 = lerp(h[0][0], h[1][0], fx);
    let z1 = lerp(h[0][1], h[1][1], fx);
    cell.y as f32 + lerp(z0, z1, fz)
}

fn fluid_height_at(world: &World, wx: i32, wy: i32, wz: i32, fluid: Block) -> Option<f32> {
    if Block::from_id(world.chunk_block(wx, wy, wz)).fluid() != Some(fluid) {
        return None;
    }
    Some(petramond::world::fluid::fluid_height(
        world.fluid_meta_world(wx, wy, wz),
        Block::from_id(world.chunk_block(wx, wy + 1, wz)),
        fluid,
    ))
}

fn fills_cell_at(world: &World, wx: i32, wy: i32, wz: i32, fluid: Block) -> bool {
    if Block::from_id(world.chunk_block(wx, wy, wz)).fluid() != Some(fluid) {
        return false;
    }
    petramond::world::fluid::fills_cell(
        world.fluid_meta_world(wx, wy, wz),
        Block::from_id(world.chunk_block(wx, wy + 1, wz)),
        fluid,
    )
}

#[cfg(test)]
mod tests {
    use super::eye_inside;
    use crate::game::Game;
    use petramond_math::math::IVec3;
    use petramond_math::world_pos::WorldPos;
    use petramond_render::camera::Camera;
    use petramond_world::block::Block;
    use petramond_world::chunk::ChunkPos;

    /// A synthetic eye margin: the mechanism under test is that the eye test
    /// honours whatever margin a medium declares, not any row's value.
    const MARGIN: f32 = 0.1;
    const FALLING_META: u8 = 0x80;

    fn game() -> Game {
        let mut game = Game::new(
            Camera::new(WorldPos::new(0.0, 80.0, 0.0), 16.0 / 9.0),
            "",
            1,
            1,
        );
        // The environment reads the REPLICA (what the camera sees); a full
        // empty column (every section present) so a fluid write at any Y lands.
        game.replica.clear_world();
        game.replica
            .insert_empty_column_for_test(ChunkPos::new(0, 0));
        game
    }

    fn set_fluid(game: &mut Game, pos: IVec3, meta: u8) {
        let section = game
            .replica
            .section_at_world_mut_for_test(pos.x, pos.y, pos.z)
            .expect("test section must be installed");
        section.set_fluid(
            (pos.x & 0x0F) as usize,
            pos.y.rem_euclid(16) as usize,
            (pos.z & 0x0F) as usize,
            Block::Water,
            meta,
        );
    }

    fn inside(game: &Game, p: IVec3, y: f32) -> bool {
        let eye = WorldPos::new(p.x as f64 + 0.5, f64::from(y), p.z as f64 + 0.5);
        eye_inside(&game.replica, eye, Block::Water, Some(MARGIN))
    }

    #[test]
    fn the_eye_follows_a_flowing_surface_height() {
        let mut game = game();
        let p = IVec3::new(4, 64, 4);
        set_fluid(&mut game, p, 7); // the flow's leading edge: the thinnest film
        let surface =
            p.y as f32 + petramond::world::fluid::fluid_height(7, Block::Air, Block::Water);
        assert!(!inside(&game, p, p.y as f32 + 0.5));
        assert!(inside(&game, p, surface - MARGIN - 0.01));
    }

    #[test]
    fn the_eye_waits_until_the_margin_below_an_open_surface() {
        let mut game = game();
        let p = IVec3::new(5, 64, 5);
        set_fluid(&mut game, p, 0);
        let surface =
            p.y as f32 + petramond::world::fluid::fluid_height(0, Block::Air, Block::Water);
        assert!(!inside(&game, p, surface + 0.01));
        assert!(!inside(&game, p, surface - MARGIN * 0.5));
        assert!(inside(&game, p, surface - MARGIN - 0.01));
        let eye = WorldPos::new(
            p.x as f64 + 0.5,
            f64::from(surface - MARGIN * 0.5),
            p.z as f64 + 0.5,
        );
        assert!(
            eye_inside(&game.replica, eye, Block::Water, None),
            "a medium without a margin counts any eye in its cell"
        );
    }

    #[test]
    fn a_capped_or_falling_cell_is_inside_up_to_its_top() {
        let mut game = game();
        let capped = IVec3::new(6, 64, 6);
        set_fluid(&mut game, capped, 0);
        set_fluid(&mut game, capped + IVec3::Y, 0);
        assert!(inside(&game, capped, capped.y as f32 + 0.99));

        let falling = IVec3::new(7, 64, 7);
        set_fluid(&mut game, falling, FALLING_META);
        assert!(inside(
            &game,
            falling,
            falling.y as f32 + 1.0 - MARGIN - 0.01
        ));
    }

    #[test]
    fn the_environment_reports_the_eye_fluid() {
        let mut game = game();
        let p = IVec3::new(8, 64, 8);
        set_fluid(&mut game, p, 0);
        set_fluid(&mut game, p + IVec3::Y, 0);
        game.cam.pos = WorldPos::new(p.x as f64 + 0.5, p.y as f64 + 0.5, p.z as f64 + 0.5);
        assert_eq!(game.environment(0.0).eye_fluid, Some(Block::Water));
    }
}
