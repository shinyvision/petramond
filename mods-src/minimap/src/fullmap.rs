//! The `M` full-map modal canvas: a retained 5×5 slot grid of reusable
//! 160×160 tile images, pan via the two-float view offset, dirty-rect tile
//! invalidation with blit-based partial updates, the native-resolution
//! player sprite, and the pointer flow.

use crate::*;

pub(crate) const FULL_SIZE: usize = 640;
const FULL_TILE_IMAGE_PREFIX: &str = "minimap:full_tile_";
pub(crate) const PLAYER_ARROW_IMAGE: &str = "minimap:player_arrow";
pub(crate) const FULL_TILE_BLOCKS: i32 = 80;
const FULL_TILE_SIZE: usize = 160;
const FULL_TILE_GRID: i32 = 5;
pub(crate) const FULL_TILE_SLOTS: usize = (FULL_TILE_GRID * FULL_TILE_GRID) as usize;
const PLAYER_ARROW_SIZE: usize = 48;
pub(crate) const FULL_BLOCKS_PER_PIXEL: f32 = 0.5;
const FULL_TILE_TEXT_RUN_MAX: usize = 256;

#[derive(Copy, Clone, Default, Debug, PartialEq, Eq)]
pub(crate) struct FullTileSlot {
    pub(crate) coord: Option<(i32, i32)>,
    /// Tile-local block rect `[x0, z0, x1, z1)` whose raster is stale, or
    /// `None` when the cached image is current. Repainted as one blit.
    pub(crate) dirty: Option<[i32; 4]>,
}

#[derive(Copy, Clone, PartialEq, Eq)]
pub(crate) struct FullSceneStamp {
    pub(crate) bounds: [i32; 4],
    pub(crate) player: [u32; 2],
}

pub(crate) struct FullWaypointLayout {
    pub(crate) marker: [i64; 2],
    pub(crate) label: [i64; 4],
    pub(crate) text: String,
    pub(crate) color: [u8; 3],
}

impl Minimap {
    pub(crate) fn sync_full_canvas(&mut self) {
        let bounds = full_tile_bounds(self.pan);
        let mut full = Vec::new();
        let mut partial = Vec::new();
        for tz in bounds[2]..=bounds[3] {
            for tx in bounds[0]..=bounds[1] {
                let coord = (tx, tz);
                let slot = full_tile_slot(coord);
                let cached = self.full_tile_slots[slot];
                if cached.coord != Some(coord) {
                    full.push((coord, slot));
                } else if let Some(rect) = cached.dirty {
                    partial.push((coord, slot, rect));
                }
            }
        }
        if !full.is_empty() || !partial.is_empty() {
            self.ensure_full_tile_data(bounds);
            self.ensure_full_layouts();
            for (coord, slot) in full {
                let (rgba, text_runs) =
                    self.render_full_region(coord, [0, 0, FULL_TILE_BLOCKS, FULL_TILE_BLOCKS]);
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
                    dirty: None,
                };
            }
            for (coord, slot, rect) in partial {
                let (rgba, text_runs) = self.render_full_region(coord, rect);
                let image_key = full_tile_image_key(slot);
                client_image_blit(
                    &image_key,
                    [rect[0] as u16 * 2, rect[1] as u16 * 2],
                    [
                        (rect[2] - rect[0]) as u16 * 2,
                        (rect[3] - rect[1]) as u16 * 2,
                    ],
                    rgba,
                );
                if !text_runs.is_empty() {
                    client_image_draw_texts(&image_key, text_runs);
                }
                self.full_tile_slots[slot].dirty = None;
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

    /// Rasterize one block rect of a full-map tile (`rect` tile-local,
    /// `[x0, z0, x1, z1)`) at 2×2 px per block: relief-shaded terrain plus
    /// the waypoint markers and label fills that intersect it. Text runs are
    /// positioned in whole-tile image coordinates (the host draws them into
    /// the image after the pixels land), while pixels are region-local.
    /// Explored cells are gathered per 16×16 tile — one map lookup per tile,
    /// not per block.
    fn render_full_region(
        &self,
        coord: (i32, i32),
        rect: [i32; 4],
    ) -> (Vec<u8>, Vec<ClientTextRun>) {
        let wb = (rect[2] - rect[0]) as usize;
        let hb = (rect[3] - rect[1]) as usize;
        let width_px = wb * 2;
        let mut rgba = vec![0u8; width_px * hb * 2 * 4];

        // Gather the region plus a one-cell northwest apron (relief input).
        let gw = wb + 1;
        let gh = hb + 1;
        let gx0 = coord.0 * FULL_TILE_BLOCKS + rect[0] - 1;
        let gz0 = coord.1 * FULL_TILE_BLOCKS + rect[1] - 1;
        let mut cells = vec![Cell::default(); gw * gh];
        for tz in gz0.div_euclid(16)..=(gz0 + gh as i32 - 1).div_euclid(16) {
            for tx in gx0.div_euclid(16)..=(gx0 + gw as i32 - 1).div_euclid(16) {
                let Some(cached) = self.tiles.get(&(tx, tz)) else {
                    continue;
                };
                let x_lo = gx0.max(tx * 16);
                let x_hi = (gx0 + gw as i32).min(tx * 16 + 16);
                let z_lo = gz0.max(tz * 16);
                let z_hi = (gz0 + gh as i32).min(tz * 16 + 16);
                let len = (x_hi - x_lo) as usize;
                for wz in z_lo..z_hi {
                    let src = ((wz - tz * 16) * 16 + (x_lo - tx * 16)) as usize;
                    let dst = (wz - gz0) as usize * gw + (x_lo - gx0) as usize;
                    cells[dst..dst + len].copy_from_slice(&cached.tile.cells[src..src + len]);
                }
            }
        }

        for bz in 0..hb {
            for bx in 0..wb {
                let cell = cells[(bz + 1) * gw + (bx + 1)];
                let rgb = if cell.height == UNKNOWN_HEIGHT {
                    [0, 0, 0] // unexplored stays black
                } else {
                    let northwest = cells[bz * gw + bx].height;
                    let northwest = if northwest == UNKNOWN_HEIGHT {
                        cell.height
                    } else {
                        northwest
                    };
                    shade_rgb(cell.rgb, cell.height, northwest)
                };
                let px = (bx * 2) as i32;
                let py = (bz * 2) as i32;
                set_pixel(&mut rgba, width_px, px, py, rgb);
                set_pixel(&mut rgba, width_px, px + 1, py, rgb);
                set_pixel(&mut rgba, width_px, px, py + 1, rgb);
                set_pixel(&mut rgba, width_px, px + 1, py + 1, rgb);
            }
        }

        let tile_px = [
            coord.0 as i64 * FULL_TILE_SIZE as i64,
            coord.1 as i64 * FULL_TILE_SIZE as i64,
        ];
        let region_px = [
            tile_px[0] + rect[0] as i64 * 2,
            tile_px[1] + rect[1] as i64 * 2,
            width_px as i64,
            hb as i64 * 2,
        ];
        let mut text_runs = Vec::new();
        let waypoints = self
            .full_layouts
            .as_ref()
            .map(|(_, layouts)| layouts.as_slice())
            .unwrap_or(&[]);
        for waypoint in waypoints {
            let marker_rect = [waypoint.marker[0] - 9, waypoint.marker[1] - 9, 19, 19];
            if rects_intersect(marker_rect, region_px) {
                draw_diamond(
                    &mut rgba,
                    width_px,
                    (waypoint.marker[0] - region_px[0]) as i32,
                    (waypoint.marker[1] - region_px[1]) as i32,
                    waypoint.color,
                );
            }
            if rects_intersect(waypoint.label, region_px)
                && text_runs.len() < FULL_TILE_TEXT_RUN_MAX
            {
                fill_rect(
                    &mut rgba,
                    width_px,
                    (waypoint.label[0] - region_px[0]) as i32,
                    (waypoint.label[1] - region_px[1]) as i32,
                    waypoint.label[2] as i32,
                    waypoint.label[3] as i32,
                    [18, 22, 26],
                );
                text_runs.push(ClientTextRun {
                    text: waypoint.text.clone(),
                    position: [
                        (waypoint.label[0] + 2 - tile_px[0]) as i32,
                        (waypoint.label[1] + 2 - tile_px[1]) as i32,
                    ],
                    scale: 2,
                    color: [waypoint.color[0], waypoint.color[1], waypoint.color[2], 255],
                });
            }
        }
        (rgba, text_runs)
    }

    /// Rebuild the cached waypoint layouts when the waypoint set changed.
    /// Text measurement is a host call — it runs per waypoint edit, never
    /// per publish.
    pub(crate) fn ensure_full_layouts(&mut self) {
        if self
            .full_layouts
            .as_ref()
            .is_some_and(|(revision, _)| *revision == self.waypoint_revision)
        {
            return;
        }
        let mut layouts = Vec::new();
        for index in 0..self.waypoints.len() {
            let (name, pos, color) = {
                let waypoint = &self.waypoints[index];
                (waypoint.name.clone(), waypoint.pos, waypoint.color)
            };
            if let Some(layout) = self.build_waypoint_layout(&name, pos, color) {
                layouts.push(layout);
            }
        }
        self.full_layouts = Some((self.waypoint_revision, layouts));
    }

    fn build_waypoint_layout(
        &mut self,
        name: &str,
        pos: [i32; 3],
        color: [u8; 3],
    ) -> Option<FullWaypointLayout> {
        let text: String = name.chars().take(48).collect();
        if text.is_empty() {
            return None;
        }
        let [text_width, text_height] = self.measure_cached(&text);
        let marker = [pos[0] as i64 * 2 + 1, pos[2] as i64 * 2 + 1];
        let width = text_width as i64 + 4;
        let height = text_height as i64 + 4;
        Some(FullWaypointLayout {
            marker,
            label: [marker[0] + 12, marker[1] - height / 2, width, height],
            text,
            color,
        })
    }

    /// Union a world-space block rect (`[x0, z0, x1, z1)`, already including
    /// any relief fringe) into the dirty rect of every currently cached
    /// full-map tile it touches.
    pub(crate) fn mark_full_tiles_dirty(&mut self, rect: [i32; 4]) {
        if rect[0] >= rect[2] || rect[1] >= rect[3] {
            return;
        }
        for tz in rect[1].div_euclid(FULL_TILE_BLOCKS)..=(rect[3] - 1).div_euclid(FULL_TILE_BLOCKS)
        {
            for tx in
                rect[0].div_euclid(FULL_TILE_BLOCKS)..=(rect[2] - 1).div_euclid(FULL_TILE_BLOCKS)
            {
                let coord = (tx, tz);
                let slot = full_tile_slot(coord);
                if self.full_tile_slots[slot].coord != Some(coord) {
                    continue;
                }
                let local = [
                    (rect[0] - tx * FULL_TILE_BLOCKS).max(0),
                    (rect[1] - tz * FULL_TILE_BLOCKS).max(0),
                    (rect[2] - tx * FULL_TILE_BLOCKS).min(FULL_TILE_BLOCKS),
                    (rect[3] - tz * FULL_TILE_BLOCKS).min(FULL_TILE_BLOCKS),
                ];
                let dirty = &mut self.full_tile_slots[slot].dirty;
                *dirty = Some(match *dirty {
                    None => local,
                    Some(d) => [
                        d[0].min(local[0]),
                        d[1].min(local[1]),
                        d[2].max(local[2]),
                        d[3].max(local[3]),
                    ],
                });
            }
        }
    }

    /// Dirty the map area one waypoint occupies (marker + label), so edits
    /// repaint only the tiles they touch instead of every visible tile.
    pub(crate) fn invalidate_waypoint_area(&mut self, name: &str, pos: [i32; 3], color: [u8; 3]) {
        let Some(layout) = self.build_waypoint_layout(name, pos, color) else {
            return;
        };
        let marker_rect = [layout.marker[0] - 9, layout.marker[1] - 9, 19, 19];
        for px_rect in [marker_rect, layout.label] {
            self.mark_full_tiles_dirty([
                px_rect[0].div_euclid(2) as i32,
                px_rect[1].div_euclid(2) as i32,
                ((px_rect[0] + px_rect[2] + 1).div_euclid(2) + 1) as i32,
                ((px_rect[1] + px_rect[3] + 1).div_euclid(2) + 1) as i32,
            ]);
        }
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

pub(crate) fn full_tile_slot((tx, tz): (i32, i32)) -> usize {
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

    fn synthetic_map() -> Minimap {
        let mut map = Minimap::default();
        for tz in -2..=6 {
            for tx in -2..=6 {
                let mut tile = Tile::default();
                for i in 0..256 {
                    let wx = tx * 16 + (i % 16) as i32;
                    let wz = tz * 16 + (i / 16) as i32;
                    if (wx + wz).rem_euclid(7) == 0 {
                        continue; // holes exercise the unknown paths
                    }
                    tile.cells[i] = Cell {
                        height: ((wx * 3 + wz * 5).rem_euclid(60)) as i16,
                        rgb: [
                            (wx.rem_euclid(251)) as u8,
                            (wz.rem_euclid(241)) as u8,
                            200,
                        ],
                    };
                }
                map.tiles.insert(
                    (tx, tz),
                    CachedTile {
                        tile,
                        last_used: 0,
                        watermark: 0,
                        dirty: false,
                    },
                );
            }
        }
        map
    }

    #[test]
    fn region_raster_matches_the_full_tile_raster() {
        // The blit path repaints sub-rects of a tile; any drift against the
        // whole-tile raster leaves visible seams.
        let map = synthetic_map();
        let (full, _) =
            map.render_full_region((0, 0), [0, 0, FULL_TILE_BLOCKS, FULL_TILE_BLOCKS]);
        for rect in [[0, 0, 40, 40], [40, 0, 80, 40], [17, 23, 61, 59], [0, 79, 80, 80]] {
            let (part, _) = map.render_full_region((0, 0), rect);
            let wb = (rect[2] - rect[0]) as usize;
            for bz in 0..(rect[3] - rect[1]) as usize * 2 {
                for bx in 0..wb * 2 {
                    let full_at =
                        ((rect[1] as usize * 2 + bz) * FULL_TILE_SIZE + rect[0] as usize * 2 + bx)
                            * 4;
                    let part_at = (bz * wb * 2 + bx) * 4;
                    assert_eq!(
                        &part[part_at..part_at + 4],
                        &full[full_at..full_at + 4],
                        "pixel ({bx}, {bz}) of region {rect:?} must match the full raster"
                    );
                }
            }
        }
    }

    #[test]
    fn explored_changes_dirty_only_matching_slots() {
        let mut map = Minimap::default();
        let a = (0, 0);
        let b = (1, 0);
        map.full_tile_slots[full_tile_slot(a)] = FullTileSlot {
            coord: Some(a),
            dirty: None,
        };
        map.full_tile_slots[full_tile_slot(b)] = FullTileSlot {
            coord: Some(b),
            dirty: None,
        };
        // A change at tile a's east edge; its +1 relief fringe crosses into b.
        map.mark_full_tiles_dirty([79, 10, 81, 12]);
        assert_eq!(
            map.full_tile_slots[full_tile_slot(a)].dirty,
            Some([79, 10, 80, 12])
        );
        assert_eq!(
            map.full_tile_slots[full_tile_slot(b)].dirty,
            Some([0, 10, 1, 12])
        );
        // Rects union; a slot caching another coord stays untouched.
        map.mark_full_tiles_dirty([0, 0, 2, 2]);
        assert_eq!(
            map.full_tile_slots[full_tile_slot(a)].dirty,
            Some([0, 0, 80, 12])
        );
        let elsewhere = (2, 0);
        map.full_tile_slots[full_tile_slot(elsewhere)] = FullTileSlot {
            coord: Some((7, 7)),
            dirty: None,
        };
        map.mark_full_tiles_dirty([170, 0, 172, 2]);
        assert_eq!(map.full_tile_slots[full_tile_slot(elsewhere)].dirty, None);
    }
}
