use mod_api::{EventKind, GuestRet, Outcome};

use crate::events::{PostEvent, RosterRefs};
use crate::player::PlayerId;

use super::super::ModHost;
use super::{echoing_guest, Sim};

#[test]
fn a_client_event_reaches_only_the_mod_owning_its_key() {
    let reply = GuestRet::Event {
        outcome: Outcome::Continue,
        payload: None,
    };
    let listener = |id| echoing_guest(id, EventKind::ClientEvent, &reply);
    let mut sim = Sim::new();
    let mut host = ModHost::from_instances(vec![listener("alpha"), listener("beta")]);
    sim.init(&mut host);
    let heard = |host: &ModHost| [host.probe(0).1, host.probe(1).1];
    let before = heard(&host);

    for key in ["alpha:picked", "alpha_two:picked", "gamma:picked"] {
        sim.bus.emit(PostEvent::ClientEvent {
            player: PlayerId(7),
            key: key.into(),
            data: vec![1, 2, 3],
        });
    }
    sim.bus
        .drain_post(&mut sim.world, &mut RosterRefs::empty(), &mut sim.feed);

    assert_eq!(
        heard(&host),
        [before[0] + 1, before[1]],
        "one dispatch to the owner, none to a mod whose namespace the key does not name"
    );
    assert!(!host.probe(0).0 && !host.probe(1).0);
}
