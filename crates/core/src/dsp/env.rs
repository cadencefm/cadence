//! Exponential ADSR, hold-decay, and 0-Hz cubic-capacitor voice envelopes,
//! UI contour curve evaluation, and sample-accurate rhythmic step envelopes.

use crate::dsp::cubic::ZeroHzCapacitor;
use crate::dsp::math::{
    decay_coeff_60db_ms, one_pole_coeff_ms, sanitize_clamp_f64, sanitize_sample_f64,
    sanitize_sample_rate, CHOKE_MS, DECAY_LN_60DB, LN_100, TAIL_SILENCE_FLOOR_F64,
};
use crate::filter_patterns::CompiledFilterPattern;
use crate::rack::TriggerMode;

const ENV_SILENCE: f64 = TAIL_SILENCE_FLOOR_F64;

/// Stage of an [`AdsrState`] envelope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdsrStage {
    Attack,
    Decay,
    Sustain,
    Release,
    Done,
}

/// Zero-allocation exponential ADSR envelope supporting `TriggerMode::OneShot`,
/// click-free retriggering, legato progress adoption, and 3.5 ms drum-kit chokes.
#[derive(Clone, Copy, Debug)]
pub struct AdsrState {
    stage: AdsrStage,
    level: f64,
    remaining: u64,
    attack_frames: u64,
    decay_frames: u64,
    release_frames: u64,
    decay_coeff: f64,
    release_coeff: f64,
    one_shot: bool,
}

impl AdsrState {
    fn ms_to_frames(sr: f64, ms: f32) -> u64 {
        let clamped = if ms.is_finite() { ms.max(0.1) } else { 0.1 };
        (f64::from(clamped) * 0.001 * sr).round().max(1.0) as u64
    }

    /// Create a freshly triggered envelope starting from `level = 0.0`.
    ///
    /// Both `TriggerMode::OneShot` and `TriggerMode::Additive` run as one-shot
    /// attack-decay envelopes (with continuous non-resetting retrigger from the
    /// current level via [`AdsrState::retrigger`]).
    pub fn new(
        sample_rate: f64,
        attack_ms: f32,
        decay_ms: f32,
        release_ms: f32,
        trigger_mode: TriggerMode,
    ) -> Self {
        Self::new_with_one_shot(
            sample_rate,
            attack_ms,
            decay_ms,
            release_ms,
            matches!(trigger_mode, TriggerMode::OneShot | TriggerMode::Additive),
        )
    }

    /// Create a freshly triggered envelope with explicit `one_shot` flag.
    pub fn new_with_one_shot(
        sample_rate: f64,
        attack_ms: f32,
        decay_ms: f32,
        release_ms: f32,
        one_shot: bool,
    ) -> Self {
        let sr = sanitize_sample_rate(sample_rate);
        let attack_frames = Self::ms_to_frames(sr, attack_ms);
        let decay_frames = Self::ms_to_frames(sr, decay_ms);
        let release_frames = Self::ms_to_frames(sr, release_ms);
        Self {
            stage: AdsrStage::Attack,
            level: 0.0,
            remaining: attack_frames,
            attack_frames,
            decay_frames,
            release_frames,
            decay_coeff: decay_coeff_60db_ms(f64::from(decay_ms), sr),
            release_coeff: decay_coeff_60db_ms(f64::from(release_ms), sr),
            one_shot,
        }
    }

    /// Zero-click retrigger: enters `Attack` from the current `level` without
    /// resetting `level` to `0.0` (eliminates non-legato mono retrigger pops).
    pub fn retrigger(
        &mut self,
        sample_rate: f64,
        attack_ms: f32,
        decay_ms: f32,
        release_ms: f32,
        trigger_mode: TriggerMode,
    ) {
        let prev_level = self.level.clamp(0.0, 1.0);
        *self = Self::new(sample_rate, attack_ms, decay_ms, release_ms, trigger_mode);
        self.level = prev_level;
    }

    /// Enter the `Release` stage (no-op if already releasing or done).
    #[inline]
    pub fn release(&mut self) {
        if !matches!(self.stage, AdsrStage::Release | AdsrStage::Done) {
            self.stage = AdsrStage::Release;
            self.remaining = self.release_frames;
        }
    }

    /// Fast ~3.5 ms choke fade for drum-kit choke groups.
    #[inline]
    pub fn choke(&mut self, sample_rate: f64) {
        if self.stage != AdsrStage::Done {
            let sr = sanitize_sample_rate(sample_rate);
            let frames = (CHOKE_MS * 0.001 * sr).round().max(1.0) as u64;
            self.stage = AdsrStage::Release;
            self.release_frames = frames;
            self.release_coeff = decay_coeff_60db_ms(CHOKE_MS, sr);
            self.remaining = frames;
        }
    }

    /// Legato progress transfer: preserves audible stage/level while adopting the
    /// new trigger's timing for the remainder of the stage and subsequent stages.
    pub fn adopt_progress_from(&mut self, previous: Self) {
        self.stage = previous.stage;
        self.level = previous.level;
        let (prev_dur, dur) = match self.stage {
            AdsrStage::Attack => (previous.attack_frames, self.attack_frames),
            AdsrStage::Decay => (previous.decay_frames, self.decay_frames),
            AdsrStage::Release => (previous.release_frames, self.release_frames),
            AdsrStage::Sustain | AdsrStage::Done => {
                self.remaining = 0;
                return;
            }
        };
        let frac = previous.remaining as f64 / prev_dur.max(1) as f64;
        self.remaining = (frac * dur as f64).round().max(1.0) as u64;
    }

    /// Advance by one sample and return the envelope level in `[0.0, 1.0]`.
    #[inline]
    pub fn next(&mut self, sustain: f32) -> f64 {
        let sustain = if self.one_shot {
            0.0
        } else {
            f64::from(if sustain.is_finite() {
                sustain.clamp(0.0, 1.0)
            } else {
                0.0
            })
        };
        match self.stage {
            AdsrStage::Attack => {
                self.level += (1.0 - self.level) / self.remaining.max(1) as f64;
                self.remaining = self.remaining.saturating_sub(1);
                if self.remaining == 0 {
                    self.level = 1.0;
                    self.stage = AdsrStage::Decay;
                    self.remaining = self.decay_frames;
                }
            }
            AdsrStage::Decay => {
                self.level = sustain + (self.level - sustain) * self.decay_coeff;
                self.remaining = self.remaining.saturating_sub(1);
                if self.remaining == 0 {
                    self.level = sustain;
                    self.stage = if self.one_shot {
                        AdsrStage::Done
                    } else {
                        AdsrStage::Sustain
                    };
                }
            }
            AdsrStage::Sustain => self.level = sustain,
            AdsrStage::Release => {
                self.level *= self.release_coeff;
                self.remaining = self.remaining.saturating_sub(1);
                if self.remaining == 0 || self.level < ENV_SILENCE {
                    self.level = 0.0;
                    self.stage = AdsrStage::Done;
                }
            }
            AdsrStage::Done => self.level = 0.0,
        }
        self.level
    }

    #[inline]
    pub fn level(&self) -> f64 {
        self.level
    }

    #[inline]
    pub fn stage(&self) -> AdsrStage {
        self.stage
    }

    #[inline]
    pub fn is_done(&self) -> bool {
        self.stage == AdsrStage::Done
    }

    #[inline]
    pub fn is_releasing(&self) -> bool {
        matches!(self.stage, AdsrStage::Release | AdsrStage::Done)
    }
}

/// Zero-allocation hold + `-60 dB` exponential decay envelope (`1.0 -> 0.0`),
/// used for transient pitch sweeps (e.g. analog kick/tom capacitor hold + discharge).
#[derive(Clone, Copy, Debug)]
pub struct HoldDecayEnv {
    level: f64,
    hold_frames: u32,
    decay_coeff: f64,
}

impl HoldDecayEnv {
    /// Create a freshly triggered envelope at `1.0`, holding for `hold_ms` ms
    /// before decaying exponentially (`-60 dB` in `decay_ms` ms).
    pub fn new(sample_rate: f64, hold_ms: f32, decay_ms: f32) -> Self {
        let sr = sanitize_sample_rate(sample_rate);
        let hold = if hold_ms.is_finite() {
            hold_ms.clamp(0.0, 5_000.0)
        } else {
            0.0
        };
        let decay = if decay_ms.is_finite() {
            decay_ms.max(1.0)
        } else {
            1.0
        };
        let hold_frames = (f64::from(hold) * 0.001 * sr)
            .round()
            .clamp(0.0, u32::MAX as f64) as u32;
        let decay_coeff = decay_coeff_60db_ms(f64::from(decay), sr);
        Self {
            level: 1.0,
            hold_frames,
            decay_coeff,
        }
    }

    /// Read the current level (`1.0` on sample 0) and advance by one sample frame.
    #[inline]
    pub fn next_sample(&mut self) -> f64 {
        let out = self.level;
        if self.hold_frames > 0 {
            self.hold_frames -= 1;
        } else {
            self.level *= self.decay_coeff;
            if self.level < ENV_SILENCE {
                self.level = 0.0;
            }
        }
        out
    }

    /// Current envelope level without advancing.
    #[inline]
    pub fn level(&self) -> f64 {
        self.level
    }
}

/// Evaluate a visual ADSR curve with true exponential decay & release (`-60 dB`)
/// and full `TriggerMode::OneShot` support.
///
/// Returns `samples` `(x, y)` points with `x ∈ [0.0, 1.0]` and `y ∈ [0.0, 1.0]`.
pub fn eval_adsr_curve(
    attack_ms: f32,
    decay_ms: f32,
    sustain: f32,
    release_ms: f32,
    one_shot: bool,
    samples: usize,
) -> Vec<(f32, f32)> {
    let n = samples.max(6);
    let seg = |ms: f32| {
        let clamped = if ms.is_finite() { ms.max(0.5) } else { 0.5 };
        (clamped + 1.0).ln() / 9.0
    };
    let a = seg(attack_ms).clamp(0.03, 0.30);
    let d = seg(decay_ms).clamp(0.05, 0.30);
    let s = if one_shot || !sustain.is_finite() {
        0.0
    } else {
        sustain.clamp(0.0, 1.0)
    };

    let mut out = Vec::with_capacity(n);
    if one_shot {
        let total = a + d;
        let t_a = a / total;
        // Allocate points cleanly so the exact peak at (t_a, 1.0) is always represented.
        let atk_pts = ((n as f32 * t_a).round() as usize).clamp(2, n - 3);
        let dec_pts = n - atk_pts;
        for i in 0..atk_pts {
            let u = i as f32 / (atk_pts - 1) as f32;
            out.push((u * t_a, u));
        }
        for j in 1..=dec_pts {
            let u = j as f32 / dec_pts as f32;
            let x = t_a + u * (1.0 - t_a);
            let y = if j == dec_pts {
                0.0
            } else {
                (-DECAY_LN_60DB * f64::from(u)).exp() as f32
            };
            out.push((x, y));
        }
        return out;
    }

    let r = seg(release_ms).clamp(0.05, 0.30);
    let hold = (1.0 - a - d - r).max(0.14);
    let total = a + d + hold + r;
    let t_a = a / total;
    let t_d = (a + d) / total;
    let t_h = (a + d + hold) / total;

    // Distribute interior samples while guaranteeing exact breakpoints (0, t_a, t_d, t_h, 1).
    let rem = n - 5;
    let d_extra = ((rem as f32) * 0.45).round() as usize;
    let r_extra = ((rem as f32) * 0.45).round() as usize;
    let a_extra = rem.saturating_sub(d_extra + r_extra);

    // Attack: 0 ..= t_a
    let a_steps = a_extra + 1;
    for i in 0..=a_steps {
        let u = i as f32 / a_steps as f32;
        out.push((u * t_a, u));
    }
    // Decay: (t_a ..= t_d) with exact exponential decay from 1.0 to sustain
    let d_steps = d_extra + 1;
    for i in 1..=d_steps {
        let u = i as f32 / d_steps as f32;
        let x = t_a + u * (t_d - t_a);
        let y = if i == d_steps {
            s
        } else {
            s + (1.0 - s) * (-DECAY_LN_60DB * f64::from(u)).exp() as f32
        };
        out.push((x, y));
    }
    // Sustain hold end point
    out.push((t_h, s));
    // Release: (t_h ..= 1.0) with exact exponential release from sustain to 0.0
    let r_steps = r_extra + 1;
    for i in 1..=r_steps {
        let u = i as f32 / r_steps as f32;
        let x = t_h + u * (1.0 - t_h);
        let y = if i == r_steps {
            0.0
        } else {
            s * (-DECAY_LN_60DB * f64::from(u)).exp() as f32
        };
        out.push((x, y));
    }
    out
}

crate::define_choice_enum! {
    /// Envelope contour mode for [`ModularEnv`] and [`crate::rack::EnvModule`].
    #[derive(Default)]
    pub enum EnvMode {
        #[default]
        Adsr => ("adsr", "ADSR", ["0"]),
        HoldDecay => ("hold_decay", "Hold-Decay", ["1", "holddecay", "hd", "hold", "decay", "perc"]),
        Capacitor => ("capacitor", "Capacitor", ["2", "cap", "rc", "c13", "303"]),
    }
}

/// Pure DSP parameter snapshot shared by [`ModularEnv`] and [`eval_modular_env_curve`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModularEnvParams {
    pub mode: EnvMode,
    pub attack_ms: f32,
    pub hold_ms: f32,
    pub decay_ms: f32,
    pub sustain: f32,
    pub release_ms: f32,
    pub cubic_limit: f32,
    pub trigger_mode: TriggerMode,
    pub delay_ms: f32,
    pub bursts: u8,
    pub burst_spacing_ms: f32,
}

impl Default for ModularEnvParams {
    fn default() -> Self {
        Self {
            mode: EnvMode::Adsr,
            attack_ms: 4.0,
            hold_ms: 0.0,
            decay_ms: 250.0,
            sustain: 0.7,
            release_ms: 180.0,
            cubic_limit: 0.0,
            trigger_mode: TriggerMode::Gate,
            delay_ms: 0.0,
            bursts: 1,
            burst_spacing_ms: 10.0,
        }
    }
}

impl ModularEnvParams {
    #[inline]
    fn sanitized(self) -> Self {
        let finite_clamp = |v: f32, lo: f32, hi: f32, fallback: f32| {
            if v.is_finite() {
                v.clamp(lo, hi)
            } else {
                fallback
            }
        };
        Self {
            mode: self.mode,
            attack_ms: finite_clamp(self.attack_ms, 0.1, 10_000.0, 4.0),
            hold_ms: finite_clamp(self.hold_ms, 0.0, 500.0, 0.0),
            decay_ms: finite_clamp(self.decay_ms, 1.0, 20_000.0, 250.0),
            sustain: finite_clamp(self.sustain, 0.0, 1.0, 0.7),
            release_ms: finite_clamp(self.release_ms, 1.0, 20_000.0, 180.0),
            cubic_limit: finite_clamp(self.cubic_limit, 0.0, 10.0, 0.0),
            trigger_mode: self.trigger_mode,
            delay_ms: finite_clamp(self.delay_ms, 0.0, 500.0, 0.0),
            bursts: self.bursts.clamp(1, 8),
            burst_spacing_ms: finite_clamp(self.burst_spacing_ms, 2.0, 50.0, 10.0),
        }
    }
    /// Resolve the charging pulse duration in milliseconds for [`EnvMode::Capacitor`].
    ///
    /// When `hold_ms > 0.0`, uses `hold_ms` directly. When `hold_ms <= 0.0`, uses
    /// `(attack_ms * 2.0).clamp(4.0, 20.0)` ms — matching the TB-303's `4 ms` trigger
    /// pulse at `attack_ms = 2.0 ms` so default `hold_ms = 0.0` charges cleanly toward `1.0`.
    #[inline]
    pub fn capacitor_pulse_ms(self) -> f32 {
        let p = self.sanitized();
        if p.hold_ms > 0.0 {
            p.hold_ms
        } else {
            (p.attack_ms * 2.0).clamp(4.0, 20.0)
        }
    }

    #[inline]
    fn capacitor_burst_spec(self, is_pre_burst: bool) -> CapacitorBurstSpec {
        let base_pulse_ms = self.capacitor_pulse_ms();
        if is_pre_burst {
            if self.trigger_mode == TriggerMode::Additive {
                CapacitorBurstSpec {
                    tau_decay_s: (f64::from(self.decay_ms) * 0.001) / DECAY_LN_60DB,
                    g_cubic: f64::from(self.cubic_limit),
                    tau_charge_s: (f64::from(self.attack_ms) * 0.001) / LN_100,
                    pulse_ms: base_pulse_ms.min(self.burst_spacing_ms * 0.5).max(0.1),
                }
            } else {
                let atk_ms = self.attack_ms.min(self.burst_spacing_ms * 0.25).max(0.1);
                let dec_ms = self.decay_ms.min(self.burst_spacing_ms * 1.15).max(1.0);
                let pulse_ms = base_pulse_ms.min(self.burst_spacing_ms * 0.35).max(0.1);
                CapacitorBurstSpec {
                    tau_decay_s: (f64::from(dec_ms) * 0.001) / DECAY_LN_60DB,
                    g_cubic: f64::from(self.cubic_limit),
                    tau_charge_s: (f64::from(atk_ms) * 0.001) / LN_100,
                    pulse_ms,
                }
            }
        } else {
            CapacitorBurstSpec {
                tau_decay_s: (f64::from(self.decay_ms) * 0.001) / DECAY_LN_60DB,
                g_cubic: f64::from(self.cubic_limit),
                tau_charge_s: (f64::from(self.attack_ms) * 0.001) / LN_100,
                pulse_ms: base_pulse_ms,
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct CapacitorBurstSpec {
    tau_decay_s: f64,
    g_cubic: f64,
    tau_charge_s: f64,
    pulse_ms: f32,
}

/// Zero-allocation modular envelope composing [`AdsrState`], [`HoldDecayEnv`], and
/// [`ZeroHzCapacitor`] with optional trigger pre-delay (`0..=500 ms`), multi-burst
/// retriggering (`1..=8` bursts spaced `2..=50 ms`), click-free retriggering,
/// legato progress adoption, continuous `sustain` / `cubic_limit` updates, and
/// 3.5 ms drum-kit chokes.
///
/// ### `EnvMode::Capacitor` Parameter & Pulse-Width Contract
/// - `attack_ms` (`0.1..=10_000 ms`) maps to `tau_charge = (attack_ms * 0.001) / ln(100)` s
///   (`99%` RC charge reached in `attack_ms`).
/// - `decay_ms` (`1.0..=20_000 ms`) maps to `tau_decay = (decay_ms * 0.001) / ln(1000)` s
///   (exact `-60 dB` decay time parity with `Adsr` and `HoldDecay`).
/// - `cubic_limit` (`0.0..=10.0`) maps to `g_cubic` (`0.0` = pure linear RC decay;
///   `> 0.0` = bare-cubic self-limiting discharge `-g * |c| * c`).
/// - **Charging pulse width:** uses `hold_ms` when `hold_ms > 0.0`; when `hold_ms <= 0.0`,
///   derives `(attack_ms * 2.0).clamp(4.0, 20.0)` ms (matching the TB-303's `4 ms` trigger
///   pulse at `attack_ms = 2.0 ms`).
/// - **`TriggerMode`:**
///   - `Gate`: charges toward target via `ZeroHzCapacitor::trigger`; `release()` cuts any
///     remaining charge pulse to `0` so the capacitor immediately enters free discharge.
///   - `OneShot`: charges via `ZeroHzCapacitor::trigger` and ignores `release()`.
///   - `Additive`: stacks charge onto existing capacitor voltage via `ZeroHzCapacitor::add`
///     (self-limiting via `cubic_limit` when `> 0.0`, modeling the TB-303 `C13` accent
///     capacitor) and ignores `release()`.
#[derive(Clone, Copy, Debug)]
pub struct ModularEnv {
    sample_rate: f64,
    params: ModularEnvParams,
    adsr: AdsrState,
    hold_decay: HoldDecayEnv,
    cap: ZeroHzCapacitor,
    cap_choking: bool,
    cap_releasing: bool,
    cap_trigger_amount: f64,
    hd_level: f64,
    hd_release_coeff: f64,
    hd_releasing: bool,
    hd_done: bool,
    delay_remaining: u64,
    bursts_left: u8,
    burst_spacing_frames: u64,
    burst_countdown: u64,
    started: bool,
    level: f64,
}

impl ModularEnv {
    #[inline]
    fn ms_to_frames(sr: f64, ms: f32) -> u64 {
        if !ms.is_finite() || ms <= 0.0 {
            return 0;
        }
        (f64::from(ms) * 0.001 * sr).round().max(0.0) as u64
    }

    #[inline]
    fn capacitor_pulse_samples(sr: f64, pulse_ms: f32) -> u32 {
        Self::ms_to_frames(sr, pulse_ms).clamp(1, u32::MAX as u64) as u32
    }

    #[inline]
    fn sanitize_amount(amount: f64) -> f64 {
        if amount.is_finite() {
            amount.max(0.0)
        } else {
            0.0
        }
    }

    #[inline]
    fn sync_capacitor_times(&mut self, tau_decay_s: f64, g_cubic: f64, tau_charge_s: f64) {
        if (self.cap.sample_rate() - self.sample_rate).abs() > 1e-9 {
            let prev_c = self.cap.c;
            let prev_remaining = self.cap.remaining;
            let prev_target = self.cap.target;
            self.cap = ZeroHzCapacitor::new(self.sample_rate, tau_decay_s, g_cubic, tau_charge_s);
            self.cap.c = prev_c;
            self.cap.remaining = prev_remaining;
            self.cap.target = prev_target;
        } else {
            self.cap.set_times(tau_decay_s, g_cubic, tau_charge_s);
        }
    }

    #[inline]
    fn is_cap_done(&self) -> bool {
        self.cap.remaining == 0 && self.cap.value().abs() < ENV_SILENCE
    }

    /// Create a freshly triggered [`ModularEnv`].
    pub fn new(sample_rate: f64, params: ModularEnvParams) -> Self {
        let sr = sanitize_sample_rate(sample_rate);
        let p = params.sanitized();
        let delay_remaining = Self::ms_to_frames(sr, p.delay_ms);
        let burst_spacing_frames = Self::ms_to_frames(sr, p.burst_spacing_ms).max(1);
        let bursts_left = p.bursts.saturating_sub(1);
        let cap_spec = p.capacitor_burst_spec(false);
        let mut env = Self {
            sample_rate: sr,
            params: p,
            adsr: AdsrState::new(sr, p.attack_ms, p.decay_ms, p.release_ms, p.trigger_mode),
            hold_decay: HoldDecayEnv::new(sr, p.hold_ms, p.decay_ms),
            cap: ZeroHzCapacitor::new(
                sr,
                cap_spec.tau_decay_s,
                cap_spec.g_cubic,
                cap_spec.tau_charge_s,
            ),
            cap_choking: false,
            cap_releasing: false,
            cap_trigger_amount: 1.0,
            hd_level: 0.0,
            hd_release_coeff: decay_coeff_60db_ms(f64::from(p.release_ms), sr),
            hd_releasing: false,
            hd_done: true,
            delay_remaining,
            bursts_left,
            burst_spacing_frames,
            burst_countdown: 0,
            started: false,
            level: 0.0,
        };
        if delay_remaining == 0 {
            env.fire_burst(false);
        }
        env
    }

    /// Create an idle (silent, `is_done() == true`) [`ModularEnv`], useful for
    /// envelopes driven strictly by an external `TrigIn` cable.
    pub fn new_idle(sample_rate: f64, params: ModularEnvParams) -> Self {
        let sr = sanitize_sample_rate(sample_rate);
        let p = params.sanitized();
        let mut adsr = AdsrState::new(sr, p.attack_ms, p.decay_ms, p.release_ms, p.trigger_mode);
        adsr.stage = AdsrStage::Done;
        adsr.level = 0.0;
        adsr.remaining = 0;
        let cap_spec = p.capacitor_burst_spec(false);
        Self {
            sample_rate: sr,
            params: p,
            adsr,
            hold_decay: HoldDecayEnv::new(sr, 0.0, 1.0),
            cap: ZeroHzCapacitor::new(
                sr,
                cap_spec.tau_decay_s,
                cap_spec.g_cubic,
                cap_spec.tau_charge_s,
            ),
            cap_choking: false,
            cap_releasing: false,
            cap_trigger_amount: 1.0,
            hd_level: 0.0,
            hd_release_coeff: decay_coeff_60db_ms(f64::from(p.release_ms), sr),
            hd_releasing: false,
            hd_done: true,
            delay_remaining: 0,
            bursts_left: 0,
            burst_spacing_frames: Self::ms_to_frames(sr, p.burst_spacing_ms).max(1),
            burst_countdown: 0,
            started: true,
            level: 0.0,
        }
    }

    fn fire_burst(&mut self, carry_level: bool) {
        self.started = true;
        let is_pre_burst = self.bursts_left > 0;
        let p = self.params;
        match p.mode {
            EnvMode::Adsr => {
                let prev = if carry_level {
                    self.level.clamp(0.0, 1.0)
                } else {
                    0.0
                };
                if is_pre_burst {
                    let atk = p.attack_ms.min(p.burst_spacing_ms * 0.25).max(0.1);
                    let dec = p.decay_ms.min(p.burst_spacing_ms * 1.15).max(1.0);
                    self.adsr = AdsrState::new_with_one_shot(
                        self.sample_rate,
                        atk,
                        dec,
                        p.release_ms,
                        true,
                    );
                    self.burst_countdown = self.burst_spacing_frames;
                } else {
                    self.adsr = AdsrState::new(
                        self.sample_rate,
                        p.attack_ms,
                        p.decay_ms,
                        p.release_ms,
                        p.trigger_mode,
                    );
                    self.burst_countdown = 0;
                }
                self.adsr.level = prev;
                self.level = prev;
            }
            EnvMode::HoldDecay => {
                if is_pre_burst {
                    let hold = p.hold_ms.min(p.burst_spacing_ms * 0.25);
                    let dec = p.decay_ms.min(p.burst_spacing_ms * 1.15).max(1.0);
                    self.hold_decay = HoldDecayEnv::new(self.sample_rate, hold, dec);
                    self.burst_countdown = self.burst_spacing_frames;
                } else {
                    self.hold_decay = HoldDecayEnv::new(self.sample_rate, p.hold_ms, p.decay_ms);
                    self.burst_countdown = 0;
                }
                self.hd_level = 1.0;
                self.hd_release_coeff =
                    decay_coeff_60db_ms(f64::from(p.release_ms), self.sample_rate);
                self.hd_releasing = false;
                self.hd_done = false;
                self.level = 1.0;
            }
            EnvMode::Capacitor => {
                let spec = p.capacitor_burst_spec(is_pre_burst);
                let pulse_samples = Self::capacitor_pulse_samples(self.sample_rate, spec.pulse_ms);
                self.sync_capacitor_times(spec.tau_decay_s, spec.g_cubic, spec.tau_charge_s);
                if !carry_level {
                    self.cap.reset();
                }
                self.cap_choking = false;
                self.cap_releasing = false;
                match p.trigger_mode {
                    TriggerMode::Additive => {
                        if self.cap_trigger_amount > 0.0 {
                            self.cap.add(self.cap_trigger_amount, pulse_samples);
                        }
                    }
                    TriggerMode::Gate | TriggerMode::OneShot => {
                        self.cap.trigger(self.cap_trigger_amount, pulse_samples);
                    }
                }
                self.burst_countdown = if is_pre_burst {
                    self.burst_spacing_frames
                } else {
                    0
                };
                self.level = self.cap.value();
            }
        }
    }

    /// Retrigger in place using the currently configured sample rate and parameters at unity (`1.0`).
    #[inline]
    pub fn trigger(&mut self) {
        self.retrigger_with_amount(self.sample_rate, self.params, 1.0);
    }

    /// Retrigger in place with an explicit charging impulse `amount` (used by `EnvMode::Capacitor`).
    #[inline]
    pub fn trigger_with_amount(&mut self, amount: f64) {
        self.retrigger_with_amount(self.sample_rate, self.params, amount);
    }

    /// Zero-click retrigger preserving the current level in `Adsr` and `Capacitor` modes,
    /// stacking charge in `EnvMode::Capacitor` + `TriggerMode::Additive`, and arming any
    /// configured pre-delay and burst sequence.
    pub fn retrigger(&mut self, sample_rate: f64, params: ModularEnvParams) {
        self.retrigger_with_amount(sample_rate, params, 1.0);
    }

    /// Zero-click retrigger with explicit charging impulse `amount` (`>= 0.0`, scaling the
    /// charging pulse target/increment in `EnvMode::Capacitor`; `Adsr` and `HoldDecay`
    /// retrigger at unity).
    pub fn retrigger_with_amount(
        &mut self,
        sample_rate: f64,
        params: ModularEnvParams,
        amount: f64,
    ) {
        let prev_mode = self.params.mode;
        let prev_level = self.level;
        let sr = sanitize_sample_rate(sample_rate);
        let p = params.sanitized();
        let amt = Self::sanitize_amount(amount);

        self.sample_rate = sr;
        self.params = p;
        self.cap_trigger_amount = amt;
        self.delay_remaining = Self::ms_to_frames(sr, p.delay_ms);
        self.burst_spacing_frames = Self::ms_to_frames(sr, p.burst_spacing_ms).max(1);
        self.bursts_left = p.bursts.saturating_sub(1);
        self.burst_countdown = 0;
        self.hd_release_coeff = decay_coeff_60db_ms(f64::from(p.release_ms), sr);
        self.hd_releasing = false;

        if p.mode == EnvMode::Capacitor {
            if prev_mode != EnvMode::Capacitor {
                self.cap.c = prev_level.max(0.0);
                self.cap.remaining = 0;
                self.cap.target = self.cap.c;
            }
            let spec = p.capacitor_burst_spec(self.bursts_left > 0);
            self.sync_capacitor_times(spec.tau_decay_s, spec.g_cubic, spec.tau_charge_s);
        }

        if self.delay_remaining == 0 {
            if p.mode != EnvMode::Capacitor {
                self.level = prev_level.clamp(0.0, 1.0);
            }
            self.fire_burst(true);
        } else {
            self.started = false;
            self.hd_done = false;
            if p.mode == EnvMode::Capacitor {
                if p.trigger_mode != TriggerMode::Additive {
                    self.cap.reset();
                    self.cap_choking = false;
                    self.cap_releasing = false;
                    self.level = 0.0;
                } else {
                    self.cap_choking = false;
                    self.cap_releasing = false;
                    self.level = self.cap.value();
                }
            } else {
                self.level = 0.0;
            }
        }
    }

    /// Update sustain (`0.0..=1.0`) continuously without resetting envelope progress
    /// or capacitor charge.
    #[inline]
    pub fn set_sustain(&mut self, sustain: f32) {
        self.params.sustain = if sustain.is_finite() {
            sustain.clamp(0.0, 1.0)
        } else {
            0.0
        };
    }

    /// Update `cubic_limit` (`0.0..=10.0`) continuously without resetting capacitor
    /// charge or active charging pulse progress.
    #[inline]
    pub fn set_cubic_limit(&mut self, cubic_limit: f32) {
        let clamped = if cubic_limit.is_finite() {
            cubic_limit.clamp(0.0, 10.0)
        } else {
            0.0
        };
        self.params.cubic_limit = clamped;
        if !self.cap_choking {
            self.cap.set_times(
                self.cap.tau_decay(),
                f64::from(clamped),
                self.cap.tau_charge(),
            );
        }
    }

    /// Current sanitized envelope parameter snapshot.
    #[inline]
    pub fn params(&self) -> ModularEnvParams {
        self.params
    }

    /// Enter release on note-off. Honored in `TriggerMode::Gate`; ignored for `OneShot`
    /// and `Additive`.
    #[inline]
    pub fn release(&mut self) {
        if matches!(
            self.params.trigger_mode,
            TriggerMode::OneShot | TriggerMode::Additive
        ) {
            return;
        }
        if !self.started {
            // Released before pre-delay expired: cancel cleanly so it never fires late.
            self.delay_remaining = 0;
            self.bursts_left = 0;
            self.burst_countdown = 0;
            self.started = true;
            self.level = 0.0;
            self.adsr.stage = AdsrStage::Done;
            self.adsr.level = 0.0;
            self.hd_level = 0.0;
            self.hd_done = true;
            self.cap.reset();
            self.cap_choking = false;
            self.cap_releasing = true;
            return;
        }
        self.bursts_left = 0;
        self.burst_countdown = 0;
        match self.params.mode {
            EnvMode::Adsr => self.adsr.release(),
            EnvMode::HoldDecay => {
                if !self.hd_done {
                    self.hd_releasing = true;
                }
            }
            EnvMode::Capacitor => {
                self.cap.remaining = 0;
                self.cap_releasing = true;
                if self.cap.c.abs() < ENV_SILENCE {
                    self.cap.reset();
                    self.level = 0.0;
                }
            }
        }
    }

    /// Fast ~3.5 ms choke fade for drum-kit choke groups. Cancels pending delay and bursts.
    #[inline]
    pub fn choke(&mut self, sample_rate: f64) {
        let sr = sanitize_sample_rate(sample_rate);
        self.delay_remaining = 0;
        self.bursts_left = 0;
        self.burst_countdown = 0;
        if !self.started {
            self.started = true;
            self.level = 0.0;
            self.adsr.stage = AdsrStage::Done;
            self.adsr.level = 0.0;
            self.hd_level = 0.0;
            self.hd_done = true;
            self.cap.reset();
            self.cap_choking = false;
            self.cap_releasing = true;
            return;
        }
        match self.params.mode {
            EnvMode::Adsr => self.adsr.choke(sr),
            EnvMode::HoldDecay => {
                if !self.hd_done {
                    self.hd_release_coeff = decay_coeff_60db_ms(CHOKE_MS, sr);
                    self.hd_releasing = true;
                }
            }
            EnvMode::Capacitor => {
                if !self.is_cap_done() {
                    let tau_choke = (CHOKE_MS * 0.001) / DECAY_LN_60DB;
                    let prev_c = self.cap.c;
                    if (self.cap.sample_rate() - sr).abs() > 1e-9 {
                        self.cap = ZeroHzCapacitor::new(sr, tau_choke, 0.0, 0.0);
                        self.cap.c = prev_c;
                    } else {
                        self.cap.set_times(tau_choke, 0.0, 0.0);
                    }
                    self.cap.remaining = 0;
                    self.cap_choking = true;
                    self.cap_releasing = true;
                    if self.cap.c.abs() < ENV_SILENCE {
                        self.cap.reset();
                        self.level = 0.0;
                    }
                }
            }
        }
    }

    /// Legato progress transfer from a previously active mono voice without resetting
    /// live envelope level or capacitor charge.
    pub fn adopt_progress_from(&mut self, previous: Self) {
        self.started = previous.started;
        self.delay_remaining = previous.delay_remaining;
        self.bursts_left = previous.bursts_left;
        self.burst_countdown = previous.burst_countdown;
        self.level = previous.level;
        match self.params.mode {
            EnvMode::Adsr => self.adsr.adopt_progress_from(previous.adsr),
            EnvMode::HoldDecay => {
                self.hold_decay = previous.hold_decay;
                self.hd_level = previous.hd_level;
                self.hd_releasing = previous.hd_releasing;
                self.hd_done = previous.hd_done;
            }
            EnvMode::Capacitor => {
                if previous.params.mode == EnvMode::Capacitor {
                    self.cap.c = previous.cap.c;
                    self.cap.target = previous.cap.target;
                    self.cap_choking = previous.cap_choking;
                    self.cap_releasing = previous.cap_releasing;
                    self.cap_trigger_amount = previous.cap_trigger_amount;
                    if previous.cap.remaining > 0 {
                        let prev_pulse = Self::capacitor_pulse_samples(
                            previous.sample_rate,
                            previous.params.capacitor_pulse_ms(),
                        );
                        let new_pulse = Self::capacitor_pulse_samples(
                            self.sample_rate,
                            self.params.capacitor_pulse_ms(),
                        );
                        let frac = f64::from(previous.cap.remaining) / f64::from(prev_pulse.max(1));
                        self.cap.remaining = (frac * f64::from(new_pulse))
                            .round()
                            .clamp(1.0, u32::MAX as f64)
                            as u32;
                    } else {
                        self.cap.remaining = 0;
                    }
                    if previous.cap_choking {
                        let tau_choke = (CHOKE_MS * 0.001) / DECAY_LN_60DB;
                        self.sync_capacitor_times(tau_choke, 0.0, 0.0);
                    }
                } else {
                    self.cap.c = previous.level.max(0.0);
                    self.cap.remaining = 0;
                    self.cap.target = self.cap.c;
                    self.cap_choking = false;
                    self.cap_releasing = previous.is_releasing();
                }
                self.level = self.cap.value();
            }
        }
    }

    /// Advance by one sample and return the envelope level (`[0.0, 1.0]` for `Adsr`,
    /// `HoldDecay`, and non-additive `Capacitor`; `>= 0.0` self-limited by `cubic_limit`
    /// for `EnvMode::Capacitor` with `TriggerMode::Additive`).
    #[inline]
    pub fn next(&mut self, sustain: f32) -> f64 {
        if !self.started {
            if self.delay_remaining > 0 {
                self.delay_remaining -= 1;
                if self.params.mode == EnvMode::Capacitor
                    && self.params.trigger_mode == TriggerMode::Additive
                    && !self.is_cap_done()
                {
                    let mut v = self.cap.step();
                    if self.cap.remaining == 0 && v.abs() < ENV_SILENCE {
                        self.cap.reset();
                        v = 0.0;
                    }
                    self.level = v;
                    return v;
                }
                self.level = 0.0;
                return 0.0;
            }
            let carry = self.params.mode == EnvMode::Capacitor
                && self.params.trigger_mode == TriggerMode::Additive
                && !self.is_cap_done();
            self.fire_burst(carry);
        }

        let is_pre_burst = self.bursts_left > 0;
        let val = match self.params.mode {
            EnvMode::Adsr => {
                let s = if is_pre_burst { 0.0 } else { sustain };
                self.adsr.next(s)
            }
            EnvMode::HoldDecay => {
                if self.hd_done {
                    0.0
                } else if self.hd_releasing {
                    let out = self.hd_level;
                    self.hd_level *= self.hd_release_coeff;
                    if self.hd_level < ENV_SILENCE {
                        self.hd_level = 0.0;
                        self.hd_done = true;
                    }
                    out
                } else {
                    let out = self.hold_decay.next_sample();
                    self.hd_level = self.hold_decay.level();
                    if self.hd_level < ENV_SILENCE {
                        self.hd_level = 0.0;
                        self.hd_done = true;
                    }
                    out
                }
            }
            EnvMode::Capacitor => {
                if self.is_cap_done() {
                    0.0
                } else {
                    let mut v = self.cap.step();
                    if self.cap.remaining == 0 && v.abs() < ENV_SILENCE {
                        self.cap.reset();
                        v = 0.0;
                    }
                    v
                }
            }
        };
        self.level = val;

        if self.bursts_left > 0 {
            self.burst_countdown = self.burst_countdown.saturating_sub(1);
            if self.burst_countdown == 0 {
                self.bursts_left -= 1;
                self.fire_burst(true);
            }
        }
        val
    }

    #[inline]
    pub fn level(&self) -> f64 {
        self.level
    }

    #[inline]
    pub fn stage(&self) -> AdsrStage {
        if !self.started {
            return AdsrStage::Attack;
        }
        match self.params.mode {
            EnvMode::Adsr => self.adsr.stage(),
            EnvMode::HoldDecay => {
                if self.hd_done {
                    AdsrStage::Done
                } else if self.hd_releasing {
                    AdsrStage::Release
                } else {
                    AdsrStage::Decay
                }
            }
            EnvMode::Capacitor => {
                if self.is_cap_done() && self.bursts_left == 0 {
                    AdsrStage::Done
                } else if self.cap_choking || self.cap_releasing {
                    AdsrStage::Release
                } else if self.cap.remaining > 0 {
                    AdsrStage::Attack
                } else {
                    AdsrStage::Decay
                }
            }
        }
    }

    #[inline]
    pub fn is_done(&self) -> bool {
        if !self.started || self.bursts_left > 0 {
            return false;
        }
        match self.params.mode {
            EnvMode::Adsr => self.adsr.is_done(),
            EnvMode::HoldDecay => self.hd_done,
            EnvMode::Capacitor => self.is_cap_done(),
        }
    }

    #[inline]
    pub fn is_releasing(&self) -> bool {
        if !self.started || self.bursts_left > 0 {
            return false;
        }
        match self.params.mode {
            EnvMode::Adsr => self.adsr.is_releasing(),
            EnvMode::HoldDecay => self.hd_releasing || self.hd_done,
            EnvMode::Capacitor => self.cap_releasing || self.cap_choking || self.is_cap_done(),
        }
    }
}

#[inline]
fn capacitor_advance_charge(c: f64, target: f64, tau_charge_s: f64, dt_sec: f64) -> f64 {
    if !dt_sec.is_finite() || dt_sec <= 0.0 {
        return c;
    }
    let mut cap = ZeroHzCapacitor::new(1.0 / dt_sec, 1.0, 0.0, tau_charge_s);
    cap.c = c;
    cap.trigger(target, 1);
    cap.step()
}

#[inline]
fn capacitor_advance_discharge(c: f64, tau_decay_s: f64, g_cubic: f64, dt_sec: f64) -> f64 {
    if !dt_sec.is_finite() || dt_sec <= 0.0 {
        return c;
    }
    let mut cap = ZeroHzCapacitor::new(1.0 / dt_sec, tau_decay_s, g_cubic, 0.0);
    cap.c = c;
    cap.step()
}

#[derive(Clone, Copy, Default)]
struct CapPreviewBurst {
    c_start: f64,
    target: f64,
    c_peak: f64,
    spec: CapacitorBurstSpec,
}

fn eval_capacitor_env_curve(p: ModularEnvParams, n: usize) -> Vec<(f32, f32)> {
    let seg = |ms: f32| (ms.max(0.5) + 1.0).ln() / 9.0;
    let w_delay = if p.delay_ms > 0.01 {
        seg(p.delay_ms).clamp(0.05, 0.22)
    } else {
        0.0
    };
    let pre_bursts = usize::from(p.bursts.saturating_sub(1));
    let burst_unit = if pre_bursts > 0 {
        (0.38 / pre_bursts as f32).clamp(0.035, 0.14)
    } else {
        0.0
    };
    let w_bursts = burst_unit * pre_bursts as f32;
    let w_tail = (1.0 - w_delay - w_bursts).max(0.32);
    let total = w_delay + w_bursts + w_tail;

    let x_delay_end = w_delay / total;
    let burst_w = burst_unit / total;
    let tail_start = (w_delay + w_bursts) / total;
    let tail_w = w_tail / total;

    // Simulate exact ZeroHzCapacitor state across pre-bursts and the final burst.
    let mut burst_states = [CapPreviewBurst::default(); 8];
    let mut c = 0.0_f64;
    let mut max_c = 0.0_f64;
    for slot in burst_states.iter_mut().take(pre_bursts) {
        let spec = p.capacitor_burst_spec(true);
        let c_start = c;
        let target = if p.trigger_mode == TriggerMode::Additive {
            c_start + 1.0
        } else {
            1.0
        };
        let charge_s = f64::from(spec.pulse_ms) * 0.001;
        let c_peak = capacitor_advance_charge(c_start, target, spec.tau_charge_s, charge_s);
        let decay_s = (f64::from(p.burst_spacing_ms) - f64::from(spec.pulse_ms)).max(0.0) * 0.001;
        c = capacitor_advance_discharge(c_peak, spec.tau_decay_s, spec.g_cubic, decay_s);
        max_c = max_c.max(c_peak);
        *slot = CapPreviewBurst {
            c_start,
            target,
            c_peak,
            spec,
        };
    }

    let final_spec = p.capacitor_burst_spec(false);
    let final_target = if p.trigger_mode == TriggerMode::Additive {
        c + 1.0
    } else {
        1.0
    };
    let final_charge_s = f64::from(final_spec.pulse_ms) * 0.001;
    let final_peak =
        capacitor_advance_charge(c, final_target, final_spec.tau_charge_s, final_charge_s);
    max_c = max_c.max(final_peak);
    let final_burst = CapPreviewBurst {
        c_start: c,
        target: final_target,
        c_peak: final_peak,
        spec: final_spec,
    };

    let tail_decay_s = (final_spec.tau_decay_s * DECAY_LN_60DB).max(0.001);
    let w_chg = seg(final_spec.pulse_ms).clamp(0.06, 0.35);
    let w_dec = seg(p.decay_ms).clamp(0.20, 0.85);
    let tail_charge_frac = w_chg / (w_chg + w_dec);
    let x_final_peak = tail_start + tail_w * tail_charge_frac;
    let norm = max_c.max(1.0);

    let eval_raw = |x: f32| -> f64 {
        if x < x_delay_end {
            return 0.0;
        }
        if x < tail_start && pre_bursts > 0 {
            let local = (x - x_delay_end) / burst_w.max(1e-6);
            let idx = (local.floor() as usize).min(pre_bursts - 1);
            let frac = (local - idx as f32).clamp(0.0, 1.0);
            let bs = burst_states[idx];
            let pulse_frac = (bs.spec.pulse_ms / p.burst_spacing_ms.max(2.0)).clamp(0.05, 0.5);
            if frac <= pulse_frac {
                let dt_s =
                    f64::from(frac / pulse_frac.max(1e-6)) * f64::from(bs.spec.pulse_ms) * 0.001;
                return capacitor_advance_charge(bs.c_start, bs.target, bs.spec.tau_charge_s, dt_s);
            }
            let du = f64::from((frac - pulse_frac) / (1.0 - pulse_frac).max(1e-6));
            let decay_s =
                (f64::from(p.burst_spacing_ms) - f64::from(bs.spec.pulse_ms)).max(0.0) * 0.001;
            return capacitor_advance_discharge(
                bs.c_peak,
                bs.spec.tau_decay_s,
                bs.spec.g_cubic,
                du * decay_s,
            );
        }
        let u = ((x - tail_start) / tail_w.max(1e-6)).clamp(0.0, 1.0);
        if u <= tail_charge_frac {
            let cu = f64::from(u / tail_charge_frac.max(1e-6));
            let dt_s = cu * final_charge_s;
            capacitor_advance_charge(
                final_burst.c_start,
                final_burst.target,
                final_burst.spec.tau_charge_s,
                dt_s,
            )
        } else {
            let du = f64::from((u - tail_charge_frac) / (1.0 - tail_charge_frac).max(1e-6));
            capacitor_advance_discharge(
                final_burst.c_peak,
                final_burst.spec.tau_decay_s,
                final_burst.spec.g_cubic,
                du * tail_decay_s,
            )
        }
    };

    // Collect exact landmark coordinates (pre-delay edge, burst peaks/valleys, final charge peak,
    // and endpoint), then fill remaining points uniformly across [0.0, 1.0].
    let mut xs = Vec::with_capacity(n + pre_bursts * 2 + 5);
    xs.push(0.0_f32);
    if w_delay > 0.0 {
        xs.push((x_delay_end - 1e-4).max(0.0));
        xs.push(x_delay_end);
    }
    for (b, bs) in burst_states.iter().enumerate().take(pre_bursts) {
        let bx = x_delay_end + b as f32 * burst_w;
        let pulse_frac = (bs.spec.pulse_ms / p.burst_spacing_ms.max(2.0)).clamp(0.05, 0.5);
        xs.push((bx + burst_w * pulse_frac).min(1.0));
        xs.push((bx + burst_w - 1e-4).clamp(0.0, 1.0));
    }
    if tail_start > 0.0 {
        xs.push(tail_start);
    }
    xs.push(x_final_peak.clamp(0.0, 1.0));
    xs.push(1.0_f32);

    let extra = n.saturating_sub(xs.len());
    for i in 1..=extra {
        xs.push(i as f32 / (extra + 1) as f32);
    }
    xs.sort_by(f32::total_cmp);
    if xs.len() > n {
        let mut trimmed = Vec::with_capacity(n);
        for i in 0..n {
            let idx = i * (xs.len() - 1) / (n - 1);
            trimmed.push(xs[idx]);
        }
        xs = trimmed;
    }

    xs.into_iter()
        .enumerate()
        .map(|(i, x)| {
            let y = if i + 1 == n {
                0.0
            } else {
                ((eval_raw(x) / norm) as f32).clamp(0.0, 1.0)
            };
            (x.clamp(0.0, 1.0), y)
        })
        .collect()
}

/// Evaluate a visual [`ModularEnv`] contour across `samples` `(x, y)` points
/// (`x ∈ [0.0, 1.0]`, `y ∈ [0.0, 1.0]`), sharing exact exponential `-60 dB` decay,
/// [`ZeroHzCapacitor`] charging/discharging, `cubic_limit` self-limiting,
/// `TriggerMode::Additive` stacking, pre-delay, multi-burst spikes, and
/// `EnvMode::{Adsr, HoldDecay, Capacitor}` semantics.
pub fn eval_modular_env_curve(params: ModularEnvParams, samples: usize) -> Vec<(f32, f32)> {
    let p = params.sanitized();
    let n = samples.max(8);

    if p.mode == EnvMode::Capacitor {
        return eval_capacitor_env_curve(p, n);
    }

    let one_shot = matches!(p.trigger_mode, TriggerMode::OneShot | TriggerMode::Additive);

    if p.mode == EnvMode::Adsr && p.delay_ms <= 0.01 && p.bursts <= 1 {
        return eval_adsr_curve(
            p.attack_ms,
            p.decay_ms,
            p.sustain,
            p.release_ms,
            one_shot,
            n,
        );
    }

    // Build exact piecewise breakpoints on an internal [0.0, 1.0] timeline, then
    // sample cleanly at exact peaks and uniform interior coordinates so every
    // pre-delay, burst peak, hold plateau, and exponential tail stays sharp.
    let seg = |ms: f32| (ms.max(0.5) + 1.0).ln() / 9.0;
    let w_delay = if p.delay_ms > 0.01 {
        seg(p.delay_ms).clamp(0.05, 0.22)
    } else {
        0.0
    };
    let pre_bursts = usize::from(p.bursts.saturating_sub(1));
    let burst_unit = if pre_bursts > 0 {
        (0.38 / pre_bursts as f32).clamp(0.035, 0.14)
    } else {
        0.0
    };
    let w_bursts = burst_unit * pre_bursts as f32;
    let w_tail = (1.0 - w_delay - w_bursts).max(0.32);
    let total = w_delay + w_bursts + w_tail;

    let x_delay_end = w_delay / total;
    let burst_w = burst_unit / total;
    let tail_start = (w_delay + w_bursts) / total;
    let tail_w = w_tail / total;

    // Helper evaluating the tail shape for u in [0.0, 1.0].
    let eval_tail = |u: f32| -> f32 {
        let u = u.clamp(0.0, 1.0);
        match p.mode {
            EnvMode::HoldDecay => {
                let w_h = if p.hold_ms > 0.01 {
                    seg(p.hold_ms).clamp(0.04, 0.35)
                } else {
                    0.0
                };
                let w_d = seg(p.decay_ms).clamp(0.15, 0.85);
                let h_end = w_h / (w_h + w_d);
                if u <= h_end {
                    1.0
                } else if u >= 1.0 - 1e-6 {
                    0.0
                } else {
                    let du = (u - h_end) / (1.0 - h_end).max(1e-6);
                    (-DECAY_LN_60DB * f64::from(du)).exp() as f32
                }
            }
            EnvMode::Adsr => {
                let a = seg(p.attack_ms).clamp(0.03, 0.30);
                let d = seg(p.decay_ms).clamp(0.05, 0.30);
                let s = if one_shot { 0.0 } else { p.sustain };
                if one_shot {
                    let t_a = a / (a + d);
                    if u <= t_a {
                        u / t_a.max(1e-6)
                    } else if u >= 1.0 - 1e-6 {
                        0.0
                    } else {
                        let du = (u - t_a) / (1.0 - t_a).max(1e-6);
                        (-DECAY_LN_60DB * f64::from(du)).exp() as f32
                    }
                } else {
                    let r = seg(p.release_ms).clamp(0.05, 0.30);
                    let hold = (1.0 - a - d - r).max(0.14);
                    let sum = a + d + hold + r;
                    let t_a = a / sum;
                    let t_d = (a + d) / sum;
                    let t_h = (a + d + hold) / sum;
                    if u <= t_a {
                        u / t_a.max(1e-6)
                    } else if u <= t_d {
                        let du = (u - t_a) / (t_d - t_a).max(1e-6);
                        s + (1.0 - s) * (-DECAY_LN_60DB * f64::from(du)).exp() as f32
                    } else if u <= t_h {
                        s
                    } else if u >= 1.0 - 1e-6 {
                        0.0
                    } else {
                        let du = (u - t_h) / (1.0 - t_h).max(1e-6);
                        s * (-DECAY_LN_60DB * f64::from(du)).exp() as f32
                    }
                }
            }
            EnvMode::Capacitor => {
                unreachable!("Capacitor mode is handled by eval_capacitor_env_curve")
            }
        }
    };

    let eval_at_x = |x: f32| -> f32 {
        if x < x_delay_end {
            return 0.0;
        }
        if x < tail_start && pre_bursts > 0 {
            let local = (x - x_delay_end) / burst_w.max(1e-6);
            let frac = (local - local.floor()).clamp(0.0, 1.0);
            if p.mode == EnvMode::HoldDecay {
                let h = (p.hold_ms.min(p.burst_spacing_ms * 0.25) / p.burst_spacing_ms.max(2.0))
                    .clamp(0.0, 0.25);
                if frac <= h {
                    return 1.0;
                }
                let du = (frac - h) / (1.0 - h).max(1e-6);
                return (-DECAY_LN_60DB * 0.85 * f64::from(du)).exp() as f32;
            } else {
                let atk_f = 0.12_f32;
                if frac <= atk_f {
                    return frac / atk_f;
                }
                let du = (frac - atk_f) / (1.0 - atk_f);
                return (-DECAY_LN_60DB * 0.85 * f64::from(du)).exp() as f32;
            }
        }
        let u = ((x - tail_start) / tail_w.max(1e-6)).clamp(0.0, 1.0);
        eval_tail(u)
    };

    // Collect mandatory exact feature x-coordinates (pre-delay corner, every burst peak/valley,
    // tail start, tail peak, and endpoint), then fill remaining budget uniformly.
    let mut xs = Vec::with_capacity(n + pre_bursts * 2 + 4);
    xs.push(0.0_f32);
    if w_delay > 0.0 {
        xs.push((x_delay_end - 1e-4).max(0.0));
        xs.push(x_delay_end);
    }
    for b in 0..pre_bursts {
        let bx = x_delay_end + b as f32 * burst_w;
        let peak_x = if p.mode == EnvMode::HoldDecay {
            bx + 1e-4
        } else {
            bx + burst_w * 0.12
        };
        xs.push(peak_x.min(1.0));
        xs.push((bx + burst_w - 1e-4).clamp(0.0, 1.0));
    }
    xs.push(tail_start);
    xs.push(1.0_f32);

    let extra = n.saturating_sub(xs.len());
    for i in 1..=extra {
        xs.push(i as f32 / (extra + 1) as f32);
    }
    xs.sort_by(f32::total_cmp);
    // Keep exact length `n` if `xs` exceeded `n` (only possible when `n` is very small).
    if xs.len() > n {
        let mut trimmed = Vec::with_capacity(n);
        for i in 0..n {
            let idx = i * (xs.len() - 1) / (n - 1);
            trimmed.push(xs[idx]);
        }
        xs = trimmed;
    }

    xs.into_iter()
        .enumerate()
        .map(|(i, x)| {
            let y = if i + 1 == n {
                0.0
            } else if i == 0 && w_delay == 0.0 && p.mode == EnvMode::HoldDecay {
                1.0
            } else {
                eval_at_x(x).clamp(0.0, 1.0)
            };
            (x.clamp(0.0, 1.0), y)
        })
        .collect()
}

/// Sample-accurate rhythmic step envelope shared by `RhythmicFilter` and UI previews.
///
/// Uses an exact coupled 2-stage 1-pole ODE (`tau_decay = env_decay_ms`, `tau_atk = 1.5 ms`
/// snappy or `18.0 ms` smooth-attack) for BOTH real-time audio and analytical preview curves.
#[derive(Clone, Copy, Debug)]
pub struct RhythmicEnv {
    sample_rate: f64,
    pattern: CompiledFilterPattern,
    decay_coef: f64,
    cached_decay_tau_bits: u64,
    fast_attack: f64,
    smooth_attack: f64,
    env_raw: f64,
    env_smooth: f64,
    song_pos: f64,
    steps_per_sample: f64,
    last_pat_step: u64,
    last_ctrl_pos: f64,
    transport_running: bool,
    clock_synced: bool,
}

impl RhythmicEnv {
    /// Crisp percussive attack time constant (1.5 ms).
    pub const FAST_ATTACK_MS: f32 = 1.5;
    /// Smooth wah/sweep attack time constant (18.0 ms — exact match between DSP & UI!).
    pub const SMOOTH_ATTACK_MS: f32 = 18.0;
    /// Drift threshold (in 16th-note steps) before snapping song position on seek/loop.
    const SEEK_THRESHOLD_STEPS: f64 = 0.02;
    const MIN_DECAY_MS: f64 = 15.0;
    const MAX_DECAY_MS: f64 = 1500.0;
    const DEFAULT_DECAY_MS: f64 = 240.0;

    #[inline]
    fn sanitize_decay_sec(env_decay_ms: f32) -> f64 {
        sanitize_clamp_f64(
            f64::from(env_decay_ms),
            Self::MIN_DECAY_MS,
            Self::MAX_DECAY_MS,
            Self::DEFAULT_DECAY_MS,
        ) * 0.001
    }

    pub fn new(sample_rate: f64) -> Self {
        let sr = sanitize_sample_rate(sample_rate);
        let default_tau = Self::DEFAULT_DECAY_MS * 0.001;
        Self {
            sample_rate: sr,
            pattern: CompiledFilterPattern::default(),
            decay_coef: (-1.0 / (default_tau * sr)).exp(),
            cached_decay_tau_bits: default_tau.to_bits(),
            fast_attack: one_pole_coeff_ms(Self::FAST_ATTACK_MS, sr),
            smooth_attack: one_pole_coeff_ms(Self::SMOOTH_ATTACK_MS, sr),
            env_raw: 0.0,
            env_smooth: 0.0,
            song_pos: 0.0,
            steps_per_sample: 120.0 / (15.0 * sr),
            last_pat_step: u64::MAX,
            last_ctrl_pos: f64::NAN,
            transport_running: false,
            clock_synced: false,
        }
    }

    pub fn reset(&mut self) {
        self.env_raw = 0.0;
        self.env_smooth = 0.0;
        self.last_pat_step = u64::MAX;
        self.last_ctrl_pos = f64::NAN;
        self.transport_running = false;
        self.clock_synced = false;
    }

    /// Update pattern and decay time without disturbing clock synchronization.
    pub fn set_pattern(&mut self, compiled: CompiledFilterPattern, env_decay_ms: f32) {
        self.pattern = compiled;
        let tau = Self::sanitize_decay_sec(env_decay_ms);
        let tau_bits = tau.to_bits();
        if tau_bits != self.cached_decay_tau_bits {
            self.cached_decay_tau_bits = tau_bits;
            self.decay_coef = (-1.0 / (tau * self.sample_rate)).exp();
        }
    }

    /// Synchronize the rhythmic envelope to the transport clock (called once per control block).
    ///
    /// - Tight `0.02`-step drift tolerance prevents phase lag and jumps.
    /// - Immediately halts advancement on transport stop (zero 2-block overrun!).
    /// - Avoids double-triggering on stop, resume, or minor PLL drift correction.
    pub fn sync_song_pos(
        &mut self,
        compiled: CompiledFilterPattern,
        env_decay_ms: f32,
        song_pos: f64,
        bpm: f64,
    ) {
        self.set_pattern(compiled, env_decay_ms);
        let safe_bpm = if bpm.is_finite() {
            bpm.max(20.0)
        } else {
            120.0
        };
        self.steps_per_sample = safe_bpm / (15.0 * self.sample_rate);
        let pos = sanitize_sample_f64(song_pos).max(0.0);

        if !self.clock_synced {
            self.song_pos = pos;
            self.last_ctrl_pos = pos;
            self.last_pat_step = u64::MAX;
            self.transport_running = true;
            self.clock_synced = true;
            return;
        }

        if (pos - self.last_ctrl_pos).abs() < 1e-9 {
            // Transport stopped/paused: stop immediately without advancing or re-triggering.
            self.transport_running = false;
            self.song_pos = pos;
            return;
        }

        let was_stopped = !self.transport_running;
        self.transport_running = true;
        let delta = pos - self.song_pos;
        if delta.abs() > Self::SEEK_THRESHOLD_STEPS {
            // Genuine seek, loop wrap, or resume at a new position.
            let backward_or_big_jump = delta < -Self::SEEK_THRESHOLD_STEPS || delta > 0.5;
            self.song_pos = pos;
            if backward_or_big_jump && !was_stopped {
                let (new_pat_step, _, _) = self.pattern.step_at(pos);
                if new_pat_step != self.last_pat_step {
                    self.last_pat_step = u64::MAX;
                } else {
                    // Loop wrap landed on the same step index (e.g. seek back to bar 0): re-arm.
                    self.last_pat_step = u64::MAX;
                }
            } else if was_stopped {
                let (new_pat_step, _, _) = self.pattern.step_at(pos);
                if new_pat_step != self.last_pat_step {
                    self.last_pat_step = u64::MAX;
                }
            }
        } else {
            // Gentle PLL pull eliminates tiny floating-point clock drift without skipping steps.
            self.song_pos += delta * 0.25;
        }
        self.last_ctrl_pos = pos;
    }

    /// Advance one audio sample and return the smoothed envelope level in `[0.0, 1.0]`.
    #[inline]
    pub fn next_sample(&mut self) -> f64 {
        if self.transport_running {
            let (pat_step, _, vel) = self.pattern.step_at(self.song_pos);
            if pat_step != self.last_pat_step {
                self.last_pat_step = pat_step;
                if vel > 0 {
                    self.env_raw = f64::from(vel) / 127.0;
                }
            }
            self.song_pos += self.steps_per_sample;
        }
        self.env_raw *= self.decay_coef;
        if self.env_raw < 1e-12 {
            self.env_raw = 0.0;
        }
        let atk = if self.pattern.smooth_attack {
            self.smooth_attack
        } else {
            self.fast_attack
        };
        self.env_smooth += (self.env_raw - self.env_smooth) * atk;
        if self.env_smooth < 1e-12 {
            self.env_smooth = 0.0;
        }
        self.env_smooth
    }

    /// Current smoothed envelope value without advancing state.
    #[inline]
    pub fn current(&self) -> f64 {
        self.env_smooth
    }

    /// Advance the rhythmic envelope across `frames` silent or idle-bypassed samples
    /// with bit-exact parity to [`RhythmicEnv::next_sample`], returning the final
    /// smoothed envelope level.
    #[inline]
    pub fn advance_silent(&mut self, frames: usize) -> f64 {
        if frames == 0 {
            return self.env_smooth;
        }
        if !self.transport_running && self.env_raw == 0.0 && self.env_smooth == 0.0 {
            return 0.0;
        }
        for _ in 0..frames {
            self.next_sample();
            if !self.transport_running && self.env_raw == 0.0 && self.env_smooth == 0.0 {
                break;
            }
        }
        self.env_smooth
    }

    /// Exact closed-form advance of the coupled 2-stage 1-pole ODE over `dt_sec`:
    /// `r'(t) = -r / tau_decay`, `s'(t) = (r - s) / tau_atk`.
    #[inline]
    fn advance_ode(raw: f64, smooth: f64, dt_sec: f64, tau_decay: f64, tau_atk: f64) -> (f64, f64) {
        if dt_sec <= 0.0 {
            return (raw, smooth);
        }
        let a = 1.0 / tau_decay;
        let b = 1.0 / tau_atk;
        let ea = (-a * dt_sec).exp();
        let eb = (-b * dt_sec).exp();
        let next_raw = raw * ea;
        let next_smooth = if (b - a).abs() > 1e-6 {
            smooth * eb + raw * (b / (b - a)) * (ea - eb)
        } else {
            (smooth + raw * b * dt_sec) * eb
        };
        (
            if next_raw < 1e-12 { 0.0 } else { next_raw },
            if next_smooth < 1e-12 {
                0.0
            } else {
                next_smooth
            },
        )
    }

    /// Deterministic analytical evaluation of `(active_slot, env_level_0_to_1)` at
    /// `song_step_16th`, matching [`RhythmicEnv::next_sample`] using the exact same
    /// `18 ms` smooth-attack / `1.5 ms` fast-attack and `env_decay_ms` ODE.
    pub fn eval_at(
        compiled: &CompiledFilterPattern,
        song_step_16th: f64,
        bpm: f64,
        env_decay_ms: f32,
    ) -> (usize, f32) {
        let len =
            usize::from(compiled.length).clamp(1, crate::filter_patterns::MAX_FILTER_ENV_STEPS);
        let pat_pos_f =
            sanitize_sample_f64(song_step_16th).max(0.0) * compiled.steps_per_sixteenth();
        let abs_step = pat_pos_f.floor() as u64;
        let frac = (pat_pos_f - pat_pos_f.floor()).clamp(0.0, 1.0);
        let cur_slot = (abs_step % len as u64) as usize;

        let safe_bpm = if bpm.is_finite() {
            bpm.max(20.0)
        } else {
            120.0
        };
        let step_sec = 60.0 / (safe_bpm * 4.0 * compiled.steps_per_sixteenth());
        let tau_decay = Self::sanitize_decay_sec(env_decay_ms).max(0.005);
        let tau_atk = f64::from(if compiled.smooth_attack {
            Self::SMOOTH_ATTACK_MS
        } else {
            Self::FAST_ATTACK_MS
        }) * 0.001;

        let lookback = abs_step.min(64);
        let start = abs_step - lookback;
        let mut raw = 0.0_f64;
        let mut smooth = 0.0_f64;
        for s in start..=abs_step {
            let slot = (s % len as u64) as usize;
            let v = compiled.steps[slot];
            if v > 0 {
                raw = f64::from(v) / 127.0;
            }
            let dt = if s == abs_step {
                frac * step_sec
            } else {
                step_sec
            };
            (raw, smooth) = Self::advance_ode(raw, smooth, dt, tau_decay, tau_atk);
        }
        (cur_slot, smooth.clamp(0.0, 1.0) as f32)
    }

    /// Generate a steady-state preview contour across one full cycle of `compiled`
    /// (`len * points_per_step` samples in `[0.0, 1.0]`) sharing the exact same ODE math.
    pub fn preview(
        compiled: &CompiledFilterPattern,
        env_decay_ms: f32,
        bpm: f64,
        points_per_step: usize,
    ) -> Vec<f32> {
        let len =
            usize::from(compiled.length).clamp(1, crate::filter_patterns::MAX_FILTER_ENV_STEPS);
        let pps = points_per_step.max(1);
        let safe_bpm = if bpm.is_finite() {
            bpm.max(20.0)
        } else {
            120.0
        };
        let step_sec = 60.0 / (safe_bpm * 4.0 * compiled.steps_per_sixteenth());
        let sub_dt = step_sec / pps as f64;
        let tau_decay = Self::sanitize_decay_sec(env_decay_ms).max(0.005);
        let tau_atk = f64::from(if compiled.smooth_attack {
            Self::SMOOTH_ATTACK_MS
        } else {
            Self::FAST_ATTACK_MS
        }) * 0.001;

        // Warm up 1–2 cycles so loop wraparound matches steady-state playback.
        let warmup_steps = len.max(16);
        let mut raw = 0.0_f64;
        let mut smooth = 0.0_f64;
        for s in 0..warmup_steps {
            let slot = s % len;
            let v = compiled.steps[slot];
            if v > 0 {
                raw = f64::from(v) / 127.0;
            }
            (raw, smooth) = Self::advance_ode(raw, smooth, step_sec, tau_decay, tau_atk);
        }

        let mut out = Vec::with_capacity(len * pps);
        for &v in compiled.steps.iter().take(len) {
            if v > 0 {
                raw = f64::from(v) / 127.0;
            }
            for _ in 0..pps {
                (raw, smooth) = Self::advance_ode(raw, smooth, sub_dt, tau_decay, tau_atk);
                out.push(smooth.clamp(0.0, 1.0) as f32);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adsr_retrigger_is_click_free_and_one_shot_terminates() {
        let sr = 48_000.0;
        let mut env = AdsrState::new(sr, 5.0, 40.0, 80.0, TriggerMode::Gate);
        let mut last = 0.0;
        for _ in 0..500 {
            last = env.next(0.6);
        }
        assert!(last > 0.5);
        // Zero-click retrigger preserves current level instead of dropping to 0.
        env.retrigger(sr, 10.0, 40.0, 80.0, TriggerMode::Gate);
        let after = env.next(0.6);
        assert!(
            (after - last).abs() < 0.05 && after >= last,
            "retrigger dropped level: before={last}, after={after}"
        );

        let mut os = AdsrState::new(sr, 2.0, 20.0, 500.0, TriggerMode::OneShot);
        for _ in 0..4000 {
            os.next(0.8);
        }
        assert!(os.is_done());
        assert_eq!(os.level(), 0.0);
    }

    #[test]
    fn eval_adsr_curve_is_exponential_and_honors_one_shot() {
        let gate = eval_adsr_curve(10.0, 200.0, 0.4, 300.0, false, 64);
        assert_eq!(gate.len(), 64);
        assert_eq!(gate.first().unwrap().0, 0.0);
        assert_eq!(gate.first().unwrap().1, 0.0);
        assert_eq!(gate.last().unwrap().0, 1.0);
        assert_eq!(gate.last().unwrap().1, 0.0);
        // Monotonic x and reaches exact peak 1.0.
        for w in gate.windows(2) {
            assert!(w[1].0 >= w[0].0);
        }
        assert!(gate.iter().any(|&(_, y)| (y - 1.0).abs() < 1e-6));

        // Exponential decay curves below the straight chord connecting peak (1.0) to sustain (0.4).
        let peak_idx = gate
            .iter()
            .position(|&(_, y)| (y - 1.0).abs() < 1e-6)
            .unwrap();
        let mid_decay = gate[peak_idx + 4];
        assert!(mid_decay.1 < 0.9 && mid_decay.1 > 0.39);

        // OneShot decays directly to 0 without sustain plateau.
        let os = eval_adsr_curve(10.0, 200.0, 0.8, 300.0, true, 64);
        assert_eq!(os.len(), 64);
        assert_eq!(os.last().unwrap().1, 0.0);
        // After midpoint of decay, level should be well below ignored sustain=0.8.
        let late = os[os.len() * 3 / 4].1;
        assert!(late < 0.3, "OneShot should ignore sustain=0.8, got {late}");
    }

    #[test]
    fn rhythmic_env_realtime_matches_analytical_preview_and_stops_cleanly() {
        let sr = 48_000.0;
        let bpm = 120.0;
        let decay_ms = 180.0_f32;
        let mut pulse_steps = [0u8; crate::filter_patterns::MAX_FILTER_ENV_STEPS];
        pulse_steps[0] = 127;
        let mut swell_steps = [0u8; crate::filter_patterns::MAX_FILTER_ENV_STEPS];
        swell_steps[..16].copy_from_slice(&[
            4, 15, 29, 44, 60, 72, 86, 101, 115, 101, 86, 72, 58, 44, 29, 15,
        ]);
        let test_patterns = [
            (
                "snappy_pulse",
                CompiledFilterPattern {
                    resolution: 16,
                    length: 16,
                    smooth_attack: false,
                    steps: pulse_steps,
                },
            ),
            (
                "smooth_swell",
                CompiledFilterPattern {
                    resolution: 16,
                    length: 16,
                    smooth_attack: true,
                    steps: swell_steps,
                },
            ),
        ];
        for (label, pat) in test_patterns {
            let mut env = RhythmicEnv::new(sr);
            let step_samples = (sr * 15.0 / bpm) as usize;
            let total_steps = 8;
            let mut max_err = 0.0_f64;
            for i in 0..(step_samples * total_steps) {
                let song_pos = i as f64 / step_samples as f64;
                if i % 64 == 0 {
                    env.sync_song_pos(pat, decay_ms, song_pos, bpm);
                }
                let rt = env.next_sample();
                if i % 1000 == 500 {
                    let (_, pred) = RhythmicEnv::eval_at(&pat, song_pos, bpm, decay_ms);
                    max_err = max_err.max((rt - f64::from(pred)).abs());
                }
            }
            assert!(
                max_err < 0.03,
                "pattern {label}: RT vs analytical max error {max_err}"
            );

            // Stopping transport halts immediately without double-triggering.
            let stop_pos = 8.0;
            env.sync_song_pos(pat, decay_ms, stop_pos, bpm);
            env.sync_song_pos(pat, decay_ms, stop_pos, bpm);
            let before = env.current();
            for _ in 0..500 {
                env.next_sample();
            }
            assert!(
                env.current() <= before + 1e-4,
                "stopped transport must not double-trigger"
            );

            let preview = RhythmicEnv::preview(&pat, decay_ms, bpm, 4);
            assert_eq!(preview.len(), usize::from(pat.length) * 4);
            assert!(preview
                .iter()
                .all(|v| v.is_finite() && (0.0..=1.0).contains(v)));
        }
    }

    #[test]
    fn modular_env_delay_multi_burst_hold_decay_choke_and_curve_preview() {
        let sr = 48_000.0;

        // 1. EnvMode SPOT roundtrip.
        for (i, &mode) in EnvMode::ALL.iter().enumerate() {
            assert_eq!(mode.to_index(), i);
            assert_eq!(EnvMode::from_index(i), mode);
            assert_eq!(EnvMode::parse(mode.slug()), Some(mode));
            assert_eq!(EnvMode::parse(mode.label()), Some(mode));
        }

        // 2. Pre-delay stays silent for delay_ms then fires cleanly.
        let delay_params = ModularEnvParams {
            mode: EnvMode::HoldDecay,
            hold_ms: 2.0,
            decay_ms: 40.0,
            trigger_mode: TriggerMode::OneShot,
            delay_ms: 10.0,
            bursts: 1,
            ..ModularEnvParams::default()
        };
        let mut env = ModularEnv::new(sr, delay_params);
        let delay_frames = (0.010 * sr) as usize; // 480 frames
        for i in 0..delay_frames {
            assert_eq!(
                env.next(0.0),
                0.0,
                "frame {i} inside pre-delay must be silent"
            );
            assert!(!env.is_done());
        }
        let first_active = env.next(0.0);
        assert!(
            (first_active - 1.0).abs() < 1e-6,
            "HoldDecay must peak at 1.0 right after pre-delay, got {first_active}"
        );

        // Gate release during pre-delay cancels trigger cleanly.
        let mut gated_delay = ModularEnv::new(
            sr,
            ModularEnvParams {
                trigger_mode: TriggerMode::Gate,
                ..delay_params
            },
        );
        for _ in 0..100 {
            assert_eq!(gated_delay.next(0.0), 0.0);
        }
        gated_delay.release();
        assert!(gated_delay.is_done());
        for _ in 0..1000 {
            assert_eq!(gated_delay.next(0.0), 0.0);
        }

        // 3. Multi-burst clap envelope (4 bursts @ 10 ms spacing, 180 ms final tail).
        let clap = ModularEnvParams {
            mode: EnvMode::HoldDecay,
            hold_ms: 0.5,
            decay_ms: 180.0,
            trigger_mode: TriggerMode::OneShot,
            delay_ms: 0.0,
            bursts: 4,
            burst_spacing_ms: 10.0,
            ..ModularEnvParams::default()
        };
        let mut clap_env = ModularEnv::new(sr, clap);
        let spacing = (0.010 * sr) as usize;
        let mut peak_frames = Vec::new();
        let mut prev = 0.0;
        for i in 0..(spacing * 5) {
            let v = clap_env.next(0.0);
            if v >= 0.999 && prev < 0.9 {
                peak_frames.push(i);
            }
            prev = v;
        }
        assert_eq!(
            peak_frames.len(),
            4,
            "4-burst clap should emit 4 distinct attacks, got {peak_frames:?}"
        );
        assert!(
            !clap_env.is_done(),
            "final 180ms tail should still be ringing after 50ms"
        );
        // Choke cuts remaining tail cleanly in ~3.5 ms.
        clap_env.choke(sr);
        for _ in 0..400 {
            clap_env.next(0.0);
        }
        assert!(clap_env.is_done(), "choke must terminate envelope quickly");

        // 4. eval_modular_env_curve matches eval_adsr_curve for plain ADSR and captures bursts/delay.
        let plain = ModularEnvParams::default();
        assert_eq!(
            eval_modular_env_curve(plain, 64),
            eval_adsr_curve(
                plain.attack_ms,
                plain.decay_ms,
                plain.sustain,
                plain.release_ms,
                false,
                64
            )
        );
        let clap_curve = eval_modular_env_curve(clap, 64);
        assert_eq!(clap_curve.len(), 64);
        assert!(clap_curve.windows(2).all(|w| w[1].0 >= w[0].0));
        assert!(clap_curve
            .iter()
            .all(|&(x, y)| x.is_finite() && y.is_finite() && (0.0..=1.0).contains(&y)));
        assert_eq!(clap_curve.last().unwrap().1, 0.0);
        let peak_count = clap_curve.iter().filter(|&&(_, y)| y > 0.95).count();
        assert!(
            peak_count >= 4,
            "clap visual curve should preserve all 4 burst peaks, got {peak_count}"
        );

        let delayed_curve = eval_modular_env_curve(delay_params, 64);
        assert_eq!(delayed_curve.len(), 64);
        assert_eq!(delayed_curve[0], (0.0, 0.0));
        assert_eq!(delayed_curve.last().unwrap().1, 0.0);
    }

    #[test]
    fn rhythmic_env_eval_and_preview_sanitize_nan_decay_and_bpm() {
        let mut pat = CompiledFilterPattern {
            length: 4,
            ..CompiledFilterPattern::default()
        };
        pat.steps[0] = 127;
        pat.steps[2] = 96;

        let (slot, val) = RhythmicEnv::eval_at(&pat, 0.5, f64::NAN, f32::NAN);
        assert!(slot < 4 && val.is_finite() && (0.0..=1.0).contains(&val));

        let prev = RhythmicEnv::preview(&pat, f32::NAN, f64::NAN, 8);
        assert_eq!(prev.len(), 32);
        assert!(prev
            .iter()
            .all(|v| v.is_finite() && (0.0..=1.0).contains(v)));
    }

    #[test]
    fn choke_and_hold_decay_silence_floor_stay_aligned_across_sample_rates() {
        for sr in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            let choke_frames = (CHOKE_MS * 0.001 * sr).round() as usize;

            // 1. AdsrState choke decays at CHOKE_MS to -60 dB and finishes cleanly.
            let mut adsr = AdsrState::new(sr, 0.5, 40.0, 500.0, TriggerMode::Gate);
            for _ in 0..((sr * 0.002) as usize) {
                adsr.next(1.0);
            }
            assert!((adsr.level() - 1.0).abs() < 1e-6);
            adsr.choke(sr);
            let mut prev = adsr.level();
            for _ in 0..(choke_frames.saturating_sub(1)) {
                let v = adsr.next(1.0);
                assert!(v <= prev && !adsr.is_done());
                prev = v;
            }
            assert!(
                (prev - 1e-3).abs() < 1e-3,
                "sr={sr}: expected ~-60 dB near end of CHOKE_MS, got {prev}"
            );
            assert_eq!(adsr.next(1.0), 0.0);
            assert!(adsr.is_done());

            // 2. ModularEnv HoldDecay choke uses identical CHOKE_MS rate and terminates at TAIL_SILENCE_FLOOR_F64.
            let mut mod_hd = ModularEnv::new(
                sr,
                ModularEnvParams {
                    mode: EnvMode::HoldDecay,
                    hold_ms: 50.0,
                    decay_ms: 500.0,
                    trigger_mode: TriggerMode::OneShot,
                    ..ModularEnvParams::default()
                },
            );
            let _ = mod_hd.next(0.0);
            mod_hd.choke(sr);
            for _ in 0..choke_frames {
                let _ = mod_hd.next(0.0);
            }
            assert!(
                (mod_hd.level() - 1e-3).abs() < 5e-4,
                "sr={sr}: HoldDecay choke after CHOKE_MS expected ~1e-3, got {}",
                mod_hd.level()
            );
            let full_drain = (CHOKE_MS * 0.001 * 2.5 * sr).ceil() as usize;
            for _ in 0..full_drain {
                let _ = mod_hd.next(0.0);
            }
            assert!(mod_hd.is_done(), "sr={sr}: HoldDecay choke must complete");
            assert_eq!(mod_hd.level(), 0.0);

            // 3. Standalone HoldDecayEnv flushes cleanly to 0.0 once below TAIL_SILENCE_FLOOR_F64.
            let mut hd = HoldDecayEnv::new(sr, 0.0, 20.0);
            let mut saw_floor_flush = false;
            let limit = (sr * 0.08) as usize;
            for _ in 0..limit {
                let _ = hd.next_sample();
                let lvl = hd.level();
                assert!(lvl == 0.0 || lvl >= TAIL_SILENCE_FLOOR_F64);
                if lvl == 0.0 {
                    saw_floor_flush = true;
                    break;
                }
            }
            assert!(
                saw_floor_flush,
                "sr={sr}: HoldDecayEnv must flush cleanly at TAIL_SILENCE_FLOOR_F64"
            );
        }
    }

    #[test]
    fn capacitor_env_matches_zero_hz_capacitor_ode_and_completes_across_sample_rates() {
        assert_eq!(EnvMode::ALL.len(), 3);
        for alias in ["capacitor", "cap", "rc", "c13", "303", "2"] {
            assert_eq!(EnvMode::parse(alias), Some(EnvMode::Capacitor));
        }

        // Default pulse width: hold_ms = 0.0 derives (attack_ms * 2.0).clamp(4.0, 20.0) ms
        // (matching the TB-303's 4 ms trigger pulse at attack_ms = 2.0 ms).
        let default_303 = ModularEnvParams {
            mode: EnvMode::Capacitor,
            attack_ms: 2.0,
            hold_ms: 0.0,
            decay_ms: 145.0,
            cubic_limit: 1.4,
            trigger_mode: TriggerMode::OneShot,
            ..ModularEnvParams::default()
        };
        assert!((default_303.capacitor_pulse_ms() - 4.0).abs() < 1e-6);

        let explicit_hold = ModularEnvParams {
            hold_ms: 12.5,
            ..default_303
        };
        assert!((explicit_hold.capacitor_pulse_ms() - 12.5).abs() < 1e-6);

        for sr in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            let pulse_samples = (f64::from(default_303.capacitor_pulse_ms()) * 0.001 * sr)
                .round()
                .max(1.0) as u32;

            // Exact parity against ZeroHzCapacitor across both charge and cubic discharge.
            let mut env = ModularEnv::new(sr, default_303);
            let mut ref_cap = ZeroHzCapacitor::new(
                sr,
                (f64::from(default_303.decay_ms) * 0.001) / DECAY_LN_60DB,
                f64::from(default_303.cubic_limit),
                (f64::from(default_303.attack_ms) * 0.001) / LN_100,
            );
            ref_cap.trigger(1.0, pulse_samples);

            assert_eq!(env.stage(), AdsrStage::Attack);
            assert!(!env.is_done());

            let compare_frames = (sr * 0.08) as usize;
            let mut peak = 0.0_f64;
            for i in 0..compare_frames {
                let got = env.next(0.0);
                let expected = ref_cap.step();
                assert!(
                    (got - expected).abs() < 1e-13,
                    "sr={sr} frame={i}: ModularEnv({got}) != ZeroHzCapacitor({expected})"
                );
                peak = peak.max(got);
                if i == pulse_samples as usize {
                    assert_eq!(env.stage(), AdsrStage::Decay);
                }
            }
            assert!(
                peak > 0.99 && peak <= 1.0,
                "sr={sr}: default 4ms TB-303 pulse should charge cleanly to unity, got {peak}"
            );

            // Cubic limit > 0 decays strictly faster at high charge than pure linear RC (g = 0).
            let mut linear_env = ModularEnv::new(
                sr,
                ModularEnvParams {
                    cubic_limit: 0.0,
                    ..default_303
                },
            );
            for _ in 0..compare_frames {
                linear_env.next(0.0);
            }
            assert!(
                env.level() < linear_env.level(),
                "sr={sr}: cubic_limit > 0 should decay faster than linear RC ({} vs {})",
                env.level(),
                linear_env.level()
            );

            // Continues decaying smoothly down to is_done() == true with zero latched voices.
            let max_tail = (sr * 3.0) as usize;
            let mut completed = false;
            for _ in 0..max_tail {
                let v = env.next(0.0);
                assert!(v.is_finite() && v >= 0.0);
                if env.is_done() {
                    completed = true;
                    break;
                }
            }
            assert!(
                completed,
                "sr={sr}: Capacitor envelope must reach is_done()"
            );
            assert_eq!(env.level(), 0.0);
            assert_eq!(env.stage(), AdsrStage::Done);
            assert_eq!(env.next(0.0), 0.0);

            // Choke drains Capacitor by ~-60 dB in CHOKE_MS and completes cleanly.
            let mut choked = ModularEnv::new(
                sr,
                ModularEnvParams {
                    decay_ms: 2_000.0,
                    ..default_303
                },
            );
            for _ in 0..pulse_samples {
                choked.next(0.0);
            }
            let pre_choke = choked.level();
            assert!(pre_choke > 0.8);
            choked.choke(sr);
            assert!(choked.is_releasing());
            let choke_frames = (CHOKE_MS * 0.001 * sr).round() as usize;
            for _ in 0..choke_frames {
                choked.next(0.0);
            }
            let ratio = choked.level() / pre_choke;
            assert!(
                (ratio - 1e-3).abs() < 5e-4,
                "sr={sr}: Capacitor choke after CHOKE_MS expected ~1e-3 ratio, got {ratio}"
            );
            let drain_frames = (CHOKE_MS * 0.001 * 2.5 * sr).ceil() as usize;
            for _ in 0..drain_frames {
                choked.next(0.0);
            }
            assert!(
                choked.is_done() && choked.level() == 0.0,
                "sr={sr}: Capacitor choke must complete cleanly"
            );
        }
    }

    #[test]
    fn capacitor_env_trigger_modes_additive_stacking_bursts_delay_legato_and_live_updates() {
        let sr = 48_000.0;

        // 1. new_idle starts silent/done and activates cleanly on retrigger.
        let base = ModularEnvParams {
            mode: EnvMode::Capacitor,
            attack_ms: 2.0,
            hold_ms: 0.0,
            decay_ms: 420.0,
            cubic_limit: 1.8,
            trigger_mode: TriggerMode::Additive,
            ..ModularEnvParams::default()
        };
        let mut idle = ModularEnv::new_idle(sr, base);
        assert!(idle.is_done());
        assert_eq!(idle.stage(), AdsrStage::Done);
        assert_eq!(idle.next(0.0), 0.0);
        idle.retrigger(sr, base);
        assert!(!idle.is_done());
        assert!(idle.next(0.0) > 0.0);

        // 2. TriggerMode::Gate cuts active charging pulse on release() and cancels pre-delay.
        let gate_params = ModularEnvParams {
            mode: EnvMode::Capacitor,
            attack_ms: 10.0,
            hold_ms: 80.0,
            decay_ms: 120.0,
            cubic_limit: 0.5,
            trigger_mode: TriggerMode::Gate,
            ..ModularEnvParams::default()
        };
        let mut gate_env = ModularEnv::new(sr, gate_params);
        let cut_frames = (0.0015 * sr) as usize;
        for _ in 0..cut_frames {
            gate_env.next(0.0);
        }
        let at_release = gate_env.level();
        assert!(at_release > 0.2 && at_release < 0.7);
        gate_env.release();
        assert!(gate_env.is_releasing());
        assert_eq!(gate_env.stage(), AdsrStage::Release);
        let after_release = gate_env.next(0.0);
        assert!(
            after_release < at_release,
            "Gate release must immediately enter discharge: before={at_release}, after={after_release}"
        );

        let mut gated_delay = ModularEnv::new(
            sr,
            ModularEnvParams {
                delay_ms: 15.0,
                ..gate_params
            },
        );
        for _ in 0..100 {
            assert_eq!(gated_delay.next(0.0), 0.0);
        }
        gated_delay.release();
        assert!(gated_delay.is_done());
        for _ in 0..1000 {
            assert_eq!(gated_delay.next(0.0), 0.0);
        }

        // 3. TriggerMode::OneShot ignores release() during pre-delay and charging.
        let mut os_env = ModularEnv::new(
            sr,
            ModularEnvParams {
                trigger_mode: TriggerMode::OneShot,
                delay_ms: 5.0,
                ..gate_params
            },
        );
        os_env.release();
        assert!(!os_env.is_done());
        for _ in 0..((0.025 * sr) as usize) {
            os_env.next(0.0);
        }
        assert!(os_env.level() > 0.5);

        // 4. TriggerMode::Additive stacks charge across rapid retriggers and self-limits via cubic_limit.
        let run_additive_train = |cubic_limit: f32| -> (f64, f64) {
            let p = ModularEnvParams {
                cubic_limit,
                ..base
            };
            let pulse_frames = (f64::from(p.capacitor_pulse_ms()) * 0.001 * sr).round() as usize;
            let gap_frames = (0.040 * sr) as usize;
            let mut env = ModularEnv::new(sr, p);
            let mut first_peak = 0.0_f64;
            let mut last_peak = 0.0_f64;
            for hit in 0..24 {
                if hit > 0 {
                    env.retrigger_with_amount(sr, p, 0.9);
                }
                env.release(); // Additive ignores release()
                let mut hit_peak = 0.0_f64;
                for _ in 0..pulse_frames {
                    hit_peak = hit_peak.max(env.next(0.0));
                }
                if hit == 0 {
                    first_peak = hit_peak;
                }
                last_peak = hit_peak;
                for _ in 0..gap_frames {
                    let v = env.next(0.0);
                    assert!(v.is_finite() && v >= 0.0);
                }
            }
            (first_peak, last_peak)
        };
        let (first_lim, last_lim) = run_additive_train(1.8);
        let (_, last_strong_lim) = run_additive_train(5.0);
        assert!(
            last_lim > first_lim && last_lim > 1.0,
            "Additive retriggers should stack charge above 1.0: first={first_lim}, last={last_lim}"
        );
        assert!(
            last_lim < 5.0,
            "cubic_limit = 1.8 must self-limit stacked accents without diverging: {last_lim}"
        );
        assert!(
            last_strong_lim < last_lim,
            "Higher cubic_limit should self-limit to a lower equilibrium: g=5 -> {last_strong_lim} vs g=1.8 -> {last_lim}"
        );

        // 5. Delay + multi-burst stacking in EnvMode::Capacitor + TriggerMode::Additive.
        let burst_add = ModularEnvParams {
            mode: EnvMode::Capacitor,
            attack_ms: 2.0,
            hold_ms: 0.0,
            decay_ms: 120.0,
            cubic_limit: 1.5,
            trigger_mode: TriggerMode::Additive,
            delay_ms: 8.0,
            bursts: 4,
            burst_spacing_ms: 10.0,
            ..ModularEnvParams::default()
        };
        let mut b_env = ModularEnv::new(sr, burst_add);
        let delay_frames = (0.008 * sr) as usize;
        for _ in 0..delay_frames {
            assert_eq!(b_env.next(0.0), 0.0);
            assert!(!b_env.is_done());
        }
        let spacing = (0.010 * sr) as usize;
        let mut burst_peaks = [0.0_f64; 4];
        for slot in &mut burst_peaks {
            let mut pk = 0.0_f64;
            for _ in 0..spacing {
                pk = pk.max(b_env.next(0.0));
            }
            *slot = pk;
        }
        assert!(
            burst_peaks[3] > burst_peaks[0] && burst_peaks[3] > 1.5,
            "4-burst Additive capacitor should stack charge across bursts: {burst_peaks:?}"
        );
        for _ in 0..((sr * 2.5) as usize) {
            b_env.next(0.0);
            if b_env.is_done() {
                break;
            }
        }
        assert!(b_env.is_done());

        // 6. Legato adopt_progress_from and continuous set_cubic_limit / set_sustain preserve charge.
        let mut prev = ModularEnv::new(sr, base);
        for _ in 0..600 {
            prev.next(0.0);
        }
        let charge_before = prev.level();
        assert!(charge_before > 0.5);

        prev.set_cubic_limit(4.5);
        prev.set_sustain(0.35);
        assert!(
            (prev.level() - charge_before).abs() < 1e-12,
            "set_cubic_limit/set_sustain must not reset capacitor charge"
        );
        assert!((prev.params().cubic_limit - 4.5).abs() < 1e-6);
        assert!((prev.params().sustain - 0.35).abs() < 1e-6);

        let next_params = ModularEnvParams {
            decay_ms: 300.0,
            cubic_limit: 0.8,
            ..base
        };
        let mut legato = ModularEnv::new(sr, next_params);
        legato.adopt_progress_from(prev);
        assert!(
            (legato.level() - charge_before).abs() < 1e-12,
            "adopt_progress_from must preserve live capacitor charge"
        );
        let legato_next = legato.next(0.0);
        assert!(
            (legato_next - charge_before).abs() < 0.02,
            "legato transition must remain click-free: before={charge_before}, next={legato_next}"
        );

        // 7. TriggerMode::Additive on Adsr behaves like OneShot with non-resetting retrigger.
        let adsr_add = ModularEnvParams {
            mode: EnvMode::Adsr,
            attack_ms: 5.0,
            decay_ms: 40.0,
            sustain: 0.9,
            release_ms: 200.0,
            trigger_mode: TriggerMode::Additive,
            ..ModularEnvParams::default()
        };
        let mut a_env = ModularEnv::new(sr, adsr_add);
        for _ in 0..400 {
            a_env.next(0.9);
        }
        let before_retrig = a_env.level();
        assert!(before_retrig > 0.5);
        a_env.retrigger(sr, adsr_add);
        let after_retrig = a_env.next(0.9);
        assert!(
            after_retrig >= before_retrig,
            "Adsr Additive retrigger must continue from current level: before={before_retrig}, after={after_retrig}"
        );
        a_env.release(); // ignored in Additive
        for _ in 0..5000 {
            a_env.next(0.9);
        }
        assert!(
            a_env.is_done(),
            "Adsr Additive should terminate like OneShot"
        );
    }

    #[test]
    fn eval_modular_env_curve_capacitor_modes_bursts_delay_and_cubic_limit() {
        let single = ModularEnvParams {
            mode: EnvMode::Capacitor,
            attack_ms: 2.0,
            hold_ms: 0.0,
            decay_ms: 145.0,
            cubic_limit: 0.0,
            trigger_mode: TriggerMode::OneShot,
            ..ModularEnvParams::default()
        };
        let curve_lin = eval_modular_env_curve(single, 64);
        assert_eq!(curve_lin.len(), 64);
        assert_eq!(curve_lin.first().unwrap().0, 0.0);
        assert_eq!(curve_lin.first().unwrap().1, 0.0);
        assert_eq!(curve_lin.last().unwrap().0, 1.0);
        assert_eq!(curve_lin.last().unwrap().1, 0.0);
        assert!(curve_lin.windows(2).all(|w| w[1].0 >= w[0].0));
        assert!(curve_lin.iter().all(|&(x, y)| x.is_finite()
            && y.is_finite()
            && (0.0..=1.0).contains(&x)
            && (0.0..=1.0).contains(&y)));

        let (peak_idx, peak_val) = curve_lin
            .iter()
            .enumerate()
            .max_by(|a, b| a.1 .1.total_cmp(&b.1 .1))
            .map(|(i, &(_, y))| (i, y))
            .unwrap();
        assert!(
            peak_idx > 0 && peak_idx < curve_lin.len() - 1 && peak_val > 0.80,
            "Capacitor curve must rise then decay with clear peak: idx={peak_idx}, peak={peak_val}"
        );
        // Strictly rising before peak and monotonically decaying after peak.
        assert!(curve_lin[..=peak_idx]
            .windows(2)
            .all(|w| w[1].1 >= w[0].1 - 1e-5));
        assert!(curve_lin[peak_idx..]
            .windows(2)
            .all(|w| w[1].1 <= w[0].1 + 1e-5));

        // Higher cubic_limit bends the post-peak discharge strictly lower than pure linear RC.
        let curve_cubic = eval_modular_env_curve(
            ModularEnvParams {
                cubic_limit: 6.0,
                ..single
            },
            64,
        );
        assert_eq!(curve_cubic.len(), 64);
        let mid_idx = (peak_idx + curve_lin.len() - 1) / 2;
        assert!(
            curve_cubic[mid_idx].1 < curve_lin[mid_idx].1,
            "cubic_limit > 0 should decay faster on visual curve: cubic={} vs lin={}",
            curve_cubic[mid_idx].1,
            curve_lin[mid_idx].1
        );

        // Pre-delay holds at 0.0 before firing.
        let delayed = eval_modular_env_curve(
            ModularEnvParams {
                delay_ms: 40.0,
                ..single
            },
            64,
        );
        assert_eq!(delayed.len(), 64);
        assert_eq!(delayed[0], (0.0, 0.0));
        assert_eq!(delayed[1].1, 0.0);
        assert!(delayed.iter().any(|&(_, y)| y > 0.8));
        assert_eq!(delayed.last().unwrap().1, 0.0);

        // Multi-burst Additive stacks cleanly across bursts and normalizes smoothly into [0, 1].
        let stacked = eval_modular_env_curve(
            ModularEnvParams {
                mode: EnvMode::Capacitor,
                attack_ms: 2.0,
                hold_ms: 0.0,
                decay_ms: 420.0,
                cubic_limit: 1.5,
                trigger_mode: TriggerMode::Additive,
                bursts: 4,
                burst_spacing_ms: 10.0,
                ..ModularEnvParams::default()
            },
            64,
        );
        assert_eq!(stacked.len(), 64);
        assert!(stacked.windows(2).all(|w| w[1].0 >= w[0].0));
        assert!(stacked.iter().all(|&(x, y)| x.is_finite()
            && y.is_finite()
            && (0.0..=1.0).contains(&x)
            && (0.0..=1.0).contains(&y)));
        assert_eq!(stacked.last().unwrap().1, 0.0);
        let stacked_max = stacked.iter().map(|&(_, y)| y).fold(0.0_f32, f32::max);
        assert!(
            (stacked_max - 1.0).abs() < 1e-4,
            "Stacked Additive bursts (> 1.0 charge) should normalize cleanly to peak 1.0, got {stacked_max}"
        );
        // Early first burst peak should sit strictly below the final stacked peak (1.0).
        let early_max = stacked
            .iter()
            .filter(|&&(x, _)| x < 0.18)
            .map(|&(_, y)| y)
            .fold(0.0_f32, f32::max);
        assert!(
            early_max > 0.15 && early_max < 0.6,
            "First Additive burst should sit below final stacked peak: early_max={early_max}"
        );
    }
}
