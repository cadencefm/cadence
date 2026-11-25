//! Universal sample loader (PCM/Float WAV, FL Ogg-in-RIFF WAV, FLAC, Ogg, MP3,
//! AIFF, and pure-Rust WavPack `.wv`), RIFF `smpl`/`inst`/`cue `/`LIST(adtl)`
//! loop & slice metadata codec, shared `ParamDescriptor`, and fast fuzzy `SampleLibrary` index.

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use symphonia::core::{
    codecs::{audio::AudioDecoderOptions, CodecParameters},
    errors::Error as SymphoniaError,
    formats::{probe::Hint, FormatOptions, TrackType},
    io::{MediaSourceStream, MediaSourceStreamOptions},
    meta::MetadataOptions,
};

pub const PEAK_BINS: usize = 512;
pub const MAX_SAMPLE_SLICES: usize = 64;
pub const MAX_SAMPLE_FILE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_DECODED_SAMPLES: usize = 128 * 1024 * 1024 / std::mem::size_of::<f32>();
const MAX_LIBRARY_ENTRIES: usize = 25_000;
const MAX_LIBRARY_SCAN_ITEMS: usize = 100_000;
const MAX_CACHE_BYTES: usize = 256 * 1024 * 1024;
const MAX_RIFF_CUE_POINTS: usize = 4_096;

/// One slice / cue marker inside an audio sample (frame offset + optional RIFF `labl` name).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SampleSlice {
    pub frame: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl SampleSlice {
    pub fn new(frame: usize, label: Option<String>) -> Self {
        let cleaned = label
            .map(|s| s.trim().chars().take(48).collect::<String>())
            .filter(|s| !s.is_empty());
        Self {
            frame,
            label: cleaned,
        }
    }
}

/// Sort, deduplicate, optionally clamp against `frames`, and cap `slices` to `MAX_SAMPLE_SLICES`.
pub fn normalize_slices(slices: &mut Vec<SampleSlice>, frames: Option<usize>) {
    if let Some(total) = frames {
        if total == 0 {
            slices.clear();
            return;
        }
        slices.retain(|s| s.frame < total);
    }
    for s in slices.iter_mut() {
        if let Some(lbl) = s.label.take() {
            let trimmed: String = lbl.trim().chars().take(48).collect();
            if !trimmed.is_empty() {
                s.label = Some(trimmed);
            }
        }
    }
    slices.sort_by_key(|s| s.frame);
    let mut deduped: Vec<SampleSlice> = Vec::with_capacity(slices.len().min(MAX_SAMPLE_SLICES));
    for s in slices.drain(..) {
        if let Some(last) = deduped.last_mut() {
            if last.frame == s.frame {
                if last.label.is_none() && s.label.is_some() {
                    last.label = s.label;
                }
                continue;
            }
        }
        if deduped.len() >= MAX_SAMPLE_SLICES {
            break;
        }
        deduped.push(s);
    }
    *slices = deduped;
}

crate::define_choice_enum! {
    #[derive(Default)]
    pub enum SampleLoopMode {
        #[default]
        Off => ("off", "One-Shot / Off", ["0", "none", "one_shot", "oneshot"]),
        Forward => ("forward", "Forward Loop", ["1", "fwd", "loop"]),
        PingPong => ("ping_pong", "Ping-Pong", ["2", "pingpong", "pp"]),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SampleMetadata {
    pub root_midi: u8,
    /// True when `root_midi` came from embedded RIFF `smpl` or `inst` metadata rather than the default `60`.
    pub has_root: bool,
    pub fine_cents: f32,
    pub low_midi: Option<u8>,
    pub high_midi: Option<u8>,
    pub low_vel: Option<u8>,
    pub high_vel: Option<u8>,
    pub loop_mode: SampleLoopMode,
    pub loop_start: usize,
    pub loop_end: usize,
    /// Embedded RIFF `cue ` + `LIST/adtl/labl` slice markers (sorted ascending by frame).
    pub slices: Vec<SampleSlice>,
}

impl Default for SampleMetadata {
    fn default() -> Self {
        Self {
            root_midi: 60,
            has_root: false,
            fine_cents: 0.0,
            low_midi: None,
            high_midi: None,
            low_vel: None,
            high_vel: None,
            loop_mode: SampleLoopMode::Off,
            loop_start: 0,
            loop_end: 0,
            slices: Vec::new(),
        }
    }
}

/// Immutable decoded audio buffer shared with the real-time audio engine via `Arc`.
#[derive(Debug)]
pub struct SampleData {
    pub path_label: String,
    pub resolved_path: PathBuf,
    pub name: String,
    pub sample_rate: u32,
    pub channels: u8,
    pub frames: usize,
    pub left: Box<[f32]>,
    pub right: Option<Box<[f32]>>,
    /// Precomputed `(min, max)` envelope for zero-cost 60 FPS UI waveform rendering.
    pub peaks: Box<[(f32, f32)]>,
    pub meta: SampleMetadata,
}

impl SampleData {
    #[inline]
    pub fn frame_stereo(&self, index: usize) -> [f32; 2] {
        let idx = index.min(self.frames.saturating_sub(1));
        let l = self.left.get(idx).copied().unwrap_or(0.0);
        let r = self
            .right
            .as_ref()
            .and_then(|ch| ch.get(idx).copied())
            .unwrap_or(l);
        [l, r]
    }

    pub fn duration_seconds(&self) -> f64 {
        if self.sample_rate == 0 {
            0.0
        } else {
            self.frames as f64 / f64::from(self.sample_rate)
        }
    }
}

/// Off-timeline preview playback spec for a full sample, cropped region, or slice.
#[derive(Clone, Debug)]
pub struct SamplePreview {
    pub sample: Arc<SampleData>,
    pub sample_start: usize,
    pub sample_end: usize,
    pub root_midi: u8,
    pub coarse_semitones: i16,
    pub fine_cents: f32,
    pub loop_mode: SampleLoopMode,
    pub loop_start: usize,
    pub loop_end: usize,
    pub loop_xfade_ms: f32,
    pub level: f32,
}

impl SamplePreview {
    /// Preview the entire sample as a clean one-shot at its embedded root pitch.
    pub fn full(sample: Arc<SampleData>) -> Self {
        let frames = sample.frames;
        let root_midi = sample.meta.root_midi.min(127);
        let fine_cents = sample.meta.fine_cents.clamp(-100.0, 100.0);
        Self {
            sample,
            sample_start: 0,
            sample_end: frames,
            root_midi,
            coarse_semitones: 0,
            fine_cents,
            loop_mode: SampleLoopMode::Off,
            loop_start: 0,
            loop_end: frames,
            loop_xfade_ms: 0.0,
            level: 1.0,
        }
    }

    /// Preview a specific frame region `[start, end)` (e.g. a single slice) as a clean one-shot.
    pub fn region(sample: Arc<SampleData>, start: usize, end: usize) -> Self {
        let frames = sample.frames;
        let mut spec = Self::full(sample);
        if frames > 0 {
            let s = start.min(frames.saturating_sub(1));
            let e = end.clamp(s + 1, frames);
            spec.sample_start = s;
            spec.sample_end = e;
            spec.loop_start = s;
            spec.loop_end = e;
        }
        spec
    }
}

/// Unified parameter metadata shared by Sampler and Synth decks.
#[derive(Clone, Debug)]
pub struct ParamDescriptor {
    pub id: usize,
    pub module: &'static str,
    pub name: &'static str,
    pub display: String,
    /// `0.0..=1.0` normalized value for rendering sliders/gauges.
    pub normalized: f32,
}

struct RiffParse<'a> {
    fmt_tag: Option<u16>,
    data_chunk: Option<&'a [u8]>,
    meta: SampleMetadata,
}

fn read_le_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let raw: [u8; 4] = bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?;
    Some(u32::from_le_bytes(raw))
}

fn parse_riff_wave(bytes: &[u8]) -> Option<RiffParse<'_>> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let mut fmt_tag = None;
    let mut data_chunk = None;
    let mut meta = SampleMetadata::default();
    let mut has_smpl_root = false;
    let mut raw_cues: Vec<(u32, usize)> = Vec::new();
    let mut cue_labels: HashMap<u32, String> = HashMap::new();

    let mut pos = 12usize;
    while pos + 8 <= bytes.len() {
        let cid = &bytes[pos..pos + 4];
        let csz = read_le_u32(bytes, pos + 4)? as usize;
        let body_start = pos + 8;
        let body_end = body_start.saturating_add(csz).min(bytes.len());
        let body = &bytes[body_start..body_end];

        match cid {
            b"fmt " if body.len() >= 2 => {
                fmt_tag = Some(u16::from_le_bytes([body[0], body[1]]));
            }
            b"data" => {
                data_chunk = Some(body);
            }
            b"smpl" if body.len() >= 36 => {
                if let (Some(unity), Some(pitch_frac), Some(num_loops)) = (
                    read_le_u32(body, 12),
                    read_le_u32(body, 16),
                    read_le_u32(body, 28),
                ) {
                    if unity <= 127 {
                        meta.root_midi = unity as u8;
                        meta.has_root = true;
                        meta.fine_cents = (pitch_frac as f64 / f64::from(u32::MAX) * 100.0) as f32;
                        has_smpl_root = true;
                    }
                    if num_loops > 0 && body.len() >= 60 {
                        if let (Some(loop_type), Some(start), Some(end)) = (
                            read_le_u32(body, 40),
                            read_le_u32(body, 44),
                            read_le_u32(body, 48),
                        ) {
                            let start = start as usize;
                            let end = end as usize;
                            if end > start {
                                meta.loop_mode = match loop_type {
                                    1 => SampleLoopMode::PingPong,
                                    _ => SampleLoopMode::Forward,
                                };
                                meta.loop_start = start;
                                meta.loop_end = end.saturating_add(1);
                            }
                        }
                    }
                }
            }
            b"inst" if body.len() >= 2 => {
                let unshifted = body[0];
                let fine = body[1] as i8;
                if !has_smpl_root && unshifted <= 127 {
                    meta.root_midi = unshifted;
                    meta.has_root = true;
                    meta.fine_cents = f32::from(fine).clamp(-100.0, 100.0);
                }
                // Full 7-byte RIFF `inst`: [unshifted_note, fine_tune, gain, low_note, high_note, low_vel, high_vel].
                if body.len() >= 7 {
                    let lo_note = body[3];
                    let hi_note = body[4];
                    if lo_note <= 127
                        && hi_note <= 127
                        && lo_note <= hi_note
                        && !(lo_note == 0 && hi_note == 0 && meta.root_midi > 0)
                    {
                        meta.low_midi = Some(lo_note);
                        meta.high_midi = Some(hi_note);
                    }
                    let lo_vel = body[5];
                    let hi_vel = body[6];
                    if lo_vel <= 127 && hi_vel <= 127 && lo_vel <= hi_vel && hi_vel > 0 {
                        meta.low_vel = Some(lo_vel);
                        meta.high_vel = Some(hi_vel);
                    }
                }
            }
            b"cue " if body.len() >= 4 => {
                let declared = read_le_u32(body, 0).unwrap_or(0) as usize;
                let max_fit = body.len().saturating_sub(4) / 24;
                let count = declared.min(max_fit).min(MAX_RIFF_CUE_POINTS);
                for i in 0..count {
                    let off = 4 + i * 24;
                    if let (Some(cue_id), Some(dw_pos), Some(sample_off)) = (
                        read_le_u32(body, off),
                        read_le_u32(body, off + 4),
                        read_le_u32(body, off + 20),
                    ) {
                        let frame = if sample_off > 0 || dw_pos == 0 {
                            sample_off as usize
                        } else {
                            dw_pos as usize
                        };
                        raw_cues.push((cue_id, frame));
                    }
                }
            }
            b"LIST" if body.len() >= 4 && &body[0..4] == b"adtl" => {
                let mut sub = 4usize;
                while sub + 8 <= body.len() {
                    let sub_id = &body[sub..sub + 4];
                    let Some(sub_sz) = read_le_u32(body, sub + 4).map(|v| v as usize) else {
                        break;
                    };
                    let sub_start = sub + 8;
                    let sub_end = sub_start.saturating_add(sub_sz).min(body.len());
                    if sub_id == b"labl" && sub_end >= sub_start + 4 {
                        if let Some(cue_id) = read_le_u32(body, sub_start) {
                            let raw_txt = &body[sub_start + 4..sub_end];
                            let trimmed_bytes = match raw_txt.iter().position(|&b| b == 0) {
                                Some(nul) => &raw_txt[..nul],
                                None => raw_txt,
                            };
                            let text = String::from_utf8_lossy(trimmed_bytes).trim().to_string();
                            if !text.is_empty() {
                                cue_labels.entry(cue_id).or_insert(text);
                            }
                        }
                    }
                    let padded_sub = sub_sz.saturating_add(sub_sz & 1);
                    sub = sub_start.saturating_add(padded_sub);
                }
            }
            _ => {}
        }

        let padded = csz.saturating_add(csz & 1);
        pos = body_start.saturating_add(padded);
    }

    if !raw_cues.is_empty() {
        meta.slices = raw_cues
            .into_iter()
            .map(|(id, frame)| SampleSlice::new(frame, cue_labels.get(&id).cloned()))
            .collect();
        normalize_slices(&mut meta.slices, None);
    }

    Some(RiffParse {
        fmt_tag,
        data_chunk,
        meta,
    })
}

/// Encode standard RIFF metadata chunks (`inst`, `smpl`, and optional `cue ` + `LIST/adtl/labl`)
/// with exact 2-byte RIFF word alignment. Shared by [`write_wav_with_metadata`] and
/// [`update_wav_riff_metadata_bytes`].
fn encode_riff_metadata_chunks(sample_rate: u32, frames: usize, meta: &SampleMetadata) -> Vec<u8> {
    let mut out = Vec::new();
    let root = meta.root_midi.min(127);
    let fine_i8 = meta.fine_cents.round().clamp(-50.0, 50.0) as i8;
    let lo_note = meta.low_midi.unwrap_or(0).min(127);
    let hi_note = meta.high_midi.unwrap_or(127).min(127).max(lo_note);
    let lo_vel = meta.low_vel.unwrap_or(0).min(127);
    let hi_vel = meta.high_vel.unwrap_or(127).min(127).max(lo_vel).max(1);

    // 7-byte RIFF `inst` chunk (+ 1 word-alignment pad byte).
    out.extend_from_slice(b"inst");
    out.extend_from_slice(&7u32.to_le_bytes());
    out.extend_from_slice(&[root, fine_i8 as u8, 0, lo_note, hi_note, lo_vel, hi_vel]);
    out.push(0);

    // RIFF `smpl` chunk (MIDI unity note, pitch fraction, and optional loop record).
    let has_loop = meta.loop_mode != SampleLoopMode::Off && meta.loop_end > meta.loop_start;
    let smpl_body_len: usize = if has_loop { 36 + 24 } else { 36 };
    let sample_period_ns = (1_000_000_000u64 / u64::from(sample_rate.max(1))) as u32;
    let pitch_frac = if meta.fine_cents.is_finite() && meta.fine_cents > 0.0 {
        ((f64::from(meta.fine_cents.clamp(0.0, 99.999)) / 100.0) * f64::from(u32::MAX)).round()
            as u32
    } else {
        0
    };
    let mut smpl = vec![0u8; smpl_body_len];
    smpl[8..12].copy_from_slice(&sample_period_ns.to_le_bytes());
    smpl[12..16].copy_from_slice(&u32::from(root).to_le_bytes());
    smpl[16..20].copy_from_slice(&pitch_frac.to_le_bytes());
    if has_loop {
        let loop_start = meta.loop_start.min(frames.saturating_sub(1)) as u32;
        let loop_end_inclusive = meta
            .loop_end
            .min(frames)
            .saturating_sub(1)
            .max(loop_start as usize) as u32;
        let loop_type: u32 = match meta.loop_mode {
            SampleLoopMode::PingPong => 1,
            _ => 0,
        };
        smpl[28..32].copy_from_slice(&1u32.to_le_bytes());
        smpl[40..44].copy_from_slice(&loop_type.to_le_bytes());
        smpl[44..48].copy_from_slice(&loop_start.to_le_bytes());
        smpl[48..52].copy_from_slice(&loop_end_inclusive.to_le_bytes());
    }
    out.extend_from_slice(b"smpl");
    out.extend_from_slice(&(smpl.len() as u32).to_le_bytes());
    out.extend_from_slice(&smpl);

    // Standard RIFF `cue ` chunk + optional `LIST`/`adtl` (`labl`) slice markers.
    let mut slices = meta.slices.clone();
    normalize_slices(&mut slices, (frames > 0).then_some(frames));
    if !slices.is_empty() {
        let cue_body_len = 4 + slices.len() * 24;
        out.extend_from_slice(b"cue ");
        out.extend_from_slice(&(cue_body_len as u32).to_le_bytes());
        out.extend_from_slice(&(slices.len() as u32).to_le_bytes());
        for (idx, sl) in slices.iter().enumerate() {
            let cue_id = (idx as u32) + 1;
            let frame_u32 = sl.frame.min(u32::MAX as usize) as u32;
            out.extend_from_slice(&cue_id.to_le_bytes()); // dwName
            out.extend_from_slice(&frame_u32.to_le_bytes()); // dwPosition
            out.extend_from_slice(b"data"); // fccChunk
            out.extend_from_slice(&0u32.to_le_bytes()); // dwChunkStart
            out.extend_from_slice(&0u32.to_le_bytes()); // dwBlockStart
            out.extend_from_slice(&frame_u32.to_le_bytes()); // dwSampleOffset
        }

        let mut adtl_body = Vec::new();
        for (idx, sl) in slices.iter().enumerate() {
            if let Some(ref lbl) = sl.label {
                let cue_id = (idx as u32) + 1;
                let mut text_bytes = lbl.as_bytes().to_vec();
                text_bytes.push(0); // NUL-terminated per RIFF `labl` spec
                let sub_len = 4 + text_bytes.len();
                adtl_body.extend_from_slice(b"labl");
                adtl_body.extend_from_slice(&(sub_len as u32).to_le_bytes());
                adtl_body.extend_from_slice(&cue_id.to_le_bytes());
                adtl_body.extend_from_slice(&text_bytes);
                if sub_len & 1 == 1 {
                    adtl_body.push(0);
                }
            }
        }
        if !adtl_body.is_empty() {
            let list_len = 4 + adtl_body.len();
            out.extend_from_slice(b"LIST");
            out.extend_from_slice(&(list_len as u32).to_le_bytes());
            out.extend_from_slice(b"adtl");
            out.extend_from_slice(&adtl_body);
        }
    }

    out
}

/// Losslessly rewrite RIFF metadata chunks (`inst`, `smpl`, `cue `, `LIST/adtl`) inside an
/// existing RIFF WAVE byte stream while preserving `fmt `, `data`, and all other chunks
/// bit-for-bit untouched (supports 16-bit PCM, 24-bit PCM, 32-bit float, and Ogg-in-RIFF WAVs).
pub fn update_wav_riff_metadata_bytes(
    existing_wav: &[u8],
    sample_rate: u32,
    frames: usize,
    meta: &SampleMetadata,
) -> Result<Vec<u8>> {
    ensure!(
        existing_wav.len() >= 12
            && &existing_wav[0..4] == b"RIFF"
            && &existing_wav[8..12] == b"WAVE",
        "Target file is not a RIFF WAVE container"
    );
    let meta_chunks = encode_riff_metadata_chunks(sample_rate, frames, meta);
    let mut preserved = Vec::with_capacity(existing_wav.len());
    let mut pos = 12usize;
    while pos + 8 <= existing_wav.len() {
        let cid = &existing_wav[pos..pos + 4];
        let csz = read_le_u32(existing_wav, pos + 4).context("Truncated RIFF chunk size")? as usize;
        let body_start = pos + 8;
        let body_end = body_start.saturating_add(csz).min(existing_wav.len());
        let is_adtl_list = cid == b"LIST"
            && body_end >= body_start + 4
            && &existing_wav[body_start..body_start + 4] == b"adtl";
        let strip = matches!(cid, b"inst" | b"smpl" | b"cue ") || is_adtl_list;
        if !strip {
            let body = &existing_wav[body_start..body_end];
            preserved.extend_from_slice(cid);
            preserved.extend_from_slice(&(body.len() as u32).to_le_bytes());
            preserved.extend_from_slice(body);
            if body.len() & 1 == 1 {
                preserved.push(0);
            }
        }
        let padded = csz.saturating_add(csz & 1);
        pos = body_start.saturating_add(padded);
    }

    let total_len = 12usize
        .checked_add(preserved.len())
        .and_then(|n| n.checked_add(meta_chunks.len()))
        .context("RIFF WAVE size overflow")?;
    ensure!(
        total_len as u64 <= MAX_SAMPLE_FILE_BYTES,
        "Updated WAV exceeds maximum sample size"
    );
    let mut out = Vec::with_capacity(total_len);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&((total_len - 8) as u32).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(&preserved);
    out.extend_from_slice(&meta_chunks);
    Ok(out)
}

/// Write a mono (`right: None`) or stereo (`right: Some(..)`) 16-bit PCM RIFF WAVE file
/// atomically with embedded RIFF `inst` (root, fine tune, key & velocity range),
/// `smpl` (MIDI unity note, pitch fraction, optional loop points), and `cue ` / `LIST(adtl)`
/// slice marker chunks.
pub fn write_wav_with_metadata(
    path: &Path,
    left: &[f32],
    right: Option<&[f32]>,
    sample_rate: u32,
    meta: &SampleMetadata,
) -> Result<()> {
    ensure!(
        (8_000..=192_000).contains(&sample_rate),
        "WAV sample rate {sample_rate} must be within 8000..=192000 Hz"
    );
    ensure!(!left.is_empty(), "Cannot write zero-frame WAV file");
    if let Some(r) = right {
        ensure!(
            r.len() == left.len(),
            "Stereo WAV channel length mismatch ({} vs {})",
            left.len(),
            r.len()
        );
    }
    let channels: u16 = if right.is_some() { 2 } else { 1 };
    let frames = left.len();
    let data_bytes = frames
        .checked_mul(usize::from(channels))
        .and_then(|n| n.checked_mul(2))
        .context("WAV PCM size overflow")?;
    let meta_chunks = encode_riff_metadata_chunks(sample_rate, frames, meta);
    let total_len = 12 + (8 + 16) + (8 + data_bytes) + meta_chunks.len();
    ensure!(
        total_len as u64 <= MAX_SAMPLE_FILE_BYTES,
        "WAV exceeds maximum sample size"
    );

    crate::store::write_atomic(path, MAX_SAMPLE_FILE_BYTES, |buf| {
        buf.reserve(total_len);

        // RIFF WAVE header
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&((total_len - 8) as u32).to_le_bytes());
        buf.extend_from_slice(b"WAVE");

        // `fmt ` chunk (16-bit PCM)
        let block_align = channels * 2;
        let byte_rate = sample_rate * u32::from(block_align);
        buf.extend_from_slice(b"fmt ");
        buf.extend_from_slice(&16u32.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
        buf.extend_from_slice(&channels.to_le_bytes());
        buf.extend_from_slice(&sample_rate.to_le_bytes());
        buf.extend_from_slice(&byte_rate.to_le_bytes());
        buf.extend_from_slice(&block_align.to_le_bytes());
        buf.extend_from_slice(&16u16.to_le_bytes());

        // `data` chunk
        buf.extend_from_slice(b"data");
        buf.extend_from_slice(&(data_bytes as u32).to_le_bytes());
        let to_i16 = |s: f32| -> i16 {
            (crate::dsp::math::sanitize_sample(s).clamp(-1.0, 1.0) * f32::from(i16::MAX)).round()
                as i16
        };
        if let Some(r) = right {
            for i in 0..frames {
                buf.extend_from_slice(&to_i16(left[i]).to_le_bytes());
                buf.extend_from_slice(&to_i16(r[i]).to_le_bytes());
            }
        } else {
            for &s in left {
                buf.extend_from_slice(&to_i16(s).to_le_bytes());
            }
        }

        buf.extend_from_slice(&meta_chunks);
        Ok(())
    })
}

/// Decode a sample file from disk into an immutable `Arc<SampleData>`.
pub fn load_sample_file(
    path: impl AsRef<Path>,
    path_label: Option<String>,
) -> Result<Arc<SampleData>> {
    let resolved = path.as_ref().to_path_buf();
    let bytes = crate::store::read_bounded(&resolved, MAX_SAMPLE_FILE_BYTES)
        .with_context(|| format!("Could not read sample {}", resolved.display()))?;
    let name = resolved
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Sample".into());
    let label = path_label.unwrap_or_else(|| resolved.to_string_lossy().into_owned());
    decode_sample_bytes(&bytes, name, label, resolved)
}

pub fn decode_sample_bytes(
    bytes: &[u8],
    name: String,
    path_label: String,
    resolved_path: PathBuf,
) -> Result<Arc<SampleData>> {
    ensure!(!bytes.is_empty(), "Sample file is empty");

    // 1. WavPack (.wv) lossless container (`wvpk` magic)
    if bytes.len() >= 32 && &bytes[0..4] == b"wvpk" {
        return decode_wavpack(bytes, name, path_label, resolved_path);
    }

    // 2. RIFF WAVE (either standard PCM/Float OR FL Studio Ogg-in-RIFF `0x674f`/`0x6750`/`0x6771`)
    if let Some(riff) = parse_riff_wave(bytes) {
        let mut meta = riff.meta;
        let is_ogg_in_riff = matches!(riff.fmt_tag, Some(0x674f | 0x6750 | 0x6771))
            || riff
                .data_chunk
                .is_some_and(|d| d.len() >= 4 && &d[0..4] == b"OggS");

        let (sample_rate, left, right) = if is_ogg_in_riff {
            let ogg_slice = riff.data_chunk.context("Missing Ogg data chunk in RIFF")?;
            decode_with_symphonia(ogg_slice, Some("ogg"))?
        } else {
            decode_with_symphonia(bytes, Some("wav"))?
        };
        return finish_sample(
            name,
            path_label,
            resolved_path,
            sample_rate,
            left,
            right,
            &mut meta,
        );
    }

    // 3. FLAC / Ogg / MP3 / AIFF via Symphonia format probe
    let ext = resolved_path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    let mut meta = SampleMetadata::default();
    let (sample_rate, left, right) = decode_with_symphonia(bytes, ext.as_deref())?;
    finish_sample(
        name,
        path_label,
        resolved_path,
        sample_rate,
        left,
        right,
        &mut meta,
    )
}

fn decode_wavpack(
    bytes: &[u8],
    name: String,
    path_label: String,
    resolved_path: PathBuf,
) -> Result<Arc<SampleData>> {
    // Trim any trailing APEv2 / ID3v1 tags after the last valid `wvpk` block.
    let mut end = 0usize;
    let mut riff_blob = Vec::new();
    let mut output_samples = 0usize;
    while end + 32 <= bytes.len() && &bytes[end..end + 4] == b"wvpk" {
        let Some(ck_size) = read_le_u32(bytes, end + 4).map(|v| v as usize) else {
            break;
        };
        let block_end = end.saturating_add(ck_size).saturating_add(8);
        if block_end < end + 32 || block_end > bytes.len() {
            break;
        }
        if let Ok(header) = wavicle::BlockHeader::parse(&bytes[end..block_end]) {
            let channels = header.flags.output_channels() as usize;
            // The decoder expands every block into a whole-stream buffer. Check
            // actual block totals before it allocates, even with unknown length.
            output_samples = checked_sample_count(
                output_samples,
                header.block_samples as usize,
                channels,
                MAX_DECODED_SAMPLES,
            )?;
            if let Some(frames) = header.total_samples.filter(|_| header.block_index == 0) {
                ensure!(
                    frames <= (MAX_DECODED_SAMPLES / channels.max(1)) as u64,
                    "Decoded sample exceeds 128 MiB PCM limit"
                );
            }
            let block = wavicle::Block {
                header,
                metadata: &bytes[end + 32..block_end],
            };
            for sub in block.sub_blocks().flatten() {
                // WavPack ID_RIFF_HEADER (0x21) and ID_RIFF_TRAILER (0x22)
                if matches!(sub.id, 0x21 | 0x22) {
                    riff_blob.extend_from_slice(sub.data);
                }
            }
        }
        end = block_end;
    }
    ensure!(end >= 32, "Malformed WavPack stream");

    let decoded = wavicle::decode_stream(&bytes[..end])
        .map_err(|e| anyhow::anyhow!("WavPack decode error: {e:?}"))?;
    ensure!(
        decoded.channels == 1 || decoded.channels == 2,
        "Unsupported WavPack channel count {}",
        decoded.channels
    );

    let scale = if decoded.is_float {
        1.0_f32
    } else {
        let shift = decoded.bits_per_sample.clamp(8, 32) - 1;
        1.0 / ((1_u64 << shift) as f32)
    };
    let convert = |raw: i32| -> f32 {
        if decoded.is_float {
            crate::dsp::math::sanitize_sample(f32::from_bits(raw as u32)).clamp(-1.0, 1.0)
        } else {
            (raw as f32 * scale).clamp(-1.0, 1.0)
        }
    };

    let (left, right) = if decoded.channels == 1 {
        let l: Vec<f32> = decoded.samples.into_iter().map(convert).collect();
        (l, None)
    } else {
        let frames = decoded.samples.len() / 2;
        let mut l = Vec::with_capacity(frames);
        let mut r = Vec::with_capacity(frames);
        for &[left_raw, right_raw] in decoded.samples.as_chunks::<2>().0 {
            l.push(convert(left_raw));
            r.push(convert(right_raw));
        }
        (l, Some(r))
    };

    let mut meta = parse_riff_wave(&riff_blob)
        .map(|r| r.meta)
        .unwrap_or_default();
    finish_sample(
        name,
        path_label,
        resolved_path,
        decoded.sample_rate,
        left,
        right,
        &mut meta,
    )
}

fn decode_with_symphonia(
    bytes: &[u8],
    ext_hint: Option<&str>,
) -> Result<(u32, Vec<f32>, Option<Vec<f32>>)> {
    decode_with_symphonia_limit(bytes, ext_hint, MAX_DECODED_SAMPLES)
}

fn checked_sample_count(
    previous: usize,
    frames: usize,
    channels: usize,
    limit: usize,
) -> Result<usize> {
    frames
        .checked_mul(channels)
        .and_then(|n| previous.checked_add(n))
        .filter(|&n| n <= limit)
        .context("Decoded sample exceeds 128 MiB PCM limit")
}

fn decode_with_symphonia_limit(
    bytes: &[u8],
    ext_hint: Option<&str>,
    max_samples: usize,
) -> Result<(u32, Vec<f32>, Option<Vec<f32>>)> {
    let cursor = Cursor::new(bytes.to_vec());
    let mss = MediaSourceStream::new(Box::new(cursor), MediaSourceStreamOptions::default());
    let mut hint = Hint::new();
    if let Some(ext) = ext_hint {
        hint.with_extension(ext);
    }

    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .context("Unsupported or corrupted audio container")?;

    let track = format
        .default_track(TrackType::Audio)
        .context("Audio file contains no decodable audio stream")?;
    let track_id = track.id;
    let Some(CodecParameters::Audio(audio_params)) = track.codec_params.clone() else {
        anyhow::bail!("Selected track does not contain audio codec parameters");
    };
    let mut sample_rate = audio_params.sample_rate.unwrap_or(44_100);
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&audio_params, &AudioDecoderOptions::default())
        .context("Unsupported audio codec")?;

    let mut left = Vec::new();
    let mut right: Option<Vec<f32>> = None;
    let mut interleaved: Vec<f32> = Vec::new();
    let mut layout = None;

    loop {
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            Err(SymphoniaError::IoError(err))
                if err.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(SymphoniaError::ResetRequired) => break,
            Err(err) => return Err(anyhow::anyhow!("Audio packet read failed: {err}")),
        };

        if packet.track_id != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(SymphoniaError::IoError(err))
                if err.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(err) => return Err(anyhow::anyhow!("Audio decode failed: {err}")),
        };

        let spec = decoded.spec().clone();
        sample_rate = spec.rate();
        let channels = spec.channels().count().max(1);
        let current_layout = (sample_rate, channels);
        ensure!(
            layout.is_none_or(|first| first == current_layout),
            "Sample changes channel count or sample rate within its stream"
        );
        layout = Some(current_layout);
        checked_sample_count(
            left.len().saturating_mul(channels),
            decoded.frames(),
            channels,
            max_samples,
        )?;
        interleaved.clear();
        decoded.copy_to_vec_interleaved(&mut interleaved);

        if channels == 1 {
            left.extend(
                interleaved
                    .iter()
                    .map(|&s| crate::dsp::math::sanitize_sample(s).clamp(-1.0, 1.0)),
            );
        } else {
            let r_vec = right.get_or_insert_with(|| Vec::with_capacity(left.len()));
            for frame in interleaved.chunks_exact(channels) {
                left.push(crate::dsp::math::sanitize_sample(frame[0]).clamp(-1.0, 1.0));
                r_vec.push(crate::dsp::math::sanitize_sample(frame[1]).clamp(-1.0, 1.0));
            }
        }
    }

    ensure!(
        !left.is_empty(),
        "Decoded audio sample contains zero frames"
    );
    Ok((sample_rate, left, right))
}

fn finish_sample(
    name: String,
    path_label: String,
    resolved_path: PathBuf,
    sample_rate: u32,
    left: Vec<f32>,
    right: Option<Vec<f32>>,
    meta: &mut SampleMetadata,
) -> Result<Arc<SampleData>> {
    ensure!(
        (4_000..=384_000).contains(&sample_rate),
        "Unsupported sample rate {sample_rate} Hz"
    );
    let frames = left.len();
    ensure!(frames > 0, "Sample contains zero frames");
    if meta.loop_mode != SampleLoopMode::Off {
        meta.loop_end = meta.loop_end.min(frames);
        meta.loop_start = meta.loop_start.min(meta.loop_end.saturating_sub(1));
        if meta.loop_end <= meta.loop_start {
            meta.loop_mode = SampleLoopMode::Off;
        }
    }
    normalize_slices(&mut meta.slices, Some(frames));
    let peaks =
        crate::dsp::compute_waveform_peaks_window(&left, right.as_deref(), 0, frames, PEAK_BINS);
    let channels = if right.is_some() { 2 } else { 1 };
    Ok(Arc::new(SampleData {
        path_label,
        resolved_path,
        name,
        sample_rate,
        channels,
        frames,
        left: left.into_boxed_slice(),
        right: right.map(Vec::into_boxed_slice),
        peaks: peaks.into_boxed_slice(),
        meta: meta.clone(),
    }))
}

#[derive(Clone, Debug)]
pub struct SampleEntry {
    /// Portable relative label such as `demo/reese-bass.wav` or `Drums/Kicks/808 Kick.wav`
    pub label: String,
    pub category: String,
    pub name: String,
    pub path: PathBuf,
    search_key: String,
}

/// Indexed sample browser and bounded decoded cache across portable roots.
pub struct SampleLibrary {
    roots: Vec<(Option<String>, PathBuf)>,
    global_roots: Vec<(Option<String>, PathBuf)>,
    project_dir: Option<PathBuf>,
    entries: Vec<SampleEntry>,
    filtered: Vec<usize>,
    pub query: String,
    pub cursor: usize,
    cache: HashMap<PathBuf, Arc<SampleData>>,
    cache_order: VecDeque<PathBuf>,
    index_truncated: bool,
    include_project_assets: bool,
}

/// Source context for the self-contained example loaded when no project is supplied.
pub fn bundled_demo_project_path() -> PathBuf {
    let rel = Path::new("examples/demo/project.json");
    if let Some(root) = option_env!("CADENCE_WORKSPACE_DIR").filter(|s| !s.is_empty()) {
        let candidate = Path::new(root).join(rel);
        if candidate.exists() {
            return candidate;
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(rel);
            if candidate.exists() {
                return candidate;
            }
        }
    }
    rel.to_path_buf()
}

impl SampleLibrary {
    pub fn discover(extra_dirs: &[PathBuf], project_path: Option<&Path>) -> Self {
        let user_cfg = crate::config::UserConfig::load_default();
        Self::discover_with_config(extra_dirs, project_path, &user_cfg)
    }

    pub fn discover_with_config(
        extra_dirs: &[PathBuf],
        project_path: Option<&Path>,
        user_cfg: &crate::config::UserConfig,
    ) -> Self {
        let mut roots: Vec<(Option<String>, PathBuf)> = Vec::new();
        let mut push_unique = |prefix: Option<&str>, candidate: PathBuf| {
            let resolved = candidate
                .canonicalize()
                .unwrap_or_else(|_| candidate.clone());
            if resolved.is_dir() && !roots.iter().any(|(_, r)| r == &resolved) {
                roots.push((prefix.map(str::to_string), resolved));
            }
        };

        for dir in extra_dirs {
            push_unique(None, dir.clone());
        }
        for raw_dir in &user_cfg.sample_dirs {
            let trimmed = raw_dir.trim();
            if trimmed.is_empty() {
                continue;
            }
            push_unique(None, crate::store::expand_tilde(trimmed));
        }
        push_unique(None, PathBuf::from("samples"));
        // External MIT factory kit samples (`./factory-samples`, workspace, or exe sibling).
        // Absent `factory-samples/` is silently skipped (graceful non-fatal missing-sample report).
        push_unique(Some("factory-samples"), PathBuf::from("factory-samples"));
        if let Some(ws) = option_env!("CADENCE_WORKSPACE_DIR").filter(|s| !s.is_empty()) {
            push_unique(
                Some("factory-samples"),
                Path::new(ws).join("factory-samples"),
            );
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                push_unique(Some("factory-samples"), dir.join("factory-samples"));
            }
        }
        // The bundled example remains available in the browser from any cwd.
        if let Some(demo_dir) = bundled_demo_project_path().parent() {
            push_unique(Some("demo"), demo_dir.join("samples"));
        }

        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            for rel in [Path::new("Music").join("Samples"), PathBuf::from("Samples")] {
                let dir = home.join(rel);
                if dir.is_dir() {
                    push_unique(None, dir);
                }
            }
        }

        let mut lib = Self {
            global_roots: roots.clone(),
            project_dir: None,
            roots,
            entries: Vec::new(),
            filtered: Vec::new(),
            query: String::new(),
            cursor: 0,
            cache: HashMap::new(),
            cache_order: VecDeque::new(),
            index_truncated: false,
            include_project_assets: true,
        };
        lib.set_project_path(project_path.unwrap_or(&bundled_demo_project_path()));
        lib
    }

    /// Deterministic library restricted strictly to explicit `(prefix, dir)` roots (plus optional
    /// project assets), bypassing host `./samples/`, `~/Samples`, and `~/.config/cadence/config.json`.
    pub fn from_prefixed_roots(
        roots: &[(Option<&str>, PathBuf)],
        project_path: Option<&Path>,
        include_project_assets: bool,
    ) -> Self {
        let mut unique: Vec<(Option<String>, PathBuf)> = Vec::new();
        for (prefix, root) in roots {
            let path = root.canonicalize().unwrap_or_else(|_| root.clone());
            if path.is_dir() && !unique.iter().any(|(_, p)| p == &path) {
                unique.push((prefix.map(str::to_string), path));
            }
        }
        let mut library = Self {
            roots: unique.clone(),
            global_roots: unique,
            project_dir: None,
            entries: Vec::new(),
            filtered: Vec::new(),
            query: String::new(),
            cursor: 0,
            cache: HashMap::new(),
            cache_order: VecDeque::new(),
            index_truncated: false,
            include_project_assets,
        };
        if let Some(path) = project_path {
            library.set_project_path(path);
        } else {
            library.rescan();
        }
        library
    }

    /// A deterministic library with only explicit roots plus project assets.
    /// Useful to embed Cadence or exercise a portable bundle without user defaults.
    pub fn from_roots(roots: &[PathBuf], project_path: Option<&Path>) -> Self {
        let prefixed: Vec<(Option<&str>, PathBuf)> =
            roots.iter().cloned().map(|r| (None, r)).collect();
        Self::from_prefixed_roots(&prefixed, project_path, true)
    }

    /// Browser catalog limited to explicitly supplied roots. Project-relative
    /// references still resolve, but do not implicitly add another browse root.
    pub fn from_roots_only(roots: &[PathBuf], project_path: Option<&Path>) -> Self {
        let prefixed: Vec<(Option<&str>, PathBuf)> =
            roots.iter().cloned().map(|r| (None, r)).collect();
        Self::from_prefixed_roots(&prefixed, project_path, false)
    }
    pub fn index_truncated(&self) -> bool {
        self.index_truncated
    }
    /// Exact-path resource reuse for asynchronous browser decoding.
    pub fn cached_path(&self, path: &Path) -> Option<Arc<SampleData>> {
        let path = path.canonicalize().ok()?;
        self.cache.get(&path).cloned()
    }
    pub fn cache_prepared(&mut self, path: &Path, sample: Arc<SampleData>) {
        let Ok(path) = path.canonicalize() else {
            return;
        };
        if sample.resolved_path.canonicalize().ok().as_ref() != Some(&path) {
            return;
        }
        self.cache.remove(&path);
        self.cache_order.retain(|p| p != &path);
        let bytes = sample_cache_bytes(&sample);
        if bytes > MAX_CACHE_BYTES {
            return;
        }
        let mut total: usize = self.cache.values().map(|s| sample_cache_bytes(s)).sum();
        while self.cache.len() >= 64 || total.saturating_add(bytes) > MAX_CACHE_BYTES {
            let Some(oldest) = self.cache_order.pop_front() else {
                break;
            };
            if let Some(old) = self.cache.remove(&oldest) {
                total = total.saturating_sub(sample_cache_bytes(&old));
            }
        }
        self.cache_order.push_back(path.clone());
        self.cache.insert(path, sample);
    }
    pub fn load_entry(&mut self, entry: &SampleEntry) -> Result<Arc<SampleData>> {
        let path = entry
            .path
            .canonicalize()
            .with_context(|| format!("Sample missing: {}", entry.path.display()))?;
        if let Some(sample) = self.cache.get(&path).cloned() {
            self.cache_order.retain(|p| p != &path);
            self.cache_order.push_back(path);
            return Ok(sample);
        }
        let sample = load_sample_file(&path, Some(entry.label.clone()))?;
        self.cache_prepared(&path, Arc::clone(&sample));
        Ok(sample)
    }

    /// Explicit reload of a specific catalog entry, bypassing any cached decode.
    pub fn reload_entry(&mut self, entry: &SampleEntry) -> Result<Arc<SampleData>> {
        if let Ok(path) = entry.path.canonicalize() {
            self.cache.remove(&path);
            self.cache_order.retain(|p| p != &path);
        }
        self.load_entry(entry)
    }

    /// Rebase project-relative assets when opening another score in this session.
    /// The project's samples directory precedes configured library aliases.
    pub fn set_project_path(&mut self, path: &Path) {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let absolute = if parent.is_absolute() {
            parent.to_path_buf()
        } else {
            std::env::current_dir().unwrap_or_default().join(parent)
        };
        let project_dir = absolute.canonicalize().unwrap_or(absolute);
        if self.project_dir.as_ref() == Some(&project_dir) {
            return;
        }
        self.project_dir = Some(project_dir.clone());
        self.roots.clear();
        let candidate = project_dir.join("samples");
        if self.include_project_assets && candidate.is_dir() {
            let resolved = candidate.canonicalize().unwrap_or(candidate);
            self.roots.push((None, resolved));
        }
        for (prefix, root) in &self.global_roots {
            if !self.roots.iter().any(|(_, p)| p == root) {
                self.roots.push((prefix.clone(), root.clone()));
            }
        }
        self.invalidate_decoded_cache();
        self.rescan();
    }

    pub fn project_directory(&self) -> Option<&Path> {
        self.project_dir.as_deref()
    }

    /// Anchor `project_dir` when unset without evicting decoded caches or triggering a full rescan.
    pub fn set_project_dir_if_unset(&mut self, dir: &Path) {
        if self.project_dir.is_some() {
            return;
        }
        let absolute = if dir.is_absolute() {
            dir.to_path_buf()
        } else {
            std::env::current_dir().unwrap_or_default().join(dir)
        };
        self.project_dir = Some(absolute.canonicalize().unwrap_or(absolute));
    }

    /// Explicit reloads re-read assets even when their path has not changed.
    pub fn invalidate_decoded_cache(&mut self) {
        self.cache.clear();
        self.cache_order.clear();
    }

    pub fn total_count(&self) -> usize {
        self.entries.len()
    }

    pub fn filtered_count(&self) -> usize {
        self.filtered.len()
    }

    pub fn visible_entries(&self) -> impl Iterator<Item = (usize, &SampleEntry)> {
        self.filtered
            .iter()
            .enumerate()
            .map(|(vis_idx, &entry_idx)| (vis_idx, &self.entries[entry_idx]))
    }

    pub fn selected_entry(&self) -> Option<&SampleEntry> {
        let idx = *self.filtered.get(self.cursor)?;
        self.entries.get(idx)
    }

    fn build_entry(prefix: Option<&str>, root: &Path, child: PathBuf) -> SampleEntry {
        let rel = child.strip_prefix(root).unwrap_or(&child);
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        let label = match prefix {
            Some(p) => format!("{p}/{rel_str}"),
            None => rel_str,
        };
        let parent = rel
            .parent()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        let category = match (prefix, parent.is_empty()) {
            (Some(p), true) => p.to_string(),
            (Some(p), false) => format!("{p}/{parent}"),
            (None, _) => parent,
        };
        let name = child
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| label.clone());
        let search_key = format!("{} {}", label, child.display()).to_ascii_lowercase();
        SampleEntry {
            label,
            category,
            name,
            path: child,
            search_key,
        }
    }

    /// Incrementally register (or refresh) a single newly created audio file without rescanning
    /// all configured sample roots or evicting unrelated decoded buffers from cache.
    pub fn register_file(&mut self, path: &Path) -> bool {
        if !path.is_file() || !is_supported_sample_ext(path) {
            return false;
        }
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.cache.remove(&canonical);
        self.cache_order.retain(|p| p != &canonical);

        let entry = self
            .roots
            .iter()
            .find_map(|(prefix, root)| {
                let root_canon = root.canonicalize().unwrap_or_else(|_| root.clone());
                if canonical.starts_with(&root_canon) {
                    Some(Self::build_entry(
                        prefix.as_deref(),
                        &root_canon,
                        canonical.clone(),
                    ))
                } else if path.starts_with(root) {
                    Some(Self::build_entry(
                        prefix.as_deref(),
                        root,
                        path.to_path_buf(),
                    ))
                } else {
                    None
                }
            })
            .unwrap_or_else(|| {
                let fallback_root = self
                    .project_dir
                    .as_deref()
                    .and_then(|d| d.canonicalize().ok().or_else(|| Some(d.to_path_buf())))
                    .or_else(|| canonical.parent().map(Path::to_path_buf))
                    .unwrap_or_default();
                Self::build_entry(None, &fallback_root, canonical.clone())
            });

        self.entries.retain(|existing| {
            existing.path != entry.path
                && existing
                    .path
                    .canonicalize()
                    .map_or(true, |c| c != canonical)
        });
        self.entries.push(entry);
        self.entries
            .sort_by(|a, b| a.label.cmp(&b.label).then(a.path.cmp(&b.path)));
        self.update_filter();
        true
    }

    pub fn rescan(&mut self) {
        self.rescan_with_limits(MAX_LIBRARY_ENTRIES, MAX_LIBRARY_SCAN_ITEMS);
    }

    fn rescan_with_limits(&mut self, max_entries: usize, max_items: usize) {
        self.entries.clear();
        self.index_truncated = false;
        let mut visited = HashSet::new();
        let mut scanned = 0usize;
        'roots: for (prefix, root) in &self.roots {
            let mut stack = vec![root.clone()];
            while let Some(dir) = stack.pop() {
                let Ok(real_dir) = dir.canonicalize() else {
                    continue;
                };
                if !visited.insert(real_dir) {
                    continue;
                }
                let Ok(read_dir) = fs::read_dir(&dir) else {
                    continue;
                };
                let remaining = max_items.saturating_sub(scanned);
                let mut children: Vec<PathBuf> = read_dir
                    .flatten()
                    .take(remaining.saturating_add(1))
                    .map(|entry| entry.path())
                    .collect();
                if children.len() > remaining {
                    children.truncate(remaining);
                    self.index_truncated = true;
                }
                scanned = scanned.saturating_add(children.len());
                children.sort();
                for child in children {
                    // Directory symlinks can cycle or escape the selected catalog.
                    // Explicit symlink roots were canonicalized when registered.
                    let Ok(meta) = fs::symlink_metadata(&child) else {
                        continue;
                    };
                    if meta.file_type().is_dir() {
                        stack.push(child);
                    } else if child.is_file() && is_supported_sample_ext(&child) {
                        if self.entries.len() >= max_entries {
                            self.index_truncated = true;
                            break 'roots;
                        }
                        self.entries
                            .push(Self::build_entry(prefix.as_deref(), root, child));
                    }
                }
                if scanned >= max_items {
                    self.index_truncated = true;
                    break 'roots;
                }
            }
        }
        self.entries
            .sort_by(|a, b| a.label.cmp(&b.label).then(a.path.cmp(&b.path)));
        self.update_filter();
    }

    pub fn set_query(&mut self, query: String) {
        self.query = query;
        self.update_filter();
    }

    fn update_filter(&mut self) {
        let tokens: Vec<String> = self
            .query
            .split_whitespace()
            .map(str::to_ascii_lowercase)
            .collect();
        if tokens.is_empty() {
            self.filtered = (0..self.entries.len()).collect();
        } else {
            self.filtered = self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, entry)| tokens.iter().all(|tok| entry.search_key.contains(tok)))
                .map(|(idx, _)| idx)
                .collect();
        }
        self.cursor = self.cursor.min(self.filtered.len().saturating_sub(1));
    }

    pub fn move_cursor(&mut self, delta: isize) {
        if self.filtered.is_empty() {
            self.cursor = 0;
            return;
        }
        let max = (self.filtered.len() - 1) as isize;
        self.cursor = (self.cursor as isize + delta).clamp(0, max) as usize;
    }

    /// Resolve either a library-relative label (`Drums/Kicks/...`) or a direct path,
    /// caching up to 64 decoded `Arc<SampleData>` buffers in memory.
    pub fn load(&mut self, label_or_path: &str) -> Result<Arc<SampleData>> {
        let resolved = self
            .resolve_path(label_or_path)
            .with_context(|| format!("Sample not found: {label_or_path}"))?
            .canonicalize()?;
        if let Some(sample) = self.cache.get(&resolved).cloned() {
            self.cache_order.retain(|p| p != &resolved);
            self.cache_order.push_back(resolved);
            return Ok(sample);
        }
        let sample = load_sample_file(&resolved, Some(label_or_path.to_string()))?;
        self.cache_prepared(&resolved, Arc::clone(&sample));
        Ok(sample)
    }

    /// Explicit user load: bypass a previous decoded buffer for this file.
    pub fn reload(&mut self, label_or_path: &str) -> Result<Arc<SampleData>> {
        if let Some(resolved) = self.resolve_path(label_or_path) {
            let resolved = resolved.canonicalize().unwrap_or(resolved);
            self.cache.remove(&resolved);
            self.cache_order.retain(|p| p != &resolved);
        }
        self.load(label_or_path)
    }

    /// Returns the catalog label for a canonicalized file inside a configured
    /// sample root, if one matches.
    pub fn catalog_label_for(&self, resolved: &Path) -> Option<String> {
        let canonical = resolved
            .canonicalize()
            .unwrap_or_else(|_| resolved.to_path_buf());
        if let Some(entry) = self
            .entries
            .iter()
            .find(|e| e.path.canonicalize().unwrap_or_else(|_| e.path.clone()) == canonical)
        {
            return Some(entry.label.clone());
        }
        for (prefix, root) in &self.global_roots {
            if let Ok(rel) = canonical.strip_prefix(root) {
                let rel = rel.to_string_lossy().replace('\\', "/");
                return Some(match prefix {
                    Some(p) => format!("{p}/{rel}"),
                    None => rel,
                });
            }
        }
        None
    }

    pub fn resolve_path(&self, label_or_path: &str) -> Option<PathBuf> {
        let candidate = Path::new(label_or_path);
        if candidate.is_absolute() {
            return None;
        }
        if let Some(project_dir) = &self.project_dir {
            let project_relative = project_dir.join(candidate);
            if project_relative.is_file() {
                return Some(project_relative);
            }
            // An explicit relative pathname is not a portable library alias.
            if matches!(
                candidate.components().next(),
                Some(std::path::Component::CurDir | std::path::Component::ParentDir)
            ) {
                return None;
            }
        }
        if let Some(entry) = self
            .entries
            .iter()
            .find(|e| e.label == label_or_path && e.path.is_file())
        {
            return Some(entry.path.clone());
        }
        for (prefix, root) in &self.roots {
            if let Some(p) = prefix {
                if let Some(stripped) = label_or_path.strip_prefix(&format!("{p}/")) {
                    let joined = root.join(stripped);
                    if joined.is_file() {
                        return Some(joined);
                    }
                }
            }
            let joined = root.join(label_or_path);
            if joined.is_file() {
                return Some(joined);
            }
        }
        None
    }

    /// Resolve a folder locator relative to the active project directory or configured sample roots.
    pub fn resolve_dir(&self, folder: &str) -> Option<PathBuf> {
        let trimmed = folder.trim();
        if trimmed.is_empty() {
            return None;
        }
        let candidate = Path::new(trimmed);
        if candidate.is_absolute() {
            return candidate.is_dir().then(|| candidate.to_path_buf());
        }
        if let Some(project_dir) = &self.project_dir {
            let project_relative = project_dir.join(candidate);
            if project_relative.is_dir() {
                return Some(project_relative);
            }
            if matches!(
                candidate.components().next(),
                Some(std::path::Component::CurDir | std::path::Component::ParentDir)
            ) {
                return None;
            }
        }
        for (prefix, root) in &self.roots {
            if let Some(p) = prefix {
                if trimmed == p {
                    if root.is_dir() {
                        return Some(root.clone());
                    }
                } else if let Some(stripped) = trimmed.strip_prefix(&format!("{p}/")) {
                    let joined = root.join(stripped);
                    if joined.is_dir() {
                        return Some(joined);
                    }
                }
            }
            let joined = root.join(trimmed);
            if joined.is_dir() {
                return Some(joined);
            }
        }
        None
    }

    /// List supported audio files inside a resolved folder, sorted deterministically by filename.
    pub fn list_folder_samples(&self, folder: &str) -> Result<Vec<(String, PathBuf)>> {
        const MAX_FOLDER_SAMPLE_ENTRIES: usize = 2_048;
        let dir = self
            .resolve_dir(folder)
            .with_context(|| format!("Sample folder not found: {folder}"))?;
        let dir = dir.canonicalize().unwrap_or(dir);
        let mut files: Vec<PathBuf> = fs::read_dir(&dir)
            .with_context(|| format!("Could not read sample folder {}", dir.display()))?
            .take(MAX_FOLDER_SAMPLE_ENTRIES)
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file() && is_supported_sample_ext(p))
            .collect();
        files.sort_by(|a, b| {
            let na = a
                .file_name()
                .map(|s| s.to_string_lossy().to_ascii_lowercase());
            let nb = b
                .file_name()
                .map(|s| s.to_string_lossy().to_ascii_lowercase());
            na.cmp(&nb).then_with(|| a.cmp(b))
        });
        ensure!(
            !files.is_empty(),
            "No supported audio samples (.wav, .wv, .flac, .ogg, .mp3, .aif, .aiff) found in '{folder}'"
        );
        Ok(files
            .into_iter()
            .map(|path| {
                let name = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Sample".into());
                (name, path)
            })
            .collect())
    }
}

/// Parse an embedded scientific pitch token (`C4`, `F#3`, `Db5`, `A#2ogg`, `C-1`, etc.)
/// from a sample filename or label without false positives on words (`Cello`, `Brass`)
/// or bare track/index numbers (`(1)`, `Take_2`, `A440`).
pub fn parse_filename_midi(name: &str) -> Option<u8> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name).trim();
    if base.is_empty() {
        return None;
    }
    let stem = strip_audio_ext_dot(base);
    let chunks: Vec<&str> = stem
        .split([' ', '_', '.', '(', ')', '[', ']'])
        .filter(|s| !s.is_empty())
        .collect();
    for &chunk in chunks.iter().rev() {
        if let Some(midi) = parse_note_chunk(chunk) {
            return Some(midi);
        }
    }
    None
}

fn strip_audio_ext_dot(name: &str) -> &str {
    if let Some((stem, ext)) = name.rsplit_once('.') {
        if matches!(
            ext.to_ascii_lowercase().as_str(),
            "wav" | "wv" | "flac" | "ogg" | "mp3" | "aif" | "aiff"
        ) && !stem.is_empty()
        {
            return stem;
        }
    }
    name
}

fn strip_glued_audio_suffix(tok: &str) -> &str {
    let lower = tok.to_ascii_lowercase();
    for suffix in ["aiff", "flac", "wav", "ogg", "mp3", "aif", "wv"] {
        if lower.ends_with(suffix) && tok.len() > suffix.len() {
            let prefix = &tok[..tok.len() - suffix.len()];
            if prefix.chars().last().is_some_and(|c| c.is_ascii_digit()) {
                return prefix;
            }
        }
    }
    tok
}

fn parse_note_chunk(chunk: &str) -> Option<u8> {
    if let Some(m) = parse_exact_note_token(chunk) {
        return Some(m);
    }
    let parts: Vec<&str> = chunk.split('-').collect();
    if parts.len() <= 1 {
        return None;
    }
    let mut i = parts.len();
    while i > 0 {
        i -= 1;
        // Handle negative octave `-1` (e.g. `Bass-C-1` or `C#-1`).
        if parts[i] == "1" && i > 0 {
            let combined = format!("{}-1", parts[i - 1]);
            if let Some(m) = parse_exact_note_token(&combined) {
                return Some(m);
            }
        }
        if let Some(m) = parse_exact_note_token(parts[i]) {
            return Some(m);
        }
    }
    None
}

fn parse_exact_note_token(raw: &str) -> Option<u8> {
    let tok = strip_glued_audio_suffix(raw.trim());
    let mut chars = tok.chars().peekable();
    let letter = chars.next()?;
    let base: i32 = match letter.to_ascii_uppercase() {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let accidental: i32 = match chars.peek().copied() {
        Some('#' | '♯') => {
            chars.next();
            1
        }
        Some('b' | '♭') => {
            chars.next();
            -1
        }
        _ => 0,
    };
    let oct_str: String = chars.collect();
    if oct_str.is_empty() {
        return None;
    }
    // Only allow strict scientific octaves `-1` or single digit `0..=9` (rejects `A440`, `C12`, etc.).
    let octave: i32 = if oct_str == "-1" {
        -1
    } else if oct_str.len() == 1 && oct_str.as_bytes()[0].is_ascii_digit() {
        i32::from(oct_str.as_bytes()[0] - b'0')
    } else {
        return None;
    };
    let midi = (octave + 1) * 12 + base + accidental;
    (0..=127).contains(&midi).then_some(midi as u8)
}

fn sample_cache_bytes(sample: &SampleData) -> usize {
    sample
        .left
        .len()
        .saturating_add(sample.right.as_ref().map_or(0, |r| r.len()))
        .saturating_mul(std::mem::size_of::<f32>())
        .saturating_add(
            sample
                .peaks
                .len()
                .saturating_mul(std::mem::size_of::<(f32, f32)>()),
        )
}

pub fn is_supported_sample_ext(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "wav" | "wv" | "flac" | "ogg" | "mp3" | "aif" | "aiff"
            )
        })
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod decode_and_metadata_tests {
    use super::*;

    #[test]
    fn float_wav_decoder_sanitizes_nan_and_inf_samples() {
        let mut cursor = Cursor::new(Vec::<u8>::new());
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        {
            let mut writer = hound::WavWriter::new(&mut cursor, spec).unwrap();
            let samples = [
                0.25_f32,
                -0.25,
                f32::NAN,
                f32::INFINITY,
                f32::NEG_INFINITY,
                2.5,
                -3.0,
                0.5,
            ];
            for s in samples {
                writer.write_sample(s).unwrap();
            }
            writer.finalize().unwrap();
        }
        let bytes = cursor.into_inner();
        let data = decode_sample_bytes(
            &bytes,
            "nan_test".into(),
            "nan_test.wav".into(),
            PathBuf::from("nan_test.wav"),
        )
        .unwrap();
        assert_eq!(data.frames, 4);
        assert!(data
            .left
            .iter()
            .all(|s| s.is_finite() && (-1.0..=1.0).contains(s)));
        let right = data.right.as_ref().unwrap();
        assert!(right
            .iter()
            .all(|s| s.is_finite() && (-1.0..=1.0).contains(s)));
        assert_eq!(&*data.left, &[0.25, 0.0, 0.0, -1.0]);
        assert_eq!(&**right, &[-0.25, 0.0, 1.0, 0.5]);
    }

    fn build_wav_with_chunks(
        frames: usize,
        inst: Option<[u8; 7]>,
        smpl: Option<(u32, u32, u32, u32)>,
    ) -> Vec<u8> {
        let mut cursor = Cursor::new(Vec::<u8>::new());
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        {
            let mut w = hound::WavWriter::new(&mut cursor, spec).unwrap();
            for n in 0..frames {
                w.write_sample((((n as f32) * 0.05).sin() * 12_000.0) as i16)
                    .unwrap();
            }
            w.finalize().unwrap();
        }
        let mut bytes = cursor.into_inner();
        if let Some(inst_bytes) = inst {
            bytes.extend_from_slice(b"inst");
            bytes.extend_from_slice(&7u32.to_le_bytes());
            bytes.extend_from_slice(&inst_bytes);
            bytes.push(0); // RIFF word-alignment pad
        }
        if let Some((root, loop_type, start, end)) = smpl {
            let mut body = vec![0u8; 36 + 24];
            body[12..16].copy_from_slice(&root.to_le_bytes());
            body[28..32].copy_from_slice(&1u32.to_le_bytes()); // num_loops = 1
            body[40..44].copy_from_slice(&loop_type.to_le_bytes());
            body[44..48].copy_from_slice(&start.to_le_bytes());
            body[48..52].copy_from_slice(&end.to_le_bytes());
            bytes.extend_from_slice(b"smpl");
            bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
            bytes.extend_from_slice(&body);
        }
        let riff_size = (bytes.len() - 8) as u32;
        bytes[4..8].copy_from_slice(&riff_size.to_le_bytes());
        bytes
    }

    #[test]
    fn riff_inst_and_smpl_parse_full_seven_bytes_and_reject_degenerate_bounds() {
        // Both smpl (root 64, ping-pong loop 100..900) and inst (bounds 60..72, vel 64..127).
        let wav = build_wav_with_chunks(
            1200,
            Some([60, 0, 0, 60, 72, 64, 127]),
            Some((64, 1, 100, 900)),
        );
        let data = decode_sample_bytes(
            &wav,
            "str".into(),
            "str.wav".into(),
            PathBuf::from("str.wav"),
        )
        .unwrap();
        assert!(data.meta.has_root);
        assert_eq!(data.meta.root_midi, 64); // smpl root takes priority
        assert_eq!(data.meta.loop_mode, SampleLoopMode::PingPong);
        assert_eq!(data.meta.loop_start, 100);
        assert_eq!(data.meta.loop_end, 901);
        assert_eq!(data.meta.low_midi, Some(60));
        assert_eq!(data.meta.high_midi, Some(72));
        assert_eq!(data.meta.low_vel, Some(64));
        assert_eq!(data.meta.high_vel, Some(127));

        // inst-only with fine tune -15 cents and valid bounds; zero-filled velocity ignored.
        let inst_only =
            build_wav_with_chunks(512, Some([55, (-15i8) as u8, 0, 48, 60, 0, 0]), None);
        let d2 = decode_sample_bytes(
            &inst_only,
            "g".into(),
            "g.wav".into(),
            PathBuf::from("g.wav"),
        )
        .unwrap();
        assert!(d2.meta.has_root);
        assert_eq!(d2.meta.root_midi, 55);
        assert!((d2.meta.fine_cents + 15.0).abs() < 1e-4);
        assert_eq!(d2.meta.low_midi, Some(48));
        assert_eq!(d2.meta.high_midi, Some(60));
        assert_eq!(d2.meta.low_vel, None);
        assert_eq!(d2.meta.high_vel, None);

        // Degenerate low > high ignored cleanly.
        let degen = build_wav_with_chunks(512, Some([60, 0, 0, 80, 50, 100, 20]), None);
        let d3 = decode_sample_bytes(&degen, "d".into(), "d.wav".into(), PathBuf::from("d.wav"))
            .unwrap();
        assert_eq!(d3.meta.low_midi, None);
        assert_eq!(d3.meta.high_midi, None);
        assert_eq!(d3.meta.low_vel, None);
        assert_eq!(d3.meta.high_vel, None);
    }

    #[test]
    fn parse_filename_midi_recognizes_conventions_and_avoids_false_positives() {
        assert_eq!(parse_filename_midi("OSTR C4.wav"), Some(60));
        assert_eq!(parse_filename_midi("Bass_A#2ogg.ogg"), Some(46));
        assert_eq!(parse_filename_midi("Violin-F#3.flac"), Some(54));
        assert_eq!(parse_filename_midi("Piano_Db5.wav"), Some(73));
        assert_eq!(parse_filename_midi("C#1.wav"), Some(25));
        assert_eq!(parse_filename_midi("G2"), Some(43));
        assert_eq!(parse_filename_midi("Sub_C-1.wav"), Some(0));
        assert_eq!(parse_filename_midi("Bell_G9.wav"), Some(127));

        // Non-pitch words & track numbers must not false-positive.
        assert_eq!(parse_filename_midi("Cello.wav"), None);
        assert_eq!(parse_filename_midi("Brass_Section.wav"), None);
        assert_eq!(parse_filename_midi("Track (1).wav"), None);
        assert_eq!(parse_filename_midi("Tone_A440.wav"), None);
        assert_eq!(parse_filename_midi("Kick_808.wav"), None);
    }

    #[test]
    fn write_wav_with_metadata_roundtrips_riff_inst_smpl_and_pcm() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pad.wav");
        let left: Vec<f32> = (0..1024).map(|i| ((i as f32) * 0.08).sin() * 0.6).collect();
        let right: Vec<f32> = (0..1024).map(|i| ((i as f32) * 0.08).cos() * 0.5).collect();
        let meta = SampleMetadata {
            root_midi: 36,
            has_root: true,
            fine_cents: 0.0,
            low_midi: Some(36),
            high_midi: Some(36),
            low_vel: Some(0),
            high_vel: Some(127),
            loop_mode: SampleLoopMode::Forward,
            loop_start: 120,
            loop_end: 820,
            slices: vec![
                SampleSlice::new(0, Some("Kick".into())),
                SampleSlice::new(256, Some("Snare".into())),
                SampleSlice::new(512, None),
                SampleSlice::new(768, Some("Open Hat".into())),
            ],
        };
        write_wav_with_metadata(&path, &left, Some(&right), 48_000, &meta).unwrap();
        let loaded = load_sample_file(&path, None).unwrap();
        assert_eq!(loaded.sample_rate, 48_000);
        assert_eq!(loaded.channels, 2);
        assert_eq!(loaded.frames, 1024);
        assert!(loaded.meta.has_root);
        assert_eq!(loaded.meta.root_midi, 36);
        assert_eq!(loaded.meta.low_midi, Some(36));
        assert_eq!(loaded.meta.high_midi, Some(36));
        assert_eq!(loaded.meta.low_vel, Some(0));
        assert_eq!(loaded.meta.high_vel, Some(127));
        assert_eq!(loaded.meta.loop_mode, SampleLoopMode::Forward);
        assert_eq!(loaded.meta.loop_start, 120);
        assert_eq!(loaded.meta.loop_end, 820);
        assert_eq!(loaded.meta.slices, meta.slices);
        for i in 0..1024 {
            assert!((loaded.left[i] - left[i]).abs() < 1e-3);
            assert!((loaded.right.as_ref().unwrap()[i] - right[i]).abs() < 1e-3);
        }

        // Lossless RIFF metadata update replaces `cue `/`labl`/`smpl` while preserving raw `data` and custom chunks!
        let mut orig_bytes = fs::read(&path).unwrap();
        let custom_payload = b"CUSTOM!"; // 7 bytes (odd length -> tests RIFF pad handling)
        let orig_riff_sz = u32::from_le_bytes(orig_bytes[4..8].try_into().unwrap());
        orig_bytes.extend_from_slice(b"JUNK");
        orig_bytes.extend_from_slice(&(custom_payload.len() as u32).to_le_bytes());
        orig_bytes.extend_from_slice(custom_payload);
        orig_bytes.push(0);
        let new_riff_sz = orig_riff_sz + 8 + 8;
        orig_bytes[4..8].copy_from_slice(&new_riff_sz.to_le_bytes());

        let updated_meta = SampleMetadata {
            root_midi: 48,
            has_root: true,
            loop_mode: SampleLoopMode::PingPong,
            loop_start: 100,
            loop_end: 600,
            slices: vec![
                SampleSlice::new(64, Some("Attack".into())),
                SampleSlice::new(500, Some("Tail".into())),
            ],
            ..SampleMetadata::default()
        };
        let rewritten =
            update_wav_riff_metadata_bytes(&orig_bytes, 48_000, 1024, &updated_meta).unwrap();
        assert!(
            rewritten
                .windows(custom_payload.len())
                .any(|w| w == custom_payload),
            "Lossless RIFF metadata rewrite must preserve unknown chunks bit-for-bit"
        );
        let reloaded =
            decode_sample_bytes(&rewritten, "pad".into(), "pad.wav".into(), path.clone()).unwrap();
        assert_eq!(reloaded.meta.root_midi, 48);
        assert_eq!(reloaded.meta.loop_mode, SampleLoopMode::PingPong);
        assert_eq!(reloaded.meta.slices, updated_meta.slices);
        assert_eq!(reloaded.left.as_ref(), loaded.left.as_ref());

        // Mono one-shot roundtrip without loop or slices
        let mono_path = dir.path().join("hit.wav");
        let mono_meta = SampleMetadata {
            root_midi: 42,
            has_root: true,
            fine_cents: 0.0,
            low_midi: Some(42),
            high_midi: Some(42),
            low_vel: Some(0),
            high_vel: Some(127),
            loop_mode: SampleLoopMode::Off,
            loop_start: 0,
            loop_end: 0,
            slices: Vec::new(),
        };
        write_wav_with_metadata(&mono_path, &left, None, 48_000, &mono_meta).unwrap();
        let loaded_mono = load_sample_file(&mono_path, None).unwrap();
        assert_eq!(loaded_mono.channels, 1);
        assert_eq!(loaded_mono.meta.root_midi, 42);
        assert_eq!(loaded_mono.meta.low_midi, Some(42));
        assert_eq!(loaded_mono.meta.high_midi, Some(42));
        assert_eq!(loaded_mono.meta.loop_mode, SampleLoopMode::Off);
    }

    #[test]
    fn malformed_wavpack_short_block_size_returns_error_instead_of_panicking() {
        // 32-byte `wvpk` header with ck_size = 0 -> block_end (8) < end + 32 (32).
        let mut malformed = vec![0u8; 40];
        malformed[0..4].copy_from_slice(b"wvpk");
        malformed[4..8].copy_from_slice(&0u32.to_le_bytes());
        let err = decode_sample_bytes(
            &malformed,
            "bad".into(),
            "bad.wv".into(),
            PathBuf::from("bad.wv"),
        );
        assert!(err.is_err());
    }
}
