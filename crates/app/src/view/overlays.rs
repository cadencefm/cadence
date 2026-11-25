//! Semantic modal overlays (Browser, Authoring/Workflow drafts, ValueInput, Picker,
//! Event Provenance, Confirm, Tabbed & Searchable Help, Settings, Inspect/Assets,
//! Tutorial, and Prefix key legends).
use crate::authoring::DraftView;
use crate::{
    App, BrowserView, EventInspectionView, GrooveBrowserView, MiniEditorView, Mode, PaletteView,
    PickerView, PresetBrowserView, SongPropertiesView, ValueInputView, TUTORIAL,
};
use cadence_core::music::note_name;

#[derive(Clone, Debug)]
pub enum OverlayView {
    Browser(BrowserView),
    PresetBrowser(PresetBrowserView),
    GrooveBrowser(GrooveBrowserView),
    SongProperties(SongPropertiesView),
    MiniEditor(MiniEditorView),
    Palette(PaletteView),
    Authoring(DraftView),
    ValueInput(ValueInputView),
    Picker(PickerView),
    EventInspection(EventInspectionView),
    Confirm {
        title: &'static str,
        message: String,
    },
    Help(HelpView),
    Settings(SettingsView),
    FileHub(FileHubView),
    Inspect {
        title: &'static str,
        lines: Vec<String>,
        scroll: usize,
    },
    Tutorial {
        step: usize,
        total: usize,
        title: &'static str,
        body: &'static str,
        command: &'static str,
    },
    PrefixMenu {
        title: &'static str,
        items: Vec<(String, String)>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HelpTab {
    All,
    Navigate,
    Compose,
    MiniAlgebra,
    SoundFx,
    LiveJam,
    FilesCli,
}

impl HelpTab {
    pub const ALL: [Self; 7] = [
        Self::All,
        Self::Navigate,
        Self::Compose,
        Self::MiniAlgebra,
        Self::SoundFx,
        Self::LiveJam,
        Self::FilesCli,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Navigate => "Navigate",
            Self::Compose => "Compose",
            Self::MiniAlgebra => "Mini & Algebra",
            Self::SoundFx => "Sound & FX",
            Self::LiveJam => "Live & Jam",
            Self::FilesCli => "Files & CLI",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct HelpEntry {
    pub tab: HelpTab,
    pub category: &'static str,
    pub keys: &'static str,
    pub summary: &'static str,
}

#[derive(Clone, Debug)]
pub struct HelpView {
    pub tabs: Vec<(&'static str, bool)>,
    pub active_tab: usize,
    pub query: String,
    pub searching: bool,
    pub scroll: usize,
    pub entries: Vec<HelpEntry>,
    pub total_entries: usize,
}

#[derive(Clone, Debug)]
pub struct SettingsRowView {
    pub group: &'static str,
    pub label: String,
    pub value: String,
    pub detail: String,
    pub selected: bool,
    pub adjustable: bool,
}

#[derive(Clone, Debug)]
pub struct SettingsView {
    pub rows: Vec<SettingsRowView>,
    pub cursor: usize,
    pub dirty: bool,
    pub config_path: String,
    pub editing_prompt: Option<(String, String)>,
}

#[derive(Clone, Debug)]
pub struct FileHubRowView {
    pub section: &'static str,
    pub badge: &'static str,
    pub label: String,
    pub value: String,
    pub detail: String,
    pub selected: bool,
    pub is_action: bool,
    pub is_dir: bool,
}

#[derive(Clone, Debug)]
pub struct FileHubView {
    pub tabs: Vec<(&'static str, bool)>,
    pub active_tab: crate::FileHubTab,
    pub current_dir: String,
    pub query: String,
    pub searching: bool,
    pub editing_field: Option<String>,
    pub rows: Vec<FileHubRowView>,
    pub cursor: usize,
    pub preview: String,
    pub export_summary: Option<crate::ExportSummary>,
}

pub const HELP_ENTRIES: &[HelpEntry] = &[
    // Navigate & Layout
    HelpEntry {
        tab: HelpTab::Navigate,
        category: "DECKS",
        keys: "ga/Ctrl+a · gb/Ctrl+b · gp/Ctrl+p · gm/Ctrl+m · gi/Ctrl+i · gk/Ctrl+k · gt/Ctrl+t · gG/Ctrl+g · gd/Ctrl+d · go/Ctrl+o",
        summary: "Jump directly to Arrange · Bank · Pattern · Mixer · Instrument · Macros · Take · Signal Graph · Devices (Audio/MIDI) · Output",
    },
    HelpEntry {
        tab: HelpTab::Navigate,
        category: "PALETTE",
        keys: ": · Ctrl+Shift+p · ↑/↓ · Tab · Enter · Esc",
        summary: "Fuzzy Action Palette: search any action, deck, workflow, FX, section, pattern, part, channel, lane or modulation",
    },
    HelpEntry {
        tab: HelpTab::Navigate,
        category: "MODALS",
        keys: "Ctrl+Shift+f/o/s/e/i/,/b/n/? · Ctrl+Shift+j · gj",
        summary: "File Hub · Open · Save As · Export · Song Properties · Settings (Ctrl+,) · Sample Browser · Create Menu · Help · Jam",
    },
    HelpEntry {
        tab: HelpTab::Navigate,
        category: "LAYOUT",
        keys: "z1–z4 · zz · zp · zx · zo · Ctrl+w · Tab/Shift+Tab",
        summary: "Layout presets (Compose/Perform/Sound/Mix) · zoom full-screen · pin · close · only · window prefix · cycle focus",
    },
    HelpEntry {
        tab: HelpTab::Navigate,
        category: "NAV",
        keys: "0/$ · G/End · PgUp/PgDn · gg/ge · Enter · Esc · Bksp · z=/z-/z0 · Ctrl+=/−/0",
        summary: "Start/end of focused list or timeline · page scroll · song start/end · follow/exact · unwind trail · zoom time",
    },
    HelpEntry {
        tab: HelpTab::Navigate,
        category: "SEARCH",
        keys: "/ · Enter · Esc · Ctrl+u",
        summary: "Universal filter in Bank, Sample/Preset/Groove Browsers, File Hub, Picker & Help · Enter applies · Esc clears then closes",
    },
    HelpEntry {
        tab: HelpTab::Navigate,
        category: "GRAPH",
        keys: "gG/Ctrl+g · hjkl · Enter · o/r · a · s/m/d · e · x/X",
        summary: "Interactive 5/6-column Signal Graph incl. FX nodes: open deck · route bus · add/edit/unlink Send, Mod & Duck cables",
    },
    HelpEntry {
        tab: HelpTab::Navigate,
        category: "DIRECT EDIT",
        keys: "hjkl nav · H/L tabs/bars · Alt+hjkl/Arrows & ,/. fine · Alt+HJKL & </> coarse · Alt+Bksp reset · Enter exact",
        summary: "4-tier direct editing: hjkl/HJKL strictly navigate (h/l no-op on 1-axis inspectors) · Alt+hjkl & ,/. sculpt with coalesced undo",
    },
    // Compose & Edit
    HelpEntry {
        tab: HelpTab::Compose,
        category: "ARRANGE",
        keys: "hjkl · H/L · Alt+h/l move · Alt+j/k lane · ,/. </> resize · d dup · vp/vo/vr · Enter · n→t · r/x/X",
        summary: "Navigate lanes/bars/sections · direct placement move/lane/resize/duplicate · Overview/Roll · open/place · edit/del Section",
    },
    HelpEntry {
        tab: HelpTab::Compose,
        category: "NOTES",
        keys: "h/l step · H/L bar · j/k voice · w/b note · Alt+j/k pitch · Alt+h/l gate · ,/. vel · [/] accid · c/C cond · o/m/x/X",
        summary: "2D polyphonic chord sculpting · Alt+J/K octave · </> coarse vel · o stack chord · m mute step · x/X del · i insert · A audition",
    },
    HelpEntry {
        tab: HelpTab::Compose,
        category: "STEP INSERT",
        keys: "i · 1–8 · zsxdcvgbhnjm, · Shift · </> oct · . rest · Esc",
        summary: "Piano-keyboard step entry (preserves chord stack when editing a chord voice) · Shift accents · . rest · Esc normal",
    },
    HelpEntry {
        tab: HelpTab::Compose,
        category: "FORK & FREEZE",
        keys: "' (apostrophe) · f fork · F freeze · o local (Placement)",
        summary: "Arm From-Here fork (next edit forks from current bar) · fork pattern · freeze to literal · open local occurrence",
    },
    HelpEntry {
        tab: HelpTab::Compose,
        category: "VALUES",
        keys: "h/l · H/L · w/b · Alt+j/k & ,/. value · Alt+h/l slide · Enter exact · e consumers · m Hold/Ramp · x del",
        summary: "Browse steps/breakpoints · adjust value/slide directly · exact ValueInput · inspect Mod/Link consumers · Hold/Ramp · delete",
    },
    HelpEntry {
        tab: HelpTab::Compose,
        category: "CREATE & EDIT",
        keys: "n · d · r · [N]y/p/P · ; repeat · u/Ctrl+z · U/Ctrl+r/Ctrl+Shift+z",
        summary: "Create all 22 PatternKinds & objects · derive any operator · edit in-place · repeat last step edit (;) · undo/redo",
    },
    // Mini-Notation & Pattern Algebra
    HelpEntry {
        tab: HelpTab::MiniAlgebra,
        category: "MINI CODE",
        keys: "Enter (on Mini) · n→m (New Mini) · Esc",
        summary: "Dedicated Mini-Notation Editor modal with live keystroke parse validation & Cycle 1 preview (Enter commits when valid)",
    },
    HelpEntry {
        tab: HelpTab::MiniAlgebra,
        category: "MINI SYNTAX",
        keys: "c4 eb3 ~ [a b] [c,e,g] <a b> *N /N (k,n) ?p !",
        summary: "Notes/degrees · rests · subdivisions · chords · alternation · repeat/slow · Euclid · chance · accent",
    },
    HelpEntry {
        tab: HelpTab::MiniAlgebra,
        category: "EUCLIDEAN",
        keys: "n→e (New Euclidean) · r edit · F freeze",
        summary: "Generate Bjorklund Euclidean rhythm (pulses, steps, rotation) via Create workflow · r edits parameters · F freezes to literal",
    },
    HelpEntry {
        tab: HelpTab::MiniAlgebra,
        category: "HARMONY",
        keys: "d (Derive) · add · mask · transpose · from · fold",
        summary: "Shift degrees by Harmony · mask by Rhythm · transpose · notes from values · octave range-fold",
    },
    HelpEntry {
        tab: HelpTab::MiniAlgebra,
        category: "TIME & GROOVE",
        keys: "d (Derive) · arp · swing · ratchet · fast · slow",
        summary: "Arpeggiate held chords (up/down/updown/random) · micro-swing · sub-step ratchets · time scale",
    },
    HelpEntry {
        tab: HelpTab::MiniAlgebra,
        category: "STRUCTURE",
        keys: "d (Derive) · every · concat · stack · shape · reverse · shift · chance",
        summary: "Cycle alternation · sequence parts · poly-stack · velocity/gate envelope · mirror · rotate · seeded chance",
    },
    // Sound & Studio FX
    HelpEntry {
        tab: HelpTab::SoundFx,
        category: "RACK PANEL",
        keys: "H/L tab · h/l module · j/k param · w/b group · Alt+h/l ,/. adjust · Alt+j/k pin order · a/x mod · p/e/r/o/X",
        summary: "Unified Eurorack Panel: Quick curated pins or per-module faceplate · Enter exact · a/x mod · p pin · e links · r rename · o route",
    },
    HelpEntry {
        tab: HelpTab::SoundFx,
        category: "PATCHBAY",
        keys: "hjkl node · w/b cable · Alt+h/l ,/. amt · c patch · C/Enter exact · x cut/del · a mod · p pin · e links",
        summary: "4-column Eurorack patchbay (SOURCES | MODULATORS | PROCESSORS | VCA / OUT) · attenuverter cables with z-1 feedback · Enter→Panel",
    },
    HelpEntry {
        tab: HelpTab::SoundFx,
        category: "RACK ZONES",
        keys: "h/l zone · j/k ctrl · w/b group · Alt+h/l ,/. adjust · Alt+j/k reorder · a/d/x zone · r ren · B sample · e links",
        summary: "Horizontal Zone strip + 2D Key×Velocity map + vertical Zone/Voice/Rack controls · each zone owns its modular graph · Enter exact/browse",
    },
    HelpEntry {
        tab: HelpTab::SoundFx,
        category: "PRESETS",
        keys: "P (Rack) · h/l H/L category · j/k preview · / filter · A audition · s save · R reload · Enter load · Esc restore",
        summary: "Unified Rack Preset Browser (All/Synth/Sampler/Drums/Kit/Modular + bare templates) with live non-destructive preview",
    },
    HelpEntry {
        tab: HelpTab::SoundFx,
        category: "MIXER",
        keys: "gm/Ctrl+m · h/l strip · j/k row · Alt+h/l ,/. adjust · Alt+j/k fader/reorder · a add · o ch · e links · b/r/v fx · C clip",
        summary: "Strips + vertical FX/SND/OUT/DUCK stack · Alt+Bksp reset · Enter open/exact · f/w/d fx/send/duck · m/s mute/solo · C/tC clears clip",
    },
    HelpEntry {
        tab: HelpTab::SoundFx,
        category: "STUDIO FX",
        keys: "f (Mixer/Ch) · j/k param · w/b group · Alt+h/l ,/. adjust · Alt+j/k reorder · r rename · v move · m bypass · e links · X del",
        summary: "8 zero-alloc native FX (EQ, Filter, Comp, Limiter, Drive, Delay, Chorus, Reverb) · live modulated values · direct sculpting",
    },
    HelpEntry {
        tab: HelpTab::SoundFx,
        category: "FILTER GROOVES",
        keys: "P/Enter (Filter) · / filter · j/k live preview · f fork pattern · s save to bank",
        summary: "Browse extensible filter_grooves.json banks (up to 128 slots) with live non-destructive preview · f forks groove · s saves to bank",
    },
    HelpEntry {
        tab: HelpTab::SoundFx,
        category: "BOUNCE",
        keys: "tb · Bounce Selection (Palette)",
        summary: "Resample part, pattern, or bars into a new Sampler Rack or directly into any Rack zone's Sampler module",
    },
    HelpEntry {
        tab: HelpTab::SoundFx,
        category: "RECORD AUDIO",
        keys: "ta · tA · ti · Record Audio (Palette)",
        summary: "Arm/stop quantized 24-bit WAV audio input recording (ta), open Record dialog (tA), cycle input monitor (ti) — lands in Sampler + Arrange or Rack zone",
    },
    HelpEntry {
        tab: HelpTab::SoundFx,
        category: "SAMPLES",
        keys: "B/gS/Ctrl+Shift+b · Sample row Enter · / filter · A audition · r/R reload/rescan · Esc",
        summary: "Open Sample Browser to load WAV into active zone's Sampler module (auto-adds Sampler if needed) with RIFF smpl/inst metadata",
    },
    // Live Performance, Launchpad & Jam
    HelpEntry {
        tab: HelpTab::LiveJam,
        category: "TRANSPORT",
        keys: "Space · Home · \\ · [/] · tc/tp · tC · te · tr/tR · ta/tA/ti · ts · tl/tx · tf · tb · tq/tQ/tE",
        summary: "Play · stop · loop · cue · clear clips (tC) · end rule · arm take (tr) · audio rec/dialog/monitor (ta/tA/ti) · follow · bounce · LiveTiming",
    },
    HelpEntry {
        tab: HelpTab::LiveJam,
        category: "STAGING",
        keys: "S · Alt+Enter · Enter/f · x · Bksp",
        summary: "Stage synchronized multi-lane launches/mutes/solos/releases (S never shadowed) · fire on boundary · discard · cancel",
    },
    HelpEntry {
        tab: HelpTab::LiveJam,
        category: "MACROS",
        keys: "gk/Ctrl+k · h/l 1–8 slot · j/k dial/binding · Alt+h/l ,/. dial/min · Alt+H/L </> coarse/max · Alt+j/k reorder · a/r/e/v/x/X",
        summary: "1D horizontal M1–M8 strip + scrollable Bindings · Alt+Bksp reset · Enter exact ValueInput or jump target · M MIDI learn",
    },
    HelpEntry {
        tab: HelpTab::LiveJam,
        category: "AUDIO DEV",
        keys: "Ctrl+d · H/L tab · j/k row · w/b group · Alt+h/l ,/. cycle · Alt+Bksp reset · Enter pick/run · r rescan · Ctrl+s",
        summary: "Audio Output/Input/Cue, Input Mode/Monitor & Latency Trim, Sample Rate & Buffer Size with ms latency, live CPU/xruns/input meters, Audio Panic & Restart",
    },
    HelpEntry {
        tab: HelpTab::LiveJam,
        category: "MIDI CTRL",
        keys: "gd/Ctrl+d · H/L tab · j/k row · w/b group · Alt+h/l ,/. cycle · Enter pick/learn · a add · v inv · r rescan · x/X del/clear · M/R · Ctrl+s",
        summary: "Navigable DEVICE & PROFILE (IN/OUT/Profile/Pads/Pickup) + MAPPINGS · MIDI Learn/Unlearn (M) · NOTE+CC REC (R) · save custom profiles",
    },
    HelpEntry {
        tab: HelpTab::LiveJam,
        category: "LAUNCHPAD",
        keys: "gb/Ctrl+b · v · zz · j/k lane · h/l part · Enter/Shift+Enter · a/r/X · A audition · x release · m/s",
        summary: "Row-oriented Launchpad (lanes=rows, parts=horizontal cards) · launch (Shift+Enter now) · add/edit phase/drop · audition · release",
    },
    HelpEntry {
        tab: HelpTab::LiveJam,
        category: "JAM MODE",
        keys: "gj · Ctrl+Shift+j · 1–0 · q–p + 1–9 · z–, · </> · Ctrl+Shift+p · Ctrl+Shift+/",
        summary: "Live keyboard HUD · jump sections · trigger lane layers · 13-key piano · shift octave · Palette & Help reachable from Jam",
    },
    HelpEntry {
        tab: HelpTab::LiveJam,
        category: "ALT LAYER",
        keys: "Alt+1–0 · Alt+q–p · Alt+m/M · Alt+s/S · Alt+x/X · Alt+c/C · Alt+Enter",
        summary: "Works in any mode: sections · lanes · mute/unmute all · solo/clear · release/all · Keep Override / Atomic Keep All · Stage",
    },
    HelpEntry {
        tab: HelpTab::LiveJam,
        category: "TAKE DECK",
        keys: "gt/Ctrl+t · tr · h/l · 0/$/G · v · Enter/Alt+c keep · Ctrl+s/Ctrl+Shift+s save · o load · x/X",
        summary: "Arm/disarm take (tr) · browse bars · visual mark range (v) · keep into arrangement · save/save as · load · clear marks · discard",
    },
    // Files, Settings & CLI
    HelpEntry {
        tab: HelpTab::FilesCli,
        category: "FILE HUB",
        keys: "gf · Ctrl+Shift+f/o/s/e · Ctrl+s save · H/L Tab · Alt+h/l ,/. export opt · / · e path · F · R",
        summary: "4-tab Hub: Open (Recent, Examples, Bundles, Takes) · Save (+ Bundle/Preset/MIDI/Take) · Import (.rbs/Presets/MIDI) · Export WAV/Stems",
    },
    HelpEntry {
        tab: HelpTab::FilesCli,
        category: "SONG PROPS",
        keys: "gP/Ctrl+Shift+i · j/k · w/b · Alt+h/l ,/. adjust · Enter exact/rename · Alt+Bksp reset · u/U undo",
        summary: "Edit song Title, Tempo (BPM), Key Root, Scale (13 scales), Length (Bars), End Rule & Live Timing cleanly on the undo stack",
    },
    HelpEntry {
        tab: HelpTab::FilesCli,
        category: "SETTINGS",
        keys: "g,/Ctrl+,/Ctrl+Shift+, · j/k · w/b · Alt+h/l ,/. adjust · Enter exact/toggle · Alt+Bksp reset · a/x · F · Ctrl+s save",
        summary: "Theme · layout · key hints · follow · auto-arm takes · default BPM/key/scale · LiveTiming · sample_dirs/preset_dirs",
    },
    HelpEntry {
        tab: HelpTab::FilesCli,
        category: "PROJECT & AUDIO",
        keys: "Ctrl+s · Ctrl+Shift+s · Ctrl+Shift+e · gf → Save/Export",
        summary: "Save project · package portable bundle + samples · render 16-bit Master WAV or per-channel Stems · inspect Sample Assets in Palette",
    },
    HelpEntry {
        tab: HelpTab::FilesCli,
        category: "REBIRTH .RBS",
        keys: "gf → Import (.rbs) · --import-rbs FILE.rbs",
        summary: "Clean-room import of ReBirth RB-338 v1.x & v2.0 songs (2x 303, 808, 909, PCF, Dist, Delay, Comp)",
    },
    HelpEntry {
        tab: HelpTab::FilesCli,
        category: "RENAME",
        keys: "r (on any deck) · Palette → Rename",
        summary: "Rename instrument, channel, fx, pattern, part, lane, section, module, pin, zone, or macro (all variants also available in : Palette)",
    },
    HelpEntry {
        tab: HelpTab::FilesCli,
        category: "CLI & IO",
        keys: "--project · --import-rbs · --render · --take · --stdio",
        summary: "Launch/import song · offline WAV render · replay live take · crash recovery · JSONL API",
    },
];

impl App {
    fn filtered_help_entries(&self) -> Vec<HelpEntry> {
        let active_tab = self.help_ui.tab.min(HelpTab::ALL.len() - 1);
        let tab_filter = HelpTab::ALL[active_tab];
        let query = self.help_ui.query.trim();
        let q_lower = query.to_ascii_lowercase();
        HELP_ENTRIES
            .iter()
            .copied()
            .filter(|e| {
                let tab_ok =
                    !q_lower.is_empty() || tab_filter == HelpTab::All || e.tab == tab_filter;
                if !tab_ok {
                    return false;
                }
                if q_lower.is_empty() {
                    return true;
                }
                q_lower.split_whitespace().all(|tok| {
                    e.category.to_ascii_lowercase().contains(tok)
                        || e.keys.to_ascii_lowercase().contains(tok)
                        || e.summary.to_ascii_lowercase().contains(tok)
                        || e.tab.label().to_ascii_lowercase().contains(tok)
                })
            })
            .collect()
    }

    pub(crate) fn help_entry_count(&self) -> usize {
        self.filtered_help_entries().len()
    }

    pub(crate) fn build_help_view(&self) -> HelpView {
        let active_tab = self.help_ui.tab.min(HelpTab::ALL.len() - 1);
        let entries = self.filtered_help_entries();
        let tabs = HelpTab::ALL
            .iter()
            .enumerate()
            .map(|(i, t)| (t.label(), i == active_tab))
            .collect();
        HelpView {
            tabs,
            active_tab,
            query: self.help_ui.query.clone(),
            searching: self.help_ui.searching,
            scroll: self
                .help_ui
                .overlay_scroll
                .min(entries.len().saturating_sub(1)),
            total_entries: HELP_ENTRIES.len(),
            entries,
        }
    }

    pub(crate) fn build_settings_view(&self) -> SettingsView {
        let preset_name = match self.config.default_preset {
            1 => "1 · Compose (Arrange + Pattern)",
            2 => "2 · Perform (Arrange + Launchpad)",
            3 => "3 · Sound (Instrument + Pattern)",
            4 => "4 · Mix (Arrange + Mixer)",
            _ => "1 · Compose",
        };
        let default_scale_label = cadence_core::music::named_scale(&self.config.default_scale)
            .map(|s| cadence_core::music::scale_label(&s))
            .unwrap_or("Natural Minor")
            .to_string();
        let mut rows: Vec<SettingsRowView> = crate::SettingsCoreRow::ALL
            .into_iter()
            .map(|row| {
                let (label, value, detail) = match row {
                    crate::SettingsCoreRow::Theme => (
                        "GPU Theme",
                        self.config.theme.label().to_string(),
                        "Switches all GPU decks, cables, meters & HUDs immediately",
                    ),
                    crate::SettingsCoreRow::Preset => (
                        "Startup Layout",
                        preset_name.to_string(),
                        "Default deck layout preset applied on launch (z1–z4)",
                    ),
                    crate::SettingsCoreRow::Monitor => (
                        "Output Monitor",
                        if self.config.show_output_monitor {
                            "Visible".into()
                        } else {
                            "Hidden".into()
                        },
                        "Show the Final Output oscilloscope & peak meter strip (go / Ctrl+o)",
                    ),
                    crate::SettingsCoreRow::TopBarCpu => (
                        "Top-Bar DSP CPU",
                        if self.config.show_top_bar_cpu {
                            "Visible".into()
                        } else {
                            "Hidden".into()
                        },
                        "Show the fixed-width DSP CPU load pill beside FINAL in the top chrome",
                    ),
                    crate::SettingsCoreRow::TopBarMeter => (
                        "Top-Bar Stereo Meter",
                        self.config.top_bar_meter.label().to_string(),
                        "Compact L/R bar beside FINAL (Auto hides when Output is shown)",
                    ),
                    crate::SettingsCoreRow::ArrangeLaneMeters => (
                        "Arrange Lane Meters",
                        if self.config.show_arrange_lane_meters {
                            "Visible".into()
                        } else {
                            "Hidden".into()
                        },
                        "Show compact vertical L/R peak meter pills in Arrange lane gutters",
                    ),
                    crate::SettingsCoreRow::KeyHints => (
                        "Contextual Key Hints",
                        if self.config.show_key_hints {
                            "Visible".into()
                        } else {
                            "Hidden".into()
                        },
                        "Show live mode & deck shortcut hints in the footer bar",
                    ),
                    crate::SettingsCoreRow::Follow => (
                        "Viewport Follow",
                        self.config.default_follow.label().to_string(),
                        "Auto-scroll Arrange & Pattern pages while playing (tf)",
                    ),
                    crate::SettingsCoreRow::AutoRecord => (
                        "Auto-Record Takes",
                        if self.config.auto_record_takes {
                            "On".into()
                        } else {
                            "Off".into()
                        },
                        "Auto-arm take recording on Space · tr toggles live",
                    ),
                    crate::SettingsCoreRow::DefaultBpm => (
                        "Default Tempo",
                        format!("{:.0} BPM", self.config.default_bpm),
                        "Starting BPM (20–300) for new blank projects · Enter exact",
                    ),
                    crate::SettingsCoreRow::DefaultRoot => (
                        "Default Key",
                        format!(
                            "{} (MIDI {})",
                            note_name(self.config.default_root_midi),
                            self.config.default_root_midi
                        ),
                        "Starting root note (C1–C6 / MIDI 24–84) for new projects · Enter exact",
                    ),
                    crate::SettingsCoreRow::DefaultScale => (
                        "Default Scale",
                        default_scale_label.clone(),
                        "Starting scale (13 named scales) for new blank projects",
                    ),
                    crate::SettingsCoreRow::LaunchQuantize => (
                        "Launch Quantize",
                        self.project().timing.launch.label(self.project()),
                        "now/beat/bar/bars/cycle · c picks Cycle pattern · tq/tQ",
                    ),
                    crate::SettingsCoreRow::EditTiming => (
                        "Live Edit Timing",
                        match self.project().timing.edits {
                            cadence_core::material::EditTiming::Now => "Now (next step)".into(),
                            cadence_core::material::EditTiming::Launch => {
                                "Follow Launch Quantize".into()
                            }
                        },
                        "When live edits land while playing (tE toggles live)",
                    ),
                    crate::SettingsCoreRow::LateTolerance => (
                        "Late Tolerance",
                        format!(
                            "{} step{}",
                            self.project().timing.late_tolerance_steps,
                            if self.project().timing.late_tolerance_steps == 1 {
                                ""
                            } else {
                                "s"
                            }
                        ),
                        "Steps after a boundary still caught as on-time (0–4 · Enter exact)",
                    ),
                };
                SettingsRowView {
                    group: row.group(),
                    label: label.into(),
                    value,
                    detail: detail.into(),
                    selected: false,
                    adjustable: true,
                }
            })
            .collect();

        rows.push(SettingsRowView {
            group: "ASSET DIRECTORIES",
            label: "Sample Directories (sample_dirs)".into(),
            value: if self.config.sample_dirs.is_empty() {
                "Defaults only · [a] Add".into()
            } else {
                format!("{} custom · [a] Add", self.config.sample_dirs.len())
            },
            detail: "a / Enter add folder path · F Finder folder · auto-rescans Sample Browser"
                .into(),
            selected: false,
            adjustable: false,
        });
        for dir in &self.config.sample_dirs {
            rows.push(SettingsRowView {
                group: "ASSET DIRECTORIES",
                label: "↪ Sample Dir".into(),
                value: dir.clone(),
                detail: "x remove · a add another · Ctrl+s saves config.json".into(),
                selected: false,
                adjustable: false,
            });
        }
        rows.push(SettingsRowView {
            group: "ASSET DIRECTORIES",
            label: "Preset Directories (preset_dirs)".into(),
            value: if self.config.preset_dirs.is_empty() {
                "Defaults only · [a] Add".into()
            } else {
                format!("{} custom · [a] Add", self.config.preset_dirs.len())
            },
            detail: "a / Enter add folder path · F Finder folder · auto-reloads Preset Catalog"
                .into(),
            selected: false,
            adjustable: false,
        });
        for dir in &self.config.preset_dirs {
            rows.push(SettingsRowView {
                group: "ASSET DIRECTORIES",
                label: "↪ Preset Dir".into(),
                value: dir.clone(),
                detail: "x remove · a add another · Ctrl+s saves config.json".into(),
                selected: false,
                adjustable: false,
            });
        }

        let cursor = self.settings_ui.cursor.min(rows.len().saturating_sub(1));
        if let Some(row) = rows.get_mut(cursor) {
            row.selected = true;
        }
        let editing_prompt = self.settings_ui.editing.as_ref().map(|(kind, buf)| {
            let label = match kind {
                crate::SettingsDirKind::SampleDirs => "ADD SAMPLE DIR",
                crate::SettingsDirKind::PresetDirs => "ADD PRESET DIR",
            };
            (label.to_string(), buf.clone())
        });
        SettingsView {
            rows,
            cursor,
            dirty: self.settings_ui.config_dirty,
            config_path: self.io.config_path().display().to_string(),
            editing_prompt,
        }
    }

    pub(crate) fn build_file_hub_view(&self) -> Option<FileHubView> {
        let hub = self.modal.file_hub()?;
        let tabs = crate::FileHubTab::ALL
            .iter()
            .map(|&t| (t.label(), t == hub.tab))
            .collect();

        let (rows, preview) = match hub.tab {
            crate::FileHubTab::Quick | crate::FileHubTab::Browse | crate::FileHubTab::Import => {
                let cursor = if hub.entries.is_empty() {
                    0
                } else {
                    hub.cursor.min(hub.entries.len() - 1)
                };
                let rows = hub
                    .entries
                    .iter()
                    .enumerate()
                    .map(|(i, e)| FileHubRowView {
                        section: e.section,
                        badge: e.kind.badge(),
                        label: e.label.clone(),
                        value: String::new(),
                        detail: e.detail.clone(),
                        selected: i == cursor,
                        is_action: e.kind == crate::FileEntryKind::NewProject,
                        is_dir: matches!(
                            e.kind,
                            crate::FileEntryKind::Dir | crate::FileEntryKind::ParentDir
                        ),
                    })
                    .collect();
                let preview = hub
                    .entries
                    .get(cursor)
                    .and_then(|e| hub.preview_cache.get(&e.path).cloned())
                    .unwrap_or_else(|| match hub.tab {
                        crate::FileHubTab::Import => {
                            "Select an .rbs song, Instrument preset, or MIDI profile · press F for native Finder"
                                .into()
                        }
                        crate::FileHubTab::Browse => {
                            "Browse folders & projects · Backspace parent · ~ home · . project · F Finder"
                                .into()
                        }
                        _ => {
                            "Start a new project or open a recent song, demo, or take · 2 Browse · F Finder"
                                .into()
                        }
                    });
                (rows, preview)
            }
            crate::FileHubTab::Save => {
                let save_label = if self.doc.is_untitled() {
                    format!("Save ({})", hub.save_as_path)
                } else {
                    format!(
                        "Quick Save ({}{})",
                        self.doc.save_path().display(),
                        if self.dirty() {
                            " · modified"
                        } else {
                            " · clean"
                        }
                    )
                };
                let rows = vec![
                    FileHubRowView {
                        section: "PROJECT FILE",
                        badge: "SAVE",
                        label: "Save Current Project".into(),
                        value: save_label,
                        detail: "Atomic write of definitions, parts, placements & mixer".into(),
                        selected: hub.cursor == 0,
                        is_action: true,
                        is_dir: false,
                    },
                    FileHubRowView {
                        section: "PROJECT FILE",
                        badge: "PATH",
                        label: "Save Project As".into(),
                        value: hub.save_as_path.clone(),
                        detail: "e edit path · F Finder Save As · Enter write JSON".into(),
                        selected: hub.cursor == 1,
                        is_action: false,
                        is_dir: false,
                    },
                    FileHubRowView {
                        section: "PORTABLE PACKAGE",
                        badge: "BUNDLE",
                        label: "Export Portable Bundle".into(),
                        value: hub.bundle_dir.clone(),
                        detail: "Copies all Rack Sampler audio into ./samples/ + project.json (F pick folder)".into(),
                        selected: hub.cursor == 2,
                        is_action: true,
                        is_dir: true,
                    },
                    FileHubRowView {
                        section: "REUSABLE ASSETS",
                        badge: "PRESET",
                        label: "Save Active Instrument Preset".into(),
                        value: hub.preset_path.clone(),
                        detail: "Export selected Rack instrument as .preset.json"
                            .into(),
                        selected: hub.cursor == 3,
                        is_action: false,
                        is_dir: false,
                    },
                    FileHubRowView {
                        section: "REUSABLE ASSETS",
                        badge: "MIDI",
                        label: "Save Active MIDI Profile".into(),
                        value: hub.midi_profile_path.clone(),
                        detail: "Export active device profile + user MIDI Learn mappings".into(),
                        selected: hub.cursor == 4,
                        is_action: false,
                        is_dir: false,
                    },
                    FileHubRowView {
                        section: "LIVE TAKE",
                        badge: "TAKE",
                        label: "Save Active Live Take".into(),
                        value: hub.take_path.clone(),
                        detail: "Export active or most recent take (.take.jsonl) · e edit · F Finder"
                            .into(),
                        selected: hub.cursor == 5,
                        is_action: false,
                        is_dir: false,
                    },
                ];
                let preview = match hub.cursor {
                    2 => "Portable Bundle gathers every referenced audio sample into <dir>/samples/ with relative links for easy sharing.".into(),
                    3 => "Saves the active Rack instrument to a standalone reusable JSON preset.".into(),
                    4 => "Saves your active controller profile and custom MIDI Learn mappings as a portable JSON file.".into(),
                    5 => match self.current_take() {
                        Some(t) => format!("Saves live take ({} bars · {} changes) as portable .take.jsonl", t.steps().div_ceil(16).max(1), t.events.len()),
                        None => "No live take recorded yet · press tr to arm take recording, or load a .take.jsonl in Open/Import.".into(),
                    },
                    _ => format!("Active project: \"{}\" · {} bars @ {:.1} BPM · {} instruments", self.doc.project().title, self.doc.project().bars, self.doc.project().bpm, self.doc.project().instruments.len()),
                };
                (rows, preview)
            }
            crate::FileHubTab::Export => {
                let scope_text = match hub.export_scope {
                    crate::ExportScopeKind::FullSong => {
                        format!("Full Song ({} bars)", self.doc.project().bars)
                    }
                    crate::ExportScopeKind::LoopRegion => match self.playing_loop() {
                        Some((s, e)) => format!("Loop Region (B{:02}–{:02})", s / 16 + 1, e / 16),
                        None => {
                            format!("Loop Region (none → full {} bars)", self.doc.project().bars)
                        }
                    },
                    crate::ExportScopeKind::CustomBars => {
                        format!("Custom ({} bars · +/- adjusts)", hub.export_custom_bars)
                    }
                    crate::ExportScopeKind::ActiveTake => match self.current_take() {
                        Some(t) => format!("Active Take ({} bars)", t.steps().div_ceil(16).max(1)),
                        None => "Active Take (no take recorded yet)".into(),
                    },
                };
                let audio_ch_count = self
                    .doc
                    .project()
                    .mixer
                    .channels
                    .iter()
                    .filter(|c| c.kind == cadence_core::mixer::ChannelKind::Audio)
                    .count();
                let dest_detail = match hub.export_mode {
                    crate::ExportMode::MasterWav => {
                        "16-bit PCM stereo WAV · e edit path · F Finder Save".into()
                    }
                    crate::ExportMode::MasterAndStems => format!(
                        "Writes 00-Master.wav + {audio_ch_count} isolated Audio stems · F pick folder"
                    ),
                };
                let rows = vec![
                    FileHubRowView {
                        section: "RENDER FORMAT",
                        badge: "MODE",
                        label: "Export Mode".into(),
                        value: hub.export_mode.label().into(),
                        detail: "Alt+h/l, ,/. or Enter toggles Master Stereo WAV vs Master + Per-Channel Stems".into(),
                        selected: hub.cursor == 0,
                        is_action: false,
                        is_dir: false,
                    },
                    FileHubRowView {
                        section: "RENDER FORMAT",
                        badge: "SCOPE",
                        label: "Time Range".into(),
                        value: scope_text,
                        detail: "Alt+h/l or ,/. cycles Full Song / Loop Region / Custom Bars / Active Take · +/- custom bars".into(),
                        selected: hub.cursor == 1,
                        is_action: false,
                        is_dir: false,
                    },
                    FileHubRowView {
                        section: "RENDER FORMAT",
                        badge: "RATE",
                        label: "Sample Rate".into(),
                        value: format!("{} Hz", hub.export_sample_rate),
                        detail: "Alt+h/l or ,/. cycles 44,100 Hz / 48,000 Hz / 96,000 Hz".into(),
                        selected: hub.cursor == 2,
                        is_action: false,
                        is_dir: false,
                    },
                    FileHubRowView {
                        section: "DESTINATION",
                        badge: if hub.export_mode == crate::ExportMode::MasterWav {
                            "WAV"
                        } else {
                            "DIR"
                        },
                        label: "Output Path".into(),
                        value: hub.export_dest.clone(),
                        detail: dest_detail,
                        selected: hub.cursor == 3,
                        is_action: false,
                        is_dir: hub.export_mode == crate::ExportMode::MasterAndStems,
                    },
                    FileHubRowView {
                        section: "EXECUTE",
                        badge: "RUN",
                        label: match hub.export_mode {
                            crate::ExportMode::MasterWav => "▶ Render Stereo WAV Now".into(),
                            crate::ExportMode::MasterAndStems => {
                                format!("▶ Render Master + {audio_ch_count} Stems Now")
                            }
                        },
                        value: "Enter".into(),
                        detail: "Offline deterministic render through the production mixer & FX graph".into(),
                        selected: hub.cursor == 4,
                        is_action: true,
                        is_dir: false,
                    },
                ];
                let preview = if self.ready() {
                    format!(
                        "Ready to render \"{}\" @ {:.1} BPM · 32-bit float mix, 16-bit PCM WAV output",
                        self.doc.project().title,
                        self.doc.project().bpm
                    )
                } else {
                    "Missing sample assets · inspect Sample Assets from Action Palette (:) before exporting".into()
                };
                (rows, preview)
            }
        };

        Some(FileHubView {
            tabs,
            active_tab: hub.tab,
            current_dir: hub.dir.display().to_string(),
            query: hub.query.clone(),
            searching: hub.searching,
            editing_field: hub.editing_field.clone(),
            rows,
            cursor: hub.cursor,
            preview,
            export_summary: self.last_export_summary.clone(),
        })
    }

    pub(crate) fn inspect_overlay_lines(&self) -> Vec<String> {
        let value = if self.show_assets() {
            self.inspect_assets()
        } else {
            self.inspect(None)
        };
        let text = serde_json::to_string_pretty(&value).unwrap_or_default();
        text.lines().map(str::to_owned).collect()
    }

    pub(super) fn build_overlay_view(&self) -> Option<OverlayView> {
        if let Some((title, fallback)) = self.confirm_prompt() {
            let message = self
                .confirm_detail()
                .unwrap_or_else(|| fallback.to_string());
            return Some(OverlayView::Confirm { title, message });
        }
        if self.input.mode == Mode::FileHub {
            if let Some(hub) = self.build_file_hub_view() {
                return Some(OverlayView::FileHub(hub));
            }
        }
        if let Some(pb) = self.preset_browser_view() {
            return Some(OverlayView::PresetBrowser(pb));
        }
        if let Some(gb) = self.groove_browser_view() {
            return Some(OverlayView::GrooveBrowser(gb));
        }
        if let Some(sp) = self.song_properties_view() {
            return Some(OverlayView::SongProperties(sp));
        }
        if let Some(me) = self.mini_editor_view() {
            return Some(OverlayView::MiniEditor(me));
        }
        if let Some(pal) = self.palette_view() {
            return Some(OverlayView::Palette(pal));
        }
        if let Some(browser) = self.browser_view() {
            return Some(OverlayView::Browser(browser));
        }
        if let Some(draft) = self.authoring_view() {
            return Some(OverlayView::Authoring(draft));
        }
        if let Some(input) = self.value_input_view() {
            return Some(OverlayView::ValueInput(input));
        }
        if let Some(picker) = self.picker_view() {
            return Some(OverlayView::Picker(picker));
        }
        if self.is_arrangement_inspection() {
            if let Some(insp) = self.arrangement_inspection() {
                return Some(OverlayView::EventInspection(insp));
            }
        }
        if self.show_settings() {
            return Some(OverlayView::Settings(self.build_settings_view()));
        }
        if self.show_help() {
            return Some(OverlayView::Help(self.build_help_view()));
        }
        if self.show_inspection() || self.show_assets() {
            let lines = self.inspect_overlay_lines();
            let max_scroll = lines.len().saturating_sub(1);
            return Some(OverlayView::Inspect {
                title: if self.show_assets() {
                    "ASSETS · paths / readiness"
                } else {
                    "INSPECT · definitions / references / scope"
                },
                lines,
                scroll: self.help_ui.overlay_scroll.min(max_scroll),
            });
        }
        if self.show_tutorial() {
            let idx = self
                .help_ui
                .tutorial_step
                .min(TUTORIAL.len().saturating_sub(1));
            let (title, body, command) = TUTORIAL[idx];
            return Some(OverlayView::Tutorial {
                step: idx + 1,
                total: TUTORIAL.len(),
                title,
                body,
                command,
            });
        }
        match self.input.mode {
            Mode::Go
            | Mode::Transport
            | Mode::Window
            | Mode::ArrangeViews
            | Mode::Stage
            | Mode::Interpolation => Some(OverlayView::PrefixMenu {
                title: self.mode_name(),
                items: self.hints(),
            }),
            _ => None,
        }
    }
}
