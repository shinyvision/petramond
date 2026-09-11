use std::ops::{ControlFlow, Range};

use super::Cut;

struct Node {
    bounds: [[i32; 3]; 2],
    cuts: Range<usize>,
    end: usize,
}

/// Preorder bounds permit stack-free queries over arbitrarily shaped batches.
/// Leaves retain exact ellipsoid evaluation; only disjoint subtrees are skipped.
#[derive(Default)]
pub(super) struct Index(Vec<Node>);

impl Index {
    pub(super) fn build(cuts: &mut [Cut]) -> Self {
        let mut out = Self::default();
        if !cuts.is_empty() {
            out.partition(cuts, 0);
        }
        out
    }

    fn partition(&mut self, cuts: &mut [Cut], start: usize) {
        let mut bounds = [[i32::MAX; 3], [i32::MIN; 3]];
        for cut in cuts.iter() {
            for (axis, (center, extent)) in cut.center.into_iter().zip(cut.extent).enumerate() {
                // Outward integer rounding keeps boundary samples inside despite
                // floating-point subtraction at an ellipsoid's support boundary.
                bounds[0][axis] = bounds[0][axis].min((center - extent).floor() as i32);
                bounds[1][axis] = bounds[1][axis].max((center + extent).ceil() as i32);
            }
        }
        let index = self.0.len();
        self.0.push(Node {
            bounds,
            cuts: start..start + cuts.len(),
            end: 0,
        });
        if cuts.len() > 8 {
            let axis = (0..3).max_by_key(|&a| bounds[1][a] - bounds[0][a]).unwrap();
            let middle = cuts.len() / 2;
            cuts.select_nth_unstable_by(middle, |a, b| a.center[axis].total_cmp(&b.center[axis]));
            let (left, right) = cuts.split_at_mut(middle);
            self.partition(left, start);
            self.partition(right, start + middle);
            self.0[index].cuts = 0..0;
        }
        self.0[index].end = self.0.len();
    }

    pub(super) fn at(&self, cuts: &[Cut], p: [f64; 3]) -> f64 {
        let mut value = 1.0_f64;
        let mut i = 0;
        while let Some(node) = self.0.get(i) {
            if (0..3).any(|a| p[a] < node.bounds[0][a] as f64 || p[a] > node.bounds[1][a] as f64) {
                i = node.end;
                continue;
            }
            for cut in &cuts[node.cuts.clone()] {
                value = value.min(cut.density(p));
            }
            i += 1;
        }
        value
    }

    pub(super) fn intersects(&self, cuts: &[Cut], [lo, hi]: [[i32; 3]; 2]) -> bool {
        self.visit(cuts, [lo, hi], |_| ControlFlow::Break(()))
            .is_break()
    }

    pub(super) fn visit(
        &self,
        cuts: &[Cut],
        [lo, hi]: [[i32; 3]; 2],
        mut visit: impl FnMut(Cut) -> ControlFlow<()>,
    ) -> ControlFlow<()> {
        let mut i = 0;
        while let Some(node) = self.0.get(i) {
            if (0..3).any(|a| hi[a] < node.bounds[0][a] || lo[a] > node.bounds[1][a]) {
                i = node.end;
                continue;
            }
            for &cut in cuts[node.cuts.clone()]
                .iter()
                .filter(|cut| cut.intersects([lo, hi]))
            {
                visit(cut)?;
            }
            i += 1;
        }
        ControlFlow::Continue(())
    }
}
