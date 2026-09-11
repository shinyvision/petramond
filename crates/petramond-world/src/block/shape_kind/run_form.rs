//! The RUN rule: which form a cell of a linear run takes from its place in
//! the run. Pure over two neighbour predicates, so the family, the placement
//! pre-write and the tests all resolve through one function.

use crate::mathh::IVec3;

use super::{RunForm, RunRoot, RUN_BASE, RUN_FRUSTUM, RUN_MERGE, RUN_MIDDLE, RUN_TIP};

/// Resolve the form of the run cell at `pos`.
///
/// `same(q)` says whether `q` holds a segment of THIS run (same kind — and
/// so the same root); `opposing(q)` whether it holds a run of the mirrored
/// kind (the other root along the same axis). Reads reach at most two cells
/// toward the free end and one toward the root, and read only IDENTITY,
/// never a neighbour's refined byte — so the cascade stays acyclic and a
/// change two cells away always flips the cell in between (the neighbour's
/// own free-end test), which is what carries it here.
///
/// - the free end (`tip`) when nothing of the run continues tipward — or
///   `merge` when what continues is an opposing run's free end, two spikes
///   meeting into a column;
/// - `frustum` when the tipward neighbour IS the free end;
/// - otherwise `base` when the rootward cell is not part of the run (the
///   attached segment), `middle` when it is.
pub fn run_form(
    pos: IVec3,
    root: RunRoot,
    same: impl Fn(IVec3) -> bool,
    opposing: impl Fn(IVec3) -> bool,
) -> RunForm {
    let tip = root.tip();
    if !same(pos + tip) {
        return if opposing(pos + tip) {
            RUN_MERGE
        } else {
            RUN_TIP
        };
    }
    if !same(pos + tip * 2) {
        return RUN_FRUSTUM;
    }
    if same(pos + root.dir()) {
        RUN_MIDDLE
    } else {
        RUN_BASE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A four-long hanging run over air reads base / middle / frustum / tip
    /// from the ceiling down, and every interior read is identity only.
    #[test]
    fn a_hanging_run_tapers_from_its_root_to_its_free_end() {
        let run: Vec<IVec3> = (0..4).map(|i| IVec3::new(0, 10 - i, 0)).collect();
        let same = |q: IVec3| run.contains(&q);
        let none = |_: IVec3| false;
        let forms: Vec<RunForm> = run
            .iter()
            .map(|&p| run_form(p, RunRoot::Up, same, none))
            .collect();
        assert_eq!(forms, [RUN_BASE, RUN_MIDDLE, RUN_FRUSTUM, RUN_TIP]);
    }

    /// The same rule mirrored: a standing run tapers upward, and a lone cell
    /// is a free end (a single spike), never a base.
    #[test]
    fn a_standing_run_tapers_upward_and_a_lone_cell_is_a_tip() {
        let run: Vec<IVec3> = (0..3).map(|i| IVec3::new(0, i, 0)).collect();
        let same = |q: IVec3| run.contains(&q);
        let none = |_: IVec3| false;
        let forms: Vec<RunForm> = run
            .iter()
            .map(|&p| run_form(p, RunRoot::Down, same, none))
            .collect();
        assert_eq!(forms, [RUN_BASE, RUN_FRUSTUM, RUN_TIP]);
        assert_eq!(
            run_form(IVec3::new(5, 5, 5), RunRoot::Down, |_| false, |_| false),
            RUN_TIP
        );
    }

    /// Two runs meeting tip to tip form a column: both free ends read
    /// `merge`, and the segments behind them still read `frustum`.
    #[test]
    fn opposing_free_ends_merge() {
        let hanging = [IVec3::new(0, 6, 0), IVec3::new(0, 5, 0)];
        let standing = [IVec3::new(0, 3, 0), IVec3::new(0, 4, 0)];
        let h = |q: IVec3| hanging.contains(&q);
        let s = |q: IVec3| standing.contains(&q);
        assert_eq!(run_form(IVec3::new(0, 5, 0), RunRoot::Up, h, s), RUN_MERGE);
        assert_eq!(
            run_form(IVec3::new(0, 4, 0), RunRoot::Down, s, h),
            RUN_MERGE
        );
        assert_eq!(
            run_form(IVec3::new(0, 6, 0), RunRoot::Up, h, s),
            RUN_FRUSTUM
        );
        assert_eq!(
            run_form(IVec3::new(0, 3, 0), RunRoot::Down, s, h),
            RUN_FRUSTUM
        );
    }

    /// The cascade only revisits neighbours of a CHANGED cell, so a cell's
    /// two-deep read must be covered by a one-deep change: whenever the cell
    /// two tipward flips identity, the cell one tipward changes form. Pinned
    /// over every neighbourhood the rule can see.
    #[test]
    fn a_two_deep_change_always_changes_the_cell_between() {
        let p = IVec3::ZERO;
        for root in [RunRoot::Up, RunRoot::Down] {
            let tip = root.tip();
            for rootward in [false, true] {
                for far in [false, true] {
                    // The neighbour one tipward is part of the run (else the
                    // cell never reads two deep at all).
                    let same_with = |two: bool| {
                        move |q: IVec3| {
                            q == p
                                || (q == p + tip)
                                || (q == p + tip * 2 && two)
                                || (q == p + root.dir() && rootward)
                                || (q == p + tip * 3 && far)
                        }
                    };
                    let before = run_form(p + tip, root, same_with(false), |_| false);
                    let after = run_form(p + tip, root, same_with(true), |_| false);
                    assert_ne!(
                        before, after,
                        "root {root:?}: the neighbour must change when its own tipward cell does"
                    );
                }
            }
        }
    }
}
