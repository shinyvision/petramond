//! What a plan that produced no step is waiting on.

/// A search or probe that ran out of budget before it had an answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Probe {
    ClimbSealing,
    CourseFlood,
    OnwardPillar,
    Onward,
    Pillar,
    Sealing,
    StanceSealing,
    Stance,
    Walk,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Waiting {
    #[default]
    Nothing,
    Resupply,
    FetchingTools,
    FetchingScaffold,
    ChestsUnread,
    SiteLoading,
    GroundLoading,
    CandidatesDeferred,
    DeferredOrUnloaded,
    Probe(Probe),
}

impl Waiting {
    /// The wait as the trace names it.
    pub fn label(self) -> &'static str {
        match self {
            Waiting::Nothing => "",
            Waiting::Resupply => "resupply",
            Waiting::FetchingTools => "fetching tools",
            Waiting::FetchingScaffold => "fetching blocks to stand on",
            Waiting::ChestsUnread => "waiting for the chests to read",
            Waiting::SiteLoading => "waiting for the site to load",
            Waiting::GroundLoading => "waiting for the ground around to load",
            Waiting::CandidatesDeferred => "candidates deferred this tick",
            Waiting::DeferredOrUnloaded => "waiting on deferred or unloaded work",
            Waiting::Probe(probe) => match probe {
                Probe::ClimbSealing => "climb sealing probe busy",
                Probe::CourseFlood => "course flood busy",
                Probe::OnwardPillar => "onward pillar search busy",
                Probe::Onward => "onward probe busy",
                Probe::Pillar => "pillar search busy",
                Probe::Sealing => "sealing probe busy",
                Probe::StanceSealing => "stance sealing probe busy",
                Probe::Stance => "stance search busy",
                Probe::Walk => "walk probe busy",
            },
        }
    }

    /// What a golem standing still is waiting for, in the owner's words. `None`
    /// for a wait of a tick or two that says nothing useful.
    pub fn told(self) -> Option<&'static str> {
        Some(match self {
            Waiting::Resupply | Waiting::FetchingTools => "Waiting for materials",
            Waiting::SiteLoading | Waiting::GroundLoading => "Waiting for the ground to load",
            Waiting::CandidatesDeferred | Waiting::DeferredOrUnloaded => {
                "Waiting to try the rest again"
            }
            _ => return None,
        })
    }

    /// The planner's shorter pauses, which the table never showed.
    pub fn pondering(self) -> Option<&'static str> {
        Some(match self {
            Waiting::FetchingScaffold => "Going for blocks to stand on",
            Waiting::ChestsUnread => "Waiting for the chests to load",
            Waiting::Probe(_) => "Working out a way to the next block",
            _ => return None,
        })
    }
}
