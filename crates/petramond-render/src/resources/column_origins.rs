/// The shared instance-step table of per-column world XZ origins.
///
/// `vs_terrain` reconstructs absolute positions from a column-local vertex plus
/// this origin. It used to be a 16-byte GPU buffer PER COLUMN, which cost a
/// `set_vertex_buffer` on every single terrain draw — a quarter of the frame's
/// recorded commands at high render distance, and thousands of tiny buffer
/// objects for the driver and wgpu's submit-time resource tracker to carry.
/// Now every column indexes ONE array and the draw selects its row through
/// `first_instance`, so the bind happens once per pass.
pub struct ColumnOrigins {
    buf: wgpu::Buffer,
    /// CPU mirror, so growing the buffer is one write of everything live.
    values: Vec<[f32; 4]>,
    free: std::sync::Arc<std::sync::Mutex<Vec<u32>>>,
}

/// A column's row in [`ColumnOrigins`], returned to the free list on drop —
/// which is what keeps the table bounded across every path that can drop a
/// column (retain, remove, clear).
pub struct ColumnOriginSlot {
    index: u32,
    free: std::sync::Arc<std::sync::Mutex<Vec<u32>>>,
}

impl ColumnOriginSlot {
    #[inline]
    pub fn index(&self) -> u32 {
        self.index
    }
}

impl Drop for ColumnOriginSlot {
    fn drop(&mut self) {
        if let Ok(mut free) = self.free.lock() {
            free.push(self.index);
        }
    }
}

/// Rows the table starts with; it doubles from here.
const COLUMN_ORIGIN_INITIAL: u32 = 2048;

impl ColumnOrigins {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        Self {
            buf: Self::create(device, COLUMN_ORIGIN_INITIAL),
            values: Vec::new(),
            free: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    fn create(device: &wgpu::Device, rows: u32) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("column origins"),
            size: u64::from(rows) * 16,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    pub fn buffer(&self) -> &wgpu::Buffer {
        &self.buf
    }

    /// Claim (or refresh) the row holding `(col_ox, col_oz)`.
    pub(super) fn slot(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        prev: Option<ColumnOriginSlot>,
        col_ox: i32,
        col_oz: i32,
    ) -> ColumnOriginSlot {
        let value = [col_ox as f32, 0.0, col_oz as f32, 0.0];
        let slot = match prev {
            Some(s) => s,
            None => {
                let reused = self.free.lock().ok().and_then(|mut f| f.pop());
                let index = match reused {
                    Some(i) => i,
                    None => {
                        let i = self.values.len() as u32;
                        self.values.push(value);
                        i
                    }
                };
                ColumnOriginSlot {
                    index,
                    free: std::sync::Arc::clone(&self.free),
                }
            }
        };
        let i = slot.index as usize;
        if i >= self.values.len() {
            self.values.resize(i + 1, [0.0; 4]);
        }
        self.values[i] = value;
        let rows = (self.buf.size() / 16) as u32;
        if slot.index >= rows {
            let mut grown = rows.max(1);
            while slot.index >= grown {
                grown *= 2;
            }
            self.buf = Self::create(device, grown);
            queue.write_buffer(&self.buf, 0, bytemuck::cast_slice(&self.values));
        } else {
            queue.write_buffer(
                &self.buf,
                u64::from(slot.index) * 16,
                bytemuck::bytes_of(&value),
            );
        }
        slot
    }
}
