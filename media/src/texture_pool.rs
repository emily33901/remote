use std::{
    collections::VecDeque,
    sync::{Arc, Weak},
};

use derive_more::{Deref, DerefMut};
use parking_lot::Mutex;
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;

#[derive(Debug, Clone, Deref, DerefMut)]
pub(crate) struct Texture(
    #[deref]
    #[deref_mut]
    ID3D11Texture2D,
    Weak<Mutex<_TexturePool>>,
);

impl Texture {
    pub(crate) fn texture(&self) -> &ID3D11Texture2D {
        &self.0
    }
}

impl Drop for Texture {
    fn drop(&mut self) {
        if let Some(pool) = self.1.upgrade() {
            pool.lock().pool.push_back(self.0.clone());
        }
    }
}

struct _TexturePool {
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

    fn get(&mut self) -> ID3D11Texture2D {
        self.pool.pop_front().unwrap()
    }
}

pub(crate) struct TexturePool(Arc<Mutex<_TexturePool>>);

impl TexturePool {
    pub(crate) fn new<F: Fn() -> ID3D11Texture2D>(make: F, count: usize) -> Self {
        Self(Arc::new(Mutex::new(_TexturePool::new(make, count))))
    }

    pub(crate) fn get(&self) -> Texture {
        Texture(self.0.lock().get(), Arc::downgrade(&self.0))
    }
}
