use super::{CellData, Schematic, SchematicCell};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const SECTION_VOLUME: usize = 16 * 16 * 16;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SchematicSection {
    pub pos: [i32; 3],
    pub palette: Vec<CellData>,
    pub cells: Vec<SectionCell>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SectionCell {
    pub index: u16,
    pub palette: u16,
}

#[derive(Clone, Copy, Debug)]
pub struct CellRef<'a> {
    pub pos: [i32; 3],
    pub data: &'a CellData,
}

impl CellRef<'_> {
    pub fn to_owned(self) -> SchematicCell {
        SchematicCell {
            pos: self.pos,
            data: self.data.clone(),
        }
    }
}

impl SchematicSection {
    pub fn iter(&self) -> impl Iterator<Item = CellRef<'_>> {
        self.cells.iter().map(|cell| {
            let i = i32::from(cell.index);
            let local = [i % 16, i / 256, i / 16 % 16];
            CellRef {
                pos: std::array::from_fn(|a| self.pos[a] * 16 + local[a]),
                data: &self.palette[usize::from(cell.palette)],
            }
        })
    }

    pub(crate) fn validate(&self, size: [i32; 3]) -> Result<(), String> {
        if (0..3).any(|a| self.pos[a] < 0 || self.pos[a] > (size[a] - 1) / 16)
            || self.cells.is_empty()
            || self.cells.len() > SECTION_VOLUME
            || self.palette.is_empty()
            || self.palette.len() > self.cells.len()
        {
            return Err("Invalid schematic section".into());
        }
        let mut previous = None;
        for cell in &self.cells {
            if usize::from(cell.index) >= SECTION_VOLUME
                || usize::from(cell.palette) >= self.palette.len()
                || previous.is_some_and(|i| i >= cell.index)
            {
                return Err("Invalid or duplicate schematic cell".into());
            }
            previous = Some(cell.index);
        }
        if self.iter().any(|c| (0..3).any(|a| c.pos[a] >= size[a])) {
            return Err("Schematic cell outside bounds".into());
        }
        for data in &self.palette {
            data.validate()?;
        }
        Ok(())
    }
}

#[derive(Default)]
struct SectionBuilder {
    palette: FxHashMap<CellData, u16>,
    cells: BTreeMap<u16, u16>,
}

/// Sparse positions and interned records; neither holes nor repeated block data
/// consume full cell snapshots while capturing a structure.
#[derive(Default)]
pub struct SchematicBuilder {
    sections: BTreeMap<[i32; 3], SectionBuilder>,
}
impl SchematicBuilder {
    fn address(pos: [i32; 3]) -> ([i32; 3], u16) {
        let [x, y, z] = pos.map(|p| p.rem_euclid(16) as u16);
        (pos.map(|p| p.div_euclid(16)), x + 16 * z + 256 * y)
    }

    pub fn contains(&self, pos: [i32; 3]) -> bool {
        let (section, index) = Self::address(pos);
        self.sections
            .get(&section)
            .is_some_and(|s| s.cells.contains_key(&index))
    }

    pub fn insert(&mut self, pos: [i32; 3], data: CellData) -> Result<(), String> {
        let (section, index) = Self::address(pos);
        let section = self.sections.entry(section).or_default();
        if section.cells.contains_key(&index) {
            return Err("Duplicate schematic cell".into());
        }
        let next = section.palette.len() as u16;
        let palette = match section.palette.entry(data) {
            std::collections::hash_map::Entry::Occupied(entry) => *entry.get(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.key().validate()?;
                *entry.insert(next)
            }
        };
        section.cells.insert(index, palette);
        Ok(())
    }

    pub fn finish(self, name: String, size: [i32; 3]) -> Result<Schematic, String> {
        let sections = self
            .sections
            .into_iter()
            .map(|(pos, builder)| {
                let mut palette: Vec<_> = builder.palette.into_iter().collect();
                palette.sort_unstable_by_key(|(_, index)| *index);
                let mut palette: Vec<_> = palette.into_iter().map(|(data, _)| Some(data)).collect();
                // First occurrence in voxel order makes the file independent of selection order.
                let mut remap = vec![None; palette.len()];
                let mut canonical = Vec::new();
                let cells = builder
                    .cells
                    .into_iter()
                    .map(|(index, old)| {
                        let p = *remap[usize::from(old)].get_or_insert_with(|| {
                            let p = canonical.len() as u16;
                            canonical.push(
                                palette[usize::from(old)]
                                    .take()
                                    .expect("first palette reference"),
                            );
                            p
                        });
                        SectionCell { index, palette: p }
                    })
                    .collect();
                SchematicSection {
                    pos,
                    palette: canonical,
                    cells,
                }
            })
            .collect();
        let result = Schematic {
            name,
            size,
            sections,
        };
        result.validate()?;
        Ok(result)
    }

    pub fn normalized(self, name: String) -> Result<Schematic, String> {
        if self.sections.is_empty() {
            return Err("The selection contains no blocks".into());
        }
        let mut lo = [i32::MAX; 3];
        let mut hi = [i32::MIN; 3];
        for (section, builder) in &self.sections {
            for &i in builder.cells.keys() {
                let i = i32::from(i);
                let local = [i % 16, i / 256, i / 16 % 16];
                for a in 0..3 {
                    let p = section[a] * 16 + local[a];
                    lo[a] = lo[a].min(p);
                    hi[a] = hi[a].max(p);
                }
            }
        }
        let mut result = Self::default();
        for (section, builder) in self.sections {
            let palette: FxHashMap<_, _> =
                builder.palette.into_iter().map(|(d, i)| (i, d)).collect();
            let mut remap = FxHashMap::default();
            for (i, value) in builder.cells {
                let i = i32::from(i);
                let local = [i % 16, i / 256, i / 16 % 16];
                let p = std::array::from_fn(|a| section[a] * 16 + local[a] - lo[a]);
                let (target, index) = Self::address(p);
                let destination = result.sections.entry(target).or_default();
                let value = *remap.entry((target, value)).or_insert_with(|| {
                    let data = &palette[&value];
                    if let Some(value) = destination.palette.get(data) {
                        *value
                    } else {
                        let next = destination.palette.len() as u16;
                        destination.palette.insert(data.clone(), next);
                        next
                    }
                });
                destination.cells.insert(index, value);
            }
        }
        result.finish(name, std::array::from_fn(|a| hi[a] - lo[a] + 1))
    }
}

impl Schematic {
    pub fn from_cells(
        name: String,
        size: [i32; 3],
        cells: impl IntoIterator<Item = SchematicCell>,
    ) -> Result<Self, String> {
        let mut builder = SchematicBuilder::default();
        for cell in cells {
            builder.insert(cell.pos, cell.data)?;
        }
        builder.finish(name, size)
    }

    pub fn cells(&self) -> impl Iterator<Item = CellRef<'_>> {
        self.sections.iter().flat_map(SchematicSection::iter)
    }

    pub fn cell_count(&self) -> usize {
        self.sections.iter().map(|s| s.cells.len()).sum()
    }
}
