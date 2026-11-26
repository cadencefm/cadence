use crate::common::{
    build_snapshot, frame_diff_ratio, render_checked, with_gpu, write_test_wav, TempTestDir,
    DEFAULT_H, DEFAULT_W, STATUS,
};
use cadence_app::host_io::{HostIo, MemoryHostIo};
use cadence_app::view::{DeckView, HelpTab, OverlayView, PatternContentView, UiModeKind};
use cadence_app::{
    Action, App, ControlTarget, ExportSummary, FileHubTab, KeyCode, KeyEvent, KeyModifiers,
};
use cadence_core::config::ThemeId;
use cadence_core::material::{Boundary, Project};
use cadence_core::params::ParamAddress;
use cadence_core::presets::{InstrumentPreset, PresetCatalog};
use cadence_core::rack::{ModuleKind, ModuleParam, RackPatch};
use cadence_core::runtime::AudioSnapshot;
use cadence_core::sampler::SampleLibrary;
use cadence_gui::decks::build_display_list;
use cadence_gui::theme::Theme;
use std::path::PathBuf;
use std::sync::Arc;

fn send_key(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::new(code, KeyModifiers::NONE))
        .unwrap();
}

#[test]
fn gpu_renders_modeless_patterns_jam_and_editing_overlays() {
    with_gpu(DEFAULT_W, DEFAULT_H, |gpu| {
        let dark = Theme::cadence_dark();
        let light = Theme::studio_light();

        let mut app =
            App::new_in_memory(Project::demo(), PathBuf::from("patterns_overlays.json")).unwrap();
        app.execute_command("section Intro 1 4").unwrap();
        app.execute_command("section Drop 5 4").unwrap();
        app.execute_command("mini AcidRiff 16 c4 [e4 g4] ~ [b4 c5] g4 ~ e4 d4")
            .unwrap();
        let snap = build_snapshot(app.project(), 18);

        // 1. Polyphonic Pattern chords (`o` stays in NORMAL, `Alt+L` lengthens Voice 2/3, `i` Step Insert)
        app.execute_command("pattern notes ChordVisual 0 . 2 . 4 . 7 . 0 . 3 . 5 . 7 .")
            .unwrap();
        app.execute_command("open ChordVisual").unwrap();
        app.press_keys("gp").unwrap();
        app.press_keys("ooj<m-L><m-L>").unwrap();
        let fv_pat = app.frame_view(&snap, STATUS);
        assert_eq!(fv_pat.chrome.mode_kind, UiModeKind::Normal);
        let pat_ok = fv_pat.decks.iter().any(|d| match &d.view {
            DeckView::Pattern(pv) => {
                let readout_ok = pv.voice_readout.as_deref().unwrap_or("").contains("2/3");
                let single_selected = match &pv.content {
                    PatternContentView::Notes { steps, .. } => {
                        steps[0].voices.len() == 3
                            && steps[0].voices.iter().filter(|v| v.selected).count() == 1
                            && steps[0].voices[1].selected
                    }
                    _ => false,
                };
                readout_ok && single_selected
            }
            _ => false,
        });
        assert!(pat_ok, "Pattern view must highlight ONLY Voice 2/3");
        let px_pat_dark = render_checked(gpu, "pat-01-chords-dark.png", &fv_pat, &dark, 0.20);
        render_checked(gpu, "pat-02-chords-light.png", &fv_pat, &light, 0.20);

        app.press_keys("i").unwrap();
        let px_ins = render_checked(
            gpu,
            "pat-03-step-insert-dark.png",
            &app.frame_view(&snap, STATUS),
            &dark,
            0.20,
        );
        assert!(frame_diff_ratio(&px_pat_dark, &px_ins) > 0.001);
        send_key(&mut app, KeyCode::Esc);

        // 2. Values Pattern Deck: literal `VALUES` vs derived `DERIVED` pill
        app.execute_command("pattern values SweepCurve 16 ramp 0:0.15 8:0.85 12:0.45")
            .unwrap();
        app.execute_command("open SweepCurve").unwrap();
        app.press_keys("gpzz").unwrap();
        let fv_val_lit = app.frame_view(&snap, STATUS);
        assert!(fv_val_lit.decks.iter().any(|d| matches!(
            &d.view,
            DeckView::Pattern(pv)
                if pv.editable_literal && matches!(pv.content, PatternContentView::Values { .. })
        )));
        let px_val_lit = render_checked(gpu, "pat-04-values-literal.png", &fv_val_lit, &dark, 0.20);

        app.execute_command("derive ShiftedSweep shift SweepCurve 4")
            .unwrap();
        app.execute_command("open ShiftedSweep").unwrap();
        let fv_val_drv = app.frame_view(&snap, STATUS);
        assert!(fv_val_drv.decks.iter().any(|d| matches!(
            &d.view,
            DeckView::Pattern(pv)
                if !pv.editable_literal && matches!(pv.content, PatternContentView::Values { .. })
        )));
        let px_val_drv = render_checked(gpu, "pat-05-values-derived.png", &fv_val_drv, &dark, 0.20);
        assert!(frame_diff_ratio(&px_val_lit, &px_val_drv) > 0.002);
        app.press_keys("zz").unwrap();

        // 3. JAM mode electric violet HUD (`gj`, Dark & Light)
        app.press_keys("gj").unwrap();
        let fv_jam = app.frame_view(&snap, STATUS);
        assert_eq!(fv_jam.chrome.mode_kind, UiModeKind::Jam);
        let px_jam_dark = render_checked(gpu, "pat-06-jam-violet-dark.png", &fv_jam, &dark, 0.20);
        let px_jam_light =
            render_checked(gpu, "pat-07-jam-violet-light.png", &fv_jam, &light, 0.20);
        assert_ne!(px_jam_dark, px_jam_light);
        send_key(&mut app, KeyCode::Esc);

        // 4. Song Properties modal (`gP`): normal vs direct Scale cycle (Dark & Light)
        app.press_keys("gP").unwrap();
        assert!(matches!(
            app.frame_view(&snap, STATUS).overlay,
            Some(OverlayView::SongProperties(_))
        ));
        let px_sp_norm = render_checked(
            gpu,
            "ov-01-song-props-normal.png",
            &app.frame_view(&snap, STATUS),
            &dark,
            0.15,
        );
        app.press_keys("jjj<m-l>").unwrap();
        let fv_sp_edit = app.frame_view(&snap, STATUS);
        let px_sp_dark = render_checked(
            gpu,
            "ov-02-song-props-edit-dark.png",
            &fv_sp_edit,
            &dark,
            0.15,
        );
        let mut fv_sp_light = fv_sp_edit;
        fv_sp_light.chrome.theme = ThemeId::StudioLight;
        let px_sp_light = render_checked(
            gpu,
            "ov-03-song-props-edit-light.png",
            &fv_sp_light,
            &light,
            0.15,
        );
        assert!(frame_diff_ratio(&px_sp_norm, &px_sp_dark) > 0.005);
        assert!(frame_diff_ratio(&px_sp_dark, &px_sp_light) > 0.50);
        send_key(&mut app, KeyCode::Esc);

        // 5. Mini-Notation Editor (`Mode::MiniEditor`): Valid (Dark & Light) vs Syntax Error
        app.execute_command("open AcidRiff").unwrap();
        app.press_keys("gp<Enter>").unwrap();
        let fv_mini_ok = app.frame_view(&snap, STATUS);
        assert!(matches!(
            fv_mini_ok.overlay,
            Some(OverlayView::MiniEditor(ref m)) if m.valid
        ));
        let px_mini_dark = render_checked(
            gpu,
            "ov-04-mini-editor-valid-dark.png",
            &fv_mini_ok,
            &dark,
            0.15,
        );
        let mut fv_mini_light = fv_mini_ok;
        fv_mini_light.chrome.theme = ThemeId::StudioLight;
        render_checked(
            gpu,
            "ov-05-mini-editor-valid-light.png",
            &fv_mini_light,
            &light,
            0.15,
        );
        app.press_keys(" [c5").unwrap();
        let fv_mini_err = app.frame_view(&snap, STATUS);
        assert!(matches!(
            fv_mini_err.overlay,
            Some(OverlayView::MiniEditor(ref m)) if !m.valid
        ));
        let px_mini_err = render_checked(
            gpu,
            "ov-06-mini-editor-error-dark.png",
            &fv_mini_err,
            &dark,
            0.15,
        );
        assert!(frame_diff_ratio(&px_mini_dark, &px_mini_err) > 0.01);
        send_key(&mut app, KeyCode::Esc);

        // 6. Fuzzy Action Palette (`Ctrl+Shift+P`): empty, fuzzy filtered (Dark & Light), CMD passthrough
        app.handle_key(KeyEvent::new(
            KeyCode::Char('P'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ))
        .unwrap();
        let px_pal_empty = render_checked(
            gpu,
            "ov-07-palette-empty.png",
            &app.frame_view(&snap, STATUS),
            &dark,
            0.15,
        );
        app.press_keys("song key").unwrap();
        let fv_pal_fuzzy = app.frame_view(&snap, STATUS);
        let px_pal_fuzzy = render_checked(
            gpu,
            "ov-08-palette-fuzzy-dark.png",
            &fv_pal_fuzzy,
            &dark,
            0.15,
        );
        render_checked(
            gpu,
            "ov-09-palette-fuzzy-light.png",
            &fv_pal_fuzzy,
            &light,
            0.15,
        );
        app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL))
            .unwrap();
        app.press_keys("bpm 136").unwrap();
        let px_pal_cmd = render_checked(
            gpu,
            "ov-10-palette-cmd.png",
            &app.frame_view(&snap, STATUS),
            &dark,
            0.15,
        );
        assert!(frame_diff_ratio(&px_pal_empty, &px_pal_fuzzy) > 0.01);
        assert!(frame_diff_ratio(&px_pal_fuzzy, &px_pal_cmd) > 0.01);
        send_key(&mut app, KeyCode::Esc);

        // 7. ValueInput popover on Instrument Cutoff with live preview (Dark & Light)
        let inst_name = app.project().instruments[0].name.clone();
        app.execute_command(&format!("instrument {inst_name}"))
            .unwrap();
        let cutoff_idx = app
            .control_descriptors()
            .iter()
            .position(|c| {
                matches!(
                    c.target,
                    ControlTarget::Param(ParamAddress::Module {
                        param: ModuleParam::Cutoff,
                        ..
                    })
                )
            })
            .expect("Cutoff control");
        app.control_cursor = cutoff_idx;
        app.press_keys("<Enter>1.75khz").unwrap();
        let fv_vi = app.frame_view(&snap, STATUS);
        assert!(matches!(
            fv_vi.overlay,
            Some(OverlayView::ValueInput(ref v)) if v.live_preview.is_some()
        ));
        let px_vi_dark = render_checked(gpu, "ov-11-value-input-dark.png", &fv_vi, &dark, 0.15);
        let px_vi_light = render_checked(gpu, "ov-12-value-input-light.png", &fv_vi, &light, 0.15);
        assert!(frame_diff_ratio(&px_vi_dark, &px_vi_light) > 0.50);
        send_key(&mut app, KeyCode::Esc);

        // 8. Picker active searching vs committed filter + LiveTiming::Cycle Chrome pill
        let pat_id = app.project().patterns[0].id;
        app.press_keys("tQ/bar").unwrap();
        let fv_pick_search = app.frame_view(&snap, STATUS);
        assert!(matches!(
            fv_pick_search.overlay,
            Some(OverlayView::Picker(ref p)) if p.searching && p.filter.as_deref() == Some("bar")
        ));
        let px_pick_search = render_checked(
            gpu,
            "ov-13-picker-searching.png",
            &fv_pick_search,
            &dark,
            0.15,
        );
        send_key(&mut app, KeyCode::Enter);
        let fv_pick_commit = app.frame_view(&snap, STATUS);
        assert!(matches!(
            fv_pick_commit.overlay,
            Some(OverlayView::Picker(ref p)) if !p.searching && p.filter.as_deref() == Some("bar")
        ));
        let px_pick_commit = render_checked(
            gpu,
            "ov-14-picker-committed.png",
            &fv_pick_commit,
            &dark,
            0.15,
        );
        assert!(frame_diff_ratio(&px_pick_search, &px_pick_commit) > 0.0002);
        send_key(&mut app, KeyCode::Esc);
        send_key(&mut app, KeyCode::Esc);

        app.press_keys("tQ<Enter>tE").unwrap();
        assert_eq!(
            app.project().timing.launch,
            Boundary::Cycle { pattern: pat_id }
        );

        // 9. Pattern Derive 16-ops overlay (with Cycle pill active in Chrome)
        app.press_keys("gpdDeriveTest<Enter>jjjjjj").unwrap();
        render_checked(
            gpu,
            "ov-15-derive-16-ops.png",
            &app.frame_view(&snap, STATUS),
            &dark,
            0.20,
        );
    });
}

#[test]
fn gpu_renders_takes_filehub_settings_help_authoring_and_inspect() {
    with_gpu(DEFAULT_W, DEFAULT_H, |gpu| {
        let dark = Theme::cadence_dark();
        let light = Theme::studio_light();

        // 1. Take Deck (`TAKE ARM`, `● REC TAKE`, `VISUAL` range marking) & Sample Browser search
        let dir = TempTestDir::new("overlays-takes");
        let samples_dir = dir.path().join("samples");
        std::fs::create_dir_all(&samples_dir).unwrap();
        write_test_wav(&samples_dir.join("alpha-kick.wav"), 2400, 48_000);
        write_test_wav(&samples_dir.join("beta-snare.wav"), 2400, 48_000);
        let save_path = dir.path().join("session.json");
        let lib = SampleLibrary::from_roots(&[samples_dir], Some(&save_path));
        let mut take_app = App::new_with_library(Project::demo(), save_path, lib).unwrap();

        take_app.press_keys("trgt").unwrap();
        render_checked(
            gpu,
            "ov-16-take-arm-empty.png",
            &take_app.frame_view(
                &AudioSnapshot {
                    sample_rate: 48_000,
                    ..AudioSnapshot::default()
                },
                STATUS,
            ),
            &dark,
            0.15,
        );

        take_app.press_keys(" ").unwrap();
        take_app.apply(Action::Advance { steps: 16 }).unwrap();
        take_app.execute_command("bpm 142").unwrap();
        take_app.apply(Action::Advance { steps: 64 }).unwrap();
        take_app.press_keys("gtlvll").unwrap();
        render_checked(
            gpu,
            "ov-17-take-rec-visual-mark.png",
            &take_app.frame_view(
                &AudioSnapshot {
                    playing: true,
                    absolute_step: take_app.transport_step(),
                    sample_rate: 48_000,
                    peak: 0.45,
                    peak_left: 0.45,
                    peak_right: 0.42,
                    ..AudioSnapshot::default()
                },
                STATUS,
            ),
            &dark,
            0.15,
        );

        take_app.press_keys("tr<Home>").unwrap();
        take_app.execute_command("sampler DrumBox").unwrap();
        take_app.press_keys("gil").unwrap();
        take_app.control_cursor = 0;
        take_app.press_keys("<Enter>/beta<Enter>").unwrap();
        render_checked(
            gpu,
            "ov-18-sample-browser-search.png",
            &take_app.frame_view(&AudioSnapshot::default(), STATUS),
            &dark,
            0.15,
        );

        // 2. FileHub (Quick, Browse Dark & Light, Import Preset Library, Search, Export summary)
        let presets_dir = TempTestDir::new("filehub-presets");
        let mut synth_patch = RackPatch::default_synth();
        for m in &mut synth_patch.zones[0].modules {
            if let ModuleKind::Filter(f) = &mut m.kind {
                f.cutoff_hz = 1850.0;
            }
        }
        InstrumentPreset::new("Acid Lead", Some("Lead".into()), synth_patch)
            .save(&presets_dir.path().join("acid-lead.preset.json"))
            .unwrap();

        let io = Arc::new(MemoryHostIo::new("/virtual/studio"));
        io.set_presets(PresetCatalog::from_roots(&[presets_dir
            .path()
            .to_path_buf()]));
        io.set_home_dir("/virtual/home");
        io.insert_file(
            "/virtual/studio/session.json",
            b"{\"title\":\"Afterglow\",\"bpm\":128.0,\"bars\":16,\"instruments\":[],\"patterns\":[]}",
        );
        io.insert_file("/virtual/studio/LiveJam.take.jsonl", b"{}\n");
        io.create_dir("/virtual/studio/archive");
        io.insert_file("/virtual/studio/songs/ambient/cathedral.json", b"{}");

        let mut app = App::new_with_host_io(
            Project::demo(),
            "/virtual/studio/session.json".into(),
            Arc::clone(&io) as Arc<dyn HostIo>,
        )
        .unwrap();
        app.reload_presets();
        app.config.recent_projects = vec!["/virtual/studio/session.json".into()];
        let snap = build_snapshot(app.project(), 24);

        app.open_file_hub(FileHubTab::Quick);
        render_checked(
            gpu,
            "ov-19-filehub-quick.png",
            &app.frame_view(&snap, STATUS),
            &dark,
            0.20,
        );

        app.press_keys("2").unwrap();
        if let Some(OverlayView::FileHub(fh)) = app.frame_view(&snap, STATUS).overlay {
            if let Some(songs_idx) = fh.rows.iter().position(|r| r.label == "songs/") {
                let cur = fh.cursor;
                if songs_idx > cur {
                    app.press_keys(&"j".repeat(songs_idx - cur)).unwrap();
                } else if cur > songs_idx {
                    app.press_keys(&"k".repeat(cur - songs_idx)).unwrap();
                }
                app.press_keys("<Enter><Backspace>").unwrap();
            }
        }
        let fv_browse = app.frame_view(&snap, STATUS);
        render_checked(
            gpu,
            "ov-20-filehub-browse-dark.png",
            &fv_browse,
            &dark,
            0.20,
        );
        render_checked(
            gpu,
            "ov-21-filehub-browse-light.png",
            &fv_browse,
            &light,
            0.20,
        );

        app.press_keys("/songs<Enter>").unwrap();
        render_checked(
            gpu,
            "ov-22-filehub-search.png",
            &app.frame_view(&snap, STATUS),
            &dark,
            0.15,
        );
        app.press_keys("<Esc><Esc>").unwrap();

        app.open_file_hub(FileHubTab::Import);
        render_checked(
            gpu,
            "ov-23-filehub-import-presets.png",
            &app.frame_view(&snap, STATUS),
            &dark,
            0.20,
        );

        app.press_keys("5<m-l>").unwrap();
        app.record_export_summary(ExportSummary {
            target: "afterglow-stems/".into(),
            files_written: 11,
            duration_sec: 120.0,
            peak_left_db: -0.9,
            peak_right_db: -1.1,
            rms_db: -11.4,
            clips: 0,
            sample_rate: 48_000,
        });
        render_checked(
            gpu,
            "ov-24-filehub-export-summary.png",
            &app.frame_view(&snap, STATUS),
            &dark,
            0.20,
        );
        app.press_keys("<Esc>").unwrap();

        // 3. Settings (`g,`): Key Hints / Monitoring (Dark & Light), Live Timing, Asset Directories editor
        app.config.sample_dirs = vec!["/Users/studio/Samples/Drums".into()];
        app.config.preset_dirs = vec!["/Users/studio/Presets/Analog".into()];
        app.press_keys("g,").unwrap();
        app.settings_ui.cursor = 2;
        let fv_set = app.frame_view(&snap, STATUS);
        assert!(matches!(fv_set.overlay, Some(OverlayView::Settings(_))));
        render_checked(gpu, "ov-25-settings-dark.png", &fv_set, &dark, 0.20);
        render_checked(gpu, "ov-26-settings-light.png", &fv_set, &light, 0.20);

        app.settings_ui.cursor = 15;
        app.press_keys("a/Users/studio/Samples/Pads").unwrap();
        render_checked(
            gpu,
            "ov-27-settings-asset-dirs.png",
            &app.frame_view(&snap, STATUS),
            &dark,
            0.20,
        );
        app.press_keys("<Esc><Esc>").unwrap();

        // 4. Compact `1440×460` Authoring draft exercising left field-list scroll + `▲/▼`
        gpu.resize(1440, 460);
        app.press_keys("np<Enter><Enter><Enter><Enter><Enter><Enter><Enter>")
            .unwrap();
        render_checked(
            gpu,
            "ov-28-authoring-left-scroll-1440x460.png",
            &app.frame_view(&snap, STATUS),
            &dark,
            0.20,
        );
        app.press_keys("<Esc>").unwrap();

        // 5. Wrapped `:assets` Inspect overlay on a narrow `1024×680` viewport
        gpu.resize(1024, 680);
        take_app.press_keys("<Esc><Esc>").unwrap();
        take_app.execute_command("assets").unwrap();
        let fv_inspect = take_app.frame_view(&snap, STATUS);
        assert!(matches!(
            fv_inspect.overlay,
            Some(OverlayView::Inspect { .. })
        ));
        render_checked(
            gpu,
            "ov-29-inspect-wrapped-1024x680.png",
            &fv_inspect,
            &dark,
            0.15,
        );

        // 6. `?` Help overlay: zero-ellipsis & zero-missing-glyph across ALL tabs at 1440×860 & 1100×720
        gpu.resize(DEFAULT_W, DEFAULT_H);
        app.press_keys("?").unwrap();
        for tab_idx in 0..HelpTab::ALL.len() {
            let fv = app.frame_view(&snap, STATUS);
            assert!(
                matches!(fv.overlay, Some(OverlayView::Help(_))),
                "expected Help overlay on tab {tab_idx}"
            );
            for (w, h) in [(1440.0_f32, 860.0_f32), (1100.0_f32, 720.0_f32)] {
                let dl = build_display_list(&fv, w, h, 1.0, &dark);
                assert_eq!(
                    gpu.first_missing_glyph(&dl, &dark),
                    None,
                    "Help tab {tab_idx} emitted a missing glyph at {w}x{h}"
                );
                for run in &dl.overlay.texts {
                    assert!(
                        !run.text.contains('…'),
                        "Help tab {tab_idx} truncated with ellipsis at {w}x{h}: {:?}",
                        run.text
                    );
                }
            }
            app.press_keys("l").unwrap();
        }

        // Scroll `All` tab to confirm variable-height scroll window changes frame
        let px_all_top = render_checked(
            gpu,
            "ov-30-help-all-top.png",
            &app.frame_view(&snap, STATUS),
            &dark,
            0.25,
        );
        app.press_keys("jjjjjjjjjjjjjjjjjjjjjjjjjjjjjj").unwrap();
        let px_all_scrolled = render_checked(
            gpu,
            "ov-31-help-all-scrolled.png",
            &app.frame_view(&snap, STATUS),
            &dark,
            0.25,
        );
        assert_ne!(px_all_top, px_all_scrolled);

        // `Navigate` tab (Dark & Light)
        app.press_keys("l").unwrap();
        let fv_nav = app.frame_view(&snap, STATUS);
        let px_nav_dark = render_checked(gpu, "ov-32-help-navigate-dark.png", &fv_nav, &dark, 0.25);
        let px_nav_light =
            render_checked(gpu, "ov-33-help-navigate-light.png", &fv_nav, &light, 0.25);
        assert_ne!(px_nav_dark, px_nav_light);

        // `Sound & FX` tab at `1100×720` (Dark & Light via HeadlessGpu::resize)
        app.press_keys("lll").unwrap();
        gpu.resize(1100, 720);
        let fv_sfx = app.frame_view(&snap, STATUS);
        let px_sfx_dark = render_checked(
            gpu,
            "ov-34-help-soundfx-1100x720-dark.png",
            &fv_sfx,
            &dark,
            0.25,
        );
        let px_sfx_light = render_checked(
            gpu,
            "ov-35-help-soundfx-1100x720-light.png",
            &fv_sfx,
            &light,
            0.25,
        );
        assert_ne!(px_sfx_dark, px_sfx_light);
    });
}
