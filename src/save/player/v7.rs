use petramond_math::math::{IVec3, Vec3};
use petramond_math::world_pos::WorldPos;
use petramond_persist::bytecodec::Reader;

use crate::save::codec::DiskSlot;
use crate::save::format::RecordError;
use crate::save::wire::{TaggedWriter, Wire};

use super::FORMAT;

const V7_SLOTS: usize = 36;

pub(super) fn upgrade(body: &[u8]) -> Result<Vec<u8>, RecordError> {
    let mut r = Reader::new(body);
    let corrupt =
        |what: &'static str, r: &Reader| RecordError::corrupt(FORMAT.name, what, 4 + r.offset());
    macro_rules! read {
        ($t:ty, $what:literal) => {
            <$t>::get(&mut r).ok_or_else(|| corrupt($what, &r))?
        };
    }
    let pos = read!(WorldPos, "position");
    let vel = read!(Vec3, "velocity");
    let yaw = read!(f32, "yaw");
    let pitch = read!(f32, "pitch");
    let mode = read!(u8, "mode");
    let health = read!(u32, "health");
    let bed = match read!(u8, "bed spawn") {
        1 => Some((read!(IVec3, "bed"), read!(IVec3, "bed wake spot"))),
        _ => None,
    };
    let mut slots = Vec::with_capacity(V7_SLOTS);
    for _ in 0..V7_SLOTS {
        slots.push(read!(DiskSlot, "inventory slot"));
    }
    let cursor = read!(DiskSlot, "cursor slot");
    let off_hand = read!(DiskSlot, "off-hand slot");
    let active_slot = read!(u8, "active slot");
    let craft_craftable_only = read!(u8, "craftable-only filter") != 0;
    let effects = read!(Vec<(String, u32)>, "effects");
    let obtained = read!(Vec<String>, "obtained items");
    let unlocked = read!(Vec<String>, "unlocked recipes");
    if !r.is_at_end() {
        return Err(corrupt("trailing bytes", &r));
    }

    let mut out = Vec::with_capacity(body.len() + 128);
    let mut w = TaggedWriter::new(&mut out);
    w.field(1, &pos);
    w.field(2, &vel);
    w.field(3, &yaw);
    w.field(4, &pitch);
    w.field(5, &mode);
    w.field(6, &health);
    w.field(7, &bed);
    w.field(8, &slots);
    w.field(9, &cursor);
    w.field(10, &off_hand);
    w.field(11, &active_slot);
    w.field(12, &craft_craftable_only);
    w.field(13, &effects);
    w.field(14, &obtained);
    w.field(15, &unlocked);
    w.finish();
    Ok(out)
}
