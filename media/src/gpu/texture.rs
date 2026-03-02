use std::sync::{Arc, Mutex, Weak};

use crate::types::PixelFormat;

pub struct GpuTexture {
    texture: Arc<wgpu::Texture>,
    view: wgpu::TextureView,
    format: PixelFormat,
    pool: Weak<Mutex<TexturePoolInner>>,
}

impl GpuTexture {
    pub fn new(texture: wgpu::Texture, format: PixelFormat) -> Self {
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            texture: Arc::new(texture),
            view,
            format,
            pool: Weak::new(),
        }
    }

    pub fn width(&self) -> u32 {
        self.texture.width()
    }

    pub fn height(&self) -> u32 {
        self.texture.height()
    }

    pub fn format(&self) -> PixelFormat {
        self.format
    }

    pub fn texture(&self) -> &Arc<wgpu::Texture> {
        &self.texture
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }
}

impl Drop for GpuTexture {
    fn drop(&mut self) {
        if let Some(pool) = self.pool.upgrade() {
            if let Ok(mut inner) = pool.lock() {
                inner.release(self.texture.clone());
            }
        }
    }
}

pub struct TexturePoolInner {
    pool: Vec<Arc<wgpu::Texture>>,
}

impl TexturePoolInner {
    fn release(&mut self, texture: Arc<wgpu::Texture>) {
        self.pool.push(texture);
    }
}

pub struct TexturePool {
    inner: Arc<Mutex<TexturePoolInner>>,
    device: Arc<wgpu::Device>,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
}

impl TexturePool {
    pub fn new(
        device: Arc<wgpu::Device>,
        width: u32,
        height: u32,
        format: PixelFormat,
        count: usize,
    ) -> Self {
        let wgpu_format = pixel_format_to_wgpu(format);

        let mut pool = Vec::with_capacity(count);
        for _ in 0..count {
            let texture = Self::create_texture(&device, width, height, wgpu_format);
            pool.push(Arc::new(texture));
        }

        Self {
            inner: Arc::new(Mutex::new(TexturePoolInner { pool })),
            device,
            width,
            height,
            format: wgpu_format,
        }
    }

    pub fn acquire(&self) -> Option<GpuTexture> {
        let texture = self.inner.lock().ok()?.pool.pop()?;
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let pixel_format = wgpu_to_pixel_format(self.format);

        Some(GpuTexture {
            texture,
            view,
            format: pixel_format,
            pool: Arc::downgrade(&self.inner),
        })
    }

    fn create_texture(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("texture pool texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
    }
}

fn pixel_format_to_wgpu(format: PixelFormat) -> wgpu::TextureFormat {
    match format {
        PixelFormat::Bgra => wgpu::TextureFormat::Bgra8Unorm,
        PixelFormat::Rgba => wgpu::TextureFormat::Rgba8Unorm,
        PixelFormat::Nv12 => wgpu::TextureFormat::R8Unorm,
        PixelFormat::I420 => wgpu::TextureFormat::R8Unorm,
        PixelFormat::I444 => wgpu::TextureFormat::R8Unorm,
    }
}

fn wgpu_to_pixel_format(format: wgpu::TextureFormat) -> PixelFormat {
    match format {
        wgpu::TextureFormat::Bgra8Unorm => PixelFormat::Bgra,
        wgpu::TextureFormat::Rgba8Unorm => PixelFormat::Rgba,
        _ => PixelFormat::I420,
    }
}
