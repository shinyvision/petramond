//! Typed consumer-data rows: the one way a mod reads the `data` entries
//! other packs attach to their item, block and mob rows.
//!
//! Row data is the main extension surface between packs — a pack opts its own
//! rows into another mod's system by carrying that mod's namespaced key — so
//! its schema is the consumer's public contract. A consumer declares it as a
//! `serde::Deserialize` type (with `#[serde(deny_unknown_fields)]`, so a
//! misspelt field is an error instead of a silent default) and reads every
//! row through [`items_with_data_as`] / [`blocks_with_data_as`] /
//! [`mobs_with_data_as`]. A row that does not match is skipped with one
//! uniform log line naming the key, the row and serde's reason, so the pack
//! author sees exactly which row broke and why.

use mod_api::{BlockId, ItemId, MobId};
use serde::de::DeserializeOwned;

use crate::{block_names, blocks_with_data, item_names, items_with_data, log};
use crate::{mob_names, mobs_with_data};

/// Parse one row's raw JSON `value` for data key `key`. `Err` carries the
/// reason, for the caller's log line.
pub fn parse_row_data<T: DeserializeOwned>(value: &str) -> Result<T, String> {
    serde_json::from_str(value).map_err(|error| error.to_string())
}

/// Every item row carrying data entry `key`, its value parsed as `T`, in id
/// order. Rows that do not parse are logged and left out.
pub fn items_with_data_as<T: DeserializeOwned>(key: &str) -> Vec<(ItemId, T)> {
    typed_rows(key, items_with_data(key), |ids| item_names(ids.to_vec()))
}

/// The block twin of [`items_with_data_as`].
pub fn blocks_with_data_as<T: DeserializeOwned>(key: &str) -> Vec<(BlockId, T)> {
    typed_rows(key, blocks_with_data(key), |ids| block_names(ids.to_vec()))
}

/// The mob-species twin of [`items_with_data_as`].
pub fn mobs_with_data_as<T: DeserializeOwned>(key: &str) -> Vec<(MobId, T)> {
    typed_rows(key, mobs_with_data(key), |ids| mob_names(ids.to_vec()))
}

/// Parse `rows`, naming each rejected row (one batched name lookup, only
/// when something was rejected) in its log line.
fn typed_rows<Id: Copy + std::fmt::Debug, T: DeserializeOwned>(
    key: &str,
    rows: Vec<(Id, String)>,
    names: impl FnOnce(&[Id]) -> Vec<Option<String>>,
) -> Vec<(Id, T)> {
    let (parsed, rejected) = split_rows(rows);
    if !rejected.is_empty() {
        let ids: Vec<Id> = rejected.iter().map(|(id, _)| *id).collect();
        let names = names(&ids);
        for (index, (id, reason)) in rejected.iter().enumerate() {
            let row = names
                .get(index)
                .cloned()
                .flatten()
                .unwrap_or_else(|| format!("{id:?}"));
            log(&row_error(key, &row, reason));
        }
    }
    parsed
}

/// The rows that parse, and the rejected ones with serde's reason.
type SplitRows<Id, T> = (Vec<(Id, T)>, Vec<(Id, String)>);

fn split_rows<Id, T: DeserializeOwned>(rows: Vec<(Id, String)>) -> SplitRows<Id, T> {
    let mut parsed = Vec::with_capacity(rows.len());
    let mut rejected = Vec::new();
    for (id, raw) in rows {
        match parse_row_data::<T>(&raw) {
            Ok(value) => parsed.push((id, value)),
            Err(reason) => rejected.push((id, reason)),
        }
    }
    (parsed, rejected)
}

/// Every row of a pack document carrying data entry `key`, as (row name, the
/// entry's raw JSON): `document` is the text of e.g. `mobs.json`, whose rows
/// sit under `table` (`"mobs"`) and are named by the singular field
/// (`"mob"`), or by `"patch"` for a row patching another pack's. For a
/// consumer's test that every row it ships parses into its schema; an
/// unparsable document yields no rows.
pub fn pack_rows_with_data(document: &str, table: &str, key: &str) -> Vec<(String, String)> {
    let Ok(document) = serde_json::from_str::<serde_json::Value>(document) else {
        return Vec::new();
    };
    let name_field = table.strip_suffix('s').unwrap_or(table);
    document
        .get(table)
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let entry = row.get("data")?.get(key)?;
            let name = [name_field, "patch"]
                .iter()
                .find_map(|field| row.get(*field).and_then(serde_json::Value::as_str))
                .unwrap_or("?");
            Some((name.to_owned(), entry.to_string()))
        })
        .collect()
}

/// The one log line a rejected row produces.
pub fn row_error(key: &str, row: &str, reason: &str) -> String {
    format!("row data '{key}' on '{row}' is ignored: {reason}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(deny_unknown_fields)]
    struct Vessel {
        filled: String,
        #[serde(default)]
        capacity: u8,
    }

    #[test]
    fn matching_rows_parse_and_the_rest_are_rejected_with_a_reason() {
        let rows = vec![
            (1u16, r#"{"filled":"a:full","capacity":3}"#.to_owned()),
            (2, r#"{"filed":"a:typo"}"#.to_owned()),
            (3, r#"{"filled":"b:full"}"#.to_owned()),
            (4, "not json".to_owned()),
        ];
        let (parsed, rejected) = split_rows::<_, Vessel>(rows);
        assert_eq!(
            parsed,
            vec![
                (
                    1,
                    Vessel {
                        filled: "a:full".into(),
                        capacity: 3
                    }
                ),
                (
                    3,
                    Vessel {
                        filled: "b:full".into(),
                        capacity: 0
                    }
                ),
            ]
        );
        assert_eq!(
            rejected.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            [2, 4]
        );
        assert!(rejected[0].1.contains("filed"), "{}", rejected[0].1);
    }

    #[test]
    fn a_pack_documents_rows_are_listed_with_their_names() {
        let document = r#"{"items": [
            {"item": "a:bucket", "data": {"x:vessel": {"filled": "a:full"}}},
            {"item": "a:stick"},
            {"item": "a:jar", "data": {"x:vessel": {"filled": "a:jam"}, "y:other": 1}},
            {"patch": "b:pot", "data": {"x:vessel": {"filled": "b:stew"}}}
        ]}"#;
        let rows = pack_rows_with_data(document, "items", "x:vessel");
        assert_eq!(
            rows,
            vec![
                ("a:bucket".to_owned(), r#"{"filled":"a:full"}"#.to_owned()),
                ("a:jar".to_owned(), r#"{"filled":"a:jam"}"#.to_owned()),
                ("b:pot".to_owned(), r#"{"filled":"b:stew"}"#.to_owned()),
            ]
        );
        assert!(pack_rows_with_data("not json", "items", "x:vessel").is_empty());
    }

    #[test]
    fn a_rejected_row_is_named_in_its_log_line() {
        let line = row_error("x:vessel", "y:bucket", "missing field `filled`");
        assert_eq!(
            line,
            "row data 'x:vessel' on 'y:bucket' is ignored: missing field `filled`"
        );
    }
}
