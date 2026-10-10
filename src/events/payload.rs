use crate::mob::{Mob, MobDamageFeedback};
use petramond_math::facing::Facing;
use petramond_math::math::{IVec3, Vec3};
use petramond_world::block::Block;
use petramond_world::chunk::SectionPos;
use petramond_world::item::ItemType;

#[derive(Copy, Clone, Debug)]
pub struct BlockPlacePre {
    pub pos: IVec3,
    pub block: Block,
    pub facing: Facing,
    pub actor: crate::mob::EntityRef,
}

#[derive(Clone, Debug)]
pub struct BlockBreakPre {
    pub pos: IVec3,
    pub block: Block,
    pub harvested: bool,
    pub actor: crate::mob::EntityRef,
    pub drops: Option<Vec<petramond_world::item::ItemStack>>,
}

#[derive(Copy, Clone, Debug)]
pub struct CellsEditPre {
    pub min: IVec3,
    pub max: IVec3,
    pub cells: usize,
    pub actor: crate::mob::EntityRef,
}

#[derive(Copy, Clone, Debug)]
pub struct InteractAttempt {
    pub block: Option<IVec3>,
    pub face: Option<IVec3>,
    pub mob: Option<u64>,
    pub player: crate::player::PlayerId,
}

#[derive(Copy, Clone, Debug)]
pub struct AttackAttempt {
    pub block: Option<IVec3>,
    pub face: Option<IVec3>,
    pub mob: Option<u64>,
    pub target: Option<crate::player::PlayerId>,
    pub player: crate::player::PlayerId,
}

#[derive(Clone, Debug)]
pub struct ProjectileHit {
    pub entity: u64,
    pub item: ItemType,
    pub owner: Option<crate::mob::EntityRef>,
    pub target: crate::world::ImpactTarget,
    pub pos: petramond_math::world_pos::WorldPos,
    pub vel: Vec3,
    pub fate: crate::entity::Fate,
}

#[derive(Copy, Clone, Debug)]
pub struct ItemUsePre {
    pub item: ItemType,
    pub target: Option<IVec3>,
}

#[derive(Clone, Debug)]
pub struct MobDamagePre {
    pub mob_id: u64,
    pub kind: Mob,
    pub amount: f32,
    pub source: DamageSource,
    pub origin: Option<petramond_math::world_pos::WorldPos>,
    pub feedback: MobDamageFeedback,
}

#[derive(Copy, Clone, Debug)]
pub struct PlayerDamagePre {
    pub amount: i32,
    pub source: DamageSource,
    pub origin: Option<petramond_math::world_pos::WorldPos>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DamageSource {
    Fall,
    PlayerAttack(crate::player::PlayerId),
    MobAttack { kind: Mob, id: u64 },
    Mod(&'static str),
    Fluid(petramond_world::block::Block),
    Condition(petramond_world::condition::ConditionId),
}

impl DamageSource {
    #[inline]
    pub fn is_attack(self) -> bool {
        matches!(self, Self::PlayerAttack(_) | Self::MobAttack { .. })
    }

    #[inline]
    pub fn attacker(self) -> Option<crate::mob::EntityRef> {
        match self {
            Self::PlayerAttack(pid) => Some(crate::mob::EntityRef::Player(pid)),
            Self::MobAttack { id, .. } => Some(crate::mob::EntityRef::Mob(id)),
            Self::Fall | Self::Fluid(_) | Self::Condition(_) | Self::Mod(_) => None,
        }
    }
}

#[derive(Clone, Debug)]
pub enum DeferredAction {
    DamagePlayer {
        player: crate::player::PlayerId,
        amount: i32,
        source: DamageSource,
        origin: Option<petramond_math::world_pos::WorldPos>,
    },
    DamageMob {
        mob_id: u64,
        amount: f32,
        source: DamageSource,
        origin: Option<petramond_math::world_pos::WorldPos>,
        feedback: Option<crate::mob::MobDamageFeedback>,
    },
    OpenGui {
        player: crate::player::PlayerId,
        kind: petramond_world::gui_state::GuiKind,
        anchor: Option<crate::menu::MenuAnchor>,
    },
    CloseGui {
        player: crate::player::PlayerId,
    },
    ChatSend {
        text: String,
        targets: Option<Vec<u8>>,
    },
    ActorBreak {
        mob_id: u64,
        pos: IVec3,
        target: crate::world::actor::DigTarget,
        tool_slot: Option<u32>,
        collect: bool,
    },
    ActorPlace {
        mob_id: u64,
        pos: IVec3,
        record: petramond_world::construction::Record,
        pay: bool,
    },
    ActorInteract {
        mob_id: u64,
        pos: IVec3,
    },
    ContainerHold {
        mob_id: u64,
        pos: IVec3,
        open: bool,
    },
    SchematicChoose {
        player: crate::player::PlayerId,
        tag: String,
    },
    SchematicPosition {
        player: crate::player::PlayerId,
        tag: String,
        asset: crate::schematic::store::Digest,
        origin: Option<IVec3>,
        turns: u8,
    },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ItemUseEvent {
    Eaten,
    Handler,
    Claimed,
}

#[derive(Clone, Debug)]
pub enum PostEvent {
    BlockPlaced {
        pos: IVec3,
        block: Block,
        player: Option<crate::player::PlayerId>,
    },
    BlockBroken {
        pos: IVec3,
        block: Block,
        harvested: bool,
        natural: bool,
        player: Option<crate::player::PlayerId>,
    },
    ItemUsed {
        player: crate::player::PlayerId,
        item: ItemType,
        kind: ItemUseEvent,
    },
    MobDied {
        id: u64,
        kind: Mob,
        pos: petramond_math::world_pos::WorldPos,
    },
    MobSpawned {
        id: u64,
        kind: Mob,
        pos: petramond_math::world_pos::WorldPos,
    },
    PlayerDamaged {
        player: crate::player::PlayerId,
        amount: i32,
        new_health: i32,
    },
    PlayerDied {
        player: crate::player::PlayerId,
    },
    ContainerOpened {
        player: crate::player::PlayerId,
        kind: petramond_world::gui_state::GuiKind,
        anchor: Option<crate::menu::MenuAnchor>,
    },
    ContainerClosed {
        player: crate::player::PlayerId,
        kind: petramond_world::gui_state::GuiKind,
        anchor: Option<crate::menu::MenuAnchor>,
    },
    SectionGenerated {
        pos: SectionPos,
    },
    SectionLoaded {
        pos: SectionPos,
    },
    PlayerDismounted {
        player: crate::player::PlayerId,
        mount: crate::mob::riding::Mount,
    },
    MobTagAdded {
        id: u64,
        kind: Mob,
        key: String,
        value: crate::mob::MobTagValue,
    },
    MobTagRemoved {
        id: u64,
        kind: Mob,
        key: String,
        value: crate::mob::MobTagValue,
    },
    ItemPickedUp {
        player: crate::player::PlayerId,
        item: ItemType,
        count: u8,
        pos: petramond_math::world_pos::WorldPos,
    },
    ItemObtained {
        player: crate::player::PlayerId,
        item: ItemType,
    },
    MobDamaged {
        mob_id: u64,
        kind: Mob,
        amount: f32,
        source: DamageSource,
        killed: bool,
    },
    Interacted {
        block: Option<IVec3>,
        face: Option<IVec3>,
        mob: Option<u64>,
        player: crate::player::PlayerId,
        consumed: bool,
    },
    ModEvent {
        key: String,
        data: Vec<u8>,
    },
    ActorActed {
        actor: crate::mob::EntityRef,
        pos: IVec3,
        action: mod_api::ActorAction,
        refusal: Option<mod_api::ActionRefusal>,
    },
    SchematicChosen {
        player: crate::player::PlayerId,
        tag: String,
        asset: crate::schematic::store::Digest,
    },
    SchematicPositioned {
        player: crate::player::PlayerId,
        tag: String,
        asset: crate::schematic::store::Digest,
        origin: IVec3,
        turns: u8,
    },
    /// Sent by `player`'s client mod instance; `key` and `data` are that client's claim.
    ClientEvent {
        player: crate::player::PlayerId,
        key: String,
        data: Vec<u8>,
    },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PostEventKind {
    BlockPlaced,
    BlockBroken,
    ItemUsed,
    MobDied,
    MobSpawned,
    PlayerDamaged,
    PlayerDied,
    ContainerOpened,
    ContainerClosed,
    SectionGenerated,
    SectionLoaded,
    PlayerDismounted,
    MobTagAdded,
    MobTagRemoved,
    ItemPickedUp,
    ItemObtained,
    MobDamaged,
    Interacted,
    ModEvent,
    ActorActed,
    SchematicChosen,
    SchematicPositioned,
    ClientEvent,
}

impl PostEventKind {
    pub const COUNT: usize = 23;
}

impl PostEvent {
    pub fn kind(&self) -> PostEventKind {
        match self {
            PostEvent::BlockPlaced { .. } => PostEventKind::BlockPlaced,
            PostEvent::BlockBroken { .. } => PostEventKind::BlockBroken,
            PostEvent::ItemUsed { .. } => PostEventKind::ItemUsed,
            PostEvent::MobDied { .. } => PostEventKind::MobDied,
            PostEvent::MobSpawned { .. } => PostEventKind::MobSpawned,
            PostEvent::PlayerDamaged { .. } => PostEventKind::PlayerDamaged,
            PostEvent::PlayerDied { .. } => PostEventKind::PlayerDied,
            PostEvent::ContainerOpened { .. } => PostEventKind::ContainerOpened,
            PostEvent::ContainerClosed { .. } => PostEventKind::ContainerClosed,
            PostEvent::SectionGenerated { .. } => PostEventKind::SectionGenerated,
            PostEvent::SectionLoaded { .. } => PostEventKind::SectionLoaded,
            PostEvent::PlayerDismounted { .. } => PostEventKind::PlayerDismounted,
            PostEvent::MobTagAdded { .. } => PostEventKind::MobTagAdded,
            PostEvent::MobTagRemoved { .. } => PostEventKind::MobTagRemoved,
            PostEvent::ItemPickedUp { .. } => PostEventKind::ItemPickedUp,
            PostEvent::ItemObtained { .. } => PostEventKind::ItemObtained,
            PostEvent::MobDamaged { .. } => PostEventKind::MobDamaged,
            PostEvent::Interacted { .. } => PostEventKind::Interacted,
            PostEvent::ModEvent { .. } => PostEventKind::ModEvent,
            PostEvent::ActorActed { .. } => PostEventKind::ActorActed,
            PostEvent::SchematicChosen { .. } => PostEventKind::SchematicChosen,
            PostEvent::SchematicPositioned { .. } => PostEventKind::SchematicPositioned,
            PostEvent::ClientEvent { .. } => PostEventKind::ClientEvent,
        }
    }

    pub fn actor(&self) -> Option<crate::player::PlayerId> {
        match self {
            PostEvent::BlockPlaced { player, .. } | PostEvent::BlockBroken { player, .. } => {
                *player
            }
            PostEvent::ItemUsed { player, .. }
            | PostEvent::PlayerDamaged { player, .. }
            | PostEvent::PlayerDied { player }
            | PostEvent::ContainerOpened { player, .. }
            | PostEvent::ContainerClosed { player, .. }
            | PostEvent::PlayerDismounted { player, .. }
            | PostEvent::ItemPickedUp { player, .. }
            | PostEvent::ItemObtained { player, .. }
            | PostEvent::Interacted { player, .. }
            | PostEvent::SchematicChosen { player, .. }
            | PostEvent::SchematicPositioned { player, .. }
            | PostEvent::ClientEvent { player, .. } => Some(*player),
            PostEvent::ActorActed { actor, .. } => actor.player(),
            PostEvent::MobDied { .. }
            | PostEvent::MobSpawned { .. }
            | PostEvent::SectionGenerated { .. }
            | PostEvent::SectionLoaded { .. }
            | PostEvent::MobTagAdded { .. }
            | PostEvent::MobTagRemoved { .. }
            | PostEvent::MobDamaged { .. }
            | PostEvent::ModEvent { .. } => None,
        }
    }
}
