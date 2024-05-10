use std::{
    collections::VecDeque,
    sync::{Arc, Weak},
};

use derive_more::Deref;
use parking_lot::Mutex;
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;

#[derive(Debug, Deref)]
pub struct Texture(#[deref] ID3D11Texture2D, Weak<Mutex<_TexturePool>>);

impl Drop for Texture {
    fn drop(&mut self) {
        if let Some(pool) = self.1.upgrade() {
            pool.lock().release(self.0.clone())
        }
    }
}

impl Texture {
    pub fn unpooled(texture: ID3D11Texture2D) -> Self {
        Self(texture, Weak::new())
    }
}

struct _TexturePool {
    // TODO(emily): Would probably be smart to use an mpsc channel here as then
    // we can wait for a pool element and dont need to panic
    pool: VecDeque<ID3D11Texture2D>,
}

impl _TexturePool {
    fn new<F: Fn() -> ID3D11Texture2D>(make_texture: F, count: usize) -> Self {
        let mut pool = VecDeque::new();

        for i in 0..count {
            pool.push_back(make_texture());
        }

        Self { pool }
    }

    fn acquire(&mut self) -> ID3D11Texture2D {
        self.pool.pop_front().unwrap()
    }

    fn release(&mut self, texture: ID3D11Texture2D) {
        self.pool.push_back(texture);
    }
}

pub(crate) struct TexturePool(Arc<Mutex<_TexturePool>>);

impl TexturePool {
    // TODO(emily): Pick a better name
    pub(crate) fn update_texture_format<F: Fn() -> ID3D11Texture2D>(&self, make: F, count: usize) {
        *self.0.lock() = _TexturePool::new(make, count);
    }

    pub(crate) fn new<F: Fn() -> ID3D11Texture2D>(make: F, count: usize) -> Self {
        Self(Arc::new(Mutex::new(_TexturePool::new(make, count))))
    }

    pub(crate) fn acquire(&self) -> Texture {
        let texture = self.0.lock().acquire();

        Texture(texture, Arc::downgrade(&self.0))
    }

    pub(crate) fn release(&self, texture: ID3D11Texture2D) {
        self.0.lock().release(texture);
    }
}
