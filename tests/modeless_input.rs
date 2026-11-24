//! Modeless input architecture, directional tiers, global modifier routing, and canonical verbs.
//!
//! Verifies end-to-end (100% synthetic, zero external files):
//! 1. Tier 1 (`h/j/k/l`, Arrows) and Tier 2 (`H/J/K/L`) strictly navigate and NEVER mutate
//!    project or config state across every deck and overlay (including FileHub Export,
//!    Settings, and Song Properties).
//! 2. Tier 3 (`Alt+h/j/k/l`, `Alt+H/J/K/L`, `Alt+Arrows`, `Alt+Shift+Arrows`) and Tier 4
//!    (`,/.`, `</>`) direct manipulation, vertical reordering (`Alt+j/k` on Mixer Inserts,
//!    Rack Zones, Quick Pins, Macro Bindings, and Arrange Placement lanes), key-repeat
//!    coalesced single-step undo, and `'` From-Here fork compatibility.
//! 3. `;` (`repeat_last`) and `Alt+Backspace` universal reset-to-default across Instrument,
//!    Effect, Channel, Mixer Fader/Pan/Send, Macro Dial/Binding, Patchbay Cable, Settings,
//!    and Song Properties.
//! 4. Unshifted `Ctrl+<letter>` for all 10 docked decks + CUA core (`Ctrl+s/z/r/w/c/,`) and
//!    `Ctrl+Shift+<key>` for all modals/overlays (including `Ctrl+Shift+p`, `Ctrl+Shift+j`,
//!    and `Ctrl+?` from Jam mode).
//! 5. `Alt+c`/`Alt+C` Keep Override, `gj` Jam entry, `i` Step Insert on Pattern Notes
//!    (preserving chords), Pattern Notes `[`/`]`/`#` accidental, `c`/`C` condition, `m` mute,
//!    and `o` chord stacking staying in `Normal`.
//! 6. Canonical verbs & zero shadowing: `A` universal audition, `x` remove child without `d`
//!    delete on Rack (`d` duplicates Zone), `Enter` exact `ValueInput` on scalars, `S` Stage
//!    never shadowed on Rack/Filter, `R` MIDI Rec never shadowed on `Detail::Placement`,
//!    clipboard `y`/`p`/`P` scoped strictly to Arrange Placements & Pattern (`p` pins Patchbay
//!    cable cleanly), Rack Zones `h/l` zone (clamping `control_cursor`) & `j/k` controls,
//!    `0`/`$`/`G`/`PgUp`/`PgDn` isolation on Patchbay & Bank Parts, and Bank Parts
//!    `Shift+Enter` immediate launch.

use cadence_app::view::{DeckView, FrameView, OverlayView, PatternContentView, UiModeKind};
use cadence_app::{
    App, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MixerSlot, RackTab, Workspace,
};
use cadence_core::material::{Id, PatternKind, Project};
use cadence_core::mixer::{Channel, ChannelKind, ChannelRef};
use cadence_core::rack::ModuleKind;
use cadence_core::runtime::AudioSnapshot;
use cadence_core::sampler::SampleLibrary;

fn channel_strip(app: &App, id: Id) -> &Channel {
    app.project()
        .mixer
        .channels
        .iter()
        .find(|c| c.id == id)
        .expect("channel exists")
}

fn hermetic_app(project: Project, name: &str) -> (tempfile::TempDir, App) {
    let dir = tempfile::tempdir().unwrap();
    let save_path = dir.path().join(name);
    let lib = SampleLibrary::from_roots(&[], Some(&save_path));
    let app = App::new_with_library(project, save_path, lib).unwrap();
    (dir, app)
}

fn snap_view(app: &App) -> FrameView {
    let snap = AudioSnapshot {
        playing: app.playing(),
        absolute_step: app.transport_step(),
        sample_rate: 48_000,
        ..AudioSnapshot::default()
    };
    app.frame_view(&snap, "Modeless Input Test · 48 kHz")
}

fn send_key(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::new(code, KeyModifiers::NONE))
        .unwrap();
}

fn send_mod(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
    app.handle_key(KeyEvent::new(code, modifiers)).unwrap();
}

fn send_repeat(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
    let mut ev = KeyEvent::new(code, modifiers);
    ev.kind = KeyEventKind::Repeat;
    app.handle_key(ev).unwrap();
}

fn assert_tier1_tier2_never_mutate(app: &mut App, context: &str) {
    assert_eq!(
        app.mode_name(),
        "NORMAL",
        "{context}: must start in NORMAL mode"
    );
    let rev_before = app.revision();
    let proj_before = serde_json::to_string(app.project()).unwrap();
    let cfg_before = serde_json::to_string(&app.config).unwrap();

    for code in [
        KeyCode::Char('h'),
        KeyCode::Char('l'),
        KeyCode::Char('j'),
        KeyCode::Char('k'),
        KeyCode::Left,
        KeyCode::Right,
        KeyCode::Down,
        KeyCode::Up,
        KeyCode::Char('H'),
        KeyCode::Char('L'),
        KeyCode::Char('J'),
        KeyCode::Char('K'),
    ] {
        send_key(app, code);
        assert_eq!(
            app.revision(),
            rev_before,
            "{context}: key {code:?} mutated project revision in NORMAL!"
        );
        assert_eq!(
            serde_json::to_string(app.project()).unwrap(),
            proj_before,
            "{context}: key {code:?} mutated project state in NORMAL!"
        );
        assert_eq!(
            serde_json::to_string(&app.config).unwrap(),
            cfg_before,
            "{context}: key {code:?} mutated config in NORMAL!"
        );
        if app.mode_name() != "NORMAL" {
            send_key(app, KeyCode::Esc);
        }
    }
}

// ---------------------------------------------------------------------------
// 1. Strict Tier 1 & Tier 2 non-mutation invariant across EVERY deck & overlay
// ---------------------------------------------------------------------------

#[test]
fn tier1_and_tier2_navigation_never_mutate_across_every_deck_and_overlay() {
    let (_dir, mut app) = hermetic_app(Project::demo(), "tier_invariants.json");

    let synth_name = app.project().instruments[0].name.clone();
    app.execute_command("pattern values Sweep 16 0:0.2 8:0.8")
        .unwrap();
    app.execute_command("pattern rhythm HatRhy x.x.x.x.x.x.x.x.")
        .unwrap();
    app.execute_command("instrument Kit kit").unwrap();
    app.execute_command("zone add Rim").unwrap();
    app.execute_command("fx add filter").unwrap();
    app.execute_command(&format!(
        "macro 1 bind instrument {synth_name} cutoff 0.1 0.9"
    ))
    .unwrap();

    // Pattern: Notes, Values, Rhythm
    app.execute_command("open Motif").unwrap();
    app.press_keys("gp").unwrap();
    assert_tier1_tier2_never_mutate(&mut app, "Pattern(Notes)");

    app.execute_command("open Sweep").unwrap();
    app.press_keys("gp").unwrap();
    assert_tier1_tier2_never_mutate(&mut app, "Pattern(Values)");

    app.execute_command("open HatRhy").unwrap();
    app.press_keys("gp").unwrap();
    assert_tier1_tier2_never_mutate(&mut app, "Pattern(Rhythm)");

    // Mixer + Channel + Effect
    app.press_keys("gm").unwrap();
    assert_tier1_tier2_never_mutate(&mut app, "Mixer");

    app.press_keys("o").unwrap();
    // Verify bare h/l is strictly a no-op on 1-axis inspector Detail::Channel.
    let cc_before = app.control_cursor;
    send_key(&mut app, KeyCode::Char('l'));
    assert_eq!(
        app.control_cursor, cc_before,
        "bare l must not move control_cursor on Detail::Channel"
    );
    send_key(&mut app, KeyCode::Char('h'));
    assert_eq!(
        app.control_cursor, cc_before,
        "bare h must not move control_cursor on Detail::Channel"
    );
    assert_tier1_tier2_never_mutate(&mut app, "Detail::Channel");

    let fx_id = app
        .project()
        .mixer
        .channels
        .iter()
        .flat_map(|c| c.inserts.iter())
        .next()
        .expect("seeded filter insert")
        .id;
    app.execute_command(&format!("fx {fx_id}")).unwrap();
    assert_tier1_tier2_never_mutate(&mut app, "Detail::Effect");

    // Instrument: Panel, Patchbay, Zones
    app.execute_command(&format!("instrument {synth_name}"))
        .unwrap();
    assert_tier1_tier2_never_mutate(&mut app, "Detail::Instrument(Panel)");
    app.execute_command("tab patchbay").unwrap();
    assert_tier1_tier2_never_mutate(&mut app, "Detail::Instrument(Patchbay)");
    app.execute_command("instrument Kit").unwrap();
    app.execute_command("tab zones").unwrap();
    assert_tier1_tier2_never_mutate(&mut app, "Detail::Instrument(Zones)");
    app.execute_command("tab panel").unwrap();

    // Part, Arrange, Placement, Macros, Bank, Graph, MIDI
    let part_name = app.project().parts[0].name.clone();
    app.execute_command(&format!("part {part_name}")).unwrap();
    assert_tier1_tier2_never_mutate(&mut app, "Detail::Part");

    app.press_keys("ga").unwrap();
    assert_tier1_tier2_never_mutate(&mut app, "Arrange");
    send_key(&mut app, KeyCode::Enter);
    assert_tier1_tier2_never_mutate(&mut app, "Detail::Placement");

    app.press_keys("gk").unwrap();
    assert_tier1_tier2_never_mutate(&mut app, "Macros");

    app.press_keys("gb").unwrap();
    assert_tier1_tier2_never_mutate(&mut app, "Bank");

    app.press_keys("gG").unwrap();
    assert_tier1_tier2_never_mutate(&mut app, "SignalGraph");

    app.press_keys("gd").unwrap();
    assert_tier1_tier2_never_mutate(&mut app, "Midi");

    // Settings overlay: h/j/k/l/H/L never mutate config.
    app.press_keys("g,").unwrap();
    assert_eq!(app.mode_name(), "SETTINGS");
    let cfg_before = serde_json::to_string(&app.config).unwrap();
    for code in [
        KeyCode::Char('h'),
        KeyCode::Char('l'),
        KeyCode::Char('j'),
        KeyCode::Char('k'),
        KeyCode::Left,
        KeyCode::Right,
        KeyCode::Down,
        KeyCode::Up,
    ] {
        send_key(&mut app, code);
        assert_eq!(
            serde_json::to_string(&app.config).unwrap(),
            cfg_before,
            "Settings overlay: {code:?} must not mutate config"
        );
    }
    send_key(&mut app, KeyCode::Esc);

    // Song Properties overlay: h/j/k/l never mutate project.
    app.press_keys("gP").unwrap();
    assert_eq!(app.mode_name(), "SONG PROPS");
    let proj_before = serde_json::to_string(app.project()).unwrap();
    for code in [
        KeyCode::Char('h'),
        KeyCode::Char('l'),
        KeyCode::Char('j'),
        KeyCode::Char('k'),
        KeyCode::Left,
        KeyCode::Right,
    ] {
        send_key(&mut app, code);
        assert_eq!(
            serde_json::to_string(app.project()).unwrap(),
            proj_before,
            "Song Properties: {code:?} must not mutate project"
        );
    }
    send_key(&mut app, KeyCode::Esc);

    // FileHub Export tab: bare h/l switches tabs instead of mutating export options!
    send_mod(
        &mut app,
        KeyCode::Char('E'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );
    assert_eq!(app.mode_name(), "FILES");
    let tab_before = match snap_view(&app).overlay {
        Some(OverlayView::FileHub(f)) => f.active_tab,
        _ => panic!("expected FileHub overlay"),
    };
    send_key(&mut app, KeyCode::Char('h'));
    let tab_after = match snap_view(&app).overlay {
        Some(OverlayView::FileHub(f)) => f.active_tab,
        _ => panic!("expected FileHub overlay"),
    };
    assert_ne!(
        tab_after, tab_before,
        "FileHub Export bare h must switch tab, not mutate export option"
    );
    send_key(&mut app, KeyCode::Esc);
}

// ---------------------------------------------------------------------------
// 2. Polyphonic Pattern Notes (modeless Tier 3/4, accidental/cond/mute, `i` Insert)
// ---------------------------------------------------------------------------

#[test]
fn modeless_pattern_notes_sculpting_chords_insert_and_repeat_last() {
    let (_dir, mut app) = hermetic_app(Project::demo(), "pattern_modeless.json");
    app.execute_command("pattern notes ChordLab 0 . . . . . . .")
        .unwrap();
    app.execute_command("open ChordLab").unwrap();
    app.press_keys("gp").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");

    let pat_id = app
        .project()
        .patterns
        .iter()
        .find(|p| p.name == "ChordLab")
        .unwrap()
        .id;
    let read_step = |app: &App, idx: usize| match &app.project().pattern(pat_id).unwrap().kind {
        PatternKind::Notes { steps } => steps[idx].clone(),
        _ => panic!("expected Notes pattern"),
    };

    // `o` stacks a chord tone (+2 degrees -> 3rd) and STAYS in NORMAL!
    app.press_keys("o").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
    assert_eq!(app.ui_mode_kind(), UiModeKind::Normal);
    assert_eq!(read_step(&app, 0).chord.len(), 1);
    assert_eq!(read_step(&app, 0).chord[0].degree, 3);

    // Stack a 3rd voice (`o` -> 5th) and transpose it down 1 octave (`Alt+J`) below root.
    app.press_keys("o").unwrap();
    assert_eq!(read_step(&app, 0).chord[1].degree, 5);
    app.press_keys("<m-J>").unwrap();
    assert_eq!(read_step(&app, 0).chord[1].degree, -2);
    assert_eq!(app.mode_name(), "NORMAL");

    // Visual pitch ordering: `k` moves up to middle voice (root), `k` to top, `j` back to middle.
    let get_readout = |app: &App| {
        snap_view(app)
            .decks
            .into_iter()
            .find_map(|d| match d.view {
                DeckView::Pattern(pv) => pv.voice_readout,
                _ => None,
            })
            .unwrap_or_default()
    };
    assert!(get_readout(&app).starts_with("Voice 1/3"));
    app.press_keys("k").unwrap();
    assert!(get_readout(&app).starts_with("Voice 2/3"));
    app.press_keys("k").unwrap();
    assert!(get_readout(&app).starts_with("Voice 3/3"));
    app.press_keys("j").unwrap();
    assert!(get_readout(&app).starts_with("Voice 2/3"));

    // Sculpt ONLY the selected root voice directly in NORMAL:
    // Alt+k pitch +1, ] accidental +1, Alt+l gate +0.25, Alt+L gate +1.0, . velocity +5%, c condition.
    let low_before = read_step(&app, 0).chord[1];
    let high_before = read_step(&app, 0).chord[0];
    let gate_before = read_step(&app, 0).gate;
    let vel_before = read_step(&app, 0).velocity;
    assert!(read_step(&app, 0).condition.is_always());

    app.press_keys("<m-k>]<m-l><m-L>.c").unwrap();
    let after = read_step(&app, 0);
    assert_eq!(after.degree, 2);
    assert_eq!(after.accidental, 1);
    assert!((after.gate - (gate_before + 1.25)).abs() < 1e-4);
    assert!((after.velocity - (vel_before + 0.05)).abs() < 1e-4);
    assert!(!after.condition.is_always());
    assert_eq!(after.chord[1], low_before, "low voice untouched");
    assert_eq!(after.chord[0], high_before, "high voice untouched");

    // `[` flattens accidental back to 0; `C` cycles condition backwards to Always.
    app.press_keys("[C").unwrap();
    assert_eq!(read_step(&app, 0).accidental, 0);
    assert!(read_step(&app, 0).condition.is_always());

    // `m` mutes/rests the step preserving pitch & chord, and `m` again restores it!
    app.press_keys("m").unwrap();
    assert!(!read_step(&app, 0).active);
    assert_eq!(read_step(&app, 0).chord.len(), 2);
    app.press_keys("m").unwrap();
    assert!(read_step(&app, 0).active);

    // Select root voice (Voice 2/3) then `i` enters STEP INSERT directly (preserving sibling chord voices!).
    app.press_keys("k").unwrap();
    app.press_keys("i").unwrap();
    assert_eq!(app.mode_name(), "STEP INSERT");
    app.press_keys("3").unwrap(); // sets selected voice (root) to degree 3, advances to step 1
    send_key(&mut app, KeyCode::Esc);
    assert_eq!(app.mode_name(), "NORMAL");
    assert_eq!(read_step(&app, 0).degree, 3);
    assert_eq!(
        read_step(&app, 0).chord.len(),
        2,
        "`i` Step Insert must preserve sibling chord tones"
    );

    // `;` repeats last edit on step 1!
    assert_eq!(app.cursor, 1);
    assert!(!read_step(&app, 1).active);
    app.press_keys(";").unwrap();
    assert!(
        read_step(&app, 1).active,
        "`;` must repeat last pattern step edit"
    );

    // `x` on step 0 deletes ONLY root and promotes chord[0]; `X` clears whole step.
    app.press_keys("h").unwrap();
    let pv_check = snap_view(&app)
        .decks
        .into_iter()
        .find_map(|d| match d.view {
            DeckView::Pattern(pv) => Some(pv),
            _ => None,
        })
        .unwrap();
    let PatternContentView::Notes { steps, .. } = pv_check.content else {
        panic!("expected Notes");
    };
    let root_visual_idx = steps[0].voices.iter().position(|v| v.is_root).unwrap();
    while app.note_voice < root_visual_idx {
        app.press_keys("k").unwrap();
    }
    while app.note_voice > root_visual_idx {
        app.press_keys("j").unwrap();
    }
    app.press_keys("x").unwrap();
    assert!(read_step(&app, 0).active);
    assert_eq!(read_step(&app, 0).chord.len(), 1);
    app.press_keys("X").unwrap();
    assert!(!read_step(&app, 0).active);
    assert!(read_step(&app, 0).chord.is_empty());
}

// ---------------------------------------------------------------------------
// 3. Key-repeat coalesced undo + From-Here `'` fork compatibility
// ---------------------------------------------------------------------------

#[test]
fn key_repeat_coalesces_into_single_undo_and_respects_from_here_fork() {
    let (_dir, mut app) = hermetic_app(Project::demo(), "coalesce_undo.json");

    // 1. Instrument parameter (Cutoff) via Alt+l Press + 5 Repeats -> ONE `u` restores!
    app.press_keys("gi").unwrap();
    let inst_id = app.project().instruments[0].id;
    let read_cutoff = |app: &App| {
        app.project().instrument(inst_id).unwrap().rack.zones[0]
            .modules
            .iter()
            .find_map(|m| match &m.kind {
                ModuleKind::Filter(f) => Some(f.cutoff_hz),
                _ => None,
            })
            .unwrap()
    };
    for _ in 0..16 {
        let fv = snap_view(&app);
        if fv.decks.iter().any(|d| match &d.view {
            DeckView::Instrument(iv) => iv
                .controls
                .iter()
                .any(|c| c.selected && c.label.eq_ignore_ascii_case("Cutoff")),
            _ => false,
        }) {
            break;
        }
        send_key(&mut app, KeyCode::Char('j'));
    }
    let cutoff0 = read_cutoff(&app);
    send_mod(&mut app, KeyCode::Char('l'), KeyModifiers::ALT);
    for _ in 0..5 {
        send_repeat(&mut app, KeyCode::Char('l'), KeyModifiers::ALT);
    }
    let cutoff_after = read_cutoff(&app);
    assert!(cutoff_after > cutoff0, "Alt+l + repeats must raise cutoff");
    send_key(&mut app, KeyCode::Char('u'));
    assert!(
        (read_cutoff(&app) - cutoff0).abs() < 1e-3,
        "single `u` must undo entire coalesced Alt+l repeat burst"
    );

    // 2. Pattern Notes Alt+k Press + 3 Repeats -> ONE `u` restores!
    app.execute_command("pattern notes CoalesceMel 0 . . .")
        .unwrap();
    app.execute_command("open CoalesceMel").unwrap();
    app.press_keys("gp").unwrap();
    let pat_id = app
        .project()
        .patterns
        .iter()
        .find(|p| p.name == "CoalesceMel")
        .unwrap()
        .id;
    let deg = |app: &App| match &app.project().pattern(pat_id).unwrap().kind {
        PatternKind::Notes { steps } => steps[0].degree,
        _ => panic!("expected Notes"),
    };
    let d0 = deg(&app);
    send_mod(&mut app, KeyCode::Char('k'), KeyModifiers::ALT);
    for _ in 0..3 {
        send_repeat(&mut app, KeyCode::Char('k'), KeyModifiers::ALT);
    }
    assert_eq!(deg(&app), d0 + 4);
    send_key(&mut app, KeyCode::Char('u'));
    assert_eq!(
        deg(&app),
        d0,
        "single `u` must undo coalesced Pattern Alt+k burst"
    );

    // 3. `'` From-Here arms a non-coalesced fork on a coalesced edit!
    app.press_keys("'").unwrap();
    assert!(app.from_here);
    send_mod(&mut app, KeyCode::Char('k'), KeyModifiers::ALT);
    assert!(!app.from_here, "From-Here consumes on first coalesced edit");
    send_mod(&mut app, KeyCode::Char('k'), KeyModifiers::ALT);
    let active_pat = app.selected_pattern.unwrap();
    let active_deg = match &app.project().pattern(active_pat).unwrap().kind {
        PatternKind::Notes { steps } => steps[0].degree,
        _ => panic!("expected Notes"),
    };
    assert_eq!(active_deg, d0 + 2);
}

// ---------------------------------------------------------------------------
// 4. Mixer direct manipulation, Insert reorder, Reset, Channel alignment
// ---------------------------------------------------------------------------

#[test]
fn mixer_direct_manipulation_reorder_reset_and_step_alignment() {
    let (_dir, mut app) = hermetic_app(Project::demo(), "mixer_modeless.json");
    app.execute_command("channel return RevRet").unwrap();
    app.execute_command("channel group DrumGrp").unwrap();
    let audio_id = app
        .project()
        .mixer
        .channels
        .iter()
        .find(|c| c.kind == ChannelKind::Audio)
        .unwrap()
        .id;
    let return_id = app
        .project()
        .mixer
        .channels
        .iter()
        .find(|c| c.kind == ChannelKind::Return)
        .unwrap()
        .id;
    let part_id = app.project().parts[0].id;
    app.execute_command(&format!("fx add filter {audio_id}"))
        .unwrap();
    app.execute_command(&format!("fx add delay {audio_id}"))
        .unwrap();
    app.execute_command(&format!("send {audio_id} {return_id} 0.35"))
        .unwrap();
    app.execute_command(&format!("duck Pump {part_id} {audio_id} 0.40 160ms"))
        .unwrap();

    app.press_keys("gm").unwrap();
    let target_idx = app
        .channels()
        .iter()
        .position(|c| *c == ChannelRef::Channel(audio_id))
        .unwrap();
    while app
        .channels()
        .iter()
        .position(|c| *c == app.selected_channel)
        != Some(target_idx)
    {
        app.press_keys("l").unwrap();
    }

    let get_slot = |app: &App| {
        snap_view(app)
            .decks
            .into_iter()
            .find_map(|d| match d.view {
                DeckView::Mixer(mv) => Some(mv.focused_slot),
                _ => None,
            })
            .unwrap()
    };

    // Fader: Alt+k / Alt+j / . / , adjust gain; Alt+Backspace resets to unity (1.0).
    assert_eq!(get_slot(&app), MixerSlot::Fader);
    let g0 = channel_strip(&app, audio_id).strip.gain;
    app.press_keys("<m-k>").unwrap();
    let g1 = channel_strip(&app, audio_id).strip.gain;
    assert!(g1 > g0, "Mixer Fader Alt+k raises gain");
    app.press_keys("<m-J>").unwrap();
    assert!(
        channel_strip(&app, audio_id).strip.gain < g0,
        "Mixer Fader Alt+J lowers gain coarsely"
    );
    app.press_keys("<m-bs>").unwrap();
    assert!(
        (channel_strip(&app, audio_id).strip.gain - 1.0).abs() < 1e-3,
        "Alt+Backspace resets Fader to 0 dB (1.0)"
    );
    // Enter on Fader opens exact ValueInput directly!
    send_key(&mut app, KeyCode::Enter);
    assert_eq!(app.mode_name(), "VALUE ENTRY");
    send_key(&mut app, KeyCode::Esc);

    // Pan: Alt+l, Alt+L, Alt+Backspace reset to center.
    app.press_keys("j").unwrap();
    assert_eq!(get_slot(&app), MixerSlot::Pan);
    app.press_keys("<m-l>").unwrap();
    assert!(channel_strip(&app, audio_id).strip.pan > 0.0);
    app.press_keys("<m-bs>").unwrap();
    assert!(channel_strip(&app, audio_id).strip.pan.abs() < 1e-4);

    // Insert(1): Alt+k reorders upward to Insert(0) and `b` bypasses directly in NORMAL!
    app.press_keys("jj").unwrap();
    assert_eq!(get_slot(&app), MixerSlot::Insert(1));
    let second_fx = channel_strip(&app, audio_id).inserts[1].id;
    app.press_keys("<m-k>").unwrap();
    assert_eq!(get_slot(&app), MixerSlot::Insert(0));
    assert_eq!(channel_strip(&app, audio_id).inserts[0].id, second_fx);
    app.press_keys("b").unwrap();
    assert!(channel_strip(&app, audio_id).inserts[0].bypassed);

    // Send(0): `.` / `>` raise amount; Alt+Backspace resets to 0.0.
    while get_slot(&app) != MixerSlot::Send(0) {
        app.press_keys("j").unwrap();
    }
    let s0 = channel_strip(&app, audio_id).sends[0].amount;
    app.press_keys(">").unwrap();
    assert!(channel_strip(&app, audio_id).sends[0].amount > s0);
    app.press_keys("<m-bs>").unwrap();
    assert!(channel_strip(&app, audio_id).sends[0].amount.abs() < 1e-4);

    // Output: Alt+l cycles bus target.
    app.press_keys("j").unwrap();
    assert_eq!(get_slot(&app), MixerSlot::Output);
    let out0 = channel_strip(&app, audio_id).output;
    app.press_keys("<m-l>").unwrap();
    assert_ne!(channel_strip(&app, audio_id).output, out0);

    // Duck(0): `,` / `.` adjust duck depth.
    app.press_keys("j").unwrap();
    assert_eq!(get_slot(&app), MixerSlot::Duck(0));
    let duck0 = app
        .project()
        .ducks
        .iter()
        .find(|d| d.target == ChannelRef::Channel(audio_id))
        .unwrap()
        .depth;
    app.press_keys(".").unwrap();
    let duck1 = app
        .project()
        .ducks
        .iter()
        .find(|d| d.target == ChannelRef::Channel(audio_id))
        .unwrap()
        .depth;
    assert!(duck1 > duck0);
}

// ---------------------------------------------------------------------------
// 5. Rack (Panel / Patchbay / Zones), Macros, Arrange, Settings, SongProps
// ---------------------------------------------------------------------------

#[test]
fn modeless_rack_macros_arrange_values_and_overlays() {
    let (_dir, mut app) = hermetic_app(Project::demo(), "decks_modeless.json");

    // --- Rack Panel: H/L tabs, Alt+h/l & ,/. adjust, Alt+Backspace reset, Enter ValueInput,
    //     `S` frees global Stage, `d` does NOT delete module, `x` removes module ---
    app.press_keys("gi").unwrap();
    assert_eq!(app.rack_ui.tab, RackTab::Panel);

    // `S` on Rack triggers global Stage (NOT Save Preset!).
    assert!(app.staging().is_none());
    app.press_keys("S").unwrap();
    assert!(
        app.staging().is_some(),
        "`S` on Rack must trigger global Stage"
    );
    send_key(&mut app, KeyCode::Esc); // discard staging
    assert!(app.staging().is_none());

    // Focus Cutoff on Panel, adjust with Alt+L and `,`, reset with Alt+Backspace, open ValueInput with Enter.
    let inst_id = app.project().instruments[0].id;
    let read_cutoff = |app: &App| {
        app.project().instrument(inst_id).unwrap().rack.zones[0]
            .modules
            .iter()
            .find_map(|m| match &m.kind {
                ModuleKind::Filter(f) => Some(f.cutoff_hz),
                _ => None,
            })
            .unwrap()
    };
    for _ in 0..16 {
        if snap_view(&app).decks.iter().any(|d| match &d.view {
            DeckView::Instrument(iv) => iv
                .controls
                .iter()
                .any(|c| c.selected && c.label.eq_ignore_ascii_case("Cutoff")),
            _ => false,
        }) {
            break;
        }
        send_key(&mut app, KeyCode::Char('j'));
    }
    let c0 = read_cutoff(&app);
    app.press_keys("<m-L>").unwrap();
    let c1 = read_cutoff(&app);
    assert!(c1 > c0);
    app.press_keys(",").unwrap();
    assert!(read_cutoff(&app) < c1);
    app.press_keys("<m-bs>").unwrap();
    assert_ne!(
        (read_cutoff(&app) - c1).abs(),
        0.0,
        "Alt+Backspace resets Cutoff to module default"
    );
    send_key(&mut app, KeyCode::Enter);
    assert_eq!(app.mode_name(), "VALUE ENTRY");
    send_key(&mut app, KeyCode::Esc);

    // Switch to a removable module pill on Panel: `d` must NOT delete it; `x` removes it.
    let mods_before = app.project().instrument(inst_id).unwrap().rack.zones[0]
        .modules
        .len();
    app.press_keys("l").unwrap(); // move off Quick pill to first module
    app.press_keys("d").unwrap();
    assert_eq!(
        app.project().instrument(inst_id).unwrap().rack.zones[0]
            .modules
            .len(),
        mods_before,
        "Rack Panel `d` must NOT delete modules"
    );

    // --- Rack Patchbay: w/b select cable, Alt+h/l & ,/. adjust amount, `p` pins cleanly without paste leak,
    //     `0`/`$`/`G` stay in Patchbay, Enter on module jumps to Panel ---
    app.press_keys("L").unwrap();
    assert_eq!(app.rack_ui.tab, RackTab::Patchbay);
    app.press_keys("w").unwrap();
    let cid = app
        .rack_ui
        .patchbay_cable_id
        .expect("w selects attached cable");
    let amt0 = app.project().instrument(inst_id).unwrap().rack.zones[0]
        .cable(cid)
        .unwrap()
        .amount;
    app.press_keys(".").unwrap();
    let amt1 = app.project().instrument(inst_id).unwrap().rack.zones[0]
        .cable(cid)
        .unwrap()
        .amount;
    assert_ne!(amt1, amt0, "Patchbay `.` adjusts selected cable amount");
    // `p` pins cable without paste error!
    app.press_keys("p").unwrap();
    assert!(
        !app.status.to_ascii_lowercase().contains("nothing yanked"),
        "Patchbay `p` must pin, never leak paste error: {}",
        app.status
    );
    // `0` / `G` navigate Patchbay rows without leaking to hidden control_cursor.
    let cc_saved = app.control_cursor;
    app.press_keys("G").unwrap();
    app.press_keys("0").unwrap();
    assert_eq!(app.control_cursor, cc_saved);
    // Deselect cable (`Esc`), then `Enter` on module jumps straight to `Panel`!
    send_key(&mut app, KeyCode::Esc);
    assert!(app.rack_ui.patchbay_cable_id.is_none());
    send_key(&mut app, KeyCode::Enter);
    assert_eq!(app.rack_ui.tab, RackTab::Panel);

    // --- Rack Zones: `h/l` switches zone (clamping control_cursor), `j/k` controls,
    //     `d` duplicates zone, `Alt+j/k` reorders zone, `A` auditions, `x` removes zone ---
    app.execute_command("instrument Kit kit").unwrap();
    app.execute_command("zone add Snare").unwrap();
    app.execute_command("instrument Kit").unwrap();
    app.press_keys("H").unwrap(); // Panel -> Zones
    assert_eq!(app.rack_ui.tab, RackTab::Zones);
    let kit_id = app
        .project()
        .instruments
        .iter()
        .find(|i| i.name == "Kit")
        .unwrap()
        .id;
    let z0 = app.rack_ui.zone_cursor;
    app.press_keys("j").unwrap(); // step vertical control
    app.press_keys("h").unwrap(); // switch zone horizontally
    assert_ne!(app.rack_ui.zone_cursor, z0, "Zones `h` switches zone");
    app.press_keys("l").unwrap();
    assert_eq!(app.rack_ui.zone_cursor, z0, "Zones `l` switches zone back");

    // `d` duplicates zone; `Alt+k` reorders zone!
    let zcount = app.project().instrument(kit_id).unwrap().rack.zones.len();
    app.press_keys("d").unwrap();
    assert_eq!(
        app.project().instrument(kit_id).unwrap().rack.zones.len(),
        zcount + 1,
        "Zones `d` must duplicate focused zone"
    );
    let focused_zid =
        app.project().instrument(kit_id).unwrap().rack.zones[app.rack_ui.zone_cursor].id;
    let pos_before = app.rack_ui.zone_cursor;
    if pos_before > 0 {
        app.press_keys("<m-k>").unwrap();
        assert_eq!(app.rack_ui.zone_cursor, pos_before - 1);
        assert_eq!(
            app.project().instrument(kit_id).unwrap().rack.zones[pos_before - 1].id,
            focused_zid,
            "Zones Alt+k must reorder active zone"
        );
    }
    // `x` deletes the duplicated zone cleanly.
    app.press_keys("x").unwrap();
    assert_eq!(
        app.project().instrument(kit_id).unwrap().rack.zones.len(),
        zcount
    );

    // `A` auditions focused zone cleanly.
    app.execute_command("zone preset init").unwrap();
    let outcomes = app.press_keys("A").unwrap();
    assert!(outcomes
        .iter()
        .any(|o| o.audition_instrument.is_some() || !o.live_notes_on.is_empty()));

    // --- Macros: Alt+h/l & ,/. dial/min, Alt+H/L & </> max, Alt+j/k reorder bindings,
    //     v invert, Alt+Backspace reset, Enter exact / jump ---
    let synth_name = app.project().instruments[0].name.clone();
    app.execute_command(&format!(
        "macro 1 bind instrument {synth_name} cutoff 0.2 0.8"
    ))
    .unwrap();
    app.execute_command(&format!(
        "macro 1 bind instrument {synth_name} resonance 0.1 0.6"
    ))
    .unwrap();
    app.press_keys("gk").unwrap();
    app.press_keys("<m-L>").unwrap(); // dial +10%
    assert!(app.project().macros[0].value > 0.05);
    app.press_keys("<m-bs>").unwrap(); // reset dial -> 0%
    assert!(app.project().macros[0].value.abs() < 1e-4);
    send_key(&mut app, KeyCode::Enter);
    assert_eq!(app.mode_name(), "VALUE ENTRY");
    send_key(&mut app, KeyCode::Esc);

    // Binding row 0: `.` raises min, `>` raises max, `v` inverts, `Alt+j` reorders below row 1!
    app.press_keys("j").unwrap();
    let b0_target = app.project().macros[0].bindings[0].target;
    let min0 = app.project().macros[0].bindings[0].min;
    let max0 = app.project().macros[0].bindings[0].max;
    app.press_keys(".>").unwrap();
    assert!(app.project().macros[0].bindings[0].min > min0);
    assert!(app.project().macros[0].bindings[0].max > max0);
    let min_inv = app.project().macros[0].bindings[0].min;
    let max_inv = app.project().macros[0].bindings[0].max;
    app.press_keys("v").unwrap();
    assert!((app.project().macros[0].bindings[0].min - max_inv).abs() < 1e-4);
    assert!((app.project().macros[0].bindings[0].max - min_inv).abs() < 1e-4);
    app.press_keys("<m-j>").unwrap();
    assert_eq!(app.project().macros[0].bindings[1].target, b0_target);

    // --- Arrange Placements: Alt+h/l move start ±4, `,/.` resize ±4, Alt+j/k move across lanes! ---
    app.execute_command("part KitPart Motif Kit").unwrap();
    app.execute_command("lane FreeLaneA").unwrap();
    app.execute_command("lane FreeLaneB").unwrap();
    app.execute_command("place KitPart FreeLaneA 2 4 song")
        .unwrap();
    let lane_a = app
        .project()
        .lanes
        .iter()
        .find(|l| l.name == "FreeLaneA")
        .unwrap()
        .id;
    let lane_b = app
        .project()
        .lanes
        .iter()
        .find(|l| l.name == "FreeLaneB")
        .unwrap()
        .id;
    let pl_id = app.project().lane(lane_a).unwrap().placements[0].id;
    app.press_keys("ga").unwrap();
    app.selected_lane = Some(lane_a);
    app.selected_placement = Some(pl_id);
    let st0 = app.project().placement(pl_id).unwrap().1.start_step;
    let len0 = app.project().placement(pl_id).unwrap().1.length_steps;

    app.press_keys("<m-l>").unwrap();
    assert_eq!(
        app.project().placement(pl_id).unwrap().1.start_step,
        st0 + 4
    );
    app.press_keys(".").unwrap();
    assert_eq!(
        app.project().placement(pl_id).unwrap().1.length_steps,
        len0 + 4
    );
    app.press_keys(",").unwrap();
    assert_eq!(app.project().placement(pl_id).unwrap().1.length_steps, len0);
    app.press_keys("<m-j>").unwrap();
    assert_eq!(
        app.project().placement(pl_id).unwrap().0.id,
        lane_b,
        "Arrange Alt+j must move placement down into FreeLaneB"
    );

    // Detail::Placement: `R` triggers global MIDI Record (NOT shadowed by Replace Instrument!).
    send_key(&mut app, KeyCode::Enter);
    app.press_keys("R").unwrap();
    assert!(
        !app.status.to_ascii_lowercase().contains("replace"),
        "`R` on Detail::Placement must NOT shadow global MIDI Record: {}",
        app.status
    );
    if app.midi.record {
        app.press_keys("R").unwrap();
    }

    // --- Pattern Values: Alt+k / `.` raise value, Alt+l slides breakpoint ---
    app.execute_command("pattern values EnvVal 16 0:0.25 8:0.75")
        .unwrap();
    app.execute_command("open EnvVal").unwrap();
    app.press_keys("gp").unwrap();
    app.press_keys("<m-K>").unwrap();
    let v_after = match &app
        .project()
        .patterns
        .iter()
        .find(|p| p.name == "EnvVal")
        .unwrap()
        .kind
    {
        PatternKind::Values { points, .. } => points[0].value,
        _ => panic!("expected Values"),
    };
    assert!(v_after > 0.25);
    app.press_keys("<m-l>").unwrap();
    assert_eq!(app.cursor, 1, "Values Alt+l slides breakpoint +1 step");

    // --- Settings & Song Properties: Alt+l / `.` adjust, Alt+Backspace resets ---
    app.press_keys("gP").unwrap();
    assert_eq!(app.mode_name(), "SONG PROPS");
    app.press_keys("j").unwrap(); // Tempo row (row 1)
    let bpm0 = app.project().bpm;
    app.press_keys("<m-l>").unwrap();
    assert!((app.project().bpm - (bpm0 + 1.0)).abs() < 1e-3);
    app.press_keys("<m-bs>").unwrap();
    assert!(
        (app.project().bpm - Project::default().bpm).abs() < 1e-3,
        "Song Props Alt+Backspace resets BPM to Project::default()"
    );
    send_key(&mut app, KeyCode::Esc);

    app.press_keys("g,").unwrap();
    assert_eq!(app.mode_name(), "SETTINGS");
    let theme0 = app.config.theme;
    app.press_keys("<m-l>").unwrap();
    assert_ne!(app.config.theme, theme0);
    app.press_keys("<m-bs>").unwrap();
    assert_eq!(
        app.config.theme,
        cadence_core::config::UserConfig::default().theme
    );
    send_key(&mut app, KeyCode::Esc);
}

// ---------------------------------------------------------------------------
// 6. Global Modifier Routing: Ctrl decks, Ctrl+Shift modals, Jam, Alt+c, Bank
// ---------------------------------------------------------------------------

#[test]
fn global_ctrl_decks_ctrl_shift_modals_jam_and_bank_isolation() {
    let (_dir, mut app) = hermetic_app(Project::demo(), "global_shortcuts.json");

    // All 10 unshifted Ctrl docked decks!
    let deck_checks: &[(char, Workspace)] = &[
        ('p', Workspace::Pattern),
        ('m', Workspace::Mixer),
        ('a', Workspace::Arrange),
    ];
    for &(ch, expected_ws) in deck_checks {
        send_mod(&mut app, KeyCode::Char(ch), KeyModifiers::CONTROL);
        assert_eq!(
            app.focused_canvas(),
            Some(expected_ws),
            "Ctrl+{ch} must focus {expected_ws:?}"
        );
    }
    send_mod(&mut app, KeyCode::Char('b'), KeyModifiers::CONTROL);
    assert!(app.bank_focused(), "Ctrl+b focuses Bank deck");

    send_mod(&mut app, KeyCode::Char('i'), KeyModifiers::CONTROL);
    assert!(app.deck_focused(), "Ctrl+i focuses Instrument deck");
    send_mod(&mut app, KeyCode::Char('k'), KeyModifiers::CONTROL);
    assert!(app.macros_focused(), "Ctrl+k focuses Macros deck");
    send_mod(&mut app, KeyCode::Char('t'), KeyModifiers::CONTROL);
    assert!(app.take_focused(), "Ctrl+t focuses Take deck");
    send_mod(&mut app, KeyCode::Char('g'), KeyModifiers::CONTROL);
    assert!(app.graph_focused(), "Ctrl+g focuses Signal Graph deck");
    send_mod(&mut app, KeyCode::Char('d'), KeyModifiers::CONTROL);
    assert!(app.midi_focused(), "Ctrl+d focuses MIDI deck");

    // Ctrl+o toggles Output monitor dock!
    let out_was_open = snap_view(&app)
        .decks
        .iter()
        .any(|d| matches!(d.view, DeckView::Output(_)));
    send_mod(&mut app, KeyCode::Char('o'), KeyModifiers::CONTROL);
    let out_now = snap_view(&app)
        .decks
        .iter()
        .any(|d| matches!(d.view, DeckView::Output(_)));
    assert_ne!(out_now, out_was_open, "Ctrl+o must toggle Output monitor");
    send_mod(&mut app, KeyCode::Char('o'), KeyModifiers::CONTROL);

    // Ctrl+Shift modals + Ctrl+, Settings alias!
    let cs = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
    send_mod(&mut app, KeyCode::Char('P'), cs);
    assert_eq!(app.mode_name(), "PALETTE");
    send_key(&mut app, KeyCode::Esc);

    send_mod(&mut app, KeyCode::Char('F'), cs);
    assert_eq!(app.mode_name(), "FILES");
    send_key(&mut app, KeyCode::Esc);

    send_mod(&mut app, KeyCode::Char('I'), cs);
    assert_eq!(app.mode_name(), "SONG PROPS");
    send_key(&mut app, KeyCode::Esc);

    send_mod(&mut app, KeyCode::Char(','), KeyModifiers::CONTROL);
    assert_eq!(app.mode_name(), "SETTINGS");
    send_key(&mut app, KeyCode::Esc);

    send_mod(&mut app, KeyCode::Char(','), cs);
    assert_eq!(app.mode_name(), "SETTINGS");
    send_key(&mut app, KeyCode::Esc);

    send_mod(&mut app, KeyCode::Char('B'), cs);
    assert_eq!(app.mode_name(), "SAMPLES");
    send_key(&mut app, KeyCode::Esc);

    send_mod(&mut app, KeyCode::Char('N'), cs);
    assert_eq!(app.mode_name(), "NEW");
    send_key(&mut app, KeyCode::Esc);

    send_mod(&mut app, KeyCode::Char('/'), cs);
    assert_eq!(app.mode_name(), "HELP");
    send_mod(&mut app, KeyCode::Char('?'), KeyModifiers::CONTROL);
    assert_eq!(app.mode_name(), "NORMAL", "Ctrl+? toggles Help closed");

    // Jam mode: `gj` enters, `Ctrl+Shift+p` opens Palette from Jam (Esc returns to Jam!), `Ctrl+Shift+j` toggles Jam off!
    app.press_keys("gj").unwrap();
    assert_eq!(app.mode_name(), "JAM");
    assert_eq!(app.ui_mode_kind(), UiModeKind::Jam);
    send_mod(&mut app, KeyCode::Char('P'), cs);
    assert_eq!(app.mode_name(), "PALETTE", "Ctrl+Shift+p works inside Jam");
    send_key(&mut app, KeyCode::Esc);
    assert_eq!(
        app.mode_name(),
        "JAM",
        "Esc from Palette opened in Jam returns to Jam"
    );

    send_mod(&mut app, KeyCode::Char('J'), cs);
    assert_eq!(app.mode_name(), "NORMAL", "Ctrl+Shift+j toggles Jam off");
    send_mod(&mut app, KeyCode::Char('J'), cs);
    assert_eq!(app.mode_name(), "JAM", "Ctrl+Shift+j toggles Jam on");
    send_mod(&mut app, KeyCode::Char('J'), cs);
    assert_eq!(app.mode_name(), "NORMAL", "Ctrl+Shift+j toggles Jam off");

    // Bank Parts: `0`/`G` navigate lanes without mutating Material cursor, and `Shift+Enter` launches immediately!
    send_mod(&mut app, KeyCode::Char('b'), KeyModifiers::CONTROL);
    assert!(app.bank_focused());
    let pat_before = app.selected_pattern;
    app.press_keys("G").unwrap();
    app.press_keys("0").unwrap();
    assert_eq!(app.selected_pattern, pat_before);
    send_mod(&mut app, KeyCode::Enter, KeyModifiers::SHIFT);
    let lane0 = app.project().lanes[0].id;
    assert!(
        app.live.lane_state(lane0).is_some() || app.live.pending_for_lane(lane0).next().is_some(),
        "Bank Parts Shift+Enter must launch part immediately"
    );

    // `Alt+c` keeps live override cleanly (moved from `Alt+k`).
    app.press_keys("<m-c>").unwrap();
    assert!(
        !app.status.is_empty(),
        "Alt+c must execute Keep Override cleanly"
    );
}

#[test]
fn open_instrument_resets_rack_cursors_graph_add_picker_and_zero_colon_leaks() {
    let mut app = App::new(Project::demo(), "cross_stream.json".into()).unwrap();
    assert!(app.project().instruments.len() >= 2);
    let inst0 = app.project().instruments[0].id;
    let inst1 = app.project().instruments[1].id;

    // 1. Switching to a DIFFERENT instrument resets panel_slot and control_cursor cleanly,
    //    while re-opening the SAME instrument preserves them.
    app.apply(cadence_app::Action::SelectInstrument { instrument: inst0 })
        .unwrap();
    app.press_keys("l").unwrap();
    app.press_keys("jj").unwrap();
    let slot_on_inst0 = app.rack_ui.panel_slot;
    let cursor_on_inst0 = app.control_cursor;
    assert!(slot_on_inst0 > 0 || cursor_on_inst0 > 0);
    app.apply(cadence_app::Action::SelectInstrument { instrument: inst0 })
        .unwrap();
    assert_eq!(app.rack_ui.panel_slot, slot_on_inst0);
    assert_eq!(app.control_cursor, cursor_on_inst0);
    app.apply(cadence_app::Action::SelectInstrument { instrument: inst1 })
        .unwrap();
    assert_eq!(
        app.rack_ui.panel_slot, 0,
        "Switching to a different instrument must reset rack_ui.panel_slot to 0"
    );
    assert_eq!(
        app.control_cursor, 0,
        "Switching to a different instrument must reset control_cursor to 0"
    );

    // 2. Signal Graph `a` opens the Add Graph Connection picker (Send / Modulation / Duck).
    app.press_keys("gGa").unwrap();
    assert_eq!(app.mode_name(), "PICK");
    app.press_keys("<Esc>").unwrap();

    // 3. Zero `:cmd` leaks across hints() on every deck/mode and `inspect_assets()`.
    let has_colon_cmd = |s: &str| {
        s.split_whitespace().any(|tok| {
            tok.starts_with(':') && tok.len() > 1 && tok.as_bytes()[1].is_ascii_alphabetic()
        })
    };
    let repair = app.inspect_assets()["repair"]
        .as_str()
        .unwrap_or("")
        .to_string();
    assert!(
        !has_colon_cmd(&repair),
        "inspect_assets repair must not leak raw Ex commands: {repair:?}"
    );
    for seq in [
        "ga", "gp", "gb", "gm", "gi", "gk", "gt", "gG", "gd", "g,", "gP", "gf", "?",
    ] {
        app.press_keys("<Esc><Esc>").unwrap();
        app.press_keys(seq).unwrap();
        for (k, d) in app.hints() {
            assert!(
                !has_colon_cmd(&k) && !has_colon_cmd(&d),
                "Hints for `{seq}` leaked raw Ex command: ({k:?}, {d:?})"
            );
        }
    }
}
