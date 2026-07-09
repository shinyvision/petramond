//! Off-thread section meshing.
//!
//! The render thread must never build a section mesh: doing it inline (a blocking
//! `rayon` batch) makes the frame stall while streaming. Instead the world hands each
//! dirty section to the shared [`JobPool`] as an owned snapshot — the section itself
//! plus a one-block-padded shell of its neighbours for voxel/light reads, plus the
//! wider XZ biome halo needed by tint blending — and drains back a finished
//! [`ChunkMesh`] to install on a later frame.
//! Each job carries the section's `mesh_revision`; a result whose section has since
//! changed (re-edited, re-lit) is discarded, so stale snapshots never reach the GPU.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use crate::block_state::{SlabState, StairState};
use crate::chunk::{SectionPos, SECTION_SIZE, SKY_FULL, WORLD_MIN_Y};
use crate::mesh::{build_section_mesh_from_pad, ChunkMesh, SectionMeshPad};
use crate::section::Section;
use crate::worker::JobPool;

/// Total worker nanoseconds and jobs spent building section meshes — temporary
/// perf-session diagnostics read by the out-of-tree streaming profiler.
pub(crate) static MESH_STAGE_NS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
pub(crate) static MESH_STAGE_JOBS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Padded neighbourhood side length: the section (16) plus one cell of border on each
/// face — all the mesher's face-culling / AO / smooth-light sampling ever reaches.
pub(super) const PAD: usize = SECTION_SIZE + 2;
pub(super) const PAD_VOL: usize = PAD * PAD * PAD;

/// Tint blending samples a 5×5 biome window, so an edge block needs two biome columns
/// past the section on both X/Z axes. Keep this separate from the voxel pad: culling,
/// AO, and smooth-light only need one block of 3D neighbour data.
pub(super) const BIOME_PAD_RADIUS: i32 = 2;
pub(super) const BIOME_PAD: usize = SECTION_SIZE + (BIOME_PAD_RADIUS as usize * 2);
pub(super) const BIOME_PAD_AREA: usize = BIOME_PAD * BIOME_PAD;

#[inline]
pub(super) fn pad_idx(x: usize, y: usize, z: usize) -> usize {
    (y * PAD + z) * PAD + x
}

/// Map a padded-axis coordinate `0..PAD` to `(neighbour delta −1/0/+1, section-local
/// 0..16)`: index 0 is the low neighbour's last cell, `1..PAD-1` is this section, and
/// `PAD-1` is the high neighbour's first cell.
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

/// Cheap field-`Arc` snapshot of one neighbour section's voxel buffers — all the mesher
/// reads of a neighbour (block ids for face culling, water/light for sampling). Cloning it
/// is four `Arc` refcount bumps and zero allocations, and it does NOT share the world's
/// `Arc<Section>`, so streaming edits never copy-on-write a section because a mesh job holds
/// it. Absent (`None`) buffers fall back exactly like [`Section`]'s accessors.
pub(super) struct NeighborSnap {
    pub blocks: std::sync::Arc<[u8]>,
    pub water: Option<std::sync::Arc<[u8]>>,
    pub skylight: Option<std::sync::Arc<[u8]>>,
    pub blocklight: Option<std::sync::Arc<[u8]>>,
    pub stair_states: Option<Box<[(u16, StairState)]>>,
    pub slab_states: Option<Box<[(u16, SlabState)]>>,
}

/// A self-contained meshing job: the 3×3×3 neighbourhood as cheap field-`Arc` snapshots
/// (indexed by [`nbhd_idx27`], centre at 13) plus an owned clone of the centre section (the
/// only one the mesher needs as a full `Section`, for its block-entity maps) and the small
/// per-column biome strip. Creating it is one `Section` clone + 27×4 `Arc` bumps, not 27
/// deep copies, and it shares no mutable world state. The worker assembles the padded mesh
/// buffers (the heavy part) off-thread in [`build`].
pub(super) struct MeshJob {
    pub pos: SectionPos,
    pub revision: u64,
    pub center: Section,
    pub nbhd: [Option<NeighborSnap>; 27],
    pub biome: Arc<[u8]>,
}

/// Index into a 3×3×3 section neighbourhood by neighbour delta (−1/0/+1 each axis); centre
/// `(0,0,0)` is 13.
#[inline]
pub(super) fn nbhd_idx27(dx: i32, dy: i32, dz: i32) -> usize {
    (((dy + 1) * 3 + (dz + 1)) * 3 + (dx + 1)) as usize
}

/// An empty per-column biome strip (`BIOME_PAD×BIOME_PAD` in XZ), filled by the caller.
pub(super) fn empty_biome() -> Arc<[u8]> {
    Arc::from(vec![0u8; BIOME_PAD_AREA].into_boxed_slice())
}

pub(super) struct MeshDone {
    pub pos: SectionPos,
    pub revision: u64,
    pub mesh: Option<ChunkMesh>,
    pub cancel: crate::worker::JobCancel,
}

/// Mesh-stage adapter over the shared [`JobPool`]: `submit` queues a snapshot build
/// at a distance priority, `try_recv` drains finished meshes on the main thread.
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
        let cancel = crate::worker::JobCancel::new();
        let job_cancel = cancel.clone();
        let pos = job.pos;
        let revision = job.revision;
        let tx = self.tx.clone();
        self.pool.submit(key, move || {
            let done = if job_cancel.is_cancelled() {
                MeshDone {
                    pos,
                    revision,
                    mesh: None,
                    cancel: job_cancel,
                }
            } else {
                build(job, job_cancel)
            };
            let _ = tx.send(done);
        });
        cancel
    }

    pub fn try_recv(&self) -> Option<MeshDone> {
        self.rx.lock().unwrap().try_recv().ok()
    }
}

/// The assembled one-cell-padded neighbourhood buffers a section mesh reads (18³ each):
/// block ids, water/light state, per-cell stair facing, and a loaded flag. Reads beyond
/// the pad fall back exactly as the live world's accessors do (air / open sky / not-loaded).
struct Pad {
    blocks: Box<[u8]>,
    water: Box<[u8]>,
    skylight: Box<[u8]>,
    blocklight: Box<[u8]>,
    stair_states: Box<[u8]>,
    slab_states: Box<[SlabState]>,
    loaded: Box<[bool]>,
}

impl Pad {
    fn new() -> Self {
        Self {
            blocks: vec![0u8; PAD_VOL].into_boxed_slice(),
            water: vec![0u8; PAD_VOL].into_boxed_slice(),
            skylight: vec![SKY_FULL; PAD_VOL].into_boxed_slice(),
            blocklight: vec![0u8; PAD_VOL].into_boxed_slice(),
            stair_states: vec![StairState::default().encode(); PAD_VOL].into_boxed_slice(),
            slab_states: vec![SlabState::EMPTY; PAD_VOL].into_boxed_slice(),
            loaded: vec![false; PAD_VOL].into_boxed_slice(),
        }
    }

    /// Restore the freshly-allocated defaults (air / no water / full sky / no block
    /// light / north stairs / not loaded) so a reused pad assembles byte-identically.
    fn reset(&mut self) {
        self.blocks.fill(0);
        self.water.fill(0);
        self.skylight.fill(SKY_FULL);
        self.blocklight.fill(0);
        self.stair_states.fill(StairState::default().encode());
        self.slab_states.fill(SlabState::EMPTY);
        self.loaded.fill(false);
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
/// instead of all 18³ cells. Stair states (rare) are scattered per bearing neighbour after.
fn assemble_pad(pos: SectionPos, nbhd: &[Option<NeighborSnap>; 27], pad: &mut Pad) {
    let (_ox, oy, _oz) = pos.origin_world();
    pad.reset();
    let Pad {
        blocks,
        water,
        skylight,
        blocklight,
        stair_states,
        slab_states,
        loaded,
    } = pad;

    // Interior X run of every row: cells px=1..=16 all come from the centre-X neighbour
    // (dx=0), so one neighbour lookup + one slice copy per (py,pz) fills 16 cells. The two
    // X-border cells (px=0 from dx=-1, px=17 from dx=+1) are handled per-cell below.
    for pz in 0..PAD {
        let (ddz, lz) = pad_axis(pz);
        for py in 0..PAD {
            let (ddy, ly) = pad_axis(py);
            let base = pad_idx(1, py, pz); // px=1
            let src = crate::chunk::section_idx(0, ly, lz); // local x=0 of this row
            match nbhd[nbhd_idx27(0, ddy, ddz)].as_ref() {
                Some(s) => {
                    blocks[base..base + SECTION_SIZE]
                        .copy_from_slice(&s.blocks[src..src + SECTION_SIZE]);
                    if let Some(w) = s.water.as_ref() {
                        water[base..base + SECTION_SIZE]
                            .copy_from_slice(&w[src..src + SECTION_SIZE]);
                    }
                    // skylight buffer starts full sky, so a `None` (uncomputed) neighbour
                    // correctly leaves the run at SKY_FULL — only copy a computed cube.
                    if let Some(sk) = s.skylight.as_ref() {
                        skylight[base..base + SECTION_SIZE]
                            .copy_from_slice(&sk[src..src + SECTION_SIZE]);
                    }
                    if let Some(bl) = s.blocklight.as_ref() {
                        blocklight[base..base + SECTION_SIZE]
                            .copy_from_slice(&bl[src..src + SECTION_SIZE]);
                    }
                    loaded[base..base + SECTION_SIZE].fill(true);
                }
                None => {
                    // Absent neighbour: air / no light / not loaded (buffer defaults),
                    // and dark below the world floor (above stays the SKY_FULL default).
                    let wy = oy - 1 + py as i32;
                    if wy < WORLD_MIN_Y {
                        skylight[base..base + SECTION_SIZE].fill(0);
                    }
                }
            }
        }
    }

    // The two X-border planes: px=0 (from the dx=-1 neighbour, its local x=15) and
    // px=PAD-1 (dx=+1, local x=0). One cell of each row, per-cell like the old assembler.
    for &(px, ddx, lx) in &[(0usize, -1i32, SECTION_SIZE - 1), (PAD - 1, 1i32, 0usize)] {
        for pz in 0..PAD {
            let (ddz, lz) = pad_axis(pz);
            for py in 0..PAD {
                let (ddy, ly) = pad_axis(py);
                let pi = pad_idx(px, py, pz);
                let li = crate::chunk::section_idx(lx, ly, lz);
                match nbhd[nbhd_idx27(ddx, ddy, ddz)].as_ref() {
                    Some(s) => {
                        blocks[pi] = s.blocks[li];
                        water[pi] = s.water.as_ref().map_or(0, |w| w[li]);
                        skylight[pi] = s.skylight.as_ref().map_or(SKY_FULL, |s| s[li]);
                        blocklight[pi] = s.blocklight.as_ref().map_or(0, |b| b[li]);
                        loaded[pi] = true;
                    }
                    None => {
                        let wy = oy - 1 + py as i32;
                        skylight[pi] = if wy >= WORLD_MIN_Y { SKY_FULL } else { 0 };
                    }
                }
            }
        }
    }

    // Sparse per-cell shape states (stairs, slabs) are rare (most sections carry none,
    // so most neighbours skip entirely). Scatter each bearing neighbour's entries into
    // the pad, mapping local coords to a pad index only when the cell lies inside it.
    for dy in -1i32..=1 {
        for dz in -1i32..=1 {
            for dx in -1i32..=1 {
                let Some(s) = nbhd[nbhd_idx27(dx, dy, dz)].as_ref() else {
                    continue;
                };
                if let Some(states) = s.stair_states.as_ref() {
                    scatter_border_states(states, (dx, dy, dz), |i, state| {
                        stair_states[i] = state.encode();
                    });
                }
                if let Some(states) = s.slab_states.as_ref() {
                    scatter_border_states(states, (dx, dy, dz), |i, state| slab_states[i] = state);
                }
            }
        }
    }
}

/// Scatter one neighbour's sparse per-cell states into the pad via `write(pad_idx,
/// state)`, skipping cells that lie outside this section's one-cell border ring.
fn scatter_border_states<T: Copy>(
    states: &[(u16, T)],
    (dx, dy, dz): (i32, i32, i32),
    mut write: impl FnMut(usize, T),
) {
    for &(key, state) in states {
        let (lx, ly, lz) = crate::chunk::section_local(key as usize);
        let (Some(px), Some(py), Some(pz)) =
            (pad_border(dx, lx), pad_border(dy, ly), pad_border(dz, lz))
        else {
            continue;
        };
        write(pad_idx(px, py, pz), state);
    }
}

/// Pad coordinate a neighbour cell at local `c` (0..16) maps to for neighbour delta `d`,
/// or `None` when that cell lies outside this section's one-cell pad (a `d=±1` neighbour
/// only contributes its single face plane).
#[inline]
fn pad_border(d: i32, c: usize) -> Option<usize> {
    match d {
        0 => Some(c + 1),
        -1 if c == SECTION_SIZE - 1 => Some(0),
        1 if c == 0 => Some(PAD - 1),
        _ => None,
    }
}

/// Build one section mesh from its owned snapshot.
fn build(job: MeshJob, cancel: crate::worker::JobCancel) -> MeshDone {
    let t_stage = std::time::Instant::now();
    let MeshJob {
        pos,
        revision,
        center,
        nbhd,
        biome,
    } = job;

    let mesh = PAD_SCRATCH.with(|pad| {
        let mut pad = pad.borrow_mut();
        assemble_pad(pos, &nbhd, &mut pad);
        if cancel.is_cancelled() {
            return None;
        }
        Some(build_section_mesh_from_pad(
            &center,
            pos,
            SectionMeshPad {
                blocks: &pad.blocks,
                water: &pad.water,
                skylight: &pad.skylight,
                blocklight: &pad.blocklight,
                stair_states: &pad.stair_states,
                slab_states: &pad.slab_states,
                loaded: &pad.loaded,
                biome: &biome,
            },
        ))
    });
    if mesh.is_some() {
        MESH_STAGE_NS.fetch_add(
            t_stage.elapsed().as_nanos() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
        MESH_STAGE_JOBS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    MeshDone {
        pos,
        revision,
        mesh,
        cancel,
    }
}
