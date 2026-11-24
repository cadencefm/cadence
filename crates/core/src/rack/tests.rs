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
    assert_eq!(FilterMode::parse("lp12"), Some(FilterMode::LowPass));

    for (i, &w) in Waveform::ALL.iter().enumerate() {
        assert_eq!(Waveform::from_index(i), w);
        assert_eq!(Waveform::parse(w.slug()), Some(w));
    }

    for (i, &rm) in ResonatorMode::ALL.iter().enumerate() {
        assert_eq!(rm.to_index(), i);
        assert_eq!(ResonatorMode::from_index(i), rm);
        let json = serde_json::to_string(&rm).unwrap();
        assert_eq!(json, format!("\"{}\"", rm.slug()));
        assert_eq!(serde_json::from_str::<ResonatorMode>(&json).unwrap(), rm);
        assert_eq!(ResonatorMode::parse(rm.slug()), Some(rm));
        assert_eq!(ResonatorMode::parse(rm.label()), Some(rm));
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
    for (i, &t) in TriggerMode::ALL.iter().enumerate() {
        assert_eq!(TriggerMode::from_index(i), t);
        assert_eq!(TriggerMode::parse(t.slug()), Some(t));
    }
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

    // Resonator accepts In / PitchCv / Cv, rejects FmIn, and caps at MAX_RESONATORS_PER_ZONE.
    let mut res_ok = RackPatch::default_synth();
    res_ok.zones[0].modules.push(RackModule::new(
        7,
        "Plate",
        ModuleKind::Resonator(ResonatorModule::default()),
    ));
    res_ok.zones[0]
        .cables
        .push(PatchCable::new(90, 1, 7, InPort::In, 1.0));
    res_ok.zones[0]
        .cables
        .push(PatchCable::new(91, 4, 7, InPort::Cv, 0.35));
    res_ok.zones[0]
        .cables
        .push(PatchCable::new(92, 4, 7, InPort::PitchCv, 12.0));
    assert!(res_ok.validate().is_ok());

    let mut bad_res_port = res_ok.clone();
    bad_res_port.zones[0]
        .cables
        .push(PatchCable::new(93, 1, 7, InPort::FmIn, 0.5));
    assert!(bad_res_port.validate().is_err());

    let mut too_many_res = RackPatch::default_synth();
    for k in 0..=MAX_RESONATORS_PER_ZONE {
        too_many_res.zones[0].modules.push(RackModule::new(
            (20 + k) as u16,
            format!("Res {k}"),
            ModuleKind::Resonator(ResonatorModule::default()),
        ));
    }
    assert!(too_many_res.validate().is_err());

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
