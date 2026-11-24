//! Studio dynamics processors: shared soft-knee gain computer, stereo-linked
//! feed-forward [`Compressor`], and stereo-linked lookahead brickwall [`Limiter`].
//!
//! Both processors smooth gain reduction in the dB domain after the gain computer,
//! and [`Limiter`] advances its lookahead buffer continuously across bypass so
//! plugin delay compensation stays constant without stale bursts.

use crate::dsp::math::{
    db_to_lin_f64, lin_to_db_f64, one_pole_decay_ms, sanitize_frame, sanitize_sample_f64,
    sanitize_sample_rate, Smoother,
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
/// gain computer** (starting from `0.0 dB` with zero startup lag). Zero heap allocations.
pub struct Compressor {
    sample_rate: f64,
    threshold_db: f64,
    ratio: f64,
    attack_decay: f64,
    release_decay: f64,
    makeup_gain: Smoother,
    mix: Smoother,
    gr_db: f64,
    current_gr_db: f32,
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
            gr_db: 0.0,
            current_gr_db: 0.0,
            has_processed: false,
        };
        c.set_parameters(default);
        c
    }

    pub fn reset(&mut self) {
        self.gr_db = 0.0;
        self.current_gr_db = 0.0;
        self.makeup_gain.snap(self.makeup_gain.target());
        self.mix.snap(self.mix.target());
        self.has_processed = false;
    }

    /// Current instantaneous gain reduction in dB (`>= 0.0`, zero heap allocation).
    #[inline]
    pub fn gain_reduction_db(&self) -> f32 {
        self.current_gr_db
    }

    pub fn set_parameters(&mut self, patch: CompressorPatch) {
        self.threshold_db = f64::from(patch.threshold_db).clamp(-48.0, 0.0);
        self.ratio = f64::from(patch.ratio).clamp(1.0, 20.0);
        let att_ms = patch.attack_ms.clamp(0.5, 100.0);
        let rel_ms = patch.release_ms.clamp(10.0, 1000.0);
        self.attack_decay = one_pole_decay_ms(att_ms, self.sample_rate);
        self.release_decay = one_pole_decay_ms(rel_ms, self.sample_rate);
        let makeup = db_to_lin_f64(f64::from(patch.makeup_db).clamp(-6.0, 24.0));
        let mix = f64::from(patch.mix).clamp(0.0, 1.0);
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
        if bypassed {
            // Reset envelope on bypass so un-bypassing starts cleanly at 0.0 dB GR.
            self.gr_db = 0.0;
            self.current_gr_db = 0.0;
            return dry;
        }
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
        self.current_gr_db = self.gr_db as f32;

        let makeup = self.makeup_gain.next();
        let wet_mix = self.mix.next();
        let dry_mix = 1.0 - wet_mix;
        let gain = db_to_lin_f64(-self.gr_db) * makeup;
        let total = dry_mix + wet_mix * gain;
        [
            (f64::from(dry[0]) * total) as f32,
            (f64::from(dry[1]) * total) as f32,
        ]
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
    bypass_wet: Smoother,
    attack_decay: f64,
    release_decay: f64,
    gr_db: f64,
    current_gr_db: f32,
    has_processed: bool,
    was_active: bool,
}

impl Limiter {
    pub fn new(sample_rate: f64) -> Self {
        let sr = sanitize_sample_rate(sample_rate);
        let lookahead = limiter_lookahead_frames(sr);
        let deque_cap = lookahead + 2;
        // Reach 99% of target GR across the exact lookahead duration before the peak arrives.
        let attack_decay = (-4.605_170_185_988_091 / lookahead as f64).exp();
        let default = LimiterPatch::default();
        let mut lim = Self {
            sample_rate: sr,
            lookahead,
            delay_buf: vec![[0.0; 2]; lookahead].into_boxed_slice(),
            write_idx: 0,
            deque: vec![(0, 0.0); deque_cap].into_boxed_slice(),
            deque_head: 0,
            deque_tail: 0,
            sample_index: 0,
            ceiling_db: Smoother::new(f64::from(default.ceiling_db), 4.0, sr),
            drive_db: Smoother::new(f64::from(default.input_drive_db), 4.0, sr),
            mix: Smoother::new(f64::from(default.mix), 4.0, sr),
            bypass_wet: Smoother::new(1.0, 1.5, sr),
            attack_decay,
            release_decay: one_pole_decay_ms(default.release_ms, sr),
            gr_db: 0.0,
            current_gr_db: 0.0,
            has_processed: false,
            was_active: false,
        };
        lim.set_parameters(default);
        lim
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
        self.bypass_wet.snap(1.0);
        self.has_processed = false;
        self.was_active = false;
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
        let ceil = f64::from(patch.ceiling_db).clamp(-18.0, -0.1);
        let drive = f64::from(patch.input_drive_db).clamp(0.0, 24.0);
        let rel_ms = patch.release_ms.clamp(10.0, 1000.0);
        let mix = f64::from(patch.mix).clamp(0.0, 1.0);
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
            self.deque_head = (self.deque_head + 1) % cap;
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
        self.deque_tail = (self.deque_tail + 1) % cap;
        self.sample_index = idx.wrapping_add(1);
        self.deque[self.deque_head].1
    }

    /// Process one stereo frame.
    ///
    /// While `bypassed == true`, `delay_buf` and the sliding peak deque continue to
    /// advance cleanly with live dry audio (`0.0` peak pushed), keeping latency constant
    /// for PDC and preventing stale audio bursts when un-bypassing.
    pub fn process(&mut self, input: [f32; 2], bypassed: bool) -> [f32; 2] {
        self.has_processed = true;
        let dry_in = sanitize_frame(input);
        let delayed_dry = self.delay_buf[self.write_idx];
        self.delay_buf[self.write_idx] = dry_in;
        self.write_idx += 1;
        if self.write_idx >= self.lookahead {
            self.write_idx = 0;
        }

        let ceiling_db = self.ceiling_db.next();
        let ceiling_lin = db_to_lin_f64(ceiling_db);
        let drive_lin = db_to_lin_f64(self.drive_db.next());
        let mix = self.mix.next();

        if bypassed && !self.was_active {
            let _ = self.push_window_peak(0.0);
            self.bypass_wet.snap(0.0);
            self.gr_db = 0.0;
            self.current_gr_db = 0.0;
            return delayed_dry;
        }

        self.bypass_wet.set_target(if bypassed { 0.0 } else { 1.0 });
        let bypass_w = self.bypass_wet.next();
        if bypassed && bypass_w <= 1e-4 {
            let _ = self.push_window_peak(0.0);
            self.bypass_wet.snap(0.0);
            self.gr_db = 0.0;
            self.current_gr_db = 0.0;
            self.was_active = false;
            return delayed_dry;
        }
        self.was_active = true;

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

        let gain = db_to_lin_f64(-self.gr_db);
        let wet_l = (del_l * gain).clamp(-ceiling_lin, ceiling_lin);
        let wet_r = (del_r * gain).clamp(-ceiling_lin, ceiling_lin);

        let eff_wet = mix * bypass_w;
        if eff_wet >= 1.0 - 1e-6 {
            [wet_l as f32, wet_r as f32]
        } else {
            let dry_mix = 1.0 - eff_wet;
            [
                (f64::from(delayed_dry[0]) * dry_mix + wet_l * eff_wet) as f32,
                (f64::from(delayed_dry[1]) * dry_mix + wet_r * eff_wet) as f32,
            ]
        }
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

            // Bypass clears GR and passes dry input untouched.
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
}
