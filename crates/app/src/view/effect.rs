//! Semantic Effect deck view (`EffectDeckView`) with pre-sampled GPU curves
//! for all 8 native studio insert FX (EQ, Filter/PCF, Compressor, Limiter,
//! Drive, Delay, Chorus, Reverb) plus live Compressor/Limiter gain-reduction
//! metering from `AudioSnapshot`.
use super::instrument::map_controls;
use super::ControlView;
use crate::{App, Detail};
use cadence_core::dsp::chorus::eval_quadrature;
use cadence_core::dsp::{
    db_to_lin_f64, drive_transfer_sample, eq_response_db, filter_mix_magnitude_db, lin_to_db_f64,
    log_to_norm, norm_to_log, octaves_to_ratio_f32, reverb_hf_rt60_envelope, reverb_rt60_envelope,
    sanitize_sample_rate, soft_knee_gr_db_f64, RhythmicEnv, TptOnePoleLpf, COMPRESSOR_KNEE_DB,
    LIMITER_KNEE_DB,
};
use cadence_core::material::Id;
use cadence_core::mixer::{
    ChorusPatch, CompressorPatch, DelayMode, DelayPatch, DriveMode, DrivePatch, EffectKind,
    EffectParam, EqPatch, FilterPatch, LimiterPatch, ReverbPatch,
};
use cadence_core::params::{format_hz, ParamAddress};
use cadence_core::runtime::AudioSnapshot;

const FREQ_MIN_HZ: f32 = 20.0;
const EQ_CURVE_POINTS: usize = 128;
const FILTER_CURVE_POINTS: usize = 128;
const FILTER_ENV_PREVIEW_PPS: usize = 4;
const COMP_CURVE_POINTS: usize = 96;
const LIMITER_CURVE_POINTS: usize = 96;
const DRIVE_CURVE_POINTS: usize = 160;
const TONE_CURVE_POINTS: usize = 64;
const CHORUS_CURVE_POINTS: usize = 96;
const REVERB_CURVE_POINTS: usize = 96;

/// Lower axis bound (dB) of the Compressor input/output transfer curve.
pub const COMP_TRANSFER_MIN_DB: f32 = -48.0;
/// Upper axis bound (dB) of the Compressor input/output transfer curve (+6 dB 32-bit float headroom).
pub const COMP_TRANSFER_MAX_DB: f32 = 6.0;

#[derive(Clone, Debug)]
pub struct EffectDeckView {
    pub id: Id,
    pub name: String,
    pub kind_label: String,
    pub channel_name: String,
    pub bypassed: bool,
    pub controls: Vec<ControlView>,
    pub control_cursor: usize,
    pub visual: EffectVisualView,
}

#[derive(Clone, Copy, Debug)]
pub struct DelayTapView {
    /// Normalized x position in `[0.0, 1.0]` across the step grid.
    pub x_norm: f32,
    /// Echo amplitude in `[0.0, 1.0]`.
    pub amplitude: f32,
    /// True for Left ping-pong tap, false for Right tap.
    pub left_channel: bool,
    pub is_dry: bool,
}

#[derive(Clone, Debug)]
pub struct FilterEnvVisual {
    pub source_label: String,
    pub resolution: u8,
    pub smooth_attack: bool,
    pub steps: Vec<f32>,
    /// Smooth analytical ODE envelope contour across one full cycle (`cadence_core::dsp::RhythmicEnv::preview`).
    pub env_curve: Vec<f32>,
    pub active_step: usize,
    pub env_level: f32,
    pub env_amount: f32,
    pub env_decay_ms: f32,
}

#[derive(Clone, Debug)]
pub enum EffectVisualView {
    Eq {
        /// Upper frequency axis bound in Hz (`min(20_000.0, sample_rate * 0.45)`).
        freq_high_hz: f32,
        /// 128 log-spaced points (`20 Hz..freq_high_hz`) of combined RBJ magnitude in dB clamped to `[-18.0, 18.0]`.
        curve_db: Vec<f32>,
        /// Optional live modulated response curve when any EQ band is automated.
        live_curve_db: Option<Vec<f32>>,
        /// Normalized log-frequency x position `[0..1]` of the HPF cutoff.
        hpf_norm: f32,
        hpf_active: bool,
        hpf_hz: f32,
        low_db: f32,
        mid_db: f32,
        mid_hz: f32,
        mid_q: f32,
        /// Normalized log-frequency x position `[0..1]` of the swept Mid peak.
        mid_hz_norm: f32,
        /// Normalized log-frequency bandwidth span `(lo_norm, hi_norm)` derived from `mid_q`.
        mid_q_span: (f32, f32),
        /// Optional live modulated Mid frequency x position `[0..1]`.
        mod_mid_hz_norm: Option<f32>,
        high_db: f32,
        /// Normalized log-frequency x positions of Low (120 Hz), Mid (`mid_hz`), High (6 kHz).
        band_norms: [f32; 3],
        summary: String,
    },
    Filter {
        /// Upper frequency axis bound in Hz (`min(20_000.0, sample_rate * 0.45)`).
        freq_high_hz: f32,
        mode_label: &'static str,
        cutoff_hz: f32,
        resonance: f32,
        drive: f32,
        mix: f32,
        /// 128 log-spaced points (`20 Hz..freq_high_hz`) in dB clamped to `[-48.0, 18.0]` (including parallel `mix`).
        curve_db: Vec<f32>,
        /// Optional live modulated response curve when Filter params are automated.
        mod_curve_db: Option<Vec<f32>>,
        /// Base cutoff log-frequency x position `[0..1]`.
        cutoff_norm: f32,
        /// Live modulated cutoff log-frequency x position `[0..1]`.
        mod_cutoff_norm: Option<f32>,
        mod_cutoff_hz: Option<f32>,
        /// Active rhythmic envelope pattern & live step state when `env_amount != 0`.
        env: Option<FilterEnvVisual>,
        summary: String,
    },
    Compressor {
        /// 96 normalized `(in_norm, out_norm)` pairs in `[0..1]` on `-48..+6 dB` axes (with makeup + mix).
        transfer_curve: Vec<(f32, f32)>,
        /// 96 normalized `(in_norm, out_norm)` pairs without makeup lift (raw compression bend).
        compression_curve: Vec<(f32, f32)>,
        /// Normalized threshold position in `[0..1]` on `-48..+6 dB`.
        threshold_norm: f32,
        /// Soft knee span `(start_norm, end_norm)` around threshold (`±3 dB`).
        knee_span: (f32, f32),
        /// Live gain reduction in dB (`>= 0.0`) from `AudioSnapshot`.
        gain_reduction_db: f32,
        /// Peak-held gain reduction in dB (`>= 0.0`) from `AudioSnapshot`.
        gr_hold_db: f32,
        threshold_db: f32,
        ratio: f32,
        attack_ms: f32,
        release_ms: f32,
        makeup_db: f32,
        mix: f32,
        summary: String,
    },
    Limiter {
        ceiling_db: f32,
        input_drive_db: f32,
        release_ms: f32,
        mix: f32,
        /// 96 normalized `(in_norm, out_norm)` pairs in `[0..1]` on `-48..0 dB` axes.
        transfer_curve: Vec<(f32, f32)>,
        /// Live gain reduction in dB (`>= 0.0`) from `AudioSnapshot`.
        gain_reduction_db: f32,
        /// Peak-held gain reduction in dB (`>= 0.0`) from `AudioSnapshot`.
        gr_hold_db: f32,
        modulated: bool,
        /// Normalized ceiling height in `[0..1]` on `-48..0 dB`.
        ceiling_norm: f32,
        summary: String,
    },
    Drive {
        mode: DriveMode,
        /// Upper frequency axis bound in Hz (`min(20_000.0, sample_rate * 0.45)`) for the tone LPF plot.
        freq_high_hz: f32,
        /// 160 `(x in -1..=1, y in -1.25..=1.25)` saturator/wavefolder transfer curve points.
        curve: Vec<(f32, f32)>,
        /// 64 log-spaced points (`20 Hz..freq_high_hz`) of post-saturation warmth tone filter in dB `[-24..3]`.
        tone_curve_db: Vec<f32>,
        /// Normalized log-frequency x position `[0..1]` of `tone_hz`.
        tone_norm: f32,
        drive: f32,
        asymmetry: f32,
        tone_hz: f32,
        output_db: f32,
        mix: f32,
        summary: String,
    },
    Delay {
        mode: DelayMode,
        steps: f32,
        delay_ms: f32,
        feedback: f32,
        mix: f32,
        grid_steps: f32,
        taps: Vec<DelayTapView>,
        summary: String,
    },
    Chorus {
        /// 96 samples in `[-1.0, 1.0]` for Left channel quadrature LFO.
        left_lfo: Vec<f32>,
        /// 96 samples in `[-1.0, 1.0]` for Right channel quadrature LFO.
        right_lfo: Vec<f32>,
        rate_hz: f32,
        depth: f32,
        width: f32,
        mix: f32,
        phase_deg: f32,
        summary: String,
    },
    Reverb {
        /// Upper frequency axis bound in Hz (`min(20_000.0, sample_rate * 0.45)`) for the HF damping plot.
        freq_high_hz: f32,
        /// 96 samples in `[0.0, 1.0]` of full-band RT60 decay envelope (including predelay gap).
        decay_curve: Vec<f32>,
        /// 96 samples in `[0.0, 1.0]` of high-frequency damped decay envelope.
        hf_decay_curve: Vec<f32>,
        /// 64 log-spaced points (`20 Hz..freq_high_hz`) of damping LPF response in dB `[-24..3]`.
        damping_curve_db: Vec<f32>,
        predelay_norm: f32,
        damping_norm: f32,
        window_sec: f32,
        size: f32,
        decay_seconds: f32,
        damping_hz: f32,
        predelay_ms: f32,
        width: f32,
        mix: f32,
        summary: String,
    },
    Missing,
}

fn effective_sr(sample_rate: u32) -> f64 {
    if sample_rate < 8_000 {
        48_000.0
    } else {
        sanitize_sample_rate(f64::from(sample_rate))
    }
}

fn freq_high_hz(sr: f64) -> f32 {
    let sr = sanitize_sample_rate(sr);
    (sr * 0.45).clamp(4_000.0, 20_000.0) as f32
}

fn sample_eq_curve(patch: &EqPatch, sr: f64) -> Vec<f32> {
    let high_hz = freq_high_hz(sr);
    (0..EQ_CURVE_POINTS)
        .map(|i| {
            let x = i as f32 / (EQ_CURVE_POINTS - 1) as f32;
            let f = norm_to_log(x, FREQ_MIN_HZ, high_hz);
            let db = eq_response_db(patch, f64::from(f), sr);
            if db.is_finite() {
                db.clamp(-18.0, 18.0) as f32
            } else {
                0.0
            }
        })
        .collect()
}

pub fn build_eq_visual(patch: &EqPatch, mod_patch: Option<&EqPatch>, sr: f64) -> EffectVisualView {
    let sr = sanitize_sample_rate(sr);
    let high_hz = freq_high_hz(sr);
    let curve_db = sample_eq_curve(patch, sr);
    let live_curve_db = mod_patch.map(|p| sample_eq_curve(p, sr));
    let hpf_active = patch.hpf_hz > 21.0;
    let mid_hz = patch.mid_hz.clamp(160.0, 6000.0);
    let mid_q = patch.mid_q.clamp(0.3, 8.0);
    let mid_hz_norm = log_to_norm(mid_hz, FREQ_MIN_HZ, high_hz);
    // Exact half-gain Q bandwidth edges: f_hi/f0 - f_lo/f0 = 1/Q, f_lo * f_hi = f0^2.
    let inv_2q = 0.5 / f64::from(mid_q);
    let root = (1.0 + inv_2q * inv_2q).sqrt();
    let f_lo = (f64::from(mid_hz) * (root - inv_2q)) as f32;
    let f_hi = (f64::from(mid_hz) * (root + inv_2q)) as f32;
    let mid_q_span = (
        log_to_norm(f_lo, FREQ_MIN_HZ, high_hz),
        log_to_norm(f_hi, FREQ_MIN_HZ, high_hz),
    );
    let mod_mid_hz_norm =
        mod_patch.map(|p| log_to_norm(p.mid_hz.clamp(160.0, 6000.0), FREQ_MIN_HZ, high_hz));

    let fmt_signed = |db: f32| {
        if db.abs() < 0.05 {
            "0.0".into()
        } else {
            format!("{db:+.1}")
        }
    };
    let summary = format!(
        "HPF {} · L {} · M {} @{} Q{:.2} · H {} dB",
        if hpf_active {
            format_hz(patch.hpf_hz)
        } else {
            "OFF".into()
        },
        fmt_signed(patch.low_db),
        fmt_signed(patch.mid_db),
        format_hz(mid_hz),
        mid_q,
        fmt_signed(patch.high_db),
    );
    EffectVisualView::Eq {
        freq_high_hz: high_hz,
        curve_db,
        live_curve_db,
        hpf_norm: log_to_norm(patch.hpf_hz, FREQ_MIN_HZ, high_hz),
        hpf_active,
        hpf_hz: patch.hpf_hz,
        low_db: patch.low_db,
        mid_db: patch.mid_db,
        mid_hz,
        mid_q,
        mid_hz_norm,
        mid_q_span,
        mod_mid_hz_norm,
        high_db: patch.high_db,
        band_norms: [
            log_to_norm(120.0, FREQ_MIN_HZ, high_hz),
            mid_hz_norm,
            log_to_norm(6000.0, FREQ_MIN_HZ, high_hz),
        ],
        summary,
    }
}

fn sample_filter_curve(patch: &FilterPatch, sr: f64) -> Vec<f32> {
    let high_hz = freq_high_hz(sr);
    (0..FILTER_CURVE_POINTS)
        .map(|i| {
            let x = i as f32 / (FILTER_CURVE_POINTS - 1) as f32;
            let f = norm_to_log(x, FREQ_MIN_HZ, high_hz);
            let db = filter_mix_magnitude_db(
                patch.mode,
                patch.cutoff_hz,
                patch.resonance,
                patch.mix,
                f64::from(f),
                sr,
            );
            if db.is_finite() {
                db.clamp(-48.0, 18.0) as f32
            } else {
                -48.0
            }
        })
        .collect()
}

pub fn build_filter_visual(
    patch: &FilterPatch,
    mod_patch: Option<&FilterPatch>,
    mod_cutoff_hz: Option<f32>,
    env_visual: Option<FilterEnvVisual>,
    sr: f64,
) -> EffectVisualView {
    let sr = sanitize_sample_rate(sr);
    let high_hz = freq_high_hz(sr);
    let max_cutoff_hz = (sr * 0.45) as f32;
    let effective_base = mod_patch.unwrap_or(patch);
    let curve_db = sample_filter_curve(patch, sr);
    let mut swept_cutoff_hz = mod_cutoff_hz.map(|hz| hz.clamp(FREQ_MIN_HZ, max_cutoff_hz));
    let mut mod_curve_db = mod_patch.map(|p| sample_filter_curve(p, sr));
    if let Some(ref ev) = env_visual {
        let base_hz = swept_cutoff_hz.unwrap_or(effective_base.cutoff_hz);
        let live_hz = (base_hz * octaves_to_ratio_f32(ev.env_level * ev.env_amount))
            .clamp(FREQ_MIN_HZ, max_cutoff_hz);
        let swept_patch = FilterPatch {
            cutoff_hz: live_hz,
            ..*effective_base
        };
        swept_cutoff_hz = Some(live_hz);
        mod_curve_db = Some(sample_filter_curve(&swept_patch, sr));
    }
    let cutoff_norm = log_to_norm(patch.cutoff_hz, FREQ_MIN_HZ, high_hz);
    let mod_cutoff_norm = swept_cutoff_hz.map(|hz| log_to_norm(hz, FREQ_MIN_HZ, high_hz));
    let active_mode = effective_base.mode;
    let summary = match (&env_visual, swept_cutoff_hz) {
        (Some(ev), Some(hz)) => format!(
            "{} · {} → {} · Q {:.0}% · {:+.1} oct",
            active_mode.label(),
            format_hz(effective_base.cutoff_hz),
            format_hz(hz),
            effective_base.resonance * 100.0,
            ev.env_amount
        ),
        (None, Some(hz)) => format!(
            "{} · {} → {} · Q {:.0}%",
            active_mode.label(),
            format_hz(patch.cutoff_hz),
            format_hz(hz),
            effective_base.resonance * 100.0
        ),
        _ => format!(
            "{} · {} · Q {:.0}% · Mix {:.0}%",
            patch.mode.label(),
            format_hz(patch.cutoff_hz),
            patch.resonance * 100.0,
            patch.mix * 100.0
        ),
    };
    EffectVisualView::Filter {
        freq_high_hz: high_hz,
        mode_label: active_mode.label(),
        cutoff_hz: patch.cutoff_hz,
        resonance: effective_base.resonance,
        drive: effective_base.drive,
        mix: effective_base.mix,
        curve_db,
        mod_curve_db,
        cutoff_norm,
        mod_cutoff_norm,
        mod_cutoff_hz: swept_cutoff_hz,
        env: env_visual,
        summary,
    }
}

pub fn build_compressor_visual(
    patch: &CompressorPatch,
    gain_reduction_db: f32,
    gr_hold_db: f32,
) -> EffectVisualView {
    let min_db = f64::from(COMP_TRANSFER_MIN_DB);
    let max_db = f64::from(COMP_TRANSFER_MAX_DB);
    let span_db = max_db - min_db;
    let db_to_norm = |db: f64| ((db - min_db) / span_db).clamp(0.0, 1.0) as f32;

    let thresh = f64::from(patch.threshold_db).clamp(-48.0, 0.0);
    let ratio = f64::from(patch.ratio).clamp(1.0, 20.0);
    let knee = f64::from(COMPRESSOR_KNEE_DB);
    let makeup = f64::from(patch.makeup_db).clamp(-6.0, 24.0);
    let mix = f64::from(patch.mix).clamp(0.0, 1.0);
    let gr = if gain_reduction_db.is_finite() {
        gain_reduction_db.clamp(0.0, 48.0)
    } else {
        0.0
    };
    let gr_hold = if gr_hold_db.is_finite() {
        gr_hold_db.clamp(0.0, 48.0).max(gr)
    } else {
        gr
    };

    let mut transfer_curve = Vec::with_capacity(COMP_CURVE_POINTS);
    let mut compression_curve = Vec::with_capacity(COMP_CURVE_POINTS);
    for i in 0..COMP_CURVE_POINTS {
        let xn = i as f64 / (COMP_CURVE_POINTS - 1) as f64;
        let in_db = min_db + xn * span_db;
        let red_db = soft_knee_gr_db_f64(in_db, thresh, ratio, knee);
        let raw_db = (in_db - red_db).clamp(min_db, max_db);
        compression_curve.push((xn as f32, db_to_norm(raw_db)));

        let wet_db = in_db - red_db + makeup;
        let out_lin = (1.0 - mix) * db_to_lin_f64(in_db) + mix * db_to_lin_f64(wet_db);
        let out_db = lin_to_db_f64(out_lin).clamp(min_db, max_db);
        transfer_curve.push((xn as f32, db_to_norm(out_db)));
    }

    let half_knee = knee * 0.5;
    let threshold_norm = db_to_norm(thresh);
    let knee_lo = db_to_norm(thresh - half_knee);
    let knee_hi = db_to_norm(thresh + half_knee);
    let summary = format!(
        "Thresh {:.1} dB · {:.1}:1 · Atk {:.1} ms · Rel {:.0} ms · {:+.1} dB",
        patch.threshold_db, patch.ratio, patch.attack_ms, patch.release_ms, patch.makeup_db
    );
    EffectVisualView::Compressor {
        transfer_curve,
        compression_curve,
        threshold_norm,
        knee_span: (knee_lo, knee_hi),
        gain_reduction_db: gr,
        gr_hold_db: gr_hold,
        threshold_db: patch.threshold_db,
        ratio: patch.ratio,
        attack_ms: patch.attack_ms,
        release_ms: patch.release_ms,
        makeup_db: patch.makeup_db,
        mix: patch.mix,
        summary,
    }
}

pub fn build_limiter_visual(
    patch: &LimiterPatch,
    gain_reduction_db: f32,
    gr_hold_db: f32,
    modulated: bool,
) -> EffectVisualView {
    let ceiling_db = f64::from(patch.ceiling_db).clamp(-18.0, -0.1);
    let drive_db = f64::from(patch.input_drive_db).clamp(0.0, 24.0);
    let release_ms = patch.release_ms.clamp(10.0, 1000.0);
    let mix = f64::from(patch.mix).clamp(0.0, 1.0);
    let knee = f64::from(LIMITER_KNEE_DB);
    let gr = if gain_reduction_db.is_finite() {
        gain_reduction_db.clamp(0.0, 48.0)
    } else {
        0.0
    };
    let gr_hold = if gr_hold_db.is_finite() {
        gr_hold_db.clamp(0.0, 48.0).max(gr)
    } else {
        gr
    };

    let mut transfer_curve = Vec::with_capacity(LIMITER_CURVE_POINTS);
    for i in 0..LIMITER_CURVE_POINTS {
        let xn = i as f64 / (LIMITER_CURVE_POINTS - 1) as f64;
        let in_db = -48.0 + xn * 48.0;
        let driven_db = in_db + drive_db;
        let red_db = soft_knee_gr_db_f64(driven_db, ceiling_db, f64::INFINITY, knee);
        let wet_db = (driven_db - red_db).min(ceiling_db);
        let out_lin = (1.0 - mix) * db_to_lin_f64(in_db) + mix * db_to_lin_f64(wet_db);
        let out_db = lin_to_db_f64(out_lin).clamp(-48.0, 0.0);
        let out_y = ((out_db + 48.0) / 48.0).clamp(0.0, 1.0) as f32;
        transfer_curve.push((xn as f32, out_y));
    }

    let ceiling_norm = ((ceiling_db + 48.0) / 48.0).clamp(0.0, 1.0) as f32;
    let summary = format!(
        "Ceil {:.1} dB · Drive +{:.1} dB · Rel {:.0} ms · Mix {:.0}%",
        patch.ceiling_db,
        patch.input_drive_db,
        release_ms,
        patch.mix * 100.0
    );
    EffectVisualView::Limiter {
        ceiling_db: patch.ceiling_db,
        input_drive_db: patch.input_drive_db,
        release_ms,
        mix: patch.mix,
        transfer_curve,
        gain_reduction_db: gr,
        gr_hold_db: gr_hold,
        modulated,
        ceiling_norm,
        summary,
    }
}

fn sample_tpt_lpf_db(cutoff_hz: f32, sr: f64) -> Vec<f32> {
    let high_hz = freq_high_hz(sr);
    (0..TONE_CURVE_POINTS)
        .map(|i| {
            let x = i as f32 / (TONE_CURVE_POINTS - 1) as f32;
            let f = norm_to_log(x, FREQ_MIN_HZ, high_hz);
            let db = TptOnePoleLpf::magnitude_db(f64::from(cutoff_hz), f64::from(f), sr);
            if db.is_finite() {
                db.clamp(-24.0, 3.0) as f32
            } else {
                -24.0
            }
        })
        .collect()
}

pub fn build_drive_visual(patch: &DrivePatch, sr: f64) -> EffectVisualView {
    let sr = sanitize_sample_rate(sr);
    let curve: Vec<(f32, f32)> = (0..DRIVE_CURVE_POINTS)
        .map(|i| {
            let x = -1.0 + 2.0 * (i as f32 / (DRIVE_CURVE_POINTS - 1) as f32);
            let y = drive_transfer_sample(x, patch).clamp(-1.25, 1.25);
            (x, y)
        })
        .collect();

    let high_hz = freq_high_hz(sr);
    let tone_curve_db = sample_tpt_lpf_db(patch.tone_hz, sr);
    let tone_norm = log_to_norm(patch.tone_hz, FREQ_MIN_HZ, high_hz);
    let summary = if patch.asymmetry.abs() >= 0.005 {
        format!(
            "Drive {:.0}% · Asym {:+.0}% · Tone {} · Out {:+.1} dB · Mix {:.0}%",
            patch.drive * 100.0,
            patch.asymmetry * 100.0,
            format_hz(patch.tone_hz),
            patch.output_db,
            patch.mix * 100.0
        )
    } else {
        format!(
            "Drive {:.0}% · Tone {} · Out {:+.1} dB · Mix {:.0}%",
            patch.drive * 100.0,
            format_hz(patch.tone_hz),
            patch.output_db,
            patch.mix * 100.0
        )
    };
    EffectVisualView::Drive {
        mode: patch.mode,
        freq_high_hz: high_hz,
        curve,
        tone_curve_db,
        tone_norm,
        drive: patch.drive,
        asymmetry: patch.asymmetry,
        tone_hz: patch.tone_hz,
        output_db: patch.output_db,
        mix: patch.mix,
        summary,
    }
}

pub fn build_delay_visual(patch: &DelayPatch, bpm: f64) -> EffectVisualView {
    let bpm = bpm.clamp(20.0, 300.0);
    let (min_steps, max_steps) = EffectParam::Time.range();
    let steps = patch.steps.clamp(min_steps, max_steps);
    let feedback = patch.feedback.clamp(0.0, 0.88);
    let mix = patch.mix.clamp(0.0, 1.0);
    let delay_ms = (15_000.0 / bpm * f64::from(steps)) as f32;
    let raw_span = (steps * 6.0).clamp(16.0, 128.0);
    let grid_steps = (raw_span / 4.0).ceil() * 4.0;

    let mut taps = Vec::with_capacity(14);
    taps.push(DelayTapView {
        x_norm: 0.0,
        amplitude: (1.0 - mix).clamp(0.0, 1.0),
        left_channel: true,
        is_dry: true,
    });
    for k in 1..=12_i32 {
        let pos = k as f32 * steps;
        if pos > grid_steps + 1e-4 {
            break;
        }
        let amp = (mix * feedback.powi(k - 1)).clamp(0.0, 1.0);
        if k > 1 && amp < 0.015 {
            break;
        }
        taps.push(DelayTapView {
            x_norm: (pos / grid_steps).clamp(0.0, 1.0),
            amplitude: amp,
            left_channel: patch.mode == DelayMode::Stereo || k % 2 == 1,
            is_dry: false,
        });
    }

    let summary = format!(
        "{steps:.2} steps ({delay_ms:.0} ms) · FB {:.0}% · Mix {:.0}%",
        feedback * 100.0,
        mix * 100.0
    );
    EffectVisualView::Delay {
        mode: patch.mode,
        steps,
        delay_ms,
        feedback,
        mix,
        grid_steps,
        taps,
        summary,
    }
}

pub fn build_chorus_visual(
    patch: &ChorusPatch,
    playing: bool,
    query_time_steps: f64,
    bpm: f64,
) -> EffectVisualView {
    let rate = f64::from(patch.rate_hz).clamp(0.05, 6.0);
    let depth = f64::from(patch.depth).clamp(0.0, 1.0);
    let width = f64::from(patch.width).clamp(0.0, 1.0);
    let mix = patch.mix.clamp(0.0, 1.0);
    let cycles = (rate * 1.5).clamp(0.6, 5.0);
    let phase_cycles = if playing {
        let sec = query_time_steps * 15.0 / bpm.clamp(20.0, 300.0);
        (sec * rate).fract()
    } else {
        0.0
    };
    let amp = depth.max(0.04);

    let mut left_lfo = Vec::with_capacity(CHORUS_CURVE_POINTS);
    let mut right_lfo = Vec::with_capacity(CHORUS_CURVE_POINTS);
    for i in 0..CHORUS_CURVE_POINTS {
        let t = i as f64 / (CHORUS_CURVE_POINTS - 1) as f64;
        let [l, r] = eval_quadrature(cycles * t + phase_cycles, width);
        left_lfo.push((amp * l).clamp(-1.0, 1.0) as f32);
        right_lfo.push((amp * r).clamp(-1.0, 1.0) as f32);
    }
    let phase_deg = (width * 90.0) as f32;
    let summary = format!(
        "{:.2} Hz · Depth {:.0}% · Width {:.0}% · Mix {:.0}%",
        patch.rate_hz,
        patch.depth * 100.0,
        patch.width * 100.0,
        mix * 100.0
    );
    EffectVisualView::Chorus {
        left_lfo,
        right_lfo,
        rate_hz: patch.rate_hz,
        depth: patch.depth,
        width: patch.width,
        mix,
        phase_deg,
        summary,
    }
}

pub fn build_reverb_visual(patch: &ReverbPatch, sr: f64) -> EffectVisualView {
    let sr = sanitize_sample_rate(sr);
    let size = patch.size.clamp(0.0, 1.0);
    let decay = patch.decay_seconds.clamp(0.2, 12.0);
    let damping_hz = patch.damping_hz.clamp(500.0, 18_000.0);
    let predelay_ms = patch.predelay_ms.clamp(0.0, 120.0);
    let width = patch.width.clamp(0.0, 1.0);
    let mix = patch.mix.clamp(0.0, 1.0);

    let predelay_sec = predelay_ms * 0.001;
    let window_sec = (predelay_sec + decay * 1.1).clamp(0.8, 12.5);
    let predelay_norm = (predelay_sec / window_sec).clamp(0.0, 0.30);

    let mut decay_curve = Vec::with_capacity(REVERB_CURVE_POINTS);
    let mut hf_decay_curve = Vec::with_capacity(REVERB_CURVE_POINTS);
    for i in 0..REVERB_CURVE_POINTS {
        let xn = i as f32 / (REVERB_CURVE_POINTS - 1) as f32;
        let t = xn * window_sec;
        if t < predelay_sec {
            decay_curve.push(0.0);
            hf_decay_curve.push(0.0);
        } else {
            let dt = t - predelay_sec;
            decay_curve.push(reverb_rt60_envelope(decay, dt));
            hf_decay_curve.push(reverb_hf_rt60_envelope(decay, damping_hz, dt));
        }
    }

    let high_hz = freq_high_hz(sr);
    let damping_curve_db = sample_tpt_lpf_db(damping_hz, sr);
    let damping_norm = log_to_norm(damping_hz, FREQ_MIN_HZ, high_hz);
    let summary = format!(
        "RT60 {:.2} s · Pre {:.0} ms · Damp {} · Size {:.0}%",
        patch.decay_seconds,
        predelay_ms,
        format_hz(damping_hz),
        size * 100.0
    );
    EffectVisualView::Reverb {
        freq_high_hz: high_hz,
        decay_curve,
        hf_decay_curve,
        damping_curve_db,
        predelay_norm,
        damping_norm,
        window_sec,
        size,
        decay_seconds: patch.decay_seconds,
        damping_hz,
        predelay_ms,
        width,
        mix,
        summary,
    }
}

impl App {
    pub(super) fn build_effect_view(
        &self,
        id: Id,
        cursor: usize,
        snapshot: &AudioSnapshot,
    ) -> EffectDeckView {
        let query_time = self.live_query_time(snapshot);
        let raw_controls = self.control_descriptors_for(Detail::Effect(id));
        let (controls, control_cursor) =
            map_controls(self, &raw_controls, cursor, Some(query_time));

        let Some((owner, device)) = self.effect(id) else {
            return EffectDeckView {
                id,
                name: format!("Effect {id}"),
                kind_label: "EFFECT".into(),
                channel_name: String::new(),
                bypassed: false,
                controls,
                control_cursor,
                visual: EffectVisualView::Missing,
            };
        };

        let project = self.project();
        let channel_name = self.channel_name(owner).to_string();
        let sr = effective_sr(snapshot.sample_rate);

        // Resolve any active Effect parameter modulations or Macro bindings at the current transport step.
        let mut modulated_kind = device.kind;
        let mut any_modulated = false;
        let mut mod_cutoff_hz = None;
        for &param in device.params() {
            if let Some(norm) =
                self.evaluate_live_param_at(ParamAddress::Effect { device: id, param }, query_time)
            {
                param.write_kind(&mut modulated_kind, norm);
                any_modulated = true;
                if param == EffectParam::Cutoff {
                    if let EffectKind::Filter(fp) = modulated_kind {
                        mod_cutoff_hz = Some(fp.cutoff_hz);
                    }
                }
            }
        }

        let visual = match device.kind {
            EffectKind::Eq(patch) => {
                let mod_patch = if any_modulated {
                    match modulated_kind {
                        EffectKind::Eq(p) => Some(p),
                        _ => None,
                    }
                } else {
                    None
                };
                build_eq_visual(&patch, mod_patch.as_ref(), sr)
            }
            EffectKind::Filter(patch) => {
                let mod_patch = if any_modulated {
                    match modulated_kind {
                        EffectKind::Filter(p) => Some(p),
                        _ => None,
                    }
                } else {
                    None
                };
                let eff = mod_patch.unwrap_or(patch);
                let env_visual = if eff.env_amount.abs() > 0.01 {
                    let (compiled, source_label) = match eff.env_pattern_id {
                        Some(pid) => {
                            let c = project.compile_filter_pattern(pid).unwrap_or_else(|| {
                                self.assets.filter_grooves.compiled(eff.env_pattern)
                            });
                            let lbl = project.pattern(pid).map_or_else(
                                || format!("Pattern #{pid}"),
                                |p| format!("Pat: {}", p.name),
                            );
                            (c, lbl)
                        }
                        None => (
                            self.assets.filter_grooves.compiled(eff.env_pattern),
                            self.assets.filter_grooves.preset(eff.env_pattern).name,
                        ),
                    };
                    let (active_step, env_level) =
                        compiled.preview_envelope(query_time, project.bpm, eff.env_decay_ms);
                    let env_curve = RhythmicEnv::preview(
                        &compiled,
                        eff.env_decay_ms,
                        project.bpm,
                        FILTER_ENV_PREVIEW_PPS,
                    );
                    let steps = compiled
                        .active_steps()
                        .iter()
                        .map(|&v| f32::from(v) / 127.0)
                        .collect();
                    Some(FilterEnvVisual {
                        source_label,
                        resolution: compiled.resolution,
                        smooth_attack: compiled.smooth_attack,
                        steps,
                        env_curve,
                        active_step,
                        env_level,
                        env_amount: eff.env_amount,
                        env_decay_ms: eff.env_decay_ms,
                    })
                } else {
                    None
                };
                build_filter_visual(&patch, mod_patch.as_ref(), mod_cutoff_hz, env_visual, sr)
            }
            EffectKind::Compressor(patch) => {
                let effective_patch = match modulated_kind {
                    EffectKind::Compressor(p) if any_modulated => p,
                    _ => patch,
                };
                let (gr_db, gr_hold_db) = if device.bypassed {
                    (0.0, 0.0)
                } else {
                    (
                        snapshot.effect_gain_reduction_db(id),
                        snapshot.effect_gr_hold_db(id),
                    )
                };
                build_compressor_visual(&effective_patch, gr_db, gr_hold_db)
            }
            EffectKind::Limiter(patch) => {
                let effective_patch = match modulated_kind {
                    EffectKind::Limiter(p) if any_modulated => p,
                    _ => patch,
                };
                let (gr_db, gr_hold_db) = if device.bypassed {
                    (0.0, 0.0)
                } else {
                    (
                        snapshot.effect_gain_reduction_db(id),
                        snapshot.effect_gr_hold_db(id),
                    )
                };
                build_limiter_visual(&effective_patch, gr_db, gr_hold_db, any_modulated)
            }
            EffectKind::Drive(patch) => {
                let effective_patch = match modulated_kind {
                    EffectKind::Drive(p) if any_modulated => p,
                    _ => patch,
                };
                build_drive_visual(&effective_patch, sr)
            }
            EffectKind::Delay(patch) => {
                let effective_patch = match modulated_kind {
                    EffectKind::Delay(p) if any_modulated => p,
                    _ => patch,
                };
                build_delay_visual(&effective_patch, project.bpm)
            }
            EffectKind::Chorus(patch) => {
                let effective_patch = match modulated_kind {
                    EffectKind::Chorus(p) if any_modulated => p,
                    _ => patch,
                };
                build_chorus_visual(&effective_patch, snapshot.playing, query_time, project.bpm)
            }
            EffectKind::Reverb(patch) => {
                let effective_patch = match modulated_kind {
                    EffectKind::Reverb(p) if any_modulated => p,
                    _ => patch,
                };
                build_reverb_visual(&effective_patch, sr)
            }
        };

        EffectDeckView {
            id,
            name: device.name.clone(),
            kind_label: device.kind.label().to_ascii_uppercase(),
            channel_name,
            bypassed: device.bypassed,
            controls,
            control_cursor,
            visual,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::DeckView;
    use cadence_core::filter_patterns::CompiledFilterPattern;
    use cadence_core::material::{
        Modulation, ModulationTarget, Pattern, PatternKind, Project, ValuePoint, ValueUnit,
    };
    use cadence_core::mixer::{ChannelRef, EffectDevice};
    use cadence_core::params::ValueMode;
    use cadence_core::rack::FilterMode;

    #[test]
    fn all_eight_effect_visual_builders_produce_finite_bounded_curves() {
        let sr = 48_000.0;

        // 1. EQ (Parametric mid_hz + mid_q)
        let eq = build_eq_visual(
            &EqPatch {
                hpf_hz: 120.0,
                low_db: 4.5,
                mid_db: -3.0,
                mid_hz: 2400.0,
                mid_q: 3.2,
                high_db: 6.0,
            },
            None,
            sr,
        );
        match eq {
            EffectVisualView::Eq {
                curve_db,
                hpf_active,
                hpf_norm,
                mid_hz_norm,
                mid_q_span,
                ..
            } => {
                assert_eq!(curve_db.len(), EQ_CURVE_POINTS);
                assert!(hpf_active);
                assert!((0.0..=1.0).contains(&hpf_norm));
                assert!(mid_hz_norm > 0.6 && mid_hz_norm < 0.85);
                assert!(mid_q_span.0 < mid_hz_norm && mid_hz_norm < mid_q_span.1);
                assert!(curve_db
                    .iter()
                    .all(|v| v.is_finite() && (-18.0..=18.0).contains(v)));
                // HPF at 120 Hz pulls 20 Hz far down while high shelf lifts 20 kHz.
                assert!(curve_db[0] < -10.0, "HPF must attenuate 20 Hz");
                assert!(curve_db[EQ_CURVE_POINTS - 1] > 3.0, "High shelf must boost");
            }
            other => panic!("expected Eq visual, got {other:?}"),
        }

        // 2. Filter (Rhythmic Envelope / PCF)
        let filt_base = FilterPatch {
            mode: FilterMode::AcidLowPass,
            cutoff_hz: 600.0,
            resonance: 0.72,
            drive: 0.35,
            mix: 1.0,
            env_pattern: 6,
            env_pattern_id: None,
            env_amount: 2.5,
            env_decay_ms: 240.0,
        };
        let filt_mod = FilterPatch {
            cutoff_hz: 4200.0,
            ..filt_base
        };
        let filt = build_filter_visual(&filt_base, Some(&filt_mod), Some(4200.0), None, sr);
        match filt {
            EffectVisualView::Filter {
                curve_db,
                mod_curve_db,
                cutoff_norm,
                mod_cutoff_norm,
                ..
            } => {
                assert_eq!(curve_db.len(), FILTER_CURVE_POINTS);
                assert!(curve_db
                    .iter()
                    .all(|v| v.is_finite() && (-48.0..=18.0).contains(v)));
                let mc = mod_curve_db.expect("modulated curve");
                assert_eq!(mc.len(), FILTER_CURVE_POINTS);
                assert!(mc
                    .iter()
                    .all(|v| v.is_finite() && (-48.0..=18.0).contains(v)));
                assert!(mod_cutoff_norm.unwrap() > cutoff_norm);
            }
            other => panic!("expected Filter visual, got {other:?}"),
        }

        // 3. Compressor
        let comp = build_compressor_visual(
            &CompressorPatch {
                threshold_db: -18.0,
                ratio: 6.0,
                attack_ms: 5.0,
                release_ms: 80.0,
                makeup_db: 4.0,
                mix: 1.0,
            },
            7.2,
            9.5,
        );
        match comp {
            EffectVisualView::Compressor {
                transfer_curve,
                compression_curve,
                threshold_norm,
                gain_reduction_db,
                gr_hold_db,
                ..
            } => {
                assert_eq!(transfer_curve.len(), COMP_CURVE_POINTS);
                assert_eq!(compression_curve.len(), COMP_CURVE_POINTS);
                assert!((0.0..=1.0).contains(&threshold_norm));
                assert!((gain_reduction_db - 7.2).abs() < 1e-4);
                assert!((gr_hold_db - 9.5).abs() < 1e-4);
                assert!(transfer_curve.iter().all(|(x, y)| x.is_finite()
                    && y.is_finite()
                    && (0.0..=1.0).contains(x)
                    && (0.0..=1.0).contains(y)));
                // Above threshold without makeup, output must stay below 1:1 unity diagonal.
                let (x_hi, y_hi) = compression_curve[COMP_CURVE_POINTS - 1];
                assert!(
                    y_hi < x_hi - 0.15,
                    "compression curve must bend below unity"
                );
            }
            other => panic!("expected Compressor visual, got {other:?}"),
        }

        // 4. Drive
        let drv = build_drive_visual(
            &DrivePatch {
                mode: DriveMode::Tape,
                drive: 0.78,
                asymmetry: 0.0,
                tone_hz: 4500.0,
                output_db: -1.5,
                mix: 1.0,
            },
            sr,
        );
        match drv {
            EffectVisualView::Drive {
                mode,
                curve,
                tone_curve_db,
                tone_norm,
                ..
            } => {
                assert_eq!(mode, DriveMode::Tape);
                assert_eq!(curve.len(), DRIVE_CURVE_POINTS);
                assert_eq!(tone_curve_db.len(), TONE_CURVE_POINTS);
                assert!((0.0..=1.0).contains(&tone_norm));
                assert!(curve.iter().all(|(x, y)| x.is_finite()
                    && y.is_finite()
                    && (-1.0..=1.0).contains(x)
                    && (-1.25..=1.25).contains(y)));
                assert!(tone_curve_db
                    .iter()
                    .all(|v| v.is_finite() && (-24.0..=3.0).contains(v)));
            }
            other => panic!("expected Drive visual, got {other:?}"),
        }

        // 4b. Drive modes (HardClip & Fold) produce distinct transfer curves matching SPOT!
        let hard = build_drive_visual(
            &DrivePatch {
                mode: DriveMode::HardClip,
                drive: 0.90,
                asymmetry: 0.0,
                tone_hz: 8000.0,
                output_db: 0.0,
                mix: 1.0,
            },
            sr,
        );
        let fold = build_drive_visual(
            &DrivePatch {
                mode: DriveMode::Fold,
                drive: 0.75,
                asymmetry: 0.0,
                tone_hz: 8000.0,
                output_db: 0.0,
                mix: 1.0,
            },
            sr,
        );
        match (hard, fold) {
            (
                EffectVisualView::Drive {
                    mode: m_h,
                    curve: c_h,
                    ..
                },
                EffectVisualView::Drive {
                    mode: m_f,
                    curve: c_f,
                    ..
                },
            ) => {
                assert_eq!(m_h, DriveMode::HardClip);
                assert_eq!(m_f, DriveMode::Fold);
                assert_ne!(c_h, c_f);
            }
            other => panic!("expected HardClip & Fold visuals, got {other:?}"),
        }

        // 5. Delay (PingPong & Stereo)
        let dly = build_delay_visual(
            &DelayPatch {
                mode: DelayMode::PingPong,
                steps: 3.0,
                feedback: 0.55,
                mix: 0.45,
            },
            128.0,
        );
        match dly {
            EffectVisualView::Delay {
                mode,
                taps,
                delay_ms,
                ..
            } => {
                assert_eq!(mode, DelayMode::PingPong);
                assert!(delay_ms.is_finite() && delay_ms > 200.0);
                assert!(taps.len() >= 4);
                assert!(taps[0].is_dry);
                assert!(taps.iter().all(|t| t.x_norm.is_finite()
                    && t.amplitude.is_finite()
                    && (0.0..=1.0).contains(&t.x_norm)
                    && (0.0..=1.0).contains(&t.amplitude)));
                // Echo taps decay monotonically and alternate L/R in PingPong.
                assert!(taps[1].amplitude > taps[2].amplitude);
                assert_ne!(taps[1].left_channel, taps[2].left_channel);
            }
            other => panic!("expected Delay visual, got {other:?}"),
        }
        let dly_st = build_delay_visual(
            &DelayPatch {
                mode: DelayMode::Stereo,
                steps: 3.0,
                feedback: 0.55,
                mix: 0.45,
            },
            128.0,
        );
        match dly_st {
            EffectVisualView::Delay { mode, taps, .. } => {
                assert_eq!(mode, DelayMode::Stereo);
                assert!(taps.len() >= 4);
                assert!(taps.iter().all(|t| t.left_channel));
            }
            other => panic!("expected Stereo Delay visual, got {other:?}"),
        }

        // 6. Chorus
        let chr = build_chorus_visual(
            &ChorusPatch {
                rate_hz: 1.1,
                depth: 0.65,
                width: 0.9,
                mix: 0.5,
            },
            true,
            12.5,
            124.0,
        );
        match chr {
            EffectVisualView::Chorus {
                left_lfo,
                right_lfo,
                phase_deg,
                ..
            } => {
                assert_eq!(left_lfo.len(), CHORUS_CURVE_POINTS);
                assert_eq!(right_lfo.len(), CHORUS_CURVE_POINTS);
                assert!((phase_deg - 81.0).abs() < 0.5);
                assert!(left_lfo
                    .iter()
                    .chain(right_lfo.iter())
                    .all(|v| v.is_finite() && (-1.0..=1.0).contains(v)));
            }
            other => panic!("expected Chorus visual, got {other:?}"),
        }

        // 7. Reverb
        let rvb = build_reverb_visual(
            &ReverbPatch {
                size: 0.72,
                decay_seconds: 3.6,
                damping_hz: 4200.0,
                predelay_ms: 42.0,
                mix: 0.35,
                width: 0.85,
            },
            sr,
        );
        match rvb {
            EffectVisualView::Reverb {
                decay_curve,
                hf_decay_curve,
                damping_curve_db,
                predelay_norm,
                ..
            } => {
                assert_eq!(decay_curve.len(), REVERB_CURVE_POINTS);
                assert_eq!(hf_decay_curve.len(), REVERB_CURVE_POINTS);
                assert_eq!(damping_curve_db.len(), TONE_CURVE_POINTS);
                assert!(predelay_norm > 0.0 && predelay_norm <= 0.30);
                assert!(decay_curve
                    .iter()
                    .chain(hf_decay_curve.iter())
                    .all(|v| v.is_finite() && (0.0..=1.0).contains(v)));
                // Predelay starts at 0.0 and tail decays smoothly toward end.
                assert_eq!(decay_curve[0], 0.0);
                assert!(decay_curve[REVERB_CURVE_POINTS - 1] < 0.15);
            }
            other => panic!("expected Reverb visual, got {other:?}"),
        }

        // 8. Limiter
        let lim = build_limiter_visual(
            &LimiterPatch {
                ceiling_db: -1.5,
                input_drive_db: 9.0,
                release_ms: 65.0,
                mix: 1.0,
            },
            5.4,
            7.8,
            false,
        );
        match lim {
            EffectVisualView::Limiter {
                transfer_curve,
                ceiling_norm,
                gain_reduction_db,
                gr_hold_db,
                ..
            } => {
                assert_eq!(transfer_curve.len(), LIMITER_CURVE_POINTS);
                assert!((gain_reduction_db - 5.4).abs() < 1e-4);
                assert!((gr_hold_db - 7.8).abs() < 1e-4);
                assert!(transfer_curve.iter().all(|(x, y)| x.is_finite()
                    && y.is_finite()
                    && (0.0..=1.0).contains(x)
                    && *y <= ceiling_norm + 1e-4));
            }
            other => panic!("expected Limiter visual, got {other:?}"),
        }
    }

    #[test]
    fn frame_view_emits_effect_deck_view_with_live_gr_and_pcf_sweep() {
        let mut project = Project::demo();
        let next_id = project
            .mixer
            .channels
            .iter()
            .flat_map(|c| c.inserts.iter())
            .chain(project.mixer.master.inserts.iter())
            .map(|d| d.id)
            .max()
            .unwrap_or(0)
            + 1;
        let comp_id = next_id;
        let filt_id = next_id + 1;
        project
            .mixer
            .master
            .inserts
            .push(EffectDevice::compressor(comp_id));
        project
            .mixer
            .master
            .inserts
            .push(EffectDevice::filter(filt_id));

        let pat_id = project.patterns.iter().map(|p| p.id).max().unwrap_or(0) + 1;
        project.patterns.push(Pattern {
            id: pat_id,
            name: "PCF Sweep".into(),
            kind: PatternKind::Values {
                unit: ValueUnit::Normalized,
                period: 16,
                mode: ValueMode::Ramp,
                points: vec![
                    ValuePoint {
                        step: 0,
                        value: 0.15,
                    },
                    ValuePoint {
                        step: 8,
                        value: 0.90,
                    },
                ],
            },
        });
        let mod_id = project.modulations.iter().map(|m| m.id).max().unwrap_or(0) + 1;
        project.modulations.push(Modulation {
            id: mod_id,
            name: "PCF Cut".into(),
            pattern: pat_id,
            target: ModulationTarget::Effect {
                device: filt_id,
                parameter: EffectParam::Cutoff,
            },
        });

        let mut app = App::new(project, "demo.json".into()).unwrap();
        let mut snap = AudioSnapshot {
            playing: true,
            absolute_step: 8,
            step_fraction: 0.0,
            sample_rate: 48_000,
            ..AudioSnapshot::default()
        };
        snap.set_effect_meter(comp_id, 6.4, 8.9);

        app.execute_command(&format!("fx {comp_id}")).unwrap();
        let fv_comp = app.frame_view(&snap, "silent");
        let comp_deck = fv_comp
            .decks
            .iter()
            .find_map(|d| match &d.view {
                DeckView::Effect(v) => Some(v),
                _ => None,
            })
            .expect("Compressor EffectDeckView");
        assert_eq!(comp_deck.channel_name, app.channel_name(ChannelRef::Master));
        assert!(!comp_deck.bypassed);
        match &comp_deck.visual {
            EffectVisualView::Compressor {
                gain_reduction_db,
                gr_hold_db,
                ..
            } => {
                assert!((*gain_reduction_db - 6.4).abs() < 1e-4);
                assert!((*gr_hold_db - 8.9).abs() < 1e-4);
            }
            other => panic!("expected Compressor visual, got {other:?}"),
        }

        app.execute_command(&format!("fx {filt_id}")).unwrap();
        let fv_filt = app.frame_view(&snap, "silent");
        let filt_deck = fv_filt
            .decks
            .iter()
            .find_map(|d| match &d.view {
                DeckView::Effect(v) => Some(v),
                _ => None,
            })
            .expect("Filter EffectDeckView");
        match &filt_deck.visual {
            EffectVisualView::Filter {
                mod_cutoff_norm,
                mod_cutoff_hz,
                mod_curve_db,
                ..
            } => {
                assert!(mod_cutoff_norm.is_some());
                assert!(mod_cutoff_hz.unwrap() > 8000.0);
                assert!(mod_curve_db.is_some());
            }
            other => panic!("expected Filter visual, got {other:?}"),
        }
    }

    #[test]
    fn effect_visual_curves_have_zero_drift_parity_with_core_dsp() {
        // 1. EQ parity across 44.1 / 48 / 96 kHz and exact freq_high_hz axis scaling.
        let eq_patch = EqPatch {
            hpf_hz: 95.0,
            low_db: 3.5,
            mid_db: -4.5,
            mid_hz: 1850.0,
            mid_q: 2.4,
            high_db: 4.0,
        };
        for (sr, expected_hi) in [
            (44_100.0_f64, 19_845.0_f32),
            (48_000.0, 20_000.0),
            (96_000.0, 20_000.0),
            (24_000.0, 10_800.0),
        ] {
            match build_eq_visual(&eq_patch, None, sr) {
                EffectVisualView::Eq {
                    freq_high_hz,
                    curve_db,
                    ..
                } => {
                    assert!(
                        (freq_high_hz - expected_hi).abs() < 1e-2,
                        "sr={sr}: expected freq_high_hz={expected_hi}, got {freq_high_hz}"
                    );
                    for (i, &db) in curve_db.iter().enumerate() {
                        let x = i as f32 / (EQ_CURVE_POINTS - 1) as f32;
                        let f = norm_to_log(x, FREQ_MIN_HZ, freq_high_hz);
                        let expected =
                            eq_response_db(&eq_patch, f64::from(f), sr).clamp(-18.0, 18.0) as f32;
                        assert!(
                            (db - expected).abs() < 1e-4,
                            "EQ drift at sr={sr} i={i}: view={db} vs dsp={expected}"
                        );
                    }
                }
                other => panic!("expected Eq visual, got {other:?}"),
            }
        }

        // 2. Insert Filter parity with parallel mix < 1.0, AcidLowPass k = 3.85, and sr * 0.45 cutoff clamp.
        let sr = 48_000.0;
        let dry_filter = FilterPatch {
            mode: FilterMode::LowPass,
            cutoff_hz: 400.0,
            resonance: 0.65,
            drive: 0.0,
            mix: 0.0,
            ..FilterPatch::default()
        };
        match build_filter_visual(&dry_filter, None, None, None, sr) {
            EffectVisualView::Filter { curve_db, .. } => {
                assert!(
                    curve_db.iter().all(|&db| db.abs() < 1e-4),
                    "Filter with mix=0.0 must be flat 0 dB"
                );
            }
            other => panic!("expected Filter visual, got {other:?}"),
        }

        let parallel_acid = FilterPatch {
            mode: FilterMode::AcidLowPass,
            cutoff_hz: 900.0,
            resonance: 0.80,
            drive: 0.25,
            mix: 0.5,
            ..FilterPatch::default()
        };
        match build_filter_visual(&parallel_acid, None, None, None, 44_100.0) {
            EffectVisualView::Filter {
                freq_high_hz,
                curve_db,
                ..
            } => {
                assert!((freq_high_hz - 19_845.0).abs() < 1e-2);
                for (i, &db) in curve_db.iter().enumerate() {
                    let x = i as f32 / (FILTER_CURVE_POINTS - 1) as f32;
                    let f = norm_to_log(x, FREQ_MIN_HZ, freq_high_hz);
                    let expected = filter_mix_magnitude_db(
                        parallel_acid.mode,
                        parallel_acid.cutoff_hz,
                        parallel_acid.resonance,
                        parallel_acid.mix,
                        f64::from(f),
                        44_100.0,
                    )
                    .clamp(-48.0, 18.0) as f32;
                    assert!(
                        (db - expected).abs() < 1e-4,
                        "Filter drift at i={i}: view={db} vs dsp={expected}"
                    );
                }
            }
            other => panic!("expected Filter visual, got {other:?}"),
        }

        // Filter envelope modulation clamps to sr * 0.45.
        let compiled = CompiledFilterPattern::from_preset(12);
        let env_vis = FilterEnvVisual {
            source_label: "12 Velvet Arch".into(),
            resolution: compiled.resolution,
            smooth_attack: compiled.smooth_attack,
            steps: compiled
                .active_steps()
                .iter()
                .map(|&v| f32::from(v) / 127.0)
                .collect(),
            env_curve: RhythmicEnv::preview(&compiled, 200.0, 120.0, FILTER_ENV_PREVIEW_PPS),
            active_step: 0,
            env_level: 1.0,
            env_amount: 3.0,
            env_decay_ms: 200.0,
        };
        let high_filt = FilterPatch {
            cutoff_hz: 8_000.0,
            ..FilterPatch::default()
        };
        match build_filter_visual(&high_filt, None, None, Some(env_vis), 48_000.0) {
            EffectVisualView::Filter { mod_cutoff_hz, .. } => {
                let swept = mod_cutoff_hz.expect("swept cutoff");
                assert!(
                    (swept - 21_600.0).abs() < 1e-2,
                    "expected 48k * 0.45 = 21600 Hz clamp, got {swept}"
                );
            }
            other => panic!("expected Filter visual, got {other:?}"),
        }

        // 3. Compressor: signed makeup formatting and -48..+6 dB headroom range.
        let neg_comp = CompressorPatch {
            threshold_db: -18.0,
            ratio: 4.0,
            attack_ms: 5.0,
            release_ms: 80.0,
            makeup_db: -3.0,
            mix: 1.0,
        };
        match build_compressor_visual(&neg_comp, 2.0, 2.0) {
            EffectVisualView::Compressor {
                summary,
                transfer_curve,
                ..
            } => {
                assert!(
                    summary.contains("-3.0 dB") && !summary.contains("+-"),
                    "Compressor summary must format negative makeup cleanly: {summary}"
                );
                let min_db = f64::from(COMP_TRANSFER_MIN_DB);
                let max_db = f64::from(COMP_TRANSFER_MAX_DB);
                let span = max_db - min_db;
                for &(xn, yn) in &transfer_curve {
                    let in_db = min_db + f64::from(xn) * span;
                    let gr = soft_knee_gr_db_f64(in_db, -18.0, 4.0, f64::from(COMPRESSOR_KNEE_DB));
                    let expected_db = (in_db - gr - 3.0).clamp(min_db, max_db);
                    let expected_y = ((expected_db - min_db) / span) as f32;
                    assert!((yn - expected_y).abs() < 1e-4);
                }
            }
            other => panic!("expected Compressor visual, got {other:?}"),
        }

        // Positive makeup lift above 0 dB does not flatline at 0 dB thanks to +6 dB headroom!
        let hot_comp = CompressorPatch {
            threshold_db: -12.0,
            ratio: 2.0,
            attack_ms: 5.0,
            release_ms: 80.0,
            makeup_db: 6.0,
            mix: 1.0,
        };
        match build_compressor_visual(&hot_comp, 0.0, 0.0) {
            EffectVisualView::Compressor { transfer_curve, .. } => {
                let zero_norm =
                    (0.0 - COMP_TRANSFER_MIN_DB) / (COMP_TRANSFER_MAX_DB - COMP_TRANSFER_MIN_DB);
                let (_, y_top) = *transfer_curve.last().unwrap();
                // At in_db = +6 dB: gr = 9 dB, out_db = 6 - 9 + 6 = +3 dB > 0 dB!
                assert!(
                    y_top > zero_norm + 0.04,
                    "Compressor output (+3 dB) must render above 0 dB reference ({zero_norm:.3}), got {y_top:.3}"
                );
            }
            other => panic!("expected Compressor visual, got {other:?}"),
        }

        // 4. Limiter parity with soft_knee_gr_db_f64(..., f64::INFINITY, LIMITER_KNEE_DB).
        let lim_patch = LimiterPatch {
            ceiling_db: -2.0,
            input_drive_db: 8.0,
            release_ms: 60.0,
            mix: 1.0,
        };
        match build_limiter_visual(&lim_patch, 3.0, 3.0, false) {
            EffectVisualView::Limiter { transfer_curve, .. } => {
                for &(xn, yn) in &transfer_curve {
                    let in_db = -48.0 + f64::from(xn) * 48.0;
                    let driven = in_db + 8.0;
                    let gr = soft_knee_gr_db_f64(
                        driven,
                        -2.0,
                        f64::INFINITY,
                        f64::from(LIMITER_KNEE_DB),
                    );
                    let expected_db = (driven - gr).min(-2.0).clamp(-48.0, 0.0);
                    let expected_y = ((expected_db + 48.0) / 48.0) as f32;
                    assert!((yn - expected_y).abs() < 1e-4);
                }
            }
            other => panic!("expected Limiter visual, got {other:?}"),
        }

        // 5. Drive zero-pop identity at drive == 0.0 across ALL modes & exact TptOnePoleLpf tone curve parity.
        for mode in DriveMode::ALL {
            let zero_drive = DrivePatch {
                mode,
                drive: 0.0,
                asymmetry: 0.0,
                tone_hz: 6_000.0,
                output_db: 0.0,
                mix: 1.0,
            };
            match build_drive_visual(&zero_drive, sr) {
                EffectVisualView::Drive {
                    mode: vis_mode,
                    freq_high_hz,
                    curve,
                    tone_curve_db,
                    ..
                } => {
                    assert_eq!(vis_mode, mode);
                    for &(x, y) in &curve {
                        assert!(
                            (y - x).abs() < 1e-6,
                            "Drive {mode:?} at drive=0.0 must be exact identity y==x, got x={x} y={y}"
                        );
                    }
                    for (i, &db) in tone_curve_db.iter().enumerate() {
                        let xn = i as f32 / (TONE_CURVE_POINTS - 1) as f32;
                        let f = norm_to_log(xn, FREQ_MIN_HZ, freq_high_hz);
                        let expected = TptOnePoleLpf::magnitude_db(6_000.0, f64::from(f), sr)
                            .clamp(-24.0, 3.0) as f32;
                        assert!((db - expected).abs() < 1e-4);
                    }
                }
                other => panic!("expected Drive visual, got {other:?}"),
            }
        }

        // 6. Reverb exact -60 dB RT60 envelope parity with core DSP.
        let rev_patch = ReverbPatch {
            size: 0.6,
            decay_seconds: 2.0,
            damping_hz: 5_000.0,
            predelay_ms: 0.0,
            mix: 0.5,
            width: 0.0,
        };
        match build_reverb_visual(&rev_patch, sr) {
            EffectVisualView::Reverb {
                decay_curve,
                hf_decay_curve,
                window_sec,
                ..
            } => {
                assert_eq!(decay_curve[0], 1.0);
                // At t == decay_seconds, linear amplitude reaches ~0.001 (-60 dB).
                let idx_rt60 = ((rev_patch.decay_seconds / window_sec)
                    * (REVERB_CURVE_POINTS - 1) as f32)
                    .round() as usize;
                let t_sample = (idx_rt60 as f32 / (REVERB_CURVE_POINTS - 1) as f32) * window_sec;
                let expected_full = reverb_rt60_envelope(rev_patch.decay_seconds, t_sample);
                let expected_hf = reverb_hf_rt60_envelope(
                    rev_patch.decay_seconds,
                    rev_patch.damping_hz,
                    t_sample,
                );
                assert!((decay_curve[idx_rt60] - expected_full).abs() < 1e-6);
                assert!((hf_decay_curve[idx_rt60] - expected_hf).abs() < 1e-6);
                assert!(
                    decay_curve[idx_rt60] < 0.003,
                    "Reverb decay near t=RT60 must hit -60 dB (~0.001), got {}",
                    decay_curve[idx_rt60]
                );
            }
            other => panic!("expected Reverb visual, got {other:?}"),
        }

        // 7. Drive asymmetry bends transfer curve asymmetrically & shows in summary.
        let asym_drv = build_drive_visual(
            &DrivePatch {
                mode: DriveMode::HardClip,
                drive: 0.55,
                asymmetry: 0.50,
                tone_hz: 10_000.0,
                output_db: 0.0,
                mix: 1.0,
            },
            sr,
        );
        match asym_drv {
            EffectVisualView::Drive {
                asymmetry,
                curve,
                summary,
                ..
            } => {
                assert!((asymmetry - 0.50).abs() < 1e-6);
                assert!(summary.contains("Asym +50%"), "summary={summary}");
                let y_neg = curve.first().unwrap().1;
                let y_pos = curve.last().unwrap().1;
                assert!(
                    (y_pos.abs() - y_neg.abs()).abs() > 0.05,
                    "asymmetry=+0.50 must produce distinct +1/-1 clip ceilings: y_neg={y_neg} y_pos={y_pos}"
                );
            }
            other => panic!("expected Drive visual, got {other:?}"),
        }

        // 8. FilterMode::LowPass6 curve matches exact 1-pole -6 dB/oct response.
        let lp6_vis = build_filter_visual(
            &FilterPatch {
                mode: FilterMode::LowPass6,
                cutoff_hz: 1_000.0,
                resonance: 0.5,
                drive: 0.0,
                mix: 1.0,
                ..FilterPatch::default()
            },
            None,
            None,
            None,
            sr,
        );
        match lp6_vis {
            EffectVisualView::Filter {
                mode_label,
                curve_db,
                freq_high_hz,
                ..
            } => {
                assert_eq!(mode_label, "LP 6dB");
                let last_db = *curve_db.last().unwrap();
                let expected =
                    TptOnePoleLpf::magnitude_db(1_000.0, f64::from(freq_high_hz), sr) as f32;
                assert!(
                    (last_db - expected).abs() < 1e-3,
                    "LowPass6 curve_db tail {last_db} != expected 1-pole {expected}"
                );
            }
            other => panic!("expected Filter visual, got {other:?}"),
        }
    }

    #[test]
    fn effect_view_projects_live_modulated_values_and_links_rows() {
        use crate::view::DeckView;
        use cadence_core::macros::{MacroBinding, MacroControl};
        use cadence_core::material::{
            Modulation, ModulationTarget, Pattern, PatternKind, Project, ValuePoint, ValueUnit,
        };
        use cadence_core::mixer::{EffectDevice, EffectKind, FilterPatch};
        use cadence_core::params::ValueMode;

        let mut project = Project::demo();
        let fx_id = project.max_id() + 1;
        project.mixer.master.inserts.push(EffectDevice {
            id: fx_id,
            name: "AcidFilter".into(),
            bypassed: false,
            kind: EffectKind::Filter(FilterPatch {
                cutoff_hz: 1_000.0,
                resonance: 0.2,
                ..FilterPatch::default()
            }),
        });
        let pat_id = project.max_id() + 2;
        project.patterns.push(Pattern {
            id: pat_id,
            name: "CutoffSweep".into(),
            kind: PatternKind::Values {
                unit: ValueUnit::Normalized,
                period: 16,
                mode: ValueMode::Hold,
                points: vec![ValuePoint {
                    step: 0,
                    value: 0.9,
                }],
            },
        });
        let mod_id = project.max_id() + 3;
        project.modulations.push(Modulation {
            id: mod_id,
            name: "SweepMod".into(),
            pattern: pat_id,
            target: ModulationTarget::Effect {
                device: fx_id,
                parameter: EffectParam::Cutoff,
            },
        });
        project.macros.push(MacroControl {
            index: 2,
            name: "Resonator".into(),
            value: 0.85,
            bindings: vec![MacroBinding::new(
                ParamAddress::Effect {
                    device: fx_id,
                    param: EffectParam::Resonance,
                },
                0.0,
                1.0,
            )],
        });

        let mut app = App::new(project, "fx-live.json".into()).unwrap();
        app.open_effect(fx_id).unwrap();

        let snap = AudioSnapshot {
            playing: true,
            absolute_step: 2,
            step_fraction: 0.0,
            sample_rate: 48_000,
            ..AudioSnapshot::default()
        };
        let fv = app.frame_view(&snap, "silent");
        let fx_view = fv
            .decks
            .iter()
            .find_map(|d| match &d.view {
                DeckView::Effect(v) if v.id == fx_id => Some(v),
                _ => None,
            })
            .expect("EffectDeckView");

        let cutoff = fx_view
            .controls
            .iter()
            .find(|c| c.label == "Cutoff")
            .expect("Cutoff row");
        assert!(cutoff.controlled, "modulated Cutoff must mark controlled");
        assert!(
            cutoff.display.contains('→'),
            "modulated Cutoff must render 'base → live', got: {}",
            cutoff.display
        );
        assert!((cutoff.normalized.unwrap() - 0.9).abs() < 1e-3);

        let res = fx_view
            .controls
            .iter()
            .find(|c| c.label == "Resonance")
            .expect("Resonance row");
        assert!(res.controlled, "Macro-bound Resonance must mark controlled");
        assert!(
            res.display.contains('→'),
            "Macro-bound Resonance must render 'base → live', got: {}",
            res.display
        );
        assert!((res.normalized.unwrap() - 0.85).abs() < 1e-3);

        let links: Vec<_> = fx_view
            .controls
            .iter()
            .filter(|c| c.module == "LINKS")
            .collect();
        assert_eq!(links.len(), 2, "expected Modulation + Macro LINKS rows");
        assert!(links.iter().any(|l| l.display == "CutoffSweep"));
        assert!(links
            .iter()
            .any(|l| l.label.contains("M3 Resonator") && l.label.contains("Resonance")));

        // Pressing Enter on the Macro link jumps straight to gk on Macro slot M3 (index 2) binding 0.
        let macro_row = app
            .control_descriptors()
            .iter()
            .position(|c| {
                matches!(
                    c.target,
                    crate::ControlTarget::MacroBinding {
                        index: 2,
                        binding: 0
                    }
                )
            })
            .expect("MacroBinding LINKS control");
        app.control_cursor = macro_row;
        app.press_keys("<Enter>").unwrap();
        assert!(app.macros_focused());
        assert_eq!(app.view_state.macro_cursor, 2);
        assert_eq!(app.view_state.macro_binding_cursor, Some(0));
    }
}
