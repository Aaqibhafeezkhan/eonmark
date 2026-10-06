//! The RTS camera: perspective, pitch ~55 deg, yaw locked, zoom 15-60 m.
//!
//! Pan: WASD / arrows, edge scroll (12 px margin) and two-finger trackpad
//! scroll (`MouseScrollUnit::Pixel`). Zoom: wheel lines
//! (`MouseScrollUnit::Line`) and `PinchGesture`. Every speed is multiplied
//! by `Time::delta_secs` so travel is identical at 60 and 120 Hz; pixel
//! scroll deltas are displacements, not speeds, and are scaled by zoom.

use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::input::gestures::PinchGesture;
use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::light::CascadeShadowConfigBuilder;
use bevy::prelude::*;
use bevy::render::view::Msaa;
use bevy::window::PrimaryWindow;

use crate::ground::HALF_EXTENT;
use crate::palette;

/// Camera pitch below the horizon, degrees.
pub const PITCH_DEG: f32 = 55.0;
/// Closest zoom (distance from focus), metres.
pub const ZOOM_MIN: f32 = 15.0;
/// Farthest zoom, metres.
pub const ZOOM_MAX: f32 = 60.0;
/// Starting zoom, metres.
pub const ZOOM_DEFAULT: f32 = 35.0;
/// Edge-scroll band, logical pixels.
pub const EDGE_SCROLL_MARGIN_PX: f32 = 12.0;

/// Keyboard and edge-scroll pan speed as a fraction of zoom distance per second.
const PAN_SPEED_PER_ZOOM: f32 = 0.7;
/// Trackpad pan: world metres per logical pixel, per metre of zoom, over the window height.
const TRACKPAD_PAN_GAIN: f32 = 1.6;
/// Wheel zoom: exponential step per scroll line.
const LINE_ZOOM_STEP: f32 = 0.1;
/// Pinch zoom: exponential gain per unit of `PinchGesture` delta.
const PINCH_ZOOM_GAIN: f32 = 1.0;
/// Zoom smoothing rate, 1/s (higher snaps faster).
const ZOOM_SMOOTHING: f32 = 12.0;

/// Spawns the camera and the sun, and runs the control systems.
pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_camera_and_sun).add_systems(
            Update,
            (
                pan_keyboard,
                pan_edge_scroll,
                scroll_pan_and_zoom,
                pinch_zoom,
                apply_camera,
            )
                .chain(),
        );
    }
}

/// State of the one RTS camera. The `Transform` is derived from this every frame.
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component)]
pub struct RtsCamera {
    /// Point on the ground plane the camera looks at.
    pub focus: Vec3,
    /// Current (smoothed) distance from focus, metres.
    pub zoom: f32,
    /// Target distance the smoothing moves toward, metres.
    pub zoom_target: f32,
}

impl Default for RtsCamera {
    fn default() -> Self {
        Self {
            focus: Vec3::ZERO,
            zoom: ZOOM_DEFAULT,
            zoom_target: ZOOM_DEFAULT,
        }
    }
}

impl RtsCamera {
    /// Camera position for the current focus and zoom.
    pub fn eye(&self) -> Vec3 {
        let pitch = PITCH_DEG.to_radians();
        self.focus + Vec3::new(0.0, self.zoom * pitch.sin(), self.zoom * pitch.cos())
    }

    fn pan(&mut self, delta: Vec3) {
        self.focus += delta;
        self.focus.x = self.focus.x.clamp(-HALF_EXTENT, HALF_EXTENT);
        self.focus.z = self.focus.z.clamp(-HALF_EXTENT, HALF_EXTENT);
        self.focus.y = 0.0;
    }

    fn zoom_by_factor(&mut self, factor: f32) {
        self.zoom_target = (self.zoom_target * factor).clamp(ZOOM_MIN, ZOOM_MAX);
    }
}

fn spawn_camera_and_sun(mut commands: Commands) {
    let rts = RtsCamera::default();
    let transform = Transform::from_translation(rts.eye()).looking_at(rts.focus, Vec3::Y);
    commands.spawn((
        Name::new("RTS camera"),
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection {
            fov: 40.0_f32.to_radians(),
            near: 0.5,
            far: 400.0,
            ..default()
        }),
        Tonemapping::TonyMcMapface,
        Msaa::Sample4,
        AmbientLight {
            color: palette::AMBIENT,
            brightness: 300.0,
            ..default()
        },
        transform,
        rts,
    ));

    commands.spawn((
        Name::new("Sun"),
        DirectionalLight {
            color: palette::SUN,
            illuminance: 7_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        CascadeShadowConfigBuilder {
            num_cascades: 2,
            minimum_distance: 1.0,
            maximum_distance: 160.0,
            first_cascade_far_bound: 45.0,
            overlap_proportion: 0.2,
        }
        .build(),
        // Light shines along its -Z: from high in the south-east.
        Transform::from_xyz(40.0, 70.0, 30.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

fn pan_keyboard(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut cameras: Query<&mut RtsCamera>,
) {
    let mut dir = Vec3::ZERO;
    if keys.any_pressed([KeyCode::KeyW, KeyCode::ArrowUp]) {
        dir.z -= 1.0;
    }
    if keys.any_pressed([KeyCode::KeyS, KeyCode::ArrowDown]) {
        dir.z += 1.0;
    }
    if keys.any_pressed([KeyCode::KeyA, KeyCode::ArrowLeft]) {
        dir.x -= 1.0;
    }
    if keys.any_pressed([KeyCode::KeyD, KeyCode::ArrowRight]) {
        dir.x += 1.0;
    }
    if dir == Vec3::ZERO {
        return;
    }
    let dir = dir.normalize();
    for mut cam in &mut cameras {
        let speed = cam.zoom * PAN_SPEED_PER_ZOOM;
        cam.pan(dir * speed * time.delta_secs());
    }
}

fn pan_edge_scroll(
    windows: Query<&Window, With<PrimaryWindow>>,
    time: Res<Time>,
    mut cameras: Query<&mut RtsCamera>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    if !window.focused {
        return;
    }
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    let size = window.size();
    let mut dir = Vec3::ZERO;
    if cursor.x <= EDGE_SCROLL_MARGIN_PX {
        dir.x -= 1.0;
    } else if cursor.x >= size.x - EDGE_SCROLL_MARGIN_PX {
        dir.x += 1.0;
    }
    if cursor.y <= EDGE_SCROLL_MARGIN_PX {
        dir.z -= 1.0;
    } else if cursor.y >= size.y - EDGE_SCROLL_MARGIN_PX {
        dir.z += 1.0;
    }
    if dir == Vec3::ZERO {
        return;
    }
    let dir = dir.normalize();
    for mut cam in &mut cameras {
        let speed = cam.zoom * PAN_SPEED_PER_ZOOM;
        cam.pan(dir * speed * time.delta_secs());
    }
}

/// Trackpad two-finger scroll arrives as `Pixel` deltas and pans; a wheel
/// arrives as `Line` deltas and zooms.
fn scroll_pan_and_zoom(
    mut wheel: MessageReader<MouseWheel>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut cameras: Query<&mut RtsCamera>,
) {
    let window_height = windows
        .single()
        .map(|w| w.height().max(1.0))
        .unwrap_or(800.0);
    for event in wheel.read() {
        for mut cam in &mut cameras {
            match event.unit {
                MouseScrollUnit::Pixel => {
                    // Drag the map with the fingers: content follows the gesture.
                    let metres_per_px = cam.zoom * TRACKPAD_PAN_GAIN / window_height;
                    cam.pan(Vec3::new(-event.x, 0.0, -event.y) * metres_per_px);
                }
                MouseScrollUnit::Line => {
                    cam.zoom_by_factor((-event.y * LINE_ZOOM_STEP).exp());
                }
            }
        }
    }
}

fn pinch_zoom(mut pinches: MessageReader<PinchGesture>, mut cameras: Query<&mut RtsCamera>) {
    for pinch in pinches.read() {
        for mut cam in &mut cameras {
            // Positive delta = fingers spreading = zoom in.
            cam.zoom_by_factor((-pinch.0 * PINCH_ZOOM_GAIN).exp());
        }
    }
}

fn apply_camera(time: Res<Time>, mut cameras: Query<(&mut RtsCamera, &mut Transform)>) {
    let blend = 1.0 - (-ZOOM_SMOOTHING * time.delta_secs()).exp();
    for (mut cam, mut transform) in &mut cameras {
        cam.zoom = cam.zoom.lerp(cam.zoom_target, blend);
        if (cam.zoom - cam.zoom_target).abs() < 1e-3 {
            cam.zoom = cam.zoom_target;
        }
        transform.translation = cam.eye();
        transform.look_at(cam.focus, Vec3::Y);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eye_sits_above_and_behind_focus_at_the_pitch() {
        let cam = RtsCamera::default();
        let eye = cam.eye();
        let dir = eye - cam.focus;
        assert!((dir.length() - ZOOM_DEFAULT).abs() < 1e-3);
        let pitch = (dir.y / dir.length()).asin().to_degrees();
        assert!((pitch - PITCH_DEG).abs() < 1e-3);
        assert!(dir.x.abs() < 1e-6, "yaw is locked");
        assert!(dir.z > 0.0);
    }

    #[test]
    fn zoom_is_clamped() {
        let mut cam = RtsCamera::default();
        cam.zoom_by_factor(100.0);
        assert_eq!(cam.zoom_target, ZOOM_MAX);
        cam.zoom_by_factor(0.0001);
        assert_eq!(cam.zoom_target, ZOOM_MIN);
    }

    #[test]
    fn focus_is_clamped_to_the_ground() {
        let mut cam = RtsCamera::default();
        cam.pan(Vec3::new(1000.0, 5.0, -1000.0));
        assert_eq!(cam.focus, Vec3::new(HALF_EXTENT, 0.0, -HALF_EXTENT));
    }
}
