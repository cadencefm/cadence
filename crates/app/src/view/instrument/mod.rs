//! Semantic Instrument, Object, and Output deck views with pre-sampled GPU plot curves.
use super::output_observed;
use crate::layout::DeckKind;
use crate::rack::RackTab;
use crate::{App, Control, Detail};
use cadence_core::dsp::sanitize_sample;
use cadence_core::material::Id;
use cadence_core::mixer::ChannelRef;
use cadence_core::params::{format_hz, format_pan, InstrumentParam, ParamAddress};
use cadence_core::rack::{InPort, ModuleKind, ModuleKindTag, PitchTracking, RackModule, RackPatch};
use cadence_core::runtime::AudioSnapshot;
use cadence_core::sampler::SampleLoopMode;

mod patchbay;
mod visuals;
mod zones;

pub(in crate::view) const RACK_ENV_POINTS: usize = 40;

#[derive(Clone, Debug)]
pub struct ControlView {
    pub module: &'static str,
    pub label: String,
    pub display: String,
    pub normalized: Option<f32>,
    pub controlled: bool,
    pub driver_pills: Vec<String>,
    pub pinned: bool,
    pub selected: bool,
    pub midi_badge: Option<String>,
    pub midi_learning: bool,
}

#[derive(Clone, Debug)]
pub struct InstrumentDeckView {
    pub id: Id,
    pub name: String,
    pub channel_name: String,
    pub controls: Vec<ControlView>,
    pub control_cursor: usize,
    pub visual: InstrumentVisualView,
}

#[derive(Clone, Debug)]
pub enum InstrumentVisualView {
    Rack(Box<RackDeckView>),
    Missing,
}

/// Unified Eurorack Modular Instrument Deck view model (`Panel` / `Patchbay` / `Zones`).
#[derive(Clone, Debug)]
pub struct RackDeckView {
    pub kit_name: String,
    pub active_tab: RackTab,
    pub tabs: Vec<(&'static str, bool)>,
    pub active_zone_id: u16,
    pub active_zone_name: String,
    pub zone_count: usize,
    pub modules: Vec<RackModulePillView>,
    pub visual: ModuleVisualView,
    pub patchbay: PatchbayDeckView,
    pub zones: Vec<RackZoneView>,
    pub selected_zone_summary: String,
    pub live_waveform: Vec<f32>,
}

/// One horizontal module pill on the Instrument `Panel` tab strip (including optional `Quick` pins).
#[derive(Clone, Debug)]
pub struct RackModulePillView {
    pub module_id: Option<u16>,
    pub kind: Option<ModuleKindTag>,
    pub label: String,
    pub tag: &'static str,
    pub column: u8,
    pub active: bool,
}

/// Pre-sampled DSP visualization for the active `Panel` module (or `Quick` pinned overview).
#[derive(Clone, Debug)]
pub enum ModuleVisualView {
    PinnedOverview {
        pin_count: usize,
        osc_label: Option<String>,
        osc_curve: Vec<f32>,
        filter_label: Option<String>,
        filter_curve: Vec<f32>,
        filter_cutoff_norm: f32,
        filter_mod_cutoff_norm: Option<f32>,
        env_label: Option<String>,
        env_curve: Vec<(f32, f32)>,
        live_waveform: Vec<f32>,
    },
    Vco {
        module_id: u16,
        name: String,
        label: String,
        waveform_label: &'static str,
        /// Two cycles (or extended sub-harmonic ratio window under FM) sampled in `[-1.2, 1.2]`.
        curve: Vec<f32>,
        fm_active: bool,
        unison_voices: u8,
        active: bool,
        live_waveform: Vec<f32>,
    },
    Sampler {
        module_id: u16,
        name: String,
        sample_name: Option<String>,
        duration_sec: f64,
        peaks: Vec<(f32, f32)>,
        frames: usize,
        sample_start: f32,
        sample_end: f32,
        loop_mode: SampleLoopMode,
        loop_start: f32,
        loop_end: f32,
        live_waveform: Vec<f32>,
    },
    Noise {
        module_id: u16,
        name: String,
        color_label: &'static str,
        highpass_hz: f32,
        lowpass_hz: f32,
        /// Log-frequency band-shaping curve in dB clamped to `[-48.0, 12.0]`.
        band_curve: Vec<f32>,
        highpass_norm: f32,
        lowpass_norm: f32,
        live_waveform: Vec<f32>,
    },
    Env {
        module_id: u16,
        name: String,
        label: String,
        mode_label: &'static str,
        /// Normalized `(x in 0..=1, y in 0..=1)` breakpoints strictly via `eval_modular_env_curve`.
        curve: Vec<(f32, f32)>,
        bursts: u8,
        live_waveform: Vec<f32>,
    },
    Lfo {
        module_id: u16,
        name: String,
        shape_label: &'static str,
        bipolar: bool,
        sync: bool,
        rate_label: String,
        /// Two cycles in `[-1.0, 1.0]` (bipolar) or `[0.0, 1.0]` (unipolar).
        curve: Vec<f32>,
        live_waveform: Vec<f32>,
    },
    Filter {
        module_id: u16,
        name: String,
        label: String,
        mode_label: &'static str,
        /// Log-frequency filter response in dB clamped to `[-48.0, 18.0]` via `filter_mix_magnitude_db`.
        curve: Vec<f32>,
        cutoff_norm: f32,
        mod_cutoff_norm: Option<f32>,
        mix: f32,
        live_waveform: Vec<f32>,
    },
    Vca {
        module_id: u16,
        name: String,
        gain: f32,
        bias: f32,
        cv_patched: bool,
        ring_mod: bool,
        /// Gain/ring-mod transfer across normalized CV sweep.
        transfer_curve: Vec<f32>,
        live_waveform: Vec<f32>,
    },
    Drive {
        module_id: u16,
        name: String,
        label: String,
        mode_label: &'static str,
        /// Waveshaper transfer curve for `x in [-1.0, 1.0]` via `drive_transfer_sample`.
        transfer_curve: Vec<f32>,
        live_waveform: Vec<f32>,
    },
    LoFi {
        module_id: u16,
        name: String,
        bits: f32,
        rate_hz: f32,
        mix: f32,
        /// Quantizer transfer staircase for `x in [-1.0, 1.0]` via `lofi_transfer_sample`.
        transfer_curve: Vec<f32>,
        live_waveform: Vec<f32>,
    },
    Resonator {
        module_id: u16,
        name: String,
        mode_label: &'static str,
        base_hz: f32,
        decay_ms: f32,
        brightness: f32,
        position: f32,
        mix: f32,
        /// `(freq_norm, amp_norm, decay_norm)` stems derived strictly via `resonator_mode_descriptors`.
        modes: Vec<(f32, f32, f32)>,
        live_waveform: Vec<f32>,
    },
    Mobius {
        module_id: u16,
        name: String,
        exciter_label: &'static str,
        dispersion_label: &'static str,
        topology_label: &'static str,
        base_hz: f32,
        atom_count: u8,
        stiffness: f32,
        kerr_semitones: f32,
        distance_ms: f32,
        feedback: f32,
        /// Target source preview sampled strictly via `mobius_target_preview`.
        target_curve: Vec<f32>,
        /// `(freq_norm, ring_norm, kerr_signed_norm, delay_norm)` stems derived via `mobius_atom_descriptors`.
        atoms: Vec<(f32, f32, f32, f32)>,
        live_waveform: Vec<f32>,
    },
    Out {
        module_id: u16,
        name: String,
        gain_db: f32,
        pan: f32,
        spread: f32,
        voice_summary: String,
        live_waveform: Vec<f32>,
    },
}

/// Semantic signal category for color-coding Patchbay cables in `cable_graph.rs`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatchbayCableKind {
    Audio,
    Cv,
    Pitch,
    Fm,
    Gate,
}

impl PatchbayCableKind {
    pub const fn from_port(port: InPort) -> Self {
        match port {
            InPort::In => Self::Audio,
            InPort::Cv | InPort::PwCv => Self::Cv,
            InPort::PitchCv => Self::Pitch,
            InPort::FmIn => Self::Fm,
            InPort::TrigIn => Self::Gate,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PatchbayNodeView {
    pub module_id: u16,
    pub col: usize,
    pub row: usize,
    pub name: String,
    pub badge: &'static str,
    pub detail: String,
    pub kind: ModuleKindTag,
    pub selected: bool,
    pub in_path: bool,
    pub incoming_ports: Vec<InPort>,
    pub has_output: bool,
}

#[derive(Clone, Debug)]
pub struct PatchbayColumnView {
    pub title: &'static str,
    pub nodes: Vec<PatchbayNodeView>,
}

#[derive(Clone, Debug)]
pub struct PatchbayCableView {
    pub cable_id: u16,
    pub from_module: u16,
    pub to_module: u16,
    pub to_port: InPort,
    pub from_col: usize,
    pub from_row: usize,
    pub to_col: usize,
    pub to_row: usize,
    pub to_port_y: f32,
    pub kind: PatchbayCableKind,
    pub amount: f32,
    /// Formatted attenuverter label; feedback edges use strictly ASCII `"z-1"` badge.
    pub label: String,
    pub feedback: bool,
    pub selected: bool,
    pub highlighted: bool,
}

#[derive(Clone, Debug)]
pub struct PatchbayDeckView {
    pub columns: Vec<PatchbayColumnView>,
    pub nodes: Vec<PatchbayNodeView>,
    pub cables: Vec<PatchbayCableView>,
    pub selected_col: usize,
    pub selected_row: usize,
    pub selected_module: Option<u16>,
    pub selected_cable: Option<u16>,
    pub selected_summary: String,
    pub has_feedback: bool,
    /// ASCII-safe badge string (`"z-1"`) for JetBrains Mono glyph compatibility.
    pub feedback_badge: &'static str,
    pub live_waveform: Vec<f32>,
}

#[derive(Clone, Debug)]
pub struct RackZoneView {
    pub id: u16,
    pub name: String,
    pub low_midi: u8,
    pub high_midi: u8,
    pub root_midi: u8,
    pub low_vel: f32,
    pub high_vel: f32,
    pub note_range: String,
    pub vel_range: String,
    pub voice_summary: String,
    pub sample_label: Option<String>,
    pub module_count: usize,
    pub cable_count: usize,
    pub has_sampler: bool,
    pub source_tag: &'static str,
    pub choke_group: Option<u8>,
    pub selected: bool,
}

#[derive(Clone, Debug)]
pub struct ObjectDeckView {
    pub kind: DeckKind,
    pub title: String,
    pub controls: Vec<ControlView>,
    pub control_cursor: usize,
}

#[derive(Clone, Debug)]
pub struct OutputDeckView {
    pub title: &'static str,
    pub waveform: Vec<f32>,
    pub input_left: Option<f32>,
    pub input_right: Option<f32>,
    pub input_hold_left: Option<f32>,
    pub input_hold_right: Option<f32>,
    pub peak_left: f32,
    pub peak_right: f32,
    pub hold_left: Option<f32>,
    pub hold_right: Option<f32>,
    pub hot: bool,
    pub clipped: bool,
    pub master_gr_db: f32,
    pub master_gr_hold_db: f32,
    pub master_gr_tag: Option<&'static str>,
    pub observed: bool,
}

pub(in crate::view) fn map_controls(
    app: &App,
    controls: &[Control],
    cursor: usize,
    query_time: Option<f64>,
) -> (Vec<ControlView>, usize) {
    let selected = if controls.is_empty() {
        0
    } else {
        cursor.min(controls.len() - 1)
    };
    let mapped = controls
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let (midi_badge, midi_learning, driver_pills, display, normalized, live_active) =
                match c.target {
                    crate::ControlTarget::Param(addr) => {
                        let badge = app.midi_badge_for_param(addr);
                        let learning = app.is_param_learning(addr);
                        let drivers = app.param_driver_pills(addr);
                        if let Some(live_norm) =
                            query_time.and_then(|qt| app.evaluate_live_param_at(addr, qt))
                        {
                            let live_str = app.format_param_normalized_value(addr, live_norm);
                            let disp = if live_str != c.display {
                                format!("{} → {live_str}", c.display)
                            } else {
                                c.display.clone()
                            };
                            (badge, learning, drivers, disp, Some(live_norm), true)
                        } else {
                            (
                                badge,
                                learning,
                                drivers,
                                c.display.clone(),
                                c.normalized,
                                false,
                            )
                        }
                    }
                    _ => (
                        None,
                        false,
                        Vec::new(),
                        c.display.clone(),
                        c.normalized,
                        false,
                    ),
                };
            ControlView {
                module: c.group,
                label: c.label.clone(),
                display,
                normalized,
                controlled: c.controlled || live_active,
                driver_pills,
                pinned: false,
                selected: i == selected,
                midi_badge,
                midi_learning,
            }
        })
        .collect();
    (mapped, selected)
}

pub(super) fn sanitize_wave(samples: &[f32]) -> Vec<f32> {
    samples
        .iter()
        .map(|&s| sanitize_sample(s).clamp(-1.0, 1.0))
        .collect()
}

pub(super) fn module_card_detail(module: &RackModule) -> String {
    match &module.kind {
        ModuleKind::Vco(v) => format!("{} · {:+.0}st", v.waveform.label(), v.tune_semitones),
        ModuleKind::Sampler(s) => s
            .path
            .as_deref()
            .and_then(|p| p.rsplit('/').next())
            .unwrap_or("(no sample)")
            .to_string(),
        ModuleKind::Noise(n) => format!("{} · {:.0}%", n.color.label(), n.level * 100.0),
        ModuleKind::Env(e) => format!(
            "{} · {:.0}/{:.0}ms",
            e.mode.label(),
            e.attack_ms,
            e.decay_ms
        ),
        ModuleKind::Lfo(l) => {
            if l.sync {
                format!("{} · {:.0}st", l.shape.label(), l.division_steps)
            } else {
                format!("{} · {:.2}Hz", l.shape.label(), l.rate_hz)
            }
        }
        ModuleKind::Filter(f) => format!("{} · {}", f.mode.label(), format_hz(f.cutoff_hz)),
        ModuleKind::Vca(v) => format!("gain {:.0}% · bias {:.0}%", v.gain * 100.0, v.bias * 100.0),
        ModuleKind::Drive(d) => format!("{} · {:.0}%", d.mode.label(), d.drive * 100.0),
        ModuleKind::LoFi(l) => format!("{:.0}b · {}", l.bits, format_hz(l.rate_hz)),
        ModuleKind::Resonator(r) => {
            if r.pitch_tracking == PitchTracking::Fixed {
                format!("{} · {}", r.mode.label(), format_hz(r.fixed_hz))
            } else {
                format!("{} · {:.0}ms", r.mode.label(), r.decay_ms)
            }
        }
        ModuleKind::Mobius(m) => {
            if m.distance_ms > 0.5 {
                format!(
                    "{}A {} · {:.0}ms",
                    m.atoms,
                    m.topology.label(),
                    m.distance_ms
                )
            } else if m.feedback > 0.01 {
                format!(
                    "{}A {} · FB {:.0}%",
                    m.atoms,
                    m.topology.label(),
                    m.feedback * 100.0
                )
            } else {
                format!(
                    "{}A {} · {}",
                    m.atoms,
                    m.topology.label(),
                    m.exciter.label()
                )
            }
        }
        ModuleKind::Out(o) => format!("{:+.1} dB · {}", o.gain_db, format_pan(o.pan)),
    }
}

impl App {
    /// Resolve live modulated/linked normalized value for any Rack `InstrumentParam` via `live_params.rs`.
    pub(super) fn resolve_live_rack_param(
        &self,
        inst_id: Id,
        target: InstrumentParam,
        query_time: f64,
    ) -> Option<f32> {
        let addr = ParamAddress::from_instrument(inst_id, target);
        self.evaluate_live_param_at(addr, query_time).or_else(|| {
            let project = self.project();
            let pid = project.parts.iter().find_map(|part| {
                (part.instrument == inst_id)
                    .then(|| {
                        part.parameter_links
                            .iter()
                            .find(|l| l.target == target)
                            .map(|l| l.pattern)
                    })
                    .flatten()
            })?;
            project
                .value_at(pid, query_time)
                .ok()
                .filter(|v| v.is_finite())
                .map(|v| v.clamp(0.0, 1.0))
        })
    }

    /// Build a live-modulated copy of `module` (only evaluating params that are actively modulated/linked).
    pub(super) fn live_effective_module(
        &self,
        inst_id: Id,
        rack: &RackPatch,
        zone_id: u16,
        module: &RackModule,
        query_time: f64,
    ) -> (RackModule, bool) {
        let frames = self.module_sample_frames(inst_id, zone_id, module.id);
        let mut eff_rack: Option<RackPatch> = None;
        let mut any_mod = false;
        for &param in module.kind.tag().params() {
            let target = InstrumentParam::Module {
                zone: zone_id,
                module: module.id,
                param,
            };
            if let Some(norm) = self.resolve_live_rack_param(inst_id, target, query_time) {
                let r = eff_rack.get_or_insert_with(|| rack.clone());
                let _ = r.write_param_normalized(target, norm, frames);
                any_mod = true;
            }
        }
        let eff_mod = eff_rack
            .and_then(|r| r.zone(zone_id).and_then(|z| z.module(module.id)).cloned())
            .unwrap_or_else(|| module.clone());
        (eff_mod, any_mod)
    }

    pub(in crate::view) fn build_instrument_view(
        &self,
        id: Id,
        cursor: usize,
        snapshot: &AudioSnapshot,
    ) -> InstrumentDeckView {
        let query_time = self.live_query_time(snapshot);
        let raw_controls = self.control_descriptors_for(Detail::Instrument(id));
        let (mut controls, control_cursor) =
            map_controls(self, &raw_controls, cursor, Some(query_time));
        let live = snapshot.track_waveform(id);

        let Some(inst) = self.instrument(id) else {
            return InstrumentDeckView {
                id,
                name: format!("Instrument {id}"),
                channel_name: String::new(),
                controls,
                control_cursor,
                visual: InstrumentVisualView::Missing,
            };
        };

        let channel_name = self
            .channel_name(ChannelRef::Channel(inst.channel))
            .to_string();
        let rack = &inst.rack;
        let selected_zone = rack.clamp_zone_index(self.rack_ui.zone_cursor);
        let tabs: Vec<(&'static str, bool)> = RackTab::ALL
            .iter()
            .map(|&t| (t.label(), t == self.rack_ui.tab))
            .collect();
        let zone_views = zones::build_rack_zone_views(rack, selected_zone);

        let (active_zone_id, active_zone_name, modules, visual, patchbay, selected_zone_summary) =
            if let Some(zone) = rack.zones.get(selected_zone) {
                let (modules, active_module_id) =
                    zones::build_rack_module_pills(rack, zone, self.rack_ui.panel_slot);
                let visual = self.build_module_visual(
                    id,
                    rack,
                    zone,
                    active_module_id,
                    snapshot,
                    query_time,
                );
                let patchbay = self.build_patchbay_view(id, zone, live, query_time);
                let summary =
                    zones::format_selected_zone_summary(zone, selected_zone, rack.zones.len());
                (
                    zone.id,
                    zone.name.clone(),
                    modules,
                    visual,
                    patchbay,
                    summary,
                )
            } else {
                (
                    0,
                    String::new(),
                    Vec::new(),
                    visuals::empty_pinned_overview(live),
                    patchbay::empty_patchbay_view(live),
                    "No zones · press a to add a zone".into(),
                )
            };

        let kit_name = if rack.kit_name.is_empty() {
            inst.name.clone()
        } else {
            rack.kit_name.clone()
        };
        for (cv, raw) in controls.iter_mut().zip(&raw_controls) {
            if raw.group != "QUICK" {
                if let crate::ControlTarget::Param(addr) = raw.target {
                    cv.pinned = addr.as_instrument().is_some_and(|(inst_id, target)| {
                        inst_id == id && rack.panel_pins.iter().any(|p| p.target == target)
                    });
                }
            }
        }

        InstrumentDeckView {
            id,
            name: inst.name.clone(),
            channel_name,
            controls,
            control_cursor,
            visual: InstrumentVisualView::Rack(Box::new(RackDeckView {
                kit_name,
                active_tab: self.rack_ui.tab,
                tabs,
                active_zone_id,
                active_zone_name,
                zone_count: rack.zones.len(),
                modules,
                visual,
                patchbay,
                zones: zone_views,
                selected_zone_summary,
                live_waveform: sanitize_wave(live),
            })),
        }
    }

    pub(in crate::view) fn build_object_view(
        &self,
        kind: DeckKind,
        detail: Detail,
        cursor: usize,
        snapshot: &AudioSnapshot,
    ) -> ObjectDeckView {
        let query_time =
            matches!(detail, Detail::Channel(_)).then(|| self.live_query_time(snapshot));
        let raw_controls = self.control_descriptors_for(detail);
        let (controls, control_cursor) = map_controls(self, &raw_controls, cursor, query_time);
        let title = match detail {
            Detail::Placement(id) => self.placement(id).map_or_else(
                || format!("PLACEMENT {id}"),
                |(lane, pl)| {
                    format!(
                        "PLACEMENT {} · {} · B{:02}",
                        self.part(pl.part).map_or("?", |p| p.name.as_str()),
                        lane.name,
                        pl.start_step / 16 + 1
                    )
                },
            ),
            Detail::Part(id) => self
                .part(id)
                .map_or_else(|| format!("PART {id}"), |p| format!("PART {}", p.name)),
            Detail::Channel(ch) => format!("CHANNEL {}", self.channel_name(ch)),
            _ => kind.label().into(),
        };

        ObjectDeckView {
            kind,
            title,
            controls,
            control_cursor,
        }
    }

    pub(in crate::view) fn build_output_view(&self, snapshot: &AudioSnapshot) -> OutputDeckView {
        let master = snapshot.channel_meter(ChannelRef::Master);
        let (master_gr_db, master_gr_hold_db, master_gr_tag) = if self.is_solo_auditioning() {
            (0.0, 0.0, None)
        } else {
            super::mixer::strip_dynamics_gr(&self.project().mixer.master.inserts, snapshot)
        };
        OutputDeckView {
            title: if self.is_solo_auditioning() {
                "AUDITION OUTPUT · 32-bit float"
            } else {
                "MASTER OUTPUT · 32-bit float"
            },
            waveform: sanitize_wave(&snapshot.waveform),
            input_left: master.map(|m| m.input_peak[0]),
            input_right: master.map(|m| m.input_peak[1]),
            input_hold_left: master.map(|m| m.input_peak_hold[0]),
            input_hold_right: master.map(|m| m.input_peak_hold[1]),
            peak_left: snapshot.peak_left,
            peak_right: snapshot.peak_right,
            hold_left: master.map(|m| m.output_peak_hold[0]),
            hold_right: master.map(|m| m.output_peak_hold[1]),
            hot: master.is_some_and(|m| m.hot),
            clipped: master.is_some_and(|m| m.clipped),
            master_gr_db,
            master_gr_hold_db,
            master_gr_tag,
            observed: output_observed(snapshot),
        }
    }
}
