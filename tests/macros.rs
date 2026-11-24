//! Integration tests for Assignable Project Macros 1–8:
//! 1. Core data model, validation, multi-target & inverted (`min > max`) bindings, pruning, undo/redo, JSON round-trip
//! 2. Static & automated (`ModulationTarget::Macro`) audio verification via offline PCM rendering
//! 3. Hardware Smart Macros (MiniLab 3), 1-key MIDI Learn (`M`), OLED feedback, Live CC Recording (`R`), & CC coalescing
//! 4. Macros Deck (`gk`) keyboard controls, sound-only fast path, Ex commands (`:macro`, `:mod ... macro`), Chrome summary & Signal Graph (`gG`) fan-out cables
use cadence_app::layout::DeckKind;
use cadence_app::view::{DeckView, GraphEdgeKind, GraphTarget};
use cadence_app::{Action, App, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use cadence_core::macros::{MacroBinding, MacroControl, MAX_MACRO_BINDINGS, MAX_PROJECT_MACROS};
use cadence_core::material::{
    parse_document_json, write_document_json, Edit, LiveFlags, ModulationTarget, Pattern,
    PatternKind, Project, ValuePoint, ValueUnit,
};
use cadence_core::midi_map::{MidiFeedback, MidiMapping, MidiSource, MidiTarget, MidiValueMode};
use cadence_core::mixer::{ChannelParam, ChannelRef, EffectParam, MixerParamTarget};
use cadence_core::params::{InstrumentParam, ModuleParam, ParamAddress, ValueMode, ZoneParam};
use cadence_core::rack::{ModuleKind, RackPatch};
use cadence_core::runtime::{AudioSnapshot, MidiMessage};
use cadence_engine::audio::render_schedule_pcm;
use std::collections::BTreeSet;
use std::path::PathBuf;

fn app_demo() -> App {
    App::new(Project::demo(), PathBuf::from("macros.json")).unwrap()
}

fn channel_rms_lr(pcm: &[[i16; 2]], start_frame: usize, end_frame: usize) -> (f32, f32) {
    let total_frames = pcm.len();
    let a = start_frame.min(total_frames);
    let b = end_frame.min(total_frames).max(a + 1);
    let mut sum_l = 0.0_f64;
    let mut sum_r = 0.0_f64;
    let n = (b - a) as f64;
    for &[li, ri] in &pcm[a..b] {
        let l = f64::from(li) / 32768.0;
        let r = f64::from(ri) / 32768.0;
        sum_l += l * l;
        sum_r += r * r;
    }
    ((sum_l / n).sqrt() as f32, (sum_r / n).sqrt() as f32)
}

/// High-frequency roughness proxy (mean |x[n] - x[n-1]| normalized by mean |x|) to verify filter cutoff brightness audibly.
fn spectral_brightness(pcm: &[[i16; 2]], start_frame: usize, end_frame: usize) -> f32 {
    let total_frames = pcm.len();
    let a = start_frame.min(total_frames);
    let b = end_frame.min(total_frames).max(a + 2);
    let mut diff_sum = 0.0_f64;
    let mut energy_sum = 0.0_f64;
    let mut prev = 0.5 * (f64::from(pcm[a][0]) + f64::from(pcm[a][1])) / 32768.0;
    for &[li, ri] in &pcm[(a + 1)..b] {
        let cur = 0.5 * (f64::from(li) + f64::from(ri)) / 32768.0;
        diff_sum += (cur - prev).abs();
        energy_sum += cur.abs();
        prev = cur;
    }
    if energy_sum < 1e-6 {
        0.0
    } else {
        (diff_sum / energy_sum) as f32
    }
}

#[test]
fn macro_crud_inverted_bindings_undo_redo_pruning_and_roundtrip() {
    let mut app = app_demo();
    let saw_id = app.project().instruments[0].id;
    let lead_ch = ChannelRef::Channel(app.project().mixer.channels[0].id);
    let delay_dev_id = app.project().mixer.channels[4].inserts[0].id;

    // 1. Empty `macros` list is omitted on serialization and round-trips cleanly.
    let mut raw_empty = Vec::new();
    write_document_json(&mut raw_empty, app.project(), &LiveFlags::default()).unwrap();
    let json_empty = String::from_utf8(raw_empty.clone()).unwrap();
    assert!(
        !json_empty.contains("\"macros\""),
        "empty macros must be omitted during serialization"
    );
    let (parsed_empty, _) = parse_document_json(&raw_empty).unwrap();
    assert!(parsed_empty.macros.is_empty());

    // 2. Rename, bind multi-target (including inverted min > max!), set value via Ex commands & Actions.
    app.execute_command("macro 1 name Filter Pan").unwrap();
    app.execute_command("macro 1 bind instrument Saw cutoff 0.15 0.95")
        .unwrap();
    app.execute_command("macro 1 bind channel Lead pan 0.90 0.10")
        .unwrap(); // inverted!
    app.execute_command(&format!("macro 1 bind effect {delay_dev_id} mix 10% 85%"))
        .unwrap();
    app.execute_command("macro 1 75%").unwrap();

    let m1 = app.project().macro_control(0).expect("M1 created");
    assert_eq!(m1.name, "Filter Pan");
    assert!((m1.value - 0.75).abs() < 1e-5);
    assert_eq!(m1.bindings.len(), 3);
    assert!(!m1.bindings[0].is_inverted());
    assert!(m1.bindings[1].is_inverted());
    assert!((m1.bindings[1].evaluate(0.0) - 0.90).abs() < 1e-5);
    assert!((m1.bindings[1].evaluate(1.0) - 0.10).abs() < 1e-5);
    assert!((m1.bindings[1].evaluate(0.75) - 0.30).abs() < 1e-5);

    // Original authored project base patch & channel strip remain untouched!
    let base_pan = app
        .project()
        .channel(app.project().mixer.channels[0].id)
        .unwrap()
        .strip
        .pan;
    assert!(
        base_pan.abs() < 1e-5,
        "authored channel base pan must stay untouched when macro overrides compiled ScoreWindow"
    );
    // Compiled ScoreWindow reflects the mapped macro values!
    let compiled_pan = app
        .compiled_project()
        .channel(app.project().mixer.channels[0].id)
        .unwrap()
        .strip
        .pan;
    let expected_pan = 0.30 * 2.0 - 1.0; // -0.40
    assert!(
        (compiled_pan - expected_pan).abs() < 1e-4,
        "expected compiled pan {expected_pan}, got {compiled_pan}"
    );

    // 3. JSON round-trip preserves macros + inverted bounds exactly.
    let mut saved = Vec::new();
    write_document_json(&mut saved, app.project(), app.flags()).unwrap();
    let (reloaded, _) = parse_document_json(&saved).unwrap();
    assert_eq!(reloaded.macros, app.project().macros);

    // 4. Undo / Redo round-trip on SetMacro, Unbind, and Clear.
    app.apply(Action::Undo).unwrap();
    assert!((app.project().macro_control(0).unwrap().value - 0.0).abs() < 1e-5);
    app.apply(Action::Redo).unwrap();
    assert!((app.project().macro_control(0).unwrap().value - 0.75).abs() < 1e-5);

    // Unbind 1-based binding #2 (inverted Lead pan) -> 2 bindings remain.
    app.execute_command("macro 1 unbind 2").unwrap();
    assert_eq!(app.project().macro_control(0).unwrap().bindings.len(), 2);
    app.apply(Action::Undo).unwrap();
    assert_eq!(app.project().macro_control(0).unwrap().bindings.len(), 3);

    // Unbind all -> 0 bindings, macro still exists.
    app.execute_command("macro 1 unbind all").unwrap();
    assert!(app.project().macro_control(0).unwrap().bindings.is_empty());
    app.apply(Action::Undo).unwrap();
    assert_eq!(app.project().macro_control(0).unwrap().bindings.len(), 3);

    // 5. Pruning when referenced effect device is deleted!
    app.execute_command(&format!("fx delete {delay_dev_id}"))
        .unwrap();
    assert_eq!(
        app.project().macro_control(0).unwrap().bindings.len(),
        2,
        "deleting a bound effect device must prune its macro binding cleanly"
    );
    assert!(app.project().validate().is_ok());

    // 6. Clear macro removes slot 0 cleanly (undoable).
    app.execute_command("macro 1 clear").unwrap();
    assert!(app.project().macro_control(0).is_none());
    app.apply(Action::Undo).unwrap();
    assert!(app.project().macro_control(0).is_some());

    // 7. Validation guards: out-of-range index, invalid bounds, duplicate index, over-limit bindings.
    let mut bad = app.project().clone();
    bad.macros.push(MacroControl {
        index: MAX_PROJECT_MACROS as u8,
        name: "Overflow".into(),
        value: 0.5,
        bindings: Vec::new(),
    });
    assert!(bad.validate().is_err(), "index >= 8 must fail validation");

    let mut bad_bounds = app.project().clone();
    bad_bounds.macros[0].bindings.push(MacroBinding::new(
        ParamAddress::Channel {
            channel: lead_ch,
            param: ChannelParam::Gain,
        },
        -0.1,
        1.2,
    ));
    assert!(
        bad_bounds.validate().is_err(),
        "out-of-range bounds must fail validation"
    );

    let mut bad_mod = app.project().clone();
    let pat_id = bad_mod.patterns[6].id;
    bad_mod
        .modulations
        .push(cadence_core::material::Modulation {
            id: bad_mod.max_id() + 1,
            name: "BadMacroMod".into(),
            target: ModulationTarget::Macro { index: 8 },
            pattern: pat_id,
        });
    assert!(
        bad_mod.validate().is_err(),
        "ModulationTarget::Macro index >= 8 must fail validation"
    );

    // Ensure binding limit is respected (16 max per macro).
    assert_eq!(MAX_MACRO_BINDINGS, 16);
    assert!(
        app.project()
            .validate_macro_target(ParamAddress::Zone {
                instrument: saw_id,
                zone: 1,
                param: ZoneParam::VoiceMode,
            })
            .is_err(),
        "structural ZoneParam::VoiceMode must not be bindable to a Macro"
    );
}

#[test]
fn static_macro_and_automated_macro_audibly_sweep_multiple_targets_including_inverted_range() {
    // Build a clean isolated Synth lead project (no pan variation link so L/R stereo balance is pure!).
    let mut project = Project::demo();
    project.parts[1].parameter_links.clear();
    // Solo Lead lane so audio measurements reflect Lead Synth cutoff + Lead Channel pan directly.
    let flags = LiveFlags {
        solo_lanes: BTreeSet::from([project.lanes[0].id]),
        ..LiveFlags::default()
    };
    let saw_id = project.instruments[0].id;
    let lead_ch_id = project.mixer.channels[0].id;
    let lead_ch = ChannelRef::Channel(lead_ch_id);
    // Keep Echo return quiet so stereo pan measurement on Lead is crystal clear.
    project.mixer.channels[4].strip.gain = 0.0;

    // Bind Macro 1 to:
    //  - Saw Filter Cutoff (zone 1, module 4): 0.08 -> 0.96 (dark -> bright)
    //  - Lead Pan:                              0.95 -> 0.05 (INVERTED! Right -> Left!)
    let saw_cutoff_param = InstrumentParam::Module {
        zone: 1,
        module: 4,
        param: ModuleParam::Cutoff,
    };
    let mut m1 = MacroControl::new(0, "Sweep");
    m1.value = 0.0;
    m1.bindings = vec![
        MacroBinding::new(
            ParamAddress::from_instrument(saw_id, saw_cutoff_param),
            0.08,
            0.96,
        ),
        MacroBinding::new(
            ParamAddress::Channel {
                channel: lead_ch,
                param: ChannelParam::Pan,
            },
            0.95,
            0.05,
        ),
    ];
    project.macros.push(m1);

    // 1. Static macro at 0.0 (dark + hard Right) vs 1.0 (bright + hard Left).
    let score_low = project.compile_with_flags(&flags).unwrap();
    let (_, pcm_low) = render_schedule_pcm(&[(0, &score_low)], 0, 32, Vec::new(), 24_000).unwrap();

    project.macro_control_mut(0).unwrap().value = 1.0;
    let score_high = project.compile_with_flags(&flags).unwrap();
    let (_, pcm_high) =
        render_schedule_pcm(&[(0, &score_high)], 0, 32, Vec::new(), 24_000).unwrap();

    assert_ne!(pcm_low, pcm_high, "sweeping static Macro must change audio");
    let frames_low = pcm_low.len();
    let (low_l, low_r) = channel_rms_lr(&pcm_low, 0, frames_low);
    let (high_l, high_r) = channel_rms_lr(&pcm_high, 0, pcm_high.len());
    assert!(
        low_r > low_l * 2.0,
        "at macro=0.0 inverted pan=0.95 should be strongly Right: L={low_l}, R={low_r}"
    );
    assert!(
        high_l > high_r * 2.0,
        "at macro=1.0 inverted pan=0.05 should be strongly Left: L={high_l}, R={high_r}"
    );
    let bright_low = spectral_brightness(&pcm_low, 0, frames_low);
    let bright_high = spectral_brightness(&pcm_high, 0, pcm_high.len());
    assert!(
        bright_high > bright_low * 1.35,
        "at macro=1.0 cutoff should be audibly brighter than macro=0.0: low={bright_low}, high={bright_high}"
    );

    // 2. Automated macro via `ModulationTarget::Macro { index: 0 }` using a Ramp Values pattern (0.0 -> 1.0 over 32 steps).
    let mut app =
        App::new_with_flags(project, flags.clone(), PathBuf::from("auto_macro.json")).unwrap();
    let ramp_pat_id = app.project().max_id() + 1;
    app.apply(Action::Edit {
        edit: Edit::PutPattern {
            pattern: Pattern {
                id: ramp_pat_id,
                name: "MacroRamp".into(),
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
                            step: 8,
                            value: 0.0,
                        },
                        ValuePoint {
                            step: 20,
                            value: 1.0,
                        },
                        ValuePoint {
                            step: 28,
                            value: 1.0,
                        },
                    ],
                },
            },
        },
    })
    .unwrap();
    app.execute_command("mod MacroSweep macro 1 MacroRamp")
        .unwrap();

    // Verify compiled ScoreWindow emitted BOTH mapped ModLane (Rack Filter Cutoff) AND inverted MixerAutomation (Channel Pan)!
    let compiled = app.compiled_project();
    let cutoff_lane = compiled
        .mod_lanes
        .iter()
        .find(|l| l.target == saw_cutoff_param)
        .expect("ModLane emitted for Rack Filter Cutoff");
    assert!((cutoff_lane.values[0] - 0.08).abs() < 1e-4);
    assert!((cutoff_lane.values[20] - 0.96).abs() < 1e-4);

    let pan_auto = compiled
        .mixer
        .automations
        .iter()
        .find(|a| {
            a.target
                == MixerParamTarget::Channel {
                    channel: lead_ch,
                    param: ChannelParam::Pan,
                }
        })
        .expect("MixerAutomation emitted for inverted Channel Pan");
    assert!((pan_auto.points[0].value - 0.95).abs() < 1e-4);
    assert!((pan_auto.points[20].value - 0.05).abs() < 1e-4);

    // Render 32 steps of the automated song and verify early window is Right+Dark while late window is Left+Bright!
    let (_, pcm_auto) = render_schedule_pcm(&[(0, compiled)], 0, 32, Vec::new(), 24_000).unwrap();
    let total_f = pcm_auto.len();
    let early_end = total_f / 4;
    let late_start = (total_f * 5) / 8;
    let late_end = (total_f * 7) / 8;
    let (e_l, e_r) = channel_rms_lr(&pcm_auto, 0, early_end);
    let (l_l, l_r) = channel_rms_lr(&pcm_auto, late_start, late_end);
    assert!(
        e_r > e_l * 1.4,
        "early automated segment should be Right-panned: L={e_l}, R={e_r}"
    );
    assert!(
        l_l > l_r * 1.4,
        "late automated segment should sweep to Left via inverted macro binding: L={l_l}, R={l_r}"
    );
    let b_early = spectral_brightness(&pcm_auto, 0, early_end);
    let b_late = spectral_brightness(&pcm_auto, late_start, late_end);
    assert!(
        b_late > b_early * 1.2,
        "automated macro sweep should open filter cutoff over time: early={b_early}, late={b_late}"
    );
}

#[test]
fn minilab3_smart_macros_midi_learn_and_live_cc_recording_drive_project_macros() {
    let mut app = app_demo();
    app.config.midi_pickup = false;
    app.execute_command("midi profile minilab3_daw").unwrap();

    // Configure Project Macro 1 ("Filter Wash") bound to Saw Cutoff + Delay Mix.
    let delay_id = app.project().mixer.channels[4].inserts[0].id;
    app.execute_command("macro 1 name Filter Wash").unwrap();
    app.execute_command("macro 1 bind instrument Saw cutoff 0.2 0.9")
        .unwrap();
    app.execute_command(&format!("macro 1 bind effect {delay_id} mix 0.1 0.8"))
        .unwrap();

    // 1. On Arrange (not inside an object deck), MiniLab 3 Encoder 1 (CC 86 ch 1 -> FocusedMacro 0)
    // automatically drives Project Macro 1 AND emits OLED feedback with custom name + bar!
    let out = app
        .handle_midi(MidiMessage::Control {
            channel: 0,
            controller: 86,
            value: 102, // ~80%
        })
        .unwrap();
    let m1_val = app.project().macro_control(0).unwrap().value;
    assert!(
        (m1_val - (102.0 / 127.0)).abs() < 1e-3,
        "MiniLab 3 CC 86 should drive Project Macro 1, got {m1_val}"
    );
    assert!(
        out.midi_feedback.iter().any(|fb| matches!(
            fb,
            MidiFeedback::Display { line1, line2, value_bar: Some(_) }
                if line1.contains("Filter Wash") && line2.contains("80%")
        )),
        "OLED display must show custom Macro name and percentage: {:?}",
        out.midi_feedback
    );

    // 2. When explicitly focused inside an Instrument deck, FocusedMacro 0 targets the Instrument instead!
    app.press_keys("gi").unwrap();
    assert!(app.deck_focused());
    let prev_m1 = app.project().macro_control(0).unwrap().value;
    app.handle_midi(MidiMessage::Control {
        channel: 0,
        controller: 86,
        value: 32,
    })
    .unwrap();
    assert!(
        (app.project().macro_control(0).unwrap().value - prev_m1).abs() < 1e-6,
        "FocusedMacro must yield to focused Instrument object deck when explicitly inside it"
    );

    // 3. Open Macros Deck (`gk`), test keyboard controls (`1..8`, `h/l`, `j/k`, `H/L`, `M` MIDI Learn, `R` Record)!
    app.press_keys("gk").unwrap();
    assert!(app.macros_focused());
    app.press_keys("2").unwrap();
    assert_eq!(
        app.view_state.macro_cursor, 1,
        "pressing '2' in Macros deck selects M2"
    );
    app.press_keys("<m-L>").unwrap(); // coarse nudge up +10% directly in NORMAL
    assert!(
        (app.project().macro_control(1).unwrap().value - 0.10).abs() < 1e-4,
        "pressing 'Alt+L' in Macros deck should coarsely nudge M2 up"
    );
    // 1-key MIDI Learn (`M`) directly onto Project Macro 2 (`MidiTarget::ProjectMacro { slot: 1 }`)!
    app.press_keys("M").unwrap();
    assert_eq!(app.midi.learn, Some(MidiTarget::ProjectMacro { slot: 1 }));
    app.handle_midi(MidiMessage::Control {
        channel: 0,
        controller: 75,
        value: 64,
    })
    .unwrap();
    assert!(app.midi.learn.is_none());
    assert!(app
        .config
        .midi_mappings
        .iter()
        .any(|m| m.target == MidiTarget::ProjectMacro { slot: 1 }));
    // Turning newly learned CC 75 now drives M2 directly from ANY deck!
    app.handle_midi(MidiMessage::Control {
        channel: 0,
        controller: 75,
        value: 127,
    })
    .unwrap();
    assert!((app.project().macro_control(1).unwrap().value - 1.0).abs() < 1e-4);

    // 4. Arm MIDI Record (`R`) during playback and twist MiniLab 3 CC 86 (M1) across steps 0, 4, 8!
    app.apply(Action::Play { playing: true }).unwrap();
    app.press_keys("R").unwrap();
    assert!(app.midi.record);

    app.handle_midi(MidiMessage::Control {
        channel: 0,
        controller: 86,
        value: 20,
    })
    .unwrap();
    app.apply(Action::Advance { steps: 5 }).unwrap(); // song step 4
    app.handle_midi(MidiMessage::Control {
        channel: 0,
        controller: 86,
        value: 90,
    })
    .unwrap();
    app.apply(Action::Advance { steps: 4 }).unwrap(); // song step 8
    app.handle_midi(MidiMessage::Control {
        channel: 0,
        controller: 86,
        value: 127,
    })
    .unwrap();

    let macro_mod = app
        .project()
        .modulations
        .iter()
        .find(|m| m.target == ModulationTarget::Macro { index: 0 })
        .expect("live CC recording must create ModulationTarget::Macro { index: 0 }");
    let PatternKind::Values {
        unit, mode, points, ..
    } = &app.project().pattern(macro_mod.pattern).unwrap().kind
    else {
        panic!("expected Values pattern for macro automation");
    };
    assert_eq!(*unit, ValueUnit::Normalized);
    assert_eq!(*mode, ValueMode::Ramp);
    assert_eq!(points.len(), 3);
    assert_eq!(points[0].step, 0);
    assert_eq!(points[1].step, 4);
    assert_eq!(points[2].step, 8);
    assert!((points[2].value - 1.0).abs() < 1e-4);
    // Live audio compiled score immediately contains the mapped Rack Filter Cutoff & Delay Mix lanes!
    assert!(app.audio_compiled().mod_lanes.iter().any(|l| {
        l.target
            == InstrumentParam::Module {
                zone: 1,
                module: 4,
                param: ModuleParam::Cutoff,
            }
    }));
    assert!(app.audio_compiled().mixer.automations.iter().any(|a| {
        a.target
            == MixerParamTarget::Device {
                device_id: delay_id,
                param: EffectParam::Mix,
            }
    }));

    // Single atomic undo reverts the entire coalesced CC pass!
    app.apply(Action::Undo).unwrap();
    assert!(
        !app.project()
            .modulations
            .iter()
            .any(|m| m.target == ModulationTarget::Macro { index: 0 }),
        "single atomic undo must revert coalesced macro CC pass"
    );
}

#[test]
fn signal_graph_projects_macro_nodes_and_fanout_edges_and_navigates_to_macros_deck() {
    let mut app = app_demo();
    let saw_id = app.project().instruments[0].id;
    let lead_ch = ChannelRef::Channel(app.project().mixer.channels[0].id);
    let delay_id = app.project().mixer.channels[4].inserts[0].id;
    let energy_pat_id = app.project().patterns[6].id;

    app.execute_command("macro 1 name Master Wash").unwrap();
    app.execute_command("macro 1 bind instrument Saw cutoff 0.15 0.90")
        .unwrap();
    app.execute_command("macro 1 bind channel Lead pan 0.85 0.15")
        .unwrap();
    app.execute_command(&format!("macro 1 bind effect {delay_id} mix 0.05 0.75"))
        .unwrap();
    app.execute_command("mod WashAuto macro 1 Energy").unwrap();

    // Open Signal Graph (`gG`) and inspect Macro nodes + fan-out cables!
    app.press_keys("gG").unwrap();
    let snap = AudioSnapshot::default();
    let fv = app.frame_view(&snap, "CoreAudio");
    assert_eq!(fv.chrome.active_macro_count, 1);
    assert_eq!(fv.chrome.active_macro_automated, 1);

    let graph = fv
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Graph(g) => Some(g),
            _ => None,
        })
        .expect("GraphDeckView");

    assert_eq!(
        graph.columns.len(),
        6,
        "MACROS column appears when macros exist"
    );
    let macro_col_idx = graph
        .columns
        .iter()
        .position(|c| c.title == "MACROS")
        .expect("MACROS column");
    let macro_node = graph.columns[macro_col_idx]
        .nodes
        .iter()
        .find(|n| n.target == GraphTarget::Macro(0))
        .expect("Macro 1 node in Signal Graph");

    let pat_node = graph
        .columns
        .iter()
        .flat_map(|c| c.nodes.iter())
        .find(|n| n.target == GraphTarget::Pattern(energy_pat_id))
        .expect("Energy pattern node");
    let saw_node = graph
        .columns
        .iter()
        .flat_map(|c| c.nodes.iter())
        .find(|n| n.target == GraphTarget::Instrument(saw_id))
        .expect("Saw instrument node");
    let lead_node = graph
        .columns
        .iter()
        .flat_map(|c| c.nodes.iter())
        .find(|n| n.target == GraphTarget::Channel(lead_ch))
        .expect("Lead channel node");
    let echo_node = graph
        .columns
        .iter()
        .flat_map(|c| c.nodes.iter())
        .find(|n| n.target == GraphTarget::Effect(delay_id))
        .expect("Echo FX node");

    // Upstream Values -> Macro modulation cable!
    assert!(graph.edges.iter().any(|e| {
        e.kind == GraphEdgeKind::Modulation
            && e.from_col == pat_node.col
            && e.from_row == pat_node.row
            && e.to_col == macro_node.col
            && e.to_row == macro_node.row
    }));
    // Downstream Macro fan-out cables to Instrument (Saw), Channel (Lead), and Effect's Channel (Echo)!
    for (label, dst) in [
        ("Saw", saw_node),
        ("Lead", lead_node),
        ("Echo FX", echo_node),
    ] {
        assert!(
            graph.edges.iter().any(|e| {
                e.kind == GraphEdgeKind::Macro
                    && e.from_col == macro_node.col
                    && e.from_row == macro_node.row
                    && e.to_col == dst.col
                    && e.to_row == dst.row
            }),
            "missing Macro fan-out edge to {label}"
        );
    }

    // Pressing Enter on a Macro node in Signal Graph opens the Macros deck on that macro!
    app.view_state.graph_col = macro_col_idx;
    app.view_state.graph_row = 0;
    app.press_keys("<Enter>").unwrap();
    assert!(app.deck_visible_kind(DeckKind::Macros));
    assert!(app.macros_focused());
    assert_eq!(app.view_state.macro_cursor, 0);
}

#[test]
fn macro_works_with_sampler_and_multizone_kit_racks_too() {
    let mut app = app_demo();
    app.execute_command("kit Drums808 tr808").unwrap();
    let kit_id = app
        .project()
        .instruments
        .iter()
        .find(|i| i.name == "Drums808")
        .unwrap()
        .id;
    assert!(app.instrument(kit_id).unwrap().rack.zones.len() > 1);
    app.execute_command("macro 4 name Kit Tone").unwrap();
    app.execute_command("macro 4 bind instrument Drums808 cutoff 0.15 0.90")
        .unwrap();
    app.execute_command("macro 4 0.5").unwrap();

    let compiled_kit = app
        .compiled_project()
        .instruments
        .iter()
        .find(|i| i.id == kit_id)
        .unwrap();
    assert!(
        compiled_kit.rack.cutoff_hz < 18_000.0 && compiled_kit.rack.cutoff_hz > 200.0,
        "Macro 4 should modulate multi-zone Kit Rack bus cutoff in compiled ScoreWindow, got {}",
        compiled_kit.rack.cutoff_hz
    );
}

#[test]
fn frame_sampler_macro_synthetic_sends_device_move_and_precedence() {
    use cadence_core::material::{InstrumentInstance, Modulation};

    let mut project = Project::demo();
    let lead_ch_id = project.mixer.channels[0].id;
    let bass_ch_id = project.mixer.channels[1].id;
    let echo_ch_id = project.mixer.channels[4].id;
    let delay_dev_id = project.mixer.channels[4].inserts[0].id;
    let saw_id = project.instruments[0].id;

    // Static Macro bound to frame-based Sampler ModuleParam emits a constant ModLane instead of
    // truncating SamplerModule against total_frames = 0.
    let smp_id = project.max_id() + 1;
    let mut chop_rack = RackPatch::default_sampler();
    if let Some(ModuleKind::Sampler(s)) = chop_rack.zones[0].module_mut(1).map(|m| &mut m.kind) {
        s.sample_start = 1000;
        s.sample_end = 48_000;
    }
    project.instruments.push(InstrumentInstance {
        id: smp_id,
        name: "Chop".into(),
        channel: lead_ch_id,
        rack: chop_rack,
    });
    let smp_start_param = InstrumentParam::Module {
        zone: 1,
        module: 1,
        param: ModuleParam::SampleStart,
    };
    let smp_end_param = InstrumentParam::Module {
        zone: 1,
        module: 1,
        param: ModuleParam::SampleEnd,
    };
    let mut m1 = MacroControl::new(0, "Frame Macro");
    m1.value = 0.5;
    m1.bindings = vec![
        MacroBinding::new(
            ParamAddress::from_instrument(smp_id, smp_start_param),
            0.2,
            0.8,
        ),
        MacroBinding::new(
            ParamAddress::from_instrument(smp_id, smp_end_param),
            0.6,
            1.0,
        ),
    ];
    project.macros.push(m1);
    let score = project.compile().unwrap();
    let smp_idx = score.instrument_index(smp_id).unwrap();
    let Some(ModuleKind::Sampler(compiled_smp)) = score.instruments[smp_idx].rack.zones[0]
        .module(1)
        .map(|m| &m.kind)
    else {
        panic!("expected Sampler module 1");
    };
    assert_eq!(compiled_smp.sample_start, 1000);
    assert_eq!(compiled_smp.sample_end, 48_000);
    let start_lane = score
        .mod_lanes
        .iter()
        .find(|l| usize::from(l.inst_idx) == smp_idx && l.target == smp_start_param)
        .expect("static Macro on SampleStart must emit constant ModLane");
    assert!((start_lane.values[0] - 0.5).abs() < 1e-5);

    // Updating Macro value and calling patch_score_sound updates the constant ModLane.
    project.macro_control_mut(0).unwrap().value = 1.0;
    let patched = project
        .patch_score_sound(&score, &LiveFlags::default())
        .unwrap();
    let patched_start = patched
        .mod_lanes
        .iter()
        .find(|l| usize::from(l.inst_idx) == smp_idx && l.target == smp_start_param)
        .unwrap();
    assert!((patched_start.values[0] - 0.8).abs() < 1e-5);

    // patch_score_sound preserves synthetic ChannelSend for automated Macro sends.
    let mut m2 = MacroControl::new(1, "Auto Send");
    m2.bindings = vec![MacroBinding::new(
        ParamAddress::Send {
            channel: ChannelRef::Channel(lead_ch_id),
            target: echo_ch_id,
        },
        0.1,
        0.9,
    )];
    project.macros.push(m2);
    let energy_pat = project.patterns[6].id;
    let m2_mod_id = project.max_id() + 1;
    project.modulations.push(Modulation {
        id: m2_mod_id,
        name: "Auto M2".into(),
        target: ModulationTarget::Macro { index: 1 },
        pattern: energy_pat,
    });
    assert!(project.channel(lead_ch_id).unwrap().sends.is_empty());
    let score_send = project.compile().unwrap();
    assert!(score_send
        .channel(lead_ch_id)
        .unwrap()
        .sends
        .iter()
        .any(|s| s.target == echo_ch_id));
    let patched_send = project
        .patch_score_sound(&score_send, &LiveFlags::default())
        .unwrap();
    assert!(
        patched_send
            .channel(lead_ch_id)
            .unwrap()
            .sends
            .iter()
            .any(|s| s.target == echo_ch_id),
        "patch_score_sound must re-synthesize missing ChannelSend for automated Macro sends"
    );

    // Moving an effect device from one channel to another in one transaction preserves Macro bindings.
    let mut m3 = MacroControl::new(2, "FX Macro");
    m3.bindings = vec![MacroBinding::new(
        ParamAddress::Effect {
            device: delay_dev_id,
            param: EffectParam::Mix,
        },
        0.1,
        0.9,
    )];
    project.macros.push(m3);
    project
        .apply(Edit::MoveEffect {
            device: delay_dev_id,
            target_channel: ChannelRef::Channel(bass_ch_id),
            target_slot: None,
        })
        .unwrap();
    assert_eq!(
        project.macro_control(2).unwrap().bindings.len(),
        1,
        "moving effect device across channels must not destroy Macro binding"
    );

    // Validate ModulationTarget::Macro existence, guard RemoveMacro, and enforce deterministic precedence.
    let mut missing_mac = project.clone();
    missing_mac.modulations.push(Modulation {
        id: missing_mac.max_id() + 1,
        name: "DanglingMacro".into(),
        target: ModulationTarget::Macro { index: 5 },
        pattern: energy_pat,
    });
    assert!(missing_mac.validate().is_err());
    assert!(
        project.apply(Edit::RemoveMacro { index: 1 }).is_err(),
        "RemoveMacro must be rejected while a Modulation targets it"
    );

    // Deterministic precedence: direct Modulation on Saw Cutoff wins over automated Macro
    // regardless of whether the direct Modulation appears before or after the Macro in `modulations`.
    let low_pat = project.max_id() + 1;
    project.patterns.push(Pattern {
        id: low_pat,
        name: "ConstLow".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 4,
            mode: ValueMode::Hold,
            points: vec![ValuePoint {
                step: 0,
                value: 0.12,
            }],
        },
    });
    let high_pat = project.max_id() + 1;
    project.patterns.push(Pattern {
        id: high_pat,
        name: "ConstHigh".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 4,
            mode: ValueMode::Hold,
            points: vec![ValuePoint {
                step: 0,
                value: 0.88,
            }],
        },
    });
    let saw_cutoff_param = InstrumentParam::Module {
        zone: 1,
        module: 4,
        param: ModuleParam::Cutoff,
    };
    let mut m4 = MacroControl::new(3, "Cutoff Macro");
    m4.bindings = vec![MacroBinding::new(
        ParamAddress::from_instrument(saw_id, saw_cutoff_param),
        0.0,
        1.0,
    )];
    project.macros.push(m4);
    let direct_mod = Modulation {
        id: project.max_id() + 1,
        name: "DirectCutoff".into(),
        target: ModulationTarget::Instrument {
            instrument: saw_id,
            parameter: saw_cutoff_param,
        },
        pattern: low_pat,
    };
    let macro_mod = Modulation {
        id: project.max_id() + 2,
        name: "MacroCutoff".into(),
        target: ModulationTarget::Macro { index: 3 },
        pattern: high_pat,
    };

    let mut order_a = project.clone();
    order_a.modulations.push(direct_mod.clone());
    order_a.modulations.push(macro_mod.clone());
    let mut order_b = project.clone();
    order_b.modulations.push(macro_mod);
    order_b.modulations.push(direct_mod);

    let score_a = order_a.compile().unwrap();
    let score_b = order_b.compile().unwrap();
    let saw_idx = score_a.instrument_index(saw_id).unwrap();
    let val_a = score_a
        .mod_lanes
        .iter()
        .find(|l| usize::from(l.inst_idx) == saw_idx && l.target == saw_cutoff_param)
        .unwrap()
        .values[0];
    let val_b = score_b
        .mod_lanes
        .iter()
        .find(|l| usize::from(l.inst_idx) == saw_idx && l.target == saw_cutoff_param)
        .unwrap()
        .values[0];
    assert!((val_a - 0.12).abs() < 1e-5);
    assert!((val_b - 0.12).abs() < 1e-5);
}

#[test]
fn midi_macro_cc_sweeps_and_keyboard_nudges_coalesce_via_sound_only_fast_path() {
    let mut app = app_demo();
    let synth_id = app.project().instruments[0].id;
    let synth_name = app.instrument(synth_id).unwrap().name.clone();
    app.execute_command(&format!(
        "macro 1 bind instrument {synth_name} cutoff 0.1 0.9"
    ))
    .unwrap();
    app.execute_command("macro 1 0.2").unwrap();

    // 1. Rapid MIDI CC updates to a ProjectMacro coalesce into 1 Document undo step.
    app.config.midi_mappings.push(MidiMapping {
        id: 502,
        label: "M1".into(),
        source: MidiSource::Cc {
            channel: None,
            cc: 74,
        },
        target: MidiTarget::ProjectMacro { slot: 0 },
        mode: MidiValueMode::Absolute,
        min: 0.0,
        max: 1.0,
        invert: false,
        pickup: false,
    });
    for val in [40u8, 60, 80, 100] {
        let _ = app.handle_midi_event(MidiMessage::Control {
            channel: 0,
            controller: 74,
            value: val,
        });
    }
    assert!(app.project().macro_control(0).unwrap().value > 0.7);
    app.apply(Action::Undo).unwrap();
    assert!(
        (app.project().macro_control(0).unwrap().value - 0.2).abs() < 1e-4,
        "4 rapid MIDI Macro CC updates must coalesce into 1 Document undo step"
    );

    // 2. Keyboard repeat nudges on Macros deck coalesce cleanly without resyncing SampleBank.
    app.press_keys("gk1").unwrap();
    let out_press = app
        .handle_key(KeyEvent::new_with_kind(
            KeyCode::Char('l'),
            KeyModifiers::ALT,
            KeyEventKind::Press,
        ))
        .unwrap();
    assert!(out_press.changed);
    assert!(!out_press.samples_changed);
    for _ in 0..4 {
        let out_rep = app
            .handle_key(KeyEvent::new_with_kind(
                KeyCode::Char('l'),
                KeyModifiers::ALT,
                KeyEventKind::Repeat,
            ))
            .unwrap();
        assert!(out_rep.changed);
        assert!(!out_rep.samples_changed);
    }
    assert!(app.project().macro_control(0).unwrap().value > 0.24);
    app.apply(Action::Undo).unwrap();
    assert!(
        (app.project().macro_control(0).unwrap().value - 0.2).abs() < 1e-4,
        "Single Undo must revert all 5 coalesced Macro nudges back to 0.2"
    );
    app.apply(Action::Undo).unwrap();
    assert!(
        app.project().macro_control(0).unwrap().value.abs() < 1e-4,
        "Second Undo must revert past 0.2 to the initial value 0.0"
    );
}

#[test]
fn macros_deck_gui_crud_rename_bind_jump_invert_reorder_and_automated_clear() {
    use cadence_app::view::{DeckView, OverlayView};
    use cadence_app::{ControlTarget, Detail, RackTab};

    let selected_target = |app: &App| {
        app.control_descriptors()
            .get(app.control_cursor)
            .map(|c| c.target.clone())
    };

    let mut app = app_demo();
    let saw_id = app.project().instruments[0].id;
    let lead_ch = app.project().mixer.channels[0].id;
    let delay_id = app
        .project()
        .mixer
        .channels
        .iter()
        .flat_map(|c| c.inserts.iter())
        .find(|d| matches!(d.kind, cadence_core::mixer::EffectKind::Delay(_)))
        .expect("Delay insert in demo")
        .id;

    // 1. Open Macros deck (`gk`), select unassigned M5 (`5`), rename via `r` (materializes slot).
    app.press_keys("gk5").unwrap();
    assert!(app.macros_focused());
    assert!(app.project().macro_control(4).is_none());
    // Alt+Backspace on unassigned slot must be a clean no-op (no ghost macro).
    app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT))
        .unwrap();
    assert!(app.project().macro_control(4).is_none());

    app.press_keys("rAcid Sweep<Enter>").unwrap();
    assert_eq!(app.project().macro_control(4).unwrap().name, "Acid Sweep");
    app.apply(Action::Undo).unwrap();
    assert!(app.project().macro_control(4).is_none());
    app.apply(Action::Redo).unwrap();
    assert_eq!(app.project().macro_control(4).unwrap().name, "Acid Sweep");

    // 2. Press `a` -> fuzzy target/param picker covers all 7 MacroTargetKind badges!
    app.press_keys("a").unwrap();
    assert_eq!(app.mode_name(), "PICK");
    let picker_items: Vec<String> = {
        let fv = app.frame_view(&AudioSnapshot::default(), "test");
        let Some(OverlayView::Picker(picker)) = fv.overlay else {
            panic!("expected Picker overlay");
        };
        picker.items.into_iter().map(|it| it.0).collect()
    };
    for badge in [
        "[RACK]", "[ZONE]", "[MOD]", "[CABLE]", "[FX]", "[CH]", "[SEND]",
    ] {
        assert!(
            picker_items.iter().any(|n| n.contains(badge)),
            "Macro bind picker must cover {badge} targets"
        );
    }

    // Bind 3 distinct target kinds via multi-token fuzzy search: Module (Saw Filter Cutoff),
    // Channel (Lead Pan), and Effect (Delay Mix), plus a Cable target.
    app.press_keys("/mod saw filter cutoff<Enter><Enter>")
        .unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
    assert_eq!(app.project().macro_control(4).unwrap().bindings.len(), 1);
    assert_eq!(app.view_state.macro_binding_cursor, Some(0));

    app.press_keys("a/ch lead pan<Enter><Enter>").unwrap();
    app.press_keys("a/fx delay mix<Enter><Enter>").unwrap();
    app.press_keys("a/cable saw<Enter><Enter>").unwrap();
    assert_eq!(app.project().macro_control(4).unwrap().bindings.len(), 4);
    assert_eq!(app.view_state.macro_binding_cursor, Some(3));

    // 3. Test `v` invert, `,`/`.` min adjust, `<`/`>` max adjust, `Alt+Backspace` reset, and `Alt+j`/`k` reorder.
    app.view_state.macro_binding_cursor = Some(0);
    app.press_keys("v").unwrap();
    let b0 = app.project().macro_control(4).unwrap().bindings[0];
    assert!(b0.is_inverted());
    assert!((b0.min - 1.0).abs() < 1e-4 && b0.max.abs() < 1e-4);
    app.press_keys("v").unwrap();
    let b0 = app.project().macro_control(4).unwrap().bindings[0];
    assert!(!b0.is_inverted());

    app.press_keys(".").unwrap(); // fine +2% on binding min
    assert!((app.project().macro_control(4).unwrap().bindings[0].min - 0.02).abs() < 1e-4);
    app.press_keys("<").unwrap(); // coarse -2% on binding max
    assert!((app.project().macro_control(4).unwrap().bindings[0].max - 0.98).abs() < 1e-4);
    app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT))
        .unwrap();
    let b0 = app.project().macro_control(4).unwrap().bindings[0];
    assert!(b0.min.abs() < 1e-4 && (b0.max - 1.0).abs() < 1e-4);

    // Reorder binding #0 down via Alt+j, then back up via Alt+k.
    let first_target = app.project().macro_control(4).unwrap().bindings[0].target;
    let second_target = app.project().macro_control(4).unwrap().bindings[1].target;
    app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT))
        .unwrap();
    assert_eq!(app.view_state.macro_binding_cursor, Some(1));
    assert_eq!(
        app.project().macro_control(4).unwrap().bindings[0].target,
        second_target
    );
    assert_eq!(
        app.project().macro_control(4).unwrap().bindings[1].target,
        first_target
    );
    app.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::ALT))
        .unwrap();
    assert_eq!(app.view_state.macro_binding_cursor, Some(0));
    assert_eq!(
        app.project().macro_control(4).unwrap().bindings[0].target,
        first_target
    );

    // 4. Dial header direct manipulation (`,`/`.`, `<`/`>`, `Alt+Backspace`, and `Enter` ValueInput).
    app.press_keys("k").unwrap(); // back to Dial header
    assert_eq!(app.view_state.macro_binding_cursor, None);
    app.press_keys(".").unwrap();
    assert!((app.project().macro_control(4).unwrap().value - 0.02).abs() < 1e-4);
    app.press_keys(">").unwrap();
    assert!((app.project().macro_control(4).unwrap().value - 0.12).abs() < 1e-4);
    app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT))
        .unwrap();
    assert!(app.project().macro_control(4).unwrap().value.abs() < 1e-4);
    app.press_keys("<Enter>65%<Enter>").unwrap();
    assert!((app.project().macro_control(4).unwrap().value - 0.65).abs() < 1e-4);

    // 5. `Enter` on binding rows jumps directly to bound target parameter's deck & control!
    // Binding 0: Saw Filter Cutoff -> Detail::Instrument(saw_id), RackTab::Panel, exact control.
    app.view_state.macro_binding_cursor = Some(0);
    app.press_keys("<Enter>").unwrap();
    assert_eq!(app.detail, Detail::Instrument(saw_id));
    assert_eq!(app.rack_ui.tab, RackTab::Panel);
    assert!(matches!(
        selected_target(&app),
        Some(ControlTarget::Param(t)) if t == first_target
    ));

    // Binding 1: Lead Pan -> Detail::Channel(Lead), exact Pan control.
    app.press_keys("gk5").unwrap();
    app.view_state.macro_binding_cursor = Some(1);
    app.press_keys("<Enter>").unwrap();
    assert_eq!(app.detail, Detail::Channel(ChannelRef::Channel(lead_ch)));
    assert!(matches!(
        selected_target(&app),
        Some(ControlTarget::Param(t)) if t == second_target
    ));

    // Binding 2: Delay Mix -> Detail::Effect(delay_id), exact Mix control.
    app.press_keys("gk5").unwrap();
    let fx_target = app.project().macro_control(4).unwrap().bindings[2].target;
    app.view_state.macro_binding_cursor = Some(2);
    app.press_keys("<Enter>").unwrap();
    assert_eq!(app.detail, Detail::Effect(delay_id));
    assert!(matches!(
        selected_target(&app),
        Some(ControlTarget::Param(t)) if t == fx_target
    ));

    // Binding 3: Saw Cable -> Detail::Instrument(saw_id), RackTab::Patchbay, exact cable selected.
    app.press_keys("gk5").unwrap();
    let ParamAddress::Cable { cable: cab_id, .. } =
        app.project().macro_control(4).unwrap().bindings[3].target
    else {
        panic!("expected Cable target at binding 3");
    };
    app.view_state.macro_binding_cursor = Some(3);
    app.press_keys("<Enter>").unwrap();
    assert_eq!(app.detail, Detail::Instrument(saw_id));
    assert_eq!(app.rack_ui.tab, RackTab::Patchbay);
    assert_eq!(app.rack_ui.patchbay_cable_id, Some(cab_id));

    // 6. `e` on Macro Dial header attaches driving Modulation, jumps to its pattern,
    // and `X` on automated Macro atomically unlinks modulation + clears slot in one Undo step!
    app.press_keys("gk5").unwrap();
    assert_eq!(app.view_state.macro_binding_cursor, None);
    app.press_keys("e").unwrap();
    assert_eq!(app.mode_name(), "PICK");
    // Single action when unautomated: "Attach driving Modulation…".
    app.press_keys("<Enter>").unwrap();
    assert_eq!(app.mode_name(), "WORKFLOW");
    // Accept prefilled Values pattern, M5 target (skips ModParam for Macro), default name.
    app.press_keys("<Enter><Enter><Enter>").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
    assert!(app
        .project()
        .modulations
        .iter()
        .any(|m| m.target == ModulationTarget::Macro { index: 4 }));

    // Now `e` on automated M5 opens Jump/Edit/Unlink menu; top choice jumps to driving Values pattern!
    app.press_keys("e<Enter>").unwrap();
    assert!(app.pattern_canvas_focused());
    let fv_pat = app.frame_view(&AudioSnapshot::default(), "test");
    let pat_view = fv_pat
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Pattern(p) => Some(p),
            _ => None,
        })
        .expect("PatternDeckView");
    assert!(
        pat_view
            .consumers
            .iter()
            .any(|c| c.label.contains("M5") && c.label.contains("Acid Sweep")),
        "Values pattern must show downstream consumer chip for M5 Acid Sweep, got {:?}",
        pat_view.consumers
    );

    // Return to `gk5` and press `X`: must atomically remove driving Modulation AND clear M5!
    app.press_keys("gk5X").unwrap();
    assert!(app.project().macro_control(4).is_none());
    assert!(app
        .project()
        .modulations
        .iter()
        .all(|m| m.target != ModulationTarget::Macro { index: 4 }));

    // Single Undo restores BOTH M5 (with all 4 bindings) AND its driving Modulation!
    app.apply(Action::Undo).unwrap();
    assert_eq!(app.project().macro_control(4).unwrap().bindings.len(), 4);
    assert!(app
        .project()
        .modulations
        .iter()
        .any(|m| m.target == ModulationTarget::Macro { index: 4 }));
}
