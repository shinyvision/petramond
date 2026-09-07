use super::*;
use crate::world::store::LoadTarget;
use petramond_world::section::Section;

#[test]
fn recurring_arrivals_cannot_postpone_a_mesh_forever_and_nearby_work_does_not_wait() {
    let mut world = World::new(1, 0);
    let pos = SectionPos::new(12, 0, 0);
    world.insert_section_for_test(pos, Section::new(pos.cx, pos.cy, pos.cz));
    world.last_load_target = Some(LoadTarget::new(0, 0, 0, 16));
    world.defer_stream_mesh(pos);
    assert!(world.stream_mesh_waiting(pos));
    world.terrain.mesh_pump_frame += 3;
    world.defer_stream_mesh(pos);
    assert!(world.stream_mesh_waiting(pos));
    world.terrain.mesh_pump_frame += 1;
    assert!(!world.stream_mesh_waiting(pos));
    world.defer_stream_mesh(pos);
    world.last_load_target = Some(LoadTarget::new(pos.cx, pos.cy, pos.cz, 16));
    assert!(!world.stream_mesh_waiting(pos));
}
