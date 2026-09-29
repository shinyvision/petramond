//! Off-thread section meshing.
//!
//! Ordinary streaming must never build a section mesh on the render thread; doing it inline makes
//! flight stall. World hands each dirty section to the shared [`JobPool`] as an owned snapshot:
//! the section plus a one-block-padded shell of neighbours for voxel/light reads, and the wider XZ
//! biome halo for tint blending. Result comes back later as a [`ChunkMesh`]. Initial local
//! prediction is the one exception, runs the same builder synchronously for latency.
//! Each job carries `mesh_revision`; if the section changed since (re-edited, re-lit) the result
//! gets dropped instead of reaching the GPU.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use crate::worker::JobPool;
use petramond_mesh::{ChunkMesh, SectionMeshPad, SectionVisibility};
use petramond_world::chunk::{SectionPos, SECTION_SIZE, SKY_FULL, WORLD_MIN_Y};
use petramond_world::section::Section;

pub(super) const PAD: usize = SECTION_SIZE + 2;
pub(super) const PAD_VOL: usize = PAD * PAD * PAD;

pub(super) const BIOME_PAD_RADIUS: i32 = 2;
pub(super) const BIOME_PAD: usize = SECTION_SIZE + (BIOME_PAD_RADIUS as usize * 2);
pub(super) const BIOME_PAD_AREA: usize = BIOME_PAD * BIOME_PAD;

#[inline]
pub(super) fn pad_idx(x: usize, y: usize, z: usize) -> usize {
    (y * PAD + z) * PAD + x
}

#[inline]
pub(super) fn pad_axis(p: usize) -> (i32, usize) {
    if p == 0 {
        (-1, SECTION_SIZE - 1)
    } else if p == PAD - 1 {
        (1, 0)
    } else {
        (0, p - 1)
    }
}

#[inline]
pub(super) fn biome_pad_idx(x: usize, z: usize) -> usize {
    z * BIOME_PAD + x
}

pub(super) struct NeighborSnap {
    pub blocks: petramond_world::section::BlockCube,
    pub fluid: Option<std::sync::Arc<[u8]>>,
    pub skylight: Option<std::sync::Arc<[u8]>>,
    pub blocklight: Option<std::sync::Arc<[petramond_world::light::LightRgb]>>,
    pub cell_states: Option<Box<[(u16, petramond_world::block::ShapeState)]>>,
    pub transition_tints: Box<[(u16, bool)]>,
    /// Whether the section MAY hold a snow cover or snow-bedded block (a superset test off its
    /// id set), which is the only thing the pad's transition exclusion pass looks for.
    pub blanket: bool,
}

/// The ids of every snow cover / snow-bedded block.
pub(super) static BLANKET_IDS: petramond_world::content::Slot<petramond_world::section::IdSet> =
    petramond_world::content::Slot::new(
        "snow blanket ids",
        &[petramond_world::content::stage::BLOCK_VIEWS],
        |_| {
            Ok(petramond_world::section::IdSet::from_ids(
                petramond_world::block::Block::all()
                    .iter()
                    .filter(|b| b.is_snow_cover() || b.is_snow_bedded())
                    .map(|b| b.id()),
            ))
        },
    );

/// Meshing job: 3x3x3 neighbourhood as cheap Arc snapshots (indexed by [`nbhd_idx27`], centre at
/// 13), plus the centre section's own `Arc` (the mesher needs the full `Section` for its
/// block-entity and cell-state maps; sharing the handle costs a refcount bump, and a write to
/// the section while the job is in flight copies it on write instead of every job deep-copying
/// its state maps up front), plus small per-column biome strip. No mutable world state touched.
/// The padded mesh buffers, the heavy part, get built off-thread in [`build`].
pub(super) struct MeshJob {
    pub pos: SectionPos,
    pub revision: u64,
    pub center: Arc<Section>,
    pub nbhd: [Option<NeighborSnap>; 27],
    pub biome: Arc<[u8]>,
}

impl MeshJob {
    pub(super) fn replace_light_snapshot(
        &mut self,
        pos: SectionPos,
        skylight: Arc<[u8]>,
        blocklight: Arc<[petramond_world::light::LightRgb]>,
    ) {
        let (dx, dy, dz) = (
            pos.cx - self.pos.cx,
            pos.cy - self.pos.cy,
            pos.cz - self.pos.cz,
        );
        if !(-1..=1).contains(&dx) || !(-1..=1).contains(&dy) || !(-1..=1).contains(&dz) {
            return;
        }
        if let Some(section) = self.nbhd[nbhd_idx27(dx, dy, dz)].as_mut() {
            section.skylight = Some(skylight);
            section.blocklight = Some(blocklight);
        }
    }
}

#[inline]
pub(super) fn nbhd_idx27(dx: i32, dy: i32, dz: i32) -> usize {
    (((dy + 1) * 3 + (dz + 1)) * 3 + (dx + 1)) as usize
}

pub(super) fn empty_biome() -> Arc<[u8]> {
    Arc::from(vec![0u8; BIOME_PAD_AREA].into_boxed_slice())
}

pub(super) enum MeshOutcome {
    Built(Box<ChunkMesh>),
    Cancelled,
    Failed,
}

pub(super) struct MeshDone {
    pub pos: SectionPos,
    pub revision: u64,
    pub outcome: MeshOutcome,
    pub cancel: crate::worker::JobCancel,
}

pub(super) struct MeshPool {
    pool: Arc<JobPool>,
    tx: Sender<MeshDone>,
    rx: Mutex<Receiver<MeshDone>>,
}

impl MeshPool {
    pub fn new(pool: Arc<JobPool>) -> Self {
        let (tx, rx) = channel::<MeshDone>();
        Self {
            pool,
            tx,
            rx: Mutex::new(rx),
        }
    }

    pub fn submit(&self, key: i64, job: MeshJob) -> crate::worker::JobCancel {
        let (pos, revision) = (job.pos, job.revision);
        self.submit_build(key, pos, revision, move |cancel| build(job, cancel))
    }

    pub(super) fn submit_build(
        &self,
        key: i64,
        pos: SectionPos,
        revision: u64,
        build: impl FnOnce(&crate::worker::JobCancel) -> Option<ChunkMesh> + Send + 'static,
    ) -> crate::worker::JobCancel {
        let cancel = crate::worker::JobCancel::new();
        let job_cancel = cancel.clone();
        let failed = MeshDone {
            pos,
            revision,
            outcome: MeshOutcome::Failed,
            cancel: cancel.clone(),
        };
        let slot = crate::worker::ReportSlot::new(self.tx.clone(), failed, "mesh", pos);
        self.pool.submit(key, move || {
            let outcome = if job_cancel.is_cancelled() {
                MeshOutcome::Cancelled
            } else {
                build(&job_cancel).map_or(MeshOutcome::Cancelled, |mesh| {
                    MeshOutcome::Built(Box::new(mesh))
                })
            };
            slot.complete(MeshDone {
                pos,
                revision,
                outcome,
                cancel: job_cancel,
            });
        });
        cancel
    }

    pub fn try_recv(&self) -> Option<MeshDone> {
        self.rx.lock().unwrap().try_recv().ok()
    }
}

pub(super) fn build_inline(job: MeshJob) -> Option<ChunkMesh> {
    build(job, &crate::worker::JobCancel::new())
}

impl crate::world::ReplicaWorld {
    #[cfg(any(test, feature = "test-support"))]
    pub fn mesh_section_inline(&self, pos: SectionPos) -> Option<ChunkMesh> {
        build_inline(self.build_mesh_job(pos)?)
    }
}

struct Pad {
    blocks: Box<[u16]>,
    fluid: Box<[u8]>,
    skylight: Box<[u8]>,
    blocklight: Box<[petramond_world::light::LightRgb]>,
    cell_states: Box<[petramond_world::block::ShapeState]>,
    loaded: Box<[bool]>,
    transition_blocked: Box<[bool]>,
    /// The sparse fields' cells the last assembly wrote: the next one clears exactly those
    /// instead of the whole pad. The dense fields are written whole, row by row.
    written_states: Vec<usize>,
    written_blocked: Vec<usize>,
}

impl Pad {
    fn new() -> Self {
        Self {
            blocks: vec![0u16; PAD_VOL].into_boxed_slice(),
            fluid: vec![0u8; PAD_VOL].into_boxed_slice(),
            skylight: vec![SKY_FULL; PAD_VOL].into_boxed_slice(),
            blocklight: vec![petramond_world::light::LightRgb::ZERO; PAD_VOL].into_boxed_slice(),
            cell_states: vec![petramond_world::block::ShapeState::NONE; PAD_VOL].into_boxed_slice(),
            loaded: vec![false; PAD_VOL].into_boxed_slice(),
            transition_blocked: vec![false; PAD_VOL].into_boxed_slice(),
            written_states: Vec::new(),
            written_blocked: Vec::new(),
        }
    }

    fn clear_sparse(&mut self) {
        for i in self.written_states.drain(..) {
            self.cell_states[i] = petramond_world::block::ShapeState::NONE;
        }
        for i in self.written_blocked.drain(..) {
            self.transition_blocked[i] = false;
        }
    }
}

thread_local! {
    /// Reusable per-mesh-thread pad (~35 KB of buffers): streaming meshes thousands of
    /// sections, so assembling into a reused pad keeps the six per-job neighbourhood
    /// boxes off the allocator. Reset before each assemble; the built `ChunkMesh`
    /// output is allocated fresh since it outlives the job.
    static PAD_SCRATCH: std::cell::RefCell<Pad> = std::cell::RefCell::new(Pad::new());
}

/// Assemble the 18³ padded neighbourhood from the cheap field-`Arc` snapshots the request
/// took — off the render thread. Reads match the live neighbour accessors exactly (air /
/// open-sky / not-loaded fallbacks), so the off-thread mesh is byte-identical to an inline one.
///
/// Filled a row at a time along X: the 16-wide interior run of each row comes from ONE
/// neighbour (the centre-X section) and is a contiguous slice copy, not 16 per-cell
/// neighbour lookups; only the two X-border cells fall to per-cell handling. That keeps the
/// per-cell `pad_axis`/`nbhd_idx27`/`Option` decode to the two edges plus once per row,
/// instead of all 18³ cells. Every dense field is written exactly once per cell (a present
/// neighbour's row or the absent-neighbour default), so nothing is cleared up front; the two
/// sparse fields clear only the cells the previous assembly wrote. Stair states (rare) are
/// scattered per bearing neighbour after.
fn assemble_pad(pos: SectionPos, nbhd: &[Option<NeighborSnap>; 27], pad: &mut Pad) {
    let (_ox, oy, _oz) = pos.origin_world();
    pad.clear_sparse();
    let Pad {
        blocks,
        fluid,
        skylight,
        blocklight,
        cell_states,
        loaded,
        transition_blocked,
        written_states,
        written_blocked,
    } = pad;

    for pz in 0..PAD {
        let (ddz, lz) = pad_axis(pz);
        for py in 0..PAD {
            let (ddy, ly) = pad_axis(py);
            let base = pad_idx(1, py, pz);
            let row = base..base + SECTION_SIZE;
            let src = petramond_world::chunk::section_idx(0, ly, lz);
            match nbhd[nbhd_idx27(0, ddy, ddz)].as_ref() {
                Some(s) => {
                    s.blocks.expand_row_into(src, &mut blocks[row.clone()]);
                    match s.fluid.as_ref() {
                        Some(w) => fluid[row.clone()].copy_from_slice(&w[src..src + SECTION_SIZE]),
                        None => fluid[row.clone()].fill(0),
                    }
                    match s.skylight.as_ref() {
                        Some(sk) => {
                            skylight[row.clone()].copy_from_slice(&sk[src..src + SECTION_SIZE])
                        }
                        None => skylight[row.clone()].fill(SKY_FULL),
                    }
                    match s.blocklight.as_ref() {
                        Some(bl) => {
                            blocklight[row.clone()].copy_from_slice(&bl[src..src + SECTION_SIZE])
                        }
                        None => {
                            blocklight[row.clone()].fill(petramond_world::light::LightRgb::ZERO)
                        }
                    }
                    loaded[row].fill(true);
                }
                None => {
                    let wy = oy - 1 + py as i32;
                    blocks[row.clone()].fill(0);
                    fluid[row.clone()].fill(0);
                    skylight[row.clone()].fill(if wy >= WORLD_MIN_Y { SKY_FULL } else { 0 });
                    blocklight[row.clone()].fill(petramond_world::light::LightRgb::ZERO);
                    loaded[row].fill(false);
                }
            }
        }
    }

    for &(px, ddx, lx) in &[(0usize, -1i32, SECTION_SIZE - 1), (PAD - 1, 1i32, 0usize)] {
        for pz in 0..PAD {
            let (ddz, lz) = pad_axis(pz);
            for py in 0..PAD {
                let (ddy, ly) = pad_axis(py);
                let pi = pad_idx(px, py, pz);
                let li = petramond_world::chunk::section_idx(lx, ly, lz);
                match nbhd[nbhd_idx27(ddx, ddy, ddz)].as_ref() {
                    Some(s) => {
                        blocks[pi] = s.blocks.get(li);
                        fluid[pi] = s.fluid.as_ref().map_or(0, |w| w[li]);
                        skylight[pi] = s.skylight.as_ref().map_or(SKY_FULL, |s| s[li]);
                        blocklight[pi] = s
                            .blocklight
                            .as_ref()
                            .map_or(petramond_world::light::LightRgb::ZERO, |b| b[li]);
                        loaded[pi] = true;
                    }
                    None => {
                        let wy = oy - 1 + py as i32;
                        blocks[pi] = 0;
                        fluid[pi] = 0;
                        skylight[pi] = if wy >= WORLD_MIN_Y { SKY_FULL } else { 0 };
                        blocklight[pi] = petramond_world::light::LightRgb::ZERO;
                        loaded[pi] = false;
                    }
                }
            }
        }
    }

    // Stairs/slabs states are rare, most neighbours have none. Skip those, scatter the rest into
    // the pad, keep only cells that land inside it.
    for dy in -1i32..=1 {
        for dz in -1i32..=1 {
            for dx in -1i32..=1 {
                let Some(s) = nbhd[nbhd_idx27(dx, dy, dz)].as_ref() else {
                    continue;
                };
                scatter_border_states(&s.transition_tints, (dx, dy, dz), |i, _| {
                    transition_blocked[i] = true;
                    written_blocked.push(i);
                });
                if let Some(states) = s.cell_states.as_ref() {
                    scatter_border_states(states, (dx, dy, dz), |i, state| {
                        cell_states[i] = state;
                        written_states.push(i);
                    });
                }
            }
        }
    }
    let rules = petramond_world::texture_transition::rules();
    let section = glam::IVec3::splat(SECTION_SIZE as i32);
    let pad_side = glam::IVec3::splat(PAD as i32);
    let pad_index = |p: glam::IVec3| -> Option<usize> {
        let q = p + glam::IVec3::ONE;
        (q.cmpge(glam::IVec3::ZERO).all() && q.cmplt(pad_side).all())
            .then(|| pad_idx(q.x as usize, q.y as usize, q.z as usize))
    };
    let table = petramond_world::block::BlockTable::current();
    let block_at = |p: glam::IVec3| -> Option<petramond_world::block::Block> {
        if let Some(i) = pad_index(p) {
            return loaded[i].then(|| table.block(blocks[i]));
        }
        let d = p.div_euclid(section);
        let l = p.rem_euclid(section);
        nbhd[nbhd_idx27(d.x, d.y, d.z)].as_ref().map(|s| {
            table.block(s.blocks.get(petramond_world::chunk::section_idx(
                l.x as usize,
                l.y as usize,
                l.z as usize,
            )))
        })
    };
    let cover_step = glam::IVec3::new(0, petramond_world::block::SNOW_COVER_REACH, 0);
    let mut exclude_below = |cover: glam::IVec3| {
        if let Some(i) = pad_index(cover - cover_step) {
            if rules.is_material(blocks[i]) {
                transition_blocked[i] = true;
                written_blocked.push(i);
            }
        }
    };
    use petramond_world::block::BlockTag;
    let blanket = |id: u16| {
        table.has_tag(id, BlockTag::SNOW_COVER) || table.has_tag(id, BlockTag::SNOW_BEDDED)
    };
    // The exclusion pass only ever marks a cell under an unloaded cell or under a blanket; with
    // every neighbour present and none that may hold a blanket, it would walk 19×18×18 cells
    // to mark nothing. (Every pad cell and the layer above it come from the 27 neighbours.)
    let nothing_to_exclude = nbhd.iter().all(|s| s.as_ref().is_some_and(|s| !s.blanket));
    if nothing_to_exclude {
        return;
    }
    for py in 0..=PAD {
        for pz in 0..PAD {
            for px in 0..PAD {
                let p = glam::IVec3::new(px as i32 - 1, py as i32 - 1, pz as i32 - 1);
                let id = if py < PAD {
                    let i = pad_idx(px, py, pz);
                    loaded[i].then_some(blocks[i])
                } else {
                    block_at(p).map(|b| b.id())
                };
                let Some(id) = id else {
                    exclude_below(p);
                    continue;
                };
                if blanket(id)
                    && petramond_world::block::snow_cover_at(p, |q| {
                        block_at(q).unwrap_or(petramond_world::block::Block::Air)
                    })
                    .is_some()
                {
                    exclude_below(p);
                }
            }
        }
    }
}

fn scatter_border_states<T: Copy>(
    states: &[(u16, T)],
    (dx, dy, dz): (i32, i32, i32),
    mut write: impl FnMut(usize, T),
) {
    for &(key, state) in states {
        let (lx, ly, lz) = petramond_world::chunk::section_local(key as usize);
        let (Some(px), Some(py), Some(pz)) =
            (pad_border(dx, lx), pad_border(dy, ly), pad_border(dz, lz))
        else {
            continue;
        };
        write(pad_idx(px, py, pz), state);
    }
}

#[inline]
fn pad_border(d: i32, c: usize) -> Option<usize> {
    match d {
        0 => Some(c + 1),
        -1 if c == SECTION_SIZE - 1 => Some(0),
        1 if c == 0 => Some(PAD - 1),
        _ => None,
    }
}

fn build(job: MeshJob, cancel: &crate::worker::JobCancel) -> Option<ChunkMesh> {
    let MeshJob {
        pos,
        center,
        nbhd,
        biome,
        ..
    } = job;

    let mesh = PAD_SCRATCH.with(|pad| {
        let mut pad = pad.borrow_mut();
        assemble_pad(pos, &nbhd, &mut pad);
        if cancel.is_cancelled() {
            return None;
        }
        petramond_mesh::build_section_mesh_cancellable(
            &center,
            pos,
            SectionMeshPad {
                table: petramond_world::block::BlockTable::current(),
                blocks: &pad.blocks,
                fluid: &pad.fluid,
                skylight: &pad.skylight,
                blocklight: &pad.blocklight,
                cell_states: &pad.cell_states,
                loaded: &pad.loaded,
                transition_blocked: &pad.transition_blocked,
                biome: &biome,
            },
            petramond_mesh::MeshContext::global(),
            &|| cancel.is_cancelled(),
        )
    });
    mesh.map(|mut mesh| {
        mesh.visibility = SectionVisibility::of_section(&center);
        mesh.into_sealed()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drain_one(pool: &MeshPool) -> MeshDone {
        let deadline = std::time::Instant::now() + petramond_util::test_time::TEST_HARD_DEADLINE;
        loop {
            if let Some(done) = pool.try_recv() {
                return done;
            }
            assert!(std::time::Instant::now() < deadline, "no mesh result");
            std::thread::yield_now();
        }
    }

    #[test]
    fn a_panicking_build_reports_failed_instead_of_leaking_its_slot() {
        for threads in [0, 1] {
            let pool = MeshPool::new(Arc::new(JobPool::new(threads)));
            let pos = SectionPos::new(3, 1, -2);
            pool.submit_build(0, pos, 9, |_| panic!("injected mesher panic"));
            let done = drain_one(&pool);
            assert_eq!((done.pos, done.revision), (pos, 9));
            assert!(matches!(done.outcome, MeshOutcome::Failed));
        }
    }

    #[test]
    fn a_cancelled_build_reports_cancelled() {
        let pool = MeshPool::new(Arc::new(JobPool::inline()));
        let pos = SectionPos::new(0, 0, 0);
        pool.submit_build(0, pos, 1, |cancel| {
            cancel.cancel();
            None
        });
        assert!(matches!(drain_one(&pool).outcome, MeshOutcome::Cancelled));
    }
}
