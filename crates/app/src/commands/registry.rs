//! Authoritative single-point-of-truth Ex command verb registry.
//!
//! Both `App::execute_command` and Fuzzy Action Palette passthrough recognition
//! (`palette::is_recognized_command_line`) derive directly from `EX_VERBS` / `ExVerb`,
//! making command dispatch and palette recognition impossible to drift.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LiveVerb {
    Launch,
    Release,
    Stage,
    Unstage,
    Undo,
    Redo,
    Cancel,
    Quantize,
    Tolerance,
    Advance,
    Take,
    Keep,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ArrangeVerb {
    Stop,
    Restart,
    Cue { play: bool },
    Monitor,
    Follow,
    Bpm,
    Bars,
    Title,
    Key,
    Lane,
    Place,
    Placement,
    Section,
    End,
    Loop,
    Arrange,
    InspectEvent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MaterialVerb {
    Pattern,
    Mini,
    Open,
    Part,
    Derive,
    Fork,
    Freeze,
    Len,
    Note,
    Value,
    ValueMode,
    Local,
    Source,
    Inherit,
    Link,
    Unlink,
    Audition,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InstrumentVerb {
    Instrument,
    Kit,
    Sampler,
    Preset,
    PresetSave,
    PresetLoad,
    Tab,
    ReloadSamples,
    ReloadPresets,
    Samples,
    Sample,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MixerVerb {
    Mod,
    Macro,
    Duck,
    Param,
    Channel,
    Fx,
    Route,
    RouteInstrument,
    Send,
    Clips,
    Rename,
    Delete,
    Mixer,
    Wav,
    Bundle,
    Stems,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionVerb {
    New { force: bool },
    Write,
    Open { force: bool },
    ImportRbs { force: bool },
    Quit { force: bool },
    WriteQuit,
    Inspect,
    Assets,
    Deck,
    Tips,
    Tutorial,
    Help,
    Settings,
    Files,
    SaveAs,
    Zone,
    Patch,
    Unpatch,
    Pin,
    Unpin,
    Bounce,
    Record,
    Audio,
    Panic,
    Midi,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExVerb {
    Live(LiveVerb),
    Arrange(ArrangeVerb),
    Material(MaterialVerb),
    Instrument(InstrumentVerb),
    Mixer(MixerVerb),
    Session(SessionVerb),
}

/// Complete table of every recognized Ex command verb string and its typed dispatcher variant.
pub(crate) const EX_VERBS: &[(&str, ExVerb)] = &[
    // Session / file / workspace / rack / audio / midi
    ("new", ExVerb::Session(SessionVerb::New { force: false })),
    ("new!", ExVerb::Session(SessionVerb::New { force: true })),
    ("w", ExVerb::Session(SessionVerb::Write)),
    ("write", ExVerb::Session(SessionVerb::Write)),
    ("e", ExVerb::Session(SessionVerb::Open { force: false })),
    ("e!", ExVerb::Session(SessionVerb::Open { force: true })),
    (
        "open-project",
        ExVerb::Session(SessionVerb::Open { force: false }),
    ),
    (
        "open-project!",
        ExVerb::Session(SessionVerb::Open { force: true }),
    ),
    (
        "import-rbs",
        ExVerb::Session(SessionVerb::ImportRbs { force: false }),
    ),
    (
        "import-rbs!",
        ExVerb::Session(SessionVerb::ImportRbs { force: true }),
    ),
    ("q", ExVerb::Session(SessionVerb::Quit { force: false })),
    ("q!", ExVerb::Session(SessionVerb::Quit { force: true })),
    ("wq", ExVerb::Session(SessionVerb::WriteQuit)),
    ("inspect", ExVerb::Session(SessionVerb::Inspect)),
    ("assets", ExVerb::Session(SessionVerb::Assets)),
    ("deck", ExVerb::Session(SessionVerb::Deck)),
    ("tips", ExVerb::Session(SessionVerb::Tips)),
    ("tutorial", ExVerb::Session(SessionVerb::Tutorial)),
    ("help", ExVerb::Session(SessionVerb::Help)),
    ("settings", ExVerb::Session(SessionVerb::Settings)),
    ("config", ExVerb::Session(SessionVerb::Settings)),
    ("files", ExVerb::Session(SessionVerb::Files)),
    ("hub", ExVerb::Session(SessionVerb::Files)),
    ("saveas", ExVerb::Session(SessionVerb::SaveAs)),
    ("zone", ExVerb::Session(SessionVerb::Zone)),
    ("patch", ExVerb::Session(SessionVerb::Patch)),
    ("unpatch", ExVerb::Session(SessionVerb::Unpatch)),
    ("pin", ExVerb::Session(SessionVerb::Pin)),
    ("unpin", ExVerb::Session(SessionVerb::Unpin)),
    ("bounce", ExVerb::Session(SessionVerb::Bounce)),
    ("resample", ExVerb::Session(SessionVerb::Bounce)),
    ("rec", ExVerb::Session(SessionVerb::Record)),
    ("record", ExVerb::Session(SessionVerb::Record)),
    ("audio", ExVerb::Session(SessionVerb::Audio)),
    ("panic", ExVerb::Session(SessionVerb::Panic)),
    ("midi", ExVerb::Session(SessionVerb::Midi)),
    // Live performance & takes
    ("launch", ExVerb::Live(LiveVerb::Launch)),
    ("release", ExVerb::Live(LiveVerb::Release)),
    ("stage", ExVerb::Live(LiveVerb::Stage)),
    ("unstage", ExVerb::Live(LiveVerb::Unstage)),
    ("undo", ExVerb::Live(LiveVerb::Undo)),
    ("redo", ExVerb::Live(LiveVerb::Redo)),
    ("cancel", ExVerb::Live(LiveVerb::Cancel)),
    ("quantize", ExVerb::Live(LiveVerb::Quantize)),
    ("tolerance", ExVerb::Live(LiveVerb::Tolerance)),
    ("advance", ExVerb::Live(LiveVerb::Advance)),
    ("take", ExVerb::Live(LiveVerb::Take)),
    ("keep", ExVerb::Live(LiveVerb::Keep)),
    // Arrangement, transport, sections, lanes, placements
    ("stop", ExVerb::Arrange(ArrangeVerb::Stop)),
    ("restart", ExVerb::Arrange(ArrangeVerb::Restart)),
    ("cue", ExVerb::Arrange(ArrangeVerb::Cue { play: false })),
    ("play", ExVerb::Arrange(ArrangeVerb::Cue { play: true })),
    ("monitor", ExVerb::Arrange(ArrangeVerb::Monitor)),
    ("follow", ExVerb::Arrange(ArrangeVerb::Follow)),
    ("bpm", ExVerb::Arrange(ArrangeVerb::Bpm)),
    ("bars", ExVerb::Arrange(ArrangeVerb::Bars)),
    ("title", ExVerb::Arrange(ArrangeVerb::Title)),
    ("key", ExVerb::Arrange(ArrangeVerb::Key)),
    ("lane", ExVerb::Arrange(ArrangeVerb::Lane)),
    ("place", ExVerb::Arrange(ArrangeVerb::Place)),
    ("placement", ExVerb::Arrange(ArrangeVerb::Placement)),
    ("section", ExVerb::Arrange(ArrangeVerb::Section)),
    ("end", ExVerb::Arrange(ArrangeVerb::End)),
    ("loop", ExVerb::Arrange(ArrangeVerb::Loop)),
    ("arrange", ExVerb::Arrange(ArrangeVerb::Arrange)),
    ("inspect-event", ExVerb::Arrange(ArrangeVerb::InspectEvent)),
    // Material, patterns, parts, audition
    ("pattern", ExVerb::Material(MaterialVerb::Pattern)),
    ("mini", ExVerb::Material(MaterialVerb::Mini)),
    ("open", ExVerb::Material(MaterialVerb::Open)),
    ("part", ExVerb::Material(MaterialVerb::Part)),
    ("derive", ExVerb::Material(MaterialVerb::Derive)),
    ("fork", ExVerb::Material(MaterialVerb::Fork)),
    ("freeze", ExVerb::Material(MaterialVerb::Freeze)),
    ("len", ExVerb::Material(MaterialVerb::Len)),
    ("note", ExVerb::Material(MaterialVerb::Note)),
    ("value", ExVerb::Material(MaterialVerb::Value)),
    ("value-mode", ExVerb::Material(MaterialVerb::ValueMode)),
    ("local", ExVerb::Material(MaterialVerb::Local)),
    ("source", ExVerb::Material(MaterialVerb::Source)),
    ("inherit", ExVerb::Material(MaterialVerb::Inherit)),
    ("link", ExVerb::Material(MaterialVerb::Link)),
    ("unlink", ExVerb::Material(MaterialVerb::Unlink)),
    ("audition", ExVerb::Material(MaterialVerb::Audition)),
    // Instruments, presets, samples
    ("instrument", ExVerb::Instrument(InstrumentVerb::Instrument)),
    ("kit", ExVerb::Instrument(InstrumentVerb::Kit)),
    ("sampler", ExVerb::Instrument(InstrumentVerb::Sampler)),
    ("preset", ExVerb::Instrument(InstrumentVerb::Preset)),
    (
        "preset-save",
        ExVerb::Instrument(InstrumentVerb::PresetSave),
    ),
    (
        "preset-load",
        ExVerb::Instrument(InstrumentVerb::PresetLoad),
    ),
    ("tab", ExVerb::Instrument(InstrumentVerb::Tab)),
    (
        "reload-samples",
        ExVerb::Instrument(InstrumentVerb::ReloadSamples),
    ),
    (
        "reload-presets",
        ExVerb::Instrument(InstrumentVerb::ReloadPresets),
    ),
    ("samples", ExVerb::Instrument(InstrumentVerb::Samples)),
    ("browser", ExVerb::Instrument(InstrumentVerb::Samples)),
    ("sample", ExVerb::Instrument(InstrumentVerb::Sample)),
    // Mixer, routing, effects, modulation, macros, export
    ("mod", ExVerb::Mixer(MixerVerb::Mod)),
    ("macro", ExVerb::Mixer(MixerVerb::Macro)),
    ("macros", ExVerb::Mixer(MixerVerb::Macro)),
    ("duck", ExVerb::Mixer(MixerVerb::Duck)),
    ("param", ExVerb::Mixer(MixerVerb::Param)),
    ("channel", ExVerb::Mixer(MixerVerb::Channel)),
    ("fx", ExVerb::Mixer(MixerVerb::Fx)),
    ("route", ExVerb::Mixer(MixerVerb::Route)),
    (
        "route-instrument",
        ExVerb::Mixer(MixerVerb::RouteInstrument),
    ),
    ("send", ExVerb::Mixer(MixerVerb::Send)),
    ("clips", ExVerb::Mixer(MixerVerb::Clips)),
    ("rename", ExVerb::Mixer(MixerVerb::Rename)),
    ("delete", ExVerb::Mixer(MixerVerb::Delete)),
    ("mixer", ExVerb::Mixer(MixerVerb::Mixer)),
    ("wav", ExVerb::Mixer(MixerVerb::Wav)),
    ("bundle", ExVerb::Mixer(MixerVerb::Bundle)),
    ("stems", ExVerb::Mixer(MixerVerb::Stems)),
];

impl ExVerb {
    pub(crate) fn parse(name: &str) -> Option<Self> {
        EX_VERBS
            .iter()
            .find_map(|&(verb, parsed)| (verb == name).then_some(parsed))
    }

    /// Whether a bare `verb` with zero arguments should surface the Fuzzy Action Palette's
    /// top `▶ Run command:` passthrough entry (vs. requiring `verb <args...>` so bare queries
    /// like `"preset"`, `"bpm"`, or `"key"` fuzzy-match interactive GUI actions).
    pub(crate) const fn allows_zero_args(self) -> bool {
        match self {
            Self::Session(v) => match v {
                SessionVerb::New { .. }
                | SessionVerb::Write
                | SessionVerb::Quit { .. }
                | SessionVerb::WriteQuit
                | SessionVerb::Inspect
                | SessionVerb::Assets
                | SessionVerb::Deck
                | SessionVerb::Tips
                | SessionVerb::Tutorial
                | SessionVerb::Help
                | SessionVerb::Settings
                | SessionVerb::Files
                | SessionVerb::SaveAs
                | SessionVerb::Pin
                | SessionVerb::Unpin
                | SessionVerb::Bounce
                | SessionVerb::Record
                | SessionVerb::Audio
                | SessionVerb::Panic
                | SessionVerb::Midi => true,
                SessionVerb::Open { .. }
                | SessionVerb::ImportRbs { .. }
                | SessionVerb::Zone
                | SessionVerb::Patch
                | SessionVerb::Unpatch => false,
            },
            Self::Live(v) => match v {
                LiveVerb::Release
                | LiveVerb::Stage
                | LiveVerb::Unstage
                | LiveVerb::Undo
                | LiveVerb::Redo
                | LiveVerb::Cancel
                | LiveVerb::Quantize
                | LiveVerb::Keep => true,
                LiveVerb::Launch | LiveVerb::Tolerance | LiveVerb::Advance | LiveVerb::Take => {
                    false
                }
            },
            Self::Arrange(v) => match v {
                ArrangeVerb::Stop
                | ArrangeVerb::Restart
                | ArrangeVerb::Cue { .. }
                | ArrangeVerb::Monitor
                | ArrangeVerb::Follow
                | ArrangeVerb::Loop
                | ArrangeVerb::Arrange
                | ArrangeVerb::InspectEvent => true,
                ArrangeVerb::Bpm
                | ArrangeVerb::Bars
                | ArrangeVerb::Title
                | ArrangeVerb::Key
                | ArrangeVerb::Lane
                | ArrangeVerb::Place
                | ArrangeVerb::Placement
                | ArrangeVerb::Section
                | ArrangeVerb::End => false,
            },
            Self::Material(v) => match v {
                MaterialVerb::Local
                | MaterialVerb::Source
                | MaterialVerb::Inherit
                | MaterialVerb::Audition => true,
                MaterialVerb::Pattern
                | MaterialVerb::Mini
                | MaterialVerb::Open
                | MaterialVerb::Part
                | MaterialVerb::Derive
                | MaterialVerb::Fork
                | MaterialVerb::Freeze
                | MaterialVerb::Len
                | MaterialVerb::Note
                | MaterialVerb::Value
                | MaterialVerb::ValueMode
                | MaterialVerb::Link
                | MaterialVerb::Unlink => false,
            },
            Self::Instrument(v) => match v {
                InstrumentVerb::ReloadSamples
                | InstrumentVerb::ReloadPresets
                | InstrumentVerb::Samples => true,
                InstrumentVerb::Instrument
                | InstrumentVerb::Kit
                | InstrumentVerb::Sampler
                | InstrumentVerb::Preset
                | InstrumentVerb::PresetSave
                | InstrumentVerb::PresetLoad
                | InstrumentVerb::Tab
                | InstrumentVerb::Sample => false,
            },
            Self::Mixer(v) => match v {
                MixerVerb::Macro | MixerVerb::Mixer | MixerVerb::Wav | MixerVerb::Bundle => true,
                MixerVerb::Mod
                | MixerVerb::Duck
                | MixerVerb::Param
                | MixerVerb::Channel
                | MixerVerb::Fx
                | MixerVerb::Route
                | MixerVerb::RouteInstrument
                | MixerVerb::Send
                | MixerVerb::Clips
                | MixerVerb::Rename
                | MixerVerb::Delete
                | MixerVerb::Stems => false,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::is_recognized_command_line;
    use crate::App;
    use cadence_core::material::Project;
    use std::collections::HashSet;

    #[test]
    fn ex_verb_registry_has_no_duplicates_and_matches_palette_and_dispatcher() {
        let mut seen = HashSet::new();
        for &(name, verb) in EX_VERBS {
            assert!(
                seen.insert(name),
                "duplicate Ex verb '{name}' in EX_VERBS registry"
            );
            assert_eq!(ExVerb::parse(name), Some(verb));
            assert_eq!(
                is_recognized_command_line(name),
                verb.allows_zero_args(),
                "bare verb '{name}' palette recognition mismatch"
            );
            assert!(
                is_recognized_command_line(&format!("{name} arg")),
                "verb with argument '{name} arg' must be recognized by palette"
            );
        }

        // Verify every registered verb string is dispatched (never hits "Unknown command")
        // using hermetic MemoryHostIo so no OS files are touched.
        let io = std::sync::Arc::new(crate::MemoryHostIo::new("/workspace"));
        let mut app = App::new_with_host_io(
            Project::demo(),
            std::path::PathBuf::from("/workspace/registry-test.json"),
            io,
        )
        .unwrap();
        for &(name, _) in EX_VERBS {
            if let Err(err) = app.execute_command(&format!("{name} __probe_arg__")) {
                let msg = format!("{err:#}");
                assert!(
                    !msg.starts_with("Unknown command"),
                    "registered verb '{name}' fell through as unknown: {msg}"
                );
            }
        }

        assert_eq!(ExVerb::parse("not-a-real-command"), None);
        assert!(!is_recognized_command_line("not-a-real-command arg"));
        assert!(!is_recognized_command_line(""));
    }
}
