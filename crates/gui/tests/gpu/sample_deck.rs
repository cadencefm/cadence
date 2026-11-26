use crate::common::{render_checked, with_gpu, TempTestDir, DEFAULT_H, DEFAULT_W, STATUS};
use cadence_app::layout::DeckKind;
use cadence_app::view::DeckView;
use cadence_app::App;
use cadence_core::dsp::LivePeakRecorder;
use cadence_core::material::Project;
use cadence_core::rack::SampleLoopMode;
use cadence_core::runtime::{
    AudioInputMode, AudioInputStatus, AudioInputTelemetry, AudioSnapshot, CapturePhase,
    InputMonitorMode,
};
use cadence_core::sampler::{write_wav_with_metadata, SampleLibrary, SampleMetadata, SampleSlice};
use cadence_gui::theme::Theme;
use std::path::Path;

fn write_visual_break_wav(path: &Path) {
    let sr = 44_100u32;
    let frames = sr as usize * 2;
    let mut left = vec![0.0f32; frames];
    let mut right = vec![0.0f32; frames];
    let hits = [
        (0usize, "Kick", 58.0f32, 0.92f32, false),
        (frames / 8, "Hat 1", 4200.0, 0.42, true),
        (frames / 4, "Snare", 220.0, 0.88, true),
        ((frames * 3) / 8, "Ghost", 310.0, 0.35, true),
        (frames / 2, "Kick 2", 58.0, 0.90, false),
        ((frames * 5) / 8, "Open Hat", 5100.0, 0.55, true),
        ((frames * 3) / 4, "Snare 2", 215.0, 0.86, true),
        ((frames * 7) / 8, "Perc", 780.0, 0.65, false),
    ];

    for (idx, &(start, _, freq, amp, noisy)) in hits.iter().enumerate() {
        let dur = (frames / 8).min(frames - start);
        for i in 0..dur {
            let t = i as f32 / sr as f32;
            let env = (-t * if noisy { 18.0 } else { 14.0 }).exp();
            let f = if idx == 0 || idx == 4 {
                freq * (1.0 + 2.4 * (-t * 42.0).exp())
            } else {
                freq
            };
            let osc = (std::f32::consts::TAU * f * t).sin();
            let noise =
                ((i.wrapping_mul(1664525).wrapping_add(1013904223) >> 16) as f32 / 32768.0) - 1.0;
            let sample = if noisy {
                (osc * 0.4 + noise * 0.6) * env * amp
            } else {
                osc * env * amp
            };
            left[start + i] = sample;
            right[start + i] = sample * if idx % 2 == 0 { 0.94 } else { 1.0 };
        }
    }

    let meta = SampleMetadata {
        root_midi: 60,
        has_root: true,
        loop_mode: SampleLoopMode::Forward,
        loop_start: frames / 4,
        loop_end: (frames * 3) / 4,
        slices: hits
            .iter()
            .map(|&(start, lbl, _, _, _)| SampleSlice::new(start, Some(lbl.into())))
            .collect(),
        ..SampleMetadata::default()
    };

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    write_wav_with_metadata(path, &left, Some(&right), sr, &meta).unwrap();
}

#[test]
fn gpu_renders_sample_waveform_deck_dark_light_zoomed_and_empty() {
    with_gpu(DEFAULT_W, DEFAULT_H, |gpu| {
        let tmp = TempTestDir::new("sample-deck");
        let save_path = tmp.path().join("project.json");
        let wav_path = tmp.path().join("samples/funk_break_120.wav");
        write_visual_break_wav(&wav_path);

        let lib = SampleLibrary::from_roots_only(&[tmp.path().join("samples")], Some(&save_path));
        let mut app = App::new_with_library(Project::demo(), save_path, lib).unwrap();
        app.execute_command("sampler BreakLoop samples/funk_break_120.wav")
            .unwrap();
        assert!(app.ready());

        app.press_keys("gw0ww").unwrap();
        assert_eq!(app.focused_deck_kind(), Some(DeckKind::Sample));

        let snap = AudioSnapshot {
            playing: false,
            sample_rate: 48_000,
            ..AudioSnapshot::default()
        };
        let dark = Theme::cadence_dark();
        let light = Theme::studio_light();

        // 1. Split-pane Dark
        let fv_split = app.frame_view(&snap, STATUS);
        let sv = fv_split
            .decks
            .iter()
            .find_map(|d| match &d.view {
                DeckView::Sample(s) => Some(s),
                _ => None,
            })
            .expect("SampleDeckView in split pane");
        assert!(sv.loaded);
        assert_eq!(sv.slices.len(), 8);
        assert!(sv.has_riff_cues);
        assert_eq!(sv.active_slice_idx, Some(2));
        assert!(!sv.peaks.is_empty());
        render_checked(gpu, "sample-01-split-dark.png", &fv_split, &dark, 0.20);

        // 2. Zoomed deck + horizontal zoom + cropped end (Dark & Light)
        app.press_keys("zz==$h]0ww").unwrap();
        let fv_zoomed = app.frame_view(&snap, STATUS);
        let sv_z = fv_zoomed
            .decks
            .iter()
            .find_map(|d| match &d.view {
                DeckView::Sample(s) => Some(s),
                _ => None,
            })
            .expect("zoomed SampleDeckView");
        assert!(sv_z.zoom > 1.5);
        assert!(sv_z.sample_end < sv_z.total_frames);
        let px_dark = render_checked(gpu, "sample-02-zoomed-dark.png", &fv_zoomed, &dark, 0.20);
        let px_light = render_checked(gpu, "sample-03-zoomed-light.png", &fv_zoomed, &light, 0.20);
        assert_ne!(px_dark, px_light);

        // 3. Empty Sampler state
        app.execute_command("sampler BlankSlot blank").unwrap();
        app.press_keys("gw").unwrap();
        let fv_empty = app.frame_view(&snap, STATUS);
        let sv_empty = fv_empty
            .decks
            .iter()
            .find_map(|d| match &d.view {
                DeckView::Sample(s) => Some(s),
                _ => None,
            })
            .expect("empty SampleDeckView");
        assert!(!sv_empty.loaded);
        render_checked(gpu, "sample-04-empty-dark.png", &fv_empty, &dark, 0.10);
    });
}

#[test]
fn gpu_renders_sample_deck_live_recording_monitor_and_offline_edit_picker() {
    with_gpu(DEFAULT_W, DEFAULT_H, |gpu| {
        let tmp = TempTestDir::new("sample-deck-rec");
        let save_path = tmp.path().join("project.json");
        let wav_path = tmp.path().join("samples/funk_break_120.wav");
        write_visual_break_wav(&wav_path);

        let lib = SampleLibrary::from_roots_only(&[tmp.path().join("samples")], Some(&save_path));
        let mut app = App::new_with_library(Project::demo(), save_path, lib).unwrap();
        app.config.audio_input_device = Some("Focusrite Scarlett 4i4".into());
        app.config.audio_input_monitor = InputMonitorMode::Auto;
        app.execute_command("sampler BreakLoop samples/funk_break_120.wav")
            .unwrap();
        app.press_keys("gwzz").unwrap();

        let dark = Theme::cadence_dark();
        let light = Theme::studio_light();

        // 1. Offline Sample Edit picker (`e`)
        let base_snap = AudioSnapshot {
            sample_rate: 48_000,
            ..AudioSnapshot::default()
        };
        app.press_keys("e").unwrap();
        let fv_picker = app.frame_view(&base_snap, STATUS);
        assert!(fv_picker.overlay.is_some());
        render_checked(
            gpu,
            "sample-05-edit-picker-dark.png",
            &fv_picker,
            &dark,
            0.20,
        );
        app.press_keys("<Esc>").unwrap();

        // 2. Armed Live Recording Monitor (oscilloscope + L/R meters)
        app.press_keys("ta").unwrap();
        let live_scope: Vec<f32> = (0..256)
            .map(|i| {
                let t = i as f32 / 256.0;
                (std::f32::consts::TAU * 3.0 * t).sin() * 0.52
                    + (std::f32::consts::TAU * 11.0 * t).sin() * 0.22
            })
            .collect();
        let mut rec_snap = AudioSnapshot {
            playing: true,
            sample_rate: 48_000,
            input: AudioInputTelemetry {
                status: AudioInputStatus::Running,
                sample_rate: 48_000,
                channels: 2,
                buffer_frames: 256,
                peak: [0.58, 0.49],
                peak_hold: [0.74, 0.66],
                clipped: false,
                digital_silence: false,
                mode: AudioInputMode::Stereo,
                monitor_mode: InputMonitorMode::Auto,
                monitoring: true,
                capture_phase: CapturePhase::Armed,
                capture_start_step: 16,
                capture_end_step: 48,
                captured_frames: 0,
                dropped_frames: 0,
            },
            input_waveform: live_scope,
            ..AudioSnapshot::default()
        };
        render_checked(
            gpu,
            "sample-06-armed-dark.png",
            &app.frame_view(&rec_snap, STATUS),
            &dark,
            0.15,
        );

        // 3. Active Recording Monitor (growing crimson LivePeakRecorder envelope + inset LIVE IN scope)
        rec_snap.input.capture_phase = CapturePhase::Recording;
        rec_snap.input.captured_frames = 115_200;
        rec_snap.input.peak = [0.76, 0.68];
        rec_snap.input.peak_hold = [0.88, 0.81];
        app.observe_audio_recording(&rec_snap);

        let mut tracker = LivePeakRecorder::new(512, 128);
        let synth_take: Vec<[f32; 2]> = (0..115_200)
            .map(|i| {
                let t = i as f32 / 48_000.0;
                let beat = (t * 2.0).fract();
                let env = (-beat * 8.5).exp() * 0.82 + 0.12;
                let sig = (std::f32::consts::TAU * 140.0 * t).sin() * env;
                [sig, sig * 0.9]
            })
            .collect();
        tracker.push_stereo(&synth_take);
        app.set_recording_preview_peaks(tracker.snapshot_peaks());

        let fv_rec = app.frame_view(&rec_snap, STATUS);
        let sv_rec = fv_rec
            .decks
            .iter()
            .find_map(|d| match &d.view {
                DeckView::Sample(s) => Some(s),
                _ => None,
            })
            .expect("SampleDeckView while recording");
        assert!(sv_rec
            .record_monitor
            .as_ref()
            .is_some_and(|m| !m.recording_peaks.is_empty()));

        let px_rec_dark = render_checked(gpu, "sample-07-recording-dark.png", &fv_rec, &dark, 0.18);
        let px_rec_light =
            render_checked(gpu, "sample-08-recording-light.png", &fv_rec, &light, 0.18);
        assert_ne!(px_rec_dark, px_rec_light);
    });
}
