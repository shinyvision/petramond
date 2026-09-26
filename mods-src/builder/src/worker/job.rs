//! One project's work in hand: its compiled design, the survey of what the
//! world already holds, and the crew building it. The worker owns the type
//! it works on; the job registry ([`crate::jobs`]) only keeps them, so the
//! dependency runs one way.

use crate::design::Design;
use crate::project::ProjectId;
use crate::survey::{Summary, Survey};

use super::Crew;

pub struct Job {
    pub id: ProjectId,
    pub design: Design,
    pub survey: Option<Survey>,
    pub failed: Option<String>,
    pub crew: Crew,
}

impl Job {
    /// A fresh job over `design`, nothing surveyed and no golem yet.
    pub fn new(id: ProjectId, design: Design) -> Self {
        Self {
            id,
            design,
            survey: None,
            failed: None,
            crew: Crew::default(),
        }
    }

    pub fn summary(&self) -> Option<&Summary> {
        self.survey.as_ref()?.summary()
    }

    /// The fraction of the design's units the world holds.
    pub fn done(&self) -> f32 {
        let Some(survey) = self.survey.as_ref() else {
            return 0.0;
        };
        if survey.known.is_empty() {
            return 1.0;
        }
        1.0 - survey.open() as f32 / survey.known.len() as f32
    }
}
