//! Converts gilrs gamepads into the WPILib Xbox controller layout.

use ds::JoystickValue;
use gilrs::{Axis, Button, Gamepad};

/// Gilrs buttons in WPILib order; button `n` is at index `n - 1`.
///
/// X and Y keep Conductor 0.1's North/West assignment, which was verified on real controllers:
/// the Linux kernel reports the Xbox X button as BTN_X (an alias of BTN_NORTH).
const BUTTONS: [Button; 10] = [
    Button::South,        // 1: A
    Button::East,         // 2: B
    Button::North,        // 3: X
    Button::West,         // 4: Y
    Button::LeftTrigger,  // 5: left bumper
    Button::RightTrigger, // 6: right bumper
    Button::Select,       // 7: back
    Button::Start,        // 8: start
    Button::LeftThumb,    // 9: left stick
    Button::RightThumb,   // 10: right stick
];

pub fn gamepad_values(gamepad: &Gamepad<'_>) -> Vec<JoystickValue> {
    // gilrs reports up as positive; WPILib expects forward to be negative
    let axes = [
        gamepad.value(Axis::LeftStickX),
        -gamepad.value(Axis::LeftStickY),
        trigger(gamepad, Button::LeftTrigger2, Axis::LeftZ),
        trigger(gamepad, Button::RightTrigger2, Axis::RightZ),
        gamepad.value(Axis::RightStickX),
        -gamepad.value(Axis::RightStickY),
    ];

    let mut values: Vec<_> = axes
        .into_iter()
        .enumerate()
        .map(|(id, value)| JoystickValue::Axis {
            id: id as u8,
            value,
        })
        .collect();
    values.extend(
        BUTTONS
            .iter()
            .enumerate()
            .map(|(i, &button)| JoystickValue::Button {
                id: i as u8 + 1,
                pressed: gamepad.is_pressed(button),
            }),
    );

    let dpad = |button, axis, sign: f32| {
        gamepad.is_pressed(button) || (gamepad.value(axis) * sign - 1.0).abs() < f32::EPSILON
    };
    values.push(JoystickValue::POV {
        id: 0,
        angle: pov_angle(
            dpad(Button::DPadUp, Axis::DPadY, 1.0),
            dpad(Button::DPadRight, Axis::DPadX, 1.0),
            dpad(Button::DPadDown, Axis::DPadY, -1.0),
            dpad(Button::DPadLeft, Axis::DPadX, -1.0),
        ),
    });
    values
}

/// Analog triggers are buttons with values on SDL-mapped controllers, and Z axes otherwise
fn trigger(gamepad: &Gamepad<'_>, button: Button, axis: Axis) -> f32 {
    let from_button = gamepad.button_data(button).map_or(0.0, |b| b.value());
    let from_axis = gamepad
        .axis_data(axis)
        .map_or(0.0, |a| (a.value() + 1.0) / 2.0);
    from_button.max(from_axis).clamp(0.0, 1.0)
}

/// The POV angle for a D-pad, in degrees clockwise from up, or -1 when released
pub fn pov_angle(up: bool, right: bool, down: bool, left: bool) -> i16 {
    match (up && !down, right && !left, down && !up, left && !right) {
        (true, false, _, false) => 0,
        (true, true, _, _) => 45,
        (false, true, false, _) => 90,
        (_, true, true, _) => 135,
        (_, false, true, false) => 180,
        (_, _, true, true) => 225,
        (false, _, false, true) => 270,
        (true, _, _, true) => 315,
        _ => -1,
    }
}

#[cfg(test)]
mod tests {
    use super::pov_angle;

    #[test]
    fn pov_angles() {
        assert_eq!(pov_angle(false, false, false, false), -1);
        assert_eq!(pov_angle(true, false, false, false), 0);
        assert_eq!(pov_angle(true, true, false, false), 45);
        assert_eq!(pov_angle(false, true, false, false), 90);
        assert_eq!(pov_angle(false, true, true, false), 135);
        assert_eq!(pov_angle(false, false, true, false), 180);
        assert_eq!(pov_angle(false, false, true, true), 225);
        assert_eq!(pov_angle(false, false, false, true), 270);
        assert_eq!(pov_angle(true, false, false, true), 315);
        // Opposing directions cancel out
        assert_eq!(pov_angle(true, false, true, false), -1);
    }
}
