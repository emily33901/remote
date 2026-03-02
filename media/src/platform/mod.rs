#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "windows")]
pub use self::windows::WindowsCapture as PlatformCapture;

#[cfg(target_os = "linux")]
pub use self::linux::LinuxCapture as PlatformCapture;

#[cfg(target_os = "macos")]
pub use self::macos::MacOSCapture as PlatformCapture;

use crate::traits::Capture;

#[cfg(target_os = "windows")]
pub fn create_capture() -> impl Capture {
    windows::WindowsCapture::new()
}

#[cfg(target_os = "linux")]
pub fn create_capture() -> impl Capture {
    linux::LinuxCapture::new()
}

#[cfg(target_os = "macos")]
pub fn create_capture() -> impl Capture {
    macos::MacOSCapture::new()
}
