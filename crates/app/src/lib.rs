//! Session/controller for the retained material workspace.
//! Musical edits pass through one validated transaction; key input is an adapter.
use crate::drafts::{Draft, DraftOutcome, DraftView, WorkflowDraft, WorkflowKind};
use anyhow::{bail, ensure, Context, Result};
use cadence_core::assets::{load_project_samples_with, SampleIssue};
use cadence_core::filter_patterns::FilterGrooveCatalog;
use cadence_core::follow::FollowState;
use cadence_core::material::{
    self, Audition, Consumer, Edit, Id, InstrumentInstance, Lane, LanePart, Modulation,
    ModulationTarget, Part, Pattern, PatternKind, PatternType, PatternValue, PhaseMode, Placement,
    Project, Section, ValuePoint, ValueUnit, AUDITION_INSTRUMENT_ID,
};
use cadence_core::mixer::{
    Channel, ChannelKind, ChannelParam, ChannelRef, ChannelSend, Duck, EffectDevice, EffectParam,
    LiveFlags,
};
use cadence_core::music::{ChordTone, Step};
use cadence_core::params::{InstrumentParam, ParamAddress, ValueMode};
use cadence_core::presets::{InstrumentPreset, PresetCatalog, INSTRUMENT_PRESET_VERSION};
use cadence_core::rack::{
    module_sample_key, InPort, ModuleKind, ModuleKindTag, ModuleParam, PanelPin, PatchCable,
    RackModule, RackParam, RackPatch, RackTemplate, RackZone, ZoneParam,
};
use cadence_core::runtime::{SampleBank, TransportIntent};
use cadence_core::sampler::{SampleData, SampleLibrary};
use cadence_core::score::ScoreWindow;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};
const HISTORY_LIMIT: usize = 128;
const MAX_PROJECT_BYTES: u64 = 16 * 1024 * 1024;

mod action;
mod apply;
mod arrangement;
mod bounce;
mod browser;
mod clipboard;
mod commands;
mod controls;
mod decks;
mod devices;
mod document;
pub mod drafts;
mod editing;
mod file_hub;
mod follow;
mod groove_browser;
mod hints;
pub mod host_io;
mod input;
pub mod keys;
pub mod layout;
mod links_ui;
mod macros;
mod midi;
mod mini_editor;
mod navigation;
mod overrides;
mod palette;
mod param_ops;
mod perform;
mod picker;
mod preset_browser;
mod preview;
pub mod rack;
mod recording;
mod sample_deck;
mod sections;
mod song_props;
mod state;
mod transport;
mod value_editor;
mod verbs;
pub mod view;
mod zones;

pub use action::{Action, Request, SampleEditOp};
pub use arrangement::{ArrangeView, EventInspectionView, EventSourceView};
pub use bounce::{BounceRequest, BounceScope, BounceTarget};
pub use browser::{BrowserEntryView, BrowserView};
pub use controls::{Control, ControlTarget};
pub use devices::{
    format_buffer_latency, format_sample_rate, AudioDeckRow, AudioDevicesState, DevicesTab,
};
pub use document::{load_document, load_document_with_io, load_project, load_project_with_io};
pub use drafts::pattern as authoring;
pub use drafts::workflow;
pub use file_hub::{
    ExportMode, ExportScopeKind, ExportSummary, FileDialogRequest, FileEntryKind, FileHubTab,
};
pub use groove_browser::{GrooveBrowserItemView, GrooveBrowserPreview, GrooveBrowserView};
pub use hints::TUTORIAL;
pub use host_io::{CoreHostIo, DirEntryInfo, HostIo, MemoryHostIo};
pub use input::{normalize_shifted_key, parse_key_sequence};
pub use keys::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
pub use midi::{MidiConnectRequest, MidiLogEntry};
pub use mini_editor::MiniEditorView;
pub use palette::{PaletteItemView, PaletteView};
pub use perform::{parse_duration, StageChoice, LANE_KEYS, SECTION_KEYS};
pub use picker::PickerView;
pub use preset_browser::{PresetBrowserItemView, PresetBrowserView};
pub use rack::RackTab;
pub use recording::{
    CompletedRecording, RecordingHostCommand, RecordingMode, RecordingRequest, RecordingState,
    StartRecordingOpts,
};
pub use song_props::{SongPropRowView, SongPropertiesView};
pub use state::{
    ArrangeUiState, AssetCatalogs, HelpUiState, MidiState, RackUiState, SampleDeckUiState,
    SampleToolbarField, SettingsUiState, TakeSession, ViewState,
};
pub use transport::Transport;
pub use value_editor::{ValueCursorView, ValueInputView};
pub use zones::ZoneControl;

use browser::BrowserTarget;
use cadence_core::arrangement_view::EventKey;
pub use document::Document;
use document::{absolute_parent_with_io, HistoryStep};
use input::Input;
use param_ops::{parse_amount, parse_instrument_param as instrument_parameter};
use state::ActiveModal;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BankMode {
    #[default]
    Parts,
    Material,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Workspace {
    #[default]
    Arrange,
    Pattern,
    Mixer,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Focus {
    Library,
    #[default]
    Canvas,
    Inspector,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scope", content = "id", rename_all = "snake_case")]
pub enum EditScope {
    #[default]
    Definition,
    Placement(Id),
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "id", rename_all = "snake_case")]
pub enum Detail {
    #[default]
    None,
    Part(Id),
    Placement(Id),
    Instrument(Id),
    Channel(ChannelRef),
    Effect(Id),
}

/// Focused interactive slot within the selected channel strip on the Mixer deck (`gm`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "slot", content = "index", rename_all = "snake_case")]
pub enum MixerSlot {
    #[default]
    Fader,
    Pan,
    Insert(usize),
    Send(usize),
    Output,
    Duck(usize),
}

#[derive(Clone, Debug, PartialEq)]
enum ConfirmAction {
    Quit,
    New(Option<String>),
    Open(PathBuf),
    ImportRbs(PathBuf),
    DeleteChannel(Id),
    DeleteEffect(Id),
    DeletePattern(Id),
    DeletePart(Id),
    DeleteLane(Id),
    DeleteSection(Id),
    DeleteInstrument(Id),
    DiscardTake,
    ClearMidiMappings,
    SwitchMidiProfile(String),
}

#[derive(Clone, Debug, PartialEq)]
enum Mode {
    Normal,
    /// Piano-keyboard Pattern step entry (`i` on editable Notes: `z-,`, `1-8`, `.`).
    Insert,
    Authoring,
    Create,
    Workflow,
    Browser,
    PresetBrowser,
    GrooveBrowser,
    SongProperties,
    MiniEditor,
    ValueInput,
    Interpolation,
    Palette,
    Search(String),
    Go,
    Transport,
    Window,
    Help,
    Settings,
    FileHub,
    Inspect,
    Assets,
    Tutorial,
    Confirm(ConfirmAction),
    ArrangeViews,
    ArrangementInspect,
    /// The keyboard as an instrument: sections, lanes and notes.
    Jam,
    /// Finishing a staged change: fire, save or discard.
    Stage,
    /// Choosing from a picker over the working deck.
    Pick,
}
#[derive(Clone, Copy, Debug)]
struct ArrangeNav {
    view: ArrangeView,
    step: u32,
    event: Option<EventKey>,
    pitch_anchor: Option<u8>,
    source_cursor: usize,
    inspecting: bool,
}

#[derive(Clone, Copy, Debug)]
struct SelectionNav {
    scope: EditScope,
    pattern: Option<Id>,
    lane: Option<Id>,
    placement: Option<Id>,
    part: Option<Id>,
    instrument: Option<Id>,
    channel: ChannelRef,
}

#[derive(Clone, Copy, Debug)]
struct CursorNav {
    step: usize,
    note_voice: usize,
    control: usize,
    source: usize,
    mixer_slot: MixerSlot,
    macro_binding: Option<usize>,
}

/// Navigation history is session state, separate from musical undo.
#[derive(Clone, Debug)]
pub(crate) struct Navigation {
    layout: layout::Layout,
    arrange: ArrangeNav,
    selection: SelectionNav,
    cursors: CursorNav,
    rack_ui: RackUiState,
    query: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstrumentNoteOn {
    pub instrument_id: Id,
    pub midi: u8,
    pub velocity: f32,
    pub sustain: bool,
    pub part_id: Option<Id>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstrumentNoteOff {
    pub instrument_id: Id,
    pub midi: u8,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportRequest {
    pub path: PathBuf,
    pub bars: u32,
    pub start_step: u64,
    pub steps: u64,
    pub sample_rate: u32,
    pub mode: ExportMode,
    pub use_take: bool,
}
#[derive(Clone, Debug, Default)]
pub struct Outcome {
    pub changed: bool,
    pub samples_changed: bool,
    pub project_boundary: bool,
    pub transport: Vec<TransportIntent>,
    pub rack_audition: Option<(RackPatch, u8, f32)>,
    pub audition_instrument: Option<(Id, u8, f32)>,
    pub live_notes_on: Vec<InstrumentNoteOn>,
    pub live_notes_off: Vec<InstrumentNoteOff>,
    pub preview_sample: Option<cadence_core::sampler::SamplePreview>,
    pub stop_audition: bool,
    pub clear_mixer_meters: Option<Option<ChannelRef>>,
    pub export: Option<ExportRequest>,
    pub bounce: Option<BounceRequest>,
    pub inspection: Option<serde_json::Value>,
    pub quit: bool,
    /// Outgoing semantic MIDI feedback packets (OLED display, RGB pad/transport LEDs, CC echo).
    pub midi_feedback: Vec<cadence_core::midi_map::MidiFeedback>,
    /// Optional request for the host to connect/disconnect MIDI input/output ports.
    pub midi_connect: Option<MidiConnectRequest>,
    /// Request for the host to rescan hot-plugged OS MIDI ports.
    pub midi_rescan: bool,
    /// Request for the host to reconfigure the live hardware audio stream (output, rate, buffer).
    pub audio_reconfigure: Option<cadence_core::runtime::AudioStreamRequest>,
    /// Request for the host to reconfigure the live audio input device, channel mode, or monitor
    /// policy without interrupting active output playback.
    pub audio_input_reconfigure: Option<cadence_core::runtime::AudioInputRequest>,
    /// Request for the host to restart/reopen the active hardware audio stream.
    pub audio_restart: bool,
    /// Request for the host to re-enumerate available OS audio input/output devices.
    pub audio_rescan: bool,
    /// Request for the host recorder + audio engine to arm, stop, or cancel live input capture.
    pub recording_command: Option<RecordingHostCommand>,
    /// Request for the GUI host to open a native OS Finder / desktop file dialog (`rfd`).
    pub file_dialog: Option<FileDialogRequest>,
}

impl Outcome {
    /// Merge every field of `src` into `self` with zero silent drops.
    /// Destructuring all fields ensures any future `Outcome` field must be handled explicitly.
    pub fn merge(&mut self, src: Outcome) {
        let Outcome {
            changed,
            samples_changed,
            project_boundary,
            transport,
            rack_audition,
            audition_instrument,
            live_notes_on,
            live_notes_off,
            preview_sample,
            stop_audition,
            clear_mixer_meters,
            export,
            bounce,
            inspection,
            quit,
            midi_feedback,
            midi_connect,
            midi_rescan,
            audio_reconfigure,
            audio_input_reconfigure,
            audio_restart,
            audio_rescan,
            recording_command,
            file_dialog,
        } = src;
        self.changed |= changed;
        self.samples_changed |= samples_changed;
        self.project_boundary |= project_boundary;
        self.transport.extend(transport);
        if rack_audition.is_some() {
            self.rack_audition = rack_audition;
        }
        if audition_instrument.is_some() {
            self.audition_instrument = audition_instrument;
        }
        self.live_notes_on.extend(live_notes_on);
        self.live_notes_off.extend(live_notes_off);
        if preview_sample.is_some() {
            self.preview_sample = preview_sample;
        }
        self.stop_audition |= stop_audition;
        if let Some(clear) = clear_mixer_meters {
            if self.clear_mixer_meters != Some(None) {
                self.clear_mixer_meters = Some(clear);
            }
        }
        if export.is_some() {
            self.export = export;
        }
        if bounce.is_some() {
            self.bounce = bounce;
        }
        if inspection.is_some() {
            self.inspection = inspection;
        }
        self.quit |= quit;
        self.midi_feedback.extend(midi_feedback);
        if let Some(next_conn) = midi_connect {
            match &mut self.midi_connect {
                Some(existing) => {
                    if next_conn.input.is_some() {
                        existing.input = next_conn.input;
                    }
                    if next_conn.output.is_some() {
                        existing.output = next_conn.output;
                    }
                }
                None => self.midi_connect = Some(next_conn),
            }
        }
        self.midi_rescan |= midi_rescan;
        if audio_reconfigure.is_some() {
            self.audio_reconfigure = audio_reconfigure;
        }
        if audio_input_reconfigure.is_some() {
            self.audio_input_reconfigure = audio_input_reconfigure;
        }
        self.audio_restart |= audio_restart;
        self.audio_rescan |= audio_rescan;
        if recording_command.is_some() {
            self.recording_command = recording_command;
        }
        if file_dialog.is_some() {
            self.file_dialog = file_dialog;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettingsDirKind {
    SampleDirs,
    PresetDirs,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettingsCoreRow {
    Theme,
    Preset,
    KeyHints,
    Monitor,
    TopBarCpu,
    TopBarMeter,
    ArrangeLaneMeters,
    Follow,
    AutoRecord,
    DefaultBpm,
    DefaultRoot,
    DefaultScale,
    LaunchQuantize,
    EditTiming,
    LateTolerance,
}

impl SettingsCoreRow {
    pub(crate) const ALL: [Self; 15] = [
        Self::Theme,
        Self::Preset,
        Self::KeyHints,
        Self::Monitor,
        Self::TopBarCpu,
        Self::TopBarMeter,
        Self::ArrangeLaneMeters,
        Self::Follow,
        Self::AutoRecord,
        Self::DefaultBpm,
        Self::DefaultRoot,
        Self::DefaultScale,
        Self::LaunchQuantize,
        Self::EditTiming,
        Self::LateTolerance,
    ];

    pub(crate) fn index(self) -> usize {
        Self::ALL.iter().position(|&r| r == self).unwrap_or(0)
    }

    pub(crate) fn from_index(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }

    pub(crate) fn group(self) -> &'static str {
        match self {
            Self::Theme => "APPEARANCE",
            Self::Preset | Self::KeyHints => "WORKSPACE",
            Self::Monitor | Self::TopBarCpu | Self::TopBarMeter | Self::ArrangeLaneMeters => {
                "MONITORING"
            }
            Self::Follow | Self::AutoRecord => "TRANSPORT",
            Self::DefaultBpm | Self::DefaultRoot | Self::DefaultScale => "NEW PROJECT",
            Self::LaunchQuantize | Self::EditTiming | Self::LateTolerance => {
                "LIVE TIMING (PROJECT)"
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettingsRowTarget {
    Core(SettingsCoreRow),
    AddDir(SettingsDirKind),
    DirEntry(SettingsDirKind, usize),
}

impl SettingsRowTarget {
    pub(crate) fn group(self) -> &'static str {
        match self {
            Self::Core(row) => row.group(),
            Self::AddDir(_) | Self::DirEntry(_, _) => "ASSET DIRECTORIES",
        }
    }
}

pub struct App {
    // ── Core Document, I/O, Input, Layout & Active Modal ──
    doc: Document,
    io: Arc<dyn HostIo>,
    input: Input,
    layout: layout::Layout,
    modal: ActiveModal,

    // ── Session Infrastructure & Persisted Global Preferences ──
    clipboard: Option<clipboard::Clip>,
    back: Vec<Navigation>,
    last_edit: Option<Vec<Edit>>,
    preview_patch: RackPatch,
    /// Persisted global user preferences (`g,` or `:settings`).
    pub config: cadence_core::config::UserConfig,

    // ── Live Transport, Compiled Score, Staging & Status ──
    pub transport: Transport,
    follow: FollowState,
    /// Live performance state: timeline, overrides, pending changes, audition.
    pub live: cadence_core::live::Performance,
    /// Always-current compiled ring window published to the audio engine.
    audio_score: ScoreWindow,
    staging: Option<Vec<cadence_core::live::StagedChange>>,
    pub status: String,
    pub from_here: bool,
    /// Most recent WAV / Stem export telemetry for display inside File Hub.
    pub last_export_summary: Option<ExportSummary>,

    // ── Cohesive Domain & UI Sub-States ──
    pub devices: AudioDevicesState,
    pub recording: RecordingState,
    pub midi: MidiState,
    pub takes: TakeSession,
    pub assets: AssetCatalogs,
    pub settings_ui: SettingsUiState,
    pub help_ui: HelpUiState,
    pub rack_ui: RackUiState,
    pub sample_ui: SampleDeckUiState,
    pub arrange_ui: ArrangeUiState,
    pub view_state: ViewState,

    // ── Primary Musical Selection & Editing Cursors ──
    pub detail: Detail,
    pub scope: EditScope,
    pub selected_pattern: Option<Id>,
    pub selected_lane: Option<Id>,
    pub selected_placement: Option<Id>,
    pub selected_part: Option<Id>,
    pub selected_instrument: Option<Id>,
    pub selected_channel: ChannelRef,
    pub cursor: usize,
    /// Visually ordered voice index (`0` = lowest pitch .. `N-1` = highest pitch) at `cursor` in Pattern Notes.
    pub note_voice: usize,
    pub control_cursor: usize,
    pub source_cursor: usize,
    pub query: String,
}

impl App {
    pub fn new(project: Project, save_path: PathBuf) -> Result<Self> {
        Self::new_with_flags(project, LiveFlags::default(), save_path)
    }
    pub fn new_with_flags(project: Project, flags: LiveFlags, save_path: PathBuf) -> Result<Self> {
        let library = SampleLibrary::discover(&[], Some(&save_path));
        Self::new_with_library_and_flags(project, flags, save_path, library)
    }
    pub fn new_with_library(
        project: Project,
        save_path: PathBuf,
        library: SampleLibrary,
    ) -> Result<Self> {
        Self::new_with_library_and_flags(project, LiveFlags::default(), save_path, library)
    }
    pub fn new_with_library_and_flags(
        project: Project,
        flags: LiveFlags,
        save_path: PathBuf,
        library: SampleLibrary,
    ) -> Result<Self> {
        Self::with_host_io(project, flags, save_path, library, Arc::new(CoreHostIo))
    }

    /// Construct a pure session backed by an injected [`HostIo`] and empty sample roots
    /// (zero OS filesystem reads when paired with [`MemoryHostIo`]).
    pub fn new_with_host_io(
        project: Project,
        save_path: PathBuf,
        io: Arc<dyn HostIo>,
    ) -> Result<Self> {
        let project_dir = absolute_parent_with_io(io.as_ref(), &save_path)?;
        let library = SampleLibrary::empty_virtual(project_dir);
        Self::with_host_io(project, LiveFlags::default(), save_path, library, io)
    }

    /// Full constructor injecting [`HostIo`] alongside explicit `LiveFlags` and `SampleLibrary`.
    pub fn with_host_io(
        project: Project,
        flags: LiveFlags,
        save_path: PathBuf,
        library: SampleLibrary,
        io: Arc<dyn HostIo>,
    ) -> Result<Self> {
        let project_for_live = project.clone();
        let flags_for_live = flags.clone();
        let asset_base = match library.project_directory() {
            Some(dir) => PathBuf::from(dir),
            None => io.current_dir()?,
        };
        let selected_pattern = project.patterns.first().map(|p| p.id);
        let selected_lane = project.lanes.first().map(|l| l.id);
        let selected_instrument = project.instruments.first().map(|i| i.id);
        let default_config = cadence_core::config::UserConfig::default();
        let presets = io.discover_presets(&[], Some(&save_path), &default_config);
        let groove_roots = filter_groove_roots_for(&asset_base, presets.roots());
        let filter_grooves = io.load_filter_grooves(&groove_roots);
        let groove_bank = filter_grooves.compiled_bank();
        let doc = Document::new(
            project,
            flags,
            save_path,
            asset_base,
            Arc::clone(&groove_bank),
        )?;
        let mut live =
            cadence_core::live::Performance::new_with_flags(&project_for_live, flags_for_live);
        let mut audio_score = live.compile(doc.project(), doc.flags(), None)?;
        audio_score.mixer.filter_grooves = groove_bank;
        let mut app = Self {
            doc,
            io,
            input: Input::default(),
            layout: Self::default_layout(),
            modal: ActiveModal::None,
            clipboard: None,
            back: Vec::new(),
            last_edit: None,
            preview_patch: RackPatch::default_synth(),
            config: cadence_core::config::UserConfig::default(),
            transport: Transport::default(),
            follow: FollowState::default(),
            live,
            audio_score,
            staging: None,
            status: "Enter open · Alt+hjkl / ,. adjust · g decks · gj jam · ? guide".into(),
            from_here: false,
            last_export_summary: None,
            devices: AudioDevicesState::default(),
            recording: RecordingState::default(),
            midi: MidiState::default(),
            takes: TakeSession::new(default_config.auto_record_takes),
            assets: AssetCatalogs::new(library, presets, filter_grooves),
            settings_ui: SettingsUiState::default(),
            help_ui: HelpUiState::default(),
            rack_ui: RackUiState::default(),
            sample_ui: SampleDeckUiState::default(),
            arrange_ui: ArrangeUiState::new(selected_lane),
            view_state: ViewState::default(),
            detail: Detail::None,
            scope: EditScope::Definition,
            selected_pattern,
            selected_lane,
            selected_placement: None,
            selected_part: None,
            selected_instrument,
            selected_channel: ChannelRef::Master,
            cursor: 0,
            note_voice: 0,
            control_cursor: 0,
            source_cursor: 0,
            query: String::new(),
        };
        app.sync_loaded_samples();
        app.normalize_focus();
        app.select_at_cursor();
        Ok(app)
    }
    /// Configure a directory for streaming auto-recorded session take journals.
    /// When unset (default for pure/headless runs), takes record in memory only.
    pub fn set_session_takes_dir(&mut self, dir: PathBuf) {
        self.takes.session_dir = Some(dir);
    }
    /// Snapshot of the currently rolling take, or the most recently sealed take.
    pub fn current_take(&self) -> Option<cadence_core::take::Take> {
        if let Some(journal) = &self.takes.active {
            Some(journal.snapshot(self.live.next_step))
        } else {
            self.takes.last.clone()
        }
    }
    /// Disk path of the currently open session take journal, if disk-backed.
    pub fn active_take_path(&self) -> Option<&Path> {
        self.takes.active.as_ref().and_then(|t| t.path.as_deref())
    }
    /// Disk path of the most recently sealed session take journal, if disk-backed.
    pub fn last_sealed_take_path(&self) -> Option<&Path> {
        self.takes.last_sealed_path.as_deref()
    }
    /// Read-only access to the injected [`HostIo`] implementation.
    pub fn host_io(&self) -> &dyn HostIo {
        self.io.as_ref()
    }

    pub(crate) fn record_recent_project(&mut self, path: &Path) {
        let canonical = self
            .io
            .canonicalize(path)
            .unwrap_or_else(|_| path.to_path_buf());
        self.config.record_recent_project_canonical(canonical);
    }

    /// Export the current (or most recently sealed) take to `path`.
    pub fn save_take(&self, path: &Path) -> Result<cadence_core::take::Take> {
        let take = self
            .current_take()
            .context("No take has been recorded yet")?;
        let bytes = take.to_jsonl_bytes()?;
        self.io
            .write_atomic(path, cadence_core::take::MAX_TAKE_BYTES, &bytes)?;
        Ok(take)
    }
    /// The authored project. Changes go through actions and commands.
    pub fn project(&self) -> &Project {
        self.doc.project()
    }
    /// Live mute and solo flags (saved with the project, outside undo).
    pub fn flags(&self) -> &LiveFlags {
        self.doc.flags()
    }
    /// Increases with every recorded change, undo and redo.
    pub fn revision(&self) -> u64 {
        self.doc.revision()
    }
    pub fn document(&self) -> &Document {
        &self.doc
    }
    pub fn compiled_project(&self) -> &ScoreWindow {
        self.doc.compiled()
    }
    pub fn performance(&self) -> &ScoreWindow {
        self.compiled_project()
    }
    /// The compiled ring score currently heard by the audio engine.
    pub fn audio_project(&self) -> &ScoreWindow {
        &self.audio_score
    }
    /// The compiled ring window (score + provenance + required instruments).
    pub fn audio_compiled(&self) -> &ScoreWindow {
        &self.audio_score
    }
    pub fn audition(&self) -> Option<Audition> {
        self.live.audition
    }
    pub fn is_auditioning(&self) -> bool {
        self.live.audition.is_some()
    }
    pub fn is_solo_auditioning(&self) -> bool {
        self.live.audition.is_some_and(|a| a.solo)
    }
    pub fn audition_pattern(&self) -> Option<Id> {
        self.live.audition.map(|a| a.pattern)
    }
    pub fn scope_instrument(&self) -> Option<Id> {
        self.detail_instrument()
            .filter(|_| self.deck_active())
            .or_else(|| self.live.audition.map(|a| a.instrument))
            .or(self.selected_instrument)
    }
    pub fn sync_loaded_samples(&mut self) {
        let io = Arc::clone(&self.io);
        let report = load_project_samples_with(
            self.doc.compiled(),
            &mut self.assets.library,
            |p| io.is_file(p),
            |p| io.canonicalize(p),
            |p, lbl| io.load_sample_file(p, lbl),
        );
        self.assets.samples = report.bank.into_iter().collect();
        self.assets.issues = report.issues;
    }

    pub(crate) fn load_library_sample(&mut self, locator: &str) -> Result<Arc<SampleData>> {
        let io = Arc::clone(&self.io);
        self.assets.library.load_with(
            locator,
            |p| io.is_file(p),
            |p| io.canonicalize(p),
            |p, lbl| io.load_sample_file(p, lbl),
        )
    }

    pub(crate) fn reload_library_sample(&mut self, locator: &str) -> Result<Arc<SampleData>> {
        let io = Arc::clone(&self.io);
        self.assets.library.reload_with(
            locator,
            |p| io.is_file(p),
            |p| io.canonicalize(p),
            |p, lbl| io.load_sample_file(p, lbl),
        )
    }

    pub(crate) fn resolve_library_sample_path(&self, locator: &str) -> Option<PathBuf> {
        self.assets
            .library
            .resolve_path_with(locator, |p| self.io.is_file(p))
    }
    pub fn sample_bank(&self) -> SampleBank {
        self.assets
            .samples
            .iter()
            .map(|(&id, s)| (id, Arc::clone(s)))
            .collect()
    }
    pub fn sample_issues(&self) -> &[SampleIssue] {
        &self.assets.issues
    }
    pub fn required_sample_issues(&self) -> Vec<&SampleIssue> {
        self.assets
            .issues
            .iter()
            .filter(|s| {
                self.doc
                    .compiled()
                    .required_instrument_ids
                    .contains(&s.instrument_id)
            })
            .collect()
    }
    pub fn ready(&self) -> bool {
        self.required_sample_issues().is_empty()
    }
    pub fn ensure_samples_ready(&self) -> Result<()> {
        let issues: Vec<_> = self.required_sample_issues().into_iter().cloned().collect();
        cadence_core::assets::ensure_samples_ready(&issues)
    }
    pub fn instrument_sample(&self, sample_key: Id) -> Option<&Arc<SampleData>> {
        self.assets.samples.get(&sample_key)
    }
    pub fn dirty(&self) -> bool {
        if let Some(original_dirty) = self.modal.preview_original_dirty() {
            return original_dirty;
        }
        self.doc.dirty()
    }
    pub(crate) fn has_preview_browser(&self) -> bool {
        self.modal.is_preview_browser()
    }
    pub(crate) fn cancel_active_preview_browser(&mut self) -> Result<Outcome> {
        match self.modal {
            ActiveModal::PresetBrowser(_) => self.cancel_preset_browser(),
            ActiveModal::GrooveBrowser(_) => self.cancel_groove_browser(),
            _ => Ok(Outcome::default()),
        }
    }
    pub fn command_line(&self) -> Option<&str> {
        self.modal
            .palette()
            .filter(|_| self.input.mode == Mode::Palette)
            .map(|p| p.query.as_str())
    }
    pub fn authoring_view(&self) -> Option<DraftView> {
        match self.input.mode {
            Mode::Create => Some(self.create_view()),
            Mode::Workflow => self
                .modal
                .workflow()
                .map(|draft| draft.view(self.doc.project())),
            Mode::Authoring => self
                .modal
                .authoring()
                .map(|draft| draft.view(self.doc.project())),
            _ => None,
        }
    }
    pub fn is_authoring(&self) -> bool {
        matches!(
            self.input.mode,
            Mode::Create | Mode::Workflow | Mode::Authoring
        )
    }
    pub fn is_value_input(&self) -> bool {
        self.input.mode == Mode::ValueInput && self.modal.value_input().is_some()
    }
    pub fn pending_count(&self) -> Option<usize> {
        self.input.count
    }
    pub fn search_line(&self) -> Option<&str> {
        match &self.input.mode {
            Mode::Search(s) => Some(s),
            _ => None,
        }
    }
    pub fn show_help(&self) -> bool {
        self.input.mode == Mode::Help
    }
    pub fn show_settings(&self) -> bool {
        self.input.mode == Mode::Settings
    }
    pub fn ui_mode_kind(&self) -> view::UiModeKind {
        match self.input.mode {
            Mode::Normal | Mode::Insert => view::UiModeKind::Normal,
            Mode::Jam | Mode::Stage => view::UiModeKind::Jam,
            _ => view::UiModeKind::Overlay,
        }
    }
    pub fn show_inspection(&self) -> bool {
        matches!(self.input.mode, Mode::Inspect | Mode::Assets)
    }
    pub fn show_assets(&self) -> bool {
        self.input.mode == Mode::Assets
    }
    pub fn show_tutorial(&self) -> bool {
        self.input.mode == Mode::Tutorial
    }
    pub fn confirm_quit(&self) -> bool {
        matches!(self.input.mode, Mode::Confirm(_))
    }
    pub fn confirm_prompt(&self) -> Option<(&'static str, String)> {
        let Mode::Confirm(action) = &self.input.mode else {
            return None;
        };
        Some(match action {
            ConfirmAction::Quit if self.dirty() && self.doc.is_untitled() => (
                "UNSAVED MATERIAL",
                "s save as · y / Enter discard and quit · Esc cancel".into(),
            ),
            ConfirmAction::Quit if self.dirty() => (
                "UNSAVED MATERIAL",
                "s save & quit · y / Enter discard and quit · Esc cancel".into(),
            ),
            ConfirmAction::Quit => ("QUIT CADENCE", "y / Enter quit · Esc cancel".into()),
            ConfirmAction::New(_) => (
                "UNSAVED MATERIAL",
                "y / Enter discard and start new · Esc cancel".into(),
            ),
            ConfirmAction::Open(_) => (
                "UNSAVED MATERIAL",
                "y / Enter discard and open · Esc cancel".into(),
            ),
            ConfirmAction::ImportRbs(_) => (
                "UNSAVED MATERIAL",
                "y / Enter discard and import .rbs · Esc cancel".into(),
            ),
            ConfirmAction::DeleteChannel(_) => (
                "DELETE CHANNEL",
                "y / Enter delete channel and reroute dependents · Esc cancel".into(),
            ),
            ConfirmAction::DeleteEffect(id) => (
                "DELETE EFFECT",
                self.effect_delete_summary(*id)
                    .unwrap_or_else(|_| "Delete effect?".into()),
            ),
            ConfirmAction::DeletePattern(id) => (
                "DELETE PATTERN",
                self.pattern_delete_summary(*id)
                    .unwrap_or_else(|_| "Delete pattern?".into()),
            ),
            ConfirmAction::DeletePart(id) => (
                "DELETE PART",
                self.part_delete_summary(*id)
                    .unwrap_or_else(|_| "Delete part?".into()),
            ),
            ConfirmAction::DeleteLane(id) => (
                "DELETE LANE",
                self.lane_delete_summary(*id)
                    .unwrap_or_else(|_| "Delete lane?".into()),
            ),
            ConfirmAction::DeleteSection(id) => (
                "DELETE SECTION",
                self.section_delete_summary(*id)
                    .unwrap_or_else(|_| "Delete section?".into()),
            ),
            ConfirmAction::DeleteInstrument(_) => (
                "DELETE INSTRUMENT",
                "y / Enter delete instrument and dependent parts/modulations · Esc cancel".into(),
            ),
            ConfirmAction::DiscardTake => (
                "DISCARD TAKE",
                "y / Enter discard unsaved take and marks · Esc cancel".into(),
            ),
            ConfirmAction::ClearMidiMappings => {
                let count = self.config.midi_mappings.len();
                (
                    "CLEAR USER MIDI MAPPINGS",
                    format!(
                        "Clear {count} user MIDI mapping{}? Built-in profile bindings remain · y / Enter confirm · Esc cancel",
                        if count == 1 { "" } else { "s" }
                    ),
                )
            }
            ConfirmAction::SwitchMidiProfile(target_id) => {
                let target_name = cadence_core::midi_map::MidiProfile::by_id(target_id).name;
                let detail = match (
                    self.midi.custom_profile.as_ref(),
                    self.config.midi_mappings.len(),
                ) {
                    (Some(custom), 0) => format!(
                        "Switch profile to {target_name}? Unloads custom profile '{}' · y / Enter confirm · Esc cancel",
                        custom.name
                    ),
                    (Some(custom), n) => format!(
                        "Switch profile to {target_name}? Unloads custom profile '{}' and clears {n} user mapping{} · y / Enter confirm · Esc cancel",
                        custom.name,
                        if n == 1 { "" } else { "s" }
                    ),
                    (None, n) => format!(
                        "Switch profile to {target_name}? Clears {n} user mapping{} · y / Enter confirm · Esc cancel",
                        if n == 1 { "" } else { "s" }
                    ),
                };
                ("SWITCH MIDI PROFILE", detail)
            }
        })
    }
    pub fn confirm_detail(&self) -> Option<String> {
        let Mode::Confirm(action) = &self.input.mode else {
            return None;
        };
        match action {
            ConfirmAction::DeleteInstrument(id) => self
                .instrument_delete_summary(*id)
                .ok()
                .map(|(_, detail)| detail),
            _ => self.confirm_prompt().map(|(_, msg)| msg),
        }
    }
    pub fn is_transport_prefix(&self) -> bool {
        self.input.mode == Mode::Transport
    }
    pub fn is_window_prefix(&self) -> bool {
        self.input.mode == Mode::Window
    }
    pub fn deck_active(&self) -> bool {
        self.view_state.deck_visible && self.detail != Detail::None
    }
    /// Whether the Take deck currently has focus.
    pub fn take_focused(&self) -> bool {
        self.layout
            .focused()
            .is_some_and(|d| d.kind == layout::DeckKind::Take)
    }
    /// Whether the Relationship / Patchbay Graph deck currently has focus.
    pub fn graph_focused(&self) -> bool {
        self.layout
            .focused()
            .is_some_and(|d| d.kind == layout::DeckKind::Graph)
    }
    /// Whether the Assignable Project Macros deck (`gk`) currently has focus.
    pub fn macros_focused(&self) -> bool {
        self.layout
            .focused()
            .is_some_and(|d| d.kind == layout::DeckKind::Macros)
    }
    /// Currently held MIDI notes across keyboard Jam + hardware MIDI (for Jam keyboard HUD).
    pub fn held_midi_notes(&self) -> Vec<u8> {
        let mut notes: Vec<u8> = self
            .input
            .held
            .iter()
            .map(|&(_, _, midi)| midi)
            .chain(self.input.midi_held.iter().map(|&(m, _)| m))
            .collect();
        notes.sort_unstable();
        notes.dedup();
        notes
    }
    pub fn deck_focused(&self) -> bool {
        self.layout.focused().is_some_and(|d| d.kind.is_object())
    }
    pub fn detail_instrument(&self) -> Option<Id> {
        match self.detail {
            Detail::Instrument(id) => Some(id),
            _ => None,
        }
    }
    fn active_instrument(&self) -> Option<Id> {
        if self.deck_focused() {
            self.detail_instrument().or(self.selected_instrument)
        } else {
            self.selected_instrument
        }
    }
    pub fn visible_patterns(&self) -> Vec<&Pattern> {
        let query = if let Mode::Search(s) = &self.input.mode {
            s
        } else {
            &self.query
        };
        self.doc
            .project()
            .patterns
            .iter()
            .filter(|p| matches_query(query, &format!("{} {}", p.name, kind_label(&p.kind))))
            .collect()
    }
    pub fn pattern(&self, id: Id) -> Option<&Pattern> {
        self.doc.project().patterns.iter().find(|p| p.id == id)
    }
    pub fn instrument(&self, id: Id) -> Option<&InstrumentInstance> {
        self.doc.project().instruments.iter().find(|p| p.id == id)
    }
    pub fn part(&self, id: Id) -> Option<&Part> {
        self.doc.project().parts.iter().find(|p| p.id == id)
    }
    pub fn lane(&self, id: Id) -> Option<&Lane> {
        self.doc.project().lanes.iter().find(|p| p.id == id)
    }
    pub fn placement(&self, id: Id) -> Option<(&Lane, &Placement)> {
        self.doc
            .project()
            .lanes
            .iter()
            .find_map(|l| l.placements.iter().find(|p| p.id == id).map(|p| (l, p)))
    }
    pub fn selected_pattern_object(&self) -> Option<&Pattern> {
        self.selected_pattern.and_then(|id| self.pattern(id))
    }
    pub fn selected_instrument_object(&self) -> Option<&InstrumentInstance> {
        self.selected_instrument.and_then(|id| self.instrument(id))
    }
    pub fn selected_part_object(&self) -> Option<&Part> {
        self.selected_part.and_then(|id| self.part(id))
    }
    pub fn resolve_pattern(&self, id: Id, time: i64) -> Result<PatternValue> {
        self.doc.project().evaluate_pattern(id, time)
    }
    pub fn pattern_period(&self, id: Id) -> usize {
        self.doc.project().pattern_period(id).unwrap_or(1) as usize
    }
    pub fn consumers(&self, id: Id) -> Vec<Consumer> {
        self.doc.project().consumers(id)
    }
    pub fn current_length(&self) -> usize {
        self.length_of(self.selected_pattern, self.scope)
    }
    /// The number of steps a Pattern deck shows for a pattern and scope.
    pub fn length_of(&self, pattern: Option<Id>, scope: EditScope) -> usize {
        match scope {
            EditScope::Placement(id) => self
                .placement(id)
                .map_or(1, |(_, p)| p.length_steps as usize),
            EditScope::Definition => match pattern.and_then(|id| self.pattern(id)).map(|p| &p.kind)
            {
                Some(PatternKind::Notes { steps }) => steps.len(),
                Some(PatternKind::Rhythm { steps }) => steps.len(),
                _ => pattern.map_or(1, |id| self.pattern_period(id)),
            },
        }
    }
    pub fn current_value(&self, step: usize) -> Result<PatternValue> {
        self.value_of(self.selected_pattern, self.scope, step)
    }
    /// A step's value as a Pattern deck shows it for a pattern and scope.
    pub fn value_of(
        &self,
        pattern: Option<Id>,
        scope: EditScope,
        step: usize,
    ) -> Result<PatternValue> {
        match scope {
            EditScope::Definition => {
                let id = pattern.context("Select a pattern")?;
                if let Some(Pattern {
                    kind: PatternKind::Notes { steps },
                    ..
                }) = self.pattern(id)
                {
                    return Ok(PatternValue::Notes(steps[step % steps.len()].clone()));
                }
                self.resolve_pattern(id, step as i64)
            }
            EditScope::Placement(id) => {
                let (_, place) = self.placement(id).context("Placement missing")?;
                ensure!(
                    step < place.length_steps as usize,
                    "Step is outside this placement"
                );
                if let Some(local) = place.exceptions.iter().find(|o| o.step as usize == step) {
                    Ok(PatternValue::Notes(local.value.clone()))
                } else {
                    let part = self.part(place.part).context("Part missing")?;
                    self.resolve_pattern(
                        part.pattern,
                        place.pattern_time(place.start_step + step as u32),
                    )
                }
            }
        }
    }
    pub fn chain(&self) -> String {
        let current_ws = self.workspace();
        let workspace = match current_ws {
            Workspace::Arrange => "ARRANGE",
            Workspace::Pattern => "PATTERN",
            Workspace::Mixer => "MIXER",
        };
        let selection = match if self.deck_focused() {
            self.detail
        } else {
            Detail::None
        } {
            Detail::Instrument(id) => format!(
                "Instrument · {}",
                self.instrument(id).map_or("missing", |x| x.name.as_str())
            ),
            Detail::Effect(id) => format!("Device {id}"),
            Detail::Channel(c) => format!("{}", c),
            Detail::Part(id) => format!(
                "Part · {}",
                self.part(id).map_or("missing", |x| x.name.as_str())
            ),
            Detail::Placement(id) => format!("Placement {id}"),
            Detail::None => match current_ws {
                Workspace::Pattern => self
                    .selected_pattern_object()
                    .map_or("No pattern".into(), |p| {
                        format!("{} · {}", p.name, kind_label(&p.kind))
                    }),
                Workspace::Arrange => self
                    .selected_lane
                    .and_then(|id| self.lane(id))
                    .map_or("No lane".into(), |l| l.name.clone()),
                Workspace::Mixer => format!("{}", self.selected_channel),
            },
        };
        let scope = match self.scope {
            EditScope::Definition => "DEFINITION".into(),
            EditScope::Placement(id) => format!("PLACEMENT {id} · EXCEPTIONS"),
        };
        format!(
            "{workspace} › {selection} · {scope}{}",
            match self.audition() {
                Some(aud) if aud.solo => " · AUDITION (SOLO)",
                Some(_) => " · AUDITION (LIVE)",
                None => "",
            }
        )
    }

    /// Read-only access to the active external instrument preset catalog.
    pub fn presets(&self) -> &PresetCatalog {
        &self.assets.presets
    }
    /// Read-only access to the active external rhythmic filter groove catalog.
    pub fn filter_grooves(&self) -> &FilterGrooveCatalog {
        &self.assets.filter_grooves
    }
    /// Replace the active external preset catalog (e.g., from CLI `--preset-dir` or tests).
    pub fn set_preset_catalog(&mut self, presets: PresetCatalog) {
        self.assets.presets = presets;
        self.assets.custom_presets = true;
    }
    /// Replace the active external filter groove catalog and immediately refresh the compiled
    /// lock-free `Mixer::filter_grooves` snapshot for the audio engine.
    pub fn set_filter_grooves(&mut self, catalog: FilterGrooveCatalog) {
        let bank = catalog.compiled_bank();
        self.assets.filter_grooves = catalog;
        self.doc.set_filter_grooves_bank(Arc::clone(&bank));
        self.audio_score.mixer.filter_grooves = bank;
    }
    pub(crate) fn filter_groove_search_roots(&self) -> Vec<PathBuf> {
        filter_groove_roots_for(self.doc.asset_base(), self.assets.presets.roots())
    }
    /// Rescan configured `preset_dirs` and `filter_grooves.json` around the current project directory.
    pub fn reload_presets(&mut self) {
        self.assets.presets =
            self.io
                .discover_presets(&[], Some(self.doc.save_path()), &self.config);
        let groove_roots = self.filter_groove_search_roots();
        let grooves = self.io.load_filter_grooves(&groove_roots);
        self.set_filter_grooves(grooves);
        self.assets.custom_presets = false;
    }

    pub(crate) fn reload_presets_with_status(&mut self) -> usize {
        self.reload_presets();
        let total = self.assets.presets.entries().len();
        let grooves = self.assets.filter_grooves.len();
        self.status = format!(
            "Reloaded {total} Rack preset{} · {grooves} filter groove{}",
            if total == 1 { "" } else { "s" },
            if grooves == 1 { "" } else { "s" }
        );
        total
    }

    pub(crate) fn reload_sample_library_from_config(&mut self) {
        self.assets.library =
            self.io
                .discover_samples(&[], Some(self.doc.save_path()), &self.config);
        self.sync_loaded_samples();
    }

    pub(crate) fn settings_row_targets(&self) -> Vec<SettingsRowTarget> {
        let mut out = Vec::with_capacity(
            SettingsCoreRow::ALL.len()
                + 2
                + self.config.sample_dirs.len()
                + self.config.preset_dirs.len(),
        );
        for row in SettingsCoreRow::ALL {
            out.push(SettingsRowTarget::Core(row));
        }
        out.push(SettingsRowTarget::AddDir(SettingsDirKind::SampleDirs));
        for i in 0..self.config.sample_dirs.len() {
            out.push(SettingsRowTarget::DirEntry(SettingsDirKind::SampleDirs, i));
        }
        out.push(SettingsRowTarget::AddDir(SettingsDirKind::PresetDirs));
        for i in 0..self.config.preset_dirs.len() {
            out.push(SettingsRowTarget::DirEntry(SettingsDirKind::PresetDirs, i));
        }
        out
    }

    pub(crate) fn add_config_dir(&mut self, kind: SettingsDirKind, raw: String) -> Result<()> {
        let trimmed = raw.trim().to_string();
        ensure!(
            !trimmed.is_empty() && trimmed.len() <= 512 && !trimmed.chars().any(char::is_control),
            "Directory path must be 1..=512 printable characters"
        );
        let list = match kind {
            SettingsDirKind::SampleDirs => &mut self.config.sample_dirs,
            SettingsDirKind::PresetDirs => &mut self.config.preset_dirs,
        };
        if !list.iter().any(|d| d == &trimmed) {
            list.push(trimmed.clone());
        }
        self.settings_ui.config_dirty = true;
        self.maybe_persist_config();
        match kind {
            SettingsDirKind::SampleDirs => {
                self.reload_sample_library_from_config();
                self.status = format!(
                    "Added sample dir '{trimmed}' · {} sample{} indexed",
                    self.assets.library.total_count(),
                    if self.assets.library.total_count() == 1 {
                        ""
                    } else {
                        "s"
                    }
                );
            }
            SettingsDirKind::PresetDirs => {
                let count = self.reload_presets_with_status();
                self.status = format!("Added preset dir '{trimmed}' · {count} presets discovered");
            }
        }
        Ok(())
    }

    pub(crate) fn remove_config_dir(&mut self, kind: SettingsDirKind, idx: usize) -> bool {
        let removed = match kind {
            SettingsDirKind::SampleDirs if idx < self.config.sample_dirs.len() => {
                Some(self.config.sample_dirs.remove(idx))
            }
            SettingsDirKind::PresetDirs if idx < self.config.preset_dirs.len() => {
                Some(self.config.preset_dirs.remove(idx))
            }
            _ => None,
        };
        let Some(removed) = removed else {
            return false;
        };
        self.settings_ui.config_dirty = true;
        self.maybe_persist_config();
        let max_idx = self.settings_row_targets().len().saturating_sub(1);
        self.settings_ui.cursor = self.settings_ui.cursor.min(max_idx);
        match kind {
            SettingsDirKind::SampleDirs => {
                self.reload_sample_library_from_config();
                self.status = format!(
                    "Removed sample dir '{removed}' · {} samples indexed",
                    self.assets.library.total_count()
                );
            }
            SettingsDirKind::PresetDirs => {
                let count = self.reload_presets_with_status();
                self.status =
                    format!("Removed preset dir '{removed}' · {count} presets discovered");
            }
        }
        true
    }
    /// Apply persisted global user preferences (called on startup and after Settings edits).
    pub fn apply_config(&mut self, cfg: cadence_core::config::UserConfig) {
        use cadence_core::follow::FollowMode;
        self.config = cfg.clone();
        self.midi.custom_profile = cfg.midi_profile_path.as_deref().and_then(|path| {
            let bytes = self
                .io
                .read_bounded(path, cadence_core::midi_map::MAX_MIDI_PROFILE_BYTES)
                .ok()?;
            cadence_core::midi_map::MidiProfile::from_json_bytes(&bytes).ok()
        });
        self.settings_ui.config_dirty = false;
        self.settings_ui.persist_on_change = true;
        if !self.assets.custom_presets {
            self.assets.presets =
                self.io
                    .discover_presets(&[], Some(self.doc.save_path()), &self.config);
            let groove_roots = self.filter_groove_search_roots();
            let grooves = self.io.load_filter_grooves(&groove_roots);
            self.set_filter_grooves(grooves);
        }
        self.follow
            .set_enabled(cfg.default_follow == FollowMode::Active);
        let _ = self.set_take_armed(cfg.auto_record_takes);
        if cfg.default_preset != 1 {
            self.apply_layout_preset(cfg.default_preset);
        }
        let _ = self.apply(Action::SetMonitor {
            visible: cfg.show_output_monitor,
        });
        self.view_state.tips = cfg.show_key_hints;
    }

    /// Control whether routine Settings/window updates auto-save `config.json`.
    /// Disabled by the host when an existing `config.json` failed to parse on startup so
    /// background state persistence never overwrites a user's recoverable file until explicit save.
    pub fn set_config_persist_on_change(&mut self, persist: bool) {
        self.settings_ui.persist_on_change = persist;
    }

    /// Update contextual footer key hints visibility (shared by Settings, Action Palette, and `:tips`).
    pub(crate) fn set_key_hints_visible(&mut self, visible: bool) {
        self.view_state.tips = visible;
        self.config.show_key_hints = visible;
        self.settings_ui.config_dirty = true;
        self.maybe_persist_config();
        self.status = format!(
            "Contextual key hints → {}",
            if visible { "On" } else { "Off" }
        );
    }

    pub(crate) fn set_top_bar_cpu_visible(&mut self, visible: bool) {
        self.config.show_top_bar_cpu = visible;
        self.settings_ui.config_dirty = true;
        self.maybe_persist_config();
        self.status = format!("Top-bar DSP CPU → {}", if visible { "On" } else { "Off" });
    }

    pub(crate) fn cycle_top_bar_meter(&mut self, delta: i32) {
        self.config.top_bar_meter = self.config.top_bar_meter.cycle(delta);
        self.settings_ui.config_dirty = true;
        self.maybe_persist_config();
        self.status = format!(
            "Top-bar stereo meter → {}",
            self.config.top_bar_meter.label()
        );
    }

    pub(crate) fn set_arrange_lane_meters_visible(&mut self, visible: bool) {
        self.config.show_arrange_lane_meters = visible;
        self.settings_ui.config_dirty = true;
        self.maybe_persist_config();
        self.status = format!(
            "Arrange lane meters → {}",
            if visible { "On" } else { "Off" }
        );
    }

    pub(crate) fn maybe_persist_config(&mut self) {
        if self.settings_ui.persist_on_change && self.io.save_user_config(&self.config).is_ok() {
            self.settings_ui.config_dirty = false;
        }
    }

    /// Record and persist GUI window dimensions & maximized/fullscreen state cleanly on shutdown.
    pub fn persist_window_state(
        &mut self,
        width: u32,
        height: u32,
        maximized: bool,
        fullscreen: bool,
    ) {
        use cadence_core::config::{
            MAX_WINDOW_HEIGHT, MAX_WINDOW_WIDTH, MIN_WINDOW_HEIGHT, MIN_WINDOW_WIDTH,
        };
        let clamped_w = Some(width.clamp(MIN_WINDOW_WIDTH, MAX_WINDOW_WIDTH));
        let clamped_h = Some(height.clamp(MIN_WINDOW_HEIGHT, MAX_WINDOW_HEIGHT));
        let changed = self.config.window_width != clamped_w
            || self.config.window_height != clamped_h
            || self.config.window_maximized != maximized
            || self.config.window_fullscreen != fullscreen;
        if !changed {
            return;
        }
        self.config.window_width = clamped_w;
        self.config.window_height = clamped_h;
        self.config.window_maximized = maximized;
        self.config.window_fullscreen = fullscreen;
        self.maybe_persist_config();
    }

    /// Clear all transient modal/overlay session state cleanly back to baseline.
    pub(crate) fn dismiss_transient_modals(&mut self) {
        self.clear_entry_state();
        self.settings_ui.clear_editing();
        self.help_ui.clear_search();
        self.input.resume_mode = None;
    }

    /// Single authoritative dirty-checked quit flow shared by `:q`, Action Palette,
    /// OS window close (`CloseRequested`), and macOS `Cmd+Q` / `Cmd+W`.
    pub fn request_quit(&mut self) -> Outcome {
        let mut out = if self.has_preview_browser() {
            self.cancel_active_preview_browser().unwrap_or_default()
        } else {
            Outcome::default()
        };
        self.dismiss_transient_modals();
        out.merge(self.release_held());
        if self.dirty() {
            self.input.mode = Mode::Confirm(ConfirmAction::Quit);
        } else {
            out.quit = true;
        }
        out
    }

    pub(crate) fn take_resume_mode(&mut self) -> Mode {
        self.input.resume_mode.take().unwrap_or(Mode::Normal)
    }

    /// Adjust the focused core row in the Settings modal live.
    pub(crate) fn adjust_setting(&mut self, dir: isize, coarse: bool) {
        use cadence_core::config::{
            MAX_DEFAULT_BPM, MAX_DEFAULT_ROOT_MIDI, MIN_DEFAULT_BPM, MIN_DEFAULT_ROOT_MIDI,
        };
        use cadence_core::follow::FollowMode;
        use cadence_core::music::{cycle_scale, named_scale, note_name, scale_label};
        let step_sign = if dir >= 0 { 1isize } else { -1isize };
        let Some(row) = SettingsCoreRow::from_index(self.settings_ui.cursor) else {
            return;
        };
        match row {
            SettingsCoreRow::Theme => {
                self.config.theme = self.config.theme.cycle(step_sign as i32);
                self.status = format!("Theme: {} (previewing live)", self.config.theme.label());
            }
            SettingsCoreRow::Preset => {
                let next =
                    ((self.config.default_preset as isize - 1 + step_sign).rem_euclid(4) + 1) as u8;
                self.config.default_preset = next;
                self.apply_layout_preset(next);
            }
            SettingsCoreRow::Monitor => {
                self.config.show_output_monitor = !self.config.show_output_monitor;
                let _ = self.apply(Action::SetMonitor {
                    visible: self.config.show_output_monitor,
                });
                self.status = format!(
                    "Output monitor: {}",
                    if self.view_state.monitor_visible {
                        "on"
                    } else {
                        "hidden"
                    }
                );
            }
            SettingsCoreRow::TopBarCpu => {
                self.config.show_top_bar_cpu = !self.config.show_top_bar_cpu;
                self.status = format!(
                    "Top-bar DSP CPU: {}",
                    if self.config.show_top_bar_cpu {
                        "on"
                    } else {
                        "hidden"
                    }
                );
            }
            SettingsCoreRow::TopBarMeter => {
                self.config.top_bar_meter = self.config.top_bar_meter.cycle(step_sign as i32);
                self.status = format!(
                    "Top-bar stereo meter: {}",
                    self.config.top_bar_meter.label()
                );
            }
            SettingsCoreRow::ArrangeLaneMeters => {
                self.config.show_arrange_lane_meters = !self.config.show_arrange_lane_meters;
                self.status = format!(
                    "Arrange lane meters: {}",
                    if self.config.show_arrange_lane_meters {
                        "on"
                    } else {
                        "hidden"
                    }
                );
            }
            SettingsCoreRow::KeyHints => {
                self.config.show_key_hints = !self.config.show_key_hints;
                self.view_state.tips = self.config.show_key_hints;
                self.status = format!(
                    "Contextual key hints: {}",
                    if self.config.show_key_hints {
                        "on"
                    } else {
                        "hidden"
                    }
                );
            }
            SettingsCoreRow::Follow => {
                let next = match self.config.default_follow {
                    FollowMode::Off => FollowMode::Active,
                    FollowMode::Active | FollowMode::Suspended => FollowMode::Off,
                };
                self.config.default_follow = next;
                self.follow.set_enabled(next == FollowMode::Active);
                self.status = format!("Default follow: {}", next.label());
            }
            SettingsCoreRow::AutoRecord => {
                self.config.auto_record_takes = !self.config.auto_record_takes;
                let _ = self.set_take_armed(self.config.auto_record_takes);
                self.status = format!(
                    "Auto-record takes: {} (live take {})",
                    if self.config.auto_record_takes {
                        "on"
                    } else {
                        "off"
                    },
                    if self.takes.armed {
                        "armed"
                    } else {
                        "disarmed"
                    }
                );
            }
            SettingsCoreRow::DefaultBpm => {
                let delta = step_sign as f64 * if coarse { 10.0 } else { 1.0 };
                self.config.default_bpm =
                    (self.config.default_bpm + delta).clamp(MIN_DEFAULT_BPM, MAX_DEFAULT_BPM);
                self.status = format!("Default new-song BPM: {:.0}", self.config.default_bpm);
            }
            SettingsCoreRow::DefaultRoot => {
                let delta = step_sign * if coarse { 12 } else { 1 };
                self.config.default_root_midi = (self.config.default_root_midi as isize + delta)
                    .clamp(
                        isize::from(MIN_DEFAULT_ROOT_MIDI),
                        isize::from(MAX_DEFAULT_ROOT_MIDI),
                    ) as u8;
                self.status = format!(
                    "Default new-song key: {}",
                    note_name(self.config.default_root_midi)
                );
            }
            SettingsCoreRow::DefaultScale => {
                let current = named_scale(&self.config.default_scale)
                    .unwrap_or_else(|| vec![0, 2, 3, 5, 7, 8, 10]);
                let hops = if coarse { 3 } else { 1 };
                let mut cur = current;
                let mut next_spec = cycle_scale(&cur, step_sign as i8);
                for _ in 1..hops {
                    cur = next_spec.intervals.to_vec();
                    next_spec = cycle_scale(&cur, step_sign as i8);
                }
                self.config.default_scale = next_spec.id.to_string();
                self.status = format!(
                    "Default new-song scale: {}",
                    scale_label(next_spec.intervals)
                );
            }
            SettingsCoreRow::LaunchQuantize => {
                if let Err(err) = self.cycle_launch_boundary(step_sign) {
                    self.status = format!("{err:#}");
                }
                return;
            }
            SettingsCoreRow::EditTiming => {
                if let Err(err) = self.toggle_edit_timing() {
                    self.status = format!("{err:#}");
                }
                return;
            }
            SettingsCoreRow::LateTolerance => {
                if let Err(err) = self.adjust_late_tolerance(step_sign) {
                    self.status = format!("{err:#}");
                }
                return;
            }
        }
        self.settings_ui.config_dirty = true;
    }

    /// Reset the currently focused core row in Settings back to its authoritative default (`Alt+Backspace`).
    pub(crate) fn reset_focused_setting(&mut self) {
        use cadence_core::follow::FollowMode;
        use cadence_core::music::{named_scale, note_name, scale_label};
        let Some(row) = SettingsCoreRow::from_index(self.settings_ui.cursor) else {
            return;
        };
        let def = cadence_core::config::UserConfig::default();
        let proj_def = Project::default();
        match row {
            SettingsCoreRow::Theme => {
                self.config.theme = def.theme;
                self.status = format!("Reset Theme → {}", self.config.theme.label());
            }
            SettingsCoreRow::Preset => {
                self.config.default_preset = def.default_preset;
                self.apply_layout_preset(def.default_preset);
            }
            SettingsCoreRow::Monitor => {
                self.config.show_output_monitor = def.show_output_monitor;
                let _ = self.apply(Action::SetMonitor {
                    visible: self.config.show_output_monitor,
                });
                self.status = "Reset Output monitor → default".into();
            }
            SettingsCoreRow::TopBarCpu => {
                self.config.show_top_bar_cpu = def.show_top_bar_cpu;
                self.status = "Reset Top-bar DSP CPU → Visible".into();
            }
            SettingsCoreRow::TopBarMeter => {
                self.config.top_bar_meter = def.top_bar_meter;
                self.status = format!("Reset Top-bar stereo meter → {}", def.top_bar_meter.label());
            }
            SettingsCoreRow::ArrangeLaneMeters => {
                self.config.show_arrange_lane_meters = def.show_arrange_lane_meters;
                self.status = "Reset Arrange lane meters → Hidden".into();
            }
            SettingsCoreRow::KeyHints => {
                self.config.show_key_hints = def.show_key_hints;
                self.view_state.tips = def.show_key_hints;
                self.status = "Reset Contextual key hints → On".into();
            }
            SettingsCoreRow::Follow => {
                self.config.default_follow = def.default_follow;
                self.follow
                    .set_enabled(def.default_follow == FollowMode::Active);
                self.status = format!("Reset Default follow → {}", def.default_follow.label());
            }
            SettingsCoreRow::AutoRecord => {
                self.config.auto_record_takes = def.auto_record_takes;
                let _ = self.set_take_armed(def.auto_record_takes);
                self.status = "Reset Auto-record takes → default".into();
            }
            SettingsCoreRow::DefaultBpm => {
                self.config.default_bpm = def.default_bpm;
                self.status = format!("Reset Default BPM → {:.0}", self.config.default_bpm);
            }
            SettingsCoreRow::DefaultRoot => {
                self.config.default_root_midi = def.default_root_midi;
                self.status = format!(
                    "Reset Default key → {}",
                    note_name(self.config.default_root_midi)
                );
            }
            SettingsCoreRow::DefaultScale => {
                self.config.default_scale = def.default_scale.clone();
                let label = named_scale(&self.config.default_scale)
                    .map(|s| scale_label(&s))
                    .unwrap_or("Natural Minor");
                self.status = format!("Reset Default scale → {label}");
            }
            SettingsCoreRow::LaunchQuantize => {
                let mut timing = self.doc.project().timing;
                timing.launch = proj_def.timing.launch;
                let _ = self.commit(
                    vec![Edit::SetLiveTiming { timing }],
                    format!(
                        "Reset Launch quantize → {}",
                        timing.launch.label(self.doc.project())
                    ),
                );
                return;
            }
            SettingsCoreRow::EditTiming => {
                let mut timing = self.doc.project().timing;
                timing.edits = proj_def.timing.edits;
                let _ = self.commit(
                    vec![Edit::SetLiveTiming { timing }],
                    "Reset Edit timing → default".into(),
                );
                return;
            }
            SettingsCoreRow::LateTolerance => {
                let mut timing = self.doc.project().timing;
                timing.late_tolerance_steps = proj_def.timing.late_tolerance_steps;
                let _ = self.commit(
                    vec![Edit::SetLiveTiming { timing }],
                    format!(
                        "Reset Late tolerance → {} steps",
                        timing.late_tolerance_steps
                    ),
                );
                return;
            }
        }
        self.settings_ui.config_dirty = true;
    }

    pub(crate) fn reset_config(&mut self) {
        let def = cadence_core::config::UserConfig::default();
        self.apply_config(def);
        self.reload_sample_library_from_config();
        self.settings_ui.config_dirty = true;
        self.settings_ui.cursor = self
            .settings_ui
            .cursor
            .min(self.settings_row_targets().len().saturating_sub(1));
        self.status = "Settings reset to defaults · Ctrl+s saves config.json".into();
    }

    pub fn save_config(&mut self) -> Result<()> {
        let path = self.io.save_user_config(&self.config)?;
        self.settings_ui.config_dirty = false;
        self.status = format!("Saved settings to {}", path.display());
        Ok(())
    }
}
fn matches_query(query: &str, text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    query
        .split_whitespace()
        .all(|q| text.contains(&q.to_ascii_lowercase()))
}
pub fn kind_label(kind: &PatternKind) -> &'static str {
    kind.label()
}

/// Choose a unique name derived from `base` (case-insensitive against `taken`),
/// stripping control chars and optionally clamping character length to `limit`.
pub(crate) fn unique_name<'a>(
    base: &str,
    taken: impl IntoIterator<Item = &'a str>,
    limit: Option<usize>,
) -> String {
    let taken: Vec<&str> = taken.into_iter().collect();
    let trimmed = base.trim();
    let base = if trimmed.is_empty() {
        "Untitled"
    } else {
        trimmed
    };
    let clean: String = match limit {
        Some(lim) => base.chars().filter(|c| !c.is_control()).take(lim).collect(),
        None => base.chars().filter(|c| !c.is_control()).collect(),
    };
    if !taken.iter().any(|t| t.eq_ignore_ascii_case(&clean)) {
        return clean;
    }
    for n in 2..=taken.len() + 2 {
        let suffix = format!(" {n}");
        let candidate = match limit {
            Some(lim) => format!(
                "{}{suffix}",
                clean
                    .chars()
                    .take(lim.saturating_sub(suffix.len()))
                    .collect::<String>()
            ),
            None => format!("{clean}{suffix}"),
        };
        if !taken.iter().any(|t| t.eq_ignore_ascii_case(&candidate)) {
            return candidate;
        }
    }
    format!("{clean} {}", taken.len() + 3)
}

fn lookup<'a>(text: &str, items: impl Iterator<Item = (Id, &'a str)>, kind: &str) -> Result<Id> {
    let pairs: Vec<_> = items.collect();
    if let Ok(id) = text.trim_start_matches('#').parse::<Id>() {
        if pairs.iter().any(|&(candidate, _)| candidate == id) {
            return Ok(id);
        }
    }
    let matches: Vec<_> = pairs
        .iter()
        .filter(|(_, name)| name.eq_ignore_ascii_case(text))
        .collect();
    ensure!(
        matches.len() == 1,
        "{} '{}' is missing or ambiguous; use its stable ID",
        kind,
        text
    );
    Ok(matches[0].0)
}
fn move_id(current: Option<Id>, ids: &[Id], delta: isize) -> Option<Id> {
    if ids.is_empty() {
        None
    } else {
        let index = current
            .and_then(|id| ids.iter().position(|&candidate| candidate == id))
            .unwrap_or(0);
        Some(ids[(index as isize + delta).clamp(0, ids.len() as isize - 1) as usize])
    }
}

/// Shared URL/filename slugifier used by File Hub default paths and Bounce export filenames.
pub(crate) fn slugify(raw: &str, fallback: &str, max_chars: Option<usize>) -> String {
    let mut out = String::new();
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if (ch.is_whitespace() || ch == '-' || ch == '_')
            && !out.ends_with('-')
            && !out.is_empty()
        {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        fallback.to_string()
    } else if let Some(max) = max_chars {
        trimmed.chars().take(max).collect()
    } else {
        trimmed.to_string()
    }
}

/// Shared cyclic group-start jump helper for vertical control lists, Settings, and Song Properties.
pub(crate) fn jump_group_index(groups: &[&str], current: usize, forward: bool) -> Option<usize> {
    if groups.is_empty() {
        return None;
    }
    let starts: Vec<usize> = groups
        .iter()
        .enumerate()
        .filter_map(|(i, g)| (i == 0 || *g != groups[i - 1]).then_some(i))
        .collect();
    let cur = current.min(groups.len() - 1);
    if starts.len() <= 1 {
        return Some(cur);
    }
    if forward {
        Some(
            starts
                .iter()
                .copied()
                .find(|&s| s > cur)
                .unwrap_or(starts[0]),
        )
    } else {
        Some(
            starts
                .iter()
                .copied()
                .rev()
                .find(|&s| s < cur)
                .or_else(|| starts.last().copied())
                .unwrap_or(0),
        )
    }
}

pub(crate) fn filter_groove_roots_for(asset_base: &Path, preset_roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut roots = Vec::with_capacity(preset_roots.len() + 1);
    let project_presets = asset_base.join("presets");
    roots.push(project_presets);
    for r in preset_roots {
        if !roots.iter().any(|existing| existing == r) {
            roots.push(r.clone());
        }
    }
    roots
}
