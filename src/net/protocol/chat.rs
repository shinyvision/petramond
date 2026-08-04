use serde::{Deserialize, Serialize};

pub const MAX_CHAT_CHARS: usize = 256;

/// The small fixed chat palette understood by clients. Messages carry
/// structured spans, not inline control text, so clients never parse player
/// content as formatting.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChatColor {
    White,
    Red,
    Yellow,
    Blue,
    Cyan,
}

/// One styled text run in a chat line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatSpan {
    pub fg: ChatColor,
    pub text: String,
}

/// One server-accepted chat line. Sequence numbers are session-local and only
/// provide a stable ordering key for clients/tests; chat history is not
/// retained server-side or replayed to later joiners.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatLine {
    pub seq: u64,
    pub spans: Vec<ChatSpan>,
}
