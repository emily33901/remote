use std::time::Duration;
use anyhow::Result;
use tokio::sync::mpsc;
use windows::Win32::{
    Foundation::{GetLastError, HWND, LPARAM, LRESULT, WPARAM},
    System::LibraryLoader::GetModuleHandleA,
    UI::WindowsAndMessaging::{
        CreateWindowExA, DefWindowProcA, DispatchMessageA, LoadCursorW, PeekMessageA,
        PostQuitMessage, RegisterClassA, TranslateMessage, CS_HREDRAW, CS_VREDRAW,
        CW_USEDEFAULT, IDC_ARROW, MSG, PM_REMOVE, WINDOW_EX_STYLE, WM_DESTROY,
        WNDCLASSA, WS_MINIMIZEBOX, WS_OVERLAPPEDWINDOW, WS_SYSMENU, WS_VISIBLE,
    },
};

use media::pipeline::{RecvPipeline, RecvPipelineConfig, RecvControl, EncodedData};
use media::{Encoding, VideoBuffer};

const ARBITRARY_CHANNEL_LIMIT: usize = 10;

pub struct D3D11PresenterWindow;

impl D3D11PresenterWindow {
    pub fn spawn(width: u32, height: u32, title: &str) -> Result<mpsc::Sender<VideoBuffer>> {
        let (data_tx, data_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);
        let title = title.to_string();

        std::thread::spawn(move || {
            if let Err(e) = Self::run_window(width, height, &title, data_rx) {
                tracing::error!("D3D11 presenter window error: {}", e);
            }
        });

        Ok(data_tx)
    }

    fn run_window(
        width: u32,
        height: u32,
        title: &str,
        mut data_rx: mpsc::Receiver<VideoBuffer>,
    ) -> Result<()> {
        let hwnd = Self::create_window(width, height, title)?;

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;

        let pipeline = rt.block_on(async {
            let config = RecvPipelineConfig {
                hwnd: hwnd.0 as isize,
                width,
                height,
                encoding: Encoding::H264,
            };
            RecvPipeline::new(config).await
        })?;

        tracing::info!("D3D11 presenter window started");

        loop {
            unsafe {
                let mut message = MSG::default();
                while PeekMessageA(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                    if message.message == WM_DESTROY {
                        tracing::info!("D3D11 presenter window closing");
                        let _ = rt.block_on(pipeline.send(RecvControl::Stop));
                        return Ok(());
                    }
                    let _ = TranslateMessage(&message);
                    DispatchMessageA(&message);
                }
            }

            match data_rx.try_recv() {
                Ok(video_buffer) => {
                    let encoded = EncodedData {
                        buffer: video_buffer,
                    };
                    if let Err(e) = rt.block_on(pipeline.send(RecvControl::Data(encoded))) {
                        tracing::warn!("Failed to send to pipeline: {}", e);
                    }
                }
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    tracing::info!("Data channel disconnected, closing window");
                    let _ = rt.block_on(pipeline.send(RecvControl::Stop));
                    break;
                }
                Err(mpsc::error::TryRecvError::Empty) => {}
            }

            std::thread::sleep(Duration::from_millis(1));
        }

        Ok(())
    }

    fn create_window(width: u32, height: u32, title: &str) -> Result<HWND> {
        unsafe {
            let instance = GetModuleHandleA(None)?;
            debug_assert!(!instance.is_invalid());

            let class_name = windows::core::s!("d3d11-presenter-window");

            let wc = WNDCLASSA {
                hCursor: LoadCursorW(None, IDC_ARROW)?,
                hInstance: instance.into(),
                lpszClassName: class_name,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(Self::wndproc),
                ..Default::default()
            };

            let atom = RegisterClassA(&wc);
            if atom == 0 {
                let last_error = GetLastError();
                if last_error.0 != 1410 {
                    return Err(anyhow::anyhow!("RegisterClassA failed: {}", last_error.0));
                }
            }

            let title_bytes: Vec<u8> = title.bytes().chain(std::iter::once(0)).collect();
            let title_pcstr = windows::core::PCSTR(title_bytes.as_ptr());

            let window_handle = CreateWindowExA(
                WINDOW_EX_STYLE::default(),
                class_name,
                title_pcstr,
                WS_OVERLAPPEDWINDOW | WS_VISIBLE | WS_SYSMENU | WS_MINIMIZEBOX,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                width as i32,
                height as i32,
                None,
                None,
                Some(instance.into()),
                None,
            )?;

            Ok(window_handle)
        }
    }

    extern "system" fn wndproc(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        unsafe {
            match message {
                WM_DESTROY => {
                    PostQuitMessage(0);
                    LRESULT(0)
                }
                _ => DefWindowProcA(window, message, wparam, lparam),
            }
        }
    }
}

pub fn create_d3d11_window(
    width: u32,
    height: u32,
    title: &str,
) -> Result<mpsc::Sender<VideoBuffer>> {
    D3D11PresenterWindow::spawn(width, height, title)
}
