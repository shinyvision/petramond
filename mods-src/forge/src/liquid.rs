//! The molten metal, SIMULATED.
//!
//! Everything the furnace shows that has a level, a front, or a surface lives
//! here as numbers, and is drawn with [`set_block_draw`] — boxes the mod
//! computes each tick, not pictures the pack authored in advance. The parts
//! mask stages what the machine IS (whether the fire is in); this is what the
//! metal is DOING.
//!
//! Coordinates are the furnace's own FOOTPRINT space — 16 px = 1.0, origin at
//! the footprint corner, turned by the placed facing. The SHIPPED model is
//! authored in exactly that space, so every constant below is a raw read off a
//! cube of it, and the mod never learns where it was placed or which way it
//! faces.

use mod_sdk::*;

/// The authored model's own pixels, as block fractions (16 px = 1 block).
const PX: f32 = 1.0 / 16.0;

/// THE LAUNDER IS AN L, because a furnace taps through a hole in its SIDE and
/// the vessel is out in front of it — and it is ONE RUN THAT TURNS, on one
/// floor plane at y19. Leg A comes out of the tap hole between walls at x28 and
/// x30; leg B carries on along the shelf between walls at z9 and z11 to a lip
/// over the crucible. The metal STANDS ON that floor, so the `_Y`s are its top
/// and the channel fills upward from it.
const SPOUT_A_X: (f32, f32) = (28.0 * PX, 30.0 * PX);
const SPOUT_A_Y: f32 = 19.0 * PX;
const SPOUT_A_Z: (f32, f32) = (11.0 * PX, 19.0 * PX);
const SPOUT_X: (f32, f32) = (23.0 * PX, 30.0 * PX);
const SPOUT_Y: f32 = 19.0 * PX;
const SPOUT_Z: (f32, f32) = (9.0 * PX, 11.0 * PX);
/// How deep the metal lies in a channel running full — the walls stand 2 px
/// over the floor, and metal to the brim would hide them.
const CHANNEL_DEPTH: f32 = 1.5 * PX;
/// Where the metal leaves the lip and starts to fall: the gap between leg B's
/// walls, just clear of the lip's end face.
const FALL_X: (f32, f32) = (21.5 * PX, 23.0 * PX);
const FALL_Z: (f32, f32) = (9.0 * PX, 11.0 * PX);
const BASIN_X: (f32, f32) = (19.25 * PX, 26.75 * PX);
const BASIN_Z: (f32, f32) = (5.25 * PX, 12.75 * PX);
const BASIN_FLOOR: f32 = 12.0 * PX;
const BASIN_DEPTH: f32 = 3.0 * PX;
/// Where a mould sits in the crucible, and how big it draws. The mould and the
/// cast are drawn as their own ITEMS, so what you see in the crucible and what
/// you see in your hand cannot drift apart when the art changes.
///
/// Both SEAT INSIDE the crucible, under the rim. A sprite item is a one-texel
/// extrusion drawn about its own centre, so at `scale` its half-thickness is
/// `scale / 32`: the mould's centre is the floor plus that plus a hair of
/// clearance (~12.3 px). Riding PROUD of the rim instead read as hovering
/// above the vessel; seating lower buries the one thing worth looking at
/// behind the rim.
const TRAY_AT: [f32; 3] = [
    23.0 * PX,
    BASIN_FLOOR + TRAY_SCALE / 32.0 + 0.08 * PX,
    9.0 * PX,
];
const TRAY_SCALE: f32 = 7.0 * PX;
/// The cast's BOTTOM plane: a hair INSIDE the mould's top (~12.34 px), so the
/// product reads as set into the mould rather than stacked on it. Its centre
/// is `CAST_SEAT + scale / 32` at whatever size the fill gives it — a
/// full-size cast centres ~12.5 px and tops out ~12.7 px, under the 15 px
/// rim.
const CAST_SEAT: f32 = 12.34 * PX;
const CAST_SCALE: f32 = 6.0 * PX;
/// Where the finished cast pops out: over the crucible, clear of the rim and
/// CLEAR OF THE LAUNDER that overhangs it — hence the z, which is the front of
/// the square rather than its middle. It is FOOTPRINT-LOCAL like every other
/// number in this file, and the engine turns it by the placed facing
/// (`block_local_to_world`) — a world offset off the anchor is right at one
/// facing and lands inside the masonry at the other three.
pub const EJECT_AT: [f32; 3] = [23.0 * PX, 17.5 * PX, 6.0 * PX];
const LIE_FLAT: f32 = std::f32::consts::FRAC_PI_2;

fn cast_scale(fill: f32) -> f32 {
    (0.35 + 0.65 * fill.clamp(0.0, 1.0)) * CAST_SCALE
}

const FALL_SPEED: f32 = 0.055;
const CHANNEL_RATE: f32 = 0.12;

/// The molten metal's continuous state — the part of a pour that is a NUMBER
/// rather than a stage. Kept in the machine's own cell KV, so a furnace
/// reloads mid-pour with the stream exactly where it was.
#[derive(Clone, Copy, Default, PartialEq)]
pub struct Liquid {
    /// How full the spout channel is, `0..1`.
    pub channel: f32,
    /// How far the falling stream's LEADING edge has got from the lip to the
    /// metal's surface, `0..1`. Zero while nothing is falling; `1` once it has
    /// landed.
    pub head: f32,
    /// Trailing edge of the falling column (the top).
    ///
    /// Needs both ends to obey gravity. While the tap runs, the tail stays pinned at the lip and
    /// the column grows downward. When the tap shuts, the tail falls at the same speed as
    /// everything else, so the column detaches and shortens from the top down. Don't pin the top
    /// and raise the bottom with one number; that looks like the stream getting sucked back into
    /// the furnace.
    pub tail: f32,
    pub level: f32,
}

#[derive(Default)]
pub struct Basin {
    pub mould: Option<String>,
    pub cast: Option<(String, [u8; 3], f32)>,
}

impl Liquid {
    pub fn decode(r: &mut ByteReader) -> Liquid {
        let unit = |v: Option<f32>| v.filter(|f| f.is_finite()).unwrap_or(0.0).clamp(0.0, 1.0);
        Liquid {
            channel: unit(r.f32()),
            head: unit(r.f32()),
            tail: unit(r.f32()),
            level: unit(r.f32()),
        }
    }

    pub fn encode(&self, w: &mut ByteWriter) {
        w.f32(self.channel);
        w.f32(self.head);
        w.f32(self.tail);
        w.f32(self.level);
    }

    /// One tick of flow. `tapped` means the lever's open and there's metal to run. `fill_rate` is
    /// how much of the basin one pour fills per tick.
    ///
    /// Order matters here, it's the whole sim: channel fills first, head only leaves the lip once
    /// channel has something in it, basin only rises once head hits bottom. That's what makes it
    /// read as metal traveling instead of three bars moving independently.
    pub fn step(&mut self, tapped: bool, fill_rate: f32) {
        let running = tapped || self.channel > 0.0;
        if tapped {
            self.channel = (self.channel + CHANNEL_RATE).min(1.0);
        } else {
            self.channel = (self.channel - CHANNEL_RATE).max(0.0);
        }
        if running && self.channel > 0.35 {
            self.head = (self.head + FALL_SPEED).min(1.0);
        }
        if running {
            self.tail = 0.0;
        } else if self.head > 0.0 {
            self.tail = (self.tail + FALL_SPEED).min(1.0);
            if self.tail >= self.head {
                self.head = 0.0;
                self.tail = 0.0;
            }
        }
        if self.landed() {
            self.level = (self.level + fill_rate).min(1.0);
        }
    }

    pub fn landed(&self) -> bool {
        self.head >= 1.0
    }

    fn lip_y(&self) -> f32 {
        SPOUT_Y + CHANNEL_DEPTH * self.channel.clamp(0.0, 1.0)
    }

    #[cfg(test)]
    fn fall_y(&self, t: f32) -> f32 {
        self.fall_y_to(t, BASIN_FLOOR + BASIN_DEPTH * self.level)
    }

    fn fall_y_to(&self, t: f32, surface: f32) -> f32 {
        let top = self.lip_y();
        top - (top - surface) * t.clamp(0.0, 1.0)
    }

    fn landing_y(&self, basin: &Basin) -> f32 {
        match &basin.cast {
            Some((_, _, fill)) if *fill > 0.0 => CAST_SEAT + cast_scale(*fill) / 16.0,
            _ => BASIN_FLOOR + BASIN_DEPTH * self.level,
        }
    }

    pub fn prims(&self, tile: &str, rgb: [u8; 3], basin: &Basin) -> Vec<DrawPrim> {
        let mut out = Vec::new();
        if let Some(mould) = &basin.mould {
            out.push(DrawPrim::Item {
                at: TRAY_AT,
                scale: TRAY_SCALE,
                yaw: 0.0,
                pitch: LIE_FLAT,
                item: mould.clone(),
                tint: [255, 255, 255],
            });
        }
        // The cast, ditto — the product's own art, GROWING with the fill while
        // the metal is still arriving (a pour starts at a third of full size
        // rather than popping in), and tinted from pour heat down to the
        // metal's own colour as it sets. Its centre is the seat plus its own
        // half-thickness, so a growing cast rises out of the mould instead of
        // sinking through it.
        if let Some((cast, tint, fill)) = &basin.cast {
            if *fill > 0.0 {
                let scale = cast_scale(*fill);
                out.push(DrawPrim::Item {
                    at: [TRAY_AT[0], CAST_SEAT + scale / 32.0, TRAY_AT[2]],
                    scale,
                    yaw: 0.0,
                    pitch: LIE_FLAT,
                    item: cast.clone(),
                    tint: *tint,
                });
            }
        }
        let cuboid = |min: [f32; 3], max: [f32; 3]| DrawPrim::Cuboid {
            min,
            max,
            tile: tile.to_owned(),
            tint: rgb,
            emissive: false,
        };
        if self.channel > 0.0 {
            let depth = CHANNEL_DEPTH * self.channel;
            out.push(cuboid(
                [SPOUT_A_X.0, SPOUT_A_Y, SPOUT_A_Z.0],
                [SPOUT_A_X.1, SPOUT_A_Y + depth, SPOUT_A_Z.1],
            ));
            out.push(cuboid(
                [SPOUT_X.0, SPOUT_Y, SPOUT_Z.0],
                [SPOUT_X.1, SPOUT_Y + depth, SPOUT_Z.1],
            ));
        }
        if self.head > self.tail {
            let surface = self.landing_y(basin);
            let (bottom, top) = (
                self.fall_y_to(self.head, surface),
                self.fall_y_to(self.tail, surface),
            );
            if top - bottom > 0.001 {
                out.push(cuboid(
                    [FALL_X.0, bottom, FALL_Z.0],
                    [FALL_X.1, top, FALL_Z.1],
                ));
            }
        }
        if self.level > 0.0 && basin.cast.is_none() {
            out.push(cuboid(
                [BASIN_X.0, BASIN_FLOOR, BASIN_Z.0],
                [BASIN_X.1, BASIN_FLOOR + BASIN_DEPTH * self.level, BASIN_Z.1],
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metal_travels_channel_then_air_then_basin() {
        let mut l = Liquid::default();
        l.step(true, 0.05);
        assert!(l.channel > 0.0);
        assert_eq!(l.head, 0.0, "nothing falls out of an empty channel");
        assert_eq!(l.level, 0.0);

        while !l.landed() {
            assert_eq!(l.level, 0.0, "the basin cannot rise before metal lands");
            l.step(true, 0.05);
        }
        l.step(true, 0.05);
        assert!(l.level > 0.0);
    }

    #[test]
    fn the_column_never_moves_upward_at_any_point_in_a_pour() {
        let mut l = Liquid::default();
        let (mut top, mut bottom) = (f32::INFINITY, f32::INFINITY);
        let mut drawn = 0;
        for tick in 0..400 {
            l.step(tick < 90, 0.008);
            if l.head <= l.tail {
                continue;
            }
            let (t, b) = (l.fall_y(l.tail), l.fall_y(l.head));
            assert!(b <= t, "the column is never inside out");
            if drawn > 0 {
                if l.tail > 0.0 {
                    assert!(
                        t <= top + 1e-6,
                        "tick {tick}: a detached top rose {top} -> {t}"
                    );
                }
                if !l.landed() {
                    assert!(
                        b <= bottom + 1e-6,
                        "tick {tick}: the bottom rose in mid-air"
                    );
                }
            }
            top = t;
            bottom = b;
            drawn += 1;
        }
        assert!(drawn > 30, "fixture: the stream is drawn for a good while");
    }

    #[test]
    fn the_drop_begins_at_the_metal_in_the_channel_not_under_it() {
        let mut l = Liquid::default();
        for _ in 0..12 {
            l.step(true, 0.0);
        }
        assert!(l.channel > 0.0, "fixture: the channel is running");
        let channel_surface = SPOUT_Y + CHANNEL_DEPTH * l.channel;
        assert!(
            (l.fall_y(0.0) - channel_surface).abs() < 1e-6,
            "the drop starts at {} but the channel's metal reaches {channel_surface}",
            l.fall_y(0.0)
        );
    }

    #[test]
    fn the_last_of_the_pour_falls_away_from_the_lip_downward() {
        let mut l = Liquid::default();
        while !l.landed() {
            l.step(true, 0.0);
        }
        assert_eq!(l.tail, 0.0, "while it runs, the column is attached");

        let mut prev_top = l.fall_y(l.tail);
        let mut prev_bottom = l.fall_y(l.head);
        for _ in 0..40 {
            l.step(false, 0.0);
            if l.head <= l.tail {
                break;
            }
            let (top, bottom) = (l.fall_y(l.tail), l.fall_y(l.head));
            assert!(
                top <= prev_top + 1e-6,
                "the top of the column only ever descends"
            );
            assert!(bottom <= prev_bottom + 1e-6, "and so does the bottom");
            prev_top = top;
            prev_bottom = bottom;
        }
        assert_eq!(l.head, 0.0, "the stream is gone once the tail catches it");
    }

    #[test]
    fn closing_the_tap_leaves_the_airborne_metal_falling() {
        let mut l = Liquid::default();
        for _ in 0..8 {
            l.step(true, 0.05);
        }
        let (channel, head) = (l.channel, l.head);
        assert!(
            channel > 0.0 && head > 0.0 && head < 1.0,
            "fixture: mid-fall"
        );

        l.step(false, 0.05);
        assert!(l.channel < channel, "the channel drains");
        assert!(l.head > head, "and the head keeps going");
    }

    fn elements() -> Vec<json::Value> {
        json::Value::parse(include_str!("../pack/models/forging_furnace.bbmodel"))
            .and_then(|m| {
                m.get("elements")
                    .and_then(json::Value::as_array)
                    .map(<[_]>::to_vec)
            })
            .expect("the pack's furnace model parses and has elements")
    }

    fn corner(e: &json::Value, key: &str) -> Option<[f32; 3]> {
        let a = e.get(key)?.as_array()?;
        (a.len() == 3).then(|| std::array::from_fn(|i| a[i].as_f64().unwrap_or(0.0) as f32))
    }

    fn cube(name: &str) -> ([f32; 3], [f32; 3]) {
        elements()
            .iter()
            .find(|e| e.get("name").and_then(json::Value::as_str) == Some(name))
            .and_then(|e| Some((corner(e, "from")?, corner(e, "to")?)))
            .unwrap_or_else(|| {
                panic!("no cube named '{name}' — the constants above were measured off it")
            })
    }

    /// `"fit": "native"` MAPS THE AUTHORED ORIGIN ONTO THE FOOTPRINT ORIGIN,
    /// which makes every number in this file a raw read off the model — and
    /// makes a model authored about its own centre render a whole cell away
    /// from the block it belongs to.
    ///
    /// That is invisible in Blockbench, where the model looks perfect, and it
    /// is invisible in a lone screenshot, where a furnace one cell to the left
    /// is still a furnace. It shows up as the machine standing beside the
    /// metal it is pouring, and as a block you have to aim off-target to hit.
    /// The shipped model is translated into its footprint for exactly this
    /// reason; this is the assertion that says so.
    #[test]
    fn the_model_fills_the_footprint_it_declares() {
        const BOX: [f32; 3] = [32.0, 48.0, 32.0];
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for e in elements().iter().filter(|e| e.get("rotation").is_none()) {
            let (a, b) = (corner(e, "from").unwrap(), corner(e, "to").unwrap());
            for i in 0..3 {
                lo[i] = lo[i].min(a[i]);
                hi[i] = hi[i].max(b[i]);
            }
        }
        for i in 0..3 {
            assert!(
                lo[i] > -0.25 && hi[i] < BOX[i] + 0.25,
                "axis {i} spans {}..{}, not the declared 0..{}",
                lo[i],
                hi[i],
                BOX[i]
            );
        }
        assert!(
            lo[0] < 1.0 && hi[0] > BOX[0] - 1.0,
            "the model spans its width"
        );
        assert!(lo[2] < 1.0 && hi[2] > BOX[2] - 1.0, "and its depth");
    }

    #[test]
    fn the_liquid_is_drawn_inside_the_vessel_the_model_gives_it() {
        let px = |v: f32| v * PX;
        let close = |a: f32, b: f32| (a - b).abs() < 1e-5;

        let (spout_lo, spout_hi) = cube("tap_spout");
        let (far, near) = (cube("tap_wall_far"), cube("tap_wall_near"));
        assert!(
            close(SPOUT_A_Y, px(spout_hi[1]))
                && close(SPOUT_A_Z.0, px(near.0[2]))
                && close(SPOUT_A_Z.1, px(spout_hi[2])),
            "leg A stands on the floor and runs from the turn to the tap hole"
        );
        assert!(
            close(SPOUT_A_X.0, px(cube("tap_spout_l").1[0]))
                && close(SPOUT_A_X.1, px(cube("tap_spout_r").0[0])),
            "and is the gap between leg A's walls"
        );
        let (lip_lo, lip_hi) = cube("tap_lip");
        assert!(
            close(SPOUT_X.0, px(lip_lo[0])) && close(SPOUT_X.1, px(cube("tap_spout_r").0[0])),
            "leg B runs from its lip to the outer wall it turns against"
        );
        assert!(
            close(SPOUT_Y, px(lip_hi[1])) && close(SPOUT_A_Y, SPOUT_Y),
            "the metal STANDS ON one continuous floor, so both legs read its top"
        );
        assert!(
            close(SPOUT_Z.0, px(far.1[2])) && close(SPOUT_Z.1, px(near.0[2])),
            "leg B's channel is the gap BETWEEN its walls"
        );
        assert!(
            close(FALL_Z.0, SPOUT_Z.0) && close(FALL_Z.1, SPOUT_Z.1),
            "and the falling column leaves that same gap"
        );
        assert!(
            CHANNEL_DEPTH <= px(far.1[1] - lip_hi[1]),
            "a full channel stays below the walls that hold it in"
        );

        let rim_top = px(cube("crucible_front").1[1]);
        assert!(
            close(BASIN_FLOOR, px(cube("crucible_floor").1[1])),
            "the pool stands on the crucible's floor"
        );
        assert!(
            close(BASIN_FLOOR + BASIN_DEPTH, rim_top),
            "and full means level with the rim, whatever the rim is"
        );
        let (in_x, in_z) = (
            (
                px(cube("crucible_left").1[0]),
                px(cube("crucible_right").0[0]),
            ),
            (
                px(cube("crucible_front").1[2]),
                px(cube("crucible_back").0[2]),
            ),
        );
        assert!(
            close(in_x.1 - in_x.0, in_z.1 - in_z.0),
            "THE CRUCIBLE IS SQUARE — everything cast in it is, and a stretched \
             pan reads as a gutter"
        );
        assert!(
            BASIN_X.0 >= in_x.0 && BASIN_X.1 <= in_x.1,
            "the pool is inside the rim, not through it"
        );
        assert!(BASIN_Z.0 >= in_z.0 && BASIN_Z.1 <= in_z.1);
        assert!(
            SPOUT_Y > rim_top,
            "the metal has to fall INTO the crucible, so the lip is above its rim"
        );

        let lip_x = px(lip_lo[0]);
        assert!(
            lip_x > in_x.0 && lip_x < in_x.1,
            "the launder's lip overhangs the crucible's interior"
        );
        assert!(
            px(far.0[2]) > in_z.0 && px(near.1[2]) < in_z.1,
            "and the whole channel is narrower than the square it pours into"
        );
        assert!(
            FALL_X.1 <= lip_x && FALL_X.0 >= in_x.0,
            "the stream falls OFF the lip and still inside the crucible"
        );
        let mouth = (px(cube("pier_inner").0[0]), px(cube("pier_outer").1[0]));
        assert!(
            SPOUT_A_X.0 >= mouth.0 && SPOUT_A_X.1 <= mouth.1,
            "leg A runs through the tap hole in the jamb, not across the hearth"
        );
        let strut = cube("tap_strut");
        assert!(
            close(px(strut.1[1]), px(lip_lo[1])) && px(strut.0[1]) <= rim_top,
            "the far end stands on a strut off the crucible's own rim"
        );
        assert!(
            close(px(cube("tap_console_1").1[1]), px(spout_lo[1])),
            "and the near end on the corbel out of the jamb's face"
        );

        for (what, at, scale) in [
            ("mould", TRAY_AT, TRAY_SCALE),
            (
                "cast",
                [TRAY_AT[0], CAST_SEAT + CAST_SCALE / 32.0, TRAY_AT[2]],
                CAST_SCALE,
            ),
        ] {
            assert!(
                at[0] - scale / 2.0 > in_x.0 && at[0] + scale / 2.0 < in_x.1,
                "the {what} fits inside the crucible"
            );
            assert!(
                at[2] - scale / 2.0 > in_z.0 && at[2] + scale / 2.0 < in_z.1,
                "the {what} fits inside the crucible"
            );
            assert!(
                at[1] - scale / 32.0 > BASIN_FLOOR,
                "the {what} floats just off the crucible's floor, not through it"
            );
            assert!(
                at[1] + scale / 32.0 < rim_top,
                "the {what} seats under the rim rather than riding proud of it"
            );
        }
        assert!(
            EJECT_AT[1] > rim_top && EJECT_AT[0] > in_x.0 && EJECT_AT[0] < in_x.1,
            "the cast pops out above the crucible, clear of the rim"
        );
        assert!(
            EJECT_AT[2] < px(far.0[2]) && EJECT_AT[2] > in_z.0,
            "and clear of the launder that overhangs it, not inside the trough"
        );
    }

    #[test]
    fn the_launder_turns_its_corner_without_a_step_a_notch_or_an_overhang() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-5;
        let (spout, lip) = (cube("tap_spout"), cube("tap_lip"));
        assert!(
            close(spout.0[1], lip.0[1]) && close(spout.1[1], lip.1[1]),
            "ONE floor plane at ONE thickness through the turn"
        );
        assert!(
            close(spout.0[2], lip.1[2]),
            "and its two pieces meet edge to edge"
        );
        for (arm, run, axis) in [
            ("outer", (cube("tap_spout_r"), cube("tap_wall_far")), 0),
            ("inner", (cube("tap_spout_l"), cube("tap_wall_near")), 0),
        ] {
            let (a, b) = run;
            assert!(
                close(a.0[axis], b.1[axis]),
                "the {arm} walls meet: no gap, no overlap"
            );
            assert!(
                close(a.0[2], b.0[2]),
                "the {arm} corner CLOSES — the leg-A wall reaches the far edge \
                 of the one it meets, or the turn has a notch in it"
            );
            assert!(
                close(a.0[1], b.0[1]) && close(a.1[1], b.1[1]),
                "and they are the same height, so the trough does not change \
                 section through the bend"
            );
            assert!(
                a.1[axis] <= lip.1[axis] + 1e-5 && a.0[axis] >= lip.0[axis] - 1e-5,
                "the {arm} wall stands ON the floor rather than off the end of it"
            );
        }
    }

    #[test]
    fn liquid_survives_the_kv_round_trip() {
        let mut l = Liquid::default();
        for _ in 0..14 {
            l.step(true, 0.07);
        }
        let mut w = ByteWriter::new();
        l.encode(&mut w);
        let bytes = w.finish();
        let back = Liquid::decode(&mut ByteReader::new(&bytes));
        assert!(back == l, "the pour resumes exactly where it stopped");
    }

    #[test]
    fn a_cast_replaces_the_pool_rather_than_joining_it() {
        let l = Liquid {
            level: 0.8,
            ..Liquid::default()
        };
        let pooled = l.prims("stone", [255, 0, 0], &Basin::default());
        let cast = l.prims(
            "stone",
            [255, 0, 0],
            &Basin {
                mould: None,
                cast: Some(("forge:iron_ingot".into(), [255, 128, 0], 0.8)),
            },
        );
        let cuboids = |v: &[DrawPrim]| {
            v.iter()
                .filter(|p| matches!(p, DrawPrim::Cuboid { .. }))
                .count()
        };
        assert_eq!(cuboids(&cast) + 1, cuboids(&pooled));
    }

    #[test]
    fn the_accumulating_metal_grows_in_the_moulds_shape() {
        let l = Liquid {
            level: 0.5,
            ..Liquid::default()
        };
        let basin = |fill| Basin {
            mould: Some("forge:mould_axe".into()),
            cast: Some(("forge:iron_axe_head".into(), [255, 128, 0], fill)),
        };
        let cast_prim = |fill: f32| {
            l.prims("stone", [255, 0, 0], &basin(fill))
                .into_iter()
                .find_map(|p| match p {
                    DrawPrim::Item {
                        item, at, scale, ..
                    } if item == "forge:iron_axe_head" => Some((at, scale)),
                    _ => None,
                })
                .expect("the cast is drawn while the mould fills")
        };
        let want = |fill: f32| (0.35 + 0.65 * fill) * CAST_SCALE;
        let (half_at, half_scale) = cast_prim(0.5);
        let (full_at, full_scale) = cast_prim(1.0);
        assert!(
            (half_scale - want(0.5)).abs() < 1e-6 && (full_scale - want(1.0)).abs() < 1e-6,
            "the cast's scale tracks the fill: {half_scale} and {full_scale}"
        );
        assert!(
            (half_at[1] - (CAST_SEAT + half_scale / 32.0)).abs() < 1e-6
                && (full_at[1] - (CAST_SEAT + full_scale / 32.0)).abs() < 1e-6,
            "and it rises out of the mould as it grows rather than sinking through it"
        );
        let cuboids = l
            .prims("stone", [255, 0, 0], &basin(0.5))
            .iter()
            .filter(|p| matches!(p, DrawPrim::Cuboid { .. }))
            .count();
        assert_eq!(cuboids, 0, "the shaped cast replaces the square pool");
    }

    #[test]
    fn the_column_lands_on_the_shaped_cast_not_beside_it() {
        let l = Liquid {
            head: 1.0,
            level: 0.5,
            ..Liquid::default()
        };
        let column_bottom = |basin: &Basin| {
            l.prims("stone", [255, 0, 0], basin)
                .into_iter()
                .find_map(|p| match p {
                    DrawPrim::Cuboid { min, .. } if min[0] == FALL_X.0 => Some(min[1]),
                    _ => None,
                })
                .expect("the falling column is drawn")
        };
        let shaped = Basin {
            mould: Some("forge:mould_axe".into()),
            cast: Some(("forge:iron_axe_head".into(), [255, 128, 0], 0.5)),
        };
        let cast_top = CAST_SEAT + cast_scale(0.5) / 16.0;
        let pool_surface = BASIN_FLOOR + BASIN_DEPTH * 0.5;
        assert!(
            (cast_top - pool_surface).abs() > 1e-3,
            "fixture: the two surfaces are genuinely different at this fill"
        );
        assert!(
            (column_bottom(&shaped) - cast_top).abs() < 1e-6,
            "a shaped pour's column ends on the cast's top"
        );
        assert!(
            (column_bottom(&Basin::default()) - pool_surface).abs() < 1e-6,
            "a mould-less basin keeps the rising pool surface"
        );
    }

    #[test]
    fn a_mould_less_basin_still_pools_square() {
        let l = Liquid {
            level: 0.6,
            ..Liquid::default()
        };
        let prims = l.prims("stone", [255, 0, 0], &Basin::default());
        assert!(
            prims.iter().any(|p| matches!(p, DrawPrim::Cuboid { .. })),
            "the square pool is the bare basin's metal"
        );
        assert!(
            prims.iter().all(|p| !matches!(p, DrawPrim::Item { .. })),
            "and no item stands in for it"
        );
    }
}
