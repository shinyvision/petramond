//! The always-on circular minimap overlay: rotated terrain raster, border,
//! waypoint markers with initials, the fixed player pointer, and the
//! absolute-bearing cardinal labels. Re-rasterized only when its inputs
//! (yaw, position, explored cells, waypoints) actually changed.

use crate::*;

const HUD_SIZE: usize = 256;
const HUD_CENTER: f32 = 128.0;
const HUD_TERRAIN_RADIUS: f32 = 104.0;
const HUD_BORDER_RADIUS: f32 = 112.0;
const HUD_BLOCKS_PER_PIXEL: f32 = 0.5;
const HUD_PLAYER_ARROW_WIDTH: usize = 14;
const HUD_PLAYER_ARROW_HEIGHT: usize = 22;

/// Everything the HUD raster depends on; an unchanged stamp skips the whole
/// publish (no 256×256 re-raster, no texture upload) for a stationary player.
#[derive(Copy, Clone, PartialEq, Eq)]
pub(crate) struct HudStamp {
    yaw: u32,
    x: u32,
    z: u32,
    explored: u64,
    waypoints: u64,
}

impl Minimap {
    pub(crate) fn hud_stamp(&self) -> HudStamp {
        HudStamp {
            yaw: self.yaw.to_bits(),
            x: self.player[0].to_bits(),
            z: self.player[2].to_bits(),
            explored: self.explored_revision,
            waypoints: self.waypoint_revision,
        }
    }

    pub(crate) fn publish_hud(&mut self) {
        let mut rgba = vec![0u8; HUD_SIZE * HUD_SIZE * 4];
        let sin = self.yaw.sin();
        let cos = self.yaw.cos();
        let right = [-cos, sin];
        let forward = [sin, cos];
        {
            let mut reader = CellReader::new(&self.store.tiles);
            for py in 0..HUD_SIZE {
                for px in 0..HUD_SIZE {
                    let sx = px as f32 + 0.5 - HUD_CENTER;
                    let sy = py as f32 + 0.5 - HUD_CENTER;
                    let radius = (sx * sx + sy * sy).sqrt();
                    if radius <= HUD_TERRAIN_RADIUS {
                        let up = -sy * HUD_BLOCKS_PER_PIXEL;
                        let side = sx * HUD_BLOCKS_PER_PIXEL;
                        let wx =
                            (self.player[0] + side * right[0] + up * forward[0]).floor() as i32;
                        let wz =
                            (self.player[2] + side * right[1] + up * forward[1]).floor() as i32;
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
        }
        let mut text_runs = self.draw_hud_waypoints(&mut rgba, right, forward);
        draw_player_arrow(&mut rgba, HUD_SIZE, HUD_CENTER as i32, HUD_CENTER as i32);
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
                pos[0] as f32 + 0.5 - self.player[0],
                pos[2] as f32 + 0.5 - self.player[2],
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
                    text_runs.push(layout_waypoint_initial_below(rgba, HUD_SIZE, x, y, text, size));
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

fn draw_player_arrow(rgba: &mut [u8], width: usize, x: i32, y: i32) {
    let sprite = hud_player_arrow_rgba();
    composite_rgba_at(
        rgba,
        width,
        &sprite,
        HUD_PLAYER_ARROW_WIDTH,
        [x - 8, y - 14],
    );
}

fn cardinal_text_runs(right: [f32; 2], forward: [f32; 2], text_size: [u16; 2]) -> Vec<ClientTextRun> {
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
            center_x - text_width as i32 / 2,
            center_y - text_height as i32 / 2,
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
