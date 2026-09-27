//! Client-side REPLICATED entity/self stores.
//!
//! The client renders mobs, dropped items and its own HUD state from these stores, fed by the
//! per-tick [`TickUpdate`] batches the server emits. The sim itself runs on its own thread and is
//! unreachable. Locally the batches are plain values over channels; over TCP the same messages
//! arrive remapped, so nothing here changes.
//!
//! Each store keeps the PREVIOUS and CURRENT row per stable id. `collect_mobs` and
//! `collect_item_entities` blend them at `tick_alpha`.
//!
//! The server replicates only the entities in this client's interest, via per-kind
//! [`EntityLane`](petramond::net::protocol::EntityLane)s. An entity stays in its store from spawn
//! to despawn, which is how leaving view differs from absence in one batch. Dying is shown from
//! its rows (`dead`, ragdoll) while it is still tracked.
//!
//! Light is left out of the rows on purpose. The client samples it at the entity position from
//! its REPLICA world.

use std::sync::Arc;

use glam::{Quat, Vec3};
use petramond_render::{AnimId, AnimInterner, AnimNames, GaitClip};

use petramond_world::gui_state::ContainerView;

use petramond::net::protocol::{
    ItemLane, ItemSlotWire, ItemStateRow, MenuSyncMsg, MenuTargetWire, MobLane, MobStateRow,
    PlayerActionKind, PlayerLane, PlayerMount, SelfState, SpatialSoundMsg, TickSection, TickUpdate,
    WorldEventMsg,
};

use crate::game::presentation::BodyEmitters;
use petramond::player::PlayerId;
use petramond::player::{Player, PlayerMode};
use petramond_math::math::{IVec3, Tilt};
use petramond_world::gui_state::GuiStateMap;
use petramond_world::inventory::{Hand, Inventory};
use petramond_world::item::{ItemStack, ItemType};

use super::tick::WorldEvent;
use super::Game;

mod entity_replica;
mod entity_store;
pub use entity_replica::{Committed, EntityReplica};
pub(crate) use entity_store::{Adopt, AssignFrom, EntityStore, Replica};

pub struct EntityWindow {
    pub tick: u64,
    // Shared straight off the batch: each lane selects rows out of tables
    // the server builds once per tick window, and the FIFO holds up to four
    // windows, so staging them is refcount bumps and index lists rather than
    // deep copies of every tracked entity.
    pub mobs: MobLane,
    pub items: ItemLane,
    pub players: PlayerLane,
    pub actions: Arc<[(PlayerId, PlayerActionKind)]>,
}

/// One `TickUpdate`'s entity rows, STAGED until render time crosses into
/// their segment (see [`ReplicaClock`](super::tick::ReplicaClock)): the
/// committed prev→curr pair under the render never shifts mid-segment, which
/// is what keeps interpolated motion — and a rider's camera glued to it —
/// free of arrival-jitter rubber-banding.
pub struct StagedRows {
    window: EntityWindow,
    folded: Vec<EntityWindow>,
    resync: bool,
}

impl StagedRows {
    pub fn new(window: EntityWindow) -> Self {
        Self {
            window,
            folded: Vec::new(),
            resync: false,
        }
    }

    pub fn windows(&self) -> impl Iterator<Item = &EntityWindow> {
        std::iter::once(&self.window).chain(&self.folded)
    }
}

#[cfg(test)]
pub(super) fn snapshot_lane<R: petramond::net::protocol::EntityRow>(
    held: impl Iterator<Item = R::Id>,
    rows: &[R],
) -> petramond::net::protocol::EntityLane<R, R::Id> {
    petramond::net::protocol::EntityLane {
        despawned: held
            .filter(|id| !rows.iter().any(|row| row.entity_id() == *id))
            .collect(),
        spawned: Default::default(),
        updated: rows.to_vec().into(),
    }
}

pub const MAX_STAGED_ROW_BATCHES: usize = 4;

const ANIM_BLEND_PER_SEC: f32 = 6.0;

pub struct ReplicatedMob {
    pub prev: MobStateRow,
    pub curr: MobStateRow,
    prev_anims: Vec<(AnimId, f32)>,
    curr_anims: Vec<(AnimId, f32)>,
    pub anim_blend: Vec<(AnimId, f32, f32)>,
    pub gait_blend: Vec<(GaitClip, f32, f32)>,
    pub draw: Option<petramond::world::draw::BlockDraw>,
    emitters: BodyEmitters,
}

fn resolve_draw(row: &MobStateRow) -> Option<petramond::world::draw::BlockDraw> {
    (!row.draw.prims.is_empty()).then(|| {
        std::sync::Arc::new(petramond::world::draw::BlockDrawSet::new(
            row.draw.prims.clone(),
        ))
    })
}

fn same_draw(a: &petramond::world::draw::DrawPrims, b: &petramond::world::draw::DrawPrims) -> bool {
    Arc::ptr_eq(&a.0, &b.0) || (a.is_empty() && b.is_empty()) || a == b
}

fn intern_anims(
    anims: &[(String, f32)],
    prev_names: &[(String, f32)],
    prev_ids: &[(AnimId, f32)],
    interner: &mut AnimInterner,
    out: &mut Vec<(AnimId, f32)>,
) {
    out.clear();
    out.extend(anims.iter().enumerate().map(|(i, (name, phase))| {
        let id = match (prev_names.get(i), prev_ids.get(i)) {
            (Some((prev, _)), Some(&(id, _))) if prev == name => id,
            _ => interner.intern(name),
        };
        (id, *phase)
    }));
}

pub fn gait_of(row: &MobStateRow) -> Option<GaitClip> {
    if row.moving {
        Some(GaitClip::Walk)
    } else {
        row.idle_anim.map(GaitClip::Idle)
    }
}

impl Replica<MobStateRow> for ReplicatedMob {
    type Ctx = AnimInterner;

    fn spawn(row: &MobStateRow, interner: &mut AnimInterner) -> Self {
        let mut mob = ReplicatedMob {
            prev: row.clone(),
            curr: row.clone(),
            prev_anims: Vec::new(),
            curr_anims: Vec::new(),
            anim_blend: Vec::new(),
            gait_blend: Vec::new(),
            draw: resolve_draw(row),
            emitters: BodyEmitters::default(),
        };
        intern_anims(&row.anims, &[], &[], interner, &mut mob.curr_anims);
        mob.prev_anims.clone_from(&mob.curr_anims);
        mob.emitters.refresh(&row.emitters, &row.conditions);
        mob.settle_blends();
        mob
    }

    fn advance(&mut self, row: &MobStateRow, interner: &mut AnimInterner) {
        if !same_draw(&self.curr.draw.prims, &row.draw.prims) {
            self.draw = resolve_draw(row);
        }
        std::mem::swap(&mut self.prev_anims, &mut self.curr_anims);
        intern_anims(
            &row.anims,
            &self.curr.anims,
            &self.prev_anims,
            interner,
            &mut self.curr_anims,
        );
        std::mem::swap(&mut self.prev, &mut self.curr);
        self.curr.assign_from(row);
        self.emitters.refresh(&row.emitters, &row.conditions);
    }

    fn reseed(&mut self, row: &MobStateRow, interner: &mut AnimInterner) {
        self.advance(row, interner);
        self.hold();
        self.settle_blends();
    }

    fn hold(&mut self) {
        self.prev.assign_from(&self.curr);
        self.prev_anims.clone_from(&self.curr_anims);
    }
}

impl ReplicatedMob {
    fn settle_blends(&mut self) {
        self.anim_blend.clear();
        self.anim_blend
            .extend(self.curr_anims.iter().map(|&(id, phase)| (id, 1.0, phase)));
        self.gait_blend.clear();
        self.gait_blend
            .extend(gait_of(&self.curr).map(|clip| (clip, 1.0, self.curr.anim_time)));
    }

    pub fn prev_anims(&self) -> &[(AnimId, f32)] {
        &self.prev_anims
    }

    pub fn curr_anims(&self) -> &[(AnimId, f32)] {
        &self.curr_anims
    }

    pub fn emitters(&self) -> &BodyEmitters {
        &self.emitters
    }

    pub fn interpolated_pose(&self, alpha: f32) -> (petramond_math::world_pos::WorldPos, f32) {
        (
            self.prev.pos.lerp(self.curr.pos, alpha),
            petramond_math::math::lerp_angle(self.prev.yaw, self.curr.yaw, alpha),
        )
    }

    pub fn interpolated_tilt(&self, alpha: f32) -> Tilt {
        self.prev.tilt.lerp(self.curr.tilt, alpha)
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct MountPose {
    pub seat: petramond_math::world_pos::WorldPos,
    pub body_yaw: f32,
    pub tilt: Tilt,
}

#[derive(Default)]
pub struct ReplicatedMobs {
    rows: EntityStore<u64, ReplicatedMob>,
    anim_names: AnimInterner,
}

impl ReplicatedMobs {
    pub fn apply(&mut self, lane: &MobLane) {
        self.rows
            .apply(lane, Adopt::Advance, &mut self.anim_names, |_| true);
    }

    #[cfg(test)]
    pub fn apply_snapshot(&mut self, rows: &[MobStateRow]) {
        let lane = snapshot_lane(self.rows.keys(), rows);
        self.apply(&lane);
    }

    fn resync(&mut self, lane: &MobLane) {
        self.rows
            .apply(lane, Adopt::Reseed, &mut self.anim_names, |_| true);
    }

    pub fn anim_names(&self) -> &AnimNames {
        self.anim_names.names()
    }

    pub fn advance_anim_blends(&mut self, dt: f32) {
        let step = ANIM_BLEND_PER_SEC * dt;
        for entry in self.rows.iter_mut() {
            for &(id, phase) in &entry.curr_anims {
                if !entry.anim_blend.iter().any(|(n, _, _)| *n == id) {
                    entry.anim_blend.push((id, 0.0, phase));
                }
            }
            let target = &entry.curr_anims;
            for (id, weight, phase) in entry.anim_blend.iter_mut() {
                match target.iter().find(|(n, _)| *n == *id) {
                    Some((_, row_phase)) => {
                        *weight = (*weight + step).min(1.0);
                        *phase = *row_phase;
                    }
                    None => *weight -= step,
                }
            }
            entry.anim_blend.retain(|(_, w, _)| *w > 0.0);
            let gait = gait_of(&entry.curr);
            if let Some(clip) = gait {
                if !entry.gait_blend.iter().any(|(c, _, _)| *c == clip) {
                    entry.gait_blend.push((clip, 0.0, entry.curr.anim_time));
                }
            }
            for (clip, weight, phase) in entry.gait_blend.iter_mut() {
                if Some(*clip) == gait {
                    *weight = (*weight + step).min(1.0);
                    *phase = entry.curr.anim_time;
                } else {
                    *weight -= step;
                }
            }
            entry.gait_blend.retain(|(_, w, _)| *w > 0.0);
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &ReplicatedMob> {
        self.rows.iter()
    }

    pub fn get(&self, id: u64) -> Option<&ReplicatedMob> {
        self.rows.get(&id)
    }

    pub fn mount_pose(&self, mount: PlayerMount, alpha: f32) -> Option<MountPose> {
        match mount {
            PlayerMount::Mob { id, seat } => {
                let entry = self.get(id)?;
                let def = petramond::mob::def(petramond::mob::Mob(entry.curr.kind_id));
                let offset = *def.seats.get(seat as usize)?;
                let (pos, yaw) = entry.interpolated_pose(alpha);
                let tilt = entry.interpolated_tilt(alpha);
                Some(MountPose {
                    seat: petramond::mob::riding::seat_world_pos(pos, yaw, tilt, offset),
                    body_yaw: petramond_math::math::wrap_angle(yaw + std::f32::consts::PI),
                    tilt,
                })
            }
            PlayerMount::Anchor { pos, yaw, .. } => Some(MountPose {
                seat: pos,
                body_yaw: yaw,
                tilt: Tilt::LEVEL,
            }),
        }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub struct ReplicatedItem {
    pub prev: ItemStateRow,
    pub curr: ItemStateRow,
}

#[derive(Default)]
pub struct ReplicatedItems {
    rows: EntityStore<u64, ReplicatedItem>,
}

impl Replica<ItemStateRow> for ReplicatedItem {
    type Ctx = ();

    fn spawn(row: &ItemStateRow, _: &mut ()) -> Self {
        ReplicatedItem {
            prev: row.clone(),
            curr: row.clone(),
        }
    }

    fn advance(&mut self, row: &ItemStateRow, _: &mut ()) {
        std::mem::swap(&mut self.prev, &mut self.curr);
        self.curr.assign_from(row);
    }

    fn reseed(&mut self, row: &ItemStateRow, _: &mut ()) {
        self.curr.assign_from(row);
        self.prev.assign_from(row);
    }

    fn hold(&mut self) {
        self.prev.assign_from(&self.curr);
    }
}

impl ReplicatedItems {
    pub fn apply(&mut self, lane: &ItemLane) {
        self.rows.apply(lane, Adopt::Advance, &mut (), |_| true);
    }

    fn resync(&mut self, lane: &ItemLane) {
        self.rows.apply(lane, Adopt::Reseed, &mut (), |_| true);
    }

    pub fn iter(&self) -> impl Iterator<Item = &ReplicatedItem> {
        self.rows.iter()
    }
}

pub struct SelfView {
    pub conditions: Vec<(u8, u8)>,
    pub health: i32,
    pub mode: PlayerMode,
    pub effects: Vec<(petramond_world::effect::Effect, u32)>,
    pub inventory: Inventory,
    pub inventory_revision: u64,
    pub mining: Option<(IVec3, u8)>,
    pub eating: Option<f32>,
    pub eating_off_hand: bool,
    pub sleeping: Option<f32>,
    pub sleep_bed: Option<IVec3>,
    pub move_scale: f32,
    pub fly_scale: f32,
    pub denied_actions: petramond::player::DeniedActions,
    pub held_pose_main: Option<mod_api::HeldPose>,
    pub held_pose_off: Option<mod_api::HeldPose>,
    pub held_display: [Option<ItemType>; 2],
    pub bone_poses: Vec<petramond::player::BonePose>,
    pub animator: petramond::player::AnimatorClaims,
}

impl SelfView {
    pub fn seed_from(player: &Player) -> Self {
        Self {
            health: player.health(),
            conditions: player
                .conditions()
                .active()
                .iter()
                .map(|c| (c.condition.0, c.stage()))
                .collect(),
            mode: player.mode(),
            effects: player
                .effects()
                .iter()
                .map(|e| (e.effect, e.remaining))
                .collect(),
            inventory: player.inventory.clone(),
            inventory_revision: player.inventory.revision(),
            mining: None,
            eating: None,
            eating_off_hand: false,
            sleeping: None,
            sleep_bed: None,
            move_scale: player
                .claims
                .replicated_attribute(mod_api::PlayerAttribute::MoveSpeed),
            fly_scale: player
                .claims
                .replicated_attribute(mod_api::PlayerAttribute::FlySpeed),
            denied_actions: player.claims.replicated_denied_actions(),
            held_pose_main: player.claims.held_pose(Hand::Main),
            held_pose_off: player.claims.held_pose(Hand::Off),
            held_display: [
                player.claims.held_display(Hand::Main),
                player.claims.held_display(Hand::Off),
            ],
            bone_poses: player.claims.bone_poses().collect(),
            animator: player.claims.animator().clone(),
        }
    }

    pub fn nobody() -> Self {
        Self::seed_from(&Player::new(petramond_math::world_pos::WorldPos::ZERO))
            .with_mode(PlayerMode::Spectator)
    }

    fn with_mode(mut self, mode: PlayerMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn to_wire(&self) -> SelfState {
        let inv = &self.inventory;
        SelfState {
            conditions: self.conditions.clone(),
            health: self.health,
            mode: self.mode.to_u8(),
            effects: self.effects.iter().map(|&(e, left)| (e.0, left)).collect(),
            inventory_revision: self.inventory_revision,
            inventory: Some(
                inv.raw_slots()
                    .iter()
                    .copied()
                    .chain(std::iter::once(inv.cursor().copied()))
                    .chain(std::iter::once(inv.off_hand().copied()))
                    .map(|slot| slot.map(petramond::net::protocol::ItemSlotWire::from_stack))
                    .collect(),
            ),
            eating: self
                .eating
                .map(|p| (p.clamp(0.0, 1.0) * 255.0).round() as u8),
            eating_off_hand: self.eating_off_hand,
            sleeping: self
                .sleeping
                .map(|p| (p.clamp(0.0, 1.0) * 255.0).round() as u8),
            sleep_bed: self.sleep_bed,
            move_scale: self.move_scale,
            fly_scale: self.fly_scale,
            denied_actions: self.denied_actions,
            held_pose_main: self.held_pose_main,
            held_pose_off: self.held_pose_off,
            held_display: self.held_display.map(|id| id.map(|i| i.0)),
            bone_poses: self.bone_poses.clone(),
            animator: self.animator.clone(),
            transform: None,
        }
    }

    pub fn health_view(&self) -> Option<petramond_world::gui_state::HealthView> {
        (self.mode == PlayerMode::Survival).then_some(petramond_world::gui_state::HealthView {
            current: self.health,
            max: petramond::player::MAX_HEALTH,
        })
    }

    pub fn effect_icons(&self) -> Vec<petramond_world::effect::Effect> {
        if self.mode != PlayerMode::Survival {
            return Vec::new();
        }
        self.effects.iter().map(|&(e, _)| e).collect()
    }

    pub fn apply(&mut self, state: &SelfState, adopt_inventory: bool) {
        self.health = state.health;
        self.conditions.clone_from(&state.conditions);
        self.mode = PlayerMode::from_u8(state.mode);
        self.effects = state
            .effects
            .iter()
            .map(|&(id, remaining)| (petramond_world::effect::Effect(id), remaining))
            .collect();
        if adopt_inventory {
            if let Some(slots) = &state.inventory {
                let active = self.inventory.active_slot();
                self.inventory = inventory_from_wire(slots, active);
            }
            self.inventory_revision = state.inventory_revision;
        }
        self.eating = state.eating.map(|p| p as f32 / 255.0);
        self.eating_off_hand = state.eating_off_hand;
        self.sleeping = state.sleeping.map(|p| p as f32 / 255.0);
        self.sleep_bed = state.sleep_bed;
        self.move_scale = state.move_scale;
        self.fly_scale = state.fly_scale;
        self.denied_actions = state.denied_actions;
        self.held_pose_main = state.held_pose_main;
        self.held_pose_off = state.held_pose_off;
        self.held_display = state.held_display.map(|id| id.map(ItemType));
        self.bone_poses.clone_from(&state.bone_poses);
        self.animator.clone_from(&state.animator);
    }
}
#[derive(Clone, Debug, Default)]
pub struct MenuView {
    pub craft_output: Option<ItemStack>,
    pub container: Option<ContainerView>,
    pub container_kind: Option<petramond_world::gui_state::GuiKind>,
    pub gui_state: Option<Arc<GuiStateMap>>,
}

fn stack_from_wire(slot: &Option<ItemSlotWire>) -> Option<ItemStack> {
    slot.as_ref().map(ItemSlotWire::to_stack)
}

impl MenuView {
    pub fn apply(&mut self, msg: MenuSyncMsg) {
        self.craft_output = None;
        self.container = None;
        self.container_kind = None;
        match msg.target {
            MenuTargetWire::None => {
                self.gui_state = None;
            }
            MenuTargetWire::Crafting { output } => {
                self.gui_state = None;
                self.craft_output = stack_from_wire(&output);
            }
            MenuTargetWire::Container {
                kind_key,
                slots,
                gui_state,
                ..
            } => {
                self.container_kind = petramond_world::gui_state::resolve_kind(&kind_key);
                self.container = slots.map(|slots| ContainerView {
                    slots: slots.iter().map(stack_from_wire).collect(),
                });
                if let Some(entries) = gui_state {
                    self.gui_state = Some(Arc::new(
                        entries
                            .into_iter()
                            .map(|(k, v)| (k, v.into_value()))
                            .collect(),
                    ));
                } else if self.gui_state.is_none() {
                    self.gui_state = Some(petramond_world::gui_state::empty_gui_state());
                }
            }
        }
    }

    pub fn adopt_gui_state(&mut self, msg: MenuSyncMsg) {
        if let MenuTargetWire::Container {
            gui_state: Some(entries),
            ..
        } = msg.target
        {
            self.gui_state = Some(Arc::new(
                entries
                    .into_iter()
                    .map(|(k, v)| (k, v.into_value()))
                    .collect(),
            ));
        }
    }
}

pub fn inventory_from_wire(
    slots: &[Option<petramond::net::protocol::ItemSlotWire>],
    active: u8,
) -> Inventory {
    let mut grid: [Option<ItemStack>; petramond_world::inventory::TOTAL_SLOTS] =
        [None; petramond_world::inventory::TOTAL_SLOTS];
    for (dst, src) in grid.iter_mut().zip(slots.iter()) {
        *dst = src.as_ref().map(ItemSlotWire::to_stack);
    }
    let cursor = slots
        .get(petramond_world::inventory::TOTAL_SLOTS)
        .and_then(|s| s.as_ref())
        .map(ItemSlotWire::to_stack);
    let off_hand = slots
        .get(petramond_world::inventory::TOTAL_SLOTS + 1)
        .and_then(|s| s.as_ref())
        .map(ItemSlotWire::to_stack);
    Inventory::from_parts(grid, cursor, off_hand, active)
}

pub fn lerp_ragdoll(
    prev: Option<&[([f32; 3], [f32; 4])]>,
    curr: &[([f32; 3], [f32; 4])],
    alpha: f32,
    out: &mut Vec<(Vec3, Quat)>,
) {
    let to_pose = |&(p, q): &([f32; 3], [f32; 4])| (Vec3::from(p), Quat::from_array(q));
    match prev {
        Some(prev) if prev.len() == curr.len() => {
            out.extend(prev.iter().zip(curr.iter()).map(|(a, b)| {
                let (pa, qa) = to_pose(a);
                let (pb, qb) = to_pose(b);
                (pa.lerp(pb, alpha), qa.slerp(qb, alpha))
            }))
        }
        _ => out.extend(curr.iter().map(to_pose)),
    }
}

impl Game {
    pub fn apply_server_messages(
        &mut self,
        msgs: &mut Vec<petramond::net::protocol::ServerToClient>,
    ) {
        use petramond::net::protocol::ServerToClient;
        debug_assert!(self.replica.section_installs.is_empty());
        for msg in msgs.drain(..) {
            match msg {
                ServerToClient::ColumnData(column) => {
                    self.replica.world.install_remote_column(column)
                }
                ServerToClient::SectionData(section) => {
                    self.replica.section_cache.discard(section.pos);
                    if let Some(pos) = self.replica.world.install_remote_section_deferred(*section)
                    {
                        self.replica.section_installs.push(pos);
                    }
                }
                ServerToClient::LightData(light) => self.replica.world.install_remote_light(light),
                ServerToClient::SectionUnload { pos, cache_hash } => {
                    let evicted = self.replica.world.uninstall_remote_section(pos);
                    if let (Some(section), Some(hash)) = (evicted, cache_hash) {
                        self.park_evicted_section(pos, section, hash);
                    }
                }
                ServerToClient::ColumnUnload { pos, cache_hashes } => {
                    for (sp, section) in self.replica.world.uninstall_remote_column(pos) {
                        if let Some(&(_, hash)) = cache_hashes.iter().find(|(cy, _)| *cy == sp.cy) {
                            self.park_evicted_section(sp, section, hash);
                        }
                    }
                }
                ServerToClient::SectionCached { pos, hash } => {
                    match self.replica.section_cache.promote(pos, hash) {
                        Some(section) => {
                            let pos = self.replica.world.install_cached_section(pos, section);
                            self.replica.section_installs.push(pos);
                        }
                        None => self.net.send_now(
                            petramond::net::protocol::ClientToServer::SectionCacheMiss { pos },
                        ),
                    }
                }
                ServerToClient::Tick(update) => self.apply_tick_update(update),
                ServerToClient::PlayerJoined { id, name } => {
                    self.replica.entities.player_joined(id, name);
                }
                ServerToClient::PlayerLeft { id } => {
                    self.replica.entities.player_left(id);
                }
                ServerToClient::ChatLine(line) => {
                    self.replica.chat_lines.push(line);
                }
                ServerToClient::RecipesUnlocked { recipes } => {
                    for recipe in recipes {
                        self.local.player.progression.unlock(&recipe);
                    }
                }
                ServerToClient::ModsDisabled { mods } => {
                    self.client_mods.disable_from_server(&mods);
                }
                ServerToClient::StreamBatchStart => self.net.stream_batch_started(),
                ServerToClient::StreamBatchEnd { count } => {
                    self.replica
                        .world
                        .finish_remote_install_batch(&self.replica.section_installs);
                    self.replica.section_installs.clear();
                    self.net.stream_batch_ended(count);
                }
                ServerToClient::KeepAlive => {}
                ServerToClient::ServerClosing => {
                    self.net.note_lost_because("the server closed");
                }
                ServerToClient::Disconnect { reason } => {
                    self.net
                        .note_lost_because(&format!("disconnected: {reason}"));
                }
                other => {
                    debug_assert!(false, "unexpected post-join message: {other:?}");
                }
            }
        }
        self.replica
            .world
            .finish_remote_install_batch(&self.replica.section_installs);
        self.replica.section_installs.clear();
    }

    /// Park one server-vouched evicted section in the section cache — unless
    /// a pending predicted edit touches it. The vouched hash covers the
    /// server's content at unload issue, which the ordered stream makes equal
    /// to the replica's copy at unload APPLY only when nothing local mutated
    /// it; an unconfirmed prediction breaks that, and a wrongly parked copy
    /// would re-promote as silent desync. Dropping instead costs one
    /// SectionCacheMiss round-trip if the section ever comes back.
    fn park_evicted_section(
        &mut self,
        pos: petramond_world::chunk::SectionPos,
        section: std::sync::Arc<petramond_world::section::Section>,
        hash: u64,
    ) {
        let predicted = self
            .prediction
            .suppressed_cells()
            .any(|c| petramond_world::chunk::SectionPos::from_world(c.x, c.y, c.z) == Some(pos));
        if !predicted {
            self.replica.section_cache.park(pos, section, hash);
        }
    }

    /// Mount→None edge: predict the SAME side-of-the-hull landing spot the
    /// server's riding pass chose, from the replica + interpolated rows. The
    /// local body was slaved INSIDE the hull's solid box; left there it keeps
    /// claiming that position, claim adoption keeps accepting it, and the
    /// body swims out through the boat. Prediction and authority run the one
    /// shared `dismount_spot`, so they converge without a visible correction.
    fn predict_dismount_placement(&mut self) {
        if self.local.player.is_spectator() {
            return;
        }
        let obstacles = self.replica.solid_entity_obstacles();
        let spot = petramond::mob::riding::dismount_spot(
            self.local.player.pos,
            self.local.player.yaw,
            |feet| {
                petramond::mob::riding::player_body_free(
                    self.replica.world.data(),
                    feet,
                    &obstacles,
                )
            },
            |feet| petramond::mob::riding::dismount_footing_safe(self.replica.world.data(), feet),
        );
        if let Some(feet) = spot {
            self.local.player.teleport(feet);
        }
    }

    pub fn advance_interp_window(&mut self) {
        if let Some(at) = self.presentation_position() {
            while let Some(committed) = self.replica.entities.commit_presented(at) {
                self.after_commit(committed);
            }
            return;
        }
        while let Some(committed) = self.replica.entities.commit_next_due() {
            self.after_commit(committed);
        }
    }

    fn after_commit(&mut self, committed: Committed) {
        if committed.dismounted {
            self.predict_dismount_placement();
        }
    }

    #[cfg(test)]
    pub fn commit_replication_window_for_test(&mut self) {
        self.replica
            .entities
            .advance_clock(crate::game::tick::TICK_DT * 1.001);
        self.advance_interp_window();
    }

    #[allow(clippy::boxed_local)]
    pub fn apply_tick_update(&mut self, update: Box<TickUpdate>) {
        let TickUpdate {
            tick,
            clock: _,
            sections,
        } = *update;
        self.replica.entities.set_tick(tick);
        let mut rows = EntityWindow {
            tick,
            mobs: MobLane::default(),
            items: ItemLane::default(),
            players: PlayerLane::default(),
            actions: Vec::new().into(),
        };
        let mut sleep_tally = None;
        let mut delta_cells = rustc_hash::FxHashSet::<IVec3>::default();
        let mut outcomes = Vec::new();
        let mut self_state = None;
        let mut menu_sync = None;
        let mut open_chests = Vec::new();
        let mut world_events = Vec::new();
        for section in sections {
            match section {
                TickSection::Creative(replies) => self.receive_creative_replies(replies),
                TickSection::Schematics(notices) => self.receive_schematic_notices(notices),
                TickSection::BlockDeltas(deltas) => {
                    delta_cells.extend(deltas.iter().map(|d| d.pos));
                    self.note_ghost_changes(deltas.iter().map(|d| d.pos));
                    for delta in deltas {
                        self.replica.world.apply_remote_delta(delta);
                    }
                }
                TickSection::BlockDraws(draws) => {
                    for d in draws {
                        self.replica.world.apply_remote_block_draw(d.pos, d.prims);
                    }
                }
                TickSection::CellKvDeltas(deltas) => {
                    for delta in deltas {
                        self.replica.world.apply_remote_cell_kv(delta);
                    }
                }
                TickSection::Mobs(lane) => rows.mobs = lane,
                TickSection::Items(lane) => rows.items = lane,
                TickSection::Players(lane) => rows.players = lane,
                TickSection::PlayerActions(actions) => rows.actions = actions,
                TickSection::SleepTally(tally) => sleep_tally = Some(tally),
                TickSection::ActionOutcomes(list) => outcomes = list,
                TickSection::SelfState(state) => self_state = Some(state),
                TickSection::MenuSync(sync) => menu_sync = Some(sync),
                TickSection::Env(env) => {
                    for (key, value) in env {
                        self.replica.world.set_shader_param(key, value);
                    }
                }
                TickSection::OpenChests(chests) => open_chests = chests,
                TickSection::Events(events) => world_events = events,
                TickSection::SelfEvents(events) => {
                    self.replica.events.self_events.merge_from(events);
                }
            }
        }
        let tally = sleep_tally.unwrap_or_else(|| self.replica.entities.sleep_tally());
        let committed = self.replica.entities.receive(tally, StagedRows::new(rows));
        self.after_commit(committed);
        self.fx.set_open_chests(open_chests.into_iter().collect());
        self.apply_authority(&outcomes, self_state, menu_sync, &delta_cells, world_events);
    }

    fn apply_authority(
        &mut self,
        outcomes: &[petramond::net::protocol::ActionOutcome],
        self_state: Option<SelfState>,
        menu_sync: Option<MenuSyncMsg>,
        delta_cells: &rustc_hash::FxHashSet<IVec3>,
        world_events: Vec<WorldEventMsg>,
    ) {
        // A batch's inventory / menu snapshot reflects the server state as of
        // the requests it ANSWERS. A snapshot-bearing prediction of the same
        // store that stays pending past this batch postdates that snapshot —
        // adopting it would visibly regress the newer prediction for one RTT
        // (the rapid-click flicker). Skip adoption then: every predicted menu
        // request forces the authoritative pair into its own outcome batch,
        // so the store reconciles the moment the pending queue drains.
        let stale_inventory = self.prediction.awaits_inventory_authority(outcomes);
        let stale_menu = self.prediction.awaits_menu_authority(outcomes);
        let adopted_inventory =
            !stale_inventory && self_state.as_ref().is_some_and(|s| s.inventory.is_some());
        let adopted_menu = !stale_menu && menu_sync.is_some();
        if let Some(state) = &self_state {
            self.replica.self_view.apply(state, !stale_inventory);
            if self.local.player.mode() != self.replica.self_view.mode {
                self.local.player.set_mode(self.replica.self_view.mode);
            }
            self.local.player.set_effects(
                self.replica
                    .self_view
                    .effects
                    .iter()
                    .map(
                        |&(effect, remaining)| petramond_world::effect::ActiveEffect {
                            effect,
                            remaining,
                        },
                    )
                    .collect(),
            );
            self.local.player.adopt_resolved_body(
                self.replica.self_view.move_scale,
                self.replica.self_view.fly_scale,
                self.replica.self_view.denied_actions,
            );
            if let Some(t) = &state.transform {
                self.adopt_authoritative_transform(t);
            }
        }
        let suppress: rustc_hash::FxHashSet<IVec3> = self.prediction.suppressed_cells().collect();
        // Authoritative inventory / block deltas win; then apply deny rollbacks
        // for any predicted mutations the server rejected. Snapshots come back
        // oldest-first, each capturing the state BEFORE its own prediction —
        // so a newer snapshot still embeds an older denied mutation. Applied
        // newest-first so the OLDEST snapshot wins.
        let rollbacks = self.prediction.reconcile(outcomes);
        for snap in rollbacks.into_iter().rev() {
            match snap {
                crate::game::prediction::PredictionSnapshot::None => {}
                crate::game::prediction::PredictionSnapshot::Inventory(inv) => {
                    if !adopted_inventory {
                        self.replica.self_view.inventory = inv;
                    }
                }
                crate::game::prediction::PredictionSnapshot::Menu { inventory, menu } => {
                    if !adopted_inventory {
                        self.replica.self_view.inventory = inventory;
                    }
                    if !adopted_menu {
                        self.replica.menu_view = menu;
                    }
                }
                crate::game::prediction::PredictionSnapshot::World { inventory, cells } => {
                    if let Some(inv) = inventory {
                        if !adopted_inventory {
                            self.replica.self_view.inventory = inv;
                        }
                    }
                    let mut restored = Vec::with_capacity(cells.len());
                    for (pos, prev_block_id) in cells {
                        if delta_cells.contains(&pos) {
                            continue;
                        }
                        let before = self.replica.world.data().chunk_block(pos.x, pos.y, pos.z);
                        let _ = self.replica.world.set_block_world(
                            pos.x,
                            pos.y,
                            pos.z,
                            petramond_world::block::Block::from_id(prev_block_id),
                        );
                        restored.push((pos, before));
                    }
                    self.replica.world.reconcile_predicted_edit(&restored);
                }
            }
        }
        if let Some(sync) = menu_sync {
            if stale_menu {
                self.replica.menu_view.adopt_gui_state(sync);
            } else {
                self.replica.menu_view.apply(sync);
            }
        }
        for msg in world_events {
            self.buffer_world_event(msg, &suppress);
        }
    }

    pub(super) fn buffer_world_event(
        &mut self,
        msg: WorldEventMsg,
        suppress: &rustc_hash::FxHashSet<IVec3>,
    ) {
        use crate::game::tick::{MobSoundEvent, SoundEvent, SpatialSoundCommand};
        let ev = &mut self.replica.events;
        match msg {
            WorldEventMsg::BlockBroken {
                pos,
                block_id,
                normal,
                tint,
            } => {
                if suppress.contains(&pos) {
                    return;
                }
                ev.world.push(WorldEvent::BlockBroken {
                    pos,
                    block: petramond_world::block::Block::from_id(block_id),
                    normal,
                    tint,
                });
            }
            WorldEventMsg::BlockPlaced { pos, block_id } => {
                if suppress.contains(&pos) {
                    return;
                }
                ev.world.push(WorldEvent::BlockPlaced {
                    pos,
                    block: petramond_world::block::Block::from_id(block_id),
                });
            }
            WorldEventMsg::PanelToggled { anchor, open } => {
                ev.world.push(WorldEvent::PanelToggled { anchor, open })
            }
            WorldEventMsg::ChestOpened { pos } => ev.world.push(WorldEvent::ChestOpened { pos }),
            WorldEventMsg::ChestClosed { pos } => ev.world.push(WorldEvent::ChestClosed { pos }),
            WorldEventMsg::ItemPickedUp { pos, by } => ev.world.push(WorldEvent::ItemPickedUp {
                pos,
                by_self: by == self.replica.entities.self_id(),
            }),
            WorldEventMsg::MobSound {
                mob_id,
                kind_id,
                category,
                pos,
            } => ev.mob_sounds.push(MobSoundEvent {
                mob_id,
                kind: petramond::mob::Mob(kind_id),
                category: petramond::mob::MobSoundCategory::from_u8(category),
                pos,
            }),
            WorldEventMsg::Sound { sound_id, pos } => ev.sounds.push(SoundEvent {
                sound: petramond_world::sound_registry::Sound(sound_id),
                pos,
            }),
            WorldEventMsg::EmitterBurst {
                emitter_id,
                pos,
                intensity,
                direction,
                texture,
            } => {
                use crate::particle::BurstLook;
                use petramond::net::protocol::BurstTextureMsg;
                let look = match texture {
                    None => BurstLook::Row,
                    Some(BurstTextureMsg::Block { block_id, tint }) => BurstLook::Block {
                        block: petramond_world::block::Block::from_id(block_id),
                        kv_tint: tint,
                    },
                    Some(BurstTextureMsg::Tile { tile, slice, tint }) => {
                        petramond_world::particle_emitters::TextureSlice::named(&tile, slice)
                            .map_or(BurstLook::Row, |slice| BurstLook::Texture {
                                slice,
                                tint: tint.map(|c| f32::from(c) / 255.0),
                            })
                    }
                };
                ev.world.push(WorldEvent::EmitterBurst {
                    emitter: emitter_id,
                    pos,
                    intensity,
                    direction,
                    look,
                })
            }
            WorldEventMsg::SpatialSound(cmd) => ev.spatial_sounds.push(match cmd {
                SpatialSoundMsg::PlayAt {
                    handle,
                    sound_id,
                    pos,
                    volume,
                    pitch,
                } => SpatialSoundCommand::PlayAt {
                    handle,
                    sound: petramond_world::sound_registry::Sound(sound_id),
                    pos,
                    volume,
                    pitch,
                },
                SpatialSoundMsg::PlayOnMob {
                    handle,
                    sound_id,
                    mob_id,
                    volume,
                    pitch,
                    last_pos,
                } => SpatialSoundCommand::PlayOnMob {
                    handle,
                    sound: petramond_world::sound_registry::Sound(sound_id),
                    mob_id,
                    volume,
                    pitch,
                    last_pos,
                },
                SpatialSoundMsg::Set {
                    handle,
                    volume,
                    pitch,
                } => SpatialSoundCommand::Set {
                    handle,
                    volume,
                    pitch,
                },
                SpatialSoundMsg::Stop { handle } => SpatialSoundCommand::Stop { handle },
            }),
        }
    }
}
