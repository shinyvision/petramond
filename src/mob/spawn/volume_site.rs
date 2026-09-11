use super::*;

pub(super) fn find(
    world: &World,
    kind: Mob,
    rule: &super::super::SpawnRule,
    x: i32,
    z: i32,
    [lo, hi]: [i32; 2],
) -> Option<Vec3> {
    let biome = Biome::from_id(world.column_biome(x, z)?);
    let mut candidates = Vec::new();
    for y in lo..=hi {
        let ground = if rule.ground.is_empty() {
            Some(Block::Air)
        } else {
            world.block_if_stream_final(x, y - 1, z)
        };
        let Some(ground) = ground else {
            continue;
        };
        if rule.admits(biome, ground) {
            candidates.push([x, y, z]);
        }
    }
    let territories = if rule.underground.is_empty() {
        vec![0; candidates.len()]
    } else {
        petramond_worldgen::underground_biomes_at(world.seed, &candidates)
    };
    // A positional start avoids always preferring the highest cave floor.
    let offset = splitmix((x as i64 as u64) ^ (z as i64 as u64).rotate_left(32)) as usize;
    for n in 0..candidates.len() {
        let at = offset.wrapping_add(n) % candidates.len();
        let [x, y, z] = candidates[at];
        if !rule.underground.is_empty() && !rule.underground.contains(&territories[at]) {
            continue;
        }
        let feet = IVec3::new(x, y, z);
        let pos = Vec3::new(x as f32 + 0.5, y as f32, z as f32 + 0.5);
        let fits = match rule.space {
            Some(space) => body_in_space(world, kind, pos, 0.0, space),
            None => body_fits_at(world, kind, feet),
        };
        if fits && world.mob_spawn_pose_clear(kind, pos, 0.0) {
            return Some(pos);
        }
    }
    None
}

pub(super) fn body_in_space(
    world: &World,
    kind: Mob,
    pos: Vec3,
    yaw: f32,
    allowed: &[Block],
) -> bool {
    let size = def(kind).size;
    let half_length = size.half_length.unwrap_or(size.half_width);
    let (sin, cos) = yaw.sin_cos();
    let x = size.half_width * cos.abs() + half_length * sin.abs();
    let z = size.half_width * sin.abs() + half_length * cos.abs();
    let low = pos - Vec3::new(x, 0.0, z);
    let high = pos + Vec3::new(x, size.height, z) - Vec3::splat(1e-4);
    for y in low.y.floor() as i32..=high.y.floor() as i32 {
        for z in low.z.floor() as i32..=high.z.floor() as i32 {
            for x in low.x.floor() as i32..=high.x.floor() as i32 {
                if !world
                    .block_if_stream_final(x, y, z)
                    .is_some_and(|block| allowed.contains(&block))
                {
                    return false;
                }
            }
        }
    }
    true
}

#[cfg(test)]
mod tests;
