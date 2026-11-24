//! Reproducible builder for Cadence's flagship modern melodic EDM showcase: **"Afterglow"**.
//!
//! 64 bars · 128 BPM · F natural minor (~2:00 full journey).
//! Demonstrates Cadence's complete production stack:
//! - Retained pattern algebra (`Harmony` driving 6 melodic/bass layers via `AddDegrees`, `Mask`,
//!   `Arp`, `RangeFold`, `Shape`, `Stack`, `Every`, `Concat`, `Ratchet`, `Swing`, `Chance`,
//!   `Euclidean`, `SeededValues`, and inline `Mini` notation `"Bell Mini"`)
//! - First-class MIT **`RackPatch`** (`EDM Kit` referencing relative `../../factory-samples/edm-kit/`
//!   WAVs) with stereo zone layout, hi-hat choke group 1, and per-zone/module P-locks
//! - 6 playable & automated **Project Macros** (`M1 Filter Lift`, `M2 Drop Wash`, `M3 Kit Crisp`,
//!   `M4 Bass Grit`, `M5 Space Air`, `M6 Gate Pump`) driving synths, sends, zones, and rhythmic `Filter`
//! - Studio bus mixing & mastering (20 FX across 9 Audio channels, 2 Groups, 2 Returns & Master:
//!   `Compressor`, `Drive`, `Eq`, rhythmic `Filter`, `Chorus`, `Delay(PingPong)`, `Reverb`, and
//!   `Master Limiter` at `-0.6 dBFS` ceiling), plus dedicated `FX Sweep` riser/downlifter lane
//!   and 5 sidechain `Duck` envelopes off the sculpted F1 Analog Kick.
//!
//! Run from the repository root:
//! ```bash
//! cargo run --release --example build-demo -- --analyze
//! ```

use anyhow::Result;
use cadence_core::assets::load_project_samples;
use cadence_core::dsp::lin_to_db_f64;
use cadence_core::macros::{MacroBinding, MacroControl};
use cadence_core::material::{
    write_document_json, ArpMode, Id, InstrumentInstance, Lane, LanePart, LiveFlags, LiveTiming,
    MixerGraph, Modulation, ModulationTarget, ParameterLink, Part, Pattern, PatternKind, PhaseMode,
    Placement, Project, Section, ValuePoint, ValueUnit, VERSION,
};
use cadence_core::mixer::{
    Channel, ChannelKind, ChannelRef, ChannelSend, ChannelStrip, ChorusPatch, CompressorPatch,
    DelayMode, DelayPatch, DriveMode, DrivePatch, Duck, EffectDevice, EffectKind, EffectParam,
    EqPatch, FilterPatch, LimiterPatch, MasterChannel, ReverbPatch,
};
use cadence_core::music::{midi_to_degree, pitch_midi, ChordTone, EndRule, Step};
use cadence_core::params::{InstrumentParam, ParamAddress, ValueMode};
use cadence_core::rack::{
    drum_midi, normalize_cable_amount, EnvMode, EnvModule, FilterMode, FilterModule, InPort,
    LfoModule, LfoShape, ModuleKind, ModuleKindTag, ModuleParam, NoiseColor, NoiseModule,
    OutModule, PanelPin, PatchCable, PitchTracking, RackModule, RackParam, RackPatch, RackZone,
    SamplerModule, TriggerMode, VcaModule, VcoModule, VoiceMode, VoiceSettings, Waveform,
    ZoneParam,
};
use cadence_core::sampler::SampleLibrary;
use cadence_engine::audio::render_schedule_pcm;
use std::path::PathBuf;

const ROOT_MIDI: u8 = 53; // F3 (F natural minor)
const F_MINOR: [i8; 7] = [0, 2, 3, 5, 7, 8, 10];

// Stable 1-based zone & module IDs inside `EDM Kit` Rack (used by Macros & P-locks).
const ZONE_SD: u16 = 1;
const ZONE_CH: u16 = 3;
const ZONE_OH: u16 = 4;
const ZONE_RS: u16 = 9;
const KIT_SAMPLER_MOD: u16 = 1;
const KIT_FILTER_MOD: u16 = 2;

// Stable module & cable IDs inside synth Racks referenced by Macros & Modulations.
const BASS_FILTER_MOD: u16 = 4;
const SUPERSAW_FILTER_MOD: u16 = 3;
const LEAD_FILTER_MOD: u16 = 3;
const ARP_FILTER_MOD: u16 = 3;
const BELL_FM_CABLE: u16 = 7;
const FX_SWEEP_FILTER_MOD: u16 = 4;

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

fn drum_step(midi: u8, velocity: f32, gate: f32) -> Step {
    let (degree, accidental) = midi_to_degree(ROOT_MIDI, &F_MINOR, midi, 0);
    debug_assert_eq!(pitch_midi(ROOT_MIDI, &F_MINOR, degree, accidental, 0), midi);
    Step {
        active: true,
        degree,
        accidental,
        velocity,
        gate,
        ..Step::default()
    }
}

fn drum_chord(midis: &[u8], velocity: f32, gate: f32) -> Step {
    let mut s = drum_step(midis[0], velocity, gate);
    s.chord = midis[1..]
        .iter()
        .map(|&m| {
            let (degree, accidental) = midi_to_degree(ROOT_MIDI, &F_MINOR, m, 0);
            debug_assert_eq!(pitch_midi(ROOT_MIDI, &F_MINOR, degree, accidental, 0), m);
            ChordTone {
                degree,
                accidental,
                velocity,
                gate,
            }
        })
        .collect();
    s
}

fn rhythm_from_str(pattern: &str) -> Vec<bool> {
    pattern
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| matches!(c, 'x' | 'X' | '1'))
        .collect()
}

fn value_points(pairs: &[(u32, f32)]) -> Vec<ValuePoint> {
    pairs
        .iter()
        .map(|&(step, value)| ValuePoint { step, value })
        .collect()
}

struct ZoneSpec<'a> {
    name: &'a str,
    midi: u8,
    file: &'a str,
    gain_db: f32,
    pan: f32,
    choke_group: Option<u8>,
    coarse_semitones: i16,
    cutoff_hz: f32,
    decay_ms: f32,
    sustain: f32,
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

fn edm_zone(id: u16, spec: ZoneSpec<'_>) -> RackZone {
    let sampler = RackModule::new(
        KIT_SAMPLER_MOD,
        "Sampler",
        ModuleKind::Sampler(SamplerModule {
            path: Some(format!("../../factory-samples/edm-kit/{}", spec.file)),
            root_midi: spec.midi,
            coarse_semitones: spec.coarse_semitones,
            one_shot: true,
            ..SamplerModule::default()
        }),
    );
    let filter = RackModule::new(
        KIT_FILTER_MOD,
        "Filter",
        ModuleKind::Filter(FilterModule {
            mode: FilterMode::LowPass,
            cutoff_hz: spec.cutoff_hz,
            resonance: 0.04,
            drive: 0.0,
            key_tracking: 0.0,
            vel_amount: 0.0,
            mix: 1.0,
        }),
    );
    let (modules, cables) = if spec.sustain < 0.01 {
        (
            vec![
                sampler,
                filter,
                RackModule::new(3, "VCA", ModuleKind::Vca(VcaModule::default())),
                RackModule::new(
                    4,
                    "Amp Env",
                    ModuleKind::Env(EnvModule {
                        mode: EnvMode::Adsr,
                        attack_ms: 0.1,
                        decay_ms: spec.decay_ms,
                        sustain: 0.0,
                        release_ms: 30.0,
                        trigger_mode: TriggerMode::OneShot,
                        ..EnvModule::default()
                    }),
                ),
                RackModule::new(5, "Out", ModuleKind::Out(OutModule::default())),
            ],
            vec![
                PatchCable::new(1, KIT_SAMPLER_MOD, KIT_FILTER_MOD, InPort::In, 1.0),
                PatchCable::new(2, KIT_FILTER_MOD, 3, InPort::In, 1.0),
                PatchCable::new(3, 3, 5, InPort::In, 1.0),
                PatchCable::new(4, 4, 3, InPort::Cv, 1.0),
            ],
        )
    } else {
        (
            vec![
                sampler,
                filter,
                RackModule::new(3, "Out", ModuleKind::Out(OutModule::default())),
            ],
            vec![
                PatchCable::new(1, KIT_SAMPLER_MOD, KIT_FILTER_MOD, InPort::In, 1.0),
                PatchCable::new(2, KIT_FILTER_MOD, 3, InPort::In, 1.0),
            ],
        )
    };
    RackZone {
        id,
        name: spec.name.into(),
        low_midi: spec.midi,
        high_midi: spec.midi,
        root_midi: spec.midi,
        low_vel: 0.0,
        high_vel: 1.0,
        gain_db: spec.gain_db,
        pan: spec.pan,
        choke_group: spec.choke_group,
        voice: VoiceSettings::default(),
        modules,
        cables,
    }
}

fn edm_kit_rack() -> RackPatch {
    RackPatch {
        kit_name: "EDM Kit".into(),
        zones: vec![
            // z1: SD — snappy center snare
            edm_zone(
                1,
                ZoneSpec {
                    name: "SD",
                    midi: drum_midi::SD,
                    file: "02-sd.wav",
                    gain_db: -2.0,
                    pan: 0.0,
                    choke_group: None,
                    coarse_semitones: 0,
                    cutoff_hz: 16_500.0,
                    decay_ms: 180.0,
                    sustain: 1.0,
                },
            ),
            // z2: CP — glossy wide stadium clap
            edm_zone(
                2,
                ZoneSpec {
                    name: "CP",
                    midi: drum_midi::CP,
                    file: "08-cp.wav",
                    gain_db: -2.5,
                    pan: 0.0,
                    choke_group: None,
                    coarse_semitones: 0,
                    cutoff_hz: 18_500.0,
                    decay_ms: 260.0,
                    sustain: 1.0,
                },
            ),
            // z3: CH — tight swung closed hat (choke group 1)
            edm_zone(
                3,
                ZoneSpec {
                    name: "CH",
                    midi: drum_midi::CH,
                    file: "07-ch.wav",
                    gain_db: -4.5,
                    pan: 0.26,
                    choke_group: Some(1),
                    coarse_semitones: 0,
                    cutoff_hz: 15_500.0,
                    decay_ms: 95.0,
                    sustain: 0.85,
                },
            ),
            // z4: OH — shimmering offbeat open hat (choke group 1)
            edm_zone(
                4,
                ZoneSpec {
                    name: "OH",
                    midi: drum_midi::OH,
                    file: "11-oh.wav",
                    gain_db: -3.5,
                    pan: 0.32,
                    choke_group: Some(1),
                    coarse_semitones: 0,
                    cutoff_hz: 17_500.0,
                    decay_ms: 320.0,
                    sustain: 0.0,
                },
            ),
            // z5: CR — lush drop impact crash (left-center)
            edm_zone(
                5,
                ZoneSpec {
                    name: "CR",
                    midi: drum_midi::CB_OR_CR,
                    file: "09-cr.wav",
                    gain_db: -5.5,
                    pan: -0.28,
                    choke_group: None,
                    coarse_semitones: 0,
                    cutoff_hz: 16_500.0,
                    decay_ms: 600.0,
                    sustain: 1.0,
                },
            ),
            // z6: RD — silvery driving Climax ride (right-center)
            edm_zone(
                6,
                ZoneSpec {
                    name: "RD",
                    midi: drum_midi::CY_OR_RD,
                    file: "10-rd.wav",
                    gain_db: -6.0,
                    pan: 0.36,
                    choke_group: None,
                    coarse_semitones: 0,
                    cutoff_hz: 15_000.0,
                    decay_ms: 400.0,
                    sustain: 1.0,
                },
            ),
            // z7: LT — punchy low tom tuned to F minor (left)
            edm_zone(
                7,
                ZoneSpec {
                    name: "LT",
                    midi: drum_midi::LT,
                    file: "03-lt.wav",
                    gain_db: -3.5,
                    pan: -0.38,
                    choke_group: None,
                    coarse_semitones: -1,
                    cutoff_hz: 9_500.0,
                    decay_ms: 220.0,
                    sustain: 1.0,
                },
            ),
            // z8: HT — snappy high tom (right)
            edm_zone(
                8,
                ZoneSpec {
                    name: "HT",
                    midi: drum_midi::HT,
                    file: "05-ht.wav",
                    gain_db: -3.5,
                    pan: 0.34,
                    choke_group: None,
                    coarse_semitones: 1,
                    cutoff_hz: 12_000.0,
                    decay_ms: 180.0,
                    sustain: 1.0,
                },
            ),
            // z9: RS — syncopated woody rimshot (left-mid, modulated via ZoneParam P-locks)
            edm_zone(
                9,
                ZoneSpec {
                    name: "RS",
                    midi: drum_midi::RS,
                    file: "06-rs.wav",
                    gain_db: -6.0,
                    pan: -0.30,
                    choke_group: None,
                    coarse_semitones: 0,
                    cutoff_hz: 13_500.0,
                    decay_ms: 120.0,
                    sustain: 1.0,
                },
            ),
        ],
        gain_db: 0.0,
        pan: 0.0,
        tune_semitones: 0.0,
        cutoff_hz: 16_500.0,
        resonance: 0.04,
        panel_pins: Vec::new(),
    }
}

// Sculpted F1 Analog Kick (pure sine body + crisp transient click, fixed at 43.65 Hz).
fn kick_rack() -> RackPatch {
    let zone = RackZone {
        id: 1,
        name: "Club Kick".into(),
        low_midi: 0,
        high_midi: 127,
        root_midi: 29, // F1 (43.65 Hz) — locked to song tonic!
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
                "Body VCO",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Sine,
                    level: 1.0,
                    unison_detune_cents: 0.0,
                    unison_width: 0.0,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                2,
                "Pitch Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::HoldDecay,
                    attack_ms: 0.1,
                    decay_ms: 27.0,
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
                    mode: FilterMode::LowPass,
                    cutoff_hz: 7800.0,
                    resonance: 0.05,
                    drive: 0.34,
                    ..FilterModule::default()
                }),
            ),
            RackModule::new(4, "Body VCA", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(
                5,
                "Amp Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::Adsr,
                    attack_ms: 0.15,
                    decay_ms: 235.0,
                    sustain: 0.0,
                    release_ms: 32.0,
                    trigger_mode: TriggerMode::OneShot,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(
                6,
                "Click Noise",
                ModuleKind::Noise(NoiseModule {
                    color: NoiseColor::White,
                    highpass_hz: 2600.0,
                    lowpass_hz: 7800.0,
                    level: 0.16,
                }),
            ),
            RackModule::new(7, "Click VCA", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(
                8,
                "Click Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::Adsr,
                    attack_ms: 0.2,
                    decay_ms: 3.2,
                    sustain: 0.0,
                    release_ms: 5.0,
                    trigger_mode: TriggerMode::OneShot,
                    ..EnvModule::default()
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
    single_zone_rack(
        zone,
        vec![
            PanelPin::cable(1, 1, "Sweep"),
            PanelPin::module(1, 2, ModuleParam::Decay, "Pitch Dec"),
            PanelPin::module(1, 5, ModuleParam::Decay, "Body Dec"),
            PanelPin::module(1, 6, ModuleParam::Level, "Click"),
            PanelPin::module(1, 3, ModuleParam::Drive, "Drive"),
            PanelPin::zone(1, ZoneParam::RootMidi, "Root"),
        ],
    )
}

fn perc_rack() -> RackPatch {
    let zone = RackZone {
        id: 1,
        name: "Tech Perc".into(),
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
                    waveform: Waveform::Triangle,
                    level: 0.72,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                2,
                "VCO B",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Sine,
                    tune_semitones: 12.0,
                    level: 1.0,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                3,
                "Filter",
                ModuleKind::Filter(FilterModule {
                    mode: FilterMode::LowPass,
                    cutoff_hz: 3600.0,
                    resonance: 0.28,
                    drive: 0.16,
                    ..FilterModule::default()
                }),
            ),
            RackModule::new(4, "VCA", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(
                5,
                "Amp Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::Adsr,
                    attack_ms: 0.2,
                    decay_ms: 78.0,
                    sustain: 0.0,
                    release_ms: 30.0,
                    trigger_mode: TriggerMode::OneShot,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(
                6,
                "Pitch Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::HoldDecay,
                    attack_ms: 0.1,
                    decay_ms: 18.0,
                    sustain: 0.0,
                    release_ms: 20.0,
                    trigger_mode: TriggerMode::OneShot,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(
                7,
                "Noise",
                ModuleKind::Noise(NoiseModule {
                    color: NoiseColor::White,
                    highpass_hz: 3000.0,
                    lowpass_hz: 3600.0,
                    level: 0.14,
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
                    spread: 0.35,
                }),
            ),
        ],
        cables: vec![
            PatchCable::new(1, 1, 3, InPort::In, 1.0),
            PatchCable::new(2, 2, 3, InPort::In, 0.30),
            PatchCable::new(3, 2, 1, InPort::FmIn, 0.14),
            PatchCable::new(4, 3, 4, InPort::In, 1.0),
            PatchCable::new(5, 4, 10, InPort::In, 1.0),
            PatchCable::new(6, 5, 4, InPort::Cv, 1.0),
            PatchCable::new(7, 6, 1, InPort::PitchCv, 14.0),
            PatchCable::new(8, 6, 2, InPort::PitchCv, 14.0),
            PatchCable::new(9, 7, 8, InPort::In, 1.0),
            PatchCable::new(10, 9, 8, InPort::Cv, 1.0),
            PatchCable::new(11, 8, 10, InPort::In, 1.0),
        ],
    };
    single_zone_rack(
        zone,
        vec![
            PanelPin::cable(1, 3, "FM Depth"),
            PanelPin::cable(1, 7, "Sweep"),
            PanelPin::module(1, 3, ModuleParam::Cutoff, "Cutoff"),
            PanelPin::module(1, 3, ModuleParam::Resonance, "Res"),
            PanelPin::module(1, 5, ModuleParam::Decay, "Decay"),
            PanelPin::module(1, 10, ModuleParam::Spread, "Spread"),
        ],
    )
}

fn bass_rack() -> RackPatch {
    let zone = RackZone {
        id: 1,
        name: "Sub Drive".into(),
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
            glide_ms: 16.0,
            pitch_tracking: PitchTracking::Chromatic,
            phase_reset: true,
        },
        modules: vec![
            RackModule::new(
                1,
                "VCO A",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Saw,
                    fine_cents: -4.0,
                    level: 0.62,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                2,
                "VCO B",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Pulse,
                    fine_cents: 4.0,
                    pulse_width: 0.40,
                    level: 0.32,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                3,
                "Sub",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Sine,
                    tune_semitones: -12.0, // With RangeFold 36..48, pure sine sub hits 34.6..51.9 Hz!
                    level: 0.62,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                BASS_FILTER_MOD,
                "Filter",
                ModuleKind::Filter(FilterModule {
                    mode: FilterMode::LowPass,
                    cutoff_hz: 740.0,
                    resonance: 0.20,
                    drive: 0.35,
                    ..FilterModule::default()
                }),
            ),
            RackModule::new(5, "VCA", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(
                6,
                "Amp Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::Adsr,
                    attack_ms: 1.2,
                    decay_ms: 210.0,
                    sustain: 0.80,
                    release_ms: 68.0,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(
                7,
                "Filter Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::Adsr,
                    attack_ms: 0.8,
                    decay_ms: 140.0,
                    sustain: 0.20,
                    release_ms: 150.0,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(
                8,
                "Out",
                ModuleKind::Out(OutModule {
                    gain_db: 0.0,
                    pan: 0.0,
                    spread: 0.06,
                }),
            ),
        ],
        cables: vec![
            PatchCable::new(1, 1, BASS_FILTER_MOD, InPort::In, 1.0),
            PatchCable::new(2, 2, BASS_FILTER_MOD, InPort::In, 1.0),
            PatchCable::new(3, 3, BASS_FILTER_MOD, InPort::In, 1.0),
            PatchCable::new(4, BASS_FILTER_MOD, 5, InPort::In, 1.0),
            PatchCable::new(5, 5, 8, InPort::In, 1.0),
            PatchCable::new(6, 6, 5, InPort::Cv, 1.0),
            PatchCable::new(7, 7, BASS_FILTER_MOD, InPort::Cv, 1.9),
        ],
    };
    single_zone_rack(
        zone,
        vec![
            PanelPin::module(1, BASS_FILTER_MOD, ModuleParam::Cutoff, "Cutoff"),
            PanelPin::module(1, BASS_FILTER_MOD, ModuleParam::Resonance, "Res"),
            PanelPin::module(1, BASS_FILTER_MOD, ModuleParam::Drive, "Drive"),
            PanelPin::cable(1, 7, "Env Mod"),
            PanelPin::module(1, 3, ModuleParam::Level, "Sub Level"),
            PanelPin::module(1, 6, ModuleParam::Decay, "Decay"),
            PanelPin::zone(1, ZoneParam::Glide, "Glide"),
        ],
    )
}

// Widened/tightened 5-voice Supersaw pad & drop stab stack.
fn supersaw_rack() -> RackPatch {
    let zone = RackZone {
        id: 1,
        name: "Supersaw".into(),
        low_midi: 0,
        high_midi: 127,
        root_midi: 60,
        low_vel: 0.0,
        high_vel: 1.0,
        gain_db: -12.0,
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
                    waveform: Waveform::Saw,
                    fine_cents: -5.0,
                    level: 0.42,
                    unison_voices: 5,
                    unison_detune_cents: 16.0,
                    unison_width: 1.0,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                2,
                "VCO B",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Saw,
                    fine_cents: 5.0,
                    level: 0.40,
                    unison_voices: 5,
                    unison_detune_cents: 16.0,
                    unison_width: 1.0,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                SUPERSAW_FILTER_MOD,
                "Filter",
                ModuleKind::Filter(FilterModule {
                    mode: FilterMode::LowPass,
                    cutoff_hz: 3000.0,
                    resonance: 0.14,
                    drive: 0.16,
                    key_tracking: 0.30,
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
                    attack_ms: 4.0,
                    decay_ms: 460.0,
                    sustain: 0.80,
                    release_ms: 280.0,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(
                6,
                "Filter Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::Adsr,
                    attack_ms: 6.0,
                    decay_ms: 220.0,
                    sustain: 0.48,
                    release_ms: 320.0,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(
                7,
                "LFO",
                ModuleKind::Lfo(LfoModule {
                    shape: LfoShape::Sine,
                    rate_hz: 0.32,
                    retrigger: false,
                    ..LfoModule::default()
                }),
            ),
            RackModule::new(
                8,
                "Out",
                ModuleKind::Out(OutModule {
                    gain_db: 0.0,
                    pan: 0.0,
                    spread: 0.94,
                }),
            ),
        ],
        cables: vec![
            PatchCable::new(1, 1, SUPERSAW_FILTER_MOD, InPort::In, 1.0),
            PatchCable::new(2, 2, SUPERSAW_FILTER_MOD, InPort::In, 1.0),
            PatchCable::new(3, SUPERSAW_FILTER_MOD, 4, InPort::In, 1.0),
            PatchCable::new(4, 4, 8, InPort::In, 1.0),
            PatchCable::new(5, 5, 4, InPort::Cv, 1.0),
            PatchCable::new(6, 6, SUPERSAW_FILTER_MOD, InPort::Cv, 1.65),
            PatchCable::new(7, 7, SUPERSAW_FILTER_MOD, InPort::Cv, 0.08),
        ],
    };
    single_zone_rack(
        zone,
        vec![
            PanelPin::module(1, SUPERSAW_FILTER_MOD, ModuleParam::Cutoff, "Cutoff"),
            PanelPin::module(1, SUPERSAW_FILTER_MOD, ModuleParam::Resonance, "Res"),
            PanelPin::cable(1, 6, "Env Mod"),
            PanelPin::module(1, 1, ModuleParam::UnisonDetune, "Detune"),
            PanelPin::module(1, 5, ModuleParam::Attack, "Attack"),
            PanelPin::module(1, 5, ModuleParam::Release, "Release"),
            PanelPin::module(1, 8, ModuleParam::Spread, "Spread"),
        ],
    )
}

// Octave-stacked 5-voice Anthem Lead with crisp breath-transient noise chiff.
fn lead_rack() -> RackPatch {
    let zone = RackZone {
        id: 1,
        name: "Anthem Lead".into(),
        low_midi: 0,
        high_midi: 127,
        root_midi: 60,
        low_vel: 0.0,
        high_vel: 1.0,
        gain_db: -5.5,
        pan: 0.0,
        choke_group: None,
        voice: VoiceSettings {
            voice_mode: VoiceMode::Poly,
            phase_reset: false,
            ..VoiceSettings::default()
        },
        modules: vec![
            RackModule::new(
                1,
                "VCO A",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Saw,
                    fine_cents: -4.0,
                    level: 0.56,
                    unison_voices: 5,
                    unison_detune_cents: 15.0,
                    unison_width: 0.88,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                2,
                "VCO B",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Saw,
                    tune_semitones: 12.0, // Octave shimmer stack!
                    fine_cents: 4.0,
                    pulse_width: 0.38,
                    level: 0.36,
                    unison_voices: 5,
                    unison_detune_cents: 15.0,
                    unison_width: 0.88,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                LEAD_FILTER_MOD,
                "Filter",
                ModuleKind::Filter(FilterModule {
                    mode: FilterMode::LowPass,
                    cutoff_hz: 4200.0,
                    resonance: 0.15,
                    drive: 0.12,
                    key_tracking: 0.45,
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
                    attack_ms: 1.5,
                    decay_ms: 280.0,
                    sustain: 0.58,
                    release_ms: 250.0,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(
                6,
                "Filter Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::Adsr,
                    attack_ms: 1.0,
                    decay_ms: 145.0,
                    sustain: 0.32,
                    release_ms: 150.0,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(
                7,
                "Noise",
                ModuleKind::Noise(NoiseModule {
                    color: NoiseColor::White,
                    highpass_hz: 5500.0,
                    lowpass_hz: (4200.0 * 2.0_f32.powf(1.85)).clamp(20.0, 20_000.0),
                    level: 0.055,
                }),
            ),
            RackModule::new(8, "Noise VCA", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(
                9,
                "Noise Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::Adsr,
                    attack_ms: 0.2,
                    decay_ms: 22.0,
                    sustain: 0.0,
                    release_ms: 5.0,
                    trigger_mode: TriggerMode::OneShot,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(
                10,
                "LFO",
                ModuleKind::Lfo(LfoModule {
                    shape: LfoShape::Sine,
                    rate_hz: 5.4,
                    ..LfoModule::default()
                }),
            ),
            RackModule::new(
                11,
                "Out",
                ModuleKind::Out(OutModule {
                    gain_db: 0.0,
                    pan: 0.0,
                    spread: 0.72,
                }),
            ),
        ],
        cables: vec![
            PatchCable::new(1, 1, LEAD_FILTER_MOD, InPort::In, 1.0),
            PatchCable::new(2, 2, LEAD_FILTER_MOD, InPort::In, 1.0),
            PatchCable::new(3, LEAD_FILTER_MOD, 4, InPort::In, 1.0),
            PatchCable::new(4, 4, 11, InPort::In, 1.0),
            PatchCable::new(5, 5, 4, InPort::Cv, 1.0),
            PatchCable::new(6, 6, LEAD_FILTER_MOD, InPort::Cv, 1.85),
            PatchCable::new(7, 7, 8, InPort::In, 1.0),
            PatchCable::new(8, 9, 8, InPort::Cv, 1.0),
            PatchCable::new(9, 8, 11, InPort::In, 1.0),
            PatchCable::new(10, 10, 1, InPort::PitchCv, 0.05),
            PatchCable::new(11, 10, 2, InPort::PitchCv, 0.05),
        ],
    };
    single_zone_rack(
        zone,
        vec![
            PanelPin::module(1, LEAD_FILTER_MOD, ModuleParam::Cutoff, "Cutoff"),
            PanelPin::module(1, LEAD_FILTER_MOD, ModuleParam::Resonance, "Res"),
            PanelPin::cable(1, 6, "Env Mod"),
            PanelPin::module(1, 5, ModuleParam::Decay, "Decay"),
            PanelPin::module(1, 5, ModuleParam::Release, "Release"),
            PanelPin::module(1, 11, ModuleParam::Spread, "Spread"),
        ],
    )
}

fn arp_rack() -> RackPatch {
    let zone = RackZone {
        id: 1,
        name: "Glass Arp".into(),
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
                    waveform: Waveform::Triangle,
                    level: 0.64,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                2,
                "VCO B",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Saw,
                    tune_semitones: 12.0,
                    fine_cents: 4.0,
                    level: 1.0,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                ARP_FILTER_MOD,
                "Filter",
                ModuleKind::Filter(FilterModule {
                    mode: FilterMode::LowPass,
                    cutoff_hz: 1900.0,
                    resonance: 0.24,
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
                    attack_ms: 0.6,
                    decay_ms: 210.0,
                    sustain: 0.04,
                    release_ms: 110.0,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(
                6,
                "Filter Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::Adsr,
                    attack_ms: 0.5,
                    decay_ms: 115.0,
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
                    highpass_hz: 5800.0,
                    lowpass_hz: (1900.0 * 2.0_f32.powf(2.4)).clamp(20.0, 20_000.0),
                    level: 0.04,
                }),
            ),
            RackModule::new(8, "Noise VCA", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(
                9,
                "Noise Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::Adsr,
                    attack_ms: 0.2,
                    decay_ms: 8.0,
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
                    spread: 0.50,
                }),
            ),
        ],
        cables: vec![
            PatchCable::new(1, 1, ARP_FILTER_MOD, InPort::In, 1.0),
            PatchCable::new(2, 2, ARP_FILTER_MOD, InPort::In, 0.34),
            PatchCable::new(3, 2, 1, InPort::FmIn, 0.10),
            PatchCable::new(4, ARP_FILTER_MOD, 4, InPort::In, 1.0),
            PatchCable::new(5, 4, 10, InPort::In, 1.0),
            PatchCable::new(6, 5, 4, InPort::Cv, 1.0),
            PatchCable::new(7, 6, ARP_FILTER_MOD, InPort::Cv, 2.4),
            PatchCable::new(8, 7, 8, InPort::In, 1.0),
            PatchCable::new(9, 9, 8, InPort::Cv, 1.0),
            PatchCable::new(10, 8, 10, InPort::In, 1.0),
        ],
    };
    single_zone_rack(
        zone,
        vec![
            PanelPin::module(1, ARP_FILTER_MOD, ModuleParam::Cutoff, "Cutoff"),
            PanelPin::module(1, ARP_FILTER_MOD, ModuleParam::Resonance, "Res"),
            PanelPin::cable(1, 7, "Env Mod"),
            PanelPin::cable(1, 3, "FM Depth"),
            PanelPin::module(1, 5, ModuleParam::Decay, "Decay"),
            PanelPin::module(1, 10, ModuleParam::Spread, "Spread"),
        ],
    )
}

fn bell_rack() -> RackPatch {
    let zone = RackZone {
        id: 1,
        name: "Shimmer Bell".into(),
        low_midi: 0,
        high_midi: 127,
        root_midi: 60,
        low_vel: 0.0,
        high_vel: 1.0,
        gain_db: -2.0,
        pan: 0.0,
        choke_group: None,
        voice: VoiceSettings::default(),
        modules: vec![
            RackModule::new(
                1,
                "VCO A",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Sine,
                    level: 0.72,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                2,
                "VCO B",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Sine,
                    tune_semitones: 12.0,
                    fine_cents: 4.0,
                    level: 1.0,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                3,
                "Filter",
                ModuleKind::Filter(FilterModule {
                    mode: FilterMode::LowPass,
                    cutoff_hz: 5600.0,
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
                    attack_ms: 1.0,
                    decay_ms: 420.0,
                    sustain: 0.06,
                    release_ms: 480.0,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(
                6,
                "Filter Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::Adsr,
                    attack_ms: 2.0,
                    decay_ms: 220.0,
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
                    highpass_hz: 6800.0,
                    lowpass_hz: (5600.0 * 2.0_f32.powf(1.4)).clamp(20.0, 20_000.0),
                    level: 0.03,
                }),
            ),
            RackModule::new(8, "Noise VCA", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(
                9,
                "Noise Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::Adsr,
                    attack_ms: 0.2,
                    decay_ms: 8.0,
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
                    spread: 0.58,
                }),
            ),
        ],
        cables: vec![
            PatchCable::new(1, 1, 3, InPort::In, 1.0),
            PatchCable::new(2, 2, 3, InPort::In, 0.26),
            PatchCable::new(3, 3, 4, InPort::In, 1.0),
            PatchCable::new(4, 4, 10, InPort::In, 1.0),
            PatchCable::new(5, 5, 4, InPort::Cv, 1.0),
            PatchCable::new(6, 6, 3, InPort::Cv, 1.4),
            PatchCable::new(BELL_FM_CABLE, 2, 1, InPort::FmIn, 0.26),
            PatchCable::new(8, 7, 8, InPort::In, 1.0),
            PatchCable::new(9, 9, 8, InPort::Cv, 1.0),
            PatchCable::new(10, 8, 10, InPort::In, 1.0),
        ],
    };
    single_zone_rack(
        zone,
        vec![
            PanelPin::cable(1, BELL_FM_CABLE, "FM Depth"),
            PanelPin::module(1, 3, ModuleParam::Cutoff, "Cutoff"),
            PanelPin::cable(1, 6, "Env Mod"),
            PanelPin::module(1, 5, ModuleParam::Decay, "Decay"),
            PanelPin::module(1, 5, ModuleParam::Release, "Release"),
            PanelPin::module(1, 10, ModuleParam::Spread, "Spread"),
        ],
    )
}

// Mono legato wide noise + fifth-detuned saw riser & drop downlifter synth.
fn fx_sweep_rack() -> RackPatch {
    let zone = RackZone {
        id: 1,
        name: "FX Sweep".into(),
        low_midi: 0,
        high_midi: 127,
        root_midi: 60,
        low_vel: 0.0,
        high_vel: 1.0,
        gain_db: -3.0,
        pan: 0.0,
        choke_group: None,
        voice: VoiceSettings {
            voice_mode: VoiceMode::Mono,
            legato: true,
            glide_ms: 0.0,
            pitch_tracking: PitchTracking::Chromatic,
            phase_reset: false,
        },
        modules: vec![
            RackModule::new(
                1,
                "VCO A",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Saw,
                    fine_cents: -8.0,
                    level: 0.22,
                    unison_voices: 3,
                    unison_detune_cents: 18.0,
                    unison_width: 1.0,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                2,
                "VCO B",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Saw,
                    tune_semitones: 7.0, // Perfect-fifth harmonic lift
                    fine_cents: 8.0,
                    level: 0.15,
                    unison_voices: 3,
                    unison_detune_cents: 18.0,
                    unison_width: 1.0,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                3,
                "Noise",
                ModuleKind::Noise(NoiseModule {
                    color: NoiseColor::White,
                    highpass_hz: 240.0,
                    lowpass_hz: 20_000.0,
                    level: 0.92,
                }),
            ),
            RackModule::new(
                FX_SWEEP_FILTER_MOD,
                "Filter",
                ModuleKind::Filter(FilterModule {
                    mode: FilterMode::LowPass,
                    cutoff_hz: 300.0,
                    resonance: 0.42,
                    drive: 0.12,
                    ..FilterModule::default()
                }),
            ),
            RackModule::new(5, "VCA", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(
                6,
                "Amp Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::Adsr,
                    attack_ms: 45.0,
                    decay_ms: 200.0,
                    sustain: 1.0,
                    release_ms: 360.0,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(
                7,
                "Out",
                ModuleKind::Out(OutModule {
                    gain_db: 0.0,
                    pan: 0.0,
                    spread: 1.0,
                }),
            ),
        ],
        cables: vec![
            PatchCable::new(1, 1, FX_SWEEP_FILTER_MOD, InPort::In, 1.0),
            PatchCable::new(2, 2, FX_SWEEP_FILTER_MOD, InPort::In, 1.0),
            PatchCable::new(3, 3, FX_SWEEP_FILTER_MOD, InPort::In, 1.0),
            PatchCable::new(4, FX_SWEEP_FILTER_MOD, 5, InPort::In, 1.0),
            PatchCable::new(5, 5, 7, InPort::In, 1.0),
            PatchCable::new(6, 6, 5, InPort::Cv, 1.0),
        ],
    };
    single_zone_rack(
        zone,
        vec![
            PanelPin::module(1, FX_SWEEP_FILTER_MOD, ModuleParam::Cutoff, "Cutoff"),
            PanelPin::module(1, FX_SWEEP_FILTER_MOD, ModuleParam::Resonance, "Res"),
            PanelPin::rack(RackParam::Tune, "Pitch"),
            PanelPin::module(1, 3, ModuleParam::Level, "Noise"),
            PanelPin::module(1, 7, ModuleParam::Spread, "Spread"),
        ],
    )
}

fn build_afterglow() -> Result<Project> {
    let ids = IdGen::new();

    // Pre-allocate rhythmic filter gate pattern ID so `Music Filter` binds directly to it.
    let pat_gate_rhythm = ids.next();

    // ─── 1. Mixer Architecture (9 Audio → 2 Groups + 2 Returns → Master, 20 total FX) ───
    let ch_drums_grp = ids.next();
    let ch_music_grp = ids.next();
    let ch_echo_ret = ids.next();
    let ch_hall_ret = ids.next();

    let ch_kick = ids.next();
    let ch_kit = ids.next();
    let ch_perc = ids.next();
    let ch_bass = ids.next();
    let ch_chords = ids.next();
    let ch_lead = ids.next();
    let ch_arp = ids.next();
    let ch_bell = ids.next();
    let ch_fx_sweep = ids.next();

    // FX IDs referenced by Project Macros
    let fx_drum_punch = ids.next();
    let fx_music_filter = ids.next();
    let fx_echo_delay = ids.next();
    let fx_sky_hall = ids.next();
    let fx_bass_drive = ids.next();

    // Drums Group (3 inserts)
    let mut drums_grp = Channel::new(ch_drums_grp, ChannelKind::Group, "Drums");
    drums_grp.strip.gain = 0.76;
    drums_grp.inserts = vec![
        fx(
            ids.next(),
            "Drum Glue",
            EffectKind::Compressor(CompressorPatch {
                threshold_db: -12.0,
                ratio: 2.6,
                attack_ms: 26.0,
                release_ms: 60.0,
                makeup_db: 0.8,
                mix: 0.65,
            }),
        ),
        fx(
            ids.next(),
            "Drum Tape",
            EffectKind::Drive(DrivePatch {
                mode: DriveMode::Tape,
                drive: 0.30,
                asymmetry: 0.0,
                tone_hz: 15_000.0,
                output_db: 0.0,
                mix: 0.30,
            }),
        ),
        fx(
            fx_drum_punch,
            "Drum Punch",
            EffectKind::Eq(EqPatch {
                hpf_hz: 30.0,
                low_db: 1.4,
                mid_db: -1.8,
                mid_hz: 360.0,
                mid_q: 1.2,
                high_db: 1.6,
            }),
        ),
    ];

    // Music Group (3 inserts: rhythmic Filter + Bus Glue Compressor + Carve EQ)
    let mut music_grp = Channel::new(ch_music_grp, ChannelKind::Group, "Music");
    music_grp.strip.gain = 0.76;
    music_grp.inserts = vec![
        fx(
            fx_music_filter,
            "Music Filter",
            EffectKind::Filter(FilterPatch {
                mode: FilterMode::LowPass,
                cutoff_hz: 18_000.0,
                resonance: 0.10,
                drive: 0.06,
                mix: 1.0,
                env_pattern: 8,
                env_pattern_id: Some(pat_gate_rhythm),
                env_amount: 0.0,
                env_decay_ms: 165.0,
            }),
        ),
        fx(
            ids.next(),
            "Music Glue",
            EffectKind::Compressor(CompressorPatch {
                threshold_db: -14.0,
                ratio: 2.2,
                attack_ms: 16.0,
                release_ms: 80.0,
                makeup_db: 0.6,
                mix: 0.60,
            }),
        ),
        fx(
            ids.next(),
            "Music Carve",
            EffectKind::Eq(EqPatch {
                hpf_hz: 160.0,
                low_db: -2.2,
                mid_db: -1.6,
                mid_hz: 420.0,
                mid_q: 1.0,
                high_db: 1.4,
            }),
        ),
    ];

    // Echo Return (2 inserts)
    let mut echo_ret = Channel::new(ch_echo_ret, ChannelKind::Return, "Echo");
    echo_ret.strip.gain = 0.84;
    echo_ret.inserts = vec![
        fx(
            fx_echo_delay,
            "Dotted 8th",
            EffectKind::Delay(DelayPatch {
                mode: DelayMode::PingPong,
                steps: 3.0, // Dotted-8th stereo ping-pong shimmer!
                feedback: 0.36,
                mix: 1.0,
            }),
        ),
        fx(
            ids.next(),
            "Echo Sculpt",
            EffectKind::Eq(EqPatch {
                hpf_hz: 450.0,
                low_db: -4.0,
                mid_db: -1.5,
                mid_hz: 1400.0,
                mid_q: 0.9,
                high_db: -0.4,
            }),
        ),
    ];

    // Hall Return (2 inserts)
    let mut hall_ret = Channel::new(ch_hall_ret, ChannelKind::Return, "Hall");
    hall_ret.strip.gain = 0.96;
    hall_ret.inserts = vec![
        fx(
            fx_sky_hall,
            "Sky Hall",
            EffectKind::Reverb(ReverbPatch {
                size: 0.78,
                decay_seconds: 3.4,
                damping_hz: 5200.0,
                predelay_ms: 24.0,
                width: 0.96,
                mix: 1.0,
            }),
        ),
        fx(
            ids.next(),
            "Hall Sculpt",
            EffectKind::Eq(EqPatch {
                hpf_hz: 380.0,
                low_db: -5.0,
                mid_db: -2.0,
                mid_hz: 800.0,
                mid_q: 0.8,
                high_db: 1.8,
            }),
        ),
    ];

    // ch0: Kick (1 insert: Kick Sculpt EQ, clean headroom into Drums bus)
    let mut kick_ch = Channel::new(ch_kick, ChannelKind::Audio, "Kick");
    kick_ch.output = ChannelRef::Channel(ch_drums_grp);
    kick_ch.strip.gain = 0.88;
    kick_ch.inserts = vec![fx(
        ids.next(),
        "Kick Sculpt",
        EffectKind::Eq(EqPatch {
            hpf_hz: 32.0,
            low_db: 1.8,
            mid_db: -2.2,
            mid_hz: 280.0,
            mid_q: 1.3,
            high_db: 1.8,
        }),
    )];

    // ch1: EDM Kit multi-zone Rack (0 inserts)
    let mut kit_ch = Channel::new(ch_kit, ChannelKind::Audio, "EDM Kit");
    kit_ch.output = ChannelRef::Channel(ch_drums_grp);
    kit_ch.strip.gain = 0.72;
    kit_ch.sends = vec![
        ChannelSend {
            target: ch_hall_ret,
            amount: 0.14,
        },
        ChannelSend {
            target: ch_echo_ret,
            amount: 0.08,
        },
    ];

    // ch2: Tech Perc (0 inserts)
    let mut perc_ch = Channel::new(ch_perc, ChannelKind::Audio, "Perc");
    perc_ch.output = ChannelRef::Channel(ch_drums_grp);
    perc_ch.strip = ChannelStrip {
        gain: 0.80,
        pan: -0.30,
    };
    perc_ch.sends = vec![
        ChannelSend {
            target: ch_echo_ret,
            amount: 0.30,
        },
        ChannelSend {
            target: ch_hall_ret,
            amount: 0.18,
        },
    ];

    // ch3: Sub Drive Bass (2 inserts, direct to Master, ducked independently!)
    let mut bass_ch = Channel::new(ch_bass, ChannelKind::Audio, "Bass");
    bass_ch.output = ChannelRef::Master;
    bass_ch.strip.gain = 0.58;
    bass_ch.inserts = vec![
        fx(
            fx_bass_drive,
            "Bass Drive",
            EffectKind::Drive(DrivePatch {
                mode: DriveMode::Tape,
                drive: 0.38,
                asymmetry: 0.0,
                tone_hz: 2400.0,
                output_db: 0.0,
                mix: 0.42,
            }),
        ),
        fx(
            ids.next(),
            "Bass Sculpt",
            EffectKind::Eq(EqPatch {
                hpf_hz: 32.0,
                low_db: 2.2,
                mid_db: -1.0,
                mid_hz: 340.0,
                mid_q: 1.0,
                high_db: 2.0,
            }),
        ),
    ];

    // ch4: Supersaw Chords (2 inserts)
    let mut chords_ch = Channel::new(ch_chords, ChannelKind::Audio, "Chords");
    chords_ch.output = ChannelRef::Channel(ch_music_grp);
    chords_ch.strip.gain = 0.72; // -2.85 dB — leaves clean pocket for octave Lead
    chords_ch.inserts = vec![
        fx(
            ids.next(),
            "Wide Chorus",
            EffectKind::Chorus(ChorusPatch {
                rate_hz: 0.30,
                depth: 0.24,
                width: 1.0,
                mix: 0.16,
            }),
        ),
        fx(
            ids.next(),
            "Chord Warmth",
            EffectKind::Eq(EqPatch {
                hpf_hz: 210.0,
                low_db: -3.2,
                mid_db: -3.4,
                mid_hz: 2300.0,
                mid_q: 1.0,
                high_db: 2.0,
            }),
        ),
    ];
    chords_ch.sends = vec![ChannelSend {
        target: ch_hall_ret,
        amount: 0.28,
    }];

    // ch5: Anthem Lead (1 insert)
    let mut lead_ch = Channel::new(ch_lead, ChannelKind::Audio, "Lead");
    lead_ch.output = ChannelRef::Channel(ch_music_grp);
    lead_ch.strip.gain = 0.80;
    lead_ch.inserts = vec![fx(
        ids.next(),
        "Lead Presence",
        EffectKind::Eq(EqPatch {
            hpf_hz: 320.0,
            low_db: -3.2,
            mid_db: 0.8,
            mid_hz: 3400.0,
            mid_q: 1.0,
            high_db: 2.4,
        }),
    )];
    lead_ch.sends = vec![
        ChannelSend {
            target: ch_echo_ret,
            amount: 0.30,
        },
        ChannelSend {
            target: ch_hall_ret,
            amount: 0.28,
        },
    ];

    // ch6: Glass Arp (0 inserts, clear left stereo pocket!)
    let mut arp_ch = Channel::new(ch_arp, ChannelKind::Audio, "Arp");
    arp_ch.output = ChannelRef::Channel(ch_music_grp);
    arp_ch.strip = ChannelStrip {
        gain: 0.82,
        pan: -0.36,
    };
    arp_ch.sends = vec![
        ChannelSend {
            target: ch_echo_ret,
            amount: 0.42,
        },
        ChannelSend {
            target: ch_hall_ret,
            amount: 0.28,
        },
    ];

    // ch7: Shimmer Bell (0 inserts, clear right stereo pocket!)
    let mut bell_ch = Channel::new(ch_bell, ChannelKind::Audio, "Bell");
    bell_ch.output = ChannelRef::Channel(ch_music_grp);
    bell_ch.strip = ChannelStrip {
        gain: 0.84,
        pan: 0.38,
    };
    bell_ch.sends = vec![
        ChannelSend {
            target: ch_echo_ret,
            amount: 0.46,
        },
        ChannelSend {
            target: ch_hall_ret,
            amount: 0.44,
        },
    ];

    // ch8: FX Sweep (1 insert, direct to Master, ducked independently on drops!)
    let mut fx_sweep_ch = Channel::new(ch_fx_sweep, ChannelKind::Audio, "FX Sweep");
    fx_sweep_ch.output = ChannelRef::Master;
    fx_sweep_ch.strip.gain = 0.54;
    fx_sweep_ch.inserts = vec![fx(
        ids.next(),
        "Sweep Sculpt",
        EffectKind::Eq(EqPatch {
            hpf_hz: 180.0,
            low_db: -3.0,
            mid_db: -1.2,
            mid_hz: 1600.0,
            mid_q: 0.9,
            high_db: 2.0,
        }),
    )];
    fx_sweep_ch.sends = vec![
        ChannelSend {
            target: ch_hall_ret,
            amount: 0.50,
        },
        ChannelSend {
            target: ch_echo_ret,
            amount: 0.28,
        },
    ];

    // Master Bus (3 inserts: Compressor + EQ + Limiter @ -0.6 dBFS!)
    let mixer = MixerGraph {
        channels: vec![
            kick_ch,
            kit_ch,
            perc_ch,
            bass_ch,
            chords_ch,
            lead_ch,
            arp_ch,
            bell_ch,
            fx_sweep_ch,
            drums_grp,
            music_grp,
            echo_ret,
            hall_ret,
        ],
        master: MasterChannel {
            strip: ChannelStrip {
                gain: 1.0,
                pan: 0.0,
            },
            inserts: vec![
                fx(
                    ids.next(),
                    "Master Glue",
                    EffectKind::Compressor(CompressorPatch {
                        threshold_db: -12.0,
                        ratio: 1.8,
                        attack_ms: 25.0,
                        release_ms: 85.0,
                        makeup_db: 0.6,
                        mix: 0.65,
                    }),
                ),
                fx(
                    ids.next(),
                    "Master Air",
                    EffectKind::Eq(EqPatch {
                        hpf_hz: 28.0,
                        low_db: 0.6,
                        mid_db: -0.4,
                        mid_hz: 600.0,
                        mid_q: 0.8,
                        high_db: 1.0,
                    }),
                ),
                fx(
                    ids.next(),
                    "Master Limiter",
                    EffectKind::Limiter(LimiterPatch {
                        ceiling_db: -0.8,
                        input_drive_db: 2.2,
                        release_ms: 45.0,
                        mix: 1.0,
                    }),
                ),
            ],
        },
    };

    // ─── 2. Sculpted Instruments (8 Synth Racks + 1 MIT Multi-Zone EDM Kit Rack) ───
    let inst_kick = ids.next();
    let inst_kit = ids.next();
    let inst_perc = ids.next();
    let inst_bass = ids.next();
    let inst_chords = ids.next();
    let inst_lead = ids.next();
    let inst_arp = ids.next();
    let inst_bell = ids.next();
    let inst_fx_sweep = ids.next();

    let instruments = vec![
        InstrumentInstance {
            id: inst_kick,
            name: "Club Kick".into(),
            channel: ch_kick,
            rack: kick_rack(),
        },
        InstrumentInstance {
            id: inst_kit,
            name: "EDM Kit".into(),
            channel: ch_kit,
            rack: edm_kit_rack(),
        },
        InstrumentInstance {
            id: inst_perc,
            name: "Tech Perc".into(),
            channel: ch_perc,
            rack: perc_rack(),
        },
        InstrumentInstance {
            id: inst_bass,
            name: "Sub Drive".into(),
            channel: ch_bass,
            rack: bass_rack(),
        },
        InstrumentInstance {
            id: inst_chords,
            name: "Supersaw".into(),
            channel: ch_chords,
            rack: supersaw_rack(),
        },
        InstrumentInstance {
            id: inst_lead,
            name: "Anthem Lead".into(),
            channel: ch_lead,
            rack: lead_rack(),
        },
        InstrumentInstance {
            id: inst_arp,
            name: "Glass Arp".into(),
            channel: ch_arp,
            rack: arp_rack(),
        },
        InstrumentInstance {
            id: inst_bell,
            name: "Shimmer Bell".into(),
            channel: ch_bell,
            rack: bell_rack(),
        },
        InstrumentInstance {
            id: inst_fx_sweep,
            name: "FX Sweep".into(),
            channel: ch_fx_sweep,
            rack: fx_sweep_rack(),
        },
    ];

    // ─── 3. Expressive Patterns (Small Musical Cells → Composed Algebra!) ───
    let mut patterns = Vec::new();

    // Rhythmic gate pattern bound to `Music Filter`'s step envelope (driven by `M6 Gate Pump`)
    patterns.push(Pattern {
        id: pat_gate_rhythm,
        name: "Gate Pulse".into(),
        kind: PatternKind::Rhythm {
            steps: rhythm_from_str("x.x..x..x..x..x."),
        },
    });

    // Master 4-bar harmonic progression: Fm (0) → Dbmaj7 (-2) → Abmaj7 (+2) → Eb (-1)
    let pat_harmony = ids.next();
    patterns.push(Pattern {
        id: pat_harmony,
        name: "Harmony".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Degrees,
            period: 64,
            mode: ValueMode::Hold,
            points: vec![
                ValuePoint {
                    step: 0,
                    value: 0.0,
                },
                ValuePoint {
                    step: 16,
                    value: -2.0,
                },
                ValuePoint {
                    step: 32,
                    value: 2.0,
                },
                ValuePoint {
                    step: 48,
                    value: -1.0,
                },
            ],
        },
    });

    // Single 1-bar sustained chord cell (root + 3rd + 5th + octave in F minor)
    let pat_chord_cell = ids.next();
    let mut chord_pad_steps = vec![Step::default(); 16];
    chord_pad_steps[0] = chord_step(&[1, 3, 5, 8], 0.70, 14.8);
    patterns.push(Pattern {
        id: pat_chord_cell,
        name: "Chord Cell".into(),
        kind: PatternKind::Notes {
            steps: chord_pad_steps,
        },
    });

    // Derived 4-bar lush pad chords: AddDegrees(ChordCell, Harmony)
    let pat_pad_chords = ids.next();
    patterns.push(Pattern {
        id: pat_pad_chords,
        name: "Pad Chords".into(),
        kind: PatternKind::AddDegrees {
            notes: pat_chord_cell,
            values: pat_harmony,
        },
    });

    // 1-bar pumping EDM Drop supersaw voicing (full lush gate between kicks!)
    let pat_stab_cell = ids.next();
    patterns.push(Pattern {
        id: pat_stab_cell,
        name: "Stab Voicing".into(),
        kind: PatternKind::Notes {
            steps: vec![chord_step(&[1, 3, 5, 8], 0.94, 2.9); 16],
        },
    });
    let pat_all_chords = ids.next();
    patterns.push(Pattern {
        id: pat_all_chords,
        name: "Chord Stream".into(),
        kind: PatternKind::AddDegrees {
            notes: pat_stab_cell,
            values: pat_harmony,
        },
    });

    // Iconic modern EDM syncopated stab rhythm (3-3-4-2-4 groove!)
    let pat_stab_rhythm = ids.next();
    patterns.push(Pattern {
        id: pat_stab_rhythm,
        name: "Stab Rhythm".into(),
        kind: PatternKind::Rhythm {
            steps: rhythm_from_str("x..x..x...x.x..."),
        },
    });
    let pat_drop_stabs = ids.next();
    patterns.push(Pattern {
        id: pat_drop_stabs,
        name: "Drop Stabs".into(),
        kind: PatternKind::Mask {
            notes: pat_all_chords,
            rhythm: pat_stab_rhythm,
        },
    });

    // Sparkling Arp derived directly from Pad Chords (UpDown 16ths + Climax 32nds + Velocity Shape!)
    let pat_arp_raw = ids.next();
    patterns.push(Pattern {
        id: pat_arp_raw,
        name: "Sparkle Arp".into(),
        kind: PatternKind::Arp {
            source: pat_pad_chords,
            mode: ArpMode::UpDown,
            rate: 1,
        },
    });
    let pat_arp_vel = ids.next();
    patterns.push(Pattern {
        id: pat_arp_vel,
        name: "Arp Accent".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 8,
            mode: ValueMode::Ramp,
            points: vec![
                ValuePoint {
                    step: 0,
                    value: 0.96,
                },
                ValuePoint {
                    step: 3,
                    value: 0.56,
                },
                ValuePoint {
                    step: 5,
                    value: 0.88,
                },
                ValuePoint {
                    step: 7,
                    value: 0.62,
                },
            ],
        },
    });
    let pat_arp_shaped = ids.next();
    patterns.push(Pattern {
        id: pat_arp_shaped,
        name: "Arp Shimmer".into(),
        kind: PatternKind::Shape {
            source: pat_arp_raw,
            velocity: pat_arp_vel,
            gate: None,
        },
    });
    let pat_arp_double = ids.next();
    patterns.push(Pattern {
        id: pat_arp_double,
        name: "Arp 32nd".into(),
        kind: PatternKind::Arp {
            source: pat_pad_chords,
            mode: ArpMode::UpDown,
            rate: 2,
        },
    });

    // Catchy 4-bar vocal-anthem lead hook (Fm → Db → Ab → Eb)
    // Degrees (F3=1): C5=12, Db5=13, Eb5=14, F5=15, G5=16, Ab4=10, Bb4=11, G4=9
    let pat_hook_main = ids.next();
    let mut hook = vec![Step::default(); 64];
    // Bar 1 (Fm)
    hook[0] = step_note(12, 0.92, 2.6);
    hook[3] = step_note(14, 0.88, 2.5);
    hook[6] = step_note(15, 0.96, 3.2);
    hook[10] = step_note(14, 0.82, 1.7);
    hook[12] = step_note(12, 0.86, 1.8);
    hook[14] = step_note(11, 0.80, 1.6);
    // Bar 2 (Dbmaj7)
    hook[16] = step_note(10, 0.90, 2.6);
    hook[19] = step_note(12, 0.88, 2.5);
    hook[22] = step_note(13, 0.92, 3.2);
    hook[26] = step_note(12, 0.84, 1.7);
    hook[28] = step_note(10, 0.82, 3.0);
    // Bar 3 (Abmaj7 — euphoric high G5 peak!)
    hook[32] = step_note(12, 0.92, 2.6);
    hook[35] = step_note(14, 0.90, 2.5);
    hook[38] = step_note(16, 1.00, 3.5);
    hook[42] = step_note(15, 0.88, 1.7);
    hook[44] = step_note(14, 0.86, 1.8);
    hook[46] = step_note(12, 0.84, 1.6);
    // Bar 4 (Ebadd9 — radiant lift back to C5!)
    hook[48] = step_note(11, 0.90, 2.6);
    hook[51] = step_note(14, 0.92, 2.5);
    hook[54] = step_note(15, 0.96, 2.8);
    hook[58] = step_note(14, 0.86, 1.7);
    hook[60] = step_note(11, 0.82, 1.8);
    hook[62] = step_note(9, 0.78, 1.6);
    patterns.push(Pattern {
        id: pat_hook_main,
        name: "Afterglow Hook".into(),
        kind: PatternKind::Notes { steps: hook },
    });

    // Sparse Hook Tease (Mask first half of each bar for Section 2 anticipation!)
    let pat_tease_mask = ids.next();
    patterns.push(Pattern {
        id: pat_tease_mask,
        name: "Tease Mask".into(),
        kind: PatternKind::Rhythm {
            steps: rhythm_from_str("xxxxxxxx........"),
        },
    });
    let pat_hook_tease = ids.next();
    patterns.push(Pattern {
        id: pat_hook_tease,
        name: "Hook Tease".into(),
        kind: PatternKind::Mask {
            notes: pat_hook_main,
            rhythm: pat_tease_mask,
        },
    });

    // High-octave anthem Stack via Every(2, HookMain, Stack([HookMain, SoftOctave]))
    // Upper octave shaped gently (0.42) so it adds silky shimmer without shrillness.
    let pat_hook_high = ids.next();
    patterns.push(Pattern {
        id: pat_hook_high,
        name: "Hook Octave".into(),
        kind: PatternKind::Transpose {
            source: pat_hook_main,
            semitones: 12,
        },
    });
    let pat_halo_level = ids.next();
    patterns.push(Pattern {
        id: pat_halo_level,
        name: "Octave Halo".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 16,
            mode: ValueMode::Hold,
            points: vec![ValuePoint {
                step: 0,
                value: 0.42,
            }],
        },
    });
    let pat_hook_halo = ids.next();
    patterns.push(Pattern {
        id: pat_hook_halo,
        name: "Hook Halo".into(),
        kind: PatternKind::Shape {
            source: pat_hook_high,
            velocity: pat_halo_level,
            gate: None,
        },
    });
    let pat_hook_stack = ids.next();
    patterns.push(Pattern {
        id: pat_hook_stack,
        name: "Hook Anthem".into(),
        kind: PatternKind::Stack {
            parts: vec![pat_hook_main, pat_hook_halo],
        },
    });
    let pat_hook_climax = ids.next();
    patterns.push(Pattern {
        id: pat_hook_climax,
        name: "Hook Climax".into(),
        kind: PatternKind::Every {
            interval: 2,
            base: pat_hook_main,
            alt: pat_hook_stack,
        },
    });

    // Inline Strudel Mini-Notation shimmering bell counter-melody (sub-step flourishes!)
    let pat_bell_mini = ids.next();
    patterns.push(Pattern {
        id: pat_bell_mini,
        name: "Bell Mini".into(),
        kind: PatternKind::Mini {
            code: "~ c5 ~ [eb5 f5] ~ ~ [g5 f5] eb5 ~ c5 ~ [bb4 c5] eb5*2 ~ [f5 g5] ~".into(),
            steps: 16,
        },
    });

    // Rolling Club Bass derived from Harmony → RangeFold(36..48) so pure sub hits 34.6..51.9 Hz!
    let pat_bass_rhythm = ids.next();
    patterns.push(Pattern {
        id: pat_bass_rhythm,
        name: "Bass Groove".into(),
        kind: PatternKind::Rhythm {
            steps: rhythm_from_str("..xx..xx..xx.xxx"),
        },
    });
    let pat_bass_raw = ids.next();
    patterns.push(Pattern {
        id: pat_bass_raw,
        name: "Bass Degrees".into(),
        kind: PatternKind::NotesFromValues {
            values: pat_harmony,
            rhythm: pat_bass_rhythm,
            degree_offset: 0,
            velocity: 0.90,
            gate: 0.80,
        },
    });
    let pat_bass_folded = ids.next();
    patterns.push(Pattern {
        id: pat_bass_folded,
        name: "Club Bass".into(),
        kind: PatternKind::RangeFold {
            source: pat_bass_raw,
            low_midi: 36,
            high_midi: 48,
        },
    });
    let pat_bass_vel = ids.next();
    patterns.push(Pattern {
        id: pat_bass_vel,
        name: "Bass Bounce".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 4,
            mode: ValueMode::Hold,
            points: vec![
                ValuePoint {
                    step: 0,
                    value: 0.72,
                },
                ValuePoint {
                    step: 2,
                    value: 0.98,
                },
                ValuePoint {
                    step: 3,
                    value: 0.78,
                },
            ],
        },
    });
    let pat_bass_shaped = ids.next();
    patterns.push(Pattern {
        id: pat_bass_shaped,
        name: "Bass Driven".into(),
        kind: PatternKind::Shape {
            source: pat_bass_folded,
            velocity: pat_bass_vel,
            gate: None,
        },
    });

    // Warm Intro/Breakdown gentle 1-bar breathing sub swell
    let pat_sub_rhythm = ids.next();
    patterns.push(Pattern {
        id: pat_sub_rhythm,
        name: "Sub Pulse".into(),
        kind: PatternKind::Rhythm {
            steps: rhythm_from_str("x..............."),
        },
    });
    let pat_sub_raw = ids.next();
    patterns.push(Pattern {
        id: pat_sub_raw,
        name: "Sub Raw".into(),
        kind: PatternKind::NotesFromValues {
            values: pat_harmony,
            rhythm: pat_sub_rhythm,
            degree_offset: 0,
            velocity: 0.56,
            gate: 11.0,
        },
    });
    let pat_sub_warm = ids.next();
    patterns.push(Pattern {
        id: pat_sub_warm,
        name: "Warm Sub".into(),
        kind: PatternKind::RangeFold {
            source: pat_sub_raw,
            low_midi: 36,
            high_midi: 48,
        },
    });

    // Punchy 4-on-floor Club Kick + Build-up Kick (cuts cleanly 2 beats before Drops!)
    let pat_kick_4 = ids.next();
    let mut k4 = vec![Step::default(); 16];
    for s in [0, 4, 8, 12] {
        k4[s] = step_note(1, 0.98, 0.9);
    }
    patterns.push(Pattern {
        id: pat_kick_4,
        name: "Kick 4x4".into(),
        kind: PatternKind::Notes { steps: k4 },
    });

    let pat_kick_half = ids.next();
    let mut kh = vec![Step::default(); 16];
    for s in [0, 4] {
        kh[s] = step_note(1, 0.96, 0.9);
    }
    patterns.push(Pattern {
        id: pat_kick_half,
        name: "Kick Drop Cut".into(),
        kind: PatternKind::Notes { steps: kh },
    });

    // Bar 3 of Build: driving 8th-note kick acceleration before the half-bar drop cut!
    let pat_kick_8ths = ids.next();
    let mut k8 = vec![Step::default(); 16];
    for s in (0..16).step_by(2) {
        let vel = if s % 4 == 0 { 0.96 } else { 0.82 };
        k8[s] = step_note(1, vel, 0.85);
    }
    patterns.push(Pattern {
        id: pat_kick_8ths,
        name: "Kick 8ths Drive".into(),
        kind: PatternKind::Notes { steps: k8 },
    });

    let pat_kick_build_4 = ids.next();
    patterns.push(Pattern {
        id: pat_kick_build_4,
        name: "Kick Build".into(),
        kind: PatternKind::Concat {
            parts: vec![pat_kick_4, pat_kick_4, pat_kick_8ths, pat_kick_half],
        },
    });

    // ─── EDM Kit Rack Patterns (Layered Snare/Clap, Choked Hats, Crash, Ride, Toms, Rim) ───
    // 1. Crisp 16th ghost Closed Hats (Swing 0.14 + Chance 0.88) + rock-solid Offbeat Open Hats (choked by CH!)
    let pat_kit_ch_base = ids.next();
    let mut ch_steps = vec![Step::default(); 16];
    for (s, slot) in ch_steps.iter_mut().enumerate() {
        if s % 4 != 2 {
            let vel = match s % 4 {
                0 => 0.42,
                1 => 0.56,
                _ => 0.68, // Step 3/7/11/15 chokes the offbeat OH cleanly!
            };
            *slot = drum_step(drum_midi::CH, vel, 0.45);
        }
    }
    patterns.push(Pattern {
        id: pat_kit_ch_base,
        name: "Kit Closed Hats".into(),
        kind: PatternKind::Notes { steps: ch_steps },
    });
    let pat_kit_ch_swung = ids.next();
    patterns.push(Pattern {
        id: pat_kit_ch_swung,
        name: "Swung Hats".into(),
        kind: PatternKind::Swing {
            source: pat_kit_ch_base,
            amount: 0.14,
        },
    });
    let pat_kit_ch_live = ids.next();
    patterns.push(Pattern {
        id: pat_kit_ch_live,
        name: "Live Hats".into(),
        kind: PatternKind::Chance {
            source: pat_kit_ch_swung,
            probability: 0.90,
            seed: 42,
        },
    });

    // Driving offbeat EDM Open Hat + crisp choke on following 16th
    let pat_kit_oh = ids.next();
    let mut oh_steps = vec![Step::default(); 16];
    for s in [2, 6, 10, 14] {
        oh_steps[s] = drum_step(drum_midi::OH, 0.86, 1.1);
        oh_steps[s + 1] = drum_step(drum_midi::CH, 0.46, 0.4);
    }
    patterns.push(Pattern {
        id: pat_kit_oh,
        name: "Offbeat Open Hat".into(),
        kind: PatternKind::Notes { steps: oh_steps },
    });

    // Syncopated woody Rimshot bounce (with ZoneParam P-locks on Drop/Climax parts!)
    let pat_kit_rim_raw = ids.next();
    let mut rim_steps = vec![Step::default(); 16];
    rim_steps[3] = drum_step(drum_midi::RS, 0.72, 0.5);
    rim_steps[10] = drum_step(drum_midi::RS, 0.64, 0.5);
    rim_steps[15] = drum_step(drum_midi::RS, 0.78, 0.5);
    patterns.push(Pattern {
        id: pat_kit_rim_raw,
        name: "Rim Bounce".into(),
        kind: PatternKind::Notes { steps: rim_steps },
    });
    let pat_kit_rim = ids.next();
    patterns.push(Pattern {
        id: pat_kit_rim,
        name: "Swung Rim".into(),
        kind: PatternKind::Swing {
            source: pat_kit_rim_raw,
            amount: 0.12,
        },
    });

    // Soft Backbeat Clap (Tease Lift & Outro)
    let pat_kit_clap_soft = ids.next();
    let mut clap_soft = vec![Step::default(); 16];
    clap_soft[4] = drum_step(drum_midi::CP, 0.80, 0.9);
    clap_soft[12] = drum_step(drum_midi::CP, 0.86, 0.9);
    patterns.push(Pattern {
        id: pat_kit_clap_soft,
        name: "Backbeat Clap".into(),
        kind: PatternKind::Notes { steps: clap_soft },
    });

    // Layered Drop Backbeat (Snare + Clap punch on 2 & 4 + subtle snare ghost)
    let pat_kit_backbeat_drop = ids.next();
    let mut drop_bb = vec![Step::default(); 16];
    drop_bb[4] = drum_chord(&[drum_midi::SD, drum_midi::CP], 0.95, 1.0);
    drop_bb[12] = drum_chord(&[drum_midi::SD, drum_midi::CP], 0.98, 1.0);
    drop_bb[15] = drum_step(drum_midi::SD, 0.36, 0.5);
    patterns.push(Pattern {
        id: pat_kit_backbeat_drop,
        name: "Drop Snare Clap".into(),
        kind: PatternKind::Notes { steps: drop_bb },
    });

    // Every-4th-bar Stereo Tom Turnaround Fill (`HT R28 → LT L32 → Snare+Clap`)
    let pat_kit_tom_fill = ids.next();
    let mut tom_fill = vec![Step::default(); 16];
    tom_fill[4] = drum_chord(&[drum_midi::SD, drum_midi::CP], 0.94, 1.0);
    tom_fill[10] = drum_step(drum_midi::HT, 0.82, 0.7);
    tom_fill[12] = drum_chord(&[drum_midi::SD, drum_midi::CP], 0.96, 1.0);
    tom_fill[13] = drum_step(drum_midi::HT, 0.78, 0.6);
    tom_fill[14] = drum_step(drum_midi::LT, 0.88, 0.8);
    tom_fill[15] = drum_step(drum_midi::LT, 0.94, 0.9);
    patterns.push(Pattern {
        id: pat_kit_tom_fill,
        name: "Tom Turnaround".into(),
        kind: PatternKind::Notes { steps: tom_fill },
    });
    let pat_kit_drop_groove = ids.next();
    patterns.push(Pattern {
        id: pat_kit_drop_groove,
        name: "Drop Kit Backbeat".into(),
        kind: PatternKind::Every {
            interval: 4,
            base: pat_kit_backbeat_drop,
            alt: pat_kit_tom_fill,
        },
    });

    // Shimmering 4-bar Crash Impact (downbeat of every 4-bar phrase!) + 1-bar Breakdown Crash
    let pat_kit_crash_4bar = ids.next();
    let mut cr4 = vec![Step::default(); 64];
    cr4[0] = drum_step(drum_midi::CB_OR_CR, 0.94, 4.0);
    patterns.push(Pattern {
        id: pat_kit_crash_4bar,
        name: "Drop Crash".into(),
        kind: PatternKind::Notes { steps: cr4 },
    });

    let pat_kit_crash_once = ids.next();
    let mut cr1 = vec![Step::default(); 16];
    cr1[0] = drum_step(drum_midi::CB_OR_CR, 0.84, 4.0);
    patterns.push(Pattern {
        id: pat_kit_crash_once,
        name: "Release Crash".into(),
        kind: PatternKind::Notes { steps: cr1 },
    });

    // Silvery driving Climax Ride (8th-note push-pull!)
    let pat_kit_ride = ids.next();
    let mut ride_steps = vec![Step::default(); 16];
    for s in (0..16).step_by(2) {
        let vel = if s % 4 == 2 { 0.88 } else { 0.58 };
        ride_steps[s] = drum_step(drum_midi::CY_OR_RD, vel, 1.2);
    }
    patterns.push(Pattern {
        id: pat_kit_ride,
        name: "Climax Ride".into(),
        kind: PatternKind::Notes { steps: ride_steps },
    });

    // Festival 4-bar EDM Snare Build:
    // Bar 1: Quarters (SD+CP) → Bar 2: 8ths Heavy → Bar 3: 16ths Heavy →
    // Bar 4: Beat 1 16ths → Beat 2 32nd Ratchet → Beat 3 Vacuum Breath → Beat 4 Tom+Snare Drop Fill!
    let pat_snare_quarters = ids.next();
    let mut sn_q = vec![Step::default(); 16];
    for (s, vel) in [(0, 0.82), (4, 0.84), (8, 0.88), (12, 0.90)] {
        sn_q[s] = drum_chord(&[drum_midi::SD, drum_midi::CP], vel, 0.85);
    }
    patterns.push(Pattern {
        id: pat_snare_quarters,
        name: "Snare Quarters".into(),
        kind: PatternKind::Notes { steps: sn_q },
    });

    let pat_snare_8_heavy = ids.next();
    let mut sn8 = vec![Step::default(); 16];
    for (s, vel) in [(0, 0.84), (4, 0.86), (8, 0.88), (12, 0.92)] {
        sn8[s] = drum_chord(&[drum_midi::SD, drum_midi::CP], vel, 0.85);
    }
    for (s, vel) in [(2, 0.72), (6, 0.74), (10, 0.76), (14, 0.80)] {
        sn8[s] = drum_step(drum_midi::SD, vel, 0.85);
    }
    patterns.push(Pattern {
        id: pat_snare_8_heavy,
        name: "Snare 8ths Heavy".into(),
        kind: PatternKind::Notes { steps: sn8 },
    });

    let pat_snare_16_heavy = ids.next();
    let mut sn16 = vec![drum_step(drum_midi::SD, 0.78, 0.85); 16];
    for s in [0, 4, 8, 12] {
        sn16[s] = drum_chord(&[drum_midi::SD, drum_midi::CP], 0.94, 0.85);
    }
    patterns.push(Pattern {
        id: pat_snare_16_heavy,
        name: "Snare 16ths Heavy".into(),
        kind: PatternKind::Notes { steps: sn16 },
    });

    // Bar 4 beat 1: accelerating 16ths with SD+CP accent on beat 1.
    let pat_build_q1_16ths = ids.next();
    patterns.push(Pattern {
        id: pat_build_q1_16ths,
        name: "Build Q1 16ths".into(),
        kind: PatternKind::Notes {
            steps: vec![
                drum_chord(&[drum_midi::SD, drum_midi::CP], 0.96, 0.85),
                drum_step(drum_midi::SD, 0.82, 0.85),
                drum_step(drum_midi::SD, 0.86, 0.85),
                drum_step(drum_midi::SD, 0.90, 0.85),
            ],
        },
    });

    // Bar 4 beat 2: 4 steps ratcheted x2 (= blazing 32nd roll!).
    let pat_roll_q2_base = ids.next();
    patterns.push(Pattern {
        id: pat_roll_q2_base,
        name: "Roll Q2 Base".into(),
        kind: PatternKind::Notes {
            steps: vec![
                drum_step(drum_midi::SD, 0.90, 0.85),
                drum_step(drum_midi::SD, 0.93, 0.85),
                drum_step(drum_midi::SD, 0.96, 0.85),
                drum_step(drum_midi::SD, 1.00, 0.85),
            ],
        },
    });
    let pat_snare_ratchet = ids.next();
    patterns.push(Pattern {
        id: pat_snare_ratchet,
        name: "Snare 32nd Roll".into(),
        kind: PatternKind::Ratchet {
            source: pat_roll_q2_base,
            count: 2,
        },
    });

    // Bar 4 beat 3: dramatic 1-beat vacuum breath!
    let pat_build_breath = ids.next();
    patterns.push(Pattern {
        id: pat_build_breath,
        name: "Build Breath".into(),
        kind: PatternKind::Notes {
            steps: vec![Step::default(); 4],
        },
    });

    // Bar 4 beat 4: explosive HT+SD → LT+SD → SD+CP launch fill into the Drop!
    let pat_build_drop_fill = ids.next();
    patterns.push(Pattern {
        id: pat_build_drop_fill,
        name: "Build Drop Fill".into(),
        kind: PatternKind::Notes {
            steps: vec![
                drum_chord(&[drum_midi::HT, drum_midi::SD], 0.92, 0.85),
                Step::default(),
                drum_chord(&[drum_midi::LT, drum_midi::SD], 0.96, 0.85),
                drum_chord(&[drum_midi::SD, drum_midi::CP], 1.00, 0.85),
            ],
        },
    });

    let pat_snare_concat = ids.next();
    patterns.push(Pattern {
        id: pat_snare_concat,
        name: "Build Roll Raw".into(),
        kind: PatternKind::Concat {
            parts: vec![
                pat_snare_quarters,
                pat_snare_8_heavy,
                pat_snare_16_heavy,
                pat_build_q1_16ths,
                pat_snare_ratchet,
                pat_build_breath,
                pat_build_drop_fill,
            ],
        },
    });
    let pat_crescendo = ids.next();
    patterns.push(Pattern {
        id: pat_crescendo,
        name: "Crescendo".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 64,
            mode: ValueMode::Ramp,
            points: value_points(&[
                (0, 0.58),
                (16, 0.70),
                (32, 0.82),
                (48, 0.92),
                (55, 1.00),
                (56, 0.95),
                (60, 0.98),
                (63, 1.00),
            ]),
        },
    });
    let pat_snare_build = ids.next();
    patterns.push(Pattern {
        id: pat_snare_build,
        name: "EDM Snare Build".into(),
        kind: PatternKind::Shape {
            source: pat_snare_concat,
            velocity: pat_crescendo,
            gate: None,
        },
    });

    // Composed Stacked Rack Kit grooves!
    let pat_kit_tease = ids.next();
    patterns.push(Pattern {
        id: pat_kit_tease,
        name: "Kit Tease".into(),
        kind: PatternKind::Stack {
            parts: vec![pat_kit_ch_live, pat_kit_oh, pat_kit_rim],
        },
    });
    let pat_kit_tease_lift = ids.next();
    patterns.push(Pattern {
        id: pat_kit_tease_lift,
        name: "Kit Tease Lift".into(),
        kind: PatternKind::Stack {
            parts: vec![pat_kit_clap_soft, pat_kit_ch_live, pat_kit_oh, pat_kit_rim],
        },
    });
    let pat_kit_drop1 = ids.next();
    patterns.push(Pattern {
        id: pat_kit_drop1,
        name: "Kit Drop 1".into(),
        kind: PatternKind::Stack {
            parts: vec![
                pat_kit_crash_4bar,
                pat_kit_drop_groove,
                pat_kit_ch_live,
                pat_kit_oh,
                pat_kit_rim,
            ],
        },
    });
    let pat_kit_climax = ids.next();
    patterns.push(Pattern {
        id: pat_kit_climax,
        name: "Kit Climax".into(),
        kind: PatternKind::Stack {
            parts: vec![
                pat_kit_crash_4bar,
                pat_kit_drop_groove,
                pat_kit_ch_live,
                pat_kit_oh,
                pat_kit_ride,
                pat_kit_rim,
            ],
        },
    });
    let pat_kit_outro = ids.next();
    patterns.push(Pattern {
        id: pat_kit_outro,
        name: "Kit Outro".into(),
        kind: PatternKind::Stack {
            parts: vec![pat_kit_crash_4bar, pat_kit_clap_soft, pat_kit_ch_live],
        },
    });

    // Per-zone / module P-lock curves for `EDM Kit`!
    // 1. Snare Pitch Rise (`0 → +12 st` octave sweep, snapping cleanly to `0 st` on the breath & drop fill!)
    let pat_sd_pitch_rise = ids.next();
    patterns.push(Pattern {
        id: pat_sd_pitch_rise,
        name: "Snare Pitch Rise".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 64,
            mode: ValueMode::Ramp,
            points: value_points(&[
                (0, ModuleParam::CoarseTune.normalize(0.0)),
                (32, ModuleParam::CoarseTune.normalize(2.4)),
                (48, ModuleParam::CoarseTune.normalize(5.76)),
                (55, ModuleParam::CoarseTune.normalize(12.0)),
                (56, ModuleParam::CoarseTune.normalize(0.0)),
                (63, ModuleParam::CoarseTune.normalize(0.0)),
            ]),
        },
    });
    // 2. Snare Cutoff Lift (`4.2 kHz → 18.5 kHz`) across the 4-bar Build crescendo!
    let pat_sd_cutoff_rise = ids.next();
    patterns.push(Pattern {
        id: pat_sd_cutoff_rise,
        name: "Snare Filter Rise".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 64,
            mode: ValueMode::Ramp,
            points: vec![
                ValuePoint {
                    step: 0,
                    value: ModuleParam::Cutoff.normalize(4_200.0),
                },
                ValuePoint {
                    step: 48,
                    value: ModuleParam::Cutoff.normalize(14_000.0),
                },
                ValuePoint {
                    step: 55,
                    value: ModuleParam::Cutoff.normalize(18_500.0),
                },
                ValuePoint {
                    step: 63,
                    value: ModuleParam::Cutoff.normalize(16_500.0),
                },
            ],
        },
    });
    // 3. Rimshot Stereo Pan Motion (`L38 ↔ R22`) across each bar in Drop 1 & Climax!
    let pat_rim_pan_motion = ids.next();
    patterns.push(Pattern {
        id: pat_rim_pan_motion,
        name: "Rim Pan Motion".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 16,
            mode: ValueMode::Hold,
            points: vec![
                ValuePoint {
                    step: 0,
                    value: ZoneParam::Pan.normalize(-0.38),
                },
                ValuePoint {
                    step: 8,
                    value: ZoneParam::Pan.normalize(0.22),
                },
                ValuePoint {
                    step: 14,
                    value: ZoneParam::Pan.normalize(-0.24),
                },
            ],
        },
    });

    // Afro-tech / Melodic-techno Euclidean(5, 16, 2) Percussion with Seeded pitch contour!
    let pat_perc_euclid = ids.next();
    patterns.push(Pattern {
        id: pat_perc_euclid,
        name: "Euclid 5/16".into(),
        kind: PatternKind::Euclidean {
            pulses: 5,
            steps: 16,
            rotation: 2,
        },
    });
    let pat_perc_pitch = ids.next();
    patterns.push(Pattern {
        id: pat_perc_pitch,
        name: "Perc Pitch".into(),
        kind: PatternKind::SeededValues {
            unit: ValueUnit::Degrees,
            period: 16,
            seed: 707,
            min: 0.0,
            max: 7.0,
        },
    });
    let pat_perc_groove = ids.next();
    patterns.push(Pattern {
        id: pat_perc_groove,
        name: "Euclid Perc".into(),
        kind: PatternKind::NotesFromValues {
            values: pat_perc_pitch,
            rhythm: pat_perc_euclid,
            degree_offset: 7,
            velocity: 0.78,
            gate: 0.65,
        },
    });

    // FX Sweep notes: 8-bar sustained Build Riser + 4-bar Drop/Breakdown Downlifter Impact!
    let pat_fx_riser = ids.next();
    let mut riser_steps = vec![Step::default(); 128];
    riser_steps[0] = step_note(8, 0.90, 64.0);
    riser_steps[64] = step_note(8, 0.92, 55.0);
    patterns.push(Pattern {
        id: pat_fx_riser,
        name: "Build Riser".into(),
        kind: PatternKind::Notes { steps: riser_steps },
    });

    let pat_fx_impact = ids.next();
    let mut impact_steps = vec![Step::default(); 64];
    impact_steps[0] = step_note(1, 0.96, 30.0);
    patterns.push(Pattern {
        id: pat_fx_impact,
        name: "Drop Impact".into(),
        kind: PatternKind::Notes {
            steps: impact_steps,
        },
    });

    // ─── 4. 64-Bar Macro & FX Sweep Automation Curves (1024-Step Emotional Arcs!) ───
    // M1 Filter Lift: drives Supersaw, Anthem Lead, and Glass Arp cutoffs together!
    let pat_macro_filter_lift = ids.next();
    patterns.push(Pattern {
        id: pat_macro_filter_lift,
        name: "Filter Lift Arc".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 1024,
            mode: ValueMode::Ramp,
            points: value_points(&[
                (0, 0.18),    // Intro warm submerged
                (112, 0.32),  //
                (128, 0.28),  // Hook Tease controlled lift
                (240, 0.40),  //
                (256, 0.40),  // Build 1 start
                (368, 0.90),  // Build 1 soaring sweep!
                (376, 0.96),  // Pre-drop peak
                (383, 0.88),  //
                (384, 0.88),  // Drop 1 wide & lush!
                (504, 0.94),  //
                (512, 0.16),  // Breakdown intimate floating hush!
                (624, 0.30),  //
                (640, 0.36),  // Build 2 start
                (752, 0.94),  // Build 2 huge sweep!
                (760, 0.98),  //
                (767, 0.96),  //
                (768, 0.96),  // Climax full bloom (brighter & bigger than Drop 1!)
                (888, 1.00),  //
                (896, 0.56),  // Outro wind-down
                (992, 0.22),  //
                (1023, 0.12), // Sunset close
            ]),
        },
    });

    // M2 Drop Wash: swells Hall + Echo sends & delay feedback into pre-drop breaths, snaps tight on Drops!
    let pat_macro_drop_wash = ids.next();
    patterns.push(Pattern {
        id: pat_macro_drop_wash,
        name: "Drop Wash Arc".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 1024,
            mode: ValueMode::Ramp,
            points: value_points(&[
                (0, 0.32),   // Intro atmospheric sheen
                (120, 0.18), //
                (128, 0.24), // Hook Tease airy pocket
                (256, 0.20), // Build 1 start
                (320, 0.48), //
                (374, 0.94), // Huge wash into breath!
                (382, 0.96), //
                (384, 0.08), // Drop 1 RAZOR-CLEAN dry snap!
                (504, 0.22), //
                (512, 0.60), // Breakdown lush sky hall!
                (636, 0.44), //
                (640, 0.26), // Build 2 start
                (720, 0.58), //
                (758, 0.98), // Stadium wash peak!
                (766, 1.00), //
                (768, 0.08), // Climax impact snap!
                (888, 0.22), //
                (896, 0.44), // Outro dissolve
                (960, 0.68), //
                (1023, 0.78),
            ]),
        },
    });

    // M6 Gate Pump: rhythmic Music Filter trance-gate pulse across Build 2, snaps open at bar 48.5!
    let pat_macro_gate_pump = ids.next();
    patterns.push(Pattern {
        id: pat_macro_gate_pump,
        name: "Gate Pump Arc".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 1024,
            mode: ValueMode::Ramp,
            points: value_points(&[
                (0, 0.0),
                (639, 0.0),
                (640, 0.26), // Build 2 rhythmic filter gate wakes up!
                (728, 0.62), // Hypnotic pumping pulse into Climax!
                (756, 0.42),
                (760, 0.0), // Opens cleanly for the pre-drop breath & fill!
                (768, 0.0), // Wide open transparent Climax & Outro!
                (1023, 0.0),
            ]),
        },
    });

    // FX Sweep 1024-step arcs: filter sweep, +/- pitch rise/fall, and smooth channel-gain envelope!
    let pat_sweep_cutoff_arc = ids.next();
    patterns.push(Pattern {
        id: pat_sweep_cutoff_arc,
        name: "Sweep Cutoff Arc".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 1024,
            mode: ValueMode::Ramp,
            points: value_points(&[
                (0, 0.20),
                (256, 0.20),
                (320, 0.48),
                (374, 0.96),
                (376, 0.88),
                (383, 0.95),
                (384, 0.95),
                (416, 0.16),
                (448, 0.14),
                (511, 0.86),
                (512, 0.86),
                (544, 0.14),
                (576, 0.14),
                (640, 0.22),
                (704, 0.54),
                (758, 0.98),
                (760, 0.90),
                (767, 0.96),
                (768, 0.96),
                (802, 0.16),
                (832, 0.14),
                (1023, 0.14),
            ]),
        },
    });

    let pat_sweep_pitch_arc = ids.next();
    patterns.push(Pattern {
        id: pat_sweep_pitch_arc,
        name: "Sweep Pitch Arc".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 1024,
            mode: ValueMode::Ramp,
            points: value_points(&[
                (0, RackParam::Tune.normalize(0.0)),
                (256, RackParam::Tune.normalize(0.0)),
                (320, RackParam::Tune.normalize(5.76)),
                (375, RackParam::Tune.normalize(24.0)), // +24 st (+2 oct) riser peak!
                (383, RackParam::Tune.normalize(0.0)),
                (384, RackParam::Tune.normalize(0.0)),
                (416, RackParam::Tune.normalize(-12.0)), // -12 st (-1 oct) downlifter fall!
                (448, RackParam::Tune.normalize(-12.0)),
                (511, RackParam::Tune.normalize(0.0)),
                (512, RackParam::Tune.normalize(0.0)),
                (544, RackParam::Tune.normalize(-12.0)),
                (640, RackParam::Tune.normalize(0.0)),
                (704, RackParam::Tune.normalize(7.68)),
                (759, RackParam::Tune.normalize(24.0)),
                (767, RackParam::Tune.normalize(0.0)),
                (768, RackParam::Tune.normalize(0.0)),
                (802, RackParam::Tune.normalize(-12.0)),
                (1023, RackParam::Tune.normalize(-12.0)),
            ]),
        },
    });

    let pat_sweep_gain_arc = ids.next();
    patterns.push(Pattern {
        id: pat_sweep_gain_arc,
        name: "Sweep Gain Arc".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 1024,
            mode: ValueMode::Ramp,
            points: value_points(&[
                (0, 0.00),
                (255, 0.00),
                (256, 0.14),
                (320, 0.38),
                (374, 0.70),
                (376, 0.16),
                (383, 0.00),
                (384, 0.68),
                (390, 0.32),
                (398, 0.12),
                (408, 0.03),
                (416, 0.00),
                (448, 0.00),
                (511, 0.00),
                (512, 0.46),
                (518, 0.22),
                (528, 0.07),
                (540, 0.00),
                (576, 0.00),
                (639, 0.00),
                (640, 0.16),
                (704, 0.42),
                (758, 0.74),
                (760, 0.18),
                (767, 0.00),
                (768, 0.66),
                (774, 0.30),
                (782, 0.10),
                (792, 0.02),
                (800, 0.00),
                (832, 0.00),
                (1023, 0.00),
            ]),
        },
    });

    // ─── 5. Musical Parts (Ready for both Arrangement & Live Launchpad Matrix!) ───
    let part_hook_tease = ids.next();
    let part_hook_main = ids.next();
    let part_hook_climax = ids.next();

    let part_pad = ids.next();
    let part_stabs = ids.next();

    let part_arp_16 = ids.next();
    let part_arp_32 = ids.next();

    let part_bell = ids.next();

    let part_sub = ids.next();
    let part_bass_tease = ids.next();
    let part_bass_drop = ids.next();

    let part_kick_4 = ids.next();
    let part_kick_build = ids.next();

    let part_kit_tease = ids.next();
    let part_kit_tease_lift = ids.next();
    let part_kit_build = ids.next();
    let part_kit_drop1 = ids.next();
    let part_kit_crash = ids.next();
    let part_kit_climax = ids.next();
    let part_kit_outro = ids.next();

    let part_perc = ids.next();

    let part_fx_riser = ids.next();
    let part_fx_impact = ids.next();

    let parts = vec![
        Part {
            id: part_hook_tease,
            name: "Hook Tease".into(),
            pattern: pat_hook_tease,
            instrument: inst_lead,
            transpose: 0,
            velocity_scale: 0.60,
            parameter_links: Vec::new(),
        },
        Part {
            id: part_hook_main,
            name: "Afterglow Lead".into(),
            pattern: pat_hook_main,
            instrument: inst_lead,
            transpose: 0,
            velocity_scale: 0.92,
            parameter_links: Vec::new(),
        },
        Part {
            id: part_hook_climax,
            name: "Climax Lead".into(),
            pattern: pat_hook_climax,
            instrument: inst_lead,
            transpose: 0,
            velocity_scale: 1.0,
            parameter_links: Vec::new(),
        },
        Part {
            id: part_pad,
            name: "Lush Pad".into(),
            pattern: pat_pad_chords,
            instrument: inst_chords,
            transpose: 0,
            velocity_scale: 0.54,
            parameter_links: Vec::new(),
        },
        Part {
            id: part_stabs,
            name: "Drop Supersaw".into(),
            pattern: pat_drop_stabs,
            instrument: inst_chords,
            transpose: 0,
            velocity_scale: 1.0,
            parameter_links: Vec::new(),
        },
        Part {
            id: part_arp_16,
            name: "Arp 16th".into(),
            pattern: pat_arp_shaped,
            instrument: inst_arp,
            transpose: 12,
            velocity_scale: 0.80,
            parameter_links: Vec::new(),
        },
        Part {
            id: part_arp_32,
            name: "Arp 32nd Lift".into(),
            pattern: pat_arp_double,
            instrument: inst_arp,
            transpose: 12,
            velocity_scale: 0.86,
            parameter_links: Vec::new(),
        },
        Part {
            id: part_bell,
            name: "Mini Shimmer".into(),
            pattern: pat_bell_mini,
            instrument: inst_bell,
            transpose: 0,
            velocity_scale: 0.92,
            parameter_links: Vec::new(),
        },
        Part {
            id: part_sub,
            name: "Warm Sub".into(),
            pattern: pat_sub_warm,
            instrument: inst_bass,
            transpose: 0,
            velocity_scale: 0.44,
            parameter_links: Vec::new(),
        },
        Part {
            id: part_bass_tease,
            name: "Tease Bass".into(),
            pattern: pat_bass_shaped,
            instrument: inst_bass,
            transpose: 0,
            velocity_scale: 0.72,
            parameter_links: Vec::new(),
        },
        Part {
            id: part_bass_drop,
            name: "Rolling Bass".into(),
            pattern: pat_bass_shaped,
            instrument: inst_bass,
            transpose: 0,
            velocity_scale: 1.0,
            parameter_links: Vec::new(),
        },
        Part {
            id: part_kick_4,
            name: "Club Kick".into(),
            pattern: pat_kick_4,
            instrument: inst_kick,
            transpose: 0,
            velocity_scale: 1.0,
            parameter_links: Vec::new(),
        },
        Part {
            id: part_kick_build,
            name: "Build Kick".into(),
            pattern: pat_kick_build_4,
            instrument: inst_kick,
            transpose: 0,
            velocity_scale: 0.94,
            parameter_links: Vec::new(),
        },
        Part {
            id: part_kit_tease,
            name: "Kit Tease".into(),
            pattern: pat_kit_tease,
            instrument: inst_kit,
            transpose: 0,
            velocity_scale: 0.78,
            parameter_links: Vec::new(),
        },
        Part {
            id: part_kit_tease_lift,
            name: "Kit Tease Lift".into(),
            pattern: pat_kit_tease_lift,
            instrument: inst_kit,
            transpose: 0,
            velocity_scale: 0.84,
            parameter_links: Vec::new(),
        },
        Part {
            id: part_kit_build,
            name: "Kit Build".into(),
            pattern: pat_snare_build,
            instrument: inst_kit,
            transpose: 0,
            velocity_scale: 0.98,
            // Per-zone module P-locks: SD pitches up 0→+12 st & opens filter during 32nd roll!
            parameter_links: vec![
                ParameterLink {
                    target: InstrumentParam::Module {
                        zone: ZONE_SD,
                        module: KIT_SAMPLER_MOD,
                        param: ModuleParam::CoarseTune,
                    },
                    pattern: pat_sd_pitch_rise,
                },
                ParameterLink {
                    target: InstrumentParam::Module {
                        zone: ZONE_SD,
                        module: KIT_FILTER_MOD,
                        param: ModuleParam::Cutoff,
                    },
                    pattern: pat_sd_cutoff_rise,
                },
            ],
        },
        Part {
            id: part_kit_drop1,
            name: "Kit Drop 1".into(),
            pattern: pat_kit_drop1,
            instrument: inst_kit,
            transpose: 0,
            velocity_scale: 0.98,
            parameter_links: vec![ParameterLink {
                target: InstrumentParam::Zone {
                    zone: ZONE_RS,
                    param: ZoneParam::Pan,
                },
                pattern: pat_rim_pan_motion,
            }],
        },
        Part {
            id: part_kit_crash,
            name: "Kit Crash".into(),
            pattern: pat_kit_crash_once,
            instrument: inst_kit,
            transpose: 0,
            velocity_scale: 0.88,
            parameter_links: Vec::new(),
        },
        Part {
            id: part_kit_climax,
            name: "Kit Climax".into(),
            pattern: pat_kit_climax,
            instrument: inst_kit,
            transpose: 0,
            velocity_scale: 1.0,
            parameter_links: vec![ParameterLink {
                target: InstrumentParam::Zone {
                    zone: ZONE_RS,
                    param: ZoneParam::Pan,
                },
                pattern: pat_rim_pan_motion,
            }],
        },
        Part {
            id: part_kit_outro,
            name: "Kit Outro".into(),
            pattern: pat_kit_outro,
            instrument: inst_kit,
            transpose: 0,
            velocity_scale: 0.84,
            parameter_links: Vec::new(),
        },
        Part {
            id: part_perc,
            name: "Euclid Perc".into(),
            pattern: pat_perc_groove,
            instrument: inst_perc,
            transpose: 0,
            velocity_scale: 0.94,
            parameter_links: Vec::new(),
        },
        Part {
            id: part_fx_riser,
            name: "Build Riser".into(),
            pattern: pat_fx_riser,
            instrument: inst_fx_sweep,
            transpose: 0,
            velocity_scale: 1.0,
            parameter_links: Vec::new(),
        },
        Part {
            id: part_fx_impact,
            name: "Drop Impact".into(),
            pattern: pat_fx_impact,
            instrument: inst_fx_sweep,
            transpose: 0,
            velocity_scale: 1.0,
            parameter_links: Vec::new(),
        },
    ];

    // ─── 6. 64-Bar Arrangement (8 Iconic Sections × 8 Bars Each, 9 Lanes) ───
    let place_steps = |part: Id, start_step: u32, length_steps: u32, phase: PhaseMode| Placement {
        id: ids.next(),
        part,
        start_step,
        length_steps,
        phase,
        offset_steps: 0,
        exceptions: Vec::new(),
    };
    let place = |part: Id, start_bar: u32, bars: u32, phase: PhaseMode| {
        place_steps(part, start_bar * 16, bars * 16, phase)
    };

    let lp = |part: Id| LanePart {
        part,
        phase: PhaseMode::Song,
        offset_steps: 0,
    };
    let lp_restart = |part: Id| LanePart {
        part,
        phase: PhaseMode::Restart,
        offset_steps: 0,
    };

    let lanes = vec![
        Lane {
            id: ids.next(),
            name: "Lead".into(),
            parts: vec![
                lp(part_hook_tease),
                lp(part_hook_main),
                lp(part_hook_climax),
            ],
            placements: vec![
                place(part_hook_tease, 8, 8, PhaseMode::Song),
                place_steps(part_hook_tease, 256, 120, PhaseMode::Song), // extends to bar 24.5 before breath & fill!
                place(part_hook_main, 24, 8, PhaseMode::Song),           // Drop 1 full anthem!
                place_steps(part_hook_tease, 640, 120, PhaseMode::Song), // Build 2 tease to bar 48.5!
                place(part_hook_climax, 48, 8, PhaseMode::Song), // Climax Drop 2 stacked anthem!
                place(part_hook_tease, 56, 4, PhaseMode::Song),  // Outro echo
            ],
        },
        Lane {
            id: ids.next(),
            name: "Chords".into(),
            parts: vec![lp(part_pad), lp(part_stabs)],
            placements: vec![
                place_steps(part_pad, 0, 376, PhaseMode::Song), // Intro + Tease + Build 1 to bar 24.5
                place(part_stabs, 24, 8, PhaseMode::Song),      // Drop 1 wide pumping supersaws!
                place_steps(part_pad, 512, 248, PhaseMode::Song), // Breakdown + Build 2 to bar 48.5
                place(part_stabs, 48, 8, PhaseMode::Song),      // Climax Drop 2 stabs!
                place(part_pad, 56, 7, PhaseMode::Song), // Outro sunset pad (bar 64 = clean reverb tail!)
            ],
        },
        Lane {
            id: ids.next(),
            name: "Arp".into(),
            parts: vec![lp(part_arp_16), lp(part_arp_32)],
            placements: vec![
                place_steps(part_arp_16, 64, 312, PhaseMode::Song), // enters mid-Intro, cuts at bar 24.5 breath!
                place(part_arp_16, 24, 16, PhaseMode::Song),        // Drop 1 + Breakdown
                place_steps(part_arp_32, 640, 120, PhaseMode::Song), // Build 2 rising 32nd shimmer to bar 48.5!
                place(part_arp_32, 48, 8, PhaseMode::Song),          // Climax 32nd euphoria!
                place(part_arp_16, 56, 6, PhaseMode::Song), // Outro wind-down into delay tail!
            ],
        },
        Lane {
            id: ids.next(),
            name: "Bell".into(),
            parts: vec![lp(part_bell)],
            placements: vec![
                place(part_bell, 32, 8, PhaseMode::Song), // Breakdown inline Mini sparkle
                place(part_bell, 48, 8, PhaseMode::Song), // Climax Drop 2 counter-bells!
                place(part_bell, 56, 4, PhaseMode::Song), // Outro shimmer
            ],
        },
        Lane {
            id: ids.next(),
            name: "Bass".into(),
            parts: vec![lp(part_sub), lp(part_bass_tease), lp(part_bass_drop)],
            placements: vec![
                place(part_sub, 4, 4, PhaseMode::Song),
                place(part_bass_tease, 8, 8, PhaseMode::Song), // Hook Tease restrained rolling bass (leaves headroom for Drop 1!)
                place(part_sub, 16, 4, PhaseMode::Song), // Pulls sub out for last 4 bars of Build 1!
                place(part_bass_drop, 24, 8, PhaseMode::Song), // Drop 1 bass SLAMS!
                place(part_sub, 36, 4, PhaseMode::Song), // Second half of Breakdown: warm sub swells under floating Bell+Pad!
                place(part_sub, 40, 4, PhaseMode::Song), // Early Build 2 warm anchor (pulls out for 4-bar snare roll!)
                place(part_bass_drop, 48, 8, PhaseMode::Song), // Climax Drop 2 bass!
                place(part_sub, 56, 5, PhaseMode::Song), // Outro warm tail
            ],
        },
        Lane {
            id: ids.next(),
            name: "Kick".into(),
            parts: vec![lp(part_kick_4), lp(part_kick_build)],
            placements: vec![
                place(part_kick_4, 8, 8, PhaseMode::Song), // Tease groove
                place(part_kick_build, 20, 4, PhaseMode::Restart), // Build 1 -> cuts 2 beats before drop!
                place(part_kick_4, 24, 8, PhaseMode::Song),        // Drop 1!
                place(part_kick_build, 44, 4, PhaseMode::Restart), // Build 2 -> pre-drop breath!
                place(part_kick_4, 48, 8, PhaseMode::Song),        // Climax Drop 2!
                place(part_kick_4, 56, 4, PhaseMode::Song),        // Outro fade
            ],
        },
        Lane {
            id: ids.next(),
            name: "EDM Kit".into(),
            parts: vec![
                lp(part_kit_tease),
                lp(part_kit_tease_lift),
                lp(part_kit_build),
                lp(part_kit_drop1),
                lp(part_kit_climax),
                lp(part_kit_outro),
            ],
            placements: vec![
                place(part_kit_tease, 8, 4, PhaseMode::Song), // Tease hats + rim
                place(part_kit_tease_lift, 12, 4, PhaseMode::Song), // Tease backbeat clap lift
                place(part_kit_tease, 16, 4, PhaseMode::Song), // Early Build 1 groove
                place(part_kit_build, 20, 4, PhaseMode::Restart), // Build 1 pitching 32nd snare roll!
                place(part_kit_drop1, 24, 8, PhaseMode::Restart), // Drop 1 Crash + Snare/Clap + Hats + Toms!
                place(part_kit_crash, 32, 1, PhaseMode::Restart), // Breakdown release Crash!
                place(part_kit_tease, 40, 4, PhaseMode::Song),    // Early Build 2 groove
                place(part_kit_build, 44, 4, PhaseMode::Restart), // Build 2 massive roll!
                place(part_kit_climax, 48, 8, PhaseMode::Restart), // Climax Crash + Ride + Snare/Clap + Toms!
                place(part_kit_outro, 56, 4, PhaseMode::Restart),  // Outro Crash + groove wind-down
            ],
        },
        Lane {
            id: ids.next(),
            name: "Perc".into(),
            parts: vec![lp(part_perc)],
            placements: vec![
                place(part_perc, 28, 4, PhaseMode::Song), // Drop 1 second-half energy lift!
                place(part_perc, 48, 8, PhaseMode::Song), // Climax Drop 2 full Euclidean drive!
            ],
        },
        Lane {
            id: ids.next(),
            name: "FX Sweep".into(),
            parts: vec![lp_restart(part_fx_riser), lp_restart(part_fx_impact)],
            placements: vec![
                place_steps(part_fx_riser, 256, 120, PhaseMode::Restart), // Build 1 riser (bars 17–24.5)
                place(part_fx_impact, 24, 4, PhaseMode::Restart), // Drop 1 downlifter impact!
                place(part_fx_impact, 32, 4, PhaseMode::Restart), // Breakdown release downlifter
                place_steps(part_fx_riser, 640, 120, PhaseMode::Restart), // Build 2 riser (bars 41–48.5)
                place(part_fx_impact, 48, 4, PhaseMode::Restart), // Climax downlifter impact!
            ],
        },
    ];

    // ─── 7. Sections, 6 Project Macros (M1..M6), Modulations & Sidechain Ducks ───
    let sections = vec![
        Section {
            id: ids.next(),
            name: "Intro".into(),
            start_step: 0,
            length_steps: 128,
        },
        Section {
            id: ids.next(),
            name: "Hook Tease".into(),
            start_step: 128,
            length_steps: 128,
        },
        Section {
            id: ids.next(),
            name: "Build 1".into(),
            start_step: 256,
            length_steps: 128,
        },
        Section {
            id: ids.next(),
            name: "Drop 1".into(),
            start_step: 384,
            length_steps: 128,
        },
        Section {
            id: ids.next(),
            name: "Breakdown".into(),
            start_step: 512,
            length_steps: 128,
        },
        Section {
            id: ids.next(),
            name: "Build 2".into(),
            start_step: 640,
            length_steps: 128,
        },
        Section {
            id: ids.next(),
            name: "Climax".into(),
            start_step: 768,
            length_steps: 128,
        },
        Section {
            id: ids.next(),
            name: "Outro".into(),
            start_step: 896,
            length_steps: 128,
        },
    ];

    let macros = vec![
        // M1: Filter Lift (AUTOMATED across full 64-bar arc!)
        MacroControl {
            index: 0,
            name: "Filter Lift".into(),
            value: 0.60,
            bindings: vec![
                MacroBinding::new(
                    ParamAddress::Module {
                        instrument: inst_chords,
                        zone: 1,
                        module: SUPERSAW_FILTER_MOD,
                        param: ModuleParam::Cutoff,
                    },
                    ModuleParam::Cutoff.normalize(520.0),
                    0.90, // ~10.0 kHz wide-open Supersaw bloom
                ),
                MacroBinding::new(
                    ParamAddress::Module {
                        instrument: inst_lead,
                        zone: 1,
                        module: LEAD_FILTER_MOD,
                        param: ModuleParam::Cutoff,
                    },
                    ModuleParam::Cutoff.normalize(950.0),
                    0.89, // ~9.35 kHz soaring octave Lead bloom
                ),
                MacroBinding::new(
                    ParamAddress::Module {
                        instrument: inst_arp,
                        zone: 1,
                        module: ARP_FILTER_MOD,
                        param: ModuleParam::Cutoff,
                    },
                    ModuleParam::Cutoff.normalize(850.0),
                    ModuleParam::Cutoff.normalize(8_400.0),
                ),
                MacroBinding::new(
                    ParamAddress::Module {
                        instrument: inst_chords,
                        zone: 1,
                        module: SUPERSAW_FILTER_MOD,
                        param: ModuleParam::Resonance,
                    },
                    ModuleParam::Resonance.normalize(0.12),
                    ModuleParam::Resonance.normalize(0.24),
                ),
            ],
        },
        // M2: Drop Wash (AUTOMATED transition send throws into Hall + Echo!)
        MacroControl {
            index: 1,
            name: "Drop Wash".into(),
            value: 0.25,
            bindings: vec![
                MacroBinding::new(
                    ParamAddress::Send {
                        channel: ChannelRef::Channel(ch_chords),
                        target: ch_hall_ret,
                    },
                    0.22,
                    0.72,
                ),
                MacroBinding::new(
                    ParamAddress::Send {
                        channel: ChannelRef::Channel(ch_lead),
                        target: ch_hall_ret,
                    },
                    0.28,
                    0.68,
                ),
                MacroBinding::new(
                    ParamAddress::Send {
                        channel: ChannelRef::Channel(ch_lead),
                        target: ch_echo_ret,
                    },
                    0.32,
                    0.64,
                ),
                MacroBinding::new(
                    ParamAddress::Send {
                        channel: ChannelRef::Channel(ch_kit),
                        target: ch_hall_ret,
                    },
                    0.12,
                    0.44,
                ),
                MacroBinding::new(
                    ParamAddress::Effect {
                        device: fx_echo_delay,
                        param: EffectParam::Feedback,
                    },
                    EffectParam::Feedback.normalize(0.32),
                    EffectParam::Feedback.normalize(0.60),
                ),
            ],
        },
        // M3: Kit Crisp (playable live — shapes EDM Kit filter, hat zones & Drum Punch air!)
        MacroControl {
            index: 2,
            name: "Kit Crisp".into(),
            value: 0.72,
            bindings: vec![
                MacroBinding::new(
                    ParamAddress::Rack {
                        instrument: inst_kit,
                        param: RackParam::Cutoff,
                    },
                    RackParam::Cutoff.normalize(2_200.0),
                    RackParam::Cutoff.normalize(20_000.0),
                ),
                MacroBinding::new(
                    ParamAddress::Module {
                        instrument: inst_kit,
                        zone: ZONE_CH,
                        module: KIT_FILTER_MOD,
                        param: ModuleParam::Cutoff,
                    },
                    ModuleParam::Cutoff.normalize(4_200.0),
                    ModuleParam::Cutoff.normalize(20_000.0),
                ),
                MacroBinding::new(
                    ParamAddress::Module {
                        instrument: inst_kit,
                        zone: ZONE_OH,
                        module: KIT_FILTER_MOD,
                        param: ModuleParam::Cutoff,
                    },
                    ModuleParam::Cutoff.normalize(5_000.0),
                    ModuleParam::Cutoff.normalize(20_000.0),
                ),
                MacroBinding::new(
                    ParamAddress::Effect {
                        device: fx_drum_punch,
                        param: EffectParam::High,
                    },
                    EffectParam::High.normalize(-3.0),
                    EffectParam::High.normalize(3.5),
                ),
            ],
        },
        // M4: Bass Grit (playable live — sweeps Sub Drive cutoff, synth drive & tape saturation!)
        MacroControl {
            index: 3,
            name: "Bass Grit".into(),
            value: 0.45,
            bindings: vec![
                MacroBinding::new(
                    ParamAddress::Module {
                        instrument: inst_bass,
                        zone: 1,
                        module: BASS_FILTER_MOD,
                        param: ModuleParam::Cutoff,
                    },
                    ModuleParam::Cutoff.normalize(280.0),
                    ModuleParam::Cutoff.normalize(2_400.0),
                ),
                MacroBinding::new(
                    ParamAddress::Module {
                        instrument: inst_bass,
                        zone: 1,
                        module: BASS_FILTER_MOD,
                        param: ModuleParam::Drive,
                    },
                    ModuleParam::Drive.normalize(0.10),
                    ModuleParam::Drive.normalize(0.65),
                ),
                MacroBinding::new(
                    ParamAddress::Effect {
                        device: fx_bass_drive,
                        param: EffectParam::Drive,
                    },
                    EffectParam::Drive.normalize(0.15),
                    EffectParam::Drive.normalize(0.78),
                ),
                MacroBinding::new(
                    ParamAddress::Effect {
                        device: fx_bass_drive,
                        param: EffectParam::Mix,
                    },
                    EffectParam::Mix.normalize(0.20),
                    EffectParam::Mix.normalize(0.75),
                ),
            ],
        },
        // M5: Space Air (playable live — expands Sky Hall cathedral + crystalline Bell FM!)
        MacroControl {
            index: 4,
            name: "Space Air".into(),
            value: 0.44,
            bindings: vec![
                MacroBinding::new(
                    ParamAddress::Effect {
                        device: fx_sky_hall,
                        param: EffectParam::Decay,
                    },
                    EffectParam::Decay.normalize(1.8),
                    EffectParam::Decay.normalize(5.6),
                ),
                MacroBinding::new(
                    ParamAddress::Effect {
                        device: fx_sky_hall,
                        param: EffectParam::Damping,
                    },
                    EffectParam::Damping.normalize(2_400.0),
                    EffectParam::Damping.normalize(11_000.0),
                ),
                MacroBinding::new(
                    ParamAddress::Effect {
                        device: fx_sky_hall,
                        param: EffectParam::Size,
                    },
                    EffectParam::Size.normalize(0.54),
                    EffectParam::Size.normalize(0.96),
                ),
                MacroBinding::new(
                    ParamAddress::Cable {
                        instrument: inst_bell,
                        zone: 1,
                        cable: BELL_FM_CABLE,
                    },
                    normalize_cable_amount(ModuleKindTag::Vco, InPort::FmIn, 0.08),
                    normalize_cable_amount(ModuleKindTag::Vco, InPort::FmIn, 0.45),
                ),
            ],
        },
        // M6: Gate Pump (AUTOMATED on Build 2 + crankable live — rhythmic Music Filter gate!)
        MacroControl {
            index: 5,
            name: "Gate Pump".into(),
            value: 0.0,
            bindings: vec![
                // Inverted cutoff sweep: 18 kHz transparent at 0% → 1.8 kHz pulsing floor at 100%!
                MacroBinding::new(
                    ParamAddress::Effect {
                        device: fx_music_filter,
                        param: EffectParam::Cutoff,
                    },
                    EffectParam::Cutoff.normalize(18_000.0),
                    EffectParam::Cutoff.normalize(1_800.0),
                ),
                MacroBinding::new(
                    ParamAddress::Effect {
                        device: fx_music_filter,
                        param: EffectParam::EnvAmount,
                    },
                    EffectParam::EnvAmount.normalize(0.0),
                    EffectParam::EnvAmount.normalize(3.0),
                ),
                MacroBinding::new(
                    ParamAddress::Effect {
                        device: fx_music_filter,
                        param: EffectParam::Resonance,
                    },
                    EffectParam::Resonance.normalize(0.10),
                    EffectParam::Resonance.normalize(0.42),
                ),
            ],
        },
    ];

    let modulations = vec![
        Modulation {
            id: ids.next(),
            name: "M1 Filter Lift".into(),
            target: ModulationTarget::Macro { index: 0 },
            pattern: pat_macro_filter_lift,
        },
        Modulation {
            id: ids.next(),
            name: "M2 Drop Wash".into(),
            target: ModulationTarget::Macro { index: 1 },
            pattern: pat_macro_drop_wash,
        },
        Modulation {
            id: ids.next(),
            name: "M6 Gate Pump".into(),
            target: ModulationTarget::Macro { index: 5 },
            pattern: pat_macro_gate_pump,
        },
        Modulation {
            id: ids.next(),
            name: "FX Sweep Cutoff".into(),
            target: ModulationTarget::Instrument {
                instrument: inst_fx_sweep,
                parameter: InstrumentParam::Module {
                    zone: 1,
                    module: FX_SWEEP_FILTER_MOD,
                    param: ModuleParam::Cutoff,
                },
            },
            pattern: pat_sweep_cutoff_arc,
        },
        Modulation {
            id: ids.next(),
            name: "FX Sweep Pitch".into(),
            target: ModulationTarget::Instrument {
                instrument: inst_fx_sweep,
                parameter: InstrumentParam::Rack {
                    param: RackParam::Tune,
                },
            },
            pattern: pat_sweep_pitch_arc,
        },
        Modulation {
            id: ids.next(),
            name: "FX Sweep Gain".into(),
            target: ModulationTarget::ChannelGain {
                channel: ChannelRef::Channel(ch_fx_sweep),
            },
            pattern: pat_sweep_gain_arc,
        },
    ];

    let ducks = vec![
        Duck {
            id: ids.next(),
            name: "Kick -> Music".into(),
            source_part: part_kick_4,
            target: ChannelRef::Channel(ch_music_grp),
            depth: 0.80,
            recovery_ms: 135.0,
        },
        Duck {
            id: ids.next(),
            name: "Kick -> Bass".into(),
            source_part: part_kick_4,
            target: ChannelRef::Channel(ch_bass),
            depth: 0.82,
            recovery_ms: 84.0,
        },
        Duck {
            id: ids.next(),
            name: "Kick -> Hall".into(),
            source_part: part_kick_4,
            target: ChannelRef::Channel(ch_hall_ret),
            depth: 0.86,
            recovery_ms: 175.0,
        },
        Duck {
            id: ids.next(),
            name: "Build Kick -> Music".into(),
            source_part: part_kick_build,
            target: ChannelRef::Channel(ch_music_grp),
            depth: 0.50,
            recovery_ms: 105.0,
        },
        Duck {
            id: ids.next(),
            name: "Kick -> FX Sweep".into(),
            source_part: part_kick_4,
            target: ChannelRef::Channel(ch_fx_sweep),
            depth: 0.72,
            recovery_ms: 110.0,
        },
    ];

    let project = Project {
        version: VERSION,
        title: "Afterglow".into(),
        bpm: 128.0,
        root_midi: ROOT_MIDI,
        scale: F_MINOR.to_vec(),
        bars: 64,
        patterns,
        instruments,
        parts,
        lanes,
        sections,
        mixer,
        ducks,
        modulations,
        macros,
        timing: LiveTiming::default(),
        end: EndRule::Loop,
    };
    project.validate()?;
    Ok(project)
}

fn main() -> Result<()> {
    let project = build_afterglow()?;
    let out_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/demo/project.json");
    let mut json = Vec::new();
    write_document_json(&mut json, &project, &LiveFlags::default())?;
    std::fs::write(&out_path, &json)?;
    let fx_count: usize = project.mixer.master.inserts.len()
        + project
            .mixer
            .channels
            .iter()
            .map(|c| c.inserts.len())
            .sum::<usize>();
    println!(
        "Wrote {} ({} bytes · {} patterns · {} parts · {} instruments · {} lanes · {} macros · {} FX)",
        out_path.display(),
        json.len(),
        project.patterns.len(),
        project.parts.len(),
        project.instruments.len(),
        project.lanes.len(),
        project.macros.len(),
        fx_count,
    );

    if std::env::args().any(|a| a == "--analyze") {
        let score = project.compile()?;
        let mut library = SampleLibrary::from_roots(&[], Some(&out_path));
        let loaded = load_project_samples(&score, &mut library);
        loaded.ensure_ready()?;
        let sr = 48_000u32;
        let (report, pcm) = render_schedule_pcm(&[(0, &score)], 0, 1024, loaded.bank, sr)?;
        let frames = pcm.len();
        let mut peak = 0.0_f64;
        let mut clipped = 0usize;
        let mut mid_sq = 0.0_f64;
        let mut side_sq = 0.0_f64;
        for &[li, ri] in &pcm {
            let l = f64::from(li) / 32768.0;
            let r = f64::from(ri) / 32768.0;
            peak = peak.max(l.abs()).max(r.abs());
            if li.abs() >= 32766 || ri.abs() >= 32766 {
                clipped += 1;
            }
            let m = 0.5 * (l + r);
            let s = 0.5 * (l - r);
            mid_sq += m * m;
            side_sq += s * s;
        }
        let rms = ((mid_sq + side_sq) / frames.max(1) as f64).sqrt();
        let width = (side_sq / mid_sq.max(1e-12)).sqrt();
        let tail_peak = report.tail.peak[0].max(report.tail.peak[1]);
        println!(
            "\nAudio analysis (48 kHz, {:.1}s): peak={:.2} dBFS  RMS={:.2} dBFS  tail={:.2} dBFS  width={:.2}  clipped={}  dc={:.6}",
            frames as f64 / f64::from(sr),
            lin_to_db_f64(peak),
            lin_to_db_f64(rms),
            lin_to_db_f64(tail_peak),
            width,
            clipped,
            report.output.dc[0].abs().max(report.output.dc[1].abs()),
        );
        let frames_per_sec = frames / 8;
        for (i, sec) in project.sections.iter().enumerate() {
            let start = i * frames_per_sec;
            let end = ((i + 1) * frames_per_sec).min(frames);
            let mut s_peak = 0.0_f64;
            let mut s_sq = 0.0_f64;
            for &[li, ri] in &pcm[start..end] {
                let l = f64::from(li) / 32768.0;
                let r = f64::from(ri) / 32768.0;
                s_peak = s_peak.max(l.abs()).max(r.abs());
                s_sq += 0.5 * (l * l + r * r);
            }
            let s_rms = (s_sq / (end - start).max(1) as f64).sqrt();
            println!(
                "  S{} {:<12}  peak={:+6.2} dBFS   RMS={:+6.2} dBFS",
                i + 1,
                sec.name,
                lin_to_db_f64(s_peak),
                lin_to_db_f64(s_rms)
            );
        }
        println!("\nChannel peaks (pre → post):");
        for ch in &report.channels {
            let pre = f64::from(ch.input_peak_hold[0].max(ch.input_peak_hold[1]));
            let post = f64::from(ch.output_peak_hold[0].max(ch.output_peak_hold[1]));
            println!(
                "  {:<12}  pre={:+6.2} dBFS ({:.2})  post={:+6.2} dBFS ({:.2}){}{}",
                ch.name,
                lin_to_db_f64(pre),
                pre,
                lin_to_db_f64(post),
                post,
                if ch.hot { "  HOT" } else { "" },
                if ch.clipped { "  !CLIP" } else { "" },
            );
        }
        if let Some(wav_arg) = std::env::args().skip_while(|a| a != "--wav").nth(1) {
            let spec = hound::WavSpec {
                channels: 2,
                sample_rate: sr,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            };
            let mut writer = hound::WavWriter::create(&wav_arg, spec)?;
            for &[l, r] in &pcm {
                writer.write_sample(l)?;
                writer.write_sample(r)?;
            }
            writer.finalize()?;
            println!("Rendered WAV -> {wav_arg}");
        }
    }
    Ok(())
}
