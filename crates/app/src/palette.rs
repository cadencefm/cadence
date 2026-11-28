//! Fuzzy Action Palette (`:` and `Ctrl+Shift+p` -> `Mode::Palette`) with top-slot Ex command
//! passthrough for `--key-script` and direct command entry.
use super::*;
use cadence_core::config::ThemeId;
use cadence_core::music::{note_name, scale_label};

/// Single point of truth deciding whether `raw` should surface the top
/// `▶ Run command: <raw>` passthrough entry in the Fuzzy Action Palette.
pub(crate) fn is_recognized_command_line(raw: &str) -> bool {
    let mut parts = raw.split_whitespace();
    let Some(verb) = parts.next().and_then(commands::ExVerb::parse) else {
        return false;
    };
    verb.allows_zero_args() || parts.next().is_some()
}

#[derive(Clone, Debug)]
pub struct PaletteItemView {
    pub category: &'static str,
    pub label: String,
    pub detail: String,
    pub shortcut: &'static str,
    pub is_command: bool,
    pub selected: bool,
}

#[derive(Clone, Debug)]
pub struct PaletteView {
    pub query: String,
    pub items: Vec<PaletteItemView>,
    pub cursor: usize,
    pub total_matches: usize,
    pub command_passthrough: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct PaletteState {
    pub query: String,
    pub cursor: usize,
    pub user_moved: bool,
}

#[derive(Clone, Debug)]
enum PaletteAction {
    RunCommand(String),
    SongProperties {
        row: song_props::SongPropRow,
        edit: bool,
    },
    BrowsePresets,
    SavePreset,
    BrowseFilterGrooves,
    ForkFilterGroove,
    SaveFilterGroove,
    LoadFilterGrooveBank,
    RenameInstrument,
    DeleteInstrument,
    RenameChannel,
    RenameEffect,
    RenamePattern,
    RenamePart,
    RenameLane,
    ReloadSamples,
    ReloadPresets,
    BrowseSamples,
    SampleAutoSlice,
    SampleGridSlice,
    SampleSliceToKit,
    SampleWriteRiffMeta,
    SampleProcess(SampleEditOp),
    EditMiniNotation,
    GoDeck(layout::DeckKind),
    GoDevices(DevicesTab),
    GoInstrument,
    AudioPanic,
    RestartAudio,
    RescanAudio,
    ToggleOutputMonitor,
    ToggleJam,
    ToggleStage,
    KeepFocusedOverride,
    KeepAllOverrides,
    ReleaseFocusedLane,
    ReleaseAllLanes,
    LayoutPreset(u8),
    ToggleZoom,
    ZoomViewIn,
    ZoomViewOut,
    ZoomViewReset,
    PlayPause,
    Stop,
    CueBar,
    CancelAllPending,
    ToggleLoop,
    ToggleFollow,
    ToggleTakeArmed,
    DiscardTake,
    CycleEndRule,
    BounceSelection,
    ToggleAudioRecord,
    OpenRecordDialog,
    CycleInputMonitor,
    CreatePattern,
    DerivePattern,
    ForkPattern,
    FreezePattern,
    DeletePattern,
    OpenInspect,
    NewProject,
    Workflow(WorkflowKind),
    QuickAddEffect(&'static str),
    ClearMeters,
    OpenFileHub(FileHubTab),
    SaveMidiProfileHub,
    LoadMidiProfileHub,
    OpenSettings,
    OpenSettingsRow(SettingsCoreRow),
    ToggleKeyHints,
    ToggleTopBarCpu,
    CycleTopBarMeter,
    ToggleArrangeLaneMeters,
    SetTheme(ThemeId),
    OpenHelp,
    OpenAssets,
    OpenTutorial,
    Quit,
    Undo,
    Redo,
    JumpSection(Id),
    LoopSection(Id),
    OpenPattern(Id),
    OpenInstrument(Id),
    OpenPart(Id),
    OpenChannel(ChannelRef),
    OpenEffect(Id),
    OpenLane(Id),
    OpenModulation(Id),
    ReplacePlacement(Id, verbs::Replace),
}

#[derive(Clone, Debug)]
struct PaletteEntry {
    category: &'static str,
    label: String,
    detail: String,
    keywords: String,
    shortcut: &'static str,
    priority: i32,
    is_command: bool,
    action: PaletteAction,
}

/// Crisp subsequence fuzzy scorer (higher is better; `None` means no match).
/// Matches every whitespace-separated token in `query` against `haystack`, rewarding
/// exact prefix matches, word-boundary hits, and consecutive character runs.
fn fuzzy_score(query: &str, label: &str, extra: &str) -> Option<i32> {
    let q = query.trim();
    if q.is_empty() {
        return Some(0);
    }
    let label_lc = label.to_ascii_lowercase();
    let full_lc = format!("{label_lc} {}", extra.to_ascii_lowercase());
    let mut total = 0i32;

    for token in q.split_whitespace() {
        let tok = token.to_ascii_lowercase();
        if tok.is_empty() {
            continue;
        }
        if let Some(label_score) = score_token(&tok, &label_lc) {
            total += label_score + 40;
        } else {
            total += score_token(&tok, &full_lc)?;
        }
    }
    Some(total)
}

fn score_token(needle: &str, haystack: &str) -> Option<i32> {
    if haystack == needle {
        return Some(240);
    }
    if haystack.starts_with(needle) {
        return Some(190);
    }
    if let Some(pos) = haystack.find(needle) {
        let at_word_start = pos == 0
            || haystack[..pos]
                .chars()
                .next_back()
                .is_some_and(|c| !c.is_ascii_alphanumeric());
        return Some(if at_word_start { 150 } else { 110 } - (pos as i32).min(40));
    }
    // Subsequence match with word-boundary & adjacency bonuses.
    let hay_chars: Vec<char> = haystack.chars().collect();
    let mut h_idx = 0usize;
    let mut last_match: Option<usize> = None;
    let mut score = 40i32;
    for nc in needle.chars() {
        let mut found = None;
        for (i, &hc) in hay_chars.iter().enumerate().skip(h_idx) {
            if hc == nc {
                found = Some(i);
                break;
            }
        }
        let pos = found?;
        if last_match.is_some_and(|prev| prev + 1 == pos) {
            score += 14;
        }
        if pos == 0 || !hay_chars[pos - 1].is_ascii_alphanumeric() {
            score += 12;
        }
        score -= (pos.saturating_sub(h_idx) as i32).min(8);
        last_match = Some(pos);
        h_idx = pos + 1;
    }
    Some(score)
}

impl App {
    /// Open the Fuzzy Action Palette (`:` or `Ctrl+Shift+p`), optionally pre-populated with `initial`.
    pub(crate) fn open_palette(&mut self, initial: String, resume_mode: Option<Mode>) {
        self.modal = ActiveModal::Palette(Box::new(PaletteState {
            query: initial,
            cursor: 0,
            user_moved: false,
        }));
        self.input.count = None;
        self.input.resume_mode = resume_mode;
        self.input.mode = Mode::Palette;
    }

    fn build_palette_entries(&self) -> Vec<PaletteEntry> {
        let p = self.doc.project();
        let mut entries: Vec<PaletteEntry> =
            Vec::with_capacity(64 + p.sections.len() + p.patterns.len() + p.instruments.len());

        fn push(
            entries: &mut Vec<PaletteEntry>,
            category: &'static str,
            label: impl Into<String>,
            detail: impl Into<String>,
            keywords: impl Into<String>,
            shortcut: &'static str,
            action: PaletteAction,
        ) {
            entries.push(PaletteEntry {
                category,
                label: label.into(),
                detail: detail.into(),
                keywords: keywords.into(),
                shortcut,
                priority: 0,
                is_command: false,
                action,
            });
        }

        // 1. Song / Project Properties
        push(
            &mut entries,
            "SONG",
            "Song Properties",
            format!(
                "{} · {:.0} BPM · {} {} · {} bars",
                p.title,
                p.bpm,
                note_name(p.root_midi),
                scale_label(&p.scale),
                p.bars
            ),
            "project properties tempo bpm key scale bars end rule timing",
            "gP / Ctrl+Shift+i",
            PaletteAction::SongProperties {
                row: song_props::SongPropRow::Title,
                edit: false,
            },
        );
        push(
            &mut entries,
            "SONG",
            "Set Song Key & Scale",
            format!(
                "Currently {} {}",
                note_name(p.root_midi),
                scale_label(&p.scale)
            ),
            "song key scale root major minor dorian phrygian lydian mixolydian blues chromatic",
            "gP",
            PaletteAction::SongProperties {
                row: song_props::SongPropRow::KeyRoot,
                edit: false,
            },
        );
        push(
            &mut entries,
            "SONG",
            "Set Tempo (BPM)",
            format!("Currently {:.1} BPM", p.bpm),
            "song tempo bpm speed",
            "gP",
            PaletteAction::SongProperties {
                row: song_props::SongPropRow::Tempo,
                edit: false,
            },
        );
        push(
            &mut entries,
            "SONG",
            "Set Song Length (Bars)",
            format!("Currently {} bars", p.bars),
            "song length bars arrangement duration",
            "gP",
            PaletteAction::SongProperties {
                row: song_props::SongPropRow::Bars,
                edit: false,
            },
        );
        push(
            &mut entries,
            "SONG",
            "Rename Song",
            format!("Currently '{}'", p.title),
            "song title rename project",
            "gP",
            PaletteAction::SongProperties {
                row: song_props::SongPropRow::Title,
                edit: true,
            },
        );

        // 2. Sound / Instruments / Presets
        let active_inst_label = self
            .active_instrument()
            .or(self.selected_instrument)
            .and_then(|id| self.instrument(id))
            .map(|i| {
                let zones = i.rack.zones.len();
                let mods: usize = i.rack.zones.iter().map(|z| z.modules.len()).sum();
                format!("{} (Rack · {zones}z/{mods}m)", i.name)
            })
            .unwrap_or_else(|| "Active instrument".into());
        push(
            &mut entries,
            "SOUND",
            "Browse Instrument Presets",
            format!("Live non-destructive preview on {active_inst_label}"),
            "preset browser presets patch sound load",
            "P",
            PaletteAction::BrowsePresets,
        );
        if let Some(last) = entries.last_mut() {
            last.priority = 12;
        }
        push(
            &mut entries,
            "SOUND",
            "Save Instrument Preset",
            format!("Save {active_inst_label} to disk"),
            "save preset export patch json",
            "gf",
            PaletteAction::SavePreset,
        );
        push(
            &mut entries,
            "SOUND",
            "Rename Instrument",
            format!("Rename {active_inst_label}"),
            "rename instrument sound",
            "r",
            PaletteAction::RenameInstrument,
        );
        push(
            &mut entries,
            "SOUND",
            "Delete Instrument",
            format!("Delete {active_inst_label} (with dependency confirmation)"),
            "delete remove instrument rack sound",
            "gi: X",
            PaletteAction::DeleteInstrument,
        );
        push(
            &mut entries,
            "SOUND",
            "Reload External Presets",
            "Rescan configured preset directories and filter groove banks",
            "reload rescan refresh external presets catalog grooves",
            "P: R / gf: R",
            PaletteAction::ReloadPresets,
        );
        push(
            &mut entries,
            "SOUND",
            "Reload Project Samples",
            "Invalidate sample cache and reload all project audio files from disk",
            "reload rescan refresh project samples wav audio assets",
            "Assets: r",
            PaletteAction::ReloadSamples,
        );
        push(
            &mut entries,
            "SOUND",
            "Browse Sample Library",
            "Audition and assign audio samples",
            "samples browser audio wav kit",
            "B / gS / Ctrl+Shift+b",
            PaletteAction::BrowseSamples,
        );
        push(
            &mut entries,
            "SOUND",
            "Auto-Slice Sample Transients",
            "Detect percussive onsets and place slice markers in Sample deck",
            "slice auto transient onset detect chop drum break beat",
            "Sample: d",
            PaletteAction::SampleAutoSlice,
        );
        push(
            &mut entries,
            "SOUND",
            "Grid-Slice Sample Regions",
            "Divide cropped sample into equal zero-crossing-snapped slices",
            "slice grid equal divisions chop loop sample",
            "Sample: D",
            PaletteAction::SampleGridSlice,
        );
        push(
            &mut entries,
            "SOUND",
            "Slice Sample to Rack Kit",
            "Convert slice markers into a chromatic multi-zone Rack Kit (+ optional Pattern)",
            "slice kit zones drum pads chop convert pattern",
            "Sample: K",
            PaletteAction::SampleSliceToKit,
        );
        push(
            &mut entries,
            "SOUND",
            "Write Sample RIFF Cue/Loop Metadata",
            "Losslessly save slice cues and loop bounds into source WAV file",
            "sample riff cue labl smpl inst write save metadata wav",
            "Sample: W",
            PaletteAction::SampleWriteRiffMeta,
        );
        push(
            &mut entries,
            "SOUND",
            "Trim Sample Crop to New WAV",
            "Write cropped sample region into a new portable WAV in samples/ (undoable)",
            "sample trim crop cut export wav offline edit",
            "Sample: T",
            PaletteAction::SampleProcess(SampleEditOp::Trim),
        );
        push(
            &mut entries,
            "SOUND",
            "Normalize Sample Peak (-0.5 dBFS)",
            "Stereo-linked peak normalization into a new portable WAV in samples/",
            "sample normalize norm gain loud peak wav offline edit",
            "Sample: N",
            PaletteAction::SampleProcess(SampleEditOp::Normalize {
                target_dbfs: cadence_core::dsp::DEFAULT_NORMALIZE_TARGET_DBFS,
            }),
        );
        push(
            &mut entries,
            "SOUND",
            "Reverse Sample Audio & Slices",
            "Reverse audio frames, crop bounds, loop points, and slices into a new WAV",
            "sample reverse rev backwards flip wav offline edit",
            "Sample: e",
            PaletteAction::SampleProcess(SampleEditOp::Reverse),
        );
        push(
            &mut entries,
            "SOUND",
            "Fade In Sample",
            "Apply raised-cosine (Hann) fade-in to active crop into a new portable WAV",
            "sample fade in ramp attack smooth wav offline edit",
            "Sample: e",
            PaletteAction::SampleProcess(SampleEditOp::FadeIn { duration_ms: None }),
        );
        push(
            &mut entries,
            "SOUND",
            "Fade Out Sample",
            "Apply raised-cosine (Hann) fade-out to active crop into a new portable WAV",
            "sample fade out tail decay smooth wav offline edit",
            "Sample: e",
            PaletteAction::SampleProcess(SampleEditOp::FadeOut { duration_ms: None }),
        );
        push(
            &mut entries,
            "SOUND",
            "New Instrument",
            "Create unified Rack instrument (Synth, Sampler, Kit, or Modular)",
            "new create instrument rack synth sampler kit modular",
            "ni",
            PaletteAction::Workflow(WorkflowKind::Instrument),
        );

        // 3. Transport & Takes
        push(
            &mut entries,
            "TRANSPORT",
            if self.transport.playing {
                "Pause Playback"
            } else {
                "Play Song"
            },
            "Toggle song playback",
            "play pause transport start",
            "Space",
            PaletteAction::PlayPause,
        );
        push(
            &mut entries,
            "TRANSPORT",
            "Stop & Return to Start",
            "Stop transport and reset playhead to bar 1",
            "stop reset home rewind",
            "ts / Home",
            PaletteAction::Stop,
        );
        push(
            &mut entries,
            "TRANSPORT",
            if self.takes.armed {
                "Disarm Take Recording"
            } else {
                "Arm Take Recording"
            },
            "Toggle take recording on playback",
            "arm take disarm record performance",
            "tr",
            PaletteAction::ToggleTakeArmed,
        );
        push(
            &mut entries,
            "TRANSPORT",
            "Discard Active Take",
            "Clear current unsaved performance take",
            "discard take delete clear",
            "gt X",
            PaletteAction::DiscardTake,
        );
        push(
            &mut entries,
            "TRANSPORT",
            "Toggle Loop Region",
            "Enable or disable song loop",
            "loop cycle repeat",
            "\\",
            PaletteAction::ToggleLoop,
        );
        push(
            &mut entries,
            "TRANSPORT",
            "Toggle Follow Playhead",
            "Auto-scroll workspace with playback",
            "follow playhead scroll",
            "tf",
            PaletteAction::ToggleFollow,
        );
        push(
            &mut entries,
            "TRANSPORT",
            "Cycle Song End Rule",
            format!("Currently {:?}", p.end),
            "end rule loop stop continue",
            "te",
            PaletteAction::CycleEndRule,
        );
        push(
            &mut entries,
            "TRANSPORT",
            "Bounce Selection to Audio Sample",
            "Render selection offline into a Sampler instrument",
            "bounce resample wav freeze audio",
            "tb",
            PaletteAction::BounceSelection,
        );
        push(
            &mut entries,
            "TRANSPORT",
            "Record Audio Input (Toggle Arm / Stop)",
            "Arm quantized audio input capture or stop & save 24-bit WAV",
            "record rec audio mic input sample capture",
            "ta",
            PaletteAction::ToggleAudioRecord,
        );
        push(
            &mut entries,
            "TRANSPORT",
            "Record Audio Input Dialog",
            "Configure mode (quantized/count-in/free), bars, target, and name",
            "record rec audio dialog zone bars countin free",
            "tA",
            PaletteAction::OpenRecordDialog,
        );
        push(
            &mut entries,
            "TRANSPORT",
            "Cycle Audio Input Monitor (Off / Auto / On)",
            "Toggle live software input monitoring in the audio engine",
            "monitor input mic audio live listen",
            "ti",
            PaletteAction::CycleInputMonitor,
        );

        // 4. Authoring & Mixing Workflows
        if let Some(pid) = match self.detail {
            Detail::Placement(id) => Some(id),
            _ => self.selected_placement,
        } {
            push(
                &mut entries,
                "AUTHOR",
                "Replace Placement Pattern",
                "Swap the pattern played by the focused placement",
                "replace placement pattern swap",
                "r",
                PaletteAction::ReplacePlacement(pid, verbs::Replace::Pattern),
            );
            push(
                &mut entries,
                "AUTHOR",
                "Replace Placement Instrument / Sound",
                "Swap the instrument played by the focused placement",
                "replace placement instrument sound swap",
                "r (on Instrument)",
                PaletteAction::ReplacePlacement(pid, verbs::Replace::Instrument),
            );
        }
        push(
            &mut entries,
            "AUTHOR",
            "Edit Mini-Notation",
            "Open live-validated Mini-Notation code editor",
            "mini notation tidal strudel code pattern",
            "Enter",
            PaletteAction::EditMiniNotation,
        );
        push(
            &mut entries,
            "AUTHOR",
            "New Pattern",
            "Create Notes, Rhythm, Values, Euclidean, or Mini pattern",
            "new create pattern notes rhythm values mini euclid",
            "n",
            PaletteAction::CreatePattern,
        );
        push(
            &mut entries,
            "AUTHOR",
            "Derive Pattern",
            "Create derived pattern from existing material",
            "derive transform pattern transpose mask arp swing",
            "d",
            PaletteAction::DerivePattern,
        );
        push(
            &mut entries,
            "AUTHOR",
            "New Part",
            "Bind a pattern to an instrument",
            "new part bind pattern instrument",
            "nb",
            PaletteAction::Workflow(WorkflowKind::Part),
        );
        push(
            &mut entries,
            "AUTHOR",
            "New Lane",
            "Add an arrangement lane",
            "new lane track",
            "nl",
            PaletteAction::Workflow(WorkflowKind::Lane),
        );
        push(
            &mut entries,
            "AUTHOR",
            "Place Part on Arrangement",
            "Schedule a part onto a lane",
            "place part arrangement clip",
            "no",
            PaletteAction::Workflow(WorkflowKind::Place),
        );
        push(
            &mut entries,
            "AUTHOR",
            "New Placed Part",
            "Create Pattern, Instrument, Part, and Arrangement placement in one workflow",
            "new placed part quick create track clip arrangement",
            "np",
            PaletteAction::Workflow(WorkflowKind::PlacedPart),
        );
        push(
            &mut entries,
            "AUTHOR",
            "Rename Pattern",
            "Edit the selected pattern's name and metadata",
            "rename edit pattern material",
            "gp: r",
            PaletteAction::RenamePattern,
        );
        push(
            &mut entries,
            "AUTHOR",
            "Rename Part",
            "Rename the selected part",
            "rename part",
            "Part: r",
            PaletteAction::RenamePart,
        );
        push(
            &mut entries,
            "AUTHOR",
            "Rename Lane",
            "Rename the selected arrangement lane",
            "rename lane track",
            "ga: r",
            PaletteAction::RenameLane,
        );
        push(
            &mut entries,
            "MIXER",
            "Rename Channel",
            format!("Rename {}", self.channel_name(self.selected_channel)),
            "rename channel mixer bus strip",
            "gm: r",
            PaletteAction::RenameChannel,
        );
        push(
            &mut entries,
            "MIXER",
            "Rename Effect",
            "Rename the focused or active insert effect device",
            "rename effect fx insert",
            "FX: r",
            PaletteAction::RenameEffect,
        );
        push(
            &mut entries,
            "AUTHOR",
            "New Section",
            "Define a named musical section",
            "new section marker verse chorus drop",
            "nt",
            PaletteAction::Workflow(WorkflowKind::Section),
        );
        push(
            &mut entries,
            "MIXER",
            "New Mixer Channel",
            "Add an Audio, Group, or Return channel",
            "new channel bus group return",
            "nc",
            PaletteAction::Workflow(WorkflowKind::Channel),
        );
        push(
            &mut entries,
            "MIXER",
            "Add Effect (Choose Kind)",
            format!(
                "Insert FX onto {}",
                self.channel_name(self.selected_channel)
            ),
            "add effect fx insert plugin",
            "nf",
            PaletteAction::Workflow(WorkflowKind::Effect),
        );
        for (fx_kind, fx_label, fx_detail) in [
            (
                "reverb",
                "Add Effect: Reverb",
                "Zero-delay 96-mode ambient modal room reverb",
            ),
            (
                "delay",
                "Add Effect: Delay",
                "Tempo-synced ping-pong stereo delay",
            ),
            (
                "filter",
                "Add Effect: Filter",
                "Resonant LP12–24 / Diode / SVF filter with rhythmic envelope",
            ),
            ("eq", "Add Effect: EQ", "3-band parametric equalizer"),
            (
                "compressor",
                "Add Effect: Compressor",
                "Dynamics bus compressor with gain reduction",
            ),
            (
                "drive",
                "Add Effect: Drive",
                "Cubic 1x waveshaper (Atomic Jet / Shock Port) & tone",
            ),
            (
                "chorus",
                "Add Effect: Chorus",
                "Modulated stereo chorus widener",
            ),
            (
                "limiter",
                "Add Effect: Limiter",
                "Lookahead brickwall peak limiter",
            ),
        ] {
            push(
                &mut entries,
                "FX",
                fx_label,
                format!(
                    "Insert on {} · {fx_detail}",
                    self.channel_name(self.selected_channel)
                ),
                format!("fx effect insert {fx_kind} {fx_detail}"),
                "nf",
                PaletteAction::QuickAddEffect(fx_kind),
            );
        }
        push(
            &mut entries,
            "FX",
            "Browse Filter Grooves",
            format!(
                "Live non-destructive Filter groove picker ({} grooves loaded)",
                self.assets.filter_grooves.len()
            ),
            "filter groove browser picker pcf rhythmic envelope pattern",
            "P",
            PaletteAction::BrowseFilterGrooves,
        );
        push(
            &mut entries,
            "FX",
            "Fork Filter Groove to Pattern",
            "Extract active Filter groove into an editable project Pattern",
            "fork filter groove pattern extract edit values",
            "f",
            PaletteAction::ForkFilterGroove,
        );
        push(
            &mut entries,
            "FX",
            "Save Filter Groove to Bank",
            "Export active Filter groove into presets/filter_grooves.json",
            "save export filter groove bank json custom share",
            "Palette",
            PaletteAction::SaveFilterGroove,
        );
        push(
            &mut entries,
            "FX",
            "Load Filter Groove Bank",
            "Pick any filter_grooves.json or *.grooves.json bank file (File Hub / Finder)",
            "load import open filter groove bank json file picker custom",
            "P: o",
            PaletteAction::LoadFilterGrooveBank,
        );
        push(
            &mut entries,
            "MIXER",
            "Add Channel Send",
            "Route an audio/group channel to a Return bus",
            "send return aux reverb delay bus",
            "nw",
            PaletteAction::Workflow(WorkflowKind::Send),
        );
        push(
            &mut entries,
            "MIXER",
            "Add Sidechain Duck",
            "Configure part-triggered sidechain ducking",
            "duck sidechain pump kick",
            "nu",
            PaletteAction::Workflow(WorkflowKind::Duck),
        );
        push(
            &mut entries,
            "MIXER",
            "Add Global Modulation",
            "Automate any instrument, FX, mixer, or macro parameter",
            "mod modulation automation curve",
            "ny",
            PaletteAction::Workflow(WorkflowKind::Modulation),
        );
        push(
            &mut entries,
            "MIXER",
            "Clear Clipped Meters",
            "Clear peak and clip holds across all channels and Master",
            "clear clips meters peak hold hot reset mixer output",
            "C / tC",
            PaletteAction::ClearMeters,
        );

        // 5. Decks & Layouts
        for (label, kw, sc, kind) in [
            (
                "Open Arrange Deck",
                "deck arrange timeline song",
                "ga / Ctrl+a",
                layout::DeckKind::Arrange,
            ),
            (
                "Open Bank Launchpad",
                "deck bank clips launchpad material",
                "gb / Ctrl+b",
                layout::DeckKind::Bank,
            ),
            (
                "Open Pattern Deck",
                "deck pattern piano roll notes values",
                "gp / Ctrl+p",
                layout::DeckKind::Pattern,
            ),
            (
                "Open Sample / Waveform Deck",
                "deck sample waveform slice chop cue riff loop crop",
                "gw",
                layout::DeckKind::Sample,
            ),
            (
                "Open Mixer Deck",
                "deck mixer console faders strips",
                "gm / Ctrl+m",
                layout::DeckKind::Mixer,
            ),
            (
                "Open Take Deck",
                "deck take performance recording",
                "gt / Ctrl+t",
                layout::DeckKind::Take,
            ),
            (
                "Open Signal Graph",
                "deck graph routing cables modular",
                "gG / Ctrl+g",
                layout::DeckKind::Graph,
            ),
            (
                "Open Macros Deck",
                "deck macros dials m1 m8",
                "gk / Ctrl+k",
                layout::DeckKind::Macros,
            ),
        ] {
            push(
                &mut entries,
                "DECK",
                label,
                "Focus workspace deck",
                kw,
                sc,
                PaletteAction::GoDeck(kind),
            );
        }
        push(
            &mut entries,
            "DECK",
            "Devices · Audio",
            "Configure audio output, input, sample rate, buffer size & engine health",
            "deck devices audio interface output input cue sample rate buffer latency health cpu xruns",
            "gd / Ctrl+d",
            PaletteAction::GoDevices(DevicesTab::Audio),
        );
        push(
            &mut entries,
            "DECK",
            "Open MIDI Deck",
            "Focus Devices · MIDI controller deck",
            "deck devices midi controller learn cc ports profile",
            "gd / Ctrl+d",
            PaletteAction::GoDevices(DevicesTab::Midi),
        );
        push(
            &mut entries,
            "AUDIO",
            "Audio Panic (Silence All)",
            "Immediately silence ringing voices & FX tails and clear overload latch",
            "audio panic silence all notes off reset overload stop tails",
            "Ctrl+d",
            PaletteAction::AudioPanic,
        );
        push(
            &mut entries,
            "AUDIO",
            "Restart Audio Engine",
            "Reopen the active hardware audio stream with current device settings",
            "audio restart engine stream recover device interface",
            "Ctrl+d",
            PaletteAction::RestartAudio,
        );
        push(
            &mut entries,
            "AUDIO",
            "Rescan Audio & MIDI Devices",
            "Re-enumerate hot-plugged OS audio interfaces and MIDI ports",
            "audio midi rescan devices ports refresh hotplug",
            "Ctrl+d: r",
            PaletteAction::RescanAudio,
        );
        push(
            &mut entries,
            "DECK",
            "Open Instrument Deck",
            format!("Inspect {active_inst_label}"),
            "deck instrument rack synth sampler kit modular sound",
            "gi / Ctrl+i",
            PaletteAction::GoInstrument,
        );
        push(
            &mut entries,
            "DECK",
            "Toggle Output Monitor Scope",
            "Show or hide final output waveform & peak meters",
            "output monitor scope meter",
            "go / Ctrl+o",
            PaletteAction::ToggleOutputMonitor,
        );
        for (preset, name, desc, sc) in [
            (1, "Layout: Compose", "Arrange + Pattern", "z1"),
            (2, "Layout: Perform", "Arrange + Bank + Output", "z2"),
            (
                3,
                "Layout: Sound Design",
                "Pattern + Instrument + Output",
                "z3",
            ),
            (4, "Layout: Mix", "Arrange + Mixer + Output", "z4"),
        ] {
            push(
                &mut entries,
                "LAYOUT",
                name,
                desc,
                format!("layout workspace preset {name}"),
                sc,
                PaletteAction::LayoutPreset(preset),
            );
        }
        push(
            &mut entries,
            "LAYOUT",
            "Toggle Zoom Focused Deck",
            "Expand or restore active deck full-screen",
            "zoom fullscreen maximize deck",
            "zz",
            PaletteAction::ToggleZoom,
        );
        push(
            &mut entries,
            "LAYOUT",
            "Zoom Timeline In",
            "Zoom in horizontal bars/steps in Arrange or Pattern",
            "zoom in timeline bars steps arrange pattern overview roll",
            "z= / Ctrl+=",
            PaletteAction::ZoomViewIn,
        );
        push(
            &mut entries,
            "LAYOUT",
            "Zoom Timeline Out",
            "Zoom out horizontal bars/steps in Arrange or Pattern",
            "zoom out timeline bars steps arrange pattern overview roll",
            "z- / Ctrl+-",
            PaletteAction::ZoomViewOut,
        );
        push(
            &mut entries,
            "LAYOUT",
            "Reset Timeline Zoom (100%)",
            "Reset Arrange or Pattern horizontal zoom back to 100%",
            "zoom reset default 100 timeline arrange pattern",
            "z0 / Ctrl+0",
            PaletteAction::ZoomViewReset,
        );

        // 6. Files, Live Performance, Theme, Settings & History
        push(
            &mut entries,
            "FILE",
            "New Blank Project",
            "Create an empty Cadence project using Settings defaults",
            "new blank empty project start fresh",
            "gf → NEW",
            PaletteAction::NewProject,
        );
        push(
            &mut entries,
            "FILE",
            "Open File Hub (Quick Open / Recent)",
            "Start a blank project or open recent projects, examples, and takes",
            "file hub open project load recent examples quick takes",
            "gf / Ctrl+Shift+o",
            PaletteAction::OpenFileHub(FileHubTab::Quick),
        );
        push(
            &mut entries,
            "FILE",
            "Browse Files & Folders",
            "Navigate directories, portable bundles, projects, and .rbs files on disk",
            "browse files folders directory filesystem open bundle",
            "gf → 2 Browse",
            PaletteAction::OpenFileHub(FileHubTab::Browse),
        );
        push(
            &mut entries,
            "FILE",
            "Import… (.rbs / Preset / MIDI / Take)",
            "Open File Hub Import tab for ReBirth .rbs, Instrument Presets, MIDI Profiles, or Takes",
            "import rbs preset midi profile take file hub",
            "gf → 4 Import",
            PaletteAction::OpenFileHub(FileHubTab::Import),
        );
        push(
            &mut entries,
            "FILE",
            "Save Project / Save As / Bundle",
            "Save project, portable bundle, preset, or MIDI profile",
            "save as bundle write project",
            "Ctrl+Shift+s",
            PaletteAction::OpenFileHub(FileHubTab::Save),
        );
        push(
            &mut entries,
            "FILE",
            "Save Active MIDI Profile",
            "Export active controller profile and user MIDI Learn mappings to .midi.json",
            "save export midi profile custom controller json",
            "gd: Ctrl+Shift+s",
            PaletteAction::SaveMidiProfileHub,
        );
        push(
            &mut entries,
            "FILE",
            "Load Custom MIDI Profile",
            "Import a custom .midi.json controller profile via File Hub",
            "load import midi profile custom controller json",
            "gd: Profile Enter",
            PaletteAction::LoadMidiProfileHub,
        );
        push(
            &mut entries,
            "FILE",
            "Export WAV / Stems",
            "Render 16-bit PCM master WAV and per-channel stems",
            "export wav stems render audio mixdown 16-bit pcm",
            "Ctrl+Shift+e",
            PaletteAction::OpenFileHub(FileHubTab::Export),
        );
        push(
            &mut entries,
            "TRANSPORT",
            "Cue Playback to Bar…",
            "Jump or cue transport playback directly to a 1-based bar number",
            "cue bar jump goto playhead transport",
            "tg",
            PaletteAction::CueBar,
        );
        push(
            &mut entries,
            "LIVE",
            if self.is_jam() {
                "Leave Jam Mode"
            } else {
                "Toggle Jam Mode"
            },
            "Open or close the floating live performance keyboard & launch surface",
            "toggle jam live keyboard perform surface",
            "gj / Ctrl+Shift+j",
            PaletteAction::ToggleJam,
        );
        push(
            &mut entries,
            "LIVE",
            if self.staging.is_some() {
                "Fire / Finish Staged Changes"
            } else {
                "Stage Live Changes"
            },
            "Start synchronized multi-lane staging or open Stage finish prompt",
            "stage staging fire synchronized live launch",
            "S / Alt+Enter",
            PaletteAction::ToggleStage,
        );
        push(
            &mut entries,
            "LIVE",
            "Keep Focused Live Override",
            "Commit the focused lane's live override (or Take selection) into the arrangement",
            "keep commit override focused lane live",
            "Alt+c",
            PaletteAction::KeepFocusedOverride,
        );
        push(
            &mut entries,
            "LIVE",
            "Keep All Live Overrides",
            "Atomically commit all active live lane overrides into the arrangement",
            "keep all commit overrides live arrangement",
            "Alt+C",
            PaletteAction::KeepAllOverrides,
        );
        push(
            &mut entries,
            "LIVE",
            "Release Focused Lane Override",
            "Release the focused lane back to the authored arrangement",
            "release focused lane override live",
            "Alt+x",
            PaletteAction::ReleaseFocusedLane,
        );
        push(
            &mut entries,
            "LIVE",
            "Release All Lane Overrides",
            "Release every live lane override back to the authored arrangement",
            "release all lanes overrides live",
            "Alt+X",
            PaletteAction::ReleaseAllLanes,
        );
        push(
            &mut entries,
            "LIVE",
            "Cancel All Pending Live Changes",
            "Cancel every queued lane launch, release, flag, or section jump",
            "cancel pending live queue clear all",
            "tX",
            PaletteAction::CancelAllPending,
        );
        push(
            &mut entries,
            "CREATE",
            "Fork Selected Pattern",
            "Duplicate the selected pattern into an independent copy",
            "fork pattern clone duplicate material",
            "f (Pattern)",
            PaletteAction::ForkPattern,
        );
        push(
            &mut entries,
            "CREATE",
            "Freeze Pattern to Literal…",
            "Bake a derived, Mini, Euclidean, or Seeded pattern into editable steps",
            "freeze bake flatten literal pattern",
            "F (Pattern)",
            PaletteAction::FreezePattern,
        );
        push(
            &mut entries,
            "CREATE",
            "Delete Selected Pattern…",
            "Remove the selected pattern with dependency confirmation",
            "delete remove pattern material",
            "X (Material)",
            PaletteAction::DeletePattern,
        );
        push(
            &mut entries,
            "APP",
            "Inspect Pattern Dependencies",
            "View full pattern dependency graph, consumers, and provenance",
            "inspect dependencies consumers material graph pattern",
            "I (Pattern)",
            PaletteAction::OpenInspect,
        );
        push(
            &mut entries,
            "APP",
            "Settings & Live Timing",
            "Configure theme, hints, default scale, sample/preset dirs, take arming",
            "settings preferences config directories",
            "g, / Ctrl+,",
            PaletteAction::OpenSettings,
        );
        push(
            &mut entries,
            "APP",
            "Toggle Contextual Key Hints",
            if self.view_state.tips {
                "Currently visible · hide contextual footer shortcut hints"
            } else {
                "Currently hidden · show contextual footer shortcut hints"
            },
            "toggle contextual key hints tips footer bar",
            "g,",
            PaletteAction::ToggleKeyHints,
        );
        push(
            &mut entries,
            "VIEW",
            "Toggle Top-Bar DSP CPU Pill",
            if self.config.show_top_bar_cpu {
                "Currently visible · hide DSP CPU pill beside FINAL"
            } else {
                "Currently hidden · show DSP CPU pill beside FINAL"
            },
            "toggle top bar dsp cpu load meter chrome monitoring",
            "g,",
            PaletteAction::ToggleTopBarCpu,
        );
        push(
            &mut entries,
            "VIEW",
            "Cycle Top-Bar Stereo Meter",
            format!(
                "Currently {} · cycle Auto (Output hidden) / Always / Off",
                self.config.top_bar_meter.label()
            ),
            "cycle top bar stereo meter compact final chrome output hidden",
            "g,",
            PaletteAction::CycleTopBarMeter,
        );
        push(
            &mut entries,
            "VIEW",
            "Toggle Arrange Lane Meters",
            if self.config.show_arrange_lane_meters {
                "Currently visible · hide vertical per-lane meter pills in Arrange"
            } else {
                "Currently hidden · show vertical per-lane meter pills in Arrange"
            },
            "toggle arrange lane meters vertical gutter level monitoring",
            "g,",
            PaletteAction::ToggleArrangeLaneMeters,
        );
        let default_scale_lbl = cadence_core::music::named_scale(&self.config.default_scale)
            .map(|s| scale_label(&s))
            .unwrap_or("Natural Minor");
        push(
            &mut entries,
            "APP",
            "Default New-Project Scale",
            format!("Currently {default_scale_lbl} · edit in Settings"),
            "default scale new project settings key",
            "g,",
            PaletteAction::OpenSettingsRow(SettingsCoreRow::DefaultScale),
        );
        for theme in [
            ThemeId::CadenceDark,
            ThemeId::MidnightNeon,
            ThemeId::StudioLight,
        ] {
            push(
                &mut entries,
                "THEME",
                format!("Theme: {}", theme.label()),
                "Switch UI color theme immediately",
                format!("theme appearance color dark light neon {}", theme.label()),
                "g,",
                PaletteAction::SetTheme(theme),
            );
        }
        push(
            &mut entries,
            "APP",
            "Keyboard Help & Cheatsheet",
            "Searchable reference of all decks, keys, and commands",
            "help cheatsheet keys bindings manual",
            "? / Ctrl+Shift+/",
            PaletteAction::OpenHelp,
        );
        push(
            &mut entries,
            "APP",
            "Inspect Sample Assets & Readiness",
            "Audit sample paths and missing audio files",
            "assets samples readiness check missing",
            "Palette",
            PaletteAction::OpenAssets,
        );
        push(
            &mut entries,
            "APP",
            "Interactive Tutorial",
            "Guided 10-step material-first walkthrough",
            "tutorial walkthrough guide learn",
            "Palette",
            PaletteAction::OpenTutorial,
        );
        push(
            &mut entries,
            "APP",
            "Quit Cadence",
            "Exit Cadence (prompts if unsaved changes exist)",
            "quit exit close leave bye q",
            "Ctrl+c",
            PaletteAction::Quit,
        );
        push(
            &mut entries,
            "EDIT",
            "Undo Last Change",
            "Step backward in undo history",
            "undo revert",
            "u / Ctrl+z",
            PaletteAction::Undo,
        );
        push(
            &mut entries,
            "EDIT",
            "Redo Change",
            "Step forward in redo history",
            "redo",
            "U / Ctrl+Shift+z",
            PaletteAction::Redo,
        );

        // 7. Dynamic Live Project Entries (Sections, Patterns, Instruments, Parts, Channels, Effects, Lanes, Modulations)
        for sec in &p.sections {
            let start_bar = sec.start_step / 16 + 1;
            let len_bars = sec.length_steps.div_ceil(16);
            push(
                &mut entries,
                "SECTION",
                format!("Jump Section: {}", sec.name),
                format!("Bar {start_bar} ({len_bars} bars)"),
                format!("jump section goto {}", sec.name),
                "Alt+1–0",
                PaletteAction::JumpSection(sec.id),
            );
            push(
                &mut entries,
                "SECTION",
                format!("Loop Section: {}", sec.name),
                format!(
                    "Set loop region to bars {start_bar}–{}",
                    start_bar + len_bars - 1
                ),
                format!("loop section repeat {}", sec.name),
                "tl",
                PaletteAction::LoopSection(sec.id),
            );
        }
        for pat in &p.patterns {
            push(
                &mut entries,
                "PATTERN",
                format!("Open Pattern: {}", pat.name),
                format!("{} pattern", pat.kind.label()),
                format!("open pattern material {} {}", pat.name, pat.kind.label()),
                "gp",
                PaletteAction::OpenPattern(pat.id),
            );
        }
        for inst in &p.instruments {
            let zones = inst.rack.zones.len();
            let mods: usize = inst.rack.zones.iter().map(|z| z.modules.len()).sum();
            let rack_desc = format!("Rack · {zones}z/{mods}m");
            push(
                &mut entries,
                "INSTRUMENT",
                format!("Open Instrument: {}", inst.name),
                format!(
                    "{rack_desc} · {}",
                    self.channel_name(ChannelRef::Channel(inst.channel))
                ),
                format!("open instrument rack sound {} {rack_desc}", inst.name),
                "gi",
                PaletteAction::OpenInstrument(inst.id),
            );
        }
        for part in &p.parts {
            let pat = self
                .pattern(part.pattern)
                .map_or("?", |pt| pt.name.as_str());
            let inst = self
                .instrument(part.instrument)
                .map_or("?", |i| i.name.as_str());
            push(
                &mut entries,
                "PART",
                format!("Open Part: {}", part.name),
                format!("{pat} → {inst}"),
                format!("open part detail {} {pat} {inst}", part.name),
                "Part",
                PaletteAction::OpenPart(part.id),
            );
        }
        for ch_ref in self.channels() {
            let ch_name = self.channel_name(ch_ref).to_string();
            let detail = match ch_ref {
                ChannelRef::Master => format!(
                    "Master bus · {} insert{}",
                    p.mixer.master.inserts.len(),
                    if p.mixer.master.inserts.len() == 1 {
                        ""
                    } else {
                        "s"
                    }
                ),
                ChannelRef::Channel(cid) => self.channel(cid).map_or_else(
                    || "Channel".into(),
                    |c| {
                        format!(
                            "{:?} → {} · {} FX",
                            c.kind,
                            self.channel_name(c.output),
                            c.inserts.len()
                        )
                    },
                ),
            };
            push(
                &mut entries,
                "CHANNEL",
                format!("Open Channel: {ch_name}"),
                detail,
                format!("open channel mixer strip bus {ch_name}"),
                "gm",
                PaletteAction::OpenChannel(ch_ref),
            );
        }
        for ch_ref in self.channels() {
            let ch_name = self.channel_name(ch_ref).to_string();
            let inserts = match ch_ref {
                ChannelRef::Master => &p.mixer.master.inserts,
                ChannelRef::Channel(cid) => match self.channel(cid) {
                    Some(c) => &c.inserts,
                    None => continue,
                },
            };
            for dev in inserts {
                let state = if dev.bypassed { " · bypassed" } else { "" };
                push(
                    &mut entries,
                    "EFFECT",
                    format!("Open Effect: {}", dev.name),
                    format!("{} on {ch_name}{state}", dev.kind.label()),
                    format!(
                        "open effect fx insert {} {} {ch_name}",
                        dev.name,
                        dev.kind.label()
                    ),
                    "FX",
                    PaletteAction::OpenEffect(dev.id),
                );
            }
        }
        for lane in &p.lanes {
            push(
                &mut entries,
                "LANE",
                format!("Open Lane: {}", lane.name),
                format!(
                    "{} placement{} · {} bank part{}",
                    lane.placements.len(),
                    if lane.placements.len() == 1 { "" } else { "s" },
                    lane.parts.len(),
                    if lane.parts.len() == 1 { "" } else { "s" }
                ),
                format!("open lane track arrange {}", lane.name),
                "ga",
                PaletteAction::OpenLane(lane.id),
            );
        }
        for m in &p.modulations {
            let pat = self.pattern(m.pattern).map_or("?", |pt| pt.name.as_str());
            let target = crate::drafts::format_modulation_target(p, m.target);
            push(
                &mut entries,
                "MODULATION",
                format!("Open Modulation: {}", m.name),
                format!("{pat} → {target}"),
                format!("open edit modulation automation {} {pat} {target}", m.name),
                "e",
                PaletteAction::OpenModulation(m.id),
            );
        }

        entries
    }

    fn filtered_palette_entries(&self, state: &PaletteState) -> (Vec<PaletteEntry>, bool) {
        let mut ranked: Vec<(i32, usize, PaletteEntry)> = self
            .build_palette_entries()
            .into_iter()
            .enumerate()
            .filter_map(|(idx, entry)| {
                let extra = format!(
                    "{} {} {} {}",
                    entry.category, entry.detail, entry.keywords, entry.shortcut
                );
                let score = fuzzy_score(&state.query, &entry.label, &extra)?;
                Some((score + entry.priority, idx, entry))
            })
            .collect();

        if !state.query.trim().is_empty() {
            ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        }

        let mut results: Vec<PaletteEntry> = ranked.into_iter().map(|(_, _, e)| e).collect();
        let trimmed = state.query.trim();
        let command_passthrough = is_recognized_command_line(trimmed);
        if command_passthrough {
            results.insert(
                0,
                PaletteEntry {
                    category: "CMD",
                    label: format!("▶ Run command: {trimmed}"),
                    detail: "Execute Ex command line directly".into(),
                    keywords: String::new(),
                    shortcut: "Enter",
                    priority: 0,
                    is_command: true,
                    action: PaletteAction::RunCommand(trimmed.to_string()),
                },
            );
        }
        (results, command_passthrough)
    }

    pub fn palette_view(&self) -> Option<PaletteView> {
        let state = self
            .modal
            .palette()
            .filter(|_| self.input.mode == Mode::Palette)?;
        let (entries, command_passthrough) = self.filtered_palette_entries(state);
        let cursor = if entries.is_empty() {
            0
        } else {
            state.cursor.min(entries.len() - 1)
        };
        let total_matches = entries.len();
        let items = entries
            .into_iter()
            .enumerate()
            .map(|(i, e)| PaletteItemView {
                category: e.category,
                label: e.label,
                detail: e.detail,
                shortcut: e.shortcut,
                is_command: e.is_command,
                selected: i == cursor,
            })
            .collect();
        Some(PaletteView {
            query: state.query.clone(),
            items,
            cursor,
            total_matches,
            command_passthrough,
        })
    }

    pub(super) fn handle_palette(&mut self, key: KeyEvent) -> Result<Outcome> {
        let repeat = key.kind == KeyEventKind::Repeat;
        if key.code == KeyCode::Esc {
            self.modal = ActiveModal::None;
            self.input.mode = self.take_resume_mode();
            self.status = "Action Palette closed".into();
            return Ok(Outcome::default());
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('u') => {
                    if let Some(p) = self.modal.palette_mut() {
                        p.query.clear();
                        p.cursor = 0;
                        p.user_moved = false;
                    }
                    return Ok(Outcome::default());
                }
                KeyCode::Char('n') => {
                    self.move_palette_cursor(1);
                    return Ok(Outcome::default());
                }
                KeyCode::Char('p') => {
                    self.move_palette_cursor(-1);
                    return Ok(Outcome::default());
                }
                _ => return Ok(Outcome::default()),
            }
        }

        match key.code {
            KeyCode::Down | KeyCode::Tab => {
                self.move_palette_cursor(1);
                Ok(Outcome::default())
            }
            KeyCode::Up | KeyCode::BackTab => {
                self.move_palette_cursor(-1);
                Ok(Outcome::default())
            }
            KeyCode::Enter => {
                if repeat {
                    return Ok(Outcome::default());
                }
                let Some(state) = self.modal.take_palette() else {
                    self.input.mode = Mode::Normal;
                    return Ok(Outcome::default());
                };
                let (entries, command_passthrough) = self.filtered_palette_entries(&state);
                let resume = self.input.resume_mode.take();
                self.input.mode = if resume == Some(Mode::Jam) {
                    Mode::Jam
                } else {
                    Mode::Normal
                };

                if state.query.trim().is_empty() && entries.is_empty() {
                    if let Some(mode) = resume {
                        self.input.mode = mode;
                    }
                    return Ok(Outcome::default());
                }

                // If user hasn't moved off index 0 and typed a recognized command line, run it cleanly!
                let result = if !state.user_moved && state.cursor == 0 && command_passthrough {
                    self.execute_command(state.query.trim())
                } else {
                    let idx = state.cursor.min(entries.len().saturating_sub(1));
                    if let Some(entry) = entries.get(idx) {
                        self.execute_palette_action(entry.action.clone())
                    } else {
                        // Fallback: if query was non-empty and had 0 palette matches, try execute_command
                        // so unknown Ex commands surface their clear helpful error message.
                        let q = state.query.trim();
                        if !q.is_empty() {
                            self.execute_command(q)
                        } else {
                            Ok(Outcome::default())
                        }
                    }
                };
                if resume == Some(Mode::Tutorial) {
                    if let Ok(ref out) = result {
                        if !out.quit {
                            self.help_ui.tutorial_step = (self.help_ui.tutorial_step + 1)
                                .min(TUTORIAL.len().saturating_sub(1));
                            if self.input.mode == Mode::Normal {
                                self.input.mode = Mode::Tutorial;
                            } else {
                                self.input.resume_mode = Some(Mode::Tutorial);
                            }
                        }
                    } else if self.input.mode == Mode::Normal {
                        self.input.mode = Mode::Tutorial;
                    }
                }
                result
            }
            KeyCode::Backspace => {
                if let Some(p) = self.modal.palette_mut() {
                    p.query.pop();
                    p.cursor = 0;
                    p.user_moved = false;
                }
                Ok(Outcome::default())
            }
            KeyCode::Char(c) if !c.is_control() => {
                if let Some(p) = self.modal.palette_mut() {
                    if p.query.len() < 2048 {
                        p.query.push(c);
                        p.cursor = 0;
                        p.user_moved = false;
                    }
                }
                Ok(Outcome::default())
            }
            _ => Ok(Outcome::default()),
        }
    }

    fn move_palette_cursor(&mut self, delta: isize) {
        let Some(state) = self.modal.palette() else {
            return;
        };
        let len = self.filtered_palette_entries(state).0.len();
        if len == 0 {
            return;
        }
        if let Some(p) = self.modal.palette_mut() {
            let next = (p.cursor as isize + delta).rem_euclid(len as isize) as usize;
            p.cursor = next;
            p.user_moved = next != 0;
        }
    }

    fn execute_palette_action(&mut self, action: PaletteAction) -> Result<Outcome> {
        match action {
            PaletteAction::RunCommand(cmd) => self.execute_command(&cmd),
            PaletteAction::SongProperties { row, edit } => {
                self.open_song_properties(row, edit);
                Ok(Outcome::default())
            }
            PaletteAction::BrowsePresets => self.open_preset_browser(),
            PaletteAction::SavePreset => {
                self.open_save_preset_flow();
                Ok(Outcome::default())
            }
            PaletteAction::BrowseFilterGrooves => self.open_groove_browser(),
            PaletteAction::ForkFilterGroove => self.fork_active_filter_groove(),
            PaletteAction::SaveFilterGroove => self.begin_save_active_filter_groove(),
            PaletteAction::LoadFilterGrooveBank => {
                self.open_file_hub_for_groove_bank_load();
                Ok(Outcome::default())
            }
            PaletteAction::RenameInstrument => {
                let id = self
                    .active_instrument()
                    .or(self.selected_instrument)
                    .context("Select an instrument to rename")?;
                self.begin_workflow(WorkflowKind::RenameInstrument(id))
            }
            PaletteAction::DeleteInstrument => {
                let id = self
                    .active_instrument()
                    .or(self.selected_instrument)
                    .context("Select an instrument to delete")?;
                self.request_delete_instrument(id)
            }
            PaletteAction::RenameChannel => {
                let ChannelRef::Channel(id) = self.selected_channel else {
                    bail!("Master channel cannot be renamed");
                };
                self.begin_workflow(WorkflowKind::RenameChannel(id))
            }
            PaletteAction::RenameEffect => {
                let id = match self.detail {
                    Detail::Effect(id) => Some(id),
                    _ => self.focused_effect_for_reorder().or_else(|| {
                        if let MixerSlot::Insert(idx) = self.view_state.mixer_slot {
                            self.mixer_focused_insert(idx)
                        } else {
                            match self.selected_channel {
                                ChannelRef::Master => self
                                    .doc
                                    .project()
                                    .mixer
                                    .master
                                    .inserts
                                    .first()
                                    .map(|d| d.id),
                                ChannelRef::Channel(cid) => self
                                    .channel(cid)
                                    .and_then(|c| c.inserts.first())
                                    .map(|d| d.id),
                            }
                        }
                    }),
                }
                .context("Select or open an effect to rename")?;
                self.begin_workflow(WorkflowKind::RenameEffect(id))
            }
            PaletteAction::RenamePattern => {
                let id = self
                    .selected_pattern
                    .context("Select a pattern to rename")?;
                self.begin_authoring(Draft::edit(self.doc.project(), id)?)
            }
            PaletteAction::RenamePart => {
                let id = match self.detail {
                    Detail::Part(id) => Some(id),
                    _ => self.selected_part,
                }
                .context("Select a part to rename")?;
                self.begin_workflow(WorkflowKind::RenamePart(id))
            }
            PaletteAction::RenameLane => {
                let id = self.selected_lane.context("Select a lane to rename")?;
                self.begin_workflow(WorkflowKind::RenameLane(id))
            }
            PaletteAction::ReloadSamples => self.apply(Action::ReloadSamples),
            PaletteAction::ReloadPresets => {
                self.reload_presets_with_status();
                Ok(Outcome::default())
            }
            PaletteAction::BrowseSamples => self.browse_samples(),
            PaletteAction::SampleAutoSlice => {
                let (inst, zone, module) = self
                    .resolve_sample_deck_target()
                    .context("No Sampler instrument in project")?;
                self.open_sample_deck_for(inst, zone, module)?;
                self.auto_slice_transients(inst, zone, module)
            }
            PaletteAction::SampleGridSlice => {
                let (inst, zone, module) = self
                    .resolve_sample_deck_target()
                    .context("No Sampler instrument in project")?;
                self.open_sample_deck_for(inst, zone, module)?;
                self.auto_slice_grid(inst, zone, module)
            }
            PaletteAction::SampleSliceToKit => {
                let (inst, zone, module) = self
                    .resolve_sample_deck_target()
                    .context("No Sampler instrument in project")?;
                self.open_slice_to_kit_picker(inst, zone, module)
            }
            PaletteAction::SampleWriteRiffMeta => self.execute_sample_write_meta_command(),
            PaletteAction::SampleProcess(edit) => {
                let (inst, zone, module) = self
                    .resolve_sample_deck_target()
                    .context("No Sampler instrument in project")?;
                self.open_sample_deck_for(inst, zone, module)?;
                self.apply(Action::ProcessSample {
                    instrument: inst,
                    zone,
                    module,
                    edit,
                })
            }
            PaletteAction::EditMiniNotation => self.open_active_mini_editor(),
            PaletteAction::GoDeck(kind) => self.focus_docked_deck(kind),
            PaletteAction::GoDevices(tab) => {
                self.open_devices_deck(tab);
                self.normalize_focus();
                Ok(Outcome::default())
            }
            PaletteAction::AudioPanic => self.apply(Action::AudioPanic),
            PaletteAction::RestartAudio => self.apply(Action::RestartAudio),
            PaletteAction::RescanAudio => self.apply(Action::RescanAudio),
            PaletteAction::GoInstrument => {
                let id = self
                    .active_instrument()
                    .or(self.selected_instrument)
                    .or_else(|| self.doc.project().instruments.first().map(|i| i.id))
                    .context("No instrument in project yet")?;
                self.open_instrument(id)?;
                Ok(Outcome::default())
            }
            PaletteAction::ToggleOutputMonitor => self.apply(Action::SetMonitor {
                visible: !self.view_state.monitor_visible,
            }),
            PaletteAction::ToggleJam => Ok(if self.is_jam() {
                self.leave_jam()
            } else {
                self.enter_jam()
            }),
            PaletteAction::ToggleStage => self.toggle_stage_key(),
            PaletteAction::KeepFocusedOverride => {
                if self.take_focused() {
                    self.keep_take_selection()
                } else {
                    let lane = self.focused_lane().context("Select a lane to keep")?;
                    self.keep_override(lane)
                }
            }
            PaletteAction::KeepAllOverrides => self.keep_all_overrides(),
            PaletteAction::ReleaseFocusedLane => {
                let lane = self.focused_lane().context("Select a lane to release")?;
                self.apply(Action::Release { lane, at: None })
            }
            PaletteAction::ReleaseAllLanes => self.apply(Action::ReleaseAll { at: None }),
            PaletteAction::LayoutPreset(n) => {
                self.apply_layout_preset(n);
                Ok(Outcome::default())
            }
            PaletteAction::ToggleZoom => {
                self.zoom_active();
                Ok(Outcome::default())
            }
            PaletteAction::ZoomViewIn => {
                self.zoom_view(1);
                Ok(Outcome::default())
            }
            PaletteAction::ZoomViewOut => {
                self.zoom_view(-1);
                Ok(Outcome::default())
            }
            PaletteAction::ZoomViewReset => {
                self.zoom_view_reset();
                Ok(Outcome::default())
            }
            PaletteAction::PlayPause => self.apply(Action::Play {
                playing: !self.transport.playing,
            }),
            PaletteAction::Stop => self.apply(Action::Stop),
            PaletteAction::CueBar => self.begin_cue_bar_value_input(),
            PaletteAction::CancelAllPending => self.apply(Action::CancelPending { all: true }),
            PaletteAction::ToggleLoop => self.toggle_song_loop(),
            PaletteAction::ToggleFollow => self.apply(Action::ToggleFollow),
            PaletteAction::ToggleTakeArmed => self.apply(Action::ToggleTakeArmed),
            PaletteAction::DiscardTake => self.request_discard_take(),
            PaletteAction::CycleEndRule => self.set_end_rule(self.end_rule().next()),
            PaletteAction::BounceSelection => self.open_bounce_dialog(),
            PaletteAction::ToggleAudioRecord => self.apply(Action::ToggleAudioRecord),
            PaletteAction::OpenRecordDialog => self.open_record_dialog(),
            PaletteAction::CycleInputMonitor => self.apply(Action::CycleInputMonitor),
            PaletteAction::CreatePattern => self.begin_create(),
            PaletteAction::DerivePattern => {
                self.begin_authoring(Draft::derive(self.selected_pattern))
            }
            PaletteAction::ForkPattern => {
                let id = self.selected_pattern.context("Select a pattern to fork")?;
                let source = self.pattern(id).context("Pattern missing")?;
                let name = crate::unique_name(
                    &format!("{} fork", source.name),
                    self.doc.project().patterns.iter().map(|p| p.name.as_str()),
                    None,
                );
                self.apply(Action::Fork { source: id, name })
            }
            PaletteAction::FreezePattern => {
                let id = self
                    .selected_pattern
                    .context("Select a pattern to freeze")?;
                let pat = self.pattern(id).context("Pattern missing")?;
                ensure!(
                    !matches!(
                        pat.kind,
                        PatternKind::Notes { .. }
                            | PatternKind::Rhythm { .. }
                            | PatternKind::Values { .. }
                    ),
                    "Pattern '{}' is already literal",
                    pat.name
                );
                self.begin_workflow(WorkflowKind::FreezePattern(id))
            }
            PaletteAction::DeletePattern => {
                let id = self
                    .selected_pattern
                    .context("Select a pattern to delete")?;
                self.request_delete_pattern(id)
            }
            PaletteAction::OpenInspect => {
                self.help_ui.overlay_scroll = 0;
                self.input.mode = Mode::Inspect;
                self.status = "Inspect · j/k scroll · Esc close".into();
                Ok(Outcome::default())
            }
            PaletteAction::NewProject => {
                if self.dirty() {
                    self.input.mode = Mode::Confirm(ConfirmAction::New(None));
                    self.status =
                        "Unsaved changes · y / Enter discard and start new · Esc cancel".into();
                    return Ok(Outcome::default());
                }
                self.apply(Action::New { title: None })
            }
            PaletteAction::Workflow(kind) => self.begin_workflow(kind),
            PaletteAction::QuickAddEffect(kind) => self.apply(Action::AddEffect {
                channel: self.selected_channel,
                kind: kind.into(),
            }),
            PaletteAction::ClearMeters => self.apply(Action::ClearMeters { channel: None }),
            PaletteAction::OpenFileHub(tab) => {
                self.open_file_hub(tab);
                Ok(Outcome::default())
            }
            PaletteAction::SaveMidiProfileHub => {
                self.open_file_hub_for_midi_profile_save();
                Ok(Outcome::default())
            }
            PaletteAction::LoadMidiProfileHub => {
                self.open_file_hub_for_midi_profile_load();
                Ok(Outcome::default())
            }
            PaletteAction::OpenSettings => {
                self.settings_ui.cursor = 0;
                self.input.mode = Mode::Settings;
                Ok(Outcome::default())
            }
            PaletteAction::OpenSettingsRow(row) => {
                let max = self.settings_row_targets().len().saturating_sub(1);
                self.settings_ui.cursor = row.index().min(max);
                self.input.mode = Mode::Settings;
                Ok(Outcome::default())
            }
            PaletteAction::ToggleKeyHints => {
                self.set_key_hints_visible(!self.view_state.tips);
                Ok(Outcome::default())
            }
            PaletteAction::ToggleTopBarCpu => {
                self.set_top_bar_cpu_visible(!self.config.show_top_bar_cpu);
                Ok(Outcome::default())
            }
            PaletteAction::CycleTopBarMeter => {
                self.cycle_top_bar_meter(1);
                Ok(Outcome::default())
            }
            PaletteAction::ToggleArrangeLaneMeters => {
                self.set_arrange_lane_meters_visible(!self.config.show_arrange_lane_meters);
                Ok(Outcome::default())
            }
            PaletteAction::SetTheme(theme) => {
                self.config.theme = theme;
                self.settings_ui.config_dirty = true;
                self.maybe_persist_config();
                self.status = format!("Theme → {}", theme.label());
                Ok(Outcome::default())
            }
            PaletteAction::OpenHelp => {
                self.help_ui.searching = false;
                self.help_ui.query.clear();
                self.help_ui.overlay_scroll = 0;
                self.input.mode = Mode::Help;
                Ok(Outcome::default())
            }
            PaletteAction::OpenAssets => {
                self.help_ui.overlay_scroll = 0;
                self.input.mode = Mode::Assets;
                Ok(Outcome::default())
            }
            PaletteAction::OpenTutorial => {
                self.help_ui.tutorial_step = 0;
                self.input.mode = Mode::Tutorial;
                Ok(Outcome::default())
            }
            PaletteAction::Quit => Ok(self.request_quit()),
            PaletteAction::Undo => self.apply(Action::Undo),
            PaletteAction::Redo => self.apply(Action::Redo),
            PaletteAction::JumpSection(id) => {
                if self.doc.project().sections.iter().any(|s| s.id == id) {
                    let key_hint = self.section_key_char(id);
                    return self.trigger_section_jump_by_id(id, key_hint);
                }
                Ok(Outcome::default())
            }
            PaletteAction::LoopSection(id) => {
                if let Some(sec) = self
                    .doc
                    .project()
                    .sections
                    .iter()
                    .find(|s| s.id == id)
                    .cloned()
                {
                    self.arrange_ui.step = sec.start_step;
                    self.select_at_cursor();
                    return self.loop_section_at_cursor();
                }
                Ok(Outcome::default())
            }
            PaletteAction::OpenPattern(id) => {
                self.select_pattern(id)?;
                self.go_to_deck(self.workspace_deck(Workspace::Pattern));
                Ok(Outcome::default())
            }
            PaletteAction::OpenInstrument(id) => {
                self.open_instrument(id)?;
                Ok(Outcome::default())
            }
            PaletteAction::OpenPart(id) => {
                ensure!(self.part(id).is_some(), "Part missing");
                self.push_back();
                self.selected_part = Some(id);
                self.show_detail(Detail::Part(id));
                Ok(Outcome::default())
            }
            PaletteAction::OpenChannel(ch) => {
                self.open_channel(ch)?;
                Ok(Outcome::default())
            }
            PaletteAction::OpenEffect(id) => {
                self.open_effect(id)?;
                Ok(Outcome::default())
            }
            PaletteAction::OpenLane(id) => {
                ensure!(self.lane(id).is_some(), "Lane missing");
                self.push_back();
                self.selected_lane = Some(id);
                self.go_to_deck(self.workspace_deck(Workspace::Arrange));
                self.select_at_cursor();
                Ok(Outcome::default())
            }
            PaletteAction::OpenModulation(id) => {
                ensure!(
                    self.doc.project().modulations.iter().any(|m| m.id == id),
                    "Modulation missing"
                );
                self.begin_workflow(WorkflowKind::EditModulation(id))
            }
            PaletteAction::ReplacePlacement(id, what) => self.open_replace(id, what),
        }
    }
}
