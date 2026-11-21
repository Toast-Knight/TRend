//! The little vector math the renderer needs. `f32` throughout.

/// A 3D vector or point.
pub type Vec3 = [f32; 3];

pub(crate) fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub(crate) fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub(crate) fn scale(a: Vec3, factor: f32) -> Vec3 {
    [a[0] * factor, a[1] * factor, a[2] * factor]
}

pub(crate) fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub(crate) fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub(crate) fn length(a: Vec3) -> f32 {
    dot(a, a).sqrt()
}

/// `a` scaled to length 1, or `fallback` when `a` has no direction.
pub(crate) fn normalize_or(a: Vec3, fallback: Vec3) -> Vec3 {
    let length = length(a);
    if length > f32::EPSILON && length.is_finite() {
        scale(a, 1.0 / length)
    } else {
        fallback
    }
}

pub(crate) fn lerp(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_operations() {
        assert_eq!(cross([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]), [0.0, 0.0, 1.0]);
        assert_eq!(dot([1.0, 2.0, 3.0], [4.0, 5.0, 6.0]), 32.0);
        assert_eq!(
            normalize_or([0.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
            [0.0, 1.0, 0.0]
        );
        assert_eq!(
            normalize_or([0.0, 3.0, 4.0], [1.0, 0.0, 0.0]),
            [0.0, 0.6, 0.8]
        );
        assert_eq!(lerp([0.0; 3], [2.0, 4.0, 6.0], 0.5), [1.0, 2.0, 3.0]);
    }
}
