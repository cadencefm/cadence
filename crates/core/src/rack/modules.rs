//! The 12 orthogonal Eurorack modules (`Vco`, `Sampler`, `Noise`, `Mobius`, `Env`, `Lfo`,
//! `Filter`, `Vca`, `Drive`, `LoFi`, `Resonator`, `Out`), port compatibility, and cable attenuverter SPOTs.

use crate::dsp::env::{EnvMode, ModularEnvParams};
use crate::dsp::mobius::MobiusSynthParams;
use crate::mixer::{DriveMode, DrivePatch};
use crate::rack::params::{ModuleParam, LFO_DIVISIONS};
use crate::rack::types::{
    FilterMode, InPort, LfoShape, MobiusDispersion, MobiusExciter, MobiusTopology, NoiseColor,
    PitchTracking, ResonatorMode, TriggerMode, Unit, Waveform,
};
use crate::sampler::{
    parse_filename_midi, ParamDescriptor, SampleData, SampleLoopMode, SampleMetadata,
};
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};

fn default_vco_waveform() -> Waveform {
    Waveform::Saw
}
fn default_half() -> f32 {
    0.5
}
fn default_vco_level() -> f32 {
    0.75
}
fn default_fixed_hz() -> f32 {
    440.0
}
fn default_one_u8() -> u8 {
    1
}
fn default_unison_detune() -> f32 {
    12.0
}
fn default_unison_width() -> f32 {
    0.7
}
fn default_root_midi() -> u8 {
    60
}
fn default_loop_xfade() -> f32 {
    4.0
}
fn default_true() -> bool {
    true
}
fn default_one() -> f32 {
    1.0
}
fn default_noise_hp() -> f32 {
    20.0
}
fn default_noise_lp() -> f32 {
    20_000.0
}
fn default_noise_level() -> f32 {
    0.5
}
fn default_env_attack() -> f32 {
    4.0
}
fn default_env_decay() -> f32 {
    250.0
}
fn default_env_sustain() -> f32 {
    0.7
}
fn default_env_release() -> f32 {
    180.0
}
fn default_burst_spacing() -> f32 {
    10.0
}
fn default_lfo_rate() -> f32 {
    1.0
}
fn default_lfo_div() -> f32 {
    16.0
}
fn default_filter_cutoff() -> f32 {
    7_000.0
}
fn default_filter_res() -> f32 {
    0.12
}
fn default_drive_amount() -> f32 {
    0.35
}
fn default_drive_tone() -> f32 {
    12_000.0
}
fn default_lofi_bits() -> f32 {
    12.0
}
fn default_lofi_rate() -> f32 {
    16_000.0
}
fn default_resonator_decay() -> f32 {
    900.0
}
fn default_resonator_brightness() -> f32 {
    0.65
}
fn default_resonator_position() -> f32 {
    0.35
}
fn default_mobius_brightness() -> f32 {
    0.72
}
fn default_mobius_chirality() -> f32 {
    0.20
}
fn default_mobius_decay() -> f32 {
    220.0
}
fn default_mobius_sustain() -> f32 {
    0.80
}
fn default_mobius_atoms() -> u8 {
    2
}
fn default_mobius_structure() -> f32 {
    0.35
}
fn default_mobius_stiffness() -> f32 {
    0.20
}
fn default_mobius_kerr() -> f32 {
    1.5
}
fn default_mobius_coupling() -> f32 {
    0.85
}
fn default_mobius_fb_cycles() -> f32 {
    1.5
}
fn default_mobius_fb_ms() -> f32 {
    8.4
}
fn default_mobius_width() -> f32 {
    0.75
}

crate::define_choice_enum! {
    /// Discriminant tag for the 12 modular Eurorack [`ModuleKind`] variants.
    pub enum ModuleKindTag {
        Vco => ("vco", "VCO", ["osc", "oscillator"]),
        Sampler => ("sampler", "Sampler", ["smp", "sample"]),
        Noise => ("noise", "Noise", ["noi"]),
        Env => ("env", "Env", ["envelope", "adsr"]),
        Lfo => ("lfo", "LFO"),
        Filter => ("filter", "Filter", ["flt", "vcf"]),
        Vca => ("vca", "VCA", ["amp", "ring", "ringmod"]),
        Drive => ("drive", "Drive", ["drv", "sat", "saturator", "dist", "fold"]),
        #[serde(rename = "lofi")]
        LoFi => ("lofi", "Lo-Fi", ["lo_fi", "lfi", "crush", "bitcrush", "bitcrusher"]),
        Resonator => ("resonator", "Resonator", ["res", "modal", "rings", "plate"]),
        Mobius => ("mobius", "Mobius", ["mbs", "cubic", "atoms"]),
        Out => ("out", "Out", ["output"]),
    }
}

impl ModuleKindTag {
    /// All module kinds a user can freely add to a zone (`Out` is singular per zone).
    /// `Mobius` is appended after `Resonator` so existing picker hotkeys `a1..a9, a0` remain intact.
    pub const ADDABLE: &'static [Self] = &[
        Self::Vco,
        Self::Sampler,
        Self::Noise,
        Self::Env,
        Self::Lfo,
        Self::Filter,
        Self::Vca,
        Self::Drive,
        Self::LoFi,
        Self::Resonator,
        Self::Mobius,
    ];

    pub fn short_badge(self) -> &'static str {
        match self {
            Self::Vco => "VCO",
            Self::Sampler => "SMP",
            Self::Noise => "NOISE",
            Self::Env => "ENV",
            Self::Lfo => "LFO",
            Self::Filter => "FLT",
            Self::Vca => "VCA",
            Self::Drive => "DRV",
            Self::LoFi => "LOFI",
            Self::Resonator => "RES",
            Self::Mobius => "MBS",
            Self::Out => "OUT",
        }
    }

    pub fn default_name(self) -> &'static str {
        self.label()
    }

    /// Semantic Patchbay column (`0=SOURCES`, `1=MODULATORS`, `2=PROCESSORS`, `3=VCA/OUT`).
    pub const fn column(self) -> u8 {
        match self {
            Self::Vco | Self::Sampler | Self::Noise | Self::Mobius => 0,
            Self::Env | Self::Lfo => 1,
            Self::Filter | Self::Drive | Self::LoFi | Self::Resonator => 2,
            Self::Vca | Self::Out => 3,
        }
    }

    /// Whether this module kind produces a patchable `OutPort::Out` signal.
    pub const fn has_output(self) -> bool {
        !matches!(self, Self::Out)
    }

    /// Supported input ports for this module kind (SPOT for validation, Patchbay UI, and `:patch`).
    pub const fn in_ports(self) -> &'static [InPort] {
        match self {
            Self::Vco => &[InPort::PitchCv, InPort::FmIn, InPort::PwCv],
            Self::Sampler => &[InPort::PitchCv, InPort::TrigIn],
            Self::Noise | Self::Lfo => &[],
            Self::Env => &[InPort::TrigIn],
            Self::Filter | Self::Vca | Self::Drive | Self::Out => &[InPort::In, InPort::Cv],
            Self::LoFi => &[InPort::In],
            Self::Resonator => &[InPort::In, InPort::PitchCv, InPort::Cv],
            Self::Mobius => &[InPort::In, InPort::PitchCv, InPort::Cv, InPort::TrigIn],
        }
    }

    pub fn accepts_in_port(self, port: InPort) -> bool {
        self.in_ports().contains(&port)
    }

    /// Default target input port when `:patch <from> <to>` omits an explicit `.port`.
    pub fn default_in_port(self, from_kind: ModuleKindTag) -> Option<InPort> {
        let is_mod = matches!(from_kind, Self::Env | Self::Lfo);
        let ports = self.in_ports();
        if is_mod {
            for preferred in [InPort::Cv, InPort::PitchCv, InPort::TrigIn, InPort::PwCv] {
                if ports.contains(&preferred) {
                    return Some(preferred);
                }
            }
        } else {
            for preferred in [InPort::In, InPort::FmIn, InPort::Cv, InPort::PitchCv] {
                if ports.contains(&preferred) {
                    return Some(preferred);
                }
            }
        }
        ports.first().copied()
    }

    /// Ordered faceplate parameters owned by this module kind.
    pub const fn params(self) -> &'static [ModuleParam] {
        match self {
            Self::Vco => &[
                ModuleParam::Waveform,
                ModuleParam::Tune,
                ModuleParam::Fine,
                ModuleParam::PulseWidth,
                ModuleParam::Phase,
                ModuleParam::Level,
                ModuleParam::VcoPitchTrack,
                ModuleParam::FixedHz,
                ModuleParam::UnisonVoices,
                ModuleParam::UnisonDetune,
                ModuleParam::UnisonWidth,
            ],
            Self::Sampler => &[
                ModuleParam::RootMidi,
                ModuleParam::CoarseTune,
                ModuleParam::Fine,
                ModuleParam::SampleStart,
                ModuleParam::SampleEnd,
                ModuleParam::LoopMode,
                ModuleParam::LoopStart,
                ModuleParam::LoopEnd,
                ModuleParam::LoopXfade,
                ModuleParam::OneShot,
                ModuleParam::Level,
            ],
            Self::Noise => &[
                ModuleParam::NoiseColor,
                ModuleParam::HighpassHz,
                ModuleParam::LowpassHz,
                ModuleParam::Level,
            ],
            Self::Env => &[
                ModuleParam::EnvMode,
                ModuleParam::Attack,
                ModuleParam::Hold,
                ModuleParam::Decay,
                ModuleParam::Sustain,
                ModuleParam::Release,
                ModuleParam::TriggerMode,
                ModuleParam::DelayMs,
                ModuleParam::Bursts,
                ModuleParam::BurstSpacingMs,
            ],
            Self::Lfo => &[
                ModuleParam::LfoShape,
                ModuleParam::LfoRate,
                ModuleParam::LfoSync,
                ModuleParam::LfoDivision,
                ModuleParam::LfoRetrigger,
                ModuleParam::LfoBipolar,
            ],
            Self::Filter => &[
                ModuleParam::FilterMode,
                ModuleParam::Cutoff,
                ModuleParam::Resonance,
                ModuleParam::Drive,
                ModuleParam::KeyTrack,
                ModuleParam::VelAmount,
                ModuleParam::Mix,
            ],
            Self::Vca => &[ModuleParam::VcaGain, ModuleParam::VcaBias],
            Self::Drive => &[
                ModuleParam::DriveMode,
                ModuleParam::Drive,
                ModuleParam::Asymmetry,
                ModuleParam::ToneHz,
                ModuleParam::OutputDb,
                ModuleParam::Mix,
            ],
            Self::LoFi => &[
                ModuleParam::Bits,
                ModuleParam::SampleRateHz,
                ModuleParam::Mix,
            ],
            Self::Resonator => &[
                ModuleParam::ResonatorMode,
                ModuleParam::VcoPitchTrack,
                ModuleParam::Tune,
                ModuleParam::Fine,
                ModuleParam::FixedHz,
                ModuleParam::Structure,
                ModuleParam::ResonatorDecay,
                ModuleParam::Brightness,
                ModuleParam::Position,
                ModuleParam::Mix,
            ],
            Self::Mobius => &[
                ModuleParam::MobiusExciter,
                ModuleParam::Brightness,
                ModuleParam::MobiusShape,
                ModuleParam::MobiusChirality,
                ModuleParam::MobiusTexture,
                ModuleParam::Attack,
                ModuleParam::Decay,
                ModuleParam::Sustain,
                ModuleParam::MobiusAtoms,
                ModuleParam::MobiusDispersion,
                ModuleParam::Structure,
                ModuleParam::MobiusStiffness,
                ModuleParam::MobiusStiffnessTilt,
                ModuleParam::MobiusKerr,
                ModuleParam::MobiusKerrSpread,
                ModuleParam::MobiusTopology,
                ModuleParam::MobiusCoupling,
                ModuleParam::MobiusDistance,
                ModuleParam::MobiusSympathy,
                ModuleParam::MobiusFeedback,
                ModuleParam::MobiusFbLock,
                ModuleParam::MobiusFbCycles,
                ModuleParam::MobiusFbMs,
                ModuleParam::VcoPitchTrack,
                ModuleParam::Tune,
                ModuleParam::Fine,
                ModuleParam::FixedHz,
                ModuleParam::KeyTrack,
                ModuleParam::Spread,
                ModuleParam::Mix,
            ],
            Self::Out => &[ModuleParam::GainDb, ModuleParam::Pan, ModuleParam::Spread],
        }
    }

    pub fn default_kind(self) -> ModuleKind {
        match self {
            Self::Vco => ModuleKind::Vco(VcoModule::default()),
            Self::Sampler => ModuleKind::Sampler(SamplerModule::default()),
            Self::Noise => ModuleKind::Noise(NoiseModule::default()),
            Self::Env => ModuleKind::Env(EnvModule::default()),
            Self::Lfo => ModuleKind::Lfo(LfoModule::default()),
            Self::Filter => ModuleKind::Filter(FilterModule::default()),
            Self::Vca => ModuleKind::Vca(VcaModule::default()),
            Self::Drive => ModuleKind::Drive(DriveModule::default()),
            Self::LoFi => ModuleKind::LoFi(LoFiModule::default()),
            Self::Resonator => ModuleKind::Resonator(ResonatorModule::default()),
            Self::Mobius => ModuleKind::Mobius(MobiusModule::default()),
            Self::Out => ModuleKind::Out(OutModule::default()),
        }
    }
}

/// Bandlimited PolyBLEP/PolyBLAMP VCO with unison (1..=7), optional fixed-frequency mode,
/// start phase, through-zero linear FM (`FmIn`), pitch CV (`PitchCv`), and PWM (`PwCv`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VcoModule {
    #[serde(default = "default_vco_waveform")]
    pub waveform: Waveform,
    #[serde(default)]
    pub tune_semitones: f32,
    #[serde(default)]
    pub fine_cents: f32,
    #[serde(default = "default_half")]
    pub pulse_width: f32,
    #[serde(default)]
    pub phase_deg: f32,
    #[serde(default = "default_vco_level")]
    pub level: f32,
    #[serde(default)]
    pub pitch_tracking: PitchTracking,
    #[serde(default = "default_fixed_hz")]
    pub fixed_hz: f32,
    #[serde(default = "default_one_u8")]
    pub unison_voices: u8,
    #[serde(default = "default_unison_detune")]
    pub unison_detune_cents: f32,
    #[serde(default = "default_unison_width")]
    pub unison_width: f32,
}

impl Default for VcoModule {
    fn default() -> Self {
        Self {
            waveform: Waveform::Saw,
            tune_semitones: 0.0,
            fine_cents: 0.0,
            pulse_width: 0.5,
            phase_deg: 0.0,
            level: 0.75,
            pitch_tracking: PitchTracking::Chromatic,
            fixed_hz: 440.0,
            unison_voices: 1,
            unison_detune_cents: 12.0,
            unison_width: 0.7,
        }
    }
}

/// Stack-only `Copy` snapshot of [`SamplerModule`] synthesis parameters
/// (omits `path: Option<String>` so real-time voice setup never touches the heap).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SamplerParams {
    pub root_midi: u8,
    pub coarse_semitones: i16,
    pub fine_cents: f32,
    pub sample_start: usize,
    /// `0` means play to the end of the loaded sample.
    pub sample_end: usize,
    pub loop_mode: SampleLoopMode,
    pub loop_start: usize,
    pub loop_end: usize,
    pub loop_xfade_ms: f32,
    pub one_shot: bool,
    pub level: f32,
}

impl Default for SamplerParams {
    fn default() -> Self {
        Self {
            root_midi: 60,
            coarse_semitones: 0,
            fine_cents: 0.0,
            sample_start: 0,
            sample_end: 0,
            loop_mode: SampleLoopMode::Off,
            loop_start: 0,
            loop_end: 0,
            loop_xfade_ms: 4.0,
            one_shot: true,
            level: 1.0,
        }
    }
}

impl SamplerParams {
    /// Extract stack-only `Copy` synthesis parameters from a [`SamplerModule`].
    #[inline]
    pub const fn from_module(m: &SamplerModule) -> Self {
        m.params()
    }

    /// Playback window and loop configuration for [`crate::dsp::SamplerCursor`].
    #[inline]
    pub const fn region(self) -> crate::dsp::SamplerRegion {
        crate::dsp::SamplerRegion {
            sample_start: self.sample_start,
            sample_end: self.sample_end,
            loop_mode: self.loop_mode,
            loop_start: self.loop_start,
            loop_end: self.loop_end,
            loop_xfade_ms: self.loop_xfade_ms,
        }
    }

    #[inline]
    fn effective_frame(self, param: ModuleParam, total_frames: usize) -> usize {
        match param {
            ModuleParam::SampleStart => self.sample_start,
            ModuleParam::SampleEnd => {
                if self.sample_end == 0 {
                    total_frames
                } else {
                    self.sample_end
                }
            }
            ModuleParam::LoopStart => self.loop_start,
            ModuleParam::LoopEnd => {
                if self.loop_end == 0 {
                    total_frames
                } else {
                    self.loop_end
                }
            }
            _ => 0,
        }
    }

    #[inline]
    pub fn read_native(self, param: ModuleParam) -> Option<f32> {
        match param {
            ModuleParam::RootMidi => Some(f32::from(self.root_midi)),
            ModuleParam::CoarseTune => Some(f32::from(self.coarse_semitones)),
            ModuleParam::Fine => Some(self.fine_cents),
            ModuleParam::SampleStart => Some(self.sample_start as f32),
            ModuleParam::SampleEnd => Some(self.sample_end as f32),
            ModuleParam::LoopMode => Some(self.loop_mode.to_index() as f32),
            ModuleParam::LoopStart => Some(self.loop_start as f32),
            ModuleParam::LoopEnd => Some(self.loop_end as f32),
            ModuleParam::LoopXfade => Some(self.loop_xfade_ms),
            ModuleParam::OneShot => Some(if self.one_shot { 1.0 } else { 0.0 }),
            ModuleParam::Level => Some(self.level),
            _ => None,
        }
    }

    pub fn write_native(&mut self, param: ModuleParam, value: f32) -> Result<()> {
        ensure!(
            ModuleKindTag::Sampler.params().contains(&param),
            "{} module does not support parameter {}",
            ModuleKindTag::Sampler.label(),
            param.label()
        );
        param.validate_native(value)?;
        match param {
            ModuleParam::RootMidi => self.root_midi = value.round() as u8,
            ModuleParam::CoarseTune => self.coarse_semitones = value.round() as i16,
            ModuleParam::Fine => self.fine_cents = value,
            ModuleParam::SampleStart => self.sample_start = value.round().max(0.0) as usize,
            ModuleParam::SampleEnd => self.sample_end = value.round().max(0.0) as usize,
            ModuleParam::LoopMode => {
                self.loop_mode = SampleLoopMode::from_index(value.round().max(0.0) as usize)
            }
            ModuleParam::LoopStart => self.loop_start = value.round().max(0.0) as usize,
            ModuleParam::LoopEnd => self.loop_end = value.round().max(0.0) as usize,
            ModuleParam::LoopXfade => self.loop_xfade_ms = value,
            ModuleParam::OneShot => self.one_shot = value >= 0.5,
            ModuleParam::Level => self.level = value,
            _ => bail!("Unsupported module parameter"),
        }
        Ok(())
    }

    pub fn write_normalized(&mut self, param: ModuleParam, norm: f32, total_frames: usize) -> bool {
        if !ModuleKindTag::Sampler.params().contains(&param) {
            return false;
        }
        let n = if norm.is_finite() {
            norm.clamp(0.0, 1.0)
        } else {
            0.0
        };
        if param.is_frame_based() {
            let max_f = total_frames.max(self.sample_end).max(self.loop_end).max(1);
            match param {
                ModuleParam::SampleStart => {
                    let limit = self.effective_frame(ModuleParam::SampleEnd, total_frames);
                    self.sample_start =
                        (param.denormalize(n, max_f) as usize).min(limit.saturating_sub(1));
                }
                ModuleParam::SampleEnd => {
                    self.sample_end =
                        (param.denormalize(n, max_f) as usize).max(self.sample_start + 1);
                }
                ModuleParam::LoopStart => {
                    let limit = self.effective_frame(ModuleParam::LoopEnd, total_frames);
                    self.loop_start =
                        (param.denormalize(n, max_f) as usize).min(limit.saturating_sub(1));
                }
                ModuleParam::LoopEnd => {
                    self.loop_end = (param.denormalize(n, max_f) as usize).max(self.loop_start + 1);
                }
                _ => {}
            }
            return true;
        }
        self.write_native(param, param.denormalize(n, total_frames))
            .is_ok()
    }
}

/// First-class modular stereo sample player accepting audio-rate `PitchCv` and external `TrigIn`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SamplerModule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default = "default_root_midi")]
    pub root_midi: u8,
    #[serde(default)]
    pub coarse_semitones: i16,
    #[serde(default)]
    pub fine_cents: f32,
    #[serde(default)]
    pub sample_start: usize,
    /// `0` means play to the end of the loaded sample.
    #[serde(default)]
    pub sample_end: usize,
    #[serde(default)]
    pub loop_mode: SampleLoopMode,
    #[serde(default)]
    pub loop_start: usize,
    #[serde(default)]
    pub loop_end: usize,
    #[serde(default = "default_loop_xfade")]
    pub loop_xfade_ms: f32,
    #[serde(default = "default_true")]
    pub one_shot: bool,
    #[serde(default = "default_one")]
    pub level: f32,
}

impl Default for SamplerModule {
    fn default() -> Self {
        let p = SamplerParams::default();
        Self {
            path: None,
            root_midi: p.root_midi,
            coarse_semitones: p.coarse_semitones,
            fine_cents: p.fine_cents,
            sample_start: p.sample_start,
            sample_end: p.sample_end,
            loop_mode: p.loop_mode,
            loop_start: p.loop_start,
            loop_end: p.loop_end,
            loop_xfade_ms: p.loop_xfade_ms,
            one_shot: p.one_shot,
            level: p.level,
        }
    }
}

impl SamplerModule {
    /// Extract stack-only `Copy` synthesis parameters without cloning `path`.
    #[inline]
    pub const fn params(&self) -> SamplerParams {
        SamplerParams {
            root_midi: self.root_midi,
            coarse_semitones: self.coarse_semitones,
            fine_cents: self.fine_cents,
            sample_start: self.sample_start,
            sample_end: self.sample_end,
            loop_mode: self.loop_mode,
            loop_start: self.loop_start,
            loop_end: self.loop_end,
            loop_xfade_ms: self.loop_xfade_ms,
            one_shot: self.one_shot,
            level: self.level,
        }
    }

    /// Overwrite all synthesis parameters in place while preserving `path`.
    #[inline]
    pub fn apply_params(&mut self, p: SamplerParams) {
        self.root_midi = p.root_midi;
        self.coarse_semitones = p.coarse_semitones;
        self.fine_cents = p.fine_cents;
        self.sample_start = p.sample_start;
        self.sample_end = p.sample_end;
        self.loop_mode = p.loop_mode;
        self.loop_start = p.loop_start;
        self.loop_end = p.loop_end;
        self.loop_xfade_ms = p.loop_xfade_ms;
        self.one_shot = p.one_shot;
        self.level = p.level;
    }

    #[inline]
    pub fn read_native(&self, param: ModuleParam) -> Option<f32> {
        self.params().read_native(param)
    }

    pub fn write_native(&mut self, param: ModuleParam, value: f32) -> Result<()> {
        let mut p = self.params();
        p.write_native(param, value)?;
        self.apply_params(p);
        Ok(())
    }

    pub fn write_normalized(&mut self, param: ModuleParam, norm: f32, total_frames: usize) -> bool {
        let mut p = self.params();
        if p.write_normalized(param, norm, total_frames) {
            self.apply_params(p);
            true
        } else {
            false
        }
    }

    /// Populate pitch & loop fields from raw RIFF `smpl`/`inst` `SampleMetadata` and total frames.
    pub fn apply_metadata(
        &mut self,
        path_label: String,
        meta: &SampleMetadata,
        frames: usize,
        fallback_root: u8,
    ) {
        self.path = Some(path_label);
        self.root_midi = if meta.has_root {
            meta.root_midi.min(127)
        } else {
            fallback_root.min(127)
        };
        self.fine_cents = meta.fine_cents.clamp(-100.0, 100.0);
        self.sample_start = 0;
        self.sample_end = frames;
        if meta.loop_mode != SampleLoopMode::Off && meta.loop_end > meta.loop_start {
            self.loop_mode = meta.loop_mode;
            self.loop_start = meta.loop_start.min(frames.saturating_sub(1));
            self.loop_end = meta.loop_end.min(frames);
            self.one_shot = false;
            self.loop_xfade_ms = 4.0;
        } else {
            self.loop_mode = SampleLoopMode::Off;
            self.loop_start = 0;
            self.loop_end = frames;
            self.one_shot = true;
        }
    }

    /// Populate pitch & loop fields when loading a fresh sample from disk.
    pub fn apply_sample_defaults(&mut self, path_label: String, sample: &SampleData) {
        let fallback = parse_filename_midi(&sample.name)
            .or_else(|| parse_filename_midi(&path_label))
            .unwrap_or(60);
        self.apply_metadata(path_label, &sample.meta, sample.frames, fallback);
    }
}

/// White/Pink noise generator with independent high-pass and low-pass band shaping.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoiseModule {
    #[serde(default)]
    pub color: NoiseColor,
    #[serde(default = "default_noise_hp")]
    pub highpass_hz: f32,
    #[serde(default = "default_noise_lp")]
    pub lowpass_hz: f32,
    #[serde(default = "default_noise_level")]
    pub level: f32,
}

impl Default for NoiseModule {
    fn default() -> Self {
        Self {
            color: NoiseColor::White,
            highpass_hz: 20.0,
            lowpass_hz: 20_000.0,
            level: 0.5,
        }
    }
}

/// Composable ADSR / Hold-Decay envelope generator with pre-delay and multi-burst retriggering.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvModule {
    #[serde(default)]
    pub mode: EnvMode,
    #[serde(default = "default_env_attack")]
    pub attack_ms: f32,
    #[serde(default)]
    pub hold_ms: f32,
    #[serde(default = "default_env_decay")]
    pub decay_ms: f32,
    #[serde(default = "default_env_sustain")]
    pub sustain: f32,
    #[serde(default = "default_env_release")]
    pub release_ms: f32,
    #[serde(default)]
    pub trigger_mode: TriggerMode,
    #[serde(default)]
    pub delay_ms: f32,
    #[serde(default = "default_one_u8")]
    pub bursts: u8,
    #[serde(default = "default_burst_spacing")]
    pub burst_spacing_ms: f32,
}

impl Default for EnvModule {
    fn default() -> Self {
        Self {
            mode: EnvMode::Adsr,
            attack_ms: 4.0,
            hold_ms: 0.0,
            decay_ms: 250.0,
            sustain: 0.7,
            release_ms: 180.0,
            trigger_mode: TriggerMode::Gate,
            delay_ms: 0.0,
            bursts: 1,
            burst_spacing_ms: 10.0,
        }
    }
}

impl EnvModule {
    /// Convert directly into the pure DSP parameter snapshot shared by `ModularEnv` and UI curves.
    #[inline]
    pub fn to_dsp_params(&self) -> ModularEnvParams {
        ModularEnvParams {
            mode: self.mode,
            attack_ms: self.attack_ms,
            hold_ms: self.hold_ms,
            decay_ms: self.decay_ms,
            sustain: self.sustain,
            release_ms: self.release_ms,
            trigger_mode: self.trigger_mode,
            delay_ms: self.delay_ms,
            bursts: self.bursts,
            burst_spacing_ms: self.burst_spacing_ms,
        }
    }
}

/// Tempo-syncable multi-shape LFO (`bipolar = true` outputs `[-1, 1]`, `false` outputs `[0, 1]`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LfoModule {
    #[serde(default)]
    pub shape: LfoShape,
    #[serde(default = "default_lfo_rate")]
    pub rate_hz: f32,
    #[serde(default)]
    pub sync: bool,
    #[serde(default = "default_lfo_div")]
    pub division_steps: f32,
    #[serde(default = "default_true")]
    pub retrigger: bool,
    #[serde(default = "default_true")]
    pub bipolar: bool,
}

impl Default for LfoModule {
    fn default() -> Self {
        Self {
            shape: LfoShape::Sine,
            rate_hz: 1.0,
            sync: false,
            division_steps: 16.0,
            retrigger: true,
            bipolar: true,
        }
    }
}

impl LfoModule {
    #[inline]
    pub fn effective_hz(&self, bpm: f64) -> f64 {
        if self.sync {
            (bpm.clamp(20.0, 300.0) / 15.0) / f64::from(self.division_steps.max(0.25))
        } else {
            f64::from(self.rate_hz)
        }
    }
}

/// Multi-mode ZDF filter (`LowPass`, `BandPass`, `HighPass`, `AcidLowPass`, `LowPass6`)
/// with pre-drive, key tracking, velocity/Acid accent modulation, and parallel dry/wet mix.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilterModule {
    #[serde(default)]
    pub mode: FilterMode,
    #[serde(default = "default_filter_cutoff")]
    pub cutoff_hz: f32,
    #[serde(default = "default_filter_res")]
    pub resonance: f32,
    #[serde(default)]
    pub drive: f32,
    #[serde(default)]
    pub key_tracking: f32,
    #[serde(default)]
    pub vel_amount: f32,
    #[serde(default = "default_one")]
    pub mix: f32,
}

impl Default for FilterModule {
    fn default() -> Self {
        Self {
            mode: FilterMode::LowPass,
            cutoff_hz: 7_000.0,
            resonance: 0.12,
            drive: 0.0,
            key_tracking: 0.0,
            vel_amount: 0.0,
            mix: 1.0,
        }
    }
}

/// Voltage-controlled amplifier and 4-quadrant audio-rate Ring Modulator.
///
/// - When `Cv` has incoming cables: `output = in * gain * (bias + cv_sum)`.
///   With `bias = 0.0`, an `Env -> Vca.Cv` acts as an amp envelope and a `Vco -> Vca.Cv`
///   acts as a 4-quadrant audio-rate Ring Modulator.
/// - When `Cv` has no incoming cables: `output = in * gain` (passes cleanly when unpatched).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VcaModule {
    #[serde(default = "default_one")]
    pub gain: f32,
    #[serde(default)]
    pub bias: f32,
}

impl Default for VcaModule {
    fn default() -> Self {
        Self {
            gain: 1.0,
            bias: 0.0,
        }
    }
}

/// Multi-mode waveshaper (`Tape`, `HardClip`, `Fold`) with bipolar asymmetry, tone LPF,
/// output trim, and dry/wet mix, backed directly by `crate::dsp::saturate`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DriveModule {
    #[serde(default)]
    pub mode: DriveMode,
    #[serde(default = "default_drive_amount")]
    pub drive: f32,
    #[serde(default)]
    pub asymmetry: f32,
    #[serde(default = "default_drive_tone")]
    pub tone_hz: f32,
    #[serde(default)]
    pub output_db: f32,
    #[serde(default = "default_one")]
    pub mix: f32,
}

impl Default for DriveModule {
    fn default() -> Self {
        Self {
            mode: DriveMode::Tape,
            drive: 0.35,
            asymmetry: 0.0,
            tone_hz: 12_000.0,
            output_db: 0.0,
            mix: 1.0,
        }
    }
}

impl DriveModule {
    /// Convert directly to [`DrivePatch`] for `dsp::saturate::drive_transfer_sample` and `Drive`.
    #[inline]
    pub fn to_drive_patch(&self) -> DrivePatch {
        DrivePatch {
            mode: self.mode,
            drive: self.drive,
            asymmetry: self.asymmetry,
            tone_hz: self.tone_hz,
            output_db: self.output_db,
            mix: self.mix,
        }
    }
}

/// Lo-Fi bit-depth quantizer (`2..=24` bits) and sample-rate clock decimator (`500..=48000 Hz`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoFiModule {
    #[serde(default = "default_lofi_bits")]
    pub bits: f32,
    #[serde(default = "default_lofi_rate")]
    pub rate_hz: f32,
    #[serde(default = "default_one")]
    pub mix: f32,
}

impl Default for LoFiModule {
    fn default() -> Self {
        Self {
            bits: 12.0,
            rate_hz: 16_000.0,
            mix: 1.0,
        }
    }
}

/// Physical 32-mode modal resonator (`String`, `Bar`, `Membrane`, `Plate`) excited by `In`,
/// with chromatic or fixed pitch (`PitchCv`), continuous `brightness` modulation (`Cv`),
/// spatial strike `position`, inharmonic `structure`, and `-60 dB` RT60 `decay_ms`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResonatorModule {
    #[serde(default)]
    pub mode: ResonatorMode,
    #[serde(default)]
    pub pitch_tracking: PitchTracking,
    #[serde(default)]
    pub tune_semitones: f32,
    #[serde(default)]
    pub fine_cents: f32,
    #[serde(default = "default_fixed_hz")]
    pub fixed_hz: f32,
    #[serde(default = "default_half")]
    pub structure: f32,
    #[serde(default = "default_resonator_decay")]
    pub decay_ms: f32,
    #[serde(default = "default_resonator_brightness")]
    pub brightness: f32,
    #[serde(default = "default_resonator_position")]
    pub position: f32,
    #[serde(default = "default_one")]
    pub mix: f32,
}

impl Default for ResonatorModule {
    fn default() -> Self {
        Self {
            mode: ResonatorMode::String,
            pitch_tracking: PitchTracking::Chromatic,
            tune_semitones: 0.0,
            fine_cents: 0.0,
            fixed_hz: 440.0,
            structure: 0.5,
            decay_ms: 900.0,
            brightness: 0.65,
            position: 0.35,
            mix: 1.0,
        }
    }
}

/// Complex State-Kerr Cubic-Atom Synthesizer and space-time resonant processor (`N = 1..=8` atoms).
///
/// Composes closed-form unit-disk exciters (`Szego`, `Dipole`, `Poisson`, `Strike`, `External`),
/// pure unlifted State-Kerr cubic atoms across 6 pure-target topologies (`Cascade`, `Bloom`,
/// `Star`, `Ring`, `Swarm`, `Hybrid`), convex propagation delays, delayed projective bore
/// feedback, and zero-comb quadrature stereo projection.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MobiusModule {
    #[serde(default)]
    pub exciter: MobiusExciter,
    #[serde(default = "default_mobius_brightness")]
    pub brightness: f32,
    #[serde(default = "default_half")]
    pub shape: f32,
    #[serde(default = "default_mobius_chirality")]
    pub chirality: f32,
    #[serde(default)]
    pub texture: f32,
    #[serde(default = "default_env_attack")]
    pub attack_ms: f32,
    #[serde(default = "default_mobius_decay")]
    pub decay_ms: f32,
    #[serde(default = "default_mobius_sustain")]
    pub sustain: f32,
    #[serde(default = "default_mobius_atoms")]
    pub atoms: u8,
    #[serde(default)]
    pub dispersion: MobiusDispersion,
    #[serde(default = "default_mobius_structure")]
    pub structure: f32,
    #[serde(default = "default_mobius_stiffness")]
    pub stiffness: f32,
    #[serde(default)]
    pub stiffness_tilt: f32,
    #[serde(default = "default_mobius_kerr")]
    pub kerr_semitones: f32,
    #[serde(default)]
    pub kerr_spread: f32,
    #[serde(default)]
    pub topology: MobiusTopology,
    #[serde(default = "default_mobius_coupling")]
    pub coupling: f32,
    #[serde(default)]
    pub distance_ms: f32,
    #[serde(default)]
    pub sympathy: f32,
    #[serde(default)]
    pub feedback: f32,
    #[serde(default = "default_true")]
    pub fb_lock: bool,
    #[serde(default = "default_mobius_fb_cycles")]
    pub fb_cycles: f32,
    #[serde(default = "default_mobius_fb_ms")]
    pub fb_ms: f32,
    #[serde(default)]
    pub pitch_tracking: PitchTracking,
    #[serde(default)]
    pub tune_semitones: f32,
    #[serde(default)]
    pub fine_cents: f32,
    #[serde(default = "default_fixed_hz")]
    pub fixed_hz: f32,
    #[serde(default = "default_one")]
    pub key_tracking: f32,
    #[serde(default = "default_mobius_width")]
    pub stereo_width: f32,
    #[serde(default = "default_one")]
    pub mix: f32,
}

impl Default for MobiusModule {
    fn default() -> Self {
        Self {
            exciter: MobiusExciter::Dipole,
            brightness: 0.72,
            shape: 0.50,
            chirality: 0.20,
            texture: 0.0,
            attack_ms: 4.0,
            decay_ms: 220.0,
            sustain: 0.80,
            atoms: 2,
            dispersion: MobiusDispersion::Harmonic,
            structure: 0.35,
            stiffness: 0.20,
            stiffness_tilt: 0.0,
            kerr_semitones: 1.5,
            kerr_spread: 0.0,
            topology: MobiusTopology::Cascade,
            coupling: 0.85,
            distance_ms: 0.0,
            sympathy: 0.0,
            feedback: 0.0,
            fb_lock: true,
            fb_cycles: 1.5,
            fb_ms: 8.4,
            pitch_tracking: PitchTracking::Chromatic,
            tune_semitones: 0.0,
            fine_cents: 0.0,
            fixed_hz: 440.0,
            key_tracking: 1.0,
            stereo_width: 0.75,
            mix: 1.0,
        }
    }
}

impl MobiusModule {
    /// Project directly into the pure DSP parameter snapshot shared by [`crate::dsp::MobiusSynth`]
    /// and GPU visualizers.
    #[inline]
    pub fn to_dsp_params(&self, base_hz: f64, brightness_override: f64) -> MobiusSynthParams {
        MobiusSynthParams {
            exciter: self.exciter,
            base_hz,
            brightness: brightness_override,
            shape: f64::from(self.shape),
            chirality: f64::from(self.chirality),
            texture: f64::from(self.texture),
            attack_ms: f64::from(self.attack_ms),
            decay_ms: f64::from(self.decay_ms),
            sustain: f64::from(self.sustain),
            atoms: usize::from(self.atoms).clamp(1, crate::dsp::MOBIUS_MAX_ATOMS),
            dispersion: self.dispersion,
            structure: f64::from(self.structure),
            stiffness: f64::from(self.stiffness),
            stiffness_tilt: f64::from(self.stiffness_tilt),
            kerr_semitones: f64::from(self.kerr_semitones),
            kerr_spread: f64::from(self.kerr_spread),
            key_track: f64::from(self.key_tracking),
            topology: self.topology,
            coupling: f64::from(self.coupling),
            distance_ms: f64::from(self.distance_ms),
            sympathy: f64::from(self.sympathy),
            feedback: f64::from(self.feedback),
            fb_lock: self.fb_lock,
            fb_cycles: f64::from(self.fb_cycles),
            fb_ms: f64::from(self.fb_ms),
            stereo_width: f64::from(self.stereo_width),
        }
    }
}

/// Minimum `OutModule::gain_db` in decibels shared by parameter metadata and `OutNode`.
pub const OUT_MIN_GAIN_DB: f32 = -36.0;
/// Maximum `OutModule::gain_db` in decibels shared by parameter metadata and `OutNode`.
pub const OUT_MAX_GAIN_DB: f32 = 12.0;

/// Terminal zone output module (DC block + voice safety guard + stereo gain/pan/spread).
/// Exactly one `OutModule` is required per [`crate::rack::RackZone`].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutModule {
    #[serde(default)]
    pub gain_db: f32,
    #[serde(default)]
    pub pan: f32,
    #[serde(default)]
    pub spread: f32,
}

impl Default for OutModule {
    fn default() -> Self {
        Self {
            gain_db: 0.0,
            pan: 0.0,
            spread: 0.0,
        }
    }
}

/// The 12 orthogonal Eurorack module kinds composing every Cadence instrument zone.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModuleKind {
    Vco(VcoModule),
    Sampler(SamplerModule),
    Noise(NoiseModule),
    Env(EnvModule),
    Lfo(LfoModule),
    Filter(FilterModule),
    Vca(VcaModule),
    Drive(DriveModule),
    #[serde(rename = "lofi")]
    LoFi(LoFiModule),
    Resonator(ResonatorModule),
    Mobius(MobiusModule),
    Out(OutModule),
}

impl ModuleKind {
    pub const fn tag(&self) -> ModuleKindTag {
        match self {
            Self::Vco(_) => ModuleKindTag::Vco,
            Self::Sampler(_) => ModuleKindTag::Sampler,
            Self::Noise(_) => ModuleKindTag::Noise,
            Self::Env(_) => ModuleKindTag::Env,
            Self::Lfo(_) => ModuleKindTag::Lfo,
            Self::Filter(_) => ModuleKindTag::Filter,
            Self::Vca(_) => ModuleKindTag::Vca,
            Self::Drive(_) => ModuleKindTag::Drive,
            Self::LoFi(_) => ModuleKindTag::LoFi,
            Self::Resonator(_) => ModuleKindTag::Resonator,
            Self::Mobius(_) => ModuleKindTag::Mobius,
            Self::Out(_) => ModuleKindTag::Out,
        }
    }

    pub fn label(&self) -> &'static str {
        self.tag().label()
    }

    pub const fn column(&self) -> u8 {
        self.tag().column()
    }

    pub const fn has_output(&self) -> bool {
        self.tag().has_output()
    }

    pub const fn in_ports(&self) -> &'static [InPort] {
        self.tag().in_ports()
    }

    pub fn accepts_in_port(&self, port: InPort) -> bool {
        self.tag().accepts_in_port(port)
    }

    pub const fn params(&self) -> &'static [ModuleParam] {
        self.tag().params()
    }

    pub fn supports_param(&self, param: ModuleParam) -> bool {
        self.params().contains(&param)
    }

    pub fn validate(&self) -> Result<()> {
        if let Self::Sampler(smp) = self {
            if let Some(p) = &smp.path {
                ensure!(
                    !p.trim().is_empty() && p.len() <= 1024 && !p.chars().any(char::is_control),
                    "Sample path must be 1–1024 printable characters"
                );
            }
        }
        if let Self::Lfo(lfo) = self {
            ensure!(
                LFO_DIVISIONS
                    .iter()
                    .any(|&d| (d - lfo.division_steps).abs() < 1e-4),
                "LFO division is not a supported musical fraction"
            );
        }
        for &param in self.params() {
            let value = self.read_native(param).unwrap_or(f32::NAN);
            let (min, max) = param.range();
            ensure!(
                value.is_finite() && value >= min && value <= max,
                "{} {} must be finite and within {min}..={max}",
                self.label(),
                param.label()
            );
        }
        Ok(())
    }

    pub fn read_native(&self, param: ModuleParam) -> Option<f32> {
        match (self, param) {
            // VCO
            (Self::Vco(m), ModuleParam::Waveform) => Some(m.waveform.to_index() as f32),
            (Self::Vco(m), ModuleParam::Tune) => Some(m.tune_semitones),
            (Self::Vco(m), ModuleParam::Fine) => Some(m.fine_cents),
            (Self::Vco(m), ModuleParam::PulseWidth) => Some(m.pulse_width),
            (Self::Vco(m), ModuleParam::Phase) => Some(m.phase_deg),
            (Self::Vco(m), ModuleParam::Level) => Some(m.level),
            (Self::Vco(m), ModuleParam::VcoPitchTrack) => Some(m.pitch_tracking.to_index() as f32),
            (Self::Vco(m), ModuleParam::FixedHz) => Some(m.fixed_hz),
            (Self::Vco(m), ModuleParam::UnisonVoices) => Some(f32::from(m.unison_voices)),
            (Self::Vco(m), ModuleParam::UnisonDetune) => Some(m.unison_detune_cents),
            (Self::Vco(m), ModuleParam::UnisonWidth) => Some(m.unison_width),
            // Sampler
            (Self::Sampler(m), p) => m.read_native(p),
            // Noise
            (Self::Noise(m), ModuleParam::NoiseColor) => Some(m.color.to_index() as f32),
            (Self::Noise(m), ModuleParam::HighpassHz) => Some(m.highpass_hz),
            (Self::Noise(m), ModuleParam::LowpassHz) => Some(m.lowpass_hz),
            (Self::Noise(m), ModuleParam::Level) => Some(m.level),
            // Env
            (Self::Env(m), ModuleParam::EnvMode) => Some(m.mode.to_index() as f32),
            (Self::Env(m), ModuleParam::Attack) => Some(m.attack_ms),
            (Self::Env(m), ModuleParam::Hold) => Some(m.hold_ms),
            (Self::Env(m), ModuleParam::Decay) => Some(m.decay_ms),
            (Self::Env(m), ModuleParam::Sustain) => Some(m.sustain),
            (Self::Env(m), ModuleParam::Release) => Some(m.release_ms),
            (Self::Env(m), ModuleParam::TriggerMode) => Some(m.trigger_mode.to_index() as f32),
            (Self::Env(m), ModuleParam::DelayMs) => Some(m.delay_ms),
            (Self::Env(m), ModuleParam::Bursts) => Some(f32::from(m.bursts)),
            (Self::Env(m), ModuleParam::BurstSpacingMs) => Some(m.burst_spacing_ms),
            // LFO
            (Self::Lfo(m), ModuleParam::LfoShape) => Some(m.shape.to_index() as f32),
            (Self::Lfo(m), ModuleParam::LfoRate) => Some(m.rate_hz),
            (Self::Lfo(m), ModuleParam::LfoSync) => Some(if m.sync { 1.0 } else { 0.0 }),
            (Self::Lfo(m), ModuleParam::LfoDivision) => Some(m.division_steps),
            (Self::Lfo(m), ModuleParam::LfoRetrigger) => Some(if m.retrigger { 1.0 } else { 0.0 }),
            (Self::Lfo(m), ModuleParam::LfoBipolar) => Some(if m.bipolar { 1.0 } else { 0.0 }),
            // Filter
            (Self::Filter(m), ModuleParam::FilterMode) => Some(m.mode.to_index() as f32),
            (Self::Filter(m), ModuleParam::Cutoff) => Some(m.cutoff_hz),
            (Self::Filter(m), ModuleParam::Resonance) => Some(m.resonance),
            (Self::Filter(m), ModuleParam::Drive) => Some(m.drive),
            (Self::Filter(m), ModuleParam::KeyTrack) => Some(m.key_tracking),
            (Self::Filter(m), ModuleParam::VelAmount) => Some(m.vel_amount),
            (Self::Filter(m), ModuleParam::Mix) => Some(m.mix),
            // VCA
            (Self::Vca(m), ModuleParam::VcaGain) => Some(m.gain),
            (Self::Vca(m), ModuleParam::VcaBias) => Some(m.bias),
            // Drive
            (Self::Drive(m), ModuleParam::DriveMode) => Some(m.mode.to_index() as f32),
            (Self::Drive(m), ModuleParam::Drive) => Some(m.drive),
            (Self::Drive(m), ModuleParam::Asymmetry) => Some(m.asymmetry),
            (Self::Drive(m), ModuleParam::ToneHz) => Some(m.tone_hz),
            (Self::Drive(m), ModuleParam::OutputDb) => Some(m.output_db),
            (Self::Drive(m), ModuleParam::Mix) => Some(m.mix),
            // LoFi
            (Self::LoFi(m), ModuleParam::Bits) => Some(m.bits),
            (Self::LoFi(m), ModuleParam::SampleRateHz) => Some(m.rate_hz),
            (Self::LoFi(m), ModuleParam::Mix) => Some(m.mix),
            // Resonator
            (Self::Resonator(m), ModuleParam::ResonatorMode) => Some(m.mode.to_index() as f32),
            (Self::Resonator(m), ModuleParam::VcoPitchTrack) => {
                Some(m.pitch_tracking.to_index() as f32)
            }
            (Self::Resonator(m), ModuleParam::Tune) => Some(m.tune_semitones),
            (Self::Resonator(m), ModuleParam::Fine) => Some(m.fine_cents),
            (Self::Resonator(m), ModuleParam::FixedHz) => Some(m.fixed_hz),
            (Self::Resonator(m), ModuleParam::Structure) => Some(m.structure),
            (Self::Resonator(m), ModuleParam::ResonatorDecay) => Some(m.decay_ms),
            (Self::Resonator(m), ModuleParam::Brightness) => Some(m.brightness),
            (Self::Resonator(m), ModuleParam::Position) => Some(m.position),
            (Self::Resonator(m), ModuleParam::Mix) => Some(m.mix),
            // Mobius
            (Self::Mobius(m), ModuleParam::MobiusExciter) => Some(m.exciter.to_index() as f32),
            (Self::Mobius(m), ModuleParam::Brightness) => Some(m.brightness),
            (Self::Mobius(m), ModuleParam::MobiusShape) => Some(m.shape),
            (Self::Mobius(m), ModuleParam::MobiusChirality) => Some(m.chirality),
            (Self::Mobius(m), ModuleParam::MobiusTexture) => Some(m.texture),
            (Self::Mobius(m), ModuleParam::Attack) => Some(m.attack_ms),
            (Self::Mobius(m), ModuleParam::Decay) => Some(m.decay_ms),
            (Self::Mobius(m), ModuleParam::Sustain) => Some(m.sustain),
            (Self::Mobius(m), ModuleParam::MobiusAtoms) => Some(f32::from(m.atoms)),
            (Self::Mobius(m), ModuleParam::MobiusDispersion) => {
                Some(m.dispersion.to_index() as f32)
            }
            (Self::Mobius(m), ModuleParam::Structure) => Some(m.structure),
            (Self::Mobius(m), ModuleParam::MobiusStiffness) => Some(m.stiffness),
            (Self::Mobius(m), ModuleParam::MobiusStiffnessTilt) => Some(m.stiffness_tilt),
            (Self::Mobius(m), ModuleParam::MobiusKerr) => Some(m.kerr_semitones),
            (Self::Mobius(m), ModuleParam::MobiusKerrSpread) => Some(m.kerr_spread),
            (Self::Mobius(m), ModuleParam::MobiusTopology) => Some(m.topology.to_index() as f32),
            (Self::Mobius(m), ModuleParam::MobiusCoupling) => Some(m.coupling),
            (Self::Mobius(m), ModuleParam::MobiusDistance) => Some(m.distance_ms),
            (Self::Mobius(m), ModuleParam::MobiusSympathy) => Some(m.sympathy),
            (Self::Mobius(m), ModuleParam::MobiusFeedback) => Some(m.feedback),
            (Self::Mobius(m), ModuleParam::MobiusFbLock) => Some(if m.fb_lock { 1.0 } else { 0.0 }),
            (Self::Mobius(m), ModuleParam::MobiusFbCycles) => Some(m.fb_cycles),
            (Self::Mobius(m), ModuleParam::MobiusFbMs) => Some(m.fb_ms),
            (Self::Mobius(m), ModuleParam::VcoPitchTrack) => {
                Some(m.pitch_tracking.to_index() as f32)
            }
            (Self::Mobius(m), ModuleParam::Tune) => Some(m.tune_semitones),
            (Self::Mobius(m), ModuleParam::Fine) => Some(m.fine_cents),
            (Self::Mobius(m), ModuleParam::FixedHz) => Some(m.fixed_hz),
            (Self::Mobius(m), ModuleParam::KeyTrack) => Some(m.key_tracking),
            (Self::Mobius(m), ModuleParam::Spread) => Some(m.stereo_width),
            (Self::Mobius(m), ModuleParam::Mix) => Some(m.mix),
            // Out
            (Self::Out(m), ModuleParam::GainDb) => Some(m.gain_db),
            (Self::Out(m), ModuleParam::Pan) => Some(m.pan),
            (Self::Out(m), ModuleParam::Spread) => Some(m.spread),
            _ => None,
        }
    }

    pub fn write_native(&mut self, param: ModuleParam, value: f32) -> Result<()> {
        if let Self::Sampler(m) = self {
            return m.write_native(param, value);
        }
        ensure!(
            self.supports_param(param),
            "{} module does not support parameter {}",
            self.label(),
            param.label()
        );
        param.validate_native(value)?;
        match (self, param) {
            // VCO
            (Self::Vco(m), ModuleParam::Waveform) => {
                m.waveform = Waveform::from_index(value.round().max(0.0) as usize)
            }
            (Self::Vco(m), ModuleParam::Tune) => m.tune_semitones = value,
            (Self::Vco(m), ModuleParam::Fine) => m.fine_cents = value,
            (Self::Vco(m), ModuleParam::PulseWidth) => m.pulse_width = value,
            (Self::Vco(m), ModuleParam::Phase) => m.phase_deg = value,
            (Self::Vco(m), ModuleParam::Level) => m.level = value,
            (Self::Vco(m), ModuleParam::VcoPitchTrack) => {
                m.pitch_tracking = PitchTracking::from_index(value.round().max(0.0) as usize)
            }
            (Self::Vco(m), ModuleParam::FixedHz) => m.fixed_hz = value,
            (Self::Vco(m), ModuleParam::UnisonVoices) => m.unison_voices = value.round() as u8,
            (Self::Vco(m), ModuleParam::UnisonDetune) => m.unison_detune_cents = value,
            (Self::Vco(m), ModuleParam::UnisonWidth) => m.unison_width = value,
            // Noise
            (Self::Noise(m), ModuleParam::NoiseColor) => {
                m.color = NoiseColor::from_index(value.round().max(0.0) as usize)
            }
            (Self::Noise(m), ModuleParam::HighpassHz) => m.highpass_hz = value,
            (Self::Noise(m), ModuleParam::LowpassHz) => m.lowpass_hz = value,
            (Self::Noise(m), ModuleParam::Level) => m.level = value,
            // Env
            (Self::Env(m), ModuleParam::EnvMode) => {
                m.mode = EnvMode::from_index(value.round().max(0.0) as usize)
            }
            (Self::Env(m), ModuleParam::Attack) => m.attack_ms = value,
            (Self::Env(m), ModuleParam::Hold) => m.hold_ms = value,
            (Self::Env(m), ModuleParam::Decay) => m.decay_ms = value,
            (Self::Env(m), ModuleParam::Sustain) => m.sustain = value,
            (Self::Env(m), ModuleParam::Release) => m.release_ms = value,
            (Self::Env(m), ModuleParam::TriggerMode) => {
                m.trigger_mode = TriggerMode::from_index(value.round().max(0.0) as usize)
            }
            (Self::Env(m), ModuleParam::DelayMs) => m.delay_ms = value,
            (Self::Env(m), ModuleParam::Bursts) => m.bursts = value.round() as u8,
            (Self::Env(m), ModuleParam::BurstSpacingMs) => m.burst_spacing_ms = value,
            // LFO
            (Self::Lfo(m), ModuleParam::LfoShape) => {
                m.shape = LfoShape::from_index(value.round().max(0.0) as usize)
            }
            (Self::Lfo(m), ModuleParam::LfoRate) => m.rate_hz = value,
            (Self::Lfo(m), ModuleParam::LfoSync) => m.sync = value >= 0.5,
            (Self::Lfo(m), ModuleParam::LfoDivision) => m.division_steps = value,
            (Self::Lfo(m), ModuleParam::LfoRetrigger) => m.retrigger = value >= 0.5,
            (Self::Lfo(m), ModuleParam::LfoBipolar) => m.bipolar = value >= 0.5,
            // Filter
            (Self::Filter(m), ModuleParam::FilterMode) => {
                m.mode = FilterMode::from_index(value.round().max(0.0) as usize)
            }
            (Self::Filter(m), ModuleParam::Cutoff) => m.cutoff_hz = value,
            (Self::Filter(m), ModuleParam::Resonance) => m.resonance = value,
            (Self::Filter(m), ModuleParam::Drive) => m.drive = value,
            (Self::Filter(m), ModuleParam::KeyTrack) => m.key_tracking = value,
            (Self::Filter(m), ModuleParam::VelAmount) => m.vel_amount = value,
            (Self::Filter(m), ModuleParam::Mix) => m.mix = value,
            // VCA
            (Self::Vca(m), ModuleParam::VcaGain) => m.gain = value,
            (Self::Vca(m), ModuleParam::VcaBias) => m.bias = value,
            // Drive
            (Self::Drive(m), ModuleParam::DriveMode) => {
                m.mode = DriveMode::from_index(value.round().max(0.0) as usize)
            }
            (Self::Drive(m), ModuleParam::Drive) => m.drive = value,
            (Self::Drive(m), ModuleParam::Asymmetry) => m.asymmetry = value,
            (Self::Drive(m), ModuleParam::ToneHz) => m.tone_hz = value,
            (Self::Drive(m), ModuleParam::OutputDb) => m.output_db = value,
            (Self::Drive(m), ModuleParam::Mix) => m.mix = value,
            // LoFi
            (Self::LoFi(m), ModuleParam::Bits) => m.bits = value,
            (Self::LoFi(m), ModuleParam::SampleRateHz) => m.rate_hz = value,
            (Self::LoFi(m), ModuleParam::Mix) => m.mix = value,
            // Resonator
            (Self::Resonator(m), ModuleParam::ResonatorMode) => {
                m.mode = ResonatorMode::from_index(value.round().max(0.0) as usize)
            }
            (Self::Resonator(m), ModuleParam::VcoPitchTrack) => {
                m.pitch_tracking = PitchTracking::from_index(value.round().max(0.0) as usize)
            }
            (Self::Resonator(m), ModuleParam::Tune) => m.tune_semitones = value,
            (Self::Resonator(m), ModuleParam::Fine) => m.fine_cents = value,
            (Self::Resonator(m), ModuleParam::FixedHz) => m.fixed_hz = value,
            (Self::Resonator(m), ModuleParam::Structure) => m.structure = value,
            (Self::Resonator(m), ModuleParam::ResonatorDecay) => m.decay_ms = value,
            (Self::Resonator(m), ModuleParam::Brightness) => m.brightness = value,
            (Self::Resonator(m), ModuleParam::Position) => m.position = value,
            (Self::Resonator(m), ModuleParam::Mix) => m.mix = value,
            // Mobius
            (Self::Mobius(m), ModuleParam::MobiusExciter) => {
                m.exciter = MobiusExciter::from_index(value.round().max(0.0) as usize)
            }
            (Self::Mobius(m), ModuleParam::Brightness) => m.brightness = value,
            (Self::Mobius(m), ModuleParam::MobiusShape) => m.shape = value,
            (Self::Mobius(m), ModuleParam::MobiusChirality) => m.chirality = value,
            (Self::Mobius(m), ModuleParam::MobiusTexture) => m.texture = value,
            (Self::Mobius(m), ModuleParam::Attack) => m.attack_ms = value,
            (Self::Mobius(m), ModuleParam::Decay) => m.decay_ms = value,
            (Self::Mobius(m), ModuleParam::Sustain) => m.sustain = value,
            (Self::Mobius(m), ModuleParam::MobiusAtoms) => m.atoms = value.round() as u8,
            (Self::Mobius(m), ModuleParam::MobiusDispersion) => {
                m.dispersion = MobiusDispersion::from_index(value.round().max(0.0) as usize)
            }
            (Self::Mobius(m), ModuleParam::Structure) => m.structure = value,
            (Self::Mobius(m), ModuleParam::MobiusStiffness) => m.stiffness = value,
            (Self::Mobius(m), ModuleParam::MobiusStiffnessTilt) => m.stiffness_tilt = value,
            (Self::Mobius(m), ModuleParam::MobiusKerr) => m.kerr_semitones = value,
            (Self::Mobius(m), ModuleParam::MobiusKerrSpread) => m.kerr_spread = value,
            (Self::Mobius(m), ModuleParam::MobiusTopology) => {
                m.topology = MobiusTopology::from_index(value.round().max(0.0) as usize)
            }
            (Self::Mobius(m), ModuleParam::MobiusCoupling) => m.coupling = value,
            (Self::Mobius(m), ModuleParam::MobiusDistance) => m.distance_ms = value,
            (Self::Mobius(m), ModuleParam::MobiusSympathy) => m.sympathy = value,
            (Self::Mobius(m), ModuleParam::MobiusFeedback) => m.feedback = value,
            (Self::Mobius(m), ModuleParam::MobiusFbLock) => m.fb_lock = value >= 0.5,
            (Self::Mobius(m), ModuleParam::MobiusFbCycles) => m.fb_cycles = value,
            (Self::Mobius(m), ModuleParam::MobiusFbMs) => m.fb_ms = value,
            (Self::Mobius(m), ModuleParam::VcoPitchTrack) => {
                m.pitch_tracking = PitchTracking::from_index(value.round().max(0.0) as usize)
            }
            (Self::Mobius(m), ModuleParam::Tune) => m.tune_semitones = value,
            (Self::Mobius(m), ModuleParam::Fine) => m.fine_cents = value,
            (Self::Mobius(m), ModuleParam::FixedHz) => m.fixed_hz = value,
            (Self::Mobius(m), ModuleParam::KeyTrack) => m.key_tracking = value,
            (Self::Mobius(m), ModuleParam::Spread) => m.stereo_width = value,
            (Self::Mobius(m), ModuleParam::Mix) => m.mix = value,
            // Out
            (Self::Out(m), ModuleParam::GainDb) => m.gain_db = value,
            (Self::Out(m), ModuleParam::Pan) => m.pan = value,
            (Self::Out(m), ModuleParam::Spread) => m.spread = value,
            _ => bail!("Unsupported module parameter"),
        }
        Ok(())
    }

    pub fn read_normalized(&self, param: ModuleParam, total_frames: usize) -> Option<f32> {
        if let Self::Sampler(smp) = self {
            if param.is_frame_based() {
                let max_f = total_frames.max(smp.sample_end).max(smp.loop_end).max(1) as f32;
                let raw = smp.params().effective_frame(param, total_frames);
                return Some((raw as f32 / max_f).clamp(0.0, 1.0));
            }
        }
        Some(param.normalize(self.read_native(param)?))
    }

    pub fn write_normalized(&mut self, param: ModuleParam, norm: f32, total_frames: usize) -> bool {
        if let Self::Sampler(smp) = self {
            return smp.write_normalized(param, norm, total_frames);
        }
        if !self.supports_param(param) {
            return false;
        }
        let n = if norm.is_finite() {
            norm.clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.write_native(param, param.denormalize(n, total_frames))
            .is_ok()
    }

    pub fn format_param(&self, param: ModuleParam, total_frames: usize) -> Option<String> {
        if let Self::Sampler(smp) = self {
            if param.is_frame_based() {
                let eff = smp.params().effective_frame(param, total_frames);
                return Some(format!("{eff}"));
            }
        }
        Some(param.format_native(self.read_native(param)?, total_frames))
    }

    pub fn nudge_param(
        &mut self,
        param: ModuleParam,
        dir: i8,
        coarse: bool,
        total_frames: usize,
    ) -> Result<f32> {
        ensure!(
            self.supports_param(param),
            "{} module does not support parameter {}",
            self.label(),
            param.label()
        );
        if let Self::Sampler(smp) = self {
            let frame_cap = total_frames.max(1);
            let frame_step = if coarse {
                (frame_cap / 25).max(64)
            } else {
                (frame_cap / 250).max(8)
            };
            match param {
                ModuleParam::LoopMode => {
                    smp.loop_mode = smp.loop_mode.cycle(i32::from(dir));
                    if smp.loop_mode != SampleLoopMode::Off {
                        smp.one_shot = false;
                        if smp.loop_end <= smp.loop_start && total_frames > 1 {
                            smp.loop_end = total_frames;
                        }
                    }
                    return Ok(smp.loop_mode.to_index() as f32);
                }
                ModuleParam::SampleStart => {
                    let end = if smp.sample_end == 0 {
                        frame_cap
                    } else {
                        smp.sample_end
                    };
                    smp.sample_start = if dir >= 0 {
                        (smp.sample_start + frame_step).min(end.saturating_sub(1))
                    } else {
                        smp.sample_start.saturating_sub(frame_step)
                    };
                    return Ok(smp.sample_start as f32);
                }
                ModuleParam::SampleEnd => {
                    let cur = if smp.sample_end == 0 {
                        frame_cap
                    } else {
                        smp.sample_end
                    };
                    smp.sample_end = if dir >= 0 {
                        (cur + frame_step).min(frame_cap)
                    } else {
                        cur.saturating_sub(frame_step)
                            .max(smp.sample_start.saturating_add(1).min(frame_cap))
                    };
                    return Ok(smp.sample_end as f32);
                }
                ModuleParam::LoopStart => {
                    let end = if smp.loop_end == 0 {
                        frame_cap
                    } else {
                        smp.loop_end
                    };
                    smp.loop_start = if dir >= 0 {
                        (smp.loop_start + frame_step).min(end.saturating_sub(1))
                    } else {
                        smp.loop_start.saturating_sub(frame_step)
                    };
                    return Ok(smp.loop_start as f32);
                }
                ModuleParam::LoopEnd => {
                    let cur = if smp.loop_end == 0 {
                        frame_cap
                    } else {
                        smp.loop_end
                    };
                    smp.loop_end = if dir >= 0 {
                        (cur + frame_step).min(frame_cap)
                    } else {
                        cur.saturating_sub(frame_step)
                            .max(smp.loop_start.saturating_add(1).min(frame_cap))
                    };
                    return Ok(smp.loop_end as f32);
                }
                _ => {}
            }
        }
        let cur = self
            .read_native(param)
            .context("Missing module parameter")?;
        let next = param.nudge_native(cur, dir, coarse);
        self.write_native(param, next)?;
        Ok(next)
    }

    pub fn descriptors(&self, total_frames: usize) -> Vec<ParamDescriptor> {
        let group = self.tag().short_badge();
        self.params()
            .iter()
            .enumerate()
            .map(|(id, &param)| ParamDescriptor {
                id,
                module: group,
                name: param.label(),
                display: self
                    .format_param(param, total_frames)
                    .unwrap_or_else(|| "-".into()),
                normalized: self.read_normalized(param, total_frames).unwrap_or(0.0),
            })
            .collect()
    }
}

/// Authoritative native attenuverter range `(min, max)` for a [`crate::rack::PatchCable`]
/// terminating at `(target_kind, to_port)`.
#[inline]
pub const fn cable_amount_range(target_kind: ModuleKindTag, to_port: InPort) -> (f32, f32) {
    match (target_kind, to_port) {
        (_, InPort::In) => (-2.0, 2.0),
        (_, InPort::PitchCv) => (-48.0, 48.0),
        (_, InPort::FmIn) => (-1.0, 1.0),
        (_, InPort::PwCv) => (-0.45, 0.45),
        (_, InPort::TrigIn) => (-2.0, 2.0),
        (ModuleKindTag::Filter, InPort::Cv) => (-6.0, 6.0),
        (ModuleKindTag::Vca, InPort::Cv) => (-2.0, 2.0),
        (
            ModuleKindTag::Drive
            | ModuleKindTag::Resonator
            | ModuleKindTag::Mobius
            | ModuleKindTag::Out,
            InPort::Cv,
        ) => (-1.0, 1.0),
        _ => (-2.0, 2.0),
    }
}

/// Physical unit for displaying and parsing a patch cable's attenuverter depth.
#[inline]
pub const fn cable_amount_unit(target_kind: ModuleKindTag, to_port: InPort) -> Unit {
    match (target_kind, to_port) {
        (_, InPort::PitchCv) => Unit::Semitones,
        (ModuleKindTag::Filter, InPort::Cv) => Unit::Octaves,
        (_, InPort::TrigIn) => Unit::Plain,
        _ => Unit::Percent,
    }
}

/// Sensible initial attenuverter depth when creating a new cable to `(target_kind, to_port)`.
#[inline]
pub const fn cable_default_amount(target_kind: ModuleKindTag, to_port: InPort) -> f32 {
    match (target_kind, to_port) {
        (_, InPort::In) => 1.0,
        (_, InPort::PitchCv) => 12.0,
        (_, InPort::FmIn) => 0.35,
        (_, InPort::PwCv) => 0.20,
        (_, InPort::TrigIn) => 1.0,
        (ModuleKindTag::Filter, InPort::Cv) => 2.0,
        (ModuleKindTag::Vca, InPort::Cv) => 1.0,
        (ModuleKindTag::Drive | ModuleKindTag::Out, InPort::Cv) => 0.5,
        (ModuleKindTag::Resonator | ModuleKindTag::Mobius, InPort::Cv) => 0.35,
        _ => 1.0,
    }
}

#[inline]
pub fn normalize_cable_amount(target_kind: ModuleKindTag, to_port: InPort, amount: f32) -> f32 {
    let (lo, hi) = cable_amount_range(target_kind, to_port);
    if !amount.is_finite() {
        return 0.5;
    }
    ((amount - lo) / (hi - lo)).clamp(0.0, 1.0)
}

#[inline]
pub fn denormalize_cable_amount(target_kind: ModuleKindTag, to_port: InPort, norm: f32) -> f32 {
    let (lo, hi) = cable_amount_range(target_kind, to_port);
    let n = if norm.is_finite() {
        norm.clamp(0.0, 1.0)
    } else {
        0.5
    };
    (lo + n * (hi - lo)).clamp(lo, hi)
}

pub fn format_cable_amount(target_kind: ModuleKindTag, to_port: InPort, amount: f32) -> String {
    match cable_amount_unit(target_kind, to_port) {
        Unit::Semitones => format!("{amount:+.1} st"),
        Unit::Octaves => format!("{amount:+.2} oct"),
        Unit::Percent => format!("{:+.0}%", amount * 100.0),
        _ => format!("{amount:+.2}"),
    }
}

pub fn parse_cable_amount(target_kind: ModuleKindTag, to_port: InPort, input: &str) -> Result<f32> {
    let raw = input.trim();
    let lower = raw.to_ascii_lowercase().replace(' ', "");
    let unit = cable_amount_unit(target_kind, to_port);
    let (num, scale) = match unit {
        Unit::Semitones => (lower.strip_suffix("st").unwrap_or(&lower), 1.0),
        Unit::Octaves => (lower.strip_suffix("oct").unwrap_or(&lower), 1.0),
        Unit::Percent => {
            if let Some(v) = lower.strip_suffix('%') {
                (v, 0.01)
            } else {
                (lower.as_str(), 1.0)
            }
        }
        _ => (lower.as_str(), 1.0),
    };
    let val = num
        .parse::<f32>()
        .with_context(|| format!("Invalid cable amount '{raw}'"))?
        * scale;
    let (lo, hi) = cable_amount_range(target_kind, to_port);
    ensure!(
        val.is_finite() && (lo..=hi).contains(&val),
        "Cable amount must be finite and within {lo}..={hi}"
    );
    Ok(if val == 0.0 { 0.0 } else { val })
}

pub fn nudge_cable_amount(
    target_kind: ModuleKindTag,
    to_port: InPort,
    current: f32,
    dir: i8,
    coarse: bool,
) -> f32 {
    let (lo, hi) = cable_amount_range(target_kind, to_port);
    let d = f32::from(dir);
    let next = match cable_amount_unit(target_kind, to_port) {
        Unit::Semitones => {
            let step = if coarse { 12.0 } else { 1.0 };
            current.round() + d * step
        }
        Unit::Octaves => {
            let step_tw = if coarse { 10.0 } else { 1.0 };
            ((current * 20.0).round() + d * step_tw) / 20.0
        }
        Unit::Percent => {
            let step_pct = if coarse { 10.0 } else { 2.0 };
            ((current * 100.0).round() + d * step_pct) / 100.0
        }
        _ => {
            let step = if coarse { 0.25 } else { 0.05 };
            ((current * 20.0).round() + d * step * 20.0) / 20.0
        }
    };
    let clamped = next.clamp(lo, hi);
    if clamped.abs() < 1e-6 {
        0.0
    } else {
        clamped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::ChoiceEnum;

    fn assert_choice_contract<T: ChoiceEnum + std::fmt::Debug + serde::Serialize>() {
        let n = T::ALL.len();
        assert!(n > 0);
        for (i, &v) in T::ALL.iter().enumerate() {
            assert_eq!(v.to_index(), i);
            assert_eq!(T::from_index(i), v);
            assert_eq!(T::parse(v.slug()), Some(v));
            assert_eq!(T::parse(&v.slug().to_ascii_uppercase()), Some(v));
            assert_eq!(T::parse(v.label()), Some(v));
            assert_eq!(v.cycle(n as i32), v);
            assert_eq!(v.cycle(-(n as i32)), v);
            assert_eq!(v.cycle(1).cycle(-1), v);
            assert_eq!(
                serde_json::to_string(&v).unwrap(),
                format!("\"{}\"", v.slug())
            );
        }
        assert_eq!(T::from_index(n + 99), T::ALL[n - 1]);
        assert_eq!(T::ALL[0].cycle(-1), T::ALL[n - 1]);
        assert_eq!(T::ALL[n - 1].cycle(1), T::ALL[0]);
        assert_eq!(T::parse("___nonexistent_choice___"), None);
    }

    #[test]
    fn choice_enums_preserve_serde_wire_names_and_aliases() {
        assert_choice_contract::<ModuleKindTag>();
        assert_choice_contract::<SampleLoopMode>();
        assert_choice_contract::<EnvMode>();
        assert_choice_contract::<MobiusExciter>();
        assert_choice_contract::<MobiusDispersion>();
        assert_choice_contract::<MobiusTopology>();

        assert_eq!(ModuleKindTag::parse("mbs"), Some(ModuleKindTag::Mobius));
        assert_eq!(ModuleKindTag::Mobius.short_badge(), "MBS");

        assert_eq!(ModuleKindTag::parse("oscillator"), Some(ModuleKindTag::Vco));
        assert_eq!(
            ModuleKindTag::parse("bitcrusher"),
            Some(ModuleKindTag::LoFi)
        );
        assert_eq!(
            serde_json::to_string(&ModuleKindTag::LoFi).unwrap(),
            "\"lofi\""
        );
        assert_eq!(SampleLoopMode::parse("oneshot"), Some(SampleLoopMode::Off));
        assert_eq!(SampleLoopMode::parse("fwd"), Some(SampleLoopMode::Forward));
        assert_eq!(SampleLoopMode::parse("pp"), Some(SampleLoopMode::PingPong));
        assert_eq!(EnvMode::parse("0"), Some(EnvMode::Adsr));
        assert_eq!(EnvMode::parse("perc"), Some(EnvMode::HoldDecay));
    }

    #[test]
    fn sampler_params_spot_delegation_and_flat_serde_contract() {
        let mut module = SamplerModule {
            path: Some("drums/kick.wav".into()),
            root_midi: 48,
            coarse_semitones: -5,
            fine_cents: 12.5,
            sample_start: 100,
            sample_end: 4000,
            loop_mode: SampleLoopMode::PingPong,
            loop_start: 500,
            loop_end: 3000,
            loop_xfade_ms: 8.5,
            one_shot: false,
            level: 0.85,
        };

        // Flat wire JSON format and strict `deny_unknown_fields` preserved.
        let json = serde_json::to_string(&module).unwrap();
        let val: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(val["path"], "drums/kick.wav");
        assert_eq!(val["root_midi"], 48);
        assert_eq!(val["loop_mode"], "ping_pong");
        let roundtrip: SamplerModule = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtrip, module);
        assert!(serde_json::from_str::<SamplerModule>(r#"{"unknown_field": 1}"#).is_err());

        // `SamplerParams` Copy snapshot & region extraction.
        let mut p: SamplerParams = module.params();
        let copy_check: SamplerParams = p;
        assert_eq!(p, copy_check);
        assert_eq!(p.region().sample_start, 100);
        assert_eq!(p.region().loop_mode, SampleLoopMode::PingPong);
        assert_eq!(p.read_native(ModuleParam::RootMidi), Some(48.0));
        assert_eq!(p.read_native(ModuleParam::Cutoff), None);

        // `write_native` & `write_normalized` round-trip via `apply_params` preserving `path`.
        p.write_native(ModuleParam::CoarseTune, 7.0).unwrap();
        assert!(p.write_normalized(ModuleParam::SampleStart, 0.25, 4000));
        assert!(!p.write_normalized(ModuleParam::Cutoff, 0.5, 4000));
        module.apply_params(p);
        assert_eq!(module.path.as_deref(), Some("drums/kick.wav"));
        assert_eq!(module.coarse_semitones, 7);
        assert_eq!(module.sample_start, 1000);
        assert_eq!(module.read_native(ModuleParam::CoarseTune), Some(7.0));
    }
}
