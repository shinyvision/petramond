use std::sync::LazyLock;

use super::CaveField;
use crate::memo::SharedMemo;

type Query = ([i32; 3], i32);

#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) struct Key<Q> {
    seed: u32,
    tables: [usize; 2],
    queries: Q,
}

pub(super) type QueryMemo<T> = SharedMemo<Key<Box<[Query]>>, Vec<T>>;
/// Openness answers.
pub(super) static CARVED: LazyLock<QueryMemo<bool>> = LazyLock::new(|| SharedMemo::new(512));
/// What each open cell holds.
pub(super) static FILLED: LazyLock<QueryMemo<Option<u16>>> = LazyLock::new(|| SharedMemo::new(512));
const MAX_CACHED_POINTS: usize = 4096;

impl CaveField {
    pub(super) fn cache_carve_queries<T: Clone>(
        &self,
        memo: &QueryMemo<T>,
        queries: &[Query],
        out: &mut Vec<T>,
        evaluate: impl FnOnce(&mut Vec<T>),
    ) {
        if queries.len() > MAX_CACHED_POINTS {
            evaluate(out);
            return;
        }
        let key = Key {
            seed: self.seed,
            tables: self.table_identities(),
            queries,
        };
        if let Some(answers) = memo.find(&key, |saved| {
            saved.seed == key.seed
                && saved.tables == key.tables
                && saved.queries.as_ref() == queries
        }) {
            *out = answers;
            return;
        }
        evaluate(out);
        memo.insert(
            Key {
                seed: key.seed,
                tables: key.tables,
                queries: queries.into(),
            },
            out.clone(),
        );
    }
}
