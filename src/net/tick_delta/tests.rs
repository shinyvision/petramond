use petramond_math::world_pos::WorldPos;

use super::*;
use crate::net::protocol::{ItemLane, MobLane, ServerToClient};

fn mob(id: u64, step: u32) -> MobStateRow {
    let s = step as f32;
    MobStateRow {
        id,
        kind_id: (id % 3) as u8,
        pos: WorldPos::new(100.25 + id as f64, 64.0, -30.5 + f64::from(step / 3) * 0.13),
        yaw: s * 0.2,
        tilt: petramond_math::math::Tilt::LEVEL,
        anim_time: if step.is_multiple_of(4) {
            0.0
        } else {
            s * 0.05
        },
        moving: step % 5 < 2,
        idle_anim: step.is_multiple_of(7).then_some(1),
        head_yaw: 0.1,
        head_pitch: 0.0,
        hurt_timer: 0.0,
        dead: false,
        shorn: id.is_multiple_of(2),
        emitters: Vec::new(),
        conditions: Vec::new(),
        anims: if step % 6 < 4 {
            vec![("breathe".to_owned(), s * 0.05)]
        } else {
            vec![("breathe".to_owned(), s * 0.05), ("graze".to_owned(), 0.2)]
        },
        ragdoll: None,
        dig: None,
        held: [None; 2],
        draw: Default::default(),
    }
}

fn item(id: u64, step: u32) -> ItemStateRow {
    ItemStateRow {
        id,
        item_id: 7,
        count: 1 + (step / 10) as u8,
        data: None,
        pos: WorldPos::new(id as f64, 70.0 - f64::from(step.min(5)), 3.0),
        spin: step as f32 * 0.3,
        flight: None,
    }
}

fn window(step: u32) -> TickUpdate {
    let alive = |id: u64| !((3..6).contains(&step) && id == 4);
    let (mut spawned, mut updated) = (Vec::new(), Vec::new());
    for id in (1..8u64).filter(|&id| alive(id)) {
        if step == 0 || (step == 6 && id == 4) {
            spawned.push(mob(id, step));
        } else {
            updated.push(mob(id, step));
        }
    }
    let mobs = MobLane {
        despawned: if step == 3 { vec![4] } else { Vec::new() },
        spawned: spawned.into(),
        updated: updated.into(),
    };
    let items: ItemLane = (0..5).map(|id| item(id, step)).collect::<Vec<_>>().into();
    TickUpdate::new(u64::from(step), 0).with(mobs).with(items)
}

/// The receiving connection must hand the game exactly the rows the server built, across
/// spawns, despawns, re-spawns, a row the baseline never saw (it ships whole) and rows equal
/// to their baseline.
#[test]
fn packed_windows_unpack_to_the_rows_that_were_sent() {
    let mut writer = TickDelta::default();
    let mut reader = TickDelta::default();
    for step in 0..24 {
        let sent = window(step);
        let mut wire = ServerToClient::Tick(Box::new(sent.clone()));
        if let ServerToClient::Tick(update) = &mut wire {
            writer.pack(update);
        }
        let frame = crate::net::framing::encode_frame(&wire).expect("encodes");
        let (mut got, _): (ServerToClient, usize) =
            crate::net::framing::decode_frame(&frame, crate::net::framing::MAX_FRAME)
                .expect("decodes")
                .expect("a whole frame");
        if let ServerToClient::Tick(update) = &mut got {
            reader.unpack(update).expect("unpacks");
        }
        assert_eq!(got, ServerToClient::Tick(Box::new(sent)), "window {step}");
    }
}

#[test]
fn a_change_for_an_entity_without_a_baseline_is_refused() {
    let mut writer = TickDelta::default();
    let mut update = window(1);
    writer.pack(&mut update);
    let mut fresh = TickDelta::default();
    let mut second = window(2);
    writer.pack(&mut second);
    assert!(fresh.unpack(&mut second).is_err());
}
