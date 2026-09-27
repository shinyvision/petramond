//! The frame graph: every pass the frame can record, declared as a node with
//! its phase, its attachments and the targets its shaders sample — and the
//! frame's render passes derived from those declarations.
//!
//! Order is the phase order ([`Phase`]); within a phase, nodes keep the
//! order they were declared in. The ordering contracts between passes (a
//! stamp before the sky, ice before its crack, the crack before the water)
//! are the phase list's own order, pinned by tests, rather than prose next to
//! a hand-ordered encode function.
//!
//! Each frame [`FrameGraph::plan`] takes the nodes that have something to
//! draw and:
//! - merges consecutive nodes with identical attachments into ONE render
//!   pass — a node that clears an attachment always starts a new one, since
//!   a clear can only happen at a pass boundary;
//! - clears an attachment only where a node asks for it, and loads it
//!   otherwise;
//! - stores an attachment only when a later pass loads or samples it before
//!   anything clears it, or when it is the frame's output — everything else
//!   is discarded, which is what lets a tiled GPU keep the target on chip;
//! - resolves a multisampled world at the end of the last pass drawing it.
//!
//! The planning is pure data: the tests exercise it without a GPU.

use std::fmt;
use std::ops::Range;

use super::post_process::SceneRoute;

/// When a node draws, as a total order over the frame. Each variant's doc
/// states why it sits where it does; `passes/tests.rs` pins every contract.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Phase {
    /// Opaque terrain, near to far for early-Z. The frame's first world
    /// draw: it clears colour (to the fog colour) and depth.
    Opaque,
    /// Multiply-blended ground stamps (model contact shadows, entity blob
    /// shadows) that test depth but write none. BEFORE the sky: if a stamp's
    /// supporting terrain was culled while the stamp stayed visible, the
    /// sky's far-plane draw replaces the orphaned darkening instead of
    /// leaving a smudge on the background.
    GroundDecal,
    /// The full-screen sky at the far plane. AFTER opaque geometry, so its
    /// LessEqual test shades only the pixels nothing covered (the sky
    /// fragment shader is the priciest full-screen one).
    Sky,
    /// Depth-writing solids that are not terrain: placed models, dropped
    /// items, animated blocks, mobs and players.
    Solid,
    /// Alpha-blended but depth-writing surfaces (ice, a model's glass), each
    /// resolving its own face order through depth. BEFORE the break decals,
    /// so a crack on mined ice draws on top of the ice instead of being
    /// washed out under it.
    Translucent,
    /// The destroy cracks, model and block. After translucent surfaces and
    /// BEFORE the fluid: a crack is a decal on its block, so water must blend
    /// in front of a submerged one.
    BreakDecal,
    /// Alpha-cutout particles, depth-writing. After the cracks (they sit in
    /// front of them) and BEFORE the fluid, so water blends over the ones
    /// behind it while the ones in front still occlude it.
    Cutout,
    /// See-through fluid, far to near, depth test only.
    Fluid,
    /// Pack volumetrics. They SAMPLE the frame depth, so they follow all
    /// depth-writing world geometry — and the fluid, whose surface writes no
    /// depth, so only paint order keeps a cloud in front of a lake.
    Environment,
    /// Alpha-blended emitter particles (no depth write), after the
    /// volumetrics so flames and rain streak over a cloud deck.
    Emitter,
    /// World-space highlights: the block outline, build ghosts, the region
    /// selection — over everything the world drew.
    Highlight,
    /// The first-person hand, in its own cleared depth.
    Hand,
    /// The finished world image to the swapchain: supersample reduction,
    /// render-scale upscale, colour grade.
    PostProcess,
    /// Screen chrome, drawn ungraded over the final image.
    Screen,
}

/// A colour target a node can draw into.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum ColorTarget {
    /// The world image. Physically the swapchain, the multisampled colour or
    /// the scene texture, by the frame's [`SceneRoute`].
    World,
    /// The half-resolution volumetric colour.
    EnvColor,
    /// The presented image.
    Swapchain,
    /// The world's eye depth the window's world marks test against, kept
    /// before the hand clears the frame depth. Read after the frame.
    MarksEye,
}

/// A depth target a node can attach.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum DepthTarget {
    /// The frame depth (multisampled with the world).
    Depth,
    /// The half-resolution depth the volumetrics march against.
    EnvDepth,
}

/// A target a node's shaders read as a texture.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Sampled {
    /// The finished world image (the resolve's output under MSAA).
    World,
    EnvColor,
    Depth,
    EnvDepth,
}

/// What a node needs an attachment to hold when it starts drawing.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum LoadOp {
    /// Whatever the frame drew into it so far.
    Load,
    /// A fresh clear (the target's clear value).
    Clear,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct Attachment<T> {
    pub(crate) target: T,
    pub(crate) load: LoadOp,
}

/// One declared node: identity, place in the frame, and resource use.
pub(crate) struct PassNode<N> {
    pub(crate) id: N,
    /// Debug-group label (and the render-pass label when the node opens one).
    pub(crate) label: &'static str,
    pub(crate) phase: Phase,
    pub(crate) color: Option<Attachment<ColorTarget>>,
    pub(crate) depth: Option<Attachment<DepthTarget>>,
    pub(crate) samples: &'static [Sampled],
}

impl<N> PassNode<N> {
    pub(crate) fn new(id: N, label: &'static str, phase: Phase) -> Self {
        Self {
            id,
            label,
            phase,
            color: None,
            depth: None,
            samples: &[],
        }
    }

    pub(crate) fn color(self, target: ColorTarget, load: LoadOp) -> Self {
        Self {
            color: Some(Attachment { target, load }),
            ..self
        }
    }

    pub(crate) fn depth(self, target: DepthTarget, load: LoadOp) -> Self {
        Self {
            depth: Some(Attachment { target, load }),
            ..self
        }
    }

    pub(crate) fn sampling(self, samples: &'static [Sampled]) -> Self {
        Self { samples, ..self }
    }

    /// Whether this node attaches the target `sampled` names — sampling it in
    /// the same pass would be a feedback loop under every route.
    fn attaches(&self, sampled: Sampled) -> bool {
        let color = self.color.map(|c| c.target);
        let depth = self.depth.map(|d| d.target);
        match sampled {
            Sampled::World => color == Some(ColorTarget::World),
            Sampled::EnvColor => color == Some(ColorTarget::EnvColor),
            Sampled::Depth => depth == Some(DepthTarget::Depth),
            Sampled::EnvDepth => depth == Some(DepthTarget::EnvDepth),
        }
    }
}

/// A physical GPU resource behind the logical targets. The load/store
/// analysis runs on these, because a route can alias two logical targets
/// (the world IS the swapchain when nothing post-processes it).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Resource {
    Swapchain,
    SceneColor,
    MultisampleColor,
    EnvColor,
    Depth,
    EnvDepth,
    MarksEye,
}

impl Resource {
    const ALL: [Self; 7] = [
        Self::Swapchain,
        Self::SceneColor,
        Self::MultisampleColor,
        Self::EnvColor,
        Self::Depth,
        Self::EnvDepth,
        Self::MarksEye,
    ];

    const fn bit(self) -> u8 {
        1 << self as u8
    }
}

/// How this frame's world reaches the screen, which decides what the
/// logical targets are physically.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct FrameShape {
    pub(crate) route: SceneRoute,
    /// The world colour and depth are multisampled.
    pub(crate) msaa: bool,
    /// The scene texture is read after the frame: a world capture grades it
    /// again into a target of its own.
    pub(crate) keep_scene: bool,
}

impl FrameShape {
    fn color(self, target: ColorTarget) -> Resource {
        match target {
            ColorTarget::World => match (self.route, self.msaa) {
                (SceneRoute::Direct, _) => Resource::Swapchain,
                (_, true) => Resource::MultisampleColor,
                _ => Resource::SceneColor,
            },
            ColorTarget::EnvColor => Resource::EnvColor,
            ColorTarget::Swapchain => Resource::Swapchain,
            ColorTarget::MarksEye => Resource::MarksEye,
        }
    }

    /// Whether `resource` is read after the frame's last pass, and so must
    /// survive it whatever the passes do.
    fn read_after(self, resource: Resource) -> bool {
        match resource {
            Resource::Swapchain | Resource::MarksEye => true,
            Resource::SceneColor => self.keep_scene,
            _ => false,
        }
    }

    fn depth(self, target: DepthTarget) -> Resource {
        match target {
            DepthTarget::Depth => Resource::Depth,
            DepthTarget::EnvDepth => Resource::EnvDepth,
        }
    }

    fn sampled(self, sampled: Sampled) -> Resource {
        match sampled {
            Sampled::World if self.route == SceneRoute::Direct => Resource::Swapchain,
            Sampled::World => Resource::SceneColor,
            Sampled::EnvColor => Resource::EnvColor,
            Sampled::Depth => Resource::Depth,
            Sampled::EnvDepth => Resource::EnvDepth,
        }
    }

    /// Where the multisampled world resolves to.
    fn resolve_destination(self) -> Resource {
        match self.route {
            SceneRoute::ResolveToSwapchain => Resource::Swapchain,
            SceneRoute::Direct | SceneRoute::PostProcess => Resource::SceneColor,
        }
    }
}

/// An attachment as one render pass uses it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct AttachmentOps<T> {
    pub(crate) target: T,
    /// Clear on load (else load the previous contents).
    pub(crate) clear: bool,
    /// Keep the contents when the pass ends (else discard them).
    pub(crate) store: bool,
}

/// One wgpu render pass: consecutive nodes sharing its attachments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PassGroup {
    /// The first node's label, which names the pass (and its GPU timing).
    pub(crate) label: &'static str,
    pub(crate) color: Option<AttachmentOps<ColorTarget>>,
    pub(crate) depth: Option<AttachmentOps<DepthTarget>>,
    /// Resolve the multisampled world at the end of this pass.
    pub(crate) resolve: bool,
    /// The pass's nodes, as a range of [`FramePlan::nodes`].
    pub(crate) nodes: Range<usize>,
    /// [`Resource`] bits the pass's shaders sample.
    sampled: u8,
}

impl PassGroup {
    /// `(resource, cleared)` for each attachment.
    fn attachments(&self, shape: FrameShape) -> impl Iterator<Item = (Resource, bool)> {
        let color = self.color.map(|c| (shape.color(c.target), c.clear));
        let depth = self.depth.map(|d| (shape.depth(d.target), d.clear));
        color.into_iter().chain(depth)
    }

    fn attached(&self, shape: FrameShape) -> u8 {
        self.attachments(shape)
            .fold(0, |bits, (r, _)| bits | r.bit())
    }
}

/// One frame's render passes, in order. Reused frame to frame.
pub(crate) struct FramePlan<N> {
    pub(crate) groups: Vec<PassGroup>,
    /// `(node, label)` in recording order; each group owns a range.
    pub(crate) nodes: Vec<(N, &'static str)>,
}

impl<N> Default for FramePlan<N> {
    fn default() -> Self {
        Self {
            groups: Vec::new(),
            nodes: Vec::new(),
        }
    }
}

impl<N> FramePlan<N> {
    /// Check the plan reads nothing the frame has not written yet and never
    /// samples a resource the same pass draws into.
    pub(crate) fn validate(&self, shape: FrameShape) -> Result<(), GraphError> {
        let mut written = 0u8;
        for group in &self.groups {
            if group.sampled & group.attached(shape) != 0 {
                return Err(GraphError::Feedback(group.label));
            }
            let loads = group
                .attachments(shape)
                .filter(|&(_, cleared)| !cleared)
                .map(|(resource, _)| resource);
            let samples = Resource::ALL
                .into_iter()
                .filter(|r| group.sampled & r.bit() != 0);
            if let Some(resource) = loads.chain(samples).find(|r| written & r.bit() == 0) {
                return Err(GraphError::ReadBeforeWrite {
                    pass: group.label,
                    resource,
                });
            }
            written |= group.attached(shape);
            if group.resolve {
                written |= shape.resolve_destination().bit();
            }
        }
        Ok(())
    }
}

/// A pass table or plan the frame cannot run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum GraphError {
    DuplicateNode(&'static str),
    /// A node with neither a colour nor a depth attachment has nothing to
    /// open a render pass over.
    NoAttachment(&'static str),
    /// A pass samples a resource it also draws into.
    Feedback(&'static str),
    /// A pass loads or samples a resource nothing earlier in the frame wrote.
    ReadBeforeWrite {
        pass: &'static str,
        resource: Resource,
    },
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateNode(label) => write!(f, "'{label}' is declared twice"),
            Self::NoAttachment(label) => write!(f, "'{label}' attaches no target"),
            Self::Feedback(label) => write!(f, "'{label}' samples a target it draws into"),
            Self::ReadBeforeWrite { pass, resource } => {
                write!(f, "'{pass}' reads {resource:?} before the frame writes it")
            }
        }
    }
}

/// The declared nodes, sorted into frame order once.
pub(crate) struct FrameGraph<N> {
    nodes: Vec<PassNode<N>>,
}

impl<N: Copy + PartialEq> FrameGraph<N> {
    /// Validate the declarations and fix the frame order: by phase, and by
    /// declaration order within one.
    pub(crate) fn new(mut nodes: Vec<PassNode<N>>) -> Result<Self, GraphError> {
        for (i, node) in nodes.iter().enumerate() {
            if nodes[..i].iter().any(|earlier| earlier.id == node.id) {
                return Err(GraphError::DuplicateNode(node.label));
            }
            if node.color.is_none() && node.depth.is_none() {
                return Err(GraphError::NoAttachment(node.label));
            }
            if node.samples.iter().any(|&s| node.attaches(s)) {
                return Err(GraphError::Feedback(node.label));
            }
        }
        // Stable: equal phases keep declaration order.
        nodes.sort_by_key(|node| node.phase);
        Ok(Self { nodes })
    }

    /// Every node in frame order.
    #[cfg(test)]
    pub(crate) fn order(&self) -> impl Iterator<Item = N> + '_ {
        self.nodes.iter().map(|node| node.id)
    }

    /// This frame's render passes over the nodes `active` accepts.
    pub(crate) fn plan(
        &self,
        shape: FrameShape,
        mut active: impl FnMut(N) -> bool,
        out: &mut FramePlan<N>,
    ) {
        out.groups.clear();
        out.nodes.clear();
        for node in &self.nodes {
            if !active(node.id) {
                continue;
            }
            let sampled = node
                .samples
                .iter()
                .fold(0, |bits, &s| bits | shape.sampled(s).bit());
            let index = out.nodes.len();
            out.nodes.push((node.id, node.label));
            let joins = |group: &PassGroup| {
                group.color.map(|c| c.target) == node.color.map(|c| c.target)
                    && group.depth.map(|d| d.target) == node.depth.map(|d| d.target)
                    && node.color.is_none_or(|c| c.load == LoadOp::Load)
                    && node.depth.is_none_or(|d| d.load == LoadOp::Load)
            };
            match out.groups.last_mut().filter(|group| joins(group)) {
                Some(group) => {
                    group.nodes.end = index + 1;
                    group.sampled |= sampled;
                }
                None => out.groups.push(PassGroup {
                    label: node.label,
                    color: node.color.map(|c| AttachmentOps {
                        target: c.target,
                        clear: c.load == LoadOp::Clear,
                        store: true,
                    }),
                    depth: node.depth.map(|d| AttachmentOps {
                        target: d.target,
                        clear: d.load == LoadOp::Clear,
                        store: true,
                    }),
                    resolve: false,
                    nodes: index..index + 1,
                    sampled,
                }),
            }
        }
        if shape.msaa {
            if let Some(last_world) = out
                .groups
                .iter_mut()
                .rev()
                .find(|g| g.color.is_some_and(|c| c.target == ColorTarget::World))
            {
                last_world.resolve = true;
            }
        }
        for i in 0..out.groups.len() {
            let later = &out.groups[i + 1..];
            let group = &out.groups[i];
            let color_store = group
                .color
                .map(|c| stored(shape.color(c.target), later, shape));
            let depth_store = group
                .depth
                .map(|d| stored(shape.depth(d.target), later, shape));
            let group = &mut out.groups[i];
            if let (Some(color), Some(store)) = (group.color.as_mut(), color_store) {
                color.store = store;
            }
            if let (Some(depth), Some(store)) = (group.depth.as_mut(), depth_store) {
                depth.store = store;
            }
        }
    }
}

/// Whether a pass must keep `resource` when it ends: it is read after the
/// frame, or a later pass reads it (loads or samples) before any pass clears
/// it.
fn stored(resource: Resource, later: &[PassGroup], shape: FrameShape) -> bool {
    if shape.read_after(resource) {
        return true;
    }
    for group in later {
        if group.sampled & resource.bit() != 0 {
            return true;
        }
        if let Some((_, cleared)) = group.attachments(shape).find(|&(r, _)| r == resource) {
            return !cleared;
        }
    }
    false
}

#[cfg(test)]
mod tests;
