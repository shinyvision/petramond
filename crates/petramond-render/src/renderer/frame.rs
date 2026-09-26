//! The renderer's frame orchestration and public knobs: fog coupling, the
//! census/profile readouts, and `render` → acquire, bake, plan, encode, submit.

use std::time::Instant;

use super::graph::FrameShape;
use super::*;

impl Renderer {
    /// Couple the fog band (and with it the terrain draw-cull distance) to the
    /// streaming render distance, so the fade always ends at the loaded edge.
    pub(super) fn set_render_distance(&mut self, chunks: i32) {
        let (start, end) = crate::uniforms::fog_range(chunks);
        self.sky.fog_start = start;
        self.sky.fog_end = end;
    }

    /// Terrain draw-cull distance: nothing beyond this is fully un-fogged.
    pub(crate) fn terrain_cull_dist(&self) -> f32 {
        self.sky.fog_end + TERRAIN_FOG_CULL_PAD
    }

    /// What this frame can draw, as published by the last
    /// [`update_uniforms`](Self::update_uniforms): the culling frustum and the
    /// fog cull distance. Hand it to a per-frame gather so the gather's cost
    /// tracks what is visible instead of what is loaded.
    pub fn view_volume(&self) -> ViewVolume {
        ViewVolume::new(
            self.view.frustum,
            self.view.render_origin,
            self.view.cam_pos,
            self.terrain_cull_dist(),
            self.pixel_scale(),
        )
    }

    /// Presented pixels one world block spans at one block of distance. The
    /// PRESENTED size, not the supersampled scene's: a gather asking "can this
    /// be seen" is asking about the display, and SSAA must not quietly extend
    /// how far small detail is built.
    fn pixel_scale(&self) -> f32 {
        0.5 * self.screen_size().1 as f32 * self.view.proj_y_scale
    }

    /// Emitter-derived particle density from the particles graphics option
    /// (`0` = off, `0.5` = reduced, `1` = full). Scales each looping emitter's
    /// active-particle count; zero skips emitter baking entirely.
    pub fn set_particle_density(&mut self, density: f32) {
        self.particle.density = density.clamp(0.0, 1.0);
    }

    /// Accumulated GPU nanoseconds per pass over the frames measured since the last
    /// [`Renderer::reset_gpu_profile`], as `(label, total_ns, frames)`. Empty
    /// unless `PETRAMOND_GPU_TIMING` is set.
    pub fn gpu_profile(&self) -> Vec<(&'static str, f64, u32)> {
        self.gpu_timer
            .as_ref()
            .map(|t| t.report())
            .unwrap_or_default()
    }

    /// Where the packed terrain columns' GPU memory actually is. The
    /// renderer's dominant VRAM consumer at high render distance, and the
    /// number that says whether an allocation-policy change paid.
    pub fn terrain_memory(&self) -> TerrainMemory {
        let mut suballocated = 0u64;
        let mut live_allocs = 0usize;
        let mut used = 0u64;
        for col in self.terrain.columns.values() {
            for b in [
                &col.opaque_vbuf,
                &col.transparent_vbuf,
                &col.transparent_ts_vbuf,
                &col.translucent_vbuf,
                &col.model_vbuf,
                &col.model_ibuf,
                &col.contact_vbuf,
            ]
            .into_iter()
            .flatten()
            {
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

    /// `(bytes, count)` of every GPU texture created this process (see
    /// [`crate::gpu_mem`]). Gross, not net: resize-replaced targets are
    /// counted each time.
    pub fn texture_memory(&self) -> (u64, u64) {
        crate::gpu_mem::texture_totals()
    }

    /// Texture bytes per descriptor label, largest first.
    pub fn texture_memory_by_label(&self) -> Vec<(String, u64)> {
        crate::gpu_mem::texture_by_label()
    }

    /// Terrain draw work submitted by the last encoded frame:
    /// `(opaque draws, opaque indices, transparent draws, transparent indices)`.
    pub fn last_terrain_draws(&self) -> (u32, u64, u32, u64) {
        let s = self.last_stats;
        (
            s.opaque_draws,
            s.opaque_indices,
            s.transparent_draws,
            s.transparent_indices,
        )
    }

    /// Mean CPU nanoseconds per frame stage, same shape as [`Renderer::gpu_profile`].
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

    /// The failure that stopped this renderer, if any: the device was lost
    /// or the GPU ran out of memory. A renderer that reports one draws
    /// nothing more; the host rebuilds it (device loss) or reports the error
    /// (out of memory). Check after every [`render`](Self::render).
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
        self.encode_frame(&view);
        frame.present();
    }

    /// The swapchain image to draw into, or `None` when this frame draws
    /// nothing: a surfaceless renderer, a swapchain that needed rebuilding
    /// first, an acquire that timed out or failed, or no memory left for a
    /// frame (recorded as this renderer's [`failure`](Self::failure)).
    fn acquire_swapchain_frame(&mut self) -> Option<wgpu::SurfaceTexture> {
        let surface = self.surface.as_ref()?;
        match surface.get_current_texture() {
            // A suboptimal frame still presents (with a per-present driver
            // warning), but the swapchain no longer matches the surface —
            // rebuild it once and draw from the fresh one next frame. The
            // frame must drop BEFORE the reconfigure (a live SurfaceTexture
            // across a swapchain rebuild panics).
            Ok(t) if t.suboptimal && !self.suboptimal_retried => {
                self.suboptimal_retried = true;
                drop(t);
                surface.configure(&self.device, &self.config);
                None
            }
            Ok(t) => {
                self.suboptimal_retried = t.suboptimal;
                Some(t)
            }
            // Stale/lost swapchain (a resize or compositor change the events
            // haven't delivered yet): reconfigure at the current size and let
            // the next frame draw.
            Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
                surface.configure(&self.device, &self.config);
                None
            }
            // The compositor did not hand an image back in time (a hidden or
            // minimized window, a stalled present queue): skip this frame.
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
            // The device's error callback has the details.
            Err(wgpu::SurfaceError::Other) => {
                log::warn!("swapchain acquire failed; skipping a frame");
                None
            }
        }
    }

    /// Everything between "here is the colour target" and "the GPU has this
    /// frame": the per-frame CPU bakes, draw planning, the frame graph's
    /// plan, pass encoding, submit. Target-agnostic, so the windowed
    /// swapchain and an offscreen capture share one frame graph.
    pub(super) fn encode_frame(&mut self, view: &wgpu::TextureView) {
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
        };
        // The reusable plan is taken out while `self` is read to fill it,
        // then put back (capacity retained next frame).
        let mut plan = std::mem::take(&mut self.frame_plan);
        self.graph.plan(shape, |node| self.node_active(node, route), &mut plan);
        debug_assert_eq!(plan.validate(shape), Ok(()), "unrunnable frame plan");
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        let mut stats = RenderStats::default();
        self.encode_passes(&mut enc, view, &plan, route, &mut stats);
        self.frame_plan = plan;
        self.cpu_stage("cpu: encode passes", t);

        if let Some(timer) = &self.gpu_timer {
            timer.finish_frame(&mut enc);
        }
        let t = Instant::now();
        let cb = enc.finish();
        self.cpu_stage("cpu: encoder finish", t);
        let t = Instant::now();
        self.queue.submit(std::iter::once(cb));
        self.cpu_stage("cpu: queue submit", t);
        if let Some(timer) = &self.gpu_timer {
            timer.after_submit(&self.device);
        }
        self.last_stats = stats;
    }

    /// Fold the CPU time since `since` into the profile under `label` (a
    /// no-op unless GPU timing is on).
    fn cpu_stage(&self, label: &'static str, since: Instant) {
        if let Some(timer) = &self.gpu_timer {
            timer.cpu_stage(label, since.elapsed().as_nanos() as f64);
        }
    }
}
