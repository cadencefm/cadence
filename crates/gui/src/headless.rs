//! Offscreen `wgpu` GPU rendering harness for automated verification across every deck.
//!
//! Renders real `FrameView` scenes through the exact same `sdf_quad.wgsl`, `scope.wgsl`,
//! `bloom.wgsl`, and `glyphon` pipelines used by the live window, reading back RGBA8 pixels
//! to assert zero WGSL/validation errors and non-empty rasterized output.
use crate::decks::populate_display_list;
use crate::display::DisplayList;
use crate::gpu::GpuRenderer;
use crate::theme::{Color, Theme};
use anyhow::{Context, Result};
use cadence_app::view::FrameView;

const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// Fraction of RGBA8 pixels whose RGB differs from `canvas` by more than 4/255.
pub fn distinct_pixel_ratio(rgba: &[u8], canvas: Color) -> f32 {
    let total = rgba.len() / 4;
    if total == 0 {
        return 0.0;
    }
    let non_bg = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|px| {
            (i16::from(px[0]) - i16::from(canvas.r)).abs() > 4
                || (i16::from(px[1]) - i16::from(canvas.g)).abs() > 4
                || (i16::from(px[2]) - i16::from(canvas.b)).abs() > 4
        })
        .count();
    non_bg as f32 / total as f32
}

/// Save an RGBA8 framebuffer `(width * height * 4)` to a PNG file, creating parent directories.
pub fn save_rgba8_png(path: &std::path::Path, width: u32, height: u32, rgba: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating PNG directory {}", parent.display()))?;
    }
    let img = image::RgbaImage::from_raw(width, height, rgba.to_vec())
        .with_context(|| format!("RGBA8 buffer size mismatch for {width}x{height}"))?;
    img.save(path)
        .with_context(|| format!("saving PNG {}", path.display()))
}

pub struct HeadlessGpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: GpuRenderer,
    display_list: DisplayList,
    width: u32,
    height: u32,
    target_tex: wgpu::Texture,
    target_view: wgpu::TextureView,
}

impl HeadlessGpu {
    pub fn new(width: u32, height: u32) -> Result<Self> {
        pollster::block_on(Self::new_async(width, height))
    }

    async fn new_async(width: u32, height: u32) -> Result<Self> {
        let width = width.max(64);
        let height = height.max(64);
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await
            .context("No GPU adapter available for headless wgpu rendering")?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .context("Failed to request wgpu device")?;

        let (target_tex, target_view) = create_offscreen_target(&device, width, height);
        let renderer = GpuRenderer::new(&device, &queue, OFFSCREEN_FORMAT, width, height);
        let display_list = DisplayList::new(width as f32, height as f32, 1.0);

        Ok(Self {
            device,
            queue,
            renderer,
            display_list,
            width,
            height,
            target_tex,
            target_view,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Resize the offscreen render target and renderer bloom textures in place (no-op if unchanged).
    pub fn resize(&mut self, width: u32, height: u32) {
        let width = width.max(64);
        let height = height.max(64);
        if self.width == width && self.height == height {
            return;
        }
        let (target_tex, target_view) = create_offscreen_target(&self.device, width, height);
        self.renderer.resize(&self.device, width, height);
        self.target_tex = target_tex;
        self.target_view = target_view;
        self.width = width;
        self.height = height;
    }

    /// Populate the reusable `DisplayList`, verify balanced `push_clip`/`pop_clip`, non-empty base
    /// layer, and zero missing embedded-font glyphs, then render into RGBA8 `(width * height * 4)`.
    pub fn render_rgba8(&mut self, frame: &FrameView, theme: &Theme) -> Result<Vec<u8>> {
        populate_display_list(
            &mut self.display_list,
            frame,
            self.width as f32,
            self.height as f32,
            1.0,
            theme,
        );
        anyhow::ensure!(
            self.display_list.current_clip() == self.display_list.viewport,
            "unbalanced DisplayList push_clip/pop_clip stack"
        );
        anyhow::ensure!(
            !self.display_list.base.quads.is_empty()
                && !self.display_list.base.texts.is_empty()
                && !self.display_list.base.commands.is_empty(),
            "DisplayList base layer must contain quads, text runs, and draw commands"
        );
        if let Some((ch, run)) = find_missing_glyph(&mut self.renderer, &self.display_list, theme) {
            anyhow::bail!(
                "emitted missing glyph '{ch}' (U+{:04X}) in {run:?}",
                ch as u32
            );
        }
        self.renderer.render(
            &self.device,
            &self.queue,
            &self.target_view,
            &self.display_list,
            theme,
        )?;
        self.readback_rgba8()
    }

    /// Render an arbitrary `DisplayList` directly into RGBA8 pixels `(width * height * 4)`.
    pub fn render_display_list_rgba8(
        &mut self,
        dl: &DisplayList,
        theme: &Theme,
    ) -> Result<Vec<u8>> {
        self.renderer
            .render(&self.device, &self.queue, &self.target_view, dl, theme)?;
        self.readback_rgba8()
    }

    /// Return the first `(char, text)` in `dl` that shapes to a missing `.notdef` glyph
    /// in the embedded font, or `None` when every emitted text run is natively covered.
    pub fn first_missing_glyph(
        &mut self,
        dl: &DisplayList,
        theme: &Theme,
    ) -> Option<(char, String)> {
        find_missing_glyph(&mut self.renderer, dl, theme)
    }

    fn readback_rgba8(&self) -> Result<Vec<u8>> {
        let bytes_per_row = align_up(self.width * 4, wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cadence.headless_readback"),
            size: u64::from(bytes_per_row * self.height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("cadence.headless_copy"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.target_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(encoder.finish()));

        let slice = readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |res| {
            let _ = tx.send(res);
        });
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        rx.recv().context("Waiting for GPU readback")??;

        let data = slice
            .get_mapped_range()
            .context("Mapping GPU readback range")?;
        let mut pixels = Vec::with_capacity((self.width * self.height * 4) as usize);
        for row in 0..self.height as usize {
            let start = row * bytes_per_row as usize;
            let end = start + (self.width * 4) as usize;
            pixels.extend_from_slice(&data[start..end]);
        }
        drop(data);
        readback.unmap();
        Ok(pixels)
    }
}

fn create_offscreen_target(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let target_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("cadence.headless_target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: OFFSCREEN_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target_view = target_tex.create_view(&wgpu::TextureViewDescriptor::default());
    (target_tex, target_view)
}

fn find_missing_glyph(
    renderer: &mut GpuRenderer,
    dl: &DisplayList,
    theme: &Theme,
) -> Option<(char, String)> {
    for run in dl.base.texts.iter().chain(dl.overlay.texts.iter()) {
        if let Some(ch) = renderer.first_missing_glyph(&run.text, run.bold, theme.font.family) {
            return Some((ch, run.text.clone()));
        }
    }
    None
}

fn align_up(value: u32, align: u32) -> u32 {
    value.div_ceil(align) * align
}
