use super::*;
fn stack(count: u8, data: Vec<(String, Vec<u8>)>) -> Option<ItemStackData> {
    Some(ItemStackData {
        item: "test:metal".into(),
        count,
        data,
    })
}
#[test]
fn purchases_plan_all_costs_and_preserve_variant_identity() {
    let inventory = vec![
        stack(2, vec![]),
        stack(3, vec![("test:mark".into(), vec![9])]),
    ];
    let cost = vec![("test:metal".into(), 4)];
    let plan = purchase_plan(&inventory, &cost).unwrap();
    assert_eq!(plan.iter().map(|s| s.count as u32).sum::<u32>(), 4);
    assert_eq!(plan[1].data, inventory[1].as_ref().unwrap().data);
    assert!(purchase_plan(
        &inventory,
        &[("test:metal".into(), 3), ("test:metal".into(), 3)]
    )
    .is_none());
    assert_eq!(inventory[0].as_ref().unwrap().count, 2);
}
