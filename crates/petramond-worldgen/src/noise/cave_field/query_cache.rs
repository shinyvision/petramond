use super::CaveField;
use crate::cache::Memo;

pub(super) type Query = ([i32; 3], i32);

#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) struct Key<Q> {
    seed: u32,
    tables: [usize; 2],
    queries: Q,
}

pub(super) type QueryMemo<T> = Memo<Key<Box<[Query]>>, Vec<T>>;
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
