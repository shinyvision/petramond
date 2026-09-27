use crate::entity::{DroppedItem, Heading, Motion, Stuck};
use crate::save::codec::DiskSlot;
use crate::save::palette::Palette;
use crate::save::wire::{tagged_record, wire_struct, UnknownFields, Wire};
use petramond_math::math::{IVec3, Vec3};
use petramond_math::world_pos::WorldPos;
use petramond_persist::bytecodec::Reader;

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

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct StuckRecord {
    yaw: f32,
    pitch: f32,
    anchor: IVec3,
}
wire_struct!(StuckRecord { yaw, pitch, anchor });

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EntityRecord {
    pos: WorldPos,
    vel: Vec3,
    slot: DiskSlot,
    ticks_lived: u32,
    spin: f32,
    motion: u8,
    stuck: Option<StuckRecord>,
    unknown: UnknownFields,
}
tagged_record!(EntityRecord {
    1 => pos,
    2 => vel,
    3 => slot,
    4 => ticks_lived,
    5 => spin,
    6 => motion,
    7 => stuck,
});

impl EntityRecord {
    fn of(it: &DroppedItem, pal: &Palette) -> Self {
        let (motion, stuck) = match it.motion {
            Motion::Loose => (MotionKind::Loose, None),
            Motion::Flight(_) => (MotionKind::Flight, None),
            Motion::Stuck(s) => (
                MotionKind::Stuck,
                Some(StuckRecord {
                    yaw: s.heading.yaw,
                    pitch: s.heading.pitch,
                    anchor: s.anchor,
                }),
            ),
        };
        Self {
            pos: it.pos,
            vel: it.vel,
            slot: DiskSlot::of(Some(it.stack), pal),
            ticks_lived: it.ticks_lived,
            spin: it.spin,
            motion: motion.to_u8(),
            stuck,
            unknown: UnknownFields::new(),
        }
    }

    /// The live drop. `Ok(None)` means an empty stack, which is dropped; `Err` hands back the
    /// record when this build can't represent it. Motion, lifetime and spin resume from the save,
    /// skipping the random spawn "pop". A restored lodged item's anchor is unverified (see
    /// `Stuck::verified`).
    fn resolve(self, pal: &Palette) -> Result<Option<DroppedItem>, Box<EntityRecord>> {
        if !self.unknown.is_empty() {
            return Err(Box::new(self));
        }
        let stack = match self.slot.clone().resolve(pal) {
            Ok(Some(stack)) => stack,
            Ok(None) => return Ok(None),
            Err(_) => return Err(Box::new(self)),
        };
        let motion = match (MotionKind::from_u8(self.motion), self.stuck) {
            (MotionKind::Flight, _) => Motion::flying(self.vel, None),
            (MotionKind::Stuck, Some(s)) => Motion::Stuck(Stuck {
                heading: Heading {
                    yaw: s.yaw,
                    pitch: s.pitch,
                },
                anchor: s.anchor,
                verified: false,
            }),
            _ => Motion::Loose,
        };
        let mut d = DroppedItem::with_motion(self.pos, stack, self.vel, motion);
        d.ticks_lived = self.ticks_lived;
        d.spin = self.spin;
        d.prev_spin = self.spin;
        Ok(Some(d))
    }
}

pub fn put_entities(
    buf: &mut Vec<u8>,
    items: &[DroppedItem],
    kept: &[EntityRecord],
    pal: &Palette,
) {
    let records: Vec<EntityRecord> = items
        .iter()
        .map(|it| EntityRecord::of(it, pal))
        .chain(kept.iter().cloned())
        .collect();
    records.put(buf);
}

pub fn get_entities(
    r: &mut Reader,
    pal: &Palette,
) -> Option<(Vec<DroppedItem>, Vec<EntityRecord>)> {
    let mut live = Vec::new();
    let mut kept = Vec::new();
    for record in Vec::<EntityRecord>::get(r)? {
        match record.resolve(pal) {
            Ok(Some(drop)) => live.push(drop),
            Ok(None) => {}
            Err(record) => kept.push(*record),
        }
    }
    Some((live, kept))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::save::wire::{to_bytes, TaggedWriter};
    use petramond_world::item::{ItemStack, ItemType};

    fn roundtrip(items: &[DroppedItem]) -> Vec<DroppedItem> {
        let pal = Palette::identity();
        let mut buf = Vec::new();
        put_entities(&mut buf, items, &[], &pal);
        let (live, kept) = get_entities(&mut Reader::new(&buf), &pal).expect("decodes");
        assert!(kept.is_empty());
        live
    }

    #[test]
    fn entities_roundtrip_through_a_buffer() {
        let mut a = DroppedItem::new(
            WorldPos::new(1.0, 64.0, 2.0),
            ItemStack::new(ItemType::Stone, 5),
            1,
        );
        a.vel = Vec3::new(0.1, -0.2, 0.3);
        a.ticks_lived = 3000;
        a.spin = 1.25;
        let b = DroppedItem::new(
            WorldPos::new(-3.0, 70.0, 8.0),
            ItemStack::new(ItemType::Dirt, 1),
            2,
        );

        let got = roundtrip(&[a.clone(), b.clone()]);
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
    fn motion_survives_the_entity_roundtrip() {
        let heading = Heading {
            yaw: 0.7,
            pitch: -0.2,
        };
        let anchor = IVec3::new(-3, 64, 7);
        let mut stuck = DroppedItem::new(
            WorldPos::new(1.0, 64.0, 2.0),
            ItemStack::new(ItemType::Stone, 1),
            1,
        );
        stuck.motion = Motion::Stuck(Stuck {
            heading,
            anchor,
            verified: true,
        });
        let flying = DroppedItem::launched(
            WorldPos::new(1.0, 64.0, 2.0),
            ItemStack::new(ItemType::Stone, 1),
            Vec3::new(3.0, 1.0, 0.0),
            Some(crate::mob::EntityRef::Player(crate::player::PlayerId(4))),
        );
        let got = roundtrip(&[stuck.clone(), flying.clone()]);
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
            WorldPos::new(0.0, 64.0, 0.0),
            ItemStack::with_variant(ItemType::Stone, 2, v),
            1,
        );
        let got = roundtrip(&[d]);
        assert_eq!(
            got[0].stack.variant, v,
            "the blob re-interns to the same id"
        );
    }

    #[test]
    fn an_entity_this_build_cannot_represent_is_kept_not_dropped() {
        let pal = Palette::identity();
        let mut strange_item = EntityRecord::of(
            &DroppedItem::new(
                WorldPos::new(0.0, 64.0, 0.0),
                ItemStack::new(ItemType::Stone, 2),
                1,
            ),
            &pal,
        );
        strange_item.slot.item = u16::MAX - 1;
        let mut newer = EntityRecord::of(
            &DroppedItem::new(
                WorldPos::new(1.0, 64.0, 0.0),
                ItemStack::new(ItemType::Dirt, 1),
                1,
            ),
            &pal,
        );
        newer.unknown.insert(40, vec![1, 2, 3]);
        let kept = vec![strange_item, newer];

        let mut buf = Vec::new();
        put_entities(&mut buf, &[], &kept, &pal);
        let (live, back) = get_entities(&mut Reader::new(&buf), &pal).expect("decodes");
        assert!(live.is_empty());
        assert_eq!(back, kept);
        let mut again = Vec::new();
        put_entities(&mut again, &[], &back, &pal);
        assert_eq!(again, buf, "kept records re-encode unchanged");
    }

    #[test]
    fn a_record_missing_fields_reads_them_as_defaults() {
        let pal = Palette::identity();
        let mut record = Vec::new();
        let mut w = TaggedWriter::new(&mut record);
        w.field(1, &WorldPos::new(4.0, 64.0, 4.0));
        w.field(
            3,
            &DiskSlot::of(Some(ItemStack::new(ItemType::Stone, 3)), &pal),
        );
        w.finish();
        let mut list = to_bytes(&1u32);
        list.extend(record);
        let (live, kept) = get_entities(&mut Reader::new(&list), &pal).expect("decodes");
        assert!(kept.is_empty());
        assert_eq!(live[0].stack, ItemStack::new(ItemType::Stone, 3));
        assert_eq!(live[0].ticks_lived, 0);
        assert!(matches!(live[0].motion, Motion::Loose));
    }

    #[test]
    fn empty_list_roundtrips() {
        assert!(roundtrip(&[]).is_empty());
    }

    #[test]
    fn truncated_input_is_none() {
        let buf = to_bytes(&1u32);
        assert!(get_entities(&mut Reader::new(&buf), &Palette::identity()).is_none());
    }
}
