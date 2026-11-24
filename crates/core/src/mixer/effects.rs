//! Effect mode enums, insert patch structs, `EffectKind`, `EffectDevice`, and `dispatch_effect_kind!`.

use super::validate::validate_name;
use super::{EffectParam, EffectParamDescriptor, ParameterUnit};
use crate::filter_patterns::{step_filter_groove_id, FILTER_ENV_PATTERN_COUNT};
use crate::params::{nudge_log, nudge_quantized};
use crate::rack::FilterMode;
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};

crate::define_choice_enum! {
    /// Stereo routing topology for [`DelayPatch`] and [`crate::dsp::Delay`].
    #[derive(Default)]
    pub enum DelayMode [array] {
        #[default]
        PingPong => ("ping_pong", "Ping-Pong", ["0", "pingpong", "pp", "ping"]),
        Stereo => ("stereo", "Stereo", ["1", "dual", "st"]),
    }
}

crate::define_choice_enum! {
    /// Waveshaper circuit mode for [`DrivePatch`] and [`crate::dsp::Drive`].
    #[derive(Default)]
    pub enum DriveMode [array] {
        #[default]
        Tape => ("tape", "Tape", ["0", "tube", "warm", "soft", "analog"]),
        HardClip => ("hard_clip", "Hard Clip", ["1", "hardclip", "hard", "clip", "dist", "diode", "fuzz"]),
        Fold => ("fold", "Fold", ["2", "wavefold", "wave_fold", "west", "west_coast"]),
    }
}

crate::define_choice_enum! {
    /// Fieldless discriminant tag for all 8 native [`EffectKind`] insert devices.
    pub enum EffectKindTag {
        Delay => ("delay", "Delay"),
        Reverb => ("reverb", "Reverb"),
        Eq => ("eq", "EQ"),
        Chorus => ("chorus", "Chorus"),
        Drive => ("drive", "Drive", ["sat", "saturator"]),
        Filter => ("filter", "Filter", ["pcf"]),
        Compressor => ("compressor", "Compressor", ["comp"]),
        Limiter => ("limiter", "Limiter", ["lim"]),
    }
}

impl EffectKindTag {
    /// Short 2–3 character badge for compact UI nodes (e.g. Signal Graph).
    pub fn short_tag(self) -> &'static str {
        match self {
            Self::Delay => "DLY",
            Self::Reverb => "RVB",
            Self::Eq => "EQ",
            Self::Chorus => "CHR",
            Self::Drive => "DRV",
            Self::Filter => "FLT",
            Self::Compressor => "CMP",
            Self::Limiter => "LIM",
        }
    }

    /// Ordered native parameter list for this effect kind.
    pub fn params(self) -> &'static [EffectParam] {
        match self {
            Self::Delay => &EffectParam::DELAY,
            Self::Reverb => &EffectParam::REVERB,
            Self::Eq => &EffectParam::EQ,
            Self::Chorus => &EffectParam::CHORUS,
            Self::Drive => &EffectParam::DRIVE,
            Self::Filter => &EffectParam::FILTER,
            Self::Compressor => &EffectParam::COMPRESSOR,
            Self::Limiter => &EffectParam::LIMITER,
        }
    }

    pub fn default_kind(self) -> EffectKind {
        match self {
            Self::Delay => EffectKind::Delay(DelayPatch::default()),
            Self::Reverb => EffectKind::Reverb(ReverbPatch::default()),
            Self::Eq => EffectKind::Eq(EqPatch::default()),
            Self::Chorus => EffectKind::Chorus(ChorusPatch::default()),
            Self::Drive => EffectKind::Drive(DrivePatch::default()),
            Self::Filter => EffectKind::Filter(FilterPatch::default()),
            Self::Compressor => EffectKind::Compressor(CompressorPatch::default()),
            Self::Limiter => EffectKind::Limiter(LimiterPatch::default()),
        }
    }

    pub fn default_device(self, id: u64) -> EffectDevice {
        EffectDevice {
            id,
            name: self.label().into(),
            bypassed: false,
            kind: self.default_kind(),
        }
    }
}

/// Exhaustively dispatch over all 8 [`crate::mixer::EffectKind`] variants without
/// duplicating 8-arm match boilerplate across `cadence-core` and `cadence-engine`.
///
/// Forms:
/// - Uniform inner-patch dispatch: `dispatch_effect_kind!(kind_expr, patch => body)`
/// - Discriminant projection: `dispatch_effect_kind!(kind_expr, @tag TargetEnum)`
#[macro_export]
macro_rules! dispatch_effect_kind {
    ($kind:expr, @tag $tag:ty) => {
        match $kind {
            $crate::mixer::EffectKind::Delay(_) => <$tag>::Delay,
            $crate::mixer::EffectKind::Reverb(_) => <$tag>::Reverb,
            $crate::mixer::EffectKind::Eq(_) => <$tag>::Eq,
            $crate::mixer::EffectKind::Chorus(_) => <$tag>::Chorus,
            $crate::mixer::EffectKind::Drive(_) => <$tag>::Drive,
            $crate::mixer::EffectKind::Filter(_) => <$tag>::Filter,
            $crate::mixer::EffectKind::Compressor(_) => <$tag>::Compressor,
            $crate::mixer::EffectKind::Limiter(_) => <$tag>::Limiter,
        }
    };
    ($kind:expr, $patch:pat => $body:expr) => {
        match $kind {
            $crate::mixer::EffectKind::Delay($patch) => $body,
            $crate::mixer::EffectKind::Reverb($patch) => $body,
            $crate::mixer::EffectKind::Eq($patch) => $body,
            $crate::mixer::EffectKind::Chorus($patch) => $body,
            $crate::mixer::EffectKind::Drive($patch) => $body,
            $crate::mixer::EffectKind::Filter($patch) => $body,
            $crate::mixer::EffectKind::Compressor($patch) => $body,
            $crate::mixer::EffectKind::Limiter($patch) => $body,
        }
    };
}

fn validate_patch_params(kind: &str, entries: &[(EffectParam, f32)]) -> Result<()> {
    for &(param, value) in entries {
        let (lo, hi) = param.range();
        ensure!(
            value.is_finite() && (lo..=hi).contains(&value),
            "{kind} {} must be finite and within {lo}..{hi}",
            param.name()
        );
    }
    Ok(())
}

/// Generate `validate`, `native_value`, and `write_native` for an effect patch struct
/// from a single authoritative parameter-to-field table.
macro_rules! impl_effect_patch {
    ($patch:ident, $label:literal, {
        $( $kind:ident $param:ident => $field:ident $( : $ty:ty )? ),+ $(,)?
    }) => {
        impl $patch {
            pub fn validate(&self) -> Result<()> {
                validate_patch_params(
                    $label,
                    &[ $( (EffectParam::$param, impl_effect_patch!(@read $kind, self.$field)) ),+ ],
                )
            }

            pub(super) fn native_value(&self, param: EffectParam) -> Option<f32> {
                match param {
                    $( EffectParam::$param => Some(impl_effect_patch!(@read $kind, self.$field)), )+
                    _ => None,
                }
            }

            pub(super) fn write_native(&mut self, param: EffectParam, value: f32) {
                match param {
                    $( EffectParam::$param => impl_effect_patch!(@write $kind, self.$field, value $(, $ty)?), )+
                    _ => {}
                }
            }
        }
    };
    (@read f32, $field:expr) => { $field };
    (@read choice, $field:expr) => { $field.to_index() as f32 };
    (@read groove_u8, $field:expr) => { f32::from($field) };

    (@write f32, $field:expr, $val:expr) => { $field = $val };
    (@write choice, $field:expr, $val:expr, $ty:ty) => {
        $field = <$ty>::from_index($val.round().max(0.0) as usize)
    };
    (@write groove_u8, $field:expr, $val:expr) => {
        $field = $val
            .round()
            .clamp(0.0, (FILTER_ENV_PATTERN_COUNT - 1) as f32) as u8
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DelayPatch {
    pub mode: DelayMode,
    pub steps: f32,
    pub feedback: f32,
    pub mix: f32,
}

impl Default for DelayPatch {
    fn default() -> Self {
        Self {
            mode: DelayMode::PingPong,
            steps: 3.0,
            feedback: 0.30,
            mix: 1.0,
        }
    }
}

impl_effect_patch!(DelayPatch, "Delay", {
    choice DelayMode => mode: DelayMode,
    f32    Time      => steps,
    f32    Feedback  => feedback,
    f32    Mix       => mix,
});

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EqPatch {
    pub hpf_hz: f32,
    pub low_db: f32,
    pub mid_db: f32,
    pub mid_hz: f32,
    pub mid_q: f32,
    pub high_db: f32,
}

impl Default for EqPatch {
    fn default() -> Self {
        Self {
            hpf_hz: 20.0,
            low_db: 0.0,
            mid_db: 0.0,
            mid_hz: 1000.0,
            mid_q: 0.8,
            high_db: 0.0,
        }
    }
}

impl_effect_patch!(EqPatch, "EQ", {
    f32 Hpf     => hpf_hz,
    f32 Low     => low_db,
    f32 Mid     => mid_db,
    f32 MidFreq => mid_hz,
    f32 MidQ    => mid_q,
    f32 High    => high_db,
});

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChorusPatch {
    pub rate_hz: f32,
    pub depth: f32,
    pub width: f32,
    pub mix: f32,
}

impl Default for ChorusPatch {
    fn default() -> Self {
        Self {
            rate_hz: 0.55,
            depth: 0.45,
            width: 0.85,
            mix: 0.40,
        }
    }
}

impl_effect_patch!(ChorusPatch, "Chorus", {
    f32 Rate  => rate_hz,
    f32 Depth => depth,
    f32 Width => width,
    f32 Mix   => mix,
});

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DrivePatch {
    pub mode: DriveMode,
    pub drive: f32,
    pub asymmetry: f32,
    pub tone_hz: f32,
    pub output_db: f32,
    pub mix: f32,
}

impl Default for DrivePatch {
    fn default() -> Self {
        Self {
            mode: DriveMode::Tape,
            drive: 0.35,
            asymmetry: 0.0,
            tone_hz: 7500.0,
            output_db: 0.0,
            mix: 1.0,
        }
    }
}

impl_effect_patch!(DrivePatch, "Drive", {
    choice DriveMode => mode: DriveMode,
    f32    Drive     => drive,
    f32    Asymmetry => asymmetry,
    f32    Tone      => tone_hz,
    f32    Output    => output_db,
    f32    Mix       => mix,
});

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReverbPatch {
    pub size: f32,
    pub decay_seconds: f32,
    pub damping_hz: f32,
    pub predelay_ms: f32,
    pub mix: f32,
    pub width: f32,
}

impl Default for ReverbPatch {
    fn default() -> Self {
        Self {
            size: 0.55,
            decay_seconds: 2.4,
            damping_hz: 6000.0,
            predelay_ms: 12.0,
            mix: 0.25,
            width: 0.8,
        }
    }
}

impl_effect_patch!(ReverbPatch, "Reverb", {
    f32 Size     => size,
    f32 Decay    => decay_seconds,
    f32 Damping  => damping_hz,
    f32 Predelay => predelay_ms,
    f32 Mix      => mix,
    f32 Width    => width,
});

/// Stereo resonant multi-mode insert filter (LP / BP / HP / Acid LP) with an
/// optional rhythmic step envelope (external multi-groove bank `00..=127` or any
/// bound project `Pattern`) and smooth parameter automation.
///
/// When `env_amount == 0.0` (default), behaves identically to a static studio filter.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilterPatch {
    pub mode: FilterMode,
    pub cutoff_hz: f32,
    pub resonance: f32,
    pub drive: f32,
    pub mix: f32,
    /// External rhythmic envelope groove ID (`0..=127`). Used when `env_pattern_id` is `None`.
    pub env_pattern: u8,
    /// Optional project `Pattern` ID (`Notes`, `Rhythm`, `Mini`, `Euclid`, `Values`, etc.)
    /// driving the filter's rhythmic envelope directly from authored Cadence material.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_pattern_id: Option<u64>,
    /// Bipolar rhythmic envelope depth in octaves (`-4.0..=+4.0`). `0.0` = static filter.
    pub env_amount: f32,
    /// Rhythmic envelope exponential decay time in milliseconds (`15.0..=1500.0`).
    pub env_decay_ms: f32,
}

impl Default for FilterPatch {
    fn default() -> Self {
        Self {
            mode: FilterMode::LowPass,
            cutoff_hz: 1600.0,
            resonance: 0.35,
            drive: 0.0,
            mix: 1.0,
            env_pattern: 0,
            env_pattern_id: None,
            env_amount: 0.0,
            env_decay_ms: 240.0,
        }
    }
}

impl_effect_patch!(FilterPatch, "Filter", {
    choice    FilterMode => mode: FilterMode,
    f32       Cutoff     => cutoff_hz,
    f32       Resonance  => resonance,
    f32       Drive      => drive,
    groove_u8 EnvPattern => env_pattern,
    f32       EnvAmount  => env_amount,
    f32       EnvDecay   => env_decay_ms,
    f32       Mix        => mix,
});

/// Zero-allocation stereo-linked feed-forward bus/master compressor with soft knee,
/// makeup gain, and parallel mix.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompressorPatch {
    pub threshold_db: f32,
    pub ratio: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub makeup_db: f32,
    pub mix: f32,
}

impl Default for CompressorPatch {
    fn default() -> Self {
        Self {
            threshold_db: -14.0,
            ratio: 4.0,
            attack_ms: 8.0,
            release_ms: 120.0,
            makeup_db: 2.0,
            mix: 1.0,
        }
    }
}

impl_effect_patch!(CompressorPatch, "Compressor", {
    f32 Threshold => threshold_db,
    f32 Ratio     => ratio,
    f32 Attack    => attack_ms,
    f32 Release   => release_ms,
    f32 Makeup    => makeup_db,
    f32 Mix       => mix,
});

/// Zero-allocation stereo-linked lookahead brickwall peak limiter with input drive,
/// configurable release, and parallel mix.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LimiterPatch {
    pub ceiling_db: f32,
    pub input_drive_db: f32,
    pub release_ms: f32,
    pub mix: f32,
}

impl Default for LimiterPatch {
    fn default() -> Self {
        Self {
            ceiling_db: -0.3,
            input_drive_db: 0.0,
            release_ms: 80.0,
            mix: 1.0,
        }
    }
}

impl_effect_patch!(LimiterPatch, "Limiter", {
    f32 Ceiling    => ceiling_db,
    f32 InputDrive => input_drive_db,
    f32 Release    => release_ms,
    f32 Mix        => mix,
});

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "patch",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum EffectKind {
    Delay(DelayPatch),
    Reverb(ReverbPatch),
    Eq(EqPatch),
    Chorus(ChorusPatch),
    Drive(DrivePatch),
    Filter(FilterPatch),
    Compressor(CompressorPatch),
    Limiter(LimiterPatch),
}

impl EffectKind {
    /// Fieldless discriminant tag for this effect kind.
    #[inline]
    pub fn tag(&self) -> EffectKindTag {
        crate::dispatch_effect_kind!(self, @tag EffectKindTag)
    }

    /// Human-readable display label for this effect kind.
    pub fn label(&self) -> &'static str {
        self.tag().label()
    }

    /// Short 2–3 character badge for compact UI nodes (e.g. Signal Graph).
    pub fn short_tag(&self) -> &'static str {
        self.tag().short_tag()
    }

    /// Ordered native parameter list for this effect kind.
    pub fn params(&self) -> &'static [EffectParam] {
        self.tag().params()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectDevice {
    pub id: u64,
    pub name: String,
    pub bypassed: bool,
    pub kind: EffectKind,
}

impl EffectDevice {
    pub fn delay(id: u64) -> Self {
        EffectKindTag::Delay.default_device(id)
    }
    pub fn reverb(id: u64) -> Self {
        EffectKindTag::Reverb.default_device(id)
    }
    pub fn eq(id: u64) -> Self {
        EffectKindTag::Eq.default_device(id)
    }
    pub fn chorus(id: u64) -> Self {
        EffectKindTag::Chorus.default_device(id)
    }
    pub fn drive(id: u64) -> Self {
        EffectKindTag::Drive.default_device(id)
    }
    pub fn filter(id: u64) -> Self {
        EffectKindTag::Filter.default_device(id)
    }
    pub fn compressor(id: u64) -> Self {
        EffectKindTag::Compressor.default_device(id)
    }
    pub fn limiter(id: u64) -> Self {
        EffectKindTag::Limiter.default_device(id)
    }

    /// Construct a default `EffectDevice` from a canonical or alias kind name.
    pub fn from_kind_name(id: u64, kind: &str) -> Option<Self> {
        EffectKindTag::parse(kind).map(|tag| tag.default_device(id))
    }

    /// Whether `name` is a recognized effect kind alias for `:fx` / `AddEffect`.
    pub fn is_kind_name(kind: &str) -> bool {
        EffectKindTag::parse(kind).is_some()
    }

    /// Usage hint listing all effect kind names.
    pub fn kind_names_hint() -> &'static str {
        "delay, reverb, eq, chorus, drive, filter, comp, limiter"
    }

    /// Whether inserting this device directly on a non-return channel defaults to a subtle 30% wet mix.
    pub fn prefers_subtle_insert_mix(&self) -> bool {
        matches!(self.kind, EffectKind::Delay(_) | EffectKind::Reverb(_))
    }

    /// Processing latency in sample frames at `sample_rate` Hz (e.g. Limiter lookahead).
    pub fn latency_frames(&self, sample_rate: u32) -> u32 {
        match self.kind {
            EffectKind::Limiter(_) => {
                crate::dsp::dynamics::limiter_lookahead_frames(f64::from(sample_rate)) as u32
            }
            _ => 0,
        }
    }

    pub fn params(&self) -> &'static [EffectParam] {
        self.kind.params()
    }

    pub fn parse_param(&self, input: &str) -> Option<EffectParam> {
        EffectParam::parse_for_kind(&self.kind, input)
    }

    pub fn supports_param(&self, param: EffectParam) -> bool {
        self.params().contains(&param)
    }

    pub fn read_normalized(&self, param: EffectParam) -> Option<f32> {
        self.native_value(param).map(|v| param.normalize(v))
    }

    pub fn native_value(&self, param: EffectParam) -> Option<f32> {
        crate::dispatch_effect_kind!(self.kind, p => p.native_value(param))
    }

    pub fn write_native(&mut self, param: EffectParam, value: f32) -> Result<()> {
        ensure!(
            self.supports_param(param),
            "Parameter {} is unsupported by {}",
            param.name(),
            self.name
        );
        let (lo, hi) = param.range();
        ensure!(
            value.is_finite() && (lo..=hi).contains(&value),
            "{} must be within {lo}..{hi}",
            param.label()
        );
        param.write_kind_native(&mut self.kind, value);
        Ok(())
    }

    pub fn write_normalized(&mut self, param: EffectParam, n: f32) -> Result<()> {
        ensure!(
            n.is_finite() && (0.0..=1.0).contains(&n),
            "Effect value must be normalized 0..1"
        );
        self.write_native(param, param.denormalize(n))
    }

    pub fn nudge(&mut self, param: EffectParam, direction: i8, coarse: bool) -> Result<()> {
        let next = self.nudge_native(param, direction, coarse)?;
        self.write_native(param, next)
    }

    pub fn nudge_native(&self, param: EffectParam, direction: i8, coarse: bool) -> Result<f32> {
        let current = self
            .native_value(param)
            .context("Parameter is unsupported")?;
        let (min, max) = param.range();
        let next = match param.unit() {
            ParameterUnit::Ratio | ParameterUnit::BipolarRatio => {
                nudge_quantized(current, direction, 100.0, 1.0, 5.0, coarse)
            }
            ParameterUnit::SixteenthSteps => {
                nudge_quantized(current, direction, 4.0, 1.0, 4.0, coarse)
            }
            ParameterUnit::Seconds | ParameterUnit::Octaves => {
                nudge_quantized(current, direction, 10.0, 1.0, 5.0, coarse)
            }
            ParameterUnit::Milliseconds => {
                if param == EffectParam::EnvDecay {
                    nudge_log(current, direction, 1.12, 1.4, coarse).round()
                } else {
                    nudge_quantized(current, direction, 1.0, 1.0, 10.0, coarse)
                }
            }
            ParameterUnit::Hertz => nudge_log(current, direction, 1.08, 1.35, coarse),
            ParameterUnit::Decibels => {
                if param == EffectParam::Ceiling {
                    nudge_quantized(current, direction, 10.0, 1.0, 10.0, coarse)
                } else {
                    nudge_quantized(current, direction, 2.0, 1.0, 4.0, coarse)
                }
            }
            ParameterUnit::FilterMode | ParameterUnit::DelayMode | ParameterUnit::DriveMode => {
                current.round() + f32::from(direction)
            }
            ParameterUnit::CompressionRatio => {
                nudge_quantized(current, direction, 2.0, 1.0, 4.0, coarse)
            }
            ParameterUnit::FilterPattern => {
                let step = isize::from(direction) * if coarse { 5 } else { 1 };
                let cur = current
                    .round()
                    .clamp(0.0, (FILTER_ENV_PATTERN_COUNT - 1) as f32)
                    as u8;
                f32::from(step_filter_groove_id(cur, step))
            }
            ParameterUnit::QFactor => nudge_log(current, direction, 1.10, 1.45, coarse),
        };
        Ok(next.clamp(min, max))
    }

    pub fn parameter_descriptors(&self) -> Vec<EffectParamDescriptor> {
        let defaults = self.kind.tag().default_device(0);
        self.params()
            .iter()
            .filter_map(|&param| {
                let (native_min, native_max) = param.range();
                let normalized = self.read_normalized(param)?;
                let native_default = defaults.native_value(param)?;
                let native_value = self.native_value(param)?;
                Some(EffectParamDescriptor {
                    param,
                    name: param.name(),
                    unit: param.unit(),
                    native_min,
                    native_max,
                    native_default,
                    native_value,
                    normalized,
                    formatted: param.format_normalized(normalized),
                    automatable: true,
                    per_note_automation: false,
                })
            })
            .collect()
    }

    pub fn validate(&self) -> Result<()> {
        validate_name(&self.name, "Device")?;
        crate::dispatch_effect_kind!(self.kind, p => p.validate())
    }
}
