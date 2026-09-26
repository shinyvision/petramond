use super::CaveField;
use crate::cache::{GenContext, Memo};

pub(super) type Query = ([i32; 3], i32);

#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) struct Key<Q> {
    context: GenContext,
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
            context: self.context(),
            queries,
        };
        if let Some(answers) = memo.find(&key, |saved| {
            saved.context == key.context && saved.queries.as_ref() == queries
        }) {
            *out = answers;
            return;
        }
        evaluate(out);
        memo.insert(
            Key {
                context: key.context,
                queries: queries.into(),
            },
            out.clone(),
        );
    }
}
