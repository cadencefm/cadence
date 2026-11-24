//! Stereo quadrature-LFO BBD-style [`Chorus`] insert processor.
//!
//! Uses a true `(1.0 - mix) * dry + mix * wet` crossfade, smoothed `rate` /
//! `depth` / `width` / `mix`, cubic Hermite delay interpolation, [`TptOnePoleLpf`]
//! warmth with denormal flushing, and click-free bypass crossfade.

use crate::dsp::biquad::TptOnePoleLpf;
use crate::dsp::delay::DelayLine;
use crate::dsp::math::{
    mix_stereo_f64, sanitize_clamp_f64, sanitize_frame, sanitize_sample_f64, sanitize_sample_rate,
    BypassFader, BypassStep, Smoother,
};
use crate::mixer::ChorusPatch;
use std::f64::consts::{FRAC_PI_2, TAU};

const MAX_CHORUS_SECONDS: f64 = 0.064;
const BBD_TONE_HZ: f64 = 8_500.0;

/// Evaluate primary stereo quadrature LFO channels `(L, R)` at normalized cycle `phase_cycles`
/// and stereo spread `width ∈ [0.0, 1.0]` (0° mono -> 90° quadrature wide).
///
/// Single source of truth shared between [`QuadratureLfo`] DSP and UI chorus visualizers.
#[inline]
pub fn eval_quadrature(phase_cycles: f64, width: f64) -> [f64; 2] {
    let a = sanitize_sample_f64(phase_cycles) * TAU;
    let spread = sanitize_sample_f64(width).clamp(0.0, 1.0) * FRAC_PI_2;
    [a.sin(), (a + spread).sin()]
}

/// Dual-voice stereo quadrature LFO oscillator (`phase ∈ [0, 1)`).
#[derive(Clone, Copy, Debug)]
pub struct QuadratureLfo {
    sample_rate: f64,
    phase: f64,
}

impl QuadratureLfo {
    pub fn new(sample_rate: f64) -> Self {
        Self {
            sample_rate: sanitize_sample_rate(sample_rate),
            phase: 0.0,
        }
    }

    #[inline]
    pub fn reset(&mut self) {
        self.phase = 0.0;
    }

    #[inline]
    pub fn phase(&self) -> f64 {
        self.phase
    }

    /// Advance by one sample and return `(voice1_lr, voice2_lr)` in `[-1.0, 1.0]`.
    #[inline]
    pub fn next(&mut self, rate_hz: f64, width: f64) -> ([f64; 2], [f64; 2]) {
        let a = self.phase * TAU;
        let spread = sanitize_sample_f64(width).clamp(0.0, 1.0) * FRAC_PI_2;
        let v1 = [a.sin(), (a + spread).sin()];
        let v2 = [-v1[0], -v1[1]];
        let inc = sanitize_sample_f64(rate_hz).clamp(0.0, 40.0) / self.sample_rate;
        self.phase = (self.phase + inc).rem_euclid(1.0);
        (v1, v2)
    }
}

/// Stereo BBD-style dual-voice quadrature chorus with cubic Hermite delay reads,
/// true `(1.0 - mix) * dry + mix * wet` law, smooth parameter slewing, and
/// click-free bypass crossfade. Pre-allocated in `new`; zero RT allocations.
pub struct Chorus {
    sample_rate: f64,
    line: DelayLine,
    lfo: QuadratureLfo,
    rate_hz: Smoother,
    depth: Smoother,
    width: Smoother,
    mix: Smoother,
    bypass: BypassFader,
    tone_lpf: TptOnePoleLpf,
    last_patch: ChorusPatch,
    has_processed: bool,
}

impl Chorus {
    pub fn new(sample_rate: f64) -> Self {
        let sr = sanitize_sample_rate(sample_rate);
        let cap = ((sr * MAX_CHORUS_SECONDS).ceil() as usize).max(256);
        let default = ChorusPatch::default();
        let mut ch = Self {
            sample_rate: sr,
            line: DelayLine::new(cap),
            lfo: QuadratureLfo::new(sr),
            rate_hz: Smoother::new(f64::from(default.rate_hz), 5.0, sr),
            depth: Smoother::new(f64::from(default.depth), 5.0, sr),
            width: Smoother::new(f64::from(default.width), 5.0, sr),
            mix: Smoother::new(f64::from(default.mix), 5.0, sr),
            bypass: BypassFader::new(sr),
            tone_lpf: TptOnePoleLpf::new(sr, BBD_TONE_HZ),
            last_patch: default,
            has_processed: false,
        };
        ch.set_parameters(default);
        ch
    }

    #[cfg(test)]
    pub(crate) fn is_idle(&self) -> bool {
        self.bypass.is_idle()
    }

    pub fn reset(&mut self) {
        self.line.reset();
        self.lfo.reset();
        self.tone_lpf.reset();
        self.rate_hz.snap(self.rate_hz.target());
        self.depth.snap(self.depth.target());
        self.width.snap(self.width.target());
        self.mix.snap(self.mix.target());
        self.bypass.reset();
        self.has_processed = false;
    }

    pub fn set_parameters(&mut self, patch: ChorusPatch) {
        let def = ChorusPatch::default();
        let rate = sanitize_clamp_f64(f64::from(patch.rate_hz), 0.05, 6.0, f64::from(def.rate_hz));
        let depth = sanitize_clamp_f64(f64::from(patch.depth), 0.0, 1.0, f64::from(def.depth));
        let width = sanitize_clamp_f64(f64::from(patch.width), 0.0, 1.0, f64::from(def.width));
        let mix = sanitize_clamp_f64(f64::from(patch.mix), 0.0, 1.0, f64::from(def.mix));
        let sanitized = ChorusPatch {
            rate_hz: rate as f32,
            depth: depth as f32,
            width: width as f32,
            mix: mix as f32,
        };
        if self.has_processed && sanitized == self.last_patch {
            return;
        }
        self.last_patch = sanitized;
        if !self.has_processed {
            self.rate_hz.snap(rate);
            self.depth.snap(depth);
            self.width.snap(width);
            self.mix.snap(mix);
        } else {
            self.rate_hz.set_target(rate);
            self.depth.set_target(depth);
            self.width.set_target(width);
            self.mix.set_target(mix);
        }
    }

    pub fn process(&mut self, input: [f32; 2], bypassed: bool) -> [f32; 2] {
        self.has_processed = true;
        let dry = sanitize_frame(input);
        let bypass_w = match self.bypass.step(bypassed) {
            BypassStep::Idle => return dry,
            BypassStep::SettledBypass => {
                // O(1) validity reset prevents stale bursts when un-bypassed later.
                self.line.reset();
                self.tone_lpf.reset();
                self.bypass.mark_idle();
                return dry;
            }
            BypassStep::Active(w) => w,
        };

        self.line.write(dry);
        let rate = self.rate_hz.next();
        let depth = self.depth.next();
        let width = self.width.next();
        let mix = self.mix.next();
        let (v1, v2) = self.lfo.next(rate, width);

        let base_ms = 7.5;
        let swing_ms = 5.5 * depth;
        let ms_to_samp = self.sample_rate * 0.001;

        let d_l1 = (base_ms + v1[0] * swing_ms) * ms_to_samp;
        let d_l2 = (base_ms + 3.2 + v2[0] * swing_ms * 0.7) * ms_to_samp;
        let d_r1 = (base_ms + v1[1] * swing_ms) * ms_to_samp;
        let d_r2 = (base_ms + 3.2 + v2[1] * swing_ms * 0.7) * ms_to_samp;

        let wet_l = 0.5
            * f64::from(
                self.line.read_cubic_channel(d_l1, 0) + self.line.read_cubic_channel(d_l2, 0),
            );
        let wet_r = 0.5
            * f64::from(
                self.line.read_cubic_channel(d_r1, 1) + self.line.read_cubic_channel(d_r2, 1),
            );
        let toned = self.tone_lpf.process_stereo([wet_l, wet_r]);

        // True (1 - mix) * dry + mix * wet law (zero dry bleed at 100% wet, flat bass at 50% mix!).
        let out = mix_stereo_f64(
            [f64::from(dry[0]), f64::from(dry[1])],
            toned,
            mix * bypass_w,
        );
        [out[0] as f32, out[1] as f32]
    }
}

impl crate::dsp::StereoEffect for Chorus {
    #[inline]
    fn process(&mut self, input: [f32; 2], bypassed: bool) -> [f32; 2] {
        Chorus::process(self, input, bypassed)
    }

    #[inline]
    fn reset(&mut self) {
        Chorus::reset(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chorus_mix_100_has_zero_dry_bleed_and_mix_50_has_flat_bass() {
        for sr in [44_100.0, 48_000.0, 96_000.0] {
            // 1. At mix = 1.0, an impulse at frame 0 must produce EXACTLY 0.0 at frame 0
            //    (zero dry bleed; earliest wet tap is >= 2 ms = 88+ samples away!).
            let mut ch = Chorus::new(sr);
            ch.set_parameters(ChorusPatch {
                rate_hz: 0.8,
                depth: 0.5,
                width: 1.0,
                mix: 1.0,
            });
            let first = ch.process([1.0, 1.0], false);
            assert!(
                first[0].abs() < 1e-6 && first[1].abs() < 1e-6,
                "sr={sr}: mix=1.0 leaked dry signal on frame 0: {first:?}"
            );

            // 2. At mix = 0.5 and mix = 1.0, sub-bass / DC gain stays flat (~0 dB, <= +0.1 dB).
            for test_mix in [0.5_f32, 1.0_f32] {
                let mut bass_ch = Chorus::new(sr);
                bass_ch.set_parameters(ChorusPatch {
                    rate_hz: 0.3,
                    depth: 0.25,
                    width: 0.5,
                    mix: test_mix,
                });
                let warmup = (sr * 0.2) as usize;
                let measure = (sr * 0.25) as usize;
                let mut in_sq = 0.0;
                let mut out_sq = 0.0;
                for i in 0..(warmup + measure) {
                    // 10 Hz stays well below the 55 Hz comb notch of the ~9 ms chorus delay.
                    let s = (TAU * 10.0 * i as f64 / sr).sin() as f32 * 0.5;
                    let y = bass_ch.process([s, s], false);
                    assert!(y[0].is_finite() && y[1].is_finite());
                    if i >= warmup {
                        in_sq += f64::from(s) * f64::from(s);
                        out_sq += f64::from(y[0]) * f64::from(y[0]);
                    }
                }
                let gain_db = crate::dsp::lin_to_db_f64((out_sq / in_sq).sqrt());
                assert!(
                    (-1.0..=0.15).contains(&gain_db),
                    "sr={sr} mix={test_mix}: 10 Hz bass gain={gain_db:+.2} dB (expected ~0 dB)"
                );
            }
        }
    }

    #[test]
    fn chorus_widens_mono_signal_cleanly() {
        let mut ch = Chorus::new(48_000.0);
        ch.set_parameters(ChorusPatch {
            rate_hz: 1.2,
            depth: 0.7,
            width: 1.0,
            mix: 0.6,
        });
        let mut diff_energy = 0.0_f64;
        for i in 0..8000 {
            let s = (TAU * 220.0 * i as f64 / 48_000.0).sin() as f32 * 0.5;
            let out = ch.process([s, s], false);
            assert!(out[0].is_finite() && out[1].is_finite());
            if i > 1000 {
                diff_energy += f64::from(out[0] - out[1]).powi(2);
            }
        }
        assert!(diff_energy > 1.0);
    }

    #[test]
    fn chorus_bypass_idles_cleanly_without_stale_burst_and_sanitizes_nan_params() {
        for sr in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            let mut ch = Chorus::new(sr);
            ch.set_parameters(ChorusPatch {
                rate_hz: 1.0,
                depth: 0.6,
                width: 1.0,
                mix: 1.0,
            });
            for _ in 0..512 {
                let _ = ch.process([0.9, -0.9], false);
            }
            assert!(!ch.is_idle());

            let fade_frames = (sr * 0.03) as usize;
            for _ in 0..fade_frames {
                let _ = ch.process([0.2, -0.2], true);
            }
            assert!(ch.is_idle(), "sr={sr}: bypassed chorus must idle");
            assert_eq!(ch.process([0.4, -0.3], true), [0.4, -0.3]);
            assert!(ch.is_idle());

            // Un-bypassing with silence produces zero stale chorus burst (line was O(1) reset).
            let check_frames = (sr * 0.03) as usize;
            for _ in 0..check_frames {
                assert_eq!(ch.process([0.0, 0.0], false), [0.0, 0.0]);
            }

            // NaN parameters sanitize cleanly.
            ch.set_parameters(ChorusPatch {
                rate_hz: f32::NAN,
                depth: f32::NAN,
                width: f32::NAN,
                mix: f32::NAN,
            });
            let y = ch.process([0.5, -0.5], false);
            assert!(y[0].is_finite() && y[1].is_finite());
        }
    }

    #[test]
    fn quadrature_lfo_voice_two_matches_exact_antiphase_across_sample_rates() {
        use std::f64::consts::PI;

        for sr in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            for width in [0.0, 0.35, 0.7, 1.0] {
                let mut lfo = QuadratureLfo::new(sr);
                let spread = width * FRAC_PI_2;
                for _ in 0..512 {
                    let phase = lfo.phase();
                    let a = phase * TAU;
                    let expected_v1 = eval_quadrature(phase, width);
                    let expected_v2 = [(a + PI).sin(), (a + PI + spread).sin()];
                    let (v1, v2) = lfo.next(1.75, width);
                    assert_eq!(v1, expected_v1);
                    assert_eq!(v2, [-v1[0], -v1[1]]);
                    assert!(
                        (v2[0] - expected_v2[0]).abs() < 1e-14
                            && (v2[1] - expected_v2[1]).abs() < 1e-14,
                        "sr={sr} width={width}: v2={v2:?} vs expected={expected_v2:?}"
                    );
                }
            }
        }
    }
}
