//! Integration tests for universal `e` link menus, live driver pills, `Values` pattern
//! downstream consumer chips, `Detail::Part` P-lock CRUD, and `WorkflowKind::Modulation` targeting.
use cadence_app::drafts::{DraftField, WorkflowContext, WorkflowDraft, WorkflowKind};
use cadence_app::view::{ControlView, DeckView, PatternDeckView};
use cadence_app::{Action, App, ControlTarget, Detail, MixerSlot, RackTab};
use cadence_core::material::{ModulationTarget, Project};
use cadence_core::mixer::{ChannelParam, ChannelRef, EffectKind, EffectParam};
use cadence_core::params::ParamAddress;
use cadence_core::presets::PresetCatalog;
use cadence_core::rack::{ModuleParam, ZoneParam};
use cadence_core::runtime::AudioSnapshot;

fn demo_app() -> App {
    App::new_in_memory(Project::demo(), "links-demo.json".into()).unwrap()
}

fn deck_controls(app: &App) -> Vec<ControlView> {
    let fv = app.frame_view(&AudioSnapshot::default(), "test");
    fv.decks
        .into_iter()
        .find_map(|d| match d.view {
            DeckView::Instrument(i) => Some(i.controls),
            DeckView::Object(o) => Some(o.controls),
            DeckView::Effect(e) => Some(e.controls),
            _ => None,
        })
        .expect("controls deck view")
}

fn pattern_view(app: &App) -> PatternDeckView {
    let fv = app.frame_view(&AudioSnapshot::default(), "test");
    fv.decks
        .into_iter()
        .find_map(|d| match d.view {
            DeckView::Pattern(p) => Some(p),
            _ => None,
        })
        .expect("PatternDeckView")
}

#[test]
fn universal_e_links_menu_driver_pills_and_values_consumers_across_decks() {
    let mut app = demo_app();
    let saw_id = app.project().instruments[0].id;
    let lead_ch = app.project().mixer.channels[0].id;
    let delay_id = app
        .project()
        .mixer
        .channels
        .iter()
        .flat_map(|c| c.inserts.iter())
        .find(|d| matches!(d.kind, EffectKind::Delay(_)))
        .expect("Delay insert in demo")
        .id;

    // 1. Open Rack Panel on Saw, focus Filter Cutoff, press `e` -> bind to M2.
    app.press_keys("gi").unwrap();
    assert_eq!(app.detail, Detail::Instrument(saw_id));
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
        .expect("Filter Cutoff control on Saw Panel");
    let ControlTarget::Param(cutoff_addr) = app.control_descriptors()[cutoff_idx].target else {
        unreachable!();
    };
    app.control_cursor = cutoff_idx;

    app.press_keys("e/Bind to Macro<Enter><Enter>/M2<Enter><Enter>")
        .unwrap();
    assert!(app
        .project()
        .macro_control(1)
        .is_some_and(|m| m.bindings.iter().any(|b| b.target == cutoff_addr)));

    // Driver pill `M2` must appear on Cutoff row, and `gi` LINKS must list the Macro binding with param name!
    let controls = deck_controls(&app);
    let cutoff_view = controls
        .iter()
        .find(|c| c.label == "Cutoff")
        .expect("Cutoff ControlView");
    assert!(
        cutoff_view.driver_pills.iter().any(|p| p == "M2"),
        "expected M2 driver pill on Cutoff, got {:?}",
        cutoff_view.driver_pills
    );
    assert!(cutoff_view.controlled);
    assert!(
        controls.iter().any(|c| {
            c.module == "LINKS" && c.label.contains("M2") && c.label.contains("Cutoff")
        }),
        "expected gi LINKS row showing M2 + Cutoff"
    );

    // 2. Press `e` on Cutoff again -> attach global Modulation!
    app.press_keys("e/Attach global Modulation<Enter><Enter><Enter><Enter><Enter><Enter>")
        .unwrap();
    assert!(app.project().modulations.iter().any(|m| {
        matches!(
            m.target,
            ModulationTarget::Instrument { instrument, .. } if instrument == saw_id
        )
    }));

    // 3. Press `e` on Cutoff -> Jump to driving Values pattern, inspect consumer chips, press `e` -> jump back!
    app.press_keys("e/Jump to pattern<Enter><Enter>").unwrap();
    assert!(app.pattern_canvas_focused());
    let pv = pattern_view(&app);
    assert!(
        !pv.consumers.is_empty(),
        "Values pattern driving a modulation must show downstream consumer chips"
    );
    app.press_keys("e/Saw<Enter><Enter>").unwrap();
    assert_eq!(app.detail, Detail::Instrument(saw_id));
    assert!(matches!(
        app.control_descriptors()
            .get(app.control_cursor)
            .map(|c| c.target.clone()),
        Some(ControlTarget::Param(addr)) if addr == cutoff_addr
    ));

    // 4. Focus the M2 MacroBinding row inside `gi` LINKS and press `x` -> unbinds cleanly!
    let link_idx = app
        .control_descriptors()
        .iter()
        .position(|c| matches!(c.target, ControlTarget::MacroBinding { index: 1, .. }))
        .expect("MacroBinding control in gi LINKS");
    app.control_cursor = link_idx;
    app.press_keys("x").unwrap();
    assert!(app
        .project()
        .macro_control(1)
        .is_some_and(|m| m.bindings.is_empty()));

    // 5. Verify `e` works on Rack Patchbay cable and Rack Zones (`ZoneParam::Gain`).
    app.rack_ui.tab = RackTab::Patchbay;
    let first_cab = app.instrument(saw_id).unwrap().rack.zones[0].cables[0].id;
    app.rack_ui.patchbay_cable_id = Some(first_cab);
    app.press_keys("e").unwrap();
    assert_eq!(app.mode_name(), "PICK");
    app.press_keys("<Esc>").unwrap();

    app.rack_ui.tab = RackTab::Zones;
    let zone_gain_idx = app
        .control_descriptors()
        .iter()
        .position(|c| {
            matches!(
                c.target,
                ControlTarget::Param(ParamAddress::Zone {
                    param: ZoneParam::Gain,
                    ..
                })
            )
        })
        .expect("Zone Gain control");
    app.control_cursor = zone_gain_idx;
    app.press_keys("e").unwrap();
    assert_eq!(app.mode_name(), "PICK");
    app.press_keys("<Esc>").unwrap();

    // 6. Verify `e` from Mixer Fader/Pan -> binds Lead Gain to M3, and Detail::Channel shows `M3` pill!
    app.press_keys("gm").unwrap();
    app.selected_channel = ChannelRef::Channel(lead_ch);
    app.view_state.mixer_slot = MixerSlot::Fader;
    app.press_keys("e/Bind to Macro<Enter><Enter>/M3<Enter><Enter>")
        .unwrap();
    assert!(app.project().macro_control(2).is_some_and(|m| {
        m.bindings.iter().any(|b| {
            b.target
                == ParamAddress::Channel {
                    channel: ChannelRef::Channel(lead_ch),
                    param: ChannelParam::Gain,
                }
        })
    }));
    app.apply(Action::SelectChannel {
        channel: ChannelRef::Channel(lead_ch),
    })
    .unwrap();
    let ch_controls = deck_controls(&app);
    let ch_gain = ch_controls
        .iter()
        .find(|c| c.label == "Trim")
        .expect("Channel Gain (Trim) ControlView");
    assert!(
        ch_gain.driver_pills.iter().any(|p| p == "M3"),
        "Detail::Channel Gain must display M3 driver pill, got {:?}",
        ch_gain.driver_pills
    );

    // 7. Verify `e` from Detail::Effect (Delay Mix) -> binds to M4 and shows `M4` pill!
    app.execute_command(&format!("fx {delay_id}")).unwrap();
    assert_eq!(app.detail, Detail::Effect(delay_id));
    let mix_idx = app
        .control_descriptors()
        .iter()
        .position(|c| {
            matches!(
                c.target,
                ControlTarget::Param(ParamAddress::Effect {
                    param: EffectParam::Mix,
                    ..
                })
            )
        })
        .expect("Delay Mix control");
    app.control_cursor = mix_idx;
    app.press_keys("e/Bind to Macro<Enter><Enter>/M4<Enter><Enter>")
        .unwrap();
    let fx_controls = deck_controls(&app);
    let mix_view = fx_controls
        .iter()
        .find(|c| c.label == "Mix")
        .expect("Delay Mix ControlView");
    assert!(
        mix_view.driver_pills.iter().any(|p| p == "M4"),
        "Detail::Effect Mix must display M4 driver pill, got {:?}",
        mix_view.driver_pills
    );

    // 8. Verify `e` on a Values pattern from Bank Material (`gb`) opens downstream consumers menu!
    let val_pat_id = app
        .project()
        .modulations
        .iter()
        .find(|m| {
            matches!(
                m.target,
                ModulationTarget::Instrument { instrument, .. } if instrument == saw_id
            )
        })
        .unwrap()
        .pattern;
    app.press_keys("gb").unwrap();
    app.view_state.bank_mode = cadence_app::BankMode::Material;
    app.selected_pattern = Some(val_pat_id);
    app.press_keys("e").unwrap();
    assert_eq!(app.mode_name(), "PICK");
    app.press_keys("<Esc>").unwrap();
}

#[test]
fn detail_part_parameter_link_crud_driver_pills_and_undo() {
    let mut app = demo_app();
    let part_id = app.project().parts[0].id; // "Intro" (Saw, no initial links)
    let saw_id = app.part(part_id).unwrap().instrument;
    let initial_links = app.part(part_id).unwrap().parameter_links.len();

    // Open Detail::Part via `:part`.
    app.execute_command(&format!("part {part_id}")).unwrap();
    assert_eq!(app.detail, Detail::Part(part_id));

    // 1. Press `a` -> choose Filter Cutoff -> choose `+ New Values pattern` (atomic 1-transaction create!).
    let pats_before = app.project().patterns.len();
    app.press_keys("a/Filter Cutoff<Enter><Enter>/New Values<Enter><Enter>")
        .unwrap();
    assert_eq!(app.project().patterns.len(), pats_before + 1);
    assert_eq!(
        app.part(part_id).unwrap().parameter_links.len(),
        initial_links + 1
    );
    let added_link = app.part(part_id).unwrap().parameter_links[initial_links].clone();

    // Unified `param_is_controlled` + driver pills must mark Cutoff controlled by `Link:Intro`!
    app.execute_command(&format!("instrument {saw_id}"))
        .unwrap();
    let saw_controls = deck_controls(&app);
    let cutoff = saw_controls
        .iter()
        .find(|c| c.label == "Cutoff")
        .expect("Cutoff control");
    assert!(cutoff.controlled);
    assert!(
        cutoff.driver_pills.iter().any(|p| p.starts_with("Link:")),
        "expected Link:<Part> driver pill on P-locked Cutoff, got {:?}",
        cutoff.driver_pills
    );

    // 2. Return to Detail::Part, focus the P-lock row, test `Enter` jump to Values pattern,
    //    `e` edit/unlink menu, and direct `x` unlink + single Undo!
    app.execute_command(&format!("part {part_id}")).unwrap();
    let link_ctrl = app
        .control_descriptors()
        .iter()
        .position(|c| {
            matches!(
                c.target,
                ControlTarget::PartParamLink { part, index, .. }
                    if part == part_id && index == initial_links
            )
        })
        .expect("PartParamLink row in Detail::Part");
    app.control_cursor = link_ctrl;
    app.press_keys("<Enter>").unwrap();
    assert!(app.pattern_canvas_focused());
    assert_eq!(app.selected_pattern, Some(added_link.pattern));

    // Re-open Detail::Part, focus P-lock, open `e` menu -> Change target parameter -> Resonance.
    app.execute_command(&format!("part {part_id}")).unwrap();
    app.control_cursor = link_ctrl;
    app.press_keys("e/Change target<Enter><Enter>/Filter Resonance<Enter><Enter>")
        .unwrap();
    assert!(matches!(
        app.part(part_id).unwrap().parameter_links[initial_links].target,
        cadence_core::params::InstrumentParam::Module {
            param: ModuleParam::Resonance,
            ..
        }
    ));

    // Direct `x` unlinks the P-lock; `u` restores it!
    app.control_cursor = link_ctrl;
    app.press_keys("x").unwrap();
    assert_eq!(
        app.part(part_id).unwrap().parameter_links.len(),
        initial_links
    );
    app.apply(Action::Undo).unwrap();
    assert_eq!(
        app.part(part_id).unwrap().parameter_links.len(),
        initial_links + 1
    );
}

#[test]
fn modulation_workflow_targets_focused_mixer_channel_unconfigured_macros_and_automatable_zone_params(
) {
    let mut app = demo_app();
    let bass_ch = app
        .project()
        .mixer
        .channels
        .iter()
        .find(|c| c.name == "Bass")
        .expect("Bass channel")
        .id;
    let music_ch = app
        .project()
        .mixer
        .channels
        .iter()
        .find(|c| c.name == "Music")
        .expect("Music group channel")
        .id;
    let echo_ret = app
        .project()
        .mixer
        .channels
        .iter()
        .find(|c| c.name == "Echo")
        .expect("Echo return")
        .id;

    // 1. From Mixer (`gm`) on Bass channel Pan slot, `ny` must pre-fill Bass Pan (NOT background Saw instrument!).
    app.press_keys("gm").unwrap();
    app.selected_channel = ChannelRef::Channel(bass_ch);
    app.view_state.mixer_slot = MixerSlot::Pan;
    let mods_before = app.project().modulations.len();
    app.press_keys("ny<Enter><Enter><Enter><Enter>").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
    assert_eq!(app.project().modulations.len(), mods_before + 1);
    assert_eq!(
        app.project().modulations.last().unwrap().target,
        ModulationTarget::ChannelPan {
            channel: ChannelRef::Channel(bass_ch)
        }
    );

    // 2. From Mixer (`gm`) on Music channel Send slot (Music -> Echo), `ny` pre-fills Send target!
    app.selected_channel = ChannelRef::Channel(music_ch);
    app.view_state.mixer_slot = MixerSlot::Send(0);
    app.press_keys("ny<Enter><Enter><Enter><Enter>").unwrap();
    assert_eq!(
        app.project().modulations.last().unwrap().target,
        ModulationTarget::Send {
            channel: ChannelRef::Channel(music_ch),
            target: echo_ret
        }
    );

    // 3. Targeting an unconfigured Macro slot (M8) via `ny` atomically creates M8 + Modulation in one transaction!
    // Macro target skips ModParam: /M8<Enter> locks filter, <Enter> selects M8, <Enter> commits Name.
    assert!(app.project().macro_control(7).is_none());
    app.press_keys("ny<Enter>/M8<Enter><Enter><Enter>").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
    assert!(app.project().macro_control(7).is_some());
    assert_eq!(
        app.project().modulations.last().unwrap().target,
        ModulationTarget::Macro { index: 7 }
    );
    // Single Undo removes BOTH the Modulation and the auto-created M8 slot!
    app.apply(Action::Undo).unwrap();
    assert!(app.project().macro_control(7).is_none());
    assert!(app
        .project()
        .modulations
        .iter()
        .all(|m| m.target != ModulationTarget::Macro { index: 7 }));

    // 4. Verify `WorkflowDraft` `DraftField::ModParam` derives ZoneParam choices from `ZoneParam::ALL.filter(is_automatable)`.
    let project = app.project();
    let presets = PresetCatalog::empty();
    let ctx = WorkflowContext {
        instrument: Some(project.instruments[0].id),
        ..WorkflowContext::default()
    };
    let mut draft = WorkflowDraft::new(WorkflowKind::Modulation, project, ctx, &presets);
    // Advance from ModPattern -> ModTarget -> ModParam.
    let enter =
        cadence_app::KeyEvent::new(cadence_app::KeyCode::Enter, cadence_app::KeyModifiers::NONE);
    draft.handle_key(project, enter);
    draft.handle_key(project, enter);
    let view = draft.view(project);
    assert_eq!(view.field, DraftField::ModParam);
    let labels: Vec<String> = view.candidates.iter().map(|c| c.label.clone()).collect();
    for zp in ZoneParam::ALL {
        let expected = format!("Zone · {}", zp.label());
        assert_eq!(
            labels.iter().any(|l| l == &expected),
            zp.is_automatable(),
            "ZoneParam::{zp:?} presence in ModParam must match is_automatable()"
        );
    }
}

#[test]
fn effect_and_channel_links_rows_support_menu_unlink_gs_and_exact_macro_jump() {
    let mut app = demo_app();
    let delay_id = app
        .project()
        .mixer
        .channels
        .iter()
        .flat_map(|c| c.inserts.iter())
        .find(|d| matches!(d.kind, EffectKind::Delay(_)))
        .expect("Delay insert in demo")
        .id;
    let lead_ch = app.project().mixer.channels[0].id;
    let echo_ret = app
        .project()
        .mixer
        .channels
        .iter()
        .find(|c| c.name == "Echo")
        .expect("Echo return")
        .id;
    let val_pat = app
        .project()
        .patterns
        .iter()
        .find(|p| {
            app.project().pattern_type(p.id).is_ok_and(|t| {
                t == cadence_core::material::PatternType::Values(
                    cadence_core::material::ValueUnit::Normalized,
                )
            })
        })
        .expect("normalized Values pattern")
        .id;

    // Bind Delay Mix to M2 (binding 0) and attach a global Modulation to Delay Feedback.
    let delay_mix = ParamAddress::Effect {
        device: delay_id,
        param: EffectParam::Mix,
    };
    app.apply(Action::BindMacro {
        index: 1,
        target: delay_mix,
        min: 0.1,
        max: 0.8,
    })
    .unwrap();
    app.apply(Action::CreateModulation {
        name: "DelayFbAuto".into(),
        target: ModulationTarget::Effect {
            device: delay_id,
            parameter: EffectParam::Feedback,
        },
        pattern: val_pat,
    })
    .unwrap();

    // 1. Inspect Detail::Effect LINKS rows: must emit typed ModulationLink and MacroBinding targets.
    app.execute_command(&format!("fx {delay_id}")).unwrap();
    assert_eq!(app.detail, Detail::Effect(delay_id));
    let mod_idx = app
        .control_descriptors()
        .iter()
        .position(|c| {
            matches!(
                c.target,
                ControlTarget::ModulationLink { pattern, .. } if pattern == val_pat
            )
        })
        .expect("Detail::Effect ModulationLink row");
    let mac_idx = app
        .control_descriptors()
        .iter()
        .position(|c| {
            matches!(
                c.target,
                ControlTarget::MacroBinding {
                    index: 1,
                    binding: 0
                }
            )
        })
        .expect("Detail::Effect MacroBinding row");

    // `e` opens Links menu on both Effect LINKS rows.
    app.control_cursor = mod_idx;
    app.press_keys("e").unwrap();
    assert_eq!(app.mode_name(), "PICK");
    app.press_keys("<Esc>").unwrap();

    app.control_cursor = mac_idx;
    app.press_keys("e").unwrap();
    assert_eq!(app.mode_name(), "PICK");
    app.press_keys("<Esc>").unwrap();

    // `Enter` on Effect MacroBinding jumps directly to Macros on exact slot & binding index!
    app.control_cursor = mac_idx;
    app.press_keys("<Enter>").unwrap();
    assert!(app.macros_focused());
    assert_eq!(app.view_state.macro_cursor, 1);
    assert_eq!(app.view_state.macro_binding_cursor, Some(0));

    // `gs` and `Enter` on Effect ModulationLink jump cleanly to the driving Values pattern!
    app.execute_command(&format!("fx {delay_id}")).unwrap();
    app.control_cursor = mod_idx;
    app.press_keys("gs").unwrap();
    assert!(app.pattern_canvas_focused());
    assert_eq!(app.selected_pattern, Some(val_pat));

    app.execute_command(&format!("fx {delay_id}")).unwrap();
    app.control_cursor = mod_idx;
    app.press_keys("<Enter>").unwrap();
    assert!(app.pattern_canvas_focused());
    assert_eq!(app.selected_pattern, Some(val_pat));

    // `x` unlinks both ModulationLink and MacroBinding cleanly on Detail::Effect.
    app.execute_command(&format!("fx {delay_id}")).unwrap();
    let mod_idx = app
        .control_descriptors()
        .iter()
        .position(|c| matches!(c.target, ControlTarget::ModulationLink { .. }))
        .unwrap();
    app.control_cursor = mod_idx;
    app.press_keys("x").unwrap();
    assert!(!app
        .project()
        .modulations
        .iter()
        .any(|m| m.name == "DelayFbAuto"));

    let mac_idx = app
        .control_descriptors()
        .iter()
        .position(|c| matches!(c.target, ControlTarget::MacroBinding { index: 1, .. }))
        .unwrap();
    app.control_cursor = mac_idx;
    app.press_keys("x").unwrap();
    assert!(app
        .project()
        .macro_control(1)
        .is_some_and(|m| m.bindings.is_empty()));

    // 2. Detail::Channel Macro LINKS rows for Gain, Pan, and Send + Enter jump + x unbind!
    for addr in [
        ParamAddress::Channel {
            channel: ChannelRef::Channel(lead_ch),
            param: ChannelParam::Gain,
        },
        ParamAddress::Channel {
            channel: ChannelRef::Channel(lead_ch),
            param: ChannelParam::Pan,
        },
        ParamAddress::Send {
            channel: ChannelRef::Channel(lead_ch),
            target: echo_ret,
        },
    ] {
        app.apply(Action::BindMacro {
            index: 2,
            target: addr,
            min: 0.0,
            max: 1.0,
        })
        .unwrap();
    }

    app.apply(Action::SelectChannel {
        channel: ChannelRef::Channel(lead_ch),
    })
    .unwrap();
    assert_eq!(app.detail, Detail::Channel(ChannelRef::Channel(lead_ch)));
    let ch_macro_rows: Vec<(usize, String)> = app
        .control_descriptors()
        .iter()
        .enumerate()
        .filter(|(_, c)| matches!(c.target, ControlTarget::MacroBinding { index: 2, .. }))
        .map(|(i, c)| (i, c.label.clone()))
        .collect();
    assert_eq!(
        ch_macro_rows.len(),
        3,
        "Detail::Channel must emit LINKS rows for Channel Gain, Pan, and Send macro bindings: {ch_macro_rows:?}"
    );
    assert!(ch_macro_rows.iter().any(|(_, l)| l.contains("Gain")));
    assert!(ch_macro_rows.iter().any(|(_, l)| l.contains("Pan")));
    assert!(ch_macro_rows.iter().any(|(_, l)| l.contains("Send → Echo")));

    // `e` opens links menu, `Enter` jumps to exact Macro binding, and `x` unbinds.
    app.control_cursor = ch_macro_rows[1].0;
    app.press_keys("e").unwrap();
    assert_eq!(app.mode_name(), "PICK");
    app.press_keys("<Esc>").unwrap();

    app.press_keys("<Enter>").unwrap();
    assert!(app.macros_focused());
    assert_eq!(app.view_state.macro_cursor, 2);
    assert_eq!(app.view_state.macro_binding_cursor, Some(1));

    app.apply(Action::SelectChannel {
        channel: ChannelRef::Channel(lead_ch),
    })
    .unwrap();
    app.control_cursor = ch_macro_rows[2].0;
    app.press_keys("x").unwrap();
    assert_eq!(app.project().macro_control(2).unwrap().bindings.len(), 2);
}

#[test]
fn macros_e_filter_env_links_multipart_plock_and_values_consumers() {
    let mut app = demo_app();
    let saw_id = app.project().instruments[0].id;
    let val_pat = app
        .project()
        .patterns
        .iter()
        .find(|p| {
            app.project().pattern_type(p.id).is_ok_and(|t| {
                t == cadence_core::material::PatternType::Values(
                    cadence_core::material::ValueUnit::Normalized,
                )
            })
        })
        .expect("Values pattern")
        .id;

    // 1. Macros `e` on M1 dial header opens driving Modulation menu -> attach global mod to M1!
    app.press_keys("gk1").unwrap();
    assert!(app.macros_focused());
    assert_eq!(app.view_state.macro_cursor, 0);
    app.press_keys("e").unwrap();
    assert_eq!(app.mode_name(), "PICK");
    let picker = app.picker_view().expect("Macros e picker");
    assert!(
        picker
            .items
            .iter()
            .any(|(name, _, _)| name.contains("Attach")),
        "expected Attach global Modulation option on unmodulated M1, got {:?}",
        picker.items
    );
    app.press_keys("<Esc>").unwrap();

    app.apply(Action::SetMacro {
        index: 0,
        value: 0.5,
    })
    .unwrap();
    app.apply(Action::CreateModulation {
        name: "Macro1Mod".into(),
        target: ModulationTarget::Macro { index: 0 },
        pattern: val_pat,
    })
    .unwrap();
    app.press_keys("e/Jump to pattern<Enter><Enter>").unwrap();
    assert!(app.pattern_canvas_focused());
    assert_eq!(app.selected_pattern, Some(val_pat));

    // 2. FilterEnv `e` and `x`: add a Filter insert, bind custom env_pattern_id, test `e` menu & `x` unlink!
    app.execute_command("fx add filter master").unwrap();
    let filter_id = app
        .project()
        .mixer
        .master
        .inserts
        .last()
        .expect("Filter on master")
        .id;
    app.execute_command(&format!("fx {filter_id}")).unwrap();
    let env_src_idx = app
        .control_descriptors()
        .iter()
        .position(|c| matches!(c.target, ControlTarget::FilterEnvSource(d) if d == filter_id))
        .expect("FilterEnvSource row");
    app.control_cursor = env_src_idx;
    // Pick custom Values pattern as Filter env source via `:param source <val_pat>`.
    app.execute_command(&format!("param source {val_pat}"))
        .unwrap();
    assert!(matches!(
        app.effect(filter_id).unwrap().1.kind,
        EffectKind::Filter(fp) if fp.env_pattern_id == Some(val_pat)
    ));
    // `e` opens FilterEnv links menu with Jump + Unlink options.
    app.control_cursor = env_src_idx;
    app.press_keys("e").unwrap();
    assert_eq!(app.mode_name(), "PICK");
    let fp_picker = app.picker_view().unwrap();
    assert!(fp_picker
        .items
        .iter()
        .any(|(n, _, _)| n.contains("Jump to")));
    assert!(fp_picker.items.iter().any(|(n, _, _)| n.contains("Unlink")));
    app.press_keys("<Esc>").unwrap();

    // Verify Values pattern consumers lists BOTH M1 Modulation AND Filter Env!
    app.apply(Action::SelectPattern { pattern: val_pat })
        .unwrap();
    let pv = pattern_view(&app);
    assert!(
        pv.consumers.iter().any(|c| c.label.contains("M1")),
        "expected M1 consumer chip, got {:?}",
        pv.consumers
    );
    assert!(
        pv.consumers.iter().any(|c| c.label.contains("Filter")),
        "expected Filter Env consumer chip, got {:?}",
        pv.consumers
    );

    // Direct `x` on FilterEnvSource unlinks cleanly back to Groove Bank!
    app.execute_command(&format!("fx {filter_id}")).unwrap();
    app.control_cursor = env_src_idx;
    app.press_keys("x").unwrap();
    assert!(matches!(
        app.effect(filter_id).unwrap().1.kind,
        EffectKind::Filter(fp) if fp.env_pattern_id.is_none()
    ));

    // 3. Multi-part `PickPartForParamLink`: ensure >= 2 parts target Saw, then `e` -> Attach Part P-lock!
    let saw_parts_before = app
        .project()
        .parts
        .iter()
        .filter(|p| p.instrument == saw_id)
        .count();
    if saw_parts_before < 2 {
        let first_notes = app.project().parts[0].pattern;
        app.execute_command(&format!("part SawSecond {first_notes} {saw_id}"))
            .unwrap();
    }
    let saw_parts: Vec<u64> = app
        .project()
        .parts
        .iter()
        .filter(|p| p.instrument == saw_id)
        .map(|p| p.id)
        .collect();
    assert!(saw_parts.len() >= 2);
    let second_part = saw_parts[1];
    let second_part_name = app.part(second_part).unwrap().name.clone();

    app.execute_command(&format!("instrument {saw_id}"))
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
    // `e` -> Set Part P-lock… -> PickPartForParamLink -> choose second part -> pick New Values pattern!
    app.press_keys(&format!(
        "e/Set Part P-lock<Enter><Enter>/{second_part_name}<Enter><Enter>/New Values<Enter><Enter>"
    ))
    .unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
    assert!(
        !app.part(second_part).unwrap().parameter_links.is_empty(),
        "PickPartForParamLink must attach P-lock onto the chosen Part"
    );
}

#[test]
fn rack_zones_x_unlinks_focused_link_row_without_deleting_active_zone() {
    let mut app = demo_app();
    let saw_id = app.project().instruments[0].id;
    // Duplicate zone so Saw has 2 zones (making zone deletion otherwise possible via `x`).
    app.execute_command(&format!("instrument {saw_id}"))
        .unwrap();
    app.rack_ui.tab = RackTab::Zones;
    app.press_keys("d").unwrap();
    let zones_before = app.instrument(saw_id).unwrap().rack.zones.len();
    assert_eq!(zones_before, 2);

    // Bind active zone's Gain to M4 so a LINKS MacroBinding row appears inside RackTab::Zones!
    let zone0_id = app.instrument(saw_id).unwrap().rack.zones[0].id;
    let zone_gain = ParamAddress::Zone {
        instrument: saw_id,
        zone: zone0_id,
        param: ZoneParam::Gain,
    };
    app.apply(Action::BindMacro {
        index: 3,
        target: zone_gain,
        min: 0.2,
        max: 0.9,
    })
    .unwrap();
    assert_eq!(app.project().macro_control(3).unwrap().bindings.len(), 1);

    // Focus the MacroBinding LINKS row inside Zones tab and press `x`.
    app.rack_ui.tab = RackTab::Zones;
    let link_idx = app
        .control_descriptors()
        .iter()
        .position(|c| matches!(c.target, ControlTarget::MacroBinding { index: 3, .. }))
        .expect("MacroBinding LINKS row inside RackTab::Zones");
    app.control_cursor = link_idx;
    app.press_keys("x").unwrap();

    // Must unlink the Macro binding WITHOUT deleting the active Zone!
    assert_eq!(
        app.instrument(saw_id).unwrap().rack.zones.len(),
        zones_before,
        "x on a LINKS row in Rack Zones must NOT delete the active Zone"
    );
    assert!(
        app.project().macro_control(3).unwrap().bindings.is_empty(),
        "x on a LINKS row in Rack Zones must unbind the focused MacroBinding"
    );

    // Undo restores the binding cleanly.
    app.apply(Action::Undo).unwrap();
    assert_eq!(app.project().macro_control(3).unwrap().bindings.len(), 1);
}

#[test]
fn jump_to_param_address_covers_all_seven_target_kinds_including_unpinned_rack() {
    let mut app = demo_app();
    let saw_id = app.project().instruments[0].id;
    // Clear all panel_pins on Saw to verify Rack target still lands on Panel slot 0 (no stale has_pins fallback!).
    let mut unpinned = app.instrument(saw_id).unwrap().rack.clone();
    unpinned.panel_pins.clear();
    app.apply(Action::SetRack {
        instrument: saw_id,
        rack: unpinned,
    })
    .unwrap();
    assert!(app.instrument(saw_id).unwrap().rack.panel_pins.is_empty());

    let zone0 = &app.instrument(saw_id).unwrap().rack.zones[0];
    let z0_id = zone0.id;
    let mod_id = zone0
        .modules
        .iter()
        .find(|m| m.kind.tag() == cadence_core::rack::ModuleKindTag::Filter)
        .expect("Filter module")
        .id;
    let cab_id = zone0.cables[0].id;
    let lead_ch = app.project().mixer.channels[0].id;
    let echo_ret = app
        .project()
        .mixer
        .channels
        .iter()
        .find(|c| c.name == "Echo")
        .expect("Echo return")
        .id;
    let delay_id = app
        .project()
        .mixer
        .channels
        .iter()
        .flat_map(|c| c.inserts.iter())
        .find(|d| matches!(d.kind, EffectKind::Delay(_)))
        .expect("Delay insert")
        .id;

    let targets = vec![
        ParamAddress::Rack {
            instrument: saw_id,
            param: cadence_core::rack::RackParam::Gain,
        },
        ParamAddress::Zone {
            instrument: saw_id,
            zone: z0_id,
            param: ZoneParam::Gain,
        },
        ParamAddress::Module {
            instrument: saw_id,
            zone: z0_id,
            module: mod_id,
            param: ModuleParam::Cutoff,
        },
        ParamAddress::Cable {
            instrument: saw_id,
            zone: z0_id,
            cable: cab_id,
        },
        ParamAddress::Effect {
            device: delay_id,
            param: EffectParam::Mix,
        },
        ParamAddress::Channel {
            channel: ChannelRef::Channel(lead_ch),
            param: ChannelParam::Pan,
        },
        ParamAddress::Send {
            channel: ChannelRef::Channel(lead_ch),
            target: echo_ret,
        },
    ];
    for &addr in &targets {
        app.apply(Action::BindMacro {
            index: 4,
            target: addr,
            min: 0.0,
            max: 1.0,
        })
        .unwrap();
    }

    for (b_idx, &addr) in targets.iter().enumerate() {
        app.press_keys("gk5").unwrap();
        app.view_state.macro_binding_cursor = Some(b_idx);
        app.press_keys("<Enter>").unwrap();
        let focused_target = app.control_descriptors()[app.control_cursor].target.clone();
        match addr {
            ParamAddress::Rack { instrument, .. } => {
                assert_eq!(app.detail, Detail::Instrument(instrument));
                assert_eq!(
                    app.rack_ui.tab,
                    RackTab::Panel,
                    "Rack param must open Panel slot 0 even with zero panel_pins"
                );
                assert_eq!(app.rack_ui.panel_slot, 0);
                assert!(matches!(focused_target, ControlTarget::Param(a) if a == addr));
            }
            ParamAddress::Zone { instrument, .. } => {
                assert_eq!(app.detail, Detail::Instrument(instrument));
                assert_eq!(app.rack_ui.tab, RackTab::Zones);
                assert!(matches!(focused_target, ControlTarget::Param(a) if a == addr));
            }
            ParamAddress::Module { instrument, .. } => {
                assert_eq!(app.detail, Detail::Instrument(instrument));
                assert_eq!(app.rack_ui.tab, RackTab::Panel);
                assert!(matches!(focused_target, ControlTarget::Param(a) if a == addr));
            }
            ParamAddress::Cable {
                instrument, cable, ..
            } => {
                assert_eq!(app.detail, Detail::Instrument(instrument));
                assert_eq!(app.rack_ui.tab, RackTab::Patchbay);
                assert_eq!(app.rack_ui.patchbay_cable_id, Some(cable));
            }
            ParamAddress::Effect { device, .. } => {
                assert_eq!(app.detail, Detail::Effect(device));
                assert!(matches!(focused_target, ControlTarget::Param(a) if a == addr));
            }
            ParamAddress::Channel { channel, .. } | ParamAddress::Send { channel, .. } => {
                assert_eq!(app.detail, Detail::Channel(channel));
                assert!(matches!(focused_target, ControlTarget::Param(a) if a == addr));
            }
        }
    }
}
