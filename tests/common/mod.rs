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

/// Build a minimal self-contained synthetic ReBirth 2.0 (4.2) `.rbs` binary fixture
/// (2 bars, 303#1 + 808 + Delay + Master Comp) for RBS import and CLI/bridge tests.
pub fn minimal_rbs_bytes(title: &str, bpm_milli: u32) -> Vec<u8> {
    let chunk = |tag: &[u8; 4], payload: &[u8]| -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + payload.len() + 1);
        out.extend_from_slice(tag);
        out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        out.extend_from_slice(payload);
        if payload.len() % 2 == 1 {
            out.push(0);
        }
        out
    };
    let cat = |kind: &[u8; 4], items: &[Vec<u8>]| -> Vec<u8> {
        let mut body = kind.to_vec();
        for it in items {
            body.extend_from_slice(it);
        }
        chunk(b"CAT ", &body)
    };
    let mut head = vec![0u8; 256];
    head[0..9].copy_from_slice(&[0x5B, 0x54, 0x5B, 0x54, 0xBC, 0x04, 0x02, 0x00, 0x00]);
    let mut glob = vec![0u8; 512];
    glob[0] = 1; // song mode
    glob[1] = 1; // loop on
    glob[2..6].copy_from_slice(&bpm_milli.to_be_bytes());
    glob[10..14].copy_from_slice(&(2 * 768u32).to_be_bytes());
    let mut usri = vec![0u8; 712];
    let title_bytes = title.as_bytes();
    let n = title_bytes.len().min(64);
    usri[..n].copy_from_slice(&title_bytes[..n]);
    let mut mixr = vec![0u8; 64];
    mixr[0] = 110;
    mixr[1] = 1;
    for c in 0..4 {
        let b = 16 + 12 * c;
        mixr[b] = 1;
        mixr[b + 1] = 105;
        mixr[b + 2] = 64;
    }
    let mut tb1 = vec![0u8; 1097];
    tb1[0..9].copy_from_slice(&[1, 0, 64, 52, 95, 90, 70, 95, 0]);
    tb1[10] = 16;
    for s in 0..16 {
        tb1[11 + s * 2] = [0, 0, 12, 3][s % 4];
        tb1[11 + s * 2 + 1] = if s % 4 == 0 { 0x02 } else { 0x00 };
    }
    let mut tb2 = vec![0u8; 1097];
    tb2[10] = 16;
    for s in 0..16 {
        tb2[11 + s * 2 + 1] = 0x10;
    }
    let mut t808 = vec![0u8; 6238];
    t808[0] = 1;
    t808[3] = 115;
    t808[31] = 16;
    for s in (0..16).step_by(4) {
        t808[32 + s * 12 + 1] = 1; // BD
    }
    let mut t909 = vec![0u8; 6239];
    t909[32] = 16;
    let empty_trak = chunk(b"TRAK", &[0, 0, 0, 1, 0, 0, 0]);
    let on_trak = chunk(b"TRAK", &[0, 0, 0, 2, 0, 0, 1, 0, 1, 0]);
    let devl = cat(
        b"DEVL",
        &[
            chunk(b"MIXR", &mixr),
            chunk(b"DELY", &[1, 3, 1, 64, 64, 0, 0, 0]),
            chunk(b"PCF ", &[0, 60, 64, 40, 0, 64, 1, 0, 0, 0, 0, 0]),
            chunk(b"DIST", &[0, 60, 0, 0, 0, 0, 0, 0]),
            chunk(b"COMP", &[1, 70, 50, 0, 0, 0, 0, 0]),
            chunk(b"303 ", &tb1),
            chunk(b"303 ", &tb2),
            chunk(b"808 ", &t808),
            chunk(b"909 ", &t909),
        ],
    );
    let trkl = cat(
        b"TRKL",
        &[
            chunk(b"TRAK", &[0, 0, 0, 1, 0, 1, 1]),
            on_trak.clone(),
            empty_trak.clone(),
            on_trak,
            empty_trak.clone(),
            empty_trak.clone(),
            empty_trak.clone(),
            empty_trak.clone(),
            empty_trak,
        ],
    );
    cat(
        b"RB40",
        &[
            chunk(b"HEAD", &head),
            chunk(b"GLOB", &glob),
            chunk(b"USRI", &usri),
            devl,
            trkl,
        ],
    )
}
