mod app;
mod color;
mod peer;
mod window;

use crate::config::Config;

use anyhow::Result;
use media::dx::create_device_and_swapchain;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM_SRGB;
use winit::dpi::PhysicalSize;
use winit::event::{Event, WindowEvent};
use winit::event_loop::EventLoopBuilder;
use winit::window::Window;
use winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawWindowHandle};

use self::app::App;
use self::window::{create_render_target_for_swap_chain, resize_swap_chain_and_render_target};

pub async fn ui() -> Result<()> {
    let _system = crate::windows::System::new()?;

    let mut app = App::default();
    let config = Config::load();

    let (width, height) = (config.width, config.height);

    let event_loop = EventLoopBuilder::new().build()?;
    let window_attributes = Window::default_attributes()
        .with_title("remote")
        .with_inner_size(PhysicalSize::new(width, height));
    let window = event_loop.create_window(window_attributes)?;

    let window_handle = if let RawWindowHandle::Win32(raw) = window.window_handle()?.as_raw() {
        HWND(raw.hwnd.get() as *mut _)
    } else {
        panic!("unexpected RawWindowHandle variant");
    };

    let (device, context, swap_chain) = create_device_and_swapchain(window_handle, width, height)?;

    let mut render_target = Some(create_render_target_for_swap_chain(&device, &swap_chain)?);

    let egui_ctx = egui::Context::default();
    let mut egui_renderer = egui_directx11::Renderer::new(&device)?;
    let mut egui_winit = egui_winit::State::new(
        egui_ctx.clone(),
        egui_ctx.viewport_id(),
        &window.display_handle()?,
        None,
        None,
        None,
    );

    let mut egui_demo = egui_demo_lib::DemoWindows::default();

    event_loop.run(move |event, _control_flow| match event {
        Event::AboutToWait => window.request_redraw(),
        Event::WindowEvent { window_id, event } => {
            if window_id != window.id() {
                return;
            }

            if egui_winit.on_window_event(&window, &event).consumed {
                return;
            }

            match event {
                WindowEvent::CloseRequested => {
                    std::process::exit(0);
                }
                WindowEvent::Resized(PhysicalSize {
                    width: new_width,
                    height: new_height,
                }) => {
                    if let Err(err) = resize_swap_chain_and_render_target(
                        &device,
                        &swap_chain,
                        &mut render_target,
                        new_width,
                        new_height,
                        DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
                    ) {
                        panic!("fail to resize framebuffers: {err:?}");
                    }
                }
                WindowEvent::RedrawRequested => {
                    if let Some(render_target) = &render_target {
                        let egui_input = egui_winit.take_egui_input(&window);
                        let egui_output = egui_ctx.run(egui_input, |ctx| {
                            app.ui(ctx);
                            egui_demo.ui(ctx);
                        });
                        let (renderer_output, platform_output, _) =
                            egui_directx11::split_output(egui_output);
                        egui_winit.handle_platform_output(&window, platform_output);

                        unsafe {
                            context.ClearRenderTargetView(render_target, &[0.0, 0.0, 0.0, 1.0]);
                        }
                        let _ = egui_renderer.render(
                            &context,
                            render_target,
                            &egui_ctx,
                            renderer_output,
                        );
                        let _ = unsafe { swap_chain.Present(1, windows::Win32::Graphics::Dxgi::DXGI_PRESENT(0)) };
                    } else {
                        unreachable!();
                    }
                }
                _ => (),
            }
        }
        _ => (),
    })?;

    Ok(())
}
