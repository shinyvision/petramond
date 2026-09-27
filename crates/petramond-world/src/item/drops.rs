use super::ItemType;

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Drop {
    pub item: ItemType,
    pub min: u8,
    pub max: u8,
    pub chance: f32,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct DropSpec {
    pub drops: &'static [Drop],
}

impl DropSpec {
    pub const NONE: DropSpec = DropSpec { drops: &[] };
}
