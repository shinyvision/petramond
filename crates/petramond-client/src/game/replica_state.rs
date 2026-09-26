//! Everything the server told this client, in one owned subsystem: the
//! replica world, the replicated entity stores, the self and menu views, the
//! join-time catalogs and the per-batch inboxes the frame drains.
//!
//! The replica is written only by the message drain (`Game::apply_*`) and the
//! prediction ledger's optimistic writes; every other client system reads it
//! through a shared borrow handed out by `Game`.

use petramond::mob;
use petramond::net::protocol::{ChatLine, PlayerMount};
use petramond::player;
use petramond::world::ReplicaWorld;
use petramond_math::math::{IVec3, Vec3};
use petramond_math::world_pos::WorldPos;
use petramond_world::chunk::SectionPos;
use petramond_world::collision::DynBox;
use petramond_world::crafting::CraftingCatalog;
use petramond_worldgen::density::surface::SurfaceDensitySystem;

use super::replicated::{EntityReplica, MenuView, MountPose, SelfView};
use super::section_cache::SectionCache;
use super::tick::ClientEvents;

pub(super) struct ReplicaState {
    /// The client's REPLICA world (role `ClientReplica`): installed from the
    /// server's terrain payloads + deltas, it owns light + meshes for the
    /// renderer and answers every client-side world read — collision,
    /// raycast, particles, door/chest presentation, environment sampling.
    pub(super) world: ReplicaWorld,
    /// The replicated ENTITY state: mob / item / remote-player stores, the
    /// own mount, the staged interpolation window over them, and the
    /// per-batch session facts (tick, sleep headcount, roster, own id).
    pub(super) entities: EntityReplica,
    /// The client-side mirror of the local player's replicated `SelfState`:
    /// the HUD/hand/overlay read model (health, effects, inventory, mining,
    /// eating, sleeping).
    pub(super) self_view: SelfView,
    /// The client's replicated MENU-session view (`MenuSyncMsg`, on-change),
    /// including disposable P1 menu predictions until their outcomes arrive:
    /// the exclusive source `menu_read_model` renders container screens from.
    pub(super) menu_view: MenuView,
    /// Immutable enabled player-crafting catalog agreed at join. Remote
    /// clients use the server's name-addressed rows, never local pack files.
    pub(super) crafting: CraftingCatalog,
    /// Seed-derived surface climate, sampled where the replica has no column
    /// yet (environment tint and fog before the payloads land).
    pub(super) fallback_world: SurfaceDensitySystem,
    /// This frame's replicated tick events (world-anchored + self one-shots +
    /// sound queues), buffered by `apply_tick_update` and drained once per
    /// `Game::tick` into `GameEvents`.
    pub(super) events: ClientEvents,
    /// Chat lines received from the server and not yet adopted by the app's
    /// client-side chat history.
    pub(super) chat_lines: Vec<ChatLine>,
    /// Replica sections installed during the current message drain. Their
    /// overlapping mesh invalidations are applied once after the batch.
    pub(super) section_installs: Vec<SectionPos>,
    /// Evicted replica sections parked for `SectionCached` re-promotion —
    /// harvested by the app shell on disconnect so a reconnect's Join
    /// manifest can claim them.
    pub(super) section_cache: SectionCache,
}

impl ReplicaState {
    pub(super) fn new(
        world: ReplicaWorld,
        entities: EntityReplica,
        self_view: SelfView,
        crafting: CraftingCatalog,
        fallback_world: SurfaceDensitySystem,
    ) -> Self {
        Self {
            world,
            entities,
            self_view,
            menu_view: MenuView::default(),
            crafting,
            fallback_world,
            events: ClientEvents::default(),
            chat_lines: Vec::new(),
            section_installs: Vec::new(),
            section_cache: SectionCache::default(),
        }
    }

    /// While sleeping, the engine yaw the lying body's head faces: from the
    /// bed's base (foot) cell toward its pillow cell. `None` while awake or if
    /// the bed vanished mid-sleep. The bed cell is REPLICATED
    /// (`SelfState::sleep_bed`); the bed's model group is read from the
    /// replica world (model cells replicate via payload states + deltas).
    pub(super) fn sleep_head_yaw(&self) -> Option<f32> {
        let base = self.self_view.sleep_bed?;
        let (_, _, cells) = self.world.model_group(base)?;
        let other: IVec3 = cells.iter().copied().find(|c| *c != base)?;
        let d = other - base;
        Some((d.x as f32).atan2(d.z as f32))
    }

    /// The stable id of the mob the local player rides, if the own mount is a
    /// mob (an anchor seat is not an entity and cannot be targeted or bumped).
    pub(super) fn own_mount_mob(&self) -> Option<u64> {
        self.entities.own_mount().and_then(|m| match m {
            PlayerMount::Mob { id, .. } => Some(id),
            PlayerMount::Anchor { .. } => None,
        })
    }

    /// The local player's frame on its mount this frame: `None` when
    /// unmounted, or while a mob mount's rows are not available yet — the
    /// caller keeps its current transform and waits for the rows to agree.
    /// A rider sits square in its seat and leans with it; only the head
    /// follows the look (see `collect_player`).
    pub(super) fn self_mount_pose(&self) -> Option<MountPose> {
        self.entities
            .mobs()
            .mount_pose(self.entities.own_mount()?, self.entities.alpha())
    }

    /// Dynamic collision boxes for the local player's physics this frame:
    /// every live SOLID entity (see `MobCollision::Solid`) at its
    /// interpolated replicated transform — except the own mount, whose box
    /// the slaved rider sits inside.
    pub(super) fn solid_entity_obstacles(&self) -> Vec<DynBox> {
        let alpha = self.entities.alpha();
        let own_mount = self.own_mount_mob();
        let mut out = Vec::new();
        for entry in self.entities.mobs().iter() {
            let row = &entry.curr;
            if row.dead || Some(row.id) == own_mount {
                continue;
            }
            let d = mob::def(mob::Mob(row.kind_id));
            if d.collision != mob::MobCollision::Solid {
                continue;
            }
            let (pos, yaw) = entry.interpolated_pose(alpha);
            mob::solid_boxes(row.id, pos, yaw, d.size, &mut out);
        }
        out
    }

    /// The closest replicated mob in front of the eye whose shared body boxes
    /// the ray enters within `max_dist` (and within reach), with its ray
    /// distance; skips dead corpses and the own mount. `max_dist` is the block
    /// hit distance, so a mob *behind* the block isn't targeted (the block
    /// occludes it).
    pub(super) fn closest_mob(
        &self,
        eye: WorldPos,
        dir: Vec3,
        max_dist: f32,
    ) -> Option<(u64, f32)> {
        let limit = max_dist.min(player::REACH);
        let own_mount = self.own_mount_mob();
        let alpha = self.entities.alpha();
        let bodies = self.entities.mobs().iter().filter_map(|entry| {
            let row = &entry.curr;
            (!row.dead && Some(row.id) != own_mount).then(|| {
                let (pos, yaw) = entry.interpolated_pose(alpha);
                (row.id, pos, yaw, mob::def(mob::Mob(row.kind_id)).size)
            })
        });
        mob::closest_body_ray_hit(eye, dir, limit, bodies)
    }

    /// The closest VISIBLE, alive remote player whose body AABB (row feet
    /// position + the player half-extents) the ray enters within `max_dist`
    /// (and within reach), with its ray distance — [`closest_mob`] over the
    /// remote-player rows. The store never holds the own id, so self-targeting
    /// is impossible; spectators and the dead ship `visible: false`/`alive:
    /// false` rows and are skipped.
    ///
    /// [`closest_mob`]: Self::closest_mob
    pub(super) fn closest_remote_player(
        &self,
        eye: WorldPos,
        dir: Vec3,
        max_dist: f32,
    ) -> Option<(u8, f32)> {
        let limit = max_dist.min(player::REACH);
        let mut best: Option<(u8, f32)> = None;
        for p in self.entities.players().iter() {
            let row = &p.curr;
            if !row.visible || !row.alive {
                continue;
            }
            let pos = row.transform.pos - eye;
            let min = Vec3::new(pos.x - player::HALF_W, pos.y, pos.z - player::HALF_W);
            let max = Vec3::new(
                pos.x + player::HALF_W,
                pos.y + player::HEIGHT,
                pos.z + player::HALF_W,
            );
            if let Some(t) = player::ray_vs_aabb(Vec3::ZERO, dir, min, max) {
                if t <= limit && best.is_none_or(|(_, bt)| t < bt) {
                    best = Some((row.id.0, t));
                }
            }
        }
        best
    }
}
