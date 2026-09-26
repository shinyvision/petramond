//! A protocol-level fake server: the client joins from a hand-built
//! `JoinData` over the loopback pipe with NO `ServerGame` behind it. Tests
//! script the `ServerToClient` stream and assert on the `ClientToServer`
//! messages the client sends and on its public read models — client
//! behaviour in isolation, exactly as a remote join sees it.

use petramond::net::handle::{LoopbackServer, ServerHandle};
use petramond::net::protocol::{
    ChatColor, ChatLine, ChatSpan, ClientToServer, ItemSlotWire, JoinData, PlayerAction,
    SelfRestore, ServerToClient,
};
use petramond::player::{PlayerId, PlayerMode};
use petramond_math::math::Vec3;
use petramond_math::world_pos::WorldPos;
use petramond_world::item::ItemType;

use crate::game::{Game, GameEvents, GameInput};

/// The joined client and the server end of its pipe.
pub(super) struct ScriptedServer {
    pub(super) game: Game,
    pipe: LoopbackServer,
}

/// The join payload the fake server hands out: a survival player at a fixed
/// spot holding twelve dirt in hotbar slot 2, with one other player online.
pub(super) fn join_data(spawn: WorldPos) -> Box<JoinData> {
    let mut slots: Vec<Option<ItemSlotWire>> = vec![None; 37];
    slots[2] = Some(ItemSlotWire {
        item_id: ItemType::Dirt.0,
        count: 12,
        data: None,
    });
    Box::new(JoinData {
        player_id: PlayerId(3),
        seed: 42,
        clock: 0,
        tables: petramond::net::remap::local_name_tables(),
        self_restore: SelfRestore {
            transform: petramond::net::protocol::Transform {
                pos: spawn,
                vel: Vec3::ZERO,
                yaw: 0.0,
                pitch: 0.0,
            },
            mode: 0, // survival
            health: 20,
            bed_spawn: None,
            effects: Vec::new(),
            inventory: slots,
            active_slot: 2,
            craft_craftable_only: false,
            unlocked_recipes: Vec::new(),
        },
        crafting_recipes: Vec::new(),
        players: vec![(PlayerId(0), "Host".to_string())],
    })
}

impl ScriptedServer {
    /// Join the fake server as a remote client.
    pub(super) fn join() -> Self {
        let (handle, pipe) = ServerHandle::loopback();
        let cam = petramond_render::camera::Camera::new(WorldPos::new(0.0, 80.0, 0.0), 16.0 / 9.0);
        let game = Game::new_remote(
            cam,
            join_data(WorldPos::new(4.5, 90.0, -7.5)),
            handle,
            1,
            "scripted-server",
            &std::collections::BTreeSet::new(),
            None,
        );
        Self { game, pipe }
    }

    /// Queue a server→client message for the client's next drain.
    pub(super) fn push(&mut self, msg: ServerToClient) {
        self.pipe.outbox.send(msg).expect("the client end is alive");
    }

    /// One client frame: its send half, then its receive half draining
    /// whatever was scripted.
    pub(super) fn frame(&mut self, dt: f32, input: &GameInput) -> GameEvents {
        self.game.tick(dt, input)
    }

    /// Every message the client has sent since the last call, in order.
    pub(super) fn sent(&mut self) -> Vec<ClientToServer> {
        self.pipe.inbox.try_iter().collect()
    }
}

fn player_updates(msgs: &[ClientToServer]) -> usize {
    msgs.iter()
        .filter(|m| matches!(m, ClientToServer::PlayerUpdate(_)))
        .count()
}

#[test]
fn every_frame_sends_exactly_one_player_update_with_the_client_owned_slot() {
    let mut server = ScriptedServer::join();
    server.frame(1.0 / 60.0, &GameInput::default());
    let sent = server.sent();
    assert_eq!(player_updates(&sent), 1);
    let update = sent
        .iter()
        .find_map(|m| match m {
            ClientToServer::PlayerUpdate(u) => Some(u),
            _ => None,
        })
        .expect("the frame sent its PlayerUpdate");
    assert_eq!(update.hotbar_slot, 2, "the restored active slot");

    server.game.set_active_hotbar(5);
    server.frame(1.0 / 60.0, &GameInput::default());
    let sent = server.sent();
    assert_eq!(player_updates(&sent), 1);
    assert!(
        sent.iter()
            .any(|m| matches!(m, ClientToServer::PlayerUpdate(u) if u.hotbar_slot == 5)),
        "the selection is client-owned and rides the next update"
    );
}

#[test]
fn a_remote_mode_toggle_waits_for_the_server() {
    let mut server = ScriptedServer::join();
    server.game.toggle_player_mode();
    assert_eq!(
        server.game.player_mode(),
        PlayerMode::Survival,
        "a remote client never predicts its own privilege change"
    );
    server.frame(1.0 / 60.0, &GameInput::default());
    assert!(server
        .sent()
        .iter()
        .any(|m| matches!(m, ClientToServer::Action(PlayerAction::ToggleMode))));
}

#[test]
fn scripted_roster_and_chat_reach_the_client_read_models() {
    let mut server = ScriptedServer::join();
    let line = ChatLine {
        seq: 1,
        spans: vec![ChatSpan {
            fg: ChatColor::White,
            text: "hello".into(),
        }],
    };
    server.push(ServerToClient::PlayerJoined {
        id: PlayerId(4),
        name: "Guest".to_string(),
    });
    server.push(ServerToClient::PlayerLeft { id: PlayerId(0) });
    server.push(ServerToClient::ChatLine(line.clone()));
    server.frame(1.0 / 60.0, &GameInput::default());

    let roster = server.game.player_roster();
    assert_eq!(roster.get(&PlayerId(4)).map(String::as_str), Some("Guest"));
    assert!(!roster.contains_key(&PlayerId(0)));
    assert_eq!(server.game.take_chat_lines(), vec![line]);
    assert!(
        server.game.take_chat_lines().is_empty(),
        "chat is taken once"
    );
}
