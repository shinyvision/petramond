use super::*;

fn workbench_over(below: Option<Block>) -> ChunkMesh {
    let mut blocks = vec![((8usize, 8usize, 8usize), Block::FurnitureWorkbench)];
    if let Some(b) = below {
        blocks.push(((8, 7, 8), b));
    }
    mesh(&section_with(&blocks))
}

#[test]
fn model_on_opaque_cube_emits_a_bounded_contact_stamp() {
    let mesh = workbench_over(Some(Block::Stone));
    assert!(
        !mesh.contact.is_empty(),
        "a model standing on stone must stamp a contact shadow"
    );
    for v in &mesh.contact {
        assert!(
            (v.pos[1] - 8.0).abs() < 1e-4,
            "the stamp lies on the supporting face's plane: {:?}",
            v.pos
        );
        assert!(
            (8.0 - 1e-4..=9.0 + 1e-4).contains(&v.pos[0])
                && (8.0 - 1e-4..=9.0 + 1e-4).contains(&v.pos[2]),
            "the stamp stays inside its own cell: {:?}",
            v.pos
        );
        assert!(
            v.darken >= 0.0 && v.darken < 1.0,
            "darkening must stay a partial multiplier: {}",
            v.darken
        );
    }
    assert!(
        mesh.contact.iter().any(|v| v.darken > 0.0),
        "the stamp actually darkens somewhere"
    );
}

#[test]
fn contact_stamp_crosses_cell_boundaries_onto_supported_neighbours() {
    let spill = |extra: &[((usize, usize, usize), Block)]| {
        let mut blocks = vec![
            ((8usize, 8usize, 8usize), Block::FurnitureWorkbench),
            ((8, 7, 8), Block::Stone),
        ];
        blocks.extend_from_slice(extra);
        mesh(&section_with(&blocks))
    };

    let supported = spill(&[((7, 7, 8), Block::Stone)]);
    assert!(
        supported.contact.iter().any(|v| v.pos[0] < 8.0 - 1e-4),
        "the stamp must spill onto the supported neighbouring floor"
    );
    assert!(
        supported.contact.iter().all(|v| v.pos[0] > 7.0 - 1e-4),
        "the spill stays within the one-cell dilation ring"
    );

    let unsupported = spill(&[]);
    assert!(
        !unsupported.contact.is_empty()
            && unsupported.contact.iter().all(|v| v.pos[0] >= 8.0 - 1e-4),
        "an unsupported neighbour cell gets no spill"
    );

    let buried = spill(&[((7, 7, 8), Block::Stone), ((7, 8, 8), Block::Stone)]);
    assert!(
        buried.contact.iter().all(|v| v.pos[0] >= 8.0 - 1e-4),
        "a buried neighbouring floor gets no spill"
    );
}

#[test]
fn model_without_an_opaque_cube_below_gets_no_stamp() {
    assert!(
        workbench_over(None).contact.is_empty(),
        "floating model: no stamp"
    );
    assert!(
        workbench_over(Some(Block::Glass)).contact.is_empty(),
        "glass below: no stamp (only opaque full cubes support one)"
    );
}
