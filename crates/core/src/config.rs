//! Persisted user preferences (`config.json`), loaded on startup and edited live
//! via Cadence's Settings modal (`g,` or `:settings`).
use crate::follow::FollowMode;
use crate::midi_map::{MidiMapping, MidiPadMode};
use crate::runtime::{
    AudioInputMode, AudioInputRequest, AudioStreamRequest, InputMonitorMode,
    MAX_AUDIO_BUFFER_FRAMES, MAX_AUDIO_LATENCY_TRIM_MS, MAX_AUDIO_SAMPLE_RATE,
    MIN_AUDIO_BUFFER_FRAMES, MIN_AUDIO_LATENCY_TRIM_MS, MIN_AUDIO_SAMPLE_RATE,
};
use crate::store::{read_bounded, write_atomic};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const MAX_CONFIG_BYTES: u64 = 256 * 1024;

pub const DEFAULT_WINDOW_WIDTH: u32 = 1440;
pub const DEFAULT_WINDOW_HEIGHT: u32 = 900;
pub const MIN_WINDOW_WIDTH: u32 = 800;
pub const MIN_WINDOW_HEIGHT: u32 = 520;
pub const MAX_WINDOW_WIDTH: u32 = 7680;
pub const MAX_WINDOW_HEIGHT: u32 = 4320;
pub const MIN_DEFAULT_BPM: f64 = 20.0;
pub const MAX_DEFAULT_BPM: f64 = 300.0;
pub const MIN_DEFAULT_ROOT_MIDI: u8 = 24;
pub const MAX_DEFAULT_ROOT_MIDI: u8 = 84;

crate::define_choice_enum! {
    /// Semantic GPU theme identifier. Actual RGBA palettes live exclusively in `cadence-gui::theme`.
    #[derive(Default)]
    pub enum ThemeId {
        #[default]
        CadenceDark => ("cadence_dark", "Cadence Dark", ["dark"]),
        MidnightNeon => ("midnight_neon", "Midnight Neon", ["neon", "midnight"]),
        StudioLight => ("studio_light", "Studio Light", ["light"]),
    }
}

crate::define_choice_enum! {
    /// Visibility policy for the compact stereo meter bar beside `FINAL` in the top chrome bar.
    #[derive(Default)]
    pub enum TopBarMeterMode {
        #[default]
        Auto => ("auto", "Auto (Output hidden)", []),
        Always => ("always", "Always", ["on"]),
        Off => ("off", "Off", ["none"]),
    }
}

/// Global user preferences persisted between sessions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UserConfig {
    /// Active semantic GPU theme.
    pub theme: ThemeId,
    /// Startup deck layout preset (`1` Compose, `2` Perform, `3` Sound, `4` Mix).
    pub default_preset: u8,
    /// Default viewport follow behavior on startup.
    pub default_follow: FollowMode,
    /// Automatically arm live take recording when playback starts (`tr` toggles live).
    #[serde(default)]
    pub auto_record_takes: bool,
    /// Show the Final Output oscilloscope/meter strip by default.
    pub show_output_monitor: bool,
    /// Show the compact DSP CPU load pill in the top chrome bar alongside `FINAL`.
    #[serde(default = "default_true")]
    pub show_top_bar_cpu: bool,
    /// Visibility policy for the compact stereo meter bar beside `FINAL` (`Auto`, `Always`, `Off`).
    #[serde(default)]
    pub top_bar_meter: TopBarMeterMode,
    /// Show compact vertical stereo level meter pills inside Arrange lane gutters.
    #[serde(default)]
    pub show_arrange_lane_meters: bool,
    /// Show contextual keyboard hints in the footer chrome bar.
    #[serde(default = "default_true")]
    pub show_key_hints: bool,
    /// Default tempo for newly created (`--empty` / `:new`) projects (`20.0..=300.0`).
    pub default_bpm: f64,
    /// Default musical key root MIDI note (`24..=84`, e.g. `62` = D4, `53` = F3).
    pub default_root_midi: u8,
    /// Default musical scale ID for newly created projects (`cadence_core::music::NAMED_SCALES`).
    #[serde(default = "default_scale")]
    pub default_scale: String,
    /// Preferred audio output device name (`None` = System Default, follows OS switches).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_output_device: Option<String>,
    /// Preferred audio input device name (`None` = inactive; no OS mic stream opened).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_input_device: Option<String>,
    /// Channel routing applied to hardware audio input frames (`Stereo`, `MonoLeft`, `MonoRight`).
    #[serde(default)]
    pub audio_input_mode: AudioInputMode,
    /// Software live input monitoring policy (`Off`, `Auto`, `On`).
    #[serde(default)]
    pub audio_input_monitor: InputMonitorMode,
    /// Manual audio recording latency trim in milliseconds (`-250.0..=250.0`) applied on top of
    /// hardware input + output buffer compensation.
    #[serde(default)]
    pub audio_input_latency_ms: f32,
    /// Preferred dedicated cue output device name (`None` = Off; reserved for future cue bus).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_cue_device: Option<String>,
    /// Preferred audio output sample rate in Hz (`None` = Device Default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_sample_rate: Option<u32>,
    /// Preferred audio callback buffer size in frames (`None` = Auto safe default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_buffer_size: Option<u32>,
    /// Preferred MIDI input port name substring (auto-connected on startup if present).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub midi_in_port: Option<String>,
    /// Preferred MIDI output port name substring for LED/OLED feedback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub midi_out_port: Option<String>,
    /// Active built-in controller profile ID (`"generic"`, `"standard_cc"`, `"minilab3_daw"`).
    #[serde(default = "default_midi_profile")]
    pub midi_profile: String,
    /// Optional path to a custom `.midi.json` controller profile (overrides `midi_profile` when set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub midi_profile_path: Option<PathBuf>,
    /// Enable soft-pickup takeover for absolute MIDI knobs/faders.
    #[serde(default = "default_true")]
    pub midi_pickup: bool,
    /// Default performance pad behavior (`notes`, `sections`, `mutes`).
    #[serde(default)]
    pub midi_pad_mode: MidiPadMode,
    /// User-learned MIDI controller mappings persisted globally across sessions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub midi_mappings: Vec<MidiMapping>,
    /// Most-recently-used project & `.rbs` paths for the File Hub (`gf`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_projects: Vec<PathBuf>,
    /// Additional user-configured sample search directories (scanned alongside defaults).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sample_dirs: Vec<String>,
    /// Additional user-configured external preset search directories (scanned alongside defaults).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preset_dirs: Vec<String>,
    /// Persisted non-maximized logical window width (`800..=7680`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_width: Option<u32>,
    /// Persisted non-maximized logical window height (`520..=4320`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_height: Option<u32>,
    /// Whether the GUI window was maximized when last closed.
    #[serde(default)]
    pub window_maximized: bool,
    /// Whether the GUI window was in native borderless fullscreen when last closed.
    #[serde(default)]
    pub window_fullscreen: bool,
}

fn default_midi_profile() -> String {
    "generic".into()
}

fn default_scale() -> String {
    "minor".into()
}

fn default_true() -> bool {
    true
}

impl Default for UserConfig {
    fn default() -> Self {
        Self {
            theme: ThemeId::CadenceDark,
            default_preset: 1,
            default_follow: FollowMode::Off,
            auto_record_takes: false,
            show_output_monitor: true,
            show_top_bar_cpu: true,
            top_bar_meter: TopBarMeterMode::Auto,
            show_arrange_lane_meters: false,
            show_key_hints: true,
            default_bpm: 124.0,
            default_root_midi: 62,
            default_scale: default_scale(),
            audio_output_device: None,
            audio_input_device: None,
            audio_input_mode: AudioInputMode::Stereo,
            audio_input_monitor: InputMonitorMode::Off,
            audio_input_latency_ms: 0.0,
            audio_cue_device: None,
            audio_sample_rate: None,
            audio_buffer_size: None,
            midi_in_port: None,
            midi_out_port: None,
            midi_profile: default_midi_profile(),
            midi_profile_path: None,
            midi_pickup: true,
            midi_pad_mode: MidiPadMode::Notes,
            midi_mappings: Vec::new(),
            recent_projects: Vec::new(),
            sample_dirs: Vec::new(),
            preset_dirs: Vec::new(),
            window_width: None,
            window_height: None,
            window_maximized: false,
            window_fullscreen: false,
        }
    }
}

/// Result of loading persisted [`UserConfig`] with non-fatal diagnostics.
#[derive(Clone, Debug, PartialEq)]
pub struct UserConfigLoad {
    pub config: UserConfig,
    /// Human-readable warning when an existing config file could not be read, parsed, or validated.
    pub warning: Option<String>,
    /// True when a file exists at the config path but failed to load.
    /// Hosts must skip automatic saves so the user's file is never silently overwritten.
    pub corrupt_on_disk: bool,
}

impl UserConfig {
    /// Clamped logical window dimensions for restoring the non-maximized GUI frame.
    pub fn window_dimensions(&self) -> (u32, u32) {
        (
            self.window_width
                .unwrap_or(DEFAULT_WINDOW_WIDTH)
                .clamp(MIN_WINDOW_WIDTH, MAX_WINDOW_WIDTH),
            self.window_height
                .unwrap_or(DEFAULT_WINDOW_HEIGHT)
                .clamp(MIN_WINDOW_HEIGHT, MAX_WINDOW_HEIGHT),
        )
    }

    pub fn record_recent_project_canonical(&mut self, canonical: PathBuf) {
        self.recent_projects.retain(|p| p != &canonical);
        self.recent_projects.insert(0, canonical);
        self.recent_projects.truncate(12);
    }

    pub fn record_recent_project(&mut self, path: &Path) {
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        self.record_recent_project_canonical(canonical);
    }
    /// Build an [`AudioStreamRequest`] from the persisted audio device preferences.
    pub fn audio_stream_request(&self) -> AudioStreamRequest {
        AudioStreamRequest {
            output_device: self.audio_output_device.clone(),
            sample_rate: self.audio_sample_rate,
            buffer_frames: self.audio_buffer_size,
        }
    }

    /// Build an [`AudioInputRequest`] from the persisted audio input & monitoring preferences.
    pub fn audio_input_request(&self) -> AudioInputRequest {
        AudioInputRequest {
            input_device: self.audio_input_device.clone(),
            mode: self.audio_input_mode,
            monitor: self.audio_input_monitor,
        }
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            (1..=4).contains(&self.default_preset),
            "default_preset must be 1..=4"
        );
        ensure!(
            self.default_bpm.is_finite()
                && (MIN_DEFAULT_BPM..=MAX_DEFAULT_BPM).contains(&self.default_bpm),
            "default_bpm must be {MIN_DEFAULT_BPM}..={MAX_DEFAULT_BPM}"
        );
        ensure!(
            (MIN_DEFAULT_ROOT_MIDI..=MAX_DEFAULT_ROOT_MIDI).contains(&self.default_root_midi),
            "default_root_midi must be {MIN_DEFAULT_ROOT_MIDI}..={MAX_DEFAULT_ROOT_MIDI}"
        );
        ensure!(
            crate::music::named_scale(&self.default_scale).is_some(),
            "default_scale '{}' is not a known scale",
            self.default_scale
        );
        if let Some(sr) = self.audio_sample_rate {
            ensure!(
                (MIN_AUDIO_SAMPLE_RATE..=MAX_AUDIO_SAMPLE_RATE).contains(&sr),
                "audio_sample_rate must be {MIN_AUDIO_SAMPLE_RATE}..={MAX_AUDIO_SAMPLE_RATE}"
            );
        }
        if let Some(buf) = self.audio_buffer_size {
            ensure!(
                (MIN_AUDIO_BUFFER_FRAMES..=MAX_AUDIO_BUFFER_FRAMES).contains(&buf),
                "audio_buffer_size must be {MIN_AUDIO_BUFFER_FRAMES}..={MAX_AUDIO_BUFFER_FRAMES}"
            );
        }
        ensure!(
            self.audio_input_latency_ms.is_finite()
                && (MIN_AUDIO_LATENCY_TRIM_MS..=MAX_AUDIO_LATENCY_TRIM_MS)
                    .contains(&self.audio_input_latency_ms),
            "audio_input_latency_ms must be {MIN_AUDIO_LATENCY_TRIM_MS}..={MAX_AUDIO_LATENCY_TRIM_MS}"
        );
        if let Some(w) = self.window_width {
            ensure!(
                (320..=16384).contains(&w),
                "window_width must be 320..=16384"
            );
        }
        if let Some(h) = self.window_height {
            ensure!(
                (240..=16384).contains(&h),
                "window_height must be 240..=16384"
            );
        }
        Ok(())
    }

    /// Resolve the standard config path, honoring `CADENCE_CONFIG_PATH` for tests and custom setups.
    pub fn default_path() -> PathBuf {
        if let Ok(override_path) = std::env::var("CADENCE_CONFIG_PATH") {
            if !override_path.trim().is_empty() {
                return PathBuf::from(override_path);
            }
        }
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            if !xdg.trim().is_empty() {
                return PathBuf::from(xdg).join("cadence/config.json");
            }
        }
        if let Ok(home) = std::env::var("HOME") {
            let home = PathBuf::from(home);
            #[cfg(target_os = "macos")]
            {
                return home.join("Library/Application Support/cadence/config.json");
            }
            #[cfg(not(target_os = "macos"))]
            {
                return home.join(".config/cadence/config.json");
            }
        }
        PathBuf::from("cadence.config.json")
    }

    /// Parent directory of [`Self::default_path()`] (e.g. `~/.config/cadence` or
    /// `~/Library/Application Support/cadence`), used for user `presets/` discovery.
    pub fn default_config_dir() -> Option<PathBuf> {
        let path = Self::default_path();
        path.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        let bytes = read_bounded(path, MAX_CONFIG_BYTES)
            .with_context(|| format!("Reading config {}", path.display()))?;
        let cfg: Self = serde_json::from_slice(&bytes)
            .with_context(|| format!("Parsing config {}", path.display()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Load user config from `path`, falling back to `Self::default()` with a diagnostic
    /// warning when an existing file cannot be read, parsed, or validated (leaving disk untouched).
    pub fn load_from_diagnostic(path: &Path) -> UserConfigLoad {
        match std::fs::symlink_metadata(path) {
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => UserConfigLoad {
                config: Self::default(),
                warning: None,
                corrupt_on_disk: false,
            },
            _ => match Self::load_from(path) {
                Ok(config) => UserConfigLoad {
                    config,
                    warning: None,
                    corrupt_on_disk: false,
                },
                Err(err) => UserConfigLoad {
                    config: Self::default(),
                    warning: Some(format!(
                        "Config {} could not be loaded ({err:#}); using defaults without overwriting file",
                        path.display()
                    )),
                    corrupt_on_disk: true,
                },
            },
        }
    }

    /// Load user config from `default_path()`, returning defaults plus diagnostics whenever
    /// an existing `config.json` fails to load so hosts never silently overwrite it.
    pub fn load_default_diagnostic() -> UserConfigLoad {
        Self::load_from_diagnostic(&Self::default_path())
    }

    /// Load user config from `default_path()`, falling back cleanly to `Self::default()` if absent.
    pub fn load_default() -> Self {
        Self::load_default_diagnostic().config
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        self.validate()?;
        let mut bytes = serde_json::to_vec_pretty(self).context("Serializing UserConfig")?;
        bytes.push(b'\n');
        write_atomic(path, MAX_CONFIG_BYTES, |buf| {
            buf.extend_from_slice(&bytes);
            Ok(())
        })?;
        Ok(())
    }

    pub fn save_default(&self) -> Result<PathBuf> {
        let path = Self::default_path();
        self.save_to(&path)?;
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::midi_map::{MidiSource, MidiTarget, MidiValueMode};
    use crate::mixer::{ChannelParam, ChannelRef};
    use crate::params::ParamAddress;

    #[test]
    fn user_config_roundtrips_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/config.json");
        let mut cfg = UserConfig {
            theme: ThemeId::MidnightNeon,
            default_preset: 2,
            default_follow: FollowMode::Active,
            auto_record_takes: true,
            show_output_monitor: false,
            show_top_bar_cpu: false,
            top_bar_meter: TopBarMeterMode::Always,
            show_arrange_lane_meters: true,
            show_key_hints: false,
            default_bpm: 128.0,
            default_root_midi: 53,
            default_scale: "dorian".into(),
            audio_output_device: Some("Studio DAC".into()),
            audio_input_device: Some("USB Preamp".into()),
            audio_input_mode: AudioInputMode::MonoLeft,
            audio_input_monitor: InputMonitorMode::Auto,
            audio_input_latency_ms: -4.5,
            audio_cue_device: Some("Headphones".into()),
            audio_sample_rate: Some(48_000),
            audio_buffer_size: Some(512),
            midi_in_port: Some("Minilab3 MIDI".into()),
            midi_out_port: Some("Minilab3 MIDI".into()),
            midi_profile: "minilab3_daw".into(),
            midi_profile_path: Some(PathBuf::from("/Users/studio/rig.midi.json")),
            midi_pickup: false,
            midi_pad_mode: MidiPadMode::Sections,
            midi_mappings: vec![MidiMapping::new(
                1,
                MidiSource::Cc {
                    channel: Some(0),
                    cc: 74,
                },
                MidiValueMode::Absolute,
                MidiTarget::Param {
                    address: ParamAddress::Channel {
                        channel: ChannelRef::Master,
                        param: ChannelParam::Gain,
                    },
                },
            )],
            recent_projects: Vec::new(),
            sample_dirs: vec!["~/Samples".into()],
            preset_dirs: vec!["~/Presets".into()],
            window_width: Some(1728),
            window_height: Some(1024),
            window_maximized: true,
            window_fullscreen: false,
        };
        cfg.record_recent_project(Path::new("examples/afterglow.json"));
        cfg.save_to(&path).unwrap();
        let loaded = UserConfig::load_from(&path).unwrap();
        assert_eq!(loaded, cfg);
        assert_eq!(loaded.window_dimensions(), (1728, 1024));

        // Omitting optional fields from JSON defaults cleanly.
        let minimal: UserConfig = serde_json::from_str("{}").unwrap();
        assert!(!minimal.auto_record_takes);
        assert_eq!(minimal.window_width, None);
        assert_eq!(minimal.window_height, None);
        assert!(!minimal.window_maximized);
        assert!(!minimal.window_fullscreen);
        assert_eq!(
            minimal.window_dimensions(),
            (DEFAULT_WINDOW_WIDTH, DEFAULT_WINDOW_HEIGHT)
        );

        // Oversized config file is rejected before unbounded allocation.
        let huge = dir.path().join("oversized.json");
        std::fs::write(&huge, vec![b' '; (MAX_CONFIG_BYTES + 1) as usize]).unwrap();
        assert!(UserConfig::load_from(&huge).is_err());
    }

    #[test]
    fn diagnostic_config_loader_preserves_corrupt_file_and_reports_warning() {
        let dir = tempfile::tempdir().unwrap();

        // Missing file returns clean defaults with no warning and corrupt_on_disk == false.
        let missing = dir.path().join("absent.json");
        let missing_load = UserConfig::load_from_diagnostic(&missing);
        assert_eq!(missing_load.config, UserConfig::default());
        assert!(missing_load.warning.is_none());
        assert!(!missing_load.corrupt_on_disk);

        // Corrupt JSON returns defaults, surfaces a diagnostic warning, sets corrupt_on_disk,
        // and leaves the user's file untouched on disk.
        let corrupt = dir.path().join("corrupt.json");
        let raw_corrupt = b"{ \"default_bpm\": 128, broken_json }";
        std::fs::write(&corrupt, raw_corrupt).unwrap();
        let corrupt_load = UserConfig::load_from_diagnostic(&corrupt);
        assert_eq!(corrupt_load.config, UserConfig::default());
        assert!(corrupt_load.corrupt_on_disk);
        let warning = corrupt_load
            .warning
            .as_deref()
            .expect("corrupt config must produce warning");
        assert!(warning.contains("could not be loaded"));
        assert!(warning.contains("without overwriting"));
        assert_eq!(
            std::fs::read(&corrupt).unwrap(),
            raw_corrupt,
            "corrupt config file on disk must remain untouched"
        );

        // Semantically invalid config (out-of-range BPM) also marks corrupt_on_disk.
        let invalid = dir.path().join("invalid.json");
        let raw_invalid = b"{\"default_bpm\": 9999.0}\n";
        std::fs::write(&invalid, raw_invalid).unwrap();
        let invalid_load = UserConfig::load_from_diagnostic(&invalid);
        assert_eq!(invalid_load.config, UserConfig::default());
        assert!(invalid_load.corrupt_on_disk);
        assert!(invalid_load.warning.is_some());
        assert_eq!(std::fs::read(&invalid).unwrap(), raw_invalid);

        // Valid config loads cleanly with no warning.
        let valid = dir.path().join("valid.json");
        let custom = UserConfig {
            default_bpm: 135.0,
            ..UserConfig::default()
        };
        custom.save_to(&valid).unwrap();
        let valid_load = UserConfig::load_from_diagnostic(&valid);
        assert_eq!(valid_load.config, custom);
        assert!(valid_load.warning.is_none());
        assert!(!valid_load.corrupt_on_disk);
    }
}
