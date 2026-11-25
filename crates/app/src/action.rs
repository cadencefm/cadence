//! Typed actions: the one path every input takes into the session.
use super::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    GetState,
    SetArrangeView {
        view: ArrangeView,
    },
    SelectArrangementEvent {
        event: EventKey,
    },
    InspectArrangementEvent {
        #[serde(default)]
        event: Option<EventKey>,
    },
    InspectAssets,
    ReloadSamples,
    Inspect {
        #[serde(default)]
        pattern: Option<Id>,
    },
    Edit {
        edit: Edit,
    },
    Transaction {
        edits: Vec<Edit>,
    },
    CreatePattern {
        name: String,
        kind: PatternKind,
    },
    CreateLane {
        name: String,
    },
    CreateInstrument {
        name: String,
        sound: workflow::SoundChoice,
        #[serde(default)]
        channel: Option<Id>,
    },
    CreatePart {
        name: String,
        pattern: Id,
        instrument: Id,
        #[serde(default)]
        transpose: Option<i8>,
        #[serde(default)]
        velocity_scale: Option<f32>,
    },
    Place {
        part: Id,
        lane: Id,
        start_step: u32,
        length_steps: u32,
        #[serde(default)]
        phase: PhaseMode,
        #[serde(default)]
        offset_steps: i32,
    },
    PlacedPart {
        name: String,
        sound: workflow::SoundChoice,
        notes: Vec<Step>,
        #[serde(default)]
        start_step: u32,
        #[serde(default)]
        length_steps: Option<u32>,
        #[serde(default)]
        transpose: Option<i8>,
        #[serde(default)]
        velocity_scale: Option<f32>,
    },
    Fork {
        source: Id,
        name: String,
    },
    Freeze {
        source: Id,
        name: String,
        start: i64,
        length: usize,
    },
    SelectPattern {
        pattern: Id,
    },
    SelectLane {
        lane: Id,
    },
    SelectPlacement {
        placement: Id,
    },
    SelectInstrument {
        instrument: Id,
    },
    SelectChannel {
        channel: ChannelRef,
    },
    SetParam {
        address: ParamAddress,
        value: f32,
    },
    SetRack {
        instrument: Id,
        rack: RackPatch,
    },
    RackAddModule {
        instrument: Id,
        zone: u16,
        kind: ModuleKindTag,
        #[serde(default)]
        name: Option<String>,
    },
    RackRemoveModule {
        instrument: Id,
        zone: u16,
        module: u16,
    },
    RackRenameModule {
        instrument: Id,
        zone: u16,
        module: u16,
        name: String,
    },
    RackConnectCable {
        instrument: Id,
        zone: u16,
        from_module: u16,
        to_module: u16,
        to_port: InPort,
        amount: f32,
    },
    RackDisconnectCable {
        instrument: Id,
        zone: u16,
        cable: u16,
    },
    RackPinControl {
        instrument: Id,
        pin: PanelPin,
    },
    RackUnpinControl {
        instrument: Id,
        index: usize,
    },
    LoadPreset {
        instrument: Id,
        preset: String,
    },
    RenameInstrument {
        instrument: Id,
        name: String,
    },
    RenamePattern {
        pattern: Id,
        name: String,
    },
    RenamePart {
        part: Id,
        name: String,
    },
    RenameLane {
        lane: Id,
        name: String,
    },
    ReorderLane {
        lane: Id,
        target_index: usize,
    },
    DeleteInstrument {
        instrument: Id,
    },
    CreateChannel {
        name: String,
        kind: ChannelKind,
    },
    RenameChannel {
        channel: Id,
        name: String,
    },
    DeleteChannel {
        channel: Id,
    },
    AddEffect {
        channel: ChannelRef,
        kind: String,
    },
    DeleteEffect {
        device: Id,
    },
    MoveEffect {
        device: Id,
        channel: ChannelRef,
        position: usize,
    },
    SetEffectBypass {
        device: Id,
        bypassed: bool,
    },
    RenameEffect {
        device: Id,
        name: String,
    },
    RouteInstrument {
        instrument: Id,
        channel: Id,
    },
    SetLaneMuted {
        lane: Id,
        muted: bool,
    },
    SetLaneSolo {
        lane: Id,
        solo: bool,
    },
    SetChannelMuted {
        channel: ChannelRef,
        muted: bool,
    },
    SetChannelSolo {
        channel: ChannelRef,
        solo: bool,
    },
    SetChannelOutput {
        channel: Id,
        output: ChannelRef,
    },
    SetChannelSend {
        channel: Id,
        target: Id,
        amount: Option<f32>,
    },
    CreateSection {
        name: String,
        start_step: u32,
        length_steps: u32,
    },
    CreateModulation {
        name: String,
        target: ModulationTarget,
        pattern: Id,
    },
    CreateDuck {
        name: String,
        source_part: Id,
        target: ChannelRef,
        depth: f32,
        recovery_ms: f32,
    },
    ClearMeters {
        #[serde(default)]
        channel: Option<ChannelRef>,
    },
    Audition {
        pattern: Id,
        #[serde(default)]
        length: Option<usize>,
    },
    StopAudition,
    Play {
        playing: bool,
    },
    Stop,
    /// Silence all ringing voices & FX tails immediately at the current step and clear overload latches.
    AudioPanic,
    /// Request the host to restart/reopen the active hardware audio stream.
    RestartAudio,
    /// Request the host to rescan hot-plugged OS audio and MIDI devices.
    RescanAudio,
    RestartLoop,
    CueSelection {
        #[serde(default)]
        play: bool,
    },
    CueSong {
        step: usize,
        #[serde(default)]
        play: bool,
    },
    SetMonitor {
        visible: bool,
    },
    SetFollow {
        enabled: bool,
    },
    ToggleFollow,
    SetDeck {
        visible: bool,
    },
    PinDeck {
        pinned: bool,
    },
    CueBar {
        bar: usize,
    },
    Undo,
    Redo,
    Save {
        path: PathBuf,
    },
    Load {
        path: PathBuf,
    },
    ImportRbs {
        path: PathBuf,
    },
    New {
        #[serde(default)]
        title: Option<String>,
    },
    Export {
        path: PathBuf,
        #[serde(default)]
        bars: Option<u32>,
    },
    AddLanePart {
        lane: Id,
        entry: LanePart,
    },
    DropLanePart {
        lane: Id,
        part: Id,
    },
    /// Launch a part on a lane at the launch boundary, or at `at`.
    Launch {
        lane: Id,
        part: LanePart,
        #[serde(default)]
        at: Option<cadence_core::material::Boundary>,
    },
    Release {
        lane: Id,
        #[serde(default)]
        at: Option<cadence_core::material::Boundary>,
    },
    ReleaseAll {
        #[serde(default)]
        at: Option<cadence_core::material::Boundary>,
    },
    /// Release a playing lane, or relaunch its last part.
    ToggleLane {
        lane: Id,
        #[serde(default)]
        at: Option<cadence_core::material::Boundary>,
    },
    CancelPending {
        #[serde(default)]
        all: bool,
    },
    /// Headless transport: advance the transport as if it played `steps`.
    Advance {
        steps: u64,
    },
    /// Explicitly arm or disarm live take recording.
    SetTakeArmed {
        armed: bool,
    },
    /// Toggle live take recording arm state (`tr`).
    ToggleTakeArmed,
    /// Discard the active or loaded take and clear Take deck marks.
    DiscardTake,
    /// Save the active (or most recently sealed) take to a file.
    SaveTake {
        #[serde(default)]
        path: Option<PathBuf>,
    },
    /// Load a saved `.take.jsonl` performance file into the Take deck (`gt`) via `HostIo`.
    LoadTake {
        path: PathBuf,
    },
    /// Keep an inclusive 1-based bar range from the active/last take into the arrangement.
    KeepRange {
        start_bar: usize,
        end_bar: usize,
    },
    /// Collect live gestures without letting them land.
    Stage,
    FinishStaging {
        choice: StageChoice,
    },
    /// Rack zone navigation and authoring actions.
    ZoneSelect {
        #[serde(default)]
        instrument: Option<Id>,
        zone: String,
    },
    ZoneStep {
        #[serde(default)]
        instrument: Option<Id>,
        delta: isize,
    },
    ZoneAdd {
        #[serde(default)]
        instrument: Option<Id>,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        midi: Option<u8>,
        #[serde(default)]
        sample_path: Option<String>,
    },
    ZoneDuplicate {
        #[serde(default)]
        instrument: Option<Id>,
        #[serde(default)]
        name: Option<String>,
    },
    ZoneRemove {
        #[serde(default)]
        instrument: Option<Id>,
    },
    ZoneSetSample {
        #[serde(default)]
        instrument: Option<Id>,
        path: String,
    },
    ZoneLoadPreset {
        #[serde(default)]
        instrument: Option<Id>,
        preset: String,
    },
    ZoneRename {
        #[serde(default)]
        instrument: Option<Id>,
        name: String,
    },
    ZoneSetRange {
        #[serde(default)]
        instrument: Option<Id>,
        low_midi: u8,
        high_midi: u8,
        #[serde(default)]
        root_midi: Option<u8>,
    },
    ZoneSetVelocity {
        #[serde(default)]
        instrument: Option<Id>,
        low_vel: f32,
        high_vel: f32,
    },
    ZoneSetChoke {
        #[serde(default)]
        instrument: Option<Id>,
        choke_group: Option<u8>,
    },
    ZoneImportFolder {
        #[serde(default)]
        instrument: Option<Id>,
        folder: String,
    },
    /// Update crop, loop mode/bounds, and loop crossfade on a specific Sampler module.
    SetSamplerRegion {
        instrument: Id,
        zone: u16,
        module: u16,
        #[serde(default)]
        sample_start: Option<usize>,
        #[serde(default)]
        sample_end: Option<usize>,
        #[serde(default)]
        loop_mode: Option<cadence_core::rack::SampleLoopMode>,
        #[serde(default)]
        loop_start: Option<usize>,
        #[serde(default)]
        loop_end: Option<usize>,
        #[serde(default)]
        loop_xfade_ms: Option<f32>,
    },
    /// Replace the persisted slice markers on a specific Sampler module.
    SetSamplerSlices {
        instrument: Id,
        zone: u16,
        module: u16,
        slices: Vec<cadence_core::sampler::SampleSlice>,
    },
    /// Convert a Sampler module's slice markers into a multi-zone chromatic Rack Kit
    /// (with optional sequential slice Pattern + Arrange Placement in one undoable transaction).
    SliceToRackKit {
        instrument: Id,
        zone: u16,
        module: u16,
        #[serde(default)]
        replace_existing: bool,
        #[serde(default)]
        create_pattern: bool,
        #[serde(default)]
        choke_group: Option<u8>,
        #[serde(default)]
        kit_name: Option<String>,
    },
    /// Losslessly write a Sampler module's loop/root/cue slice markers into its RIFF WAV file on disk.
    WriteSampleRiffMetadata {
        instrument: Id,
        zone: u16,
        module: u16,
    },
    /// Resample / bounce a part, pattern, or bar range to a portable WAV + playable `Sampler`
    /// or directly into a Rack zone.
    Bounce {
        #[serde(default)]
        scope: Option<BounceScope>,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        create_part: bool,
        #[serde(default)]
        target: Option<BounceTarget>,
    },
    /// Toggle live hardware audio recording (`Idle` -> arm/start, `Armed` -> disarm, `Recording` -> stop).
    ToggleAudioRecord,
    /// Arm or start live hardware audio input recording with explicit or smart-default options.
    StartAudioRecord {
        #[serde(default)]
        mode: Option<RecordingMode>,
        #[serde(default)]
        target: Option<BounceTarget>,
        #[serde(default)]
        place_in_arrange: Option<bool>,
        #[serde(default)]
        bars: Option<usize>,
        #[serde(default)]
        name: Option<String>,
    },
    /// Stop an active audio recording cleanly and finalize the 24-bit WAV clip.
    StopAudioRecord,
    /// Cancel an armed or active audio recording immediately without writing a WAV.
    CancelAudioRecord,
    /// Cycle software input monitoring (`Off` -> `Auto (Armed)` -> `On`).
    CycleInputMonitor,
    /// Set an assignable Project Macro (`0..8`) normalized value (`0.0..=1.0`).
    SetMacro {
        index: u8,
        value: f32,
    },
    /// Rename (or initialize) an assignable Project Macro (`0..8`).
    RenameMacro {
        index: u8,
        name: String,
    },
    /// Bind a normalized target parameter range (`min..max` in `0.0..=1.0`, allowing `min > max`) onto a Project Macro.
    BindMacro {
        index: u8,
        target: ParamAddress,
        min: f32,
        max: f32,
    },
    /// Remove a binding from a Project Macro by 0-based binding index, or `None` to remove all bindings.
    UnbindMacro {
        index: u8,
        #[serde(default)]
        binding: Option<usize>,
    },
    /// Clear/remove a Project Macro slot (`0..8`).
    ClearMacro {
        index: u8,
    },
    /// Execute a Cadence Ex command line (same parser & undo stack as interactive `:cmd`).
    Command {
        line: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    #[serde(default)]
    pub id: Option<serde_json::Value>,
    #[serde(default)]
    pub expected_revision: Option<u64>,
    #[serde(default)]
    pub compact: bool,
    pub action: Action,
}
