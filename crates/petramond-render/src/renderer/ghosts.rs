//! Schematic ghosts: the one preview that follows a placement being
//! positioned, and the anchored pieces of designs standing in the world.
//! Geometry is built off the frame thread; a mesh keeps drawing its previous
//! content until its replacement lands.

mod mesh;

use super::Renderer;
use crate::job::Job;
use crate::schematic::Geometry;
use glam::IVec3;
use mesh::{GhostMesh, GhostPipelines};
use petramond::schematic::Scene;
use petramond::worker::JobPool;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// One piece of an anchored ghost to draw: a stable id, the revision of its
/// content, its scene, the scene section it draws, and the world position of
/// the scene's local origin.
pub struct GhostPiece {
    pub id: u64,
    pub revision: u64,
    pub scene: Arc<Scene>,
    pub section: petramond_world::chunk::SectionPos,
    pub origin: [i32; 3],
}

struct AnchoredPiece {
    revision: u64,
    mesh: GhostMesh,
    building: Option<(u64, Job<Geometry>)>,
}

struct Preview {
    mesh: GhostMesh,
    /// The scene the mesh currently holds.
    scene: Option<Arc<Scene>>,
    building: Option<(Arc<Scene>, Job<Geometry>)>,
    visible: bool,
}

#[derive(Default)]
pub(super) struct GhostPass {
    pipelines: Option<GhostPipelines>,
    preview: Option<Preview>,
    anchored: HashMap<u64, AnchoredPiece>,
}

impl GhostPass {
    /// The pipelines sample world-scoped atlases and registered fluid media,
    /// so they leave with the world too.
    pub(super) fn clear_world(&mut self) {
        *self = Self::default();
    }

    pub(super) fn is_empty(&self) -> bool {
        self.anchored.is_empty() && !self.preview.as_ref().is_some_and(|p| p.visible)
    }

    pub(super) fn camera(&self, queue: &wgpu::Queue, u: &crate::uniforms::Uniforms) {
        for mesh in self.meshes() {
            mesh.camera(queue, u);
        }
    }

    pub(super) fn draw(&self, pass: &mut wgpu::RenderPass<'_>, samples: u32) {
        for mesh in self.meshes() {
            mesh.draw(pass, samples);
        }
    }

    fn meshes(&self) -> impl Iterator<Item = &GhostMesh> {
        let preview = self.preview.as_ref().filter(|p| p.visible).map(|p| &p.mesh);
        self.anchored.values().map(|p| &p.mesh).chain(preview)
    }

    fn pipelines(&mut self, r: &Renderer) -> GhostPipelines {
        self.pipelines
            .get_or_insert_with(|| GhostPipelines::new(r))
            .clone()
    }
}

impl Renderer {
    /// Show `scene` as the placement preview at `origin`; without an origin
    /// the preview keeps its mesh but is not drawn.
    pub fn set_schematic_preview(
        &mut self,
        jobs: &JobPool,
        scene: Option<Arc<Scene>>,
        origin: Option<[i32; 3]>,
    ) {
        let Some(scene) = scene else {
            self.ghosts.preview = None;
            return;
        };
        let mut pass = std::mem::take(&mut self.ghosts);
        let pipelines = pass.pipelines(self);
        let preview = pass.preview.get_or_insert_with(|| Preview {
            mesh: GhostMesh::new(&self.device, &pipelines),
            scene: None,
            building: None,
            visible: false,
        });
        preview.visible = origin.is_some();
        if let Some(origin) = origin {
            preview.mesh.origin = IVec3::from_array(origin);
        }
        if let Some((built, job)) = &preview.building {
            if let Some(outcome) = job.poll() {
                if let (Ok(geometry), true) = (outcome, Arc::ptr_eq(built, &scene)) {
                    preview.mesh.upload(&self.device, &self.queue, &geometry);
                    preview.scene = Some(built.clone());
                }
                preview.building = None;
            }
        }
        let shown = preview
            .scene
            .as_ref()
            .is_some_and(|s| Arc::ptr_eq(s, &scene));
        if !shown {
            preview.mesh.clear();
            preview.scene = None;
            if preview.building.is_none() {
                let copy = scene.clone();
                preview.building = Some((scene, Job::spawn(jobs, move || Geometry::build(&copy))));
            }
        }
        self.ghosts = pass;
    }

    /// Draw exactly these anchored ghost pieces: a piece whose revision
    /// changed re-meshes off the frame thread and keeps drawing its previous
    /// mesh until the new one lands; a piece no longer listed is dropped.
    pub fn set_anchored_ghosts(&mut self, jobs: &JobPool, pieces: &[GhostPiece]) {
        if pieces.is_empty() {
            self.ghosts.anchored.clear();
            return;
        }
        let mut pass = std::mem::take(&mut self.ghosts);
        let pipelines = pass.pipelines(self);
        let listed: HashSet<u64> = pieces.iter().map(|piece| piece.id).collect();
        pass.anchored.retain(|id, _| listed.contains(id));
        for piece in pieces {
            let anchored = pass
                .anchored
                .entry(piece.id)
                .or_insert_with(|| AnchoredPiece {
                    revision: u64::MAX,
                    mesh: GhostMesh::new(&self.device, &pipelines),
                    building: None,
                });
            anchored.mesh.origin = IVec3::from_array(piece.origin);
            if let Some((revision, job)) = &anchored.building {
                if let Some(outcome) = job.poll() {
                    if let Ok(geometry) = outcome {
                        anchored.mesh.upload(&self.device, &self.queue, &geometry);
                        anchored.revision = *revision;
                    }
                    anchored.building = None;
                }
            }
            if anchored.revision != piece.revision && anchored.building.is_none() {
                let scene = piece.scene.clone();
                let section = piece.section;
                anchored.building = Some((
                    piece.revision,
                    Job::spawn(jobs, move || {
                        Geometry::build_sections(&scene, Some(section))
                    }),
                ));
            }
        }
        self.ghosts = pass;
    }
}
