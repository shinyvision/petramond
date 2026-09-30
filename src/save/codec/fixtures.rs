use super::*;
use crate::save::format::RecordError;
use petramond_world::light::LightRgb;

const V19: &[u8] = include_bytes!("../fixtures/section_v19.bin");
const V20: &[u8] = include_bytes!("../fixtures/section_v20.bin");
const V21: &[u8] = include_bytes!("../fixtures/section_v21.bin");
const V19_ENTITIES: &[u8] = include_bytes!("../fixtures/section_v19_entities.bin");
const V20_ENTITIES: &[u8] = include_bytes!("../fixtures/section_v20_entities.bin");
const V21_ENTITIES: &[u8] = include_bytes!("../fixtures/section_v21_entities.bin");

const V21_FIRST_FRAME: usize = 1 + 3 + (2 + 3 * 2 + SECTION_VOLUME);

fn pos() -> SectionPos {
    SectionPos::new(3, -1, 5)
}

fn decode_identity(blob: &[u8]) -> Result<DecodedSection, RecordError> {
    decode_section(pos(), blob, &palette::Palette::identity())
}

fn inflated(blob: &[u8]) -> Vec<u8> {
    inflate(blob).expect("fixture inflates")
}

fn assert_fixture_content(section: &Section) {
    let ids: Vec<u16> = section.block_cube().iter().collect();
    assert_eq!((ids[0], ids[100], ids[200]), (0, 1, 2));
    assert_eq!(ids.iter().filter(|&&id| id != 0).count(), 2);
    let fluid = section.fluid_arc().expect("fluid persisted");
    assert_eq!(fluid[300], 0x23);
    assert_eq!(fluid.iter().filter(|&&m| m != 0).count(), 1);
    assert_eq!(
        section.furnaces().get(&100),
        Some(&Furnace {
            cook_progress: 431,
            burn_remaining: 1200,
            burn_max: 4800,
        })
    );
    assert_eq!(
        section.cell_states().get(&200).map(|s| s.bytes().to_vec()),
        Some(vec![5, 7])
    );
    assert_eq!(
        section.cell_kv().get(&7),
        Some(&BTreeMap::from([("test:k".to_owned(), vec![9, 8])]))
    );
    let chest = section.containers().get(&100).expect("container persisted");
    assert_eq!(chest.slots, vec![None; 3]);
    let sky = section.skylight_arc().expect("skylight persisted");
    assert!(sky.iter().enumerate().all(|(i, &s)| s == (i % 16) as u8));
    let bl = section.blocklight_arc().expect("block light persisted");
    assert_eq!(bl[100], LightRgb::new(9, 2, 30));
    assert_eq!(bl.iter().filter(|&&c| c != LightRgb::ZERO).count(), 1);
    assert!(!section.light_dirty, "persisted light loads clean");
}

#[test]
fn every_fixture_version_decodes_to_the_same_content() {
    for (version, blob) in [(19, V19), (20, V20), (21, V21)] {
        let decoded = decode_identity(blob).unwrap_or_else(|e| panic!("v{version}: {e}"));
        assert_fixture_content(&decoded.section);
        assert!(decoded.entities.is_empty() && decoded.mobs.is_empty());
        assert!(decoded.kept.is_empty());
    }
}

#[test]
fn each_step_produces_the_next_fixture_bytes() {
    type FixtureStep<'a> = (
        &'a [u8],
        &'a [u8],
        fn(&[u8]) -> Result<Vec<u8>, RecordError>,
    );
    let steps: [FixtureStep<'_>; 4] = [
        (V19, V20, v19::upgrade),
        (V19_ENTITIES, V20_ENTITIES, v19::upgrade),
        (V20, V21, v20::upgrade),
        (V20_ENTITIES, V21_ENTITIES, v20::upgrade),
    ];
    for (old, new, step) in steps {
        let (old, new) = (inflated(old), inflated(new));
        assert_eq!(new[0], old[0] + 1);
        assert_eq!(step(&old[1..]).expect("upgrades"), &new[1..]);
    }
}

#[test]
fn the_encoder_writes_the_v21_layout() {
    let pal = palette::Palette::identity();
    let fixtures = [
        ([V19, V20, V21], V21),
        ([V19_ENTITIES, V20_ENTITIES, V21_ENTITIES], V21_ENTITIES),
    ];
    for (blobs, fixture) in fixtures {
        for blob in blobs {
            let decoded = decode_identity(blob).expect("decodes");
            let mut snap = SectionSnapshot::from_section(&decoded.section);
            snap.entities = decoded.entities;
            snap.mobs = decoded.mobs;
            snap.kept = decoded.kept;
            let record = encode_snapshot(&snap, &pal);
            assert_eq!(inflated(&record), inflated(fixture));
        }
    }
}

#[test]
fn fixture_entities_and_mobs_survive_the_migration() {
    for blob in [V19_ENTITIES, V20_ENTITIES, V21_ENTITIES] {
        let decoded = decode_identity(blob).expect("decodes");
        let entities = &decoded.entities;
        assert_eq!(entities.len(), 2);
        assert_eq!(entities[0].ticks_lived, 100);
        match entities[1].motion {
            crate::entity::Motion::Stuck(s) => {
                assert_eq!(s.anchor, petramond_math::math::IVec3::new(1, 64, -3));
            }
            other => panic!("the lodged item reloads lodged, not {other:?}"),
        }
        assert_eq!(decoded.mobs.len(), 1);
        assert_eq!(
            decoded.mobs[0].tags.get("d:str"),
            Some(&crate::mob::MobTagValue::String("hi".into()))
        );
    }
}

#[test]
fn versions_outside_the_chain_are_typed_errors() {
    let mut payload = inflated(V21);
    payload[0] = SECTION_REC_VERSION + 1;
    assert!(matches!(
        decode_identity(&deflate(&payload)),
        Err(RecordError::Newer { found: 22, .. })
    ));
    payload[0] = 18;
    assert!(matches!(
        decode_identity(&deflate(&payload)),
        Err(RecordError::Retired {
            found: 18,
            oldest: 19,
            ..
        })
    ));
}

#[test]
fn an_unknown_flag_bit_is_refused_as_a_newer_payload() {
    let mut payload = inflated(V21);
    payload[3] |= 0x80;
    let err = decode_identity(&deflate(&payload)).err().expect("refused");
    assert_eq!(
        err,
        RecordError::UnknownPayload {
            format: SECTION.name,
            flags: 0x80 << 16,
        }
    );
    assert!(err.is_from_newer_build());
}

#[test]
fn a_frame_that_disagrees_with_its_payload_is_corrupt_with_its_offset() {
    let mut payload = inflated(V21);
    let len = u32::from_le_bytes(payload[V21_FIRST_FRAME..][..4].try_into().unwrap());
    assert_eq!(len as usize, SECTION_VOLUME, "the first frame is the fluid");
    payload[V21_FIRST_FRAME..][..4].copy_from_slice(&(len - 1).to_le_bytes());
    assert_eq!(
        decode_identity(&deflate(&payload)).err(),
        Some(RecordError::corrupt(
            SECTION.name,
            "fluid",
            V21_FIRST_FRAME + 4
        ))
    );

    let mut long = inflated(V21);
    long.push(0);
    assert_eq!(
        decode_identity(&deflate(&long)).err(),
        Some(RecordError::corrupt(
            SECTION.name,
            "trailing bytes",
            long.len() - 1
        ))
    );

    let short = inflated(V21);
    assert!(matches!(
        decode_identity(&deflate(&short[..short.len() - 1])),
        Err(RecordError::Corrupt {
            what: "block light",
            ..
        })
    ));
}

#[test]
fn a_truncated_old_record_fails_its_migration_with_the_payload_named() {
    for blob in [V19, V20] {
        let old = inflated(blob);
        assert!(matches!(
            decode_identity(&deflate(&old[..old.len() - 1])),
            Err(RecordError::Corrupt {
                what: "block light",
                ..
            })
        ));
    }
}
