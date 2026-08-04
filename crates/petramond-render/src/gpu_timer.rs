//! Per-pass GPU timing for the frame graph, off unless asked for.
//!
//! `perf` hardware counters are unavailable on the development machine, so
//! wgpu timestamp queries are the only instrument that can say which render
//! pass a frame is actually spent in. The timer is opt-in
//! (`PETRAMOND_GPU_TIMING=1`) and inert otherwise: with it off the renderer
//! holds `None`, every pass descriptor takes `timestamp_writes: None`, and not
//! one extra GPU command is recorded.
//!
//! Timestamps ride the render-pass descriptor (`TIMESTAMP_QUERY`), not encoder
//! writes, so a pass is measured without changing what it records. Reading them
//! back blocks on the submit, which serializes CPU and GPU — fine for the
//! question it answers (which pass costs what), useless for whole-frame
//! throughput, which is why it is opt-in.

use std::cell::RefCell;

/// Passes the timer can measure. Sized generously; a pass that does not run in
/// a given frame simply writes no pair.
const MAX_PASSES: u32 = 32;
const MAX_QUERIES: u32 = MAX_PASSES * 2;

pub(crate) struct GpuTimer {
    query_set: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
    period_ns: f32,
    state: RefCell<TimerState>,
}

#[derive(Default)]
struct TimerState {
    /// Labels of the passes that wrote a pair this frame, in write order.
    labels: Vec<&'static str>,
    /// Queries written so far this frame.
    used: u32,
    /// Accumulated per-label totals in nanoseconds and the frame count they
    /// cover, so a caller can report a mean over many frames.
    totals: Vec<(&'static str, f64, u32)>,
    /// The same accumulation for CPU stages of the frame, which the GPU
    /// timestamps cannot see.
    cpu: Vec<(&'static str, f64, u32)>,
}

impl GpuTimer {
    /// A timer over `device`, or `None` when GPU timing was not requested or
    /// the adapter cannot do it. Call before `request_device` decides features.
    pub(crate) fn wanted() -> bool {
        std::env::var("PETRAMOND_GPU_TIMING").is_ok_and(|v| v != "0")
    }

    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Self> {
        if !Self::wanted() || !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            return None;
        }
        let query_set = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("frame timestamps"),
            ty: wgpu::QueryType::Timestamp,
            count: MAX_QUERIES,
        });
        let resolve = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("timestamp resolve"),
            size: u64::from(MAX_QUERIES) * 8,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("timestamp readback"),
            size: u64::from(MAX_QUERIES) * 8,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Some(Self {
            query_set,
            resolve,
            readback,
            period_ns: queue.get_timestamp_period(),
            state: RefCell::new(TimerState::default()),
        })
    }

    /// Claim a begin/end query pair for `label`, or `None` once the frame has
    /// used them all.
    pub(crate) fn pass<'a>(
        &'a self,
        label: &'static str,
    ) -> Option<wgpu::RenderPassTimestampWrites<'a>> {
        let mut st = self.state.borrow_mut();
        if st.used + 2 > MAX_QUERIES {
            return None;
        }
        let base = st.used;
        st.used += 2;
        st.labels.push(label);
        Some(wgpu::RenderPassTimestampWrites {
            query_set: &self.query_set,
            beginning_of_pass_write_index: Some(base),
            end_of_pass_write_index: Some(base + 1),
        })
    }

    /// Record the resolve + copy for this frame's queries. Must be the last
    /// thing encoded.
    pub(crate) fn finish_frame(&self, enc: &mut wgpu::CommandEncoder) {
        let used = self.state.borrow().used;
        if used == 0 {
            return;
        }
        enc.resolve_query_set(&self.query_set, 0..used, &self.resolve, 0);
        enc.copy_buffer_to_buffer(&self.resolve, 0, &self.readback, 0, u64::from(used) * 8);
    }

    /// After a submit: hand the frame's labels to the readback slot. The
    /// previous frame's numbers (if any) are folded into the totals first.
    pub(crate) fn after_submit(&self, device: &wgpu::Device) {
        let mut st = self.state.borrow_mut();
        if st.used == 0 {
            st.labels.clear();
            return;
        }
        let labels = std::mem::take(&mut st.labels);
        st.used = 0;
        drop(st);
        self.readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, |_| {});
        let _ = device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        {
            let mapped = self.readback.slice(..).get_mapped_range();
            let mut st = self.state.borrow_mut();
            for (i, label) in labels.iter().enumerate() {
                let a = u64::from_le_bytes(mapped[i * 16..i * 16 + 8].try_into().unwrap());
                let b = u64::from_le_bytes(mapped[i * 16 + 8..i * 16 + 16].try_into().unwrap());
                let ns = b.saturating_sub(a) as f64 * f64::from(self.period_ns);
                match st.totals.iter_mut().find(|(l, _, _)| l == label) {
                    Some(e) => {
                        e.1 += ns;
                        e.2 += 1;
                    }
                    None => st.totals.push((label, ns, 1)),
                }
            }
        }
        self.readback.unmap();
    }

    /// Fold one CPU stage sample into the report.
    pub(crate) fn cpu_stage(&self, label: &'static str, ns: f64) {
        let mut st = self.state.borrow_mut();
        match st.cpu.iter_mut().find(|(l, _, _)| *l == label) {
            Some(e) => {
                e.1 += ns;
                e.2 += 1;
            }
            None => st.cpu.push((label, ns, 1)),
        }
    }

    /// Mean nanoseconds per measured frame, per pass, in first-seen order.
    pub(crate) fn report(&self) -> Vec<(&'static str, f64, u32)> {
        self.state.borrow().totals.clone()
    }

    pub(crate) fn report_cpu(&self) -> Vec<(&'static str, f64, u32)> {
        self.state.borrow().cpu.clone()
    }

    pub(crate) fn reset(&self) {
        let mut st = self.state.borrow_mut();
        st.totals.clear();
        st.cpu.clear();
    }
}
