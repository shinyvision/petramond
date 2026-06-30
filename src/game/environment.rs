use crate::biome::{blended_fog_color, Biome};
use crate::block::Block;
use crate::mathh::{lerp, voxel_at, IVec3, Vec3};
use crate::world::World;

use super::Game;

/// Deep, murky blue the world fades to (fog + clear colour) when the camera eye
/// is underwater.
const UNDERWATER_FOG_COLOR: [f32; 3] = [0.04, 0.16, 0.30];

/// Require the camera eye to sit this far below an open water surface before the
/// underwater shader/fog kicks in. This keeps shallow flowing films from tinting
/// the view when the eye is only barely clipping their rendered surface.
const UNDERWATER_SURFACE_MARGIN: f32 = 0.03;

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct GameEnvironment {
    pub(crate) fog: [f32; 3],
    pub(crate) time: f32,
    pub(crate) underwater: bool,
}

impl Game {
    pub(super) fn environment(&self, now: f64) -> GameEnvironment {
        let eye = self.cam.pos;
        let underwater = camera_eye_underwater(&self.world, eye);

        let fog = if underwater {
            UNDERWATER_FOG_COLOR
        } else {
            self.blended_sky_fog_color(eye.x, eye.z)
        };

        GameEnvironment {
            fog,
            underwater,
            time: (now % 3600.0) as f32,
        }
    }

    fn blended_sky_fog_color(&self, x: f32, z: f32) -> [f32; 3] {
        blended_fog_color(x, z, |wx, wz| {
            if let Some(id) = self.world.column_biome(wx, wz) {
                return Biome::from_id(id);
            }

            self.fallback_world.biome_at(wx, wz)
        })
    }
}

fn camera_eye_underwater(world: &World, eye: Vec3) -> bool {
    let cell = voxel_at(eye);
    if Block::from_id(world.chunk_block(cell.x, cell.y, cell.z)) != Block::Water {
        return false;
    }

    // Water above means this is an interior water volume, not the open surface.
    if Block::from_id(world.chunk_block(cell.x, cell.y + 1, cell.z)) == Block::Water {
        return true;
    }

    let surface_y = water_surface_y_at(world, cell, eye.x, eye.z);
    eye.y < surface_y - UNDERWATER_SURFACE_MARGIN
}

fn water_surface_y_at(world: &World, cell: IVec3, eye_x: f32, eye_z: f32) -> f32 {
    if water_fills_cell_at(world, cell.x, cell.y, cell.z) {
        return cell.y as f32 + 1.0;
    }

    let mut h = [[1.0f32; 2]; 2];

    // Match the water mesher's corner-height rule: each top vertex averages the
    // water cells meeting that corner, so flowing water forms one sloped sheet.
    for cx in 0..2i32 {
        for cz in 0..2i32 {
            let mut sum = 0.0;
            let mut cnt = 0;
            for ox in (cx - 1)..=cx {
                for oz in (cz - 1)..=cz {
                    if let Some(height) = fluid_height_at(world, cell.x + ox, cell.y, cell.z + oz) {
                        sum += height;
                        cnt += 1;
                    }
                }
            }
            h[cx as usize][cz as usize] = if cnt == 0 { 1.0 } else { sum / cnt as f32 };
        }
    }

    let fx = (eye_x - cell.x as f32).clamp(0.0, 1.0);
    let fz = (eye_z - cell.z as f32).clamp(0.0, 1.0);
    let z0 = lerp(h[0][0], h[1][0], fx);
    let z1 = lerp(h[0][1], h[1][1], fx);
    cell.y as f32 + lerp(z0, z1, fz)
}

fn fluid_height_at(world: &World, wx: i32, wy: i32, wz: i32) -> Option<f32> {
    if Block::from_id(world.chunk_block(wx, wy, wz)) != Block::Water {
        return None;
    }
    let water_above = Block::from_id(world.chunk_block(wx, wy + 1, wz)) == Block::Water;
    Some(crate::world::water::fluid_height(
        world.water_meta_world(wx, wy, wz),
        water_above,
    ))
}

fn water_fills_cell_at(world: &World, wx: i32, wy: i32, wz: i32) -> bool {
    if Block::from_id(world.chunk_block(wx, wy, wz)) != Block::Water {
        return false;
    }
    let water_above = Block::from_id(world.chunk_block(wx, wy + 1, wz)) == Block::Water;
    crate::world::water::fills_cell(world.water_meta_world(wx, wy, wz), water_above)
}

#[cfg(test)]
mod tests {
    use crate::camera::Camera;
    use crate::chunk::{Chunk, ChunkPos};
    use crate::game::Game;
    use crate::mathh::{IVec3, Vec3};

    use super::UNDERWATER_SURFACE_MARGIN;
    use crate::block::Block;

    fn game() -> Game {
        Game::new(Camera::new(Vec3::new(0.0, 80.0, 0.0), 16.0 / 9.0), "", 1, 1)
    }

    fn install_empty_chunk(game: &mut Game) {
        let pos = ChunkPos::new(0, 0);
        game.world.clear_world();
        // A full empty column (every section present) so a water write at any Y lands in
        // a loaded section — an empty `Chunk` would split to no surface sections.
        game.world.insert_empty_column_for_test(pos);
    }

    fn set_test_water(game: &mut Game, pos: IVec3, meta: u8) {
        let section = game
            .world
            .section_at_world_mut_for_test(pos.x, pos.y, pos.z)
            .expect("test section must be installed");
        section.set_water(
            (pos.x & 0x0F) as usize,
            pos.y.rem_euclid(16) as usize,
            (pos.z & 0x0F) as usize,
            Block::Water,
            meta,
        );
    }

    #[test]
    fn underwater_shader_uses_flowing_water_surface_height() {
        let mut game = game();
        install_empty_chunk(&mut game);
        let p = IVec3::new(4, 64, 4);
        set_test_water(&mut game, p, 1); // flowing edge: the thinnest water film

        game.cam.pos = Vec3::new(p.x as f32 + 0.5, p.y as f32 + 0.5, p.z as f32 + 0.5);
        assert!(!game.environment(0.0).underwater);

        let surface = p.y as f32 + crate::world::water::fluid_height(1, false);
        game.cam.pos = Vec3::new(
            p.x as f32 + 0.5,
            surface - UNDERWATER_SURFACE_MARGIN - 0.01,
            p.z as f32 + 0.5,
        );
        assert!(game.environment(0.0).underwater);
    }

    #[test]
    fn underwater_shader_waits_until_confidently_below_source_surface() {
        let mut game = game();
        install_empty_chunk(&mut game);
        let p = IVec3::new(5, 64, 5);
        set_test_water(&mut game, p, 0);

        let surface = p.y as f32 + crate::world::water::fluid_height(0, false);
        game.cam.pos = Vec3::new(p.x as f32 + 0.5, surface + 0.01, p.z as f32 + 0.5);
        assert!(!game.environment(0.0).underwater);

        game.cam.pos = Vec3::new(
            p.x as f32 + 0.5,
            surface - UNDERWATER_SURFACE_MARGIN * 0.5,
            p.z as f32 + 0.5,
        );
        assert!(!game.environment(0.0).underwater);

        game.cam.pos = Vec3::new(
            p.x as f32 + 0.5,
            surface - UNDERWATER_SURFACE_MARGIN - 0.01,
            p.z as f32 + 0.5,
        );
        assert!(game.environment(0.0).underwater);
    }

    #[test]
    fn capped_water_cell_is_underwater_even_near_its_top() {
        let mut game = game();
        install_empty_chunk(&mut game);
        let p = IVec3::new(6, 64, 6);
        set_test_water(&mut game, p, 0);
        set_test_water(&mut game, p + IVec3::Y, 0);

        game.cam.pos = Vec3::new(p.x as f32 + 0.5, p.y as f32 + 0.99, p.z as f32 + 0.5);
        assert!(game.environment(0.0).underwater);
    }

    #[test]
    fn underwater_shader_treats_falling_water_as_full_height() {
        const FALLING_META: u8 = 0x80;

        let mut game = game();
        install_empty_chunk(&mut game);
        let p = IVec3::new(7, 64, 7);
        set_test_water(&mut game, p, FALLING_META);

        game.cam.pos = Vec3::new(p.x as f32 + 0.5, p.y as f32 + 0.5, p.z as f32 + 0.5);
        assert!(game.environment(0.0).underwater);
    }
}
