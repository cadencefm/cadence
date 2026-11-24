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

    /// Render a `FrameView` into RGBA8 pixels `(width * height * 4)`.
    pub fn render_rgba8(&mut self, frame: &FrameView, theme: &Theme) -> Result<Vec<u8>> {
        populate_display_list(
            &mut self.display_list,
            frame,
            self.width as f32,
            self.height as f32,
            1.0,
            theme,
        );
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
        for run in dl.base.texts.iter().chain(dl.overlay.texts.iter()) {
            if let Some(ch) =
                self.renderer
                    .first_missing_glyph(&run.text, run.bold, theme.font.family)
            {
                return Some((ch, run.text.clone()));
            }
        }
        None
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

fn align_up(value: u32, align: u32) -> u32 {
    value.div_ceil(align) * align
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadence_app::keys::{KeyCode, KeyEvent, KeyModifiers};
    use cadence_app::App;
    use cadence_core::material::Project;
    use cadence_core::mixer::ChannelRef;
    use cadence_core::runtime::{AudioSnapshot, ChannelMeter};

    fn rich_snapshot() -> AudioSnapshot {
        let wave: Vec<f32> = (0..256)
            .map(|i| ((i as f32 / 256.0) * std::f32::consts::TAU * 3.0).sin() * 0.6)
            .collect();
        AudioSnapshot {
            playing: true,
            absolute_step: 18,
            step_fraction: 0.42,
            sample_rate: 48_000,
            peak: 0.7,
            peak_left: 0.65,
            peak_right: 0.52,
            waveform: wave.clone(),
            track_count: 1,
            track_waveforms: vec![wave],
            channel_meters: vec![ChannelMeter {
                channel: ChannelRef::Master,
                input_peak: [0.65, 0.52],
                input_peak_hold: [0.78, 0.68],
                output_peak: [0.65, 0.52],
                output_peak_hold: [0.78, 0.68],
                hot: false,
                clipped: false,
            }],
            ..AudioSnapshot::default()
        }
    }

    #[test]
    fn gpu_renders_all_decks_and_overlays_without_validation_errors() {
        let Ok(mut gpu) = HeadlessGpu::new(1280, 760) else {
            // Skip gracefully only if running in a GPU-less CI container.
            return;
        };
        let theme = Theme::cadence_dark();
        let snap = rich_snapshot();

        // 1. Default demo workspace (Arrange Placements + Bank + Output)
        let mut app = App::new(Project::demo(), "demo.json".into()).unwrap();
        let px_default = gpu
            .render_rgba8(&app.frame_view(&snap, "Metal · 48 kHz"), &theme)
            .unwrap();
        assert!(
            distinct_pixel_ratio(&px_default, theme.surface.canvas) > 0.25,
            "default workspace rendered too few visible pixels"
        );

        // 2. Arrange Evaluated Overview + Lane Roll
        app.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE))
            .unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE))
            .unwrap();
        let px_over = gpu
            .render_rgba8(&app.frame_view(&snap, "Metal · 48 kHz"), &theme)
            .unwrap();
        assert!(distinct_pixel_ratio(&px_over, theme.surface.canvas) > 0.25);

        app.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE))
            .unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE))
            .unwrap();
        let px_roll = gpu
            .render_rgba8(&app.frame_view(&snap, "Metal · 48 kHz"), &theme)
            .unwrap();
        assert!(distinct_pixel_ratio(&px_roll, theme.surface.canvas) > 0.25);

        // 3. Pattern deck (Notes piano roll + velocity lane)
        app.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE))
            .unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE))
            .unwrap();
        let px_pat = gpu
            .render_rgba8(&app.frame_view(&snap, "Metal · 48 kHz"), &theme)
            .unwrap();
        assert!(distinct_pixel_ratio(&px_pat, theme.surface.canvas) > 0.25);

        // 4. Mixer deck
        app.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE))
            .unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE))
            .unwrap();
        let px_mix = gpu
            .render_rgba8(&app.frame_view(&snap, "Metal · 48 kHz"), &theme)
            .unwrap();
        assert!(distinct_pixel_ratio(&px_mix, theme.surface.canvas) > 0.25);

        // 5. Rack Instrument deck (Quick overview visualizers + parameter bars)
        app.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE))
            .unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE))
            .unwrap();
        let px_inst = gpu
            .render_rgba8(&app.frame_view(&snap, "Metal · 48 kHz"), &theme)
            .unwrap();
        assert!(distinct_pixel_ratio(&px_inst, theme.surface.canvas) > 0.25);

        // 6. Take deck (gt) after arming take (tr) and rolling playback, marked via Vim `v` visual range
        app.press_keys("tr").unwrap();
        app.apply(cadence_app::Action::Play { playing: true })
            .unwrap();
        app.advance(64);
        app.press_keys("gtvllv").unwrap();
        let px_take = gpu
            .render_rgba8(&app.frame_view(&snap, "Metal · 48 kHz"), &theme)
            .unwrap();
        assert!(distinct_pixel_ratio(&px_take, theme.surface.canvas) > 0.25);

        // 7. Jam mode with GPU Jam Drawer + floating Lane Layer HUD + held note glow
        app.press_keys("ga<m-j>q").unwrap();
        let px_jam = gpu
            .render_rgba8(&app.frame_view(&snap, "Metal · 48 kHz"), &theme)
            .unwrap();
        assert!(distinct_pixel_ratio(&px_jam, theme.surface.canvas) > 0.25);

        // 8. Multi-Zone Kit Rack Deck (TR-909 Zones + Quick Overview + Live Scope)
        app.execute_command("kit Kit909 tr909").unwrap();
        app.execute_command("instrument Kit909").unwrap();
        let px_kit = gpu
            .render_rgba8(&app.frame_view(&snap, "Metal · 48 kHz"), &theme)
            .unwrap();
        assert!(distinct_pixel_ratio(&px_kit, theme.surface.canvas) > 0.25);
        if let Ok(dir) = std::env::var("CADENCE_DUMP_RACK_KIT_DIR") {
            let dir = std::path::PathBuf::from(dir);
            let _ = std::fs::create_dir_all(&dir);
            if let Some(img) = image::RgbaImage::from_raw(1280, 760, px_kit) {
                let _ = img.save(dir.join("14-rack-kit-tr909.png"));
            }
        }
        app.press_keys("<Esc><Esc>").unwrap();

        // 9. Bank zoomed Ableton-style lane-columns launch matrix (gb zz)
        app.press_keys("gbzz").unwrap();
        let px_bank_matrix = gpu
            .render_rgba8(&app.frame_view(&snap, "Metal · 48 kHz"), &theme)
            .unwrap();
        assert!(distinct_pixel_ratio(&px_bank_matrix, theme.surface.canvas) > 0.25);
        app.press_keys("zz").unwrap();

        // 10. Live Relationship / Patchbay Graph deck (gG zz) with active signal-path illumination
        app.press_keys("gGzzlj").unwrap();
        let px_graph = gpu
            .render_rgba8(&app.frame_view(&snap, "Metal · 48 kHz"), &theme)
            .unwrap();
        assert!(distinct_pixel_ratio(&px_graph, theme.surface.canvas) > 0.25);
        app.press_keys("zz").unwrap();

        // 11. Inline Mini-Notation Pattern deck with fractional sub-step bars
        app.execute_command("mini Sparkle 8 c4 [e4 g4] ~ [c4,e4,g4] d4 [f4 a4 c5] g4*3 ~")
            .unwrap();
        app.press_keys("gp").unwrap();
        let px_mini = gpu
            .render_rgba8(&app.frame_view(&snap, "Metal · 48 kHz"), &theme)
            .unwrap();
        assert!(distinct_pixel_ratio(&px_mini, theme.surface.canvas) > 0.25);

        // 12. Tabbed & searchable Help overlay (`?`, tab to Mini & Algebra)
        app.press_keys("?lll").unwrap();
        let px_help = gpu
            .render_rgba8(&app.frame_view(&snap, "Metal · 48 kHz"), &theme)
            .unwrap();
        assert!(distinct_pixel_ratio(&px_help, theme.surface.canvas) > 0.25);

        // 13. Persisted Global Settings modal (`g,`) live-switching to Midnight Neon theme (`Alt+l`).
        app.press_keys("<Esc>g,<m-l>").unwrap();
        let fv_settings = app.frame_view(&snap, "Metal · 48 kHz");
        assert_eq!(
            fv_settings.chrome.theme,
            cadence_core::config::ThemeId::MidnightNeon
        );
        let neon_theme = Theme::from_id(fv_settings.chrome.theme);
        let px_settings = gpu.render_rgba8(&fv_settings, &neon_theme).unwrap();
        assert!(distinct_pixel_ratio(&px_settings, neon_theme.surface.canvas) > 0.25);

        // Confirm different views actually produce visually distinct frames.
        assert_ne!(px_default, px_mix);
        assert_ne!(px_mix, px_inst);
        assert_ne!(px_inst, px_take);
        assert_ne!(px_take, px_jam);
        assert_ne!(px_jam, px_bank_matrix);
        assert_ne!(px_bank_matrix, px_graph);
        assert_ne!(px_graph, px_mini);
        assert_ne!(px_mini, px_help);
        assert_ne!(px_help, px_settings);

        if let Ok(dir) = std::env::var("CADENCE_DUMP_GUI_DIR") {
            let dir = std::path::PathBuf::from(dir);
            let _ = std::fs::create_dir_all(&dir);
            for (name, px) in [
                ("01-arrange-default.png", &px_default),
                ("02-arrange-overview.png", &px_over),
                ("03-arrange-roll.png", &px_roll),
                ("04-pattern-roll.png", &px_pat),
                ("05-mixer.png", &px_mix),
                ("06-rack-instrument.png", &px_inst),
                ("07-take-deck.png", &px_take),
                ("08-jam-hud.png", &px_jam),
                ("09-bank-matrix.png", &px_bank_matrix),
                ("10-signal-graph.png", &px_graph),
                ("11-mini-notation.png", &px_mini),
                ("12-help-overlay.png", &px_help),
                ("13-settings-neon.png", &px_settings),
            ] {
                if let Some(img) = image::RgbaImage::from_raw(1280, 760, px.clone()) {
                    let _ = img.save(dir.join(name));
                }
            }
        }
    }

    #[test]
    fn gpu_renders_afterglow_showcase_screenshots() {
        let Ok(mut gpu) = HeadlessGpu::new(1440, 860) else {
            return;
        };
        let demo_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/demo/project.json");
        let Ok(project) = cadence_app::load_project(&demo_path) else {
            return;
        };
        let theme = Theme::cadence_dark();
        let wave: Vec<f32> = (0..256)
            .map(|i| {
                let t = i as f32 / 256.0 * std::f32::consts::TAU;
                (t * 2.0).sin() * 0.45 + (t * 5.0).sin() * 0.25 + (t * 11.0).sin() * 0.10
            })
            .collect();
        let mut meters = vec![ChannelMeter {
            channel: ChannelRef::Master,
            input_peak: [0.84, 0.78],
            input_peak_hold: [0.91, 0.87],
            output_peak: [0.84, 0.78],
            output_peak_hold: [0.91, 0.87],
            hot: false,
            clipped: false,
        }];
        for (idx, ch) in project.mixer.channels.iter().enumerate() {
            let lvl = (0.38 + ((idx * 17) % 10) as f32 * 0.055).min(0.88);
            meters.push(ChannelMeter {
                channel: ChannelRef::Channel(ch.id),
                input_peak: [lvl, (lvl * 0.92).min(0.88)],
                input_peak_hold: [(lvl + 0.06).min(0.94), (lvl + 0.04).min(0.92)],
                output_peak: [lvl, (lvl * 0.92).min(0.88)],
                output_peak_hold: [(lvl + 0.06).min(0.94), (lvl + 0.04).min(0.92)],
                hot: false,
                clipped: false,
            });
        }
        let mut track_ids = [0u64; 16];
        for (i, inst) in project.instruments.iter().take(16).enumerate() {
            track_ids[i] = inst.id;
        }
        // Step 324 = mid-Build 1 (bar 21) so macro Cutoff sweeps are visibly displaced!
        let snap = AudioSnapshot {
            playing: true,
            absolute_step: 324,
            step_fraction: 0.35,
            sample_rate: 48_000,
            peak: 0.84,
            peak_left: 0.84,
            peak_right: 0.78,
            waveform: wave.clone(),
            track_count: project.instruments.len(),
            track_ids,
            track_waveforms: vec![wave; project.instruments.len()],
            channel_meters: meters,
            ..AudioSnapshot::default()
        };

        let mut app = App::new(project, demo_path).unwrap();
        let status = "Metal · 48 kHz";

        // 1. Default Arrange + Bank + Live Output (Afterglow 64-bar EDM arrangement)
        let px_arrange = gpu
            .render_rgba8(&app.frame_view(&snap, status), &theme)
            .unwrap();
        assert!(distinct_pixel_ratio(&px_arrange, theme.surface.canvas) > 0.25);

        // 2. Full-Screen Signal Graph Deck (gG zz) illuminating Harmony -> Drop Stabs -> Supersaw -> Music -> Master!
        app.press_keys("gGzzljj").unwrap();
        let px_graph = gpu
            .render_rgba8(&app.frame_view(&snap, status), &theme)
            .unwrap();
        assert!(distinct_pixel_ratio(&px_graph, theme.surface.canvas) > 0.25);
        app.press_keys("zz").unwrap();

        // 3. Pattern Deck: Inline Strudel Mini-Notation (`Bell Mini`) with fractional sub-step bars!
        app.execute_command("open \"Bell Mini\"").unwrap();
        app.press_keys("gp").unwrap();
        let px_mini = gpu
            .render_rgba8(&app.frame_view(&snap, status), &theme)
            .unwrap();
        assert!(distinct_pixel_ratio(&px_mini, theme.surface.canvas) > 0.25);

        // 4. Pattern Deck: Derived pattern (`Drop Stabs`) with stacked live source preview strips!
        app.execute_command("open \"Drop Stabs\"").unwrap();
        app.press_keys("gp").unwrap();
        let px_derived = gpu
            .render_rgba8(&app.frame_view(&snap, status), &theme)
            .unwrap();
        assert!(distinct_pixel_ratio(&px_derived, theme.surface.canvas) > 0.25);

        // 5. Rack Instrument Deck (`Anthem Lead`) showing live modulated Cutoff dot & sweep ribbon!
        app.execute_command("instrument \"Anthem Lead\"").unwrap();
        let px_rack = gpu
            .render_rgba8(&app.frame_view(&snap, status), &theme)
            .unwrap();
        assert!(distinct_pixel_ratio(&px_rack, theme.surface.canvas) > 0.25);

        // 6. Mixer Deck (`gm zz`) showing full Audio / Group / Return / Master console + FX inserts!
        app.press_keys("gmzz").unwrap();
        let px_mixer = gpu
            .render_rgba8(&app.frame_view(&snap, status), &theme)
            .unwrap();
        assert!(distinct_pixel_ratio(&px_mixer, theme.surface.canvas) > 0.25);
        app.press_keys("zz").unwrap();

        // 7. Full-Screen Ableton-Style Bank Launchpad Matrix (`gb zz`)!
        app.press_keys("gbzz").unwrap();
        let px_matrix = gpu
            .render_rgba8(&app.frame_view(&snap, status), &theme)
            .unwrap();
        assert!(distinct_pixel_ratio(&px_matrix, theme.surface.canvas) > 0.25);
        app.press_keys("zz").unwrap();

        // 8. Live Jam Mode HUD Drawer + Floating Lane-Layer Card (`z1 <m-j> q`)!
        app.press_keys("z1<m-j>q").unwrap();
        let px_jam = gpu
            .render_rgba8(&app.frame_view(&snap, status), &theme)
            .unwrap();
        assert!(distinct_pixel_ratio(&px_jam, theme.surface.canvas) > 0.25);

        if let Ok(dir) = std::env::var("CADENCE_DUMP_SHOWCASE_DIR") {
            let dir = std::path::PathBuf::from(dir);
            let _ = std::fs::create_dir_all(&dir);
            for (name, px) in [
                ("01-arrange.png", &px_arrange),
                ("02-signal-graph.png", &px_graph),
                ("03-pattern-mini.png", &px_mini),
                ("04-pattern-derived.png", &px_derived),
                ("05-rack-instrument.png", &px_rack),
                ("06-mixer.png", &px_mixer),
                ("07-launchpad-matrix.png", &px_matrix),
                ("08-jam-hud.png", &px_jam),
            ] {
                if let Some(img) = image::RgbaImage::from_raw(1440, 860, px.clone()) {
                    let _ = img.save(dir.join(name));
                }
            }
        }
    }

    #[test]
    fn gpu_midi_deck_and_inline_badges_render_cleanly() {
        let Ok(mut gpu) = HeadlessGpu::new(1440, 860) else {
            // Skip gracefully only if running in a GPU-less CI container.
            return;
        };
        let theme = Theme::cadence_dark();
        let snap = rich_snapshot();
        let status = "48000 Hz · wgpu";

        let demo_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/demo/project.json");
        let project = cadence_app::load_project(&demo_path).unwrap();
        let mut app = App::new(project, demo_path).unwrap();
        app.set_midi_available_ports(
            vec!["Minilab3 MIDI".into(), "IAC Driver Bus 1".into()],
            vec!["Minilab3 MIDI".into(), "IAC Driver Bus 1".into()],
        );
        app.midi.port = Some("Minilab3 MIDI".into());
        app.midi.out_port = Some("Minilab3 MIDI".into());
        app.execute_command("midi profile minilab3_daw").unwrap();
        app.execute_command("midi pads sections").unwrap();
        app.config.midi_pickup = false;
        let _ = app.handle_midi(cadence_core::runtime::MidiMessage::Control {
            channel: 0,
            controller: 86,
            value: 92,
        });
        let _ = app.handle_midi(cadence_core::runtime::MidiMessage::Control {
            channel: 0,
            controller: 87,
            value: 74,
        });
        let _ = app.handle_midi(cadence_core::runtime::MidiMessage::NoteOn {
            channel: 9,
            note: 37,
            velocity: 118,
        });

        // Render full Devices · MIDI Deck (`gd zz`)
        app.press_keys("gdzz").unwrap();
        let px_midi = gpu
            .render_rgba8(&app.frame_view(&snap, status), &theme)
            .unwrap();
        assert!(distinct_pixel_ratio(&px_midi, theme.surface.canvas) > 0.20);

        // Render Rack Instrument Deck with live `[M1·CC86]` badges + armed `[LEARN…]` pill!
        app.press_keys("zz").unwrap();
        app.execute_command("instrument \"Anthem Lead\"").unwrap();
        app.press_keys("M").unwrap();
        let px_badges = gpu
            .render_rgba8(&app.frame_view(&snap, status), &theme)
            .unwrap();
        assert!(distinct_pixel_ratio(&px_badges, theme.surface.canvas) > 0.25);

        if let Ok(dir) = std::env::var("CADENCE_DUMP_MIDI_DIR") {
            let dir = std::path::PathBuf::from(dir);
            let _ = std::fs::create_dir_all(&dir);
            if let Some(img) = image::RgbaImage::from_raw(1440, 860, px_midi) {
                let _ = img.save(dir.join("midi-deck.png"));
            }
            if let Some(img) = image::RgbaImage::from_raw(1440, 860, px_badges) {
                let _ = img.save(dir.join("rack-midi-badges.png"));
            }
        }
    }

    #[test]
    fn gpu_file_hub_modal_tabs_render_cleanly() {
        let Ok(mut gpu) = HeadlessGpu::new(1440, 860) else {
            // Skip gracefully only if running in a GPU-less CI container.
            return;
        };
        let theme = Theme::cadence_dark();
        let snap = rich_snapshot();
        let status = "48000 Hz · wgpu";

        let demo_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/demo/project.json");
        let project = cadence_app::load_project(&demo_path).unwrap();
        let mut app = App::new(project, demo_path.clone()).unwrap();
        app.config.record_recent_project(&demo_path);

        // 1. Open / Recent tab (`gf`)
        app.press_keys("gf").unwrap();
        let px_open = gpu
            .render_rgba8(&app.frame_view(&snap, status), &theme)
            .unwrap();
        assert!(distinct_pixel_ratio(&px_open, theme.surface.canvas) > 0.20);

        // 2. Export Audio tab (`4`) with rich post-export telemetry!
        app.press_keys("4l").unwrap(); // Master + Per-Channel Stems
        app.record_export_summary(cadence_app::ExportSummary {
            target: "afterglow-stems/".into(),
            files_written: 11,
            duration_sec: 120.0,
            peak_left_db: -0.9,
            peak_right_db: -1.1,
            rms_db: -11.4,
            clips: 0,
            sample_rate: 48_000,
        });
        let px_export = gpu
            .render_rgba8(&app.frame_view(&snap, status), &theme)
            .unwrap();
        assert!(distinct_pixel_ratio(&px_export, theme.surface.canvas) > 0.20);

        if let Ok(dir) = std::env::var("CADENCE_DUMP_FILEHUB_DIR") {
            let dir = std::path::PathBuf::from(dir);
            let _ = std::fs::create_dir_all(&dir);
            if let Some(img) = image::RgbaImage::from_raw(1440, 860, px_open) {
                let _ = img.save(dir.join("filehub-open.png"));
            }
            if let Some(img) = image::RgbaImage::from_raw(1440, 860, px_export) {
                let _ = img.save(dir.join("filehub-export.png"));
            }
        }
    }

    #[test]
    fn gpu_z_order_occludes_cables_behind_cards_and_draws_handles_over_curves() {
        let Ok(mut gpu) = HeadlessGpu::new(256, 256) else {
            return;
        };
        let theme = Theme::cadence_dark();
        let pixel_at = |rgba: &[u8], x: u32, y: u32| -> [u8; 3] {
            let off = ((y * 256 + x) * 4) as usize;
            [rgba[off], rgba[off + 1], rgba[off + 2]]
        };
        let close_rgb = |actual: [u8; 3], expected: Color, tol: i16| -> bool {
            (i16::from(actual[0]) - i16::from(expected.r)).abs() <= tol
                && (i16::from(actual[1]) - i16::from(expected.g)).abs() <= tol
                && (i16::from(actual[2]) - i16::from(expected.b)).abs() <= tol
        };

        // 1. Painter's order:
        //    Top half: bright emissive red cable BEHIND opaque elevated card.
        //    Bottom half: bright green handle ON TOP OF bright emissive red curve.
        let mut dl = DisplayList::new(256.0, 256.0, 1.0);
        dl.polyline(
            &[(16.0, 80.0), (240.0, 80.0)],
            8.0,
            theme.accent.alert,
            0.60,
            None,
        );
        dl.panel(
            crate::display::Rect::new(80.0, 48.0, 96.0, 64.0),
            theme.surface.elevated,
            theme.accent.focus,
            6.0,
            1.5,
            0.0,
        );
        dl.polyline(
            &[(16.0, 180.0), (240.0, 180.0)],
            8.0,
            theme.accent.alert,
            0.50,
            None,
        );
        dl.rounded_glow(
            crate::display::Rect::new(120.0, 172.0, 16.0, 16.0),
            theme.accent.playhead,
            8.0,
            0.40,
        );

        let rgba = gpu.render_display_list_rgba8(&dl, &theme).unwrap();

        // Exposed cable outside card is bright alert red.
        let exposed = pixel_at(&rgba, 40, 80);
        assert!(
            exposed[0] > exposed[1].saturating_add(60) && exposed[0] > 160,
            "expected exposed cable pixel to be alert red, got {exposed:?}"
        );
        // Cable behind opaque card is cleanly occluded (including its emissive glow!).
        let occluded = pixel_at(&rgba, 128, 80);
        assert!(
            close_rgb(occluded, theme.surface.elevated, 10),
            "opaque card must occlude cable behind it: expected {:?}, got {occluded:?}",
            [
                theme.surface.elevated.r,
                theme.surface.elevated.g,
                theme.surface.elevated.b
            ]
        );
        // Handle drawn after curve sits cleanly ON TOP of the curve.
        let handle = pixel_at(&rgba, 128, 180);
        assert!(
            handle[1] > handle[0].saturating_add(40) && handle[1] > 150,
            "expected handle pixel on top of curve to be playhead green, got {handle:?}"
        );

        // 2. Zero-bloom fast path (`emissive_count == 0`): skips blur passes cleanly.
        dl.clear();
        dl.polyline(
            &[(24.0, 200.0), (128.0, 48.0), (232.0, 200.0)],
            4.0,
            theme.accent.focus,
            0.0,
            None,
        );
        assert_eq!(dl.base.emissive_count, 0);
        let rgba_no_bloom = gpu.render_display_list_rgba8(&dl, &theme).unwrap();
        assert!(distinct_pixel_ratio(&rgba_no_bloom, theme.surface.canvas) > 0.01);
        // Sharp miter/bevel apex at (128, 48) must be filled without a triangular notch gap.
        let apex = pixel_at(&rgba_no_bloom, 128, 48);
        assert!(
            !close_rgb(apex, theme.surface.canvas, 10),
            "polyline vertex join apex must be filled, got canvas {apex:?}"
        );
    }

    #[test]
    fn gpu_renders_sampler_crop_dimming_and_signal_graph_z_order() {
        let Ok(mut gpu) = HeadlessGpu::new(1440, 860) else {
            return;
        };
        let theme = Theme::cadence_dark();
        let snap = rich_snapshot();
        let status = "Metal · 48 kHz";

        let demo_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/demo/project.json");
        let project = cadence_app::load_project(&demo_path).unwrap_or_else(|_| Project::demo());
        let sample_path = demo_path.parent().unwrap().join("samples/brass-pad.wav");
        let frames = cadence_core::sampler::load_sample_file(&sample_path, None)
            .expect("load brass-pad.wav")
            .frames;
        let mut app = App::new(project, demo_path).unwrap();

        // 1. Sampler Instrument deck (`gi zz`) with active start/end crop dimming + Forward loop!
        app.execute_command("sampler BrassCrop samples/brass-pad.wav")
            .unwrap();
        app.execute_command("instrument BrassCrop").unwrap();
        app.execute_command(&format!("param sample_start {}", frames * 28 / 100))
            .unwrap();
        app.execute_command(&format!("param sample_end {}", frames * 74 / 100))
            .unwrap();
        app.execute_command("param loop_mode forward").unwrap();
        app.execute_command(&format!("param loop_start {}", frames * 36 / 100))
            .unwrap();
        app.execute_command(&format!("param loop_end {}", frames * 66 / 100))
            .unwrap();
        app.press_keys("zz").unwrap();
        let px_sampler = gpu
            .render_rgba8(&app.frame_view(&snap, status), &theme)
            .unwrap();
        assert!(distinct_pixel_ratio(&px_sampler, theme.surface.canvas) > 0.25);

        // 2. Signal Graph (`gG zz`) where highlighted cables route across columns behind node cards.
        app.press_keys("zzgGzz").unwrap();
        app.view_state.graph_col = 0;
        app.view_state.graph_row = 0;
        let px_graph = gpu
            .render_rgba8(&app.frame_view(&snap, status), &theme)
            .unwrap();
        assert!(distinct_pixel_ratio(&px_graph, theme.surface.canvas) > 0.20);

        if let Ok(dir) = std::env::var("CADENCE_DUMP_ZORDER_DIR") {
            let dir = std::path::PathBuf::from(dir);
            let _ = std::fs::create_dir_all(&dir);
            if let Some(img) = image::RgbaImage::from_raw(1440, 860, px_sampler) {
                let _ = img.save(dir.join("01-sampler-crop-dimming.png"));
            }
            if let Some(img) = image::RgbaImage::from_raw(1440, 860, px_graph) {
                let _ = img.save(dir.join("02-signal-graph-zorder.png"));
            }
        }
    }
}
