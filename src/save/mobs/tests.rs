use super::*;
use crate::save::wire::to_bytes;
use petramond_world::item::{ItemStack, ItemType};

fn owl(x: f64) -> SavedMob {
    SavedMob {
        kind: Mob::Owl,
        pos: WorldPos::new(x, 64.0, 2.0),
        yaw: 1.5,
        tags: BTreeMap::new(),
        container: Default::default(),
    }
}

fn encode(mobs: &[SavedMob], kept: &[DiskMob]) -> Vec<u8> {
    let mut buf = Vec::new();
    put_mobs(&mut buf, mobs, kept, &Palette::identity());
    buf
}

fn decode(bytes: &[u8]) -> Option<DecodedMobs> {
    let mut r = Reader::new(bytes);
    let got = get_mobs(&mut r, &Palette::identity())?;
    r.is_at_end().then_some(got)
}

#[test]
fn mobs_roundtrip_through_a_buffer() {
    let a = owl(1.0);
    let b = SavedMob {
        kind: Mob::Sheep,
        pos: WorldPos::new(-3.0, 70.0, 8.0),
        yaw: -0.25,
        tags: BTreeMap::from([
            (
                crate::mob::tags::CONFINED.to_owned(),
                MobTagValue::Bool(true),
            ),
            (
                crate::mob::tags::SHEAR_REGROW.to_owned(),
                MobTagValue::Int(4321),
            ),
            (crate::mob::tags::HEALTH.to_owned(), MobTagValue::Float(3.5)),
            ("farm:quality".to_owned(), MobTagValue::Int(7)),
            (
                "farm:note".to_owned(),
                MobTagValue::String("x".repeat(70_000)),
            ),
        ]),
        container: Container {
            slots: vec![None, Some(ItemStack::new(ItemType::Coal, 9)), None],
        },
    };
    let got = decode(&encode(&[a.clone(), b.clone()], &[])).expect("decodes");
    assert!(got.kept.is_empty());
    assert_eq!(got.live, [a, b], "every persisted field survives");
}

#[test]
fn a_mob_this_build_cannot_spawn_is_kept_and_written_back() {
    let known = crate::mob::defs().len() as u8;
    assert!(known < 200);
    let stranger = DiskMob {
        species: 200,
        pos: WorldPos::new(2.0, 64.0, 2.0),
        yaw: 0.5,
        tags: BTreeMap::from([("strange:mod".to_owned(), MobTagValue::Int(9))]),
        slots: vec![
            DiskSlot::default(),
            DiskSlot::of(
                Some(ItemStack::new(ItemType::Coal, 2)),
                &Palette::identity(),
            ),
        ],
        unknown: UnknownFields::new(),
    };
    let bytes = encode(&[owl(1.0), owl(3.0)], std::slice::from_ref(&stranger));
    let got = decode(&bytes).expect("decodes despite the stranger");
    assert_eq!(got.live.len(), 2, "the known mobs spawn");
    assert_eq!(
        got.live[1].pos.x, 3.0,
        "later records decode from the right offset"
    );
    assert_eq!(got.kept, [stranger]);
    assert_eq!(
        encode(&got.live, &got.kept),
        bytes,
        "kept mobs are written back unchanged"
    );
}

#[test]
fn a_known_mob_with_unresolvable_content_is_kept_whole() {
    let pal = Palette::identity();
    let mut unknown_item = DiskMob::of(&owl(1.0), Mob::Owl.id(), &pal);
    unknown_item.slots.push(DiskSlot {
        item: u16::MAX - 1,
        count: 3,
        blob: Default::default(),
    });
    let mut newer = DiskMob::of(&owl(2.0), Mob::Owl.id(), &pal);
    newer.unknown.insert(99, vec![7]);
    let got = decode(&encode(&[], &[unknown_item.clone(), newer.clone()])).expect("decodes");
    assert!(got.live.is_empty());
    assert_eq!(got.kept, [unknown_item, newer]);
}

#[test]
fn empty_list_roundtrips() {
    let got = decode(&encode(&[], &[])).expect("decodes");
    assert!(got.live.is_empty() && got.kept.is_empty());
}

#[test]
fn a_shared_tag_key_is_stored_once_and_read_back_by_every_mob() {
    let key = crate::mob::tags::CONFINED;
    let penned = |x| SavedMob {
        kind: Mob::Sheep,
        pos: WorldPos::new(x, 64.0, 2.0),
        yaw: 0.0,
        tags: BTreeMap::from([(key.to_owned(), MobTagValue::Bool(true))]),
        container: Default::default(),
    };
    let buf = encode(&[penned(1.0), penned(2.0)], &[]);
    assert_eq!(
        buf.windows(key.len())
            .filter(|w| *w == key.as_bytes())
            .count(),
        1,
        "the shared key rides the table, not each mob's record"
    );
    let got = decode(&buf).expect("decodes");
    assert_eq!(got.live.len(), 2);
    assert!(
        got.live
            .iter()
            .all(|m| m.tags.get(key) == Some(&MobTagValue::Bool(true))),
        "both mobs resolve their tag through the shared table"
    );
}

#[test]
fn a_tag_key_index_outside_the_table_is_rejected() {
    let list = MobList {
        keys: Vec::new(),
        mobs: vec![MobRecord {
            species: Mob::Owl.id(),
            pos: WorldPos::new(1.0, 64.0, 2.0),
            yaw: 0.5,
            tags: vec![(0, MobTagValue::Bool(true))],
            slots: Vec::new(),
            unknown: UnknownFields::new(),
        }],
    };
    assert!(decode(&to_bytes(&list)).is_none());
}

#[test]
fn truncated_input_is_none() {
    let mut buf = encode(&[owl(1.0)], &[]);
    buf.pop();
    assert!(decode(&buf).is_none());
}
