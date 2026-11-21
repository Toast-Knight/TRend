//! An orbit camera: it circles a target point, the way model viewers do.

use crate::math::{Vec3, add, cross, dot, normalize_or, scale, sub};
use crate::mesh::{Bounds, Mesh};

/// Which world axis points up. OBJ, PLY and glTF models are usually Y-up; STL (from CAD and 3D
/// printing) is usually Z-up.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum UpAxis {
    #[default]
    Y,
    Z,
}

impl UpAxis {
    pub const fn toggled(self) -> Self {
        match self {
            Self::Y => Self::Z,
            Self::Z => Self::Y,
        }
    }

    /// Maps a direction in the Y-up camera frame to the world.
    fn to_world(self, [x, y, z]: Vec3) -> Vec3 {
        match self {
            Self::Y => [x, y, z],
            // A 90° turn about X: local Y becomes world Z.
            Self::Z => [x, -z, y],
        }
    }

    fn vector(self) -> Vec3 {
        self.to_world([0.0, 1.0, 0.0])
    }
}

/// The camera's view: where it is and which way its axes point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewBasis {
    pub eye: Vec3,
    pub right: Vec3,
    pub up: Vec3,
    /// Points from the eye into the scene.
    pub forward: Vec3,
}

impl ViewBasis {
    /// `point` in view space: x right, y up, z distance in front of the eye.
    pub fn to_view(&self, point: Vec3) -> Vec3 {
        let relative = sub(point, self.eye);
        [
            dot(relative, self.right),
            dot(relative, self.up),
            dot(relative, self.forward),
        ]
    }

    /// A direction (such as a normal) in view space.
    pub fn direction_to_view(&self, direction: Vec3) -> Vec3 {
        [
            dot(direction, self.right),
            dot(direction, self.up),
            dot(direction, self.forward),
        ]
    }
}

/// A camera orbiting `target` at `distance`, turned by `yaw` about the up axis and tilted by `pitch`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrbitCamera {
    pub target: Vec3,
    /// Radians about the up axis.
    pub yaw: f32,
    /// Radians above (positive) or below the horizon, kept inside ±89°.
    pub pitch: f32,
    pub distance: f32,
    /// Vertical field of view, radians.
    pub fov_y: f32,
    pub up: UpAxis,
    /// Size of the subject, which bounds zoom and sets the near plane.
    pub subject_radius: f32,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            target: [0.0; 3],
            yaw: Self::DEFAULT_YAW,
            pitch: Self::DEFAULT_PITCH,
            distance: 3.0,
            fov_y: 40_f32.to_radians(),
            up: UpAxis::Y,
            subject_radius: 1.0,
        }
    }
}

impl OrbitCamera {
    /// A three-quarter view from the front left and a little above.
    pub const DEFAULT_YAW: f32 = -0.6;
    pub const DEFAULT_PITCH: f32 = 0.42;
    const MAX_PITCH: f32 = 89.0 * std::f32::consts::PI / 180.0;

    /// A first-person camera: the eye at `eye`, looking along `yaw` (radians about the up axis; 0
    /// looks along -Z for Y-up, positive turns left) and `pitch` (radians, positive looks up),
    /// drawing everything farther than `near`. Move it by
    /// building a new one each frame; [`Self::orbit`] turns it in place.
    pub fn first_person(eye: Vec3, yaw: f32, pitch: f32, near: f32, up: UpAxis) -> Self {
        let near = near.max(1e-4);
        // The orbit target sits just ahead of the eye; the subject radius sets the near plane.
        let mut camera = Self {
            target: [0.0; 3],
            yaw,
            // An orbit camera's pitch raises the eye above the target, which looks down.
            pitch: (-pitch).clamp(-Self::MAX_PITCH, Self::MAX_PITCH),
            distance: near * 2.0,
            up,
            subject_radius: near * 0.5,
            ..Self::default()
        };
        let back = camera.basis().forward;
        camera.target = add(eye, scale(back, camera.distance));
        camera
    }

    /// A camera that frames `bounds` from the default angle.
    pub fn framing(bounds: Bounds, up: UpAxis) -> Self {
        let mut camera = Self {
            up,
            ..Self::default()
        };
        camera.frame(bounds);
        camera
    }

    /// A camera that frames `mesh` from the default angle, using its tight bounding sphere.
    pub fn framing_mesh(mesh: &Mesh, up: UpAxis) -> Option<Self> {
        let (center, radius) = mesh.bounding_sphere()?;
        let mut camera = Self {
            up,
            ..Self::default()
        };
        camera.frame_sphere(center, radius);
        Some(camera)
    }

    /// Points at `bounds` and backs off until it fits the view, keeping the angle.
    pub fn frame(&mut self, bounds: Bounds) {
        self.frame_sphere(bounds.center(), bounds.radius());
    }

    /// Points at a sphere and backs off until it fits the vertical field of view, with a margin.
    pub fn frame_sphere(&mut self, center: Vec3, radius: f32) {
        let radius = radius.max(1e-6);
        self.target = center;
        self.subject_radius = radius;
        self.distance = radius / (self.fov_y * 0.5).sin() * 1.05;
    }

    /// Turns the camera around the target.
    pub fn orbit(&mut self, delta_yaw: f32, delta_pitch: f32) {
        self.yaw = (self.yaw + delta_yaw) % std::f32::consts::TAU;
        self.pitch = (self.pitch + delta_pitch).clamp(-Self::MAX_PITCH, Self::MAX_PITCH);
    }

    /// Moves closer (`factor` < 1) or farther (`factor` > 1), within sensible bounds of the subject.
    pub fn zoom(&mut self, factor: f32) {
        if factor.is_finite() && factor > 0.0 {
            self.distance = (self.distance * factor)
                .clamp(self.subject_radius * 0.05, self.subject_radius * 100.0);
        }
    }

    /// Slides the target across the view by fractions of the distance (right and up).
    pub fn pan(&mut self, right: f32, up: f32) {
        let basis = self.basis();
        let shift = add(
            scale(basis.right, right * self.distance),
            scale(basis.up, up * self.distance),
        );
        self.target = add(self.target, shift);
    }

    pub fn basis(&self) -> ViewBasis {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        // Direction from the target to the eye, in the Y-up frame.
        let back = self
            .up
            .to_world([cos_pitch * sin_yaw, sin_pitch, cos_pitch * cos_yaw]);
        let eye = add(self.target, scale(back, self.distance));
        let forward = scale(back, -1.0);
        let right = normalize_or(
            cross(forward, self.up.vector()),
            self.up.to_world([1.0, 0.0, 0.0]),
        );
        let up = cross(right, forward);
        ViewBasis {
            eye,
            right,
            up,
            forward,
        }
    }

    /// Nearest distance drawn; triangles in front of it are clipped.
    pub fn near(&self) -> f32 {
        (self.distance - self.subject_radius * 2.0)
            .max(self.distance * 0.002)
            .max(1e-5)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3) -> bool {
        a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-5)
    }

    #[test]
    fn first_person_cameras_sit_at_the_eye() {
        let camera = OrbitCamera::first_person([1.0, 2.0, 3.0], 0.0, 0.0, 0.05, UpAxis::Y);
        let basis = camera.basis();
        assert!(close(basis.eye, [1.0, 2.0, 3.0]));
        assert!(close(basis.forward, [0.0, 0.0, -1.0]));
        assert!((camera.near() - 0.05).abs() < 1e-6);
        let turned =
            OrbitCamera::first_person([0.0; 3], std::f32::consts::FRAC_PI_2, 0.0, 0.05, UpAxis::Y);
        assert!(close(turned.basis().forward, [-1.0, 0.0, 0.0]));
        let up = OrbitCamera::first_person([0.0; 3], 0.0, 0.5, 0.05, UpAxis::Y);
        assert!(up.basis().forward[1] > 0.4, "positive pitch looks up");
    }

    #[test]
    fn basis_is_orthonormal_and_looks_at_the_target() {
        for up in [UpAxis::Y, UpAxis::Z] {
            for (yaw, pitch) in [(0.0, 0.0), (1.0, 0.5), (-2.0, -1.2), (3.0, 1.5)] {
                let camera = OrbitCamera {
                    yaw,
                    pitch,
                    up,
                    target: [1.0, 2.0, 3.0],
                    ..OrbitCamera::default()
                };
                let basis = camera.basis();
                for axis in [basis.right, basis.up, basis.forward] {
                    assert!((dot(axis, axis) - 1.0).abs() < 1e-5);
                }
                assert!(dot(basis.right, basis.up).abs() < 1e-5);
                assert!(dot(basis.up, basis.forward).abs() < 1e-5);
                let target = basis.to_view(camera.target);
                assert!(close(target, [0.0, 0.0, camera.distance]));
                // The world's up axis never points down on screen.
                assert!(basis.direction_to_view(up.vector())[1] >= 0.0);
            }
        }
    }

    #[test]
    fn framing_fits_the_bounds_and_zoom_is_bounded() {
        let bounds = Bounds {
            min: [-1.0, -1.0, -1.0],
            max: [1.0, 1.0, 1.0],
        };
        let mut camera = OrbitCamera::framing(bounds, UpAxis::Y);
        let radius = bounds.radius();
        assert!(camera.distance * (camera.fov_y * 0.5).sin() > radius);
        camera.zoom(1e-9);
        assert!((camera.distance - radius * 0.05).abs() < 1e-5);
        camera.zoom(1e9);
        assert!((camera.distance - radius * 100.0).abs() < 1e-3);
        camera.orbit(0.0, 10.0);
        assert!(camera.pitch < 1.56);
        assert!(camera.near() > 0.0);
    }
}
