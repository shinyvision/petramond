use super::{pin, Content};

impl Content {
    pub fn scope<R>(self, work: impl FnOnce() -> R) -> R {
        let _content = pin(self);
        work()
    }

    pub fn task<R>(self, work: impl FnOnce() -> R) -> impl FnOnce() -> R {
        move || self.scope(work)
    }
}

pub fn parallel_map<T: Sync, R: Send>(rows: &[T], work: impl Fn(&T) -> R + Sync + Send) -> Vec<R> {
    use rayon::prelude::*;
    let content = Content::current();
    rows.par_iter()
        .map(|row| content.scope(|| work(row)))
        .collect()
}
