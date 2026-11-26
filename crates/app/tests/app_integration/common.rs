#![allow(dead_code)]

use cadence_core::sampler::{write_wav_with_metadata, SampleLibrary, SampleMetadata};
use std::path::{Path, PathBuf};

/// Workspace root directory resolved from `crates/app`.
pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

/// Hermetic [`SampleLibrary`] pointing strictly at `factory-samples/` (plus optional project assets),
/// isolated from host `./samples/`, `~/Samples`, and `~/.config/cadence/config.json`.
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
    let scale = amp.clamp(0.0, 1.0);
    let samples: Vec<f32> = (0..frames)
        .map(|i| (std::f32::consts::TAU * freq_hz * (i as f32 / sample_rate as f32)).sin() * scale)
        .collect();
    let right = (channels >= 2).then_some(samples.as_slice());
    write_wav_with_metadata(
        path,
        &samples,
        right,
        sample_rate,
        &SampleMetadata::default(),
    )
    .expect("write synthetic sine WAV");
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
    let scale = amp.clamp(0.0, 1.0);
    let denom = frames.max(1) as f32;
    let samples: Vec<f32> = (0..frames)
        .map(|i| {
            let t = i as f32 / sample_rate as f32;
            let env = (1.0 - i as f32 / denom).max(0.0);
            (t * freq_hz * std::f32::consts::TAU).sin() * env * scale
        })
        .collect();
    let right = (channels >= 2).then_some(samples.as_slice());
    write_wav_with_metadata(
        path,
        &samples,
        right,
        sample_rate,
        &SampleMetadata::default(),
    )
    .expect("write synthetic decay WAV");
}
