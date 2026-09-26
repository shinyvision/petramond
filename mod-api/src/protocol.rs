//! The four wire enums — [`HostCall`] (nested by domain)/[`HostRet`]
//! (guest→host request/reply)
//! and [`GuestCall`]/[`GuestRet`] (host→guest) — plus the worldgen write
//! alias. Evolution rules live in the crate docs; the recorded encoding
//! lives in `wire_pin`.

mod guest;
mod host;

#[cfg(test)]
mod tests;

pub use guest::{FeaturePlacement, GenOutput, GenWrite, GuestRet, HostCall, StructurePlacement};
pub use host::{
    calls, decode_host_call, ActorCall, BlockCall, BodyCall, ClientCall, ConditionCall,
    ConstructionCall, ContainerCall, CoreCall, EntityCall, GuestCall, GuiCall, HostRet,
    ItemMotionCall, KvCall, MemoCall, MemoClaim, PlayerCall, RegistryCall, SchematicCall,
    SoundCall, TagCall, WorldgenCall,
};
