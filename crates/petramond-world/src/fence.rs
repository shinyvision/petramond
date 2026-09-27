use crate::block::Aabb;

pub const RAIL_INSET: f32 = 0.5 / 16.0;

#[inline]
pub const fn rail_cross(post_lo: f32, post_hi: f32) -> (f32, f32) {
    (post_lo + RAIL_INSET, post_hi - RAIL_INSET)
}

pub const RAIL_TOP_LO: f32 = 11.0 / 16.0;
pub const RAIL_TOP_HI: f32 = 14.0 / 16.0;
pub const RAIL_BOT_LO: f32 = 2.0 / 16.0;
pub const RAIL_BOT_HI: f32 = 5.0 / 16.0;

pub const fn item_posts(post_lo: f32, post_hi: f32) -> [Aabb; 2] {
    let thickness = post_hi - post_lo;
    [
        Aabb {
            min: [0.0, 0.0, post_lo],
            max: [thickness, 1.0, post_hi],
        },
        Aabb {
            min: [1.0 - thickness, 0.0, post_lo],
            max: [1.0, 1.0, post_hi],
        },
    ]
}

pub const fn item_rails(post_lo: f32, post_hi: f32) -> [Aabb; 2] {
    let thickness = post_hi - post_lo;
    let rail_lo = post_lo + RAIL_INSET;
    let rail_hi = post_hi - RAIL_INSET;
    [
        Aabb {
            min: [thickness, RAIL_TOP_LO, rail_lo],
            max: [1.0 - thickness, RAIL_TOP_HI, rail_hi],
        },
        Aabb {
            min: [thickness, RAIL_BOT_LO, rail_lo],
            max: [1.0 - thickness, RAIL_BOT_HI, rail_hi],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const POST_LO: f32 = 6.0 / 16.0;
    const POST_HI: f32 = 10.0 / 16.0;

    #[test]
    fn item_shape_is_two_posts_bridged_by_the_rails() {
        let posts = item_posts(POST_LO, POST_HI);
        for post in posts {
            assert_eq!(post.min[1], 0.0);
            assert_eq!(post.max[1], 1.0);
            assert_eq!(post.min[2], POST_LO);
            assert_eq!(post.max[2], POST_HI);
        }
        let [west_post, east_post] = posts;
        for rail in item_rails(POST_LO, POST_HI) {
            assert_eq!(rail.min[0], west_post.max[0]);
            assert_eq!(rail.max[0], east_post.min[0]);
            assert!(rail.min[1] > 0.0 && rail.max[1] < 1.0);
            assert!(rail.min[2] >= POST_LO && rail.max[2] <= POST_HI);
        }
    }
}
