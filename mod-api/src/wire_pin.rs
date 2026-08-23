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
        key: "m:k".into(), pos: [1.0, 2.0, 3.0], yaw: 0.5, checked: true,
    });
    s.pin("HostCall::MobsInRadius", &HostCall::MobsInRadius { pos: [1.0, 2.0, 3.0], radius: 4.0 });
    s.pin("HostCall::DamageMob", &HostCall::DamageMob {
        mob_id: 7, amount: 2.0, origin: Some([1.0, 2.0, 3.0]),
        feedback: Some(crate::events::MobDamageFeedback {
            components: vec![
                crate::events::MobDamageFeedbackComponent::DecreaseHealth,
                crate::events::MobDamageFeedbackComponent::Immunity { ticks: 10 },
            ],
        }),
    });
    s.pin("HostCall::DespawnMob", &HostCall::DespawnMob { mob_id: 7 });
    s.pin("HostCall::SpawnItem", &HostCall::SpawnItem {
        item: "m:i".into(), count: 3, pos: [1.0, 2.0, 3.0],
        data: Vec::new(),
    });
    s.pin("HostCall::PlayerState", &HostCall::PlayerState);
    s.pin("HostCall::DamagePlayer", &HostCall::DamagePlayer { amount: 2 });
    s.pin("HostCall::ApplyKnockback", &HostCall::ApplyKnockback { impulse: [1.0, 2.0, 3.0] });
    s.pin("HostCall::GiveItem", &HostCall::GiveItem {
        item: "m:i".into(),
        count: 2,
        data: vec![("m:k".into(), vec![1, 2, 3])],
    });
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
    s.pin("HostCall::MobTagGet", &HostCall::MobTagGet { mob_id: 7, key: "m:k".into() });
    s.pin("HostCall::MobTagSet", &HostCall::MobTagSet {
        mob_id: 7, key: "m:k".into(), value: MobTagValue::I64(-3),
    });
    s.pin("HostCall::MobTagDelete", &HostCall::MobTagDelete { mob_id: 7, key: "m:k".into() });
    s.pin("HostCall::ResolveBlock", &HostCall::ResolveBlock { name: "m:b".into() });
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
    s.pin("HostCall::ChatSend", &HostCall::ChatSend {
        text: "t".into(), targets: Some(vec![PlayerId(1)]),
    });
    s.pin("HostCall::SoundPlayAt", &HostCall::SoundPlayAt {
        key: "m:s".into(), pos: [1.0, 2.0, 3.0], volume: 1.0, pitch: 1.0,
    });
    s.pin("HostCall::SoundPlayOnMob", &HostCall::SoundPlayOnMob {
        mob_id: 1, key: "m:s".into(), volume: 1.0, pitch: 1.0,
    });
    s.pin("HostCall::SoundStop", &HostCall::SoundStop { handle: 1 });
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
        slots: vec![(0, Some(ItemStackData { item: "m:i".into(), count: 1, data: Vec::new() })), (1, None)],
    });
    s.pin("HostCall::ItemInfo", &HostCall::ItemInfo { item: "m:i".into() });
    s.pin("HostCall::RecipeResult", &HostCall::RecipeResult {
        class: "m:c".into(), item: "m:i".into(),
    });
    s.pin("HostCall::EffectApply", &HostCall::EffectApply { key: "m:e".into(), ticks: 5 });
    s.pin("HostCall::EffectsActive", &HostCall::EffectsActive);
    s.pin("HostCall::SwapModelBlock", &HostCall::SwapModelBlock {
        pos: [1, 2, 3], block: BlockId(6),
    });
    s.pin("HostCall::ContainerGetMany", &HostCall::ContainerGetMany {
        positions: vec![[1, 2, 3]],
    });
    s.pin("HostCall::MobEmitterSet", &HostCall::MobEmitterSet {
        mob_id: 7, key: "m:e".into(), active: true,
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
    s.pin("HostCall::ClientSurfaceColumns", &HostCall::ClientSurfaceColumns {
        queries: vec![ClientSurfaceQuery { coord: [1, -2], revision: 3 }],
    });
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
        entries: vec![("m:k".into(), ByteBuf::from(vec![1]))],
    });
    s.pin("HostCall::ResolveItem", &HostCall::ResolveItem { name: "m:i".into() });
    s.pin("HostCall::ClientImageBlit", &HostCall::ClientImageBlit {
        key: "m:i".into(), origin: [1, 2], size: [1, 1], rgba: vec![1, 2, 3, 4],
    });
    s.pin("HostCall::ClientStorageReadBegin", &HostCall::ClientStorageReadBegin {
        keys: vec!["m:k".into()],
    });
    s.pin("HostCall::ClientStorageReadPoll", &HostCall::ClientStorageReadPoll { ticket: 7 });
    s.pin("HostCall::ConsumeHeld", &HostCall::ConsumeHeld { item: ItemId(3), count: 1 });
    s.pin("HostCall::ReplaceHeldOne", &HostCall::ReplaceHeldOne { item: ItemId(3), replacement: "m:i".into() });
    s.pin("HostCall::MobMount", &HostCall::MobMount {
        mob_id: 7, player_id: PlayerId(1), seat: 0,
    });
    s.pin("HostCall::MobDismount", &HostCall::MobDismount { player_id: PlayerId(1) });
    s.pin("HostCall::MobRiders", &HostCall::MobRiders { mob_id: 7 });
    s.pin("HostCall::MobDrive", &HostCall::MobDrive {
        mob_id: 7, horizontal: Some([1.0, 2.0]), vertical: Some(4.5), yaw: Some(0.5), while_walking: true,
    });
    s.pin("HostCall::MobAnimSet", &HostCall::MobAnimSet {
        mob_id: 7, anim: "row".into(), active: true,
    });
    s.pin("HostCall::MobAnimRate", &HostCall::MobAnimRate {
        mob_id: 7, anim: "row".into(), rate: -1.0,
    });
    s.pin("HostCall::MobAnimSeek", &HostCall::MobAnimSeek {
        mob_id: 7, anim: "row".into(), phase: 1.5, rate: 0.75,
    });
    s.pin("HostCall::PlayerInput", &HostCall::PlayerInput { player_id: PlayerId(1) });
    s.pin("HostCall::MobAnimState", &HostCall::MobAnimState {
        mob_id: 7, anim: "row".into(),
    });
    s.pin("HostCall::BiomeAt", &HostCall::BiomeAt { pos: [1, -2] });
    s.pin("HostCall::SurfaceYAt", &HostCall::SurfaceYAt { pos: [1, -2] });
    s.pin("HostCall::Players", &HostCall::Players);
    s.pin("HostCall::ClientEnvParams", &HostCall::ClientEnvParams {
        keys: vec!["m:k".into()],
    });
    s.pin("HostCall::ClientBiomeAt", &HostCall::ClientBiomeAt { pos: [1, -2] });
    s.pin("HostCall::ClientAmbientSet", &HostCall::ClientAmbientSet {
        key: "m:rain".into(), intensity: 0.5, wind: [1.0, -2.0],
    });
    s.pin("HostCall::ClientLoopSet", &HostCall::ClientLoopSet {
        key: "m:loop".into(), gain: 0.5,
    });
    s.pin("HostCall::ClientMoodSet", &HostCall::ClientMoodSet {
        darken: 0.25, desaturate: 0.5,
    });
    s.pin("HostCall::ClientBlocksAt", &HostCall::ClientBlocksAt {
        positions: vec![[1, -2, 3]],
    });
    s.pin("HostCall::BlocksByTag", &HostCall::BlocksByTag { tag: "m:t".into() });
    s.pin("HostCall::ItemsByTag", &HostCall::ItemsByTag { tag: "m:t".into() });
    s.pin("HostCall::BlockNames", &HostCall::BlockNames { blocks: vec![BlockId(1), BlockId(9)] });
    s.pin("HostCall::ItemNames", &HostCall::ItemNames { items: vec![ItemId(1), ItemId(9)] });
    s.pin("HostCall::ResolveMob", &HostCall::ResolveMob { key: "m:k".into() });
    s.pin("HostCall::MobNames", &HostCall::MobNames { mobs: vec![MobId(1), MobId(9)] });
    s.pin("HostCall::CollisionShapeAt", &HostCall::CollisionShapeAt { pos: [1, 2, 3] });
    s.pin("HostCall::MobTagsGet", &HostCall::MobTagsGet { mob_id: 7 });
    s.pin("HostCall::MobsWithTag", &HostCall::MobsWithTag {
        key: "m:k".into(), value: Some(MobTagValue::I64(-3)),
    });
    s.pin("HostCall::FindBlocks", &HostCall::FindBlocks {
        min: [-1, 2, -3], max: [4, 5, 6], blocks: vec![BlockId(1), BlockId(9)],
    });
    s.pin("HostCall::MobInfo", &HostCall::MobInfo { mob_id: 7 });
    s.pin("HostCall::MobCanReach", &HostCall::MobCanReach { mob_id: 7, cell: [1, -2, 3] });
    s.pin("HostCall::ResolveShape", &HostCall::ResolveShape { key: "m:s".into() });
    s.pin("HostCall::PlayerPoseSet", &HostCall::PlayerPoseSet {
        player_id: PlayerId(1), anchor: [1.5, 2.0, -3.5], yaw: 0.5, pose: pose::SITTING,
    });
    s.pin("HostCall::BlockModelGroup", &HostCall::BlockModelGroup { pos: [1, 2, 3] });
    s.pin("HostCall::ClientCellKvAt", &HostCall::ClientCellKvAt {
        key: "m:k".into(), cells: vec![[1, -2, 3]],
    });
    s.pin("HostCall::ItemDataGet", &HostCall::ItemDataGet { item: ItemId(3), key: "m:k".into() });
    s.pin("HostCall::ItemsWithData", &HostCall::ItemsWithData { key: "m:k".into() });
    s.pin("HostCall::BlockDataGet", &HostCall::BlockDataGet { block: BlockId(4), key: "m:k".into() });
    s.pin("HostCall::BlocksWithData", &HostCall::BlocksWithData { key: "m:k".into() });
    s.pin("HostCall::ResolveUndergroundBiome", &HostCall::ResolveUndergroundBiome { key: "m:u".into() });
    s.pin("HostCall::UndergroundBiomeAt", &HostCall::UndergroundBiomeAt {
        positions: vec![[1, -2, 3]],
    });
    s.pin("HostCall::TerrainSolidAt", &HostCall::TerrainSolidAt {
        positions: vec![[1, -2, 3]],
    });
    s.pin("HostCall::UndergroundBiomesInBox", &HostCall::UndergroundBiomesInBox {
        lo: [1, -2, 3], hi: [4, 5, -6],
    });
    s.pin("HostCall::UnlockRecipe", &HostCall::UnlockRecipe {
        player: PlayerId(1), recipe: "m:r".into(),
    });
    s.pin("HostCall::RecipeUnlocked", &HostCall::RecipeUnlocked {
        player: PlayerId(1), recipe: "m:r".into(),
    });
    s.pin("HostCall::EmitEvent", &HostCall::EmitEvent {
        key: "m:e".into(), data: vec![1, 2],
    });
    s.pin("HostCall::SurfaceBiomeAt", &HostCall::SurfaceBiomeAt {
        columns: vec![[1, -2]],
    });
    s.pin("HostCall::SetModelParts", &HostCall::SetModelParts {
        pos: [1, 2, 3], parts: 5, tint: Some([9, 8, 7]),
    });
    s.pin("HostCall::SetBlockDraw", &HostCall::SetBlockDraw {
        pos: [1, 2, 3],
        prims: vec![
            crate::DrawPrim::Cuboid {
                min: [0.0, 0.25, 0.0], max: [1.0, 0.5, 1.0],
                tile: "stone".into(), tint: [9, 8, 7], emissive: true,
            },
            crate::DrawPrim::Item {
                at: [0.5, 0.5, 0.5], scale: 0.5, yaw: 1.5, pitch: 0.25,
                item: "m:i".into(), tint: [1, 2, 3],
            },
        ],
    });
    s.pin("HostCall::BlockLocalToWorld", &HostCall::BlockLocalToWorld {
        pos: [1, 2, 3], points: vec![[0.25, 0.5, 0.75]],
    });
    s.pin("HostCall::SetBlockDraws", &HostCall::SetBlockDraws {
        sets: vec![([1, 2, 3], vec![crate::DrawPrim::Cuboid {
            min: [0.0, 0.25, 0.0], max: [1.0, 0.5, 1.0],
            tile: "stone".into(), tint: [9, 8, 7], emissive: true,
        }])],
    });
    s.pin("HostCall::SetModelPartsMany", &HostCall::SetModelPartsMany {
        sets: vec![([1, 2, 3], 5, Some([9, 8, 7]))],
    });
    s.pin("HostCall::SectionKvGetMany", &HostCall::SectionKvGetMany {
        key: "m:k".into(), positions: vec![[1, 2, 3]],
    });
    s.pin("HostCall::SectionKvSetMany", &HostCall::SectionKvSetMany {
        key: "m:k".into(), writes: vec![([1, 2, 3], Some(vec![7])), ([4, 5, 6], None)],
    });
    s.pin("HostCall::GuiViewers", &HostCall::GuiViewers);
    s.pin("HostCall::GuiStateSetFor", &HostCall::GuiStateSetFor {
        player_id: PlayerId(2), key: "k".into(), value: GuiValue::F32(0.5),
    });
    s.pin("HostCall::BlockInfo", &HostCall::BlockInfo { block: BlockId(300) });
    s.pin("HostCall::PlayerHeld", &HostCall::PlayerHeld { player: PlayerId(2) });
    s.pin(
        "HostCall::GiveItemTo",
        &HostCall::GiveItemTo {
            player: PlayerId(2),
            item: "i".into(),
            count: 3,
            data: vec![("k".into(), vec![7])],
        },
    );
    s.pin(
        "HostCall::SetPlayerHeldData",
        &HostCall::SetPlayerHeldData {
            player: PlayerId(2),
            expect_item: "i".into(),
            expect_data: vec![("k".into(), vec![6])],
            data: vec![("k".into(), vec![7])],
        },
    );
    s.pin("HostCall::SiteOpen", &HostCall::SiteOpen { key: "m:k".into(), cell: [1, -2, 3] });
    s.pin("HostCall::SetPlayerSpeedScale", &HostCall::SetPlayerSpeedScale {
        player: PlayerId(3), scale: 1.5,
    });
    s.pin("HostCall::SetPlayerBonePose", &HostCall::SetPlayerBonePose {
        player: PlayerId(5),
        bones: vec![crate::BonePoseData {
            bone: "left_shoulder".into(), rotation: [-22.0, 0.0, 0.0], translation: [0.0, 1.0, -2.0],
            mode: crate::BonePoseMode::Replace,
        }],
    });
    s.pin("HostCall::SetPlayerHeldPose", &HostCall::SetPlayerHeldPose {
        player: PlayerId(4),
        main: Some(crate::HeldPose {
            first_person: crate::HeldPoseData { rotation: [0.0, 1.5, 0.0], translation: [0.1, -0.2, -0.3] },
            third_person: crate::HeldPoseData { rotation: [0.5; 3], translation: [0.4; 3] },
        }),
        off: None,
    });
    s.pin("HostCall::EmitEventTo", &HostCall::EmitEventTo {
        player: PlayerId(6), key: "m:e".into(), data: vec![1, 2],
    });
    s.pin("HostCall::SetPlayerDeniedActions", &HostCall::SetPlayerDeniedActions {
        player: PlayerId(7), actions: vec![BodyAction::Attack, BodyAction::Mine],
    });
    s.pin("HostCall::HoldUse", &HostCall::HoldUse { player: PlayerId(8) });

    // --- HostRet: every variant, declaration order --------------------------
    s.pin("HostRet::Unit", &HostRet::Unit);
    s.pin("HostRet::U64", &HostRet::U64(1));
    s.pin("HostRet::Error", &HostRet::Error("e".into()));
    s.pin("HostRet::Bool", &HostRet::Bool(true));
    s.pin("HostRet::Block", &HostRet::Block(Some(BlockId(1))));
    s.pin("HostRet::Blocks", &HostRet::Blocks(vec![None, Some(BlockId(2))]));
    s.pin("HostRet::Light", &HostRet::Light(Some(LightData { combined: 1, sky: 2, block: 3, block_rgb: [3, 2, 1] })));
    s.pin("HostRet::Mobs", &HostRet::Mobs(vec![MobSnapshot {
        index: 1, kind: MobId(2), pos: [1.0, 2.0, 3.0], health: 4.0, id: 5,
        yaw: 0.5, vel: [1.0, 0.0, 2.0], on_ground: true, moving: false,
    }]));
    s.pin("HostRet::Player", &HostRet::Player(PlayerSnapshot {
        id: Some(PlayerId(1)),
        pos: [1.0, 2.0, 3.0], vel: [0.0, 0.0, 0.0], yaw: 0.5, pitch: 0.25,
        health: 20, on_ground: true, spectator: false, sneak: true, held: Some(ItemId(2)), held_count: 3,
        off_held: None, use_held: false, holds_use: false,
        pose_anchor: Some([1.5, 2.0, -3.5]),
    }));
    s.pin("HostRet::Bytes", &HostRet::Bytes(Some(vec![1])));
    s.pin("HostRet::MobTag", &HostRet::MobTag(MobTagLookup::Value(MobTagValue::Bool(true))));
    s.pin("HostRet::GuiValue", &HostRet::GuiValue(Some(GuiValue::F32(1.0))));
    s.pin("HostRet::ContainerSlots", &HostRet::ContainerSlots(Some(vec![
        Some(ItemStackData { item: "m:i".into(), count: 1, data: Vec::new() }), None,
    ])));
    s.pin("HostRet::ItemInfo", &HostRet::ItemInfo(Some(Box::new(ItemInfoData {
        max_stack: 64, fuel_burn_ticks: 0, tags: vec!["t".into()],
        display_name: "N".into(), block: Some(BlockId(2)),
        tool: Some(ToolInfoData {
            kind: "pickaxe".into(),
            tier: 1,
            speed: 2.0,
            damage: [1.0, 1.5],
        }),
        food: Some(FoodInfoData {
            eat_ticks: 60,
            effects: vec![FoodEffectData { effect: "m:e".into(), ticks: 100 }],
        }),
        item_use: Some("bucket_fill".into()),
    }))));
    s.pin("HostRet::ItemStack", &HostRet::ItemStack(Some(ItemStackData {
        item: "m:i".into(), count: 2, data: Vec::new(),
    })));
    s.pin("HostRet::Effects", &HostRet::Effects(vec![EffectStateData {
        key: "m:e".into(), remaining: 9,
    }]));
    s.pin("HostRet::Containers", &HostRet::Containers(vec![Some(vec![None]), None]));
    s.pin("HostRet::RuntimeSide", &HostRet::RuntimeSide(RuntimeSide::Client));
    s.pin("HostRet::ClientSurfaceColumns", &HostRet::ClientSurfaceColumns(vec![
        None,
        Some(ClientSurfaceColumn { revision: 2, cells: None }),
        Some(ClientSurfaceColumn { revision: 3, cells: Some(vec![255, 127, 1, 2, 3]) }),
    ]));
    s.pin("HostRet::ClientTextSize", &HostRet::ClientTextSize([1, 2]));
    s.pin("HostRet::ClientStorageValues", &HostRet::ClientStorageValues(vec![None, Some(ByteBuf::from(vec![1]))]));
    s.pin("HostRet::Item", &HostRet::Item(Some(ItemId(1))));
    s.pin("HostRet::ClientStorageRead", &HostRet::ClientStorageRead(Some(vec![None, Some(ByteBuf::from(vec![1]))])));
    s.pin("HostRet::Riders", &HostRet::Riders(Some(MobRidersData {
        capacity: 2, riders: vec![MobRiderData { seat: 0, player_id: PlayerId(1) }],
    })));
    s.pin("HostRet::ModelGroup", &HostRet::ModelGroup(Some(ModelGroupData {
        base: [1, -2, 3], facing: Facing::East,
    })));
    s.pin("HostRet::PlayerInput", &HostRet::PlayerInput(Some(PlayerInputData {
        forward: 1.0, strafe: -1.0, jump: true, sneak: false, yaw: 0.5, pitch: 0.25,
    })));
    s.pin("HostRet::MobAnimState", &HostRet::MobAnimState(Some(MobAnimStateData {
        phase: 1.5, rate: 0.75, seek: Some(2.0),
    })));
    s.pin("HostRet::MaybeByte", &HostRet::MaybeByte(Some(4)));
    s.pin("HostRet::MaybeI32", &HostRet::MaybeI32(Some(-7)));
    s.pin("HostRet::Players", &HostRet::Players(vec![PlayerListEntry {
        id: PlayerId(1),
        state: PlayerSnapshot {
            id: Some(PlayerId(1)),
            pos: [1.0, 2.0, 3.0], vel: [0.0, 0.0, 0.0], yaw: 0.5, pitch: 0.25,
            health: 20, on_ground: true, spectator: false, sneak: false, held: None, held_count: 0,
            off_held: Some(ItemId(5)), use_held: true, holds_use: false,
            pose_anchor: None,
        },
    }]));
    s.pin("HostRet::EnvParams", &HostRet::EnvParams(vec![None, Some([1.0, 2.0, 3.0, 4.0])]));
    s.pin("HostRet::BlockList", &HostRet::BlockList(vec![BlockId(1), BlockId(9)]));
    s.pin("HostRet::ItemList", &HostRet::ItemList(vec![ItemId(1), ItemId(9)]));
    s.pin("HostRet::Names", &HostRet::Names(vec![None, Some("m:b".into())]));
    s.pin("HostRet::MobKind", &HostRet::MobKind(Some(MobId(1))));
    s.pin("HostRet::CollisionShape", &HostRet::CollisionShape(Some(CollisionShape::Full)));
    s.pin("HostRet::MobTags", &HostRet::MobTags(Some(vec![
        ("m:k".into(), MobTagValue::Bool(true)),
    ])));
    s.pin("HostRet::SpawnedMob", &HostRet::SpawnedMob(Some(7)));
    s.pin("HostRet::FoundBlocks", &HostRet::FoundBlocks(Some(vec![[1, -2, 3]])));
    s.pin("HostRet::Mob", &HostRet::Mob(Some(MobSnapshot {
        index: 1, kind: MobId(2), pos: [1.0, 2.0, 3.0], health: 4.0, id: 5,
        yaw: 0.5, vel: [1.0, 0.0, 2.0], on_ground: true, moving: false,
    })));
    s.pin("HostRet::BytesMany", &HostRet::BytesMany(vec![Some(vec![1, 2]), None]));
    s.pin("HostRet::ItemDataRows", &HostRet::ItemDataRows(vec![(ItemId(3), "{}".into())]));
    s.pin("HostRet::BlockDataRows", &HostRet::BlockDataRows(vec![(BlockId(4), "{}".into())]));
    s.pin("HostRet::UndergroundBiomes", &HostRet::UndergroundBiomes(vec![0, 2]));
    s.pin("HostRet::TerrainSolid", &HostRet::TerrainSolid(vec![true, false]));
    s.pin("HostRet::SurfaceBiomes", &HostRet::SurfaceBiomes(vec![3, 6]));
    s.pin("HostRet::Points", &HostRet::Points(Some(vec![[1.5, 2.5, 3.5]])));
    s.pin("HostRet::Bools", &HostRet::Bools(vec![true, false]));
    s.pin("HostRet::GuiViewers", &HostRet::GuiViewers(vec![GuiViewerData {
        player_id: PlayerId(2), kind: "m:g".into(), anchor: Some([1, 2, 3]),
    }]));
    s.pin("HostRet::BlockInfo", &HostRet::BlockInfo(Some(Box::new(BlockInfoData {
        material: "stone".into(), hardness: 1.5, harvest_tier: 1,
        preferred_tool: Some("pickaxe".into()), item: Some(ItemId(300)),
    }))));
    s.pin("HostRet::HeldStack", &HostRet::HeldStack(Some(ItemStackData {
        item: "m:i".into(), count: 1, data: vec![("m:k".into(), vec![7])],
    })));

    // --- GuestCall: every variant, declaration order -------------------------
    s.pin("GuestCall::TickSystem", &GuestCall::TickSystem { id: 1 });
    s.pin("GuestCall::HandleEvent", &GuestCall::HandleEvent {
        id: 1, payload: EventPayload::PlayerDied,
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
            nearest_player_dist: 40.0,
        },
    });
    s.pin("GuestCall::BlockBehavior", &GuestCall::BlockBehavior {
        callback_id: 1, kind: BlockHookKind::RandomTick, pos: [1, 2, 3],
    });
    s.pin("GuestCall::AiNode", &GuestCall::AiNode {
        callback_id: 1,
        ctx: AiNodeCtx {
            mob_id: 1, pos: [1.0, 2.0, 3.0], cell: [1, 2, 3], yaw: 0.5,
            tick: 9, player_id: PlayerId(2),
            player_pos: [4.0, 5.0, 6.0], nav_idle: true, in_water: false,
            player_held: Some(ItemId(7)), player_foothold: Some([4, 5, 6]),
            tags: vec![("m:k".into(), MobTagValue::I64(-3))],
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
    s.pin("GuestCall::ClientCanvasScroll", &GuestCall::ClientCanvasScroll {
        canvas_key: "m:c".into(), x: 1.0, y: 2.0, delta: -1.0,
    });
    s.pin("GuestCall::BakeShapeSim", &GuestCall::BakeShapeSim {
        shape_kind: 1,
        cells: vec![CellInput {
            world_pos: [1, 2, 3], block_id: BlockId(4),
            neighbor_ids: [BlockId(0); 6],
            state: Some(vec![7]),
            neighbor_states: [None, Some(vec![9]), None, None, None, None],
        }],
    });
    s.pin("GuestCall::BakeShapeRender", &GuestCall::BakeShapeRender {
        shape_kind: 1,
        cells: vec![CellInput {
            world_pos: [1, 2, 3], block_id: BlockId(4),
            neighbor_ids: [BlockId(0); 6],
            state: Some(vec![7]),
            neighbor_states: [None, Some(vec![9]), None, None, None, None],
        }],
    });
    s.pin("GuestCall::BakeShapeItem", &GuestCall::BakeShapeItem { shape_kind: 1, block_id: BlockId(4) });
    s.pin("GuestCall::ShapePlacementPlan", &GuestCall::ShapePlacementPlan {
        shape_kind: 1, block_id: BlockId(4),
        inputs: PlaceInputsView { hit: [0, 0, 0], normal: [0, 1, 0], place_pos: [0, 1, 0], player_facing: 0 },
    });

    // --- GuestRet: every variant, declaration order --------------------------
    s.pin("GuestRet::Unit", &GuestRet::Unit);
    s.pin("GuestRet::Event", &GuestRet::Event {
        outcome: Outcome::Cancel, payload: EventPayload::ItemUsed { player: PlayerId(1), item: ItemId(1), kind: ItemUseEvent::Handler },
    });
    s.pin("GuestRet::GenWrites", &GuestRet::GenWrites(vec![([1, 2, 3], BlockId(4))]));
    s.pin("GuestRet::GenBlocks", &GuestRet::GenBlocks(vec![1, 2]));
    s.pin("GuestRet::GenBiomes", &GuestRet::GenBiomes(vec![3]));
    s.pin("GuestRet::HostileSpawn", &GuestRet::HostileSpawn(Some("m:k".into())));
    s.pin("GuestRet::AiDecision", &GuestRet::AiDecision(Some(AiNodeDecision {
        goal: Some([1, 2, 3]), head_look: Some([0.5, 0.25]),
        idle_anim: Some(1), attack: Some([2.0, 3.0]),
        tags: vec![MobTagWrite { key: "m:k".into(), value: Some(MobTagValue::Bool(true)) }],
    })));
    // Registry ids are TWO bytes, and postcard varint-encodes them: any sample
    // below 128 encodes byte-for-byte like the one-byte ids used to, so ONLY a
    // high id pins the width. Without these, silently narrowing `BlockId` /
    // `ItemId` back to `u8` would pass every other pin in this file.
    s.pin("BlockId (wide)", &BlockId(300));
    s.pin("ItemId (wide)", &ItemId(4095));
    s.pin("GuestRet::GenWrites (wide)", &GuestRet::GenWrites(vec![([1, 2, 3], BlockId(300))]));
    s.pin("GuestRet::GenBlocks (wide)", &GuestRet::GenBlocks(vec![1, 300]));

    s.pin("GuestRet::BakedSim", &GuestRet::BakedSim(vec![BakedSimCell {
        collision_boxes: vec![ShapeAabb { min: [0.0, 0.0, 0.0], max: [1.0, 1.0, 1.0] }],
        light_aperture: LightAperture::Open,
    }]));
    s.pin("GuestRet::BakedRender", &GuestRet::BakedRender(vec![BakedRenderCell {
        boxes: vec![ShapeRenderBox {
            aabb: ShapeAabb { min: [0.0, 0.0, 0.0], max: [1.0, 1.0, 1.0] },
            tint: Some([200, 30, 40]),
            ao: Some(30),
            dyed: true,
        }],
    }]));
    s.pin("GuestRet::BakedItem", &GuestRet::BakedItem(BakedItemGeometry { boxes: vec![] }));
    s.pin("GuestRet::ShapePlacement", &GuestRet::ShapePlacement(ShapePlacementResult {
        accepted: true, anchor: [0, 1, 0], cells: vec![[0, 1, 0]], block: Some(BlockId(2)),
    }));

    // --- EventPayload: every variant, declaration order ----------------------
    s.pin("EventPayload::BlockPlacePre", &EventPayload::BlockPlacePre {
        pos: [1, 2, 3], block: BlockId(1), facing: Facing::North,
    });
    s.pin("EventPayload::BlockBreakPre", &EventPayload::BlockBreakPre {
        pos: [1, 2, 3], block: BlockId(1), harvested: true, player: PlayerId(2),
        drops: Some(vec![ItemStackData {
            item: "m:i".into(), count: 1, data: vec![("m:k".into(), vec![7])],
        }]),
    });
    s.pin("EventPayload::InteractAttempt", &EventPayload::InteractAttempt {
        block: Some([1, 2, 3]), face: Some([0, 1, 0]), mob: Some(7), player: PlayerId(0),
    });
    s.pin("EventPayload::UseUnclaimed", &EventPayload::UseUnclaimed {
        block: Some([1, 2, 3]), face: Some([0, 1, 0]), mob: Some(7), player: PlayerId(0),
    });
    s.pin("EventPayload::ItemUsePre", &EventPayload::ItemUsePre {
        item: ItemId(1), target: Some([1, 2, 3]),
    });
    s.pin("EventPayload::MobDamagePre", &EventPayload::MobDamagePre {
        mob_id: 7, kind: MobId(2), amount: 3.0, source: DamageSource::Fall,
        origin: Some([1.0, 2.0, 3.0]),
        feedback: MobDamageFeedback {
            components: vec![
                MobDamageFeedbackComponent::DecreaseHealth,
                MobDamageFeedbackComponent::Flash { duration: 0.5 },
                MobDamageFeedbackComponent::Knockback { scale: 1.0, duration: 0.5 },
                MobDamageFeedbackComponent::Sound { category: MobDamageSound::Hurt },
                MobDamageFeedbackComponent::Ragdoll,
                MobDamageFeedbackComponent::Immunity { ticks: 10 },
            ],
        },
    });
    s.pin("EventPayload::PlayerDamagePre", &EventPayload::PlayerDamagePre {
        amount: 1, source: DamageSource::PlayerAttack { id: PlayerId(1) }, origin: None,
    });
    s.pin("EventPayload::BlockPlaced", &EventPayload::BlockPlaced {
        pos: [1, 2, 3], block: BlockId(1),
    });
    s.pin("EventPayload::BlockBroken", &EventPayload::BlockBroken {
        pos: [1, 2, 3], block: BlockId(1), harvested: false, natural: true,
    });
    s.pin("EventPayload::ItemUsed", &EventPayload::ItemUsed {
        player: PlayerId(2), item: ItemId(2), kind: ItemUseEvent::Eaten,
    });
    s.pin("ItemUseEvent::*", &[ItemUseEvent::Eaten, ItemUseEvent::Handler, ItemUseEvent::Claimed]);
    s.pin("EventPayload::MobDied", &EventPayload::MobDied {
        id: 7, kind: MobId(1), pos: [1.0, 2.0, 3.0],
    });
    s.pin("EventPayload::MobSpawned", &EventPayload::MobSpawned {
        id: 7, kind: MobId(1), pos: [1.0, 2.0, 3.0],
    });
    s.pin("EventPayload::PlayerDamaged", &EventPayload::PlayerDamaged {
        amount: 1, new_health: 19,
    });
    s.pin("EventPayload::PlayerDied", &EventPayload::PlayerDied);
    s.pin("EventPayload::ContainerOpened", &EventPayload::ContainerOpened {
        kind: ContainerKind::new("petramond:chest"), pos: Some([1, 2, 3]),
    });
    s.pin("EventPayload::ContainerClosed", &EventPayload::ContainerClosed {
        kind: ContainerKind::new("m:g"), pos: None,
    });
    s.pin("EventPayload::SectionGenerated", &EventPayload::SectionGenerated { pos: [1, 2, 3] });
    s.pin("EventPayload::SectionLoaded", &EventPayload::SectionLoaded { pos: [1, 2, 3] });
    s.pin("EventPayload::PlayerDismounted", &EventPayload::PlayerDismounted {
        player_id: PlayerId(0), mount: MountTarget::Mob(7),
    });
    s.pin("EventPayload::PlayerDismounted(anchor)", &EventPayload::PlayerDismounted {
        player_id: PlayerId(0), mount: MountTarget::Anchor([1.5, 2.0, -3.5]),
    });
    s.pin("EventPayload::MobTagAdded", &EventPayload::MobTagAdded {
        mob_id: 7, kind: MobId(2), key: "m:k".into(), value: MobTagValue::I64(-3),
    });
    s.pin("EventPayload::MobTagRemoved", &EventPayload::MobTagRemoved {
        mob_id: 7, kind: MobId(2), key: "m:k".into(), value: MobTagValue::I64(-3),
    });
    s.pin("EventPayload::ItemPickedUp", &EventPayload::ItemPickedUp {
        player: PlayerId(1), item: ItemId(3), count: 4, pos: [1.5, 2.0, -3.5],
    });
    s.pin("EventPayload::ItemObtained", &EventPayload::ItemObtained {
        player: PlayerId(1), item: ItemId(3),
    });
    s.pin("EventPayload::MobDamaged", &EventPayload::MobDamaged {
        mob_id: 7, kind: MobId(2), amount: 1.5, source: DamageSource::Fall, killed: true,
    });
    s.pin("EventPayload::Interacted", &EventPayload::Interacted {
        block: Some([1, -2, 3]), face: Some([0, 1, 0]), mob: None,
        player: PlayerId(1), consumed: true,
    });
    s.pin("EventPayload::ModEvent", &EventPayload::ModEvent {
        key: "m:e".into(), data: vec![1, 2],
    });

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
        EventKind::BlockPlacePre, EventKind::BlockBreakPre, EventKind::InteractAttempt,
        EventKind::ItemUsePre, EventKind::MobDamagePre, EventKind::PlayerDamagePre,
        EventKind::BlockPlaced, EventKind::BlockBroken, EventKind::ItemUsed,
        EventKind::MobDied, EventKind::MobSpawned, EventKind::PlayerDamaged,
        EventKind::PlayerDied, EventKind::ContainerOpened, EventKind::ContainerClosed,
        EventKind::SectionGenerated, EventKind::SectionLoaded,
        EventKind::PlayerDismounted, EventKind::MobTagAdded, EventKind::MobTagRemoved,
        EventKind::ItemPickedUp, EventKind::ItemObtained, EventKind::MobDamaged,
        EventKind::Interacted, EventKind::ModEvent,
            EventKind::UseUnclaimed,
    ]);
    s.pin("DamageSource::*", &vec![
        DamageSource::Fall,
        DamageSource::PlayerAttack { id: PlayerId(1) },
        DamageSource::MobAttack { key: "m:k".into() },
        DamageSource::Mod { mod_id: "m".into() },
    ]);
    s.pin("ContainerKind::*", &vec![
        ContainerKind::new("petramond:inventory"), ContainerKind::new("petramond:chest"),
        ContainerKind::new("m:g"),
    ]);
    s.pin("Facing::*", &vec![Facing::North, Facing::South, Facing::West, Facing::East]);
    s.pin("MobDamageFeedbackComponent::*", &vec![
        MobDamageFeedbackComponent::DecreaseHealth,
        MobDamageFeedbackComponent::Flash { duration: 0.5 },
        MobDamageFeedbackComponent::Knockback { scale: 1.0, duration: 0.5 },
        MobDamageFeedbackComponent::Sound { category: MobDamageSound::Hurt },
        MobDamageFeedbackComponent::Ragdoll,
        MobDamageFeedbackComponent::Immunity { ticks: 10 },
    ]);
    s.pin("MobDamageSound::*", &vec![MobDamageSound::Hurt, MobDamageSound::Death]);
    s.pin("BodyAction::*", &vec![BodyAction::Attack, BodyAction::Mine, BodyAction::Use]);
    s.pin("GuiValue::*", &vec![GuiValue::F32(1.0), GuiValue::I32(-1), GuiValue::Str("s".into())]);
    s.pin("MobTagValue::*", &vec![
        MobTagValue::Bool(true),
        MobTagValue::I64(-1),
        MobTagValue::F64(1.5),
        MobTagValue::Str("s".into()),
    ]);
    s.pin("MobTagLookup::*", &vec![
        MobTagLookup::MissingMob,
        MobTagLookup::Absent,
        MobTagLookup::Value(MobTagValue::I64(-1)),
    ]);
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
    s.pin("LightAperture::*", &vec![LightAperture::Opaque, LightAperture::Open]);

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
    ("HostCall::SpawnMob", "0c036d3a6b0000803f00000040000040400000003f01"),
    ("HostCall::MobsInRadius", "0d0000803f000000400000404000008040"),
    ("HostCall::DamageMob", "0e0700000040010000803f0000004000004040010200050a"),
    ("HostCall::DespawnMob", "0f07"),
    ("HostCall::SpawnItem", "10036d3a69030000803f000000400000404000"),
    ("HostCall::PlayerState", "11"),
    ("HostCall::DamagePlayer", "1204"),
    ("HostCall::ApplyKnockback", "130000803f0000004000004040"),
    ("HostCall::GiveItem", "14036d3a690201036d3a6b03010203"),
    ("HostCall::SetHealth", "1528"),
    ("HostCall::Teleport", "160000803f0000004000004040"),
    ("HostCall::EmitSound", "17036d3a73010000803f0000004000004040"),
    ("HostCall::WorldKvGet", "18036d3a6b"),
    ("HostCall::WorldKvSet", "19036d3a6b0101"),
    ("HostCall::WorldKvDelete", "1a036d3a6b"),
    ("HostCall::SectionKvGet", "1b020406036d3a6b"),
    ("HostCall::SectionKvSet", "1c020406036d3a6b0102"),
    ("HostCall::SectionKvDelete", "1d020406036d3a6b"),
    ("HostCall::MobTagGet", "1e07036d3a6b"),
    ("HostCall::MobTagSet", "1f07036d3a6b0105"),
    ("HostCall::MobTagDelete", "2007036d3a6b"),
    ("HostCall::ResolveBlock", "21036d3a62"),
    ("HostCall::RegisterWorldgenFeature", "220104"),
    ("HostCall::RegisterStageReplacement", "230102"),
    ("HostCall::RegisterGenerator", "2403"),
    ("HostCall::GuiStateSet", "25016b0102"),
    ("HostCall::GuiStateGet", "26016b"),
    ("HostCall::GuiOpen", "27036d3a67"),
    ("HostCall::GuiClose", "28"),
    ("HostCall::ChatSend", "290174010101"),
    ("HostCall::SoundPlayAt", "2a036d3a730000803f00000040000040400000803f0000803f"),
    ("HostCall::SoundPlayOnMob", "2b01036d3a730000803f0000803f"),
    ("HostCall::SoundStop", "2c01"),
    ("HostCall::ShaderSetParam", "2d036d3a70000000000000803e0000003f0000803f"),
    ("HostCall::RegisterHostileSpawner", "2e0104"),
    ("HostCall::RegisterBlockBehavior", "2f036d3a6201"),
    ("HostCall::RegisterAiNode", "30036d3a6e02"),
    ("HostCall::ContainerGet", "31020406"),
    ("HostCall::ContainerSet", "32020406020001036d3a6901000100"),
    ("HostCall::ItemInfo", "33036d3a69"),
    ("HostCall::RecipeResult", "34036d3a63036d3a69"),
    ("HostCall::EffectApply", "35036d3a6505"),
    ("HostCall::EffectsActive", "36"),
    ("HostCall::SwapModelBlock", "3702040606"),
    ("HostCall::ContainerGetMany", "3801020406"),
    ("HostCall::MobEmitterSet", "3907036d3a6501"),
    ("HostCall::EmitterBurst", "3a036d3a650000803f000000400000404000000040"),
    ("HostCall::RuntimeSide", "3b"),
    ("HostCall::ClientRegisterOverlay", "3c036d3a690001020304"),
    ("HostCall::ClientRegisterKey", "3d086f70656e5f6d61700e4f70656e20576f726c64204d6170056b65795f6d01"),
    ("HostCall::ClientSurfaceColumns", "3e01020303"),
    ("HostCall::ClientUiStateSet", "3f036d3a6b020176"),
    ("HostCall::ClientUiStateGet", "40036d3a6b"),
    ("HostCall::ClientImageSet", "41036d3a6901010401020304"),
    ("HostCall::ClientTextMeasure", "42017402"),
    ("HostCall::ClientImageDrawTexts", "43036d3a6901017402040101020304"),
    ("HostCall::ClientGuiOpen", "44036d3a67"),
    ("HostCall::ClientGuiClose", "45"),
    ("HostCall::ClientCanvasOpen", "46036d3a630102"),
    ("HostCall::ClientCanvasClose", "47"),
    ("HostCall::ClientCanvasSceneSet", "48036d3a630200036d3a690000803f00000040000040400000804001036d3a690000803f00000040"),
    ("HostCall::ClientCanvasViewSet", "49036d3a630000803f00000040"),
    ("HostCall::ClientStorageGetMany", "4a01036d3a6b"),
    ("HostCall::ClientStorageSetMany", "4b01036d3a6b0101"),
    ("HostCall::ResolveItem", "4c036d3a69"),
    ("HostCall::ClientImageBlit", "4d036d3a69010201010401020304"),
    ("HostCall::ClientStorageReadBegin", "4e01036d3a6b"),
    ("HostCall::ClientStorageReadPoll", "4f07"),
    ("HostCall::ConsumeHeld", "500301"),
    ("HostCall::ReplaceHeldOne", "5103036d3a69"),
    ("HostCall::MobMount", "52070100"),
    ("HostCall::MobDismount", "5301"),
    ("HostCall::MobRiders", "5407"),
    ("HostCall::MobDrive", "5507010000803f000000400100009040010000003f01"),
    ("HostCall::MobAnimSet", "560703726f7701"),
    ("HostCall::MobAnimRate", "570703726f77000080bf"),
    ("HostCall::MobAnimSeek", "580703726f770000c03f0000403f"),
    ("HostCall::PlayerInput", "5901"),
    ("HostCall::MobAnimState", "5a0703726f77"),
    ("HostCall::BiomeAt", "5b0203"),
    ("HostCall::SurfaceYAt", "5c0203"),
    ("HostCall::Players", "5d"),
    ("HostCall::ClientEnvParams", "5e01036d3a6b"),
    ("HostCall::ClientBiomeAt", "5f0203"),
    ("HostCall::ClientAmbientSet", "60066d3a7261696e0000003f0000803f000000c0"),
    ("HostCall::ClientLoopSet", "61066d3a6c6f6f700000003f"),
    ("HostCall::ClientMoodSet", "620000803e0000003f"),
    ("HostCall::ClientBlocksAt", "6301020306"),
    ("HostCall::BlocksByTag", "64036d3a74"),
    ("HostCall::ItemsByTag", "65036d3a74"),
    ("HostCall::BlockNames", "66020109"),
    ("HostCall::ItemNames", "67020109"),
    ("HostCall::ResolveMob", "68036d3a6b"),
    ("HostCall::MobNames", "69020109"),
    ("HostCall::CollisionShapeAt", "6a020406"),
    ("HostCall::MobTagsGet", "6b07"),
    ("HostCall::MobsWithTag", "6c036d3a6b010105"),
    ("HostCall::FindBlocks", "6d010405080a0c020109"),
    ("HostCall::MobInfo", "6e07"),
    ("HostCall::MobCanReach", "6f07020306"),
    ("HostCall::ResolveShape", "70036d3a73"),
    ("HostCall::PlayerPoseSet", "71010000c03f00000040000060c00000003f01"),
    ("HostCall::BlockModelGroup", "72020406"),
    ("HostCall::ClientCellKvAt", "73036d3a6b01020306"),
    ("HostCall::ItemDataGet", "7403036d3a6b"),
    ("HostCall::ItemsWithData", "75036d3a6b"),
    ("HostCall::BlockDataGet", "7604036d3a6b"),
    ("HostCall::BlocksWithData", "77036d3a6b"),
    ("HostCall::ResolveUndergroundBiome", "78036d3a75"),
    ("HostCall::UndergroundBiomeAt", "7901020306"),
    ("HostCall::TerrainSolidAt", "7a01020306"),
    ("HostCall::UndergroundBiomesInBox", "7b020306080a0b"),
    ("HostCall::UnlockRecipe", "7c01036d3a72"),
    ("HostCall::RecipeUnlocked", "7d01036d3a72"),
    ("HostCall::EmitEvent", "7e036d3a65020102"),
    ("HostCall::SurfaceBiomeAt", "7f010203"),
    ("HostCall::SetModelParts", "80010204060501090807"),
    ("HostCall::SetBlockDraw", "81010204060200000000000000803e000000000000803f0000003f0000803f0573746f6e6509080701010000003f0000003f0000003f0000003f0000c03f0000803e036d3a69010203"),
    ("HostCall::BlockLocalToWorld", "8201020406010000803e0000003f0000403f"),
    ("HostCall::SetBlockDraws", "8301010204060100000000000000803e000000000000803f0000003f0000803f0573746f6e6509080701"),
    ("HostCall::SetModelPartsMany", "8401010204060501090807"),
    ("HostCall::SectionKvGetMany", "8501036d3a6b01020406"),
    ("HostCall::SectionKvSetMany", "8601036d3a6b02020406010107080a0c00"),
    ("HostCall::GuiViewers", "8701"),
    ("HostCall::GuiStateSetFor", "880102016b000000003f"),
    ("HostCall::BlockInfo", "8901ac02"),
    ("HostCall::PlayerHeld", "8a0102"),
    ("HostCall::GiveItemTo", "8b010201690301016b0107"),
    ("HostCall::SetPlayerHeldData", "8c0102016901016b010601016b0107"),
    ("HostCall::SiteOpen", "8d01036d3a6b020306"),
    ("HostCall::SetPlayerSpeedScale", "8e01030000c03f"),
    ("HostCall::SetPlayerBonePose", "900105010d6c6566745f73686f756c6465720000b0c10000000000000000000000000000803f000000c001"),
    ("HostCall::SetPlayerHeldPose", "8f010401000000000000c03f00000000cdcccc3dcdcc4cbe9a9999be0000003f0000003f0000003fcdcccc3ecdcccc3ecdcccc3e00"),
    ("HostCall::EmitEventTo", "910106036d3a65020102"),
    ("HostCall::SetPlayerDeniedActions", "920107020001"),
    ("HostCall::HoldUse", "930108"),
    ("HostRet::Unit", "00"),
    ("HostRet::U64", "0101"),
    ("HostRet::Error", "020165"),
    ("HostRet::Bool", "0301"),
    ("HostRet::Block", "040101"),
    ("HostRet::Blocks", "0502000102"),
    ("HostRet::Light", "0601010203030201"),
    ("HostRet::Mobs", "070101020000803f000000400000404000008040050000003f0000803f00000000000000400100"),
    ("HostRet::Player", "0801010000803f00000040000040400000000000000000000000000000003f0000803e28010001010200000003010000c03f00000040000060c0"),
    ("HostRet::Bytes", "09010101"),
    ("HostRet::MobTag", "0a020001"),
    ("HostRet::GuiValue", "0b01000000803f"),
    ("HostRet::ContainerSlots", "0c010201036d3a69010000"),
    ("HostRet::ItemInfo", "0d014000010174014e010201077069636b61786501000000400000803f0000c03f013c01036d3a6564010b6275636b65745f66696c6c"),
    ("HostRet::ItemStack", "0e01036d3a690200"),
    ("HostRet::Effects", "0f01036d3a6509"),
    ("HostRet::Containers", "100201010000"),
    ("HostRet::RuntimeSide", "1102"),
    ("HostRet::ClientSurfaceColumns", "12030001020001030105ff7f010203"),
    ("HostRet::ClientTextSize", "130102"),
    ("HostRet::ClientStorageValues", "140200010101"),
    ("HostRet::Item", "150101"),
    ("HostRet::ClientStorageRead", "16010200010101"),
    ("HostRet::Riders", "170102010001"),
    ("HostRet::ModelGroup", "180102030603"),
    ("HostRet::PlayerInput", "19010000803f000080bf01000000003f0000803e"),
    ("HostRet::MobAnimState", "1a010000c03f0000403f0100000040"),
    ("HostRet::MaybeByte", "1b0104"),
    ("HostRet::MaybeI32", "1c010d"),
    ("HostRet::Players", "1d010101010000803f00000040000040400000000000000000000000000000003f0000803e2801000000010501000000"),
    ("HostRet::EnvParams", "1e0200010000803f000000400000404000008040"),
    ("HostRet::BlockList", "1f020109"),
    ("HostRet::ItemList", "20020109"),
    ("HostRet::Names", "21020001036d3a62"),
    ("HostRet::MobKind", "220101"),
    ("HostRet::CollisionShape", "230102"),
    ("HostRet::MobTags", "240101036d3a6b0001"),
    ("HostRet::SpawnedMob", "250107"),
    ("HostRet::FoundBlocks", "260101020306"),
    ("HostRet::Mob", "270101020000803f000000400000404000008040050000003f0000803f00000000000000400100"),
    ("HostRet::BytesMany", "28020102010200"),
    ("HostRet::ItemDataRows", "290103027b7d"),
    ("HostRet::BlockDataRows", "2a0104027b7d"),
    ("HostRet::UndergroundBiomes", "2b020002"),
    ("HostRet::TerrainSolid", "2c020100"),
    ("HostRet::SurfaceBiomes", "2d020306"),
    ("HostRet::Points", "2e01010000c03f0000204000006040"),
    ("HostRet::Bools", "2f020100"),
    ("HostRet::GuiViewers", "300102036d3a6701020406"),
    ("HostRet::BlockInfo", "31010573746f6e650000c03f0101077069636b61786501ac02"),
    ("HostRet::HeldStack", "3201036d3a690101036d3a6b0107"),
    ("GuestCall::TickSystem", "0001"),
    ("GuestCall::HandleEvent", "01010c"),
    ("GuestCall::GenFeature", "020102040604020102010a01060e"),
    ("GuestCall::GenStage", "0301020204060401010104010308"),
    ("GuestCall::GuiClick", "04036d3a67017701020406"),
    ("GuestCall::HostileSpawnCandidate", "05010000803f000000400000404002040601020300002042"),
    ("GuestCall::BlockBehavior", "060100020406"),
    ("GuestCall::AiNode", "0701010000803f00000040000040400204060000003f0902000080400000a0400000c0400100010701080a0c01036d3a6b0105"),
    ("GuestCall::ClientFrame", "08cdcc4c3d0000803f00000040000040400000003f0000803e8005e00301036d3a6700"),
    ("GuestCall::ClientKey", "090101"),
    ("GuestCall::ClientUi", "0a036d3a67000162"),
    ("GuestCall::ClientCanvas", "0b036d3a63000000803f0000004000"),
    ("GuestCall::ClientCanvasScroll", "0c036d3a630000803f00000040000080bf"),
    ("GuestCall::BakeShapeSim", "0d0101020406040000000000000101070001010900000000"),
    ("GuestCall::BakeShapeRender", "0e0101020406040000000000000101070001010900000000"),
    ("GuestCall::BakeShapeItem", "0f0104"),
    ("GuestCall::ShapePlacementPlan", "10010400000000020000020000"),
    ("GuestRet::Unit", "00"),
    ("GuestRet::Event", "010108010101"),
    ("GuestRet::GenWrites", "020102040604"),
    ("GuestRet::GenBlocks", "03020102"),
    ("GuestRet::GenBiomes", "040103"),
    ("GuestRet::HostileSpawn", "0501036d3a6b"),
    ("GuestRet::AiDecision", "060101020406010000003f0000803e010101000000400000404001036d3a6b010001"),
    ("BlockId (wide)", "ac02"),
    ("ItemId (wide)", "ff1f"),
    ("GuestRet::GenWrites (wide)", "0201020406ac02"),
    ("GuestRet::GenBlocks (wide)", "030201ac02"),
    ("GuestRet::BakedSim", "0701010000000000000000000000000000803f0000803f0000803f01"),
    ("GuestRet::BakedRender", "0801010000000000000000000000000000803f0000803f0000803f01c81e28011e01"),
    ("GuestRet::BakedItem", "0900"),
    ("GuestRet::ShapePlacement", "0a01000200010002000102"),
    ("EventPayload::BlockPlacePre", "000204060100"),
    ("EventPayload::BlockBreakPre", "010204060101020101036d3a690101036d3a6b0107"),
    ("EventPayload::InteractAttempt", "020102040601000200010700"),
    ("EventPayload::UseUnclaimed", "190102040601000200010700"),
    ("EventPayload::ItemUsePre", "030101020406"),
    ("EventPayload::MobDamagePre", "0407020000404000010000803f00000040000040400600010000003f020000803f0000003f030004050a"),
    ("EventPayload::PlayerDamagePre", "0502010100"),
    ("EventPayload::BlockPlaced", "0602040601"),
    ("EventPayload::BlockBroken", "07020406010001"),
    ("EventPayload::ItemUsed", "08020200"),
    ("ItemUseEvent::*", "000102"),
    ("EventPayload::MobDied", "0907010000803f0000004000004040"),
    ("EventPayload::MobSpawned", "0a07010000803f0000004000004040"),
    ("EventPayload::PlayerDamaged", "0b0226"),
    ("EventPayload::PlayerDied", "0c"),
    ("EventPayload::ContainerOpened", "0d0f70657472616d6f6e643a636865737401020406"),
    ("EventPayload::ContainerClosed", "0e036d3a6700"),
    ("EventPayload::SectionGenerated", "0f020406"),
    ("EventPayload::SectionLoaded", "10020406"),
    ("EventPayload::PlayerDismounted", "11000007"),
    ("EventPayload::PlayerDismounted(anchor)", "1100010000c03f00000040000060c0"),
    ("EventPayload::MobTagAdded", "120702036d3a6b0105"),
    ("EventPayload::MobTagRemoved", "130702036d3a6b0105"),
    ("EventPayload::ItemPickedUp", "140103040000c03f00000040000060c0"),
    ("EventPayload::ItemObtained", "150103"),
    ("EventPayload::MobDamaged", "1607020000c03f0001"),
    ("EventPayload::Interacted", "170102030601000200000101"),
    ("EventPayload::ModEvent", "18036d3a65020102"),
    ("Outcome::*", "020001"),
    ("Stage::*", "0c000102030405060708090a0b"),
    ("AttachSide::*", "020001"),
    ("WorldgenStage::*", "050001020304"),
    ("EventKind::*", "1a000102030405060708090a0b0c0d0e0f10111213141516171819"),
    ("DamageSource::*", "0400010102036d3a6b03016d"),
    ("ContainerKind::*", "031370657472616d6f6e643a696e76656e746f72790f70657472616d6f6e643a6368657374036d3a67"),
    ("Facing::*", "0400010203"),
    ("MobDamageFeedbackComponent::*", "0600010000003f020000803f0000003f030004050a"),
    ("MobDamageSound::*", "020001"),
    ("BodyAction::*", "03000102"),
    ("GuiValue::*", "03000000803f0101020173"),
    ("MobTagValue::*", "040001010102000000000000f83f030173"),
    ("MobTagLookup::*", "030001020101"),
    ("RuntimeSide::*", "03000102"),
    ("ClientOverlayAnchor::*", "0400010203"),
    ("ClientPointerPhase::*", "03000102"),
    ("ClientPointerButton::*", "020001"),
    ("ClientCanvasElement::*", "0200036d3a690000803f00000040000040400000804001036d3a690000803f00000040"),
    ("ClientUiEvent::*", "0400016201016201740201620174030162020000803f0000004001"),
    ("BlockHookKind::*", "03000102"),
    ("LightAperture::*", "020001"),
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
