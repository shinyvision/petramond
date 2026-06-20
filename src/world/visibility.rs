use std::collections::VecDeque;

use crate::block::Block;
use crate::chunk::{Chunk, ChunkPos, CHUNK_SX, CHUNK_SZ, SECTION_COUNT, SECTION_SIZE};

use super::store::World;

const SECTION_VOLUME: usize = SECTION_SIZE * SECTION_SIZE * SECTION_SIZE;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SectionPos {
    pub cx: i32,
    pub sy: i32,
    pub cz: i32,
}

impl SectionPos {
    pub const fn new(cx: i32, sy: i32, cz: i32) -> Self {
        Self { cx, sy, cz }
    }

    pub fn from_world(wx: i32, wy: i32, wz: i32) -> Option<Self> {
        if wy < 0 || wy >= (SECTION_COUNT * SECTION_SIZE) as i32 {
            return None;
        }
        Some(Self {
            cx: wx >> 4,
            sy: wy / SECTION_SIZE as i32,
            cz: wz >> 4,
        })
    }

    pub fn chunk_pos(self) -> ChunkPos {
        ChunkPos::new(self.cx, self.cz)
    }

    pub fn neighbor(self, face: SectionFace) -> Option<Self> {
        match face {
            SectionFace::PosX => Some(Self::new(self.cx + 1, self.sy, self.cz)),
            SectionFace::NegX => Some(Self::new(self.cx - 1, self.sy, self.cz)),
            SectionFace::PosY => {
                let sy = self.sy + 1;
                (sy < SECTION_COUNT as i32).then_some(Self::new(self.cx, sy, self.cz))
            }
            SectionFace::NegY => {
                let sy = self.sy - 1;
                (sy >= 0).then_some(Self::new(self.cx, sy, self.cz))
            }
            SectionFace::PosZ => Some(Self::new(self.cx, self.sy, self.cz + 1)),
            SectionFace::NegZ => Some(Self::new(self.cx, self.sy, self.cz - 1)),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum SectionFace {
    PosX = 0,
    NegX = 1,
    PosY = 2,
    NegY = 3,
    PosZ = 4,
    NegZ = 5,
}

impl SectionFace {
    pub const fn index(self) -> usize {
        self as usize
    }

    pub const fn bit(self) -> u8 {
        1u8 << (self as u8)
    }

    pub const fn opposite(self) -> Self {
        match self {
            Self::PosX => Self::NegX,
            Self::NegX => Self::PosX,
            Self::PosY => Self::NegY,
            Self::NegY => Self::PosY,
            Self::PosZ => Self::NegZ,
            Self::NegZ => Self::PosZ,
        }
    }
}

pub const SECTION_FACES: [SectionFace; 6] = [
    SectionFace::PosX,
    SectionFace::NegX,
    SectionFace::PosY,
    SectionFace::NegY,
    SectionFace::PosZ,
    SectionFace::NegZ,
];

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct SectionConnectivity {
    exits: [u8; 6],
}

impl SectionConnectivity {
    pub fn exits_from(self, entry: SectionFace) -> u8 {
        self.exits[entry.index()]
    }
}

pub fn build_chunk_section_visibility(chunk: &Chunk) -> [SectionConnectivity; SECTION_COUNT] {
    std::array::from_fn(|section| build_section_connectivity(chunk, section))
}

fn build_section_connectivity(chunk: &Chunk, section: usize) -> SectionConnectivity {
    let mut visited = [false; SECTION_VOLUME];
    let mut connectivity = SectionConnectivity::default();

    for ly in 0..SECTION_SIZE {
        for lz in 0..SECTION_SZ {
            for lx in 0..SECTION_SX {
                let i = section_idx(lx, ly, lz);
                if visited[i] || !section_cell_open(chunk, section, lx, ly, lz) {
                    continue;
                }
                let faces = flood_section_component(chunk, section, lx, ly, lz, &mut visited);
                for face in SECTION_FACES {
                    if faces & face.bit() != 0 {
                        connectivity.exits[face.index()] |= faces & !face.bit();
                    }
                }
            }
        }
    }

    connectivity
}

const SECTION_SX: usize = CHUNK_SX;
const SECTION_SZ: usize = CHUNK_SZ;

fn flood_section_component(
    chunk: &Chunk,
    section: usize,
    sx: usize,
    sy: usize,
    sz: usize,
    visited: &mut [bool; SECTION_VOLUME],
) -> u8 {
    let mut faces = section_cell_faces(sx, sy, sz);
    let mut queue = VecDeque::new();
    visited[section_idx(sx, sy, sz)] = true;
    queue.push_back((sx, sy, sz));

    while let Some((x, y, z)) = queue.pop_front() {
        for (nx, ny, nz) in section_neighbors(x, y, z) {
            let i = section_idx(nx, ny, nz);
            if visited[i] || !section_cell_open(chunk, section, nx, ny, nz) {
                continue;
            }
            visited[i] = true;
            faces |= section_cell_faces(nx, ny, nz);
            queue.push_back((nx, ny, nz));
        }
    }

    faces
}

fn section_neighbors(x: usize, y: usize, z: usize) -> impl Iterator<Item = (usize, usize, usize)> {
    let mut out = [(0usize, 0usize, 0usize); 6];
    let mut len = 0usize;
    if x + 1 < SECTION_SIZE {
        out[len] = (x + 1, y, z);
        len += 1;
    }
    if x > 0 {
        out[len] = (x - 1, y, z);
        len += 1;
    }
    if y + 1 < SECTION_SIZE {
        out[len] = (x, y + 1, z);
        len += 1;
    }
    if y > 0 {
        out[len] = (x, y - 1, z);
        len += 1;
    }
    if z + 1 < SECTION_SIZE {
        out[len] = (x, y, z + 1);
        len += 1;
    }
    if z > 0 {
        out[len] = (x, y, z - 1);
        len += 1;
    }
    out.into_iter().take(len)
}

fn section_cell_open(chunk: &Chunk, section: usize, x: usize, y: usize, z: usize) -> bool {
    let wy = section * SECTION_SIZE + y;
    !Block::from_id(chunk.block_raw(x, wy, z)).is_opaque()
}

fn section_cell_faces(x: usize, y: usize, z: usize) -> u8 {
    let mut faces = 0u8;
    if x == 0 {
        faces |= SectionFace::NegX.bit();
    }
    if x + 1 == SECTION_SIZE {
        faces |= SectionFace::PosX.bit();
    }
    if y == 0 {
        faces |= SectionFace::NegY.bit();
    }
    if y + 1 == SECTION_SIZE {
        faces |= SectionFace::PosY.bit();
    }
    if z == 0 {
        faces |= SectionFace::NegZ.bit();
    }
    if z + 1 == SECTION_SIZE {
        faces |= SectionFace::PosZ.bit();
    }
    faces
}

fn section_idx(x: usize, y: usize, z: usize) -> usize {
    (y * SECTION_SIZE * SECTION_SIZE) + (z * SECTION_SIZE) + x
}

impl World {
    pub(crate) fn rebuild_section_visibility(&mut self, pos: ChunkPos) {
        let Some(chunk) = self.chunks.get(&pos) else {
            self.section_visibility.remove(&pos);
            return;
        };

        let visibility = build_chunk_section_visibility(chunk);
        self.section_visibility.insert(pos, visibility);
    }

    pub(crate) fn invalidate_section_visibility(&mut self, pos: ChunkPos) {
        self.section_visibility.remove(&pos);
        self.bump_visibility_revision();
    }

    pub(crate) fn has_section_visibility(&self, pos: ChunkPos) -> bool {
        self.section_visibility.contains_key(&pos)
    }

    pub(crate) fn ensure_section_visibility(&mut self, pos: ChunkPos) -> bool {
        if self.has_section_visibility(pos) {
            return true;
        }
        self.rebuild_section_visibility(pos);
        self.has_section_visibility(pos)
    }

    pub(super) fn bump_visibility_revision(&mut self) {
        self.visibility_revision = self.visibility_revision.wrapping_add(1);
    }

    pub fn section_connectivity(&self, pos: SectionPos) -> Option<SectionConnectivity> {
        if pos.sy < 0 || pos.sy >= SECTION_COUNT as i32 {
            return None;
        }
        self.section_visibility
            .get(&pos.chunk_pos())
            .map(|sections| sections[pos.sy as usize])
    }

    pub fn camera_section_exits(&self, wx: i32, wy: i32, wz: i32) -> Option<(SectionPos, u8)> {
        if self.can_see_sky_from(wx, wy, wz) {
            return None;
        }
        let pos = SectionPos::from_world(wx, wy, wz)?;
        let chunk = self.chunks.get(&pos.chunk_pos())?;
        let lx = (wx & 0x0F) as usize;
        let ly = wy as usize % SECTION_SIZE;
        let lz = (wz & 0x0F) as usize;
        if !section_cell_open(chunk, pos.sy as usize, lx, ly, lz) {
            return None;
        }

        let mut visited = [false; SECTION_VOLUME];
        let exits = flood_section_component(chunk, pos.sy as usize, lx, ly, lz, &mut visited);
        Some((pos, exits))
    }

    pub fn can_see_sky_from(&self, wx: i32, wy: i32, wz: i32) -> bool {
        if wy < 0 || wy >= (SECTION_COUNT * SECTION_SIZE) as i32 {
            return true;
        }
        let Some(chunk) = self.chunks.get(&ChunkPos::new(wx >> 4, wz >> 4)) else {
            return true;
        };
        let lx = (wx & 0x0F) as usize;
        let lz = (wz & 0x0F) as usize;
        for y in (wy as usize + 1)..(SECTION_COUNT * SECTION_SIZE) {
            if Block::from_id(chunk.block_raw(lx, y, lz)).is_opaque() {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solid_section_has_no_face_connections() {
        let mut chunk = Chunk::new(0, 0);
        for y in 0..SECTION_SIZE {
            for z in 0..CHUNK_SZ {
                for x in 0..CHUNK_SX {
                    chunk.set_block(x, y, z, Block::Stone);
                }
            }
        }

        let visibility = build_chunk_section_visibility(&chunk);

        for face in SECTION_FACES {
            assert_eq!(visibility[0].exits_from(face), 0);
        }
    }

    #[test]
    fn open_section_connects_all_other_faces() {
        let chunk = Chunk::new(0, 0);

        let visibility = build_chunk_section_visibility(&chunk);

        assert_eq!(
            visibility[0].exits_from(SectionFace::NegX),
            SectionFace::PosX.bit()
                | SectionFace::PosY.bit()
                | SectionFace::NegY.bit()
                | SectionFace::PosZ.bit()
                | SectionFace::NegZ.bit()
        );
    }

    #[test]
    fn isolated_air_pocket_does_not_connect_faces() {
        let mut chunk = Chunk::new(0, 0);
        for y in 0..SECTION_SIZE {
            for z in 0..CHUNK_SZ {
                for x in 0..CHUNK_SX {
                    chunk.set_block(x, y, z, Block::Stone);
                }
            }
        }
        chunk.set_block(8, 8, 8, Block::Air);

        let visibility = build_chunk_section_visibility(&chunk);

        for face in SECTION_FACES {
            assert_eq!(visibility[0].exits_from(face), 0);
        }
    }
}
