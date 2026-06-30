//! Off-thread section meshing.
//!
//! The render thread must never build a section mesh: doing it inline (a blocking
//! `rayon` batch) makes the frame stall while streaming. Instead the world hands each
//! dirty section to this pool as an owned snapshot — the section itself plus a
//! one-block-padded shell of its neighbours for voxel/light reads, plus the wider
//! XZ biome halo needed by tint blending — and the pool returns a finished [`ChunkMesh`]
//! the world installs on a later frame.
//! Each job carries the section's `mesh_revision`; a result whose section has since
//! changed (re-edited, re-lit) is discarded, so stale snapshots never reach the GPU.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::chunk::{SectionPos, SECTION_SIZE, SKY_FULL, WORLD_MAX_Y, WORLD_MIN_Y};
use crate::mesh::{build_section_mesh, ChunkMesh};
use crate::section::Section;

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
    pub biome: Box<[u8]>,
}

/// Index into a 3×3×3 section neighbourhood by neighbour delta (−1/0/+1 each axis); centre
/// `(0,0,0)` is 13.
#[inline]
pub(super) fn nbhd_idx27(dx: i32, dy: i32, dz: i32) -> usize {
    (((dy + 1) * 3 + (dz + 1)) * 3 + (dx + 1)) as usize
}

/// An empty per-column biome strip (`BIOME_PAD×BIOME_PAD` in XZ), filled by the caller.
pub(super) fn empty_biome() -> Box<[u8]> {
    vec![0u8; BIOME_PAD_AREA].into_boxed_slice()
}

pub(super) struct MeshDone {
    pub pos: SectionPos,
    pub revision: u64,
    pub mesh: ChunkMesh,
}

pub(super) struct MeshPool {
    tx: Sender<MeshJob>,
    rx: Mutex<Receiver<MeshDone>>,
    _handles: Vec<thread::JoinHandle<()>>,
}

impl MeshPool {
    pub fn new(threads: usize) -> Self {
        let (tx, rx_job) = channel::<MeshJob>();
        let (tx_done, rx) = channel::<MeshDone>();
        let rx_job = Arc::new(Mutex::new(rx_job));
        let mut handles = Vec::with_capacity(threads);
        for _ in 0..threads.max(1) {
            let rx_job = rx_job.clone();
            let tx_done = tx_done.clone();
            let h = thread::Builder::new()
                .name("llamacraft-mesh".to_string())
                .spawn(move || loop {
                    let job = {
                        let g = rx_job.lock().unwrap();
                        g.recv()
                    };
                    match job {
                        Ok(job) => {
                            let t = std::time::Instant::now();
                            let done = build(job);
                            crate::perf::MESH.record(t.elapsed().as_nanos() as u64);
                            if tx_done.send(done).is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                })
                .expect("spawn mesh worker");
            handles.push(h);
        }
        Self {
            tx,
            rx: Mutex::new(rx),
            _handles: handles,
        }
    }

    pub fn submit(&self, job: MeshJob) -> bool {
        self.tx.send(job).is_ok()
    }

    pub fn try_recv(&self) -> Option<MeshDone> {
        self.rx.lock().unwrap().try_recv().ok()
    }
}

/// Build one section mesh from its owned snapshot. The neighbour closures read the
/// padded buffers (covering this section and its one-cell shell); reads beyond the pad
/// fall back exactly as the live world's accessors do (air / open sky / not-loaded).
fn build(job: MeshJob) -> MeshDone {
    let MeshJob {
        pos,
        revision,
        center,
        nbhd,
        biome,
    } = job;

    // Assemble the padded neighbourhood buffers here, off the render thread, from the cheap
    // field-Arc snapshots the request took. Reads match the live neighbour accessors exactly
    // (air / open-sky / not-loaded fallbacks), so the off-thread mesh is byte-identical to
    // an inline one.
    let (ox, oy, oz) = pos.origin_world();
    let mut blocks = vec![0u8; PAD_VOL].into_boxed_slice();
    let mut water = vec![0u8; PAD_VOL].into_boxed_slice();
    let mut skylight = vec![SKY_FULL; PAD_VOL].into_boxed_slice();
    let mut blocklight = vec![0u8; PAD_VOL].into_boxed_slice();
    let mut loaded = vec![false; PAD_VOL].into_boxed_slice();
    for pz in 0..PAD {
        let (ddz, lz) = pad_axis(pz);
        for py in 0..PAD {
            let (ddy, ly) = pad_axis(py);
            let wy = oy - 1 + py as i32;
            for px in 0..PAD {
                let (ddx, lx) = pad_axis(px);
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
                        // Absent neighbour: air, no block light, not loaded; skylight reads
                        // open sky in/above the world and dark below it.
                        skylight[pi] = if wy >= WORLD_MIN_Y { SKY_FULL } else { 0 };
                    }
                }
            }
        }
    }
    let center = &center;
    let (bx, by, bz) = (ox - 1, oy - 1, oz - 1);
    let pad = |wx: i32, wy: i32, wz: i32| -> Option<usize> {
        let (px, py, pz) = (wx - bx, wy - by, wz - bz);
        let n = PAD as i32;
        if (0..n).contains(&px) && (0..n).contains(&py) && (0..n).contains(&pz) {
            Some(pad_idx(px as usize, py as usize, pz as usize))
        } else {
            None
        }
    };
    let nb_block = |wx, wy, wz| pad(wx, wy, wz).map(|i| blocks[i]).unwrap_or(0);
    let nb_water = |wx, wy, wz| pad(wx, wy, wz).map(|i| water[i]).unwrap_or(0);
    let nb_skylight = |wx, wy, wz| {
        if wy >= WORLD_MAX_Y {
            return SKY_FULL;
        }
        if wy < WORLD_MIN_Y {
            return 0;
        }
        pad(wx, wy, wz).map(|i| skylight[i]).unwrap_or(SKY_FULL)
    };
    let nb_blocklight = |wx, wy, wz| pad(wx, wy, wz).map(|i| blocklight[i]).unwrap_or(0);
    let nb_loaded = |wx, wy, wz| pad(wx, wy, wz).map(|i| loaded[i]).unwrap_or(false);
    let (biome_bx, biome_bz) = (ox - BIOME_PAD_RADIUS, oz - BIOME_PAD_RADIUS);
    let nb_biome = |wx: i32, wz: i32| {
        let (px, pz) = (wx - biome_bx, wz - biome_bz);
        let n = BIOME_PAD as i32;
        if (0..n).contains(&px) && (0..n).contains(&pz) {
            biome[biome_pad_idx(px as usize, pz as usize)]
        } else {
            0
        }
    };
    let mesh = build_section_mesh(
        center,
        pos,
        nb_block,
        nb_water,
        nb_biome,
        nb_skylight,
        nb_blocklight,
        nb_loaded,
    );
    MeshDone {
        pos,
        revision,
        mesh,
    }
}
