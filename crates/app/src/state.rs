//! Cohesive state sub-structs and exclusive modal payload enum owned by [`App`].
use crate::{
    arrangement, browser, file_hub, groove_browser, midi, mini_editor, overrides, palette, picker,
    preset_browser, song_props, value_editor, ArrangeView, BankMode, Draft, MixerSlot, Navigation,
    RackTab, SettingsDirKind, WorkflowDraft,
};
use cadence_core::arrangement_view::EventKey;
use cadence_core::assets::SampleIssue;
use cadence_core::filter_patterns::FilterGrooveCatalog;
use cadence_core::material::Id;
use cadence_core::mixer::ChannelRef;
use cadence_core::presets::PresetCatalog;
use cadence_core::sampler::{SampleData, SampleLibrary};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

/// Creation workflow suspended while the Sample Browser picks an audio file.
pub(crate) struct SuspendedWorkflow {
    pub draft: WorkflowDraft,
    pub origin: Option<Navigation>,
}

/// Owned payload extracted when closing or committing [`ActiveModal::Browser`].
pub(crate) struct TakenBrowser {
    pub state: Box<browser::BrowserState>,
    pub return_workflow: Option<Box<SuspendedWorkflow>>,
}

/// Mutually exclusive modal / overlay payload active on [`App`].
/// Keeping every exclusive modal payload in one enum makes dual-modal state
/// structurally unrepresentable while keeping `Mode` lightweight (`Clone + PartialEq`).
#[derive(Default)]
pub(crate) enum ActiveModal {
    #[default]
    None,
    Picker(Box<picker::Picker>),
    Create {
        cursor: usize,
        origin: Option<Navigation>,
    },
    Authoring {
        draft: Box<Draft>,
        origin: Option<Navigation>,
    },
    Workflow {
        draft: Box<WorkflowDraft>,
        origin: Option<Navigation>,
    },
    Browser {
        state: Box<browser::BrowserState>,
        /// Suspended `Workflow` draft when Sample Browser was opened from a creation workflow.
        return_workflow: Option<Box<SuspendedWorkflow>>,
    },
    ValueInput {
        state: Box<value_editor::ValueInput>,
        /// Suspended `SongProperties` modal when ValueInput was opened from Song Properties.
        return_song_props: Option<song_props::SongPropertiesState>,
    },
    FileHub(Box<file_hub::FileHubState>),
    PresetBrowser(Box<preset_browser::PresetBrowserState>),
    GrooveBrowser(Box<groove_browser::GrooveBrowserState>),
    SongProperties(song_props::SongPropertiesState),
    MiniEditor(Box<mini_editor::MiniEditorState>),
    Palette(Box<palette::PaletteState>),
    Search {
        origin: Option<Navigation>,
    },
}

impl ActiveModal {
    pub(crate) fn is_preview_browser(&self) -> bool {
        matches!(self, Self::PresetBrowser(_) | Self::GrooveBrowser(_))
    }

    pub(crate) fn preview_original_dirty(&self) -> Option<bool> {
        match self {
            Self::PresetBrowser(pb) => Some(pb.original_dirty),
            Self::GrooveBrowser(gb) => Some(gb.original_dirty),
            _ => None,
        }
    }

    pub(crate) fn picker(&self) -> Option<&picker::Picker> {
        match self {
            Self::Picker(p) => Some(p),
            _ => None,
        }
    }

    pub(crate) fn picker_mut(&mut self) -> Option<&mut picker::Picker> {
        match self {
            Self::Picker(p) => Some(p),
            _ => None,
        }
    }

    pub(crate) fn take_picker(&mut self) -> Option<Box<picker::Picker>> {
        if matches!(self, Self::Picker(_)) {
            match std::mem::take(self) {
                Self::Picker(p) => Some(p),
                _ => unreachable!(),
            }
        } else {
            None
        }
    }

    pub(crate) fn authoring(&self) -> Option<&Draft> {
        match self {
            Self::Authoring { draft, .. } => Some(draft),
            _ => None,
        }
    }

    pub(crate) fn take_authoring(&mut self) -> Option<(Box<Draft>, Option<Navigation>)> {
        if matches!(self, Self::Authoring { .. }) {
            match std::mem::take(self) {
                Self::Authoring { draft, origin } => Some((draft, origin)),
                _ => unreachable!(),
            }
        } else {
            None
        }
    }

    pub(crate) fn workflow(&self) -> Option<&WorkflowDraft> {
        match self {
            Self::Workflow { draft, .. } => Some(draft),
            _ => None,
        }
    }

    pub(crate) fn workflow_mut(&mut self) -> Option<&mut WorkflowDraft> {
        match self {
            Self::Workflow { draft, .. } => Some(draft),
            _ => None,
        }
    }

    pub(crate) fn take_workflow(&mut self) -> Option<(Box<WorkflowDraft>, Option<Navigation>)> {
        if matches!(self, Self::Workflow { .. }) {
            match std::mem::take(self) {
                Self::Workflow { draft, origin } => Some((draft, origin)),
                _ => unreachable!(),
            }
        } else {
            None
        }
    }

    pub(crate) fn create_cursor(&self) -> usize {
        match self {
            Self::Create { cursor, .. } => *cursor,
            _ => 0,
        }
    }

    pub(crate) fn create_cursor_mut(&mut self) -> Option<&mut usize> {
        match self {
            Self::Create { cursor, .. } => Some(cursor),
            _ => None,
        }
    }

    pub(crate) fn take_origin(&mut self) -> Option<Navigation> {
        match self {
            Self::Create { origin, .. }
            | Self::Authoring { origin, .. }
            | Self::Workflow { origin, .. }
            | Self::Search { origin } => origin.take(),
            _ => None,
        }
    }

    pub(crate) fn browser(&self) -> Option<&browser::BrowserState> {
        match self {
            Self::Browser { state, .. } => Some(state),
            _ => None,
        }
    }

    pub(crate) fn browser_mut(&mut self) -> Option<&mut browser::BrowserState> {
        match self {
            Self::Browser { state, .. } => Some(state),
            _ => None,
        }
    }

    pub(crate) fn browser_return_workflow(&self) -> Option<&WorkflowDraft> {
        match self {
            Self::Browser {
                return_workflow: Some(wf),
                ..
            } => Some(&wf.draft),
            _ => None,
        }
    }

    pub(crate) fn take_browser(&mut self) -> Option<TakenBrowser> {
        if matches!(self, Self::Browser { .. }) {
            match std::mem::take(self) {
                Self::Browser {
                    state,
                    return_workflow,
                } => Some(TakenBrowser {
                    state,
                    return_workflow,
                }),
                _ => unreachable!(),
            }
        } else {
            None
        }
    }

    pub(crate) fn value_input(&self) -> Option<&value_editor::ValueInput> {
        match self {
            Self::ValueInput { state, .. } => Some(state),
            _ => None,
        }
    }

    pub(crate) fn value_input_mut(&mut self) -> Option<&mut value_editor::ValueInput> {
        match self {
            Self::ValueInput { state, .. } => Some(state),
            _ => None,
        }
    }

    pub(crate) fn take_value_input(
        &mut self,
    ) -> Option<(
        Box<value_editor::ValueInput>,
        Option<song_props::SongPropertiesState>,
    )> {
        if matches!(self, Self::ValueInput { .. }) {
            match std::mem::take(self) {
                Self::ValueInput {
                    state,
                    return_song_props,
                } => Some((state, return_song_props)),
                _ => unreachable!(),
            }
        } else {
            None
        }
    }

    pub(crate) fn file_hub(&self) -> Option<&file_hub::FileHubState> {
        match self {
            Self::FileHub(h) => Some(h),
            _ => None,
        }
    }

    pub(crate) fn file_hub_mut(&mut self) -> Option<&mut file_hub::FileHubState> {
        match self {
            Self::FileHub(h) => Some(h),
            _ => None,
        }
    }

    pub(crate) fn take_file_hub(&mut self) -> Option<Box<file_hub::FileHubState>> {
        if matches!(self, Self::FileHub(_)) {
            match std::mem::take(self) {
                Self::FileHub(h) => Some(h),
                _ => unreachable!(),
            }
        } else {
            None
        }
    }

    pub(crate) fn preset_browser(&self) -> Option<&preset_browser::PresetBrowserState> {
        match self {
            Self::PresetBrowser(pb) => Some(pb),
            _ => None,
        }
    }

    pub(crate) fn preset_browser_mut(&mut self) -> Option<&mut preset_browser::PresetBrowserState> {
        match self {
            Self::PresetBrowser(pb) => Some(pb),
            _ => None,
        }
    }

    pub(crate) fn take_preset_browser(
        &mut self,
    ) -> Option<Box<preset_browser::PresetBrowserState>> {
        if matches!(self, Self::PresetBrowser(_)) {
            match std::mem::take(self) {
                Self::PresetBrowser(pb) => Some(pb),
                _ => unreachable!(),
            }
        } else {
            None
        }
    }

    pub(crate) fn groove_browser(&self) -> Option<&groove_browser::GrooveBrowserState> {
        match self {
            Self::GrooveBrowser(gb) => Some(gb),
            _ => None,
        }
    }

    pub(crate) fn groove_browser_mut(&mut self) -> Option<&mut groove_browser::GrooveBrowserState> {
        match self {
            Self::GrooveBrowser(gb) => Some(gb),
            _ => None,
        }
    }

    pub(crate) fn take_groove_browser(
        &mut self,
    ) -> Option<Box<groove_browser::GrooveBrowserState>> {
        if matches!(self, Self::GrooveBrowser(_)) {
            match std::mem::take(self) {
                Self::GrooveBrowser(gb) => Some(gb),
                _ => unreachable!(),
            }
        } else {
            None
        }
    }

    pub(crate) fn song_props(&self) -> Option<&song_props::SongPropertiesState> {
        match self {
            Self::SongProperties(sp) => Some(sp),
            _ => None,
        }
    }

    pub(crate) fn song_props_mut(&mut self) -> Option<&mut song_props::SongPropertiesState> {
        match self {
            Self::SongProperties(sp) => Some(sp),
            _ => None,
        }
    }

    pub(crate) fn take_song_props(&mut self) -> Option<song_props::SongPropertiesState> {
        if matches!(self, Self::SongProperties(_)) {
            match std::mem::take(self) {
                Self::SongProperties(sp) => Some(sp),
                _ => unreachable!(),
            }
        } else {
            None
        }
    }

    pub(crate) fn mini_editor(&self) -> Option<&mini_editor::MiniEditorState> {
        match self {
            Self::MiniEditor(me) => Some(me),
            _ => None,
        }
    }

    pub(crate) fn take_mini_editor(&mut self) -> Option<Box<mini_editor::MiniEditorState>> {
        if matches!(self, Self::MiniEditor(_)) {
            match std::mem::take(self) {
                Self::MiniEditor(me) => Some(me),
                _ => unreachable!(),
            }
        } else {
            None
        }
    }

    pub(crate) fn palette(&self) -> Option<&palette::PaletteState> {
        match self {
            Self::Palette(p) => Some(p),
            _ => None,
        }
    }

    pub(crate) fn palette_mut(&mut self) -> Option<&mut palette::PaletteState> {
        match self {
            Self::Palette(p) => Some(p),
            _ => None,
        }
    }

    pub(crate) fn take_palette(&mut self) -> Option<Box<palette::PaletteState>> {
        if matches!(self, Self::Palette(_)) {
            match std::mem::take(self) {
                Self::Palette(p) => Some(p),
                _ => unreachable!(),
            }
        } else {
            None
        }
    }

    pub(crate) fn has_search_origin(&self) -> bool {
        matches!(self, Self::Search { origin: Some(_) })
    }

    pub(crate) fn take_search_origin(&mut self) -> Option<Navigation> {
        if matches!(self, Self::Search { .. }) {
            match std::mem::take(self) {
                Self::Search { origin } => origin,
                _ => unreachable!(),
            }
        } else {
            None
        }
    }
}

/// Hardware MIDI ports, learn/record arms, live CC snapshots, tap-tempo, and feedback diff state.
pub struct MidiState {
    /// Connected MIDI input port, set by the frontend.
    pub port: Option<String>,
    /// Connected MIDI output port for LED/OLED feedback, set by the frontend.
    pub out_port: Option<String>,
    /// Discovered OS MIDI input port names (updated on startup & rescan).
    pub in_ports: Vec<String>,
    /// Discovered OS MIDI output port names (updated on startup & rescan).
    pub out_ports: Vec<String>,
    /// Custom loaded MIDI device profile (overrides built-in `config.midi_profile` when set).
    pub custom_profile: Option<cadence_core::midi_map::MidiProfile>,
    /// Armed 1-key MIDI Learn target (`M`).
    pub learn: Option<cadence_core::midi_map::MidiTarget>,
    /// Live MIDI note recording (`R`: step-entry when stopped, quantized overdub when playing).
    pub record: bool,
    pub(crate) last_rec_step: Option<u64>,
    /// Selected mapping row in the MIDI Devices tab (`gd`).
    pub cursor: usize,
    /// Recent incoming MIDI activity monitor (newest first).
    pub log: VecDeque<midi::MidiLogEntry>,
    /// Live incoming CC value snapshot (`0..=127`) per CC number.
    pub cc_live: [Option<u8>; 128],
    pub(crate) pickup_latched: HashMap<u64, bool>,
    pub(crate) tap_instants: VecDeque<Instant>,
    pub(crate) feedback_cache: midi::MidiFeedbackCache,
    /// Last MIDI message received, for display.
    pub last: Option<String>,
}

impl Default for MidiState {
    fn default() -> Self {
        Self {
            port: None,
            out_port: None,
            in_ports: Vec::new(),
            out_ports: Vec::new(),
            custom_profile: None,
            learn: None,
            record: false,
            last_rec_step: None,
            cursor: 0,
            log: midi::new_midi_log(),
            cc_live: [None; 128],
            pickup_latched: HashMap::new(),
            tap_instants: VecDeque::new(),
            feedback_cache: midi::MidiFeedbackCache::default(),
            last: None,
        }
    }
}

/// Session take recording, range-marking, disk journal paths, and active growing-keeps state.
pub struct TakeSession {
    pub cursor_bar: usize,
    pub mark_start: Option<usize>,
    pub mark_end: Option<usize>,
    /// Active Vim-style visual anchor bar in the Take deck (`v`).
    pub visual_anchor: Option<usize>,
    /// Explicit take recording arm flag (initialized from `config.auto_record_takes`, default `false`).
    pub armed: bool,
    pub(crate) saved: bool,
    pub(crate) active: Option<cadence_core::take::TakeJournal>,
    pub(crate) last: Option<cadence_core::take::Take>,
    pub(crate) last_sealed_path: Option<PathBuf>,
    pub(crate) session_dir: Option<PathBuf>,
    pub(crate) growing_keeps: Vec<overrides::GrowingKeep>,
    pub(crate) growing_keep_revision: Option<u64>,
}

impl TakeSession {
    pub(crate) fn new(armed: bool) -> Self {
        Self {
            cursor_bar: 0,
            mark_start: None,
            mark_end: None,
            visual_anchor: None,
            armed,
            saved: false,
            active: None,
            last: None,
            last_sealed_path: None,
            session_dir: None,
            growing_keeps: Vec::new(),
            growing_keep_revision: None,
        }
    }
}

/// Discovered sample/preset/groove catalogs, decoded sample bank, asset issues, and background sample loader.
pub struct AssetCatalogs {
    pub library: SampleLibrary,
    pub(crate) presets: PresetCatalog,
    pub(crate) filter_grooves: FilterGrooveCatalog,
    pub(crate) custom_presets: bool,
    pub(crate) samples: HashMap<Id, Arc<SampleData>>,
    pub(crate) issues: Vec<SampleIssue>,
    pub(crate) browser_loader: Option<browser::BrowserLoader>,
    pub(crate) browser_generation: u64,
}

impl AssetCatalogs {
    pub(crate) fn new(
        library: SampleLibrary,
        presets: PresetCatalog,
        filter_grooves: FilterGrooveCatalog,
    ) -> Self {
        Self {
            library,
            presets,
            filter_grooves,
            custom_presets: false,
            samples: HashMap::new(),
            issues: Vec::new(),
            browser_loader: None,
            browser_generation: 0,
        }
    }
}

/// Settings overlay cursor/editing flags and global config dirty/auto-persist state.
#[derive(Clone, Debug, Default)]
pub struct SettingsUiState {
    pub cursor: usize,
    pub(crate) editing: Option<(SettingsDirKind, String)>,
    pub config_dirty: bool,
    pub(crate) persist_on_change: bool,
}

impl SettingsUiState {
    pub(crate) fn clear_editing(&mut self) {
        self.editing = None;
    }
}

/// Help modal tab/search state, shared overlay scroll cursor, and tutorial step index.
#[derive(Clone, Debug, Default)]
pub struct HelpUiState {
    pub tab: usize,
    pub query: String,
    pub searching: bool,
    /// Scroll cursor for Help / Inspect / Assets overlays (isolated from deck `control_cursor`).
    pub overlay_scroll: usize,
    pub tutorial_step: usize,
}

impl HelpUiState {
    pub(crate) fn clear_search(&mut self) {
        self.searching = false;
        self.query.clear();
    }
}

/// Focused bottom-toolbar control inside the dedicated Sample / Waveform deck (`gw`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SampleToolbarField {
    #[default]
    Cursor,
    Sensitivity,
    MinGap,
    GridDivisions,
    LoopXfade,
    SnapZero,
}

impl SampleToolbarField {
    pub const ALL: [Self; 6] = [
        Self::Cursor,
        Self::Sensitivity,
        Self::MinGap,
        Self::GridDivisions,
        Self::LoopXfade,
        Self::SnapZero,
    ];

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|&f| f == self).unwrap_or(0)
    }

    pub fn step(self, delta: isize) -> Self {
        let len = Self::ALL.len() as isize;
        let next = (self.index() as isize + delta).rem_euclid(len) as usize;
        Self::ALL[next]
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Cursor => "CURSOR",
            Self::Sensitivity => "SENSITIVITY",
            Self::MinGap => "MIN GAP",
            Self::GridDivisions => "GRID",
            Self::LoopXfade => "LOOP XFADE",
            Self::SnapZero => "SNAP",
        }
    }
}

/// Dedicated Sample / Waveform deck (`gw`) viewport, slicing analysis parameters, and edit cursor.
#[derive(Clone, Debug)]
pub struct SampleDeckUiState {
    pub cursor_frame: usize,
    /// Horizontal waveform zoom (`1.0` = full sample, up to `128.0` = fine sample inspection).
    pub zoom: f32,
    /// Leftmost visible frame index for smooth horizontal scrolloff.
    pub view_start_frame: Cell<usize>,
    /// Transient detector sensitivity (`0.05..=1.0`, higher detects softer hits).
    pub sensitivity: f32,
    /// Minimum refractory gap between transient slices in milliseconds (`10..=500`).
    pub min_gap_ms: f32,
    /// Equal-grid slice count (`2..=32`).
    pub grid_divisions: usize,
    /// Snap new/adjusted crop, loop, and slice boundaries to nearest zero-crossing.
    pub snap_zero: bool,
    /// Active bottom-toolbar control (`j`/`k` cycles).
    pub toolbar_field: SampleToolbarField,
    /// Last projected `(instrument, zone, module)` target so switching targets resets zoom/cursor cleanly.
    pub last_target: Cell<Option<(Id, u16, u16)>>,
}

impl Default for SampleDeckUiState {
    fn default() -> Self {
        Self {
            cursor_frame: 0,
            zoom: 1.0,
            view_start_frame: Cell::new(0),
            sensitivity: 0.55,
            min_gap_ms: 50.0,
            grid_divisions: 8,
            snap_zero: true,
            toolbar_field: SampleToolbarField::Cursor,
            last_target: Cell::new(None),
        }
    }
}

/// Instrument Rack deck tab, Panel module pill cursor, Zone cursor, and 2D Patchbay selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RackUiState {
    pub tab: RackTab,
    pub panel_slot: usize,
    pub zone_cursor: usize,
    pub patchbay_col: usize,
    pub patchbay_row: usize,
    pub patchbay_cable_id: Option<u16>,
}

/// Arrange deck mode, paging/zoom, playhead/event selection, pitch-bank anchor, and inspection cache.
pub struct ArrangeUiState {
    pub view: ArrangeView,
    /// Optional explicit page stride override; when `None`, paging uses `arrange_visible_bars() * 16`.
    pub page_steps: Option<usize>,
    /// Horizontal zoom factor for Arrange (`1.0` = `song_bars.min(16)` bars, down to `0.125` for 128 bars).
    pub zoom: f32,
    pub step: u32,
    /// Bar-aligned horizontal viewport anchor used for smooth scrolloff in Arrange.
    pub(crate) viewport_start_step: std::cell::Cell<usize>,
    pub event: Option<EventKey>,
    /// Parked pitch-bank anchor; observed transport never changes it.
    pub pitch_anchor: Option<u8>,
    pub(crate) pitch_lane: Option<Id>,
    pub(crate) source_cursor: usize,
    pub(crate) inspection_cache: RefCell<Option<arrangement::CachedArrangementInspection>>,
}

impl ArrangeUiState {
    pub(crate) fn new(selected_lane: Option<Id>) -> Self {
        Self {
            view: ArrangeView::Placements,
            page_steps: None,
            zoom: 1.0,
            step: 0,
            viewport_start_step: std::cell::Cell::new(0),
            event: None,
            pitch_anchor: None,
            pitch_lane: selected_lane,
            source_cursor: 0,
            inspection_cache: RefCell::new(None),
        }
    }
}

/// Deck-level view/cursor/zoom parameters and workspace chrome visibility flags.
#[derive(Clone, Debug)]
pub struct ViewState {
    /// Horizontal zoom factor for Pattern (`1.0` = `length.clamp(1, 16)` steps, bounded by `4..=64` steps).
    pub pattern_zoom: f32,
    /// Focused vertical row inside the selected Mixer channel strip (`Fader → Pan → Inserts → Sends → Output → Ducks`).
    pub mixer_slot: MixerSlot,
    /// Number of scrollable bank channel strips visible in the current Mixer viewport, supplied by the frontend.
    pub mixer_page_size: usize,
    /// Number of visible detail rows in the Mixer's fixed-height inspector stack, supplied by the frontend.
    pub mixer_detail_rows: usize,
    /// Leftmost visible bank channel strip index for smooth horizontal Mixer scrolling.
    pub mixer_scroll: Cell<usize>,
    /// Top visible detail row index for smooth vertical scrolling inside the Mixer detail stack.
    pub mixer_detail_scroll: Cell<usize>,
    /// Last channel whose Mixer detail stack was projected, so switching channels resets detail scroll cleanly.
    pub mixer_detail_channel: Cell<Option<ChannelRef>>,
    pub bank_mode: BankMode,
    pub bank_lane_idx: usize,
    pub bank_part_idx: usize,
    /// Selected column (`0..5`) and row in the Relationship Graph deck (`gG`).
    pub graph_col: usize,
    pub graph_row: usize,
    /// Selected macro slot (`0..8`) in the Project Macros Deck (`gk`).
    pub macro_cursor: usize,
    /// Focused binding row inside the selected macro (`None` = Macro Dial, `Some(i)` = Binding row `i`).
    pub macro_binding_cursor: Option<usize>,
    pub zoomed: bool,
    pub deck_visible: bool,
    pub deck_pinned: bool,
    pub deck_zoomed: bool,
    pub tips: bool,
    pub monitor_visible: bool,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            pattern_zoom: 1.0,
            mixer_slot: MixerSlot::Fader,
            mixer_page_size: 4,
            mixer_detail_rows: 3,
            mixer_scroll: Cell::new(0),
            mixer_detail_scroll: Cell::new(0),
            mixer_detail_channel: Cell::new(None),
            bank_mode: BankMode::Material,
            bank_lane_idx: 0,
            bank_part_idx: 0,
            graph_col: 0,
            graph_row: 0,
            macro_cursor: 0,
            macro_binding_cursor: None,
            zoomed: false,
            deck_visible: false,
            deck_pinned: false,
            deck_zoomed: false,
            tips: true,
            monitor_visible: true,
        }
    }
}
