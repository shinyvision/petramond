use super::*;
use petramond::save::client::{AntiAliasing, GraphicsSettings};

/// How the finished world image reaches the swapchain this frame.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum SceneRoute {
    /// Single-sample scene at native size with no grade: the world passes
    /// draw into the swapchain and nothing is copied.
    Direct,
    /// Multisampled scene with no grade: the MSAA resolve writes the
    /// swapchain itself — no scene texture round-trip, no full-screen copy.
    ResolveToSwapchain,
    /// Everything else: scene texture → post-process pass (supersample
    /// reduction, render-scale upscale, colour grade) → swapchain.
    PostProcess,
}

impl Renderer {
    /// Apply every renderer-owned graphics setting at once — the ONE path from
    /// `client.json` to the GPU, at window creation and on every options
    /// change alike (the per-knob setters below are renderer-private for that
    /// reason). Returns the anti-aliasing mode that actually runs (the
    /// requested one, or the nearest the device and viewport support), which
    /// the caller writes back so the options readout tells the truth.
    pub fn apply_graphics(&mut self, settings: &GraphicsSettings) -> AntiAliasing {
        self.set_render_distance(settings.render_dist);
        self.set_particle_density(settings.particle_density);
        self.hand.screen_shake = settings.screen_shake;
        self.set_render_scale(settings.render_scale);
        self.set_grade_enabled(settings.grade);
        self.set_anti_aliasing(settings.anti_aliasing)
    }

    /// Set the world-resolution scale used with AA Off (clamped `0.5..=1.0`).
    /// AA modes use native output size as their base; the stored scale resumes when AA is off.
    pub(super) fn set_render_scale(&mut self, scale: f32) {
        let scale = if scale.is_finite() {
            scale.clamp(0.5, 1.0)
        } else {
            1.0
        };
        if (scale - self.targets.render_scale).abs() < f32::EPSILON {
            return;
        }
        let previous = self.scene_dims();
        self.targets.render_scale = scale;
        if self.scene_dims() != previous {
            self.recreate_scene_targets();
        }
    }

    /// Toggle the world colour grade independently of scene sampling.
    pub(super) fn set_grade_enabled(&mut self, on: bool) {
        if self.targets.grade_enabled != on {
            self.targets.grade_enabled = on;
            self.upload_post_process();
        }
    }

    /// The applied scene sampling mode, including device and texture-dimension limits.
    pub fn anti_aliasing(&self) -> AntiAliasing {
        self.targets.anti_aliasing
    }

    /// Maximum per-axis multiplier that fits this viewport on the current device.
    pub(super) fn max_anti_aliasing_multiplier(&self) -> u32 {
        max_multiplier(
            self.screen_size(),
            self.device.limits().max_texture_dimension_2d,
        )
    }

    pub fn max_anti_aliasing_samples(&self) -> u32 {
        self.targets.max_samples
    }

    /// Sample the scene before drawing the native-resolution HUD and UI.
    /// Returns the mode that fits the current device and viewport.
    pub(super) fn set_anti_aliasing(&mut self, requested: AntiAliasing) -> AntiAliasing {
        let mode = supported_mode(
            requested,
            self.max_anti_aliasing_multiplier(),
            self.targets.max_samples,
        );
        if self.targets.anti_aliasing != mode {
            self.targets.anti_aliasing = mode;
            self.recreate_scene_targets();
        }
        mode
    }

    pub(super) fn upload_post_process(&self) {
        self.queue.write_buffer(
            &self.targets.post_process_buf,
            0,
            bytemuck::cast_slice(&[
                self.targets.mood[0],
                self.targets.mood[1],
                self.targets.anti_aliasing.resolution_multiplier() as f32,
                u8::from(self.targets.grade_enabled) as f32,
            ]),
        );
    }

    pub(crate) fn scene_route(&self) -> SceneRoute {
        if self.captures.world_due {
            return SceneRoute::PostProcess;
        }
        let native = self.scene_dims() == self.screen_size();
        match (
            self.targets.grade_enabled,
            self.targets.anti_aliasing.sample_count(),
        ) {
            (false, 1) if native => SceneRoute::Direct,
            (false, _) if native => SceneRoute::ResolveToSwapchain,
            _ => SceneRoute::PostProcess,
        }
    }

    pub(crate) fn scene_dims(&self) -> (u32, u32) {
        scene_dimensions(
            self.screen_size(),
            self.targets.render_scale,
            self.targets.anti_aliasing,
        )
    }
}

pub(super) fn max_multiplier((w, h): (u32, u32), limit: u32) -> u32 {
    (limit / w.max(h).max(1)).max(1)
}

pub(super) fn supported_mode(requested: AntiAliasing, max: u32, max_samples: u32) -> AntiAliasing {
    if requested.sample_count() > 1 {
        return [
            AntiAliasing::Msaa8x,
            AntiAliasing::Msaa4x,
            AntiAliasing::Off,
        ]
        .into_iter()
        .find(|m| m.sample_count() <= requested.sample_count() && m.sample_count() <= max_samples)
        .unwrap();
    }
    [
        AntiAliasing::Ssaa16x,
        AntiAliasing::Ssaa4x,
        AntiAliasing::Off,
    ]
    .into_iter()
    .find(|m| {
        m.resolution_multiplier() <= max
            && m.resolution_multiplier() <= requested.resolution_multiplier()
    })
    .unwrap_or(AntiAliasing::Off)
}

fn scene_dimensions((w, h): (u32, u32), render_scale: f32, aa: AntiAliasing) -> (u32, u32) {
    if aa != AntiAliasing::Off {
        let multiplier = aa.resolution_multiplier();
        (w.max(1) * multiplier, h.max(1) * multiplier)
    } else {
        (
            ((w as f32 * render_scale).round() as u32).max(1),
            ((h as f32 * render_scale).round() as u32).max(1),
        )
    }
}

impl Renderer {
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 || self.resize_under_sized_frames(width, height) {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.ui.scene_generation = self.ui.scene_generation.wrapping_add(1).max(1);
        self.ui.window_generation = self.ui.window_generation.wrapping_add(1).max(1);
        if let Some(surface) = &self.surface {
            surface.configure(&self.device, &self.config);
        }
        self.recreate_scene_targets();
        self.chrome.crosshair_drawn_size = (0, 0);
        // A real size change earns a fresh suboptimal-retry (render()).
        self.suboptimal_retried = false;
    }

    /// (Re)build the world-pass targets at the current `render_scale` (and the
    /// grade bind that reads them), then the volumetric targets over the new
    /// depth. Called on resize and scale changes.
    pub(super) fn recreate_scene_targets(&mut self) {
        self.targets.anti_aliasing = super::post_process::supported_mode(
            self.targets.anti_aliasing,
            self.max_anti_aliasing_multiplier(),
            self.targets.max_samples,
        );
        self.upload_post_process();
        let scene = self.scene_dims();
        self.targets
            .recreate_views(&self.device, self.config.format, scene);
        self.sky.recreate_env_targets(
            &self.device,
            &self.binds.uniform_buf,
            &self.targets.depth,
            scene,
            self.config.format,
            self.targets.anti_aliasing.sample_count(),
        );
    }
}

impl SceneTargets {
    /// The scene targets at `scene` dims for `anti_aliasing`, with the
    /// post-process controls and the grade bind that reads them.
    pub(super) fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        (w, h): (u32, u32),
        anti_aliasing: AntiAliasing,
        max_samples: u32,
        grade_pipe: wgpu::RenderPipeline,
        grade_bgl: wgpu::BindGroupLayout,
    ) -> Self {
        let samples = anti_aliasing.sample_count();
        let scene_color = create_scene_color(device, w, h, format);
        let post_process_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("post-process controls"),
            contents: bytemuck::cast_slice(&[
                0.0_f32,
                0.0,
                anti_aliasing.resolution_multiplier() as f32,
                1.0,
            ]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let grade_bind = super::super::pipeline::create_grade_bind(
            device,
            &grade_bgl,
            &scene_color,
            &post_process_buf,
        );
        Self {
            scene_color,
            multisample_color: create_multisample_color(device, w, h, format, samples),
            max_samples,
            depth: crate::resources::create_depth_sampled(device, w, h, samples),
            render_scale: 1.0,
            grade_enabled: true,
            anti_aliasing,
            grade_pipe,
            grade_bgl,
            grade_bind,
            post_process_buf,
            mood: [0.0, 0.0],
        }
    }

    /// Rebuild the views at `scene` dims for the current sample count, and
    /// the grade bind over the new scene colour.
    fn recreate_views(
        &mut self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        (w, h): (u32, u32),
    ) {
        let samples = self.anti_aliasing.sample_count();
        self.depth = crate::resources::create_depth_sampled(device, w, h, samples);
        self.multisample_color = create_multisample_color(device, w, h, format, samples);
        self.scene_color = create_scene_color(device, w, h, format);
        self.grade_bind = super::super::pipeline::create_grade_bind(
            device,
            &self.grade_bgl,
            &self.scene_color,
            &self.post_process_buf,
        );
    }
}

fn create_multisample_color(
    device: &wgpu::Device,
    w: u32,
    h: u32,
    format: wgpu::TextureFormat,
    samples: u32,
) -> Option<wgpu::TextureView> {
    (samples > 1)
        .then(|| crate::resources::create_scene_color_sampled(device, w, h, format, samples))
}

#[cfg(test)]
mod tests;
