struct Overlay { mvp: mat4x4<f32>, color: vec4<f32> };
@group(0) @binding(0) var<uniform> overlay: Overlay;
@vertex fn vs_main(@location(0) pos: vec3<f32>) -> @builtin(position) vec4<f32> {
    var p = overlay.mvp * vec4<f32>(pos, 1.0);
    p.z -= 0.00005 * p.w;
    return p;
}
@fragment fn fs_main() -> @location(0) vec4<f32> { return overlay.color; }
