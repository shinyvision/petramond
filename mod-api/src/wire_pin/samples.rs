//! The canonical sample per ABI enum variant, in DECLARATION ORDER.
//! Split from the pin test only because one file may not hold both and
//! stay inside the source audit's ceiling.

use super::Samples;
use crate::*;

#[rustfmt::skip]
pub(super) fn samples() -> Samples {
    let mut s = Samples(Vec::new());

    // --- HostCall: every variant, declaration order ------------------------
    s.pin("HostCall::Log", &HostCall::from(calls::Log { msg: "a".into() }));
    s.pin("HostCall::CurrentTick", &HostCall::from(calls::CurrentTick));
    s.pin("HostCall::RngU64", &HostCall::from(calls::RngU64 { stream_key: "s".into() }));
    s.pin("HostCall::RegisterTickSystem", &HostCall::from(calls::RegisterTickSystem {
        stage: Stage::Mining, attach: AttachSide::Before, priority: -1, system_id: 1,
    }));
    s.pin("HostCall::RegisterEventHandler", &HostCall::from(calls::RegisterEventHandler {
        event: EventKind::BlockPlacePre, priority: 1, handler_id: 2,
        filter: EventFilter::default(),
    }));
    s.pin("HostCall::GetBlock", &HostCall::from(calls::GetBlock { pos: [1, -2, 3] }));
    s.pin("HostCall::GetBlocks", &HostCall::from(calls::GetBlocks { positions: vec![[0, 0, 0]] }));
    s.pin("HostCall::SetBlock", &HostCall::from(calls::SetBlock { pos: [1, 2, 3], block: BlockId(4) }));
    s.pin("HostCall::SetBlocks", &HostCall::from(calls::SetBlocks { blocks: vec![([1, 2, 3], BlockId(5))] }));
    s.pin("HostCall::ScheduleTick", &HostCall::from(calls::ScheduleTick { pos: [1, 2, 3], delay: 7 }));
    s.pin("HostCall::IsLoaded", &HostCall::from(calls::IsLoaded { pos: [1, 2, 3] }));
    s.pin("HostCall::LightAt", &HostCall::from(calls::LightAt { pos: [1, 2, 3] }));
    s.pin("HostCall::SpawnMob", &HostCall::from(calls::SpawnMob {
        key: "m:k".into(), pos: [1.0, 2.0, 3.0], yaw: 0.5, checked: true,
    }));
    s.pin("HostCall::MobsInRadius", &HostCall::from(calls::MobsInRadius { pos: [1.0, 2.0, 3.0], radius: 4.0 }));
    s.pin("HostCall::DamageMob", &HostCall::from(calls::DamageMob {
        mob_id: 7, amount: 2.0, origin: Some([1.0, 2.0, 3.0]),
        feedback: Some(crate::events::MobDamageFeedback {
            components: vec![
                crate::events::MobDamageFeedbackComponent::DecreaseHealth,
                crate::events::MobDamageFeedbackComponent::Immunity { ticks: 10 },
            ],
        }),
        attacker: Some(EntityRef::Player(PlayerId(3))),
    }));
    s.pin("HostCall::DespawnMob", &HostCall::from(calls::DespawnMob { mob_id: 7 }));
    s.pin("HostCall::SpawnItem", &HostCall::from(calls::SpawnItem {
        item: "m:i".into(), count: 3, pos: [1.0, 2.0, 3.0],
        data: Vec::new(),
    }));
    s.pin("HostCall::PlayerState", &HostCall::from(calls::PlayerState));
    s.pin("HostCall::DamagePlayer", &HostCall::from(calls::DamagePlayer {
        player: PlayerId(1), amount: 2, origin: Some([1.0, 2.0, 3.0]),
        attacker: Some(EntityRef::Mob(9)),
    }));
    s.pin("HostCall::ApplyKnockback", &HostCall::from(calls::ApplyKnockback { impulse: [1.0, 2.0, 3.0] }));
    s.pin("HostCall::GiveItem", &HostCall::from(calls::GiveItem {
        item: "m:i".into(),
        count: 2,
        data: vec![("m:k".into(), vec![1, 2, 3])],
    }));
    s.pin("HostCall::SetHealth", &HostCall::from(calls::SetHealth { value: 20 }));
    s.pin("HostCall::Teleport", &HostCall::from(calls::Teleport { pos: [1.0, 2.0, 3.0] }));
    s.pin("HostCall::EmitSound", &HostCall::from(calls::EmitSound {
        key: "m:s".into(), pos: Some([1.0, 2.0, 3.0]),
    }));
    s.pin("HostCall::WorldKvGet", &HostCall::from(calls::WorldKvGet { key: "m:k".into() }));
    s.pin("HostCall::WorldKvSet", &HostCall::from(calls::WorldKvSet { key: "m:k".into(), value: vec![1] }));
    s.pin("HostCall::WorldKvDelete", &HostCall::from(calls::WorldKvDelete { key: "m:k".into() }));
    s.pin("HostCall::SectionKvGet", &HostCall::from(calls::SectionKvGet { pos: [1, 2, 3], key: "m:k".into() }));
    s.pin("HostCall::SectionKvSet", &HostCall::from(calls::SectionKvSet {
        pos: [1, 2, 3], key: "m:k".into(), value: vec![2],
    }));
    s.pin("HostCall::SectionKvDelete", &HostCall::from(calls::SectionKvDelete {
        pos: [1, 2, 3], key: "m:k".into(),
    }));
    s.pin("HostCall::MobTagGet", &HostCall::from(calls::MobTagGet { mob_id: 7, key: "m:k".into() }));
    s.pin("HostCall::MobTagSet", &HostCall::from(calls::MobTagSet {
        mob_id: 7, key: "m:k".into(), value: MobTagValue::I64(-3),
    }));
    s.pin("HostCall::MobTagDelete", &HostCall::from(calls::MobTagDelete { mob_id: 7, key: "m:k".into() }));
    s.pin("HostCall::ResolveBlock", &HostCall::from(calls::ResolveBlock { name: "m:b".into() }));
    s.pin("HostCall::RegisterWorldgenFeature", &HostCall::from(calls::RegisterWorldgenFeature {
        feature_id: 1, stage: WorldgenStage::Trees,
        filter: Default::default(),
    }));
    s.pin("HostCall::RegisterStageReplacement", &HostCall::from(calls::RegisterStageReplacement {
        stage: WorldgenStage::Terrain, callback_id: 2,
    }));
    s.pin("HostCall::RegisterGenerator", &HostCall::from(calls::RegisterGenerator { callback_id: 3 }));
    s.pin("HostCall::GuiStateSet", &HostCall::from(calls::GuiStateSet {
        key: "k".into(), value: GuiValue::I32(1),
    }));
    s.pin("HostCall::GuiStateGet", &HostCall::from(calls::GuiStateGet { key: "k".into() }));
    s.pin("HostCall::GuiOpen", &HostCall::from(calls::GuiOpen { kind_key: "m:g".into(), at: Some(ContainerAddress::Block([1, 2, 3])) }));
    s.pin("HostCall::GuiClose", &HostCall::from(calls::GuiClose));
    s.pin("HostCall::ChatSend", &HostCall::from(calls::ChatSend {
        text: "t".into(), targets: Some(vec![PlayerId(1)]),
    }));
    s.pin("HostCall::SoundPlayAt", &HostCall::from(calls::SoundPlayAt {
        key: "m:s".into(), pos: [1.0, 2.0, 3.0], volume: 1.0, pitch: 1.0,
    }));
    s.pin("HostCall::SoundPlayOnMob", &HostCall::from(calls::SoundPlayOnMob {
        mob_id: 1, key: "m:s".into(), volume: 1.0, pitch: 1.0,
    }));
    s.pin("HostCall::SoundStop", &HostCall::from(calls::SoundStop { handle: 1 }));
    s.pin("HostCall::ShaderSetParam", &HostCall::from(calls::ShaderSetParam {
        key: "m:p".into(), value: [0.0, 0.25, 0.5, 1.0],
    }));
    s.pin("HostCall::RegisterHostileSpawner", &HostCall::from(calls::RegisterHostileSpawner {
        callback_id: 1, priority: 2,
    }));
    s.pin("HostCall::RegisterBlockBehavior", &HostCall::from(calls::RegisterBlockBehavior {
        key: "m:b".into(), callback_id: 1,
    }));
    s.pin("HostCall::RegisterAiNode", &HostCall::from(calls::RegisterAiNode {
        key: "m:n".into(), callback_id: 2,
    }));
    s.pin("HostCall::ContainerGet", &HostCall::from(calls::ContainerGet { at: ContainerAddress::Block([1, 2, 3]) }));
    s.pin("HostCall::ContainerSet", &HostCall::from(calls::ContainerSet {
        at: ContainerAddress::Mob(9),
        slots: vec![(0, Some(ItemStackData { item: "m:i".into(), count: 1, data: Vec::new() })), (1, None)],
    }));
    s.pin("HostCall::ItemInfo", &HostCall::from(calls::ItemInfo {
        item: "m:i".into(), data: vec![("m:k".into(), vec![7])],
    }));
    s.pin("HostCall::RecipeResult", &HostCall::from(calls::RecipeResult {
        class: "m:c".into(), item: "m:i".into(),
    }));
    s.pin("HostCall::EffectApply", &HostCall::from(calls::EffectApply { key: "m:e".into(), ticks: 5 }));
    s.pin("HostCall::EffectsActive", &HostCall::from(calls::EffectsActive));
    s.pin("HostCall::SwapBlock", &HostCall::from(calls::SwapBlock {
        pos: [1, 2, 3], block: BlockId(6),
    }));
    s.pin("HostCall::ContainerGetMany", &HostCall::from(calls::ContainerGetMany {
        addresses: vec![ContainerAddress::Block([1, 2, 3]), ContainerAddress::Mob(4)],
    }));
    s.pin("HostCall::MobEmitterSet", &HostCall::from(calls::MobEmitterSet {
        mob_id: 7, key: "m:e".into(), active: true,
    }));
    s.pin("HostCall::EmitterBurst", &HostCall::from(calls::EmitterBurst {
        key: "m:e".into(), pos: [1.0, 2.0, 3.0], intensity: 2.0,
        direction: Some([0.0, 1.0, 0.0]),
        texture: Some(ParticleTexture::Tile {
            tile: "m:t".into(), slice: [0.0, 0.25, 0.5, 1.0], tint: [1, 2, 3],
        }),
    }));
    s.pin("ParticleTexture::Block", &ParticleTexture::Block { block: BlockId(4), tint: Some([1, 2, 3]) });
    s.pin("HostCall::RuntimeSide", &HostCall::from(calls::RuntimeSide));
    s.pin("HostCall::ClientRegisterOverlay", &HostCall::from(calls::ClientRegisterOverlay {
        image_key: "m:i".into(), anchor: ClientOverlayAnchor::TopLeft,
        margin: [1, 2], display_size: [3, 4], hud: false,
    }));
    s.pin("HostCall::ClientRegisterKey", &HostCall::from(calls::ClientRegisterKey {
        id: "open_map".into(), label: "Open World Map".into(),
        key: "key_m".into(), mods: ClientKeyMods { ctrl: true, shift: false, alt: true },
        contexts: ClientKeyContexts { gameplay: true, screens: vec!["m:s".into()] },
        action_id: 1,
    }));
    s.pin("HostCall::ClientSurfaceColumns", &HostCall::from(calls::ClientSurfaceColumns {
        queries: vec![ClientSurfaceQuery { coord: [1, -2], revision: 3 }],
    }));
    s.pin("HostCall::ClientUiStateSet", &HostCall::from(calls::ClientUiStateSet {
        key: "m:k".into(), value: GuiValue::Str("v".into()),
    }));
    s.pin("HostCall::ClientUiStateGet", &HostCall::from(calls::ClientUiStateGet { key: "m:k".into() }));
    s.pin("HostCall::ClientImageSet", &HostCall::from(calls::ClientImageSet {
        key: "m:i".into(), width: 1, height: 1, rgba: vec![1, 2, 3, 4],
    }));
    s.pin("HostCall::ClientTextMeasure", &HostCall::from(calls::ClientTextMeasure {
        text: "t".into(), scale: 2,
    }));
    s.pin("HostCall::ClientImageDrawTexts", &HostCall::from(calls::ClientImageDrawTexts {
        key: "m:i".into(),
        runs: vec![ClientTextRun { text: "t".into(), position: [1, 2], scale: 1, color: [1, 2, 3, 4] }],
    }));
    s.pin("HostCall::ClientGuiOpen", &HostCall::from(calls::ClientGuiOpen { kind_key: "m:g".into() }));
    s.pin("HostCall::ClientGuiClose", &HostCall::from(calls::ClientGuiClose));
    s.pin("HostCall::ClientCanvasOpen", &HostCall::from(calls::ClientCanvasOpen {
        canvas_key: "m:c".into(), size: [1, 2],
    }));
    s.pin("HostCall::ClientCanvasClose", &HostCall::from(calls::ClientCanvasClose));
    s.pin("HostCall::ClientCanvasSceneSet", &HostCall::from(calls::ClientCanvasSceneSet {
        canvas_key: "m:c".into(),
        elements: vec![
            ClientCanvasElement::Image { image_key: "m:i".into(), rect: [1.0, 2.0, 3.0, 4.0] },
            ClientCanvasElement::Sprite { image_key: "m:i".into(), center: [1.0, 2.0] },
        ],
    }));
    s.pin("HostCall::ClientCanvasViewSet", &HostCall::from(calls::ClientCanvasViewSet {
        canvas_key: "m:c".into(), offset: [1.0, 2.0],
    }));
    s.pin("HostCall::ClientStorageGetMany", &HostCall::from(calls::ClientStorageGetMany {
        scope: ClientStorageScope::Pack, keys: vec!["m:k".into()],
    }));
    s.pin("HostCall::ClientStorageSetMany", &HostCall::from(calls::ClientStorageSetMany {
        scope: ClientStorageScope::Pack,
        entries: vec![("m:k".into(), Some(ByteBuf::from(vec![1]))), ("m:d".into(), None)],
    }));
    s.pin("HostCall::ResolveItem", &HostCall::from(calls::ResolveItem { name: "m:i".into() }));
    s.pin("HostCall::ClientImageBlit", &HostCall::from(calls::ClientImageBlit {
        key: "m:i".into(), origin: [1, 2], size: [1, 1], rgba: vec![1, 2, 3, 4],
    }));
    s.pin("HostCall::ClientStorageReadBegin", &HostCall::from(calls::ClientStorageReadBegin {
        scope: ClientStorageScope::Pack, keys: vec!["m:k".into()],
    }));
    s.pin("HostCall::ClientStorageReadPoll", &HostCall::from(calls::ClientStorageReadPoll {
        scope: ClientStorageScope::Pack, ticket: 7,
    }));
    s.pin("HostCall::ConsumeHeld", &HostCall::from(calls::ConsumeHeld { item: ItemId(3), count: 1 }));
    s.pin("HostCall::ReplaceHeldOne", &HostCall::from(calls::ReplaceHeldOne { item: ItemId(3), replacement: "m:i".into() }));
    s.pin("HostCall::MobMount", &HostCall::from(calls::MobMount {
        mob_id: 7, player_id: PlayerId(1), seat: 0,
    }));
    s.pin("HostCall::MobDismount", &HostCall::from(calls::MobDismount { player_id: PlayerId(1) }));
    s.pin("HostCall::MobRiders", &HostCall::from(calls::MobRiders { mob_id: 7 }));
    s.pin("HostCall::MobDrive", &HostCall::from(calls::MobDrive {
        mob_id: 7, horizontal: Some([1.0, 2.0]), vertical: Some(4.5), yaw: Some(0.5), while_walking: true,
        gait: false,
    }));
    s.pin("HostCall::MobAnimSet", &HostCall::from(calls::MobAnimSet {
        mob_id: 7, anim: "row".into(), active: true,
    }));
    s.pin("HostCall::MobAnimRate", &HostCall::from(calls::MobAnimRate {
        mob_id: 7, anim: "row".into(), rate: -1.0,
    }));
    s.pin("HostCall::MobAnimSeek", &HostCall::from(calls::MobAnimSeek {
        mob_id: 7, anim: "row".into(), phase: 1.5, rate: 0.75,
    }));
    s.pin("HostCall::PlayerInput", &HostCall::from(calls::PlayerInput { player_id: PlayerId(1) }));
    s.pin("HostCall::MobAnimState", &HostCall::from(calls::MobAnimState {
        mob_id: 7, anim: "row".into(),
    }));
    s.pin("HostCall::BiomeAt", &HostCall::from(calls::BiomeAt { pos: [1, -2] }));
    s.pin("HostCall::SurfaceYAt", &HostCall::from(calls::SurfaceYAt { pos: [1, -2] }));
    s.pin("HostCall::Players", &HostCall::from(calls::Players));
    s.pin("HostCall::ClientEnvParams", &HostCall::from(calls::ClientEnvParams {
        keys: vec!["m:k".into()],
    }));
    s.pin("HostCall::ClientBiomeAt", &HostCall::from(calls::ClientBiomeAt { pos: [1, -2] }));
    s.pin("HostCall::ClientAmbientSet", &HostCall::from(calls::ClientAmbientSet {
        key: "m:rain".into(), intensity: 0.5, wind: [1.0, -2.0],
    }));
    s.pin("HostCall::ClientLoopSet", &HostCall::from(calls::ClientLoopSet {
        key: "m:loop".into(), gain: 0.5,
    }));
    s.pin("HostCall::ClientMoodSet", &HostCall::from(calls::ClientMoodSet {
        darken: 0.25, desaturate: 0.5,
    }));
    s.pin("HostCall::ClientBlocksAt", &HostCall::from(calls::ClientBlocksAt {
        positions: vec![[1, -2, 3]],
    }));
    s.pin("HostCall::BlocksByTag", &HostCall::from(calls::BlocksByTag { tag: "m:t".into() }));
    s.pin("HostCall::ItemsByTag", &HostCall::from(calls::ItemsByTag { tag: "m:t".into() }));
    s.pin("HostCall::BlockNames", &HostCall::from(calls::BlockNames { blocks: vec![BlockId(1), BlockId(9)] }));
    s.pin("HostCall::ItemNames", &HostCall::from(calls::ItemNames { items: vec![ItemId(1), ItemId(9)] }));
    s.pin("HostCall::ResolveMob", &HostCall::from(calls::ResolveMob { key: "m:k".into() }));
    s.pin("HostCall::MobNames", &HostCall::from(calls::MobNames { mobs: vec![MobId(1), MobId(9)] }));
    s.pin("HostCall::CollisionShapeAt", &HostCall::from(calls::CollisionShapeAt { pos: [1, 2, 3] }));
    s.pin("HostCall::MobTagsGet", &HostCall::from(calls::MobTagsGet { mob_id: 7 }));
    s.pin("HostCall::MobsWithTag", &HostCall::from(calls::MobsWithTag {
        key: "m:k".into(), value: Some(MobTagValue::I64(-3)),
    }));
    s.pin("HostCall::FindBlocks", &HostCall::from(calls::FindBlocks {
        min: [-1, 2, -3], max: [4, 5, 6], blocks: vec![BlockId(1), BlockId(9)],
    }));
    s.pin("HostCall::MobInfo", &HostCall::from(calls::MobInfo { mob_id: 7 }));
    s.pin("HostCall::MobCanReach", &HostCall::from(calls::MobCanReach { mob_id: 7, cell: [1, -2, 3] }));
    s.pin("HostCall::ResolveShape", &HostCall::from(calls::ResolveShape { key: "m:s".into() }));
    s.pin("HostCall::PlayerPoseSet", &HostCall::from(calls::PlayerPoseSet {
        player_id: PlayerId(1), anchor: [1.5, 2.0, -3.5], yaw: 0.5, pose: pose::SITTING,
    }));
    s.pin("HostCall::BlockModelGroup", &HostCall::from(calls::BlockModelGroup { pos: [1, 2, 3] }));
    s.pin("HostCall::ClientCellKvAt", &HostCall::from(calls::ClientCellKvAt {
        key: "m:k".into(), cells: vec![[1, -2, 3]],
    }));
    s.pin("HostCall::ItemDataGet", &HostCall::from(calls::ItemDataGet { item: ItemId(3), key: "m:k".into() }));
    s.pin("HostCall::ItemsWithData", &HostCall::from(calls::ItemsWithData { key: "m:k".into() }));
    s.pin("HostCall::BlockDataGet", &HostCall::from(calls::BlockDataGet { block: BlockId(4), key: "m:k".into() }));
    s.pin("HostCall::BlocksWithData", &HostCall::from(calls::BlocksWithData { key: "m:k".into() }));
    s.pin("HostCall::ResolveUndergroundBiome", &HostCall::from(calls::ResolveUndergroundBiome { key: "m:u".into() }));
    s.pin("HostCall::UndergroundBiomeAt", &HostCall::from(calls::UndergroundBiomeAt {
        positions: vec![[1, -2, 3]],
    }));
    s.pin("HostCall::TerrainSolidAt", &HostCall::from(calls::TerrainSolidAt {
        positions: vec![[1, -2, 3]],
    }));
    s.pin("HostCall::UndergroundBiomesInBox", &HostCall::from(calls::UndergroundBiomesInBox {
        lo: [1, -2, 3], hi: [4, 5, -6],
    }));
    s.pin("HostCall::UnlockRecipe", &HostCall::from(calls::UnlockRecipe {
        player: PlayerId(1), recipe: "m:r".into(),
    }));
    s.pin("HostCall::RecipeUnlocked", &HostCall::from(calls::RecipeUnlocked {
        player: PlayerId(1), recipe: "m:r".into(),
    }));
    s.pin("HostCall::EmitEvent", &HostCall::from(calls::EmitEvent {
        key: "m:e".into(), data: vec![1, 2],
    }));
    s.pin("HostCall::SurfaceBiomeAt", &HostCall::from(calls::SurfaceBiomeAt {
        columns: vec![[1, -2]],
    }));
    s.pin("HostCall::SetModelParts", &HostCall::from(calls::SetModelParts {
        pos: [1, 2, 3], parts: 5, tint: Some([9, 8, 7]),
    }));
    s.pin("HostCall::SetBlockDraw", &HostCall::from(calls::SetBlockDraw {
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
    }));
    s.pin("HostCall::BlockLocalToWorld", &HostCall::from(calls::BlockLocalToWorld {
        pos: [1, 2, 3], points: vec![[0.25, 0.5, 0.75]],
    }));
    s.pin("HostCall::SetBlockDraws", &HostCall::from(calls::SetBlockDraws {
        sets: vec![([1, 2, 3], vec![crate::DrawPrim::Cuboid {
            min: [0.0, 0.25, 0.0], max: [1.0, 0.5, 1.0],
            tile: "stone".into(), tint: [9, 8, 7], emissive: true,
        }])],
    }));
    s.pin("HostCall::SetModelPartsMany", &HostCall::from(calls::SetModelPartsMany {
        sets: vec![([1, 2, 3], 5, Some([9, 8, 7]))],
    }));
    s.pin("HostCall::SectionKvGetMany", &HostCall::from(calls::SectionKvGetMany {
        key: "m:k".into(), positions: vec![[1, 2, 3]],
    }));
    s.pin("HostCall::SectionKvSetMany", &HostCall::from(calls::SectionKvSetMany {
        key: "m:k".into(), writes: vec![([1, 2, 3], Some(vec![7])), ([4, 5, 6], None)],
    }));
    s.pin("HostCall::GuiViewers", &HostCall::from(calls::GuiViewers));
    s.pin("HostCall::GuiStateSetFor", &HostCall::from(calls::GuiStateSetFor {
        player_id: PlayerId(2), key: "k".into(), value: GuiValue::F32(0.5),
    }));
    s.pin("HostCall::BlockInfo", &HostCall::from(calls::BlockInfo { block: BlockId(300) }));
    s.pin("HostCall::PlayerHeld", &HostCall::from(calls::PlayerHeld { player: PlayerId(2) }));
    s.pin(
        "HostCall::GiveItemTo",
        &HostCall::from(calls::GiveItemTo {
            player: PlayerId(2),
            item: "i".into(),
            count: 3,
            data: vec![("k".into(), vec![7])],
        }),
    );
    s.pin(
        "HostCall::SetPlayerHeldData",
        &HostCall::from(calls::SetPlayerHeldData {
            player: PlayerId(2),
            expect_item: "i".into(),
            expect_data: vec![("k".into(), vec![6])],
            data: vec![("k".into(), vec![7])],
        }),
    );
    s.pin("HostCall::SiteOpen", &HostCall::from(calls::SiteOpen { key: "m:k".into(), cell: [1, -2, 3] }));
    s.pin("HostCall::SetPlayerAttribute", &HostCall::from(calls::SetPlayerAttribute {
        player: PlayerId(3), attribute: PlayerAttribute::AttackCooldown, scale: 1.5,
    }));
    s.pin("HostCall::SetPlayerBonePose", &HostCall::from(calls::SetPlayerBonePose {
        player: PlayerId(5),
        bones: vec![crate::BonePoseData {
            bone: "left_shoulder".into(), rotation: [-22.0, 0.0, 0.0], translation: [0.0, 1.0, -2.0],
            mode: crate::BonePoseMode::Replace,
        }],
    }));
    s.pin("HostCall::SetPlayerHeldPose", &HostCall::from(calls::SetPlayerHeldPose {
        player: PlayerId(4),
        main: Some(crate::HeldPose {
            first_person: crate::HeldPoseData { rotation: [0.0, 1.5, 0.0], translation: [0.1, -0.2, -0.3] },
            third_person: crate::HeldPoseData { rotation: [0.5; 3], translation: [0.4; 3] },
        }),
        off: None,
    }));
    s.pin("HostCall::EmitEventTo", &HostCall::from(calls::EmitEventTo {
        player: PlayerId(6), key: "m:e".into(), data: vec![1, 2],
    }));
    s.pin("HostCall::SetPlayerDeniedActions", &HostCall::from(calls::SetPlayerDeniedActions {
        player: PlayerId(7), actions: vec![BodyAction::Attack, BodyAction::Mine],
    }));
    s.pin("HostCall::HoldUse", &HostCall::from(calls::HoldUse { player: PlayerId(8) }));
    s.pin("HostCall::Raycast", &HostCall::from(calls::Raycast {
        from: [1.0, 2.0, 3.0], dir: [0.0, -1.0, 0.0], max: 8.0, filter: RayFilter::Collidable,
    }));
    s.pin("HostCall::LaunchItem", &HostCall::from(calls::LaunchItem {
        item: "m:i".into(), pos: [1.0, 2.0, 3.0], vel: [0.0, 4.0, 0.0],
        owner: Some(EntityRef::Player(PlayerId(1))), data: vec![],
    }));
    s.pin("HostCall::ItemEntity", &HostCall::from(calls::ItemEntity { entity: 9 }));
    s.pin("HostCall::TakeItem", &HostCall::from(calls::TakeItem {
        player: PlayerId(2), item: "m:i".into(), count: 3, data: Some(vec![("m:k".into(), vec![7])]),
    }));
    s.pin("HostCall::SetPlayerHeldDisplay", &HostCall::from(calls::SetPlayerHeldDisplay {
        player: PlayerId(3), main: Some("m:i".into()), off: None,
    }));
    s.pin("HostCall::PlayerInventory", &HostCall::from(calls::PlayerInventory { player: PlayerId(3) }));
    s.pin("HostCall::MobKinematic", &HostCall::from(calls::MobKinematic {
        mob_id: 7, pos: [1.0, 2.0, 3.0], yaw: 0.5, pitch: -0.25, roll: 0.125,
    }));
    s.pin("HostCall::SoundSet", &HostCall::from(calls::SoundSet { handle: 1, volume: 0.5, pitch: 1.0 }));

    s.pin("HostCall::ContainerInsert", &HostCall::from(calls::ContainerInsert { at: ContainerAddress::Block([1, 2, 3]), stack: ItemStackData {item: "m:i".into(), count: 2, data: Vec::new()} }));
    s.pin("HostCall::ContainerTake", &HostCall::from(calls::ContainerTake { at: ContainerAddress::Mob(5), slot: 4, count: 2 }));

    s.pin("HostCall::ItemEntitiesInRadius", &HostCall::from(calls::ItemEntitiesInRadius { pos: [1.0, 2.0, 3.0], radius: 4.0, limit: 8 }));
    s.pin("HostCall::ItemImpulses", &HostCall::from(calls::ItemImpulses { impulses: vec![(9, [1.0, 0.0, -1.0])] }));
    s.pin("HostCall::SectionKvFind", &HostCall::from(calls::SectionKvFind { section: [-1, -2, 3], key: "fixture:marker".into() }));
    s.pin("HostCall::StructureInfo", &HostCall::from(calls::StructureInfo { key: "fixture:room".into() }));
    s.pin("HostCall::LootRoll", &HostCall::from(calls::LootRoll { key: "fixture:loot".into(), seed: 7 }));
    s.pin("HostCall::MobDataGet", &HostCall::from(calls::MobDataGet { mob: MobId(4), key: "m:k".into() }));
    s.pin("HostCall::MobsWithData", &HostCall::from(calls::MobsWithData { key: "m:k".into() }));
    s.pin("HostCall::TerrainSpaceAt", &HostCall::from(calls::TerrainSpaceAt { positions: vec![[1, -2, 3]] }));
    s.pin("HostCall::MemoGet", &HostCall::from(calls::MemoGet { key: b"k".to_vec() }));
    s.pin("HostCall::MemoGetMany", &HostCall::from(calls::MemoGetMany { keys: vec![b"k".to_vec()] }));
    s.pin("HostCall::MemoPut", &HostCall::from(calls::MemoPut { key: b"k".to_vec(), value: b"v".to_vec() }));
    s.pin("HostCall::MemoClaim", &HostCall::from(calls::MemoClaim { key: b"k".to_vec() }));

    s.pin("HostCall::TerrainBlocksAt", &HostCall::from(calls::TerrainBlocksAt { positions: vec![[1, -2, 3]] }));
    s.pin("HostCall::TerrainHeightsAt", &HostCall::from(calls::TerrainHeightsAt { columns: vec![[1, -2]] }));
    s.pin("HostCall::TerrainSectionAt", &HostCall::from(calls::TerrainSectionAt { section: [1, -2, 3] }));

    // --- HostRet: every variant, declaration order --------------------------
    s.pin("HostRet::Unit", &HostRet::Unit);
    s.pin("HostRet::U64", &HostRet::U64(1));
    s.pin("HostRet::Err", &HostRet::invalid("e".into()));
    s.pin("HostRet::Err(Refused)", &HostRet::refused("e"));
    s.pin("HostRet::Bool", &HostRet::Bool(true));
    s.pin("HostRet::Block", &HostRet::Block(Some(BlockId(1))));
    s.pin("HostRet::Blocks", &HostRet::Blocks(vec![None, Some(BlockId(2))]));
    s.pin("HostRet::Light", &HostRet::Light(Some(LightData { combined: 1, sky: 2, block: 3, block_rgb: [3, 2, 1] })));
    s.pin("HostRet::Mobs", &HostRet::Mobs(vec![MobSnapshot {
        index: 1, kind: MobId(2), pos: [1.0, 2.0, 3.0], health: 4.0, id: 5,
        yaw: 0.5, pitch: 0.0, roll: 0.0, vel: [1.0, 0.0, 2.0], on_ground: true, moving: false,
        half_width: 0.4, height: 1.2, half_length: 0.4, entombed: false,
        conditions: vec![ConditionData { condition: ConditionId(0), stage: 1, remaining: 120, elapsed: 3 }],
    }]));
    s.pin("HostRet::Player", &HostRet::Player(Box::new(PlayerSnapshot {
        id: Some(PlayerId(1)),
        pos: [1.0, 2.0, 3.0], vel: [0.0, 0.0, 0.0], yaw: 0.5, pitch: 0.25,
        health: 20, on_ground: true, spectator: false, sneak: true, held: Some(ItemId(2)), held_count: 3,
        off_held: None, use_held: false, holds_use: false,
        pose_anchor: Some([1.5, 2.0, -3.5]),
        swing: crate::HandSwing { mining: true, main: Some(crate::SwingKind::Attack), off: None },
        half_width: 0.3, height: 1.8, eye_height: 1.62, entombed: false, conditions: Vec::new(),
    })));
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
            knockback: 1.0,
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
            swing: crate::HandSwing::default(),
            half_width: 0.3, height: 1.8, eye_height: 1.62, entombed: false, conditions: Vec::new(),
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
        yaw: 0.5, pitch: 0.0, roll: 0.0, vel: [1.0, 0.0, 2.0], on_ground: true, moving: false,
        half_width: 0.4, height: 1.2, half_length: 0.4, entombed: false,
        conditions: vec![ConditionData { condition: ConditionId(0), stage: 1, remaining: 120, elapsed: 3 }],
    })));
    s.pin("HostRet::ItemEntity", &HostRet::ItemEntity(Some(Box::new(ItemEntityData {
        id: 9, stack: ItemStackData { item: "m:i".into(), count: 1, data: vec![("m:k".into(), vec![7])] },
        owner: Some(EntityRef::Player(PlayerId(0))), pos: [1.0, 2.0, 3.0], vel: [0.0, 0.0, -4.0],
        motion: ItemMotion::Flight,
    }))));
    s.pin("HostRet::BytesMany", &HostRet::BytesMany(vec![Some(vec![1, 2]), None]));
    s.pin("HostRet::ItemDataRows", &HostRet::ItemDataRows(vec![(ItemId(3), "{}".into())]));
    s.pin("HostRet::BlockDataRows", &HostRet::BlockDataRows(vec![(BlockId(4), "{}".into())]));
    s.pin("HostRet::UndergroundBiomes", &HostRet::UndergroundBiomes(vec![0, 2]));
    s.pin("HostRet::TerrainSolid", &HostRet::TerrainSolid(vec![true, false]));
    s.pin("HostRet::SurfaceBiomes", &HostRet::SurfaceBiomes(vec![3, 6]));
    s.pin("HostRet::Points", &HostRet::Points(Some(vec![[1.5, 2.5, 3.5]])));
    s.pin("HostRet::Bools", &HostRet::Bools(vec![true, false]));
    s.pin("HostRet::GuiViewers", &HostRet::GuiViewers(vec![GuiViewerData {
        player_id: PlayerId(2), kind: "m:g".into(), anchor: Some(ContainerAddress::Block([1, 2, 3])),
    }]));
    s.pin("HostRet::BlockInfo", &HostRet::BlockInfo(Some(Box::new(BlockInfoData {
        material: "stone".into(), hardness: 1.5, harvest_tier: 1,
        preferred_tool: Some("pickaxe".into()), item: Some(ItemId(300)),
        collision: vec![([0.0, 0.0, 0.0], [1.0, 0.5, 1.0])],
        fluid: Some(FluidInfoData {
            delay: 30, drop_off: 2, renewable: false,
            quench: Some(QuenchData { by: BlockId(1), result: BlockId(2) }),
            contact_damage: Some(PulseData { amount: 3, interval: 10 }),
            applies: Some(ConditionGrantData { condition: ConditionId(0), stage: 1, ticks: 120 }),
            clears: vec![ConditionId(1)], destroys_items: true,
        }), replaceable: false, interaction: Some(crate::BlockUse::ToggleDoor),
    }))));
    s.pin("HostRet::HeldStack", &HostRet::HeldStack(Some(ItemStackData {
        item: "m:i".into(), count: 1, data: vec![("m:k".into(), vec![7])],
    })));
    s.pin("HostRet::Raycast", &HostRet::Raycast(Some(RaycastHitData {
        block: [1, 2, 3], face: [0, 1, 0], distance: 2.5,
    })));
    s.pin("HostRet::ItemEntities", &HostRet::ItemEntities(Vec::new()));
    s.pin("HostRet::StructureInfo", &HostRet::StructureInfo(Some(Box::new(StructureInfoData {
        bounds: [([-1, 0, -2], [1, 3, 2]); 4],
        connectors: vec![StructureConnectorData { name: "out".into(), kind: "fixture:door".into(), pos: [0, 0, 2], normal: [0, 0, 1] }],
        requirements: vec![StructureRequirementData { min: [-1, -2, -3], max: [1, 2, 3], space: TerrainSpace::Solid }],
    }))));

    // --- GuestCall: every variant, declaration order -------------------------
    s.pin("HostRet::Loot", &HostRet::Loot(Some(Vec::new())));
    s.pin("HostRet::MobDataRows", &HostRet::MobDataRows(vec![(MobId(4), "{}".into())]));
    s.pin("HostRet::MemoClaim (value)", &HostRet::MemoClaim(MemoClaim::Value(b"v".to_vec())));
    s.pin("HostRet::MemoClaim (lease)", &HostRet::MemoClaim(MemoClaim::Lease));
    s.pin("HostRet::MemoClaim (pending)", &HostRet::MemoClaim(MemoClaim::Pending));
    s.pin("HostRet::TerrainHeights", &HostRet::TerrainHeights(vec![1, -2]));
    s.pin("HostRet::MaybeU16", &HostRet::MaybeU16(Some(300)));
    s.pin("HostRet::SectionBlocks", &HostRet::SectionBlocks(vec![1, 0, 9, 0]));
    s.pin("HostRet::TerrainSpaces", &HostRet::TerrainSpaces(vec![TerrainSpace::Air, TerrainSpace::Fluid, TerrainSpace::Solid]));
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
        kind_key: "m:g".into(), widget_id: "w".into(), at: Some(ContainerAddress::Mob(7)),
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
            player_pos: [4.0, 5.0, 6.0], nav_idle: true, in_fluid: Some(BlockId(3)),
            target: Some(EntityRef::Mob(8)), attacker: Some((EntityRef::Player(PlayerId(2)), 3)),
            player_held: Some(ItemId(7)), player_foothold: Some([4, 5, 6]),
            tags: vec![("m:k".into(), MobTagValue::I64(-3))],
        },
    });
    s.pin("GuestCall::ClientFrame", &GuestCall::ClientFrame {
        frame: ClientFrameData {
            dt: 0.05, player_pos: [1.0, 2.0, 3.0], yaw: 0.5, pitch: 0.25,
            screen: [640, 480], open_gui: Some("m:g".into()), open_canvas: None,
            gui_scale: 2, frozen: true, wall_dt: 0.0625,
        },
    });
    s.pin("GuestCall::ClientKey", &GuestCall::ClientKey { action_id: 1, pressed: true });
    s.pin("GuestCall::ClientUi", &GuestCall::ClientUi {
        kind_key: "m:g".into(), event: ClientUiEvent::Click { id: "b".into(), item: None },
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
        outcome: Outcome::Cancel, payload: None,
    });
    s.pin("GuestRet::GenOutput", &GuestRet::GenOutput(vec![([1, 2, 3], BlockId(4))].into()));
    s.pin("GuestRet::GenBlocks", &GuestRet::GenBlocks(vec![1, 2]));
    s.pin("GuestRet::GenBiomes", &GuestRet::GenBiomes(vec![3]));
    s.pin("GuestRet::HostileSpawn", &GuestRet::HostileSpawn(Some("m:k".into())));
    s.pin("GuestRet::AiDecision", &GuestRet::AiDecision(Some(AiNodeDecision {
        goal: Some([1, 2, 3]), head_look: Some([0.5, 0.25]), facing: Some(1.0),
        speed_scale: Some(2.0), idle_anim: Some(1), attack: Some([2.0, 3.0]),
        animation: Some("m".into()), target: Some(EntityRef::Mob(4)),
        claims: ChannelClaims::of(&[DecisionChannel::Attack, DecisionChannel::Target]),
        tags: vec![MobTagWrite { key: "m:k".into(), value: Some(MobTagValue::Bool(true)) }],
    })));
    // The claim mask is a bit set: a sample past bit 6 pins its width.
    s.pin("ChannelClaims (all)", &ChannelClaims::ALL);
    // Registry ids are TWO bytes, and postcard varint-encodes them: any sample
    // below 128 encodes byte-for-byte like the one-byte ids used to, so ONLY a
    // high id pins the width. Without these, silently narrowing `BlockId` /
    // `ItemId` back to `u8` would pass every other pin in this file.
    s.pin("BlockId (wide)", &BlockId(300));
    s.pin("ItemId (wide)", &ItemId(4095));
    s.pin("GuestRet::GenOutput (wide)", &GuestRet::GenOutput(vec![([1, 2, 3], BlockId(300))].into()));
    s.pin("GuestRet::GenOutput (structure)", &GuestRet::GenOutput(GenOutput {
        features: Vec::new(), blocks: Vec::new(), structures: vec![StructurePlacement { template: "fixture:room".into(), origin: [-17, -32, 15], turn: 3 }],
        deferred: false,
    }));
    s.pin("GuestRet::GenOutput (feature)", &GuestRet::GenOutput(GenOutput {
        features: vec![FeaturePlacement { feature: "fixture:tree".into(), origins: vec![[-17, -32, 15]], salt: 7 }],
        ..GenOutput::default()
    }));
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
    s.pin("GuestRet::Unsupported", &GuestRet::Unsupported);

    // --- EventPayload: every variant, declaration order ----------------------
    s.pin("EventPayload::BlockPlacePre", &EventPayload::BlockPlacePre {
        pos: [1, 2, 3], block: BlockId(1), facing: Facing::North, actor: EntityRef::Mob(3),
    });
    s.pin("EventPayload::BlockBreakPre", &EventPayload::BlockBreakPre {
        pos: [1, 2, 3], block: BlockId(1), harvested: true, actor: EntityRef::Player(PlayerId(2)),
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
    s.pin("EventPayload::AttackAttempt", &EventPayload::AttackAttempt {
        block: None, face: None, mob: Some(7), target: Some(PlayerId(2)), player: PlayerId(0),
    });
    s.pin("EventPayload::ProjectileHit", &EventPayload::ProjectileHit {
        entity: 9, item: ItemId(1), target: ProjectileTarget::Mob(7),
        pos: [1.0, 2.0, 3.0], vel: [0.0, 0.0, -4.0], fate: ProjectileFate::Drop,
    });
    s.pin("EventPayload::CellsEditPre", &EventPayload::CellsEditPre {
        min: [1, 2, 3], max: [4, 5, 6], cells: 7, actor: EntityRef::Player(PlayerId(2)),
    });
    s.pin("ProjectileTarget::*", &vec![
        ProjectileTarget::Mob(7), ProjectileTarget::Player(PlayerId(2)),
        ProjectileTarget::Block { pos: [1, 2, 3], face: [0, 1, 0] },
    ]);
    s.pin("ProjectileFate::*", &vec![
        ProjectileFate::Consume, ProjectileFate::Lodge, ProjectileFate::Drop,
    ]);
    s.pin("ItemMotion::*", &vec![
        ItemMotion::Loose, ItemMotion::Flight, ItemMotion::Stuck { cell: [1, 2, 3] },
    ]);
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
        kind: ContainerKind::new("petramond:chest"), at: Some(ContainerAddress::Block([1, 2, 3])),
    });
    s.pin("EventPayload::ContainerClosed", &EventPayload::ContainerClosed {
        kind: ContainerKind::new("m:g"), at: None,
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
            EventKind::UseUnclaimed, EventKind::AttackAttempt, EventKind::ProjectileHit,
            EventKind::ActorActed, EventKind::SchematicChosen, EventKind::SchematicPositioned,
            EventKind::CellsEditPre,
    ]);
    s.pin("DamageSource::*", &vec![
        DamageSource::Fall,
        DamageSource::PlayerAttack { id: PlayerId(1) },
        DamageSource::MobAttack { key: "m:k".into() },
        DamageSource::Mod { mod_id: "m".into() },
        DamageSource::FluidContact { block: BlockId(3) },
        DamageSource::Condition { condition: ConditionId(1) },
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
    s.pin("GuiValue::List", &GuiValue::List(vec![[ ("n".into(),GuiValue::I32(2)) ].into_iter().collect()]));
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
        ClientUiEvent::Click { id: "b".into(), item: Some(2) },
        ClientUiEvent::TextChanged { id: "b".into(), text: "t".into() },
        ClientUiEvent::Submit { id: "b".into(), text: "t".into() },
        ClientUiEvent::ImagePointer {
            id: "b".into(), phase: ClientPointerPhase::Up, x: 1.0, y: 2.0,
            button: ClientPointerButton::Secondary,
        },
        ClientUiEvent::Toggle { id: "t".into(), item: None, on: true },
        ClientUiEvent::Slider { id: "s".into(), item: Some(1), value: 0.5, committed: true },
        ClientUiEvent::ListSelect { id: "l".into(), index: 3 },
        ClientUiEvent::ListActivate { id: "l".into(), index: 3 },
        ClientUiEvent::TabSelect { id: "tb".into(), index: 1 },
        ClientUiEvent::Dismiss,
        ClientUiEvent::Hover { id: Some("h".into()), item: None },
        ClientUiEvent::Blur { id: "i".into(), item: None },
        ClientUiEvent::ListRange { id: "l".into(), first: 4, count: 9 },
        ClientUiEvent::CanvasPointer {
            id: "c".into(), item: None, phase: ClientPointerPhase::Leave, x: 1.0, y: 2.0,
            button: Some(ClientPointerButton::Primary),
            mods: ClientKeyMods { ctrl: true, shift: false, alt: false }, clicks: 2,
        },
        ClientUiEvent::CanvasScroll {
            id: "c".into(), item: Some(1), x: 1.0, y: 2.0, delta: -1.0, mods: ClientKeyMods::default(),
        },
        ClientUiEvent::CanvasSize { id: "c".into(), item: None, w: 320, h: 180 },
    ]);
    s.pin("BlockHookKind::*", &vec![
        BlockHookKind::RandomTick, BlockHookKind::ScheduledTick, BlockHookKind::NeighborUpdate,
    ]);
    s.pin("LightAperture::*", &vec![LightAperture::Opaque, LightAperture::Open]);

    s.pin("HostCall::ResolveCondition", &HostCall::from(calls::ResolveCondition { key: "m:c".into() }));
    s.pin("HostCall::ConditionNames", &HostCall::from(calls::ConditionNames { conditions: vec![ConditionId(1)] }));
    s.pin("HostCall::EntityConditionApply", &HostCall::from(calls::EntityConditionApply { entity: EntityRef::Mob(7), condition: ConditionId(0), stage: 1, ticks: 120 }));
    s.pin("HostCall::EntityConditionCool", &HostCall::from(calls::EntityConditionCool { entity: EntityRef::Player(PlayerId(1)), condition: ConditionId(0), ticks: 3 }));
    s.pin("HostRet::Condition", &HostRet::Condition(Some(ConditionInfoData { id: ConditionId(0), key: "m:c".into(), stages: vec!["a".into()] })));
    s.pin("HostCall::BlockInfos", &HostCall::from(calls::BlockInfos { blocks: vec![BlockId(1), BlockId(9)] }));
    s.pin("HostRet::BlockInfos", &HostRet::BlockInfos(vec![None]));
    s.pin("HostCall::SetPlayerAnimatorParams", &HostCall::from(calls::SetPlayerAnimatorParams {
        player: PlayerId(2),
        params: vec![crate::AnimatorParam { rig: "r".into(), param: "p".into(), value: crate::AnimatorValue::Name("n".into()) }],
    }));
    s.pin("HostCall::SetPlayerAnimatorPlays", &HostCall::from(calls::SetPlayerAnimatorPlays {
        player: PlayerId(2),
        plays: vec![
            crate::AnimatorPlay { rig: "r".into(), slot: "s".into(), clip: "c".into(), clock: crate::AnimatorClock::Scrub(0.5), mirror: true, priority: 1 },
            crate::AnimatorPlay { rig: "r".into(), slot: "t".into(), clip: "c".into(), clock: crate::AnimatorClock::Run { rate: 1.5, looping: true }, mirror: false, priority: 0 },
        ],
    }));
    s.pin("HostCall::FirePlayerAnimatorEvent", &HostCall::from(calls::FirePlayerAnimatorEvent { player: PlayerId(2), rig: "r".into(), event: "e".into() }));
    s.pin("HostCall::AnimationClip", &HostCall::from(calls::AnimationClip { rig: "r".into(), clip: "b".into() }));
    s.pin("HostRet::AnimationClip", &HostRet::AnimationClip(Some(crate::AnimationClipInfo {
        length: 0.5, looping: false, markers: vec![("impact".into(), 0.25)],
    })));
    let record = crate::BlockRecord {
        block: "m:b".into(), state: vec![1, 0, 0], refs: vec![(1, "m:c".into())],
        data: vec![("m:k".into(), vec![7])],
    };
    s.pin("HostCall::ContainerTransfer", &HostCall::from(calls::ContainerTransfer {
        from: ContainerAddress::Block([1, 2, 3]), slot: 2, to: ContainerAddress::Mob(7), count: 5,
    }));
    s.pin("HostCall::BlockRecordsAt", &HostCall::from(calls::BlockRecordsAt { positions: vec![[1, 2, 3]] }));
    s.pin("HostCall::BlockRecordPlans", &HostCall::from(calls::BlockRecordPlans { records: vec![record.clone()] }));
    s.pin("HostCall::BlockRecordStatuses", &HostCall::from(calls::BlockRecordStatuses { cells: vec![([1, 2, 3], record.clone())] }));
    s.pin("HostCall::ActorDig", &HostCall::from(calls::ActorDig {
        actor: EntityRef::Mob(7), pos: [1, 2, 3], tool_slot: Some(4), collect: true,
    }));
    s.pin("HostCall::ActorPlace", &HostCall::from(calls::ActorPlace {
        actor: EntityRef::Mob(7), pos: [1, 2, 3], record: record.clone(), pay: true,
    }));
    s.pin("HostCall::PathProbe", &HostCall::from(calls::PathProbe {
        key: "m:g".into(), from: [1, 2, 3], to: [4, 5, 6], blocked: vec![[2, 2, 3]], max_nodes: 900,
    }));
    s.pin("HostCall::Footholds", &HostCall::from(calls::Footholds { key: "m:g".into(), cells: vec![[1, 2, 3]] }));
    s.pin("HostCall::MobHeldDisplay", &HostCall::from(calls::MobHeldDisplay {
        mob_id: 7, main: Some("m:i".into()), off: None,
    }));
    s.pin("HostCall::SchematicInfo", &HostCall::from(calls::SchematicInfo { asset: [3; 32] }));
    s.pin("HostCall::SchematicCells", &HostCall::from(calls::SchematicCells { asset: [3; 32], section: 2, turns: 1 }));
    s.pin("HostCall::SchematicChoose", &HostCall::from(calls::SchematicChoose { player: PlayerId(2), tag: "m:t".into() }));
    s.pin("HostCall::SchematicPosition", &HostCall::from(calls::SchematicPosition {
        player: PlayerId(2), tag: "m:t".into(), asset: [3; 32], origin: Some([1, 2, 3]), turns: 2,
    }));
    s.pin("HostCall::SchematicGhostSet", &HostCall::from(calls::SchematicGhostSet {
        key: "m:g".into(),
        ghost: Some(crate::SchematicGhostData { asset: [3; 32], origin: [1, 2, 3], turns: 1, viewers: vec![PlayerId(2)], yields_to_positioning: true }),
    }));
    s.pin("HostRet::BlockRecords", &HostRet::BlockRecords(vec![Some(record.clone()), None]));
    s.pin("HostRet::RecordPlans", &HostRet::RecordPlans(vec![
        crate::RecordPlan::Air,
        crate::RecordPlan::Member { anchor: [0, -1, 0] },
        crate::RecordPlan::Unit {
            cost: vec![ItemStackData { item: "m:i".into(), count: 2, data: Vec::new() }],
            footprint: vec![[0, 0, 0], [0, 1, 0]],
        },
        crate::RecordPlan::Unsupported { reason: "r".into() },
    ]));
    s.pin("HostRet::RecordStatuses", &HostRet::RecordStatuses(vec![
        crate::RecordStatus::Unloaded,
        crate::RecordStatus::Satisfied,
        crate::RecordStatus::Place { missing: vec![ItemStackData { item: "m:i".into(), count: 1, data: Vec::new() }] },
        crate::RecordStatus::Clear { at: [1, 2, 3], block: BlockId(4), footprint: vec![[1, 2, 3]], holds_items: true },
        crate::RecordStatus::Pending { anchor: [1, 1, 3] },
        crate::RecordStatus::Unsupported { reason: "r".into() },
    ]));
    s.pin("HostRet::Dig", &HostRet::Dig(crate::DigProgress::Digging { progress: 0.5 }));
    s.pin("HostRet::Place", &HostRet::Place(crate::PlaceRequest::Refused(crate::ActionRefusal::NoFace)));
    s.pin("HostRet::Route", &HostRet::Route(Some(crate::Route::Undecided)));
    s.pin("HostRet::Schematic", &HostRet::Schematic(crate::SchematicLookup::Ready(crate::SchematicInfoData {
        title: "t".into(), size: [1, 2, 3], cells: 5, sections: 1,
    })));
    s.pin("HostRet::SchematicCells", &HostRet::SchematicCells(Some(crate::SchematicCellsData {
        cells: vec![([1, 2, 3], 0)], palette: vec![record.clone()],
    })));
    s.pin("HostCall::PlayerIdentity", &HostCall::from(calls::PlayerIdentity { player: PlayerId(2) }));
    s.pin("HostRet::Identity", &HostRet::Identity(Some(crate::PlayerIdentityData {
        name: "p".into(), operator: true,
    })));
    s.pin("HostCall::ActorPlaceCheck", &HostCall::from(calls::ActorPlaceCheck {
        actor: EntityRef::Mob(7), from: [1.5, 2.0, 3.5], pos: [1, 2, 3], record: record.clone(), pay: true,
    }));
    s.pin("HostCall::WalkRegion", &HostCall::from(calls::WalkRegion {
        key: "m:g".into(), from: [1, 2, 3], min: [0, 0, 0], max: [4, 5, 6], blocked: vec![[2, 2, 2]], toward: true, max_nodes: 500,
    }));
    s.pin("HostRet::Flood", &HostRet::Flood(crate::Flood::Reached(vec![([1, 2, 3], 4)])));
    s.pin("HostCall::ActorInteract", &HostCall::from(calls::ActorInteract { actor: EntityRef::Mob(7), pos: [1, 2, 3] }));
    s.pin("HostCall::ActorAims", &HostCall::from(calls::ActorAims {
        actor: EntityRef::Mob(7), from: vec![[1.5, 2.0, 3.5]], pos: [1, 2, 3], record: None,
    }));
    s.pin("HostCall::SetMobDraw", &HostCall::from(calls::SetMobDraw {
        mob_id: 7, frame: crate::DrawFrame::World,
        prims: vec![crate::DrawPrim::Sprite {
            at: [0.0, 2.0, 0.0], scale: 0.5, yaw: 1.5, pitch: 0.25, spin: 2.0,
            bob: [0.125, 4.0], faces_viewer: true,
            tile: "m:t".into(), tint: [1, 2, 3], emissive: true,
        }],
    }));
    s.pin("HostRet::Aims", &HostRet::Aims(vec![Ok([1.5, 2.0, 3.5]), Err(crate::ActionRefusal::NotAimed)]));
    s.pin("HostCall::BlockChangesSince", &HostCall::from(calls::BlockChangesSince { since: Some(7) }));
    s.pin("HostRet::BlockChanges", &HostRet::BlockChanges(crate::BlockChanges { next: 9, lost: true, cells: vec![[1, 2, 3]] }));
    s.pin("HostCall::ContainerHold", &HostCall::from(calls::ContainerHold {
        at: crate::ContainerAddress::Block([1, 2, 3]), actor: EntityRef::Mob(7), open: true,
    }));
    s.pin("EventPayload::ActorActed", &EventPayload::ActorActed {
        actor: EntityRef::Mob(7), pos: [1, 2, 3], action: crate::ActorAction::Place,
        refusal: Some(crate::ActionRefusal::MissingItems),
    });
    s.pin("EventPayload::SchematicChosen", &EventPayload::SchematicChosen {
        player: PlayerId(2), tag: "m:t".into(), asset: [3; 32],
    });
    s.pin("EventPayload::SchematicPositioned", &EventPayload::SchematicPositioned {
        player: PlayerId(2), tag: "m:t".into(), asset: [3; 32], origin: [1, 2, 3], turns: 1,
    });
    s.pin("HostRet::Unsupported", &HostRet::Unsupported);
    s.pin("HostCall::ActingPlayer", &HostCall::from(calls::ActingPlayer));
    s.pin("HostCall::PlayerStateOf", &HostCall::from(calls::PlayerStateOf { player: PlayerId(2) }));
    s.pin("HostCall::ApplyKnockbackTo", &HostCall::from(calls::ApplyKnockbackTo { player: PlayerId(2), impulse: [1.0, 2.0, 3.0] }));
    s.pin("HostCall::SetHealthOf", &HostCall::from(calls::SetHealthOf { player: PlayerId(2), value: 20 }));
    s.pin("HostCall::TeleportPlayer", &HostCall::from(calls::TeleportPlayer { player: PlayerId(2), pos: [1.0, 2.0, 3.0] }));
    s.pin("HostCall::EffectApplyTo", &HostCall::from(calls::EffectApplyTo { player: PlayerId(2), key: "m:e".into(), ticks: 40 }));
    s.pin("HostCall::EffectsActiveOf", &HostCall::from(calls::EffectsActiveOf { player: PlayerId(2) }));
    s.pin("HostCall::ConsumeHeldBy", &HostCall::from(calls::ConsumeHeldBy { player: PlayerId(2), item: ItemId(5), count: 3 }));
    s.pin("HostCall::ReplaceHeldOneBy", &HostCall::from(calls::ReplaceHeldOneBy { player: PlayerId(2), item: ItemId(5), replacement: "m:i".into() }));
    s.pin("HostCall::GuiStateGetFor", &HostCall::from(calls::GuiStateGetFor { player_id: PlayerId(2), key: "k".into() }));
    s.pin("HostCall::GuiOpenFor", &HostCall::from(calls::GuiOpenFor {
        player_id: PlayerId(2), kind_key: "m:g".into(), at: Some(crate::ContainerAddress::Block([1, 2, 3])),
    }));
    s.pin("HostCall::GuiCloseFor", &HostCall::from(calls::GuiCloseFor { player_id: PlayerId(2) }));
    s.pin("HostRet::ActingPlayer", &HostRet::ActingPlayer(Some(PlayerId(2))));
    s.pin("HostRet::PlayerOf", &HostRet::PlayerOf(None));
    s.pin("HostRet::EffectsOf", &HostRet::EffectsOf(Some(vec![EffectStateData { key: "m:e".into(), remaining: 40 }])));
    s.pin("GuestCall::AiNodeBatch", &GuestCall::AiNodeBatch {
        callback_id: 1,
        ctxs: vec![AiNodeCtx {
            mob_id: 1, pos: [1.0, 2.0, 3.0], cell: [1, 2, 3], yaw: 0.5,
            tick: 9, player_id: PlayerId(2),
            player_pos: [4.0, 5.0, 6.0], nav_idle: true, in_fluid: Some(BlockId(3)),
            target: Some(EntityRef::Mob(8)), attacker: Some((EntityRef::Player(PlayerId(2)), 3)),
            player_held: Some(ItemId(7)), player_foothold: Some([4, 5, 6]),
            tags: vec![("m:k".into(), MobTagValue::I64(-3))],
        }],
    });
    s.pin("GuestRet::AiDecisions", &GuestRet::AiDecisions(vec![
        Some(AiNodeDecision {
            goal: Some([1, 2, 3]), head_look: Some([0.5, 0.25]), facing: Some(1.0),
            speed_scale: Some(2.0), idle_anim: Some(1), attack: Some([2.0, 3.0]),
            animation: Some("m".into()), target: Some(EntityRef::Mob(4)),
            claims: ChannelClaims::of(&[DecisionChannel::Attack, DecisionChannel::Target]),
            tags: vec![MobTagWrite { key: "m:k".into(), value: Some(MobTagValue::Bool(true)) }],
        }),
        None,
    ]));
    s.pin("HostCall::LightAtMany", &HostCall::from(calls::LightAtMany { positions: vec![[1, 2, 3]] }));
    s.pin("HostCall::MobTagsGetMany", &HostCall::from(calls::MobTagsGetMany { mob_ids: vec![7] }));
    s.pin("HostCall::MobTagsWrite", &HostCall::from(calls::MobTagsWrite { writes: vec![MobTagOp::Delete { mob_id: 7, key: "m:k".into() }] }));
    s.pin("HostCall::MobDriveMany", &HostCall::from(calls::MobDriveMany { drives: vec![MobDriveData::horizontal(7, [1.0, 2.0], None)] }));
    s.pin("HostCall::MobKinematicMany", &HostCall::from(calls::MobKinematicMany { poses: vec![MobKinematicData { mob_id: 7, pos: [1.0, 2.0, 3.0], yaw: 0.0, pitch: 0.0, roll: 0.0 }] }));
    s.pin("HostCall::MobAnimMany", &HostCall::from(calls::MobAnimMany { ops: vec![MobAnimOp::Set { mob_id: 7, anim: "a".into(), active: true }] }));
    s.pin("HostCall::MobRidersMany", &HostCall::from(calls::MobRidersMany { mob_ids: vec![7] }));
    s.pin("HostCall::PlayerInputs", &HostCall::from(calls::PlayerInputs { player_ids: vec![PlayerId(2)] }));
    s.pin("HostCall::EntityConditionsMany", &HostCall::from(calls::EntityConditionsMany { ops: vec![ConditionOp::Cool { entity: EntityRef::Mob(7), condition: ConditionId(1), ticks: 2 }] }));
    s.pin("HostRet::Lights", &HostRet::Lights(vec![None]));
    s.pin("HostRet::MobTagsMany", &HostRet::MobTagsMany(vec![None]));
    s.pin("HostRet::PlayerInputs", &HostRet::PlayerInputs(vec![None]));
    s.pin("HostRet::RidersMany", &HostRet::RidersMany(vec![None]));

    s.pin("HostCall::ClientViewCameraSet", &HostCall::from(calls::ClientViewCameraSet {
        pos: [1.5, 2.0, 3.5], yaw: 0.25, pitch: -0.5, roll: 0.125, fov_y: Some(1.25),
        anchor: Some(EntityRef::Mob(9)),
    }));
    s.pin("HostCall::ClientViewCameraRelease", &HostCall::from(calls::ClientViewCameraRelease));
    s.pin("HostCall::ClientViewChromeSet", &HostCall::from(calls::ClientViewChromeSet {
        hud: Some(false), hands: None, crosshair: Some(true),
    }));
    s.pin("HostCall::ClientViewPerspectiveSet", &HostCall::from(calls::ClientViewPerspectiveSet {
        third_person: Some(true),
    }));
    s.pin("HostCall::ClientViewState", &HostCall::from(calls::ClientViewState));
    s.pin("HostRet::ClientViewState", &HostRet::ClientViewState(crate::ClientViewStateData {
        third_person: true, hud_visible: false, hands_visible: true, crosshair_visible: false,
        camera_claimed: true, fov_y: 1.25, pos: [1.0, 2.0, 3.0], yaw: 0.5, pitch: 0.25,
        roll: 0.125, frame_size: [1920, 1080], subject: Some(PlayerId(2)), anchor_missing: true,
        world_settled: false,
    }));
    s.pin("HostCall::ClientEnvSet", &HostCall::from(calls::ClientEnvSet {
        params: vec![("m:k".into(), [1.0, 2.0, 3.0, 4.0])],
    }));
    s.pin("HostCall::ClientWorldMarksSet", &HostCall::from(calls::ClientWorldMarksSet {
        set: "path".into(),
        marks: vec![ClientWorldMark::Line {
            from: [1.5, 2.0, 3.5], to: [-4.0, 5.0, 6.25], color: [1, 2, 3, 4], width: 2.0, occluded: 64,
        }],
    }));
    s.pin("HostCall::ClientContext", &HostCall::from(calls::ClientContext));
    s.pin("HostRet::ClientContext", &HostRet::ClientContext(crate::ClientContext::Presentation {
        owner: "m".into(),
    }));
    s.pin("ClientContext::Remote", &crate::ClientContext::Remote {
        name: "s".into(), presentation_packs: false,
    });
    s.pin("ClientContext::Local", &crate::ClientContext::Local { name: "w".into(), shared: true });
    s.pin("ClientCanvasElement::Rect", &ClientCanvasElement::Rect {
        rect: [1.0, 2.0, 3.0, 4.0], color: [1, 2, 3, 4], filled: true,
    });
    s.pin("ClientCanvasElement::Text", &ClientCanvasElement::Text {
        pos: [1.0, 2.0], text: "t".into(), color: [1, 2, 3, 4], small: true, max_w: Some(40.0),
    });
    s.pin("ClientWorldMark::Point", &ClientWorldMark::Point {
        pos: [1.0, 2.0, 3.0], sprite: Some(ClientSprite::Theme { part: "icon.plus".into() }),
        size: 16.0, color: [1, 2, 3, 4], label: Some("a".into()), occluded: 255,
    });
    s.pin("ClientSprite::Image", &ClientSprite::Image { key: "m:i".into() });
    s.pin("HostCall::ClientStorageWritePoll", &HostCall::from(calls::ClientStorageWritePoll {
        scope: ClientStorageScope::World, ticket: 3,
    }));
    s.pin("HostRet::ClientStorageWrite", &HostRet::ClientStorageWrite(7));
    s.pin("HostRet::ClientStorageWritten", &HostRet::ClientStorageWritten(true));
    s.pin("HostCall::ClientUiFocus", &HostCall::from(calls::ClientUiFocus { id: "f".into(), item: Some(1) }));
    s.pin("HostCall::ClientPauseOpen", &HostCall::from(calls::ClientPauseOpen));
    s.pin("HostCall::ClientViewFrameSet", &HostCall::from(calls::ClientViewFrameSet { size: Some([1920, 1080]) }));
    s.pin("HostCall::ClientViewSubjectSet", &HostCall::from(calls::ClientViewSubjectSet { player: Some(PlayerId(3)) }));
    s.pin("HostCall::ClientEntities", &HostCall::from(calls::ClientEntities {
        ids: vec![EntityRef::Player(PlayerId(1))],
        near: Some(ClientEntitiesNear { center: [1.0, 2.0, 3.0], radius: 16.0, max: 8 }),
    }));
    s.pin("HostRet::ClientEntities", &HostRet::ClientEntities(vec![ClientEntityData {
        id: EntityRef::Mob(4), mob_kind: Some(MobId(2)), name: Some("n".into()),
        feet: [1.0, 2.0, 3.0], eye: [1.0, 3.5, 3.0], look: [0.0, 0.0, 1.0],
        body_facing: [0.0, 1.0], velocity: [0.5, 0.0, 0.0], size: [0.6, 1.8],
    }]));
    s.pin("HostCall::ClientKeyLabels", &HostCall::from(calls::ClientKeyLabels { ids: vec!["open_map".into()] }));

    // --- files, world capture, presentation, frames/clock/taps/media, facts ---
    let pose = crate::ClientPose { pos: [1.5, 70.0, -3.25], yaw: 0.5, pitch: -0.25 };
    let ranges = crate::ClientFileRanges {
        scope: ClientStorageScope::Pack, path: "r/w.pmc".into(), ranges: vec![[40, 80], [200, 12]],
    };
    s.pin("HostCall::ClientFileAppend", &HostCall::from(calls::ClientFileAppend {
        scope: ClientStorageScope::Pack, path: "a/b".into(), bytes: vec![1, 2, 3],
    }));
    s.pin("HostCall::ClientFileWrite", &HostCall::from(calls::ClientFileWrite {
        scope: ClientStorageScope::World, path: "a".into(), offset: 300, bytes: vec![9], truncate: true,
    }));
    s.pin("HostCall::ClientFileSync", &HostCall::from(calls::ClientFileSync { scope: ClientStorageScope::Pack, path: "a".into() }));
    s.pin("HostCall::ClientFileRename", &HostCall::from(calls::ClientFileRename {
        scope: ClientStorageScope::Pack, from: "a.tmp".into(), to: "a".into(),
    }));
    s.pin("HostCall::ClientFileDelete", &HostCall::from(calls::ClientFileDelete { scope: ClientStorageScope::Pack, path: "r".into() }));
    s.pin("HostCall::ClientFileRead", &HostCall::from(calls::ClientFileRead {
        scope: ClientStorageScope::Pack, path: "a".into(), offset: 1 << 33, len: 4096,
    }));
    s.pin("HostCall::ClientFileList", &HostCall::from(calls::ClientFileList {
        scope: ClientStorageScope::Pack, dir: "".into(), after: Some("r3".into()), max_bytes: 65536,
    }));
    s.pin("HostCall::ClientFilePoll", &HostCall::from(calls::ClientFilePoll { ticket: 7 }));
    s.pin("HostCall::ClientFileStat", &HostCall::from(calls::ClientFileStat { scope: ClientStorageScope::Pack, path: "a".into() }));
    s.pin("HostCall::ClientFileReveal", &HostCall::from(calls::ClientFileReveal { scope: ClientStorageScope::Pack, path: "videos".into() }));
    s.pin("HostCall::ClientFolderChoose", &HostCall::from(calls::ClientFolderChoose { folder: 2, title: "Videos".into() }));
    s.pin("HostCall::ClientFolderState", &HostCall::from(calls::ClientFolderState { folder: 2 }));
    s.pin("ClientStorageScope::Chosen", &ClientStorageScope::Chosen(2));
    s.pin("HostCall::ClientWorldStateWrite", &HostCall::from(calls::ClientWorldStateWrite {
        scope: ClientStorageScope::Pack, path: "r/w.pmc".into(),
        select: crate::ClientStateSelect::Keys(vec![
            crate::ClientStateKey::Section([1, -2, 3]), crate::ClientStateKey::Column([4, -5]),
            crate::ClientStateKey::Presence, crate::ClientStateKey::Population,
            crate::ClientStateKey::Session, crate::ClientStateKey::Tables,
            crate::ClientStateKey::Clock, crate::ClientStateKey::Mob(9),
            crate::ClientStateKey::Item(10), crate::ClientStateKey::Player(PlayerId(2)),
            crate::ClientStateKey::Roster, crate::ClientStateKey::Environment,
            crate::ClientStateKey::Activity, crate::ClientStateKey::Viewer,
        ]),
        kinds: Some(vec![crate::ClientPieceKind::Section, crate::ClientPieceKind::View]),
        envelopes: Some("r/w.env".into()),
    }));
    s.pin("ClientStateSelect::ChangedSince", &crate::ClientStateSelect::ChangedSince(1 << 40));
    s.pin("HostCall::ClientWorldEventsBegin", &HostCall::from(calls::ClientWorldEventsBegin {
        scope: ClientStorageScope::Pack, path: "r/e.pmc".into(), envelopes: None,
    }));
    s.pin("HostCall::ClientWorldEventsEnd", &HostCall::from(calls::ClientWorldEventsEnd { events: 3 }));
    s.pin("HostCall::ClientWorldEventsPoll", &HostCall::from(calls::ClientWorldEventsPoll { events: 3 }));
    s.pin("HostCall::ClientPresentationOpen", &HostCall::from(calls::ClientPresentationOpen {
        tables: crate::ClientFileRange {
            scope: ClientStorageScope::Pack, path: "r/w.pmc".into(), offset: 80, len: 900,
        },
        seed: 0xdead_beef, mods: vec!["m".into()], viewer: Some(pose),
    }));
    s.pin("HostCall::ClientPresentationApply", &HostCall::from(calls::ClientPresentationApply {
        state: vec![ranges.clone()], events: vec![ranges.clone()], at: 12.75,
    }));
    s.pin("HostCall::ClientPresentationCancel", &HostCall::from(calls::ClientPresentationCancel { apply: 4 }));
    s.pin("HostCall::ClientPresentationQueue", &HostCall::from(calls::ClientPresentationQueue { events: vec![ranges] }));
    s.pin("HostCall::ClientPresentationTime", &HostCall::from(calls::ClientPresentationTime { at: 13.5 }));
    s.pin("HostCall::ClientPresentationViewer", &HostCall::from(calls::ClientPresentationViewer { pose, flying: true }));
    s.pin("HostCall::ClientPresentationState", &HostCall::from(calls::ClientPresentationState));
    s.pin("HostCall::ClientPresentationClose", &HostCall::from(calls::ClientPresentationClose));
    s.pin("HostCall::ClientFrameCapture", &HostCall::from(calls::ClientFrameCapture {
        source: crate::ClientCaptureSource::World, size: Some([640, 360]),
        when: crate::ClientCaptureWhen::Settled, advance: true,
        into: crate::ClientCaptureInto::File { scope: ClientStorageScope::Pack, path: "t.raw".into() },
    }));
    s.pin("ClientCaptureInto::Media", &crate::ClientCaptureInto::Media(5));
    s.pin("HostCall::ClientFrameCapturePoll", &HostCall::from(calls::ClientFrameCapturePoll { capture: 6 }));
    s.pin("HostCall::ClientFrameCancel", &HostCall::from(calls::ClientFrameCancel { capture: 6 }));
    s.pin("HostCall::ClientClockSet", &HostCall::from(calls::ClientClockSet {
        step: Some(crate::ClientStep { seconds: 1.0 / 60.0, seed: 77 }),
    }));
    s.pin("HostCall::ClientClockAdvance", &HostCall::from(calls::ClientClockAdvance));
    s.pin("HostCall::ClientAudioTap", &HostCall::from(calls::ClientAudioTap {
        sample_rate: 48_000, channels: 2, into: crate::ClientAudioInto::Media(5),
    }));
    s.pin("HostCall::ClientAudioTapState", &HostCall::from(calls::ClientAudioTapState { tap: 8 }));
    s.pin("HostCall::ClientAudioTapEnd", &HostCall::from(calls::ClientAudioTapEnd { tap: 8 }));
    s.pin("HostCall::ClientMediaEncoders", &HostCall::from(calls::ClientMediaEncoders { refresh: true }));
    s.pin("HostCall::ClientMediaOpen", &HostCall::from(calls::ClientMediaOpen {
        scope: ClientStorageScope::Pack, path: "videos/a.mp4".into(), container: "mp4".into(),
        video: Some(crate::ClientMediaVideo {
            codec: "libx264".into(), width: 1920, height: 1080, fps: [60, 1],
            options: vec![("crf".into(), "18".into())],
        }),
        audio: Some(crate::ClientMediaAudio {
            codec: "aac".into(), sample_rate: 48_000, channels: 2, options: Vec::new(),
        }),
        options: vec![("movflags".into(), "+faststart".into())],
    }));
    s.pin("HostCall::ClientMediaPushFrame", &HostCall::from(calls::ClientMediaPushFrame { media: 5, rgba: vec![1, 2, 3, 4] }));
    s.pin("HostCall::ClientMediaPushAudio", &HostCall::from(calls::ClientMediaPushAudio { media: 5, pcm: vec![0, 0, 128, 63] }));
    s.pin("HostCall::ClientMediaClose", &HostCall::from(calls::ClientMediaClose { media: 5 }));
    s.pin("HostCall::ClientMediaAbort", &HostCall::from(calls::ClientMediaAbort { media: 5 }));
    s.pin("HostCall::ClientMediaState", &HostCall::from(calls::ClientMediaState { media: 5 }));
    s.pin("HostCall::ClientEngineFacts", &HostCall::from(calls::ClientEngineFacts));
    s.pin("HostCall::ClientPacks", &HostCall::from(calls::ClientPacks));
    s.pin("HostCall::ClientWallClock", &HostCall::from(calls::ClientWallClock));

    s.pin("HostRet::Ticket", &HostRet::Ticket(4));
    s.pin("HostRet::ClientStateTicket", &HostRet::ClientStateTicket(crate::ClientStateTicketData {
        write: 3, revision: 1 << 40,
    }));
    s.pin("HostRet::ClientFilePolled", &HostRet::ClientFilePolled(Some(crate::ClientFileAnswer::Done {
        range: Some([40, 900]), envelope: Some([800, 140]),
    })));
    s.pin("ClientFileAnswer::Read", &crate::ClientFileAnswer::Read(vec![5, 6]));
    s.pin("ClientFileAnswer::Listing", &crate::ClientFileAnswer::Listing {
        entries: vec![crate::ClientFileEntry { name: "r1".into(), dir: true, len: 0, modified_unix_ms: 1_700_000_000_000 }],
        more: true,
    });
    s.pin("HostRet::ClientFileStat", &HostRet::ClientFileStat(Some(crate::ClientFileInfo {
        len: 900, written: 800, modified_unix_ms: 5, error: Some("e".into()),
    })));
    s.pin("HostRet::ClientEvents", &HostRet::ClientEvents(Some(crate::ClientEventsReport {
        phase: crate::ClientEventsPhase::Ending, error: None, frames: 9, written_through: 4096, backlog_bytes: 12,
    })));
    s.pin("HostRet::ClientPresentationState", &HostRet::ClientPresentationState(Box::new(
        crate::ClientPresentationStateData {
            opening: false, open: true, owner: Some("m".into()), position: 12.75,
            released_through: Some(13), ready: true, pending: vec![5], applied: 4,
            exhausted: false, error: Some("e".into()),
        },
    )));
    s.pin("HostRet::ClientCaptureStatus", &HostRet::ClientCaptureStatus(crate::ClientCaptureStatus::Delivered {
        time: 2.5, width: 640, height: 360, range: Some([0, 921_600]),
    }));
    s.pin("HostRet::ClientAudioTapState", &HostRet::ClientAudioTapState(Some(crate::ClientAudioTapData {
        started_at: Some(2.5), frames: 48_000, ended: true, error: Some("e".into()),
    })));
    s.pin("HostRet::ClientMediaEncoders", &HostRet::ClientMediaEncoders(Some(crate::ClientMediaCapabilities {
        encoder: Some("ffmpeg version 7".into()), containers: vec!["mp4".into()],
        video_codecs: vec!["libx264".into()], audio_codecs: vec!["aac".into()],
    })));
    s.pin("HostRet::ClientMediaState", &HostRet::ClientMediaState(Some(Box::new(crate::ClientMediaStateData {
        phase: crate::ClientMediaPhase::Failed, frames_in: 60, frames_encoded: 58, audio_in: 48_000,
        bytes: 0, failure: Some(crate::ClientMediaFailure::DiskFull), error: Some("e".into()),
    }))));
    s.pin("HostRet::ClientEngineFacts", &HostRet::ClientEngineFacts(crate::ClientEngineFactsData {
        capture_format: 1, protocol: 54, vocabulary: 0x0123_4567_89ab_cdef, tick_dt: 0.05,
        max_frame_side: 16384, max_frame_bytes: 1 << 30, guest_memory_max: 1 << 32,
    }));
    s.pin("HostRet::ClientPacks", &HostRet::ClientPacks(vec![crate::ClientPackInfo {
        id: "m".into(), version: "1.0".into(), affects_world: true, client_wasm: false,
    }]));
    s.pin("HostRet::ClientWallClock", &HostRet::ClientWallClock(crate::ClientWallTime {
        unix_ms: 1_758_844_800_000, utc_offset_min: -420,
    }));
    s.pin("HostRet::ClientFolder", &HostRet::ClientFolder(Some(crate::ClientFolderInfo { label: "/v".into() })));
    s.pin("ClientFileAnswer::Folder", &crate::ClientFileAnswer::Folder(None));
    s.pin("ClientEnvelopeEntry", &crate::ClientEnvelopeEntry {
        record: [0, 300],
        envelope: crate::ClientEnvelope::Frame(crate::ClientFrameEnvelope {
            seq: 1, revision: 2, presented_tick: 3.5, batches: vec![3],
            view: Some(crate::ClientCapturedView {
                player: PlayerId(1), pos: [1.0, 2.0, 3.0], yaw: 0.5, pitch: 0.25, roll: 0.0, fov_y: 1.2,
            }),
            pieces: vec![crate::ClientPieceInfo {
                kind: crate::ClientPieceKind::BatchRows, key: None, batch: Some(3), range: [40, 60],
                crc: 0xfeed_f00d, provisional: false,
            }],
            touched: vec![crate::ClientStateKey::Section([0, 4, 0])],
            removed: vec![crate::ClientStateKey::Column([2, 2])],
        }),
    });
    s.pin("ClientEnvelope::State", &crate::ClientEnvelope::State(crate::ClientStateEnvelope {
        revision: 2, since: Some(1), tick: 3, presented_tick: 3.5,
        pieces: vec![crate::ClientPieceInfo {
            kind: crate::ClientPieceKind::Section, key: Some(crate::ClientStateKey::Section([1, 2, 3])),
            batch: None, range: [40, 60], crc: 1, provisional: true,
        }],
        absent: vec![crate::ClientStateKey::Mob(3)],
    }));
    s.pin("ClientPresence", &crate::ClientPresence {
        cy_min: -4, columns: vec![crate::ClientColumnPresence { pos: [1, -1], sections: vec![0b1011] }],
    });
    s.pin("ClientPopulation", &crate::ClientPopulation { mobs: vec![1], items: vec![2], players: vec![PlayerId(0)] });
    s.pin("ClientCapturedSession", &crate::ClientCapturedSession {
        seed: 7, local_player: PlayerId(0),
        mods: vec![crate::ClientCapturedMod { id: "m".into(), version: "1".into(), affects_world: true }],
    });
    s.pin("ClientCapturedClock", &crate::ClientCapturedClock { tick: 9, day_clock: 1200 });
    s.pin("ClientRosterEntry", &crate::ClientRosterEntry { player: PlayerId(0), name: "p".into() });
    s.pin("ClientCapturedEnv", &crate::ClientCapturedEnv { params: vec![("m:k".into(), [1.0, 0.0, 0.0, 1.0])] });

    s
}
