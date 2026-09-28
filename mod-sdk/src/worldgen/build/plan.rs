use crate::{
    AuthoredData, AuthoredPalette, BlockId, ByteReader, ByteWriter, ColumnBox, ColumnMask,
    FxHashMap, GenFill, GenOutput, GenRng, SectionBox,
};

use super::families::Families;
use super::material::{Form, Half, Material, AIR};
use super::rng::Draw;

const FORMAT: u32 = 3;
const UNSEEN: u32 = u32::MAX;
const UNRESOLVED: u32 = u32::MAX - 1;

struct Object {
    material: u16,
    footprint: Vec<[i32; 3]>,
}

/// Plan cells in dense 16³ slabs, one per section touched: slot `0` is empty, otherwise the
/// material id plus one. Builders write in clusters, so a small cache of recent lookups answers
/// most of them without hashing, and encoding walks slots already in section order.
#[derive(Default)]
struct Cells {
    slabs: Vec<([i32; 3], Box<Slab>)>,
    index: FxHashMap<[i32; 3], usize>,
    /// Recent lookups, including misses, direct-mapped by the section's low coordinate bits.
    recent: [std::cell::Cell<([i32; 3], u32)>; RECENT],
}

const RECENT: usize = 32;
const RECENT_MATERIALS: usize = 16;
const NO_SLAB: u32 = u32::MAX;

#[inline]
fn recent_slot([x, y, z]: [i32; 3]) -> usize {
    (x & 3 | (z & 3) << 2 | (y & 1) << 4) as usize
}

struct Slab {
    slots: [u16; 4096],
    occupied: [u64; 64],
    /// Cells inside a multi-cell object's footprint.
    covered: [u64; 64],
}

impl Slab {
    /// Occupied `(local, slot)` pairs in local order.
    fn iter(&self) -> impl Iterator<Item = (u16, u16)> + '_ {
        self.occupied
            .iter()
            .enumerate()
            .flat_map(move |(w, &word)| {
                let mut bits = word;
                std::iter::from_fn(move || {
                    if bits == 0 {
                        return None;
                    }
                    let l = (w * 64) as u16 + bits.trailing_zeros() as u16;
                    bits &= bits - 1;
                    Some((l, self.slots[l as usize]))
                })
            })
    }
}

impl Cells {
    fn len(&self) -> usize {
        let occupied = |slab: &Slab| {
            slab.occupied
                .iter()
                .map(|w| w.count_ones() as usize)
                .sum::<usize>()
        };
        self.slabs.iter().map(|(_, slab)| occupied(slab)).sum()
    }

    #[inline(always)]
    fn slab(&self, section: [i32; 3]) -> Option<usize> {
        let (s, i) = self.recent[recent_slot(section)].get();
        if s == section && i != 0 {
            return (i != NO_SLAB).then(|| i as usize - 1);
        }
        self.slab_uncached(section)
    }

    #[inline(never)]
    fn slab_uncached(&self, section: [i32; 3]) -> Option<usize> {
        let found = self.index.get(&section).copied();
        self.recent[recent_slot(section)].set((section, found.map_or(NO_SLAB, |i| i as u32 + 1)));
        found
    }

    #[inline]
    fn get(&self, pos: [i32; 3]) -> Option<u16> {
        let slot = self.slabs[self.slab(section_of(pos))?].1.slots[local(pos) as usize];
        (slot != 0).then(|| slot - 1)
    }

    #[inline(always)]
    fn slab_or_new(&mut self, section: [i32; 3]) -> usize {
        match self.slab(section) {
            Some(i) => i,
            None => self.new_slab(section),
        }
    }

    #[cold]
    #[inline(never)]
    fn new_slab(&mut self, section: [i32; 3]) -> usize {
        self.slabs.push((
            section,
            Box::new(Slab {
                slots: [0; 4096],
                occupied: [0; 64],
                covered: [0; 64],
            }),
        ));
        let i = self.slabs.len() - 1;
        self.index.insert(section, i);
        self.recent[recent_slot(section)].set((section, i as u32 + 1));
        i
    }

    #[inline]
    fn is_covered(&self, pos: [i32; 3]) -> bool {
        self.slab(section_of(pos)).is_some_and(|i| {
            let l = local(pos) as usize;
            self.slabs[i].1.covered[l / 64] & (1 << (l % 64)) != 0
        })
    }

    fn set_covered(&mut self, pos: [i32; 3], on: bool) {
        let i = self.slab_or_new(section_of(pos));
        let l = local(pos) as usize;
        let word = &mut self.slabs[i].1.covered[l / 64];
        if on {
            *word |= 1 << (l % 64);
        } else {
            *word &= !(1 << (l % 64));
        }
    }

    fn insert(&mut self, pos: [i32; 3], id: u16) {
        let i = self.slab_or_new(section_of(pos));
        let l = local(pos) as usize;
        let slab = &mut self.slabs[i].1;
        slab.slots[l] = id + 1;
        slab.occupied[l / 64] |= 1 << (l % 64);
    }

    #[inline]
    fn remove(&mut self, pos: [i32; 3]) {
        if let Some(i) = self.slab(section_of(pos)) {
            let l = local(pos) as usize;
            let slab = &mut self.slabs[i].1;
            slab.slots[l] = 0;
            slab.occupied[l / 64] &= !(1 << (l % 64));
        }
    }

    fn iter(&self) -> impl Iterator<Item = ([i32; 3], u16)> + '_ {
        self.slabs.iter().flat_map(|(section, slab)| {
            let origin = section.map(|v| v * 16);
            slab.iter().map(move |(l, slot)| (at(origin, l), slot - 1))
        })
    }
}

/// Everything one generated structure writes, keyed by world position. Later writes replace
/// earlier ones; writing into any cell of a multi-cell object removes the whole object, so a
/// plan can never leave half a door standing.
#[derive(Default)]
pub struct Plan {
    palette: Vec<Material>,
    lookup: FxHashMap<Material, u16>,
    /// Recently interned materials, direct-mapped by [`Material::quick_slot`].
    recent: [Option<(Material, u16)>; RECENT_MATERIALS],
    cells: Cells,
    slabs: Vec<[i32; 3]>,
    objects: FxHashMap<[i32; 3], Object>,
    covered: FxHashMap<[i32; 3], [i32; 3]>,
    data: FxHashMap<[i32; 3], Vec<(String, Vec<u8>)>>,
    clears: Vec<([i32; 2], i32, i32)>,
    clear_grids: Vec<ClearGrid>,
    claim: Option<ColumnMask>,
}

impl Plan {
    pub fn new() -> Plan {
        Plan::default()
    }

    #[inline(always)]
    fn intern(&mut self, material: &Material) -> u16 {
        let slot = material.quick_slot() % RECENT_MATERIALS;
        if let Some((m, id)) = &self.recent[slot] {
            if m == material {
                return *id;
            }
        }
        self.intern_uncached(material, slot)
    }

    #[inline(never)]
    fn intern_uncached(&mut self, material: &Material, slot: usize) -> u16 {
        let id = match self.lookup.get(material) {
            Some(&id) => id,
            None => {
                let id = self.palette.len() as u16;
                self.palette.push(*material);
                self.lookup.insert(*material, id);
                id
            }
        };
        self.recent[slot] = Some((*material, id));
        id
    }

    #[cold]
    #[inline(never)]
    fn drop_object_at(&mut self, pos: [i32; 3]) {
        let Some(anchor) = self.covered.get(&pos).copied() else {
            return;
        };
        if let Some(object) = self.objects.remove(&anchor) {
            for offset in object.footprint {
                let cell = add(anchor, offset);
                self.covered.remove(&cell);
                self.cells.set_covered(cell, false);
            }
        }
    }

    #[inline]
    fn forget(&mut self, pos: [i32; 3]) {
        if self.cells.is_covered(pos) {
            self.drop_object_at(pos);
        }
        if !self.data.is_empty() {
            self.data.remove(&pos);
        }
    }

    #[inline(always)]
    pub fn set(&mut self, pos: [i32; 3], material: &Material) {
        let id = self.intern(material);
        let i = self.cells.slab_or_new(section_of(pos));
        let l = local(pos) as usize;
        if !self.covered.is_empty() && self.cells.slabs[i].1.covered[l / 64] & (1 << (l % 64)) != 0
        {
            self.drop_object_at(pos);
        }
        if !self.data.is_empty() {
            self.data.remove(&pos);
        }
        let slab = &mut self.cells.slabs[i].1;
        slab.slots[l] = id + 1;
        slab.occupied[l / 64] |= 1 << (l % 64);
        if material.form == Form::Slab {
            self.slabs.push(pos);
        }
    }

    /// Stops writing `pos`: the terrain (or whatever else generated there) stays, unless a
    /// [`clear_column`](Plan::clear_column) range covers it.
    #[inline]
    pub fn unset(&mut self, pos: [i32; 3]) {
        self.forget(pos);
        self.cells.remove(pos);
    }

    /// A multi-cell block anchored at `anchor` whose shape family writes every `footprint`
    /// offset (a door is `[[0, 0, 0], [0, 1, 0]]`).
    pub fn object(&mut self, anchor: [i32; 3], material: &Material, footprint: &[[i32; 3]]) {
        for &offset in footprint {
            let pos = add(anchor, offset);
            self.forget(pos);
            self.cells.remove(pos);
        }
        let id = self.intern(material);
        for &offset in footprint {
            self.covered.insert(add(anchor, offset), anchor);
            self.cells.set_covered(add(anchor, offset), true);
        }
        self.objects.insert(
            anchor,
            Object {
                material: id,
                footprint: footprint.to_vec(),
            },
        );
    }

    #[inline(always)]
    pub fn get(&self, pos: [i32; 3]) -> Option<&Material> {
        let slab = &self.cells.slabs[self.cells.slab(section_of(pos))?].1;
        let l = local(pos) as usize;
        let slot = slab.slots[l];
        if slot != 0 {
            return Some(&self.palette[slot as usize - 1]);
        }
        if slab.covered[l / 64] & (1 << (l % 64)) == 0 {
            return None;
        }
        let anchor = self.covered.get(&pos)?;
        Some(&self.palette[self.objects[anchor].material as usize])
    }

    /// Whether the plan builds something visible at `pos` (not air, not decor).
    #[inline]
    pub fn occupied(&self, pos: [i32; 3]) -> bool {
        self.get(pos).is_some_and(Material::occupies)
    }

    /// Namespaced data for the cell at `pos`, which must be written by this plan when emitted.
    pub fn data(&mut self, pos: [i32; 3], key: &str, value: Vec<u8>) {
        let entries = self.data.entry(pos).or_default();
        match entries.iter_mut().find(|(k, _)| k == key) {
            Some(entry) => entry.1 = value,
            None => entries.push((key.to_string(), value)),
        }
    }

    /// Clears the column's natural contents from `lo` to `hi` (inclusive): trees, plants and
    /// terrain the structure cuts away. Whatever the plan itself builds there still stands.
    /// Keeps the columns of `mask` for this structure: the engine's trees stay out of them.
    pub fn claim(&mut self, mask: ColumnMask) {
        self.claim = Some(mask);
    }

    pub fn claim_mask(&self) -> Option<&ColumnMask> {
        self.claim.as_ref()
    }

    pub fn clear_column(&mut self, x: i32, z: i32, lo: i32, hi: i32) {
        if lo <= hi {
            self.clears.push(([x, z], lo, hi));
        }
    }

    /// [`clear_column`](Plan::clear_column) for every column of the rectangle `width` columns
    /// wide at `min`, `ranges` holding its `(lo, hi)` row by row along x (none where `lo > hi`).
    pub fn clear_columns(&mut self, min: [i32; 2], width: usize, ranges: Vec<(i32, i32)>) {
        if width > 0 && !ranges.is_empty() {
            self.clear_grids.push(ClearGrid {
                min,
                size: [width, ranges.len().div_ceil(width)],
                ranges,
                extra: Vec::new(),
            });
        }
    }

    pub fn cells(&self) -> impl Iterator<Item = ([i32; 3], &Material)> + '_ {
        self.cells
            .iter()
            .map(|(pos, id)| (pos, &self.palette[id as usize]))
            .chain(
                self.objects
                    .iter()
                    .map(|(&pos, o)| (pos, &self.palette[o.material as usize])),
            )
    }

    pub fn len(&self) -> usize {
        self.cells.len() + self.objects.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Replaces every slab that would leave a half-block gap against what it rests on or what
    /// rests on it with its family's full block: a top slab over something solid (or the ground
    /// under `ground_top`), a bottom slab under something solid. The grid cannot close that gap
    /// any other way.
    pub fn settle_slabs(&mut self, families: &Families, ground_top: impl Fn(i32, i32) -> i32) {
        let mut replace = Vec::new();
        let mut slabs = std::mem::take(&mut self.slabs);
        slabs.sort_unstable();
        slabs.dedup();
        for pos in slabs {
            let Some(id) = self.cells.get(pos) else {
                continue;
            };
            let material = &self.palette[id as usize];
            if material.form != Form::Slab {
                continue;
            }
            let [x, y, z] = pos;
            let touches = match material.get_half() {
                Half::Top => {
                    let below = [x, y - 1, z];
                    match self.get(below) {
                        Some(m) => m.occupies(),
                        None => y - 1 <= ground_top(x, z),
                    }
                }
                Half::Bottom => self.occupied([x, y + 1, z]),
            };
            if touches {
                if let Some(full) = families.sibling(material, Form::Block) {
                    replace.push((pos, full));
                }
            }
        }
        for (pos, full) in replace {
            let id = self.intern(&full);
            self.cells.insert(pos, id);
        }
    }

    /// Lays `cover` (a snow layer) on the topmost solid-topped cell of every column the plan
    /// writes, with probability `chance(x, z)`.
    pub fn cover(&mut self, cover: &Material, rng: &mut GenRng, chance: impl Fn(i32, i32) -> f32) {
        let Some((lo, hi)) = self.column_bounds() else {
            return;
        };
        let width = (hi[0] - lo[0] + 1) as usize;
        let mut tops = vec![i32::MIN; width * (hi[1] - lo[1] + 1) as usize];
        let at = |x: i32, z: i32| (z - lo[1]) as usize * width + (x - lo[0]) as usize;
        for (pos, material) in self.cells() {
            if !material.is_air() {
                let top = &mut tops[at(pos[0], pos[2])];
                *top = (*top).max(pos[1]);
            }
        }
        let columns: Vec<([i32; 2], i32)> = (lo[0]..=hi[0])
            .flat_map(|x| (lo[1]..=hi[1]).map(move |z| [x, z]))
            .filter_map(|[x, z]| {
                let y = tops[at(x, z)];
                (y != i32::MIN).then_some(([x, z], y))
            })
            .collect();
        for ([x, z], y) in columns {
            let solid = self.get([x, y, z]).is_some_and(Material::solid_top);
            let above = self.get([x, y + 1, z]);
            if solid && above.is_none_or(Material::is_air) && rng.roll(chance(x, z)) {
                self.set([x, y + 1, z], cover);
            }
        }
    }

    /// The inclusive column box every cell and object anchor lies in.
    fn column_bounds(&self) -> Option<([i32; 2], [i32; 2])> {
        let mut lo = [i32::MAX; 2];
        let mut hi = [i32::MIN; 2];
        let mut take = |x: i32, z: i32| {
            lo = [lo[0].min(x), lo[1].min(z)];
            hi = [hi[0].max(x), hi[1].max(z)];
        };
        for (section, _) in &self.cells.slabs {
            take(section[0] * 16, section[2] * 16);
            take(section[0] * 16 + 15, section[2] * 16 + 15);
        }
        for anchor in self.objects.keys() {
            take(anchor[0], anchor[2]);
        }
        (lo[0] <= hi[0]).then_some((lo, hi))
    }

    /// Every cleared column range as one dense grid over their columns, one range per column,
    /// and the ranges that could not join their column's (disjoint from it).
    fn clear_grid(&self) -> ClearGrid {
        if let ([grid], []) = (self.clear_grids.as_slice(), self.clears.as_slice()) {
            return grid.clone();
        }
        let mut grid = ClearGrid::default();
        let columns = self.clear_grids.iter().flat_map(|g| {
            [
                g.min,
                [
                    g.min[0] + g.size[0] as i32 - 1,
                    g.min[1] + g.size[1] as i32 - 1,
                ],
            ]
        });
        let Some(first) = columns
            .clone()
            .chain(self.clears.iter().map(|c| c.0))
            .next()
        else {
            return grid;
        };
        let (mut lo, mut hi) = (first, first);
        for [x, z] in columns.chain(self.clears.iter().map(|c| c.0)) {
            lo = [lo[0].min(x), lo[1].min(z)];
            hi = [hi[0].max(x), hi[1].max(z)];
        }
        let size = [0, 1].map(|a| (hi[a] as i64 - lo[a] as i64 + 1) as usize);
        let listed =
            self.clear_grids.iter().flat_map(|g| {
                let min = g.min;
                let width = g.size[0];
                g.ranges.iter().enumerate().filter(|(_, r)| r.0 <= r.1).map(
                    move |(i, &(y0, y1))| {
                        (
                            [min[0] + (i % width) as i32, min[1] + (i / width) as i32],
                            y0,
                            y1,
                        )
                    },
                )
            });
        let all = listed.chain(self.clears.iter().copied());
        if size[0].saturating_mul(size[1]) > 1 << 20 {
            grid.extra = all.collect();
            return grid;
        }
        grid.min = lo;
        grid.size = size;
        grid.ranges = vec![(i32::MAX, i32::MIN); size[0] * size[1]];
        for ([x, z], y0, y1) in all {
            let range = &mut grid.ranges[(z - lo[1]) as usize * size[0] + (x - lo[0]) as usize];
            if range.0 > range.1 {
                *range = (y0, y1);
            } else if y0 as i64 <= range.1 as i64 + 1 && range.0 as i64 <= y1 as i64 + 1 {
                *range = (range.0.min(y0), range.1.max(y1));
            } else {
                grid.extra.push(([x, z], y0, y1));
            }
        }
        grid
    }

    /// The plan bucketed by section, in the form [`IndexedPlan`] emits from: a worker reads
    /// only the section it generates, never the whole plan.
    pub fn encode(&self) -> Vec<u8> {
        let mut sections: FxHashMap<[i32; 3], Slice> = FxHashMap::default();
        for (section, slab) in &self.cells.slabs {
            let slice = sections.entry(*section).or_default();
            let cells = &mut slice.cells;
            cells.reserve(slab.occupied.iter().map(|w| w.count_ones() as usize).sum());
            for (w, &word) in slab.occupied.iter().enumerate() {
                let mut bits = word;
                while bits != 0 {
                    let l = (w * 64) as u16 + bits.trailing_zeros() as u16;
                    bits &= bits - 1;
                    let ([l0, l1], [id0, id1]) =
                        (l.to_le_bytes(), (slab.slots[l as usize] - 1).to_le_bytes());
                    cells.push([l0, l1, id0, id1]);
                }
            }
        }
        for (&anchor, object) in &self.objects {
            let mut touched: Vec<[i32; 3]> = object
                .footprint
                .iter()
                .map(|&f| section_of(add(anchor, f)))
                .collect();
            touched.sort_unstable();
            touched.dedup();
            for s in touched {
                sections
                    .entry(s)
                    .or_default()
                    .anchors
                    .push((anchor, object.material));
            }
        }
        for (&pos, kv) in &self.data {
            let slice = sections.entry(section_of(pos)).or_default();
            for (k, v) in kv {
                slice.data.push((local(pos), k, v));
            }
        }
        for (section, boxes) in self.clear_grid().boxes() {
            sections.entry(section).or_default().cleared = boxes;
        }

        let mut bodies: Vec<([i32; 3], Vec<u8>)> = sections
            .into_iter()
            .map(|(s, mut slice)| (s, slice.encode()))
            .collect();
        bodies.sort_unstable_by_key(|(s, _)| *s);
        let payload: usize = bodies.iter().map(|(_, b)| b.len()).sum();
        let mut w =
            ByteWriter::with_capacity(payload + bodies.len() * 20 + self.palette.len() * 48);
        w.u32(FORMAT);
        match &self.claim {
            Some(mask) => {
                w.u32(1);
                write_mask(&mut w, mask);
            }
            None => w.u32(0),
        }
        w.u32(self.palette.len() as u32);
        for m in &self.palette {
            w.blob(m.block.as_str().as_bytes());
            w.u32(m.state_pairs().count() as u32);
            for (k, v) in m.state_pairs() {
                w.blob(k.as_bytes());
                w.blob(v.as_bytes());
            }
        }
        w.u32(bodies.len() as u32);
        let mut offset = 0u32;
        for (s, body) in &bodies {
            w.i32x3(*s);
            w.u32(offset);
            w.u32(body.len() as u32);
            offset += body.len() as u32;
        }
        for (_, body) in &bodies {
            w.raw(body);
        }
        w.finish()
    }
}

#[inline]
fn add(a: [i32; 3], b: [i32; 3]) -> [i32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

#[inline]
fn section_of([x, y, z]: [i32; 3]) -> [i32; 3] {
    [x >> 4, y >> 4, z >> 4]
}

/// A cell's index within its section, `y * 256 + z * 16 + x`.
#[inline]
fn local([x, y, z]: [i32; 3]) -> u16 {
    (((y & 15) << 8) | ((z & 15) << 4) | (x & 15)) as u16
}

/// The world position of cell `local` in the section whose minimum corner is `origin`.
#[inline]
fn at(origin: [i32; 3], local: u16) -> [i32; 3] {
    let l = local as i32;
    [
        origin[0] + (l & 15),
        origin[1] + (l >> 8),
        origin[2] + ((l >> 4) & 15),
    ]
}

#[derive(Default)]
struct Slice<'a> {
    /// `(local, material)` records, little-endian.
    cells: Vec<[u8; 4]>,
    anchors: Vec<([i32; 3], u16)>,
    data: Vec<(u16, &'a String, &'a Vec<u8>)>,
    cleared: Vec<[u8; 6]>,
}

impl Slice<'_> {
    fn encode(&mut self) -> Vec<u8> {
        self.anchors.sort_unstable();
        self.data.sort_unstable();
        let mut w = ByteWriter::with_capacity(
            16 + self.cells.len() * 4 + self.anchors.len() * 14 + self.cleared.len() * 6,
        );
        w.u32(self.cells.len() as u32);
        w.raw(self.cells.as_flattened());
        w.u32(self.anchors.len() as u32);
        for &(anchor, id) in &self.anchors {
            w.i32x3(anchor);
            w.u16(id);
        }
        w.u32(self.data.len() as u32);
        for &(at, k, v) in &self.data {
            w.u16(at);
            w.blob(k.as_bytes());
            w.blob(v);
        }
        w.u32(self.cleared.len() as u32);
        for b in &self.cleared {
            w.raw(b);
        }
        w.finish()
    }
}

/// Cleared column ranges: a dense grid of one `(lo, hi)` per column over `min`, `size` (empty
/// where `lo > hi`), plus the ranges disjoint from their column's.
#[derive(Clone, Default)]
struct ClearGrid {
    min: [i32; 2],
    size: [usize; 2],
    ranges: Vec<(i32, i32)>,
    extra: Vec<([i32; 2], i32, i32)>,
}

impl ClearGrid {
    /// The cleared cells as disjoint boxes, each clipped into every section it reaches: runs of
    /// columns with the same range along x, stacked along z, neither crossing a section border.
    fn boxes(&self) -> Vec<([i32; 3], Vec<[u8; 6]>)> {
        let (mut lo, mut hi) = ([i32::MAX; 3], [i32::MIN; 3]);
        let mut take = |[x, z]: [i32; 2], y0: i32, y1: i32| {
            lo = [lo[0].min(x >> 4), lo[1].min(y0 >> 4), lo[2].min(z >> 4)];
            hi = [hi[0].max(x >> 4), hi[1].max(y1 >> 4), hi[2].max(z >> 4)];
        };
        if !self.ranges.is_empty() {
            let (y0, y1) = self
                .ranges
                .iter()
                .filter(|r| r.0 <= r.1)
                .fold((i32::MAX, i32::MIN), |(a, b), r| (a.min(r.0), b.max(r.1)));
            if y0 <= y1 {
                take(self.min, y0, y1);
                let last = [0, 1].map(|a| self.min[a] + self.size[a] as i32 - 1);
                take(last, y0, y1);
            }
        }
        for &(column, y0, y1) in &self.extra {
            take(column, y0, y1);
        }
        let mut out = SectionBins::new(lo, hi);
        let mut emit = |[x0, x1]: [i32; 2], [z0, z1]: [i32; 2], (lo, hi): (i32, i32)| {
            for sy in lo >> 4..=hi >> 4 {
                let (y0, y1) = (lo.max(sy * 16) - sy * 16, hi.min(sy * 16 + 15) - sy * 16);
                out.push(
                    [x0 >> 4, sy, z0 >> 4],
                    [
                        (x0 & 15) as u8,
                        (x1 & 15) as u8,
                        (z0 & 15) as u8,
                        (z1 & 15) as u8,
                        y0 as u8,
                        y1 as u8,
                    ],
                );
            }
        };
        // Runs of the previous row still growing along z: `(x0, x1, z0, range)`.
        let mut open: Vec<(i32, i32, i32, (i32, i32))> = Vec::new();
        let mut next: Vec<(i32, i32, i32, (i32, i32))> = Vec::new();
        let width = self.size[0].max(1);
        for (row, z) in self.ranges.chunks(width).zip(self.min[1]..) {
            if z & 15 == 0 {
                for (x0, x1, z0, range) in open.drain(..) {
                    emit([x0, x1], [z0, z - 1], range);
                }
            }
            next.clear();
            let mut j = 0;
            let mut i = 0;
            while i < row.len() {
                let range = row[i];
                let x0 = self.min[0] + i as i32;
                let mut end = i;
                while end + 1 < row.len()
                    && row[end + 1] == range
                    && (self.min[0] + end as i32 + 1) & 15 != 0
                {
                    end += 1;
                }
                let x1 = self.min[0] + end as i32;
                i = end + 1;
                if range.0 > range.1 {
                    continue;
                }
                while j < open.len() && open[j].0 < x0 {
                    let (a, b, z0, r) = open[j];
                    emit([a, b], [z0, z - 1], r);
                    j += 1;
                }
                let mut z0 = z;
                if j < open.len() && open[j].0 == x0 {
                    let (a, b, from, r) = open[j];
                    if (b, r) == (x1, range) {
                        z0 = from;
                    } else {
                        emit([a, b], [from, z - 1], r);
                    }
                    j += 1;
                }
                next.push((x0, x1, z0, range));
            }
            let last = z;
            for &(a, b, z0, r) in &open[j..] {
                emit([a, b], [z0, last - 1], r);
            }
            std::mem::swap(&mut open, &mut next);
        }
        let end = self.min[1] + (self.ranges.len() / width) as i32;
        for (x0, x1, z0, range) in open.drain(..) {
            emit([x0, x1], [z0, end - 1], range);
        }
        for (column, pieces) in self.extra_pieces() {
            for range in pieces {
                emit([column[0]; 2], [column[1]; 2], range);
            }
        }
        out.into_vec()
    }

    /// The extra ranges merged per column and cut away from the column's grid range, so no cell
    /// is cleared twice.
    fn extra_pieces(&self) -> Vec<ColumnPieces> {
        let mut extra = self.extra.clone();
        extra.sort_unstable_by_key(|&([x, z], lo, _)| (z, x, lo));
        let mut out: Vec<ColumnPieces> = Vec::new();
        for ([x, z], lo, hi) in extra {
            match out.last_mut() {
                Some((c, pieces)) if *c == [x, z] => match pieces.last_mut() {
                    Some(last) if lo as i64 <= last.1 as i64 + 1 => last.1 = last.1.max(hi),
                    _ => pieces.push((lo, hi)),
                },
                _ => out.push(([x, z], vec![(lo, hi)])),
            }
        }
        for ([x, z], pieces) in &mut out {
            let (gx, gz) = (
                *x as i64 - self.min[0] as i64,
                *z as i64 - self.min[1] as i64,
            );
            let inside =
                (0..self.size[0] as i64).contains(&gx) && (0..self.size[1] as i64).contains(&gz);
            if !inside {
                continue;
            }
            let (glo, ghi) = self.ranges[gz as usize * self.size[0] + gx as usize];
            if glo > ghi {
                continue;
            }
            *pieces = pieces
                .iter()
                .flat_map(|&(lo, hi)| {
                    [
                        (lo, hi.min(glo.saturating_sub(1))),
                        (lo.max(ghi.saturating_add(1)), hi),
                    ]
                })
                .filter(|&(lo, hi)| lo <= hi)
                .collect();
        }
        out
    }
}

/// A column and the `(lo, hi)` ranges cleared in it.
type ColumnPieces = ([i32; 2], Vec<(i32, i32)>);

/// Per-section lists over a box of sections: indexed directly when the box is small.
struct SectionBins<T> {
    lo: [i32; 3],
    size: [usize; 3],
    dense: Vec<Vec<T>>,
    sparse: FxHashMap<[i32; 3], Vec<T>>,
}

impl<T> SectionBins<T> {
    fn new(lo: [i32; 3], hi: [i32; 3]) -> Self {
        let size = [0, 1, 2].map(|a| (hi[a] as i64 - lo[a] as i64 + 1).max(0) as usize);
        let volume = size[0].saturating_mul(size[1]).saturating_mul(size[2]);
        let dense = if volume <= 1 << 14 {
            (0..volume).map(|_| Vec::new()).collect()
        } else {
            Vec::new()
        };
        SectionBins {
            lo,
            size,
            dense,
            sparse: FxHashMap::default(),
        }
    }

    #[inline]
    fn push(&mut self, section: [i32; 3], item: T) {
        if self.dense.is_empty() {
            self.sparse.entry(section).or_default().push(item);
            return;
        }
        let [x, y, z] = [0, 1, 2].map(|a| (section[a] - self.lo[a]) as usize);
        self.dense[(z * self.size[1] + y) * self.size[0] + x].push(item);
    }

    fn into_vec(self) -> Vec<([i32; 3], Vec<T>)> {
        let (lo, size) = (self.lo, self.size);
        let dense = self
            .dense
            .into_iter()
            .enumerate()
            .filter(|(_, v)| !v.is_empty());
        dense
            .map(|(i, v)| {
                let (x, rest) = (i % size[0], i / size[0]);
                let section = [
                    lo[0] + x as i32,
                    lo[1] + (rest % size[1]) as i32,
                    lo[2] + (rest / size[1]) as i32,
                ];
                (section, v)
            })
            .chain(self.sparse)
            .collect()
    }
}

fn write_mask(w: &mut ByteWriter, mask: &ColumnMask) {
    for v in [mask.bounds.min, mask.bounds.max].concat() {
        w.i32(v);
    }
    w.blob(&mask.bits);
}

fn read_mask(r: &mut ByteReader) -> Option<ColumnMask> {
    let [a, b, c, d] = [r.i32()?, r.i32()?, r.i32()?, r.i32()?];
    let bounds = ColumnBox {
        min: [a, b],
        max: [c, d],
    };
    Some(ColumnMask {
        bounds,
        bits: r.blob()?.to_vec(),
    })
    .filter(ColumnMask::is_well_formed)
}

/// A claim mask on its own, as [`Plan::encode`] writes it inside a plan.
pub(super) fn encode_mask(mask: &ColumnMask) -> Vec<u8> {
    let mut w = ByteWriter::with_capacity(20 + mask.bits.len());
    write_mask(&mut w, mask);
    w.finish()
}

pub(super) fn decode_mask(bytes: &[u8]) -> Option<ColumnMask> {
    read_mask(&mut ByteReader::new(bytes))
}

/// A published plan as one worker holds it: the encoded bytes, a table of its sections, and the
/// materials this worker has resolved so far. Emitting a section decodes only that section.
pub struct IndexedPlan {
    bytes: Vec<u8>,
    materials: Vec<usize>,
    sections: Vec<([i32; 3], usize, usize)>,
    palette: Resolved,
    claim: Option<ColumnMask>,
}

/// Plan materials as this session's packed palette entries, resolved on first use.
struct Resolved {
    air: Option<Option<BlockId>>,
    materials: Vec<Option<Option<Vec<u8>>>>,
    slot: Vec<u32>,
    used: Vec<u16>,
}

impl Resolved {
    /// The output palette index of plan material `id`, adding it to `out` on first use in this
    /// emission; `None` when its block does not resolve.
    #[inline(always)]
    fn slot(
        &mut self,
        bytes: &[u8],
        offsets: &[usize],
        id: u16,
        resolve: &mut impl FnMut(&str) -> Option<BlockId>,
        out: &mut GenOutput,
    ) -> Option<u16> {
        match *self.slot.get(id as usize)? {
            UNRESOLVED => None,
            UNSEEN => self.first_use(bytes, offsets, id, resolve, out),
            index => Some(index as u16),
        }
    }

    #[inline(never)]
    fn first_use(
        &mut self,
        bytes: &[u8],
        offsets: &[usize],
        id: u16,
        resolve: &mut impl FnMut(&str) -> Option<BlockId>,
        out: &mut GenOutput,
    ) -> Option<u16> {
        let i = id as usize;
        if self.materials[i].is_none() {
            self.materials[i] = Some(decode_material(&bytes[offsets[i]..], resolve));
        }
        self.used.push(id);
        let index = match self.materials[i].as_ref().and_then(Option::as_ref) {
            Some(entry) => out.authored.palette.push_encoded(entry),
            None => UNRESOLVED,
        };
        self.slot[i] = index;
        (index != UNRESOLVED).then_some(index as u16)
    }

    fn reset(&mut self) {
        for id in self.used.drain(..) {
            self.slot[id as usize] = UNSEEN;
        }
    }
}

fn decode_material(
    bytes: &[u8],
    resolve: &mut impl FnMut(&str) -> Option<BlockId>,
) -> Option<Vec<u8>> {
    let mut r = ByteReader::new(bytes);
    let block = resolve(std::str::from_utf8(r.blob()?).ok()?)?;
    let mut state = Vec::new();
    for _ in 0..r.u32()? {
        let k = std::str::from_utf8(r.blob()?).ok()?;
        let v = std::str::from_utf8(r.blob()?).ok()?;
        state.push((k, v));
    }
    Some(AuthoredPalette::encode(block, state))
}

impl IndexedPlan {
    /// Reads the header of a plan [`encode`](Plan::encode)d at `bytes[start..]`; `None` when
    /// the bytes are not one.
    pub fn parse(bytes: Vec<u8>, start: usize) -> Option<IndexedPlan> {
        let mut r = ByteReader::new(bytes.get(start..)?);
        if r.u32()? != FORMAT {
            return None;
        }
        let claim = match r.u32()? {
            0 => None,
            _ => Some(read_mask(&mut r)?),
        };
        let palette = r.u32()? as usize;
        let mut materials = Vec::with_capacity(palette);
        for _ in 0..palette {
            materials.push(start + r.position());
            r.blob()?;
            for _ in 0..r.u32()? {
                r.blob()?;
                r.blob()?;
            }
        }
        let count = r.u32()? as usize;
        let mut sections = Vec::with_capacity(count);
        for _ in 0..count {
            let s = r.i32x3()?;
            let (offset, len) = (r.u32()? as usize, r.u32()? as usize);
            sections.push((s, offset, len));
        }
        let payload = start + r.position();
        for s in &mut sections {
            s.1 += payload;
            if s.1 + s.2 > bytes.len() {
                return None;
            }
        }
        Some(IndexedPlan {
            bytes,
            palette: Resolved {
                air: None,
                materials: vec![None; materials.len()],
                slot: vec![UNSEEN; materials.len()],
                used: Vec::new(),
            },
            materials,
            sections,
            claim,
        })
    }

    /// The columns the plan [claims](Plan::claim).
    pub fn claim(&self) -> Option<&ColumnMask> {
        self.claim.as_ref()
    }

    fn section(&self, section: [i32; 3]) -> Option<(usize, usize)> {
        let i = self
            .sections
            .binary_search_by(|(s, _, _)| s.cmp(&section))
            .ok()?;
        Some((self.sections[i].1, self.sections[i].2))
    }

    /// The sections the plan writes in.
    pub fn sections(&self) -> impl Iterator<Item = [i32; 3]> + '_ {
        self.sections.iter().map(|&(s, _, _)| s)
    }

    pub fn touches(&self, section: [i32; 3]) -> bool {
        self.section(section).is_some()
    }

    /// Boxes covering every section of `within` this plan writes nothing in.
    pub fn untouched(&self, within: SectionBox) -> Vec<SectionBox> {
        let mut out = Vec::new();
        if (0..3).any(|a| within.min[a] > within.max[a]) {
            return out;
        }
        let y_lo = self
            .sections
            .iter()
            .map(|(s, _, _)| s[1])
            .min()
            .unwrap_or(0);
        let y_hi = self
            .sections
            .iter()
            .map(|(s, _, _)| s[1])
            .max()
            .unwrap_or(0);
        let (w, d) = (
            (within.max[0] as i64 - within.min[0] as i64 + 1) as usize,
            (within.max[2] as i64 - within.min[2] as i64 + 1) as usize,
        );
        if y_hi - y_lo >= 64 || w.saturating_mul(d) > 1 << 16 {
            return Vec::new();
        }
        // The sections each column of `within` touches, as a bit per y above `y_lo`.
        let mut touched = vec![0u64; w * d];
        for (s, _, _) in &self.sections {
            if within.contains(*s) {
                let i = (s[0] - within.min[0]) as usize * d + (s[2] - within.min[2]) as usize;
                touched[i] |= 1 << (s[1] - y_lo);
            }
        }
        for (xi, column) in touched.chunks(d).enumerate() {
            let x = within.min[0] + xi as i32;
            let mut start = 0;
            while start < d {
                let mut end = start;
                while end + 1 < d && column[end + 1] == column[start] {
                    end += 1;
                }
                let z = [within.min[2] + start as i32, within.min[2] + end as i32];
                let mut run = |y0: i64, y1: i64| {
                    out.push(SectionBox {
                        min: [x, y0 as i32, z[0]],
                        max: [x, y1 as i32, z[1]],
                    })
                };
                let mut from = within.min[1] as i64;
                let mut bits = column[start];
                while bits != 0 {
                    let y = y_lo as i64 + bits.trailing_zeros() as i64;
                    bits &= bits - 1;
                    if y > from {
                        run(from, y - 1);
                    }
                    from = y + 1;
                }
                if from <= within.max[1] as i64 {
                    run(from, within.max[1] as i64);
                }
                start = end + 1;
            }
        }
        out
    }

    /// Appends this plan's share of `section` to `out`: box fills clearing the natural contents
    /// it cuts away, then authored cells for everything it builds (which the host applies after
    /// the fills). `resolve` maps registry names to this session's block ids; a name it cannot
    /// resolve is skipped.
    pub fn emit(
        &mut self,
        section: [i32; 3],
        resolve: &mut impl FnMut(&str) -> Option<BlockId>,
        out: &mut GenOutput,
    ) {
        let Some((offset, len)) = self.section(section) else {
            return;
        };
        let IndexedPlan {
            bytes,
            materials,
            palette,
            ..
        } = self;
        let mut r = ByteReader::new(&bytes[offset..offset + len]);
        emit_slice(&mut r, section, bytes, materials, palette, resolve, out);
        palette.reset();
    }
}

fn emit_slice(
    r: &mut ByteReader,
    section: [i32; 3],
    bytes: &[u8],
    offsets: &[usize],
    palette: &mut Resolved,
    resolve: &mut impl FnMut(&str) -> Option<BlockId>,
    out: &mut GenOutput,
) -> Option<()> {
    let origin = section.map(|v| v * 16);
    let at = |local: u16| at(origin, local);
    let mut emitted = [0u64; 64];
    let mut mark = |l: u16| emitted[l as usize / 64] |= 1 << (l % 64);
    let cells = r.u32()? as usize;
    out.authored.cells.reserve(cells);
    let records = r.take(cells.checked_mul(4)?)?.as_chunks::<4>().0;
    for &[l0, l1, id0, id1] in records {
        let (l, id) = (u16::from_le_bytes([l0, l1]), u16::from_le_bytes([id0, id1]));
        if let Some(index) = palette.slot(bytes, offsets, id, resolve, out) {
            out.authored.cells.push(at(l), index);
            mark(l);
        }
    }
    let anchors = r.u32()?;
    out.authored.cells.reserve(anchors as usize);
    for _ in 0..anchors {
        let (anchor, id) = (r.i32x3()?, r.u16()?);
        if let Some(index) = palette.slot(bytes, offsets, id, resolve, out) {
            out.authored.cells.push(anchor, index);
            if section_of(anchor) == section {
                mark(local(anchor));
            }
        }
    }
    for _ in 0..r.u32()? {
        let l = r.u16()?;
        let (k, v) = (r.blob()?, r.blob()?);
        if emitted[l as usize / 64] & (1 << (l % 64)) != 0 {
            out.authored.data.push(AuthoredData {
                pos: at(l),
                key: String::from_utf8(k.to_vec()).ok()?,
                value: v.to_vec(),
            });
        }
    }
    let boxes = r.u32()?;
    if boxes > 0 {
        let air = *palette.air.get_or_insert_with(|| resolve(AIR));
        let air = air?;
        out.fills.reserve(boxes as usize);
        let [ox, oy, oz] = origin;
        for &[x0, x1, z0, z1, y0, y1] in r.take(boxes as usize * 6)?.as_chunks::<6>().0 {
            out.fills.push(GenFill {
                min: [ox + x0 as i32, oy + y0 as i32, oz + z0 as i32],
                max: [ox + x1 as i32, oy + y1 as i32, oz + z1 as i32],
                block: air,
            });
        }
    }
    Some(())
}
