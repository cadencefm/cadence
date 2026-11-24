//! Choice list builders, default naming, view projection, and shortcut tests for `WorkflowDraft`.
use super::form::{
    is_name, is_picker, unique_name, DraftCandidate, DraftField, DraftFieldView, DraftInput,
    DraftView,
};
use super::plan::WorkflowKind;
use super::workflow::{
    BounceActionChoice, BounceScopeKind, BounceTargetKind, Candidate, Choice, KitSourceChoice,
    ModTargetNode, RecordActionChoice, RefChoice, SamplerSourceChoice, WorkflowDraft,
};
use cadence_core::macros::{MacroControl, MAX_PROJECT_MACROS};
use cadence_core::material::{ModulationTarget, PatternType, PhaseMode, Project, ValueUnit};
use cadence_core::mixer::{ChannelKind, ChannelRef, MAX_INSERTS};
use cadence_core::params::InstrumentParam;
use cadence_core::rack::{RackParam, RackTemplate, ZoneParam};

fn candidate_query_rank(candidate: &Candidate, query: &str) -> u8 {
    let label = candidate.view.label.trim().to_lowercase();
    let preset_id = match &candidate.choice {
        Choice::Preset(Some(id))
        | Choice::SamplerSource(SamplerSourceChoice::Preset(id))
        | Choice::KitSource(KitSourceChoice::Preset(id)) => Some(id.to_lowercase()),
        _ => None,
    };
    if label == query || preset_id.as_deref() == Some(query) {
        return 0;
    }
    if label.starts_with(query) || preset_id.as_deref().is_some_and(|id| id.starts_with(query)) {
        return 1;
    }
    2
}

fn channel_display_name(project: &Project, ch: ChannelRef) -> String {
    match ch {
        ChannelRef::Master => "Master".into(),
        ChannelRef::Channel(id) => project
            .channel(id)
            .map_or_else(|| format!("Missing #{id}"), |c| c.name.clone()),
    }
}

fn push_all_channels(project: &Project, out: &mut Vec<Candidate>) {
    for c in &project.mixer.channels {
        out.push(Candidate {
            view: DraftCandidate {
                id: Some(c.id),
                key: None,
                label: c.name.clone(),
                detail: format!(
                    "{} · {}/{MAX_INSERTS} inserts · #{}",
                    c.kind.label(),
                    c.inserts.len(),
                    c.id
                ),
            },
            choice: Choice::ChannelRef(ChannelRef::Channel(c.id)),
        });
    }
    out.push(Candidate {
        view: DraftCandidate {
            id: None,
            key: None,
            label: "Master".into(),
            detail: format!(
                "Master bus · {}/{MAX_INSERTS} inserts",
                project.mixer.master.inserts.len()
            ),
        },
        choice: Choice::ChannelRef(ChannelRef::Master),
    });
}

impl WorkflowDraft {
    pub(super) fn choices(&self, project: &Project, field: DraftField) -> Vec<Candidate> {
        let mut out = Vec::new();
        let mut existing = |id, label: String, detail: String| {
            out.push(Candidate {
                view: DraftCandidate {
                    id: Some(id),
                    key: None,
                    label,
                    detail,
                },
                choice: Choice::Reference(RefChoice::Existing(id)),
            })
        };
        match field {
            DraftField::Pattern => {
                for p in &project.patterns {
                    if project.pattern_type(p.id).ok() == Some(PatternType::Notes) {
                        existing(
                            p.id,
                            p.name.clone(),
                            format!(
                                "Notes · {} steps · #{}",
                                project.pattern_period(p.id).unwrap_or(0),
                                p.id
                            ),
                        );
                    }
                }
            }
            DraftField::Instrument => {
                for i in &project.instruments {
                    existing(
                        i.id,
                        i.name.clone(),
                        format!(
                            "Rack ({}z) → {} · #{}",
                            i.rack.zones.len(),
                            project
                                .channel(i.channel)
                                .map_or("Missing channel", |c| c.name.as_str()),
                            i.id
                        ),
                    );
                }
            }
            DraftField::Channel => {
                for c in &project.mixer.channels {
                    if c.kind == ChannelKind::Audio {
                        existing(
                            c.id,
                            c.name.clone(),
                            format!(
                                "Audio channel · {} instrument(s) · #{}",
                                project
                                    .instruments
                                    .iter()
                                    .filter(|i| i.channel == c.id)
                                    .count(),
                                c.id
                            ),
                        );
                    }
                }
            }
            DraftField::Part => {
                for b in &project.parts {
                    existing(
                        b.id,
                        b.name.clone(),
                        format!(
                            "{} → {} · #{}",
                            project
                                .pattern(b.pattern)
                                .map_or("Missing", |p| p.name.as_str()),
                            project
                                .instrument(b.instrument)
                                .map_or("Missing", |i| i.name.as_str()),
                            b.id
                        ),
                    );
                }
            }
            DraftField::Lane => {
                for l in &project.lanes {
                    existing(
                        l.id,
                        l.name.clone(),
                        format!(
                            "{} placements · {} parts · #{}",
                            l.placements.len(),
                            l.parts.len(),
                            l.id
                        ),
                    );
                }
            }
            DraftField::InstrumentKind => {
                for (key, label, tmpl, detail) in [
                    (
                        's',
                        "Synth",
                        RackTemplate::Synth,
                        "Init Synth Rack (VCO → Filter → VCA → Out) or Rack presets",
                    ),
                    (
                        'm',
                        "Sampler",
                        RackTemplate::Sampler,
                        "Blank Sampler Rack, Sample Browser, or Rack presets",
                    ),
                    (
                        'd',
                        "Kit / Multi-Zone",
                        RackTemplate::Kit,
                        "Blank multi-zone Kit Rack, sample folder import, or Drum/Kit presets",
                    ),
                    (
                        'u',
                        "Modular",
                        RackTemplate::Modular,
                        "Blank Eurorack Modular canvas (Out module ready to patch)",
                    ),
                ] {
                    out.push(Candidate {
                        view: DraftCandidate {
                            id: None,
                            key: Some(key),
                            label: label.into(),
                            detail: detail.into(),
                        },
                        choice: Choice::Template(tmpl),
                    });
                }
            }
            DraftField::Preset => {
                let default_label = self.rack_template.default_preset_name();
                out.push(Candidate {
                    view: DraftCandidate {
                        id: None,
                        key: None,
                        label: default_label.into(),
                        detail: format!(
                            "init · neutral {} Rack template · A auditions",
                            self.rack_template.label()
                        ),
                    },
                    choice: Choice::Preset(None),
                });
                for preset in &self.rack_presets {
                    let cat = preset.category.as_deref().unwrap_or("Rack");
                    out.push(Candidate {
                        view: DraftCandidate {
                            id: None,
                            key: None,
                            label: preset.name.clone(),
                            detail: format!("{} · {cat} preset · A auditions", preset.id),
                        },
                        choice: Choice::Preset(Some(preset.id.clone())),
                    });
                }
            }
            DraftField::Sample => {
                if let Some((path, label)) = &self.sample {
                    out.push(Candidate {
                        view: DraftCandidate {
                            id: None,
                            key: None,
                            label: label.clone(),
                            detail: format!("Selected sample · {path}"),
                        },
                        choice: Choice::SamplerSource(SamplerSourceChoice::SelectedSample),
                    });
                }
                out.push(Candidate {
                    view: DraftCandidate {
                        id: None,
                        key: Some('s'),
                        label: "Blank Sampler".into(),
                        detail:
                            "Empty neutral Sampler Rack · assign a sample later with B (Sample Browser)"
                                .into(),
                    },
                    choice: Choice::SamplerSource(SamplerSourceChoice::Blank),
                });
                out.push(Candidate {
                    view: DraftCandidate {
                        id: None,
                        key: Some('b'),
                        label: if self.sample.is_some() {
                            "Browse / replace sample file…".into()
                        } else {
                            "Browse sample files…".into()
                        },
                        detail: "Open Sample Browser to audition & choose an audio file".into(),
                    },
                    choice: Choice::SamplerSource(SamplerSourceChoice::Browse),
                });
                for preset in &self.rack_presets {
                    let cat = preset.category.as_deref().unwrap_or("Rack");
                    out.push(Candidate {
                        view: DraftCandidate {
                            id: None,
                            key: None,
                            label: preset.name.clone(),
                            detail: format!("{} · {cat} preset", preset.id),
                        },
                        choice: Choice::SamplerSource(SamplerSourceChoice::Preset(
                            preset.id.clone(),
                        )),
                    });
                }
            }
            DraftField::KitSource => {
                out.push(Candidate {
                    view: DraftCandidate {
                        id: None,
                        key: Some('b'),
                        label: "Blank Kit".into(),
                        detail: "Empty multi-zone Kit Rack · add zones via a (Add Zone) or B (Sample Browser)"
                            .into(),
                    },
                    choice: Choice::KitSource(KitSourceChoice::Blank),
                });
                out.push(Candidate {
                    view: DraftCandidate {
                        id: None,
                        key: Some('f'),
                        label: "Sample folder…".into(),
                        detail: "Import a folder of audio files with RIFF inst/smpl & key splits"
                            .into(),
                    },
                    choice: Choice::KitSource(KitSourceChoice::Folder),
                });
                // Prioritize multi-zone / Kit-categorized presets ahead of single-zone patches.
                for want_kit in [true, false] {
                    for kit in self.rack_presets.iter().filter(|p| p.is_kit == want_kit) {
                        let cat = kit.category.as_deref().unwrap_or("Rack");
                        out.push(Candidate {
                            view: DraftCandidate {
                                id: None,
                                key: None,
                                label: kit.name.clone(),
                                detail: format!("{} · {cat} preset", kit.id),
                            },
                            choice: Choice::KitSource(KitSourceChoice::Preset(kit.id.clone())),
                        });
                    }
                }
            }
            DraftField::Phase => {
                for (key, label, mode, detail) in [
                    (
                        's',
                        "Song",
                        PhaseMode::Song,
                        "Follow song time; keep source cycles across placements",
                    ),
                    (
                        'r',
                        "Restart",
                        PhaseMode::Restart,
                        "Start this occurrence's source clock at its beginning",
                    ),
                ] {
                    out.push(Candidate {
                        view: DraftCandidate {
                            id: None,
                            key: Some(key),
                            label: label.into(),
                            detail: detail.into(),
                        },
                        choice: Choice::Phase(mode),
                    });
                }
            }
            DraftField::ChannelKind => {
                for (key, label, kind, detail) in [
                    (
                        'u',
                        "Audio",
                        ChannelKind::Audio,
                        "Instrument destination channel with sends & inserts",
                    ),
                    (
                        'g',
                        "Group",
                        ChannelKind::Group,
                        "Submix bus (supports Audio → Group and Group → Group DAG)",
                    ),
                    (
                        'r',
                        "Return",
                        ChannelKind::Return,
                        "Parallel FX return bus fed by channel sends → Master",
                    ),
                ] {
                    out.push(Candidate {
                        view: DraftCandidate {
                            id: None,
                            key: Some(key),
                            label: label.into(),
                            detail: detail.into(),
                        },
                        choice: Choice::ChannelKind(kind),
                    });
                }
            }
            DraftField::ChannelOutput => {
                out.push(Candidate {
                    view: DraftCandidate {
                        id: None,
                        key: None,
                        label: "Master".into(),
                        detail: "Route directly to Master bus".into(),
                    },
                    choice: Choice::ChannelRef(ChannelRef::Master),
                });
                for c in &project.mixer.channels {
                    if c.kind == ChannelKind::Group {
                        out.push(Candidate {
                            view: DraftCandidate {
                                id: Some(c.id),
                                key: None,
                                label: c.name.clone(),
                                detail: format!(
                                    "Group bus → {} · #{}",
                                    channel_display_name(project, c.output),
                                    c.id
                                ),
                            },
                            choice: Choice::ChannelRef(ChannelRef::Channel(c.id)),
                        });
                    }
                }
            }
            DraftField::EffectKind => {
                for (key, label, id, detail) in [
                    (
                        'e',
                        "EQ",
                        "eq",
                        "Parametric 3-band EQ + HPF + sweepable mid Q",
                    ),
                    (
                        'f',
                        "Filter",
                        "filter",
                        "Pattern-Controlled Filter (SVF / Acid 303 + rhythmic env)",
                    ),
                    (
                        'c',
                        "Compressor",
                        "comp",
                        "Feed-forward bus compressor with live GR metering",
                    ),
                    (
                        'l',
                        "Limiter",
                        "limiter",
                        "Brickwall lookahead studio peak limiter",
                    ),
                    (
                        'd',
                        "Drive",
                        "drive",
                        "Analog saturation / overdrive + tone LPF",
                    ),
                    ('t', "Delay", "delay", "Tempo-synced stereo ping-pong delay"),
                    ('h', "Chorus", "chorus", "Stereo modulated BBD-style chorus"),
                    ('r', "Reverb", "reverb", "Coprime FDN stereo studio reverb"),
                ] {
                    out.push(Candidate {
                        view: DraftCandidate {
                            id: None,
                            key: Some(key),
                            label: label.into(),
                            detail: detail.into(),
                        },
                        choice: Choice::EffectKind(id),
                    });
                }
            }
            DraftField::EffectChannel | DraftField::DuckTarget => {
                push_all_channels(project, &mut out);
            }
            DraftField::SendSource => {
                for c in &project.mixer.channels {
                    if c.kind != ChannelKind::Return {
                        out.push(Candidate {
                            view: DraftCandidate {
                                id: Some(c.id),
                                key: None,
                                label: c.name.clone(),
                                detail: format!(
                                    "{} · {} sends · #{}",
                                    c.kind.label(),
                                    c.sends.len(),
                                    c.id
                                ),
                            },
                            choice: Choice::ExistingId(c.id),
                        });
                    }
                }
            }
            DraftField::SendTarget => {
                for c in &project.mixer.channels {
                    if c.kind == ChannelKind::Return {
                        let current_send = self
                            .send_source
                            .and_then(|src| project.channel(src))
                            .and_then(|ch| ch.sends.iter().find(|s| s.target == c.id))
                            .map(|s| format!(" · current {:.0}%", s.amount * 100.0))
                            .unwrap_or_default();
                        existing(
                            c.id,
                            c.name.clone(),
                            format!(
                                "Return bus · {} inserts{current_send} · #{}",
                                c.inserts.len(),
                                c.id
                            ),
                        );
                    }
                }
            }
            DraftField::DuckSource => {
                for b in &project.parts {
                    out.push(Candidate {
                        view: DraftCandidate {
                            id: Some(b.id),
                            key: None,
                            label: b.name.clone(),
                            detail: format!(
                                "{} → {} · #{}",
                                project
                                    .pattern(b.pattern)
                                    .map_or("Missing", |p| p.name.as_str()),
                                project
                                    .instrument(b.instrument)
                                    .map_or("Missing", |i| i.name.as_str()),
                                b.id
                            ),
                        },
                        choice: Choice::ExistingId(b.id),
                    });
                }
            }
            DraftField::ModPattern => {
                for p in &project.patterns {
                    if project.pattern_type(p.id).ok()
                        == Some(PatternType::Values(ValueUnit::Normalized))
                    {
                        existing(
                            p.id,
                            p.name.clone(),
                            format!(
                                "Values · {} steps · #{}",
                                project.pattern_period(p.id).unwrap_or(0),
                                p.id
                            ),
                        );
                    }
                }
            }
            DraftField::ModTarget => {
                for inst in &project.instruments {
                    out.push(Candidate {
                        view: DraftCandidate {
                            id: Some(inst.id),
                            key: None,
                            label: inst.name.clone(),
                            detail: format!("Rack ({}z) · #{}", inst.rack.zones.len(), inst.id),
                        },
                        choice: Choice::ModTargetNode(ModTargetNode::Instrument(inst.id)),
                    });
                }
                for ch in &project.mixer.channels {
                    for dev in &ch.inserts {
                        out.push(Candidate {
                            view: DraftCandidate {
                                id: Some(dev.id),
                                key: None,
                                label: format!("{} · {}", ch.name, dev.name),
                                detail: format!(
                                    "Insert FX ({}) · #{}",
                                    dev.kind.short_tag(),
                                    dev.id
                                ),
                            },
                            choice: Choice::ModTargetNode(ModTargetNode::Effect(dev.id)),
                        });
                    }
                }
                for dev in &project.mixer.master.inserts {
                    out.push(Candidate {
                        view: DraftCandidate {
                            id: Some(dev.id),
                            key: None,
                            label: format!("Master · {}", dev.name),
                            detail: format!("Master FX ({}) · #{}", dev.kind.short_tag(), dev.id),
                        },
                        choice: Choice::ModTargetNode(ModTargetNode::Effect(dev.id)),
                    });
                }
                for ch in &project.mixer.channels {
                    out.push(Candidate {
                        view: DraftCandidate {
                            id: Some(ch.id),
                            key: None,
                            label: ch.name.clone(),
                            detail: format!("{} channel · #{}", ch.kind.label(), ch.id),
                        },
                        choice: Choice::ModTargetNode(ModTargetNode::Channel(ChannelRef::Channel(
                            ch.id,
                        ))),
                    });
                }
                out.push(Candidate {
                    view: DraftCandidate {
                        id: None,
                        key: None,
                        label: "Master".into(),
                        detail: "Master output bus".into(),
                    },
                    choice: Choice::ModTargetNode(ModTargetNode::Channel(ChannelRef::Master)),
                });
                for idx in 0..MAX_PROJECT_MACROS {
                    let index = idx as u8;
                    let (label, detail) = match project.macro_control(index) {
                        Some(mac) => (
                            format!("M{} · {}", idx + 1, mac.name),
                            format!("Project Macro · {} bindings", mac.bindings.len()),
                        ),
                        None => (
                            format!("M{} · {}", idx + 1, MacroControl::default_name(index)),
                            "Project Macro · unassigned (auto-creates slot)".into(),
                        ),
                    };
                    out.push(Candidate {
                        view: DraftCandidate {
                            id: None,
                            key: None,
                            label,
                            detail,
                        },
                        choice: Choice::ModTargetNode(ModTargetNode::Macro(index)),
                    });
                }
            }
            DraftField::ModParam => {
                if let Some(node) = self.mod_target_node {
                    match node {
                        ModTargetNode::Instrument(inst_id) => {
                            if let Some(inst) = project.instrument(inst_id) {
                                let rack = &inst.rack;
                                let multi_zone = rack.zones.len() > 1;
                                for p in RackParam::ALL {
                                    out.push(Candidate {
                                        view: DraftCandidate {
                                            id: None,
                                            key: None,
                                            label: format!("Rack · {}", p.label()),
                                            detail: format!("{} · {}", p.module(), p.name()),
                                        },
                                        choice: Choice::ModTarget(ModulationTarget::Instrument {
                                            instrument: inst_id,
                                            parameter: InstrumentParam::Rack { param: p },
                                        }),
                                    });
                                }
                                for zone in &rack.zones {
                                    for zp in ZoneParam::ALL
                                        .iter()
                                        .copied()
                                        .filter(|p| p.is_automatable())
                                    {
                                        let label = if multi_zone {
                                            format!("z{} {} · {}", zone.id, zone.name, zp.label())
                                        } else {
                                            format!("Zone · {}", zp.label())
                                        };
                                        out.push(Candidate {
                                            view: DraftCandidate {
                                                id: None,
                                                key: None,
                                                label,
                                                detail: format!("z{} · {}", zone.id, zp.name()),
                                            },
                                            choice: Choice::ModTarget(
                                                ModulationTarget::Instrument {
                                                    instrument: inst_id,
                                                    parameter: InstrumentParam::Zone {
                                                        zone: zone.id,
                                                        param: zp,
                                                    },
                                                },
                                            ),
                                        });
                                    }
                                    for m in &zone.modules {
                                        for &mp in m.kind.tag().params() {
                                            if !mp.is_automatable() {
                                                continue;
                                            }
                                            let label = if multi_zone {
                                                format!("{}.{} · {}", zone.name, m.name, mp.label())
                                            } else {
                                                format!("{} · {}", m.name, mp.label())
                                            };
                                            out.push(Candidate {
                                                view: DraftCandidate {
                                                    id: None,
                                                    key: None,
                                                    label,
                                                    detail: format!(
                                                        "{} · {}",
                                                        m.kind.tag().short_badge(),
                                                        mp.name()
                                                    ),
                                                },
                                                choice: Choice::ModTarget(
                                                    ModulationTarget::Instrument {
                                                        instrument: inst_id,
                                                        parameter: InstrumentParam::Module {
                                                            zone: zone.id,
                                                            module: m.id,
                                                            param: mp,
                                                        },
                                                    },
                                                ),
                                            });
                                        }
                                    }
                                    for c in &zone.cables {
                                        let from = zone.module(c.from_module).map_or_else(
                                            || format!("m{}", c.from_module),
                                            |m| m.name.clone(),
                                        );
                                        let to = zone.module(c.to_module).map_or_else(
                                            || format!("m{}", c.to_module),
                                            |m| m.name.clone(),
                                        );
                                        out.push(Candidate {
                                            view: DraftCandidate {
                                                id: None,
                                                key: None,
                                                label: format!(
                                                    "Cable c{} · {from} → {to}.{}",
                                                    c.id,
                                                    c.to_port.slug()
                                                ),
                                                detail: format!("z{} attenuverter", zone.id),
                                            },
                                            choice: Choice::ModTarget(
                                                ModulationTarget::Instrument {
                                                    instrument: inst_id,
                                                    parameter: InstrumentParam::Cable {
                                                        zone: zone.id,
                                                        cable: c.id,
                                                    },
                                                },
                                            ),
                                        });
                                    }
                                }
                            }
                        }
                        ModTargetNode::Effect(dev_id) => {
                            if let Some((_, dev)) = project.device(dev_id) {
                                for &ep in dev.params() {
                                    out.push(Candidate {
                                        view: DraftCandidate {
                                            id: None,
                                            key: None,
                                            label: ep.label().into(),
                                            detail: format!("{} · {}", dev.name, ep.name()),
                                        },
                                        choice: Choice::ModTarget(ModulationTarget::Effect {
                                            device: dev_id,
                                            parameter: ep,
                                        }),
                                    });
                                }
                            }
                        }
                        ModTargetNode::Channel(ch) => {
                            out.push(Candidate {
                                view: DraftCandidate {
                                    id: None,
                                    key: None,
                                    label: "Gain".into(),
                                    detail: format!(
                                        "{} channel gain scale",
                                        channel_display_name(project, ch)
                                    ),
                                },
                                choice: Choice::ModTarget(ModulationTarget::ChannelGain {
                                    channel: ch,
                                }),
                            });
                            out.push(Candidate {
                                view: DraftCandidate {
                                    id: None,
                                    key: None,
                                    label: "Pan".into(),
                                    detail: format!(
                                        "{} stereo balance",
                                        channel_display_name(project, ch)
                                    ),
                                },
                                choice: Choice::ModTarget(ModulationTarget::ChannelPan {
                                    channel: ch,
                                }),
                            });
                            if let ChannelRef::Channel(cid) = ch {
                                if project
                                    .channel(cid)
                                    .is_some_and(|c| c.kind != ChannelKind::Return)
                                {
                                    for ret in project
                                        .mixer
                                        .channels
                                        .iter()
                                        .filter(|c| c.kind == ChannelKind::Return)
                                    {
                                        out.push(Candidate {
                                            view: DraftCandidate {
                                                id: Some(ret.id),
                                                key: None,
                                                label: format!("Send → {}", ret.name),
                                                detail: format!(
                                                    "Automate send level to Return #{}",
                                                    ret.id
                                                ),
                                            },
                                            choice: Choice::ModTarget(ModulationTarget::Send {
                                                channel: ch,
                                                target: ret.id,
                                            }),
                                        });
                                    }
                                }
                            }
                        }
                        ModTargetNode::Macro(index) => {
                            out.push(Candidate {
                                view: DraftCandidate {
                                    id: None,
                                    key: None,
                                    label: format!("M{} Value", usize::from(index) + 1),
                                    detail: "Project Macro position (0..100%)".into(),
                                },
                                choice: Choice::ModTarget(ModulationTarget::Macro { index }),
                            });
                        }
                    }
                }
            }
            DraftField::PresetCategory => {
                for (key, cat, detail) in [
                    ('s', "Synth", "Subtractive / FM / polyphonic synth patch"),
                    ('m', "Sampler", "Single-zone sample instrument"),
                    ('d', "Drums", "Single-voice drum or percussion synth"),
                    ('t', "Kit", "Multi-zone drum kit or key-mapped instrument"),
                    ('u', "Modular", "Complex custom Eurorack patch"),
                ] {
                    out.push(Candidate {
                        view: DraftCandidate {
                            id: None,
                            key: Some(key),
                            label: cat.into(),
                            detail: detail.into(),
                        },
                        choice: Choice::PresetCategory(cat),
                    });
                }
            }
            DraftField::BounceAction => {
                let scope_desc = self.bounce_scope_summary(project);
                let target_desc = self.bounce_target_summary(project);
                for (key, act, label, detail) in [
                    (
                        'b',
                        BounceActionChoice::BounceNow,
                        "Bounce now",
                        format!("{scope_desc} → {target_desc}"),
                    ),
                    (
                        's',
                        BounceActionChoice::EditScope,
                        "Change scope",
                        format!("Current: {scope_desc} (Part / Pattern / Range)"),
                    ),
                    (
                        'r',
                        BounceActionChoice::EditRange,
                        "Edit bar range",
                        format!("Current range: bars {}", self.bounce_range),
                    ),
                    (
                        't',
                        BounceActionChoice::EditTarget,
                        "Change target",
                        format!("Current: {target_desc} (New Sampler / Replace Zone / New Zone)"),
                    ),
                    (
                        'n',
                        BounceActionChoice::EditName,
                        "Edit name",
                        format!(
                            "Current: {}",
                            if self.name.trim().is_empty() {
                                "Auto"
                            } else {
                                self.name.trim()
                            }
                        ),
                    ),
                    (
                        'p',
                        BounceActionChoice::EditCreatePart,
                        "Trigger part (--part)",
                        format!(
                            "Current: {}",
                            if self.bounce_create_part { "Yes" } else { "No" }
                        ),
                    ),
                ] {
                    out.push(Candidate {
                        view: DraftCandidate {
                            id: None,
                            key: Some(key),
                            label: label.into(),
                            detail,
                        },
                        choice: Choice::BounceAction(act),
                    });
                }
            }
            DraftField::BounceScope => {
                for (key, kind, label, detail) in [
                    (
                        'r',
                        BounceScopeKind::Range,
                        "Bar range",
                        format!("Render arrangement bars ({})", self.bounce_range),
                    ),
                    (
                        'p',
                        BounceScopeKind::Part,
                        "Part",
                        "Render one isolated Part through its instrument & FX".to_string(),
                    ),
                    (
                        'n',
                        BounceScopeKind::Pattern,
                        "Pattern",
                        "Render one Notes Pattern through the active instrument".to_string(),
                    ),
                ] {
                    if kind == BounceScopeKind::Part && project.parts.is_empty() {
                        continue;
                    }
                    if kind == BounceScopeKind::Pattern
                        && !project
                            .patterns
                            .iter()
                            .any(|p| project.pattern_type(p.id).ok() == Some(PatternType::Notes))
                    {
                        continue;
                    }
                    out.push(Candidate {
                        view: DraftCandidate {
                            id: None,
                            key: Some(key),
                            label: label.into(),
                            detail,
                        },
                        choice: Choice::BounceScopeKind(kind),
                    });
                }
            }
            DraftField::BouncePart => {
                for b in &project.parts {
                    out.push(Candidate {
                        view: DraftCandidate {
                            id: Some(b.id),
                            key: None,
                            label: b.name.clone(),
                            detail: format!(
                                "{} → {} · #{}",
                                project
                                    .pattern(b.pattern)
                                    .map_or("Missing", |p| p.name.as_str()),
                                project
                                    .instrument(b.instrument)
                                    .map_or("Missing", |i| i.name.as_str()),
                                b.id
                            ),
                        },
                        choice: Choice::ExistingId(b.id),
                    });
                }
            }
            DraftField::BouncePattern => {
                for p in &project.patterns {
                    if project.pattern_type(p.id).ok() == Some(PatternType::Notes) {
                        out.push(Candidate {
                            view: DraftCandidate {
                                id: Some(p.id),
                                key: None,
                                label: p.name.clone(),
                                detail: format!(
                                    "Notes · {} steps · #{}",
                                    project.pattern_period(p.id).unwrap_or(0),
                                    p.id
                                ),
                            },
                            choice: Choice::ExistingId(p.id),
                        });
                    }
                }
            }
            DraftField::BounceTarget => {
                for (key, kind, label, detail) in [
                    (
                        's',
                        BounceTargetKind::NewSampler,
                        "New Sampler",
                        "Create a brand-new Sampler Rack instrument + Audio channel",
                    ),
                    (
                        'z',
                        BounceTargetKind::ReplaceZone,
                        "Replace Zone",
                        "Load bounced WAV directly into an existing Rack zone",
                    ),
                    (
                        'n',
                        BounceTargetKind::NewZoneMidi,
                        "New Zone + MIDI",
                        "Append a brand-new zone at a chosen MIDI note on a Rack",
                    ),
                ] {
                    if kind != BounceTargetKind::NewSampler && project.instruments.is_empty() {
                        continue;
                    }
                    out.push(Candidate {
                        view: DraftCandidate {
                            id: None,
                            key: Some(key),
                            label: label.into(),
                            detail: detail.into(),
                        },
                        choice: Choice::BounceTargetKind(kind),
                    });
                }
            }
            DraftField::BounceZoneInstrument => {
                for inst in &project.instruments {
                    out.push(Candidate {
                        view: DraftCandidate {
                            id: Some(inst.id),
                            key: None,
                            label: inst.name.clone(),
                            detail: format!("Rack ({}z) · #{}", inst.rack.zones.len(), inst.id),
                        },
                        choice: Choice::ExistingId(inst.id),
                    });
                }
            }
            DraftField::BounceZone => {
                if let Some(inst) = self
                    .bounce_zone_instrument
                    .and_then(|id| project.instrument(id))
                {
                    for (idx, z) in inst.rack.zones.iter().enumerate() {
                        out.push(Candidate {
                            view: DraftCandidate {
                                id: None,
                                key: None,
                                label: format!("z{} · {}", z.id, z.name),
                                detail: format!(
                                    "{}..{} (root {})",
                                    cadence_core::music::note_name(z.low_midi),
                                    cadence_core::music::note_name(z.high_midi),
                                    cadence_core::music::note_name(z.root_midi)
                                ),
                            },
                            choice: Choice::BounceZoneIndex(idx),
                        });
                    }
                }
            }
            DraftField::BounceCreatePart => {
                let (no_detail, yes_label, yes_detail) = if self.kind == WorkflowKind::Record {
                    (
                        "Create Sampler Rack only (no Arrangement placement)",
                        "Yes (Arrange)",
                        "Create Sampler + trigger Part + arranged Placement at punch-in",
                    )
                } else {
                    (
                        "Bounce WAV into target only",
                        "Yes (--part)",
                        "Also create a trigger Notes pattern, Part, and placement",
                    )
                };
                for (key, val, label, detail) in [
                    ('n', false, "No", no_detail),
                    ('y', true, yes_label, yes_detail),
                ] {
                    out.push(Candidate {
                        view: DraftCandidate {
                            id: None,
                            key: Some(key),
                            label: label.into(),
                            detail: detail.into(),
                        },
                        choice: Choice::Bool(val),
                    });
                }
            }
            DraftField::RecordAction => {
                let target_desc = self.bounce_target_summary(project);
                let bars_desc = if self.record_mode == crate::recording::RecordingMode::Free {
                    "manual stop".into()
                } else if self.record_bars.trim().is_empty() {
                    "open / loop".into()
                } else {
                    format!("{} bars", self.record_bars.trim())
                };
                for (key, act, label, detail) in [
                    (
                        'r',
                        RecordActionChoice::RecordNow,
                        "Record now",
                        format!("{} ({bars_desc}) → {target_desc}", self.record_mode.label()),
                    ),
                    (
                        'm',
                        RecordActionChoice::EditMode,
                        "Timing mode",
                        format!(
                            "Current: {} (Quantized / 1-Bar Count-in / Free)",
                            self.record_mode.label()
                        ),
                    ),
                    (
                        'b',
                        RecordActionChoice::EditBars,
                        "Length (bars)",
                        format!("Current: {bars_desc} (1..=64 or blank for open/loop)"),
                    ),
                    (
                        't',
                        RecordActionChoice::EditTarget,
                        "Change target",
                        format!("Current: {target_desc} (New Sampler / Replace Zone / New Zone)"),
                    ),
                    (
                        'n',
                        RecordActionChoice::EditName,
                        "Edit name",
                        format!(
                            "Current: {}",
                            if self.name.trim().is_empty() {
                                "Auto"
                            } else {
                                self.name.trim()
                            }
                        ),
                    ),
                    (
                        'p',
                        RecordActionChoice::EditArrange,
                        "Place in Arrange",
                        format!(
                            "Current: {}",
                            if self.bounce_create_part { "Yes" } else { "No" }
                        ),
                    ),
                ] {
                    out.push(Candidate {
                        view: DraftCandidate {
                            id: None,
                            key: Some(key),
                            label: label.into(),
                            detail,
                        },
                        choice: Choice::RecordAction(act),
                    });
                }
            }
            DraftField::RecordMode => {
                for (key, mode, detail) in [
                    (
                        'q',
                        crate::recording::RecordingMode::Quantized,
                        "Start transport & punch in on bar/loop boundary",
                    ),
                    (
                        'c',
                        crate::recording::RecordingMode::CountIn,
                        "1-bar count-in before punching in on the beat",
                    ),
                    (
                        'f',
                        crate::recording::RecordingMode::Free,
                        "Record immediately without starting transport",
                    ),
                ] {
                    out.push(Candidate {
                        view: DraftCandidate {
                            id: None,
                            key: Some(key),
                            label: mode.label().into(),
                            detail: detail.into(),
                        },
                        choice: Choice::RecordMode(mode),
                    });
                }
            }
            _ => {}
        }
        let new = match field {
            DraftField::Pattern => Some((
                "New note pattern",
                "Blank literal Notes; enter its notes after creation",
            )),
            DraftField::Instrument => Some((
                "New instrument",
                "Stage a Modular Rack (Synth, Sampler, Kit, or Modular) with an Audio destination",
            )),
            DraftField::Channel => Some((
                "New Audio channel",
                "Independent channel, initially routed to Master",
            )),
            DraftField::SendTarget => Some((
                "New Return channel",
                "Parallel FX return routed to Master; add reverb/delay after",
            )),
            DraftField::Part => Some(("New part", "Choose material and an existing or new sound")),
            DraftField::Lane => Some(("New lane", "Independent arrangement lane")),
            DraftField::ModPattern => Some((
                "New Values pattern",
                "16-step normalized Ramp curve ready for editing",
            )),
            _ => None,
        };
        if let Some((label, detail)) = new {
            out.push(Candidate {
                view: DraftCandidate {
                    id: None,
                    key: Some('n'),
                    label: format!("+ {label}"),
                    detail: format!("{detail} · staged until acceptance"),
                },
                choice: Choice::Reference(RefChoice::New),
            });
        }
        if field == self.field && !self.query.trim().is_empty() {
            let trimmed = self.query.trim().to_lowercase();
            let tokens: Vec<_> = trimmed.split_whitespace().map(str::to_string).collect();
            out.retain(|c| {
                let text = format!("{} {}", c.view.label, c.view.detail).to_lowercase();
                tokens.iter().all(|t| text.contains(t))
            });
            // Stable rank: exact preset ID / exact label first, then prefix matches,
            // then remaining substring matches (preserving category priority on ties).
            out.sort_by_key(|c| candidate_query_rank(c, &trimmed));
        }
        out
    }

    pub(super) fn name_default(&self, project: &Project, field: DraftField) -> String {
        let base = self.part_name(project);
        match field {
            DraftField::PatternName => {
                if matches!(
                    self.kind,
                    WorkflowKind::Modulation | WorkflowKind::EditModulation(_)
                ) {
                    return unique_name(
                        &format!("{} Curve", self.default_modulation_name(project)),
                        project.patterns.iter().map(|p| p.name.as_str()),
                        120,
                    );
                }
                unique_name(
                    &format!("{base} motif"),
                    project.patterns.iter().map(|p| p.name.as_str()),
                    120,
                )
            }
            DraftField::InstrumentName => unique_name(
                &format!("{base} sound"),
                project.instruments.iter().map(|i| i.name.as_str()),
                120,
            ),
            DraftField::ChannelName => {
                if self.kind == WorkflowKind::Send {
                    return unique_name(
                        "Return",
                        project.mixer.channels.iter().map(|c| c.name.as_str()),
                        80,
                    );
                }
                let instrument_name = self.instrument_name(project);
                unique_name(
                    if self.kind == WorkflowKind::PlacedPart {
                        &self.name
                    } else {
                        &instrument_name
                    },
                    project.mixer.channels.iter().map(|c| c.name.as_str()),
                    80,
                )
            }
            DraftField::PartName => unique_name(
                &self.name,
                project.parts.iter().map(|b| b.name.as_str()),
                120,
            ),
            DraftField::LaneName => {
                unique_name(&base, project.lanes.iter().map(|l| l.name.as_str()), 120)
            }
            _ => self.name.clone(),
        }
    }

    pub(super) fn part_name(&self, project: &Project) -> String {
        if self.kind == WorkflowKind::Place {
            if self.part_name.is_empty() {
                unique_name(
                    &self.name,
                    project.parts.iter().map(|b| b.name.as_str()),
                    120,
                )
            } else {
                self.part_name.clone()
            }
        } else {
            self.name.clone()
        }
    }

    pub(super) fn instrument_name(&self, project: &Project) -> String {
        if self.kind == WorkflowKind::Instrument {
            self.name.clone()
        } else if self.instrument_name.is_empty() {
            unique_name(
                &format!("{} sound", self.part_name(project)),
                project.instruments.iter().map(|i| i.name.as_str()),
                120,
            )
        } else {
            self.instrument_name.clone()
        }
    }

    pub(super) fn named_text(&self, project: &Project, field: DraftField) -> String {
        let value = match field {
            DraftField::PatternName => &self.pattern_name,
            DraftField::InstrumentName => &self.instrument_name,
            DraftField::ChannelName => &self.channel_name,
            DraftField::PartName => &self.part_name,
            DraftField::LaneName => &self.lane_name,
            _ => &self.name,
        };
        if value.is_empty() {
            self.name_default(project, field)
        } else {
            value.clone()
        }
    }

    pub fn view(&self, project: &Project) -> DraftView {
        let choices = self.choices(project, self.field);
        let current_choice = self.selected_choice(self.field);
        let selected = choices
            .iter()
            .position(|c| Some(&c.choice) == current_choice.as_ref());
        let mut summary = match self.kind {
            WorkflowKind::Instrument => format!(
                "{} → {} · new instrument, one transaction",
                self.instrument_name(project),
                self.field_value(project, DraftField::Channel)
            ),
            WorkflowKind::Part => format!(
                "{} → {} · transpose {} · vel {}",
                self.field_value(project, DraftField::Pattern),
                self.field_value(project, DraftField::Instrument),
                self.part_transpose,
                self.part_velocity,
            ),
            WorkflowKind::RenamePart(_) => {
                format!(
                    "Rename part to '{}' (preserves placements, bank slots & ID)",
                    self.name
                )
            }
            WorkflowKind::Lane => {
                "An arrangement lane owns placements, not an instrument or pattern".into()
            }
            WorkflowKind::RenameLane(_) => {
                format!(
                    "Rename lane to '{}' (preserves placements, bank slots & ID)",
                    self.name
                )
            }
            WorkflowKind::EditLanePart { lane, part } => {
                let l_name = project.lane(lane).map_or("Lane", |l| l.name.as_str());
                let p_name = project.part(part).map_or("Part", |p| p.name.as_str());
                format!(
                    "Launchpad '{p_name}' on '{l_name}' · {:?} {:+} steps",
                    self.phase,
                    self.offset.trim().parse::<i32>().unwrap_or(0)
                )
            }
            WorkflowKind::FreezePattern(id) => {
                let src = project.pattern(id).map_or("Pattern", |p| p.name.as_str());
                format!(
                    "Freeze '{src}' → literal '{}' ({} steps from {})",
                    self.name, self.pattern_length, self.start
                )
            }
            WorkflowKind::Place | WorkflowKind::PlacedPart => format!(
                "{} → {} · start {} · length {} · {:?} {:+} steps",
                self.field_value(project, DraftField::Part),
                self.field_value(project, DraftField::Lane),
                self.start,
                self.duration,
                self.phase,
                self.offset.trim().parse::<i32>().unwrap_or(0)
            ),
            WorkflowKind::Section | WorkflowKind::EditSection(_) => format!(
                "Section {} · start {} · length {} · jump with 1–0 / Alt+1–0",
                self.name, self.start, self.duration
            ),
            WorkflowKind::Channel => format!(
                "{} channel '{}' → {}",
                self.channel_kind.label(),
                self.name,
                if self.channel_kind == ChannelKind::Return {
                    "Master".into()
                } else {
                    channel_display_name(project, self.channel_output)
                }
            ),
            WorkflowKind::RenameInstrument(_) => {
                format!(
                    "Rename instrument to '{}' (preserves parts, routing & ID)",
                    self.name
                )
            }
            WorkflowKind::RenameChannel(_) => {
                format!(
                    "Rename mixer channel to '{}' (preserves routing & ID)",
                    self.name
                )
            }
            WorkflowKind::RenameEffect(_) => {
                format!(
                    "Rename effect device to '{}' (preserves parameters, modulations & ID)",
                    self.name
                )
            }
            WorkflowKind::RenameMacro(index) => {
                format!(
                    "Rename M{} to '{}' (preserves bindings & automation)",
                    usize::from(index) + 1,
                    self.name
                )
            }
            WorkflowKind::Effect => format!(
                "Insert {} onto {}",
                self.effect_kind,
                channel_display_name(project, self.effect_channel)
            ),
            WorkflowKind::MoveEffect(dev_id) => {
                let dev_name = project
                    .device(dev_id)
                    .map_or_else(|| format!("Device #{dev_id}"), |(_, d)| d.name.clone());
                format!(
                    "Move {dev_name} → {} slot {}",
                    channel_display_name(project, self.effect_channel),
                    self.effect_slot
                )
            }
            WorkflowKind::Send => format!(
                "{} → {} @ {}",
                self.field_value(project, DraftField::SendSource),
                self.field_value(project, DraftField::SendTarget),
                self.send_amount
            ),
            WorkflowKind::Duck | WorkflowKind::EditDuck(_) => format!(
                "Duck '{}' · {} → {} · {} / {}",
                self.name,
                self.field_value(project, DraftField::DuckSource),
                channel_display_name(project, self.duck_target),
                self.duck_depth,
                self.duck_recovery
            ),
            WorkflowKind::Modulation | WorkflowKind::EditModulation(_) => format!(
                "Mod '{}' · {} → {} ({})",
                self.named_text(project, DraftField::Name),
                self.field_value(project, DraftField::ModPattern),
                self.field_value(project, DraftField::ModTarget),
                self.field_value(project, DraftField::ModParam),
            ),
            WorkflowKind::ZoneImportFolder(id) => format!(
                "Import audio folder into {} zones · {}",
                project.instrument(id).map_or("Rack", |i| i.name.as_str()),
                self.field_value(project, DraftField::SampleFolder)
            ),
            WorkflowKind::SavePreset(id) => format!(
                "Save '{}' ({}) preset from {}",
                self.name.trim(),
                self.preset_category,
                project.instrument(id).map_or("Rack", |i| i.name.as_str()),
            ),
            WorkflowKind::Bounce => format!(
                "Bounce {} → {}{}",
                self.bounce_scope_summary(project),
                self.bounce_target_summary(project),
                if self.bounce_create_part {
                    " · +trigger part"
                } else {
                    ""
                }
            ),
            WorkflowKind::Record => format!(
                "Record {} → {}{}",
                self.record_mode.label(),
                self.bounce_target_summary(project),
                if self.bounce_create_part {
                    " · +Arrange clip"
                } else {
                    ""
                }
            ),
        };
        if self.field == DraftField::Sample {
            summary = format!(
                "Assign selected sample to {} only when this complete draft is accepted",
                self.instrument_name(project)
            );
        }
        let preview = if let Some(RefChoice::Existing(id)) = self.pattern {
            project
                .pattern(id)
                .map(|p| {
                    vec![format!(
                        "Material: {} · {} steps",
                        p.name,
                        project.pattern_period(id).unwrap_or(0)
                    )]
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let esc_label = if !self.query.is_empty() {
            "clear filter"
        } else {
            "cancel"
        };
        let hints = if self.searching {
            vec![
                ("type", "filter"),
                ("↑/↓", "choose"),
                ("Tab", "keep filter"),
                ("Enter", "apply filter"),
                ("Ctrl+u", "clear"),
                ("Shift+Tab", "back"),
                ("Esc", "clear filter"),
            ]
        } else if self.field == DraftField::Sample {
            vec![
                ("j/k", "choose"),
                ("/", "filter"),
                ("b", "browse files"),
                ("s", "blank sampler"),
                ("Enter", "accept"),
                ("Shift+Tab", "back"),
                ("Esc", esc_label),
            ]
        } else if self.field == DraftField::Preset {
            vec![
                ("j/k", "preset"),
                ("/", "filter"),
                ("A", "audition"),
                ("Enter", "accept"),
                ("Shift+Tab", "back"),
                ("Esc", esc_label),
            ]
        } else if matches!(self.field, DraftField::ChannelKind | DraftField::EffectKind) {
            vec![
                ("shortcut", "choose"),
                ("j/k", "select"),
                ("Enter", "accept"),
                ("Shift+Tab", "back"),
                ("Esc", esc_label),
            ]
        } else if is_picker(self.field) {
            vec![
                ("j/k", "choose"),
                ("/", "filter"),
                ("Enter", "accept"),
                ("Shift+Tab", "back"),
                ("Esc", esc_label),
            ]
        } else {
            vec![
                (
                    "type",
                    if self.replace_on_type {
                        "replace selected text"
                    } else {
                        "edit field"
                    },
                ),
                ("Enter", "accept"),
                ("Shift+Tab", "back"),
                ("Esc", "cancel draft"),
            ]
        };
        let candidates: Vec<DraftCandidate> = choices.into_iter().map(|c| c.view).collect();
        DraftView {
            title: match self.kind {
                WorkflowKind::Instrument => "NEW INSTRUMENT",
                WorkflowKind::RenameInstrument(_) => "RENAME INSTRUMENT",
                WorkflowKind::Part => "NEW PART",
                WorkflowKind::RenamePart(_) => "RENAME PART",
                WorkflowKind::Lane => "NEW LANE",
                WorkflowKind::RenameLane(_) => "RENAME LANE",
                WorkflowKind::EditLanePart { .. } => "EDIT LAUNCHPAD ENTRY",
                WorkflowKind::FreezePattern(_) => "FREEZE PATTERN",
                WorkflowKind::Place => "PLACE PART",
                WorkflowKind::PlacedPart => "NEW PLACED PART",
                WorkflowKind::Section => "NEW SECTION",
                WorkflowKind::EditSection(_) => "EDIT SECTION",
                WorkflowKind::Channel => "NEW CHANNEL",
                WorkflowKind::RenameChannel(_) => "RENAME CHANNEL",
                WorkflowKind::RenameMacro(_) => "RENAME MACRO",
                WorkflowKind::Effect => "ADD EFFECT",
                WorkflowKind::RenameEffect(_) => "RENAME EFFECT",
                WorkflowKind::MoveEffect(_) => "MOVE EFFECT",
                WorkflowKind::Send => "CHANNEL SEND",
                WorkflowKind::Duck => "NEW DUCK",
                WorkflowKind::EditDuck(_) => "EDIT DUCK",
                WorkflowKind::Modulation => "NEW MODULATION",
                WorkflowKind::EditModulation(_) => "EDIT MODULATION",
                WorkflowKind::ZoneImportFolder(_) => "IMPORT ZONE FOLDER",
                WorkflowKind::SavePreset(_) => "SAVE PRESET",
                WorkflowKind::Bounce => "BOUNCE / RESAMPLE",
                WorkflowKind::Record => "RECORD AUDIO INPUT",
            }
            .into(),
            fields: self
                .fields()
                .into_iter()
                .map(|field| DraftFieldView {
                    label: if field == DraftField::BounceCreatePart
                        && self.kind == WorkflowKind::Record
                    {
                        "Place in Arrange".into()
                    } else if field == DraftField::BounceTarget && self.kind == WorkflowKind::Record
                    {
                        "Record target".into()
                    } else {
                        field.label().into()
                    },
                    value: self.field_value(project, field),
                    active: field == self.field,
                })
                .collect(),
            field: self.field,
            field_label: self.field.label().into(),
            input: if is_picker(self.field) {
                if matches!(
                    self.field,
                    DraftField::InstrumentKind
                        | DraftField::Sample
                        | DraftField::KitSource
                        | DraftField::Phase
                        | DraftField::ChannelKind
                        | DraftField::EffectKind
                        | DraftField::PresetCategory
                        | DraftField::BounceAction
                        | DraftField::BounceScope
                        | DraftField::BounceTarget
                        | DraftField::BounceCreatePart
                        | DraftField::RecordAction
                        | DraftField::RecordMode
                ) {
                    DraftInput::Choice
                } else {
                    DraftInput::Reference
                }
            } else if is_name(self.field)
                || matches!(
                    self.field,
                    DraftField::SampleFolder
                        | DraftField::BounceRange
                        | DraftField::BounceZoneMidi
                        | DraftField::RecordBars
                )
            {
                DraftInput::Text
            } else {
                DraftInput::Number
            },
            text: self.text().unwrap_or("").into(),
            replace_on_type: self.replace_on_type,
            query: self.query.clone(),
            searching: self.searching,
            selected,
            candidates,
            summary,
            preview,
            error: self.error.clone(),
            hints,
        }
    }

    fn bounce_scope_summary(&self, project: &Project) -> String {
        match self.bounce_scope_kind {
            BounceScopeKind::Part => self
                .bounce_part
                .and_then(|id| project.part(id))
                .map_or_else(|| "Part".into(), |p| format!("Part '{}'", p.name)),
            BounceScopeKind::Pattern => self
                .bounce_pattern
                .and_then(|id| project.pattern(id))
                .map_or_else(|| "Pattern".into(), |p| format!("Pattern '{}'", p.name)),
            BounceScopeKind::Range => format!("Bars {}", self.bounce_range),
        }
    }

    fn bounce_target_summary(&self, project: &Project) -> String {
        match self.bounce_target_kind {
            BounceTargetKind::NewSampler => "New Sampler".into(),
            BounceTargetKind::ReplaceZone => {
                let inst = self
                    .bounce_zone_instrument
                    .and_then(|id| project.instrument(id));
                match inst {
                    Some(i) => {
                        let idx = i.rack.clamp_zone_index(self.bounce_zone_index);
                        let z = &i.rack.zones[idx];
                        format!("Replace {} · z{} {}", i.name, z.id, z.name)
                    }
                    None => "Replace Zone".into(),
                }
            }
            BounceTargetKind::NewZoneMidi => {
                let iname = self
                    .bounce_zone_instrument
                    .and_then(|id| project.instrument(id))
                    .map_or("Rack", |i| i.name.as_str());
                format!("New Zone {iname} @ {}", self.bounce_zone_midi)
            }
        }
    }

    fn field_value(&self, project: &Project, field: DraftField) -> String {
        if field == DraftField::Name
            && matches!(self.kind, WorkflowKind::Bounce | WorkflowKind::Record)
        {
            return if self.name.trim().is_empty() {
                "Auto".into()
            } else {
                self.name.clone()
            };
        }
        if is_name(field) {
            return self.named_text(project, field);
        }
        match field {
            DraftField::PatternLength => format!("{} steps", self.pattern_length),
            DraftField::PartTranspose => format!("{} st", self.part_transpose),
            DraftField::PartVelocity => self.part_velocity.clone(),
            DraftField::Start => self.start.clone(),
            DraftField::Duration => self.duration.clone(),
            DraftField::Offset => format!("{} steps", self.offset),
            DraftField::InstrumentKind => self.rack_template.label().into(),
            DraftField::Preset => match &self.preset {
                None => self.rack_template.default_preset_name().into(),
                Some(id) => self
                    .rack_presets
                    .iter()
                    .find(|p| &p.id == id)
                    .map_or_else(|| id.clone(), |p| p.name.clone()),
            },
            DraftField::KitSource => match &self.kit_source {
                KitSourceChoice::Blank => "Blank Kit".into(),
                KitSourceChoice::Preset(id) => self
                    .rack_presets
                    .iter()
                    .find(|p| &p.id == id)
                    .map_or_else(|| id.clone(), |p| p.name.clone()),
                KitSourceChoice::Folder => "Sample folder…".into(),
            },
            DraftField::Sample => match &self.sampler_source {
                SamplerSourceChoice::SelectedSample => self
                    .sample
                    .as_ref()
                    .map_or("Blank Sampler".into(), |(_, label)| label.clone()),
                SamplerSourceChoice::Blank => "Blank Sampler".into(),
                SamplerSourceChoice::Browse => "Browse sample files…".into(),
                SamplerSourceChoice::Preset(id) => self
                    .rack_presets
                    .iter()
                    .find(|p| &p.id == id)
                    .map_or_else(|| id.clone(), |p| p.name.clone()),
            },
            DraftField::SampleFolder => {
                if self.sample_folder.trim().is_empty() {
                    "Enter folder path…".into()
                } else {
                    self.sample_folder.clone()
                }
            }
            DraftField::Phase => format!("{:?}", self.phase),
            DraftField::ChannelKind => self.channel_kind.label().into(),
            DraftField::ChannelOutput => channel_display_name(project, self.channel_output),
            DraftField::EffectKind => self.effect_kind.to_ascii_uppercase(),
            DraftField::EffectChannel => channel_display_name(project, self.effect_channel),
            DraftField::EffectSlot => format!("slot {}", self.effect_slot),
            DraftField::SendSource => self.send_source.map_or_else(
                || "Choose…".into(),
                |id| channel_display_name(project, ChannelRef::Channel(id)),
            ),
            DraftField::SendAmount => self.send_amount.clone(),
            DraftField::DuckSource => self.duck_source.map_or_else(
                || "Choose…".into(),
                |id| {
                    project
                        .part(id)
                        .map_or_else(|| format!("Missing #{id}"), |p| p.name.clone())
                },
            ),
            DraftField::DuckTarget => channel_display_name(project, self.duck_target),
            DraftField::DuckDepth => self.duck_depth.clone(),
            DraftField::DuckRecovery => self.duck_recovery.clone(),
            DraftField::ModTarget => self
                .mod_target_node
                .map_or_else(|| "Choose…".into(), |n| format_mod_node(project, n)),
            DraftField::ModParam => self
                .mod_target
                .map_or_else(|| "Choose…".into(), |t| format_mod_param(project, t)),
            DraftField::PresetCategory => self.preset_category.into(),
            DraftField::BounceAction => match self.bounce_action {
                BounceActionChoice::BounceNow => "Bounce now".into(),
                BounceActionChoice::EditScope => "Change scope".into(),
                BounceActionChoice::EditRange => "Edit bar range".into(),
                BounceActionChoice::EditTarget => "Change target".into(),
                BounceActionChoice::EditName => "Edit name".into(),
                BounceActionChoice::EditCreatePart => "Trigger part".into(),
            },
            DraftField::BounceScope => match self.bounce_scope_kind {
                BounceScopeKind::Part => "Part".into(),
                BounceScopeKind::Pattern => "Pattern".into(),
                BounceScopeKind::Range => "Bar range".into(),
            },
            DraftField::BouncePart => self
                .bounce_part
                .and_then(|id| project.part(id))
                .map_or_else(|| "Choose…".into(), |p| p.name.clone()),
            DraftField::BouncePattern => self
                .bounce_pattern
                .and_then(|id| project.pattern(id))
                .map_or_else(|| "Choose…".into(), |p| p.name.clone()),
            DraftField::BounceRange => self.bounce_range.clone(),
            DraftField::BounceTarget => match self.bounce_target_kind {
                BounceTargetKind::NewSampler => "New Sampler".into(),
                BounceTargetKind::ReplaceZone => "Replace Zone".into(),
                BounceTargetKind::NewZoneMidi => "New Zone + MIDI".into(),
            },
            DraftField::BounceZoneInstrument => self
                .bounce_zone_instrument
                .and_then(|id| project.instrument(id))
                .map_or_else(|| "Choose…".into(), |i| i.name.clone()),
            DraftField::BounceZone => self
                .bounce_zone_instrument
                .and_then(|id| project.instrument(id))
                .map(|i| {
                    let idx = i.rack.clamp_zone_index(self.bounce_zone_index);
                    let z = &i.rack.zones[idx];
                    format!("z{} · {}", z.id, z.name)
                })
                .unwrap_or_else(|| "Choose…".into()),
            DraftField::BounceZoneMidi => self.bounce_zone_midi.clone(),
            DraftField::BounceCreatePart => {
                if self.bounce_create_part {
                    if self.kind == WorkflowKind::Record {
                        "Yes (Arrange)".into()
                    } else {
                        "Yes (--part)".into()
                    }
                } else {
                    "No".into()
                }
            }
            DraftField::RecordAction => match self.record_action {
                RecordActionChoice::RecordNow => "Record now".into(),
                RecordActionChoice::EditMode => "Timing mode".into(),
                RecordActionChoice::EditBars => "Length (bars)".into(),
                RecordActionChoice::EditTarget => "Change target".into(),
                RecordActionChoice::EditName => "Edit name".into(),
                RecordActionChoice::EditArrange => "Place in Arrange".into(),
            },
            DraftField::RecordMode => self.record_mode.label().into(),
            DraftField::RecordBars => {
                if self.record_bars.trim().is_empty() {
                    "Open / Loop".into()
                } else {
                    format!("{} bars", self.record_bars.trim())
                }
            }
            DraftField::Part if self.kind == WorkflowKind::PlacedPart => self.name.clone(),
            reference => match self.reference(reference) {
                Some(RefChoice::Existing(id)) => match reference {
                    DraftField::Pattern | DraftField::ModPattern => {
                        project.pattern(id).map(|p| p.name.as_str())
                    }
                    DraftField::Instrument => project.instrument(id).map(|i| i.name.as_str()),
                    DraftField::Channel | DraftField::SendTarget => {
                        project.channel(id).map(|c| c.name.as_str())
                    }
                    DraftField::Part => project.part(id).map(|b| b.name.as_str()),
                    DraftField::Lane => project.lane(id).map(|l| l.name.as_str()),
                    _ => None,
                }
                .map_or(format!("Missing #{id}"), str::to_string),
                Some(RefChoice::New) => format!(
                    "+ New {}",
                    match reference {
                        DraftField::Pattern => "Notes",
                        DraftField::ModPattern => "Values",
                        DraftField::Instrument => "instrument",
                        DraftField::Channel => "Audio channel",
                        DraftField::SendTarget => "Return channel",
                        DraftField::Part => "part",
                        DraftField::Lane => "lane",
                        _ => "object",
                    }
                ),
                None => "Choose…".into(),
            },
        }
    }

    pub(super) fn default_modulation_name(&self, project: &Project) -> String {
        let base = match self.mod_target {
            Some(target) => {
                let node = format_mod_node(project, ModTargetNode::from_target(target));
                let param = format_mod_param(project, target);
                format!("{node} {param}")
            }
            None => "Mod".into(),
        };
        unique_name(
            &base,
            project.modulations.iter().map(|m| m.name.as_str()),
            120,
        )
    }
}

pub(crate) fn format_modulation_target(project: &Project, target: ModulationTarget) -> String {
    crate::links_ui::format_modulation_target(project, target)
}

fn format_mod_node(project: &Project, node: ModTargetNode) -> String {
    match node {
        ModTargetNode::Instrument(id) => project
            .instrument(id)
            .map_or_else(|| format!("Inst #{id}"), |i| i.name.clone()),
        ModTargetNode::Effect(id) => project.device(id).map_or_else(
            || format!("FX #{id}"),
            |(ch, d)| format!("{} · {}", channel_display_name(project, ch), d.name),
        ),
        ModTargetNode::Channel(ch) => channel_display_name(project, ch),
        ModTargetNode::Macro(index) => project.macro_control(index).map_or_else(
            || format!("M{}", usize::from(index) + 1),
            |m| format!("M{} {}", usize::from(index) + 1, m.name),
        ),
    }
}

fn format_mod_param(project: &Project, target: ModulationTarget) -> String {
    match target {
        ModulationTarget::Instrument {
            instrument,
            parameter,
        } => crate::links_ui::format_instrument_param_short(project, instrument, parameter),
        ModulationTarget::Effect { parameter, .. } => parameter.label().into(),
        ModulationTarget::ChannelGain { .. } => "Gain".into(),
        ModulationTarget::ChannelPan { .. } => "Pan".into(),
        ModulationTarget::Send { target, .. } => format!(
            "Send → {}",
            channel_display_name(project, ChannelRef::Channel(target))
        ),
        ModulationTarget::Macro { index } => format!("M{} Value", usize::from(index) + 1),
    }
}

#[cfg(test)]
mod tests {
    use super::super::plan::WorkflowContext;
    use super::*;
    use cadence_core::presets::PresetCatalog;
    use std::collections::HashSet;

    #[test]
    fn workflow_picker_shortcuts_never_collide_with_navigation_or_each_other() {
        // Reserved choice-picker navigation & action keys matched before shortcut lookup
        // in `WorkflowDraft::handle_key` (`j` Down, `k` Up, `/` Search, `a` Audition).
        const RESERVED: [char; 4] = ['j', 'k', '/', 'a'];
        let project = Project::demo();
        let presets = PresetCatalog::empty();
        let draft = WorkflowDraft::new(
            WorkflowKind::PlacedPart,
            &project,
            WorkflowContext::default(),
            &presets,
        );

        let mut total_shortcuts = 0usize;
        for field in DraftField::ALL {
            let mut seen = HashSet::new();
            for candidate in draft.choices(&project, field) {
                if let Some(key) = candidate.view.key {
                    let lower = key.to_ascii_lowercase();
                    assert_eq!(
                        key, lower,
                        "{field:?} candidate '{}' shortcut '{key}' must be lowercase",
                        candidate.view.label
                    );
                    assert!(
                        !RESERVED.contains(&lower),
                        "{field:?} candidate '{}' shortcut '{key}' collides with reserved navigation key {RESERVED:?}",
                        candidate.view.label
                    );
                    assert!(
                        seen.insert(lower),
                        "{field:?} has duplicate choice shortcut '{key}' (on '{}')",
                        candidate.view.label
                    );
                    total_shortcuts += 1;
                }
            }
        }
        assert!(
            total_shortcuts >= 20,
            "expected at least 20 workflow choice shortcuts across DraftField::ALL, got {total_shortcuts}"
        );
    }
}
