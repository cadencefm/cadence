//! Zero-allocation stereo [`DelayLine`] (integer, linear, and cubic Hermite reads)
//! and Cadence's multi-mode [`Delay`] insert processor (`PingPong` & `Stereo`).
//!
//! Supports lazy `new_unallocated` / `allocate` (0 heap bytes for unused slots),
//! mono-seeded L→R→L ping-pong bounce, independent stereo dual-delay mode with
//! smooth click-free feedback-matrix blending, crossfaded time changes without
//! pitch chirp, `.max(20.0)` BPM guard, TPT feedback damping, and click-free
//! bypass crossfade.

use crate::dsp::biquad::TptOnePoleLpf;
use crate::dsp::math::{
    flush_denormal_f32, hann_fade_in, hermite4_f32, sanitize_frame, sanitize_sample_rate, Smoother,
};
use crate::mixer::{DelayMode, DelayPatch, MAX_DELAY_SECONDS};

/// Heap-allocated stereo circular delay buffer with O(1) validity reset and
/// integer / linear / cubic Hermite interpolation reads. Zero RT allocations.
pub struct DelayLine {
    data: Box<[[f32; 2]]>,
    write: usize,
    valid: usize,
}

impl DelayLine {
    /// Allocate a stereo delay buffer supporting up to `capacity_frames` of delay.
    pub fn new(capacity_frames: usize) -> Self {
        // Extra 4 frames of headroom ensure cubic Hermite taps never alias write head.
        let len = capacity_frames.max(4).saturating_add(4);
        Self {
            data: vec![[0.0; 2]; len].into_boxed_slice(),
            write: 0,
            valid: 0,
        }
    }

    /// Create an unallocated placeholder (0 bytes heap usage until [`allocate`] is called).
    pub fn unallocated() -> Self {
        Self {
            data: Box::new([]),
            write: 0,
            valid: 0,
        }
    }

    #[inline]
    pub fn is_allocated(&self) -> bool {
        !self.data.is_empty()
    }

    /// Maximum usable delay length in frames (`0` when unallocated).
    #[inline]
    pub fn capacity(&self) -> usize {
        self.data.len().saturating_sub(4)
    }

    /// Allocate (or reallocate) off the real-time audio thread.
    pub fn allocate(&mut self, capacity_frames: usize) {
        let len = capacity_frames.max(4).saturating_add(4);
        if self.data.len() != len {
            self.data = vec![[0.0; 2]; len].into_boxed_slice();
        }
        self.write = 0;
        self.valid = 0;
    }

    /// O(1) reset: marks historical frames unreadable without zeroing megabytes on the RT thread.
    #[inline]
    pub fn reset(&mut self) {
        self.write = 0;
        self.valid = 0;
    }

    /// Write one stereo frame, sanitizing NaNs and flushing subnormals.
    #[inline]
    pub fn write(&mut self, frame: [f32; 2]) {
        let len = self.data.len();
        if len == 0 {
            return;
        }
        self.data[self.write] = [flush_denormal_f32(frame[0]), flush_denormal_f32(frame[1])];
        self.write += 1;
        if self.write >= len {
            self.write = 0;
        }
        if self.valid < len - 1 {
            self.valid += 1;
        }
    }

    /// Exact integer delay read (`distance >= 1` frames behind the write cursor).
    #[inline]
    pub fn read_integer(&self, distance: usize) -> [f32; 2] {
        let len = self.data.len();
        if len == 0 || distance == 0 || self.valid < distance || distance >= len {
            return [0.0; 2];
        }
        let idx = if self.write >= distance {
            self.write - distance
        } else {
            self.write + len - distance
        };
        self.data[idx]
    }

    /// Linearly interpolated fractional delay read.
    #[inline]
    pub fn read_linear(&self, delay_samples: f64) -> [f32; 2] {
        let max_d = self.capacity();
        if max_d == 0 {
            return [0.0; 2];
        }
        let d = delay_samples.clamp(1.0, max_d as f64);
        let whole = d.floor() as usize;
        let frac = (d - whole as f64) as f32;
        let a = self.read_integer(whole);
        let b = self.read_integer(whole + 1);
        [a[0] + (b[0] - a[0]) * frac, a[1] + (b[1] - a[1]) * frac]
    }

    /// 4-point Catmull-Rom cubic Hermite interpolated fractional delay read
    /// (preserves high-frequency clarity during modulated chorus/delay reads).
    #[inline]
    pub fn read_cubic(&self, delay_samples: f64) -> [f32; 2] {
        let max_d = self.capacity();
        if max_d == 0 {
            return [0.0; 2];
        }
        let d = delay_samples.clamp(1.0, max_d as f64);
        let whole = d.floor() as usize;
        let t = (d - whole as f64) as f32;
        if t.abs() < 1e-6 {
            return self.read_integer(whole);
        }
        if whole < 2 || whole + 2 >= self.data.len() {
            return self.read_linear(d);
        }
        let y0 = self.read_integer(whole - 1);
        let y1 = self.read_integer(whole);
        let y2 = self.read_integer(whole + 1);
        let y3 = self.read_integer(whole + 2);
        [
            hermite4_f32(y0[0], y1[0], y2[0], y3[0], t),
            hermite4_f32(y0[1], y1[1], y2[1], y3[1], t),
        ]
    }
}

/// Multi-mode stereo delay insert processor supporting [`DelayMode::PingPong`]
/// (mono-summed Left injection bouncing L → R → L) and [`DelayMode::Stereo`]
/// (independent stereo dual-delay preserving input & Return panning symmetrically),
/// with smooth click-free mode blending, crossfaded delay-time transitions
/// (zero Doppler chirp), TPT feedback damping, and click-free bypass crossfade.
pub struct Delay {
    line: DelayLine,
    sample_rate: f64,
    time_a: f64,
    time_b: f64,
    target_time: f64,
    xfade: f64,
    xfade_step: f64,
    feedback: Smoother,
    mix: Smoother,
    ping_pong_blend: Smoother,
    bypass_wet: Smoother,
    damp_lpf: TptOnePoleLpf,
    has_processed: bool,
    was_active: bool,
}

impl Delay {
    const DAMP_HZ: f64 = 8_500.0;
    const XFADE_SEC: f64 = 0.030;

    /// Construct an eagerly allocated delay buffer supporting [`MAX_DELAY_SECONDS`] (12 s).
    pub fn new(sample_rate: u32) -> Self {
        Self::with_max_seconds(sample_rate, MAX_DELAY_SECONDS)
    }

    /// Construct a delay buffer with an explicit maximum duration in seconds.
    pub fn with_max_seconds(sample_rate: u32, max_seconds: f64) -> Self {
        let sr = sanitize_sample_rate(f64::from(sample_rate));
        let cap = (sr * max_seconds.clamp(0.1, MAX_DELAY_SECONDS)).ceil() as usize;
        Self::from_line(sr, DelayLine::new(cap))
    }

    /// Construct an unallocated delay slot (`0` heap bytes until [`allocate`] is called off-RT),
    /// keeping unused `MixerEngine` slots memory-free.
    pub fn new_unallocated(sample_rate: u32) -> Self {
        let sr = sanitize_sample_rate(f64::from(sample_rate));
        Self::from_line(sr, DelayLine::unallocated())
    }

    fn from_line(sr: f64, line: DelayLine) -> Self {
        let xfade_frames = (sr * Self::XFADE_SEC).max(16.0);
        Self {
            line,
            sample_rate: sr,
            time_a: 1.0,
            time_b: 1.0,
            target_time: 1.0,
            xfade: 1.0,
            xfade_step: 1.0 / xfade_frames,
            feedback: Smoother::new(0.0, 5.0, sr),
            mix: Smoother::new(1.0, 5.0, sr),
            ping_pong_blend: Smoother::new(1.0, 5.0, sr),
            bypass_wet: Smoother::new(1.0, 1.5, sr),
            damp_lpf: TptOnePoleLpf::new(sr, Self::DAMP_HZ),
            has_processed: false,
            was_active: false,
        }
    }

    /// Allocate (or resize) the delay buffer off the real-time audio thread.
    pub fn allocate(&mut self, max_seconds: f64) {
        let cap = (self.sample_rate * max_seconds.clamp(0.1, MAX_DELAY_SECONDS)).ceil() as usize;
        self.line.allocate(cap);
        self.has_processed = false;
    }

    #[inline]
    pub fn is_allocated(&self) -> bool {
        self.line.is_allocated()
    }

    pub fn reset(&mut self) {
        self.line.reset();
        self.damp_lpf.reset();
        self.time_a = self.target_time;
        self.time_b = self.target_time;
        self.xfade = 1.0;
        self.feedback.snap(self.feedback.target());
        self.mix.snap(self.mix.target());
        self.ping_pong_blend.snap(self.ping_pong_blend.target());
        self.bypass_wet.snap(1.0);
        self.has_processed = false;
        self.was_active = false;
    }

    pub fn set_parameters(&mut self, patch: DelayPatch, bpm: f64) {
        let safe_bpm = if bpm.is_finite() {
            bpm.max(20.0)
        } else {
            120.0
        };
        let max_d = self.line.capacity().max(1) as f64;
        let steps = if patch.steps.is_finite() {
            f64::from(patch.steps.clamp(0.0625, 32.0))
        } else {
            3.0
        };
        let target_time = (self.sample_rate * 15.0 / safe_bpm * steps).clamp(1.0, max_d);
        let fb = f64::from(patch.feedback.clamp(0.0, 0.95));
        let mix = f64::from(patch.mix.clamp(0.0, 1.0));
        let pp = match patch.mode {
            DelayMode::PingPong => 1.0,
            DelayMode::Stereo => 0.0,
        };

        self.target_time = target_time;
        if !self.has_processed {
            self.time_a = target_time;
            self.time_b = target_time;
            self.xfade = 1.0;
            self.feedback.snap(fb);
            self.mix.snap(mix);
            self.ping_pong_blend.snap(pp);
        } else {
            self.feedback.set_target(fb);
            self.mix.set_target(mix);
            self.ping_pong_blend.set_target(pp);
            if self.xfade >= 1.0 && (target_time - self.time_a).abs() > 0.5 {
                self.time_b = target_time;
                self.xfade = 0.0;
            }
        }
    }

    pub fn process(&mut self, input: [f32; 2], bypassed: bool) -> [f32; 2] {
        self.has_processed = true;
        let dry = sanitize_frame(input);
        if !self.line.is_allocated() {
            return dry;
        }

        if bypassed && !self.was_active {
            self.bypass_wet.snap(0.0);
            return dry;
        }

        self.bypass_wet.set_target(if bypassed { 0.0 } else { 1.0 });
        let bypass_w = self.bypass_wet.next();
        if !bypassed {
            self.was_active = true;
        } else if bypass_w <= 1e-4 {
            self.bypass_wet.snap(0.0);
        }

        let fb = self.feedback.next();
        let mix = self.mix.next();
        let pp = self.ping_pong_blend.next();
        let st = 1.0 - pp;

        // Crossfaded delay time transitions eliminate harsh Doppler pitch chirps.
        let delayed = if self.xfade < 1.0 {
            self.xfade = (self.xfade + self.xfade_step).min(1.0);
            let a = self.line.read_cubic(self.time_a);
            let b = self.line.read_cubic(self.time_b);
            let w = hann_fade_in(self.xfade as f32);
            let out = [a[0] + (b[0] - a[0]) * w, a[1] + (b[1] - a[1]) * w];
            if self.xfade >= 1.0 {
                self.time_a = self.time_b;
                if (self.target_time - self.time_a).abs() > 0.5 {
                    self.time_b = self.target_time;
                    self.xfade = 0.0;
                }
            }
            out
        } else {
            if (self.target_time - self.time_a).abs() > 1e-6 {
                self.time_a = self.target_time;
            }
            self.line.read_cubic(self.time_a)
        };

        // Feed [0, 0] while bypassed so the delay tail ages out cleanly and never resurfaces.
        let feed = if bypassed { [0.0; 2] } else { dry };
        let damped = self
            .damp_lpf
            .process_stereo([f64::from(delayed[0]), f64::from(delayed[1])]);

        // Smooth convex blend between Stereo (independent dual-channel feedback) and
        // PingPong (mono-summed Left injection bouncing L -> R -> L). Provably BIBO-stable
        // for every blend weight in [0, 1] because row L1 norm == fb <= 0.95.
        let ping_in = f64::from(feed[0] + feed[1]) * 0.5;
        let pp_l = ping_in + damped[1] * fb;
        let pp_r = damped[0] * fb;
        let st_l = f64::from(feed[0]) + damped[0] * fb;
        let st_r = f64::from(feed[1]) + damped[1] * fb;
        let next_l = (st * st_l + pp * pp_l) as f32;
        let next_r = (st * st_r + pp * pp_r) as f32;
        self.line.write([next_l, next_r]);

        if bypassed && bypass_w <= 1e-4 {
            return dry;
        }
        let eff_wet = (mix * bypass_w) as f32;
        let eff_dry = 1.0 - eff_wet;
        [
            dry[0] * eff_dry + delayed[0] * eff_wet,
            dry[1] * eff_dry + delayed[1] * eff_wet,
        ]
    }
}

impl crate::dsp::StereoEffect for Delay {
    #[inline]
    fn process(&mut self, input: [f32; 2], bypassed: bool) -> [f32; 2] {
        Delay::process(self, input, bypassed)
    }

    #[inline]
    fn reset(&mut self) {
        Delay::reset(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centered_mono_impulse_bounces_asymmetrically_left_right_left() {
        for sr in [44_100u32, 48_000, 96_000] {
            let mut delay = Delay::with_max_seconds(sr, 2.0);
            let bpm = 120.0; // 1 sixteenth = 0.125 s
            let steps = 1.0_f32;
            let tap_frames = (f64::from(sr) * 15.0 / bpm * f64::from(steps)).round() as usize;
            delay.set_parameters(
                DelayPatch {
                    mode: DelayMode::PingPong,
                    steps,
                    feedback: 0.6,
                    mix: 1.0,
                },
                bpm,
            );

            // Inject centered mono impulse [1.0, 1.0].
            let mut out = Vec::with_capacity(tap_frames * 4);
            for i in 0..(tap_frames * 4) {
                let x = if i == 0 { [1.0, 1.0] } else { [0.0, 0.0] };
                out.push(delay.process(x, false));
            }

            let window_sum = |center: usize, ch: usize| -> f64 {
                let lo = center.saturating_sub(32);
                let hi = (center + 128).min(out.len());
                out[lo..hi].iter().map(|f| f64::from(f[ch])).sum::<f64>()
            };
            let window_peak = |center: usize, ch: usize| -> f32 {
                let lo = center.saturating_sub(32);
                let hi = (center + 128).min(out.len());
                out[lo..hi]
                    .iter()
                    .map(|f| f[ch].abs())
                    .fold(0.0_f32, f32::max)
            };

            // Tap 1: pure Left with unit impulse area (~1.0) and zero Right bleed!
            let t1_l = window_sum(tap_frames, 0);
            let t1_r = window_peak(tap_frames, 1);
            assert!(
                (t1_l - 1.0).abs() < 0.05 && t1_r < 1e-4,
                "sr={sr} tap 1 expected Left (~1.0): L_sum={t1_l}, R_peak={t1_r}"
            );

            // Tap 2: pure Right with feedback area (~0.6) and zero Left bleed!
            let t2_l = window_peak(tap_frames * 2, 0);
            let t2_r = window_sum(tap_frames * 2, 1);
            assert!(
                (t2_r - 0.6).abs() < 0.05 && t2_l < 1e-4,
                "sr={sr} tap 2 expected Right (~0.6): L_peak={t2_l}, R_sum={t2_r}"
            );

            // Tap 3: pure Left bounce with feedback^2 area (~0.36) and zero Right bleed!
            let t3_l = window_sum(tap_frames * 3, 0);
            let t3_r = window_peak(tap_frames * 3, 1);
            assert!(
                (t3_l - 0.36).abs() < 0.05 && t3_r < 1e-4,
                "sr={sr} tap 3 expected Left (~0.36): L_sum={t3_l}, R_peak={t3_r}"
            );
        }
    }

    #[test]
    fn unallocated_delay_uses_zero_storage_and_allocates_cleanly() {
        let mut delay = Delay::new_unallocated(48_000);
        assert!(!delay.is_allocated());
        assert_eq!(delay.process([0.5, -0.5], false), [0.5, -0.5]);
        delay.allocate(1.0);
        assert!(delay.is_allocated());
        delay.set_parameters(
            DelayPatch {
                mode: DelayMode::PingPong,
                steps: 0.5,
                feedback: 0.0,
                mix: 1.0,
            },
            300.0,
        );
        // BPM guard protects against bpm = 0.0 / negative / NaN.
        delay.set_parameters(DelayPatch::default(), 0.0);
        delay.set_parameters(DelayPatch::default(), f64::NAN);
        let y = delay.process([0.5, 0.5], false);
        assert!(y[0].is_finite() && y[1].is_finite());
    }

    #[test]
    fn delay_time_change_crossfades_without_nan_and_reset_clears_history() {
        let mut delay = Delay::with_max_seconds(48_000, 2.0);
        delay.set_parameters(
            DelayPatch {
                mode: DelayMode::PingPong,
                steps: 2.0,
                feedback: 0.5,
                mix: 1.0,
            },
            120.0,
        );
        for i in 0..4800 {
            if i == 2400 {
                delay.set_parameters(
                    DelayPatch {
                        mode: DelayMode::PingPong,
                        steps: 6.0,
                        feedback: 0.5,
                        mix: 1.0,
                    },
                    120.0,
                );
            }
            let x = if i < 200 { [0.5, 0.5] } else { [0.0, 0.0] };
            let y = delay.process(x, false);
            assert!(y[0].is_finite() && y[1].is_finite() && y[0].abs() < 2.0);
        }
        delay.reset();
        for _ in 0..10_000 {
            assert_eq!(delay.process([0.0, 0.0], false), [0.0, 0.0]);
        }
    }

    #[test]
    fn delay_supports_32_steps_up_to_max_delay_seconds_ceiling() {
        // 32 sixteenth steps = 480 / BPM seconds.
        // At 120 BPM: 4.0 s (< 12.0 s MAX_DELAY_SECONDS).
        // At 40 BPM: 12.0 s (exact MAX_DELAY_SECONDS ceiling).
        // Below 40 BPM (e.g. 20 BPM): clamped cleanly to MAX_DELAY_SECONDS (12.0 s).
        let sr = 8_000u32; // compact sample rate for fast exact impulse verification
        let mut delay = Delay::new(sr);
        let bpm = 120.0;
        let expected_tap = (f64::from(sr) * 15.0 / bpm * 32.0).round() as usize; // 32_000 frames (4.0 s)
        delay.set_parameters(
            DelayPatch {
                mode: DelayMode::PingPong,
                steps: 32.0,
                feedback: 0.0,
                mix: 1.0,
            },
            bpm,
        );
        let mut peak_before = 0.0_f32;
        let mut tap_sum = 0.0_f64;
        for i in 0..(expected_tap + 256) {
            let x = if i == 0 { [1.0, 1.0] } else { [0.0, 0.0] };
            let y = delay.process(x, false);
            if i + 64 < expected_tap {
                peak_before = peak_before.max(y[0].abs()).max(y[1].abs());
            }
            if i >= expected_tap.saturating_sub(32) && i < expected_tap + 128 {
                tap_sum += f64::from(y[0]);
            }
        }
        assert!(
            peak_before < 1e-4,
            "32-step delay must stay silent before 4.0 s, got {peak_before}"
        );
        assert!(
            (tap_sum - 1.0).abs() < 0.05,
            "32-step delay tap at 120 BPM expected unit Left impulse at 4.0 s, got sum {tap_sum}"
        );

        // Verify exact 12.0 s ceiling at 40 BPM and finite clamp at 20 BPM.
        delay.reset();
        delay.set_parameters(
            DelayPatch {
                mode: DelayMode::PingPong,
                steps: 32.0,
                feedback: 0.25,
                mix: 1.0,
            },
            40.0,
        );
        assert!(
            (delay.target_time - f64::from(sr) * MAX_DELAY_SECONDS).abs() < 1.5,
            "32 steps at 40 BPM should hit exact MAX_DELAY_SECONDS ceiling ({})",
            delay.target_time
        );
        delay.set_parameters(
            DelayPatch {
                mode: DelayMode::PingPong,
                steps: 32.0,
                feedback: 0.25,
                mix: 1.0,
            },
            20.0,
        );
        assert!(delay.target_time <= delay.line.capacity() as f64 + 1e-6);
        let out = delay.process([0.5, 0.5], false);
        assert!(out[0].is_finite() && out[1].is_finite());
    }

    #[test]
    fn stereo_mode_preserves_panning_with_zero_cross_bleed_and_symmetric_return_pan() {
        use crate::dsp::gain::pan_stereo_balance;

        for sr in [44_100u32, 48_000, 96_000] {
            let bpm = 120.0;
            let steps = 1.0_f32;
            let tap_frames = (f64::from(sr) * 15.0 / bpm * f64::from(steps)).round() as usize;
            let patch = DelayPatch {
                mode: DelayMode::Stereo,
                steps,
                feedback: 0.6,
                mix: 1.0,
            };

            let run_impulse = |imp: [f32; 2]| -> Vec<[f32; 2]> {
                let mut delay = Delay::with_max_seconds(sr, 2.0);
                delay.set_parameters(patch, bpm);
                let mut out = Vec::with_capacity(tap_frames * 4);
                for i in 0..(tap_frames * 4) {
                    let x = if i == 0 { imp } else { [0.0, 0.0] };
                    out.push(delay.process(x, false));
                }
                out
            };

            let window_sum = |buf: &[[f32; 2]], center: usize, ch: usize| -> f64 {
                let lo = center.saturating_sub(32);
                let hi = (center + 128).min(buf.len());
                buf[lo..hi].iter().map(|f| f64::from(f[ch])).sum::<f64>()
            };
            let window_peak = |buf: &[[f32; 2]], center: usize, ch: usize| -> f32 {
                let lo = center.saturating_sub(32);
                let hi = (center + 128).min(buf.len());
                buf[lo..hi]
                    .iter()
                    .map(|f| f[ch].abs())
                    .fold(0.0_f32, f32::max)
            };

            // 1. Right-panned impulse [0.0, 1.0] stays strictly on Right with zero Left bleed.
            let right_out = run_impulse([0.0, 1.0]);
            for (k, expected) in [(1usize, 1.0_f64), (2, 0.6), (3, 0.36)] {
                let l_peak = window_peak(&right_out, tap_frames * k, 0);
                let r_sum = window_sum(&right_out, tap_frames * k, 1);
                assert!(
                    l_peak < 1e-6 && (r_sum - expected).abs() < 0.05,
                    "sr={sr} right tap {k}: L_peak={l_peak}, R_sum={r_sum} (expected {expected})"
                );
            }

            // 2. Left-panned impulse [1.0, 0.0] stays strictly on Left with zero Right bleed.
            let left_out = run_impulse([1.0, 0.0]);
            for (k, expected) in [(1usize, 1.0_f64), (2, 0.6), (3, 0.36)] {
                let l_sum = window_sum(&left_out, tap_frames * k, 0);
                let r_peak = window_peak(&left_out, tap_frames * k, 1);
                assert!(
                    r_peak < 1e-6 && (l_sum - expected).abs() < 0.05,
                    "sr={sr} left tap {k}: L_sum={l_sum} (expected {expected}), R_peak={r_peak}"
                );
            }

            // 3. Centered impulse [1.0, 1.0] stays perfectly symmetric so Return strip panning L/R is symmetric.
            let center_out = run_impulse([1.0, 1.0]);
            let mut pan_l_energy = [0.0_f64; 2];
            let mut pan_r_energy = [0.0_f64; 2];
            for &frame in &center_out {
                assert!(
                    (frame[0] - frame[1]).abs() < 1e-6,
                    "sr={sr} centered Stereo delay must keep L == R, got {frame:?}"
                );
                let [gl_l, gl_r] = pan_stereo_balance(-1.0);
                pan_l_energy[0] += f64::from(frame[0] * gl_l).powi(2);
                pan_l_energy[1] += f64::from(frame[1] * gl_r).powi(2);
                let [gr_l, gr_r] = pan_stereo_balance(1.0);
                pan_r_energy[0] += f64::from(frame[0] * gr_l).powi(2);
                pan_r_energy[1] += f64::from(frame[1] * gr_r).powi(2);
            }
            assert!(pan_l_energy[0] > 0.5 && pan_l_energy[1] < 1e-9);
            assert!(pan_r_energy[1] > 0.5 && pan_r_energy[0] < 1e-9);
            assert!((pan_l_energy[0] - pan_r_energy[1]).abs() < 1e-6);
        }
    }

    #[test]
    fn delay_mode_switches_smoothly_without_clicks_or_instability() {
        let sr = 48_000u32;
        let mut delay = Delay::with_max_seconds(sr, 2.0);
        let bpm = 120.0;
        let pp = DelayPatch {
            mode: DelayMode::PingPong,
            steps: 1.0,
            feedback: 0.75,
            mix: 1.0,
        };
        let st = DelayPatch {
            mode: DelayMode::Stereo,
            ..pp
        };
        delay.set_parameters(pp, bpm);

        let mut prev = [0.0_f32; 2];
        let mut max_step_jump = 0.0_f32;
        for i in 0..24_000 {
            if i == 8_000 {
                delay.set_parameters(st, bpm);
            } else if i == 16_000 {
                delay.set_parameters(pp, bpm);
            }
            let phase = std::f64::consts::TAU * 220.0 * i as f64 / f64::from(sr);
            let x = [(phase.sin() * 0.25) as f32, (phase.sin() * 0.12) as f32];
            let y = delay.process(x, false);
            assert!(y[0].is_finite() && y[1].is_finite());
            assert!(y[0].abs() < 2.5 && y[1].abs() < 2.5);
            if i > 100 {
                max_step_jump = max_step_jump
                    .max((y[0] - prev[0]).abs())
                    .max((y[1] - prev[1]).abs());
            }
            prev = y;
        }
        assert!(
            max_step_jump < 0.25,
            "mode switch must stay smooth without clicks, max_step_jump={max_step_jump}"
        );
    }
}
