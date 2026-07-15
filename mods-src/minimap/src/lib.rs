//! Client-side minimap
//!
//! The host supplies only final surface samples and generic image overlay,
//! canvas, document, key, and sandboxed-storage capabilities. This module owns
//! the map projection, exploration cache, shading, waypoints, and interaction.

use std::collections::{BTreeSet, HashMap, HashSet};

use mod_sdk::*;

mod codec;
mod explore;
mod fullmap;
mod hud;
mod raster;
mod waypoints;

use explore::*;
use fullmap::*;
use hud::*;
use raster::*;
use waypoints::*;

const HUD_IMAGE: &str = "minimap:hud_image";
const FULL_CANVAS: &str = "minimap:full_map";

const KEY_MAP: u32 = 1;
const KEY_WAYPOINT: u32 = 2;

#[derive(Default)]
struct Minimap {
    /// Explored base/mip tile caches plus the async region loader.
    store: TileStore,
    waypoints: Vec<Waypoint>,
    player: [f32; 3],
    yaw: f32,
    open_canvas: Option<String>,
    last_sample: Option<(i32, i32)>,
    frame: u64,
    pan: [f32; 2],
    /// Full-map zoom level, [`ZOOM_MIN`]..=[`ZOOM_MAX`] canvas-pixel-per-block
    /// steps around the 2 px/block default; kept across open/close.
    zoom: i8,
    /// Sub-notch wheel remainder (hi-res wheels emit fractional notches).
    scroll_accum: f32,
    drag_start: Option<([f32; 2], [f32; 2])>,
    dragged: bool,
    editor: Editor,
    draft: String,
    full_tile_slots: FullTileSlots,
    full_scene_stamp: Option<FullSceneStamp>,
    full_view_bits: Option<[u32; 2]>,
    /// The (bounds, zoom) the visible region requests were queued for.
    full_needed_stamp: Option<([i32; 4], i8)>,
    /// Pan at the last visible-request recompute — its delta picks the
    /// velocity-prefetch direction.
    last_synced_pan: Option<[f32; 2]>,
    waypoint_revision: u64,
    arrow_yaw_bits: Option<u32>,
    /// Bumps whenever any explored cell changes; part of the HUD stamp.
    explored_revision: u64,
    /// The inputs the current HUD raster was published from.
    hud_stamp: Option<HudStamp>,
    /// Waypoint layouts cached per (`waypoint_revision`, zoom) — text
    /// measurement is a host call, never re-measured per publish, and layout
    /// pixel positions scale with the zoom level.
    full_layouts: Option<(u64, i8, Vec<FullWaypointLayout>)>,
    /// Scale-2 text measurement cache (waypoint names, initials, cardinals).
    text_sizes: HashMap<String, [u16; 2]>,
}

impl Mod for Minimap {
    fn init(&mut self) {
        if runtime_side() != RuntimeSide::Client {
            return;
        }
        client_register_overlay(HUD_IMAGE, ClientOverlayAnchor::TopRight, [8, 8], [256, 256]);
        client_register_key("open_map", "Open World Map", "key_m", KEY_MAP);
        client_register_key("add_waypoint", "Add Waypoint", "key_n", KEY_WAYPOINT);
        self.load_waypoints();
        log("client minimap initialized");
    }

    fn client_frame(&mut self, frame: &ClientFrameData) {
        self.frame = self.frame.wrapping_add(1);
        self.player = frame.player_pos;
        self.yaw = frame.yaw;
        self.open_canvas = frame.open_canvas.clone();
        self.store.begin_frame(self.frame);
        // Loader heartbeat: poll async region reads, repaint arrivals, trim
        // the caches when the map is closed.
        self.pump_store();
        let center = (
            frame.player_pos[0].floor() as i32,
            frame.player_pos[2].floor() as i32,
        );
        let moved = self.last_sample.is_none_or(|old| {
            (center.0 - old.0).abs() >= SAMPLE_STEP || (center.1 - old.1).abs() >= SAMPLE_STEP
        });
        if moved || self.frame % 30 == 1 {
            self.refresh_surface(center);
            self.last_sample = Some(center);
        }
        if frame.open_gui.is_none() && frame.open_canvas.is_none() {
            let stamp = self.hud_stamp();
            if self.hud_stamp != Some(stamp) {
                self.publish_hud();
                self.hud_stamp = Some(stamp);
            }
        }
        if frame.open_canvas.as_deref() == Some(FULL_CANVAS) {
            self.sync_full_canvas();
        } else if self.full_needed_stamp.is_some() {
            // Map closed: a reopen recomputes its visible requests.
            self.full_needed_stamp = None;
        }
        if self.frame % FLUSH_INTERVAL == 0 {
            self.store.flush_dirty();
        }
    }

    fn client_key(&mut self, action_id: u32, pressed: bool) {
        if !pressed {
            return;
        }
        match action_id {
            KEY_MAP => {
                if self.open_canvas.as_deref() == Some(FULL_CANVAS) {
                    client_canvas_close();
                } else {
                    self.editor = Editor::None;
                    self.scroll_accum = 0.0;
                    self.pan = [
                        snap_to_source_pixel(self.player[0], self.zoom),
                        snap_to_source_pixel(self.player[2], self.zoom),
                    ];
                    self.sync_full_canvas();
                    client_canvas_open(FULL_CANVAS, [FULL_SIZE as u16, FULL_SIZE as u16]);
                }
            }
            KEY_WAYPOINT => self.open_create(),
            _ => {}
        }
    }

    fn client_ui(&mut self, _kind_key: &str, event: &ClientUiEvent) {
        match event {
            ClientUiEvent::TextChanged { id, text } | ClientUiEvent::Submit { id, text }
                if id == "name" =>
            {
                self.draft = text.clone();
                if matches!(event, ClientUiEvent::Submit { .. }) {
                    self.save_editor();
                }
            }
            ClientUiEvent::Click { id } if id == "save" => self.save_editor(),
            ClientUiEvent::Click { id } if id == "cancel" => self.cancel_editor(),
            ClientUiEvent::Click { id } if id == "delete" => self.delete_editor(),
            _ => {}
        }
    }

    fn client_canvas(&mut self, canvas_key: &str, event: &ClientCanvasEvent) {
        if canvas_key == FULL_CANVAS && event.button == ClientPointerButton::Primary {
            self.map_pointer(event.phase, event.x, event.y);
        }
    }

    fn client_canvas_scroll(&mut self, canvas_key: &str, x: f32, y: f32, delta: f32) {
        if canvas_key != FULL_CANVAS {
            return;
        }
        // Whole notches step the zoom (wheel up = in), anchored at the
        // cursor; the sub-notch remainder carries to the next event.
        self.scroll_accum += delta;
        let steps = self.scroll_accum.trunc();
        self.scroll_accum -= steps;
        if steps != 0.0 {
            self.zoom_step(steps as i32, x, y);
        }
    }
}

impl Minimap {
    /// Scale-2 single-line measurement through a cache: `client_text_measure`
    /// crosses the ABI, so each distinct string measures once.
    fn measure_cached(&mut self, text: &str) -> [u16; 2] {
        if let Some(&size) = self.text_sizes.get(text) {
            return size;
        }
        let size = client_text_measure(text, 2);
        if self.text_sizes.len() >= 512 {
            self.text_sizes.clear();
        }
        self.text_sizes.insert(text.to_owned(), size);
        size
    }
}

register_mod!(Minimap);
