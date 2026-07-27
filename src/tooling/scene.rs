//! Windowless scene capture: the real streaming world plus the real renderer,
//! aimed at an image instead of a window.
//!
//! Enough surface to build a world at a seed, stream terrain in around a point,
//! aim a camera, and get pixels back — so terrain and lighting work can be
//! LOOKED at without launching the game. This is not a second render path:
//! every capture goes through the same frame graph the window draws, and the
//! camera environment (fog, underwater) comes from the same derivation the game
//! feeds its own uniforms.

use std::time::{Duration, Instant};

use crate::biome::Biome;
use crate::camera::Camera;
use crate::mathh::{voxel_at, Vec3};
use crate::render::Renderer;
use crate::world::environment::ShaderParamMap;
use crate::world::World;

pub use crate::render::RenderedFrame;

/// Colour format captures render in. sRGB like the game's swapchain (so the
/// pipelines and the pre-baked icon atlas behave identically) and RGBA-ordered,
/// so readback needs no channel swizzle.
const CAPTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// Consecutive quiet pumps before [`SceneCapture::settle`] calls the terrain
/// settled. The streaming pools go briefly idle mid-flight, so one quiet pump
/// is not enough.
const SETTLE_PUMPS: u32 = 50;

/// Pause between settle pumps. `poll` is a non-blocking drain, so without this
/// the settle loop would burn a core that the generation/light/mesh workers
/// want.
const SETTLE_PUMP_PAUSE: Duration = Duration::from_micros(200);

/// Upper bound on `sync_meshes` pumps per capture. GPU uploads are
/// frame-budgeted to protect interactive frame time; a capture has exactly one
/// frame, so it drains the queue first.
const UPLOAD_PUMP_LIMIT: u32 = 4096;

/// Mesh jobs to retire per pump, generous because nothing here is bounded by a
/// frame deadline.
const MESH_BUDGET: usize = 4096;

/// Day fraction a capture starts at.
const NOON: f32 = 0.25;

pub struct SceneCapture {
    world: World,
    renderer: Renderer,
    camera: Camera,
    shader_params: ShaderParamMap,
    animation_time: f32,
}

impl SceneCapture {
    /// A fresh world at `seed` and a surfaceless renderer drawing
    /// `width` × `height`. `render_distance` is in chunks and drives both
    /// streaming and the fog band, exactly as it does in game.
    pub fn new(seed: u32, render_distance: i32, width: u32, height: u32) -> Self {
        let mut renderer = pollster::block_on(crate::render::new_offscreen_renderer(
            width,
            height,
            CAPTURE_FORMAT,
        ));
        renderer.set_render_distance(render_distance);
        let aspect = width as f32 / height.max(1) as f32;
        let mut this = Self {
            world: World::new(seed, render_distance),
            renderer,
            camera: Camera::new(Vec3::ZERO, aspect),
            shader_params: ShaderParamMap::new(),
            animation_time: 0.0,
        };
        // Without sky params the shaders fall back to their zero state, which is
        // not a visible failure — it is fully lit terrain under a dawn sky, i.e.
        // a believable image of the wrong time of day. Start at noon so a caller
        // who never sets a time still gets a truthful one.
        this.set_time_of_day(NOON, 0.0);
        this
    }

    /// Stream and mesh the world around `pos` (world coordinates), pumping the
    /// generation/light/mesh pools until the terrain stops changing. Returns
    /// `false` if `timeout` ran out first — the capture still works, it is just
    /// incomplete.
    pub fn load_around(&mut self, pos: [f32; 3], timeout: Duration) -> bool {
        let cell = voxel_at(Vec3::from(pos));
        self.world
            .update_load(cell.x >> 4, cell.y >> 4, cell.z >> 4);
        self.settle(timeout)
    }

    /// Pump the generation/light/mesh pools until the terrain stops changing,
    /// without moving the streaming centre. This is the half of
    /// [`SceneCapture::load_around`] a caller wants after editing blocks: the
    /// world is already streamed, it just has to re-light and re-mesh.
    pub fn settle(&mut self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut quiet = 0u32;
        let mut last = (0usize, 0usize);
        while Instant::now() < deadline {
            self.world.poll();
            self.world.tick_mesh_budget(MESH_BUDGET);
            let now = (
                self.world.loaded_section_count(),
                self.world.iter_meshes().count(),
            );
            // The counts must be nonzero as well as stable: a world that has not
            // produced its first section yet is "stable" too, and would settle
            // instantly.
            if now == last && now.1 > 0 && !self.world.has_dirty_meshes() {
                quiet += 1;
                if quiet >= SETTLE_PUMPS {
                    return true;
                }
            } else {
                quiet = 0;
                last = now;
            }
            std::thread::sleep(SETTLE_PUMP_PAUSE);
        }
        false
    }

    /// Place the camera. `yaw`/`pitch` are radians in the game's convention
    /// (yaw 0 looks towards +Z, positive pitch looks up); `fov_y_degrees` is the
    /// vertical field of view. Aspect comes from the capture size.
    pub fn look_from(&mut self, pos: [f32; 3], yaw: f32, pitch: f32, fov_y_degrees: f32) {
        self.camera.pos = Vec3::from(pos);
        self.camera.yaw = yaw;
        self.camera.pitch = pitch;
        self.camera.fov_y = fov_y_degrees.to_radians();
    }

    /// Publish a named visual shader parameter, exactly as a mod would on the
    /// tick. The active shader pack maps declared keys to GPU slots. Non-finite
    /// values are dropped, matching the sim-side setter.
    pub fn set_shader_param(&mut self, key: &str, value: [f32; 4]) {
        if !value.iter().all(|v| v.is_finite()) {
            return;
        }
        self.shader_params.insert(key.to_string(), value);
    }

    /// Put the sky at a point in the day/night cycle: `day_fraction` in `0..1`
    /// (`0.25` = noon, `0.75` = midnight), `moon_phase` in `0..8`. Publishes the
    /// same two shader params the live cycle does, derived the same way, so a
    /// capture's sky matches the game's at that fraction. Defaults to noon.
    pub fn set_time_of_day(&mut self, day_fraction: f32, moon_phase: f32) {
        let (time, light) = crate::server::daynight::sky_params(day_fraction, moon_phase);
        self.set_shader_param(crate::server::daynight::SKY_TIME_PARAM, time);
        self.set_shader_param(crate::server::daynight::SKY_LIGHT_PARAM, light);
    }

    /// Animation clock (seconds) for time-varying visuals: atlas animation,
    /// water caustics, particle phase. Held at 0 by default so repeated captures
    /// of the same scene are identical.
    pub fn set_animation_time(&mut self, seconds: f32) {
        self.animation_time = seconds;
    }

    /// Render one frame and read it back as RGBA8.
    pub fn capture(&mut self) -> RenderedFrame {
        self.publish_camera();
        self.drain_uploads();
        self.renderer.capture_frame()
    }

    /// Publish this capture's camera + environment into the renderer uniforms,
    /// exactly as the game's frame does.
    fn publish_camera(&mut self) {
        let eye = self.camera.pos;
        let (fog, underwater) = crate::game::environment::camera_fog(&self.world, eye, |wx, wz| {
            self.world
                .biome_at_world(wx, wz)
                .map_or(Biome::Plains, Biome::from_id)
        });
        self.renderer.update_uniforms(
            &self.camera,
            fog,
            self.animation_time,
            underwater,
            Some(&self.shader_params),
        );
    }

    /// Drain every pending terrain upload. `sync_meshes` prioritizes by the
    /// frustum the uniforms just published, so this must follow the camera.
    fn drain_uploads(&mut self) {
        for _ in 0..UPLOAD_PUMP_LIMIT {
            {
                let mut terrain = self.world.terrain_render_handoff();
                self.renderer.sync_meshes(&mut terrain);
            }
            if !self.renderer.terrain_uploads_pending() {
                break;
            }
            // A column whose CPU mesh was released re-queues a forced remesh;
            // without pumping, the drain would spin until the cap.
            self.world.tick_mesh_budget(MESH_BUDGET);
            self.world.poll();
        }
    }

    /// Bring the terrain uploads up to date for the current camera, then
    /// render one frame into a reused offscreen target without reading it back.
    /// The frame-cost instrument — [`SceneCapture::capture`]'s per-call texture
    /// and readback would dominate a repeated timing.
    pub fn render_frame(&mut self) {
        self.publish_camera();
        self.drain_uploads();
        self.renderer.render_offscreen();
    }

    /// The renderer behind the camera, for reading its per-frame profile.
    pub fn renderer(&mut self) -> &mut crate::render::Renderer {
        &mut self.renderer
    }

    /// The live world behind the camera, for probing what is being looked at.
    pub fn world(&mut self) -> &mut World {
        &mut self.world
    }
}
