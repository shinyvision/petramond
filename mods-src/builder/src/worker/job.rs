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
