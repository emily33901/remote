use std::sync::Arc;

use anyhow::Result;

use crate::gpu::context::GpuContext;
use crate::types::{CpuBuffer, PixelFormat};

pub struct FormatConverter {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    bgra_to_nv12_pipeline: wgpu::ComputePipeline,
    nv12_to_i420_pipeline: wgpu::ComputePipeline,
    i420_to_nv12_pipeline: wgpu::ComputePipeline,
}

impl FormatConverter {
    pub fn new(context: &GpuContext) -> Result<Self> {
        let device = context.device().clone();
        let queue = context.queue().clone();

        let bgra_to_nv12_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("bgra_to_nv12"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/bgra_to_nv12.wgsl").into()),
        });

        let nv12_to_i420_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("nv12_to_i420"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/nv12_to_i420.wgsl").into()),
        });

        let i420_to_nv12_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("i420_to_nv12"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/i420_to_nv12.wgsl").into()),
        });

        let bgra_to_nv12_pipeline =
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("bgra_to_nv12_pipeline"),
                layout: None,
                module: &bgra_to_nv12_shader,
                entry_point: "bgra_to_nv12",
            });

        let nv12_to_i420_pipeline =
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("nv12_to_i420_pipeline"),
                layout: None,
                module: &nv12_to_i420_shader,
                entry_point: "nv12_to_i420",
            });

        let i420_to_nv12_pipeline =
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("i420_to_nv12_pipeline"),
                layout: None,
                module: &i420_to_nv12_shader,
                entry_point: "i420_to_nv12",
            });

        Ok(Self {
            device,
            queue,
            bgra_to_nv12_pipeline,
            nv12_to_i420_pipeline,
            i420_to_nv12_pipeline,
        })
    }

    pub fn bgra_to_nv12(&self, input: &wgpu::Texture) -> Result<wgpu::Texture> {
        let width = input.width();
        let height = input.height();

        let output_y = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("nv12_y"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        let output_uv = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("nv12_uv"),
            size: wgpu::Extent3d {
                width: width / 2,
                height: height / 2,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rg8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        let input_view = input.create_view(&wgpu::TextureViewDescriptor::default());
        let output_y_view = output_y.create_view(&wgpu::TextureViewDescriptor::default());
        let output_uv_view = output_uv.create_view(&wgpu::TextureViewDescriptor::default());

        let bind_group_layout = self.bgra_to_nv12_pipeline.get_bind_group_layout(0);

        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bgra_to_nv12_bind_group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&input_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&output_y_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&output_uv_view),
                },
            ],
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("bgra_to_nv12_encoder"),
            });

        {
            let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("bgra_to_nv12_pass"),
                timestamp_writes: None,
            });

            compute_pass.set_pipeline(&self.bgra_to_nv12_pipeline);
            compute_pass.set_bind_group(0, &bind_group, &[]);
            compute_pass.dispatch_workgroups(width / 8 + 1, height / 8 + 1, 1);
        }

        self.queue.submit(std::iter::once(encoder.finish()));

        Ok(output_y)
    }

    pub fn cpu_buffer_to_texture(&self, buffer: &CpuBuffer) -> Result<wgpu::Texture> {
        let format = match buffer.format {
            PixelFormat::Bgra => wgpu::TextureFormat::Bgra8Unorm,
            PixelFormat::Rgba => wgpu::TextureFormat::Rgba8Unorm,
            PixelFormat::Nv12 => wgpu::TextureFormat::R8Unorm,
            PixelFormat::I420 => wgpu::TextureFormat::R8Unorm,
            PixelFormat::I444 => wgpu::TextureFormat::R8Unorm,
        };

        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("cpu_upload_texture"),
            size: wgpu::Extent3d {
                width: buffer.width,
                height: buffer.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });

        self.queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &buffer.data,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(buffer.stride.first().copied().unwrap_or(buffer.width * 4)),
                rows_per_image: Some(buffer.height),
            },
            wgpu::Extent3d {
                width: buffer.width,
                height: buffer.height,
                depth_or_array_layers: 1,
            },
        );

        Ok(texture)
    }

    pub fn texture_to_cpu_buffer(
        &self,
        texture: &wgpu::Texture,
        format: PixelFormat,
    ) -> Result<CpuBuffer> {
        let width = texture.width();
        let height = texture.height();

        let bytes_per_pixel = match format {
            PixelFormat::Bgra | PixelFormat::Rgba => 4,
            PixelFormat::Nv12 => 1,
            PixelFormat::I420 => 1,
            PixelFormat::I444 => 1,
        };

        let row_bytes = width * bytes_per_pixel;
        let total_bytes = (row_bytes * height) as usize;

        let output_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("texture_download_buffer"),
            size: total_bytes as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("texture_download_encoder"),
            });

        encoder.copy_texture_to_buffer(
            wgpu::ImageCopyTexture {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyBuffer {
                buffer: &output_buffer,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(row_bytes),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        self.queue.submit(std::iter::once(encoder.finish()));

        let buffer_slice = output_buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            tx.send(result).ok();
        });
        self.device.poll(wgpu::Maintain::Wait);

        rx.recv()?
            .map_err(|e| anyhow::anyhow!("Failed to map buffer: {:?}", e))?;

        let data = buffer_slice.get_mapped_range().to_vec();

        Ok(CpuBuffer {
            data,
            width,
            height,
            format,
            stride: vec![row_bytes],
        })
    }
}
