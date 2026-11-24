//! Single Point of Truth for ReBirth RB-338 device tracks, controller IDs, validation ranges,
//! and declarative static/automation control tables across TB-303, TR-808, TR-909, Mixer, and FX.

use super::calibration::{
    rbs_303_accent, rbs_303_cutoff_hz, rbs_303_decay_ms, rbs_303_decay_ms_for_mode,
    rbs_303_env_mod, rbs_303_resonance, rbs_303_tune, rbs_303_waveform_f32, rbs_808_bd_cutoff_hz,
    rbs_808_bd_decay_ms, rbs_808_cy_cutoff_hz, rbs_808_cy_decay_ms, rbs_808_oh_decay_ms,
    rbs_808_sd_cutoff_hz, rbs_808_sd_snappy_noise, rbs_808_tom_tune, rbs_909_bd_attack_noise,
    rbs_909_bd_decay_ms, rbs_909_bd_tune, rbs_909_ch_decay_ms, rbs_909_oh_decay_ms,
    rbs_909_pitch_tune, rbs_909_sd_cutoff_hz, rbs_909_sd_snappy_noise, rbs_909_sd_tune,
    rbs_909_tom_decay_ms, rbs_comp_ratio, rbs_comp_threshold_db, rbs_pcf_cutoff_hz,
    rbs_pcf_decay_ms, rbs_pcf_env_amount, rbs_pcf_env_pattern_f32, rbs_pcf_filter_mode_f32,
    rbs_pcf_resonance, rbs_presence_cutoff_hz, rbs_sample_decay_ms, rbs_zone_level_db,
    TR808_CY_SYNTH_DECAY_SCALE, TR808_OH_SYNTH_DECAY_SCALE,
};
use cadence_core::mixer::EffectParam;
use cadence_core::params::{InstrumentParam, ValueMode, ZoneParam};
use cadence_core::rack::{
    drum_midi, normalize_cable_amount, InPort, ModuleKindTag, ModuleParam, RackPatch, RackZone,
};

// ---------------------------------------------------------------------------
// Canonical ReBirth automation track indices (`RbsSong::tracks[0..9]`)
// ---------------------------------------------------------------------------

pub(super) const TRACK_MIXER: usize = 0;
pub(super) const TRACK_303_A: usize = 1;
pub(super) const TRACK_808: usize = 3;
pub(super) const TRACK_909: usize = 4;
pub(super) const TRACK_DELAY: usize = 5;
pub(super) const TRACK_DIST: usize = 6;
pub(super) const TRACK_PCF: usize = 7;
pub(super) const TRACK_COMP: usize = 8;

// ---------------------------------------------------------------------------
// Mixer Track 0 controller IDs
// ---------------------------------------------------------------------------

pub(super) const MIXER_CTRL_MASTER_LEVEL: u8 = 0;
pub(super) const MIXER_CTRL_COMP_TARGET: u8 = 1;
pub(super) const MIXER_CTRL_PCF_TARGET: u8 = 2;

#[inline]
pub(super) const fn mixer_ch_enable_ctrl(ch: usize) -> u8 {
    5 + 6 * (ch as u8)
}

#[inline]
pub(super) const fn mixer_ch_level_ctrl(ch: usize) -> u8 {
    6 + 6 * (ch as u8)
}

#[inline]
pub(super) const fn mixer_ch_pan_ctrl(ch: usize) -> u8 {
    7 + 6 * (ch as u8)
}

#[inline]
pub(super) const fn mixer_ch_send_ctrl(ch: usize) -> u8 {
    8 + 6 * (ch as u8)
}

#[inline]
pub(super) const fn mixer_ch_dist_ctrl(ch: usize) -> u8 {
    9 + 6 * (ch as u8)
}

#[inline]
pub(super) const fn is_mixer_ch_enable_ctrl(ctrl: u8) -> bool {
    matches!(ctrl, 5 | 11 | 17 | 23)
}

// ---------------------------------------------------------------------------
// Device-level control indices (pattern, enable, accent, flam, FX)
// ---------------------------------------------------------------------------

pub(super) const DEV_CTRL_ENABLE: u8 = 0;
pub(super) const DEV_CTRL_PATTERN: u8 = 1;
pub(super) const DRUM_CTRL_ACCENT: u8 = 2;
pub(super) const TR909_CTRL_FLAM: u8 = 29;

pub(super) const DELAY_CTRL_ENABLE: u8 = 0;
pub(super) const DELAY_CTRL_STEPS: u8 = 1;
pub(super) const DELAY_CTRL_MODE: u8 = 2;
pub(super) const DELAY_CTRL_FEEDBACK: u8 = 3;
pub(super) const DELAY_CTRL_PAN: u8 = 4;

pub(super) const DIST_CTRL_ENABLE: u8 = 0;
pub(super) const DIST_CTRL_AMOUNT: u8 = 1;
pub(super) const DIST_CTRL_SHAPE: u8 = 2;

pub(super) const PCF_CTRL_ENABLE: u8 = 0;
pub(super) const COMP_CTRL_ENABLE: u8 = 0;
pub(super) const COMP_CTRL_AMOUNT: u8 = 1;
pub(super) const COMP_CTRL_THRESHOLD: u8 = 2;

/// Validate `(track, ctrl, val)` against ReBirth RB-338's authoritative controller ranges.
/// Preserves Track 0 Master Level (`ctrl 0`) and v4.2 per-channel Enable/Mute (`ctrls 5,11,17,23`).
pub(super) fn valid_track_event(track: usize, ctrl: u8, val: u8) -> bool {
    match track {
        TRACK_MIXER => match ctrl {
            MIXER_CTRL_MASTER_LEVEL => val <= 127,
            MIXER_CTRL_COMP_TARGET => val <= 5,
            MIXER_CTRL_PCF_TARGET => val <= 4,
            5 | 11 | 17 | 23 => val <= 1,
            6 | 7 | 8 | 12 | 13 | 14 | 18 | 19 | 20 | 24 | 25 | 26 => val <= 127,
            9 | 15 | 21 | 27 => val <= 1,
            _ => false,
        },
        1 | 2 => match ctrl {
            DEV_CTRL_ENABLE | 8 => val <= 1,
            DEV_CTRL_PATTERN => val <= 31,
            2..=7 => val <= 127,
            _ => false,
        },
        TRACK_808 => match ctrl {
            DEV_CTRL_ENABLE => val <= 1,
            DEV_CTRL_PATTERN => val <= 31,
            2..=28 => val <= 127,
            _ => false,
        },
        TRACK_909 => match ctrl {
            DEV_CTRL_ENABLE => val <= 1,
            DEV_CTRL_PATTERN => val <= 31,
            2..=29 => val <= 127,
            _ => false,
        },
        TRACK_DELAY => match ctrl {
            DELAY_CTRL_ENABLE | DELAY_CTRL_MODE => val <= 1,
            DELAY_CTRL_STEPS => (1..=32).contains(&val),
            DELAY_CTRL_FEEDBACK | DELAY_CTRL_PAN => val <= 127,
            _ => false,
        },
        TRACK_DIST | TRACK_COMP => match ctrl {
            0 => val <= 1,
            1 | 2 => val <= 127,
            _ => false,
        },
        TRACK_PCF => match ctrl {
            PCF_CTRL_ENABLE | 6 => val <= 1,
            1..=5 => val <= 127,
            _ => false,
        },
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// TB-303 Declarative Control Table (shared by static init & Song Mode)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tb303TargetKind {
    Cutoff,
    Resonance,
    EnvMod,
    FilterDecay,
    Accent,
    Tune,
    Waveform,
}

#[derive(Clone, Copy)]
pub(super) struct Tb303ControlSpec {
    pub ctrl: u8,
    pub label: &'static str,
    pub target: Tb303TargetKind,
    pub mode: ValueMode,
    pub to_native: fn(u8) -> f32,
}

impl Tb303ControlSpec {
    #[inline]
    pub(super) fn native(self, raw: u8, vintage: bool) -> f32 {
        if self.target == Tb303TargetKind::FilterDecay {
            rbs_303_decay_ms_for_mode(raw, vintage)
        } else {
            (self.to_native)(raw)
        }
    }

    #[inline]
    pub(super) fn normalized(self, raw: u8, vintage: bool) -> f32 {
        let native = self.native(raw, vintage);
        match self.target {
            Tb303TargetKind::Cutoff => ModuleParam::Cutoff.normalize(native),
            Tb303TargetKind::Resonance => ModuleParam::Resonance.normalize(native),
            Tb303TargetKind::EnvMod => {
                normalize_cable_amount(ModuleKindTag::Filter, InPort::Cv, native)
            }
            Tb303TargetKind::FilterDecay => ModuleParam::Decay.normalize(native),
            Tb303TargetKind::Accent => ModuleParam::VelAmount.normalize(native),
            Tb303TargetKind::Tune => ModuleParam::Tune.normalize(native),
            Tb303TargetKind::Waveform => ModuleParam::Waveform.normalize(native),
        }
    }

    pub(super) fn resolve_param(
        self,
        targets: super::calibration::Tb303Targets,
    ) -> InstrumentParam {
        match self.target {
            Tb303TargetKind::Cutoff => InstrumentParam::Module {
                zone: targets.zone_id,
                module: targets.filter_id,
                param: ModuleParam::Cutoff,
            },
            Tb303TargetKind::Resonance => InstrumentParam::Module {
                zone: targets.zone_id,
                module: targets.filter_id,
                param: ModuleParam::Resonance,
            },
            Tb303TargetKind::EnvMod => InstrumentParam::Cable {
                zone: targets.zone_id,
                cable: targets.env_to_filter_cable_id,
            },
            Tb303TargetKind::FilterDecay => InstrumentParam::Module {
                zone: targets.zone_id,
                module: targets.filter_env_id,
                param: ModuleParam::Decay,
            },
            Tb303TargetKind::Accent => InstrumentParam::Module {
                zone: targets.zone_id,
                module: targets.filter_id,
                param: ModuleParam::VelAmount,
            },
            Tb303TargetKind::Tune => InstrumentParam::Module {
                zone: targets.zone_id,
                module: targets.vco_id,
                param: ModuleParam::Tune,
            },
            Tb303TargetKind::Waveform => InstrumentParam::Module {
                zone: targets.zone_id,
                module: targets.vco_id,
                param: ModuleParam::Waveform,
            },
        }
    }
}

/// Authoritative TB-303 parameter table in Song-Mode automation priority order.
pub(super) const TB303_CONTROLS: [Tb303ControlSpec; 7] = [
    Tb303ControlSpec {
        ctrl: 3,
        label: "Cutoff",
        target: Tb303TargetKind::Cutoff,
        mode: ValueMode::Ramp,
        to_native: rbs_303_cutoff_hz,
    },
    Tb303ControlSpec {
        ctrl: 4,
        label: "Res",
        target: Tb303TargetKind::Resonance,
        mode: ValueMode::Ramp,
        to_native: rbs_303_resonance,
    },
    Tb303ControlSpec {
        ctrl: 5,
        label: "EnvMod",
        target: Tb303TargetKind::EnvMod,
        mode: ValueMode::Ramp,
        to_native: rbs_303_env_mod,
    },
    Tb303ControlSpec {
        ctrl: 6,
        label: "Decay",
        target: Tb303TargetKind::FilterDecay,
        mode: ValueMode::Ramp,
        to_native: rbs_303_decay_ms,
    },
    Tb303ControlSpec {
        ctrl: 7,
        label: "Accent",
        target: Tb303TargetKind::Accent,
        mode: ValueMode::Ramp,
        to_native: rbs_303_accent,
    },
    Tb303ControlSpec {
        ctrl: 2,
        label: "Tune",
        target: Tb303TargetKind::Tune,
        mode: ValueMode::Hold,
        to_native: rbs_303_tune,
    },
    Tb303ControlSpec {
        ctrl: 8,
        label: "Wave",
        target: Tb303TargetKind::Waveform,
        mode: ValueMode::Hold,
        to_native: rbs_303_waveform_f32,
    },
];

// ---------------------------------------------------------------------------
// Shared Effect Declarative Control Tables (PCF & Compressor)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub(super) struct EffectControlSpec {
    pub ctrl: u8,
    pub label: &'static str,
    pub param: EffectParam,
    pub mode: ValueMode,
    pub to_native: fn(u8) -> f32,
}

impl EffectControlSpec {
    #[inline]
    pub(super) fn normalized(self, raw: u8) -> f32 {
        self.param.normalize((self.to_native)(raw))
    }
}

/// Authoritative Pattern Controlled Filter (`PCF `, Track 7) parameter table.
pub(super) const PCF_CONTROLS: [EffectControlSpec; 6] = [
    EffectControlSpec {
        ctrl: 1,
        label: "PCF Freq",
        param: EffectParam::Cutoff,
        mode: ValueMode::Ramp,
        to_native: rbs_pcf_cutoff_hz,
    },
    EffectControlSpec {
        ctrl: 2,
        label: "PCF Res",
        param: EffectParam::Resonance,
        mode: ValueMode::Ramp,
        to_native: rbs_pcf_resonance,
    },
    EffectControlSpec {
        ctrl: 3,
        label: "PCF Amt",
        param: EffectParam::EnvAmount,
        mode: ValueMode::Ramp,
        to_native: rbs_pcf_env_amount,
    },
    EffectControlSpec {
        ctrl: 4,
        label: "PCF Pat",
        param: EffectParam::EnvPattern,
        mode: ValueMode::Hold,
        to_native: rbs_pcf_env_pattern_f32,
    },
    EffectControlSpec {
        ctrl: 5,
        label: "PCF Decay",
        param: EffectParam::EnvDecay,
        mode: ValueMode::Ramp,
        to_native: rbs_pcf_decay_ms,
    },
    EffectControlSpec {
        ctrl: 6,
        label: "PCF Mode",
        param: EffectParam::FilterMode,
        mode: ValueMode::Hold,
        to_native: rbs_pcf_filter_mode_f32,
    },
];

/// Authoritative Compressor (`COMP`, Track 8) parameter table (zero auto makeup gain).
pub(super) const COMP_CONTROLS: [EffectControlSpec; 2] = [
    EffectControlSpec {
        ctrl: COMP_CTRL_AMOUNT,
        label: "Comp Ratio",
        param: EffectParam::Ratio,
        mode: ValueMode::Ramp,
        to_native: rbs_comp_ratio,
    },
    EffectControlSpec {
        ctrl: COMP_CTRL_THRESHOLD,
        label: "Comp Thresh",
        param: EffectParam::Threshold,
        mode: ValueMode::Ramp,
        to_native: rbs_comp_threshold_db,
    },
];

// ---------------------------------------------------------------------------
// TR-808 Voice Selectors & TR-808 / TR-909 Per-Zone Drum Control Tables
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub(super) struct Tr808SelectorSpec {
    pub ctrl: u8,
    pub pattern_col: usize,
    pub primary_midi: u8,
    pub alt_midi: u8,
    pub alt_label: &'static str,
    pub alt_preset_id: &'static str,
    pub tune_ctrl: Option<u8>,
}

/// ReBirth TR-808 voice selector switches (`0` UP = Alt `LC/MC/HC/CL/MA`, `!= 0` DN = Primary `LT/MT/HT/RS/CP`).
pub(super) const TR808_SELECTOR_SPECS: [Tr808SelectorSpec; 5] = [
    Tr808SelectorSpec {
        ctrl: 11,
        pattern_col: 3,
        primary_midi: drum_midi::LT,
        alt_midi: drum_midi::LC,
        alt_label: "LC",
        alt_preset_id: "tr-808-lc",
        tune_ctrl: Some(10),
    },
    Tr808SelectorSpec {
        ctrl: 14,
        pattern_col: 4,
        primary_midi: drum_midi::MT,
        alt_midi: drum_midi::MC,
        alt_label: "MC",
        alt_preset_id: "tr-808-mc",
        tune_ctrl: Some(13),
    },
    Tr808SelectorSpec {
        ctrl: 17,
        pattern_col: 5,
        primary_midi: drum_midi::HT,
        alt_midi: drum_midi::HC,
        alt_label: "HC",
        alt_preset_id: "tr-808-hc",
        tune_ctrl: Some(16),
    },
    Tr808SelectorSpec {
        ctrl: 19,
        pattern_col: 6,
        primary_midi: drum_midi::RS,
        alt_midi: drum_midi::CL,
        alt_label: "CL",
        alt_preset_id: "tr-808-cl",
        tune_ctrl: None,
    },
    Tr808SelectorSpec {
        ctrl: 21,
        pattern_col: 7,
        primary_midi: drum_midi::CP,
        alt_midi: drum_midi::MA,
        alt_label: "MA",
        alt_preset_id: "tr-808-ma",
        tune_ctrl: None,
    },
];

#[derive(Clone, Copy)]
pub(super) enum DrumZoneParamKind {
    /// Per-voice level trim in dB (`ZoneParam::Gain` for both Sample & Synth zones).
    Gain { trim_db: f32 },
    /// Tone/brightness filter cutoff (`Filter` module `ModuleParam::Cutoff`, spliced on samples).
    Cutoff { curve: fn(u8, bool) -> f32 },
    /// Envelope decay time (`Env` module `ModuleParam::Decay`, spliced as OneShot `Env -> Vca` on samples).
    Decay {
        curve: fn(u8) -> f32,
        synth_scale: Option<f32>,
    },
    /// Pitch tuning in semitones (`Sampler` `ModuleParam::CoarseTune` or `Vco` `ModuleParam::Tune`).
    Tune { curve: fn(u8) -> f32 },
    /// Attack/Snappy emphasis (`Noise` `ModuleParam::Level` on synth, or spliced `Filter` cutoff on sample).
    NoiseOrPresence { synth_noise: fn(u8) -> f32 },
}

#[derive(Clone, Copy)]
pub(super) struct DrumZoneControlSpec {
    pub midi: u8,
    pub alt_midi: Option<u8>,
    pub label: &'static str,
    pub alt_label: Option<&'static str>,
    pub ctrl: u8,
    pub param_tag: &'static str,
    pub kind: DrumZoneParamKind,
}

impl DrumZoneControlSpec {
    /// Resolve the concrete [`InstrumentParam`] on `zone` (splicing `Filter` / `OneShot Env -> Vca`
    /// onto sampled zones on demand) and normalize a raw RBS knob value.
    pub(super) fn resolve_automation_target(
        self,
        zone: &mut RackZone,
        is_sample: bool,
    ) -> Option<InstrumentParam> {
        let zid = zone.id;
        match self.kind {
            DrumZoneParamKind::Gain { .. } => Some(InstrumentParam::Zone {
                zone: zid,
                param: ZoneParam::Gain,
            }),
            DrumZoneParamKind::Cutoff { .. } => {
                let flt_id = if is_sample {
                    super::calibration::ensure_zone_filter(zone)
                } else {
                    super::calibration::find_zone_filter(zone)
                }?;
                Some(InstrumentParam::Module {
                    zone: zid,
                    module: flt_id,
                    param: ModuleParam::Cutoff,
                })
            }
            DrumZoneParamKind::Decay { .. } => {
                let env_id = if is_sample {
                    super::calibration::ensure_zone_decay_env(zone)
                } else {
                    super::calibration::find_zone_primary_env(zone)
                }?;
                Some(InstrumentParam::Module {
                    zone: zid,
                    module: env_id,
                    param: ModuleParam::Decay,
                })
            }
            DrumZoneParamKind::Tune { .. } => {
                if is_sample {
                    let smp_id = super::calibration::find_zone_sampler(zone)?;
                    Some(InstrumentParam::Module {
                        zone: zid,
                        module: smp_id,
                        param: ModuleParam::CoarseTune,
                    })
                } else {
                    let vco_id = super::calibration::find_zone_vco(zone)?;
                    Some(InstrumentParam::Module {
                        zone: zid,
                        module: vco_id,
                        param: ModuleParam::Tune,
                    })
                }
            }
            DrumZoneParamKind::NoiseOrPresence { .. } => {
                if is_sample {
                    let flt_id = super::calibration::ensure_zone_filter(zone)?;
                    Some(InstrumentParam::Module {
                        zone: zid,
                        module: flt_id,
                        param: ModuleParam::Cutoff,
                    })
                } else {
                    let noise_id = super::calibration::find_zone_noise(zone)?;
                    Some(InstrumentParam::Module {
                        zone: zid,
                        module: noise_id,
                        param: ModuleParam::Level,
                    })
                }
            }
        }
    }

    /// Map a raw `0..=127` RBS knob value to its native physical value for `is_sample`.
    pub(super) fn native_value(self, is_sample: bool, raw: u8) -> f32 {
        match self.kind {
            DrumZoneParamKind::Gain { trim_db } => rbs_zone_level_db(raw, trim_db),
            DrumZoneParamKind::Cutoff { curve } => curve(raw, is_sample),
            DrumZoneParamKind::Decay { curve, synth_scale } => {
                let ms = curve(raw);
                if is_sample {
                    rbs_sample_decay_ms(ms)
                } else {
                    match synth_scale {
                        Some(scale) => (ms * scale).clamp(10.0, 10_000.0),
                        None => ms,
                    }
                }
            }
            DrumZoneParamKind::Tune { curve } => curve(raw),
            DrumZoneParamKind::NoiseOrPresence { synth_noise } => {
                if is_sample {
                    rbs_presence_cutoff_hz(raw)
                } else {
                    synth_noise(raw)
                }
            }
        }
    }

    /// Normalize `native` against `param`'s physical range on `kit`.
    pub(super) fn normalize_param(kit: &RackPatch, param: InstrumentParam, native: f32) -> f32 {
        match param {
            InstrumentParam::Zone { param: zp, .. } => zp.normalize(native),
            InstrumentParam::Module { param: mp, .. } => mp.normalize(native),
            InstrumentParam::Rack { param: rp } => rp.normalize(native),
            InstrumentParam::Cable { .. } => {
                let mut probe = kit.clone();
                let _ = probe.write_param_native(param, native);
                probe.read_param_normalized(param, 0).unwrap_or(0.5)
            }
        }
    }
}

/// Coupled Snare Tone + Snappy descriptor (both knobs share the spliced `Filter` `Cutoff` via
/// `min` on sampled snares, while mapping independently to `Filter.Cutoff` + `Noise.Level` on synth snares).
#[derive(Clone, Copy)]
pub(super) struct DrumSdPairSpec {
    pub tone_ctrl: u8,
    pub snappy_ctrl: u8,
    pub cutoff_fn: fn(u8, bool) -> f32,
    pub snappy_noise_fn: fn(u8) -> f32,
    /// Optional static tone-linked pitch offset (TR-808 SD Tone gently shifts snare pitch ±2 st).
    pub static_tone_tune_range: Option<f32>,
    /// Optional static synth-only decay shaping from Tone (TR-909 SD Tone shapes synth body decay).
    pub static_synth_tone_decay: Option<(f32, f32)>,
}

pub(super) const TR808_SD_PAIR: DrumSdPairSpec = DrumSdPairSpec {
    tone_ctrl: 7,
    snappy_ctrl: 8,
    cutoff_fn: rbs_808_sd_cutoff_hz,
    snappy_noise_fn: rbs_808_sd_snappy_noise,
    static_tone_tune_range: Some(4.0),
    static_synth_tone_decay: None,
};

pub(super) const TR909_SD_PAIR: DrumSdPairSpec = DrumSdPairSpec {
    tone_ctrl: 9,
    snappy_ctrl: 10,
    cutoff_fn: rbs_909_sd_cutoff_hz,
    snappy_noise_fn: rbs_909_sd_snappy_noise,
    static_tone_tune_range: None,
    static_synth_tone_decay: Some((55.0, 115.0)),
};

/// Authoritative TR-808 per-zone parameter specs ordered by Song-Mode automation priority
/// (Tone -> Decay -> Tune -> Level; SD Tone/Snappy handled via [`TR808_SD_PAIR`]).
pub(super) const TR808_ZONE_CONTROLS: &[DrumZoneControlSpec] = &[
    // 1. Tone / Cutoff
    DrumZoneControlSpec {
        midi: drum_midi::BD,
        alt_midi: None,
        label: "BD",
        alt_label: None,
        ctrl: 4,
        param_tag: "Cutoff",
        kind: DrumZoneParamKind::Cutoff {
            curve: rbs_808_bd_cutoff_hz,
        },
    },
    DrumZoneControlSpec {
        midi: drum_midi::CY_OR_RD,
        alt_midi: None,
        label: "CY",
        alt_label: None,
        ctrl: 24,
        param_tag: "Cutoff",
        kind: DrumZoneParamKind::Cutoff {
            curve: rbs_808_cy_cutoff_hz,
        },
    },
    // 2. Decay
    DrumZoneControlSpec {
        midi: drum_midi::BD,
        alt_midi: None,
        label: "BD",
        alt_label: None,
        ctrl: 5,
        param_tag: "Decay",
        kind: DrumZoneParamKind::Decay {
            curve: rbs_808_bd_decay_ms,
            synth_scale: None,
        },
    },
    DrumZoneControlSpec {
        midi: drum_midi::OH,
        alt_midi: None,
        label: "OH",
        alt_label: None,
        ctrl: 27,
        param_tag: "Decay",
        kind: DrumZoneParamKind::Decay {
            curve: rbs_808_oh_decay_ms,
            synth_scale: Some(TR808_OH_SYNTH_DECAY_SCALE),
        },
    },
    DrumZoneControlSpec {
        midi: drum_midi::CY_OR_RD,
        alt_midi: None,
        label: "CY",
        alt_label: None,
        ctrl: 25,
        param_tag: "Decay",
        kind: DrumZoneParamKind::Decay {
            curve: rbs_808_cy_decay_ms,
            synth_scale: Some(TR808_CY_SYNTH_DECAY_SCALE),
        },
    },
    // 3. Tune (shared by Toms and dynamic Conga alt zones)
    DrumZoneControlSpec {
        midi: drum_midi::LT,
        alt_midi: Some(drum_midi::LC),
        label: "LT",
        alt_label: Some("LC"),
        ctrl: 10,
        param_tag: "Tune",
        kind: DrumZoneParamKind::Tune {
            curve: rbs_808_tom_tune,
        },
    },
    DrumZoneControlSpec {
        midi: drum_midi::MT,
        alt_midi: Some(drum_midi::MC),
        label: "MT",
        alt_label: Some("MC"),
        ctrl: 13,
        param_tag: "Tune",
        kind: DrumZoneParamKind::Tune {
            curve: rbs_808_tom_tune,
        },
    },
    DrumZoneControlSpec {
        midi: drum_midi::HT,
        alt_midi: Some(drum_midi::HC),
        label: "HT",
        alt_label: Some("HC"),
        ctrl: 16,
        param_tag: "Tune",
        kind: DrumZoneParamKind::Tune {
            curve: rbs_808_tom_tune,
        },
    },
    // 4. Per-voice Level (shared with dynamic alt zones)
    DrumZoneControlSpec {
        midi: drum_midi::BD,
        alt_midi: None,
        label: "BD",
        alt_label: None,
        ctrl: 3,
        param_tag: "Gain",
        kind: DrumZoneParamKind::Gain { trim_db: 2.5 },
    },
    DrumZoneControlSpec {
        midi: drum_midi::SD,
        alt_midi: None,
        label: "SD",
        alt_label: None,
        ctrl: 6,
        param_tag: "Gain",
        kind: DrumZoneParamKind::Gain { trim_db: 1.0 },
    },
    DrumZoneControlSpec {
        midi: drum_midi::LT,
        alt_midi: Some(drum_midi::LC),
        label: "LT",
        alt_label: Some("LC"),
        ctrl: 9,
        param_tag: "Gain",
        kind: DrumZoneParamKind::Gain { trim_db: 0.5 },
    },
    DrumZoneControlSpec {
        midi: drum_midi::MT,
        alt_midi: Some(drum_midi::MC),
        label: "MT",
        alt_label: Some("MC"),
        ctrl: 12,
        param_tag: "Gain",
        kind: DrumZoneParamKind::Gain { trim_db: 0.5 },
    },
    DrumZoneControlSpec {
        midi: drum_midi::HT,
        alt_midi: Some(drum_midi::HC),
        label: "HT",
        alt_label: Some("HC"),
        ctrl: 15,
        param_tag: "Gain",
        kind: DrumZoneParamKind::Gain { trim_db: 0.5 },
    },
    DrumZoneControlSpec {
        midi: drum_midi::RS,
        alt_midi: Some(drum_midi::CL),
        label: "RS",
        alt_label: Some("CL"),
        ctrl: 18,
        param_tag: "Gain",
        kind: DrumZoneParamKind::Gain { trim_db: -1.0 },
    },
    DrumZoneControlSpec {
        midi: drum_midi::CH,
        alt_midi: None,
        label: "CH",
        alt_label: None,
        ctrl: 28,
        param_tag: "Gain",
        kind: DrumZoneParamKind::Gain { trim_db: -1.8 },
    },
    DrumZoneControlSpec {
        midi: drum_midi::CP,
        alt_midi: Some(drum_midi::MA),
        label: "CP",
        alt_label: Some("MA"),
        ctrl: 20,
        param_tag: "Gain",
        kind: DrumZoneParamKind::Gain { trim_db: 0.5 },
    },
    DrumZoneControlSpec {
        midi: drum_midi::CB_OR_CR,
        alt_midi: None,
        label: "CB",
        alt_label: None,
        ctrl: 22,
        param_tag: "Gain",
        kind: DrumZoneParamKind::Gain { trim_db: -1.0 },
    },
    DrumZoneControlSpec {
        midi: drum_midi::CY_OR_RD,
        alt_midi: None,
        label: "CY",
        alt_label: None,
        ctrl: 23,
        param_tag: "Gain",
        kind: DrumZoneParamKind::Gain { trim_db: -1.8 },
    },
    DrumZoneControlSpec {
        midi: drum_midi::OH,
        alt_midi: None,
        label: "OH",
        alt_label: None,
        ctrl: 26,
        param_tag: "Gain",
        kind: DrumZoneParamKind::Gain { trim_db: -1.5 },
    },
];

/// Authoritative TR-909 per-zone parameter specs ordered by Song-Mode automation priority
/// (Attack -> Decay -> Tune -> Level; SD Tone/Snappy handled via [`TR909_SD_PAIR`]).
pub(super) const TR909_ZONE_CONTROLS: &[DrumZoneControlSpec] = &[
    // 1. Attack / Presence
    DrumZoneControlSpec {
        midi: drum_midi::BD,
        alt_midi: None,
        label: "BD",
        alt_label: None,
        ctrl: 5,
        param_tag: "Attack",
        kind: DrumZoneParamKind::NoiseOrPresence {
            synth_noise: rbs_909_bd_attack_noise,
        },
    },
    // 2. Decay
    DrumZoneControlSpec {
        midi: drum_midi::BD,
        alt_midi: None,
        label: "BD",
        alt_label: None,
        ctrl: 6,
        param_tag: "Decay",
        kind: DrumZoneParamKind::Decay {
            curve: rbs_909_bd_decay_ms,
            synth_scale: None,
        },
    },
    DrumZoneControlSpec {
        midi: drum_midi::OH,
        alt_midi: None,
        label: "OH",
        alt_label: None,
        ctrl: 24,
        param_tag: "Decay",
        kind: DrumZoneParamKind::Decay {
            curve: rbs_909_oh_decay_ms,
            synth_scale: Some(1.0),
        },
    },
    DrumZoneControlSpec {
        midi: drum_midi::CH,
        alt_midi: None,
        label: "CH",
        alt_label: None,
        ctrl: 23,
        param_tag: "Decay",
        kind: DrumZoneParamKind::Decay {
            curve: rbs_909_ch_decay_ms,
            synth_scale: Some(1.0),
        },
    },
    DrumZoneControlSpec {
        midi: drum_midi::LT,
        alt_midi: None,
        label: "LT",
        alt_label: None,
        ctrl: 13,
        param_tag: "Decay",
        kind: DrumZoneParamKind::Decay {
            curve: rbs_909_tom_decay_ms,
            synth_scale: None,
        },
    },
    DrumZoneControlSpec {
        midi: drum_midi::MT,
        alt_midi: None,
        label: "MT",
        alt_label: None,
        ctrl: 16,
        param_tag: "Decay",
        kind: DrumZoneParamKind::Decay {
            curve: rbs_909_tom_decay_ms,
            synth_scale: None,
        },
    },
    DrumZoneControlSpec {
        midi: drum_midi::HT,
        alt_midi: None,
        label: "HT",
        alt_label: None,
        ctrl: 19,
        param_tag: "Decay",
        kind: DrumZoneParamKind::Decay {
            curve: rbs_909_tom_decay_ms,
            synth_scale: None,
        },
    },
    // 3. Tune
    DrumZoneControlSpec {
        midi: drum_midi::BD,
        alt_midi: None,
        label: "BD",
        alt_label: None,
        ctrl: 4,
        param_tag: "Tune",
        kind: DrumZoneParamKind::Tune {
            curve: rbs_909_bd_tune,
        },
    },
    DrumZoneControlSpec {
        midi: drum_midi::SD,
        alt_midi: None,
        label: "SD",
        alt_label: None,
        ctrl: 8,
        param_tag: "Tune",
        kind: DrumZoneParamKind::Tune {
            curve: rbs_909_sd_tune,
        },
    },
    DrumZoneControlSpec {
        midi: drum_midi::LT,
        alt_midi: None,
        label: "LT",
        alt_label: None,
        ctrl: 12,
        param_tag: "Tune",
        kind: DrumZoneParamKind::Tune {
            curve: rbs_909_pitch_tune,
        },
    },
    DrumZoneControlSpec {
        midi: drum_midi::MT,
        alt_midi: None,
        label: "MT",
        alt_label: None,
        ctrl: 15,
        param_tag: "Tune",
        kind: DrumZoneParamKind::Tune {
            curve: rbs_909_pitch_tune,
        },
    },
    DrumZoneControlSpec {
        midi: drum_midi::HT,
        alt_midi: None,
        label: "HT",
        alt_label: None,
        ctrl: 18,
        param_tag: "Tune",
        kind: DrumZoneParamKind::Tune {
            curve: rbs_909_pitch_tune,
        },
    },
    DrumZoneControlSpec {
        midi: drum_midi::CB_OR_CR,
        alt_midi: None,
        label: "CR",
        alt_label: None,
        ctrl: 26,
        param_tag: "Tune",
        kind: DrumZoneParamKind::Tune {
            curve: rbs_909_pitch_tune,
        },
    },
    DrumZoneControlSpec {
        midi: drum_midi::CY_OR_RD,
        alt_midi: None,
        label: "RD",
        alt_label: None,
        ctrl: 28,
        param_tag: "Tune",
        kind: DrumZoneParamKind::Tune {
            curve: rbs_909_pitch_tune,
        },
    },
    // 4. Per-voice Level (including shared HH ctrl 22 on both CH & OH)
    DrumZoneControlSpec {
        midi: drum_midi::BD,
        alt_midi: None,
        label: "BD",
        alt_label: None,
        ctrl: 3,
        param_tag: "Gain",
        kind: DrumZoneParamKind::Gain { trim_db: 2.5 },
    },
    DrumZoneControlSpec {
        midi: drum_midi::SD,
        alt_midi: None,
        label: "SD",
        alt_label: None,
        ctrl: 7,
        param_tag: "Gain",
        kind: DrumZoneParamKind::Gain { trim_db: 1.0 },
    },
    DrumZoneControlSpec {
        midi: drum_midi::LT,
        alt_midi: None,
        label: "LT",
        alt_label: None,
        ctrl: 11,
        param_tag: "Gain",
        kind: DrumZoneParamKind::Gain { trim_db: 0.5 },
    },
    DrumZoneControlSpec {
        midi: drum_midi::MT,
        alt_midi: None,
        label: "MT",
        alt_label: None,
        ctrl: 14,
        param_tag: "Gain",
        kind: DrumZoneParamKind::Gain { trim_db: 0.5 },
    },
    DrumZoneControlSpec {
        midi: drum_midi::HT,
        alt_midi: None,
        label: "HT",
        alt_label: None,
        ctrl: 17,
        param_tag: "Gain",
        kind: DrumZoneParamKind::Gain { trim_db: 0.5 },
    },
    DrumZoneControlSpec {
        midi: drum_midi::RS,
        alt_midi: None,
        label: "RS",
        alt_label: None,
        ctrl: 20,
        param_tag: "Gain",
        kind: DrumZoneParamKind::Gain { trim_db: -1.0 },
    },
    DrumZoneControlSpec {
        midi: drum_midi::CH,
        alt_midi: None,
        label: "CH",
        alt_label: None,
        ctrl: 22,
        param_tag: "Gain",
        kind: DrumZoneParamKind::Gain { trim_db: -1.8 },
    },
    DrumZoneControlSpec {
        midi: drum_midi::CP,
        alt_midi: None,
        label: "CP",
        alt_label: None,
        ctrl: 21,
        param_tag: "Gain",
        kind: DrumZoneParamKind::Gain { trim_db: 0.5 },
    },
    DrumZoneControlSpec {
        midi: drum_midi::CB_OR_CR,
        alt_midi: None,
        label: "CR",
        alt_label: None,
        ctrl: 25,
        param_tag: "Gain",
        kind: DrumZoneParamKind::Gain { trim_db: -1.5 },
    },
    DrumZoneControlSpec {
        midi: drum_midi::CY_OR_RD,
        alt_midi: None,
        label: "RD",
        alt_label: None,
        ctrl: 27,
        param_tag: "Gain",
        kind: DrumZoneParamKind::Gain { trim_db: -1.8 },
    },
    DrumZoneControlSpec {
        midi: drum_midi::OH,
        alt_midi: None,
        label: "OH",
        alt_label: None,
        ctrl: 22,
        param_tag: "Gain",
        kind: DrumZoneParamKind::Gain { trim_db: -1.5 },
    },
];
