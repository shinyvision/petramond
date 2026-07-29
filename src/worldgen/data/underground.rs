//! Underground (cave) biomes — a layered catalog (`assets/underground_biomes.json`).
//!
//! A cave biome is DATA: a band of the very-low-frequency cave-biome field it
//! claims, an optional depth band, the block its cave walls are lined with, and
//! how ROOMY its caves are (the caliber knobs). The carver holds no knowledge of
//! any particular biome — it reads the compiled table below and nothing else, so
//! a pack adds a cave biome the same way it adds a tree feature.
//!
//! # Selection is declared, never derived from ids
//!
//! A row's territory comes from the `field` band the row itself writes, NEVER
//! from its registry id, row index, or pack load order — those shift whenever a
//! pack is enabled, disabled, or reordered, which would reshuffle every biome in
//! every existing world. Overlaps resolve by SPECIFICITY (narrower field band
//! first, then narrower depth band, then the row's namespaced name), so a rare
//! biome may live inside a common one's territory with no coordination between
//! the two packs, and the outcome does not depend on which layer was read first.
//!
//! # Lazy-init rule
//!
//! [`table`] resolves each row's lining BLOCK NAME, which reads the block name
//! table. It is therefore safe from worldgen and the host-call handlers, but it
//! must NEVER be touched from a block/item/shape loader: a table derived inside
//! the block registry's own lazy re-enters that lazy and deadlocks.

use std::sync::LazyLock;

use serde::Deserialize;

use crate::block::Block;
use crate::chunk::{WORLD_MAX_Y, WORLD_MIN_Y};
use crate::registry::Catalog;
use crate::worldgen::noise::settings::{CAVE_BIOME_FIELD_MAX, CAVE_LATTICE_STEP, CAVE_MIN_Y};

/// Engine underground-biome names in frozen id order. Id 0 is the structural
/// FALLBACK: it declares no band, so nothing can select it by field, and
/// [`UndergroundBiomes::id_at`] answers it for every cell no banded row claims.
const ENGINE_UNDERGROUND_BIOME_NAMES: &[&str] = &["petramond:stone"];

/// How roomy a biome's caves are, and how thick it lines them.
///
/// `tunnel` multiplies the radius carvers (spaghetti, its branch family, and
/// noodles) — caliber in blocks scales as radius/frequency, so a multiplier is
/// the dimensionally correct "wider". `cheese` is ADDITIVE on the cavern carve
/// threshold, which is a threshold test rather than a radius: raising it makes
/// caverns simultaneously bigger and more common. `shell` multiplies both
/// lining-shell widths at once, so an author states one thickness and the
/// per-carver gradient calibration is preserved.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct Caliber {
    pub tunnel: f64,
    pub cheese: f64,
    pub shell: f64,
}

/// A CHAMBER: an additive density term a row contributes to the CAVERN CARVE
/// THRESHOLD itself, evaluated by the engine on the cave lattice.
///
/// This is what lets a pack declare a big room that the natural carvers EXTEND
/// into rather than being sheared by. A stamp written after the carve is a
/// boolean union against finished geometry, so a tunnel crossing its wall ends
/// at a flat face; a term summed into the threshold before it is compared makes
/// the room's rim a region where the noise still decides, so an approaching
/// tunnel simply merges with it. `caliber.cheese` is the same kind of term
/// applied uniformly across a biome; a chamber is the same term applied as a
/// bounded, positionally-rolled blob.
///
/// The engine knows only "an additive density blob on a positional lattice
/// inside a declared band". It never learns what the room is for.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct Chamber {
    /// The declaring row. A chamber rolls only in cells that row WINS, so its
    /// territory is the band the row declares and nothing else.
    pub row: u8,
    /// Stream salt, derived from the row's namespaced NAME — never from its id
    /// or index, which move the moment another pack is enabled.
    pub salt: u64,
    /// Candidate-COLUMN spacing in blocks; a multiple of [`CAVE_LATTICE_STEP`].
    ///
    /// Candidates live on a 2-D lattice and roll their depth inside the row's
    /// own `y` band, rather than on a 3-D lattice. That is not a shortcut: it
    /// is what keeps a room INSIDE the territory that authorised it, so the
    /// biome lines its walls and a pack dresses its ceiling. A room straddling
    /// its band's rim would come out half-claimed, which reads as an
    /// undecorated roof and is very hard to attribute back to here.
    pub lattice: i32,
    /// One candidate column in N carries a room, before the territory test.
    pub one_in: i32,
    pub r_min: i32,
    pub r_max: i32,
    /// Vertical radius as a fraction of the horizontal one. Well under 1: a
    /// sphere reads as a bubble, an oblate dome reads as a hall.
    pub flatten: f64,
    /// Hard bottom cut, as a fraction of the vertical radius below the centre.
    /// Below it the row contributes exactly nothing, which is what leaves a
    /// FLOOR to stand on (and lets an ordinary tunnel punch up through it)
    /// instead of the one-cell tangent point an isotropic blob bottoms out in.
    pub sill: f64,
    /// Width in BLOCKS of the rim over which the term ramps to zero, measured
    /// radially outward from the room's own surface. The whole point of the
    /// feature: inside it the threshold is only nudged, so the noise carves
    /// knuckles, alcoves and struts and the wall dissolves. Measuring it in
    /// blocks rather than in normalised radius is what keeps the CEILING as
    /// ragged as the walls — a normalised rim shrinks with `flatten`, and a
    /// thin rim is a smooth analytic dome.
    pub feather: f64,
    pub strength: f64,
    /// Upper bound on how many overlapping bubbles a room is built from
    /// (rolled `1..=lobes`). Lobes are combined by MAX, not by sum: summing
    /// lifts the crease between two bubbles back over the threshold and the
    /// result is one convex blob again, while a max leaves the crease weakly
    /// carved, so the noise puts a pillar or a pinch-point there. That
    /// non-convex silhouette is the difference between "a room" and "a dome".
    pub lobes: i32,
    /// Satellite centre offset per axis, as a fraction of the primary lobe's
    /// radii. Bounded so a satellite can never detach (see the loader).
    pub lobe_spread: f64,
    /// Satellite radius as a fraction of the primary's, `[min, max]`.
    pub lobe_scale: (f64, f64),
    /// How much a room widens the RADIUS carvers running through it, as a
    /// multiplier gain on the term: a tunnel arriving at the rim flares up to
    /// `1 + gain` times wider — and, because the lining shell rides the same
    /// fattened radius, it flares mossy. Zero (the default) leaves the radius
    /// carvers untouched, which is what a chamber did before.
    ///
    /// This is the arm that ATTACHES a room. The threshold term alone can only
    /// merge a room where the cavern field is already near its threshold; where
    /// it is not, the room is sealed however wide its rim is.
    pub tunnel: f64,
    /// How hard an existing cave field kneads the room's RIM and floor,
    /// scaling the falloff's ramp rather than displacing it.
    ///
    /// The rim is a sponge only where the cavern field is near its threshold.
    /// Where that field is dead the noise decides nothing and the room
    /// degenerates to its analytic core — a smooth machined dome with a flat
    /// disc of floor, which is exactly the read a stamped hall gives. Kneading
    /// the profile itself is the only thing that reaches that half, and it is
    /// free: the field is one the cave lattice already samples at every corner
    /// this term is evaluated at, so it costs a multiply and no sampler.
    pub rim_noise: f64,
}

impl Chamber {
    /// Vertical radius for a rolled horizontal radius. Floored at one cell so a
    /// very flat row still has a room rather than a plane.
    #[inline]
    pub fn ry(&self, rx: i32) -> f64 {
        (rx as f64 * self.flatten).max(1.0)
    }

    /// How far a satellite lobe can push a room's surface past the primary's,
    /// as a multiple of the primary's radii. Never below 1: the primary lobe
    /// is always there.
    #[inline]
    pub fn spread_factor(&self) -> f64 {
        if self.lobes > 1 {
            (self.lobe_spread + self.lobe_scale.1).max(1.0)
        } else {
            1.0
        }
    }

    /// How far below its centre a room reaches (the sill cut) and how far
    /// above (the lobes plus their rim). Every depth bound rests on these, so
    /// a profile that is not exactly zero past them is a correctness bug.
    #[inline]
    pub fn drop(&self, rx: i32) -> i32 {
        (self.ry(rx) * self.sill).round() as i32
    }

    #[inline]
    pub fn rise(&self, rx: i32) -> i32 {
        (self.ry(rx) * self.spread_factor() + self.feather).ceil() as i32
    }

    /// Furthest from its centre a rolled room's non-zero influence can reach
    /// horizontally — the candidate window's only pad.
    #[inline]
    pub fn reach_xz(&self) -> i32 {
        (self.r_max as f64 * self.spread_factor() + self.feather).ceil() as i32
    }

    /// Worst-case vertical extent over every radius the row can roll. Not
    /// monotone in `rx` once [`Self::ry`]'s floor bites, so it is taken by
    /// scanning rather than evaluated at `r_max`.
    pub fn extent_y(&self) -> (i32, i32) {
        (self.r_min..=self.r_max).fold((0, 0), |(d, r), rx| {
            (d.max(self.drop(rx)), r.max(self.rise(rx)))
        })
    }

    /// The depth band a room's CENTRE may be rolled in: the row's own band,
    /// clipped to the range the carvers actually cut. Nothing is carved below
    /// [`CAVE_MIN_Y`], so a room rolled under it is not tapered by its own sill
    /// but sliced by the world floor — a dead-flat plane of bare rock, since
    /// the lining shell needs the same `interior` gate the carve does.
    #[inline]
    pub fn placement_band(band: (i32, i32)) -> (i32, i32) {
        (band.0.max(CAVE_MIN_Y), band.1)
    }
}

/// What a biome paints on ONE orientation of its cave surface.
///
/// The engine derives the orientation geometrically — is the cell directly
/// under carved air, directly over it, or neither — and never learns what any
/// of the three means. A pack writes "sand on the floor" or "ice on the
/// ceiling" through the same clause.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct FaceLining {
    /// Block written here; `0` (air) leaves the rock bare.
    pub block: u8,
    /// Share of eligible cells painted. `1.0` takes NO roll at all, which is
    /// what makes a floor rule a guarantee rather than a high probability.
    pub weight: f32,
}

/// Per-orientation lining for one row. Absent (`UndergroundBiomes::faces`
/// answers `None`) means the row lines its walls the one way it always did, so
/// a table with no `faces` anywhere compiles to exactly the old code path.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct LiningFaces {
    /// A cell directly under carved air. Painted whether or not it falls in the
    /// carve's lining SHELL — that is the whole point of the clause, since the
    /// shell is a level set in noise units and comes out thinnest exactly where
    /// the field's Y gradient is steepest, i.e. on horizontal surfaces.
    pub floor: FaceLining,
    /// A cell that hugs a wall (the shell band) and is neither floor nor
    /// ceiling.
    pub wall: FaceLining,
    /// A cell in the shell band directly above carved air.
    pub ceiling: FaceLining,
    /// How many blocks of rock below a cave floor the floor rule paints.
    pub floor_depth: i32,
    /// What the floor course paints BELOW its top cell — the subsurface under
    /// the surface, as grass sits over dirt. `None` paints the whole course
    /// with `floor.block`, which is what every row did before the clause
    /// existed. Only meaningful with `floor_depth > 1`.
    pub floor_under: Option<FaceLining>,
    /// Dither stream salt, from the row's namespaced NAME with its own prefix
    /// so a row that declares both a chamber and a weight does not correlate
    /// its speckle with its room placement.
    pub salt: u64,
}

/// Componentwise caliber maxima over some window of the partition — what the
/// carver's conservative skip mask widens by. A 4³ lattice cell can straddle a
/// band boundary, so the mask must widen by the most permissive caliber any row
/// reaching that cell can produce; a row with a caliber below the fallback's
/// must never shrink it.
///
/// Chambers are deliberately NOT here. A caliber applies exactly where its row
/// wins, so a window query narrows it honestly; a chamber's influence spills
/// out of its row's band by design, so the same narrowing would be unsound.
/// The mask bounds chambers from the sampled lattice lane instead, which is
/// exact rather than merely conservative — see `CaveLattice::may_cut_mask`.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct CaliberBounds {
    pub tunnel: f64,
    pub cheese: f64,
    pub shell: f64,
}

/// One banded row, compiled for the selection scan.
#[derive(Copy, Clone)]
struct Band {
    lo: f64,
    hi: f64,
    y_min: i32,
    y_max: i32,
    id: u8,
    caliber: Caliber,
    blend_f: f64,
    blend_y: f64,
}

impl Band {
    #[inline]
    fn contains(&self, f: f64, y: i32) -> bool {
        y >= self.y_min && y <= self.y_max && f >= self.lo && f < self.hi
    }

    /// 0 at the band rim, easing to 1 well inside it. The biome ID is a hard
    /// step on a smooth field; without this the caliber would step with it and
    /// slice tunnels at an invisible boundary. The feathered value always lies
    /// between the fallback caliber and the row's own, so it can never exceed
    /// the bounds the skip mask widened by.
    ///
    /// The DOWNWARD ramp measures from the deepest cell the carvers cut, not
    /// from the row's declared floor. Nothing is carved below [`CAVE_MIN_Y`],
    /// so a band reaching under it would otherwise run at full caliber right
    /// into the plane where the world stops cutting, and every cell the
    /// widening opened down there is sliced off flat: one dead-level floor
    /// across the whole biome instead of a cavern bottom. Same reasoning as
    /// [`Chamber::placement_band`], one level up — that clips a room's centre
    /// for the room's own sill, this clips the caliber for everything else.
    #[inline]
    fn feather(&self, f: f64, y: i32) -> f64 {
        let ramp = |d: f64, w: f64| {
            if w > 0.0 {
                (d / w).clamp(0.0, 1.0)
            } else {
                1.0
            }
        };
        let y_lo = self.y_min.max(CAVE_MIN_Y);
        let fr = ramp((f - self.lo).min(self.hi - f), self.blend_f);
        let yr = ramp(((y - y_lo).min(self.y_max - y)) as f64, self.blend_y);
        smoothstep(fr.min(yr))
    }
}

/// One row of the loaded underground-biome table.
pub struct UndergroundBiomeDef {
    /// The row's registry name (`"petramond:marble"`, `"mymod:mushroom_cavern"`).
    pub name: &'static str,
    /// Half-open `[lo, hi)` band of the cave-biome field this row claims, or
    /// `None` for the structural fallback row.
    band: Option<(f64, f64)>,
    y: (i32, i32),
    /// Block id this biome lines cave walls with; `0` (air) = bare stone.
    lining: u8,
    lining_name: &'static str,
    /// Per-orientation override of `lining`; `None` = the one-block-everywhere
    /// shell every row had before.
    faces: Option<LiningFaces>,
    face_names: [&'static str; 3],
    caliber: Caliber,
    blend: (f64, f64),
    chamber: Option<Chamber>,
}

/// A set over the closed row-id space (ids are `u8`), for the box queries that
/// answer "which biomes CAN be here" without visiting a cell.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct IdSet([u64; 4]);

impl IdSet {
    #[inline]
    pub fn insert(&mut self, id: u8) {
        self.0[id as usize >> 6] |= 1 << (id & 63);
    }
    #[inline]
    pub fn contains(&self, id: u8) -> bool {
        self.0[id as usize >> 6] & (1 << (id & 63)) != 0
    }
    #[inline]
    pub fn union(&mut self, other: &IdSet) {
        for (a, b) in self.0.iter_mut().zip(other.0) {
            *a |= b;
        }
    }
    pub fn ids(&self) -> Vec<u8> {
        (0..=u8::MAX).filter(|&i| self.contains(i)).collect()
    }
}

/// The compiled underground-biome partition: everything the carver and the mod
/// ABI read, in the shapes their hot paths need.
pub struct UndergroundBiomes {
    catalog: Catalog<UndergroundBiomeDef>,
    /// EVERY banded row, specificity order. Lining and caliber both resolve
    /// through [`Self::winner`], so they cannot disagree about who owns a cell
    /// — a narrower row with no caliber of its own MUST shadow a wider row
    /// that has one, or a cell would be lined as one biome and shaped like
    /// another.
    bands: Box<[Band]>,
    /// Field-axis acceleration for the specificity scan. A band's field
    /// interval is half-open, so the SET of bands covering a field value
    /// changes only at a band endpoint: cutting the axis at the sorted
    /// distinct endpoints leaves segments over which the candidate set is
    /// constant. `candidates[segments[k].0..segments[k].1]` lists — still in
    /// specificity order — exactly the bands covering segment `k`, so a lookup
    /// is a binary search plus a scan of the rows that genuinely overlap
    /// THERE. Ten packs contributing one biome each scan one band, not ten;
    /// without this the per-voxel caliber lookup would be O(loaded rows).
    cuts: Box<[f64]>,
    segments: Box<[(u32, u32)]>,
    /// Indices into `bands` (never ids): row 0 is the bandless fallback, so
    /// `bands` holds at most 255 entries and an index is one byte.
    candidates: Box<[u8]>,
    /// The fallback row's caliber, applied everywhere unconditionally.
    pub base: Caliber,
    /// Whether ANY banded row declares a caliber differing from [`Self::base`]
    /// — the single gate deciding whether the carve decision needs the biome
    /// field at all. False for the shipped table.
    pub caliber_varies: bool,
    /// Dense lining block id per biome id (`0` = none): the carve inner loop's
    /// only lining lookup.
    lining: [u8; 256],
    /// Dense per-orientation lining, parallel to [`Self::lining`].
    faces: Box<[Option<LiningFaces>; 256]>,
    /// Hoisted from `faces`: whether ANY row declares one. The single gate on
    /// the orientation machinery — the extra lattice row it needs in Y, the
    /// skip mask's dilation, and the column bookkeeping all hang off it, so a
    /// table with no `faces` (every shipped one) carves exactly as before.
    pub lining_faces_vary: bool,
    /// Hoisted: some row paints FLOORS and claims depth `CAVE_MIN_Y - 1`. The
    /// deepest cave floor in the world rests on the plane the carvers refuse to
    /// cut, and that plane lives in a section the carve otherwise skips
    /// entirely — so a floor guarantee has to reach one block below the carve.
    pub lining_floor_under_world_floor: bool,
    /// Hoisted: the deepest floor course any row paints (`0` when none does).
    /// A course can start above a batch's top voxel and reach down into it, so
    /// the carve lattice pads Y by this much — the batch has to be able to ASK
    /// how far the cave floor above it is, and a remembered answer would make
    /// the course depend on which batch generated the cell.
    pub lining_floor_depth_max: i32,
    /// Caliber maxima over the WHOLE table — the answer for a window covering
    /// everything, and the only honest one when the caller has no biome field
    /// to narrow with.
    pub bounds: CaliberBounds,
    /// Caliber maxima with NO banded row in play: the fallback's own caliber,
    /// with `shell` floored at 1.0 because the entrance arm uses the unscaled
    /// shell. The floor of [`Self::bounds_for`], and its answer for a window
    /// no banded row reaches.
    pub base_bounds: CaliberBounds,
    /// Hash of the compiled table — stamped into the column-gen cache so a
    /// pack that changes cave shape cannot be served stale cached columns.
    pub fingerprint: u64,
    /// Every declared chamber, in a frozen order (see [`Self::chambers`]).
    chambers: Box<[Chamber]>,
    /// Inclusive world-Y span outside which NO declared chamber can contribute
    /// anything, or `None` when none are declared. The carve lattice skips its
    /// chamber lane entirely outside this, which is sound precisely because the
    /// contribution there is provably zero rather than merely unlikely.
    pub chamber_y_span: Option<(i32, i32)>,
}

impl UndergroundBiomes {
    /// The highest-specificity band claiming `(field value, y)`, or `None` for
    /// the fallback. The ONE resolution both lining and caliber go through.
    ///
    /// The segment index only NARROWS which bands are tested; the test itself
    /// is still [`Band::contains`] in specificity order, so the answer is the
    /// full scan's answer whatever the index says.
    #[inline]
    fn winner(&self, f: f64, y: i32) -> Option<&Band> {
        // Number of cuts at or below `f` — the segment index by construction,
        // and 0 for a NaN field value (nothing claims it, so the fallback).
        let seg = self.cuts.partition_point(|c| *c <= f);
        let (lo, hi) = self.segments[seg];
        self.candidates[lo as usize..hi as usize]
            .iter()
            .map(|&i| &self.bands[i as usize])
            .find(|b| b.contains(f, y))
    }

    /// The biome owning `(field value, y)`. Pure and TOTAL: volume no row
    /// claims answers the fallback, id 0.
    #[inline]
    pub fn id_at(&self, f: f64, y: i32) -> u8 {
        self.winner(f, y).map_or(0, |b| b.id)
    }

    /// The lining block id for a biome id (`0` = bare stone).
    #[inline]
    pub fn lining(&self, id: u8) -> u8 {
        self.lining[id as usize]
    }

    /// The per-orientation lining for a biome id, or `None` when the row lines
    /// every cave surface with the same block.
    #[inline]
    pub fn faces(&self, id: u8) -> Option<&LiningFaces> {
        self.faces[id as usize].as_ref()
    }

    /// The caliber at `(field value, y)`, feathered from [`Self::base`] at band
    /// edges. Only entered when [`Self::caliber_varies`].
    #[inline]
    pub fn caliber_at(&self, f: f64, y: i32) -> Caliber {
        match self.winner(f, y) {
            Some(b) if b.caliber != self.base => {
                let t = b.feather(f, y);
                Caliber {
                    tunnel: lerp(self.base.tunnel, b.caliber.tunnel, t),
                    cheese: lerp(self.base.cheese, b.caliber.cheese, t),
                    shell: lerp(self.base.shell, b.caliber.shell, t),
                }
            }
            _ => self.base,
        }
    }

    /// The widest caliber any row that can APPLY inside the depth window `y`
    /// (inclusive) and field window `field` (inclusive) reaches.
    ///
    /// [`Self::bounds`] is the same maximum taken over the whole table. Taking
    /// it per window instead is what stops a row banded to a rare field value
    /// or a narrow depth from widening the carver's skip mask in volume it can
    /// never claim: the global maximum converges on the load caps as packs
    /// accumulate, until nothing is skipped anywhere for anyone.
    ///
    /// Conservative in the direction that matters — a row is counted whenever
    /// its bands merely INTERSECT the window, so the result can only exceed
    /// what the carver will actually use inside it.
    pub fn bounds_for(&self, y: (i32, i32), field: (f64, f64)) -> CaliberBounds {
        // A NaN or inverted window cannot be narrowed honestly; take the table.
        if field.0.is_nan() || field.1.is_nan() || field.0 > field.1 {
            return self.bounds;
        }
        // Every band endpoint is a cut, so a band overlapping the field window
        // covers at least one whole segment of it and appears in that
        // segment's candidate list — the same construction `winner` relies on.
        let k0 = self.cuts.partition_point(|c| *c <= field.0);
        let k1 = self.cuts.partition_point(|c| *c <= field.1);
        let (lo, hi) = (self.segments[k0].0 as usize, self.segments[k1].1 as usize);
        let mut ub = self.base_bounds;
        for &i in &self.candidates[lo..hi] {
            let b = &self.bands[i as usize];
            if b.y_max >= y.0 && b.y_min <= y.1 {
                ub.tunnel = ub.tunnel.max(b.caliber.tunnel);
                ub.cheese = ub.cheese.max(b.caliber.cheese);
                ub.shell = ub.shell.max(b.caliber.shell);
            }
        }
        ub
    }

    /// Every row id that can OWN a cell somewhere inside the depth window `y`
    /// and field window `field` (both inclusive), folded into `out`.
    ///
    /// The same window logic as [`Self::bounds_for`], answering identity
    /// instead of caliber: a band is counted whenever it merely INTERSECTS the
    /// window, and the fallback is always counted because volume no band
    /// claims answers it. So the result is a conservative SUPERSET — an id it
    /// omits provably does not occur in the window, which is what makes it a
    /// sound rejection gate.
    pub fn ids_in(&self, y: (i32, i32), field: (f64, f64), out: &mut IdSet) {
        out.insert(0);
        if field.0.is_nan() || field.1.is_nan() || field.0 > field.1 {
            for b in self.bands.iter() {
                out.insert(b.id);
            }
            return;
        }
        let k0 = self.cuts.partition_point(|c| *c <= field.0);
        let k1 = self.cuts.partition_point(|c| *c <= field.1);
        let (lo, hi) = (self.segments[k0].0 as usize, self.segments[k1].1 as usize);
        for &i in &self.candidates[lo..hi] {
            let b = &self.bands[i as usize];
            if b.y_max >= y.0 && b.y_min <= y.1 {
                out.insert(b.id);
            }
        }
    }

    /// Every declared chamber, in the order the carver must accumulate them.
    ///
    /// The order is frozen (row id, which the catalog assigns by layer then by
    /// first appearance) because f64 addition is not associative: two lattice
    /// boxes sharing a corner must sum the same terms in the same sequence or
    /// the corner differs in its last ulp and a seam appears at some section
    /// boundaries on some seeds.
    #[inline]
    pub fn chambers(&self) -> &[Chamber] {
        &self.chambers
    }

    /// A row's inclusive depth band.
    #[inline]
    pub fn row_y(&self, id: u8) -> (i32, i32) {
        self.catalog.rows()[id as usize].y
    }

    /// The id registered under `name`, or `None` when no such row is loaded.
    pub fn id(&self, name: &str) -> Option<u8> {
        self.catalog.id(name)
    }

    /// The registry name of `id`, or `None` when out of range.
    pub fn name(&self, id: u8) -> Option<&'static str> {
        self.catalog.rows().get(id as usize).map(|r| r.name)
    }
}

/// One underground-biome row as written in `underground_biomes.json`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawUndergroundBiome {
    underground_biome: String,
    /// Half-open `[lo, hi)` selection band on the cave-biome field. Required
    /// for every row but the fallback (a row without one could never generate).
    #[serde(default)]
    field: Option<[f64; 2]>,
    /// Inclusive `[min, max]` depth band; absent = the whole column.
    #[serde(default)]
    y: Option<[i32; 2]>,
    #[serde(default)]
    lining: Option<RawLining>,
    #[serde(default)]
    caliber: Option<RawCaliber>,
    #[serde(default)]
    chamber: Option<RawChamber>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLining {
    block: Block,
    #[serde(default = "one")]
    shell: f64,
    #[serde(default)]
    faces: Option<RawFaces>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFaces {
    #[serde(default)]
    floor: Option<RawFace>,
    #[serde(default)]
    wall: Option<RawFace>,
    #[serde(default)]
    ceiling: Option<RawFace>,
    /// Blocks of rock below a cave floor the floor rule paints.
    #[serde(default = "one_i32")]
    floor_depth: i32,
    /// The course BELOW its top cell — surface over subsurface, as grass sits
    /// over dirt. Omit to paint the whole course with the floor block.
    #[serde(default)]
    floor_under: Option<RawFace>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFace {
    /// Overrides `lining.block` for this orientation; omit to use it.
    #[serde(default)]
    block: Option<Block>,
    /// Share of eligible cells painted; `1` (the default) is every one of them.
    #[serde(default = "one_f32")]
    weight: f32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCaliber {
    #[serde(default = "one")]
    tunnel: f64,
    #[serde(default)]
    cheese: f64,
    #[serde(default = "default_blend")]
    blend: [f64; 2],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawChamber {
    lattice: i32,
    #[serde(default = "one_i32")]
    one_in: i32,
    radius: [i32; 2],
    #[serde(default = "default_flatten")]
    flatten: f64,
    #[serde(default = "default_sill")]
    sill: f64,
    feather: f64,
    #[serde(default = "one")]
    strength: f64,
    #[serde(default = "one_i32")]
    lobes: i32,
    #[serde(default)]
    lobe_spread: f64,
    #[serde(default = "default_lobe_scale")]
    lobe_scale: [f64; 2],
    #[serde(default)]
    tunnel: f64,
    #[serde(default)]
    rim_noise: f64,
}

fn one() -> f64 {
    1.0
}

fn one_f32() -> f32 {
    1.0
}

fn one_i32() -> i32 {
    1
}

fn default_lobe_scale() -> [f64; 2] {
    [1.0, 1.0]
}

fn default_flatten() -> f64 {
    0.6
}

fn default_sill() -> f64 {
    0.7
}

/// Caliber feather widths: field units, then blocks.
fn default_blend() -> [f64; 2] {
    [0.05, 8.0]
}

// Load bounds. Generous by design — they bound how far the carver's skip mask
// has to widen (and hence generation cost), not the author's taste. Violations
// are load errors: loud beats plausible.

// Chamber load bounds. Same policy: these bound the CARVER's cost and the
// soundness arguments the skip mask rests on, not the author's taste.

/// The process-wide table, built once from the real catalog layers.
///
/// See the module docs: safe from worldgen and the host-call handlers, never
/// from a block/item/shape loader.
mod load;

use load::parse_layers;

pub fn table() -> &'static UndergroundBiomes {
    static TABLE: LazyLock<UndergroundBiomes> = LazyLock::new(|| {
        crate::registry::read_catalog("underground_biomes.json", "underground biome", parse_layers)
    });
    &TABLE
}

/// The underground-biome id registered under `name` — the mod ABI's resolver.
pub fn id_by_name(name: &str) -> Option<u8> {
    table().id(name)
}

type ConvertedFaces = (Option<LiningFaces>, [&'static str; 3]);

/// Cut the field axis at every distinct band endpoint and list, per segment,
/// the bands covering it — see [`UndergroundBiomes::cuts`].
type FieldAxisIndex = (Box<[f64]>, Box<[(u32, u32)]>, Box<[u8]>);

#[inline]
fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

#[inline]
fn smoothstep(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Test seam: compile a table from the shipped engine layer plus synthetic pack
/// layers, leaked so it can drive a [`CaveField`](crate::worldgen::noise::cave_field::CaveField)
/// without touching the process-wide catalog.
#[cfg(test)]
pub(crate) fn test_table(pack_layers: &[&str]) -> &'static UndergroundBiomes {
    let base = shipped_layer();
    let mut texts: Vec<&str> = vec![&base];
    texts.extend_from_slice(pack_layers);
    Box::leak(Box::new(
        parse_layers(&texts).expect("synthetic underground table"),
    ))
}

/// The BASE layer only — a synthetic table must mean the same thing whether or
/// not a pack shipping its own cave biomes happens to be installed.
#[cfg(test)]
pub(crate) fn shipped_layer() -> String {
    crate::assets::read_base_text("underground_biomes.json")
        .expect("shipped underground_biomes.json")
        .0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hazard 1 in the flesh: territory must come from the band a row DECLARES,
    /// so the partition survives enabling, disabling, or reordering any pack.
    #[test]
    fn overlapping_bands_resolve_by_specificity_not_load_order() {
        let wide = r#"{"underground_biomes": [
            {"underground_biome": "wide:zone", "field": [0.0, 1.0]}]}"#;
        let narrow = r#"{"underground_biomes": [
            {"underground_biome": "narrow:zone", "field": [0.4, 0.5]}]}"#;

        let a = test_table(&[wide, narrow]);
        let b = test_table(&[narrow, wide]);
        let (wa, na) = (a.id("wide:zone").unwrap(), a.id("narrow:zone").unwrap());
        let (wb, nb) = (b.id("wide:zone").unwrap(), b.id("narrow:zone").unwrap());
        assert_ne!(wa, wb, "load order does move registry IDS ...");
        for y in [-40, 0, 40] {
            assert_eq!(a.id_at(0.45, y), na, "the narrower band wins");
            assert_eq!(b.id_at(0.45, y), nb, "... and the SAME row wins either way");
            // 0.1 is below the shipped marble band, so only `wide` claims it.
            assert_eq!(a.id_at(0.1, y), wa);
            assert_eq!(b.id_at(0.1, y), wb);
            assert_eq!(a.id_at(-0.5, y), 0, "unclaimed volume is the fallback");
        }

        // A depth band is narrower than none at the same field width.
        let deep = r#"{"underground_biomes": [
            {"underground_biome": "deep:zone", "field": [0.0, 1.0], "y": [-64, 0]}]}"#;
        let t = test_table(&[wide, deep]);
        assert_eq!(t.id_at(0.1, -20), t.id("deep:zone").unwrap());
        assert_eq!(t.id_at(0.1, 20), t.id("wide:zone").unwrap());

        // Identical specificity: the alphabetically earlier NAME wins, both ways.
        let alpha = r#"{"underground_biomes": [
            {"underground_biome": "aaa:zone", "field": [0.1, 0.2]}]}"#;
        let beta = r#"{"underground_biomes": [
            {"underground_biome": "zzz:zone", "field": [0.1, 0.2]}]}"#;
        for order in [[alpha, beta], [beta, alpha]] {
            let t = test_table(&order);
            assert_eq!(t.id_at(0.15, 0), t.id("aaa:zone").unwrap());
        }
    }

    /// The shipped file resolves against the real block registry, pack rows
    /// register after the engine range, and every load bound is a hard error.
    #[test]
    fn engine_rows_hold_frozen_ids_and_pack_rows_register_after() {
        let base = shipped_layer();
        // The first row RE-DECLARES an engine row (the fallback), which must
        // replace it in place and add no id; the second is a pack addition.
        let pack = r#"{"underground_biomes": [
            {"underground_biome": "petramond:stone",
             "lining": {"block": "petramond:marble"}},
            {"underground_biome": "mymod:fungal", "field": [0.3, 1.0], "y": [-64, 8],
             "lining": {"block": "petramond:moss_block", "shell": 1.5},
             "caliber": {"tunnel": 1.35, "cheese": 0.13, "blend": [0.06, 8]}}]}"#;
        let t = parse_layers(&[&base, pack]).expect("loads");
        for (id, name) in ENGINE_UNDERGROUND_BIOME_NAMES.iter().enumerate() {
            assert_eq!(t.name(id as u8), Some(*name), "engine ids never move");
        }
        let after_engine = ENGINE_UNDERGROUND_BIOME_NAMES.len() as u8;
        assert_eq!(t.id("mymod:fungal"), Some(after_engine));
        assert_eq!(
            t.name(after_engine + 1),
            None,
            "the engine override adds no id; only the pack addition does"
        );
        assert!(t.caliber_varies, "a modulating row arms the biome read");
        assert!(t.bounds.tunnel >= 1.35 && t.bounds.cheese >= 0.13 && t.bounds.shell >= 1.5);
        assert_ne!(
            t.fingerprint,
            parse_layers(&[&base]).unwrap().fingerprint,
            "a pack that reshapes caves must not reuse cached columns"
        );

        // Every base band here is REACHABLE, so each case fails for the reason
        // it names. A dead band is its own case below: reuse one as the base
        // and eleven of these would pass without testing anything.
        for bad in [
            r#"{"underground_biome": "mymod:x", "field": [0.3, 1.0],
                "lining": {"block": "petramond:not_a_block"}}"#,
            r#"{"underground_biome": "petramond:stone", "field": [0.3, 1.0]}"#,
            r#"{"underground_biome": "mymod:x"}"#,
            r#"{"underground_biome": "mymod:x", "field": [0.4, 0.3]}"#,
            r#"{"underground_biome": "mymod:x", "field": [0.3, 1.0], "y": [8, -64]}"#,
            r#"{"underground_biome": "mymod:x", "field": [0.3, 1.0],
                "caliber": {"tunnel": 12.0}}"#,
            r#"{"underground_biome": "mymod:x", "field": [0.3, 1.0],
                "caliber": {"cheese": 3.0}}"#,
            r#"{"underground_biome": "mymod:x", "field": [0.3, 1.0],
                "lining": {"block": "petramond:marble", "shell": 0.0}}"#,
            r#"{"underground_biome": "mymod:x", "field": [0.3, 1.0],
                "lining": {"block": "petramond:air"}}"#,
            r#"{"underground_biome": "mymod:x", "field": [0.3, 1.0], "sparkle": 1}"#,
            // A chamber's bounds all protect either the carver's cost or a
            // soundness argument, so every one of them is loud.
            r#"{"underground_biome": "mymod:x", "field": [0.3, 1.0],
                "chamber": {"lattice": 64, "radius": [8, 12], "feather": 10}}"#,
            r#"{"underground_biome": "petramond:stone",
                "chamber": {"lattice": 64, "radius": [8, 12], "feather": 10}}"#,
            r#"{"underground_biome": "mymod:x", "field": [0.3, 1.0], "y": [-64, -16],
                "chamber": {"lattice": 66, "radius": [8, 12], "feather": 10}}"#,
            r#"{"underground_biome": "mymod:x", "field": [0.3, 1.0], "y": [-64, -16],
                "chamber": {"lattice": 64, "radius": [12, 8], "feather": 10}}"#,
            r#"{"underground_biome": "mymod:x", "field": [0.3, 1.0], "y": [-64, -16],
                "chamber": {"lattice": 64, "radius": [8, 12], "feather": 4}}"#,
            r#"{"underground_biome": "mymod:x", "field": [0.3, 1.0], "y": [-64, -16],
                "chamber": {"lattice": 64, "radius": [8, 12], "feather": 10, "strength": 9.0}}"#,
            r#"{"underground_biome": "mymod:x", "field": [0.3, 1.0], "y": [-64, -16],
                "chamber": {"lattice": 64, "radius": [8, 12], "feather": 10, "sill": 2.0}}"#,
            // reach 40 > half of lattice 64: the candidate window stops being
            // a small constant.
            r#"{"underground_biome": "mymod:x", "field": [0.3, 1.0], "y": [-64, -16],
                "chamber": {"lattice": 64, "radius": [8, 30], "feather": 10}}"#,
            // a band far too short to hold the room it authorises
            r#"{"underground_biome": "mymod:x", "field": [0.3, 1.0], "y": [-64, -56],
                "chamber": {"lattice": 64, "radius": [8, 12], "feather": 10}}"#,
            // long enough on paper, but nothing is carved below CAVE_MIN_Y and
            // the room is placed in the CARVABLE part: a room that only fits
            // where it can never be cut is the "loads fine, never generates"
            // failure again.
            r#"{"underground_biome": "mymod:x", "field": [0.3, 1.0], "y": [-64, -34],
                "chamber": {"lattice": 64, "radius": [8, 12], "feather": 10}}"#,
            // a satellite lobe that can clear the primary leaves a detached
            // bubble — a sealed pocket, the exact failure the attachment work
            // removes.
            r#"{"underground_biome": "mymod:x", "field": [0.3, 1.0], "y": [-64, -16],
                "chamber": {"lattice": 64, "radius": [8, 12], "feather": 10,
                            "lobes": 3, "lobe_spread": 0.9, "lobe_scale": [0.4, 0.6]}}"#,
            r#"{"underground_biome": "mymod:x", "field": [0.3, 1.0], "y": [-64, -16],
                "chamber": {"lattice": 64, "radius": [8, 12], "feather": 10, "lobes": 9}}"#,
            r#"{"underground_biome": "mymod:x", "field": [0.3, 1.0], "y": [-64, -16],
                "chamber": {"lattice": 64, "radius": [8, 12], "feather": 10,
                            "lobe_scale": [0.8, 0.4]}}"#,
            r#"{"underground_biome": "mymod:x", "field": [0.3, 1.0], "y": [-64, -16],
                "chamber": {"lattice": 64, "radius": [8, 12], "feather": 10, "tunnel": 9.0}}"#,
            r#"{"underground_biome": "mymod:x", "field": [0.3, 1.0], "y": [-64, -16],
                "chamber": {"lattice": 64, "radius": [8, 12], "feather": 10,
                            "rim_noise": 4.0}}"#,
            // an orientation lining's own bounds
            r#"{"underground_biome": "mymod:x", "field": [0.3, 1.0],
                "lining": {"block": "petramond:marble",
                           "faces": {"floor_depth": 9}}}"#,
            r#"{"underground_biome": "mymod:x", "field": [0.3, 1.0],
                "lining": {"block": "petramond:marble",
                           "faces": {"wall": {"weight": 1.5}}}}"#,
            r#"{"underground_biome": "mymod:x", "field": [0.3, 1.0],
                "lining": {"block": "petramond:marble", "faces": {"roof": {}}}}"#,
        ] {
            let layer = format!("{{\"underground_biomes\": [{bad}]}}");
            assert!(
                parse_layers(&[&base, &layer]).is_err(),
                "should be a load error: {bad}"
            );
        }
    }

    /// A chamber or an orientation lining reshapes what a column generates, so
    /// a retune of one must not be served stale columns out of the colgen
    /// cache. The fingerprint is a hand-written field list, which is exactly
    /// why this needs an assertion rather than a promise: every knob has to
    /// move it.
    #[test]
    fn every_chamber_knob_moves_the_table_fingerprint() {
        let base = shipped_layer();
        let row = |clause: &str| {
            let layer = format!(
                "{{\"underground_biomes\": [{{\"underground_biome\": \"mymod:x\", \
                 \"field\": [0.3, 1.0], \"y\": [-64, -8], {clause}}}]}}"
            );
            parse_layers(&[&base, &layer])
                .expect("valid row")
                .fingerprint
        };
        let chamber = |c: &str| row(&format!("\"chamber\": {c}"));
        const CANON: &str = r#"{"lattice": 64, "one_in": 2, "radius": [8, 12],
            "flatten": 0.6, "sill": 0.7, "feather": 10, "strength": 1.0,
            "lobes": 2, "lobe_spread": 0.4, "lobe_scale": [0.6, 0.8],
            "tunnel": 1.0, "rim_noise": 0.5}"#;
        let canon = chamber(CANON);
        assert_ne!(canon, parse_layers(&[&base]).unwrap().fingerprint);
        // One knob at a time, spelled out: a loop over the CANON string would
        // pass just as well against a fingerprint that hashed the raw JSON.
        for (key, was, now) in [
            ("\"lattice\": 64", "64", "96"),
            ("\"one_in\": 2", "2", "3"),
            ("\"radius\": [8, 12]", "[8, 12]", "[9, 12]"),
            ("\"radius\": [8, 12]", "[8, 12]", "[8, 13]"),
            ("\"flatten\": 0.6", "0.6", "0.65"),
            ("\"sill\": 0.7", "0.7", "0.75"),
            ("\"feather\": 10", "10", "11"),
            ("\"strength\": 1.0", "1.0", "1.1"),
            ("\"lobes\": 2", "2", "3"),
            ("\"lobe_spread\": 0.4", "0.4", "0.45"),
            ("\"lobe_scale\": [0.6, 0.8]", "[0.6, 0.8]", "[0.55, 0.8]"),
            ("\"lobe_scale\": [0.6, 0.8]", "[0.6, 0.8]", "[0.6, 0.85]"),
            ("\"tunnel\": 1.0", "1.0", "1.5"),
            ("\"rim_noise\": 0.5", "0.5", "0.6"),
        ] {
            let tweak = CANON.replace(key, &key.replace(was, now));
            assert_ne!(
                canon,
                chamber(&tweak),
                "fingerprint blind to: {key} -> {now}"
            );
        }

        // The same for the orientation lining, which also moves generated blocks.
        const LINE: &str = r#""lining": {"block": "petramond:moss_block", "shell": 1.4,
            "faces": {"floor_depth": 2, "floor": {"weight": 0.9},
                      "wall": {"weight": 0.7}, "ceiling": {"weight": 0.2}}}"#;
        let lined = row(LINE);
        assert_ne!(
            lined,
            row(r#""lining": {"block": "petramond:moss_block", "shell": 1.4}"#)
        );
        for (was, now) in [
            ("\"floor_depth\": 2", "\"floor_depth\": 3"),
            (
                "\"floor\": {\"weight\": 0.9}",
                "\"floor\": {\"weight\": 0.8}",
            ),
            ("\"wall\": {\"weight\": 0.7}", "\"wall\": {\"weight\": 0.6}"),
            (
                "\"ceiling\": {\"weight\": 0.2}",
                "\"ceiling\": {\"weight\": 0.1}",
            ),
            (
                "\"floor\": {\"weight\": 0.9}",
                "\"floor\": {\"weight\": 0.9, \"block\": \"petramond:marble\"}",
            ),
        ] {
            assert_ne!(
                lined,
                row(&LINE.replace(was, now)),
                "fingerprint blind to: {now}"
            );
        }
    }

    /// A band outside what the cave-biome field can REACH loads fine and then
    /// silently never generates — the same authoring mistake as a row with no
    /// band, which is already a load error. The open-ended `[lo, 1.0]` idiom
    /// every shipped row uses must survive the guard.
    #[test]
    fn a_field_band_the_noise_can_never_reach_is_a_load_error() {
        let base = shipped_layer();
        let load = |band: &str| {
            let layer = format!(
                "{{\"underground_biomes\": [{{\"underground_biome\": \"mymod:x\", \
                 \"field\": {band}}}]}}"
            );
            parse_layers(&[&base, &layer]).is_ok()
        };
        let reach = CAVE_BIOME_FIELD_MAX;
        for dead in ["[0.6, 1.0]", "[1.4, 1.5]", "[-1.0, -0.6]", "[-9.0, -0.9]"] {
            assert!(!load(dead), "unreachable band {dead} must not load");
        }
        for live in ["[0.24, 1.0]", "[0.17, 1.0]", "[-1.5, 1.5]", "[-0.54, -0.5]"] {
            assert!(load(live), "reachable band {live} must still load");
        }
        // Half-open: a band ENDING at the field's floor claims nothing, while
        // one STARTING at its ceiling still claims that extreme value.
        assert!(
            !load(&format!("[-1.0, {}]", -reach)),
            "hi == -reach is empty"
        );
        assert!(load(&format!("[{reach}, 1.0]")), "lo == +reach is not");
    }

    /// The field-axis index is an ACCELERATOR, never a second answer: whatever
    /// it narrows the scan to, the winner must be the one a full specificity
    /// scan finds. Probed hardest exactly AT the band endpoints, where a
    /// half-open interval and a segment boundary can disagree by one ulp.
    #[test]
    fn the_field_axis_index_answers_what_a_full_specificity_scan_answers() {
        // Nested, adjacent, identical and depth-split bands in one table —
        // every overlap shape the specificity rule has to arbitrate.
        let pack = r#"{"underground_biomes": [
            {"underground_biome": "a:wide",   "field": [-1.0, 1.0]},
            {"underground_biome": "b:mid",    "field": [-0.2, 0.6]},
            {"underground_biome": "c:narrow", "field": [0.1, 0.2]},
            {"underground_biome": "d:same",   "field": [0.1, 0.2]},
            {"underground_biome": "e:touch",  "field": [0.2, 0.3]},
            {"underground_biome": "f:deep",   "field": [-0.2, 0.6], "y": [-64, -20]},
            {"underground_biome": "g:high",   "field": [0.3, 0.53], "y": [40, 200]}]}"#;
        let t = test_table(&[pack]);
        let naive = |f: f64, y: i32| {
            t.bands
                .iter()
                .find(|b| b.contains(f, y))
                .map_or(0, |b| b.id)
        };

        let mut probes: Vec<f64> = vec![f64::NAN, -2.0, 2.0];
        for b in t.bands.iter() {
            for edge in [b.lo, b.hi] {
                probes.extend([
                    edge,
                    f64::from_bits(edge.to_bits() - 1),
                    f64::from_bits(edge.to_bits() + 1),
                    edge + 0.001,
                    edge - 0.001,
                ]);
            }
        }
        for i in -300..=300 {
            probes.push(i as f64 / 200.0);
        }
        for f in probes {
            for y in [-64, -21, -20, -19, 0, 39, 40, 41, 200, 255] {
                assert_eq!(t.id_at(f, y), naive(f, y), "indexed lookup at ({f}, {y})");
            }
        }
    }

    /// `bounds_for` is what keeps one pack row from taxing every carve in the
    /// world, and it is sound only if it counts every row whose bands merely
    /// INTERSECT the window — a window straddling a band edge from either
    /// side, on either axis, included. Too wide only costs generation time;
    /// too narrow makes the skip mask delete caves, which nothing downstream
    /// can detect. Like the field-axis index it accelerates, it must answer
    /// exactly what a full scan over the intersecting rows answers.
    #[test]
    fn bounds_for_answers_what_a_full_scan_over_intersecting_rows_answers() {
        let pack = r#"{"underground_biomes": [
            {"underground_biome": "a:wide",   "field": [-1.0, 1.0],
             "caliber": {"tunnel": 1.2}},
            {"underground_biome": "b:mid",    "field": [-0.2, 0.4],
             "caliber": {"tunnel": 2.0, "cheese": 0.1}},
            {"underground_biome": "c:narrow", "field": [0.1, 0.2],
             "lining": {"block": "petramond:marble", "shell": 3.0}},
            {"underground_biome": "d:touch",  "field": [0.2, 0.3],
             "caliber": {"tunnel": 4.0, "cheese": 0.5}},
            {"underground_biome": "e:deep",   "field": [-0.2, 0.4], "y": [-64, -20],
             "caliber": {"tunnel": 3.0, "cheese": 0.4}},
            {"underground_biome": "f:high",   "field": [0.3, 0.53], "y": [40, 200],
             "caliber": {"tunnel": 2.2}}]}"#;
        let t = test_table(&[pack]);
        let naive = |y: (i32, i32), f: (f64, f64)| {
            let mut ub = t.base_bounds;
            for b in t.bands.iter() {
                if b.y_max >= y.0 && b.y_min <= y.1 && f.1 >= b.lo && f.0 < b.hi {
                    ub.tunnel = ub.tunnel.max(b.caliber.tunnel);
                    ub.cheese = ub.cheese.max(b.caliber.cheese);
                    ub.shell = ub.shell.max(b.caliber.shell);
                }
            }
            ub
        };

        let mut edges: Vec<f64> = vec![-2.0, -0.55, 0.0, 0.55, 2.0];
        for b in t.bands.iter() {
            for e in [b.lo, b.hi] {
                edges.extend([
                    e,
                    f64::from_bits(e.to_bits() - 1),
                    f64::from_bits(e.to_bits() + 1),
                    e - 0.001,
                    e + 0.001,
                ]);
            }
        }
        // A lattice cell's field window is narrow, so the interesting windows
        // are short ones straddling an edge — plus degenerate and wide ones.
        let widths = [0.0, 1e-9, 0.002, 0.05, 0.5];
        let depths = [
            (-64, -61),
            (-24, -21),
            (-21, -18),
            (-4, -1),
            (40, 43),
            (200, 203),
        ];
        for &lo in &edges {
            for w in widths {
                let f = (lo, lo + w);
                for y in depths {
                    assert_eq!(t.bounds_for(y, f), naive(y, f), "window {y:?} {f:?}");
                }
            }
        }
        // A NaN window cannot be narrowed honestly; it must fall back to the
        // whole table rather than quietly answering the base.
        assert_eq!(t.bounds_for((-64, 0), (f64::NAN, 0.3)), t.bounds);
    }

    /// Lining and caliber must resolve to the SAME row. A narrower plain row
    /// shadowing a wider caliber row is the case that separates "scan the
    /// bands" from "scan the modulating subset": get it wrong and a cell is
    /// lined as one biome but shaped like another.
    #[test]
    fn the_row_that_owns_a_cell_owns_both_its_lining_and_its_caliber() {
        let pack = r#"{"underground_biomes": [
            {"underground_biome": "wide:roomy", "field": [-1.5, 1.5],
             "caliber": {"tunnel": 2.0, "blend": [0, 0]}},
            {"underground_biome": "narrow:plain", "field": [0.0, 0.1],
             "lining": {"block": "petramond:moss_block"}}]}"#;
        let t = test_table(&[pack]);
        let narrow = t.id("narrow:plain").unwrap();
        assert_eq!(t.id_at(0.05, 0), narrow, "the narrower row owns the cell");
        assert_eq!(
            t.caliber_at(0.05, 0),
            t.base,
            "so the wider row's caliber must NOT leak into it"
        );
        assert_eq!(t.caliber_at(-0.05, 0).tunnel, 2.0, "and does apply outside");
    }

    /// The caliber feather stays between the fallback and the row's own value
    /// (what makes the skip-mask widening sound) and reaches both ends.
    #[test]
    fn caliber_feathers_between_the_fallback_and_the_row() {
        let pack = r#"{"underground_biomes": [
            {"underground_biome": "mymod:roomy", "field": [0.2, 0.8],
             "caliber": {"tunnel": 2.0, "blend": [0.1, 0]}}]}"#;
        let t = test_table(&[pack]);
        assert_eq!(
            t.caliber_at(0.1, 0).tunnel,
            t.base.tunnel,
            "outside the band"
        );
        assert_eq!(t.caliber_at(0.5, 0).tunnel, 2.0, "deep inside the band");
        for f in [0.21, 0.25, 0.3, 0.6, 0.75, 0.79] {
            let c = t.caliber_at(f, 0).tunnel;
            assert!(
                (t.base.tunnel..=2.0).contains(&c),
                "feathered caliber {c} left the [base, row] interval at f={f}"
            );
        }
    }

    /// A row banded BELOW the carvable floor must still feather its caliber out
    /// at [`CAVE_MIN_Y`]. Nothing is cut lower, so a row that reaches full
    /// caliber there has everything its widening opened sliced off by the world
    /// floor — one dead-level plane across the whole biome, which is what
    /// Rachel reported as "the lowest level goes flat". One `max()` a refactor
    /// loses in silence, and the symptom is 40 blocks away from the code.
    #[test]
    fn a_rows_caliber_feathers_out_at_the_carvable_floor_not_its_declared_one() {
        let pack = r#"{"underground_biomes": [
            {"underground_biome": "mymod:deep", "field": [0.2, 1.0], "y": [-64, -16],
             "caliber": {"tunnel": 2.0, "blend": [0.0, 8]}},
            {"underground_biome": "mymod:shallow", "field": [-1.0, -0.2], "y": [-40, -16],
             "caliber": {"tunnel": 2.0, "blend": [0.0, 8]}}]}"#;
        let t = test_table(&[pack]);
        let deep = |y| t.caliber_at(0.5, y).tunnel;
        assert_eq!(deep(CAVE_MIN_Y), t.base.tunnel, "the guillotine plane");
        assert_eq!(deep(CAVE_MIN_Y + 8), 2.0, "a blend width above it");
        for y in CAVE_MIN_Y..CAVE_MIN_Y + 8 {
            assert!(
                deep(y) < deep(y + 1),
                "the ramp off the carve floor must be strictly rising at y={y}"
            );
        }
        // A row whose own floor is already inside the carvable range keeps
        // feathering against the floor it declared.
        let shallow = |y| t.caliber_at(-0.5, y).tunnel;
        assert_eq!(shallow(-40), t.base.tunnel, "its own declared floor");
        assert_eq!(shallow(-32), 2.0, "a blend width above that");
    }
}
