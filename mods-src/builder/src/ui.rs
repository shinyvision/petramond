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
