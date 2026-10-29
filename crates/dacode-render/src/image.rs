//! Pictures: decoded images and rendered PDF pages, each in a texture of its own. See `image.wgsl`.
//!
//! A [`Picture`] is built off the UI thread, mips included, and handed to the frame behind an `Arc`;
//! this pipeline uploads one the first frame it appears in and keeps the texture while it keeps
//! appearing. A picture absent from a frame is dropped from the GPU at the end of it, so a closed
//! viewer, or a PDF page scrolled out of view, gives its memory back the frame after.

use crate::frame::{Image, Layer};
use bytemuck::{Pod, Zeroable};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// The longest side a picture is given, in pixels: wgpu's guaranteed texture limit. A larger image
/// is scaled down to it when decoded, which is still more pixels than any screen it is shown on.
pub const MAX_SIDE: u32 = 8192;

/// RGBA pixels, sRGB, straight alpha, with every mip level below them.
#[derive(Debug)]
pub struct Picture {
    id: u64,
    pub width: u32,
    pub height: u32,
    levels: Vec<(u32, u32, Vec<u8>)>,
}

impl Picture {
    /// `rgba` is `width * height * 4` bytes. The mip chain is built here, which is why this is
    /// meant to run on a worker: a large photo's chain is tens of millions of additions.
    ///
    /// Panics if `rgba` is not that size or a side is zero or past [`MAX_SIDE`] — the decoder is
    /// what guarantees all three, and a picture breaking them would be a validation error on upload.
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Picture {
        assert!(width > 0 && height > 0 && width <= MAX_SIDE && height <= MAX_SIDE);
        assert_eq!(rgba.len(), (width * height * 4) as usize);
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Picture {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            width,
            height,
            levels: mips(width, height, rgba),
        }
    }

    pub fn id(&self) -> u64 {
        self.id
    }
}

/// Box-filtered halves down to one pixel, largest first. Averaged in sRGB rather than linear light,
/// which darkens fine high-contrast detail a little at small sizes — the same trade every image
/// viewer that does not convert makes, and invisible on photographs and documents.
fn mips(mut width: u32, mut height: u32, mut level: Vec<u8>) -> Vec<(u32, u32, Vec<u8>)> {
    let mut levels = Vec::new();
    loop {
        if width == 1 && height == 1 {
            levels.push((width, height, level));
            return levels;
        }
        let (half_w, half_h) = ((width / 2).max(1), (height / 2).max(1));
        let mut next = vec![0u8; (half_w * half_h * 4) as usize];
        for y in 0..half_h {
            for x in 0..half_w {
                for channel in 0..4 {
                    let mut sum = 0u32;
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let sx = (x * 2 + dx).min(width - 1);
                        let sy = (y * 2 + dy).min(height - 1);
                        sum += level[((sy * width + sx) * 4 + channel) as usize] as u32;
                    }
                    next[((y * half_w + x) * 4 + channel) as usize] = ((sum + 2) / 4) as u8;
                }
            }
        }
        levels.push((width, height, std::mem::replace(&mut level, next)));
        width = half_w;
        height = half_h;
    }
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct Instance {
    rect: [f32; 4],
    uv: [f32; 4],
    style: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct Params {
    screen: [f32; 2],
    _pad: [f32; 2],
}

const INSTANCE_ATTRIBUTES: [wgpu::VertexAttribute; 3] =
    wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4];

struct Uploaded {
    bind_group: wgpu::BindGroup,
    /// Whether this frame drew it; a texture no frame drew is dropped.
    used: bool,
}

pub struct ImagePipeline {
    pipeline: wgpu::RenderPipeline,
    params_buffer: wgpu::Buffer,
    params_group: wgpu::BindGroup,
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    textures: HashMap<u64, Uploaded>,
    instances: wgpu::Buffer,
    capacity: usize,
    /// One draw per image, in frame order, each with its own texture.
    draws: Vec<(Layer, u64, u32)>,
}

impl ImagePipeline {
    const INITIAL_CAPACITY: usize = 8;

    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("dacode image shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("image.wgsl").into()),
        });
        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dacode image params"),
            size: std::mem::size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let params_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("dacode image params layout"),
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
        let params_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("dacode image params group"),
            layout: &params_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: params_buffer.as_entire_binding(),
            }],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("dacode image texture layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("dacode image sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("dacode image pipeline layout"),
            bind_group_layouts: &[Some(&params_layout), Some(&texture_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("dacode image pipeline"),
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
            params_group,
            texture_layout,
            sampler,
            textures: HashMap::new(),
            instances: Self::alloc(device, Self::INITIAL_CAPACITY),
            capacity: Self::INITIAL_CAPACITY,
            draws: Vec::new(),
        }
    }

    fn alloc(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dacode image instances"),
            size: (capacity * std::mem::size_of::<Instance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    fn upload(&self, device: &wgpu::Device, queue: &wgpu::Queue, picture: &Picture) -> Uploaded {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("dacode picture"),
            size: wgpu::Extent3d {
                width: picture.width,
                height: picture.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: picture.levels.len() as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // sRGB, so sampling and filtering happen in linear light and the surface's own sRGB
            // encode puts it back: the colours on screen are the file's.
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (mip, (width, height, data)) in picture.levels.iter().enumerate() {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: mip as u32,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(width * 4),
                    rows_per_image: Some(*height),
                },
                wgpu::Extent3d {
                    width: *width,
                    height: *height,
                    depth_or_array_layers: 1,
                },
            );
        }
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("dacode picture group"),
            layout: &self.texture_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        Uploaded {
            bind_group,
            used: true,
        }
    }

    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        images: &[Image],
    ) {
        queue.write_buffer(
            &self.params_buffer,
            0,
            bytemuck::bytes_of(&Params {
                screen: [width as f32, height as f32],
                _pad: [0.0, 0.0],
            }),
        );
        for uploaded in self.textures.values_mut() {
            uploaded.used = false;
        }
        let mut instances = Vec::with_capacity(images.len());
        let mut draws = Vec::with_capacity(images.len());
        for layer in Layer::ALL {
            for image in images.iter().filter(|image| image.layer == layer) {
                let id = image.picture.id();
                match self.textures.get_mut(&id) {
                    Some(uploaded) => uploaded.used = true,
                    None => {
                        let uploaded = self.upload(device, queue, &image.picture);
                        self.textures.insert(id, uploaded);
                    }
                }
                draws.push((layer, id, instances.len() as u32));
                instances.push(Instance {
                    rect: [
                        image.rect.x,
                        image.rect.y,
                        image.rect.width,
                        image.rect.height,
                    ],
                    uv: image.uv,
                    style: [image.opacity, 0.0, 0.0, 0.0],
                });
            }
        }
        self.textures.retain(|_, uploaded| uploaded.used);
        if instances.len() > self.capacity {
            self.capacity = instances.len().next_power_of_two();
            self.instances = Self::alloc(device, self.capacity);
        }
        if !instances.is_empty() {
            queue.write_buffer(&self.instances, 0, bytemuck::cast_slice(&instances));
        }
        self.draws = draws;
    }

    pub fn render(&self, pass: &mut wgpu::RenderPass<'_>, layer: Layer) {
        let mut set = false;
        for (draw_layer, id, index) in &self.draws {
            if *draw_layer != layer {
                continue;
            }
            let Some(uploaded) = self.textures.get(id) else {
                continue;
            };
            if !set {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.params_group, &[]);
                pass.set_vertex_buffer(0, self.instances.slice(..));
                set = true;
            }
            pass.set_bind_group(1, &uploaded.bind_group, &[]);
            pass.draw(0..4, *index..*index + 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mip_chain_of_an_odd_rectangle_runs_down_to_one_pixel() {
        // Not square and not a power of two, which is what photographs are: each side halves on
        // its own and stops at one, or the last level is a size wgpu refuses.
        let picture = Picture::new(5, 3, vec![255; 5 * 3 * 4]);
        let sizes: Vec<(u32, u32)> = picture.levels.iter().map(|(w, h, _)| (*w, *h)).collect();
        assert_eq!(sizes, vec![(5, 3), (2, 1), (1, 1)]);
        for (width, height, data) in &picture.levels {
            assert_eq!(data.len(), (width * height * 4) as usize);
        }
    }

    #[test]
    fn every_picture_has_an_identity_of_its_own() {
        // The texture cache is keyed by it: two pictures sharing one would show each other.
        let a = Picture::new(1, 1, vec![0; 4]);
        let b = Picture::new(1, 1, vec![0; 4]);
        assert_ne!(a.id(), b.id());
    }
}
