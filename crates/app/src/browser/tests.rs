use super::*;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

fn fixture() -> (tempfile::TempDir, App) {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("tone.wav");
    let mut wave = hound::WavWriter::create(
        path,
        hound::WavSpec {
            channels: 1,
            sample_rate: 8_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )
    .unwrap();
    for _ in 0..80 {
        wave.write_sample(1_000_i16).unwrap();
    }
    wave.finalize().unwrap();
    let project = folder.path().join("project.json");
    let library = SampleLibrary::from_roots_only(&[folder.path().into()], Some(&project));
    let app = App::new_with_library(Project::empty(), project, library).unwrap();
    (folder, app)
}

fn settle(app: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let out = app.poll_browser_updates().unwrap();
        assert!(
            out.preview_sample.is_none(),
            "failed intent must not resume"
        );
        if !app.browser_busy() {
            return;
        }
        assert!(Instant::now() < deadline, "browser worker did not settle");
        thread::sleep(Duration::from_millis(2));
    }
}

fn assert_failed_without_edit(app: &App, text: &str) {
    assert!(app.is_sample_browser());
    assert!(!app.browser_busy());
    assert!(!app.browser_accept_pending());
    let state = app.modal.browser().unwrap();
    assert!(!state.audition_when_ready);
    assert!(state.sample.is_none());
    assert!(state
        .error
        .as_ref()
        .is_some_and(|error| error.contains(text)));
    assert_eq!(app.revision(), 0);
    assert!(app.project().instruments.is_empty());
}

#[test]
fn decoder_panic_clears_pending_intents_and_worker_can_load_next_request() {
    let (_folder, mut app) = fixture();
    let panic_once = AtomicBool::new(true);
    app.assets.browser_loader = Some(
        BrowserLoader::with_decoder(move |job| {
            assert!(
                !panic_once.swap(false, Ordering::Relaxed),
                "injected decoder panic"
            );
            cadence_core::sampler::load_sample_file(&job.path, Some(job.label.clone()))
                .map_err(|error| format!("{error:#}"))
        })
        .unwrap(),
    );
    app.begin_sample_browser(BrowserTarget::NewSampler).unwrap();
    app.press_keys("p<Enter>").unwrap();
    settle(&mut app);
    assert_failed_without_edit(&app, "failed unexpectedly");
    app.press_keys("r").unwrap();
    settle(&mut app);
    assert!(app.browser_view().unwrap().sample.is_some());
    assert!(app.browser_view().unwrap().error.is_none());
    assert_eq!(app.revision(), 0);
}

#[test]
fn disconnected_submitted_worker_clears_acceptance_and_retry_recreates_worker() {
    let (_folder, mut app) = fixture();
    let (tx, jobs) = mpsc::sync_channel(1);
    let (results, rx) = mpsc::sync_channel(1);
    app.assets.browser_loader = Some(BrowserLoader {
        tx,
        rx,
        active: Arc::new(AtomicU64::new(0)),
    });
    app.begin_sample_browser(BrowserTarget::NewSampler).unwrap();
    app.press_keys("p<Enter>").unwrap();
    assert!(app.modal.browser().unwrap().submitted);
    drop(jobs);
    drop(results);
    app.poll_browser_updates().unwrap();
    assert_failed_without_edit(&app, "disconnected");
    assert!(app.assets.browser_loader.is_none());
    app.press_keys("r").unwrap();
    settle(&mut app);
    assert!(app.browser_view().unwrap().sample.is_some());
    assert!(app.browser_view().unwrap().error.is_none());
}

#[test]
fn disconnected_enqueue_recovers_when_browser_reopens() {
    let (_folder, mut app) = fixture();
    let (tx, jobs) = mpsc::sync_channel(1);
    let (results, rx) = mpsc::sync_channel(1);
    drop(jobs);
    drop(results);
    app.assets.browser_loader = Some(BrowserLoader {
        tx,
        rx,
        active: Arc::new(AtomicU64::new(0)),
    });
    app.begin_sample_browser(BrowserTarget::NewSampler).unwrap();
    assert_failed_without_edit(&app, "disconnected");
    app.press_keys("<Esc>B").unwrap();
    settle(&mut app);
    assert!(app.browser_view().unwrap().sample.is_some());
    assert!(app.browser_view().unwrap().error.is_none());
}
