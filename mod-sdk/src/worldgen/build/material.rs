use std::collections::BTreeMap;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::{LazyLock, Mutex};

use crate::FxHashMap;

/// A horizontal direction; north is -Z, east is +X.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Dir {
    North,
    East,
    South,
    West,
}

impl Dir {
    pub const ALL: [Dir; 4] = [Dir::North, Dir::East, Dir::South, Dir::West];

    #[inline]
    pub fn offset(self) -> [i32; 2] {
        match self {
            Dir::North => [0, -1],
            Dir::East => [1, 0],
            Dir::South => [0, 1],
            Dir::West => [-1, 0],
        }
    }

    #[inline]
    pub fn opposite(self) -> Dir {
        self.turn(2)
    }

    /// Quarter turns clockwise seen from above.
    #[inline]
    pub fn turn(self, quarters: u8) -> Dir {
        Dir::ALL[(self as usize + quarters as usize) % 4]
    }

    /// The direction whose axis dominates `(dx, dz)`; ties go to the X axis.
    #[inline]
    pub fn of(dx: f32, dz: f32) -> Dir {
        if dx.abs() >= dz.abs() {
            if dx >= 0.0 {
                Dir::East
            } else {
                Dir::West
            }
        } else if dz >= 0.0 {
            Dir::South
        } else {
            Dir::North
        }
    }

    #[inline]
    pub fn axis(self) -> Axis {
        match self {
            Dir::East | Dir::West => Axis::X,
            Dir::North | Dir::South => Axis::Z,
        }
    }

    #[inline]
    pub fn name(self) -> &'static str {
        match self {
            Dir::North => "north",
            Dir::East => "east",
            Dir::South => "south",
            Dir::West => "west",
        }
    }

    pub fn parse(name: &str) -> Option<Dir> {
        Dir::ALL.into_iter().find(|d| d.name() == name)
    }

    #[inline]
    pub fn step(self, [x, z]: [i32; 2], n: i32) -> [i32; 2] {
        let [dx, dz] = self.offset();
        [x + dx * n, z + dz * n]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    #[inline]
    pub fn name(self) -> &'static str {
        match self {
            Axis::X => "x",
            Axis::Y => "y",
            Axis::Z => "z",
        }
    }

    pub fn parse(name: &str) -> Option<Axis> {
        [Axis::X, Axis::Y, Axis::Z]
            .into_iter()
            .find(|a| a.name() == name)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Half {
    Bottom,
    Top,
}

impl Half {
    #[inline]
    pub fn name(self) -> &'static str {
        match self {
            Half::Bottom => "bottom",
            Half::Top => "top",
        }
    }

    pub fn parse(name: &str) -> Option<Half> {
        [Half::Bottom, Half::Top]
            .into_iter()
            .find(|h| h.name() == name)
    }
}

/// What a material is to a builder: which member of its family it is, and whether it holds up
/// whatever sits on it. `Decor` never supports (torches, ladders, plants, snow layers).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Form {
    Block,
    Log,
    Slab,
    Stairs,
    Fence,
    Door,
    Trapdoor,
    Other,
    Decor,
}

impl Form {
    pub const ALL: [Form; 9] = [
        Form::Block,
        Form::Log,
        Form::Slab,
        Form::Stairs,
        Form::Fence,
        Form::Door,
        Form::Trapdoor,
        Form::Other,
        Form::Decor,
    ];

    pub fn parse(name: &str) -> Option<Form> {
        Some(match name {
            "block" => Form::Block,
            "log" => Form::Log,
            "slab" => Form::Slab,
            "stairs" => Form::Stairs,
            "fence" => Form::Fence,
            "door" => Form::Door,
            "trapdoor" => Form::Trapdoor,
            _ => return None,
        })
    }
}

/// An interned registry key or family name: a thin handle, so a name copies, hashes and
/// compares as a pointer.
#[derive(Clone, Copy)]
pub struct Name(&'static &'static str);

static NAMES: LazyLock<Mutex<FxHashMap<&'static str, Name>>> = LazyLock::new(Default::default);

impl Name {
    pub fn new(name: &str) -> Name {
        let mut names = NAMES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(&interned) = names.get(name) {
            return interned;
        }
        let text: &'static str = Box::leak(name.into());
        let interned = Name(Box::leak(Box::new(text)));
        names.insert(text, interned);
        interned
    }

    #[inline]
    pub fn as_str(self) -> &'static str {
        self.0
    }
}

impl From<&str> for Name {
    fn from(name: &str) -> Name {
        Name::new(name)
    }
}

impl PartialEq for Name {
    #[inline]
    fn eq(&self, other: &Name) -> bool {
        std::ptr::eq(self.0, other.0)
    }
}

impl Eq for Name {}

impl Hash for Name {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        (self.0 as *const &str as usize).hash(state);
    }
}

impl PartialEq<str> for Name {
    fn eq(&self, other: &str) -> bool {
        *self.0 == other
    }
}

impl PartialEq<&str> for Name {
    fn eq(&self, other: &&str) -> bool {
        *self.0 == *other
    }
}

impl fmt::Debug for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(*self.0, f)
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

type Pairs = &'static [(Name, Name)];

/// Authored state beyond the typed properties, interned so a material stays `Copy` and small.
#[derive(Clone, Copy)]
struct Extra(Option<&'static Pairs>);

static EXTRAS: LazyLock<Mutex<FxHashMap<Pairs, &'static Pairs>>> = LazyLock::new(Default::default);

impl Extra {
    const NONE: Extra = Extra(None);

    fn pairs(self) -> Pairs {
        self.0.map_or(&[], |pairs| *pairs)
    }

    fn with(self, key: Name, value: Name) -> Extra {
        let mut pairs: Vec<(Name, Name)> = self
            .pairs()
            .iter()
            .copied()
            .filter(|(k, _)| *k != key)
            .collect();
        pairs.push((key, value));
        pairs.sort_unstable_by_key(|(k, _)| k.as_str());
        let mut extras = EXTRAS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(&interned) = extras.get(pairs.as_slice()) {
            return Extra(Some(interned));
        }
        let leaked: Pairs = Box::leak(pairs.into_boxed_slice());
        let interned: &'static Pairs = Box::leak(Box::new(leaked));
        extras.insert(leaked, interned);
        Extra(Some(interned))
    }
}

impl PartialEq for Extra {
    #[inline]
    fn eq(&self, other: &Extra) -> bool {
        match (self.0, other.0) {
            (None, None) => true,
            (Some(a), Some(b)) => std::ptr::eq(a, b),
            _ => false,
        }
    }
}

impl Eq for Extra {}

impl Hash for Extra {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.map_or(0, |p| p as *const Pairs as usize).hash(state);
    }
}

/// A block plus authored state, named by registry key so a memoized plan never carries a
/// session-local block id. `Copy`: building one allocates nothing once its names are interned.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Material {
    pub block: Name,
    pub form: Form,
    pub family: Option<Name>,
    facing: Option<Dir>,
    half: Option<Half>,
    axis: Option<Axis>,
    open: Option<bool>,
    extra: Extra,
}

impl Hash for Material {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        let address = |name: Option<Name>| name.map_or(0, |n| n.0 as *const &str as usize) as u64;
        let small = self.form as u64
            | (self.facing.map_or(0, |d| d as u64 + 1) << 8)
            | (self.half.map_or(0, |h| h as u64 + 1) << 16)
            | (self.axis.map_or(0, |a| a as u64 + 1) << 24)
            | (self.open.map_or(0, |o| o as u64 + 1) << 32);
        let extra = self.extra.0.map_or(0, |p| p as *const Pairs as usize) as u64;
        state.write_u64(address(Some(self.block)) ^ address(self.family).rotate_left(21));
        state.write_u64(small ^ extra.rotate_left(42));
    }
}

pub const AIR: &str = "petramond:air";

static AIR_NAME: LazyLock<Name> = LazyLock::new(|| Name::new(AIR));

impl Material {
    pub fn named(block: &str) -> Material {
        Material::of(Name::new(block), Form::Other)
    }

    #[inline]
    pub fn of(block: Name, form: Form) -> Material {
        Material {
            block,
            form,
            family: None,
            facing: None,
            half: None,
            axis: None,
            open: None,
            extra: Extra::NONE,
        }
    }

    /// A block that holds nothing up: a torch, ladder, plant or snow layer.
    pub fn decor(block: &str) -> Material {
        Material::of(Name::new(block), Form::Decor)
    }

    pub fn air() -> Material {
        Material::of(*AIR_NAME, Form::Decor)
    }

    /// A cheap spread of the fields that most often tell materials apart, for small caches.
    #[inline]
    pub(super) fn quick_slot(&self) -> usize {
        (self.block.0 as *const &str as usize >> 3)
            ^ (self.facing.map_or(0, |d| d as usize + 1) * 5)
            ^ (self.half.map_or(0, |h| h as usize + 1) * 11)
            ^ (self.axis.map_or(0, |a| a as usize + 1) * 3)
    }

    #[inline]
    pub fn is_air(&self) -> bool {
        self.block == *AIR_NAME
    }

    #[inline]
    pub fn in_family(mut self, family: Name) -> Material {
        self.family = Some(family);
        self
    }

    /// Any authored property; `facing`, `half`, `axis` and `open` also have typed setters.
    pub fn with(mut self, key: &str, value: &str) -> Material {
        match key {
            "facing" if Dir::parse(value).is_some() => self.facing = Dir::parse(value),
            "half" if Half::parse(value).is_some() => self.half = Half::parse(value),
            "axis" if Axis::parse(value).is_some() => self.axis = Axis::parse(value),
            "open" if matches!(value, "true" | "false") => self.open = Some(value == "true"),
            _ => self.extra = self.extra.with(Name::new(key), Name::new(value)),
        }
        self
    }

    #[inline]
    pub fn facing(mut self, dir: Dir) -> Material {
        self.facing = Some(dir);
        self
    }

    #[inline]
    pub fn half(mut self, half: Half) -> Material {
        self.half = Some(half);
        self
    }

    #[inline]
    pub fn axis(mut self, axis: Axis) -> Material {
        self.axis = Some(axis);
        self
    }

    #[inline]
    pub fn open(mut self, open: bool) -> Material {
        self.open = Some(open);
        self
    }

    #[inline]
    pub fn get_facing(&self) -> Option<Dir> {
        self.facing
    }

    #[inline]
    pub fn get_half(&self) -> Half {
        self.half.unwrap_or(Half::Bottom)
    }

    /// The authored state properties, `(key, value)`, in a structure template palette's
    /// vocabulary.
    pub fn state_pairs(&self) -> impl Iterator<Item = (&'static str, &'static str)> + '_ {
        let typed = [
            self.facing.map(|d| ("facing", d.name())),
            self.half.map(|h| ("half", h.name())),
            self.axis.map(|a| ("axis", a.name())),
            self.open
                .map(|o| ("open", if o { "true" } else { "false" })),
        ];
        typed.into_iter().flatten().chain(
            self.extra
                .pairs()
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str())),
        )
    }

    /// The authored state in a structure template palette's vocabulary.
    pub fn state(&self) -> BTreeMap<String, String> {
        self.state_pairs()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// Whether a block resting on this one sits flush on a solid top face.
    #[inline]
    pub fn solid_top(&self) -> bool {
        match self.form {
            Form::Decor | Form::Door | Form::Trapdoor => false,
            Form::Slab => self.get_half() == Half::Top,
            _ => !self.is_air(),
        }
    }

    /// Whether something built here visibly touches its neighbours: air and decor do not.
    #[inline]
    pub fn occupies(&self) -> bool {
        self.form != Form::Decor && !self.is_air()
    }
}

impl fmt::Debug for Material {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{:?}", self.block, self.state())
    }
}
