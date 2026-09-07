//! Field-naming range checks for catalog rows: every error names the field,
//! the value, and the bound it broke, so a bad row is a one-line fix.

use std::fmt::Display;
use std::ops::RangeInclusive;

/// `v` lies within `allowed`.
pub fn within<T: PartialOrd + Display>(
    field: &str,
    v: T,
    allowed: RangeInclusive<T>,
) -> Result<(), String> {
    if allowed.contains(&v) {
        Ok(())
    } else {
        Err(format!(
            "{field}: {v} must lie within {}..={}",
            allowed.start(),
            allowed.end()
        ))
    }
}

/// `range` is ascending (`lo <= hi`) and both ends lie within `allowed`.
pub fn ascending<T: PartialOrd + Display + Copy>(
    field: &str,
    range: (T, T),
    allowed: RangeInclusive<T>,
) -> Result<(), String> {
    if range.0 > range.1 {
        return Err(format!(
            "{field}: [{}, {}] must be ascending",
            range.0, range.1
        ));
    }
    within(field, range.0, allowed.clone())?;
    within(field, range.1, allowed)
}

/// `v` is a finite fraction in `0..=1`.
pub fn unit(field: &str, v: f32) -> Result<(), String> {
    if !v.is_finite() {
        return Err(format!("{field}: must be a finite number"));
    }
    within(field, v, 0.0..=1.0)
}

/// `range` is an ascending pair of finite fractions in `0..=1`.
pub fn unit_range(field: &str, range: (f32, f32)) -> Result<(), String> {
    unit(field, range.0)?;
    unit(field, range.1)?;
    ascending(field, range, 0.0..=1.0)
}
