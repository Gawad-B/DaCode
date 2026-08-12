//! Offscreen rendering — the editor drawing itself into an image with no window.
//!
//! This exists for verification, and it earns its place twice over: it is how the design gets
//! checked without a compositor (screenshots over SSH, in CI, in a headless VM), and it is the
//! basis for reference-image tests once the chrome stabilizes.

use crate::frame::Frame;
use crate::painter::Painter;
use anyhow::{Context, Result};
use vex_theme::Oklch;

/// sRGB so the written PNG matches what a display would show.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

pub struct Headless {
    painter: Painter,
    width: u32,
    height: u32,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

impl Headless {
    pub fn new(width: u32, height: u32) -> Result<Self> {
        pollster::block_on(Self::new_async(width, height))
    }

    async fn new_async(width: u32, height: u32) -> Result<Self> {
        let instance = wgpu::Instance::default();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .context("no GPU adapter available for offscreen rendering")?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("vex headless device"),
                ..Default::default()
            })
            .await
            .context("requesting an offscreen device")?;

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("vex headless target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        Ok(Self {
            painter: Painter::new(device, queue, FORMAT),
            width,
            height,
            texture,
            view,
        })
    }

    pub fn resolved_families(&self) -> [&str; 3] {
        self.painter.resolved_families()
    }

    /// Renders a frame and reads it back as tightly-packed RGBA8.
    pub fn capture(&mut self, frame: &Frame, clear: Oklch) -> Result<Vec<u8>> {
        // Buffer rows must be aligned to 256 bytes for a texture copy, so the readback is
        // padded and then unpadded below.
        let unpadded_row = self.width * 4;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_row = unpadded_row.div_ceil(align) * align;

        let readback = self.painter.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vex readback"),
            size: (padded_row * self.height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder =
            self.painter
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("vex headless frame"),
                });

        self.painter.paint(
            &mut encoder,
            &self.view,
            self.width,
            self.height,
            frame,
            clear,
        )?;

        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );

        self.painter.queue.submit(Some(encoder.finish()));

        let (sender, receiver) = std::sync::mpsc::channel();
        readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        self.painter
            .device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .context("waiting for the GPU to finish")?;
        receiver
            .recv()
            .context("readback channel closed")?
            .context("mapping the readback buffer")?;

        let mapped = readback
            .slice(..)
            .get_mapped_range()
            .context("reading the mapped range")?;
        let mut pixels = Vec::with_capacity((unpadded_row * self.height) as usize);
        for row in 0..self.height {
            let start = (row * padded_row) as usize;
            pixels.extend_from_slice(&mapped[start..start + unpadded_row as usize]);
        }
        drop(mapped);
        readback.unmap();

        Ok(pixels)
    }

    /// Renders a frame straight to a PNG on disk.
    pub fn capture_png(
        &mut self,
        frame: &Frame,
        clear: Oklch,
        path: impl AsRef<std::path::Path>,
    ) -> Result<()> {
        let pixels = self.capture(frame, clear)?;
        let file = std::fs::File::create(path.as_ref())
            .with_context(|| format!("creating {}", path.as_ref().display()))?;
        let writer = std::io::BufWriter::new(file);

        let mut encoder = png::Encoder::new(writer, self.width, self.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .context("writing the PNG header")?
            .write_image_data(&pixels)
            .context("writing PNG pixels")?;
        Ok(())
    }
}
