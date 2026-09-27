use crate::mob::Mob;
use crate::player::Player;
use petramond_math::math::{IVec3, Vec3};
use petramond_math::world_pos::WorldPos;
use petramond_world::block::Block;
use petramond_world::item::{ItemStack, ItemType, ItemUse};
use petramond_world::world::{raycast, WorldData};

pub trait ActorView {
    fn held(&self) -> Option<&ItemStack>;
    fn is_spectator(&self) -> bool;
    fn eye(&self) -> WorldPos;
    fn look_dir(&self) -> Vec3;
}

impl ActorView for Player {
    fn held(&self) -> Option<&ItemStack> {
        Player::held(self)
    }

    fn is_spectator(&self) -> bool {
        Player::is_spectator(self)
    }

    fn eye(&self) -> WorldPos {
        Player::eye(self)
    }

    fn look_dir(&self) -> Vec3 {
        self.forward()
    }
}

pub fn held_item(actor: &impl ActorView) -> Option<ItemType> {
    actor.held().map(|st| st.item)
}

pub fn holds_shears(actor: &impl ActorView) -> bool {
    held_item(actor).and_then(ItemType::item_use) == Some(ItemUse::Shear)
}

pub fn can_shear_coat(kind: Mob, dead: bool, shorn: bool) -> bool {
    crate::mob::def(kind).shear.is_some() && !dead && !shorn
}

pub fn is_contextual_placeable(item: ItemType) -> bool {
    item.food().is_some() && item.as_block().is_some_and(|b| b != Block::Air)
}

pub fn eat_claims(actor: &impl ActorView) -> bool {
    held_item(actor).is_some_and(|item| item.food().is_some()) && !actor.is_spectator()
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum EngineItemUse {
    Fill {
        source: IVec3,
        becomes: ItemType,
    },
    Pour {
        cell: IVec3,
        fluid: Block,
        becomes: ItemType,
    },
}

impl EngineItemUse {
    pub fn lands(&self, world: &WorldData) -> bool {
        match *self {
            EngineItemUse::Fill { .. } => true,
            EngineItemUse::Pour { cell, .. } => pour_lands(world, cell),
        }
    }
}

pub fn resolve_engine_item_use(actor: &impl ActorView, world: &WorldData) -> Option<EngineItemUse> {
    match held_item(actor)?.item_use()? {
        ItemUse::BucketFill { fills } => {
            bucket_fill_target(world, actor.eye(), actor.look_dir(), fills)
                .map(|(source, becomes)| EngineItemUse::Fill { source, becomes })
        }
        ItemUse::BucketPour { becomes, fluid } => {
            bucket_pour_cell(world, actor.eye(), actor.look_dir()).map(|cell| EngineItemUse::Pour {
                cell,
                fluid,
                becomes,
            })
        }
        ItemUse::Shear => None,
    }
}

pub fn engine_item_use_claims(actor: &impl ActorView, world: &WorldData) -> bool {
    resolve_engine_item_use(actor, world).is_some_and(|u| u.lands(world))
}

pub fn bucket_fill_target(
    world: &WorldData,
    eye: WorldPos,
    dir: Vec3,
    fills: &[(Block, ItemType)],
) -> Option<(IVec3, ItemType)> {
    let takes = |fluid: Block| fills.iter().any(|&(b, _)| b == fluid);
    let (hit, _) = raycast::fluid_sources(eye, dir, world, takes)?;
    let scooped = Block::from_id(world.chunk_block(hit.block.x, hit.block.y, hit.block.z));
    let &(_, becomes) = fills.iter().find(|&&(b, _)| b == scooped)?;
    world
        .is_fluid_source_world(hit.block, scooped)
        .then_some((hit.block, becomes))
}

pub fn bucket_pour_cell(world: &WorldData, eye: WorldPos, dir: Vec3) -> Option<IVec3> {
    let (hit, _) = raycast::including_any_fluid(eye, dir, world)?;
    let looked_at = Block::from_id(world.chunk_block(hit.block.x, hit.block.y, hit.block.z));
    if crate::world::placement::replaces_in_place(looked_at) {
        Some(hit.block)
    } else if hit.normal == IVec3::ZERO {
        None
    } else {
        Some(hit.block + hit.normal)
    }
}

pub fn pour_lands(world: &WorldData, cell: IVec3) -> bool {
    Block::from_id(world.chunk_block(cell.x, cell.y, cell.z)).is_replaceable()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{ReplicaWorld, ServerWorld};
    use petramond_world::chunk::{
        Chunk, ChunkPos, SectionPos, CHUNK_SX, CHUNK_SZ, SECTION_MAX_CY, SECTION_MIN_CY,
    };

    struct FakeActor {
        held: Option<ItemStack>,
        spectator: bool,
        eye: WorldPos,
        dir: Vec3,
    }

    impl ActorView for FakeActor {
        fn held(&self) -> Option<&ItemStack> {
            self.held.as_ref()
        }
        fn is_spectator(&self) -> bool {
            self.spectator
        }
        fn eye(&self) -> WorldPos {
            self.eye
        }
        fn look_dir(&self) -> Vec3 {
            self.dir
        }
    }

    fn looking_down_at(item: ItemType, cell: IVec3) -> FakeActor {
        FakeActor {
            held: Some(ItemStack::new(item, 1)),
            spectator: false,
            eye: WorldPos::new(
                f64::from(cell.x) + 0.5,
                f64::from(cell.y) + 3.0,
                f64::from(cell.z) + 0.5,
            ),
            dir: Vec3::new(0.0, -1.0, 0.0),
        }
    }

    /// An authoritative world from `build` (3x3 chunks, stone floor at y=64), plus a client
    /// replica installed through the real wire payloads. Each mirror runs the rules on one.
    fn server_and_replica(build: impl FnOnce(&mut ServerWorld)) -> (ServerWorld, ReplicaWorld) {
        let pool = std::sync::Arc::new(crate::worker::JobPool::new(1));
        let mut server = ServerWorld::with_pool(0, 1, pool.clone());
        let columns: Vec<ChunkPos> = (-1..=1)
            .flat_map(|cz| (-1..=1).map(move |cx| ChunkPos::new(cx, cz)))
            .collect();
        for &cp in &columns {
            let mut c = Chunk::new(cp.cx, cp.cz);
            for z in 0..CHUNK_SZ {
                for x in 0..CHUNK_SX {
                    c.set_block(x, 64, z, Block::Stone);
                }
            }
            server.insert_chunk_for_test(cp, c);
        }
        build(&mut server);
        let mut replica = ReplicaWorld::with_pool(0, 1, pool);
        for &cp in &columns {
            replica.install_remote_column(server.column_payload(cp).expect("a loaded column"));
            for cy in SECTION_MIN_CY..=SECTION_MAX_CY {
                if let Some(p) = server.section_payload(SectionPos::new(cp.cx, cy, cp.cz)) {
                    replica.install_remote_section(p);
                }
            }
        }
        (server, replica)
    }

    fn run_ticks(w: &mut ServerWorld, n: u32) {
        let recipes = petramond_world::crafting::Recipes::default();
        for _ in 0..n {
            w.game_tick(&recipes);
        }
    }

    #[test]
    fn a_fill_resolves_the_same_source_on_the_server_and_its_replica() {
        let src = IVec3::new(8, 65, 8);
        let (server, replica) = server_and_replica(|w| {
            assert!(w.set_block_world(src.x, src.y, src.z, Block::Water));
        });
        let actor = looking_down_at(ItemType::WoodenBucket, src);
        let on_server = resolve_engine_item_use(&actor, server.data());
        assert_eq!(
            on_server,
            Some(EngineItemUse::Fill {
                source: src,
                becomes: ItemType::WaterBucket
            })
        );
        assert_eq!(resolve_engine_item_use(&actor, replica.data()), on_server);
        assert!(engine_item_use_claims(&actor, server.data()));
        assert!(engine_item_use_claims(&actor, replica.data()));
    }

    #[test]
    fn flowing_water_claims_no_fill_on_either_mirror() {
        let src = IVec3::new(8, 65, 8);
        let flow = src + IVec3::X;
        let (server, replica) = server_and_replica(|w| {
            assert!(w.set_block_world(src.x, src.y, src.z, Block::Water));
            run_ticks(w, 30);
            assert!(!w.is_water_source_world(flow), "the ring cell is flow");
        });
        let actor = looking_down_at(ItemType::WoodenBucket, flow);
        assert_eq!(resolve_engine_item_use(&actor, server.data()), None);
        assert_eq!(resolve_engine_item_use(&actor, replica.data()), None);
    }

    #[test]
    fn a_pour_resolves_and_lands_identically_on_both_mirrors() {
        let floor = IVec3::new(4, 64, 4);
        let (server, replica) = server_and_replica(|_| {});
        let actor = looking_down_at(ItemType::WaterBucket, floor);
        let on_server = resolve_engine_item_use(&actor, server.data());
        assert!(
            matches!(on_server, Some(EngineItemUse::Pour { cell, fluid: Block::Water, .. }) if cell == floor + IVec3::Y),
            "stone is poured against: {on_server:?}"
        );
        assert_eq!(resolve_engine_item_use(&actor, replica.data()), on_server);
        assert!(engine_item_use_claims(&actor, server.data()));
        assert!(engine_item_use_claims(&actor, replica.data()));
    }

    #[test]
    fn a_pour_with_nothing_in_reach_resolves_nothing() {
        let (server, replica) = server_and_replica(|_| {});
        let mut actor = looking_down_at(ItemType::WaterBucket, IVec3::new(4, 64, 4));
        actor.dir = Vec3::new(0.0, 1.0, 0.0);
        assert_eq!(resolve_engine_item_use(&actor, server.data()), None);
        assert_eq!(resolve_engine_item_use(&actor, replica.data()), None);
    }

    #[test]
    fn shears_resolve_no_block_use() {
        let (server, _) = server_and_replica(|_| {});
        let actor = looking_down_at(ItemType::Shears, IVec3::new(4, 64, 4));
        assert!(holds_shears(&actor));
        assert_eq!(resolve_engine_item_use(&actor, server.data()), None);
    }

    #[test]
    fn eating_needs_food_and_a_body_in_play() {
        let food = ItemType::all()
            .iter()
            .copied()
            .find(|i| i.food().is_some())
            .expect("a registered food");
        let mut actor = looking_down_at(food, IVec3::ZERO);
        assert!(eat_claims(&actor));
        actor.spectator = true;
        assert!(!eat_claims(&actor), "a spectator eats nothing");
        actor.spectator = false;
        actor.held = Some(ItemStack::new(ItemType::Dirt, 1));
        assert!(!eat_claims(&actor));
        actor.held = None;
        assert!(!eat_claims(&actor));
    }

    #[test]
    fn a_coat_is_shearable_only_on_a_live_unshorn_shearable_species() {
        assert!(can_shear_coat(Mob::Sheep, false, false));
        assert!(!can_shear_coat(Mob::Sheep, false, true), "already shorn");
        assert!(!can_shear_coat(Mob::Sheep, true, false), "a ragdoll");
        let bare = crate::mob::Mob::all()
            .iter()
            .copied()
            .find(|&m| crate::mob::def(m).shear.is_none());
        if let Some(bare) = bare {
            assert!(!can_shear_coat(bare, false, false));
        }
    }
}
