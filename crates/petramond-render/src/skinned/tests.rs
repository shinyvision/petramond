use super::*;
use crate::item_model::ItemVertex;
use crate::lighting::{LightEnv, FULL_SKYLIGHT};
use crate::mob_model::{bake_model_cubes, body_tint};
use petramond_world::bbmodel::clips;
use petramond_world::light::BlockLight6;

fn model(name: &str) -> Model {
    let path = format!(
        "{}/../../assets/models/{name}.bbmodel",
        env!("CARGO_MANIFEST_DIR")
    );
    Model::load(&std::fs::read_to_string(&path).expect("model source")).expect("model parses")
}

fn look() -> SkinLook {
    SkinLook {
        hurt: 0.0,
        emitter_tint: [1.0; 3],
        emitter_self_lit: 0.0,
        light: DynLight::new(FULL_SKYLIGHT, BlockLight6::DARK),
        hidden: 0,
    }
}

fn shader_tint(inst: &SkinInstance, env: LightEnv) -> [f32; 3] {
    const SKY_MIN: f32 = 0.02;
    const FINAL_MIN: f32 = 0.006;
    let x = inst.light[0];
    let sky = SKY_MIN + (1.0 - SKY_MIN) * (x * x * x * env.sky_scale.clamp(0.0, 1.0));
    std::array::from_fn(|c| {
        let b = inst.light[1 + c];
        let block = SKY_MIN + (1.0 - SKY_MIN) * (b * b * b);
        let lit = (sky * env.sky_color[c]).max(block).max(FINAL_MIN);
        let lit = lit + (1.0 - lit) * inst.self_lit;
        inst.tint[c] * lit
    })
}

#[test]
fn vertex_and_instance_strides_match_their_declared_layouts() {
    assert_eq!(std::mem::size_of::<SkinVertex>(), 32);
    assert_eq!(std::mem::offset_of!(SkinVertex, bone), 24);
    assert_eq!(std::mem::offset_of!(SkinVertex, parts), 28);
    assert_eq!(std::mem::size_of::<SkinInstance>(), 48);
    assert_eq!(std::mem::offset_of!(SkinInstance, light), 16);
    assert_eq!(std::mem::offset_of!(SkinInstance, bone_base), 32);
    assert_eq!(std::mem::offset_of!(SkinInstance, hidden), 36);
}

#[test]
fn skinned_mesh_matches_the_cpu_bake() {
    let owl = model("owl");
    let scale = 0.25;
    let self_ao = owl.rest_self_ao(1.0 / (16.0 * scale), |_| true);
    let mesh = SkinMesh::build(&owl, scale, Some(&self_ao), |_| 0);
    let walk = owl.animation(clips::WALK).expect("the owl walks");
    for pose in [owl.rest_pose(), owl.pose_layers(&[(walk, 0.3, 1.0)])] {
        let global = Mat4::from_translation(Vec3::new(3.0, 64.0, -7.5))
            * Mat4::from_rotation_y(0.7)
            * Mat4::from_scale(Vec3::splat(scale));
        let (mut cpu, mut cpu_indices) = (Vec::<ItemVertex>::new(), Vec::new());
        bake_model_cubes(
            &owl,
            &pose,
            global,
            [1.0; 3],
            |_| false,
            Some(&self_ao),
            &mut cpu,
            &mut cpu_indices,
        );
        let mut batch = SkinBatch::default();
        let at = batch.push(&pose, global, bone_slots(&owl), look());
        assert_eq!(mesh.verts.len(), cpu.len(), "same faces survive");
        assert_eq!(mesh.indices, cpu_indices, "same triangulation");
        let skinned = skin_positions(&mesh, &batch, at);
        for ((gpu, pos), cpu) in mesh.verts.iter().zip(skinned).zip(&cpu) {
            assert_eq!(gpu.uv, cpu.uv);
            assert_eq!(gpu.shade, cpu.shade);
            let d = pos.distance(Vec3::from(cpu.pos));
            assert!(d < 1e-4, "skinned vertex drifted {d} from the bake");
        }
    }
}

#[test]
fn coat_parts_flag_exactly_the_coat_cubes() {
    let sheep = model("sheep");
    let mesh = SkinMesh::build(&sheep, 0.0625, None, |cube| {
        if sheep.cubes[cube].name == "wool" {
            PART_COAT
        } else {
            0
        }
    });
    let coat = mesh.verts.iter().filter(|v| v.parts == PART_COAT).count();
    assert!(coat > 0, "the fleece is flagged");
    assert!(coat < mesh.verts.len(), "the body under it is not");
    let (mut shorn, mut indices) = (Vec::new(), Vec::new());
    bake_model_cubes(
        &sheep,
        &sheep.rest_pose(),
        Mat4::from_scale(Vec3::splat(0.0625)),
        [1.0; 3],
        |cube| sheep.cubes[cube].name == "wool",
        None,
        &mut shorn,
        &mut indices,
    );
    assert_eq!(mesh.verts.len() - coat, shorn.len());
}

#[test]
fn palette_packs_global_times_pose_and_falls_back_to_global() {
    let global = Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0));
    let pose = [
        Mat4::from_rotation_z(0.5),
        Mat4::from_scale(Vec3::splat(2.0)),
    ];
    let mut batch = SkinBatch::default();
    batch.push(&pose, global, 3, look());
    assert_eq!(batch.palette.len(), 3);
    assert_eq!(batch.palette[0], (global * pose[0]).to_cols_array_2d());
    assert_eq!(batch.palette[1], (global * pose[1]).to_cols_array_2d());
    assert_eq!(
        batch.palette[2],
        global.to_cols_array_2d(),
        "a slot past the pose rides the placement alone"
    );
}

#[test]
fn instances_address_their_own_palette_runs() {
    let mut batch = SkinBatch::default();
    let a = batch.push(&[Mat4::IDENTITY; 4], Mat4::IDENTITY, 5, look());
    let b = batch.push(
        &[Mat4::IDENTITY; 2],
        Mat4::IDENTITY,
        3,
        SkinLook {
            hidden: PART_COAT,
            ..look()
        },
    );
    assert_eq!((a, b), (0, 1));
    assert_eq!(batch.next_instance(), 2);
    assert_eq!(batch.instances[0].bone_base, 0);
    assert_eq!(batch.instances[1].bone_base, 5);
    assert_eq!(batch.instances[1].hidden, PART_COAT);
    assert_eq!(batch.palette.len(), 8);
    batch.clear();
    assert!(batch.palette.is_empty() && batch.instances.is_empty());
    assert_eq!(batch.next_instance(), 0);
}

#[test]
fn unbound_cubes_ride_the_extra_slot() {
    let mut owl = model("owl");
    let bones = owl.bones.len();
    for cube in &mut owl.cubes {
        cube.bone = bones + 7;
    }
    let mesh = SkinMesh::build(&owl, 0.25, None, |_| 0);
    assert_eq!(bone_slots(&owl) as usize, bones + 1);
    assert!(!mesh.verts.is_empty());
    assert!(mesh.verts.iter().all(|v| v.bone as usize == bones));
}

#[test]
fn instance_light_reproduces_the_baked_body_tint() {
    let envs = [
        LightEnv::IDENTITY,
        LightEnv {
            sky_scale: 0.0,
            sky_color: [0.6, 0.7, 1.0],
        },
        LightEnv {
            sky_scale: 0.4,
            sky_color: [1.0, 0.8, 0.7],
        },
    ];
    let lights = [
        DynLight::new(63, BlockLight6::DARK),
        DynLight::new(0, BlockLight6::DARK),
        DynLight::new(20, BlockLight6::new(40, 0, 63)),
        DynLight::new(0, BlockLight6::grey(60)),
    ];
    for env in envs {
        for light in lights {
            for (hurt, self_lit) in [(0.0, 0.0), (0.5, 0.0), (0.3, 1.0), (1.0, 0.4)] {
                let look = SkinLook {
                    hurt,
                    emitter_tint: [1.0, 0.7, 0.4],
                    emitter_self_lit: self_lit,
                    light,
                    hidden: 0,
                };
                let inst = look.instance(0);
                let want = body_tint(hurt, look.emitter_tint, light, env, self_lit);
                let got = shader_tint(&inst, env);
                for c in 0..3 {
                    assert!(
                        (got[c] - want[c]).abs() < 1e-6,
                        "{light:?} {env:?} hurt={hurt} self_lit={self_lit}: {got:?} vs {want:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn the_skinned_shader_lights_bodies_with_the_shared_curve() {
    let src = include_str!("../../shaders/skinned.wgsl");
    for term in [
        "SKY_MIN + (1.0 - SKY_MIN) * (x * x * x * clamp(u.fog_color.w, 0.0, 1.0))",
        "mix(vec3<f32>(SKY_MIN), vec3<f32>(1.0), blk * blk * blk)",
        "max(max(sky_term, block_term), vec3<f32>(FINAL_MIN))",
        "lit + (vec3<f32>(1.0) - lit) * self_lit",
    ] {
        assert!(src.contains(term), "skinned.wgsl no longer spells `{term}`");
    }
}
