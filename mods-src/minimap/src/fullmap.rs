//! The `M` full-map modal canvas: a retained 5×5 slot grid of reusable
//! 160×160 tile images, pan via the two-float view offset, tile
//! invalidation, the native-resolution player sprite, and pointer flow.

use crate::*;

pub(crate) const FULL_SIZE: usize = 640;
const FULL_TILE_IMAGE_PREFIX: &str = "minimap:full_tile_";
pub(crate) const PLAYER_ARROW_IMAGE: &str = "minimap:player_arrow";
const FULL_TILE_BLOCKS: i32 = 80;
const FULL_TILE_SIZE: usize = 160;
const FULL_TILE_GRID: i32 = 5;
pub(crate) const FULL_TILE_SLOTS: usize = (FULL_TILE_GRID * FULL_TILE_GRID) as usize;
const PLAYER_ARROW_SIZE: usize = 48;
pub(crate) const FULL_BLOCKS_PER_PIXEL: f32 = 0.5;
const FULL_TILE_TEXT_RUN_MAX: usize = 256;

#[derive(Copy, Clone, Default)]
pub(crate) struct FullTileSlot {
    pub(crate) coord: Option<(i32, i32)>,
    pub(crate) waypoint_revision: u64,
}

#[derive(Copy, Clone, PartialEq, Eq)]
pub(crate) struct FullSceneStamp {
    pub(crate) bounds: [i32; 4],
    pub(crate) player: [u32; 2],
}

struct FullWaypointLayout {
    marker: [i64; 2],
    label: [i64; 4],
    text: String,
    color: [u8; 3],
}

impl Minimap {
    pub(crate) fn sync_full_canvas(&mut self) {
        let bounds = full_tile_bounds(self.pan);
        let mut publish = Vec::new();
        for tz in bounds[2]..=bounds[3] {
            for tx in bounds[0]..=bounds[1] {
                let coord = (tx, tz);
                let slot = full_tile_slot(coord);
                let cached = self.full_tile_slots[slot];
                if cached.coord != Some(coord) || cached.waypoint_revision != self.waypoint_revision
                {
                    publish.push((coord, slot));
                }
            }
        }
        if !publish.is_empty() {
            self.ensure_full_tile_data(bounds);
            let waypoints = self.full_waypoint_layouts();
            for (coord, slot) in publish {
                let (rgba, text_runs) = self.render_full_tile(coord, &waypoints);
                let image_key = full_tile_image_key(slot);
                client_image_set(
                    &image_key,
                    FULL_TILE_SIZE as u16,
                    FULL_TILE_SIZE as u16,
                    rgba,
                );
                if !text_runs.is_empty() {
                    client_image_draw_texts(&image_key, text_runs);
                }
                self.full_tile_slots[slot] = FullTileSlot {
                    coord: Some(coord),
                    waypoint_revision: self.waypoint_revision,
                };
            }
        }

        self.publish_player_arrow_texture();
        let scene_stamp = FullSceneStamp {
            bounds,
            player: [self.player[0].to_bits(), self.player[2].to_bits()],
        };
        if self.full_scene_stamp != Some(scene_stamp) {
            let origin = [
                bounds[0] as f32 * FULL_TILE_BLOCKS as f32,
                bounds[2] as f32 * FULL_TILE_BLOCKS as f32,
            ];
            let mut elements = Vec::new();
            for tz in bounds[2]..=bounds[3] {
                for tx in bounds[0]..=bounds[1] {
                    let coord = (tx, tz);
                    elements.push(ClientCanvasElement::Image {
                        image_key: full_tile_image_key(full_tile_slot(coord)),
                        rect: [
                            (tx - bounds[0]) as f32 * FULL_TILE_SIZE as f32,
                            (tz - bounds[2]) as f32 * FULL_TILE_SIZE as f32,
                            FULL_TILE_SIZE as f32,
                            FULL_TILE_SIZE as f32,
                        ],
                    });
                }
            }
            elements.push(ClientCanvasElement::Sprite {
                image_key: PLAYER_ARROW_IMAGE.into(),
                center: [
                    (self.player[0] - origin[0]) / FULL_BLOCKS_PER_PIXEL,
                    (self.player[2] - origin[1]) / FULL_BLOCKS_PER_PIXEL,
                ],
            });
            client_canvas_scene_set(FULL_CANVAS, elements);
            self.full_scene_stamp = Some(scene_stamp);
        }

        let origin = [
            bounds[0] as f32 * FULL_TILE_BLOCKS as f32,
            bounds[2] as f32 * FULL_TILE_BLOCKS as f32,
        ];
        let half = FULL_SIZE as f32 * 0.5;
        let offset = [
            (half - (self.pan[0] - origin[0]) / FULL_BLOCKS_PER_PIXEL).round(),
            (half - (self.pan[1] - origin[1]) / FULL_BLOCKS_PER_PIXEL).round(),
        ];
        let view_bits = [offset[0].to_bits(), offset[1].to_bits()];
        if self.full_view_bits != Some(view_bits) {
            client_canvas_view_set(FULL_CANVAS, offset);
            self.full_view_bits = Some(view_bits);
        }
    }

    fn ensure_full_tile_data(&mut self, bounds: [i32; 4]) {
        let min_wx = bounds[0] * FULL_TILE_BLOCKS - 1;
        let max_wx = (bounds[1] + 1) * FULL_TILE_BLOCKS - 1;
        let min_wz = bounds[2] * FULL_TILE_BLOCKS - 1;
        let max_wz = (bounds[3] + 1) * FULL_TILE_BLOCKS - 1;
        let needed: BTreeSet<_> = (min_wz.div_euclid(16)..=max_wz.div_euclid(16))
            .flat_map(|cz| (min_wx.div_euclid(16)..=max_wx.div_euclid(16)).map(move |cx| (cx, cz)))
            .collect();
        self.ensure_tiles(&needed);
    }

    fn render_full_tile(
        &self,
        coord: (i32, i32),
        waypoints: &[FullWaypointLayout],
    ) -> (Vec<u8>, Vec<ClientTextRun>) {
        let mut rgba = vec![0u8; FULL_TILE_SIZE * FULL_TILE_SIZE * 4];
        let world_x = coord.0 * FULL_TILE_BLOCKS;
        let world_z = coord.1 * FULL_TILE_BLOCKS;
        for bz in 0..FULL_TILE_BLOCKS {
            for bx in 0..FULL_TILE_BLOCKS {
                let rgb = self.terrain_rgb(world_x + bx, world_z + bz);
                let px = bx * 2;
                let py = bz * 2;
                set_pixel(&mut rgba, FULL_TILE_SIZE, px, py, rgb);
                set_pixel(&mut rgba, FULL_TILE_SIZE, px + 1, py, rgb);
                set_pixel(&mut rgba, FULL_TILE_SIZE, px, py + 1, rgb);
                set_pixel(&mut rgba, FULL_TILE_SIZE, px + 1, py + 1, rgb);
            }
        }

        let tile_rect = [
            coord.0 as i64 * FULL_TILE_SIZE as i64,
            coord.1 as i64 * FULL_TILE_SIZE as i64,
            FULL_TILE_SIZE as i64,
            FULL_TILE_SIZE as i64,
        ];
        let mut text_runs = Vec::new();
        for waypoint in waypoints {
            let marker_rect = [waypoint.marker[0] - 9, waypoint.marker[1] - 9, 19, 19];
            if rects_intersect(marker_rect, tile_rect) {
                draw_diamond(
                    &mut rgba,
                    FULL_TILE_SIZE,
                    (waypoint.marker[0] - tile_rect[0]) as i32,
                    (waypoint.marker[1] - tile_rect[1]) as i32,
                    waypoint.color,
                );
            }
            if rects_intersect(waypoint.label, tile_rect)
                && text_runs.len() < FULL_TILE_TEXT_RUN_MAX
            {
                fill_rect(
                    &mut rgba,
                    FULL_TILE_SIZE,
                    (waypoint.label[0] - tile_rect[0]) as i32,
                    (waypoint.label[1] - tile_rect[1]) as i32,
                    waypoint.label[2] as i32,
                    waypoint.label[3] as i32,
                    [18, 22, 26],
                );
                text_runs.push(ClientTextRun {
                    text: waypoint.text.clone(),
                    position: [
                        (waypoint.label[0] + 2 - tile_rect[0]) as i32,
                        (waypoint.label[1] + 2 - tile_rect[1]) as i32,
                    ],
                    scale: 2,
                    color: [waypoint.color[0], waypoint.color[1], waypoint.color[2], 255],
                });
            }
        }
        (rgba, text_runs)
    }

    fn full_waypoint_layouts(&self) -> Vec<FullWaypointLayout> {
        self.waypoints
            .iter()
            .filter_map(|waypoint| {
                let text: String = waypoint.name.chars().take(48).collect();
                if text.is_empty() {
                    return None;
                }
                let [text_width, text_height] = client_text_measure(&text, 2);
                let marker = [
                    waypoint.pos[0] as i64 * 2 + 1,
                    waypoint.pos[2] as i64 * 2 + 1,
                ];
                let width = text_width as i64 + 4;
                let height = text_height as i64 + 4;
                Some(FullWaypointLayout {
                    marker,
                    label: [marker[0] + 12, marker[1] - height / 2, width, height],
                    text,
                    color: waypoint.color,
                })
            })
            .collect()
    }

    fn publish_player_arrow_texture(&mut self) {
        let yaw_bits = self.yaw.to_bits();
        if self.arrow_yaw_bits == Some(yaw_bits) {
            return;
        }
        client_image_set(
            PLAYER_ARROW_IMAGE,
            PLAYER_ARROW_SIZE as u16,
            PLAYER_ARROW_SIZE as u16,
            player_arrow_rgba(self.yaw),
        );
        self.arrow_yaw_bits = Some(yaw_bits);
    }

    pub(crate) fn invalidate_full_tile(&mut self, coord: (i32, i32)) {
        let slot = full_tile_slot(coord);
        if self.full_tile_slots[slot].coord == Some(coord) {
            self.full_tile_slots[slot].coord = None;
        }
    }

    pub(crate) fn map_pointer(&mut self, phase: ClientPointerPhase, x: f32, y: f32) {
        match phase {
            ClientPointerPhase::Down => {
                self.drag_start = Some(([x, y], self.pan));
                self.dragged = false;
            }
            ClientPointerPhase::Move => {
                let Some((start, pan)) = self.drag_start else {
                    return;
                };
                let dx = x - start[0];
                let dy = y - start[1];
                if dx * dx + dy * dy > 16.0 {
                    self.dragged = true;
                }
                let next = [
                    pan[0] - dx.round() * FULL_BLOCKS_PER_PIXEL,
                    pan[1] - dy.round() * FULL_BLOCKS_PER_PIXEL,
                ];
                if self.pan != next {
                    self.pan = next;
                    self.sync_full_canvas();
                }
            }
            ClientPointerPhase::Up => {
                if self.drag_start.take().is_some() && !self.dragged {
                    self.select_waypoint_at(x, y);
                }
                self.dragged = false;
            }
        }
    }
}

fn full_tile_bounds(pan: [f32; 2]) -> [i32; 4] {
    let axis = |center: f32| {
        let half_blocks = FULL_SIZE as f64 * FULL_BLOCKS_PER_PIXEL as f64 * 0.5;
        let tile_blocks = FULL_TILE_BLOCKS as f64;
        let min = ((center as f64 - half_blocks) / tile_blocks).floor() as i32;
        let max = ((center as f64 + half_blocks) / tile_blocks).ceil() as i32 - 1;
        (min, max)
    };
    let (min_x, max_x) = axis(pan[0]);
    let (min_z, max_z) = axis(pan[1]);
    [min_x, max_x, min_z, max_z]
}

pub(crate) fn full_tile_coord(wx: i32, wz: i32) -> (i32, i32) {
    (
        wx.div_euclid(FULL_TILE_BLOCKS),
        wz.div_euclid(FULL_TILE_BLOCKS),
    )
}

fn full_tile_slot((tx, tz): (i32, i32)) -> usize {
    // A viewport intersects at most five consecutive tiles per axis, so the
    // modulo grid is collision-free while letting the map roam without new keys.
    (tx.rem_euclid(FULL_TILE_GRID) + tz.rem_euclid(FULL_TILE_GRID) * FULL_TILE_GRID) as usize
}

fn full_tile_image_key(slot: usize) -> String {
    format!("{FULL_TILE_IMAGE_PREFIX}{slot}")
}

pub(crate) fn snap_half_block(value: f32) -> f32 {
    (value / FULL_BLOCKS_PER_PIXEL).round() * FULL_BLOCKS_PER_PIXEL
}

fn player_arrow_rgba(yaw: f32) -> Vec<u8> {
    let mut rgba = vec![0; PLAYER_ARROW_SIZE * PLAYER_ARROW_SIZE * 4];
    let mut arrow = vec![0; PLAYER_ARROW_SIZE * PLAYER_ARROW_SIZE * 4];
    let center = PLAYER_ARROW_SIZE as f32 * 0.5;
    let forward = [yaw.sin(), yaw.cos()];
    let right = [-forward[1], forward[0]];
    let point = |side: f32, along: f32| {
        [
            center + right[0] * side + forward[0] * along,
            center + right[1] * side + forward[1] * along,
        ]
    };
    let tip = point(0.0, 18.0);
    let left = point(-10.0, -7.0);
    let seam = point(0.0, -2.0);
    let right = point(10.0, -7.0);

    fill_player_pointer_faces(
        &mut arrow,
        PLAYER_ARROW_SIZE,
        [tip, seam, right],
        [tip, left, seam],
    );
    composite_alpha_mask(
        &mut rgba,
        PLAYER_ARROW_SIZE,
        &arrow,
        player_arrow_shadow_offset(),
        [0, 0, 0],
        128,
    );
    composite_rgba(&mut rgba, PLAYER_ARROW_SIZE, &arrow);
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_visible_full_map_tile_has_a_unique_reusable_slot() {
        for half_steps in -2000..=2000 {
            let pan = half_steps as f32 * FULL_BLOCKS_PER_PIXEL;
            let bounds = full_tile_bounds([pan, -pan]);
            assert!(bounds[1] - bounds[0] + 1 <= FULL_TILE_GRID);
            assert!(bounds[3] - bounds[2] + 1 <= FULL_TILE_GRID);

            let mut slots = BTreeSet::new();
            for tz in bounds[2]..=bounds[3] {
                for tx in bounds[0]..=bounds[1] {
                    assert!(slots.insert(full_tile_slot((tx, tz))));
                }
            }
        }
    }
}
