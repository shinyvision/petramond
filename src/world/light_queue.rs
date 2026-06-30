use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Arc;

use crate::block::Block;
use crate::chunk::{section_idx, ChunkPos, SectionPos, SECTION_SIZE, SECTION_VOLUME, SKY_FULL};
use crate::column::Column;
use crate::mathh::IVec3;
use crate::section::Section;

pub(super) struct LightBakeQueue {
    backend: Backend,
    pending: HashMap<SectionPos, PendingLightBake>,
    next_id: u64,
}

#[derive(Copy, Clone, Debug)]
struct PendingLightBake {
    id: u64,
}

struct LightBakeJob {
    id: u64,
    pos: SectionPos,
    revision: u64,
    /// How this section's skylight resolves, decided from the column heightmaps. The
    /// common cases (a section wholly above all cover, or wholly out of seep range below
    /// it) carry no buffers; only a surface-straddling section floods.
    sky: SkyPlan,
    /// Cheap shared handles to the block buffers of the 3×3×3 section neighbourhood (centre
    /// at array index 13), shared by the skylight and block-light floods. `None` when
    /// neither floods — sky is Full/Dark and no emitter is in range. The render thread only
    /// clones 27 `Arc`s here; the actual `NBHD³` buffer is assembled off-thread in the bake.
    nbhd: Option<NbhdArcs>,
    /// Block-light emitter world positions across the neighbourhood; empty ⇒ an all-dark
    /// block-light cube.
    emitters: Vec<IVec3>,
}

/// Shared block buffers of a section's 3×3×3 neighbourhood, indexed by [`nbhd_arc_idx`].
/// `None` for an absent (unloaded) neighbour, which reads as air.
type NbhdArcs = [Option<Arc<[u8]>>; 27];

#[inline]
fn nbhd_arc_idx(dcx: i32, dcy: i32, dcz: i32) -> usize {
    (((dcy + 1) * 3 + (dcz + 1)) * 3 + (dcx + 1)) as usize
}

/// How a section's skylight resolves, decided cheaply from the 3×3 column heightmaps
/// before any block buffer is assembled.
enum SkyPlan {
    /// Every cell sits above all surrounding cover: full daylight, no flood.
    Full,
    /// Every cell sits deeper than skylight can seep below the lowest cover: dark, no flood.
    Dark,
    /// The section straddles the surface band: flood from the open-sky cells. Carries the
    /// 3×3 column heightmap grid (`NBHD×NBHD` in XZ) that seeds the flood.
    Flood { surface: Box<[i32]> },
}

/// Side length of the light flood neighbourhood (3 sections).
const NBHD: usize = 3 * SECTION_SIZE;
const NBHD_VOLUME: usize = NBHD * NBHD * NBHD;
const NBHD_AREA: usize = NBHD * NBHD;

/// How far (in cells) skylight reaches from an open-sky cell before it is fully dark:
/// `SKY_FULL` divided by the 2-per-cell falloff. A section more than this far below the
/// lowest surrounding cover can hold no skylight, so it never needs the flood.
const SKY_SEEP_REACH: i32 = (SKY_FULL / 2) as i32;

/// Heightmap stand-in for a neighbour column that is not loaded: treated as fully covered
/// so it seeds no phantom skylight into the centre. The loaded edge re-bakes once the
/// neighbour streams in (poll marks the new section's neighbourhood light-dirty).
const COVERED: i32 = i32::MAX;

#[inline]
fn nbhd_idx(x: usize, y: usize, z: usize) -> usize {
    (y * NBHD + z) * NBHD + x
}

pub(super) struct LightBakeResult {
    id: u64,
    pub pos: SectionPos,
    pub revision: u64,
    /// Full 16³ skylight cube (x2 scale). `Arc` so the drain installs it into the section
    /// with no copy (the section stores light behind an `Arc`).
    pub skylight: Arc<[u8]>,
    /// Full 16³ block-light cube (x2 scale).
    pub blocklight: Arc<[u8]>,
}

impl LightBakeQueue {
    pub fn new() -> Self {
        Self {
            backend: Backend::new(),
            pending: HashMap::new(),
            next_id: 1,
        }
    }

    pub fn request(
        &mut self,
        pos: SectionPos,
        sections: &HashMap<SectionPos, Arc<Section>>,
        columns: &HashMap<ChunkPos, Column>,
    ) {
        if self.pending.contains_key(&pos) {
            return;
        }
        let Some(section) = sections.get(&pos) else {
            self.pending.remove(&pos);
            return;
        };
        if !section.light_dirty {
            self.pending.remove(&pos);
            return;
        }

        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        let revision = section.light_revision;

        let sky = plan_skylight(pos, columns);
        let emitters = collect_nbhd_emitters(pos, sections);
        // Take the shared block neighbourhood only if a flood will actually read it. This is
        // 27 cheap `Arc` clones on the render thread; the heavy `NBHD³` assembly happens
        // off-thread in `run_light_bake`.
        let nbhd = (matches!(sky, SkyPlan::Flood { .. }) || !emitters.is_empty())
            .then(|| gather_nbhd_arcs(pos, sections));

        let job = LightBakeJob {
            id,
            pos,
            revision,
            sky,
            nbhd,
            emitters,
        };
        self.pending.insert(pos, PendingLightBake { id });
        self.backend.submit(job);
    }

    pub fn cancel(&mut self, pos: SectionPos) {
        self.pending.remove(&pos);
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub fn try_recv(&mut self) -> Option<LightBakeResult> {
        while let Some(res) = self.backend.try_recv() {
            if self.pending.get(&res.pos).is_some_and(|p| p.id == res.id) {
                self.pending.remove(&res.pos);
                return Some(res);
            }
        }
        None
    }
}

/// Decide how `pos`'s skylight resolves from the 3×3 column heightmaps alone: a section
/// wholly above the tallest surrounding cover is full daylight; one wholly deeper than
/// [`SKY_SEEP_REACH`] below the lowest cover is dark; anything straddling the surface band
/// floods. The cheap min/max gate runs first so the common Full/Dark cases never allocate
/// the seed grid — only a flooding section builds it.
fn plan_skylight(pos: SectionPos, columns: &HashMap<ChunkPos, Column>) -> SkyPlan {
    let (hmin, hmax) = sky_cover_range(pos, columns);
    let oy = pos.origin_world().1;
    let top = oy + SECTION_SIZE as i32 - 1;
    if oy > hmax {
        SkyPlan::Full
    } else if top < hmin + 1 - SKY_SEEP_REACH {
        SkyPlan::Dark
    } else {
        SkyPlan::Flood {
            surface: gather_sky_surface(pos, columns),
        }
    }
}

/// The min/max surface (cover) height over the LOADED columns of `pos`'s 3×3 neighbourhood
/// — the seed range [`plan_skylight`] gates on. Falls back to open sky when (impossibly) no
/// column is loaded, so nothing is spuriously darkened.
fn sky_cover_range(pos: SectionPos, columns: &HashMap<ChunkPos, Column>) -> (i32, i32) {
    let (mut hmin, mut hmax) = (i32::MAX, i32::MIN);
    for dcz in -1..=1 {
        for dcx in -1..=1 {
            let cp = ChunkPos::new(pos.cx + dcx, pos.cz + dcz);
            if let Some(col) = columns.get(&cp) {
                for &h in col.heightmap_slice() {
                    hmin = hmin.min(h);
                    hmax = hmax.max(h);
                }
            }
        }
    }
    if hmin == i32::MAX {
        (crate::column::NO_SURFACE, crate::column::NO_SURFACE)
    } else {
        (hmin, hmax)
    }
}

/// Gather the 3×3 neighbour columns' surface heightmaps into one `NBHD×NBHD` (XZ) grid that
/// seeds [`flood_skylight`]. A missing neighbour column reads as [`COVERED`] so it seeds no
/// phantom skylight into the centre.
fn gather_sky_surface(pos: SectionPos, columns: &HashMap<ChunkPos, Column>) -> Box<[i32]> {
    let mut surface = vec![COVERED; NBHD_AREA].into_boxed_slice();
    for dcz in -1..=1 {
        for dcx in -1..=1 {
            let cp = ChunkPos::new(pos.cx + dcx, pos.cz + dcz);
            let Some(col) = columns.get(&cp) else {
                continue;
            };
            let hm = col.heightmap_slice();
            let bx = ((dcx + 1) as usize) * SECTION_SIZE;
            let bz = ((dcz + 1) as usize) * SECTION_SIZE;
            for lz in 0..SECTION_SIZE {
                for lx in 0..SECTION_SIZE {
                    surface[(bz + lz) * NBHD + (bx + lx)] = hm[lz * SECTION_SIZE + lx];
                }
            }
        }
    }
    surface
}

/// Take cheap shared `Arc` clones of the block buffers of `pos`'s 3×3×3 section
/// neighbourhood (centre at array index 13); absent neighbours stay `None` (read as air).
/// Runs on the render thread — 27 atomic refcount bumps, no copy. The `NBHD³` buffer is
/// assembled from these off-thread by [`assemble_nbhd_blocks`].
fn gather_nbhd_arcs(pos: SectionPos, sections: &HashMap<SectionPos, Arc<Section>>) -> NbhdArcs {
    let mut arcs: NbhdArcs = std::array::from_fn(|_| None);
    for dcy in -1..=1 {
        for dcz in -1..=1 {
            for dcx in -1..=1 {
                let npos = SectionPos::new(pos.cx + dcx, pos.cy + dcy, pos.cz + dcz);
                if let Some(section) = sections.get(&npos) {
                    arcs[nbhd_arc_idx(dcx, dcy, dcz)] = Some(section.blocks_arc());
                }
            }
        }
    }
    arcs
}

/// Assemble the `NBHD³` block-id buffer from the neighbourhood's shared block `Arc`s (centre
/// at local `[16,32)`); absent neighbours read as air (transparent). Shared by both floods.
/// Runs off the render thread, inside the light bake.
fn assemble_nbhd_blocks(nbhd: &NbhdArcs) -> Box<[u8]> {
    let mut blocks = vec![0u8; NBHD_VOLUME].into_boxed_slice();
    for dcy in -1..=1 {
        for dcz in -1..=1 {
            for dcx in -1..=1 {
                let Some(src) = &nbhd[nbhd_arc_idx(dcx, dcy, dcz)] else {
                    continue;
                };
                let bx = ((dcx + 1) as usize) * SECTION_SIZE;
                let by = ((dcy + 1) as usize) * SECTION_SIZE;
                let bz = ((dcz + 1) as usize) * SECTION_SIZE;
                for ly in 0..SECTION_SIZE {
                    for lz in 0..SECTION_SIZE {
                        for lx in 0..SECTION_SIZE {
                            blocks[nbhd_idx(bx + lx, by + ly, bz + lz)] =
                                src[section_idx(lx, ly, lz)];
                        }
                    }
                }
            }
        }
    }
    blocks
}

/// Collect every block-light emitter (torches + lit furnaces) in `pos`'s 3×3×3 section
/// neighbourhood, in world coords. Empty ⇒ the section bakes an all-dark block-light cube.
fn collect_nbhd_emitters(
    pos: SectionPos,
    sections: &HashMap<SectionPos, Arc<Section>>,
) -> Vec<IVec3> {
    let mut emitters = Vec::new();
    for dcy in -1..=1 {
        for dcz in -1..=1 {
            for dcx in -1..=1 {
                let npos = SectionPos::new(pos.cx + dcx, pos.cy + dcy, pos.cz + dcz);
                if let Some(section) = sections.get(&npos) {
                    collect_section_emitters(npos, section, &mut emitters);
                }
            }
        }
    }
    emitters
}

/// Append one section's emitters (torches + lit furnaces) in world coords.
fn collect_section_emitters(pos: SectionPos, section: &Section, out: &mut Vec<IVec3>) {
    let (ox, oy, oz) = pos.origin_world();
    let world_of = |key: u16| {
        IVec3::new(
            ox + (key & 0x0F) as i32,
            oy + (key >> 8) as i32,
            oz + ((key >> 4) & 0x0F) as i32,
        )
    };
    out.extend(section.torches().keys().map(|&k| world_of(k)));
    out.extend(
        section
            .furnaces()
            .iter()
            .filter(|(_, f)| f.is_lit())
            .map(|(&k, _)| world_of(k)),
    );
}

/// Light value an emitter seeds at its own cell (x2 scale). One torch ≈ level 14.
const EMITTER_LIGHT: u8 = 28;

fn run_light_bake(job: LightBakeJob) -> LightBakeResult {
    let LightBakeJob {
        id,
        pos,
        revision,
        sky,
        nbhd,
        emitters,
    } = job;

    // Assemble the shared NBHD³ block buffer here, off the render thread, from the cheap
    // Arc clones the request took.
    let nbhd_blocks = nbhd.as_ref().map(assemble_nbhd_blocks);

    let skylight: Arc<[u8]> = match sky {
        SkyPlan::Full => vec![SKY_FULL; SECTION_VOLUME].into(),
        SkyPlan::Dark => vec![0u8; SECTION_VOLUME].into(),
        SkyPlan::Flood { surface } => flood_skylight(
            pos,
            nbhd_blocks
                .as_deref()
                .expect("a flooding skylight bake carries its neighbourhood blocks"),
            &surface,
        ),
    };

    // Block-light: a cross-section BFS flood over the 3×3×3 neighbourhood from every
    // emitter in range, clipped back to this section. No emitter ⇒ dark.
    let blocklight: Arc<[u8]> = if emitters.is_empty() {
        vec![0u8; SECTION_VOLUME].into()
    } else {
        flood_block_light(
            pos,
            nbhd_blocks
                .as_deref()
                .expect("a block-light flood bake carries its neighbourhood blocks"),
            &emitters,
        )
    };

    LightBakeResult {
        id,
        pos,
        revision,
        skylight,
        blocklight,
    }
}

/// Flood skylight across the 3×3×3 section neighbourhood, then clip to the centre. Every
/// open-sky cell (strictly above its column's surface height) seeds full daylight; light
/// then spreads to non-opaque neighbours losing two (one level) per cell — straight down
/// a shaft and sideways into the shadow under an overhang, a canopy, or a placed block,
/// so a single covering block no longer blacks out the column beneath it. Stateless and
/// order-independent (multi-source BFS at uniform cost); seep reach (< one section) makes
/// the neighbourhood flood identical to a global one for the centre's cells.
fn flood_skylight(pos: SectionPos, blocks: &[u8], surface: &[i32]) -> Arc<[u8]> {
    let noy = pos.origin_world().1 - SECTION_SIZE as i32;
    let opaque =
        |x: usize, y: usize, z: usize| Block::from_id(blocks[nbhd_idx(x, y, z)]).is_opaque();

    let mut light = vec![0u8; NBHD_VOLUME].into_boxed_slice();
    let mut queue: VecDeque<(usize, usize, usize)> = VecDeque::new();

    // Seed every open-sky cell: world Y strictly above its column's surface height.
    for y in 0..NBHD {
        let wy = noy + y as i32;
        for z in 0..NBHD {
            for x in 0..NBHD {
                if wy > surface[z * NBHD + x] {
                    let i = nbhd_idx(x, y, z);
                    light[i] = SKY_FULL;
                    queue.push_back((x, y, z));
                }
            }
        }
    }

    while let Some((x, y, z)) = queue.pop_front() {
        let level = light[nbhd_idx(x, y, z)];
        if level <= 2 {
            continue;
        }
        let next = level - 2;
        let mut step =
            |nx: usize, ny: usize, nz: usize, q: &mut VecDeque<(usize, usize, usize)>| {
                if opaque(nx, ny, nz) {
                    return;
                }
                let ni = nbhd_idx(nx, ny, nz);
                if light[ni] < next {
                    light[ni] = next;
                    q.push_back((nx, ny, nz));
                }
            };
        if x + 1 < NBHD {
            step(x + 1, y, z, &mut queue);
        }
        if x > 0 {
            step(x - 1, y, z, &mut queue);
        }
        if y + 1 < NBHD {
            step(x, y + 1, z, &mut queue);
        }
        if y > 0 {
            step(x, y - 1, z, &mut queue);
        }
        if z + 1 < NBHD {
            step(x, y, z + 1, &mut queue);
        }
        if z > 0 {
            step(x, y, z - 1, &mut queue);
        }
    }

    clip_center(&light)
}

/// Flood block light across the 3×3×3 section neighbourhood from every emitter, then clip
/// to the centre. Because an emitter's reach (< one section) can only cross into the
/// centre from an adjacent section, this neighbourhood flood is byte-identical to a global
/// flood for the centre's cells — and fully stateless (no convergence pass).
fn flood_block_light(pos: SectionPos, blocks: &[u8], emitters: &[IVec3]) -> Arc<[u8]> {
    let opaque =
        |x: usize, y: usize, z: usize| Block::from_id(blocks[nbhd_idx(x, y, z)]).is_opaque();

    // Neighbourhood origin (world) = centre section origin minus one section.
    let (cox, coy, coz) = pos.origin_world();
    let (nox, noy, noz) = (
        cox - SECTION_SIZE as i32,
        coy - SECTION_SIZE as i32,
        coz - SECTION_SIZE as i32,
    );
    let n = NBHD as i32;

    let mut light = vec![0u8; NBHD_VOLUME].into_boxed_slice();
    let mut queue: VecDeque<(usize, usize, usize)> = VecDeque::new();
    for e in emitters {
        let (x, y, z) = (e.x - nox, e.y - noy, e.z - noz);
        if !(0..n).contains(&x) || !(0..n).contains(&y) || !(0..n).contains(&z) {
            continue;
        }
        let (x, y, z) = (x as usize, y as usize, z as usize);
        let i = nbhd_idx(x, y, z);
        if light[i] < EMITTER_LIGHT {
            light[i] = EMITTER_LIGHT;
            queue.push_back((x, y, z));
        }
    }
    while let Some((x, y, z)) = queue.pop_front() {
        let level = light[nbhd_idx(x, y, z)];
        if level <= 2 {
            continue;
        }
        let next = level - 2;
        let mut step =
            |nx: usize, ny: usize, nz: usize, q: &mut VecDeque<(usize, usize, usize)>| {
                if opaque(nx, ny, nz) {
                    return;
                }
                let ni = nbhd_idx(nx, ny, nz);
                if light[ni] < next {
                    light[ni] = next;
                    q.push_back((nx, ny, nz));
                }
            };
        if x + 1 < NBHD {
            step(x + 1, y, z, &mut queue);
        }
        if x > 0 {
            step(x - 1, y, z, &mut queue);
        }
        if y + 1 < NBHD {
            step(x, y + 1, z, &mut queue);
        }
        if y > 0 {
            step(x, y - 1, z, &mut queue);
        }
        if z + 1 < NBHD {
            step(x, y, z + 1, &mut queue);
        }
        if z > 0 {
            step(x, y, z - 1, &mut queue);
        }
    }

    clip_center(&light)
}

/// Clip the centre section `[16,32)³` out of an `NBHD³` flood buffer into a 16³ cube.
fn clip_center(light: &[u8]) -> Arc<[u8]> {
    let mut out = vec![0u8; SECTION_VOLUME];
    for ly in 0..SECTION_SIZE {
        for lz in 0..SECTION_SIZE {
            for lx in 0..SECTION_SIZE {
                out[section_idx(lx, ly, lz)] =
                    light[nbhd_idx(lx + SECTION_SIZE, ly + SECTION_SIZE, lz + SECTION_SIZE)];
            }
        }
    }
    out.into()
}

struct Backend {
    tx_req: std::sync::mpsc::Sender<LightBakeJob>,
    rx_res: std::sync::mpsc::Receiver<LightBakeResult>,
    _handles: Vec<std::thread::JoinHandle<()>>,
}

impl Backend {
    fn new() -> Self {
        let (tx_req, rx_req) = std::sync::mpsc::channel::<LightBakeJob>();
        let (tx_res, rx_res) = std::sync::mpsc::channel::<LightBakeResult>();

        let rx_req = std::sync::Arc::new(std::sync::Mutex::new(rx_req));
        // Light bakes are quick; take only a small share of cores so the gen and mesh
        // pools and the render thread aren't starved (see `worker::background_thread_counts`).
        let (_, n, _) = crate::worker::background_thread_counts();
        let mut handles = Vec::with_capacity(n);
        for _ in 0..n {
            let rx_req = rx_req.clone();
            let tx_res = tx_res.clone();
            let h = std::thread::Builder::new()
                .name("llamacraft-light".to_string())
                .spawn(move || loop {
                    let job = {
                        let g = rx_req.lock().unwrap();
                        g.recv()
                    };
                    match job {
                        Ok(job) => {
                            let res = run_light_bake(job);
                            if tx_res.send(res).is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                })
                .expect("spawn light worker");
            handles.push(h);
        }
        Self {
            tx_req,
            rx_res,
            _handles: handles,
        }
    }

    fn submit(&self, job: LightBakeJob) {
        let _ = self.tx_req.send(job);
    }

    fn try_recv(&mut self) -> Option<LightBakeResult> {
        self.rx_res.try_recv().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A torch sitting in the section just across a seam lights the cells on the other
    /// side: the flood crosses the section boundary (block light used to stop dead at the
    /// section edge).
    #[test]
    fn block_light_floods_across_a_section_seam() {
        let pos = SectionPos::new(0, 0, 0); // centre section spans world [0,16)
                                            // Torch one cell west of the centre section's NegX face — i.e. in the −X neighbour.
        let emitter = IVec3::new(-1, 8, 8);
        let blocks = vec![0u8; NBHD_VOLUME].into_boxed_slice(); // all air

        let cube = flood_block_light(pos, &blocks, &[emitter]);

        // The centre cell touching that seam is one flood step from the torch.
        assert_eq!(
            cube[section_idx(0, 8, 8)],
            EMITTER_LIGHT - 2,
            "the seam cell is lit one step down from the emitter, across the section border"
        );
        // Falls off with distance and is dark deep inside the section (out of reach).
        assert!(cube[section_idx(4, 8, 8)] < cube[section_idx(0, 8, 8)]);
        assert_eq!(cube[section_idx(15, 8, 8)], 0);
    }

    /// An opaque wall on the seam blocks the cross-section flood.
    #[test]
    fn opaque_seam_blocks_the_cross_section_flood() {
        let pos = SectionPos::new(0, 0, 0);
        let emitter = IVec3::new(-1, 8, 8);
        let mut blocks = vec![0u8; NBHD_VOLUME].into_boxed_slice();
        // Fill the centre section's NegX face plane (neighbourhood x == SECTION_SIZE) with
        // stone, walling the torch out.
        for ly in 0..SECTION_SIZE {
            for lz in 0..SECTION_SIZE {
                blocks[nbhd_idx(SECTION_SIZE, ly + SECTION_SIZE, lz + SECTION_SIZE)] =
                    Block::Stone.id();
            }
        }

        let cube = flood_block_light(pos, &blocks, &[emitter]);

        // The walled seam cell is opaque (no light), and nothing behind it is lit.
        assert_eq!(cube[section_idx(0, 8, 8)], 0);
        assert_eq!(cube[section_idx(1, 8, 8)], 0);
    }

    /// The skylight regression: a single covering block (a raised column surface) must not
    /// black out the air beneath it — skylight seeps in horizontally from the open columns
    /// one cell away. This is the bug the binary heightmap model produced (everything at or
    /// below the raised surface went instantly dark).
    #[test]
    fn skylight_seeps_under_a_single_covering_block() {
        let pos = SectionPos::new(0, 0, 0); // centre world [0,16)
        let blocks = vec![0u8; NBHD_VOLUME].into_boxed_slice(); // all air
                                                                // Every column is open to the sky (surface far below the section) except one, raised
                                                                // to a covering height above the whole section, as if a single block were placed
                                                                // high over it.
        let mut surface = vec![-100i32; NBHD_AREA].into_boxed_slice();
        let (gx, gz) = (8 + SECTION_SIZE, 8 + SECTION_SIZE); // centre-local (8,8)
        surface[gz * NBHD + gx] = 40;

        let cube = flood_skylight(pos, &blocks, &surface);

        // Directly under the cover (centre-local 8,8 world y 8) the cell is not black: light
        // seeped in from the open columns one step away.
        assert!(
            cube[section_idx(8, 8, 8)] > 0,
            "a single covering block must not black out the column below it"
        );
        // Its open-sky neighbour one cell over is full daylight.
        assert_eq!(cube[section_idx(7, 8, 8)], SKY_FULL);
    }

    /// The flood is not indiscriminate: a section fully under cover with no open-sky cell in
    /// range stays dark (an enclosed space needs torches), so the seep can't manufacture
    /// light where none reaches.
    #[test]
    fn skylight_stays_dark_under_full_cover() {
        let pos = SectionPos::new(0, 0, 0);
        let blocks = vec![0u8; NBHD_VOLUME].into_boxed_slice();
        // The whole neighbourhood is covered above the section: no open-sky seed exists.
        let surface = vec![40i32; NBHD_AREA].into_boxed_slice();

        let cube = flood_skylight(pos, &blocks, &surface);

        assert!(
            cube.iter().all(|&l| l == 0),
            "a fully covered section holds no skylight"
        );
    }
}
