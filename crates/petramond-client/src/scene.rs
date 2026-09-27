use std::time::{Duration, Instant};

use petramond::worker::JobPool;
use petramond::world::environment::ShaderParamMap;
use petramond::world::{ReplicaMirror, ReplicaWorld, ServerWorld};
use petramond_math::math::{voxel_at, Vec3};
use petramond_render::camera::Camera;
use petramond_render::Renderer;
use petramond_world::biome::Biome;

pub use petramond_render::RenderedFrame;

const CAPTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

const SETTLE_PUMPS: u32 = 50;

const SETTLE_PUMP_PAUSE: Duration = Duration::from_micros(200);

const UPLOAD_PUMP_LIMIT: u32 = 4096;

const MESH_BUDGET: usize = 4096;

const NOON: f32 = 0.25;

pub struct SceneCapture {
    server: ServerWorld,
    replica: ReplicaWorld,
    mirror: ReplicaMirror,
    renderer: Renderer,
    camera: Camera,
    shader_params: ShaderParamMap,
    animation_time: f32,
    client_mods: petramond::modding::client::ClientModRuntime,
}

impl SceneCapture {
    pub fn new(
        seed: u32,
        render_distance: i32,
        width: u32,
        height: u32,
    ) -> Result<Self, petramond_render::RenderInitError> {
        let mut renderer = pollster::block_on(petramond_render::new_offscreen_renderer(
            width,
            height,
            CAPTURE_FORMAT,
        ))?;
        let mut graphics = petramond::save::client::ClientSettings::default().graphics();
        graphics.render_dist = render_distance;
        renderer.apply_graphics(&graphics);
        let aspect = width as f32 / height.max(1) as f32;
        let enabled: std::collections::BTreeSet<String> = petramond_world::assets::packs()
            .iter()
            .filter_map(|p| p.id.clone())
            .collect();
        let jobs = std::sync::Arc::new(JobPool::new(JobPool::default_threads()));
        let mut server = ServerWorld::with_pool(seed, render_distance, jobs.clone());
        let mirror = ReplicaMirror::new(&mut server);
        let mut this = Self {
            server,
            replica: ReplicaWorld::with_pool(seed, render_distance, jobs),
            mirror,
            renderer,
            camera: Camera::new(petramond_math::world_pos::WorldPos::ZERO, aspect),
            shader_params: ShaderParamMap::new(),
            animation_time: 0.0,
            client_mods: petramond::modding::client::ClientModRuntime::load(
                seed,
                &petramond::modding::client::local_session_key("harness"),
                &enabled,
                mod_api::ClientContext::Local {
                    name: "harness".into(),
                    shared: false,
                },
            ),
        };
        this.set_time_of_day(NOON, 0.0);
        Ok(this)
    }

    pub fn load_around(&mut self, pos: [f32; 3], timeout: Duration) -> bool {
        let cell = voxel_at(Vec3::from(pos));
        self.server
            .update_load(cell.x >> 4, cell.y >> 4, cell.z >> 4);
        self.replica
            .set_replica_view_center(cell.x >> 4, cell.y >> 4, cell.z >> 4);
        self.settle(timeout)
    }

    pub fn settle(&mut self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        self.client_mods.bake_custom_shapes(&mut self.replica);
        let mut quiet = 0u32;
        let mut last = (0usize, 0usize);
        while Instant::now() < deadline {
            self.pump_server();
            self.client_mods.bake_custom_shapes(&mut self.replica);
            self.replica.tick_mesh_budget(MESH_BUDGET);
            let now = (
                self.replica.data().loaded_section_count(),
                self.replica.iter_meshes().count(),
            );
            let busy = self.replica.has_dirty_meshes() || self.server.has_pending_stream_work();
            if now == last && now.1 > 0 && !busy {
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

    pub fn look_from(&mut self, pos: [f32; 3], yaw: f32, pitch: f32, fov_y_degrees: f32) {
        self.camera.pos = petramond_math::world_pos::WorldPos::from_array(pos.map(f64::from));
        self.camera.yaw = yaw;
        self.camera.pitch = pitch;
        self.camera.fov_y = fov_y_degrees.to_radians();
    }

    pub fn set_shader_param(&mut self, key: &str, value: [f32; 4]) {
        if !value.iter().all(|v| v.is_finite()) {
            return;
        }
        self.shader_params.insert(key.to_string(), value);
    }

    pub fn set_time_of_day(&mut self, day_fraction: f32, moon_phase: f32) {
        let (time, light) = petramond::rules::daynight::sky_params(day_fraction, moon_phase);
        self.set_shader_param(petramond::rules::daynight::SKY_TIME_PARAM, time);
        self.set_shader_param(petramond::rules::daynight::SKY_LIGHT_PARAM, light);
    }

    pub fn set_animation_time(&mut self, seconds: f32) {
        self.animation_time = seconds;
    }

    pub fn capture(&mut self) -> RenderedFrame {
        self.publish_camera();
        self.publish_block_draws();
        self.drain_uploads();
        self.renderer.capture_frame()
    }

    fn publish_block_draws(&mut self) {
        let mut rows = Vec::new();
        self.replica.collect_block_draws(
            &petramond_render::camera::ViewVolume::unbounded(),
            &mut rows,
        );
        self.renderer.swap_block_draws(&mut rows);
    }

    fn publish_camera(&mut self) {
        let eye = self.camera.pos;
        let (fog, eye_fluid) =
            crate::game::environment::camera_fog(&self.replica, eye, |wx, wz| {
                self.replica
                    .data()
                    .biome_at_world(wx, wz)
                    .map_or(Biome::PLAINS, Biome::from_id)
            });
        self.renderer.update_uniforms(
            &self.camera,
            fog,
            self.animation_time,
            eye_fluid,
            Some(&self.shader_params),
        );
    }

    fn drain_uploads(&mut self) {
        for _ in 0..UPLOAD_PUMP_LIMIT {
            {
                let mut terrain = self.replica.terrain_render_handoff();
                self.renderer.sync_meshes(&mut terrain);
            }
            if !self.renderer.terrain_uploads_pending() {
                break;
            }
            // A column whose CPU mesh was released re-queues a forced remesh;
            // without pumping, the drain would spin until the cap.
            self.replica.tick_mesh_budget(MESH_BUDGET);
            self.pump_server();
        }
    }

    fn pump_server(&mut self) {
        self.server.poll();
        self.server.pump_light_bakes();
        self.mirror.sync(&mut self.server, &mut self.replica);
    }

    pub fn render_frame(&mut self) {
        self.publish_camera();
        self.publish_block_draws();
        self.drain_uploads();
        self.renderer.render_offscreen();
    }

    pub fn set_mobs(
        &mut self,
        mobs: &[petramond_render::MobRenderInstance],
        arena: &petramond_render::MobArena,
        names: &petramond_render::AnimNames,
    ) {
        self.renderer
            .swap_mobs(&mut mobs.to_vec(), &mut arena.clone(), names);
    }

    pub fn set_player_bodies(
        &mut self,
        bodies: &[petramond_render::PlayerBodyRender],
        poses: &[glam::Mat4],
    ) {
        self.renderer
            .swap_player_bodies(&mut bodies.to_vec(), &mut poses.to_vec());
    }

    pub fn renderer(&mut self) -> &mut petramond_render::Renderer {
        &mut self.renderer
    }

    pub fn world(&mut self) -> &mut ServerWorld {
        &mut self.server
    }
}
