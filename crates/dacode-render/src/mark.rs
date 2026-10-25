//! The pipeline that draws the DaCode mark. See `mark.wgsl` for why it is a pair of masks.

use crate::frame::{Layer, Mark};
use bytemuck::{Pod, Zeroable};

/// The masks, 512 pixels square: red is the letter, green the amber block. Derived from the logo
/// sheet; `crates/dacode/examples/icons.rs` builds every icon from the same two files.
const FULL: &[u8] = include_bytes!("../assets/mark.png");
const SMALL: &[u8] = include_bytes!("../assets/mark-small.png");
const SIDE: u32 = 512;

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct Instance {
    rect: [f32; 4],
    ink: [f32; 4],
    accent: [f32; 4],
    variant: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct Params {
    screen: [f32; 2],
    _pad: [f32; 2],
}

const INSTANCE_ATTRIBUTES: [wgpu::VertexAttribute; 4] =
    wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4];

/// The red and green channels of a bundled mask, as tightly packed RG pairs.
fn decode(png_bytes: &[u8]) -> Vec<u8> {
    let decoder = png::Decoder::new(std::io::Cursor::new(png_bytes));
    let mut reader = decoder
        .read_info()
        .expect("the bundled mark is a valid PNG");
    let mut pixels = vec![0; reader.output_buffer_size().expect("a decodable size")];
    let info = reader
        .next_frame(&mut pixels)
        .expect("the bundled mark decodes");
    let channels = info.color_type.samples();
    pixels[..info.buffer_size()]
        .chunks_exact(channels)
        .flat_map(|pixel| [pixel[0], pixel[1]])
        .collect()
}

/// Every mip level of an RG mask, largest first, each a box-filtered half of the one before.
///
/// Built here rather than shipped: the title bar draws the mark at 28 pixels from a 512 one, and
/// sampling that far down without mips turns its curves to sparkle.
fn mips(mut level: Vec<u8>, mut side: u32) -> Vec<(u32, Vec<u8>)> {
    let mut levels = vec![(side, level.clone())];
    while side > 1 {
        let half = side / 2;
        let at =
            |x: u32, y: u32, channel: usize| level[((y * side + x) * 2) as usize + channel] as u32;
        let next: Vec<u8> = (0..half)
            .flat_map(|y| (0..half).map(move |x| (x, y)))
            .flat_map(|(x, y)| (0..2).map(move |channel| (x, y, channel)))
            .map(|(x, y, channel)| {
                ((at(2 * x, 2 * y, channel)
                    + at(2 * x + 1, 2 * y, channel)
                    + at(2 * x, 2 * y + 1, channel)
                    + at(2 * x + 1, 2 * y + 1, channel)
                    + 2)
                    / 4) as u8
            })
            .collect();
        levels.push((half, next.clone()));
        level = next;
        side = half;
    }
    levels
}

pub struct MarkPipeline {
    pipeline: wgpu::RenderPipeline,
    params_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    instances: wgpu::Buffer,
    capacity: usize,
    ranges: Vec<(Layer, std::ops::Range<u32>)>,
}

impl MarkPipeline {
    /// The mark appears at most a few times in a frame — title bar, and an empty state.
    const INITIAL_CAPACITY: usize = 8;

    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("dacode mark shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("mark.wgsl").into()),
        });

        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dacode mark params"),
            size: std::mem::size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let levels = mips(decode(FULL), SIDE).len() as u32;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("dacode mark masks"),
            size: wgpu::Extent3d {
                width: SIDE,
                height: SIDE,
                depth_or_array_layers: 2,
            },
            mip_level_count: levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // Coverage, not colour: linear, so a half-covered edge pixel is half, not sRGB's idea of it.
            format: wgpu::TextureFormat::Rg8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (layer, source) in [FULL, SMALL].into_iter().enumerate() {
            for (mip, (side, data)) in mips(decode(source), SIDE).into_iter().enumerate() {
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &texture,
                        mip_level: mip as u32,
                        origin: wgpu::Origin3d {
                            x: 0,
                            y: 0,
                            z: layer as u32,
                        },
                        aspect: wgpu::TextureAspect::All,
                    },
                    &data,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(side * 2),
                        rows_per_image: Some(side),
                    },
                    wgpu::Extent3d {
                        width: side,
                        height: side,
                        depth_or_array_layers: 1,
                    },
                );
            }
        }
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("dacode mark sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("dacode mark layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("dacode mark bind group"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("dacode mark pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("dacode mark pipeline"),
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
                    // Coverage in alpha, so the mark sits on whatever surface it is drawn on rather
                    // than on a tile of its own.
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
            label: Some("dacode mark instances"),
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
        marks: &[Mark],
    ) {
        queue.write_buffer(
            &self.params_buffer,
            0,
            bytemuck::bytes_of(&Params {
                screen: [width as f32, height as f32],
                _pad: [0.0, 0.0],
            }),
        );

        let mut instances: Vec<Instance> = Vec::with_capacity(marks.len());
        let mut ranges = Vec::new();

        for layer in Layer::ALL {
            let start = instances.len() as u32;
            instances.extend(marks.iter().filter(|mark| mark.layer == layer).map(|mark| {
                Instance {
                    rect: [mark.rect.x, mark.rect.y, mark.rect.width, mark.rect.height],
                    ink: mark.ink,
                    accent: mark.accent,
                    variant: [if mark.small { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0],
                }
            }));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_masks_decode_to_the_size_the_texture_expects() {
        // A mask exported at another size would upload past the end of its layer, which wgpu
        // reports as a validation error — at startup, on every machine.
        for mask in [FULL, SMALL] {
            assert_eq!(decode(mask).len(), (SIDE * SIDE * 2) as usize);
        }
    }

    #[test]
    fn the_mip_chain_runs_down_to_one_pixel() {
        let levels = mips(decode(FULL), SIDE);
        assert_eq!(levels.len(), 10, "512 halves nine times to reach 1");
        assert_eq!(
            levels.last().map(|(side, data)| (*side, data.len())),
            Some((1, 2))
        );
    }
}
