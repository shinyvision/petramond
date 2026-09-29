use crate::*;

const HUD_SIZE: usize = 256;
const HUD_CENTER: f32 = 128.0;
const HUD_TERRAIN_RADIUS: f32 = 104.0;
const HUD_BORDER_RADIUS: f32 = 112.0;
const HUD_BLOCKS_PER_PIXEL: f32 = 0.5;
const HUD_PLAYER_ARROW_WIDTH: usize = 14;
const HUD_PLAYER_ARROW_HEIGHT: usize = 22;

/// Blocks per side of the pre-shaded terrain window the HUD samples. The HUD reaches
/// `HUD_TERRAIN_RADIUS * HUD_BLOCKS_PER_PIXEL` (52) blocks from the player and the window is
/// re-centred once the player strays `WINDOW_RECENTER` blocks, so every sample lands inside the
/// window (64 - 9 > 52 + 1) and the raster indexes it without a bounds branch.
const WINDOW_SHIFT: u32 = 7;
const WINDOW: i32 = 1 << WINDOW_SHIFT;
const WINDOW_CELLS: usize = (WINDOW * WINDOW) as usize;
const WINDOW_HALF: i32 = WINDOW / 2;
const WINDOW_RECENTER: i32 = 8;

#[derive(Copy, Clone, PartialEq, Eq)]
pub(crate) struct HudStamp {
    yaw: u32,
    x: u64,
    z: u64,
    explored: u64,
    tiles: u64,
    waypoints: u64,
}

/// The HUD raster's reusable parts. Everything that does not move with the player (the border
/// ring, the terrain disc's row spans, the arrow sprite) is built once; the terrain is shaded once
/// per block into a north-up window and only re-sampled, rotated, per frame.
#[derive(Default)]
pub(crate) struct HudRaster {
    template: Vec<u8>,
    spans: Vec<(u16, u16)>,
    arrow: Vec<u8>,
    window: Vec<u32>,
    window_origin: (i32, i32),
    window_revision: Option<u64>,
    gather: Vec<Cell>,
}

impl HudRaster {
    fn ensure_static(&mut self) {
        if !self.template.is_empty() {
            return;
        }
        let mut template = vec![0u8; HUD_SIZE * HUD_SIZE * 4];
        let mut spans = vec![(0u16, 0u16); HUD_SIZE];
        for (py, span) in spans.iter_mut().enumerate() {
            let mut first = None;
            let mut end = 0;
            for px in 0..HUD_SIZE {
                let sx = px as f32 + 0.5 - HUD_CENTER;
                let sy = py as f32 + 0.5 - HUD_CENTER;
                let radius = (sx * sx + sy * sy).sqrt();
                if radius <= HUD_TERRAIN_RADIUS {
                    first.get_or_insert(px);
                    end = px + 1;
                } else if radius <= HUD_BORDER_RADIUS + 0.5 {
                    let c = if radius < HUD_TERRAIN_RADIUS + 1.2 {
                        [132, 144, 154]
                    } else {
                        [29, 34, 39]
                    };
                    let coverage = (HUD_BORDER_RADIUS + 0.5 - radius).clamp(0.0, 1.0);
                    set_pixel_alpha(
                        &mut template,
                        HUD_SIZE,
                        px as i32,
                        py as i32,
                        c,
                        (coverage * 255.0).round() as u8,
                    );
                }
            }
            if let Some(first) = first {
                *span = (first as u16, end as u16);
            }
        }
        self.template = template;
        self.spans = spans;
        self.arrow = hud_player_arrow_rgba();
    }

    fn arrow(&self) -> &[u8] {
        &self.arrow
    }

    /// Shade every block of the window around `center` once: a cell's colour with its relief
    /// against the northwest neighbour, black where nothing is explored.
    fn rebuild_window(&mut self, tiles: &HashMap<(i32, i32), CachedTile>, center: (i32, i32)) {
        let origin = (center.0 - WINDOW_HALF, center.1 - WINDOW_HALF);
        // One extra row and column on the northwest side for the relief neighbour.
        let side = (WINDOW + 1) as usize;
        let (gx, gz) = (origin.0 - 1, origin.1 - 1);
        self.gather.clear();
        self.gather.resize(side * side, Cell::default());
        for tz in gz.div_euclid(16)..=(gz + side as i32 - 1).div_euclid(16) {
            for tx in gx.div_euclid(16)..=(gx + side as i32 - 1).div_euclid(16) {
                let Some(cached) = tiles.get(&(tx, tz)) else {
                    continue;
                };
                let x0 = (tx * 16).max(gx);
                let x1 = (tx * 16 + 16).min(gx + side as i32);
                let z0 = (tz * 16).max(gz);
                let z1 = (tz * 16 + 16).min(gz + side as i32);
                let width = (x1 - x0) as usize;
                for wz in z0..z1 {
                    let src = ((wz - tz * 16) * 16 + (x0 - tx * 16)) as usize;
                    let dst = (wz - gz) as usize * side + (x0 - gx) as usize;
                    self.gather[dst..dst + width]
                        .copy_from_slice(&cached.tile.cells[src..src + width]);
                }
            }
        }
        self.window.clear();
        self.window.reserve(WINDOW as usize * WINDOW as usize);
        for lz in 0..WINDOW as usize {
            let row = (lz + 1) * side + 1;
            let above = lz * side;
            for lx in 0..WINDOW as usize {
                let cell = self.gather[row + lx];
                let rgb = if cell.height == UNKNOWN_HEIGHT {
                    [0, 0, 0]
                } else {
                    let northwest = self.gather[above + lx].height;
                    let northwest = if northwest == UNKNOWN_HEIGHT {
                        cell.height
                    } else {
                        northwest
                    };
                    shade_rgb(cell.rgb, cell.height, northwest)
                };
                self.window
                    .push(u32::from_le_bytes([rgb[0], rgb[1], rgb[2], 255]));
            }
        }
        self.window_origin = origin;
    }

    /// The terrain disc and border ring, rotated so `forward` points up.
    pub(crate) fn raster_terrain(
        &mut self,
        store: &TileStore,
        player: [f64; 3],
        right: [f32; 2],
        forward: [f32; 2],
    ) -> Vec<u8> {
        self.ensure_static();
        let center = (player[0].floor() as i32, player[2].floor() as i32);
        let window_center = (
            self.window_origin.0 + WINDOW_HALF,
            self.window_origin.1 + WINDOW_HALF,
        );
        if self.window_revision != Some(store.base_revision)
            || (center.0 - window_center.0).abs() > WINDOW_RECENTER
            || (center.1 - window_center.1).abs() > WINDOW_RECENTER
        {
            self.rebuild_window(&store.tiles, center);
            self.window_revision = Some(store.base_revision);
        }
        let mut rgba = self.template.clone();
        let (ox, oz) = self.window_origin;
        let window: &[u32; WINDOW_CELLS] = self.window.as_slice().try_into().expect("window size");
        // The same f32 products the per-pixel formula forms, hoisted out of the row loop.
        let mut across_x = [0f32; HUD_SIZE];
        let mut across_z = [0f32; HUD_SIZE];
        for px in 0..HUD_SIZE {
            let side = (px as f32 + 0.5 - HUD_CENTER) * HUD_BLOCKS_PER_PIXEL;
            across_x[px] = side * right[0];
            across_z[px] = side * right[1];
        }
        let sampler = RowSampler {
            player: [player[0], player[2]],
            origin: [ox, oz],
            window,
        };
        for (py, &(x0, x1)) in self.spans.iter().enumerate() {
            let sy = py as f32 + 0.5 - HUD_CENTER;
            let up = -sy * HUD_BLOCKS_PER_PIXEL;
            let (x0, x1) = (x0 as usize, x1 as usize);
            sampler.row(
                &mut rgba[(py * HUD_SIZE + x0) * 4..(py * HUD_SIZE + x1) * 4],
                &across_x[x0..x1],
                &across_z[x0..x1],
                [up * forward[0], up * forward[1]],
            );
        }
        rgba
    }
}

/// Samples one row of the rotated disc: pixel `i` shows the window cell under
/// `floor(player + f64(across[i] + up))` on each axis. Every sample lies inside the window (see
/// `WINDOW_RECENTER`), so the index is masked rather than bounds-branched.
struct RowSampler<'a> {
    player: [f64; 2],
    origin: [i32; 2],
    window: &'a [u32; WINDOW_CELLS],
}

impl RowSampler<'_> {
    #[inline]
    fn cell(&self, wx: i32, wz: i32) -> u32 {
        let (lx, lz) = (
            wx.wrapping_sub(self.origin[0]) as u32,
            wz.wrapping_sub(self.origin[1]) as u32,
        );
        debug_assert!(lx < WINDOW as u32 && lz < WINDOW as u32);
        let mask = WINDOW as u32 - 1;
        self.window[((lz & mask) << WINDOW_SHIFT | (lx & mask)) as usize]
    }

    #[inline]
    fn row(&self, out: &mut [u8], across_x: &[f32], across_z: &[f32], up: [f32; 2]) {
        #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
        let done = self.row_simd(out, across_x, across_z, up);
        #[cfg(not(all(target_arch = "wasm32", target_feature = "simd128")))]
        let done = 0;
        self.row_scalar(
            &mut out[done * 4..],
            &across_x[done..],
            &across_z[done..],
            up,
        );
    }

    fn row_scalar(&self, out: &mut [u8], across_x: &[f32], across_z: &[f32], up: [f32; 2]) {
        for (pixel, (&side_x, &side_z)) in
            out.chunks_exact_mut(4).zip(across_x.iter().zip(across_z))
        {
            let wx = (self.player[0] + f64::from(side_x + up[0])).floor() as i32;
            let wz = (self.player[1] + f64::from(side_z + up[1])).floor() as i32;
            pixel.copy_from_slice(&self.cell(wx, wz).to_le_bytes());
        }
    }

    /// Four pixels per step with the same IEEE operations lane-wise (f32 add, f64 promote/add/floor,
    /// saturating truncation), so it matches `row_scalar` bit for bit. Returns the pixels done.
    #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
    fn row_simd(&self, out: &mut [u8], across_x: &[f32], across_z: &[f32], up: [f32; 2]) -> usize {
        use core::arch::wasm32::*;
        let quads = across_x.len() / 4;
        let (up_x, up_z) = (f32x4_splat(up[0]), f32x4_splat(up[1]));
        let (px, pz) = (f64x2_splat(self.player[0]), f64x2_splat(self.player[1]));
        let floor_i32 = |v: v128, p: v128| -> v128 {
            let lo = f64x2_floor(f64x2_add(f64x2_promote_low_f32x4(v), p));
            let hi = f64x2_floor(f64x2_add(
                f64x2_promote_low_f32x4(i32x4_shuffle::<2, 3, 0, 1>(v, v)),
                p,
            ));
            i32x4_shuffle::<0, 1, 4, 5>(
                i32x4_trunc_sat_f64x2_zero(lo),
                i32x4_trunc_sat_f64x2_zero(hi),
            )
        };
        let (ox, oz) = (i32x4_splat(self.origin[0]), i32x4_splat(self.origin[1]));
        let mask = i32x4_splat(WINDOW - 1);
        for q in 0..quads {
            // SAFETY: `q * 4 + 3 < across.len()`, and `out` holds 4 bytes per `across` entry.
            unsafe {
                let ax = v128_load(across_x.as_ptr().add(q * 4) as *const v128);
                let az = v128_load(across_z.as_ptr().add(q * 4) as *const v128);
                let wx = floor_i32(f32x4_add(ax, up_x), px);
                let wz = floor_i32(f32x4_add(az, up_z), pz);
                let lx = v128_and(i32x4_sub(wx, ox), mask);
                let lz = v128_and(i32x4_sub(wz, oz), mask);
                let at = v128_or(i32x4_shl(lz, WINDOW_SHIFT), lx);
                let window = self.window.as_ptr();
                let gathered = u32x4(
                    *window.add(u32x4_extract_lane::<0>(at) as usize),
                    *window.add(u32x4_extract_lane::<1>(at) as usize),
                    *window.add(u32x4_extract_lane::<2>(at) as usize),
                    *window.add(u32x4_extract_lane::<3>(at) as usize),
                );
                v128_store(out.as_mut_ptr().add(q * 16) as *mut v128, gathered);
            }
        }
        quads * 4
    }
}

impl Minimap {
    pub(crate) fn hud_stamp(&self) -> HudStamp {
        HudStamp {
            yaw: self.yaw.to_bits(),
            x: self.player[0].to_bits(),
            z: self.player[2].to_bits(),
            explored: self.explored_revision,
            tiles: self.store.base_revision,
            waypoints: self.waypoint_revision,
        }
    }

    pub(crate) fn publish_hud(&mut self) {
        let sin = self.yaw.sin();
        let cos = self.yaw.cos();
        let right = [-cos, sin];
        let forward = [sin, cos];
        let mut rgba = self
            .hud
            .raster_terrain(&self.store, self.player, right, forward);
        let mut text_runs = self.draw_hud_waypoints(&mut rgba, right, forward);
        composite_rgba_at(
            &mut rgba,
            HUD_SIZE,
            self.hud.arrow(),
            HUD_PLAYER_ARROW_WIDTH,
            [HUD_CENTER as i32 - 8, HUD_CENTER as i32 - 14],
        );
        let cardinal_size = self.measure_cached("N");
        text_runs.extend(cardinal_text_runs(right, forward, cardinal_size));
        client_image_set(HUD_IMAGE, HUD_SIZE as u16, HUD_SIZE as u16, rgba);
        client_image_draw_texts(HUD_IMAGE, text_runs);
    }

    fn draw_hud_waypoints(
        &mut self,
        rgba: &mut [u8],
        right: [f32; 2],
        forward: [f32; 2],
    ) -> Vec<ClientTextRun> {
        let mut text_runs = Vec::new();
        for index in 0..self.waypoints.len() {
            let (pos, color, initial) = {
                let waypoint = &self.waypoints[index];
                (waypoint.pos, waypoint.color, waypoint.name.chars().next())
            };
            let delta = [
                (f64::from(pos[0]) + 0.5 - self.player[0]) as f32,
                (f64::from(pos[2]) + 0.5 - self.player[2]) as f32,
            ];
            let sx = (delta[0] * right[0] + delta[1] * right[1]) / HUD_BLOCKS_PER_PIXEL;
            let sy = -(delta[0] * forward[0] + delta[1] * forward[1]) / HUD_BLOCKS_PER_PIXEL;
            if sx * sx + sy * sy <= (HUD_TERRAIN_RADIUS - 8.0).powi(2) {
                let x = (HUD_CENTER + sx).round() as i32;
                let y = (HUD_CENTER + sy).round() as i32;
                draw_hud_waypoint_diamond(rgba, HUD_SIZE, x, y, color);
                if let Some(initial) = initial {
                    let text = initial.to_string();
                    let size = self.measure_cached(&text);
                    text_runs.push(layout_waypoint_initial_below(
                        rgba, HUD_SIZE, x, y, text, size,
                    ));
                }
            }
        }
        text_runs
    }
}

fn hud_player_arrow_rgba() -> Vec<u8> {
    let mut rgba = vec![0; HUD_PLAYER_ARROW_WIDTH * HUD_PLAYER_ARROW_HEIGHT * 4];
    let tip = [8.0, -0.5];
    let left = [2.0, 18.5];
    let seam = [8.0, 14.5];
    let right = [14.0, 18.5];

    stamp_arrow_with_shadow(
        &mut rgba,
        HUD_PLAYER_ARROW_WIDTH,
        [tip, left, seam],
        [tip, seam, right],
    );
    rgba
}

fn layout_waypoint_initial_below(
    rgba: &mut [u8],
    width: usize,
    x: i32,
    y: i32,
    text: String,
    text_size: [u16; 2],
) -> ClientTextRun {
    let label_width = text_size[0] as i32 + 2;
    let label_height = text_size[1] as i32 + 2;
    let height = rgba.len() as i32 / 4 / width as i32;
    let left = (x - label_width / 2).clamp(0, width as i32 - label_width);
    let top = (y + 9).clamp(0, height - label_height);
    fill_rect(
        rgba,
        width,
        left,
        top,
        label_width,
        label_height,
        [18, 22, 26],
    );
    ClientTextRun {
        text,
        position: [left + 1, top + 1],
        scale: 2,
        color: [250, 250, 250, 255],
    }
}

fn cardinal_text_runs(
    right: [f32; 2],
    forward: [f32; 2],
    text_size: [u16; 2],
) -> Vec<ClientTextRun> {
    let [text_width, text_height] = text_size;
    let mut runs = Vec::with_capacity(36);
    for (letter, direction) in [
        ('N', [0.0, -1.0]),
        ('E', [1.0, 0.0]),
        ('S', [0.0, 1.0]),
        ('W', [-1.0, 0.0]),
    ] {
        let sx = direction[0] * right[0] + direction[1] * right[1];
        let up = direction[0] * forward[0] + direction[1] * forward[1];
        let center_x = (HUD_CENTER + sx * 120.0).round() as i32;
        let center_y = (HUD_CENTER - up * 120.0).round() as i32;
        let position = [
            inside_hud(center_x - text_width as i32 / 2, text_width),
            inside_hud(center_y - text_height as i32 / 2, text_height),
        ];
        let text = letter.to_string();
        for offset in [
            [-1, -1],
            [0, -1],
            [1, -1],
            [-1, 0],
            [1, 0],
            [-1, 1],
            [0, 1],
            [1, 1],
        ] {
            runs.push(ClientTextRun {
                text: text.clone(),
                position: [position[0] + offset[0], position[1] + offset[1]],
                scale: 2,
                color: [20, 20, 20, 255],
            });
        }
        runs.push(ClientTextRun {
            text,
            position,
            scale: 2,
            color: [250, 250, 250, 255],
        });
    }
    runs
}

fn inside_hud(top_left: i32, extent: u16) -> i32 {
    let max = (HUD_SIZE as i32 - i32::from(extent) - 1).max(1);
    top_left.clamp(1, max)
}

#[cfg(test)]
mod tests;
