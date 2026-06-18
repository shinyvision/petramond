//! App loop shared between native and web (after wgpu surface init).
//!
//! Owns World + Camera + Renderer, drives input -> movement -> world update
//! -> render. The platform shell handles window/event loop and surfaces.

use std::time::Instant;

use crate::camera::Camera;
use crate::chunk::CHUNK_SX;
use crate::mathh::Vec3;
use crate::render::Renderer;
use crate::world::World;

pub struct App {
    pub cam: Camera,
    pub world: World,
    pub last: Instant,
    pub keys: KeyState,
    pub mouse: MouseState,
}

#[derive(Default, Copy, Clone)]
pub struct KeyState {
    pub w: bool, a: bool, s: bool, d: bool,
    pub space: bool, shift: bool, ctrl: bool,
}

#[derive(Default, Copy, Clone)]
pub struct MouseState {
    pub dx: f32, pub dy: f32, pub grabbing: bool,
}

impl App {
    pub fn new(cam: Camera, seed: u32, render_dist: i32) -> Self {
        Self {
            cam, world: World::new(seed, render_dist),
            last: Instant::now(), keys: KeyState::default(),
            mouse: MouseState::default(),
        }
    }

    /// Advance one frame. `dt_override` lets web supply a fixed step.
    pub fn tick(&mut self, renderer: &mut Renderer) {
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f32();
        self.last = now;

        // Apply mouse look.
        if self.mouse.grabbing {
            const SENS: f32 = 0.0025;
            self.cam.rotate(-self.mouse.dx * SENS, -self.mouse.dy * SENS);
        }
        self.mouse.dx = 0.0; self.mouse.dy = 0.0;

        // Movement.
        let speed = if self.keys.ctrl { 180.0 } else { 22.0 } * dt.max(0.001);
        let mut delta = Vec3::ZERO;
        let fwd = self.cam.forward();
        let right = self.cam.right();
        if self.keys.w { delta += fwd * speed; }
        if self.keys.s { delta -= fwd * speed; }
        if self.keys.d { delta += right * speed; }
        if self.keys.a { delta -= right * speed; }
        if self.keys.space  { delta += Vec3::Y * speed; }
        if self.keys.shift  { delta -= Vec3::Y * speed; }
        if delta != Vec3::ZERO { self.cam.move_by(delta); }

        // World update around camera chunk coords.
        let cam_cx = (self.cam.pos.x as i32) >> 4;
        let cam_cz = (self.cam.pos.z as i32) >> 4;
        self.world.update_load(cam_cx, cam_cz);
        let _ = self.world.poll();
        self.world.tick_mesh_budget(6); // 6 chunks/frame

        // Fog colour sampled from biome at camera column.
        let climate = self.world_seed_climate(cam_cx, cam_cz);
        let biome = crate::biome::biome_at(climate, self.cam.pos.y as i32);
        let fog = biome.fog_color();

        renderer.update_uniforms(&self.cam, fog);
        renderer.sync_meshes(&mut self.world);
        renderer.render();
    }

    fn world_seed_climate(&self, cx: i32, cz: i32) -> crate::biome::Climate {
        use crate::worldgen::WorldNoise;
        let n = WorldNoise::new(self.world.seed);
        n.climate(cx * CHUNK_SX as i32, cz * CHUNK_SX as i32)
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
            _ => {}
        }
    }
}