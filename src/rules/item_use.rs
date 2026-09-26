//! The held item's use rules — the shear, eat and bucket gates — written
//! once, over an [`ActorView`] and a `World`.
//!
//! The server runs them against its authoritative world and the acting
//! session's player, then executes the verdict (the swap, the scoop, the
//! pour write); the client runs them against its replica (a `World` too)
//! and the predicted local body, and keeps only the verdict. A new
//! [`ItemUse`] kind is one arm in [`resolve_engine_item_use`] that both
//! mirrors pick up — never a server consumer plus a client copy.

use crate::mob::Mob;
use crate::player::Player;
use petramond_world::world::{raycast, WorldData};
use petramond_math::math::{IVec3, Vec3};
use petramond_math::world_pos::WorldPos;
use petramond_world::block::Block;
use petramond_world::item::{ItemStack, ItemType, ItemUse};

/// The acting body as the item-use rules read it. The server answers from
/// the session's [`Player`]; the client from its predicted local body plus
/// the replicated self view (whose inventory is the one the server
/// confirms).
pub trait ActorView {
    /// The ACTING hand's stack — the main hand, or the off hand during the
    /// ladder's second pass.
    fn held(&self) -> Option<&ItemStack>;
    /// Whether the body is a spectator (it can use nothing).
    fn is_spectator(&self) -> bool;
    /// Where the item-use rays start.
    fn eye(&self) -> WorldPos;
    /// Which way the item-use rays go.
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

/// The acting hand's item.
pub fn held_item(actor: &impl ActorView) -> Option<ItemType> {
    actor.held().map(|st| st.item)
}

/// Whether the acting hand holds shears — the first half of the shear
/// consumer's gate.
pub fn holds_shears(actor: &impl ActorView) -> bool {
    held_item(actor).and_then(ItemType::item_use) == Some(ItemUse::Shear)
}

/// Whether a mob of `kind` still has a coat the shears can take: a species
/// with a shear row, alive, and not already shorn (the regrow countdown is
/// what `shorn` replicates).
pub fn can_shear_coat(kind: Mob, dead: bool, shorn: bool) -> bool {
    crate::mob::def(kind).shear.is_some() && !dead && !shorn
}

/// Whether an item is BOTH food and placeable (a plantable carrot) — the
/// dual nature the contextual-place / ordinary-place consumer pair splits on.
pub fn is_contextual_placeable(item: ItemType) -> bool {
    item.food().is_some() && item.as_block().is_some_and(|b| b != Block::Air)
}

/// Whether the acting hand's click belongs to the eat consumer: food in hand
/// and a body that can use. Whether the eat then starts, is already running,
/// or is cancelled by a mod, the click is consumed.
pub fn eat_claims(actor: &impl ActorView) -> bool {
    held_item(actor).is_some_and(|item| item.food().is_some()) && !actor.is_spectator()
}

/// The engine's own use of the held item, resolved against the world: what
/// the click would act on. `None` = the engine use has nothing to act on.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum EngineItemUse {
    /// Scoop the fluid source at `source`; the held item becomes `becomes`.
    Fill { source: IVec3, becomes: ItemType },
    /// Pour `fluid` into `cell`; the held item becomes `becomes`. Whether it
    /// lands is [`EngineItemUse::lands`] — asked after the pour's
    /// `block_place_pre`, exactly where the server asks it.
    Pour {
        cell: IVec3,
        fluid: Block,
        becomes: ItemType,
    },
}

impl EngineItemUse {
    /// Whether the resolved use goes through in `world`: a fill always does
    /// (its target rule already found the source); a pour only into a
    /// replaceable cell.
    pub fn lands(&self, world: &WorldData) -> bool {
        match *self {
            EngineItemUse::Fill { .. } => true,
            EngineItemUse::Pour { cell, .. } => pour_lands(world, cell),
        }
    }
}

/// Resolve the acting hand's data-declared engine use (`"use"` in
/// items.json) against `world`. Shears act only through a mob target (the
/// shear consumer) and resolve nothing here; mod items react through the
/// `item_use_pre` event instead.
pub fn resolve_engine_item_use(actor: &impl ActorView, world: &WorldData) -> Option<EngineItemUse> {
    match held_item(actor)?.item_use()? {
        ItemUse::BucketFill { fills } => bucket_fill_target(world, actor.eye(), actor.look_dir(), fills)
            .map(|(source, becomes)| EngineItemUse::Fill { source, becomes }),
        ItemUse::BucketPour { becomes, fluid } => bucket_pour_cell(world, actor.eye(), actor.look_dir())
            .map(|cell| EngineItemUse::Pour {
                cell,
                fluid,
                becomes,
            }),
        ItemUse::Shear => None,
    }
}

/// Whether the engine use claims the click — resolved AND landing. The
/// client's whole prediction of the rung; the server's execution reaches the
/// same answer one step at a time.
pub fn engine_item_use_claims(actor: &impl ActorView, world: &WorldData) -> bool {
    resolve_engine_item_use(actor, world).is_some_and(|u| u.lands(world))
}

/// The bucket FILL target rule: the source-stopping ray hits, within reach, a
/// still SOURCE of a fluid the bucket has a result for. Answers the scooped
/// cell and the item the bucket becomes. Flowing fluid (and fluid the bucket
/// does not take) is transparent to the ray, so a spread sheet or thin film —
/// which can render exactly like still water — never shadows the source the
/// player is aiming at, and aiming at pure flow does nothing.
pub fn bucket_fill_target(
    world: &WorldData,
    eye: WorldPos,
    dir: Vec3,
    fills: &[(Block, ItemType)],
) -> Option<(IVec3, ItemType)> {
    let takes = |fluid: Block| fills.iter().any(|&(b, _)| b == fluid);
    let (hit, _) = raycast::fluid_sources(eye, dir, world, takes)?;
    let scooped = Block::from_id(world.chunk_block(hit.block.x, hit.block.y, hit.block.z));
    // A solid hit is simply nothing to scoop.
    let &(_, becomes) = fills.iter().find(|&&(b, _)| b == scooped)?;
    world
        .is_fluid_source_world(hit.block, scooped)
        .then_some((hit.block, becomes))
}

/// The bucket POUR cell rule: the any-fluid-stopping ray hits something
/// within reach; a replaceable hit (every fluid, grass, a fern) is poured in
/// place, anything else against the clicked face. `None` = nothing in reach,
/// or the eye inside the hit cell (no face to pour against).
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

/// Whether a pour into `cell` lands: the cell must be replaceable. Pouring
/// onto a source of the same fluid still lands (a no-op write that empties
/// the bucket), so on fluid the pour is always predictable.
pub fn pour_lands(world: &WorldData, cell: IVec3) -> bool {
    Block::from_id(world.chunk_block(cell.x, cell.y, cell.z)).is_replaceable()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::WorldRole;
    use petramond_world::chunk::{
        Chunk, ChunkPos, SectionPos, CHUNK_SX, CHUNK_SZ, SECTION_MAX_CY, SECTION_MIN_CY,
    };

    /// A fake acting body: exactly the four facts the rules read.
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

    /// Holding `item`, eye two cells above the top face of `cell`, looking
    /// straight down.
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

    /// An authoritative world (3×3 loaded chunks, stone floor at y=64) built
    /// by `build`, plus a CLIENT REPLICA of it installed through the real wire
    /// payloads — the two worlds the two mirrors evaluate the rules against.
    fn server_and_replica(build: impl FnOnce(&mut World)) -> (World, World) {
        let pool = std::sync::Arc::new(crate::worker::JobPool::new(1));
        let mut server = World::new_with_pool(0, 1, WorldRole::Combined, pool.clone());
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
        let mut replica = World::new_with_pool(0, 1, WorldRole::ClientReplica, pool);
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

    fn run_ticks(w: &mut World, n: u32) {
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
        let on_server = resolve_engine_item_use(&actor, &server);
        assert_eq!(
            on_server,
            Some(EngineItemUse::Fill {
                source: src,
                becomes: ItemType::WaterBucket
            })
        );
        assert_eq!(resolve_engine_item_use(&actor, &replica), on_server);
        assert!(engine_item_use_claims(&actor, &server));
        assert!(engine_item_use_claims(&actor, &replica));
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
        assert_eq!(resolve_engine_item_use(&actor, &server), None);
        assert_eq!(resolve_engine_item_use(&actor, &replica), None);
    }

    #[test]
    fn a_pour_resolves_and_lands_identically_on_both_mirrors() {
        let floor = IVec3::new(4, 64, 4);
        let (server, replica) = server_and_replica(|_| {});
        let actor = looking_down_at(ItemType::WaterBucket, floor);
        let on_server = resolve_engine_item_use(&actor, &server);
        assert!(
            matches!(on_server, Some(EngineItemUse::Pour { cell, fluid: Block::Water, .. }) if cell == floor + IVec3::Y),
            "stone is poured against: {on_server:?}"
        );
        assert_eq!(resolve_engine_item_use(&actor, &replica), on_server);
        assert!(engine_item_use_claims(&actor, &server));
        assert!(engine_item_use_claims(&actor, &replica));
    }

    #[test]
    fn a_pour_with_nothing_in_reach_resolves_nothing() {
        let (server, replica) = server_and_replica(|_| {});
        let mut actor = looking_down_at(ItemType::WaterBucket, IVec3::new(4, 64, 4));
        actor.dir = Vec3::new(0.0, 1.0, 0.0);
        assert_eq!(resolve_engine_item_use(&actor, &server), None);
        assert_eq!(resolve_engine_item_use(&actor, &replica), None);
    }

    #[test]
    fn shears_resolve_no_block_use() {
        let (server, _) = server_and_replica(|_| {});
        let actor = looking_down_at(ItemType::Shears, IVec3::new(4, 64, 4));
        assert!(holds_shears(&actor));
        assert_eq!(resolve_engine_item_use(&actor, &server), None);
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
