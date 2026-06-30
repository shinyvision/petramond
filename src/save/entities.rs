//! Per-entity (de)serialization for the dropped item-stacks stored inside a
//! chunk's save record.
//!
//! Item entities live with their owning chunk now — so a stack's lifetime timer
//! pauses when the chunk unloads and resumes (with the right remaining time) when
//! it loads — so this is a helper for the chunk codec rather than a standalone
//! file format. A chunk record appends a length-prefixed list of these after its
//! block/biome/water data; see `save::codec`.

use crate::entity::DroppedItem;
use crate::item::{ItemStack, ItemType};
use crate::mathh::Vec3;
use crate::save::codec::{put_f32, put_u16, put_u32, put_u8, Reader};

/// Bytes per serialized entity: pos(12) + vel(12) + item(1) + count(1) +
/// ticks_lived(4) + spin(4).
const ENTITY_BYTES: usize = 34;

/// Append a `u16`-length-prefixed list of item entities to `buf`. The count is
/// capped at `u16::MAX` (a chunk never holds anywhere near that many drops).
pub fn put_entities(buf: &mut Vec<u8>, items: &[DroppedItem]) {
    let n = items.len().min(u16::MAX as usize);
    buf.reserve(2 + n * ENTITY_BYTES);
    put_u16(buf, n as u16);
    for it in &items[..n] {
        put_f32(buf, it.pos.x);
        put_f32(buf, it.pos.y);
        put_f32(buf, it.pos.z);
        put_f32(buf, it.vel.x);
        put_f32(buf, it.vel.y);
        put_f32(buf, it.vel.z);
        put_u8(buf, it.stack.item.id());
        put_u8(buf, it.stack.count);
        put_u32(buf, it.ticks_lived);
        put_f32(buf, it.spin);
    }
}

/// Read a list of item entities written by [`put_entities`]. Empty (zero-count)
/// stacks are dropped; `None` on truncated input. The reconstructed drop resumes
/// with its saved motion, lifetime and spin (the random spawn "pop" is bypassed).
pub fn get_entities(r: &mut Reader) -> Option<Vec<DroppedItem>> {
    let n = r.u16()? as usize;
    let mut out = Vec::with_capacity(n.min(1024));
    for _ in 0..n {
        let pos = Vec3::new(r.f32()?, r.f32()?, r.f32()?);
        let vel = Vec3::new(r.f32()?, r.f32()?, r.f32()?);
        let id = r.u8()?;
        let count = r.u8()?;
        let ticks_lived = r.u32()?;
        let spin = r.f32()?;
        let stack = ItemStack::new(ItemType::from_id(id), count);
        if stack.is_empty() {
            continue;
        }
        let mut d = DroppedItem::new(pos, stack, 0);
        d.vel = vel;
        d.ticks_lived = ticks_lived;
        d.spin = spin;
        out.push(d);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entities_roundtrip_through_a_buffer() {
        let mut a = DroppedItem::new(
            Vec3::new(1.0, 64.0, 2.0),
            ItemStack::new(ItemType::Stone, 5),
            1,
        );
        a.vel = Vec3::new(0.1, -0.2, 0.3);
        a.ticks_lived = 3000;
        a.spin = 1.25;
        let b = DroppedItem::new(
            Vec3::new(-3.0, 70.0, 8.0),
            ItemStack::new(ItemType::Dirt, 1),
            2,
        );

        let mut buf = Vec::new();
        put_entities(&mut buf, &[a.clone(), b.clone()]);

        let mut r = Reader::new(&buf);
        let got = get_entities(&mut r).expect("decodes");
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].pos, a.pos);
        assert_eq!(got[0].vel, a.vel);
        assert_eq!(got[0].stack, a.stack);
        assert_eq!(
            got[0].ticks_lived, 3000,
            "remaining lifetime survives the round-trip"
        );
        assert_eq!(got[0].spin, 1.25);
        assert_eq!(got[1].stack, b.stack);
        assert_eq!(got[1].ticks_lived, 0);
    }

    #[test]
    fn empty_list_roundtrips() {
        let mut buf = Vec::new();
        put_entities(&mut buf, &[]);
        let mut r = Reader::new(&buf);
        assert!(get_entities(&mut r).expect("decodes").is_empty());
    }

    #[test]
    fn truncated_input_is_none() {
        // Claims one entity but provides no body.
        let mut buf = Vec::new();
        put_u16(&mut buf, 1);
        let mut r = Reader::new(&buf);
        assert!(get_entities(&mut r).is_none());
    }
}
