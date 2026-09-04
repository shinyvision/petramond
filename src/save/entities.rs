//! Per-entity (de)serialization for the dropped item-stacks stored inside a
//! chunk's save record.
//!
//! Item entities live with their owning chunk now — so a stack's lifetime timer
//! pauses when the chunk unloads and resumes (with the right remaining time) when
//! it loads — so this is a helper for the chunk codec rather than a standalone
//! file format. A chunk record appends a length-prefixed list of these after its
//! block/biome/water data; see `save::codec`.

use crate::entity::{DroppedItem, Heading, Motion, Stuck};
use crate::save::codec::{get_item_slot, put_f32, put_item_slot, put_u16, put_u32, Reader};
use petramond_math::math::{IVec3, Vec3};

/// Bytes per serialized entity: pos(12) + vel(12) + slot(4, plain stack) +
/// ticks_lived(4) + spin(4) + motion(1). Data-bearing stacks and lodged
/// items append past this; the constant is only a reserve hint.
const ENTITY_BYTES: usize = 37;

petramond_math::wire_enum::wire_enum! {
    /// The persisted motion tag. A flight persists WITHOUT its owner (a
    /// session, and sessions do not outlive the process) and without its
    /// heading, which its first step derives from the velocity; a lodged
    /// item's heading IS state (its velocity is zero) and rides along with
    /// its anchor.
    enum MotionKind: u8 {
        Loose = 0,
        Flight = 1,
        Stuck = 2,
    }
    default Loose
}

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
        put_item_slot(buf, Some(it.stack));
        put_u32(buf, it.ticks_lived);
        put_f32(buf, it.spin);
        match it.motion {
            Motion::Loose => buf.push(MotionKind::Loose.to_u8()),
            Motion::Flight(_) => buf.push(MotionKind::Flight.to_u8()),
            Motion::Stuck(s) => {
                buf.push(MotionKind::Stuck.to_u8());
                put_heading(buf, s.heading);
                for c in s.anchor.to_array() {
                    put_u32(buf, c as u32);
                }
            }
        }
    }
}

fn put_heading(buf: &mut Vec<u8>, h: Heading) {
    put_f32(buf, h.yaw);
    put_f32(buf, h.pitch);
}

fn get_heading(r: &mut Reader) -> Option<Heading> {
    Some(Heading {
        yaw: r.f32()?,
        pitch: r.f32()?,
    })
}

/// Read a list of item entities written by [`put_entities`]. Empty (zero-count)
/// stacks are dropped; `None` on truncated input. The reconstructed drop resumes
/// with its saved motion, lifetime and spin (the random spawn "pop" is bypassed).
/// A restored lodged item's anchor is unverified (see `Stuck::verified`).
pub fn get_entities(r: &mut Reader) -> Option<Vec<DroppedItem>> {
    let n = r.u16()? as usize;
    let mut out = Vec::with_capacity(n.min(1024));
    for _ in 0..n {
        let pos = Vec3::new(r.f32()?, r.f32()?, r.f32()?);
        let vel = Vec3::new(r.f32()?, r.f32()?, r.f32()?);
        let slot = get_item_slot(r)?;
        let ticks_lived = r.u32()?;
        let spin = r.f32()?;
        let motion = match MotionKind::from_u8(r.u8()?) {
            MotionKind::Loose => Motion::Loose,
            MotionKind::Flight => Motion::flying(vel, None),
            MotionKind::Stuck => Motion::Stuck(Stuck {
                heading: get_heading(r)?,
                anchor: IVec3::new(r.u32()? as i32, r.u32()? as i32, r.u32()? as i32),
                verified: false,
            }),
        };
        let Some(stack) = slot else {
            continue;
        };
        let mut d = DroppedItem::with_motion(pos, stack, vel, motion);
        d.ticks_lived = ticks_lived;
        d.spin = spin;
        d.prev_spin = spin;
        out.push(d);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use petramond_world::item::{ItemStack, ItemType};

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

    /// A lodged item reloads lodged, pointing the same way, anchored to
    /// the same cell but UNVERIFIED (the block may have gone meanwhile); a
    /// flight reloads as a flight with its owner forgotten (sessions do not
    /// outlive the process) and its heading re-derived from its velocity.
    #[test]
    fn motion_survives_the_entity_roundtrip() {
        use crate::entity::{Heading, Motion};
        let heading = Heading {
            yaw: 0.7,
            pitch: -0.2,
        };
        let anchor = IVec3::new(-3, 64, 7);
        let mut stuck = DroppedItem::new(
            Vec3::new(1.0, 64.0, 2.0),
            ItemStack::new(ItemType::Stone, 1),
            1,
        );
        stuck.motion = Motion::Stuck(crate::entity::Stuck {
            heading,
            anchor,
            verified: true,
        });
        let flying = DroppedItem::launched(
            Vec3::new(1.0, 64.0, 2.0),
            ItemStack::new(ItemType::Stone, 1),
            Vec3::new(3.0, 1.0, 0.0),
            Some(crate::mob::EntityRef::Player(crate::player::PlayerId(4))),
        );
        let mut buf = Vec::new();
        put_entities(&mut buf, &[stuck.clone(), flying.clone()]);
        let got = get_entities(&mut Reader::new(&buf)).expect("decodes");
        match got[0].motion {
            Motion::Stuck(s) => {
                assert_eq!(s.heading, heading);
                assert_eq!(s.anchor, anchor);
                assert!(!s.verified, "a restored anchor is re-probed");
            }
            other => panic!("a lodged item reloads lodged, not {other:?}"),
        }
        match got[1].motion {
            Motion::Flight(f) => {
                assert_eq!(f.owner, None, "an owner is a session, never saved");
                assert!(f.left_owner, "no owner left to spare");
                assert_eq!(Some(f.heading), flying.heading());
            }
            other => panic!("a flight reloads as a flight, not {other:?}"),
        }
    }

    #[test]
    fn instance_data_survives_the_entity_roundtrip() {
        use petramond_world::item::variant;
        let mut m = variant::VariantMap::new();
        m.insert("petramond:tint".into(), vec![1, 2, 3]);
        let v = variant::intern(&m).unwrap();
        let d = DroppedItem::new(
            Vec3::new(0.0, 64.0, 0.0),
            petramond_world::item::ItemStack::with_variant(ItemType::Stone, 2, v),
            1,
        );
        let mut buf = Vec::new();
        put_entities(&mut buf, &[d]);
        let got = get_entities(&mut Reader::new(&buf)).expect("decodes");
        assert_eq!(
            got[0].stack.variant, v,
            "the blob re-interns to the same id"
        );
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
