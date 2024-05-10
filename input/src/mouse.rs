use std::cell::{Cell, RefCell};

use bitflags::bitflags;
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

use crate::display::Display;

pub type InjectInputFn = extern "C" fn(*const SyntheticInfo, count: i32);

bitflags! {
    #[repr(C)]
    pub struct SyntheticOptions: u32 {
        const NONE = 0;
        /// Move (coalesce move messages). If a mouse event occurs and the application has not yet processed the
        /// previous mouse event, the previous one is thrown away.
        const MOVE = 1;
        /// Left mouse button pressed.
        const LEFT_DOWN = 2;
        /// Left mouse button released.
        const LEFT_UP = 4;
        /// Right mouse button pressed.
        const RIGHT_DOWN = 8;
        /// Right mouse button released.
        const RIGHT_UP = 16;
        /// Middle mouse button pressed.
        const MIDDLE_DOWN = 32;
        /// Middle mouse button released.
        const MIDDLE_UP = 64;
        /// XBUTTON pressed.
        const XDOWN = 128;
        /// XBUTTON released.
        const XUP = 256;
        /// Mouse wheel.
        const WHEEL = 2048;
        /// Mouse tilt wheel.
        const HWHEEL = 4096;
        /// Move (do not coalesce move messages). The application processes all mouse events since the previously
        /// processed mouse event.
        const MOVE_NO_COALESCE = 8192;
        /// Map coordinates to the entire virtual desktop.
        const VIRTUAL_DESK = 16384;
        /// Normalized absolute coordinates between 0 and 65,535. If the flag is not set, relative data (the change
        /// in position since the last reported position) is used.
        /// Coordinate (0,0) maps onto the upper-left corner of the display surface; coordinate (65535,65535) maps
        /// onto the lower-right corner. In a multi-monitor system, the coordinates map to the primary monitor.
        const ABSOLUTE = 32768;
    }
}

pub enum MouseButton {
    Left,
    Right,
    Middle,
    X1,
    X2,
}

pub enum ButtonAction {
    Down,
    Up,
}

pub enum ScrollAxis {
    Vertical,
    Horizontal,
}

#[repr(C)]
pub struct SyntheticInfo {
    pub delta_x: i32,
    pub delta_y: i32,
    pub mouse_data: i32,
    pub options: SyntheticOptions,
    pub time_offset: u32,
    pub extra_info: *const std::ffi::c_void,
}

pub struct Mouse {
    inject_mouse_input: InjectInputFn,
    mouse_error: RefCell<(f32, f32)>,
}

impl Mouse {
    fn position_absolute(&self, display: &Display) -> (i32, i32) {
        let mut point = POINT { x: 0, y: 0 };
        unsafe {
            GetCursorPos(&mut point).unwrap();
        }

        (point.x - display.bounds.left, point.y - display.bounds.top)
    }

    fn position_relative(&self) -> (i32, i32) {
        let mut point = POINT { x: 0, y: 0 };
        unsafe {
            GetCursorPos(&mut point).unwrap();
        }

        (point.x, point.y)
    }

    fn as_windows_mouse(&self) -> &self::Mouse {
        self
    }

    fn move_relative(&self, display: &Display, pos: (i32, i32)) {
        let scale = display.mouse_correction_factor;
        let scaled = (pos.0 as f32 * scale.0, pos.1 as f32 * scale.1);

        let (mut whole_x, mut whole_y) = (scaled.0.trunc(), scaled.1.trunc());
        let (remainder_x, remainder_y) = (scaled.0 - whole_x, scaled.1 - whole_y);

        // We can only move mouse by whole values, but we want to keep track of fractional values
        // so do that here
        self.mouse_error.replace_with(|(error_x, error_y)| {
            *error_x += remainder_x;
            *error_y += remainder_y;

            let f = |error: &mut f32, whole: &mut f32| {
                let sign = error.signum();
                *whole += 1.0 * sign;
                *error -= 1.0 * sign;
            };

            f(error_x, &mut whole_x);
            f(error_y, &mut whole_y);

            (*error_x, *error_y)
        });

        let infos = [SyntheticInfo {
            delta_x: whole_x as i32,
            delta_y: whole_y as i32,
            mouse_data: 0,
            options: SyntheticOptions::MOVE_NO_COALESCE,
            time_offset: 0,
            extra_info: std::ptr::null(),
        }];

        (self.inject_mouse_input)(infos.as_ptr(), 1);
    }

    fn move_absolute(&self, display: &Display, pos: (u32, u32)) {
        let bounds = display.bounds;
        let desktop_bounds = Display::desktop_bounds();
        let desktop_size = desktop_bounds.size();

        let abs_point = (
            pos.0 as i32 + bounds.left - desktop_bounds.left,
            pos.1 as i32 + bounds.top - desktop_bounds.top,
        );

        // Scale point across the entire virtual desktop (0 - 65535)
        let virtual_point = (
            ((abs_point.0 as f32 * (65535.0 / desktop_size.0 as f32)).round() + 1.0) as i32,
            ((abs_point.1 as f32 * (65535.0 / desktop_size.1 as f32)).round() + 1.0) as i32,
        );

        let infos = [SyntheticInfo {
            delta_x: virtual_point.0 as i32,
            delta_y: virtual_point.1 as i32,
            mouse_data: 0,
            options: SyntheticOptions::MOVE_NO_COALESCE
                | SyntheticOptions::ABSOLUTE
                | SyntheticOptions::VIRTUAL_DESK,
            time_offset: 0,
            extra_info: std::ptr::null(),
        }];

        (self.inject_mouse_input)(infos.as_ptr(), 1);
    }

    fn action(&self, button: MouseButton, action: ButtonAction) {
        let (button, mouse_data) = if let ButtonAction::Down = action {
            match button {
                MouseButton::Left => (SyntheticOptions::LEFT_DOWN, 0),
                MouseButton::Right => (SyntheticOptions::RIGHT_DOWN, 0),
                MouseButton::Middle => (SyntheticOptions::MIDDLE_DOWN, 0),
                MouseButton::X1 => (SyntheticOptions::XDOWN, 1),
                MouseButton::X2 => (SyntheticOptions::XDOWN, 2),
            }
        } else {
            match button {
                MouseButton::Left => (SyntheticOptions::LEFT_UP, 0),
                MouseButton::Right => (SyntheticOptions::RIGHT_UP, 0),
                MouseButton::Middle => (SyntheticOptions::MIDDLE_UP, 0),
                MouseButton::X1 => (SyntheticOptions::XUP, 1),
                MouseButton::X2 => (SyntheticOptions::XUP, 2),
            }
        };

        let infos = [SyntheticInfo {
            delta_x: 0,
            delta_y: 0,
            mouse_data,
            options: button,
            time_offset: 0,
            extra_info: std::ptr::null(),
        }];

        (self.inject_mouse_input)(infos.as_ptr(), 1);
    }

    fn scroll(&self, amount: i32, direction: ScrollAxis) {
        const MOUSE_WHEEL_CLICK_SIZE: i32 = -120;
        let infos = [SyntheticInfo {
            delta_x: 0,
            delta_y: 0,
            mouse_data: amount * MOUSE_WHEEL_CLICK_SIZE,
            options: if let ScrollAxis::Vertical = direction {
                SyntheticOptions::WHEEL
            } else {
                SyntheticOptions::HWHEEL
            },
            time_offset: 0,
            extra_info: std::ptr::null(),
        }];

        (self.inject_mouse_input)(infos.as_ptr(), 1);
    }
}
