//! Per-call legality: WHERE a host call may be made, declared once next to
//! the call.
//!
//! Every variant of every domain call enum carries a [`Legality`] in the
//! table its declaration generates ([`HostCall::legality`],
//! [`HostCall::name`], and each domain's `CALLS` list). The host derives its
//! gates from that one table — which instance sides a call reaches, whether
//! it is confined to the `mod_init` registration window, whether it mutates
//! (the read-only dispatch gate) — so there is no second list anywhere that
//! has to agree with the declaration.
//!
//! [`HostCall::legality`]: crate::HostCall::legality
//! [`HostCall::name`]: crate::HostCall::name

use serde::{Deserialize, Serialize};

use crate::data::RuntimeSide;

/// A set of instance sides ([`RuntimeSide`]) a call is legal on.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Sides(u8);

impl Sides {
    /// The authoritative simulation instance.
    pub const SERVER: Self = Self(1 << 0);
    /// The detached per-thread worldgen instances: no simulation context,
    /// replies must be pure functions of their inputs and the world seed.
    pub const WORLDGEN: Self = Self(1 << 1);
    /// A presentation-only `client_wasm` instance beside a world.
    pub const CLIENT: Self = Self(1 << 2);
    /// A `client_wasm` instance on the shell, with no world beside it
    /// ([`ClientContext::Shell`](crate::ClientContext::Shell)): its registries,
    /// its own UI, images, storage and files, media, and opening a presentation.
    pub const SHELL: Self = Self(1 << 3);
    pub const SERVER_WORLDGEN: Self = Self::SERVER.union(Self::WORLDGEN);
    pub const SERVER_CLIENT: Self = Self::SERVER.union(Self::CLIENT);
    /// A client instance wherever it runs: beside a world or on the shell.
    pub const CLIENT_SHELL: Self = Self::CLIENT.union(Self::SHELL);
    /// Every instance that runs beside a world.
    pub const BESIDE_WORLD: Self = Self::SERVER_WORLDGEN.union(Self::CLIENT);
    /// Every instance side, the shell included.
    pub const EVERY: Self = Self::BESIDE_WORLD.union(Self::SHELL);

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// The single side an instance runs on.
    pub const fn of(side: RuntimeSide) -> Self {
        match side {
            RuntimeSide::Server => Self::SERVER,
            RuntimeSide::Worldgen => Self::WORLDGEN,
            RuntimeSide::Client => Self::CLIENT,
        }
    }

    /// Whether an instance on `side` may make the call.
    pub const fn allows(self, side: RuntimeSide) -> bool {
        self.contains(Self::of(side))
    }

    /// Whether an instance on `side` may make the call, where `shell` says a
    /// client instance runs on the shell with no world: there only the
    /// [`SHELL`](Self::SHELL) side admits.
    pub const fn admits(self, side: RuntimeSide, shell: bool) -> bool {
        match side {
            RuntimeSide::Client if shell => self.contains(Self::SHELL),
            _ => self.allows(side),
        }
    }
}

/// When during an instance's life a call is legal.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scope {
    /// Any dispatch, `mod_init` included: the call reads only the mod's own
    /// store, the process-wide registries, or pure functions of the world
    /// seed.
    Any,
    /// Only inside `mod_init` (the registration window).
    Init,
    /// Needs a live dispatch context: the simulation on a server instance
    /// (`mod_init`, tick systems, event handlers, hooks), the prediction
    /// scope on a client instance. Outside one the call is refused with
    /// [`ErrorCode::NoContext`](crate::ErrorCode::NoContext).
    Sim,
}

/// Whether a call changes state another dispatch can observe.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Access {
    /// Changes nothing another dispatch can observe: queries, the mod's own
    /// RNG stream, logging.
    Read,
    /// Mutates the world, a player, a registration, shared memo state, or
    /// presentation state others see. Refused inside a read-only dispatch
    /// (a shape placement plan).
    Write,
}

/// One call's legality: the table row the host's gates read.
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

/// A call's name and legality, as its domain's declaration lists it: the
/// data a host gate, a diagnostic or an audit tool reads without matching on
/// the call enums.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallInfo {
    pub name: &'static str,
    pub legality: Legality,
}

/// The vocabulary the domain declarations spell their legality column in.
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
