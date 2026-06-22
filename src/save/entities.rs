//! `entities.dat`: the world's dropped item-stacks. A flat list — item entities
//! are few (bounded by the 300 s despawn) and live on `Game`, not in chunks, so
//! they persist as one small file rather than per-region.

use crate::entity::DroppedItem;
use crate::item::{ItemStack, ItemType};
use crate::mathh::Vec3;
use crate::save::codec::{Reader, Writer};

const VERSION: u32 = 1;

pub fn encode(items: &[DroppedItem]) -> Vec<u8> {
    let mut b = Vec::with_capacity(8 + items.len() * 28);
    b.put_u32(VERSION);
    b.put_u32(items.len() as u32);
    for it in items {
        b.put_f32(it.pos.x);
        b.put_f32(it.pos.y);
        b.put_f32(it.pos.z);
        b.put_f32(it.vel.x);
        b.put_f32(it.vel.y);
        b.put_f32(it.vel.z);
        b.put_u8(it.stack.item.id());
        b.put_u8(it.stack.count);
        b.put_f32(it.age);
        b.put_f32(it.spin);
    }
    b
}

pub fn decode(bytes: &[u8]) -> Option<Vec<DroppedItem>> {
    let mut r = Reader::new(bytes);
    if r.u32()? != VERSION {
        return None;
    }
    let n = r.u32()? as usize;
    let mut out = Vec::with_capacity(n.min(4096));
    for _ in 0..n {
        let pos = Vec3::new(r.f32()?, r.f32()?, r.f32()?);
        let vel = Vec3::new(r.f32()?, r.f32()?, r.f32()?);
        let id = r.u8()?;
        let count = r.u8()?;
        let age = r.f32()?;
        let spin = r.f32()?;
        let stack = ItemStack::new(ItemType::from_id(id), count);
        if stack.is_empty() {
            continue;
        }
        // `new` sets pos + a deterministic pop velocity; overwrite the saved
        // motion/age/spin so the drop resumes exactly where it left off.
        let mut d = DroppedItem::new(pos, stack, 0);
        d.vel = vel;
        d.age = age;
        d.spin = spin;
        out.push(d);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entities_roundtrip() {
        let mut a = DroppedItem::new(Vec3::new(1.0, 64.0, 2.0), ItemStack::new(ItemType::Stone, 5), 1);
        a.vel = Vec3::new(0.1, -0.2, 0.3);
        a.age = 42.5;
        a.spin = 1.25;
        let b = DroppedItem::new(Vec3::new(-3.0, 70.0, 8.0), ItemStack::new(ItemType::Dirt, 1), 2);

        let bytes = encode(&[a.clone(), b.clone()]);
        let got = decode(&bytes).expect("decodes");
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].pos, a.pos);
        assert_eq!(got[0].vel, a.vel);
        assert_eq!(got[0].stack, a.stack);
        assert_eq!(got[0].age, 42.5);
        assert_eq!(got[0].spin, 1.25);
        assert_eq!(got[1].stack, b.stack);
    }
}
