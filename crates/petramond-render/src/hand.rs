//! The first-person REST SEATS: where each held render kind sits in view
//! space before the rig's fist carries it (`first_person::FirstPersonRig::carry`)
//! — the placements approved item by item, kept exactly. Sprites and blocks
//! were tuned under a 70° hand camera; the rig draws under Blockbench's
//! first-person camera ([`model_hand_view_proj`]), so their seats widen by the
//! focal ratio to land on the same pixels. A claimed held pose composes
//! inside the seat, so the fist carries a posed item instead of fighting it.

use glam::{Mat4, Quat, Vec3};

use super::HeldItemView;
use petramond_world::item::ItemRenderKind;

/// The camera the sprite and block seats were tuned under.
const HAND_FOV_Y: f32 = 70.0 * std::f32::consts::PI / 180.0;
const HAND_DEPTH: f32 = 1.65;
const REST_NDC_X: f32 = 0.68;
const REST_NDC_Y: f32 = -0.83;

/// Where a bbmodel item sits relative to the camera (view units = blocks;
/// camera at origin looking down −Z): the vanilla right-hand anchor, the same
/// point Blockbench's first-person preview seats its `display_area` at (its
/// "monitor" reference: `(9.039, −8.318+24, 20.8)` pixels against a camera
/// at `(0, 24, 32.4)`). Held bbmodel items compose their authored pose about
/// this anchor so the in-game hold matches the Blockbench preview exactly.
const MODEL_HAND_ANCHOR: Vec3 = Vec3::new(9.039 / 16.0, -8.318 / 16.0, -11.6 / 16.0);

/// The framebuffer shape the NDC-anchored seats resolve at: the rig's arms
/// live in fixed view space, so the seats do too.
const RIG_SEAT_ASPECT: f32 = 16.0 / 9.0;

/// The vertical half-extent slope of the rig's camera — Blockbench's
/// first-person "monitor" window, ±0.93 at 1.2 units before the camera.
const RIG_CAMERA_SLOPE: f32 = 0.93 / 1.2;

/// Where `view`'s item RESTS in first person as a model → view transform
/// under [`model_hand_view_proj`], keeping the claimed held pose. `None` for
/// an empty hand.
pub(crate) fn rest_seat(view: &HeldItemView, off: bool) -> Option<Mat4> {
    let item = view.item?;
    // Widening x and y by the focal ratio puts every point of a seat tuned
    // under the 70° camera on the same pixel under the rig's camera.
    let focal = RIG_CAMERA_SLOPE / (HAND_FOV_Y * 0.5).tan();
    let widen = Mat4::from_scale(Vec3::new(focal, focal, 1.0));
    Some(match item.render_kind() {
        ItemRenderKind::BlockCube(_) => {
            let seat = legacy_anchor(view) * block_base();
            widen * if off { mirror_x(seat) } else { seat }
        }
        ItemRenderKind::Sprite(_) => {
            let seat = sprite_seat(view);
            widen * if off { reflect_x(seat) } else { seat }
        }
        ItemRenderKind::Model(kind) => model_seat(view, kind, off),
    })
}

/// Compose the rig's `carry` over an item's rest `seat` under the viewmodel's
/// one composition rule: **a swing may bring a held item toward the eye, never
/// push it further away than the distance its seat sits at.**
///
/// The carry welds the item to the fist, which is right — but the fist is a rig
/// locator and the seats are view-space compositions, so the two are ~3 blocks
/// apart. A weld across that lever turns the arm's rotation into a large
/// translation, and an item seated close to the eye rides it far away: the
/// bbmodel seat is the vanilla hand anchor (0.7 blocks, pulled nearer still by a
/// model's authored display translation), so a punch or the swim stance — which
/// drop the hold clip that the seat was composed against — sent a held bbmodel
/// to ~9x its distance, i.e. a ninth of its size (2026-09-23: the furniture
/// workbench shrinking to a speck on a punch or on entering water).
///
/// Clamping the DISTANCE only, along the view ray, keeps every approved swing:
/// the legacy block and sprite seats swing toward the eye, never away, so their
/// composition is unchanged to the float. Nothing here knows a render kind.
pub(crate) fn carried(seat: Mat4, carry: Mat4) -> Mat4 {
    let at = carry * seat;
    let seat_depth = -seat.w_axis.z;
    let depth = -at.w_axis.z;
    if seat_depth <= 0.0 || depth <= seat_depth {
        return at;
    }
    // Pulling the origin back along its own ray keeps the item's ANGULAR place
    // (and so the whole lateral sweep of the swing); only the recession goes.
    let pulled = at.w_axis.truncate() * (seat_depth / depth);
    Mat4::from_translation(pulled - at.w_axis.truncate()) * at
}

/// A held block: a corner toward the camera (three-quarter view).
fn block_base() -> Mat4 {
    Mat4::from_scale_rotation_translation(
        Vec3::splat(0.55),
        Quat::from_rotation_y(0.55) * Quat::from_rotation_x(-0.20),
        Vec3::ZERO,
    )
}

/// Mirror a rigid placement across the view-space YZ plane by CONJUGATION:
/// `S · M · S` with `S = diag(-1, 1, 1)`. Preserves the determinant, so
/// geometry keeps its winding and its texturing; only the POSE mirrors to the
/// screen's left. Used ONLY for the held block cube — a cube's three-quarter
/// view survives it because the geometry is symmetric. Anything with an
/// asymmetric pose or silhouette (tool sprites, bbmodels) must use
/// [`reflect_x`] instead: conjugation shows a DIFFERENT orientation (negated
/// rotations of unmirrored geometry — the 2026-08-21 hoe-facing-the-player /
/// invisible-pottery-table bug), not the mirror image.
fn mirror_x(m: Mat4) -> Mat4 {
    let s = Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0));
    s * m * s
}

/// TRUE reflection across the view-space YZ plane: `S · M`. The left-hand
/// image is EXACTLY the right-hand image flipped horizontally — which is what
/// Blockbench's `firstperson_lefthand` preview shows (the authored left pose
/// rendered in a mirrored view space), and therefore the target for every
/// authored pose. Flips triangle winding, so only double-sided (cull `None`)
/// paths may draw it — the item3d held pipeline is.
fn reflect_x(m: Mat4) -> Mat4 {
    Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0)) * m
}

/// The sprite seat: the extruded unit slab tilted by the held STACK's own
/// authored hold (`view.hold` — item data, never a display stand-in's): roll
/// (Z) first, in the sprite's own plane, lays a swung tool's long axis
/// diagonally; yaw (Y) then swings the slab past head-on to a steep,
/// near-side-on angle so the EXTRUDED THICKNESS reads; pitch (X) is a spare
/// tilt. `nudge` lifts and shifts it within the shared anchor.
fn sprite_seat(view: &HeldItemView) -> Mat4 {
    let pose = view.hold;
    let nudge = Vec3::new(0.10, 0.10, 0.0);
    legacy_anchor(view)
        * Mat4::from_translation(nudge)
        * Mat4::from_quat(
            Quat::from_rotation_y(pose.yaw)
                * Quat::from_rotation_x(pose.pitch)
                * Quat::from_rotation_z(pose.roll),
        )
}

/// The bbmodel seat: the authored `firstperson_righthand` display pose
/// (its `lefthand` twin for the off hand, [`DisplayTransform::left_hand`]
/// applied as Blockbench previews it) composed about the vanilla hand
/// anchor, with `ModelInstance::display_from_unit` rebasing the baked unit
/// geometry into display space. Editing the pose in Blockbench moves the
/// in-game hold; no code. The off hand mirrors the ANCHOR by conjugation and
/// leaves the geometry untouched: a whole-chain reflection would mirror each
/// model's own authored x-offset (the 2026-08-21 pottery-table drift).
///
/// [`DisplayTransform::left_hand`]: petramond_world::block_model::DisplayTransform::left_hand
fn model_seat(
    view: &HeldItemView,
    kind: petramond_world::block_model::BlockModelKind,
    off: bool,
) -> Mat4 {
    let display = petramond_world::block_model::display(kind);
    let pose = if off {
        display
            .firstperson_lefthand
            .as_ref()
            .unwrap_or(&display.firstperson_righthand)
            .left_hand()
    } else {
        display.firstperson_righthand
    };
    let model = pose.base_matrix() * petramond_world::block_model::instance(kind).display_from_unit;
    let anchor = seat_anchor(view, MODEL_HAND_ANCHOR);
    let anchor = if off { mirror_x(anchor) } else { anchor };
    anchor * model
}

/// The sprite and block anchor: the lower-right rest point under the seat
/// camera.
fn legacy_anchor(view: &HeldItemView) -> Mat4 {
    let t = (HAND_FOV_Y * 0.5).tan();
    let rest = Vec3::new(
        REST_NDC_X * RIG_SEAT_ASPECT * t * HAND_DEPTH,
        REST_NDC_Y * t * HAND_DEPTH,
        -HAND_DEPTH,
    );
    seat_anchor(view, rest)
}

/// Seat an item at `rest` (view units) with the claimed held pose composed
/// INSIDE the seat, its 1/16-block translation landing in view space (which
/// is blocks). Every off-hand path mirrors a chain containing this one, and
/// conjugating a display transform by the x-flip is exactly
/// `DisplayTransform::left_hand`, so one authored pose reads correctly from
/// either fist with no per-hand rule here.
fn seat_anchor(view: &HeldItemView, rest: Vec3) -> Mat4 {
    Mat4::from_translation(rest) * view.pose.first_person.base_matrix()
}

/// The camera under which Blockbench's first-person preview is SEEN: its
/// "monitor" reference masks the (wider, `getOptimalFocalLength`) render down
/// to a screen window of black planes — inner edges ±1.65 × ±0.93 at 1.2
/// units before the camera (display_references `monitor`) — and that window
/// is the vanilla screen. Mapping our framebuffer to that window means a
/// FIXED vertical half-extent slope of `0.93 / 1.2` (≈75.6° vertical),
/// horizontal spanning with aspect, independent of Blockbench's
/// render-canvas fov (window and scene geometry cancel it). Verified against
/// a Blockbench screenshot to <1% (bed features, window-normalized).
pub(crate) fn model_hand_view_proj(aspect: f32) -> Mat4 {
    let proj = Mat4::perspective_rh(
        2.0 * RIG_CAMERA_SLOPE.atan(),
        aspect.max(0.0001),
        0.01,
        10.0,
    );
    let view = Mat4::look_at_rh(Vec3::ZERO, Vec3::new(0.0, 0.0, -1.0), Vec3::Y);
    proj * view
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The seat sits 2 blocks down the view ray; a carry that would push it to
    /// 6 pulls back to 2 along the SAME ray — the swing keeps its whole
    /// angular sweep and loses only the recession (which reads as shrinking).
    #[test]
    fn a_carry_may_pull_a_held_item_closer_but_never_push_it_away() {
        let seat = Mat4::from_translation(Vec3::new(0.5, -0.3, -2.0));
        let away = carried(seat, Mat4::from_translation(Vec3::new(1.0, 0.0, -4.0)));
        let at = away.w_axis.truncate();
        assert!(
            (-at.z - 2.0).abs() < 1e-4,
            "a receding carry is clamped to the seat's distance, got {at:?}"
        );
        // Same direction as the unclamped weld: only the distance changed.
        let unclamped = Vec3::new(1.5, -0.3, -6.0);
        assert!(
            at.normalize().dot(unclamped.normalize()) > 0.9999,
            "the clamp must keep the item on its ray, got {at:?}"
        );
        // Toward the eye is the approved swing and passes through untouched.
        let closer = carried(seat, Mat4::from_translation(Vec3::new(0.0, 0.0, 1.2)));
        assert!((-closer.w_axis.z - 0.8).abs() < 1e-4);
    }
}
