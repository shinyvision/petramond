use crate::events::{PostEvent, PostEventKind};
use crate::server::mod_runtime::ModRuntime;
use crate::world::{ServerWorld, StreamEvent};

pub(super) fn pump_stream_events(world: &mut ServerWorld, mods: &mut ModRuntime) {
    let wants = mods.bus().wants(PostEventKind::SectionGenerated)
        || mods.bus().wants(PostEventKind::SectionLoaded);
    world.set_stream_event_capture(wants);
    if !wants {
        return;
    }
    for ev in world.take_stream_events() {
        mods.emit(match ev {
            StreamEvent::Generated(pos) => PostEvent::SectionGenerated { pos },
            StreamEvent::Loaded(pos) => PostEvent::SectionLoaded { pos },
        });
    }
}
