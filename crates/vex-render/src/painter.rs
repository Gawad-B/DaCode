//! The drawing core, independent of where the pixels end up.
//!
//! Shared by the on-screen renderer and the offscreen one. Splitting it out is what makes the
//! editor's appearance verifiable: the same code path that draws the window can render into a
//! texture and be compared against a reference image, with no compositor involved.

use crate::frame::{Frame, Layer};
use crate::quad::QuadPipeline;
use crate::text::TextStack;
use anyhow::Result;
use vex_theme::Oklch;

pub struct Painter {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    quads: QuadPipeline,
    text: TextStack,
}

impl Painter {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let quads = QuadPipeline::new(&device, format);
        let text = TextStack::new(&device, &queue, format);
        Self {
            device,
            queue,
            quads,
            text,
        }
    }

    pub fn resolved_families(&self) -> [&str; 3] {
        self.text.resolved_families()
    }

    /// Uploads this frame's geometry and glyphs, then records the draw calls into `encoder`.
    pub fn paint(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        width: u32,
        height: u32,
        frame: &Frame,
        clear: Oklch,
    ) -> Result<()> {
        self.quads
            .prepare(&self.device, &self.queue, width, height, &frame.quads);
        self.text
            .prepare(&self.device, &self.queue, width, height, &frame.texts)?;

        let [r, g, b, a] = clear.to_linear();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("vex main pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: r as f64,
                        g: g as f64,
                        b: b as f64,
                        a: a as f64,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });

        // Layer by layer: each layer's rectangles, then its glyphs. Drawing all rectangles first
        // would leave every overlay transparent, because the text beneath would be painted over it.
        for layer in Layer::ALL {
            self.quads.render(&mut pass, layer);
            self.text.render(&mut pass, layer)?;
        }
        Ok(())
    }

    pub fn trim_atlas(&mut self) {
        self.text.trim_atlas();
    }
}
