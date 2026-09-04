//! Rail forms and the connection rule — how a placed rail decides which of
//! its neighbours it joins, and which of them turn to meet it.
//!
//! A rail cell has exactly two EXITS. A straight has one at each end of its
//! axis, a curve two adjacent ones, a slope one level exit at its foot and one
//! exit that steps UP a cell at its head. Two rails are LINKED when each has
//! an exit that leads to the other; an exit leading to no rail (or to a rail
//! that does not point back) is a FREE SLOT. Placing a rail joins it to up to
//! two neighbours that have a free slot toward it (or already point at it),
//! and a neighbour that takes the new link turns to accommodate it — a lone
//! rail swings to face the newcomer, a straight with one real link becomes
//! the curve or slope that reaches both. Removing a rail changes nothing
//! around it: its neighbours keep pointing at the gap, so a run rebuilt over
//! it comes back exactly as it was.
//!
//! Everything here is pure over a [`RailMap`]; the mod supplies the map from
//! one batched block read and applies the resolution's row swaps.

/// A horizontal cardinal direction. `N` is `-Z`, `E` is `+X` — the engine's
/// world axes, which is also what the rail art is authored against (north up).
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Dir {
    N,
    E,
    S,
    W,
}

impl Dir {
    pub const ALL: [Dir; 4] = [Dir::N, Dir::E, Dir::S, Dir::W];

    pub fn opposite(self) -> Dir {
        match self {
            Dir::N => Dir::S,
            Dir::E => Dir::W,
            Dir::S => Dir::N,
            Dir::W => Dir::E,
        }
    }

    pub fn axis(self) -> Axis {
        match self {
            Dir::N | Dir::S => Axis::NS,
            Dir::E | Dir::W => Axis::EW,
        }
    }

    /// The cell step this direction takes.
    pub fn offset(self) -> [i32; 3] {
        match self {
            Dir::N => [0, 0, -1],
            Dir::E => [1, 0, 0],
            Dir::S => [0, 0, 1],
            Dir::W => [-1, 0, 0],
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Axis {
    NS,
    EW,
}

/// The corner a curve turns through, named by its two exits: `NE` leads out
/// north and east. A type of exactly the four legal pairs, so a curve can
/// never be asked to join two opposite or two equal directions.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Corner {
    NE,
    SE,
    SW,
    NW,
}

impl Corner {
    /// The corner whose exits are `a` and `b` in either order, if they are
    /// adjacent directions.
    pub fn of(a: Dir, b: Dir) -> Option<Corner> {
        let (ns, ew) = match (a.axis(), b.axis()) {
            (Axis::NS, Axis::EW) => (a, b),
            (Axis::EW, Axis::NS) => (b, a),
            _ => return None,
        };
        Some(match (ns, ew) {
            (Dir::N, Dir::E) => Corner::NE,
            (Dir::S, Dir::E) => Corner::SE,
            (Dir::S, Dir::W) => Corner::SW,
            (Dir::N, Dir::W) => Corner::NW,
            _ => unreachable!("an axis pair is one north/south and one east/west direction"),
        })
    }

    /// The two exits, north/south first.
    pub fn exits(self) -> [Dir; 2] {
        match self {
            Corner::NE => [Dir::N, Dir::E],
            Corner::SE => [Dir::S, Dir::E],
            Corner::SW => [Dir::S, Dir::W],
            Corner::NW => [Dir::N, Dir::W],
        }
    }
}

/// One end of a rail: the direction it leads out of the cell, and whether it
/// leads to the cell one HIGHER (the head of a slope). A level exit leads to
/// the neighbour at the same height, or down onto a slope rising to meet it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Exit {
    pub dir: Dir,
    pub up: bool,
}

impl Exit {
    pub const fn level(dir: Dir) -> Exit {
        Exit { dir, up: false }
    }
    pub const fn up(dir: Dir) -> Exit {
        Exit { dir, up: true }
    }
}

/// The ten shapes a rail cell can take. `Slope(d)` ascends TOWARD `d`; a
/// curve turns through its corner.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Form {
    Straight(Axis),
    Curve(Corner),
    Slope(Dir),
}

impl Form {
    /// Every form, in the order the pack's block rows are named.
    pub const ALL: [Form; 10] = [
        Form::Straight(Axis::NS),
        Form::Straight(Axis::EW),
        Form::Curve(Corner::NE),
        Form::Curve(Corner::SE),
        Form::Curve(Corner::SW),
        Form::Curve(Corner::NW),
        Form::Slope(Dir::N),
        Form::Slope(Dir::E),
        Form::Slope(Dir::S),
        Form::Slope(Dir::W),
    ];

    /// The row-name suffix of this form (`vehicles:rail_<name>`).
    pub fn name(self) -> &'static str {
        match self {
            Form::Straight(Axis::NS) => "ns",
            Form::Straight(Axis::EW) => "ew",
            Form::Curve(Corner::NE) => "curve_ne",
            Form::Curve(Corner::SE) => "curve_se",
            Form::Curve(Corner::SW) => "curve_sw",
            Form::Curve(Corner::NW) => "curve_nw",
            Form::Slope(Dir::N) => "slope_n",
            Form::Slope(Dir::E) => "slope_e",
            Form::Slope(Dir::S) => "slope_s",
            Form::Slope(Dir::W) => "slope_w",
        }
    }

    pub fn is_curve(self) -> bool {
        matches!(self, Form::Curve(..))
    }

    /// The two exits, in a fixed order: a straight's north/west end first, a
    /// curve's north/south exit first, a slope's FOOT (level exit) first.
    pub fn exits(self) -> [Exit; 2] {
        match self {
            Form::Straight(Axis::NS) => [Exit::level(Dir::N), Exit::level(Dir::S)],
            Form::Straight(Axis::EW) => [Exit::level(Dir::W), Exit::level(Dir::E)],
            Form::Curve(c) => c.exits().map(Exit::level),
            Form::Slope(d) => [Exit::level(d.opposite()), Exit::up(d)],
        }
    }

    /// The exit leading out in `dir`, if this form has one.
    pub fn exit_toward(self, dir: Dir) -> Option<Exit> {
        self.exits().into_iter().find(|e| e.dir == dir)
    }

    /// The form joining exactly these two exits, or `None` when no rail can:
    /// a curve never climbs, and nothing climbs both ways.
    pub fn from_exits(a: Exit, b: Exit) -> Option<Form> {
        if a.dir == b.dir {
            return None;
        }
        if a.dir == b.dir.opposite() {
            return match (a.up, b.up) {
                (false, false) => Some(Form::Straight(a.dir.axis())),
                (true, false) => Some(Form::Slope(a.dir)),
                (false, true) => Some(Form::Slope(b.dir)),
                (true, true) => None,
            };
        }
        if a.up || b.up {
            return None;
        }
        Corner::of(a.dir, b.dir).map(Form::Curve)
    }

    /// The form a rail takes when it links one way only: a straight along
    /// that direction's axis, or the slope climbing toward it.
    pub fn from_single(e: Exit) -> Form {
        if e.up {
            Form::Slope(e.dir)
        } else {
            Form::Straight(e.dir.axis())
        }
    }
}

/// A rail cell as the connection rule sees it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Rail {
    pub form: Form,
    /// A booster rail is straight or sloped, never curved.
    pub booster: bool,
}

impl Rail {
    /// Whether this rail kind may take `form`.
    pub fn allows(self, form: Form) -> bool {
        !(self.booster && form.is_curve())
    }
}

/// What the rule reads: the rail at a cell, if any. Unloaded cells read as no
/// rail — the rule is best effort at the edge of the loaded world.
pub trait RailMap {
    fn rail(&self, cell: [i32; 3]) -> Option<Rail>;
}

pub fn add(cell: [i32; 3], off: [i32; 3]) -> [i32; 3] {
    [cell[0] + off[0], cell[1] + off[1], cell[2] + off[2]]
}

/// Where an exit leads: the rail cell it joins and that rail's exit pointing
/// back — `None` when nothing joins there. A level exit joins the neighbour
/// at the same height pointing back level, or else the neighbour one lower
/// whose slope climbs to meet it; an up exit joins the neighbour one higher
/// pointing back level.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub cell: [i32; 3],
    pub entry: Exit,
}

pub fn link(map: &impl RailMap, cell: [i32; 3], exit: Exit) -> Option<Link> {
    let back = exit.dir.opposite();
    let level = add(cell, exit.dir.offset());
    if exit.up {
        let target = add(level, [0, 1, 0]);
        let rail = map.rail(target)?;
        return (rail.form.exit_toward(back) == Some(Exit::level(back))).then_some(Link {
            cell: target,
            entry: Exit::level(back),
        });
    }
    if let Some(rail) = map.rail(level) {
        if rail.form.exit_toward(back) == Some(Exit::level(back)) {
            return Some(Link {
                cell: level,
                entry: Exit::level(back),
            });
        }
        return None;
    }
    let below = add(level, [0, -1, 0]);
    let rail = map.rail(below)?;
    (rail.form.exit_toward(back) == Some(Exit::up(back))).then_some(Link {
        cell: below,
        entry: Exit::up(back),
    })
}

/// The links a rail actually has (each exit that leads to a rail pointing
/// back), skipping links to `ignore` — the cell being placed, whose default
/// row must not count as a link it has not yet chosen.
fn real_links(
    map: &impl RailMap,
    cell: [i32; 3],
    rail: Rail,
    ignore: [i32; 3],
) -> Vec<(Exit, Link)> {
    rail.form
        .exits()
        .into_iter()
        .filter_map(|e| link(map, cell, e).map(|l| (e, l)))
        .filter(|(_, l)| l.cell != ignore)
        .collect()
}

/// A neighbour that could take the new rail's link.
#[derive(Copy, Clone, Debug, PartialEq)]
struct Candidate {
    /// The exit the NEW rail would use toward it.
    exit: Exit,
    cell: [i32; 3],
    /// The form the neighbour takes to accept the link (`None` = it already
    /// points at the new rail and keeps its form).
    turns_into: Option<Form>,
    /// Already pointing at the new rail — preferred, so a run being extended
    /// keeps its shape.
    linked: bool,
}

/// The outcome of placing a rail: its own form and every neighbour that
/// turns to meet it.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolution {
    pub form: Form,
    pub turns: Vec<([i32; 3], Form)>,
}

/// Resolve the form of a rail placed at `cell` (kind `placed`, already in the
/// map as its default row) and the neighbours that turn to join it.
pub fn resolve_placement(map: &impl RailMap, cell: [i32; 3], placed: Rail) -> Resolution {
    let mut candidates: Vec<Candidate> = Vec::new();
    for dir in Dir::ALL {
        for dy in [1, 0, -1] {
            let n = add(add(cell, dir.offset()), [0, dy, 0]);
            let Some(rail) = map.rail(n) else { continue };
            // A neighbour above is reached by climbing; one below climbs to
            // us and so uses an UP exit of its own.
            let exit = Exit { dir, up: dy == 1 };
            let back = Exit {
                dir: dir.opposite(),
                up: dy == -1,
            };
            let links = real_links(map, n, rail, cell);
            let linked = rail.form.exit_toward(back.dir) == Some(back);
            let turns_into = if linked {
                None
            } else if links.len() >= 2 {
                continue;
            } else {
                let form = match links.first() {
                    Some((e, _)) => Form::from_exits(*e, back),
                    None => Some(Form::from_single(back)),
                };
                match form {
                    Some(f) if rail.allows(f) => Some(f),
                    _ => continue,
                }
            };
            candidates.push(Candidate {
                exit,
                cell: n,
                turns_into,
                linked,
            });
        }
    }
    // Rank: rails already pointing at us, then climbs (a raised neighbour
    // can only ever be joined by a slope, so it must not lose to a level
    // one), then level joins; ties by compass order.
    candidates.sort_by_key(|c| (!c.linked, !c.exit.up));

    let mut chosen: Vec<Candidate> = Vec::new();
    let mut form = None;
    for c in &candidates {
        match chosen.first() {
            None => {
                let single = Form::from_single(c.exit);
                if placed.allows(single) {
                    chosen.push(*c);
                    form = Some(single);
                }
            }
            Some(first) => {
                if let Some(pair) = Form::from_exits(first.exit, c.exit) {
                    if placed.allows(pair) {
                        chosen.push(*c);
                        form = Some(pair);
                        break;
                    }
                }
            }
        }
    }
    Resolution {
        form: form.unwrap_or(Form::Straight(Axis::NS)),
        turns: chosen
            .into_iter()
            .filter_map(|c| c.turns_into.map(|f| (c.cell, f)))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct Map(BTreeMap<[i32; 3], Rail>);

    impl Map {
        fn with(mut self, cell: [i32; 3], form: Form) -> Self {
            self.0.insert(
                cell,
                Rail {
                    form,
                    booster: false,
                },
            );
            self
        }
        fn booster(mut self, cell: [i32; 3], form: Form) -> Self {
            self.0.insert(
                cell,
                Rail {
                    form,
                    booster: true,
                },
            );
            self
        }
        fn apply(&mut self, cell: [i32; 3], r: &Resolution) {
            let booster = self.0[&cell].booster;
            self.0.insert(
                cell,
                Rail {
                    form: r.form,
                    booster,
                },
            );
            for (c, f) in &r.turns {
                let b = self.0[c].booster;
                self.0.insert(
                    *c,
                    Rail {
                        form: *f,
                        booster: b,
                    },
                );
            }
        }
        fn place(&mut self, cell: [i32; 3]) -> Resolution {
            self.0.insert(
                cell,
                Rail {
                    form: Form::Straight(Axis::NS),
                    booster: false,
                },
            );
            let r = resolve_placement(
                self,
                cell,
                Rail {
                    form: Form::Straight(Axis::NS),
                    booster: false,
                },
            );
            self.apply(cell, &r);
            r
        }
    }

    impl RailMap for Map {
        fn rail(&self, cell: [i32; 3]) -> Option<Rail> {
            self.0.get(&cell).copied()
        }
    }

    const NS: Form = Form::Straight(Axis::NS);
    const EW: Form = Form::Straight(Axis::EW);

    #[test]
    fn every_form_joins_exactly_its_two_exits_and_back() {
        for f in Form::ALL {
            let [a, b] = f.exits();
            assert_ne!(a.dir, b.dir);
            assert_eq!(Form::from_exits(a, b), Some(f), "{f:?}");
            assert_eq!(Form::from_exits(b, a), Some(f), "{f:?}");
        }
        assert_eq!(Form::from_exits(Exit::up(Dir::N), Exit::up(Dir::S)), None);
        assert_eq!(
            Form::from_exits(Exit::up(Dir::N), Exit::level(Dir::E)),
            None
        );
        assert_eq!(
            Form::from_exits(Exit::level(Dir::N), Exit::level(Dir::N)),
            None
        );
    }

    #[test]
    fn a_lone_rail_is_north_south_and_a_neighbour_swings_to_meet_a_newcomer() {
        let mut m = Map::default();
        assert_eq!(m.place([0, 0, 0]).form, NS);
        // Placing to the EAST of a lone rail: it turns east-west to reach us.
        let r = m.place([1, 0, 0]);
        assert_eq!(r.form, EW);
        assert_eq!(r.turns, vec![([0, 0, 0], EW)]);
    }

    #[test]
    fn a_run_extends_straight_and_a_side_rail_curves_into_a_free_end() {
        let mut m = Map::default();
        m.place([0, 0, 0]);
        m.place([0, 0, 1]);
        m.place([0, 0, 2]);
        assert!(m.0.values().all(|r| r.form == NS));
        // The middle rail has both slots taken: a rail beside it stays alone.
        let r = m.place([1, 0, 1]);
        assert_eq!(r.form, NS);
        assert!(
            r.turns.is_empty(),
            "a full rail does not turn: {:?}",
            r.turns
        );
        m.0.remove(&[1, 0, 1]);
        // The run's end has a free slot: a rail beside it makes a corner.
        let r = m.place([1, 0, 2]);
        assert_eq!(r.form, EW, "the newcomer runs east-west into the corner");
        assert_eq!(r.turns, vec![([0, 0, 2], Form::Curve(Corner::NE))]);
    }

    #[test]
    fn a_raised_neighbour_is_reached_by_a_slope_and_a_lower_one_climbs_to_us() {
        let mut m = Map::default();
        m.place([0, 0, 0]);
        // One higher, to the north: we slope up toward it.
        let r = m.place([0, 1, -1]);
        // From the raised cell's view the lower rail is level-linked through
        // the lower rail's climb; the raised rail itself is straight.
        assert_eq!(r.form, NS);
        assert_eq!(r.turns, vec![([0, 0, 0], Form::Slope(Dir::N))]);
        assert_eq!(
            link(&m, [0, 0, 0], Exit::up(Dir::N)),
            Some(Link {
                cell: [0, 1, -1],
                entry: Exit::level(Dir::S)
            })
        );
        assert_eq!(
            link(&m, [0, 1, -1], Exit::level(Dir::S)),
            Some(Link {
                cell: [0, 0, 0],
                entry: Exit::up(Dir::N)
            }),
            "a level exit reaches down onto the slope climbing to it"
        );
        // Placing BELOW the foot of the slope: the newcomer climbs.
        let r = m.place([0, -1, 1]);
        assert_eq!(r.form, Form::Slope(Dir::N));
        assert!(r.turns.is_empty(), "the slope's foot already faced south");
    }

    #[test]
    fn a_curve_never_takes_a_climb_and_a_booster_never_curves() {
        let mut m = Map::default()
            .with([0, 0, 0], Form::Curve(Corner::NE))
            .with([1, 0, 0], EW);
        // A rail placed raised at the curve's north exit cannot be joined:
        // the curve would have to climb toward it.
        let r = m.place([0, 1, -1]);
        assert!(r.turns.is_empty(), "{:?}", r.turns);
        assert_eq!(r.form, NS);
        // Level there, it joins: the curve already points north.
        m.0.remove(&[0, 1, -1]);
        let r = m.place([0, 0, -1]);
        assert!(r.turns.is_empty());
        assert_eq!(r.form, NS);

        let mut b = Map::default().booster([0, 0, 0], NS);
        b.place([0, 0, -1]);
        let r = b.place([1, 0, 0]);
        assert!(
            r.turns.is_empty(),
            "a booster with a link keeps straight rather than curving"
        );
        assert_eq!(r.form, NS);
    }

    #[test]
    fn removal_leaves_the_neighbours_pointing_at_the_gap() {
        let mut m = Map::default();
        m.place([0, 0, 0]);
        m.place([1, 0, 0]);
        m.place([1, 0, 1]);
        assert_eq!(m.0[&[1, 0, 0]].form, Form::Curve(Corner::SW));
        m.0.remove(&[1, 0, 1]);
        assert_eq!(m.0[&[1, 0, 0]].form, Form::Curve(Corner::SW));
        // Rebuilding the removed rail restores the same picture.
        let r = m.place([1, 0, 1]);
        assert_eq!(r.form, NS);
        assert!(r.turns.is_empty());
    }
}
