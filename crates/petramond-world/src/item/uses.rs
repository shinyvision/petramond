#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ItemUse {
    BucketFill {
        fills: &'static [(crate::block::Block, super::ItemType)],
    },
    BucketPour {
        becomes: super::ItemType,
        fluid: crate::block::Block,
    },
    Shear,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum UseRay {
    #[default]
    Solid,
    Fluids(&'static [crate::block::Block]),
}

impl UseRay {
    pub fn stops_at(self, fluid: crate::block::Block) -> bool {
        match self {
            UseRay::Solid => false,
            UseRay::Fluids(fluids) => fluids.contains(&fluid),
        }
    }

    pub fn sees_fluid(self) -> bool {
        matches!(self, UseRay::Fluids(fluids) if !fluids.is_empty())
    }
}
