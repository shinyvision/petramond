//! Golden section records and the migration chain.
//!
//! The fixtures under `save/fixtures/` are laid out by hand, byte by byte,
//! independent of the encoder: `section_v19*.bin` as v19 shipped (bare
//! payloads) and `section_v20*.bin` as the current layout (framed payloads).
//! `section_v*.bin` holds only palette-independent content (blocks through
//! an explicit identity palette, empty slots); `section_v*_entities.bin`
//! holds item entities and a mob, whose ids cross the process-wide palette,
//! so those are checked structurally.

use super::*;
use crate::save::format::RecordError;
use petramond_world::light::LightRgb;

const V19: &[u8] = include_bytes!("../fixtures/section_v19.bin");
const V20: &[u8] = include_bytes!("../fixtures/section_v20.bin");
const V19_ENTITIES: &[u8] = include_bytes!("../fixtures/section_v19_entities.bin");
const V20_ENTITIES: &[u8] = include_bytes!("../fixtures/section_v20_entities.bin");

/// The first framed payload of the v20 fixture (fluid) starts after the
/// version, three flags bytes and a three-id block cube.
const V20_FIRST_FRAME: usize = 1 + 3 + (2 + 3 * 2 + SECTION_VOLUME);

fn pos() -> SectionPos {
    SectionPos::new(3, -1, 5)
}

fn decode_identity(blob: &[u8]) -> Result<DecodedSection, RecordError> {
    decode_section_with(pos(), blob, &palette::Palette::identity())
}

fn inflated(blob: &[u8]) -> Vec<u8> {
    inflate(blob).expect("fixture inflates")
}

/// What the palette-independent fixture holds, whichever version wrote it.
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
fn golden_v20_decodes() {
    let (section, entities, mobs) = decode_identity(V20).expect("v20 decodes");
    assert_fixture_content(&section);
    assert!(entities.is_empty() && mobs.is_empty());
}

#[test]
fn golden_v19_migrates_to_the_same_content() {
    let (section, entities, mobs) = decode_identity(V19).expect("v19 migrates");
    assert_fixture_content(&section);
    assert!(entities.is_empty() && mobs.is_empty());
}

/// The upgrade step is a pure byte rewrite: v19 in, the golden v20 out.
#[test]
fn the_v19_step_produces_the_golden_v20_bytes() {
    for (old, new) in [(V19, V20), (V19_ENTITIES, V20_ENTITIES)] {
        let (old, new) = (inflated(old), inflated(new));
        assert_eq!((old[0], new[0]), (19, 20));
        assert_eq!(v19::upgrade(&old[1..]).expect("upgrades"), &new[1..]);
    }
}

/// Re-encoding what either fixture decodes to writes the golden v20 record
/// byte for byte: a layout change that forgets its version bump fails here.
#[test]
fn the_encoder_writes_the_golden_v20_layout() {
    let pal = palette::Palette::identity();
    for blob in [V19, V20] {
        let (section, ..) = decode_identity(blob).expect("decodes");
        let record = encode_snapshot_with(&SectionSnapshot::from_section(&section), &pal);
        assert_eq!(inflated(&record), inflated(V20));
    }
}

#[test]
fn golden_entities_and_mobs_survive_the_migration() {
    for blob in [V19_ENTITIES, V20_ENTITIES] {
        let (_, entities, _) = decode_section(pos(), blob).expect("decodes");
        assert_eq!(entities.len(), 2);
        assert_eq!(entities[0].ticks_lived, 100);
        match entities[1].motion {
            crate::entity::Motion::Stuck(s) => {
                assert_eq!(s.anchor, petramond_math::math::IVec3::new(1, 64, -3));
            }
            other => panic!("the lodged item reloads lodged, not {other:?}"),
        }
    }
}

#[test]
fn versions_outside_the_chain_are_typed_errors() {
    let mut payload = inflated(V20);
    payload[0] = SECTION_REC_VERSION + 1;
    assert!(matches!(
        decode_identity(&deflate(&payload)),
        Err(RecordError::Newer { found: 21, .. })
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

/// A flag bit this build has no payload for means a newer build wrote the
/// record: refused as such, never decoded with that payload dropped.
#[test]
fn an_unknown_flag_bit_is_refused_as_a_newer_payload() {
    let mut payload = inflated(V20);
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

/// Every payload must fill its frame exactly, and the error says which
/// payload broke and where.
#[test]
fn a_frame_that_disagrees_with_its_payload_is_corrupt_with_its_offset() {
    let mut payload = inflated(V20);
    let len = u32::from_le_bytes(payload[V20_FIRST_FRAME..][..4].try_into().unwrap());
    assert_eq!(len as usize, SECTION_VOLUME, "the first frame is the fluid");
    payload[V20_FIRST_FRAME..][..4].copy_from_slice(&(len - 1).to_le_bytes());
    assert_eq!(
        decode_identity(&deflate(&payload)).err(),
        Some(RecordError::corrupt(
            SECTION.name,
            "fluid",
            V20_FIRST_FRAME + 4
        ))
    );

    let mut long = inflated(V20);
    long.push(0);
    assert_eq!(
        decode_identity(&deflate(&long)).err(),
        Some(RecordError::corrupt(
            SECTION.name,
            "trailing bytes",
            long.len() - 1
        ))
    );

    let short = inflated(V20);
    assert!(matches!(
        decode_identity(&deflate(&short[..short.len() - 1])),
        Err(RecordError::Corrupt {
            what: "block light",
            ..
        })
    ));
}

#[test]
fn a_truncated_v19_record_fails_its_migration_with_the_payload_named() {
    let old = inflated(V19);
    assert!(matches!(
        decode_identity(&deflate(&old[..old.len() - 1])),
        Err(RecordError::Corrupt {
            what: "block light",
            ..
        })
    ));
}
