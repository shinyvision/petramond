//! The `M` full-map modal canvas: a retained 6×6 slot grid of reusable
//! 160×160 tile images, pan via the two-float view offset, mouse-wheel zoom,
//! dirty-rect tile invalidation with blit-based partial updates, the
//! native-resolution player sprite, and the pointer flow.
//!
//! Zoom levels −2..=+2 render 0.5 / 1 / 2 (default) / 4 / 8 canvas pixels per
//! block. Every level rasterizes explored cells natively — never by scaling a
//! finished image. Tile images keep a fixed pixel size; their BLOCK coverage
//! scales with zoom, so tile coordinates are zoom-scoped and a zoom change
//! invalidates every cached slot. At the outermost level one pixel covers a
//! 2×2-block cell whose color is the HSL average of its known blocks.

use crate::*;

pub(crate) const FULL_SIZE: usize = 800;
const FULL_TILE_IMAGE_PREFIX: &str = "minimap:full_tile_";
pub(crate) const PLAYER_ARROW_IMAGE: &str = "minimap:player_arrow";
const FULL_TILE_SIZE: usize = 160;
const FULL_TILE_GRID: i32 = 6;
pub(crate) const FULL_TILE_SLOTS: usize = (FULL_TILE_GRID * FULL_TILE_GRID) as usize;
const PLAYER_ARROW_SIZE: usize = 48;
const FULL_TILE_TEXT_RUN_MAX: usize = 256;
pub(crate) const ZOOM_MIN: i8 = -2;
pub(crate) const ZOOM_MAX: i8 = 2;

/// Blocks per raster cell edge: 2 at the outermost zoom (one canvas pixel
/// covers a 2×2-block cell), 1 everywhere else.
pub(crate) fn cell_blocks(zoom: i8) -> i32 {
    if zoom <= -2 {
        2
    } else {
        1
    }
}

/// Canvas pixels per raster cell edge.
pub(crate) fn cell_px(zoom: i8) -> i32 {
    match zoom {
        i8::MIN..=-1 => 1,
        0 => 2,
        1 => 4,
        _ => 8,
    }
}

pub(crate) fn blocks_per_pixel(zoom: i8) -> f32 {
    cell_blocks(zoom) as f32 / cell_px(zoom) as f32
}

/// World blocks covered by one full-map tile image edge at this zoom.
pub(crate) fn full_tile_blocks(zoom: i8) -> i32 {
    FULL_TILE_SIZE as i32 / cell_px(zoom) * cell_blocks(zoom)
}

/// Raster work budget per sync, in output pixels (~12 tile images). Every
/// level rasters with plain copies now (the outermost renders from
/// write-time mips); the budget paces publish bursts so a grid fill spreads
/// over ~3 frames instead of spiking one.
fn raster_px_budget(_zoom: i8) -> usize {
    320_000
}

/// The source store one zoom level rasters from: base tiles (one cell per
/// block) or, at the outermost level, mip tiles (one cell per 2×2 blocks).
pub(crate) fn source_kind(zoom: i8) -> RegionKind {
    if cell_blocks(zoom) == 2 {
        RegionKind::Mip
    } else {
        RegionKind::Base
    }
}

#[derive(Copy, Clone, Default, Debug, PartialEq, Eq)]
pub(crate) struct FullTileSlot {
    pub(crate) coord: Option<(i32, i32)>,
    /// Tile-local block rect `[x0, z0, x1, z1)` whose raster is stale, or
    /// `None` when the cached image is current. Repainted as one blit.
    pub(crate) dirty: Option<[i32; 4]>,
}

/// Fixed slot array behind a wrapper only because `Default` does not derive
/// for arrays this long.
pub(crate) struct FullTileSlots([FullTileSlot; FULL_TILE_SLOTS]);

impl Default for FullTileSlots {
    fn default() -> Self {
        Self([FullTileSlot::default(); FULL_TILE_SLOTS])
    }
}

impl std::ops::Deref for FullTileSlots {
    type Target = [FullTileSlot];
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for FullTileSlots {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
pub(crate) struct FullSceneStamp {
    pub(crate) bounds: [i32; 4],
    pub(crate) player: [u32; 2],
    pub(crate) zoom: i8,
    /// Which visible grid positions have a rastered tile (budgeted rasters
    /// and progressive loads complete across frames).
    pub(crate) present: u64,
}

pub(crate) struct FullWaypointLayout {
    pub(crate) marker: [i64; 2],
    pub(crate) label: [i64; 4],
    pub(crate) text: String,
    pub(crate) color: [u8; 3],
}

impl Minimap {
    pub(crate) fn sync_full_canvas(&mut self) {
        let zoom = self.zoom;
        let bounds = full_tile_bounds(self.pan, zoom);
        self.request_visible_regions(bounds);

        let mut full = Vec::new();
        let mut partial = Vec::new();
        for tz in bounds[2]..=bounds[3] {
            for tx in bounds[0]..=bounds[1] {
                let coord = (tx, tz);
                let slot = full_tile_slot(coord);
                let cached = self.full_tile_slots[slot];
                if cached.coord != Some(coord) {
                    // Paint once: hold a slot's first raster until every
                    // source region under it has resolved (resident or
                    // absent) — never raster holes just to repaint them.
                    if self.slot_source_ready(coord) {
                        full.push((coord, slot));
                    }
                } else if let Some(rect) = cached.dirty {
                    partial.push((coord, slot, rect));
                }
            }
        }
        if !full.is_empty() || !partial.is_empty() {
            self.ensure_full_layouts();
            let tb = full_tile_blocks(zoom);
            let cb = cell_blocks(zoom);
            let cp = cell_px(zoom);
            // A zoom change invalidates the whole visible grid; repainting it
            // in one frame would hitch, so raster work is budgeted and
            // deferred slots stay pending until the next frames.
            let mut budget = raster_px_budget(zoom) as i64;
            const TILE_PX: i64 = (FULL_TILE_SIZE * FULL_TILE_SIZE) as i64;
            for (coord, slot) in full {
                if budget <= 0 {
                    break;
                }
                budget -= TILE_PX;
                let (rgba, text_runs) = self.render_full_region(coord, [0, 0, tb, tb]);
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
                if budget <= 0 {
                    break;
                }
                let rect = align_rect_to_cells(rect, zoom);
                let size_px = [
                    (rect[2] - rect[0]) / cb * cp,
                    (rect[3] - rect[1]) / cb * cp,
                ];
                budget -= size_px[0] as i64 * size_px[1] as i64;
                let (rgba, text_runs) = self.render_full_region(coord, rect);
                let image_key = full_tile_image_key(slot);
                client_image_blit(
                    &image_key,
                    [(rect[0] / cb * cp) as u16, (rect[1] / cb * cp) as u16],
                    [size_px[0] as u16, size_px[1] as u16],
                    rgba,
                );
                if !text_runs.is_empty() {
                    client_image_draw_texts(&image_key, text_runs);
                }
                self.full_tile_slots[slot].dirty = None;
            }
        }

        self.publish_player_arrow_texture();
        let mut present = 0u64;
        let mut bit = 0;
        for tz in bounds[2]..=bounds[3] {
            for tx in bounds[0]..=bounds[1] {
                let coord = (tx, tz);
                if self.full_tile_slots[full_tile_slot(coord)].coord == Some(coord) {
                    present |= 1 << bit;
                }
                bit += 1;
            }
        }
        let tb = full_tile_blocks(zoom);
        let bpp = blocks_per_pixel(zoom);
        let origin = [(bounds[0] * tb) as f32, (bounds[2] * tb) as f32];
        let scene_stamp = FullSceneStamp {
            bounds,
            player: [self.player[0].to_bits(), self.player[2].to_bits()],
            zoom,
            present,
        };
        if self.full_scene_stamp != Some(scene_stamp) {
            let mut elements = Vec::new();
            for tz in bounds[2]..=bounds[3] {
                for tx in bounds[0]..=bounds[1] {
                    let coord = (tx, tz);
                    let slot = full_tile_slot(coord);
                    if self.full_tile_slots[slot].coord != Some(coord) {
                        continue; // pending raster: no stale pixels on screen
                    }
                    elements.push(ClientCanvasElement::Image {
                        image_key: full_tile_image_key(slot),
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
                    (self.player[0] - origin[0]) / bpp,
                    (self.player[2] - origin[1]) / bpp,
                ],
            });
            client_canvas_scene_set(FULL_CANVAS, elements);
            self.full_scene_stamp = Some(scene_stamp);
        }

        let half = FULL_SIZE as f32 * 0.5;
        let offset = [
            (half - (self.pan[0] - origin[0]) / bpp).round(),
            (half - (self.pan[1] - origin[1]) / bpp).round(),
        ];
        let view_bits = [offset[0].to_bits(), offset[1].to_bits()];
        if self.full_view_bits != Some(view_bits) {
            client_canvas_view_set(FULL_CANVAS, offset);
            self.full_view_bits = Some(view_bits);
        }
    }

    /// Step the zoom level, keeping the world point under canvas pixel
    /// (`x`, `y`) fixed.
    pub(crate) fn zoom_step(&mut self, steps: i32, x: f32, y: f32) {
        let target = (self.zoom as i32 + steps).clamp(ZOOM_MIN as i32, ZOOM_MAX as i32) as i8;
        if target == self.zoom {
            return;
        }
        self.pan = zoomed_pan(self.pan, self.zoom, target, x, y);
        self.zoom = target;
        // Tile coordinates are zoom-scoped: every cached slot raster is stale.
        self.full_tile_slots = FullTileSlots::default();
        self.full_scene_stamp = None;
        self.full_view_bits = None;
        self.full_needed_stamp = None;
        self.drag_start = None;
        self.dragged = false;
        self.sync_full_canvas();
    }

    /// Whether every source region under one full-map tile (plus its
    /// northwest relief apron) has resolved — resident or known-absent.
    fn slot_source_ready(&self, coord: (i32, i32)) -> bool {
        let kind = source_kind(self.zoom);
        let tb = full_tile_blocks(self.zoom);
        let cb = cell_blocks(self.zoom);
        let span = region_block_rect(kind, (0, 0))[2];
        let min_wx = coord.0 * tb - cb;
        let max_wx = (coord.0 + 1) * tb - 1;
        let min_wz = coord.1 * tb - cb;
        let max_wz = (coord.1 + 1) * tb - 1;
        for rz in min_wz.div_euclid(span)..=max_wz.div_euclid(span) {
            for rx in min_wx.div_euclid(span)..=max_wx.div_euclid(span) {
                if self.store.region_pending(kind, (rx, rz)) {
                    return false;
                }
            }
        }
        true
    }

    /// World-block rect `[x0, z0, x1, z1)` covering the open full map's
    /// visible tile grid plus every prefetch band `request_visible_regions`
    /// can reach (one MIP-region span — the larger kind — on all sides
    /// covers the velocity band and the adjacent-zoom store alike). This is
    /// the region working set the cache trim must protect while the map is
    /// open; see `pump_store`.
    pub(crate) fn full_view_world_rect(&self) -> [i32; 4] {
        let bounds = full_tile_bounds(self.pan, self.zoom);
        let tb = full_tile_blocks(self.zoom);
        let span = region_block_rect(RegionKind::Mip, (0, 0))[2];
        [
            bounds[0] * tb - span,
            bounds[2] * tb - span,
            (bounds[1] + 1) * tb + span,
            (bounds[3] + 1) * tb + span,
        ]
    }

    /// Queue async loads for every source region the visible bounds need
    /// (base regions, or mip regions at the outermost zoom), plus two
    /// prefetch layers: one region band beyond the edge the pan is moving
    /// toward (data lands before it's exposed), and the ADJACENT zoom level's
    /// store under the viewport (a wheel step paints instantly). Idempotent
    /// per (bounds, zoom); arrivals repaint through the store's pump.
    fn request_visible_regions(&mut self, bounds: [i32; 4]) {
        let stamp = (bounds, self.zoom);
        if self.full_needed_stamp == Some(stamp) {
            return;
        }
        self.full_needed_stamp = Some(stamp);
        let kind = source_kind(self.zoom);
        let cb = cell_blocks(self.zoom);
        let tb = full_tile_blocks(self.zoom);
        let span = region_block_rect(kind, (0, 0))[2];
        let min_wx = bounds[0] * tb - cb;
        let max_wx = (bounds[1] + 1) * tb - 1 + cb;
        let min_wz = bounds[2] * tb - cb;
        let max_wz = (bounds[3] + 1) * tb - 1 + cb;
        for rz in min_wz.div_euclid(span)..=max_wz.div_euclid(span) {
            for rx in min_wx.div_euclid(span)..=max_wx.div_euclid(span) {
                self.store.request_region(kind, (rx, rz), LoadTier::Visible);
            }
        }

        // Velocity prefetch: the band one region beyond the moving edge(s).
        let velocity = match self.last_synced_pan {
            Some(last) => [self.pan[0] - last[0], self.pan[1] - last[1]],
            None => [0.0, 0.0],
        };
        self.last_synced_pan = Some(self.pan);
        let (mut px0, mut px1, mut pz0, mut pz1) = (
            min_wx.div_euclid(span),
            max_wx.div_euclid(span),
            min_wz.div_euclid(span),
            max_wz.div_euclid(span),
        );
        if velocity[0] > 0.5 {
            px1 += 1;
        } else if velocity[0] < -0.5 {
            px0 -= 1;
        }
        if velocity[1] > 0.5 {
            pz1 += 1;
        } else if velocity[1] < -0.5 {
            pz0 -= 1;
        }
        for rz in pz0..=pz1 {
            for rx in px0..=px1 {
                self.store.request_region(kind, (rx, rz), LoadTier::Prefetch);
            }
        }

        // Adjacent-zoom prefetch: the level one wheel step away renders from
        // the OTHER store exactly at the −1/−2 boundary.
        let other = match self.zoom {
            -2 => Some(RegionKind::Base),
            -1 => Some(RegionKind::Mip),
            _ => None,
        };
        if let Some(other) = other {
            let other_span = region_block_rect(other, (0, 0))[2];
            for rz in min_wz.div_euclid(other_span)..=max_wz.div_euclid(other_span) {
                for rx in min_wx.div_euclid(other_span)..=max_wx.div_euclid(other_span) {
                    self.store
                        .request_region(other, (rx, rz), LoadTier::Prefetch);
                }
            }
        }
    }

    /// Rasterize one block rect of a full-map tile (`rect` tile-local,
    /// `[x0, z0, x1, z1)`, cell-aligned): relief-shaded terrain plus the
    /// waypoint markers and label fills that intersect it. Text runs are
    /// positioned in whole-tile image coordinates (the host draws them into
    /// the image after the pixels land), while pixels are region-local.
    ///
    /// Every zoom level renders as plain per-cell copies from its SOURCE
    /// store — base tiles, or write-time mip tiles at the outermost level —
    /// gathered per 16×16 source tile (one map lookup per tile, not per
    /// cell).
    fn render_full_region(
        &self,
        coord: (i32, i32),
        rect: [i32; 4],
    ) -> (Vec<u8>, Vec<ClientTextRun>) {
        let zoom = self.zoom;
        let cb = cell_blocks(zoom);
        let cp = cell_px(zoom) as usize;
        let source = match source_kind(zoom) {
            RegionKind::Base => &self.store.tiles,
            RegionKind::Mip => &self.store.mips,
        };
        // Everything below runs in SOURCE-CELL space (cells of `cb` blocks;
        // source tiles are 16×16 cells in both stores).
        let wc = ((rect[2] - rect[0]) / cb) as usize;
        let hc = ((rect[3] - rect[1]) / cb) as usize;
        let width_px = wc * cp;
        let mut rgba = vec![0u8; width_px * hc * cp * 4];

        // Gather the region plus a one-cell northwest apron (relief input).
        let gw = wc + 1;
        let gh = hc + 1;
        let gx0 = (coord.0 * full_tile_blocks(zoom) + rect[0]) / cb - 1;
        let gz0 = (coord.1 * full_tile_blocks(zoom) + rect[1]) / cb - 1;
        let mut cells = vec![Cell::default(); gw * gh];
        for tz in gz0.div_euclid(16)..=(gz0 + gh as i32 - 1).div_euclid(16) {
            for tx in gx0.div_euclid(16)..=(gx0 + gw as i32 - 1).div_euclid(16) {
                let Some(cached) = source.get(&(tx, tz)) else {
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

        // Build each cell row once into a scratch line, then duplicate the
        // whole line for the cell's remaining pixel rows — the classic
        // integer-scaler shape, no per-pixel index math.
        let mut line = vec![0u8; width_px * 4];
        for cz in 0..hc {
            for cx in 0..wc {
                let cell = cells[(cz + 1) * gw + cx + 1];
                let px = if cell.height == UNKNOWN_HEIGHT {
                    [0, 0, 0, 255] // unexplored stays black
                } else {
                    let northwest = cells[cz * gw + cx].height;
                    let northwest = if northwest == UNKNOWN_HEIGHT {
                        cell.height
                    } else {
                        northwest
                    };
                    let [r, g, b] = shade_rgb(cell.rgb, cell.height, northwest);
                    [r, g, b, 255]
                };
                for dx in 0..cp {
                    line[(cx * cp + dx) * 4..][..4].copy_from_slice(&px);
                }
            }
            for dy in 0..cp {
                rgba[(cz * cp + dy) * width_px * 4..][..width_px * 4].copy_from_slice(&line);
            }
        }

        let tile_px = [
            coord.0 as i64 * FULL_TILE_SIZE as i64,
            coord.1 as i64 * FULL_TILE_SIZE as i64,
        ];
        let region_px = [
            tile_px[0] + rect[0] as i64 / cb as i64 * cp as i64,
            tile_px[1] + rect[1] as i64 / cb as i64 * cp as i64,
            width_px as i64,
            (hc * cp) as i64,
        ];
        let mut text_runs = Vec::new();
        let waypoints = self
            .full_layouts
            .as_ref()
            .map(|(_, _, layouts)| layouts.as_slice())
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

    /// Rebuild the cached waypoint layouts when the waypoint set or the zoom
    /// changed. Text measurement is a host call — it runs per waypoint edit,
    /// never per publish.
    pub(crate) fn ensure_full_layouts(&mut self) {
        if self
            .full_layouts
            .as_ref()
            .is_some_and(|(revision, zoom, _)| {
                *revision == self.waypoint_revision && *zoom == self.zoom
            })
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
        self.full_layouts = Some((self.waypoint_revision, self.zoom, layouts));
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
        let cb = cell_blocks(self.zoom) as i64;
        let cp = cell_px(self.zoom) as i64;
        let marker = [
            (pos[0] as i64 * cp).div_euclid(cb) + cp / 2,
            (pos[2] as i64 * cp).div_euclid(cb) + cp / 2,
        ];
        let width = text_width as i64 + 4;
        let height = text_height as i64 + 4;
        Some(FullWaypointLayout {
            marker,
            label: [marker[0] + 12, marker[1] - height / 2, width, height],
            text,
            color,
        })
    }

    /// Union a world-space rect of CHANGED blocks (`[x0, z0, x1, z1)`) into
    /// the dirty rect of every currently cached full-map tile whose raster it
    /// stales. The southeast relief fringe — and, at the outermost zoom, the
    /// rest of a changed block's 2×2-block cell — is added here, not by
    /// callers.
    pub(crate) fn mark_full_tiles_dirty(&mut self, rect: [i32; 4]) {
        if rect[0] >= rect[2] || rect[1] >= rect[3] {
            return;
        }
        let cb = cell_blocks(self.zoom);
        let fringe = 2 * cb - 1;
        let rect = [rect[0], rect[1], rect[2] + fringe, rect[3] + fringe];
        let tb = full_tile_blocks(self.zoom);
        for tz in rect[1].div_euclid(tb)..=(rect[3] - 1).div_euclid(tb) {
            for tx in rect[0].div_euclid(tb)..=(rect[2] - 1).div_euclid(tb) {
                let coord = (tx, tz);
                let slot = full_tile_slot(coord);
                if self.full_tile_slots[slot].coord != Some(coord) {
                    continue;
                }
                let local = [
                    (rect[0] - tx * tb).max(0),
                    (rect[1] - tz * tb).max(0),
                    (rect[2] - tx * tb).min(tb),
                    (rect[3] - tz * tb).min(tb),
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
        let cb = cell_blocks(self.zoom) as i64;
        let cp = cell_px(self.zoom) as i64;
        let marker_rect = [layout.marker[0] - 9, layout.marker[1] - 9, 19, 19];
        for px_rect in [marker_rect, layout.label] {
            self.mark_full_tiles_dirty([
                (px_rect[0] * cb).div_euclid(cp) as i32,
                (px_rect[1] * cb).div_euclid(cp) as i32,
                ((px_rect[0] + px_rect[2]) * cb + cp - 1).div_euclid(cp) as i32 + 1,
                ((px_rect[1] + px_rect[3]) * cb + cp - 1).div_euclid(cp) as i32 + 1,
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
                let bpp = blocks_per_pixel(self.zoom);
                let next = [pan[0] - dx.round() * bpp, pan[1] - dy.round() * bpp];
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

fn full_tile_bounds(pan: [f32; 2], zoom: i8) -> [i32; 4] {
    let axis = |center: f32| {
        let half_blocks = FULL_SIZE as f64 * blocks_per_pixel(zoom) as f64 * 0.5;
        let tile_blocks = full_tile_blocks(zoom) as f64;
        let min = ((center as f64 - half_blocks) / tile_blocks).floor() as i32;
        let max = ((center as f64 + half_blocks) / tile_blocks).ceil() as i32 - 1;
        (min, max)
    };
    let (min_x, max_x) = axis(pan[0]);
    let (min_z, max_z) = axis(pan[1]);
    [min_x, max_x, min_z, max_z]
}

pub(crate) fn full_tile_slot((tx, tz): (i32, i32)) -> usize {
    // A viewport intersects at most six consecutive tiles per axis, so the
    // modulo grid is collision-free while letting the map roam without new keys.
    (tx.rem_euclid(FULL_TILE_GRID) + tz.rem_euclid(FULL_TILE_GRID) * FULL_TILE_GRID) as usize
}

fn full_tile_image_key(slot: usize) -> String {
    format!("{FULL_TILE_IMAGE_PREFIX}{slot}")
}

/// Round a block rect outward to whole raster cells, so a blit lands on the
/// tile image's pixel grid at every zoom.
fn align_rect_to_cells(rect: [i32; 4], zoom: i8) -> [i32; 4] {
    let cb = cell_blocks(zoom);
    [
        rect[0].div_euclid(cb) * cb,
        rect[1].div_euclid(cb) * cb,
        (rect[2] + cb - 1).div_euclid(cb) * cb,
        (rect[3] + cb - 1).div_euclid(cb) * cb,
    ]
}

pub(crate) fn snap_to_source_pixel(value: f32, zoom: i8) -> f32 {
    let bpp = blocks_per_pixel(zoom);
    (value / bpp).round() * bpp
}

/// The pan that keeps the world point under canvas pixel (`x`, `y`) fixed
/// across a zoom change, snapped to the target zoom's source-pixel grid.
pub(crate) fn zoomed_pan(pan: [f32; 2], from: i8, to: i8, x: f32, y: f32) -> [f32; 2] {
    let half = FULL_SIZE as f32 * 0.5;
    let shift = blocks_per_pixel(from) - blocks_per_pixel(to);
    [
        snap_to_source_pixel(pan[0] + (x - half) * shift, to),
        snap_to_source_pixel(pan[1] + (y - half) * shift, to),
    ]
}

fn player_arrow_rgba(yaw: f32) -> Vec<u8> {
    let mut rgba = vec![0; PLAYER_ARROW_SIZE * PLAYER_ARROW_SIZE * 4];
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

    stamp_arrow_with_shadow(
        &mut rgba,
        PLAYER_ARROW_SIZE,
        [tip, seam, right],
        [tip, left, seam],
    );
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_visible_full_map_tile_has_a_unique_reusable_slot() {
        for zoom in ZOOM_MIN..=ZOOM_MAX {
            let bpp = blocks_per_pixel(zoom);
            for half_steps in -2000..=2000 {
                let pan = half_steps as f32 * bpp;
                let bounds = full_tile_bounds([pan, -pan], zoom);
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

    /// Deterministic cell pattern shared by both stores; `holes` exercises
    /// the unknown paths.
    fn synthetic_cell(x: i32, z: i32) -> Cell {
        if (x + z).rem_euclid(7) == 0 {
            return Cell::default();
        }
        Cell {
            height: ((x * 3 + z * 5).rem_euclid(60)) as i16,
            rgb: [(x.rem_euclid(251)) as u8, (z.rem_euclid(241)) as u8, 200],
        }
    }

    fn synthetic_map() -> Minimap {
        let mut map = Minimap::default();
        // Cover the outermost zoom's tile (0, 0) in both stores: base cells
        // for blocks −2..320+, mip cells over the same span.
        for tz in -2..=21 {
            for tx in -2..=21 {
                let mut tile = Tile::default();
                let mut mip = Tile::default();
                for i in 0..256usize {
                    let (lx, lz) = ((i % 16) as i32, (i / 16) as i32);
                    tile.cells[i] = synthetic_cell(tx * 16 + lx, tz * 16 + lz);
                    // Mip tiles live in mip-cell space (one cell per 2×2
                    // blocks); any deterministic pattern works for parity.
                    mip.cells[i] = synthetic_cell(tx * 16 + lx + 1000, tz * 16 + lz - 1000);
                }
                map.store
                    .tiles
                    .insert((tx, tz), CachedTile::new(Box::new(tile)));
                map.store
                    .mips
                    .insert((tx, tz), CachedTile::new(Box::new(mip)));
            }
        }
        map
    }

    #[test]
    fn region_raster_matches_the_full_tile_raster_at_every_zoom() {
        // The blit path repaints sub-rects of a tile; any drift against the
        // whole-tile raster leaves visible seams.
        let mut map = synthetic_map();
        for zoom in ZOOM_MIN..=ZOOM_MAX {
            map.zoom = zoom;
            let tb = full_tile_blocks(zoom);
            let cb = cell_blocks(zoom) as usize;
            let cp = cell_px(zoom) as usize;
            let (full, _) = map.render_full_region((0, 0), [0, 0, tb, tb]);
            let raw_rects = [
                [0, 0, tb / 2, tb / 2],
                [tb / 2, 0, tb, tb / 2],
                [17, 23, 61, 59],
                [0, tb - 1, tb, tb],
            ];
            for raw in raw_rects {
                let rect = align_rect_to_cells(raw, zoom);
                let rect = [
                    rect[0].max(0),
                    rect[1].max(0),
                    rect[2].min(tb),
                    rect[3].min(tb),
                ];
                if rect[0] >= rect[2] || rect[1] >= rect[3] {
                    continue;
                }
                let (part, _) = map.render_full_region((0, 0), rect);
                let wc = (rect[2] - rect[0]) as usize / cb;
                let hc = (rect[3] - rect[1]) as usize / cb;
                for py in 0..hc * cp {
                    for px in 0..wc * cp {
                        let full_at = ((rect[1] as usize / cb * cp + py) * FULL_TILE_SIZE
                            + rect[0] as usize / cb * cp
                            + px)
                            * 4;
                        let part_at = (py * wc * cp + px) * 4;
                        assert_eq!(
                            &part[part_at..part_at + 4],
                            &full[full_at..full_at + 4],
                            "pixel ({px}, {py}) of region {rect:?} at zoom {zoom} \
                             must match the full raster"
                        );
                    }
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
        // A change at tile a's east edge; its southeast relief fringe crosses
        // into b (the fringe is added by mark_full_tiles_dirty itself).
        map.mark_full_tiles_dirty([79, 10, 80, 12]);
        assert_eq!(
            map.full_tile_slots[full_tile_slot(a)].dirty,
            Some([79, 10, 80, 13])
        );
        assert_eq!(
            map.full_tile_slots[full_tile_slot(b)].dirty,
            Some([0, 10, 1, 13])
        );
        // Rects union; a slot caching another coord stays untouched.
        map.mark_full_tiles_dirty([0, 0, 2, 2]);
        assert_eq!(
            map.full_tile_slots[full_tile_slot(a)].dirty,
            Some([0, 0, 80, 13])
        );
        let elsewhere = (2, 0);
        map.full_tile_slots[full_tile_slot(elsewhere)] = FullTileSlot {
            coord: Some((7, 7)),
            dirty: None,
        };
        map.mark_full_tiles_dirty([170, 0, 172, 2]);
        assert_eq!(map.full_tile_slots[full_tile_slot(elsewhere)].dirty, None);
    }

    #[test]
    fn zoom_keeps_the_point_under_the_cursor_fixed() {
        let half = FULL_SIZE as f32 * 0.5;
        for (from, to) in [(0i8, 1i8), (1, 2), (0, -1), (-1, -2), (-2, 2), (2, -2)] {
            let pan = [37.5f32, -1204.0];
            let (x, y) = (123.0f32, 456.0f32);
            let world = [
                pan[0] + (x - half) * blocks_per_pixel(from),
                pan[1] + (y - half) * blocks_per_pixel(from),
            ];
            let next = zoomed_pan(pan, from, to, x, y);
            let world_after = [
                next[0] + (x - half) * blocks_per_pixel(to),
                next[1] + (y - half) * blocks_per_pixel(to),
            ];
            let tolerance = blocks_per_pixel(to);
            assert!(
                (world[0] - world_after[0]).abs() <= tolerance
                    && (world[1] - world_after[1]).abs() <= tolerance,
                "{from}->{to}: {world:?} vs {world_after:?}"
            );
        }
    }

    #[test]
    fn tile_geometry_is_consistent_across_zooms() {
        for zoom in ZOOM_MIN..=ZOOM_MAX {
            let cb = cell_blocks(zoom);
            let cp = cell_px(zoom);
            let tb = full_tile_blocks(zoom);
            // A tile edge is a whole number of cells and always maps to the
            // fixed image size.
            assert_eq!(tb % cb, 0);
            assert_eq!((tb / cb * cp) as usize, FULL_TILE_SIZE);
            let aligned = align_rect_to_cells([1, 1, 3, 3], zoom);
            assert_eq!(aligned[0] % cb, 0);
            assert_eq!(aligned[2] % cb, 0);
            assert!(aligned[0] <= 1 && aligned[2] >= 3);
        }
    }
}
