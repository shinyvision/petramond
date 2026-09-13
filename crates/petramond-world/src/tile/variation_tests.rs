use super::*;

#[test]
fn alternatives_inherit_material_and_do_not_animate() {
    let (base, _) = crate::assets::read_base_text("textures/atlas.json").unwrap();
    let layer = r#"{"tiles":[{"name":"stone","file":"stone.png","variants":["dirt.png","sand.png"],"variation":"cell","tint":"foliage","icon_tint":"grass","fill_cutout_mips":true}]}"#;
    let d = build(&[&base, layer]).unwrap();
    let base = d.by_name["stone"].index();
    let count = d.cells[base].variation_count as usize;
    assert_eq!(count, 3);
    assert_eq!(d.cells[base].variation, Some(VariationSelect::Cell));
    for cell in &d.cells[base..base + count] {
        assert_eq!(cell.anim_frames, 0);
        assert_eq!(cell.frame, 0);
        assert_eq!(cell.world_tint, Some(TileTint::Foliage));
        assert_eq!(cell.icon_tint, Some(TileTint::Grass));
        assert!(cell.fill_cutout_mips);
    }
    assert!(d.cells[base + 1..base + count]
        .iter()
        .all(|c| c.variation_count == 1 && c.variation.is_none()));
    assert_eq!(d.cells[base + 1].file, "dirt.png");
    assert_eq!(d.cells[base + 2].file, "sand.png");
}

#[test]
fn animation_and_variants_are_mutually_exclusive() {
    let invalid =
        r#"{"tiles":[{"name":"test","file":"unused.png","anim":true,"variants":["unused.png"]}]}"#;
    assert!(build(&[invalid]).err().unwrap().contains("cannot combine"));
}

#[test]
fn a_selector_requires_alternatives_to_select_among() {
    for select in ["face", "cell"] {
        let invalid = format!(
            r#"{{"tiles":[{{"name":"test","file":"unused.png","variation":"{select}"}}]}}"#
        );
        assert!(build(&[&invalid])
            .err()
            .unwrap()
            .contains("without variants"));
    }
}

#[test]
fn variation_selection_stays_inside_its_tile_family() {
    for tile in Tile::all() {
        let count = tile.variation_count();
        let picked: std::collections::HashSet<_> = (0..count as u32)
            .map(|seed| tile.variation(seed).index())
            .collect();
        assert_eq!(picked.len(), count);
        for seed in [0, 1, 127, u32::MAX] {
            assert!((tile.index()..tile.index() + count).contains(&tile.variation(seed).index()));
        }
    }
}

#[test]
fn only_face_selecting_tiles_vary_per_face() {
    let (base, _) = crate::assets::read_base_text("textures/atlas.json").unwrap();
    let layer = r#"{"tiles":[
        {"name":"test_face","file":"stone.png","variants":["dirt.png","sand.png"],"variation":"face"},
        {"name":"test_cell","file":"stone.png","variants":["dirt.png","sand.png"],"variation":"cell"},
        {"name":"test_plain","file":"stone.png","variants":["dirt.png","sand.png"]}]}"#;
    let d = build(&[&base, layer]).unwrap();
    // The face selector's arithmetic against the synthetic table (the tile
    // methods read the process-wide manifest).
    let select = |name: &str, cell: [i32; 3], normal: u32| {
        let t = d.by_name[name];
        let meta = &d.cells[t.index()];
        match meta.variation {
            Some(VariationSelect::Face) => {
                t.index() + (spatial_hash(cell, normal) % meta.variation_count as u32) as usize
            }
            _ => t.index(),
        }
    };
    let chosen: std::collections::HashSet<_> = (0..64)
        .flat_map(|x| (1..=6).map(move |n| select("test_face", [x, -17, 31], n)))
        .collect();
    assert_eq!(
        chosen.len(),
        3,
        "a face selector exercises every alternative"
    );
    for name in ["test_cell", "test_plain"] {
        assert_eq!(select(name, [5, 6, 7], 3), d.by_name[name].index());
    }
}

#[test]
fn the_spatial_hash_does_not_repeat_across_section_seams() {
    let mut seen = std::collections::HashSet::new();
    for x in -64..64 {
        for normal in 1..=6 {
            assert!(seen.insert(spatial_hash([x * 16, -16, 16], normal)));
        }
    }
}

#[test]
fn a_flipbook_rate_is_authored_one_way() {
    let row = |rate: &str| format!(r#"{{"tiles":[{{"name":"test","file":"unused.png",{rate}}}]}}"#);
    for (rate, error) in [
        (r#""frame_ticks":2,"fps":10"#, "both"),
        (r#""fps":0"#, "fps must be positive"),
        (r#""frame_ticks":-1"#, "frame_ticks must be positive"),
    ] {
        let err = build(&[&row(rate)]).err().unwrap();
        assert!(err.contains(error), "{rate}: {err}");
    }
}
