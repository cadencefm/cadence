//! Custom `wgpu 30` + `glyphon 0.12` GPU rendering engine for Cadence.
//!
//! Renders `DisplayList` through 3 lightweight WGSL pipelines:
//! 1. Main + Selective Emissive MRT pass (`sdf_quad.wgsl` + `scope.wgsl`)
//! 2. Half-resolution 2-pass separable Gaussian phosphor bloom (`bloom.wgsl`)
//! 3. Final additive composite + razor-sharp HiDPI `glyphon` text on top

use crate::display::{CurveVertex, DisplayList, DrawCmd, QuadInstance, TextAlign};
use crate::theme::Theme;
use anyhow::{Context, Result};
use bytemuck::{Pod, Zeroable};
use glyphon::cosmic_text::{FeatureTag, FontFeatures};
use glyphon::{
    Attrs, Buffer, Cache, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache, TextArea,
    TextAtlas, TextBounds, TextRenderer, Viewport, Weight,
};
use std::collections::HashMap;

const JETBRAINS_REGULAR: &[u8] = include_bytes!("../../assets/fonts/JetBrainsMono-Regular.ttf");
const JETBRAINS_BOLD: &[u8] = include_bytes!("../../assets/fonts/JetBrainsMono-Bold.ttf");

/// Construct a hermetic `FontSystem` backed exclusively by the embedded JetBrains Mono
/// Regular (400) and Bold (700) faces — zero OS system font scanning or proportional fallback.
fn create_embedded_font_system() -> FontSystem {
    let mut db = glyphon::fontdb::Database::new();
    db.load_font_data(JETBRAINS_REGULAR.to_vec());
    db.load_font_data(JETBRAINS_BOLD.to_vec());
    db.set_monospace_family("JetBrains Mono");
    db.set_sans_serif_family("JetBrains Mono");
    db.set_serif_family("JetBrains Mono");
    FontSystem::new_with_locale_and_db("en-US".to_string(), db)
}

/// Disable programming ligatures (`calt`/`liga`/`clig`/`dlig`) so keybinding hints like
/// `-/=`, `!=`, `->`, and `</>` always shape into literal separate monospace glyphs.
fn no_ligatures_features() -> FontFeatures {
    let mut features = FontFeatures::new();
    features
        .disable(FeatureTag::CONTEXTUAL_ALTERNATES)
        .disable(FeatureTag::STANDARD_LIGATURES)
        .disable(FeatureTag::CONTEXTUAL_LIGATURES)
        .disable(FeatureTag::DISCRETIONARY_LIGATURES);
    features
}

const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// Maximum unused frame age before a cached shaped `glyphon::Buffer` is evicted.
const TEXT_CACHE_EVICT_AGE: u64 = 120;
/// Periodic frame interval between cache sweeps.
const TEXT_CACHE_TRIM_INTERVAL: u64 = 60;
/// Soft cap on total cached text buffers before triggering an immediate aggressive sweep.
const TEXT_CACHE_SOFT_CAP: usize = 2048;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct TextStyleKey {
    family: &'static str,
    font_px_bits: u32,
    width_px_bits: u32,
    height_px_bits: u32,
    bold: bool,
    align: TextAlign,
}

struct CachedTextBuffer {
    buffer: Buffer,
    last_used_frame: u64,
}

/// Generation-swept cache of shaped `glyphon::Buffer`s so static labels, meter
/// graticules, and repeated badges are shaped once instead of every frame.
struct TextBufferCache {
    buckets: HashMap<TextStyleKey, HashMap<String, CachedTextBuffer>>,
    frame: u64,
    total_entries: usize,
    #[cfg(test)]
    shape_count: u64,
}

impl TextBufferCache {
    fn new() -> Self {
        Self {
            buckets: HashMap::with_capacity(64),
            frame: 0,
            total_entries: 0,
            #[cfg(test)]
            shape_count: 0,
        }
    }

    fn begin_frame(&mut self) {
        self.frame = self.frame.wrapping_add(1);
    }

    fn style_key(
        run: &crate::display::TextRun,
        scale: f32,
        family: &'static str,
    ) -> (TextStyleKey, f32, f32, f32, f32) {
        let font_px = (run.size * scale).max(6.0);
        let line_px = (font_px * 1.28).ceil();
        let max_w = (run.rect.w * scale).max(1.0);
        let max_h = (run.rect.h * scale).max(line_px);
        let key = TextStyleKey {
            family,
            font_px_bits: font_px.to_bits(),
            width_px_bits: max_w.to_bits(),
            height_px_bits: max_h.to_bits(),
            bold: run.bold,
            align: run.align,
        };
        (key, font_px, line_px, max_w, max_h)
    }

    fn ensure_shaped(
        &mut self,
        font_system: &mut FontSystem,
        run: &crate::display::TextRun,
        scale: f32,
        family: &'static str,
    ) -> TextStyleKey {
        let (key, font_px, line_px, max_w, max_h) = Self::style_key(run, scale, family);
        let bucket = self.buckets.entry(key).or_default();
        if let Some(entry) = bucket.get_mut(run.text.as_str()) {
            entry.last_used_frame = self.frame;
            return key;
        }
        let mut buffer = Buffer::new(font_system, Metrics::new(font_px, line_px));
        buffer.set_size(Some(max_w), Some(max_h));
        let mut attrs = Attrs::new()
            .family(Family::Name(family))
            .font_features(no_ligatures_features());
        if run.bold {
            attrs = attrs.weight(Weight::BOLD);
        }
        let align = match run.align {
            TextAlign::Left => Some(glyphon::cosmic_text::Align::Left),
            TextAlign::Center => Some(glyphon::cosmic_text::Align::Center),
            TextAlign::Right => Some(glyphon::cosmic_text::Align::Right),
        };
        buffer.set_text(&run.text, &attrs, Shaping::Advanced, align);
        buffer.shape_until_scroll(font_system, false);
        bucket.insert(
            run.text.clone(),
            CachedTextBuffer {
                buffer,
                last_used_frame: self.frame,
            },
        );
        self.total_entries += 1;
        #[cfg(test)]
        {
            self.shape_count += 1;
        }
        key
    }

    fn get(&self, key: &TextStyleKey, text: &str) -> Option<&Buffer> {
        self.buckets
            .get(key)
            .and_then(|bucket| bucket.get(text))
            .map(|entry| &entry.buffer)
    }

    fn trim_periodic(&mut self) {
        if self.total_entries > TEXT_CACHE_SOFT_CAP {
            self.trim_older_than(1);
        } else if self.frame.is_multiple_of(TEXT_CACHE_TRIM_INTERVAL) {
            self.trim_older_than(TEXT_CACHE_EVICT_AGE);
        }
    }

    fn trim_older_than(&mut self, max_age: u64) {
        let frame = self.frame;
        let mut kept = 0usize;
        self.buckets.retain(|_, bucket| {
            bucket.retain(|_, entry| frame.wrapping_sub(entry.last_used_frame) <= max_age);
            kept += bucket.len();
            !bucket.is_empty()
        });
        self.total_entries = kept;
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GlobalsUniform {
    viewport: [f32; 2],
    _pad: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct BlurParamsUniform {
    texel_dir: [f32; 2],
    strength: f32,
    _pad: f32,
}

struct RenderTargets {
    bloom_width: u32,
    bloom_height: u32,
    _main_tex: wgpu::Texture,
    main_view: wgpu::TextureView,
    _emissive_tex: wgpu::Texture,
    emissive_view: wgpu::TextureView,
    _bloom_a_tex: wgpu::Texture,
    bloom_a_view: wgpu::TextureView,
    _bloom_b_tex: wgpu::Texture,
    bloom_b_view: wgpu::TextureView,
    blur_h_bg: wgpu::BindGroup,
    blur_v_bg: wgpu::BindGroup,
    composite_main_bg: wgpu::BindGroup,
    composite_bloom_bg: wgpu::BindGroup,
}

struct BloomLayouts {
    sampler: wgpu::Sampler,
    blur_bgl: wgpu::BindGroupLayout,
    bloom_tex_bgl: wgpu::BindGroupLayout,
    blur_h_buf: wgpu::Buffer,
    blur_v_buf: wgpu::Buffer,
    composite_buf: wgpu::Buffer,
}

struct LayerResources {
    label: &'static str,
    quad_pipeline: wgpu::RenderPipeline,
    scope_pipeline: wgpu::RenderPipeline,
    quad_buf: wgpu::Buffer,
    quad_cap: usize,
    curve_buf: wgpu::Buffer,
    curve_cap: usize,
}

#[derive(Clone, Copy)]
enum TextLayerKind {
    Base,
    Overlay,
}

pub struct GpuRenderer {
    width: u32,
    height: u32,
    globals_buf: wgpu::Buffer,
    globals_bg: wgpu::BindGroup,
    bloom: BloomLayouts,
    blur_pipeline: wgpu::RenderPipeline,
    composite_pipeline: wgpu::RenderPipeline,
    base_layer: LayerResources,
    overlay_layer: LayerResources,
    targets: RenderTargets,
    font_system: FontSystem,
    swash_cache: SwashCache,
    viewport: Viewport,
    atlas: TextAtlas,
    text_renderer: TextRenderer,
    overlay_text_renderer: TextRenderer,
    text_cache: TextBufferCache,
    text_keys_scratch: Vec<TextStyleKey>,
}

impl GpuRenderer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> Self {
        let width = width.max(1);
        let height = height.max(1);

        let globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cadence.globals"),
            size: std::mem::size_of::<GlobalsUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cadence.globals.bgl"),
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
        let globals_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cadence.globals.bg"),
            layout: &globals_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals_buf.as_entire_binding(),
            }],
        });

        let blur_h_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cadence.blur_h"),
            size: std::mem::size_of::<BlurParamsUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let blur_v_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cadence.blur_v"),
            size: std::mem::size_of::<BlurParamsUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let composite_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cadence.composite"),
            size: std::mem::size_of::<BlurParamsUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("cadence.linear_sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });

        let blur_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cadence.blur.bgl"),
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
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let bloom_tex_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cadence.bloom_tex.bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });

        let quad_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cadence.sdf_quad"),
            source: wgpu::ShaderSource::Wgsl(include_str!("sdf_quad.wgsl").into()),
        });
        let scope_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cadence.scope"),
            source: wgpu::ShaderSource::Wgsl(include_str!("scope.wgsl").into()),
        });
        let bloom_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cadence.bloom"),
            source: wgpu::ShaderSource::Wgsl(include_str!("bloom.wgsl").into()),
        });

        let geom_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cadence.geom.layout"),
            bind_group_layouts: &[Some(&globals_bgl)],
            immediate_size: 0,
        });

        let main_blend = wgpu::BlendState::ALPHA_BLENDING;
        // Premultiplied Porter-Duff OVER on the emissive target preserves Z-order occlusion:
        // opaque non-emissive cards or dimming rectangles cleanly occlude glow from curves behind them.
        let emissive_blend = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::OVER,
        };
        let mrt_targets = [
            Some(wgpu::ColorTargetState {
                format: HDR_FORMAT,
                blend: Some(main_blend),
                write_mask: wgpu::ColorWrites::ALL,
            }),
            Some(wgpu::ColorTargetState {
                format: HDR_FORMAT,
                blend: Some(emissive_blend),
                write_mask: wgpu::ColorWrites::ALL,
            }),
        ];

        let quad_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("cadence.sdf_quad.pipeline"),
            layout: Some(&geom_layout),
            vertex: wgpu::VertexState {
                module: &quad_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<QuadInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x4,
                        1 => Float32x4,
                        2 => Float32x4,
                        3 => Float32x4,
                        4 => Float32x4,
                    ],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &quad_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &mrt_targets,
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let scope_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("cadence.scope.pipeline"),
            layout: Some(&geom_layout),
            vertex: wgpu::VertexState {
                module: &scope_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<CurveVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x2,
                        1 => Float32,
                        2 => Float32,
                        3 => Float32x4,
                        4 => Float32,
                    ],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &scope_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &mrt_targets,
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let overlay_target = [Some(wgpu::ColorTargetState {
            format: surface_format,
            blend: Some(main_blend),
            write_mask: wgpu::ColorWrites::ALL,
        })];
        let overlay_quad_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("cadence.overlay_quad.pipeline"),
                layout: Some(&geom_layout),
                vertex: wgpu::VertexState {
                    module: &quad_shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<QuadInstance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &wgpu::vertex_attr_array![
                            0 => Float32x4,
                            1 => Float32x4,
                            2 => Float32x4,
                            3 => Float32x4,
                            4 => Float32x4,
                        ],
                    })],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &quad_shader,
                    entry_point: Some("fs_overlay"),
                    compilation_options: Default::default(),
                    targets: &overlay_target,
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            });
        let overlay_scope_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("cadence.overlay_scope.pipeline"),
                layout: Some(&geom_layout),
                vertex: wgpu::VertexState {
                    module: &scope_shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<CurveVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![
                            0 => Float32x2,
                            1 => Float32,
                            2 => Float32,
                            3 => Float32x4,
                            4 => Float32,
                        ],
                    })],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &scope_shader,
                    entry_point: Some("fs_overlay"),
                    compilation_options: Default::default(),
                    targets: &overlay_target,
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            });

        let blur_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cadence.blur.layout"),
            bind_group_layouts: &[Some(&blur_bgl)],
            immediate_size: 0,
        });
        let blur_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("cadence.blur.pipeline"),
            layout: Some(&blur_layout),
            vertex: wgpu::VertexState {
                module: &bloom_shader,
                entry_point: Some("vs_fullscreen"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &bloom_shader,
                entry_point: Some("fs_blur"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: HDR_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let composite_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cadence.composite.layout"),
            bind_group_layouts: &[Some(&blur_bgl), Some(&bloom_tex_bgl)],
            immediate_size: 0,
        });
        let composite_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("cadence.composite.pipeline"),
            layout: Some(&composite_layout),
            vertex: wgpu::VertexState {
                module: &bloom_shader,
                entry_point: Some("vs_fullscreen"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &bloom_shader,
                entry_point: Some("fs_composite"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let base_layer = LayerResources::new(
            device,
            "cadence.base",
            quad_pipeline,
            scope_pipeline,
            4096,
            16384,
        );
        let overlay_layer = LayerResources::new(
            device,
            "cadence.overlay",
            overlay_quad_pipeline,
            overlay_scope_pipeline,
            512,
            2048,
        );

        let bloom = BloomLayouts {
            sampler,
            blur_bgl,
            bloom_tex_bgl,
            blur_h_buf,
            blur_v_buf,
            composite_buf,
        };
        let targets = bloom.create_targets(device, width, height);

        let font_system = create_embedded_font_system();

        let swash_cache = SwashCache::new();
        let cache = Cache::new(device);
        let viewport = Viewport::new(device, &cache);
        let mut atlas = TextAtlas::new(device, queue, &cache, surface_format);
        let text_renderer =
            TextRenderer::new(&mut atlas, device, wgpu::MultisampleState::default(), None);
        let overlay_text_renderer =
            TextRenderer::new(&mut atlas, device, wgpu::MultisampleState::default(), None);

        Self {
            width,
            height,
            globals_buf,
            globals_bg,
            bloom,
            blur_pipeline,
            composite_pipeline,
            base_layer,
            overlay_layer,
            targets,
            font_system,
            swash_cache,
            viewport,
            atlas,
            text_renderer,
            overlay_text_renderer,
            text_cache: TextBufferCache::new(),
            text_keys_scratch: Vec::with_capacity(512),
        }
    }

    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        if self.width == width && self.height == height {
            return;
        }
        self.width = width;
        self.height = height;
        self.targets = self.bloom.create_targets(device, width, height);
    }

    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target_view: &wgpu::TextureView,
        list: &DisplayList,
        theme: &Theme,
    ) -> Result<()> {
        let w = self.width as f32;
        let h = self.height as f32;

        // 1. Upload uniforms.
        let globals = GlobalsUniform {
            viewport: [w, h],
            _pad: [0.0; 2],
        };
        queue.write_buffer(&self.globals_buf, 0, bytemuck::bytes_of(&globals));

        let bloom_active = list.base.emissive_count > 0 && theme.glow.bloom_strength > 0.0;
        if bloom_active {
            let bw = self.targets.bloom_width.max(1) as f32;
            let bh = self.targets.bloom_height.max(1) as f32;
            let blur_h = BlurParamsUniform {
                texel_dir: [1.35 / bw, 0.0],
                strength: 1.0,
                _pad: 0.0,
            };
            let blur_v = BlurParamsUniform {
                texel_dir: [0.0, 1.35 / bh],
                strength: 1.0,
                _pad: 0.0,
            };
            queue.write_buffer(&self.bloom.blur_h_buf, 0, bytemuck::bytes_of(&blur_h));
            queue.write_buffer(&self.bloom.blur_v_buf, 0, bytemuck::bytes_of(&blur_v));
        }
        let composite = BlurParamsUniform {
            texel_dir: [0.0, 0.0],
            strength: if bloom_active {
                theme.glow.bloom_strength
            } else {
                0.0
            },
            _pad: 0.0,
        };
        queue.write_buffer(&self.bloom.composite_buf, 0, bytemuck::bytes_of(&composite));

        // 2. Upload base + overlay geometry buffers (growing dynamically if needed).
        self.base_layer.upload(device, queue, &list.base);
        let has_overlay = !list.overlay.is_empty();
        if has_overlay {
            self.overlay_layer.upload(device, queue, &list.overlay);
        }

        // 3. Prepare cached glyphon text buffers for both base and overlay layers.
        self.text_cache.begin_frame();
        self.viewport.update(
            queue,
            Resolution {
                width: self.width,
                height: self.height,
            },
        );
        self.prepare_text_layer(device, queue, list, TextLayerKind::Base, theme)?;
        if has_overlay {
            self.prepare_text_layer(device, queue, list, TextLayerKind::Overlay, theme)?;
        }

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("cadence.frame_encoder"),
        });

        // Pass 1: Base Main + Emissive MRT geometry pass in exact painter's order.
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("cadence.pass_main_mrt"),
                color_attachments: &[
                    Some(wgpu::RenderPassColorAttachment {
                        view: &self.targets.main_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(theme.surface.canvas.to_wgpu_clear()),
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                    Some(wgpu::RenderPassColorAttachment {
                        view: &self.targets.emissive_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: if bloom_active {
                                wgpu::StoreOp::Store
                            } else {
                                wgpu::StoreOp::Discard
                            },
                        },
                    }),
                ],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });
            self.base_layer.draw(
                &mut pass,
                &list.base,
                &self.globals_bg,
                (self.width, self.height),
            );
        }

        // Pass 2: Skip half-res horizontal + vertical phosphor blur completely when zero bloom.
        if bloom_active {
            // Pass 2a: Half-res horizontal phosphor blur (emissive -> bloom_a).
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("cadence.pass_bloom_h"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.targets.bloom_a_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    occlusion_query_set: None,
                    timestamp_writes: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(&self.blur_pipeline);
                pass.set_bind_group(0, &self.targets.blur_h_bg, &[]);
                pass.draw(0..3, 0..1);
            }

            // Pass 2b: Half-res vertical phosphor blur (bloom_a -> bloom_b).
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("cadence.pass_bloom_v"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.targets.bloom_b_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    occlusion_query_set: None,
                    timestamp_writes: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(&self.blur_pipeline);
                pass.set_bind_group(0, &self.targets.blur_v_bg, &[]);
                pass.draw(0..3, 0..1);
            }
        }

        // Pass 3: Additive composite onto swapchain + crisp base glyphon text on top.
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("cadence.pass_composite_and_text"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(theme.surface.canvas.to_wgpu_clear()),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.composite_pipeline);
            pass.set_bind_group(0, &self.targets.composite_main_bg, &[]);
            pass.set_bind_group(1, &self.targets.composite_bloom_bg, &[]);
            pass.draw(0..3, 0..1);

            self.text_renderer
                .render(&self.atlas, &self.viewport, &mut pass)
                .context("Rendering base glyphon text")?;

            // Pass 4 (in-place): Modal overlays render strictly on top of base decks & base text!
            if has_overlay {
                self.overlay_layer.draw(
                    &mut pass,
                    &list.overlay,
                    &self.globals_bg,
                    (self.width, self.height),
                );
                pass.set_scissor_rect(0, 0, self.width, self.height);
                self.overlay_text_renderer
                    .render(&self.atlas, &self.viewport, &mut pass)
                    .context("Rendering overlay glyphon text")?;
            }
        }

        queue.submit(Some(encoder.finish()));
        self.atlas.trim();
        self.text_cache.trim_periodic();
        Ok(())
    }

    fn prepare_text_layer(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        list: &DisplayList,
        kind: TextLayerKind,
        theme: &Theme,
    ) -> Result<()> {
        let (texts, renderer) = match kind {
            TextLayerKind::Base => (&list.base.texts[..], &mut self.text_renderer),
            TextLayerKind::Overlay => (&list.overlay.texts[..], &mut self.overlay_text_renderer),
        };
        let scale = list.scale;
        let w = self.width as f32;
        let h = self.height as f32;

        self.text_keys_scratch.clear();
        self.text_keys_scratch.reserve(texts.len());
        for run in texts {
            let key =
                self.text_cache
                    .ensure_shaped(&mut self.font_system, run, scale, theme.font.family);
            self.text_keys_scratch.push(key);
        }

        let areas: Vec<TextArea<'_>> = texts
            .iter()
            .zip(self.text_keys_scratch.iter())
            .filter_map(|(run, key)| {
                let buffer = self.text_cache.get(key, run.text.as_str())?;
                let left = (run.rect.x * scale).round();
                let line_h = (run.size * scale * 1.28).ceil();
                let top =
                    (run.rect.y * scale + ((run.rect.h * scale - line_h).max(0.0) * 0.5)).floor();
                let clip_l = (run.clip.x * scale).floor().clamp(0.0, w) as i32;
                let clip_t = (run.clip.y * scale).floor().clamp(0.0, h) as i32;
                let clip_r = (run.clip.right() * scale).ceil().clamp(0.0, w) as i32;
                let clip_b = (run.clip.bottom() * scale).ceil().clamp(0.0, h) as i32;
                Some(TextArea {
                    buffer,
                    left,
                    top,
                    scale: 1.0,
                    bounds: TextBounds {
                        left: clip_l,
                        top: clip_t,
                        right: clip_r,
                        bottom: clip_b,
                    },
                    default_color: run.color.to_glyphon(),
                    custom_glyphs: &[],
                })
            })
            .collect();

        renderer
            .prepare(
                device,
                queue,
                &mut self.font_system,
                &mut self.atlas,
                &self.viewport,
                areas,
                &mut self.swash_cache,
            )
            .context("Preparing glyphon text buffers")
    }

    /// Return the first character in `text` that shapes to a `.notdef` (`glyph_id == 0`)
    /// glyph under the embedded font system, or `None` when every glyph is natively covered.
    pub fn first_missing_glyph(&mut self, text: &str, bold: bool, family: &str) -> Option<char> {
        let mut buffer = Buffer::new(&mut self.font_system, Metrics::new(12.0, 16.0));
        buffer.set_size(Some(4096.0), Some(64.0));
        let mut attrs = Attrs::new()
            .family(Family::Name(family))
            .font_features(no_ligatures_features());
        if bold {
            attrs = attrs.weight(Weight::BOLD);
        }
        buffer.set_text(text, &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.font_system, false);
        for run in buffer.layout_runs() {
            for glyph in run.glyphs {
                if glyph.glyph_id == 0 {
                    return text[glyph.start..glyph.end].chars().next();
                }
            }
        }
        None
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ActivePipeline {
    None,
    Quads,
    Curves,
}

impl LayerResources {
    fn new(
        device: &wgpu::Device,
        label: &'static str,
        quad_pipeline: wgpu::RenderPipeline,
        scope_pipeline: wgpu::RenderPipeline,
        quad_cap: usize,
        curve_cap: usize,
    ) -> Self {
        let quad_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: (quad_cap * std::mem::size_of::<QuadInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let curve_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: (curve_cap * std::mem::size_of::<CurveVertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            label,
            quad_pipeline,
            scope_pipeline,
            quad_buf,
            quad_cap,
            curve_buf,
            curve_cap,
        }
    }

    fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layer: &crate::display::DisplayLayer,
    ) {
        if layer.quads.len() > self.quad_cap {
            self.quad_cap = layer.quads.len().next_power_of_two();
            self.quad_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(self.label),
                size: (self.quad_cap * std::mem::size_of::<QuadInstance>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !layer.quads.is_empty() {
            queue.write_buffer(&self.quad_buf, 0, bytemuck::cast_slice(&layer.quads));
        }
        if layer.curve_vertices.len() > self.curve_cap {
            self.curve_cap = layer.curve_vertices.len().next_power_of_two();
            self.curve_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(self.label),
                size: (self.curve_cap * std::mem::size_of::<CurveVertex>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !layer.curve_vertices.is_empty() {
            queue.write_buffer(
                &self.curve_buf,
                0,
                bytemuck::cast_slice(&layer.curve_vertices),
            );
        }
    }

    fn draw<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        layer: &crate::display::DisplayLayer,
        globals_bg: &'a wgpu::BindGroup,
        (width, height): (u32, u32),
    ) {
        if layer.commands.is_empty() {
            return;
        }
        let mut active = ActivePipeline::None;
        let mut last_scissor: Option<(u32, u32, u32, u32)> = None;

        for cmd in &layer.commands {
            match *cmd {
                DrawCmd::Quads(batch) => {
                    let Some(scissor) = scissor_rect(batch.clip, width, height) else {
                        continue;
                    };
                    if active != ActivePipeline::Quads {
                        pass.set_pipeline(&self.quad_pipeline);
                        pass.set_bind_group(0, globals_bg, &[]);
                        pass.set_vertex_buffer(0, self.quad_buf.slice(..));
                        active = ActivePipeline::Quads;
                    }
                    if last_scissor != Some(scissor) {
                        pass.set_scissor_rect(scissor.0, scissor.1, scissor.2, scissor.3);
                        last_scissor = Some(scissor);
                    }
                    pass.draw(0..6, batch.start..batch.start + batch.count);
                }
                DrawCmd::Curves(batch) => {
                    let Some(scissor) = scissor_rect(batch.clip, width, height) else {
                        continue;
                    };
                    if active != ActivePipeline::Curves {
                        pass.set_pipeline(&self.scope_pipeline);
                        pass.set_bind_group(0, globals_bg, &[]);
                        pass.set_vertex_buffer(0, self.curve_buf.slice(..));
                        active = ActivePipeline::Curves;
                    }
                    if last_scissor != Some(scissor) {
                        pass.set_scissor_rect(scissor.0, scissor.1, scissor.2, scissor.3);
                        last_scissor = Some(scissor);
                    }
                    pass.draw(batch.start..batch.start + batch.count, 0..1);
                }
            }
        }
    }
}

fn scissor_rect(
    clip: crate::display::Rect,
    max_w: u32,
    max_h: u32,
) -> Option<(u32, u32, u32, u32)> {
    let x0 = clip.x.floor().max(0.0) as u32;
    let y0 = clip.y.floor().max(0.0) as u32;
    let x1 = clip.right().ceil().min(max_w as f32).max(0.0) as u32;
    let y1 = clip.bottom().ceil().min(max_h as f32).max(0.0) as u32;
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some((x0, y0, x1 - x0, y1 - y0))
}

impl BloomLayouts {
    fn create_targets(&self, device: &wgpu::Device, width: u32, height: u32) -> RenderTargets {
        let bloom_width = (width / 2).max(1);
        let bloom_height = (height / 2).max(1);
        let make_tex = |label, w, h| {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: HDR_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
            (tex, view)
        };

        let (main_tex, main_view) = make_tex("cadence.main_hdr", width, height);
        let (emissive_tex, emissive_view) = make_tex("cadence.emissive_hdr", width, height);
        let (bloom_a_tex, bloom_a_view) = make_tex("cadence.bloom_a", bloom_width, bloom_height);
        let (bloom_b_tex, bloom_b_view) = make_tex("cadence.bloom_b", bloom_width, bloom_height);

        let blur_h_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cadence.blur_h.bg"),
            layout: &self.blur_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&emissive_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.blur_h_buf.as_entire_binding(),
                },
            ],
        });
        let blur_v_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cadence.blur_v.bg"),
            layout: &self.blur_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&bloom_a_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.blur_v_buf.as_entire_binding(),
                },
            ],
        });
        let composite_main_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cadence.composite_main.bg"),
            layout: &self.blur_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&main_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.composite_buf.as_entire_binding(),
                },
            ],
        });
        let composite_bloom_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cadence.composite_bloom.bg"),
            layout: &self.bloom_tex_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&bloom_b_view),
            }],
        });

        RenderTargets {
            bloom_width,
            bloom_height,
            _main_tex: main_tex,
            main_view,
            _emissive_tex: emissive_tex,
            emissive_view,
            _bloom_a_tex: bloom_a_tex,
            bloom_a_view,
            _bloom_b_tex: bloom_b_tex,
            bloom_b_view,
            blur_h_bg,
            blur_v_bg,
            composite_main_bg,
            composite_bloom_bg,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::{Rect, TextRun};

    fn make_run(text: &str, size: f32, bold: bool, align: TextAlign, w: f32) -> TextRun {
        let theme = Theme::cadence_dark();
        let rect = Rect::new(0.0, 0.0, w, 20.0);
        TextRun {
            rect,
            text: text.into(),
            size,
            bold,
            color: theme.text.primary,
            align,
            clip: rect,
        }
    }

    #[test]
    fn text_buffer_cache_reuses_shaped_buffers_and_evicts_stale_entries() {
        let mut font_system = create_embedded_font_system();
        let theme = Theme::cadence_dark();
        let mut cache = TextBufferCache::new();

        let static_run = make_run("MIXER", 12.0, true, TextAlign::Left, 100.0);
        let alt_bold = make_run("MIXER", 12.0, false, TextAlign::Left, 100.0);
        let ephemeral = make_run("001.1.1", 12.0, false, TextAlign::Right, 80.0);

        // Frame 1: cold misses shape each distinct (style, text) once.
        cache.begin_frame();
        let k_static = cache.ensure_shaped(&mut font_system, &static_run, 2.0, theme.font.family);
        let k_dup = cache.ensure_shaped(&mut font_system, &static_run, 2.0, theme.font.family);
        let k_alt = cache.ensure_shaped(&mut font_system, &alt_bold, 2.0, theme.font.family);
        let k_eph = cache.ensure_shaped(&mut font_system, &ephemeral, 2.0, theme.font.family);
        assert_eq!(k_static, k_dup);
        assert_ne!(k_static, k_alt);
        assert_eq!(cache.shape_count, 3);
        assert_eq!(cache.total_entries, 3);
        assert!(cache.get(&k_static, "MIXER").is_some());

        // Advance past eviction horizon keeping only `static_run` alive.
        for _ in 0..(TEXT_CACHE_EVICT_AGE + TEXT_CACHE_TRIM_INTERVAL) {
            cache.begin_frame();
            cache.ensure_shaped(&mut font_system, &static_run, 2.0, theme.font.family);
            cache.trim_periodic();
        }
        assert_eq!(
            cache.shape_count, 3,
            "static label must never be re-shaped across frames"
        );
        assert_eq!(cache.total_entries, 1, "unused entries must be trimmed");
        assert!(cache.get(&k_static, "MIXER").is_some());
        assert!(cache.get(&k_eph, "001.1.1").is_none());
    }

    fn collect_glyphs(buffer: &Buffer) -> Vec<&glyphon::LayoutGlyph> {
        buffer
            .layout_runs()
            .flat_map(|run| run.glyphs.iter())
            .collect()
    }

    #[test]
    fn embedded_font_system_loads_only_jetbrains_regular_and_bold() {
        let mut font_system = create_embedded_font_system();
        let faces: Vec<_> = font_system.db().faces().collect();
        assert_eq!(
            faces.len(),
            2,
            "hermetic FontSystem must contain ONLY embedded Regular and Bold faces, got {}",
            faces.len()
        );
        let regular_face = faces
            .iter()
            .find(|f| f.weight == Weight::NORMAL)
            .expect("embedded JetBrains Mono Regular (400) face missing");
        let bold_face = faces
            .iter()
            .find(|f| f.weight == Weight::BOLD)
            .expect("embedded JetBrains Mono Bold (700) face missing");
        assert_ne!(regular_face.id, bold_face.id);
        assert!(regular_face.monospaced && bold_face.monospaced);
        assert!(regular_face
            .families
            .iter()
            .any(|(name, _)| name == "JetBrains Mono"));
        assert!(bold_face
            .families
            .iter()
            .any(|(name, _)| name == "JetBrains Mono"));

        let regular_id = regular_face.id;
        let bold_id = bold_face.id;
        let theme = Theme::cadence_dark();
        let mut cache = TextBufferCache::new();
        cache.begin_frame();

        let reg_run = make_run("Cutoff", 13.0, false, TextAlign::Left, 200.0);
        let bold_run = make_run("Cutoff", 13.0, true, TextAlign::Left, 200.0);
        let k_reg = cache.ensure_shaped(&mut font_system, &reg_run, 1.0, theme.font.family);
        let k_bold = cache.ensure_shaped(&mut font_system, &bold_run, 1.0, theme.font.family);

        let reg_glyphs = collect_glyphs(cache.get(&k_reg, "Cutoff").unwrap());
        let bold_glyphs = collect_glyphs(cache.get(&k_bold, "Cutoff").unwrap());
        assert_eq!(reg_glyphs.len(), 6);
        assert_eq!(bold_glyphs.len(), 6);
        assert!(
            reg_glyphs.iter().all(|g| g.font_id == regular_id),
            "bold:false must shape strictly with embedded JetBrains Mono Regular"
        );
        assert!(
            bold_glyphs.iter().all(|g| g.font_id == bold_id),
            "bold:true must shape strictly with embedded JetBrains Mono Bold (zero OS fallback)"
        );
    }

    #[test]
    fn regular_and_bold_have_identical_monospace_advance_widths() {
        let mut font_system = create_embedded_font_system();
        let theme = Theme::cadence_dark();
        let mut cache = TextBufferCache::new();
        cache.begin_frame();

        let equal_len_pairs = [
            ("Cutoff", "Reson."),
            ("3.60 kHz", "128.0BPM"),
            ("Anthem Lead", "Warm Sub 01"),
            ("0123456789ABCDEF", "h/l ,/. -/= fine"),
        ];

        for scale in [1.0_f32, 2.0_f32] {
            for size in [theme.font.small, theme.font.body, theme.font.title] {
                for (a, b) in equal_len_pairs {
                    assert_eq!(a.chars().count(), b.chars().count());
                    let run_reg_a = make_run(a, size, false, TextAlign::Left, 600.0);
                    let run_bold_a = make_run(a, size, true, TextAlign::Left, 600.0);
                    let run_bold_b = make_run(b, size, true, TextAlign::Left, 600.0);

                    let ka =
                        cache.ensure_shaped(&mut font_system, &run_reg_a, scale, theme.font.family);
                    let kb = cache.ensure_shaped(
                        &mut font_system,
                        &run_bold_a,
                        scale,
                        theme.font.family,
                    );
                    let kc = cache.ensure_shaped(
                        &mut font_system,
                        &run_bold_b,
                        scale,
                        theme.font.family,
                    );

                    let w_reg_a = cache
                        .get(&ka, a)
                        .unwrap()
                        .layout_runs()
                        .next()
                        .unwrap()
                        .line_w;
                    let w_bold_a = cache
                        .get(&kb, a)
                        .unwrap()
                        .layout_runs()
                        .next()
                        .unwrap()
                        .line_w;
                    let w_bold_b = cache
                        .get(&kc, b)
                        .unwrap()
                        .layout_runs()
                        .next()
                        .unwrap()
                        .line_w;

                    assert!(
                        (w_reg_a - w_bold_a).abs() <= 0.1,
                        "Regular vs Bold width drift for {a:?} at {size}@{scale}x: {w_reg_a} vs {w_bold_a}"
                    );
                    assert!(
                        (w_bold_a - w_bold_b).abs() <= 0.1,
                        "Equal-length monospace Bold width drift for {a:?} vs {b:?} at {size}@{scale}x: {w_bold_a} vs {w_bold_b}"
                    );
                }
            }
        }
    }

    #[test]
    fn coding_ligatures_disabled_while_unicode_symbols_render_crisply() {
        let mut font_system = create_embedded_font_system();
        let theme = Theme::cadence_dark();
        let mut cache = TextBufferCache::new();
        cache.begin_frame();

        // 1. Keybinding hint `-/=` must shape into 3 separate literal glyphs (NOT `≠` ligature).
        for bold in [false, true] {
            let run = make_run("-/=", 12.0, bold, TextAlign::Left, 120.0);
            let k = cache.ensure_shaped(&mut font_system, &run, 1.0, theme.font.family);
            let glyphs = collect_glyphs(cache.get(&k, "-/=").unwrap());
            assert_eq!(
                glyphs.len(),
                3,
                "'-/=' (bold={bold}) must shape into 3 separate glyphs, got {}",
                glyphs.len()
            );
            assert!(
                glyphs.iter().all(|g| g.glyph_id != 0 && g.w > 0.0),
                "every glyph in '-/=' must be non-empty with positive advance"
            );
            let actual_ids: Vec<u16> = glyphs.iter().map(|g| g.glyph_id).collect();

            // Verify glyph IDs match standalone '-', '/', '=' (proving zero calt/liga substitution).
            let expected_ids: Vec<u16> = ["-", "/", "="]
                .into_iter()
                .map(|ch| {
                    let r = make_run(ch, 12.0, bold, TextAlign::Left, 40.0);
                    let kc = cache.ensure_shaped(&mut font_system, &r, 1.0, theme.font.family);
                    collect_glyphs(cache.get(&kc, ch).unwrap())[0].glyph_id
                })
                .collect();
            assert_eq!(
                actual_ids, expected_ids,
                "'-/=' (bold={bold}) glyph IDs must match literal '-', '/', '='"
            );
        }

        // 2. Other common JetBrains Mono ligature pairs must also stay un-ligated.
        for sample in ["!=", "->", "=>", "<=", ">=", "</>", "..."] {
            let run = make_run(sample, 12.0, false, TextAlign::Left, 160.0);
            let k = cache.ensure_shaped(&mut font_system, &run, 1.0, theme.font.family);
            let glyphs = collect_glyphs(cache.get(&k, sample).unwrap());
            assert_eq!(
                glyphs.len(),
                sample.chars().count(),
                "{sample:?} must not collapse into a ligature"
            );
        }

        // 3. Every Unicode symbol used across Cadence UI shapes directly to a real non-zero glyph.
        let symbols = "·→↪›▸●▏⌥✓–—…←↓−≤≥■□▲▶▼◀◆◇◈◉♭♯°±Δ∥↩↔∾✶";
        for bold in [false, true] {
            let run = make_run(symbols, 12.0, bold, TextAlign::Left, 800.0);
            let k = cache.ensure_shaped(&mut font_system, &run, 1.0, theme.font.family);
            let glyphs = collect_glyphs(cache.get(&k, symbols).unwrap());
            assert_eq!(glyphs.len(), symbols.chars().count());
            for (ch, g) in symbols.chars().zip(glyphs.iter()) {
                assert_ne!(
                    g.glyph_id, 0,
                    "Unicode symbol {ch:?} (U+{:04X}, bold={bold}) produced missing .notdef glyph",
                    ch as u32
                );
            }
        }
    }
}
