use eyre::eyre;

use windows::Win32::{
    Foundation::GetLastError, Foundation::BOOL, Foundation::POINT, Graphics::Gdi, UI::HiDpi::*,
    UI::WindowsAndMessaging::*,
};

use eyre::Result;

#[derive(Default, Clone, Copy, Debug)]
pub struct Bounds {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Bounds {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }

    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }

    pub fn size(&self) -> (i32, i32) {
        (self.width(), self.height())
    }

    pub fn size_u32(&self) -> (u32, u32) {
        (self.width() as u32, self.height() as u32)
    }
}

#[derive(Clone, Copy)]
pub struct Display {
    pub mouse_correction_factor: (f32, f32),
    pub bounds: Bounds,
    pub handle: Gdi::HMONITOR,
}

const SPEED_CORRECTION_TABLE: [f32; 20] = [
    1.0 / 32.0,
    1.0 / 16.0,
    1.0 / 8.0,
    2.0 / 8.0,
    3.0 / 8.0,
    4.0 / 8.0,
    5.0 / 8.0,
    6.0 / 8.0,
    7.0 / 8.0,
    1.0,
    1.25,
    1.5,
    1.75,
    2.0,
    2.25,
    2.5,
    2.75,
    3.0,
    3.25,
    3.50,
];

impl Display {
    pub fn new(handle: Gdi::HMONITOR) -> Result<Display> {
        let mut monitor_info_ex = Gdi::MONITORINFOEXA::default();
        monitor_info_ex.monitorInfo.cbSize = std::mem::size_of::<Gdi::MONITORINFOEXA>() as u32;

        if unsafe {
            !BOOL::as_bool(Gdi::GetMonitorInfoA(
                handle,
                &mut monitor_info_ex.monitorInfo,
            ))
        } {
            return Err(eyre!("Failed to get monitor info {:?}", unsafe {
                GetLastError()
            }));
        }

        let monitor_info = monitor_info_ex.monitorInfo;

        let mut dpi_x: u32 = 0;
        let mut dpi_y: u32 = 0;

        if let Err(err) =
            unsafe { GetDpiForMonitor(handle, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) }
        {
            return Err(eyre!("Unable to get DPI for monitor {}", err));
        }

        let mut mouse_correction_factor_x = dpi_x as f32 / 96.0;
        let mut mouse_correction_factor_y = dpi_y as f32 / 96.0;

        let mut mouse_speed_index: u32 = 0;
        unsafe {
            SystemParametersInfoA(
                SPI_GETMOUSESPEED,
                0,
                Some(&mut mouse_speed_index as *mut _ as *mut std::ffi::c_void),
                SPIF_SENDCHANGE,
            )?;
        }
        mouse_speed_index -= 1;

        assert!(mouse_speed_index < 20);

        mouse_correction_factor_x *= 1.0 / SPEED_CORRECTION_TABLE[mouse_speed_index as usize];
        mouse_correction_factor_y *= 1.0 / SPEED_CORRECTION_TABLE[mouse_speed_index as usize];

        let display = Display {
            mouse_correction_factor: (mouse_correction_factor_x, mouse_correction_factor_y),
            bounds: Bounds {
                left: monitor_info.rcMonitor.left,
                top: monitor_info.rcMonitor.top,
                right: monitor_info.rcMonitor.right,
                bottom: monitor_info.rcMonitor.bottom,
            },
            handle,
        };

        Ok(display)
    }

    pub fn from_point(pos: (i32, i32)) -> eyre::Result<Display> {
        let handle = unsafe {
            Gdi::MonitorFromPoint(POINT { x: pos.0, y: pos.1 }, Gdi::MONITOR_DEFAULTTONULL)
        };

        if handle.is_invalid() {
            return Err(eyre!("Point is not on a monitor"));
        }

        Display::new(handle)
    }

    pub fn desktop_bounds() -> Bounds {
        unsafe {
            Bounds {
                left: GetSystemMetrics(SM_XVIRTUALSCREEN),
                top: GetSystemMetrics(SM_YVIRTUALSCREEN),
                right: GetSystemMetrics(SM_XVIRTUALSCREEN) + GetSystemMetrics(SM_CXVIRTUALSCREEN),
                bottom: GetSystemMetrics(SM_YVIRTUALSCREEN) + GetSystemMetrics(SM_CYVIRTUALSCREEN),
            }
        }
    }

    pub fn update(&mut self) -> eyre::Result<()> {
        *self = Display::new(self.handle)?;
        Ok(())
    }
}
