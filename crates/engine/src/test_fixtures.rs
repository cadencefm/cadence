//! Synthetic, self-contained full-engine test fixtures covering every studio
//! effect kind, modular rack primitive, mixer routing topology, PDC alignment
//! path, muted-channel metering path, silent sleep window, and live modulation
//! lane — with zero dependency on `Project::demo()` or bundled `examples/`.

use cadence_core::material::{
    InstrumentInstance, Lane, Modulation, ModulationTarget, ParameterLink, Part, Pattern,
    PatternKind, Placement, Project, ValuePoint, ValueUnit,
};
use cadence_core::mixer::{
    Channel, ChannelKind, ChannelRef, ChannelSend, ChorusPatch, CompressorPatch, DelayMode,
    DelayPatch, DriveMode, DrivePatch, Duck, EffectDevice, EffectKind, EffectKindTag, EffectParam,
    EqPatch, FilterPatch, LimiterPatch, LiveFlags, ReverbPatch,
};
use cadence_core::music::{ChordTone, EndRule, Step};
use cadence_core::params::{InstrumentParam, RackParam, ValueMode};
use cadence_core::rack::{
    drum_midi, module_sample_key, EnvModule, FilterMode, FilterModule, InPort, LfoModule, LfoShape,
    ModuleKind, NoiseColor, NoiseModule, OutModule, PatchCable, PitchTracking, RackModule,
    RackPatch, RackZone, ResonatorMode, ResonatorModule, SamplerModule, TriggerMode, VcaModule,
    VcoModule, VoiceMode, VoiceSettings, Waveform,
};
use cadence_core::runtime::SampleBank;
use cadence_core::sampler::{SampleData, SampleLoopMode, SampleMetadata};
use cadence_core::score::ScoreWindow;
use std::collections::HashSet;
use std::f64::consts::TAU;
use std::path::PathBuf;
use std::sync::Arc;

/// Complete synthetic kitchen-sink fixture ready for engine/mixer/render tests.
pub(crate) struct KitchenSinkFixture {
    pub project: Project,
    pub score: ScoreWindow,
    pub samples: SampleBank,
}

fn synthetic_stereo_sample(frames: usize, sample_rate: u32) -> Arc<SampleData> {
    let sr = f64::from(sample_rate);
    let left: Box<[f32]> = (0..frames)
        .map(|i| {
            let t = i as f64 / sr;
            ((TAU * 220.0 * t).sin() * 0.55 + (TAU * 660.0 * t).sin() * 0.25) as f32
        })
        .collect::<Vec<_>>()
        .into_boxed_slice();
    let right: Box<[f32]> = (0..frames)
        .map(|i| {
            let t = i as f64 / sr;
            ((TAU * 277.18 * t).cos() * 0.5 + (TAU * 554.37 * t).sin() * 0.25) as f32
        })
        .collect::<Vec<_>>()
        .into_boxed_slice();
    Arc::new(SampleData {
        path_label: "./synthetic_pad.wav".into(),
        resolved_path: PathBuf::from("synthetic_pad.wav"),
        name: "synthetic_pad".into(),
        sample_rate,
        channels: 2,
        frames,
        left,
        right: Some(right),
        peaks: vec![(-0.8, 0.8); 64].into_boxed_slice(),
        meta: SampleMetadata::default(),
    })
}

fn hit(degree: i16, velocity: f32, gate: f32) -> Step {
    Step {
        active: true,
        degree,
        velocity,
        gate,
        ..Step::default()
    }
}

fn drum_kit_rack() -> RackPatch {
    let kick_zone = RackZone {
        id: 1,
        name: "Kick".into(),
        low_midi: drum_midi::BD,
        high_midi: drum_midi::BD,
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
                "Body",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Sine,
                    pitch_tracking: PitchTracking::Fixed,
                    fixed_hz: 58.0,
                    level: 0.85,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                2,
                "PitchEnv",
                ModuleKind::Env(EnvModule {
                    attack_ms: 0.5,
                    decay_ms: 35.0,
                    sustain: 0.0,
                    release_ms: 20.0,
                    trigger_mode: TriggerMode::OneShot,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(3, "VCA", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(
                4,
                "AmpEnv",
                ModuleKind::Env(EnvModule {
                    attack_ms: 0.5,
                    decay_ms: 110.0,
                    sustain: 0.0,
                    release_ms: 25.0,
                    trigger_mode: TriggerMode::OneShot,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(5, "Out", ModuleKind::Out(OutModule::default())),
        ],
        cables: vec![
            PatchCable::new(1, 2, 1, InPort::PitchCv, 2.0),
            PatchCable::new(2, 1, 3, InPort::In, 1.0),
            PatchCable::new(3, 4, 3, InPort::Cv, 1.0),
            PatchCable::new(4, 3, 5, InPort::In, 1.0),
        ],
    };

    let open_hat_zone = RackZone {
        id: 2,
        name: "OpenHat".into(),
        low_midi: drum_midi::OH,
        high_midi: drum_midi::OH,
        root_midi: drum_midi::OH,
        low_vel: 0.0,
        high_vel: 1.0,
        gain_db: -3.0,
        pan: 0.25,
        choke_group: Some(1),
        voice: VoiceSettings::default(),
        modules: vec![
            RackModule::new(
                1,
                "Noise",
                ModuleKind::Noise(NoiseModule {
                    color: NoiseColor::White,
                    highpass_hz: 5_500.0,
                    lowpass_hz: 16_000.0,
                    level: 0.6,
                }),
            ),
            RackModule::new(2, "VCA", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(
                3,
                "Env",
                ModuleKind::Env(EnvModule {
                    attack_ms: 0.5,
                    decay_ms: 240.0,
                    sustain: 0.0,
                    release_ms: 40.0,
                    trigger_mode: TriggerMode::OneShot,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(4, "Out", ModuleKind::Out(OutModule::default())),
        ],
        cables: vec![
            PatchCable::new(1, 1, 2, InPort::In, 1.0),
            PatchCable::new(2, 3, 2, InPort::Cv, 1.0),
            PatchCable::new(3, 2, 4, InPort::In, 1.0),
        ],
    };

    let closed_hat_zone = RackZone {
        id: 3,
        name: "ClosedHat".into(),
        low_midi: drum_midi::CH,
        high_midi: drum_midi::CH,
        root_midi: drum_midi::CH,
        low_vel: 0.0,
        high_vel: 1.0,
        gain_db: -4.0,
        pan: -0.2,
        choke_group: Some(1),
        voice: VoiceSettings::default(),
        modules: vec![
            RackModule::new(
                1,
                "Noise",
                ModuleKind::Noise(NoiseModule {
                    color: NoiseColor::Pink,
                    highpass_hz: 6_500.0,
                    lowpass_hz: 15_000.0,
                    level: 0.55,
                }),
            ),
            RackModule::new(2, "VCA", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(
                3,
                "Env",
                ModuleKind::Env(EnvModule {
                    attack_ms: 0.5,
                    decay_ms: 35.0,
                    sustain: 0.0,
                    release_ms: 15.0,
                    trigger_mode: TriggerMode::OneShot,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(4, "Out", ModuleKind::Out(OutModule::default())),
        ],
        cables: vec![
            PatchCable::new(1, 1, 2, InPort::In, 1.0),
            PatchCable::new(2, 3, 2, InPort::Cv, 1.0),
            PatchCable::new(3, 2, 4, InPort::In, 1.0),
        ],
    };

    RackPatch {
        kit_name: "SyntheticKit".into(),
        zones: vec![kick_zone, open_hat_zone, closed_hat_zone],
        gain_db: 0.0,
        pan: 0.0,
        tune_semitones: 0.0,
        cutoff_hz: 20_000.0,
        resonance: 0.0,
        panel_pins: Vec::new(),
    }
}

fn mono_acid_bass_rack() -> RackPatch {
    let mut rack = RackPatch::acid_303();
    rack.zones[0].voice.voice_mode = VoiceMode::Mono;
    rack.zones[0].voice.glide_ms = 24.0;
    rack.zones[0].voice.legato = true;
    rack
}

fn poly_air_rack() -> RackPatch {
    RackPatch {
        kit_name: String::new(),
        zones: vec![RackZone {
            id: 1,
            name: "Air".into(),
            low_midi: 0,
            high_midi: 127,
            root_midi: 60,
            low_vel: 0.0,
            high_vel: 1.0,
            gain_db: -3.0,
            pan: 0.0,
            choke_group: None,
            voice: VoiceSettings {
                voice_mode: VoiceMode::Poly,
                ..VoiceSettings::default()
            },
            modules: vec![
                RackModule::new(
                    1,
                    "VCO",
                    ModuleKind::Vco(VcoModule {
                        waveform: Waveform::MobiusJet,
                        unison_voices: 3,
                        unison_detune_cents: 12.0,
                        unison_width: 0.75,
                        level: 0.55,
                        ..VcoModule::default()
                    }),
                ),
                RackModule::new(
                    2,
                    "LFO",
                    ModuleKind::Lfo(LfoModule {
                        shape: LfoShape::Sine,
                        rate_hz: 2.5,
                        sync: false,
                        retrigger: false,
                        ..LfoModule::default()
                    }),
                ),
                RackModule::new(
                    3,
                    "Filter",
                    ModuleKind::Filter(FilterModule {
                        mode: FilterMode::ImLpf2,
                        cutoff_hz: 2_200.0,
                        resonance: 0.35,
                        ..FilterModule::default()
                    }),
                ),
                RackModule::new(4, "VCA", ModuleKind::Vca(VcaModule::default())),
                RackModule::new(
                    5,
                    "Env",
                    ModuleKind::Env(EnvModule {
                        attack_ms: 8.0,
                        decay_ms: 120.0,
                        sustain: 0.5,
                        release_ms: 90.0,
                        ..EnvModule::default()
                    }),
                ),
                RackModule::new(6, "Out", ModuleKind::Out(OutModule::default())),
            ],
            cables: vec![
                PatchCable::new(1, 1, 3, InPort::In, 1.0),
                PatchCable::new(2, 2, 3, InPort::Cv, 0.4),
                PatchCable::new(3, 3, 4, InPort::In, 1.0),
                PatchCable::new(4, 5, 4, InPort::Cv, 1.0),
                PatchCable::new(5, 4, 6, InPort::In, 1.0),
            ],
        }],
        gain_db: 0.0,
        pan: 0.0,
        tune_semitones: 0.0,
        cutoff_hz: 20_000.0,
        resonance: 0.0,
        panel_pins: Vec::new(),
    }
}

fn sampler_resonator_rack(sample_frames: usize) -> RackPatch {
    RackPatch {
        kit_name: String::new(),
        zones: vec![RackZone {
            id: 1,
            name: "SamplerRes".into(),
            low_midi: 0,
            high_midi: 127,
            root_midi: 60,
            low_vel: 0.0,
            high_vel: 1.0,
            gain_db: -3.0,
            pan: 0.0,
            choke_group: None,
            voice: VoiceSettings::default(),
            modules: vec![
                RackModule::new(
                    1,
                    "Sampler",
                    ModuleKind::Sampler(SamplerModule {
                        path: Some("./synthetic_pad.wav".into()),
                        root_midi: 60,
                        sample_start: 0,
                        sample_end: sample_frames,
                        loop_mode: SampleLoopMode::PingPong,
                        loop_start: 128,
                        loop_end: sample_frames.saturating_sub(128).max(256),
                        one_shot: false,
                        ..SamplerModule::default()
                    }),
                ),
                RackModule::new(2, "VCA", ModuleKind::Vca(VcaModule::default())),
                RackModule::new(
                    3,
                    "Env",
                    ModuleKind::Env(EnvModule {
                        attack_ms: 4.0,
                        decay_ms: 90.0,
                        sustain: 0.35,
                        release_ms: 60.0,
                        ..EnvModule::default()
                    }),
                ),
                RackModule::new(
                    4,
                    "Resonator",
                    ModuleKind::Resonator(ResonatorModule {
                        mode: ResonatorMode::String,
                        decay_ms: 140.0,
                        brightness: 0.6,
                        structure: 0.45,
                        mix: 0.5,
                        ..ResonatorModule::default()
                    }),
                ),
                RackModule::new(5, "Out", ModuleKind::Out(OutModule::default())),
            ],
            cables: vec![
                PatchCable::new(1, 1, 2, InPort::In, 1.0),
                PatchCable::new(2, 3, 2, InPort::Cv, 1.0),
                PatchCable::new(3, 2, 4, InPort::In, 1.0),
                PatchCable::new(4, 4, 5, InPort::In, 1.0),
            ],
        }],
        gain_db: 0.0,
        pan: 0.0,
        tune_semitones: 0.0,
        cutoff_hz: 20_000.0,
        resonance: 0.0,
        panel_pins: Vec::new(),
    }
}

/// Build a 4-bar (`64`-step) synthetic project exercising every studio effect kind,
/// chained Groups (`Group 2 -> Group 1 -> Master`), multi-Return sends, parallel +
/// send + sidechain-duck PDC, muted-channel live input metering, late-entering
/// `Chorus` & clocked `RhythmicFilter` sleeping across bars under live automation,
/// sub-step fractional offsets, chokes, mono legato glide, and P-locks.
pub(crate) fn kitchen_sink_fixture(sample_rate: u32) -> KitchenSinkFixture {
    let mut p = Project::empty();
    p.title = "KitchenSink".into();
    p.bpm = 126.0;
    p.bars = 4;
    p.root_midi = 48;
    p.end = EndRule::Loop;
    p.mixer.master.strip.gain = 0.85;

    // Channel IDs:
    // Audio: 10 (Kick), 11 (Bass), 12 (Air), 13 (Sampler), 14 (MutedFX)
    // Group: 30 (Drums -> Master), 31 (Music -> Drums)
    // Return: 40 (DelayRet -> Master), 41 (VerbRet -> Master)
    let mut ch_kick = Channel::new(10, ChannelKind::Audio, "Kick");
    ch_kick.output = ChannelRef::Channel(30);
    ch_kick.strip.gain = 0.8;
    ch_kick.sends.push(ChannelSend {
        target: 40,
        amount: 0.25,
    });
    ch_kick.inserts.push(EffectDevice {
        id: 101,
        name: "KickComp".into(),
        bypassed: false,
        kind: EffectKind::Compressor(CompressorPatch {
            threshold_db: -18.0,
            ratio: 6.0,
            attack_ms: 2.0,
            release_ms: 45.0,
            makeup_db: 2.0,
            mix: 0.9,
        }),
    });

    let mut ch_bass = Channel::new(11, ChannelKind::Audio, "Bass");
    ch_bass.output = ChannelRef::Channel(30);
    ch_bass.strip.gain = 0.72;
    ch_bass.strip.pan = -0.15;
    ch_bass.inserts.push(EffectDevice {
        id: 102,
        name: "BassDrive".into(),
        bypassed: false,
        kind: EffectKind::Drive(DrivePatch {
            mode: DriveMode::AtomicJet,
            drive: 0.55,
            asymmetry: 0.25,
            tone_hz: 5_200.0,
            output_db: -2.0,
            mix: 0.75,
        }),
    });
    ch_bass.inserts.push(EffectDevice {
        id: 103,
        name: "BassEq".into(),
        bypassed: false,
        kind: EffectKind::Eq(EqPatch {
            hpf_hz: 55.0,
            low_db: 2.5,
            mid_db: -3.0,
            mid_hz: 650.0,
            mid_q: 1.6,
            high_db: 1.5,
        }),
    });
    // Per-channel Limiter forces parallel PDC on Audio 1, Group 2, Sends, and Ducking!
    ch_bass.inserts.push(EffectDevice {
        id: 104,
        name: "BassLim".into(),
        bypassed: false,
        kind: EffectKind::Limiter(LimiterPatch {
            ceiling_db: -3.5,
            input_drive_db: 5.0,
            release_ms: 45.0,
            mix: 1.0,
        }),
    });

    let mut ch_air = Channel::new(12, ChannelKind::Audio, "Air");
    ch_air.output = ChannelRef::Channel(31);
    ch_air.strip.gain = 0.7;
    ch_air.strip.pan = 0.2;
    ch_air.inserts.push(EffectDevice {
        id: 105,
        name: "AirChorus".into(),
        bypassed: false,
        kind: EffectKind::Chorus(ChorusPatch {
            rate_hz: 1.35,
            depth: 0.65,
            width: 0.95,
            mix: 0.65,
        }),
    });
    ch_air.inserts.push(EffectDevice {
        id: 106,
        name: "AirFilter".into(),
        bypassed: false,
        kind: EffectKind::Filter(FilterPatch {
            mode: FilterMode::BandPass,
            cutoff_hz: 700.0,
            resonance: 0.55,
            drive: 0.3,
            kerr_semitones: 0.0,
            spread: 0.0,
            mix: 0.85,
            env_pattern: 0,
            env_pattern_id: Some(200),
            env_amount: 2.6,
            env_decay_ms: 90.0,
        }),
    });

    let mut ch_sampler = Channel::new(13, ChannelKind::Audio, "Sampler");
    ch_sampler.output = ChannelRef::Channel(31);
    ch_sampler.strip.gain = 0.68;
    ch_sampler.sends.push(ChannelSend {
        target: 41,
        amount: 0.35,
    });

    let mut ch_muted = Channel::new(14, ChannelKind::Audio, "MutedFX");
    ch_muted.output = ChannelRef::Master;
    ch_muted.strip.gain = 0.75;
    ch_muted.inserts.push(EffectDevice {
        id: 107,
        name: "MutedDelay".into(),
        bypassed: false,
        kind: EffectKind::Delay(DelayPatch {
            mode: DelayMode::Stereo,
            steps: 1.0,
            feedback: 0.4,
            mix: 0.5,
        }),
    });
    ch_muted.inserts.push(EffectDevice {
        id: 108,
        name: "MutedEq".into(),
        bypassed: false,
        kind: EffectKind::Eq(EqPatch {
            hpf_hz: 110.0,
            low_db: -2.0,
            mid_db: 2.0,
            mid_hz: 1_400.0,
            mid_q: 1.2,
            high_db: 2.0,
        }),
    });

    let mut grp_drums = Channel::new(30, ChannelKind::Group, "DrumsBus");
    grp_drums.output = ChannelRef::Master;
    grp_drums.strip.gain = 0.88;
    grp_drums.inserts.push(EffectDevice {
        id: 109,
        name: "DrumsComp".into(),
        bypassed: false,
        kind: EffectKind::Compressor(CompressorPatch {
            threshold_db: -14.0,
            ratio: 4.0,
            attack_ms: 4.0,
            release_ms: 60.0,
            makeup_db: 1.0,
            mix: 0.8,
        }),
    });

    // Chained group: MusicBus -> DrumsBus -> Master!
    let mut grp_music = Channel::new(31, ChannelKind::Group, "MusicBus");
    grp_music.output = ChannelRef::Channel(30);
    grp_music.strip.gain = 0.85;
    grp_music.sends.push(ChannelSend {
        target: 41,
        amount: 0.25,
    });
    grp_music.inserts.push(EffectDevice {
        id: 110,
        name: "MusicEq".into(),
        bypassed: false,
        kind: EffectKind::Eq(EqPatch {
            hpf_hz: 90.0,
            low_db: -1.5,
            mid_db: 1.5,
            mid_hz: 2_200.0,
            mid_q: 1.0,
            high_db: 2.0,
        }),
    });

    let mut ret_delay = Channel::new(40, ChannelKind::Return, "DelayRet");
    ret_delay.strip.gain = 0.5;
    ret_delay.inserts.push(EffectDevice {
        id: 111,
        name: "RetDelay".into(),
        bypassed: false,
        kind: EffectKind::Delay(DelayPatch {
            mode: DelayMode::PingPong,
            steps: 1.5,
            feedback: 0.5,
            mix: 1.0,
        }),
    });

    let mut ret_verb = Channel::new(41, ChannelKind::Return, "VerbRet");
    ret_verb.strip.gain = 0.45;
    ret_verb.inserts.push(EffectDevice {
        id: 112,
        name: "RetVerb".into(),
        bypassed: false,
        kind: EffectKind::Reverb(ReverbPatch {
            size: 0.5,
            decay_seconds: 0.65,
            damping_hz: 7_200.0,
            diffusion: 0.5,
            mix: 1.0,
            width: 0.9,
        }),
    });
    ret_verb.inserts.push(EffectDevice {
        id: 113,
        name: "RetLim".into(),
        bypassed: false,
        kind: EffectKind::Limiter(LimiterPatch {
            ceiling_db: -4.0,
            input_drive_db: 3.0,
            release_ms: 50.0,
            mix: 1.0,
        }),
    });

    p.mixer.channels = vec![
        ch_kick, ch_bass, ch_air, ch_sampler, ch_muted, grp_drums, grp_music, ret_delay, ret_verb,
    ];
    p.mixer.master.inserts = vec![
        EffectDevice {
            id: 114,
            name: "MasterComp".into(),
            bypassed: false,
            kind: EffectKind::Compressor(CompressorPatch {
                threshold_db: -12.0,
                ratio: 3.0,
                attack_ms: 6.0,
                release_ms: 80.0,
                makeup_db: 1.0,
                mix: 0.75,
            }),
        },
        EffectDevice {
            id: 115,
            name: "MasterLim".into(),
            bypassed: false,
            kind: EffectKind::Limiter(LimiterPatch {
                ceiling_db: -1.0,
                input_drive_db: 4.0,
                release_ms: 60.0,
                mix: 1.0,
            }),
        },
    ];
    let mut flags = LiveFlags::default();
    flags.muted_channels.insert(14);

    // Exhaustive self-check: every EffectKindTag::ALL must exist and be un-bypassed in this rig.
    let present: HashSet<EffectKindTag> = p
        .mixer
        .all_inserts()
        .filter(|d| !d.bypassed)
        .map(|d| d.kind.tag())
        .collect();
    for &tag in EffectKindTag::ALL {
        assert!(
            present.contains(&tag),
            "kitchen_sink_fixture missing active EffectKindTag::{tag:?}"
        );
    }

    // Instruments (IDs 1..=5).
    let sample_frames = (sample_rate as usize / 8).max(1024);
    let sample = synthetic_stereo_sample(sample_frames, sample_rate);
    p.instruments = vec![
        InstrumentInstance {
            id: 1,
            name: "KickKit".into(),
            rack: drum_kit_rack(),
            channel: 10,
        },
        InstrumentInstance {
            id: 2,
            name: "Bass".into(),
            rack: mono_acid_bass_rack(),
            channel: 11,
        },
        InstrumentInstance {
            id: 3,
            name: "Air".into(),
            rack: poly_air_rack(),
            channel: 12,
        },
        InstrumentInstance {
            id: 4,
            name: "Sampler".into(),
            rack: sampler_resonator_rack(sample_frames),
            channel: 13,
        },
        InstrumentInstance {
            id: 5,
            name: "MutedSrc".into(),
            rack: RackPatch::default_synth(),
            channel: 14,
        },
    ];

    // Patterns:
    // 1: Drum kit pattern (Kick + OpenHat choked by ClosedHat) mapped via `Project::midi_to_degree`.
    let (bd_deg, bd_acc) = p.midi_to_degree(drum_midi::BD);
    let (oh_deg, oh_acc) = p.midi_to_degree(drum_midi::OH);
    let (ch_deg, ch_acc) = p.midi_to_degree(drum_midi::CH);
    let mut kit_steps = vec![Step::default(); 16];
    // Kick hits on 0, 4, 8, 12; OpenHat on 2, 10; ClosedHat choking OpenHat on 3, 11.
    for &s in &[0usize, 4, 8, 12] {
        kit_steps[s] = Step {
            active: true,
            degree: bd_deg,
            accidental: bd_acc,
            velocity: 0.95,
            gate: 0.5,
            ..Step::default()
        };
    }
    for &s in &[2usize, 10] {
        kit_steps[s] = Step {
            active: true,
            degree: oh_deg,
            accidental: oh_acc,
            velocity: 0.8,
            gate: 0.5,
            ..Step::default()
        };
    }
    for &s in &[3usize, 11] {
        kit_steps[s] = Step {
            active: true,
            degree: ch_deg,
            accidental: ch_acc,
            velocity: 0.85,
            gate: 0.25,
            ..Step::default()
        };
    }
    // Pattern 2: Mono legato bass with overlapping gates and velocity accents (> 0.85).
    let mut bass_steps = vec![Step::default(); 16];
    bass_steps[0] = hit(1, 0.96, 2.2);
    bass_steps[2] = hit(3, 0.78, 1.5);
    bass_steps[4] = hit(5, 0.94, 2.5);
    bass_steps[7] = hit(4, 0.72, 1.2);
    bass_steps[10] = hit(1, 0.92, 1.8);
    bass_steps[12] = hit(6, 0.82, 2.0);

    // Pattern 3: TimeScaled bass/sub-step variant generating true fractional sub-step offsets (3/2).
    // Pattern 4: Polyphonic chord pattern for Air (enters late on bar 1, sleeps bars 0 & 2, wakes bar 3).
    let mut air_steps = vec![Step::default(); 16];
    let mut chord_hit = hit(8, 0.85, 3.5);
    chord_hit.chord = vec![
        ChordTone {
            degree: 10,
            accidental: 0,
            velocity: 0.8,
            gate: 3.5,
        },
        ChordTone {
            degree: 12,
            accidental: 0,
            velocity: 0.78,
            gate: 3.5,
        },
    ];
    air_steps[0] = chord_hit.clone();
    air_steps[8] = chord_hit;

    // Pattern 5: Sampler pattern (plays bars 0 & 2, sleeps bars 1 & 3).
    let mut smp_steps = vec![Step::default(); 16];
    smp_steps[0] = hit(8, 0.85, 3.0);
    smp_steps[8] = hit(10, 0.8, 2.5);

    // Pattern 6: Muted channel live note pattern.
    let mut muted_steps = vec![Step::default(); 16];
    muted_steps[0] = hit(8, 0.9, 1.0);
    muted_steps[4] = hit(10, 0.85, 1.0);
    muted_steps[8] = hit(12, 0.9, 1.0);
    muted_steps[12] = hit(8, 0.8, 1.0);

    // Pattern 200: Custom rhythmic filter trigger grid bound to AirFilter (`id=106`).
    let filter_rhythm = vec![
        true, false, true, false, false, true, false, true, true, false, false, true, false, true,
        false, false,
    ];

    // Value patterns for P-locks (201) and continuous modulation lanes (202..=207).
    let ramp = |id: u64, name: &str, period: u32, a: f32, b: f32| Pattern {
        id,
        name: name.into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period,
            mode: ValueMode::Ramp,
            points: vec![
                ValuePoint { step: 0, value: a },
                ValuePoint {
                    step: period / 2,
                    value: b,
                },
            ],
        },
    };

    p.patterns = vec![
        Pattern {
            id: 1,
            name: "KitPat".into(),
            kind: PatternKind::Notes { steps: kit_steps },
        },
        Pattern {
            id: 2,
            name: "BassPat".into(),
            kind: PatternKind::Notes { steps: bass_steps },
        },
        Pattern {
            id: 3,
            name: "BassSubStep".into(),
            kind: PatternKind::TimeScale {
                source: 2,
                num: 3,
                den: 2,
            },
        },
        Pattern {
            id: 4,
            name: "AirPat".into(),
            kind: PatternKind::Notes { steps: air_steps },
        },
        Pattern {
            id: 5,
            name: "SamplerPat".into(),
            kind: PatternKind::Notes { steps: smp_steps },
        },
        Pattern {
            id: 6,
            name: "MutedPat".into(),
            kind: PatternKind::Notes { steps: muted_steps },
        },
        Pattern {
            id: 200,
            name: "FilterTrig".into(),
            kind: PatternKind::Rhythm {
                steps: filter_rhythm,
            },
        },
        ramp(201, "BassCutoffLock", 16, 0.25, 0.85),
        ramp(202, "ChorusRateMod", 64, 0.15, 0.85),
        ramp(203, "AirFilterCutoffMod", 64, 0.2, 0.85),
        ramp(204, "DelayFbMod", 64, 0.25, 0.65),
        ramp(205, "VerbDampMod", 64, 0.25, 0.9),
        ramp(206, "BassGainMod", 64, 0.6, 1.0),
        ramp(207, "KickSendMod", 64, 0.15, 0.55),
    ];

    // Parts (IDs 1..=5).
    let mut part_bass = Part::new(2, "BassPart", 3, 2);
    part_bass.parameter_links.push(ParameterLink {
        target: InstrumentParam::Rack {
            param: RackParam::Cutoff,
        },
        pattern: 201,
    });
    p.parts = vec![
        Part::new(1, "KickPart", 1, 1),
        part_bass,
        Part::new(3, "AirPart", 4, 3),
        Part::new(4, "SamplerPart", 5, 4),
        Part::new(5, "MutedPart", 6, 5),
    ];

    // Lanes with deliberate active + silent sleep windows across the 4 bars (0..64):
    // - Kick: bars 0..4 (continuous, drives sidechain ducking on MusicBus)
    // - Bass: bars 0..2 & bar 3 (sleeps across bar 2 = steps 32..47)
    // - Air (Chorus + clocked RhythmicFilter): bars 1 & 3 (sleeps bar 0 = steps 0..15 AND bar 2 = steps 32..47!)
    // - Sampler (PingPong + Resonator): bars 0 & 2 (sleeps bars 1 & 3)
    // - MutedFX: bars 0..4 (muted live input metering + silent insert drain)
    let mut lane_kick = Lane::new(1, "Kick");
    lane_kick.placements.push(Placement::new(1, 1, 0, 64));

    let mut lane_bass = Lane::new(2, "Bass");
    lane_bass.placements.push(Placement::new(2, 2, 0, 32));
    lane_bass.placements.push(Placement::new(3, 2, 48, 16));

    let mut lane_air = Lane::new(3, "Air");
    lane_air.placements.push(Placement::new(4, 3, 16, 16));
    lane_air.placements.push(Placement::new(5, 3, 48, 16));

    let mut lane_sampler = Lane::new(4, "Sampler");
    lane_sampler.placements.push(Placement::new(6, 4, 0, 16));
    lane_sampler.placements.push(Placement::new(7, 4, 32, 16));

    let mut lane_muted = Lane::new(5, "Muted");
    lane_muted.placements.push(Placement::new(8, 5, 0, 64));

    p.lanes = vec![lane_kick, lane_bass, lane_air, lane_sampler, lane_muted];

    // Sidechain duck from Kick Part (1) ducking Group 2 (MusicBus, id=31).
    p.ducks.push(Duck {
        id: 1,
        name: "KickDuckMusic".into(),
        source_part: 1,
        target: ChannelRef::Channel(31),
        depth: 0.6,
        recovery_ms: 140.0,
    });

    // Continuous modulation lanes actively moving across BOTH silent and active bars.
    p.modulations = vec![
        Modulation {
            id: 1,
            name: "ChorusRate".into(),
            target: ModulationTarget::Effect {
                device: 105,
                parameter: EffectParam::Rate,
            },
            pattern: 202,
        },
        Modulation {
            id: 2,
            name: "AirFilterCutoff".into(),
            target: ModulationTarget::Effect {
                device: 106,
                parameter: EffectParam::Cutoff,
            },
            pattern: 203,
        },
        Modulation {
            id: 3,
            name: "DelayFeedback".into(),
            target: ModulationTarget::Effect {
                device: 111,
                parameter: EffectParam::Feedback,
            },
            pattern: 204,
        },
        Modulation {
            id: 4,
            name: "VerbDamping".into(),
            target: ModulationTarget::Effect {
                device: 112,
                parameter: EffectParam::Damping,
            },
            pattern: 205,
        },
        Modulation {
            id: 5,
            name: "BassGain".into(),
            target: ModulationTarget::ChannelGain {
                channel: ChannelRef::Channel(11),
            },
            pattern: 206,
        },
        Modulation {
            id: 6,
            name: "KickDelaySend".into(),
            target: ModulationTarget::Send {
                channel: ChannelRef::Channel(10),
                target: 40,
            },
            pattern: 207,
        },
        Modulation {
            id: 7,
            name: "AirRackCutoff".into(),
            target: ModulationTarget::Instrument {
                instrument: 3,
                parameter: InstrumentParam::Rack {
                    param: RackParam::Cutoff,
                },
            },
            pattern: 203,
        },
    ];

    p.validate()
        .expect("synthetic kitchen_sink_fixture project must validate");
    let score = p
        .compile_with_flags(&flags)
        .expect("synthetic kitchen_sink_fixture must compile");
    for required_midi in [drum_midi::BD, drum_midi::OH, drum_midi::CH] {
        assert!(
            score
                .events
                .iter()
                .any(|ev| ev.inst_idx == 0 && ev.part_id == Some(1) && ev.midi == required_midi),
            "kitchen_sink_fixture drum kit must emit MIDI {required_midi} events"
        );
    }
    let samples = vec![(module_sample_key(4, 1, 1), sample)];
    KitchenSinkFixture {
        project: p,
        score,
        samples,
    }
}

/// Build a live hot-swap variant of [`kitchen_sink_fixture`] (changed BPM, effect
/// bypass toggle, Drive/Filter mode switches, and mute/send flip) for Take & live
/// score replacement parity tests.
pub(crate) fn kitchen_sink_hotswap_variant(base: &Project) -> ScoreWindow {
    let mut next = base.clone();
    next.bpm = 134.0;
    let mut flags = LiveFlags::default();
    flags.muted_channels.insert(13);
    for dev in next.mixer.all_inserts_mut() {
        match dev.id {
            102 => {
                if let EffectKind::Drive(ref mut d) = dev.kind {
                    d.mode = DriveMode::ShockPort;
                    d.drive = 0.72;
                }
            }
            105 => dev.bypassed = true,
            106 => {
                if let EffectKind::Filter(ref mut f) = dev.kind {
                    f.mode = FilterMode::HighPass;
                    f.cutoff_hz = 1_100.0;
                }
            }
            _ => {}
        }
    }
    next.validate()
        .expect("kitchen_sink_hotswap_variant must validate");
    next.compile_with_flags(&flags)
        .expect("kitchen_sink_hotswap_variant must compile")
}
