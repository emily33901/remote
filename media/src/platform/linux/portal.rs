use std::collections::HashMap;
use std::os::unix::io::OwnedFd;

use anyhow::Result;
use zbus::proxy;
use zbus::zvariant::{DeserializeDict, Dict, SerializeDict, Type, Value};

#[proxy(
    interface = "org.freedesktop.portal.ScreenCast",
    default_service = "org.freedesktop.portal.Desktop",
    default_path = "/org/freedesktop/portal/desktop"
)]
trait ScreenCast {
    async fn create_session(&self, options: SessionOptions) -> Result<SessionResult>;
    
    async fn select_sources(
        &self,
        session_handle: &str,
        options: SelectSourcesOptions,
    ) -> Result<()>;
    
    async fn start(
        &self,
        session_handle: &str,
        parent_window: &str,
    ) -> Result<StartResult>;
    
    async fn open_pipe_wire_remote(&self, session_handle: &str) -> Result<OwnedFd>;
}

#[derive(Debug, Type, SerializeDict, DeserializeDict)]
pub struct SessionOptions {
    handle_token: String,
    session_handle_token: String,
}

impl SessionOptions {
    pub fn new() -> Self {
        let token = format!("session{}", std::process::id());
        Self {
            handle_token: format!("handle{}", std::process::id()),
            session_handle_token: token,
        }
    }
}

#[derive(Debug, Type, SerializeDict, DeserializeDict)]
pub struct SelectSourcesOptions {
    types: u32,
    multiple: bool,
    cursor_mode: u32,
    restore_data: Option<(String, u32)>,
    persist_mode: u32,
}

impl SelectSourcesOptions {
    pub fn new() -> Self {
        Self {
            types: 1,
            multiple: false,
            cursor_mode: 1,
            restore_data: None,
            persist_mode: 0,
        }
    }
    
    pub fn with_cursor(mut self, show: bool) -> Self {
        self.cursor_mode = if show { 2 } else { 1 };
        self
    }
}

#[derive(Debug, Type, DeserializeDict)]
pub struct SessionResult {
    pub session_handle: String,
}

#[derive(Debug, Type, DeserializeDict)]
pub struct StartResult {
    pub streams: Vec<Stream>,
}

#[derive(Debug, Type, DeserializeDict)]
pub struct Stream {
    pub id: u32,
    pub position: (i32, i32),
    pub size: (u32, u32),
    pub source_type: u32,
    pub mapping: Option<String>,
}

pub struct PortalSession {
    pub session_handle: String,
    pub streams: Vec<Stream>,
    pub pipewire_fd: OwnedFd,
}

pub async fn start_screen_capture_session(show_cursor: bool) -> Result<PortalSession> {
    let connection = zbus::Connection::session().await?;
    let proxy = ScreenCastProxy::new(&connection).await?;
    
    let session_options = SessionOptions::new();
    let session_result = proxy.create_session(session_options).await?;
    let session_handle = session_result.session_handle;
    
    let select_options = SelectSourcesOptions::new().with_cursor(show_cursor);
    proxy.select_sources(&session_handle, select_options).await?;
    
    let start_result = proxy.start(&session_handle, "").await?;
    
    let pipewire_fd = proxy.open_pipe_wire_remote(&session_handle).await?;
    
    Ok(PortalSession {
        session_handle,
        streams: start_result.streams,
        pipewire_fd,
    })
}
