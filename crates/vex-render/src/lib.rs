//! `vex-render` — the GPU layer: rectangles and shaped text, nothing else.
//!
//! Owning `winit` + `wgpu` + `glyphon` directly rather than adopting a widget framework is a
//! deliberate cost. Immediate-mode and retained-widget toolkits are not built for per-glyph
//! styling over million-line documents with block cursors and virtualized scrolling, and the
//! ceiling they impose would be hit exactly where this project cannot afford one.
//!
//! The UI layer above builds a [`Frame`] of primitives with no GPU involvement, which keeps
//! layout testable headlessly and keeps this module small.

pub mod frame;
mod headless;
mod painter;
mod quad;
mod text;
mod timer;

pub use frame::{Align, Frame, Layer, Quad, Rect, TextRun};
pub use headless::Headless;
pub use timer::FrameTimer;

use anyhow::{Context, Result};
use painter::Painter;
use std::sync::Arc;
use vex_theme::Oklch;
use wgpu::CurrentSurfaceTexture;
use winit::window::Window;

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    painter: Painter,
    pub timer: FrameTimer,
}

impl Renderer {
    pub fn new(window: Arc<Window>) -> Result<Self> {
        pollster::block_on(Self::new_async(window))
    }

    async fn new_async(window: Arc<Window>) -> Result<Self> {
        let size = window.inner_size();
        let (width, height) = (size.width.max(1), size.height.max(1));

        let instance = wgpu::Instance::default();
        let surface = instance
            .create_surface(window.clone())
            .context("creating the window surface")?;

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                // An editor is not a game: integrated graphics is the better default for
                // battery life, and the workload is trivial for any GPU.
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await
            .context("no suitable GPU adapter")?;

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("vex device"),
                ..Default::default()
            })
            .await
            .context("requesting the GPU device")?;

        let mut config = surface
            .get_default_config(&adapter, width, height)
            .context("surface is incompatible with this adapter")?;
        // Vsync: an editor that renders faster than the display is burning battery for nothing.
        config.present_mode = wgpu::PresentMode::AutoVsync;
        surface.configure(&device, &config);

        let format = config.format;
        Ok(Self {
            surface,
            config,
            painter: Painter::new(device, queue, format),
            timer: FrameTimer::new(),
        })
    }

    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    pub fn resolved_families(&self) -> [&str; 3] {
        self.painter.resolved_families()
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.painter.device, &self.config);
    }

    /// Draws a frame. `clear` is painted before anything else; the design requires the window
    /// background to be set before the first paint so startup never flashes light.
    pub fn render(&mut self, frame: &Frame, clear: Oklch) -> Result<()> {
        self.timer.begin();

        let surface_texture = match self.surface.get_current_texture() {
            CurrentSurfaceTexture::Success(texture) => texture,
            // Suboptimal still hands back a usable texture; reconfigure so the *next* frame is
            // optimal instead of dropping this one.
            CurrentSurfaceTexture::Suboptimal(texture) => {
                self.surface.configure(&self.painter.device, &self.config);
                texture
            }
            // Routine, not errors: resize, monitor change, minimized window. Skip the frame.
            CurrentSurfaceTexture::Outdated | CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.painter.device, &self.config);
                self.timer.end();
                return Ok(());
            }
            CurrentSurfaceTexture::Timeout | CurrentSurfaceTexture::Occluded => {
                self.timer.end();
                return Ok(());
            }
            CurrentSurfaceTexture::Validation => {
                anyhow::bail!("surface validation error while acquiring a frame")
            }
        };

        let view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let (width, height) = (self.config.width, self.config.height);

        let mut encoder =
            self.painter
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("vex frame"),
                });

        self.painter
            .paint(&mut encoder, &view, width, height, frame, clear)?;

        self.painter.queue.submit(Some(encoder.finish()));
        self.painter.queue.present(surface_texture);
        self.painter.trim_atlas();

        self.timer.end();
        Ok(())
    }
}
