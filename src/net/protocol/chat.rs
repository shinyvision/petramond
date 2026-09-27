use serde::{Deserialize, Serialize};

pub const MAX_CHAT_CHARS: usize = 256;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChatColor {
    White,
    Red,
    Yellow,
    Blue,
    Cyan,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatSpan {
    pub fg: ChatColor,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatLine {
    pub seq: u64,
    pub spans: Vec<ChatSpan>,
}
