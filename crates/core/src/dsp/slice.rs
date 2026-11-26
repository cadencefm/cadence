//! Pure, deterministic waveform analysis, live peak tracking, slicing, and offline sample
//! buffer DSP (`trim`, `normalize`, `reverse`, `fade-in`/`fade-out`).
use crate::dsp::gain::stereo_mid_f32;
use crate::dsp::math::{db_to_lin, hann_fade_in, hann_fade_out, lin_to_db, sanitize_sample};
use crate::sampler::{SampleSlice, MAX_SAMPLE_SLICES};
use anyhow::{ensure, Result};

/// Default peak normalization target (`-0.5 dBFS`, preserving inter-sample headroom).
pub const DEFAULT_NORMALIZE_TARGET_DBFS: f32 = -0.5;
pub const MIN_NORMALIZE_TARGET_DBFS: f32 = -24.0;
pub const MAX_NORMALIZE_TARGET_DBFS: f32 = 0.0;

/// Bounded, O(1)-per-frame hierarchical peak accumulator for live hardware input recording.
///
/// Streams arbitrary recording lengths into at most `max_bins` `(min, max)` waveform bins
/// by coalescing adjacent bin pairs and doubling `frames_per_bin` whenever capacity fills,
/// guaranteeing O(new_frames) CPU per UI poll and strictly bounded memory.
#[derive(Clone, Debug)]
pub struct LivePeakRecorder {
    max_bins: usize,
    frames_per_bin: usize,
    bins: Vec<(f32, f32)>,
    cur_min: f32,
    cur_max: f32,
    cur_count: usize,
    total_frames: u64,
}

impl LivePeakRecorder {
    pub fn new(max_bins: usize, initial_frames_per_bin: usize) -> Self {
        let cap = max_bins.max(8);
        Self {
            max_bins: cap,
            frames_per_bin: initial_frames_per_bin.max(1),
            bins: Vec::with_capacity(cap),
            cur_min: 0.0,
            cur_max: 0.0,
            cur_count: 0,
            total_frames: 0,
        }
    }

    pub fn clear(&mut self, initial_frames_per_bin: usize) {
        self.frames_per_bin = initial_frames_per_bin.max(1);
        self.bins.clear();
        self.cur_min = 0.0;
        self.cur_max = 0.0;
        self.cur_count = 0;
        self.total_frames = 0;
    }

    pub fn total_frames(&self) -> u64 {
        self.total_frames
    }

    pub fn frames_per_bin(&self) -> usize {
        self.frames_per_bin
    }

    /// Push newly captured stereo frames in O(frames.len()) time.
    pub fn push_stereo(&mut self, frames: &[[f32; 2]]) {
        for &frame in frames {
            let m = stereo_mid_f32([sanitize_sample(frame[0]), sanitize_sample(frame[1])]);
            if self.cur_count == 0 {
                self.cur_min = m.min(0.0);
                self.cur_max = m.max(0.0);
            } else {
                self.cur_min = self.cur_min.min(m);
                self.cur_max = self.cur_max.max(m);
            }
            self.cur_count += 1;
            self.total_frames = self.total_frames.saturating_add(1);
            if self.cur_count >= self.frames_per_bin {
                if self.bins.len() >= self.max_bins {
                    self.coalesce_half();
                }
                self.bins.push((self.cur_min, self.cur_max));
                self.cur_min = 0.0;
                self.cur_max = 0.0;
                self.cur_count = 0;
            }
        }
    }

    fn coalesce_half(&mut self) {
        let pairs = self.bins.len() / 2;
        for i in 0..pairs {
            let a = self.bins[i * 2];
            let b = self.bins[i * 2 + 1];
            self.bins[i] = (a.0.min(b.0), a.1.max(b.1));
        }
        if self.bins.len() % 2 == 1 {
            let tail = self.bins[self.bins.len() - 1];
            self.bins[pairs] = tail;
            self.bins.truncate(pairs + 1);
        } else {
            self.bins.truncate(pairs);
        }
        self.frames_per_bin = self.frames_per_bin.saturating_mul(2);
    }

    /// Snapshot current waveform peaks including any partial trailing bin so initial
    /// recording frames appear immediately.
    pub fn snapshot_peaks(&self) -> Vec<(f32, f32)> {
        let mut out = self.bins.clone();
        if self.cur_count > 0 && out.len() < self.max_bins {
            out.push((self.cur_min, self.cur_max));
        }
        out
    }
}

/// Extract the cropped frame range `[start_frame, end_frame)` into new mono/stereo buffers.
pub fn trim_sample_buffers(
    left: &[f32],
    right: Option<&[f32]>,
    start_frame: usize,
    end_frame: usize,
) -> Result<(Vec<f32>, Option<Vec<f32>>)> {
    let total = left.len();
    ensure!(total > 0, "Cannot trim an empty sample");
    if let Some(r) = right {
        ensure!(r.len() == total, "Stereo channel length mismatch");
    }
    let start = start_frame.min(total.saturating_sub(1));
    let end = end_frame.clamp(start + 1, total);
    ensure!(end > start, "Trim range is empty");
    let new_left = left[start..end].to_vec();
    let new_right = right.map(|r| r[start..end].to_vec());
    Ok((new_left, new_right))
}

/// Linked stereo/mono peak normalization to `target_dbfs` (`-24.0..=0.0 dBFS`).
/// Returns `(left, right, applied_gain_db)` and rejects digital silence.
pub fn normalize_sample_buffers(
    left: &[f32],
    right: Option<&[f32]>,
    target_dbfs: f32,
) -> Result<(Vec<f32>, Option<Vec<f32>>, f32)> {
    let total = left.len();
    ensure!(total > 0, "Cannot normalize an empty sample");
    if let Some(r) = right {
        ensure!(r.len() == total, "Stereo channel length mismatch");
    }
    ensure!(target_dbfs.is_finite(), "Normalize target must be finite");
    let target_db = target_dbfs.clamp(MIN_NORMALIZE_TARGET_DBFS, MAX_NORMALIZE_TARGET_DBFS);
    let mut peak = 0.0_f32;
    for &s in left {
        peak = peak.max(sanitize_sample(s).abs());
    }
    if let Some(r) = right {
        for &s in r {
            peak = peak.max(sanitize_sample(s).abs());
        }
    }
    ensure!(peak >= 1e-6, "Sample is silent — cannot normalize");
    let target_lin = db_to_lin(target_db);
    let gain = target_lin / peak;
    let gain_db = lin_to_db(gain);
    let new_left = left
        .iter()
        .map(|&s| (sanitize_sample(s) * gain).clamp(-1.0, 1.0))
        .collect();
    let new_right = right.map(|r| {
        r.iter()
            .map(|&s| (sanitize_sample(s) * gain).clamp(-1.0, 1.0))
            .collect()
    });
    Ok((new_left, new_right, gain_db))
}

/// Reverse mono or stereo sample buffers frame-for-frame.
pub fn reverse_sample_buffers(
    left: &[f32],
    right: Option<&[f32]>,
) -> Result<(Vec<f32>, Option<Vec<f32>>)> {
    let total = left.len();
    ensure!(total > 0, "Cannot reverse an empty sample");
    if let Some(r) = right {
        ensure!(r.len() == total, "Stereo channel length mismatch");
    }
    let mut new_left = left.to_vec();
    new_left.reverse();
    let new_right = right.map(|r| {
        let mut nr = r.to_vec();
        nr.reverse();
        nr
    });
    Ok((new_left, new_right))
}

/// Apply a smooth half-cosine (Hann) fade-in or fade-out inside `[start_frame, end_frame)`
/// over `fade_frames` frames, leaving samples outside the fade region untouched.
pub fn fade_sample_buffers(
    left: &[f32],
    right: Option<&[f32]>,
    start_frame: usize,
    end_frame: usize,
    fade_frames: usize,
    fade_in: bool,
) -> Result<(Vec<f32>, Option<Vec<f32>>)> {
    let total = left.len();
    ensure!(total > 0, "Cannot fade an empty sample");
    if let Some(r) = right {
        ensure!(r.len() == total, "Stereo channel length mismatch");
    }
    let start = start_frame.min(total.saturating_sub(1));
    let end = end_frame.clamp(start + 1, total);
    let span = end - start;
    let fade = fade_frames.clamp(1, span);
    let mut new_left = left.to_vec();
    let mut new_right = right.map(<[f32]>::to_vec);
    let denom = (fade.saturating_sub(1)).max(1) as f32;

    if fade_in {
        for i in 0..fade {
            let g = if fade == 1 {
                0.0
            } else {
                hann_fade_in(i as f32 / denom)
            };
            let idx = start + i;
            new_left[idx] = sanitize_sample(new_left[idx]) * g;
            if let Some(r) = new_right.as_mut() {
                r[idx] = sanitize_sample(r[idx]) * g;
            }
        }
    } else {
        let fade_start = end - fade;
        for i in 0..fade {
            let g = if fade == 1 {
                0.0
            } else {
                hann_fade_out(i as f32 / denom)
            };
            let idx = fade_start + i;
            new_left[idx] = sanitize_sample(new_left[idx]) * g;
            if let Some(r) = new_right.as_mut() {
                r[idx] = sanitize_sample(r[idx]) * g;
            }
        }
    }
    Ok((new_left, new_right))
}

/// Compute `(min, max)` mono-summed peak bins over the frame range `[start_frame, end_frame)`
/// for crisp zoomed waveform rendering at any zoom level.
pub fn compute_waveform_peaks_window(
    left: &[f32],
    right: Option<&[f32]>,
    start_frame: usize,
    end_frame: usize,
    max_bins: usize,
) -> Vec<(f32, f32)> {
    let total = left.len();
    if total == 0 || max_bins == 0 {
        return Vec::new();
    }
    let start = start_frame.min(total.saturating_sub(1));
    let end = end_frame.clamp(start + 1, total);
    let span = end - start;
    let bins = max_bins.min(span).max(1);
    let mut peaks = Vec::with_capacity(bins);
    for b in 0..bins {
        let s = start + b * span / bins;
        let e = (start + (b + 1) * span / bins).max(s + 1).min(end);
        let mut lo = 0.0_f32;
        let mut hi = 0.0_f32;
        for (i, &l) in left.iter().enumerate().take(e).skip(s) {
            let mono = match right {
                Some(r) => stereo_mid_f32([l, r.get(i).copied().unwrap_or(l)]),
                None => l,
            };
            lo = lo.min(mono);
            hi = hi.max(mono);
        }
        peaks.push((lo, hi));
    }
    peaks
}

#[inline]
fn mono_sample(left: &[f32], right: Option<&[f32]>, index: usize) -> f32 {
    let l = left.get(index).copied().unwrap_or(0.0);
    match right {
        Some(r) => stereo_mid_f32([l, r.get(index).copied().unwrap_or(l)]),
        None => l,
    }
}

/// Snap `frame` to the nearest zero-crossing within `±search_radius` frames inside
/// `[min_frame, max_frame]`, minimizing click artifacts at crop/loop/slice boundaries.
pub fn snap_to_zero_crossing(
    left: &[f32],
    right: Option<&[f32]>,
    frame: usize,
    min_frame: usize,
    max_frame: usize,
    search_radius: usize,
) -> usize {
    let total = left.len();
    if total < 2 {
        return frame.min(total);
    }
    let lo_bound = min_frame.min(total - 1);
    let hi_bound = max_frame.min(total - 1).max(lo_bound);
    let target = frame.clamp(lo_bound, hi_bound);
    if search_radius == 0 || hi_bound <= lo_bound {
        return target;
    }
    let search_lo = target.saturating_sub(search_radius).max(lo_bound);
    let search_hi = target.saturating_add(search_radius).min(hi_bound);

    let mut best: Option<(usize, usize, f32)> = None; // (dist, idx, abs_val)
    let mut prev = mono_sample(left, right, search_lo);
    for idx in (search_lo + 1)..=search_hi {
        let cur = mono_sample(left, right, idx);
        let crossed = (prev <= 0.0 && cur >= 0.0) || (prev >= 0.0 && cur <= 0.0);
        if crossed {
            let candidate = if prev.abs() <= cur.abs() {
                idx - 1
            } else {
                idx
            };
            let val = mono_sample(left, right, candidate).abs();
            let dist = candidate.abs_diff(target);
            let better = match best {
                None => true,
                Some((best_dist, _, best_val)) => {
                    dist < best_dist || (dist == best_dist && val < best_val)
                }
            };
            if better {
                best = Some((dist, candidate, val));
            }
        }
        prev = cur;
    }
    best.map_or(target, |(_, idx, _)| idx)
}

/// Default zero-crossing search radius (~4 ms, capped to `256` frames).
#[inline]
pub fn default_zero_snap_radius(sample_rate: u32) -> usize {
    ((sample_rate.max(8_000) as usize) / 250).clamp(32, 256)
}

/// Detect musical onset/transient slice markers inside `[start_frame, end_frame)`.
///
/// Uses a dual-band (high-pass + broadband) log-energy flux novelty curve with an adaptive
/// moving threshold, `min_gap_ms` refractory debouncing, attack-foot backtracking, and
/// zero-crossing snapping.
///
/// - `sensitivity`: `0.0..=1.0` (`0.0` = strictest / loudest peaks only, `1.0` = most sensitive).
/// - `min_gap_ms`: minimum spacing between consecutive slices in milliseconds (`15.0..=500.0`).
/// - `max_slices`: hard cap on returned slice count (clamped to `1..=MAX_SAMPLE_SLICES`).
#[allow(clippy::too_many_arguments)]
pub fn detect_transient_slices(
    left: &[f32],
    right: Option<&[f32]>,
    sample_rate: u32,
    start_frame: usize,
    end_frame: usize,
    sensitivity: f32,
    min_gap_ms: f32,
    max_slices: usize,
) -> Vec<usize> {
    let total = left.len();
    if total == 0 || max_slices == 0 {
        return Vec::new();
    }
    let start = start_frame.min(total.saturating_sub(1));
    let end = end_frame.clamp(start + 1, total);
    let span = end - start;
    let sr = sample_rate.clamp(8_000, 384_000) as f32;
    let sens = sensitivity.clamp(0.0, 1.0);
    let min_gap_frames = ((min_gap_ms.clamp(15.0, 500.0) * 0.001) * sr).round() as usize;
    let cap = max_slices.min(MAX_SAMPLE_SLICES);

    // Analysis hop (~2.5 ms) and window (2 * hop).
    let hop = ((sr / 400.0).round() as usize).clamp(32, 256);
    if span < hop * 2 {
        return vec![start];
    }
    let win = hop * 2;
    let num_hops = (span.saturating_sub(win)) / hop + 1;
    if num_hops < 3 {
        return vec![start];
    }

    // Compute broadband & first-difference (HF-emphasized) RMS energy per analysis hop.
    let mut bb_log = Vec::with_capacity(num_hops);
    let mut hf_log = Vec::with_capacity(num_hops);
    let mut hop_peak = Vec::with_capacity(num_hops);
    let mut global_peak = 0.0_f32;
    const LOG_COMP: f32 = 40.0;

    for h in 0..num_hops {
        let base = start + h * hop;
        let stop = (base + win).min(end);
        let mut bb_sum = 0.0_f32;
        let mut hf_sum = 0.0_f32;
        let mut peak = 0.0_f32;
        let mut prev = mono_sample(left, right, base);
        for i in base..stop {
            let x = mono_sample(left, right, i);
            let ax = x.abs();
            if ax > peak {
                peak = ax;
            }
            bb_sum += x * x;
            let diff = x - prev;
            hf_sum += diff * diff;
            prev = x;
        }
        let n = (stop - base).max(1) as f32;
        let bb_rms = (bb_sum / n).sqrt();
        let hf_rms = (hf_sum / n).sqrt();
        bb_log.push((1.0 + LOG_COMP * bb_rms).ln());
        hf_log.push((1.0 + LOG_COMP * hf_rms).ln());
        hop_peak.push(peak);
        if peak > global_peak {
            global_peak = peak;
        }
    }

    if global_peak < 1e-4 {
        return vec![start];
    }

    // Half-wave rectified combined log-energy flux novelty curve.
    let mut flux = vec![0.0_f32; num_hops];
    for h in 1..num_hops {
        let df_hf = (hf_log[h] - hf_log[h - 1]).max(0.0);
        let df_bb = (bb_log[h] - bb_log[h - 1]).max(0.0);
        flux[h] = df_hf * 0.70 + df_bb * 0.55;
    }
    // Check if audio starts abruptly right at `start` (hop 0 already loud).
    let noise_floor = (global_peak * (0.08 - sens * 0.065)).max(0.004);
    let start_is_attack = hop_peak[0] >= noise_floor * 1.5;

    // Adaptive local threshold over ±75 ms neighborhood.
    let half_ctx = ((0.075 * sr) / hop as f32).round().max(3.0) as usize;
    let thresh_mult = 2.6 - sens * 1.65; // sens=0.0 -> 2.6x, sens=0.55 -> 1.69x, sens=1.0 -> 0.95x
    let min_flux = (0.36 - sens * 0.30).max(0.035);
    let min_gap_hops = (min_gap_frames / hop).max(1);
    let snap_radius = default_zero_snap_radius(sample_rate);

    let mut raw_peaks: Vec<(usize, f32)> = Vec::new();
    let mut last_picked_hop: Option<usize> = None;

    if start_is_attack {
        raw_peaks.push((0, flux.get(1).copied().unwrap_or(1.0).max(1.0)));
        last_picked_hop = Some(0);
    }

    for h in 1..(num_hops - 1) {
        let f = flux[h];
        if f < min_flux {
            continue;
        }
        // Strict local maximum (allowing flat plateau start).
        if f < flux[h - 1] || f <= flux[h + 1] {
            continue;
        }
        // Verify real audible energy at or immediately after this onset.
        let local_peak = hop_peak[h].max(hop_peak[(h + 1).min(num_hops - 1)]);
        if local_peak < noise_floor {
            continue;
        }
        // Local adaptive mean threshold.
        let ctx_lo = h.saturating_sub(half_ctx);
        let ctx_hi = (h + half_ctx + 1).min(num_hops);
        let ctx_mean = flux[ctx_lo..ctx_hi].iter().sum::<f32>() / (ctx_hi - ctx_lo).max(1) as f32;
        if f < ctx_mean * thresh_mult + min_flux * 0.45 {
            continue;
        }
        if let Some(prev_h) = last_picked_hop {
            if h.saturating_sub(prev_h) < min_gap_hops {
                // Keep the stronger onset if two triggers collide within min_gap.
                if let Some(last) = raw_peaks.last_mut() {
                    if prev_h > 0 && f > last.1 * 1.25 {
                        *last = (h, f);
                        last_picked_hop = Some(h);
                    }
                }
                continue;
            }
        }
        raw_peaks.push((h, f));
        last_picked_hop = Some(h);
    }

    // Backtrack each peak to its attack foot and refine to exact sample + zero-crossing.
    let mut candidates: Vec<(usize, f32)> = Vec::with_capacity(raw_peaks.len());
    for (h, strength) in raw_peaks {
        if h == 0 {
            candidates.push((start, strength));
            continue;
        }
        let mut foot = h;
        let peak_amp = hop_peak[h].max(hop_peak[(h + 1).min(num_hops - 1)]);
        let foot_thresh = (peak_amp * 0.12).max(noise_floor * 0.5);
        while foot > 0 && foot + 3 >= h && hop_peak[foot - 1] >= foot_thresh {
            if flux[foot - 1] <= flux[foot] * 0.15 {
                break;
            }
            foot -= 1;
        }
        let approx = (start + foot * hop).min(end.saturating_sub(1));
        let snapped = snap_to_zero_crossing(
            left,
            right,
            approx,
            start,
            end.saturating_sub(1),
            snap_radius.min(hop),
        );
        candidates.push((snapped, strength));
    }

    // If more onsets than `cap`, keep the strongest `cap` peaks (preserving chronological order).
    if candidates.len() > cap {
        candidates.sort_by(|a, b| b.1.total_cmp(&a.1));
        candidates.truncate(cap);
    }
    candidates.sort_by_key(|c| c.0);
    let mut frames: Vec<usize> = Vec::with_capacity(candidates.len());
    for (frame, _) in candidates {
        if frames.last().is_none_or(|&prev| frame > prev) {
            frames.push(frame);
        }
    }
    if frames.is_empty() {
        frames.push(start);
    }
    frames
}

/// Divide `[start_frame, end_frame)` into `divisions` (`2..=MAX_SAMPLE_SLICES`) equal grid slices,
/// optionally snapping interior slice points to the nearest zero-crossing.
pub fn detect_grid_slices(
    start_frame: usize,
    end_frame: usize,
    divisions: usize,
    left: &[f32],
    right: Option<&[f32]>,
    sample_rate: u32,
    snap_zero: bool,
) -> Vec<usize> {
    let total = left.len();
    if total == 0 {
        return Vec::new();
    }
    let start = start_frame.min(total.saturating_sub(1));
    let end = end_frame.clamp(start + 1, total);
    let span = end - start;
    let count = divisions.clamp(1, MAX_SAMPLE_SLICES).min(span);
    if count <= 1 {
        return vec![start];
    }
    let radius = default_zero_snap_radius(sample_rate).min(span / (count * 4).max(1));
    let mut out = Vec::with_capacity(count);
    out.push(start);
    for i in 1..count {
        let raw = start + i * span / count;
        let frame = if snap_zero && radius > 0 {
            snap_to_zero_crossing(left, right, raw, start + 1, end.saturating_sub(1), radius)
        } else {
            raw
        };
        if out.last().is_none_or(|&prev| frame > prev && frame < end) {
            out.push(frame);
        }
    }
    out
}

/// Convert sorted slice markers inside `[start_frame, end_frame)` into contiguous playback
/// regions `(region_start, region_end, optional_label)`.
///
/// When `slices` is empty (or contains no markers within `[start_frame, end_frame)`), returns
/// the single full playback region `[(start_frame, end_frame, None)]`.
pub fn slice_regions(
    slices: &[SampleSlice],
    start_frame: usize,
    end_frame: usize,
) -> Vec<(usize, usize, Option<&str>)> {
    if end_frame <= start_frame {
        return Vec::new();
    }
    let active: Vec<&SampleSlice> = slices
        .iter()
        .filter(|s| s.frame >= start_frame && s.frame < end_frame)
        .collect();
    if active.is_empty() {
        return vec![(start_frame, end_frame, None)];
    }
    let mut regions = Vec::with_capacity(active.len());
    for (i, sl) in active.iter().enumerate() {
        let next_end = active
            .get(i + 1)
            .map_or(end_frame, |next| next.frame)
            .min(end_frame);
        if next_end > sl.frame {
            regions.push((sl.frame, next_end, sl.label.as_deref()));
        }
    }
    if regions.is_empty() {
        regions.push((start_frame, end_frame, None));
    }
    regions
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_drum_loop(sr: u32, hit_frames: &[usize], total_frames: usize) -> Vec<f32> {
        let mut buf = vec![0.0_f32; total_frames];
        for &hit in hit_frames {
            let len = (sr as usize / 10).min(total_frames.saturating_sub(hit));
            for i in 0..len {
                let t = i as f32 / sr as f32;
                let env = (-t * 45.0).exp();
                let click = (2.0 * std::f32::consts::PI * 1_600.0 * t).sin() * (-t * 220.0).exp();
                let body = (2.0 * std::f32::consts::PI * 110.0 * t).sin() * env;
                buf[hit + i] += (click * 0.7 + body * 0.6).clamp(-1.0, 1.0);
            }
        }
        buf
    }

    #[test]
    fn transient_detector_finds_four_percussive_hits_with_zero_crossing_snap() {
        let sr = 48_000;
        let hits = [4_800, 16_800, 28_800, 40_800];
        let audio = synthetic_drum_loop(sr, &hits, 48_000);

        let detected = detect_transient_slices(&audio, None, sr, 0, audio.len(), 0.55, 50.0, 32);
        assert_eq!(
            detected.len(),
            4,
            "expected 4 detected onsets, got {detected:?}"
        );
        for (&expected, &actual) in hits.iter().zip(&detected) {
            assert!(
                actual.abs_diff(expected) <= 400,
                "onset {actual} drifted too far from true hit {expected}"
            );
        }
    }

    #[test]
    fn grid_slicer_and_slice_regions_partition_window_cleanly() {
        let sr = 48_000;
        let audio: Vec<f32> = (0..4_800)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / sr as f32).sin())
            .collect();
        let grid = detect_grid_slices(0, 4_800, 4, &audio, None, sr, true);
        assert_eq!(grid.len(), 4);
        assert_eq!(grid[0], 0);
        // Interior grid boundaries snapped near zero crossings (~1200, ~2400, ~3600).
        for &f in &grid[1..] {
            assert!(
                audio[f].abs() < 0.08,
                "grid slice {f} not near zero: {}",
                audio[f]
            );
        }

        let slices: Vec<SampleSlice> = grid
            .iter()
            .enumerate()
            .map(|(i, &frame)| SampleSlice {
                frame,
                label: Some(format!("Hit {}", i + 1)),
            })
            .collect();
        let regions = slice_regions(&slices, 0, 4_800);
        assert_eq!(regions.len(), 4);
        assert_eq!(regions[0].0, 0);
        assert_eq!(regions[3].1, 4_800);
        assert_eq!(regions[0].2, Some("Hit 1"));
    }

    #[test]
    fn zoomed_peak_window_resolves_local_transient() {
        let mut audio = vec![0.0_f32; 2_000];
        audio[1_500] = 0.9;
        audio[1_501] = -0.8;
        let zoomed = compute_waveform_peaks_window(&audio, None, 1_400, 1_600, 100);
        assert_eq!(zoomed.len(), 100);
        assert!(zoomed.iter().any(|&(lo, hi)| hi >= 0.89 && lo <= -0.79));
    }

    #[test]
    fn live_peak_recorder_stays_bounded_and_preserves_extremes_across_coalescing() {
        let mut rec = LivePeakRecorder::new(64, 16);
        let mut chunk = vec![[0.1_f32, 0.1_f32]; 10_000];
        chunk[4_321] = [0.95, 0.95];
        chunk[7_777] = [-0.88, -0.88];
        rec.push_stereo(&chunk[..3_000]);
        rec.push_stereo(&chunk[3_000..]);
        assert_eq!(rec.total_frames(), 10_000);
        let peaks = rec.snapshot_peaks();
        assert!(peaks.len() <= 64, "peaks must stay bounded by max_bins");
        assert!(peaks.len() >= 16);
        assert!(peaks.iter().any(|&(_, hi)| (hi - 0.95).abs() < 1e-4));
        assert!(peaks.iter().any(|&(lo, _)| (lo + 0.88).abs() < 1e-4));
    }

    #[test]
    fn offline_sample_ops_trim_normalize_reverse_and_fade_accurately() {
        let left = vec![0.1, 0.25, -0.5, 0.25, 0.1, 0.05];
        let right = vec![-0.1, 0.1, 0.2, -0.25, 0.05, -0.02];

        // 1. Trim extracts exact slice.
        let (tl, tr) = trim_sample_buffers(&left, Some(&right), 1, 5).unwrap();
        assert_eq!(tl, vec![0.25, -0.5, 0.25, 0.1]);
        assert_eq!(tr.unwrap(), vec![0.1, 0.2, -0.25, 0.05]);

        // 2. Normalize scales stereo-linked peak to exact target dBFS and rejects silence.
        let (nl, nr, gain_db) =
            normalize_sample_buffers(&left, Some(&right), DEFAULT_NORMALIZE_TARGET_DBFS).unwrap();
        let expected_peak = db_to_lin(DEFAULT_NORMALIZE_TARGET_DBFS);
        let actual_peak = nl
            .iter()
            .chain(nr.as_ref().unwrap().iter())
            .fold(0.0_f32, |acc, &s| acc.max(s.abs()));
        assert!((actual_peak - expected_peak).abs() < 1e-4);
        assert!(gain_db > 5.0);
        assert!(normalize_sample_buffers(&[0.0; 16], None, -0.5).is_err());

        // 3. Reverse round-trips cleanly.
        let (rl, rr) = reverse_sample_buffers(&left, Some(&right)).unwrap();
        assert_eq!(rl[0], left[5]);
        assert_eq!(rl[5], left[0]);
        let (rl2, rr2) = reverse_sample_buffers(&rl, rr.as_deref()).unwrap();
        assert_eq!(rl2, left);
        assert_eq!(rr2.unwrap(), right);

        // 4. Hann fade-in / fade-out hit exact 0.0 and 1.0 endpoints without altering exterior frames.
        let ones = vec![1.0_f32; 12];
        let (fi, _) = fade_sample_buffers(&ones, None, 2, 10, 4, true).unwrap();
        assert_eq!(fi[0], 1.0);
        assert_eq!(fi[1], 1.0);
        assert!(fi[2].abs() < 1e-6);
        assert!((fi[5] - 1.0).abs() < 1e-6);
        assert_eq!(fi[6], 1.0);

        let (fo, _) = fade_sample_buffers(&ones, None, 2, 10, 4, false).unwrap();
        assert_eq!(fo[5], 1.0);
        assert!((fo[6] - 1.0).abs() < 1e-6);
        assert!(fo[9].abs() < 1e-6);
        assert_eq!(fo[10], 1.0);
    }
}
