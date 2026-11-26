use super::draw_instrument;
use crate::display::{DisplayList, Rect};
use crate::theme::Theme;
use cadence_app::view::{
    InstrumentDeckView, InstrumentVisualView, ModuleVisualView, PatchbayCableKind,
    PatchbayCableView, PatchbayColumnView, PatchbayDeckView, PatchbayNodeView, RackDeckView,
    RackModulePillView, RackTab, RackZoneView, UiModeKind,
};
use cadence_core::rack::{InPort, ModuleKindTag};
use cadence_core::sampler::SampleLoopMode;

fn sample_wave() -> Vec<f32> {
    (0..64)
        .map(|i| ((i as f32 / 64.0) * std::f32::consts::TAU * 2.0).sin() * 0.65)
        .collect()
}

fn base_rack_view(tab: RackTab, visual: ModuleVisualView) -> InstrumentDeckView {
    let pb = PatchbayDeckView {
        columns: vec![
            PatchbayColumnView {
                title: "SOURCES",
                nodes: vec![PatchbayNodeView {
                    module_id: 1,
                    col: 0,
                    row: 0,
                    name: "VCO 1".into(),
                    badge: "VCO",
                    detail: "Saw · +0st".into(),
                    kind: ModuleKindTag::Vco,
                    activity: 0.78,
                    hot: false,
                    selected: true,
                    in_path: true,
                    incoming_ports: vec![InPort::PitchCv, InPort::FmIn],
                    has_output: true,
                }],
            },
            PatchbayColumnView {
                title: "MODULATORS",
                nodes: vec![],
            },
            PatchbayColumnView {
                title: "PROCESSORS",
                nodes: vec![PatchbayNodeView {
                    module_id: 2,
                    col: 2,
                    row: 0,
                    name: "Filter".into(),
                    badge: "FLT",
                    detail: "LP12 · 1.2kHz".into(),
                    kind: ModuleKindTag::Filter,
                    activity: 0.62,
                    hot: false,
                    selected: false,
                    in_path: true,
                    incoming_ports: vec![InPort::In, InPort::Cv],
                    has_output: true,
                }],
            },
            PatchbayColumnView {
                title: "VCA / OUT",
                nodes: vec![PatchbayNodeView {
                    module_id: 3,
                    col: 3,
                    row: 0,
                    name: "Out".into(),
                    badge: "OUT",
                    detail: "+0.0 dB · C".into(),
                    kind: ModuleKindTag::Out,
                    activity: 0.55,
                    hot: false,
                    selected: false,
                    in_path: true,
                    incoming_ports: vec![InPort::In],
                    has_output: false,
                }],
            },
        ],
        nodes: vec![],
        cables: vec![
            PatchbayCableView {
                cable_id: 1,
                from_module: 1,
                to_module: 2,
                to_port: InPort::In,
                from_col: 0,
                from_row: 0,
                to_col: 2,
                to_row: 0,
                to_port_y: 0.24,
                kind: PatchbayCableKind::Audio,
                amount: 1.0,
                activity: 0.74,
                label: "1.00".into(),
                feedback: false,
                selected: true,
                highlighted: true,
            },
            PatchbayCableView {
                cable_id: 2,
                from_module: 2,
                to_module: 1,
                to_port: InPort::FmIn,
                from_col: 2,
                from_row: 0,
                to_col: 0,
                to_row: 0,
                to_port_y: 0.76,
                kind: PatchbayCableKind::Fm,
                amount: 0.35,
                activity: 0.48,
                label: "35% z-1".into(),
                feedback: true,
                selected: false,
                highlighted: true,
            },
        ],
        selected_col: 0,
        selected_row: 0,
        selected_module: Some(1),
        selected_cable: Some(1),
        selected_summary: "CABLE #1 · VCO 1 → Filter.in · 1.00".into(),
        has_feedback: true,
        feedback_badge: "z-1",
        live_waveform: sample_wave(),
    };
    let zones = vec![
        RackZoneView {
            id: 1,
            name: "BD".into(),
            low_midi: 36,
            high_midi: 36,
            root_midi: 36,
            low_vel: 0.0,
            high_vel: 1.0,
            note_range: "C1".into(),
            vel_range: "FULL".into(),
            voice_summary: "Poly · Fixed".into(),
            sample_label: None,
            module_count: 6,
            cable_count: 7,
            has_sampler: false,
            source_tag: "SYNTH",
            choke_group: None,
            selected: true,
        },
        RackZoneView {
            id: 2,
            name: "CH".into(),
            low_midi: 42,
            high_midi: 42,
            root_midi: 42,
            low_vel: 0.2,
            high_vel: 1.0,
            note_range: "F#1".into(),
            vel_range: "20–100%".into(),
            voice_summary: "Poly · Fixed".into(),
            sample_label: Some("ch.wav".into()),
            module_count: 5,
            cable_count: 6,
            has_sampler: true,
            source_tag: "SAMPLER",
            choke_group: Some(1),
            selected: false,
        },
    ];
    InstrumentDeckView {
        id: 1,
        name: "Acid Rack".into(),
        channel_name: "Lead".into(),
        controls: vec![],
        control_cursor: 0,
        visual: InstrumentVisualView::Rack(Box::new(RackDeckView {
            kit_name: "Acid Rack".into(),
            active_tab: tab,
            tabs: vec![
                ("Panel", tab == RackTab::Panel),
                ("Patchbay", tab == RackTab::Patchbay),
                ("Zones", tab == RackTab::Zones),
            ],
            active_zone_id: 1,
            active_zone_name: "BD".into(),
            zone_count: 2,
            modules: vec![
                RackModulePillView {
                    module_id: None,
                    kind: None,
                    label: "Quick".into(),
                    tag: "PINS",
                    column: 0,
                    active: true,
                },
                RackModulePillView {
                    module_id: Some(1),
                    kind: Some(ModuleKindTag::Vco),
                    label: "VCO 1".into(),
                    tag: "VCO",
                    column: 0,
                    active: false,
                },
                RackModulePillView {
                    module_id: Some(2),
                    kind: Some(ModuleKindTag::Filter),
                    label: "Filter".into(),
                    tag: "FLT",
                    column: 2,
                    active: false,
                },
            ],
            visual,
            patchbay: pb,
            zones,
            selected_zone_summary: "ZONE 1/2 · BD · C1..C1 · 6m/7c".into(),
            live_waveform: sample_wave(),
        })),
    }
}

#[test]
fn all_twelve_module_visuals_and_three_tabs_emit_balanced_display_lists() {
    let theme = Theme::cadence_dark();
    let wave = sample_wave();
    let visuals = vec![
        ModuleVisualView::PinnedOverview {
            pin_count: 6,
            osc_label: Some("VCO 1 · Saw".into()),
            osc_curve: wave.clone(),
            filter_label: Some("Filter · LP12 · 1.2 kHz".into()),
            filter_curve: vec![0.0; 32],
            filter_cutoff_norm: 0.55,
            filter_mod_cutoff_norm: Some(0.75),
            env_label: Some("Amp Env · 4/180/60%/220".into()),
            env_curve: vec![(0.0, 0.0), (0.1, 1.0), (0.5, 0.6), (1.0, 0.0)],
            live_waveform: wave.clone(),
        },
        ModuleVisualView::Vco {
            module_id: 1,
            name: "VCO 1".into(),
            label: "VCO 1 · Saw · FM".into(),
            waveform_label: "Saw",
            curve: wave.clone(),
            fm_active: true,
            unison_voices: 3,
            active: true,
            live_waveform: wave.clone(),
        },
        ModuleVisualView::Sampler {
            module_id: 2,
            name: "Sampler".into(),
            sample_name: Some("kick.wav".into()),
            duration_sec: 0.42,
            peaks: vec![(-0.6, 0.6); 32],
            frames: 20160,
            sample_start: 0.1,
            sample_end: 0.9,
            loop_mode: SampleLoopMode::Forward,
            loop_start: 0.25,
            loop_end: 0.75,
            live_waveform: wave.clone(),
        },
        ModuleVisualView::Noise {
            module_id: 3,
            name: "Noise".into(),
            color_label: "Pink",
            highpass_hz: 200.0,
            lowpass_hz: 6000.0,
            band_curve: vec![-6.0; 32],
            highpass_norm: 0.3,
            lowpass_norm: 0.8,
            live_waveform: wave.clone(),
        },
        ModuleVisualView::Env {
            module_id: 4,
            name: "Clap Env".into(),
            label: "Clap Env · HoldDecay · 4x".into(),
            mode_label: "HoldDecay",
            curve: vec![(0.0, 0.0), (0.1, 1.0), (0.3, 0.2), (0.5, 1.0), (1.0, 0.0)],
            bursts: 4,
            live_waveform: wave.clone(),
        },
        ModuleVisualView::Lfo {
            module_id: 5,
            name: "LFO 1".into(),
            shape_label: "Sine",
            bipolar: true,
            sync: true,
            rate_label: "4.0 steps (2.00 Hz)".into(),
            curve: wave.clone(),
            live_waveform: wave.clone(),
        },
        ModuleVisualView::Filter {
            module_id: 6,
            name: "Filter".into(),
            label: "Filter · Acid · 840 Hz → 2.4 kHz".into(),
            mode_label: "Acid",
            curve: vec![0.0; 32],
            cutoff_norm: 0.45,
            mod_cutoff_norm: Some(0.72),
            mix: 1.0,
            live_waveform: wave.clone(),
        },
        ModuleVisualView::Vca {
            module_id: 7,
            name: "Ring VCA".into(),
            gain: 1.0,
            bias: 0.0,
            cv_patched: true,
            ring_mod: true,
            transfer_curve: wave.clone(),
            live_waveform: wave.clone(),
        },
        ModuleVisualView::Drive {
            module_id: 8,
            name: "Drive".into(),
            label: "Drive · Fold · drive 65% · mix 100%".into(),
            mode_label: "Fold",
            transfer_curve: wave.clone(),
            live_waveform: wave.clone(),
        },
        ModuleVisualView::LoFi {
            module_id: 9,
            name: "LoFi".into(),
            bits: 6.0,
            rate_hz: 8000.0,
            mix: 0.8,
            transfer_curve: wave.clone(),
            live_waveform: wave.clone(),
        },
        ModuleVisualView::Resonator {
            module_id: 11,
            name: "Plate Res".into(),
            mode_label: "Plate",
            base_hz: 535.0,
            decay_ms: 1350.0,
            brightness: 0.9,
            position: 0.65,
            mix: 1.0,
            modes: (0..16)
                .map(|i| (0.2 + i as f32 * 0.045, 0.35 + (i % 5) as f32 * 0.14, 0.8))
                .collect(),
            live_waveform: wave.clone(),
        },
        ModuleVisualView::Out {
            module_id: 10,
            name: "Out".into(),
            gain_db: 0.0,
            pan: -0.25,
            spread: 0.2,
            voice_summary: "Poly · Track · glide 15ms".into(),
            live_waveform: wave,
        },
    ];

    for vis in visuals {
        let mut dl = DisplayList::new(1200.0, 640.0, 1.0);
        let view = base_rack_view(RackTab::Panel, vis);
        draw_instrument(
            &mut dl,
            Rect::new(0.0, 0.0, 1200.0, 640.0),
            &view,
            true,
            UiModeKind::Normal,
            &theme,
        );
        assert!(!dl.base.quads.is_empty());
        assert!(!dl.base.curve_vertices.is_empty());
        assert_eq!(dl.current_clip(), dl.viewport, "clip stack must balance");
    }

    // Patchbay tab: verify ASCII "z-1" (never superscript minus U+207B) and cable geometry.
    let mut dl_pb = DisplayList::new(1200.0, 640.0, 1.0);
    let pb_view = base_rack_view(
        RackTab::Patchbay,
        ModuleVisualView::Out {
            module_id: 3,
            name: "Out".into(),
            gain_db: 0.0,
            pan: 0.0,
            spread: 0.0,
            voice_summary: String::new(),
            live_waveform: sample_wave(),
        },
    );
    draw_instrument(
        &mut dl_pb,
        Rect::new(0.0, 0.0, 1200.0, 640.0),
        &pb_view,
        true,
        UiModeKind::Normal,
        &theme,
    );
    assert!(!dl_pb.base.curve_vertices.is_empty());
    assert_eq!(dl_pb.current_clip(), dl_pb.viewport);
    assert!(
        dl_pb.base.texts.iter().any(|t| t.text.contains("z-1")),
        "Patchbay must emit ASCII z-1 feedback badge"
    );
    assert!(
        !dl_pb.base.texts.iter().any(|t| t.text.contains('\u{207B}')),
        "Patchbay must never emit U+207B superscript minus"
    );

    // Zones tab: verify horizontal zone pills, overview card & balanced clip stack.
    let mut dl_z = DisplayList::new(1200.0, 640.0, 1.0);
    let mut z_view = base_rack_view(
        RackTab::Zones,
        ModuleVisualView::Out {
            module_id: 3,
            name: "Out".into(),
            gain_db: 0.0,
            pan: 0.0,
            spread: 0.0,
            voice_summary: String::new(),
            live_waveform: sample_wave(),
        },
    );
    z_view.controls = vec![
        cadence_app::view::ControlView {
            module: "ZONE",
            label: "Root Note".into(),
            display: "C2 (36)".into(),
            normalized: Some(36.0 / 127.0),
            controlled: false,
            driver_pills: Vec::new(),
            pinned: true,
            selected: true,
            midi_badge: None,
            midi_learning: false,
        },
        cadence_app::view::ControlView {
            module: "VOICE",
            label: "Glide".into(),
            display: "0.0 ms".into(),
            normalized: Some(0.0),
            controlled: false,
            driver_pills: Vec::new(),
            pinned: false,
            selected: false,
            midi_badge: None,
            midi_learning: false,
        },
    ];
    draw_instrument(
        &mut dl_z,
        Rect::new(0.0, 0.0, 1200.0, 640.0),
        &z_view,
        true,
        UiModeKind::Normal,
        &theme,
    );
    assert!(dl_z.base.texts.iter().any(|t| t.text == "C1"));
    assert!(dl_z.base.texts.iter().any(|t| t.text == "F#1"));
    assert!(dl_z.base.texts.iter().any(|t| t.text == "BD"));
    assert!(dl_z.base.texts.iter().any(|t| t.text == "CH"));
    assert!(dl_z.base.texts.iter().any(|t| t.text.contains("6m/7c")));
    assert!(
        dl_z.base.texts.iter().any(|t| t.text == "✶"),
        "pinned controls must render crisp ✶ badge"
    );
    assert_eq!(dl_z.current_clip(), dl_z.viewport);

    // Narrow horizontal zone pill strip scrolls cleanly with ◀ / ▶ chevrons and balanced clips.
    let mut dl_scroll = DisplayList::new(420.0, 480.0, 1.0);
    let mut many_view = base_rack_view(
        RackTab::Zones,
        ModuleVisualView::Out {
            module_id: 3,
            name: "Out".into(),
            gain_db: 0.0,
            pan: 0.0,
            spread: 0.0,
            voice_summary: String::new(),
            live_waveform: sample_wave(),
        },
    );
    if let InstrumentVisualView::Rack(ref mut rack) = many_view.visual {
        rack.zones = (0..12)
            .map(|i| RackZoneView {
                id: (i + 1) as u16,
                name: format!("Pad {}", i + 1),
                low_midi: 36 + i as u8,
                high_midi: 36 + i as u8,
                root_midi: 36 + i as u8,
                low_vel: 0.0,
                high_vel: 1.0,
                note_range: format!("N{i}"),
                vel_range: "FULL".into(),
                voice_summary: "Poly · Fixed".into(),
                sample_label: None,
                module_count: 2,
                cable_count: 1,
                has_sampler: true,
                source_tag: "SAMPLER",
                choke_group: None,
                selected: i == 6,
            })
            .collect();
        rack.zone_count = 12;
    }
    draw_instrument(
        &mut dl_scroll,
        Rect::new(0.0, 0.0, 420.0, 480.0),
        &many_view,
        true,
        UiModeKind::Normal,
        &theme,
    );
    assert!(dl_scroll.base.texts.iter().any(|t| t.text == "◀"));
    assert!(dl_scroll.base.texts.iter().any(|t| t.text == "▶"));
    assert_eq!(dl_scroll.current_clip(), dl_scroll.viewport);
}

#[test]
fn zone_map_midi_window_frames_kits_splits_and_full_range() {
    use super::zones::zone_map_midi_window;

    let make_zone = |low: u8, high: u8, root: u8| RackZoneView {
        id: 1,
        name: "Z".into(),
        low_midi: low,
        high_midi: high,
        root_midi: root,
        low_vel: 0.0,
        high_vel: 1.0,
        note_range: "C2".into(),
        vel_range: "FULL".into(),
        voice_summary: "Poly · Chromatic".into(),
        sample_label: None,
        module_count: 2,
        cable_count: 1,
        has_sampler: true,
        source_tag: "SAMPLER",
        choke_group: None,
        selected: true,
    };

    assert_eq!(zone_map_midi_window(&[]), (0, 127));
    assert_eq!(zone_map_midi_window(&[make_zone(0, 127, 60)]), (0, 127));
    // Drum kit spanning C2 (36) .. Bb2 (46) frames cleanly onto 3 full octaves C1..B3 (24..59).
    let kit: Vec<RackZoneView> = (36..=46).map(|n| make_zone(n, n, n)).collect();
    assert_eq!(zone_map_midi_window(&kit), (24, 59));
    // Single mid-octave pad still gets at least 2 full octaves (24 semitones).
    let (lo, hi) = zone_map_midi_window(&[make_zone(41, 41, 41)]);
    assert_eq!(lo % 12, 0);
    assert!(hi - lo + 1 >= 24);
    assert!(lo <= 41 && 41 <= hi);
}

fn make_zone_view(
    id: u16,
    name: &str,
    midi: (u8, u8, u8),
    vel: (f32, f32),
    selected: bool,
) -> RackZoneView {
    let (low_midi, high_midi, root_midi) = midi;
    let (low_vel, high_vel) = vel;
    let note_range = if low_midi == high_midi {
        cadence_core::music::note_name(low_midi)
    } else {
        format!(
            "{}–{}",
            cadence_core::music::note_name(low_midi),
            cadence_core::music::note_name(high_midi)
        )
    };
    let vel_range = if low_vel <= 0.001 && high_vel >= 0.999 {
        "FULL".into()
    } else {
        format!("{:.0}–{:.0}%", low_vel * 100.0, high_vel * 100.0)
    };
    RackZoneView {
        id,
        name: name.into(),
        low_midi,
        high_midi,
        root_midi,
        low_vel,
        high_vel,
        note_range,
        vel_range,
        voice_summary: "Poly · Chromatic".into(),
        sample_label: None,
        module_count: 4,
        cable_count: 5,
        has_sampler: false,
        source_tag: "SYNTH",
        choke_group: None,
        selected,
    }
}

fn zones_view_with(zones: Vec<RackZoneView>) -> InstrumentDeckView {
    let mut view = base_rack_view(
        RackTab::Zones,
        ModuleVisualView::Out {
            module_id: 3,
            name: "Out".into(),
            gain_db: 0.0,
            pan: 0.0,
            spread: 0.0,
            voice_summary: String::new(),
            live_waveform: sample_wave(),
        },
    );
    if let InstrumentVisualView::Rack(ref mut rack) = view.visual {
        rack.zone_count = zones.len();
        rack.zones = zones;
    }
    view
}

#[test]
fn zone_velocity_layer_geometry_maps_vertical_bands_and_degenerate_clamps() {
    use super::zones::zone_block_rect;

    let grid = Rect::new(40.0, 20.0, 480.0, 100.0);
    let soft = make_zone_view(1, "Soft", (48, 72, 60), (0.0, 0.45), false);
    let hard = make_zone_view(2, "Hard", (48, 72, 60), (0.55, 1.0), true);
    let thin = make_zone_view(3, "Thin", (60, 60, 60), (0.50, 0.50), false);
    let out_of_view = make_zone_view(4, "High", (96, 108, 96), (0.0, 1.0), false);

    let r_soft = zone_block_rect(grid, &soft, 36, 83, 10.0).expect("soft in view");
    let r_hard = zone_block_rect(grid, &hard, 36, 83, 10.0).expect("hard in view");
    let r_thin = zone_block_rect(grid, &thin, 36, 83, 10.0).expect("thin in view");
    assert!(zone_block_rect(grid, &out_of_view, 36, 83, 10.0).is_none());

    // Higher velocity sits visually above lower velocity (smaller y), with exact proportional heights.
    assert!(
        r_hard.bottom() < r_soft.y,
        "high velocity band ({r_hard:?}) must sit above low velocity band ({r_soft:?})"
    );
    assert!((r_hard.y - grid.y).abs() < 0.01);
    assert!((r_soft.bottom() - grid.bottom()).abs() < 0.01);
    assert!((r_hard.h - 45.0).abs() < 0.5);
    assert!((r_soft.h - 45.0).abs() < 0.5);

    // Degenerate zero-height velocity layer clamps cleanly to minimum 4 px inside grid bounds.
    assert!(r_thin.h >= 4.0);
    assert!(r_thin.y >= grid.y && r_thin.bottom() <= grid.bottom());
}

#[test]
fn overlapping_zones_render_corner_pill_for_spans_and_centered_label_for_single_key_pad() {
    let theme = Theme::cadence_dark();
    let zones = vec![
        make_zone_view(1, "WarmPad", (36, 45, 40), (0.0, 0.60), false),
        make_zone_view(2, "SD", (42, 42, 42), (0.0, 1.0), false),
        make_zone_view(3, "LeadSplit", (38, 45, 42), (0.45, 1.0), true),
    ];
    let view = zones_view_with(zones);
    let mut dl = DisplayList::new(1200.0, 640.0, 1.0);
    draw_instrument(
        &mut dl,
        Rect::new(0.0, 0.0, 1200.0, 640.0),
        &view,
        true,
        UiModeKind::Normal,
        &theme,
    );
    assert_eq!(dl.current_clip(), dl.viewport);

    // Multi-key active velocity-cropped zone emits a top-left corner pill with name + vel_range.
    let active_pill = dl
        .base
        .texts
        .iter()
        .find(|t| t.text.contains("LeadSplit") && t.text.contains("45–100%"))
        .expect("multi-key velocity-cropped active zone must emit corner pill with vel range");
    assert_eq!(active_pill.align, crate::display::TextAlign::Center);

    // Sibling wide multi-key span emits its corner pill inside the map, while single-key drum pad
    // renders centered vertically across the full pad height and AFTER wider siblings in painter order.
    let pad_pos = dl
        .base
        .texts
        .iter()
        .rposition(|t| t.text == "SD" && t.align == crate::display::TextAlign::Center)
        .expect("single-key pad must emit centered block label");
    let wide_pos = dl
        .base
        .texts
        .iter()
        .rposition(|t| t.text == "WarmPad" && t.align == crate::display::TextAlign::Center)
        .expect("wide multi-key sibling must emit corner pill label");
    assert!(
        pad_pos > wide_pos,
        "narrow single-key pad ({pad_pos}) must paint after wide multi-key sibling ({wide_pos})"
    );
    let pad_text = &dl.base.texts[pad_pos];
    let wide_text = &dl.base.texts[wide_pos];
    assert!(
        pad_text.rect.h > wide_text.rect.h + 10.0,
        "single-key pad label spans pad height ({}) while multi-key corner pill is compact ({})",
        pad_text.rect.h,
        wide_text.rect.h
    );
}

#[test]
fn empty_zones_fallback_and_compact_card_height_balance_clips() {
    let theme = Theme::cadence_dark();

    // 1. Empty zones list emits clean fallback copy and balances clips.
    let empty_view = zones_view_with(Vec::new());
    let mut dl_empty = DisplayList::new(900.0, 480.0, 1.0);
    draw_instrument(
        &mut dl_empty,
        Rect::new(0.0, 0.0, 900.0, 480.0),
        &empty_view,
        true,
        UiModeKind::Normal,
        &theme,
    );
    assert_eq!(dl_empty.current_clip(), dl_empty.viewport);
    assert!(dl_empty.base.texts.iter().any(|t| t.text == "0 ZONES"));
    assert!(dl_empty
        .base
        .texts
        .iter()
        .any(|t| t.text.contains("No zones · press a to add a zone")));

    // 2. Compact viewport where overview card height < 120 px and body height < 120 px omits pill strip
    //    cleanly while rendering the zone map and balancing clips.
    let compact_view = zones_view_with(vec![make_zone_view(
        1,
        "Keys",
        (36, 72, 60),
        (0.0, 1.0),
        true,
    )]);
    let mut dl_compact = DisplayList::new(640.0, 175.0, 1.0);
    draw_instrument(
        &mut dl_compact,
        Rect::new(0.0, 0.0, 640.0, 175.0),
        &compact_view,
        true,
        UiModeKind::Normal,
        &theme,
    );
    assert_eq!(dl_compact.current_clip(), dl_compact.viewport);
    assert!(dl_compact
        .base
        .texts
        .iter()
        .any(|t| t.text.contains("Keys")));
    assert!(!dl_compact.base.quads.is_empty());
}

#[test]
fn zones_map_renders_warm_amber_root_column_and_adapts_piano_octave_stride() {
    let theme = Theme::cadence_dark();

    // Multi-key 2-octave zone at wide width -> warm amber root column wash + stride-1 octave labels (`C1`, `C2`, `C3`).
    let wide_view = zones_view_with(vec![make_zone_view(
        1,
        "Bass",
        (36, 59, 48),
        (0.0, 1.0),
        true,
    )]);
    let mut dl_wide = DisplayList::new(1200.0, 640.0, 1.0);
    draw_instrument(
        &mut dl_wide,
        Rect::new(0.0, 0.0, 1200.0, 640.0),
        &wide_view,
        true,
        UiModeKind::Normal,
        &theme,
    );
    assert_eq!(dl_wide.current_clip(), dl_wide.viewport);

    // Warm amber (`warning_key`) root wash quad appears inside the velocity grid (height > 10 px).
    let amber = theme.accent.warning_key.to_linear();
    assert!(
        dl_wide.base.quads.iter().any(|q| {
            (q.color_top[0] - amber[0]).abs() < 1e-4
                && (q.color_top[1] - amber[1]).abs() < 1e-4
                && (q.color_top[2] - amber[2]).abs() < 1e-4
                && q.rect[3] > 10.0
        }),
        "multi-key active zone must paint a warm amber root-column wash"
    );

    // Stride 1: adjacent octaves C1, C2, C3 all appear on the piano strip.
    let wide_labels: Vec<&str> = dl_wide.base.texts.iter().map(|t| t.text.as_str()).collect();
    assert!(
        wide_labels.contains(&"C1") && wide_labels.contains(&"C2") && wide_labels.contains(&"C3"),
        "wide zoomed zone map must label every octave (stride 1), got: {wide_labels:?}"
    );

    // Full-range 0..127 zone on narrow width (oct_px < 28 px) -> octave stride switches to 2 (even midi/12 only).
    let narrow_full = zones_view_with(vec![make_zone_view(
        1,
        "Full",
        (0, 127, 60),
        (0.0, 1.0),
        true,
    )]);
    let mut dl_narrow = DisplayList::new(340.0, 480.0, 1.0);
    draw_instrument(
        &mut dl_narrow,
        Rect::new(0.0, 0.0, 340.0, 480.0),
        &narrow_full,
        true,
        UiModeKind::Normal,
        &theme,
    );
    assert_eq!(dl_narrow.current_clip(), dl_narrow.viewport);
    let narrow_labels: Vec<&str> = dl_narrow
        .base
        .texts
        .iter()
        .map(|t| t.text.as_str())
        .collect();
    assert!(
        narrow_labels.contains(&"C1") && narrow_labels.contains(&"C3"),
        "narrow full-range piano strip must emit stride-2 octaves (C1, C3, ...), got: {narrow_labels:?}"
    );
    assert!(
        !narrow_labels.contains(&"C2") && !narrow_labels.contains(&"C4"),
        "narrow full-range piano strip must skip odd octaves when oct_px < 28, got: {narrow_labels:?}"
    );
}

#[test]
fn patchbay_live_activity_emits_node_shimmer_and_cable_glow() {
    let theme = Theme::cadence_dark();
    let area = Rect::new(0.0, 0.0, 960.0, 520.0);

    let active_view = base_rack_view(
        RackTab::Patchbay,
        ModuleVisualView::Out {
            module_id: 3,
            name: "Out".into(),
            gain_db: 0.0,
            pan: 0.0,
            spread: 0.25,
            voice_summary: "POLY".into(),
            live_waveform: sample_wave(),
        },
    );
    let mut silent_view = active_view.clone();
    if let InstrumentVisualView::Rack(ref mut rack) = silent_view.visual {
        for col in &mut rack.patchbay.columns {
            for node in &mut col.nodes {
                node.activity = 0.0;
                node.hot = false;
            }
        }
        for cable in &mut rack.patchbay.cables {
            cable.activity = 0.0;
        }
    }

    let mut dl_silent = DisplayList::new(area.w, area.h, 1.0);
    draw_instrument(
        &mut dl_silent,
        area,
        &silent_view,
        true,
        UiModeKind::Normal,
        &theme,
    );
    assert_eq!(dl_silent.current_clip(), dl_silent.viewport);

    let mut dl_active = DisplayList::new(area.w, area.h, 1.0);
    draw_instrument(
        &mut dl_active,
        area,
        &active_view,
        true,
        UiModeKind::Normal,
        &theme,
    );
    assert_eq!(dl_active.current_clip(), dl_active.viewport);

    // Active modules emit bottom shimmer quads (`theme.accent.playhead`) and active cables
    // emit extra outer energy halo curve vertices compared to the silent baseline.
    let active_quads = dl_active.base.quads.len();
    let silent_quads = dl_silent.base.quads.len();
    assert!(
        active_quads >= silent_quads + 3,
        "3 active Patchbay nodes must emit 3 shimmer quads (active={active_quads}, silent={silent_quads})"
    );
    let active_curves = dl_active.base.curve_vertices.len();
    let silent_curves = dl_silent.base.curve_vertices.len();
    assert!(
        active_curves > silent_curves,
        "live active highlighted cable must emit outer energy halo stroke (active={active_curves}, silent={silent_curves})"
    );
}
