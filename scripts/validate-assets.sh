#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# CARGO may be multi-word (Makefile default: nice -n 10 cargo), so split it into words once.
IFS=' ' read -r -a cargo_cmd <<< "${CARGO:-cargo}"
cd "$repo_root"

# Data catalogs and their cross-references.
"${cargo_cmd[@]}" test -p petramond-world --lib shipped_
"${cargo_cmd[@]}" test -p petramond-world --lib \
    block_model::tests::every_registered_model_compiles_with_geometry_and_texture -- --exact
"${cargo_cmd[@]}" test -p petramond --lib \
    mob::load::tests::shipped_mobs_json_loads_fully -- --exact
"${cargo_cmd[@]}" test -p petramond --lib \
    gui::documents::tests::every_shipped_document_fits_the_smallest_viewport -- --exact

# Parse every bundled WGSL source and, where an adapter is available, build
# every production GPU pipeline under wgpu's validation layer.
"${cargo_cmd[@]}" test -p petramond-render --lib \
    shader_pack::tests::bundled_pack_shaders_parse_and_validate -- --exact
"${cargo_cmd[@]}" test -p petramond-render --lib \
    pipeline::gpu_validation::packed_vertex_pipeline_validates -- --exact
