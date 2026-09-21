//! Anchored ghosts as this client draws them: each design is split into 16³
//! pieces of its turned frame, and a piece shows only the cells the world
//! does not hold yet, measured against the replica with the same construction
//! rule the server builds by. A piece re-meshes only when the set of cells it
//! shows changes, so one placed block never rebuilds a whole building.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use petramond::schematic::share::GhostPlacement;
use petramond::schematic::{CellData, ResolvedCell, Scene, Schematic};
use petramond_math::math::IVec3;
use petramond_world::construction::{self, Record, Status};

use super::Game;

/// How often every nearby piece is re-measured, catching changes no block
/// delta announced (a section streaming in, a prediction settling).
const SWEEP_SECONDS: f64 = 1.0;

#[derive(Clone, Copy)]
struct Cell {
    local: IVec3,
    section: u32,
    palette: u16,
}

struct Built {
    shown: u64,
    revision: u64,
    scene: Arc<Scene>,
}

struct Index {
    placement: GhostPlacement,
    size: [i32; 3],
    pieces: BTreeMap<[i32; 3], Vec<Cell>>,
    /// Per design section, its palette as construction records ready to draw.
    palettes: Vec<Vec<Option<(Record, ResolvedCell)>>>,
    built: HashMap<[i32; 3], Built>,
    dirty: BTreeSet<[i32; 3]>,
}

#[derive(Default)]
pub struct Ghosts {
    index: BTreeMap<String, Index>,
    next_revision: u64,
    last_sweep: f64,
}

fn piece_of(local: IVec3) -> [i32; 3] {
    [
        local.x.div_euclid(16),
        local.y.div_euclid(16),
        local.z.div_euclid(16),
    ]
}

fn piece_id(key: &str, piece: [i32; 3]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    piece.hash(&mut h);
    h.finish()
}

impl Index {
    fn new(placement: GhostPlacement, schematic: &Schematic) -> Self {
        let mut pieces: BTreeMap<[i32; 3], Vec<Cell>> = BTreeMap::new();
        let mut palettes = Vec::with_capacity(schematic.sections.len());
        for index in 0..schematic.sections.len() {
            let Some((cells, palette)) = schematic.construction_section(index, placement.turns)
            else {
                continue;
            };
            palettes.push(palette.iter().map(drawable).collect());
            for (local, palette) in cells {
                let local = IVec3::from_array(local);
                pieces.entry(piece_of(local)).or_default().push(Cell {
                    local,
                    section: index as u32,
                    palette,
                });
            }
        }
        let dirty = pieces.keys().copied().collect();
        Self {
            placement,
            size: schematic.rotated_size(placement.turns),
            pieces,
            palettes,
            built: HashMap::new(),
            dirty,
        }
    }

    fn record(&self, cell: &Cell) -> Option<&(Record, ResolvedCell)> {
        self.palettes
            .get(cell.section as usize)?
            .get(cell.palette as usize)?
            .as_ref()
    }
}

/// A palette record as the ghost draws it: its construction form, never an
/// archived running state.
fn drawable(data: &CellData) -> Option<(Record, ResolvedCell)> {
    let record = data.construction_record().ok()?;
    let built = record.built();
    (built.block != petramond_world::block::Block::Air).then(|| {
        let cell = ResolvedCell {
            block: built.block,
            state: built
                .block
                .shape_kind_def()
                .placement
                .authored_state(built.block, built.state),
            fluid: 0,
            kv: built.data.clone(),
            container: None,
            furnace: None,
        };
        (record, cell)
    })
}

impl Game {
    /// Mark the pieces holding any of `cells` for re-measurement.
    pub(super) fn note_ghost_changes(&mut self, cells: impl Iterator<Item = IVec3> + Clone) {
        for index in self.ghosts.index.values_mut() {
            let origin = IVec3::from_array(index.placement.origin);
            let size = IVec3::from_array(index.size);
            for pos in cells.clone() {
                let local = pos - origin;
                if local.cmpge(IVec3::splat(-1)).all() && local.cmple(size).all() {
                    index.dirty.insert(piece_of(local));
                }
            }
        }
    }

    /// The anchored ghost pieces to draw this frame: those within the view
    /// distance, since a piece compares itself against terrain that must be
    /// streamed in.
    pub fn ghost_pieces(
        &mut self,
        now: f64,
        view_chunks: i32,
    ) -> Vec<petramond_render::GhostPiece> {
        self.sync_ghost_index();
        let reach = (view_chunks.max(1) * 16) as f32;
        let camera = self.cam.pos;
        let sweep = now - self.ghosts.last_sweep >= SWEEP_SECONDS;
        if sweep {
            self.ghosts.last_sweep = now;
        }
        let mut out = Vec::new();
        let keys: Vec<String> = self.ghosts.index.keys().cloned().collect();
        for key in keys {
            let positioned = self.schematic_preview.positioning_tag();
            if self.ghosts.index[&key].placement.yields_to_positioning
                && positioned == Some(key.as_str())
            {
                continue;
            }
            let near: Vec<[i32; 3]> = {
                let index = &self.ghosts.index[&key];
                let origin = IVec3::from_array(index.placement.origin);
                index
                    .pieces
                    .keys()
                    .copied()
                    .filter(|piece| {
                        let centre = origin + IVec3::from_array(*piece) * 16 + IVec3::splat(8);
                        (petramond_math::world_pos::WorldPos::block_center(centre) - camera)
                            .length()
                            <= reach
                    })
                    .collect()
            };
            if sweep {
                let index = self.ghosts.index.get_mut(&key).unwrap();
                index.dirty.extend(near.iter().copied());
            }
            for piece in &near {
                if self.ghosts.index[&key].dirty.contains(piece) {
                    self.remeasure_piece(&key, *piece);
                }
            }
            let index = &self.ghosts.index[&key];
            for piece in near {
                if let Some(built) = index.built.get(&piece) {
                    out.push(petramond_render::GhostPiece {
                        id: piece_id(&key, piece),
                        revision: built.revision,
                        scene: built.scene.clone(),
                        section: petramond_world::chunk::SectionPos::from_world(
                            piece[0] * 16,
                            petramond_world::chunk::WORLD_MIN_Y + piece[1] * 16,
                            piece[2] * 16,
                        )
                        .expect("a ghost piece lies inside the scene's world"),
                        origin: index.placement.origin,
                    });
                }
            }
        }
        out
    }

    fn sync_ghost_index(&mut self) {
        let wanted = &self.schematics.ghosts;
        self.ghosts
            .index
            .retain(|key, index| wanted.get(key) == Some(&index.placement));
        for (key, placement) in wanted.clone() {
            if self.ghosts.index.contains_key(&key) {
                continue;
            }
            let Some(schematic) = self.schematic_design(&placement.digest).cloned() else {
                continue;
            };
            self.ghosts
                .index
                .insert(key, Index::new(placement, &schematic));
        }
    }

    /// Measure one piece against the replica and rebuild its scene if the
    /// cells it shows changed.
    fn remeasure_piece(&mut self, key: &str, piece: [i32; 3]) {
        let index = self.ghosts.index.get_mut(key).unwrap();
        index.dirty.remove(&piece);
        let origin = IVec3::from_array(index.placement.origin);
        let shows = |index: &Index, cell: &Cell| {
            index.record(cell).is_some_and(|(record, _)| {
                construction::status(&self.replica, origin + cell.local, record)
                    != Status::Satisfied
            })
        };
        let own: Vec<Cell> = index.pieces[&piece]
            .iter()
            .filter(|cell| shows(index, cell))
            .copied()
            .collect();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for cell in &own {
            (cell.local.to_array(), cell.section, cell.palette).hash(&mut hasher);
        }
        let shown = hasher.finish();
        if index.built.get(&piece).is_some_and(|b| b.shown == shown) {
            return;
        }
        if own.is_empty() {
            index.built.remove(&piece);
            return;
        }
        // Neighbouring pieces' shown cells along the shared faces, so faces
        // between two pieces cull like faces inside one.
        let lo = IVec3::from_array(piece) * 16 - IVec3::ONE;
        let hi = IVec3::from_array(piece) * 16 + IVec3::splat(16);
        let mut cells: Vec<(IVec3, ResolvedCell)> = Vec::with_capacity(own.len());
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let neighbour = [piece[0] + dx, piece[1] + dy, piece[2] + dz];
                    let Some(list) = index.pieces.get(&neighbour) else {
                        continue;
                    };
                    let own_piece = neighbour == piece;
                    for cell in list {
                        let inside = cell.local.cmpge(lo).all() && cell.local.cmple(hi).all();
                        if !inside || (!own_piece && !shows(index, cell)) {
                            continue;
                        }
                        if own_piece && !own.iter().any(|c| c.local == cell.local) {
                            continue;
                        }
                        if let Some((_, drawn)) = index.record(cell) {
                            cells.push((cell.local, drawn.clone()));
                        }
                    }
                }
            }
        }
        let size = index.size;
        let scene = Scene::from_cells(size, cells, |world| {
            self.client_mods.bake_custom_shapes(world)
        });
        let index = self.ghosts.index.get_mut(key).unwrap();
        match scene {
            Ok(scene) => {
                self.ghosts.next_revision += 1;
                index.built.insert(
                    piece,
                    Built {
                        shown,
                        revision: self.ghosts.next_revision,
                        scene: Arc::new(scene),
                    },
                );
            }
            Err(error) => log::warn!("ghost piece {piece:?}: {error}"),
        }
    }
}
