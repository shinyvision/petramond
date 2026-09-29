//! Per-connection delta coding of entity rows on the TCP transport.
//!
//! A tick window ships a row for every tracked mob, item and player, and most of a row repeats
//! what the same connection received a window earlier. The connection's writer keeps the last row
//! it sent per entity and replaces a lane's `updated` rows with a packed stream of per-field
//! changes against it; the reader keeps the same baseline and rebuilds the full rows before
//! anything else sees the message. Both ends see every frame in order, so the baselines never
//! diverge. A row equal to its baseline still ships (as an empty change set), so the recipient
//! applies exactly the rows it would have applied without the coding.
//!
//! Floats are coded as the XOR of their bits with the baseline's, as a varint: exact, and a small
//! move keeps the sign, exponent and top mantissa bits, so the XOR is short.

use std::hash::Hash;

use rustc_hash::FxHashMap;
use serde::de::DeserializeOwned;
use serde::Serialize;

use super::protocol::{
    EntityLane, EntityRow, ItemStateRow, MobStateRow, PlayerStateRow, RowSet, TickSection,
    TickUpdate,
};

/// Marks an entry that carries a whole row (no baseline for its id).
const FULL: u32 = 1 << 31;

pub type DeltaError = postcard::Error;

fn put<T: Serialize + ?Sized>(out: &mut Vec<u8>, value: &T) {
    *out = postcard::to_extend(value, std::mem::take(out)).expect("row fields encode");
}

fn take<T: DeserializeOwned>(input: &mut &[u8]) -> Result<T, DeltaError> {
    let (value, rest) = postcard::take_from_bytes(input)?;
    *input = rest;
    Ok(value)
}

struct Fields<'a> {
    mask: u32,
    body: &'a mut Vec<u8>,
}

impl Fields<'_> {
    fn value<T: Serialize + PartialEq>(&mut self, bit: u32, new: &T, base: &T) {
        if new != base {
            self.mask |= 1 << bit;
            put(self.body, new);
        }
    }

    fn f64(&mut self, bit: u32, new: f64, base: f64) {
        let x = new.to_bits() ^ base.to_bits();
        if x != 0 {
            self.mask |= 1 << bit;
            put(self.body, &x);
        }
    }

    fn f32(&mut self, bit: u32, new: f32, base: f32) {
        let x = new.to_bits() ^ base.to_bits();
        if x != 0 {
            self.mask |= 1 << bit;
            put(self.body, &x);
        }
    }
}

struct Patch<'a, 'b> {
    mask: u32,
    input: &'a mut &'b [u8],
}

impl Patch<'_, '_> {
    fn value<T: DeserializeOwned>(&mut self, bit: u32, slot: &mut T) -> Result<(), DeltaError> {
        if self.mask & (1 << bit) != 0 {
            *slot = take(self.input)?;
        }
        Ok(())
    }

    fn f64(&mut self, bit: u32, slot: &mut f64) -> Result<(), DeltaError> {
        if self.mask & (1 << bit) != 0 {
            *slot = f64::from_bits(slot.to_bits() ^ take::<u64>(self.input)?);
        }
        Ok(())
    }

    fn f32(&mut self, bit: u32, slot: &mut f32) -> Result<(), DeltaError> {
        if self.mask & (1 << bit) != 0 {
            *slot = f32::from_bits(slot.to_bits() ^ take::<u32>(self.input)?);
        }
        Ok(())
    }
}

/// A row type the transport can code as changes against the previous row of the same entity.
pub trait DeltaRow: EntityRow + Serialize + DeserializeOwned {
    /// Writes the changed fields of `self` against `base` into `body`; returns their mask.
    fn write_changes(&self, base: &Self, body: &mut Vec<u8>) -> u32;
    /// Applies the fields `mask` names, read from `input`, onto `row` (a copy of the baseline).
    fn apply_changes(row: &mut Self, mask: u32, input: &mut &[u8]) -> Result<(), DeltaError>;
}

impl DeltaRow for MobStateRow {
    fn write_changes(&self, base: &Self, body: &mut Vec<u8>) -> u32 {
        let mut f = Fields { mask: 0, body };
        f.value(0, &self.kind_id, &base.kind_id);
        f.f64(1, self.pos.x, base.pos.x);
        f.f64(2, self.pos.y, base.pos.y);
        f.f64(3, self.pos.z, base.pos.z);
        f.f32(4, self.yaw, base.yaw);
        f.f32(5, self.tilt.pitch, base.tilt.pitch);
        f.f32(6, self.tilt.roll, base.tilt.roll);
        f.f32(7, self.anim_time, base.anim_time);
        f.value(8, &self.moving, &base.moving);
        f.value(9, &self.idle_anim, &base.idle_anim);
        f.f32(10, self.head_yaw, base.head_yaw);
        f.f32(11, self.head_pitch, base.head_pitch);
        f.f32(12, self.hurt_timer, base.hurt_timer);
        f.value(13, &self.dead, &base.dead);
        f.value(14, &self.shorn, &base.shorn);
        f.value(15, &self.emitters, &base.emitters);
        f.value(16, &self.conditions, &base.conditions);
        let same_names = self.anims.len() == base.anims.len()
            && self
                .anims
                .iter()
                .zip(&base.anims)
                .all(|((a, _), (b, _))| a == b);
        if same_names {
            if self.anims != base.anims {
                f.mask |= 1 << 17;
                for ((_, new), (_, old)) in self.anims.iter().zip(&base.anims) {
                    put(f.body, &(new.to_bits() ^ old.to_bits()));
                }
            }
        } else {
            f.mask |= 1 << 18;
            put(f.body, &self.anims);
        }
        f.value(19, &self.ragdoll, &base.ragdoll);
        f.value(20, &self.dig, &base.dig);
        f.value(21, &self.held, &base.held);
        f.value(22, &self.draw, &base.draw);
        f.mask
    }

    fn apply_changes(row: &mut Self, mask: u32, input: &mut &[u8]) -> Result<(), DeltaError> {
        let mut p = Patch { mask, input };
        p.value(0, &mut row.kind_id)?;
        p.f64(1, &mut row.pos.x)?;
        p.f64(2, &mut row.pos.y)?;
        p.f64(3, &mut row.pos.z)?;
        p.f32(4, &mut row.yaw)?;
        p.f32(5, &mut row.tilt.pitch)?;
        p.f32(6, &mut row.tilt.roll)?;
        p.f32(7, &mut row.anim_time)?;
        p.value(8, &mut row.moving)?;
        p.value(9, &mut row.idle_anim)?;
        p.f32(10, &mut row.head_yaw)?;
        p.f32(11, &mut row.head_pitch)?;
        p.f32(12, &mut row.hurt_timer)?;
        p.value(13, &mut row.dead)?;
        p.value(14, &mut row.shorn)?;
        p.value(15, &mut row.emitters)?;
        p.value(16, &mut row.conditions)?;
        if mask & (1 << 17) != 0 {
            for (_, phase) in &mut row.anims {
                *phase = f32::from_bits(phase.to_bits() ^ take::<u32>(p.input)?);
            }
        }
        p.value(18, &mut row.anims)?;
        p.value(19, &mut row.ragdoll)?;
        p.value(20, &mut row.dig)?;
        p.value(21, &mut row.held)?;
        p.value(22, &mut row.draw)?;
        Ok(())
    }
}

impl DeltaRow for ItemStateRow {
    fn write_changes(&self, base: &Self, body: &mut Vec<u8>) -> u32 {
        let mut f = Fields { mask: 0, body };
        f.value(0, &self.item_id, &base.item_id);
        f.value(1, &self.count, &base.count);
        f.value(2, &self.data, &base.data);
        f.f64(3, self.pos.x, base.pos.x);
        f.f64(4, self.pos.y, base.pos.y);
        f.f64(5, self.pos.z, base.pos.z);
        f.f32(6, self.spin, base.spin);
        f.value(7, &self.flight, &base.flight);
        f.mask
    }

    fn apply_changes(row: &mut Self, mask: u32, input: &mut &[u8]) -> Result<(), DeltaError> {
        let mut p = Patch { mask, input };
        p.value(0, &mut row.item_id)?;
        p.value(1, &mut row.count)?;
        p.value(2, &mut row.data)?;
        p.f64(3, &mut row.pos.x)?;
        p.f64(4, &mut row.pos.y)?;
        p.f64(5, &mut row.pos.z)?;
        p.f32(6, &mut row.spin)?;
        p.value(7, &mut row.flight)?;
        Ok(())
    }
}

/// Players are few; a changed player row ships whole.
impl DeltaRow for PlayerStateRow {
    fn write_changes(&self, base: &Self, body: &mut Vec<u8>) -> u32 {
        let mut f = Fields { mask: 0, body };
        f.value(0, self, base);
        f.mask
    }

    fn apply_changes(row: &mut Self, mask: u32, input: &mut &[u8]) -> Result<(), DeltaError> {
        Patch { mask, input }.value(0, row)
    }
}

struct Baseline<R: EntityRow> {
    rows: FxHashMap<R::Id, R>,
}

impl<R: EntityRow> Default for Baseline<R> {
    fn default() -> Self {
        Baseline {
            rows: FxHashMap::default(),
        }
    }
}

impl<R> Baseline<R>
where
    R: DeltaRow,
    R::Id: Hash + Serialize + DeserializeOwned,
{
    fn note_membership(&mut self, lane: &EntityLane<R, R::Id>) {
        for id in &lane.despawned {
            self.rows.remove(id);
        }
        for row in lane.spawned.iter() {
            self.rows.insert(row.entity_id(), row.clone());
        }
    }

    fn pack(&mut self, lane: &mut EntityLane<R, R::Id>) {
        self.note_membership(lane);
        if lane.updated.is_empty() {
            return;
        }
        let mut out = Vec::new();
        let mut body = Vec::new();
        for row in lane.updated.iter() {
            let id = row.entity_id();
            put(&mut out, &id);
            match self.rows.get_mut(&id) {
                Some(base) => {
                    body.clear();
                    let mask = row.write_changes(base, &mut body);
                    put(&mut out, &mask);
                    out.extend_from_slice(&body);
                    if mask != 0 {
                        base.clone_from(row);
                    }
                }
                None => {
                    put(&mut out, &FULL);
                    put(&mut out, row);
                    self.rows.insert(id, row.clone());
                }
            }
        }
        lane.updated = RowSet::packed(out);
    }

    fn unpack(&mut self, lane: &mut EntityLane<R, R::Id>) -> Result<(), DeltaError> {
        self.note_membership(lane);
        let Some(packed) = lane.updated.take_packed() else {
            return Ok(());
        };
        let mut input = packed.as_slice();
        let mut rows = Vec::new();
        while !input.is_empty() {
            let id: R::Id = take(&mut input)?;
            let mask: u32 = take(&mut input)?;
            let row = if mask == FULL {
                take::<R>(&mut input)?
            } else {
                let base = self
                    .rows
                    .get(&id)
                    .ok_or(postcard::Error::DeserializeBadEncoding)?;
                let mut row = base.clone();
                R::apply_changes(&mut row, mask, &mut input)?;
                row
            };
            self.rows.insert(id, row.clone());
            rows.push(row);
        }
        lane.updated = rows.into();
        Ok(())
    }
}

/// One direction of one connection: the writer packs with it, the reader unpacks with its own.
#[derive(Default)]
pub struct TickDelta {
    mobs: Baseline<MobStateRow>,
    items: Baseline<ItemStateRow>,
    players: Baseline<PlayerStateRow>,
}

impl TickDelta {
    pub fn pack(&mut self, update: &mut TickUpdate) {
        for section in &mut update.sections {
            match section {
                TickSection::Mobs(lane) => self.mobs.pack(lane),
                TickSection::Items(lane) => self.items.pack(lane),
                TickSection::Players(lane) => self.players.pack(lane),
                _ => {}
            }
        }
    }

    pub fn unpack(&mut self, update: &mut TickUpdate) -> Result<(), DeltaError> {
        for section in &mut update.sections {
            match section {
                TickSection::Mobs(lane) => self.mobs.unpack(lane)?,
                TickSection::Items(lane) => self.items.unpack(lane)?,
                TickSection::Players(lane) => self.players.unpack(lane)?,
                _ => {}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
