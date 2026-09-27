use std::fmt;
use std::ops::Range;

use super::post_process::SceneRoute;

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Phase {
    Opaque,
    GroundDecal,
    Sky,
    Solid,
    Translucent,
    BreakDecal,
    Cutout,
    Fluid,
    /// Pack volumetrics. They SAMPLE the frame depth, so they follow all
    /// depth-writing world geometry — and the fluid, whose surface writes no
    /// depth, so only paint order keeps a cloud in front of a lake.
    Environment,
    Emitter,
    Highlight,
    Hand,
    PostProcess,
    Screen,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum ColorTarget {
    World,
    EnvColor,
    Swapchain,
    MarksEye,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum DepthTarget {
    Depth,
    EnvDepth,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Sampled {
    World,
    EnvColor,
    Depth,
    EnvDepth,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum LoadOp {
    Load,
    Clear,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct Attachment<T> {
    pub(crate) target: T,
    pub(crate) load: LoadOp,
}

pub(crate) struct PassNode<N> {
    pub(crate) id: N,
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

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct FrameShape {
    pub(crate) route: SceneRoute,
    pub(crate) msaa: bool,
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

    fn resolve_destination(self) -> Resource {
        match self.route {
            SceneRoute::ResolveToSwapchain => Resource::Swapchain,
            SceneRoute::Direct | SceneRoute::PostProcess => Resource::SceneColor,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct AttachmentOps<T> {
    pub(crate) target: T,
    pub(crate) clear: bool,
    pub(crate) store: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PassGroup {
    pub(crate) label: &'static str,
    pub(crate) color: Option<AttachmentOps<ColorTarget>>,
    pub(crate) depth: Option<AttachmentOps<DepthTarget>>,
    pub(crate) resolve: bool,
    pub(crate) nodes: Range<usize>,
    sampled: u8,
}

impl PassGroup {
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

pub(crate) struct FramePlan<N> {
    pub(crate) groups: Vec<PassGroup>,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum GraphError {
    DuplicateNode(&'static str),
    NoAttachment(&'static str),
    Feedback(&'static str),
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

pub(crate) struct FrameGraph<N> {
    nodes: Vec<PassNode<N>>,
}

impl<N: Copy + PartialEq> FrameGraph<N> {
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
        nodes.sort_by_key(|node| node.phase);
        Ok(Self { nodes })
    }

    #[cfg(test)]
    pub(crate) fn order(&self) -> impl Iterator<Item = N> + '_ {
        self.nodes.iter().map(|node| node.id)
    }

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
