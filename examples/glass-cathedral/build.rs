//! Reproducible builder for Cadence's generative ambient / IDM showcase: **"Glass Cathedral"**.
//!
//! 32 bars · 108 BPM · D Dorian (~1:11).
//! Demonstrates polymetric 5/7/11/16 phasing, `SeededValues`, `RangeFold`, `Reverse`,
//! `Every`, `TimeScale` triplets (`3/2`), `Arp(Random)`, `Chance`, inline `Mini` chimes,
//! 4 playable Project Macros (`M1..M4`), automated `Send` swells, and lush
//! `Chorus` + `Delay` + `Reverb` + `Eq` + studio `Drive(AtomicJet)` / `Compressor` / `Limiter` mastering.
//!
//! Run from the repository root:
//! ```bash
//! cargo run --example build-glass-cathedral --release
//! ```

use anyhow::Result;
use cadence_core::assets::load_project_samples;
use cadence_core::dsp::lin_to_db_f64;
use cadence_core::material::{
    write_document_json, ArpMode, Id, InstrumentInstance, Lane, LanePart, LiveFlags, LiveTiming,
    MacroBinding, MacroControl, MixerGraph, Modulation, ModulationTarget, Part, Pattern,
    PatternKind, PhaseMode, Placement, Project, Section, ValuePoint, ValueUnit, VERSION,
};
use cadence_core::mixer::{
    channel_gain_to_norm, Channel, ChannelKind, ChannelParam, ChannelRef, ChannelSend,
    ChannelStrip, ChorusPatch, CompressorPatch, DelayPatch, DriveMode, DrivePatch, EffectDevice,
    EffectKind, EffectParam, EqPatch, LimiterPatch, MasterChannel, ReverbPatch,
};
use cadence_core::music::{ChordTone, EndRule, Step};
use cadence_core::params::{ParamAddress, ValueMode};
use cadence_core::rack::{
    EnvMode, EnvModule, FilterMode, FilterModule, InPort, LfoModule, LfoShape, ModuleKind,
    ModuleParam, NoiseColor, NoiseModule, OutModule, PanelPin, PatchCable, PitchTracking,
    RackModule, RackPatch, RackZone, TriggerMode, VcaModule, VcoModule, VoiceMode, VoiceSettings,
    Waveform, ZoneParam,
};
use cadence_core::sampler::SampleLibrary;
use cadence_engine::audio::render_schedule_pcm;
use std::path::PathBuf;

// Stable module IDs referenced by Project Macros.
const PAD_FILTER_MOD: u16 = 4;
const MARIMBA_FILTER_MOD: u16 = 3;
const CHIME_FILTER_MOD: u16 = 3;
const CELLO_SUB_MOD: u16 = 3;
const CELLO_FILTER_MOD: u16 = 4;

fn fx(id: Id, name: &str, kind: EffectKind) -> EffectDevice {
    EffectDevice {
        id,
        name: name.into(),
        bypassed: false,
        kind,
    }
}

struct IdGen(std::cell::Cell<Id>);
impl IdGen {
    fn new() -> Self {
        Self(std::cell::Cell::new(0))
    }
    fn next(&self) -> Id {
        let next = self.0.get() + 1;
        self.0.set(next);
        next
    }
}

fn step_note(degree: i16, velocity: f32, gate: f32) -> Step {
    Step {
        active: true,
        degree,
        accidental: 0,
        velocity,
        gate,
        ..Step::default()
    }
}

fn chord_step(degrees: &[i16], velocity: f32, gate: f32) -> Step {
    let mut s = step_note(degrees[0], velocity, gate);
    s.chord = degrees[1..]
        .iter()
        .map(|&d| ChordTone {
            degree: d,
            accidental: 0,
            velocity,
            gate,
        })
        .collect();
    s
}

fn single_zone_rack(zone: RackZone, panel_pins: Vec<PanelPin>) -> RackPatch {
    RackPatch {
        kit_name: String::new(),
        zones: vec![zone],
        gain_db: 0.0,
        pan: 0.0,
        tune_semitones: 0.0,
        cutoff_hz: 20_000.0,
        resonance: 0.0,
        panel_pins,
    }
}

fn cathedral_pad_rack() -> RackPatch {
    let zone = RackZone {
        id: 1,
        name: "Cathedral Pad".into(),
        low_midi: 0,
        high_midi: 127,
        root_midi: 60,
        low_vel: 0.0,
        high_vel: 1.0,
        gain_db: -4.0,
        pan: 0.0,
        choke_group: None,
        voice: VoiceSettings {
            phase_reset: false,
            ..VoiceSettings::default()
        },
        modules: vec![
            RackModule::new(
                1,
                "VCO A",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::MobiusJet,
                    fine_cents: -8.0,
                    level: 0.42,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                2,
                "VCO B",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::BareCubic,
                    reach: 0.35,
                    shape: 0.0,
                    fine_cents: 8.0,
                    level: 0.46,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                3,
                "Noise",
                ModuleKind::Noise(NoiseModule {
                    color: NoiseColor::White,
                    highpass_hz: 3400.0,
                    lowpass_hz: 20_000.0,
                    level: 0.015,
                }),
            ),
            RackModule::new(
                PAD_FILTER_MOD,
                "Filter",
                ModuleKind::Filter(FilterModule {
                    mode: FilterMode::ImLpf2,
                    cutoff_hz: 1250.0,
                    resonance: 0.20,
                    drive: 0.05,
                    kerr_semitones: 0.0,
                    spread: 0.0,
                    key_tracking: 0.35,
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
                    attack_ms: 400.0,
                    decay_ms: 1200.0,
                    sustain: 0.78,
                    release_ms: 2100.0,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(
                7,
                "Filter Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::Adsr,
                    attack_ms: 750.0,
                    decay_ms: 1800.0,
                    sustain: 0.45,
                    release_ms: 150.0,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(
                8,
                "LFO",
                ModuleKind::Lfo(LfoModule {
                    shape: LfoShape::Sine,
                    rate_hz: 0.14,
                    retrigger: false,
                    ..LfoModule::default()
                }),
            ),
            RackModule::new(
                9,
                "Out",
                ModuleKind::Out(OutModule {
                    gain_db: 0.0,
                    pan: 0.0,
                    spread: 0.85,
                }),
            ),
        ],
        cables: vec![
            PatchCable::new(1, 1, PAD_FILTER_MOD, InPort::In, 1.0),
            PatchCable::new(2, 2, PAD_FILTER_MOD, InPort::In, 1.0),
            PatchCable::new(3, 3, PAD_FILTER_MOD, InPort::In, 1.0),
            PatchCable::new(4, PAD_FILTER_MOD, 5, InPort::In, 1.0),
            PatchCable::new(5, 5, 9, InPort::In, 1.0),
            PatchCable::new(6, 6, 5, InPort::Cv, 1.0),
            PatchCable::new(7, 7, PAD_FILTER_MOD, InPort::Cv, 1.0),
            PatchCable::new(8, 8, PAD_FILTER_MOD, InPort::Cv, 0.28),
        ],
    };
    single_zone_rack(
        zone,
        vec![
            PanelPin::module(1, PAD_FILTER_MOD, ModuleParam::Cutoff, "Cutoff"),
            PanelPin::module(1, PAD_FILTER_MOD, ModuleParam::Resonance, "Res"),
            PanelPin::cable(1, 7, "Env Mod"),
            PanelPin::module(1, 6, ModuleParam::Attack, "Attack"),
            PanelPin::module(1, 6, ModuleParam::Decay, "Decay"),
            PanelPin::module(1, 6, ModuleParam::Sustain, "Sustain"),
            PanelPin::module(1, 6, ModuleParam::Release, "Release"),
            PanelPin::module(1, 9, ModuleParam::Spread, "Spread"),
        ],
    )
}

fn glass_marimba_rack() -> RackPatch {
    let zone = RackZone {
        id: 1,
        name: "Glass Marimba".into(),
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
            RackModule::new(
                1,
                "VCO A",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Sine,
                    level: 0.78,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                2,
                "VCO B",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::BareCubic,
                    reach: 0.35,
                    shape: 0.0,
                    tune_semitones: 12.0,
                    fine_cents: 3.0,
                    level: 0.30,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                MARIMBA_FILTER_MOD,
                "Filter",
                ModuleKind::Filter(FilterModule {
                    mode: FilterMode::ImLpf2,
                    cutoff_hz: 2300.0,
                    resonance: 0.17,
                    drive: 0.05,
                    ..FilterModule::default()
                }),
            ),
            RackModule::new(4, "VCA", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(
                5,
                "Amp Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::Adsr,
                    attack_ms: 1.5,
                    decay_ms: 490.0,
                    sustain: 0.0,
                    release_ms: 340.0,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(
                6,
                "Filter Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::Adsr,
                    attack_ms: 2.0,
                    decay_ms: 175.0,
                    sustain: 0.0,
                    release_ms: 150.0,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(
                7,
                "Noise",
                ModuleKind::Noise(NoiseModule {
                    color: NoiseColor::White,
                    highpass_hz: 5600.0,
                    lowpass_hz: (2300.0 * 2.0_f32.powf(1.60)).clamp(20.0, 20_000.0),
                    level: 0.025,
                }),
            ),
            RackModule::new(8, "Noise VCA", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(
                9,
                "Noise Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::Adsr,
                    attack_ms: 0.2,
                    decay_ms: 10.0,
                    sustain: 0.0,
                    release_ms: 5.0,
                    trigger_mode: TriggerMode::OneShot,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(
                10,
                "Out",
                ModuleKind::Out(OutModule {
                    gain_db: 0.0,
                    pan: 0.0,
                    spread: 0.52,
                }),
            ),
        ],
        cables: vec![
            PatchCable::new(1, 1, MARIMBA_FILTER_MOD, InPort::In, 1.0),
            PatchCable::new(2, 2, MARIMBA_FILTER_MOD, InPort::In, 1.0),
            PatchCable::new(3, MARIMBA_FILTER_MOD, 4, InPort::In, 1.0),
            PatchCable::new(4, 4, 10, InPort::In, 1.0),
            PatchCable::new(5, 5, 4, InPort::Cv, 1.0),
            PatchCable::new(6, 6, MARIMBA_FILTER_MOD, InPort::Cv, 1.60),
            PatchCable::new(7, 7, 8, InPort::In, 1.0),
            PatchCable::new(8, 9, 8, InPort::Cv, 1.0),
            PatchCable::new(9, 8, 10, InPort::In, 1.0),
        ],
    };
    single_zone_rack(
        zone,
        vec![
            PanelPin::module(1, MARIMBA_FILTER_MOD, ModuleParam::Cutoff, "Cutoff"),
            PanelPin::module(1, MARIMBA_FILTER_MOD, ModuleParam::Resonance, "Res"),
            PanelPin::cable(1, 6, "Env Mod"),
            PanelPin::module(1, 5, ModuleParam::Decay, "Decay"),
            PanelPin::module(1, 5, ModuleParam::Release, "Release"),
            PanelPin::module(1, 7, ModuleParam::Level, "Mallet"),
            PanelPin::module(1, 10, ModuleParam::Pan, "Pan"),
        ],
    )
}

fn crystal_chime_rack() -> RackPatch {
    let zone = RackZone {
        id: 1,
        name: "Crystal Chime".into(),
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
            RackModule::new(
                1,
                "VCO A",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Sine,
                    level: 0.80,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                2,
                "VCO B",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::BareCubic,
                    reach: 0.35,
                    shape: 0.0,
                    tune_semitones: 12.0,
                    fine_cents: -4.0,
                    level: 0.24,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                CHIME_FILTER_MOD,
                "Filter",
                ModuleKind::Filter(FilterModule {
                    mode: FilterMode::ImLpf2,
                    cutoff_hz: 2100.0,
                    resonance: 0.14,
                    drive: 0.05,
                    ..FilterModule::default()
                }),
            ),
            RackModule::new(4, "VCA", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(
                5,
                "Amp Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::Adsr,
                    attack_ms: 2.8,
                    decay_ms: 620.0,
                    sustain: 0.0,
                    release_ms: 520.0,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(
                6,
                "Filter Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::Adsr,
                    attack_ms: 2.0,
                    decay_ms: 210.0,
                    sustain: 0.0,
                    release_ms: 150.0,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(
                7,
                "Noise",
                ModuleKind::Noise(NoiseModule {
                    color: NoiseColor::White,
                    highpass_hz: 6400.0,
                    lowpass_hz: (2100.0 * 2.0_f32.powf(1.40)).clamp(20.0, 20_000.0),
                    level: 0.018,
                }),
            ),
            RackModule::new(8, "Noise VCA", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(
                9,
                "Noise Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::Adsr,
                    attack_ms: 0.2,
                    decay_ms: 9.0,
                    sustain: 0.0,
                    release_ms: 5.0,
                    trigger_mode: TriggerMode::OneShot,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(
                10,
                "Out",
                ModuleKind::Out(OutModule {
                    gain_db: 0.0,
                    pan: 0.0,
                    spread: 0.48,
                }),
            ),
        ],
        cables: vec![
            PatchCable::new(1, 1, CHIME_FILTER_MOD, InPort::In, 1.0),
            PatchCable::new(2, 2, CHIME_FILTER_MOD, InPort::In, 1.0),
            PatchCable::new(3, CHIME_FILTER_MOD, 4, InPort::In, 1.0),
            PatchCable::new(4, 4, 10, InPort::In, 1.0),
            PatchCable::new(5, 5, 4, InPort::Cv, 1.0),
            PatchCable::new(6, 6, CHIME_FILTER_MOD, InPort::Cv, 1.40),
            PatchCable::new(7, 7, 8, InPort::In, 1.0),
            PatchCable::new(8, 9, 8, InPort::Cv, 1.0),
            PatchCable::new(9, 8, 10, InPort::In, 1.0),
        ],
    };
    single_zone_rack(
        zone,
        vec![
            PanelPin::module(1, CHIME_FILTER_MOD, ModuleParam::Cutoff, "Cutoff"),
            PanelPin::module(1, CHIME_FILTER_MOD, ModuleParam::Resonance, "Res"),
            PanelPin::cable(1, 6, "Env Mod"),
            PanelPin::module(1, 5, ModuleParam::Decay, "Decay"),
            PanelPin::module(1, 5, ModuleParam::Release, "Release"),
            PanelPin::module(1, 10, ModuleParam::Pan, "Pan"),
        ],
    )
}

fn cello_bass_rack() -> RackPatch {
    let zone = RackZone {
        id: 1,
        name: "Warm Cello".into(),
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
            glide_ms: 75.0,
            pitch_tracking: PitchTracking::Chromatic,
            phase_reset: true,
        },
        modules: vec![
            RackModule::new(
                1,
                "VCO A",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::MobiusJet,
                    level: 0.58,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                2,
                "VCO B",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Sine,
                    level: 0.54,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                CELLO_SUB_MOD,
                "Sub",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Sine,
                    tune_semitones: -12.0,
                    level: 0.42,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                CELLO_FILTER_MOD,
                "Filter",
                ModuleKind::Filter(FilterModule {
                    mode: FilterMode::ImLpf2,
                    cutoff_hz: 580.0,
                    resonance: 0.14,
                    drive: 0.14,
                    ..FilterModule::default()
                }),
            ),
            RackModule::new(5, "VCA", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(
                6,
                "Amp Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::Adsr,
                    attack_ms: 42.0,
                    decay_ms: 520.0,
                    sustain: 0.86,
                    release_ms: 500.0,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(
                7,
                "Filter Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::Adsr,
                    attack_ms: 2.0,
                    decay_ms: 420.0,
                    sustain: 0.0,
                    release_ms: 150.0,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(8, "Out", ModuleKind::Out(OutModule::default())),
        ],
        cables: vec![
            PatchCable::new(1, 1, CELLO_FILTER_MOD, InPort::In, 1.0),
            PatchCable::new(2, 2, CELLO_FILTER_MOD, InPort::In, 1.0),
            PatchCable::new(3, CELLO_SUB_MOD, CELLO_FILTER_MOD, InPort::In, 1.0),
            PatchCable::new(4, CELLO_FILTER_MOD, 5, InPort::In, 1.0),
            PatchCable::new(5, 5, 8, InPort::In, 1.0),
            PatchCable::new(6, 6, 5, InPort::Cv, 1.0),
            PatchCable::new(7, 7, CELLO_FILTER_MOD, InPort::Cv, 0.95),
        ],
    };
    single_zone_rack(
        zone,
        vec![
            PanelPin::module(1, CELLO_FILTER_MOD, ModuleParam::Cutoff, "Cutoff"),
            PanelPin::module(1, CELLO_FILTER_MOD, ModuleParam::Resonance, "Res"),
            PanelPin::module(1, CELLO_SUB_MOD, ModuleParam::Level, "Sub Level"),
            PanelPin::cable(1, 7, "Env Mod"),
            PanelPin::module(1, 6, ModuleParam::Attack, "Attack"),
            PanelPin::module(1, 6, ModuleParam::Release, "Release"),
            PanelPin::zone(1, ZoneParam::Glide, "Glide"),
        ],
    )
}

fn soft_pulse_rack() -> RackPatch {
    let zone = RackZone {
        id: 1,
        name: "Soft Pulse".into(),
        low_midi: 0,
        high_midi: 127,
        root_midi: 38, // D2 soft warm thump
        low_vel: 0.0,
        high_vel: 1.0,
        gain_db: 0.0,
        pan: 0.0,
        choke_group: None,
        voice: VoiceSettings {
            pitch_tracking: PitchTracking::Fixed,
            ..VoiceSettings::default()
        },
        modules: vec![
            RackModule::new(
                1,
                "VCO",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Sine,
                    level: 0.92,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                2,
                "Pitch Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::HoldDecay,
                    attack_ms: 0.1,
                    decay_ms: 32.0,
                    sustain: 0.0,
                    release_ms: 20.0,
                    trigger_mode: TriggerMode::OneShot,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(
                3,
                "Filter",
                ModuleKind::Filter(FilterModule {
                    mode: FilterMode::ImLpf2,
                    cutoff_hz: 4200.0,
                    resonance: 0.12,
                    drive: 0.05,
                    ..FilterModule::default()
                }),
            ),
            RackModule::new(4, "VCA", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(
                5,
                "Amp Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::Adsr,
                    attack_ms: 1.2,
                    decay_ms: 210.0,
                    sustain: 0.0,
                    release_ms: 55.0,
                    trigger_mode: TriggerMode::OneShot,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(
                6,
                "Noise",
                ModuleKind::Noise(NoiseModule {
                    color: NoiseColor::White,
                    highpass_hz: 2200.0,
                    lowpass_hz: 4200.0,
                    level: 0.05,
                }),
            ),
            RackModule::new(7, "Noise VCA", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(
                8,
                "Noise Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::Adsr,
                    attack_ms: 0.2,
                    decay_ms: 7.0,
                    sustain: 0.0,
                    release_ms: 5.0,
                    trigger_mode: TriggerMode::OneShot,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(9, "Out", ModuleKind::Out(OutModule::default())),
        ],
        cables: vec![
            PatchCable::new(1, 2, 1, InPort::PitchCv, 20.0),
            PatchCable::new(2, 1, 3, InPort::In, 1.0),
            PatchCable::new(3, 3, 4, InPort::In, 1.0),
            PatchCable::new(4, 5, 4, InPort::Cv, 1.0),
            PatchCable::new(5, 4, 9, InPort::In, 1.0),
            PatchCable::new(6, 6, 7, InPort::In, 1.0),
            PatchCable::new(7, 8, 7, InPort::Cv, 1.0),
            PatchCable::new(8, 7, 9, InPort::In, 1.0),
        ],
    };
    single_zone_rack(
        zone,
        vec![
            PanelPin::cable(1, 1, "Sweep"),
            PanelPin::module(1, 2, ModuleParam::Decay, "Pitch Dec"),
            PanelPin::module(1, 5, ModuleParam::Decay, "Body Dec"),
            PanelPin::module(1, 3, ModuleParam::Cutoff, "Cutoff"),
            PanelPin::zone(1, ZoneParam::RootMidi, "Root"),
        ],
    )
}

fn micro_click_rack() -> RackPatch {
    let zone = RackZone {
        id: 1,
        name: "Micro Click".into(),
        low_midi: 0,
        high_midi: 127,
        root_midi: 90,
        low_vel: 0.0,
        high_vel: 1.0,
        gain_db: 0.0,
        pan: 0.0,
        choke_group: None,
        voice: VoiceSettings {
            pitch_tracking: PitchTracking::Fixed,
            ..VoiceSettings::default()
        },
        modules: vec![
            RackModule::new(
                1,
                "Noise",
                ModuleKind::Noise(NoiseModule {
                    color: NoiseColor::White,
                    highpass_hz: 5400.0,
                    lowpass_hz: 20_000.0,
                    level: 0.78,
                }),
            ),
            RackModule::new(
                2,
                "Filter",
                ModuleKind::Filter(FilterModule {
                    mode: FilterMode::BandPass,
                    cutoff_hz: 6200.0,
                    resonance: 0.32,
                    drive: 0.05,
                    ..FilterModule::default()
                }),
            ),
            RackModule::new(3, "VCA", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(
                4,
                "Amp Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::Adsr,
                    attack_ms: 0.2,
                    decay_ms: 36.0,
                    sustain: 0.0,
                    release_ms: 5.0,
                    trigger_mode: TriggerMode::OneShot,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(
                5,
                "Out",
                ModuleKind::Out(OutModule {
                    gain_db: 0.0,
                    pan: 0.0,
                    spread: 0.55,
                }),
            ),
        ],
        cables: vec![
            PatchCable::new(1, 1, 2, InPort::In, 1.0),
            PatchCable::new(2, 2, 3, InPort::In, 1.0),
            PatchCable::new(3, 3, 5, InPort::In, 1.0),
            PatchCable::new(4, 4, 3, InPort::Cv, 1.0),
        ],
    };
    single_zone_rack(
        zone,
        vec![
            PanelPin::module(1, 2, ModuleParam::Cutoff, "Cutoff"),
            PanelPin::module(1, 2, ModuleParam::Resonance, "Res"),
            PanelPin::module(1, 4, ModuleParam::Decay, "Decay"),
            PanelPin::module(1, 1, ModuleParam::HighpassHz, "Noise HP"),
            PanelPin::module(1, 5, ModuleParam::Spread, "Spread"),
        ],
    )
}

fn build_glass_cathedral() -> Result<Project> {
    let ids = IdGen::new();

    let ch_pad = ids.next();
    let ch_glass = ids.next();
    let ch_chime = ids.next();
    let ch_bass = ids.next();
    let ch_perc = ids.next();
    let ch_delay = ids.next();
    let ch_reverb = ids.next();

    let dev_delay = ids.next();
    let dev_delay_eq = ids.next();
    let mut delay_ret = Channel::new(ch_delay, ChannelKind::Return, "Tap");
    delay_ret.strip.gain = 0.64;
    delay_ret.inserts = vec![
        fx(
            dev_delay,
            "Tap 5/16",
            EffectKind::Delay(DelayPatch {
                steps: 5.0, // Polymetric 5/16 cascading echoes!
                feedback: 0.47,
                mix: 1.0,
                ..DelayPatch::default()
            }),
        ),
        fx(
            dev_delay_eq,
            "Tap HPF",
            EffectKind::Eq(EqPatch {
                hpf_hz: 260.0,
                low_db: -2.0,
                mid_hz: 3200.0,
                mid_q: 1.0,
                mid_db: -1.5,
                high_db: -2.0,
            }),
        ),
    ];

    let dev_reverb = ids.next();
    let mut reverb_ret = Channel::new(ch_reverb, ChannelKind::Return, "Cathedral");
    reverb_ret.strip.gain = 0.76;
    reverb_ret.inserts = vec![
        fx(
            dev_reverb,
            "Vault Verb",
            EffectKind::Reverb(ReverbPatch {
                size: 0.92,
                decay_seconds: 5.8,
                damping_hz: 3200.0,
                diffusion: 0.72,
                width: 0.96,
                mix: 1.0,
            }),
        ),
        fx(
            ids.next(),
            "Vault HPF",
            EffectKind::Eq(EqPatch {
                hpf_hz: 190.0,
                low_db: -3.0,
                mid_hz: 3000.0,
                mid_q: 0.9,
                mid_db: -1.5,
                high_db: -0.5,
            }),
        ),
    ];

    let mut pad_c = Channel::new(ch_pad, ChannelKind::Audio, "Pad");
    pad_c.strip.gain = 0.94;
    pad_c.inserts = vec![
        fx(
            ids.next(),
            "Pad Chorus",
            EffectKind::Chorus(ChorusPatch {
                rate_hz: 0.30,
                depth: 0.56,
                width: 0.95,
                mix: 0.46,
            }),
        ),
        fx(
            ids.next(),
            "Pad Air",
            EffectKind::Eq(EqPatch {
                hpf_hz: 130.0,
                low_db: -0.8,
                mid_hz: 2800.0,
                mid_q: 0.9,
                mid_db: -1.0,
                high_db: 1.2,
            }),
        ),
    ];
    pad_c.sends = vec![ChannelSend {
        target: ch_reverb,
        amount: 0.54,
    }];

    let dev_glass_eq = ids.next();
    let mut glass_c = Channel::new(ch_glass, ChannelKind::Audio, "Glass");
    glass_c.strip = ChannelStrip {
        gain: 0.80,
        pan: -0.20,
    };
    glass_c.inserts = vec![fx(
        dev_glass_eq,
        "Glass Silk",
        EffectKind::Eq(EqPatch {
            hpf_hz: 180.0,
            low_db: 0.0,
            mid_hz: 3200.0,
            mid_q: 1.1,
            mid_db: -2.8,
            high_db: -1.0,
        }),
    )];
    glass_c.sends = vec![
        ChannelSend {
            target: ch_delay,
            amount: 0.44,
        },
        ChannelSend {
            target: ch_reverb,
            amount: 0.48,
        },
    ];

    let dev_chime_eq = ids.next();
    let mut chime_c = Channel::new(ch_chime, ChannelKind::Audio, "Chimes");
    chime_c.strip = ChannelStrip {
        gain: 0.72,
        pan: 0.38,
    };
    chime_c.inserts = vec![fx(
        dev_chime_eq,
        "Chime Silk",
        EffectKind::Eq(EqPatch {
            hpf_hz: 240.0,
            low_db: -1.0,
            mid_hz: 3800.0,
            mid_q: 1.2,
            mid_db: -3.2,
            high_db: -1.2,
        }),
    )];
    chime_c.sends = vec![
        ChannelSend {
            target: ch_delay,
            amount: 0.50,
        },
        ChannelSend {
            target: ch_reverb,
            amount: 0.56,
        },
    ];

    let dev_cello_eq = ids.next();
    let mut bass_c = Channel::new(ch_bass, ChannelKind::Audio, "Cello");
    bass_c.strip.gain = 0.84;
    bass_c.inserts = vec![fx(
        dev_cello_eq,
        "Cello Warm",
        EffectKind::Eq(EqPatch {
            hpf_hz: 30.0,
            low_db: 1.5,
            mid_hz: 650.0,
            mid_q: 0.8,
            mid_db: -1.5,
            high_db: 0.0,
        }),
    )];

    let mut perc_c = Channel::new(ch_perc, ChannelKind::Audio, "Micro");
    perc_c.strip.gain = 0.64;
    perc_c.sends = vec![
        ChannelSend {
            target: ch_delay,
            amount: 0.25,
        },
        ChannelSend {
            target: ch_reverb,
            amount: 0.14,
        },
    ];

    let mixer = MixerGraph {
        channels: vec![
            pad_c, glass_c, chime_c, bass_c, perc_c, delay_ret, reverb_ret,
        ],
        master: MasterChannel {
            strip: ChannelStrip {
                gain: 1.0,
                pan: 0.0,
            },
            inserts: vec![
                fx(
                    ids.next(),
                    "Tape Warmth",
                    EffectKind::Drive(DrivePatch {
                        mode: DriveMode::AtomicJet,
                        drive: 0.14,
                        asymmetry: 0.0,
                        tone_hz: 12_000.0,
                        output_db: 0.0,
                        mix: 0.55,
                    }),
                ),
                fx(
                    ids.next(),
                    "Cathedral Glue",
                    EffectKind::Compressor(CompressorPatch {
                        threshold_db: -15.5,
                        ratio: 1.75,
                        attack_ms: 30.0,
                        release_ms: 240.0,
                        makeup_db: 1.4,
                        mix: 0.80,
                    }),
                ),
                fx(
                    ids.next(),
                    "Master Tone",
                    EffectKind::Eq(EqPatch {
                        hpf_hz: 26.0,
                        low_db: 0.8,
                        mid_hz: 3200.0,
                        mid_q: 0.75,
                        mid_db: -0.8,
                        high_db: 0.5,
                    }),
                ),
                fx(
                    ids.next(),
                    "Master Limiter",
                    EffectKind::Limiter(LimiterPatch {
                        ceiling_db: -1.5,
                        input_drive_db: 1.6,
                        release_ms: 150.0,
                        mix: 1.0,
                    }),
                ),
            ],
        },
    };

    let inst_pad = ids.next();
    let inst_glass = ids.next();
    let inst_chime = ids.next();
    let inst_bass = ids.next();
    let inst_thump = ids.next();
    let inst_click = ids.next();

    let instruments = vec![
        InstrumentInstance {
            id: inst_pad,
            name: "Cathedral Pad".into(),
            channel: ch_pad,
            rack: cathedral_pad_rack(),
        },
        InstrumentInstance {
            id: inst_glass,
            name: "Glass Marimba".into(),
            channel: ch_glass,
            rack: glass_marimba_rack(),
        },
        InstrumentInstance {
            id: inst_chime,
            name: "Crystal Chime".into(),
            channel: ch_chime,
            rack: crystal_chime_rack(),
        },
        InstrumentInstance {
            id: inst_bass,
            name: "Warm Cello".into(),
            channel: ch_bass,
            rack: cello_bass_rack(),
        },
        InstrumentInstance {
            id: inst_thump,
            name: "Soft Pulse".into(),
            channel: ch_perc,
            rack: soft_pulse_rack(),
        },
        InstrumentInstance {
            id: inst_click,
            name: "Micro Click".into(),
            channel: ch_perc,
            rack: micro_click_rack(),
        },
    ];

    let mut patterns = Vec::new();

    // 4-bar Dorian modal drift: Dm9 (0) → G13 (+3) → Cmaj7 (-1) → Am7 (+4)
    let pat_harmony = ids.next();
    patterns.push(Pattern {
        id: pat_harmony,
        name: "Dorian Drift".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Degrees,
            period: 128,
            mode: ValueMode::Hold,
            points: vec![
                ValuePoint {
                    step: 0,
                    value: 0.0,
                },
                ValuePoint {
                    step: 32,
                    value: 3.0,
                },
                ValuePoint {
                    step: 64,
                    value: -1.0,
                },
                ValuePoint {
                    step: 96,
                    value: 4.0,
                },
            ],
        },
    });

    let pat_chord_cell = ids.next();
    let mut cc = vec![Step::default(); 32];
    cc[0] = chord_step(&[1, 3, 5, 7, 9], 0.78, 15.8);
    cc[16] = chord_step(&[1, 3, 5, 7, 9], 0.68, 15.8);
    patterns.push(Pattern {
        id: pat_chord_cell,
        name: "Open 9th".into(),
        kind: PatternKind::Notes { steps: cc },
    });

    let pat_chords = ids.next();
    patterns.push(Pattern {
        id: pat_chords,
        name: "Cathedral Chords".into(),
        kind: PatternKind::AddDegrees {
            notes: pat_chord_cell,
            values: pat_harmony,
        },
    });

    // Generative polymetric 7-step Seeded melody masked by Euclidean(5, 11) + Reverse every 3rd cycle!
    let pat_seed_mel = ids.next();
    patterns.push(Pattern {
        id: pat_seed_mel,
        name: "Seeded 7".into(),
        kind: PatternKind::SeededValues {
            unit: ValueUnit::Degrees,
            period: 7,
            seed: 2026,
            min: 0.0,
            max: 11.0,
        },
    });
    let pat_euc_11 = ids.next();
    patterns.push(Pattern {
        id: pat_euc_11,
        name: "Euclid 5/11".into(),
        kind: PatternKind::Euclidean {
            pulses: 5,
            steps: 11,
            rotation: 1,
        },
    });
    let pat_glass_raw = ids.next();
    patterns.push(Pattern {
        id: pat_glass_raw,
        name: "Glass Stream".into(),
        kind: PatternKind::NotesFromValues {
            values: pat_seed_mel,
            rhythm: pat_euc_11,
            degree_offset: 7,
            velocity: 0.74,
            gate: 2.2,
        },
    });
    let pat_glass_rev = ids.next();
    patterns.push(Pattern {
        id: pat_glass_rev,
        name: "Glass Mirror".into(),
        kind: PatternKind::Reverse {
            source: pat_glass_raw,
        },
    });
    let pat_glass_every = ids.next();
    patterns.push(Pattern {
        id: pat_glass_every,
        name: "Glass Phasing".into(),
        kind: PatternKind::Every {
            interval: 3,
            base: pat_glass_raw,
            alt: pat_glass_rev,
        },
    });

    // Random Arp twinkling softly across the held Dorian 9th chords + Triplet TimeScale option!
    let pat_arp_rand = ids.next();
    patterns.push(Pattern {
        id: pat_arp_rand,
        name: "Rain Arp".into(),
        kind: PatternKind::Arp {
            source: pat_chords,
            mode: ArpMode::Random,
            rate: 1,
        },
    });
    let pat_arp_triplets = ids.next();
    patterns.push(Pattern {
        id: pat_arp_triplets,
        name: "Triplet Rain".into(),
        kind: PatternKind::TimeScale {
            source: pat_arp_rand,
            num: 3,
            den: 2,
        },
    });

    // Shimmering inline Mini-Notation crystalline chimes
    let pat_chime_mini = ids.next();
    patterns.push(Pattern {
        id: pat_chime_mini,
        name: "Cathedral Bells".into(),
        kind: PatternKind::Mini {
            code: "d5 ~ [a5 c6] ~ ~ f5 [e5 d5] ~ a4 ~ [c5 e5 g5] ~ ~ [f5 a5] ~ ~".into(),
            steps: 16,
        },
    });

    // Deep resonant Cello drone folded cleanly into D2..D3!
    let pat_bass_rhy = ids.next();
    patterns.push(Pattern {
        id: pat_bass_rhy,
        name: "Cello Bow".into(),
        kind: PatternKind::Rhythm {
            steps: vec![
                true, false, false, false, false, false, false, false, false, false, false, false,
                false, false, false, false, true, false, false, false, false, false, false, false,
                false, false, false, false, false, false, false, false,
            ],
        },
    });
    let pat_bass_raw = ids.next();
    patterns.push(Pattern {
        id: pat_bass_raw,
        name: "Cello Root".into(),
        kind: PatternKind::NotesFromValues {
            values: pat_harmony,
            rhythm: pat_bass_rhy,
            degree_offset: 0,
            velocity: 0.78,
            gate: 15.5,
        },
    });
    let pat_bass_fold = ids.next();
    patterns.push(Pattern {
        id: pat_bass_fold,
        name: "Cello Drone".into(),
        kind: PatternKind::RangeFold {
            source: pat_bass_raw,
            low_midi: 36,
            high_midi: 48,
        },
    });

    // Subtle IDM micro-rhythm: Euclidean(5, 16) soft pulse + Chance-sculpted clicks
    let pat_thump_euc = ids.next();
    patterns.push(Pattern {
        id: pat_thump_euc,
        name: "Pulse 5/16".into(),
        kind: PatternKind::Euclidean {
            pulses: 5,
            steps: 16,
            rotation: 0,
        },
    });
    let pat_zero = ids.next();
    patterns.push(Pattern {
        id: pat_zero,
        name: "Root Degree".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Degrees,
            period: 16,
            mode: ValueMode::Hold,
            points: vec![ValuePoint {
                step: 0,
                value: 0.0,
            }],
        },
    });
    let pat_thump = ids.next();
    patterns.push(Pattern {
        id: pat_thump,
        name: "IDM Pulse".into(),
        kind: PatternKind::NotesFromValues {
            values: pat_zero,
            rhythm: pat_thump_euc,
            degree_offset: 0,
            velocity: 0.72,
            gate: 0.8,
        },
    });

    let pat_click_base = ids.next();
    patterns.push(Pattern {
        id: pat_click_base,
        name: "Click Grid".into(),
        kind: PatternKind::Notes {
            steps: vec![step_note(1, 0.56, 0.4); 16],
        },
    });
    let pat_click_swung = ids.next();
    patterns.push(Pattern {
        id: pat_click_swung,
        name: "Swung Click".into(),
        kind: PatternKind::Swing {
            source: pat_click_base,
            amount: 0.22,
        },
    });
    let pat_click_chance = ids.next();
    patterns.push(Pattern {
        id: pat_click_chance,
        name: "Micro Dust".into(),
        kind: PatternKind::Chance {
            source: pat_click_swung,
            probability: 0.36,
            seed: 909,
        },
    });

    // 32-bar Vault Bloom macro arc (sweeps Pad cutoff + Chimes/Glass Reverb sends + Vault damping)
    let pat_vault_bloom = ids.next();
    patterns.push(Pattern {
        id: pat_vault_bloom,
        name: "Vault Bloom Arc".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 512,
            mode: ValueMode::Ramp,
            points: vec![
                ValuePoint {
                    step: 0,
                    value: 0.24,
                },
                ValuePoint {
                    step: 64,
                    value: 0.38,
                },
                ValuePoint {
                    step: 128,
                    value: 0.50,
                },
                ValuePoint {
                    step: 192,
                    value: 0.62,
                },
                ValuePoint {
                    step: 256,
                    value: 0.74,
                },
                ValuePoint {
                    step: 336,
                    value: 0.86,
                },
                ValuePoint {
                    step: 384,
                    value: 0.72,
                },
                ValuePoint {
                    step: 448,
                    value: 0.40,
                },
                ValuePoint {
                    step: 496,
                    value: 0.16,
                },
                ValuePoint {
                    step: 511,
                    value: 0.10,
                },
            ],
        },
    });

    // 8-bar phrasing peak swell for Glass -> 5/16 Tap Delay (breathes across chord cadences)
    let pat_tap_swell = ids.next();
    patterns.push(Pattern {
        id: pat_tap_swell,
        name: "Tap Phrasing Swell".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 128,
            mode: ValueMode::Ramp,
            points: vec![
                ValuePoint {
                    step: 0,
                    value: 0.34,
                },
                ValuePoint {
                    step: 48,
                    value: 0.58,
                },
                ValuePoint {
                    step: 64,
                    value: 0.38,
                },
                ValuePoint {
                    step: 112,
                    value: 0.66,
                },
                ValuePoint {
                    step: 127,
                    value: 0.36,
                },
            ],
        },
    });

    // 32-bar Pad -> Cathedral Reverb spacious bloom & dissolving outro tail
    let pat_pad_verb_swell = ids.next();
    patterns.push(Pattern {
        id: pat_pad_verb_swell,
        name: "Cathedral Halo Swell".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 512,
            mode: ValueMode::Ramp,
            points: vec![
                ValuePoint {
                    step: 0,
                    value: 0.50,
                },
                ValuePoint {
                    step: 128,
                    value: 0.58,
                },
                ValuePoint {
                    step: 256,
                    value: 0.66,
                },
                ValuePoint {
                    step: 384,
                    value: 0.62,
                },
                ValuePoint {
                    step: 480,
                    value: 0.74,
                },
                ValuePoint {
                    step: 511,
                    value: 0.54,
                },
            ],
        },
    });

    let part_pad = ids.next();
    let part_glass = ids.next();
    let part_rain = ids.next();
    let part_chime = ids.next();
    let part_bass = ids.next();
    let part_thump = ids.next();
    let part_click = ids.next();

    let parts = vec![
        Part {
            id: part_pad,
            name: "Cathedral Pad".into(),
            pattern: pat_chords,
            instrument: inst_pad,
            transpose: 0,
            velocity_scale: 0.72,
            parameter_links: Vec::new(),
        },
        Part {
            id: part_glass,
            name: "Glass 5/11".into(),
            pattern: pat_glass_every,
            instrument: inst_glass,
            transpose: 0,
            velocity_scale: 0.80,
            parameter_links: Vec::new(),
        },
        Part {
            id: part_rain,
            name: "Triplet Rain".into(),
            pattern: pat_arp_triplets,
            instrument: inst_glass,
            transpose: 12,
            velocity_scale: 0.52,
            parameter_links: Vec::new(),
        },
        Part {
            id: part_chime,
            name: "Mini Chimes".into(),
            pattern: pat_chime_mini,
            instrument: inst_chime,
            transpose: 0,
            velocity_scale: 0.72,
            parameter_links: Vec::new(),
        },
        Part {
            id: part_bass,
            name: "Cello Drone".into(),
            pattern: pat_bass_fold,
            instrument: inst_bass,
            transpose: 0,
            velocity_scale: 0.84,
            parameter_links: Vec::new(),
        },
        Part {
            id: part_thump,
            name: "IDM Pulse".into(),
            pattern: pat_thump,
            instrument: inst_thump,
            transpose: 0,
            velocity_scale: 0.82,
            parameter_links: Vec::new(),
        },
        Part {
            id: part_click,
            name: "Micro Dust".into(),
            pattern: pat_click_chance,
            instrument: inst_click,
            transpose: 0,
            velocity_scale: 0.72,
            parameter_links: Vec::new(),
        },
    ];

    let place = |part: Id, start_bar: u32, bars: u32| Placement {
        id: ids.next(),
        part,
        start_step: start_bar * 16,
        length_steps: bars * 16,
        phase: PhaseMode::Song,
        offset_steps: 0,
        exceptions: Vec::new(),
    };
    let lp = |part: Id| LanePart {
        part,
        phase: PhaseMode::Song,
        offset_steps: 0,
    };

    let lanes = vec![
        Lane {
            id: ids.next(),
            name: "Pad".into(),
            parts: vec![lp(part_pad)],
            // Ends after bar 31 so bar 32 lets the final Am7 chord dissolve naturally into the 5.8s Vault reverb tail.
            placements: vec![place(part_pad, 0, 31)],
        },
        Lane {
            id: ids.next(),
            name: "Glass".into(),
            parts: vec![lp(part_glass), lp(part_rain)],
            placements: vec![place(part_glass, 4, 12), place(part_rain, 16, 12)],
        },
        Lane {
            id: ids.next(),
            name: "Chimes".into(),
            parts: vec![lp(part_chime)],
            placements: vec![place(part_chime, 8, 20)],
        },
        Lane {
            id: ids.next(),
            name: "Cello".into(),
            parts: vec![lp(part_bass)],
            placements: vec![place(part_bass, 4, 26)],
        },
        Lane {
            id: ids.next(),
            name: "Pulse".into(),
            parts: vec![lp(part_thump)],
            placements: vec![place(part_thump, 12, 16)],
        },
        Lane {
            id: ids.next(),
            name: "Dust".into(),
            parts: vec![lp(part_click)],
            placements: vec![place(part_click, 12, 16)],
        },
    ];

    let sections = vec![
        Section {
            id: ids.next(),
            name: "Nave".into(),
            start_step: 0,
            length_steps: 128,
        },
        Section {
            id: ids.next(),
            name: "Refraction".into(),
            start_step: 128,
            length_steps: 128,
        },
        Section {
            id: ids.next(),
            name: "Resonance".into(),
            start_step: 256,
            length_steps: 128,
        },
        Section {
            id: ids.next(),
            name: "Stillness".into(),
            start_step: 384,
            length_steps: 128,
        },
    ];

    // 4 Playable Ambient Project Macros (M1..M4).
    // Static macros (M2..M4) rest at 0.50 with symmetric bounds around the calibrated studio mix.
    let tap_gain_center = channel_gain_to_norm(0.64);
    let macros = vec![
        MacroControl {
            index: 0,
            name: "Vault Bloom".into(),
            value: 0.45,
            bindings: vec![
                MacroBinding::new(
                    ParamAddress::Module {
                        instrument: inst_pad,
                        zone: 1,
                        module: PAD_FILTER_MOD,
                        param: ModuleParam::Cutoff,
                    },
                    ModuleParam::Cutoff.normalize(340.0),
                    ModuleParam::Cutoff.normalize(3200.0),
                ),
                MacroBinding::new(
                    ParamAddress::Send {
                        channel: ChannelRef::Channel(ch_chime),
                        target: ch_reverb,
                    },
                    0.36,
                    0.78,
                ),
                MacroBinding::new(
                    ParamAddress::Send {
                        channel: ChannelRef::Channel(ch_glass),
                        target: ch_reverb,
                    },
                    0.32,
                    0.68,
                ),
                MacroBinding::new(
                    ParamAddress::Effect {
                        device: dev_reverb,
                        param: EffectParam::Damping,
                    },
                    EffectParam::Damping.normalize(2100.0),
                    EffectParam::Damping.normalize(3900.0),
                ),
            ],
        },
        MacroControl {
            index: 1,
            name: "Glass Shimmer".into(),
            value: 0.50,
            bindings: vec![
                MacroBinding::new(
                    ParamAddress::Module {
                        instrument: inst_glass,
                        zone: 1,
                        module: MARIMBA_FILTER_MOD,
                        param: ModuleParam::Cutoff,
                    },
                    ModuleParam::Cutoff.normalize(1150.0),
                    ModuleParam::Cutoff.normalize(4600.0),
                ),
                MacroBinding::new(
                    ParamAddress::Module {
                        instrument: inst_chime,
                        zone: 1,
                        module: CHIME_FILTER_MOD,
                        param: ModuleParam::Cutoff,
                    },
                    ModuleParam::Cutoff.normalize(1050.0),
                    ModuleParam::Cutoff.normalize(4200.0),
                ),
                MacroBinding::new(
                    ParamAddress::Effect {
                        device: dev_glass_eq,
                        param: EffectParam::High,
                    },
                    EffectParam::High.normalize(-3.5),
                    EffectParam::High.normalize(1.5),
                ),
                MacroBinding::new(
                    ParamAddress::Effect {
                        device: dev_chime_eq,
                        param: EffectParam::High,
                    },
                    EffectParam::High.normalize(-4.0),
                    EffectParam::High.normalize(1.6),
                ),
            ],
        },
        MacroControl {
            index: 2,
            name: "Echo Orbit".into(),
            value: 0.50,
            bindings: vec![
                MacroBinding::new(
                    ParamAddress::Effect {
                        device: dev_delay,
                        param: EffectParam::Feedback,
                    },
                    EffectParam::Feedback.normalize(0.22),
                    EffectParam::Feedback.normalize(0.72),
                ),
                MacroBinding::new(
                    ParamAddress::Channel {
                        channel: ChannelRef::Channel(ch_delay),
                        param: ChannelParam::Gain,
                    },
                    (tap_gain_center - 0.18).clamp(0.0, 1.0),
                    (tap_gain_center + 0.18).clamp(0.0, 1.0),
                ),
                MacroBinding::new(
                    ParamAddress::Effect {
                        device: dev_delay_eq,
                        param: EffectParam::High,
                    },
                    EffectParam::High.normalize(-4.0),
                    EffectParam::High.normalize(0.0),
                ),
            ],
        },
        MacroControl {
            index: 3,
            name: "Cello Depth".into(),
            value: 0.50,
            bindings: vec![
                MacroBinding::new(
                    ParamAddress::Module {
                        instrument: inst_bass,
                        zone: 1,
                        module: CELLO_FILTER_MOD,
                        param: ModuleParam::Cutoff,
                    },
                    ModuleParam::Cutoff.normalize(240.0),
                    ModuleParam::Cutoff.normalize(1400.0),
                ),
                MacroBinding::new(
                    ParamAddress::Module {
                        instrument: inst_bass,
                        zone: 1,
                        module: CELLO_SUB_MOD,
                        param: ModuleParam::Level,
                    },
                    ModuleParam::Level.normalize(0.18),
                    ModuleParam::Level.normalize(0.66),
                ),
                MacroBinding::new(
                    ParamAddress::Effect {
                        device: dev_cello_eq,
                        param: EffectParam::Low,
                    },
                    EffectParam::Low.normalize(-1.0),
                    EffectParam::Low.normalize(4.0),
                ),
            ],
        },
    ];

    let modulations = vec![
        Modulation {
            id: ids.next(),
            name: "Vault Bloom".into(),
            target: ModulationTarget::Macro { index: 0 },
            pattern: pat_vault_bloom,
        },
        Modulation {
            id: ids.next(),
            name: "Glass Tap Swell".into(),
            target: ModulationTarget::Send {
                channel: ChannelRef::Channel(ch_glass),
                target: ch_delay,
            },
            pattern: pat_tap_swell,
        },
        Modulation {
            id: ids.next(),
            name: "Pad Vault Swell".into(),
            target: ModulationTarget::Send {
                channel: ChannelRef::Channel(ch_pad),
                target: ch_reverb,
            },
            pattern: pat_pad_verb_swell,
        },
    ];

    let project = Project {
        version: VERSION,
        title: "Glass Cathedral".into(),
        bpm: 108.0,
        root_midi: 50, // D3 (D Dorian)
        scale: vec![0, 2, 3, 5, 7, 9, 10],
        bars: 32,
        patterns,
        instruments,
        parts,
        lanes,
        sections,
        mixer,
        ducks: Vec::new(),
        modulations,
        macros,
        timing: LiveTiming::default(),
        end: EndRule::Loop,
    };
    project.validate()?;
    Ok(project)
}

fn main() -> Result<()> {
    let project = build_glass_cathedral()?;
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/glass-cathedral");
    std::fs::create_dir_all(&dir)?;
    let out_path = dir.join("project.json");
    let mut json = Vec::new();
    write_document_json(&mut json, &project, &LiveFlags::default())?;
    std::fs::write(&out_path, &json)?;
    println!(
        "Wrote {} ({} bytes · {} patterns · {} parts · {} instruments · {} macros · {} modulations)",
        out_path.display(),
        json.len(),
        project.patterns.len(),
        project.parts.len(),
        project.instruments.len(),
        project.macros.len(),
        project.modulations.len()
    );
    if std::env::args().any(|a| a == "--analyze") {
        let score = project.compile()?;
        let mut library = SampleLibrary::from_roots(&[], Some(&out_path));
        let samples = load_project_samples(&score, &mut library);
        samples.ensure_ready()?;
        let (rep, pcm) = render_schedule_pcm(&[(0, &score)], 0, 512, samples.bank, 48_000)?;
        let mut peak = 0.0_f64;
        let mut sq = 0.0_f64;
        for &[li, ri] in &pcm {
            let l = f64::from(li) / 32768.0;
            let r = f64::from(ri) / 32768.0;
            peak = peak.max(l.abs()).max(r.abs());
            sq += 0.5 * (l * l + r * r);
        }
        let rms = (sq / pcm.len().max(1) as f64).sqrt();
        let tail_peak = rep.tail.peak[0].max(rep.tail.peak[1]);
        println!(
            "Glass Cathedral 48 kHz: peak={:+.2} dBFS  RMS={:+.2} dBFS  tail={:+.2} dBFS  clips={}",
            lin_to_db_f64(peak),
            lin_to_db_f64(rms),
            lin_to_db_f64(tail_peak),
            rep.output.full_scale_samples
        );
        let total_steps = (project.bars * 16).max(1) as usize;
        for sec in &project.sections {
            let start = (sec.start_step as usize * pcm.len()) / total_steps;
            let end = (((sec.start_step + sec.length_steps) as usize * pcm.len()) / total_steps)
                .min(pcm.len());
            let mut s_peak = 0.0_f64;
            let mut s_sq = 0.0_f64;
            let slice = &pcm[start..end];
            for &[li, ri] in slice {
                let l = f64::from(li) / 32768.0;
                let r = f64::from(ri) / 32768.0;
                s_peak = s_peak.max(l.abs()).max(r.abs());
                s_sq += 0.5 * (l * l + r * r);
            }
            let s_rms = (s_sq / slice.len().max(1) as f64).sqrt();
            println!(
                "  Section {:<11} peak={:+.2} dBFS  RMS={:+.2} dBFS",
                sec.name,
                lin_to_db_f64(s_peak),
                lin_to_db_f64(s_rms)
            );
        }
        for fx_rep in &rep.effects {
            println!(
                "  FX {:?} (#{}) max_gr={:.2} dB  avg_gr={:.2} dB",
                fx_rep.kind, fx_rep.device_id, fx_rep.max_gr_db, fx_rep.avg_gr_db
            );
        }
    }
    Ok(())
}
