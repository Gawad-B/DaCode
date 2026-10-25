//! The pipeline for antialiased geometry — what icons are made of. See `shape.wgsl`.

use crate::frame::{Layer, Shape, ShapeKind};
use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct Instance {
    ab: [f32; 4],
    c: [f32; 4],
    style: [f32; 4],
    color: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct Params {
    screen: [f32; 2],
    _pad: [f32; 2],
}

const INSTANCE_ATTRIBUTES: [wgpu::VertexAttribute; 4] =
    wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4];

pub struct ShapePipeline {
    pipeline: wgpu::RenderPipeline,
    params_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    instances: wgpu::Buffer,
    capacity: usize,
    ranges: Vec<(Layer, std::ops::Range<u32>)>,
}

impl ShapePipeline {
    /// A dozen icons of four to eight strokes each is a typical frame.
    const INITIAL_CAPACITY: usize = 256;

    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("dacode shape shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shape.wgsl").into()),
        });

        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dacode shape params"),
            size: std::mem::size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("dacode shape layout"),
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
            label: Some("dacode shape bind group"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: params_buffer.as_entire_binding(),
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("dacode shape pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("dacode shape pipeline"),
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
                    blend: Some(crate::quad::BLEND),
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

        Self {
            pipeline,
            params_buffer,
            bind_group,
            instances: Self::alloc(device, Self::INITIAL_CAPACITY),
            capacity: Self::INITIAL_CAPACITY,
            ranges: Vec::new(),
        }
    }

    fn alloc(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dacode shape instances"),
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
        shapes: &[Shape],
    ) {
        queue.write_buffer(
            &self.params_buffer,
            0,
            bytemuck::bytes_of(&Params {
                screen: [width as f32, height as f32],
                _pad: [0.0, 0.0],
            }),
        );

        let mut instances: Vec<Instance> = Vec::with_capacity(shapes.len());
        let mut ranges = Vec::new();
        for layer in Layer::ALL {
            let start = instances.len() as u32;
            instances.extend(
                shapes
                    .iter()
                    .filter(|shape| shape.layer == layer)
                    .map(instance),
            );
            let end = instances.len() as u32;
            if end > start {
                ranges.push((layer, start..end));
            }
        }

        if instances.len() > self.capacity {
            self.capacity = instances.len().next_power_of_two();
            self.instances = Self::alloc(device, self.capacity);
        }
        if !instances.is_empty() {
            queue.write_buffer(&self.instances, 0, bytemuck::cast_slice(&instances));
        }
        self.ranges = ranges;
    }

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

fn instance(shape: &Shape) -> Instance {
    let (kind, ab, c, radius) = match shape.kind {
        ShapeKind::Segment { from, to } => (0.0, [from[0], from[1], to[0], to[1]], [0.0; 4], 0.0),
        ShapeKind::Ring { centre, radius } => {
            (1.0, [centre[0], centre[1], 0.0, 0.0], [0.0; 4], radius)
        }
        ShapeKind::Arc {
            centre,
            radius,
            start,
            end,
        } => (
            2.0,
            [centre[0], centre[1], 0.0, 0.0],
            [start, end, 0.0, 0.0],
            radius,
        ),
        ShapeKind::Disc { centre, radius } => {
            (3.0, [centre[0], centre[1], 0.0, 0.0], [0.0; 4], radius)
        }
        ShapeKind::Triangle { a, b, c } => {
            (4.0, [a[0], a[1], b[0], b[1]], [c[0], c[1], 0.0, 0.0], 0.0)
        }
    };
    Instance {
        ab,
        c,
        style: [shape.width, kind, radius, 0.0],
        color: shape.color,
    }
}
