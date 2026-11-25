use super::*;
use crate::keys::{KeyCode, KeyEvent, KeyModifiers};
use crate::layout::DeckKind;
use cadence_core::material::Project;
use cadence_core::mixer::ChannelRef;
use cadence_core::runtime::{AudioSnapshot, ChannelMeter};

fn demo_app() -> App {
    App::new(Project::demo(), "demo.json".into()).unwrap()
}

#[test]
fn frame_view_projects_smooth_substep_playhead_and_chrome() {
    let app = demo_app();
    let snap = AudioSnapshot {
        playing: true,
        absolute_step: 20,
        step_fraction: 0.625,
        sample_rate: 48_000,
        peak_left: 0.5,
        peak_right: 0.25,
        waveform: vec![0.1, -0.2, 0.3],
        channel_meters: vec![ChannelMeter {
            channel: ChannelRef::Master,
            input_peak: [0.55, 0.28],
            input_peak_hold: [0.65, 0.35],
            output_peak: [0.5, 0.25],
            output_peak_hold: [0.6, 0.3],
            hot: false,
            clipped: false,
        }],
        ..AudioSnapshot::default()
    };
    let fv = app.frame_view(&snap, "CoreAudio 48 kHz");
    assert!(fv.chrome.playing);
    assert_eq!(fv.chrome.song_step, 20);
    assert!((fv.chrome.song_step_smooth.unwrap() - 20.625).abs() < 1e-4);
    assert_eq!(fv.chrome.bar, 2);
    assert_eq!(fv.chrome.beat, 2);
    assert_eq!(fv.chrome.substep, 1);
    assert!(fv.chrome.output_observed);
    assert!(fv.decks.iter().any(|d| d.kind == DeckKind::Arrange));
    assert!(fv.decks.iter().any(|d| d.kind == DeckKind::Bank));
    assert!(fv.decks.iter().any(|d| d.kind == DeckKind::Output));

    let arrange = fv
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Arrange(a) => Some(a),
            _ => None,
        })
        .expect("arrange deck");
    assert!((arrange.playhead_step.unwrap() - 20.625).abs() < 1e-4);
    assert!(!arrange.lanes.is_empty());
}

#[test]
fn zoom_bindings_adjust_arrange_and_pattern_zoom_with_clamping() {
    let mut app = demo_app();
    assert!((app.arrange_ui.zoom - 1.0).abs() < 1e-6);

    // z= zooms in Arrange, z- zooms out, z0 resets.
    app.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE))
        .unwrap();
    let fv_menu = app.frame_view(&AudioSnapshot::default(), "silent");
    assert!(matches!(
        fv_menu.overlay,
        Some(OverlayView::PrefixMenu { .. })
    ));

    app.handle_key(KeyEvent::new(KeyCode::Char('='), KeyModifiers::NONE))
        .unwrap();
    assert!(app.arrange_ui.zoom > 1.2);

    // Ctrl+- zooms back down, Ctrl+0 resets cleanly.
    app.handle_key(KeyEvent::new(KeyCode::Char('-'), KeyModifiers::CONTROL))
        .unwrap();
    assert!((app.arrange_ui.zoom - 1.0).abs() < 1e-4);

    for _ in 0..20 {
        app.handle_key(KeyEvent::new(KeyCode::Char('='), KeyModifiers::CONTROL))
            .unwrap();
    }
    assert!((app.arrange_ui.zoom - 16.0).abs() < 1e-4);
    assert_eq!(app.arrange_visible_bars(), 1);

    app.handle_key(KeyEvent::new(KeyCode::Char('0'), KeyModifiers::CONTROL))
        .unwrap();
    assert!((app.arrange_ui.zoom - 1.0).abs() < 1e-6);

    // Direct = and - in Arrange Overview & Lane Roll visibly change page_steps on every press!
    app.press_keys("vo").unwrap();
    let snap = AudioSnapshot::default();
    let bars_before = app.arrange_visible_bars();
    app.handle_key(KeyEvent::new(KeyCode::Char('='), KeyModifiers::NONE))
        .unwrap();
    let bars_after = app.arrange_visible_bars();
    assert!(bars_after < bars_before);
    let fv_over = app.frame_view(&snap, "silent");
    let over_steps = fv_over
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Arrange(a) => Some(a.page_steps),
            _ => None,
        })
        .expect("arrange deck");
    assert_eq!(over_steps, bars_after * 16);

    app.press_keys("vr").unwrap();
    app.handle_key(KeyEvent::new(KeyCode::Char('='), KeyModifiers::NONE))
        .unwrap();
    let bars_roll = app.arrange_visible_bars();
    assert!(bars_roll < bars_after);
    let fv_roll = app.frame_view(&snap, "silent");
    let roll_steps = fv_roll
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Arrange(a) => Some(a.page_steps),
            _ => None,
        })
        .expect("arrange deck");
    assert_eq!(roll_steps, bars_roll * 16);

    // Reset Arrange zoom back to 100%, then switch to Pattern deck and confirm Pattern zoom is independent.
    app.handle_key(KeyEvent::new(KeyCode::Char('0'), KeyModifiers::CONTROL))
        .unwrap();
    app.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE))
        .unwrap();
    app.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE))
        .unwrap();
    app.handle_key(KeyEvent::new(KeyCode::Char('='), KeyModifiers::CONTROL))
        .unwrap();
    assert!(app.view_state.pattern_zoom > 1.2);
    assert!((app.arrange_ui.zoom - 1.0).abs() < 1e-6);
}

#[test]
fn palette_quit_tutorial_resume_and_window_state_persistence() {
    let mut app = demo_app();

    // 1. Quit is directly searchable and executable in Action Palette (dirty-checked).
    app.press_keys(":quit cad<Enter>").unwrap();
    // Clean project quits cleanly via Outcome::quit (verify via handle_key directly).
    let mut clean_app = demo_app();
    clean_app.press_keys(":quit cad").unwrap();
    let pv = clean_app.palette_view().expect("palette open");
    assert!(
        pv.items.iter().any(|i| i.label == "Quit Cadence"),
        "Quit Cadence must appear in Action Palette"
    );
    let out = clean_app
        .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
    assert!(out.quit, "clean project quits immediately from Palette");

    // Dirty project prompts Confirm::Quit instead of quitting silently.
    let mut dirty_app = demo_app();
    dirty_app.execute_command("bpm 130").unwrap();
    assert!(dirty_app.dirty());
    dirty_app.press_keys(":quit cad").unwrap();
    let out = dirty_app
        .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
    assert!(!out.quit, "dirty project must confirm before quitting");
    assert!(dirty_app.confirm_prompt().is_some());

    // 2. Tutorial returns cleanly to Mode::Tutorial and advances step after running a lesson.
    let mut tut = App::new(Project::empty(), "tutorial_test.json".into()).unwrap();
    tut.execute_command("tutorial").unwrap();
    assert!(tut.show_tutorial());
    assert_eq!(tut.help_ui.tutorial_step, 0);
    // Enter pre-fills Palette with step 1's command; Esc returns to Tutorial on same step.
    tut.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
    assert!(tut.palette_view().is_some());
    tut.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        .unwrap();
    assert!(tut.show_tutorial());
    assert_eq!(tut.help_ui.tutorial_step, 0);
    // Running step 1 advances cleanly to step 2 while keeping Tutorial open!
    tut.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
    tut.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
    assert!(tut.show_tutorial());
    assert_eq!(tut.help_ui.tutorial_step, 1);

    // 3. Derived pattern chips always indicate active source chip without Tabbing.
    let derived = app
        .project()
        .patterns
        .iter()
        .find(|p| p.kind.sources().len() >= 2)
        .expect("derived pattern with >=2 sources")
        .id;
    app.apply(crate::Action::SelectPattern { pattern: derived })
        .unwrap();
    let snap = AudioSnapshot::default();
    let fv0 = app.frame_view(&snap, "silent");
    let chips0 = fv0
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Pattern(p) if p.focused => Some(p.sources.clone()),
            _ => None,
        })
        .expect("focused derived pattern");
    assert!(chips0[0].selected && !chips0[1].selected);
    app.press_keys("j").unwrap();
    let fv1 = app.frame_view(&snap, "silent");
    let chips1 = fv1
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Pattern(p) if p.focused => Some(p.sources.clone()),
            _ => None,
        })
        .expect("focused derived pattern");
    assert!(!chips1[0].selected && chips1[1].selected);

    // 4. Window state persists cleanly via MemoryHostIo when config persistence is active.
    let mem_io = std::sync::Arc::new(crate::MemoryHostIo::new("/workspace"));
    let mut win_app = App::new_with_host_io(
        Project::empty(),
        "win_test.json".into(),
        std::sync::Arc::clone(&mem_io) as std::sync::Arc<dyn crate::HostIo>,
    )
    .unwrap();
    win_app.apply_config(cadence_core::config::UserConfig::default());
    win_app.persist_window_state(1680, 1050, true, false);
    let saved = mem_io.saved_config().expect("config saved to MemoryHostIo");
    assert_eq!(saved.window_width, Some(1680));
    assert_eq!(saved.window_height, Some(1050));
    assert!(saved.window_maximized);
    assert!(!saved.window_fullscreen);

    // 5. Public request_quit() (used by OS window close & Cmd+Q/W):
    //    - Clean project quits immediately.
    let clean_out = win_app.request_quit();
    assert!(clean_out.quit, "clean project closes immediately");

    //    - Dirty project (even with Help open!) dismisses Help, surfaces Confirm on top,
    //      and `s` saves & quits cleanly to MemoryHostIo.
    win_app.execute_command("bpm 132").unwrap();
    win_app.press_keys("?").unwrap();
    assert!(win_app.show_help());
    let dirty_out = win_app.request_quit();
    assert!(
        !dirty_out.quit,
        "dirty close must prompt instead of exiting"
    );
    assert!(!win_app.show_help());
    let fv_confirm = win_app.frame_view(&AudioSnapshot::default(), "silent");
    assert!(
        matches!(fv_confirm.overlay, Some(OverlayView::Confirm { .. })),
        "Confirm overlay must take top priority over all other overlays"
    );
    let save_quit_out = win_app
        .handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE))
        .unwrap();
    assert!(save_quit_out.quit, "`s` in dirty quit prompt saves & quits");
    assert!(!win_app.dirty());
    assert!(
        mem_io.read_file("win_test.json").is_some(),
        "saved project must exist on disk/io after save & quit"
    );
}

#[test]
fn frame_view_builds_every_deck_family_and_overlays() {
    let mut app = demo_app();
    let snap = AudioSnapshot {
        playing: true,
        absolute_step: 4,
        step_fraction: 0.5,
        sample_rate: 48_000,
        ..AudioSnapshot::default()
    };

    // Evaluated Overview & Roll views
    app.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE))
        .unwrap();
    app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE))
        .unwrap();
    let fv_over = app.frame_view(&snap, "silent");
    let arr = fv_over
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Arrange(a) => Some(a),
            _ => None,
        })
        .unwrap();
    assert!(arr.evaluated.as_ref().is_some_and(|e| !e.notes.is_empty()));

    // Pattern deck
    app.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE))
        .unwrap();
    app.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE))
        .unwrap();
    let fv_pat = app.frame_view(&snap, "silent");
    let pat = fv_pat
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Pattern(p) => Some(p),
            _ => None,
        })
        .unwrap();
    assert!(!pat.empty);

    // Mixer deck
    app.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE))
        .unwrap();
    app.handle_key(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE))
        .unwrap();
    let fv_mix = app.frame_view(&snap, "silent");
    let mix = fv_mix
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Mixer(m) => Some(m),
            _ => None,
        })
        .unwrap();
    assert!(!mix.channels.is_empty());

    // Instrument Rack deck (panel curves + ADSR + controls)
    app.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE))
        .unwrap();
    app.handle_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE))
        .unwrap();
    let fv_inst = app.frame_view(&snap, "silent");
    let inst = fv_inst
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Instrument(i) => Some(i),
            _ => None,
        })
        .unwrap();
    assert!(!inst.controls.is_empty());
    match &inst.visual {
        InstrumentVisualView::Rack(rack) => {
            assert_eq!(rack.tabs.len(), 3);
            assert!(!rack.modules.is_empty());
            assert!(!rack.zones.is_empty());
            assert_eq!(rack.patchbay.columns.len(), 4);
            assert_eq!(rack.patchbay.feedback_badge, "z-1");
            match &rack.visual {
                ModuleVisualView::PinnedOverview {
                    osc_curve,
                    filter_curve,
                    env_curve,
                    ..
                } => {
                    assert_eq!(osc_curve.len(), 96);
                    assert_eq!(filter_curve.len(), 96);
                    assert_eq!(env_curve.len(), instrument::RACK_ENV_POINTS);
                }
                ModuleVisualView::Vco { curve, .. } => assert!(!curve.is_empty()),
                _ => {}
            }
        }
        InstrumentVisualView::Missing => panic!("expected instrument visual"),
    }

    // Help overlay
    app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE))
        .unwrap();
    let fv_help = app.frame_view(&snap, "silent");
    assert!(matches!(fv_help.overlay, Some(OverlayView::Help { .. })));
}

#[test]
fn mini_algebra_graph_deck_and_instrument_navigation() {
    let mut app = demo_app();
    let snap = AudioSnapshot {
        playing: true,
        absolute_step: 8,
        step_fraction: 0.25,
        sample_rate: 48_000,
        ..AudioSnapshot::default()
    };

    // :mini creates Mini pattern with sub-step events + code bar, and re-running :mini edits in place!
    app.execute_command("mini Hook 4 c4 [e4 g4] ~ [c4,e4,g4]")
        .unwrap();
    app.press_keys("gp").unwrap();
    let fv = app.frame_view(&snap, "silent");
    let pat = fv
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Pattern(p) => Some(p),
            _ => None,
        })
        .expect("pattern deck");
    assert_eq!(pat.mini_code.as_deref(), Some("c4 [e4 g4] ~ [c4,e4,g4]"));
    match &pat.content {
        PatternContentView::Notes { sub_notes, .. } => {
            assert!(
                sub_notes.iter().any(|n| n.offset > 0.4),
                "Mini subdivision must produce fractional sub-step notes"
            );
        }
        other => panic!("expected Notes content, got {other:?}"),
    }

    // :pattern euclid + :derive algebra commands
    app.execute_command("pattern euclid Tresillo 3 8 0")
        .unwrap();
    app.execute_command("derive ArpHook arp Hook updown 2")
        .unwrap();
    app.execute_command("derive SwungHook swing Hook 0.20")
        .unwrap();
    app.execute_command("derive RollHook ratchet Hook 3")
        .unwrap();
    app.execute_command("derive FastHook fast Hook 3 2")
        .unwrap();
    app.execute_command("derive BassFold fold Hook 36 48")
        .unwrap();
    app.execute_command("derive RevHook reverse Hook").unwrap();
    app.execute_command("derive EveryHook every 4 Hook RevHook")
        .unwrap();

    // Signal Graph deck (gG): 5 columns, hjkl walks columns/nodes, Enter jumps directly into deck!
    app.press_keys("gG").unwrap();
    assert!(app.graph_focused());
    let fv_g = app.frame_view(&snap, "silent");
    let graph = fv_g
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Graph(g) => Some(g),
            _ => None,
        })
        .expect("graph deck");
    assert_eq!(graph.columns.len(), 5);
    assert!(!graph.edges.is_empty());
    assert!(graph.edges.iter().any(|e| e.highlighted));

    // Walk to Instruments column (col 3) and press Enter -> opens Instrument deck!
    app.press_keys("lll<Enter>").unwrap();
    assert!(app
        .layout()
        .focused()
        .is_some_and(|d| d.kind == DeckKind::Instrument));
}

#[test]
fn loop_clear_inline_mini_tabbed_help_and_settings_modal() {
    let mut app = demo_app();
    let snap = AudioSnapshot::default();

    // 1. Set loop brackets with [ and ], then clear them completely via tx and :loop clear.
    app.press_keys("ga[]").unwrap();
    assert!(app.loop_region().is_some());
    app.press_keys("tx").unwrap();
    assert_eq!(app.loop_region(), None);

    app.press_keys("[]").unwrap();
    assert!(app.loop_region().is_some());
    app.execute_command("loop clear").unwrap();
    assert_eq!(app.loop_region(), None);

    // 2. Pressing `Enter` on a Mini pattern opens the dedicated Mini-Notation Editor modal!
    app.execute_command("mini Riff 4 c4 [e4 g4] ~ c5").unwrap();
    app.press_keys("gp<Enter>").unwrap();
    let fv_mini = app.frame_view(&snap, "silent");
    match &fv_mini.overlay {
        Some(OverlayView::MiniEditor(me)) => {
            assert_eq!(me.code, "c4 [e4 g4] ~ c5");
            assert!(me.valid);
            assert!(me.event_count > 0);
        }
        other => panic!("expected MiniEditor overlay, got {other:?}"),
    }
    app.press_keys("<Esc>").unwrap();

    // 3. Tabbed + `/` searchable Help dialog (`?`)
    app.press_keys("?").unwrap();
    let fv_all = app.frame_view(&snap, "silent");
    let total = match &fv_all.overlay {
        Some(OverlayView::Help(h)) => h.entries.len(),
        other => panic!("expected Help overlay, got {other:?}"),
    };
    assert!(total >= 20);
    app.press_keys("l").unwrap(); // Navigate tab
    let fv_tab = app.frame_view(&snap, "silent");
    let tab_count = match &fv_tab.overlay {
        Some(OverlayView::Help(h)) => {
            assert_eq!(h.active_tab, 1);
            h.entries.len()
        }
        other => panic!("expected Help overlay, got {other:?}"),
    };
    assert!(tab_count < total);
    app.press_keys("/euclid<Enter>").unwrap();
    let fv_search = app.frame_view(&snap, "silent");
    match &fv_search.overlay {
        Some(OverlayView::Help(h)) => {
            assert_eq!(h.query, "euclid");
            assert!(!h.entries.is_empty());
            assert!(h.entries.len() <= 4);
        }
        other => panic!("expected Help overlay, got {other:?}"),
    }
    app.press_keys("<Esc><Esc>").unwrap();

    // 4. Settings modal (`g,`) switches live theme & updates chrome.theme!
    app.press_keys("g,").unwrap();
    let fv_set = app.frame_view(&snap, "silent");
    assert!(matches!(fv_set.overlay, Some(OverlayView::Settings(_))));
    assert_eq!(
        fv_set.chrome.theme,
        cadence_core::config::ThemeId::CadenceDark
    );
    app.press_keys("<m-l>").unwrap();
    let fv_neon = app.frame_view(&snap, "silent");
    assert_eq!(
        fv_neon.chrome.theme,
        cadence_core::config::ThemeId::MidnightNeon
    );
    app.press_keys("<m-l><Esc>").unwrap();
    let fv_light = app.frame_view(&snap, "silent");
    assert_eq!(
        fv_light.chrome.theme,
        cadence_core::config::ThemeId::StudioLight
    );
}

#[test]
fn rack_module_visual_curves_match_core_dsp_and_phase_bounds() {
    use cadence_core::dsp::{
        drive_transfer_sample, eval_modular_env_curve, filter_mix_magnitude_db,
        lofi_transfer_sample, norm_to_log, resonator_mode_descriptors,
    };
    use cadence_core::material::Edit;
    use cadence_core::rack::{
        DriveModule, EnvModule, FilterMode, FilterModule, InPort, LoFiModule, ModuleKind,
        PatchCable, PitchTracking, RackModule, RackPatch, ResonatorMode, ResonatorModule,
        TriggerMode, VcoModule, Waveform,
    };

    // Build a Rack with VCO A (Saw), VCO B (Saw), Filter (AcidLowPass), Env (Gate), Drive, and LoFi.
    let mut rack = RackPatch::default_synth();
    let z = &mut rack.zones[0];
    z.modules.clear();
    z.cables.clear();
    rack.panel_pins.clear();

    let vco_a = VcoModule {
        waveform: Waveform::Saw,
        ..Default::default()
    };
    let vco_b = VcoModule {
        waveform: Waveform::Saw,
        ..Default::default()
    };
    let filt = FilterModule {
        mode: FilterMode::AcidLowPass,
        cutoff_hz: 1_200.0,
        resonance: 0.72,
        mix: 0.85,
        ..FilterModule::default()
    };
    let env = EnvModule {
        attack_ms: 8.0,
        decay_ms: 220.0,
        sustain: 0.55,
        release_ms: 320.0,
        trigger_mode: TriggerMode::Gate,
        ..EnvModule::default()
    };
    let drive = DriveModule {
        drive: 0.65,
        asymmetry: 0.2,
        mix: 0.9,
        ..DriveModule::default()
    };
    let lofi = LoFiModule {
        bits: 6.0,
        rate_hz: 8_000.0,
        mix: 1.0,
    };
    let res = ResonatorModule {
        mode: ResonatorMode::Plate,
        pitch_tracking: PitchTracking::Fixed,
        fixed_hz: 520.0,
        structure: 0.5,
        decay_ms: 1_200.0,
        brightness: 0.72,
        position: 0.58,
        mix: 0.85,
        ..ResonatorModule::default()
    };
    z.modules = vec![
        RackModule::new(1, "VCO A", ModuleKind::Vco(vco_a)),
        RackModule::new(2, "VCO B", ModuleKind::Vco(vco_b)),
        RackModule::new(3, "Filter", ModuleKind::Filter(filt)),
        RackModule::new(4, "Amp Env", ModuleKind::Env(env)),
        RackModule::new(5, "Drive", ModuleKind::Drive(drive)),
        RackModule::new(6, "LoFi", ModuleKind::LoFi(lofi)),
        RackModule::new(8, "Plate Res", ModuleKind::Resonator(res)),
        RackModule::new(7, "Out", ModuleKind::Out(Default::default())),
    ];
    z.cables = vec![
        PatchCable::new(1, 1, 3, InPort::In, 1.0),
        PatchCable::new(2, 3, 5, InPort::In, 1.0),
        PatchCable::new(3, 5, 6, InPort::In, 1.0),
        PatchCable::new(4, 6, 8, InPort::In, 1.0),
        PatchCable::new(6, 8, 7, InPort::In, 1.0),
        PatchCable::new(5, 4, 7, InPort::Cv, 1.0),
    ];

    let mut proj = Project::demo();
    let inst_id = proj.instruments[0].id;
    proj.apply(Edit::UpdateRackPatch {
        id: inst_id,
        rack: rack.clone(),
    })
    .unwrap();
    let mut app = App::new(proj, "rack-vis.json".into()).unwrap();
    app.open_instrument(inst_id).unwrap();
    let snap = AudioSnapshot {
        sample_rate: 48_000,
        ..AudioSnapshot::default()
    };

    let get_visual = |app: &mut App, mod_id: u16| {
        app.focus_rack_module_panel(inst_id, mod_id);
        match app
            .frame_view(&snap, "silent")
            .decks
            .into_iter()
            .find_map(|d| match d.view {
                DeckView::Instrument(i) => Some(i.visual),
                _ => None,
            })
            .unwrap()
        {
            InstrumentVisualView::Rack(r) => r.visual,
            other => panic!("expected Rack visual, got {other:?}"),
        }
    };

    // 1. VCO A Saw has NO right-edge wrap spike.
    match get_visual(&mut app, 1) {
        ModuleVisualView::Vco {
            curve, fm_active, ..
        } => {
            assert!(!fm_active);
            assert_eq!(curve.len(), 96);
            assert!((curve[0] - (-1.0)).abs() < 1e-5);
            assert!(
                *curve.last().unwrap() > 0.90,
                "Saw right edge must NOT wrap back to -1.0, got {}",
                curve.last().unwrap()
            );
            assert!(curve[95] > curve[94]);
        }
        other => panic!("expected Vco visual, got {other:?}"),
    }

    // 2. Filter curve matches cadence_core::dsp::filter_mix_magnitude_db with zero drift.
    match get_visual(&mut app, 3) {
        ModuleVisualView::Filter { curve, .. } => {
            let high_hz = 20_000.0_f32;
            for (i, &db) in curve.iter().enumerate() {
                let x = i as f32 / (curve.len() - 1) as f32;
                let f = norm_to_log(x, 20.0, high_hz);
                let expected = filter_mix_magnitude_db(
                    filt.mode,
                    filt.cutoff_hz,
                    filt.resonance,
                    filt.mix,
                    f64::from(f),
                    48_000.0,
                )
                .clamp(-48.0, 18.0) as f32;
                assert!(
                    (db - expected).abs() < 1e-4,
                    "Rack filter curve drift at i={i}: view={db} vs dsp={expected}"
                );
            }
        }
        other => panic!("expected Filter visual, got {other:?}"),
    }

    // 3. Env matches cadence_core::dsp::eval_modular_env_curve exactly.
    match get_visual(&mut app, 4) {
        ModuleVisualView::Env { curve, .. } => {
            let expected = eval_modular_env_curve(env.to_dsp_params(), instrument::RACK_ENV_POINTS);
            assert_eq!(curve, expected);
        }
        other => panic!("expected Env visual, got {other:?}"),
    }

    // 4. Drive & LoFi transfer curves match core DSP SPOTs exactly.
    match get_visual(&mut app, 5) {
        ModuleVisualView::Drive { transfer_curve, .. } => {
            let dp = drive.to_drive_patch();
            for (i, &y) in transfer_curve.iter().enumerate() {
                let x = (i as f32 / (transfer_curve.len() - 1) as f32) * 2.0 - 1.0;
                let expected = drive_transfer_sample(x, &dp).clamp(-1.5, 1.5);
                assert!((y - expected).abs() < 1e-5);
            }
        }
        other => panic!("expected Drive visual, got {other:?}"),
    }
    match get_visual(&mut app, 6) {
        ModuleVisualView::LoFi { transfer_curve, .. } => {
            for (i, &y) in transfer_curve.iter().enumerate() {
                let x = (i as f32 / (transfer_curve.len() - 1) as f32) * 2.0 - 1.0;
                let expected = lofi_transfer_sample(x, lofi.bits, lofi.mix).clamp(-1.2, 1.2);
                assert!((y - expected).abs() < 1e-5);
            }
        }
        other => panic!("expected LoFi visual, got {other:?}"),
    }
    match get_visual(&mut app, 8) {
        ModuleVisualView::Resonator {
            mode_label,
            base_hz,
            modes,
            ..
        } => {
            assert_eq!(mode_label, "Plate");
            assert!((base_hz - 520.0).abs() < 1e-3);
            let descs = resonator_mode_descriptors(
                res.mode,
                f64::from(res.fixed_hz),
                res.structure,
                res.decay_ms,
                res.brightness,
                res.position,
                48_000.0,
            );
            let active_count = descs.iter().filter(|d| d.active && d.gain > 0.0).count();
            assert_eq!(modes.len(), active_count);
            assert!(modes.iter().all(|(x, h, d)| {
                (0.0..=1.0).contains(x) && (0.0..=1.0).contains(h) && (0.0..=1.0).contains(d)
            }));
        }
        other => panic!("expected Resonator visual, got {other:?}"),
    }

    // 5. Patched FM (`VCO B -> VCO A.FmIn`) distinguishes 48st (16:1) vs 36st (8:1) and -36st vs -24st,
    //    and VCO phase_deg shifts waveform start (90° Triangle -> 0.0 rising, 180° -> +1.0 peak).
    let sample_fm = |semi: f32| {
        let mut r = rack.clone();
        if let ModuleKind::Vco(v) = &mut r.zones[0].modules[0].kind {
            v.waveform = Waveform::Sine;
        }
        if let ModuleKind::Vco(v) = &mut r.zones[0].modules[1].kind {
            v.waveform = Waveform::Sine;
            v.tune_semitones = semi;
        }
        r.zones[0]
            .cables
            .push(PatchCable::new(9, 2, 1, InPort::FmIn, 0.65));
        let mut p = Project::demo();
        p.apply(Edit::UpdateRackPatch {
            id: inst_id,
            rack: r,
        })
        .unwrap();
        let mut a = App::new(p, "fm.json".into()).unwrap();
        a.open_instrument(inst_id).unwrap();
        a.focus_rack_module_panel(inst_id, 1);
        match a
            .frame_view(&snap, "silent")
            .decks
            .into_iter()
            .find_map(|d| match d.view {
                DeckView::Instrument(i) => Some(i.visual),
                _ => None,
            })
            .unwrap()
        {
            InstrumentVisualView::Rack(rv) => match rv.visual {
                ModuleVisualView::Vco {
                    curve, fm_active, ..
                } => {
                    assert!(fm_active);
                    curve
                }
                other => panic!("expected Vco, got {other:?}"),
            },
            other => panic!("expected Rack, got {other:?}"),
        }
    };
    let fm_hi_8x = sample_fm(36.0);
    let fm_hi_16x = sample_fm(48.0);
    let fm_lo_qtr = sample_fm(-24.0);
    let fm_lo_eighth = sample_fm(-36.0);
    assert_ne!(fm_hi_8x, fm_hi_16x, "48 st (16:1) FM must not clamp to 8:1");
    assert_ne!(
        fm_lo_qtr, fm_lo_eighth,
        "-36 st (1:8) FM must not clamp to 1:4"
    );
    assert!(fm_hi_16x
        .iter()
        .chain(fm_lo_eighth.iter())
        .all(|v| v.is_finite() && (-1.2..=1.2).contains(v)));

    // Phase offset check (Triangle shares sin(2πφ) phase: 0°→0 rising, 90°→+1, 180°→0 falling)
    let mut r_ph = rack;
    if let ModuleKind::Vco(v) = &mut r_ph.zones[0].modules[0].kind {
        v.waveform = Waveform::Triangle;
        v.phase_deg = 0.0;
    }
    if let ModuleKind::Vco(v) = &mut r_ph.zones[0].modules[1].kind {
        v.waveform = Waveform::Triangle;
        v.phase_deg = 90.0;
    }
    let mut p_ph = Project::demo();
    p_ph.apply(Edit::UpdateRackPatch {
        id: inst_id,
        rack: r_ph,
    })
    .unwrap();
    let mut a_ph = App::new(p_ph, "ph.json".into()).unwrap();
    a_ph.open_instrument(inst_id).unwrap();
    match get_visual(&mut a_ph, 1) {
        ModuleVisualView::Vco { curve, .. } => {
            assert!(
                curve[0].abs() < 1e-5 && curve[1] > curve[0],
                "0° Triangle must start at 0.0 rising: {:?}",
                &curve[..4]
            );
        }
        other => panic!("expected Vco, got {other:?}"),
    }
    match get_visual(&mut a_ph, 2) {
        ModuleVisualView::Vco { curve, .. } => {
            assert!(
                (curve[0] - 1.0).abs() < 1e-5 && curve[1] < curve[0],
                "90° Triangle must start at +1.0 falling: {:?}",
                &curve[..4]
            );
        }
        other => panic!("expected Vco, got {other:?}"),
    }
}

#[test]
fn float_metering_projects_chrome_mixer_gr_and_output_views() {
    use cadence_core::mixer::{CompressorPatch, EffectDevice, EffectKind, LimiterPatch};

    let mut project = Project::demo();
    let ch0_id = project.mixer.channels[0].id;
    let comp_id = project.max_id() + 1;
    let lim_id = project.max_id() + 2;
    project.mixer.channels[0].inserts.push(EffectDevice {
        id: comp_id,
        name: "Strip Comp".into(),
        bypassed: false,
        kind: EffectKind::Compressor(CompressorPatch::default()),
    });
    project.mixer.master.inserts.push(EffectDevice {
        id: lim_id,
        name: "Master Lim".into(),
        bypassed: false,
        kind: EffectKind::Limiter(LimiterPatch::default()),
    });

    let mut app = App::new(project, "demo.json".into()).unwrap();
    app.press_keys("gm").unwrap();

    let mut snap = AudioSnapshot {
        playing: true,
        sample_rate: 48_000,
        peak_left: 0.92,
        peak_right: 0.89,
        channel_meters: vec![
            ChannelMeter {
                channel: ChannelRef::Master,
                input_peak: [1.35, 1.28],
                input_peak_hold: [1.50, 1.42],
                output_peak: [0.92, 0.89],
                output_peak_hold: [0.96, 0.94],
                hot: true,
                clipped: false,
            },
            ChannelMeter {
                channel: ChannelRef::Channel(ch0_id),
                input_peak: [1.48, 1.40],
                input_peak_hold: [1.62, 1.54],
                output_peak: [0.74, 0.70],
                output_peak_hold: [0.81, 0.77],
                hot: true,
                clipped: false,
            },
        ],
        ..AudioSnapshot::default()
    };
    snap.set_effect_meter(comp_id, 4.3, 6.8);
    snap.set_effect_meter(lim_id, 2.9, 5.1);

    let fv = app.frame_view(&snap, "CoreAudio 48 kHz");
    assert!(fv.chrome.master_hot && !fv.chrome.master_clipped);

    let mixer = fv
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Mixer(m) => Some(m),
            _ => None,
        })
        .expect("MixerDeckView");
    let strip0 = mixer
        .channels
        .iter()
        .find(|c| c.channel == ChannelRef::Channel(ch0_id))
        .expect("ch0 strip");
    assert_eq!(strip0.gr_tag, Some("CMP"));
    assert!((strip0.gr_db - 4.3).abs() < 1e-4);
    assert!((strip0.gr_hold_db - 6.8).abs() < 1e-4);
    assert!(strip0.meter.is_some_and(|m| m.hot && !m.clipped));

    let master_strip = mixer
        .channels
        .iter()
        .find(|c| c.channel == ChannelRef::Master)
        .expect("master strip");
    assert_eq!(master_strip.gr_tag, Some("LIM"));
    assert!((master_strip.gr_db - 2.9).abs() < 1e-4);
    assert!((master_strip.gr_hold_db - 5.1).abs() < 1e-4);

    let out = fv
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Output(o) => Some(o),
            _ => None,
        })
        .expect("OutputDeckView");
    assert!(out.hot && !out.clipped);
    assert_eq!(out.input_left, Some(1.35));
    assert_eq!(out.input_hold_left, Some(1.50));
    assert_eq!(out.hold_left, Some(0.96));
    assert_eq!(out.master_gr_tag, Some("LIM"));
    assert!((out.master_gr_db - 2.9).abs() < 1e-4);
    assert!((out.master_gr_hold_db - 5.1).abs() < 1e-4);
}

#[test]
fn macro_bindings_project_live_curves_across_rack_effect_mixer_and_loop_wrap() {
    use cadence_core::macros::{MacroBinding, MacroControl};
    use cadence_core::material::{
        Modulation, ModulationTarget, Pattern, PatternKind, ValuePoint, ValueUnit,
    };
    use cadence_core::mixer::{ChannelParam, EffectDevice, EffectKind, EffectParam, FilterPatch};
    use cadence_core::params::{ParamAddress, ValueMode};
    use cadence_core::rack::{FilterMode, ModuleKind, ModuleParam};

    let mut project = Project::demo();
    // Pick the first Rack instrument that contains a Filter module.
    let (synth_id, zone_id, filter_mod_id) = project
        .instruments
        .iter()
        .find_map(|i| {
            let z = i.rack.zones.first()?;
            let fm = z
                .modules
                .iter()
                .find(|m| matches!(m.kind, ModuleKind::Filter(_)))?;
            Some((i.id, z.id, fm.id))
        })
        .expect("demo instrument with Filter module");
    let ch_id = project.mixer.channels[0].id;
    let ch_ref = ChannelRef::Channel(ch_id);
    project.mixer.channels[0].strip.gain = 0.8;
    project.mixer.channels[0].strip.pan = 0.0;

    // Add an insert Filter effect on ch0.
    let filt_id = project.max_id() + 1;
    project.mixer.channels[0].inserts.push(EffectDevice {
        id: filt_id,
        name: "Macro Filter".into(),
        bypassed: false,
        kind: EffectKind::Filter(FilterPatch {
            mode: FilterMode::LowPass,
            cutoff_hz: 400.0,
            resonance: 0.1,
            mix: 1.0,
            ..FilterPatch::default()
        }),
    });

    // Baseline static Rack Filter view before any Macro bindings.
    let mut base_app = App::new(project.clone(), "demo.json".into()).unwrap();
    base_app.open_instrument(synth_id).unwrap();
    base_app.focus_rack_module_panel(synth_id, filter_mod_id);
    let base_snap = AudioSnapshot {
        playing: true,
        absolute_step: 4,
        step_fraction: 0.5,
        sample_rate: 48_000,
        ..AudioSnapshot::default()
    };
    let base_filter_curve = match base_app
        .frame_view(&base_snap, "silent")
        .decks
        .into_iter()
        .find_map(|d| match d.view {
            DeckView::Instrument(i) => Some(i.visual),
            _ => None,
        })
        .unwrap()
    {
        InstrumentVisualView::Rack(r) => match r.visual {
            ModuleVisualView::Filter {
                curve,
                mod_cutoff_norm,
                ..
            } => {
                assert!(mod_cutoff_norm.is_none());
                curve
            }
            other => panic!("expected Filter module visual, got {other:?}"),
        },
        other => panic!("expected Rack visual, got {other:?}"),
    };

    // Static Macro M1 drives Rack Filter Mode -> HighPass, Cutoff -> high, Resonance -> 0.85,
    // plus Mixer Gain -> 0.5, Pan -> R100, and Insert Filter Cutoff -> high!
    let hp_norm = FilterMode::HighPass.to_index() as f32 / (FilterMode::ALL.len() - 1) as f32;
    project.macros.push(MacroControl {
        index: 0,
        name: "Tone".into(),
        value: 1.0,
        bindings: vec![
            MacroBinding {
                target: ParamAddress::Module {
                    instrument: synth_id,
                    zone: zone_id,
                    module: filter_mod_id,
                    param: ModuleParam::Cutoff,
                },
                min: 0.2,
                max: 0.9,
            },
            MacroBinding {
                target: ParamAddress::Module {
                    instrument: synth_id,
                    zone: zone_id,
                    module: filter_mod_id,
                    param: ModuleParam::Resonance,
                },
                min: 0.0,
                max: 0.85,
            },
            MacroBinding {
                target: ParamAddress::Module {
                    instrument: synth_id,
                    zone: zone_id,
                    module: filter_mod_id,
                    param: ModuleParam::FilterMode,
                },
                min: hp_norm,
                max: hp_norm,
            },
            MacroBinding {
                target: ParamAddress::Channel {
                    channel: ch_ref,
                    param: ChannelParam::Gain,
                },
                min: 0.0,
                max: 0.5,
            },
            MacroBinding {
                target: ParamAddress::Channel {
                    channel: ch_ref,
                    param: ChannelParam::Pan,
                },
                min: 0.5,
                max: 1.0,
            },
            MacroBinding {
                target: ParamAddress::Effect {
                    device: filt_id,
                    param: EffectParam::Cutoff,
                },
                min: 0.1,
                max: 0.95,
            },
        ],
    });

    let mut app = App::new(project.clone(), "demo.json".into()).unwrap();
    app.open_instrument(synth_id).unwrap();
    app.focus_rack_module_panel(synth_id, filter_mod_id);
    let fv_inst = app.frame_view(&base_snap, "silent");
    match fv_inst
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Instrument(i) => Some(&i.visual),
            _ => None,
        })
        .unwrap()
    {
        InstrumentVisualView::Rack(r) => match &r.visual {
            ModuleVisualView::Filter {
                label,
                curve,
                mod_cutoff_norm,
                ..
            } => {
                assert!(
                    label.contains(FilterMode::HighPass.label()),
                    "static Macro FilterMode must update Filter label, got {label}"
                );
                assert!(
                    mod_cutoff_norm.is_some(),
                    "static Macro Cutoff must set mod_cutoff_norm"
                );
                assert_ne!(
                    *curve, base_filter_curve,
                    "Macro-driven Cutoff/Resonance/FilterMode must move Filter curve"
                );
            }
            other => panic!("expected Filter module visual, got {other:?}"),
        },
        other => panic!("expected Rack visual, got {other:?}"),
    }

    // Mixer view reflects Macro Gain & Pan!
    app.press_keys("gm").unwrap();
    let fv_mix = app.frame_view(&base_snap, "silent");
    let mix = fv_mix
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Mixer(m) => Some(m),
            _ => None,
        })
        .unwrap();
    let ch0 = mix.channels.iter().find(|c| c.channel == ch_ref).unwrap();
    assert!(
        (ch0.effective_gain.unwrap() - cadence_core::dsp::norm_to_channel_gain(0.5)).abs() < 1e-3
    );
    assert!(ch0.effective_gain_db.is_some());
    assert!(ch0.effective_pan.unwrap() > 0.95);
    assert!(ch0.pan_modulators.iter().any(|m| m.contains("M1 Tone")));

    // Effect view reflects Macro Cutoff!
    app.execute_command(&format!("fx {filt_id}")).unwrap();
    let fv_fx = app.frame_view(&base_snap, "silent");
    match fv_fx
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Effect(e) => Some(&e.visual),
            _ => None,
        })
        .unwrap()
    {
        EffectVisualView::Filter {
            mod_cutoff_norm,
            mod_cutoff_hz,
            mod_curve_db,
            ..
        } => {
            assert!(mod_cutoff_norm.is_some());
            assert!(mod_cutoff_hz.unwrap() > 8_000.0);
            assert!(mod_curve_db.is_some());
        }
        other => panic!("expected Filter visual, got {other:?}"),
    }

    // Now automate Macro M1 with a 32-step ramp (0.0 at step 0 -> 1.0 at step 16 -> 0.0 at step 32)
    // and loop B01 (steps 0..16). At absolute_step = 20, raw absolute_step would read descending
    // value ~0.75, whereas smooth song_position wraps to song step 4 (ascending value 0.25)!
    let ramp_pat = project.max_id() + 2;
    project.patterns.push(Pattern {
        id: ramp_pat,
        name: "Macro Ramp".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 32,
            mode: ValueMode::Ramp,
            points: vec![
                ValuePoint {
                    step: 0,
                    value: 0.0,
                },
                ValuePoint {
                    step: 16,
                    value: 1.0,
                },
                ValuePoint {
                    step: 31,
                    value: 1.0,
                },
            ],
        },
    });
    let m_id = project.max_id() + 3;
    project.modulations.push(Modulation {
        id: m_id,
        name: "M1 Ramp".into(),
        pattern: ramp_pat,
        target: ModulationTarget::Macro { index: 0 },
    });

    let mut loop_app = App::new(project, "demo.json".into()).unwrap();
    loop_app.press_keys("ga[]\\<Space>").unwrap(); // loop steps 0..16 and start playing!
    assert_eq!(loop_app.playing_loop(), Some((0, 16)));
    loop_app.open_instrument(synth_id).unwrap();
    loop_app.focus_rack_module_panel(synth_id, filter_mod_id);
    let snap_looped = AudioSnapshot {
        playing: true,
        absolute_step: 20, // wraps to song step 4 inside 0..16 loop!
        step_fraction: 0.0,
        sample_rate: 48_000,
        ..AudioSnapshot::default()
    };
    let snap_first_pass = AudioSnapshot {
        playing: true,
        absolute_step: 4,
        step_fraction: 0.0,
        sample_rate: 48_000,
        ..AudioSnapshot::default()
    };
    let cutoff_at = |s: &AudioSnapshot| match loop_app
        .frame_view(s, "silent")
        .decks
        .into_iter()
        .find_map(|d| match d.view {
            DeckView::Instrument(i) => Some(i.visual),
            _ => None,
        })
        .unwrap()
    {
        InstrumentVisualView::Rack(r) => match r.visual {
            ModuleVisualView::Filter {
                mod_cutoff_norm, ..
            } => mod_cutoff_norm.expect("modulated cutoff"),
            other => panic!("expected Filter module visual, got {other:?}"),
        },
        other => panic!("expected Rack visual, got {other:?}"),
    };
    assert!(
        (cutoff_at(&snap_looped) - cutoff_at(&snap_first_pass)).abs() < 1e-4,
        "Rack view must use song_position_smooth across loops, not raw absolute_step"
    );
}

#[test]
fn direct_modulation_overrides_macro_and_last_write_wins_matching_compiler() {
    use cadence_core::macros::{MacroBinding, MacroControl};
    use cadence_core::material::{
        Modulation, ModulationTarget, Pattern, PatternKind, ValuePoint, ValueUnit,
    };
    use cadence_core::mixer::{ChannelParam, EffectDevice, EffectKind, EffectParam, FilterPatch};
    use cadence_core::params::{InstrumentParam, ParamAddress, ValueMode};
    use cadence_core::rack::{ModuleKind, ModuleParam};

    let mut project = Project::demo();
    let (synth_id, zone_id, filter_mod_id) = project
        .instruments
        .iter()
        .find_map(|i| {
            let z = i.rack.zones.first()?;
            let fm = z
                .modules
                .iter()
                .find(|m| matches!(m.kind, ModuleKind::Filter(_)))?;
            Some((i.id, z.id, fm.id))
        })
        .expect("demo instrument with Filter module");
    let ch_id = project.mixer.channels[0].id;
    let ch_ref = ChannelRef::Channel(ch_id);
    project.mixer.channels[0].strip.gain = 0.8;

    let filt_id = project.max_id() + 1;
    project.mixer.channels[0].inserts.push(EffectDevice {
        id: filt_id,
        name: "Prec Filter".into(),
        bypassed: false,
        kind: EffectKind::Filter(FilterPatch::default()),
    });

    let mut next_id = project.max_id() + 2;
    let mut add_const_pat = |name: &str, val: f32| {
        let id = next_id;
        next_id += 1;
        project.patterns.push(Pattern {
            id,
            name: name.into(),
            kind: PatternKind::Values {
                unit: ValueUnit::Normalized,
                period: 16,
                mode: ValueMode::Hold,
                points: vec![ValuePoint {
                    step: 0,
                    value: val,
                }],
            },
        });
        id
    };

    let pat_lo = add_const_pat("Lo 0.1", 0.1);
    let pat_mid = add_const_pat("Mid 0.5", 0.5);
    let pat_hi = add_const_pat("Hi 0.9", 0.9);
    let pat_scale = add_const_pat("Scale 0.5", 0.5);

    let mut push_mod =
        |name: &str, pattern: cadence_core::material::Id, target: ModulationTarget| {
            let id = next_id;
            next_id += 1;
            project.modulations.push(Modulation {
                id,
                name: name.into(),
                pattern,
                target,
            });
        };

    // Direct Filter Cutoff (0.5) must beat automated Macro Cutoff (0.9)!
    push_mod(
        "Direct Cutoff",
        pat_mid,
        ModulationTarget::Instrument {
            instrument: synth_id,
            parameter: InstrumentParam::Module {
                zone: zone_id,
                module: filter_mod_id,
                param: ModuleParam::Cutoff,
            },
        },
    );
    // Direct Pan (0.1 -> -0.8) must beat automated Macro Pan (0.9 -> +0.8)!
    push_mod(
        "Direct Pan",
        pat_lo,
        ModulationTarget::ChannelPan { channel: ch_ref },
    );
    // Direct ChannelGain (0.5 scale) MULTIPLIES Macro's ChannelParam::Gain (0.9 * 0.8 = 0.72 -> 0.36)!
    push_mod(
        "Direct Gain Scale",
        pat_scale,
        ModulationTarget::ChannelGain { channel: ch_ref },
    );
    // Direct Effect Cutoff (0.5) must beat automated Macro Cutoff (0.9)!
    push_mod(
        "Direct Fx",
        pat_mid,
        ModulationTarget::Effect {
            device: filt_id,
            parameter: EffectParam::Cutoff,
        },
    );

    // Static M1 (0.2) vs Static M2 (0.7) both bound to Effect Resonance -> higher index M2 (0.7) wins!
    project.macros.push(MacroControl {
        index: 0,
        name: "Low Macro".into(),
        value: 0.2,
        bindings: vec![MacroBinding {
            target: ParamAddress::Effect {
                device: filt_id,
                param: EffectParam::Resonance,
            },
            min: 0.0,
            max: 1.0,
        }],
    });
    project.macros.push(MacroControl {
        index: 1,
        name: "High Static".into(),
        value: 0.7,
        bindings: vec![
            MacroBinding {
                target: ParamAddress::Effect {
                    device: filt_id,
                    param: EffectParam::Resonance,
                },
                min: 0.0,
                max: 1.0,
            },
            MacroBinding {
                target: ParamAddress::Module {
                    instrument: synth_id,
                    zone: zone_id,
                    module: filter_mod_id,
                    param: ModuleParam::Resonance,
                },
                min: 0.0,
                max: 1.0,
            },
        ],
    });
    // Automated M3 (index 2, automated to 0.9) binds Filter Cutoff/Resonance, Mixer Pan/Gain, Effect Cutoff
    // and comes AFTER all direct modulations in `project.modulations`.
    project.macros.push(MacroControl {
        index: 2,
        name: "Big Auto".into(),
        value: 0.0,
        bindings: vec![
            MacroBinding {
                target: ParamAddress::Module {
                    instrument: synth_id,
                    zone: zone_id,
                    module: filter_mod_id,
                    param: ModuleParam::Cutoff,
                },
                min: 0.0,
                max: 1.0,
            },
            MacroBinding {
                target: ParamAddress::Module {
                    instrument: synth_id,
                    zone: zone_id,
                    module: filter_mod_id,
                    param: ModuleParam::Resonance,
                },
                min: 0.0,
                max: 1.0,
            },
            MacroBinding {
                target: ParamAddress::Channel {
                    channel: ch_ref,
                    param: ChannelParam::Pan,
                },
                min: 0.0,
                max: 1.0,
            },
            MacroBinding {
                target: ParamAddress::Channel {
                    channel: ch_ref,
                    param: ChannelParam::Gain,
                },
                min: 0.0,
                max: 0.8,
            },
            MacroBinding {
                target: ParamAddress::Effect {
                    device: filt_id,
                    param: EffectParam::Cutoff,
                },
                min: 0.0,
                max: 1.0,
            },
        ],
    });
    push_mod("M3 Auto", pat_hi, ModulationTarget::Macro { index: 2 });

    let mut app = App::new(project, "demo.json".into()).unwrap();
    let snap = AudioSnapshot {
        playing: true,
        absolute_step: 2,
        step_fraction: 0.0,
        sample_rate: 48_000,
        ..AudioSnapshot::default()
    };

    // 1. Macros view: M3 reflects automated value 0.9!
    app.press_keys("gk").unwrap();
    let fv_mac = app.frame_view(&snap, "silent");
    let mview = fv_mac
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Macros(m) => Some(m),
            _ => None,
        })
        .unwrap();
    assert!((mview.slots[2].effective_value.unwrap() - 0.9).abs() < 1e-4);

    // 2. Direct Filter Cutoff (0.5) beats automated M3 (0.9);
    //    Automated M3 Filter Resonance (0.9) beats static M2 (0.7);
    //    Higher-index static M2 Effect Resonance (0.7) beats lower-index static M1 (0.2)!
    let qt = app.live_query_time(&snap);
    let synth_cutoff = app
        .evaluate_live_param_at(
            ParamAddress::Module {
                instrument: synth_id,
                zone: zone_id,
                module: filter_mod_id,
                param: ModuleParam::Cutoff,
            },
            qt,
        )
        .unwrap();
    assert!((synth_cutoff - 0.5).abs() < 1e-4);
    let synth_res = app
        .evaluate_live_param_at(
            ParamAddress::Module {
                instrument: synth_id,
                zone: zone_id,
                module: filter_mod_id,
                param: ModuleParam::Resonance,
            },
            qt,
        )
        .unwrap();
    assert!((synth_res - 0.9).abs() < 1e-4);
    let fx_res = app
        .evaluate_live_param_at(
            ParamAddress::Effect {
                device: filt_id,
                param: EffectParam::Resonance,
            },
            qt,
        )
        .unwrap();
    assert!((fx_res - 0.7).abs() < 1e-4);

    // 3. Mixer view: Direct Pan (0.1 -> -0.8) beats automated M3 Pan (0.9 -> +0.8),
    //    and M3 Gain (norm 0.9 * 0.8 = 0.72 -> norm_to_channel_gain(0.72)) * Direct GainScale (0.5)!
    app.press_keys("gm").unwrap();
    let fv_mix = app.frame_view(&snap, "silent");
    let mix = fv_mix
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Mixer(m) => Some(m),
            _ => None,
        })
        .unwrap();
    let ch0 = mix.channels.iter().find(|c| c.channel == ch_ref).unwrap();
    assert!((ch0.effective_pan.unwrap() - (-0.8)).abs() < 1e-3);
    assert!((ch0.automated_gain_scale.unwrap() - 0.5).abs() < 1e-3);
    assert!(
        (ch0.effective_gain.unwrap() - cadence_core::dsp::norm_to_channel_gain(0.72) * 0.5).abs()
            < 1e-3
    );

    // 4. Effect view: Direct Filter Cutoff (0.5) beats automated M3 Cutoff (0.9)!
    let fx_cutoff = app
        .evaluate_live_param_at(
            ParamAddress::Effect {
                device: filt_id,
                param: EffectParam::Cutoff,
            },
            qt,
        )
        .unwrap();
    assert!((fx_cutoff - 0.5).abs() < 1e-4);
}

#[test]
fn delay_and_drive_modes_have_full_app_symmetry_across_controls_commands_macros_mods_and_midi() {
    use cadence_core::material::{Pattern, PatternKind, ValuePoint, ValueUnit};
    use cadence_core::midi_map::MidiTarget;
    use cadence_core::mixer::{DelayMode, DriveMode, EffectDevice, EffectKind, EffectParam};
    use cadence_core::params::{ParamAddress, ValueMode};

    let mut project = Project::demo();
    let delay_id = project.max_id() + 1;
    let drive_id = delay_id + 1;
    let pat_id = drive_id + 1;
    project.mixer.master.inserts.clear();
    let mut delay_dev = EffectDevice::delay(delay_id);
    delay_dev.name = "ModeDelay".into();
    let mut drive_dev = EffectDevice::drive(drive_id);
    drive_dev.name = "ModeDrive".into();
    project.mixer.master.inserts.push(delay_dev);
    project.mixer.master.inserts.push(drive_dev);
    project.patterns.push(Pattern {
        id: pat_id,
        name: "ModeAuto".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 16,
            mode: ValueMode::Hold,
            points: vec![ValuePoint {
                step: 0,
                value: 1.0,
            }],
        },
    });
    project.validate().unwrap();

    let mut app = App::new(project, "modes-sym.json".into()).unwrap();
    let snap = AudioSnapshot::default();

    // 1. Delay Effect deck: interactive Mode row, :param mode / :param delay_mode, MIDI Learn.
    app.execute_command(&format!("fx {delay_id}")).unwrap();
    let ctrls = app.control_descriptors_for(Detail::Effect(delay_id));
    assert_eq!(ctrls[0].label, "Mode");
    assert_eq!(ctrls[0].display, "Ping-Pong");
    app.control_cursor = 0;
    assert_eq!(
        app.learn_target_from_focus().map(|(t, _)| t),
        Some(MidiTarget::Param {
            address: ParamAddress::Effect {
                device: delay_id,
                param: EffectParam::DelayMode,
            }
        })
    );
    app.execute_command("param mode stereo").unwrap();
    assert!(matches!(
        app.effect(delay_id).unwrap().1.kind,
        EffectKind::Delay(p) if p.mode == DelayMode::Stereo
    ));
    app.execute_command("param delay_mode ping_pong").unwrap();
    assert!(matches!(
        app.effect(delay_id).unwrap().1.kind,
        EffectKind::Delay(p) if p.mode == DelayMode::PingPong
    ));

    // 2. Drive Effect deck: interactive Mode row, :param mode / :param drive_mode, MIDI Learn.
    app.execute_command(&format!("fx {drive_id}")).unwrap();
    let dctrls = app.control_descriptors_for(Detail::Effect(drive_id));
    assert_eq!(dctrls[0].label, "Mode");
    assert_eq!(dctrls[0].display, "Tape");
    app.control_cursor = 0;
    assert_eq!(
        app.learn_target_from_focus().map(|(t, _)| t),
        Some(MidiTarget::Param {
            address: ParamAddress::Effect {
                device: drive_id,
                param: EffectParam::DriveMode,
            }
        })
    );
    app.execute_command("param mode hard_clip").unwrap();
    assert!(matches!(
        app.effect(drive_id).unwrap().1.kind,
        EffectKind::Drive(p) if p.mode == DriveMode::HardClip
    ));
    app.execute_command("param drive_mode fold").unwrap();
    assert!(matches!(
        app.effect(drive_id).unwrap().1.kind,
        EffectKind::Drive(p) if p.mode == DriveMode::Fold
    ));

    // 3. Macro binding with contextual `mode` drives live DriveVisual mode via live_params!
    app.execute_command("macro 1 bind fx ModeDrive mode 0 1")
        .unwrap();
    for (val, expected) in [
        ("0", DriveMode::Tape),
        ("0.5", DriveMode::HardClip),
        ("1", DriveMode::Fold),
    ] {
        app.execute_command(&format!("macro 1 {val}")).unwrap();
        let fv_macro = app.frame_view(&snap, "ok");
        let vis = fv_macro
            .decks
            .iter()
            .find_map(|d| match &d.view {
                DeckView::Effect(e) => Some(&e.visual),
                _ => None,
            })
            .expect("Drive EffectDeckView");
        assert!(
            matches!(vis, EffectVisualView::Drive { mode, .. } if *mode == expected),
            "macro 1 {val} must project DriveMode::{expected:?}, got {vis:?}"
        );
    }

    // 4. Modulation targeting contextual `mode` on Delay drives live DelayVisual mode!
    app.execute_command("mod DelayModeMod effect ModeDelay mode ModeAuto")
        .unwrap();
    app.execute_command(&format!("fx {delay_id}")).unwrap();
    let fv_mod = app.frame_view(&snap, "ok");
    let dvis = fv_mod
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Effect(e) => Some(&e.visual),
            _ => None,
        })
        .expect("Delay EffectDeckView");
    assert!(matches!(
        dvis,
        EffectVisualView::Delay {
            mode: DelayMode::Stereo,
            ..
        }
    ));
}

#[test]
fn arrange_and_pattern_zoom_are_monotonic_across_short_and_long_horizons_and_focus_gated() {
    use cadence_core::material::{Pattern, PatternKind};
    use cadence_core::music::Step;

    // 1. Short 4-bar and 12-bar songs: `-` at full song stays 100%, `=` zooms in monotonically, `-` returns to 100%.
    for song_bars in [4usize, 12] {
        let mut proj = Project::empty();
        proj.bars = song_bars;
        let mut app = App::new(proj, "short-zoom.json".into()).unwrap();
        assert_eq!(app.arrange_visible_bars(), song_bars);
        assert!((app.arrange_effective_zoom() - 1.0).abs() < 1e-6);

        app.zoom_view(-1);
        assert_eq!(app.arrange_visible_bars(), song_bars);
        assert!((app.arrange_ui.zoom - 1.0).abs() < 1e-6);
        assert!(
            app.status.contains("100%")
                && app
                    .status
                    .contains(&format!("full song ({song_bars} bars)")),
            "unexpected full-song zoom status on {song_bars} bars: {}",
            app.status
        );

        let mut prev_bars = song_bars;
        let mut prev_zoom = app.arrange_ui.zoom;
        while prev_bars > 1 {
            app.zoom_view(1);
            let cur_bars = app.arrange_visible_bars();
            assert!(cur_bars < prev_bars, "zoom in must reduce visible bars");
            assert!(
                app.arrange_ui.zoom > prev_zoom,
                "zoom in must increase zoom factor"
            );
            prev_bars = cur_bars;
            prev_zoom = app.arrange_ui.zoom;
        }
        assert_eq!(prev_bars, 1);

        while prev_bars < song_bars {
            app.zoom_view(-1);
            let cur_bars = app.arrange_visible_bars();
            assert!(cur_bars > prev_bars, "zoom out must increase visible bars");
            assert!(
                app.arrange_ui.zoom < prev_zoom,
                "zoom out must decrease zoom factor"
            );
            prev_bars = cur_bars;
            prev_zoom = app.arrange_ui.zoom;
        }
        assert_eq!(prev_bars, song_bars);
        assert!((app.arrange_ui.zoom - 1.0).abs() < 1e-4);
        assert!(app.status.contains("100%") && app.status.contains("full song"));
    }

    // 2. Long 80-bar and 128-bar songs: zoom out past 64 bars cleanly with accurate status and page_steps.
    for (song_bars, expect_max, expect_full) in [
        (80usize, 80usize, true),
        (128, 128, true),
        (160, 128, false),
    ] {
        let mut proj = Project::empty();
        proj.bars = song_bars;
        let mut app = App::new(proj, "long-zoom.json".into()).unwrap();
        assert_eq!(app.arrange_visible_bars(), 16);
        assert_eq!(app.arrange_page_steps(), 16 * 16);

        let mut prev_bars = 16;
        let mut prev_zoom = 1.0_f32;
        for _ in 0..20 {
            app.zoom_view(-1);
            let cur_bars = app.arrange_visible_bars();
            assert!(cur_bars >= prev_bars);
            assert!(app.arrange_ui.zoom <= prev_zoom + 1e-6);
            assert_eq!(app.arrange_page_steps(), cur_bars * 16);
            prev_bars = cur_bars;
            prev_zoom = app.arrange_ui.zoom;
            if cur_bars == expect_max {
                break;
            }
        }
        assert_eq!(
            prev_bars, expect_max,
            "song_bars={song_bars} must reach {expect_max} visible bars"
        );
        let fv = app.frame_view(&AudioSnapshot::default(), "silent");
        let arrange = fv
            .decks
            .iter()
            .find_map(|d| match &d.view {
                DeckView::Arrange(a) => Some(a),
                _ => None,
            })
            .expect("arrange deck");
        assert_eq!(arrange.page_steps, expect_max * 16);
        assert_eq!(
            app.status.contains("full song"),
            expect_full,
            "status={}",
            app.status
        );
    }

    // 3. Short 8-step Pattern: `-` at full pattern stays 100%, `=` zooms in monotonically to 4 steps, `-` returns to 100%.
    let mut pat_proj = Project::demo();
    let short_id = pat_proj.max_id() + 1;
    pat_proj.patterns.push(Pattern {
        id: short_id,
        name: "EightStep".into(),
        kind: PatternKind::Notes {
            steps: vec![Step::default(); 8],
        },
    });
    let mut app = App::new(pat_proj, "short-pat.json".into()).unwrap();
    app.apply(crate::Action::SelectPattern { pattern: short_id })
        .unwrap();
    app.press_keys("gp").unwrap();
    assert_eq!(app.pattern_visible_steps(8), 8);
    app.zoom_view(-1);
    assert_eq!(app.pattern_visible_steps(8), 8);
    assert!((app.view_state.pattern_zoom - 1.0).abs() < 1e-6);
    assert!(
        app.status.contains("100%") && app.status.contains("full pattern (8 steps)"),
        "unexpected 8-step full pattern status: {}",
        app.status
    );

    let mut prev_steps = 8;
    let mut prev_zoom = app.view_state.pattern_zoom;
    while prev_steps > 4 {
        app.zoom_view(1);
        let cur = app.pattern_visible_steps(8);
        assert!(cur < prev_steps);
        assert!(app.view_state.pattern_zoom > prev_zoom);
        prev_steps = cur;
        prev_zoom = app.view_state.pattern_zoom;
    }
    assert_eq!(prev_steps, 4);
    while prev_steps < 8 {
        app.zoom_view(-1);
        let cur = app.pattern_visible_steps(8);
        assert!(cur > prev_steps);
        assert!(app.view_state.pattern_zoom < prev_zoom);
        prev_steps = cur;
        prev_zoom = app.view_state.pattern_zoom;
    }
    assert!((app.view_state.pattern_zoom - 1.0).abs() < 1e-4);

    // 4. Focus gating: zoom keys and helpers must not mutate Arrange/Pattern when other decks are focused.
    let mut app = demo_app();
    app.arrange_ui.zoom = 2.0;
    app.view_state.pattern_zoom = 2.0;
    for focus_keys in ["gm", "gb", "gi", "gk", "gG"] {
        app.press_keys(focus_keys).unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Char('='), KeyModifiers::CONTROL))
            .unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Char('-'), KeyModifiers::CONTROL))
            .unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Char('0'), KeyModifiers::CONTROL))
            .unwrap();
        assert!(
            (app.arrange_ui.zoom - 2.0).abs() < 1e-6,
            "focus {focus_keys} must not mutate arrange zoom"
        );
        assert!(
            (app.view_state.pattern_zoom - 2.0).abs() < 1e-6,
            "focus {focus_keys} must not mutate pattern zoom"
        );
        assert!(
            app.status.contains("Arrange, Pattern, or Sample"),
            "focus {focus_keys} must report zoom hint, got {}",
            app.status
        );
    }
}

#[test]
fn arrange_tail_clamping_and_paging_stride_match_visible_bars() {
    let mut proj = Project::empty();
    proj.bars = 12;
    let mut app = App::new(proj, "tail-clamp.json".into()).unwrap();
    // Zoom in once from 12 bars -> 10 bars.
    app.zoom_view(1);
    assert_eq!(app.arrange_visible_bars(), 10);
    assert_eq!(app.arrange_page_steps(), 160);

    // Cursor on bar 11 (step 160): viewport clamps back to bar 3 (step 32), showing bars 3..=12 full width.
    app.arrange_ui.step = 160;
    let fv = app.frame_view(&AudioSnapshot::default(), "silent");
    let arrange = fv
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Arrange(a) => Some(a),
            _ => None,
        })
        .expect("arrange deck");
    assert_eq!(arrange.page_start_step, 32);
    assert_eq!(arrange.page_steps, 160);

    // PageDown/PageUp in Overview uses arrange_visible_bars() * 16 (no stale 64-step default).
    app.press_keys("vo").unwrap();
    app.arrange_ui.step = 0;
    app.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE))
        .unwrap();
    assert_eq!(app.arrange_ui.step, 160);
    app.handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE))
        .unwrap();
    assert_eq!(app.arrange_ui.step, 0);
}

#[test]
fn arrange_smooth_scrolloff_tracks_cursor_walk_follow_and_handoff() {
    let mut proj = Project::empty();
    proj.bars = 32;
    let mut app = App::new(proj, "scrolloff.json".into()).unwrap();
    app.press_keys("ga").unwrap();
    // Zoom in from 16 bars -> 8 bars (scrolloff = 2 bars).
    app.arrange_ui.zoom = 2.0;
    assert_eq!(app.arrange_visible_bars(), 8);

    // 1. Walk forward bar-by-bar from bar 1 (idx 0): viewport holds at 0 through bar 6 (idx 5),
    //    then glides +1 bar per bar keeping 2 bars of trailing lookahead.
    for bar in 0..10usize {
        if bar > 0 {
            app.press_keys("l").unwrap();
        }
        let fv = app.frame_view(&AudioSnapshot::default(), "silent");
        let arr = fv
            .decks
            .iter()
            .find_map(|d| match &d.view {
                DeckView::Arrange(a) => Some(a),
                _ => None,
            })
            .expect("arrange deck");
        assert_eq!(arr.cursor_step, (bar * 16) as u32);
        assert_eq!(
            arr.page_start_step,
            bar.saturating_sub(5) * 16,
            "forward cursor walk at bar {}",
            bar + 1
        );
    }

    // 2. Reverse 1 bar (bar 10 -> bar 9): stays inside interior dead-zone (viewport unchanged).
    app.press_keys("h").unwrap();
    let fv_back1 = app.frame_view(&AudioSnapshot::default(), "silent");
    let arr_back1 = fv_back1
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Arrange(a) => Some(a),
            _ => None,
        })
        .expect("arrange deck");
    assert_eq!(arr_back1.page_start_step, 4 * 16);

    // 3. Playhead Follow glides bar-by-bar and hands off seamlessly when `l` suspends Follow.
    app.press_keys("0tf").unwrap();
    let mut last_start = 0usize;
    for bar in 0..12usize {
        let step = (bar * 16 + 4) as u64;
        app.observe_transport(step, step + 1, true, false);
        let snap = AudioSnapshot {
            playing: true,
            absolute_step: step,
            ..AudioSnapshot::default()
        };
        let fv = app.frame_view(&snap, "silent");
        let arr = fv
            .decks
            .iter()
            .find_map(|d| match &d.view {
                DeckView::Arrange(a) => Some(a),
                _ => None,
            })
            .expect("arrange deck");
        assert_eq!(arr.page_start_step, bar.saturating_sub(5) * 16);
        last_start = arr.page_start_step;
    }
    // Pressing `h` suspends Follow at playhead bar 12 (step 176 -> 160), staying inside dead-zone!
    app.press_keys("h").unwrap();
    assert_eq!(
        app.follow_mode(),
        cadence_core::follow::FollowMode::Suspended
    );
    let fv_handoff = app.frame_view(&AudioSnapshot::default(), "silent");
    let arr_handoff = fv_handoff
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Arrange(a) => Some(a),
            _ => None,
        })
        .expect("arrange deck");
    assert_eq!(
        arr_handoff.page_start_step, last_start,
        "suspending Follow with h must hand off without snapping viewport"
    );
}

#[test]
fn monitoring_ergonomics_top_bar_cpu_auto_meter_and_arrange_lane_meters() {
    let mut app = demo_app();
    let ch0 = app.project().instruments[0].channel;
    let snap = AudioSnapshot {
        sample_rate: 48_000,
        playing: true,
        peak_left: 0.72,
        peak_right: 0.65,
        health: cadence_core::runtime::AudioHealth {
            cpu_load: 0.24,
            peak_cpu_load: 0.41,
            overloaded: false,
            failed: false,
            xrun_count: 0,
            ..Default::default()
        },
        channel_meters: vec![cadence_core::runtime::ChannelMeter {
            channel: ChannelRef::Channel(ch0),
            input_peak: [0.5, 0.4],
            input_peak_hold: [0.6, 0.5],
            output_peak: [0.78, 0.62],
            output_peak_hold: [0.82, 0.68],
            hot: false,
            clipped: false,
        }],
        ..AudioSnapshot::default()
    };

    // Default: Output Scope visible -> Auto compact top-bar meter is hidden; DSP CPU pill is visible;
    // Arrange lane meters are hidden.
    let fv_default = app.frame_view(&snap, "Metal · 48 kHz");
    assert!(fv_default.chrome.show_dsp_cpu);
    assert!(!fv_default.chrome.show_compact_meter);
    let arr0 = fv_default
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Arrange(a) => Some(a),
            _ => None,
        })
        .expect("arrange deck");
    assert!(!arr0.show_lane_meters);
    assert!(arr0.lanes.iter().all(|l| l.meter_peak.is_none()));

    // Hiding Output Scope (`go`) automatically reveals the compact top-bar stereo meter under Auto!
    app.press_keys("go").unwrap();
    let fv_hidden_out = app.frame_view(&snap, "Metal · 48 kHz");
    assert!(fv_hidden_out.chrome.show_compact_meter);

    // Restoring Output Scope hides the Auto compact meter again.
    app.press_keys("go").unwrap();
    assert!(
        !app.frame_view(&snap, "Metal · 48 kHz")
            .chrome
            .show_compact_meter
    );

    // Palette / helpers toggle Always/Off meter, DSP CPU, and Arrange lane meters cleanly.
    app.cycle_top_bar_meter(1);
    assert_eq!(
        app.config.top_bar_meter,
        cadence_core::config::TopBarMeterMode::Always
    );
    assert!(
        app.frame_view(&snap, "Metal · 48 kHz")
            .chrome
            .show_compact_meter
    );
    app.cycle_top_bar_meter(1);
    assert_eq!(
        app.config.top_bar_meter,
        cadence_core::config::TopBarMeterMode::Off
    );
    app.press_keys("go").unwrap();
    assert!(
        !app.frame_view(&snap, "Metal · 48 kHz")
            .chrome
            .show_compact_meter
    );

    app.set_top_bar_cpu_visible(false);
    assert!(!app.frame_view(&snap, "Metal · 48 kHz").chrome.show_dsp_cpu);

    app.set_arrange_lane_meters_visible(true);
    let fv_lane_meters = app.frame_view(&snap, "Metal · 48 kHz");
    let arr1 = fv_lane_meters
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Arrange(a) => Some(a),
            _ => None,
        })
        .expect("arrange deck");
    assert!(arr1.show_lane_meters);
    assert!(
        arr1.lanes
            .iter()
            .any(|l| l.meter_peak == Some([0.78, 0.62])),
        "lane routed to ch0 must expose live stereo peak meter"
    );
}

#[test]
fn pattern_follow_stays_active_and_tracks_playhead_while_zoomed() {
    let mut app = demo_app();
    // Pattern 4 = Moving lead (period 320, active across bars 5..=16, steps 64..256).
    app.apply(crate::Action::SelectPattern { pattern: 4 })
        .unwrap();
    app.press_keys("gp").unwrap();
    app.cursor = 0;
    app.press_keys("tf").unwrap();
    assert_eq!(app.follow_mode(), cadence_core::follow::FollowMode::Active);

    // Zoom in with bare `=` to 8 steps — Follow must remain active!
    while app.pattern_visible_steps(320) > 8 {
        app.press_keys("=").unwrap();
    }
    assert_eq!(app.pattern_visible_steps(320), 8);
    assert_eq!(app.follow_mode(), cadence_core::follow::FollowMode::Active);

    let snap = AudioSnapshot {
        playing: true,
        absolute_step: 83,
        step_fraction: 0.25,
        sample_rate: 48_000,
        ..AudioSnapshot::default()
    };
    let fv_in = app.frame_view(&snap, "silent");
    let pat_in = fv_in
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Pattern(p) => Some(p),
            _ => None,
        })
        .expect("pattern deck");
    assert_eq!(pat_in.page_steps, 8);
    assert_eq!(pat_in.page_start, 80);

    // Zoom out past 32 steps — Follow tracks playhead 83 onto its zoomed-out page cleanly.
    while app.pattern_visible_steps(320) < 32 {
        app.press_keys("-").unwrap();
    }
    let vis_out = app.pattern_visible_steps(320);
    assert!(vis_out >= 32);
    assert_eq!(app.follow_mode(), cadence_core::follow::FollowMode::Active);

    let fv_out = app.frame_view(&snap, "silent");
    let pat_out = fv_out
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Pattern(p) => Some(p),
            _ => None,
        })
        .expect("pattern deck");
    assert_eq!(pat_out.page_steps, vis_out);
    assert_eq!(pat_out.page_start, (83 / vis_out) * vis_out);
}

#[test]
fn pattern_view_projects_paused_next_step_and_distinguishes_song_usage_states() {
    use cadence_core::material::{
        Edit, MacroControl, Modulation, ModulationTarget, Pattern, PatternKind, Placement,
    };
    use cadence_core::mixer::{EffectDevice, EffectKind};
    use cadence_core::music::{EndRule, Step};
    use cadence_core::playback::PlaybackUse;

    let pattern_view_of = |app: &App, snap: &AudioSnapshot| -> PatternDeckView {
        app.frame_view(snap, "silent")
            .decks
            .into_iter()
            .find_map(|d| match d.view {
                DeckView::Pattern(p) => Some(p),
                _ => None,
            })
            .expect("pattern deck")
    };

    let mut app = demo_app();
    // 1. Placed at bars 5..=16 while paused at bar 1 -> "Placed (bars 5–16)", never "No active song use".
    app.apply(crate::Action::SelectPattern { pattern: 4 })
        .unwrap();
    app.press_keys("gp").unwrap();
    let paused_snap = AudioSnapshot {
        playing: false,
        absolute_step: 0,
        ..AudioSnapshot::default()
    };
    let idle = pattern_view_of(&app, &paused_snap);
    assert_eq!(idle.playback_status, "Placed (bars 5–16)");
    assert!(idle.playheads.is_empty());

    // 2. Cueing while paused advances transport_next() to 0-based bar 4 (1-based bar 5, step 64)
    //    even while snapshot.absolute_step stays 0.
    app.apply(crate::Action::CueBar { bar: 4 }).unwrap();
    let cued = pattern_view_of(&app, &paused_snap);
    assert_eq!(cued.playback_status, "∥ PAUSED step 65");
    assert_eq!(cued.playheads.len(), 1);
    assert!((cued.playheads[0].position - 64.0).abs() < 1e-6);

    // 3. Muting the active lane at bar 5 reports "Lane muted".
    app.apply(crate::Action::SetLaneMuted {
        lane: 1,
        muted: true,
    })
    .unwrap();
    let muted = pattern_view_of(&app, &paused_snap);
    assert_eq!(muted.playback_status, "Lane muted");
    app.apply(crate::Action::SetLaneMuted {
        lane: 1,
        muted: false,
    })
    .unwrap();

    // 4. EndRule::Stop past song end reports "At song end".
    app.apply(crate::Action::Edit {
        edit: Edit::SetEndRule { end: EndRule::Stop },
    })
    .unwrap();
    let past_end_snap = AudioSnapshot {
        playing: true,
        absolute_step: 300,
        ..AudioSnapshot::default()
    };
    let at_end = pattern_view_of(&app, &past_end_snap);
    assert_eq!(at_end.playback_status, "At song end");

    // 5. Truly unreferenced pattern reports "Unused in song".
    let unused_id = app.project().max_id() + 1;
    app.apply(crate::Action::Edit {
        edit: Edit::PutPattern {
            pattern: Pattern {
                id: unused_id,
                name: "Spare".into(),
                kind: PatternKind::Notes {
                    steps: vec![Step::default(); 16],
                },
            },
        },
    })
    .unwrap();
    app.apply(crate::Action::SelectPattern { pattern: unused_id })
        .unwrap();
    let unused = pattern_view_of(&app, &paused_snap);
    assert_eq!(unused.playback_status, "Unused in song");

    // 6. Disjoint placements report "Placed · idle here" when playhead sits in the gap.
    let mut disjoint_proj = Project::demo();
    let part_id = disjoint_proj.parts[0].id;
    disjoint_proj.lanes[0].placements = vec![
        Placement::new(101, part_id, 0, 16),
        Placement::new(102, part_id, 64, 16),
    ];
    let mut disjoint_app = App::new(disjoint_proj, "disjoint.json".into()).unwrap();
    disjoint_app
        .apply(crate::Action::SelectPattern { pattern: 1 })
        .unwrap();
    disjoint_app.press_keys("gp").unwrap();
    let gap_snap = AudioSnapshot {
        playing: true,
        absolute_step: 32,
        ..AudioSnapshot::default()
    };
    assert_eq!(
        pattern_view_of(&disjoint_app, &gap_snap).playback_status,
        "Placed · idle here"
    );

    // 7. Filter.env_pattern_id shows live playhead in Song clock and "At song end · Filter Env" at PastEnd.
    let mut fx_proj = Project::demo();
    fx_proj.end = EndRule::Stop;
    let env_pat_id = fx_proj.max_id() + 1;
    fx_proj.patterns.push(Pattern {
        id: env_pat_id,
        name: "PumpEnv".into(),
        kind: PatternKind::Rhythm {
            steps: (0..16).map(|i| i % 4 == 0).collect(),
        },
    });
    let flt_id = env_pat_id + 1;
    let mut flt = EffectDevice::filter(flt_id);
    if let EffectKind::Filter(ref mut fp) = flt.kind {
        fp.env_pattern_id = Some(env_pat_id);
    }
    fx_proj.mixer.channels[0].inserts.push(flt);

    let macro_pat_id = flt_id + 1;
    fx_proj.patterns.push(Pattern {
        id: macro_pat_id,
        name: "MacroRamp".into(),
        kind: PatternKind::Values {
            unit: cadence_core::material::ValueUnit::Normalized,
            period: 16,
            mode: cadence_core::params::ValueMode::Ramp,
            points: vec![cadence_core::material::ValuePoint {
                step: 0,
                value: 0.5,
            }],
        },
    });
    fx_proj.macros.push(MacroControl::new(0, "Tone"));
    fx_proj.modulations.push(Modulation {
        id: macro_pat_id + 1,
        name: "DriveM1".into(),
        target: ModulationTarget::Macro { index: 0 },
        pattern: macro_pat_id,
    });
    let mut fx_app = App::new(fx_proj, "fx-status.json".into()).unwrap();
    fx_app
        .apply(crate::Action::SelectPattern {
            pattern: env_pat_id,
        })
        .unwrap();
    fx_app.press_keys("gp").unwrap();
    let playing_snap = AudioSnapshot {
        playing: true,
        absolute_step: 4,
        ..AudioSnapshot::default()
    };
    let flt_live = pattern_view_of(&fx_app, &playing_snap);
    assert_eq!(flt_live.playback_status, "▶ PLAY step 5");
    assert_eq!(flt_live.playheads.len(), 1);
    assert_eq!(flt_live.playheads[0].kind, PlaybackUse::Modulation);

    assert_eq!(
        pattern_view_of(&fx_app, &past_end_snap).playback_status,
        "At song end · Filter Env"
    );
    fx_app
        .apply(crate::Action::SelectPattern {
            pattern: macro_pat_id,
        })
        .unwrap();
    assert_eq!(
        pattern_view_of(&fx_app, &past_end_snap).playback_status,
        "At song end · drives M1"
    );
}
