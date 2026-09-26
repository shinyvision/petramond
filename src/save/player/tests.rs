use super::*;
use petramond_world::item::ItemType;

fn encode(player: &Player) -> Vec<u8> {
    super::encode(player, &Palette::identity())
}

fn decode(bytes: &[u8]) -> Result<PlayerData, RecordError> {
    super::decode(bytes, &Palette::identity())
}

const V7: &[u8] = include_bytes!("../fixtures/player_v7.bin");
const V8: &[u8] = include_bytes!("../fixtures/player_v8.bin");

#[test]
fn player_file_roundtrips() {
    let mut player = Player::new(WorldPos::new(10.0, 72.0, -4.0));
    player.set_mode(PlayerMode::Spectator);
    player.vel = Vec3::new(0.0, -1.5, 0.25); // after set_mode, which zeroes vel
    player.yaw = 1.25;
    player.pitch = -0.5;
    player.set_health(7);
    player.inventory.set_active(3);
    *player.inventory.off_hand_mut() = Some(ItemStack::new(ItemType::Coal, 9));
    player.bed_spawn = Some(BedSpawn {
        bed: IVec3::new(-3, 70, 12),
        spot: IVec3::new(-2, 70, 13),
    });
    player.apply_effect(petramond_world::effect::Effect::Regeneration, 950);
    player.craft_craftable_only = true;
    player.progression.obtain(ItemType::OakLog);
    player.progression.obtain(ItemType::Coal);
    player.progression.unlock("petramond:oak_planks");
    player.progression.unlock("petramond:torch");

    let bytes = encode(&player);
    let got = decode(&bytes).expect("decodes");

    assert_eq!(got.pos, WorldPos::new(10.0, 72.0, -4.0));
    assert_eq!(got.vel, Vec3::new(0.0, -1.5, 0.25));
    assert_eq!(got.yaw, 1.25);
    assert_eq!(got.pitch, -0.5);
    assert_eq!(got.mode, PlayerMode::Spectator);
    assert_eq!(got.health, 7, "health survives the round-trip");
    assert_eq!(got.inventory.active_slot(), 3);
    assert_eq!(
        got.bed_spawn, player.bed_spawn,
        "the bed spawn survives the round-trip"
    );
    assert_eq!(
        got.effects,
        vec![("petramond:regeneration".to_owned(), 950)],
        "active effects survive the round-trip by name"
    );
    assert_eq!(
        got.inventory.selected().map(|s| s.item),
        player.inventory.selected().map(|s| s.item)
    );
    assert_eq!(
        got.inventory.off_hand().map(|s| (s.item, s.count)),
        Some((ItemType::Coal, 9)),
        "the off-hand slot survives the round-trip"
    );
    assert!(got.kept.is_empty(), "nothing to keep");
    // Progression is the one record a player cannot re-earn by playing:
    // losing it re-hides recipes they already unlocked, and losing the
    // obtained set re-fires `item_obtained` for things they have had for
    // days. Both travel by NAME, and unlock ORDER is the wire catch-up's
    // contract.
    let restored = got.restore();
    assert!(
        restored.craft_craftable_only,
        "the craftable-only browser preference survives the round-trip"
    );
    assert_eq!(
        restored.progression.unlocked(),
        ["petramond:oak_planks", "petramond:torch"],
        "unlocked recipes survive in unlock order"
    );
    assert!(restored.progression.obtained().intersects(
        &[ItemType::OakLog, ItemType::Coal].into_iter().collect()
    ));
    let mut fresh = petramond_world::item::ItemSet::EMPTY;
    fresh.insert(ItemType::Diamond);
    assert!(
        !restored.progression.obtained().intersects(&fresh),
        "an item never held stays unheld"
    );
}

#[test]
fn other_versions_are_typed_errors_not_a_fresh_player() {
    // A player file this build cannot read must never look like "no
    // file": the caller would respawn the player with an empty inventory
    // and save it over the original.
    let mut bytes = encode(&Player::new(WorldPos::new(1.0, 2.0, 3.0)));
    bytes[0..4].copy_from_slice(&(VERSION + 1).to_le_bytes());
    assert!(matches!(
        decode(&bytes),
        Err(RecordError::Newer { found, .. }) if found == VERSION + 1
    ));
    bytes[0..4].copy_from_slice(&(FORMAT.oldest() - 1).to_le_bytes());
    assert!(matches!(decode(&bytes), Err(RecordError::Retired { .. })));
}

#[test]
fn a_truncated_file_is_corrupt() {
    let bytes = encode(&Player::new(WorldPos::new(1.0, 2.0, 3.0)));
    assert!(matches!(
        decode(&bytes[..bytes.len() - 1]),
        Err(RecordError::Corrupt { .. })
    ));
    let v7_short = &V7[..V7.len() - 1];
    assert!(
        matches!(decode(v7_short), Err(RecordError::Corrupt { .. })),
        "a truncated v7 file fails its upgrade as corrupt"
    );
}

/// What the golden fixtures hold, whichever version wrote them: survival
/// at (1.5, 64, -2.5), health 20, no bed, an empty inventory with hotbar
/// slot 2 active, one effect, one obtained item, one recipe. Item slots are
/// all empty so the fixtures hold no palette-dependent id.
fn assert_golden(got: &PlayerData) {
    assert_eq!(got.pos, WorldPos::new(1.5, 64.0, -2.5));
    assert_eq!(got.vel, Vec3::new(0.0, -0.5, 0.0));
    assert_eq!((got.yaw, got.pitch), (0.5, -0.25));
    assert_eq!(got.mode, PlayerMode::Survival);
    assert_eq!(got.health, 20);
    assert_eq!(got.bed_spawn, None);
    assert_eq!(got.inventory.active_slot(), 2);
    assert!(got.inventory.raw_slots().iter().all(Option::is_none));
    assert!(got.craft_craftable_only);
    assert_eq!(
        got.effects,
        vec![("petramond:regeneration".to_owned(), 100)]
    );
    assert_eq!(got.obtained_items, vec!["petramond:coal".to_owned()]);
    assert_eq!(got.unlocked_recipes, vec!["petramond:torch".to_owned()]);
    assert!(got.kept.is_empty());
}

/// Golden player file v8, derived from the v7 fixture by the v7 → v8 layout
/// (tagged fields 1..=15).
#[test]
fn golden_player_v8_decodes() {
    assert_golden(&decode(V8).expect("v8 decodes"));
}

/// Golden player file v7, laid out by hand: it migrates to the same data.
#[test]
fn golden_player_v7_migrates() {
    assert_golden(&decode(V7).expect("v7 migrates"));
}

/// The upgrade step is a pure byte rewrite: v7 in, the golden v8 out.
#[test]
fn the_v7_step_produces_the_golden_v8_bytes() {
    assert_eq!(&V7[..4], &7u32.to_le_bytes());
    assert_eq!(&V8[..4], &8u32.to_le_bytes());
    assert_eq!(v7::upgrade(&V7[4..]).expect("upgrades"), &V8[4..]);
}

/// Re-encoding what the golden file decodes to writes it byte for byte: a
/// layout change that forgets its version bump fails here.
#[test]
fn the_encoder_writes_the_golden_v8_layout() {
    let got = decode(V8).expect("decodes");
    assert_eq!(encode(&got.restore()), V8);
}

/// A slot whose item this world cannot resolve loads empty, and a field
/// this build does not know is kept: both are written back — the slot
/// while it is still empty.
#[test]
fn unresolvable_slots_and_unknown_fields_are_kept_and_written_back() {
    let pal = Palette::identity();
    let strange = DiskSlot {
        item: u16::MAX - 1,
        count: 4,
        blob: Default::default(),
    };
    let mut record = PlayerRecord {
        health: 20,
        slots: vec![DiskSlot::default(); TOTAL_SLOTS],
        ..PlayerRecord::default()
    };
    record.slots[5] = strange.clone();
    record.off_hand = strange.clone();
    record.unknown.insert(90, vec![1, 2]);
    let mut bytes = VERSION.to_le_bytes().to_vec();
    record.put(&mut bytes);

    let got = super::decode(&bytes, &pal).expect("decodes");
    assert_eq!(got.inventory.raw_slots()[5], None, "loads empty");
    assert_eq!(got.inventory.off_hand(), None);
    assert!(!got.kept.is_empty());

    let player = got.restore();
    let again = encode_keeping(&player, &pal, &got.kept);
    assert_eq!(again, bytes, "written back unchanged");

    let mut filled = player.clone();
    *filled.inventory.slot_mut(5).expect("slot 5") = Some(ItemStack::new(ItemType::Stone, 1));
    let back =
        super::decode(&encode_keeping(&filled, &pal, &got.kept), &pal).expect("decodes");
    assert_eq!(
        back.inventory.raw_slots()[5],
        Some(ItemStack::new(ItemType::Stone, 1)),
        "a filled slot keeps what fills it"
    );
}

#[test]
fn restore_drops_unknown_effect_names_and_keeps_known_ones() {
    // A removed/disabled mod's effect must not error the whole restore —
    // it is dropped (with a warning) while known effects still apply.
    let mut player = Player::new(WorldPos::new(0.0, 70.0, 0.0));
    player.apply_effect(petramond_world::effect::Effect::Regeneration, 400);
    let mut data = decode(&encode(&player)).expect("decodes");
    data.effects
        .push(("gone_mod:vanished_effect".to_owned(), 100));

    let restored = data.restore();
    let active = restored.effects();
    assert_eq!(active.len(), 1, "only the known effect is restored");
    assert_eq!(
        active[0].effect,
        petramond_world::effect::Effect::Regeneration
    );
    assert_eq!(active[0].remaining, 400);
}
