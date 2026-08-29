//! What a placed oven SHOWS: the item it is cooking, sitting in the middle of
//! its chamber, and the finished bake waiting in the mouth's threshold, a
//! step forward.
//!
//! This is the draw-set surface (`set_block_draw`): prims the mod computes
//! each tick, retained engine-side, costing no re-mesh. Both contents are
//! drawn as their own ITEMS straight from the machine's slots, so what you
//! see in the oven and what you take out of the panel are the same art and
//! cannot drift apart when the art changes.
//!
//! A draw set does not survive a section unload, a reload, or a machine that
//! stopped ticking mid-state — so nothing here is memoised. Every tick the
//! oven submits what its slots hold right now; the engine drops an unchanged
//! submission before it costs anything.

use mod_sdk::*;

/// The engine fits this model to its footprint with the default `fill` mode:
/// the posed bounds (x −16..17.6, y −16..28, z 0..32 authored px) are scaled
/// uniformly until the widest axis spans the two-cell footprint width —
/// 16.8 authored px per prim-space block — then X/Z centred and rested on
/// the floor. One authored pixel of THIS model is therefore `MODEL_PX`
/// blocks wide, not the trustee 1/16. Every seat below is authored in model
/// pixels (the numbers a person reads off the `.bbmodel` in Blockbench) and
/// mapped through exactly that fit; the tests read the shipped model and
/// fail if the fit or the seats drift.
const MODEL_PX: f32 = 1.0 / 16.8;
/// The posed bounds' minimum corner; `p − BOUNDS_MIN` is an authored point's
/// model-space offset, in px.
const BOUNDS_MIN: [f32; 3] = [-16.0, -16.0, 0.0];
/// The Z centring slack the fit leaves (the model's depth is a hair shorter
/// than its two-cell footprint; X exactly fills, so its slack is zero).
const LO_Z: f32 = 0.047_619;

/// An authored model-space point, in prim space (1.0 = one footprint cell).
fn seat(at_px: [f32; 3]) -> [f32; 3] {
    [
        (at_px[0] - BOUNDS_MIN[0]) * MODEL_PX,
        (at_px[1] - BOUNDS_MIN[1]) * MODEL_PX,
        LO_Z + at_px[2] * MODEL_PX,
    ]
}

/// Where the bake sits: the middle of the chamber floor — the oven deck's
/// stone (authored y 0), centred left-right, at the depth centre between the
/// mouth and the back wall (authored px (0, 0, 11)).
const COOK_SEAT_PX: [f32; 3] = [0.0, 0.0, 11.0];
/// The full-size cooking item: 8 model px.
const COOK_SCALE_PX: f32 = 8.0;

/// Where the finished bake rests: the mouth's threshold — centred, a step
/// forward of the cooking seat, sitting in the recess the mouth trims frame
/// (authored px (0, 0, 3.25)) — like a loaf slid out to wait for pickup,
/// entirely on the deck.
const DONE_SEAT_PX: [f32; 3] = [0.0, 0.0, 3.25];
/// The finished item: 6 model px, so it fits the gap between the deck's
/// front edge and the cooking seat without overhanging either.
const DONE_SCALE_PX: f32 = 6.0;

/// A sprite item is a VERTICAL slab; this lays one flat on the stone.
const LIE_FLAT: f32 = std::f32::consts::FRAC_PI_2;
/// Clearance between a seated item and the stone under it, so it never
/// z-fights the surface it rests on.
const HAIR: f32 = 0.08 * MODEL_PX;
/// A flat sprite item's half-thickness is half of its one-texel extrusion.
const HALF_THICK: f32 = 1.0 / 32.0;

/// The oven's draw set for the tick: what its slots show.
///
/// The input sits in the chamber's middle whether or not the fire is lit —
/// it is in the oven either way — and rises a little with its cook progress,
/// settling back as the progress regresses. The output waits a step forward
/// in the mouth's threshold, reading as done and ready to take.
pub fn contents(
    input: Option<&ItemStackData>,
    output: Option<&ItemStackData>,
    progress: f32,
) -> Vec<DrawPrim> {
    let mut out = Vec::new();
    if let Some(stack) = input {
        // Rising with the bake, not popping from nothing: a fresh input
        // draws at 85% and reaches full size as it finishes.
        let rise = 0.85 + 0.15 * progress.clamp(0.0, 1.0);
        out.push(item(stack, COOK_SEAT_PX, rise * COOK_SCALE_PX * MODEL_PX));
    }
    if let Some(stack) = output {
        out.push(item(stack, DONE_SEAT_PX, DONE_SCALE_PX * MODEL_PX));
    }
    out
}

fn item(stack: &ItemStackData, seat_px: [f32; 3], scale: f32) -> DrawPrim {
    let on = seat(seat_px);
    DrawPrim::Item {
        at: [on[0], on[1] + scale * HALF_THICK + HAIR, on[2]],
        scale,
        yaw: 0.0,
        pitch: LIE_FLAT,
        item: stack.item.clone(),
        tint: [255, 255, 255],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODEL: &str = include_str!("../pack/models/kitchen_oven.bbmodel");

    fn stack(item: &str) -> ItemStackData {
        ItemStackData {
            item: item.into(),
            count: 1,
            data: Vec::new(),
        }
    }

    fn elements() -> Vec<json::Value> {
        let m = json::Value::parse(MODEL).expect("the shipped oven model parses");
        m.get("elements")
            .and_then(json::Value::as_array)
            .map(<[_]>::to_vec)
            .expect("the shipped oven model has elements")
    }

    fn box_of(e: &json::Value) -> ([f32; 3], [f32; 3]) {
        let take = |k: &str| {
            let a = e
                .get(k)
                .and_then(json::Value::as_array)
                .expect("cube from/to is a triple")
                .to_vec();
            std::array::from_fn(|i| a[i].as_f64().unwrap_or(0.0) as f32)
        };
        (take("from"), take("to"))
    }

    /// Bounds of ONE cube posed by its static tilt, as the engine bakes them.
    /// Only a single-axis Y tilt is shipped; a different rotation fails here
    /// loudly rather than posing incorrectly — extend the math when the art
    /// grows a second axis.
    fn posed_bounds(e: &json::Value) -> ([f32; 3], [f32; 3]) {
        let (from, to) = box_of(e);
        let Some(rot) = e.get("rotation") else {
            return (from, to);
        };
        let angles: [f32; 3] = {
            let a = rot
                .as_array()
                .expect("rotation is an angle triple")
                .to_vec();
            std::array::from_fn(|i| a[i].as_f64().unwrap_or(0.0) as f32)
        };
        assert!(
            angles[0].abs() < 1e-6 && angles[2].abs() < 1e-6,
            "only an authored Y tilt is supported by this test: {angles:?}"
        );
        let origin: [f32; 3] = {
            let a = e
                .get("origin")
                .and_then(json::Value::as_array)
                .expect("a rotated cube carries its pivot")
                .to_vec();
            std::array::from_fn(|i| a[i].as_f64().unwrap_or(0.0) as f32)
        };
        let a = angles[1].to_radians();
        let (s, c) = a.sin_cos();
        let mut mn = [f32::MAX; 3];
        let mut mx = [f32::MIN; 3];
        for &x in &[from[0], to[0]] {
            for &y in &[from[1], to[1]] {
                for &z in &[from[2], to[2]] {
                    let (dx, dz) = (x - origin[0], z - origin[2]);
                    let p = [origin[0] + dx * c + dz * s, y, origin[2] - dx * s + dz * c];
                    for i in 0..3 {
                        mn[i] = mn[i].min(p[i]);
                        mx[i] = mx[i].max(p[i]);
                    }
                }
            }
        }
        (mn, mx)
    }

    /// The engine's fill fit, re-read from the shipped model: posed bounds →
    /// uniform scale until the widest axis fills the footprint, X/Z centred,
    /// floor-rested.
    struct Fit {
        scale: f32,
        mn: [f32; 3],
        lo: [f32; 3],
    }

    impl Fit {
        fn read() -> Fit {
            let mut mn = [f32::MAX; 3];
            let mut mx = [f32::MIN; 3];
            for e in elements().iter() {
                let (a, b) = posed_bounds(e);
                for i in 0..3 {
                    mn[i] = mn[i].min(a[i]);
                    mx[i] = mx[i].max(b[i]);
                }
            }
            assert!(mn[0].is_finite(), "fixture: the model has geometry");
            let cells = [2.0, 3.0, 2.0];
            let per_unit = (0..3)
                .map(|a| (mx[a] - mn[a]) / cells[a])
                .fold(0.0f32, f32::max);
            let scale = 1.0 / per_unit;
            Fit {
                scale,
                mn,
                lo: [
                    (cells[0] - (mx[0] - mn[0]) * scale) * 0.5,
                    0.0,
                    (cells[2] - (mx[2] - mn[2]) * scale) * 0.5,
                ],
            }
        }

        fn at(&self, px: [f32; 3]) -> [f32; 3] {
            std::array::from_fn(|a| self.lo[a] + (px[a] - self.mn[a]) * self.scale)
        }
    }

    /// THE SEATS ARE WATERMARKS ON THE MODEL'S FACE: the fit, re-derived from
    /// the shipped `.bbmodel`, must put both items exactly where the
    /// constants claim. The engine fits the art and the tests fit the art, so
    /// a redraw that moves the chamber moves the seats with it — and one that
    /// changes the fit without moving the constants fails loud instead of
    /// baking a loaf in the chimney.
    #[test]
    fn the_seats_map_through_the_fit_the_engine_applies() {
        let fit = Fit::read();
        for (name, at_px) in [("cook", COOK_SEAT_PX), ("done", DONE_SEAT_PX)] {
            let want = seat(at_px);
            let got = fit.at(at_px);
            for a in 0..3 {
                assert!(
                    (got[a] - want[a]).abs() < 1e-5,
                    "the {name} seat {want:?} is not the model's fit {got:?}"
                );
            }
        }
        // The oven floor is the deck's top plane, and everything the oven
        // shows rests on it.
        let floor = fit.at([0.0, 0.0, 0.0])[1];
        assert!((fit.at([0.0, 0.0, 11.0])[1] - floor).abs() < 1e-5);
        assert!((fit.at(DONE_SEAT_PX)[1] - floor).abs() < 1e-5);
    }

    /// The bake must sit INSIDE the chamber the model builds — between the
    /// side walls' inner faces and behind the mouth plane, flat on the deck,
    /// clear of the mouth trim — and the finished bake on the deck's front
    /// sill: outboard of the trim, still on the deck, in front of the wall
    /// rather than inside the chamber.
    #[test]
    fn the_seats_land_inside_the_chamber_the_model_builds() {
        let fit = Fit::read();

        // The chamber features, straight off the model cubes: the walls at
        // wall height standing on the deck (three of them), the mouth trims
        // flanking the opening below the lintel, and the deck itself.
        let mut walls: Vec<([f32; 3], [f32; 3])> = Vec::new();
        let mut trims: Vec<([f32; 3], [f32; 3])> = Vec::new();
        let mut deck_top = f32::NAN;
        for e in elements().iter() {
            let (f, t) = box_of(e);
            let full_width = f[0] <= -15.5 && t[0] >= 15.5;
            if full_width && t[1].abs() < 1e-4 && f[1] < -1.0 {
                deck_top = t[1];
                continue;
            }
            if f[1].abs() > 1e-4 {
                continue;
            }
            if (t[1] - 9.0).abs() < 0.5 {
                walls.push((f, t));
            } else if (t[1] - 12.0).abs() < 0.5 && f[2] <= 2.5 {
                trims.push((f, t));
            }
        }
        assert_eq!(deck_top, 0.0, "fixture: the deck top is authored y 0");
        assert_eq!(walls.len(), 3, "fixture: two sides + the back wall");
        assert_eq!(trims.len(), 2, "fixture: two mouth trims");

        let mut left_inner = f32::NAN;
        let mut right_inner = f32::NAN;
        let mut mouth_z = f32::MAX;
        let mut back_z = f32::NAN;
        for (f, t) in walls.iter() {
            if f[0] < -10.0 {
                left_inner = t[0];
                mouth_z = mouth_z.min(f[2]);
            } else if f[0] > 4.0 {
                right_inner = f[0];
                mouth_z = mouth_z.min(f[2]);
            } else {
                back_z = f[2];
            }
        }
        let mut trim_left = f32::NAN;
        let mut trim_right = f32::NAN;
        for (f, t) in trims.iter() {
            if f[0] < 0.0 {
                trim_left = t[0];
            } else {
                trim_right = f[0];
            }
        }

        let left_inner = fit.at([left_inner, 0.0, 0.0])[0];
        let right_inner = fit.at([right_inner, 0.0, 0.0])[0];
        let mouth = fit.at([0.0, 0.0, mouth_z])[2];
        let back = fit.at([0.0, 0.0, back_z])[2];
        let mouth_left = fit.at([trim_left, 0.0, 0.0])[0];
        let mouth_right = fit.at([trim_right, 0.0, 0.0])[0];

        // --- The bake: inside the chamber, on the floor, clear of the trim.
        let cook = seat(COOK_SEAT_PX);
        let half = COOK_SCALE_PX * MODEL_PX * 0.5;
        assert!(
            cook[0] > left_inner && cook[0] < right_inner,
            "the bake sits between the chamber walls at {cook:?}"
        );
        assert!(
            cook[2] > mouth && cook[2] < back,
            "the bake sits behind the mouth plane, before the back wall at {cook:?}"
        );
        let floor = fit.at([0.0, 0.0, 0.0])[1];
        assert!(
            (cook[1] - floor).abs() < 1e-5,
            "the bake's feet are on the oven floor"
        );
        assert!(
            cook[0] - half > mouth_left && cook[0] + half < mouth_right,
            "the full-size bake fits through the mouth's width"
        );
        for (f, t) in trims.iter() {
            let (a, b) = (fit.at(*f), fit.at(*t));
            let clear = (cook[0] + half <= a[0].min(b[0]) || cook[0] - half >= a[0].max(b[0]))
                || (cook[2] + half <= a[2].min(b[2]) || cook[2] - half >= a[2].max(b[2]));
            assert!(clear, "the bake never touches a mouth trim");
        }

        // --- The finished bake: centred in the mouth's threshold, a step
        // forward of the baking seat, entirely on the deck.
        let done = seat(DONE_SEAT_PX);
        let half_done = DONE_SCALE_PX * MODEL_PX * 0.5;
        assert!(
            (done[1] - floor).abs() < 1e-5,
            "the finished bake's feet are on the oven floor too"
        );
        assert!(
            done[0] - half_done > mouth_left && done[0] + half_done < mouth_right,
            "it rests centred in the mouth's width, between the trims"
        );
        assert!(
            done[2] - half_done >= fit.at([0.0, 0.0, 0.0])[2],
            "its front edge stays on the deck, never overhanging it"
        );
        assert!(
            done[2] + half_done > mouth,
            "it sits in the mouth's threshold, its back inside the chamber"
        );
        assert!(
            (cook[2] - done[2]).abs() > half + half_done,
            "the contents' footprints never overlap: {cook:?} vs {done:?}"
        );
        for (f, t) in trims.iter() {
            let (a, b) = (fit.at(*f), fit.at(*t));
            let clear = (done[0] + half_done <= a[0].min(b[0])
                || done[0] - half_done >= a[0].max(b[0]))
                || (done[2] + half_done <= a[2].min(b[2]) || done[2] - half_done >= a[2].max(b[2]));
            assert!(clear, "the finished bake never touches a mouth trim");
        }
    }

    /// What the mod submits: empty slots clear the set, the input draws under
    /// its own name in the chamber, the output on the sill, and only the
    /// cooking item breathes with the progress.
    #[test]
    fn contents_follow_the_slots() {
        let raw = stack("kitchen:raw_mutton");
        let cooked = stack("kitchen:cooked_mutton");

        assert!(
            contents(None, None, 0.0).is_empty(),
            "an empty oven draws nothing"
        );

        let (fresh_item, fresh_scale, _fresh_at) = item_of(&contents(Some(&raw), None, 0.0), 0);
        assert_eq!(
            fresh_item, "kitchen:raw_mutton",
            "the art is the slot's item"
        );
        assert!(
            (fresh_scale - 0.85 * COOK_SCALE_PX * MODEL_PX).abs() < 1e-6,
            "a fresh input draws small and grows"
        );

        let finished = contents(Some(&raw), None, 1.0);
        let (_, full_scale, full_at) = item_of(&finished, 0);
        assert_eq!(finished.len(), 1);
        assert!(
            (full_scale - COOK_SCALE_PX * MODEL_PX).abs() < 1e-6,
            "full progress is the full size"
        );
        let on = seat(COOK_SEAT_PX);
        assert!(
            (full_at[1] - (on[1] + full_scale * HALF_THICK + HAIR)).abs() < 1e-6,
            "seated a half-thickness plus a hair off the stone"
        );
        assert!((full_at[0] - on[0]).abs() < 1e-6 && (full_at[2] - on[2]).abs() < 1e-6);

        let stalled = contents(Some(&raw), None, 0.3);
        let (_, stalled_scale, _) = item_of(&stalled, 0);
        assert!(
            fresh_scale < stalled_scale && stalled_scale < full_scale,
            "the rise tracks the progress, not a step function"
        );

        let both = contents(Some(&raw), Some(&cooked), 0.5);
        assert_eq!(both.len(), 2, "both contents draw at once");
        let (done_item, done_scale, done_at) = item_of(&both, 1);
        assert_eq!(done_item, "kitchen:cooked_mutton");
        assert_eq!(
            done_scale,
            DONE_SCALE_PX * MODEL_PX,
            "the output does not breathe"
        );
        let on = seat(DONE_SEAT_PX);
        assert!(
            (done_at[1] - (on[1] + done_scale * HALF_THICK + HAIR)).abs() < 1e-6,
            "the finished bake rests on the stone at the threshold"
        );
    }

    fn item_of(prims: &[DrawPrim], i: usize) -> (String, f32, [f32; 3]) {
        match prims.get(i) {
            Some(DrawPrim::Item {
                item,
                scale,
                at,
                yaw,
                pitch,
                tint,
            }) => {
                assert_eq!(*pitch, LIE_FLAT, "seated flat on the stone");
                assert_eq!(*yaw, 0.0);
                assert_eq!(*tint, [255, 255, 255], "no tint: the item's own colours");
                (item.clone(), *scale, *at)
            }
            other => panic!("expected an item prim at {i}, got {other:?}"),
        }
    }
}
