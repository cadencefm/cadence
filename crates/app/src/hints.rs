//! Mode names, contextual key hints and the tutorial.
use super::*;

pub const TUTORIAL: &[(&str, &str, &str)] = &[
    (
        "1 · Material before sound",
        "Create a five-step source. Its notes do not belong to a lane or instrument.",
        "pattern notes Motif 0 2 4 7 6",
    ),
    (
        "2 · An independent clock",
        "Create a held harmonic value source with a 64-step period.",
        "pattern degrees Harmony 64 0:0 24:-2 32:2 56:-1",
    ),
    (
        "3 · Retain the relationship",
        "Combine source notes and degree values without expanding them into variants.",
        "derive Moving add Motif Harmony",
    ),
    (
        "4 · Give a sound a destination",
        "This creates a Rack instrument and its default Audio channel.",
        "instrument Lead reese",
    ),
    (
        "5 · Join material to sound",
        "A part joins material to an instrument while keeping both identities independent.",
        "part LeadPart Moving Lead",
    ),
    (
        "6 · Place a performance",
        "Create a lane, then place LeadPart with a Song-anchored clock.",
        "lane LeadLane",
    ),
    (
        "7 · Schedule it",
        "Song and Restart are explicit. One placement can span several source cycles.",
        "place LeadPart LeadLane 1 4 song",
    ),
    (
        "8 · Listen and inspect",
        "Space plays the song. Press I (or Inspect Pattern Dependencies in Palette) to reveal source dependencies and consumers.",
        "inspect Moving",
    ),
    (
        "9 · Edit a shared source",
        "Open Harmony and change one value; every dependent part updates.",
        "open Harmony",
    ),
    (
        "10 · Save the relationships",
        "The project stores definitions, parts, and placements—not expanded phrase banks.",
        "w cadence-project.json",
    ),
];

impl App {
    pub fn mode_name(&self) -> &'static str {
        match self.input.mode {
            Mode::Normal => "NORMAL",
            Mode::Insert => "STEP INSERT",
            Mode::Jam => "JAM",
            Mode::Stage => "STAGE",
            Mode::Pick => "PICK",
            Mode::Authoring => "AUTHORING",
            Mode::Create => "NEW",
            Mode::Workflow => "WORKFLOW",
            Mode::Browser => "SAMPLES",
            Mode::PresetBrowser => "PRESETS",
            Mode::GrooveBrowser => "FILTER GROOVES",
            Mode::SongProperties => "SONG PROPS",
            Mode::MiniEditor => "MINI EDITOR",
            Mode::ValueInput => "VALUE ENTRY",
            Mode::Interpolation => "INTERPOLATION",
            Mode::Palette => "PALETTE",
            Mode::Search(_) => "SEARCH",
            Mode::Go => "GO",
            Mode::Transport => "TRANSPORT",
            Mode::Window => "WINDOW",
            Mode::Help => "HELP",
            Mode::Settings => "SETTINGS",
            Mode::FileHub => "FILES",
            Mode::Inspect => "INSPECT",
            Mode::Assets => "ASSETS",
            Mode::Tutorial => "TUTORIAL",
            Mode::Confirm(_) => "CONFIRM",
            Mode::ArrangeViews => "ARRANGE VIEWS",
            Mode::ArrangementInspect => "EVENT PROVENANCE",
        }
    }

    pub fn hints(&self) -> Vec<(String, String)> {
        let pairs = match &self.input.mode {
            Mode::Stage => vec![
                ("Enter/f", "fire on boundary"),
                ("x", "discard"),
                ("Esc", "keep staging"),
            ],
            Mode::Jam if self.input.lane_layer.is_some() => vec![
                ("1–9", "launch live override"),
                ("lane key", "toggle override"),
                ("Alt+x/c", "release/keep"),
                ("other keys", "close layer"),
                ("Esc", "back"),
            ],
            Mode::Jam if self.staging.is_some() => {
                let mut hints = vec![("1–0", "section"), ("q–p", "stage lane")];
                if self.jam_bank_count() > 1 {
                    hints.push(("Tab", "bank"));
                }
                hints.extend([
                    ("Enter", "fire staged"),
                    ("Space", "play"),
                    ("Esc", "leave"),
                ]);
                hints
            }
            Mode::Jam => {
                let mut hints = vec![("1–0", "section"), ("q–p", "lane")];
                if self.jam_bank_count() > 1 {
                    hints.push(("Tab", "bank"));
                }
                hints.extend([
                    ("z–,", "notes"),
                    ("Shift", "accent"),
                    ("</>", "octave"),
                    ("Alt+m/s/x/c", "mute/solo/rel/keep"),
                    ("Bksp", "cancel queued"),
                    ("Esc", "leave"),
                ]);
                hints
            }
            Mode::Create => vec![
                ("n/r/d/v/s/g/m/e/a", "pattern/derive"),
                ("i/b/l/o/p/t", "song objects"),
                ("c/f/w/u/y", "ch/fx/send/duck/mod"),
                ("j/k", "select"),
                ("Esc", "cancel"),
            ],
            Mode::Browser => {
                if self.modal.browser().is_some_and(|b| b.searching) {
                    vec![
                        ("type", "filter"),
                        ("↑/↓", "select"),
                        ("Enter", "apply filter"),
                        ("Ctrl+u", "clear"),
                        ("Esc", "clear search"),
                    ]
                } else {
                    vec![
                        ("j/k", "select"),
                        ("/", "filter"),
                        ("A", "audition"),
                        ("Enter", "choose"),
                        ("r/R", "reload/rescan"),
                        (
                            "Esc",
                            if !self.assets.library.query.is_empty() {
                                "clear filter"
                            } else {
                                "back"
                            },
                        ),
                    ]
                }
            }
            Mode::Workflow | Mode::Authoring => vec![
                ("Enter", "accept field"),
                ("j/k", "choose"),
                ("/", "filter"),
                ("Shift+Tab", "back"),
                ("Esc", "cancel"),
            ],
            Mode::PresetBrowser => {
                if self.modal.preset_browser().is_some_and(|p| p.searching) {
                    vec![
                        ("type", "filter presets"),
                        ("↑/↓", "preview"),
                        ("Enter", "apply filter"),
                        ("Ctrl+u", "clear"),
                        ("Esc", "cancel filter"),
                    ]
                } else {
                    vec![
                        ("h/l H/L", "category"),
                        ("j/k", "live preview"),
                        ("s", "save preset"),
                        ("R", "reload"),
                        ("A", "audition"),
                        ("/", "filter"),
                        ("Space", "play/pause"),
                        ("Enter", "load preset"),
                        (
                            "Esc",
                            if self
                                .modal
                                .preset_browser()
                                .is_some_and(|p| !p.query.is_empty())
                            {
                                "clear filter"
                            } else {
                                "restore"
                            },
                        ),
                    ]
                }
            }
            Mode::GrooveBrowser => {
                if self.modal.groove_browser().is_some_and(|g| g.searching) {
                    vec![
                        ("type", "filter grooves"),
                        ("↑/↓", "preview"),
                        ("Enter", "apply filter"),
                        ("Ctrl+u", "clear"),
                        ("Esc", "cancel filter"),
                    ]
                } else {
                    vec![
                        ("j/k", "live preview"),
                        ("/", "filter"),
                        ("f", "fork pattern"),
                        ("s", "save groove"),
                        ("o", "load bank"),
                        ("Space", "play/pause"),
                        ("Enter", "load groove"),
                        (
                            "Esc",
                            if self
                                .modal
                                .groove_browser()
                                .is_some_and(|g| !g.query.is_empty())
                            {
                                "clear filter"
                            } else {
                                "restore"
                            },
                        ),
                    ]
                }
            }
            Mode::SongProperties
                if self
                    .modal
                    .song_props()
                    .is_some_and(|sp| sp.title_buf.is_some()) =>
            {
                vec![
                    ("type", "song title"),
                    ("Enter", "rename"),
                    ("Ctrl+u", "clear"),
                    ("Esc", "cancel"),
                ]
            }
            Mode::SongProperties => vec![
                ("j/k", "row"),
                ("w/b", "group"),
                ("Alt+h/l ,/.", "adjust"),
                ("Enter", "exact/rename"),
                ("Alt+Bksp", "reset"),
                ("u/U", "undo/redo"),
                ("Esc", "close"),
            ],
            Mode::MiniEditor => vec![
                ("type", "mini code"),
                ("Enter", "commit when valid"),
                ("Ctrl+u", "clear"),
                ("Esc", "cancel"),
            ],
            Mode::ValueInput => vec![
                ("Enter", "commit value"),
                ("Esc", "cancel"),
                ("Ctrl+u", "clear"),
            ],
            Mode::Interpolation => vec![
                ("h", "Hold"),
                ("r", "Ramp"),
                ("m", "toggle Hold/Ramp"),
                ("Enter", "keep"),
                ("Esc", "cancel"),
            ],
            Mode::Palette => vec![
                ("type", "fuzzy search or cmd"),
                ("↑/↓ Tab", "select"),
                ("Enter", "run"),
                ("Ctrl+u", "clear"),
                ("Esc", "close"),
            ],
            Mode::Window => vec![
                ("z", "zoom deck"),
                ("=/-/0", "zoom view"),
                ("1–4", "layout preset"),
                ("p", "pin/unpin"),
                ("x", "close"),
                ("o", "close others"),
                ("hjkl", "focus deck"),
                ("w", "next"),
                ("Esc", "cancel"),
            ],
            Mode::Search(_) => vec![
                ("type", "filter"),
                ("↑/↓", "select"),
                ("Enter", "keep"),
                ("Esc", "clear"),
            ],
            Mode::ArrangeViews => vec![
                ("p", "placements"),
                ("o", "evaluated overview"),
                ("r", "lane roll"),
                ("Esc", "cancel"),
            ],
            Mode::ArrangementInspect => vec![
                ("j/k", "source row"),
                ("s/Enter", "follow source"),
                ("o", "local occurrence"),
                ("i", "instrument"),
                ("Esc", "back"),
            ],
            Mode::Transport => vec![
                ("f", "follow"),
                ("g", "cue bar"),
                ("l", "loop section"),
                ("x", "clear loop"),
                ("X", "cancel all"),
                ("q/Q", "quantize/cycle"),
                ("E", "edit timing"),
                ("e", "end rule"),
                ("s", "stop"),
                (
                    "r",
                    if self.takes.armed {
                        "disarm take"
                    } else {
                        "arm take"
                    },
                ),
                ("R", "restart loop"),
                ("a/A", "rec audio/dialog"),
                ("i", "input monitor"),
                ("c/p", "cue/play cursor"),
                ("C", "clear clips"),
                ("b", "bounce"),
                ("Esc", "cancel"),
            ],
            Mode::Go => vec![
                ("a", "Arrange"),
                ("b", "Bank"),
                ("p", "Pattern"),
                ("w", "Sample/Wave"),
                ("m", "Mixer"),
                ("t", "Take"),
                ("G", "Graph"),
                ("d", "Devices"),
                ("k", "Macros"),
                ("f", "Files"),
                ("i", "Instrument"),
                ("j", "Jam"),
                ("s/S", "Source/Samples"),
                ("o", "Output"),
                ("P", "Song Props"),
                (",", "Settings"),
            ],
            Mode::Help if self.help_ui.searching => vec![
                ("type", "filter help"),
                ("↑/↓", "scroll"),
                ("Enter", "apply filter"),
                ("Ctrl+u", "clear"),
                ("Esc", "clear search"),
            ],
            Mode::Help => vec![
                ("H/L Tab", "tab"),
                ("j/k", "scroll"),
                ("/", "search"),
                (
                    "Esc",
                    if !self.help_ui.query.is_empty() {
                        "clear filter"
                    } else {
                        "close"
                    },
                ),
            ],
            Mode::Settings if self.settings_ui.editing.is_some() => vec![
                ("type", "folder path"),
                ("Enter", "add dir"),
                ("Ctrl+u", "clear"),
                ("Esc", "cancel"),
            ],
            Mode::Settings => vec![
                ("j/k", "row"),
                ("w/b", "group"),
                ("Alt+h/l ,/.", "adjust"),
                ("Enter", "exact/toggle"),
                ("Alt+Bksp", "reset"),
                ("a/x", "add/del dir"),
                ("F", "Finder"),
                ("Ctrl+s", "save"),
                ("r", "defaults"),
                ("Esc", "close"),
            ],
            Mode::FileHub if self.modal.file_hub().is_some_and(|h| h.searching) => vec![
                ("type", "filter"),
                ("↑/↓", "select"),
                ("Enter", "apply filter"),
                ("Ctrl+u", "clear"),
                ("Esc", "clear search"),
            ],
            Mode::FileHub
                if self
                    .modal
                    .file_hub()
                    .is_some_and(|h| h.editing_field.is_some()) =>
            {
                vec![
                    ("type", "path"),
                    ("Enter", "apply path"),
                    ("Ctrl+u", "clear"),
                    ("Esc", "cancel"),
                ]
            }
            Mode::FileHub => {
                use crate::file_hub::FileHubTab;
                let (tab, has_query) = self
                    .modal
                    .file_hub()
                    .map_or((FileHubTab::Quick, false), |h| (h.tab, !h.query.is_empty()));
                let mut items = vec![("H/L 1–5", "tab"), ("j/k", "select")];
                match tab {
                    FileHubTab::Quick => {
                        items.extend([("Enter", "open"), ("/", "search all")]);
                    }
                    FileHubTab::Browse => {
                        items.extend([
                            ("Enter", "open/dir"),
                            ("Bksp", "up"),
                            ("~ / .", "home/proj"),
                            ("/", "filter"),
                        ]);
                    }
                    FileHubTab::Import => {
                        items.extend([
                            ("Enter", "import/dir"),
                            ("Bksp", "up"),
                            ("~ / .", "home/proj"),
                            ("/", "filter"),
                            ("R", "reload"),
                        ]);
                    }
                    FileHubTab::Save => {
                        items.extend([("Enter", "run"), ("e", "edit path")]);
                    }
                    FileHubTab::Export => {
                        items.extend([
                            ("Alt+h/l ,/.", "adjust"),
                            ("e", "edit path"),
                            ("Enter", "render"),
                        ]);
                    }
                }
                items.extend([
                    ("F", "Finder"),
                    ("Space", "play/pause"),
                    ("Esc", if has_query { "clear filter" } else { "close" }),
                ]);
                items
            }
            Mode::Inspect => vec![("j/k", "scroll"), ("Esc", "back")],
            Mode::Assets => vec![("j/k", "scroll"), ("r", "reload samples"), ("Esc", "back")],
            Mode::Tutorial => vec![("j/k", "lesson"), ("Enter", "try command"), ("Esc", "back")],
            Mode::Confirm(ConfirmAction::Quit) if self.dirty() => vec![
                (
                    "s",
                    if self.doc.is_untitled() {
                        "save as"
                    } else {
                        "save & quit"
                    },
                ),
                ("y/Enter", "discard & quit"),
                ("Esc", "cancel"),
            ],
            Mode::Confirm(_) => vec![("y/Enter", "confirm"), ("Esc", "cancel")],
            Mode::Insert => vec![
                ("1–8", "degree"),
                ("z–,", "piano notes"),
                ("Shift", "accent"),
                ("</>", "octave"),
                (".", "rest"),
                ("Esc", "normal"),
            ],
            Mode::Pick if self.picker_searching() => vec![
                ("type", "filter"),
                ("↑/↓", "select"),
                ("Enter", "apply filter"),
                ("Ctrl+u", "clear"),
                ("Esc", "clear search"),
            ],
            Mode::Pick => vec![
                ("1–9/Enter", "choose"),
                ("j/k", "move"),
                ("/", "filter"),
                ("A", "audition"),
                (
                    "Esc",
                    if !self.picker_query().is_empty() {
                        "clear filter"
                    } else {
                        "back"
                    },
                ),
            ],
            Mode::Normal if self.staging.is_some() => vec![
                ("S/Alt+Enter", "fire"),
                ("u", "pop"),
                ("Esc", "discard"),
                ("Alt+m/s/x", "stage lane"),
                ("Alt+1–0", "stage section"),
            ],
            Mode::Normal if self.deck_focused() && matches!(self.detail, Detail::Placement(_)) => {
                vec![
                    ("j/k", "field"),
                    ("Alt+h/l ,/.", "adjust"),
                    ("Enter", "open/exact"),
                    ("r", "replace"),
                    ("f/o", "fork/local"),
                    ("x/X", "del/clear exc"),
                    ("A", "audition"),
                    ("Esc", "back"),
                ]
            }
            Mode::Normal if self.deck_focused() && matches!(self.detail, Detail::Part(_)) => {
                vec![
                    ("j/k", "field"),
                    ("Alt+h/l ,/.", "adjust"),
                    ("Enter", "open/exact"),
                    ("a", "add link"),
                    ("e", "links"),
                    ("r", "rebind/rename"),
                    ("X", "delete part"),
                    ("A", "audition"),
                    ("Esc", "back"),
                ]
            }
            Mode::Normal if self.deck_focused() && matches!(self.detail, Detail::Channel(_)) => {
                vec![
                    ("j/k", "row"),
                    ("w/b", "group/byp"),
                    ("Alt+h/l ,/.", "adjust"),
                    ("Alt+j/k", "reorder fx"),
                    ("Enter", "open/exact"),
                    ("a/f/d", "add/fx/duck"),
                    ("e/r/v", "links/ren/move"),
                    ("C", "clear clip"),
                    ("m/s", "mute/solo"),
                ]
            }
            Mode::Normal if self.deck_focused() && matches!(self.detail, Detail::Effect(_)) => {
                if self.is_filter_effect_detail() {
                    vec![
                        ("j/k", "param"),
                        ("w/b", "group"),
                        ("Alt+h/l ,/.", "adjust"),
                        ("Alt+j/k", "reorder fx"),
                        ("Enter", "exact/pick"),
                        ("P/f", "grooves/fork"),
                        ("e", "links"),
                        ("r/v", "rename/move"),
                        ("m/X", "bypass/del"),
                        ("Esc", "back"),
                    ]
                } else {
                    vec![
                        ("j/k", "param"),
                        ("w/b", "group"),
                        ("Alt+h/l ,/.", "adjust"),
                        ("Alt+j/k", "reorder fx"),
                        ("Enter", "exact"),
                        ("e", "links"),
                        ("r/v", "rename/move"),
                        ("m/X", "bypass/del"),
                        ("M", "MIDI learn"),
                        ("Esc", "back"),
                    ]
                }
            }
            Mode::Normal if self.sample_deck_focused() => vec![
                ("h/l H/L", "scrub"),
                ("w/b", "slice"),
                ("=/-", "zoom"),
                ("[/] {/}", "crop/loop"),
                ("s/x/X", "split/del/clr"),
                ("d/D", "auto/grid"),
                ("Alt+h/l ,/.", "nudge"),
                ("j/k", "ctrl"),
                ("v/m", "snap/loop"),
                ("A", "play"),
                ("K", "kit"),
                ("W", "write RIFF"),
                ("B", "browse"),
            ],
            Mode::Normal if self.deck_focused() && matches!(self.detail, Detail::Instrument(_)) => {
                match self.rack_ui.tab {
                    RackTab::Panel => vec![
                        ("H/L", "tab"),
                        ("h/l", "module"),
                        ("j/k", "param"),
                        ("Alt+h/l ,/.", "adjust"),
                        ("Enter", "exact"),
                        ("a/x", "add/del mod"),
                        ("p/e/r", "pin/links/ren"),
                        ("o/X", "route/del inst"),
                        ("P", "presets"),
                        ("B", "sample"),
                        ("W", "wave"),
                        ("A", "audition"),
                    ],
                    RackTab::Patchbay => vec![
                        ("H/L", "tab"),
                        ("hjkl", "node"),
                        ("w/b", "cable"),
                        ("Alt+h/l ,/.", "cable amt"),
                        ("c/C/x", "patch/amt/cut"),
                        ("Enter", "panel/exact"),
                        ("a/e/r", "add/links/ren"),
                        ("p/o/X", "pin/route/del"),
                        ("A", "audition"),
                    ],
                    RackTab::Zones => vec![
                        ("H/L", "tab"),
                        ("h/l", "zone"),
                        ("j/k", "ctrl"),
                        ("Alt+h/l ,/.", "adjust"),
                        ("Alt+j/k", "reorder"),
                        ("a/d/x", "add/dup/del"),
                        ("e/r/o/X", "links/ren/rt/del"),
                        ("Enter", "exact/browse"),
                        ("A", "audition"),
                    ],
                }
            }
            Mode::Normal if self.deck_focused() => vec![
                ("j/k", "control"),
                ("w/b", "group"),
                ("Alt+h/l ,/.", "adjust"),
                ("Alt+Bksp", "reset"),
                ("Enter", "open/exact"),
                ("Tab", "next deck"),
                ("Esc", "back"),
            ],
            Mode::Normal if self.value_canvas() => vec![
                ("h/l", "step"),
                ("H/L", "bar"),
                ("w/b", "point"),
                ("Alt+j/k ,/.", "value"),
                ("Alt+h/l", "slide"),
                ("Enter", "exact"),
                ("e", "consumers"),
                ("m", "Hold/Ramp"),
                ("x", "del point"),
            ],
            Mode::Normal
                if self.pattern_canvas_focused()
                    && self.scope == EditScope::Definition
                    && self
                        .selected_pattern_object()
                        .is_some_and(|p| matches!(p.kind, PatternKind::Rhythm { .. })) =>
            {
                vec![
                    ("h/l", "step"),
                    ("H/L", "bar"),
                    ("w/b", "hit"),
                    ("Enter/x", "toggle hit"),
                    ("r/d/f/I/X", "edit/der/fork/insp/del"),
                    ("y/p", "yank/paste"),
                    ("A", "audition"),
                ]
            }
            Mode::Normal
                if self.pattern_canvas_focused()
                    && self.scope == EditScope::Definition
                    && self
                        .selected_pattern_object()
                        .is_some_and(|p| !p.kind.sources().is_empty()) =>
            {
                vec![
                    ("h/l", "step"),
                    ("H/L", "bar"),
                    ("j/k", "source"),
                    ("Enter/gs", "open source"),
                    ("r/d", "edit/derive"),
                    ("F/f/I/X", "frz/fork/insp/del"),
                    ("=/-", "zoom"),
                    ("A", "audition"),
                ]
            }
            Mode::Normal
                if self.pattern_canvas_focused()
                    && self.scope == EditScope::Definition
                    && self
                        .selected_pattern_object()
                        .is_some_and(|p| matches!(p.kind, PatternKind::Mini { .. })) =>
            {
                vec![
                    ("h/l", "step"),
                    ("H/L", "bar"),
                    ("Enter", "mini editor"),
                    ("r/d", "edit/derive"),
                    ("F/f/I/X", "frz/fork/insp/del"),
                    ("=/-", "zoom"),
                    ("y", "yank"),
                    ("A", "audition"),
                ]
            }
            Mode::Normal
                if self.pattern_canvas_focused()
                    && self.scope == EditScope::Definition
                    && self.selected_pattern_object().is_some_and(|p| {
                        matches!(
                            p.kind,
                            PatternKind::Euclidean { .. } | PatternKind::SeededValues { .. }
                        )
                    }) =>
            {
                vec![
                    ("h/l", "step"),
                    ("H/L", "bar"),
                    ("r/d", "edit/derive"),
                    ("F/f/I/X", "frz/fork/insp/del"),
                    ("=/-", "zoom"),
                    ("y", "yank"),
                    ("A", "audition"),
                ]
            }
            Mode::Normal if self.take_focused() && self.takes.visual_anchor.is_some() => vec![
                ("h/l", "extend range"),
                ("v", "lock range"),
                ("Enter/Alt+c", "keep range"),
                ("Esc", "clear marks"),
            ],
            Mode::Normal if self.take_focused() => vec![
                ("h/l", "bar"),
                ("v", "visual mark"),
                ("Enter/Alt+c", "keep range"),
                ("Ctrl+s", "save"),
                ("Ctrl+Shift+s", "save as"),
                ("o", "load"),
                ("x", "clear marks"),
                ("X", "discard"),
                ("tr", if self.takes.armed { "disarm" } else { "arm" }),
                ("Esc", "back"),
            ],
            Mode::Normal if self.graph_focused() => vec![
                ("hjkl", "node"),
                ("Enter", "open"),
                ("o/r", "route"),
                ("a s/m/d", "add link"),
                ("e", "edit cable"),
                ("x/X", "unlink/all"),
                ("Esc", "back"),
            ],
            Mode::Normal if self.midi_focused() => match self.devices.tab {
                DevicesTab::Audio => vec![
                    ("H/L", "tab"),
                    ("j/k", "row"),
                    ("w/b", "group"),
                    ("Alt+h/l ,/.", "cycle"),
                    ("Alt+Bksp", "reset"),
                    ("Enter", "pick/run"),
                    ("r", "rescan"),
                    ("Ctrl+s", "save"),
                ],
                DevicesTab::Midi => vec![
                    ("H/L", "tab"),
                    ("j/k", "row"),
                    ("w/b", "group"),
                    ("Alt+h/l ,/.", "cycle"),
                    ("Enter", "pick/learn"),
                    ("a", "add map"),
                    ("v/r", "inv/rescan"),
                    ("x/X", "del/clear"),
                    ("R", "MIDI rec"),
                    ("Ctrl+s", "save"),
                ],
            },
            Mode::Normal if self.macros_focused() => vec![
                ("h/l 1–8", "slot"),
                ("j/k", "dial/binding"),
                ("a/r/e", "bind/ren/mod"),
                ("Alt+h/l ,/.", "dial/min"),
                ("Alt+H/L </>", "coarse/max"),
                ("Alt+j/k", "reorder"),
                ("Enter/v", "exact/inv"),
                ("x/X", "unbind/clear"),
            ],
            Mode::Normal if self.evaluated_arrange_canvas() => vec![
                ("h/l", "step"),
                ("j/k", "lane"),
                ("w/b", "event"),
                ("H/L", "section"),
                ("=/-", "zoom"),
                ("Enter", "provenance"),
                ("v", "view"),
            ],
            Mode::Normal if self.bank_focused() && self.view_state.bank_mode == BankMode::Parts => {
                vec![
                    ("hjkl", "lane/part"),
                    ("Enter/Shift+Enter", "launch/now"),
                    ("a", "add part"),
                    ("r", "phase"),
                    ("X", "drop"),
                    ("x", "release"),
                    ("A", "audition"),
                    ("v", "material"),
                ]
            }
            Mode::Normal if self.bank_focused() => vec![
                ("j/k", "select"),
                ("Enter", "open"),
                ("r/f/F", "edit/fork/freeze"),
                ("I/X", "inspect/del"),
                ("A", "audition"),
                ("/", "find"),
                ("v", "parts"),
                ("n", "new"),
            ],
            Mode::Normal => match self.focused_canvas() {
                Some(Workspace::Arrange) => vec![
                    ("hjkl", "move"),
                    ("H/L", "section"),
                    ("Alt+hjkl", "move/lane"),
                    (",/. </>", "resize"),
                    ("=/-", "zoom"),
                    ("Enter", "open/place"),
                    ("d/r/X", "dup/edit/del"),
                    ("x", "del/release"),
                    ("m/s", "mute/solo"),
                    ("v", "view"),
                ],
                Some(Workspace::Pattern) => vec![
                    ("h/l", "step"),
                    ("H/L", "bar"),
                    ("j/k", "voice"),
                    ("Alt+j/k", "pitch"),
                    ("Alt+h/l", "gate"),
                    (",/.", "vel"),
                    ("c/C", "cond"),
                    ("o/m/i", "chord/mute/ins"),
                    ("x/X", "del"),
                    ("r/f/F/I", "edit/fork/frz/insp"),
                    ("A", "audition"),
                ],
                Some(Workspace::Mixer) => vec![
                    ("h/l", "strip"),
                    ("j/k", "row"),
                    ("Alt+h/l ,/.", "adjust"),
                    ("Alt+j/k", "fader/reorder"),
                    ("Enter", "open/exact"),
                    ("a", "add"),
                    ("o/e", "channel/links"),
                    ("m/s/b", "mute/solo/byp"),
                    ("C", "clear clip"),
                    ("x/X", "del item/ch"),
                ],
                None => vec![
                    ("Tab", "next deck"),
                    ("g", "decks"),
                    ("z", "layout"),
                    ("?", "guide"),
                ],
            },
        };
        pairs
            .into_iter()
            .map(|(a, b)| (a.into(), b.into()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadence_core::material::{Lane, Project, Section};
    use cadence_core::runtime::AudioSnapshot;

    #[test]
    fn jam_hints_and_hud_surface_tab_bank_only_when_exceeding_ten_lanes_or_sections() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("song.json");
        let snap = AudioSnapshot::default();

        // 1. Default demo (<= 10 lanes and <= 10 sections): no `Tab bank`.
        let mut app = App::new(Project::demo(), path.clone()).unwrap();
        assert!(app.project().lanes.len() <= LANE_KEYS.len());
        assert!(app.project().sections.len() <= SECTION_KEYS.len());
        app.press_keys("gj").unwrap();
        assert!(
            !app.hints().iter().any(|(k, d)| k == "Tab" && d == "bank"),
            "Jam hints must omit Tab bank when lanes and sections fit in one bank"
        );
        app.staging = Some(Vec::new());
        assert!(
            !app.hints().iter().any(|(k, d)| k == "Tab" && d == "bank"),
            "Staged Jam hints must omit Tab bank when lanes and sections fit in one bank"
        );
        app.staging = None;
        let hud = app.frame_view(&snap, "ok").jam_hud.expect("jam_hud");
        assert_eq!(hud.bank, 0);
        assert_eq!(hud.bank_count, 1);

        // 2. Exceeding 10 lanes (11 lanes): surfaces `Tab bank` in Jam hints + JamHudView.
        let mut project_lanes = Project::demo();
        let mut next_id = project_lanes.max_id() + 1;
        while project_lanes.lanes.len() < 11 {
            let n = project_lanes.lanes.len() + 1;
            project_lanes.lanes.push(Lane {
                id: next_id,
                name: format!("Extra Lane {n}"),
                parts: Vec::new(),
                placements: Vec::new(),
            });
            next_id += 1;
        }
        let mut app_lanes = App::new(project_lanes, path.clone()).unwrap();
        app_lanes.press_keys("gj").unwrap();
        assert!(
            app_lanes
                .hints()
                .iter()
                .any(|(k, d)| k == "Tab" && d == "bank"),
            "Jam hints must surface Tab bank when lanes exceed 10"
        );
        app_lanes.staging = Some(Vec::new());
        assert!(
            app_lanes
                .hints()
                .iter()
                .any(|(k, d)| k == "Tab" && d == "bank"),
            "Staged Jam hints must surface Tab bank when lanes exceed 10"
        );
        app_lanes.staging = None;
        let hud1 = app_lanes.frame_view(&snap, "ok").jam_hud.expect("jam_hud");
        assert_eq!(hud1.bank, 0);
        assert_eq!(hud1.bank_count, 2);
        app_lanes
            .handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
            .unwrap();
        let hud2 = app_lanes.frame_view(&snap, "ok").jam_hud.expect("jam_hud");
        assert_eq!(hud2.bank, 1);
        assert_eq!(hud2.bank_count, 2);
        assert_eq!(hud2.lanes.len(), 1);
        assert_eq!(hud2.lanes[0].0, 'q');
        assert_eq!(hud2.lanes[0].1, "Extra Lane 11");

        // 3. Exceeding 10 sections (11 sections, <= 10 lanes): surfaces `Tab bank` + bank 2 section!
        let mut project_secs = Project::demo();
        let mut next_id = project_secs.max_id() + 1;
        while project_secs.sections.len() < 11 {
            let n = project_secs.sections.len() + 1;
            project_secs.sections.push(Section {
                id: next_id,
                name: format!("Sec {n}"),
                start_step: (n as u32 - 1) * 16,
                length_steps: 16,
            });
            next_id += 1;
        }
        let mut app_secs = App::new(project_secs, path).unwrap();
        app_secs.press_keys("gj").unwrap();
        assert!(
            app_secs
                .hints()
                .iter()
                .any(|(k, d)| k == "Tab" && d == "bank"),
            "Jam hints must surface Tab bank when sections exceed 10"
        );
        app_secs
            .handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
            .unwrap();
        let hud_sec2 = app_secs.frame_view(&snap, "ok").jam_hud.expect("jam_hud");
        assert_eq!(hud_sec2.bank, 1);
        assert_eq!(hud_sec2.bank_count, 2);
        assert_eq!(hud_sec2.sections.len(), 1);
        assert_eq!(hud_sec2.sections[0].0, '1');
        assert_eq!(hud_sec2.sections[0].1, "Sec 11");
    }
}
