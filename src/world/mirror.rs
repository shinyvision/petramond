//! In-process replication harness: ships a [`ServerWorld`]'s terrain into a
//! [`ReplicaWorld`] through the same payload and delta seams a connection
//! uses, for tools and tests that want the server's generation AND the
//! replica's meshes in one process without running a session.
//!
//! This is what replaced the old single "combined" world role: a harness
//! that composes both sides, so neither side ever carries the other's state.

use crate::world::{ReplicaWorld, ServerWorld};
use rustc_hash::{FxHashMap, FxHashSet};

use petramond_world::chunk::{ChunkPos, SectionPos};


/// What has been shipped so far: per-column payload revision and the set of
/// installed sections. Columns ship before their sections, sections ship once
/// their light is final (the same gate the terrain sender applies), and every
/// later change rides the server's replication log.
#[derive(Default)]
pub struct ReplicaMirror {
    columns: FxHashMap<ChunkPos, u64>,
    sections: FxHashSet<SectionPos>,
}

impl ReplicaMirror {
    /// A mirror over `server`, whose replication capture it turns on (the
    /// per-tick delta log is how later edits reach the replica).
    pub fn new(server: &mut ServerWorld) -> Self {
        server.set_replication_capture(true);
        Self::default()
    }

    /// Bring `replica` up to date with `server`: unload what the server
    /// evicted, install new/changed columns and newly light-final sections,
    /// then apply this round's block, cell-KV, draw-set and light changes.
    pub fn sync(&mut self, server: &mut ServerWorld, replica: &mut ReplicaWorld) {
        self.unload_evicted(server, replica);
        let data = server.data();
        let changed_columns: Vec<ChunkPos> = data
            .columns
            .keys()
            .copied()
            .filter(|cp| self.columns.get(cp) != Some(&data.column_payload_revision(*cp)))
            .collect();
        for cp in changed_columns {
            if let Some(payload) = server.column_payload(cp) {
                replica.install_remote_column(payload);
                self.columns
                    .insert(cp, server.data().column_payload_revision(cp));
            }
        }
        let ready: Vec<SectionPos> = server
            .data()
            .sections
            .keys()
            .copied()
            .filter(|sp| !self.sections.contains(sp) && server.section_light_final(*sp))
            .collect();
        let mut installed = Vec::with_capacity(ready.len());
        for sp in ready {
            let Some(payload) = server.section_payload(sp) else {
                continue;
            };
            if let Some(pos) = replica.install_remote_section_deferred(payload) {
                installed.push(pos);
            }
            self.sections.insert(sp);
        }
        replica.finish_remote_install_batch(&installed);
        self.apply_changes(server, replica);
    }

    fn unload_evicted(&mut self, server: &ServerWorld, replica: &mut ReplicaWorld) {
        let data = server.data();
        let gone_columns: Vec<ChunkPos> = self
            .columns
            .keys()
            .copied()
            .filter(|cp| !data.columns.contains_key(cp))
            .collect();
        for cp in gone_columns {
            replica.uninstall_remote_column(cp);
            self.columns.remove(&cp);
            self.sections.retain(|sp| sp.chunk_pos() != cp);
        }
        let gone_sections: Vec<SectionPos> = self
            .sections
            .iter()
            .copied()
            .filter(|sp| !data.sections.contains_key(sp))
            .collect();
        for sp in gone_sections {
            replica.uninstall_remote_section(sp);
            self.sections.remove(&sp);
        }
    }

    fn apply_changes(&mut self, server: &mut ServerWorld, replica: &mut ReplicaWorld) {
        let held = |sections: &FxHashSet<SectionPos>, p: petramond_math::math::IVec3| {
            SectionPos::from_world(p.x, p.y, p.z).is_some_and(|sp| sections.contains(&sp))
        };
        for delta in server.take_block_deltas() {
            if held(&self.sections, delta.pos) {
                replica.apply_remote_delta(delta);
            }
        }
        for kv in server.take_cell_kv_deltas() {
            if held(&self.sections, kv.pos) {
                replica.apply_remote_cell_kv(kv);
            }
        }
        for draw in server.take_block_draw_deltas() {
            if held(&self.sections, draw.pos) {
                replica.apply_remote_block_draw(draw.pos, draw.prims);
            }
        }
        for sp in server.take_light_ship_log() {
            if !self.sections.contains(&sp) {
                continue;
            }
            if let Some(light) = server.light_payload(sp) {
                replica.install_remote_light(light);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::testutil::install_flat_floor;
    use petramond_world::block::Block;

    /// The mirror is a faithful connection stand-in: terrain the server
    /// finishes lands on the replica, a later edit reaches it through the
    /// delta log, and an eviction takes the section off the replica again.
    #[test]
    fn a_mirrored_replica_follows_installs_edits_and_evictions() {
        let mut server = ServerWorld::new(0, 1);
        let mut replica = ReplicaWorld::new(0, 1);
        install_flat_floor(&mut server);
        let mut mirror = ReplicaMirror::new(&mut server);

        // Fixture sections demand their first bake; pump until the floor is
        // light-final and shipped. Both worlds run inline pools, so each pump
        // finishes what the previous one submitted.
        let floor = SectionPos::from_world(0, 64, 0).expect("in range");
        for _ in 0..64 {
            if replica.data().sections.contains_key(&floor) {
                break;
            }
            server.pump_light_bakes();
            mirror.sync(&mut server, &mut replica);
        }
        assert!(
            replica.data().sections.contains_key(&floor),
            "the floor never shipped"
        );
        assert_eq!(
            Block::from_id(replica.data().chunk_block(3, 64, 3)),
            Block::Stone
        );

        assert!(server.set_block_world(3, 65, 3, Block::Dirt));
        mirror.sync(&mut server, &mut replica);
        assert_eq!(
            Block::from_id(replica.data().chunk_block(3, 65, 3)),
            Block::Dirt,
            "a server edit reaches the replica through the delta log"
        );

        server.clear_world();
        mirror.sync(&mut server, &mut replica);
        assert!(
            replica.data().sections.is_empty(),
            "an eviction on the server unloads the replica's copy"
        );
    }
}
