//! Zero-delay-feedback (TPT/ZDF) multi-mode State Variable Filter (`Svf`),
//! 4-pole nonlinear TB-303 Acid Ladder (`AcidLadder`, unified `k = 3.85`),
//! exact complex frequency responses (including parallel dry/wet mix), and
//! Cadence's rhythmic stereo `RhythmicFilter` insert processor.

use crate::dsp::biquad::{DcBlocker, TptOnePoleLpf};
use crate::dsp::env::RhythmicEnv;
use crate::dsp::math::{
    clamp_freq_hz, control_stride_last_tick, control_stride_mask, flush_denormal_f64,
    lin_to_db_f64, mix_stereo_f64, one_pole_coeff_ms, sanitize_clamp_f64, sanitize_frame,
    sanitize_sample, sanitize_sample_f64, sanitize_sample_rate, BypassFader, BypassStep,
    LogSmoother, Smoother, LIN_FLOOR_F64, NYQUIST_SAFE_RATIO, TAIL_SILENCE_FLOOR_F64,
};
use crate::dsp::saturate::SoftClipCoeffs;
use crate::filter_patterns::CompiledFilterPattern;
use crate::mixer::FilterPatch;
use crate::rack::FilterMode;
use std::f64::consts::{LN_2, PI};

/// Unified Acid Ladder resonance feedback constant (`k = 3.85 * resonance`).
pub const ACID_LADDER_K_SCALE: f64 = 3.85;
/// Unified Acid Ladder bass compensation slope (`1.0 + 0.55 * k`).
pub const ACID_BASS_COMP_SCALE: f64 = 0.55;
/// Velocity threshold above which Acid Ladder accent engages.
/// Sits cleanly above Cadence's default `Step.velocity = 0.80` so normal notes keep
/// their authored `filter_decay_ms`, while accented (`>= 0.90` / `1.0`) hits kick in crisply.
pub const ACID_ACCENT_THRESHOLD: f64 = 0.85;
/// Maximum native `ModuleParam::VelAmount` (`4.0` octaves), used to normalize
/// Acid Ladder accent resonance boost across the full parameter range.
pub const ACID_FILTER_VEL_MAX: f64 = 4.0;

/// Normalized Acid Ladder accent strength in `[0.0, 1.0]` for `velocity`.
/// Returns `0.0` at or below `ACID_ACCENT_THRESHOLD` (`0.85`) and ramps linearly to `1.0` at `1.0`.
#[inline]
pub fn acid_accent_amount(velocity: f64) -> f64 {
    if !velocity.is_finite() {
        return 0.0;
    }
    ((velocity - ACID_ACCENT_THRESHOLD) / (1.0 - ACID_ACCENT_THRESHOLD)).clamp(0.0, 1.0)
}

/// Effective filter envelope decay time (ms) for Acid Ladder voices.
/// Accented hits (`velocity > ACID_ACCENT_THRESHOLD` with `filter_vel_amount > 0`) snap
/// decay shorter without ever lengthening an already-short decay; default (`0.80`) and
/// non-accented notes leave `filter_decay_ms` untouched.
#[inline]
pub fn acid_filter_decay_ms(
    mode: FilterMode,
    filter_decay_ms: f32,
    filter_vel_amount: f32,
    velocity: f32,
) -> f32 {
    if mode == FilterMode::AcidLowPass
        && filter_vel_amount > 0.0
        && acid_accent_amount(f64::from(velocity)) > 1e-4
    {
        (filter_decay_ms * 0.48)
            .clamp(42.0, 210.0)
            .min(filter_decay_ms)
    } else {
        filter_decay_ms
    }
}

/// Filter cutoff octave offset and effective resonance contributed by velocity / accent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AcidAccentMod {
    pub cutoff_octaves: f64,
    pub resonance: f32,
}

/// Compute velocity-driven filter cutoff octave boost and effective resonance for any `FilterMode`.
///
/// - `FilterMode::AcidLowPass` with `filter_vel_amount > 0`: applies the smooth above-threshold
///   [`acid_accent_amount`] "wow" curve and boosts resonance normalized by [`ACID_FILTER_VEL_MAX`].
/// - Standard `Svf` modes: responds smoothly across the full `0.0..=1.0` velocity range without
///   altering resonance.
#[inline]
pub fn filter_velocity_mod(
    mode: FilterMode,
    velocity: f64,
    filter_envelope: f64,
    filter_vel_amount: f32,
    base_resonance: f32,
) -> AcidAccentMod {
    let vel = if velocity.is_finite() {
        velocity.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let env = if filter_envelope.is_finite() {
        filter_envelope.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let vel_amt = if filter_vel_amount.is_finite() {
        f64::from(filter_vel_amount)
    } else {
        0.0
    };
    let base_res = if base_resonance.is_finite() {
        base_resonance.clamp(0.0, 0.95)
    } else {
        0.0
    };
    if mode == FilterMode::AcidLowPass && vel_amt > 0.0 {
        let accent = acid_accent_amount(vel);
        let wow = env * (1.35 - 0.35 * env) + 0.18;
        let cutoff_octaves = accent * vel_amt * wow;
        let knob = (vel_amt / ACID_FILTER_VEL_MAX).clamp(0.0, 1.0);
        let boost = (accent * knob * (0.28 + 0.72 * env) * 0.14) as f32;
        AcidAccentMod {
            cutoff_octaves,
            resonance: (base_res + boost).clamp(0.0, 0.95),
        }
    } else {
        let cutoff_octaves = if vel_amt != 0.0 {
            vel * vel_amt * (env + 0.25)
        } else {
            0.0
        };
        AcidAccentMod {
            cutoff_octaves,
            resonance: base_res,
        }
    }
}

/// Precomputed 2-pole TPT/ZDF State Variable Filter coefficients.
#[derive(Clone, Copy, Debug)]
pub struct SvfCoeffs {
    pub a1: f64,
    pub a2: f64,
    pub a3: f64,
    pub k: f64,
}

impl SvfCoeffs {
    #[inline]
    pub fn new(sample_rate: f64, cutoff_hz: f64, resonance: f32) -> Self {
        let res = f64::from(if resonance.is_finite() {
            resonance.clamp(0.0, 0.95)
        } else {
            0.0
        });
        Self::with_damping(sample_rate, cutoff_hz, 2.0 - 1.98 * res)
    }

    /// Construct 2-pole TPT/ZDF State Variable Filter coefficients directly from a
    /// continuous-time damping factor `k = 1 / Q` (e.g. Butterworth `k = sqrt(2)`).
    #[inline]
    pub fn with_damping(sample_rate: f64, cutoff_hz: f64, k: f64) -> Self {
        let sr = sanitize_sample_rate(sample_rate);
        let fc = clamp_freq_hz(cutoff_hz, 20.0, sr, 20.0);
        let k = if k.is_finite() {
            k.clamp(0.05, 4.0)
        } else {
            std::f64::consts::SQRT_2
        };
        let g = (PI * fc / sr).tan();
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        Self {
            a1,
            a2,
            a3: g * a2,
            k,
        }
    }
}

/// Stereo/mono 2-pole TPT/ZDF State Variable Filter (`f64`, denormal flush, NaN guard).
#[derive(Clone, Copy, Debug, Default)]
pub struct Svf {
    ic1: [f64; 2],
    ic2: [f64; 2],
}

impl Svf {
    #[inline]
    pub fn reset(&mut self) {
        self.ic1 = [0.0; 2];
        self.ic2 = [0.0; 2];
    }

    #[inline]
    fn step_channel(&mut self, ch: usize, input: f64, coeff: SvfCoeffs, mode: FilterMode) -> f64 {
        let x = sanitize_sample_f64(input);
        let v3 = x - self.ic2[ch];
        let band = coeff.a1 * self.ic1[ch] + coeff.a2 * v3;
        let low = self.ic2[ch] + coeff.a2 * self.ic1[ch] + coeff.a3 * v3;
        self.ic1[ch] = flush_denormal_f64(2.0 * band - self.ic1[ch]);
        self.ic2[ch] = flush_denormal_f64(2.0 * low - self.ic2[ch]);
        match mode {
            FilterMode::LowPass | FilterMode::AcidLowPass | FilterMode::LowPass6 => low,
            FilterMode::BandPass => band,
            FilterMode::HighPass => x - coeff.k * band - low,
        }
    }

    #[inline]
    pub fn process_mono(&mut self, input: f64, coeff: SvfCoeffs, mode: FilterMode) -> f64 {
        self.step_channel(0, input, coeff, mode)
    }

    #[inline]
    pub fn process_stereo(
        &mut self,
        input: [f64; 2],
        coeff: SvfCoeffs,
        mode: FilterMode,
    ) -> [f64; 2] {
        [
            self.step_channel(0, input[0], coeff, mode),
            self.step_channel(1, input[1], coeff, mode),
        ]
    }

    /// Whether both integrator states on both channels have decayed below silence (`1e-7`).
    #[inline]
    pub fn quiet(&self) -> bool {
        self.ic1.iter().all(|v| v.abs() < TAIL_SILENCE_FLOOR_F64)
            && self.ic2.iter().all(|v| v.abs() < TAIL_SILENCE_FLOOR_F64)
    }
}

/// Precomputed 4-pole ZDF Acid Ladder coefficients (`k = 3.85 * res`).
#[derive(Clone, Copy, Debug)]
pub struct AcidCoeffs {
    pub g_big: f64,
    pub k: f64,
    pub bass_comp: f64,
    pub inv_denom: f64,
}

impl AcidCoeffs {
    #[inline]
    pub fn new(sample_rate: f64, cutoff_hz: f64, resonance: f32) -> Self {
        let sr = sanitize_sample_rate(sample_rate);
        let fc = clamp_freq_hz(cutoff_hz, 20.0, sr, 20.0);
        let res = f64::from(if resonance.is_finite() {
            resonance.clamp(0.0, 0.95)
        } else {
            0.0
        });
        let g = (PI * fc / sr).tan();
        let g_big = g / (1.0 + g);
        let k = ACID_LADDER_K_SCALE * res;
        let g2 = g_big * g_big;
        let g4 = g2 * g2;
        Self {
            g_big,
            k,
            bass_comp: 1.0 + ACID_BASS_COMP_SCALE * k,
            inv_denom: 1.0 / (1.0 + k * g4),
        }
    }
}

/// Stereo/mono 4-pole ZDF nonlinear Acid Ladder low-pass filter (`24 dB/oct`).
#[derive(Clone, Copy, Debug, Default)]
pub struct AcidLadder {
    s: [[f64; 4]; 2],
}

impl AcidLadder {
    #[inline]
    pub fn reset(&mut self) {
        self.s = [[0.0; 4]; 2];
    }

    #[inline]
    fn step_channel(&mut self, ch: usize, input: f64, coeff: AcidCoeffs) -> f64 {
        let x = sanitize_sample_f64(input);
        let g = coeff.g_big;
        let om = 1.0 - g;
        let st = &mut self.s[ch];
        let s_sum = om * (st[3] + g * (st[2] + g * (st[1] + g * st[0])));
        let raw_u = (x * coeff.bass_comp - coeff.k * s_sum) * coeff.inv_denom;
        let mut sig = 1.6 * (raw_u * 0.625).tanh();
        for state in st.iter_mut() {
            let v = g * (sig - *state);
            let y = v + *state;
            *state = flush_denormal_f64(y + v);
            sig = y;
        }
        sig
    }

    #[inline]
    pub fn process_mono(&mut self, input: f64, coeff: AcidCoeffs) -> f64 {
        self.step_channel(0, input, coeff)
    }

    #[inline]
    pub fn process_stereo(&mut self, input: [f64; 2], coeff: AcidCoeffs) -> [f64; 2] {
        [
            self.step_channel(0, input[0], coeff),
            self.step_channel(1, input[1], coeff),
        ]
    }

    /// Whether all 4 stages on both channels have decayed below silence (`1e-7`).
    #[inline]
    pub fn quiet(&self) -> bool {
        self.s
            .iter()
            .flatten()
            .all(|v| v.abs() < TAIL_SILENCE_FLOOR_F64)
    }
}

/// Exact complex transfer function `H(jω) = (re, im)` of the TPT/ZDF State Variable Filter.
#[inline]
pub fn svf_transfer(
    mode: FilterMode,
    cutoff_hz: f32,
    resonance: f32,
    freq_hz: f64,
    sample_rate: f64,
) -> (f64, f64) {
    let sr = sanitize_sample_rate(sample_rate);
    let fc = clamp_freq_hz(f64::from(cutoff_hz), 20.0, sr, 1_000.0);
    let f = sanitize_clamp_f64(freq_hz, 0.0, sr * 0.499, 0.0);
    let u = (PI * f / sr).tan() / (PI * fc / sr).tan().max(1e-12);
    let res = sanitize_clamp_f64(f64::from(resonance), 0.0, 0.95, 0.0);
    let k = 2.0 - 1.98 * res;
    let d_re = 1.0 - u * u;
    let d_im = k * u;
    let denom_sq = (d_re * d_re + d_im * d_im).max(1e-24);
    match mode {
        FilterMode::LowPass | FilterMode::AcidLowPass | FilterMode::LowPass6 => {
            (d_re / denom_sq, -d_im / denom_sq)
        }
        FilterMode::BandPass => (u * d_im / denom_sq, u * d_re / denom_sq),
        FilterMode::HighPass => (-u * u * d_re / denom_sq, u * u * d_im / denom_sq),
    }
}

/// Exact complex transfer function `H(jω) = (re, im)` of the bilinear/TPT 1-pole
/// `6 dB/oct` low-pass filter (`H(s) = 1 / (1 + s/ωc)`).
#[inline]
pub fn one_pole_lpf_transfer(cutoff_hz: f32, freq_hz: f64, sample_rate: f64) -> (f64, f64) {
    let sr = sanitize_sample_rate(sample_rate);
    let fc = clamp_freq_hz(f64::from(cutoff_hz), 20.0, sr, 1_000.0);
    let f = sanitize_clamp_f64(freq_hz, 0.0, sr * 0.499, 0.0);
    let u = (PI * f / sr).tan() / (PI * fc / sr).tan().max(1e-12);
    let denom = (1.0 + u * u).max(1e-24);
    (1.0 / denom, -u / denom)
}

/// Exact small-signal complex transfer function `H(jω) = (re, im)` of the 4-pole
/// ZDF Acid Ladder (`k = 3.85 * res`, bass compensation `1 + 0.55 * k`).
#[inline]
pub fn acid_ladder_transfer(
    cutoff_hz: f32,
    resonance: f32,
    freq_hz: f64,
    sample_rate: f64,
) -> (f64, f64) {
    let sr = sanitize_sample_rate(sample_rate);
    let fc = clamp_freq_hz(f64::from(cutoff_hz), 20.0, sr, 1_000.0);
    let f = sanitize_clamp_f64(freq_hz, 0.0, sr * 0.499, 0.0);
    let u = (PI * f / sr).tan() / (PI * fc / sr).tan().max(1e-12);
    let res = sanitize_clamp_f64(f64::from(resonance), 0.0, 0.95, 0.0);
    let k = ACID_LADDER_K_SCALE * res;
    let u2 = u * u;
    let u4 = u2 * u2;
    let d_re = 1.0 - 6.0 * u2 + u4 + k;
    let d_im = 4.0 * u * (1.0 - u2);
    let denom_sq = (d_re * d_re + d_im * d_im).max(1e-24);
    let num = 1.0 + ACID_BASS_COMP_SCALE * k;
    (num * d_re / denom_sq, -num * d_im / denom_sq)
}

/// Exact complex transfer function `H(jω) = (re, im)` for any `FilterMode`.
#[inline]
pub fn filter_transfer(
    mode: FilterMode,
    cutoff_hz: f32,
    resonance: f32,
    freq_hz: f64,
    sample_rate: f64,
) -> (f64, f64) {
    match mode {
        FilterMode::AcidLowPass => acid_ladder_transfer(cutoff_hz, resonance, freq_hz, sample_rate),
        FilterMode::LowPass6 => one_pole_lpf_transfer(cutoff_hz, freq_hz, sample_rate),
        _ => svf_transfer(mode, cutoff_hz, resonance, freq_hz, sample_rate),
    }
}

/// Exact linear magnitude `|H(jω)|` for any `FilterMode`.
#[inline]
pub fn filter_magnitude(
    mode: FilterMode,
    cutoff_hz: f32,
    resonance: f32,
    freq_hz: f64,
    sample_rate: f64,
) -> f64 {
    let (re, im) = filter_transfer(mode, cutoff_hz, resonance, freq_hz, sample_rate);
    re.hypot(im)
}

/// Exact magnitude response in decibels for any `FilterMode` (`-120 dB` floor).
#[inline]
pub fn filter_magnitude_db(
    mode: FilterMode,
    cutoff_hz: f32,
    resonance: f32,
    freq_hz: f64,
    sample_rate: f64,
) -> f64 {
    lin_to_db_f64(filter_magnitude(
        mode,
        cutoff_hz,
        resonance,
        freq_hz,
        sample_rate,
    ))
}

/// Exact parallel dry/wet magnitude response in decibels: `|(1 - mix) + mix * H(jω)|`,
/// accounting for complex phase interaction between dry and filtered paths.
#[inline]
pub fn filter_mix_magnitude_db(
    mode: FilterMode,
    cutoff_hz: f32,
    resonance: f32,
    mix: f32,
    freq_hz: f64,
    sample_rate: f64,
) -> f64 {
    let m = sanitize_clamp_f64(f64::from(mix), 0.0, 1.0, 1.0);
    let (h_re, h_im) = filter_transfer(mode, cutoff_hz, resonance, freq_hz, sample_rate);
    let re = (1.0 - m) + m * h_re;
    let im = m * h_im;
    lin_to_db_f64(re.hypot(im))
}

/// Stereo resonant multi-mode rhythmic filter insert processor composing
/// [`Svf`], [`AcidLadder`], [`RhythmicEnv`], [`SoftClipCoeffs`], [`DcBlocker`],
/// smooth parameter slewing, and click-free mode & bypass crossfades. Zero heap allocations.
pub struct RhythmicFilter {
    sample_rate: f64,
    mode: FilterMode,
    prev_mode: FilterMode,
    mode_xfade: Smoother,
    cutoff: LogSmoother,
    cutoff_coeff: f64,
    resonance: Smoother,
    drive: Smoother,
    mix: Smoother,
    bypass: BypassFader,
    env: RhythmicEnv,
    env_amount_ln2: f64,
    svf: Svf,
    acid: AcidLadder,
    one_pole: TptOnePoleLpf,
    dc_blocker: DcBlocker,
    svf_coeffs: SvfCoeffs,
    acid_coeffs: AcidCoeffs,
    cached_drive_bits: u64,
    cached_drive_coeffs: SoftClipCoeffs,
    cached_coeff_key: (u64, u64, u64, FilterMode, FilterMode, bool),
    last_patch: FilterPatch,
    ctrl_count: u32,
    ctrl_mask: u32,
    has_processed: bool,
    #[cfg(test)]
    coeff_recomputes: u64,
}

impl RhythmicFilter {
    pub fn new(sample_rate: f64) -> Self {
        let sr = sanitize_sample_rate(sample_rate);
        let default = FilterPatch::default();
        let init_fc = clamp_freq_hz(f64::from(default.cutoff_hz), 20.0, sr, 1_000.0);
        let init_drive = sanitize_clamp_f64(f64::from(default.drive), 0.0, 1.0, 0.0);
        let mut f = Self {
            sample_rate: sr,
            mode: default.mode,
            prev_mode: default.mode,
            mode_xfade: Smoother::new(1.0, 3.0, sr),
            cutoff: LogSmoother::new_clamped(init_fc, 20.0, sr * NYQUIST_SAFE_RATIO),
            cutoff_coeff: one_pole_coeff_ms(4.0, sr),
            resonance: Smoother::new(f64::from(default.resonance), 4.0, sr),
            drive: Smoother::new(init_drive, 4.0, sr),
            mix: Smoother::new(f64::from(default.mix), 4.0, sr),
            bypass: BypassFader::new(sr),
            env: RhythmicEnv::new(sr),
            env_amount_ln2: 0.0,
            svf: Svf::default(),
            acid: AcidLadder::default(),
            one_pole: TptOnePoleLpf::new(sr, init_fc),
            dc_blocker: DcBlocker::new(sr),
            svf_coeffs: SvfCoeffs::new(sr, init_fc, default.resonance),
            acid_coeffs: AcidCoeffs::new(sr, init_fc, default.resonance),
            cached_drive_bits: init_drive.to_bits(),
            cached_drive_coeffs: SoftClipCoeffs::new(init_drive),
            cached_coeff_key: (
                u64::MAX,
                u64::MAX,
                u64::MAX,
                default.mode,
                default.mode,
                false,
            ),
            last_patch: default,
            ctrl_count: 0,
            ctrl_mask: control_stride_mask(sr, 6_000.0),
            has_processed: false,
            #[cfg(test)]
            coeff_recomputes: 0,
        };
        f.set_parameters(default);
        f
    }

    #[cfg(test)]
    pub(crate) fn is_idle(&self) -> bool {
        self.bypass.is_idle()
    }

    #[cfg(test)]
    pub(crate) fn cutoff_hz_current(&self) -> f64 {
        self.cutoff.current()
    }

    #[cfg(test)]
    pub(crate) fn coeff_recomputes(&self) -> u64 {
        self.coeff_recomputes
    }

    pub fn reset(&mut self) {
        self.svf.reset();
        self.acid.reset();
        self.one_pole.reset();
        self.dc_blocker.reset();
        self.env.reset();
        self.cutoff.snap_to_target();
        self.resonance.snap(self.resonance.target());
        self.drive.snap(self.drive.target());
        let d = self.drive.current();
        self.cached_drive_bits = d.to_bits();
        self.cached_drive_coeffs = SoftClipCoeffs::new(d);
        self.mix.snap(self.mix.target());
        self.prev_mode = self.mode;
        self.mode_xfade.snap(1.0);
        self.bypass.reset();
        self.has_processed = false;
        self.ctrl_count = 0;
        self.recompute_coefficients_inner(0.0, true);
    }

    fn set_parameters_common(&mut self, patch: FilterPatch) {
        let def = FilterPatch::default();
        let fc = clamp_freq_hz(
            f64::from(patch.cutoff_hz),
            20.0,
            self.sample_rate,
            f64::from(def.cutoff_hz),
        );
        let res = sanitize_clamp_f64(
            f64::from(patch.resonance),
            0.0,
            0.95,
            f64::from(def.resonance),
        );
        let drv = sanitize_clamp_f64(f64::from(patch.drive), 0.0, 1.0, f64::from(def.drive));
        let mix = sanitize_clamp_f64(f64::from(patch.mix), 0.0, 1.0, f64::from(def.mix));
        let amt = sanitize_clamp_f64(
            f64::from(patch.env_amount),
            -4.0,
            4.0,
            f64::from(def.env_amount),
        );
        let env_decay_ms = sanitize_clamp_f64(
            f64::from(patch.env_decay_ms),
            15.0,
            1500.0,
            f64::from(def.env_decay_ms),
        );
        let sanitized = FilterPatch {
            mode: patch.mode,
            cutoff_hz: fc as f32,
            resonance: res as f32,
            drive: drv as f32,
            mix: mix as f32,
            env_pattern: patch.env_pattern,
            env_pattern_id: patch.env_pattern_id,
            env_amount: amt as f32,
            env_decay_ms: env_decay_ms as f32,
        };
        if self.has_processed && sanitized == self.last_patch {
            return;
        }
        self.last_patch = sanitized;
        self.env_amount_ln2 = if amt.abs() < 1e-4 { 0.0 } else { amt * LN_2 };

        if !self.has_processed {
            self.mode = patch.mode;
            self.prev_mode = patch.mode;
            self.mode_xfade.snap(1.0);
            self.cutoff.snap(fc);
            self.resonance.snap(res);
            self.drive.snap(drv);
            self.cached_drive_bits = drv.to_bits();
            self.cached_drive_coeffs = SoftClipCoeffs::new(drv);
            self.mix.snap(mix);
            self.recompute_coefficients_inner(self.env.current(), true);
        } else {
            if patch.mode != self.mode {
                let topo = |m: FilterMode| match m {
                    FilterMode::AcidLowPass => 1u8,
                    FilterMode::LowPass6 => 2u8,
                    _ => 0u8,
                };
                if topo(self.mode) != topo(patch.mode) {
                    match topo(patch.mode) {
                        1 => self.acid.reset(),
                        2 => self.one_pole.reset(),
                        _ => self.svf.reset(),
                    }
                }
                self.prev_mode = self.mode;
                self.mode = patch.mode;
                self.mode_xfade.snap(0.0);
                self.mode_xfade.set_target(1.0);
                self.ctrl_count = 0;
            }
            self.cutoff.set_target(fc);
            self.resonance.set_target(res);
            self.drive.set_target(drv);
            self.mix.set_target(mix);
        }
    }

    pub fn set_parameters(&mut self, patch: FilterPatch) {
        self.set_parameters_common(patch);
        self.env
            .set_pattern(CompiledFilterPattern::neutral(), patch.env_decay_ms);
    }

    /// Update parameters together with a compiled rhythmic pattern and transport clock.
    pub fn set_parameters_with_clock(
        &mut self,
        patch: FilterPatch,
        compiled: CompiledFilterPattern,
        song_pos: f64,
        bpm: f64,
    ) {
        self.set_parameters_common(patch);
        self.env
            .sync_song_pos(compiled, patch.env_decay_ms, song_pos, bpm);
    }

    #[inline]
    fn recompute_coefficients_inner(&mut self, env_level: f64, force: bool) {
        let raw_env_mod = if self.env_amount_ln2 == 0.0 {
            0.0
        } else {
            env_level * self.env_amount_ln2
        };
        let env_mod = if raw_env_mod == 0.0 { 0.0 } else { raw_env_mod };
        let fading = !self.mode_xfade.is_settled();
        let base_fc = self.cutoff.current();
        let res_f64 = self.resonance.current();
        let key = (
            base_fc.to_bits(),
            res_f64.to_bits(),
            env_mod.to_bits(),
            self.mode,
            self.prev_mode,
            fading,
        );
        if !force && key == self.cached_coeff_key {
            return;
        }
        self.cached_coeff_key = key;
        #[cfg(test)]
        {
            self.coeff_recomputes += 1;
        }

        let fc = if env_mod == 0.0 {
            base_fc
        } else {
            clamp_freq_hz(base_fc * env_mod.exp(), 20.0, self.sample_rate, 1_000.0)
        };
        let res = res_f64 as f32;
        let is_svf = |m: FilterMode| !matches!(m, FilterMode::AcidLowPass | FilterMode::LowPass6);
        let need_acid = self.mode == FilterMode::AcidLowPass
            || (fading && self.prev_mode == FilterMode::AcidLowPass);
        let need_one_pole =
            self.mode == FilterMode::LowPass6 || (fading && self.prev_mode == FilterMode::LowPass6);
        let need_svf = is_svf(self.mode) || (fading && is_svf(self.prev_mode));
        if need_acid {
            self.acid_coeffs = AcidCoeffs::new(self.sample_rate, fc, res);
        }
        if need_one_pole {
            self.one_pole.set_cutoff(self.sample_rate, fc);
        }
        if need_svf {
            self.svf_coeffs = SvfCoeffs::new(self.sample_rate, fc, res);
        }
    }

    #[inline]
    fn advance_smoothers_and_env(&mut self, frames: usize) -> f64 {
        let env_level = if self.env_amount_ln2 != 0.0 {
            self.env.advance_silent(frames)
        } else {
            0.0
        };
        self.cutoff.advance_silent(self.cutoff_coeff, frames);
        self.resonance.advance_silent(frames);
        self.drive.advance_silent(frames);
        self.mix.advance_silent(frames);
        self.mode_xfade.advance_silent(frames);
        env_level
    }

    #[inline]
    pub fn advance_silent_frames(&mut self, frames: usize, bypassed: bool) {
        if frames == 0 {
            return;
        }
        self.has_processed = true;
        self.bypass.advance_silent(bypassed);
        if let Some(last_tick) = control_stride_last_tick(self.ctrl_count, self.ctrl_mask, frames) {
            let env_at_tick = self.advance_smoothers_and_env(last_tick + 1);
            self.recompute_coefficients_inner(env_at_tick, false);
            let rem = frames - 1 - last_tick;
            if rem > 0 {
                self.advance_smoothers_and_env(rem);
            }
        } else {
            self.advance_smoothers_and_env(frames);
        }
        self.ctrl_count = self.ctrl_count.wrapping_add(frames as u32) & self.ctrl_mask;
        let drv = self.drive.current();
        let drv_bits = drv.to_bits();
        if drv_bits != self.cached_drive_bits {
            self.cached_drive_bits = drv_bits;
            self.cached_drive_coeffs = SoftClipCoeffs::new(drv);
        }
    }

    pub fn process(&mut self, input: [f32; 2], bypassed: bool) -> [f32; 2] {
        self.has_processed = true;
        let dry = sanitize_frame(input);

        let bypass_w = match self.bypass.step(bypassed) {
            BypassStep::Idle => {
                self.advance_silent_frames(1, true);
                return dry;
            }
            BypassStep::SettledBypass => {
                self.svf.reset();
                self.acid.reset();
                self.one_pole.reset();
                self.dc_blocker.reset();
                self.bypass.mark_idle();
                self.advance_silent_frames(1, true);
                return dry;
            }
            BypassStep::Active(w) => w,
        };

        let env_level = if self.env_amount_ln2 != 0.0 {
            self.env.next_sample()
        } else {
            0.0
        };

        self.cutoff.step(self.cutoff_coeff);
        self.resonance.next();
        let drv = self.drive.next();
        let mix = self.mix.next();
        let m_xf = self.mode_xfade.next();

        if self.ctrl_count == 0 {
            self.recompute_coefficients_inner(env_level, false);
        }
        self.ctrl_count = self.ctrl_count.wrapping_add(1) & self.ctrl_mask;

        let drv_bits = drv.to_bits();
        if drv_bits != self.cached_drive_bits {
            self.cached_drive_bits = drv_bits;
            self.cached_drive_coeffs = SoftClipCoeffs::new(drv);
        }
        let dry64 = [f64::from(dry[0]), f64::from(dry[1])];
        let driven = if drv > 0.0 {
            [
                self.cached_drive_coeffs.shape(dry64[0]),
                self.cached_drive_coeffs.shape(dry64[1]),
            ]
        } else {
            dry64
        };

        let filtered = if m_xf >= 1.0 - LIN_FLOOR_F64 {
            match self.mode {
                FilterMode::AcidLowPass => self.acid.process_stereo(driven, self.acid_coeffs),
                FilterMode::LowPass6 => self.one_pole.process_stereo(driven),
                mode => self.svf.process_stereo(driven, self.svf_coeffs, mode),
            }
        } else {
            let is_svf =
                |m: FilterMode| !matches!(m, FilterMode::AcidLowPass | FilterMode::LowPass6);
            let (prev_out, next_out) = if is_svf(self.prev_mode) && is_svf(self.mode) {
                // Both SVF modes: single topology step, crossfade outputs seamlessly!
                let mut prev = [0.0; 2];
                let mut next = [0.0; 2];
                for ch in 0..2 {
                    let x = driven[ch];
                    let v3 = x - self.svf.ic2[ch];
                    let band = self.svf_coeffs.a1 * self.svf.ic1[ch] + self.svf_coeffs.a2 * v3;
                    let low = self.svf.ic2[ch]
                        + self.svf_coeffs.a2 * self.svf.ic1[ch]
                        + self.svf_coeffs.a3 * v3;
                    self.svf.ic1[ch] = flush_denormal_f64(2.0 * band - self.svf.ic1[ch]);
                    self.svf.ic2[ch] = flush_denormal_f64(2.0 * low - self.svf.ic2[ch]);
                    let high = x - self.svf_coeffs.k * band - low;
                    let pick = |m: FilterMode| match m {
                        FilterMode::LowPass | FilterMode::AcidLowPass | FilterMode::LowPass6 => low,
                        FilterMode::BandPass => band,
                        FilterMode::HighPass => high,
                    };
                    prev[ch] = pick(self.prev_mode);
                    next[ch] = pick(self.mode);
                }
                (prev, next)
            } else {
                // Cross-topology fade (SVF <-> AcidLadder <-> OnePoleLpf): each topology runs independently.
                let mut run_mode = |m: FilterMode| match m {
                    FilterMode::AcidLowPass => self.acid.process_stereo(driven, self.acid_coeffs),
                    FilterMode::LowPass6 => self.one_pole.process_stereo(driven),
                    other => self.svf.process_stereo(driven, self.svf_coeffs, other),
                };
                (run_mode(self.prev_mode), run_mode(self.mode))
            };
            let inv = 1.0 - m_xf;
            [
                prev_out[0] * inv + next_out[0] * m_xf,
                prev_out[1] * inv + next_out[1] * m_xf,
            ]
        };

        let dc_free = self.dc_blocker.process_stereo(filtered);
        let wet = [
            sanitize_sample_f64(dc_free[0]),
            sanitize_sample_f64(dc_free[1]),
        ];
        let out = mix_stereo_f64(dry64, wet, mix * bypass_w);
        [
            sanitize_sample(out[0] as f32),
            sanitize_sample(out[1] as f32),
        ]
    }
}

impl crate::dsp::StereoEffect for RhythmicFilter {
    #[inline]
    fn process(&mut self, input: [f32; 2], bypassed: bool) -> [f32; 2] {
        RhythmicFilter::process(self, input, bypassed)
    }

    #[inline]
    fn process_block(&mut self, buf: &mut [[f32; 2]], bypassed: bool) {
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
            *frame = RhythmicFilter::process(self, *frame, bypassed);
        }
    }

    #[inline]
    fn reset(&mut self) {
        RhythmicFilter::reset(self);
    }

    #[inline]
    fn is_quiet(&self, bypassed: bool) -> bool {
        self.bypass.is_settled_for(bypassed)
            && (bypassed
                || (self.svf.quiet()
                    && self.acid.quiet()
                    && self.one_pole.quiet()
                    && self.dc_blocker.quiet()))
    }

    #[inline]
    fn advance_silent_frames(&mut self, frames: usize, bypassed: bool) {
        RhythmicFilter::advance_silent_frames(self, frames, bypassed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    fn measure_svf_or_acid_db(mode: FilterMode, cutoff: f32, res: f32, freq: f64, sr: f64) -> f64 {
        let warmup = (sr * 0.15) as usize;
        let measure = (sr * 0.10) as usize;
        let mut svf = Svf::default();
        let mut acid = AcidLadder::default();
        let svf_c = SvfCoeffs::new(sr, f64::from(cutoff), res);
        let acid_c = AcidCoeffs::new(sr, f64::from(cutoff), res);
        let mut in_sq = 0.0;
        let mut out_sq = 0.0;
        // Small amplitude (0.02) stays in linear regime of AcidLadder soft-tanh so exact
        // small-signal transfer function matches within < 0.25 dB!
        let amp = 0.02;
        for i in 0..(warmup + measure) {
            let x = (TAU * freq * i as f64 / sr).sin() * amp;
            let y = if mode == FilterMode::AcidLowPass {
                acid.process_mono(x, acid_c)
            } else {
                svf.process_mono(x, svf_c, mode)
            };
            if i >= warmup {
                in_sq += x * x;
                out_sq += y * y;
            }
        }
        lin_to_db_f64((out_sq / in_sq).sqrt())
    }

    #[test]
    fn svf_and_acid_ladder_analytical_response_matches_dsp_across_sample_rates() {
        for sr in [44_100.0, 48_000.0, 96_000.0] {
            for mode in [
                FilterMode::LowPass,
                FilterMode::BandPass,
                FilterMode::HighPass,
                FilterMode::AcidLowPass,
            ] {
                let cutoff = 1_000.0_f32;
                let res = 0.65_f32;
                for freq in [250.0, 1_000.0, 3_500.0] {
                    let predicted = filter_magnitude_db(mode, cutoff, res, freq, sr);
                    let measured = measure_svf_or_acid_db(mode, cutoff, res, freq, sr);
                    assert!(
                        (predicted - measured).abs() < 0.5,
                        "sr={sr} mode={mode:?} freq={freq}: predicted={predicted:.2} dB vs measured={measured:.2} dB"
                    );
                }
            }
        }
    }

    #[test]
    fn parallel_mix_magnitude_captures_complex_phase_summation() {
        let sr = 48_000.0;
        // At mix = 0.0, response must be flat 0 dB everywhere.
        for freq in [100.0, 1_000.0, 8_000.0] {
            let db = filter_mix_magnitude_db(FilterMode::LowPass, 500.0, 0.5, 0.0, freq, sr);
            assert!(db.abs() < 1e-6, "mix=0 should be 0 dB, got {db}");
        }
        // At mix = 1.0, must match filter_magnitude_db exactly.
        let full = filter_mix_magnitude_db(FilterMode::AcidLowPass, 800.0, 0.7, 1.0, 800.0, sr);
        let base = filter_magnitude_db(FilterMode::AcidLowPass, 800.0, 0.7, 800.0, sr);
        assert!((full - base).abs() < 1e-6);

        // At mix = 0.5, verify against actual RhythmicFilter DSP!
        let mut rf = RhythmicFilter::new(sr);
        rf.set_parameters(FilterPatch {
            mode: FilterMode::LowPass,
            cutoff_hz: 800.0,
            resonance: 0.3,
            drive: 0.0,
            mix: 0.5,
            ..FilterPatch::default()
        });
        rf.reset();
        let warmup = 4800;
        let measure = 4800;
        let freq = 800.0;
        let mut in_sq = 0.0;
        let mut out_sq = 0.0;
        for i in 0..(warmup + measure) {
            let x = (TAU * freq * i as f64 / sr).sin() as f32 * 0.2;
            let y = rf.process([x, x], false);
            if i >= warmup {
                in_sq += f64::from(x) * f64::from(x);
                out_sq += f64::from(y[0]) * f64::from(y[0]);
            }
        }
        let measured_db = lin_to_db_f64((out_sq / in_sq).sqrt());
        let predicted_db = filter_mix_magnitude_db(FilterMode::LowPass, 800.0, 0.3, 0.5, freq, sr);
        assert!(
            (measured_db - predicted_db).abs() < 0.5,
            "parallel mix: measured={measured_db:.2} dB vs predicted={predicted_db:.2} dB"
        );
    }

    #[test]
    fn rhythmic_filter_envelope_and_mode_switches_are_bounded_and_clean() {
        let sr = 48_000.0;
        let bpm = 120.0;
        let mut steps = [0u8; crate::filter_patterns::MAX_FILTER_ENV_STEPS];
        steps[0] = 127;
        let pat0 = CompiledFilterPattern {
            resolution: 16,
            length: 16,
            smooth_attack: false,
            steps,
        };
        let mut f = RhythmicFilter::new(sr);
        let patch = FilterPatch {
            mode: FilterMode::LowPass,
            cutoff_hz: 250.0,
            resonance: 0.25,
            drive: 0.0,
            mix: 1.0,
            env_pattern: 0,
            env_pattern_id: None,
            env_amount: 3.5,
            env_decay_ms: 80.0,
        };
        let mut peak_early = 0.0_f32;
        let mut peak_late = 0.0_f32;
        for i in 0..24_000 {
            if i % 64 == 0 {
                let pos = (i as f64) * bpm / (15.0 * sr);
                f.set_parameters_with_clock(patch, pat0, pos, bpm);
            }
            let s = (TAU * 1500.0 * i as f64 / sr).sin() as f32 * 0.5;
            let y = f.process([s, s], false);
            assert!(y[0].is_finite() && y[0].abs() < 4.0);
            if (256..2000).contains(&i) {
                peak_early = peak_early.max(y[0].abs());
            } else if i >= 20_000 {
                peak_late = peak_late.max(y[0].abs());
            }
        }
        assert!(peak_early > peak_late * 3.0);

        // Mode transition LP -> AcidLowPass -> HighPass remains finite & bounded.
        for mode in [
            FilterMode::AcidLowPass,
            FilterMode::HighPass,
            FilterMode::BandPass,
        ] {
            f.set_parameters(FilterPatch { mode, ..patch });
            for i in 0..512 {
                let s = (TAU * 440.0 * i as f64 / sr).sin() as f32 * 0.4;
                let y = f.process([s, -s], false);
                assert!(y[0].is_finite() && y[1].is_finite() && y[0].abs() < 4.0);
            }
        }
    }

    #[test]
    fn acid_accent_math_spares_default_velocity_and_normalizes_by_full_range() {
        // Default Cadence Step.velocity (0.80) and ReBirth non-accent (0.64) must NOT trigger accent.
        for vel in [0.0, 0.5, 0.64, 0.80, ACID_ACCENT_THRESHOLD] {
            assert_eq!(acid_accent_amount(vel), 0.0);
            assert_eq!(
                acid_filter_decay_ms(FilterMode::AcidLowPass, 260.0, 3.0, vel as f32),
                260.0
            );
            let m = filter_velocity_mod(FilterMode::AcidLowPass, vel, 1.0, 3.0, 0.6);
            assert_eq!(m.cutoff_octaves, 0.0);
            assert!((m.resonance - 0.6).abs() < 1e-6);
        }
        // Accented velocity (> 0.85, e.g. 0.92 and 1.0) snaps decay shorter and boosts cutoff + res.
        assert!((acid_accent_amount(1.0) - 1.0).abs() < 1e-6);
        let mid = acid_accent_amount(0.925);
        assert!((mid - 0.5).abs() < 1e-6);
        let snapped = acid_filter_decay_ms(FilterMode::AcidLowPass, 260.0, 3.0, 1.0);
        assert!((snapped - 260.0 * 0.48).abs() < 1e-4);
        // Short decay must never be lengthened by the 42 ms floor.
        assert_eq!(
            acid_filter_decay_ms(FilterMode::AcidLowPass, 25.0, 3.0, 1.0),
            25.0
        );
        // Resonance boost scales continuously across the full FilterVelAmount range (0..=4.0),
        // NOT saturating early at RBS 3.2!
        let at_32 = filter_velocity_mod(FilterMode::AcidLowPass, 1.0, 1.0, 3.2, 0.5);
        let at_40 = filter_velocity_mod(FilterMode::AcidLowPass, 1.0, 1.0, 4.0, 0.5);
        assert!(at_32.resonance > 0.5);
        assert!(
            at_40.resonance > at_32.resonance + 0.02,
            "4.0 must boost resonance more than 3.2 (at_32={}, at_40={})",
            at_32.resonance,
            at_40.resonance
        );
        assert!((at_40.resonance - 0.64).abs() < 1e-5);
        // Standard LowPass responds smoothly across full 0..=1 velocity without altering resonance.
        let lp = filter_velocity_mod(FilterMode::LowPass, 0.5, 1.0, 2.0, 0.4);
        assert!((lp.cutoff_octaves - 1.25).abs() < 1e-6);
        assert!((lp.resonance - 0.4).abs() < 1e-6);
        // Non-finite inputs sanitize safely.
        let nan_mod = filter_velocity_mod(
            FilterMode::AcidLowPass,
            f64::NAN,
            f64::NAN,
            f32::NAN,
            f32::NAN,
        );
        assert_eq!(nan_mod.cutoff_octaves, 0.0);
        assert_eq!(nan_mod.resonance, 0.0);
    }

    #[test]
    fn low_pass_6_matches_one_pole_transfer_and_crossfades_cleanly() {
        let sr = 48_000.0;
        let fc = 1_000.0_f32;
        // Exact SPOT parity between `filter_transfer(LowPass6)` and `TptOnePoleLpf::magnitude`.
        for &f in &[50.0, 250.0, 1_000.0, 2_000.0, 4_000.0, 8_000.0] {
            let m_filt = filter_magnitude(FilterMode::LowPass6, fc, 0.8, f, sr);
            let m_1p = TptOnePoleLpf::magnitude(f64::from(fc), f, sr);
            assert!(
                (m_filt - m_1p).abs() < 1e-12,
                "f={f}: filter_magnitude={m_filt} != TptOnePoleLpf={m_1p}"
            );
        }
        // -3.01 dB at cutoff, ~-6 dB/oct gentler than 2-pole LowPass (-12 dB/oct) at 4*fc.
        let at_fc_db = filter_magnitude_db(FilterMode::LowPass6, fc, 0.0, 1_000.0, sr);
        assert!((at_fc_db + 3.0103).abs() < 0.05, "at_fc_db={at_fc_db}");
        let lp6_4k = filter_magnitude_db(FilterMode::LowPass6, fc, 0.0, 4_000.0, sr);
        let lp12_4k = filter_magnitude_db(FilterMode::LowPass, fc, 0.0, 4_000.0, sr);
        assert!(
            lp6_4k > lp12_4k + 9.0,
            "1-pole LP 6dB must be much gentler at 4*fc than 2-pole LP: lp6={lp6_4k:.2} lp12={lp12_4k:.2}"
        );

        // RhythmicFilter processes LowPass6 accurately and crossfades click-free with all sibling modes.
        let mut rf = RhythmicFilter::new(sr);
        let p_lp6 = FilterPatch {
            mode: FilterMode::LowPass6,
            cutoff_hz: 1_000.0,
            resonance: 0.5,
            drive: 0.0,
            mix: 1.0,
            ..FilterPatch::default()
        };
        rf.set_parameters(p_lp6);
        for other in [
            FilterMode::LowPass,
            FilterMode::AcidLowPass,
            FilterMode::HighPass,
            FilterMode::BandPass,
            FilterMode::LowPass6,
        ] {
            rf.set_parameters(FilterPatch {
                mode: other,
                ..p_lp6
            });
            for i in 0..1_200 {
                let x = (std::f64::consts::TAU * 440.0 * i as f64 / sr).sin() as f32 * 0.5;
                let y = rf.process([x, x], false);
                assert!(y[0].is_finite() && y[1].is_finite());
                assert!(y[0].abs() < 2.5 && y[1].abs() < 2.5);
            }
        }
    }

    #[test]
    fn filter_transfers_and_rhythmic_filter_sanitize_nan_parameters() {
        for sr in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            for mode in [
                FilterMode::LowPass,
                FilterMode::HighPass,
                FilterMode::BandPass,
                FilterMode::AcidLowPass,
                FilterMode::LowPass6,
            ] {
                let (re, im) = filter_transfer(mode, f32::NAN, f32::NAN, f64::NAN, sr);
                assert!(re.is_finite() && im.is_finite(), "sr={sr} mode={mode:?}");
                let mag_db =
                    filter_mix_magnitude_db(mode, f32::NAN, f32::NAN, f32::NAN, 1_000.0, sr);
                assert!(mag_db.is_finite(), "sr={sr} mode={mode:?}");
            }

            let mut rf = RhythmicFilter::new(sr);
            rf.set_parameters(FilterPatch {
                mode: FilterMode::LowPass,
                cutoff_hz: f32::NAN,
                resonance: f32::NAN,
                drive: f32::NAN,
                mix: f32::NAN,
                env_amount: f32::NAN,
                env_decay_ms: f32::NAN,
                ..FilterPatch::default()
            });
            for _ in 0..128 {
                let y = rf.process([0.5, -0.5], false);
                assert!(y[0].is_finite() && y[1].is_finite());
            }
        }
    }

    #[test]
    fn rhythmic_filter_preserves_unclipped_resonant_headroom_and_caches_static_coeffs() {
        for sr in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            // 1. High-resonance LowPass sine at cutoff exceeds +12 dBFS (> 4.0) without clipping
            //    and matches analytical filter_magnitude_db across all sample rates.
            let mut rf = RhythmicFilter::new(sr);
            let patch = FilterPatch {
                mode: FilterMode::LowPass,
                cutoff_hz: 1_000.0,
                resonance: 0.95,
                drive: 0.0,
                mix: 1.0,
                env_amount: 0.0,
                ..FilterPatch::default()
            };
            rf.set_parameters(patch);

            let warmup = (sr * 0.12) as usize;
            let measure = (sr * 0.05) as usize;
            let mut in_sq = 0.0_f64;
            let mut out_sq = 0.0_f64;
            let mut peak = 0.0_f32;
            for i in 0..(warmup + measure) {
                let x = (TAU * 1_000.0 * i as f64 / sr).sin() as f32 * 0.7;
                let y = rf.process([x, -x], false);
                assert!(y[0].is_finite() && y[1].is_finite());
                if i >= warmup {
                    peak = peak.max(y[0].abs()).max(y[1].abs());
                    in_sq += f64::from(x) * f64::from(x);
                    out_sq += f64::from(y[0]) * f64::from(y[0]);
                }
            }
            assert!(
                peak > 4.5,
                "sr={sr}: expected unclipped resonant peak > 4.5, got {peak}"
            );
            let measured_db = lin_to_db_f64((out_sq / in_sq).sqrt());
            let expected_db = filter_magnitude_db(FilterMode::LowPass, 1_000.0, 0.95, 1_000.0, sr);
            assert!(
                (measured_db - expected_db).abs() < 0.5,
                "sr={sr}: resonant gain measured={measured_db:.2} dB vs predicted={expected_db:.2} dB"
            );

            // 2. Static settled parameters incur zero redundant coefficient recomputations,
            //    even when set_parameters is re-called every control block.
            let recomputes_after_steady = rf.coeff_recomputes();
            for i in 0..2048 {
                if i % 64 == 0 {
                    rf.set_parameters(patch);
                }
                let _ = rf.process([0.2, -0.2], false);
            }
            assert_eq!(
                rf.coeff_recomputes(),
                recomputes_after_steady,
                "sr={sr}: static settled filter must not recompute coefficients"
            );

            // 3. Bypass settles cleanly to idle via BypassFader.
            assert!(!rf.is_idle());
            for _ in 0..((sr * 0.03) as usize) {
                let _ = rf.process([0.3, -0.3], true);
            }
            assert!(rf.is_idle(), "sr={sr}: bypassed filter must idle");
            assert_eq!(rf.process([0.3, -0.3], true), [0.3, -0.3]);
        }
    }

    #[test]
    fn rhythmic_filter_cutoff_smooths_logarithmically_across_sample_rates() {
        for sr in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            let mut rf = RhythmicFilter::new(sr);
            let start_hz = 200.0_f32;
            let target_hz = 8_000.0_f32;
            rf.set_parameters(FilterPatch {
                mode: FilterMode::LowPass,
                cutoff_hz: start_hz,
                resonance: 0.25,
                drive: 0.4,
                mix: 1.0,
                env_amount: 0.0,
                ..FilterPatch::default()
            });
            let _ = rf.process([0.1, -0.1], false);
            assert!((rf.cutoff_hz_current() - f64::from(start_hz)).abs() < 1e-6);

            let recomputes_before = rf.coeff_recomputes();
            rf.set_parameters(FilterPatch {
                mode: FilterMode::LowPass,
                cutoff_hz: target_hz,
                resonance: 0.25,
                drive: 0.4,
                mix: 1.0,
                env_amount: 0.0,
                ..FilterPatch::default()
            });

            // After t = tau * ln(2) (tau = 4 ms), a log-domain smoother sits at geometric mean
            // sqrt(200 * 8000) = 1264.9 Hz (not arithmetic midpoint 4100 Hz).
            let half_frames = (0.004 * 2.0_f64.ln() * sr).round() as usize;
            let mut prev = rf.cutoff_hz_current();
            for _ in 0..half_frames {
                let y = rf.process([0.1, -0.1], false);
                assert!(y[0].is_finite() && y[1].is_finite());
                let cur = rf.cutoff_hz_current();
                assert!(cur >= prev - 1e-9 && cur <= f64::from(target_hz) + 1e-6);
                prev = cur;
            }
            let geo_mean = (f64::from(start_hz) * f64::from(target_hz)).sqrt();
            assert!(
                (rf.cutoff_hz_current() - geo_mean).abs() / geo_mean < 0.05,
                "sr={sr}: expected log midpoint ~{geo_mean:.1} Hz, got {:.1} Hz",
                rf.cutoff_hz_current()
            );

            // Settles cleanly to exact target and then stops recomputing coefficients.
            for _ in 0..((sr * 0.15) as usize) {
                let _ = rf.process([0.1, -0.1], false);
            }
            assert!(
                (rf.cutoff_hz_current() - f64::from(target_hz)).abs() < 1e-3,
                "sr={sr}: cutoff did not settle to {target_hz}, got {}",
                rf.cutoff_hz_current()
            );
            assert!(rf.coeff_recomputes() > recomputes_before);
            let settled_count = rf.coeff_recomputes();
            for _ in 0..1024 {
                let _ = rf.process([0.1, -0.1], false);
            }
            assert_eq!(rf.coeff_recomputes(), settled_count);
        }
    }
}
