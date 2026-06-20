//! App loop shared between native and web (after wgpu surface init).
//!
//! Owns World + Camera + Renderer, drives input -> movement -> world update
//! -> render. The platform shell handles window/event loop and surfaces.

use crate::block::Block;
use crate::camera::Camera;
use crate::mathh::{IVec3, Vec3};
use crate::player::{self, Input, Player, RaycastHit};
use crate::render::Renderer;
use crate::world::World;
use crate::worldgen::classic::world::CascadeWorld;

/// Deep, murky blue the world fades to (fog + clear colour) when the camera eye
/// is underwater.
const UNDERWATER_FOG_COLOR: [f32; 3] = [0.04, 0.16, 0.30];

pub struct App {
    pub cam: Camera,
    pub world: World,
    fallback_world: CascadeWorld,
    pub player: Player,
    /// Block currently under the crosshair (within reach), refreshed each tick.
    pub look: Option<RaycastHit>,
    pub last: f64,
    pub keys: KeyState,
    pub mouse: MouseState,
}

#[derive(Default, Copy, Clone)]
pub struct KeyState {
    pub w: bool,
    a: bool,
    s: bool,
    d: bool,
    pub space: bool,
    shift: bool,
    ctrl: bool,
    y: bool,
    mode_toggle_chord: bool,
}

#[derive(Default, Copy, Clone)]
pub struct MouseState {
    pub dx: f32,
    pub dy: f32,
    pub grabbing: bool,
    /// Edge-triggered click flags: set by the platform on button-press,
    /// consumed (and cleared) once per `tick` so one click = one action.
    pub left_click: bool,
    pub right_click: bool,
}

impl App {
    pub fn new(cam: Camera, seed: u32, render_dist: i32) -> Self {
        // Spawn the player so the camera (passed in as the eye position) lands at
        // the requested spot: feet sit `EYE` below the eye.
        let feet = Vec3::new(cam.pos.x, cam.pos.y - player::EYE, cam.pos.z);
        Self {
            cam,
            world: World::new(seed, render_dist),
            fallback_world: CascadeWorld::new(seed),
            player: Player::new(feet),
            look: None,
            last: now_seconds(),
            keys: KeyState::default(),
            mouse: MouseState::default(),
        }
    }

    /// Advance one frame. `dt_override` lets web supply a fixed step.
    pub fn tick(&mut self, renderer: &mut Renderer) {
        let now = now_seconds();
        let dt = (now - self.last) as f32;
        self.last = now;

        // Apply mouse look.
        if self.mouse.grabbing {
            const SENS: f32 = 0.0025;
            self.cam
                .rotate(-self.mouse.dx * SENS, -self.mouse.dy * SENS);
        }
        self.mouse.dx = 0.0;
        self.mouse.dy = 0.0;

        // Build movement intent from keys, relative to where we're looking.
        // Survival walking is horizontal; spectator movement uses the full
        // pitched camera forward plus explicit Space/Shift vertical flight.
        let f = self.cam.forward();
        let spectator = self.player.is_spectator();
        let fwd = if spectator {
            f
        } else {
            Vec3::new(f.x, 0.0, f.z).normalize_or_zero()
        };
        let right = self.cam.right(); // already horizontal
        let mut wishdir = Vec3::ZERO;
        if self.keys.w {
            wishdir += fwd;
        }
        if self.keys.s {
            wishdir -= fwd;
        }
        if self.keys.d {
            wishdir += right;
        }
        if self.keys.a {
            wishdir -= right;
        }
        if spectator {
            if self.keys.space {
                wishdir += Vec3::Y;
            }
            if self.keys.shift {
                wishdir -= Vec3::Y;
            }
        }
        let input = Input {
            wishdir: wishdir.normalize_or_zero(),
            jump: self.keys.space,
            sprint: self.keys.ctrl,
        };

        // Advance physics in fixed sub-steps so a long frame (or a backgrounded
        // tab on web) can't move the player far enough to tunnel. `dt` is also
        // capped so we never spin through a huge backlog after a stall. The
        // load-gate is checked once per frame here (column membership can't
        // change mid-frame) rather than inside every sub-step.
        if spectator || self.player.columns_loaded(&self.world) {
            let mut remaining = dt.min(0.25);
            while remaining > 0.0 {
                let step = remaining.min(player::DT_MAX);
                self.player.update(step, &self.world, input);
                remaining -= step;
            }
        }
        // Camera eye follows the player.
        self.cam.pos = self.player.eye();

        // World update around the player's chunk column.
        let cam_cx = (self.cam.pos.x as i32) >> 4;
        let cam_cz = (self.cam.pos.z as i32) >> 4;
        self.world.update_load(cam_cx, cam_cz);
        let _ = self.world.poll();

        // Crosshair raycast, then break/place against the hit (consume clicks).
        self.look = Player::raycast(self.cam.pos, self.cam.forward(), &self.world);
        self.handle_block_actions();

        // Native meshes a big burst per frame in parallel (rayon); wasm stays
        // conservative on its single thread. Done AFTER edits so a break/place
        // is remeshed and visible this same frame.
        #[cfg(not(target_arch = "wasm32"))]
        const MESH_BUDGET: usize = 32;
        #[cfg(target_arch = "wasm32")]
        const MESH_BUDGET: usize = 6;
        self.world.tick_mesh_budget(MESH_BUDGET);

        // Is the camera eye inside a water block? Drives the underwater shader
        // (blue darkening + dense fog + caustics) and the matching clear colour.
        let eye = self.cam.pos;
        let underwater = Block::from_id(self.world.chunk_block(
            eye.x.floor() as i32,
            eye.y.floor() as i32,
            eye.z.floor() as i32,
        )) == Block::Water;

        // Fog colour: blended nearby biome fog above water, or a deep murky blue
        // when submerged so distant terrain dissolves into the water.
        let fog = if underwater {
            UNDERWATER_FOG_COLOR
        } else {
            self.blended_sky_fog_color(eye.x, eye.z)
        };

        // Seconds since start (wrapped to keep the value small) drive the animated
        // underwater caustics in the shader.
        let time = (now % 3600.0) as f32;

        renderer.update_uniforms(&self.cam, fog, time, underwater);
        renderer.set_selection(self.look.map(|h| h.outline));
        renderer.sync_meshes(&mut self.world);
        renderer.update_section_visibility(&mut self.world);
        renderer.render();
    }

    /// Apply any pending left/right clicks to the targeted block. Left = break
    /// (instant), right = place Stone in the empty cell against the hit face.
    fn handle_block_actions(&mut self) {
        if self.mouse.left_click {
            if let Some(h) = self.look {
                self.world
                    .set_block_world(h.block.x, h.block.y, h.block.z, Block::Air);
            }
        }
        if self.mouse.right_click {
            if let Some(h) = self.look {
                // normal == 0 means the eye was inside a block: nowhere to place.
                if h.normal != IVec3::ZERO {
                    let p = h.block + h.normal;
                    // Place into any replaceable cell (air or water -- building
                    // into water displaces it), if the player isn't standing there.
                    let target = Block::from_id(self.world.chunk_block(p.x, p.y, p.z));
                    if target.is_replaceable() && !self.player.intersects_block(p) {
                        self.world.set_block_world(p.x, p.y, p.z, Block::Stone);
                    }
                }
            }
        }
        self.mouse.left_click = false;
        self.mouse.right_click = false;
    }

    fn blended_sky_fog_color(&self, x: f32, z: f32) -> [f32; 3] {
        use crate::biome::{blended_fog_color, Biome};

        blended_fog_color(x, z, |wx, wz| {
            if let Some(id) = self.world.column_biome(wx, wz) {
                return Biome::from_id(id);
            }

            self.fallback_world.biome_at(wx, wz)
        })
    }

    pub fn set_key(&mut self, code: &str, down: bool) {
        match code {
            "KeyW" => self.keys.w = down,
            "KeyA" => self.keys.a = down,
            "KeyS" => self.keys.s = down,
            "KeyD" => self.keys.d = down,
            "Space" => self.keys.space = down,
            "ShiftLeft" | "ShiftRight" => self.keys.shift = down,
            "ControlLeft" | "ControlRight" => self.keys.ctrl = down,
            "KeyY" => self.keys.y = down,
            _ => {}
        }
        let chord = self.keys.ctrl && self.keys.y;
        if chord && !self.keys.mode_toggle_chord {
            self.player.toggle_mode();
        }
        self.keys.mode_toggle_chord = chord;
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn now_seconds() -> f64 {
    use std::sync::OnceLock;
    use std::time::Instant;

    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

#[cfg(target_arch = "wasm32")]
fn now_seconds() -> f64 {
    web_sys::window()
        .and_then(|window| window.performance())
        .map(|performance| performance.now() / 1000.0)
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::PlayerMode;

    fn app() -> App {
        App::new(Camera::new(Vec3::new(0.0, 80.0, 0.0), 16.0 / 9.0), 1, 1)
    }

    #[test]
    fn ctrl_y_toggles_player_mode_once_per_chord() {
        let mut app = app();
        assert_eq!(app.player.mode(), PlayerMode::Survival);

        app.set_key("ControlLeft", true);
        app.set_key("KeyY", true);
        assert_eq!(app.player.mode(), PlayerMode::Spectator);

        // Repeated keydown events while the chord remains held must not bounce
        // rapidly between modes.
        app.set_key("KeyY", true);
        app.set_key("ControlLeft", true);
        assert_eq!(app.player.mode(), PlayerMode::Spectator);

        app.set_key("KeyY", false);
        app.set_key("KeyY", true);
        assert_eq!(app.player.mode(), PlayerMode::Survival);

        app.set_key("ControlLeft", false);
        app.set_key("KeyY", false);
        app.set_key("KeyY", true);
        assert_eq!(app.player.mode(), PlayerMode::Survival);
    }
}
