use petramond::schematic::Selection;
use wgpu::util::DeviceExt;

pub(crate) const SHADER: &str = include_str!("../shaders/selection_highlight.wgsl");

pub(crate) fn layout_entries() -> [wgpu::BindGroupLayoutEntry; 2] {
    [
        wgpu::BindGroupLayoutEntry {
            binding: 2,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(16),
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 3,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(32),
            },
            count: None,
        },
    ]
}

fn buffer(device: &wgpu::Device, size: u64, usage: wgpu::BufferUsages) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("selection highlight"),
        size,
        usage: usage | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn bind(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    buffers: [&wgpu::Buffer; 4],
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("selection highlight world uniforms"),
        layout,
        entries: &std::array::from_fn::<_, 4, _>(|i| wgpu::BindGroupEntry {
            binding: i as u32,
            resource: buffers[i].as_entire_binding(),
        }),
    })
}

pub(crate) fn inactive_bind(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    world: &wgpu::Buffer,
    uv: &wgpu::Buffer,
) -> wgpu::BindGroup {
    let cells = buffer(device, 16, wgpu::BufferUsages::STORAGE);
    let bounds = buffer(device, 32, wgpu::BufferUsages::UNIFORM);
    bind(device, layout, [world, uv, &cells, &bounds])
}

pub(crate) struct Highlight {
    layout: wgpu::BindGroupLayout,
    world: wgpu::Buffer,
    uv: wgpu::Buffer,
    cells: wgpu::Buffer,
    bounds: wgpu::Buffer,
    pub bind: wgpu::BindGroup,
}

impl Highlight {
    pub fn new(device: &wgpu::Device, layout: wgpu::BindGroupLayout, world: wgpu::Buffer) -> Self {
        // Terrain and world-model shaders share this layout but never read the UV table.
        let uv = buffer(
            device,
            (crate::uniforms::UV_RECTS_LEN * 16) as u64,
            wgpu::BufferUsages::UNIFORM,
        );
        let cells = buffer(device, 16, wgpu::BufferUsages::STORAGE);
        let bounds = buffer(device, 32, wgpu::BufferUsages::UNIFORM);
        let bind = bind(device, &layout, [&world, &uv, &cells, &bounds]);
        Self {
            layout,
            world,
            uv,
            cells,
            bounds,
            bind,
        }
    }

    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, selection: &Selection) {
        let mask = Mask::new(selection);
        let bytes = bytemuck::cast_slice(&mask.cells);
        if self.cells.size() < bytes.len() as u64 {
            self.cells = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("selection highlight boxes"),
                contents: bytes,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            });
            self.bind = bind(
                device,
                &self.layout,
                [&self.world, &self.uv, &self.cells, &self.bounds],
            );
        } else {
            queue.write_buffer(&self.cells, 0, bytes);
        }
        queue.write_buffer(&self.bounds, 0, bytemuck::cast_slice(&mask.bounds));
    }
}

struct Mask {
    cells: Vec<[i32; 4]>,
    bounds: [[i32; 4]; 2],
}

impl Mask {
    fn new(selection: &Selection) -> Self {
        let mut mask = Self {
            cells: Vec::new(),
            bounds: [[0; 4]; 2],
        };
        if selection.is_empty() {
            mask.cells.push([0; 4]);
            return mask;
        }
        let mut regions = selection.regions().to_vec();
        mask.branch(&mut regions);
        mask.bounds = [mask.cells[0], mask.cells[1]];
        mask.bounds[0][3] = (mask.cells.len() / 2) as i32;
        mask
    }

    // Preorder bounds with escape indices let fragments skip a whole missed subtree.
    fn branch(&mut self, regions: &mut [petramond::schematic::SelectionBox]) {
        let index = self.cells.len();
        let lo: [i32; 3] = std::array::from_fn(|i| regions.iter().map(|r| r.lo[i]).min().unwrap());
        let hi: [i32; 3] = std::array::from_fn(|i| regions.iter().map(|r| r.hi[i]).max().unwrap());
        self.cells
            .push([lo[0], lo[1], lo[2], i32::from(regions.len() == 1)]);
        self.cells.push([hi[0], hi[1], hi[2], 0]);
        if regions.len() > 1 {
            let axis = (0..3)
                .max_by_key(|i| i64::from(hi[*i]) - i64::from(lo[*i]))
                .unwrap();
            let middle = regions.len() / 2;
            regions.select_nth_unstable_by_key(middle, |r| {
                i64::from(r.lo[axis]) + i64::from(r.hi[axis])
            });
            let (left, right) = regions.split_at_mut(middle);
            self.branch(left);
            self.branch(right);
        }
        self.cells[index + 1][3] = (self.cells.len() / 2) as i32;
    }
}

#[cfg(test)]
mod tests;
