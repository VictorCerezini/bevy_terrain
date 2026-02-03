#[cfg(feature = "high_precision")]
use big_space::{
    prelude::{CellTransform, FloatingOrigin, Grids},
    world_query::CellTransformItem,
};

use bevy::{
    app::AppExit,
    ecs::system::Commands, input::mouse::MouseMotion, math::DVec3, prelude::{
        ButtonInput, Camera3d, Component, Entity, EulerRot, KeyCode, MessageReader, Quat, Query, Reflect, Res, Time, Vec2, With, default, info
    }, window::{CursorGrabMode, CursorOptions, PrimaryWindow}
};

#[derive(Clone, Debug, Reflect, Component)]
#[require(Camera3d, FloatingOrigin = FloatingOrigin)]
pub struct DebugCameraController {
    pub enabled: bool,
    /// Smoothness of translation, from `0.0` to `1.0`.
    pub translational_smoothness: f64,
    /// Smoothness of rotation, from `0.0` to `1.0`.
    pub rotational_smoothness: f32,
    pub translation_speed: f64,
    pub rotation_speed: f32,
    pub acceleration_speed: f64,
    pub translation_velocity: DVec3,
    pub rotation_velocity: Vec2,
}

impl Default for DebugCameraController {
    fn default() -> Self {
        Self {
            enabled: false,
            translational_smoothness: 0.9,
            rotational_smoothness: 0.8,
            translation_speed: 10e1,
            rotation_speed: 1e-1,
            acceleration_speed: 4.0,
            translation_velocity: Default::default(),
            rotation_velocity: Default::default(),
        }
    }
}

impl DebugCameraController {
    pub fn new(speed: f64) -> Self {
        Self {
            translation_speed: speed,
            ..default()
        }
    }
}

pub fn debug_camera_controller(
    #[cfg(feature = "high_precision")] grids: Grids,
    mut commands: Commands,
    time: Res<Time>,
    keyboard: Res<ButtonInput<KeyCode>>,
    mut cursor_options: Query<&mut CursorOptions, With<PrimaryWindow>>,
    mut mouse_move: MessageReader<MouseMotion>,
    #[cfg(feature = "high_precision")] mut camera: Query<(
        Entity,
        CellTransform,
        &mut DebugCameraController,
    )>,
    #[cfg(not(feature = "high_precision"))] mut camera: Query<(
        &mut Transform,
        &mut DebugCameraController,
    )>,
) {
    #[cfg(feature = "high_precision")]
    let Ok((
        camera,
        CellTransformItem {
            mut transform,
            mut cell,
        },
        mut controller,
    )) = camera.single_mut()
    else {
        return;
    };
    #[cfg(feature = "high_precision")]
    let grid = grids.parent_grid(camera).unwrap();

    #[cfg(not(feature = "high_precision"))]
    let (mut transform, mut controller) = camera.single_mut();

    let mut cursor_options = cursor_options.single_mut().unwrap();

    if keyboard.just_pressed(KeyCode::KeyZ) {
        controller.enabled = !controller.enabled;
        cursor_options.grab_mode = match controller.enabled {
            true => CursorGrabMode::Confined,
            false => CursorGrabMode::None,
        };
        cursor_options.visible = !controller.enabled;

        info!(
            "Controller, {:?}",
            match controller.enabled {
                true => "Enabled",
                false => "Disabled",
            }
        );
    }

    if keyboard.just_pressed(KeyCode::Escape) {
        commands.write_message(AppExit::Success);
    }

    if !controller.enabled {
        return;
    }

    let mut translation_direction = DVec3::ZERO; // x: left/right, y: up/down, z: forward/backward
    let rotation_direction = mouse_move.read().map(|m| -m.delta).sum::<Vec2>(); // x: yaw, y: pitch, z: roll
    let mut acceleration = 0.0;

    if keyboard.pressed(KeyCode::ArrowLeft) || keyboard.pressed(KeyCode::KeyA) {
        translation_direction.x -= 1.0;
    }
    if keyboard.pressed(KeyCode::ArrowRight) || keyboard.pressed(KeyCode::KeyD) {
        translation_direction.x += 1.0;
    }
    if keyboard.pressed(KeyCode::PageUp) || keyboard.pressed(KeyCode::KeyE) {
        translation_direction.y += 1.0;
    }
    if keyboard.pressed(KeyCode::PageDown) || keyboard.pressed(KeyCode::KeyQ) {
        translation_direction.y -= 1.0;
    }
    if keyboard.pressed(KeyCode::ArrowUp) || keyboard.pressed(KeyCode::KeyW) {
        translation_direction.z -= 1.0;
    }
    if keyboard.pressed(KeyCode::ArrowDown) || keyboard.pressed(KeyCode::KeyS) {
        translation_direction.z += 1.0;
    }
    keyboard.pressed(KeyCode::Home).then(|| acceleration -= 1.0);
    keyboard.pressed(KeyCode::End).then(|| acceleration += 1.0);

    translation_direction = transform.rotation.as_dquat() * translation_direction;

    let dt = time.delta_secs_f64();
    let lerp_translation = 1.0 - controller.translational_smoothness.clamp(0.0, 0.999);
    let lerp_rotation = 1.0 - controller.rotational_smoothness.clamp(0.0, 0.999);

    let translation_velocity_target = translation_direction * controller.translation_speed * dt;
    let rotation_velocity_target = rotation_direction * controller.rotation_speed * dt as f32;

    controller.translation_velocity = controller
        .translation_velocity
        .lerp(translation_velocity_target, lerp_translation);
    controller.rotation_velocity = controller
        .rotation_velocity
        .lerp(rotation_velocity_target, lerp_rotation);
    controller.translation_speed *= 1.0 + acceleration * controller.acceleration_speed * dt;

    let (yaw, pitch, _) = transform.rotation.to_euler(EulerRot::YXZ);
    let new_yaw = (yaw + controller.rotation_velocity.x) % std::f32::consts::TAU;
    let new_pitch = (pitch + controller.rotation_velocity.y)
        .clamp(-std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_2);

    #[cfg(feature = "high_precision")]
    {
        let (cell_delta, translation_delta) =
            grid.translation_to_grid(controller.translation_velocity);

        *cell += cell_delta;
        transform.translation += translation_delta;
    }
    #[cfg(not(feature = "high_precision"))]
    {
        transform.translation += controller.translation_velocity.as_vec3();
    }

    transform.rotation = Quat::from_euler(EulerRot::YXZ, new_yaw, new_pitch, 0.0);
}
