//! Shared server-command parsing and execution for the dedicated console and
//! operator-authored chat commands.

use crate::net::protocol::ChatColor;
use crate::server::chat::ChatTargets;
use crate::server::game::ServerGame;
use crate::server::player::PlayerId;

#[derive(Copy, Clone)]
enum CommandSource {
    Console,
    Player(PlayerId),
}

impl ServerGame {
    /// Execute one unprefixed dedicated-server console line. `stop` and `save`
    /// stay in the host loop because they control the server thread itself;
    /// every gameplay-visible command lands here.
    pub(crate) fn execute_console_command(&mut self, line: &str) {
        self.execute_command(CommandSource::Console, line.trim());
    }

    /// Execute a player chat command after the chat ingress has proved `/` was
    /// the message's very first character. The slash itself is not part of the
    /// shared console grammar.
    pub(crate) fn execute_player_command(&mut self, player: PlayerId, command: &str) {
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

    pub(crate) fn is_operator(&self, s: usize) -> bool {
        (self.has_local_session && s == 0)
            || self
                .operators
                .contains(&crate::server::permissions::canonical_name(
                    &self.sessions[s].name,
                ))
    }

    fn is_operator_id(&self, id: PlayerId) -> bool {
        self.sessions
            .iter()
            .position(|session| session.id == id)
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
                    self.enqueue_server_chat(args);
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
        let display = self
            .sessions
            .iter()
            .find(|session| session.name.eq_ignore_ascii_case(requested))
            .map(|session| session.name.clone())
            .unwrap_or_else(|| requested.to_owned());
        let canonical = crate::server::permissions::canonical_name(&display);
        if canonical.is_empty() {
            self.command_reply(source, ChatColor::Red, "Player name cannot be empty.");
            return;
        }

        if enabled {
            if self.sessions.first().is_some_and(|session| {
                self.has_local_session && session.name.eq_ignore_ascii_case(&display)
            }) || !self.operators.insert(canonical)
            {
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
            log::info!("server command: op {display}");
        } else {
            if self.sessions.first().is_some_and(|session| {
                self.has_local_session && session.name.eq_ignore_ascii_case(&display)
            }) {
                self.command_reply(
                    source,
                    ChatColor::Red,
                    "The local player is always an operator.",
                );
                return;
            }
            if !self.operators.remove(&canonical) {
                self.command_reply(
                    source,
                    ChatColor::Yellow,
                    &format!("{display} is not an operator."),
                );
                return;
            }
            // Revoking a spectator without returning them to survival would
            // strand them in a mode they no longer have permission to toggle.
            if let Some(session) = self
                .sessions
                .iter_mut()
                .find(|session| session.name.eq_ignore_ascii_case(&display))
            {
                session.player.set_mode(crate::player::PlayerMode::Survival);
                session.fall.reset(session.player.pos.y);
                session.pending_fall = 0.0;
            }
            crate::server::permissions::store(&mut self.world, &self.operators);
            self.command_reply(
                source,
                ChatColor::Yellow,
                &format!("Revoked operator permissions from {display}."),
            );
            log::info!("server command: deop {display}");
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
                self.enqueue_plain_chat(text, color, ChatTargets::Players(vec![id]));
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
        let (mut server, _) = crate::game::session::build_session("", 1, 2);
        let guest = crate::game::session::spawn_player(server.world.seed);
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
        server.pending_chat.clear();

        server.apply_message(
            guest,
            ClientToServer::ChatSend {
                text: "/time set midnight".into(),
            },
        );
        assert_eq!(
            super::super::daynight::current_clock(&server.world),
            super::super::daynight::CYCLE_TICKS * 3 / 4
        );

        server.apply_message(
            guest,
            ClientToServer::ChatSend {
                text: " /time set day".into(),
            },
        );
        assert_eq!(
            super::super::daynight::current_clock(&server.world),
            super::super::daynight::CYCLE_TICKS * 3 / 4,
            "leading whitespace makes the slash ordinary chat"
        );
        assert!(server.pending_chat.iter().any(|pending| {
            pending.targets.includes(id)
                && crate::server::chat::display_text(&pending.line)
                    == format!("<{name}> /time set day")
        }));

        server.pending_chat.clear();
        server.apply_message(
            guest,
            ClientToServer::ChatSend {
                text: "/save".into(),
            },
        );
        assert!(server.pending_chat.iter().any(|pending| {
            pending.targets == ChatTargets::Players(vec![id])
                && crate::server::chat::display_text(&pending.line)
                    .contains("only available from the server console")
        }));
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
