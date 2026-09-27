use crate::entity::DroppedItem;
use crate::mob::SavedMob;
use crate::save::{SectionSnapshot, WorldSave};
use crate::world::ServerWorld;
use petramond_world::chunk::SectionPos;

impl ServerWorld {
    pub fn attach_save(&mut self, save: WorldSave, saved: super::SavedIndex) {
        self.side.schematics.store = crate::schematic::store::Store::new(Some(save.dir()));
        self.side.save = Some(save);
        self.data.saved = saved;
    }

    pub fn save(&self) -> Option<&WorldSave> {
        self.side.save.as_ref()
    }

    pub fn save_mut(&mut self) -> Option<&mut WorldSave> {
        self.side.save.as_mut()
    }

    /// Shared gate for flush (autosave/quit) and unload (eviction) to decide if a section needs
    /// saving, and build the `SectionSnapshot` when it does.
    ///
    /// Persists if blocks were modified, section still has item entities or mobs, or
    /// `record_holds_entities` is set: the on-disk record may still have drops/mobs the section no
    /// longer carries, so it must be rewritten to keep reloads correct. The caller works that
    /// out from the save handle.
    ///
    /// Harvest policy and post-action are the caller's job: flush clears `modified` and keeps the
    /// section active, unload evicts it.
    pub(super) fn snapshot_section_for_save(
        &self,
        pos: SectionPos,
        entities: Vec<DroppedItem>,
        mobs: Vec<SavedMob>,
        record_holds_entities: bool,
    ) -> Option<SectionSnapshot> {
        let section = self.data.sections.get(&pos)?;
        let light_final = !section.light_dirty || section.all_opaque();
        let authoritative_exists =
            self.side.save.as_ref().is_some() && self.data.saved.authoritative_contains(pos);
        let explored_exists =
            self.side.save.as_ref().is_some() && self.data.saved.explored_contains(pos);
        let explored_first_persist = light_final && !authoritative_exists && !explored_exists;
        let relit_persisted = light_final
            && self.data.relit_since_persist.contains(&pos)
            && (authoritative_exists || explored_exists);
        let light_stale_persisted = !light_final
            && self.data.light_edited_since_persist.contains(&pos)
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

    pub fn flush_modified_chunks(&mut self) {
        if self.side.save.is_none() {
            return;
        }
        self.apply_light_edits();
        let mut by_section = self.side.entities.dropped_items.items_by_section();
        let mut mobs_by_section = self.side.entities.mobs.saved_by_section();
        let positions: Vec<SectionPos> = self.data.sections.keys().copied().collect();
        let mut snaps = Vec::new();
        let mut persisted = Vec::new();
        for pos in positions {
            let entities = by_section.remove(&pos).unwrap_or_default();
            let mobs = mobs_by_section.remove(&pos).unwrap_or_default();
            let record_holds_entities = self
                .side
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
        for pos in persisted {
            if let Some(s) = self.data.section_mut(pos) {
                s.modified = false;
            }
            self.data.relit_since_persist.remove(&pos);
            self.data.light_edited_since_persist.remove(&pos);
        }
        if let Some(save) = self.side.save.as_mut() {
            save.save_sections(&mut self.data.saved, snaps);
        }
        self.flush_pending_colgen_records();
    }

    pub(super) fn flush_pending_colgen_records(&mut self) {
        if self.side.gen.pending_colgen_records.is_empty() {
            return;
        }
        let recs = std::mem::take(&mut self.side.gen.pending_colgen_records);
        if let Some(save) = self.side.save.as_mut() {
            save.save_column_gens(recs);
        }
    }

    pub(super) fn harvest_section_snapshot(&mut self, sp: SectionPos) -> Option<SectionSnapshot> {
        if !self.data.sections.contains_key(&sp) {
            return None;
        }
        if self.side.gen.awaited_overlays.contains(&sp)
            || self.side.gen.pending_overlays.contains_key(&sp)
        {
            return None;
        }
        let entities = self.side.entities.dropped_items.take_items_in_section(sp);
        let mobs = self.side.entities.mobs.take_in_section(sp);
        let record_holds_entities = self
            .side
            .save
            .as_ref()
            .is_some_and(|s| s.record_holds_entities(sp));
        self.snapshot_section_for_save(sp, entities, mobs, record_holds_entities)
    }
}
