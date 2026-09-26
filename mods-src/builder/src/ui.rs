//! The UI vocabulary the builder's panels share, so the golem's panel and
//! the table's speak alike without reaching into each other.

/// The theme palette a status line is drawn in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tone {
    #[default]
    Muted,
    Plain,
    Accent,
    Warn,
    Danger,
}

impl Tone {
    pub fn palette(self) -> &'static str {
        match self {
            Tone::Muted => "text_muted",
            Tone::Plain => "text",
            Tone::Accent => "accent",
            Tone::Warn => "warn",
            Tone::Danger => "danger",
        }
    }
}
