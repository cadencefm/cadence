//! Hosting the app: connecting GPU window, scripts, stdio, and live `--ctl`
//! sockets to the session and the audio engine through one authoritative
//! [`apply_outcome_with_hooks`] dispatcher.
pub mod ctl;
#[cfg(feature = "gui")]
mod gui;
mod kit_render;
pub mod protocol;
pub mod recording;
mod script;

#[cfg(feature = "gui")]
pub use gui::run_gui;
pub use kit_render::{
    render_kit, ExportedVoicePreset, KitRenderOptions, KitRenderSummary, RenderedVoiceSummary,
};
pub use protocol::{
    execute_host_request, execute_host_request_bytes, process_requests, HostRequest,
    HostRequestContext, HostRequestOutcome, ListenRequest, LiveListenJob,
};
pub use recording::HostRecorder;
pub use script::replay_key_script;

use anyhow::{anyhow, Context, Result};
use cadence_app::{parse_key_sequence, App, FileDialogRequest, Outcome, RecordingHostCommand};
use cadence_core::runtime::{AudioStreamRequest, TransportIntent};
use cadence_core::store::SessionDir;
use cadence_engine::audio::AudioRuntime;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

/// Throttled atomic crash-recovery autosave tied to a [`SessionDir`].
pub struct RecoverySync {
    session: Option<SessionDir>,
    last_revision: Option<u64>,
    last_write: Instant,
    has_written: bool,
}

impl RecoverySync {
    pub fn new(session: Option<SessionDir>) -> Self {
        let has_written = session.as_ref().is_some_and(SessionDir::has_recovery);
        Self {
            session,
            last_revision: None,
            last_write: Instant::now() - Duration::from_secs(10),
            has_written,
        }
    }

    pub fn sync(&mut self, app: &App) {
        let Some(session) = &self.session else {
            return;
        };
        if app.dirty() {
            let rev = app.revision();
            if self.last_revision != Some(rev)
                && self.last_write.elapsed() >= Duration::from_secs(2)
                && session.write_recovery(app.project(), app.flags()).is_ok()
            {
                self.last_revision = Some(rev);
                self.last_write = Instant::now();
                self.has_written = true;
            }
        } else if self.has_written {
            session.clear_recovery();
            self.last_revision = None;
            self.has_written = false;
        }
    }

    pub fn flush_on_exit(&mut self, app: &App) {
        let Some(session) = &self.session else {
            return;
        };
        if !app.dirty() {
            session.clear_recovery();
            self.has_written = false;
        }
    }
}

pub fn start_audio(app: &App, silent: bool) -> Result<(AudioRuntime, Option<String>)> {
    start_audio_with_config(app, silent, AudioStreamRequest::default())
}

/// Start the live audio engine using explicit [`AudioStreamRequest`] preferences
/// (from `UserConfig` and/or CLI overrides), falling back cleanly to the silent clock
/// with an actionable warning if hardware audio cannot be opened.
pub fn start_audio_with_config(
    app: &App,
    silent: bool,
    request: AudioStreamRequest,
) -> Result<(AudioRuntime, Option<String>)> {
    let bank = app.sample_bank();
    match AudioRuntime::start_with_config(
        app.audio_project().clone(),
        bank.clone(),
        silent,
        request.clone(),
    ) {
        Ok(mut engine) => {
            // Softly open persisted audio input stream if configured (never fails startup if unplugged).
            if !silent && app.config.audio_input_device.is_some() {
                let _ = engine.configure_input(app.config.audio_input_request());
            }
            let warning = (!silent).then(|| engine.last_error()).flatten();
            Ok((engine, warning))
        }
        Err(error) if !silent => Ok((
            AudioRuntime::start_with_config(app.audio_project().clone(), bank, true, request)?,
            Some(format!(
                "Audio unavailable ({error}); silent synth active. WAV export still works."
            )),
        )),
        Err(error) => Err(error),
    }
}

/// Collect and load every sample referenced across all compiled [`ScoreWindow`] segments
/// of a recorded [`Take`], so takes whose earlier segments referenced replaced or removed
/// sampler instruments render identically in both CLI `--render --take` and GUI Take export.
pub fn load_take_samples(
    scores: &[(u64, cadence_core::score::ScoreWindow)],
    library: &mut cadence_core::sampler::SampleLibrary,
) -> Result<cadence_core::runtime::SampleBank> {
    let report = cadence_core::assets::load_schedule_samples(
        scores.iter().map(|(_, compiled)| compiled),
        library,
    );
    if !report.issues.is_empty() {
        report
            .ensure_ready()
            .context("Take needs unavailable samples; open it with --project in its folder")?;
    }
    Ok(report.bank)
}

/// Execute a WAV or Master+Stems `ExportRequest` and return rich post-export telemetry.
pub fn execute_export_request(
    app: &mut App,
    export: &cadence_app::ExportRequest,
) -> Result<cadence_app::ExportSummary> {
    let (report, files_written, target) = if export.use_take {
        let (scores, start_step, steps) = {
            let take = app
                .current_take()
                .ok_or_else(|| anyhow!("No recorded take available to export"))?;
            let scores = take.compile(2)?;
            let steps = take
                .steps()
                .max(16)
                .checked_add(32)
                .context("Take export length overflowed")?;
            (scores, take.header.start_step, steps)
        };
        let bank = load_take_samples(&scores, &mut app.assets.library)?;
        let schedule: Vec<_> = scores.iter().map(|(at, c)| (*at, c)).collect();
        let report = cadence_engine::audio::render_schedule_report(
            &schedule,
            start_step,
            steps,
            bank,
            &export.path,
            export.sample_rate,
        )?;
        (report, 1usize, export.path.display().to_string())
    } else {
        match export.mode {
            cadence_app::ExportMode::MasterWav => {
                let report = cadence_engine::audio::render_schedule_report(
                    &[(0, app.compiled_project())],
                    export.start_step,
                    export.steps.max(16),
                    app.sample_bank(),
                    &export.path,
                    export.sample_rate,
                )?;
                (report, 1usize, export.path.display().to_string())
            }
            cadence_app::ExportMode::MasterAndStems => {
                let stems = cadence_engine::audio::render_stems_with_samples_report(
                    app.project(),
                    app.flags(),
                    app.sample_bank(),
                    &export.path,
                    export.start_step,
                    export.steps.max(16),
                    export.sample_rate,
                )?;
                let count = 1 + stems.stem_paths.len();
                (
                    stems.master_report,
                    count,
                    export.path.display().to_string(),
                )
            }
        }
    };
    let duration_sec = report.frames as f64 / f64::from(report.sample_rate.max(1));
    let rms = ((report.output.rms[0] * report.output.rms[0]
        + report.output.rms[1] * report.output.rms[1])
        * 0.5)
        .sqrt();
    Ok(cadence_app::ExportSummary {
        target,
        files_written,
        duration_sec,
        peak_left_db: cadence_core::dsp::lin_to_db_f64(report.output.peak[0]),
        peak_right_db: cadence_core::dsp::lin_to_db_f64(report.output.peak[1]),
        rms_db: cadence_core::dsp::lin_to_db_f64(rms),
        clips: report.output.full_scale_samples,
        sample_rate: report.sample_rate,
    })
}

/// Execute a resampling `BounceRequest`: render exact stereo 16-bit WAV audio via
/// `cadence_engine::audio::render::render_bounce_wav`, then create & load the
/// playable `Sampler` instrument inside `app`.
pub fn execute_bounce_request(
    app: &mut App,
    bounce: &cadence_app::BounceRequest,
) -> Result<Outcome> {
    let report = cadence_engine::audio::render::render_bounce_wav(
        &bounce.score,
        app.sample_bank(),
        bounce.start_step,
        bounce.steps,
        &bounce.output_path,
        bounce.sample_rate,
    )?;
    let duration_sec = report.frames as f64 / f64::from(report.sample_rate.max(1));
    app.complete_bounce(bounce, duration_sec)
}

/// Host-mode hooks plugged into the single authoritative [`apply_outcome_with_hooks`] dispatcher.
/// Headless hosts (`--script`, `--stdio`) use [`HeadlessOutcomeHooks`]; GUI supplies MIDI,
/// native file dialog, and crash-recovery sync hooks.
pub trait OutcomeHooks {
    fn handle_midi(&mut self, _app: &mut App, _outcome: &Outcome) {}
    fn handle_recording(
        &mut self,
        audio: &mut AudioRuntime,
        cmd: &RecordingHostCommand,
    ) -> Result<()> {
        match cmd {
            RecordingHostCommand::Arm { request } => audio.arm_capture(request.arm_spec),
            RecordingHostCommand::Stop => {
                let _ = audio.stop_capture();
                Ok(())
            }
            RecordingHostCommand::Cancel => {
                audio.cancel_capture();
                Ok(())
            }
        }
    }
    fn supports_file_dialogs(&self) -> bool {
        false
    }
    fn open_file_dialog(&mut self, _req: &FileDialogRequest) -> Option<PathBuf> {
        None
    }
    fn on_state_synced(&mut self, _app: &App) {}
}

/// Default headless host hooks (no MIDI hardware ports or native OS file dialogs).
#[derive(Default)]
pub struct HeadlessOutcomeHooks;

impl OutcomeHooks for HeadlessOutcomeHooks {}

const MAX_OUTCOME_RECURSION: usize = 4;

fn sync_audio_runtime(
    app: &mut App,
    engine: &mut AudioRuntime,
    out: &Outcome,
    hooks: &mut dyn OutcomeHooks,
) -> Result<()> {
    let mut reconfigured = false;
    if let Some(req) = &out.audio_reconfigure {
        let res =
            engine.reconfigure_device(app.audio_project().clone(), app.sample_bank(), req.clone());
        app.set_audio_inventory(engine.enumerate_inventory());
        match res {
            Ok(()) => {
                reconfigured = true;
                app.status = format!("Audio active · {}", engine.description());
            }
            Err(err) => {
                app.status = format!("Audio switch failed: {err:#}");
                return Err(err);
            }
        }
    } else if out.audio_restart {
        let res = engine.restart_device(app.audio_project().clone(), app.sample_bank());
        if res.is_ok() && app.config.audio_input_device.is_some() {
            let _ = engine.configure_input(app.config.audio_input_request());
        }
        app.set_audio_inventory(engine.enumerate_inventory());
        match res {
            Ok(()) => {
                reconfigured = true;
                app.status = format!("Audio restarted · {}", engine.description());
            }
            Err(err) => {
                app.status = format!("Audio restart failed: {err:#}");
                return Err(err);
            }
        }
    }

    if let Some(in_req) = &out.audio_input_reconfigure {
        let res = engine.configure_input(in_req.clone());
        app.set_audio_inventory(engine.enumerate_inventory());
        if let Err(err) = res {
            app.status = format!("Audio input error: {err:#}");
            return Err(err);
        }
    }

    if out.audio_rescan {
        let inv = engine.enumerate_inventory();
        let out_count = inv.outputs.len();
        let in_count = inv.inputs.len();
        let silent = inv.silent_mode;
        app.set_audio_inventory(inv);
        if !out.midi_rescan {
            app.status = if silent {
                "Silent audio mode · no hardware devices".into()
            } else {
                format!("Audio devices: {out_count} out · {in_count} in")
            };
        }
    }

    if !reconfigured && (out.changed || out.samples_changed) {
        let bank = (out.samples_changed || out.project_boundary).then(|| app.sample_bank());
        engine.replace_project_with_assets(
            app.audio_project().clone(),
            bank,
            out.project_boundary,
        )?;
    }
    // Arm audio capture BEFORE dispatching TransportIntent::Play so step 0 is never missed
    // when starting transport from a stopped state.
    if let Some(cmd @ RecordingHostCommand::Arm { .. }) = &out.recording_command {
        if let Err(err) = hooks.handle_recording(engine, cmd) {
            app.fail_recording(format!("{err:#}"));
            return Err(err);
        }
    }
    for &intent in &out.transport {
        if matches!(intent, TransportIntent::Reset { .. }) {
            if !engine.transport(intent) {
                engine.panic_silence();
            }
        } else {
            anyhow::ensure!(
                engine.transport(intent),
                "Transport queue is full or engine unavailable; retry the action"
            );
        }
    }
    if let Some(channel) = out.clear_mixer_meters {
        engine.clear_mixer_meters(channel);
    }
    if out.stop_audition {
        engine.stop_audition();
    }
    if let Some((patch, midi, velocity)) = &out.rack_audition {
        engine.preview_rack(patch.clone(), *midi, *velocity)?;
    }
    if let Some(spec) = &out.preview_sample {
        engine.preview_sample_spec(spec.clone());
    }
    if let Some((instrument, midi, velocity)) = out.audition_instrument {
        engine.audition_instrument(instrument, midi, velocity);
    }
    for note in &out.live_notes_on {
        engine.live_note_on_instrument(
            note.instrument_id,
            note.midi,
            note.velocity,
            note.sustain,
            note.part_id,
        );
    }
    for note in &out.live_notes_off {
        engine.live_note_off_instrument(note.instrument_id, note.midi);
    }
    if let Some(cmd) = &out.recording_command {
        if !matches!(cmd, RecordingHostCommand::Arm { .. }) {
            if let Err(err) = hooks.handle_recording(engine, cmd) {
                app.fail_recording(format!("{err:#}"));
                return Err(err);
            }
        }
    }
    Ok(())
}

fn apply_outcome_inner(
    app: &mut App,
    mut audio: Option<&mut AudioRuntime>,
    out: &Outcome,
    hooks: &mut dyn OutcomeHooks,
    depth: usize,
) -> Result<()> {
    anyhow::ensure!(
        depth < MAX_OUTCOME_RECURSION,
        "Outcome dispatch recursion limit exceeded"
    );
    let mut first_error: Option<anyhow::Error> = None;
    let mut record_err = |err: anyhow::Error| {
        if first_error.is_none() {
            first_error = Some(err);
        }
    };

    if let Some(engine) = audio.as_deref_mut() {
        if let Err(err) = sync_audio_runtime(app, engine, out, hooks) {
            record_err(err);
        }
    }

    hooks.handle_midi(app, out);

    if let Some(export) = &out.export {
        match execute_export_request(app, export) {
            Ok(summary) => app.record_export_summary(summary),
            Err(err) => {
                app.status = format!("Export failed: {err:#}");
                record_err(err);
            }
        }
    }

    if let Some(bounce) = &out.bounce {
        match execute_bounce_request(app, bounce) {
            Ok(follow_up) => {
                if let Err(err) =
                    apply_outcome_inner(app, audio.as_deref_mut(), &follow_up, hooks, depth + 1)
                {
                    app.status = format!("Bounce audio sync failed: {err:#}");
                    record_err(err);
                }
            }
            Err(err) => {
                app.status = format!("Bounce failed: {err:#}");
                record_err(err);
            }
        }
    }

    if let Some(dialog_req) = &out.file_dialog {
        if hooks.supports_file_dialogs() {
            if let Some(path) = hooks.open_file_dialog(dialog_req) {
                match app.apply_file_dialog_result(dialog_req.clone(), path) {
                    Ok(follow_up) => {
                        if let Err(err) =
                            apply_outcome_inner(app, audio, &follow_up, hooks, depth + 1)
                        {
                            app.status = format!("File action error: {err:#}");
                            record_err(err);
                        }
                    }
                    Err(err) => {
                        app.status = format!("File error: {err:#}");
                        record_err(err);
                    }
                }
            } else {
                app.status = "File dialog cancelled".into();
            }
        } else {
            let err = anyhow!("Interactive file dialog is unavailable in headless mode");
            app.status = format!("{err:#}");
            record_err(err);
        }
    }

    hooks.on_state_synced(app);

    if let Some(err) = first_error {
        Err(err)
    } else {
        Ok(())
    }
}

/// ONE authoritative host `Outcome` dispatcher used by GUI (`EngineBridge`),
/// `--script`, `--stdio`, AND live `--ctl`.
pub fn apply_outcome_with_hooks(
    app: &mut App,
    audio: Option<&mut AudioRuntime>,
    out: &Outcome,
    hooks: &mut dyn OutcomeHooks,
) -> Result<()> {
    apply_outcome_inner(app, audio, out, hooks, 0)
}

/// Headless convenience wrapper around [`apply_outcome_with_hooks`] (handles audio sync
/// when `audio.is_some()`, plus `Outcome::export` and `Outcome::bounce` even when `audio` is `None`).
pub fn apply_outcome(app: &mut App, audio: Option<&mut AudioRuntime>, out: &Outcome) -> Result<()> {
    apply_outcome_with_hooks(app, audio, out, &mut HeadlessOutcomeHooks)
}

pub(crate) fn settle_browser(app: &mut App, include_preview: bool) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        app.poll_browser_updates()?;
        let pending = if include_preview {
            app.browser_busy()
        } else {
            app.browser_accept_pending()
        };
        if !pending {
            return Ok(());
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "Sample browser preparation exceeded 15 seconds"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

pub(crate) fn replay_view_keys(app: &mut App, script: &str) -> Result<()> {
    for key in parse_key_sequence(script) {
        settle_browser(app, false)?;
        app.handle_script_key(key)?;
        settle_browser(app, false)?;
    }
    settle_browser(app, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadence_app::{Action, ExportMode, ExportRequest};
    use cadence_core::material::{
        Edit, InstrumentInstance, Lane, Part, Pattern, PatternKind, Placement, Project,
    };
    use cadence_core::mixer::{Channel, ChannelKind};
    use cadence_core::music::Step;
    use cadence_core::rack::RackPatch;
    use cadence_core::sampler::{write_wav_with_metadata, SampleLibrary, SampleMetadata};

    #[test]
    fn gui_take_export_loads_samples_from_earlier_segments_after_sampler_removal() {
        let dir = tempfile::tempdir().unwrap();
        let samples_dir = dir.path().join("samples");
        std::fs::create_dir_all(&samples_dir).unwrap();
        let wav_path = samples_dir.join("hit.wav");
        let left: Vec<f32> = (0..1024).map(|i| ((i as f32) * 0.15).sin() * 0.8).collect();
        write_wav_with_metadata(&wav_path, &left, None, 8_000, &SampleMetadata::default()).unwrap();

        let mut proj = Project::empty();
        proj.bpm = 120.0;
        proj.bars = 2;
        proj.mixer
            .channels
            .push(Channel::new(1, ChannelKind::Audio, "Smp"));
        let mut rack = RackPatch::default_sampler();
        if let Some((_, _, smp)) = rack.sampler_modules_mut().next() {
            smp.path = Some("./samples/hit.wav".into());
        }
        proj.instruments.push(InstrumentInstance {
            id: 1,
            name: "HitSampler".into(),
            channel: 1,
            rack,
        });
        let mut steps = vec![Step::default(); 16];
        for i in (0..16).step_by(4) {
            steps[i] = Step {
                active: true,
                degree: 1,
                velocity: 0.9,
                gate: 1.0,
                ..Step::default()
            };
        }
        proj.patterns.push(Pattern {
            id: 1,
            name: "Beat".into(),
            kind: PatternKind::Notes { steps },
        });
        proj.parts.push(Part::new(1, "BeatPart", 1, 1));
        let mut lane = Lane::new(1, "Hits");
        lane.placements.push(Placement::new(1, 1, 0, 32));
        proj.lanes.push(lane);

        let project_path = dir.path().join("song.json");
        let library = SampleLibrary::from_roots_only(&[], Some(&project_path));
        let mut app = App::new_with_library(proj, project_path, library).unwrap();
        assert!(!app.sample_bank().is_empty());

        // Arm live take recording, play bar 1 with the Sampler active, then remove the
        // Sampler instrument mid-take so current `app.sample_bank()` becomes empty!
        app.apply(Action::SetTakeArmed { armed: true }).unwrap();
        app.apply(Action::Play { playing: true }).unwrap();
        app.advance(16);
        app.apply(Action::Edit {
            edit: Edit::RemoveLane { id: 1 },
        })
        .unwrap();
        app.apply(Action::Edit {
            edit: Edit::RemovePart { id: 1 },
        })
        .unwrap();
        app.apply(Action::Edit {
            edit: Edit::RemoveInstrument { id: 1 },
        })
        .unwrap();
        assert!(
            app.sample_bank().is_empty(),
            "current project should no longer hold the removed sampler"
        );
        app.advance(16);
        app.apply(Action::Play { playing: false }).unwrap();
        assert!(app.current_take().is_some());

        // Interactive GUI Take Export must load the removed sampler from segment 0 cleanly!
        let export_wav = dir.path().join("exports/take-export.wav");
        let summary = execute_export_request(
            &mut app,
            &ExportRequest {
                path: export_wav.clone(),
                bars: 2,
                start_step: 0,
                steps: 32,
                sample_rate: 8_000,
                mode: ExportMode::MasterWav,
                use_take: true,
            },
        )
        .unwrap();
        assert_eq!(summary.files_written, 1);
        assert!(export_wav.is_file());
        let mut reader = hound::WavReader::open(&export_wav).unwrap();
        let peak = reader
            .samples::<i16>()
            .map(|s| s.unwrap().unsigned_abs())
            .max()
            .unwrap_or(0);
        assert!(
            peak > 1000,
            "GUI exported take must render audible samples from earlier segment (peak={peak})"
        );
    }
}
