//! Effect parameter metadata, units, normalization, formatting, and value parsing.

use super::{DelayMode, DriveMode, EffectKind};
use crate::filter_patterns::{
    filter_env_preset_name, find_filter_groove_index, FILTER_ENV_PATTERN_COUNT,
};
use crate::params::{
    format_choice_index, format_db, format_hz, log_to_norm, norm_to_log, normalize_choice_token,
    parse_choice_index, parse_db, parse_hz, parse_ms, parse_percent_or_unit,
};
use crate::rack::FilterMode;
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ParameterUnit {
    SixteenthSteps,
    Ratio,
    BipolarRatio,
    Seconds,
    Milliseconds,
    Hertz,
    Decibels,
    FilterMode,
    DelayMode,
    DriveMode,
    CompressionRatio,
    Octaves,
    FilterPattern,
    QFactor,
}

#[derive(Clone, Copy)]
struct EffectParamSpec {
    name: &'static str,
    label: &'static str,
    min: f32,
    max: f32,
    unit: ParameterUnit,
    log: bool,
}

macro_rules! define_effect_params {
    (
        $(
            $variant:ident => (
                $name:literal,
                $label:literal,
                $min:expr,
                $max:expr,
                $unit:ident,
                $scale:ident
                $(, [ $($alias:literal),* $(,)? ])?
            )
        ),+ $(,)?
    ) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        #[repr(u8)]
        pub enum EffectParam {
            $( $variant, )+
        }

        const EFFECT_SPECS: [EffectParamSpec; [ $( EffectParam::$variant ),+ ].len()] = [
            $(
                EffectParamSpec {
                    name: $name,
                    label: $label,
                    min: $min,
                    max: $max,
                    unit: ParameterUnit::$unit,
                    log: define_effect_params!(@is_log $scale),
                },
            )+
        ];

        impl EffectParam {
            pub const ALL: [Self; [ $( Self::$variant ),+ ].len()] = [
                $( Self::$variant, )+
            ];

            pub fn parse(input: &str) -> Option<Self> {
                match normalize_choice_token(input).as_str() {
                    $(
                        $name $( $( | $alias )* )? => Some(Self::$variant),
                    )+
                    _ => None,
                }
            }
        }
    };
    (@is_log lin) => { false };
    (@is_log log) => { true };
}

define_effect_params! {
    Time       => ("time",        "Time",       0.5,    32.0,                               SixteenthSteps,   lin, ["steps"]),
    Feedback   => ("feedback",    "Feedback",   0.0,    0.88,                               Ratio,            lin, ["fb"]),
    Mix        => ("mix",         "Mix",        0.0,    1.0,                                Ratio,            lin, ["wet"]),
    Size       => ("size",        "Size",       0.0,    1.0,                                Ratio,            lin, ["room"]),
    Decay      => ("decay",       "Decay",      0.2,    12.0,                               Seconds,          lin),
    Damping    => ("damping",     "Damping",    500.0,  18000.0,                            Hertz,            log),
    Predelay   => ("predelay",    "Predelay",   0.0,    120.0,                              Milliseconds,     lin),
    Width      => ("width",       "Width",      0.0,    1.0,                                Ratio,            lin),
    Hpf        => ("hpf",         "HPF",        20.0,   600.0,                              Hertz,            log),
    Low        => ("low",         "Low",        -12.0,  12.0,                               Decibels,         lin),
    Mid        => ("mid",         "Mid",        -12.0,  12.0,                               Decibels,         lin),
    High       => ("high",        "High",       -12.0,  12.0,                               Decibels,         lin),
    Rate       => ("rate",        "Rate",       0.05,   6.0,                                Hertz,            log),
    Depth      => ("depth",       "Depth",      0.0,    1.0,                                Ratio,            lin),
    Drive      => ("drive",       "Drive",      0.0,    1.0,                                Ratio,            lin),
    Tone       => ("tone",        "Tone",       600.0,  18000.0,                            Hertz,            log),
    Output     => ("output",      "Output",     -12.0,  6.0,                                Decibels,         lin),
    FilterMode => ("mode",        "Mode",       0.0,    (FilterMode::ALL.len() - 1) as f32, FilterMode,       lin, ["filter_mode", "filtermode"]),
    Cutoff     => ("cutoff",      "Cutoff",     20.0,   18000.0,                            Hertz,            log, ["freq", "frequency"]),
    Resonance  => ("resonance",   "Resonance",  0.0,    0.95,                               Ratio,            lin, ["res", "q"]),
    Threshold  => ("threshold",   "Threshold",  -48.0,  0.0,                                Decibels,         lin, ["thresh"]),
    Ratio      => ("ratio",       "Ratio",      1.0,    20.0,                               CompressionRatio, log),
    Attack     => ("attack",      "Attack",     0.5,    100.0,                              Milliseconds,     log),
    Release    => ("release",     "Release",    10.0,   1000.0,                             Milliseconds,     log),
    Makeup     => ("makeup",      "Makeup",     -6.0,   24.0,                               Decibels,         lin, ["gain"]),
    EnvPattern => ("env_pattern", "Pattern",    0.0,    (FILTER_ENV_PATTERN_COUNT - 1) as f32, FilterPattern, lin, ["pattern", "pat", "wave", "groove"]),
    EnvAmount  => ("env_amount",  "Env Amount", -4.0,   4.0,                                Octaves,          lin, ["amount", "amt", "env", "env_amt"]),
    EnvDecay   => ("env_decay",   "Env Decay",  15.0,   1500.0,                             Milliseconds,     log, ["env_dec", "decay_ms"]),
    MidFreq    => ("mid_hz",      "Mid Freq",   160.0,  6000.0,                             Hertz,            log, ["mid_freq", "midfreq"]),
    MidQ       => ("mid_q",       "Mid Q",      0.3,    8.0,                                QFactor,          log, ["midq"]),
    Ceiling    => ("ceiling",     "Ceiling",    -18.0,  -0.1,                               Decibels,         lin, ["ceil"]),
    InputDrive => ("input_drive", "In Drive",   0.0,    24.0,                               Decibels,         lin, ["in_drive", "indrive"]),
    DelayMode  => ("delay_mode",  "Mode",       0.0,    (DelayMode::ALL.len() - 1) as f32,  DelayMode,        lin, ["delaymode"]),
    DriveMode  => ("drive_mode",  "Mode",       0.0,    (DriveMode::ALL.len() - 1) as f32,  DriveMode,        lin, ["drivemode"]),
    Asymmetry  => ("asymmetry",   "Asym",       -1.0,   1.0,                                BipolarRatio,     lin, ["asym", "bias", "symmetry", "sym"]),
}

impl EffectParam {
    pub const DELAY: [Self; 4] = [Self::DelayMode, Self::Time, Self::Feedback, Self::Mix];
    pub const REVERB: [Self; 6] = [
        Self::Size,
        Self::Decay,
        Self::Damping,
        Self::Predelay,
        Self::Mix,
        Self::Width,
    ];
    pub const EQ: [Self; 6] = [
        Self::Hpf,
        Self::Low,
        Self::Mid,
        Self::MidFreq,
        Self::MidQ,
        Self::High,
    ];
    pub const CHORUS: [Self; 4] = [Self::Rate, Self::Depth, Self::Width, Self::Mix];
    pub const DRIVE: [Self; 6] = [
        Self::DriveMode,
        Self::Drive,
        Self::Asymmetry,
        Self::Tone,
        Self::Output,
        Self::Mix,
    ];
    pub const FILTER: [Self; 8] = [
        Self::FilterMode,
        Self::Cutoff,
        Self::Resonance,
        Self::Drive,
        Self::Mix,
        Self::EnvPattern,
        Self::EnvAmount,
        Self::EnvDecay,
    ];
    pub const COMPRESSOR: [Self; 6] = [
        Self::Threshold,
        Self::Ratio,
        Self::Attack,
        Self::Release,
        Self::Makeup,
        Self::Mix,
    ];
    pub const LIMITER: [Self; 4] = [Self::Ceiling, Self::InputDrive, Self::Release, Self::Mix];

    #[inline]
    fn spec(self) -> &'static EffectParamSpec {
        &EFFECT_SPECS[self as usize]
    }

    pub fn name(self) -> &'static str {
        self.spec().name
    }

    pub fn label(self) -> &'static str {
        self.spec().label
    }

    pub fn is_discrete(self) -> bool {
        matches!(
            self.unit(),
            ParameterUnit::FilterMode
                | ParameterUnit::DelayMode
                | ParameterUnit::DriveMode
                | ParameterUnit::FilterPattern
        )
    }

    pub fn parse_for_kind(kind: &EffectKind, input: &str) -> Option<Self> {
        let key = normalize_choice_token(input);
        if key == "mode" {
            return match kind {
                EffectKind::Filter(_) => Some(Self::FilterMode),
                EffectKind::Delay(_) => Some(Self::DelayMode),
                EffectKind::Drive(_) => Some(Self::DriveMode),
                _ => None,
            };
        }
        Self::parse(&key)
    }

    pub fn range(self) -> (f32, f32) {
        let s = self.spec();
        (s.min, s.max)
    }

    pub fn unit(self) -> ParameterUnit {
        self.spec().unit
    }

    pub fn normalize(self, value: f32) -> f32 {
        let s = self.spec();
        if s.log {
            log_to_norm(value, s.min, s.max)
        } else {
            ((value - s.min) / (s.max - s.min)).clamp(0.0, 1.0)
        }
    }

    pub fn denormalize(self, value: f32) -> f32 {
        let s = self.spec();
        if s.log {
            norm_to_log(value, s.min, s.max)
        } else {
            s.min + value.clamp(0.0, 1.0) * (s.max - s.min)
        }
    }

    /// Allocation-free application to a prepared copy of a validated device patch.
    pub fn write_kind(self, kind: &mut EffectKind, n: f32) {
        self.write_kind_native(kind, self.denormalize(n));
    }

    pub(crate) fn write_kind_native(self, kind: &mut EffectKind, value: f32) {
        crate::dispatch_effect_kind!(kind, p => p.write_native(self, value));
    }

    pub fn format_normalized(self, n: f32) -> String {
        self.format_native(self.denormalize(n))
    }

    pub fn format_native(self, v: f32) -> String {
        match self.unit() {
            ParameterUnit::SixteenthSteps => format!("{v:.2} steps"),
            ParameterUnit::Ratio => format!("{:.0}%", v * 100.0),
            ParameterUnit::BipolarRatio => {
                if v.abs() < 0.005 {
                    "0%".into()
                } else {
                    format!("{:+.0}%", v * 100.0)
                }
            }
            ParameterUnit::Seconds => format!("{v:.2} s"),
            ParameterUnit::Milliseconds => format!("{v:.1} ms"),
            ParameterUnit::Hertz => format_hz(v),
            ParameterUnit::Decibels => format_db(v),
            ParameterUnit::FilterMode => format_choice_index::<FilterMode>(v).into(),
            ParameterUnit::DelayMode => format_choice_index::<DelayMode>(v).into(),
            ParameterUnit::DriveMode => format_choice_index::<DriveMode>(v).into(),
            ParameterUnit::CompressionRatio => format!("{v:.1}:1"),
            ParameterUnit::Octaves => {
                if v.abs() < 0.02 {
                    "Off".into()
                } else {
                    format!("{v:+.2} oct")
                }
            }
            ParameterUnit::FilterPattern => {
                let idx = v.round().clamp(0.0, (FILTER_ENV_PATTERN_COUNT - 1) as f32) as u8;
                filter_env_preset_name(idx)
            }
            ParameterUnit::QFactor => format!("{v:.2}"),
        }
    }

    pub fn parse_value_native(self, input: &str) -> Result<f32> {
        let raw = input.trim().to_ascii_lowercase();
        let value = match self.unit() {
            ParameterUnit::SixteenthSteps => {
                if let Some((numerator, denominator)) = raw.split_once('/') {
                    let numerator = numerator
                        .trim()
                        .parse::<u32>()
                        .context("Delay fraction expects numerator/denominator, such as 3/16")?;
                    let denominator = denominator
                        .trim()
                        .parse::<u32>()
                        .context("Delay fraction expects numerator/denominator, such as 3/16")?;
                    ensure!(
                        denominator != 0,
                        "Delay fraction denominator must be positive"
                    );
                    // Fractions are note values: one whole note contains 16 steps.
                    numerator as f32 * 16.0 / denominator as f32
                } else {
                    raw.strip_suffix("steps")
                        .or_else(|| raw.strip_suffix("step"))
                        .unwrap_or(&raw)
                        .trim()
                        .parse::<f32>()
                        .context(
                            "Delay time expects sixteenth-note steps or a note fraction such as 1/8",
                        )?
                }
            }
            ParameterUnit::Ratio => parse_percent_or_unit(&raw)?,
            ParameterUnit::BipolarRatio => {
                if matches!(raw.as_str(), "sym" | "center" | "off") {
                    0.0
                } else {
                    parse_percent_or_unit(&raw)?
                }
            }
            ParameterUnit::Seconds => {
                if let Some(v) = raw.strip_suffix("ms") {
                    v.trim().parse::<f32>()? / 1000.0
                } else {
                    raw.strip_suffix('s').unwrap_or(&raw).trim().parse()?
                }
            }
            ParameterUnit::Milliseconds => parse_ms(&raw)?,
            ParameterUnit::Hertz => parse_hz(&raw)?,
            ParameterUnit::Decibels => parse_db(&raw)?,
            ParameterUnit::FilterMode => parse_choice_index::<FilterMode>(
                &raw,
                "Filter mode expects lp / bp / hp / acid / lp6",
            )?,
            ParameterUnit::DelayMode => {
                parse_choice_index::<DelayMode>(&raw, "Delay mode expects ping_pong / stereo")?
            }
            ParameterUnit::DriveMode => {
                parse_choice_index::<DriveMode>(&raw, "Drive mode expects tape / hard_clip / fold")?
            }
            ParameterUnit::CompressionRatio => {
                raw.strip_suffix(":1").unwrap_or(&raw).trim().parse()?
            }
            ParameterUnit::Octaves => {
                if matches!(raw.as_str(), "off" | "none") {
                    0.0
                } else {
                    raw.strip_suffix("oct")
                        .or_else(|| raw.strip_suffix("octaves"))
                        .unwrap_or(&raw)
                        .trim()
                        .parse()?
                }
            }
            ParameterUnit::FilterPattern => {
                if let Ok(n) = raw.parse::<u8>() {
                    f32::from(n)
                } else if let Some(id) = find_filter_groove_index(&raw) {
                    f32::from(id)
                } else {
                    bail!(
                        "Filter pattern expects 0..={} or a groove name",
                        FILTER_ENV_PATTERN_COUNT - 1
                    );
                }
            }
            ParameterUnit::QFactor => {
                let trimmed = raw.strip_prefix('q').unwrap_or(&raw);
                trimmed
                    .strip_suffix('q')
                    .unwrap_or(trimmed)
                    .trim()
                    .parse()?
            }
        };
        let (lo, hi) = self.range();
        ensure!(
            value.is_finite() && (lo..=hi).contains(&value),
            "{} must be within {lo}..{hi}",
            self.label()
        );
        Ok(value)
    }

    pub fn parse_value(self, input: &str) -> Result<f32> {
        Ok(self.normalize(self.parse_value_native(input)?))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EffectParamDescriptor {
    pub param: EffectParam,
    pub name: &'static str,
    pub unit: ParameterUnit,
    pub native_min: f32,
    pub native_max: f32,
    pub native_default: f32,
    pub native_value: f32,
    pub normalized: f32,
    pub formatted: String,
    pub automatable: bool,
    pub per_note_automation: bool,
}
