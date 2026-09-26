use super::*;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum N {
    First,
    Stamp,
    Late,
    Sampler,
    Hand,
    Out,
    Chrome,
}

const PLAIN: FrameShape = FrameShape {
    route: SceneRoute::PostProcess,
    msaa: false,
};
const MSAA: FrameShape = FrameShape {
    route: SceneRoute::PostProcess,
    msaa: true,
};

fn world(id: N, label: &'static str, phase: Phase) -> PassNode<N> {
    PassNode::new(id, label, phase)
        .color(ColorTarget::World, LoadOp::Load)
        .depth(DepthTarget::Depth, LoadOp::Load)
}

/// A miniature of the real frame: a clearing opaque pass, two more world
/// nodes, a depth-sampling composite, a depth-clearing hand, the post pass
/// and one screen node. Declared out of order on purpose.
fn graph() -> FrameGraph<N> {
    FrameGraph::new(vec![
        PassNode::new(N::Chrome, "chrome", Phase::Screen)
            .color(ColorTarget::Swapchain, LoadOp::Load),
        world(N::Late, "late", Phase::Emitter),
        PassNode::new(N::First, "first", Phase::Opaque)
            .color(ColorTarget::World, LoadOp::Clear)
            .depth(DepthTarget::Depth, LoadOp::Clear),
        world(N::Stamp, "stamp", Phase::GroundDecal),
        PassNode::new(N::Sampler, "sampler", Phase::Environment)
            .color(ColorTarget::World, LoadOp::Load)
            .sampling(&[Sampled::Depth]),
        PassNode::new(N::Hand, "hand", Phase::Hand)
            .color(ColorTarget::World, LoadOp::Load)
            .depth(DepthTarget::Depth, LoadOp::Clear),
        PassNode::new(N::Out, "out", Phase::PostProcess)
            .color(ColorTarget::Swapchain, LoadOp::Clear)
            .sampling(&[Sampled::World]),
    ])
    .expect("valid test graph")
}

fn plan(shape: FrameShape, active: &[N]) -> FramePlan<N> {
    let mut out = FramePlan::default();
    graph().plan(shape, |n| active.contains(&n), &mut out);
    out
}

fn labels(plan: &FramePlan<N>) -> Vec<Vec<&'static str>> {
    plan.groups
        .iter()
        .map(|g| plan.nodes[g.nodes.clone()].iter().map(|&(_, l)| l).collect())
        .collect()
}

const ALL: [N; 7] = [
    N::First,
    N::Stamp,
    N::Late,
    N::Sampler,
    N::Hand,
    N::Out,
    N::Chrome,
];

#[test]
fn nodes_are_ordered_by_phase_not_declaration() {
    assert_eq!(
        graph().order().collect::<Vec<_>>(),
        [N::First, N::Stamp, N::Sampler, N::Late, N::Hand, N::Out, N::Chrome]
    );
}

#[test]
fn equal_phases_keep_declaration_order() {
    let g = FrameGraph::new(vec![
        world(N::Late, "b", Phase::Solid),
        world(N::Stamp, "a", Phase::Solid),
    ])
    .unwrap();
    assert_eq!(g.order().collect::<Vec<_>>(), [N::Late, N::Stamp]);
}

#[test]
fn nodes_sharing_attachments_merge_into_one_pass() {
    let p = plan(PLAIN, &[N::First, N::Stamp, N::Late, N::Out, N::Chrome]);
    // The post pass clears the swapchain, the chrome loads it: one pass.
    assert_eq!(
        labels(&p),
        [vec!["first", "stamp", "late"], vec!["out", "chrome"]]
    );
    assert_eq!(p.groups[0].label, "first");
    assert_eq!(p.validate(PLAIN), Ok(()));
}

#[test]
fn a_sampler_splits_the_world_and_keeps_depth_alive_across_it() {
    let p = plan(PLAIN, &ALL);
    assert_eq!(
        labels(&p),
        [
            vec!["first", "stamp"],
            vec!["sampler"],
            vec!["late"],
            vec!["hand"],
            vec!["out", "chrome"],
        ]
    );
    let first = &p.groups[0];
    assert!(first.color.unwrap().clear && first.depth.unwrap().clear);
    assert!(first.depth.unwrap().store, "the sampler reads depth later");
    let late = &p.groups[2];
    assert!(!late.color.unwrap().clear && !late.depth.unwrap().clear);
    assert!(
        !late.depth.unwrap().store,
        "the hand clears depth before anything reads it again"
    );
    assert_eq!(p.validate(PLAIN), Ok(()));
}

#[test]
fn a_clearing_node_always_opens_its_own_pass() {
    let p = plan(PLAIN, &[N::First, N::Stamp, N::Hand, N::Out]);
    assert_eq!(
        labels(&p),
        [vec!["first", "stamp"], vec!["hand"], vec!["out"]]
    );
    assert!(p.groups[1].depth.unwrap().clear);
    assert!(!p.groups[1].depth.unwrap().store, "nothing reads the hand's depth");
    assert!(p.groups[1].color.unwrap().store, "the post pass samples the world");
}

#[test]
fn msaa_resolves_on_the_last_world_pass_and_discards_the_samples() {
    let p = plan(MSAA, &ALL);
    let resolving: Vec<_> = p.groups.iter().filter(|g| g.resolve).map(|g| g.label).collect();
    assert_eq!(resolving, ["hand"]);
    let hand = &p.groups[3];
    assert!(!hand.color.unwrap().store, "the resolve carries the image on");
    assert!(p.groups[0].color.unwrap().store, "later world passes load it");
    assert_eq!(p.validate(MSAA), Ok(()));
}

#[test]
fn without_the_hand_the_resolve_moves_to_the_last_world_pass() {
    let p = plan(MSAA, &[N::First, N::Stamp, N::Late, N::Out]);
    assert_eq!(labels(&p), [vec!["first", "stamp", "late"], vec!["out"]]);
    assert!(p.groups[0].resolve);
    assert!(!p.groups[0].color.unwrap().store);
    assert!(!p.groups[0].depth.unwrap().store);
    assert_eq!(p.validate(MSAA), Ok(()));
}

#[test]
fn a_world_drawn_straight_to_the_swapchain_is_always_stored() {
    let direct = FrameShape {
        route: SceneRoute::Direct,
        msaa: false,
    };
    let p = plan(direct, &[N::First, N::Stamp, N::Chrome]);
    assert!(p.groups[0].color.unwrap().store);
    assert!(!p.groups[0].depth.unwrap().store);
    assert!(p.groups.iter().all(|g| !g.resolve));
    assert_eq!(p.validate(direct), Ok(()));
}

#[test]
fn resolving_to_the_swapchain_feeds_the_screen_passes() {
    let shape = FrameShape {
        route: SceneRoute::ResolveToSwapchain,
        msaa: true,
    };
    let p = plan(shape, &[N::First, N::Chrome]);
    assert!(p.groups[0].resolve);
    assert_eq!(p.validate(shape), Ok(()));
    // Without the resolve nothing would have written the swapchain.
    let unresolved = FrameShape {
        route: SceneRoute::ResolveToSwapchain,
        msaa: false,
    };
    let p = plan(unresolved, &[N::First, N::Chrome]);
    assert_eq!(
        p.validate(unresolved),
        Err(GraphError::ReadBeforeWrite {
            pass: "chrome",
            resource: Resource::Swapchain,
        })
    );
}

#[test]
fn validation_catches_a_load_before_any_write() {
    let p = plan(PLAIN, &[N::Stamp, N::Out]);
    assert_eq!(
        p.validate(PLAIN),
        Err(GraphError::ReadBeforeWrite {
            pass: "stamp",
            resource: Resource::SceneColor,
        })
    );
}

#[test]
fn declarations_that_cannot_run_are_rejected() {
    assert_eq!(
        FrameGraph::new(vec![world(N::First, "a", Phase::Opaque), world(N::First, "b", Phase::Sky)])
            .err(),
        Some(GraphError::DuplicateNode("b"))
    );
    assert_eq!(
        FrameGraph::new(vec![PassNode::new(N::First, "bare", Phase::Opaque)]).err(),
        Some(GraphError::NoAttachment("bare"))
    );
    assert_eq!(
        FrameGraph::new(vec![world(N::First, "loop", Phase::Opaque).sampling(&[Sampled::Depth])])
            .err(),
        Some(GraphError::Feedback("loop"))
    );
}

#[test]
fn replanning_reuses_the_plan_without_leftovers() {
    let g = graph();
    let mut out = FramePlan::default();
    g.plan(PLAIN, |_| true, &mut out);
    let full = out.groups.len();
    g.plan(PLAIN, |n| n == N::First, &mut out);
    assert!(full > 1);
    assert_eq!(out.groups.len(), 1);
    assert_eq!(out.nodes.len(), 1);
}
