#![allow(dead_code)]

use cadence_core::material::{ModulationTarget, Project};
use cadence_core::mixer::{ChannelParam, ChannelRef};
use cadence_core::params::ParamAddress;
use cadence_core::runtime::{AudioSnapshot, ChannelMeter};
use cadence_core::sampler::{write_wav_with_metadata, SampleMetadata};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Build a deterministic live `AudioSnapshot` populated with master/track waveforms and channel
/// meters for headless GPU visual tests.
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

/// Save an RGBA8 framebuffer as a PNG when `env_var` is set to an output directory.
pub fn maybe_save_png(env_var: &str, name: &str, width: u32, height: u32, rgba: &[u8]) {
    if let Ok(dir) = std::env::var(env_var) {
        let dir = PathBuf::from(dir);
        let _ = std::fs::create_dir_all(&dir);
        if let Some(img) = image::RgbaImage::from_raw(width, height, rgba.to_vec()) {
            img.save(dir.join(name))
                .unwrap_or_else(|e| panic!("saving {name}: {e}"));
        }
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
/// deterministic FX and metering tests never exceed `MAX_INSERTS`/`MAX_EFFECT_DEVICES` or have
/// showcase fader positions overridden as `examples/demo/project.json` evolves.
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
