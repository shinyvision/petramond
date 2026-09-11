use std::sync::LazyLock;

use super::CaveField;
use crate::memo::SharedMemo;

type Query = ([i32; 3], i32);

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key<Q> {
    seed: u32,
    tables: [usize; 2],
    queries: Q,
}

type QueryMemo = SharedMemo<Key<Box<[Query]>>, Vec<bool>>;
static QUERIES: LazyLock<QueryMemo> = LazyLock::new(|| SharedMemo::new(512));
const MAX_CACHED_POINTS: usize = 4096;

impl CaveField {
    pub(super) fn cache_carve_queries(
        &self,
        queries: &[Query],
        out: &mut Vec<bool>,
        evaluate: impl FnOnce(&mut Vec<bool>),
    ) {
        if queries.len() > MAX_CACHED_POINTS {
            evaluate(out);
            return;
        }
        let key = Key {
            seed: self.seed,
            tables: [
                std::ptr::from_ref(self.underground) as usize,
                std::ptr::from_ref(self.excavations) as usize,
            ],
            queries,
        };
        if let Some(answers) = QUERIES.find(&key, |saved| {
            saved.seed == key.seed
                && saved.tables == key.tables
                && saved.queries.as_ref() == queries
        }) {
            *out = answers;
            return;
        }
        evaluate(out);
        QUERIES.insert(
            Key {
                seed: key.seed,
                tables: key.tables,
                queries: queries.into(),
            },
            out.clone(),
        );
    }
}
