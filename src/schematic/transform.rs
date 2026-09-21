use super::{ResolvedCell, Schematic};
use petramond_math::math::IVec3;
use petramond_world::block::CellView;

pub fn rotate_position(mut p: [i32; 3], mut size: [i32; 3], turns: u8) -> [i32; 3] {
    for _ in 0..turns % 4 {
        p = [size[2] - 1 - p[2], p[1], p[0]];
        size.swap(0, 2);
    }
    p
}

impl Schematic {
    pub fn rotated_size(&self, turns: u8) -> [i32; 3] {
        let mut size = self.size;
        if !turns.is_multiple_of(2) {
            size.swap(0, 2);
        }
        size
    }

    /// The bottom center cell, snapped toward the higher index for even footprints.
    pub fn placement_pivot(&self, turns: u8) -> IVec3 {
        let [x, _, z] = self.rotated_size(turns);
        IVec3::new(x / 2, 0, z / 2)
    }

    /// Resolve every name and rotate all records before a world write can begin.
    pub fn placed_cells(
        &self,
        origin: IVec3,
        turns: u8,
    ) -> Result<Vec<(IVec3, ResolvedCell)>, String> {
        self.validate()?;
        let size = self.rotated_size(turns);
        let end: Option<Vec<_>> = (0..3).map(|i| origin[i].checked_add(size[i] - 1)).collect();
        let end = end.ok_or("Schematic position overflow")?;
        if !petramond_world::border::contains_column(origin.x, origin.z)
            || !petramond_world::border::contains_column(end[0], end[2])
            || origin.y < petramond_world::chunk::WORLD_MIN_Y
            || end[1] >= petramond_world::chunk::WORLD_MAX_Y
        {
            return Err("Schematic crosses the world bounds".into());
        }
        let mut cells = Vec::with_capacity(self.cell_count());
        let mut moved_anchors = Vec::new();
        for section in &self.sections {
            let original_palette = section
                .palette
                .iter()
                .map(|data| data.resolve())
                .collect::<Result<Vec<_>, _>>()?;
            let palette = original_palette
                .iter()
                .map(|data| {
                    let mut data = data.clone();
                    (data.block, data.state, data.kv) =
                        rotate_record(data.block, data.state, data.kv, turns);
                    Ok(data)
                })
                .collect::<Result<Vec<_>, String>>()?;
            for (cell, reference) in section.iter().zip(&section.cells) {
                let original = IVec3::from_array(cell.pos);
                let was_anchor =
                    model_base(original, &original_palette[usize::from(reference.palette)])
                        == Some(original);
                let data = palette[usize::from(reference.palette)].clone();
                let relative = IVec3::from_array(rotate_position(cell.pos, self.size, turns));
                // The validated rotated bounds already guarantee this addition is in range.
                let pos = origin + relative;
                if was_anchor {
                    if let Some(base) = model_base(pos, &data) {
                        if base != pos {
                            moved_anchors.push((pos, base));
                        }
                    }
                }
                cells.push((pos, data));
            }
        }
        if moved_anchors.is_empty() {
            return Ok(cells);
        }
        // Model machine records live at the rotated footprint's minimum cell,
        // while each cell's shape offset stays in authored model coordinates.
        let indices: std::collections::HashMap<_, _> = cells
            .iter()
            .enumerate()
            .map(|(i, (p, _))| (*p, i))
            .collect();
        for (from, to) in moved_anchors {
            let a = *indices.get(&from).ok_or("Incomplete model footprint")?;
            let b = *indices.get(&to).ok_or("Incomplete model footprint")?;
            let mut left = cells[a].1.clone();
            let right = &mut cells[b].1;
            std::mem::swap(&mut left.kv, &mut right.kv);
            std::mem::swap(&mut left.container, &mut right.container);
            std::mem::swap(&mut left.furnace, &mut right.furnace);
            cells[a].1 = left;
        }
        Ok(cells)
    }
}

/// Turn one cell's row, shape state and part-addressed data `turns` quarter
/// turns clockwise, as its family and rows declare.
pub fn rotate_record(
    mut block: petramond_world::block::Block,
    mut state: petramond_world::block::ShapeState,
    mut kv: std::collections::BTreeMap<String, Vec<u8>>,
    turns: u8,
) -> (
    petramond_world::block::Block,
    petramond_world::block::ShapeState,
    std::collections::BTreeMap<String, Vec<u8>>,
) {
    for _ in 0..turns % 4 {
        let turn = block.shape_kind_def().sim.rotate_y(block, state);
        state = turn.state;
        if turn.swap_parts {
            kv = kv
                .into_iter()
                .map(|(key, value)| {
                    let (base, part) = petramond_world::block::split_part_kv_key(&key);
                    (
                        petramond_world::block::part_kv_key(
                            base,
                            if part < 2 { 1 - part } else { part },
                        ),
                        value,
                    )
                })
                .collect();
        }
        block = block.rotated_row().unwrap_or(turn.block);
    }
    (block, state, kv)
}

/// A section as construction reads it: each cell's position with an index
/// into the record palette, and that palette.
pub type ConstructionSection = (Vec<([i32; 3], u16)>, Vec<super::CellData>);

impl Schematic {
    /// Section `index` turned `turns` quarter turns, as construction records:
    /// each stored cell's position within the turned design (its minimum
    /// corner at the origin) and an index into the section's record palette.
    /// A record whose names do not resolve stays in its stored names, and
    /// construction reports why it cannot be built. `None` = no such section.
    pub fn construction_section(&self, index: usize, turns: u8) -> Option<ConstructionSection> {
        let section = self.sections.get(index)?;
        let palette = section
            .palette
            .iter()
            .map(|data| match data.construction_record() {
                Ok(record) => {
                    let (block, state, kv) =
                        rotate_record(record.block, record.state, record.data, turns);
                    super::CellData::of_record(&petramond_world::construction::Record {
                        block,
                        state,
                        data: kv,
                    })
                }
                Err(_) => super::CellData {
                    kv: Default::default(),
                    container: None,
                    furnace: None,
                    fluid: 0,
                    ..data.clone()
                },
            })
            .collect();
        let cells = section
            .iter()
            .zip(&section.cells)
            .map(|(cell, stored)| (rotate_position(cell.pos, self.size, turns), stored.palette))
            .collect();
        Some((cells, palette))
    }
}

fn model_base(pos: IVec3, data: &ResolvedCell) -> Option<IVec3> {
    let kind = data.block.model_kind()?;
    let state = petramond_world::block_model::ModelCellState::from_cell(data.state);
    Some(petramond_world::block_model::base_from_cell(
        pos,
        kind,
        state.offset,
        state.facing,
    ))
}
