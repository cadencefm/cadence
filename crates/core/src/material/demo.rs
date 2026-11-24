//! Built-in empty and self-contained demo `Project` constructors.

use super::{
    InstrumentInstance, Lane, LanePart, LiveTiming, MixerGraph, Modulation, ModulationTarget,
    ParameterLink, Part, Pattern, PatternKind, Placement, Project, Section, ValuePoint, ValueUnit,
    VERSION,
};
use crate::mixer::{Channel, ChannelKind, ChannelRef, Duck, EffectDevice, MasterChannel};
use crate::music::{EndRule, Step};
use crate::params::{InstrumentParam, ValueMode};
use crate::rack::{ModuleParam, RackPatch};

impl Project {
    pub fn empty() -> Self {
        Self {
            version: VERSION,
            title: "Untitled".into(),
            bpm: 128.0,
            root_midi: 62,
            scale: vec![0, 2, 3, 5, 7, 8, 10],
            bars: 16,
            patterns: Vec::new(),
            instruments: Vec::new(),
            parts: Vec::new(),
            lanes: Vec::new(),
            sections: Vec::new(),
            mixer: MixerGraph {
                master: MasterChannel {
                    strip: crate::mixer::ChannelStrip {
                        gain: 0.72,
                        ..Default::default()
                    },
                    inserts: Vec::new(),
                },
                channels: Vec::new(),
            },
            ducks: Vec::new(),
            modulations: Vec::new(),
            macros: Vec::new(),
            timing: LiveTiming::default(),
            // A new, empty song keeps playing past its end.
            end: EndRule::Continue,
        }
    }

    /// Empty project pre-populated with user preferences (`default_bpm`, `default_root_midi`, `default_scale`).
    pub fn empty_with_config(config: &crate::config::UserConfig) -> Self {
        use crate::config::{
            MAX_DEFAULT_BPM, MAX_DEFAULT_ROOT_MIDI, MIN_DEFAULT_BPM, MIN_DEFAULT_ROOT_MIDI,
        };
        let mut project = Self::empty();
        project.bpm = config.default_bpm.clamp(MIN_DEFAULT_BPM, MAX_DEFAULT_BPM);
        project.root_midi = config
            .default_root_midi
            .clamp(MIN_DEFAULT_ROOT_MIDI, MAX_DEFAULT_ROOT_MIDI);
        if let Some(scale) = crate::music::named_scale(&config.default_scale) {
            project.scale = scale;
        }
        project
    }
    /// Small self-contained construction example. Root demo builders may extend
    /// it, but no sample assets or external plugin are required to play it.
    pub fn demo() -> Self {
        use crate::mixer::{ChannelSend, ChannelStrip};
        let mut project = Self::empty();
        project.title = "Material Sketch".into();
        project.end = EndRule::Loop;
        project.bpm = 136.0;
        let note = |degree: i16| Step {
            active: true,
            degree: degree + 1,
            velocity: 0.78,
            gate: 0.68,
            ..Step::default()
        };
        project.patterns = vec![
            Pattern {
                id: 1,
                name: "Motif".into(),
                kind: PatternKind::Notes {
                    steps: [0, 2, 4, 7, 6].into_iter().map(note).collect(),
                },
            },
            Pattern {
                id: 2,
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
                            step: 24,
                            value: -2.0,
                        },
                        ValuePoint {
                            step: 32,
                            value: 2.0,
                        },
                        ValuePoint {
                            step: 56,
                            value: -1.0,
                        },
                    ],
                },
            },
            Pattern {
                id: 3,
                name: "Offbeats".into(),
                kind: PatternKind::Rhythm {
                    steps: (0..16).map(|i| i % 4 == 2).collect(),
                },
            },
            Pattern {
                id: 4,
                name: "Moving lead".into(),
                kind: PatternKind::AddDegrees {
                    notes: 1,
                    values: 2,
                },
            },
            Pattern {
                id: 5,
                name: "Bass notes".into(),
                kind: PatternKind::NotesFromValues {
                    values: 2,
                    rhythm: 3,
                    degree_offset: -14,
                    velocity: 0.84,
                    gate: 1.1,
                },
            },
            Pattern {
                id: 6,
                name: "Pulse".into(),
                kind: PatternKind::Notes {
                    steps: (0..16)
                        .map(|i| Step {
                            active: i % 4 == 0,
                            velocity: 0.92,
                            ..Step::default()
                        })
                        .collect(),
                },
            },
            Pattern {
                id: 7,
                name: "Energy".into(),
                kind: PatternKind::Values {
                    unit: ValueUnit::Normalized,
                    period: 256,
                    mode: ValueMode::Ramp,
                    points: vec![
                        ValuePoint {
                            step: 0,
                            value: 0.65,
                        },
                        ValuePoint {
                            step: 64,
                            value: 0.8,
                        },
                        ValuePoint {
                            step: 128,
                            value: 1.0,
                        },
                        ValuePoint {
                            step: 240,
                            value: 0.75,
                        },
                    ],
                },
            },
            Pattern {
                id: 8,
                name: "Pan variation".into(),
                kind: PatternKind::SeededValues {
                    unit: ValueUnit::Normalized,
                    period: 16,
                    seed: 771,
                    min: 0.26,
                    max: 0.74,
                },
            },
        ];
        for (id, name, kind) in [
            (1, "Lead", ChannelKind::Audio),
            (2, "Bass", ChannelKind::Audio),
            (3, "Drums", ChannelKind::Audio),
            (4, "Music", ChannelKind::Group),
            (5, "Echo", ChannelKind::Return),
        ] {
            let mut channel = Channel::new(id, kind, name);
            if id == 1 || id == 2 {
                channel.output = ChannelRef::Channel(4);
            }
            if id == 4 {
                channel.strip.gain = 0.8;
                channel.sends.push(ChannelSend {
                    target: 5,
                    amount: 0.24,
                });
            }
            if id == 5 {
                channel.strip.gain = 0.25;
                channel.inserts.push(EffectDevice::delay(1));
            }
            project.mixer.channels.push(channel);
        }
        project.mixer.master.strip = ChannelStrip {
            gain: 0.82,
            ..ChannelStrip::default()
        };
        for (id, name, rack, channel) in [
            (1, "Saw", RackPatch::demo_lead(), 1),
            (2, "Low voice", RackPatch::demo_bass(), 2),
            (3, "Kick", RackPatch::demo_kick(), 3),
        ] {
            project.instruments.push(InstrumentInstance {
                id,
                name: name.into(),
                rack,
                channel,
            });
        }
        project.parts = vec![
            Part::new(1, "Intro", 1, 1),
            Part::new(2, "Moving", 4, 1),
            Part::new(3, "Bass", 5, 2),
            Part::new(4, "Kick", 6, 3),
        ];
        project.parts[1].parameter_links.push(ParameterLink {
            target: InstrumentParam::Module {
                zone: 1,
                module: 9,
                param: ModuleParam::Pan,
            },
            pattern: 8,
        });
        let mut lead = Lane::new(1, "Lead");
        lead.parts = vec![LanePart::new(1), LanePart::new(2)];
        lead.placements = vec![Placement::new(1, 1, 0, 64), Placement::new(2, 2, 64, 192)];
        let mut bass = Lane::new(2, "Bass");
        bass.parts.push(LanePart::new(3));
        bass.placements.push(Placement::new(3, 3, 64, 192));
        let mut drums = Lane::new(3, "Drums");
        drums.parts.push(LanePart::new(4));
        drums.placements.push(Placement::new(4, 4, 64, 192));
        project.lanes = vec![lead, bass, drums];
        project.sections = vec![
            Section {
                id: 1,
                name: "Seed".into(),
                start_step: 0,
                length_steps: 64,
            },
            Section {
                id: 2,
                name: "Motion".into(),
                start_step: 64,
                length_steps: 192,
            },
        ];
        project.modulations.push(Modulation {
            id: 1,
            name: "Music contour".into(),
            target: ModulationTarget::ChannelGain {
                channel: ChannelRef::Channel(4),
            },
            pattern: 7,
        });
        project.ducks.push(Duck {
            id: 1,
            name: "Kick breath".into(),
            source_part: 4,
            target: ChannelRef::Channel(4),
            depth: 0.72,
            recovery_ms: 170.0,
        });
        project
    }
}
