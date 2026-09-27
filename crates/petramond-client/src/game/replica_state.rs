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
    pub(super) world: ReplicaWorld,
    pub(super) entities: EntityReplica,
    pub(super) self_view: SelfView,
    pub(super) menu_view: MenuView,
    pub(super) crafting: CraftingCatalog,
    pub(super) fallback_world: SurfaceDensitySystem,
    pub(super) events: ClientEvents,
    pub(super) chat_lines: Vec<ChatLine>,
    pub(super) section_installs: Vec<SectionPos>,
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

    pub(super) fn sleep_head_yaw(&self) -> Option<f32> {
        let base = self.self_view.sleep_bed?;
        let (_, _, cells) = self.world.model_group(base)?;
        let other: IVec3 = cells.iter().copied().find(|c| *c != base)?;
        let d = other - base;
        Some((d.x as f32).atan2(d.z as f32))
    }

    pub(super) fn own_mount_mob(&self) -> Option<u64> {
        self.entities.own_mount().and_then(|m| match m {
            PlayerMount::Mob { id, .. } => Some(id),
            PlayerMount::Anchor { .. } => None,
        })
    }

    pub(super) fn self_mount_pose(&self) -> Option<MountPose> {
        self.entities
            .mobs()
            .mount_pose(self.entities.own_mount()?, self.entities.alpha())
    }

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
