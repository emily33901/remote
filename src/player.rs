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

use std::{
    io::{self},
    task::Poll,
};

use tokio::io::AsyncRead;
use tokio::sync::mpsc::Receiver;

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
                store.lock().await.extend(buffer.iter());
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
        let (control_tx, control_rx) = mpsc::channel(10);
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
                let source = reader.periodic_access(std::time::Duration::from_millis(1000), |r| {
                    log::debug!("!!!! player periodic access");
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
