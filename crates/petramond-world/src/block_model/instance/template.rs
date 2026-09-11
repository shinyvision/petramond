use super::*;

/// Bake one cell's render geometry at a given facing into a [`ModelCellTemplate`]. Cubes
/// are grouped by the part they belong to (so each part's geometry is contiguous), and
/// within a group faces are sub-grouped by (blend route, cull gate) — every
/// [`TemplateSegment`] is one contiguous run the mesher can gate and route with a slice
/// copy. Within a segment the original `cube_idx` × `Face::ALL` order is preserved, and a
/// model with no parts/cullfaces/blend faces produces exactly the single always-run it
/// always did. `base_xform` is the facing transform with a ZERO base (see
/// [`ModelInstance::build`]).
#[allow(clippy::too_many_arguments)]
pub(super) fn bake_cell_template(
    base_xform: Mat4,
    cubes: &[ModelCube],
    cube_idx: &[u32],
    face_ao: &[[[f32; 4]; 6]],
    face_draw: &[[bool; 6]],
    face_blend: &[[bool; 6]],
    parts: &[&'static str],
    tint_parts: &[&'static str],
    appearance: impl Fn([f32; 4]) -> crate::block_model::FaceAppearance,
) -> ModelCellTemplate {
    let mut verts = Vec::new();
    let mut indices = Vec::new();
    let mut segments = Vec::new();
    let part_of =
        |ci: u32| -> Option<usize> { parts.iter().position(|p| *p == cubes[ci as usize].name) };
    // An authored cullface direction is model-space; rotate it by the facing into
    // a WORLD direction (a vector transform, so the footprint shift drops out).
    // Deliberately NOT rotated by the cube's own static tilt: a cullface names
    // the voxel neighbour that suppresses the face, and a tilted face's authored
    // direction is the nearest axis — the same reading Blockbench/Minecraft give it.
    let world_face = |face: Face| -> Face {
        let (dx, dy, dz) = face.dir();
        let w = base_xform.transform_vector3(Vec3::new(dx as f32, dy as f32, dz as f32));
        Face::ALL
            .into_iter()
            .max_by(|&a, &b| {
                let d = |f: Face| {
                    let (x, y, z) = f.dir();
                    w.dot(Vec3::new(x as f32, y as f32, z as f32))
                };
                d(a).total_cmp(&d(b))
            })
            .expect("Face::ALL is non-empty")
    };
    // Cubes are grouped by the part they belong to so each part's geometry is
    // ONE run per (blend, cull) bucket. Within a group the original `cube_idx`
    // order is preserved, so a row declaring no parts emits exactly the stream
    // it always did.
    for group in std::iter::once(None).chain((0..parts.len()).map(Some)) {
        // This group's faces in emission order, each with its route + gate.
        let mut group_faces: Vec<(u32, usize, bool, Option<Face>)> = Vec::new();
        for &ci in cube_idx.iter().filter(|&&ci| part_of(ci) == group) {
            let cube = &cubes[ci as usize];
            for slot in 0..6 {
                // Fully-transparent faces drop out entirely (see `face_draw`).
                if cube.faces[slot].is_none() || !face_draw[ci as usize][slot] {
                    continue;
                }
                let cull = cube.cull[slot].map(|d| world_face(Face::ALL[d as usize]));
                group_faces.push((ci, slot, face_blend[ci as usize][slot], cull));
            }
        }
        // Bucket order: opaque before blend, unculled before culled (`Face::ALL`
        // order) — fixed so the baked stream is deterministic.
        for blend in [false, true] {
            for cull_bucket in 0..7 {
                let bucket_cull = if cull_bucket == 0 {
                    None
                } else {
                    Some(Face::ALL[cull_bucket - 1])
                };
                let (v0, i0) = (verts.len() as u32, indices.len() as u32);
                for &(ci, slot, ..) in group_faces
                    .iter()
                    .filter(|&&(_, _, b, c)| b == blend && c == bucket_cull)
                {
                    let cube = &cubes[ci as usize];
                    let tinted = tint_parts.contains(&cube.name.as_str());
                    let double_sided = cube_is_flat_plane(cube);
                    let m = base_xform
                        * Mat4::from_translation(cube.origin)
                        * Mat4::from_quat(euler_quat(cube.rotation))
                        * Mat4::from_translation(-cube.origin);
                    let face = Face::ALL[slot];
                    let Some(bias) = render_face_bias(cube, cubes, face) else {
                        continue;
                    };
                    push_template_face(
                        &mut verts,
                        &mut indices,
                        m,
                        face,
                        cube.from,
                        cube.to,
                        bias,
                        cube.faces[slot].expect("filtered above"),
                        SHADES[face.shade_idx() as usize],
                        face_ao[ci as usize][slot],
                        tinted,
                        double_sided,
                        appearance(cube.faces[slot].expect("filtered above").uv),
                    );
                }
                let run = PartRun {
                    vert_start: v0,
                    vert_len: verts.len() as u32 - v0,
                    index_start: i0,
                    index_len: indices.len() as u32 - i0,
                };
                if run.vert_len > 0 {
                    segments.push(TemplateSegment {
                        run,
                        blend,
                        part: group.map(|g| g as u8),
                        cull: bucket_cull,
                    });
                }
            }
        }
    }
    ModelCellTemplate {
        verts,
        indices,
        segments,
    }
}

/// Append one textured cube face to a cell template. Cell light and warm tint are
/// applied later by the mesher; the baked per-corner AO folds into `shade` here.
///
/// Faces are emitted ONCE with their outward CCW winding — the model pipelines
/// cull back faces, so a solid cube's far side never ghosts through the near
/// face's cutout texels. The exception is `double_sided` (the one kept face of
/// a zero-thickness plane): it is emitted again with reversed winding (terrain's
/// `push_back_face` pattern) so a decal stays visible from both sides.
#[allow(clippy::too_many_arguments)]
fn push_template_face(
    verts: &mut Vec<ModelTemplateVertex>,
    indices: &mut Vec<u32>,
    m: Mat4,
    face: Face,
    from: Vec3,
    to: Vec3,
    bias: Vec3,
    uv: crate::bbmodel::FaceUv,
    shade: f32,
    ao: [f32; 4],
    tinted: bool,
    double_sided: bool,
    appearance: crate::block_model::FaceAppearance,
) {
    let local = face_corners(face, from, to);
    let p: [Vec3; 4] = [
        m.transform_point3(Vec3::from(local[0]) + bias),
        m.transform_point3(Vec3::from(local[1]) + bias),
        m.transform_point3(Vec3::from(local[2]) + bias),
        m.transform_point3(Vec3::from(local[3]) + bias),
    ];
    if (p[1] - p[0]).cross(p[3] - p[0]).length_squared() < 1e-9 {
        return;
    }
    // Corner UVs in `quad_box` order, per-face rotation applied. The rect is
    // inset half an atlas texel first so edge fragments can't spill onto
    // neighbouring sheet texels (see `ModelAtlas::inset_face_uv`).
    let corner_uv = uv.with_uv(atlas().inset_face_uv(uv.uv)).corner_uv();
    let mut emit = |order: [usize; 4]| {
        let start = verts.len() as u32;
        for &i in &order {
            verts.push(ModelTemplateVertex {
                pos: p[i],
                uv: corner_uv[i],
                shade: appearance.shading(shade, ao[i]),
                tinted,
                appearance,
            });
        }
        let ordered_ao = order.map(|i| ao[i]);
        indices.extend(model_face_tris(ordered_ao).map(|i| start + i));
    };
    emit([0, 1, 2, 3]);
    if double_sided {
        emit([0, 3, 2, 1]);
    }
}

/// The quad's triangulation for its corner AO: split along the darker diagonal
/// so the interpolated gradient stays symmetric — the same anisotropy fix as
/// terrain AO's `should_flip` (strict `>` leaves ties, and every AO-free face,
/// on the default split). Public because the held/dropped/icon bakes
/// (`render::item_model`) emit the same faces from the same cubes.
pub fn model_face_tris(ao: [f32; 4]) -> [u32; 6] {
    if ao[0] + ao[2] > ao[1] + ao[3] {
        [1, 2, 3, 1, 3, 0]
    } else {
        [0, 1, 2, 0, 2, 3]
    }
}
