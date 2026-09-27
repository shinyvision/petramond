use crate::net::protocol::{ChatColor, ChatLine, ChatSpan, MAX_CHAT_CHARS};
use crate::player::PlayerId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatTargets {
    All,
    Players(Vec<PlayerId>),
}

impl ChatTargets {
    #[inline]
    pub fn includes(&self, id: PlayerId) -> bool {
        match self {
            Self::All => true,
            Self::Players(ids) => ids.contains(&id),
        }
    }
}

#[derive(Clone, Debug)]
pub struct PendingChat {
    pub line: ChatLine,
    pub targets: ChatTargets,
}

#[derive(Default)]
pub struct ChatService {
    pending: Vec<PendingChat>,
    next_seq: u64,
}

impl ChatService {
    pub fn enqueue(&mut self, line: ChatLine, targets: ChatTargets) {
        self.pending.push(PendingChat { line, targets });
    }

    pub fn player(&mut self, name: &str, text: &str, echo: bool) {
        let seq = self.alloc_seq();
        if let Some(line) = player_line(seq, name, text) {
            if echo {
                log::info!("chat: {}", display_text(&line));
            }
            self.enqueue(line, ChatTargets::All);
        }
    }

    pub fn server(&mut self, text: &str) {
        let seq = self.alloc_seq();
        self.enqueue_line(server_line(seq, text), ChatTargets::All);
    }

    pub fn authored(&mut self, text: &str, targets: ChatTargets) {
        let seq = self.alloc_seq();
        self.enqueue_line(authored_line(seq, text), targets);
    }

    pub fn plain(&mut self, text: &str, color: ChatColor, targets: ChatTargets) {
        let seq = self.alloc_seq();
        self.enqueue_line(plain_line(seq, text, color), targets);
    }

    pub fn joined(&mut self, name: &str) {
        let seq = self.alloc_seq();
        self.enqueue(joined_line(seq, name), ChatTargets::All);
    }

    pub fn left(&mut self, name: &str) {
        let seq = self.alloc_seq();
        self.enqueue(left_line(seq, name), ChatTargets::All);
    }

    pub fn take_pending(&mut self) -> Vec<PendingChat> {
        std::mem::take(&mut self.pending)
    }

    #[cfg(test)]
    pub fn pending(&self) -> &[PendingChat] {
        &self.pending
    }

    fn enqueue_line(&mut self, line: Option<ChatLine>, targets: ChatTargets) {
        if let Some(line) = line {
            self.enqueue(line, targets);
        }
    }

    fn alloc_seq(&mut self) -> u64 {
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        seq
    }
}

pub fn player_line(seq: u64, name: &str, text: &str) -> Option<ChatLine> {
    let text = clean_text(text)?;
    Some(ChatLine {
        seq,
        spans: vec![ChatSpan {
            fg: ChatColor::White,
            text: format!("<{name}> {text}"),
        }],
    })
}

pub fn server_line(seq: u64, text: &str) -> Option<ChatLine> {
    let text = clean_text(text)?;
    let source = format!("[Server] {text}");
    Some(parse_markup(seq, &source))
}

pub fn authored_line(seq: u64, text: &str) -> Option<ChatLine> {
    let text = clean_text(text)?;
    Some(parse_markup(seq, &text))
}

pub fn plain_line(seq: u64, text: &str, fg: ChatColor) -> Option<ChatLine> {
    let text = clean_text(text)?;
    Some(ChatLine {
        seq,
        spans: vec![ChatSpan { fg, text }],
    })
}

pub fn display_text(line: &ChatLine) -> String {
    line.spans.iter().map(|span| span.text.as_str()).collect()
}

pub fn joined_line(seq: u64, name: &str) -> ChatLine {
    presence_line(seq, name, "has joined the game")
}

pub fn left_line(seq: u64, name: &str) -> ChatLine {
    presence_line(seq, name, "has left the game")
}

fn presence_line(seq: u64, name: &str, what: &str) -> ChatLine {
    let name: String = name.chars().filter(|c| !c.is_control()).collect();
    ChatLine {
        seq,
        spans: vec![ChatSpan {
            fg: ChatColor::Yellow,
            text: format!("{name} {what}"),
        }],
    }
}

pub fn clean_text(text: &str) -> Option<String> {
    let out: String = text
        .chars()
        .take(MAX_CHAT_CHARS)
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect();
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
    use crate::player::PlayerId;

    #[test]
    fn the_outbox_orders_lines_by_one_seq_counter() {
        let mut chat = ChatService::default();
        chat.server("hello");
        chat.player("Rachel", "hi", false);
        chat.left("Alex");
        let pending = chat.take_pending();
        assert_eq!(
            pending.iter().map(|p| p.line.seq).collect::<Vec<_>>(),
            [0, 1, 2]
        );
        assert!(pending.iter().all(|p| p.targets == ChatTargets::All));
        assert!(chat.take_pending().is_empty(), "a pump takes everything");
    }

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
    fn system_join_is_one_yellow_span() {
        let line = joined_line(3, "Alex");
        assert_eq!(line.spans.len(), 1);
        assert_eq!(line.spans[0].fg, ChatColor::Yellow);
        assert_eq!(line.spans[0].text, "Alex has joined the game");
    }

    #[test]
    fn presence_lines_never_parse_the_name_as_markup() {
        let line = left_line(4, "$[fg=red]Mallory");
        assert_eq!(
            line.spans,
            vec![ChatSpan {
                fg: ChatColor::Yellow,
                text: "$[fg=red]Mallory has left the game".to_string(),
            }]
        );
    }

    #[test]
    fn clean_text_caps_by_characters() {
        let long = "é".repeat(MAX_CHAT_CHARS * 50);
        let out = clean_text(&long).expect("non-empty");
        assert_eq!(out.chars().count(), MAX_CHAT_CHARS);
        assert_eq!(
            clean_text(" \u{7}\n "),
            None,
            "controls alone trim to nothing"
        );
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
