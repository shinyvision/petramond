//! Wire-format pin: one canonical sample of every ABI enum variant, encoded
//! and compared against recorded bytes.
//!
//! postcard encodes enum variants by DECLARATION INDEX and struct fields
//! positionally, so reordering variants, inserting one anywhere but the end,
//! or reshaping a variant's fields silently changes the wire format — a
//! refactor that only *moves* code can break every compiled `mod.wasm`
//! without any test noticing (round-trips still pass, both sides re-derive
//! the new dialect together).
//!
//! This pin makes wire changes DELIBERATE, never accidental. The pre-release
//! policy (crate docs) allows reshaping the ABI freely: when you mean to
//! change it, run this test, paste the printed replacement block over `PINS`
//! in the same change, and rebuild the mods (`make mods`). When you did NOT
//! mean to change it, this failure is the only thing standing between you
//! and a silently re-numbered protocol.
//!
//! Appending a NEW variant at the end never disturbs existing pins (that is
//! exactly why append is the safe evolution) — add a sample for it here so
//! the next refactor covers it too.

use crate::*;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

struct Samples(Vec<(&'static str, String)>);

impl Samples {
    fn pin<T: serde::Serialize>(&mut self, name: &'static str, value: &T) {
        self.0
            .push((name, hex(&encode(value).expect("wire pin sample encodes"))));
    }
}

#[rustfmt::skip]
fn samples() -> Samples {
    let mut s = Samples(Vec::new());

    // --- HostCall: every variant, declaration order ------------------------
    s.pin("HostCall::Log", &HostCall::Log { msg: "a".into() });
    s.pin("HostCall::CurrentTick", &HostCall::CurrentTick);
    s.pin("HostCall::RngU64", &HostCall::RngU64 { stream_key: "s".into() });
    s.pin("HostCall::RegisterTickSystem", &HostCall::RegisterTickSystem {
        stage: Stage::Mining, attach: AttachSide::Before, priority: -1, system_id: 1,
    });
    s.pin("HostCall::RegisterEventHandler", &HostCall::RegisterEventHandler {
        event: EventKind::BlockPlacePre, priority: 1, handler_id: 2,
    });
    s.pin("HostCall::GetBlock", &HostCall::GetBlock { pos: [1, -2, 3] });
    s.pin("HostCall::GetBlocks", &HostCall::GetBlocks { positions: vec![[0, 0, 0]] });
    s.pin("HostCall::SetBlock", &HostCall::SetBlock { pos: [1, 2, 3], block: BlockId(4) });
    s.pin("HostCall::SetBlocks", &HostCall::SetBlocks { blocks: vec![([1, 2, 3], BlockId(5))] });
    s.pin("HostCall::ScheduleTick", &HostCall::ScheduleTick { pos: [1, 2, 3], delay: 7 });
    s.pin("HostCall::IsLoaded", &HostCall::IsLoaded { pos: [1, 2, 3] });
    s.pin("HostCall::LightAt", &HostCall::LightAt { pos: [1, 2, 3] });
    s.pin("HostCall::SpawnMob", &HostCall::SpawnMob {
        key: "m:k".into(), pos: [1.0, 2.0, 3.0], yaw: 0.5,
    });
    s.pin("HostCall::MobsInRadius", &HostCall::MobsInRadius { pos: [1.0, 2.0, 3.0], radius: 4.0 });
    s.pin("HostCall::DamageMob", &HostCall::DamageMob {
        index: 1, amount: 2.0, origin: Some([1.0, 2.0, 3.0]),
    });
    s.pin("HostCall::DespawnMob", &HostCall::DespawnMob { index: 2 });
    s.pin("HostCall::SpawnItem", &HostCall::SpawnItem {
        item_key: "m:i".into(), count: 3, pos: [1.0, 2.0, 3.0],
    });
    s.pin("HostCall::PlayerState", &HostCall::PlayerState);
    s.pin("HostCall::DamagePlayer", &HostCall::DamagePlayer { amount: 2 });
    s.pin("HostCall::ApplyKnockback", &HostCall::ApplyKnockback { impulse: [1.0, 2.0, 3.0] });
    s.pin("HostCall::GiveItem", &HostCall::GiveItem { item_key: "m:i".into(), count: 2 });
    s.pin("HostCall::KillPlayer", &HostCall::KillPlayer);
    s.pin("HostCall::SetHealth", &HostCall::SetHealth { value: 20 });
    s.pin("HostCall::Teleport", &HostCall::Teleport { pos: [1.0, 2.0, 3.0] });
    s.pin("HostCall::EmitSound", &HostCall::EmitSound {
        key: "m:s".into(), pos: Some([1.0, 2.0, 3.0]),
    });
    s.pin("HostCall::WorldKvGet", &HostCall::WorldKvGet { key: "m:k".into() });
    s.pin("HostCall::WorldKvSet", &HostCall::WorldKvSet { key: "m:k".into(), value: vec![1] });
    s.pin("HostCall::WorldKvDelete", &HostCall::WorldKvDelete { key: "m:k".into() });
    s.pin("HostCall::SectionKvGet", &HostCall::SectionKvGet { pos: [1, 2, 3], key: "m:k".into() });
    s.pin("HostCall::SectionKvSet", &HostCall::SectionKvSet {
        pos: [1, 2, 3], key: "m:k".into(), value: vec![2],
    });
    s.pin("HostCall::SectionKvDelete", &HostCall::SectionKvDelete {
        pos: [1, 2, 3], key: "m:k".into(),
    });
    s.pin("HostCall::MobKvGet", &HostCall::MobKvGet { mob_index: 1, key: "m:k".into() });
    s.pin("HostCall::MobKvSet", &HostCall::MobKvSet {
        mob_index: 1, key: "m:k".into(), value: vec![3],
    });
    s.pin("HostCall::MobKvDelete", &HostCall::MobKvDelete { mob_index: 1, key: "m:k".into() });
    s.pin("HostCall::ResolveBlock", &HostCall::ResolveBlock { key: "m:b".into() });
    s.pin("HostCall::RegisterWorldgenFeature", &HostCall::RegisterWorldgenFeature {
        feature_id: 1, stage: WorldgenStage::Trees,
    });
    s.pin("HostCall::RegisterStageReplacement", &HostCall::RegisterStageReplacement {
        stage: WorldgenStage::Terrain, callback_id: 2,
    });
    s.pin("HostCall::RegisterGenerator", &HostCall::RegisterGenerator { callback_id: 3 });
    s.pin("HostCall::GuiStateSet", &HostCall::GuiStateSet {
        key: "k".into(), value: GuiValue::I32(1),
    });
    s.pin("HostCall::GuiStateGet", &HostCall::GuiStateGet { key: "k".into() });
    s.pin("HostCall::GuiOpen", &HostCall::GuiOpen { kind_key: "m:g".into() });
    s.pin("HostCall::GuiClose", &HostCall::GuiClose);
    s.pin("HostCall::ChatSend", &HostCall::ChatSend { text: "t".into(), targets: Some(vec![1]) });
    s.pin("HostCall::SoundPlayAt", &HostCall::SoundPlayAt {
        key: "m:s".into(), pos: [1.0, 2.0, 3.0], volume: 1.0, pitch: 1.0,
    });
    s.pin("HostCall::SoundPlayOnMob", &HostCall::SoundPlayOnMob {
        mob_id: 1, key: "m:s".into(), volume: 1.0, pitch: 1.0,
    });
    s.pin("HostCall::SoundStop", &HostCall::SoundStop { handle: 1 });
    s.pin("HostCall::BlockIsFullSpawnSupport", &HostCall::BlockIsFullSpawnSupport {
        pos: [1, 2, 3],
    });
    s.pin("HostCall::ShaderSetParam", &HostCall::ShaderSetParam {
        key: "m:p".into(), value: [0.0, 0.25, 0.5, 1.0],
    });
    s.pin("HostCall::RegisterHostileSpawner", &HostCall::RegisterHostileSpawner {
        callback_id: 1, priority: 2,
    });
    s.pin("HostCall::RegisterBlockBehavior", &HostCall::RegisterBlockBehavior {
        key: "m:b".into(), callback_id: 1,
    });
    s.pin("HostCall::RegisterAiNode", &HostCall::RegisterAiNode {
        key: "m:n".into(), callback_id: 2,
    });
    s.pin("HostCall::ContainerGet", &HostCall::ContainerGet { pos: [1, 2, 3] });
    s.pin("HostCall::ContainerSet", &HostCall::ContainerSet {
        pos: [1, 2, 3],
        slots: vec![(0, Some(ItemStackData { key: "m:i".into(), count: 1 })), (1, None)],
    });
    s.pin("HostCall::ItemInfo", &HostCall::ItemInfo { key: "m:i".into() });
    s.pin("HostCall::RecipeResult", &HostCall::RecipeResult {
        class: "m:c".into(), key: "m:i".into(),
    });
    s.pin("HostCall::EffectApply", &HostCall::EffectApply { key: "m:e".into(), ticks: 5 });
    s.pin("HostCall::EffectRemove", &HostCall::EffectRemove { key: "m:e".into() });
    s.pin("HostCall::EffectsActive", &HostCall::EffectsActive);
    s.pin("HostCall::SwapModelBlock", &HostCall::SwapModelBlock {
        pos: [1, 2, 3], block: BlockId(6),
    });
    s.pin("HostCall::ContainerGetMany", &HostCall::ContainerGetMany {
        positions: vec![[1, 2, 3]],
    });
    s.pin("HostCall::MobEmitterSet", &HostCall::MobEmitterSet {
        index: 1, key: "m:e".into(), active: true,
    });
    s.pin("HostCall::EmitterBurst", &HostCall::EmitterBurst {
        key: "m:e".into(), pos: [1.0, 2.0, 3.0], intensity: 2.0,
    });
    s.pin("HostCall::RuntimeSide", &HostCall::RuntimeSide);
    s.pin("HostCall::ClientRegisterOverlay", &HostCall::ClientRegisterOverlay {
        image_key: "m:i".into(), anchor: ClientOverlayAnchor::TopLeft,
        margin: [1, 2], display_size: [3, 4],
    });
    s.pin("HostCall::ClientRegisterKey", &HostCall::ClientRegisterKey {
        id: "open_map".into(), label: "Open World Map".into(),
        key: "key_m".into(), action_id: 1,
    });
    s.pin("HostCall::ClientSurface", &HostCall::ClientSurface { center: [1, 2], radius: 3 });
    s.pin("HostCall::ClientUiStateSet", &HostCall::ClientUiStateSet {
        key: "m:k".into(), value: GuiValue::Str("v".into()),
    });
    s.pin("HostCall::ClientUiStateGet", &HostCall::ClientUiStateGet { key: "m:k".into() });
    s.pin("HostCall::ClientImageSet", &HostCall::ClientImageSet {
        key: "m:i".into(), width: 1, height: 1, rgba: vec![1, 2, 3, 4],
    });
    s.pin("HostCall::ClientTextMeasure", &HostCall::ClientTextMeasure {
        text: "t".into(), scale: 2,
    });
    s.pin("HostCall::ClientImageDrawTexts", &HostCall::ClientImageDrawTexts {
        key: "m:i".into(),
        runs: vec![ClientTextRun { text: "t".into(), position: [1, 2], scale: 1, color: [1, 2, 3, 4] }],
    });
    s.pin("HostCall::ClientGuiOpen", &HostCall::ClientGuiOpen { kind_key: "m:g".into() });
    s.pin("HostCall::ClientGuiClose", &HostCall::ClientGuiClose);
    s.pin("HostCall::ClientCanvasOpen", &HostCall::ClientCanvasOpen {
        canvas_key: "m:c".into(), size: [1, 2],
    });
    s.pin("HostCall::ClientCanvasClose", &HostCall::ClientCanvasClose);
    s.pin("HostCall::ClientCanvasSceneSet", &HostCall::ClientCanvasSceneSet {
        canvas_key: "m:c".into(),
        elements: vec![
            ClientCanvasElement::Image { image_key: "m:i".into(), rect: [1.0, 2.0, 3.0, 4.0] },
            ClientCanvasElement::Sprite { image_key: "m:i".into(), center: [1.0, 2.0] },
        ],
    });
    s.pin("HostCall::ClientCanvasViewSet", &HostCall::ClientCanvasViewSet {
        canvas_key: "m:c".into(), offset: [1.0, 2.0],
    });
    s.pin("HostCall::ClientStorageGetMany", &HostCall::ClientStorageGetMany {
        keys: vec!["m:k".into()],
    });
    s.pin("HostCall::ClientStorageSetMany", &HostCall::ClientStorageSetMany {
        entries: vec![("m:k".into(), vec![1])],
    });

    // --- HostRet: every variant, declaration order --------------------------
    s.pin("HostRet::Unit", &HostRet::Unit);
    s.pin("HostRet::U64", &HostRet::U64(1));
    s.pin("HostRet::Error", &HostRet::Error("e".into()));
    s.pin("HostRet::Bool", &HostRet::Bool(true));
    s.pin("HostRet::Block", &HostRet::Block(Some(BlockId(1))));
    s.pin("HostRet::Blocks", &HostRet::Blocks(vec![None, Some(BlockId(2))]));
    s.pin("HostRet::Light", &HostRet::Light { combined: 1, sky: 2, block: 3 });
    s.pin("HostRet::Mobs", &HostRet::Mobs(vec![MobSnapshot {
        index: 1, key: "m:k".into(), pos: [1.0, 2.0, 3.0], health: 4.0, id: 5,
    }]));
    s.pin("HostRet::Player", &HostRet::Player(PlayerSnapshot {
        pos: [1.0, 2.0, 3.0], vel: [0.0, 0.0, 0.0], yaw: 0.5, pitch: 0.25,
        health: 20, on_ground: true, spectator: false,
    }));
    s.pin("HostRet::Bytes", &HostRet::Bytes(Some(vec![1])));
    s.pin("HostRet::GuiValue", &HostRet::GuiValue(Some(GuiValue::F32(1.0))));
    s.pin("HostRet::ContainerSlots", &HostRet::ContainerSlots(Some(vec![
        Some(ItemStackData { key: "m:i".into(), count: 1 }), None,
    ])));
    s.pin("HostRet::ItemInfo", &HostRet::ItemInfo(Some(ItemInfoData {
        max_stack: 64, fuel_burn_ticks: 0, tags: vec!["t".into()],
    })));
    s.pin("HostRet::ItemStack", &HostRet::ItemStack(Some(ItemStackData {
        key: "m:i".into(), count: 2,
    })));
    s.pin("HostRet::Effects", &HostRet::Effects(vec![EffectStateData {
        key: "m:e".into(), remaining: 9,
    }]));
    s.pin("HostRet::Containers", &HostRet::Containers(vec![Some(vec![None]), None]));
    s.pin("HostRet::RuntimeSide", &HostRet::RuntimeSide(RuntimeSide::Client));
    s.pin("HostRet::ClientSurface", &HostRet::ClientSurface(vec![
        None, Some(ClientSurfaceCell { height: -1, rgb: [1, 2, 3] }),
    ]));
    s.pin("HostRet::ClientTextSize", &HostRet::ClientTextSize([1, 2]));
    s.pin("HostRet::ClientStorageValues", &HostRet::ClientStorageValues(vec![None, Some(vec![1])]));

    // --- GuestCall: every variant, declaration order -------------------------
    s.pin("GuestCall::TickSystem", &GuestCall::TickSystem { id: 1 });
    s.pin("GuestCall::HandleEvent", &GuestCall::HandleEvent {
        id: 1, kind: EventKind::PlayerDied, payload: EventPayload::PlayerDied,
    });
    s.pin("GuestCall::GenFeature", &GuestCall::GenFeature {
        feature_id: 1, section_pos: [1, 2, 3], seed: 4,
        blocks: vec![1, 2], surface_heights: vec![5], biomes: vec![6], sea_level: 7,
    });
    s.pin("GuestCall::GenStage", &GuestCall::GenStage {
        callback_id: 1, stage: WorldgenStage::Underground, section_pos: [1, 2, 3], seed: 4,
        blocks: vec![1], surface_heights: vec![2], biomes: vec![3], sea_level: 4,
    });
    s.pin("GuestCall::GuiClick", &GuestCall::GuiClick {
        kind_key: "m:g".into(), widget_id: "w".into(), pos: Some([1, 2, 3]),
    });
    s.pin("GuestCall::HostileSpawnCandidate", &GuestCall::HostileSpawnCandidate {
        callback_id: 1,
        candidate: HostileSpawnCandidate {
            pos: [1.0, 2.0, 3.0], cell: [1, 2, 3], combined_light: 1, sky_light: 2, block_light: 3,
        },
    });
    s.pin("GuestCall::BlockBehavior", &GuestCall::BlockBehavior {
        callback_id: 1, kind: BlockHookKind::RandomTick, pos: [1, 2, 3],
    });
    s.pin("GuestCall::AiNode", &GuestCall::AiNode {
        callback_id: 1,
        ctx: AiNodeCtx {
            mob_id: 1, pos: [1.0, 2.0, 3.0], cell: [1, 2, 3], yaw: 0.5,
            player_pos: [4.0, 5.0, 6.0], nav_idle: true, in_water: false,
        },
    });
    s.pin("GuestCall::ClientFrame", &GuestCall::ClientFrame {
        frame: ClientFrameData {
            dt: 0.05, player_pos: [1.0, 2.0, 3.0], yaw: 0.5, pitch: 0.25,
            screen: [640, 480], open_gui: Some("m:g".into()), open_canvas: None,
        },
    });
    s.pin("GuestCall::ClientKey", &GuestCall::ClientKey { action_id: 1, pressed: true });
    s.pin("GuestCall::ClientUi", &GuestCall::ClientUi {
        kind_key: "m:g".into(), event: ClientUiEvent::Click { id: "b".into() },
    });
    s.pin("GuestCall::ClientCanvas", &GuestCall::ClientCanvas {
        canvas_key: "m:c".into(),
        event: ClientCanvasEvent {
            phase: ClientPointerPhase::Down, x: 1.0, y: 2.0, button: ClientPointerButton::Primary,
        },
    });

    // --- GuestRet: every variant, declaration order --------------------------
    s.pin("GuestRet::Unit", &GuestRet::Unit);
    s.pin("GuestRet::Event", &GuestRet::Event {
        outcome: Outcome::Cancel, payload: EventPayload::ItemUsed { item: ItemId(1) },
    });
    s.pin("GuestRet::GenWrites", &GuestRet::GenWrites(vec![([1, 2, 3], BlockId(4))]));
    s.pin("GuestRet::GenBlocks", &GuestRet::GenBlocks(vec![1, 2]));
    s.pin("GuestRet::GenBiomes", &GuestRet::GenBiomes(vec![3]));
    s.pin("GuestRet::HostileSpawn", &GuestRet::HostileSpawn(Some("m:k".into())));
    s.pin("GuestRet::AiDecision", &GuestRet::AiDecision(Some(AiNodeDecision {
        goal: Some([1, 2, 3]), head_look: Some([0.5, 0.25]),
        idle_anim: Some(1), attack: Some([2.0, 3.0]),
    })));

    // --- EventPayload: every variant, declaration order ----------------------
    s.pin("EventPayload::BlockPlacePre", &EventPayload::BlockPlacePre {
        pos: [1, 2, 3], block: BlockId(1), facing: Facing::North,
    });
    s.pin("EventPayload::BlockBreakPre", &EventPayload::BlockBreakPre {
        pos: [1, 2, 3], block: BlockId(1), harvested: true,
    });
    s.pin("EventPayload::BlockInteract", &EventPayload::BlockInteract {
        pos: [1, 2, 3], block: BlockId(1),
    });
    s.pin("EventPayload::ItemUsePre", &EventPayload::ItemUsePre {
        item: ItemId(1), target: Some([1, 2, 3]),
    });
    s.pin("EventPayload::MobDamagePre", &EventPayload::MobDamagePre {
        mob: 1, kind: MobId(2), amount: 3.0, source: DamageSource::Fall,
        origin: Some([1.0, 2.0, 3.0]),
        feedback: MobDamageFeedback {
            components: vec![
                MobDamageFeedbackComponent::DecreaseHealth,
                MobDamageFeedbackComponent::Flash { duration: 0.5 },
                MobDamageFeedbackComponent::Knockback { scale: 1.0, duration: 0.5 },
                MobDamageFeedbackComponent::Sound { category: MobDamageSound::Hurt },
                MobDamageFeedbackComponent::Ragdoll,
            ],
        },
    });
    s.pin("EventPayload::PlayerDamagePre", &EventPayload::PlayerDamagePre {
        amount: 1, source: DamageSource::PlayerAttack { id: 1 }, origin: None,
    });
    s.pin("EventPayload::BlockPlaced", &EventPayload::BlockPlaced {
        pos: [1, 2, 3], block: BlockId(1),
    });
    s.pin("EventPayload::BlockBroken", &EventPayload::BlockBroken {
        pos: [1, 2, 3], block: BlockId(1), harvested: false, natural: true,
    });
    s.pin("EventPayload::ItemUsed", &EventPayload::ItemUsed { item: ItemId(2) });
    s.pin("EventPayload::MobDied", &EventPayload::MobDied { kind: MobId(1), pos: [1.0, 2.0, 3.0] });
    s.pin("EventPayload::MobSpawned", &EventPayload::MobSpawned {
        kind: MobId(1), pos: [1.0, 2.0, 3.0],
    });
    s.pin("EventPayload::PlayerDamaged", &EventPayload::PlayerDamaged {
        amount: 1, new_health: 19,
    });
    s.pin("EventPayload::PlayerDied", &EventPayload::PlayerDied);
    s.pin("EventPayload::ContainerOpened", &EventPayload::ContainerOpened {
        kind: ContainerKind::Chest, pos: Some([1, 2, 3]),
    });
    s.pin("EventPayload::ContainerClosed", &EventPayload::ContainerClosed {
        kind: ContainerKind::Mod { key: "m:g".into() }, pos: None,
    });
    s.pin("EventPayload::SectionGenerated", &EventPayload::SectionGenerated { pos: [1, 2, 3] });
    s.pin("EventPayload::SectionLoaded", &EventPayload::SectionLoaded { pos: [1, 2, 3] });

    // --- Auxiliary enums: ALL variants of each, encoded as one Vec ----------
    s.pin("Outcome::*", &vec![Outcome::Continue, Outcome::Cancel]);
    s.pin("Stage::*", &vec![
        Stage::Mining, Stage::Placement, Stage::Attack, Stage::Drops, Stage::Menu,
        Stage::PlayerDamage, Stage::WorldScheduled, Stage::NaturalBreaks, Stage::Pickup,
        Stage::Mobs, Stage::ItemPhysics, Stage::Spawning,
    ]);
    s.pin("AttachSide::*", &vec![AttachSide::Before, AttachSide::After]);
    s.pin("WorldgenStage::*", &vec![
        WorldgenStage::Climate, WorldgenStage::Terrain, WorldgenStage::Underground,
        WorldgenStage::Vegetation, WorldgenStage::Trees,
    ]);
    s.pin("EventKind::*", &vec![
        EventKind::BlockPlacePre, EventKind::BlockBreakPre, EventKind::BlockInteract,
        EventKind::ItemUsePre, EventKind::MobDamagePre, EventKind::PlayerDamagePre,
        EventKind::BlockPlaced, EventKind::BlockBroken, EventKind::ItemUsed,
        EventKind::MobDied, EventKind::MobSpawned, EventKind::PlayerDamaged,
        EventKind::PlayerDied, EventKind::ContainerOpened, EventKind::ContainerClosed,
        EventKind::SectionGenerated, EventKind::SectionLoaded,
    ]);
    s.pin("DamageSource::*", &vec![
        DamageSource::Fall,
        DamageSource::PlayerAttack { id: 1 },
        DamageSource::MobAttack { key: "m:k".into() },
        DamageSource::Mod { mod_id: "m".into() },
    ]);
    s.pin("ContainerKind::*", &vec![
        ContainerKind::Inventory, ContainerKind::CraftingTable, ContainerKind::Furnace,
        ContainerKind::Chest, ContainerKind::FurnitureWorkbench,
        ContainerKind::Mod { key: "m:g".into() },
    ]);
    s.pin("Facing::*", &vec![Facing::North, Facing::South, Facing::West, Facing::East]);
    s.pin("MobDamageFeedbackComponent::*", &vec![
        MobDamageFeedbackComponent::DecreaseHealth,
        MobDamageFeedbackComponent::Flash { duration: 0.5 },
        MobDamageFeedbackComponent::Knockback { scale: 1.0, duration: 0.5 },
        MobDamageFeedbackComponent::Sound { category: MobDamageSound::Hurt },
        MobDamageFeedbackComponent::Ragdoll,
    ]);
    s.pin("MobDamageSound::*", &vec![MobDamageSound::Hurt, MobDamageSound::Death]);
    s.pin("GuiValue::*", &vec![GuiValue::F32(1.0), GuiValue::I32(-1), GuiValue::Str("s".into())]);
    s.pin("RuntimeSide::*", &vec![RuntimeSide::Server, RuntimeSide::Worldgen, RuntimeSide::Client]);
    s.pin("ClientOverlayAnchor::*", &vec![
        ClientOverlayAnchor::TopLeft, ClientOverlayAnchor::TopRight,
        ClientOverlayAnchor::BottomLeft, ClientOverlayAnchor::BottomRight,
    ]);
    s.pin("ClientPointerPhase::*", &vec![
        ClientPointerPhase::Down, ClientPointerPhase::Move, ClientPointerPhase::Up,
    ]);
    s.pin("ClientPointerButton::*", &vec![
        ClientPointerButton::Primary, ClientPointerButton::Secondary,
    ]);
    s.pin("ClientCanvasElement::*", &vec![
        ClientCanvasElement::Image { image_key: "m:i".into(), rect: [1.0, 2.0, 3.0, 4.0] },
        ClientCanvasElement::Sprite { image_key: "m:i".into(), center: [1.0, 2.0] },
    ]);
    s.pin("ClientUiEvent::*", &vec![
        ClientUiEvent::Click { id: "b".into() },
        ClientUiEvent::TextChanged { id: "b".into(), text: "t".into() },
        ClientUiEvent::Submit { id: "b".into(), text: "t".into() },
        ClientUiEvent::ImagePointer {
            id: "b".into(), phase: ClientPointerPhase::Up, x: 1.0, y: 2.0,
            button: ClientPointerButton::Secondary,
        },
    ]);
    s.pin("BlockHookKind::*", &vec![
        BlockHookKind::RandomTick, BlockHookKind::ScheduledTick, BlockHookKind::NeighborUpdate,
    ]);

    s
}

/// The recorded wire bytes. Regenerate by running the test after a DELIBERATE
/// ABI change and pasting the block it prints (then `make mods`).
#[rustfmt::skip]
const PINS: &[(&str, &str)] = &[
    ("HostCall::Log", "000161"),
    ("HostCall::CurrentTick", "01"),
    ("HostCall::RngU64", "020173"),
    ("HostCall::RegisterTickSystem", "0300000101"),
    ("HostCall::RegisterEventHandler", "04000202"),
    ("HostCall::GetBlock", "05020306"),
    ("HostCall::GetBlocks", "0601000000"),
    ("HostCall::SetBlock", "0702040604"),
    ("HostCall::SetBlocks", "080102040605"),
    ("HostCall::ScheduleTick", "0902040607"),
    ("HostCall::IsLoaded", "0a020406"),
    ("HostCall::LightAt", "0b020406"),
    ("HostCall::SpawnMob", "0c036d3a6b0000803f00000040000040400000003f"),
    ("HostCall::MobsInRadius", "0d0000803f000000400000404000008040"),
    ("HostCall::DamageMob", "0e0100000040010000803f0000004000004040"),
    ("HostCall::DespawnMob", "0f02"),
    ("HostCall::SpawnItem", "10036d3a69030000803f0000004000004040"),
    ("HostCall::PlayerState", "11"),
    ("HostCall::DamagePlayer", "1204"),
    ("HostCall::ApplyKnockback", "130000803f0000004000004040"),
    ("HostCall::GiveItem", "14036d3a6902"),
    ("HostCall::KillPlayer", "15"),
    ("HostCall::SetHealth", "1628"),
    ("HostCall::Teleport", "170000803f0000004000004040"),
    ("HostCall::EmitSound", "18036d3a73010000803f0000004000004040"),
    ("HostCall::WorldKvGet", "19036d3a6b"),
    ("HostCall::WorldKvSet", "1a036d3a6b0101"),
    ("HostCall::WorldKvDelete", "1b036d3a6b"),
    ("HostCall::SectionKvGet", "1c020406036d3a6b"),
    ("HostCall::SectionKvSet", "1d020406036d3a6b0102"),
    ("HostCall::SectionKvDelete", "1e020406036d3a6b"),
    ("HostCall::MobKvGet", "1f01036d3a6b"),
    ("HostCall::MobKvSet", "2001036d3a6b0103"),
    ("HostCall::MobKvDelete", "2101036d3a6b"),
    ("HostCall::ResolveBlock", "22036d3a62"),
    ("HostCall::RegisterWorldgenFeature", "230104"),
    ("HostCall::RegisterStageReplacement", "240102"),
    ("HostCall::RegisterGenerator", "2503"),
    ("HostCall::GuiStateSet", "26016b0102"),
    ("HostCall::GuiStateGet", "27016b"),
    ("HostCall::GuiOpen", "28036d3a67"),
    ("HostCall::GuiClose", "29"),
    ("HostCall::ChatSend", "2a0174010101"),
    ("HostCall::SoundPlayAt", "2b036d3a730000803f00000040000040400000803f0000803f"),
    ("HostCall::SoundPlayOnMob", "2c01036d3a730000803f0000803f"),
    ("HostCall::SoundStop", "2d01"),
    ("HostCall::BlockIsFullSpawnSupport", "2e020406"),
    ("HostCall::ShaderSetParam", "2f036d3a70000000000000803e0000003f0000803f"),
    ("HostCall::RegisterHostileSpawner", "300104"),
    ("HostCall::RegisterBlockBehavior", "31036d3a6201"),
    ("HostCall::RegisterAiNode", "32036d3a6e02"),
    ("HostCall::ContainerGet", "33020406"),
    ("HostCall::ContainerSet", "34020406020001036d3a69010100"),
    ("HostCall::ItemInfo", "35036d3a69"),
    ("HostCall::RecipeResult", "36036d3a63036d3a69"),
    ("HostCall::EffectApply", "37036d3a6505"),
    ("HostCall::EffectRemove", "38036d3a65"),
    ("HostCall::EffectsActive", "39"),
    ("HostCall::SwapModelBlock", "3a02040606"),
    ("HostCall::ContainerGetMany", "3b01020406"),
    ("HostCall::MobEmitterSet", "3c01036d3a6501"),
    ("HostCall::EmitterBurst", "3d036d3a650000803f000000400000404000000040"),
    ("HostCall::RuntimeSide", "3e"),
    ("HostCall::ClientRegisterOverlay", "3f036d3a690001020304"),
    ("HostCall::ClientRegisterKey", "40086f70656e5f6d61700e4f70656e20576f726c64204d6170056b65795f6d01"),
    ("HostCall::ClientSurface", "41020403"),
    ("HostCall::ClientUiStateSet", "42036d3a6b020176"),
    ("HostCall::ClientUiStateGet", "43036d3a6b"),
    ("HostCall::ClientImageSet", "44036d3a6901010401020304"),
    ("HostCall::ClientTextMeasure", "45017402"),
    ("HostCall::ClientImageDrawTexts", "46036d3a6901017402040101020304"),
    ("HostCall::ClientGuiOpen", "47036d3a67"),
    ("HostCall::ClientGuiClose", "48"),
    ("HostCall::ClientCanvasOpen", "49036d3a630102"),
    ("HostCall::ClientCanvasClose", "4a"),
    ("HostCall::ClientCanvasSceneSet", "4b036d3a630200036d3a690000803f00000040000040400000804001036d3a690000803f00000040"),
    ("HostCall::ClientCanvasViewSet", "4c036d3a630000803f00000040"),
    ("HostCall::ClientStorageGetMany", "4d01036d3a6b"),
    ("HostCall::ClientStorageSetMany", "4e01036d3a6b0101"),
    ("HostRet::Unit", "00"),
    ("HostRet::U64", "0101"),
    ("HostRet::Error", "020165"),
    ("HostRet::Bool", "0301"),
    ("HostRet::Block", "040101"),
    ("HostRet::Blocks", "0502000102"),
    ("HostRet::Light", "06010203"),
    ("HostRet::Mobs", "070101036d3a6b0000803f00000040000040400000804005"),
    ("HostRet::Player", "080000803f00000040000040400000000000000000000000000000003f0000803e280100"),
    ("HostRet::Bytes", "09010101"),
    ("HostRet::GuiValue", "0a01000000803f"),
    ("HostRet::ContainerSlots", "0b010201036d3a690100"),
    ("HostRet::ItemInfo", "0c014000010174"),
    ("HostRet::ItemStack", "0d01036d3a6902"),
    ("HostRet::Effects", "0e01036d3a6509"),
    ("HostRet::Containers", "0f0201010000"),
    ("HostRet::RuntimeSide", "1002"),
    ("HostRet::ClientSurface", "1102000101010203"),
    ("HostRet::ClientTextSize", "120102"),
    ("HostRet::ClientStorageValues", "130200010101"),
    ("GuestCall::TickSystem", "0001"),
    ("GuestCall::HandleEvent", "01010c0c"),
    ("GuestCall::GenFeature", "020102040604020102010a01060e"),
    ("GuestCall::GenStage", "0301020204060401010104010308"),
    ("GuestCall::GuiClick", "04036d3a67017701020406"),
    ("GuestCall::HostileSpawnCandidate", "05010000803f0000004000004040020406010203"),
    ("GuestCall::BlockBehavior", "060100020406"),
    ("GuestCall::AiNode", "0701010000803f00000040000040400204060000003f000080400000a0400000c0400100"),
    ("GuestCall::ClientFrame", "08cdcc4c3d0000803f00000040000040400000003f0000803e8005e00301036d3a6700"),
    ("GuestCall::ClientKey", "090101"),
    ("GuestCall::ClientUi", "0a036d3a67000162"),
    ("GuestCall::ClientCanvas", "0b036d3a63000000803f0000004000"),
    ("GuestRet::Unit", "00"),
    ("GuestRet::Event", "01010801"),
    ("GuestRet::GenWrites", "020102040604"),
    ("GuestRet::GenBlocks", "03020102"),
    ("GuestRet::GenBiomes", "040103"),
    ("GuestRet::HostileSpawn", "0501036d3a6b"),
    ("GuestRet::AiDecision", "060101020406010000003f0000803e0101010000004000004040"),
    ("EventPayload::BlockPlacePre", "000204060100"),
    ("EventPayload::BlockBreakPre", "010204060101"),
    ("EventPayload::BlockInteract", "0202040601"),
    ("EventPayload::ItemUsePre", "030101020406"),
    ("EventPayload::MobDamagePre", "0401020000404000010000803f00000040000040400500010000003f020000803f0000003f030004"),
    ("EventPayload::PlayerDamagePre", "0502010100"),
    ("EventPayload::BlockPlaced", "0602040601"),
    ("EventPayload::BlockBroken", "07020406010001"),
    ("EventPayload::ItemUsed", "0802"),
    ("EventPayload::MobDied", "09010000803f0000004000004040"),
    ("EventPayload::MobSpawned", "0a010000803f0000004000004040"),
    ("EventPayload::PlayerDamaged", "0b0226"),
    ("EventPayload::PlayerDied", "0c"),
    ("EventPayload::ContainerOpened", "0d0301020406"),
    ("EventPayload::ContainerClosed", "0e05036d3a6700"),
    ("EventPayload::SectionGenerated", "0f020406"),
    ("EventPayload::SectionLoaded", "10020406"),
    ("Outcome::*", "020001"),
    ("Stage::*", "0c000102030405060708090a0b"),
    ("AttachSide::*", "020001"),
    ("WorldgenStage::*", "050001020304"),
    ("EventKind::*", "11000102030405060708090a0b0c0d0e0f10"),
    ("DamageSource::*", "0400010102036d3a6b03016d"),
    ("ContainerKind::*", "06000102030405036d3a67"),
    ("Facing::*", "0400010203"),
    ("MobDamageFeedbackComponent::*", "0500010000003f020000803f0000003f030004"),
    ("MobDamageSound::*", "020001"),
    ("GuiValue::*", "03000000803f0101020173"),
    ("RuntimeSide::*", "03000102"),
    ("ClientOverlayAnchor::*", "0400010203"),
    ("ClientPointerPhase::*", "03000102"),
    ("ClientPointerButton::*", "020001"),
    ("ClientCanvasElement::*", "0200036d3a690000803f00000040000040400000804001036d3a690000803f00000040"),
    ("ClientUiEvent::*", "0400016201016201740201620174030162020000803f0000004001"),
    ("BlockHookKind::*", "03000102"),
];

#[test]
fn wire_format_matches_the_recorded_pins() {
    let actual = samples().0;
    let matches = actual.len() == PINS.len()
        && actual
            .iter()
            .zip(PINS)
            .all(|((name, bytes), (pin_name, pin_bytes))| name == pin_name && bytes == pin_bytes);
    if matches {
        return;
    }
    let mut block = String::new();
    for (name, bytes) in &actual {
        block.push_str(&format!("    (\"{name}\", \"{bytes}\"),\n"));
    }
    for ((name, bytes), (pin_name, pin_bytes)) in actual.iter().zip(PINS) {
        if name != pin_name {
            eprintln!("first divergence: sample '{name}' where the pin has '{pin_name}'");
            break;
        }
        if bytes != pin_bytes {
            eprintln!("first divergence: '{name}' encodes {bytes}, pinned {pin_bytes}");
            break;
        }
    }
    panic!(
        "the ABI wire format no longer matches its recorded pins \
         ({} samples, {} pins).\n\
         If this change is DELIBERATE (pre-release reshapes are allowed), replace the \
         body of PINS in mod-api/src/wire_pin.rs with:\n\nconst PINS: &[(&str, &str)] = &[\n{block}];\n\n\
         and rebuild the mods (`make mods`). If it is NOT deliberate, a refactor just \
         reordered or reshaped ABI types — fix the refactor instead.",
        samples().0.len(),
        PINS.len(),
    );
}
