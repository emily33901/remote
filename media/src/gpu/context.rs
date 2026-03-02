use std::sync::Arc;

use anyhow::Result;

pub struct GpuContext {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
}

impl GpuContext {
    pub async fn new() -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: Self::preferred_backends(),
            ..Default::default()
        });
        
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .ok_or_else(|| anyhow::anyhow!("No suitable GPU adapter found"))?;
        
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("media gpu context"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                },
                None,
            )
            .await?;
        
        Ok(Self {
            device: Arc::new(device),
            queue: Arc::new(queue),
        })
    }
    
    fn preferred_backends() -> wgpu::Backends {
        #[cfg(target_os = "windows")]
        {
            wgpu::Backends::DX12
        }
        
        #[cfg(target_os = "linux")]
        {
            wgpu::Backends::VULKAN
        }
        
        #[cfg(target_os = "macos")]
        {
            wgpu::Backends::METAL
        }
        
        #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
        {
            wgpu::Backends::all()
        }
    }
    
    pub fn device(&self) -> &Arc<wgpu::Device> {
        &self.device
    }
    
    pub fn queue(&self) -> &Arc<wgpu::Queue> {
        &self.queue
    }
}
