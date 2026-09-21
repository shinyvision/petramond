use super::*;

#[test]
fn the_view_is_rebuilt_only_when_the_query_changes() {
    let mut catalog = Catalog::default();
    let (all, rows) = catalog.view("");
    let (again, rows_again) = catalog.view("");
    assert!(Arc::ptr_eq(&all, &again) && Arc::ptr_eq(&rows, &rows_again));

    let name = all.first().expect("a visible item").name().to_uppercase();
    let (found, found_rows) = catalog.view(&name);
    assert!(!Arc::ptr_eq(&all, &found));
    assert!(found.contains(&all[0]), "search ignores case");
    assert_eq!(found.len(), found_rows.len());
}
