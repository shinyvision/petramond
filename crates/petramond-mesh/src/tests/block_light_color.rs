//! Coloured block light, from the baked cell to the packed vertex.
//!
//! The bake's own mixing is pinned in `world::light`; what these cover is the
//! half the mesher owns: the per-corner gather must average light PER CHANNEL
//! (an averaged hue is meaningless between two differently-coloured emitters),
//! and the three channels must survive the split across `packed2`, the `tint`
//! alpha lane and `packed`'s chroma nibble.

use super::*;
use petramond_world::light::{BlockLight6, LightRgb};
use crate::vertex::decode_vertex_light;

/// Mesh a flat stone floor under a caller-supplied block-light field, with no
/// skylight at all — so every vertex's light is purely the emitters'.
fn mesh_lit_floor(block_light: impl Fn(i32, i32, i32) -> LightRgb) -> ChunkMesh {
    let section = floor_section(Block::Stone);
    build_section_mesh(
        &section,
        SectionPos::new(0, 0, 0),
        |wx, wy, wz| {
            if in_section(wx, wy, wz) {
                section.block_raw(wx as usize, wy as usize, wz as usize)
            } else {
                Block::Air.id()
            }
        },
        |_, _, _| petramond_world::block::ShapeState::NONE,
        |_, _, _| 0,
        |_, _| 0,
        |_, _, _| 0,
        block_light,
        |_, _, _| true,
    )
}

/// Decoded block light of every floor-top vertex standing at world `x` (the
/// floor is one flat plane at y = 1, so this picks a column of corners).
fn top_lights_at_x(mesh: &ChunkMesh, x: f32) -> Vec<BlockLight6> {
    mesh.opaque
        .iter()
        .filter(|v| (v.pos[1] - 1.0).abs() < 1e-3 && (v.pos[0] - x).abs() < 1e-3)
        .map(decode_vertex_light)
        .collect()
}

/// The floor's +Y face vertices (shade index 0), excluding the side faces whose
/// upper corners also sit at y = 1.
fn floor_top(mesh: &ChunkMesh) -> Vec<&Vertex> {
    mesh.opaque
        .iter()
        .filter(|v| shade_idx(v) == 0 && (v.pos[1] - 1.0).abs() < 1e-3)
        .collect()
}

/// A saturated emitter's hue must reach the GPU intact. The three channels ride
/// three different words, so anything that drops one of them turns a purple
/// lamp grey (or red) — and nothing but a decode can catch it.
///
/// A uniformly-lit floor GREEDY-MERGES, so this also covers the merged-quad
/// emitter, which builds its own words from the merge key rather than from the
/// per-corner path (`mesh::greedy` vs `mesh::face_emit`).
#[test]
fn a_saturated_block_light_reaches_the_vertex_with_its_hue() {
    // A uniform field, so the smooth-light mean is the cell value itself and
    // the assertion is about the ENCODING, not the gather.
    let purple = LightRgb::new(12, 8, 30);
    let mesh = mesh_lit_floor(move |_, _, _| purple);
    let want = BlockLight6::from_x2(purple);
    assert!(
        want.r() != want.g() && want.g() != want.b(),
        "the fixture must actually be coloured"
    );
    let top = floor_top(&mesh);
    assert_eq!(
        top.len(),
        4,
        "a uniformly-lit floor must still merge to one quad — colour must not \
         block a merge, and this test must be reading the MERGED emitter's words"
    );
    for v in top {
        assert_eq!(decode_vertex_light(v), want);
    }
}

/// The merged-quad emitter writes the light words itself, so it can lose a
/// channel independently of the per-corner path. Two uniform floors of the same
/// BRIGHTNESS and the same RED, differing only in green and blue, must produce
/// different merged quads: identical words would mean the greedy path never
/// carried the chroma, and one lamp would render as the other's colour.
#[test]
fn a_merged_quad_carries_the_chroma_that_distinguishes_two_lamps() {
    let warm = LightRgb::new(30, 30, 6);
    let cool = LightRgb::new(30, 6, 30);
    assert_eq!(warm.luminance(), cool.luminance());
    assert_eq!(warm.r(), cool.r(), "only green/blue may differ");

    let words = |c: LightRgb| {
        let mesh = mesh_lit_floor(move |_, _, _| c);
        let top = floor_top(&mesh);
        assert_eq!(top.len(), 4, "fixture must merge into one quad");
        (top[0].packed, top[0].packed2, top[0].tint)
    };
    let (wp, wp2, wt) = words(warm);
    let (cp, cp2, ct) = words(cool);
    assert_ne!(
        (wp, wp2, wt),
        (cp, cp2, ct),
        "two lamps of equal brightness merged to the same vertex words"
    );
    // `packed2`'s light lane holds only RED, which is equal here — so the whole
    // difference has to live in the two chroma homes.
    assert_eq!(wp2 & 0x3F, cp2 & 0x3F);
    assert_ne!((wp >> 27, wt >> 24), (cp >> 27, ct >> 24));
}

/// Colourless light must still cost NOTHING: a grey cell writes no chroma bit
/// anywhere, so a white-lit world's vertices are bit-identical to the ones the
/// pre-colour engine produced.
#[test]
fn colourless_light_writes_no_chroma_bits() {
    let mesh = mesh_lit_floor(|_, _, _| LightRgb::grey(18));
    let lit: Vec<_> = mesh
        .opaque
        .iter()
        .filter(|v| (v.pos[1] - 1.0).abs() < 1e-3)
        .collect();
    assert!(!lit.is_empty());
    for v in lit {
        assert_eq!(v.tint >> 24, 0, "chroma low byte spent on grey light");
        assert_eq!(v.packed >> 27, 0, "chroma nibble spent on grey light");
        assert_eq!(
            decode_vertex_light(v),
            BlockLight6::grey(v.packed2 & 0x3F),
            "grey light must decode back to grey"
        );
    }
}

/// Two differently-coloured lamps overlapping a face: the smooth-light gather
/// must mean the CHANNELS. Averaging a hue (or a palette index) between a
/// purple corner and a green corner produces a colour that is in neither lamp —
/// the specific failure this feature exists to avoid.
#[test]
fn a_face_between_two_colours_averages_the_channels_not_the_hue() {
    let purple = LightRgb::new(24, 4, 30);
    let green = LightRgb::new(4, 30, 8);
    // Split the world down the middle of the section: cells at x < 8 are lit
    // purple, the rest green. The corner at x = 8 gathers both.
    let mesh = mesh_lit_floor(move |wx, _, _| if wx < 8 { purple } else { green });

    for c in top_lights_at_x(&mesh, 0.0) {
        assert_eq!(c, BlockLight6::from_x2(purple), "deep in the purple half");
    }
    for c in top_lights_at_x(&mesh, 16.0) {
        assert_eq!(c, BlockLight6::from_x2(green), "deep in the green half");
    }

    // On the seam, every channel must land strictly between the two lamps'
    // values for that channel — which only a per-channel mean can do.
    let seam = top_lights_at_x(&mesh, 8.0);
    assert!(!seam.is_empty(), "the seam column must be meshed");
    let p6 = BlockLight6::from_x2(purple).channels();
    let g6 = BlockLight6::from_x2(green).channels();
    let mixed = seam
        .iter()
        .find(|c| {
            (0..3).all(|i| {
                let (lo, hi) = (p6[i].min(g6[i]), p6[i].max(g6[i]));
                c.channels()[i] > lo && c.channels()[i] < hi
            })
        })
        .unwrap_or_else(|| panic!("no blended corner on the seam: {seam:?}"));
    // ... and the blend must not have collapsed into a grey of the same
    // brightness, which is what folding to a luminance would have produced.
    assert_ne!(*mixed, BlockLight6::grey(mixed.luminance()));
}
