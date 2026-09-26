use super::*;
use crate::caches::Caches;
use crate::host::fake::rows::{AIR, CHEST, TABLE};
use crate::host::fake::{stack, Fake};
use crate::testing::Session;

const TABLE_AT: [i32; 3] = [0, 0, 0];

/// A table with a chest beside it, a second chest touching the first, and
/// a third standing apart.
fn chained() -> Session {
    let session = Session::flat(8);
    session.world.set(TABLE_AT, TABLE);
    session.world.chest([1, 0, 0], 4);
    session.world.chest([2, 0, 0], 4);
    session.world.chest([0, 0, 3], 4);
    session
}

fn chest(world: &Fake, cell: [i32; 3], item: &str, count: u8) {
    world.give(ContainerAddress::Block(cell), item, count);
}

fn key(item: &str) -> ItemKey {
    (item.into(), Vec::new())
}

#[test]
fn a_chain_runs_from_the_table_through_touching_chests_nearest_first() {
    let _session = chained();
    let supplies = Supplies::resolve();
    assert_eq!(supplies.chain(TABLE_AT), vec![[1, 0, 0], [2, 0, 0]]);
}

#[test]
fn a_double_chest_is_one_container() {
    let session = chained();
    for half in [[1, 0, 0], [2, 0, 0]] {
        session.world.state_mut().groups.insert(half, [1, 0, 0]);
    }
    assert_eq!(Supplies::resolve().chain(TABLE_AT), vec![[1, 0, 0]]);
}

#[test]
fn a_chain_ends_at_the_most_containers_one_reaches() {
    let session = Session::flat(2);
    session.world.set(TABLE_AT, TABLE);
    for x in 1..=40 {
        session.world.chest([x, 0, 0], 1);
    }
    let chain = Supplies::resolve().chain(TABLE_AT);
    assert_eq!(chain.len(), MAX_CONTAINERS);
    assert_eq!(chain.last(), Some(&[MAX_CONTAINERS as i32, 0, 0]));
}

#[test]
fn a_walked_chain_stands_until_a_cell_it_looked_at_changes() {
    let session = chained();
    let supplies = Supplies::resolve();
    assert_eq!(supplies.chain(TABLE_AT).len(), 2);
    session.world.chest([3, 0, 0], 4);
    assert_eq!(supplies.chain(TABLE_AT).len(), 2, "nobody said");
    supplies.changed(&[[3, 0, 0]], false);
    assert_eq!(supplies.chain(TABLE_AT).len(), 3);

    session.world.set([3, 0, 0], AIR);
    supplies.changed(&[[9, 0, 9]], false);
    assert_eq!(
        supplies.chain(TABLE_AT).len(),
        3,
        "a cell the walk never looked at"
    );
    supplies.changed(&[], true);
    assert_eq!(supplies.chain(TABLE_AT).len(), 2, "a lost log drops them all");
}

#[test]
fn a_chain_that_met_unloaded_ground_is_walked_again() {
    let session = chained();
    session.world.chest([3, 0, 0], 4);
    session.world.unload([3, 0, 0], [3, 0, 0]);
    let supplies = Supplies::resolve();
    assert_eq!(supplies.chain(TABLE_AT).len(), 2);
    session.world.state_mut().unloaded.clear();
    assert_eq!(supplies.chain(TABLE_AT).len(), 3);
}

#[test]
fn stock_totals_what_every_chest_holds() {
    let session = chained();
    chest(&session.world, [1, 0, 0], "petramond:dirt", 5);
    chest(&session.world, [2, 0, 0], "petramond:dirt", 3);
    chest(&session.world, [2, 0, 0], "petramond:stone", 2);
    chest(&session.world, [0, 0, 3], "petramond:stone", 50);
    let stock = Supplies::resolve().stock(TABLE_AT);
    assert!(stock.read);
    assert_eq!(stock.containers, vec![[1, 0, 0], [2, 0, 0]]);
    let totals: Vec<(ItemKey, u32)> = stock.totals.into_iter().collect();
    assert_eq!(
        totals,
        vec![(key("petramond:dirt"), 8), (key("petramond:stone"), 2)]
    );
    assert!(stock.slots.iter().flatten().any(Option::is_none));
}

#[test]
fn full_chests_have_no_room_and_an_unread_one_is_no_answer() {
    let session = chained();
    for (cell, item) in [([1, 0, 0], "petramond:dirt"), ([2, 0, 0], "petramond:stone")] {
        for _ in 0..4 {
            chest(&session.world, cell, item, 64);
        }
    }
    let supplies = Supplies::resolve();
    let stock = supplies.stock(TABLE_AT);
    assert!(stock.read && !stock.has_room());

    // A chest block whose contents do not answer (out of the loaded world).
    session.world.set([3, 0, 0], CHEST);
    supplies.changed(&[[3, 0, 0]], false);
    let stock = supplies.stock(TABLE_AT);
    assert_eq!(stock.containers.len(), 3);
    assert!(!stock.read);
}

#[test]
fn stock_is_read_once_a_tick() {
    let session = chained();
    chest(&session.world, [1, 0, 0], "petramond:dirt", 5);
    let supplies = Supplies::resolve();
    let first = supplies.stock_at(TABLE_AT, 10);
    chest(&session.world, [1, 0, 0], "petramond:dirt", 5);
    assert!(Rc::ptr_eq(&first, &supplies.stock_at(TABLE_AT, 10)));
    assert_eq!(
        supplies.stock_at(TABLE_AT, 11).totals.get(&key("petramond:dirt")),
        Some(&10)
    );
    supplies.sweep(12);
    assert!(!Rc::ptr_eq(&first, &supplies.stock_at(TABLE_AT, 12)));
}

#[test]
fn a_shortfall_is_the_biggest_gap_first_named_for_the_owner() {
    let _session = Session::flat(1);
    let bill: BTreeMap<ItemKey, u32> = [
        (key("petramond:stone"), 5),
        (key("petramond:dirt"), 3),
        (key("petramond:oak_planks"), 2),
    ]
    .into_iter()
    .collect();
    let mut have = BTreeMap::new();
    add_totals(
        &mut have,
        &[
            Some(stack("petramond:stone", 1)),
            None,
            Some(stack("petramond:dirt", 3)),
        ],
    );
    let short = shortfall(&bill, &have);
    assert_eq!(
        short,
        vec![(key("petramond:stone"), 4), (key("petramond:oak_planks"), 2)]
    );
    let named = worst(&short, &mut Caches::default()).expect("something is short");
    assert_eq!(named.to_string(), "Missing 4x Stone and more");
    assert!(crate::project::Note::read(&named.to_string()).is_shortfall());

    let even: BTreeMap<ItemKey, u32> = [(key("petramond:stone"), 2), (key("petramond:dirt"), 2)]
        .into_iter()
        .collect();
    let short = shortfall(&even, &BTreeMap::new());
    assert_eq!(short[0].0, key("petramond:dirt"), "equal gaps by name");
    let one = &short[1..];
    let named = worst(one, &mut Caches::default()).unwrap();
    assert_eq!((named.to_string(), named.more), ("Missing 2x Stone".into(), false));
    assert!(worst(&[], &mut Caches::default()).is_none());
}
