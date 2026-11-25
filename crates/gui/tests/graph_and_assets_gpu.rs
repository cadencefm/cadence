//! Headless Metal/wgpu GPU visual tests for the Signal Graph, Asset Hubs & Layout:
//! 1. Interactive Signal Graph (`gG`) with indented FX insert cards, bypassed FX styling,
//!    Modulation/Macro/Sidechain/Send cables, `selected_cables` pill, and footer legend.
//! 2. FileHub `Import` tab surfacing `PRESET LIBRARY` (Rack synth & kit presets) + reload hint.
//! 3. Settings modal (`g,`) exposing `ASSET DIRECTORIES` (`sample_dirs` / `preset_dirs`) + inline editor.
//! 4. Deep nested `PlacedPart` Authoring draft at compact height exercising left field-list scroll + `▲/▼`.
//! 5. 2-column `draw_control_list` with `GROUP (CONT.)` continuation header (no orphaned unheaded col 2).
//! 6. Rack `Out` module faceplate displaying contiguous `OUT / VOICE / RACK` module headers.
//!
//! Set `CADENCE_DUMP_GRAPH_ASSETS_DIR=/path/to/dir` to save PNG screenshots for visual inspection.

mod common;

use cadence_app::authoring::{DraftField, DraftFieldView, DraftInput, DraftView};
use cadence_app::host_io::{HostIo, MemoryHostIo};
use cadence_app::view::{ControlView, GraphTarget, OverlayView, RackTab};
use cadence_app::{Action, App, FileHubTab, PickerView};
use cadence_core::material::Project;
use cadence_core::mixer::{ChannelKind, ChannelRef};
use cadence_core::presets::{InstrumentPreset, PresetCatalog};
use cadence_core::rack::{ModuleKind, RackPatch};
use cadence_core::runtime::AudioSnapshot;
use cadence_gui::decks::object::draw_control_list;
use cadence_gui::display::{DisplayList, Rect};
use cadence_gui::headless::{distinct_pixel_ratio, HeadlessGpu};
use cadence_gui::overlays::draw_overlay;
use cadence_gui::theme::Theme;
use std::sync::Arc;

const DUMP_ENV: &str = "CADENCE_DUMP_GRAPH_ASSETS_DIR";

#[test]
fn draw_control_list_two_col_never_starts_col2_with_orphaned_unheaded_row() {
    let theme = Theme::cadence_dark();
    // Case 1: Large group `OSC A` (9 items = 10 entries incl. header) + `FILTER` (4 items)
    // in a compact 2-col area (rows_per_col = 8) -> `OSC A` MUST span the midpoint and emit
    // `OSC A (CONT.)` at the top of column 2.
    let mut controls = Vec::new();
    for i in 0..9 {
        controls.push(ControlView {
            label: format!("Param A{i}"),
            display: format!("{i}%"),
            normalized: Some(i as f32 / 8.0),
            module: "OSC A",
            selected: i == 0,
            controlled: false,
            driver_pills: Vec::new(),
            pinned: false,
            midi_badge: None,
            midi_learning: false,
        });
    }
    for i in 0..4 {
        controls.push(ControlView {
            label: format!("Param B{i}"),
            display: format!("{i}%"),
            normalized: Some(i as f32 / 3.0),
            module: "FILTER",
            selected: false,
            controlled: false,
            driver_pills: Vec::new(),
            pinned: false,
            midi_badge: None,
            midi_learning: false,
        });
    }

    let mut dl = DisplayList::new(960.0, 180.0, 1.0);
    let area = Rect::new(0.0, 0.0, 960.0, 180.0);
    draw_control_list(&mut dl, area, &controls, 0, &theme);

    let texts: Vec<&str> = dl.base.texts.iter().map(|t| t.text.as_str()).collect();
    assert!(
        texts.iter().any(|t| t.contains("OSC A (CONT.)")),
        "expected `OSC A (CONT.)` continuation header when a large group spans the 2-col midpoint, got: {texts:?}"
    );

    // Case 2: Small group `ENV` starting near bottom of col 1 (4 items in OSC A, 3 in ENV, 2 in LFO)
    // pulls `ENV` whole into col 2 (no `ENV (CONT.)` split and no orphaned control at top of col 2).
    let mut small_ctrls = Vec::new();
    for i in 0..4 {
        small_ctrls.push(ControlView {
            label: format!("O{i}"),
            display: "0".into(),
            normalized: Some(0.5),
            module: "OSC A",
            selected: i == 0,
            controlled: false,
            driver_pills: Vec::new(),
            pinned: false,
            midi_badge: None,
            midi_learning: false,
        });
    }
    for i in 0..3 {
        small_ctrls.push(ControlView {
            label: format!("E{i}"),
            display: "0".into(),
            normalized: Some(0.5),
            module: "ENV",
            selected: false,
            controlled: false,
            driver_pills: Vec::new(),
            pinned: false,
            midi_badge: None,
            midi_learning: false,
        });
    }
    for i in 0..2 {
        small_ctrls.push(ControlView {
            label: format!("L{i}"),
            display: "0".into(),
            normalized: Some(0.5),
            module: "LFO",
            selected: false,
            controlled: false,
            driver_pills: Vec::new(),
            pinned: false,
            midi_badge: None,
            midi_learning: false,
        });
    }
    let mut dl2 = DisplayList::new(960.0, 190.0, 1.0);
    draw_control_list(
        &mut dl2,
        Rect::new(0.0, 0.0, 960.0, 190.0),
        &small_ctrls,
        0,
        &theme,
    );
    let col2_top = dl2
        .base
        .texts
        .iter()
        .filter(|t| t.rect.x > 400.0)
        .min_by(|a, b| a.rect.y.partial_cmp(&b.rect.y).unwrap())
        .expect("column 2 texts");
    assert_eq!(
        col2_top.text, "ENV",
        "small split group `ENV` must be pulled intact to row 0 of column 2"
    );
}

#[test]
fn draw_overlay_authoring_left_pane_scrolls_and_picker_centers_cursor() {
    let theme = Theme::cadence_dark();
    let viewport = Rect::new(0.0, 0.0, 1000.0, 360.0);

    // 1. Authoring overlay with 14 fields and active field at index 11 -> left pane MUST scroll & show ▲.
    let fields: Vec<DraftFieldView> = (0..14)
        .map(|i| DraftFieldView {
            label: format!("Field{i:02}"),
            value: format!("Val{i:02}"),
            active: i == 11,
        })
        .collect();
    let draft = DraftView {
        title: "PLACED PART".into(),
        field: DraftField::Start,
        field_label: "Start bar".into(),
        input: DraftInput::Number,
        text: "1".into(),
        replace_on_type: true,
        query: String::new(),
        searching: false,
        candidates: Vec::new(),
        selected: None,
        fields,
        preview: vec!["Preview".into()],
        summary: "Summary".into(),
        error: None,
        hints: vec![("Enter", "accept")],
    };
    let mut dl = DisplayList::new(viewport.w, viewport.h, 1.0);
    dl.begin_overlay();
    draw_overlay(&mut dl, viewport, &OverlayView::Authoring(draft), &theme);
    let auth_texts: Vec<&str> = dl.overlay.texts.iter().map(|t| t.text.as_str()).collect();
    assert!(
        auth_texts.iter().any(|t| t.contains("Field11")),
        "Active field #11 must be scrolled into view: {auth_texts:?}"
    );
    assert!(
        auth_texts.iter().any(|t| t.contains("▲") && t.contains("12/14")),
        "Expected vertical scroll indicator (`▲ ... 12/14`) on left field list, got: {auth_texts:?}"
    );

    // 2. Picker overlay with 24 items and cursor at 15 -> centered scroll shows items BOTH before & after 15.
    let picker = PickerView {
        title: "ROUTE CHANNEL".into(),
        filter: None,
        searching: false,
        cursor: 15,
        audition: false,
        items: (0..24)
            .map(|i| (format!("Channel {i:02}"), "Group".into(), i == 15))
            .collect(),
    };
    let mut dl_p = DisplayList::new(1000.0, 420.0, 1.0);
    dl_p.begin_overlay();
    draw_overlay(
        &mut dl_p,
        Rect::new(0.0, 0.0, 1000.0, 420.0),
        &OverlayView::Picker(picker),
        &theme,
    );
    let pick_texts: Vec<&str> = dl_p.overlay.texts.iter().map(|t| t.text.as_str()).collect();
    assert!(
        pick_texts.contains(&"Channel 15")
            && pick_texts.contains(&"Channel 14")
            && pick_texts.contains(&"Channel 16"),
        "Picker scroll must center around cursor 15 (showing neighbors 14 & 16), got: {pick_texts:?}"
    );
    assert!(
        !pick_texts.contains(&"Channel 00"),
        "Picker must have scrolled past item 00"
    );
}

#[test]
fn gpu_renders_signal_graph_filehub_settings_authoring_and_rack_out_faceplate() {
    let Ok(mut gpu) = HeadlessGpu::new(1440, 860) else {
        return;
    };

    let tmp_dir = common::TempTestDir::new("graph-assets-presets");

    let mut synth_patch = RackPatch::default_synth();
    for m in &mut synth_patch.zones[0].modules {
        if let ModuleKind::Filter(f) = &mut m.kind {
            f.cutoff_hz = 1850.0;
        }
    }
    let mut kit = RackPatch::default_synth();
    kit.kit_name = "Electro 808".into();
    kit.zones[0].name = "Kick808".into();
    kit.zones[0].low_midi = 36;
    kit.zones[0].high_midi = 36;
    kit.zones[0].root_midi = 36;
    let mut snare = RackPatch::default_synth().zones.remove(0);
    snare.id = 2;
    snare.name = "Snare808".into();
    snare.low_midi = 38;
    snare.high_midi = 38;
    snare.root_midi = 38;
    kit.zones.push(snare);
    InstrumentPreset::new("Acid Lead", Some("Lead".into()), synth_patch)
        .save(&tmp_dir.path().join("acid-lead.preset.json"))
        .unwrap();
    InstrumentPreset::new("Electro 808", Some("Kit".into()), kit)
        .save(&tmp_dir.path().join("electro-808.preset.json"))
        .unwrap();

    let io = Arc::new(MemoryHostIo::new("/virtual/studio"));
    io.set_presets(PresetCatalog::from_roots(&[tmp_dir.path().to_path_buf()]));
    io.insert_file("/virtual/studio/LiveJam.take.jsonl", b"{}\n");

    let mut app = App::new_with_host_io(
        Project::demo(),
        "/virtual/studio/session.json".into(),
        Arc::clone(&io) as Arc<dyn HostIo>,
    )
    .unwrap();
    app.reload_presets();

    let dark = Theme::cadence_dark();
    let status = "Metal · 48 kHz";
    let snap = AudioSnapshot {
        playing: true,
        absolute_step: 8,
        step_fraction: 0.4,
        sample_rate: 48_000,
        peak: 0.62,
        peak_left: 0.62,
        peak_right: 0.56,
        ..AudioSnapshot::default()
    };

    // Setup rich Signal Graph: Return, Filter FX + bypassed Delay FX, Send, Duck, Macro.
    let lead_ch = app.project().mixer.channels[0].id;
    app.apply(Action::CreateChannel {
        name: "VerbRet".into(),
        kind: ChannelKind::Return,
    })
    .unwrap();
    let ret_id = app
        .project()
        .mixer
        .channels
        .iter()
        .find(|c| c.name == "VerbRet")
        .unwrap()
        .id;
    app.apply(Action::SetChannelSend {
        channel: lead_ch,
        target: ret_id,
        amount: Some(0.42),
    })
    .unwrap();
    app.apply(Action::AddEffect {
        channel: ChannelRef::Channel(lead_ch),
        kind: "filter".into(),
    })
    .unwrap();
    app.apply(Action::AddEffect {
        channel: ChannelRef::Channel(lead_ch),
        kind: "delay".into(),
    })
    .unwrap();
    let delay_id = app.channel(lead_ch).unwrap().inserts.last().unwrap().id;
    app.apply(Action::SetEffectBypass {
        device: delay_id,
        bypassed: true,
    })
    .unwrap();
    let src_part = app.project().parts[0].name.clone();
    let dst_ch = app.project().mixer.channels[0].name.clone();
    app.execute_command(&format!("duck LeadPump {src_part} {dst_ch} 0.65 140"))
        .unwrap();
    app.execute_command("macro 1 name Sweep").unwrap();
    app.execute_command("macro 1 bind instrument Saw cutoff 0.15 0.90")
        .unwrap();

    // 1. Signal Graph (`gG`) zoomed showing indented FX cards, cables, and `selected_cables` pill.
    app.press_keys("gGzz").unwrap();
    let fv_tmp = app.frame_view(&snap, status);
    if let Some(g) = fv_tmp.decks.iter().find_map(|d| match &d.view {
        cadence_app::view::DeckView::Graph(g) => Some(g),
        _ => None,
    }) {
        for (c, col) in g.columns.iter().enumerate() {
            for (r, node) in col.nodes.iter().enumerate() {
                if node.target == GraphTarget::Channel(ChannelRef::Channel(lead_ch)) {
                    app.view_state.graph_col = c;
                    app.view_state.graph_row = r;
                }
            }
        }
    }
    let fv1 = app.frame_view(&snap, status);
    let px1 = gpu.render_rgba8(&fv1, &dark).unwrap();
    assert!(distinct_pixel_ratio(&px1, dark.surface.canvas) > 0.20);
    common::maybe_save_png(DUMP_ENV, "01-signal-graph-fx-cables.png", 1440, 860, &px1);
    app.press_keys("zz").unwrap();

    // 2. FileHub Import tab showing `PRESET LIBRARY` entries.
    app.open_file_hub(FileHubTab::Import);
    let fv2 = app.frame_view(&snap, status);
    let px2 = gpu.render_rgba8(&fv2, &dark).unwrap();
    assert!(distinct_pixel_ratio(&px2, dark.surface.canvas) > 0.20);
    common::maybe_save_png(DUMP_ENV, "02-file-hub-preset-library.png", 1440, 860, &px2);
    app.press_keys("<Esc>").unwrap();

    // 3. Settings modal (`g,`) showing configured `sample_dirs` / `preset_dirs` + inline editor.
    app.config.sample_dirs = vec!["/Users/studio/Samples/Drums".into()];
    app.config.preset_dirs = vec!["/Users/studio/Presets/Analog".into()];
    app.press_keys("g,").unwrap();
    app.settings_ui.cursor = 15; // Sample Directories row
    app.press_keys("a/Users/studio/Samples/Pads").unwrap();
    let fv3 = app.frame_view(&snap, status);
    let px3 = gpu.render_rgba8(&fv3, &dark).unwrap();
    assert!(distinct_pixel_ratio(&px3, dark.surface.canvas) > 0.20);
    common::maybe_save_png(DUMP_ENV, "03-settings-asset-dirs.png", 1440, 860, &px3);
    app.press_keys("<Esc><Esc>").unwrap();

    // 4. Deep nested `PlacedPart` Authoring draft at compact height (1440×460) showing left-pane scroll + `▲/▼`.
    if let Ok(mut compact_gpu) = HeadlessGpu::new(1440, 460) {
        // `n` -> `p` (Placed Part) -> accept Part=New -> Pattern=New -> Name -> PatternKind -> PatternName -> Instrument=New -> Sound -> ...
        app.press_keys("np<Enter><Enter><Enter><Enter><Enter><Enter><Enter>")
            .unwrap();
        let fv4 = app.frame_view(&snap, status);
        let px4 = compact_gpu.render_rgba8(&fv4, &dark).unwrap();
        assert!(distinct_pixel_ratio(&px4, dark.surface.canvas) > 0.20);
        common::maybe_save_png(
            DUMP_ENV,
            "04-authoring-left-pane-scroll.png",
            1440,
            460,
            &px4,
        );
        app.press_keys("<Esc>").unwrap();
    }

    // 5. 2-column control list (Quick Panel in standard docked height where multi-group 2-col spans midpoint).
    let saw_name = app.project().instruments[0].name.clone();
    app.execute_command(&format!("instrument {saw_name}"))
        .unwrap();
    app.rack_ui.tab = RackTab::Panel;
    app.rack_ui.panel_slot = 0;
    let fv5 = app.frame_view(&snap, status);
    let px5 = gpu.render_rgba8(&fv5, &dark).unwrap();
    assert!(distinct_pixel_ratio(&px5, dark.surface.canvas) > 0.20);
    common::maybe_save_png(DUMP_ENV, "05-object-2col-cont-header.png", 1440, 860, &px5);

    // 6. Rack `Out` module faceplate showing contiguous OUT / VOICE / RACK headers.
    let out_slot = app.project().instruments[0].rack.zones[0].modules.len();
    app.rack_ui.panel_slot = out_slot;
    let fv6 = app.frame_view(&snap, status);
    let px6 = gpu.render_rgba8(&fv6, &dark).unwrap();
    assert!(distinct_pixel_ratio(&px6, dark.surface.canvas) > 0.20);
    common::maybe_save_png(DUMP_ENV, "06-rack-out-voice-headers.png", 1440, 860, &px6);
}
