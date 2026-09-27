use serde::{Deserialize, Serialize};

use crate::data::RuntimeSide;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Sides(u8);

impl Sides {
    pub const SERVER: Self = Self(1 << 0);
    pub const WORLDGEN: Self = Self(1 << 1);
    pub const CLIENT: Self = Self(1 << 2);
    pub const SHELL: Self = Self(1 << 3);
    pub const SERVER_WORLDGEN: Self = Self::SERVER.union(Self::WORLDGEN);
    pub const SERVER_CLIENT: Self = Self::SERVER.union(Self::CLIENT);
    pub const CLIENT_SHELL: Self = Self::CLIENT.union(Self::SHELL);
    pub const BESIDE_WORLD: Self = Self::SERVER_WORLDGEN.union(Self::CLIENT);
    pub const EVERY: Self = Self::BESIDE_WORLD.union(Self::SHELL);

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn of(side: RuntimeSide) -> Self {
        match side {
            RuntimeSide::Server => Self::SERVER,
            RuntimeSide::Worldgen => Self::WORLDGEN,
            RuntimeSide::Client => Self::CLIENT,
        }
    }

    pub const fn allows(self, side: RuntimeSide) -> bool {
        self.contains(Self::of(side))
    }

    pub const fn admits(self, side: RuntimeSide, shell: bool) -> bool {
        match side {
            RuntimeSide::Client if shell => self.contains(Self::SHELL),
            _ => self.allows(side),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scope {
    Any,
    Init,
    Sim,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Access {
    Read,
    Write,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Legality {
    pub sides: Sides,
    pub scope: Scope,
    pub access: Access,
}

impl Legality {
    pub const fn new(sides: Sides, scope: Scope, access: Access) -> Self {
        Self {
            sides,
            scope,
            access,
        }
    }

    pub const fn mutates(self) -> bool {
        matches!(self.access, Access::Write)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallInfo {
    pub name: &'static str,
    pub legality: Legality,
}

pub(crate) mod prelude {
    pub(crate) use super::Access::{Read, Write};
    pub(crate) use super::Scope::{Any, Init, Sim};
    pub(crate) use super::{Legality, Sides};

    pub(crate) const SERVER: Sides = Sides::SERVER;
    pub(crate) const CLIENT: Sides = Sides::CLIENT;
    pub(crate) const SERVER_WORLDGEN: Sides = Sides::SERVER_WORLDGEN;
    pub(crate) const SERVER_CLIENT: Sides = Sides::SERVER_CLIENT;
    pub(crate) const CLIENT_SHELL: Sides = Sides::CLIENT_SHELL;
    pub(crate) const BESIDE_WORLD: Sides = Sides::BESIDE_WORLD;
    pub(crate) const EVERY: Sides = Sides::EVERY;

    pub(crate) const fn legal(
        sides: Sides,
        scope: super::Scope,
        access: super::Access,
    ) -> Legality {
        Legality::new(sides, scope, access)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sides_admit_exactly_their_members() {
        assert!(Sides::SERVER.allows(RuntimeSide::Server));
        assert!(!Sides::SERVER.allows(RuntimeSide::Client));
        assert!(!Sides::SERVER.allows(RuntimeSide::Worldgen));
        assert!(Sides::SERVER_CLIENT.allows(RuntimeSide::Client));
        assert!(!Sides::SERVER_CLIENT.allows(RuntimeSide::Worldgen));
        for side in [
            RuntimeSide::Server,
            RuntimeSide::Worldgen,
            RuntimeSide::Client,
        ] {
            assert!(Sides::EVERY.allows(side));
            assert!(Sides::of(side).allows(side));
        }
    }

    #[test]
    fn the_shell_admits_only_shell_calls() {
        assert!(!Sides::CLIENT.admits(RuntimeSide::Client, true));
        assert!(Sides::CLIENT.admits(RuntimeSide::Client, false));
        assert!(Sides::CLIENT_SHELL.admits(RuntimeSide::Client, true));
        assert!(!Sides::BESIDE_WORLD.admits(RuntimeSide::Client, true));
        assert!(Sides::EVERY.admits(RuntimeSide::Client, true));
        assert!(Sides::SERVER.admits(RuntimeSide::Server, true));
    }

    #[test]
    fn only_writes_mutate() {
        let read = Legality::new(Sides::EVERY, Scope::Any, Access::Read);
        let write = Legality::new(Sides::SERVER, Scope::Sim, Access::Write);
        assert!(!read.mutates());
        assert!(write.mutates());
    }
}
