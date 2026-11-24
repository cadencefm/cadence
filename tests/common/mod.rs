#![allow(dead_code)]

use cadence_core::sampler::SampleLibrary;
use std::path::{Path, PathBuf};

/// Workspace root directory (`CARGO_MANIFEST_DIR`).
pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Hermetic [`SampleLibrary`] pointing strictly at `factory-samples/` (plus optional project assets),
/// completely isolated from host `./samples/`, `~/Samples`, and `~/.config/cadence/config.json`.
pub fn factory_sample_library(
    project_path: Option<&Path>,
    include_project_assets: bool,
) -> SampleLibrary {
    SampleLibrary::from_prefixed_roots(
        &[(Some("factory-samples"), repo_root().join("factory-samples"))],
        project_path,
        include_project_assets,
    )
}

/// Write a synthetic mono or stereo 16-bit PCM sine WAV fixture (`amp` in `0.0..=1.0`).
pub fn write_sine_wav(
    path: &Path,
    channels: u16,
    sample_rate: u32,
    frames: usize,
    freq_hz: f32,
    amp: f32,
) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).expect("create synthetic sine WAV");
    let scale = amp.clamp(0.0, 1.0) * f32::from(i16::MAX);
    for i in 0..frames {
        let phase = std::f32::consts::TAU * freq_hz * (i as f32 / sample_rate as f32);
        let sample = (phase.sin() * scale).round() as i16;
        for _ in 0..channels {
            writer.write_sample(sample).expect("write WAV sample");
        }
    }
    writer.finalize().expect("finalize synthetic WAV");
}

/// Write a synthetic linearly decaying percussive 16-bit PCM WAV fixture (`amp` in `0.0..=1.0`).
pub fn write_decay_wav(
    path: &Path,
    channels: u16,
    sample_rate: u32,
    frames: usize,
    freq_hz: f32,
    amp: f32,
) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).expect("create synthetic decay WAV");
    let scale = amp.clamp(0.0, 1.0) * f32::from(i16::MAX);
    let denom = frames.max(1) as f32;
    for i in 0..frames {
        let t = i as f32 / sample_rate as f32;
        let env = (1.0 - i as f32 / denom).max(0.0);
        let sample = ((t * freq_hz * std::f32::consts::TAU).sin() * env * scale).round() as i16;
        for _ in 0..channels {
            writer.write_sample(sample).expect("write WAV sample");
        }
    }
    writer.finalize().expect("finalize synthetic decay WAV");
}
