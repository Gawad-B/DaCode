//! The rectangle pipeline. One instanced draw call for every non-text element on screen.

use crate::frame::{Layer, Quad};
use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct Instance {
    rect: [f32; 4],
    color: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct Params {
    screen: [f32; 2],
    _pad: [f32; 2],
}

/// `rect` then `color`, matching the `Instance` struct in `quad.wgsl`. A const so the array
/// outlives the descriptor that borrows it.
const INSTANCE_ATTRIBUTES: [wgpu::VertexAttribute; 2] =
    wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4];

pub struct QuadPipeline {
    pipeline: wgpu::RenderPipeline,
    params_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    instances: wgpu::Buffer,
    capacity: usize,
    /// Where each layer's instances start and end in the buffer, so a layer can be drawn on its own.
    ranges: Vec<(Layer, std::ops::Range<u32>)>,
}

impl QuadPipeline {
    const INITIAL_CAPACITY: usize = 1024;

    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("vex quad shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("quad.wgsl").into()),
        });

        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vex quad params"),
            size: std::mem::size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vex quad layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vex quad bind group"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: params_buffer.as_entire_binding(),
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("vex quad pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("vex quad pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Instance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &INSTANCE_ATTRIBUTES,
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    // Straight alpha: the cursor and selection are drawn translucent over the
                    // canvas rather than pre-composited.
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let instances = Self::alloc(device, Self::INITIAL_CAPACITY);

        Self {
            pipeline,
            params_buffer,
            bind_group,
            instances,
            capacity: Self::INITIAL_CAPACITY,
            ranges: Vec::new(),
        }
    }

    fn alloc(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vex quad instances"),
            size: (capacity * std::mem::size_of::<Instance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        quads: &[Quad],
    ) {
        queue.write_buffer(
            &self.params_buffer,
            0,
            bytemuck::bytes_of(&Params {
                screen: [width as f32, height as f32],
                _pad: [0.0, 0.0],
            }),
        );

        // Grouped by layer so each layer can be drawn between the text passes. Stable within a
        // layer, so painting order inside one is still the order the UI asked for.
        let mut instances: Vec<Instance> = Vec::with_capacity(quads.len());
        let mut ranges = Vec::new();

        for layer in Layer::ALL {
            let start = instances.len() as u32;
            instances.extend(quads.iter().filter(|quad| quad.layer == layer).map(|quad| {
                Instance {
                    rect: [quad.rect.x, quad.rect.y, quad.rect.width, quad.rect.height],
                    color: quad.color,
                }
            }));
            let end = instances.len() as u32;
            if end > start {
                ranges.push((layer, start..end));
            }
        }

        if instances.len() > self.capacity {
            // Grow geometrically so a busy frame does not reallocate repeatedly.
            self.capacity = instances.len().next_power_of_two();
            self.instances = Self::alloc(device, self.capacity);
        }

        if !instances.is_empty() {
            queue.write_buffer(&self.instances, 0, bytemuck::cast_slice(&instances));
        }
        self.ranges = ranges;
    }

    /// Draws one layer's rectangles.
    pub fn render(&self, pass: &mut wgpu::RenderPass<'_>, layer: Layer) {
        let Some((_, range)) = self.ranges.iter().find(|(l, _)| *l == layer) else {
            return;
        };
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.instances.slice(..));
        pass.draw(0..4, range.clone());
    }
}
