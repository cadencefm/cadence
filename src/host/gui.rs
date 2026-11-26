//! Host bridge connecting `AudioRuntime`, bi-directional MIDI I/O, and optional
//! live `--ctl` control socket to `cadence-gui` through the single authoritative
//! [`apply_outcome_with_hooks`] dispatcher.
use super::ctl::CtlServer;
use super::{apply_outcome_with_hooks, HostRecorder, OutcomeHooks, RecoverySync};
use anyhow::Result;
use cadence_app::{App, FileDialogRequest, Outcome, RecordingHostCommand};
#[cfg(feature = "midi")]
use cadence_core::midi_map::MidiFeedback;
use cadence_core::runtime::{AudioSnapshot, AudioStreamRequest, MidiMessage};
use cadence_core::store::SessionDir;
use cadence_engine::audio::{AudioDeviceChange, AudioRuntime};
use cadence_gui::{GuiOptions, HostBridge, HostPollResult};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

const AUDIO_HOTPLUG_POLL_INTERVAL: Duration = Duration::from_millis(1500);
const MAX_HOTPLUG_BACKOFF: Duration = Duration::from_secs(10);

struct GuiOutcomeHooks {
    #[cfg(feature = "midi")]
    midi_in: Option<cadence_engine::midi::MidiInput>,
    #[cfg(feature = "midi")]
    midi_out: Option<cadence_engine::midi::MidiOutput>,
    recovery: RecoverySync,
    recorder: HostRecorder,
}

impl GuiOutcomeHooks {
    #[cfg(feature = "midi")]
    fn rescan_midi(&self, app: &mut App) {
        let ins = cadence_engine::midi::list_ports().unwrap_or_default();
        let outs = cadence_engine::midi::list_output_ports().unwrap_or_default();
        app.set_midi_available_ports(ins, outs);
    }

    #[cfg(feature = "midi")]
    fn sync_midi_out_error(&self, app: &mut App) -> bool {
        let next = self
            .midi_out
            .as_ref()
            .and_then(|out| out.last_error())
            .map(str::to_owned);
        if app.devices.midi_out_error != next {
            app.set_midi_out_error(next);
            true
        } else {
            false
        }
    }

    #[cfg(not(feature = "midi"))]
    fn sync_midi_out_error(&self, _app: &mut App) -> bool {
        false
    }

    #[cfg(feature = "midi")]
    fn handle_midi_impl(&mut self, app: &mut App, outcome: &Outcome) {
        if outcome.midi_rescan {
            self.rescan_midi(app);
            if outcome.audio_rescan {
                app.status = format!(
                    "Devices rescanned · Audio: {} out, {} in · MIDI: {} in, {} out",
                    app.devices.inventory.outputs.len(),
                    app.devices.inventory.inputs.len(),
                    app.midi.in_ports.len(),
                    app.midi.out_ports.len()
                );
            } else {
                app.status = format!(
                    "MIDI ports: {} in · {} out",
                    app.midi.in_ports.len(),
                    app.midi.out_ports.len()
                );
            }
        }
        if let Some(req) = &outcome.midi_connect {
            if let Some(target_in) = &req.input {
                self.midi_in = None;
                app.midi.port = None;
                if let Some(filter) = target_in {
                    match cadence_engine::midi::connect(filter) {
                        Ok(conn) => {
                            app.midi.port = Some(conn.port.clone());
                            app.status = format!("MIDI IN connected: {}", conn.port);
                            self.midi_in = Some(conn);
                        }
                        Err(err) => {
                            app.status = format!("MIDI IN error: {err:#}");
                        }
                    }
                } else {
                    app.status = "MIDI IN disconnected".into();
                }
            }
            if let Some(target_out) = &req.output {
                if let Some(old_out) = &mut self.midi_out {
                    old_out.send_feedback(
                        app.active_midi_feedback_protocol(),
                        &[MidiFeedback::InitDawMode(false)],
                    );
                }
                self.midi_out = None;
                app.midi.out_port = None;
                app.set_midi_out_error(None);
                if let Some(filter) = target_out {
                    match cadence_engine::midi::connect_output(filter) {
                        Ok(mut conn) => {
                            let port = conn.port.clone();
                            let sync = app.force_midi_feedback_sync();
                            conn.send_feedback(app.active_midi_feedback_protocol(), &sync);
                            app.midi.out_port = Some(port.clone());
                            if let Some(err) = conn.last_error() {
                                app.set_midi_out_error(Some(err.to_owned()));
                                app.status = format!("MIDI OUT error: {err}");
                            } else {
                                app.set_midi_out_error(None);
                                app.status = format!("MIDI OUT connected: {port}");
                            }
                            self.midi_out = Some(conn);
                        }
                        Err(err) => {
                            let msg = format!("{err:#}");
                            app.set_midi_out_error(Some(msg.clone()));
                            app.status = format!("MIDI OUT error: {msg}");
                        }
                    }
                } else {
                    app.status = "MIDI OUT disconnected".into();
                }
            }
        }
        if !outcome.midi_feedback.is_empty() {
            if let Some(out) = &mut self.midi_out {
                let had_err = out.last_error().is_some();
                out.send_feedback(app.active_midi_feedback_protocol(), &outcome.midi_feedback);
                let next_err = out.last_error().map(str::to_owned);
                if !had_err {
                    if let Some(err) = &next_err {
                        app.status = err.clone();
                    }
                }
                app.set_midi_out_error(next_err);
            }
        }
    }

    #[cfg(not(feature = "midi"))]
    fn handle_midi_impl(&mut self, app: &mut App, outcome: &Outcome) {
        if outcome.midi_rescan || outcome.midi_connect.is_some() {
            app.status = "This build has no MIDI support; build with the midi feature".into();
        }
    }
}

impl OutcomeHooks for GuiOutcomeHooks {
    fn handle_midi(&mut self, app: &mut App, outcome: &Outcome) {
        self.handle_midi_impl(app, outcome);
    }

    fn handle_recording(
        &mut self,
        audio: &mut AudioRuntime,
        cmd: &RecordingHostCommand,
    ) -> Result<()> {
        self.recorder.handle_command(cmd, audio)
    }

    fn supports_file_dialogs(&self) -> bool {
        true
    }

    fn open_file_dialog(&mut self, req: &FileDialogRequest) -> Option<PathBuf> {
        run_native_file_dialog(req)
    }

    fn on_state_synced(&mut self, app: &App) {
        self.recovery.sync(app);
    }
}

struct EngineBridge<'a> {
    engine: &'a mut AudioRuntime,
    hooks: GuiOutcomeHooks,
    audio_status: String,
    ctl: Option<CtlServer>,
    last_device_poll: Instant,
    device_poll_interval: Duration,
    last_device_change_notice: Option<String>,
}

impl EngineBridge<'_> {
    fn format_audio_status(engine: &AudioRuntime, has_ctl: bool) -> String {
        if has_ctl {
            format!("{} · wgpu · ctl", engine.description())
        } else {
            format!("{} · wgpu", engine.description())
        }
    }

    fn refresh_audio_status(&mut self) -> bool {
        let next = Self::format_audio_status(self.engine, self.ctl.is_some());
        if self.audio_status != next {
            self.audio_status = next;
            true
        } else {
            false
        }
    }

    fn handle_device_change(&mut self, app: &mut App, change: AudioDeviceChange) -> bool {
        let was_playing = app.transport.playing;
        let active_req = self.engine.active_request().clone();
        let primary_req = match &change {
            AudioDeviceChange::DeviceLost { .. } if active_req.output_device.is_some() => {
                AudioStreamRequest {
                    output_device: None,
                    ..active_req.clone()
                }
            }
            _ => active_req.clone(),
        };
        let mut result = self.engine.reconfigure_device(
            app.audio_project().clone(),
            app.sample_bank(),
            primary_req.clone(),
        );
        if result.is_err() && primary_req.output_device.is_some() {
            let fallback = AudioStreamRequest {
                output_device: None,
                ..primary_req
            };
            result = self.engine.reconfigure_device(
                app.audio_project().clone(),
                app.sample_bank(),
                fallback,
            );
        }
        app.set_audio_inventory(self.engine.enumerate_inventory());
        self.refresh_audio_status();
        match result {
            Ok(()) => {
                self.device_poll_interval = AUDIO_HOTPLUG_POLL_INTERVAL;
                self.last_device_change_notice = None;
                app.status = if was_playing {
                    format!("{} · paused safely", change.summary())
                } else {
                    change.summary()
                };
                true
            }
            Err(err) => {
                self.device_poll_interval =
                    (self.device_poll_interval.saturating_mul(2)).min(MAX_HOTPLUG_BACKOFF);
                let notice = format!("{} ({err:#})", change.summary());
                if self.last_device_change_notice.as_deref() != Some(notice.as_str()) {
                    app.status = notice.clone();
                    self.last_device_change_notice = Some(notice);
                    true
                } else {
                    false
                }
            }
        }
    }
}

impl HostBridge for EngineBridge<'_> {
    fn snapshot(&self) -> AudioSnapshot {
        self.engine.snapshot()
    }

    fn apply_outcome(&mut self, app: &mut App, outcome: &Outcome) -> Result<()> {
        if outcome.audio_reconfigure.is_some() || outcome.audio_restart || outcome.audio_rescan {
            self.device_poll_interval = AUDIO_HOTPLUG_POLL_INTERVAL;
            self.last_device_poll = Instant::now();
            self.last_device_change_notice = None;
        }
        let res = apply_outcome_with_hooks(app, Some(self.engine), outcome, &mut self.hooks);
        self.refresh_audio_status();
        res
    }

    fn poll_midi(&mut self) -> Vec<MidiMessage> {
        #[cfg(feature = "midi")]
        if let Some(input) = &self.hooks.midi_in {
            return input.drain();
        }
        Vec::new()
    }

    fn audio_status(&self) -> &str {
        &self.audio_status
    }

    fn set_wake_callback(&mut self, wake: Arc<dyn Fn() + Send + Sync>) {
        if let Some(ctl) = &self.ctl {
            ctl.set_wake(wake);
        }
    }

    fn poll_host(&mut self, app: &mut App) -> HostPollResult {
        self.engine
            .set_rack_activity_watch(app.active_patchbay_target());
        let mut redraw = false;
        let mut quit = false;
        if let Some(ctl) = &self.ctl {
            let summary = ctl.poll_and_dispatch(app, self.engine, &mut self.hooks);
            redraw |= summary.mutated;
            quit |= summary.quit;
        }
        // Skip non-fatal OS CoreAudio/device hot-plug queries while recording so synchronous
        // device enumeration never stalls the GUI thread or interrupts an active capture.
        if (!self.hooks.recorder.is_active() || self.engine.is_failed())
            && self.last_device_poll.elapsed() >= self.device_poll_interval
        {
            self.last_device_poll = Instant::now();
            if let Some(change) = self.engine.detect_device_change() {
                redraw |= self.handle_device_change(app, change);
            } else {
                self.device_poll_interval = AUDIO_HOTPLUG_POLL_INTERVAL;
                self.last_device_change_notice = None;
            }
        }
        if self.hooks.recorder.is_active() {
            // Routine capture ring drains are painted at paced 60 Hz by `about_to_wait`
            // (`active_input`); only recording finalization/failure requests an immediate redraw
            // so we never bypass `ACTIVE_FRAME_INTERVAL` and stall the Metal swapchain.
            let (_rec_drained, rec_follow_up) = self.hooks.recorder.poll(app, self.engine);
            if let Some(follow_up) = rec_follow_up {
                let _ =
                    apply_outcome_with_hooks(app, Some(self.engine), &follow_up, &mut self.hooks);
                redraw = true;
            } else if !self.hooks.recorder.is_active() {
                redraw = true;
            }
        }
        redraw |= self.refresh_audio_status();
        redraw |= self.hooks.sync_midi_out_error(app);
        HostPollResult { redraw, quit }
    }
}

pub fn run_gui(
    app: &mut App,
    engine: &mut AudioRuntime,
    midi_in: Option<&str>,
    midi_out: Option<&str>,
    session: Option<SessionDir>,
    ctl: Option<CtlServer>,
) -> Result<()> {
    #[cfg(feature = "midi")]
    let (midi_in_conn, midi_out_conn) = {
        let ins = cadence_engine::midi::list_ports().unwrap_or_default();
        let outs = cadence_engine::midi::list_output_ports().unwrap_or_default();
        app.set_midi_available_ports(ins, outs);

        // Explicit CLI --midi-in fails loudly if requested & unavailable; saved config port auto-connects softly.
        let in_conn = if let Some(filter) = midi_in {
            let input = cadence_engine::midi::connect(filter)?;
            app.midi.port = Some(input.port.clone());
            app.status = format!("MIDI IN: {}", input.port);
            Some(input)
        } else if let Some(saved) = app.config.midi_in_port.clone() {
            match cadence_engine::midi::connect(&saved) {
                Ok(input) => {
                    app.midi.port = Some(input.port.clone());
                    Some(input)
                }
                Err(_) => None,
            }
        } else {
            None
        };

        // Explicit CLI --midi-out fails loudly; otherwise try saved midi_out_port or match connected input port name!
        let out_conn = if let Some(filter) = midi_out {
            let mut output = cadence_engine::midi::connect_output(filter)?;
            let sync = app.force_midi_feedback_sync();
            output.send_feedback(app.active_midi_feedback_protocol(), &sync);
            app.midi.out_port = Some(output.port.clone());
            Some(output)
        } else if let Some(saved) = app.config.midi_out_port.clone() {
            match cadence_engine::midi::connect_output(&saved) {
                Ok(mut output) => {
                    let sync = app.force_midi_feedback_sync();
                    output.send_feedback(app.active_midi_feedback_protocol(), &sync);
                    app.midi.out_port = Some(output.port.clone());
                    Some(output)
                }
                Err(_) => None,
            }
        } else if let Some(filter) = midi_in.or(app.midi.port.as_deref()) {
            // Opportunistically pair matching output port for bi-directional LED/OLED feedback.
            match cadence_engine::midi::connect_output(filter) {
                Ok(mut output) => {
                    let sync = app.force_midi_feedback_sync();
                    output.send_feedback(app.active_midi_feedback_protocol(), &sync);
                    app.midi.out_port = Some(output.port.clone());
                    Some(output)
                }
                Err(_) => None,
            }
        } else {
            None
        };
        (in_conn, out_conn)
    };
    #[cfg(not(feature = "midi"))]
    if midi_in.is_some() || midi_out.is_some() {
        anyhow::bail!("This build has no MIDI support; build with the midi feature");
    }

    let audio_status = EngineBridge::format_audio_status(engine, ctl.is_some());
    let hooks = GuiOutcomeHooks {
        #[cfg(feature = "midi")]
        midi_in: midi_in_conn,
        #[cfg(feature = "midi")]
        midi_out: midi_out_conn,
        recovery: RecoverySync::new(session),
        recorder: HostRecorder::default(),
    };
    hooks.sync_midi_out_error(app);
    let mut bridge = EngineBridge {
        engine,
        hooks,
        audio_status,
        ctl,
        last_device_poll: Instant::now(),
        device_poll_interval: AUDIO_HOTPLUG_POLL_INTERVAL,
        last_device_change_notice: None,
    };
    let options = GuiOptions {
        title: format!("Cadence — {}", app.project().title),
    };
    let res = cadence_gui::run_gui(app, &mut bridge, &options);
    #[cfg(feature = "midi")]
    if let Some(out) = &mut bridge.hooks.midi_out {
        out.send_feedback(
            app.active_midi_feedback_protocol(),
            &[MidiFeedback::InitDawMode(false)],
        );
    }
    bridge.hooks.recovery.flush_on_exit(app);
    drop(bridge.ctl);
    res
}

fn run_native_file_dialog(req: &FileDialogRequest) -> Option<PathBuf> {
    match req {
        FileDialogRequest::OpenProject => rfd::FileDialog::new()
            .set_title("Open Cadence Project, Take, Preset, or ReBirth .rbs")
            .add_filter("Cadence / ReBirth / Take", &["json", "rbs", "jsonl"])
            .pick_file(),
        FileDialogRequest::SaveProjectAs { default_name } => rfd::FileDialog::new()
            .set_title("Save Cadence Project As")
            .set_file_name(default_name)
            .add_filter("Cadence Project", &["json"])
            .save_file(),
        FileDialogRequest::SaveBundleDir => rfd::FileDialog::new()
            .set_title("Choose Folder for Portable Project Bundle")
            .pick_folder(),
        FileDialogRequest::SaveInstrumentPreset { default_name } => rfd::FileDialog::new()
            .set_title("Save Instrument Preset As")
            .set_file_name(default_name)
            .add_filter("Cadence Instrument Preset", &["json"])
            .save_file(),
        FileDialogRequest::SaveMidiProfile { default_name } => rfd::FileDialog::new()
            .set_title("Save MIDI Profile As")
            .set_file_name(default_name)
            .add_filter("Cadence MIDI Profile", &["json"])
            .save_file(),
        FileDialogRequest::SaveTake { default_name } => rfd::FileDialog::new()
            .set_title("Save Live Take As")
            .set_file_name(default_name)
            .add_filter("Cadence Live Take", &["jsonl"])
            .save_file(),
        FileDialogRequest::ImportAsset => rfd::FileDialog::new()
            .set_title("Import ReBirth .rbs, Preset, Groove Bank, MIDI Profile, or Take")
            .add_filter(
                "Supported Imports (.rbs, .json, .jsonl)",
                &["rbs", "json", "jsonl"],
            )
            .pick_file(),
        FileDialogRequest::ExportWav { default_name } => rfd::FileDialog::new()
            .set_title("Export Stereo WAV")
            .set_file_name(default_name)
            .add_filter("WAV Audio", &["wav"])
            .save_file(),
        FileDialogRequest::ExportStemsDir => rfd::FileDialog::new()
            .set_title("Choose Folder for Master + Per-Channel Stems")
            .pick_folder(),
        FileDialogRequest::PickSampleDir => rfd::FileDialog::new()
            .set_title("Add Sample Library Directory")
            .pick_folder(),
        FileDialogRequest::PickPresetDir => rfd::FileDialog::new()
            .set_title("Add External Preset Directory")
            .pick_folder(),
        FileDialogRequest::PickWorkflowSampleFolder => rfd::FileDialog::new()
            .set_title("Choose Sample Folder to Import")
            .pick_folder(),
    }
}
