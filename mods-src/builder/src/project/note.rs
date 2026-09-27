use std::fmt;

use serde::{Deserialize, Serialize};

use crate::supplies::{Shortfall, MISSING};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Note {
    #[default]
    None,
    Paused,
    GolemDied,
    TableGone,
    ChestsFull,
    HandsFull,
    MissingBlueprint,
    Missing(Shortfall),
    Report(Report),
    Legacy(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub lost: u32,
    pub standing: u32,
    pub chests_full: bool,
}

impl Note {
    pub fn report(lost: u32, standing: u32) -> Note {
        let report = Report {
            lost,
            standing,
            chests_full: false,
        };
        if report == Report::default() {
            Note::None
        } else {
            Note::Report(report)
        }
    }

    pub fn with_chests_full(self) -> Note {
        match self {
            Note::None => Note::Report(Report {
                chests_full: true,
                ..Report::default()
            }),
            Note::Report(report) => Note::Report(Report {
                chests_full: true,
                ..report
            }),
            other => other,
        }
    }

    pub fn is_none(&self) -> bool {
        *self == Note::None
    }

    pub fn is_shortfall(&self) -> bool {
        matches!(self, Note::Missing(_))
    }

    pub fn from_legacy(text: &str) -> Note {
        const WORDED: [Note; 6] = [
            Note::Paused,
            Note::GolemDied,
            Note::TableGone,
            Note::ChestsFull,
            Note::HandsFull,
            Note::MissingBlueprint,
        ];
        if text.is_empty() {
            return Note::None;
        }
        if let Some(known) = WORDED.into_iter().find(|n| n.fixed_words() == Some(text)) {
            return known;
        }
        legacy_shortfall(text).map_or_else(|| Note::Legacy(text.to_owned()), Note::Missing)
    }

    fn fixed_words(&self) -> Option<&'static str> {
        Some(match self {
            Note::None => "",
            Note::Paused => "Paused",
            Note::GolemDied => "The golem died",
            Note::TableGone => "The schematic table is gone",
            Note::ChestsFull => CHESTS_FULL,
            Note::HandsFull => "The golem's hands are full",
            Note::MissingBlueprint => "Missing blueprint",
            Note::Missing(_) | Note::Report(_) | Note::Legacy(_) => return None,
        })
    }
}

const CHESTS_FULL: &str = "The chests are full";

fn legacy_shortfall(text: &str) -> Option<Shortfall> {
    let rest = text.strip_prefix(MISSING)?;
    let (count, rest) = rest.split_once("x ")?;
    let count = count.parse().ok()?;
    let (name, more) = match rest.strip_suffix(" and more") {
        Some(name) => (name, true),
        None => (rest, false),
    };
    Some(Shortfall {
        count,
        name: name.to_owned(),
        more,
    })
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut told: Vec<String> = Vec::new();
        match self.lost {
            0 => {}
            1 => told.push("1 block was lost after it was placed".into()),
            n => told.push(format!("{n} blocks were lost after they were placed")),
        }
        match self.standing {
            0 => {}
            1 => told.push("1 scaffold was out of reach and stands".into()),
            n => told.push(format!("{n} scaffolds were out of reach and stand")),
        }
        if self.chests_full {
            told.push(CHESTS_FULL.into());
        }
        f.write_str(&told.join("; "))
    }
}

impl fmt::Display for Note {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Note::Missing(short) => short.fmt(f),
            Note::Report(report) => report.fmt(f),
            Note::Legacy(text) => f.write_str(text),
            fixed => f.write_str(fixed.fixed_words().unwrap_or_default()),
        }
    }
}
