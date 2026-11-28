//! Reproducible builder for Cadence's Acid Electro / Hardware Groovebox flagship showcase:
//! **"Iron Circuit"**.
//!
//! 48 bars · 132 BPM · A natural minor (~1:27 full hardware-groovebox journey).
//!
//! Showcases Cadence capabilities musically:
//! - Unified modular `RackPatch` instruments together: dual drum kits (`TR-808` + `TR-909` with
//!   per-zone/module P-locks & continuous modulations), standalone sampler (`Electro Stab`
//!   using `../../examples/demo/samples/brass-pad.wav`), and 3 sculpted modular synth Racks
//!   (`Acid 303` with native `DiodeLadder` velocity accent, `Fold Bass`, `Gate Pad`).
//! - Nested `Group -> Group` DAG routing:
//!   `808 Kit` + `909 Kit` -> `Percussion` Group -> `Rhythm` Group (joined by `Fold Bass`) -> `Master`,
//!   plus `Acid 303` + `Electro Stab` + `Gate Pad` -> `Synths` Group (sidechain `Duck` off `808 Kick`).
//! - All 8 native FX & modes: `Drive(ShockPort)` on `Acid 303` & `Fold Bass`,
//!   `Drive(AtomicJet)` on `Percussion` & `Master`, rhythmic `Filter` (`FilterEnv` custom pattern) on
//!   `Gate Pad`, `Delay(Stereo)` 3/16 (`Dub Tape`) + `Delay(PingPong)` 6/16 (`PingPong`) + `Reverb`
//!   (`Room`), `Chorus`, `Compressor`, `Eq`, and `Master` `Limiter` (`-0.8 dBFS`).
//! - All 8 assignable Project Macros (`M1..M8`) populated (with inverted bindings on `M6` & `M8`)
//!   plus automated `ModulationTarget::Send` dub throws on phrasing turnarounds.
//! - Retained pattern algebra: `AddDegrees`, `Mask`, `Euclidean`, `Ratchet`, `Swing`, `Mini`,
//!   `SeededValues`, `RangeFold`, `Every`, `Stack`, `Shape`, and `Ramp` modulation curves.
//!
//! Run from the repository root:
//! ```bash
//! cargo run --release --example build-iron-circuit
//! ```

use anyhow::{ensure, Result};
use cadence_core::assets::load_project_samples;
use cadence_core::dsp::lin_to_db_f64;
use cadence_core::material::{
    write_document_json, Id, InstrumentInstance, Lane, LanePart, LiveFlags, LiveTiming,
    MacroBinding, MacroControl, MixerGraph, Modulation, ModulationTarget, ParameterLink, Part,
    Pattern, PatternKind, PhaseMode, Placement, Project, Section, ValuePoint, ValueUnit, VERSION,
};
use cadence_core::mixer::{
    Channel, ChannelKind, ChannelParam, ChannelRef, ChannelSend, ChannelStrip, ChorusPatch,
    CompressorPatch, DelayMode, DelayPatch, DriveMode, DrivePatch, Duck, EffectDevice, EffectKind,
    EffectParam, EqPatch, FilterPatch, LimiterPatch, MasterChannel, ReverbPatch,
};
use cadence_core::music::{midi_to_degree, ChordTone, EndRule, Step};
use cadence_core::params::{InstrumentParam, ParamAddress, ValueMode};
use cadence_core::rack::{
    drum_midi, normalize_cable_amount, EnvMode, EnvModule, FilterMode, FilterModule, InPort,
    LfoModule, LfoShape, ModuleKind, ModuleKindTag, ModuleParam, OutModule, PanelPin, PatchCable,
    PitchTracking, RackModule, RackParam, RackPatch, RackZone, SampleLoopMode, SamplerModule,
    TriggerMode, VcaModule, VcoModule, VoiceMode, VoiceSettings, Waveform, ZoneParam,
};
use cadence_core::sampler::SampleLibrary;
use cadence_engine::audio::render_schedule_pcm;
use std::path::PathBuf;

const ROOT_MIDI: u8 = 45; // A2 (110 Hz)
const SCALE_A_MINOR: [i8; 7] = [0, 2, 3, 5, 7, 8, 10]; // A natural minor

// Stable module & cable IDs inside Iron Circuit Racks referenced by Links, Macros & Modulations.
const KIT_SAMPLER_MOD: u16 = 1;
const KIT_FILTER_MOD: u16 = 2;
const ACID_FILTER_MOD: u16 = 2;
const ACID_ENV_MOD_CABLE: u16 = 5;
const FOLD_FILTER_MOD: u16 = 3;

// Stable 1-based zone IDs inside TR-808 & TR-909 kit Racks.
const ZONE_808_CP: u16 = 8;
const ZONE_808_CB: u16 = 9;
const ZONE_808_LC: u16 = 12;
const ZONE_808_MC: u16 = 13;
const ZONE_808_HC: u16 = 14;
const ZONE_909_CH: u16 = 7;
const ZONE_909_OH: u16 = 11;

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

fn fx(id: Id, name: &str, kind: EffectKind) -> EffectDevice {
    EffectDevice {
        id,
        name: name.into(),
        bypassed: false,
        kind,
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

/// Convert exact drum MIDI note (e.g. `drum_midi::BD = 36`) into a scale-relative `Step`
/// in A natural minor (`root_midi = 45`).
fn drum_step(midi: u8, velocity: f32) -> Step {
    let (degree, accidental) = midi_to_degree(ROOT_MIDI, &SCALE_A_MINOR, midi, 0);
    Step {
        active: true,
        degree,
        accidental,
        velocity,
        gate: 0.75,
        ..Step::default()
    }
}

/// Stack multiple simultaneous drum hits on a single step (same kit).
fn drum_chord(hits: &[(u8, f32)]) -> Step {
    let mut s = drum_step(hits[0].0, hits[0].1);
    s.chord = hits[1..]
        .iter()
        .map(|&(midi, velocity)| {
            let (degree, accidental) = midi_to_degree(ROOT_MIDI, &SCALE_A_MINOR, midi, 0);
            ChordTone {
                degree,
                accidental,
                velocity,
                gate: 0.75,
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

#[allow(clippy::too_many_arguments)]
fn sample_zone(
    id: u16,
    name: &str,
    midi: u8,
    path: &str,
    gain_db: f32,
    pan: f32,
    choke_group: Option<u8>,
    cutoff_hz: f32,
) -> RackZone {
    RackZone {
        id,
        name: name.into(),
        low_midi: midi,
        high_midi: midi,
        root_midi: midi,
        low_vel: 0.0,
        high_vel: 1.0,
        gain_db,
        pan,
        choke_group,
        voice: VoiceSettings::default(),
        modules: vec![
            RackModule::new(
                KIT_SAMPLER_MOD,
                "Sampler",
                ModuleKind::Sampler(SamplerModule {
                    path: Some(path.into()),
                    root_midi: midi,
                    one_shot: true,
                    ..SamplerModule::default()
                }),
            ),
            RackModule::new(
                KIT_FILTER_MOD,
                "Filter",
                ModuleKind::Filter(FilterModule {
                    mode: FilterMode::ImLpf2,
                    cutoff_hz,
                    resonance: 0.0,
                    drive: 0.0,
                    kerr_semitones: 0.0,
                    spread: 0.0,
                    key_tracking: 0.0,
                    vel_amount: 0.0,
                    mix: 1.0,
                }),
            ),
            RackModule::new(3, "Out", ModuleKind::Out(OutModule::default())),
        ],
        cables: vec![
            PatchCable::new(1, KIT_SAMPLER_MOD, KIT_FILTER_MOD, InPort::In, 1.0),
            PatchCable::new(2, KIT_FILTER_MOD, 3, InPort::In, 1.0),
        ],
    }
}

fn tr808_kit() -> RackPatch {
    // 16 authentic TR-808 zones using 100% MIT factory samples (repo-relative from examples/iron-circuit/).
    // Stable 1-based zone IDs: 1=BD, 2=SD, 3=LT, 4=MT, 5=HT, 6=RS, 7=CH, 8=CP,
    //                          9=CB, 10=CY, 11=OH, 12=LC, 13=MC, 14=HC, 15=CL, 16=MA.
    let base = "../../factory-samples/tr-808";
    RackPatch {
        kit_name: "TR-808".into(),
        zones: vec![
            sample_zone(
                1,
                "BD",
                drum_midi::BD,
                &format!("{base}/01-bd.wav"),
                0.0,
                0.0,
                None,
                20_000.0,
            ),
            sample_zone(
                2,
                "SD",
                drum_midi::SD,
                &format!("{base}/02-sd.wav"),
                -0.2,
                -0.04,
                None,
                20_000.0,
            ),
            sample_zone(
                3,
                "LT",
                drum_midi::LT,
                &format!("{base}/03-lt.wav"),
                0.0,
                -0.34,
                None,
                16_000.0,
            ),
            sample_zone(
                4,
                "MT",
                drum_midi::MT,
                &format!("{base}/04-mt.wav"),
                0.0,
                0.04,
                None,
                16_000.0,
            ),
            sample_zone(
                5,
                "HT",
                drum_midi::HT,
                &format!("{base}/05-ht.wav"),
                0.0,
                0.36,
                None,
                16_000.0,
            ),
            sample_zone(
                6,
                "RS",
                drum_midi::RS,
                &format!("{base}/06-rs.wav"),
                0.4,
                -0.26,
                None,
                18_000.0,
            ),
            sample_zone(
                7,
                "CH",
                drum_midi::CH,
                &format!("{base}/07-ch.wav"),
                -3.4,
                0.18,
                Some(1),
                18_000.0,
            ),
            sample_zone(
                8,
                "CP",
                drum_midi::CP,
                &format!("{base}/08-cp.wav"),
                -0.5,
                -0.10,
                None,
                14_000.0,
            ),
            sample_zone(
                9,
                "CB",
                drum_midi::CB_OR_CR,
                &format!("{base}/09-cb.wav"),
                -2.4,
                0.28,
                None,
                12_000.0,
            ),
            sample_zone(
                10,
                "CY",
                drum_midi::CY_OR_RD,
                &format!("{base}/10-cy.wav"),
                -6.5,
                -0.24,
                None,
                16_000.0,
            ),
            sample_zone(
                11,
                "OH",
                drum_midi::OH,
                &format!("{base}/11-oh.wav"),
                -4.2,
                0.24,
                Some(1),
                16_000.0,
            ),
            sample_zone(
                12,
                "LC",
                drum_midi::LC,
                &format!("{base}/12-lc.wav"),
                1.0,
                -0.42,
                None,
                12_000.0,
            ),
            sample_zone(
                13,
                "MC",
                drum_midi::MC,
                &format!("{base}/13-mc.wav"),
                0.8,
                0.08,
                None,
                12_000.0,
            ),
            sample_zone(
                14,
                "HC",
                drum_midi::HC,
                &format!("{base}/14-hc.wav"),
                0.8,
                0.44,
                None,
                14_000.0,
            ),
            sample_zone(
                15,
                "CL",
                drum_midi::CL,
                &format!("{base}/15-cl.wav"),
                -0.2,
                -0.32,
                None,
                16_000.0,
            ),
            sample_zone(
                16,
                "MA",
                drum_midi::MA,
                &format!("{base}/16-ma.wav"),
                -1.8,
                0.32,
                None,
                18_000.0,
            ),
        ],
        gain_db: 0.0,
        pan: 0.0,
        tune_semitones: 0.0,
        cutoff_hz: 20_000.0,
        resonance: 0.0,
        panel_pins: Vec::new(),
    }
}

fn tr808_kick_rack() -> RackPatch {
    // Dedicated TR-808 BD instance routed into the same `808 Kit` channel so its Part
    // drives the `Synths` sidechain Duck cleanly without false triggers from congas/hats!
    let zone = RackZone {
        id: 1,
        name: "808 Kick".into(),
        low_midi: 0,
        high_midi: 127,
        root_midi: drum_midi::BD,
        low_vel: 0.0,
        high_vel: 1.0,
        gain_db: 0.0,
        pan: 0.0,
        choke_group: None,
        voice: VoiceSettings::default(),
        modules: vec![
            RackModule::new(
                1,
                "Sampler",
                ModuleKind::Sampler(SamplerModule {
                    path: Some("../../factory-samples/tr-808/01-bd.wav".into()),
                    root_midi: drum_midi::BD,
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
                    slices: Vec::new(),
                }),
            ),
            RackModule::new(2, "Out", ModuleKind::Out(OutModule::default())),
        ],
        cables: vec![PatchCable::new(1, 1, 2, InPort::In, 1.0)],
    };
    single_zone_rack(
        zone,
        vec![
            PanelPin::module(1, 1, ModuleParam::RootMidi, "Root"),
            PanelPin::module(1, 1, ModuleParam::CoarseTune, "Coarse"),
            PanelPin::module(1, 1, ModuleParam::Fine, "Fine"),
            PanelPin::module(1, 2, ModuleParam::Pan, "Pan"),
        ],
    )
}

fn tr909_kit() -> RackPatch {
    // 11 authentic TR-909 zones using 100% MIT factory samples.
    // Stable 1-based zone IDs: 1=BD, 2=SD, 3=LT, 4=MT, 5=HT, 6=RS, 7=CH, 8=CP, 9=CR, 10=RD, 11=OH.
    let base = "../../factory-samples/tr-909";
    RackPatch {
        kit_name: "TR-909".into(),
        zones: vec![
            sample_zone(
                1,
                "BD",
                drum_midi::BD,
                &format!("{base}/01-bd.wav"),
                -2.5,
                0.0,
                None,
                16_000.0,
            ),
            sample_zone(
                2,
                "SD",
                drum_midi::SD,
                &format!("{base}/02-sd.wav"),
                0.5,
                0.04,
                None,
                18_000.0,
            ),
            sample_zone(
                3,
                "LT",
                drum_midi::LT,
                &format!("{base}/03-lt.wav"),
                1.2,
                -0.32,
                None,
                16_000.0,
            ),
            sample_zone(
                4,
                "MT",
                drum_midi::MT,
                &format!("{base}/04-mt.wav"),
                1.2,
                0.05,
                None,
                16_000.0,
            ),
            sample_zone(
                5,
                "HT",
                drum_midi::HT,
                &format!("{base}/05-ht.wav"),
                1.2,
                0.34,
                None,
                16_000.0,
            ),
            sample_zone(
                6,
                "RS",
                drum_midi::RS,
                &format!("{base}/06-rs.wav"),
                1.8,
                0.28,
                None,
                18_000.0,
            ),
            sample_zone(
                7,
                "CH",
                drum_midi::CH,
                &format!("{base}/07-ch.wav"),
                -3.2,
                -0.18,
                Some(1),
                16_500.0,
            ),
            sample_zone(
                8,
                "CP",
                drum_midi::CP,
                &format!("{base}/08-cp.wav"),
                -0.8,
                0.12,
                None,
                15_000.0,
            ),
            sample_zone(
                9,
                "CR",
                drum_midi::CB_OR_CR,
                &format!("{base}/09-cr.wav"),
                -5.4,
                -0.26,
                None,
                16_000.0,
            ),
            sample_zone(
                10,
                "RD",
                drum_midi::CY_OR_RD,
                &format!("{base}/10-rd.wav"),
                -3.4,
                0.30,
                None,
                17_000.0,
            ),
            sample_zone(
                11,
                "OH",
                drum_midi::OH,
                &format!("{base}/11-oh.wav"),
                -2.2,
                0.22,
                Some(1),
                15_500.0,
            ),
        ],
        gain_db: 0.0,
        pan: 0.0,
        tune_semitones: 0.0,
        cutoff_hz: 16_000.0,
        resonance: 0.04,
        panel_pins: Vec::new(),
    }
}

fn electro_stab_rack() -> RackPatch {
    // Filtered Detroit electro chord stab using the MIT brass-pad.wav (root C4 = MIDI 60).
    let zone = RackZone {
        id: 1,
        name: "Electro Stab".into(),
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
                "Sampler",
                ModuleKind::Sampler(SamplerModule {
                    path: Some("../../examples/demo/samples/brass-pad.wav".into()),
                    root_midi: 60,
                    coarse_semitones: 0,
                    fine_cents: 0.0,
                    sample_start: 0,
                    sample_end: 0,
                    loop_mode: SampleLoopMode::Off,
                    loop_start: 0,
                    loop_end: 0,
                    loop_xfade_ms: 4.0,
                    one_shot: false,
                    level: 1.0,
                    slices: Vec::new(),
                }),
            ),
            RackModule::new(
                2,
                "Filter",
                ModuleKind::Filter(FilterModule {
                    mode: FilterMode::ImLpf2,
                    cutoff_hz: 3600.0,
                    resonance: 0.24,
                    drive: 0.0,
                    kerr_semitones: 0.0,
                    spread: 0.0,
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
                    decay_ms: 235.0,
                    sustain: 0.22,
                    release_ms: 145.0,
                    trigger_mode: TriggerMode::Gate,
                    ..EnvModule::default()
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
    single_zone_rack(
        zone,
        vec![
            PanelPin::module(1, 1, ModuleParam::RootMidi, "Root"),
            PanelPin::module(1, 2, ModuleParam::Cutoff, "Cutoff"),
            PanelPin::module(1, 2, ModuleParam::Resonance, "Res"),
            PanelPin::module(1, 4, ModuleParam::Attack, "Attack"),
            PanelPin::module(1, 4, ModuleParam::Decay, "Decay"),
            PanelPin::module(1, 4, ModuleParam::Sustain, "Sustain"),
            PanelPin::module(1, 4, ModuleParam::Release, "Release"),
            PanelPin::module(1, 5, ModuleParam::Pan, "Pan"),
        ],
    )
}

fn acid_303_rack() -> RackPatch {
    // Authentic TB-303 Acid Lead: single raw Saw into 4-pole nonlinear DiodeLadder
    // with mono legato glide and native velocity > 0.85 accent squelch!
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
            glide_ms: 56.0,
            pitch_tracking: PitchTracking::Chromatic,
            phase_reset: true,
        },
        modules: vec![
            RackModule::new(
                1,
                "VCO",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Saw,
                    level: 0.92,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                ACID_FILTER_MOD,
                "Acid Filter",
                ModuleKind::Filter(FilterModule {
                    mode: FilterMode::DiodeLadder,
                    cutoff_hz: 420.0,
                    resonance: 0.72,
                    drive: 0.32,
                    kerr_semitones: 0.0,
                    spread: 0.0,
                    key_tracking: 0.28,
                    vel_amount: 2.35, // Accents (vel >= 0.94) trigger authentic 303 wow + snappy decay!
                    mix: 1.0,
                }),
            ),
            RackModule::new(3, "VCA", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(
                4,
                "Amp Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::Adsr,
                    attack_ms: 1.0,
                    decay_ms: 260.0,
                    sustain: 0.56,
                    release_ms: 52.0,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(
                5,
                "Filter Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::Adsr,
                    attack_ms: 1.2,
                    decay_ms: 215.0,
                    sustain: 0.0,
                    release_ms: 85.0,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(6, "Out", ModuleKind::Out(OutModule::default())),
        ],
        cables: vec![
            PatchCable::new(1, 1, ACID_FILTER_MOD, InPort::In, 1.0),
            PatchCable::new(2, ACID_FILTER_MOD, 3, InPort::In, 1.0),
            PatchCable::new(3, 3, 6, InPort::In, 1.0),
            PatchCable::new(4, 4, 3, InPort::Cv, 1.0),
            PatchCable::new(ACID_ENV_MOD_CABLE, 5, ACID_FILTER_MOD, InPort::Cv, 2.75),
        ],
    };
    single_zone_rack(
        zone,
        vec![
            PanelPin::module(1, 1, ModuleParam::Waveform, "Wave"),
            PanelPin::module(1, 1, ModuleParam::Tune, "Tune"),
            PanelPin::module(1, ACID_FILTER_MOD, ModuleParam::Cutoff, "Cutoff"),
            PanelPin::module(1, ACID_FILTER_MOD, ModuleParam::Resonance, "Res"),
            PanelPin::cable(1, ACID_ENV_MOD_CABLE, "Env Mod"),
            PanelPin::module(1, 5, ModuleParam::Decay, "Decay"),
            PanelPin::module(1, ACID_FILTER_MOD, ModuleParam::VelAmount, "Accent"),
            PanelPin::module(1, ACID_FILTER_MOD, ModuleParam::Drive, "Drive"),
            PanelPin::zone(1, ZoneParam::Glide, "Glide"),
        ],
    )
}

fn fold_bass_rack() -> RackPatch {
    // Funky Triangle+Sine electro bass designed to bloom through Drive(ShockPort).
    let zone = RackZone {
        id: 1,
        name: "Fold Bass".into(),
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
            glide_ms: 22.0,
            pitch_tracking: PitchTracking::Chromatic,
            phase_reset: true,
        },
        modules: vec![
            RackModule::new(
                1,
                "VCO A",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Triangle,
                    level: 0.88,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                2,
                "VCO B",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Sine,
                    fine_cents: 3.0,
                    level: 0.44,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                FOLD_FILTER_MOD,
                "Filter",
                ModuleKind::Filter(FilterModule {
                    mode: FilterMode::ImLpf2,
                    cutoff_hz: 680.0,
                    resonance: 0.22,
                    drive: 0.16,
                    kerr_semitones: 0.0,
                    spread: 0.0,
                    key_tracking: 0.18,
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
                    attack_ms: 1.2,
                    decay_ms: 220.0,
                    sustain: 0.72,
                    release_ms: 68.0,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(
                6,
                "Filter Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::Adsr,
                    attack_ms: 0.8,
                    decay_ms: 135.0,
                    sustain: 0.16,
                    release_ms: 150.0,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(7, "Out", ModuleKind::Out(OutModule::default())),
        ],
        cables: vec![
            PatchCable::new(1, 1, FOLD_FILTER_MOD, InPort::In, 1.0),
            PatchCable::new(2, 2, FOLD_FILTER_MOD, InPort::In, 1.0),
            PatchCable::new(3, FOLD_FILTER_MOD, 4, InPort::In, 1.0),
            PatchCable::new(4, 4, 7, InPort::In, 1.0),
            PatchCable::new(5, 5, 4, InPort::Cv, 1.0),
            PatchCable::new(6, 6, FOLD_FILTER_MOD, InPort::Cv, 1.9),
        ],
    };
    single_zone_rack(
        zone,
        vec![
            PanelPin::module(1, FOLD_FILTER_MOD, ModuleParam::Cutoff, "Cutoff"),
            PanelPin::module(1, FOLD_FILTER_MOD, ModuleParam::Resonance, "Res"),
            PanelPin::module(1, FOLD_FILTER_MOD, ModuleParam::Drive, "Drive"),
            PanelPin::cable(1, 6, "Env Mod"),
            PanelPin::module(1, 5, ModuleParam::Decay, "Decay"),
            PanelPin::zone(1, ZoneParam::Glide, "Glide"),
        ],
    )
}

fn gate_pad_rack() -> RackPatch {
    // Lush wide analog Detroit electro pad shaped rhythmically by the Filter(FilterEnv) insert.
    let zone = RackZone {
        id: 1,
        name: "Gate Pad".into(),
        low_midi: 0,
        high_midi: 127,
        root_midi: 60,
        low_vel: 0.0,
        high_vel: 1.0,
        gain_db: -11.0,
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
                    fine_cents: -9.0,
                    level: 0.48,
                    unison_voices: 2,
                    unison_detune_cents: 11.0,
                    unison_width: 0.86,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                2,
                "VCO B",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Pulse,
                    fine_cents: 9.0,
                    pulse_width: 0.38,
                    level: 0.42,
                    unison_voices: 2,
                    unison_detune_cents: 11.0,
                    unison_width: 0.86,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                3,
                "Filter",
                ModuleKind::Filter(FilterModule {
                    mode: FilterMode::ImLpf2,
                    cutoff_hz: 5600.0,
                    resonance: 0.14,
                    drive: 0.08,
                    kerr_semitones: 0.0,
                    spread: 0.0,
                    key_tracking: 0.28,
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
                    attack_ms: 28.0,
                    decay_ms: 520.0,
                    sustain: 0.86,
                    release_ms: 280.0,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(
                6,
                "LFO",
                ModuleKind::Lfo(LfoModule {
                    shape: LfoShape::Sine,
                    rate_hz: 0.28,
                    retrigger: false,
                    ..LfoModule::default()
                }),
            ),
            RackModule::new(
                7,
                "Out",
                ModuleKind::Out(OutModule {
                    gain_db: 0.0,
                    pan: 0.0,
                    spread: 0.76,
                }),
            ),
        ],
        cables: vec![
            PatchCable::new(1, 1, 3, InPort::In, 1.0),
            PatchCable::new(2, 2, 3, InPort::In, 1.0),
            PatchCable::new(3, 3, 4, InPort::In, 1.0),
            PatchCable::new(4, 4, 7, InPort::In, 1.0),
            PatchCable::new(5, 5, 4, InPort::Cv, 1.0),
            PatchCable::new(6, 6, 3, InPort::Cv, 0.12),
            PatchCable::new(7, 6, 2, InPort::PwCv, 0.18),
        ],
    };
    single_zone_rack(
        zone,
        vec![
            PanelPin::module(1, 3, ModuleParam::Cutoff, "Cutoff"),
            PanelPin::module(1, 3, ModuleParam::Resonance, "Res"),
            PanelPin::module(1, 5, ModuleParam::Attack, "Attack"),
            PanelPin::module(1, 5, ModuleParam::Release, "Release"),
            PanelPin::module(1, 6, ModuleParam::LfoRate, "LFO Rate"),
            PanelPin::module(1, 7, ModuleParam::Spread, "Spread"),
        ],
    )
}

fn build_iron_circuit() -> Result<Project> {
    let ids = IdGen::new();

    // ─── 1. Pattern IDs needed ahead of mixer (Filter env_pattern_id) ───
    let pat_pad_gate_groove = ids.next();

    // ─── 2. Mixer Architecture: Nested Group -> Group DAG + 3 Returns + Master ───
    // Audio (6): 808 Kit, 909 Kit, Fold Bass, Acid 303, Electro Stab, Gate Pad
    // Groups (3): Percussion -> Rhythm -> Master; Synths -> Master (sidechained off 808 Kick)
    // Returns (3): Dub Tape (Stereo 3/16), PingPong (PingPong 6/16), Room (Reverb)
    let ch_perc_grp = ids.next();
    let ch_rhythm_grp = ids.next();
    let ch_synths_grp = ids.next();
    let ch_dub_ret = ids.next();
    let ch_pp_ret = ids.next();
    let ch_room_ret = ids.next();

    let ch_808 = ids.next();
    let ch_909 = ids.next();
    let ch_fold = ids.next();
    let ch_303 = ids.next();
    let ch_stab = ids.next();
    let ch_pad = ids.next();

    // Named FX device IDs for Macro bindings & clean signal graph inspection (22 total <= 22)
    let fx_perc_comp = ids.next();
    let fx_perc_tape = ids.next();
    let fx_rhythm_eq = ids.next();
    let fx_rhythm_comp = ids.next();
    let fx_synths_eq = ids.next();
    let fx_808_eq = ids.next();
    let fx_909_eq = ids.next();
    let fx_fold_drive = ids.next();
    let fx_fold_eq = ids.next();
    let fx_303_drive = ids.next();
    let fx_303_eq = ids.next();
    let fx_stab_chorus = ids.next();
    let fx_stab_eq = ids.next();
    let fx_pad_filter = ids.next();
    let fx_pad_chorus = ids.next();
    let fx_dub_delay = ids.next();
    let fx_dub_eq = ids.next();
    let fx_pp_delay = ids.next();
    let fx_room_verb = ids.next();
    let fx_master_tape = ids.next();
    let fx_master_eq = ids.next();
    let fx_master_lim = ids.next();

    // Nested Percussion Group -> Rhythm Group
    let mut perc_grp = Channel::new(ch_perc_grp, ChannelKind::Group, "Percussion");
    perc_grp.output = ChannelRef::Channel(ch_rhythm_grp);
    perc_grp.strip.gain = 0.70;
    perc_grp.inserts = vec![
        fx(
            fx_perc_comp,
            "Perc Glue",
            EffectKind::Compressor(CompressorPatch {
                threshold_db: -13.0,
                ratio: 2.6,
                attack_ms: 12.0,
                release_ms: 82.0,
                makeup_db: 0.8,
                mix: 0.62,
            }),
        ),
        fx(
            fx_perc_tape,
            "Perc Tape",
            EffectKind::Drive(DrivePatch {
                mode: DriveMode::AtomicJet,
                drive: 0.24,
                asymmetry: 0.0,
                tone_hz: 14_500.0,
                output_db: 0.0,
                mix: 0.48,
            }),
        ),
    ];

    // Rhythm Group (Percussion Group + Fold Bass) -> Master
    let mut rhythm_grp = Channel::new(ch_rhythm_grp, ChannelKind::Group, "Rhythm");
    rhythm_grp.output = ChannelRef::Master;
    rhythm_grp.strip.gain = 0.74;
    rhythm_grp.inserts = vec![
        fx(
            fx_rhythm_eq,
            "Rhythm Sculpt",
            EffectKind::Eq(EqPatch {
                hpf_hz: 28.0,
                low_db: 1.0,
                mid_db: -0.9,
                mid_hz: 320.0,
                mid_q: 0.9,
                high_db: 1.0,
            }),
        ),
        fx(
            fx_rhythm_comp,
            "Rhythm Glue",
            EffectKind::Compressor(CompressorPatch {
                threshold_db: -12.5,
                ratio: 2.2,
                attack_ms: 16.0,
                release_ms: 98.0,
                makeup_db: 0.6,
                mix: 0.62,
            }),
        ),
    ];

    // Synths Group (Acid 303 + Electro Stab + Gate Pad) -> Master (sidechain Ducked off 808 Kick!)
    let mut synths_grp = Channel::new(ch_synths_grp, ChannelKind::Group, "Synths");
    synths_grp.output = ChannelRef::Master;
    synths_grp.strip.gain = 0.74;
    synths_grp.inserts = vec![fx(
        fx_synths_eq,
        "Synths Pocket",
        EffectKind::Eq(EqPatch {
            hpf_hz: 120.0,
            low_db: -1.4,
            mid_db: 0.5,
            mid_hz: 1450.0,
            mid_q: 0.8,
            high_db: 1.3,
        }),
    )];

    // Return 1: Dub Tape (Delay Stereo 3/16 + analog rolloff EQ)
    let mut dub_ret = Channel::new(ch_dub_ret, ChannelKind::Return, "Dub Tape");
    dub_ret.strip.gain = 0.68;
    dub_ret.inserts = vec![
        fx(
            fx_dub_delay,
            "Dub 3/16",
            EffectKind::Delay(DelayPatch {
                mode: DelayMode::Stereo,
                steps: 3.0,
                feedback: 0.38,
                mix: 1.0,
            }),
        ),
        fx(
            fx_dub_eq,
            "Dub Filter",
            EffectKind::Eq(EqPatch {
                hpf_hz: 340.0,
                low_db: -4.0,
                mid_db: 0.6,
                mid_hz: 1200.0,
                mid_q: 0.9,
                high_db: -3.2,
            }),
        ),
    ];

    // Return 2: PingPong (Delay PingPong 6/16)
    let mut pp_ret = Channel::new(ch_pp_ret, ChannelKind::Return, "PingPong");
    pp_ret.strip.gain = 0.64;
    pp_ret.inserts = vec![fx(
        fx_pp_delay,
        "PingPong 6/16",
        EffectKind::Delay(DelayPatch {
            mode: DelayMode::PingPong,
            steps: 6.0,
            feedback: 0.36,
            mix: 1.0,
        }),
    )];

    // Return 3: Room (Hardware Room Reverb)
    let mut room_ret = Channel::new(ch_room_ret, ChannelKind::Return, "Room");
    room_ret.strip.gain = 0.52;
    room_ret.inserts = vec![fx(
        fx_room_verb,
        "Hardware Room",
        EffectKind::Reverb(ReverbPatch {
            size: 0.54,
            decay_seconds: 1.45,
            damping_hz: 4800.0,
            diffusion: 0.45,
            mix: 1.0,
            width: 0.86,
        }),
    )];

    // Audio Channels
    let mut c_808 = Channel::new(ch_808, ChannelKind::Audio, "808 Kit");
    c_808.output = ChannelRef::Channel(ch_perc_grp);
    c_808.strip = ChannelStrip {
        gain: 0.78,
        pan: 0.0,
    };
    c_808.inserts = vec![fx(
        fx_808_eq,
        "808 Weight",
        EffectKind::Eq(EqPatch {
            hpf_hz: 30.0,
            low_db: 1.2,
            mid_db: -0.8,
            mid_hz: 380.0,
            mid_q: 0.9,
            high_db: 1.0,
        }),
    )];
    c_808.sends = vec![
        ChannelSend {
            target: ch_dub_ret,
            amount: 0.08,
        },
        ChannelSend {
            target: ch_room_ret,
            amount: 0.14,
        },
    ];

    let mut c_909 = Channel::new(ch_909, ChannelKind::Audio, "909 Kit");
    c_909.output = ChannelRef::Channel(ch_perc_grp);
    c_909.strip = ChannelStrip {
        gain: 0.62,
        pan: 0.04,
    };
    c_909.inserts = vec![fx(
        fx_909_eq,
        "909 Crisp",
        EffectKind::Eq(EqPatch {
            hpf_hz: 88.0,
            low_db: -2.0,
            mid_db: 0.8,
            mid_hz: 2600.0,
            mid_q: 0.9,
            high_db: 1.6,
        }),
    )];
    c_909.sends = vec![
        ChannelSend {
            target: ch_dub_ret,
            amount: 0.06,
        },
        ChannelSend {
            target: ch_room_ret,
            amount: 0.18,
        },
    ];

    let mut c_fold = Channel::new(ch_fold, ChannelKind::Audio, "Fold Bass");
    c_fold.output = ChannelRef::Channel(ch_rhythm_grp);
    c_fold.strip = ChannelStrip {
        gain: 0.50,
        pan: 0.0,
    };
    c_fold.inserts = vec![
        fx(
            fx_fold_drive,
            "West Fold",
            EffectKind::Drive(DrivePatch {
                mode: DriveMode::ShockPort,
                drive: 0.34,
                asymmetry: 0.0,
                tone_hz: 2600.0,
                output_db: 0.0,
                mix: 0.56,
            }),
        ),
        fx(
            fx_fold_eq,
            "Fold Anchor",
            EffectKind::Eq(EqPatch {
                hpf_hz: 34.0,
                low_db: 2.2,
                mid_db: -1.4,
                mid_hz: 460.0,
                mid_q: 0.9,
                high_db: -0.6,
            }),
        ),
    ];

    let mut c_303 = Channel::new(ch_303, ChannelKind::Audio, "Acid 303");
    c_303.output = ChannelRef::Channel(ch_synths_grp);
    c_303.strip = ChannelStrip {
        gain: 0.56,
        pan: 0.0,
    };
    c_303.inserts = vec![
        fx(
            fx_303_drive,
            "303 Diode",
            EffectKind::Drive(DrivePatch {
                mode: DriveMode::ShockPort,
                drive: 0.38,
                asymmetry: 0.0,
                tone_hz: 3900.0,
                output_db: -2.4,
                mix: 0.56,
            }),
        ),
        fx(
            fx_303_eq,
            "303 Bite",
            EffectKind::Eq(EqPatch {
                hpf_hz: 155.0,
                low_db: -1.2,
                mid_db: 0.8,
                mid_hz: 1150.0,
                mid_q: 1.0,
                high_db: -2.2,
            }),
        ),
    ];
    c_303.sends = vec![
        ChannelSend {
            target: ch_pp_ret,
            amount: 0.16,
        },
        ChannelSend {
            target: ch_room_ret,
            amount: 0.18,
        },
    ];

    let mut c_stab = Channel::new(ch_stab, ChannelKind::Audio, "Electro Stab");
    c_stab.output = ChannelRef::Channel(ch_synths_grp);
    c_stab.strip = ChannelStrip {
        gain: 1.48,
        pan: 0.18,
    };
    c_stab.inserts = vec![
        fx(
            fx_stab_chorus,
            "Stab Width",
            EffectKind::Chorus(ChorusPatch {
                rate_hz: 0.52,
                depth: 0.46,
                width: 0.86,
                mix: 0.44,
            }),
        ),
        fx(
            fx_stab_eq,
            "Stab Crisp",
            EffectKind::Eq(EqPatch {
                hpf_hz: 200.0,
                low_db: -1.6,
                mid_db: 1.6,
                mid_hz: 1800.0,
                mid_q: 0.9,
                high_db: 2.0,
            }),
        ),
    ];
    c_stab.sends = vec![
        ChannelSend {
            target: ch_pp_ret,
            amount: 0.28,
        },
        ChannelSend {
            target: ch_room_ret,
            amount: 0.26,
        },
    ];

    let mut c_pad = Channel::new(ch_pad, ChannelKind::Audio, "Gate Pad");
    c_pad.output = ChannelRef::Channel(ch_synths_grp);
    c_pad.strip = ChannelStrip {
        gain: 0.96,
        pan: -0.14,
    };
    c_pad.inserts = vec![
        fx(
            fx_pad_filter,
            "Pad Gate",
            EffectKind::Filter(FilterPatch {
                mode: FilterMode::ImLpf2,
                cutoff_hz: 640.0,
                resonance: 0.36,
                drive: 0.16,
                kerr_semitones: 0.0,
                spread: 0.0,
                mix: 1.0,
                env_pattern: 0,
                env_pattern_id: Some(pat_pad_gate_groove),
                env_amount: 2.35,
                env_decay_ms: 170.0,
            }),
        ),
        fx(
            fx_pad_chorus,
            "Pad Swirl",
            EffectKind::Chorus(ChorusPatch {
                rate_hz: 0.36,
                depth: 0.60,
                width: 0.94,
                mix: 0.48,
            }),
        ),
    ];
    c_pad.sends = vec![
        ChannelSend {
            target: ch_pp_ret,
            amount: 0.14,
        },
        ChannelSend {
            target: ch_room_ret,
            amount: 0.34,
        },
    ];

    let mixer = MixerGraph {
        channels: vec![
            c_808, c_909, c_fold, c_303, c_stab, c_pad, perc_grp, rhythm_grp, synths_grp, dub_ret,
            pp_ret, room_ret,
        ],
        master: MasterChannel {
            strip: ChannelStrip {
                gain: 1.0, // Unity 0.0 dB so Master Limiter -0.8 dBFS ceiling lands exact!
                pan: 0.0,
            },
            inserts: vec![
                fx(
                    fx_master_tape,
                    "Master Tape",
                    EffectKind::Drive(DrivePatch {
                        mode: DriveMode::AtomicJet,
                        drive: 0.14,
                        asymmetry: 0.0,
                        tone_hz: 16_800.0,
                        output_db: 0.0,
                        mix: 0.28,
                    }),
                ),
                fx(
                    fx_master_eq,
                    "Master Tilt",
                    EffectKind::Eq(EqPatch {
                        hpf_hz: 26.0,
                        low_db: 0.6,
                        mid_db: -0.3,
                        mid_hz: 360.0,
                        mid_q: 0.8,
                        high_db: 0.9,
                    }),
                ),
                fx(
                    fx_master_lim,
                    "Master Limiter",
                    EffectKind::Limiter(LimiterPatch {
                        ceiling_db: -0.8,
                        input_drive_db: 2.2,
                        release_ms: 68.0,
                        mix: 1.0,
                    }),
                ),
            ],
        },
    };

    let total_fx: usize = mixer.master.inserts.len()
        + mixer
            .channels
            .iter()
            .map(|c| c.inserts.len())
            .sum::<usize>();
    ensure!(total_fx <= 22, "FX device count {total_fx} must stay <= 22");

    // ─── 3. Instruments (7 Unified Modular Rack Instruments) ───
    let inst_808_kick = ids.next();
    let inst_808 = ids.next();
    let inst_909 = ids.next();
    let inst_stab = ids.next();
    let inst_303 = ids.next();
    let inst_fold = ids.next();
    let inst_pad = ids.next();

    let instruments = vec![
        InstrumentInstance {
            id: inst_808_kick,
            name: "808 Kick".into(),
            channel: ch_808,
            rack: tr808_kick_rack(),
        },
        InstrumentInstance {
            id: inst_808,
            name: "TR-808".into(),
            channel: ch_808,
            rack: tr808_kit(),
        },
        InstrumentInstance {
            id: inst_909,
            name: "TR-909".into(),
            channel: ch_909,
            rack: tr909_kit(),
        },
        InstrumentInstance {
            id: inst_stab,
            name: "Electro Stab".into(),
            channel: ch_stab,
            rack: electro_stab_rack(),
        },
        InstrumentInstance {
            id: inst_303,
            name: "Acid 303".into(),
            channel: ch_303,
            rack: acid_303_rack(),
        },
        InstrumentInstance {
            id: inst_fold,
            name: "Fold Bass".into(),
            channel: ch_fold,
            rack: fold_bass_rack(),
        },
        InstrumentInstance {
            id: inst_pad,
            name: "Gate Pad".into(),
            channel: ch_pad,
            rack: gate_pad_rack(),
        },
    ];

    // ─── 4. Retained Patterns (Small Cells -> Composed Pattern Algebra) ───
    let mut patterns = Vec::new();

    // 4a. Custom rhythmic FilterEnv trigger groove for `Gate Pad` (`env_pattern_id`)
    // Syncopated 16-step electro gate: X . x X . x X . x . X x . X x .
    let gate_vels: [f32; 16] = [
        1.0, 0.0, 0.72, 0.92, 0.0, 0.68, 1.0, 0.0, 0.80, 0.0, 0.95, 0.62, 0.0, 0.86, 0.70, 0.0,
    ];
    let gate_steps: Vec<Step> = gate_vels
        .iter()
        .map(|&v| {
            if v > 0.0 {
                step_note(1, v, 0.5)
            } else {
                Step::default()
            }
        })
        .collect();
    patterns.push(Pattern {
        id: pat_pad_gate_groove,
        name: "Pad Gate Groove".into(),
        kind: PatternKind::Notes { steps: gate_steps },
    });

    // 4b. Master 4-bar A-minor electro harmony: Am (0) -> Am (0) -> F (-2) -> G (-1) -> E (-3)
    let pat_harmony = ids.next();
    patterns.push(Pattern {
        id: pat_harmony,
        name: "Circuit Harmony".into(),
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
                    step: 32,
                    value: -2.0,
                },
                ValuePoint {
                    step: 48,
                    value: -1.0,
                },
                ValuePoint {
                    step: 56,
                    value: -3.0,
                },
            ],
        },
    });

    // ─── 4c. TR-808 Kick (Lane 1, drives Sidechain Duck on Synths!) ───
    // Classic syncopated Detroit electro kick: steps 0, 6, 10 (plus 14 on bar-4 turnaround via Every)
    let pat_808_kick_main = ids.next();
    let mut k_main = vec![Step::default(); 16];
    k_main[0] = drum_step(drum_midi::BD, 1.0);
    k_main[6] = drum_step(drum_midi::BD, 0.92);
    k_main[10] = drum_step(drum_midi::BD, 0.98);
    patterns.push(Pattern {
        id: pat_808_kick_main,
        name: "808 Kick Main".into(),
        kind: PatternKind::Notes { steps: k_main },
    });

    let pat_808_kick_turn = ids.next();
    let mut k_turn = vec![Step::default(); 16];
    k_turn[0] = drum_step(drum_midi::BD, 1.0);
    k_turn[6] = drum_step(drum_midi::BD, 0.92);
    k_turn[10] = drum_step(drum_midi::BD, 0.96);
    k_turn[14] = drum_step(drum_midi::BD, 0.86);
    patterns.push(Pattern {
        id: pat_808_kick_turn,
        name: "808 Kick Turn".into(),
        kind: PatternKind::Notes { steps: k_turn },
    });

    let pat_808_kick = ids.next();
    patterns.push(Pattern {
        id: pat_808_kick,
        name: "808 Kick Groove".into(),
        kind: PatternKind::Every {
            interval: 4,
            base: pat_808_kick_main,
            alt: pat_808_kick_turn,
        },
    });

    let pat_808_kick_end = ids.next();
    let mut k_end = vec![Step::default(); 16];
    k_end[0] = drum_step(drum_midi::BD, 0.96);
    patterns.push(Pattern {
        id: pat_808_kick_end,
        name: "808 Kick Fin".into(),
        kind: PatternKind::Notes { steps: k_end },
    });

    // ─── 4d. TR-808 Perc (Lane 2 — strictly non-kick steps 1,2,3,4,5,7,8,9,11,12,13,15!) ───
    // Backbeat Snare + Clap (steps 4 & 12) + turnaround snare ratchet on step 15!
    let pat_808_backbeat = ids.next();
    let mut bb = vec![Step::default(); 16];
    bb[4] = drum_chord(&[(drum_midi::SD, 0.94), (drum_midi::CP, 0.82)]);
    bb[12] = drum_chord(&[(drum_midi::SD, 0.98), (drum_midi::CP, 0.88)]);
    patterns.push(Pattern {
        id: pat_808_backbeat,
        name: "808 Snare Clap".into(),
        kind: PatternKind::Notes { steps: bb },
    });

    let pat_808_fill_hit = ids.next();
    let mut fh = vec![Step::default(); 16];
    fh[15] = drum_step(drum_midi::SD, 0.78);
    patterns.push(Pattern {
        id: pat_808_fill_hit,
        name: "808 Fill Seed".into(),
        kind: PatternKind::Notes { steps: fh },
    });
    let pat_808_fill_ratchet = ids.next();
    patterns.push(Pattern {
        id: pat_808_fill_ratchet,
        name: "808 Snare Ratchet".into(),
        kind: PatternKind::Ratchet {
            source: pat_808_fill_hit,
            count: 2,
        },
    });
    let pat_808_backbeat_fill = ids.next();
    patterns.push(Pattern {
        id: pat_808_backbeat_fill,
        name: "808 Backbeat Fill".into(),
        kind: PatternKind::Stack {
            parts: vec![pat_808_backbeat, pat_808_fill_ratchet],
        },
    });
    let pat_808_snare_every = ids.next();
    patterns.push(Pattern {
        id: pat_808_snare_every,
        name: "808 Snare Every4".into(),
        kind: PatternKind::Every {
            interval: 4,
            base: pat_808_backbeat,
            alt: pat_808_backbeat_fill,
        },
    });

    // Boot 808 hats (steps 2, 3, 5, 7, 8, 9, 11, 13, 15 — never colliding with kick 0, 6, 10, 14!)
    let pat_808_boot_hats = ids.next();
    let mut bh = vec![Step::default(); 16];
    for (s, midi, vel) in [
        (2, drum_midi::CH, 0.72),
        (3, drum_midi::CH, 0.56),
        (5, drum_midi::CH, 0.62),
        (7, drum_midi::OH, 0.76),
        (8, drum_midi::CH, 0.78), // chokes OH!
        (9, drum_midi::CH, 0.58),
        (11, drum_midi::CH, 0.68),
        (13, drum_midi::CH, 0.60),
        (15, drum_midi::MA, 0.64),
    ] {
        bh[s] = drum_step(midi, vel);
    }
    patterns.push(Pattern {
        id: pat_808_boot_hats,
        name: "808 Boot Hats".into(),
        kind: PatternKind::Notes { steps: bh },
    });

    // Swung 808 Congas + Clave + Rimshot via inline Mini-notation (exact MIDI notes, non-kick steps!)
    // b2=47(LC), c3=48(MC), c#3=49(HC), d3=50(CL), f2=41(RS)
    let pat_808_conga_mini = ids.next();
    patterns.push(Pattern {
        id: pat_808_conga_mini,
        name: "808 Conga Mini".into(),
        kind: PatternKind::Mini {
            code: "~ ~ b2 ~ ~ c3 ~ f2 c#3! ~ ~ b2 ~ d3 ~ c3".into(),
            steps: 16,
        },
    });
    let pat_808_conga_swung = ids.next();
    patterns.push(Pattern {
        id: pat_808_conga_swung,
        name: "808 Conga Swing".into(),
        kind: PatternKind::Swing {
            source: pat_808_conga_mini,
            amount: 0.14,
        },
    });

    // Syncopated 808 Cowbell via Euclidean(5, 16, 2) masked safely off kick steps!
    let pat_808_cb_stream = ids.next();
    patterns.push(Pattern {
        id: pat_808_cb_stream,
        name: "808 Cowbell Tone".into(),
        kind: PatternKind::Notes {
            steps: vec![drum_step(drum_midi::CB_OR_CR, 0.80); 16],
        },
    });
    let pat_808_cb_euc = ids.next();
    patterns.push(Pattern {
        id: pat_808_cb_euc,
        name: "Euclid 5/16 CB".into(),
        kind: PatternKind::Euclidean {
            pulses: 5,
            steps: 16,
            rotation: 1, // steps 1, 5, 8, 11, 14 -> wait: let's mask off step 14 too!
        },
    });
    let pat_808_cb_raw = ids.next();
    patterns.push(Pattern {
        id: pat_808_cb_raw,
        name: "808 CB Euclid".into(),
        kind: PatternKind::Mask {
            notes: pat_808_cb_stream,
            rhythm: pat_808_cb_euc,
        },
    });
    let pat_non_kick_mask = ids.next();
    patterns.push(Pattern {
        id: pat_non_kick_mask,
        name: "Non-Kick Mask".into(),
        kind: PatternKind::Rhythm {
            // Excludes 0, 6, 10, 14 so 808 Perc NEVER collides with 808 Kick!
            steps: rhythm_from_str(".xxx xx.x xx.x xx.x"),
        },
    });
    let pat_808_cb = ids.next();
    patterns.push(Pattern {
        id: pat_808_cb,
        name: "808 Cowbell Sync".into(),
        kind: PatternKind::Mask {
            notes: pat_808_cb_raw,
            rhythm: pat_non_kick_mask,
        },
    });

    // Composed 808 Perc variants via Stack:
    let pat_808_boot = ids.next();
    patterns.push(Pattern {
        id: pat_808_boot,
        name: "808 Boot Break".into(),
        kind: PatternKind::Stack {
            parts: vec![pat_808_snare_every, pat_808_boot_hats],
        },
    });

    let pat_808_full_perc = ids.next();
    patterns.push(Pattern {
        id: pat_808_full_perc,
        name: "808 Full Perc".into(),
        kind: PatternKind::Stack {
            parts: vec![pat_808_snare_every, pat_808_conga_swung],
        },
    });

    let pat_808_overload_perc = ids.next();
    patterns.push(Pattern {
        id: pat_808_overload_perc,
        name: "808 Overload Perc".into(),
        kind: PatternKind::Stack {
            parts: vec![pat_808_snare_every, pat_808_conga_swung, pat_808_cb],
        },
    });

    // Voltage Drop stripped 808 percussion (no kick, hypnotic congas + cowbell + claves + maracas)
    let pat_808_drop_mini = ids.next();
    patterns.push(Pattern {
        id: pat_808_drop_mini,
        name: "808 Drop Perc".into(),
        kind: PatternKind::Mini {
            code: "b2 ~ g#2 d3 ~ c3 g#2 ~ c#3! d#3 b2 ~ d3 g#2 c3 ~".into(),
            steps: 16,
        },
    });
    let pat_808_drop_swung = ids.next();
    patterns.push(Pattern {
        id: pat_808_drop_swung,
        name: "808 Drop Swung".into(),
        kind: PatternKind::Swing {
            source: pat_808_drop_mini,
            amount: 0.14,
        },
    });

    // Per-Zone P-lock modulation patterns for TR-808 (Congas panning + Clap filter!)
    let pat_808_lc_pan = ids.next();
    patterns.push(Pattern {
        id: pat_808_lc_pan,
        name: "808 LC Pan Seed".into(),
        kind: PatternKind::SeededValues {
            unit: ValueUnit::Normalized,
            period: 16,
            seed: 808,
            min: ZoneParam::Pan.normalize(-0.72),
            max: ZoneParam::Pan.normalize(0.44),
        },
    });

    let pat_808_hc_pan = ids.next();
    patterns.push(Pattern {
        id: pat_808_hc_pan,
        name: "808 HC Pan Wave".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 16,
            mode: ValueMode::Ramp,
            points: vec![
                ValuePoint {
                    step: 0,
                    value: ZoneParam::Pan.normalize(0.68),
                },
                ValuePoint {
                    step: 8,
                    value: ZoneParam::Pan.normalize(-0.44),
                },
                ValuePoint {
                    step: 15,
                    value: ZoneParam::Pan.normalize(0.60),
                },
            ],
        },
    });

    let pat_808_cp_cutoff = ids.next();
    patterns.push(Pattern {
        id: pat_808_cp_cutoff,
        name: "808 CP Filter".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 32,
            mode: ValueMode::Ramp,
            points: vec![
                ValuePoint {
                    step: 0,
                    value: ModuleParam::Cutoff.normalize(3200.0),
                },
                ValuePoint {
                    step: 16,
                    value: ModuleParam::Cutoff.normalize(15_000.0),
                },
                ValuePoint {
                    step: 31,
                    value: ModuleParam::Cutoff.normalize(4200.0),
                },
            ],
        },
    });

    // ─── 4e. TR-909 Crisp Top Kit (Lane 3 — single lane, stacked cleanly!) ───
    // 16th closed/open hats with velocity Shape + Swing(0.14)
    let pat_909_hats_base = ids.next();
    let mut h9 = vec![drum_step(drum_midi::CH, 0.80); 16];
    h9[2] = drum_step(drum_midi::OH, 0.84); // open hat choked by step 3 CH!
    h9[10] = drum_step(drum_midi::OH, 0.88); // open hat choked by step 11 CH!
    h9[14] = drum_step(drum_midi::OH, 0.82); // open hat choked by step 15 CH!
    patterns.push(Pattern {
        id: pat_909_hats_base,
        name: "909 Hats Grid".into(),
        kind: PatternKind::Notes { steps: h9 },
    });

    let pat_909_hat_vel = ids.next();
    patterns.push(Pattern {
        id: pat_909_hat_vel,
        name: "909 Hat Accent".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 16,
            mode: ValueMode::Hold,
            points: vec![
                ValuePoint {
                    step: 0,
                    value: 0.78,
                },
                ValuePoint {
                    step: 1,
                    value: 0.44,
                },
                ValuePoint {
                    step: 2,
                    value: 0.86,
                },
                ValuePoint {
                    step: 3,
                    value: 0.52,
                },
                ValuePoint {
                    step: 4,
                    value: 0.72,
                },
                ValuePoint {
                    step: 5,
                    value: 0.42,
                },
                ValuePoint {
                    step: 6,
                    value: 0.82,
                },
                ValuePoint {
                    step: 7,
                    value: 0.48,
                },
                ValuePoint {
                    step: 8,
                    value: 0.76,
                },
                ValuePoint {
                    step: 9,
                    value: 0.44,
                },
                ValuePoint {
                    step: 10,
                    value: 0.90,
                },
                ValuePoint {
                    step: 11,
                    value: 0.54,
                },
                ValuePoint {
                    step: 12,
                    value: 0.74,
                },
                ValuePoint {
                    step: 13,
                    value: 0.46,
                },
                ValuePoint {
                    step: 14,
                    value: 0.84,
                },
                ValuePoint {
                    step: 15,
                    value: 0.56,
                },
            ],
        },
    });
    let pat_909_hats_shaped = ids.next();
    patterns.push(Pattern {
        id: pat_909_hats_shaped,
        name: "909 Hats Shaped".into(),
        kind: PatternKind::Shape {
            source: pat_909_hats_base,
            velocity: pat_909_hat_vel,
            gate: None,
        },
    });
    let pat_909_hats_swung = ids.next();
    patterns.push(Pattern {
        id: pat_909_hats_swung,
        name: "909 Hats Groove".into(),
        kind: PatternKind::Swing {
            source: pat_909_hats_shaped,
            amount: 0.14,
        },
    });

    // Crisp 909 Snare + Rim backbeat layer
    let pat_909_snare = ids.next();
    let mut s9 = vec![Step::default(); 16];
    s9[4] = drum_step(drum_midi::SD, 0.72);
    s9[12] = drum_step(drum_midi::SD, 0.76);
    s9[9] = drum_step(drum_midi::RS, 0.56);
    patterns.push(Pattern {
        id: pat_909_snare,
        name: "909 Snare Layer".into(),
        kind: PatternKind::Notes { steps: s9 },
    });

    // 909 Bar-4 Tom/Snare Fill
    let pat_909_fill = ids.next();
    let mut f9 = vec![Step::default(); 16];
    f9[4] = drum_step(drum_midi::SD, 0.74);
    f9[12] = drum_chord(&[(drum_midi::SD, 0.82), (drum_midi::CP, 0.70)]);
    f9[13] = drum_step(drum_midi::HT, 0.72);
    f9[14] = drum_step(drum_midi::MT, 0.78);
    f9[15] = drum_step(drum_midi::LT, 0.82);
    patterns.push(Pattern {
        id: pat_909_fill,
        name: "909 Tom Fill".into(),
        kind: PatternKind::Notes { steps: f9 },
    });
    let pat_909_snare_every = ids.next();
    patterns.push(Pattern {
        id: pat_909_snare_every,
        name: "909 Snare Every4".into(),
        kind: PatternKind::Every {
            interval: 4,
            base: pat_909_snare,
            alt: pat_909_fill,
        },
    });

    // 909 Circuit Groove (Hats + Snare)
    let pat_909_groove = ids.next();
    patterns.push(Pattern {
        id: pat_909_groove,
        name: "909 Circuit Groove".into(),
        kind: PatternKind::Stack {
            parts: vec![pat_909_hats_swung, pat_909_snare_every],
        },
    });

    // 909 Ride Euclidean(7, 16, 0) for Overload climax!
    let pat_909_ride_stream = ids.next();
    patterns.push(Pattern {
        id: pat_909_ride_stream,
        name: "909 Ride Tone".into(),
        kind: PatternKind::Notes {
            steps: vec![drum_step(drum_midi::CY_OR_RD, 0.72); 16],
        },
    });
    let pat_909_ride_euc = ids.next();
    patterns.push(Pattern {
        id: pat_909_ride_euc,
        name: "Euclid 7/16 Ride".into(),
        kind: PatternKind::Euclidean {
            pulses: 7,
            steps: 16,
            rotation: 0,
        },
    });
    let pat_909_ride = ids.next();
    patterns.push(Pattern {
        id: pat_909_ride,
        name: "909 Ride Euclid".into(),
        kind: PatternKind::Mask {
            notes: pat_909_ride_stream,
            rhythm: pat_909_ride_euc,
        },
    });
    let pat_909_overload = ids.next();
    patterns.push(Pattern {
        id: pat_909_overload,
        name: "909 Overload".into(),
        kind: PatternKind::Stack {
            parts: vec![pat_909_hats_swung, pat_909_snare_every, pat_909_ride],
        },
    });

    // 909 Section Crash (1-bar hit stacked with groove on drop bars!)
    let pat_909_crash_hit = ids.next();
    let mut cr = vec![Step::default(); 16];
    cr[0] = drum_step(drum_midi::CB_OR_CR, 0.86);
    patterns.push(Pattern {
        id: pat_909_crash_hit,
        name: "909 Crash".into(),
        kind: PatternKind::Notes { steps: cr },
    });
    let pat_909_drop_hit = ids.next();
    patterns.push(Pattern {
        id: pat_909_drop_hit,
        name: "909 Crash + Groove".into(),
        kind: PatternKind::Stack {
            parts: vec![pat_909_crash_hit, pat_909_groove],
        },
    });

    // 909 Voltage Drop soft ticking hats + Bar 40 Ratchet Snare Roll riser!
    let pat_909_tick = ids.next();
    let mut t9 = vec![Step::default(); 16];
    for (s, v) in [
        (0, 0.56),
        (4, 0.48),
        (8, 0.56),
        (10, 0.62),
        (12, 0.48),
        (14, 0.52),
    ] {
        t9[s] = drum_step(
            if s == 10 {
                drum_midi::OH
            } else {
                drum_midi::CH
            },
            v,
        );
    }
    patterns.push(Pattern {
        id: pat_909_tick,
        name: "909 Breakdown Tick".into(),
        kind: PatternKind::Notes { steps: t9 },
    });

    let pat_909_riser_seed = ids.next();
    let mut r9 = vec![Step::default(); 16];
    for s in [0, 2, 4, 6, 8, 9, 10, 11, 12, 13, 14, 15] {
        let vel = 0.45 + (s as f32 / 15.0) * 0.52;
        r9[s] = drum_step(drum_midi::SD, vel);
    }
    patterns.push(Pattern {
        id: pat_909_riser_seed,
        name: "909 Build Seed".into(),
        kind: PatternKind::Notes { steps: r9 },
    });
    let pat_909_riser_ratchet = ids.next();
    patterns.push(Pattern {
        id: pat_909_riser_ratchet,
        name: "909 Snare Roll".into(),
        kind: PatternKind::Ratchet {
            source: pat_909_riser_seed,
            count: 2,
        },
    });

    // Per-Zone P-lock for TR-909 OH Pan
    let pat_909_oh_pan = ids.next();
    patterns.push(Pattern {
        id: pat_909_oh_pan,
        name: "909 OH Pan".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 16,
            mode: ValueMode::Ramp,
            points: vec![
                ValuePoint {
                    step: 0,
                    value: ZoneParam::Pan.normalize(-0.28),
                },
                ValuePoint {
                    step: 8,
                    value: ZoneParam::Pan.normalize(0.44),
                },
                ValuePoint {
                    step: 15,
                    value: ZoneParam::Pan.normalize(-0.20),
                },
            ],
        },
    });

    // ─── 4f. Fold Bass (Lane 4: West-Coast Electro Funk -> AddDegrees + RangeFold!) ───
    let pat_fold_cell = ids.next();
    let mut fb = vec![Step::default(); 32];
    // Bar 1: syncopated pocket around 808 kicks (0, 6, 10)
    fb[0] = step_note(1, 0.88, 1.5);
    fb[3] = step_note(1, 0.78, 1.4);
    fb[6] = step_note(3, 0.86, 1.2);
    fb[8] = step_note(1, 0.82, 1.4);
    fb[11] = step_note(0, 0.80, 1.2); // G
    fb[14] = step_note(5, 0.90, 1.5); // E (folded cleanly!)
                                      // Bar 2: call-and-response bounce
    fb[16] = step_note(1, 0.88, 1.5);
    fb[19] = step_note(1, 0.76, 1.4);
    fb[22] = step_note(4, 0.86, 1.2); // D
    fb[24] = step_note(3, 0.84, 1.2); // C
    fb[26] = step_note(1, 0.90, 1.5); // A
    fb[28] = step_note(0, 0.78, 1.2); // G
    fb[30] = step_note(1, 0.86, 1.3); // A
    patterns.push(Pattern {
        id: pat_fold_cell,
        name: "Fold Bass Cell".into(),
        kind: PatternKind::Notes { steps: fb },
    });

    let pat_fold_harm = ids.next();
    patterns.push(Pattern {
        id: pat_fold_harm,
        name: "Fold Bass Harm".into(),
        kind: PatternKind::AddDegrees {
            notes: pat_fold_cell,
            values: pat_harmony,
        },
    });

    // RangeFold cleanly into A1..A2 (MIDI 33..45) for chest-thumping electro sub!
    let pat_fold_groove = ids.next();
    patterns.push(Pattern {
        id: pat_fold_groove,
        name: "Fold Bass Groove".into(),
        kind: PatternKind::RangeFold {
            source: pat_fold_harm,
            low_midi: 33,
            high_midi: 45,
        },
    });

    // Sparse Boot teaser mask (Bars 5-8)
    let pat_fold_teaser_mask = ids.next();
    patterns.push(Pattern {
        id: pat_fold_teaser_mask,
        name: "Bass Teaser Mask".into(),
        kind: PatternKind::Rhythm {
            steps: rhythm_from_str("x..x ..x. .... ...."),
        },
    });
    let pat_fold_teaser = ids.next();
    patterns.push(Pattern {
        id: pat_fold_teaser,
        name: "Fold Bass Teaser".into(),
        kind: PatternKind::Mask {
            notes: pat_fold_groove,
            rhythm: pat_fold_teaser_mask,
        },
    });

    // ─── 4g. Acid 303 (Lane 5: Squelchy Native Accents > 0.85 + Mono Legato Slides!) ───
    // Main 16-step hypnotic Analord/UR acid motif in A minor (root_midi = 45 = A2):
    let pat_303_main = ids.next();
    let mut a1 = vec![Step::default(); 16];
    a1[0] = step_note(1, 0.96, 0.65); // A2 ACCENT!
    a1[1] = step_note(1, 0.70, 0.55); // A2
    a1[2] = step_note(8, 0.96, 1.35); // A3 ACCENT + SLIDE ->
    a1[3] = step_note(7, 0.72, 0.65); // G3 (slide landing)
    a1[5] = step_note(5, 0.74, 0.60); // E3
    a1[6] = step_note(3, 0.96, 1.35); // C3 ACCENT + SLIDE ->
    a1[7] = step_note(4, 0.72, 0.65); // D3 (slide landing)
    a1[8] = step_note(1, 0.76, 0.60); // A2
    a1[10] = step_note(0, 0.95, 0.65); // G2 ACCENT!
    a1[11] = step_note(1, 0.72, 1.30); // A2 SLIDE ->
    a1[12] = step_note(3, 0.75, 0.65); // C3
    a1[13] = step_note(5, 0.98, 0.60); // E3 SCREAMING ACCENT!
    a1[14] = step_note(7, 0.74, 1.35); // G3 SLIDE ->
    a1[15] = step_note(5, 0.70, 0.55); // E3
    patterns.push(Pattern {
        id: pat_303_main,
        name: "303 Acid Motif".into(),
        kind: PatternKind::Notes { steps: a1 },
    });

    // Bar-4 turnaround variation (climbs higher for phrasing punctuation)
    let pat_303_var = ids.next();
    let mut a2 = vec![Step::default(); 16];
    a2[0] = step_note(1, 0.96, 0.65); // A2 ACCENT!
    a2[2] = step_note(8, 0.96, 1.35); // A3 ACCENT + SLIDE ->
    a2[3] = step_note(10, 0.76, 0.65); // C4
    a2[5] = step_note(8, 0.74, 0.60); // A3
    a2[6] = step_note(6, 0.96, 1.35); // F3 ACCENT + SLIDE ->
    a2[7] = step_note(5, 0.74, 0.65); // E3
    a2[8] = step_note(4, 0.76, 0.60); // D3
    a2[10] = step_note(5, 0.96, 1.30); // E3 ACCENT + SLIDE ->
    a2[11] = step_note(7, 0.74, 0.65); // G3
    a2[12] = step_note(8, 0.98, 1.35); // A3 ACCENT + SLIDE ->
    a2[13] = step_note(7, 0.74, 0.60); // G3
    a2[14] = step_note(5, 0.95, 0.65); // E3 ACCENT!
    a2[15] = step_note(3, 0.70, 0.55); // C3
    patterns.push(Pattern {
        id: pat_303_var,
        name: "303 Acid Turn".into(),
        kind: PatternKind::Notes { steps: a2 },
    });

    let pat_303_line = ids.next();
    patterns.push(Pattern {
        id: pat_303_line,
        name: "303 Acid Line".into(),
        kind: PatternKind::Every {
            interval: 4,
            base: pat_303_main,
            alt: pat_303_var,
        },
    });

    // Overload Peak Acid: gnarly meaty TB-303 register (A2..E4) with ONE fiery E4 climax accent!
    let pat_303_peak_cell = ids.next();
    let mut a3 = vec![Step::default(); 16];
    a3[0] = step_note(1, 0.98, 0.65); // A2 ACCENT!
    a3[1] = step_note(8, 0.72, 0.55); // A3 octave bounce
    a3[2] = step_note(10, 0.98, 1.35); // C4 ACCENT + SLIDE ->
    a3[3] = step_note(8, 0.74, 0.65); // A3
    a3[5] = step_note(5, 0.76, 0.60); // E3
    a3[6] = step_note(7, 0.96, 1.35); // G3 ACCENT + SLIDE ->
    a3[7] = step_note(8, 0.74, 0.65); // A3
    a3[8] = step_note(3, 0.76, 0.60); // C3
    a3[10] = step_note(5, 0.96, 1.35); // E3 ACCENT + SLIDE ->
    a3[11] = step_note(7, 0.74, 0.65); // G3
    a3[12] = step_note(8, 0.76, 0.60); // A3
    a3[13] = step_note(12, 0.98, 1.35); // E4 CLIMAX ACCENT + SLIDE ->
    a3[14] = step_note(10, 0.74, 0.65); // C4
    a3[15] = step_note(8, 0.70, 0.55); // A3
    patterns.push(Pattern {
        id: pat_303_peak_cell,
        name: "303 Overload Cell".into(),
        kind: PatternKind::Notes { steps: a3 },
    });
    let pat_303_peak = ids.next();
    patterns.push(Pattern {
        id: pat_303_peak,
        name: "303 Overload Line".into(),
        kind: PatternKind::Every {
            interval: 2,
            base: pat_303_line,
            alt: pat_303_peak_cell,
        },
    });

    // Final bar-47 resolving acid root accent
    let pat_303_fin = ids.next();
    let mut a_fin = vec![Step::default(); 16];
    a_fin[0] = step_note(1, 0.98, 2.5);
    patterns.push(Pattern {
        id: pat_303_fin,
        name: "303 Final Accent".into(),
        kind: PatternKind::Notes { steps: a_fin },
    });

    // ─── 4h. Electro Stab (Lane 6: Sampler brass-pad.wav -> AddDegrees + Mask!) ───
    let pat_stab_chord = ids.next();
    patterns.push(Pattern {
        id: pat_stab_chord,
        name: "Stab Voicing".into(),
        kind: PatternKind::Notes {
            // Am7 voicing around A3..G4 (MIDI 57..67) — right around brass-pad.wav root C4 (60)!
            steps: vec![chord_step(&[8, 10, 12, 14], 0.86, 1.8); 16],
        },
    });
    let pat_stab_harm = ids.next();
    patterns.push(Pattern {
        id: pat_stab_harm,
        name: "Stab Progression".into(),
        kind: PatternKind::AddDegrees {
            notes: pat_stab_chord,
            values: pat_harmony,
        },
    });
    let pat_stab_rhy_sparse = ids.next();
    patterns.push(Pattern {
        id: pat_stab_rhy_sparse,
        name: "Stab Dub Rhythm".into(),
        kind: PatternKind::Rhythm {
            // Classic dubby electro chord hit on beat 1 and offbeat 11
            steps: rhythm_from_str("x... .... ...x ...."),
        },
    });
    let pat_stab_sparse = ids.next();
    patterns.push(Pattern {
        id: pat_stab_sparse,
        name: "Electro Stab Dub".into(),
        kind: PatternKind::Mask {
            notes: pat_stab_harm,
            rhythm: pat_stab_rhy_sparse,
        },
    });
    let pat_stab_rhy_drive = ids.next();
    patterns.push(Pattern {
        id: pat_stab_rhy_drive,
        name: "Stab Drive Rhythm".into(),
        kind: PatternKind::Rhythm {
            steps: rhythm_from_str("x... ..x. ...x ..x."),
        },
    });
    let pat_stab_drive = ids.next();
    patterns.push(Pattern {
        id: pat_stab_drive,
        name: "Electro Stab Drive".into(),
        kind: PatternKind::Mask {
            notes: pat_stab_harm,
            rhythm: pat_stab_rhy_drive,
        },
    });
    let pat_stab_fin = ids.next();
    let mut s_fin = vec![Step::default(); 16];
    s_fin[0] = chord_step(&[8, 10, 12, 14], 0.88, 3.0);
    patterns.push(Pattern {
        id: pat_stab_fin,
        name: "Electro Stab Fin".into(),
        kind: PatternKind::Notes { steps: s_fin },
    });

    // ─── 4i. Gate Pad (Lane 7: Lush sustained Am9 progression gated by FilterEnv!) ───
    let pat_pad_cell = ids.next();
    let mut pc = vec![Step::default(); 16];
    pc[0] = chord_step(&[8, 10, 12, 14, 16], 0.80, 15.6);
    patterns.push(Pattern {
        id: pat_pad_cell,
        name: "Pad Minor9 Cell".into(),
        kind: PatternKind::Notes { steps: pc },
    });
    let pat_pad_chords = ids.next();
    patterns.push(Pattern {
        id: pat_pad_chords,
        name: "Gate Pad Chords".into(),
        kind: PatternKind::AddDegrees {
            notes: pat_pad_cell,
            values: pat_harmony,
        },
    });

    // ─── 5. Automation & Macro Curves (48 bars = 768 steps, Ramp interpolation!) ───
    // M1 Acid Squelch arc across 48 bars
    let pat_m1_curve = ids.next();
    patterns.push(Pattern {
        id: pat_m1_curve,
        name: "M1 Acid Squelch Curve".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 768,
            mode: ValueMode::Ramp,
            points: vec![
                ValuePoint {
                    step: 0,
                    value: 0.16,
                },
                ValuePoint {
                    step: 192,
                    value: 0.16,
                }, // Bar 13: 303 teaser starts dark
                ValuePoint {
                    step: 254,
                    value: 0.46,
                }, // Bar 16 end: rises into Acid Line
                ValuePoint {
                    step: 256,
                    value: 0.42,
                }, // Bar 17: Acid Line opens
                ValuePoint {
                    step: 368,
                    value: 0.68,
                }, // Bar 24: rising squelch
                ValuePoint {
                    step: 384,
                    value: 0.76,
                }, // Bar 25: Overload kicks in hot!
                ValuePoint {
                    step: 496,
                    value: 0.90,
                }, // Bar 32: peak scream
                ValuePoint {
                    step: 512,
                    value: 0.14,
                }, // Bar 33: Voltage Drop dives deep
                ValuePoint {
                    step: 576,
                    value: 0.20,
                }, // Bar 37: 303 returns dark
                ValuePoint {
                    step: 638,
                    value: 0.84,
                }, // Bar 40 end: huge riser!
                ValuePoint {
                    step: 640,
                    value: 0.88,
                }, // Bar 41: Surge & Out climax!
                ValuePoint {
                    step: 704,
                    value: 0.72,
                }, // Bar 45: final groove coast
                ValuePoint {
                    step: 736,
                    value: 0.50,
                }, // Bar 47: final impact
                ValuePoint {
                    step: 767,
                    value: 0.25,
                },
            ],
        },
    });

    // M2 Fold Grit arc across 48 bars
    let pat_m2_curve = ids.next();
    patterns.push(Pattern {
        id: pat_m2_curve,
        name: "M2 Fold Grit Curve".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 768,
            mode: ValueMode::Ramp,
            points: vec![
                ValuePoint {
                    step: 0,
                    value: 0.22,
                },
                ValuePoint {
                    step: 128,
                    value: 0.42,
                }, // Bar 9: Circuit enters funky
                ValuePoint {
                    step: 256,
                    value: 0.54,
                }, // Bar 17: Acid Line
                ValuePoint {
                    step: 384,
                    value: 0.74,
                }, // Bar 25: Overload snarling fold!
                ValuePoint {
                    step: 510,
                    value: 0.82,
                },
                ValuePoint {
                    step: 512,
                    value: 0.30,
                }, // Bar 33: Voltage Drop rest
                ValuePoint {
                    step: 640,
                    value: 0.78,
                }, // Bar 41: Surge return
                ValuePoint {
                    step: 720,
                    value: 0.48,
                },
                ValuePoint {
                    step: 767,
                    value: 0.32,
                },
            ],
        },
    });

    // M3 Dub Throw turnaround spikes (drops cleanly to 0.0 at step 740 for < -35 dBFS tail!)
    let pat_m3_curve = ids.next();
    patterns.push(Pattern {
        id: pat_m3_curve,
        name: "M3 Dub Throw Curve".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 768,
            mode: ValueMode::Ramp,
            points: vec![
                ValuePoint {
                    step: 0,
                    value: 0.14,
                },
                ValuePoint {
                    step: 112,
                    value: 0.14,
                },
                ValuePoint {
                    step: 120,
                    value: 0.86,
                }, // Bar 8 turnaround throw!
                ValuePoint {
                    step: 128,
                    value: 0.12,
                },
                ValuePoint {
                    step: 240,
                    value: 0.12,
                },
                ValuePoint {
                    step: 248,
                    value: 0.88,
                }, // Bar 16 turnaround throw!
                ValuePoint {
                    step: 256,
                    value: 0.14,
                },
                ValuePoint {
                    step: 368,
                    value: 0.16,
                },
                ValuePoint {
                    step: 376,
                    value: 0.90,
                }, // Bar 24 into Overload!
                ValuePoint {
                    step: 384,
                    value: 0.18,
                },
                ValuePoint {
                    step: 496,
                    value: 0.22,
                },
                ValuePoint {
                    step: 504,
                    value: 0.96,
                }, // Bar 32 huge throw into Voltage Drop!
                ValuePoint {
                    step: 514,
                    value: 0.34,
                }, // Lush breakdown echoes
                ValuePoint {
                    step: 620,
                    value: 0.26,
                },
                ValuePoint {
                    step: 630,
                    value: 0.88,
                }, // Bar 40 throw into Surge!
                ValuePoint {
                    step: 640,
                    value: 0.18,
                },
                ValuePoint {
                    step: 716,
                    value: 0.20,
                },
                ValuePoint {
                    step: 724,
                    value: 0.82,
                }, // Bar 46 final dub throw!
                ValuePoint {
                    step: 734,
                    value: 0.12,
                },
                ValuePoint {
                    step: 740,
                    value: 0.0,
                }, // Clean silence for final tail!
                ValuePoint {
                    step: 767,
                    value: 0.0,
                },
            ],
        },
    });

    // M4 808 Pitch curve (tuned cowbell + conga cutoff modulation, dramatic in Voltage Drop!)
    let pat_m4_curve = ids.next();
    patterns.push(Pattern {
        id: pat_m4_curve,
        name: "M4 808 Pitch Curve".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 768,
            mode: ValueMode::Ramp,
            points: vec![
                ValuePoint {
                    step: 0,
                    value: 0.50,
                },
                ValuePoint {
                    step: 384,
                    value: 0.50,
                },
                ValuePoint {
                    step: 448,
                    value: 0.68,
                }, // Overload bright cowbell lift
                ValuePoint {
                    step: 512,
                    value: 0.26,
                }, // Voltage Drop low hypnotic tuning
                ValuePoint {
                    step: 576,
                    value: 0.78,
                }, // Swells bright through breakdown
                ValuePoint {
                    step: 640,
                    value: 0.52,
                }, // Surge centered punch
                ValuePoint {
                    step: 767,
                    value: 0.50,
                },
            ],
        },
    });

    // M6 Pad Gate curve (swirling FilterEnv depth/cutoff + INVERTED resonance!)
    let pat_m6_curve = ids.next();
    patterns.push(Pattern {
        id: pat_m6_curve,
        name: "M6 Pad Gate Curve".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 768,
            mode: ValueMode::Ramp,
            points: vec![
                ValuePoint {
                    step: 0,
                    value: 0.38,
                },
                ValuePoint {
                    step: 256,
                    value: 0.36,
                }, // Acid Line: tight rhythmic chop
                ValuePoint {
                    step: 384,
                    value: 0.54,
                }, // Overload: brighter pulse
                ValuePoint {
                    step: 512,
                    value: 0.30,
                }, // Voltage Drop starts intimate
                ValuePoint {
                    step: 592,
                    value: 0.88,
                }, // Blooms wide & resonant-smooth!
                ValuePoint {
                    step: 640,
                    value: 0.66,
                }, // Surge & Out driving shimmer
                ValuePoint {
                    step: 736,
                    value: 0.28,
                },
                ValuePoint {
                    step: 767,
                    value: 0.20,
                },
            ],
        },
    });

    // Direct ModulationTarget::Send curve on Acid 303 -> PingPong (phrasing turnarounds!)
    let pat_303_pp_send = ids.next();
    patterns.push(Pattern {
        id: pat_303_pp_send,
        name: "303 PingPong Throws".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 768,
            mode: ValueMode::Ramp,
            points: vec![
                ValuePoint {
                    step: 0,
                    value: 0.14,
                },
                ValuePoint {
                    step: 312,
                    value: 0.16,
                },
                ValuePoint {
                    step: 318,
                    value: 0.68,
                }, // Bar 20 Acid Line turnaround
                ValuePoint {
                    step: 324,
                    value: 0.16,
                },
                ValuePoint {
                    step: 440,
                    value: 0.18,
                },
                ValuePoint {
                    step: 446,
                    value: 0.74,
                }, // Bar 28 Overload throw
                ValuePoint {
                    step: 452,
                    value: 0.20,
                },
                ValuePoint {
                    step: 500,
                    value: 0.22,
                },
                ValuePoint {
                    step: 506,
                    value: 0.82,
                }, // Bar 32 into Voltage Drop!
                ValuePoint {
                    step: 516,
                    value: 0.38,
                },
                ValuePoint {
                    step: 632,
                    value: 0.64,
                }, // Bar 40 riser trail
                ValuePoint {
                    step: 640,
                    value: 0.18,
                },
                ValuePoint {
                    step: 720,
                    value: 0.72,
                }, // Bar 46 final acid echo
                ValuePoint {
                    step: 734,
                    value: 0.10,
                },
                ValuePoint {
                    step: 740,
                    value: 0.0,
                },
                ValuePoint {
                    step: 767,
                    value: 0.0,
                },
            ],
        },
    });

    // Direct continuous per-zone Modulation on TR-909 OH (zone 10) Cutoff!
    let pat_909_oh_cutoff = ids.next();
    patterns.push(Pattern {
        id: pat_909_oh_cutoff,
        name: "909 OH Filter Sweep".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 64,
            mode: ValueMode::Ramp,
            points: vec![
                ValuePoint {
                    step: 0,
                    value: ModuleParam::Cutoff.normalize(6500.0),
                },
                ValuePoint {
                    step: 32,
                    value: ModuleParam::Cutoff.normalize(18_500.0),
                },
                ValuePoint {
                    step: 63,
                    value: ModuleParam::Cutoff.normalize(7200.0),
                },
            ],
        },
    });

    // ─── 6. Parts (Connecting Patterns -> Instruments + ZoneParam P-Locks) ───
    let part_808_kick = ids.next();
    let part_808_kick_end = ids.next();
    let part_808_boot = ids.next();
    let part_808_perc = ids.next();
    let part_808_overload = ids.next();
    let part_808_drop = ids.next();
    let part_909_groove = ids.next();
    let part_909_crash = ids.next();
    let part_909_overload = ids.next();
    let part_909_tick = ids.next();
    let part_909_riser = ids.next();
    let part_909_fin = ids.next();
    let part_fold_teaser = ids.next();
    let part_fold_main = ids.next();
    let part_303_line = ids.next();
    let part_303_peak = ids.next();
    let part_303_fin = ids.next();
    let part_stab_dub = ids.next();
    let part_stab_drive = ids.next();
    let part_stab_fin = ids.next();
    let part_pad = ids.next();

    let zone_links_808 = vec![
        ParameterLink {
            target: InstrumentParam::Zone {
                zone: ZONE_808_LC, // LC
                param: ZoneParam::Pan,
            },
            pattern: pat_808_lc_pan,
        },
        ParameterLink {
            target: InstrumentParam::Zone {
                zone: ZONE_808_HC, // HC
                param: ZoneParam::Pan,
            },
            pattern: pat_808_hc_pan,
        },
        ParameterLink {
            target: InstrumentParam::Module {
                zone: ZONE_808_CP, // CP
                module: KIT_FILTER_MOD,
                param: ModuleParam::Cutoff,
            },
            pattern: pat_808_cp_cutoff,
        },
    ];

    let zone_links_909 = vec![ParameterLink {
        target: InstrumentParam::Zone {
            zone: ZONE_909_OH, // OH
            param: ZoneParam::Pan,
        },
        pattern: pat_909_oh_pan,
    }];

    let parts = vec![
        Part {
            id: part_808_kick,
            name: "808 Kick".into(),
            pattern: pat_808_kick,
            instrument: inst_808_kick,
            transpose: 0,
            velocity_scale: 1.0,
            parameter_links: Vec::new(),
        },
        Part {
            id: part_808_kick_end,
            name: "808 Kick Fin".into(),
            pattern: pat_808_kick_end,
            instrument: inst_808_kick,
            transpose: 0,
            velocity_scale: 0.96,
            parameter_links: Vec::new(),
        },
        Part {
            id: part_808_boot,
            name: "808 Boot Break".into(),
            pattern: pat_808_boot,
            instrument: inst_808,
            transpose: 0,
            velocity_scale: 0.96,
            parameter_links: zone_links_808.clone(),
        },
        Part {
            id: part_808_perc,
            name: "808 Perc Groove".into(),
            pattern: pat_808_full_perc,
            instrument: inst_808,
            transpose: 0,
            velocity_scale: 0.96,
            parameter_links: zone_links_808.clone(),
        },
        Part {
            id: part_808_overload,
            name: "808 Overload".into(),
            pattern: pat_808_overload_perc,
            instrument: inst_808,
            transpose: 0,
            velocity_scale: 1.0,
            parameter_links: zone_links_808.clone(),
        },
        Part {
            id: part_808_drop,
            name: "808 Voltage Drop".into(),
            pattern: pat_808_drop_swung,
            instrument: inst_808,
            transpose: 0,
            velocity_scale: 1.0,
            parameter_links: zone_links_808,
        },
        Part {
            id: part_909_groove,
            name: "909 Groove".into(),
            pattern: pat_909_groove,
            instrument: inst_909,
            transpose: 0,
            velocity_scale: 0.94,
            parameter_links: zone_links_909.clone(),
        },
        Part {
            id: part_909_crash,
            name: "909 Drop Crash".into(),
            pattern: pat_909_drop_hit,
            instrument: inst_909,
            transpose: 0,
            velocity_scale: 0.96,
            parameter_links: zone_links_909.clone(),
        },
        Part {
            id: part_909_overload,
            name: "909 Overload".into(),
            pattern: pat_909_overload,
            instrument: inst_909,
            transpose: 0,
            velocity_scale: 0.98,
            parameter_links: zone_links_909,
        },
        Part {
            id: part_909_tick,
            name: "909 Breakdown Tick".into(),
            pattern: pat_909_tick,
            instrument: inst_909,
            transpose: 0,
            velocity_scale: 0.80,
            parameter_links: Vec::new(),
        },
        Part {
            id: part_909_riser,
            name: "909 Snare Roll".into(),
            pattern: pat_909_riser_ratchet,
            instrument: inst_909,
            transpose: 0,
            velocity_scale: 0.92,
            parameter_links: Vec::new(),
        },
        Part {
            id: part_909_fin,
            name: "909 Crash Fin".into(),
            pattern: pat_909_crash_hit,
            instrument: inst_909,
            transpose: 0,
            velocity_scale: 0.82,
            parameter_links: Vec::new(),
        },
        Part {
            id: part_fold_teaser,
            name: "Fold Bass Teaser".into(),
            pattern: pat_fold_teaser,
            instrument: inst_fold,
            transpose: 0,
            velocity_scale: 0.86,
            parameter_links: Vec::new(),
        },
        Part {
            id: part_fold_main,
            name: "Fold Bass".into(),
            pattern: pat_fold_groove,
            instrument: inst_fold,
            transpose: 0,
            velocity_scale: 0.96,
            parameter_links: Vec::new(),
        },
        Part {
            id: part_303_line,
            name: "Acid 303 Line".into(),
            pattern: pat_303_line,
            instrument: inst_303,
            transpose: 0,
            velocity_scale: 1.0, // Preserves exact >= 0.95 native DiodeLadder accents!
            parameter_links: Vec::new(),
        },
        Part {
            id: part_303_peak,
            name: "Acid 303 Overload".into(),
            pattern: pat_303_peak,
            instrument: inst_303,
            transpose: 0,
            velocity_scale: 1.0,
            parameter_links: Vec::new(),
        },
        Part {
            id: part_303_fin,
            name: "Acid 303 Fin".into(),
            pattern: pat_303_fin,
            instrument: inst_303,
            transpose: 0,
            velocity_scale: 1.0,
            parameter_links: Vec::new(),
        },
        Part {
            id: part_stab_dub,
            name: "Electro Stab Dub".into(),
            pattern: pat_stab_sparse,
            instrument: inst_stab,
            transpose: 0,
            velocity_scale: 0.84,
            parameter_links: Vec::new(),
        },
        Part {
            id: part_stab_drive,
            name: "Electro Stab Drive".into(),
            pattern: pat_stab_drive,
            instrument: inst_stab,
            transpose: 0,
            velocity_scale: 0.90,
            parameter_links: Vec::new(),
        },
        Part {
            id: part_stab_fin,
            name: "Electro Stab Fin".into(),
            pattern: pat_stab_fin,
            instrument: inst_stab,
            transpose: 0,
            velocity_scale: 0.86,
            parameter_links: Vec::new(),
        },
        Part {
            id: part_pad,
            name: "Gate Pad".into(),
            pattern: pat_pad_chords,
            instrument: inst_pad,
            transpose: 0,
            velocity_scale: 0.84,
            parameter_links: Vec::new(),
        },
    ];

    // ─── 7. 48-Bar Arrangement (6 Sections × 8 Bars) ───
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
            name: "808 Kick".into(),
            parts: vec![lp(part_808_kick), lp(part_808_kick_end)],
            placements: vec![
                place(part_808_kick, 0, 32), // Boot, Circuit, Acid Line, Overload
                // Bars 32..40 (Voltage Drop): Kick drops out completely!
                place(part_808_kick, 40, 6), // Surge & Out bars 41..46
                place(part_808_kick_end, 46, 1), // Bar 47 beat 1 crisp final hit!
            ],
        },
        Lane {
            id: ids.next(),
            name: "808 Perc".into(),
            parts: vec![
                lp(part_808_boot),
                lp(part_808_perc),
                lp(part_808_overload),
                lp(part_808_drop),
            ],
            placements: vec![
                place(part_808_boot, 0, 4),      // Bars 1..4: tight 808 electro break
                place(part_808_perc, 4, 20),     // Bars 5..24: congas + snare/clap lock in
                place(part_808_overload, 24, 8), // Bars 25..32: full 808 + cowbell climax
                place(part_808_drop, 32, 8),     // Bars 33..40: hypnotic tuned congas/cowbell
                place(part_808_overload, 40, 6), // Bars 41..46: final surge
            ],
        },
        Lane {
            id: ids.next(),
            name: "909 Kit".into(),
            parts: vec![
                lp(part_909_groove),
                lp(part_909_crash),
                lp(part_909_overload),
                lp(part_909_tick),
                lp(part_909_riser),
                lp(part_909_fin),
            ],
            placements: vec![
                place(part_909_crash, 8, 1),     // Bar 9: Circuit drop crash + groove!
                place(part_909_groove, 9, 7),    // Bars 10..16: crisp 909 hats/snare
                place(part_909_crash, 16, 1),    // Bar 17: Acid Line entry crash
                place(part_909_groove, 17, 7),   // Bars 18..24
                place(part_909_crash, 24, 1),    // Bar 25: Overload drop crash
                place(part_909_groove, 25, 3),   // Bars 26..28
                place(part_909_overload, 28, 4), // Bars 29..32: 909 Ride driving climax!
                place(part_909_tick, 32, 7),     // Bars 33..39: minimal breakdown pulse
                place(part_909_riser, 39, 1),    // Bar 40: snare roll riser!
                place(part_909_crash, 40, 1),    // Bar 41: Surge drop crash
                place(part_909_overload, 41, 3), // Bars 42..44: peak ride surge
                place(part_909_groove, 44, 2),   // Bars 45..46: tight breakdown lead-out
                place(part_909_fin, 46, 1),      // Bar 47 beat 1: crisp crash tail
            ],
        },
        Lane {
            id: ids.next(),
            name: "Fold Bass".into(),
            parts: vec![lp(part_fold_teaser), lp(part_fold_main)],
            placements: vec![
                place(part_fold_teaser, 4, 4), // Bars 5..8: bass teaser
                place(part_fold_main, 8, 24),  // Bars 9..32: full West-Coast funk
                // Voltage Drop (bars 33..40): bass rests!
                place(part_fold_main, 40, 4), // Bars 41..44: final bass surge
            ],
        },
        Lane {
            id: ids.next(),
            name: "Acid 303".into(),
            parts: vec![lp(part_303_line), lp(part_303_peak), lp(part_303_fin)],
            placements: vec![
                place(part_303_line, 12, 12), // Bars 13..16 teaser -> Bars 17..24 Acid Line!
                place(part_303_peak, 24, 8),  // Bars 25..32: Overload screaming acid!
                place(part_303_line, 36, 4),  // Bars 37..40: Voltage Drop rising tension
                place(part_303_peak, 40, 6),  // Bars 41..46: Surge climax
                place(part_303_fin, 46, 1),   // Bar 47 beat 1: final resolving acid hit
            ],
        },
        Lane {
            id: ids.next(),
            name: "Electro Stab".into(),
            parts: vec![lp(part_stab_dub), lp(part_stab_drive), lp(part_stab_fin)],
            placements: vec![
                place(part_stab_dub, 0, 8),    // Bars 1..8: Boot dub stabs
                place(part_stab_drive, 8, 8),  // Bars 9..16: Circuit call-and-response
                place(part_stab_dub, 16, 8),   // Bars 17..24: space for Acid 303
                place(part_stab_drive, 24, 8), // Bars 25..32: Overload full drive
                place(part_stab_dub, 32, 8),   // Bars 33..40: Voltage Drop dub echoes
                place(part_stab_drive, 40, 4), // Bars 41..44: Surge
                place(part_stab_dub, 44, 2),   // Bars 45..46: dub breakdown
                place(part_stab_fin, 46, 1),   // Bar 47 beat 1: final chord stab
            ],
        },
        Lane {
            id: ids.next(),
            name: "Gate Pad".into(),
            parts: vec![lp(part_pad)],
            placements: vec![
                place(part_pad, 16, 28), // Bars 17..44: Acid Line -> Overload -> Voltage Drop -> Surge!
            ],
        },
    ];

    let sections = vec![
        Section {
            id: ids.next(),
            name: "Boot".into(),
            start_step: 0,
            length_steps: 8 * 16,
        },
        Section {
            id: ids.next(),
            name: "Circuit".into(),
            start_step: 8 * 16,
            length_steps: 8 * 16,
        },
        Section {
            id: ids.next(),
            name: "Acid Line".into(),
            start_step: 16 * 16,
            length_steps: 8 * 16,
        },
        Section {
            id: ids.next(),
            name: "Overload".into(),
            start_step: 24 * 16,
            length_steps: 8 * 16,
        },
        Section {
            id: ids.next(),
            name: "Voltage Drop".into(),
            start_step: 32 * 16,
            length_steps: 8 * 16,
        },
        Section {
            id: ids.next(),
            name: "Surge & Out".into(),
            start_step: 40 * 16,
            length_steps: 8 * 16,
        },
    ];

    // ─── 8. Sidechain Ducking (808 Kick -> Synths Group) ───
    let ducks = vec![Duck {
        id: ids.next(),
        name: "808 -> Synths".into(),
        source_part: part_808_kick,
        target: ChannelRef::Channel(ch_synths_grp),
        depth: 0.42,
        recovery_ms: 118.0,
    }];

    // ─── 9. All 8 Assignable Project Macros (M1..M8) ───
    let macros = vec![
        MacroControl {
            index: 0,
            name: "Acid Squelch".into(),
            value: 0.50,
            bindings: vec![
                MacroBinding::new(
                    ParamAddress::Module {
                        instrument: inst_303,
                        zone: 1,
                        module: ACID_FILTER_MOD,
                        param: ModuleParam::Cutoff,
                    },
                    ModuleParam::Cutoff.normalize(190.0),
                    ModuleParam::Cutoff.normalize(1900.0),
                ),
                MacroBinding::new(
                    ParamAddress::Module {
                        instrument: inst_303,
                        zone: 1,
                        module: ACID_FILTER_MOD,
                        param: ModuleParam::Resonance,
                    },
                    ModuleParam::Resonance.normalize(0.56),
                    ModuleParam::Resonance.normalize(0.79),
                ),
                MacroBinding::new(
                    ParamAddress::Effect {
                        device: fx_303_drive,
                        param: EffectParam::Drive,
                    },
                    EffectParam::Drive.normalize(0.18),
                    EffectParam::Drive.normalize(0.54),
                ),
                MacroBinding::new(
                    ParamAddress::Cable {
                        instrument: inst_303,
                        zone: 1,
                        cable: ACID_ENV_MOD_CABLE,
                    },
                    normalize_cable_amount(ModuleKindTag::Filter, InPort::Cv, 1.9),
                    normalize_cable_amount(ModuleKindTag::Filter, InPort::Cv, 3.1),
                ),
            ],
        },
        MacroControl {
            index: 1,
            name: "Fold Grit".into(),
            value: 0.45,
            bindings: vec![
                MacroBinding::new(
                    ParamAddress::Effect {
                        device: fx_fold_drive,
                        param: EffectParam::Drive,
                    },
                    EffectParam::Drive.normalize(0.18),
                    EffectParam::Drive.normalize(0.58),
                ),
                MacroBinding::new(
                    ParamAddress::Effect {
                        device: fx_fold_drive,
                        param: EffectParam::Tone,
                    },
                    EffectParam::Tone.normalize(1500.0),
                    EffectParam::Tone.normalize(4400.0),
                ),
                MacroBinding::new(
                    ParamAddress::Module {
                        instrument: inst_fold,
                        zone: 1,
                        module: FOLD_FILTER_MOD,
                        param: ModuleParam::Cutoff,
                    },
                    ModuleParam::Cutoff.normalize(420.0),
                    ModuleParam::Cutoff.normalize(1450.0),
                ),
            ],
        },
        MacroControl {
            index: 2,
            name: "Dub Throw".into(),
            value: 0.14,
            bindings: vec![
                MacroBinding::new(
                    ParamAddress::Send {
                        channel: ChannelRef::Channel(ch_stab),
                        target: ch_pp_ret,
                    },
                    0.16,
                    0.78,
                ),
                MacroBinding::new(
                    ParamAddress::Send {
                        channel: ChannelRef::Channel(ch_808),
                        target: ch_dub_ret,
                    },
                    0.04,
                    0.56,
                ),
                MacroBinding::new(
                    ParamAddress::Send {
                        channel: ChannelRef::Channel(ch_909),
                        target: ch_dub_ret,
                    },
                    0.02,
                    0.42,
                ),
                MacroBinding::new(
                    ParamAddress::Effect {
                        device: fx_dub_delay,
                        param: EffectParam::Feedback,
                    },
                    EffectParam::Feedback.normalize(0.32),
                    EffectParam::Feedback.normalize(0.60),
                ),
            ],
        },
        MacroControl {
            index: 3,
            name: "808 Pitch".into(),
            value: 0.50,
            bindings: vec![
                MacroBinding::new(
                    ParamAddress::Module {
                        instrument: inst_808,
                        zone: ZONE_808_CB, // CB Cowbell
                        module: KIT_SAMPLER_MOD,
                        param: ModuleParam::CoarseTune,
                    },
                    ModuleParam::CoarseTune.normalize(-4.0),
                    ModuleParam::CoarseTune.normalize(4.0),
                ),
                MacroBinding::new(
                    ParamAddress::Module {
                        instrument: inst_808,
                        zone: ZONE_808_CB, // CB Cowbell
                        module: KIT_FILTER_MOD,
                        param: ModuleParam::Cutoff,
                    },
                    ModuleParam::Cutoff.normalize(2600.0),
                    ModuleParam::Cutoff.normalize(16_000.0),
                ),
                MacroBinding::new(
                    ParamAddress::Module {
                        instrument: inst_808,
                        zone: ZONE_808_MC, // MC Mid Conga
                        module: KIT_FILTER_MOD,
                        param: ModuleParam::Cutoff,
                    },
                    ModuleParam::Cutoff.normalize(1400.0),
                    ModuleParam::Cutoff.normalize(14_000.0),
                ),
                MacroBinding::new(
                    ParamAddress::Module {
                        instrument: inst_808,
                        zone: ZONE_808_LC, // LC Low Conga
                        module: KIT_SAMPLER_MOD,
                        param: ModuleParam::Fine,
                    },
                    ModuleParam::Fine.normalize(-40.0),
                    ModuleParam::Fine.normalize(40.0),
                ),
            ],
        },
        MacroControl {
            index: 4,
            name: "909 Air".into(),
            value: 0.86, // Live-playable sweet spot (~14 kHz crisp 909 sheen)
            bindings: vec![
                MacroBinding::new(
                    ParamAddress::Rack {
                        instrument: inst_909,
                        param: RackParam::Cutoff,
                    },
                    RackParam::Cutoff.normalize(1500.0),
                    RackParam::Cutoff.normalize(20_000.0),
                ),
                MacroBinding::new(
                    ParamAddress::Rack {
                        instrument: inst_909,
                        param: RackParam::Resonance,
                    },
                    RackParam::Resonance.normalize(0.0),
                    RackParam::Resonance.normalize(0.28),
                ),
                MacroBinding::new(
                    ParamAddress::Effect {
                        device: fx_909_eq,
                        param: EffectParam::High,
                    },
                    EffectParam::High.normalize(-3.0),
                    EffectParam::High.normalize(3.2),
                ),
                MacroBinding::new(
                    ParamAddress::Module {
                        instrument: inst_909,
                        zone: ZONE_909_CH, // CH Closed Hat
                        module: KIT_FILTER_MOD,
                        param: ModuleParam::Cutoff,
                    },
                    ModuleParam::Cutoff.normalize(3200.0),
                    ModuleParam::Cutoff.normalize(20_000.0),
                ),
            ],
        },
        MacroControl {
            index: 5,
            name: "Pad Gate".into(),
            value: 0.50,
            bindings: vec![
                MacroBinding::new(
                    ParamAddress::Effect {
                        device: fx_pad_filter,
                        param: EffectParam::Cutoff,
                    },
                    EffectParam::Cutoff.normalize(280.0),
                    EffectParam::Cutoff.normalize(2400.0),
                ),
                MacroBinding::new(
                    ParamAddress::Effect {
                        device: fx_pad_filter,
                        param: EffectParam::EnvAmount,
                    },
                    EffectParam::EnvAmount.normalize(0.9),
                    EffectParam::EnvAmount.normalize(3.3),
                ),
                // Inverted resonance binding (min > max): tightens smoothly as gate blooms open!
                MacroBinding::new(
                    ParamAddress::Effect {
                        device: fx_pad_filter,
                        param: EffectParam::Resonance,
                    },
                    EffectParam::Resonance.normalize(0.60),
                    EffectParam::Resonance.normalize(0.20),
                ),
                MacroBinding::new(
                    ParamAddress::Effect {
                        device: fx_pad_filter,
                        param: EffectParam::EnvDecay,
                    },
                    EffectParam::EnvDecay.normalize(95.0),
                    EffectParam::EnvDecay.normalize(310.0),
                ),
            ],
        },
        MacroControl {
            index: 6,
            name: "Wide Field".into(),
            value: 0.68, // Live-playable stereo field sweet spot
            bindings: vec![
                MacroBinding::new(
                    ParamAddress::Effect {
                        device: fx_stab_chorus,
                        param: EffectParam::Width,
                    },
                    EffectParam::Width.normalize(0.24),
                    EffectParam::Width.normalize(1.0),
                ),
                MacroBinding::new(
                    ParamAddress::Effect {
                        device: fx_pad_chorus,
                        param: EffectParam::Mix,
                    },
                    EffectParam::Mix.normalize(0.16),
                    EffectParam::Mix.normalize(0.66),
                ),
                MacroBinding::new(
                    ParamAddress::Channel {
                        channel: ChannelRef::Channel(ch_stab),
                        param: ChannelParam::Pan,
                    },
                    0.50, // Center
                    0.66, // R32 equal-power pan
                ),
                MacroBinding::new(
                    ParamAddress::Channel {
                        channel: ChannelRef::Channel(ch_pad),
                        param: ChannelParam::Pan,
                    },
                    0.50, // Center
                    0.38, // L24 counter-balanced equal-power pan
                ),
                MacroBinding::new(
                    ParamAddress::Effect {
                        device: fx_room_verb,
                        param: EffectParam::Width,
                    },
                    EffectParam::Width.normalize(0.42),
                    EffectParam::Width.normalize(1.0),
                ),
            ],
        },
        MacroControl {
            index: 7,
            name: "Bus Glue".into(),
            value: 0.56, // Live-playable rhythm bus glue sweet spot
            bindings: vec![
                MacroBinding::new(
                    ParamAddress::Effect {
                        device: fx_perc_comp,
                        param: EffectParam::Mix,
                    },
                    EffectParam::Mix.normalize(0.32),
                    EffectParam::Mix.normalize(1.0),
                ),
                MacroBinding::new(
                    ParamAddress::Effect {
                        device: fx_perc_tape,
                        param: EffectParam::Drive,
                    },
                    EffectParam::Drive.normalize(0.08),
                    EffectParam::Drive.normalize(0.44),
                ),
                // Inverted threshold (more glue as knob increases) paired with makeup gain!
                MacroBinding::new(
                    ParamAddress::Effect {
                        device: fx_rhythm_comp,
                        param: EffectParam::Threshold,
                    },
                    EffectParam::Threshold.normalize(-8.0),
                    EffectParam::Threshold.normalize(-15.0),
                ),
                MacroBinding::new(
                    ParamAddress::Effect {
                        device: fx_rhythm_comp,
                        param: EffectParam::Makeup,
                    },
                    EffectParam::Makeup.normalize(0.0),
                    EffectParam::Makeup.normalize(1.4),
                ),
            ],
        },
    ];

    // ─── 10. Song Modulations (Automated Macros + Direct Send & Per-Zone Curves) ───
    let modulations = vec![
        Modulation {
            id: ids.next(),
            name: "M1 Acid Squelch".into(),
            target: ModulationTarget::Macro { index: 0 },
            pattern: pat_m1_curve,
        },
        Modulation {
            id: ids.next(),
            name: "M2 Fold Grit".into(),
            target: ModulationTarget::Macro { index: 1 },
            pattern: pat_m2_curve,
        },
        Modulation {
            id: ids.next(),
            name: "M3 Dub Throw".into(),
            target: ModulationTarget::Macro { index: 2 },
            pattern: pat_m3_curve,
        },
        Modulation {
            id: ids.next(),
            name: "M4 808 Pitch".into(),
            target: ModulationTarget::Macro { index: 3 },
            pattern: pat_m4_curve,
        },
        Modulation {
            id: ids.next(),
            name: "M6 Pad Gate".into(),
            target: ModulationTarget::Macro { index: 5 },
            pattern: pat_m6_curve,
        },
        Modulation {
            id: ids.next(),
            name: "303 PingPong Dub".into(),
            target: ModulationTarget::Send {
                channel: ChannelRef::Channel(ch_303),
                target: ch_pp_ret,
            },
            pattern: pat_303_pp_send,
        },
        Modulation {
            id: ids.next(),
            name: "909 OH Filter".into(),
            target: ModulationTarget::Instrument {
                instrument: inst_909,
                parameter: InstrumentParam::Module {
                    zone: ZONE_909_OH, // OH
                    module: KIT_FILTER_MOD,
                    param: ModuleParam::Cutoff,
                },
            },
            pattern: pat_909_oh_cutoff,
        },
    ];

    Ok(Project {
        version: VERSION,
        title: "Iron Circuit".into(),
        bpm: 132.0,
        root_midi: ROOT_MIDI,
        scale: SCALE_A_MINOR.to_vec(),
        bars: 48,
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
        end: EndRule::Stop,
    })
}

fn main() -> Result<()> {
    let project = build_iron_circuit()?;
    project.validate()?;

    let out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/iron-circuit");
    std::fs::create_dir_all(&out_dir)?;
    let out_path = out_dir.join("project.json");
    let mut json = Vec::new();
    write_document_json(&mut json, &project, &LiveFlags::default())?;
    std::fs::write(&out_path, &json)?;

    // Verify compilation and all 100% MIT repo-relative sample assets resolve cleanly.
    let compiled = project.compile()?;
    let mut library = SampleLibrary::from_roots(&[], Some(&out_path));
    let samples = load_project_samples(&compiled, &mut library);
    samples.ensure_ready()?;

    let total_fx: usize = project.mixer.master.inserts.len()
        + project
            .mixer
            .channels
            .iter()
            .map(|c| c.inserts.len())
            .sum::<usize>();
    println!(
        "Built {} ({} bytes · {} bars @ {:.0} BPM · {} channels · {} FX · {} instruments · {} patterns · {} macros · {} samples) -> {}",
        project.title,
        json.len(),
        project.bars,
        project.bpm,
        project.mixer.channels.len(),
        total_fx,
        project.instruments.len(),
        project.patterns.len(),
        project.macros.len(),
        samples.bank.len(),
        out_path.display()
    );

    if std::env::args().any(|a| a == "--analyze") {
        let sr = 48_000_u32;
        let total_steps = project.bars as u64 * 16;
        let (rep, pcm) = render_schedule_pcm(&[(0, &compiled)], 0, total_steps, samples.bank, sr)?;
        let peak_db = lin_to_db_f64(rep.output.peak[0].max(rep.output.peak[1]));
        let rms_l = lin_to_db_f64(rep.output.rms[0]);
        let rms_r = lin_to_db_f64(rep.output.rms[1]);
        let tail_db = lin_to_db_f64(rep.tail.peak[0].max(rep.tail.peak[1]));
        let dc_max = rep.output.dc[0].abs().max(rep.output.dc[1].abs());
        println!(
            "48 kHz Full Render: peak={peak_db:+.2} dBFS  RMS L/R={rms_l:+.2}/{rms_r:+.2} dBFS  clips={}  DC={dc_max:.6}  tail={tail_db:+.2} dBFS",
            rep.output.full_scale_samples
        );
        let frames_per_step = (f64::from(sr) * 60.0 / project.bpm) / 4.0;
        for sec in &project.sections {
            let f0 = ((sec.start_step as f64) * frames_per_step).round() as usize;
            let f1 =
                (((sec.start_step + sec.length_steps) as f64) * frames_per_step).round() as usize;
            let slice = &pcm[f0.min(pcm.len())..f1.min(pcm.len())];
            let mut pk = 0.0_f64;
            let mut sq = 0.0_f64;
            for &[li, ri] in slice {
                let l = f64::from(li) / 32768.0;
                let r = f64::from(ri) / 32768.0;
                pk = pk.max(l.abs()).max(r.abs());
                sq += 0.5 * (l * l + r * r);
            }
            let rms = (sq / slice.len().max(1) as f64).sqrt();
            println!(
                "  Section {:14} (bars {:2}..={:2}): peak={:+6.2} dBFS  RMS={:+6.2} dBFS",
                sec.name,
                sec.start_step / 16 + 1,
                (sec.start_step + sec.length_steps) / 16,
                lin_to_db_f64(pk),
                lin_to_db_f64(rms),
            );
        }
    }
    Ok(())
}
