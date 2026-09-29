use super::*;

#[test]
fn compass_text_and_outline_stay_inside_the_image_through_a_full_rotation() {
    let size = [14, 28];
    for degrees in 0..360 {
        let yaw = (degrees as f32).to_radians();
        let runs = cardinal_text_runs([-yaw.cos(), yaw.sin()], [yaw.sin(), yaw.cos()], size);
        for run in runs {
            for (axis, extent) in size.iter().enumerate() {
                assert!(run.position[axis] >= 0);
                assert!(run.position[axis] + i32::from(*extent) <= HUD_SIZE as i32);
            }
        }
    }
}

fn reference_terrain(
    store: &TileStore,
    player: [f64; 3],
    right: [f32; 2],
    forward: [f32; 2],
) -> Vec<u8> {
    let mut rgba = vec![0u8; HUD_SIZE * HUD_SIZE * 4];
    let mut reader = CellReader::new(&store.tiles);
    for py in 0..HUD_SIZE {
        for px in 0..HUD_SIZE {
            let sx = px as f32 + 0.5 - HUD_CENTER;
            let sy = py as f32 + 0.5 - HUD_CENTER;
            let radius = (sx * sx + sy * sy).sqrt();
            if radius <= HUD_TERRAIN_RADIUS {
                let up = -sy * HUD_BLOCKS_PER_PIXEL;
                let side = sx * HUD_BLOCKS_PER_PIXEL;
                let wx = (player[0] + f64::from(side * right[0] + up * forward[0])).floor() as i32;
                let wz = (player[2] + f64::from(side * right[1] + up * forward[1])).floor() as i32;
                set_pixel(
                    &mut rgba,
                    HUD_SIZE,
                    px as i32,
                    py as i32,
                    reader.terrain_rgb(wx, wz),
                );
            } else if radius <= HUD_BORDER_RADIUS + 0.5 {
                let c = if radius < HUD_TERRAIN_RADIUS + 1.2 {
                    [132, 144, 154]
                } else {
                    [29, 34, 39]
                };
                let coverage = (HUD_BORDER_RADIUS + 0.5 - radius).clamp(0.0, 1.0);
                set_pixel_alpha(
                    &mut rgba,
                    HUD_SIZE,
                    px as i32,
                    py as i32,
                    c,
                    (coverage * 255.0).round() as u8,
                );
            }
        }
    }
    rgba
}

#[test]
fn the_windowed_raster_matches_per_pixel_sampling_while_moving_and_turning() {
    let mut store = TileStore::default();
    for tz in -12i32..=12 {
        for tx in -12i32..=12 {
            if (tx * 7 + tz * 3).rem_euclid(11) == 0 {
                continue;
            }
            let mut tile = Tile::default();
            for (i, cell) in tile.cells.iter_mut().enumerate() {
                let (x, z) = (tx * 16 + (i % 16) as i32, tz * 16 + (i / 16) as i32);
                if (x * 5 + z).rem_euclid(13) != 0 {
                    *cell = Cell {
                        height: ((x * 3 - z * 7).rem_euclid(23) + 40) as i16,
                        rgb: [x.rem_euclid(251) as u8, z.rem_euclid(241) as u8, 90],
                    };
                }
            }
            store
                .tiles
                .insert((tx, tz), CachedTile::new(Box::new(tile)));
        }
    }
    let mut hud = HudRaster::default();
    for step in 0..120 {
        let t = step as f64;
        let player = [-70.3 + t * 1.37, 64.0, 31.9 - t * 0.83];
        let yaw = (step as f32 * 7.3).to_radians();
        let right = [-yaw.cos(), yaw.sin()];
        let forward = [yaw.sin(), yaw.cos()];
        if step == 60 {
            store.tiles.remove(&(0, 0));
            store.base_revision += 1;
        }
        assert!(
            hud.raster_terrain(&store, player, right, forward)
                == reference_terrain(&store, player, right, forward),
            "step {step}"
        );
    }
}
