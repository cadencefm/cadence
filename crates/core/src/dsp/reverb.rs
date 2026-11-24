//! Stereo 8×8 Hadamard FDN + 4-stage Schroeder diffuser [`Reverb`] insert processor
//! and exact `-60 dB` RT60 envelope math.
//!
//! All diffuser and FDN delay lengths are quantized to distinct prime (pairwise
//! coprime) integer frame counts for the active sample rate, preserving flat
//! allpass magnitude and clean HF response across all sample rates.
//! Automated `size` changes smoothly crossfade between coprime delay sets with zero
//! clicks. All storage is allocated once in `Reverb::new`; zero RT allocations.

use crate::dsp::biquad::TptOnePoleLpf;
use crate::dsp::gain::stereo_ms_width_uncorrelated;
use crate::dsp::math::{
    flush_denormal_f32, hann_fade_in, log_to_norm, sanitize_clamp_f64, sanitize_frame,
    sanitize_sample_rate, Smoother, BYPASS_SETTLE_EPS, TAIL_SILENCE_FLOOR_F32,
};
use crate::mixer::ReverbPatch;

const LINES: usize = 8;
const BASE_FDN_48K: [f64; LINES] = [
    1493.0, 1601.0, 1747.0, 1867.0, 1999.0, 2137.0, 2281.0, 2417.0,
];
const BASE_DIFFUSER_48K: [f64; 4] = [149.0, 263.0, 163.0, 293.0];
const LEFT: [f32; LINES] = [1.0, 1.0, 1.0, 1.0, -1.0, -1.0, -1.0, -1.0];
const RIGHT: [f32; LINES] = [1.0, 1.0, -1.0, -1.0, 1.0, 1.0, -1.0, -1.0];
const NORM: f32 = std::f32::consts::FRAC_1_SQRT_2 * 0.5;
const ALLPASS_GAIN: f32 = 0.6;

/// Exact perceptual RT60 decay amplitude envelope (`10^(-3 * t / decay_seconds)`),
/// reaching `-60 dB` (`0.001`) at `t_sec == decay_seconds`.
#[inline]
pub fn reverb_rt60_envelope(decay_seconds: f32, t_sec: f32) -> f32 {
    if !t_sec.is_finite() || t_sec <= 0.0 {
        return 1.0;
    }
    let decay = sanitize_clamp_f64(f64::from(decay_seconds), 0.05, 60.0, 2.4);
    10.0_f64
        .powf(-3.0 * f64::from(t_sec) / decay)
        .clamp(0.0, 1.0) as f32
}

/// High-frequency damped RT60 envelope for UI reverb tail curves.
#[inline]
pub fn reverb_hf_rt60_envelope(decay_seconds: f32, damping_hz: f32, t_sec: f32) -> f32 {
    if !t_sec.is_finite() || t_sec <= 0.0 {
        return 1.0;
    }
    let damp_norm = f64::from(log_to_norm(damping_hz, 500.0, 18_000.0));
    let hf_mult = 1.0 + 3.0 * (1.0 - damp_norm);
    let decay = sanitize_clamp_f64(f64::from(decay_seconds), 0.05, 60.0, 2.4);
    10.0_f64
        .powf(-3.0 * hf_mult * f64::from(t_sec) / decay)
        .clamp(0.0, 1.0) as f32
}

#[inline]
fn is_prime(n: usize) -> bool {
    if n < 2 {
        return false;
    }
    if n == 2 || n == 3 {
        return true;
    }
    if n.is_multiple_of(2) || n.is_multiple_of(3) {
        return false;
    }
    let mut d = 5usize;
    while d * d <= n {
        if n.is_multiple_of(d) || n.is_multiple_of(d + 2) {
            return false;
        }
        d += 6;
    }
    true
}

/// Find the closest prime integer in `[min_val, max_val]` not already present in `used`.
/// Distinct primes are guaranteed pairwise coprime, maximizing FDN modal density.
fn nearest_distinct_prime(target: usize, used: &[usize], min_val: usize, max_val: usize) -> usize {
    let lo = min_val.max(2);
    let hi = max_val.max(lo);
    let center = target.clamp(lo, hi);
    let max_span = center.saturating_sub(lo).max(hi.saturating_sub(center));
    for offset in 0..=max_span {
        if let Some(up) = center.checked_add(offset) {
            if up <= hi && is_prime(up) && !used.contains(&up) {
                return up;
            }
        }
        if offset > 0 {
            if let Some(dn) = center.checked_sub(offset) {
                if dn >= lo && is_prime(dn) && !used.contains(&dn) {
                    return dn;
                }
            }
        }
    }
    center
}

fn coprime_scaled_lengths<const N: usize>(
    base_48k: [f64; N],
    scale: f64,
    sample_rate: f64,
    max_len: usize,
) -> [usize; N] {
    let mut out = [2usize; N];
    for (i, base) in base_48k.into_iter().enumerate() {
        let target = (base * scale * sample_rate / 48_000.0).round() as usize;
        out[i] = nearest_distinct_prime(target, &out[..i], 5, max_len);
    }
    out
}

struct MonoRing {
    data: Box<[f32]>,
    write: usize,
    valid: usize,
}

impl MonoRing {
    fn new(capacity: usize) -> Self {
        Self {
            data: vec![0.0; capacity.max(8) + 2].into_boxed_slice(),
            write: 0,
            valid: 0,
        }
    }

    fn capacity(&self) -> usize {
        self.data.len().saturating_sub(2)
    }

    fn reset(&mut self) {
        self.write = 0;
        self.valid = 0;
    }

    #[inline]
    fn read_integer(&self, distance: usize) -> f32 {
        let len = self.data.len();
        if distance == 0 || self.valid < distance || distance >= len {
            return 0.0;
        }
        let idx = if self.write >= distance {
            self.write - distance
        } else {
            self.write + len - distance
        };
        self.data[idx]
    }

    #[inline]
    fn read_linear(&self, distance: f64) -> f32 {
        let d = sanitize_clamp_f64(distance, 1.0, self.capacity().max(1) as f64, 1.0);
        let whole = d.floor() as usize;
        let frac = (d - whole as f64) as f32;
        let a = self.read_integer(whole);
        let b = self.read_integer(whole + 1);
        a + (b - a) * frac
    }

    #[inline]
    fn write(&mut self, value: f32) {
        let len = self.data.len();
        self.data[self.write] = flush_denormal_f32(value);
        self.write += 1;
        if self.write >= len {
            self.write = 0;
        }
        if self.valid < len - 1 {
            self.valid += 1;
        }
    }

    /// Exact integer-delay Schroeder allpass: `H(z) = (z^{-d} - g) / (1 - g z^{-d})`,
    /// guaranteeing exact unity magnitude `|H(e^{jω})| == 1.0` at every frequency!
    #[inline]
    fn allpass_integer(&mut self, input: f32, distance: usize) -> f32 {
        let delayed = self.read_integer(distance);
        let output = delayed - input * ALLPASS_GAIN;
        self.write(input + output * ALLPASS_GAIN);
        output
    }
}

/// Stereo 8×8 Hadamard FDN reverb with 4 integer-prime Schroeder diffusers,
/// coprime-integer FDN delay lengths, [`TptOnePoleLpf`] damping, smooth
/// predelay/mix/width/size transitions, and click-free bypass crossfade.
pub struct Reverb {
    sample_rate: f64,
    lines: [MonoRing; LINES],
    predelay: [MonoRing; 2],
    diffuser: [MonoRing; 4],
    diffuser_lengths: [usize; 4],
    damping_lpf: [TptOnePoleLpf; LINES],
    lengths_a: [usize; LINES],
    lengths_b: [usize; LINES],
    target_lengths: [usize; LINES],
    size_xfade: f64,
    size_xfade_step: f64,
    feedback: [Smoother; LINES],
    damping_hz: Smoother,
    predelay_frames: Smoother,
    mix: Smoother,
    width: Smoother,
    bypass_wet: Smoother,
    target: ReverbPatch,
    ctrl_count: u8,
    silent_bypass_frames: u32,
    has_processed: bool,
    was_active: bool,
}

impl Reverb {
    const SIZE_XFADE_SEC: f64 = 0.015;

    pub fn new(sample_rate: u32) -> Self {
        let sr = sanitize_sample_rate(f64::from(sample_rate.clamp(8_000, 384_000)));
        let line_cap = (sr * 0.115).ceil() as usize;
        let predelay_cap = (sr * 0.125).ceil() as usize;
        let diff_cap = (sr * 0.008).ceil() as usize;
        let diffuser_lengths = coprime_scaled_lengths(BASE_DIFFUSER_48K, 1.0, sr, diff_cap);
        let default = ReverbPatch::default();
        let initial_lengths = coprime_scaled_lengths(
            BASE_FDN_48K,
            0.65 + 1.35 * f64::from(default.size),
            sr,
            line_cap,
        );
        let xfade_frames = (sr * Self::SIZE_XFADE_SEC).max(16.0);
        let mut effect = Self {
            sample_rate: sr,
            lines: std::array::from_fn(|_| MonoRing::new(line_cap)),
            predelay: std::array::from_fn(|_| MonoRing::new(predelay_cap)),
            diffuser: std::array::from_fn(|_| MonoRing::new(diff_cap)),
            diffuser_lengths,
            damping_lpf: std::array::from_fn(|_| {
                TptOnePoleLpf::new(sr, f64::from(default.damping_hz))
            }),
            lengths_a: initial_lengths,
            lengths_b: initial_lengths,
            target_lengths: initial_lengths,
            size_xfade: 1.0,
            size_xfade_step: 1.0 / xfade_frames,
            feedback: std::array::from_fn(|_| Smoother::new(0.0, 5.0, sr)),
            damping_hz: Smoother::new(f64::from(default.damping_hz), 5.0, sr),
            predelay_frames: Smoother::new(0.0, 5.0, sr),
            mix: Smoother::new(f64::from(default.mix), 5.0, sr),
            width: Smoother::new(f64::from(default.width), 5.0, sr),
            bypass_wet: Smoother::new(1.0, 1.5, sr),
            target: default,
            ctrl_count: 0,
            silent_bypass_frames: 0,
            has_processed: false,
            was_active: false,
        };
        effect.set_parameters(default);
        effect
    }

    #[cfg(test)]
    pub(crate) fn is_idle(&self) -> bool {
        !self.was_active
    }

    /// Integer-prime Schroeder diffuser delay lengths in frames at the active sample rate.
    #[inline]
    pub fn diffuser_lengths(&self) -> [usize; 4] {
        self.diffuser_lengths
    }

    /// Active integer-prime FDN delay lengths in frames at the active sample rate.
    #[inline]
    pub fn fdn_lengths(&self) -> [usize; LINES] {
        self.target_lengths
    }

    pub fn reset(&mut self) {
        for ring in self
            .lines
            .iter_mut()
            .chain(self.predelay.iter_mut())
            .chain(self.diffuser.iter_mut())
        {
            ring.reset();
        }
        for lpf in &mut self.damping_lpf {
            lpf.reset();
        }
        self.lengths_a = self.target_lengths;
        self.lengths_b = self.target_lengths;
        self.size_xfade = 1.0;
        for fb in &mut self.feedback {
            fb.snap(fb.target());
        }
        self.damping_hz.snap(self.damping_hz.target());
        let damp = self.damping_hz.current();
        for lpf in &mut self.damping_lpf {
            lpf.set_cutoff(self.sample_rate, damp);
        }
        self.predelay_frames.snap(self.predelay_frames.target());
        self.mix.snap(self.mix.target());
        self.width.snap(self.width.target());
        self.bypass_wet.snap(1.0);
        self.ctrl_count = 0;
        self.silent_bypass_frames = 0;
        self.has_processed = false;
        self.was_active = false;
        self.set_parameters(self.target);
    }

    pub fn set_parameters(&mut self, patch: ReverbPatch) {
        let def = ReverbPatch::default();
        let size = sanitize_clamp_f64(f64::from(patch.size), 0.0, 1.0, f64::from(def.size));
        let decay = sanitize_clamp_f64(
            f64::from(patch.decay_seconds),
            0.1,
            30.0,
            f64::from(def.decay_seconds),
        );
        let damp_hz = sanitize_clamp_f64(
            f64::from(patch.damping_hz),
            200.0,
            self.sample_rate * 0.45,
            f64::from(def.damping_hz),
        );
        let predelay_ms = sanitize_clamp_f64(
            f64::from(patch.predelay_ms),
            0.0,
            120.0,
            f64::from(def.predelay_ms),
        );
        let mix = sanitize_clamp_f64(f64::from(patch.mix), 0.0, 1.0, f64::from(def.mix));
        let width = sanitize_clamp_f64(f64::from(patch.width), 0.0, 1.0, f64::from(def.width));

        let sanitized = ReverbPatch {
            size: size as f32,
            decay_seconds: decay as f32,
            damping_hz: damp_hz as f32,
            predelay_ms: predelay_ms as f32,
            mix: mix as f32,
            width: width as f32,
        };
        if self.has_processed && self.target == sanitized {
            return;
        }
        self.target = sanitized;

        let scale = 0.65 + 1.35 * size;
        let line_cap = self.lines[0].capacity();
        let new_lengths = coprime_scaled_lengths(BASE_FDN_48K, scale, self.sample_rate, line_cap);
        self.target_lengths = new_lengths;

        let mut target_fb = [0.0_f64; LINES];
        for (i, &len) in new_lengths.iter().enumerate() {
            let delay_sec = len as f64 / self.sample_rate;
            target_fb[i] = 10.0_f64.powf(-3.0 * delay_sec / decay).min(0.9995);
        }

        let predelay_frames =
            (predelay_ms * 0.001 * self.sample_rate).clamp(0.0, self.predelay[0].capacity() as f64);

        if !self.has_processed {
            self.lengths_a = new_lengths;
            self.lengths_b = new_lengths;
            self.size_xfade = 1.0;
            for (fb, val) in self.feedback.iter_mut().zip(target_fb) {
                fb.snap(val);
            }
            self.damping_hz.snap(damp_hz);
            for lpf in &mut self.damping_lpf {
                lpf.set_cutoff(self.sample_rate, damp_hz);
            }
            self.predelay_frames.snap(predelay_frames);
            self.mix.snap(mix);
            self.width.snap(width);
        } else {
            if self.size_xfade >= 1.0 && new_lengths != self.lengths_a {
                self.lengths_b = new_lengths;
                self.size_xfade = 0.0;
            }
            for (fb, val) in self.feedback.iter_mut().zip(target_fb) {
                fb.set_target(val);
            }
            self.damping_hz.set_target(damp_hz);
            self.predelay_frames.set_target(predelay_frames);
            self.mix.set_target(mix);
            self.width.set_target(width);
        }
    }

    pub fn process(&mut self, input: [f32; 2], bypassed: bool) -> [f32; 2] {
        self.has_processed = true;
        let dry = sanitize_frame(input);
        if bypassed && !self.was_active {
            self.bypass_wet.snap(0.0);
            self.lengths_a = self.target_lengths;
            self.lengths_b = self.target_lengths;
            self.size_xfade = 1.0;
            return dry;
        }

        self.bypass_wet.set_target(if bypassed { 0.0 } else { 1.0 });
        let bypass_w = self.bypass_wet.next();
        let bypass_settled = bypassed && bypass_w <= BYPASS_SETTLE_EPS;
        if !bypassed {
            self.was_active = true;
            self.silent_bypass_frames = 0;
        } else if bypass_settled {
            self.bypass_wet.snap(0.0);
        } else {
            self.silent_bypass_frames = 0;
        }

        let damp = self.damping_hz.next();
        if self.ctrl_count == 0 && !self.damping_hz.is_settled() {
            for lpf in &mut self.damping_lpf {
                lpf.set_cutoff(self.sample_rate, damp);
            }
        }
        self.ctrl_count = (self.ctrl_count + 1) & 15;

        let predelay_frames = self.predelay_frames.next();
        let mix = self.mix.next();
        let width = self.width.next() as f32;

        // Age reverb tails naturally when bypassed by feeding silence into the network.
        let feed = if bypassed { [0.0; 2] } else { dry };
        let mut diffuse = [0.0_f32; 2];
        for ch in 0..2 {
            let predelayed = if predelay_frames < 1.0 {
                feed[ch]
            } else {
                self.predelay[ch].read_linear(predelay_frames)
            };
            self.predelay[ch].write(feed[ch]);
            let d0 = self.diffuser_lengths[ch * 2];
            let d1 = self.diffuser_lengths[ch * 2 + 1];
            let v0 = self.diffuser[ch * 2].allpass_integer(predelayed, d0);
            diffuse[ch] = self.diffuser[ch * 2 + 1].allpass_integer(v0, d1);
        }

        let size_w = if self.size_xfade < 1.0 {
            self.size_xfade = (self.size_xfade + self.size_xfade_step).min(1.0);
            Some(hann_fade_in(self.size_xfade as f32))
        } else {
            None
        };

        let mut values = [0.0_f32; LINES];
        let mut wet = [0.0_f32; 2];
        let mut max_tail_abs = diffuse[0].abs().max(diffuse[1].abs());
        for (i, value) in values.iter_mut().enumerate() {
            let delayed = if let Some(w) = size_w {
                let a = self.lines[i].read_integer(self.lengths_a[i]);
                let b = self.lines[i].read_integer(self.lengths_b[i]);
                a + (b - a) * w
            } else {
                self.lines[i].read_integer(self.lengths_a[i])
            };
            max_tail_abs = max_tail_abs.max(delayed.abs());
            let damped = self.damping_lpf[i].process_mono(f64::from(delayed)) as f32;
            *value = damped;
            wet[0] += delayed * LEFT[i] * NORM;
            wet[1] += delayed * RIGHT[i] * NORM;
        }
        if size_w.is_some() && self.size_xfade >= 1.0 {
            self.lengths_a = self.lengths_b;
            if self.target_lengths != self.lengths_a {
                self.lengths_b = self.target_lengths;
                self.size_xfade = 0.0;
            }
        }

        // Fast Walsh-Hadamard 8×8 orthogonal mixing matrix.
        let mut stride = 1;
        while stride < LINES {
            for base in (0..LINES).step_by(stride * 2) {
                for offset in 0..stride {
                    let a = values[base + offset];
                    let b = values[base + offset + stride];
                    values[base + offset] = a + b;
                    values[base + offset + stride] = a - b;
                }
            }
            stride *= 2;
        }

        for (i, value) in values.into_iter().enumerate() {
            let fb = self.feedback[i].next() as f32;
            let inject = (diffuse[0] * LEFT[i] + diffuse[1] * RIGHT[i]) * NORM;
            let written = inject + value * NORM * fb;
            max_tail_abs = max_tail_abs.max(written.abs());
            self.lines[i].write(written);
        }

        if bypass_settled {
            if max_tail_abs < TAIL_SILENCE_FLOOR_F32
                && self.damping_lpf.iter().all(TptOnePoleLpf::quiet)
            {
                self.silent_bypass_frames = self.silent_bypass_frames.saturating_add(1);
                let max_predelay = predelay_frames.max(self.predelay_frames.target());
                let max_diff = (self.diffuser_lengths[0] + self.diffuser_lengths[1])
                    .max(self.diffuser_lengths[2] + self.diffuser_lengths[3]);
                let max_fdn = self
                    .lengths_a
                    .iter()
                    .chain(self.lengths_b.iter())
                    .chain(self.target_lengths.iter())
                    .copied()
                    .max()
                    .unwrap_or(0);
                let drain_frames = (max_predelay.ceil() as usize)
                    .saturating_add(max_diff)
                    .saturating_add(max_fdn)
                    .saturating_add(4) as u32;
                if self.silent_bypass_frames >= drain_frames {
                    // O(1) ring resets (zero memset on RT thread) guarantee zero stale tails on un-bypass.
                    for ring in self
                        .lines
                        .iter_mut()
                        .chain(self.predelay.iter_mut())
                        .chain(self.diffuser.iter_mut())
                    {
                        ring.reset();
                    }
                    for lpf in &mut self.damping_lpf {
                        lpf.reset();
                    }
                    self.lengths_a = self.target_lengths;
                    self.lengths_b = self.target_lengths;
                    self.size_xfade = 1.0;
                    self.silent_bypass_frames = 0;
                    self.was_active = false;
                }
            } else {
                self.silent_bypass_frames = 0;
            }
            return dry;
        }

        wet = stereo_ms_width_uncorrelated(wet, width);
        let eff_wet = (mix * bypass_w) as f32;
        let eff_dry = 1.0 - eff_wet;
        [
            dry[0] * eff_dry + wet[0] * eff_wet,
            dry[1] * eff_dry + wet[1] * eff_wet,
        ]
    }
}

impl crate::dsp::StereoEffect for Reverb {
    #[inline]
    fn process(&mut self, input: [f32; 2], bypassed: bool) -> [f32; 2] {
        Reverb::process(self, input, bypassed)
    }

    #[inline]
    fn reset(&mut self) {
        Reverb::reset(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    #[test]
    fn rt60_envelope_hits_exact_minus_60_db_at_decay_seconds() {
        for decay in [0.5_f32, 2.4, 8.0] {
            assert_eq!(reverb_rt60_envelope(decay, 0.0), 1.0);
            let at_rt60 = reverb_rt60_envelope(decay, decay);
            let db = crate::dsp::lin_to_db(at_rt60);
            assert!(
                (db + 60.0).abs() < 0.05,
                "decay={decay}: expected -60.0 dB at t=decay, got {db:.2} dB"
            );
        }
    }

    #[test]
    fn diffusers_are_distinct_primes_with_exact_unity_magnitude_at_44k_48k_96k() {
        for sr in [44_100u32, 48_000, 96_000] {
            let verb = Reverb::new(sr);
            let diffs = verb.diffuser_lengths();
            let fdn = verb.fdn_lengths();
            for &d in &diffs {
                assert!(is_prime(d), "sr={sr}: diffuser len {d} must be prime");
            }
            for &l in &fdn {
                assert!(is_prime(l), "sr={sr}: FDN len {l} must be prime");
            }
            // All FDN lengths pairwise distinct.
            for i in 0..LINES {
                for j in (i + 1)..LINES {
                    assert_ne!(fdn[i], fdn[j], "sr={sr}: duplicate FDN prime");
                }
            }

            // Direct Schroeder allpass magnitude verification near Nyquist (0.45 * sr) & midband.
            for &freq in &[400.0_f64, 4_000.0, f64::from(sr) * 0.45] {
                let mut ap = MonoRing::new(diffs[0] + 8);
                let warmup = sr as usize / 4;
                let measure = sr as usize / 4;
                let mut in_sq = 0.0;
                let mut out_sq = 0.0;
                for i in 0..(warmup + measure) {
                    let x = (TAU * freq * i as f64 / f64::from(sr)).sin() as f32 * 0.5;
                    let y = ap.allpass_integer(x, diffs[0]);
                    if i >= warmup {
                        in_sq += f64::from(x) * f64::from(x);
                        out_sq += f64::from(y) * f64::from(y);
                    }
                }
                let gain_db = crate::dsp::lin_to_db_f64((out_sq / in_sq).sqrt());
                assert!(
                    gain_db.abs() < 0.15,
                    "sr={sr} freq={freq}: allpass magnitude drift {gain_db:+.3} dB (expected 0.0 dB)"
                );
            }
        }
    }

    #[test]
    fn reverb_tail_decays_cleanly_across_sample_rates_with_zero_nans() {
        for sr in [44_100u32, 48_000, 96_000] {
            let mut short = Reverb::new(sr);
            short.set_parameters(ReverbPatch {
                decay_seconds: 0.4,
                damping_hz: 12_000.0,
                predelay_ms: 0.0,
                mix: 1.0,
                ..ReverbPatch::default()
            });
            let mut long = Reverb::new(sr);
            long.set_parameters(ReverbPatch {
                decay_seconds: 3.0,
                damping_hz: 12_000.0,
                predelay_ms: 0.0,
                mix: 1.0,
                ..ReverbPatch::default()
            });

            let total = sr as usize; // 1 second
            let mut e_short = 0.0_f64;
            let mut e_long = 0.0_f64;
            let mut stereo_diff = 0.0_f64;
            for i in 0..total {
                let x = if i == 0 { [0.8, 0.0] } else { [0.0, 0.0] };
                let ys = short.process(x, false);
                let yl = long.process(x, false);
                assert!(ys[0].is_finite() && ys[1].is_finite());
                assert!(yl[0].is_finite() && yl[1].is_finite());
                assert!(yl[0].abs() < 2.0 && yl[1].abs() < 2.0);
                if i >= total / 2 {
                    e_short += f64::from(ys[0]).powi(2) + f64::from(ys[1]).powi(2);
                    e_long += f64::from(yl[0]).powi(2) + f64::from(yl[1]).powi(2);
                    stereo_diff += f64::from(yl[0] - yl[1]).powi(2);
                }
            }
            assert!(
                e_long > e_short * 20.0,
                "sr={sr}: long={e_long}, short={e_short}"
            );
            assert!(
                stereo_diff > 1e-5,
                "sr={sr}: reverb must produce stereo width"
            );
        }
    }

    #[test]
    fn reverb_width_preserves_wet_energy_and_bypass_drains_to_idle() {
        for sr in [44_100u32, 48_000, 96_000, 192_000] {
            // 1. Uncorrelated wet output preserves RMS across width = 0.0 .. 1.0 without -6 dB loss.
            let measure_rms = |width: f32| -> f64 {
                let mut verb = Reverb::new(sr);
                verb.set_parameters(ReverbPatch {
                    size: 0.5,
                    decay_seconds: 1.0,
                    damping_hz: 12_000.0,
                    predelay_ms: 0.0,
                    mix: 1.0,
                    width,
                });
                let total = (f64::from(sr) * 0.25) as usize;
                let mut energy = 0.0_f64;
                for i in 0..total {
                    let x = if i == 0 { [1.0, 0.0] } else { [0.0, 0.0] };
                    let y = verb.process(x, false);
                    energy += f64::from(y[0]).powi(2) + f64::from(y[1]).powi(2);
                }
                (energy / (2.0 * total as f64)).sqrt()
            };
            let rms_wide = measure_rms(1.0);
            let rms_mono = measure_rms(0.0);
            let drift_db = crate::dsp::lin_to_db_f64(rms_mono / rms_wide);
            assert!(
                drift_db.abs() < 0.75,
                "sr={sr}: width 1.0->0.0 wet RMS drifted by {drift_db:+.2} dB (wide={rms_wide:.5}, mono={rms_mono:.5})"
            );

            // 2. Bypassed reverb drains its tail naturally to idle, then un-bypasses cleanly.
            let mut verb = Reverb::new(sr);
            verb.set_parameters(ReverbPatch {
                size: 0.2,
                decay_seconds: 0.12,
                damping_hz: 8_000.0,
                predelay_ms: 2.0,
                mix: 1.0,
                width: 1.0,
            });
            let _ = verb.process([1.0, -1.0], false);
            assert!(!verb.is_idle());
            let drain_limit = (f64::from(sr) * 1.2) as usize;
            for _ in 0..drain_limit {
                let out = verb.process([0.25, -0.25], true);
                assert!(out[0].is_finite() && out[1].is_finite());
                if verb.is_idle() {
                    break;
                }
            }
            assert!(
                verb.is_idle(),
                "sr={sr}: bypassed reverb must settle to idle after short tail drains"
            );
            // Once idle, un-bypassing with silence must produce exact zero (zero stale tail).
            for _ in 0..256 {
                assert_eq!(verb.process([0.0, 0.0], false), [0.0, 0.0]);
            }

            // 3. NaN parameters sanitize cleanly without poisoning FDN state.
            verb.set_parameters(ReverbPatch {
                size: f32::NAN,
                decay_seconds: f32::NAN,
                damping_hz: f32::NAN,
                predelay_ms: f32::NAN,
                mix: f32::NAN,
                width: f32::NAN,
            });
            let y = verb.process([0.5, -0.5], false);
            assert!(y[0].is_finite() && y[1].is_finite());
        }
    }
}
