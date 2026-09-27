use super::BlockBehavior;

pub struct Inert;

impl BlockBehavior for Inert {
    fn key(&self) -> &'static str {
        "inert"
    }
}

pub static INERT: Inert = Inert;
