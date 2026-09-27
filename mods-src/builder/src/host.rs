//! Every call the builder makes into the world, behind one trait.
//!
//! The mod reads and moves the world only through the free functions here.
//! In the wasm guest they go straight to mod-sdk ([`Sdk`]); a native test
//! installs a [`Host`] of its own on its thread (the in-crate fake world in
//! `fake`) and the same code runs against it. The functions keep mod-sdk's
//! names and, where the builder uses all of an answer, its signatures; every
//! module takes them in through [`prelude`] in place of `mod_sdk::*`.
//!
//! Registration (`lib.rs`), diagnostics (`log`) and the SDK's own record
//! stores, change cursor and panel publisher stay on mod-sdk: they are the
//! mod's frame, not its gameplay. The test host answers the few host calls
//! the record stores and the cursor make on its own.

use mod_sdk::*;

macro_rules! sdk_body {
    ($name:ident($($arg:ident),*)) => {
        mod_sdk::$name($($arg),*)
    };
    ($name:ident($($arg:ident),*) $body:expr) => {
        $body
    };
}

macro_rules! host_calls {
    ($(
        $(#[$doc:meta])*
        fn $name:ident($($arg:ident: $ty:ty),* $(,)?) $(-> $ret:ty)? $(=> $body:expr)?;
    )*) => {
        pub trait Host {
            $(
                $(#[$doc])*
                #[allow(clippy::too_many_arguments)]
                fn $name(&self, $($arg: $ty),*) $(-> $ret)?;
            )*
        }

        impl Host for Sdk {
            $(
                fn $name(&self, $($arg: $ty),*) $(-> $ret)? {
                    sdk_body!($name($($arg),*) $($body)?)
                }
            )*
        }

        $(
            $(#[$doc])*
            pub fn $name($($arg: $ty),*) $(-> $ret)? {
                with(|host| host.$name($($arg),*))
            }
        )*

        pub mod prelude {
            pub use mod_sdk::*;
            pub use super::{$($name),*};
        }
    };
}

pub struct Sdk;

host_calls! {

    fn get_block(pos: [i32; 3]) -> Option<BlockId>;
    fn get_blocks(positions: Vec<[i32; 3]>) -> Vec<Option<BlockId>>;
    fn is_loaded(pos: [i32; 3]) -> bool;
    fn find_blocks(min: [i32; 3], max: [i32; 3], blocks: Vec<BlockId>) -> Option<Vec<[i32; 3]>>;
    fn block_model_group(pos: [i32; 3]) -> Option<ModelGroupData>;
    fn set_model_parts(pos: [i32; 3], parts: u32, tint: Option<[u8; 3]>) -> bool;

    fn resolve_block(name: &str) -> Option<BlockId>;
    fn resolve_item(name: &str) -> Option<ItemId>;
    fn resolve_mob(key: &str) -> Option<MobId>;
    fn block_info(block: BlockId) -> Option<BlockInfoData>
        => mod_sdk::block_info(block).map(|info| *info);
    fn block_infos(blocks: Vec<BlockId>) -> Vec<Option<BlockInfoData>>;
    fn block_names(blocks: Vec<BlockId>) -> Vec<Option<String>>;
    fn item_info(item: &str) -> Option<ItemInfoData>
        => mod_sdk::item_info(item).map(|info| *info);
    fn item_names(items: Vec<ItemId>) -> Vec<Option<String>>;
    fn blocks_by_tag(tag: &str) -> Vec<BlockId>;
    fn blocks_with_data(key: &str) -> Vec<(BlockId, String)>;
    fn block_record_plans(records: Vec<BlockRecord>) -> Vec<RecordPlan>;

    fn block_record_statuses(cells: Vec<([i32; 3], BlockRecord)>) -> Vec<RecordStatus>;
    fn actor_aims(
        actor: EntityRef,
        from: Vec<[f64; 3]>,
        pos: [i32; 3],
        record: Option<BlockRecord>,
    ) -> Vec<Result<[f64; 3], ActionRefusal>>;
    fn actor_place_check(
        actor: EntityRef,
        from: [f64; 3],
        pos: [i32; 3],
        record: BlockRecord,
        pay: bool,
    ) -> PlaceRequest;
    fn actor_place(actor: EntityRef, pos: [i32; 3], record: BlockRecord, pay: bool) -> PlaceRequest;
    fn actor_dig(actor: EntityRef, pos: [i32; 3], tool_slot: Option<u32>, collect: bool) -> DigProgress;
    fn actor_interact(actor: EntityRef, pos: [i32; 3]) -> bool;

    fn footholds(key: &str, cells: Vec<[i32; 3]>) -> Vec<bool>;
    fn path_probe(
        key: &str,
        from: [i32; 3],
        to: [i32; 3],
        blocked: Vec<[i32; 3]>,
        max_nodes: u32,
    ) -> Option<Route>;
    fn walk_region(
        key: &str,
        from: [i32; 3],
        min: [i32; 3],
        max: [i32; 3],
        blocked: Vec<[i32; 3]>,
        toward: bool,
        max_nodes: u32,
    ) -> Flood;
    fn mob_can_reach(mob_id: u64, cell: [i32; 3]) -> bool;

    fn spawn_mob(key: &str, pos: [f64; 3], yaw: f32) -> Option<u64>;
    fn despawn_mob(mob_id: u64) -> bool;
    fn mob_info(mob_id: u64) -> Option<MobSnapshot>;
    fn mobs_with_tag(key: &str, value: Option<MobTagValue>) -> Vec<MobSnapshot>;
    fn mob_tag_get(mob_id: u64, key: &str) -> MobTagLookup;
    fn mob_tag_set(mob_id: u64, key: &str, value: MobTagValue) -> bool;
    fn mob_tag_delete(mob_id: u64, key: &str) -> bool;
    fn mob_kinematic(mob_id: u64, pos: [f64; 3], yaw: f32, pitch: f32, roll: f32) -> bool;
    fn mob_drive(mob_id: u64, vel: [f32; 2], yaw: Option<f32>) -> bool;
    fn mob_drive_velocity(mob_id: u64, vel: [f32; 3], yaw: Option<f32>) -> bool;
    fn mob_step(mob_id: u64, vel: [f32; 2]) -> bool;
    fn mob_anim_set(mob_id: u64, anim: &str, active: bool) -> bool;
    fn mob_held_display(mob_id: u64, main: Option<String>, off: Option<String>) -> bool;
    fn set_mob_draw(mob_id: u64, frame: DrawFrame, prims: Vec<DrawPrim>) -> bool;

    fn container_get(at: ContainerAddress) -> Option<Vec<Option<ItemStackData>>>;
    fn container_get_many(
        addresses: Vec<ContainerAddress>,
    ) -> Vec<Option<Vec<Option<ItemStackData>>>>;
    fn container_set(at: ContainerAddress, slots: Vec<(u32, Option<ItemStackData>)>) -> bool;
    fn container_transfer(
        from: ContainerAddress,
        slot: u32,
        to: ContainerAddress,
        count: u8,
    ) -> Option<ItemStackData>;
    fn container_hold(at: ContainerAddress, actor: EntityRef, open: bool) -> bool;

    fn schematic_info(asset: SchematicId) -> SchematicLookup;
    fn schematic_cells(asset: SchematicId, section: u32, turns: u8) -> Option<SchematicCellsData>;
    fn schematic_ghost_set(key: &str, ghost: Option<SchematicGhostData>) -> bool;
    fn schematic_choose(player: PlayerId, tag: &str) -> bool;
    fn schematic_position(
        player: PlayerId,
        tag: &str,
        asset: SchematicId,
        origin: Option<[i32; 3]>,
        turns: u8,
    ) -> bool;

    fn players() -> Vec<(PlayerId, [f64; 3])>
        => mod_sdk::players().into_iter().map(|row| (row.id, row.state.pos)).collect();
    fn acting_player() -> Option<PlayerId> => mod_sdk::player_state().id;
    fn player_held(player: PlayerId) -> Option<ItemStackData>;
    fn player_identity(player: PlayerId) -> Option<PlayerIdentityData>;
    fn gui_viewers() -> Vec<GuiViewerData>;
    fn gui_open(kind_key: &str, at: Option<ContainerAddress>) -> bool;

    fn emitter_burst_of(
        key: &str,
        pos: [f64; 3],
        intensity: f32,
        direction: Option<[f32; 3]>,
        texture: Option<ParticleTexture>,
    ) -> bool;
    fn sound_play_at(key: &str, pos: [f64; 3], volume: f32, pitch: f32) -> u64;
    fn spawn_item_data(item: &str, count: u8, pos: [f64; 3], data: &[(&str, &[u8])]) -> bool;

    fn current_tick() -> u64;
    fn rng_u64(stream_key: &str) -> u64;
    fn world_kv_get(key: &str) -> Option<Vec<u8>>;
    fn world_kv_set(key: &str, value: Vec<u8>);
}

pub fn logged<T>(kind: &str, name: &str, found: Option<T>) -> Option<T> {
    if found.is_none() {
        log(&format!("{kind} '{name}' is not registered"));
    }
    found
}

#[cfg(not(test))]
fn with<R>(call: impl FnOnce(&dyn Host) -> R) -> R {
    call(&Sdk)
}

#[cfg(test)]
fn with<R>(call: impl FnOnce(&dyn Host) -> R) -> R {
    match installed::current() {
        Some(host) => call(&*host),
        None => call(&Sdk),
    }
}

#[cfg(test)]
pub mod installed {

    use std::cell::RefCell;
    use std::rc::Rc;

    use super::Host;

    thread_local! {
        static HOST: RefCell<Option<Rc<dyn Host>>> = const { RefCell::new(None) };
    }

    #[must_use = "the host is uninstalled when the guard drops"]
    pub fn install(host: Rc<dyn Host>) -> Installed {
        HOST.with(|slot| *slot.borrow_mut() = Some(host));
        Installed(())
    }

    pub fn current() -> Option<Rc<dyn Host>> {
        HOST.with(|slot| slot.borrow().clone())
    }

    pub struct Installed(());

    impl Drop for Installed {
        fn drop(&mut self) {
            HOST.with(|slot| *slot.borrow_mut() = None);
        }
    }
}

#[cfg(test)]
pub mod fake;
