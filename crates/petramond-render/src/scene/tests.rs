use super::*;
use petramond_math::world_pos::WorldPos;
use petramond_world::item::ItemType;

/// A settled drop with `prev_pos == pos`, so it bakes to `pos` at any alpha.
fn fresh_drop(pos: WorldPos, item: ItemType) -> DroppedItemPresentation {
    DroppedItemPresentation {
        variant: petramond_world::item::VariantId::NONE,
        prev_pos: pos,
        pos,
        item,
        count: 1,
        prev_spin: 0.0,
        spin: 0.0,
        prev_flight: None,
        flight: None,
        skylight: 0,
        blocklight: petramond_world::light::BlockLight6::DARK,
    }
}

fn particle_row(atlas: ParticleAtlas) -> ParticlePresentation {
    ParticlePresentation {
        quad_axes: None,
        atlas,
        pos: WorldPos::new(0.0, 64.0, 0.0),
        uv_min: [0.0, 0.0],
        uv_size: [0.0625; 2],
        tint: [1.0, 1.0, 1.0],
        alpha: 1.0,
        size: 0.1,
        stretch: 1.0,
        skylight: 0,
        blocklight: petramond_world::light::BlockLight6::DARK,
    }
}

#[test]
fn bake_item_entities_one_instance_per_drop() {
    let drops = vec![
        fresh_drop(WorldPos::new(1.0, 2.0, 3.0), ItemType::Dirt),
        fresh_drop(WorldPos::new(4.0, 5.0, 6.0), ItemType::Stone),
    ];
    let mut out = Vec::new();
    bake_item_entities(&drops, 1.0, &mut out);
    assert_eq!(out.len(), 2);
    // A fresh drop has prev_pos == pos, so any alpha bakes its live position.
    assert_eq!(out[0].pos, drops[0].pos);
    assert_eq!(out[0].item, ItemType::Dirt);
    assert_eq!(out[1].item, ItemType::Stone);
}

#[test]
fn bake_item_entities_interpolates_between_ticks() {
    // A drop that moved last tick (prev_pos != pos) bakes at the blended position,
    // so it renders smoothly between the 20 TPS physics ticks.
    let drop = DroppedItemPresentation {
        prev_pos: WorldPos::new(0.0, 64.0, 0.0),
        pos: WorldPos::new(2.0, 64.0, 0.0),
        ..fresh_drop(WorldPos::ZERO, ItemType::Dirt)
    };
    let mut out = Vec::new();
    bake_item_entities(std::slice::from_ref(&drop), 0.5, &mut out);
    assert_eq!(
        out[0].pos,
        WorldPos::new(1.0, 64.0, 0.0),
        "halfway between prev and current"
    );
}

#[test]
fn bake_item_entities_reuses_the_vec_without_growth() {
    let drops: Vec<_> = (0..8)
        .map(|i| {
            fresh_drop(
                petramond_math::world_pos::WorldPos::new(i as f64, i as f64, i as f64),
                ItemType::Dirt,
            )
        })
        .collect();
    let mut out = Vec::new();
    bake_item_entities(&drops, 1.0, &mut out);
    let cap = out.capacity();
    // Fewer drops -> identical-or-smaller count, so the cleared+refilled buffer
    // keeps its capacity: rebuilding never reallocs.
    bake_item_entities(&drops[..2], 1.0, &mut out);
    assert_eq!(out.len(), 2);
    assert_eq!(out.capacity(), cap, "instance buffer reused");
}

#[test]
fn bake_particles_splits_rows_by_atlas() {
    // Each row routes by its atlas tag: BLOCK rows into the block list, MODEL rows
    // into the model list, with no cross-contamination. The block-vs-model decision
    // lives upstream in presentation::collect_particles; the bake only routes.
    let particles = vec![
        particle_row(ParticleAtlas::Block),
        particle_row(ParticleAtlas::Model),
        particle_row(ParticleAtlas::Block),
        particle_row(ParticleAtlas::Solid),
    ];
    let mut block_out = Vec::new();
    let mut model_out = Vec::new();
    let mut solid_out = Vec::new();
    bake_particles(&particles, &mut block_out, &mut model_out, &mut solid_out);
    assert_eq!(block_out.len(), 2, "block rows route to the block list");
    assert_eq!(model_out.len(), 1, "model rows route to the model list");
    assert_eq!(solid_out.len(), 1, "solid rows route to the blended list");
    assert_eq!(
        solid_out[0].color, block_out[0].tint,
        "a solid particle's tint IS its color"
    );
}
