//! Every callback id the pack registers with the host, one enum per kind of callback, so no two
//! subsystems can share an id. The host hands the id back on each call, and the pack's
//! [`Mod`](mod_sdk::Mod) impl routes it to the subsystem that registered it.

macro_rules! routes {
    ($(#[$meta:meta])* $name:ident { $($variant:ident = $id:literal,)+ }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        pub enum $name {
            $($variant = $id,)+
        }

        impl $name {
            pub const fn id(self) -> u32 {
                self as u32
            }

            pub fn from_id(id: u32) -> Option<$name> {
                [$($name::$variant),+].into_iter().find(|r| r.id() == id)
            }
        }
    };
}

routes!(TickSystem {
    Sunburn = 1,
    SkeletonCombat = 2,
    Garrison = 3,
    ShoveReset = 4,
});

routes!(Handler {
    WeatherFeed = 1,
    SectionGenerated = 2,
    SectionLoaded = 3,
    SkeletonDamaged = 4,
    SkeletonDied = 5,
    PlayerDamaged = 6,
});

routes!(Feature { Camps = 1, });

routes!(Spawner { Hostiles = 1, });

routes!(AiNode { SkeletonPost = 1, });
