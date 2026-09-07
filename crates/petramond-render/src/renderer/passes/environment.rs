//! The pack environment (volumetric) passes and their half-res chain.

use super::*;

impl Renderer {
    /// Downsample the frame depth, march every live environment pass into the
    /// half-res target, and composite the result onto the scene.
    pub(super) fn encode_environment(
        &self,
        enc: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        samples: u32,
    ) {
        // ENVIRONMENT (VOLUMETRIC) PASSES: pack-supplied full-screen shaders
        // (clouds, auroras, fog volumes), composed in pack load order. Drawn
        // after ALL depth-writing world geometry so each shader can occlude
        // itself per-fragment against the frame depth, which it SAMPLES
        // (group 0 binding 2) — the pass attaches no depth, which is what
        // makes sampling it legal. Drawn AFTER the water pass: water writes no
        // depth, so paint order is the only thing keeping a cloud in front of
        // a lake (camera on a peak inside the deck, lake below punched a hole
        // through the cloud when water drew last). The reverse case — a lake
        // in FRONT of a cloudy horizon — needs no paint-order help: the march
        // clamps at the sampled depth, and the lakeBED behind the surface is
        // always nearer than any cloud behind the lake. Drawn BEFORE the
        // emitter particles so rain/snow volumes (no depth write) still streak
        // over the deck.
        //
        // HALF-RES: the passes march into `env_color` (half the scene dims)
        // against `env_depth` — a max-of-2x2 downsample of the frame depth —
        // and a depth-aware composite lifts the premultiplied result onto
        // the scene (crisp at silhouette edges, bilinear elsewhere). A
        // volumetric is soft, so this quarters its fragment cost invisibly;
        // see pipeline::EnvScaler and the two env_*.wgsl builtins.
        if self.sky.env_passes.iter().any(|env| !env.dormant) {
            {
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("env depth downsample"),
                    color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &self.sky.env_depth,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: self
                        .gpu_timer
                        .as_ref()
                        .and_then(|t| t.pass("env depth downsample")),
                    ..Default::default()
                });
                pass.set_pipeline(&self.sky.env_scaler.get(samples).down_pipe);
                pass.set_bind_group(0, &self.sky.env_down_bind, &[]);
                pass.draw(0..3, 0..1);
            }
            {
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("environment pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.sky.env_color,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            // Transparent black: premultiplied compositing is
                            // associative, so (passes over clear) over scene
                            // equals the old passes-over-scene directly.
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: self
                        .gpu_timer
                        .as_ref()
                        .and_then(|t| t.pass("environment pass")),
                    ..Default::default()
                });
                for env in self.sky.env_passes.iter().filter(|env| !env.dormant) {
                    pass.set_pipeline(&env.res.pipe);
                    pass.set_bind_group(0, &env.bind, &[]);
                    pass.set_bind_group(1, &env.res.texture_bind, &[]);
                    pass.draw(0..3, 0..1);
                }
            }
            {
                let mut pass = color_depth_pass(
                    enc,
                    view,
                    &self.targets.depth,
                    "env composite pass",
                    wgpu::LoadOp::Load,
                    None,
                    self.gpu_timer.as_ref(),
                );
                pass.set_pipeline(self.sky.env_scaler.get(samples).comp_pipe.get(samples));
                pass.set_bind_group(0, &self.sky.env_comp_bind, &[]);
                pass.draw(0..3, 0..1);
            }
        }
    }
}
