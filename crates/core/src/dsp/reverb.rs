//! Zero-delay 96-mode ambient-air modal room [`Reverb`] insert processor
//! and exact physical `-60 dB` RT60 / air-absorption envelope math.
//!
//! Composes [`AmbientModalRoom`] directly with zero delay buffers and zero
//! real-time heap allocations. All modal room parameters (`size`,
//! `decay_seconds`, `damping_hz`, `diffusion`) smooth continuously and update
//! eigenmode coefficients at a sample-rate-invariant `1.5 kHz` control cadence
//! without clearing modal state, enabling click-free live automation alongside
//! natural bypassed tail drainage.

use crate::dsp::cubic::room::{
    room_mode_rt60, AmbientModalRoom, ROOM_DEFAULT_DAMPING_HZ, ROOM_DEFAULT_DECAY_SECONDS,
    ROOM_DEFAULT_DIFFUSION, ROOM_DEFAULT_SIZE, ROOM_MAX_DECAY_SECONDS, ROOM_MIN_DAMPING_HZ,
    ROOM_MIN_DECAY_SECONDS,
};
use crate::dsp::gain::stereo_ms_width_uncorrelated;
use crate::dsp::math::{
    clamp_freq_hz, control_stride_last_tick, control_stride_mask, mix_stereo_f32,
    one_pole_coeff_ms, sanitize_clamp_f64, sanitize_frame, sanitize_sample_rate, BypassFader,
    BypassStep, LogSmoother, Smoother, NYQUIST_SAFE_RATIO, TAIL_SILENCE_FLOOR_F32,
};
use crate::mixer::ReverbPatch;

/// Stored quadratic modal energy floor below which the room tail is treated as quiet.
const ROOM_QUIET_ENERGY: f64 = 1e-12;

/// High-frequency reference point (Hz) for the physical air-absorption envelope curve.
const HF_ENVELOPE_REF_HZ: f64 = 8_000.0;

/// Control-rate cadence (Hz) for re-evaluating modal room rotation, decay, and diffusion tables.
const ROOM_CTRL_HZ: f64 = 1_500.0;

/// Parameter smoothing time constant (ms) for room controls and stereo mix/width.
const PARAM_SMOOTH_MS: f32 = 5.0;

/// Nominal studio wet readout gain at canonical `decay_seconds = 2.65 s`.
const WET_GAIN_REF: f64 = 16.0;

/// Passive upper ceiling on wet readout gain so even undiffused single-mode resonance
/// at full-scale stereo drive remains strictly non-clipping (`< 1.0` peak).
const WET_GAIN_MAX: f64 = 20.0;

/// Exact perceptual RT60 decay amplitude envelope (`10^(-3 * t / decay_seconds)`),
/// reaching `-60 dB` (`0.001`) at `t_sec == decay_seconds`.
#[inline]
pub fn reverb_rt60_envelope(decay_seconds: f32, t_sec: f32) -> f32 {
    if !t_sec.is_finite() || t_sec <= 0.0 {
        return 1.0;
    }
    let decay = sanitize_clamp_f64(
        f64::from(decay_seconds),
        0.05,
        60.0,
        ROOM_DEFAULT_DECAY_SECONDS,
    );
    10.0_f64
        .powf(-3.0 * f64::from(t_sec) / decay)
        .clamp(0.0, 1.0) as f32
}

/// Physical high-frequency air-absorption RT60 envelope at `8000 Hz`, matching
/// `RT60(8000) = decay_seconds / (1 + (8000 / damping_hz)^1.45)`.
#[inline]
pub fn reverb_hf_rt60_envelope(decay_seconds: f32, damping_hz: f32, t_sec: f32) -> f32 {
    if !t_sec.is_finite() || t_sec <= 0.0 {
        return 1.0;
    }
    let decay = sanitize_clamp_f64(
        f64::from(decay_seconds),
        0.05,
        60.0,
        ROOM_DEFAULT_DECAY_SECONDS,
    );
    let damp = sanitize_clamp_f64(
        f64::from(damping_hz),
        ROOM_MIN_DAMPING_HZ,
        48_000.0,
        ROOM_DEFAULT_DAMPING_HZ,
    );
    let rt_hf = room_mode_rt60(decay, damp, HF_ENVELOPE_REF_HZ);
    10.0_f64
        .powf(-3.0 * f64::from(t_sec) / rt_hf)
        .clamp(0.0, 1.0) as f32
}

/// Decay-compensated wet readout gain matching studio send/insert levels across
/// `0.2..=12.0 s` while preserving strict passivity and non-clipping headroom.
#[inline]
fn wet_makeup_gain(decay_seconds: f64) -> f64 {
    let ratio = (decay_seconds / ROOM_DEFAULT_DECAY_SECONDS).max(0.0);
    (WET_GAIN_REF * ratio.sqrt()).min(WET_GAIN_MAX)
}

/// Stereo zero-delay 96-mode ambient-air modal room reverb with smooth size,
/// decay, log-domain air-absorption damping, Even/Odd diffusion, uncorrelated
/// Mid/Side width, and click-free tail-draining bypass crossfade.
pub struct Reverb {
    sample_rate: f64,
    room: Box<AmbientModalRoom>,
    size: Smoother,
    decay_seconds: Smoother,
    damping_hz: LogSmoother,
    damping_coeff: f64,
    diffusion: Smoother,
    mix: Smoother,
    width: Smoother,
    cached_size_bits: u64,
    cached_decay_bits: u64,
    cached_damping_bits: u64,
    cached_diffusion_bits: u64,
    bypass: BypassFader,
    target: ReverbPatch,
    ctrl_count: u32,
    ctrl_mask: u32,
    room_active: bool,
    has_processed: bool,
}

impl Reverb {
    pub fn new(sample_rate: u32) -> Self {
        let sr = sanitize_sample_rate(f64::from(sample_rate.clamp(8_000, 384_000)));
        let default = ReverbPatch::default();
        let init_damp = clamp_freq_hz(
            f64::from(default.damping_hz),
            ROOM_MIN_DAMPING_HZ,
            sr,
            f64::from(default.damping_hz),
        );
        let room = AmbientModalRoom::new_boxed(sr);
        let mut effect = Self {
            sample_rate: sr,
            room,
            size: Smoother::new(f64::from(default.size), PARAM_SMOOTH_MS, sr),
            decay_seconds: Smoother::new(f64::from(default.decay_seconds), PARAM_SMOOTH_MS, sr),
            damping_hz: LogSmoother::new_clamped(
                init_damp,
                ROOM_MIN_DAMPING_HZ,
                sr * NYQUIST_SAFE_RATIO,
            ),
            damping_coeff: one_pole_coeff_ms(PARAM_SMOOTH_MS, sr),
            diffusion: Smoother::new(f64::from(default.diffusion), PARAM_SMOOTH_MS, sr),
            mix: Smoother::new(f64::from(default.mix), PARAM_SMOOTH_MS, sr),
            width: Smoother::new(f64::from(default.width), PARAM_SMOOTH_MS, sr),
            // Track the canonical parameters loaded by `AmbientModalRoom::new_boxed`.
            cached_size_bits: ROOM_DEFAULT_SIZE.to_bits(),
            cached_decay_bits: ROOM_DEFAULT_DECAY_SECONDS.to_bits(),
            cached_damping_bits: ROOM_DEFAULT_DAMPING_HZ.to_bits(),
            cached_diffusion_bits: ROOM_DEFAULT_DIFFUSION.to_bits(),
            bypass: BypassFader::new(sr),
            target: default,
            ctrl_count: 0,
            ctrl_mask: control_stride_mask(sr, ROOM_CTRL_HZ),
            room_active: false,
            has_processed: false,
        };
        effect.set_parameters(default);
        effect
    }

    #[cfg(test)]
    pub(crate) fn is_idle(&self) -> bool {
        self.bypass.is_idle()
    }

    #[cfg(test)]
    pub(crate) fn damping_hz_current(&self) -> f64 {
        self.damping_hz.current()
    }

    #[cfg(test)]
    pub(crate) fn room_energy(&self) -> f64 {
        self.room.energy()
    }

    #[inline]
    fn sync_room_if_changed(&mut self, size: f64, decay: f64, damp: f64, diffusion: f64) {
        let size_bits = size.to_bits();
        let decay_bits = decay.to_bits();
        let damp_bits = damp.to_bits();
        let diff_bits = diffusion.to_bits();
        if size_bits != self.cached_size_bits
            || decay_bits != self.cached_decay_bits
            || damp_bits != self.cached_damping_bits
            || diff_bits != self.cached_diffusion_bits
        {
            self.cached_size_bits = size_bits;
            self.cached_decay_bits = decay_bits;
            self.cached_damping_bits = damp_bits;
            self.cached_diffusion_bits = diff_bits;
            self.room.configure(size, decay, damp, diffusion);
        }
    }

    pub fn reset(&mut self) {
        self.room.reset();
        self.room_active = false;
        self.bypass.reset();
        self.ctrl_count = 0;
        self.has_processed = false;
        self.set_parameters(self.target);
    }

    pub fn set_parameters(&mut self, patch: ReverbPatch) {
        let def = ReverbPatch::default();
        let size = sanitize_clamp_f64(f64::from(patch.size), 0.0, 1.0, f64::from(def.size));
        let decay = sanitize_clamp_f64(
            f64::from(patch.decay_seconds),
            ROOM_MIN_DECAY_SECONDS,
            ROOM_MAX_DECAY_SECONDS,
            f64::from(def.decay_seconds),
        );
        let damp_hz = clamp_freq_hz(
            f64::from(patch.damping_hz),
            ROOM_MIN_DAMPING_HZ,
            self.sample_rate,
            f64::from(def.damping_hz),
        );
        let diffusion = sanitize_clamp_f64(
            f64::from(patch.diffusion),
            0.0,
            1.0,
            f64::from(def.diffusion),
        );
        let mix = sanitize_clamp_f64(f64::from(patch.mix), 0.0, 1.0, f64::from(def.mix));
        let width = sanitize_clamp_f64(f64::from(patch.width), 0.0, 1.0, f64::from(def.width));

        let sanitized = ReverbPatch {
            size: size as f32,
            decay_seconds: decay as f32,
            damping_hz: damp_hz as f32,
            diffusion: diffusion as f32,
            mix: mix as f32,
            width: width as f32,
        };
        if self.has_processed && self.target == sanitized {
            return;
        }
        self.target = sanitized;

        if !self.has_processed {
            self.size.snap(size);
            self.decay_seconds.snap(decay);
            self.damping_hz.snap(damp_hz);
            self.diffusion.snap(diffusion);
            self.mix.snap(mix);
            self.width.snap(width);
            self.sync_room_if_changed(
                self.size.current(),
                self.decay_seconds.current(),
                self.damping_hz.current(),
                self.diffusion.current(),
            );
        } else {
            self.size.set_target(size);
            self.decay_seconds.set_target(decay);
            self.damping_hz.set_target(damp_hz);
            self.diffusion.set_target(diffusion);
            self.mix.set_target(mix);
            self.width.set_target(width);
        }
    }

    #[inline]
    pub fn latency_frames(&self) -> usize {
        0
    }

    #[inline]
    pub fn is_quiet(&self, bypassed: bool) -> bool {
        self.room.energy() < ROOM_QUIET_ENERGY && self.bypass.is_settled_for(bypassed)
    }

    #[inline]
    pub fn advance_silent_frames(&mut self, frames: usize, bypassed: bool) {
        if frames == 0 {
            return;
        }
        self.has_processed = true;
        self.bypass.advance_silent(bypassed);
        if let Some(last_tick) = control_stride_last_tick(self.ctrl_count, self.ctrl_mask, frames) {
            let lead = last_tick + 1;
            self.size.advance_silent(lead);
            self.decay_seconds.advance_silent(lead);
            self.damping_hz.advance_silent(self.damping_coeff, lead);
            self.diffusion.advance_silent(lead);
            self.sync_room_if_changed(
                self.size.current(),
                self.decay_seconds.current(),
                self.damping_hz.current(),
                self.diffusion.current(),
            );
            let rem = frames - lead;
            if rem > 0 {
                self.size.advance_silent(rem);
                self.decay_seconds.advance_silent(rem);
                self.damping_hz.advance_silent(self.damping_coeff, rem);
                self.diffusion.advance_silent(rem);
            }
        } else {
            self.size.advance_silent(frames);
            self.decay_seconds.advance_silent(frames);
            self.damping_hz.advance_silent(self.damping_coeff, frames);
            self.diffusion.advance_silent(frames);
        }
        self.ctrl_count = self.ctrl_count.wrapping_add(frames as u32) & self.ctrl_mask;
        self.mix.advance_silent(frames);
        self.width.advance_silent(frames);
    }

    pub fn process(&mut self, input: [f32; 2], bypassed: bool) -> [f32; 2] {
        self.has_processed = true;
        let dry = sanitize_frame(input);
        let (bypass_w, bypass_settled) = match self.bypass.step(bypassed) {
            BypassStep::Idle => {
                self.advance_silent_frames(1, true);
                return dry;
            }
            BypassStep::SettledBypass => (0.0, true),
            BypassStep::Active(w) => (w, false),
        };

        let size = self.size.next();
        let decay = self.decay_seconds.next();
        let damp = self.damping_hz.step(self.damping_coeff);
        let diffusion = self.diffusion.next();
        if self.ctrl_count == 0 {
            self.sync_room_if_changed(size, decay, damp, diffusion);
        }
        self.ctrl_count = self.ctrl_count.wrapping_add(1) & self.ctrl_mask;

        let mix = self.mix.next();
        let width = self.width.next() as f32;

        // Age modal tails naturally when bypassed by feeding silence into the room.
        let feed = if bypassed { [0.0; 2] } else { dry };
        let feed_silent =
            feed[0].abs() < TAIL_SILENCE_FLOOR_F32 && feed[1].abs() < TAIL_SILENCE_FLOOR_F32;

        let wet_raw = if self.room_active || !feed_silent {
            self.room_active = true;
            let out = self.room.step([f64::from(feed[0]), f64::from(feed[1])]);
            if feed_silent && self.room.energy() < ROOM_QUIET_ENERGY {
                self.room.reset();
                self.room_active = false;
            }
            out
        } else {
            [0.0, 0.0]
        };

        if bypass_settled {
            if !self.room_active {
                self.bypass.mark_idle();
            }
            return dry;
        }

        let wet_gain = wet_makeup_gain(decay);
        let wet = [
            (wet_raw[0] * wet_gain) as f32,
            (wet_raw[1] * wet_gain) as f32,
        ];
        let wet_wide = stereo_ms_width_uncorrelated(wet, width);
        mix_stereo_f32(dry, wet_wide, (mix * bypass_w) as f32)
    }

    #[inline]
    pub fn process_block(&mut self, buf: &mut [[f32; 2]], bypassed: bool) {
        if buf.is_empty() {
            return;
        }
        self.has_processed = true;
        if self.bypass.check_idle_bypass(bypassed) {
            self.advance_silent_frames(buf.len(), true);
            for frame in buf.iter_mut() {
                *frame = sanitize_frame(*frame);
            }
            return;
        }
        for frame in buf.iter_mut() {
            *frame = self.process(*frame, bypassed);
        }
    }
}

impl crate::dsp::StereoEffect for Reverb {
    #[inline]
    fn process(&mut self, input: [f32; 2], bypassed: bool) -> [f32; 2] {
        Reverb::process(self, input, bypassed)
    }

    #[inline]
    fn process_block(&mut self, buf: &mut [[f32; 2]], bypassed: bool) {
        Reverb::process_block(self, buf, bypassed);
    }

    #[inline]
    fn reset(&mut self) {
        Reverb::reset(self);
    }

    #[inline]
    fn latency_frames(&self) -> usize {
        Reverb::latency_frames(self)
    }

    #[inline]
    fn is_quiet(&self, bypassed: bool) -> bool {
        Reverb::is_quiet(self, bypassed)
    }

    #[inline]
    fn advance_silent_frames(&mut self, frames: usize, bypassed: bool) {
        Reverb::advance_silent_frames(self, frames, bypassed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    #[test]
    fn rt60_and_hf_air_absorption_envelopes_hit_minus_60_db_and_respect_damping() {
        for decay in [0.5_f32, 2.65, 8.0] {
            assert_eq!(reverb_rt60_envelope(decay, 0.0), 1.0);
            assert_eq!(reverb_rt60_envelope(decay, -1.0), 1.0);
            assert_eq!(reverb_rt60_envelope(decay, f32::NAN), 1.0);
            let at_rt60 = reverb_rt60_envelope(decay, decay);
            let db = crate::dsp::lin_to_db(at_rt60);
            assert!(
                (db + 60.0).abs() < 0.05,
                "decay={decay}: expected -60.0 dB at t=decay, got {db:.2} dB"
            );

            for damp in [800.0_f32, 3_600.0, 12_000.0] {
                assert_eq!(reverb_hf_rt60_envelope(decay, damp, 0.0), 1.0);
                let rt_hf = room_mode_rt60(f64::from(decay), f64::from(damp), HF_ENVELOPE_REF_HZ);
                let at_hf = reverb_hf_rt60_envelope(decay, damp, rt_hf as f32);
                let hf_db = crate::dsp::lin_to_db(at_hf);
                assert!(
                    (hf_db + 60.0).abs() < 0.05,
                    "decay={decay} damp={damp}: expected -60.0 dB at t=rt_hf ({rt_hf:.4}s), got {hf_db:.2} dB"
                );
                let mid_t = (0.5 * rt_hf) as f32;
                assert!(
                    reverb_hf_rt60_envelope(decay, damp, mid_t)
                        < reverb_rt60_envelope(decay, mid_t),
                    "decay={decay} damp={damp}: HF air-absorption must decay faster than DC"
                );
            }

            let t_probe = 0.25_f32;
            let e_dark = reverb_hf_rt60_envelope(decay, 800.0, t_probe);
            let e_mid = reverb_hf_rt60_envelope(decay, 3_600.0, t_probe);
            let e_bright = reverb_hf_rt60_envelope(decay, 12_000.0, t_probe);
            assert!(
                e_dark < e_mid && e_mid < e_bright,
                "decay={decay}: HF envelope must rise monotonically with damping_hz"
            );
        }
    }

    #[test]
    fn modal_reverb_tail_decays_with_rt60_and_zero_latency_across_sample_rates() {
        for sr in [44_100u32, 48_000, 96_000] {
            let sr_f = f64::from(sr);
            let mut short = Reverb::new(sr);
            assert_eq!(short.latency_frames(), 0);
            short.set_parameters(ReverbPatch {
                decay_seconds: 0.4,
                damping_hz: 12_000.0,
                mix: 1.0,
                ..ReverbPatch::default()
            });
            let mut long = Reverb::new(sr);
            long.set_parameters(ReverbPatch {
                decay_seconds: 3.0,
                damping_hz: 12_000.0,
                mix: 1.0,
                ..ReverbPatch::default()
            });

            let burst_frames = (sr_f * 0.020) as usize;
            let total = sr as usize;
            let mut e_short = 0.0_f64;
            let mut e_long = 0.0_f64;
            let mut stereo_diff = 0.0_f64;
            for i in 0..total {
                let x = if i < burst_frames {
                    let t = i as f64 / sr_f;
                    let env = (-t / 0.008).exp();
                    let s = (env * ((TAU * 420.0 * t).sin() + 0.5 * (TAU * 1_180.0 * t).cos()))
                        as f32
                        * 0.65;
                    [s, 0.0]
                } else {
                    [0.0, 0.0]
                };
                let ys = short.process(x, false);
                let yl = long.process(x, false);
                assert!(ys[0].is_finite() && ys[1].is_finite());
                assert!(yl[0].is_finite() && yl[1].is_finite());
                assert!(yl[0].abs() < 1.0 && yl[1].abs() < 1.0);
                if i >= total / 2 {
                    e_short += f64::from(ys[0]).powi(2) + f64::from(ys[1]).powi(2);
                    e_long += f64::from(yl[0]).powi(2) + f64::from(yl[1]).powi(2);
                    stereo_diff += f64::from(yl[0] - yl[1]).powi(2);
                }
            }
            assert!(
                e_long > e_short * 20.0,
                "sr={sr}: long={e_long:.3e}, short={e_short:.3e}"
            );
            assert!(
                stereo_diff > 1e-5,
                "sr={sr}: modal reverb must produce diffuse stereo width (diff={stereo_diff:.3e})"
            );
        }
    }

    #[test]
    fn air_absorption_damping_diffusion_and_size_shape_modal_response() {
        let sr = 48_000u32;
        let sr_f = f64::from(sr);

        // 1. Air-absorption damping selectively attenuates high-frequency tails.
        let measure_band_tail = |damping_hz: f32, tone_hz: f64| -> f64 {
            let mut verb = Reverb::new(sr);
            verb.set_parameters(ReverbPatch {
                size: 0.5,
                decay_seconds: 2.0,
                damping_hz,
                diffusion: 0.5,
                mix: 1.0,
                width: 1.0,
            });
            let excite = (sr_f * 0.015) as usize;
            let tail_start = (sr_f * 0.08) as usize;
            let total = (sr_f * 0.25) as usize;
            let mut energy = 0.0_f64;
            for i in 0..total {
                let x = if i < excite {
                    let s = (TAU * tone_hz * i as f64 / sr_f).sin() as f32 * 0.8;
                    [s, s]
                } else {
                    [0.0, 0.0]
                };
                let y = verb.process(x, false);
                if i >= tail_start {
                    energy += f64::from(y[0]).powi(2) + f64::from(y[1]).powi(2);
                }
            }
            energy
        };
        let hf_bright = measure_band_tail(14_000.0, 5_000.0);
        let hf_dark = measure_band_tail(800.0, 5_000.0);
        assert!(
            hf_bright > hf_dark * 10.0,
            "air absorption must damp 5 kHz tail strongly: bright={hf_bright:.3e}, dark={hf_dark:.3e}"
        );
        let lf_bright = measure_band_tail(14_000.0, 120.0);
        let lf_dark = measure_band_tail(800.0, 120.0);
        assert!(
            lf_dark > lf_bright * 0.4,
            "low-frequency 120 Hz tail should remain intact under HF damping: bright={lf_bright:.3e}, dark={lf_dark:.3e}"
        );

        // 2. Pair diffusion smoothly reshapes Even/Odd modal coupling without instability.
        let run_diffusion = |diffusion: f32| -> Vec<[f32; 2]> {
            let mut verb = Reverb::new(sr);
            verb.set_parameters(ReverbPatch {
                size: 0.5,
                decay_seconds: 2.0,
                damping_hz: 6_000.0,
                diffusion,
                mix: 1.0,
                width: 1.0,
            });
            let total = (sr_f * 0.10) as usize;
            let mut out = Vec::with_capacity(total);
            for i in 0..total {
                let x = if i < 64 {
                    let t = i as f64 / sr_f;
                    let s =
                        ((TAU * 520.0 * t).sin() + 0.5 * (TAU * 1_640.0 * t).cos()) as f32 * 0.5;
                    [s, -s]
                } else {
                    [0.0, 0.0]
                };
                out.push(verb.process(x, false));
            }
            out
        };
        let dry_diff = run_diffusion(0.0);
        let wet_diff = run_diffusion(1.0);
        let diff_energy: f64 = dry_diff
            .iter()
            .zip(wet_diff.iter())
            .map(|(a, b)| f64::from(a[0] - b[0]).powi(2) + f64::from(a[1] - b[1]).powi(2))
            .sum();
        assert!(
            diff_energy > 1e-4,
            "diffusion 0.0 vs 1.0 must audibly alter modal coupling (diff_energy={diff_energy:.3e})"
        );

        // 3. Size scales modal frequencies inversely (smaller room => higher zero-crossing rate).
        let measure_zcr = |size: f32| -> usize {
            let mut verb = Reverb::new(sr);
            verb.set_parameters(ReverbPatch {
                size,
                decay_seconds: 2.0,
                damping_hz: 10_000.0,
                diffusion: 0.5,
                mix: 1.0,
                width: 1.0,
            });
            let total = (sr_f * 0.15) as usize;
            let mut prev = 0.0_f32;
            let mut crossings = 0usize;
            for i in 0..total {
                let x = if i < 12 { [0.9, 0.0] } else { [0.0, 0.0] };
                let y = verb.process(x, false);
                if i > 32 && ((y[0] >= 0.0) != (prev >= 0.0)) {
                    crossings += 1;
                }
                prev = y[0];
            }
            crossings
        };
        let zcr_small = measure_zcr(0.05);
        let zcr_large = measure_zcr(0.95);
        assert!(
            zcr_small > zcr_large,
            "smaller room must shift eigenmodes higher: small_zcr={zcr_small} <= large_zcr={zcr_large}"
        );
    }

    #[test]
    fn reverb_width_preserves_wet_rms_and_passivity_under_zero_input_and_steady_drive() {
        for sr in [44_100u32, 48_000, 96_000, 192_000] {
            let sr_f = f64::from(sr);
            let burst_frames = (sr_f * 0.004) as usize;
            let measure_rms = |width: f32| -> (f64, f32) {
                let mut verb = Reverb::new(sr);
                verb.set_parameters(ReverbPatch {
                    size: 0.5,
                    decay_seconds: 1.0,
                    damping_hz: 12_000.0,
                    diffusion: 0.5,
                    mix: 1.0,
                    width,
                });
                let total = (sr_f * 0.20) as usize;
                let mut energy = 0.0_f64;
                let mut max_lr_diff = 0.0_f32;
                for i in 0..total {
                    let x = if i < burst_frames {
                        let t = i as f64 / sr_f;
                        [((-t / 0.0015).exp()) as f32, 0.0]
                    } else {
                        [0.0, 0.0]
                    };
                    let y = verb.process(x, false);
                    energy += f64::from(y[0]).powi(2) + f64::from(y[1]).powi(2);
                    max_lr_diff = max_lr_diff.max((y[0] - y[1]).abs());
                }
                ((energy / (2.0 * total as f64)).sqrt(), max_lr_diff)
            };
            let (rms_wide, diff_wide) = measure_rms(1.0);
            let (rms_mono, diff_mono) = measure_rms(0.0);
            let drift_db = crate::dsp::lin_to_db_f64(rms_mono / rms_wide);
            assert!(
                drift_db.abs() < 0.75,
                "sr={sr}: width 1.0->0.0 wet RMS drifted by {drift_db:+.2} dB (wide={rms_wide:.5}, mono={rms_mono:.5})"
            );
            assert!(diff_wide > 1e-4, "sr={sr}: wide output must be stereo");
            assert!(
                diff_mono < 1e-6,
                "sr={sr}: width=0.0 must collapse cleanly to mono, diff={diff_mono}"
            );
        }

        // Zero-input strict passivity & steady-state non-clipping passivity.
        let sr = 48_000u32;
        let mut verb = Reverb::new(sr);
        verb.set_parameters(ReverbPatch {
            size: 0.5,
            decay_seconds: 2.65,
            damping_hz: 3_600.0,
            diffusion: 0.0,
            mix: 1.0,
            width: 1.0,
        });
        let mut in_sq = 0.0_f64;
        let mut out_sq = 0.0_f64;
        let mut peak = 0.0_f32;
        for i in 0..24_000 {
            let t = i as f64 / f64::from(sr);
            let s = ((TAU * 260.0 * t).sin() * 0.5
                + (TAU * 440.0 * t).cos() * 0.3
                + (TAU * 1_230.0 * t).sin() * 0.2) as f32;
            let y = verb.process([s, s], false);
            in_sq += 2.0 * f64::from(s).powi(2);
            out_sq += f64::from(y[0]).powi(2) + f64::from(y[1]).powi(2);
            peak = peak.max(y[0].abs().max(y[1].abs()));
        }
        assert!(peak < 1.0, "wet peak clipped under unit drive: {peak}");
        assert!(
            out_sq < in_sq,
            "steady-state reverb must remain strictly passive: out={out_sq:.4}, in={in_sq:.4}"
        );

        let mut prev_e = verb.room_energy();
        assert!(prev_e > ROOM_QUIET_ENERGY);
        for _ in 0..4_096 {
            let y = verb.process([0.0, 0.0], false);
            assert!(y[0].is_finite() && y[1].is_finite());
            let cur_e = verb.room_energy();
            assert!(
                cur_e < prev_e,
                "zero-input modal energy must strictly decrease: cur={cur_e} >= prev={prev_e}"
            );
            prev_e = cur_e;
        }
    }

    #[test]
    fn click_free_live_automation_bypass_drain_and_nan_sanitization() {
        // 1. Live parameter automation across all room controls stays click-free.
        let sr = 48_000u32;
        let mut live = Reverb::new(sr);
        live.set_parameters(ReverbPatch {
            size: 0.2,
            decay_seconds: 1.2,
            damping_hz: 1_200.0,
            diffusion: 0.1,
            mix: 1.0,
            width: 0.4,
        });
        for i in 0..4_800 {
            let t = i as f64 / f64::from(sr);
            let s = (TAU * 330.0 * t).sin() as f32 * 0.7;
            let _ = live.process([s, -s], false);
        }
        live.set_parameters(ReverbPatch {
            size: 0.9,
            decay_seconds: 6.5,
            damping_hz: 14_000.0,
            diffusion: 0.95,
            mix: 1.0,
            width: 1.0,
        });
        let mut prev = live.process([0.0, 0.0], false);
        let mut max_step = 0.0_f32;
        for _ in 0..4_800 {
            let cur = live.process([0.0, 0.0], false);
            assert!(cur[0].is_finite() && cur[1].is_finite());
            max_step = max_step
                .max((cur[0] - prev[0]).abs())
                .max((cur[1] - prev[1]).abs());
            prev = cur;
        }
        assert!(
            max_step < 0.05,
            "live modal parameter sweep produced click jump {max_step}"
        );

        // 2. Bypassed reverb drains naturally to idle across sample rates, then un-bypasses cleanly.
        for sr in [44_100u32, 48_000, 96_000, 192_000] {
            let mut verb = Reverb::new(sr);
            verb.set_parameters(ReverbPatch {
                size: 0.25,
                decay_seconds: 0.2,
                damping_hz: 8_000.0,
                diffusion: 0.5,
                mix: 1.0,
                width: 1.0,
            });
            let excite = (f64::from(sr) * 0.005) as usize;
            for _ in 0..excite {
                let _ = verb.process([0.9, -0.9], false);
            }
            assert!(!verb.is_idle());
            assert!(!verb.is_quiet(false));

            let drain_limit = (f64::from(sr) * 1.5) as usize;
            for _ in 0..drain_limit {
                let out = verb.process([0.25, -0.25], true);
                assert!(out[0].is_finite() && out[1].is_finite());
                if verb.is_idle() {
                    break;
                }
            }
            assert!(
                verb.is_idle() && verb.is_quiet(true),
                "sr={sr}: bypassed reverb must drain to idle & quiet"
            );
            // Once idle, un-bypassing with silence must produce exact zero (zero stale tail).
            for _ in 0..256 {
                assert_eq!(verb.process([0.0, 0.0], false), [0.0, 0.0]);
            }

            // 3. NaN parameters sanitize cleanly without poisoning modal state.
            verb.set_parameters(ReverbPatch {
                size: f32::NAN,
                decay_seconds: f32::NAN,
                damping_hz: f32::NAN,
                diffusion: f32::NAN,
                mix: f32::NAN,
                width: f32::NAN,
            });
            let y = verb.process([0.5, -0.5], false);
            assert!(y[0].is_finite() && y[1].is_finite());
        }
    }

    #[test]
    fn reverb_damping_smooths_logarithmically_across_sample_rates() {
        for sr in [44_100u32, 48_000, 96_000, 192_000] {
            let sr_f = f64::from(sr);
            let mut verb = Reverb::new(sr);
            let start_hz = 400.0_f32;
            let target_hz = 10_000.0_f32;
            verb.set_parameters(ReverbPatch {
                damping_hz: start_hz,
                mix: 1.0,
                ..ReverbPatch::default()
            });
            let _ = verb.process([0.2, -0.2], false);
            assert!((verb.damping_hz_current() - f64::from(start_hz)).abs() < 1e-6);

            verb.set_parameters(ReverbPatch {
                damping_hz: target_hz,
                mix: 1.0,
                ..ReverbPatch::default()
            });

            // At t = tau * ln(2) (tau = 5 ms), LogSmoother reaches geometric mean sqrt(400 * 10000) = 2000 Hz.
            let half_frames = (0.005 * 2.0_f64.ln() * sr_f).round() as usize;
            let mut prev = verb.damping_hz_current();
            for _ in 0..half_frames {
                let y = verb.process([0.0, 0.0], false);
                assert!(y[0].is_finite() && y[1].is_finite());
                let cur = verb.damping_hz_current();
                assert!(cur >= prev - 1e-9 && cur <= f64::from(target_hz) + 1e-6);
                prev = cur;
            }
            let geo_mean = (f64::from(start_hz) * f64::from(target_hz)).sqrt();
            assert!(
                (verb.damping_hz_current() - geo_mean).abs() / geo_mean < 0.05,
                "sr={sr}: expected log midpoint ~{geo_mean:.1} Hz, got {:.1} Hz",
                verb.damping_hz_current()
            );

            for _ in 0..((sr_f * 0.20) as usize) {
                let _ = verb.process([0.0, 0.0], false);
            }
            assert!(
                (verb.damping_hz_current() - f64::from(target_hz)).abs() < 1e-2,
                "sr={sr}: damping_hz did not settle to {target_hz}, got {}",
                verb.damping_hz_current()
            );

            // Out-of-range frequency clamps cleanly to NYQUIST_SAFE_RATIO * sr.
            verb.set_parameters(ReverbPatch {
                damping_hz: 1_000_000.0,
                mix: 1.0,
                ..ReverbPatch::default()
            });
            for _ in 0..((sr_f * 0.20) as usize) {
                let _ = verb.process([0.0, 0.0], false);
            }
            assert!(
                verb.damping_hz_current() <= sr_f * NYQUIST_SAFE_RATIO + 1e-6,
                "sr={sr}: damping_hz exceeded Nyquist cap: {}",
                verb.damping_hz_current()
            );
        }
    }

    #[test]
    fn silent_sleep_advance_matches_silent_processing_bit_for_bit() {
        let patch_a = ReverbPatch {
            size: 0.3,
            decay_seconds: 0.4,
            damping_hz: 3_000.0,
            diffusion: 0.25,
            mix: 0.65,
            width: 0.8,
        };
        let patch_b = ReverbPatch {
            size: 0.85,
            decay_seconds: 4.5,
            damping_hz: 11_000.0,
            diffusion: 0.9,
            mix: 0.95,
            width: 0.3,
        };

        for sr in [48_000u32, 96_000] {
            for &bypassed in &[false, true] {
                let mut awake = Reverb::new(sr);
                let mut sleeper = Reverb::new(sr);
                awake.set_parameters(patch_a);
                sleeper.set_parameters(patch_a);
                assert!(sleeper.is_quiet(bypassed));

                for &(n, use_b) in &[
                    (1usize, true),
                    (7, true),
                    (13, true),
                    (32, false),
                    (64, false),
                ] {
                    let p = if use_b { patch_b } else { patch_a };
                    awake.set_parameters(p);
                    sleeper.set_parameters(p);
                    let mut zeros = vec![[0.0_f32; 2]; n];
                    awake.process_block(&mut zeros, bypassed);
                    sleeper.advance_silent_frames(n, bypassed);
                }

                let mut wake_a: Vec<[f32; 2]> = (0..128)
                    .map(|i| {
                        let t = i as f64 / f64::from(sr);
                        [
                            (TAU * 280.0 * t).sin() as f32 * 0.7,
                            (TAU * 410.0 * t).cos() as f32 * 0.7,
                        ]
                    })
                    .collect();
                let mut wake_s = wake_a.clone();
                awake.process_block(&mut wake_a, false);
                sleeper.process_block(&mut wake_s, false);

                for (i, (&a, &s)) in wake_a.iter().zip(wake_s.iter()).enumerate() {
                    assert_eq!(
                        [a[0].to_bits(), a[1].to_bits()],
                        [s[0].to_bits(), s[1].to_bits()],
                        "sr={sr} bypassed={bypassed} frame {i}: awake={a:?} sleeper={s:?}"
                    );
                }
            }
        }
    }
}
