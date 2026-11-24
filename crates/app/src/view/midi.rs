//! Semantic view model for the unified GPU Devices Deck (`gd` / `Ctrl+d` — `Audio | MIDI`).
use crate::devices::{
    auto_buffer_label, format_buffer_latency, format_sample_rate, AudioDeckRow, DevicesTab,
};
use crate::midi::MidiDeckRow;
use crate::{App, MidiLogEntry};
use cadence_core::midi_map::{MidiSource, MidiTarget};
use cadence_core::runtime::{
    default_auto_buffer_frames, AudioInputStatus, AudioSnapshot, AudioStreamStatus, CapturePhase,
    InputMonitorMode,
};

#[derive(Clone, Debug)]
pub struct AudioInputView {
    pub status: AudioInputStatus,
    pub active_input: Option<String>,
    pub sample_rate: u32,
    pub channels: u16,
    pub buffer_frames: u32,
    pub peak: [f32; 2],
    pub peak_hold: [f32; 2],
    pub clipped: bool,
    pub digital_silence: bool,
    pub mode_label: &'static str,
    pub monitor_label: &'static str,
    pub monitoring: bool,
    pub capture_phase: CapturePhase,
    pub captured_sec: f32,
    pub dropped_frames: u64,
    pub round_trip_latency_ms: Option<f32>,
    pub latency_trim_ms: f32,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct AudioDeckRowView {
    pub group: &'static str,
    pub label: &'static str,
    pub value: String,
    pub detail: String,
    pub active: bool,
    pub selected: bool,
    pub actionable: bool,
}

#[derive(Clone, Debug)]
pub struct AudioDeckView {
    pub rows: Vec<AudioDeckRowView>,
    pub cursor: usize,
    pub status: AudioStreamStatus,
    pub cpu_load: f32,
    pub peak_cpu_load: f32,
    pub xrun_count: u64,
    pub sample_rate: u32,
    pub buffer_frames: u32,
    pub latency_ms: Option<f32>,
    pub channels: u16,
    pub format_label: String,
    pub active_output: Option<String>,
    pub silent_mode: bool,
    pub overloaded: bool,
    pub failed: bool,
    pub last_error: Option<String>,
    pub input: AudioInputView,
}

#[derive(Clone, Debug)]
pub struct MidiDeviceRowView {
    pub label: &'static str,
    pub value: String,
    pub detail: String,
    pub active: bool,
    pub selected: bool,
    pub alert: bool,
}

#[derive(Clone, Debug)]
pub struct MidiPadTileView {
    pub index: usize,
    pub label: String,
    pub note_badge: String,
    /// `0..=127` hardware RGB value mirrored directly in the pad LED tile.
    pub rgb: (u8, u8, u8),
    pub lit: bool,
}

#[derive(Clone, Debug)]
pub struct MidiMacroTileView {
    pub slot: usize,
    pub label: String,
    pub value_display: String,
    pub normalized: f32,
    pub cc_badge: Option<String>,
    pub is_project_macro: bool,
    pub binding_count: usize,
    pub automated: bool,
}

#[derive(Clone, Debug)]
pub struct MidiMappingRowView {
    pub source: String,
    pub mode: &'static str,
    pub target: String,
    pub flags: String,
    pub live_norm: Option<f32>,
    pub is_user: bool,
    pub selected: bool,
    pub learning: bool,
}

#[derive(Clone, Debug)]
pub struct MidiDeckView {
    pub active_tab: DevicesTab,
    pub tabs: [(&'static str, bool); 2],
    pub audio: Box<AudioDeckView>,
    pub midi_out_error: Option<String>,
    pub in_port: Option<String>,
    pub out_port: Option<String>,
    pub in_count: usize,
    pub out_count: usize,
    pub profile_name: String,
    pub custom_profile_path: Option<String>,
    pub feedback_label: &'static str,
    pub pad_mode_label: &'static str,
    pub pickup: bool,
    pub recording: bool,
    pub cc_last_recorded: Option<String>,
    pub learn_target: Option<String>,
    pub user_mapping_count: usize,
    pub device_rows: Vec<MidiDeviceRowView>,
    pub pads: Vec<MidiPadTileView>,
    pub macros: Vec<MidiMacroTileView>,
    pub project_macro_count: usize,
    pub log: Vec<MidiLogEntry>,
    pub mappings: Vec<MidiMappingRowView>,
    /// Unified vertical row cursor across `device_rows` (`0..5`) and `mappings` (`5..`).
    pub cursor: usize,
}

impl App {
    pub(super) fn build_midi_view(&self, snapshot: &AudioSnapshot) -> MidiDeckView {
        let profile = self.active_midi_profile();
        let effective = self.effective_midi_mappings();
        let pad_states = self.compute_pad_states();
        let pads = pad_states
            .into_iter()
            .enumerate()
            .map(|(index, (label, rgb, lit))| MidiPadTileView {
                index,
                label,
                note_badge: format!("N{}", profile.pad_notes[index]),
                rgb,
                lit,
            })
            .collect();

        let macro_targets = self.focused_macro_targets();
        let mut project_macro_count = 0usize;
        let macros = (0..8)
            .map(|slot| {
                let cc_badge = effective
                    .iter()
                    .find(|(m, _)| m.target == MidiTarget::ProjectMacro { slot })
                    .or_else(|| {
                        effective
                            .iter()
                            .find(|(m, _)| m.target == MidiTarget::FocusedMacro { slot })
                    })
                    .map(|(m, _)| m.source.short_badge());
                if let Some(mac_idx) = self.focused_macro_resolves_project_macro(slot) {
                    if let Some(mac) = self.project().macro_control(mac_idx) {
                        project_macro_count += 1;
                        let live = self.live_macro_modulation(mac_idx, snapshot);
                        let norm = live.as_ref().map_or(mac.value, |(v, _)| *v);
                        return MidiMacroTileView {
                            slot,
                            label: mac.name.clone(),
                            value_display: format!("{:.0}%", norm * 100.0),
                            normalized: norm,
                            cc_badge,
                            is_project_macro: true,
                            binding_count: mac.bindings.len(),
                            automated: live.is_some(),
                        };
                    }
                }
                if let Some((label, address)) = macro_targets.get(slot) {
                    let norm = self.read_param_normalized(*address).unwrap_or(0.0);
                    let display = self.format_param_normalized_value(*address, norm);
                    MidiMacroTileView {
                        slot,
                        label: label.clone(),
                        value_display: display,
                        normalized: norm,
                        cc_badge,
                        is_project_macro: false,
                        binding_count: 0,
                        automated: false,
                    }
                } else {
                    MidiMacroTileView {
                        slot,
                        label: format!("Macro {}", slot + 1),
                        value_display: "—".into(),
                        normalized: 0.0,
                        cc_badge,
                        is_project_macro: false,
                        binding_count: 0,
                        automated: false,
                    }
                }
            })
            .collect();

        let total_rows = MidiDeckRow::COUNT + effective.len();
        let cursor = self.midi.cursor.min(total_rows.saturating_sub(1));
        let custom_profile_path = self
            .midi
            .custom_profile
            .as_ref()
            .and(self.config.midi_profile_path.as_ref())
            .map(|p| p.display().to_string());

        let in_value = match &self.midi.port {
            Some(p) => p.clone(),
            None => format!("Off ({} available)", self.midi.in_ports.len()),
        };
        let out_value = match &self.midi.out_port {
            Some(p) => p.clone(),
            None => format!("Off ({} available)", self.midi.out_ports.len()),
        };
        let profile_value = if self.midi.custom_profile.is_some() {
            format!("{} [CUSTOM]", profile.name)
        } else {
            profile.name.clone()
        };
        let profile_detail = match &custom_profile_path {
            Some(path) => format!("{} · {path}", profile.feedback.label()),
            None => format!(
                "{} bindings · feedback: {} · Enter pick/load/save",
                profile.mappings.len(),
                profile.feedback.label()
            ),
        };
        let out_detail = match &self.devices.midi_out_error {
            Some(err) => format!("OUT ERROR: {err}"),
            None => "LED/OLED feedback · Alt+h/l ,/. cycle · Enter pick".into(),
        };
        let device_rows = vec![
            MidiDeviceRowView {
                label: "IN",
                value: in_value,
                detail: "Alt+h/l ,/. cycle · Enter pick port · r rescan".into(),
                active: self.midi.port.is_some(),
                selected: cursor == MidiDeckRow::In.index(),
                alert: false,
            },
            MidiDeviceRowView {
                label: "OUT",
                value: out_value,
                detail: out_detail,
                active: self.midi.out_port.is_some(),
                selected: cursor == MidiDeckRow::Out.index(),
                alert: self.devices.midi_out_error.is_some(),
            },
            MidiDeviceRowView {
                label: "Profile",
                value: profile_value,
                detail: profile_detail,
                active: true,
                selected: cursor == MidiDeckRow::Profile.index(),
                alert: false,
            },
            MidiDeviceRowView {
                label: "Pads",
                value: self.config.midi_pad_mode.label().into(),
                detail: "Alt+h/l ,/. cycle · Enter pick (Notes / Sections / Mutes)".into(),
                active: self.config.midi_pad_mode != cadence_core::midi_map::MidiPadMode::Notes,
                selected: cursor == MidiDeckRow::Pads.index(),
                alert: false,
            },
            MidiDeviceRowView {
                label: "Pickup",
                value: if self.config.midi_pickup {
                    "On (Soft Takeover)".into()
                } else {
                    "Off (Immediate)".into()
                },
                detail: "Prevent parameter jumps on absolute knobs · Enter / Alt+h/l toggle".into(),
                active: self.config.midi_pickup,
                selected: cursor == MidiDeckRow::Pickup.index(),
                alert: false,
            },
        ];

        let mappings = effective
            .iter()
            .enumerate()
            .map(|(i, (m, is_user))| {
                let live_norm = match m.source {
                    MidiSource::Cc { cc, .. } => self
                        .midi
                        .cc_live
                        .get(usize::from(cc))
                        .and_then(|v| *v)
                        .map(|v| f32::from(v) / 127.0)
                        .or_else(|| {
                            self.resolve_target_project_macro(m.target).map(|idx| {
                                self.project().macro_control(idx).map_or(0.0, |mc| mc.value)
                            })
                        }),
                    _ => self
                        .resolve_target_project_macro(m.target)
                        .map(|idx| self.project().macro_control(idx).map_or(0.0, |mc| mc.value))
                        .or_else(|| {
                            self.resolve_target_param(m.target)
                                .and_then(|addr| self.read_param_normalized(addr))
                        }),
                };
                let target_label = if let Some(idx) = self.resolve_target_project_macro(m.target) {
                    if let Some(mc) = self.project().macro_control(idx) {
                        format!("M{} · {}", usize::from(idx) + 1, mc.name)
                    } else {
                        m.label.clone()
                    }
                } else {
                    m.label.clone()
                };
                let mut flags = Vec::new();
                if m.invert {
                    flags.push("INV");
                }
                if m.pickup
                    && self.config.midi_pickup
                    && m.mode == cadence_core::midi_map::MidiValueMode::Absolute
                {
                    flags.push("PICKUP");
                }
                if *is_user {
                    flags.push("USER");
                } else {
                    flags.push("PROF");
                }
                MidiMappingRowView {
                    source: m.source.display_label(),
                    mode: m.mode.label(),
                    target: target_label,
                    flags: flags.join(" · "),
                    live_norm,
                    is_user: *is_user,
                    selected: cursor == MidiDeckRow::COUNT + i,
                    learning: self.midi.learn == Some(m.target),
                }
            })
            .collect();

        let active_tab = self.devices.tab;
        let tabs = [
            (DevicesTab::Audio.label(), active_tab == DevicesTab::Audio),
            (DevicesTab::Midi.label(), active_tab == DevicesTab::Midi),
        ];
        let audio = self.build_audio_deck_view(snapshot);

        MidiDeckView {
            active_tab,
            tabs,
            audio: Box::new(audio),
            midi_out_error: self.devices.midi_out_error.clone(),
            in_port: self.midi.port.clone(),
            out_port: self.midi.out_port.clone(),
            in_count: self.midi.in_ports.len(),
            out_count: self.midi.out_ports.len(),
            profile_name: profile.name,
            custom_profile_path,
            feedback_label: profile.feedback.label(),
            pad_mode_label: self.config.midi_pad_mode.label(),
            pickup: self.config.midi_pickup,
            recording: self.midi.record,
            cc_last_recorded: self.midi_cc_last_recorded().map(str::to_owned),
            learn_target: self.midi.learn.map(|t| t.default_label()),
            user_mapping_count: self.config.midi_mappings.len(),
            device_rows,
            pads,
            macros,
            project_macro_count,
            log: self.midi.log.iter().cloned().collect(),
            mappings,
            cursor,
        }
    }

    fn build_audio_deck_view(&self, snapshot: &AudioSnapshot) -> AudioDeckView {
        let inv = &self.devices.inventory;
        let health = snapshot.health;
        let cursor = self
            .devices
            .cursor
            .min(AudioDeckRow::ALL.len().saturating_sub(1));

        let sample_rate = if snapshot.sample_rate > 0 {
            snapshot.sample_rate
        } else {
            inv.active_sample_rate
        };
        let buffer_frames = if health.buffer_frames > 0 {
            health.buffer_frames
        } else {
            inv.active_buffer_frames
        };
        let latency_ms = if buffer_frames > 0 && sample_rate > 0 {
            Some(buffer_frames as f32 * 1000.0 / sample_rate as f32)
        } else {
            health.buffer_latency_ms(sample_rate)
        };
        let channels = if health.channels > 0 {
            health.channels
        } else {
            inv.active_channels
        };
        let format_label = if !inv.active_format_label.is_empty() {
            inv.active_format_label.clone()
        } else if health.bit_depth > 0 {
            health.format_label().into()
        } else {
            "32-bit float".into()
        };
        let silent_mode = inv.silent_mode
            || (health.status == AudioStreamStatus::Silent && inv.active_output.is_none());
        let status = if health.failed {
            AudioStreamStatus::Failed
        } else if health.overloaded {
            AudioStreamStatus::Overloaded
        } else if health.status != AudioStreamStatus::Silent {
            health.status
        } else if !inv.silent_mode && inv.active_output.is_some() {
            AudioStreamStatus::Running
        } else {
            AudioStreamStatus::Silent
        };
        let last_error = inv
            .last_error
            .clone()
            .or_else(|| health.error_code.message().map(str::to_owned));
        let cpu_load = if health.cpu_load.is_finite() {
            health.cpu_load.max(0.0)
        } else {
            0.0
        };
        let peak_cpu_load = if health.peak_cpu_load.is_finite() {
            health.peak_cpu_load.max(0.0)
        } else {
            0.0
        };

        let latency_sr = self.effective_audio_sample_rate(snapshot.sample_rate);
        let active_out_info = self.active_output_device_info();
        let active_out_ch = if channels > 0 {
            channels
        } else {
            active_out_info.map_or(0, |d| d.channels)
        };

        let output_value = match &self.config.audio_output_device {
            Some(name) => name.clone(),
            None => "System Default".into(),
        };
        let output_detail = match (&self.config.audio_output_device, &inv.active_output) {
            (None, Some(active)) => {
                if active_out_ch > 0 {
                    format!(
                        "Active: {active} · {active_out_ch} ch · Alt+h/l ,/. cycle · Enter pick"
                    )
                } else {
                    format!("Active: {active} · Alt+h/l ,/. cycle · Enter pick")
                }
            }
            (Some(pref), Some(active)) if pref == active => {
                if active_out_ch > 0 {
                    format!("Active · {active_out_ch} ch · Alt+h/l ,/. cycle · Enter pick")
                } else {
                    "Active · Alt+h/l ,/. cycle · Enter pick".into()
                }
            }
            (Some(_), Some(active)) => {
                format!("Fallback active: {active} · Enter pick · Alt+Bksp default")
            }
            (_, None) => {
                if inv.outputs.is_empty() {
                    "Follows OS default · r rescans hardware outputs".into()
                } else {
                    format!(
                        "{} output{} available · Alt+h/l ,/. cycle · Enter pick",
                        inv.outputs.len(),
                        if inv.outputs.len() == 1 { "" } else { "s" }
                    )
                }
            }
        };

        let inp = snapshot.input;
        let in_sr = if inp.sample_rate > 0 {
            inp.sample_rate
        } else {
            inv.active_input_sample_rate
        };
        let in_ch = if inp.channels > 0 {
            inp.channels
        } else {
            inv.active_input_channels
        };
        let in_buf = if inp.buffer_frames > 0 {
            inp.buffer_frames
        } else {
            inv.active_input_buffer_frames
        };
        let input_status = if inp.status != AudioInputStatus::Inactive {
            inp.status
        } else if inv.input_last_error.is_some() {
            AudioInputStatus::Failed
        } else if inv.active_input.is_some() && !inv.silent_mode {
            AudioInputStatus::Running
        } else {
            AudioInputStatus::Inactive
        };
        let rt_latency_ms =
            inp.round_trip_latency_ms(health, sample_rate, self.config.audio_input_latency_ms);

        let input_value = self
            .config
            .audio_input_device
            .clone()
            .unwrap_or_else(|| "None".into());
        let input_detail = if let Some(err) = &inv.input_last_error {
            format!("INPUT ERROR: {err}")
        } else if let Some(active) = &inv.active_input {
            if in_sr > 0 && in_ch > 0 {
                format!(
                    "Active: {active} · {in_ch} ch · {} · t a records",
                    format_sample_rate(in_sr)
                )
            } else {
                format!("Selected: {active} · t a records · Enter pick")
            }
        } else {
            format!(
                "{} input{} available · Enter pick · t a records",
                inv.inputs.len(),
                if inv.inputs.len() == 1 { "" } else { "s" }
            )
        };

        let input_mode_value = self.config.audio_input_mode.label().to_string();
        let input_mode_detail =
            "Channel routing (Stereo 1+2 / Mono Ch 1 / Mono Ch 2) · Alt+h/l ,/. cycle".to_string();

        let input_monitor_value = self.config.audio_input_monitor.label().to_string();
        let input_monitor_detail = if inp.monitoring {
            "LIVE MONITORING · t i cycles Off / Auto (Armed) / On".to_string()
        } else {
            "Default Off prevents mic feedback · t i cycles Off / Auto / On".to_string()
        };

        let latency_trim_value = format!("{:+.1} ms", self.config.audio_input_latency_ms);
        let latency_trim_detail = match rt_latency_ms {
            Some(rt) => {
                format!("Est. round-trip {rt:.1} ms · Alt+h/l ±1 ms · Enter exact · Alt+Bksp 0")
            }
            None => "Manual offset on top of hardware buffer latency · Enter exact".into(),
        };

        let cue_value = self
            .config
            .audio_cue_device
            .clone()
            .unwrap_or_else(|| "Off".into());
        let cue_detail = "Reserved · future cue bus · Alt+h/l ,/. cycle · Enter pick".to_string();

        let rate_value = match self.config.audio_sample_rate {
            Some(sr) => format_sample_rate(sr),
            None => "Auto (Device)".into(),
        };
        let rate_detail = if sample_rate > 0 {
            format!(
                "Active: {} · Alt+h/l ,/. cycle · Enter pick",
                format_sample_rate(sample_rate)
            )
        } else {
            "Uses hardware device default · Alt+h/l ,/. cycle · Enter pick".into()
        };

        let effective_frames = self
            .config
            .audio_buffer_size
            .or_else(|| (buffer_frames > 0).then_some(buffer_frames))
            .unwrap_or_else(default_auto_buffer_frames);
        let buffer_value = match self.config.audio_buffer_size {
            Some(frames) => format_buffer_latency(frames, latency_sr),
            None => format!(
                "Auto ({})",
                format_buffer_latency(effective_frames, latency_sr)
            ),
        };
        let buffer_detail =
            match active_out_info.and_then(|d| d.min_buffer_frames.zip(d.max_buffer_frames)) {
                Some((lo, hi)) => format!(
                    "{} · device {lo}–{hi} fr · Alt+h/l ,/. cycle · Enter pick",
                    auto_buffer_label()
                ),
                None => format!("{} · Alt+h/l ,/. cycle · Enter pick", auto_buffer_label()),
            };

        let rows = AudioDeckRow::ALL
            .into_iter()
            .enumerate()
            .map(|(idx, row)| {
                let (value, detail, active) = match row {
                    AudioDeckRow::Output => (
                        output_value.clone(),
                        output_detail.clone(),
                        inv.active_output.is_some() || self.config.audio_output_device.is_some(),
                    ),
                    AudioDeckRow::Input => (
                        input_value.clone(),
                        input_detail.clone(),
                        input_status == AudioInputStatus::Running
                            || self.config.audio_input_device.is_some(),
                    ),
                    AudioDeckRow::InputMode => (
                        input_mode_value.clone(),
                        input_mode_detail.clone(),
                        self.config.audio_input_device.is_some(),
                    ),
                    AudioDeckRow::InputMonitor => (
                        input_monitor_value.clone(),
                        input_monitor_detail.clone(),
                        inp.monitoring || self.config.audio_input_monitor != InputMonitorMode::Off,
                    ),
                    AudioDeckRow::LatencyTrim => (
                        latency_trim_value.clone(),
                        latency_trim_detail.clone(),
                        self.config.audio_input_latency_ms.abs() > 0.01,
                    ),
                    AudioDeckRow::CueOut => (
                        cue_value.clone(),
                        cue_detail.clone(),
                        self.config.audio_cue_device.is_some(),
                    ),
                    AudioDeckRow::SampleRate => (
                        rate_value.clone(),
                        rate_detail.clone(),
                        sample_rate > 0 || self.config.audio_sample_rate.is_some(),
                    ),
                    AudioDeckRow::BufferSize => (
                        buffer_value.clone(),
                        buffer_detail.clone(),
                        buffer_frames > 0 || self.config.audio_buffer_size.is_some(),
                    ),
                    AudioDeckRow::AudioPanic => (
                        "Enter · Silence All".into(),
                        "Silence ringing voices & FX tails immediately · clears overload latch"
                            .into(),
                        health.overloaded,
                    ),
                    AudioDeckRow::RestartAudio => (
                        "Enter · Restart Stream".into(),
                        "Reopen active hardware audio stream with current device settings".into(),
                        health.failed || health.overloaded,
                    ),
                    AudioDeckRow::RescanDevices => (
                        "Enter / r · Rescan OS".into(),
                        format!(
                            "Re-enumerate hot-plugged devices ({} out · {} in · {} MIDI)",
                            inv.outputs.len(),
                            inv.inputs.len(),
                            self.midi.in_ports.len() + self.midi.out_ports.len()
                        ),
                        false,
                    ),
                };
                AudioDeckRowView {
                    group: row.group(),
                    label: row.label(),
                    value,
                    detail,
                    active,
                    selected: idx == cursor,
                    actionable: row.is_action(),
                }
            })
            .collect();

        let cap_phase = if self.recording.phase != CapturePhase::Idle {
            self.recording.phase
        } else {
            inp.capture_phase
        };
        let captured_frames = self.recording.captured_frames.max(inp.captured_frames);
        let captured_sec = if in_sr > 0 {
            captured_frames as f32 / in_sr as f32
        } else {
            0.0
        };
        let input_view = AudioInputView {
            status: input_status,
            active_input: inv
                .active_input
                .clone()
                .or_else(|| self.config.audio_input_device.clone()),
            sample_rate: in_sr,
            channels: in_ch,
            buffer_frames: in_buf,
            peak: inp.peak,
            peak_hold: inp.peak_hold,
            clipped: inp.clipped,
            digital_silence: inp.digital_silence,
            mode_label: self.config.audio_input_mode.label(),
            monitor_label: self.config.audio_input_monitor.label(),
            monitoring: inp.monitoring,
            capture_phase: cap_phase,
            captured_sec,
            dropped_frames: self.recording.dropped_frames.max(inp.dropped_frames),
            round_trip_latency_ms: rt_latency_ms,
            latency_trim_ms: self.config.audio_input_latency_ms,
            last_error: inv.input_last_error.clone(),
        };

        AudioDeckView {
            rows,
            cursor,
            status,
            cpu_load,
            peak_cpu_load,
            xrun_count: health.xrun_count,
            sample_rate,
            buffer_frames,
            latency_ms,
            channels,
            format_label,
            active_output: inv.active_output.clone(),
            silent_mode,
            overloaded: health.overloaded,
            failed: health.failed,
            last_error,
            input: input_view,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadence_core::material::Project;

    #[test]
    fn smart_macro_tiles_format_non_project_macros_with_native_parameter_units() {
        let mut app = App::new(Project::demo(), "midi-unit.json".into()).unwrap();
        app.press_keys("gi").unwrap();
        let view = app.build_midi_view(&AudioSnapshot::default());
        let cutoff_tile = view
            .macros
            .iter()
            .find(|t| !t.is_project_macro && t.label.to_ascii_lowercase().contains("cutoff"))
            .expect("expected Cutoff Smart Macro tile for focused Rack instrument");
        assert!(
            cutoff_tile.value_display.contains("Hz"),
            "expected native Hz/kHz unit on Cutoff Smart Macro tile, got {:?}",
            cutoff_tile.value_display
        );
    }
}
