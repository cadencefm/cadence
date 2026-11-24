//! Unified Devices Deck (`gd` / `Ctrl+d` — `Audio | MIDI` tabs), hardware audio interface
//! configuration, recovery actions, edge-triggered audio health alerts, and `:audio` / `:panic`.
use crate::*;
use cadence_core::runtime::{
    default_auto_buffer_frames, AudioDeviceInfo, AudioHealth, AudioInputMode, AudioInventory,
    InputMonitorMode, MAX_AUDIO_LATENCY_TRIM_MS, MIN_AUDIO_LATENCY_TRIM_MS, STANDARD_BUFFER_SIZES,
    STANDARD_SAMPLE_RATES,
};

cadence_core::define_choice_enum! {
    /// Active tab of the unified Devices Deck (`H`/`L` switches `Audio ↔ MIDI`).
    #[derive(Default)]
    pub enum DevicesTab [array] {
        Audio => ("audio", "Audio", ["interface", "engine"]),
        #[default]
        Midi => ("midi", "MIDI", ["controller", "cc"]),
    }
}

/// Typed vertical rows on the `Devices · Audio` tab (zero raw `usize` row indices).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioDeckRow {
    Output,
    SampleRate,
    BufferSize,
    CueOut,
    Input,
    InputMode,
    InputMonitor,
    LatencyTrim,
    AudioPanic,
    RestartAudio,
    RescanDevices,
}

impl AudioDeckRow {
    pub const ALL: [Self; 11] = [
        Self::Output,
        Self::SampleRate,
        Self::BufferSize,
        Self::CueOut,
        Self::Input,
        Self::InputMode,
        Self::InputMonitor,
        Self::LatencyTrim,
        Self::AudioPanic,
        Self::RestartAudio,
        Self::RescanDevices,
    ];

    pub const GROUPS: [&'static str; 11] = [
        "AUDIO INTERFACE",
        "AUDIO INTERFACE",
        "AUDIO INTERFACE",
        "AUDIO INTERFACE",
        "AUDIO INPUT & RECORDING",
        "AUDIO INPUT & RECORDING",
        "AUDIO INPUT & RECORDING",
        "AUDIO INPUT & RECORDING",
        "ENGINE & RECOVERY",
        "ENGINE & RECOVERY",
        "ENGINE & RECOVERY",
    ];

    pub fn from_index(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }

    pub fn group(self) -> &'static str {
        match self {
            Self::Output | Self::SampleRate | Self::BufferSize | Self::CueOut => "AUDIO INTERFACE",
            Self::Input | Self::InputMode | Self::InputMonitor | Self::LatencyTrim => {
                "AUDIO INPUT & RECORDING"
            }
            Self::AudioPanic | Self::RestartAudio | Self::RescanDevices => "ENGINE & RECOVERY",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Output => "Output",
            Self::SampleRate => "Sample Rate",
            Self::BufferSize => "Buffer Size",
            Self::CueOut => "Cue Out",
            Self::Input => "Input",
            Self::InputMode => "Input Mode",
            Self::InputMonitor => "Input Monitor",
            Self::LatencyTrim => "Latency Trim",
            Self::AudioPanic => "Audio Panic",
            Self::RestartAudio => "Restart Audio",
            Self::RescanDevices => "Rescan Devices",
        }
    }

    pub fn is_action(self) -> bool {
        matches!(
            self,
            Self::AudioPanic | Self::RestartAudio | Self::RescanDevices
        )
    }
}

/// Pure session state for the unified `Devices` deck (`Audio | MIDI` tabs),
/// live OS audio inventory, MIDI output error diagnostics, and edge-triggered alert latches.
#[derive(Clone, Debug, Default)]
pub struct AudioDevicesState {
    pub tab: DevicesTab,
    pub inventory: AudioInventory,
    pub cursor: usize,
    pub midi_out_error: Option<String>,
    pub(crate) last_overloaded: bool,
    pub(crate) last_failed: bool,
}

/// Format an audio sample rate in Hz cleanly (e.g. `44100` -> `"44.1 kHz"`, `48000` -> `"48 kHz"`).
pub fn format_sample_rate(sample_rate: u32) -> String {
    if sample_rate == 0 {
        return "—".into();
    }
    if sample_rate.is_multiple_of(1_000) {
        format!("{} kHz", sample_rate / 1_000)
    } else {
        format!("{:.1} kHz", sample_rate as f32 / 1_000.0)
    }
}

/// Format a callback buffer size with exact one-way latency in milliseconds (e.g. `"512 fr · 10.7 ms"`).
pub fn format_buffer_latency(frames: u32, sample_rate: u32) -> String {
    if frames == 0 {
        return "—".into();
    }
    if sample_rate == 0 {
        return format!("{frames} fr");
    }
    let ms = frames as f32 * 1000.0 / sample_rate as f32;
    format!("{frames} fr · {ms:.1} ms")
}

/// Canonical label for the automatic buffer size option (`None` in `UserConfig`).
pub(crate) const fn auto_buffer_label() -> &'static str {
    "Auto (SR-scaled)"
}

/// Cycle `[None, Some(items[0]), Some(items[1]), ...]` cyclically by `dir`.
pub(crate) fn cycle_optional<T: PartialEq + Clone>(
    items: &[T],
    current: Option<&T>,
    dir: isize,
) -> Option<T> {
    if items.is_empty() {
        return None;
    }
    let total = items.len() + 1;
    let cur_idx = match current {
        None => 0,
        Some(val) => items.iter().position(|x| x == val).map_or(0, |i| i + 1),
    };
    let next_idx = (cur_idx as isize + dir).rem_euclid(total as isize) as usize;
    if next_idx == 0 {
        None
    } else {
        items.get(next_idx - 1).cloned()
    }
}

impl App {
    /// Update the discovered OS audio hardware inventory and active stream metadata from the host.
    pub fn set_audio_inventory(&mut self, inv: AudioInventory) {
        self.devices.inventory = inv;
    }

    /// Update or clear the active MIDI output error diagnostic surfaced on the `Devices · MIDI` tab.
    pub fn set_midi_out_error(&mut self, err: Option<String>) {
        self.devices.midi_out_error = err;
    }

    /// Returns true if `health` has an unobserved overload/failure state transition.
    pub fn audio_health_alert_changed(&self, health: &AudioHealth) -> bool {
        health.overloaded != self.devices.last_overloaded
            || health.failed != self.devices.last_failed
    }

    /// Observe live [`AudioHealth`] telemetry and surface edge-triggered overload / stream error alerts.
    pub fn observe_audio_health(&mut self, health: &AudioHealth) {
        if health.overloaded && !self.devices.last_overloaded {
            self.status = if cfg!(debug_assertions) {
                "Audio overload — paused safely · raise Buffer Size in Devices (Ctrl+d) or run --release"
                    .into()
            } else {
                "Audio overload — paused safely · raise Buffer Size in Devices (Ctrl+d)".into()
            };
        }
        if health.failed && !self.devices.last_failed {
            self.status =
                "Audio stream error — press Ctrl+d (Audio) to restart or switch output device"
                    .into();
        }
        self.devices.last_overloaded = health.overloaded;
        self.devices.last_failed = health.failed;
    }

    /// Resolve the active output [`AudioDeviceInfo`] matching user selection, active OS device, or default.
    pub(crate) fn active_output_device_info(&self) -> Option<&AudioDeviceInfo> {
        let outputs = &self.devices.inventory.outputs;
        if let Some(pref) = self.config.audio_output_device.as_deref() {
            if let Some(dev) = outputs.iter().find(|d| d.name == pref) {
                return Some(dev);
            }
        }
        if let Some(active) = self.devices.inventory.active_output.as_deref() {
            if let Some(dev) = outputs.iter().find(|d| d.name == active) {
                return Some(dev);
            }
        }
        outputs
            .iter()
            .find(|d| d.is_default)
            .or_else(|| outputs.first())
    }

    /// Effective sample rate in Hz used for computing buffer latency estimates in UI & pickers.
    pub(crate) fn effective_audio_sample_rate(&self, snapshot_sample_rate: u32) -> u32 {
        if let Some(sr) = self.config.audio_sample_rate.filter(|&r| r > 0) {
            return sr;
        }
        if snapshot_sample_rate > 0 {
            return snapshot_sample_rate;
        }
        if self.devices.inventory.active_sample_rate > 0 {
            return self.devices.inventory.active_sample_rate;
        }
        if let Some(dev) = self.active_output_device_info() {
            if dev.default_sample_rate > 0 {
                return dev.default_sample_rate;
            }
        }
        48_000
    }

    /// Sorted list of discrete sample rates offered on the `Sample Rate` row & picker.
    pub(crate) fn available_sample_rates(&self) -> Vec<u32> {
        let mut rates: Vec<u32> = self
            .active_output_device_info()
            .filter(|d| !d.supported_sample_rates.is_empty())
            .map(|d| d.supported_sample_rates.clone())
            .unwrap_or_else(|| STANDARD_SAMPLE_RATES.to_vec());
        if let Some(cur) = self.config.audio_sample_rate {
            if !rates.contains(&cur) {
                rates.push(cur);
            }
        }
        rates.sort_unstable();
        rates.dedup();
        rates
    }

    /// Sorted list of discrete buffer sizes offered on the `Buffer Size` row & picker,
    /// filtered to the active output device's `[min_buffer_frames, max_buffer_frames]` when known.
    pub(crate) fn available_buffer_sizes(&self) -> Vec<u32> {
        let (min_b, max_b) = self
            .active_output_device_info()
            .map_or((None, None), |d| (d.min_buffer_frames, d.max_buffer_frames));
        let mut sizes: Vec<u32> = STANDARD_BUFFER_SIZES
            .iter()
            .copied()
            .filter(|&sz| min_b.is_none_or(|lo| sz >= lo) && max_b.is_none_or(|hi| sz <= hi))
            .collect();
        if sizes.is_empty() {
            sizes = STANDARD_BUFFER_SIZES.to_vec();
        }
        if let Some(cur) = self.config.audio_buffer_size {
            if !sizes.contains(&cur) {
                sizes.push(cur);
            }
        }
        sizes.sort_unstable();
        sizes.dedup();
        sizes
    }

    /// Open the unified Devices deck directly on `tab`.
    pub(crate) fn open_devices_deck(&mut self, tab: DevicesTab) {
        self.devices.tab = tab;
        self.go_to_deck(layout::Deck::song(layout::DeckKind::Midi));
    }

    pub(crate) fn step_devices_tab(&mut self, delta: i32) {
        self.devices.tab = self.devices.tab.cycle(delta);
        self.status = match self.devices.tab {
            DevicesTab::Audio => {
                "Devices · Audio · H/L tab · j/k row · w/b group · Alt+h/l ,/. cycle · Enter pick/run · r rescan"
                    .into()
            }
            DevicesTab::Midi => {
                "Devices · MIDI · H/L tab · j/k row · w/b group · Alt+h/l ,/. cycle · Enter pick/learn · a add · r rescan"
                    .into()
            }
        };
    }

    pub(crate) fn set_audio_output_choice(&mut self, next: Option<String>) -> Outcome {
        self.config.audio_output_device = next.clone();
        self.settings_ui.config_dirty = true;
        self.maybe_persist_config();
        self.devices.last_failed = false;
        self.status = match &next {
            Some(name) => format!("Audio Output → {name}"),
            None => "Audio Output → System Default".into(),
        };
        Outcome {
            audio_reconfigure: Some(self.config.audio_stream_request()),
            ..Outcome::default()
        }
    }

    pub(crate) fn set_audio_input_choice(&mut self, next: Option<String>) -> Outcome {
        self.config.audio_input_device = next.clone();
        self.settings_ui.config_dirty = true;
        self.maybe_persist_config();
        self.recording.last_input_failed = false;
        self.status = match &next {
            Some(name) => format!("Audio Input → {name} · t a records · t i monitor"),
            None => "Audio Input → None (input stream closed)".into(),
        };
        Outcome {
            audio_input_reconfigure: Some(self.config.audio_input_request()),
            ..Outcome::default()
        }
    }

    pub(crate) fn set_audio_input_mode_choice(&mut self, next: AudioInputMode) -> Outcome {
        self.config.audio_input_mode = next;
        self.settings_ui.config_dirty = true;
        self.maybe_persist_config();
        self.status = format!("Audio Input Mode → {}", next.label());
        Outcome {
            audio_input_reconfigure: Some(self.config.audio_input_request()),
            ..Outcome::default()
        }
    }

    pub(crate) fn set_audio_input_monitor_choice(&mut self, next: InputMonitorMode) -> Outcome {
        self.config.audio_input_monitor = next;
        self.settings_ui.config_dirty = true;
        self.maybe_persist_config();
        self.status = format!("Input Monitor → {}", next.label());
        Outcome {
            audio_input_reconfigure: Some(self.config.audio_input_request()),
            ..Outcome::default()
        }
    }

    pub(crate) fn set_audio_latency_trim_ms(&mut self, next_ms: f32) -> Outcome {
        let clamped = if next_ms.is_finite() {
            next_ms.clamp(MIN_AUDIO_LATENCY_TRIM_MS, MAX_AUDIO_LATENCY_TRIM_MS)
        } else {
            0.0
        };
        // Round to 0.5 ms cleanly to avoid float drift on repeated nudges.
        let rounded = (clamped * 2.0).round() * 0.5;
        self.config.audio_input_latency_ms = if rounded.abs() < 1e-4 { 0.0 } else { rounded };
        self.settings_ui.config_dirty = true;
        self.maybe_persist_config();
        self.status = format!(
            "Recording Latency Trim → {:+.1} ms",
            self.config.audio_input_latency_ms
        );
        Outcome::default()
    }

    pub(crate) fn set_audio_cue_choice(&mut self, next: Option<String>) -> Outcome {
        self.config.audio_cue_device = next.clone();
        self.settings_ui.config_dirty = true;
        self.maybe_persist_config();
        self.status = match &next {
            Some(name) => format!("Cue Out → {name} (reserved · future cue bus)"),
            None => "Cue Out → Off".into(),
        };
        Outcome::default()
    }

    pub(crate) fn set_audio_sample_rate_choice(&mut self, next: Option<u32>) -> Outcome {
        self.config.audio_sample_rate = next;
        self.settings_ui.config_dirty = true;
        self.maybe_persist_config();
        self.status = match next {
            Some(sr) => format!("Audio Sample Rate → {}", format_sample_rate(sr)),
            None => "Audio Sample Rate → Auto (Device)".into(),
        };
        Outcome {
            audio_reconfigure: Some(self.config.audio_stream_request()),
            ..Outcome::default()
        }
    }

    pub(crate) fn set_audio_buffer_size_choice(&mut self, next: Option<u32>) -> Outcome {
        self.config.audio_buffer_size = next;
        self.settings_ui.config_dirty = true;
        self.maybe_persist_config();
        let sr = self.effective_audio_sample_rate(0);
        self.status = match next {
            Some(frames) => {
                format!("Audio Buffer Size → {}", format_buffer_latency(frames, sr))
            }
            None => format!(
                "Audio Buffer Size → Auto ({})",
                format_buffer_latency(default_auto_buffer_frames(sr), sr)
            ),
        };
        Outcome {
            audio_reconfigure: Some(self.config.audio_stream_request()),
            ..Outcome::default()
        }
    }

    /// Immediately silence all ringing voices & FX tails at the current step and clear overload latches.
    pub(crate) fn apply_audio_panic(&mut self) -> Result<Outcome> {
        let mut out = self.release_held();
        for (midi, instrument_id) in self.input.midi_held.drain(..) {
            out.live_notes_off.push(InstrumentNoteOff {
                instrument_id,
                midi,
            });
        }
        out.stop_audition = true;
        self.stop_pending_browser_audition();
        if self.is_solo_auditioning() {
            let stopped = self.stop_audition()?;
            out.live_notes_off.extend(stopped.live_notes_off);
            out.changed |= stopped.changed;
        } else {
            self.live.audition = None;
        }
        let step = self.transport.step;
        self.transport.playing = false;
        self.live.playing = false;
        self.devices.last_overloaded = false;
        if self.recording.is_active() {
            self.recording.phase = cadence_core::runtime::CapturePhase::Idle;
            self.recording.active_request = None;
            out.recording_command = Some(RecordingHostCommand::Cancel);
        }
        out.transport.push(TransportIntent::Reset { step });
        self.status =
            "Audio Panic · silenced all voices & FX tails · overload latch cleared".into();
        Ok(out)
    }

    pub(crate) fn apply_restart_audio(&mut self) -> Result<Outcome> {
        self.devices.last_overloaded = false;
        self.devices.last_failed = false;
        self.status = "Restarting audio engine stream…".into();
        Ok(Outcome {
            audio_restart: true,
            ..Outcome::default()
        })
    }

    pub(crate) fn apply_rescan_audio(&mut self) -> Result<Outcome> {
        self.status = "Rescanning OS audio & MIDI devices…".into();
        Ok(Outcome {
            audio_rescan: true,
            midi_rescan: true,
            ..Outcome::default()
        })
    }

    /// Tier 3 (`Alt+h`/`l`) & Tier 4 (`,`/`.`) value cycling for the focused `Devices · Audio` row.
    pub(crate) fn adjust_audio_deck_row(&mut self, dir: isize) -> Result<Outcome> {
        let total = AudioDeckRow::ALL.len();
        self.devices.cursor = self.devices.cursor.min(total - 1);
        let Some(row) = AudioDeckRow::from_index(self.devices.cursor) else {
            return Ok(Outcome::default());
        };
        match row {
            AudioDeckRow::Output => {
                let names: Vec<String> = self
                    .devices
                    .inventory
                    .outputs
                    .iter()
                    .map(|d| d.name.clone())
                    .collect();
                if names.is_empty() && self.config.audio_output_device.is_none() {
                    self.status = "No audio outputs discovered · press r to rescan".into();
                    return Ok(Outcome::default());
                }
                let next = cycle_optional(&names, self.config.audio_output_device.as_ref(), dir);
                Ok(self.set_audio_output_choice(next))
            }
            AudioDeckRow::Input => {
                let names: Vec<String> = self
                    .devices
                    .inventory
                    .inputs
                    .iter()
                    .map(|d| d.name.clone())
                    .collect();
                if names.is_empty() && self.config.audio_input_device.is_none() {
                    self.status = "No audio inputs discovered · press r to rescan".into();
                    return Ok(Outcome::default());
                }
                let next = cycle_optional(&names, self.config.audio_input_device.as_ref(), dir);
                Ok(self.set_audio_input_choice(next))
            }
            AudioDeckRow::InputMode => {
                let next = self.config.audio_input_mode.cycle(dir as i32);
                Ok(self.set_audio_input_mode_choice(next))
            }
            AudioDeckRow::InputMonitor => {
                let next = self.config.audio_input_monitor.cycle(dir as i32);
                Ok(self.set_audio_input_monitor_choice(next))
            }
            AudioDeckRow::LatencyTrim => {
                let next = self.config.audio_input_latency_ms + dir as f32;
                Ok(self.set_audio_latency_trim_ms(next))
            }
            AudioDeckRow::CueOut => {
                let names: Vec<String> = self
                    .devices
                    .inventory
                    .outputs
                    .iter()
                    .map(|d| d.name.clone())
                    .collect();
                if names.is_empty() && self.config.audio_cue_device.is_none() {
                    self.status = "No audio outputs discovered · press r to rescan".into();
                    return Ok(Outcome::default());
                }
                let next = cycle_optional(&names, self.config.audio_cue_device.as_ref(), dir);
                Ok(self.set_audio_cue_choice(next))
            }
            AudioDeckRow::SampleRate => {
                let rates = self.available_sample_rates();
                let next = cycle_optional(&rates, self.config.audio_sample_rate.as_ref(), dir);
                Ok(self.set_audio_sample_rate_choice(next))
            }
            AudioDeckRow::BufferSize => {
                let sizes = self.available_buffer_sizes();
                let next = cycle_optional(&sizes, self.config.audio_buffer_size.as_ref(), dir);
                Ok(self.set_audio_buffer_size_choice(next))
            }
            AudioDeckRow::AudioPanic | AudioDeckRow::RestartAudio | AudioDeckRow::RescanDevices => {
                self.status = format!("Press Enter to run {}", row.label());
                Ok(Outcome::default())
            }
        }
    }

    /// Universal reset (`Alt+Backspace`) for the focused `Devices · Audio` row.
    pub(crate) fn reset_audio_deck_row(&mut self) -> Result<Outcome> {
        let total = AudioDeckRow::ALL.len();
        self.devices.cursor = self.devices.cursor.min(total - 1);
        let Some(row) = AudioDeckRow::from_index(self.devices.cursor) else {
            return Ok(Outcome::default());
        };
        match row {
            AudioDeckRow::Output => Ok(self.set_audio_output_choice(None)),
            AudioDeckRow::Input => Ok(self.set_audio_input_choice(None)),
            AudioDeckRow::InputMode => Ok(self.set_audio_input_mode_choice(AudioInputMode::Stereo)),
            AudioDeckRow::InputMonitor => {
                Ok(self.set_audio_input_monitor_choice(InputMonitorMode::Off))
            }
            AudioDeckRow::LatencyTrim => Ok(self.set_audio_latency_trim_ms(0.0)),
            AudioDeckRow::CueOut => Ok(self.set_audio_cue_choice(None)),
            AudioDeckRow::SampleRate => Ok(self.set_audio_sample_rate_choice(None)),
            AudioDeckRow::BufferSize => Ok(self.set_audio_buffer_size_choice(None)),
            AudioDeckRow::AudioPanic | AudioDeckRow::RestartAudio | AudioDeckRow::RescanDevices => {
                Ok(Outcome::default())
            }
        }
    }

    /// Primary `Enter` action on the focused `Devices · Audio` row.
    pub(crate) fn activate_audio_deck_row(&mut self) -> Result<Outcome> {
        let total = AudioDeckRow::ALL.len();
        self.devices.cursor = self.devices.cursor.min(total - 1);
        let Some(row) = AudioDeckRow::from_index(self.devices.cursor) else {
            return Ok(Outcome::default());
        };
        match row {
            AudioDeckRow::Output => self.open_audio_output_picker(false),
            AudioDeckRow::Input => self.open_audio_input_picker(),
            AudioDeckRow::InputMode => {
                let next = self.config.audio_input_mode.cycle(1);
                Ok(self.set_audio_input_mode_choice(next))
            }
            AudioDeckRow::InputMonitor => {
                let next = self.config.audio_input_monitor.cycle(1);
                Ok(self.set_audio_input_monitor_choice(next))
            }
            AudioDeckRow::LatencyTrim => self.begin_audio_latency_trim_value_input(),
            AudioDeckRow::CueOut => self.open_audio_output_picker(true),
            AudioDeckRow::SampleRate => self.open_audio_sample_rate_picker(),
            AudioDeckRow::BufferSize => self.open_audio_buffer_size_picker(),
            AudioDeckRow::AudioPanic => self.apply(Action::AudioPanic),
            AudioDeckRow::RestartAudio => self.apply(Action::RestartAudio),
            AudioDeckRow::RescanDevices => self.apply(Action::RescanAudio),
        }
    }

    /// Top-level keyboard dispatch for the unified Devices deck (`Audio | MIDI`).
    pub(crate) fn handle_devices_deck_key(
        &mut self,
        key: KeyEvent,
        count: usize,
    ) -> Option<Result<Outcome>> {
        match key.code {
            KeyCode::Char('H') => {
                self.step_devices_tab(-1);
                return Some(Ok(Outcome::default()));
            }
            KeyCode::Char('L') => {
                self.step_devices_tab(1);
                return Some(Ok(Outcome::default()));
            }
            _ => {}
        }
        match self.devices.tab {
            DevicesTab::Audio => self.handle_audio_deck_key(key, count),
            DevicesTab::Midi => self.handle_midi_deck_key(key, count),
        }
    }

    /// Keyboard handling when `Devices · Audio` tab has focus.
    fn handle_audio_deck_key(&mut self, key: KeyEvent, count: usize) -> Option<Result<Outcome>> {
        let total = AudioDeckRow::ALL.len();
        self.devices.cursor = self.devices.cursor.min(total - 1);
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.devices.cursor = (self.devices.cursor + count).min(total - 1);
                Some(Ok(Outcome::default()))
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.devices.cursor = self.devices.cursor.saturating_sub(count);
                Some(Ok(Outcome::default()))
            }
            // Bare h/l strictly no-op on 1-axis vertical inspector deck.
            KeyCode::Char('h' | 'l') | KeyCode::Left | KeyCode::Right => {
                Some(Ok(Outcome::default()))
            }
            KeyCode::Char('w') => {
                if let Some(next) =
                    crate::jump_group_index(&AudioDeckRow::GROUPS, self.devices.cursor, true)
                {
                    self.devices.cursor = next;
                }
                Some(Ok(Outcome::default()))
            }
            KeyCode::Char('b') => {
                if let Some(next) =
                    crate::jump_group_index(&AudioDeckRow::GROUPS, self.devices.cursor, false)
                {
                    self.devices.cursor = next;
                }
                Some(Ok(Outcome::default()))
            }
            KeyCode::Enter => Some(self.activate_audio_deck_row()),
            KeyCode::Char('r') => Some(self.apply(Action::RescanAudio)),
            // Do NOT shadow global `t`, `i`, `S`, `R`, `Backspace`, etc.
            _ => None,
        }
    }

    /// `:audio [deck|open|restart|rescan|panic]` command handler.
    pub(crate) fn execute_audio_command(&mut self, args: &[String]) -> Result<Outcome> {
        match args.first().map(|s| s.to_ascii_lowercase()).as_deref() {
            None | Some("deck" | "open") => {
                self.open_devices_deck(DevicesTab::Audio);
                self.status =
                    "Devices · Audio · H/L tab · j/k row · w/b group · Alt+h/l ,/. cycle · Enter pick/run · r rescan"
                        .into();
                Ok(Outcome::default())
            }
            Some("restart") => self.apply(Action::RestartAudio),
            Some("rescan") => self.apply(Action::RescanAudio),
            Some("panic" | "silence") => self.apply(Action::AudioPanic),
            Some(other) => bail!("Unknown :audio subcommand '{other}' · try restart|rescan|panic"),
        }
    }
}
