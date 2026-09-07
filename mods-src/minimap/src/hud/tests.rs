use super::*;

#[test]
fn compass_text_and_outline_stay_inside_the_image_through_a_full_rotation() {
    let size = [14, 28];
    for degrees in 0..360 {
        let yaw = (degrees as f32).to_radians();
        let runs = cardinal_text_runs([-yaw.cos(), yaw.sin()], [yaw.sin(), yaw.cos()], size);
        for run in runs {
            for (axis, extent) in size.iter().enumerate() {
                assert!(run.position[axis] >= 0);
                assert!(run.position[axis] + i32::from(*extent) <= HUD_SIZE as i32);
            }
        }
    }
}
