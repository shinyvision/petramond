use std::time::Instant;

use super::graph::FrameShape;
use super::*;

impl Renderer {
    pub(super) fn set_render_distance(&mut self, chunks: i32) {
        let (start, end) = crate::uniforms::fog_range(chunks);
        self.sky.fog_start = start;
        self.sky.fog_end = end;
    }

    pub(crate) fn terrain_cull_dist(&self) -> f32 {
        self.sky.fog_end + TERRAIN_FOG_CULL_PAD
    }

    pub fn view_volume(&self) -> ViewVolume {
        ViewVolume::new(
            self.view.frustum,
            self.view.render_origin,
            self.view.cam_pos,
            self.terrain_cull_dist(),
            self.pixel_scale(),
        )
    }

    fn pixel_scale(&self) -> f32 {
        0.5 * self.screen_size().1 as f32 * self.view.proj_y_scale
    }

    pub fn set_particle_density(&mut self, density: f32) {
        self.particle.density = density.clamp(0.0, 1.0);
    }

    pub fn gpu_profile(&self) -> Vec<(&'static str, f64, u32)> {
        self.gpu_timer
            .as_ref()
            .map(|t| t.report())
            .unwrap_or_default()
    }

    pub fn terrain_memory(&self) -> TerrainMemory {
        let mut suballocated = 0u64;
        let mut live_allocs = 0usize;
        let mut used = 0u64;
        for col in self.terrain.columns.values() {
            for (_, b) in col.layers() {
                suballocated += b.alloc.capacity();
                used += b.len;
                live_allocs += 1;
            }
        }
        TerrainMemory {
            arena_bytes: self.terrain.geometry.reserved_bytes(),
            arena_free: self.terrain.geometry.free_bytes(),
            arena_blocks: self.terrain.geometry.block_count(),
            suballocated,
            used,
            live_allocs,
            suballocs_since_start: crate::resources::TERRAIN_SUBALLOCS
                .load(std::sync::atomic::Ordering::Relaxed),
        }
    }

    pub fn texture_memory(&self) -> (u64, u64) {
        crate::gpu_mem::texture_totals()
    }

    pub fn texture_memory_by_label(&self) -> Vec<(String, u64)> {
        crate::gpu_mem::texture_by_label()
    }

    pub fn last_terrain_draws(&self) -> (u32, u64, u32, u64) {
        let s = self.last_stats;
        (
            s.opaque_draws,
            s.opaque_indices,
            s.transparent_draws,
            s.transparent_indices,
        )
    }

    pub fn cpu_profile(&self) -> Vec<(&'static str, f64, u32)> {
        self.gpu_timer
            .as_ref()
            .map(|t| t.report_cpu())
            .unwrap_or_default()
    }

    pub fn reset_gpu_profile(&self) {
        if let Some(t) = &self.gpu_timer {
            t.reset();
        }
    }

    pub fn failure(&self) -> Option<RenderFailure> {
        self.health.failure()
    }

    pub fn render(&mut self) {
        if self.health.failure().is_some() {
            return;
        }
        let Some(frame) = self.acquire_swapchain_frame() else {
            return;
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        self.encode_frame(FrameOut {
            scene: &view,
            scene_texture: None,
            window: Some(WindowOut::Scene),
        });
        frame.present();
    }

    pub(super) fn acquire_swapchain_frame(&mut self) -> Option<wgpu::SurfaceTexture> {
        let surface = self.surface.as_ref()?;
        match surface.get_current_texture() {
            Ok(t) if t.suboptimal && !self.suboptimal_retried => {
                self.suboptimal_retried = true;
                drop(t);
                surface.configure(&self.device, &self.surface_config());
                None
            }
            Ok(t) => {
                self.suboptimal_retried = t.suboptimal;
                Some(t)
            }
            Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
                surface.configure(&self.device, &self.surface_config());
                None
            }
            Err(wgpu::SurfaceError::Timeout) => {
                log::warn!("swapchain acquire timed out; skipping a frame");
                None
            }
            Err(wgpu::SurfaceError::OutOfMemory) => {
                self.health.record(RenderFailure::OutOfMemory(
                    "no memory left to acquire a swapchain image".into(),
                ));
                None
            }
            Err(wgpu::SurfaceError::Other) => {
                log::warn!("swapchain acquire failed; skipping a frame");
                None
            }
        }
    }

    pub(super) fn encode_frame(&mut self, out: FrameOut<'_>) -> wgpu::SubmissionIndex {
        let t = Instant::now();
        self.refresh_overlay_buffers();
        self.prepare_held_item();
        self.bake_world_instances();
        self.cpu_stage("cpu: bake world instances", t);

        let t = Instant::now();
        self.plan_draw_order();
        self.cpu_stage("cpu: plan draw order", t);

        let t = Instant::now();
        let route = self.scene_route();
        let shape = FrameShape {
            route,
            msaa: self.targets.anti_aliasing.sample_count() > 1,
            keep_scene: self.captures.world_due,
        };
        let mut plan = std::mem::take(&mut self.frame_plan);
        self.graph
            .plan(shape, |node| self.node_active(node, route), &mut plan);
        debug_assert_eq!(plan.validate(shape), Ok(()), "unrunnable frame plan");
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        let mut stats = RenderStats::default();
        self.encode_passes(&mut enc, out.scene, &plan, route, &mut stats);
        self.frame_plan = plan;
        if let Some(scene) = out.scene_texture {
            self.encode_captures(&mut enc, scene);
        }
        match out.window {
            Some(WindowOut::Scene) => {
                let (w, h) = self.screen_size();
                self.encode_world_marks(&mut enc, out.scene, (0.0, 0.0, w as f32, h as f32));
                self.encode_ui_layer(&mut enc, out.scene, &self.ui.window);
            }
            Some(WindowOut::Letterboxed { view, frame, rect }) => {
                frame.blit_into(&mut enc, view, rect, "frame to window");
                self.encode_world_marks(&mut enc, view, rect);
                self.encode_ui_layer(&mut enc, view, &self.ui.window);
            }
            None => {}
        }
        self.cpu_stage("cpu: encode passes", t);

        if let Some(timer) = &self.gpu_timer {
            timer.finish_frame(&mut enc);
        }
        let t = Instant::now();
        let cb = enc.finish();
        self.cpu_stage("cpu: encoder finish", t);
        let t = Instant::now();
        let submitted = self.queue.submit(std::iter::once(cb));
        self.cpu_stage("cpu: queue submit", t);
        if let Some(timer) = &self.gpu_timer {
            timer.after_submit(&self.device);
        }
        self.last_stats = stats;
        submitted
    }

    fn cpu_stage(&self, label: &'static str, since: Instant) {
        if let Some(timer) = &self.gpu_timer {
            timer.cpu_stage(label, since.elapsed().as_nanos() as f64);
        }
    }
}

pub(super) struct FrameOut<'a> {
    pub(super) scene: &'a wgpu::TextureView,
    pub(super) scene_texture: Option<&'a super::sized_frames::FrameTarget>,
    pub(super) window: Option<WindowOut<'a>>,
}

pub(super) enum WindowOut<'a> {
    Scene,
    Letterboxed {
        view: &'a wgpu::TextureView,
        frame: &'a super::sized_frames::FrameTarget,
        rect: (f32, f32, f32, f32),
    },
}
