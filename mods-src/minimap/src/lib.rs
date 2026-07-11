//! Client-side minimap
//!
//! The host supplies only final surface samples and generic image overlay,
//! canvas, document, key, and sandboxed-storage capabilities. This module owns
//! the map projection, exploration cache, shading, waypoints, and interaction.

use std::collections::{BTreeSet, HashMap};

use mod_sdk::*;

mod explore;
mod fullmap;
mod hud;
mod raster;
mod waypoints;

use explore::*;
use fullmap::*;
use raster::*;
use waypoints::*;

const HUD_IMAGE: &str = "minimap:hud_image";
const FULL_CANVAS: &str = "minimap:full_map";

const KEY_MAP: u32 = 1;
const KEY_WAYPOINT: u32 = 2;

#[derive(Default)]
struct Minimap {
    tiles: HashMap<(i32, i32), CachedTile>,
    waypoints: Vec<Waypoint>,
    player: [f32; 3],
    yaw: f32,
    open_canvas: Option<String>,
    last_sample: Option<(i32, i32)>,
    frame: u64,
    pan: [f32; 2],
    drag_start: Option<([f32; 2], [f32; 2])>,
    dragged: bool,
    editor: Editor,
    draft: String,
    full_tile_slots: [FullTileSlot; FULL_TILE_SLOTS],
    full_scene_stamp: Option<FullSceneStamp>,
    full_view_bits: Option<[u32; 2]>,
    waypoint_revision: u64,
    arrow_yaw_bits: Option<u32>,
}

impl Mod for Minimap {
    fn init(&mut self) {
        if runtime_side() != RuntimeSide::Client {
            return;
        }
        client_register_overlay(HUD_IMAGE, ClientOverlayAnchor::TopRight, [8, 8], [256, 256]);
        client_register_key("key_m", KEY_MAP);
        client_register_key("key_n", KEY_WAYPOINT);
        self.load_waypoints();
        log("client minimap initialized");
    }

    fn client_frame(&mut self, frame: &ClientFrameData) {
        self.frame = self.frame.wrapping_add(1);
        self.player = frame.player_pos;
        self.yaw = frame.yaw;
        self.open_canvas = frame.open_canvas.clone();
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
            self.publish_hud();
        }
        if frame.open_canvas.as_deref() == Some(FULL_CANVAS) {
            self.sync_full_canvas();
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
                    self.pan = [
                        snap_half_block(self.player[0]),
                        snap_half_block(self.player[2]),
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
}

register_mod!(Minimap);
