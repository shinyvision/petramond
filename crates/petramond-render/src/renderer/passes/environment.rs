//! The sky and the pack environment (volumetric) nodes with their half-res
//! chain.
//!
//! Volumetrics are pack-supplied full-screen shaders (clouds, auroras, fog
//! volumes) composed in pack load order. Each occludes itself per-fragment
//! against the frame depth, which it SAMPLES — which is why the chain draws
//! after all depth-writing world geometry and attaches no frame depth. The
//! reverse case — a lake in FRONT of a cloudy horizon — needs no paint-order
//! help: the march clamps at the sampled depth, and the lakebed behind a
//! see-through surface is always nearer than any cloud behind the lake.
//!
//! HALF-RES: the passes march into `env_color` (half the scene dims) against
//! `env_depth` — a max-of-2x2 downsample of the frame depth — and a
//! depth-aware composite lifts the premultiplied result onto the scene
//! (crisp at silhouette edges, bilinear elsewhere). A volumetric is soft, so
//! this quarters its fragment cost invisibly; see `pipeline::EnvScaler` and
//! the two `env_*.wgsl` builtins.

use super::*;

impl SkyPass {
    pub(super) fn record_sky(&self, pass: &mut wgpu::RenderPass<'_>, ctx: &PassCtx<'_>) {
        pass.set_pipeline(self.pipe.get(ctx.samples));
        pass.set_bind_group(0, &self.bind, &[]);
        pass.set_bind_group(1, &self.texture_bind, &[]);
        pass.draw(0..3, 0..1);
    }

    pub(super) fn environment_active(&self) -> bool {
        self.env_passes.iter().any(|env| !env.dormant)
    }

    pub(super) fn record_env_downsample(&self, pass: &mut wgpu::RenderPass<'_>, ctx: &PassCtx<'_>) {
        pass.set_pipeline(&self.env_scaler.get(ctx.samples).down_pipe);
        pass.set_bind_group(0, &self.env_down_bind, &[]);
        pass.draw(0..3, 0..1);
    }

    pub(super) fn record_environment(&self, pass: &mut wgpu::RenderPass<'_>) {
        for env in self.env_passes.iter().filter(|env| !env.dormant) {
            pass.set_pipeline(&env.res.pipe);
            pass.set_bind_group(0, &env.bind, &[]);
            pass.set_bind_group(1, &env.res.texture_bind, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    pub(super) fn record_env_composite(&self, pass: &mut wgpu::RenderPass<'_>, ctx: &PassCtx<'_>) {
        let scaler = self.env_scaler.get(ctx.samples);
        pass.set_pipeline(scaler.comp_pipe.get(ctx.samples));
        pass.set_bind_group(0, &self.env_comp_bind, &[]);
        pass.draw(0..3, 0..1);
    }
}
