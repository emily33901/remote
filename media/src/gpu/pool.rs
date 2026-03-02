use crate::gpu::texture::GpuTexture;

pub struct TexturePool;

impl TexturePool {
    pub fn new() -> Self {
        Self
    }

    pub fn acquire(&self) -> Option<GpuTexture> {
        None
    }
}
