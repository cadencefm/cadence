//! GPU-accelerated keyboard-driven frontend for Cadence (`winit` + `wgpu` + `glyphon`).
//!
//! Zero point-and-click, zero desktop widget toolkits: Cadence owns the entire GPU
//! frame with semantic `&Theme` tokens, custom WGSL SDF/scope/bloom pipelines, and
//! razor-sharp HiDPI JetBrains Mono typography.
pub mod chrome;
pub mod decks;
pub mod display;
pub mod gpu;
pub mod headless;
pub mod icon;
pub mod keys;
pub mod layout;
pub mod overlays;
pub mod theme;

use anyhow::{Context, Result};
use cadence_app::{App, Outcome};
use cadence_core::runtime::{AudioSnapshot, MidiMessage};
use decks::mixer::MIXER_MIN_STRIP_W;
use decks::populate_display_list;
use display::{DisplayList, Rect};
use gpu::GpuRenderer;
use layout::compute_pane_metrics;
use std::sync::Arc;
use std::time::{Duration, Instant};
use theme::Theme;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent as WinitKeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, KeyCode as PhysicalCode, ModifiersState, PhysicalKey};
use winit::window::{Fullscreen, Window, WindowAttributes, WindowId};

/// Result returned by [`HostBridge::poll_host`] on each event-loop wakeup.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HostPollResult {
    /// Set when host background work (e.g. a `--ctl` control request) mutated state and needs redraw.
    pub redraw: bool,
    /// Set when a host command requested clean GUI shutdown (e.g. `:q`).
    pub quit: bool,
}

/// Host runtime bridge supplied by the `cadence` binary to the GUI event loop.
pub trait HostBridge {
    fn snapshot(&self) -> AudioSnapshot;
    fn apply_outcome(&mut self, app: &mut App, outcome: &Outcome) -> Result<()>;
    fn poll_midi(&mut self) -> Vec<MidiMessage>;
    fn audio_status(&self) -> &str;
    /// Register an immediate event-loop wakeup callback invoked by off-GUI threads (such as `--ctl`).
    fn set_wake_callback(&mut self, _wake: Arc<dyn Fn() + Send + Sync>) {}
    /// Drain any pending host-initiated work (such as `--ctl` socket requests) synchronously on the GUI thread.
    fn poll_host(&mut self, _app: &mut App) -> HostPollResult {
        HostPollResult::default()
    }
}

/// Interactive GUI launch options.
#[derive(Clone, Debug)]
pub struct GuiOptions {
    pub title: String,
}

impl Default for GuiOptions {
    fn default() -> Self {
        Self {
            title: "Cadence".into(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct GuiWake;

/// Launch the interactive keyboard-driven `wgpu` GUI frontend.
pub fn run_gui(app: &mut App, host: &mut dyn HostBridge, options: &GuiOptions) -> Result<()> {
    let mut builder = EventLoop::<GuiWake>::with_user_event();
    #[cfg(target_os = "macos")]
    {
        // Disable winit's default AppKit menu whose Cmd+Q item calls `terminate:` directly
        // without routing through `CloseRequested` or dirty-save confirmation.
        use winit::platform::macos::EventLoopBuilderExtMacOS;
        builder.with_default_menu(false);
    }
    let event_loop = builder.build().context("Creating winit event loop")?;
    let proxy = event_loop.create_proxy();
    host.set_wake_callback(Arc::new(move || {
        let _ = proxy.send_event(GuiWake);
    }));
    let initial_normal_size = app.config.window_dimensions();
    let mut runner = GuiApp {
        app,
        host,
        options,
        modifiers: ModifiersState::empty(),
        gpu: None,
        last_normal_size: initial_normal_size,
        shutdown_done: false,
        redraw_needed: true,
        occluded: false,
        surface_occluded: false,
        next_surface_probe: None,
        next_frame_at: None,
        cached_snapshot: None,
        snapshot_for_redraw: false,
        snapshot_from_redraw: false,
        error: None,
    };
    event_loop
        .run_app(&mut runner)
        .context("Running winit event loop")?;
    if let Some(err) = runner.error {
        return Err(err);
    }
    Ok(())
}

struct WindowGpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    renderer: GpuRenderer,
    display_list: DisplayList,
    // Keep window last so `surface` drops before `window`.
    window: Arc<Window>,
}

impl WindowGpu {
    fn new(window: Arc<Window>, event_loop: &ActiveEventLoop) -> Result<Self> {
        pollster::block_on(Self::new_async(window, event_loop))
    }

    async fn new_async(window: Arc<Window>, event_loop: &ActiveEventLoop) -> Result<Self> {
        let size = window.inner_size();
        let width = size.width.max(1);
        let height = size.height.max(1);

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(
            Box::new(event_loop.owned_display_handle()),
        ));
        let surface = instance
            .create_surface(Arc::clone(&window))
            .context("Creating wgpu surface")?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await
            .context("Requesting GPU adapter")?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .context("Requesting GPU device")?;

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .unwrap_or(caps.formats[0]);
        let present_mode = if caps.present_modes.contains(&wgpu::PresentMode::Fifo) {
            wgpu::PresentMode::Fifo
        } else {
            caps.present_modes[0]
        };
        let alpha_mode = caps.alpha_modes[0];
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            present_mode,
            alpha_mode,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(&device, &surface_config);

        let scale = (window.scale_factor() as f32).max(1.0);
        let renderer = GpuRenderer::new(&device, &queue, format, width, height);
        let display_list = DisplayList::new(width as f32 / scale, height as f32 / scale, scale);
        Ok(Self {
            device,
            queue,
            surface,
            surface_config,
            renderer,
            display_list,
            window,
        })
    }

    fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.surface_config.width = width;
        self.surface_config.height = height;
        self.surface.configure(&self.device, &self.surface_config);
        self.renderer.resize(&self.device, width, height);
    }

    fn logical_size(&self) -> (f32, f32) {
        let scale = (self.window.scale_factor() as f32).max(1.0);
        (
            self.surface_config.width as f32 / scale,
            self.surface_config.height as f32 / scale,
        )
    }
}

const ACTIVE_FRAME_INTERVAL: Duration = Duration::from_millis(16);

struct GuiApp<'a> {
    app: &'a mut App,
    host: &'a mut dyn HostBridge,
    options: &'a GuiOptions,
    modifiers: ModifiersState,
    gpu: Option<WindowGpu>,
    /// Last non-maximized, non-fullscreen logical window size `(width, height)`.
    last_normal_size: (u32, u32),
    shutdown_done: bool,
    redraw_needed: bool,
    occluded: bool,
    surface_occluded: bool,
    next_surface_probe: Option<Instant>,
    /// Next ~60 Hz display-paced frame deadline during active audio playback/metering.
    next_frame_at: Option<Instant>,
    /// Latest cached [`AudioSnapshot`] shared across `RedrawRequested` and `about_to_wait`
    /// so each frame queries `host.snapshot()` at most once.
    cached_snapshot: Option<AudioSnapshot>,
    /// True when `about_to_wait` just captured `cached_snapshot` right before `request_redraw`.
    snapshot_for_redraw: bool,
    /// True when `RedrawRequested` just captured `cached_snapshot` right before `about_to_wait`.
    snapshot_from_redraw: bool,
    error: Option<anyhow::Error>,
}

impl GuiApp<'_> {
    fn is_hidden(&self) -> bool {
        self.occluded
            || self.gpu.as_ref().is_some_and(|gpu| {
                gpu.window.is_minimized().unwrap_or(false)
                    || gpu.window.inner_size().width == 0
                    || gpu.window.inner_size().height == 0
            })
    }

    fn poll_background(&mut self) -> HostPollResult {
        let mut result = HostPollResult::default();
        for msg in self.host.poll_midi() {
            match self.app.handle_midi(msg) {
                Ok(out) => {
                    if let Err(err) = self.host.apply_outcome(self.app, &out) {
                        self.app.status = format!("Audio: {err:#}");
                    }
                    result.quit |= out.quit;
                }
                Err(err) => self.app.status = format!("{err:#}"),
            }
            self.redraw_needed = true;
            result.redraw = true;
        }
        match self.app.poll_browser_updates() {
            Ok(out) => {
                if let Err(err) = self.host.apply_outcome(self.app, &out) {
                    self.app.status = format!("Audio: {err:#}");
                }
            }
            Err(err) => self.app.status = format!("{err:#}"),
        }
        let host_polled = self.host.poll_host(self.app);
        if host_polled.redraw {
            self.redraw_needed = true;
            result.redraw = true;
        }
        result.quit |= host_polled.quit;
        result
    }

    fn observe_transport_and_midi(&mut self, snapshot: &AudioSnapshot) {
        let mut observed = self.app.observe_snapshot(snapshot);
        self.app.sync_midi_feedback(&mut observed);
        if let Err(err) = self.host.apply_outcome(self.app, &observed) {
            self.app.status = format!("Audio: {err:#}");
        }
    }

    fn take_or_fetch_redraw_snapshot(&mut self) -> AudioSnapshot {
        if self.snapshot_for_redraw {
            self.snapshot_for_redraw = false;
            if let Some(snap) = self.cached_snapshot.take() {
                return snap;
            }
        }
        self.host.snapshot()
    }

    fn take_or_fetch_wait_snapshot(&mut self) -> (AudioSnapshot, bool) {
        if self.snapshot_from_redraw {
            self.snapshot_from_redraw = false;
            if let Some(snap) = self.cached_snapshot.take() {
                return (snap, false);
            }
        }
        (self.host.snapshot(), true)
    }

    fn update_viewport_metrics(&mut self, logical_width: f32, logical_height: f32) {
        // Keep keyboard PageUp/PageDown & Arrange paging aligned with actual GUI pane geometry.
        let viewport = Rect::new(0.0, 0.0, logical_width.max(1.0), logical_height.max(1.0));
        let metrics = compute_pane_metrics(self.app.layout(), viewport);
        self.app.view_state.mixer_page_size =
            ((metrics.mixer_inner_w / MIXER_MIN_STRIP_W).floor() as usize).clamp(1, 16);
        self.app
            .set_arrange_page_size(self.app.arrange_visible_bars() * 16);
    }

    fn record_normal_window_size(&mut self, lw: f32, lh: f32) {
        let Some(gpu) = &self.gpu else {
            return;
        };
        if gpu.window.is_maximized()
            || gpu.window.fullscreen().is_some()
            || gpu.window.is_minimized().unwrap_or(false)
        {
            return;
        }
        if lw >= f32::from(cadence_core::config::MIN_WINDOW_WIDTH as u16)
            && lh >= f32::from(cadence_core::config::MIN_WINDOW_HEIGHT as u16)
        {
            self.last_normal_size = (
                (lw.round() as u32).clamp(
                    cadence_core::config::MIN_WINDOW_WIDTH,
                    cadence_core::config::MAX_WINDOW_WIDTH,
                ),
                (lh.round() as u32).clamp(
                    cadence_core::config::MIN_WINDOW_HEIGHT,
                    cadence_core::config::MAX_WINDOW_HEIGHT,
                ),
            );
        }
    }

    fn request_quit(&mut self, event_loop: &ActiveEventLoop) {
        let out = self.app.request_quit();
        if let Err(err) = self.host.apply_outcome(self.app, &out) {
            self.app.status = format!("Audio: {err:#}");
        }
        if out.quit {
            self.shutdown();
            event_loop.exit();
        } else {
            self.redraw_needed = true;
            if let Some(gpu) = &self.gpu {
                gpu.window.request_redraw();
            }
        }
    }

    fn handle_os_window_shortcut(
        &mut self,
        event: &WinitKeyEvent,
        event_loop: &ActiveEventLoop,
    ) -> bool {
        if !cfg!(target_os = "macos")
            || event.state != ElementState::Pressed
            || event.repeat
            || !self.modifiers.super_key()
            || self.modifiers.control_key()
            || self.modifiers.alt_key()
        {
            return false;
        }
        let is_key = |ch: char, phys: PhysicalCode| {
            matches!(&event.logical_key, Key::Character(s) if s.eq_ignore_ascii_case(&ch.to_string()))
                || matches!(event.physical_key, PhysicalKey::Code(code) if code == phys)
        };
        if is_key('q', PhysicalCode::KeyQ) || is_key('w', PhysicalCode::KeyW) {
            self.request_quit(event_loop);
            return true;
        }
        if is_key('m', PhysicalCode::KeyM) {
            if let Some(gpu) = &self.gpu {
                gpu.window.set_minimized(true);
            }
            return true;
        }
        false
    }

    fn shutdown(&mut self) {
        if self.shutdown_done {
            return;
        }
        self.shutdown_done = true;
        let state = self.gpu.as_ref().map(|gpu| {
            let maximized = gpu.window.is_maximized();
            let fullscreen = gpu.window.fullscreen().is_some();
            let normal_size =
                (!maximized && !fullscreen && !gpu.window.is_minimized().unwrap_or(false))
                    .then(|| gpu.logical_size());
            (maximized, fullscreen, normal_size)
        });
        if let Some((maximized, fullscreen, normal_size)) = state {
            if let Some((lw, lh)) = normal_size {
                self.record_normal_window_size(lw, lh);
            }
            let (w, h) = self.last_normal_size;
            self.app.persist_window_state(w, h, maximized, fullscreen);
        }
        let out = self.app.shutdown();
        let _ = self.host.apply_outcome(self.app, &out);
    }
}

impl ApplicationHandler<GuiWake> for GuiApp<'_> {
    fn user_event(&mut self, event_loop: &ActiveEventLoop, _event: GuiWake) {
        let polled = self.poll_background();
        if polled.quit {
            self.shutdown();
            event_loop.exit();
            return;
        }
        if polled.redraw {
            if let Some((lw, lh)) = self.gpu.as_ref().map(WindowGpu::logical_size) {
                self.update_viewport_metrics(lw, lh);
            }
            self.redraw_needed = true;
            self.snapshot_for_redraw = false;
            if !self.is_hidden() {
                if let Some(gpu) = &self.gpu {
                    gpu.window.request_redraw();
                }
            }
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }
        let (init_w, init_h) = self.app.config.window_dimensions();
        self.last_normal_size = (init_w, init_h);
        let mut attrs = icon::apply_window_icon(
            WindowAttributes::default()
                .with_title(self.options.title.clone())
                .with_inner_size(LogicalSize::new(f64::from(init_w), f64::from(init_h)))
                .with_min_inner_size(LogicalSize::new(
                    f64::from(cadence_core::config::MIN_WINDOW_WIDTH),
                    f64::from(cadence_core::config::MIN_WINDOW_HEIGHT),
                )),
        );
        if self.app.config.window_fullscreen {
            attrs = attrs.with_fullscreen(Some(Fullscreen::Borderless(None)));
        } else if self.app.config.window_maximized {
            attrs = attrs.with_maximized(true);
        }

        #[cfg(target_os = "macos")]
        {
            use winit::platform::macos::{OptionAsAlt, WindowAttributesExtMacOS};
            attrs = attrs.with_option_as_alt(OptionAsAlt::Both);
            icon::install_app_icon();
        }

        match event_loop.create_window(attrs) {
            Ok(window) => {
                let window = Arc::new(window);
                match WindowGpu::new(Arc::clone(&window), event_loop) {
                    Ok(gpu) => {
                        let (lw, lh) = gpu.logical_size();
                        self.gpu = Some(gpu);
                        self.update_viewport_metrics(lw, lh);
                        self.redraw_needed = true;
                        window.request_redraw();
                    }
                    Err(err) => {
                        self.error = Some(err);
                        event_loop.exit();
                    }
                }
            }
            Err(err) => {
                self.error = Some(err.into());
                event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                self.request_quit(event_loop);
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods.state();
            }
            WindowEvent::Focused(false) => {
                let release = self.app.release_held();
                let _ = self.host.apply_outcome(self.app, &release);
                self.redraw_needed = true;
            }
            WindowEvent::Focused(true) => {
                self.occluded = false;
                self.surface_occluded = false;
                self.next_surface_probe = None;
                self.redraw_needed = true;
                if let Some(gpu) = &self.gpu {
                    gpu.window.request_redraw();
                }
            }
            WindowEvent::Occluded(occluded) => {
                self.occluded = occluded;
                if !occluded {
                    self.surface_occluded = false;
                    self.next_surface_probe = None;
                    self.redraw_needed = true;
                    if let Some(gpu) = &self.gpu {
                        gpu.window.request_redraw();
                    }
                }
            }
            WindowEvent::Resized(size) => {
                let logical_size = if let Some(gpu) = &mut self.gpu {
                    gpu.resize(size.width, size.height);
                    if size.width > 0 && size.height > 0 {
                        let scale = (gpu.window.scale_factor() as f32).max(1.0);
                        gpu.window.request_redraw();
                        Some((size.width as f32 / scale, size.height as f32 / scale))
                    } else {
                        None
                    }
                } else {
                    None
                };
                if let Some((lw, lh)) = logical_size {
                    self.record_normal_window_size(lw, lh);
                    self.update_viewport_metrics(lw, lh);
                }
                self.redraw_needed = true;
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                let logical_size = if let Some(gpu) = &mut self.gpu {
                    let size = gpu.window.inner_size();
                    gpu.resize(size.width, size.height);
                    gpu.window.request_redraw();
                    let sf = (scale_factor as f32).max(1.0);
                    if size.width > 0 && size.height > 0 {
                        Some((size.width as f32 / sf, size.height as f32 / sf))
                    } else {
                        None
                    }
                } else {
                    None
                };
                if let Some((lw, lh)) = logical_size {
                    self.record_normal_window_size(lw, lh);
                    self.update_viewport_metrics(lw, lh);
                }
                self.redraw_needed = true;
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if self.handle_os_window_shortcut(&event, event_loop) {
                    return;
                }
                let Some(key) = keys::from_winit(&event, self.modifiers) else {
                    return;
                };
                if self.poll_background().quit {
                    self.shutdown();
                    event_loop.exit();
                    return;
                }
                if let Some((lw, lh)) = self.gpu.as_ref().map(WindowGpu::logical_size) {
                    self.update_viewport_metrics(lw, lh);
                }
                match self.app.handle_key(key) {
                    Ok(out) => {
                        if let Err(err) = self.host.apply_outcome(self.app, &out) {
                            self.app.status = format!("Audio: {err:#}");
                        }
                        if out.quit {
                            self.shutdown();
                            event_loop.exit();
                            return;
                        }
                    }
                    Err(err) => self.app.status = format!("{err:#}"),
                }
                // Re-sync metrics after key handling in case zoom/pin/arrange view changed.
                if let Some((lw, lh)) = self.gpu.as_ref().map(WindowGpu::logical_size) {
                    self.update_viewport_metrics(lw, lh);
                }
                self.redraw_needed = true;
                self.snapshot_for_redraw = false;
                if !self.is_hidden() {
                    if let Some(gpu) = &self.gpu {
                        gpu.window.request_redraw();
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                if self.poll_background().quit {
                    self.shutdown();
                    event_loop.exit();
                    return;
                }
                let snapshot = self.take_or_fetch_redraw_snapshot();
                self.observe_transport_and_midi(&snapshot);
                if self.is_hidden() {
                    self.cached_snapshot = Some(snapshot);
                    self.snapshot_from_redraw = true;
                    self.redraw_needed = false;
                    return;
                }

                let frame_view = self.app.frame_view(&snapshot, self.host.audio_status());
                if let Some(gpu) = &mut self.gpu {
                    let scale = (gpu.window.scale_factor() as f32).max(1.0);
                    let logical_w = gpu.surface_config.width as f32 / scale;
                    let logical_h = gpu.surface_config.height as f32 / scale;
                    let active_theme = Theme::from_id(frame_view.chrome.theme);
                    populate_display_list(
                        &mut gpu.display_list,
                        &frame_view,
                        logical_w,
                        logical_h,
                        scale,
                        &active_theme,
                    );

                    match gpu.surface.get_current_texture() {
                        wgpu::CurrentSurfaceTexture::Success(surface_tex)
                        | wgpu::CurrentSurfaceTexture::Suboptimal(surface_tex) => {
                            self.surface_occluded = false;
                            self.next_surface_probe = None;
                            let view = surface_tex
                                .texture
                                .create_view(&wgpu::TextureViewDescriptor::default());
                            if let Err(err) = gpu.renderer.render(
                                &gpu.device,
                                &gpu.queue,
                                &view,
                                &gpu.display_list,
                                &active_theme,
                            ) {
                                self.app.status = format!("GPU: {err:#}");
                            }
                            gpu.queue.present(surface_tex);
                        }
                        wgpu::CurrentSurfaceTexture::Outdated
                        | wgpu::CurrentSurfaceTexture::Lost => {
                            let size = gpu.window.inner_size();
                            gpu.resize(size.width, size.height);
                        }
                        wgpu::CurrentSurfaceTexture::Timeout
                        | wgpu::CurrentSurfaceTexture::Occluded => {
                            self.surface_occluded = true;
                            self.next_surface_probe = Some(Instant::now() + ACTIVE_FRAME_INTERVAL);
                        }
                        wgpu::CurrentSurfaceTexture::Validation => {
                            self.app.status = "GPU surface validation error".into();
                        }
                    }
                }
                self.cached_snapshot = Some(snapshot);
                self.snapshot_from_redraw = true;
                self.next_frame_at = Some(Instant::now() + ACTIVE_FRAME_INTERVAL);
                self.redraw_needed = false;
            }
            _ => {}
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.shutdown();
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.poll_background().quit {
            self.shutdown();
            event_loop.exit();
            return;
        }
        let (snap, fetched_now) = self.take_or_fetch_wait_snapshot();
        let health_changed = fetched_now && self.app.audio_health_alert_changed(&snap.health);
        if health_changed {
            self.redraw_needed = true;
        }
        let active_input = snap.input.capture_phase != cadence_core::runtime::CapturePhase::Idle
            || snap.input.peak[0] > 1e-4
            || snap.input.peak[1] > 1e-4
            || snap.input.peak_hold[0] > 1e-4
            || snap.input.peak_hold[1] > 1e-4
            || (snap.input.status == cadence_core::runtime::AudioInputStatus::Running
                && self.app.midi_focused())
            || self.app.recording.is_active();
        let active_audio = snap.playing
            || snap.peak > 1e-4
            || active_input
            || self.app.is_auditioning()
            || self.app.browser_busy();
        if self.is_hidden() {
            // Avoid 100% CPU busy-spin while occluded or minimized.
            if fetched_now && (active_audio || health_changed) {
                self.observe_transport_and_midi(&snap);
            }
            self.cached_snapshot = Some(snap);
            if active_audio {
                event_loop.set_control_flow(ControlFlow::WaitUntil(
                    Instant::now() + ACTIVE_FRAME_INTERVAL,
                ));
            } else {
                event_loop.set_control_flow(ControlFlow::WaitUntil(
                    Instant::now() + Duration::from_millis(50),
                ));
            }
            return;
        }
        if self.surface_occluded {
            if fetched_now && (active_audio || health_changed) {
                self.observe_transport_and_midi(&snap);
            }
            let now = Instant::now();
            let delay = if active_audio || self.redraw_needed {
                ACTIVE_FRAME_INTERVAL
            } else {
                Duration::from_millis(50)
            };
            let probe_at = self.next_surface_probe.unwrap_or_else(|| now + delay);
            if now >= probe_at {
                let next = now + delay;
                self.next_surface_probe = Some(next);
                self.cached_snapshot = Some(snap);
                self.snapshot_for_redraw = true;
                if let Some(gpu) = &self.gpu {
                    gpu.window.request_redraw();
                }
                event_loop.set_control_flow(ControlFlow::WaitUntil(next));
            } else {
                self.cached_snapshot = Some(snap);
                event_loop.set_control_flow(ControlFlow::WaitUntil(probe_at));
            }
            return;
        }
        let now = Instant::now();
        if self.redraw_needed {
            let next = now + ACTIVE_FRAME_INTERVAL;
            self.next_frame_at = Some(next);
            self.cached_snapshot = Some(snap);
            self.snapshot_for_redraw = true;
            if let Some(gpu) = &self.gpu {
                gpu.window.request_redraw();
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(next));
        } else if active_audio {
            // Smooth ~60 Hz display pacing without pegging a CPU core via `ControlFlow::Poll`.
            let target = self.next_frame_at.unwrap_or(now);
            if now >= target {
                let next = now + ACTIVE_FRAME_INTERVAL;
                self.next_frame_at = Some(next);
                self.cached_snapshot = Some(snap);
                self.snapshot_for_redraw = true;
                if let Some(gpu) = &self.gpu {
                    gpu.window.request_redraw();
                }
                event_loop.set_control_flow(ControlFlow::WaitUntil(next));
            } else {
                self.cached_snapshot = Some(snap);
                event_loop.set_control_flow(ControlFlow::WaitUntil(target));
            }
        } else {
            self.next_frame_at = None;
            self.cached_snapshot = Some(snap);
            // Near-zero idle CPU/GPU wakeups when stopped and silent.
            event_loop.set_control_flow(ControlFlow::WaitUntil(now + Duration::from_millis(120)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadence_core::material::Project;
    use std::cell::Cell;
    use std::path::PathBuf;

    struct CountingBridge {
        snapshots: Cell<usize>,
        playing: bool,
    }

    impl HostBridge for CountingBridge {
        fn snapshot(&self) -> AudioSnapshot {
            self.snapshots.set(self.snapshots.get() + 1);
            AudioSnapshot {
                playing: self.playing,
                sample_rate: 48_000,
                ..AudioSnapshot::default()
            }
        }

        fn apply_outcome(&mut self, _app: &mut App, _outcome: &Outcome) -> Result<()> {
            Ok(())
        }

        fn poll_midi(&mut self) -> Vec<MidiMessage> {
            Vec::new()
        }

        fn audio_status(&self) -> &str {
            "Test · 48 kHz"
        }
    }

    #[test]
    fn redraw_and_wait_cache_snapshot_without_duplicate_host_queries() {
        let mut app = App::new(Project::demo(), PathBuf::from("gui-cache.json")).unwrap();
        let mut bridge = CountingBridge {
            snapshots: Cell::new(0),
            playing: true,
        };
        let opts = GuiOptions::default();
        let mut runner = GuiApp {
            app: &mut app,
            host: &mut bridge,
            options: &opts,
            modifiers: ModifiersState::empty(),
            gpu: None,
            last_normal_size: (1440, 900),
            shutdown_done: false,
            redraw_needed: true,
            occluded: false,
            surface_occluded: false,
            next_surface_probe: None,
            next_frame_at: None,
            cached_snapshot: None,
            snapshot_for_redraw: false,
            snapshot_from_redraw: false,
            error: None,
        };

        // 1. RedrawRequested fetches once, caches, and marks `snapshot_from_redraw`.
        let s1 = runner.take_or_fetch_redraw_snapshot();
        runner.cached_snapshot = Some(s1);
        runner.snapshot_from_redraw = true;
        assert_eq!(runner.host.snapshot().sample_rate, 48_000); // baseline check: now 2
                                                                // Reset counter via fresh cycle: wait reuses redraw snapshot without querying host.
        let before = runner.snapshot_from_redraw;
        assert!(before);
        let (wait_snap, fetched_now) = runner.take_or_fetch_wait_snapshot();
        assert!(!fetched_now);
        assert!(wait_snap.playing);

        // 2. When wait requests a redraw, it caches with `snapshot_for_redraw = true`,
        //    so RedrawRequested consumes the cached snapshot without re-querying host.
        runner.cached_snapshot = Some(wait_snap);
        runner.snapshot_for_redraw = true;
        let redraw_snap = runner.take_or_fetch_redraw_snapshot();
        assert!(redraw_snap.playing);
        assert!(!runner.snapshot_for_redraw);
        drop(runner);
        assert_eq!(bridge.snapshots.get(), 2);
    }
}
