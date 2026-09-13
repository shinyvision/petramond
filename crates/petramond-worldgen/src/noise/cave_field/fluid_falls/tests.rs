use super::*;
use crate::data::underground;
use petramond_world::chunk::{SectionPos, SECTION_MIN_CY};
use petramond_world::fluid_math::{is_falling, is_source};

const BAND: (i32, i32) = (-38, -11);
const SURF: i32 = 70;

/// Chunks nearest the origin first: valid fall geometry is rare even where
/// every column rolls, so a test walks outward until it has what it needs.
fn chunks_outward() -> Vec<(i32, i32)> {
    let mut chunks: Vec<(i32, i32)> = (-6..6).flat_map(|z| (-6..6).map(move |x| (x, z))).collect();
    chunks.sort_by_key(|&(x, z)| x * x + z * z);
    chunks
}

/// A synthetic fall row every column rolls, so a chunk or two exercises both
/// placements whatever the shipped row is tuned to.
fn falls_field(seed: u32) -> CaveField {
    let layer = format!(
        r#"{{"fluid_falls":[{{"fluid_fall":"test:lava","fluid":"petramond:lava",
        "chance":1.0,"y":[{},{}],"min_surface":45}}]}}"#,
        BAND.0, BAND.1
    );
    CaveField::with_table(seed, underground::synthetic_table(&[&layer]))
}

/// A fall's source replaces ROCK of the cave model and touches the cave
/// through its exit alone, so a generated source never hangs in open space
/// or pours every way once the on-load kick arms it. The pour is one open
/// column under the exit, inside the chunk, ending on rock.
#[test]
fn fall_sources_replace_enclosed_rock_and_pour_through_one_face() {
    let field = falls_field(0x1A7A_F000);
    let carved = |[x, y, z]: [i32; 3]| field.cave_carved(x, y, z, SURF);
    let chunk = |v: i32| v.div_euclid(SECTION_SIZE as i32);
    const ENOUGH: usize = 2;
    let (mut ceiling, mut wall) = (0usize, 0usize);
    for (cx, cz) in chunks_outward() {
        if ceiling >= ENOUGH && wall >= ENOUGH {
            break;
        }
        for fall in field.chunk_falls(cx, cz).falls.iter() {
            let [sx, sy, sz] = fall.source;
            assert!(
                (BAND.0..=BAND.1).contains(&sy),
                "source {sy} outside the band"
            );
            assert_eq!([chunk(sx), chunk(sz)], [cx, cz], "source outside its chunk");
            assert!(
                !carved(fall.source),
                "source {:?} sits in open space",
                fall.source
            );
            let (exit, exit_meta) = fall.pour[0];
            let d = [exit[0] - sx, exit[1] - sy, exit[2] - sz];
            assert!(
                d.iter().map(|v| v.abs()).sum::<i32>() == 1 && d[1] <= 0,
                "exit {exit:?} is not under or beside source {:?}",
                fall.source
            );
            for n in [
                [sx + 1, sy, sz],
                [sx - 1, sy, sz],
                [sx, sy + 1, sz],
                [sx, sy - 1, sz],
                [sx, sy, sz + 1],
                [sx, sy, sz - 1],
            ] {
                assert!(
                    n == exit || !carved(n),
                    "source {:?} is open toward {n:?}",
                    fall.source
                );
            }
            if d[1] == -1 {
                ceiling += 1;
                assert!(
                    is_falling(exit_meta),
                    "a ceiling pour falls out of its source"
                );
            } else {
                wall += 1;
                assert!(
                    !is_source(exit_meta) && !is_falling(exit_meta),
                    "a wall pour's exit is a flowing cell"
                );
                assert!(fall.pour.len() >= 2, "a wall pour at {exit:?} has no drop");
                assert_eq!(
                    [chunk(exit[0]), chunk(exit[2])],
                    [cx, cz],
                    "pour left its chunk"
                );
            }
            for (i, &(cell, meta)) in fall.pour.iter().enumerate() {
                assert_eq!(
                    cell,
                    [exit[0], exit[1] - i as i32, exit[2]],
                    "pour is not one column"
                );
                assert!(carved(cell), "pour cell {cell:?} is rock");
                assert!(
                    i == 0 || is_falling(meta),
                    "pour cell {cell:?} is not falling"
                );
            }
            let last = fall.pour[fall.pour.len() - 1].0;
            assert!(
                last[1] == CAVE_MIN_Y || !carved([last[0], last[1] - 1, last[2]]),
                "the fall under {exit:?} stops short of its landing"
            );
        }
    }
    assert!(
        ceiling >= ENOUGH && wall >= ENOUGH,
        "found {ceiling} ceiling and {wall} wall pours"
    );
}

/// The section stamp applies the rule to carved terrain: every still source
/// it writes is enclosed by rock on five sides — another fall's source counts,
/// being rock the cave left — and feeds exactly one pour, including sources
/// whose pour lies in the section below.
#[test]
fn stamped_fall_sources_are_enclosed_over_their_pours() {
    let field = falls_field(0x1A7A_F0FA);
    let surf = [SURF; 256];
    let sec = SECTION_SIZE as i32;
    let mut sources = 0usize;
    let with_falls = chunks_outward()
        .into_iter()
        .filter(|&(cx, cz)| {
            let mut any = false;
            field
                .chunk_falls(cx, cz)
                .cells([i32::MIN; 3], [i32::MAX; 3], |_| any = true);
            any
        })
        .take(2);
    for (cx, cz) in with_falls {
        let sections: Vec<Section> = (SECTION_MIN_CY..=BAND.1.div_euclid(sec))
            .map(|cy| {
                let mut section = Section::new(cx, cy, cz);
                section.edit_ids_bulk(|ids| ids.fill(Block::Stone.id()));
                field.carve_section(&mut section, &surf);
                section.recompute_opaque_count();
                crate::section_memo::stamp_falls(&field, SectionPos::new(cx, cy, cz), &mut section);
                section
            })
            .collect();
        let at = |x: i32, wy: i32, z: i32| -> Option<(Block, u8)> {
            let s = sections.get((wy.div_euclid(sec) - SECTION_MIN_CY) as usize)?;
            if !(0..sec).contains(&x) || !(0..sec).contains(&z) {
                return None;
            }
            let ly = wy.rem_euclid(sec) as usize;
            Some((
                s.block(x as usize, ly, z as usize),
                s.fluid_meta(x as usize, ly, z as usize),
            ))
        };
        for wy in CAVE_MIN_Y..=BAND.1 {
            for z in 0..sec {
                for x in 0..sec {
                    let Some((here, 0)) = at(x, wy, z) else {
                        continue;
                    };
                    if !here.is_fluid() {
                        continue;
                    }
                    sources += 1;
                    let mut pours = 0;
                    for (dx, dy, dz) in [
                        (1, 0, 0),
                        (-1, 0, 0),
                        (0, 1, 0),
                        (0, -1, 0),
                        (0, 0, 1),
                        (0, 0, -1),
                    ] {
                        // A chunk edge is judged by the cave model, not here.
                        let Some((block, meta)) = at(x + dx, wy + dy, z + dz) else {
                            continue;
                        };
                        let near = format!(
                            "source at ({x},{wy},{z}) of chunk ({cx},{cz}) toward {dx},{dy},{dz}"
                        );
                        if block.is_fluid() && !is_source(meta) {
                            assert!(dy <= 0, "{near} feeds upward");
                            pours += 1;
                        } else {
                            assert!(block.is_fluid() || block.is_solid(), "{near} is open");
                        }
                    }
                    assert_eq!(pours, 1, "source at ({x},{wy},{z}) of chunk ({cx},{cz})");
                }
            }
        }
    }
    assert!(sources > 0, "no stamped source");
}
