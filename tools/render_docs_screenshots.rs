//! Headless Metal/wgpu GPU documentation screenshot generator (1440×860).
//!
//! Renders all 18 flagship documentation screenshots from the real MIT showcase
//! projects (`examples/demo/project.json` — Afterglow,
//! `examples/iron-circuit/project.json` — Iron Circuit, and
//! `examples/fivefold-live/project.json` + `takes/set.take.jsonl` — Fivefold Live),
//! backed by real repo-relative MIT factory samples, external `presets/` catalog,
//! and live 48 kHz engine telemetry computed directly via
//! `cadence_engine::audio::render_schedule_f32` and
//! `cadence_engine::audio::render_rack_activity_snapshot` (channel peaks, headroom
//! flags, Compressor/Limiter gain reduction, Patchbay module/cable activity, and
//! oscilloscope traces).
//!
//! Usage:
//! - Write all 18 PNGs to `docs/screenshots/`:
//!   `cargo run --no-default-features --features gui --example render-docs-screenshots`
//! - Verify all 18 scenes render cleanly without modifying `docs/screenshots/`:
//!   `cargo run --no-default-features --features gui --example render-docs-screenshots -- --check`
use cadence_app::view::{
    DeckView, DevicesTab, EffectVisualView, FrameView, GraphTarget, InstrumentVisualView,
    ModuleVisualView, OverlayView, PatternContentView, RackTab, UiModeKind,
};
use cadence_app::{Action, App, BankMode};
use cadence_core::assets::load_project_samples;
use cadence_core::config::ThemeId;
use cadence_core::dsp::ideal_waveform;
use cadence_core::material::{
    Boundary, Modulation, ModulationTarget, Pattern, PatternKind, Project, ValuePoint, ValueUnit,
};
use cadence_core::midi_map::{MidiMapping, MidiPadMode, MidiSource, MidiTarget, MidiValueMode};
use cadence_core::mixer::{ChannelParam, ChannelRef, DriveMode};
use cadence_core::params::{ParamAddress, ValueMode};
use cadence_core::presets::PresetCatalog;
use cadence_core::rack::{
    DriveModule, InPort, LfoModule, LfoShape, LoFiModule, ModuleKind, PatchCable, RackModule,
    VcoModule, Waveform,
};
use cadence_core::runtime::{
    AudioDeviceInfo, AudioErrorCode, AudioHealth, AudioInputMode, AudioInputStatus,
    AudioInputTelemetry, AudioInventory, AudioSnapshot, AudioStreamStatus, CapturePhase,
    ChannelMeter, InputMonitorMode, MidiMessage,
};
use cadence_core::sampler::{load_sample_file, SampleLibrary};
use cadence_engine::audio::{render_rack_activity_snapshot, render_schedule_f32, RenderReport};
use cadence_gui::headless::{distinct_pixel_ratio, save_rgba8_png, HeadlessGpu};
use cadence_gui::theme::Theme;
use std::path::PathBuf;

const SHOT_W: u32 = 1440;
const SHOT_H: u32 = 860;
const TELEMETRY_SR: u32 = 48_000;
const AUDIO_STATUS: &str = "Metal · 48 kHz";

struct LiveTelemetry {
    report: RenderReport,
    main_wave: Vec<f32>,
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn load_mit_project_app(rel_path: &str) -> App {
    let path = repo_root().join(rel_path);
    let project = cadence_app::load_project(&path)
        .unwrap_or_else(|e| panic!("failed to load {rel_path}: {e:#}"));
    // Use `from_roots(&[], Some(&path))` so only repo-relative MIT samples load (never `./samples/`).
    let library = SampleLibrary::from_roots(&[], Some(&path));
    let app = App::new_with_library(project, path, library)
        .unwrap_or_else(|e| panic!("failed to initialize App for {rel_path}: {e:#}"));
    assert!(
        app.ready(),
        "{rel_path} must resolve all MIT repo-relative samples cleanly"
    );
    app
}

/// Select a lively 256-sample oscilloscope trace from the rendered 32-bit float
/// stereo frames near `focus_step` (e.g. Drop 1 / Overload bar 26 beat 2).
fn extract_scope_wave(frames: &[[f32; 2]], bpm: f64, focus_step: u64) -> Vec<f32> {
    let step_frames = ((60.0 / bpm / 4.0) * f64::from(TELEMETRY_SR)).round() as usize;
    let window_start = (focus_step as usize)
        .saturating_mul(step_frames)
        .min(frames.len().saturating_sub(256));
    let window_end = (window_start + step_frames * 16).min(frames.len());
    let slice = &frames[window_start..window_end];

    let mut best_off = 0usize;
    let mut best_score = f32::NEG_INFINITY;
    for off in (0..=slice.len().saturating_sub(256)).step_by(32) {
        let seg = &slice[off..off + 256];
        let mut mx = 0.0_f32;
        let mut mn = 0.0_f32;
        let mut zc = 0usize;
        let mut mean = 0.0_f32;
        let mut first_half_mean = 0.0_f32;
        let mut second_half_mean = 0.0_f32;
        for (i, lr) in seg.iter().enumerate() {
            let s = 0.5 * (lr[0] + lr[1]);
            mean += s;
            if i < 128 {
                first_half_mean += s;
            } else {
                second_half_mean += s;
            }
            mx = mx.max(s);
            mn = mn.min(s);
            if i > 0 {
                let prev = 0.5 * (seg[i - 1][0] + seg[i - 1][1]);
                if (prev <= 0.0 && s > 0.0) || (prev >= 0.0 && s < 0.0) {
                    zc += 1;
                }
            }
        }
        mean /= 256.0;
        let tilt = (first_half_mean - second_half_mean).abs() / 128.0;
        let balanced_peak = mx.min(-mn);
        if balanced_peak < 0.30 {
            continue;
        }
        let zc_score = if (10..=28).contains(&zc) {
            1.6 + (zc.min(20) as f32) * 0.05
        } else {
            (zc as f32) * 0.03
        };
        let score = balanced_peak * 1.4 + zc_score - mean.abs() * 2.5 - tilt * 2.0;
        if score > best_score {
            best_score = score;
            best_off = off;
        }
    }

    slice[best_off..best_off + 256]
        .iter()
        .map(|lr| (0.5 * (lr[0] + lr[1])).clamp(-0.98, 0.98))
        .collect()
}

/// Render `start_step..start_step + render_steps` of a showcase project through
/// the real 48 kHz audio engine so screenshot channel meters, GR bars, and
/// oscilloscope traces stay automatically synced with the live mixes.
fn capture_live_telemetry(
    rel_path: &str,
    start_step: u64,
    render_steps: u64,
    focus_step: u64,
) -> LiveTelemetry {
    let path = repo_root().join(rel_path);
    let project = cadence_app::load_project(&path)
        .unwrap_or_else(|e| panic!("failed to load {rel_path}: {e:#}"));
    let compiled = project
        .compile()
        .unwrap_or_else(|e| panic!("failed to compile {rel_path}: {e:#}"));
    let mut library = SampleLibrary::from_roots(&[], Some(&path));
    let samples = load_project_samples(&compiled, &mut library);
    samples
        .ensure_ready()
        .unwrap_or_else(|e| panic!("failed to load MIT samples for {rel_path}: {e:#}"));

    let (report, frames) = render_schedule_f32(
        &[(0, &compiled)],
        start_step,
        render_steps,
        samples.bank,
        TELEMETRY_SR,
    )
    .unwrap_or_else(|e| panic!("failed to render telemetry for {rel_path}: {e:#}"));

    for ch in &report.channels {
        assert!(
            !ch.clipped && !ch.hot,
            "{rel_path}: showcase channel {:?} exceeded 0 dBFS headroom (hot={}, clipped={}, in={:?}, out={:?})",
            ch.name,
            ch.hot,
            ch.clipped,
            ch.input_peak_hold,
            ch.output_peak_hold
        );
    }

    let rel_focus = focus_step.saturating_sub(start_step);
    let main_wave = extract_scope_wave(&frames, project.bpm, rel_focus);
    LiveTelemetry { report, main_wave }
}

/// Extract a 256-sample dry scope waveform directly from a repo-relative MIT WAV sample.
fn decode_mit_sample_scope(rel_wav: &str) -> Vec<f32> {
    let path = repo_root().join(rel_wav);
    let data = load_sample_file(&path, None)
        .unwrap_or_else(|e| panic!("failed to decode MIT sample {rel_wav}: {e:#}"));
    let window = data.frames.clamp(1, 1024);
    let mut peak = 0.0_f32;
    let mut out = Vec::with_capacity(256);
    for i in 0..256 {
        let idx = (i * window / 256).min(data.left.len().saturating_sub(1));
        let s = data.left[idx];
        peak = peak.max(s.abs());
        out.push(s);
    }
    if peak > 1e-4 {
        let scale = (0.86 / peak).min(1.8);
        for s in &mut out {
            *s = (*s * scale).clamp(-0.96, 0.96);
        }
    }
    out
}

/// Synthesize a realistic dry scope trace for Anthem Lead (detuned Saw + Pulse + Sub)
/// directly via `cadence_core::dsp::ideal_waveform`.
fn anthem_lead_dry_scope() -> Vec<f32> {
    (0..256)
        .map(|i| {
            let phase = (i as f32 / 256.0) * 3.0;
            let osc_a = ideal_waveform(Waveform::Saw, phase % 1.0, 0.5) * 0.50;
            let osc_b = ideal_waveform(Waveform::Pulse, (phase * 1.008 + 0.17) % 1.0, 0.38) * 0.36;
            let sub = ideal_waveform(Waveform::Sine, (phase * 0.5) % 1.0, 0.5) * 0.14;
            (osc_a + osc_b + sub).clamp(-0.95, 0.95)
        })
        .collect()
}

/// Project engine-measured `LiveTelemetry` into an `AudioSnapshot` at `step`.
fn snapshot_from_telemetry(
    telemetry: &LiveTelemetry,
    project: &Project,
    step: u64,
    step_fraction: f32,
    instrument_scopes: &[(&str, &[f32])],
) -> AudioSnapshot {
    let mut meters = Vec::with_capacity(telemetry.report.channels.len());
    let mut master_live = [0.75_f32, 0.75_f32];

    for ch in &telemetry.report.channels {
        let in_h = ch.input_peak_hold;
        let out_h = ch.output_peak_hold;
        let meter = ChannelMeter {
            channel: ch.channel,
            input_peak: [in_h[0] * 0.88, in_h[1] * 0.88],
            input_peak_hold: in_h,
            output_peak: [out_h[0] * 0.90, out_h[1] * 0.90],
            output_peak_hold: out_h,
            hot: ch.hot,
            clipped: ch.clipped,
        };
        if ch.channel == ChannelRef::Master {
            master_live = meter.output_peak;
        }
        meters.push(meter);
    }

    let mut track_ids = [0u64; 16];
    let mut track_waveforms = Vec::with_capacity(project.instruments.len());
    for (i, inst) in project.instruments.iter().take(16).enumerate() {
        track_ids[i] = inst.id;
        let w = instrument_scopes
            .iter()
            .find(|(name, _)| *name == inst.name)
            .map(|(_, wave)| wave.to_vec())
            .unwrap_or_else(|| telemetry.main_wave.clone());
        track_waveforms.push(w);
    }

    let mut snap = AudioSnapshot {
        playing: true,
        absolute_step: step,
        step_fraction,
        sample_rate: TELEMETRY_SR,
        peak: master_live[0].max(master_live[1]),
        peak_left: master_live[0],
        peak_right: master_live[1],
        waveform: telemetry.main_wave.clone(),
        track_count: project.instruments.len().min(16),
        track_ids,
        track_waveforms,
        channel_meters: meters,
        next_step: step + 1,
        health: AudioHealth {
            status: AudioStreamStatus::Running,
            cpu_load: 0.08,
            peak_cpu_load: 0.14,
            buffer_frames: 256,
            channels: 2,
            bit_depth: 32,
            sample_format_is_float: true,
            ..AudioHealth::default()
        },
        ..AudioSnapshot::default()
    };

    for fx in &telemetry.report.effects {
        if fx.max_gr_db > 0.05 {
            let live_gr = (fx.max_gr_db * 0.65).max(fx.avg_gr_db);
            snap.set_effect_meter(fx.device_id, live_gr, fx.max_gr_db);
        }
    }

    snap
}

/// Build and validate a balanced `DisplayList`, render on `HeadlessGpu`, assert
/// non-blank output, and write `docs/screenshots/<filename>` unless running in
/// `--check` mode.
fn render_and_verify(
    gpu: &mut HeadlessGpu,
    filename: &str,
    frame: &FrameView,
    theme: &Theme,
    write_pngs: bool,
) -> Vec<u8> {
    let rgba = gpu
        .render_rgba8(frame, theme)
        .unwrap_or_else(|e| panic!("GPU render failed for {filename}: {e:#}"));
    let ratio = distinct_pixel_ratio(&rgba, theme.surface.canvas);
    assert!(
        ratio > 0.20,
        "{filename}: rendered too few non-canvas pixels ({ratio:.3})"
    );

    if write_pngs {
        let out_path = repo_root().join("docs/screenshots").join(filename);
        save_rgba8_png(&out_path, SHOT_W, SHOT_H, &rgba)
            .unwrap_or_else(|e| panic!("failed to write {}: {e:#}", out_path.display()));
    }

    rgba
}

fn main() {
    let check_only = std::env::args().any(|arg| arg == "--check")
        || std::env::var("CADENCE_WRITE_DOCS_SCREENSHOTS")
            .ok()
            .as_deref()
            == Some("0");
    let write_pngs = !check_only;
    let Ok(mut gpu) = HeadlessGpu::new(SHOT_W, SHOT_H) else {
        eprintln!("No headless GPU adapter available; skipping screenshot render.");
        return;
    };

    // Capture live 48 kHz Drop 1 / Overload telemetry from both showcases in parallel
    // (Bars 25..28, steps 384..448, centered around Bar 26.2 step 404).
    let (afterglow_telemetry, iron_telemetry) = std::thread::scope(|s| {
        let h_demo = s.spawn(|| capture_live_telemetry("examples/demo/project.json", 384, 64, 404));
        let h_iron =
            s.spawn(|| capture_live_telemetry("examples/iron-circuit/project.json", 384, 64, 404));
        (h_demo.join().unwrap(), h_iron.join().unwrap())
    });

    let lead_wave = anthem_lead_dry_scope();
    let edm_kit_wave = decode_mit_sample_scope("factory-samples/edm-kit/02-sd.wav");
    let tr808_wave = decode_mit_sample_scope("factory-samples/tr-808/09-cb.wav");
    let tr909_wave = decode_mit_sample_scope("factory-samples/tr-909/02-sd.wav");

    let afterglow_scopes: [(&str, &[f32]); 2] = [
        ("Anthem Lead", lead_wave.as_slice()),
        ("EDM Kit", edm_kit_wave.as_slice()),
    ];
    let iron_scopes: [(&str, &[f32]); 2] = [
        ("TR-808", tr808_wave.as_slice()),
        ("TR-909", tr909_wave.as_slice()),
    ];

    let dark = Theme::cadence_dark();
    let light = Theme::studio_light();
    let neon = Theme::from_id(ThemeId::MidnightNeon);
    let mut rendered: Vec<(&'static str, Vec<u8>)> = Vec::with_capacity(18);

    // -------------------------------------------------------------------------
    // 1. 01-arrange.png — Afterglow (Dark): Arrange + Bank + 32-bit Float Output (Drop 1)
    // -------------------------------------------------------------------------
    {
        let mut app = load_mit_project_app("examples/demo/project.json");
        app.view_state.bank_mode = BankMode::Parts;
        app.arrange_ui.zoom = 1.0;
        let step = 404; // Bar 26.2.1 — heart of Drop 1
        app.arrange_ui.step = step as u32;
        app.press_keys("tf").unwrap(); // Enable Follow so Arrange tracks Drop 1 playhead cleanly
        app.observe_transport(step, step + 1, true, false);
        let snap = snapshot_from_telemetry(
            &afterglow_telemetry,
            app.project(),
            step,
            0.35,
            &afterglow_scopes,
        );
        let fv = app.frame_view(&snap, AUDIO_STATUS);
        assert_eq!(fv.chrome.mode_kind, UiModeKind::Normal);
        assert!(fv
            .decks
            .iter()
            .any(|d| matches!(d.view, DeckView::Arrange(_))));
        assert!(fv.decks.iter().any(|d| matches!(d.view, DeckView::Bank(_))));
        assert!(fv.decks.iter().any(|d| matches!(
            &d.view,
            DeckView::Output(o) if matches!(o.master_gr_tag, Some("CMP" | "LIM")) && !o.waveform.is_empty()
        )));
        let px = render_and_verify(&mut gpu, "01-arrange.png", &fv, &dark, write_pngs);
        rendered.push(("01-arrange.png", px));
    }

    // -------------------------------------------------------------------------
    // 2. 02-signal-graph.png — Iron Circuit (Dark): Zoomed Signal Graph Patchbay (gG, zz)
    // -------------------------------------------------------------------------
    {
        let mut app = load_mit_project_app("examples/iron-circuit/project.json");
        let step = 404; // Bar 26 — Overload
        app.observe_transport(step, step + 1, true, false);
        let snap =
            snapshot_from_telemetry(&iron_telemetry, app.project(), step, 0.35, &iron_scopes);
        app.press_keys("gGzz").unwrap();

        // Select the `Rhythm` Group bus in MIXER / FX so TR-808, TR-909, nested
        // Percussion -> Rhythm -> Master DAG, FX inserts, and Macros all illuminate together.
        let rhythm_ch_id = app
            .project()
            .mixer
            .channels
            .iter()
            .find(|c| c.name == "Rhythm")
            .expect("Rhythm group channel in Iron Circuit")
            .id;
        let fv_probe = app.frame_view(&snap, AUDIO_STATUS);
        let gv = fv_probe
            .decks
            .iter()
            .find_map(|d| match &d.view {
                DeckView::Graph(g) => Some(g),
                _ => None,
            })
            .expect("GraphDeckView");
        let mut found = false;
        for (c, col) in gv.columns.iter().enumerate() {
            for (r, node) in col.nodes.iter().enumerate() {
                if node.target == GraphTarget::Channel(ChannelRef::Channel(rhythm_ch_id)) {
                    app.view_state.graph_col = c;
                    app.view_state.graph_row = r;
                    found = true;
                }
            }
        }
        assert!(found, "Rhythm channel node must appear in Signal Graph");

        let fv = app.frame_view(&snap, AUDIO_STATUS);
        let px = render_and_verify(&mut gpu, "02-signal-graph.png", &fv, &dark, write_pngs);
        rendered.push(("02-signal-graph.png", px));
    }

    // -------------------------------------------------------------------------
    // 3. 03-mixer.png — Afterglow (Dark): Zoomed 32-bit Float Mixer Console (gm, zz)
    // -------------------------------------------------------------------------
    {
        let demo_path = repo_root().join("examples/demo/project.json");
        let mut project = cadence_app::load_project(&demo_path).expect("load Afterglow");
        // Add live Drop 1 ChannelGain & ChannelPan rides in-memory so the console showcases
        // glowing automated ghost fader & pan needles alongside real 32f headroom & GR meters.
        let mut next_id = project.max_id() + 1;
        let find_ch = |p: &Project, name: &str| {
            p.mixer
                .channels
                .iter()
                .find(|c| c.name == name)
                .unwrap_or_else(|| panic!("channel {name} missing"))
                .id
        };
        let chords_ch = find_ch(&project, "Chords");
        let arp_ch = find_ch(&project, "Arp");
        let bell_ch = find_ch(&project, "Bell");

        let gain_pat = next_id;
        next_id += 1;
        let gain_mod = next_id;
        next_id += 1;
        project.patterns.push(Pattern {
            id: gain_pat,
            name: "Drop Ride".into(),
            kind: PatternKind::Values {
                unit: ValueUnit::Normalized,
                period: 64,
                mode: ValueMode::Ramp,
                points: vec![
                    ValuePoint {
                        step: 0,
                        value: 0.72,
                    },
                    ValuePoint {
                        step: 20,
                        value: 0.62,
                    },
                    ValuePoint {
                        step: 48,
                        value: 0.94,
                    },
                ],
            },
        });
        project.modulations.push(Modulation {
            id: gain_mod,
            name: "Chords Ride".into(),
            pattern: gain_pat,
            target: ModulationTarget::ChannelGain {
                channel: ChannelRef::Channel(chords_ch),
            },
        });

        for (ch_id, name, v0, v1) in [
            (arp_ch, "Arp Drift", 0.22_f32, 0.72_f32),
            (bell_ch, "Bell Drift", 0.78_f32, 0.28_f32),
        ] {
            let pat_id = next_id;
            next_id += 1;
            let mod_id = next_id;
            next_id += 1;
            project.patterns.push(Pattern {
                id: pat_id,
                name: name.into(),
                kind: PatternKind::Values {
                    unit: ValueUnit::Normalized,
                    period: 64,
                    mode: ValueMode::Ramp,
                    points: vec![
                        ValuePoint { step: 0, value: v0 },
                        ValuePoint {
                            step: 20,
                            value: v1,
                        },
                        ValuePoint {
                            step: 48,
                            value: v0,
                        },
                    ],
                },
            });
            project.modulations.push(Modulation {
                id: mod_id,
                name: name.into(),
                pattern: pat_id,
                target: ModulationTarget::ChannelPan {
                    channel: ChannelRef::Channel(ch_id),
                },
            });
        }

        let library = SampleLibrary::from_roots(&[], Some(&demo_path));
        let mut app = App::new_with_library(project, demo_path, library).unwrap();
        let step = 404; // Bar 26.2.1 (404 % 64 = 20 -> strong visible ghost fader & pan offsets!)
        app.observe_transport(step, step + 1, true, false);
        let snap = snapshot_from_telemetry(
            &afterglow_telemetry,
            app.project(),
            step,
            0.35,
            &afterglow_scopes,
        );
        app.press_keys("gmzz").unwrap();
        app.selected_channel = ChannelRef::Master;

        let fv = app.frame_view(&snap, AUDIO_STATUS);
        let mv = fv
            .decks
            .iter()
            .find_map(|d| match &d.view {
                DeckView::Mixer(m) => Some(m),
                _ => None,
            })
            .expect("MixerDeckView");
        assert!(
            mv.channels.iter().any(|c| c.effective_gain.is_some()),
            "Mixer must show live automated ghost fader"
        );
        assert!(
            mv.channels.iter().any(|c| c.effective_pan.is_some()),
            "Mixer must show live automated ghost pan"
        );
        assert!(
            mv.channels.iter().any(|c| c.gr_tag == Some("CMP"))
                && mv.channels.iter().filter(|c| c.gr_tag.is_some()).count() >= 3,
            "Mixer must show active dynamics GR micro-meters across Drums, Music, and Master"
        );
        let px = render_and_verify(&mut gpu, "03-mixer.png", &fv, &dark, write_pngs);
        rendered.push(("03-mixer.png", px));
    }

    // -------------------------------------------------------------------------
    // 4. 04-rack-zones.png — Iron Circuit (Dark): TR-808 2D Key×Velocity Zones Map & Inspector
    // -------------------------------------------------------------------------
    {
        let mut app = load_mit_project_app("examples/iron-circuit/project.json");
        let step = 460; // Overload
        app.observe_transport(step, step + 1, true, false);
        let snap =
            snapshot_from_telemetry(&iron_telemetry, app.project(), step, 0.30, &iron_scopes);
        app.execute_command("instrument TR-808").unwrap();
        app.execute_command("zone select CH").unwrap();
        app.rack_ui.tab = RackTab::Zones;
        app.press_keys("zz").unwrap();
        // Focus the ZONE Choke Group / Zone Gain inspector row cleanly.
        let zone_row = app
            .frame_view(&snap, AUDIO_STATUS)
            .decks
            .iter()
            .find_map(|d| match &d.view {
                DeckView::Instrument(iv) => iv.controls.iter().position(|c| c.label == "Zone Gain"),
                _ => None,
            })
            .unwrap_or(0);
        app.control_cursor = zone_row;

        let fv = app.frame_view(&snap, AUDIO_STATUS);
        let iv = fv
            .decks
            .iter()
            .find_map(|d| match &d.view {
                DeckView::Instrument(iv) => Some(iv),
                _ => None,
            })
            .expect("InstrumentDeckView for TR-808");
        match &iv.visual {
            InstrumentVisualView::Rack(rack) => {
                assert_eq!(rack.zones.len(), 16, "TR-808 must render all 16 MIT zones");
                let sel = rack.zones.iter().find(|z| z.selected).expect("selected CH");
                assert_eq!(sel.name, "CH");
                assert_eq!(sel.choke_group, Some(1));
                assert!(
                    !rack.live_waveform.is_empty(),
                    "TR-808 Dry Scope must be active"
                );
            }
            other => panic!("expected Rack visual, got {other:?}"),
        }
        let px = render_and_verify(&mut gpu, "04-rack-zones.png", &fv, &dark, write_pngs);
        rendered.push(("04-rack-zones.png", px));
    }

    // -------------------------------------------------------------------------
    // 5. 05-rack-panel.png — Afterglow (Dark): Anthem Lead Eurorack Quick Panel (gi, zz)
    // -------------------------------------------------------------------------
    {
        let mut app = load_mit_project_app("examples/demo/project.json");
        let step = 368; // Build 1 peak: M1 Filter Lift = 0.94 sweeps Anthem Lead cutoff visibly bright!
        app.observe_transport(step, step + 1, true, false);
        let snap = snapshot_from_telemetry(
            &afterglow_telemetry,
            app.project(),
            step,
            0.40,
            &afterglow_scopes,
        );
        app.execute_command("instrument \"Anthem Lead\"").unwrap();
        app.rack_ui.tab = RackTab::Panel;
        app.rack_ui.panel_slot = 0;
        app.press_keys("zz").unwrap();
        // Highlight the live-modulated Cutoff row in the balanced 2-column Overview layout.
        let cutoff_row = app
            .frame_view(&snap, AUDIO_STATUS)
            .decks
            .iter()
            .find_map(|d| match &d.view {
                DeckView::Instrument(iv) => iv.controls.iter().position(|c| c.label == "Cutoff"),
                _ => None,
            })
            .unwrap_or(0);
        app.control_cursor = cutoff_row;

        let fv = app.frame_view(&snap, AUDIO_STATUS);
        assert_eq!(fv.chrome.mode_kind, UiModeKind::Normal);
        assert_eq!(fv.chrome.mode_name, "NORMAL");
        let iv = fv
            .decks
            .iter()
            .find_map(|d| match &d.view {
                DeckView::Instrument(iv) => Some(iv),
                _ => None,
            })
            .expect("InstrumentDeckView for Anthem Lead");
        match &iv.visual {
            InstrumentVisualView::Rack(rack) => match &rack.visual {
                ModuleVisualView::PinnedOverview {
                    filter_mod_cutoff_norm,
                    live_waveform,
                    ..
                } => {
                    assert!(
                        filter_mod_cutoff_norm.is_some(),
                        "Anthem Lead must display glowing live modulated cutoff sweep"
                    );
                    assert!(!live_waveform.is_empty(), "Dry Scope must be active");
                }
                other => panic!("expected PinnedOverview visual, got {other:?}"),
            },
            other => panic!("expected Rack visual, got {other:?}"),
        }
        let px = render_and_verify(&mut gpu, "05-rack-panel.png", &fv, &dark, write_pngs);
        rendered.push(("05-rack-panel.png", px));
    }

    // -------------------------------------------------------------------------
    // 6. 06-fx-filter-pcf.png — Iron Circuit (Dark): Gate Pad rhythmic Filter PCF visualizer
    // -------------------------------------------------------------------------
    {
        let mut app = load_mit_project_app("examples/iron-circuit/project.json");
        let step = 598; // Voltage Drop peak: M6 Pad Gate active sweep + Pad Gate Groove step 7 accent (vel 1.0)
        app.observe_transport(step, step + 1, true, false);
        let snap =
            snapshot_from_telemetry(&iron_telemetry, app.project(), step, 0.25, &iron_scopes);
        let pad_gate_id = app
            .project()
            .mixer
            .channels
            .iter()
            .flat_map(|c| c.inserts.iter())
            .find(|d| d.name == "Pad Gate")
            .expect("Pad Gate rhythmic Filter insert in Iron Circuit")
            .id;
        app.execute_command(&format!("fx {pad_gate_id}")).unwrap();
        if !app.layout().zoomed() {
            app.press_keys("zz").unwrap();
        }
        app.control_cursor = 1; // Focus Cutoff (live-modulated parameter)

        let fv = app.frame_view(&snap, AUDIO_STATUS);
        let ev = fv
            .decks
            .iter()
            .find_map(|d| match &d.view {
                DeckView::Effect(ev) => Some(ev),
                _ => None,
            })
            .expect("EffectDeckView for Pad Gate");
        match &ev.visual {
            EffectVisualView::Filter {
                mod_cutoff_norm,
                env,
                ..
            } => {
                assert!(
                    mod_cutoff_norm.is_some(),
                    "Pad Gate must show live modulated cutoff sweep"
                );
                let env = env.as_ref().expect("Pad Gate must show 16-step FilterEnv");
                assert_eq!(env.steps.len(), 16);
            }
            other => panic!("expected Filter visual, got {other:?}"),
        }
        let px = render_and_verify(&mut gpu, "06-fx-filter-pcf.png", &fv, &dark, write_pngs);
        rendered.push(("06-fx-filter-pcf.png", px));
    }

    // -------------------------------------------------------------------------
    // 7. 07-macros-deck.png — Iron Circuit (Dark): All 8 Assignable Project Macros (gk, zz)
    // -------------------------------------------------------------------------
    {
        let mut app = load_mit_project_app("examples/iron-circuit/project.json");
        let step = 502; // Bar 32 Overload turnaround: M1, M2, M3, M4, M6 all sweep visibly from base!
        app.observe_transport(step, step + 1, true, false);
        let snap =
            snapshot_from_telemetry(&iron_telemetry, app.project(), step, 0.30, &iron_scopes);
        // Select M6 Pad Gate — M6 has Hz->kHz, oct, ms, INV badge + live MOD sweep!
        app.press_keys("gk6zz").unwrap();

        let fv = app.frame_view(&snap, AUDIO_STATUS);
        let mv = fv
            .decks
            .iter()
            .find_map(|d| match &d.view {
                DeckView::Macros(m) => Some(m),
                _ => None,
            })
            .expect("MacrosDeckView");
        assert_eq!(mv.active_count, 8, "Iron Circuit must show all 8 macros");
        assert!(
            mv.slots
                .iter()
                .filter(|s| s.effective_value.is_some())
                .count()
                >= 5,
            "At least 5 Iron Circuit macros must display live automated sweeps"
        );
        let px = render_and_verify(&mut gpu, "07-macros-deck.png", &fv, &dark, write_pngs);
        rendered.push(("07-macros-deck.png", px));
    }

    // -------------------------------------------------------------------------
    // 8. 08-pattern-mini.png — Afterglow (Dark): Pattern Deck Bell Mini (:mini, gp, zz)
    // -------------------------------------------------------------------------
    {
        let mut app = load_mit_project_app("examples/demo/project.json");
        let step = 518; // Breakdown Bar 33 where Bell Mini plays
        app.observe_transport(step, step + 1, true, false);
        let snap = snapshot_from_telemetry(
            &afterglow_telemetry,
            app.project(),
            step,
            0.35,
            &afterglow_scopes,
        );
        app.execute_command("open \"Bell Mini\"").unwrap();
        app.press_keys("gpzz").unwrap();

        let fv = app.frame_view(&snap, AUDIO_STATUS);
        let pv = fv
            .decks
            .iter()
            .find_map(|d| match &d.view {
                DeckView::Pattern(p) => Some(p),
                _ => None,
            })
            .expect("PatternDeckView for Bell Mini");
        assert_eq!(pv.name, "Bell Mini");
        assert!(
            matches!(pv.content, PatternContentView::Notes { .. }),
            "Bell Mini must project evaluated fractional sub-step notes"
        );
        let px = render_and_verify(&mut gpu, "08-pattern-mini.png", &fv, &dark, write_pngs);
        rendered.push(("08-pattern-mini.png", px));
    }

    // -------------------------------------------------------------------------
    // 9. 09-jam-perform.png — Afterglow (Dark): Jam Mode + Lane-Layer Card + 13-key HUD
    // -------------------------------------------------------------------------
    {
        let mut app = load_mit_project_app("examples/demo/project.json");
        app.view_state.bank_mode = BankMode::Parts;
        let step = 404; // Bar 26 Drop 1
        app.arrange_ui.step = step as u32;
        app.press_keys("tf").unwrap(); // Follow on so Drop 1 + playhead frame the live Jam surface

        // Launch a live override on Bell (`Mini Shimmer`) on Drop 1 downbeat (Bar 25.1.1)
        // where Bell has open arrangement space so the hatched LIVE override bar reads crisply!
        app.observe_transport(384, 385, true, false);
        if let Some(bell_lane) = app
            .project()
            .lanes
            .iter()
            .find(|l| l.name == "Bell")
            .cloned()
        {
            if let Some(&bell_part) = bell_lane.parts.first() {
                app.apply(Action::Launch {
                    lane: bell_lane.id,
                    part: bell_part,
                    at: Some(Boundary::Now),
                })
                .unwrap();
            }
        }
        app.observe_transport(step, step + 1, true, false);
        let snap = snapshot_from_telemetry(
            &afterglow_telemetry,
            app.project(),
            step,
            0.35,
            &afterglow_scopes,
        );
        // Enter Jam Mode (`gj`), open floating Lead Lane-Layer card (`q`), and hold an
        // F-minor triad (F4=65, Ab4=68, C5=72) so white & black keys glow on the 13-key HUD!
        app.press_keys("gjq").unwrap();
        for note in [65_u8, 68, 72] {
            app.handle_midi(MidiMessage::NoteOn {
                channel: 0,
                note,
                velocity: 108,
            })
            .unwrap();
        }

        let fv = app.frame_view(&snap, AUDIO_STATUS);
        assert_eq!(fv.chrome.mode_kind, UiModeKind::Jam);
        assert!(fv.jam_hud.is_some(), "Jam HUD must be visible");
        assert!(
            fv.lane_layer.is_some(),
            "Floating Lane-Layer card must be visible"
        );
        let px = render_and_verify(&mut gpu, "09-jam-perform.png", &fv, &dark, write_pngs);
        rendered.push(("09-jam-perform.png", px));
    }

    // -------------------------------------------------------------------------
    // 10. 10-studio-light.png — Afterglow (Studio Light): Arrange + Mixer + Bank + Output
    // -------------------------------------------------------------------------
    {
        let mut app = load_mit_project_app("examples/demo/project.json");
        app.config.theme = ThemeId::StudioLight;
        app.view_state.bank_mode = BankMode::Parts;
        let step = 404; // Bar 26 Drop 1
        app.arrange_ui.step = step as u32;
        app.press_keys("tf").unwrap(); // Follow on so Drop 1 + playhead are centered
        app.observe_transport(step, step + 1, true, false);
        let snap = snapshot_from_telemetry(
            &afterglow_telemetry,
            app.project(),
            step,
            0.35,
            &afterglow_scopes,
        );
        // Open Mixer unzoomed (`gm`) and focus Drums (Page 1) so all 12 strips render side-by-side
        // beneath Arrange and directly above the 32-bit float Master Output monitor!
        app.press_keys("gm").unwrap();
        let drums_id = app
            .project()
            .mixer
            .channels
            .iter()
            .find(|c| c.name == "Drums")
            .expect("Drums group channel in Afterglow")
            .id;
        app.selected_channel = ChannelRef::Channel(drums_id);

        let fv = app.frame_view(&snap, AUDIO_STATUS);
        assert_eq!(fv.chrome.mode_kind, UiModeKind::Normal);
        assert!(fv
            .decks
            .iter()
            .any(|d| matches!(d.view, DeckView::Arrange(_))));
        assert!(fv
            .decks
            .iter()
            .any(|d| matches!(d.view, DeckView::Mixer(_))));
        assert!(fv.decks.iter().any(|d| matches!(d.view, DeckView::Bank(_))));
        assert!(fv
            .decks
            .iter()
            .any(|d| matches!(d.view, DeckView::Output(_))));
        let px = render_and_verify(&mut gpu, "10-studio-light.png", &fv, &light, write_pngs);
        rendered.push(("10-studio-light.png", px));
    }

    // -------------------------------------------------------------------------
    // 11. 11-rack-patchbay.png — Iron Circuit (Dark): Eurorack Modular Patchbay + z-1 Feedback
    // -------------------------------------------------------------------------
    {
        let iron_path = repo_root().join("examples/iron-circuit/project.json");
        let mut project = cadence_app::load_project(&iron_path).expect("load Iron Circuit");
        // Expand Iron Circuit's Acid 303 in-memory with authentic Eurorack modules & cables
        // across all 4 Patchbay columns (`SOURCES` | `MODULATORS` | `PROCESSORS` | `VCA / OUT`)
        // plus a cross-column `z-1` feedback loop (`Fold Drive -> VCO.fm -28%`).
        let acid = project
            .instruments
            .iter_mut()
            .find(|i| i.name == "Acid 303")
            .expect("Acid 303 in Iron Circuit");
        let zone = &mut acid.rack.zones[0];
        let vco_id = zone.resolve_module_id("VCO").expect("VCO module");
        let filter_id = zone
            .resolve_module_id("Acid Filter")
            .expect("Acid Filter module");
        let vca_id = zone.resolve_module_id("VCA").expect("VCA module");

        let sub_fm_id = zone.next_module_id();
        zone.modules.push(RackModule::new(
            sub_fm_id,
            "Sub FM",
            ModuleKind::Vco(VcoModule {
                waveform: Waveform::Sine,
                tune_semitones: 12.0,
                level: 0.72,
                ..VcoModule::default()
            }),
        ));
        let lfo_id = zone.next_module_id();
        zone.modules.push(RackModule::new(
            lfo_id,
            "Sync LFO",
            ModuleKind::Lfo(LfoModule {
                shape: LfoShape::Triangle,
                rate_hz: 2.2,
                sync: true,
                division_steps: 2.0,
                retrigger: true,
                bipolar: true,
            }),
        ));
        let drive_id = zone.next_module_id();
        zone.modules.push(RackModule::new(
            drive_id,
            "Shock Drive",
            ModuleKind::Drive(DriveModule {
                mode: DriveMode::ShockPort,
                drive: 0.62,
                asymmetry: 0.25,
                tone_hz: 8_800.0,
                output_db: -1.5,
                mix: 0.85,
            }),
        ));
        let lofi_id = zone.next_module_id();
        zone.modules.push(RackModule::new(
            lofi_id,
            "Bit Crush",
            ModuleKind::LoFi(LoFiModule {
                bits: 8.0,
                rate_hz: 10_000.0,
                mix: 0.78,
            }),
        ));

        // Re-route Acid Filter -> Shock Drive -> VCA (with parallel Bit Crush) + FM/LFO + z-1 feedback.
        zone.cables
            .retain(|c| !(c.from_module == filter_id && c.to_module == vca_id));
        for (from, to, port, amt) in [
            (sub_fm_id, vco_id, InPort::FmIn, 0.35_f32),
            (lfo_id, vco_id, InPort::PitchCv, 0.25),
            (lfo_id, filter_id, InPort::Cv, 0.45),
            (filter_id, drive_id, InPort::In, 1.0),
            (drive_id, vca_id, InPort::In, 1.0),
            (drive_id, lofi_id, InPort::In, 0.65),
            (lofi_id, vca_id, InPort::In, 0.45),
        ] {
            let cid = zone.next_cable_id();
            zone.cables.push(PatchCable::new(cid, from, to, port, amt));
        }
        let fb_cable_id = zone.next_cable_id();
        zone.cables.push(PatchCable::new(
            fb_cable_id,
            drive_id,
            vco_id,
            InPort::FmIn,
            -0.28,
        ));
        project
            .validate()
            .expect("expanded Acid 303 Rack validates");

        // Capture real 48 kHz per-module & per-cable Patchbay activity directly from
        // the production ModularVoice engine on this exact expanded Acid 303 patch!
        let acid_id = project
            .instruments
            .iter()
            .find(|i| i.name == "Acid 303")
            .expect("Acid 303 ID")
            .id;
        let acid_zone_id = project
            .instruments
            .iter()
            .find(|i| i.id == acid_id)
            .unwrap()
            .rack
            .zones[0]
            .id;
        let compiled_pb = project.compile().expect("compile expanded Iron Circuit");
        let mut pb_lib = SampleLibrary::from_roots(&[], Some(&iron_path));
        let pb_samples = load_project_samples(&compiled_pb, &mut pb_lib);
        pb_samples
            .ensure_ready()
            .expect("MIT samples for expanded Iron Circuit");
        let step = 404; // Bar 26 Overload
        let rack_activity = render_rack_activity_snapshot(
            &compiled_pb,
            pb_samples.bank,
            TELEMETRY_SR,
            384,
            step - 384 + 1,
            acid_id,
            acid_zone_id,
        )
        .expect("capture live Patchbay telemetry");

        let library = SampleLibrary::from_roots(&[], Some(&iron_path));
        let mut app = App::new_with_library(project, iron_path, library).unwrap();
        app.observe_transport(step, step + 1, true, false);
        app.execute_command("instrument \"Acid 303\"").unwrap();
        app.rack_ui.tab = RackTab::Patchbay;
        app.rack_ui.patchbay_cable_id = Some(fb_cable_id);
        app.press_keys("zz").unwrap();

        let mut snap =
            snapshot_from_telemetry(&iron_telemetry, app.project(), step, 0.35, &iron_scopes);
        snap.set_rack_activity(rack_activity);
        let fv = app.frame_view(&snap, AUDIO_STATUS);
        let iv = fv
            .decks
            .iter()
            .find_map(|d| match &d.view {
                DeckView::Instrument(iv) => Some(iv),
                _ => None,
            })
            .expect("InstrumentDeckView for Acid 303 Patchbay");
        match &iv.visual {
            InstrumentVisualView::Rack(rack) => {
                assert_eq!(rack.active_tab, RackTab::Patchbay);
                assert!(
                    rack.patchbay.has_feedback,
                    "Patchbay showcase must exercise z-1 feedback routing"
                );
                assert_eq!(rack.patchbay.feedback_badge, "z-1");
                assert!(
                    rack.patchbay
                        .cables
                        .iter()
                        .any(|c| c.feedback && c.selected),
                    "Selected z-1 feedback cable must be highlighted"
                );
                assert!(
                    rack.patchbay.columns.iter().all(|c| !c.nodes.is_empty()),
                    "All 4 Patchbay columns must be populated"
                );
                assert!(
                    rack.patchbay.nodes.iter().any(|n| n.activity > 0.05)
                        && rack.patchbay.cables.iter().any(|c| c.activity > 0.05),
                    "Patchbay showcase must render live engine-captured module & cable activity"
                );
            }
            other => panic!("expected Rack Patchbay visual, got {other:?}"),
        }
        let px = render_and_verify(&mut gpu, "11-rack-patchbay.png", &fv, &dark, write_pngs);
        rendered.push(("11-rack-patchbay.png", px));
    }

    // -------------------------------------------------------------------------
    // 12. 12-devices-audio.png — Afterglow (Dark): Devices · Audio Tab + Live Input Meters
    // -------------------------------------------------------------------------
    {
        let mut app = load_mit_project_app("examples/demo/project.json");
        let step = 404;
        app.observe_transport(step, step + 1, true, false);
        app.set_audio_inventory(AudioInventory {
            outputs: vec![
                AudioDeviceInfo {
                    name: "MacBook Pro Speakers".into(),
                    is_default: true,
                    default_sample_rate: 48_000,
                    supported_sample_rates: vec![44_100, 48_000, 96_000],
                    min_buffer_frames: Some(128),
                    max_buffer_frames: Some(1024),
                    channels: 2,
                },
                AudioDeviceInfo {
                    name: "Focusrite Scarlett 4i4".into(),
                    is_default: false,
                    default_sample_rate: 48_000,
                    supported_sample_rates: vec![44_100, 48_000, 88_200, 96_000],
                    min_buffer_frames: Some(64),
                    max_buffer_frames: Some(2048),
                    channels: 4,
                },
            ],
            inputs: vec![AudioDeviceInfo {
                name: "Focusrite Scarlett 4i4 In".into(),
                is_default: true,
                default_sample_rate: 48_000,
                supported_sample_rates: vec![48_000, 96_000],
                min_buffer_frames: Some(64),
                max_buffer_frames: Some(2048),
                channels: 4,
            }],
            active_output: Some("Focusrite Scarlett 4i4".into()),
            active_output_is_default: false,
            active_sample_rate: 48_000,
            active_buffer_frames: 256,
            active_channels: 4,
            active_format_label: "32-bit float".into(),
            active_input: Some("Focusrite Scarlett 4i4 In".into()),
            active_input_sample_rate: 48_000,
            active_input_channels: 4,
            active_input_buffer_frames: 256,
            silent_mode: false,
            last_error: None,
            input_last_error: None,
        });
        app.config.audio_output_device = Some("Focusrite Scarlett 4i4".into());
        app.config.audio_sample_rate = Some(48_000);
        app.config.audio_buffer_size = Some(256);
        app.config.audio_input_device = Some("Focusrite Scarlett 4i4 In".into());
        app.config.audio_input_mode = AudioInputMode::Stereo;
        app.config.audio_input_monitor = InputMonitorMode::Auto;
        app.config.audio_input_latency_ms = 2.5;
        app.execute_command("audio").unwrap();
        app.press_keys("zz").unwrap();
        // Focus Input Monitor row cleanly inside AUDIO INPUT & RECORDING.
        app.devices.cursor = 6;

        let mut snap = snapshot_from_telemetry(
            &afterglow_telemetry,
            app.project(),
            step,
            0.35,
            &afterglow_scopes,
        );
        snap.health = AudioHealth {
            status: AudioStreamStatus::Running,
            cpu_load: 0.19,
            peak_cpu_load: 0.28,
            xrun_count: 0,
            buffer_frames: 256,
            channels: 4,
            bit_depth: 32,
            sample_format_is_float: true,
            overloaded: false,
            failed: false,
            error_code: AudioErrorCode::None,
            ..AudioHealth::default()
        };
        snap.input = AudioInputTelemetry {
            status: AudioInputStatus::Running,
            sample_rate: 48_000,
            channels: 4,
            buffer_frames: 256,
            peak: [0.58, 0.51],
            peak_hold: [0.74, 0.66],
            clipped: false,
            digital_silence: false,
            mode: AudioInputMode::Stereo,
            monitor_mode: InputMonitorMode::Auto,
            monitoring: true,
            capture_phase: CapturePhase::Idle,
            captured_frames: 0,
            dropped_frames: 0,
            capture_start_step: 0,
            capture_end_step: 0,
        };
        app.observe_snapshot(&snap);

        let fv = app.frame_view(&snap, AUDIO_STATUS);
        let dv = fv
            .decks
            .iter()
            .find_map(|d| match &d.view {
                DeckView::Midi(m) => Some(m),
                _ => None,
            })
            .expect("Devices Audio view");
        assert_eq!(dv.active_tab, DevicesTab::Audio);
        let px = render_and_verify(&mut gpu, "12-devices-audio.png", &fv, &dark, write_pngs);
        rendered.push(("12-devices-audio.png", px));
    }

    // -------------------------------------------------------------------------
    // 13. 13-devices-midi.png — Afterglow (Dark): Devices · MIDI Tab (MiniLab 3 DAW + RGB Pads)
    // -------------------------------------------------------------------------
    {
        let mut app = load_mit_project_app("examples/demo/project.json");
        let step = 404;
        app.observe_transport(step, step + 1, true, false);
        app.midi.in_ports = vec!["Arturia MiniLab 3 MIDI".into(), "Launchpad X".into()];
        app.midi.out_ports = vec!["Arturia MiniLab 3 DAW".into()];
        app.midi.port = Some("Arturia MiniLab 3 MIDI".into());
        app.midi.out_port = Some("Arturia MiniLab 3 DAW".into());
        app.config.midi_in_port = Some("Arturia MiniLab 3 MIDI".into());
        app.config.midi_out_port = Some("Arturia MiniLab 3 DAW".into());
        app.config.midi_pad_mode = MidiPadMode::Sections;
        app.config.midi_pickup = true;
        app.execute_command("midi profile minilab3_daw").unwrap();
        app.config.midi_mappings.push(MidiMapping::new(
            701,
            MidiSource::Cc {
                channel: Some(0),
                cc: 74,
            },
            MidiValueMode::Absolute,
            MidiTarget::ProjectMacro { slot: 2 },
        ));
        app.config.midi_mappings.push(MidiMapping::new(
            702,
            MidiSource::Cc {
                channel: Some(0),
                cc: 7,
            },
            MidiValueMode::Absolute,
            MidiTarget::Param {
                address: ParamAddress::Channel {
                    channel: ChannelRef::Master,
                    param: ChannelParam::Gain,
                },
            },
        ));
        app.handle_midi(MidiMessage::Control {
            channel: 0,
            controller: 74,
            value: 92,
        })
        .unwrap();
        app.handle_midi(MidiMessage::NoteOn {
            channel: 0,
            note: 65,
            velocity: 108,
        })
        .unwrap();
        app.execute_command("midi").unwrap();
        app.press_keys("zz").unwrap();

        let snap = snapshot_from_telemetry(
            &afterglow_telemetry,
            app.project(),
            step,
            0.35,
            &afterglow_scopes,
        );
        let fv = app.frame_view(&snap, AUDIO_STATUS);
        let dv = fv
            .decks
            .iter()
            .find_map(|d| match &d.view {
                DeckView::Midi(m) => Some(m),
                _ => None,
            })
            .expect("Devices MIDI view");
        assert_eq!(dv.active_tab, DevicesTab::Midi);
        assert_eq!(dv.device_rows.len(), 5);
        let px = render_and_verify(&mut gpu, "13-devices-midi.png", &fv, &dark, write_pngs);
        rendered.push(("13-devices-midi.png", px));
    }

    // -------------------------------------------------------------------------
    // 14. 14-preset-browser.png — Iron Circuit (Dark): 6-Category Live Preset Browser (`P`)
    // -------------------------------------------------------------------------
    {
        let mut app = load_mit_project_app("examples/iron-circuit/project.json");
        app.set_preset_catalog(PresetCatalog::from_roots(&[repo_root().join("presets")]));
        let step = 404;
        app.observe_transport(step, step + 1, true, false);
        app.execute_command("instrument \"Acid 303\"").unwrap();
        app.rack_ui.tab = RackTab::Panel;
        app.press_keys("zzPj").unwrap();

        let snap =
            snapshot_from_telemetry(&iron_telemetry, app.project(), step, 0.35, &iron_scopes);
        let fv = app.frame_view(&snap, AUDIO_STATUS);
        assert!(matches!(fv.overlay, Some(OverlayView::PresetBrowser(_))));
        let px = render_and_verify(&mut gpu, "14-preset-browser.png", &fv, &dark, write_pngs);
        rendered.push(("14-preset-browser.png", px));
    }

    // -------------------------------------------------------------------------
    // 15. 15-take-deck.png — Fivefold Live (Dark): Arrange + Take Deck (`gt`) + `v` Visual Range
    // -------------------------------------------------------------------------
    {
        let mut app = load_mit_project_app("examples/fivefold-live/project.json");
        let take_path = repo_root().join("examples/fivefold-live/takes/set.take.jsonl");
        app.apply(Action::LoadTake { path: take_path }).unwrap();
        let step = 15 * 16 + 4; // Bar 16.2 — peak of Clap Build + Noise Lift into Prism
        app.arrange_ui.step = step as u32;
        app.press_keys("tf").unwrap();
        app.observe_transport(step, step + 1, false, false);
        app.press_keys("gt").unwrap();
        // Scrub to Bar 14 (Hands Build Eighths), start Visual mark (`v`), extend through Bar 17
        // (capturing Hands Build Eighths -> Build Sixteenths + Lift/Noise -> Prism drop!).
        app.press_keys("0").unwrap();
        for _ in 0..13 {
            app.press_keys("l").unwrap();
        }
        app.press_keys("vlll").unwrap();

        let snap = AudioSnapshot {
            playing: false,
            absolute_step: step,
            sample_rate: TELEMETRY_SR,
            ..AudioSnapshot::default()
        };
        let fv = app.frame_view(&snap, AUDIO_STATUS);
        assert!(
            fv.decks.iter().any(|d| matches!(d.view, DeckView::Take(_))),
            "Take Deck must be visible"
        );
        let px = render_and_verify(&mut gpu, "15-take-deck.png", &fv, &dark, write_pngs);
        rendered.push(("15-take-deck.png", px));
    }

    // -------------------------------------------------------------------------
    // 16. 16-action-palette.png — Afterglow (Dark): Fuzzy Action Palette (`:` / `Ctrl+Shift+p`)
    // -------------------------------------------------------------------------
    {
        let mut app = load_mit_project_app("examples/demo/project.json");
        app.view_state.bank_mode = BankMode::Parts;
        let step = 404;
        app.arrange_ui.step = step as u32;
        app.press_keys("tf").unwrap();
        app.observe_transport(step, step + 1, true, false);
        app.press_keys(":filter").unwrap();

        let snap = snapshot_from_telemetry(
            &afterglow_telemetry,
            app.project(),
            step,
            0.35,
            &afterglow_scopes,
        );
        let fv = app.frame_view(&snap, AUDIO_STATUS);
        assert!(matches!(fv.overlay, Some(OverlayView::Palette(_))));
        let px = render_and_verify(&mut gpu, "16-action-palette.png", &fv, &dark, write_pngs);
        rendered.push(("16-action-palette.png", px));
    }

    // -------------------------------------------------------------------------
    // 17. 17-settings-neon.png — Afterglow (Midnight Neon): Global Settings & Themes (`g,`)
    // -------------------------------------------------------------------------
    {
        let mut app = load_mit_project_app("examples/demo/project.json");
        app.config.theme = ThemeId::MidnightNeon;
        app.config.sample_dirs = vec!["~/Music/Studio Samples".into()];
        app.config.preset_dirs = vec!["~/.config/cadence/presets".into()];
        app.view_state.bank_mode = BankMode::Parts;
        let step = 404;
        app.arrange_ui.step = step as u32;
        app.press_keys("tf").unwrap();
        app.observe_transport(step, step + 1, true, false);
        app.press_keys("g,").unwrap();
        app.settings_ui.cursor = 0; // Highlight GPU Theme: Midnight Neon

        let snap = snapshot_from_telemetry(
            &afterglow_telemetry,
            app.project(),
            step,
            0.35,
            &afterglow_scopes,
        );
        let mut fv = app.frame_view(&snap, AUDIO_STATUS);
        match fv.overlay.as_mut() {
            Some(OverlayView::Settings(s)) => {
                // Keep public screenshots portable and free of host username paths.
                s.config_path = "~/.config/cadence/config.json".into();
            }
            other => panic!("expected Settings overlay, got {other:?}"),
        }
        let px = render_and_verify(&mut gpu, "17-settings-neon.png", &fv, &neon, write_pngs);
        rendered.push(("17-settings-neon.png", px));
    }

    // -------------------------------------------------------------------------
    // 18. 18-sample-deck.png — Afterglow (Dark): Sample / Waveform & Slicer Deck (`gw`)
    // -------------------------------------------------------------------------
    {
        let demo_path = repo_root().join("examples/demo/project.json");
        let mut project = cadence_app::load_project(&demo_path).expect("load Afterglow");
        let master_or_lead_ch = project
            .mixer
            .channels
            .iter()
            .find(|c| c.name == "Chords" || c.name == "Lead")
            .map_or(0, |c| c.id);
        let inst_id = 999_001;
        let mut rack = cadence_core::rack::RackPatch::default_sampler();
        rack.kit_name = "Brass Pad".into();
        if let Some(cadence_core::rack::ModuleKind::Sampler(smp)) = rack
            .zones
            .first_mut()
            .and_then(|z| z.modules.first_mut())
            .map(|m| &mut m.kind)
        {
            smp.path = Some("samples/brass-pad.wav".into());
            smp.sample_start = 2_400;
            smp.sample_end = 71_500;
            smp.loop_mode = cadence_core::sampler::SampleLoopMode::Forward;
            smp.loop_start = 16_800;
            smp.loop_end = 56_400;
            smp.loop_xfade_ms = 18.0;
            smp.one_shot = false;
            smp.slices = vec![
                cadence_core::sampler::SampleSlice::new(2_400, Some("Attack".into())),
                cadence_core::sampler::SampleSlice::new(16_800, Some("Swell".into())),
                cadence_core::sampler::SampleSlice::new(38_400, Some("Sustain".into())),
                cadence_core::sampler::SampleSlice::new(56_400, Some("Tail".into())),
            ];
        }
        project
            .instruments
            .push(cadence_core::material::InstrumentInstance {
                id: inst_id,
                name: "Brass Pad".into(),
                channel: master_or_lead_ch,
                rack,
            });
        project.validate().expect("Afterglow + Brass Pad validates");

        let library = SampleLibrary::from_roots(&[], Some(&demo_path));
        let mut app = App::new_with_library(project, demo_path, library).unwrap();
        assert!(app.ready(), "Brass Pad MIT sample must load cleanly");

        let step = 404;
        app.observe_transport(step, step + 1, true, false);
        app.execute_command("instrument \"Brass Pad\"").unwrap();
        // Open Sample / Waveform & Slicer Deck full-screen and focus slice 2 ('Swell').
        app.press_keys("gwzz0ww").unwrap();

        let snap = snapshot_from_telemetry(
            &afterglow_telemetry,
            app.project(),
            step,
            0.35,
            &afterglow_scopes,
        );
        let fv = app.frame_view(&snap, AUDIO_STATUS);
        let sv = fv
            .decks
            .iter()
            .find_map(|d| match &d.view {
                DeckView::Sample(s) => Some(s),
                _ => None,
            })
            .expect("SampleDeckView for Brass Pad");
        assert!(sv.loaded, "Sample deck must load MIT brass-pad.wav");
        assert_eq!(
            sv.slices.len(),
            4,
            "Sample deck showcase must show 4 slices"
        );
        assert_eq!(
            sv.active_slice_idx,
            Some(1),
            "Slice 2 ('Swell') must be active"
        );
        let px = render_and_verify(&mut gpu, "18-sample-deck.png", &fv, &dark, write_pngs);
        rendered.push(("18-sample-deck.png", px));
    }

    // Confirm all 18 rendered screenshots are unique frames.
    assert_eq!(rendered.len(), 18);
    for (i, (name_a, px_a)) in rendered.iter().enumerate() {
        for (name_b, px_b) in &rendered[i + 1..] {
            assert_ne!(
                px_a, px_b,
                "{name_a} and {name_b} must produce distinct rendered frames"
            );
        }
    }
}
