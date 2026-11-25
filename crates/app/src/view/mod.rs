//! Frontend-neutral semantic view model (`FrameView`).
//!
//! `App::frame_view` projects the entire live session and `AudioSnapshot` into
//! pure structured data with smooth sub-step coordinates, zoom factors, live
//! waveforms/meters, and modal overlays. Both `cadence-gui` and pure tests read
//! this layer directly without touching `App` internals or `model::Project`.
mod arrange;
mod bank;
pub mod effect;
mod graph;
mod instrument;
mod live_params;
mod macros;
mod midi;
mod mixer;
mod overlays;
mod pattern;
mod take;
#[cfg(test)]
mod tests;

pub use crate::{DevicesTab, MixerSlot, RackTab};
pub use arrange::{
    ArrangeDeckView, ArrangeEvalNote, ArrangeEvaluatedView, ArrangeLaneRow, ArrangeMark,
    ArrangeOverrideRow, ArrangePlacementBlock, SectionBand,
};
pub use bank::{
    BankDeckView, BankLaneGroup, BankPartEntry, BankPartStatus, BankPatternItem, BankSectionItem,
};
pub use effect::{DelayTapView, EffectDeckView, EffectVisualView, FilterEnvVisual};
pub(crate) use graph::GraphCable;
pub use graph::{GraphColumn, GraphDeckView, GraphEdge, GraphEdgeKind, GraphNode, GraphTarget};
pub use instrument::{
    ControlView, InstrumentDeckView, InstrumentVisualView, ModuleVisualView, ObjectDeckView,
    OutputDeckView, PatchbayCableKind, PatchbayCableView, PatchbayColumnView, PatchbayDeckView,
    PatchbayNodeView, RackDeckView, RackModulePillView, RackZoneView,
};
pub use macros::{MacroBindingView, MacroSlotView, MacroTargetKind, MacrosDeckView};
pub use midi::{
    AudioDeckRowView, AudioDeckView, AudioInputView, MidiDeckView, MidiDeviceRowView,
    MidiMacroTileView, MidiMappingRowView, MidiPadTileView,
};
pub use mixer::{MixerChannelView, MixerDeckView, MixerRole};
pub use overlays::{
    FileHubRowView, FileHubView, HelpEntry, HelpTab, HelpView, OverlayView, SettingsRowView,
    SettingsView, HELP_ENTRIES,
};
pub use pattern::{
    PatternConsumerChip, PatternContentView, PatternDeckView, PatternNoteStep, PatternPlayhead,
    PatternRhythmStep, PatternSourceChip, PatternVoiceView, SourcePreviewKind, SourcePreviewStrip,
    SubStepNoteView,
};
pub use take::{TakeBarSummary, TakeDeckView, TakeEventMarker};

/// Semantic UI modality driving deck border / halo shader styling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiModeKind {
    Normal,
    Jam,
    Overlay,
}

use crate::layout::{Deck, DeckKind, Shape, Spot, Target};
use crate::{App, Detail, EditScope, LANE_KEYS, SECTION_KEYS};
use cadence_core::follow::FollowMode;
use cadence_core::live::Change;
use cadence_core::mixer::ChannelRef;
use cadence_core::music::{note_name, scale_name};
use cadence_core::runtime::AudioSnapshot;

/// Complete semantic snapshot of one Cadence frame.
#[derive(Clone, Debug)]
pub struct FrameView {
    pub chrome: ChromeView,
    pub zoomed: bool,
    pub focused: Spot,
    pub decks: Vec<DeckSlot>,
    pub jam_hud: Option<JamHudView>,
    pub lane_layer: Option<LaneLayerView>,
    pub overlay: Option<OverlayView>,
}

/// Live floating Jam keyboard HUD drawer (rendered only while in Jam mode).
#[derive(Clone, Debug)]
pub struct JamHudView {
    pub instrument_name: String,
    pub octave: i8,
    pub bank: usize,
    pub bank_count: usize,
    pub held_midi: Vec<u8>,
    pub sections: Vec<(char, String, bool, bool)>,
    pub lanes: Vec<(char, String, bool, bool, bool, bool)>,
}

/// Floating lane-layer HUD card shown after pressing `Alt+q..p` or Jam `q..p`.
#[derive(Clone, Debug)]
pub struct LaneLayerView {
    pub lane_name: String,
    pub lane_index: usize,
    pub key_char: char,
    pub overriding: bool,
    pub parts: Vec<(usize, String, bool, bool)>,
}

/// Top transport bar, trail, mode/hints bar, and status line.
#[derive(Clone, Debug)]
pub struct ChromeView {
    pub theme: cadence_core::config::ThemeId,
    pub title: String,
    pub dirty: bool,
    pub audio_status: String,
    pub audio_health: cadence_core::runtime::AudioHealth,
    pub ready: bool,
    pub follow_mode: FollowMode,
    pub source_label: &'static str,
    pub playing: bool,
    pub bpm: f64,
    pub song_step: u64,
    pub song_step_smooth: Option<f64>,
    pub bar: u64,
    pub beat: u64,
    pub substep: u64,
    pub root_note: String,
    pub scale_name: &'static str,
    pub loop_label: String,
    pub live_timing_label: String,
    pub live_timing_cycle: bool,
    pub live_edits_on_launch: bool,
    pub override_count: usize,
    pub pending_count: usize,
    pub staged_count: Option<usize>,
    pub end_rule_label: Option<&'static str>,
    pub take_armed: bool,
    pub take_recording: Option<usize>,
    pub take_capped: bool,
    pub recording_phase: cadence_core::runtime::CapturePhase,
    pub recording_badge: Option<String>,
    pub input_active: bool,
    pub input_monitoring: bool,
    pub input_peak: [f32; 2],
    pub input_clipped: bool,
    pub from_here: bool,
    pub jam_active: bool,
    pub midi_port: Option<String>,
    pub midi_out_port: Option<String>,
    pub midi_learning: bool,
    pub midi_recording: bool,
    pub midi_cc_last_recorded: Option<String>,
    pub midi_last: Option<String>,
    pub active_macro_count: usize,
    pub active_macro_automated: usize,
    pub output_observed: bool,
    pub peak_left: f32,
    pub peak_right: f32,
    pub master_clipped: bool,
    pub master_hot: bool,
    pub mode_name: &'static str,
    pub mode_kind: UiModeKind,
    pub pending_key_count: Option<usize>,
    pub chain: String,
    pub search_line: Option<String>,
    pub hints: Vec<(String, String)>,
    pub status: String,
    pub status_alert: bool,
    pub trail: Vec<String>,
}

/// One visible deck placed in the workspace layout.
#[derive(Clone, Debug)]
pub struct DeckSlot {
    pub spot: Spot,
    pub kind: DeckKind,
    pub shape: Shape,
    pub pinned: bool,
    pub focused: bool,
    pub title: String,
    pub view: DeckView,
}

/// Semantic contents of one deck.
#[derive(Clone, Debug)]
pub enum DeckView {
    Arrange(ArrangeDeckView),
    Pattern(PatternDeckView),
    Bank(BankDeckView),
    Mixer(MixerDeckView),
    Take(TakeDeckView),
    Graph(GraphDeckView),
    Midi(MidiDeckView),
    Macros(MacrosDeckView),
    Instrument(InstrumentDeckView),
    Effect(EffectDeckView),
    Object(ObjectDeckView),
    Output(OutputDeckView),
}

pub(crate) fn output_observed(snapshot: &AudioSnapshot) -> bool {
    snapshot.sample_rate != 0
        || !snapshot.waveform.is_empty()
        || !snapshot.channel_meters.is_empty()
}

pub(crate) fn format_db(peak: f32) -> String {
    cadence_core::dsp::format_db_value(peak)
}

pub(crate) fn loop_label(region: Option<(u32, u32)>) -> String {
    match region {
        None => "loop song".into(),
        Some((start_step, end_step)) if start_step % 16 == 0 && end_step % 16 == 0 => {
            format!("loop B{:02}–{:02}", start_step / 16 + 1, end_step / 16)
        }
        Some((start_step, end_step)) => format!("loop s{}–{}", start_step + 1, end_step),
    }
}

impl App {
    /// Build a complete, frontend-neutral semantic snapshot of the current frame.
    pub fn frame_view(&self, snapshot: &AudioSnapshot, audio_status: &str) -> FrameView {
        let chrome = self.build_chrome_view(snapshot, audio_status);
        let layout = self.layout();
        let focused_spot = layout.focus();
        let mut decks = Vec::new();
        for spot in layout.visible() {
            let Some(deck) = layout.deck(spot) else {
                continue;
            };
            let focused = spot == focused_spot;
            let pinned = layout.is_pinned(spot);
            let title = self.deck_label(deck);
            let view = self.build_deck_view(deck, focused, snapshot);
            decks.push(DeckSlot {
                spot,
                kind: deck.kind,
                shape: deck.kind.shape(),
                pinned,
                focused,
                title,
                view,
            });
        }
        let jam_hud = self.is_jam().then(|| self.build_jam_hud());
        let lane_layer = self.build_lane_layer_hud();
        let overlay = self.build_overlay_view();
        FrameView {
            chrome,
            zoomed: layout.zoomed(),
            focused: focused_spot,
            decks,
            jam_hud,
            lane_layer,
            overlay,
        }
    }

    /// Total Jam banks needed to address all lanes (`q..p`) and sections (`1..0`) in groups of 10.
    pub(crate) fn jam_bank_count(&self) -> usize {
        let project = self.project();
        let lanes = project.lanes.len().div_ceil(LANE_KEYS.len());
        let sections = project.sections.len().div_ceil(SECTION_KEYS.len());
        lanes.max(sections).max(1)
    }

    fn build_jam_hud(&self) -> JamHudView {
        let project = self.project();
        let bank_count = self.jam_bank_count();
        let bank = self.clamped_bank();
        let heard = self.song_step_heard();
        let queued_section = self.live.pending.iter().find_map(|p| match p.change {
            Change::Timeline { segment } => Some(segment.song),
            _ => None,
        });
        let sections = self
            .effective_bank_sections()
            .into_iter()
            .enumerate()
            .filter_map(|(i, id)| {
                let s = project.sections.iter().find(|s| s.id == id)?;
                let start = u64::from(s.start_step);
                let len = u64::from(s.length_steps.max(16));
                Some((
                    SECTION_KEYS[i],
                    s.name.clone(),
                    (start..start + len).contains(&heard),
                    queued_section == Some(start),
                ))
            })
            .collect();
        let focused = self.focused_lane();
        let lanes = self
            .play_bank_lanes()
            .into_iter()
            .enumerate()
            .filter_map(|(i, id)| {
                let lane = self.lane(id)?;
                Some((
                    LANE_KEYS[i],
                    lane.name.clone(),
                    focused == Some(id),
                    self.live.lane_state(id).is_some(),
                    self.flags().lane_muted(id),
                    self.flags().lane_solo(id),
                ))
            })
            .collect();
        let instrument_name = self
            .jam_instrument()
            .and_then(|id| self.instrument(id))
            .map_or_else(|| "Preview Rack".into(), |i| i.name.clone());
        JamHudView {
            instrument_name,
            octave: self.note_octave(),
            bank,
            bank_count,
            held_midi: self.held_midi_notes(),
            sections,
            lanes,
        }
    }

    fn build_lane_layer_hud(&self) -> Option<LaneLayerView> {
        let lane_id = self.lane_layer()?;
        let lane = self.lane(lane_id)?;
        let bank_lanes = self.play_bank_lanes();
        let lane_index = bank_lanes.iter().position(|&id| id == lane_id).unwrap_or(0);
        let key_char = LANE_KEYS.get(lane_index).copied().unwrap_or('q');
        let active_ov = self.live.lane_state(lane_id).map(|s| s.part.part);
        let heard = self.song_step_heard();
        let arranged = lane
            .placements
            .iter()
            .find(|p| {
                (u64::from(p.start_step)..u64::from(p.start_step + p.length_steps)).contains(&heard)
            })
            .map(|p| p.part);
        let parts = lane
            .parts
            .iter()
            .enumerate()
            .take(9)
            .map(|(i, lp)| {
                let name = self
                    .part(lp.part)
                    .map_or_else(|| "?".into(), |p| p.name.clone());
                let is_ov = active_ov == Some(lp.part);
                let is_active = is_ov || (active_ov.is_none() && arranged == Some(lp.part));
                (i + 1, name, is_active, is_ov)
            })
            .collect();
        Some(LaneLayerView {
            lane_name: lane.name.clone(),
            lane_index,
            key_char,
            overriding: active_ov.is_some(),
            parts,
        })
    }

    fn build_chrome_view(&self, snapshot: &AudioSnapshot, audio_status: &str) -> ChromeView {
        let source_label = match self.audition() {
            Some(aud) if aud.solo => "PATTERN AUDITION",
            Some(_) if self.playing_loop().is_some() => "LOOP · AUDITION",
            Some(_) => "SONG · AUDITION",
            None if self.playing_loop().is_some() => "LOOP",
            None => "SONG",
        };
        let song_step = if self.is_solo_auditioning() {
            snapshot.absolute_step
        } else {
            self.song_position(self.observed_transport_step(snapshot))
        };
        let song_step_smooth = self.song_position_smooth(snapshot);
        let loop_text = if let Some(aud) = self.audition().filter(|a| a.solo) {
            loop_label(Some((0, aud.length as u32)))
        } else {
            loop_label(self.playing_loop())
        };
        let end_rule_label = (!self.end_rule().is_loop()).then(|| self.end_rule().label());
        let master_meter = snapshot.channel_meter(ChannelRef::Master);
        let master_clipped = master_meter.is_some_and(|m| m.clipped);
        let master_hot = master_meter.is_some_and(|m| m.hot);
        let status = if !self.ready() {
            format!("REQUIRED SAMPLE UNAVAILABLE · {}", self.status)
        } else {
            self.status.clone()
        };
        let timing = self.project().timing;
        let live_timing_label = timing.launch.label(self.project());
        let live_timing_cycle = matches!(
            timing.launch,
            cadence_core::material::Boundary::Cycle { .. }
        );
        let live_edits_on_launch = timing.edits == cadence_core::material::EditTiming::Launch;
        let inp = snapshot.input;
        let rec_phase = if self.recording.phase != cadence_core::runtime::CapturePhase::Idle {
            self.recording.phase
        } else {
            inp.capture_phase
        };
        let recording_badge = match rec_phase {
            cadence_core::runtime::CapturePhase::Idle => None,
            cadence_core::runtime::CapturePhase::Armed => {
                if let Some(req) = &self.recording.active_request {
                    let bar = req.punch_in_song_step / 16 + 1;
                    if req.mode == crate::recording::RecordingMode::CountIn {
                        Some(format!("○ ARM · count-in → B{bar:02}"))
                    } else {
                        Some(format!("○ ARM · B{bar:02}"))
                    }
                } else {
                    Some("○ ARM".into())
                }
            }
            cadence_core::runtime::CapturePhase::Recording => {
                let sr = inp
                    .sample_rate
                    .max(self.recording.input_sample_rate)
                    .max(snapshot.sample_rate)
                    .max(48_000);
                let frames = self.recording.captured_frames.max(inp.captured_frames);
                let secs = frames as f32 / sr as f32;
                if let Some(req) = &self.recording.active_request {
                    if let Some(end) = req.punch_out_abs_step {
                        let total_bars = end
                            .saturating_sub(req.punch_in_abs_step)
                            .div_ceil(16)
                            .max(1);
                        let done_bars =
                            (snapshot.absolute_step.saturating_sub(req.punch_in_abs_step) / 16 + 1)
                                .min(total_bars);
                        Some(format!("● REC {done_bars}/{total_bars}B · {secs:.1}s"))
                    } else {
                        Some(format!("● REC · {secs:.1}s"))
                    }
                } else {
                    Some(format!("● REC · {secs:.1}s"))
                }
            }
            cadence_core::runtime::CapturePhase::Finishing => Some("● REC · saving…".into()),
        };

        ChromeView {
            theme: self.config.theme,
            title: self.project().title.clone(),
            dirty: self.dirty(),
            audio_status: audio_status.into(),
            audio_health: snapshot.health,
            ready: self.ready(),
            follow_mode: self.follow_mode(),
            source_label,
            playing: snapshot.playing,
            bpm: self.project().bpm,
            song_step,
            song_step_smooth,
            bar: song_step / 16 + 1,
            beat: (song_step % 16) / 4 + 1,
            substep: song_step % 4 + 1,
            root_note: note_name(self.project().root_midi),
            scale_name: scale_name(&self.project().scale),
            loop_label: loop_text,
            live_timing_label,
            live_timing_cycle,
            live_edits_on_launch,
            override_count: self.live.active().len(),
            pending_count: self.live.pending.len(),
            staged_count: self.staging().map(<[_]>::len),
            end_rule_label,
            take_armed: self.takes.armed,
            take_recording: self.take_recording().map(|(_, n)| n),
            take_capped: self.take_capped(),
            recording_phase: rec_phase,
            recording_badge,
            input_active: inp.status == cadence_core::runtime::AudioInputStatus::Running,
            input_monitoring: inp.monitoring,
            input_peak: inp.peak,
            input_clipped: inp.clipped,
            from_here: self.from_here,
            jam_active: self.is_jam(),
            midi_port: self.midi.port.clone(),
            midi_out_port: self.midi.out_port.clone(),
            midi_learning: self.midi.learn.is_some(),
            midi_recording: self.midi.record,
            midi_cc_last_recorded: self.midi_cc_last_recorded().map(str::to_owned),
            midi_last: self.midi.last.clone(),
            active_macro_count: self.project().macros.len(),
            active_macro_automated: self
                .project()
                .macros
                .iter()
                .filter(|mc| {
                    self.project().modulations.iter().any(|m| {
                        m.target
                            == cadence_core::material::ModulationTarget::Macro { index: mc.index }
                    })
                })
                .count(),
            output_observed: output_observed(snapshot),
            peak_left: snapshot.peak_left,
            peak_right: snapshot.peak_right,
            master_clipped,
            master_hot,
            mode_name: self.mode_name(),
            mode_kind: self.ui_mode_kind(),
            pending_key_count: self.pending_count(),
            chain: self.chain(),
            search_line: self.search_line().map(str::to_owned),
            hints: if self.view_state.tips {
                self.hints()
            } else {
                Vec::new()
            },
            status,
            status_alert: !self.ready() || snapshot.health.overloaded || snapshot.health.failed,
            trail: self.trail(),
        }
    }

    fn build_deck_view(&self, deck: &Deck, focused: bool, snapshot: &AudioSnapshot) -> DeckView {
        match deck.kind {
            DeckKind::Arrange => DeckView::Arrange(self.build_arrange_view(focused, snapshot)),
            DeckKind::Pattern => {
                let pattern = match deck.target {
                    Target::Pattern(id) => Some(id),
                    _ => self.selected_pattern,
                };
                let (scope, cursor, source_cursor) = if focused {
                    (self.scope, self.cursor, self.source_cursor)
                } else {
                    (
                        deck.state
                            .local
                            .map_or(EditScope::Definition, EditScope::Placement),
                        deck.state.cursor,
                        deck.state.source_cursor,
                    )
                };
                DeckView::Pattern(self.build_pattern_view(
                    pattern::PatternViewTarget {
                        pattern_id: pattern,
                        scope,
                        cursor,
                        source_cursor,
                        focused,
                    },
                    snapshot,
                ))
            }
            DeckKind::Bank => DeckView::Bank(self.build_bank_view()),
            DeckKind::Mixer => DeckView::Mixer(self.build_mixer_view(snapshot)),
            DeckKind::Take => DeckView::Take(self.build_take_view()),
            DeckKind::Graph => DeckView::Graph(self.build_graph_view(snapshot)),
            DeckKind::Midi => DeckView::Midi(self.build_midi_view(snapshot)),
            DeckKind::Macros => DeckView::Macros(self.build_macros_view(snapshot)),
            DeckKind::Output => DeckView::Output(self.build_output_view(snapshot)),
            DeckKind::Instrument => {
                let id = match deck.target {
                    Target::Instrument(id) => id,
                    _ => self.selected_instrument.unwrap_or(0),
                };
                let cursor = if focused {
                    self.control_cursor
                } else {
                    deck.state.control_cursor
                };
                DeckView::Instrument(self.build_instrument_view(id, cursor, snapshot))
            }
            DeckKind::Effect => {
                let id = match deck.target {
                    Target::Effect(id) => id,
                    _ => match self.detail {
                        Detail::Effect(id) => id,
                        _ => 0,
                    },
                };
                let cursor = if focused {
                    self.control_cursor
                } else {
                    deck.state.control_cursor
                };
                DeckView::Effect(self.build_effect_view(id, cursor, snapshot))
            }
            DeckKind::Placement | DeckKind::Part | DeckKind::Channel => {
                let detail = match deck.target {
                    Target::Placement(id) => Detail::Placement(id),
                    Target::Part(id) => Detail::Part(id),
                    Target::Channel(ch) => Detail::Channel(ch),
                    _ => self.detail,
                };
                let cursor = if focused {
                    self.control_cursor
                } else {
                    deck.state.control_cursor
                };
                DeckView::Object(self.build_object_view(deck.kind, detail, cursor, snapshot))
            }
        }
    }
}
