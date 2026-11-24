//! Pure, deterministic musical pattern evaluation with fractional sub-step
//! timing (`offset ∈ [0.0, 1.0)`) and stateless seeded RNG.
//!
//! Offline rendering, the live ring window, and UI projection all evaluate
//! patterns through this module so they are guaranteed to agree 100%.

use crate::material::{Id, PatternValue, Project};
use crate::music::Step;
use anyhow::Result;

/// 64-bit golden-ratio Weyl sequence constant (`floor(2^64 / phi)`).
pub const GOLDEN_RATIO_64: u64 = 0x9e37_79b9_7f4a_7c15;
/// SplitMix64 mixing multiplier 1 (Stafford Mix13).
pub const SPLITMIX64_MUL1: u64 = 0xbf58_476d_1ce4_e5b9;
/// SplitMix64 mixing multiplier 2 (Stafford Mix13).
pub const SPLITMIX64_MUL2: u64 = 0x94d0_49bb_1331_11eb;
/// 64-bit salt mixing constant for independent hash domains.
const SPLITMIX64_SALT_MUL: u64 = 0x517c_c1b7_2722_0a95;

/// Pure stateless SplitMix64 finalizer (Stafford Mix13), mapping any `u64`
/// deterministically to a well-avalanched `u64`.
#[inline]
pub const fn splitmix64(mut v: u64) -> u64 {
    v = (v ^ (v >> 30)).wrapping_mul(SPLITMIX64_MUL1);
    v = (v ^ (v >> 27)).wrapping_mul(SPLITMIX64_MUL2);
    v ^ (v >> 31)
}

/// Pure stateless SplitMix64 hash mapping `(seed, pattern_id, step, salt)`
/// deterministically into `[0.0, 1.0)`. Zero mutable RNG state; safe to query
/// in any order (offline, live sliding window, or UI scroll).
#[inline]
pub fn det_hash(seed: u32, pattern_id: Id, step: i64, salt: u32) -> f32 {
    let mixed = u64::from(seed).wrapping_mul(GOLDEN_RATIO_64)
        ^ pattern_id.wrapping_mul(SPLITMIX64_MUL1)
        ^ (step as u64).wrapping_mul(SPLITMIX64_MUL2)
        ^ u64::from(salt).wrapping_mul(SPLITMIX64_SALT_MUL);
    ((splitmix64(mixed) >> 40) as f32) / 16_777_216.0
}

/// One evaluated musical note hit occurring inside a single 16th-note step.
#[derive(Clone, Debug, PartialEq)]
pub struct EvaluatedEvent {
    /// Fractional position within the 16th-note step: `0.0 <= offset < 1.0`.
    /// `0.0` fires on the exact step boundary; non-zero offsets drive sub-step
    /// swing, ratchets, triplets, time-scaling, and mini-notation subdivisions.
    pub offset: f32,
    /// Resolved note step (pitch degree/accidental, chord tones, velocity, gate, condition).
    pub step: Step,
}

/// Pure evaluation of any Notes-typed pattern at integer 16th-note `step`,
/// appending 0, 1, or many sub-step `EvaluatedEvent`s into `out` sorted by `offset`.
pub fn evaluate_notes_into(
    project: &Project,
    pattern_id: Id,
    step: i64,
    out: &mut Vec<EvaluatedEvent>,
) -> Result<()> {
    let start = out.len();
    project.evaluate_notes_events_into(pattern_id, step, out)?;
    if out.len() - start > 1 {
        out[start..].sort_by(|a, b| a.offset.total_cmp(&b.offset));
    }
    Ok(())
}

/// Convenience wrapper evaluating a pattern at one step and returning all sub-step events.
pub fn evaluate_notes_at(
    project: &Project,
    pattern_id: Id,
    step: i64,
) -> Result<Vec<EvaluatedEvent>> {
    let mut out = Vec::with_capacity(2);
    evaluate_notes_into(project, pattern_id, step, &mut out)?;
    Ok(out)
}

/// Evaluate any pattern to its primary step-boundary `PatternValue` (for UI grids & inspection).
pub fn evaluate_pattern_at(project: &Project, pattern_id: Id, step: i64) -> Result<PatternValue> {
    project.evaluate_pattern(pattern_id, step)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn det_hash_is_pure_bounded_and_well_distributed() {
        let mut sum = 0.0_f64;
        for step in -32..96 {
            let a = det_hash(42, 7, step, 0);
            let b = det_hash(42, 7, step, 0);
            assert_eq!(a, b);
            assert!((0.0..1.0).contains(&a));
            sum += f64::from(a);
        }
        let mean = sum / 128.0;
        assert!((0.35..=0.65).contains(&mean), "mean={mean}");
        assert_ne!(det_hash(42, 7, 0, 0), det_hash(43, 7, 0, 0));
        assert_ne!(det_hash(42, 7, 0, 0), det_hash(42, 8, 0, 0));
    }

    #[test]
    fn pattern_algebra_and_mini_notation_evaluate_and_compile_substeps() {
        use crate::material::{
            ArpMode, Pattern, PatternKind, PhaseMode, Placement, ValuePoint, ValueUnit,
        };
        use crate::music::ChordTone;
        use crate::params::ValueMode;

        let active_step = |degree: i16| Step {
            active: true,
            degree,
            velocity: 0.85,
            gate: 1.0,
            ..Step::default()
        };

        let mut p = Project::demo();
        let next_id = |p: &Project| p.max_id() + 1;

        // 1. Mini-notation with chords, subdivisions, ratchets, euclidean, alternation
        let mini_id = next_id(&p);
        p.patterns.push(Pattern {
            id: mini_id,
            name: "MiniHook".into(),
            kind: PatternKind::Mini {
                code: "c4 [e4 g4] ~ [c4,e4,g4]".into(),
                steps: 4,
            },
        });
        let s0 = evaluate_notes_at(&p, mini_id, 0).unwrap();
        assert_eq!(s0.len(), 1);
        assert!((s0[0].offset - 0.0).abs() < 1e-5);
        let s1 = evaluate_notes_at(&p, mini_id, 1).unwrap();
        assert_eq!(
            s1.len(),
            2,
            "subdivision [e4 g4] produces 2 sub-step events"
        );
        assert!((s1[0].offset - 0.0).abs() < 1e-5);
        assert!((s1[1].offset - 0.5).abs() < 1e-5);
        let s2 = evaluate_notes_at(&p, mini_id, 2).unwrap();
        assert!(s2.is_empty(), "rest ~ emits no event");
        let s3 = evaluate_notes_at(&p, mini_id, 3).unwrap();
        assert_eq!(s3.len(), 1);
        assert_eq!(
            s3[0].step.chord.len(),
            2,
            "triad [c4,e4,g4] has 2 chord tones"
        );

        // 2. Euclidean(3, 8, 0) -> classic tresillo [T, F, F, T, F, F, T, F]
        let euc_id = next_id(&p);
        p.patterns.push(Pattern {
            id: euc_id,
            name: "Tresillo".into(),
            kind: PatternKind::Euclidean {
                pulses: 3,
                steps: 8,
                rotation: 0,
            },
        });
        let hits: Vec<bool> = (0..8)
            .map(|t| evaluate_pattern_at(&p, euc_id, t).unwrap().active())
            .collect();
        assert_eq!(hits.iter().filter(|&&h| h).count(), 3);
        assert!(hits[0] && hits[3] && hits[6]);

        // 3. Base 4-step chord pattern + Arp (rate=2 -> 2 sub-steps per step!)
        let chord_id = next_id(&p);
        let mut chord_step = active_step(1);
        chord_step.gate = 4.0;
        chord_step.chord = vec![
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
        p.patterns.push(Pattern {
            id: chord_id,
            name: "HeldChord".into(),
            kind: PatternKind::Notes {
                steps: vec![
                    chord_step,
                    Step::default(),
                    Step::default(),
                    Step::default(),
                ],
            },
        });
        let arp_id = next_id(&p);
        p.patterns.push(Pattern {
            id: arp_id,
            name: "SparkleArp".into(),
            kind: PatternKind::Arp {
                source: chord_id,
                mode: ArpMode::UpDown,
                rate: 2,
            },
        });
        let arp0 = evaluate_notes_at(&p, arp_id, 0).unwrap();
        assert_eq!(arp0.len(), 2);
        assert!((arp0[0].offset - 0.0).abs() < 1e-5);
        assert!((arp0[1].offset - 0.5).abs() < 1e-5);
        assert_eq!(arp0[0].step.degree, 1);
        assert_eq!(arp0[1].step.degree, 3);
        let arp1 = evaluate_notes_at(&p, arp_id, 1).unwrap();
        assert_eq!(arp1[0].step.degree, 5);
        assert_eq!(arp1[1].step.degree, 3);

        // 4. TimeScale (3/2 triplet compression + 2/1 double-time)
        let fast_id = next_id(&p);
        p.patterns.push(Pattern {
            id: fast_id,
            name: "DoubleMini".into(),
            kind: PatternKind::TimeScale {
                source: chord_id,
                num: 2,
                den: 1,
            },
        });
        let f0 = evaluate_notes_at(&p, fast_id, 0).unwrap();
        assert_eq!(f0.len(), 1);
        assert!((f0[0].step.gate - 2.0).abs() < 1e-4);

        let tri_src = next_id(&p);
        p.patterns.push(Pattern {
            id: tri_src,
            name: "AllHits".into(),
            kind: PatternKind::Notes {
                steps: vec![active_step(1), active_step(2), active_step(3)],
            },
        });
        let tri_id = next_id(&p);
        p.patterns.push(Pattern {
            id: tri_id,
            name: "Triplets".into(),
            kind: PatternKind::TimeScale {
                source: tri_src,
                num: 3,
                den: 2,
            },
        });
        let t0 = evaluate_notes_at(&p, tri_id, 0).unwrap();
        let t1 = evaluate_notes_at(&p, tri_id, 1).unwrap();
        assert_eq!(t0.len() + t1.len(), 3, "3 notes packed into 2 steps");
        assert!((t0[1].offset - (2.0 / 3.0)).abs() < 1e-3);

        // 5. Swing delays odd steps cleanly
        let swing_id = next_id(&p);
        p.patterns.push(Pattern {
            id: swing_id,
            name: "Swung".into(),
            kind: PatternKind::Swing {
                source: tri_src,
                amount: 0.25,
            },
        });
        assert!((evaluate_notes_at(&p, swing_id, 0).unwrap()[0].offset - 0.0).abs() < 1e-5);
        assert!((evaluate_notes_at(&p, swing_id, 1).unwrap()[0].offset - 0.25).abs() < 1e-5);

        // 6. Ratchet splits 1 hit into 4 sub-steps
        let ratch_id = next_id(&p);
        p.patterns.push(Pattern {
            id: ratch_id,
            name: "SnareRoll".into(),
            kind: PatternKind::Ratchet {
                source: chord_id,
                count: 4,
            },
        });
        let r0 = evaluate_notes_at(&p, ratch_id, 0).unwrap();
        assert_eq!(r0.len(), 4);
        assert!((r0[3].offset - 0.75).abs() < 1e-5);

        // 7. Shift, Reverse, Chance, Every, RangeFold, Concat, Stack, Shape
        let shift_id = next_id(&p);
        p.patterns.push(Pattern {
            id: shift_id,
            name: "Shifted".into(),
            kind: PatternKind::Shift {
                source: tri_src,
                steps: 1,
            },
        });
        assert_eq!(
            evaluate_notes_at(&p, shift_id, 1).unwrap()[0].step.degree,
            1
        );

        let rev_id = next_id(&p);
        p.patterns.push(Pattern {
            id: rev_id,
            name: "Reversed".into(),
            kind: PatternKind::Reverse { source: tri_src },
        });
        assert_eq!(evaluate_notes_at(&p, rev_id, 0).unwrap()[0].step.degree, 3);
        assert_eq!(evaluate_notes_at(&p, rev_id, 2).unwrap()[0].step.degree, 1);

        let chance_id = next_id(&p);
        p.patterns.push(Pattern {
            id: chance_id,
            name: "GhostChance".into(),
            kind: PatternKind::Chance {
                source: tri_src,
                probability: 0.5,
                seed: 77,
            },
        });
        let a = evaluate_notes_at(&p, chance_id, 10).unwrap();
        let b = evaluate_notes_at(&p, chance_id, 10).unwrap();
        assert_eq!(a, b, "Chance evaluation must be deterministic");

        let every_id = next_id(&p);
        p.patterns.push(Pattern {
            id: every_id,
            name: "Every2".into(),
            kind: PatternKind::Every {
                interval: 2,
                base: tri_src,
                alt: rev_id,
            },
        });
        assert_eq!(
            evaluate_notes_at(&p, every_id, 0).unwrap()[0].step.degree,
            1
        );
        assert_eq!(
            evaluate_notes_at(&p, every_id, 3).unwrap()[0].step.degree,
            3
        );

        let fold_id = next_id(&p);
        p.patterns.push(Pattern {
            id: fold_id,
            name: "BassFold".into(),
            kind: PatternKind::RangeFold {
                source: tri_src,
                low_midi: 36,
                high_midi: 48,
            },
        });
        let folded = evaluate_notes_at(&p, fold_id, 0).unwrap()[0].step.clone();
        let folded_midi = p.pitch_midi(folded.degree, folded.accidental, 0);
        assert!((36..=48).contains(&folded_midi), "midi={folded_midi}");

        let concat_id = next_id(&p);
        p.patterns.push(Pattern {
            id: concat_id,
            name: "ConcatAB".into(),
            kind: PatternKind::Concat {
                parts: vec![tri_src, rev_id],
            },
        });
        assert_eq!(p.pattern_period(concat_id).unwrap(), 6);
        assert_eq!(
            evaluate_notes_at(&p, concat_id, 3).unwrap()[0].step.degree,
            3
        );

        let stack_id = next_id(&p);
        p.patterns.push(Pattern {
            id: stack_id,
            name: "Stacked".into(),
            kind: PatternKind::Stack {
                parts: vec![tri_src, fold_id],
            },
        });
        assert_eq!(evaluate_notes_at(&p, stack_id, 0).unwrap().len(), 2);

        let vel_curve = next_id(&p);
        p.patterns.push(Pattern {
            id: vel_curve,
            name: "RampVel".into(),
            kind: PatternKind::Values {
                unit: ValueUnit::Normalized,
                period: 4,
                mode: ValueMode::Ramp,
                points: vec![
                    ValuePoint {
                        step: 0,
                        value: 0.25,
                    },
                    ValuePoint {
                        step: 3,
                        value: 1.0,
                    },
                ],
            },
        });
        let shape_id = next_id(&p);
        p.patterns.push(Pattern {
            id: shape_id,
            name: "Shaped".into(),
            kind: PatternKind::Shape {
                source: tri_src,
                velocity: vel_curve,
                gate: Some(vel_curve),
            },
        });
        let sh0 = evaluate_notes_at(&p, shape_id, 0).unwrap();
        let sh3 = evaluate_notes_at(&p, shape_id, 3).unwrap();
        assert!(sh0[0].step.velocity < sh3[0].step.velocity);

        // Compile the whole project with a part playing `arp_id` and verify sub-step events!
        p.parts[0].pattern = arp_id;
        p.lanes[0].placements = vec![Placement {
            id: next_id(&p),
            part: p.parts[0].id,
            start_step: 0,
            length_steps: 16,
            phase: PhaseMode::default(),
            offset_steps: 0,
            exceptions: vec![],
        }];
        let score = p.compile().unwrap();
        let sub_events: Vec<_> = score
            .step_events(0)
            .iter()
            .filter(|e| e.offset > 0.4)
            .collect();
        assert!(
            !sub_events.is_empty(),
            "Compiled ScoreWindow should preserve fractional sub-step Arp offsets"
        );
    }
}
