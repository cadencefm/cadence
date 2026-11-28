use super::*;
use crate::params::InstrumentParam;

#[test]
fn shared_enums_roundtrip_serde_slug_label_and_parse_aliases() {
    // FilterMode: serde JSON matches slug(), and both "low_pass_6" and "low_pass6" parse cleanly.
    for (i, &mode) in FilterMode::ALL.iter().enumerate() {
        assert_eq!(mode.to_index(), i);
        assert_eq!(FilterMode::from_index(i), mode);
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, format!("\"{}\"", mode.slug()));
        assert_eq!(serde_json::from_str::<FilterMode>(&json).unwrap(), mode);
        assert_eq!(FilterMode::parse(mode.slug()), Some(mode));
        assert_eq!(FilterMode::parse(mode.label()), Some(mode));
        assert_eq!(FilterMode::parse(&i.to_string()), Some(mode));
    }
    assert_eq!(FilterMode::parse("low_pass6"), Some(FilterMode::LowPass6));
    assert_eq!(FilterMode::parse("low_pass_6"), Some(FilterMode::LowPass6));
    assert_eq!(FilterMode::parse("lp12"), Some(FilterMode::ImLpf2));
    assert_eq!(FilterMode::parse("lp18"), Some(FilterMode::ImLpf3));
    assert_eq!(FilterMode::parse("lp24"), Some(FilterMode::ImLpf4));
    assert_eq!(FilterMode::parse("303"), Some(FilterMode::DiodeLadder));
    assert_eq!(FilterMode::parse("diode"), Some(FilterMode::DiodeLadder));
    assert_eq!(FilterMode::parse("bp"), Some(FilterMode::BandPass));
    assert_eq!(FilterMode::parse("hp"), Some(FilterMode::HighPass));

    for (i, &dm) in DriveMode::ALL.iter().enumerate() {
        assert_eq!(dm.to_index(), i);
        assert_eq!(DriveMode::from_index(i), dm);
        let json = serde_json::to_string(&dm).unwrap();
        assert_eq!(json, format!("\"{}\"", dm.slug()));
        assert_eq!(serde_json::from_str::<DriveMode>(&json).unwrap(), dm);
        assert_eq!(DriveMode::parse(dm.slug()), Some(dm));
        assert_eq!(DriveMode::parse(dm.label()), Some(dm));
    }

    assert_eq!(
        Waveform::ALL,
        &[
            Waveform::MobiusJet,
            Waveform::MobiusPwm,
            Waveform::BareCubic,
            Waveform::HardySaw,
            Waveform::Sine,
            Waveform::Szego,
            Waveform::Dipole,
            Waveform::Poisson,
        ]
    );
    for (i, &w) in Waveform::ALL.iter().enumerate() {
        assert_eq!(w.to_index(), i);
        assert_eq!(Waveform::from_index(i), w);
        let json = serde_json::to_string(&w).unwrap();
        assert_eq!(json, format!("\"{}\"", w.slug()));
        assert_eq!(serde_json::from_str::<Waveform>(&json).unwrap(), w);
        assert_eq!(Waveform::parse(w.slug()), Some(w));
        assert_eq!(Waveform::parse(w.label()), Some(w));
        assert_eq!(Waveform::parse(&i.to_string()), Some(w));
    }
    assert_eq!(Waveform::parse("saw"), Some(Waveform::MobiusJet));
    assert_eq!(Waveform::parse("pulse"), Some(Waveform::MobiusPwm));
    assert_eq!(Waveform::parse("tri"), Some(Waveform::BareCubic));
    assert_eq!(Waveform::parse("hardy"), Some(Waveform::HardySaw));
    assert_eq!(Waveform::parse("sin"), Some(Waveform::Sine));
    assert_eq!(Waveform::parse("szegő"), Some(Waveform::Szego));
    assert_eq!(Waveform::parse("clarinet"), Some(Waveform::Dipole));
    assert_eq!(Waveform::parse("formant"), Some(Waveform::Poisson));
    assert_eq!(
        Waveform::MobiusJet.to_cubic(),
        crate::dsp::cubic::CubicWaveform::MobiusJet
    );
    assert_eq!(
        Waveform::MobiusPwm.to_cubic(),
        crate::dsp::cubic::CubicWaveform::MobiusPwm
    );
    assert_eq!(
        Waveform::BareCubic.to_cubic(),
        crate::dsp::cubic::CubicWaveform::BareCubic
    );
    assert_eq!(
        Waveform::HardySaw.to_cubic(),
        crate::dsp::cubic::CubicWaveform::HardySaw
    );
    assert_eq!(
        Waveform::Sine.to_cubic(),
        crate::dsp::cubic::CubicWaveform::Sine
    );
    assert_eq!(
        Waveform::Szego.to_cubic(),
        crate::dsp::cubic::CubicWaveform::Szego
    );
    assert_eq!(
        Waveform::Dipole.to_cubic(),
        crate::dsp::cubic::CubicWaveform::Dipole
    );
    assert_eq!(
        Waveform::Poisson.to_cubic(),
        crate::dsp::cubic::CubicWaveform::Poisson
    );

    assert_eq!(AeroMode::ALL, &[AeroMode::FlueJet, AeroMode::BrassReed]);
    for (i, &am) in AeroMode::ALL.iter().enumerate() {
        assert_eq!(am.to_index(), i);
        assert_eq!(AeroMode::from_index(i), am);
        let json = serde_json::to_string(&am).unwrap();
        assert_eq!(json, format!("\"{}\"", am.slug()));
        assert_eq!(serde_json::from_str::<AeroMode>(&json).unwrap(), am);
        assert_eq!(AeroMode::parse(am.slug()), Some(am));
        assert_eq!(AeroMode::parse(am.label()), Some(am));
    }
    assert_eq!(AeroMode::parse("flue"), Some(AeroMode::FlueJet));
    assert_eq!(AeroMode::parse("pipe"), Some(AeroMode::FlueJet));
    assert_eq!(AeroMode::parse("reed"), Some(AeroMode::BrassReed));
    assert_eq!(AeroMode::parse("brass"), Some(AeroMode::BrassReed));

    assert_eq!(
        BodyGeometry::ALL,
        &[
            BodyGeometry::Harmonic,
            BodyGeometry::Stopped,
            BodyGeometry::Membrane,
            BodyGeometry::Bar,
            BodyGeometry::Plate,
            BodyGeometry::Cluster,
        ]
    );
    for (i, &bg) in BodyGeometry::ALL.iter().enumerate() {
        assert_eq!(bg.to_index(), i);
        assert_eq!(BodyGeometry::from_index(i), bg);
        let json = serde_json::to_string(&bg).unwrap();
        assert_eq!(json, format!("\"{}\"", bg.slug()));
        assert_eq!(serde_json::from_str::<BodyGeometry>(&json).unwrap(), bg);
        assert_eq!(BodyGeometry::parse(bg.slug()), Some(bg));
        assert_eq!(BodyGeometry::parse(bg.label()), Some(bg));
    }
    assert_eq!(BodyGeometry::parse("open"), Some(BodyGeometry::Harmonic));
    assert_eq!(BodyGeometry::parse("odd"), Some(BodyGeometry::Stopped));
    assert_eq!(BodyGeometry::parse("drum"), Some(BodyGeometry::Membrane));
    assert_eq!(BodyGeometry::parse("beam"), Some(BodyGeometry::Bar));
    assert_eq!(BodyGeometry::parse("cymbal"), Some(BodyGeometry::Plate));
    assert_eq!(BodyGeometry::parse("doublet"), Some(BodyGeometry::Cluster));
    assert_eq!(
        BodyGeometry::Harmonic.to_cubic(),
        crate::dsp::cubic::BodyGeometry::Harmonic
    );
    assert_eq!(
        BodyGeometry::Stopped.to_cubic(),
        crate::dsp::cubic::BodyGeometry::Stopped
    );
    assert_eq!(
        BodyGeometry::Membrane.to_cubic(),
        crate::dsp::cubic::BodyGeometry::Membrane
    );
    assert_eq!(
        BodyGeometry::Bar.to_cubic(),
        crate::dsp::cubic::BodyGeometry::Bar
    );
    assert_eq!(
        BodyGeometry::Plate.to_cubic(),
        crate::dsp::cubic::BodyGeometry::Plate
    );
    assert_eq!(
        BodyGeometry::Cluster.to_cubic(),
        crate::dsp::cubic::BodyGeometry::Cluster
    );

    assert_eq!(
        EnvMode::ALL,
        &[EnvMode::Adsr, EnvMode::HoldDecay, EnvMode::Capacitor]
    );
    for (i, &em) in EnvMode::ALL.iter().enumerate() {
        assert_eq!(em.to_index(), i);
        assert_eq!(EnvMode::from_index(i), em);
        let json = serde_json::to_string(&em).unwrap();
        assert_eq!(json, format!("\"{}\"", em.slug()));
        assert_eq!(serde_json::from_str::<EnvMode>(&json).unwrap(), em);
        assert_eq!(EnvMode::parse(em.slug()), Some(em));
        assert_eq!(EnvMode::parse(em.label()), Some(em));
    }
    assert_eq!(EnvMode::parse("cap"), Some(EnvMode::Capacitor));
    assert_eq!(EnvMode::parse("rc"), Some(EnvMode::Capacitor));
    assert_eq!(EnvMode::parse("c13"), Some(EnvMode::Capacitor));
    assert_eq!(EnvMode::parse("303"), Some(EnvMode::Capacitor));

    for (i, &pt) in PairTopology::ALL.iter().enumerate() {
        assert_eq!(pt.to_index(), i);
        assert_eq!(PairTopology::from_index(i), pt);
        let json = serde_json::to_string(&pt).unwrap();
        assert_eq!(json, format!("\"{}\"", pt.slug()));
        assert_eq!(serde_json::from_str::<PairTopology>(&json).unwrap(), pt);
        assert_eq!(PairTopology::parse(pt.slug()), Some(pt));
        assert_eq!(PairTopology::parse(pt.label()), Some(pt));
    }
    for (i, &ta) in TriadAxis::ALL.iter().enumerate() {
        assert_eq!(ta.to_index(), i);
        assert_eq!(TriadAxis::from_index(i), ta);
        let json = serde_json::to_string(&ta).unwrap();
        assert_eq!(json, format!("\"{}\"", ta.slug()));
        assert_eq!(serde_json::from_str::<TriadAxis>(&json).unwrap(), ta);
        assert_eq!(TriadAxis::parse(ta.slug()), Some(ta));
        assert_eq!(TriadAxis::parse(ta.label()), Some(ta));
    }
    for (i, &s) in LfoShape::ALL.iter().enumerate() {
        assert_eq!(LfoShape::from_index(i), s);
        assert_eq!(LfoShape::parse(s.slug()), Some(s));
    }
    for (i, &m) in VoiceMode::ALL.iter().enumerate() {
        assert_eq!(VoiceMode::from_index(i), m);
        assert_eq!(VoiceMode::parse(m.slug()), Some(m));
    }
    for (i, &p) in PitchTracking::ALL.iter().enumerate() {
        assert_eq!(PitchTracking::from_index(i), p);
        assert_eq!(PitchTracking::parse(p.slug()), Some(p));
    }
    assert_eq!(
        TriggerMode::ALL,
        &[
            TriggerMode::Gate,
            TriggerMode::OneShot,
            TriggerMode::Additive
        ]
    );
    for (i, &t) in TriggerMode::ALL.iter().enumerate() {
        assert_eq!(t.to_index(), i);
        assert_eq!(TriggerMode::from_index(i), t);
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(json, format!("\"{}\"", t.slug()));
        assert_eq!(serde_json::from_str::<TriggerMode>(&json).unwrap(), t);
        assert_eq!(TriggerMode::parse(t.slug()), Some(t));
        assert_eq!(TriggerMode::parse(t.label()), Some(t));
    }
    assert_eq!(TriggerMode::parse("add"), Some(TriggerMode::Additive));
    assert_eq!(TriggerMode::parse("stack"), Some(TriggerMode::Additive));
    assert_eq!(TriggerMode::parse("accent"), Some(TriggerMode::Additive));
    for (i, &n) in NoiseColor::ALL.iter().enumerate() {
        assert_eq!(NoiseColor::from_index(i), n);
        assert_eq!(NoiseColor::parse(n.slug()), Some(n));
    }
    for (i, &l) in SampleLoopMode::ALL.iter().enumerate() {
        assert_eq!(SampleLoopMode::from_index(i), l);
        assert_eq!(SampleLoopMode::parse(l.slug()), Some(l));
    }
    for &tag in ModuleKindTag::ALL {
        assert_eq!(ModuleKindTag::parse(tag.slug()), Some(tag));
        let json = serde_json::to_string(&tag).unwrap();
        assert_eq!(json, format!("\"{}\"", tag.slug()));
    }
}

#[test]
fn all_templates_and_demo_racks_validate_compile_and_roundtrip_json() {
    let racks = [
        ("default_synth", RackPatch::default_synth()),
        ("default_sampler", RackPatch::default_sampler()),
        ("default_kit", RackPatch::default_kit()),
        ("blank_modular", RackPatch::blank_modular()),
        ("acid_303", RackPatch::acid_303()),
        ("demo_lead", RackPatch::demo_lead()),
        ("demo_bass", RackPatch::demo_bass()),
        ("demo_kick", RackPatch::demo_kick()),
    ];
    for (name, rack) in racks {
        rack.validate()
            .unwrap_or_else(|e| panic!("{name} failed validate: {e:#}"));
        let json = serde_json::to_string_pretty(&rack).unwrap();
        let parsed: RackPatch = serde_json::from_str(&json)
            .unwrap_or_else(|e| panic!("{name} failed JSON roundtrip: {e:#}"));
        assert_eq!(parsed, rack, "{name} JSON roundtrip mismatch");
        for zone in &rack.zones {
            let graph = compile_zone_graph(zone)
                .unwrap_or_else(|e| panic!("{name} zone '{}' failed compile: {e:#}", zone.name));
            assert!(!graph.has_feedback(), "{name} should be acyclic");
            for c in graph.cables.iter() {
                assert!(
                    c.from_slot < c.to_slot,
                    "{name}: forward cable must satisfy from_slot < to_slot"
                );
            }
            assert_eq!(graph.column_layout.len(), graph.ops.len());
            for (slot, op) in graph.ops.iter().enumerate() {
                assert_eq!(graph.slot_for_module(op.module_id), Some(slot));
                assert_eq!(graph.column_layout[slot], op.kind.column());
            }
        }
    }

    // Check lifetime_envs specifically on default_synth (1 amp env) and demo_kick (2 parallel amp envs).
    let synth = RackPatch::default_synth();
    assert_eq!(
        synth
            .panel_pins
            .iter()
            .map(|p| p.label.as_str())
            .collect::<Vec<_>>(),
        vec![
            "Cutoff", "Res", "Env Mod", "Drive", "Attack", "Decay", "Sustain", "Release", "Pan",
            "Wave",
        ],
        "default_synth Quick Panel must put Cutoff first and continuous controls ahead of discrete Wave"
    );
    let g_synth = compile_zone_graph(&synth.zones[0]).unwrap();
    assert_eq!(g_synth.lifetime_envs.len(), 1);
    assert_eq!(
        g_synth.ops[usize::from(g_synth.lifetime_envs[0])].module_id,
        4,
        "only Amp Env (#4) gates default_synth lifetime, not Filter Env (#5)"
    );

    let kick = RackPatch::demo_kick();
    let g_kick = compile_zone_graph(&kick.zones[0]).unwrap();
    assert_eq!(
        g_kick.lifetime_envs.len(),
        2,
        "demo_kick has 2 parallel amp envelopes (Body Amp #5 + Click Env #8), not Pitch Env (#2)"
    );
}

#[test]
fn dag_compiler_handles_feedback_cycles_and_self_loops_deterministically() {
    let mut rack = RackPatch::default_synth();
    {
        let zone = &mut rack.zones[0];
        // Add Drive module (#7) and create a cycle: Filter (#2) -> Drive (#7) -> Filter (#2),
        // plus a self-FM loop: VCO 1 (#1) -> VCO 1 (#1) FmIn!
        zone.modules.push(RackModule::new(
            7,
            "Drive",
            ModuleKind::Drive(DriveModule::default()),
        ));
        zone.cables.push(PatchCable::new(10, 2, 7, InPort::In, 0.8));
        zone.cables
            .push(PatchCable::new(11, 7, 2, InPort::In, -0.35));
        zone.cables
            .push(PatchCable::new(12, 1, 1, InPort::FmIn, 0.25));
    }
    rack.validate().unwrap();

    let graph = compile_zone_graph(&rack.zones[0]).unwrap();
    assert!(graph.has_feedback());
    assert_eq!(graph.ops.len(), rack.zones[0].modules.len());

    // Self-loop cable #12 MUST be marked feedback == true (from_slot == to_slot).
    let self_idx = graph.cable_index_for_id(12).unwrap();
    assert!(graph.cables[self_idx].feedback);
    assert_eq!(
        graph.cables[self_idx].from_slot,
        graph.cables[self_idx].to_slot
    );

    // Exactly one of the Filter<->Drive pair (#10 or #11) must be marked feedback == true!
    let c10 = graph.cables[graph.cable_index_for_id(10).unwrap()];
    let c11 = graph.cables[graph.cable_index_for_id(11).unwrap()];
    assert_ne!(
        c10.feedback, c11.feedback,
        "exactly one edge in a 2-cycle must be a z^-1 feedback edge"
    );
    // Every compiled cable must uphold the invariant: feedback == (from_slot >= to_slot).
    for c in graph.cables.iter() {
        assert_eq!(c.feedback, c.from_slot >= c.to_slot);
    }
}

#[test]
fn validation_rejects_port_mismatches_missing_or_duplicate_out_and_bad_pins() {
    // 1. Missing Out module rejected.
    let mut no_out = RackPatch::blank_modular();
    no_out.zones[0].modules.clear();
    assert!(no_out.validate().is_err());

    // 2. Duplicate Out module rejected.
    let mut two_outs = RackPatch::blank_modular();
    two_outs.zones[0].modules.push(RackModule::new(
        2,
        "Out 2",
        ModuleKind::Out(OutModule::default()),
    ));
    assert!(two_outs.validate().is_err());

    // 3. Cable originating from Out module rejected.
    let mut from_out = RackPatch::default_synth();
    from_out.zones[0]
        .cables
        .push(PatchCable::new(99, 6, 2, InPort::In, 0.5));
    assert!(from_out.validate().is_err());

    // 4. Incompatible port connections rejected across module kinds.
    let mut bad_port = RackPatch::default_synth();
    // VCO rejects InPort::In
    bad_port.zones[0]
        .cables
        .push(PatchCable::new(99, 4, 1, InPort::In, 0.5));
    assert!(bad_port.validate().is_err());

    // LoFi rejects InPort::Cv
    let mut bad_lofi = RackPatch::default_synth();
    bad_lofi.zones[0].modules.push(RackModule::new(
        7,
        "LoFi",
        ModuleKind::LoFi(LoFiModule::default()),
    ));
    bad_lofi.zones[0]
        .cables
        .push(PatchCable::new(99, 4, 7, InPort::Cv, 0.5));
    assert!(bad_lofi.validate().is_err());

    // Modal accepts In / PitchCv / TrigIn / Cv, rejects FmIn, and caps at MAX_MODALS_PER_ZONE.
    let mut modal_ok = RackPatch::default_synth();
    modal_ok.zones[0].modules.push(RackModule::new(
        7,
        "Plate",
        ModuleKind::Modal(ModalModule::default()),
    ));
    modal_ok.zones[0]
        .cables
        .push(PatchCable::new(90, 1, 7, InPort::In, 1.0));
    modal_ok.zones[0]
        .cables
        .push(PatchCable::new(91, 4, 7, InPort::Cv, 0.35));
    modal_ok.zones[0]
        .cables
        .push(PatchCable::new(92, 4, 7, InPort::PitchCv, 12.0));
    modal_ok.zones[0]
        .cables
        .push(PatchCable::new(93, 4, 7, InPort::TrigIn, 1.0));
    assert!(modal_ok.validate().is_ok());

    let mut bad_modal_port = modal_ok.clone();
    bad_modal_port.zones[0]
        .cables
        .push(PatchCable::new(94, 1, 7, InPort::FmIn, 0.5));
    assert!(bad_modal_port.validate().is_err());

    let mut too_many_modals = RackPatch::default_synth();
    for k in 0..=MAX_MODALS_PER_ZONE {
        too_many_modals.zones[0].modules.push(RackModule::new(
            (20 + k) as u16,
            format!("Modal {k}"),
            ModuleKind::Modal(ModalModule::default()),
        ));
    }
    assert!(too_many_modals.validate().is_err());

    // Aero accepts PitchCv / TrigIn / Cv, rejects In / FmIn / PwCv, and caps at MAX_AEROS_PER_ZONE.
    let mut aero_ok = RackPatch::default_synth();
    aero_ok.zones[0].modules.push(RackModule::new(
        7,
        "Flue",
        ModuleKind::Aero(AeroModule::default()),
    ));
    aero_ok.zones[0]
        .cables
        .push(PatchCable::new(90, 4, 7, InPort::PitchCv, 12.0));
    aero_ok.zones[0]
        .cables
        .push(PatchCable::new(91, 4, 7, InPort::TrigIn, 1.0));
    aero_ok.zones[0]
        .cables
        .push(PatchCable::new(92, 5, 7, InPort::Cv, 0.5));
    assert!(aero_ok.validate().is_ok());

    for (cid, bad_port) in [
        (93u16, InPort::In),
        (94u16, InPort::FmIn),
        (95u16, InPort::PwCv),
    ] {
        let mut bad_aero_port = aero_ok.clone();
        bad_aero_port.zones[0]
            .cables
            .push(PatchCable::new(cid, 1, 7, bad_port, 0.25));
        assert!(
            bad_aero_port.validate().is_err(),
            "Aero must reject {bad_port:?}"
        );
    }

    let mut max_aeros = RackPatch::blank_modular();
    for k in 0..MAX_AEROS_PER_ZONE {
        max_aeros.zones[0].modules.push(RackModule::new(
            (20 + k) as u16,
            format!("Aero {k}"),
            ModuleKind::Aero(AeroModule::default()),
        ));
    }
    assert!(max_aeros.validate().is_ok());

    let mut too_many_aeros = max_aeros;
    too_many_aeros.zones[0].modules.push(RackModule::new(
        (20 + MAX_AEROS_PER_ZONE) as u16,
        "Aero Overflow",
        ModuleKind::Aero(AeroModule::default()),
    ));
    assert!(too_many_aeros.validate().is_err());

    // 5. Out-of-range cable amount & NaN rejected.
    let mut bad_amt = RackPatch::default_synth();
    bad_amt.zones[0].cables[0].amount = 5.0; // Audio In max is 2.0
    assert!(bad_amt.validate().is_err());
    bad_amt.zones[0].cables[0].amount = f32::NAN;
    assert!(bad_amt.validate().is_err());

    // 6. Invalid PanelPin (wrong param for module kind or missing module) rejected.
    let mut bad_pin = RackPatch::default_synth();
    bad_pin
        .panel_pins
        .push(PanelPin::module(1, 1, ModuleParam::Cutoff, "Wrong"));
    assert!(bad_pin.validate().is_err());
}

#[test]
fn all_module_params_roundtrip_normalize_format_and_nudge() {
    for (idx, &param) in ModuleParam::ALL.iter().enumerate() {
        assert_eq!(param.id(), idx);
        assert_eq!(ModuleParam::from_slot(idx), Some(param));
    }

    for &tag in ModuleKindTag::ALL {
        let mut kind = tag.default_kind();
        kind.validate().unwrap();
        let descs = kind.descriptors(48_000);
        assert_eq!(descs.len(), tag.params().len());

        for &param in tag.params() {
            let v = kind.read_native(param).unwrap();
            let n = kind.read_normalized(param, 48_000).unwrap();
            assert!(n.is_finite() && (0.0..=1.0).contains(&n));
            let formatted = kind.format_param(param, 48_000).unwrap();
            assert!(!formatted.is_empty());

            // Nudge up and down stays valid!
            let _ = kind.nudge_param(param, 1, false, 48_000).unwrap();
            let _ = kind.nudge_param(param, -1, false, 48_000).unwrap();
            kind.validate()
                .unwrap_or_else(|e| panic!("{tag:?}.{param:?} invalid after nudge: {e:#}"));

            // Restore native value cleanly.
            kind.write_native(param, v).unwrap();
        }
    }

    // Filter module Kerr & Spread parameter roundtrip, aliases, formatting, and nudging.
    let mut flt = ModuleKind::Filter(FilterModule::default());
    assert_eq!(
        ModuleParam::parse_for_kind("kerr", ModuleKindTag::Filter),
        Some(ModuleParam::Kerr)
    );
    assert_eq!(
        ModuleParam::parse_for_kind("bend", ModuleKindTag::Filter),
        Some(ModuleParam::Kerr)
    );
    assert_eq!(
        ModuleParam::parse_for_kind("spread", ModuleKindTag::Filter),
        Some(ModuleParam::Spread)
    );
    assert_eq!(
        ModuleParam::parse_for_kind("quad", ModuleKindTag::Filter),
        Some(ModuleParam::Spread)
    );
    let kerr_val = ModuleParam::Kerr.parse_value_native("-3.5 st").unwrap();
    assert!((kerr_val + 3.5).abs() < 1e-6);
    flt.write_native(ModuleParam::Kerr, kerr_val).unwrap();
    assert_eq!(flt.format_param(ModuleParam::Kerr, 0).unwrap(), "-3.5 st");
    let nudged_kerr = flt.nudge_param(ModuleParam::Kerr, 1, false, 0).unwrap();
    assert!((nudged_kerr + 3.0).abs() < 1e-5);
    assert!((flt.read_native(ModuleParam::Kerr).unwrap() + 3.0).abs() < 1e-5);

    let spread_val = ModuleParam::Spread.parse_value_native("65%").unwrap();
    assert!((spread_val - 0.65).abs() < 1e-6);
    flt.write_native(ModuleParam::Spread, spread_val).unwrap();
    assert_eq!(flt.format_param(ModuleParam::Spread, 0).unwrap(), "65%");
    let nudged_spread = flt.nudge_param(ModuleParam::Spread, 1, true, 0).unwrap();
    assert!((nudged_spread - 0.70).abs() < 1e-5);
    assert!((flt.read_native(ModuleParam::Spread).unwrap() - 0.70).abs() < 1e-5);
    flt.validate().unwrap();

    // Frame-based sampler write_normalized(LoopEnd) must not flip loop_mode.
    let mut smp = ModuleKind::Sampler(SamplerModule::default());
    assert!(smp.write_normalized(ModuleParam::LoopEnd, 0.5, 10_000));
    if let ModuleKind::Sampler(ref s) = smp {
        assert_eq!(s.loop_mode, SampleLoopMode::Off);
        assert_eq!(s.loop_end, 5_000);
    }
}

#[test]
fn module_sample_key_is_collision_free_and_multisample_auto_splits_roots() {
    let k1 = module_sample_key(1, 1, 1);
    let k2 = module_sample_key(1, 1, 2);
    let k3 = module_sample_key(1, 2, 1);
    let k4 = module_sample_key(2, 1, 1);
    assert_ne!(k1, k2);
    assert_ne!(k1, k3);
    assert_ne!(k1, k4);
    assert_ne!(k2, k3);
    assert!(k1 > (1u64 << 63));

    let entries = vec![
        (
            "Strings C3".into(),
            "strings_c3.wav".into(),
            crate::sampler::SampleMetadata::default(),
            48_000,
        ),
        (
            "Strings C4".into(),
            "strings_c4.wav".into(),
            crate::sampler::SampleMetadata::default(),
            48_000,
        ),
        (
            "Strings C5".into(),
            "strings_c5.wav".into(),
            crate::sampler::SampleMetadata::default(),
            48_000,
        ),
    ];
    let rack = RackPatch::from_multisample_entries(&entries).unwrap();
    assert_eq!(rack.zones.len(), 3);
    assert_eq!(rack.zones[0].low_midi, 0);
    assert_eq!(rack.zones[0].root_midi, 48);
    assert_eq!(rack.zones[1].root_midi, 60);
    assert_eq!(rack.zones[2].root_midi, 72);
    assert_eq!(rack.zones[2].high_midi, 127);
    assert_eq!(rack.zones[0].high_midi + 1, rack.zones[1].low_midi);
    assert_eq!(rack.zones[1].high_midi + 1, rack.zones[2].low_midi);
    assert_eq!(rack.sampler_modules().count(), 3);

    // Unified Rack parameter read/write/format for cable amounts.
    let mut acid = RackPatch::acid_303();
    let env_mod = InstrumentParam::Cable { zone: 1, cable: 5 };
    assert!((acid.read_param_native(env_mod).unwrap() - 3.2).abs() < 1e-5);
    acid.write_param_native(env_mod, -1.5).unwrap();
    assert_eq!(acid.format_param(env_mod, 0).unwrap(), "-1.50 oct");
}

#[test]
fn compiled_post_gate_tail_slots_track_frontier_env_gated_vcas() {
    let make_zone = |modules: Vec<RackModule>, cables: Vec<PatchCable>| -> RackZone {
        RackZone {
            id: 1,
            name: "GateTest".into(),
            low_midi: 0,
            high_midi: 127,
            root_midi: 60,
            low_vel: 0.0,
            high_vel: 1.0,
            gain_db: 0.0,
            pan: 0.0,
            choke_group: None,
            voice: VoiceSettings::default(),
            modules,
            cables,
        }
    };

    // 1. Out.Cv strictly driven by lifetime Env: env_gated_vcas is empty and only Out is post-gate,
    //    even if an upstream VCA exists in series.
    let z_out_cv = make_zone(
        vec![
            RackModule::new(1, "Vco", ModuleKind::Vco(VcoModule::default())),
            RackModule::new(2, "UpVca", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(3, "Env", ModuleKind::Env(EnvModule::default())),
            RackModule::new(4, "Out", ModuleKind::Out(OutModule::default())),
        ],
        vec![
            PatchCable::new(1, 1, 2, InPort::In, 1.0),
            PatchCable::new(2, 3, 2, InPort::Cv, 1.0),
            PatchCable::new(3, 2, 4, InPort::In, 1.0),
            PatchCable::new(4, 3, 4, InPort::Cv, 1.0),
        ],
    );
    let g_out = compile_zone_graph(&z_out_cv).unwrap();
    assert!(g_out.env_gated_vcas.is_empty());
    assert_eq!(g_out.post_gate_tail_slots.as_ref(), &[g_out.out_slot]);

    // 2. Chained VCAs (Vca1 -> Vca2 -> Out): only frontier Vca2 is recorded in env_gated_vcas.
    let z_chain = make_zone(
        vec![
            RackModule::new(1, "Vco", ModuleKind::Vco(VcoModule::default())),
            RackModule::new(2, "Vca1", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(3, "Vca2", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(4, "Env", ModuleKind::Env(EnvModule::default())),
            RackModule::new(5, "Out", ModuleKind::Out(OutModule::default())),
        ],
        vec![
            PatchCable::new(1, 1, 2, InPort::In, 1.0),
            PatchCable::new(2, 4, 2, InPort::Cv, 1.0),
            PatchCable::new(3, 2, 3, InPort::In, 1.0),
            PatchCable::new(4, 4, 3, InPort::Cv, 1.0),
            PatchCable::new(5, 3, 5, InPort::In, 1.0),
        ],
    );
    let g_chain = compile_zone_graph(&z_chain).unwrap();
    let vca2_slot = g_chain.slot_for_module(3).unwrap() as u8;
    assert_eq!(g_chain.env_gated_vcas.as_ref(), &[vca2_slot]);
    assert_eq!(g_chain.post_gate_tail_slots.as_ref(), &[g_chain.out_slot]);

    // 3. Diamond with ungated parallel branch retains shared upstream Modal in post_gate_tail_slots.
    let z_diamond = make_zone(
        vec![
            RackModule::new(1, "Vco", ModuleKind::Vco(VcoModule::default())),
            RackModule::new(2, "Modal", ModuleKind::Modal(ModalModule::default())),
            RackModule::new(3, "Vca", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(4, "Flt", ModuleKind::Filter(FilterModule::default())),
            RackModule::new(5, "Env", ModuleKind::Env(EnvModule::default())),
            RackModule::new(6, "Out", ModuleKind::Out(OutModule::default())),
        ],
        vec![
            PatchCable::new(1, 1, 2, InPort::In, 1.0),
            PatchCable::new(2, 2, 3, InPort::In, 1.0),
            PatchCable::new(3, 5, 3, InPort::Cv, 1.0),
            PatchCable::new(4, 3, 6, InPort::In, 1.0),
            PatchCable::new(5, 2, 4, InPort::In, 1.0),
            PatchCable::new(6, 4, 6, InPort::In, 1.0),
        ],
    );
    let g_dia = compile_zone_graph(&z_diamond).unwrap();
    let modal_slot = g_dia.slot_for_module(2).unwrap() as u8;
    let flt_slot = g_dia.slot_for_module(4).unwrap() as u8;
    assert_eq!(g_dia.modal_slots.as_ref(), &[modal_slot]);
    assert!(g_dia.post_gate_tail_slots.contains(&modal_slot));
    assert!(g_dia.post_gate_tail_slots.contains(&flt_slot));

    // 4. VCA driven by both Env and LFO on Cv is NOT strictly env-gated.
    let z_mixed = make_zone(
        vec![
            RackModule::new(1, "Vco", ModuleKind::Vco(VcoModule::default())),
            RackModule::new(2, "Modal", ModuleKind::Modal(ModalModule::default())),
            RackModule::new(3, "Vca", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(4, "Env", ModuleKind::Env(EnvModule::default())),
            RackModule::new(5, "Lfo", ModuleKind::Lfo(LfoModule::default())),
            RackModule::new(6, "Out", ModuleKind::Out(OutModule::default())),
        ],
        vec![
            PatchCable::new(1, 1, 2, InPort::In, 1.0),
            PatchCable::new(2, 2, 3, InPort::In, 1.0),
            PatchCable::new(3, 4, 3, InPort::Cv, 1.0),
            PatchCable::new(4, 5, 3, InPort::Cv, 0.5),
            PatchCable::new(5, 3, 6, InPort::In, 1.0),
        ],
    );
    let g_mixed = compile_zone_graph(&z_mixed).unwrap();
    let modal_slot_m = g_mixed.slot_for_module(2).unwrap() as u8;
    assert!(g_mixed.env_gated_vcas.is_empty());
    assert!(g_mixed.post_gate_tail_slots.contains(&modal_slot_m));
}

#[test]
fn aero_vco_and_env_cubic_params_units_ports_and_compilation() {
    // 1. ModuleKindTag::Aero column, ports, and 17 faceplate parameter round-trips.
    assert_eq!(ModuleKindTag::Aero.column(), 0);
    assert!(ModuleKindTag::Aero.has_output());
    assert_eq!(
        ModuleKindTag::Aero.in_ports(),
        &[InPort::PitchCv, InPort::TrigIn, InPort::Cv]
    );
    assert!(ModuleKindTag::Aero.accepts_in_port(InPort::PitchCv));
    assert!(ModuleKindTag::Aero.accepts_in_port(InPort::TrigIn));
    assert!(ModuleKindTag::Aero.accepts_in_port(InPort::Cv));
    assert!(!ModuleKindTag::Aero.accepts_in_port(InPort::In));
    assert!(!ModuleKindTag::Aero.accepts_in_port(InPort::FmIn));
    assert!(!ModuleKindTag::Aero.accepts_in_port(InPort::PwCv));
    assert_eq!(ModuleKindTag::Aero.params().len(), 17);

    let aero_cases: [(ModuleParam, &str, &str, f32, &str); 17] = [
        (
            ModuleParam::AeroMode,
            "mode",
            "brass_reed",
            1.0,
            "Brass Reed",
        ),
        (ModuleParam::Geometry, "bore", "plate", 4.0, "Plate"),
        (ModuleParam::Atoms, "modes", "24", 24.0, "24"),
        (ModuleParam::VcoPitchTrack, "track", "fixed", 1.0, "Fixed"),
        (ModuleParam::Tune, "pitch", "-12 st", -12.0, "-12.0 st"),
        (ModuleParam::Fine, "cents", "+15 ct", 15.0, "+15 ct"),
        (ModuleParam::FixedHz, "freq", "2.2 kHz", 2200.0, "2.20 kHz"),
        (ModuleParam::Drive, "blow", "80%", 0.80, "80%"),
        (ModuleParam::Embouchure, "lip", "65%", 0.65, "65%"),
        (
            ModuleParam::BeatPressure,
            "beat_pa",
            "2.4 kPa",
            2400.0,
            "2400 Pa",
        ),
        (ModuleParam::Brightness, "tone", "75%", 0.75, "75%"),
        (ModuleParam::Absorption, "damping", "0.125", 0.125, "0.125"),
        (ModuleParam::Radiation, "air", "40%", 0.40, "40%"),
        (ModuleParam::Kerr, "bend", "+2.5 st", 2.5, "+2.5 st"),
        (ModuleParam::TriadAmount, "triads", "35%", 0.35, "35%"),
        (ModuleParam::Spread, "quad", "60%", 0.60, "60%"),
        (ModuleParam::Level, "amp", "90%", 0.90, "90%"),
    ];
    let mut aero = ModuleKind::Aero(AeroModule::default());
    for (param, alias, raw_val, expected_native, expected_fmt) in aero_cases {
        assert_eq!(
            ModuleParam::parse_for_kind(alias, ModuleKindTag::Aero),
            Some(param),
            "Aero alias '{alias}' must resolve to {param:?}"
        );
        assert_eq!(
            ModuleParam::parse_for_kind(param.name(), ModuleKindTag::Aero),
            Some(param)
        );
        let parsed = param.parse_value_native(raw_val).unwrap();
        assert!(
            (parsed - expected_native).abs() < 1e-4,
            "Aero {param:?} parsed '{raw_val}' as {parsed}, expected {expected_native}"
        );
        aero.write_native(param, parsed).unwrap();
        assert!(
            (aero.read_native(param).unwrap() - expected_native).abs() < 1e-4,
            "Aero {param:?} readback mismatch"
        );
        assert_eq!(aero.format_param(param, 0).unwrap(), expected_fmt);
    }
    aero.validate().unwrap();

    // 2. Unit::Pascals (`BeatPressure`) parsing, formatting, log nudge, and range/unit guards.
    assert_eq!(ModuleParam::BeatPressure.unit(), Unit::Pascals);
    assert!(
        (ModuleParam::BeatPressure
            .parse_value_native("1500 Pa")
            .unwrap()
            - 1500.0)
            .abs()
            < 1e-4
    );
    assert!(
        (ModuleParam::BeatPressure
            .parse_value_native("2.5 kPa")
            .unwrap()
            - 2500.0)
            .abs()
            < 1e-4
    );
    assert!((ModuleParam::BeatPressure.parse_value_native("800").unwrap() - 800.0).abs() < 1e-4);
    assert!(ModuleParam::BeatPressure
        .parse_value_native("500 Hz")
        .is_err());
    assert!(ModuleParam::BeatPressure
        .parse_value_native("100 Pa")
        .is_err());
    assert!(ModuleParam::BeatPressure
        .parse_value_native("9000 Pa")
        .is_err());
    let beat_nudged = aero
        .nudge_param(ModuleParam::BeatPressure, 1, false, 0)
        .unwrap();
    assert!(beat_nudged > 2400.0 && beat_nudged <= 8000.0);

    // 3. Unit::Decimal (`Absorption` on Aero, `CubicLimit` on Env) formatting, parsing, nudging.
    assert_eq!(ModuleParam::Absorption.unit(), Unit::Decimal);
    assert!(ModuleParam::Absorption.parse_value_native("50%").is_err());
    aero.write_native(ModuleParam::Absorption, 0.045).unwrap();
    assert_eq!(
        aero.format_param(ModuleParam::Absorption, 0).unwrap(),
        "0.045"
    );
    let abs_fine = aero
        .nudge_param(ModuleParam::Absorption, 1, false, 0)
        .unwrap();
    assert!((abs_fine - 0.050).abs() < 1e-5);
    assert_eq!(
        aero.format_param(ModuleParam::Absorption, 0).unwrap(),
        "0.050"
    );

    assert_eq!(ModuleParam::CubicLimit.unit(), Unit::Decimal);
    assert!(ModuleParam::CubicLimit.parse_value_native("50%").is_err());
    let mut env = ModuleKind::Env(EnvModule::default());
    assert_eq!(
        ModuleParam::parse_for_kind("limit", ModuleKindTag::Env),
        Some(ModuleParam::CubicLimit)
    );
    assert_eq!(
        ModuleParam::parse_for_kind("g_cubic", ModuleKindTag::Env),
        Some(ModuleParam::CubicLimit)
    );
    let lim = ModuleParam::CubicLimit.parse_value_native("2.5").unwrap();
    env.write_native(ModuleParam::CubicLimit, lim).unwrap();
    assert_eq!(
        env.format_param(ModuleParam::CubicLimit, 0).unwrap(),
        "2.50"
    );
    let lim_fine = env
        .nudge_param(ModuleParam::CubicLimit, 1, false, 0)
        .unwrap();
    assert!((lim_fine - 2.55).abs() < 1e-5);
    let lim_coarse = env
        .nudge_param(ModuleParam::CubicLimit, 1, true, 0)
        .unwrap();
    assert!((lim_coarse - 3.05).abs() < 1e-5);
    env.write_native(ModuleParam::EnvMode, EnvMode::Capacitor.to_index() as f32)
        .unwrap();
    env.write_native(
        ModuleParam::TriggerMode,
        TriggerMode::Additive.to_index() as f32,
    )
    .unwrap();
    env.validate().unwrap();
    if let ModuleKind::Env(ref e) = env {
        let dsp = e.to_dsp_params();
        assert_eq!(dsp.mode, EnvMode::Capacitor);
        assert_eq!(dsp.trigger_mode, TriggerMode::Additive);
        assert!((dsp.cubic_limit - 3.05).abs() < 1e-5);
    } else {
        panic!("expected Env module");
    }

    // 4. Unit::BipolarPercent (`Asymmetry` on Drive, `Chirality` on Vco) and VCO Reach/Shape.
    assert_eq!(ModuleParam::Asymmetry.unit(), Unit::BipolarPercent);
    let mut drv = ModuleKind::Drive(DriveModule::default());
    let asym = ModuleParam::Asymmetry.parse_value_native("-35%").unwrap();
    assert!((asym + 0.35).abs() < 1e-6);
    drv.write_native(ModuleParam::Asymmetry, asym).unwrap();
    assert_eq!(drv.format_param(ModuleParam::Asymmetry, 0).unwrap(), "-35%");
    assert!(ModuleParam::Asymmetry.parse_value_native("50 Hz").is_err());

    assert_eq!(ModuleParam::Chirality.unit(), Unit::BipolarPercent);
    let mut vco = ModuleKind::Vco(VcoModule::default());
    assert_eq!(
        ModuleParam::parse_for_kind("rho", ModuleKindTag::Vco),
        Some(ModuleParam::Reach)
    );
    assert_eq!(
        ModuleParam::parse_for_kind("curvature", ModuleKindTag::Vco),
        Some(ModuleParam::Shape)
    );
    assert_eq!(
        ModuleParam::parse_for_kind("chi", ModuleKindTag::Vco),
        Some(ModuleParam::Chirality)
    );
    let reach = ModuleParam::Reach.parse_value_native("88%").unwrap();
    vco.write_native(ModuleParam::Reach, reach).unwrap();
    assert_eq!(vco.format_param(ModuleParam::Reach, 0).unwrap(), "88%");
    let shape = ModuleParam::Shape.parse_value_native("30%").unwrap();
    vco.write_native(ModuleParam::Shape, shape).unwrap();
    assert_eq!(vco.format_param(ModuleParam::Shape, 0).unwrap(), "30%");
    let chi = ModuleParam::Chirality.parse_value_native("+60%").unwrap();
    vco.write_native(ModuleParam::Chirality, chi).unwrap();
    assert_eq!(vco.format_param(ModuleParam::Chirality, 0).unwrap(), "+60%");
    let chi_neg = ModuleParam::Chirality.parse_value_native("-40%").unwrap();
    vco.write_native(ModuleParam::Chirality, chi_neg).unwrap();
    assert_eq!(vco.format_param(ModuleParam::Chirality, 0).unwrap(), "-40%");
    let chi_nudged = vco.nudge_param(ModuleParam::Chirality, 1, true, 0).unwrap();
    assert!((chi_nudged + 0.35).abs() < 1e-5);
    assert_eq!(vco.format_param(ModuleParam::Chirality, 0).unwrap(), "-35%");
    vco.validate().unwrap();

    // 5. Compiling and JSON round-tripping a full RackPatch containing an Aero source.
    let mut rack = RackPatch::blank_modular();
    rack.zones[0].modules.push(RackModule::new(2, "Aero", aero));
    rack.zones[0]
        .cables
        .push(PatchCable::new(1, 2, 1, InPort::In, 1.0));
    rack.validate().unwrap();
    let json = serde_json::to_string_pretty(&rack).unwrap();
    let decoded: RackPatch = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, rack);
    let graph = compile_zone_graph(&rack.zones[0]).unwrap();
    let aero_slot = graph.slot_for_module(2).unwrap();
    assert_eq!(graph.ops[aero_slot].kind, ModuleKindTag::Aero);
    assert_eq!(graph.column_layout[aero_slot], 0);
}
