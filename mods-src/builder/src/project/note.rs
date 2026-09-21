//! What a project's note says.
//!
//! The record keeps the note as the words the owner reads, because the golem
//! writes them freely. The notes LOGIC has to recognise are named here, and
//! this is the one place that knows their words.

use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Note {
    None,
    Paused,
    GolemDied,
    TableGone,
    ChestsFull,
    /// Supplies fall short; the words are a rendered
    /// [`Shortfall`](crate::supplies::Shortfall).
    Missing(String),
    /// Anything else the golem had to say.
    Text(String),
}

/// How every rendered shortfall begins: a count follows.
pub const MISSING: &str = "Missing ";

impl Note {
    pub fn read(text: &str) -> Self {
        let counted = |rest: &str| rest.starts_with(|c: char| c.is_ascii_digit());
        for known in [
            Note::Paused,
            Note::GolemDied,
            Note::TableGone,
            Note::ChestsFull,
        ] {
            if known.words() == text {
                return known;
            }
        }
        match text {
            "" => Note::None,
            _ if text.strip_prefix(MISSING).is_some_and(counted) => Note::Missing(text.into()),
            _ => Note::Text(text.into()),
        }
    }

    pub fn is_shortfall(&self) -> bool {
        matches!(self, Note::Missing(_))
    }

    fn words(&self) -> &str {
        match self {
            Note::None => "",
            Note::Paused => "Paused",
            Note::GolemDied => "The golem died",
            Note::TableGone => "The schematic table is gone",
            Note::ChestsFull => "The chests are full",
            Note::Missing(text) | Note::Text(text) => text,
        }
    }
}

impl fmt::Display for Note {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.words())
    }
}

impl From<Note> for String {
    fn from(note: Note) -> Self {
        note.words().to_owned()
    }
}

impl From<&str> for Note {
    fn from(text: &str) -> Self {
        Note::read(text)
    }
}
