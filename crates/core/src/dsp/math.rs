//! Pure DSP math primitives: decibel/linear conversions, logarithmic parameter
//! normalization, NaN/denormal sanitization, and 1-pole exponential parameter
//! smoothing. Zero heap allocations and zero I/O.

/// Floor in dB returned by `lin_to_db` / `lin_to_db_f64` for silence or non-finite input.
pub const DB_FLOOR_F32: f32 = -120.0;
pub const DB_FLOOR_F64: f64 = -120.0;

/// Linear amplitude corresponding to `-120.0 dB` (`10^(-120/20) == 1e-6`).
pub const LIN_FLOOR_F32: f32 = 1e-6;
pub const LIN_FLOOR_F64: f64 = 1e-6;

/// Subnormal flush threshold (`< 1e-24 => 0.0`).
pub const DENORMAL_FLOOR_F32: f32 = 1e-24;
pub const DENORMAL_FLOOR_F64: f64 = 1e-24;

/// Natural logarithm of `1000` (`ln(1000)`), yielding `-60 dB` (`1e-3`) attenuation at stage end.
pub const DECAY_LN_60DB: f64 = 6.907_755_278_982_137;
/// Natural logarithm of `100` (`ln(100)`), yielding 99% (`-40 dB`) settling at stage end.
pub const LN_100: f64 = 4.605_170_185_988_092;

/// Maximum safe discrete-time filter/oscillator frequency ratio relative to sample rate (`0.45 * sr`).
pub const NYQUIST_SAFE_RATIO: f64 = 0.45;
/// Drum-kit choke exponential fade-out duration (`3.5 ms` to `-60 dB`).
pub const CHOKE_MS: f64 = 3.5;
/// Fast forced-release duration (`10.0 ms` to `-60 dB`) used for voice stop/steal cleanup.
pub const FAST_RELEASE_MS: f64 = 10.0;
/// Click-free insert effect bypass crossfade time constant (`1.5 ms`).
pub const BYPASS_FADE_MS: f32 = 1.5;

/// Relative settle tolerance for single-precision 1-pole parameter smoothers (`-120 dB` at unity).
pub const SMOOTH_SETTLE_EPS_F32: f32 = 1e-6;
/// Relative settle tolerance for double-precision 1-pole parameter smoothers (`-240 dB` at unity).
pub const SMOOTH_SETTLE_EPS_F64: f64 = 1e-12;

/// Tail silence floor (`-140 dB`, `1e-7`) for idling bypassed time-based effects and
/// quiet filter/resonator tail detectors.
pub const TAIL_SILENCE_FLOOR_F32: f32 = 1e-7;
/// Double-precision tail silence floor (`-140 dB`, `1e-7`).
pub const TAIL_SILENCE_FLOOR_F64: f64 = 1e-7;

/// Bypass crossfade settle cutoff (`-80 dB`, `1e-4`), snapping `bypass_wet` cleanly to `0.0`.
pub const BYPASS_SETTLE_EPS: f64 = 1e-4;

/// Clamp a sample rate to a sane finite range (`1_000..=384_000 Hz`, defaulting to `48_000 Hz`).
#[inline]
pub fn sanitize_sample_rate(sample_rate: f64) -> f64 {
    if sample_rate.is_finite() {
        sample_rate.clamp(1_000.0, 384_000.0)
    } else {
        48_000.0
    }
}

/// Convert decibels to linear amplitude (`10^(db / 20)`). Non-finite values map to `0.0`.
#[inline]
pub fn db_to_lin(db: f32) -> f32 {
    if !db.is_finite() || db <= -300.0 {
        return 0.0;
    }
    10.0_f32.powf(db / 20.0)
}

/// Convert decibels to linear amplitude in `f64` (`10^(db / 20)`). Non-finite values map to `0.0`.
#[inline]
pub fn db_to_lin_f64(db: f64) -> f64 {
    if !db.is_finite() || db <= -300.0 {
        return 0.0;
    }
    10.0_f64.powf(db / 20.0)
}

/// Convert linear amplitude to decibels with a `-120.0 dB` floor.
#[inline]
pub fn lin_to_db(lin: f32) -> f32 {
    if !lin.is_finite() || lin <= LIN_FLOOR_F32 {
        return DB_FLOOR_F32;
    }
    (20.0 * lin.log10()).max(DB_FLOOR_F32)
}

/// Convert linear amplitude to decibels in `f64` with a `-120.0 dB` floor.
#[inline]
pub fn lin_to_db_f64(lin: f64) -> f64 {
    if !lin.is_finite() || lin <= LIN_FLOOR_F64 {
        return DB_FLOOR_F64;
    }
    (20.0 * lin.log10()).max(DB_FLOOR_F64)
}

/// Map a value in `[lo, hi]` logarithmically onto normalized `[0.0, 1.0]`.
#[inline]
pub fn log_to_norm(val: f32, lo: f32, hi: f32) -> f32 {
    let lo = lo.max(f32::MIN_POSITIVE);
    let hi = hi.max(lo);
    if (hi - lo).abs() <= f32::EPSILON {
        return 0.0;
    }
    let clamped = if val.is_finite() {
        val.clamp(lo, hi)
    } else {
        lo
    };
    ((clamped.ln() - lo.ln()) / (hi.ln() - lo.ln())).clamp(0.0, 1.0)
}

/// Map a normalized `[0.0, 1.0]` coordinate logarithmically onto `[lo, hi]`.
#[inline]
pub fn norm_to_log(norm: f32, lo: f32, hi: f32) -> f32 {
    let lo = lo.max(f32::MIN_POSITIVE);
    let hi = hi.max(lo);
    let n = if norm.is_finite() {
        norm.clamp(0.0, 1.0)
    } else {
        0.0
    };
    (lo.ln() + n * (hi.ln() - lo.ln())).exp().clamp(lo, hi)
}

/// Replace `NaN` or `±Inf` with `0.0`.
#[inline]
pub fn sanitize_sample(x: f32) -> f32 {
    if x.is_finite() {
        x
    } else {
        0.0
    }
}

/// Replace `NaN` or `±Inf` with `0.0` in `f64`.
#[inline]
pub fn sanitize_sample_f64(x: f64) -> f64 {
    if x.is_finite() {
        x
    } else {
        0.0
    }
}

/// Replace non-finite values in a stereo `f32` frame with `0.0`.
#[inline]
pub fn sanitize_frame(frame: [f32; 2]) -> [f32; 2] {
    [sanitize_sample(frame[0]), sanitize_sample(frame[1])]
}

/// Sanitize and clamp an `f32` parameter into `[lo, hi]`, replacing non-finite
/// inputs (`NaN`, `±Inf`) with `fallback.clamp(lo, hi)`.
#[inline]
pub fn sanitize_clamp_f32(x: f32, lo: f32, hi: f32, fallback: f32) -> f32 {
    let hi = hi.max(lo);
    if x.is_finite() {
        x.clamp(lo, hi)
    } else {
        fallback.clamp(lo, hi)
    }
}

/// Sanitize and clamp an `f64` parameter into `[lo, hi]`, replacing non-finite
/// inputs (`NaN`, `±Inf`) with `fallback.clamp(lo, hi)`.
#[inline]
pub fn sanitize_clamp_f64(x: f64, lo: f64, hi: f64, fallback: f64) -> f64 {
    let hi = hi.max(lo);
    if x.is_finite() {
        x.clamp(lo, hi)
    } else {
        fallback.clamp(lo, hi)
    }
}

/// Sanitize and clamp a frequency in Hz into `[min_hz, sample_rate * NYQUIST_SAFE_RATIO]`,
/// replacing non-finite inputs with `fallback_hz` clamped to the same safe range.
#[inline]
pub fn clamp_freq_hz(freq_hz: f64, min_hz: f64, sample_rate: f64, fallback_hz: f64) -> f64 {
    let sr = sanitize_sample_rate(sample_rate);
    let lo = if min_hz.is_finite() {
        min_hz.max(0.0)
    } else {
        0.0
    };
    let hi = (sr * NYQUIST_SAFE_RATIO).max(lo);
    sanitize_clamp_f64(freq_hz, lo, hi, fallback_hz)
}

/// Linear dry/wet stereo blend in `f64` with exact endpoint snapping (`mix <= 1e-6` -> `dry`,
/// `mix >= 1 - 1e-6` -> `wet`). Non-finite `mix` defaults to `1.0`.
#[inline]
pub fn mix_stereo_f64(dry: [f64; 2], wet: [f64; 2], mix: f64) -> [f64; 2] {
    let m = sanitize_clamp_f64(mix, 0.0, 1.0, 1.0);
    if m >= 1.0 - LIN_FLOOR_F64 {
        wet
    } else if m <= LIN_FLOOR_F64 {
        dry
    } else {
        let inv = 1.0 - m;
        [dry[0] * inv + wet[0] * m, dry[1] * inv + wet[1] * m]
    }
}

/// Linear dry/wet stereo blend in `f32` with exact endpoint snapping (`mix <= 1e-6` -> `dry`,
/// `mix >= 1 - 1e-6` -> `wet`). Non-finite `mix` defaults to `1.0`.
#[inline]
pub fn mix_stereo_f32(dry: [f32; 2], wet: [f32; 2], mix: f32) -> [f32; 2] {
    let m = sanitize_clamp_f32(mix, 0.0, 1.0, 1.0);
    if m >= 1.0 - LIN_FLOOR_F32 {
        wet
    } else if m <= LIN_FLOOR_F32 {
        dry
    } else {
        let inv = 1.0 - m;
        [dry[0] * inv + wet[0] * m, dry[1] * inv + wet[1] * m]
    }
}

/// Compute a power-of-two bitmask (`stride - 1`, where `stride >= 1`) targeting a
/// sample-rate-invariant control update cadence of `target_hz` Hz.
///
/// Use as `counter = (counter.wrapping_add(1)) & mask` so `counter == 0` ticks every
/// `mask + 1` frames (`44.1 / 48 kHz` keep exact base strides; `96 / 192 / 384 kHz` scale by `2×` per octave).
#[inline]
pub fn control_stride_mask(sample_rate: f64, target_hz: f64) -> u32 {
    let sr = sanitize_sample_rate(sample_rate);
    let hz = sanitize_clamp_f64(target_hz, 1.0, sr, 1_000.0);
    let exp = (sr / hz).max(1.0).log2().round().clamp(0.0, 16.0) as u32;
    (1u32 << exp) - 1
}

/// Flush subnormal/tiny values (`|x| < 1e-24`) and non-finite numbers to `0.0`.
#[inline]
pub fn flush_denormal_f32(x: f32) -> f32 {
    if !x.is_finite() || x.abs() < DENORMAL_FLOOR_F32 {
        0.0
    } else {
        x
    }
}

/// Flush subnormal/tiny values (`|x| < 1e-24`) and non-finite numbers to `0.0` in `f64`.
#[inline]
pub fn flush_denormal_f64(x: f64) -> f64 {
    if !x.is_finite() || x.abs() < DENORMAL_FLOOR_F64 {
        0.0
    } else {
        x
    }
}

/// Convert an interval in octaves to a frequency multiplier (`2^(octaves)`), `f64`.
///
/// Non-finite inputs return unity (`1.0`).
#[inline]
pub fn octaves_to_ratio_f64(octaves: f64) -> f64 {
    if !octaves.is_finite() {
        return 1.0;
    }
    2.0_f64.powf(octaves)
}

/// Convert an interval in octaves to a frequency multiplier (`2^(octaves)`), `f32`.
///
/// Non-finite inputs return unity (`1.0`).
#[inline]
pub fn octaves_to_ratio_f32(octaves: f32) -> f32 {
    if !octaves.is_finite() {
        return 1.0;
    }
    2.0_f32.powf(octaves)
}

/// Convert a pitch interval in semitones to a frequency multiplier (`2^(semitones / 12)`), `f64`.
///
/// Non-finite inputs return unity (`1.0`).
#[inline]
pub fn semitones_to_ratio_f64(semitones: f64) -> f64 {
    if !semitones.is_finite() {
        return 1.0;
    }
    octaves_to_ratio_f64(semitones / 12.0)
}

/// Convert a pitch interval in semitones to a frequency multiplier (`2^(semitones / 12)`), `f32`.
///
/// Non-finite inputs return unity (`1.0`).
#[inline]
pub fn semitones_to_ratio_f32(semitones: f32) -> f32 {
    if !semitones.is_finite() {
        return 1.0;
    }
    octaves_to_ratio_f32(semitones / 12.0)
}

/// Convert MIDI note number (fractional allowed) to frequency in Hz (`A4 = 69 = 440 Hz`).
#[inline]
pub fn midi_to_hz(midi: f64) -> f64 {
    440.0 * semitones_to_ratio_f64(midi - 69.0)
}

/// 4-point Catmull-Rom cubic Hermite interpolation (`f32`) between `x0` (at `frac = 0.0`)
/// and `x1` (at `frac = 1.0`), using surrounding taps `xm1` and `x2`.
#[inline]
pub fn hermite4_f32(xm1: f32, x0: f32, x1: f32, x2: f32, frac: f32) -> f32 {
    let c0 = x0;
    let c1 = 0.5 * (x1 - xm1);
    let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
    let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
    ((c3 * frac + c2) * frac + c1) * frac + c0
}

/// 4-point Catmull-Rom cubic Hermite interpolation (`f64`) between `x0` (at `frac = 0.0`)
/// and `x1` (at `frac = 1.0`), using surrounding taps `xm1` and `x2`.
#[inline]
pub fn hermite4_f64(xm1: f64, x0: f64, x1: f64, x2: f64, frac: f64) -> f64 {
    let c0 = x0;
    let c1 = 0.5 * (x1 - xm1);
    let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
    let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
    ((c3 * frac + c2) * frac + c1) * frac + c0
}

/// Smooth half-cosine (Hann) fade-in gain for normalized `progress ∈ [0.0, 1.0]`.
///
/// Maps `0.0 -> 0.0`, `0.5 -> 0.5`, and `1.0 -> 1.0` with zero slope at both endpoints.
/// Non-finite inputs return `0.0`.
#[inline]
pub fn hann_fade_in(progress: f32) -> f32 {
    if !progress.is_finite() {
        return 0.0;
    }
    let t = f64::from(progress.clamp(0.0, 1.0));
    (0.5 - 0.5 * (std::f64::consts::PI * t).cos()) as f32
}

/// Smooth half-cosine (Hann) fade-out gain for normalized `progress ∈ [0.0, 1.0]`.
///
/// Maps `0.0 -> 1.0`, `0.5 -> 0.5`, and `1.0 -> 0.0` with zero slope at both endpoints.
/// Non-finite inputs return `0.0`.
#[inline]
pub fn hann_fade_out(progress: f32) -> f32 {
    if !progress.is_finite() {
        return 0.0;
    }
    let t = f64::from(progress.clamp(0.0, 1.0));
    (0.5 * (1.0 + (std::f64::consts::PI * t).cos())) as f32
}

/// One-pole exponential step weight `alpha = 1 - exp(-1 / (tau * sr))` for time constant `ms` ms.
///
/// Used as `state += (target - state) * alpha`. Returns `1.0` (instant snap) when `ms <= 0.0`.
#[inline]
pub fn one_pole_coeff_ms(ms: f32, sample_rate: f64) -> f64 {
    let sr = sanitize_sample_rate(sample_rate);
    if !ms.is_finite() || ms <= 0.0 {
        return 1.0;
    }
    let tau_samples = f64::from(ms) * 0.001 * sr;
    (1.0 - (-1.0 / tau_samples).exp()).clamp(0.0, 1.0)
}

/// One-pole exponential decay multiplier `exp(-1 / (tau * sr))` for time constant `ms` ms.
///
/// Used as `state = decay * state + (1 - decay) * target`. Returns `0.0` when `ms <= 0.0`.
#[inline]
pub fn one_pole_decay_ms(ms: f32, sample_rate: f64) -> f64 {
    let sr = sanitize_sample_rate(sample_rate);
    if !ms.is_finite() || ms <= 0.0 {
        return 0.0;
    }
    let tau_samples = f64::from(ms) * 0.001 * sr;
    (-1.0 / tau_samples).exp().clamp(0.0, 1.0)
}

/// Exponential portamento/glide step coefficient settling 99% (`-40 dB`, `ln(100)`) in `ms` ms.
///
/// Used as `pitch += (target - pitch) * coeff`. Returns `1.0` (instant snap) when `ms <= 0.0`.
#[inline]
pub fn glide_coeff_ms(ms: f32, sample_rate: f64) -> f64 {
    let sr = sanitize_sample_rate(sample_rate);
    if !ms.is_finite() || ms <= 0.0 {
        return 1.0;
    }
    let frames = f64::from(ms) * 0.001 * sr;
    (1.0 - (-LN_100 / frames).exp()).clamp(0.0, 1.0)
}

/// Per-sample exponential decay multiplier reaching `-60 dB` (`1e-3`) in `ms` milliseconds.
///
/// Used as `level *= coeff` (or `level = target + (level - target) * coeff`).
/// Clamps `ms` to `>= 0.1` ms (defaulting non-finite values to `0.1` ms) and rounds the
/// stage length to at least `1.0` sample frame at [`sanitize_sample_rate(sr)`].
#[inline]
pub fn decay_coeff_60db_ms(ms: f64, sr: f64) -> f64 {
    let sr = sanitize_sample_rate(sr);
    let clamped_ms = if ms.is_finite() { ms.max(0.1) } else { 0.1 };
    let frames = (clamped_ms * 0.001 * sr).round().max(1.0);
    (-DECAY_LN_60DB / frames).exp()
}

/// Zero-allocation 1-pole exponential parameter smoother (`f64`).
#[derive(Clone, Copy, Debug)]
pub struct Smoother {
    current: f64,
    target: f64,
    coeff: f64,
}

/// Advance a single-precision 1-pole smoother toward `target` with step coefficient `coeff`,
/// snapping exactly to `target` once within relative tolerance or upon IEEE-754 ULP stall.
///
/// Prevents permanent subnormal stalls approaching `0.0`, prevents ULP stalls short of `1.0`,
/// and guarantees bit-stable settled values for downstream parameter caches.
#[inline]
pub fn smooth_step_f32(current: &mut f32, target: f32, coeff: f32) {
    let target = sanitize_sample(target);
    if *current == target {
        return;
    }
    let diff = target - *current;
    if !diff.is_finite() || diff.abs() <= SMOOTH_SETTLE_EPS_F32 * target.abs().max(1.0) {
        *current = target;
        return;
    }
    let c = if coeff.is_finite() {
        coeff.clamp(0.0, 1.0)
    } else {
        1.0
    };
    let next = *current + diff * c;
    *current = if c > 0.0 && next == *current {
        target
    } else {
        next
    };
}

/// Advance a double-precision 1-pole smoother toward `target` with step coefficient `coeff`,
/// snapping exactly to `target` once within relative tolerance or upon IEEE-754 ULP stall.
#[inline]
pub fn smooth_step_f64(current: &mut f64, target: f64, coeff: f64) {
    let target = sanitize_sample_f64(target);
    if *current == target {
        return;
    }
    let diff = target - *current;
    if !diff.is_finite() || diff.abs() <= SMOOTH_SETTLE_EPS_F64 * target.abs().max(1.0) {
        *current = target;
        return;
    }
    let c = if coeff.is_finite() {
        coeff.clamp(0.0, 1.0)
    } else {
        1.0
    };
    let next = *current + diff * c;
    *current = if c > 0.0 && next == *current {
        target
    } else {
        next
    };
}

impl Smoother {
    const SETTLE_EPS: f64 = SMOOTH_SETTLE_EPS_F64;

    /// Create a smoother initialized at `initial` with time constant `time_ms` ms at `sample_rate`.
    #[inline]
    pub fn new(initial: f64, time_ms: f32, sample_rate: f64) -> Self {
        let init = sanitize_sample_f64(initial);
        Self {
            current: init,
            target: init,
            coeff: one_pole_coeff_ms(time_ms, sample_rate),
        }
    }

    /// Snap both current and target values immediately (click-free init/reset).
    #[inline]
    pub fn snap(&mut self, value: f64) {
        let v = sanitize_sample_f64(value);
        self.current = v;
        self.target = v;
    }

    /// Set a new target value to approach exponentially.
    #[inline]
    pub fn set_target(&mut self, value: f64) {
        self.target = sanitize_sample_f64(value);
    }

    /// Read the configured target value.
    #[inline]
    pub fn target(&self) -> f64 {
        self.target
    }

    #[inline]
    fn settle_tol(&self) -> f64 {
        Self::SETTLE_EPS * self.target.abs().max(1.0)
    }

    /// Advance by one sample and return the smoothed value.
    #[allow(clippy::should_implement_trait)]
    #[inline]
    pub fn next(&mut self) -> f64 {
        if self.current == self.target {
            return self.current;
        }
        let diff = self.target - self.current;
        if diff.abs() <= self.settle_tol() {
            self.current = self.target;
        } else {
            let next = self.current + diff * self.coeff;
            self.current = if self.coeff > 0.0 && next == self.current {
                self.target
            } else {
                next
            };
        }
        self.current
    }

    /// Current smoothed value without advancing state.
    #[inline]
    pub fn current(&self) -> f64 {
        self.current
    }

    /// Whether the smoother has converged to its target.
    #[inline]
    pub fn is_settled(&self) -> bool {
        (self.target - self.current).abs() <= self.settle_tol()
    }
}

/// Per-frame state returned by [`BypassFader::step`] for unified click-free effect bypass.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BypassStep {
    /// Effect is already idle and bypassed (`0%` DSP processing needed).
    Idle,
    /// Effect is bypassed and its crossfade has settled at `0.0`:
    /// instant-reset effects clear state and call [`BypassFader::mark_idle`] immediately,
    /// while tail-draining effects (`Delay`, `Reverb`) keep feeding silence until quiet
    /// and then call `mark_idle()`.
    SettledBypass,
    /// Effect is active or crossfading with smoothed wet weight in `(0.0, 1.0]`.
    Active(f64),
}

/// Shared click-free `1.5 ms` bypass crossfader and idle state machine composed by all
/// 8 native [`crate::dsp::StereoEffect`] processors.
#[derive(Clone, Copy, Debug)]
pub struct BypassFader {
    wet: Smoother,
    was_active: bool,
}

impl BypassFader {
    #[inline]
    pub fn new(sample_rate: f64) -> Self {
        Self {
            wet: Smoother::new(1.0, BYPASS_FADE_MS, sample_rate),
            was_active: false,
        }
    }

    #[inline]
    pub fn reset(&mut self) {
        self.wet.snap(1.0);
        self.was_active = false;
    }

    #[inline]
    pub fn is_idle(&self) -> bool {
        !self.was_active
    }

    /// Transition cleanly to idle-bypassed state after resetting internal effect buffers.
    #[inline]
    pub fn mark_idle(&mut self) {
        self.wet.snap(0.0);
        self.was_active = false;
    }

    /// Advance bypass state by one audio sample.
    #[inline]
    pub fn step(&mut self, bypassed: bool) -> BypassStep {
        if bypassed && !self.was_active {
            self.wet.snap(0.0);
            return BypassStep::Idle;
        }
        self.wet.set_target(if bypassed { 0.0 } else { 1.0 });
        let w = self.wet.next();
        if bypassed && w <= BYPASS_SETTLE_EPS {
            self.wet.snap(0.0);
            return BypassStep::SettledBypass;
        }
        self.was_active = true;
        BypassStep::Active(w)
    }
}

/// Zero-allocation 1-pole smoother operating in the natural-logarithm domain (`f64`).
///
/// Ideal for perceptual frequency and time parameters (Hz, ms) where exponential
/// glides move linearly in pitch/octaves. Clamps inputs to `[min_val, max_val]`
/// (with `min_val > 0.0`) before taking `ln()`, preventing `-Inf`/`NaN` states,
/// and caches the linear value so settled reads cost zero `exp()` calls.
#[derive(Clone, Copy, Debug)]
pub struct LogSmoother {
    current_log: f64,
    target_log: f64,
    current_val: f64,
    min_val: f64,
    max_val: f64,
}

impl LogSmoother {
    const SETTLE_EPS: f64 = SMOOTH_SETTLE_EPS_F64;

    /// Create a log-domain smoother with lower bound `min_val > 0` and no upper cap.
    #[inline]
    pub fn new(initial: f64, min_val: f64) -> Self {
        Self::new_clamped(initial, min_val, f64::INFINITY)
    }

    /// Create a log-domain smoother clamped strictly inside `[min_val, max_val]`.
    #[inline]
    pub fn new_clamped(initial: f64, min_val: f64, max_val: f64) -> Self {
        let min_v = if min_val.is_finite() {
            min_val.max(f64::MIN_POSITIVE)
        } else {
            f64::MIN_POSITIVE
        };
        let max_v = if max_val.is_nan() {
            f64::INFINITY
        } else {
            max_val.max(min_v)
        };
        let mut s = Self {
            current_log: 0.0,
            target_log: 0.0,
            current_val: min_v,
            min_val: min_v,
            max_val: max_v,
        };
        s.snap(initial);
        s
    }

    #[inline]
    fn clamp_val(&self, value: f64) -> f64 {
        if value.is_finite() {
            value.clamp(self.min_val, self.max_val)
        } else {
            self.min_val
        }
    }

    /// Set a new target value (linear domain) to approach exponentially in log space.
    #[inline]
    pub fn set_target(&mut self, value: f64) {
        self.target_log = self.clamp_val(value).ln();
    }

    /// Snap both current and target values immediately (click-free init/reset).
    #[inline]
    pub fn snap(&mut self, value: f64) {
        let clamped = self.clamp_val(value);
        let lg = clamped.ln();
        self.current_log = lg;
        self.target_log = lg;
        self.current_val = clamped;
    }

    /// Snap current state directly to the already-configured target without recomputing `ln()`.
    #[inline]
    pub fn snap_to_target(&mut self) {
        if self.current_log != self.target_log {
            self.current_log = self.target_log;
            self.current_val = self.current_log.exp();
        }
    }

    /// Advance by one sample using one-pole step coefficient `coeff` (`0.0..=1.0`)
    /// and return the current linear-domain value.
    #[inline]
    pub fn step(&mut self, coeff: f64) -> f64 {
        if self.current_log == self.target_log {
            return self.current_val;
        }
        let diff = self.target_log - self.current_log;
        let tol = Self::SETTLE_EPS * self.target_log.abs().max(1.0);
        if diff.abs() <= tol {
            self.current_log = self.target_log;
        } else {
            let c = if coeff.is_finite() {
                coeff.clamp(0.0, 1.0)
            } else {
                1.0
            };
            let next = self.current_log + diff * c;
            self.current_log = if c > 0.0 && next == self.current_log {
                self.target_log
            } else {
                next
            };
        }
        self.current_val = self.current_log.exp();
        self.current_val
    }

    /// Current linear-domain smoothed value without advancing state.
    #[inline]
    pub const fn current(&self) -> f64 {
        self.current_val
    }

    /// Configured target value in the linear domain.
    #[inline]
    pub fn target(&self) -> f64 {
        self.target_log.exp()
    }

    /// Whether the smoother has converged to its target.
    #[inline]
    pub fn is_settled(&self) -> bool {
        (self.target_log - self.current_log).abs()
            <= Self::SETTLE_EPS * self.target_log.abs().max(1.0)
    }
}

/// Zero-allocation peak-hold and exponential-decay meter state machine (`f32`).
///
/// Latches onto new peaks (`>= value && > 0.0`) for `hold_frames` samples, then
/// decays exponentially by `hold_decay` per sample without dropping below `floor`,
/// snapping to `0.0` below `zero_floor`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PeakHold {
    value: f32,
    timer: u32,
}

impl PeakHold {
    /// Default `-120 dBFS` linear meter floor below which held peaks snap cleanly to `0.0`.
    pub const DEFAULT_ZERO_FLOOR: f32 = 1e-6;

    #[inline]
    pub const fn new() -> Self {
        Self {
            value: 0.0,
            timer: 0,
        }
    }

    #[inline]
    pub fn reset(&mut self) {
        self.value = 0.0;
        self.timer = 0;
    }

    /// Current held peak value.
    #[inline]
    pub const fn value(self) -> f32 {
        self.value
    }

    /// Observe one non-negative sample `peak`, holding for `hold_frames` before
    /// decaying by `hold_decay` toward `floor` and snapping to `0.0` below `zero_floor`.
    #[inline]
    pub fn observe(
        &mut self,
        peak: f32,
        floor: f32,
        hold_frames: u32,
        hold_decay: f32,
        zero_floor: f32,
    ) -> f32 {
        let peak = if peak.is_finite() { peak.max(0.0) } else { 0.0 };
        let floor = if floor.is_finite() {
            floor.max(0.0)
        } else {
            0.0
        };
        if peak >= self.value && peak > 0.0 {
            self.value = peak;
            self.timer = hold_frames;
        } else if self.timer > 0 {
            self.timer -= 1;
        } else {
            let decayed = (self.value * hold_decay).max(floor);
            self.value = if decayed < zero_floor { 0.0 } else { decayed };
        }
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_linear_roundtrip_and_floor() {
        for db in [-96.0_f32, -24.0, -6.0, 0.0, 6.0, 18.0] {
            let lin = db_to_lin(db);
            assert!((lin_to_db(lin) - db).abs() < 1e-4, "db={db}");
            let lin64 = db_to_lin_f64(f64::from(db));
            assert!((lin_to_db_f64(lin64) - f64::from(db)).abs() < 1e-9);
        }
        assert_eq!(lin_to_db(0.0), DB_FLOOR_F32);
        assert_eq!(lin_to_db(-1.0), DB_FLOOR_F32);
        assert_eq!(lin_to_db(f32::NAN), DB_FLOOR_F32);
        assert_eq!(lin_to_db_f64(0.0), DB_FLOOR_F64);
        assert_eq!(db_to_lin(f32::NAN), 0.0);
        assert_eq!(db_to_lin(f32::INFINITY), 0.0);
        assert!((db_to_lin(0.0) - 1.0).abs() < 1e-7);
        assert!((db_to_lin(-6.0206) - 0.5).abs() < 1e-4);
    }

    #[test]
    fn log_norm_roundtrip_across_bounds() {
        let lo = 20.0_f32;
        let hi = 20_000.0_f32;
        for n in [0.0_f32, 0.25, 0.5, 0.75, 1.0] {
            let v = norm_to_log(n, lo, hi);
            let rt = log_to_norm(v, lo, hi);
            assert!((rt - n).abs() < 1e-5, "n={n}, v={v}, rt={rt}");
        }
        assert!((norm_to_log(0.5, 100.0, 10_000.0) - 1000.0).abs() < 1e-3);
        assert_eq!(log_to_norm(f32::NAN, lo, hi), 0.0);
        assert_eq!(norm_to_log(f32::NAN, lo, hi), lo);
    }

    #[test]
    fn sanitize_and_denormal_flush() {
        assert_eq!(sanitize_sample(f32::NAN), 0.0);
        assert_eq!(sanitize_sample(f32::INFINITY), 0.0);
        assert_eq!(sanitize_sample(f32::NEG_INFINITY), 0.0);
        assert_eq!(sanitize_sample(0.25), 0.25);

        assert_eq!(flush_denormal_f32(1e-25), 0.0);
        assert_eq!(flush_denormal_f32(-1e-30), 0.0);
        assert_eq!(flush_denormal_f32(f32::NAN), 0.0);
        assert_eq!(flush_denormal_f32(1e-10), 1e-10);

        assert_eq!(flush_denormal_f64(1e-25), 0.0);
        assert_eq!(flush_denormal_f64(f64::INFINITY), 0.0);
        assert_eq!(flush_denormal_f64(1e-18), 1e-18);
    }

    #[test]
    fn smoother_converges_with_sample_rate_invariant_time_constant() {
        for sr in [44_100.0, 48_000.0, 96_000.0] {
            let mut sm = Smoother::new(0.0, 10.0, sr);
            assert!(sm.is_settled());
            sm.set_target(1.0);
            assert!(!sm.is_settled());
            let tau_samples = (0.010 * sr).round() as usize;
            let mut val = 0.0;
            for _ in 0..tau_samples {
                val = sm.next();
            }
            // After 1 time constant, exponential step reaches 1 - 1/e ~= 0.6321.
            let expected = 1.0 - (-1.0_f64).exp();
            assert!(
                (val - expected).abs() < 0.005,
                "sr={sr}: val={val}, expected={expected}"
            );
            // After 30 time constants it settles completely.
            for _ in 0..(tau_samples * 30) {
                sm.next();
            }
            assert!(sm.is_settled());
            assert_eq!(sm.current(), 1.0);

            sm.snap(0.25);
            assert!(sm.is_settled());
            assert_eq!(sm.current(), 0.25);
        }
    }

    #[test]
    fn decay_coeff_60db_reaches_minus_60db_and_clamps_invalid_inputs() {
        for sr in [44_100.0, 48_000.0, 96_000.0] {
            for ms in [3.5, 10.0, 120.0] {
                let coeff = decay_coeff_60db_ms(ms, sr);
                assert!(
                    coeff > 0.0 && coeff < 1.0,
                    "ms={ms}, sr={sr}, coeff={coeff}"
                );
                let frames = (ms * 0.001 * sr).round().max(1.0) as i32;
                let end_level = coeff.powi(frames);
                assert!(
                    (end_level - 1e-3).abs() < 1e-12,
                    "ms={ms}, sr={sr}: expected 1e-3 (-60 dB), got {end_level}"
                );
            }
        }
        // Non-finite / non-positive ms clamp to 0.1 ms cleanly; NaN sr falls back to 48 kHz.
        let min_48k = decay_coeff_60db_ms(0.1, 48_000.0);
        assert_eq!(decay_coeff_60db_ms(0.0, 48_000.0), min_48k);
        assert_eq!(decay_coeff_60db_ms(-5.0, 48_000.0), min_48k);
        assert_eq!(decay_coeff_60db_ms(f64::NAN, 48_000.0), min_48k);
        assert_eq!(
            decay_coeff_60db_ms(10.0, f64::NAN),
            decay_coeff_60db_ms(10.0, 48_000.0)
        );
    }

    #[test]
    fn pitch_and_octave_ratios_are_exact_and_guard_non_finite() {
        assert!((octaves_to_ratio_f64(0.0) - 1.0).abs() < 1e-12);
        assert!((octaves_to_ratio_f64(1.0) - 2.0).abs() < 1e-12);
        assert!((octaves_to_ratio_f64(-1.0) - 0.5).abs() < 1e-12);
        assert!((octaves_to_ratio_f32(2.0) - 4.0).abs() < 1e-6);
        assert!((semitones_to_ratio_f64(12.0) - 2.0).abs() < 1e-12);
        assert!((semitones_to_ratio_f64(-12.0) - 0.5).abs() < 1e-12);
        assert!((semitones_to_ratio_f32(7.0) - 2.0_f32.powf(7.0 / 12.0)).abs() < 1e-6);
        assert_eq!(octaves_to_ratio_f64(f64::NAN), 1.0);
        assert_eq!(octaves_to_ratio_f32(f32::INFINITY), 1.0);
        assert_eq!(semitones_to_ratio_f64(f64::NEG_INFINITY), 1.0);
        assert_eq!(semitones_to_ratio_f32(f32::NAN), 1.0);
        assert!((midi_to_hz(69.0) - 440.0).abs() < 1e-12);
        assert!((midi_to_hz(81.0) - 880.0).abs() < 1e-12);
    }

    #[test]
    fn hermite4_reproduces_endpoints_and_cubic_polynomials_exactly() {
        // Catmull-Rom reproduces linear functions exactly and hits x0 at t=0, x1 at t=1.
        for t in [0.0_f64, 0.25, 0.5, 0.75, 1.0] {
            let v64 = hermite4_f64(-1.0, 0.0, 1.0, 2.0, t);
            assert!((v64 - t).abs() < 1e-12, "t={t}: got {v64}");
            let v32 = hermite4_f32(-1.0, 0.0, 1.0, 2.0, t as f32);
            assert!((v32 - t as f32).abs() < 1e-6, "t={t}: got {v32}");
        }
        // Endpoints match x0 and x1 for arbitrary non-collinear taps.
        assert_eq!(hermite4_f64(0.3, -0.7, 0.9, -0.2, 0.0), -0.7);
        assert!((hermite4_f64(0.3, -0.7, 0.9, -0.2, 1.0) - 0.9).abs() < 1e-12);
        assert_eq!(hermite4_f32(0.3, -0.7, 0.9, -0.2, 0.0), -0.7);
        assert!((hermite4_f32(0.3, -0.7, 0.9, -0.2, 1.0) - 0.9).abs() < 1e-6);
    }

    #[test]
    fn hann_fade_in_and_out_endpoints_symmetry_and_clamping() {
        assert_eq!(hann_fade_in(0.0), 0.0);
        assert!((hann_fade_in(0.5) - 0.5).abs() < 1e-6);
        assert!((hann_fade_in(1.0) - 1.0).abs() < 1e-7);
        assert_eq!(hann_fade_in(-0.5), 0.0);
        assert!((hann_fade_in(1.5) - 1.0).abs() < 1e-7);
        assert_eq!(hann_fade_in(f32::NAN), 0.0);

        assert!((hann_fade_out(0.0) - 1.0).abs() < 1e-7);
        assert!((hann_fade_out(0.5) - 0.5).abs() < 1e-6);
        assert_eq!(hann_fade_out(1.0), 0.0);
        // Clamps out-of-range inputs and sanitizes NaN.
        assert!((hann_fade_out(-0.5) - 1.0).abs() < 1e-7);
        assert_eq!(hann_fade_out(1.5), 0.0);
        assert_eq!(hann_fade_out(f32::NAN), 0.0);
        // Symmetric around 0.5 and complementary: hann_fade_in(t) + hann_fade_out(t) == 1.0.
        for t in [0.0_f32, 0.1, 0.25, 0.37, 0.5, 0.75, 1.0] {
            assert!((hann_fade_out(t) + hann_fade_out(1.0 - t) - 1.0).abs() < 1e-6);
            assert!((hann_fade_in(t) + hann_fade_out(t) - 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn log_smoother_glides_geometrically_clamps_bounds_and_settles() {
        let mut sm = LogSmoother::new_clamped(100.0, 20.0, 10_000.0);
        assert!(sm.is_settled());
        assert_eq!(sm.current(), 100.0);

        // Moving 50% of the way in log-space from 100 Hz to 10_000 Hz lands on geometric mean 1_000 Hz.
        sm.set_target(10_000.0);
        assert!(!sm.is_settled());
        let mid = sm.step(0.5);
        assert!(
            (mid - 1_000.0).abs() < 1e-9,
            "expected geometric mean 1000.0, got {mid}"
        );
        assert_eq!(sm.current(), mid);

        // Converges cleanly to target.
        let coeff = one_pole_coeff_ms(5.0, 48_000.0);
        for _ in 0..20_000 {
            sm.step(coeff);
        }
        assert!(sm.is_settled());
        assert!((sm.current() - 10_000.0).abs() < 1e-6);

        // Enforces min/max clamps and NaN safety.
        sm.snap(1.0);
        assert_eq!(sm.current(), 20.0);
        sm.set_target(50_000.0);
        sm.snap_to_target();
        assert!((sm.current() - 10_000.0).abs() < 1e-9);
        sm.snap(f64::NAN);
        assert_eq!(sm.current(), 20.0);
    }

    #[test]
    fn peak_hold_latches_holds_decays_to_floor_and_zeros() {
        let mut hold = PeakHold::new();
        assert_eq!(hold.value(), 0.0);

        // Latch peak 0.8 for 3 frames.
        assert_eq!(hold.observe(0.8, 0.8, 3, 0.5, 1e-4), 0.8);
        for _ in 0..3 {
            assert_eq!(hold.observe(0.1, 0.1, 3, 0.5, 1e-4), 0.8);
        }
        // Hold expired: decay by 0.5 per frame, never dropping below floor (0.1).
        assert!((hold.observe(0.1, 0.1, 3, 0.5, 1e-4) - 0.4).abs() < 1e-6);
        assert!((hold.observe(0.1, 0.1, 3, 0.5, 1e-4) - 0.2).abs() < 1e-6);
        assert!((hold.observe(0.1, 0.1, 3, 0.5, 1e-4) - 0.1).abs() < 1e-6);
        assert!((hold.observe(0.0, 0.1, 3, 0.5, 1e-4) - 0.1).abs() < 1e-6);

        // With floor=0, decays cleanly until snapping to 0.0 below zero_floor.
        for _ in 0..32 {
            hold.observe(0.0, 0.0, 3, 0.5, 1e-4);
        }
        assert_eq!(hold.value(), 0.0);

        // Non-finite input does not poison state, and reset clears immediately.
        assert_eq!(hold.observe(0.5, 0.5, 10, 0.9, 1e-4), 0.5);
        assert_eq!(hold.observe(f32::NAN, f32::NAN, 10, 0.9, 1e-4), 0.5);
        hold.reset();
        assert_eq!(hold.value(), 0.0);
    }

    #[test]
    fn smoothers_settle_cleanly_without_subnormal_or_ulp_stalls() {
        // Smoother targeting high-magnitude values (e.g. 18 kHz) settles cleanly across sample rates.
        for sr in [44_100.0_f64, 48_000.0, 192_000.0] {
            let mut sm = Smoother::new(600.0, 4.0, sr);
            sm.set_target(18_000.0);
            let max_frames = (sr * 0.25) as usize;
            for _ in 0..max_frames {
                sm.next();
            }
            assert!(
                sm.is_settled() && sm.current() == 18_000.0,
                "Smoother stalled at sr={sr}: current={} target={}",
                sm.current(),
                sm.target()
            );
        }

        // LogSmoother settles cleanly even at 384 kHz.
        let mut lsm = LogSmoother::new_clamped(600.0, 20.0, 20_000.0);
        lsm.set_target(18_000.0);
        let coeff_384 = one_pole_coeff_ms(5.0, 384_000.0);
        for _ in 0..(384_000 / 2) {
            lsm.step(coeff_384);
        }
        assert!(lsm.is_settled());

        // smooth_step_f32 snaps cleanly to 0.0 (no subnormal tail) and 1.0 (no ULP stall).
        for sr in [48_000.0_f64, 192_000.0] {
            let s64 = one_pole_coeff_ms(5.0, sr);
            let s32 = s64 as f32;
            let frames = (sr * 0.2) as usize;

            let mut to_zero = 1.0_f32;
            for _ in 0..frames {
                smooth_step_f32(&mut to_zero, 0.0, s32);
            }
            assert_eq!(to_zero, 0.0, "smooth_step_f32 stalled toward 0 at sr={sr}");

            let mut to_one = 0.5_f32;
            for _ in 0..frames {
                smooth_step_f32(&mut to_one, 1.0, s32);
            }
            assert_eq!(to_one, 1.0, "smooth_step_f32 stalled toward 1 at sr={sr}");

            let mut d_zero = 1.0_f64;
            let mut d_hi = 600.0_f64;
            for _ in 0..frames {
                smooth_step_f64(&mut d_zero, 0.0, s64);
                smooth_step_f64(&mut d_hi, 18_000.0, s64);
            }
            assert_eq!(d_zero, 0.0, "smooth_step_f64 stalled toward 0 at sr={sr}");
            assert_eq!(
                d_hi, 18_000.0,
                "smooth_step_f64 stalled toward 18000 at sr={sr}"
            );
        }
    }

    #[test]
    fn sanitize_clamp_guards_non_finite_and_out_of_range_inputs() {
        assert_eq!(sanitize_clamp_f32(0.5, 0.0, 1.0, 0.25), 0.5);
        assert_eq!(sanitize_clamp_f32(-5.0, 0.0, 1.0, 0.25), 0.0);
        assert_eq!(sanitize_clamp_f32(5.0, 0.0, 1.0, 0.25), 1.0);
        assert_eq!(sanitize_clamp_f32(f32::NAN, 0.0, 1.0, 0.25), 0.25);
        assert_eq!(sanitize_clamp_f32(f32::INFINITY, 0.0, 1.0, 2.0), 1.0);
        assert_eq!(sanitize_clamp_f32(f32::NEG_INFINITY, 0.0, 1.0, -2.0), 0.0);

        assert_eq!(sanitize_clamp_f64(440.0, 20.0, 20_000.0, 1_000.0), 440.0);
        assert_eq!(
            sanitize_clamp_f64(f64::NAN, 20.0, 20_000.0, 1_000.0),
            1_000.0
        );
        assert_eq!(
            sanitize_clamp_f64(f64::INFINITY, 20.0, 20_000.0, 1_000.0),
            1_000.0
        );
        assert_eq!(
            sanitize_clamp_f64(f64::NEG_INFINITY, 20.0, 20_000.0, 5.0),
            20.0
        );
    }

    #[test]
    fn clamp_freq_mix_stereo_control_stride_and_bypass_fader_work_across_sample_rates() {
        assert_eq!(
            clamp_freq_hz(50_000.0, 20.0, 48_000.0, 1_000.0),
            48_000.0 * NYQUIST_SAFE_RATIO
        );
        assert_eq!(clamp_freq_hz(f64::NAN, 20.0, 48_000.0, 1_000.0), 1_000.0);
        assert_eq!(clamp_freq_hz(5.0, 20.0, 48_000.0, 1_000.0), 20.0);

        assert_eq!(mix_stereo_f64([1.0, -1.0], [0.2, 0.4], 0.0), [1.0, -1.0]);
        assert_eq!(mix_stereo_f64([1.0, -1.0], [0.2, 0.4], 1.0), [0.2, 0.4]);
        assert_eq!(mix_stereo_f64([1.0, -1.0], [0.2, 0.4], 0.5), [0.6, -0.3]);
        assert_eq!(
            mix_stereo_f64([1.0, -1.0], [0.2, 0.4], f64::NAN),
            [0.2, 0.4]
        );
        assert_eq!(mix_stereo_f32([1.0, -1.0], [0.0, 0.0], 0.5), [0.5, -0.5]);

        // Control stride masks double cleanly per sample-rate octave.
        assert_eq!(control_stride_mask(44_100.0, 750.0), 63);
        assert_eq!(control_stride_mask(48_000.0, 750.0), 63);
        assert_eq!(control_stride_mask(96_000.0, 750.0), 127);
        assert_eq!(control_stride_mask(192_000.0, 750.0), 255);
        assert_eq!(control_stride_mask(48_000.0, 3_000.0), 15);
        assert_eq!(control_stride_mask(192_000.0, 3_000.0), 63);
        assert_eq!(control_stride_mask(48_000.0, 12_000.0), 3);
        assert_eq!(control_stride_mask(192_000.0, 12_000.0), 15);

        // BypassFader starts idle on initial bypass, ramps smoothly when bypassed after activity,
        // and reaches SettledBypass cleanly across sample rates.
        for sr in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            let mut bf = BypassFader::new(sr);
            assert!(bf.is_idle());
            assert_eq!(bf.step(true), BypassStep::Idle);
            assert!(matches!(bf.step(false), BypassStep::Active(w) if w > 0.0));
            assert!(!bf.is_idle());

            let mut settled = false;
            for _ in 0..((sr * 0.03) as usize) {
                if bf.step(true) == BypassStep::SettledBypass {
                    settled = true;
                    bf.mark_idle();
                    break;
                }
            }
            assert!(settled, "sr={sr}: BypassFader did not settle");
            assert!(bf.is_idle());
            assert_eq!(bf.step(true), BypassStep::Idle);
        }
    }
}
