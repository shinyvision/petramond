//! The frame's passes as graph nodes, and their encoding.
//!
//! [`frame_graph`] declares every node the frame can record — its phase, its
//! attachments, what its shaders sample — and [`Renderer::encode_passes`]
//! walks the plan the graph derives from those declarations: one wgpu render
//! pass per merged group, each node recorded by the pass struct that owns its
//! resources (`terrain`, `entities`, `environment`, `hand`, `screen`). Adding
//! a pass is a [`Node`] variant, one declaration row, a gate in
//! [`Renderer::node_active`] and an arm in [`Renderer::record_node`]; where it
//! runs and how its attachments load and store follow from the row.

use super::graph::{
    ColorTarget, DepthTarget, FrameGraph, FramePlan, GraphError, LoadOp, PassGroup, PassNode,
    Phase, Sampled,
};
use super::post_process::SceneRoute;
use super::*;

mod entities;
mod environment;
mod hand;
mod screen;
pub(super) use screen::overlay_pass;
mod terrain;
#[cfg(test)]
mod tests;

/// Every pass the frame can record.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum Node {
    Opaque,
    ContactShadow,
    EntityShadow,
    Sky,
    TerrainModels,
    ItemModels,
    ItemEntities,
    BlockEntities,
    Actors,
    TranslucentBlocks,
    ModelBlend,
    ModelBreak,
    BreakOverlay,
    Particles,
    Fluid,
    EnvDownsample,
    Environment,
    EnvComposite,
    EmitterParticles,
    Outline,
    Ghosts,
    WorldMarksDepth,
    Hand,
    Grade,
    Crosshair,
    Ui,
    UiOverlay,
}

/// The frame's pass table. Each row is a node's whole contract with the
/// rest of the frame; the phases carry the ordering rules (see [`Phase`]).
pub(super) fn frame_graph() -> Result<FrameGraph<Node>, GraphError> {
    use ColorTarget::{EnvColor, MarksEye, Swapchain, World};
    use DepthTarget::{Depth, EnvDepth};
    use LoadOp::{Clear, Load};
    // Most nodes draw into the world colour over the frame depth, loading both.
    let world = |id: Node, label: &'static str, phase: Phase| {
        PassNode::new(id, label, phase)
            .color(World, Load)
            .depth(Depth, Load)
    };
    let screen = |id: Node, label: &'static str| {
        PassNode::new(id, label, Phase::Screen).color(Swapchain, Load)
    };
    FrameGraph::new(vec![
        PassNode::new(Node::Opaque, "opaque pass", Phase::Opaque)
            .color(World, Clear)
            .depth(Depth, Clear),
        world(
            Node::ContactShadow,
            "contact shadow pass",
            Phase::GroundDecal,
        ),
        world(Node::EntityShadow, "entity shadow pass", Phase::GroundDecal),
        world(Node::Sky, "sky pass", Phase::Sky),
        world(Node::TerrainModels, "model pass", Phase::Solid),
        world(Node::ItemModels, "item model pass", Phase::Solid),
        world(Node::ItemEntities, "item entity pass", Phase::Solid),
        world(Node::BlockEntities, "block entity pass", Phase::Solid),
        world(Node::Actors, "mob pass", Phase::Solid),
        world(
            Node::TranslucentBlocks,
            "translucent block pass",
            Phase::Translucent,
        ),
        world(Node::ModelBlend, "model blend pass", Phase::Translucent),
        world(Node::ModelBreak, "model break pass", Phase::BreakDecal),
        world(Node::BreakOverlay, "break overlay pass", Phase::BreakDecal),
        world(Node::Particles, "particle pass", Phase::Cutout),
        world(Node::Fluid, "transparent pass", Phase::Fluid),
        PassNode::new(
            Node::EnvDownsample,
            "env depth downsample",
            Phase::Environment,
        )
        .depth(EnvDepth, Clear)
        .sampling(&[Sampled::Depth]),
        PassNode::new(Node::Environment, "environment pass", Phase::Environment)
            .color(EnvColor, Clear)
            .sampling(&[Sampled::EnvDepth]),
        // No depth attachment: the composite SAMPLES the frame depth.
        PassNode::new(Node::EnvComposite, "env composite pass", Phase::Environment)
            .color(World, Load)
            .sampling(&[Sampled::EnvColor, Sampled::EnvDepth, Sampled::Depth]),
        world(
            Node::EmitterParticles,
            "emitter particle pass",
            Phase::Emitter,
        ),
        world(Node::Outline, "outline pass", Phase::Highlight),
        world(Node::Ghosts, "ghosts and selection", Phase::Highlight),
        // The window's world marks test against the world's depth, which the
        // hand pass clears: keep it, as eye depth, first.
        PassNode::new(Node::WorldMarksDepth, "world marks depth", Phase::Highlight)
            .color(MarksEye, Clear)
            .sampling(&[Sampled::Depth]),
        // Clearing depth gives the hand its own depth space: it stays on top
        // of the world while its held geometry still self-sorts.
        PassNode::new(Node::Hand, "hand pass", Phase::Hand)
            .color(World, Load)
            .depth(Depth, Clear),
        PassNode::new(Node::Grade, "grade pass", Phase::PostProcess)
            .color(Swapchain, Clear)
            .sampling(&[Sampled::World]),
        screen(Node::Crosshair, "crosshair pass"),
        screen(Node::Ui, "ui pass"),
        screen(Node::UiOverlay, "ui overlay / drag pass"),
    ])
}

/// What every node's recording shares.
pub(super) struct PassCtx<'a> {
    /// The world's sample count, which picks each pipeline's variant.
    pub(super) samples: u32,
    pub(super) binds: &'a SharedBinds,
    /// group(0) of the terrain-family pipelines: the frame uniforms, or
    /// their selection-highlight twin while a region selection is shown.
    pub(super) world_bind: &'a wgpu::BindGroup,
}

impl Renderer {
    /// Whether `node` has anything to draw this frame. A node that does not
    /// is left out of the plan entirely — it opens no pass and costs nothing.
    pub(super) fn node_active(&self, node: Node, route: SceneRoute) -> bool {
        let plan = &self.terrain.plan;
        match node {
            Node::Opaque | Node::Sky => true,
            Node::ContactShadow => !plan.contact_columns.is_empty(),
            Node::EntityShadow => self.shadow.active(),
            Node::TerrainModels | Node::ModelBlend => plan.any_model,
            Node::ItemModels => self.item_entity.models_active(),
            Node::ItemEntities => self.item_entity.items_active(),
            Node::BlockEntities => self.block_entity.active(),
            Node::Actors => self.actor.active(),
            Node::TranslucentBlocks | Node::Fluid => plan.any_transparent,
            Node::ModelBreak => self.model_break.active(),
            Node::BreakOverlay => self.hand.break_overlay_active(),
            Node::Particles => self.particle.cutout_active(),
            Node::EnvDownsample | Node::Environment | Node::EnvComposite => {
                self.sky.environment_active()
            }
            Node::EmitterParticles => self.particle.emitters_active(),
            Node::Outline => self.chrome.outline_active(),
            Node::Ghosts => !(self.ghosts.is_empty() && self.selection.is_empty()),
            Node::WorldMarksDepth => self.world_marks_keep_depth(),
            Node::Hand => self.hand.active(),
            Node::Grade => route == SceneRoute::PostProcess,
            Node::Crosshair => self.chrome.crosshair_active(),
            Node::Ui => self.ui.scene.base_active(),
            Node::UiOverlay => self.ui.scene.overlay_active(),
        }
    }

    /// Record `node`'s draws into the open render pass. Every node sets its
    /// own pipeline and binds: it may share the pass with any other node.
    fn record_node(
        &self,
        node: Node,
        pass: &mut wgpu::RenderPass<'_>,
        ctx: &PassCtx<'_>,
        stats: &mut RenderStats,
    ) {
        match node {
            Node::Opaque => self.terrain.record_opaque(pass, ctx, stats),
            Node::ContactShadow => self.terrain.record_contact(pass, ctx),
            Node::EntityShadow => self.shadow.record(pass, ctx),
            Node::Sky => self.sky.record_sky(pass, ctx),
            Node::TerrainModels => self.terrain.record_models(pass, ctx),
            Node::ItemModels => self.item_entity.record_models(pass, ctx),
            Node::ItemEntities => self.item_entity.record_items(pass, ctx),
            Node::BlockEntities => self.block_entity.record(pass, ctx),
            Node::Actors => self.actor.record(pass, ctx),
            Node::TranslucentBlocks => self.terrain.record_translucent(pass, ctx, stats),
            Node::ModelBlend => self.terrain.record_model_blend(pass, ctx),
            Node::ModelBreak => self
                .terrain
                .record_model_break(pass, ctx, &self.model_break),
            Node::BreakOverlay => self.hand.record_break_overlay(pass, ctx),
            Node::Particles => self.particle.record_cutout(pass, ctx),
            Node::Fluid => self.terrain.record_fluid(pass, ctx, stats),
            Node::EnvDownsample => self.sky.record_env_downsample(pass, ctx),
            Node::Environment => self.sky.record_environment(pass),
            Node::EnvComposite => self.sky.record_env_composite(pass, ctx),
            Node::EmitterParticles => self.particle.record_emitters(pass, ctx),
            Node::Outline => self.chrome.record_outline(pass, ctx),
            Node::Ghosts => {
                self.ghosts.draw(pass, ctx.samples);
                self.selection.draw(pass, ctx.samples);
            }
            Node::WorldMarksDepth => self.record_world_marks_depth(pass, ctx),
            Node::Hand => self.hand.record(pass, ctx, &self.actor.player_gpu.bind),
            Node::Grade => self.targets.record_grade(pass),
            Node::Crosshair => self.chrome.record_crosshair(pass),
            Node::Ui => self.ui.record_base(pass, &self.ui.scene),
            Node::UiOverlay => self.ui.record_overlay(pass, &self.ui.scene),
        }
    }

    /// Encode this frame's plan: one render pass per group, each node inside
    /// its own debug group. Reads the baked per-frame buffers off `self`;
    /// mutates only the passed `stats`.
    pub(super) fn encode_passes(
        &self,
        enc: &mut wgpu::CommandEncoder,
        swapchain: &wgpu::TextureView,
        plan: &FramePlan<Node>,
        route: SceneRoute,
        stats: &mut RenderStats,
    ) {
        let ctx = PassCtx {
            samples: self.targets.anti_aliasing.sample_count(),
            binds: &self.binds,
            world_bind: self.selected_blocks_bind(),
        };
        for group in &plan.groups {
            let mut pass = self.begin_group(enc, group, swapchain, route);
            for &(node, label) in &plan.nodes[group.nodes.clone()] {
                pass.push_debug_group(label);
                self.record_node(node, &mut pass, &ctx, stats);
                pass.pop_debug_group();
            }
        }
    }

    /// Open the render pass for `group`, with the load, store and resolve
    /// the graph derived for it.
    fn begin_group<'e>(
        &self,
        enc: &'e mut wgpu::CommandEncoder,
        group: &PassGroup,
        swapchain: &wgpu::TextureView,
        route: SceneRoute,
    ) -> wgpu::RenderPass<'e> {
        let store = |keep: bool| {
            if keep {
                wgpu::StoreOp::Store
            } else {
                wgpu::StoreOp::Discard
            }
        };
        let color = group.color.map(|c| wgpu::RenderPassColorAttachment {
            view: self.color_view(c.target, swapchain, route),
            depth_slice: None,
            resolve_target: group.resolve.then(|| self.resolve_view(swapchain, route)),
            ops: wgpu::Operations {
                load: if c.clear {
                    wgpu::LoadOp::Clear(self.clear_color(c.target))
                } else {
                    wgpu::LoadOp::Load
                },
                store: store(c.store),
            },
        });
        let depth = group.depth.map(|d| wgpu::RenderPassDepthStencilAttachment {
            view: self.depth_view(d.target),
            depth_ops: Some(wgpu::Operations {
                load: if d.clear {
                    wgpu::LoadOp::Clear(1.0)
                } else {
                    wgpu::LoadOp::Load
                },
                store: store(d.store),
            }),
            stencil_ops: None,
        });
        let colors = [color];
        let color_attachments: &[Option<wgpu::RenderPassColorAttachment<'_>>] =
            if colors[0].is_some() { &colors } else { &[] };
        enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(group.label),
            color_attachments,
            depth_stencil_attachment: depth,
            timestamp_writes: self.gpu_timer.as_ref().and_then(|t| t.pass(group.label)),
            occlusion_query_set: None,
        })
    }

    /// The texture behind a colour target this frame. The world draws into
    /// the swapchain when nothing post-processes it, else into the
    /// multisampled colour or the scene texture.
    fn color_view<'a>(
        &'a self,
        target: ColorTarget,
        swapchain: &'a wgpu::TextureView,
        route: SceneRoute,
    ) -> &'a wgpu::TextureView {
        match target {
            ColorTarget::World if route == SceneRoute::Direct => swapchain,
            ColorTarget::World => self
                .targets
                .multisample_color
                .as_ref()
                .unwrap_or(&self.targets.scene_color),
            ColorTarget::EnvColor => &self.sky.env_color,
            ColorTarget::Swapchain => swapchain,
            ColorTarget::MarksEye => self
                .world_marks
                .eye_view()
                .expect("the marks' depth node runs only with their eye target"),
        }
    }

    fn depth_view(&self, target: DepthTarget) -> &wgpu::TextureView {
        match target {
            DepthTarget::Depth => &self.targets.depth,
            DepthTarget::EnvDepth => &self.sky.env_depth,
        }
    }

    /// Where the multisampled world resolves: straight to the swapchain when
    /// nothing post-processes it, else into the scene texture the post pass
    /// reads.
    fn resolve_view<'a>(
        &'a self,
        swapchain: &'a wgpu::TextureView,
        route: SceneRoute,
    ) -> &'a wgpu::TextureView {
        if route == SceneRoute::ResolveToSwapchain {
            swapchain
        } else {
            &self.targets.scene_color
        }
    }

    /// A colour target's clear value: the world clears to the fog colour
    /// (so the horizon matches the fog terrain fades into), the volumetric
    /// target to transparent black (premultiplied compositing over a clear is
    /// the same as compositing over the scene), the swapchain to black.
    fn clear_color(&self, target: ColorTarget) -> wgpu::Color {
        match target {
            ColorTarget::World => {
                let [r, g, b] = self.sky.clear_color;
                wgpu::Color {
                    r: r as f64,
                    g: g as f64,
                    b: b as f64,
                    a: 1.0,
                }
            }
            ColorTarget::EnvColor => wgpu::Color::TRANSPARENT,
            ColorTarget::Swapchain | ColorTarget::MarksEye => wgpu::Color::BLACK,
        }
    }
}
