//! The authority's handling of the client's movement claims — soft-accepted
//! verbatim or rejected, never granting reach, noclip or a dodged fall — and
//! the client's side of the correction loop (only real divergence ships, the
//! look and hotbar stay client-owned).

use super::common::*;
use petramond::events::tick::TickEvents;
use petramond::net::protocol::{ActionDenyReason, ClientToServer, PlayerAction, SelfTransform};
use petramond_math::math::{IVec3, Vec3};
use petramond_math::world_pos::WorldPos;
use petramond_world::block::Block;

/// F1 movement-claim validation: every case sends ONE `PlayerUpdate` claim
/// from the same airborne start, ticks movement, and asserts the claim was
/// soft-accepted verbatim or rejected (the integrated result stays near the
/// start). Claims that assert other quantities — anti-noclip geometry, reach,
/// fall-damage tracking — keep their own tests.
#[test]
fn movement_claim_validation_cases() {
    enum Expect {
        /// The claim transform is adopted exactly (velocity too when flagged).
        Accepted { adopts_vel: bool },
        /// The claim is refused: the kept position stays within this distance
        /// of the start.
        RejectedWithin(f32),
    }
    struct Case {
        label: &'static str,
        /// Free-run ticks between an at-rest claim and the claim under test
        /// (a slow client's report gap widens the drift ring).
        gap_ticks: usize,
        claim_offset: Vec3,
        claim_vel: Vec3,
        /// `Some` overrides the claim's grounded flag.
        on_ground: Option<bool>,
        expect: Expect,
    }
    let cases = [
        Case {
            label: "impossible speed claim keeps the integrated pos",
            gap_ticks: 0,
            claim_offset: Vec3::new(50.0, 0.0, 0.0),
            claim_vel: Vec3::new(200.0, 0.0, 0.0),
            on_ground: None,
            expect: Expect::RejectedWithin(5.0),
        },
        Case {
            // A position jump under an innocent velocity must not teleport.
            label: "teleport claim with a plausible velocity",
            gap_ticks: 0,
            claim_offset: Vec3::new(50.0, 0.0, 0.0),
            claim_vel: Vec3::ZERO,
            on_ground: None,
            expect: Expect::RejectedWithin(5.0),
        },
        Case {
            // A sideways hop far beyond any legitimate horizontal speed,
            // under an innocent velocity — the old isotropic (terminal-speed)
            // ring accepted this; the per-axis ring must not.
            label: "horizontal teleport inside the old isotropic ring",
            gap_ticks: 0,
            claim_offset: Vec3::new(3.5, 0.0, 0.0),
            claim_vel: Vec3::new(5.0, 0.0, 0.0),
            on_ground: None,
            expect: Expect::RejectedWithin(2.0),
        },
        Case {
            // Take-off frame of a sprint jump: horizontal sprint + full jump
            // speed. The caps are per-axis, so the combined magnitude must
            // still pass.
            label: "sprint-jump take-off frame",
            gap_ticks: 0,
            claim_offset: Vec3::new(0.2, 0.0, 0.0),
            claim_vel: Vec3::new(5.6, 8.4, 0.0),
            on_ground: Some(false),
            expect: Expect::Accepted { adopts_vel: true },
        },
        Case {
            // A slow client free-runs several server ticks with no fresh
            // claim, so its next report legitimately drifted further than one
            // frame's worth. The closeness ring scales with the claim gap —
            // no rubber-banding.
            label: "claim after a slow-client gap",
            gap_ticks: 4,
            claim_offset: Vec3::new(3.5, 0.0, 0.0),
            claim_vel: Vec3::new(5.6, 0.0, 0.0),
            on_ground: None,
            expect: Expect::Accepted { adopts_vel: false },
        },
    ];

    for case in cases {
        let mut game = game_on_empty_chunk();
        let start = WorldPos::new(8.5, 70.0, 8.5);
        game.server_player_mut().pos = start;
        if case.gap_ticks > 0 {
            let mut u = player_update(&game, true);
            u.transform.pos = start;
            u.transform.vel = Vec3::ZERO;
            game.send_to_server(ClientToServer::PlayerUpdate(u));
            game.sim_mut().tick_movement(0);
            for _ in 0..case.gap_ticks {
                game.sim_mut().tick_movement(0);
            }
        }

        let mut u = player_update(&game, true);
        u.transform.pos = start + case.claim_offset;
        u.transform.vel = case.claim_vel;
        if let Some(on_ground) = case.on_ground {
            u.on_ground = on_ground;
        }
        let claim = u;
        game.send_to_server(ClientToServer::PlayerUpdate(u));
        game.sim_mut().tick_movement(0);

        let sess = game.session();
        let after = sess.player.pos;
        match case.expect {
            Expect::Accepted { adopts_vel } => {
                assert_eq!(
                    after, claim.transform.pos,
                    "[{}] a legitimate claim must be soft-accepted",
                    case.label
                );
                if adopts_vel {
                    assert_eq!(
                        sess.player.vel, claim.transform.vel,
                        "[{}] the accepted claim's velocity is adopted",
                        case.label
                    );
                }
            }
            Expect::RejectedWithin(max_drift) => {
                assert!(
                    (after - start).length() < max_drift,
                    "[{}] the claim must not be adopted (start={start:?} after={after:?})",
                    case.label
                );
            }
        }
    }
}

#[test]
fn claim_inside_solid_geometry_is_rejected() {
    let mut game = game_on_empty_chunk();
    assert!(game.server_world_mut().set_block_world(8, 64, 8, Block::Stone));
    game.server_player_mut().pos = WorldPos::new(8.5, 66.0, 8.5);
    let mut u = player_update(&game, true);
    u.transform.pos = WorldPos::new(8.5, 64.3, 8.5); // feet well inside the stone cell
    u.transform.vel = Vec3::ZERO;
    game.send_to_server(ClientToServer::PlayerUpdate(u));
    game.sim_mut().tick_movement(0);
    let after = game.server_player().pos;
    assert!(
        after.y > 65.0,
        "a claim inside solid geometry must not be adopted (after={after:?})"
    );
}

#[test]
fn transform_corrections_ship_only_on_real_divergence() {
    let mut game = game_on_empty_chunk();
    let sess = game.session_mut();
    sess.player.pos = WorldPos::new(8.5, 70.0, 8.5);
    sess.player.vel = Vec3::new(0.0, -1.4, 0.0);
    // The server free-ran a little past the client's last claim: small pos
    // phase drift, one tick of gravity — time-phase, not divergence.
    sess.last_reported_transform = Some(SelfTransform {
        transform: petramond::net::protocol::Transform {
            pos: sess.player.pos + Vec3::new(0.4, 0.5, 0.0),
            vel: Vec3::ZERO,
            yaw: sess.player.yaw,
            pitch: sess.player.pitch,
        },
        on_ground: sess.player.on_ground,
    });
    assert!(
        game.sim_mut().build_self_state(0).transform.is_none(),
        "extrapolation past the claim must not rubber-band the client"
    );

    // A genuine tick-side teleport still corrects.
    game.server_player_mut().pos += Vec3::new(50.0, 0.0, 0.0);
    assert!(
        game.sim_mut().build_self_state(0).transform.is_some(),
        "a real teleport ships a SelfTransform"
    );
}

#[test]
fn hotbar_selection_is_client_owned_and_never_yanked_by_a_batch() {
    let mut game = game();
    game.server_player_mut().inventory = filled_inventory();

    // The client scrolls ahead of the server (which still thinks slot 0)...
    game.game.set_active_hotbar(3);
    assert_eq!(game.replica.self_view.inventory.active_slot(), 3);

    // ...and a full-inventory batch from the lagging server must keep the
    // client's newer selection, not echo the stale one back.
    game.sync_self_view_for_test();
    assert_eq!(
        game.replica.self_view.inventory.active_slot(),
        3,
        "a server batch must never yank the client-owned hotbar selection"
    );
}

#[test]
fn far_claim_does_not_grant_reach() {
    let mut game = game_on_empty_chunk();
    let far = IVec3::new(14, 64, 14);
    assert!(game
        .server_world_mut()
        .set_block_world(far.x, far.y, far.z, Block::Stone));
    game.server_player_mut().pos = WorldPos::new(2.5, 65.0, 2.5);

    // A fabricated claim right next to the far block: outside the drift ring
    // of the server's own integration, so it must not become the reach eye.
    let mut u = player_update(&game, true);
    u.transform.pos = WorldPos::new(13.5, 65.0, 13.5);
    u.transform.vel = Vec3::ZERO;
    u.target = Some(hit(far, IVec3::Y));
    game.send_to_server(ClientToServer::PlayerUpdate(u));
    assert!(
        game.session().look.is_none(),
        "an implausible claim must not validate a far look target"
    );

    game.send_to_server(ClientToServer::Action(PlayerAction::BreakFinished {
            request_id: 30,
            pos: far,
            tool_item_id: None,
            predicted: true,
        }),
    );
    game.sim_mut().tick_mining(0, &mut TickEvents::default());
    assert_eq!(
        Block::from_id(game.server_world().chunk_block(far.x, far.y, far.z)),
        Block::Stone,
        "remote reach must not break the block"
    );
    assert!(
        game.session()
            .pending_action_outcomes
            .iter()
            .any(|o| o.id == 30 && !o.accepted && o.reason == Some(ActionDenyReason::OutOfReach)),
        "the far finish denies OutOfReach"
    );
}

#[test]
fn fake_on_ground_claims_do_not_evade_fall_damage() {
    let mut game = game_on_empty_chunk();
    assert!(game.server_world_mut().set_block_world(8, 64, 8, Block::Stone));
    game.server_player_mut().pos = WorldPos::new(8.5, 80.0, 8.5);
    game.session_mut().claim_pos = game.server_player().pos;
    game.session_mut().fall.reset(80.0);

    // Descend claiming on_ground every tick — the mid-air flag is fabricated
    // (no support under the feet), so the peak must survive to the landing.
    for y in [80.0, 76.0, 72.0, 68.0, 65.0] {
        let mut u = player_update(&game, true);
        u.transform.pos = WorldPos::new(8.5, y, 8.5);
        u.transform.vel = Vec3::new(0.0, -20.0, 0.0);
        u.on_ground = true;
        game.send_to_server(ClientToServer::PlayerUpdate(u));
        game.sim_mut().tick_movement(0);
    }
    assert!(
        game.session().pending_fall >= 14.0,
        "faked grounded claims must not reset the fall (measured {})",
        game.session().pending_fall
    );
}

#[test]
fn sprint_descent_down_steps_is_not_one_tall_fall() {
    use petramond_world::block_state::{StairHalf, StairState};
    let mut game = game_on_empty_chunk();
    // A staircase of real stair blocks descending +x (low half downhill), onto
    // a floor at y = 59 — half-block steps every half block, like any player
    // staircase.
    for i in 0..12 {
        assert!(game.server_world_mut().place_stair(
            IVec3::new(2 + i, 70 - i, 8),
            Block::OakStairs,
            StairState::new(petramond_math::facing::Facing::East, StairHalf::Bottom),
        ));
    }
    for x in 14..16 {
        assert!(game.server_world_mut().set_block_world(x, 58, 8, Block::Stone));
    }

    let start = WorldPos::new(2.3, 71.0, 8.5);
    game.server_player_mut().pos = start;
    game.session_mut().claim_pos = start;
    game.session_mut().fall.reset(start.y);

    // The client's own 60 fps physics sprints down the staircase. Each step
    // contact lasts only a frame or two, so the once-per-tick report can
    // legitimately be an airborne mid-hop transform for the entire descent —
    // model that worst-case (but honest) send phase by reporting the window's
    // freshest airborne frame. The whole staircase must still never measure
    // as one tall fall: the server's own integration touched every step.
    let mut client = petramond::player::Player::new(start);
    let input = petramond::player::Input {
        wishdir: Vec3::new(1.0, 0.0, 0.0),
        jump: false,
        sprint: true,
        sneak: false,
    };
    for _ in 0..400 {
        let mut report = None;
        for _ in 0..3 {
            client.update(1.0 / 60.0, game.server_world(), input);
            if !client.on_ground || report.is_none() {
                report = Some((client.pos, client.vel, client.on_ground));
            }
        }
        let (pos, vel, on_ground) = report.unwrap();
        let mut u = player_update(&game, true);
        u.transform.pos = pos;
        u.transform.vel = vel;
        u.on_ground = on_ground;
        u.wishdir = input.wishdir;
        u.sprint = true;
        game.send_to_server(ClientToServer::PlayerUpdate(u));
        game.sim_mut().tick_movement(0);
        if client.on_ground && client.pos.x > 14.2 {
            break;
        }
    }
    assert!(
        client.on_ground && client.pos.x > 14.2,
        "the client sim must finish the descent (ended at {:?})",
        client.pos
    );
    // Stand on the floor for a few ticks so the server observes the final
    // grounded transform (the landing that would convert a mis-measured
    // descent into damage).
    for _ in 0..3 {
        let mut u = player_update(&game, true);
        u.transform.pos = client.pos;
        u.transform.vel = client.vel;
        u.on_ground = true;
        game.send_to_server(ClientToServer::PlayerUpdate(u));
        game.sim_mut().tick_movement(0);
    }

    let measured = game.session().pending_fall;
    assert_eq!(
        petramond::server::health::fall_damage_health(measured),
        0,
        "sprinting down a staircase must not deal fall damage (measured a {measured}-block fall)"
    );
}

/// A `SelfTransform` correction never adopts yaw/pitch: the look is
/// client-owned input (like the hotbar index), so a correction can only carry
/// the one-RTT-old echo of the client's own look — adopting it reverts every
/// look change for as long as a correction stream flows (rejected claims over
/// still-streaming terrain), and the server never sees the player's aim.
#[test]
fn corrections_never_adopt_the_look() {
    let mut game = game();
    game.game.local.player.yaw = 1.25;
    game.game.local.player.pitch = -0.5;
    game.game.local.last_sent_transform = Some(SelfTransform {
        transform: petramond::net::protocol::Transform {
            pos: game.game.local.player.pos,
            vel: Vec3::ZERO,
            yaw: 1.25,
            pitch: -0.5,
        },
        on_ground: true,
    });

    // The in-flight correction carries the PREVIOUS look (the echo) and a
    // genuinely server-moved position.
    let corrected_pos = game.game.local.player.pos + Vec3::new(0.0, -2.0, 0.0);
    game.game.adopt_authoritative_transform(&SelfTransform {
        transform: petramond::net::protocol::Transform {
            pos: corrected_pos,
            vel: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
        },
        on_ground: false,
    });

    assert_eq!(
        game.game.local.player.pos, corrected_pos,
        "position corrections still adopt"
    );
    assert_eq!(
        game.game.local.player.yaw, 1.25,
        "the stale yaw echo must not revert the look"
    );
    assert_eq!(
        game.game.local.player.pitch, -0.5,
        "the stale pitch echo must not revert the look"
    );
}
