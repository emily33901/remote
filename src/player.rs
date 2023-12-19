pub(crate) mod audio {
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::thread;

    use rodio::Source;
    use tokio::sync::mpsc;
    use tokio::sync::watch;
    use tokio::sync::MappedMutexGuard;
    use tokio::sync::Mutex;

    use eyre::Result;

    use crate::ARBITRARY_CHANNEL_LIMIT;

    #[derive(Debug)]
    pub(crate) struct SinkReader {
        store: Arc<Mutex<VecDeque<u8>>>,
    }

    impl SinkReader {
        fn new(store: Arc<Mutex<VecDeque<u8>>>) -> Self {
            Self { store: store }
        }
    }

    pub(crate) fn sink() -> Result<(mpsc::Sender<Vec<u8>>, SinkReader)> {
        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(10);

        let store = Arc::new(Mutex::new(VecDeque::<u8>::new()));

        tokio::spawn({
            let store = store.clone();
            async move {
                while let Some(buffer) = rx.recv().await {
                    store.lock().await.extend(buffer);
                }
            }
        });

        Ok((tx, SinkReader::new(store)))
    }

    const STORE_LOW_MARK: usize = 10000;

    impl Source for SinkReader {
        fn current_frame_len(&self) -> Option<usize> {
            None
        }

        fn channels(&self) -> u16 {
            2
        }

        fn sample_rate(&self) -> u32 {
            44100
        }

        fn total_duration(&self) -> Option<std::time::Duration> {
            None
        }
    }

    impl Iterator for SinkReader {
        type Item = i16;

        fn next(&mut self) -> Option<Self::Item> {
            if let Ok(mut store) = self.store.try_lock() {
                let mut bytes = [0_u8, 0_u8];
                if store.len() >= 2 {
                    bytes[0] = store.pop_front().unwrap();
                    bytes[1] = store.pop_front().unwrap();
                    let sample: i16 = unsafe { std::mem::transmute(bytes) };
                    Some(sample)
                } else {
                    Some(0)
                }
            } else {
                Some(0)
            }
        }
    }

    #[derive(Debug)]
    pub(crate) enum PlayerControl {
        Sink(SinkReader),
        Volume(f32),
        Skip(f32),
    }

    #[derive(Default, Clone, PartialEq)]
    pub(crate) struct PlayerState {
        pub file: PathBuf,
        pub pos: usize,
    }

    pub(crate) struct Player {
        handle: std::thread::JoinHandle<()>,
        pub control_tx: mpsc::Sender<PlayerControl>,
        pub state_rx: watch::Receiver<Option<PlayerState>>,
    }

    impl Player {
        pub(crate) fn new() -> Self {
            let (control_tx, control_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);
            let _loop_control_tx = control_tx.clone();
            let (state_tx, state_rx) = watch::channel(None);

            let handle = thread::spawn(move || {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(async move {
                        let mut inner = Inner::new(control_rx, state_tx);

                        inner.run().await;
                    });
            });

            Player {
                handle: handle,
                control_tx: control_tx,
                state_rx,
            }
        }
    }

    struct Inner {
        control_rx: mpsc::Receiver<PlayerControl>,
        state_tx: Arc<watch::Sender<Option<PlayerState>>>,

        sink_stream: Mutex<SinkStream>,
    }

    impl Inner {
        fn new(
            control_rx: mpsc::Receiver<PlayerControl>,
            state_tx: watch::Sender<Option<PlayerState>>,
        ) -> Self {
            Self {
                control_rx,
                state_tx: Arc::new(state_tx),
                sink_stream: Mutex::new(SinkStream::new()),
            }
        }

        async fn run(&mut self) {
            loop {
                tokio::select! {
                    Some(control) = self.control_rx.recv() => {
                        self.handle_control(control).await;
                    }

                    else => break,
                }
            }
        }

        async fn reset_sink(&self) {
            self.sink_stream.lock().await.reset();
        }

        async fn sink(&self) -> MappedMutexGuard<rodio::Sink> {
            tokio::sync::MutexGuard::map(self.sink_stream.lock().await, |s| &mut s.sink)
        }

        async fn handle_control(&mut self, control: PlayerControl) {
            match control {
                PlayerControl::Sink(reader) => {
                    self.reset_sink().await;
                    let source =
                        reader.periodic_access(std::time::Duration::from_millis(1000), |_r| {
                            // log::debug!("!!!! player periodic access");
                        });

                    let sink = self.sink().await;
                    sink.append(source);
                    log::debug!("!!! playing source");
                    sink.play();
                }
                PlayerControl::Volume(volume) => {
                    self.sink_stream.lock().await.set_volume(volume);
                }
                PlayerControl::Skip(_) => todo!(),
            }
        }
    }

    struct SinkStream {
        sink: rodio::Sink,
        stream: rodio::OutputStream,
        handle: rodio::OutputStreamHandle,
        volume: f32,
    }

    impl SinkStream {
        fn new() -> Self {
            let (new_stream, new_handle) = rodio::OutputStream::try_default().unwrap();
            let new_sink = rodio::Sink::try_new(&new_handle).unwrap();

            Self {
                sink: new_sink,
                stream: new_stream,
                handle: new_handle,
                volume: 1.0,
            }
        }

        fn reset(&mut self) {
            let (new_stream, new_handle) = rodio::OutputStream::try_default().unwrap();
            let new_sink = rodio::Sink::try_new(&new_handle).unwrap();
            new_sink.set_volume(self.volume);
            self.sink = new_sink;
            self.stream = new_stream;
            self.handle = new_handle;
        }

        fn set_volume(&mut self, volume: f32) {
            self.volume = volume;
            self.sink.set_volume(volume);
        }
    }
}

pub(crate) mod video {
    use std::mem::MaybeUninit;

    use eyre::{eyre, Result};
    use tokio::sync::{mpsc, mpsc::error::TryRecvError};

    use windows::{
        core::{s, ComInterface, PCSTR},
        Win32::{
            Foundation::{HWND, LPARAM, LRESULT, S_OK, TRUE, WPARAM},
            Graphics::{
                Direct3D::{Fxc::D3DCompile, *},
                Direct3D11::*,
                Dxgi::{
                    Common::{
                        DXGI_FORMAT, DXGI_FORMAT_NV12, DXGI_FORMAT_R32G32_FLOAT,
                        DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8_UNORM, DXGI_FORMAT_R8_UNORM,
                    },
                    IDXGIKeyedMutex, IDXGISwapChain, DXGI_SWAP_CHAIN_DESC,
                    DXGI_USAGE_RENDER_TARGET_OUTPUT,
                },
            },
            System::LibraryLoader::GetModuleHandleA,
            UI::WindowsAndMessaging::{
                CreateWindowExA, DefWindowProcA, DispatchMessageA, LoadCursorW, PeekMessageA,
                PostQuitMessage, RegisterClassExA, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT,
                IDC_ARROW, MSG, PM_REMOVE, WINDOW_EX_STYLE, WM_CREATE, WM_DESTROY, WNDCLASSEXA,
                WNDCLASS_STYLES, WS_MINIMIZEBOX, WS_OVERLAPPEDWINDOW, WS_SYSMENU, WS_VISIBLE,
            },
        },
    };

    use crate::{
        media::dx::{
            self, copy_texture, create_device_and_swapchain, create_texture, create_texture_sync,
        },
        ARBITRARY_CHANNEL_LIMIT,
    };

    fn compile_shader(data: &str, entry_point: PCSTR, target: PCSTR) -> Result<ID3DBlob> {
        unsafe {
            let mut blob: MaybeUninit<Option<ID3DBlob>> = MaybeUninit::uninit();
            D3DCompile(
                data.as_ptr() as *const std::ffi::c_void,
                data.len(),
                None,
                None,
                None,
                entry_point,
                target,
                0,
                0,
                &mut blob as *mut _ as *mut _,
                None,
            )?;
            Ok(unsafe { blob.assume_init().unwrap() })
        }
    }

    extern "system" fn wndproc(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        unsafe {
            match message {
                WM_CREATE => LRESULT(0),

                WM_DESTROY => {
                    PostQuitMessage(0);
                    LRESULT(0)
                }

                _ => DefWindowProcA(window, message, wparam, lparam),
            }
        }
    }

    fn create_window() -> Result<HWND> {
        unsafe {
            let instance = GetModuleHandleA(None)?;
            debug_assert!(instance.0 != 0);

            let window_class = s!("remote-player-window");

            let wc = WNDCLASSEXA {
                cbSize: std::mem::size_of::<WNDCLASSEXA>() as u32,
                hCursor: LoadCursorW(None, IDC_ARROW)?,
                hInstance: instance.into(),
                lpszClassName: window_class,

                style: WNDCLASS_STYLES(CS_HREDRAW.0 | CS_VREDRAW.0),
                lpfnWndProc: Some(wndproc),
                ..Default::default()
            };

            let atom = RegisterClassExA(&wc);
            debug_assert!(atom != 0);

            let window_handle = CreateWindowExA(
                WINDOW_EX_STYLE::default(),
                window_class,
                s!("remote-player-window"),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE | WS_SYSMENU | WS_MINIMIZEBOX,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                1920,
                1080,
                None,
                None,
                instance,
                None,
            );

            // ShowWindow(window_handle, SW_SHOW);

            Ok(window_handle)
        }
    }

    pub(crate) fn sink(width: u32, height: u32) -> Result<mpsc::Sender<ID3D11Texture2D>> {
        let (tx, mut rx) = mpsc::channel::<ID3D11Texture2D>(ARBITRARY_CHANNEL_LIMIT);

        tokio::spawn({
            let tx = tx.clone();

            async move {
                telemetry::client::watch_channel(&tx, "video sink").await;

                match tokio::task::spawn_blocking(move || -> eyre::Result<()> {
                    let (device, context, swap_chain) =
                        create_device_and_swapchain(create_window()?, width, height)?;

                    let back_buffer: ID3D11Texture2D = unsafe { swap_chain.GetBuffer(0) }?;
                    let mut render_target: Option<ID3D11RenderTargetView> = None;
                    unsafe {
                        device.CreateRenderTargetView(
                            &back_buffer,
                            None,
                            Some(&mut render_target as *mut _),
                        )
                    }?;

                    let texture =
                        dx::TextureBuilder::new(&device, width, height, dx::TextureFormat::NV12)
                            .bind_shader_resource()
                            .build()?;

                    unsafe { context.OMSetRenderTargets(Some(&[render_target.clone()]), None) };

                    let render_target = render_target.unwrap();

                    let viewport = D3D11_VIEWPORT {
                        TopLeftX: 0.0,
                        TopLeftY: 0.0,
                        Width: width as f32,
                        Height: height as f32,
                        MinDepth: 0.0,
                        MaxDepth: 1.0,
                    };

                    unsafe { context.RSSetViewports(Some(&[viewport])) };

                    let sample_desc = D3D11_SAMPLER_DESC {
                        Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                        AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                        AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                        AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                        ComparisonFunc: D3D11_COMPARISON_NEVER,
                        MinLOD: 0.0,
                        MaxLOD: f32::MAX,
                        ..Default::default()
                    };

                    let mut sampler_state: Option<ID3D11SamplerState> = None;
                    unsafe {
                        device.CreateSamplerState(&sample_desc, Some(&mut sampler_state as *mut _))
                    }?;

                    unsafe { context.PSSetSamplers(0, Some(&[sampler_state.clone()])) };

                    let vertex_shader_blob =
                        compile_shader(include_str!("shader.hlsl"), s!("vs_main"), s!("vs_5_0"))?;
                    let mut vertex_shader: Option<ID3D11VertexShader> = None;
                    unsafe {
                        let vertex_shader_blob_buffer = std::slice::from_raw_parts(
                            vertex_shader_blob.GetBufferPointer() as *const u8,
                            vertex_shader_blob.GetBufferSize(),
                        );
                        device.CreateVertexShader(
                            vertex_shader_blob_buffer,
                            None,
                            Some(&mut vertex_shader as *mut _),
                        )
                    }?;

                    let vertex_shader = vertex_shader.unwrap();

                    let pixel_shader_blob =
                        compile_shader(include_str!("shader.hlsl"), s!("ps_main"), s!("ps_5_0"))?;
                    let mut pixel_shader: Option<ID3D11PixelShader> = None;
                    unsafe {
                        let pixel_shader_blob_buffer = std::slice::from_raw_parts(
                            pixel_shader_blob.GetBufferPointer() as *const u8,
                            pixel_shader_blob.GetBufferSize(),
                        );
                        device.CreatePixelShader(
                            pixel_shader_blob_buffer,
                            None,
                            Some(&mut pixel_shader as *mut _),
                        )
                    }?;

                    let pixel_shader = pixel_shader.unwrap();

                    let pipeline_items = &[
                        D3D11_INPUT_ELEMENT_DESC {
                            SemanticName: s!("POS"),
                            SemanticIndex: 0,
                            Format: DXGI_FORMAT_R32G32_FLOAT,
                            InputSlot: 0,
                            AlignedByteOffset: 0,
                            InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                            InstanceDataStepRate: 0,
                        },
                        D3D11_INPUT_ELEMENT_DESC {
                            SemanticName: s!("TEX"),
                            SemanticIndex: 0,
                            Format: DXGI_FORMAT_R32G32_FLOAT,
                            InputSlot: 0,
                            AlignedByteOffset: D3D11_APPEND_ALIGNED_ELEMENT,
                            InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                            InstanceDataStepRate: 0,
                        },
                    ];

                    let mut input_layout: Option<ID3D11InputLayout> = None;
                    unsafe {
                        let vertex_shader_blob_buffer = std::slice::from_raw_parts(
                            vertex_shader_blob.GetBufferPointer() as *const u8,
                            vertex_shader_blob.GetBufferSize(),
                        );
                        device.CreateInputLayout(
                            pipeline_items,
                            vertex_shader_blob_buffer,
                            Some(&mut input_layout as *mut _),
                        )
                    }?;

                    let input_layout = input_layout.unwrap();

                    #[repr(C)]
                    struct Vertex {
                        x: f32,
                        y: f32,
                        u: f32,
                        v: f32,
                    }

                    let verticies = &[
                        Vertex {
                            x: -1.0,
                            y: 1.0,
                            u: 0.0,
                            v: 0.0,
                        },
                        Vertex {
                            x: 1.0,
                            y: -1.0,
                            u: 1.0,
                            v: 1.0,
                        },
                        Vertex {
                            x: -1.0,
                            y: -1.0,
                            u: 0.0,
                            v: 1.0,
                        },
                        Vertex {
                            x: -1.0,
                            y: 1.0,
                            u: 0.0,
                            v: 0.0,
                        },
                        Vertex {
                            x: 1.0,
                            y: 1.0,
                            u: 1.0,
                            v: 0.0,
                        },
                        Vertex {
                            x: 1.0,
                            y: -1.0,
                            u: 1.0,
                            v: 1.0,
                        },
                    ];

                    let topology = D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST;

                    let buffer_desc = D3D11_BUFFER_DESC {
                        Usage: D3D11_USAGE_DEFAULT,
                        ByteWidth: (verticies.len() * std::mem::size_of::<Vertex>()) as u32,
                        BindFlags: D3D11_BIND_VERTEX_BUFFER.0 as u32,
                        ..Default::default()
                    };

                    let subresource_data = D3D11_SUBRESOURCE_DATA {
                        pSysMem: verticies.as_ptr() as *const std::ffi::c_void,
                        ..Default::default()
                    };

                    let mut vertex_buffer: Option<ID3D11Buffer> = None;

                    unsafe {
                        device.CreateBuffer(
                            &buffer_desc as *const _,
                            Some(&subresource_data as *const _),
                            Some(&mut vertex_buffer as *mut _),
                        )
                    }?;

                    let mut texture_lum_view: Option<ID3D11ShaderResourceView> = None;

                    unsafe {
                        let mut texture_view_desc: D3D11_SHADER_RESOURCE_VIEW_DESC =
                            D3D11_SHADER_RESOURCE_VIEW_DESC {
                                Format: DXGI_FORMAT_R8_UNORM,
                                ViewDimension: D3D11_SRV_DIMENSION_TEXTURE2D,
                                ..Default::default()
                            };

                        texture_view_desc.Anonymous.Texture2D = D3D11_TEX2D_SRV {
                            MostDetailedMip: 0,
                            MipLevels: 1,
                        };

                        device.CreateShaderResourceView(
                            &texture,
                            Some(&texture_view_desc as *const _),
                            Some(&mut texture_lum_view as *mut _),
                        )
                    }?;

                    let mut texture_chrom_view: Option<ID3D11ShaderResourceView> = None;

                    unsafe {
                        let mut texture_view_desc: D3D11_SHADER_RESOURCE_VIEW_DESC =
                            D3D11_SHADER_RESOURCE_VIEW_DESC {
                                Format: DXGI_FORMAT_R8G8_UNORM,
                                ViewDimension: D3D11_SRV_DIMENSION_TEXTURE2D,
                                ..Default::default()
                            };

                        texture_view_desc.Anonymous.Texture2D = D3D11_TEX2D_SRV {
                            MostDetailedMip: 0,
                            MipLevels: 1,
                        };

                        device.CreateShaderResourceView(
                            &texture,
                            Some(&texture_view_desc as *const _),
                            Some(&mut texture_chrom_view as *mut _),
                        )
                    }?;

                    // let vertex_buffer = vertex_buffer.unwrap();
                    let texture_lum_view = texture_lum_view.unwrap();
                    let texture_chrom_view = texture_chrom_view.unwrap();

                    loop {
                        unsafe {
                            let mut message = MSG::default();
                            while PeekMessageA(&mut message, None, 0, 0, PM_REMOVE).into() {
                                DispatchMessageA(&message);
                            }
                        }

                        unsafe {
                            context.ClearRenderTargetView(&render_target, &[0.0, 0.0, 0.2, 1.0])
                        };

                        unsafe {
                            context.IASetInputLayout(&input_layout);
                            context.VSSetShader(&vertex_shader, None);
                            context.PSSetShader(&pixel_shader, None);
                        }

                        unsafe {
                            context.PSSetShaderResources(
                                0,
                                Some(&[
                                    Some(texture_lum_view.clone()),
                                    Some(texture_chrom_view.clone()),
                                ]),
                            );
                            context.PSSetSamplers(0, Some(&[sampler_state.clone()]));
                        }

                        unsafe {
                            let mut offset = 0_u32;
                            let stride = std::mem::size_of::<Vertex>() as u32;
                            context.IASetVertexBuffers(
                                0,
                                1,
                                Some(&vertex_buffer as *const _),
                                Some(&stride as *const _),
                                Some(&mut offset),
                            );
                            context.IASetPrimitiveTopology(topology);
                        }

                        unsafe { context.Draw(verticies.len() as u32, 0) };

                        unsafe {
                            match swap_chain.Present(1, 0) {
                                S_OK => {}
                                err => {
                                    log::debug!("Failed present {err}")
                                }
                            }
                        };

                        // Pull all frames out and use the latest
                        let mut last_video = None;
                        loop {
                            match rx.try_recv() {
                                Ok(frame) => last_video = Some(frame),
                                Err(TryRecvError::Empty) => break,
                                Err(TryRecvError::Disconnected) => {
                                    log::error!("player disconnected");
                                    Err(TryRecvError::Disconnected)?
                                }
                            }
                        }

                        if let Some(video) = last_video {
                            copy_texture(&texture, &video, None)?;
                        } else {
                            // log::warn!("no frame for player");
                            // log::error!("!!! no video frame waiting?")
                        }
                    }

                    Ok(())
                })
                .await
                .unwrap()
                {
                    Ok(_ok) => log::warn!("player sink down ok"),
                    Err(err) => log::error!("player sink down error {err}"),
                };
            }
        });

        Ok(tx)
    }
}
