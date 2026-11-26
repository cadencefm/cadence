use cadence_app::view::FrameView;
use cadence_core::material::{ModulationTarget, Project};
use cadence_core::mixer::{ChannelParam, ChannelRef};
use cadence_core::params::ParamAddress;
use cadence_core::runtime::{AudioSnapshot, ChannelMeter};
use cadence_core::sampler::{write_wav_with_metadata, SampleMetadata};
use cadence_gui::headless::{distinct_pixel_ratio, save_rgba8_png, HeadlessGpu};
use cadence_gui::theme::Theme;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

pub const DEFAULT_W: u32 = 1440;
pub const DEFAULT_H: u32 = 860;
pub const STATUS: &str = "Metal · 48 kHz";

const DUMP_ENV: &str = "CADENCE_DUMP_GPU_DIR";

static GPU_POOL: OnceLock<Mutex<Vec<HeadlessGpu>>> = OnceLock::new();
static NO_GPU_ADAPTER: AtomicBool = AtomicBool::new(false);

struct PooledGpu(Option<HeadlessGpu>);

impl Drop for PooledGpu {
    fn drop(&mut self) {
        if let Some(gpu) = self.0.take() {
            if let Ok(mut pool) = GPU_POOL.get_or_init(|| Mutex::new(Vec::new())).lock() {
                pool.push(gpu);
            }
        }
    }
}

/// Check out a pooled `HeadlessGpu` resized to `(width, height)` for parallel test execution.
/// Returns cleanly without failing on GPU-less headless CI containers.
pub fn with_gpu(width: u32, height: u32, f: impl FnOnce(&mut HeadlessGpu)) {
    let pool = GPU_POOL.get_or_init(|| Mutex::new(Vec::new()));
    let existing = pool.lock().ok().and_then(|mut p| p.pop());
    let mut gpu = match existing {
        Some(gpu) => gpu,
        None => {
            if NO_GPU_ADAPTER.load(Ordering::Relaxed) {
                return;
            }
            match HeadlessGpu::new(width, height) {
                Ok(gpu) => gpu,
                Err(_) => {
                    NO_GPU_ADAPTER.store(true, Ordering::Relaxed);
                    return;
                }
            }
        }
    };
    gpu.resize(width, height);
    let mut guard = PooledGpu(Some(gpu));
    f(guard.0.as_mut().expect("pooled gpu present"));
}

/// Render `frame` through `HeadlessGpu::render_rgba8` (single reusable `DisplayList` pass with
/// balanced clip stack and zero missing-glyph checks), assert `distinct_pixel_ratio > min_ratio`,
/// and optionally save PNG under `CADENCE_DUMP_GPU_DIR/<name>`.
pub fn render_checked(
    gpu: &mut HeadlessGpu,
    name: &str,
    frame: &FrameView,
    theme: &Theme,
    min_ratio: f32,
) -> Vec<u8> {
    let w = gpu.width();
    let h = gpu.height();
    let rgba = gpu
        .render_rgba8(frame, theme)
        .unwrap_or_else(|e| panic!("GPU render failed for {name}: {e:#}"));
    let ratio = distinct_pixel_ratio(&rgba, theme.surface.canvas);
    assert!(
        ratio > min_ratio,
        "{name}: rendered too few non-canvas pixels ({ratio:.3} <= {min_ratio:.3})"
    );
    maybe_save_png(name, w, h, &rgba);
    rgba
}

/// Save an RGBA8 framebuffer as a PNG when `CADENCE_DUMP_GPU_DIR` is set.
pub fn maybe_save_png(name: &str, width: u32, height: u32, rgba: &[u8]) {
    if let Ok(dir) = std::env::var(DUMP_ENV) {
        let path = PathBuf::from(dir).join(name);
        save_rgba8_png(&path, width, height, rgba)
            .unwrap_or_else(|e| panic!("saving {name}: {e:#}"));
    }
}

/// Resolve the repository root from `crates/gui`.
pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

/// Load `examples/demo/project.json` (Afterglow) and return `(project, path)`.
pub fn load_demo_project() -> (Project, PathBuf) {
    let path = repo_root().join("examples/demo/project.json");
    let project = cadence_app::load_project(&path).expect("load Afterglow project");
    (project, path)
}

/// Build a deterministic live `AudioSnapshot` populated with master/track waveforms and meters.
pub fn build_snapshot(project: &Project, step: u64) -> AudioSnapshot {
    let wave: Vec<f32> = (0..256)
        .map(|i| {
            let t = i as f32 / 256.0 * std::f32::consts::TAU;
            (t * 2.0).sin() * 0.48 + (t * 5.0).sin() * 0.22 + (t * 9.0).sin() * 0.10
        })
        .collect();
    let mut meters = vec![ChannelMeter {
        channel: ChannelRef::Master,
        input_peak: [0.76, 0.72],
        input_peak_hold: [0.84, 0.80],
        output_peak: [0.76, 0.72],
        output_peak_hold: [0.84, 0.80],
        hot: false,
        clipped: false,
    }];
    for (idx, ch) in project.mixer.channels.iter().enumerate() {
        let lvl = (0.36 + ((idx * 13) % 7) as f32 * 0.06).min(0.84);
        meters.push(ChannelMeter {
            channel: ChannelRef::Channel(ch.id),
            input_peak: [lvl, (lvl * 0.93).min(0.82)],
            input_peak_hold: [(lvl + 0.05).min(0.88), (lvl + 0.04).min(0.86)],
            output_peak: [lvl, (lvl * 0.93).min(0.82)],
            output_peak_hold: [(lvl + 0.05).min(0.88), (lvl + 0.04).min(0.86)],
            hot: false,
            clipped: false,
        });
    }
    let mut track_ids = [0u64; 16];
    for (i, inst) in project.instruments.iter().take(16).enumerate() {
        track_ids[i] = inst.id;
    }
    AudioSnapshot {
        playing: true,
        absolute_step: step,
        step_fraction: 0.35,
        sample_rate: 48_000,
        peak: 0.76,
        peak_left: 0.76,
        peak_right: 0.72,
        waveform: wave.clone(),
        track_count: project.instruments.len(),
        track_ids,
        track_waveforms: vec![wave; project.instruments.len()],
        channel_meters: meters,
        ..AudioSnapshot::default()
    }
}

/// Fraction of pixels whose RGB channel sum differs by more than 18 between two equal-size RGBA8 buffers.
pub fn frame_diff_ratio(a: &[u8], b: &[u8]) -> f32 {
    assert_eq!(a.len(), b.len());
    let total = (a.len() / 4).max(1);
    let mut changed = 0usize;
    for (pa, pb) in a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0.iter()) {
        let diff = (pa[0] as i32 - pb[0] as i32).abs()
            + (pa[1] as i32 - pb[1] as i32).abs()
            + (pa[2] as i32 - pb[2] as i32).abs();
        if diff > 18 {
            changed += 1;
        }
    }
    changed as f32 / total as f32
}

/// Clear pre-existing channel/master inserts and conflicting FX/ChannelGain automation so
/// deterministic FX and metering fixtures stay within `MAX_INSERTS`/`MAX_EFFECT_DEVICES`.
pub fn reset_inserts_and_channel_gain_automation(project: &mut Project) {
    project.mixer.master.inserts.clear();
    for ch in &mut project.mixer.channels {
        ch.inserts.clear();
    }
    project.modulations.retain(|m| {
        !matches!(
            m.target,
            ModulationTarget::Effect { .. } | ModulationTarget::ChannelGain { .. }
        )
    });
    for mac in &mut project.macros {
        mac.bindings.retain(|b| {
            !matches!(
                b.target,
                ParamAddress::Effect { .. }
                    | ParamAddress::Channel {
                        param: ChannelParam::Gain,
                        ..
                    }
            )
        });
    }
}

/// Write a synthetic mono 16-bit PCM WAV fixture via `cadence_core::sampler::write_wav_with_metadata`.
pub fn write_test_wav(path: &Path, frames: usize, sample_rate: u32) {
    let samples: Vec<f32> = (0..frames)
        .map(|i| {
            let t = i as f32 / sample_rate as f32;
            let env = (1.0 - i as f32 / frames.max(1) as f32).clamp(0.0, 1.0);
            (2.0 * std::f32::consts::PI * 220.0 * t).sin() * env * 0.6
        })
        .collect();
    write_wav_with_metadata(
        path,
        &samples,
        None,
        sample_rate,
        &SampleMetadata::default(),
    )
    .unwrap_or_else(|e| panic!("writing test WAV {}: {e:#}", path.display()));
}

/// RAII temporary directory under `std::env::temp_dir()` for self-contained GPU test fixtures.
pub struct TempTestDir(PathBuf);

impl TempTestDir {
    pub fn new(prefix: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("cadence-gui-{prefix}-{}-{id}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create test temp dir");
        Self(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempTestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn fixture_normalization_survives_max_fx_budget_and_conflicting_gain_automation() {
    use cadence_core::macros::{MacroBinding, MacroControl};
    use cadence_core::material::{Modulation, Pattern, PatternKind, ValuePoint, ValueUnit};
    use cadence_core::mixer::{
        ChannelKind, CompressorPatch, EffectDevice, EffectKind, EffectParam, EqPatch,
        MAX_EFFECT_DEVICES, MAX_INSERTS,
    };
    use cadence_core::params::ValueMode;

    let mut project = Project::demo();
    let mut next_id = project.max_id() + 1;

    let mut total_fx = project.mixer.master.inserts.len()
        + project
            .mixer
            .channels
            .iter()
            .map(|c| c.inserts.len())
            .sum::<usize>();
    while project.mixer.master.inserts.len() < MAX_INSERTS && total_fx < MAX_EFFECT_DEVICES {
        project.mixer.master.inserts.push(EffectDevice {
            id: next_id,
            name: format!("Packed Master EQ {next_id}"),
            bypassed: false,
            kind: EffectKind::Eq(EqPatch::default()),
        });
        next_id += 1;
        total_fx += 1;
    }
    for ch in &mut project.mixer.channels {
        while ch.inserts.len() < MAX_INSERTS && total_fx < MAX_EFFECT_DEVICES {
            ch.inserts.push(EffectDevice {
                id: next_id,
                name: format!("Packed Ch EQ {next_id}"),
                bypassed: false,
                kind: EffectKind::Eq(EqPatch::default()),
            });
            next_id += 1;
            total_fx += 1;
        }
    }
    assert_eq!(total_fx, MAX_EFFECT_DEVICES);

    let packed_dev_id = project.mixer.master.inserts[0].id;
    let ch0_id = project.mixer.channels[0].id;
    let pat_id = next_id;
    next_id += 1;
    project.patterns.push(Pattern {
        id: pat_id,
        name: "Stress Auto".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 16,
            mode: ValueMode::Ramp,
            points: vec![ValuePoint {
                step: 0,
                value: 0.5,
            }],
        },
    });
    project.modulations.push(Modulation {
        id: next_id,
        name: "Packed Dev Mod".into(),
        pattern: pat_id,
        target: ModulationTarget::Effect {
            device: packed_dev_id,
            parameter: EffectParam::Low,
        },
    });
    next_id += 1;
    project.modulations.push(Modulation {
        id: next_id,
        name: "Conflicting Ch0 Gain".into(),
        pattern: pat_id,
        target: ModulationTarget::ChannelGain {
            channel: ChannelRef::Channel(ch0_id),
        },
    });
    next_id += 1;
    project.macros.push(MacroControl {
        index: 0,
        name: "Stress Macro".into(),
        value: 0.5,
        bindings: vec![
            MacroBinding {
                target: ParamAddress::Effect {
                    device: packed_dev_id,
                    param: EffectParam::Low,
                },
                min: 0.2,
                max: 0.8,
            },
            MacroBinding {
                target: ParamAddress::Channel {
                    channel: ChannelRef::Channel(ch0_id),
                    param: ChannelParam::Gain,
                },
                min: 0.1,
                max: 0.9,
            },
        ],
    });
    project.validate().expect("stress fixture validates");

    reset_inserts_and_channel_gain_automation(&mut project);

    let mut alloc_id = next_id;
    for i in 0..4 {
        project.mixer.master.inserts.push(EffectDevice {
            id: alloc_id,
            name: format!("Test Master {i}"),
            bypassed: false,
            kind: EffectKind::Eq(EqPatch::default()),
        });
        alloc_id += 1;
        project.mixer.channels[0].inserts.push(EffectDevice {
            id: alloc_id,
            name: format!("Test Ch0 {i}"),
            bypassed: false,
            kind: EffectKind::Eq(EqPatch::default()),
        });
        alloc_id += 1;
    }
    project.mixer.channels[1].inserts.push(EffectDevice {
        id: alloc_id,
        name: "Test Ch1 Comp".into(),
        bypassed: false,
        kind: EffectKind::Compressor(CompressorPatch::default()),
    });
    alloc_id += 1;
    if let Some(grp) = project
        .mixer
        .channels
        .iter_mut()
        .find(|c| c.kind == ChannelKind::Group)
    {
        grp.inserts.push(EffectDevice {
            id: alloc_id,
            name: "Test Group Comp".into(),
            bypassed: false,
            kind: EffectKind::Compressor(CompressorPatch::default()),
        });
    }
    project
        .validate()
        .expect("normalized fixture must validate after pushing dedicated test FX");
    assert!(project
        .modulations
        .iter()
        .all(|m| !matches!(m.target, ModulationTarget::ChannelGain { .. })));
    assert!(project.macros[0].bindings.is_empty());
}
