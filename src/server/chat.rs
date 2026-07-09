//! Server-side chat formatting, sanitization, and delivery targeting.
//!
//! Chat is server-authoritative but not simulation state: accepted lines are
//! delivered to currently connected clients and never retained server-side.

use crate::net::protocol::{ChatColor, ChatLine, ChatSpan, MAX_CHAT_CHARS};
use crate::server::player::PlayerId;

/// Who should receive one accepted chat line on the next pump.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ChatTargets {
    /// Every currently connected session (console `say`, player chat, join/leave).
    All,
    /// Only the listed player ids; unknown / already-left ids are ignored.
    Players(Vec<PlayerId>),
}

impl ChatTargets {
    #[inline]
    pub(crate) fn includes(&self, id: PlayerId) -> bool {
        match self {
            Self::All => true,
            Self::Players(ids) => ids.contains(&id),
        }
    }
}

/// One accepted line waiting to ship on the next pump.
#[derive(Clone, Debug)]
pub(crate) struct PendingChat {
    pub line: ChatLine,
    pub targets: ChatTargets,
}

pub(crate) fn player_line(seq: u64, name: &str, text: &str) -> Option<ChatLine> {
    let text = clean_text(text)?;
    Some(ChatLine {
        seq,
        spans: vec![ChatSpan {
            fg: ChatColor::White,
            text: format!("<{name}> {text}"),
        }],
    })
}

pub(crate) fn server_line(seq: u64, text: &str) -> Option<ChatLine> {
    let text = clean_text(text)?;
    let source = format!("[Server] {text}");
    Some(parse_markup(seq, &source))
}

/// Mod-/engine-authored helper text: sanitized, markup-parsed, no forced prefix.
pub(crate) fn authored_line(seq: u64, text: &str) -> Option<ChatLine> {
    let text = clean_text(text)?;
    Some(parse_markup(seq, &text))
}

pub(crate) fn joined_line(seq: u64, name: &str) -> ChatLine {
    parse_markup(seq, &format!("$[fg=yellow]{name} has joined the game"))
}

pub(crate) fn left_line(seq: u64, name: &str) -> ChatLine {
    parse_markup(seq, &format!("$[fg=yellow]{name} has left the game"))
}

fn clean_text(text: &str) -> Option<String> {
    let mut out = String::new();
    for ch in text.chars() {
        let ch = if ch.is_control() { ' ' } else { ch };
        out.push(ch);
        if out.chars().count() >= MAX_CHAT_CHARS {
            break;
        }
    }
    let out = out.trim();
    (!out.is_empty()).then(|| out.to_owned())
}

fn parse_markup(seq: u64, text: &str) -> ChatLine {
    let mut spans = Vec::new();
    let mut color = ChatColor::White;
    let mut rest = text;
    while let Some(at) = rest.find("$[fg=") {
        push_span(&mut spans, color, &rest[..at]);
        rest = &rest[at + "$[fg=".len()..];
        let Some(end) = rest.find(']') else {
            push_span(&mut spans, color, "$[fg=");
            break;
        };
        if let Some(next) = color_from_name(&rest[..end]) {
            color = next;
            rest = &rest[end + 1..];
        } else {
            push_span(&mut spans, color, "$[fg=");
            push_span(&mut spans, color, &rest[..=end]);
            rest = &rest[end + 1..];
        }
    }
    push_span(&mut spans, color, rest);
    ChatLine { seq, spans }
}

fn push_span(spans: &mut Vec<ChatSpan>, fg: ChatColor, text: &str) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = spans.last_mut().filter(|s| s.fg == fg) {
        last.text.push_str(text);
    } else {
        spans.push(ChatSpan {
            fg,
            text: text.to_owned(),
        });
    }
}

fn color_from_name(name: &str) -> Option<ChatColor> {
    match name {
        "white" => Some(ChatColor::White),
        "red" => Some(ChatColor::Red),
        "yellow" => Some(ChatColor::Yellow),
        "blue" => Some(ChatColor::Blue),
        "cyan" => Some(ChatColor::Cyan),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::player::PlayerId;

    #[test]
    fn player_chat_is_sanitized_and_formatted() {
        let line = player_line(7, "Rachel", "  hello\nthere  ").unwrap();
        assert_eq!(line.seq, 7);
        assert_eq!(
            line.spans,
            vec![ChatSpan {
                fg: ChatColor::White,
                text: "<Rachel> hello there".to_string(),
            }]
        );
    }

    #[test]
    fn system_join_uses_yellow_markup() {
        let line = joined_line(3, "Alex");
        assert_eq!(line.spans.len(), 1);
        assert_eq!(line.spans[0].fg, ChatColor::Yellow);
        assert_eq!(line.spans[0].text, "Alex has joined the game");
    }

    #[test]
    fn authored_line_parses_markup_without_server_prefix() {
        let line = authored_line(1, "$[fg=cyan]Hello $[fg=white]there").unwrap();
        assert_eq!(
            line.spans,
            vec![
                ChatSpan {
                    fg: ChatColor::Cyan,
                    text: "Hello ".to_string(),
                },
                ChatSpan {
                    fg: ChatColor::White,
                    text: "there".to_string(),
                },
            ]
        );
    }

    #[test]
    fn chat_targets_all_includes_everyone() {
        assert!(ChatTargets::All.includes(PlayerId(0)));
        assert!(ChatTargets::All.includes(PlayerId(7)));
    }

    #[test]
    fn chat_targets_players_filters_by_id() {
        let targets = ChatTargets::Players(vec![PlayerId(2), PlayerId(5)]);
        assert!(!targets.includes(PlayerId(0)));
        assert!(targets.includes(PlayerId(2)));
        assert!(targets.includes(PlayerId(5)));
    }
}
