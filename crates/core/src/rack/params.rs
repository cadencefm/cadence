//! Unified parameter metadata, domains, normalization, formatting, parsing,
//! and nudging for `RackParam`, `ZoneParam`, and `ModuleParam`.

use crate::dsp::env::EnvMode;
use crate::dsp::lofi::{LOFI_MAX_BITS, LOFI_MAX_RATE_HZ, LOFI_MIN_BITS, LOFI_MIN_RATE_HZ};
use crate::dsp::math::{log_to_norm, norm_to_log};
use crate::dsp::resonator::{RESONATOR_MAX_DECAY_MS, RESONATOR_MIN_DECAY_MS};
use crate::dsp::saturate::{
    DRIVE_MAX_OUTPUT_DB, DRIVE_MAX_TONE_HZ, DRIVE_MIN_OUTPUT_DB, DRIVE_MIN_TONE_HZ,
};
use crate::mixer::DriveMode;
use crate::params::{
    format_choice_index, format_db, format_hz, format_ms, format_pan, normalize_choice_token,
    nudge_log, nudge_quantized, parse_choice_index, parse_db, parse_hz, parse_ms, parse_pan,
    parse_percent_or_unit,
};
use crate::rack::modules::ModuleKindTag;
use crate::rack::types::{
    FilterMode, LfoShape, NoiseColor, PitchTracking, RackPatch, RackZone, ResonatorMode,
    TriggerMode, Unit, UpdatePolicy, VoiceMode, VoiceSettings, Waveform,
};
use crate::sampler::SampleLoopMode;
use anyhow::{anyhow, ensure, Context, Result};
use serde::{Deserialize, Serialize};

pub const LFO_DIVISIONS: &[f32] = &[
    0.25, 0.5, 1.0, 2.0, 3.0, 4.0, 6.0, 8.0, 12.0, 16.0, 32.0, 64.0,
];
pub const LFO_DIVISION_LABELS: &[&str] = &[
    "1/64", "1/32", "1/16", "1/8", "3/16", "1/4", "3/8", "1/2", "3/4", "1/1", "2/1", "4/1",
];

macro_rules! define_rack_params {
    (
        $(
            $variant:ident => (
                $name:literal,
                $label:literal,
                $module:literal,
                $field:ident,
                ($lo:expr, $hi:expr),
                [ $($alias:literal),* $(,)? ]
            )
        ),+ $(,)?
    ) => {
        /// Rack-wide output bus parameters shared across all zones of a [`RackPatch`].
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        #[repr(u8)]
        pub enum RackParam {
            $( $variant, )+
        }

        impl RackParam {
            pub const ALL: [Self; [ $( Self::$variant ),+ ].len()] = [ $( Self::$variant, )+ ];

            pub fn name(self) -> &'static str {
                match self {
                    $( Self::$variant => $name, )+
                }
            }

            pub fn label(self) -> &'static str {
                match self {
                    $( Self::$variant => $label, )+
                }
            }

            pub fn module(self) -> &'static str {
                match self {
                    $( Self::$variant => $module, )+
                }
            }

            pub fn range(self) -> (f32, f32) {
                match self {
                    $( Self::$variant => ($lo, $hi), )+
                }
            }

            pub fn parse(input: &str) -> Option<Self> {
                match normalize_choice_token(input).as_str() {
                    $( $name $( | $alias )* => Some(Self::$variant), )+
                    _ => None,
                }
            }

            pub fn read_native(self, rack: &RackPatch) -> f32 {
                match self {
                    $( Self::$variant => rack.$field, )+
                }
            }

            pub fn write_native(self, rack: &mut RackPatch, value: f32) -> Result<()> {
                let (lo, hi) = self.range();
                ensure!(
                    value.is_finite() && (lo..=hi).contains(&value),
                    "Rack {} must be finite and within {lo}..={hi}",
                    self.label()
                );
                match self {
                    $( Self::$variant => rack.$field = value, )+
                }
                Ok(())
            }
        }
    };
}

define_rack_params! {
    Gain      => ("gain",      "Gain",      "OUTPUT", gain_db,        (-24.0, 12.0),     ["rack_gain", "kit_gain", "level"]),
    Pan       => ("pan",       "Pan",       "OUTPUT", pan,            (-1.0, 1.0),       ["rack_pan", "kit_pan"]),
    Tune      => ("tune",      "Tune",      "PITCH",  tune_semitones, (-24.0, 24.0),     ["rack_tune", "kit_tune", "pitch"]),
    Cutoff    => ("cutoff",    "Cutoff",    "FILTER", cutoff_hz,      (30.0, 20_000.0),  ["rack_cutoff", "kit_cutoff", "filter"]),
    Resonance => ("resonance", "Resonance", "FILTER", resonance,      (0.0, 0.92),       ["res", "reso", "rack_res", "kit_res"]),
}

impl RackParam {
    pub fn id(self) -> usize {
        self as usize
    }

    pub fn from_slot(id: usize) -> Option<Self> {
        Self::ALL.get(id).copied()
    }

    pub fn policy(self) -> UpdatePolicy {
        UpdatePolicy::Continuous
    }

    pub fn is_automatable(self) -> bool {
        true
    }

    pub fn normalize(self, native: f32) -> f32 {
        let (lo, hi) = self.range();
        if self == Self::Cutoff {
            log_to_norm(native, lo, hi)
        } else {
            ((native - lo) / (hi - lo)).clamp(0.0, 1.0)
        }
    }

    pub fn read_normalized(self, rack: &RackPatch) -> f32 {
        self.normalize(self.read_native(rack))
    }

    pub fn denormalize(self, norm: f32) -> f32 {
        let n = if norm.is_finite() {
            norm.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (lo, hi) = self.range();
        if self == Self::Cutoff {
            norm_to_log(n, lo, hi)
        } else {
            (lo + n * (hi - lo)).clamp(lo, hi)
        }
    }

    pub fn write_normalized(self, rack: &mut RackPatch, norm: f32) {
        let _ = self.write_native(rack, self.denormalize(norm));
    }

    pub fn format_native(self, v: f32) -> String {
        match self {
            Self::Gain => format_db(v),
            Self::Pan => format_pan(v),
            Self::Tune => format!("{v:+.1} st"),
            Self::Cutoff => format_hz(v),
            Self::Resonance => format!("{:.0}%", v * 100.0),
        }
    }

    pub fn format_normalized(self, norm: f32) -> String {
        self.format_native(self.denormalize(norm))
    }

    pub fn parse_value_native(self, input: &str) -> Result<f32> {
        let raw = input.trim().to_ascii_lowercase();
        let value = match self {
            Self::Gain => parse_db(input)?,
            Self::Pan => parse_pan(input)?,
            Self::Tune => raw.strip_suffix("st").unwrap_or(&raw).trim().parse()?,
            Self::Cutoff => parse_hz(input)?,
            Self::Resonance => parse_percent_or_unit(input)?,
        };
        let (lo, hi) = self.range();
        ensure!(
            value.is_finite() && (lo..=hi).contains(&value),
            "Rack {} must be within {lo}..={hi}",
            self.label()
        );
        Ok(value)
    }

    pub fn nudge_native(self, current: f32, dir: i8, coarse: bool) -> f32 {
        let (lo, hi) = self.range();
        let next = match self {
            Self::Gain => nudge_quantized(current, dir, 2.0, 1.0, 4.0, coarse),
            Self::Pan => nudge_quantized(current, dir, 50.0, 1.0, 5.0, coarse),
            Self::Tune => nudge_quantized(current, dir, 1.0, 1.0, 12.0, coarse),
            Self::Cutoff => nudge_log(current, dir, 1.1, 1.4, coarse),
            Self::Resonance => nudge_quantized(current, dir, 100.0, 1.0, 5.0, coarse),
        };
        next.clamp(lo, hi)
    }
}

macro_rules! define_zone_params {
    (
        $(
            $variant:ident => (
                $name:literal,
                $label:literal,
                $policy:ident,
                ($lo:expr, $hi:expr)
                $(, [ $($alias:literal),* $(,)? ])?
            )
        ),+ $(,)?
    ) => {
        /// Zone-level mix and voice-allocation parameters on a [`RackZone`].
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        #[repr(u8)]
        pub enum ZoneParam {
            $( $variant, )+
        }

        impl ZoneParam {
            pub const ALL: [Self; [ $( Self::$variant ),+ ].len()] = [ $( Self::$variant, )+ ];

            pub fn name(self) -> &'static str {
                match self {
                    $( Self::$variant => $name, )+
                }
            }

            pub fn label(self) -> &'static str {
                match self {
                    $( Self::$variant => $label, )+
                }
            }

            pub fn policy(self) -> UpdatePolicy {
                match self {
                    $( Self::$variant => UpdatePolicy::$policy, )+
                }
            }

            pub fn range(self) -> (f32, f32) {
                match self {
                    $( Self::$variant => ($lo, $hi), )+
                }
            }

            pub fn parse(input: &str) -> Option<Self> {
                match normalize_choice_token(input).as_str() {
                    $( $name $( $( | $alias )* )? => Some(Self::$variant), )+
                    _ => None,
                }
            }
        }
    };
}

define_zone_params! {
    Gain          => ("gain",           "Zone Gain",   Continuous, (RackZone::MIN_GAIN_DB, RackZone::MAX_GAIN_DB),        ["zone_gain", "gain_db", "level"]),
    Pan           => ("pan",            "Zone Pan",    Continuous, (RackZone::MIN_PAN, RackZone::MAX_PAN),                ["zone_pan"]),
    VoiceMode     => ("voice_mode",     "Voice Mode",  Structural, (0.0, (VoiceMode::ALL.len() - 1) as f32),              ["voice", "poly", "mono"]),
    Legato        => ("legato",         "Legato",      Structural, (0.0, 1.0)),
    Glide         => ("glide",          "Glide",       OnTrigger,  (VoiceSettings::MIN_GLIDE_MS, VoiceSettings::MAX_GLIDE_MS), ["glide_ms", "portamento", "slide"]),
    PitchTracking => ("pitch_tracking", "Pitch Track", OnTrigger,  (0.0, (PitchTracking::ALL.len() - 1) as f32),          ["pitch_track", "tracking"]),
    PhaseReset    => ("phase_reset",    "Phase Reset", OnTrigger,  (0.0, 1.0),                                            ["retrig_phase"]),
    RootMidi      => ("root_midi",      "Root Note",   OnTrigger,  (0.0, 127.0),                                          ["root", "root_note", "reference_pitch", "reference"]),
}

impl ZoneParam {
    pub fn is_automatable(self) -> bool {
        self.policy() != UpdatePolicy::Structural
    }

    pub fn read_native(self, zone: &RackZone) -> f32 {
        match self {
            Self::Gain => zone.gain_db,
            Self::Pan => zone.pan,
            Self::VoiceMode => zone.voice.voice_mode.to_index() as f32,
            Self::Legato => f32::from(u8::from(zone.voice.legato)),
            Self::Glide => zone.voice.glide_ms,
            Self::PitchTracking => zone.voice.pitch_tracking.to_index() as f32,
            Self::PhaseReset => f32::from(u8::from(zone.voice.phase_reset)),
            Self::RootMidi => f32::from(zone.root_midi),
        }
    }

    pub fn write_native(self, zone: &mut RackZone, value: f32) -> Result<()> {
        let (lo, hi) = self.range();
        ensure!(
            value.is_finite() && (lo..=hi).contains(&value),
            "{} must be finite and within {lo}..={hi}",
            self.label()
        );
        match self {
            Self::Gain => zone.gain_db = value,
            Self::Pan => zone.pan = value,
            Self::VoiceMode => {
                zone.voice.voice_mode = VoiceMode::from_index(value.round().max(0.0) as usize);
            }
            Self::Legato => zone.voice.legato = value >= 0.5,
            Self::Glide => zone.voice.glide_ms = value,
            Self::PitchTracking => {
                zone.voice.pitch_tracking =
                    PitchTracking::from_index(value.round().max(0.0) as usize);
            }
            Self::PhaseReset => zone.voice.phase_reset = value >= 0.5,
            Self::RootMidi => zone.set_root_midi(value.round() as u8),
        }
        Ok(())
    }

    pub fn normalize(self, native: f32) -> f32 {
        let (lo, hi) = self.range();
        ((native - lo) / (hi - lo)).clamp(0.0, 1.0)
    }

    pub fn read_normalized(self, zone: &RackZone) -> f32 {
        self.normalize(self.read_native(zone))
    }

    pub fn denormalize(self, norm: f32) -> f32 {
        let n = if norm.is_finite() {
            norm.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (lo, hi) = self.range();
        let raw = lo + n * (hi - lo);
        match self {
            Self::VoiceMode
            | Self::Legato
            | Self::PitchTracking
            | Self::PhaseReset
            | Self::RootMidi => raw.round().clamp(lo, hi),
            Self::Gain | Self::Pan | Self::Glide => raw.clamp(lo, hi),
        }
    }

    pub fn write_normalized(self, zone: &mut RackZone, norm: f32) {
        let _ = self.write_native(zone, self.denormalize(norm));
    }

    pub fn format_native(self, v: f32) -> String {
        match self {
            Self::Gain => format_db(v),
            Self::Pan => format_pan(v),
            Self::VoiceMode => format_choice_index::<VoiceMode>(v).into(),
            Self::Legato | Self::PhaseReset => if v >= 0.5 { "On" } else { "Off" }.into(),
            Self::Glide => format_ms(v),
            Self::PitchTracking => format_choice_index::<PitchTracking>(v).into(),
            Self::RootMidi => {
                let midi = v.round().clamp(0.0, 127.0) as u8;
                format!("{} ({midi})", crate::music::note_name(midi))
            }
        }
    }

    pub fn format_normalized(self, norm: f32) -> String {
        self.format_native(self.denormalize(norm))
    }

    pub fn parse_value_native(self, input: &str) -> Result<f32> {
        let raw = input.trim();
        let value = match self {
            Self::Gain => parse_db(raw)?,
            Self::Pan => parse_pan(raw)?,
            Self::VoiceMode => {
                parse_choice_index::<VoiceMode>(raw, "Voice Mode expects poly / mono")?
            }
            Self::Legato | Self::PhaseReset => parse_bool_text(raw, self.label())?,
            Self::Glide => parse_ms(raw)?,
            Self::PitchTracking => {
                parse_choice_index::<PitchTracking>(raw, "Pitch Track expects chromatic / fixed")?
            }
            Self::RootMidi => parse_note_text(raw)?,
        };
        let (lo, hi) = self.range();
        ensure!(
            value.is_finite() && (lo..=hi).contains(&value),
            "{} must be within {lo}..={hi}",
            self.label()
        );
        Ok(value)
    }

    pub fn nudge_native(self, current: f32, dir: i8, coarse: bool) -> f32 {
        let (lo, hi) = self.range();
        let next = match self {
            Self::Gain => nudge_quantized(current, dir, 2.0, 1.0, 4.0, coarse),
            Self::Pan => nudge_quantized(current, dir, 50.0, 1.0, 5.0, coarse),
            Self::VoiceMode | Self::Legato | Self::PitchTracking | Self::PhaseReset => {
                current.round() + f32::from(dir)
            }
            Self::Glide => nudge_quantized(current, dir, 1.0, 5.0, 50.0, coarse),
            Self::RootMidi => nudge_quantized(current, dir, 1.0, 1.0, 12.0, coarse),
        };
        next.clamp(lo, hi)
    }
}

macro_rules! define_choice_kinds {
    ($( $kind:ident ),+ $(,)?) => {
        #[derive(Clone, Copy)]
        enum ChoiceKind {
            $( $kind, )+
        }

        impl ChoiceKind {
            fn count(self) -> usize {
                match self {
                    $( Self::$kind => $kind::ALL.len(), )+
                }
            }

            fn format(self, idx: usize) -> &'static str {
                match self {
                    $( Self::$kind => $kind::from_index(idx).label(), )+
                }
            }

            fn parse(self, raw: &str) -> Option<usize> {
                match self {
                    $( Self::$kind => $kind::parse(raw).map($kind::to_index), )+
                }
            }
        }
    };
}

define_choice_kinds!(
    Waveform,
    PitchTracking,
    SampleLoopMode,
    NoiseColor,
    EnvMode,
    TriggerMode,
    LfoShape,
    FilterMode,
    DriveMode,
    ResonatorMode,
);

#[derive(Clone, Copy)]
enum ParamDomain {
    Linear(f32, f32),
    Log(f32, f32),
    Integer(i32, i32),
    Boolean,
    Choice(ChoiceKind),
    Division,
    Frame,
}

impl ParamDomain {
    fn range(self) -> (f32, f32) {
        match self {
            Self::Linear(a, b) | Self::Log(a, b) => (a, b),
            Self::Integer(a, b) => (a as f32, b as f32),
            Self::Boolean => (0.0, 1.0),
            Self::Choice(c) => (0.0, (c.count() - 1) as f32),
            Self::Division => (0.25, 64.0),
            Self::Frame => (0.0, f32::MAX),
        }
    }

    fn normalize(self, value: f32) -> f32 {
        if !value.is_finite() {
            return 0.0;
        }
        let (a, b) = self.range();
        match self {
            Self::Log(_, _) => log_to_norm(value, a, b),
            Self::Division => {
                let i = LFO_DIVISIONS
                    .iter()
                    .position(|&v| (v - value).abs() < 1e-6)
                    .unwrap_or(0);
                i as f32 / (LFO_DIVISIONS.len() - 1) as f32
            }
            Self::Frame => 0.0,
            _ => ((value - a) / (b - a)).clamp(0.0, 1.0),
        }
    }

    fn denormalize(self, norm: f32, sample_frames: usize) -> f32 {
        let n = if norm.is_finite() {
            norm.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (a, b) = self.range();
        match self {
            Self::Log(_, _) => norm_to_log(n, a, b),
            Self::Division => {
                LFO_DIVISIONS[(n * (LFO_DIVISIONS.len() - 1) as f32).round() as usize]
            }
            Self::Integer(_, _) | Self::Boolean | Self::Choice(_) => {
                (a + (b - a) * n).round().clamp(a, b)
            }
            Self::Frame => (n * sample_frames as f32).round(),
            Self::Linear(_, _) => (a + (b - a) * n).clamp(a, b),
        }
    }
}

#[derive(Clone, Copy)]
struct ModuleParamSpec {
    name: &'static str,
    label: &'static str,
    policy: UpdatePolicy,
    domain: ParamDomain,
    unit: Unit,
}

macro_rules! define_module_params {
    (
        $(
            $variant:ident => ($name:literal, $label:literal, $policy:ident, $domain:tt, $unit:ident)
        ),+ $(,)?
    ) => {
        /// Self-describing parameter identifier across all 11 Eurorack [`crate::rack::ModuleKind`]s.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        #[repr(u8)]
        pub enum ModuleParam {
            $( $variant, )+
        }

        const MODULE_SPECS: [ModuleParamSpec; [ $( ModuleParam::$variant ),+ ].len()] = [
            $(
                ModuleParamSpec {
                    name: $name,
                    label: $label,
                    policy: UpdatePolicy::$policy,
                    domain: define_module_params!(@domain $domain),
                    unit: Unit::$unit,
                },
            )+
        ];

        impl ModuleParam {
            pub const ALL: [Self; [ $( Self::$variant ),+ ].len()] = [ $( Self::$variant, )+ ];
        }
    };
    (@domain (Linear $lo:expr, $hi:expr)) => { ParamDomain::Linear($lo, $hi) };
    (@domain (Log $lo:expr, $hi:expr)) => { ParamDomain::Log($lo, $hi) };
    (@domain (Int $lo:expr, $hi:expr)) => { ParamDomain::Integer($lo, $hi) };
    (@domain (Choice $kind:ident)) => { ParamDomain::Choice(ChoiceKind::$kind) };
    (@domain Boolean) => { ParamDomain::Boolean };
    (@domain Division) => { ParamDomain::Division };
    (@domain Frame) => { ParamDomain::Frame };
}

define_module_params! {
    // VCO (0..11)
    Waveform       => ("waveform",         "Wave",          OnTrigger,  (Choice Waveform),                                   Plain),
    Tune           => ("tune",             "Tune",          Continuous, (Linear -48.0, 48.0),                                Semitones),
    Fine           => ("fine",             "Fine",          Continuous, (Linear -100.0, 100.0),                              Cents),
    PulseWidth     => ("pulse_width",      "Pulse Width",   Continuous, (Linear 0.05, 0.95),                                 Percent),
    Phase          => ("phase",            "Phase",         OnTrigger,  (Linear 0.0, 360.0),                                 Degrees),
    Level          => ("level",            "Level",         Continuous, (Linear 0.0, 1.0),                                   Percent),
    VcoPitchTrack  => ("vco_pitch_track",  "Pitch Track",   OnTrigger,  (Choice PitchTracking),                              Plain),
    FixedHz        => ("fixed_hz",         "Fixed Freq",    Continuous, (Log 20.0, 20_000.0),                                Hz),
    UnisonVoices   => ("unison_voices",    "Unison",        OnTrigger,  (Int 1, 7),                                          Plain),
    UnisonDetune   => ("unison_detune",    "Detune",        Continuous, (Linear 0.0, 50.0),                                  Cents),
    UnisonWidth    => ("unison_width",     "Width",         Continuous, (Linear 0.0, 1.0),                                   Percent),
    // Sampler (11..20)
    RootMidi       => ("root_midi",        "Root",          OnTrigger,  (Int 0, 127),                                        Note),
    CoarseTune     => ("coarse_tune",      "Coarse",        Continuous, (Int -48, 48),                                       Semitones),
    SampleStart    => ("sample_start",     "Start",         OnTrigger,  Frame,                                               Plain),
    SampleEnd      => ("sample_end",       "End",           OnTrigger,  Frame,                                               Plain),
    LoopMode       => ("loop_mode",        "Loop",          Structural, (Choice SampleLoopMode),                             Plain),
    LoopStart      => ("loop_start",       "Loop Start",    OnTrigger,  Frame,                                               Plain),
    LoopEnd        => ("loop_end",         "Loop End",      OnTrigger,  Frame,                                               Plain),
    LoopXfade      => ("loop_xfade",       "Loop XFade",    Structural, (Linear 0.0, 100.0),                                 Ms),
    OneShot        => ("one_shot",         "One-Shot",      Structural, Boolean,                                             Plain),
    // Noise (20..23)
    NoiseColor     => ("noise_color",      "Color",         OnTrigger,  (Choice NoiseColor),                                 Plain),
    HighpassHz     => ("highpass_hz",      "Highpass",      Continuous, (Log 20.0, 18_000.0),                                Hz),
    LowpassHz      => ("lowpass_hz",       "Lowpass",       Continuous, (Log 20.0, 20_000.0),                                Hz),
    // Env (23..33)
    EnvMode        => ("env_mode",         "Env Mode",      OnTrigger,  (Choice EnvMode),                                    Plain),
    Attack         => ("attack",           "Attack",        OnTrigger,  (Log 0.1, 10_000.0),                                 Ms),
    Hold           => ("hold",             "Hold",          OnTrigger,  (Linear 0.0, 500.0),                                 Ms),
    Decay          => ("decay",            "Decay",         OnTrigger,  (Log 1.0, 20_000.0),                                 Ms),
    Sustain        => ("sustain",          "Sustain",       Continuous, (Linear 0.0, 1.0),                                   Percent),
    Release        => ("release",          "Release",       OnTrigger,  (Log 1.0, 20_000.0),                                 Ms),
    TriggerMode    => ("trigger_mode",     "Trigger",       OnTrigger,  (Choice TriggerMode),                                Plain),
    DelayMs        => ("delay_ms",         "Pre-Delay",     OnTrigger,  (Linear 0.0, 500.0),                                 Ms),
    Bursts         => ("bursts",           "Bursts",        OnTrigger,  (Int 1, 8),                                          Plain),
    BurstSpacingMs => ("burst_spacing_ms", "Burst Spacing", OnTrigger,  (Linear 2.0, 50.0),                                  Ms),
    // LFO (33..39)
    LfoShape       => ("lfo_shape",        "Shape",         OnTrigger,  (Choice LfoShape),                                   Plain),
    LfoRate        => ("lfo_rate",         "Rate",          Continuous, (Log 0.01, 40.0),                                    Hz),
    LfoSync        => ("lfo_sync",         "Sync",          OnTrigger,  Boolean,                                             Plain),
    LfoDivision    => ("lfo_division",     "Division",      OnTrigger,  Division,                                            Plain),
    LfoRetrigger   => ("lfo_retrigger",    "Retrigger",     OnTrigger,  Boolean,                                             Plain),
    LfoBipolar     => ("lfo_bipolar",      "Bipolar",       OnTrigger,  Boolean,                                             Plain),
    // Filter (39..46)
    FilterMode     => ("filter_mode",      "Mode",          OnTrigger,  (Choice FilterMode),                                 Plain),
    Cutoff         => ("cutoff",           "Cutoff",        Continuous, (Log 20.0, 20_000.0),                                Hz),
    Resonance      => ("resonance",        "Resonance",     Continuous, (Linear 0.0, 0.95),                                  Percent),
    Drive          => ("drive",            "Drive",         Continuous, (Linear 0.0, 1.0),                                   Percent),
    KeyTrack       => ("key_track",        "Key Track",     Continuous, (Linear 0.0, 1.0),                                   Percent),
    VelAmount      => ("vel_amount",       "Vel Mod",       Continuous, (Linear 0.0, 4.0),                                   Octaves),
    Mix            => ("mix",              "Mix",           Continuous, (Linear 0.0, 1.0),                                   Percent),
    // VCA (46..48)
    VcaGain        => ("vca_gain",         "Gain",          Continuous, (Linear 0.0, 2.0),                                   Percent),
    VcaBias        => ("vca_bias",         "Bias",          Continuous, (Linear 0.0, 1.0),                                   Percent),
    // Drive (48..52)
    DriveMode      => ("drive_mode",       "Mode",          OnTrigger,  (Choice DriveMode),                                  Plain),
    Asymmetry      => ("asymmetry",        "Asymmetry",     Continuous, (Linear -1.0, 1.0),                                  Percent),
    ToneHz         => ("tone_hz",          "Tone",          Continuous, (Log DRIVE_MIN_TONE_HZ, DRIVE_MAX_TONE_HZ),          Hz),
    OutputDb       => ("output_db",        "Output",        Continuous, (Linear DRIVE_MIN_OUTPUT_DB, DRIVE_MAX_OUTPUT_DB),   Db),
    // LoFi (52..54)
    Bits           => ("bits",             "Bits",          Continuous, (Linear LOFI_MIN_BITS, LOFI_MAX_BITS),               Bits),
    SampleRateHz   => ("sample_rate_hz",   "Clock",         Continuous, (Log LOFI_MIN_RATE_HZ, LOFI_MAX_RATE_HZ),            Hz),
    // Out (54..57)
    GainDb         => ("gain_db",          "Gain",          Continuous, (Linear -36.0, 12.0),                                Db),
    Pan            => ("pan",              "Pan",           Continuous, (Linear -1.0, 1.0),                                  Pan),
    Spread         => ("spread",           "Spread",        Continuous, (Linear 0.0, 1.0),                                   Percent),
    // Resonator (57..62)
    ResonatorMode  => ("resonator_mode",   "Mode",          OnTrigger,  (Choice ResonatorMode),                              Plain),
    Structure      => ("structure",        "Structure",     Continuous, (Linear 0.0, 1.0),                                   Percent),
    ResonatorDecay => ("resonator_decay",  "Decay",         Continuous, (Log RESONATOR_MIN_DECAY_MS, RESONATOR_MAX_DECAY_MS), Ms),
    Brightness     => ("brightness",       "Brightness",    Continuous, (Linear 0.0, 1.0),                                   Percent),
    Position       => ("position",         "Position",      Continuous, (Linear 0.0, 1.0),                                   Percent),
}

macro_rules! match_module_param_aliases {
    ($kind:expr, $key:expr, {
        $(
            $tag:ident => {
                $( $param:ident : $($alias:literal)|+ ),+ $(,)?
            }
        ),+ $(,)?
    }) => {
        match ($kind, $key) {
            $(
                $(
                    (ModuleKindTag::$tag, $($alias)|+) => Some(Self::$param),
                )+
            )+
            _ => None,
        }
    };
}

impl ModuleParam {
    #[inline]
    fn spec(self) -> &'static ModuleParamSpec {
        &MODULE_SPECS[self as usize]
    }

    #[inline]
    pub fn id(self) -> usize {
        self as usize
    }

    pub fn from_slot(id: usize) -> Option<Self> {
        Self::ALL.get(id).copied()
    }

    pub fn name(self) -> &'static str {
        self.spec().name
    }

    pub fn label(self) -> &'static str {
        self.spec().label
    }

    pub fn policy(self) -> UpdatePolicy {
        self.spec().policy
    }

    pub fn unit(self) -> Unit {
        self.spec().unit
    }

    pub fn is_automatable(self) -> bool {
        self.policy() != UpdatePolicy::Structural
    }

    pub fn is_frame_based(self) -> bool {
        matches!(self.spec().domain, ParamDomain::Frame)
    }

    pub fn is_discrete(self) -> bool {
        matches!(
            self.spec().domain,
            ParamDomain::Integer(..)
                | ParamDomain::Boolean
                | ParamDomain::Choice(_)
                | ParamDomain::Division
        )
    }

    pub fn range(self) -> (f32, f32) {
        self.spec().domain.range()
    }

    pub fn validate_native(self, value: f32) -> Result<()> {
        let (min, max) = self.range();
        ensure!(
            value.is_finite() && value >= min && value <= max,
            "{} must be finite and within {min}..={max}",
            self.label()
        );
        match self.spec().domain {
            ParamDomain::Integer(..) | ParamDomain::Boolean | ParamDomain::Choice(_) => {
                ensure!(
                    (value - value.round()).abs() < 1e-4,
                    "{} expects a whole value",
                    self.label()
                );
            }
            ParamDomain::Division => {
                ensure!(
                    LFO_DIVISIONS.iter().any(|&d| (d - value).abs() < 1e-4),
                    "LFO division is not a supported musical fraction"
                );
            }
            _ => {}
        }
        Ok(())
    }

    pub fn normalize(self, native: f32) -> f32 {
        self.spec().domain.normalize(native)
    }

    pub fn denormalize(self, norm: f32, sample_frames: usize) -> f32 {
        self.spec().domain.denormalize(norm, sample_frames)
    }

    pub fn format_native(self, v: f32, total_frames: usize) -> String {
        match self.spec().domain {
            ParamDomain::Boolean => return if v >= 0.5 { "On" } else { "Off" }.into(),
            ParamDomain::Choice(c) => return c.format(v.round().max(0.0) as usize).into(),
            ParamDomain::Division => {
                let i = LFO_DIVISIONS
                    .iter()
                    .position(|&d| (d - v).abs() < 1e-6)
                    .unwrap_or(0);
                return LFO_DIVISION_LABELS[i].into();
            }
            ParamDomain::Frame => {
                let frame = v.round().max(0.0) as usize;
                return if self == Self::SampleEnd && frame == 0 {
                    "End".into()
                } else if total_frames > 0 {
                    let pct = (frame.min(total_frames) as f32 / total_frames as f32) * 100.0;
                    format!("{frame} ({pct:.0}%)")
                } else {
                    format!("{frame}")
                };
            }
            _ => {}
        }
        match self.unit() {
            Unit::Percent => format!("{:.0}%", v * 100.0),
            Unit::Hz => {
                if self == Self::LfoRate {
                    format!("{v:.2} Hz")
                } else {
                    format_hz(v)
                }
            }
            Unit::Ms => format_ms(v),
            Unit::Semitones => format!("{v:+.1} st"),
            Unit::Cents => format!("{v:+.0} ct"),
            Unit::Degrees => format!("{v:.0}°"),
            Unit::Octaves => format!("{v:+.2} oct"),
            Unit::Note => {
                let midi = v.round().clamp(0.0, 127.0) as u8;
                format!("{} ({midi})", crate::music::note_name(midi))
            }
            Unit::Db => format_db(v),
            Unit::Pan => format_pan(v),
            Unit::Bits => {
                if v >= crate::dsp::lofi::LOFI_BYPASS_BITS {
                    "24 bit (Off)".into()
                } else {
                    format!("{v:.1} bit")
                }
            }
            Unit::Plain => format!("{v:.0}"),
        }
    }

    pub fn format_normalized(self, norm: f32, total_frames: usize) -> String {
        let n = if norm.is_finite() {
            norm.clamp(0.0, 1.0)
        } else {
            0.0
        };
        if self.is_frame_based() && total_frames == 0 {
            return format!("{:.0}%", n * 100.0);
        }
        self.format_native(self.denormalize(n, total_frames), total_frames)
    }

    /// Resolve a user-friendly parameter name in the context of a specific module kind.
    pub fn parse_for_kind(input: &str, kind: ModuleKindTag) -> Option<Self> {
        let key = normalize_choice_token(input);
        let candidate = match_module_param_aliases!(kind, key.as_str(), {
            Vco => {
                Waveform:      "wave" | "shape" | "waveform",
                Tune:          "tune" | "pitch" | "semi" | "semitones",
                Fine:          "fine" | "cents",
                PulseWidth:    "pw" | "pulse_width" | "width",
                Phase:         "phase" | "phase_deg",
                Level:         "level" | "gain" | "amp",
                VcoPitchTrack: "pitch_track" | "pitch_tracking" | "track",
                FixedHz:       "fixed" | "fixed_hz" | "freq" | "hz",
                UnisonVoices:  "unison" | "unison_voices" | "voices",
                UnisonDetune:  "detune" | "unison_detune",
                UnisonWidth:   "unison_width" | "spread",
            },
            Sampler => {
                RootMidi:    "root" | "root_midi" | "root_note",
                CoarseTune:  "coarse" | "coarse_tune" | "tune",
                Fine:        "fine" | "fine_tune" | "cents",
                SampleStart: "start" | "sample_start",
                SampleEnd:   "end" | "sample_end",
                LoopMode:    "loop" | "loop_mode",
                LoopStart:   "loop_start" | "lpstart",
                LoopEnd:     "loop_end" | "lpend",
                LoopXfade:   "xfade" | "loop_xfade",
                OneShot:     "one_shot" | "oneshot" | "trigger" | "trig",
                Level:       "level" | "gain",
            },
            Noise => {
                NoiseColor: "color" | "noise_color" | "type",
                HighpassHz: "hp" | "hpf" | "highpass" | "highpass_hz",
                LowpassHz:  "lp" | "lpf" | "lowpass" | "lowpass_hz",
                Level:      "level" | "gain",
            },
            Env => {
                EnvMode:        "mode" | "env_mode",
                Attack:         "attack" | "atk" | "attack_ms",
                Hold:           "hold" | "hold_ms",
                Decay:          "decay" | "dec" | "decay_ms",
                Sustain:        "sustain" | "sus",
                Release:        "release" | "rel" | "release_ms",
                TriggerMode:    "trigger" | "trig" | "trigger_mode",
                DelayMs:        "delay" | "pre_delay" | "predelay" | "delay_ms",
                Bursts:         "bursts" | "burst" | "count",
                BurstSpacingMs: "spacing" | "burst_spacing" | "burst_spacing_ms",
            },
            Lfo => {
                LfoShape:     "shape" | "wave" | "lfo_shape",
                LfoRate:      "rate" | "freq" | "hz" | "lfo_rate",
                LfoSync:      "sync" | "lfo_sync",
                LfoDivision:  "div" | "division" | "lfo_division",
                LfoRetrigger: "retrig" | "retrigger" | "lfo_retrigger",
                LfoBipolar:   "bipolar" | "lfo_bipolar",
            },
            Filter => {
                FilterMode: "mode" | "filter_mode" | "type",
                Cutoff:     "cutoff" | "freq" | "hz" | "cutoff_hz",
                Resonance:  "res" | "reso" | "resonance",
                Drive:      "drive" | "sat",
                KeyTrack:   "key" | "key_track" | "key_tracking" | "keytrack",
                VelAmount:  "vel" | "vel_amount" | "vel_mod" | "accent",
                Mix:        "mix" | "wet",
            },
            Vca => {
                VcaGain: "gain" | "level" | "vca_gain",
                VcaBias: "bias" | "offset" | "vca_bias",
            },
            Drive => {
                DriveMode: "mode" | "drive_mode" | "type",
                Drive:     "drive" | "amount",
                Asymmetry: "asym" | "asymmetry" | "bias",
                ToneHz:    "tone" | "tone_hz",
                OutputDb:  "out" | "output" | "output_db" | "gain",
                Mix:       "mix" | "wet",
            },
            LoFi => {
                Bits:         "bits" | "depth" | "bit_depth" | "crush",
                SampleRateHz: "rate" | "rate_hz" | "sample_rate" | "sample_rate_hz" | "clock" | "sr",
                Mix:          "mix" | "wet",
            },
            Resonator => {
                ResonatorMode:  "mode" | "resonator_mode" | "type" | "body",
                VcoPitchTrack:  "track" | "tracking" | "pitch_track" | "pitch_tracking" | "vco_pitch_track",
                Tune:           "tune" | "semi" | "semitones" | "pitch",
                Fine:           "fine" | "cents",
                FixedHz:        "freq" | "hz" | "fixed" | "fixed_hz",
                Structure:      "structure" | "struc" | "inharm" | "stiffness",
                ResonatorDecay: "decay" | "decay_ms" | "resonator_decay" | "rt60",
                Brightness:     "bright" | "brightness" | "damp" | "damping" | "tone",
                Position:       "pos" | "position" | "strike" | "pluck",
                Mix:            "mix" | "wet",
            },
            Out => {
                GainDb: "gain" | "gain_db" | "level",
                Pan:    "pan",
                Spread: "spread" | "width",
            },
        });
        if candidate.is_some() {
            return candidate;
        }
        kind.params()
            .iter()
            .copied()
            .find(|p| p.name() == key || normalize_choice_token(p.label()) == key)
    }

    pub fn parse_value_native(self, input: &str) -> Result<f32> {
        let raw = input.trim();
        match self.spec().domain {
            ParamDomain::Boolean => return parse_bool_text(raw, self.label()),
            ParamDomain::Choice(c) => {
                let idx = c
                    .parse(raw)
                    .with_context(|| format!("Invalid option '{raw}' for {}", self.label()))?;
                return Ok(idx as f32);
            }
            ParamDomain::Division => {
                let i = LFO_DIVISION_LABELS
                    .iter()
                    .position(|&x| x == raw)
                    .context("LFO Division expects a musical fraction such as 1/8 or 1/4")?;
                return Ok(LFO_DIVISIONS[i]);
            }
            ParamDomain::Frame => {
                let frames = raw
                    .parse::<usize>()
                    .with_context(|| format!("{} expects a frame number", self.label()))?;
                return Ok(frames as f32);
            }
            _ => {}
        }

        match self.unit() {
            Unit::Pan => return parse_pan(raw),
            Unit::Note => return parse_note_text(raw),
            Unit::Db => {
                let v = parse_db(raw)?;
                self.validate_native(v)?;
                return Ok(v);
            }
            Unit::Bits => {
                let compact = raw.to_ascii_lowercase().replace(' ', "");
                let stripped = compact
                    .strip_suffix("bits")
                    .or_else(|| compact.strip_suffix("bit"))
                    .unwrap_or(&compact);
                let v: f32 = stripped
                    .parse()
                    .with_context(|| format!("Invalid value for {}", self.label()))?;
                self.validate_native(v)?;
                return Ok(v);
            }
            _ => {}
        }

        let compact = raw.to_ascii_lowercase().replace(' ', "");
        const UNITS: [(&str, f32, Unit); 11] = [
            ("cents", 1.0, Unit::Cents),
            ("khz", 1000.0, Unit::Hz),
            ("deg", 1.0, Unit::Degrees),
            ("oct", 1.0, Unit::Octaves),
            ("hz", 1.0, Unit::Hz),
            ("ms", 1.0, Unit::Ms),
            ("st", 1.0, Unit::Semitones),
            ("ct", 1.0, Unit::Cents),
            ("°", 1.0, Unit::Degrees),
            ("s", 1000.0, Unit::Ms),
            ("%", 0.01, Unit::Percent),
        ];
        let mut number = compact.as_str();
        let mut scale = 1.0;
        for (suffix, multiplier, unit) in UNITS {
            if let Some(v) = compact.strip_suffix(suffix) {
                ensure!(unit == self.unit(), "Wrong unit for {}", self.label());
                number = v;
                scale = multiplier;
                break;
            }
        }
        let value = number
            .parse::<f32>()
            .with_context(|| format!("Invalid value for {}", self.label()))?
            * scale;
        self.validate_native(value)?;
        Ok(value)
    }

    pub fn parse_value(self, input: &str) -> Result<f32> {
        Ok(self.normalize(self.parse_value_native(input)?))
    }

    pub fn nudge_native(self, value: f32, dir: i8, coarse: bool) -> f32 {
        let (min, max) = self.range();
        let d = f32::from(dir);
        let next = match self.spec().domain {
            ParamDomain::Choice(_) | ParamDomain::Boolean | ParamDomain::Integer(..) => {
                let step = if coarse && matches!(self, Self::RootMidi | Self::CoarseTune) {
                    12.0
                } else {
                    1.0
                };
                value.round() + d * step
            }
            ParamDomain::Division => {
                let i = LFO_DIVISIONS
                    .iter()
                    .position(|&v| (v - value).abs() < 1e-6)
                    .unwrap_or(0) as i32;
                let next_i = (i + i32::from(dir)).clamp(0, LFO_DIVISIONS.len() as i32 - 1) as usize;
                LFO_DIVISIONS[next_i]
            }
            ParamDomain::Log(_, _) => nudge_log(value, dir, 1.10, 1.5, coarse),
            ParamDomain::Frame => {
                let step = if coarse { 256.0 } else { 16.0 };
                (value + d * step).max(0.0)
            }
            ParamDomain::Linear(_, _) => match self.unit() {
                Unit::Semitones => nudge_quantized(value, dir, 1.0, 1.0, 12.0, coarse),
                Unit::Cents => nudge_quantized(value, dir, 1.0, 1.0, 10.0, coarse),
                Unit::Degrees => {
                    let step = if coarse { 45.0 } else { 5.0 };
                    (value / step).round() * step + d * step
                }
                Unit::Percent => nudge_quantized(value, dir, 100.0, 1.0, 5.0, coarse),
                Unit::Pan => nudge_quantized(value, dir, 50.0, 1.0, 5.0, coarse),
                Unit::Octaves => nudge_quantized(value, dir, 20.0, 1.0, 10.0, coarse),
                Unit::Db | Unit::Bits => nudge_quantized(value, dir, 2.0, 1.0, 4.0, coarse),
                Unit::Ms => {
                    if matches!(self, Self::Hold | Self::BurstSpacingMs) {
                        nudge_quantized(value, dir, 2.0, 1.0, 10.0, coarse)
                    } else {
                        nudge_quantized(value, dir, 1.0, 5.0, 50.0, coarse)
                    }
                }
                _ => {
                    let step = (max - min) * if coarse { 0.05 } else { 0.01 };
                    value + d * step
                }
            },
        };
        next.clamp(min, max)
    }
}

fn parse_bool_text(raw: &str, label: &str) -> Result<f32> {
    match normalize_choice_token(raw).as_str() {
        "on" | "true" | "1" | "yes" | "one_shot" | "oneshot" => Ok(1.0),
        "off" | "false" | "0" | "no" | "gate" => Ok(0.0),
        _ => Err(anyhow!("{label} expects on/off")),
    }
}

fn parse_note_text(raw: &str) -> Result<f32> {
    let trimmed = raw.trim();
    if let Ok(midi) = trimmed.parse::<u8>() {
        ensure!(midi <= 127, "MIDI note must be within 0..=127");
        Ok(f32::from(midi))
    } else {
        let midi = crate::music::parse_root_midi(trimmed)
            .context("Expected a note name such as C4 or MIDI 0..127")?;
        Ok(f32::from(midi))
    }
}
