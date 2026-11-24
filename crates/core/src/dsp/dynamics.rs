//! Studio dynamics processors: shared soft-knee gain computer, stereo-linked
//! feed-forward [`Compressor`], and stereo-linked lookahead brickwall [`Limiter`].
//!
//! Both processors smooth gain reduction in the dB domain after the gain computer,
//! and [`Limiter`] advances its lookahead buffer continuously across bypass so
//! plugin delay compensation stays constant without stale bursts.

use crate::dsp::math::{
    db_to_lin_f64, lin_to_db_f64, mix_stereo_f64, one_pole_decay_ms, sanitize_clamp_f32,
    sanitize_clamp_f64, sanitize_frame, sanitize_sample_f64, sanitize_sample_rate, BypassFader,
    BypassStep, Smoother, LN_100,
};
use crate::mixer::{CompressorPatch, LimiterPatch};

/// Soft knee width of Cadence's studio `Compressor` (6.0 dB).
pub const COMPRESSOR_KNEE_DB: f32 = 6.0;
/// Soft knee width of Cadence's studio `Limiter` (2.0 dB).
pub const LIMITER_KNEE_DB: f32 = 2.0;
/// Lookahead window of Cadence's brickwall `Limiter` (1.5 ms).
pub const LIMITER_LOOKAHEAD_SEC: f64 = 0.0015;

/// Lookahead delay in samples for [`Limiter`] at `sample_rate` Hz (single source of
/// truth shared with `EffectDevice::latency_frames`).
#[inline]
pub fn limiter_lookahead_frames(sample_rate: f64) -> usize {
    let sr = sanitize_sample_rate(sample_rate);
    ((sr * LIMITER_LOOKAHEAD_SEC).round() as usize).clamp(4, 1024)
}

/// Pure quadratic soft-knee gain reduction in dB (`>= 0.0`) in `f64`.
///
/// Single source of truth for `Compressor` (`knee_db = 6.0`, `ratio = 1..20`),
/// `Limiter` (`knee_db = 2.0`, `ratio = f64::INFINITY`), and UI transfer plots.
#[inline]
pub fn soft_knee_gr_db_f64(input_db: f64, threshold_db: f64, ratio: f64, knee_db: f64) -> f64 {
    if !input_db.is_finite() || !threshold_db.is_finite() {
        return 0.0;
    }
    let slope = if ratio.is_infinite() && ratio.is_sign_positive() {
        1.0
    } else if ratio.is_finite() {
        1.0 - 1.0 / ratio.max(1.0)
    } else {
        0.0
    };
    let knee = sanitize_sample_f64(knee_db).max(0.0);
    let over = input_db - threshold_db;
    if knee <= 1e-6 {
        if over <= 0.0 {
            0.0
        } else {
            slope * over
        }
    } else {
        let half_knee = knee * 0.5;
        if over <= -half_knee {
            0.0
        } else if over >= half_knee {
            slope * over
        } else {
            let x = over + half_knee;
            slope * x * x / (2.0 * knee)
        }
    }
}

/// Pure quadratic soft-knee gain reduction in dB (`>= 0.0`, `f32` wrapper).
#[inline]
pub fn soft_knee_gr_db(input_db: f32, threshold_db: f32, ratio: f32, knee_db: f32) -> f32 {
    let r = if ratio.is_infinite() && ratio.is_sign_positive() {
        f64::INFINITY
    } else {
        f64::from(ratio)
    };
    soft_knee_gr_db_f64(
        f64::from(input_db),
        f64::from(threshold_db),
        r,
        f64::from(knee_db),
    ) as f32
}

/// Stereo-linked feed-forward soft-knee compressor smoothing **`gr_db` after the
/// gain computer** (starting from `0.0 dB` with zero startup lag) and click-free
/// bypass crossfade. Zero heap allocations.
pub struct Compressor {
    sample_rate: f64,
    threshold_db: f64,
    ratio: f64,
    attack_decay: f64,
    release_decay: f64,
    makeup_gain: Smoother,
    mix: Smoother,
    bypass: BypassFader,
    gr_db: f64,
    current_gr_db: f32,
    last_patch: CompressorPatch,
    has_processed: bool,
}

impl Compressor {
    pub fn new(sample_rate: f64) -> Self {
        let sr = sanitize_sample_rate(sample_rate);
        let default = CompressorPatch::default();
        let mut c = Self {
            sample_rate: sr,
            threshold_db: f64::from(default.threshold_db),
            ratio: f64::from(default.ratio),
            attack_decay: one_pole_decay_ms(default.attack_ms, sr),
            release_decay: one_pole_decay_ms(default.release_ms, sr),
            makeup_gain: Smoother::new(db_to_lin_f64(f64::from(default.makeup_db)), 4.0, sr),
            mix: Smoother::new(f64::from(default.mix), 4.0, sr),
            bypass: BypassFader::new(sr),
            gr_db: 0.0,
            current_gr_db: 0.0,
            last_patch: default,
            has_processed: false,
        };
        c.set_parameters(default);
        c
    }

    #[cfg(test)]
    pub(crate) fn is_idle(&self) -> bool {
        self.bypass.is_idle()
    }

    pub fn reset(&mut self) {
        self.gr_db = 0.0;
        self.current_gr_db = 0.0;
        self.makeup_gain.snap(self.makeup_gain.target());
        self.mix.snap(self.mix.target());
        self.bypass.reset();
        self.has_processed = false;
    }

    /// Current instantaneous gain reduction in dB (`>= 0.0`, zero heap allocation).
    #[inline]
    pub fn gain_reduction_db(&self) -> f32 {
        self.current_gr_db
    }

    pub fn set_parameters(&mut self, patch: CompressorPatch) {
        let def = CompressorPatch::default();
        let threshold_db = sanitize_clamp_f64(
            f64::from(patch.threshold_db),
            -48.0,
            0.0,
            f64::from(def.threshold_db),
        );
        let ratio = sanitize_clamp_f64(f64::from(patch.ratio), 1.0, 20.0, f64::from(def.ratio));
        let att_ms = sanitize_clamp_f32(patch.attack_ms, 0.5, 100.0, def.attack_ms);
        let rel_ms = sanitize_clamp_f32(patch.release_ms, 10.0, 1000.0, def.release_ms);
        let makeup_db = sanitize_clamp_f64(
            f64::from(patch.makeup_db),
            -6.0,
            24.0,
            f64::from(def.makeup_db),
        );
        let mix = sanitize_clamp_f64(f64::from(patch.mix), 0.0, 1.0, f64::from(def.mix));
        let sanitized = CompressorPatch {
            threshold_db: threshold_db as f32,
            ratio: ratio as f32,
            attack_ms: att_ms,
            release_ms: rel_ms,
            makeup_db: makeup_db as f32,
            mix: mix as f32,
        };
        if self.has_processed && sanitized == self.last_patch {
            return;
        }
        self.last_patch = sanitized;
        self.threshold_db = threshold_db;
        self.ratio = ratio;
        self.attack_decay = one_pole_decay_ms(att_ms, self.sample_rate);
        self.release_decay = one_pole_decay_ms(rel_ms, self.sample_rate);
        let makeup = db_to_lin_f64(makeup_db);
        if !self.has_processed {
            self.makeup_gain.snap(makeup);
            self.mix.snap(mix);
        } else {
            self.makeup_gain.set_target(makeup);
            self.mix.set_target(mix);
        }
    }

    pub fn process(&mut self, input: [f32; 2], bypassed: bool) -> [f32; 2] {
        self.has_processed = true;
        let dry = sanitize_frame(input);
        let bypass_w = match self.bypass.step(bypassed) {
            BypassStep::Idle => {
                self.gr_db = 0.0;
                self.current_gr_db = 0.0;
                return dry;
            }
            BypassStep::SettledBypass => {
                self.gr_db = 0.0;
                self.current_gr_db = 0.0;
                self.bypass.mark_idle();
                return dry;
            }
            BypassStep::Active(w) => w,
        };

        let peak = f64::from(dry[0].abs().max(dry[1].abs()));
        let level_db = lin_to_db_f64(peak);
        let target_gr_db = soft_knee_gr_db_f64(
            level_db,
            self.threshold_db,
            self.ratio,
            f64::from(COMPRESSOR_KNEE_DB),
        );
        let coeff = if target_gr_db > self.gr_db {
            self.attack_decay
        } else {
            self.release_decay
        };
        self.gr_db = coeff * self.gr_db + (1.0 - coeff) * target_gr_db;
        if self.gr_db < 1e-6 {
            self.gr_db = 0.0;
        }
        self.current_gr_db = if bypassed { 0.0 } else { self.gr_db as f32 };

        let makeup = self.makeup_gain.next();
        let wet_mix = self.mix.next();
        let gain = if self.gr_db <= 0.0 {
            makeup
        } else {
            db_to_lin_f64(-self.gr_db) * makeup
        };
        let dry64 = [f64::from(dry[0]), f64::from(dry[1])];
        let wet = [dry64[0] * gain, dry64[1] * gain];
        let out = mix_stereo_f64(dry64, wet, wet_mix * bypass_w);
        [out[0] as f32, out[1] as f32]
    }
}

impl crate::dsp::StereoEffect for Compressor {
    #[inline]
    fn process(&mut self, input: [f32; 2], bypassed: bool) -> [f32; 2] {
        Compressor::process(self, input, bypassed)
    }

    #[inline]
    fn reset(&mut self) {
        Compressor::reset(self);
    }

    #[inline]
    fn gain_reduction_db(&self) -> Option<f32> {
        Some(Compressor::gain_reduction_db(self))
    }
}

/// Stereo-linked lookahead brickwall peak limiter (`1.5 ms` lookahead) with
/// O(1) sliding-window peak deque, dB-domain gain reduction smoothing, smoothed
/// `ceiling_db` / `drive` / `mix`, and continuous lookahead advance across
/// bypass. Heap-allocated once in `new`; zero RT allocations.
pub struct Limiter {
    sample_rate: f64,
    lookahead: usize,
    delay_buf: Box<[[f32; 2]]>,
    write_idx: usize,
    deque: Box<[(u64, f64)]>,
    deque_head: usize,
    deque_tail: usize,
    sample_index: u64,
    ceiling_db: Smoother,
    drive_db: Smoother,
    mix: Smoother,
    bypass: BypassFader,
    cached_ceiling_db_bits: u64,
    cached_ceiling_lin: f64,
    cached_drive_db_bits: u64,
    cached_drive_lin: f64,
    attack_decay: f64,
    release_decay: f64,
    gr_db: f64,
    current_gr_db: f32,
    last_patch: LimiterPatch,
    has_processed: bool,
}

impl Limiter {
    pub fn new(sample_rate: f64) -> Self {
        let sr = sanitize_sample_rate(sample_rate);
        let lookahead = limiter_lookahead_frames(sr);
        let deque_cap = lookahead + 2;
        // Reach 99% of target GR across the exact lookahead duration before the peak arrives.
        let attack_decay = (-LN_100 / lookahead as f64).exp();
        let default = LimiterPatch::default();
        let init_ceil = f64::from(default.ceiling_db);
        let init_drive = f64::from(default.input_drive_db);
        let mut lim = Self {
            sample_rate: sr,
            lookahead,
            delay_buf: vec![[0.0; 2]; lookahead].into_boxed_slice(),
            write_idx: 0,
            deque: vec![(0, 0.0); deque_cap].into_boxed_slice(),
            deque_head: 0,
            deque_tail: 0,
            sample_index: 0,
            ceiling_db: Smoother::new(init_ceil, 4.0, sr),
            drive_db: Smoother::new(init_drive, 4.0, sr),
            mix: Smoother::new(f64::from(default.mix), 4.0, sr),
            bypass: BypassFader::new(sr),
            cached_ceiling_db_bits: init_ceil.to_bits(),
            cached_ceiling_lin: db_to_lin_f64(init_ceil),
            cached_drive_db_bits: init_drive.to_bits(),
            cached_drive_lin: db_to_lin_f64(init_drive),
            attack_decay,
            release_decay: one_pole_decay_ms(default.release_ms, sr),
            gr_db: 0.0,
            current_gr_db: 0.0,
            last_patch: default,
            has_processed: false,
        };
        lim.set_parameters(default);
        lim
    }

    #[cfg(test)]
    pub(crate) fn is_idle(&self) -> bool {
        self.bypass.is_idle()
    }

    pub fn reset(&mut self) {
        self.delay_buf.fill([0.0; 2]);
        self.write_idx = 0;
        self.deque_head = 0;
        self.deque_tail = 0;
        self.sample_index = 0;
        self.gr_db = 0.0;
        self.current_gr_db = 0.0;
        self.ceiling_db.snap(self.ceiling_db.target());
        self.drive_db.snap(self.drive_db.target());
        self.mix.snap(self.mix.target());
        self.bypass.reset();
        self.has_processed = false;
    }

    /// Lookahead latency in sample frames (constant across active & bypassed states for PDC).
    #[inline]
    pub fn latency_frames(&self) -> usize {
        self.lookahead
    }

    /// Current instantaneous gain reduction in dB (`>= 0.0`, zero heap allocation).
    #[inline]
    pub fn gain_reduction_db(&self) -> f32 {
        self.current_gr_db
    }

    pub fn set_parameters(&mut self, patch: LimiterPatch) {
        let def = LimiterPatch::default();
        let ceil = sanitize_clamp_f64(
            f64::from(patch.ceiling_db),
            -18.0,
            -0.1,
            f64::from(def.ceiling_db),
        );
        let drive = sanitize_clamp_f64(
            f64::from(patch.input_drive_db),
            0.0,
            24.0,
            f64::from(def.input_drive_db),
        );
        let rel_ms = sanitize_clamp_f32(patch.release_ms, 10.0, 1000.0, def.release_ms);
        let mix = sanitize_clamp_f64(f64::from(patch.mix), 0.0, 1.0, f64::from(def.mix));
        let sanitized = LimiterPatch {
            ceiling_db: ceil as f32,
            input_drive_db: drive as f32,
            release_ms: rel_ms,
            mix: mix as f32,
        };
        if self.has_processed && sanitized == self.last_patch {
            return;
        }
        self.last_patch = sanitized;
        self.release_decay = one_pole_decay_ms(rel_ms, self.sample_rate);
        if !self.has_processed {
            self.ceiling_db.snap(ceil);
            self.drive_db.snap(drive);
            self.mix.snap(mix);
        } else {
            self.ceiling_db.set_target(ceil);
            self.drive_db.set_target(drive);
            self.mix.set_target(mix);
        }
    }

    #[inline]
    fn push_window_peak(&mut self, peak: f64) -> f64 {
        let cap = self.deque.len();
        let idx = self.sample_index;
        let max_age = self.lookahead as u64;
        while self.deque_head != self.deque_tail
            && idx.saturating_sub(self.deque[self.deque_head].0) > max_age
        {
            let next = self.deque_head + 1;
            self.deque_head = if next >= cap { 0 } else { next };
        }
        while self.deque_head != self.deque_tail {
            let prev = if self.deque_tail == 0 {
                cap - 1
            } else {
                self.deque_tail - 1
            };
            if self.deque[prev].1 <= peak {
                self.deque_tail = prev;
            } else {
                break;
            }
        }
        self.deque[self.deque_tail] = (idx, peak);
        let next_tail = self.deque_tail + 1;
        self.deque_tail = if next_tail >= cap { 0 } else { next_tail };
        self.sample_index = idx.wrapping_add(1);
        self.deque[self.deque_head].1
    }

    /// Process one stereo frame.
    ///
    /// While `bypassed == true`, `delay_buf` continues to advance with live dry audio so
    /// lookahead latency stays constant for PDC with zero stale audio bursts on un-bypass;
    /// once the bypass crossfade settles, the peak deque resets and skips per-sample work.
    pub fn process(&mut self, input: [f32; 2], bypassed: bool) -> [f32; 2] {
        self.has_processed = true;
        let dry_in = sanitize_frame(input);
        let delayed_dry = self.delay_buf[self.write_idx];
        self.delay_buf[self.write_idx] = dry_in;
        self.write_idx += 1;
        if self.write_idx >= self.lookahead {
            self.write_idx = 0;
        }

        let bypass_w = match self.bypass.step(bypassed) {
            BypassStep::Idle => {
                self.gr_db = 0.0;
                self.current_gr_db = 0.0;
                return delayed_dry;
            }
            BypassStep::SettledBypass => {
                self.deque_head = 0;
                self.deque_tail = 0;
                self.gr_db = 0.0;
                self.current_gr_db = 0.0;
                self.bypass.mark_idle();
                return delayed_dry;
            }
            BypassStep::Active(w) => w,
        };

        let ceiling_db = self.ceiling_db.next();
        let ceiling_bits = ceiling_db.to_bits();
        if ceiling_bits != self.cached_ceiling_db_bits {
            self.cached_ceiling_db_bits = ceiling_bits;
            self.cached_ceiling_lin = db_to_lin_f64(ceiling_db);
        }
        let ceiling_lin = self.cached_ceiling_lin;

        let drive_db = self.drive_db.next();
        let drive_bits = drive_db.to_bits();
        if drive_bits != self.cached_drive_db_bits {
            self.cached_drive_db_bits = drive_bits;
            self.cached_drive_lin = db_to_lin_f64(drive_db);
        }
        let drive_lin = self.cached_drive_lin;
        let mix = self.mix.next();

        let in_peak = if bypassed {
            0.0
        } else {
            f64::from(dry_in[0].abs().max(dry_in[1].abs())) * drive_lin
        };
        let window_peak = self.push_window_peak(in_peak);
        let window_db = lin_to_db_f64(window_peak);
        let target_gr_db = soft_knee_gr_db_f64(
            window_db,
            ceiling_db,
            f64::INFINITY,
            f64::from(LIMITER_KNEE_DB),
        );

        // Smooth gain reduction in the dB domain (eliminating linear-space release pumping).
        let coeff = if target_gr_db > self.gr_db {
            self.attack_decay
        } else {
            self.release_decay
        };
        self.gr_db = coeff * self.gr_db + (1.0 - coeff) * target_gr_db;

        let del_l = f64::from(delayed_dry[0]) * drive_lin;
        let del_r = f64::from(delayed_dry[1]) * drive_lin;
        let delayed_peak = del_l.abs().max(del_r.abs());
        if delayed_peak > ceiling_lin {
            let req_gr_db = (lin_to_db_f64(delayed_peak) - ceiling_db).max(0.0);
            if req_gr_db > self.gr_db {
                self.gr_db = req_gr_db;
            }
        }
        if self.gr_db < 1e-5 {
            self.gr_db = 0.0;
        }
        self.current_gr_db = if bypassed { 0.0 } else { self.gr_db as f32 };

        let gain = if self.gr_db <= 0.0 {
            1.0
        } else {
            db_to_lin_f64(-self.gr_db)
        };
        let wet_l = (del_l * gain).clamp(-ceiling_lin, ceiling_lin);
        let wet_r = (del_r * gain).clamp(-ceiling_lin, ceiling_lin);

        let out = mix_stereo_f64(
            [f64::from(delayed_dry[0]), f64::from(delayed_dry[1])],
            [wet_l, wet_r],
            mix * bypass_w,
        );
        [out[0] as f32, out[1] as f32]
    }
}

impl crate::dsp::StereoEffect for Limiter {
    #[inline]
    fn process(&mut self, input: [f32; 2], bypassed: bool) -> [f32; 2] {
        Limiter::process(self, input, bypassed)
    }

    #[inline]
    fn reset(&mut self) {
        Limiter::reset(self);
    }

    #[inline]
    fn latency_frames(&self) -> usize {
        Limiter::latency_frames(self)
    }

    #[inline]
    fn gain_reduction_db(&self) -> Option<f32> {
        Some(Limiter::gain_reduction_db(self))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    #[test]
    fn compressor_ac_sine_matches_soft_knee_without_startup_delay() {
        for sr in [44_100.0, 48_000.0, 96_000.0] {
            let mut c = Compressor::new(sr);
            let patch = CompressorPatch {
                threshold_db: -20.0,
                ratio: 8.0,
                attack_ms: 2.0,
                release_ms: 80.0,
                makeup_db: 0.0,
                mix: 1.0,
            };
            c.set_parameters(patch);

            // Sample 1 on a 0 dBFS transient must begin reducing gain IMMEDIATELY (zero -120 dB startup lag!).
            let _ = c.process([1.0, 1.0], false);
            assert!(
                c.gain_reduction_db() > 0.05,
                "sr={sr}: expected immediate attack on sample 1, got {} dB",
                c.gain_reduction_db()
            );

            // AC 1 kHz sine at 0 dBFS peak should track theoretical soft-knee GR (~17.5 dB) closely!
            c.reset();
            let expected_gr = soft_knee_gr_db(0.0, -20.0, 8.0, COMPRESSOR_KNEE_DB);
            assert!((expected_gr - 17.5).abs() < 1e-4);
            let frames = (sr * 0.15) as usize;
            for i in 0..frames {
                let s = (TAU * 1_000.0 * i as f64 / sr).sin() as f32;
                let _ = c.process([s, s], false);
            }
            let ac_gr = c.gain_reduction_db();
            assert!(
                (ac_gr - expected_gr).abs() < 1.5,
                "sr={sr}: AC sine GR={ac_gr:.2} dB vs theoretical {expected_gr:.2} dB"
            );

            // Bypass crossfades click-free over ~1.5 ms and settles to exact dry + 0.0 dB GR.
            let first_bypass = c.process([0.75, -0.5], true);
            assert!(
                first_bypass[0] < 0.70 && first_bypass[0] > 0.05,
                "sr={sr}: expected smooth bypass fade-out on frame 1, got {first_bypass:?}"
            );
            assert!(!c.is_idle());
            let fade_frames = (sr * 0.025) as usize;
            let mut prev_l = first_bypass[0];
            for _ in 0..fade_frames {
                let y = c.process([0.75, -0.5], true);
                assert!(y[0].is_finite() && y[1].is_finite());
                assert!(
                    (y[0] - prev_l).abs() < 0.05,
                    "sr={sr}: bypass fade step too large: prev={prev_l}, next={}",
                    y[0]
                );
                prev_l = y[0];
            }
            assert!(c.is_idle(), "sr={sr}: bypassed compressor must idle");
            assert_eq!(c.process([0.75, -0.5], true), [0.75, -0.5]);
            assert_eq!(c.gain_reduction_db(), 0.0);
        }
    }

    #[test]
    fn limiter_enforces_brickwall_ceiling_and_flushes_cleanly_across_bypass() {
        for sr in [44_100.0, 48_000.0, 96_000.0] {
            let mut lim = Limiter::new(sr);
            let ceiling_db = -3.0_f32;
            let ceiling_lin = crate::dsp::math::db_to_lin(ceiling_db);
            lim.set_parameters(LimiterPatch {
                ceiling_db,
                input_drive_db: 12.0,
                release_ms: 60.0,
                mix: 1.0,
            });
            assert_eq!(lim.latency_frames(), limiter_lookahead_frames(sr));

            let mut max_out = 0.0_f32;
            let mut max_gr = 0.0_f32;
            let frames = (sr * 0.1) as usize;
            for i in 0..frames {
                let x = if i == 100 {
                    1.0_f32
                } else {
                    (TAU * 440.0 * i as f64 / sr).sin() as f32 * 0.9
                };
                let y = lim.process([x, -x * 0.8], false);
                assert!(y[0].is_finite() && y[1].is_finite());
                max_out = max_out.max(y[0].abs()).max(y[1].abs());
                max_gr = max_gr.max(lim.gain_reduction_db());
            }
            assert!(
                max_out <= ceiling_lin + 1e-3,
                "sr={sr}: peak={max_out} > ceiling={ceiling_lin}"
            );
            assert!(max_gr > 10.0, "sr={sr}: max_gr={max_gr}");

            // Bypass with silence for >= 3× lookahead + fade time advances delay_buf and expires old peaks.
            let flush_frames = lim.latency_frames() * 4 + (sr * 0.02) as usize;
            for _ in 0..flush_frames {
                let _ = lim.process([0.0, 0.0], true);
            }
            assert_eq!(lim.gain_reduction_db(), 0.0);

            // Un-bypassing with silence must produce ZERO stale audio burst!
            for _ in 0..(lim.latency_frames() * 2) {
                let y = lim.process([0.0, 0.0], false);
                assert!(
                    y[0].abs() < 1e-6 && y[1].abs() < 1e-6,
                    "sr={sr}: stale audio burst on un-bypass: {y:?}"
                );
            }
        }
    }

    #[test]
    fn limiter_bypass_idles_preserving_pdc_and_dynamics_sanitize_nan_params() {
        for sr in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            let mut lim = Limiter::new(sr);
            let lookahead = lim.latency_frames();
            lim.set_parameters(LimiterPatch {
                ceiling_db: -6.0,
                input_drive_db: 12.0,
                release_ms: 40.0,
                mix: 1.0,
            });
            for _ in 0..(lookahead * 2) {
                let _ = lim.process([0.9, -0.9], false);
            }
            assert!(!lim.is_idle());

            // Bypass until fade settles -> limiter enters idle while preserving exact lookahead PDC.
            let flush = lookahead * 2 + (sr * 0.03) as usize;
            for _ in 0..flush {
                let _ = lim.process([0.0, 0.0], true);
            }
            assert!(lim.is_idle(), "sr={sr}: bypassed limiter must idle");
            assert_eq!(lim.gain_reduction_db(), 0.0);

            // Inject a dry impulse while idle-bypassed: must emerge at exact `lookahead` delay.
            let mut emerged_at = None;
            for i in 0..=(lookahead + 4) {
                let x = if i == 0 { [0.75, -0.5] } else { [0.0, 0.0] };
                let y = lim.process(x, true);
                assert!(lim.is_idle());
                if y[0].abs() > 0.5 {
                    emerged_at = Some((i, y));
                }
            }
            assert_eq!(
                emerged_at,
                Some((lookahead, [0.75, -0.5])),
                "sr={sr}: idle-bypassed limiter must preserve exact lookahead PDC"
            );

            // NaN parameter patches sanitize cleanly for both Compressor and Limiter.
            lim.set_parameters(LimiterPatch {
                ceiling_db: f32::NAN,
                input_drive_db: f32::NAN,
                release_ms: f32::NAN,
                mix: f32::NAN,
            });
            let y_lim = lim.process([0.5, -0.5], false);
            assert!(y_lim[0].is_finite() && y_lim[1].is_finite());

            let mut comp = Compressor::new(sr);
            comp.set_parameters(CompressorPatch {
                threshold_db: f32::NAN,
                ratio: f32::NAN,
                attack_ms: f32::NAN,
                release_ms: f32::NAN,
                makeup_db: f32::NAN,
                mix: f32::NAN,
            });
            let y_comp = comp.process([0.5, -0.5], false);
            assert!(y_comp[0].is_finite() && y_comp[1].is_finite());
        }
    }

    #[test]
    fn compressor_bypass_crossfades_click_free_and_idles_across_sample_rates() {
        for sr in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            let mut comp = Compressor::new(sr);
            assert!(comp.is_idle());
            // Initial bypass while idle passes dry untouched and stays idle.
            assert_eq!(comp.process([0.6, -0.4], true), [0.6, -0.4]);
            assert!(comp.is_idle());

            let patch = CompressorPatch {
                threshold_db: -24.0,
                ratio: 10.0,
                attack_ms: 1.0,
                release_ms: 120.0,
                makeup_db: 0.0,
                mix: 1.0,
            };
            comp.set_parameters(patch);

            // Engage strong steady-state compression.
            let drive_frames = (sr * 0.05) as usize;
            let mut last_active = [0.0_f32; 2];
            for _ in 0..drive_frames {
                last_active = comp.process([0.8, -0.6], false);
            }
            assert!(!comp.is_idle());
            assert!(
                comp.gain_reduction_db() > 14.0,
                "sr={sr}: expected strong GR, got {}",
                comp.gain_reduction_db()
            );
            assert!(last_active[0] < 0.2);

            // Toggle bypass: output must fade smoothly over ~1.5 ms without a click and reach idle.
            let tau_frames = (0.0015 * sr).round() as usize;
            let mut prev = last_active;
            let mut max_step = 0.0_f32;
            let mut at_tau = [0.0_f32; 2];
            let settle_frames = (0.03 * sr) as usize;
            for i in 0..settle_frames {
                let y = comp.process([0.8, -0.6], true);
                assert!(y[0].is_finite() && y[1].is_finite());
                max_step = max_step
                    .max((y[0] - prev[0]).abs())
                    .max((y[1] - prev[1]).abs());
                if i + 1 == tau_frames {
                    at_tau = y;
                }
                prev = y;
            }
            assert!(
                max_step < 0.02,
                "sr={sr}: bypass crossfade clicked with max_step={max_step}"
            );
            // At t = tau (1.5 ms), wet weight is ~1/e so output sits well between compressed and dry.
            assert!(
                at_tau[0] > 0.35 && at_tau[0] < 0.70,
                "sr={sr}: unexpected output at 1.5 ms fade tau: {at_tau:?}"
            );
            assert!(comp.is_idle(), "sr={sr}: compressor must idle after bypass");
            assert_eq!(comp.gain_reduction_db(), 0.0);
            assert_eq!(comp.process([0.8, -0.6], true), [0.8, -0.6]);

            // Un-bypassing after idle starts cleanly from 0.0 dB GR with smooth fade-in.
            let resume = comp.process([0.2, -0.2], false);
            assert!(resume[0].is_finite() && (resume[0] - 0.2).abs() < 0.02);
            assert!(!comp.is_idle());
        }
    }
}
