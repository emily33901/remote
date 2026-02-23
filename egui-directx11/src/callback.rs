//! Paint callback support for egui-directx11
//!
//! This module provides support for custom paint callbacks, allowing users to render
//! custom content (e.g., 3D graphics, video textures) within egui regions.

use std::sync::Arc;

pub use egui::epaint::PaintCallbackInfo;
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext};

/// Trait for callback functions that can render custom content
///
/// Implementors receive:
/// - `info`: Viewport and clipping information
/// - `device`: The Direct3D11 device
/// - `context`: The device context for rendering
///
/// The callback is responsible for:
/// - Setting up the viewport (use `info.viewport_in_pixels()`)
/// - Setting up the scissor rect (use `info.clip_rect_in_pixels()`)
/// - Restoring any pipeline state it modifies
pub trait CallbackFn: Send + Sync {
    fn call(
        &self,
        info: PaintCallbackInfo,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
    );
}

/// Internal wrapper for closure-based callbacks
struct CallbackFnImpl<F> {
    f: F,
}

impl<F> CallbackFn for CallbackFnImpl<F>
where
    F: Fn(PaintCallbackInfo, &ID3D11Device, &ID3D11DeviceContext) + Send + Sync,
{
    fn call(
        &self,
        info: PaintCallbackInfo,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
    ) {
        (self.f)(info, device, context);
    }
}

/// Create a new callback from a closure
///
/// # Example
/// ```ignore
/// let callback = egui_directx11::CallbackFn::new(|info, device, context| {
///     // Custom rendering here
/// });
///
/// let paint_callback = egui::PaintCallback {
///     rect: rect,
///     callback: std::sync::Arc::new(callback),
/// };
/// ```
pub fn callback_fn<F>(f: F) -> Arc<dyn CallbackFn>
where
    F: Fn(PaintCallbackInfo, &ID3D11Device, &ID3D11DeviceContext)
        + Send
        + Sync
        + 'static,
{
    Arc::new(CallbackFnImpl { f })
}
