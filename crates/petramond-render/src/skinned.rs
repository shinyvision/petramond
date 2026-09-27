//! GPU skinning for animated bodies — mobs and player bodies.
//!
//! A body's geometry never changes shape between frames, only its bone
//! transforms do, so each model's cubes are expanded ONCE into a static
//! bind-space [`SkinMesh`]: every vertex carries its position under the cube's
//! modelled static tilt (`S_cube`, see [`mob_model`](super::mob_model)), its
//! uv, its directional shade × rest-pose self-AO, the bone it rides and the
//! hideable part groups it belongs to. Per frame the CPU only poses the
//! skeleton: [`SkinBatch::push`] writes one bone palette entry per bone,
//! `G · pose[bone]` (the placement `G` folded in), and one [`SkinInstance`]
//! row with the body's tint, its sampled two-channel light and the parts it
//! hides. The vertex shader (`skinned.wgsl`) multiplies the bind-space vertex
//! by its palette entry and lights it per instance with the frame's sky
//! uniforms, and every visible body of one model draws in one instanced call.
//!
//! The mapping is exact: the CPU bake composed `G · pose[bone] · S_cube` per
//! cube and transformed the face corners by it; here `S_cube · corner` is the
//! stored vertex and `G · pose[bone]` the palette entry, so the product — and
//! the face list, the quad triangulation and the per-vertex shade — is the
//! same one (`skinned_mesh_matches_the_cpu_bake` pins it).

use glam::{Mat4, Vec3};

use super::lighting::{mul3, DynLight};
use super::mob_model::hurt_tint;
use petramond_math::face::Face;
use petramond_mesh::face::FaceShading;
use petramond_mesh::SHADES;
use petramond_world::bbmodel::{euler_quat, face_corners, Model};

pub(crate) const PART_COAT: u32 = 1;

#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct SkinVertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    pub shade: f32,
    pub bone: u32,
    pub parts: u32,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct SkinInstance {
    pub tint: [f32; 3],
    pub self_lit: f32,
    pub light: [f32; 4],
    pub bone_base: u32,
    pub hidden: u32,
    pub _pad: [u32; 2],
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct SkinLook {
    pub hurt: f32,
    pub emitter_tint: [f32; 3],
    pub emitter_self_lit: f32,
    pub light: DynLight,
    pub hidden: u32,
}

impl SkinLook {
    fn instance(self, bone_base: u32) -> SkinInstance {
        let block = self.light.block.fractions();
        SkinInstance {
            tint: mul3(hurt_tint(self.hurt), self.emitter_tint),
            self_lit: self.emitter_self_lit.clamp(0.0, 1.0),
            light: [
                self.light.sky.min(super::lighting::FULL_SKYLIGHT) as f32
                    / super::lighting::FULL_SKYLIGHT as f32,
                block[0],
                block[1],
                block[2],
            ],
            bone_base,
            hidden: self.hidden,
            _pad: [0; 2],
        }
    }
}

pub(crate) fn bone_slots(model: &Model) -> u32 {
    model.bones.len() as u32 + 1
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SkinMesh {
    pub verts: Vec<SkinVertex>,
    pub indices: Vec<u32>,
}

impl SkinMesh {
    /// Expand every cube face of `model` into bind space. `unit` is one model
    /// unit's size in world blocks (the body's render scale), used only to
    /// drop the faces the world-space bake dropped as degenerate — flat
    /// sub-cubes (legs, tails) have one pair of faces with area and the rest
    /// collapse to lines. `self_ao` is the per cube-face-corner rest-pose AO,
    /// and `parts` names each cube's part groups (by cube index).
    pub(crate) fn build(
        model: &Model,
        unit: f32,
        self_ao: Option<&[[[f32; 4]; 6]]>,
        parts: impl Fn(usize) -> u32,
    ) -> Self {
        let unbound = model.bones.len();
        let mut mesh = Self::default();
        for (ci, cube) in model.cubes.iter().enumerate() {
            let s_cube = Mat4::from_translation(cube.origin)
                * Mat4::from_quat(euler_quat(cube.rotation))
                * Mat4::from_translation(-cube.origin);
            let bone = cube.bone.min(unbound) as u32;
            let cube_parts = parts(ci);
            for (slot, face) in Face::ALL.into_iter().enumerate() {
                let Some(uv) = cube.faces[slot] else { continue };
                let local = face_corners(face, cube.from, cube.to);
                let p: [Vec3; 4] = local.map(|c| s_cube.transform_point3(Vec3::from(c)));
                let area = (p[1] - p[0]).cross(p[3] - p[0]) * (unit * unit);
                if area.length_squared() < 1e-9 {
                    continue;
                }
                let ao = self_ao
                    .and_then(|table| table.get(ci))
                    .map_or([1.0; 4], |faces| faces[slot]);
                let shade = SHADES[face.shade_idx() as usize];
                let corner_uv = uv.corner_uv();
                let start = mesh.verts.len() as u32;
                for i in 0..4 {
                    mesh.verts.push(SkinVertex {
                        pos: p[i].to_array(),
                        uv: corner_uv[i],
                        shade: shade * ao[i],
                        bone,
                        parts: cube_parts,
                    });
                }
                mesh.indices.extend(
                    petramond_world::block_model::model_face_tris(ao)
                        .into_iter()
                        .map(|i| start + i),
                );
            }
        }
        mesh
    }

    pub(crate) fn index_count(&self) -> u32 {
        self.indices.len() as u32
    }
}

#[derive(Default)]
pub(crate) struct SkinBatch {
    pub palette: Vec<[[f32; 4]; 4]>,
    pub instances: Vec<SkinInstance>,
}

impl SkinBatch {
    pub(crate) fn clear(&mut self) {
        self.palette.clear();
        self.instances.clear();
    }

    pub(crate) fn next_instance(&self) -> u32 {
        self.instances.len() as u32
    }

    pub(crate) fn push(&mut self, pose: &[Mat4], global: Mat4, slots: u32, look: SkinLook) -> u32 {
        let bone_base = self.palette.len() as u32;
        self.palette.extend((0..slots as usize).map(|slot| {
            let bone = pose.get(slot).copied().unwrap_or(Mat4::IDENTITY);
            (global * bone).to_cols_array_2d()
        }));
        let index = self.next_instance();
        self.instances.push(look.instance(bone_base));
        index
    }
}

#[cfg(test)]
pub(crate) fn skin_positions(mesh: &SkinMesh, batch: &SkinBatch, at: u32) -> Vec<Vec3> {
    let inst = batch.instances[at as usize];
    mesh.verts
        .iter()
        .filter(|v| v.parts & inst.hidden == 0)
        .map(|v| {
            let m = Mat4::from_cols_array_2d(&batch.palette[(inst.bone_base + v.bone) as usize]);
            m.transform_point3(Vec3::from(v.pos))
        })
        .collect()
}

#[cfg(test)]
mod tests;
