use super::*;
use crate::schematic::{library, CellData, SavedStack, SchematicCell};
use std::{collections::BTreeMap, path::PathBuf};

fn png() -> Vec<u8> {
    petramond_world::assets::read_bytes("textures/schematic_wand.png")
        .unwrap()
        .0
}
fn cell() -> CellData {
    CellData {
        block: "fixture:machine".into(),
        state: vec![7, 0, 0],
        state_ids: BTreeMap::from([(1, "fixture:material".into())]),
        fluid: 9,
        kv: BTreeMap::from([("fixture:opaque".into(), vec![0, 255, 9])]),
        container: Some(vec![
            None,
            Some(SavedStack {
                item: "fixture:tool".into(),
                count: 3,
                data: Vec::new(),
            }),
            None,
        ]),
        furnace: Some([33, 21, 55]),
    }
}
fn fixture() -> Schematic {
    let rich = cell();
    let mut empty = rich.clone();
    empty.block = "petramond:air".into();
    empty.state.clear();
    empty.state_ids.clear();
    empty.fluid = 0;
    empty.kv.clear();
    empty.container = None;
    empty.furnace = None;
    let mut no_slots = empty.clone();
    no_slots.container = Some(Vec::new());
    Schematic::from_cells(
        "Archive Ω".into(),
        [256, 256, 256],
        vec![
            SchematicCell {
                pos: [0, 0, 0],
                data: rich.clone(),
            },
            SchematicCell {
                pos: [2, 7, 9],
                data: empty,
            },
            SchematicCell {
                pos: [255, 254, 255],
                data: no_slots,
            },
            SchematicCell {
                pos: [255, 255, 255],
                data: rich,
            },
        ],
    )
    .unwrap()
}
fn dir() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "llschematic-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn archive_round_trip_keeps_sparse_air_all_cell_stores_and_exact_thumbnail() {
    let mut fixture = fixture();
    let variant = petramond_world::item::variant::intern(&BTreeMap::from([(
        "fixture:label".into(),
        b"engraved".to_vec(),
    )]))
    .unwrap();
    fixture.sections[0].palette[0].container.as_mut().unwrap()[1]
        .as_mut()
        .unwrap()
        .data = petramond_world::item::variant::blob(variant)
        .unwrap()
        .to_vec();
    let png = png();
    let bytes = encode(&fixture, &png).unwrap();
    assert_eq!(decode(&bytes).unwrap(), fixture);
    let h = Header::parse(&bytes[..HEADER_SIZE]).unwrap();
    assert_eq!(&bytes[h.thumbnail_offset()..h.payload_offset()], &png);
    let d = dir();
    let path = library::save(&d, &fixture, &png).unwrap();
    let entry = library::inspect(&path).unwrap();
    assert_eq!(entry.metadata.cell_count, fixture.cell_count());
    assert_eq!(library::read(&path).unwrap(), fixture);
    assert_eq!(library::thumbnail_bytes(&entry).unwrap(), png);
    assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1);
    library::delete(&path).unwrap();
    assert_eq!(std::fs::read_dir(&d).unwrap().count(), 0);
    std::fs::remove_dir(d).unwrap();
}

#[test]
fn archive_bytes_are_independent_of_selection_iteration_order() {
    let a = fixture();
    let bytes = encode(&a, &png()).unwrap();
    let mut cells: Vec<_> = a.cells().map(|c| c.to_owned()).collect();
    cells.reverse();
    let b = Schematic::from_cells(a.name.clone(), a.size, cells).unwrap();
    assert_eq!(encode(&b, &png()).unwrap(), bytes);
}

#[test]
fn voxel_encodings_round_trip_at_palette_bit_width_boundaries() {
    for palette in [1, 2, 3, 7, 8, 9, 255, 256, 257, 32768] {
        let cells: Vec<_> = (0..512).map(|i| (i * 31, i % palette)).collect();
        let bytes = voxels::encode(&cells, palette, 32768);
        let mut r = bytes::Reader::new(bytes.as_slice());
        assert_eq!(
            voxels::decode(&mut r, cells.len(), palette, 32768).unwrap(),
            cells
        );
        r.finish().unwrap();
    }
    for (cells, palette, mode) in [
        ((0..32768).map(|i| (i, 0)).collect::<Vec<_>>(), 1, 1),
        ((0..4096).map(|i| (i, i % 2)).collect::<Vec<_>>(), 2, 2),
        (vec![(0, 0), (16777215, 1)], 2, 0),
    ] {
        let volume = cells.last().unwrap().0 + 1;
        let encoded = voxels::encode(&cells, palette, volume);
        assert_eq!(encoded[0], mode);
        let mut r = bytes::Reader::new(encoded.as_slice());
        assert_eq!(
            voxels::decode(&mut r, cells.len(), palette, volume).unwrap(),
            cells
        );
        r.finish().unwrap();
    }
}

#[test]
fn archive_rejects_truncation_trailing_data_and_damage_in_every_section() {
    let bytes = encode(&fixture(), &png()).unwrap();
    for end in 0..bytes.len() {
        assert!(decode(&bytes[..end]).is_err(), "prefix {end}");
    }
    let mut extra = bytes.clone();
    extra.push(0);
    assert!(decode(&extra).is_err());
    let h = Header::parse(&bytes[..HEADER_SIZE]).unwrap();
    for offset in [
        0,
        8,
        12,
        28,
        40,
        HEADER_SIZE,
        h.thumbnail_offset(),
        h.payload_offset(),
    ] {
        let mut bad = bytes.clone();
        bad[offset] ^= 1;
        assert!(decode(&bad).is_err(), "offset {offset}");
    }
}

#[test]
fn indexing_and_preview_do_not_inflate_the_voxel_payload() {
    let d = dir();
    let path = library::save(&d, &fixture(), &png()).unwrap();
    let entry = library::inspect(&path).unwrap();
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[entry.header.payload_offset()] ^= 1;
    std::fs::write(&path, bytes).unwrap();
    assert_eq!(library::list(&d).unwrap().len(), 1);
    assert!(library::thumbnail(&entry).is_ok());
    assert!(library::read(&path).is_err());
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn archive_publish_never_overwrites_or_leaves_partial_files() {
    let d = dir();
    let path = d.join("same.llschematic");
    let original = fixture();
    let bytes = png();
    library::save_as(&path, &original, &bytes).unwrap();
    let mut replacement = original.clone();
    replacement.name = "Replacement".into();
    assert!(library::save_as(&path, &replacement, &bytes).is_err());
    assert_eq!(library::read(&path).unwrap(), original);
    assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1);
    assert!(library::save(&d, &replacement, b"not a PNG").is_err());
    assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1);
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn malformed_voxel_streams_are_rejected() {
    for bytes in [
        &[0, 8, 0][..],
        &[1, 0, 0, 0],
        &[1, 0, 2, 0],
        &[1, 0, 1, 4],
        &[2, 255],
        &[3],
    ] {
        assert!(voxels::decode(&mut bytes::Reader::new(bytes), 1, 2, 4).is_err());
    }
}

#[test]
fn repeated_rich_records_are_shared_instead_of_rejected_by_expanded_size() {
    let mut rich = cell();
    rich.kv = (0..32)
        .map(|i| (format!("fixture:{i}"), vec![0; 65536]))
        .collect();
    let schematic = Schematic::from_cells(
        "Rich".into(),
        [3, 1, 1],
        (0..3).map(|x| SchematicCell {
            pos: [x, 0, 0],
            data: rich.clone(),
        }),
    )
    .unwrap();
    assert!(
        schematic
            .cells()
            .map(|c| c.data.validate().unwrap())
            .sum::<usize>()
            > 4 * 1024 * 1024
    );
    let archive = encode(&schematic, &png()).unwrap();
    let decoded = decode(&archive).unwrap();
    assert_eq!(decoded, schematic);
    assert_eq!(decoded.sections[0].palette.len(), 1);
}

#[test]
fn large_multisection_archive_streams_without_the_old_cell_side_or_byte_caps() {
    let mut block = cell();
    block.kv.clear();
    let schematic = Schematic::from_cells(
        "Large".into(),
        [320, 16, 16],
        (0..320)
            .flat_map(|x| (0..16).flat_map(move |y| (0..16).map(move |z| [x, y, z])))
            .map(|pos| SchematicCell {
                pos,
                data: block.clone(),
            }),
    )
    .unwrap();
    assert!(schematic.cell_count() > 32768);
    assert!(
        schematic
            .cells()
            .map(|c| c.data.validate().unwrap())
            .sum::<usize>()
            > 4 * 1024 * 1024
    );
    assert_eq!(schematic.sections.len(), 20);
    assert!(schematic.sections.iter().all(|s| s.palette.len() == 1));
    let directory = dir();
    let path = library::save(&directory, &schematic, &png()).unwrap();
    assert_eq!(library::read(&path).unwrap(), schematic);
    assert_eq!(library::inspect(&path).unwrap().metadata.cell_count, 81920);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn a_header_declaring_an_absurd_design_is_refused() {
    let thumbnail = png();
    let refused = |size: [i32; 3], cell_count: usize| {
        let meta = Metadata {
            name: "Hostile".into(),
            size,
            cell_count,
            thumbnail_size: [16, 16],
        }
        .encode();
        let sections = cell_count.div_ceil(4096);
        let payload_len = (sections * section::PREFIX_SIZE) as u64;
        let h = Header::parse(&header::encode(&meta, &thumbnail, payload_len, sections)).unwrap();
        h.metadata(&meta).is_err()
    };
    assert!(
        !refused([MAX_AXIS, 16, 16], 4096),
        "the fixture itself is sound"
    );
    assert!(refused([MAX_AXIS + 1, 16, 16], 4096));
    assert!(refused([MAX_AXIS, MAX_AXIS, MAX_AXIS], MAX_CELLS + 1));

    let meta = [0u8; 8];
    let oversized = header::encode(&meta, &thumbnail, MAX_ARCHIVE_BYTES + 1, 1);
    assert!(Header::parse(&oversized).is_err());
}

/// The prefix, then a panic: a refusal has to come before the body is read.
struct PrefixOnly(std::io::Cursor<Vec<u8>>);
impl Read for PrefixOnly {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        match self.0.read(out)? {
            0 => panic!("the section body was read"),
            n => Ok(n),
        }
    }
}

#[test]
fn a_section_declaring_absurd_lengths_is_refused_before_its_body_is_read() {
    let sound = section::encode(&fixture().sections[0]).unwrap();
    for field in [16, 24] {
        let mut prefix = sound[..section::PREFIX_SIZE].to_vec();
        prefix[field..field + 8].copy_from_slice(&(u64::MAX / 2).to_le_bytes());
        let mut input = PrefixOnly(std::io::Cursor::new(prefix));
        assert!(section::decode(&mut input, [256, 256, 256]).is_err());
    }
}
