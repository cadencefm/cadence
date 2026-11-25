//! Core Modular Rack domain types, shared enums, zone/module/cable containers,
//! sample-bank key derivation, and unified parameter access.

use crate::params::{InstrumentParam, RackParam, ZoneParam};
use crate::rack::modules::{
    cable_amount_range, format_cable_amount, nudge_cable_amount, parse_cable_amount, ModuleKind,
    ModuleKindTag, SamplerModule,
};
use crate::rack::params::ModuleParam;
use crate::sampler::{parse_filename_midi, ParamDescriptor, SampleData, SampleMetadata};
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const MAX_ZONES: usize = 32;
pub const MAX_CHOKE_GROUPS: u8 = 8;
pub const MAX_MODULES_PER_ZONE: usize = 24;
pub const MAX_RESONATORS_PER_ZONE: usize = 4;
pub const MAX_MOBIUS_PER_ZONE: usize = 2;
pub const MAX_CABLES_PER_ZONE: usize = 64;
pub const MAX_PANEL_PINS: usize = 16;

/// Standard MIDI note map shared across factory Kit presets and song importers.
pub mod drum_midi {
    pub const BD: u8 = 36; // C2  - Bass Drum / Kick
    pub const SD: u8 = 37; // C#2 - Snare Drum
    pub const LT: u8 = 38; // D2  - Low Tom
    pub const MT: u8 = 39; // D#2 - Mid Tom
    pub const HT: u8 = 40; // E2  - High Tom
    pub const RS: u8 = 41; // F2  - Rimshot / Clave
    pub const CH: u8 = 42; // F#2 - Closed Hi-Hat (choke group 1)
    pub const CP: u8 = 43; // G2  - Handclap
    pub const CB_OR_CR: u8 = 44; // G#2 - 808 Cowbell / 909 & EDM Crash
    pub const CY_OR_RD: u8 = 45; // A2  - 808 Cymbal / 909 & EDM Ride
    pub const OH: u8 = 46; // A#2 - Open Hi-Hat (choke group 1)
    pub const LC: u8 = 47; // B2  - 808 Low Conga
    pub const MC: u8 = 48; // C3  - 808 Mid Conga
    pub const HC: u8 = 49; // C#3 - 808 High Conga
    pub const CL: u8 = 50; // D3  - 808 Claves
    pub const MA: u8 = 51; // D#3 - 808 Maracas

    /// Map standard drum voice names (`BD`, `SD`, `CH`, `OH`, `Kick`, `Snare`, etc.)
    /// to canonical MIDI note and default choke group, falling back to the next free
    /// note in `36..=127` when a voice name is unrecognized or already taken.
    pub fn infer_mapping(channel_name: &str, used_notes: &[u8]) -> (u8, Option<u8>) {
        let lower = channel_name.trim().to_ascii_lowercase();
        let tokens: Vec<&str> = lower
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|t| !t.is_empty())
            .collect();
        let has = |tok: &str| tokens.contains(&tok);

        let mapped = if has("bd") || has("kick") || has("bassdrum") {
            Some((BD, None))
        } else if has("sd") || has("snare") {
            Some((SD, None))
        } else if has("rs") || has("rim") || has("rimshot") {
            Some((RS, None))
        } else if has("cp") || has("clap") || has("handclap") {
            Some((CP, None))
        } else if has("lt") || (has("low") && has("tom")) {
            Some((LT, None))
        } else if has("mt") || (has("mid") && has("tom")) {
            Some((MT, None))
        } else if has("ht") || ((has("hi") || has("high")) && has("tom")) {
            Some((HT, None))
        } else if has("ch") || (has("closed") && has("hat")) {
            Some((CH, Some(1)))
        } else if has("oh") || (has("open") && has("hat")) {
            Some((OH, Some(1)))
        } else if has("cb") || has("cowbell") || has("cr") || has("crash") {
            Some((CB_OR_CR, None))
        } else if has("cy") || has("cymbal") || has("rd") || has("ride") {
            Some((CY_OR_RD, None))
        } else if has("lc") || (has("low") && has("conga")) {
            Some((LC, None))
        } else if has("mc") || (has("mid") && has("conga")) {
            Some((MC, None))
        } else if has("hc") || ((has("hi") || has("high")) && has("conga")) {
            Some((HC, None))
        } else if has("cl") || has("clave") || has("claves") {
            Some((CL, None))
        } else if has("ma") || has("maraca") || has("maracas") {
            Some((MA, None))
        } else {
            None
        };

        if let Some((midi, choke)) = mapped {
            if !used_notes.contains(&midi) {
                return (midi, choke);
            }
        }
        for candidate in 36u8..=127 {
            if !used_notes.contains(&candidate) {
                return (candidate, None);
            }
        }
        (60, None)
    }
}

/// Collision-free `SampleBank` lookup key for a `Rack` zone's `Sampler` module.
///
/// Uses bit 63 as sentinel, bits `62..32` for the instrument ID's low 31 bits,
/// bits `31..16` for stable `zone_id`, and bits `15..0` for stable `module_id`.
#[inline]
pub fn module_sample_key(inst_id: u64, zone_id: u16, module_id: u16) -> u64 {
    (1_u64 << 63)
        | ((inst_id & 0x7FFF_FFFF) << 32)
        | (u64::from(zone_id) << 16)
        | u64::from(module_id)
}

crate::define_choice_enum! {
    #[derive(Default)]
    pub enum Waveform {
        Sine => ("sine", "Sine", ["0", "sin"]),
        Triangle => ("triangle", "Triangle", ["1", "tri"]),
        #[default]
        Saw => ("saw", "Saw", ["2", "sawtooth"]),
        Pulse => ("pulse", "Pulse", ["3", "square", "sqr"]),
    }
}

crate::define_choice_enum! {
    #[derive(Default)]
    pub enum FilterMode {
        #[default]
        LowPass => ("low_pass", "Low-pass", ["0", "lp", "lp12", "lp_12", "lowpass", "low_pass_12", "lowpass12"]),
        BandPass => ("band_pass", "Band-pass", ["1", "bp", "bandpass"]),
        HighPass => ("high_pass", "High-pass", ["2", "hp", "highpass"]),
        AcidLowPass => ("acid_low_pass", "Acid LP", ["3", "acid", "303", "acid_lp", "acidlp"]),
        #[serde(rename = "low_pass_6")]
        LowPass6 => ("low_pass_6", "LP 6dB", ["4", "lp6", "lp_6", "lp6db", "lp_6db", "lowpass6", "low_pass6", "1pole", "one_pole", "6db"]),
    }
}

crate::define_choice_enum! {
    /// Physical modal resonator geometry for [`crate::rack::ResonatorModule`].
    #[derive(Default)]
    pub enum ResonatorMode {
        #[default]
        String => ("string", "String", ["0", "str", "pluck", "wire"]),
        Bar => ("bar", "Bar", ["1", "marimba", "beam", "chime", "vibe"]),
        Membrane => ("membrane", "Membrane", ["2", "mem", "drum", "head", "skin"]),
        Plate => ("plate", "Plate", ["3", "plt", "cymbal", "gong", "bell", "metal"]),
    }
}

crate::define_choice_enum! {
    /// Complex unit-disk target source / excitation family for [`crate::rack::MobiusModule`].
    #[derive(Default)]
    pub enum MobiusExciter {
        Szego => ("szego", "Szego", ["0", "monopole", "saw", "horn"]),
        #[default]
        Dipole => ("dipole", "Dipole", ["1", "pwm", "clarinet", "brass"]),
        Poisson => ("poisson", "Poisson", ["2", "sync", "formant", "bell"]),
        Strike => ("strike", "Strike", ["3", "mallet", "hit", "pluck", "impulse"]),
        External => ("external", "External", ["4", "ext", "audio", "in"]),
    }
}

crate::define_choice_enum! {
    /// Inter-atom frequency dispersion law for [`crate::rack::MobiusModule`].
    #[derive(Default)]
    pub enum MobiusDispersion {
        #[default]
        Harmonic => ("harmonic", "Harmonic", ["0", "harm", "string", "overtone"]),
        Bar => ("bar", "Bar", ["1", "beam", "bell", "marimba"]),
        Cluster => ("cluster", "Cluster", ["2", "detune", "unison", "chorus"]),
        Formant => ("formant", "Formant", ["3", "vowel", "vocal", "choir"]),
        Ratio => ("ratio", "Ratio", ["4", "geom", "interval", "golden"]),
    }
}

crate::define_choice_enum! {
    /// Pure target-coupling space-time network topology for [`crate::rack::MobiusModule`].
    #[derive(Default)]
    pub enum MobiusTopology {
        #[default]
        Cascade => ("cascade", "Cascade", ["0", "peel", "serial", "vocal"]),
        Bloom => ("bloom", "Bloom", ["1", "flux", "shimmer", "chain"]),
        Star => ("star", "Star", ["2", "chamber", "bridge", "body"]),
        Ring => ("ring", "Ring", ["3", "loop", "spatial", "room"]),
        Swarm => ("swarm", "Swarm", ["4", "mean", "kuramoto", "chorus"]),
        Hybrid => ("hybrid", "Hybrid", ["5", "bell", "doublet"]),
    }
}

crate::define_choice_enum! {
    #[derive(Default)]
    pub enum LfoShape {
        #[default]
        Sine => ("sine", "Sine", ["0", "sin"]),
        Triangle => ("triangle", "Triangle", ["1", "tri"]),
        Saw => ("saw", "Saw", ["2", "sawtooth"]),
        Square => ("square", "Square", ["3", "sqr", "pulse"]),
        SampleHold => ("sample_hold", "Sample/Hold", ["4", "samplehold", "sh", "s_h", "random"]),
    }
}

crate::define_choice_enum! {
    #[derive(Default)]
    pub enum VoiceMode {
        #[default]
        Poly => ("poly", "Poly", ["0", "polyphonic"]),
        Mono => ("mono", "Mono", ["1", "monophonic"]),
    }
}

crate::define_choice_enum! {
    #[derive(Default)]
    pub enum PitchTracking {
        #[default]
        Chromatic => ("chromatic", "Chromatic", ["0", "track", "key", "note"]),
        Fixed => ("fixed", "Fixed", ["1", "const", "constant"]),
    }
}

crate::define_choice_enum! {
    #[derive(Default)]
    pub enum TriggerMode {
        #[default]
        Gate => ("gate", "Gate", ["0", "adsr"]),
        OneShot => ("one_shot", "One-shot", ["1", "oneshot", "perc", "trigger"]),
    }
}

crate::define_choice_enum! {
    #[derive(Default)]
    pub enum NoiseColor {
        #[default]
        White => ("white", "White", ["0", "w"]),
        Pink => ("pink", "Pink", ["1", "p"]),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdatePolicy {
    Continuous,
    OnTrigger,
    Structural,
}

impl UpdatePolicy {
    pub fn label(self) -> &'static str {
        match self {
            Self::Continuous => "LIVE",
            Self::OnTrigger => "TRIG",
            Self::Structural => "VOICE",
        }
    }
}

/// Physical unit shared across Rack, Zone, Module, and Cable parameter formatting/parsing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Plain,
    Percent,
    Hz,
    Ms,
    Semitones,
    Cents,
    Degrees,
    Octaves,
    Note,
    Db,
    Pan,
    Bits,
}

crate::define_choice_enum! {
    /// Neutral bare engine templates for `:new <template>` and bare default presets.
    pub enum RackTemplate {
        Synth => ("synth", "Synth", ["syn", "init", "init_synth"]),
        Sampler => ("sampler", "Sampler", ["smp", "sample", "blank_sampler"]),
        Kit => ("kit", "Kit", ["drums", "blank_kit"]),
        Modular => ("modular", "Modular", ["mod", "blank", "blank_modular", "rack"]),
    }
}

impl RackTemplate {
    pub fn default_preset_name(self) -> &'static str {
        match self {
            Self::Synth => "Init Synth",
            Self::Sampler => "Blank Sampler",
            Self::Kit => "Blank Kit",
            Self::Modular => "Blank Modular",
        }
    }
}

crate::define_choice_enum! {
    /// Input port on a Eurorack [`RackModule`].
    pub enum InPort {
        /// Stereo audio input (multiple incoming cables sum).
        In => ("in", "Audio In", ["audio", "audio_in", "sig"]),
        /// Modulation / control CV (Filter cutoff octaves, VCA gain/RingMod, Drive CV, Out CV).
        Cv => ("cv", "CV", ["mod", "cutoff", "gain", "amp"]),
        /// Pitch modulation in semitones (VCO, Sampler).
        PitchCv => ("pitch_cv", "Pitch CV", ["pitch", "v_oct", "voct", "freq"]),
        /// Audio-rate through-zero linear FM (VCO).
        FmIn => ("fm_in", "FM In", ["fm", "lin_fm"]),
        /// Pulse-width modulation (VCO).
        PwCv => ("pw_cv", "PW CV", ["pw", "pwm"]),
        /// Gate / trigger input (Env, Sampler; defaults to voice gate if unpatched).
        TrigIn => ("trig_in", "Trig In", ["trig", "gate", "trigger"]),
    }
}

impl InPort {
    pub fn short_label(self) -> &'static str {
        match self {
            Self::In => "in",
            Self::Cv => "cv",
            Self::PitchCv => "pitch",
            Self::FmIn => "fm",
            Self::PwCv => "pw",
            Self::TrigIn => "trig",
        }
    }
}

crate::define_choice_enum! {
    /// Output port on a Eurorack [`RackModule`].
    #[derive(Default)]
    pub enum OutPort {
        /// Stereo `[f64; 2]` signal (mono modules write `[x, x]`).
        #[default]
        Out => ("out", "Out"),
    }
}

fn default_true() -> bool {
    true
}

fn default_one() -> f32 {
    1.0
}

fn default_cutoff() -> f32 {
    20_000.0
}

fn default_out_port() -> OutPort {
    OutPort::Out
}

/// Per-zone polyphony, legato, glide, pitch-tracking, and oscillator phase-reset settings.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoiceSettings {
    #[serde(default)]
    pub voice_mode: VoiceMode,
    #[serde(default)]
    pub legato: bool,
    #[serde(default)]
    pub glide_ms: f32,
    #[serde(default)]
    pub pitch_tracking: PitchTracking,
    #[serde(default = "default_true")]
    pub phase_reset: bool,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        Self {
            voice_mode: VoiceMode::Poly,
            legato: false,
            glide_ms: 0.0,
            pitch_tracking: PitchTracking::Chromatic,
            phase_reset: true,
        }
    }
}

impl VoiceSettings {
    pub const MIN_GLIDE_MS: f32 = 0.0;
    pub const MAX_GLIDE_MS: f32 = 2_000.0;

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.glide_ms.is_finite()
                && (Self::MIN_GLIDE_MS..=Self::MAX_GLIDE_MS).contains(&self.glide_ms),
            "Voice glide_ms must be finite and within {}..={}",
            Self::MIN_GLIDE_MS,
            Self::MAX_GLIDE_MS
        );
        Ok(())
    }
}

/// Directed audio/CV patch cable with inline bipolar attenuverter `amount`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchCable {
    pub id: u16,
    pub from_module: u16,
    #[serde(default = "default_out_port")]
    pub from_port: OutPort,
    pub to_module: u16,
    pub to_port: InPort,
    /// Bipolar attenuverter depth (audio gain, pitch semitones, cutoff octaves, etc.).
    pub amount: f32,
}

impl PatchCable {
    pub fn new(id: u16, from_module: u16, to_module: u16, to_port: InPort, amount: f32) -> Self {
        Self {
            id,
            from_module,
            from_port: OutPort::Out,
            to_module,
            to_port,
            amount,
        }
    }
}

/// One curated quick-control pinned to the Rack's top-level Performance Panel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PanelPin {
    pub target: InstrumentParam,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
}

impl PanelPin {
    pub fn module(zone: u16, module: u16, param: ModuleParam, label: impl Into<String>) -> Self {
        Self {
            target: InstrumentParam::Module {
                zone,
                module,
                param,
            },
            label: label.into(),
        }
    }

    pub fn cable(zone: u16, cable: u16, label: impl Into<String>) -> Self {
        Self {
            target: InstrumentParam::Cable { zone, cable },
            label: label.into(),
        }
    }

    pub fn zone(zone: u16, param: ZoneParam, label: impl Into<String>) -> Self {
        Self {
            target: InstrumentParam::Zone { zone, param },
            label: label.into(),
        }
    }

    pub fn rack(param: RackParam, label: impl Into<String>) -> Self {
        Self {
            target: InstrumentParam::Rack { param },
            label: label.into(),
        }
    }

    pub fn display_label(&self) -> &str {
        if self.label.trim().is_empty() {
            self.target.short_label()
        } else {
            self.label.as_str()
        }
    }
}

/// One Eurorack module instance inside a [`RackZone`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RackModule {
    pub id: u16,
    pub name: String,
    pub kind: ModuleKind,
}

impl RackModule {
    pub fn new(id: u16, name: impl Into<String>, kind: ModuleKind) -> Self {
        Self {
            id,
            name: name.into(),
            kind,
        }
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(self.id > 0, "Module ID must be nonzero");
        let trimmed = self.name.trim();
        ensure!(
            !trimmed.is_empty()
                && trimmed.chars().count() <= 32
                && !self.name.chars().any(char::is_control),
            "Module name must be 1..=32 printable characters"
        );
        self.kind
            .validate()
            .with_context(|| format!("Module '{}' (#{})", self.name, self.id))
    }
}

/// One key/velocity-split zone inside a [`RackPatch`] containing a modular patchbay.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RackZone {
    pub id: u16,
    pub name: String,
    pub low_midi: u8,
    pub high_midi: u8,
    pub root_midi: u8,
    #[serde(default)]
    pub low_vel: f32,
    #[serde(default = "default_one")]
    pub high_vel: f32,
    #[serde(default)]
    pub gain_db: f32,
    #[serde(default)]
    pub pan: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choke_group: Option<u8>,
    #[serde(default)]
    pub voice: VoiceSettings,
    pub modules: Vec<RackModule>,
    #[serde(default)]
    pub cables: Vec<PatchCable>,
}

impl RackZone {
    pub const MIN_GAIN_DB: f32 = -36.0;
    pub const MAX_GAIN_DB: f32 = 12.0;
    pub const MIN_PAN: f32 = -1.0;
    pub const MAX_PAN: f32 = 1.0;

    /// Blank full-range sample zone (`[Sampler -> Out]`, `0..=127`, root `C4`).
    pub fn blank_sample(id: u16, name: impl Into<String>) -> Self {
        Self {
            id: id.max(1),
            name: name.into(),
            low_midi: 0,
            high_midi: 127,
            root_midi: 60,
            low_vel: 0.0,
            high_vel: 1.0,
            gain_db: 0.0,
            pan: 0.0,
            choke_group: None,
            voice: VoiceSettings::default(),
            modules: vec![
                RackModule::new(1, "Sampler", ModuleKind::Sampler(SamplerModule::default())),
                RackModule::new(2, "Out", ModuleKind::Out(Default::default())),
            ],
            cables: vec![PatchCable::new(1, 1, 2, InPort::In, 1.0)],
        }
    }

    /// Single-MIDI-note drum/sample pad (`[Sampler -> Out]`).
    pub fn sample_pad(id: u16, name: impl Into<String>, midi: u8, path: impl Into<String>) -> Self {
        let midi = midi.min(127);
        let mut zone = Self::blank_sample(id, name);
        zone.low_midi = midi;
        zone.high_midi = midi;
        zone.root_midi = midi;
        if let Some(ModuleKind::Sampler(smp)) = zone.modules.first_mut().map(|m| &mut m.kind) {
            smp.path = Some(path.into());
            smp.root_midi = midi;
        }
        zone
    }

    /// Construct a validated sample zone from decoded RIFF `smpl`/`inst` metadata.
    pub fn from_sample_metadata(
        id: u16,
        name: impl Into<String>,
        rel_path: impl Into<String>,
        meta: &SampleMetadata,
        frames: usize,
        fallback_midi: u8,
    ) -> Self {
        let root = if meta.has_root {
            meta.root_midi.min(127)
        } else {
            fallback_midi.min(127)
        };
        let raw_name = name.into();
        let trimmed = raw_name.trim();
        let clean_name: String = if trimmed.is_empty() {
            "Zone".into()
        } else {
            trimmed.chars().take(32).collect()
        };
        let mut zone = Self::blank_sample(id, clean_name);
        zone.low_midi = root;
        zone.high_midi = root;
        zone.root_midi = root;
        zone.apply_sample_metadata(Some(1), rel_path.into(), meta, frames, root);
        zone
    }

    /// Apply decoded RIFF `smpl`/`inst` metadata onto the target `Sampler` module
    /// (or the first `Sampler` module in this zone if `module_id` is `None`),
    /// updating zone key/velocity bounds when present.
    pub fn apply_sample_metadata(
        &mut self,
        module_id: Option<u16>,
        rel_path: String,
        meta: &SampleMetadata,
        frames: usize,
        fallback_midi: u8,
    ) -> bool {
        let root = if meta.has_root {
            meta.root_midi.min(127)
        } else {
            fallback_midi.min(127)
        };
        let target = if let Some(mid) = module_id {
            self.modules
                .iter_mut()
                .find(|m| m.id == mid)
                .and_then(|m| match &mut m.kind {
                    ModuleKind::Sampler(s) => Some(s),
                    _ => None,
                })
        } else {
            self.modules.iter_mut().find_map(|m| match &mut m.kind {
                ModuleKind::Sampler(s) => Some(s),
                _ => None,
            })
        };
        let Some(smp) = target else {
            return false;
        };
        smp.apply_metadata(rel_path, meta, frames, root);
        self.root_midi = root;
        if let (Some(lo), Some(hi)) = (meta.low_midi, meta.high_midi) {
            if lo <= hi && hi <= 127 {
                self.low_midi = lo;
                self.high_midi = hi;
            }
        }
        if let (Some(lo_v), Some(hi_v)) = (meta.low_vel, meta.high_vel) {
            if lo_v <= hi_v && hi_v <= 127 && hi_v > 0 {
                let low = if lo_v <= 1 {
                    0.0
                } else {
                    (f32::from(lo_v) / 127.0).clamp(0.0, 1.0)
                };
                let high = if hi_v >= 127 {
                    1.0
                } else {
                    (f32::from(hi_v) / 127.0).clamp(0.0, 1.0)
                };
                self.set_vel_bounds(low, high);
            }
        }
        true
    }

    /// Apply decoded `SampleData` onto the target (or first) `Sampler` module in this zone.
    pub fn apply_sample_defaults(
        &mut self,
        module_id: Option<u16>,
        path_label: String,
        sample: &SampleData,
    ) -> bool {
        let fallback = parse_filename_midi(&sample.name)
            .or_else(|| parse_filename_midi(&path_label))
            .unwrap_or(self.root_midi);
        self.apply_sample_metadata(module_id, path_label, &sample.meta, sample.frames, fallback)
    }

    pub fn module(&self, id: u16) -> Option<&RackModule> {
        self.modules.iter().find(|m| m.id == id)
    }

    pub fn module_mut(&mut self, id: u16) -> Option<&mut RackModule> {
        self.modules.iter_mut().find(|m| m.id == id)
    }

    pub fn cable(&self, id: u16) -> Option<&PatchCable> {
        self.cables.iter().find(|c| c.id == id)
    }

    pub fn cable_mut(&mut self, id: u16) -> Option<&mut PatchCable> {
        self.cables.iter_mut().find(|c| c.id == id)
    }

    pub fn out_module(&self) -> Option<&RackModule> {
        self.modules
            .iter()
            .find(|m| matches!(m.kind, ModuleKind::Out(_)))
    }

    pub fn next_module_id(&self) -> u16 {
        self.modules
            .iter()
            .map(|m| m.id)
            .max()
            .unwrap_or(0)
            .saturating_add(1)
    }

    pub fn next_cable_id(&self) -> u16 {
        self.cables
            .iter()
            .map(|c| c.id)
            .max()
            .unwrap_or(0)
            .saturating_add(1)
    }

    /// Resolve a module token (`m3`, `3`, module name, or unique kind tag like `filter`/`vca`/`out`)
    /// into its stable `module_id`.
    pub fn resolve_module_id(&self, token: &str) -> Option<u16> {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            return None;
        }
        let stripped = trimmed
            .strip_prefix('m')
            .or_else(|| trimmed.strip_prefix('M'))
            .unwrap_or(trimmed);
        if !stripped.is_empty() && stripped.bytes().all(|b| b.is_ascii_digit()) {
            if let Ok(id) = stripped.parse::<u16>() {
                if self.module(id).is_some() {
                    return Some(id);
                }
            }
        }
        let norm = trimmed.to_ascii_lowercase().replace([' ', '-'], "_");
        if let Some(m) = self.modules.iter().find(|m| {
            m.name.eq_ignore_ascii_case(trimmed)
                || m.name.to_ascii_lowercase().replace([' ', '-'], "_") == norm
        }) {
            return Some(m.id);
        }
        if let Some(tag) = ModuleKindTag::parse(trimmed) {
            let mut matches = self.modules.iter().filter(|m| m.kind.tag() == tag);
            if let Some(first) = matches.next() {
                if matches.next().is_none() {
                    return Some(first.id);
                }
            }
        }
        let lower = trimmed.to_ascii_lowercase();
        let mut prefix_matches = self
            .modules
            .iter()
            .filter(|m| m.name.to_ascii_lowercase().starts_with(&lower));
        if let Some(first) = prefix_matches.next() {
            if prefix_matches.next().is_none() {
                return Some(first.id);
            }
        }
        None
    }

    pub fn set_name(&mut self, name: impl AsRef<str>) -> Result<()> {
        let trimmed = name.as_ref().trim();
        ensure!(
            !trimmed.is_empty()
                && trimmed.chars().count() <= 32
                && !trimmed.chars().any(char::is_control),
            "Zone name must be 1..=32 printable characters"
        );
        self.name = trimmed.to_string();
        Ok(())
    }

    pub fn set_low_midi(&mut self, midi: u8) {
        self.low_midi = midi.min(127);
        if self.low_midi > self.high_midi {
            self.high_midi = self.low_midi;
        }
    }

    pub fn set_high_midi(&mut self, midi: u8) {
        self.high_midi = midi.min(127);
        if self.high_midi < self.low_midi {
            self.low_midi = self.high_midi;
        }
    }

    pub fn set_root_midi(&mut self, midi: u8) {
        let clamped = midi.min(127);
        self.root_midi = clamped;
        for m in &mut self.modules {
            if let ModuleKind::Sampler(smp) = &mut m.kind {
                smp.root_midi = clamped;
            }
        }
    }

    pub fn set_midi_bounds(&mut self, low: u8, high: u8, root: Option<u8>) {
        let a = low.min(127);
        let b = high.min(127);
        self.low_midi = a.min(b);
        self.high_midi = a.max(b);
        if let Some(r) = root {
            self.set_root_midi(r);
        }
    }

    pub fn set_low_vel(&mut self, vel: f32) {
        let v = if vel.is_finite() {
            vel.clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.low_vel = v;
        if self.low_vel > self.high_vel {
            self.high_vel = self.low_vel;
        }
    }

    pub fn set_high_vel(&mut self, vel: f32) {
        let v = if vel.is_finite() {
            vel.clamp(0.0, 1.0)
        } else {
            1.0
        };
        self.high_vel = v;
        if self.high_vel < self.low_vel {
            self.low_vel = self.high_vel;
        }
    }

    pub fn set_vel_bounds(&mut self, low: f32, high: f32) {
        let a = if low.is_finite() {
            low.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let b = if high.is_finite() {
            high.clamp(0.0, 1.0)
        } else {
            1.0
        };
        self.low_vel = a.min(b);
        self.high_vel = a.max(b);
    }

    pub fn set_choke_group(&mut self, group: Option<u8>) -> Result<()> {
        if let Some(g) = group {
            ensure!(
                (1..=MAX_CHOKE_GROUPS).contains(&g),
                "Choke group must be within 1..={MAX_CHOKE_GROUPS}"
            );
        }
        self.choke_group = group;
        Ok(())
    }

    #[inline]
    pub fn matches(&self, midi: u8, velocity: f32) -> bool {
        if !velocity.is_finite() || midi < self.low_midi || midi > self.high_midi {
            return false;
        }
        let v = velocity.clamp(0.0, 1.0);
        if (self.high_vel - self.low_vel).abs() <= 1e-6 {
            return (v - self.low_vel).abs() <= 1e-5;
        }
        let above_low = v >= self.low_vel;
        let below_high = if self.high_vel >= 1.0 - 1e-5 {
            v <= 1.0
        } else {
            v < self.high_vel
        };
        above_low && below_high
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(self.id > 0, "Zone ID must be nonzero");
        let trimmed = self.name.trim();
        ensure!(
            !trimmed.is_empty()
                && trimmed.chars().count() <= 32
                && !self.name.chars().any(char::is_control),
            "Zone name must be 1..=32 printable characters"
        );
        ensure!(
            self.low_midi <= self.high_midi && self.high_midi <= 127 && self.root_midi <= 127,
            "Zone '{}' MIDI range must satisfy 0 <= low <= high <= 127",
            self.name
        );
        ensure!(
            self.low_vel.is_finite()
                && self.high_vel.is_finite()
                && self.low_vel >= 0.0
                && self.high_vel <= 1.0
                && self.low_vel <= self.high_vel,
            "Zone '{}' velocity range must satisfy 0 <= low_vel <= high_vel <= 1",
            self.name
        );
        ensure!(
            self.gain_db.is_finite()
                && (Self::MIN_GAIN_DB..=Self::MAX_GAIN_DB).contains(&self.gain_db),
            "Zone '{}' gain_db must be within {}..={} dB",
            self.name,
            Self::MIN_GAIN_DB,
            Self::MAX_GAIN_DB
        );
        ensure!(
            self.pan.is_finite() && (Self::MIN_PAN..=Self::MAX_PAN).contains(&self.pan),
            "Zone '{}' pan must be within {}..={}",
            self.name,
            Self::MIN_PAN,
            Self::MAX_PAN
        );
        if let Some(group) = self.choke_group {
            ensure!(
                (1..=MAX_CHOKE_GROUPS).contains(&group),
                "Zone '{}' choke_group must be within 1..={MAX_CHOKE_GROUPS}",
                self.name
            );
        }
        self.voice
            .validate()
            .with_context(|| format!("Zone '{}'", self.name))?;

        ensure!(
            !self.modules.is_empty() && self.modules.len() <= MAX_MODULES_PER_ZONE,
            "Zone '{}' must contain 1..={MAX_MODULES_PER_ZONE} modules",
            self.name
        );
        ensure!(
            self.cables.len() <= MAX_CABLES_PER_ZONE,
            "Zone '{}' exceeds maximum of {MAX_CABLES_PER_ZONE} patch cables",
            self.name
        );

        let mut mod_ids = HashSet::with_capacity(self.modules.len());
        let mut out_count = 0usize;
        let mut resonator_count = 0usize;
        let mut mobius_count = 0usize;
        for m in &self.modules {
            m.validate()
                .with_context(|| format!("Zone '{}'", self.name))?;
            ensure!(
                mod_ids.insert(m.id),
                "Zone '{}' has duplicate module ID {}",
                self.name,
                m.id
            );
            if matches!(m.kind, ModuleKind::Out(_)) {
                out_count += 1;
            }
            if matches!(m.kind, ModuleKind::Resonator(_)) {
                resonator_count += 1;
            }
            if matches!(m.kind, ModuleKind::Mobius(_)) {
                mobius_count += 1;
            }
        }
        ensure!(
            out_count == 1,
            "Zone '{}' must contain exactly one Out module (found {out_count})",
            self.name
        );
        ensure!(
            resonator_count <= MAX_RESONATORS_PER_ZONE,
            "Zone '{}' exceeds maximum of {MAX_RESONATORS_PER_ZONE} Resonator modules (found {resonator_count})",
            self.name
        );
        ensure!(
            mobius_count <= MAX_MOBIUS_PER_ZONE,
            "Zone '{}' exceeds maximum of {MAX_MOBIUS_PER_ZONE} Mobius modules (found {mobius_count})",
            self.name
        );

        let mut cable_ids = HashSet::with_capacity(self.cables.len());
        let mut seen_edges = HashSet::with_capacity(self.cables.len());
        for c in &self.cables {
            ensure!(c.id > 0, "Zone '{}' cable ID must be nonzero", self.name);
            ensure!(
                cable_ids.insert(c.id),
                "Zone '{}' has duplicate cable ID {}",
                self.name,
                c.id
            );
            let from_mod = self.module(c.from_module).with_context(|| {
                format!(
                    "Zone '{}' cable #{} references missing source module #{}",
                    self.name, c.id, c.from_module
                )
            })?;
            ensure!(
                from_mod.kind.has_output(),
                "Zone '{}' cable #{} cannot originate from sink module '{}' (#{})",
                self.name,
                c.id,
                from_mod.name,
                from_mod.id
            );
            let to_mod = self.module(c.to_module).with_context(|| {
                format!(
                    "Zone '{}' cable #{} references missing target module #{}",
                    self.name, c.id, c.to_module
                )
            })?;
            ensure!(
                to_mod.kind.accepts_in_port(c.to_port),
                "Zone '{}' cable #{} cannot patch into {} port on {} module '{}'",
                self.name,
                c.id,
                c.to_port.label(),
                to_mod.kind.label(),
                to_mod.name
            );
            ensure!(
                seen_edges.insert((c.from_module, c.from_port, c.to_module, c.to_port)),
                "Zone '{}' has duplicate cable between module #{} and #{}.{}",
                self.name,
                c.from_module,
                c.to_module,
                c.to_port.slug()
            );
            let (lo, hi) = cable_amount_range(to_mod.kind.tag(), c.to_port);
            ensure!(
                c.amount.is_finite() && (lo..=hi).contains(&c.amount),
                "Zone '{}' cable #{} amount must be finite and within {lo}..={hi}",
                self.name,
                c.id
            );
        }
        Ok(())
    }
}

/// Unified modular Eurorack instrument (`InstrumentInstance.rack`).
///
/// Single-sound synths and samplers are 1-zone Racks; drum kits and multisamples
/// are N-zone Racks with key/velocity splits and choke groups.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RackPatch {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kit_name: String,
    pub zones: Vec<RackZone>,
    #[serde(default)]
    pub gain_db: f32,
    #[serde(default)]
    pub pan: f32,
    #[serde(default)]
    pub tune_semitones: f32,
    #[serde(default = "default_cutoff")]
    pub cutoff_hz: f32,
    #[serde(default)]
    pub resonance: f32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub panel_pins: Vec<PanelPin>,
}

impl Default for RackPatch {
    fn default() -> Self {
        Self::default_synth()
    }
}

impl RackPatch {
    /// Single-point-of-truth iterator yielding every `Sampler` module across every zone
    /// as `(zone_id, &RackZone, module_id, &SamplerModule)`.
    pub fn sampler_modules(
        &self,
    ) -> impl Iterator<Item = (u16, &RackZone, u16, &SamplerModule)> + '_ {
        self.zones.iter().flat_map(|zone| {
            zone.modules.iter().filter_map(move |m| match &m.kind {
                ModuleKind::Sampler(smp) => Some((zone.id, zone, m.id, smp)),
                _ => None,
            })
        })
    }

    /// Mutable single-point-of-truth iterator yielding every `Sampler` module across every zone
    /// as `(zone_id, module_id, &mut SamplerModule)`.
    pub fn sampler_modules_mut(
        &mut self,
    ) -> impl Iterator<Item = (u16, u16, &mut SamplerModule)> + '_ {
        self.zones.iter_mut().flat_map(|zone| {
            let zone_id = zone.id;
            zone.modules.iter_mut().filter_map(move |m| {
                let mod_id = m.id;
                match &mut m.kind {
                    ModuleKind::Sampler(smp) => Some((zone_id, mod_id, smp)),
                    _ => None,
                }
            })
        })
    }

    pub fn has_samplers(&self) -> bool {
        self.sampler_modules().next().is_some()
    }

    pub fn zone(&self, zone_id: u16) -> Option<&RackZone> {
        self.zones.iter().find(|z| z.id == zone_id)
    }

    pub fn zone_mut(&mut self, zone_id: u16) -> Option<&mut RackZone> {
        self.zones.iter_mut().find(|z| z.id == zone_id)
    }

    pub fn zone_index(&self, zone_id: u16) -> Option<usize> {
        self.zones.iter().position(|z| z.id == zone_id)
    }

    pub fn next_zone_id(&self) -> u16 {
        self.zones
            .iter()
            .map(|z| z.id)
            .max()
            .unwrap_or(0)
            .saturating_add(1)
    }

    pub fn clamp_zone_index(&self, index: usize) -> usize {
        if self.zones.is_empty() {
            0
        } else {
            index.min(self.zones.len() - 1)
        }
    }

    fn parse_z_prefixed_zone_id(&self, trimmed: &str) -> Option<u16> {
        let rest = trimmed
            .strip_prefix('z')
            .or_else(|| trimmed.strip_prefix('Z'))?;
        if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let id = rest.parse::<u16>().ok()?;
        if self.zone(id).is_some() {
            Some(id)
        } else {
            None
        }
    }

    fn find_zone_index_by_name(&self, trimmed: &str) -> Option<usize> {
        let norm_query = trimmed.to_ascii_lowercase().replace([' ', '-'], "_");
        if let Some(idx) = self.zones.iter().position(|z| {
            z.name.eq_ignore_ascii_case(trimmed)
                || z.name.to_ascii_lowercase().replace([' ', '-'], "_") == norm_query
        }) {
            return Some(idx);
        }
        let lower = trimmed.to_ascii_lowercase();
        self.zones
            .iter()
            .position(|z| z.name.to_ascii_lowercase().contains(&lower))
    }

    /// Resolve a zone token (`z1`, `1`, or case-insensitive zone name) into its stable `zone_id`.
    pub fn resolve_zone_id(&self, token: &str) -> Option<u16> {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            return None;
        }
        if let Some(id) = self.parse_z_prefixed_zone_id(trimmed) {
            return Some(id);
        }
        if trimmed.bytes().all(|b| b.is_ascii_digit()) {
            if let Ok(id) = trimmed.parse::<u16>() {
                if self.zone(id).is_some() {
                    return Some(id);
                }
            }
        }
        self.find_zone_index_by_name(trimmed)
            .map(|idx| self.zones[idx].id)
    }

    /// Find a zone's slice index by `z<id>`, 1-based positional index, or case-insensitive name.
    pub fn find_zone(&self, query: &str) -> Option<usize> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return None;
        }
        if let Some(id) = self.parse_z_prefixed_zone_id(trimmed) {
            return self.zone_index(id);
        }
        if trimmed.bytes().all(|b| b.is_ascii_digit()) {
            if let Ok(n) = trimmed.parse::<usize>() {
                if n >= 1 && n <= self.zones.len() {
                    return Some(n - 1);
                }
            }
        }
        self.find_zone_index_by_name(trimmed)
    }

    /// True if `target` addresses a valid existing parameter/cable on this Rack
    /// (ignoring whether it is automatable).
    pub fn contains_param(&self, target: InstrumentParam) -> bool {
        match target {
            InstrumentParam::Rack { .. } => true,
            InstrumentParam::Zone { zone, .. } => self.zone(zone).is_some(),
            InstrumentParam::Module {
                zone,
                module,
                param,
            } => self
                .zone(zone)
                .and_then(|z| z.module(module))
                .is_some_and(|m| m.kind.supports_param(param)),
            InstrumentParam::Cable { zone, cable } => {
                self.zone(zone).and_then(|z| z.cable(cable)).is_some()
            }
        }
    }

    /// True if `target` exists on this Rack AND is automatable (links/modulations/macros).
    pub fn supports_automatable_param(&self, target: InstrumentParam) -> bool {
        target.is_automatable() && self.contains_param(target)
    }

    /// Validate that `target` exists on this Rack and is automatable.
    pub fn validate_automatable_param(&self, target: InstrumentParam) -> Result<()> {
        match target {
            InstrumentParam::Rack { param } => {
                ensure!(
                    param.is_automatable(),
                    "Rack parameter {} cannot be linked or bound",
                    param.label()
                );
            }
            InstrumentParam::Zone { zone, param } => {
                let z = self
                    .zone(zone)
                    .with_context(|| format!("Zone z{zone} does not exist"))?;
                ensure!(
                    param.is_automatable(),
                    "Zone '{}' parameter {} cannot be linked or bound",
                    z.name,
                    param.label()
                );
            }
            InstrumentParam::Module {
                zone,
                module,
                param,
            } => {
                let z = self
                    .zone(zone)
                    .with_context(|| format!("Zone z{zone} does not exist"))?;
                let m = z.module(module).with_context(|| {
                    format!("Module m{module} does not exist in zone '{}'", z.name)
                })?;
                ensure!(
                    m.kind.supports_param(param),
                    "Module '{}' ({}) does not support parameter {}",
                    m.name,
                    m.kind.label(),
                    param.label()
                );
                ensure!(
                    param.is_automatable(),
                    "Module '{}' parameter {} cannot be linked or bound",
                    m.name,
                    param.label()
                );
            }
            InstrumentParam::Cable { zone, cable } => {
                let z = self
                    .zone(zone)
                    .with_context(|| format!("Zone z{zone} does not exist"))?;
                ensure!(
                    z.cable(cable).is_some(),
                    "Cable c{cable} does not exist in zone '{}'",
                    z.name
                );
            }
        }
        Ok(())
    }

    /// Remove any `panel_pins` whose targets no longer exist on this Rack.
    pub fn prune_invalid_pins(&mut self) {
        let keep: Vec<bool> = self
            .panel_pins
            .iter()
            .map(|pin| self.contains_param(pin.target))
            .collect();
        let mut idx = 0usize;
        self.panel_pins.retain(|_| {
            let ok = keep[idx];
            idx += 1;
            ok
        });
    }

    /// Read any Rack/Zone/Module/Cable parameter in its native physical unit.
    pub fn read_param_native(&self, target: InstrumentParam) -> Option<f32> {
        match target {
            InstrumentParam::Rack { param } => Some(param.read_native(self)),
            InstrumentParam::Zone { zone, param } => Some(param.read_native(self.zone(zone)?)),
            InstrumentParam::Module {
                zone,
                module,
                param,
            } => self.zone(zone)?.module(module)?.kind.read_native(param),
            InstrumentParam::Cable { zone, cable } => Some(self.zone(zone)?.cable(cable)?.amount),
        }
    }

    /// Write a validated native-unit value onto any Rack/Zone/Module/Cable parameter.
    pub fn write_param_native(&mut self, target: InstrumentParam, value: f32) -> Result<()> {
        match target {
            InstrumentParam::Rack { param } => param.write_native(self, value),
            InstrumentParam::Zone { zone, param } => {
                let z = self
                    .zone_mut(zone)
                    .with_context(|| format!("Zone z{zone} does not exist"))?;
                param.write_native(z, value)
            }
            InstrumentParam::Module {
                zone,
                module,
                param,
            } => {
                let z = self
                    .zone_mut(zone)
                    .with_context(|| format!("Zone z{zone} does not exist"))?;
                let m = z
                    .module_mut(module)
                    .with_context(|| format!("Module m{module} does not exist"))?;
                m.kind.write_native(param, value)?;
                if param == ModuleParam::RootMidi {
                    z.root_midi = value.round().clamp(0.0, 127.0) as u8;
                }
                Ok(())
            }
            InstrumentParam::Cable { zone, cable } => {
                let z = self
                    .zone_mut(zone)
                    .with_context(|| format!("Zone z{zone} does not exist"))?;
                let (to_module, to_port) = {
                    let c = z
                        .cable(cable)
                        .with_context(|| format!("Cable c{cable} does not exist"))?;
                    (c.to_module, c.to_port)
                };
                let target_kind = z
                    .module(to_module)
                    .context("Cable target module is missing")?
                    .kind
                    .tag();
                let (lo, hi) = cable_amount_range(target_kind, to_port);
                ensure!(
                    value.is_finite() && (lo..=hi).contains(&value),
                    "Cable c{cable} amount must be finite and within {lo}..={hi}"
                );
                if let Some(c) = z.cable_mut(cable) {
                    c.amount = value;
                }
                Ok(())
            }
        }
    }

    /// Read any Rack/Zone/Module/Cable parameter normalized onto `[0.0, 1.0]`.
    pub fn read_param_normalized(
        &self,
        target: InstrumentParam,
        total_frames: usize,
    ) -> Option<f32> {
        match target {
            InstrumentParam::Rack { param } => Some(param.read_normalized(self)),
            InstrumentParam::Zone { zone, param } => Some(param.read_normalized(self.zone(zone)?)),
            InstrumentParam::Module {
                zone,
                module,
                param,
            } => self
                .zone(zone)?
                .module(module)?
                .kind
                .read_normalized(param, total_frames),
            InstrumentParam::Cable { zone, cable } => {
                let z = self.zone(zone)?;
                let c = z.cable(cable)?;
                let to_kind = z.module(c.to_module)?.kind.tag();
                Some(crate::rack::modules::normalize_cable_amount(
                    to_kind, c.to_port, c.amount,
                ))
            }
        }
    }

    /// Write a normalized `[0.0, 1.0]` value onto any Rack/Zone/Module/Cable parameter.
    pub fn write_param_normalized(
        &mut self,
        target: InstrumentParam,
        norm: f32,
        total_frames: usize,
    ) -> bool {
        let n = if norm.is_finite() {
            norm.clamp(0.0, 1.0)
        } else {
            0.0
        };
        match target {
            InstrumentParam::Rack { param } => {
                param.write_normalized(self, n);
                true
            }
            InstrumentParam::Zone { zone, param } => {
                let Some(z) = self.zone_mut(zone) else {
                    return false;
                };
                param.write_normalized(z, n);
                true
            }
            InstrumentParam::Module {
                zone,
                module,
                param,
            } => {
                let Some(z) = self.zone_mut(zone) else {
                    return false;
                };
                let Some(m) = z.module_mut(module) else {
                    return false;
                };
                let ok = m.kind.write_normalized(param, n, total_frames);
                if ok && param == ModuleParam::RootMidi {
                    if let ModuleKind::Sampler(smp) = &m.kind {
                        z.root_midi = smp.root_midi;
                    }
                }
                ok
            }
            InstrumentParam::Cable { zone, cable } => {
                let Some(z) = self.zone_mut(zone) else {
                    return false;
                };
                let Some((to_module, to_port)) = z.cable(cable).map(|c| (c.to_module, c.to_port))
                else {
                    return false;
                };
                let Some(to_kind) = z.module(to_module).map(|m| m.kind.tag()) else {
                    return false;
                };
                let val = crate::rack::modules::denormalize_cable_amount(to_kind, to_port, n);
                if let Some(c) = z.cable_mut(cable) {
                    c.amount = val;
                    true
                } else {
                    false
                }
            }
        }
    }

    /// Format a parameter's current value in human-readable native units.
    pub fn format_param(&self, target: InstrumentParam, total_frames: usize) -> Option<String> {
        match target {
            InstrumentParam::Rack { param } => Some(param.format_native(param.read_native(self))),
            InstrumentParam::Zone { zone, param } => {
                let z = self.zone(zone)?;
                Some(param.format_native(param.read_native(z)))
            }
            InstrumentParam::Module {
                zone,
                module,
                param,
            } => self
                .zone(zone)?
                .module(module)?
                .kind
                .format_param(param, total_frames),
            InstrumentParam::Cable { zone, cable } => {
                let z = self.zone(zone)?;
                let c = z.cable(cable)?;
                let to_kind = z.module(c.to_module)?.kind.tag();
                Some(format_cable_amount(to_kind, c.to_port, c.amount))
            }
        }
    }

    /// Format a normalized `0.0..=1.0` value for `target` in the context of this Rack.
    pub fn format_param_normalized(
        &self,
        target: InstrumentParam,
        norm: f32,
        total_frames: usize,
    ) -> String {
        match target {
            InstrumentParam::Rack { param } => param.format_normalized(norm),
            InstrumentParam::Zone { param, .. } => param.format_normalized(norm),
            InstrumentParam::Module { param, .. } => param.format_normalized(norm, total_frames),
            InstrumentParam::Cable { zone, cable } => {
                if let Some(z) = self.zone(zone) {
                    if let Some(c) = z.cable(cable) {
                        if let Some(m) = z.module(c.to_module) {
                            let native = crate::rack::modules::denormalize_cable_amount(
                                m.kind.tag(),
                                c.to_port,
                                norm,
                            );
                            return format_cable_amount(m.kind.tag(), c.to_port, native);
                        }
                    }
                }
                let amt = -2.0 + norm.clamp(0.0, 1.0) * 4.0;
                format!("{:+.0}%", amt * 100.0)
            }
        }
    }

    /// Parse a user-supplied native-unit string for `target` on this Rack.
    pub fn parse_param_value_native(&self, target: InstrumentParam, input: &str) -> Result<f32> {
        match target {
            InstrumentParam::Rack { param } => param.parse_value_native(input),
            InstrumentParam::Zone { param, .. } => param.parse_value_native(input),
            InstrumentParam::Module { param, .. } => param.parse_value_native(input),
            InstrumentParam::Cable { zone, cable } => {
                let z = self
                    .zone(zone)
                    .with_context(|| format!("Zone z{zone} does not exist"))?;
                let c = z
                    .cable(cable)
                    .with_context(|| format!("Cable c{cable} does not exist"))?;
                let to_kind = z
                    .module(c.to_module)
                    .context("Cable target module is missing")?
                    .kind
                    .tag();
                parse_cable_amount(to_kind, c.to_port, input)
            }
        }
    }

    /// Nudge any Rack/Zone/Module/Cable parameter in place and return its new native value.
    pub fn nudge_param(
        &mut self,
        target: InstrumentParam,
        dir: i8,
        coarse: bool,
        total_frames: usize,
    ) -> Result<f32> {
        match target {
            InstrumentParam::Rack { param } => {
                let next = param.nudge_native(param.read_native(self), dir, coarse);
                param.write_native(self, next)?;
                Ok(next)
            }
            InstrumentParam::Zone { zone, param } => {
                let z = self
                    .zone_mut(zone)
                    .with_context(|| format!("Zone z{zone} does not exist"))?;
                let next = param.nudge_native(param.read_native(z), dir, coarse);
                param.write_native(z, next)?;
                Ok(next)
            }
            InstrumentParam::Module {
                zone,
                module,
                param,
            } => {
                let z = self
                    .zone_mut(zone)
                    .with_context(|| format!("Zone z{zone} does not exist"))?;
                let m = z
                    .module_mut(module)
                    .with_context(|| format!("Module m{module} does not exist"))?;
                let next = m.kind.nudge_param(param, dir, coarse, total_frames)?;
                if param == ModuleParam::RootMidi {
                    z.root_midi = next.round().clamp(0.0, 127.0) as u8;
                }
                Ok(next)
            }
            InstrumentParam::Cable { zone, cable } => {
                let z = self
                    .zone_mut(zone)
                    .with_context(|| format!("Zone z{zone} does not exist"))?;
                let (to_module, to_port, current) = {
                    let c = z
                        .cable(cable)
                        .with_context(|| format!("Cable c{cable} does not exist"))?;
                    (c.to_module, c.to_port, c.amount)
                };
                let to_kind = z
                    .module(to_module)
                    .context("Cable target module is missing")?
                    .kind
                    .tag();
                let next = nudge_cable_amount(to_kind, to_port, current, dir, coarse);
                if let Some(c) = z.cable_mut(cable) {
                    c.amount = next;
                }
                Ok(next)
            }
        }
    }

    pub fn descriptors(&self) -> Vec<ParamDescriptor> {
        RackParam::ALL
            .into_iter()
            .map(|p| ParamDescriptor {
                id: p.id(),
                module: p.module(),
                name: p.label(),
                display: p.format_native(p.read_native(self)),
                normalized: p.read_normalized(self),
            })
            .collect()
    }

    /// Build a multi-sampled `RackPatch` from decoded `(name, rel_path, meta, frames)` entries,
    /// preserving RIFF `smpl`/`inst` bounds and auto-splitting pitched roots across `0..=127`.
    pub fn from_multisample_entries(
        entries: &[(String, String, SampleMetadata, usize)],
    ) -> Result<Self> {
        ensure!(
            !entries.is_empty(),
            "Cannot build Rack from an empty sample list"
        );
        struct ResolvedEntry<'a> {
            name: &'a str,
            rel_path: &'a str,
            meta: SampleMetadata,
            frames: usize,
            root_midi: u8,
            has_pitch: bool,
            has_inst_key_bounds: bool,
            orig_idx: usize,
        }
        let mut resolved: Vec<ResolvedEntry<'_>> = entries
            .iter()
            .enumerate()
            .map(|(orig_idx, (name, rel_path, meta, frames))| {
                let (root_midi, has_pitch) = if meta.has_root {
                    (meta.root_midi.min(127), true)
                } else if let Some(parsed) =
                    parse_filename_midi(name).or_else(|| parse_filename_midi(rel_path))
                {
                    (parsed.min(127), true)
                } else {
                    (0, false)
                };
                let has_inst_key_bounds = matches!(
                    (meta.low_midi, meta.high_midi),
                    (Some(lo), Some(hi)) if lo <= hi && hi <= 127
                );
                ResolvedEntry {
                    name: name.as_str(),
                    rel_path: rel_path.as_str(),
                    meta: *meta,
                    frames: *frames,
                    root_midi,
                    has_pitch,
                    has_inst_key_bounds,
                    orig_idx,
                }
            })
            .collect();

        let any_pitched = resolved.iter().any(|e| e.has_pitch);
        if any_pitched {
            let mut next_fallback = 60u8;
            for entry in &mut resolved {
                if !entry.has_pitch {
                    entry.root_midi = next_fallback;
                    next_fallback = next_fallback.saturating_add(1).min(127);
                }
            }
            resolved.sort_by(|a, b| {
                a.root_midi
                    .cmp(&b.root_midi)
                    .then_with(|| {
                        a.meta
                            .low_vel
                            .unwrap_or(0)
                            .cmp(&b.meta.low_vel.unwrap_or(0))
                    })
                    .then_with(|| a.name.cmp(b.name))
                    .then_with(|| a.orig_idx.cmp(&b.orig_idx))
            });
        } else {
            for (i, entry) in resolved.iter_mut().enumerate() {
                entry.root_midi = (36usize + i).min(127) as u8;
            }
        }

        let selected: Vec<ResolvedEntry<'_>> = if resolved.len() > MAX_ZONES {
            if any_pitched {
                let total = resolved.len();
                let mut picked = Vec::with_capacity(MAX_ZONES);
                let mut indices = Vec::with_capacity(MAX_ZONES);
                for i in 0..MAX_ZONES {
                    indices.push(i * (total - 1) / (MAX_ZONES - 1));
                }
                for (pos, entry) in resolved.into_iter().enumerate() {
                    if indices.binary_search(&pos).is_ok() {
                        picked.push(entry);
                    }
                }
                picked
            } else {
                resolved.into_iter().take(MAX_ZONES).collect()
            }
        } else {
            resolved
        };

        let mut unique_roots: Vec<u8> = selected
            .iter()
            .filter(|e| any_pitched && !e.has_inst_key_bounds)
            .map(|e| e.root_midi)
            .collect();
        unique_roots.sort_unstable();
        unique_roots.dedup();

        let mut used_names: Vec<String> = Vec::with_capacity(selected.len());
        let mut zones = Vec::with_capacity(selected.len());
        for (i, entry) in selected.into_iter().enumerate() {
            let zone_id = (i + 1) as u16;
            let trimmed = entry.name.trim();
            let base: String = if trimmed.is_empty() {
                format!("Zone {zone_id}")
            } else {
                trimmed.chars().take(32).collect()
            };
            let mut zone_name = base.clone();
            let mut suffix = 2usize;
            while used_names
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(&zone_name))
            {
                let tag = format!(" {suffix}");
                let max_base = 32usize.saturating_sub(tag.len());
                let prefix: String = base.chars().take(max_base).collect();
                zone_name = format!("{}{tag}", prefix.trim_end());
                suffix += 1;
            }
            used_names.push(zone_name.clone());

            let mut zone = RackZone::from_sample_metadata(
                zone_id,
                zone_name,
                entry.rel_path,
                &entry.meta,
                entry.frames,
                entry.root_midi,
            );
            if any_pitched && !entry.has_inst_key_bounds && !unique_roots.is_empty() {
                if let Ok(r_idx) = unique_roots.binary_search(&entry.root_midi) {
                    let lo = if r_idx == 0 {
                        0
                    } else {
                        ((u16::from(unique_roots[r_idx - 1]) + u16::from(unique_roots[r_idx])) / 2
                            + 1)
                        .min(127) as u8
                    };
                    let hi = if r_idx + 1 == unique_roots.len() {
                        127
                    } else {
                        ((u16::from(unique_roots[r_idx]) + u16::from(unique_roots[r_idx + 1])) / 2)
                            .min(127) as u8
                    };
                    zone.set_midi_bounds(lo, hi, Some(entry.root_midi));
                }
            }
            zones.push(zone);
        }

        let patch = Self {
            kit_name: "Multisample".into(),
            zones,
            gain_db: 0.0,
            pan: 0.0,
            tune_semitones: 0.0,
            cutoff_hz: default_cutoff(),
            resonance: 0.0,
            panel_pins: Vec::new(),
        };
        patch.validate()?;
        Ok(patch)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.kit_name.chars().count() <= 64 && !self.kit_name.chars().any(char::is_control),
            "Rack kit_name must be <= 64 printable characters"
        );
        ensure!(
            !self.zones.is_empty() && self.zones.len() <= MAX_ZONES,
            "Rack must have 1..={MAX_ZONES} zones"
        );
        for param in RackParam::ALL {
            let v = param.read_native(self);
            let (lo, hi) = param.range();
            ensure!(
                v.is_finite() && (lo..=hi).contains(&v),
                "Rack {} must be finite and within {lo}..={hi}",
                param.label()
            );
        }
        let mut zone_ids = HashSet::with_capacity(self.zones.len());
        for zone in &self.zones {
            zone.validate()?;
            ensure!(
                zone_ids.insert(zone.id),
                "Rack has duplicate zone ID {}",
                zone.id
            );
        }
        ensure!(
            self.panel_pins.len() <= MAX_PANEL_PINS,
            "Rack exceeds maximum of {MAX_PANEL_PINS} panel pins"
        );
        let mut seen_pins = HashSet::with_capacity(self.panel_pins.len());
        for pin in &self.panel_pins {
            ensure!(
                pin.label.chars().count() <= 32 && !pin.label.chars().any(char::is_control),
                "Panel pin label must be <= 32 printable characters"
            );
            ensure!(
                self.contains_param(pin.target),
                "Panel pin '{}' references a missing zone, module, cable, or unsupported parameter",
                pin.display_label()
            );
            if !seen_pins.insert(pin.target) {
                bail!("Duplicate panel pin for the same target");
            }
        }
        Ok(())
    }
}
