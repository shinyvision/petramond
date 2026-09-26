//! Geometry shared by schematic ghosts and isolated structure screenshots.
use petramond::schematic::Scene;
use petramond_math::math::IVec3;
use petramond_mesh::{ModelVertex, Vertex};
use petramond_world::{
    block::{Block, ShapeState},
    chunk::SectionPos,
};

#[derive(Default)]
pub(crate) struct Geometry {
    pub blocks: Vec<Vertex>,
    pub block_indices: Vec<u32>,
    pub models: Vec<ModelVertex>,
    pub model_indices: Vec<u32>,
    pub size: [i32; 3],
}

impl Geometry {
    pub fn build(scene: &Scene) -> Self {
        Self::build_sections(scene, None)
    }

    /// Mesh only the scene section `only` (or every section): one piece of a
    /// larger ghost, its bordering cells present for culling but not drawn.
    pub fn build_sections(scene: &Scene, only: Option<SectionPos>) -> Self {
        let mut out = Self {
            size: scene.size,
            ..Default::default()
        };
        let lookup =
            |x, y, z| SectionPos::from_world(x, y, z).and_then(|sp| scene.sections.get(&sp));
        for (pos, section) in &scene.sections {
            if only.is_some_and(|only| only != *pos) {
                continue;
            }
            let mesh = petramond_mesh::build_section_mesh(
                section,
                *pos,
                petramond_mesh::MeshContext::global(),
                &petramond_mesh::WorldReads {
                    block: &|x, y, z| {
                        lookup(x, y, z).map_or(Block::Air.id(), |s| {
                            s.block((x & 15) as usize, (y & 15) as usize, (z & 15) as usize)
                                .id()
                        })
                    },
                    cell_state: &|x, y, z| {
                        lookup(x, y, z).map_or(ShapeState::NONE, |s| {
                            s.cell_state((x & 15) as usize, (y & 15) as usize, (z & 15) as usize)
                        })
                    },
                    fluid_meta: &|x, y, z| {
                        lookup(x, y, z).map_or(0, |s| {
                            s.fluid_meta((x & 15) as usize, (y & 15) as usize, (z & 15) as usize)
                        })
                    },
                    biome: &|_, _| 0,
                    skylight: &|_, _, _| petramond_world::chunk::SKY_FULL,
                    blocklight: &|_, _, _| petramond_world::light::LightRgb::ZERO,
                    loaded: &|_, _, _| true,
                    dyed: &|_, _, _| false,
                },
            );
            for (stream, two_sided) in [
                (mesh.opaque, false),
                (mesh.transparent, false),
                (mesh.transparent_two_sided, true),
                (mesh.translucent, false),
            ] {
                let base = out.blocks.len() as u32;
                for i in (0..stream.len() as u32).step_by(4) {
                    out.block_indices
                        .extend([0, 1, 2, 0, 2, 3].map(|j| base + i + j));
                    if two_sided {
                        out.block_indices
                            .extend([2, 1, 0, 3, 2, 0].map(|j| base + i + j));
                    }
                }
                out.blocks.extend(stream.into_iter().map(|mut v| {
                    v.pos[0] += (pos.cx * 16) as f32;
                    v.pos[2] += (pos.cz * 16) as f32;
                    v
                }));
            }
            let base = out.models.len() as u32;
            out.model_indices.extend(
                mesh.model_idx
                    .into_iter()
                    .chain(mesh.model_blend_idx)
                    .map(|i| i + base),
            );
            out.models.extend(mesh.model.into_iter().map(|mut v| {
                v.pos[0] += (pos.cx * 16) as f32;
                v.pos[2] += (pos.cz * 16) as f32;
                v
            }));
        }
        // Animated blocks draw from their models, posed by the captured
        // state and resting where it says (closed, or a door standing open).
        let entities: Vec<crate::BlockEntityInstance> = scene
            .cells
            .iter()
            .filter(|(p, _)| {
                only.is_none_or(|only| SectionPos::from_world(p.x, p.y, p.z) == Some(only))
            })
            .filter_map(|(p, cell)| {
                let (_, pose) = cell.block.animated_pose(cell.state)?;
                Some(crate::BlockEntityInstance {
                    pos: *p,
                    block: cell.block,
                    facing: pose.facing,
                    variant: pose.variant,
                    open01: if pose.open { 1.0 } else { 0.0 },
                    skylight: 63,
                    blocklight: petramond_world::light::BlockLight6::DARK,
                })
            })
            .collect();
        let (mut verts, mut indices) = (Vec::new(), Vec::new());
        crate::block_entity_model::push_block_entities(
            &entities,
            IVec3::ZERO,
            &mut verts,
            &mut indices,
        );
        out.append(verts, indices);
        for vertex in &mut out.blocks {
            for axis in 0..3 {
                vertex.pos[axis] -= scene.origin[axis] as f32;
            }
        }
        for vertex in &mut out.models {
            for axis in 0..3 {
                vertex.pos[axis] -= scene.origin[axis] as f32;
            }
        }
        out
    }
    fn append(&mut self, vertices: Vec<Vertex>, indices: Vec<u32>) {
        let base = self.blocks.len() as u32;
        self.block_indices
            .extend(indices.into_iter().map(|i| i + base));
        self.blocks.extend(vertices);
    }
}
