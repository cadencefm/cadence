//! Zero-disk-I/O bare neutral engine templates (`default_synth`, `default_sampler`,
//! `default_kit`, `blank_modular`) and canonical modular Rack builders (`acid_303`,
//! and self-contained demo voices).

use crate::dsp::env::EnvMode;
use crate::params::ZoneParam;
use crate::rack::modules::{
    EnvModule, FilterModule, LfoModule, ModuleKind, NoiseModule, OutModule, SamplerModule,
    VcaModule, VcoModule,
};
use crate::rack::params::ModuleParam;
use crate::rack::types::{
    FilterMode, InPort, LfoShape, NoiseColor, PanelPin, PatchCable, PitchTracking, RackModule,
    RackPatch, RackTemplate, RackZone, TriggerMode, VoiceMode, VoiceSettings, Waveform,
};

impl RackPatch {
    /// Construct the bare neutral engine template for `template` (zero disk I/O).
    pub fn from_template(template: RackTemplate) -> Self {
        match template {
            RackTemplate::Synth => Self::default_synth(),
            RackTemplate::Sampler => Self::default_sampler(),
            RackTemplate::Kit => Self::default_kit(),
            RackTemplate::Modular => Self::blank_modular(),
        }
    }

    /// Bare neutral subtractive synth Rack:
    /// `VCO 1 (Saw) -> Filter (LP12) -> VCA -> Out`, `Amp Env -> VCA.Cv`, `Filter Env -> Filter.Cv`,
    /// with curated top-level `panel_pins`.
    pub fn default_synth() -> Self {
        let zone = RackZone {
            id: 1,
            name: "Synth".into(),
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
                RackModule::new(1, "VCO 1", ModuleKind::Vco(VcoModule::default())),
                RackModule::new(
                    2,
                    "Filter",
                    ModuleKind::Filter(FilterModule {
                        mode: FilterMode::LowPass,
                        cutoff_hz: 7_000.0,
                        resonance: 0.12,
                        drive: 0.05,
                        key_tracking: 0.0,
                        vel_amount: 0.0,
                        mix: 1.0,
                    }),
                ),
                RackModule::new(3, "VCA", ModuleKind::Vca(VcaModule::default())),
                RackModule::new(
                    4,
                    "Amp Env",
                    ModuleKind::Env(EnvModule {
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
                    }),
                ),
                RackModule::new(
                    5,
                    "Filter Env",
                    ModuleKind::Env(EnvModule {
                        mode: EnvMode::Adsr,
                        attack_ms: 2.0,
                        hold_ms: 0.0,
                        decay_ms: 300.0,
                        sustain: 0.0,
                        release_ms: 150.0,
                        trigger_mode: TriggerMode::Gate,
                        delay_ms: 0.0,
                        bursts: 1,
                        burst_spacing_ms: 10.0,
                    }),
                ),
                RackModule::new(6, "Out", ModuleKind::Out(OutModule::default())),
            ],
            cables: vec![
                PatchCable::new(1, 1, 2, InPort::In, 1.0),
                PatchCable::new(2, 2, 3, InPort::In, 1.0),
                PatchCable::new(3, 3, 6, InPort::In, 1.0),
                PatchCable::new(4, 4, 3, InPort::Cv, 1.0),
                PatchCable::new(5, 5, 2, InPort::Cv, 0.0),
            ],
        };
        Self {
            kit_name: String::new(),
            zones: vec![zone],
            gain_db: 0.0,
            pan: 0.0,
            tune_semitones: 0.0,
            cutoff_hz: 20_000.0,
            resonance: 0.0,
            panel_pins: vec![
                PanelPin::module(1, 2, ModuleParam::Cutoff, "Cutoff"),
                PanelPin::module(1, 2, ModuleParam::Resonance, "Res"),
                PanelPin::cable(1, 5, "Env Mod"),
                PanelPin::module(1, 2, ModuleParam::Drive, "Drive"),
                PanelPin::module(1, 4, ModuleParam::Attack, "Attack"),
                PanelPin::module(1, 4, ModuleParam::Decay, "Decay"),
                PanelPin::module(1, 4, ModuleParam::Sustain, "Sustain"),
                PanelPin::module(1, 4, ModuleParam::Release, "Release"),
                PanelPin::module(1, 6, ModuleParam::Pan, "Pan"),
                PanelPin::module(1, 1, ModuleParam::Waveform, "Wave"),
            ],
        }
    }

    /// Bare neutral single-zone Sampler Rack:
    /// `Sampler -> Filter (LP12 @ 18.5 kHz) -> VCA -> Out`, `Amp Env -> VCA.Cv`.
    pub fn default_sampler() -> Self {
        let zone = RackZone {
            id: 1,
            name: "Sampler".into(),
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
                RackModule::new(
                    2,
                    "Filter",
                    ModuleKind::Filter(FilterModule {
                        mode: FilterMode::LowPass,
                        cutoff_hz: 18_500.0,
                        resonance: 0.08,
                        drive: 0.0,
                        key_tracking: 0.0,
                        vel_amount: 0.0,
                        mix: 1.0,
                    }),
                ),
                RackModule::new(3, "VCA", ModuleKind::Vca(VcaModule::default())),
                RackModule::new(
                    4,
                    "Amp Env",
                    ModuleKind::Env(EnvModule {
                        mode: EnvMode::Adsr,
                        attack_ms: 1.5,
                        hold_ms: 0.0,
                        decay_ms: 350.0,
                        sustain: 0.85,
                        release_ms: 140.0,
                        trigger_mode: TriggerMode::Gate,
                        delay_ms: 0.0,
                        bursts: 1,
                        burst_spacing_ms: 10.0,
                    }),
                ),
                RackModule::new(5, "Out", ModuleKind::Out(OutModule::default())),
            ],
            cables: vec![
                PatchCable::new(1, 1, 2, InPort::In, 1.0),
                PatchCable::new(2, 2, 3, InPort::In, 1.0),
                PatchCable::new(3, 3, 5, InPort::In, 1.0),
                PatchCable::new(4, 4, 3, InPort::Cv, 1.0),
            ],
        };
        Self {
            kit_name: String::new(),
            zones: vec![zone],
            gain_db: 0.0,
            pan: 0.0,
            tune_semitones: 0.0,
            cutoff_hz: 20_000.0,
            resonance: 0.0,
            panel_pins: vec![
                PanelPin::module(1, 1, ModuleParam::RootMidi, "Root"),
                PanelPin::module(1, 1, ModuleParam::CoarseTune, "Coarse"),
                PanelPin::module(1, 2, ModuleParam::Cutoff, "Cutoff"),
                PanelPin::module(1, 2, ModuleParam::Resonance, "Res"),
                PanelPin::module(1, 4, ModuleParam::Attack, "Attack"),
                PanelPin::module(1, 4, ModuleParam::Decay, "Decay"),
                PanelPin::module(1, 4, ModuleParam::Sustain, "Sustain"),
                PanelPin::module(1, 4, ModuleParam::Release, "Release"),
                PanelPin::module(1, 5, ModuleParam::Pan, "Pan"),
            ],
        }
    }

    /// Bare neutral Kit / Multisample Rack with 1 full-range `[Sampler -> Out]` zone
    /// ready for Sample Browser (`B`), `:zone add`, or `:zone import`.
    pub fn default_kit() -> Self {
        Self {
            kit_name: "Kit".into(),
            zones: vec![RackZone::blank_sample(1, "Zone 1")],
            gain_db: 0.0,
            pan: 0.0,
            tune_semitones: 0.0,
            cutoff_hz: 20_000.0,
            resonance: 0.0,
            panel_pins: Vec::new(),
        }
    }

    /// Minimal empty Eurorack canvas (`[Out]`) ready for modular patching from scratch.
    pub fn blank_modular() -> Self {
        let zone = RackZone {
            id: 1,
            name: "Main".into(),
            low_midi: 0,
            high_midi: 127,
            root_midi: 60,
            low_vel: 0.0,
            high_vel: 1.0,
            gain_db: 0.0,
            pan: 0.0,
            choke_group: None,
            voice: VoiceSettings::default(),
            modules: vec![RackModule::new(
                1,
                "Out",
                ModuleKind::Out(OutModule::default()),
            )],
            cables: Vec::new(),
        };
        Self {
            kit_name: String::new(),
            zones: vec![zone],
            gain_db: 0.0,
            pan: 0.0,
            tune_semitones: 0.0,
            cutoff_hz: 20_000.0,
            resonance: 0.0,
            panel_pins: Vec::new(),
        }
    }

    /// Canonical 6-module Acid TB-303 Rack (`VCO -> AcidFilter -> VCA -> Out`,
    /// `Amp Env -> VCA.Cv`, `Filter Env -> AcidFilter.Cv`) with curated 303 `panel_pins`.
    pub fn acid_303() -> Self {
        let zone = RackZone {
            id: 1,
            name: "Acid 303".into(),
            low_midi: 0,
            high_midi: 127,
            root_midi: 60,
            low_vel: 0.0,
            high_vel: 1.0,
            gain_db: 0.0,
            pan: 0.0,
            choke_group: None,
            voice: VoiceSettings {
                voice_mode: VoiceMode::Mono,
                legato: true,
                glide_ms: 55.0,
                pitch_tracking: PitchTracking::Chromatic,
                phase_reset: true,
            },
            modules: vec![
                RackModule::new(
                    1,
                    "VCO",
                    ModuleKind::Vco(VcoModule {
                        waveform: Waveform::Saw,
                        level: 0.85,
                        ..VcoModule::default()
                    }),
                ),
                RackModule::new(
                    2,
                    "Acid Filter",
                    ModuleKind::Filter(FilterModule {
                        mode: FilterMode::AcidLowPass,
                        cutoff_hz: 320.0,
                        resonance: 0.68,
                        drive: 0.25,
                        key_tracking: 0.15,
                        vel_amount: 2.2,
                        mix: 1.0,
                    }),
                ),
                RackModule::new(3, "VCA", ModuleKind::Vca(VcaModule::default())),
                RackModule::new(
                    4,
                    "Amp Env",
                    ModuleKind::Env(EnvModule {
                        mode: EnvMode::Adsr,
                        attack_ms: 1.5,
                        hold_ms: 0.0,
                        decay_ms: 360.0,
                        sustain: 0.45,
                        release_ms: 55.0,
                        trigger_mode: TriggerMode::Gate,
                        delay_ms: 0.0,
                        bursts: 1,
                        burst_spacing_ms: 10.0,
                    }),
                ),
                RackModule::new(
                    5,
                    "Filter Env",
                    ModuleKind::Env(EnvModule {
                        mode: EnvMode::Adsr,
                        attack_ms: 1.0,
                        hold_ms: 0.0,
                        decay_ms: 260.0,
                        sustain: 0.0,
                        release_ms: 60.0,
                        trigger_mode: TriggerMode::Gate,
                        delay_ms: 0.0,
                        bursts: 1,
                        burst_spacing_ms: 10.0,
                    }),
                ),
                RackModule::new(6, "Out", ModuleKind::Out(OutModule::default())),
            ],
            cables: vec![
                PatchCable::new(1, 1, 2, InPort::In, 1.0),
                PatchCable::new(2, 2, 3, InPort::In, 1.0),
                PatchCable::new(3, 3, 6, InPort::In, 1.0),
                PatchCable::new(4, 4, 3, InPort::Cv, 1.0),
                PatchCable::new(5, 5, 2, InPort::Cv, 3.2),
            ],
        };
        Self {
            kit_name: String::new(),
            zones: vec![zone],
            gain_db: 0.0,
            pan: 0.0,
            tune_semitones: 0.0,
            cutoff_hz: 20_000.0,
            resonance: 0.0,
            panel_pins: vec![
                PanelPin::module(1, 1, ModuleParam::Waveform, "Wave"),
                PanelPin::module(1, 1, ModuleParam::Tune, "Tune"),
                PanelPin::module(1, 2, ModuleParam::Cutoff, "Cutoff"),
                PanelPin::module(1, 2, ModuleParam::Resonance, "Res"),
                PanelPin::cable(1, 5, "Env Mod"),
                PanelPin::module(1, 5, ModuleParam::Decay, "Decay"),
                PanelPin::module(1, 2, ModuleParam::VelAmount, "Accent"),
                PanelPin::zone(1, ZoneParam::Glide, "Glide"),
            ],
        }
    }

    /// Self-contained dual-VCO mono lead Rack used by `Project::demo()`.
    pub(crate) fn demo_lead() -> Self {
        let zone = RackZone {
            id: 1,
            name: "Lead".into(),
            low_midi: 0,
            high_midi: 127,
            root_midi: 60,
            low_vel: 0.0,
            high_vel: 1.0,
            gain_db: 0.0,
            pan: 0.0,
            choke_group: None,
            voice: VoiceSettings {
                voice_mode: VoiceMode::Mono,
                legato: true,
                glide_ms: 45.0,
                pitch_tracking: PitchTracking::Chromatic,
                phase_reset: true,
            },
            modules: vec![
                RackModule::new(
                    1,
                    "VCO A",
                    ModuleKind::Vco(VcoModule {
                        waveform: Waveform::Pulse,
                        pulse_width: 0.42,
                        level: 0.65,
                        ..VcoModule::default()
                    }),
                ),
                RackModule::new(
                    2,
                    "VCO B",
                    ModuleKind::Vco(VcoModule {
                        waveform: Waveform::Saw,
                        fine_cents: 5.0,
                        level: 0.30,
                        ..VcoModule::default()
                    }),
                ),
                RackModule::new(
                    3,
                    "Sub",
                    ModuleKind::Vco(VcoModule {
                        waveform: Waveform::Sine,
                        tune_semitones: -12.0,
                        level: 0.05,
                        ..VcoModule::default()
                    }),
                ),
                RackModule::new(
                    4,
                    "Filter",
                    ModuleKind::Filter(FilterModule {
                        mode: FilterMode::LowPass,
                        cutoff_hz: 5_000.0,
                        resonance: 0.18,
                        drive: 0.05,
                        key_tracking: 0.0,
                        vel_amount: 0.0,
                        mix: 1.0,
                    }),
                ),
                RackModule::new(5, "VCA", ModuleKind::Vca(VcaModule::default())),
                RackModule::new(
                    6,
                    "Amp Env",
                    ModuleKind::Env(EnvModule {
                        mode: EnvMode::Adsr,
                        attack_ms: 3.0,
                        hold_ms: 0.0,
                        decay_ms: 220.0,
                        sustain: 0.65,
                        release_ms: 240.0,
                        trigger_mode: TriggerMode::Gate,
                        delay_ms: 0.0,
                        bursts: 1,
                        burst_spacing_ms: 10.0,
                    }),
                ),
                RackModule::new(
                    7,
                    "Filter Env",
                    ModuleKind::Env(EnvModule {
                        mode: EnvMode::Adsr,
                        attack_ms: 2.0,
                        hold_ms: 0.0,
                        decay_ms: 250.0,
                        sustain: 0.0,
                        release_ms: 150.0,
                        trigger_mode: TriggerMode::Gate,
                        delay_ms: 0.0,
                        bursts: 1,
                        burst_spacing_ms: 10.0,
                    }),
                ),
                RackModule::new(
                    8,
                    "LFO",
                    ModuleKind::Lfo(LfoModule {
                        shape: LfoShape::Sine,
                        rate_hz: 5.0,
                        sync: false,
                        division_steps: 16.0,
                        retrigger: true,
                        bipolar: true,
                    }),
                ),
                RackModule::new(
                    9,
                    "Out",
                    ModuleKind::Out(OutModule {
                        gain_db: 0.0,
                        pan: 0.0,
                        spread: 0.2,
                    }),
                ),
            ],
            cables: vec![
                PatchCable::new(1, 1, 4, InPort::In, 1.0),
                PatchCable::new(2, 2, 4, InPort::In, 1.0),
                PatchCable::new(3, 3, 4, InPort::In, 1.0),
                PatchCable::new(4, 4, 5, InPort::In, 1.0),
                PatchCable::new(5, 5, 9, InPort::In, 1.0),
                PatchCable::new(6, 6, 5, InPort::Cv, 1.0),
                PatchCable::new(7, 7, 4, InPort::Cv, 1.0),
                PatchCable::new(8, 8, 1, InPort::PitchCv, 0.07),
                PatchCable::new(9, 8, 1, InPort::PwCv, 0.08),
            ],
        };
        Self {
            kit_name: String::new(),
            zones: vec![zone],
            gain_db: 0.0,
            pan: 0.0,
            tune_semitones: 0.0,
            cutoff_hz: 20_000.0,
            resonance: 0.0,
            panel_pins: vec![
                PanelPin::module(1, 1, ModuleParam::Waveform, "Wave"),
                PanelPin::module(1, 4, ModuleParam::Cutoff, "Cutoff"),
                PanelPin::module(1, 4, ModuleParam::Resonance, "Res"),
                PanelPin::cable(1, 7, "Env Mod"),
                PanelPin::module(1, 6, ModuleParam::Decay, "Decay"),
                PanelPin::module(1, 6, ModuleParam::Release, "Release"),
                PanelPin::module(1, 9, ModuleParam::Pan, "Pan"),
            ],
        }
    }

    /// Self-contained warm mono bass Rack used by `Project::demo()`.
    pub(crate) fn demo_bass() -> Self {
        let zone = RackZone {
            id: 1,
            name: "Bass".into(),
            low_midi: 0,
            high_midi: 127,
            root_midi: 60,
            low_vel: 0.0,
            high_vel: 1.0,
            gain_db: 0.0,
            pan: 0.0,
            choke_group: None,
            voice: VoiceSettings {
                voice_mode: VoiceMode::Mono,
                legato: true,
                glide_ms: 35.0,
                pitch_tracking: PitchTracking::Chromatic,
                phase_reset: true,
            },
            modules: vec![
                RackModule::new(
                    1,
                    "VCO",
                    ModuleKind::Vco(VcoModule {
                        waveform: Waveform::Sine,
                        level: 0.90,
                        ..VcoModule::default()
                    }),
                ),
                RackModule::new(
                    2,
                    "Sub",
                    ModuleKind::Vco(VcoModule {
                        waveform: Waveform::Sine,
                        tune_semitones: -12.0,
                        level: 0.18,
                        ..VcoModule::default()
                    }),
                ),
                RackModule::new(
                    3,
                    "Filter",
                    ModuleKind::Filter(FilterModule {
                        mode: FilterMode::LowPass,
                        cutoff_hz: 950.0,
                        resonance: 0.08,
                        drive: 0.16,
                        key_tracking: 0.0,
                        vel_amount: 0.0,
                        mix: 1.0,
                    }),
                ),
                RackModule::new(4, "VCA", ModuleKind::Vca(VcaModule::default())),
                RackModule::new(
                    5,
                    "Amp Env",
                    ModuleKind::Env(EnvModule {
                        mode: EnvMode::Adsr,
                        attack_ms: 3.0,
                        hold_ms: 0.0,
                        decay_ms: 220.0,
                        sustain: 0.9,
                        release_ms: 120.0,
                        trigger_mode: TriggerMode::Gate,
                        delay_ms: 0.0,
                        bursts: 1,
                        burst_spacing_ms: 10.0,
                    }),
                ),
                RackModule::new(6, "Out", ModuleKind::Out(OutModule::default())),
            ],
            cables: vec![
                PatchCable::new(1, 1, 3, InPort::In, 1.0),
                PatchCable::new(2, 2, 3, InPort::In, 1.0),
                PatchCable::new(3, 3, 4, InPort::In, 1.0),
                PatchCable::new(4, 4, 6, InPort::In, 1.0),
                PatchCable::new(5, 5, 4, InPort::Cv, 1.0),
            ],
        };
        Self {
            kit_name: String::new(),
            zones: vec![zone],
            gain_db: 0.0,
            pan: 0.0,
            tune_semitones: 0.0,
            cutoff_hz: 20_000.0,
            resonance: 0.0,
            panel_pins: vec![
                PanelPin::module(1, 3, ModuleParam::Cutoff, "Cutoff"),
                PanelPin::module(1, 3, ModuleParam::Drive, "Drive"),
                PanelPin::module(1, 5, ModuleParam::Decay, "Decay"),
                PanelPin::module(1, 5, ModuleParam::Release, "Release"),
            ],
        }
    }

    /// Self-contained modular analog kick Rack (Sine VCO + HoldDecay pitch sweep + parallel
    /// post-filter transient click Noise branch) used by `Project::demo()`.
    pub(crate) fn demo_kick() -> Self {
        let zone = RackZone {
            id: 1,
            name: "Kick".into(),
            low_midi: 0,
            high_midi: 127,
            root_midi: 33,
            low_vel: 0.0,
            high_vel: 1.0,
            gain_db: 0.0,
            pan: 0.0,
            choke_group: None,
            voice: VoiceSettings {
                voice_mode: VoiceMode::Poly,
                legato: false,
                glide_ms: 0.0,
                pitch_tracking: PitchTracking::Fixed,
                phase_reset: true,
            },
            modules: vec![
                RackModule::new(
                    1,
                    "Body VCO",
                    ModuleKind::Vco(VcoModule {
                        waveform: Waveform::Sine,
                        level: 0.98,
                        ..VcoModule::default()
                    }),
                ),
                RackModule::new(
                    2,
                    "Pitch Env",
                    ModuleKind::Env(EnvModule {
                        mode: EnvMode::HoldDecay,
                        attack_ms: 0.1,
                        hold_ms: 0.0,
                        decay_ms: 52.0,
                        sustain: 0.0,
                        release_ms: 20.0,
                        trigger_mode: TriggerMode::OneShot,
                        delay_ms: 0.0,
                        bursts: 1,
                        burst_spacing_ms: 10.0,
                    }),
                ),
                RackModule::new(
                    3,
                    "Filter",
                    ModuleKind::Filter(FilterModule {
                        mode: FilterMode::LowPass,
                        cutoff_hz: 11_000.0,
                        resonance: 0.05,
                        drive: 0.12,
                        key_tracking: 0.0,
                        vel_amount: 0.0,
                        mix: 1.0,
                    }),
                ),
                RackModule::new(4, "Body VCA", ModuleKind::Vca(VcaModule::default())),
                RackModule::new(
                    5,
                    "Amp Env",
                    ModuleKind::Env(EnvModule {
                        mode: EnvMode::Adsr,
                        attack_ms: 0.4,
                        hold_ms: 0.0,
                        decay_ms: 340.0,
                        sustain: 0.0,
                        release_ms: 45.0,
                        trigger_mode: TriggerMode::OneShot,
                        delay_ms: 0.0,
                        bursts: 1,
                        burst_spacing_ms: 10.0,
                    }),
                ),
                RackModule::new(
                    6,
                    "Click Noise",
                    ModuleKind::Noise(NoiseModule {
                        color: NoiseColor::White,
                        highpass_hz: 2_800.0,
                        lowpass_hz: 16_000.0,
                        level: 0.12,
                    }),
                ),
                RackModule::new(7, "Click VCA", ModuleKind::Vca(VcaModule::default())),
                RackModule::new(
                    8,
                    "Click Env",
                    ModuleKind::Env(EnvModule {
                        mode: EnvMode::Adsr,
                        attack_ms: 0.1,
                        hold_ms: 0.0,
                        decay_ms: 7.0,
                        sustain: 0.0,
                        release_ms: 5.0,
                        trigger_mode: TriggerMode::OneShot,
                        delay_ms: 0.0,
                        bursts: 1,
                        burst_spacing_ms: 10.0,
                    }),
                ),
                RackModule::new(9, "Out", ModuleKind::Out(OutModule::default())),
            ],
            cables: vec![
                PatchCable::new(1, 2, 1, InPort::PitchCv, 31.0),
                PatchCable::new(2, 1, 3, InPort::In, 1.0),
                PatchCable::new(3, 3, 4, InPort::In, 1.0),
                PatchCable::new(4, 5, 4, InPort::Cv, 1.0),
                PatchCable::new(5, 4, 9, InPort::In, 1.0),
                PatchCable::new(6, 6, 7, InPort::In, 1.0),
                PatchCable::new(7, 8, 7, InPort::Cv, 1.0),
                PatchCable::new(8, 7, 9, InPort::In, 1.0),
            ],
        };
        Self {
            kit_name: String::new(),
            zones: vec![zone],
            gain_db: 0.0,
            pan: 0.0,
            tune_semitones: 0.0,
            cutoff_hz: 20_000.0,
            resonance: 0.0,
            panel_pins: vec![
                PanelPin::cable(1, 1, "Sweep"),
                PanelPin::module(1, 2, ModuleParam::Decay, "Pitch Dec"),
                PanelPin::module(1, 5, ModuleParam::Decay, "Body Dec"),
                PanelPin::module(1, 6, ModuleParam::Level, "Click"),
                PanelPin::module(1, 3, ModuleParam::Drive, "Drive"),
            ],
        }
    }
}
