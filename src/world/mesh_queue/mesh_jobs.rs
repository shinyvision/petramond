use std::sync::Arc;

use crate::chunk::{self, ChunkPos, SectionPos};
use crate::world::store::World;

use super::{RESULT_DRAIN_MIN, RESULT_DRAIN_TIME_BUDGET};

impl World {
    /// Install meshes the pool finished, dropping any whose section has since changed
    /// (re-edited or re-lit, so its `mesh_revision` moved) or unloaded.
    pub(super) fn drain_finished_meshes(&mut self) {
        let start = std::time::Instant::now();
        let mut drained = 0usize;
        while drained < RESULT_DRAIN_MIN || start.elapsed() < RESULT_DRAIN_TIME_BUDGET {
            let Some(done) = self.mesh_pool.try_recv() else {
                break;
            };
            drained += 1;
            self.mesh_jobs_in_flight = self.mesh_jobs_in_flight.saturating_sub(1);
            if self
                .mesh_job_cancels
                .get(&done.pos)
                .is_some_and(|current| current.same_job(&done.cancel))
            {
                self.mesh_job_cancels.remove(&done.pos);
            }
            let Some(mut mesh) = done.mesh else {
                continue;
            };
            let fresh = self
                .sections
                .get(&done.pos)
                .is_some_and(|s| s.mesh_revision == done.revision);
            if !fresh {
                continue;
            }
            mesh.mesh_dirty = true; // needs a GPU upload on the next sync
            self.install_mesh(done.pos, mesh);
            if let Some(s) = self.section_mut(done.pos) {
                s.dirty = false;
            }
        }
    }

    /// Snapshot `pos` and its one-block-padded neighbourhood into an owned [`MeshJob`]
    /// the mesh pool can build with no access to the live world. Reads match the live
    /// neighbour accessors exactly (air / open-sky / not-loaded fallbacks), so the
    /// off-thread mesh is byte-identical to an inline one.
    pub(in crate::world) fn build_mesh_job(&self, pos: SectionPos) -> Option<crate::world::mesh_pool::MeshJob> {
        use crate::world::mesh_pool::{
            biome_pad_idx, empty_biome, nbhd_idx27, MeshJob, NeighborSnap, BIOME_PAD,
            BIOME_PAD_RADIUS,
        };

        let center = (**self.sections.get(&pos)?).clone();
        let revision = center.mesh_revision;

        // Snapshot the 3×3×3 neighbourhood as cheap field-Arc bundles: four refcount bumps
        // each, no allocation, and no shared `Arc<Section>` — so a streaming edit/relight
        // never copy-on-write clones a section just because a mesh job is reading it. The
        // worker assembles the padded mesh buffers from these off-thread.
        let mut nbhd: [Option<NeighborSnap>; 27] = std::array::from_fn(|_| None);
        for dy in -1..=1 {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    nbhd[nbhd_idx27(dx, dy, dz)] = self
                        .sections
                        .get(&SectionPos::new(pos.cx + dx, pos.cy + dy, pos.cz + dz))
                        .map(|s| NeighborSnap {
                            blocks: s.blocks_arc(),
                            water: s.water_arc(),
                            skylight: s.skylight_arc(),
                            blocklight: s.blocklight_arc(),
                            stair_states: sparse_state_snapshot(s.stair_states()),
                            slab_states: sparse_state_snapshot(s.slab_states()),
                        });
                }
            }
        }

        // Every live column carries the complete tint halo, captured by the
        // column-generation worker or replicated by the server. Hand-built test
        // worlds fall back to loaded column facts only; live submission never runs
        // analytical worldgen on this thread.
        let biome = self
            .column_gen
            .get(&pos.chunk_pos())
            .map(|col| col.mesh_biome())
            .or_else(|| self.column_biome_halos.get(&pos.chunk_pos()).cloned())
            .unwrap_or_else(|| {
                let mut halo = empty_biome();
                let data = Arc::make_mut(&mut halo);
                let (ox, _, oz) = pos.origin_world();
                for pz in 0..BIOME_PAD {
                    let wz = oz - BIOME_PAD_RADIUS + pz as i32;
                    for px in 0..BIOME_PAD {
                        let wx = ox - BIOME_PAD_RADIUS + px as i32;
                        if let Some(col) = self.columns.get(&ChunkPos::new(
                            wx.div_euclid(chunk::SECTION_SIZE as i32),
                            wz.div_euclid(chunk::SECTION_SIZE as i32),
                        )) {
                            data[biome_pad_idx(px, pz)] =
                                col.biome_at(chunk::lx(wx), chunk::lz(wz));
                        }
                    }
                }
                halo
            });

        Some(MeshJob {
            pos,
            revision,
            center,
            nbhd,
            biome,
        })
    }
}

/// Owned copy of a section's sparse per-cell state map for a mesh job, `None`
/// when the section carries none (the common case — no allocation).
fn sparse_state_snapshot<T: Copy>(
    map: &std::collections::HashMap<u16, T>,
) -> Option<Box<[(u16, T)]>> {
    (!map.is_empty()).then(|| map.iter().map(|(&key, &state)| (key, state)).collect())
}
