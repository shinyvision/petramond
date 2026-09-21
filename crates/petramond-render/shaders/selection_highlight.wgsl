struct SelectionBounds { lo: vec4<i32>, hi: vec4<i32> };
@group(0) @binding(2) var<storage, read> selection_cells: array<vec4<i32>>;
@group(0) @binding(3) var<uniform> selection_bounds: SelectionBounds;

fn selected_cell(cell: vec3<i32>) -> bool {
    let count = u32(selection_bounds.lo.w);
    if (count == 0u || any(cell < selection_bounds.lo.xyz) || any(cell >= selection_bounds.hi.xyz)) {
        return false;
    }
    var node = 0u;
    loop {
        if (node >= count) { break; }
        let lo = selection_cells[node * 2u];
        let hi = selection_cells[node * 2u + 1u];
        if (all(cell >= lo.xyz) && all(cell < hi.xyz)) {
            if (lo.w != 0) { return true; }
            node += 1u;
        } else {
            node = u32(hi.w);
        }
    }
    return false;
}

fn selection_brighten(color: vec3<f32>, local: vec3<f32>, normal: vec3<f32>, origin: vec3<i32>) -> vec3<f32> {
    if (selection_bounds.lo.w == 0) { return color; }
    // Move surface boundaries into their owning cell before restoring integer world coordinates.
    let cell = vec3<i32>(floor(local - normal * 0.001)) + origin;
    if (selected_cell(cell)) { return mix(color, vec3<f32>(1.0), 0.07); }
    return color;
}

fn selection_model_normal(view: vec3<f32>) -> vec3<f32> {
    if (selection_bounds.lo.w == 0) { return vec3<f32>(0.0); }
    let cross_normal = cross(dpdx(view), dpdy(view));
    let normal = cross_normal / max(length(cross_normal), 0.00001);
    return select(normal, -normal, dot(normal, view) > 0.0);
}
