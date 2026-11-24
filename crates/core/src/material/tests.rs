use super::*;
use crate::evaluate::evaluate_notes_at;
use crate::mixer::{EffectDevice, EffectKind};
use crate::music::{self, ChordTone, EndRule, Step};
use crate::params::{InstrumentParam, ValueMode};
use crate::rack::{
    InPort, LoFiModule, ModuleKind, ModuleParam, PanelPin, PatchCable, RackModule, RackPatch,
    RackZone, ZoneParam,
};

fn note_step(degree: i16, gate: f32) -> Step {
    Step {
        active: true,
        degree,
        velocity: 0.8,
        gate,
        ..Step::default()
    }
}

#[test]
fn evaluate_memoized_temporal_dag_keys_by_id_and_time() {
    let mut p = Project::empty();
    // Source rhythm: [true, false, false, false] (active ONLY on step 0 mod 4)
    p.patterns.push(Pattern {
        id: 1,
        name: "Pulse".into(),
        kind: PatternKind::Rhythm {
            steps: vec![true, false, false, false],
        },
    });
    // Shifted by +1 -> active ONLY on step 1 mod 4
    p.patterns.push(Pattern {
        id: 2,
        name: "ShiftedPulse".into(),
        kind: PatternKind::Shift {
            source: 1,
            steps: 1,
        },
    });
    // Stack of Pulse (active @ 0) and ShiftedPulse (active @ 1) sharing source 1 in one DAG!
    p.patterns.push(Pattern {
        id: 3,
        name: "Combined".into(),
        kind: PatternKind::Stack { parts: vec![1, 2] },
    });
    // Notes pattern + Mask sharing shifted source
    p.patterns.push(Pattern {
        id: 4,
        name: "Notes".into(),
        kind: PatternKind::Notes {
            steps: vec![
                note_step(1, 0.5),
                Step::default(),
                note_step(3, 0.5),
                Step::default(),
            ],
        },
    });
    p.patterns.push(Pattern {
        id: 5,
        name: "ShiftedNotes".into(),
        kind: PatternKind::Shift {
            source: 4,
            steps: 2,
        },
    });
    // Mask: ShiftedNotes (active at 0 & 2, degree 3 at 0) masked by source 4 (active at 0 & 2)!
    p.patterns.push(Pattern {
        id: 6,
        name: "MaskedShared".into(),
        kind: PatternKind::Mask {
            notes: 5,
            rhythm: 4,
        },
    });

    assert_eq!(
        p.evaluate_pattern(3, 0).unwrap(),
        PatternValue::Rhythm(true)
    );
    assert_eq!(
        p.evaluate_pattern(3, 1).unwrap(),
        PatternValue::Rhythm(true),
        "Shift(source, 1) must NOT see memoized (source, 1)=false from sibling branch"
    );
    assert_eq!(
        p.evaluate_pattern(3, 2).unwrap(),
        PatternValue::Rhythm(false)
    );

    let masked0 = p.evaluate_pattern(6, 0).unwrap();
    let PatternValue::Notes(s0) = masked0 else {
        panic!("expected Notes");
    };
    assert!(s0.active);
    assert_eq!(
        s0.degree, 3,
        "ShiftedNotes at t=0 reads source(t=-2)=deg 3 while Mask rhythm reads source(t=0)=deg 1"
    );
}

#[test]
fn timescale_on_rhythm_uses_onset_interval_for_double_and_half_time() {
    let mut p = Project::empty();
    p.patterns.push(Pattern {
        id: 1,
        name: "OffHit".into(),
        kind: PatternKind::Rhythm {
            steps: vec![false, true],
        },
    });
    p.patterns.push(Pattern {
        id: 2,
        name: "FastOff".into(),
        kind: PatternKind::TimeScale {
            source: 1,
            num: 2,
            den: 1,
        },
    });
    // Double-time [false, true]: interval [0, 2) contains source step 1 (true) -> NOT skipped!
    assert_eq!(
        p.evaluate_pattern(2, 0).unwrap(),
        PatternValue::Rhythm(true)
    );
    assert_eq!(
        p.evaluate_pattern(2, 1).unwrap(),
        PatternValue::Rhythm(true)
    );

    // Half-time [true, false]: only fires at even steps 0, 4, 8 (NOT double-triggered at 1!).
    p.patterns.push(Pattern {
        id: 3,
        name: "OnHit".into(),
        kind: PatternKind::Rhythm {
            steps: vec![true, false],
        },
    });
    p.patterns.push(Pattern {
        id: 4,
        name: "SlowOn".into(),
        kind: PatternKind::TimeScale {
            source: 3,
            num: 1,
            den: 2,
        },
    });
    let slow_hits: Vec<bool> = (0..8)
        .map(|t| p.evaluate_pattern(4, t).unwrap().active())
        .collect();
    assert_eq!(
        slow_hits,
        vec![true, false, false, false, true, false, false, false]
    );

    // NotesFromValues over double-time Rhythm emits both 32nd-note sub-step hits.
    p.patterns.push(Pattern {
        id: 5,
        name: "AllTrue".into(),
        kind: PatternKind::Rhythm { steps: vec![true] },
    });
    p.patterns.push(Pattern {
        id: 6,
        name: "DoubleTrue".into(),
        kind: PatternKind::TimeScale {
            source: 5,
            num: 2,
            den: 1,
        },
    });
    p.patterns.push(Pattern {
        id: 7,
        name: "DegVal".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Degrees,
            period: 4,
            mode: ValueMode::Hold,
            points: vec![ValuePoint {
                step: 0,
                value: 0.0,
            }],
        },
    });
    p.patterns.push(Pattern {
        id: 8,
        name: "DoubleNFV".into(),
        kind: PatternKind::NotesFromValues {
            values: 7,
            rhythm: 6,
            degree_offset: 0,
            velocity: 0.8,
            gate: 0.5,
        },
    });
    let evs = evaluate_notes_at(&p, 8, 0).unwrap();
    assert_eq!(evs.len(), 2);
    assert!(evs[0].offset.abs() < 1e-4);
    assert!((evs[1].offset - 0.5).abs() < 1e-4);
}

#[test]
fn swing_and_ratchet_carry_overflowing_substeps_across_step_boundaries() {
    let mut p = Project::empty();
    // Mini: step 0 = rest, step 1 = 4 notes at offsets 0.0, 0.25, 0.5, 0.75
    p.patterns.push(Pattern {
        id: 1,
        name: "SubMini".into(),
        kind: PatternKind::Mini {
            code: "~ [1 2 3 4]".into(),
            steps: 2,
        },
    });
    p.patterns.push(Pattern {
        id: 2,
        name: "SwungSub".into(),
        kind: PatternKind::Swing {
            source: 1,
            amount: 0.35,
        },
    });
    // Step 1 (odd): 0.0->0.35, 0.25->0.60, 0.50->0.85 stay in step 1; 0.75->1.10 carries to step 2!
    let s1 = evaluate_notes_at(&p, 2, 1).unwrap();
    assert_eq!(s1.len(), 3);
    assert!((s1[0].offset - 0.35).abs() < 1e-4);
    assert!((s1[1].offset - 0.60).abs() < 1e-4);
    assert!((s1[2].offset - 0.85).abs() < 1e-4);
    let s2 = evaluate_notes_at(&p, 2, 2).unwrap();
    assert_eq!(s2.len(), 1);
    assert_eq!(s2[0].step.degree, 4);
    assert!(
        (s2[0].offset - 0.10).abs() < 1e-4,
        "overflowing Swing hit must carry into step 2 at 0.10, got {}",
        s2[0].offset
    );

    // Ratchet(count=4) over Swing(amount=0.40) on odd step 1 -> offsets 1.40, 1.65, 1.90, 2.15!
    p.patterns.push(Pattern {
        id: 3,
        name: "TwoNotes".into(),
        kind: PatternKind::Notes {
            steps: vec![Step::default(), note_step(5, 1.0)],
        },
    });
    p.patterns.push(Pattern {
        id: 4,
        name: "Swing40".into(),
        kind: PatternKind::Swing {
            source: 3,
            amount: 0.40,
        },
    });
    p.patterns.push(Pattern {
        id: 5,
        name: "RatchetOverSwing".into(),
        kind: PatternKind::Ratchet {
            source: 4,
            count: 4,
        },
    });
    let r1 = evaluate_notes_at(&p, 5, 1).unwrap();
    assert_eq!(r1.len(), 3);
    assert!((r1[0].offset - 0.40).abs() < 1e-4);
    assert!((r1[1].offset - 0.65).abs() < 1e-4);
    assert!((r1[2].offset - 0.90).abs() < 1e-4);
    let r2 = evaluate_notes_at(&p, 5, 2).unwrap();
    assert_eq!(r2.len(), 1);
    assert!(
        (r2[0].offset - 0.15).abs() < 1e-4,
        "4th ratchet hit over swung step must carry into step 2 at 0.15, got {}",
        r2[0].offset
    );
}

#[test]
fn reverse_preserves_substep_timing_and_involution_for_notes_and_values() {
    let mut p = Project::empty();
    // 1. Values(Ramp): period 4, (0, 0.0) -> (3, 1.0)
    p.patterns.push(Pattern {
        id: 1,
        name: "Ramp".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 4,
            mode: ValueMode::Ramp,
            points: vec![
                ValuePoint {
                    step: 0,
                    value: 0.0,
                },
                ValuePoint {
                    step: 3,
                    value: 1.0,
                },
            ],
        },
    });
    p.patterns.push(Pattern {
        id: 2,
        name: "RevRamp".into(),
        kind: PatternKind::Reverse { source: 1 },
    });
    p.patterns.push(Pattern {
        id: 3,
        name: "RevRevRamp".into(),
        kind: PatternKind::Reverse { source: 2 },
    });
    assert!((p.value_at(2, 0.0).unwrap() - 1.0).abs() < 1e-5);
    assert!((p.value_at(2, 3.0).unwrap() - 0.0).abs() < 1e-5);
    // Reversed Ramp interpolates smoothly across the cyclic wrap segment [3.0, 4.0) (0.5 at 3.5).
    assert!((p.value_at(2, 3.5).unwrap() - 0.5).abs() < 1e-5);
    for &t in &[0.0, 0.25, 1.5, 2.75, 3.5] {
        assert!(
            (p.value_at(3, t).unwrap() - p.value_at(1, t).unwrap()).abs() < 1e-5,
            "Reverse(Reverse(Ramp)) must equal Ramp at t={t}"
        );
    }

    // 2. Values(Hold): step-held reflection keeps sub-steps inside their reversed step
    p.patterns.push(Pattern {
        id: 4,
        name: "Hold".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 4,
            mode: ValueMode::Hold,
            points: vec![
                ValuePoint {
                    step: 0,
                    value: 0.1,
                },
                ValuePoint {
                    step: 1,
                    value: 0.2,
                },
                ValuePoint {
                    step: 2,
                    value: 0.3,
                },
                ValuePoint {
                    step: 3,
                    value: 0.4,
                },
            ],
        },
    });
    p.patterns.push(Pattern {
        id: 5,
        name: "RevHold".into(),
        kind: PatternKind::Reverse { source: 4 },
    });
    assert!((p.value_at(5, 0.0).unwrap() - 0.4).abs() < 1e-5);
    assert!((p.value_at(5, 0.5).unwrap() - 0.4).abs() < 1e-5);
    assert!((p.value_at(5, 3.5).unwrap() - 0.1).abs() < 1e-5);

    // 3. Notes with sub-step triplets: TimeScale(3/2, [1, 2, 3]) has period 2
    p.patterns.push(Pattern {
        id: 6,
        name: "TriSrc".into(),
        kind: PatternKind::Notes {
            steps: vec![note_step(1, 0.5), note_step(2, 0.5), note_step(3, 0.5)],
        },
    });
    p.patterns.push(Pattern {
        id: 7,
        name: "Triplets".into(),
        kind: PatternKind::TimeScale {
            source: 6,
            num: 3,
            den: 2,
        },
    });
    p.patterns.push(Pattern {
        id: 8,
        name: "RevTriplets".into(),
        kind: PatternKind::Reverse { source: 7 },
    });
    p.patterns.push(Pattern {
        id: 9,
        name: "RevRevTriplets".into(),
        kind: PatternKind::Reverse { source: 8 },
    });
    let mut rev_times: Vec<(f32, i16)> = Vec::new();
    for t in 0..2 {
        for ev in evaluate_notes_at(&p, 8, t).unwrap() {
            rev_times.push((t as f32 + ev.offset, ev.step.degree));
        }
    }
    assert_eq!(rev_times.len(), 3);
    // Exact uniform 2/3-step spacing preserved under Reverse!
    assert!((rev_times[1].0 - rev_times[0].0 - 2.0 / 3.0).abs() < 1e-3);
    assert!((rev_times[2].0 - rev_times[1].0 - 2.0 / 3.0).abs() < 1e-3);
    // Involution: Reverse(Reverse(Triplets)) == Triplets!
    for t in 0..2 {
        let orig = evaluate_notes_at(&p, 7, t).unwrap();
        let round = evaluate_notes_at(&p, 9, t).unwrap();
        assert_eq!(orig.len(), round.len());
        for (a, b) in orig.iter().zip(&round) {
            assert_eq!(a.step.degree, b.step.degree);
            assert!((a.offset - b.offset).abs() < 1e-3);
        }
    }
}

#[test]
fn validated_extreme_degree_compositions_compile_cleanly() {
    let mut p = Project::demo();
    let high_notes = p.max_id() + 1;
    p.patterns.push(Pattern {
        id: high_notes,
        name: "HighNotes".into(),
        kind: PatternKind::Notes {
            steps: vec![note_step(music::MAX_DEGREE, 0.5)],
        },
    });
    let deg_plus = p.max_id() + 1;
    p.patterns.push(Pattern {
        id: deg_plus,
        name: "Plus63".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Degrees,
            period: 4,
            mode: ValueMode::Hold,
            points: vec![ValuePoint {
                step: 0,
                value: 63.0,
            }],
        },
    });
    let added = p.max_id() + 1;
    p.patterns.push(Pattern {
        id: added,
        name: "ExtremeAdd".into(),
        kind: PatternKind::AddDegrees {
            notes: high_notes,
            values: deg_plus,
        },
    });
    let nfv = p.max_id() + 1;
    p.patterns.push(Pattern {
        id: nfv,
        name: "ExtremeNFV".into(),
        kind: PatternKind::NotesFromValues {
            values: deg_plus,
            rhythm: 3,
            degree_offset: 63,
            velocity: 0.8,
            gate: 0.5,
        },
    });
    p.parts[0].pattern = added;
    p.parts[2].pattern = nfv;
    p.validate().unwrap();
    let score = p.compile().unwrap();
    assert!(!score.events.is_empty());
}

#[test]
fn arp_held_chord_survives_intervening_short_note() {
    let mut p = Project::empty();
    let mut chord = note_step(1, 4.0);
    chord.chord = vec![
        ChordTone {
            degree: 3,
            accidental: 0,
            velocity: 0.8,
            gate: 4.0,
        },
        ChordTone {
            degree: 5,
            accidental: 0,
            velocity: 0.8,
            gate: 4.0,
        },
    ];
    // Step 0: 3-note chord held for 4 steps; Step 1: short passing note (gate=0.5); Step 2: rest.
    p.patterns.push(Pattern {
        id: 1,
        name: "ChordPlusGrace".into(),
        kind: PatternKind::Notes {
            steps: vec![chord, note_step(2, 0.5), Step::default(), Step::default()],
        },
    });
    p.patterns.push(Pattern {
        id: 2,
        name: "ArpOverChord".into(),
        kind: PatternKind::Arp {
            source: 1,
            mode: ArpMode::Up,
            rate: 1,
        },
    });
    // At step 2, the short note at step 1 has ended, but step 0's chord (gate=4.0) is STILL held!
    let s2 = evaluate_notes_at(&p, 2, 2).unwrap();
    assert_eq!(
        s2.len(),
        1,
        "Arp must keep arpeggiating step 0's held chord at step 2 despite short note at step 1"
    );
    assert_eq!(s2[0].step.degree, 5);
}

#[test]
fn midi_to_degree_unclamped_roundtrips_all_midi_including_chromatic_high_notes() {
    let scales: [&[i8]; 5] = [
        &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11], // chromatic
        &[0, 2, 4, 5, 7, 9, 11],                 // major
        &[0, 2, 3, 5, 7, 8, 10],                 // minor
        &[0, 3, 5, 7, 10],                       // pentatonic
        &[0],                                    // single-note scale
    ];
    for scale in scales {
        for root_midi in [0u8, 36, 42, 60, 62, 127] {
            let mut p = Project::empty();
            p.root_midi = root_midi;
            p.scale = scale.to_vec();
            for midi in 0u8..=127 {
                let (deg, acc) = p.midi_to_degree(midi);
                assert!(
                    (music::MIN_DEGREE..=music::MAX_DEGREE).contains(&deg),
                    "deg {deg} out of bounds for root={root_midi} midi={midi}"
                );
                assert!((-6..=6).contains(&acc));
                assert_eq!(
                    p.pitch_midi(deg, acc, 0),
                    midi,
                    "roundtrip failed for root={root_midi} scale={scale:?} midi={midi} -> ({deg}, {acc})"
                );
            }
        }
    }
}

#[test]
fn value_mode_ramp_preserved_filter_pattern_64_substep_and_consumers() {
    let mut p = Project::demo();
    let ramp_id = 7; // Energy (ValueMode::Ramp)
    let shifted = p.max_id() + 1;
    p.patterns.push(Pattern {
        id: shifted,
        name: "ShiftedEnergy".into(),
        kind: PatternKind::Shift {
            source: ramp_id,
            steps: 4,
        },
    });
    let reversed = p.max_id() + 1;
    p.patterns.push(Pattern {
        id: reversed,
        name: "RevEnergy".into(),
        kind: PatternKind::Reverse { source: shifted },
    });
    assert_eq!(p.value_mode(shifted).unwrap(), ValueMode::Ramp);
    assert_eq!(p.value_mode(reversed).unwrap(), ValueMode::Ramp);

    // Compiling a modulation referencing a derived Ramp emits ValueMode::Ramp.
    p.modulations[0].pattern = reversed;
    let score = p.compile().unwrap();
    assert_eq!(score.mixer.automations[0].mode, ValueMode::Ramp);

    // FreezePattern preserves ValueMode::Ramp through derived pattern!
    let frozen_id = p.max_id() + 1;
    p.apply(Edit::FreezePattern {
        source: reversed,
        id: frozen_id,
        name: "FrozenEnergy".into(),
        start: 0,
        length: 16,
    })
    .unwrap();
    assert_eq!(p.value_mode(frozen_id).unwrap(), ValueMode::Ramp);

    // compile_filter_pattern: preserves smooth_attack on derived Ramp, rejects Values(Degrees),
    // detects sub-step >= 0.125, and supports up to 64 steps!
    assert!(p.compile_filter_pattern(reversed).unwrap().smooth_attack);
    assert!(
        p.compile_filter_pattern(2).is_none(),
        "Values(Degrees) must be rejected for filter envelope"
    );
    let sub_id = p.max_id() + 1;
    p.patterns.push(Pattern {
        id: sub_id,
        name: "Swung32".into(),
        kind: PatternKind::Swing {
            source: 6, // 16-step Pulse
            amount: 0.15,
        },
    });
    // Make a 32-step swung pattern so 32 * 2 = 64 filter env steps!
    let long_pulse = p.max_id() + 1;
    p.patterns.push(Pattern {
        id: long_pulse,
        name: "Pulse32".into(),
        kind: PatternKind::Notes {
            steps: (0..32)
                .map(|i| {
                    if i % 2 == 1 {
                        note_step(1, 0.5)
                    } else {
                        Step::default()
                    }
                })
                .collect(),
        },
    });
    let swung32 = p.max_id() + 1;
    p.patterns.push(Pattern {
        id: swung32,
        name: "Swung32Long".into(),
        kind: PatternKind::Swing {
            source: long_pulse,
            amount: 0.15,
        },
    });
    let compiled_flt = p.compile_filter_pattern(swung32).unwrap();
    assert_eq!(compiled_flt.resolution, 32);
    assert_eq!(compiled_flt.length, 64);

    // Validate rejects Filter with Values(Degrees) env_pattern_id, accepts swung32,
    // and Project::consumers() includes both env_pattern_id and Boundary::Cycle!
    let flt_id = p.max_id() + 1;
    let mut flt = EffectDevice::filter(flt_id);
    if let EffectKind::Filter(ref mut fp) = flt.kind {
        fp.env_pattern_id = Some(2); // Harmony = Values(Degrees)
    }
    p.mixer.master.inserts.push(flt);
    assert!(p.validate().is_err());

    if let EffectKind::Filter(ref mut fp) = p.mixer.master.inserts.last_mut().unwrap().kind {
        fp.env_pattern_id = Some(swung32);
    }
    p.timing.launch = Boundary::Cycle { pattern: sub_id };
    p.validate().unwrap();
    assert!(p
        .consumers(swung32)
        .iter()
        .any(|c| c.kind == "effect" && c.id == flt_id));
    assert!(p.consumers(sub_id).iter().any(|c| c.kind == "timing"));
}

#[test]
fn parameter_links_sample_fractional_offset_and_live_override_beats_arrangement() {
    let mut p = Project::demo();
    let ramp_id = p.max_id() + 1;
    p.patterns.push(Pattern {
        id: ramp_id,
        name: "StepRamp".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 2,
            mode: ValueMode::Ramp,
            points: vec![
                ValuePoint {
                    step: 0,
                    value: 0.0,
                },
                ValuePoint {
                    step: 1,
                    value: 1.0,
                },
            ],
        },
    });
    let ratch_id = p.max_id() + 1;
    p.patterns.push(Pattern {
        id: ratch_id,
        name: "Ratch4".into(),
        kind: PatternKind::Ratchet {
            source: 1,
            count: 4,
        },
    });
    p.parts[0].pattern = ratch_id;
    p.parts[0].parameter_links.push(ParameterLink {
        target: InstrumentParam::Module {
            zone: 1,
            module: 4,
            param: ModuleParam::Cutoff,
        },
        pattern: ramp_id,
    });
    let score = p.compile().unwrap();
    let evs: Vec<_> = score
        .step_events(0)
        .iter()
        .filter(|e| e.part_id == Some(p.parts[0].id))
        .collect();
    assert_eq!(evs.len(), 4);
    let lock_vals: Vec<f32> = evs.iter().map(|e| score.event_locks(e)[0].value).collect();
    assert!((lock_vals[0] - 0.0).abs() < 1e-4);
    assert!((lock_vals[1] - 0.25).abs() < 1e-4);
    assert!((lock_vals[2] - 0.50).abs() < 1e-4);
    assert!((lock_vals[3] - 0.75).abs() < 1e-4);

    // Live Override on a LATER lane (lane 2) targeting Instrument 1 must override
    // earlier lane 1's arrangement placement on Instrument 1 (on both hits AND rests!).
    let sparse_pat = p.max_id() + 1;
    p.patterns.push(Pattern {
        id: sparse_pat,
        name: "SparseHigh".into(),
        kind: PatternKind::Notes {
            steps: vec![note_step(15, 0.5), Step::default()],
        },
    });
    let ov_part_id = p.max_id() + 1;
    p.parts.push(Part::new(ov_part_id, "OvLead", sparse_pat, 1));
    p.lanes[1].parts.push(LanePart::new(ov_part_id));
    p.parts[0].pattern = 1; // 5-step Motif on lane 1 (active on step 0 AND step 1!)
    p.validate().unwrap();

    let flags = LiveFlags::default();
    let active = [Override {
        lane: p.lanes[1].id,
        part: LanePart::new(ov_part_id),
        anchor: 0,
    }];
    let seg = crate::timeline::Segment::new(0, 0, None, EndRule::Loop);
    let live_score = p
        .compile_live(
            &flags,
            0,
            &[LiveSegment {
                from: 0,
                song: &p,
                flags: &flags,
                active: &active,
                timeline: Some(seg),
                audition: None,
            }],
        )
        .unwrap();
    let inst0 = live_score.instrument_index(1).unwrap();
    // Step 0: Override's high note wins over lane 1's arrangement placement!
    let ev0 = live_score
        .event_for(inst0, 0)
        .expect("override hit at step 0");
    assert_eq!(ev0.part_id, Some(ov_part_id));
    // Step 1: Override rests -> lane 1's arrangement placement must NOT bleed through!
    assert!(
        live_score.event_for(inst0, 1).is_none(),
        "arrangement placement on lane 1 must not bleed through during lane 2 live Override rest"
    );
}

#[test]
fn rack_instrument_and_patchbay_edits_enforce_invariants() {
    let mut p = Project::demo();
    let inst_id = p.instruments[0].id;
    let ch = p.instruments[0].channel;
    let alt_ch = p
        .mixer
        .channels
        .iter()
        .find(|c| c.id != ch && c.kind == crate::mixer::ChannelKind::Audio)
        .unwrap()
        .id;

    // 1. CreateInstrument rejects duplicate IDs.
    let dup_err = p
        .apply(Edit::CreateInstrument {
            instrument: InstrumentInstance {
                id: inst_id,
                name: "Duplicate".into(),
                channel: ch,
                rack: RackPatch::default_synth(),
            },
        })
        .expect_err("CreateInstrument must reject existing instrument ID");
    assert!(dup_err.to_string().contains("already exists"));

    // 2. UpdateInstrumentMeta updates trimmed name and/or channel without touching rack.
    let original_rack = p.instrument(inst_id).unwrap().rack.clone();
    p.apply(Edit::UpdateInstrumentMeta {
        id: inst_id,
        name: Some("  Saw Renamed  ".into()),
        channel: Some(alt_ch),
    })
    .expect("UpdateInstrumentMeta must succeed");
    let updated = p.instrument(inst_id).unwrap();
    assert_eq!(updated.name, "Saw Renamed");
    assert_eq!(updated.channel, alt_ch);
    assert_eq!(updated.rack, original_rack);
    assert!(p
        .apply(Edit::UpdateInstrumentMeta {
            id: 999_999,
            name: Some("Ghost".into()),
            channel: None,
        })
        .is_err());

    // 3. Create a fresh modular Rack instrument and exercise granular zone/module/cable/pin edits.
    let rack_id = p.max_id() + 1;
    p.apply(Edit::CreateInstrument {
        instrument: InstrumentInstance {
            id: rack_id,
            name: "ModLab".into(),
            channel: ch,
            rack: RackPatch::default_synth(),
        },
    })
    .unwrap();

    // Cannot remove the only zone.
    assert!(p
        .apply(Edit::RemoveRackZone {
            instrument: rack_id,
            zone: 1,
        })
        .is_err());

    // Add a second zone, reorder it to index 0, then verify both zones stay intact.
    let z2 = RackZone::sample_pad(2, "Snare", 38, "snare.wav");
    p.apply(Edit::AddRackZone {
        instrument: rack_id,
        zone: z2,
        index: None,
    })
    .unwrap();
    assert_eq!(p.instrument(rack_id).unwrap().rack.zones.len(), 2);
    p.apply(Edit::ReorderRackZone {
        instrument: rack_id,
        zone: 2,
        target_index: 0,
    })
    .unwrap();
    assert_eq!(p.instrument(rack_id).unwrap().rack.zones[0].id, 2);
    assert_eq!(p.instrument(rack_id).unwrap().rack.zones[1].id, 1);

    // Add a LoFi module onto zone 1, rename it, patch it, adjust cable amount, and pin it.
    let lofi_id = p
        .instrument(rack_id)
        .unwrap()
        .rack
        .zone(1)
        .unwrap()
        .next_module_id();
    p.apply(Edit::AddRackModule {
        instrument: rack_id,
        zone: 1,
        module: RackModule::new(lofi_id, "Crunch", ModuleKind::LoFi(LoFiModule::default())),
    })
    .unwrap();
    p.apply(Edit::RenameRackModule {
        instrument: rack_id,
        zone: 1,
        module: lofi_id,
        name: "  BitCrush  ".into(),
    })
    .unwrap();
    assert_eq!(
        p.instrument(rack_id)
            .unwrap()
            .rack
            .zone(1)
            .unwrap()
            .module(lofi_id)
            .unwrap()
            .name,
        "BitCrush"
    );

    // Cannot add a second Out module or remove the mandatory Out module.
    let out_id = p
        .instrument(rack_id)
        .unwrap()
        .rack
        .zone(1)
        .unwrap()
        .out_module()
        .unwrap()
        .id;
    assert!(p
        .apply(Edit::AddRackModule {
            instrument: rack_id,
            zone: 1,
            module: RackModule::new(
                lofi_id + 1,
                "Out2",
                ModuleKind::Out(crate::rack::OutModule::default()),
            ),
        })
        .is_err());
    assert!(p
        .apply(Edit::RemoveRackModule {
            instrument: rack_id,
            zone: 1,
            module: out_id,
        })
        .is_err());

    // Patch VCA (#3) -> BitCrush -> Out (#6), tweak cable amount, pin bits, then remove module.
    let cable_in_id = p
        .instrument(rack_id)
        .unwrap()
        .rack
        .zone(1)
        .unwrap()
        .next_cable_id();
    let score = p
        .apply_transaction(&[
            Edit::ConnectCable {
                instrument: rack_id,
                zone: 1,
                cable: PatchCable::new(cable_in_id, 3, lofi_id, InPort::In, 0.75),
            },
            Edit::SetCableAmount {
                instrument: rack_id,
                zone: 1,
                cable: cable_in_id,
                amount: 1.25,
            },
            Edit::PinPanelControl {
                instrument: rack_id,
                pin: PanelPin::module(1, lofi_id, ModuleParam::Bits, "Bits"),
                index: Some(0),
            },
        ])
        .unwrap();
    let compiled_inst = score.instruments.iter().find(|i| i.id == rack_id).unwrap();
    let g1 = compiled_inst.compiled_zone(1).unwrap();
    let c_idx = g1.cable_index_for_id(cable_in_id).unwrap();
    assert!((g1.cables[c_idx].amount - 1.25).abs() < 1e-6);
    assert_eq!(
        p.instrument(rack_id).unwrap().rack.panel_pins[0].label,
        "Bits"
    );

    // Removing BitCrush automatically disconnects its cable AND prunes its panel pin!
    p.apply(Edit::RemoveRackModule {
        instrument: rack_id,
        zone: 1,
        module: lofi_id,
    })
    .unwrap();
    assert!(p
        .instrument(rack_id)
        .unwrap()
        .rack
        .zone(1)
        .unwrap()
        .cable(cable_in_id)
        .is_none());
    assert!(p
        .instrument(rack_id)
        .unwrap()
        .rack
        .panel_pins
        .iter()
        .all(|pin| pin.label != "Bits"));

    // Unpin removes a panel pin cleanly; missing instrument ID rejected by UpdateRackPatch.
    let before_pins = p.instrument(rack_id).unwrap().rack.panel_pins.len();
    p.apply(Edit::UnpinPanelControl {
        instrument: rack_id,
        index: 0,
    })
    .unwrap();
    assert_eq!(
        p.instrument(rack_id).unwrap().rack.panel_pins.len(),
        before_pins - 1
    );
    assert!(p
        .apply(Edit::UpdateRackPatch {
            id: 888_888,
            rack: RackPatch::default_synth(),
        })
        .is_err());
}

#[test]
fn rack_structural_edits_prune_stale_pins_macros_modulations_and_part_links() {
    use crate::macros::{MacroBinding, MacroControl};
    use crate::params::ParamAddress;

    let mut p = Project::demo();
    let ch = p.mixer.channels[0].id;
    let kit_id = p.max_id() + 1;
    let part_id = kit_id + 1;
    let mod_z1_filter = kit_id + 2;
    let mod_z1_cable = kit_id + 3;
    let mod_z2_gain = kit_id + 4;
    let val_pat_id = p.patterns[6].id;
    let note_pat_id = p.patterns[0].id;

    let mut initial_rack = RackPatch::default_synth();
    initial_rack
        .zones
        .push(RackZone::sample_pad(2, "Snare", 38, "snare.wav"));
    // `default_synth()` already pins cable 5 ("Env Mod"); also pin zone 2's Gain!
    assert!(initial_rack
        .panel_pins
        .iter()
        .any(|pin| pin.target == InstrumentParam::Cable { zone: 1, cable: 5 }));
    initial_rack
        .panel_pins
        .push(PanelPin::zone(2, ZoneParam::Gain, "SnareLvl"));

    let mut part = Part::new(part_id, "KitPart", note_pat_id, kit_id);
    part.parameter_links = vec![
        ParameterLink {
            target: InstrumentParam::Module {
                zone: 1,
                module: 2,
                param: ModuleParam::Cutoff,
            },
            pattern: val_pat_id,
        },
        ParameterLink {
            target: InstrumentParam::Cable { zone: 1, cable: 5 },
            pattern: val_pat_id,
        },
        ParameterLink {
            target: InstrumentParam::Zone {
                zone: 2,
                param: ZoneParam::Gain,
            },
            pattern: val_pat_id,
        },
    ];

    let mut mac = MacroControl::new(0, "Rack Macro");
    mac.value = 0.75;
    mac.bindings = vec![
        // Survives all structural edits below (Rack bus gain).
        MacroBinding::new(
            ParamAddress::Rack {
                instrument: kit_id,
                param: crate::rack::RackParam::Gain,
            },
            0.0,
            1.0,
        ),
        // Survives zone 2 removal, pruned when cable 5 is disconnected.
        MacroBinding::new(
            ParamAddress::Cable {
                instrument: kit_id,
                zone: 1,
                cable: 5,
            },
            0.5,
            1.0,
        ),
        // Pruned when zone 2 is removed.
        MacroBinding::new(
            ParamAddress::Zone {
                instrument: kit_id,
                zone: 2,
                param: ZoneParam::Gain,
            },
            0.0,
            1.0,
        ),
        // Pruned when Filter module #2 is removed.
        MacroBinding::new(
            ParamAddress::Module {
                instrument: kit_id,
                zone: 1,
                module: 2,
                param: ModuleParam::Cutoff,
            },
            0.0,
            1.0,
        ),
    ];

    let score = p
        .apply_transaction(&[
            Edit::CreateInstrument {
                instrument: InstrumentInstance {
                    id: kit_id,
                    name: "HybridRack".into(),
                    channel: ch,
                    rack: initial_rack.clone(),
                },
            },
            Edit::PutPart { part },
            Edit::PutModulation {
                modulation: Modulation {
                    id: mod_z1_filter,
                    name: "Z1 Cutoff".into(),
                    target: ModulationTarget::Instrument {
                        instrument: kit_id,
                        parameter: InstrumentParam::Module {
                            zone: 1,
                            module: 2,
                            param: ModuleParam::Cutoff,
                        },
                    },
                    pattern: val_pat_id,
                },
            },
            Edit::PutModulation {
                modulation: Modulation {
                    id: mod_z1_cable,
                    name: "Z1 EnvMod".into(),
                    target: ModulationTarget::Instrument {
                        instrument: kit_id,
                        parameter: InstrumentParam::Cable { zone: 1, cable: 5 },
                    },
                    pattern: val_pat_id,
                },
            },
            Edit::PutModulation {
                modulation: Modulation {
                    id: mod_z2_gain,
                    name: "Z2 Gain".into(),
                    target: ModulationTarget::Instrument {
                        instrument: kit_id,
                        parameter: InstrumentParam::Zone {
                            zone: 2,
                            param: ZoneParam::Gain,
                        },
                    },
                    pattern: val_pat_id,
                },
            },
            Edit::PutMacro { macro_control: mac },
        ])
        .expect("initial Rack setup with macro/mod/part/pin bindings must compile");

    // Verify static Macro binding on cable #5 propagated into CompiledZoneGraph!
    let sc_inst = score.instruments.iter().find(|i| i.id == kit_id).unwrap();
    let g1 = sc_inst.compiled_zone(1).unwrap();
    let c5 = g1.cables[g1.cable_index_for_id(5).unwrap()];
    // Macro value 0.75 over [0.5, 1.0] => norm 0.875 on Filter Cv (-6..=6 oct) => +4.5 oct.
    assert!(
        (c5.amount - 4.5).abs() < 1e-4,
        "static macro must update CompiledCable.amount, got {}",
        c5.amount
    );

    // Step A: Reordering zones preserves EVERY reference (stable zone_id!).
    p.apply(Edit::ReorderRackZone {
        instrument: kit_id,
        zone: 2,
        target_index: 0,
    })
    .unwrap();
    assert_eq!(p.macro_control(0).unwrap().bindings.len(), 4);
    assert_eq!(p.part(part_id).unwrap().parameter_links.len(), 3);

    // Step B: RemoveRackZone(2) via apply_edit_only prunes zone 2 refs across all 4 tables!
    let mut direct = p.clone();
    direct
        .apply_edit_only(&Edit::RemoveRackZone {
            instrument: kit_id,
            zone: 2,
        })
        .unwrap();
    direct
        .validate()
        .expect("apply_edit_only(RemoveRackZone) must prune stale refs and stay valid");
    assert_eq!(direct.macro_control(0).unwrap().bindings.len(), 3);
    assert!(direct
        .instrument(kit_id)
        .unwrap()
        .rack
        .panel_pins
        .iter()
        .all(|pin| pin.label != "SnareLvl"));

    p.apply(Edit::RemoveRackZone {
        instrument: kit_id,
        zone: 2,
    })
    .unwrap();
    assert!(!p.modulations.iter().any(|m| m.id == mod_z2_gain));
    assert_eq!(p.part(part_id).unwrap().parameter_links.len(), 2);

    // Step C: DisconnectCable(5) prunes cable 5 pin, modulation, part link, and macro binding!
    p.apply(Edit::DisconnectCable {
        instrument: kit_id,
        zone: 1,
        cable: 5,
    })
    .unwrap();
    assert!(!p.modulations.iter().any(|m| m.id == mod_z1_cable));
    assert_eq!(p.part(part_id).unwrap().parameter_links.len(), 1);
    assert_eq!(p.macro_control(0).unwrap().bindings.len(), 2);
    assert!(p
        .instrument(kit_id)
        .unwrap()
        .rack
        .panel_pins
        .iter()
        .all(|pin| pin.target != InstrumentParam::Cable { zone: 1, cable: 5 }));

    // Step D: RemoveRackModule(Filter #2) prunes Filter cutoff modulation, link, macro, and pins!
    p.apply(Edit::RemoveRackModule {
        instrument: kit_id,
        zone: 1,
        module: 2,
    })
    .unwrap();
    assert!(!p.modulations.iter().any(|m| m.id == mod_z1_filter));
    assert!(p.part(part_id).unwrap().parameter_links.is_empty());
    let remaining = &p.macro_control(0).unwrap().bindings;
    assert_eq!(remaining.len(), 1);
    assert_eq!(
        remaining[0].target,
        ParamAddress::Rack {
            instrument: kit_id,
            param: crate::rack::RackParam::Gain,
        }
    );

    // Step E: UpdateRackPatch to a bare blank_modular also validates cleanly!
    p.apply(Edit::UpdateRackPatch {
        id: kit_id,
        rack: RackPatch::blank_modular(),
    })
    .unwrap();
    assert_eq!(p.macro_control(0).unwrap().bindings.len(), 1);
}

#[test]
fn typed_channel_and_effect_edits_enforce_structural_invariants_and_round_trip() {
    use crate::macros::{MacroBinding, MacroControl};
    use crate::mixer::{
        Channel, ChannelKind, ChannelParam, ChannelRef, ChannelSend, EffectKind, EffectParam,
        EqPatch,
    };
    use crate::params::ParamAddress;
    use crate::take::diff_project_edits;

    let mut p = Project::demo();
    let audio_ch_id = p
        .mixer
        .channels
        .iter()
        .find(|c| c.kind == ChannelKind::Audio)
        .unwrap()
        .id;
    let other_audio_ch_id = p
        .mixer
        .channels
        .iter()
        .find(|c| c.id != audio_ch_id && c.kind == ChannelKind::Audio)
        .unwrap()
        .id;

    // 1. CreateChannel rejects duplicate channel IDs and duplicate effect device IDs.
    let dup_ch = Channel::new(audio_ch_id, ChannelKind::Group, "Duplicate");
    let err = p
        .apply(Edit::CreateChannel { channel: dup_ch })
        .expect_err("CreateChannel must reject existing channel ID");
    assert!(err.to_string().contains("already exists"));

    let ret_id = p.max_id() + 1;
    let mut ret_ch = Channel::new(ret_id, ChannelKind::Return, "Verb Return");
    let existing_dev_id = p
        .mixer
        .channels
        .iter()
        .flat_map(|c| c.inserts.iter())
        .chain(p.mixer.master.inserts.iter())
        .next()
        .map(|d| d.id);
    if let Some(dup_dev_id) = existing_dev_id {
        let mut bad_ch = ret_ch.clone();
        bad_ch.inserts.push(EffectDevice::reverb(dup_dev_id));
        assert!(
            p.apply(Edit::CreateChannel { channel: bad_ch }).is_err(),
            "CreateChannel must reject colliding effect device ID"
        );
    }
    let dup_internal_id = ret_id + 1;
    let mut bad_internal = ret_ch.clone();
    bad_internal
        .inserts
        .push(EffectDevice::reverb(dup_internal_id));
    bad_internal
        .inserts
        .push(EffectDevice::delay(dup_internal_id));
    assert!(
        p.apply(Edit::CreateChannel {
            channel: bad_internal,
        })
        .is_err(),
        "CreateChannel must reject duplicate device IDs inside inserts"
    );

    // Creating a fresh Return channel succeeds.
    ret_ch.strip.gain = 0.85;
    p.apply(Edit::CreateChannel {
        channel: ret_ch.clone(),
    })
    .expect("CreateChannel must accept fresh Return channel");
    assert_eq!(p.channel(ret_id).unwrap().kind, ChannelKind::Return);

    // Sneaky RemoveChannel + CreateChannel changing ChannelKind in one tx is rejected!
    let ret_as_group = Channel::new(ret_id, ChannelKind::Group, "Morph Attempt");
    let err = p
        .apply_transaction_validated(&[
            Edit::RemoveChannel { id: ret_id },
            Edit::CreateChannel {
                channel: ret_as_group,
            },
        ])
        .expect_err("RemoveChannel + CreateChannel kind swap must fail");
    assert!(err.to_string().contains("Return") && err.to_string().contains("Group"));

    // 2. UpdateChannelMeta updates name, output, and sends without touching kind, strip, or inserts.
    let grp_id = p.max_id() + 1;
    p.apply(Edit::CreateChannel {
        channel: Channel::new(grp_id, ChannelKind::Group, "Drum Bus"),
    })
    .unwrap();
    let prev_strip = p.channel(audio_ch_id).unwrap().strip;
    let prev_inserts = p.channel(audio_ch_id).unwrap().inserts.clone();
    p.apply(Edit::UpdateChannelMeta {
        id: audio_ch_id,
        name: Some("  Lead Bus  ".into()),
        output: Some(ChannelRef::Channel(grp_id)),
        sends: Some(vec![ChannelSend {
            target: ret_id,
            amount: 0.35,
        }]),
    })
    .expect("UpdateChannelMeta must succeed");
    let updated_ch = p.channel(audio_ch_id).unwrap();
    assert_eq!(updated_ch.name, "Lead Bus");
    assert_eq!(updated_ch.kind, ChannelKind::Audio);
    assert_eq!(updated_ch.output, ChannelRef::Channel(grp_id));
    assert_eq!(updated_ch.sends.len(), 1);
    assert_eq!(updated_ch.sends[0].target, ret_id);
    assert_eq!(updated_ch.strip, prev_strip);
    assert_eq!(updated_ch.inserts, prev_inserts);

    assert!(
        p.apply(Edit::UpdateChannelMeta {
            id: 999_999,
            name: Some("Ghost".into()),
            output: None,
            sends: None,
        })
        .is_err(),
        "UpdateChannelMeta must reject missing channel"
    );

    // 3. AddEffect adds strictly new device IDs onto channels & Master and rejects duplicates/OOB.
    let eq_id = p.max_id() + 1;
    let comp_id = eq_id + 1;
    p.apply(Edit::AddEffect {
        channel: ChannelRef::Channel(audio_ch_id),
        device: EffectDevice::eq(eq_id),
        index: None,
    })
    .expect("AddEffect on Audio channel must succeed");
    assert_eq!(p.device(eq_id).unwrap().0, ChannelRef::Channel(audio_ch_id));

    // Duplicate device ID rejected across both same channel and Master.
    assert!(
        p.apply(Edit::AddEffect {
            channel: ChannelRef::Master,
            device: EffectDevice::delay(eq_id),
            index: None,
        })
        .is_err(),
        "AddEffect must reject duplicate device ID"
    );
    // Out-of-bounds index rejected.
    assert!(
        p.apply(Edit::AddEffect {
            channel: ChannelRef::Channel(audio_ch_id),
            device: EffectDevice::compressor(comp_id),
            index: Some(99),
        })
        .is_err(),
        "AddEffect must reject out-of-range index"
    );
    // Explicit index 0 inserts ahead of EQ.
    p.apply(Edit::AddEffect {
        channel: ChannelRef::Channel(audio_ch_id),
        device: EffectDevice::compressor(comp_id),
        index: Some(0),
    })
    .unwrap();
    assert_eq!(p.channel(audio_ch_id).unwrap().inserts[0].id, comp_id);
    assert_eq!(p.channel(audio_ch_id).unwrap().inserts[1].id, eq_id);

    // 4. UpdateEffect updates name, bypassed, and same-kind patch; rejects cross-kind mutations!
    let next_eq = EqPatch {
        low_db: 3.5,
        hpf_hz: 80.0,
        ..EqPatch::default()
    };
    p.apply(Edit::UpdateEffect {
        device: eq_id,
        name: Some("  Warm EQ  ".into()),
        bypassed: Some(true),
        kind: Some(EffectKind::Eq(next_eq)),
    })
    .expect("UpdateEffect with matching EffectKind must succeed");
    let (_, eq_dev) = p.device(eq_id).unwrap();
    assert_eq!(eq_dev.name, "Warm EQ");
    assert!(eq_dev.bypassed);
    assert_eq!(eq_dev.kind, EffectKind::Eq(next_eq));

    let err = p
        .apply(Edit::UpdateEffect {
            device: eq_id,
            name: None,
            bypassed: None,
            kind: Some(EffectDevice::delay(eq_id).kind),
        })
        .expect_err("UpdateEffect must reject EQ -> Delay kind swap");
    assert!(err.to_string().contains("EQ") && err.to_string().contains("Delay"));

    // Sneaky RemoveEffect + AddEffect swapping EffectKind in one tx is also rejected!
    let err = p
        .apply_transaction_validated(&[
            Edit::RemoveEffect { device: eq_id },
            Edit::AddEffect {
                channel: ChannelRef::Master,
                device: EffectDevice::reverb(eq_id),
                index: None,
            },
        ])
        .expect_err("RemoveEffect + AddEffect kind swap must fail");
    assert!(err.to_string().contains("EQ") && err.to_string().contains("Reverb"));

    // 5. MoveEffect reorders within a channel and moves across channels & Master preserving
    //    id, name, bypassed, kind, AND Macro bindings!
    let mut mac = MacroControl::new(0, "Tone");
    mac.bindings.push(MacroBinding::new(
        ParamAddress::Effect {
            device: eq_id,
            param: EffectParam::Low,
        },
        0.2,
        0.8,
    ));
    p.apply(Edit::PutMacro { macro_control: mac }).unwrap();

    // Same-channel reorder: move EQ ahead of Comp (slot 0).
    p.apply(Edit::MoveEffect {
        device: eq_id,
        target_channel: ChannelRef::Channel(audio_ch_id),
        target_slot: Some(0),
    })
    .unwrap();
    assert_eq!(p.channel(audio_ch_id).unwrap().inserts[0].id, eq_id);
    assert_eq!(p.channel(audio_ch_id).unwrap().inserts[1].id, comp_id);

    // Move across Audio channels.
    p.apply(Edit::MoveEffect {
        device: eq_id,
        target_channel: ChannelRef::Channel(other_audio_ch_id),
        target_slot: None,
    })
    .unwrap();
    assert_eq!(
        p.device(eq_id).unwrap().0,
        ChannelRef::Channel(other_audio_ch_id)
    );
    assert_eq!(p.device(eq_id).unwrap().1.name, "Warm EQ");
    assert!(p.device(eq_id).unwrap().1.bypassed);
    assert_eq!(p.device(eq_id).unwrap().1.kind, EffectKind::Eq(next_eq));
    assert_eq!(p.macro_control(0).unwrap().bindings.len(), 1);

    // Move onto Master at slot 0.
    p.apply(Edit::MoveEffect {
        device: eq_id,
        target_channel: ChannelRef::Master,
        target_slot: Some(0),
    })
    .unwrap();
    assert_eq!(p.device(eq_id).unwrap().0, ChannelRef::Master);
    assert_eq!(p.mixer.master.inserts[0].id, eq_id);
    assert_eq!(p.device(eq_id).unwrap().1.name, "Warm EQ");
    assert!(p.device(eq_id).unwrap().1.bypassed);
    assert_eq!(p.macro_control(0).unwrap().bindings.len(), 1);

    // Out-of-bounds MoveEffect target_slot is rejected.
    assert!(p
        .apply(Edit::MoveEffect {
            device: eq_id,
            target_channel: ChannelRef::Master,
            target_slot: Some(99),
        })
        .is_err());

    // 6. RemoveEffect removes from channel or Master, prunes stale Macro bindings, and rejects missing IDs.
    p.apply(Edit::RemoveEffect { device: eq_id }).unwrap();
    assert!(p.device(eq_id).is_none());
    assert!(
        p.macro_control(0).unwrap().bindings.is_empty(),
        "RemoveEffect must prune stale Macro bindings"
    );
    assert!(p.apply(Edit::RemoveEffect { device: eq_id }).is_err());

    // 7. diff_project_edits round-trips complex mixer changes using only typed channel/effect/param edits!
    let before = p.clone();
    let mut after = before.clone();
    // Create a brand new Return channel with a moved device + brand new device.
    let new_ret_id = after.max_id() + 1;
    let new_dly_id = new_ret_id + 1;
    let mut new_ret = Channel::new(new_ret_id, ChannelKind::Return, "Delay Return");
    new_ret.strip.gain = 0.72;
    new_ret.strip.pan = -0.25;
    // Move comp_id from audio_ch_id to new_ret, rename & bypass it, and put a brand new Delay ahead of it.
    let mut moved_comp = after
        .mixer
        .channels
        .iter_mut()
        .find(|c| c.id == audio_ch_id)
        .unwrap()
        .inserts
        .remove(0);
    assert_eq!(moved_comp.id, comp_id);
    moved_comp.name = "Ret Comp".into();
    moved_comp.bypassed = true;
    new_ret.inserts.push(EffectDevice::delay(new_dly_id));
    new_ret.inserts.push(moved_comp);
    after.mixer.channels.push(new_ret);

    // Update audio_ch_id metadata + strip gain/pan, and Master strip gain/pan + add a Master Limiter.
    let lim_id = new_dly_id + 1;
    let ch = after
        .mixer
        .channels
        .iter_mut()
        .find(|c| c.id == audio_ch_id)
        .unwrap();
    ch.name = "Lead Strip".into();
    ch.strip.gain = 1.25;
    ch.strip.pan = 0.4;
    ch.sends.push(ChannelSend {
        target: new_ret_id,
        amount: 0.5,
    });
    after.mixer.master.strip.gain = 0.92;
    after.mixer.master.strip.pan = -0.1;
    after
        .mixer
        .master
        .inserts
        .insert(0, EffectDevice::limiter(lim_id));

    let edits = diff_project_edits(&before, &after);
    assert!(edits.iter().any(|e| matches!(
        e,
        Edit::CreateChannel { channel } if channel.id == new_ret_id
    )));
    assert!(edits.iter().any(|e| matches!(
        e,
        Edit::UpdateChannelMeta { id, name: Some(n), .. } if *id == audio_ch_id && n == "Lead Strip"
    )));
    assert!(edits.iter().any(|e| matches!(
        e,
        Edit::SetParam {
            address: ParamAddress::Channel {
                channel: ChannelRef::Channel(id),
                param: ChannelParam::Gain,
            },
            value,
        } if *id == audio_ch_id && (*value - 1.25).abs() < 1e-6
    )));
    assert!(edits.iter().any(|e| matches!(
        e,
        Edit::SetParam {
            address: ParamAddress::Channel {
                channel: ChannelRef::Master,
                param: ChannelParam::Pan,
            },
            value,
        } if (*value + 0.1).abs() < 1e-6
    )));
    assert!(edits.iter().any(|e| matches!(
        e,
        Edit::MoveEffect {
            device,
            target_channel: ChannelRef::Channel(ch_id),
            ..
        } if *device == comp_id && *ch_id == new_ret_id
    )));
    assert!(edits.iter().any(|e| matches!(
        e,
        Edit::UpdateEffect {
            device,
            name: Some(n),
            bypassed: Some(true),
            kind: None,
        } if *device == comp_id && n == "Ret Comp"
    )));
    assert!(edits.iter().any(|e| matches!(
        e,
        Edit::AddEffect {
            channel: ChannelRef::Master,
            device,
            index: None | Some(0),
        } if device.id == lim_id
    )));

    let mut rebuilt = before.clone();
    rebuilt
        .apply_transaction_validated(&edits)
        .expect("diff_project_edits forward round-trip must succeed");
    assert_eq!(rebuilt.mixer, after.mixer);

    // Reverse diff (removing new channel/effects & moving comp back) also round-trips cleanly!
    let reverse_edits = diff_project_edits(&after, &before);
    assert!(reverse_edits.iter().any(|e| matches!(
        e,
        Edit::RemoveEffect { device } if *device == lim_id
    )));
    assert!(reverse_edits.iter().any(|e| matches!(
        e,
        Edit::RemoveChannel { id } if *id == new_ret_id
    )));
    rebuilt
        .apply_transaction_validated(&reverse_edits)
        .expect("diff_project_edits reverse round-trip must succeed");
    assert_eq!(rebuilt.mixer, before.mixer);
}

#[test]
fn patch_score_sound_propagates_zone_compilation_errors() {
    let mut project = Project::demo();
    let score = project.compile().unwrap();
    // Corrupt zone 0 with a dangling cable referencing a non-existent module ID.
    project.instruments[0].rack.zones[0]
        .cables
        .push(PatchCable::new(999, 1, 999, InPort::In, 1.0));
    let err = project
        .patch_score_sound(&score, &LiveFlags::default())
        .expect_err("patch_score_sound must propagate compile_zones errors");
    assert!(
        format!("{err:#}").contains("999"),
        "error should identify dangling cable target, got: {err:#}"
    );
}

#[test]
fn reorder_lane_moves_lane_validates_bounds_and_round_trips_diff() {
    let before = Project::demo();
    assert!(before.lanes.len() >= 3);
    let mut after = before.clone();
    let lane_ids: Vec<Id> = before.lanes.iter().map(|l| l.id).collect();
    let last_id = *lane_ids.last().unwrap();

    after
        .apply(Edit::ReorderLane {
            lane: last_id,
            target_index: 0,
        })
        .unwrap();
    assert_eq!(after.lanes[0].id, last_id);
    assert_eq!(after.lanes[1].id, lane_ids[0]);

    assert!(
        after
            .apply(Edit::ReorderLane {
                lane: last_id,
                target_index: after.lanes.len(),
            })
            .is_err(),
        "out-of-range target_index must fail"
    );
    assert!(
        after
            .apply(Edit::ReorderLane {
                lane: 999_999,
                target_index: 0,
            })
            .is_err(),
        "missing lane id must fail"
    );

    let edits = crate::take::diff_project_edits(&before, &after);
    assert!(edits.iter().any(|e| matches!(
        e,
        Edit::ReorderLane {
            lane,
            target_index: 0,
        } if *lane == last_id
    )));
    let mut rebuilt = before.clone();
    rebuilt.apply_transaction_validated(&edits).unwrap();
    let rebuilt_ids: Vec<Id> = rebuilt.lanes.iter().map(|l| l.id).collect();
    let after_ids: Vec<Id> = after.lanes.iter().map(|l| l.id).collect();
    assert_eq!(rebuilt_ids, after_ids);
}
