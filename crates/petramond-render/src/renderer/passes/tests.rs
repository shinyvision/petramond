//! The frame's pass table against the contracts it encodes: ordering rules
//! that used to be comments, the render passes a typical frame collapses
//! into, and a valid plan for every combination of active nodes.

use super::super::graph::{FrameShape, PassGroup};
use super::*;

fn order() -> Vec<Node> {
    frame_graph().expect("the pass table is valid").order().collect()
}

fn position(order: &[Node], node: Node) -> usize {
    order
        .iter()
        .position(|&n| n == node)
        .unwrap_or_else(|| panic!("{node:?} is not in the pass table"))
}

#[track_caller]
fn assert_before(first: Node, then: Node, why: &str) {
    let order = order();
    assert!(
        position(&order, first) < position(&order, then),
        "{first:?} must draw before {then:?}: {why}"
    );
}

#[test]
fn ordering_contracts_hold() {
    use Node::*;
    assert_before(Opaque, Sky, "the sky shades only pixels terrain left uncovered");
    assert_before(
        ContactShadow,
        Sky,
        "an orphaned contact stamp must be painted over by the sky",
    );
    assert_before(
        EntityShadow,
        Sky,
        "an orphaned blob shadow must be painted over by the sky",
    );
    assert_before(Sky, TerrainModels, "models draw over the sky-filled background");
    assert_before(
        TranslucentBlocks,
        BreakOverlay,
        "a crack on mined ice draws on top of the ice",
    );
    assert_before(
        ModelBlend,
        ModelBreak,
        "a crack on a model's glass draws on top of the glass",
    );
    assert_before(ModelBreak, BreakOverlay, "the model crack precedes the block crack");
    assert_before(
        BreakOverlay,
        Particles,
        "particles sit in front of the crack decal",
    );
    assert_before(
        BreakOverlay,
        Fluid,
        "water blends in front of a submerged crack",
    );
    assert_before(Particles, Fluid, "water blends over the particles behind it");
    assert_before(
        Fluid,
        EnvDownsample,
        "a fluid surface writes no depth; paint order keeps clouds in front of lakes",
    );
    assert_before(EnvDownsample, Environment, "the march reads the downsampled depth");
    assert_before(Environment, EnvComposite, "the composite lifts the march result");
    assert_before(
        EnvComposite,
        EmitterParticles,
        "flames and rain streak over the cloud deck",
    );
    assert_before(EmitterParticles, Outline, "highlights draw over the whole world");
    assert_before(Ghosts, Hand, "the hand draws over everything in the world");
    assert_before(Hand, Grade, "the grade reads the finished world");
    assert_before(Grade, Crosshair, "chrome draws ungraded over the final image");
    assert_before(Crosshair, Ui, "the UI covers the crosshair");
    assert_before(Ui, UiOverlay, "tooltips and the dragged stack are front-most");
}

fn plan(shape: FrameShape, active: impl Fn(Node) -> bool) -> FramePlan<Node> {
    let mut out = FramePlan::default();
    frame_graph()
        .expect("the pass table is valid")
        .plan(shape, active, &mut out);
    out
}

fn group_labels(plan: &FramePlan<Node>) -> Vec<&'static str> {
    plan.groups.iter().map(|g| g.label).collect()
}

fn group_of(plan: &FramePlan<Node>, node: Node) -> &PassGroup {
    plan.groups
        .iter()
        .find(|g| plan.nodes[g.nodes.clone()].iter().any(|&(n, _)| n == node))
        .unwrap_or_else(|| panic!("{node:?} is not planned"))
}

const GRADED: FrameShape = FrameShape {
    route: SceneRoute::PostProcess,
    msaa: false,
};

/// Every node active: the frame that used to open 26 render passes (the
/// MSAA resolve included).
#[test]
fn a_full_frame_collapses_into_seven_render_passes() {
    let p = plan(GRADED, |_| true);
    assert_eq!(
        group_labels(&p),
        [
            "opaque pass",
            "env depth downsample",
            "environment pass",
            "env composite pass",
            "emitter particle pass",
            "hand pass",
            "grade pass",
        ]
    );
    // Opaque through fluid is one pass; the volumetrics split it only
    // because they sample the depth it writes.
    let world = &p.groups[0];
    assert_eq!(world.nodes.len(), 15);
    assert!(world.depth.unwrap().store, "the volumetrics sample depth");
    assert_eq!(p.validate(GRADED), Ok(()));
}

#[test]
fn without_volumetrics_the_world_is_one_pass_up_to_the_hand() {
    let p = plan(GRADED, |n| {
        !matches!(
            n,
            Node::EnvDownsample | Node::Environment | Node::EnvComposite
        )
    });
    assert_eq!(group_labels(&p), ["opaque pass", "hand pass", "grade pass"]);
    let world = &p.groups[0];
    assert!(world.color.unwrap().store, "the hand loads the world colour");
    assert!(
        !world.depth.unwrap().store,
        "the hand clears depth, so the world's depth never leaves the GPU tile"
    );
    let hand = &p.groups[1];
    assert!(!hand.depth.unwrap().store, "nothing reads the hand's depth");
    assert!(hand.color.unwrap().store, "the grade samples the world");
    assert_eq!(p.validate(GRADED), Ok(()));
}

#[test]
fn msaa_resolves_on_the_hand_pass_and_discards_its_samples() {
    let shape = FrameShape {
        route: SceneRoute::PostProcess,
        msaa: true,
    };
    let p = plan(shape, |_| true);
    let hand = group_of(&p, Node::Hand);
    assert!(hand.resolve);
    assert!(!hand.color.unwrap().store);
    assert_eq!(p.groups.iter().filter(|g| g.resolve).count(), 1);
    assert_eq!(p.validate(shape), Ok(()));
}

#[test]
fn msaa_without_a_hand_resolves_on_the_last_world_pass() {
    let shape = FrameShape {
        route: SceneRoute::ResolveToSwapchain,
        msaa: true,
    };
    let p = plan(shape, |n| {
        matches!(n, Node::Opaque | Node::Sky | Node::Outline | Node::Crosshair)
    });
    assert_eq!(group_labels(&p), ["opaque pass", "crosshair pass"]);
    assert!(p.groups[0].resolve);
    assert!(!p.groups[0].color.unwrap().store);
    assert!(!p.groups[0].depth.unwrap().store);
    assert_eq!(p.validate(shape), Ok(()));
}

#[test]
fn screen_chrome_shares_the_post_process_pass() {
    let p = plan(GRADED, |n| {
        matches!(
            n,
            Node::Opaque | Node::Sky | Node::Grade | Node::Crosshair | Node::Ui | Node::UiOverlay
        )
    });
    assert_eq!(group_labels(&p), ["opaque pass", "grade pass"]);
    assert_eq!(p.groups[1].nodes.len(), 4);
    assert!(p.groups[1].color.unwrap().clear);
    assert!(p.groups[1].color.unwrap().store, "the swapchain is presented");
}

/// Every route, with and without MSAA, over pseudo-random subsets of the
/// optional nodes: the plan must always be runnable. The always-on nodes
/// are on, and the grade runs exactly when the route post-processes.
#[test]
fn every_activity_combination_plans_validly() {
    let shapes = [
        FrameShape {
            route: SceneRoute::Direct,
            msaa: false,
        },
        FrameShape {
            route: SceneRoute::ResolveToSwapchain,
            msaa: true,
        },
        FrameShape {
            route: SceneRoute::PostProcess,
            msaa: false,
        },
        FrameShape {
            route: SceneRoute::PostProcess,
            msaa: true,
        },
    ];
    let order = order();
    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    for shape in shapes {
        for _ in 0..512 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let mask = seed;
            let bit = |n: Node| mask & (1 << position(&order, n)) != 0;
            let active = |n: Node| match n {
                Node::Opaque | Node::Sky => true,
                Node::Grade => shape.route == SceneRoute::PostProcess,
                // The volumetric chain shares one gate.
                Node::Environment | Node::EnvComposite => bit(Node::EnvDownsample),
                _ => bit(n),
            };
            let p = plan(shape, active);
            assert_eq!(p.validate(shape), Ok(()), "{shape:?} mask {mask:#x}");
            assert!(
                p.groups.iter().filter(|g| g.resolve).count() == usize::from(shape.msaa),
                "{shape:?} mask {mask:#x}: exactly one resolve under MSAA"
            );
        }
    }
}
