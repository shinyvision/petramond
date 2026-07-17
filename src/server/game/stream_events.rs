use crate::events::{PostEvent, PostEventKind};
use crate::world::StreamEvent;

use super::ServerGame;

impl ServerGame {
    /// Hand the section stream events buffered by the per-frame `World::poll` to
    /// the bus. The capture gate mirrors listener presence so an idle bus costs
    /// the streamer nothing.
    pub(super) fn pump_stream_events(&mut self) {
        let wants = self.bus.wants(PostEventKind::SectionGenerated)
            || self.bus.wants(PostEventKind::SectionLoaded);
        self.world.set_stream_event_capture(wants);
        if !wants {
            return;
        }
        for ev in self.world.take_stream_events() {
            self.bus.emit(match ev {
                StreamEvent::Generated(pos) => PostEvent::SectionGenerated { pos },
                StreamEvent::Loaded(pos) => PostEvent::SectionLoaded { pos },
            });
        }
    }
}
