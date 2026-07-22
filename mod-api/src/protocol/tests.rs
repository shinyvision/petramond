use serde::{Deserialize, Serialize};

use crate::*;

/// The ABI contract both sides rely on: every call/reply enum round-trips
/// through postcard, including nested payloads. (No wire-byte pinning — the encoding is postcard's contract;
/// ours is that encode∘decode is identity.)
#[test]
fn abi_roundtrip_host_and_guest_calls() {
    fn roundtrip<T>(v: T)
    where
        T: Serialize + for<'de> Deserialize<'de> + PartialEq + core::fmt::Debug,
    {
        let bytes = encode(&v).expect("encode");
        let back: T = decode(&bytes).expect("decode");
        assert_eq!(back, v);
    }

    roundtrip(HostCall::Log {
        msg: "hello".into(),
    });
    roundtrip(HostCall::CurrentTick);
    roundtrip(HostCall::RngU64 {
        stream_key: "spawn".into(),
    });
    roundtrip(HostCall::RegisterTickSystem {
        stage: Stage::Spawning,
        attach: AttachSide::After,
        priority: -3,
        system_id: 42,
    });
    roundtrip(HostCall::RegisterEventHandler {
        event: EventKind::BlockPlaced,
        priority: 7,
        handler_id: 9,
    });
    roundtrip(HostCall::GetBlock { pos: [1, -64, 3] });
    roundtrip(HostCall::GetBlocks {
        positions: vec![[0, 0, 0], [1, 2, 3]],
    });
    roundtrip(HostCall::SetBlock {
        pos: [5, 70, -2],
        block: BlockId(3),
    });
    roundtrip(HostCall::SetBlocks {
        blocks: vec![([0, 64, 0], BlockId(1)), ([0, 65, 0], BlockId(0))],
    });
    roundtrip(HostCall::ScheduleTick {
        pos: [9, 60, 9],
        delay: 5,
    });
    roundtrip(HostCall::IsLoaded { pos: [8, 0, 8] });
    roundtrip(HostCall::LightAt { pos: [8, 64, 8] });
    roundtrip(HostCall::SpawnMob {
        key: "zombies:zombie".into(),
        pos: [0.5, 64.0, 0.5],
        yaw: 1.5,
        checked: false,
    });
    roundtrip(HostCall::MobsInRadius {
        pos: [0.0, 64.0, 0.0],
        radius: 16.0,
    });
    roundtrip(HostCall::DamageMob {
        mob_id: 3,
        amount: 2.5,
        origin: Some([1.0, 64.0, 1.0]),
        feedback: Some(crate::events::MobDamageFeedback {
            components: vec![crate::events::MobDamageFeedbackComponent::Immunity { ticks: 10 }],
        }),
    });
    roundtrip(HostCall::DespawnMob { mob_id: 7 });
    roundtrip(HostCall::MobEmitterSet {
        mob_id: 5,
        key: "petramond:burn_light".into(),
        active: true,
    });
    roundtrip(HostCall::EmitterBurst {
        key: "petramond:water_splash".into(),
        pos: [0.5, 64.0, 0.5],
        intensity: 4.5,
    });
    roundtrip(HostCall::SpawnItem {
        item: "petramond:stick".into(),
        count: 4,
        pos: [0.5, 64.0, 0.5],
    });
    roundtrip(HostCall::PlayerState);
    roundtrip(HostCall::DamagePlayer { amount: 4 });
    roundtrip(HostCall::ApplyKnockback {
        impulse: [1.0, 3.0, -1.0],
    });
    roundtrip(HostCall::GiveItem {
        item: "petramond:diamond".into(),
        count: 1,
    });
    roundtrip(HostCall::SetHealth { value: 20 });
    roundtrip(HostCall::Teleport {
        pos: [10.5, 80.0, -4.5],
    });
    roundtrip(HostCall::EmitSound {
        key: "mymod:zap".into(),
        pos: Some([0.0, 64.0, 0.0]),
    });
    roundtrip(HostCall::WorldKvGet {
        key: "petramond:time".into(),
    });
    roundtrip(HostCall::WorldKvSet {
        key: "petramond:time".into(),
        value: vec![1, 2, 3],
    });
    roundtrip(HostCall::WorldKvDelete {
        key: "petramond:time".into(),
    });
    roundtrip(HostCall::SectionKvGet {
        pos: [4, -60, 4],
        key: "farm:moisture".into(),
    });
    roundtrip(HostCall::SectionKvSet {
        pos: [4, -60, 4],
        key: "farm:moisture".into(),
        value: vec![7],
    });
    roundtrip(HostCall::SectionKvDelete {
        pos: [4, -60, 4],
        key: "farm:moisture".into(),
    });
    roundtrip(HostCall::MobTagGet {
        mob_id: 2,
        key: "zombies:target".into(),
    });
    roundtrip(HostCall::MobTagSet {
        mob_id: 2,
        key: "zombies:target".into(),
        value: MobTagValue::Bool(true),
    });
    roundtrip(HostCall::MobTagDelete {
        mob_id: 2,
        key: "zombies:target".into(),
    });
    roundtrip(HostCall::MobTagsGet { mob_id: 2 });
    roundtrip(HostCall::MobsWithTag {
        key: "zombies:target".into(),
        value: Some(MobTagValue::I64(-3)),
    });
    roundtrip(HostCall::MobsWithTag {
        key: "petramond:confined".into(),
        value: None,
    });
    roundtrip(HostCall::ResolveBlock {
        name: "kitchen:oven".into(),
    });
    roundtrip(HostCall::RegisterWorldgenFeature {
        feature_id: 3,
        stage: WorldgenStage::Trees,
    });
    roundtrip(HostCall::RegisterStageReplacement {
        stage: WorldgenStage::Terrain,
        callback_id: 9,
    });
    roundtrip(HostCall::RegisterGenerator { callback_id: 1 });
    roundtrip(HostCall::GuiStateSet {
        key: "wheel:angle".into(),
        value: GuiValue::F32(1.25),
    });
    roundtrip(HostCall::GuiStateGet {
        key: "wheel:result".into(),
    });
    roundtrip(HostCall::GuiOpen {
        kind_key: "wheel:wheel".into(),
    });
    roundtrip(HostCall::GuiClose);
    roundtrip(HostCall::ChatSend {
        text: "$[fg=yellow]Hello".into(),
        targets: None,
    });
    roundtrip(HostCall::ChatSend {
        text: "whisper".into(),
        targets: Some(vec![PlayerId(0), PlayerId(2)]),
    });
    roundtrip(HostCall::SoundPlayAt {
        key: "zombies:groan".into(),
        pos: [4.5, 64.0, -2.5],
        volume: 0.8,
        pitch: 0.95,
    });
    roundtrip(HostCall::SoundPlayOnMob {
        mob_id: 42,
        key: "zombies:groan".into(),
        volume: 0.7,
        pitch: 1.05,
    });
    roundtrip(HostCall::SoundStop { handle: 99 });
    roundtrip(HostCall::CollisionShapeAt { pos: [8, 63, 8] });
    roundtrip(HostRet::CollisionShape(Some(CollisionShape::Partial)));
    roundtrip(HostRet::CollisionShape(None));
    roundtrip(HostCall::ShaderSetParam {
        key: "petramond:light".into(),
        value: [0.75, 0.0, 0.0, 1.0],
    });
    roundtrip(HostCall::RegisterHostileSpawner {
        callback_id: 7,
        priority: -1,
    });
    roundtrip(HostCall::RuntimeSide);
    roundtrip(HostCall::ClientRegisterOverlay {
        image_key: "minimap:hud".into(),
        anchor: ClientOverlayAnchor::TopRight,
        margin: [8, 8],
        display_size: [256, 256],
    });
    roundtrip(HostCall::ClientRegisterKey {
        id: "open_map".into(),
        label: "Open World Map".into(),
        key: "key_m".into(),
        action_id: 1,
    });
    roundtrip(HostCall::ClientSurfaceColumns {
        queries: vec![
            ClientSurfaceQuery {
                coord: [-12, 34],
                revision: 0,
            },
            ClientSurfaceQuery {
                coord: [3, -4],
                revision: 17,
            },
        ],
    });
    roundtrip(HostCall::ClientImageBlit {
        key: "minimap:full_tile_0".into(),
        origin: [32, 64],
        size: [2, 1],
        rgba: vec![1, 2, 3, 255, 4, 5, 6, 255],
    });
    roundtrip(HostCall::ClientUiStateSet {
        key: "minimap:waypoint_name".into(),
        value: GuiValue::Str("Home".into()),
    });
    roundtrip(HostCall::ClientUiStateGet {
        key: "minimap:waypoint_name".into(),
    });
    roundtrip(HostCall::ClientImageSet {
        key: "minimap:hud".into(),
        width: 2,
        height: 1,
        rgba: vec![1, 2, 3, 255, 4, 5, 6, 255],
    });
    roundtrip(HostCall::ClientTextMeasure {
        text: "Waypoint".into(),
        scale: 2,
    });
    roundtrip(HostCall::ClientImageDrawTexts {
        key: "minimap:hud".into(),
        runs: vec![ClientTextRun {
            text: "W".into(),
            position: [4, 9],
            scale: 2,
            color: [255, 255, 255, 255],
        }],
    });
    roundtrip(HostCall::ClientGuiOpen {
        kind_key: "minimap:edit_waypoint".into(),
    });
    roundtrip(HostCall::ClientGuiClose);
    roundtrip(HostCall::ClientCanvasOpen {
        canvas_key: "minimap:full_map".into(),
        size: [640, 640],
    });
    roundtrip(HostCall::ClientCanvasClose);
    roundtrip(HostCall::ClientCanvasSceneSet {
        canvas_key: "minimap:full_map".into(),
        elements: vec![
            ClientCanvasElement::Image {
                image_key: "minimap:tile_0".into(),
                rect: [0.0, 0.0, 160.0, 160.0],
            },
            ClientCanvasElement::Sprite {
                image_key: "minimap:player_arrow".into(),
                center: [160.0, 160.0],
            },
        ],
    });
    roundtrip(HostCall::ClientCanvasViewSet {
        canvas_key: "minimap:full_map".into(),
        offset: [-80.0, 24.0],
    });
    roundtrip(HostCall::ClientStorageReadBegin {
        keys: vec!["minimap:tile:0:0".into(), "minimap:tile:1:0".into()],
    });
    roundtrip(HostCall::ClientStorageReadPoll { ticket: 7 });
    roundtrip(HostCall::ReplaceHeldOne {
        item: ItemId(3),
        replacement: "petramond:water_bucket".into(),
    });
    roundtrip(HostRet::ClientStorageRead(None));
    roundtrip(HostRet::ClientStorageRead(Some(vec![
        Some(ByteBuf::from(vec![1, 2, 3])),
        None,
    ])));
    roundtrip(HostCall::ClientStorageGetMany {
        keys: vec!["minimap:tile/-1/2".into(), "minimap:waypoints".into()],
    });
    roundtrip(HostCall::ClientStorageSetMany {
        entries: vec![("minimap:tile/-1/2".into(), ByteBuf::from(vec![7, 8, 9]))],
    });
    roundtrip(HostRet::RuntimeSide(RuntimeSide::Client));
    roundtrip(HostRet::ClientSurfaceColumns(vec![
        None,
        Some(ClientSurfaceColumn {
            revision: 9,
            cells: None,
        }),
        Some(ClientSurfaceColumn {
            revision: 12,
            cells: Some(vec![71, 0, 42, 96, 31]),
        }),
    ]));
    roundtrip(HostRet::ClientStorageValues(vec![
        Some(ByteBuf::from(vec![3, 1, 4])),
        None,
    ]));
    roundtrip(GuestCall::ClientFrame {
        frame: ClientFrameData {
            dt: 1.0 / 60.0,
            player_pos: [4.5, 72.0, -8.5],
            yaw: 1.25,
            pitch: -0.1,
            screen: [1920, 1080],
            open_gui: None,
            open_canvas: Some("minimap:full_map".into()),
        },
    });
    roundtrip(GuestCall::ClientKey {
        action_id: 2,
        pressed: true,
    });
    roundtrip(GuestCall::ClientUi {
        kind_key: "minimap:edit_waypoint".into(),
        event: ClientUiEvent::ImagePointer {
            id: "map".into(),
            phase: ClientPointerPhase::Move,
            x: 120.5,
            y: 64.25,
            button: ClientPointerButton::Primary,
        },
    });
    roundtrip(GuestCall::ClientCanvas {
        canvas_key: "minimap:full_map".into(),
        event: ClientCanvasEvent {
            phase: ClientPointerPhase::Move,
            x: 120.5,
            y: 64.25,
            button: ClientPointerButton::Primary,
        },
    });
    roundtrip(GuestCall::ClientCanvasScroll {
        canvas_key: "minimap:full_map".into(),
        x: 120.5,
        y: 64.25,
        delta: -2.0,
    });
    roundtrip(HostRet::GuiValue(Some(GuiValue::Str(
        "petramond:diamond".into(),
    ))));
    roundtrip(HostRet::GuiValue(Some(GuiValue::I32(-3))));
    roundtrip(HostRet::GuiValue(None));
    roundtrip(HostRet::MobTag(MobTagLookup::Value(MobTagValue::Bool(true))));
    roundtrip(HostRet::MobTag(MobTagLookup::Absent));
    roundtrip(HostRet::MobTag(MobTagLookup::MissingMob));
    roundtrip(HostRet::MobTags(Some(vec![
        ("farm:quality".into(), MobTagValue::I64(7)),
        ("petramond:confined".into(), MobTagValue::Bool(true)),
    ])));
    roundtrip(HostRet::MobTags(None));
    roundtrip(GuestCall::GuiClick {
        kind_key: "wheel:wheel".into(),
        widget_id: "spin".into(),
        pos: Some([4, 65, -2]),
    });
    let candidate = HostileSpawnCandidate {
        pos: [10.5, 64.0, -2.5],
        cell: [10, 64, -3],
        combined_light: 12,
        sky_light: 8,
        block_light: 12,
        nearest_player_dist: 40.0,
    };
    roundtrip(GuestCall::HostileSpawnCandidate {
        callback_id: 7,
        candidate: candidate.clone(),
    });
    roundtrip(HostCall::RegisterBlockBehavior {
        key: "mymod:zapper".into(),
        callback_id: 3,
    });
    roundtrip(GuestCall::BlockBehavior {
        callback_id: 3,
        kind: BlockHookKind::ScheduledTick,
        pos: [4, 65, -2],
    });
    roundtrip(HostCall::RegisterAiNode {
        key: "mymod:levitate".into(),
        callback_id: 9,
    });
    roundtrip(GuestCall::AiNode {
        callback_id: 9,
        ctx: AiNodeCtx {
            mob_id: 42,
            pos: [1.5, 64.0, -3.5],
            cell: [1, 64, -4],
            yaw: 0.5,
            tick: 1200,
            player_id: PlayerId(1),
            player_pos: [8.0, 65.0, 8.0],
            nav_idle: true,
            in_water: false,
            player_held: Some(ItemId(3)),
            player_foothold: Some([8, 64, 8]),
            tags: vec![
                ("farming:following".into(), MobTagValue::Bool(true)),
                ("farming:sulk_until".into(), MobTagValue::I64(1400)),
            ],
        },
    });
    roundtrip(GuestRet::AiDecision(Some(AiNodeDecision {
        goal: Some([3, 64, 2]),
        head_look: None,
        idle_anim: Some(1),
        attack: Some([2.0, 6.0]),
        tags: vec![
            MobTagWrite {
                key: "farming:sulk_until".into(),
                value: Some(MobTagValue::I64(1400)),
            },
            MobTagWrite {
                key: "farming:following".into(),
                value: None,
            },
        ],
    })));
    roundtrip(EventPayload::ContainerOpened {
        kind: ContainerKind::Mod {
            key: "wheel:wheel".into(),
        },
        pos: None,
    });
    roundtrip(GuestCall::GenFeature {
        feature_id: 3,
        section_pos: [-2, 4, 7],
        seed: 0x312,
        blocks: vec![0; 8],
        surface_heights: vec![63; 4],
        biomes: vec![1; 4],
        sea_level: 63,
    });
    roundtrip(GuestCall::GenStage {
        callback_id: 9,
        stage: WorldgenStage::Climate,
        section_pos: [5, 0, -1],
        seed: 1,
        blocks: Vec::new(),
        surface_heights: vec![70; 2],
        biomes: vec![2; 2],
        sea_level: 63,
    });
    roundtrip(GuestRet::GenWrites(vec![([1, 64, -3], BlockId(7))]));
    roundtrip(GuestRet::GenBlocks(vec![1, 0, 1]));
    roundtrip(GuestRet::GenBiomes(vec![4, 4, 5]));
    roundtrip(GuestRet::HostileSpawn(Some("zombies:zombie".into())));
    roundtrip(GuestRet::HostileSpawn(None));
    roundtrip(HostRet::Unit);
    roundtrip(HostRet::U64(u64::MAX));
    roundtrip(HostRet::Error("nope".into()));
    roundtrip(HostRet::Bool(true));
    roundtrip(HostRet::Block(Some(BlockId(9))));
    roundtrip(HostRet::Blocks(vec![None, Some(BlockId(0))]));
    roundtrip(HostRet::Light(Some(LightData {
        combined: 63,
        sky: 63,
        block: 40,
    })));
    roundtrip(HostRet::Light(None));
    roundtrip(HostRet::Mobs(vec![MobSnapshot {
        index: 0,
        key: "petramond:owl".into(),
        kind: MobId(0),
        pos: [1.5, 64.0, -3.5],
        health: 4.0,
        id: 123,
        yaw: 0.5,
        vel: [1.0, 0.0, -2.0],
    }]));
    roundtrip(HostRet::Player(PlayerSnapshot {
        pos: [0.5, 80.0, 0.5],
        vel: [0.0, -1.0, 0.0],
        yaw: 0.5,
        pitch: -0.25,
        health: 17,
        on_ground: false,
        spectator: false,
        sneak: true,
        held: Some(ItemId(3)),
        held_count: 2,
        pose_anchor: Some([0.5, 64.0, -3.5]),
    }));
    roundtrip(HostRet::Bytes(Some(vec![1, 2, 3])));
    roundtrip(GuestRet::Event {
        outcome: Outcome::Continue,
        payload: EventPayload::PlayerDamagePre {
            amount: 2,
            source: DamageSource::MobAttack {
                key: "zombies:zombie".into(),
            },
            origin: Some([0.0, 80.0, 0.0]),
        },
    });
    roundtrip(GuestCall::TickSystem { id: 3 });
    roundtrip(GuestCall::HandleEvent {
        id: 1,
        payload: EventPayload::MobDamagePre {
            mob_id: 5,
            kind: MobId(1),
            amount: 2.5,
            source: DamageSource::PlayerAttack { id: PlayerId(0) },
            origin: Some([1.0, -2.0, 0.5]),
            feedback: MobDamageFeedback::default(),
        },
    });
    roundtrip(GuestRet::Event {
        outcome: Outcome::Cancel,
        payload: EventPayload::PlayerDamagePre {
            amount: -4,
            source: DamageSource::Fall,
            origin: None,
        },
    });
    roundtrip(EventPayload::ContainerOpened {
        kind: ContainerKind::Furnace,
        pos: Some([1, -64, 3]),
    });
    roundtrip(HostCall::BlockNames {
        blocks: vec![BlockId(0), BlockId(200)],
    });
    roundtrip(HostCall::ItemNames {
        items: vec![ItemId(3)],
    });
    roundtrip(HostRet::Names(vec![Some("petramond:stone".into()), None]));
    roundtrip(HostCall::ResolveMob {
        key: "petramond:sheep".into(),
    });
    roundtrip(HostCall::MobNames {
        mobs: vec![MobId(0), MobId(9)],
    });
    roundtrip(HostRet::MobKind(Some(MobId(2))));
}

#[test]
fn every_payload_kind_is_registerable() {
    // kind() is the dispatch routing key: it must agree with the variant.
    let samples = [
        EventPayload::PlayerDied,
        EventPayload::ItemUsed { item: ItemId(3) },
        EventPayload::SectionLoaded { pos: [0, -2, 5] },
    ];
    for s in samples {
        let bytes = encode(&s).unwrap();
        let back: EventPayload = decode(&bytes).unwrap();
        assert_eq!(back.kind(), s.kind());
    }
}
