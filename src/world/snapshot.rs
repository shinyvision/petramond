//! Section persistence: the snapshot-and-persist gate shared by autosave/quit
//! (`flush_modified_chunks`) and eviction (`harvest_section_snapshot`), plus
//! the save-handle plumbing.

use crate::chunk::SectionPos;
use crate::entity::DroppedItem;
use crate::mob::SavedMob;
use crate::save::{SectionSnapshot, WorldSave};

use super::store::{World, WorldRole};

impl World {
    /// Attach an on-disk save: enables section persistence (load-from-disk in the
    /// streamer and flush-on-evict) and gives `Game` a handle for level/entities.
    /// Never on a replica — the server owns persistence; a replica persisting its
    /// installed copies would shadow the authoritative world.
    pub fn attach_save(&mut self, save: WorldSave) {
        debug_assert!(
            self.role != WorldRole::ClientReplica,
            "a replica must not persist replicated sections"
        );
        self.save = Some(save);
    }

    pub fn save(&self) -> Option<&WorldSave> {
        self.save.as_ref()
    }

    pub fn save_mut(&mut self) -> Option<&mut WorldSave> {
        self.save.as_mut()
    }

    /// Whether an authoritative record or an explored cache record exists.
    pub(super) fn saved_section_contains(&self, pos: SectionPos) -> bool {
        self.save.as_ref().is_some_and(|save| {
            save.authoritative_manifest_contains(pos) || save.explored_manifest_contains(pos)
        })
    }

    /// The single snapshot-and-persist gate shared by [`flush_modified_chunks`]
    /// (autosave/quit) and `unload_far_columns` (eviction). Applies the three-way
    /// persist condition and, when it holds, builds the section's [`SectionSnapshot`]
    /// with `entities`/`mobs` attached; returns `None` when the section needn't persist.
    ///
    /// The gate persists a section when ANY of:
    /// - its blocks were modified,
    /// - it carries item entities or mobs right now, or
    /// - `record_holds_entities` — its on-disk record still holds drops/mobs it no
    ///   longer carries, so the stale record must be rewritten or it resurrects them on
    ///   reload (cross-session: the caller derives it from the save handle).
    ///
    /// The caller owns the harvest policy (which fed `entities` / `mobs`) and the
    /// post-action (clear `modified` vs. evict), keeping flush's "stay active" and
    /// unload's "pause / save" lifetimes distinct.
    pub(super) fn snapshot_section_for_save(
        &self,
        pos: SectionPos,
        entities: Vec<DroppedItem>,
        mobs: Vec<SavedMob>,
        record_holds_entities: bool,
    ) -> Option<SectionSnapshot> {
        let section = self.sections.get(&pos)?;
        // Derived explored terrain and authoritative edits/entities live in
        // separate stores. First cache persistence waits for final light so the
        // common path writes and compresses the record only once.
        let light_final = !section.light_dirty || section.all_opaque();
        let authoritative_exists = self
            .save
            .as_ref()
            .is_some_and(|s| s.authoritative_manifest_contains(pos));
        let explored_exists = self
            .save
            .as_ref()
            .is_some_and(|s| s.explored_manifest_contains(pos));
        let explored_first_persist = light_final && !authoritative_exists && !explored_exists;
        // A record already on disk whose light rebaked since it was written
        // (a lightless neighbour landed at the explored boundary, or an edit's
        // spill) rewrites, or its saved cubes diverge from its neighbours'.
        let relit_persisted = light_final
            && self.relit_since_persist.contains(&pos)
            && (authoritative_exists || explored_exists);
        // An edit dirtied this record's baked light and the rebake hasn't
        // landed (eviction/quit racing the bake): rewrite the record NOW —
        // the snapshot omits dirty light, so reload rebakes instead of
        // loading the pre-edit cubes as clean (a permanent dark seam).
        let light_stale_persisted = !light_final
            && self.light_edited_since_persist.contains(&pos)
            && (authoritative_exists || explored_exists);
        let authoritative =
            section.modified || !entities.is_empty() || !mobs.is_empty() || record_holds_entities;
        if authoritative || explored_first_persist || relit_persisted || light_stale_persisted {
            let mut snap = SectionSnapshot::from_section(section);
            snap.entities = entities;
            snap.mobs = mobs;
            snap.cache_only = !authoritative && !authoritative_exists;
            Some(snap)
        } else {
            None
        }
    }

    /// Snapshot every modified section to the save thread and clear the flags. Also
    /// snapshots any section holding item entities or mobs (even if its blocks are
    /// untouched) so their lifetime timers persist; they stay active in memory. Called
    /// on autosave and on quit; a no-op without an attached save.
    pub fn flush_modified_chunks(&mut self) {
        if self.save.is_none() {
            return;
        }
        // Flush's harvest policy: CLONE the resting drops and mobs (they stay active in
        // memory) so a crash can't lose them.
        let mut by_section = self.dropped_items.items_by_section();
        let mut mobs_by_section = self.mobs.saved_by_section();
        let positions: Vec<SectionPos> = self.sections.keys().copied().collect();
        let mut snaps = Vec::new();
        let mut persisted = Vec::new();
        for pos in positions {
            let entities = by_section.remove(&pos).unwrap_or_default();
            let mobs = mobs_by_section.remove(&pos).unwrap_or_default();
            let record_holds_entities = self
                .save
                .as_ref()
                .is_some_and(|s| s.record_holds_entities(pos));
            if let Some(snap) =
                self.snapshot_section_for_save(pos, entities, mobs, record_holds_entities)
            {
                snaps.push(snap);
                persisted.push(pos);
            }
        }
        // Post-action: a persisted section is now in sync with disk. The flush
        // visited EVERY loaded section, so relit bookkeeping resets wholesale
        // (evicted stragglers included — they can't re-persist anyway).
        for pos in persisted {
            if let Some(s) = self.section_mut(pos) {
                s.modified = false;
            }
            self.relit_since_persist.remove(&pos);
            self.light_edited_since_persist.remove(&pos);
        }
        if let Some(save) = self.save.as_mut() {
            save.save_sections(snaps);
        }
        self.flush_pending_colgen_records();
    }

    /// Send the buffered column-gen cache records to the save thread. Batched
    /// (autosave / unload / a size trigger in `poll`) so one region-file
    /// rewrite absorbs many columns.
    pub(super) fn flush_pending_colgen_records(&mut self) {
        if self.pending_colgen_records.is_empty() {
            return;
        }
        let recs = std::mem::take(&mut self.pending_colgen_records);
        if let Some(save) = self.save.as_mut() {
            save.save_column_gens(recs);
        }
    }

    /// Harvest a section into a save snapshot for UNLOAD: this DRAINS the section's
    /// resting drops/mobs into the record (pausing their lifetime timers until the
    /// section reloads), returning `None` when the section needn't persist. The persist
    /// gate is shared with autosave (`snapshot_section_for_save`).
    pub(super) fn harvest_section_snapshot(&mut self, sp: SectionPos) -> Option<SectionSnapshot> {
        if !self.sections.contains_key(&sp) {
            return None;
        }
        // The section's true content is still in flight (its saved record has not
        // been answered/applied yet): persisting the generated base now would
        // overwrite the player's on-disk record with pre-overlay state. Skip; the
        // record on disk stays authoritative. (Entities that wandered in are
        // dropped with the unload — losing a wanderer beats corrupting a build.)
        if self.awaited_overlays.contains(&sp) || self.pending_overlays.contains_key(&sp) {
            return None;
        }
        let entities = self.dropped_items.take_items_in_section(sp);
        let mobs = self.mobs.take_in_section(sp);
        let record_holds_entities = self
            .save
            .as_ref()
            .is_some_and(|s| s.record_holds_entities(sp));
        self.snapshot_section_for_save(sp, entities, mobs, record_holds_entities)
    }
}
