@group(0) @binding(0) var<storage, read_write> results: array<vec2<u32>>;

@compute @workgroup_size(64)
fn check(@builtin(global_invocation_id) id: vec3<u32>) {
    let origin = select(vec3<i32>(0), vec3<i32>(16777216, 0, -16777216), id.x >= 64u);
    let cell = origin + vec3<i32>(i32(id.x % 64u) - 32, -17, i32(id.x % 9u) - 4);
    let base = block_variant_layer(7u, cell, 100u);
    var errors = 0u;
    let normals = array<vec3<f32>, 6>(vec3<f32>(1,0,0), vec3<f32>(-1,0,0),
        vec3<f32>(0,1,0), vec3<f32>(0,-1,0), vec3<f32>(0,0,1), vec3<f32>(0,0,-1));
    let donor_steps = array<vec3<i32>, 6>(vec3<i32>(0,1,-1), vec3<i32>(0,1,1),
        vec3<i32>(1,0,-1), vec3<i32>(1,0,1), vec3<i32>(1,1,0), vec3<i32>(-1,1,0));
    for (var i = 0u; i < 6u; i++) {
        let point = vec3<f32>(cell - origin) + vec3<f32>(0.5) + normals[i] * 0.5;
        let own = variation_cell(point, origin, normals[i]);
        if (any(own != cell) || block_variant_layer(7u, own, 100u) != base) { errors |= 1u; }
        for (var shift = -32; shift <= 32; shift += 16) {
            let delta = vec3<i32>(shift, 16, -shift);
            let rebased = variation_cell(point - vec3<f32>(delta), origin + delta, normals[i]);
            if (any(rebased != cell)) { errors |= 2u; }
        }
        let donor = variation_donor_offset(vec2<i32>(1,-1), i + 1u);
        if (any(donor != donor_steps[i])) { errors |= 4u; }
        let neighbor = cell + donor_steps[i];
        let neighbor_point = vec3<f32>(neighbor - origin) + vec3<f32>(0.5) + normals[i] * 0.5;
        let neighbor_cell = variation_cell(neighbor_point, origin, normals[i]);
        if (block_variant_layer(7u, own + donor, 100u) != block_variant_layer(7u, neighbor_cell, 100u)) { errors |= 8u; }
    }
    if (block_variant_layer(107u, cell, 100u) != base + 100u) { errors |= 16u; }
    if (base < 7u || base >= 11u) { errors |= 32u; }
    if (block_variant_layer(6u, cell, 100u) != 6u || block_variant_layer(8u, cell, 100u) != 8u) { errors |= 64u; }
    results[id.x] = vec2<u32>(errors, base);
}
