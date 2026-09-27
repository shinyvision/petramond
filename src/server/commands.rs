use crate::net::identity::PlayerKey;
use crate::net::protocol::ChatColor;
use crate::player::PlayerId;
use crate::server::chat::ChatTargets;
use crate::server::game::ServerGame;

#[derive(Copy, Clone)]
enum CommandSource {
    Console,
    Player(PlayerId),
}

impl ServerGame {
    pub fn execute_console_command(&mut self, line: &str) {
        self.execute_command(CommandSource::Console, line.trim());
    }

    pub fn execute_player_command(&mut self, player: PlayerId, command: &str) {
        if !self.is_operator_id(player) {
            self.command_reply(
                CommandSource::Player(player),
                ChatColor::Red,
                "You do not have permission to use server commands.",
            );
            return;
        }
        self.execute_command(CommandSource::Player(player), command);
    }

    pub fn is_operator(&self, s: usize) -> bool {
        (self.sessions.has_local_session() && s == 0)
            || self.operators.contains(&self.sessions[s].key)
    }

    fn is_operator_id(&self, id: PlayerId) -> bool {
        self.sessions
            .index_of(id)
            .is_some_and(|s| self.is_operator(s))
    }

    fn execute_command(&mut self, source: CommandSource, line: &str) {
        let (name, args) = split_command(line);
        if matches!(source, CommandSource::Player(_)) && matches!(name, "stop" | "save") {
            self.command_reply(
                source,
                ChatColor::Red,
                "That command is only available from the server console.",
            );
            return;
        }

        match name {
            "say" => {
                if args.is_empty() {
                    self.command_reply(source, ChatColor::Red, "Usage: say <message>");
                } else {
                    self.chat.server(args);
                    log::info!("server command: say {args}");
                }
            }
            "op" => self.set_operator(source, args, true),
            "deop" => self.set_operator(source, args, false),
            "time" => self.execute_time(source, args),
            "" => {}
            _ => self.command_reply(
                source,
                ChatColor::Red,
                "Unknown command (commands: say, op, deop, time).",
            ),
        }
    }

    fn resolve_player(&self, requested: &str) -> Option<(PlayerKey, String)> {
        if let Some(session) = self
            .sessions
            .iter()
            .find(|session| session.name.eq_ignore_ascii_case(requested))
        {
            return Some((session.key, session.name.clone()));
        }
        if let Some(key) = self.accounts.key_for_name(requested) {
            let name = self.accounts.name_of(&key).unwrap_or(requested).to_owned();
            return Some((key, name));
        }
        let key = requested.parse::<PlayerKey>().ok()?;
        let name = self
            .accounts
            .name_of(&key)
            .map_or_else(|| key.to_string(), str::to_owned);
        Some((key, name))
    }

    fn is_local_player(&self, key: &PlayerKey) -> bool {
        self.sessions.has_local_session() && self.sessions.first().is_some_and(|s| s.key == *key)
    }

    fn set_operator(&mut self, source: CommandSource, requested: &str, enabled: bool) {
        if requested.is_empty() {
            let usage = if enabled {
                "Usage: op <playername>"
            } else {
                "Usage: deop <playername>"
            };
            self.command_reply(source, ChatColor::Red, usage);
            return;
        }
        let Some((key, display)) = self.resolve_player(requested) else {
            self.command_reply(
                source,
                ChatColor::Red,
                &format!("Unknown player '{requested}': they must join this world once first."),
            );
            return;
        };

        if enabled {
            if self.is_local_player(&key) || !self.operators.insert(key) {
                self.command_reply(
                    source,
                    ChatColor::Yellow,
                    &format!("{display} is already an operator."),
                );
                return;
            }
            crate::server::permissions::store(&mut self.world, &self.operators);
            self.command_reply(
                source,
                ChatColor::Yellow,
                &format!("Made {display} an operator."),
            );
            log::info!("server command: op {display} ({key})");
        } else {
            if self.is_local_player(&key) {
                self.command_reply(
                    source,
                    ChatColor::Red,
                    "The local player is always an operator.",
                );
                return;
            }
            if !self.operators.remove(&key) {
                self.command_reply(
                    source,
                    ChatColor::Yellow,
                    &format!("{display} is not an operator."),
                );
                return;
            }
            if let Some(session) = self.sessions.iter_mut().find(|session| session.key == key) {
                session.player.set_mode(crate::player::PlayerMode::Survival);
                session.sim.fall.reset(session.player.pos.y);
                session.sim.pending_fall = 0.0;
            }
            crate::server::permissions::store(&mut self.world, &self.operators);
            self.command_reply(
                source,
                ChatColor::Yellow,
                &format!("Revoked operator permissions from {display}."),
            );
            log::info!("server command: deop {display} ({key})");
        }
    }

    fn execute_time(&mut self, source: CommandSource, args: &str) {
        let mut words = args.split_whitespace();
        let first = words.next();
        let second = words.next();
        let extra = words.next();
        match (first, second, extra) {
            (Some("set"), Some("day"), None) => {
                crate::server::daynight::set_time(
                    &mut self.world,
                    crate::server::daynight::TimePreset::Day,
                );
            }
            (Some("set"), Some("noon"), None) => {
                crate::server::daynight::set_time(
                    &mut self.world,
                    crate::server::daynight::TimePreset::Noon,
                );
            }
            (Some("set"), Some("night"), None) => {
                crate::server::daynight::set_time(
                    &mut self.world,
                    crate::server::daynight::TimePreset::Night,
                );
            }
            (Some("set"), Some("midnight"), None) => {
                crate::server::daynight::set_time(
                    &mut self.world,
                    crate::server::daynight::TimePreset::Midnight,
                );
            }
            (Some("freeze"), None, None) => {
                crate::server::daynight::set_frozen(&mut self.world, true);
            }
            (Some("unfreeze"), None, None) => {
                crate::server::daynight::set_frozen(&mut self.world, false);
            }
            _ => {
                self.command_reply(
                    source,
                    ChatColor::Red,
                    "Usage: time set <day|noon|night|midnight> | time <freeze|unfreeze>",
                );
                return;
            }
        }
        log::info!("server command: time {args}");
    }

    fn command_reply(&mut self, source: CommandSource, color: ChatColor, text: &str) {
        match source {
            CommandSource::Console => match color {
                ChatColor::Red => log::warn!("{text}"),
                _ => log::info!("{text}"),
            },
            CommandSource::Player(id) => {
                self.chat.plain(text, color, ChatTargets::Players(vec![id]));
            }
        }
    }
}

fn split_command(line: &str) -> (&str, &str) {
    let line = line.trim();
    line.split_once(char::is_whitespace)
        .map_or((line, ""), |(name, args)| (name, args.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::protocol::{ClientToServer, PlayerAction};
    use crate::player::PlayerMode;

    fn server_with_guest() -> (ServerGame, usize) {
        let mut server = crate::server::session_build::build_server_inline("", 1, 2);
        let guest = crate::server::session_build::spawn_player(server.world.data().seed);
        let s = server.add_session_for_test(guest);
        (server, s)
    }

    #[test]
    fn spectator_toggle_requires_operator_but_local_player_is_always_allowed() {
        let (mut server, guest) = server_with_guest();

        server.apply_message(guest, ClientToServer::Action(PlayerAction::ToggleMode));
        assert_eq!(server.sessions[guest].player.mode(), PlayerMode::Survival);

        server.execute_console_command(&format!("op {}", server.sessions[guest].name));
        assert!(server.is_operator(guest));
        server.apply_message(guest, ClientToServer::Action(PlayerAction::ToggleMode));
        assert_eq!(server.sessions[guest].player.mode(), PlayerMode::Spectator);

        server.execute_console_command(&format!("deop {}", server.sessions[guest].name));
        assert!(!server.is_operator(guest));
        assert_eq!(
            server.sessions[guest].player.mode(),
            PlayerMode::Survival,
            "deop cannot strand a player in spectator"
        );

        server.apply_message(0, ClientToServer::Action(PlayerAction::ToggleMode));
        assert_eq!(server.sessions[0].player.mode(), PlayerMode::Spectator);
        let local_name = server.sessions[0].name.clone();
        server.execute_console_command(&format!("deop {local_name}"));
        assert!(server.is_operator(0), "the listen player is intrinsic op");
    }

    #[test]
    fn player_commands_require_byte_zero_slash_and_obey_console_only_commands() {
        let (mut server, guest) = server_with_guest();
        let id = server.sessions[guest].id;
        let name = server.sessions[guest].name.clone();
        server.execute_console_command(&format!("op {name}"));
        server.chat.take_pending();

        server.apply_message(
            guest,
            ClientToServer::ChatSend {
                text: "/time set midnight".into(),
            },
        );
        assert_eq!(
            super::super::daynight::current_clock(&server.world),
            server.world.day_cycle_ticks() * 3 / 4
        );

        server.apply_message(
            guest,
            ClientToServer::ChatSend {
                text: " /time set day".into(),
            },
        );
        assert_eq!(
            super::super::daynight::current_clock(&server.world),
            server.world.day_cycle_ticks() * 3 / 4,
            "leading whitespace makes the slash ordinary chat"
        );
        assert!(server.chat.pending().iter().any(|pending| {
            pending.targets.includes(id)
                && crate::server::chat::display_text(&pending.line)
                    == format!("<{name}> /time set day")
        }));

        server.chat.take_pending();
        server.apply_message(
            guest,
            ClientToServer::ChatSend {
                text: "/save".into(),
            },
        );
        assert!(server.chat.pending().iter().any(|pending| {
            pending.targets == ChatTargets::Players(vec![id])
                && crate::server::chat::display_text(&pending.line)
                    .contains("only available from the server console")
        }));
    }

    #[test]
    fn operator_rights_key_on_identity_not_name() {
        let (mut server, guest) = server_with_guest();
        let guest_key = server.sessions[guest].key;
        let name = server.sessions[guest].name.clone();
        server.execute_console_command(&format!("op {name}"));
        assert!(server.operators.contains(&guest_key));

        server.sessions[guest].name = "Renamed".into();
        let impostor = crate::server::session_build::spawn_player(server.world.data().seed);
        let s = server.add_session_for_test(impostor);
        server.sessions[s].name = name.clone();
        assert!(server.is_operator(guest), "rights stay with the identity");
        assert!(!server.is_operator(s), "the old name grants nothing");

        server.execute_console_command("op NeverJoined");
        let mut only_guest = crate::server::permissions::Operators::default();
        only_guest.insert(guest_key);
        assert_eq!(
            server.operators, only_guest,
            "a name no identity ever used cannot be pre-granted"
        );

        let hex = PlayerKey([0xEE; 32]);
        server.execute_console_command(&format!("op {hex}"));
        assert!(server.operators.contains(&hex), "a literal key pre-grants");
    }

    #[test]
    fn operator_names_roundtrip_through_world_data() {
        let (mut server, guest) = server_with_guest();
        let name = server.sessions[guest].name.clone();
        server.execute_console_command(&format!("op {name}"));
        assert_eq!(
            crate::server::permissions::load(&server.world),
            server.operators
        );
    }
}
