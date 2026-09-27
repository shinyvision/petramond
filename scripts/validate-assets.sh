#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# CARGO_CMD may be multi-word (e.g. `nice -n 10 cargo`), so split it into words once.
IFS=' ' read -r -a cargo_cmd <<< "${CARGO_CMD:-cargo}"
cd "$repo_root"
# shellcheck source=lib/named-tests.sh
source "$repo_root/scripts/lib/named-tests.sh"

# A quick local gate over the shipped data and shaders. Every test below also
# runs in the full suite (`make test`), so CI does not run this script; it
# exists to check an asset edit in seconds. Each list is explicit: a test that
# is renamed or moved fails the gate instead of silently dropping out of it.

# Data catalogs and their cross-references.
run_named_tests --profile fasttest -p petramond-world --lib -- \
    block::load::tests::shipped_blocks_json_loads_fully \
    block_model::tests::every_registered_model_compiles_with_geometry_and_texture \
    crafting::load::tests::shipped_catalog_parses_both_interaction_models \
    item::load::tests::shipped_items_json_loads_fully \
    loot::tests::shipped_loot_tables_resolve_and_have_bounded_expansion \
    particle_emitters::tests::shipped_particle_emitters_json_loads_fully \
    sound_registry::tests::shipped_sounds_json_loads_fully \
    structure::tests::shipped_structures_compile
run_named_tests --profile fasttest -p petramond-audio --no-default-features --lib -- \
    music_registry::tests::shipped_music_json_loads_fully_and_every_clip_resolves
run_named_tests --profile fasttest -p petramond --lib -- \
    mob::load::tests::shipped_mobs_json_loads_fully \
    player::animator::tests::shipped_player_animators_compile_against_their_rigs \
    player::rigs::tests::the_shipped_rigs_catalog_has_a_rig_per_presenter \
    gui::documents::tests::fit::every_shipped_document_fits_every_window \
    gui::documents::tests::fit::the_recipe_tooltip_grows_to_the_published_ingredient_width \
    gui::documents::tests::fit::authored_label_text_fits_the_box_the_document_gives_it \
    gui::documents::tests::fit::long_dynamic_text_never_pushes_a_widget_off_its_screen

# Every rig clip a shipped pack plays is on its rig (the packs are their own
# wasm workspace; this runs the pack's check natively).
run_named_tests --profile fasttest --manifest-path mods-src/Cargo.toml --target-dir target \
    -p combat --lib -- \
    rig_clips::every_clip_the_pack_plays_is_on_its_rig

# Parse every bundled WGSL source and, where an adapter is available, build
# every production GPU pipeline under wgpu's validation layer.
run_named_tests --profile fasttest -p petramond-render --lib -- \
    shader_pack::tests::bundled_pack_shaders_parse_and_validate \
    pipeline::gpu_validation::packed_vertex_pipeline_validates
