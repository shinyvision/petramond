use crate::world::ReplicaWorld;
use std::sync::Arc;

use petramond_world::chunk::{self, ChunkPos, SectionPos};
use petramond_world::section::Section;

use super::{RESULT_DRAIN_MIN, RESULT_DRAIN_TIME_BUDGET};

impl ReplicaWorld {
    pub(super) fn drain_finished_meshes(&mut self) {
        let start = std::time::Instant::now();
        let mut drained = 0usize;
        while drained < RESULT_DRAIN_MIN || start.elapsed() < RESULT_DRAIN_TIME_BUDGET {
            let Some(done) = self.side.terrain.mesh_pool.try_recv() else {
                break;
            };
            drained += 1;
            self.side.terrain.mesh_jobs_in_flight =
                self.side.terrain.mesh_jobs_in_flight.saturating_sub(1);
            if self
                .side
                .terrain
                .mesh_job_cancels
                .get(&done.pos)
                .is_some_and(|current| current.same_job(&done.cancel))
            {
                self.side.terrain.mesh_job_cancels.remove(&done.pos);
            }
            let mut mesh = match done.outcome {
                crate::world::mesh_pool::MeshOutcome::Built(mesh) => *mesh,
                crate::world::mesh_pool::MeshOutcome::Cancelled => continue,
                crate::world::mesh_pool::MeshOutcome::Failed => {
                    let current = self
                        .data
                        .sections
                        .get(&done.pos)
                        .is_some_and(|s| s.mesh_revision == done.revision);
                    if current {
                        if let Some(s) = self.data.section_mut(done.pos) {
                            s.dirty = false;
                        }
                    }
                    continue;
                }
            };
            let fresh = self
                .data
                .sections
                .get(&done.pos)
                .is_some_and(|s| s.mesh_revision == done.revision);
            if !fresh {
                continue;
            }
            mesh.mesh_dirty = true;
            self.side.terrain.install_mesh(done.pos, mesh);
            if let Some(s) = self.data.section_mut(done.pos) {
                s.dirty = false;
            }
        }
    }

    /// The 3x3x3 section handles around `pos` (centre at `nbhd_idx27(0, 0, 0)`): one column
    /// probe per XZ column and a slot read per section, shared by every scheduling test and the
    /// job snapshot of one candidate instead of each walking the map on its own.
    pub(in crate::world) fn gather_mesh_neighbourhood(&self, pos: SectionPos) -> MeshNbhd {
        use crate::world::mesh_pool::nbhd_idx27;
        let mut out: MeshNbhd = std::array::from_fn(|_| None);
        for dz in -1..=1 {
            for dx in -1..=1 {
                let Some(column) = self
                    .data
                    .sections
                    .column(ChunkPos::new(pos.cx + dx, pos.cz + dz))
                else {
                    continue;
                };
                for dy in -1..=1 {
                    out[nbhd_idx27(dx, dy, dz)] = column.at(pos.cy + dy).cloned();
                }
            }
        }
        out
    }

    pub(in crate::world) fn build_mesh_job(
        &self,
        pos: SectionPos,
    ) -> Option<crate::world::mesh_pool::MeshJob> {
        let nbhd = self.gather_mesh_neighbourhood(pos);
        self.build_mesh_job_from(pos, &nbhd)
    }

    pub(in crate::world) fn build_mesh_job_from(
        &self,
        pos: SectionPos,
        sections: &MeshNbhd,
    ) -> Option<crate::world::mesh_pool::MeshJob> {
        use crate::world::mesh_pool::{
            biome_pad_idx, empty_biome, nbhd_idx27, MeshJob, NeighborSnap, BIOME_PAD,
            BIOME_PAD_RADIUS,
        };

        let center = Arc::clone(sections[nbhd_idx27(0, 0, 0)].as_ref()?);
        let revision = center.mesh_revision;

        let blanket_ids = crate::world::mesh_pool::BLANKET_IDS.current();
        let mut nbhd: [Option<NeighborSnap>; 27] = std::array::from_fn(|_| None);
        for dy in -1..=1 {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    nbhd[nbhd_idx27(dx, dy, dz)] = sections[nbhd_idx27(dx, dy, dz)]
                        .as_ref()
                        .map(|s| NeighborSnap {
                            blocks: s.block_cube(),
                            fluid: s.fluid_arc(),
                            skylight: s.skylight_arc(),
                            blocklight: s.blocklight_arc(),
                            cell_states: sparse_state_snapshot(s.cell_states()),
                            transition_tints: s
                                .cell_tint_map()
                                .into_keys()
                                .map(|key| (key, true))
                                .collect(),
                            blanket: s.may_contain(blanket_ids),
                        })
                        .or_else(|| {
                            let n = SectionPos::new(pos.cx + dx, pos.cy + dy, pos.cz + dz);
                            (self.data.stream_writable(n)
                                && self.data.section_summary(n)
                                    == petramond_world::section::SectionSummary::Empty)
                                .then(|| NeighborSnap {
                                    blocks: petramond_world::section::BlockCube::uniform(0),
                                    fluid: None,
                                    skylight: None,
                                    blocklight: None,
                                    cell_states: None,
                                    transition_tints: Box::new([]),
                                    blanket: false,
                                })
                        });
                }
            }
        }

        let biome = self
            .data
            .column_biome_halos
            .get(&pos.chunk_pos())
            .cloned()
            .unwrap_or_else(|| {
                let mut halo = empty_biome();
                let data = Arc::make_mut(&mut halo);
                let (ox, _, oz) = pos.origin_world();
                for pz in 0..BIOME_PAD {
                    let wz = oz - BIOME_PAD_RADIUS + pz as i32;
                    for px in 0..BIOME_PAD {
                        let wx = ox - BIOME_PAD_RADIUS + px as i32;
                        if let Some(col) = self.data.columns.get(&ChunkPos::new(
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

/// The 27 section handles a mesh candidate's checks and snapshot share.
pub(in crate::world) type MeshNbhd = [Option<Arc<Section>>; 27];

fn sparse_state_snapshot<T: Copy>(
    map: &petramond_world::section::CellMap<T>,
) -> Option<Box<[(u16, T)]>> {
    (!map.is_empty()).then(|| map.iter().map(|(&key, &state)| (key, state)).collect())
}
