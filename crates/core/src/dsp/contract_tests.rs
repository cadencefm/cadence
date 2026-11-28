//! Exhaustive, type-driven contract suite for all [`StereoEffect`] processors and
//! every nested operating mode (`FilterMode`, `DriveMode`, `DelayMode`).
//!
//! Uses exhaustive `match` factories over `::ALL` discriminant arrays (zero wildcard
//! fallbacks) with non-neutral, fully-engaged `(preset_a, preset_b)` parameter pairs
//! so adding a new effect or operating mode fails to compile until covered across
//! every invariant.

use super::*;
use crate::filter_patterns::{CompiledFilterPattern, MAX_FILTER_ENV_STEPS};
use crate::mixer::{
    ChorusPatch, CompressorPatch, DelayMode, DelayPatch, DriveMode, DrivePatch, EffectKindTag,
    EqPatch, FilterPatch, LimiterPatch, ReverbPatch,
};
use crate::rack::FilterMode;
use std::f64::consts::TAU;

const CONTRACT_SAMPLE_RATES: [f64; 2] = [48_000.0, 96_000.0];

enum ContractEffect {
    Delay(Box<Delay>),
    Reverb(Box<Reverb>),
    Eq(Box<Eq>),
    Chorus(Box<Chorus>),
    Drive(Box<Drive>),
    Filter(Box<RhythmicFilter>),
    Compressor(Box<Compressor>),
    Limiter(Box<Limiter>),
}

macro_rules! dispatch_effect {
    ($self:expr, $fx:ident => $body:expr) => {
        match $self {
            Self::Delay($fx) => $body,
            Self::Reverb($fx) => $body,
            Self::Eq($fx) => $body,
            Self::Chorus($fx) => $body,
            Self::Drive($fx) => $body,
            Self::Filter($fx) => $body,
            Self::Compressor($fx) => $body,
            Self::Limiter($fx) => $body,
        }
    };
}

impl StereoEffect for ContractEffect {
    fn process(&mut self, input: [f32; 2], bypassed: bool) -> [f32; 2] {
        dispatch_effect!(self, fx => fx.process(input, bypassed))
    }

    fn process_block(&mut self, buf: &mut [[f32; 2]], bypassed: bool) {
        dispatch_effect!(self, fx => fx.process_block(buf, bypassed))
    }

    fn process_block_with_gr(
        &mut self,
        buf: &mut [[f32; 2]],
        bypassed: bool,
        gr_out: &mut [f32],
    ) -> bool {
        dispatch_effect!(self, fx => fx.process_block_with_gr(buf, bypassed, gr_out))
    }

    fn reset(&mut self) {
        dispatch_effect!(self, fx => fx.reset())
    }

    fn latency_frames(&self) -> usize {
        dispatch_effect!(self, fx => fx.latency_frames())
    }

    fn gain_reduction_db(&self) -> Option<f32> {
        dispatch_effect!(self, fx => StereoEffect::gain_reduction_db(fx.as_ref()))
    }

    fn is_quiet(&self, bypassed: bool) -> bool {
        dispatch_effect!(self, fx => fx.is_quiet(bypassed))
    }

    fn advance_silent_frames(&mut self, frames: usize, bypassed: bool) {
        dispatch_effect!(self, fx => fx.advance_silent_frames(frames, bypassed))
    }
}

#[derive(Clone, Copy)]
enum EffectPreset {
    Delay {
        patch: DelayPatch,
        bpm: f64,
    },
    Reverb(ReverbPatch),
    Eq(EqPatch),
    Chorus(ChorusPatch),
    Drive(DrivePatch),
    Filter {
        patch: FilterPatch,
        pattern: CompiledFilterPattern,
        bpm: f64,
    },
    Compressor(CompressorPatch),
    Limiter(LimiterPatch),
}

impl ContractEffect {
    fn apply(&mut self, preset: EffectPreset, song_pos: f64) {
        match (self, preset) {
            (Self::Delay(fx), EffectPreset::Delay { patch, bpm }) => fx.set_parameters(patch, bpm),
            (Self::Reverb(fx), EffectPreset::Reverb(patch)) => fx.set_parameters(patch),
            (Self::Eq(fx), EffectPreset::Eq(patch)) => fx.set_parameters(patch),
            (Self::Chorus(fx), EffectPreset::Chorus(patch)) => fx.set_parameters(patch),
            (Self::Drive(fx), EffectPreset::Drive(patch)) => fx.set_parameters(patch),
            (
                Self::Filter(fx),
                EffectPreset::Filter {
                    patch,
                    pattern,
                    bpm,
                },
            ) => {
                if patch.env_amount.abs() > 1e-4 {
                    fx.set_parameters_with_clock(patch, pattern, song_pos, bpm);
                } else {
                    fx.set_parameters(patch);
                }
            }
            (Self::Compressor(fx), EffectPreset::Compressor(patch)) => fx.set_parameters(patch),
            (Self::Limiter(fx), EffectPreset::Limiter(patch)) => fx.set_parameters(patch),
            _ => unreachable!("mismatched ContractEffect and EffectPreset"),
        }
    }
}

struct EffectCase {
    name: String,
    tag: EffectKindTag,
    preset_a: EffectPreset,
    preset_b: EffectPreset,
    max_drain_sec: f64,
}

impl EffectCase {
    fn build(&self, sr: f64) -> ContractEffect {
        let sr_u32 = sr as u32;
        match self.tag {
            EffectKindTag::Delay => {
                ContractEffect::Delay(Box::new(Delay::with_max_seconds(sr_u32, 1.5)))
            }
            EffectKindTag::Reverb => ContractEffect::Reverb(Box::new(Reverb::new(sr_u32))),
            EffectKindTag::Eq => ContractEffect::Eq(Box::new(Eq::new(sr))),
            EffectKindTag::Chorus => ContractEffect::Chorus(Box::new(Chorus::new(sr))),
            EffectKindTag::Drive => ContractEffect::Drive(Box::new(Drive::new(sr))),
            EffectKindTag::Filter => ContractEffect::Filter(Box::new(RhythmicFilter::new(sr))),
            EffectKindTag::Compressor => ContractEffect::Compressor(Box::new(Compressor::new(sr))),
            EffectKindTag::Limiter => ContractEffect::Limiter(Box::new(Limiter::new(sr))),
        }
    }
}

fn filter_pattern(smooth_attack: bool) -> CompiledFilterPattern {
    let mut steps = [0u8; MAX_FILTER_ENV_STEPS];
    steps[0] = 127;
    steps[2] = 96;
    steps[5] = 115;
    steps[8] = 120;
    steps[11] = 88;
    steps[14] = 127;
    CompiledFilterPattern {
        resolution: 16,
        length: 16,
        smooth_attack,
        steps,
    }
}

fn alternate_filter_mode(mode: FilterMode) -> FilterMode {
    match mode {
        FilterMode::ImLpf2 => FilterMode::DiodeLadder,
        FilterMode::ImLpf3 => FilterMode::ImLpf4,
        FilterMode::ImLpf4 => FilterMode::BandPass,
        FilterMode::DiodeLadder => FilterMode::ImLpf2,
        FilterMode::BandPass => FilterMode::HighPass,
        FilterMode::HighPass => FilterMode::LowPass6,
        FilterMode::LowPass6 => FilterMode::ImLpf3,
    }
}

fn alternate_drive_mode(mode: DriveMode) -> DriveMode {
    match mode {
        DriveMode::AtomicJet => DriveMode::ShockPort,
        DriveMode::ShockPort => DriveMode::AtomicJet,
    }
}

fn alternate_delay_mode(mode: DelayMode) -> DelayMode {
    match mode {
        DelayMode::PingPong => DelayMode::Stereo,
        DelayMode::Stereo => DelayMode::PingPong,
    }
}

/// Exhaustively enumerate every [`EffectKindTag`], [`FilterMode`], [`DriveMode`],
/// and [`DelayMode`] with non-neutral, fully-engaged `(preset_a, preset_b)` pairs.
fn all_effect_cases() -> Vec<EffectCase> {
    let mut cases = Vec::new();
    for &tag in EffectKindTag::ALL {
        match tag {
            EffectKindTag::Delay => {
                for mode in DelayMode::ALL {
                    let other = alternate_delay_mode(mode);
                    cases.push(EffectCase {
                        name: format!("Delay::{mode:?}"),
                        tag,
                        preset_a: EffectPreset::Delay {
                            patch: DelayPatch {
                                mode,
                                steps: 0.25,
                                feedback: 0.45,
                                mix: 0.65,
                            },
                            bpm: 132.0,
                        },
                        preset_b: EffectPreset::Delay {
                            patch: DelayPatch {
                                mode: other,
                                steps: 0.5,
                                feedback: 0.28,
                                mix: 0.85,
                            },
                            bpm: 118.0,
                        },
                        max_drain_sec: 4.0,
                    });
                }
            }
            EffectKindTag::Reverb => {
                cases.push(EffectCase {
                    name: "Reverb".into(),
                    tag,
                    preset_a: EffectPreset::Reverb(ReverbPatch {
                        size: 0.35,
                        decay_seconds: 0.25,
                        damping_hz: 6_000.0,
                        diffusion: 0.35,
                        mix: 0.6,
                        width: 0.85,
                    }),
                    preset_b: EffectPreset::Reverb(ReverbPatch {
                        size: 0.72,
                        decay_seconds: 0.45,
                        damping_hz: 11_000.0,
                        diffusion: 0.85,
                        mix: 0.9,
                        width: 0.25,
                    }),
                    max_drain_sec: 5.0,
                });
            }
            EffectKindTag::Eq => {
                cases.push(EffectCase {
                    name: "Eq".into(),
                    tag,
                    preset_a: EffectPreset::Eq(EqPatch {
                        hpf_hz: 95.0,
                        low_db: 4.5,
                        mid_db: -5.0,
                        mid_hz: 900.0,
                        mid_q: 2.2,
                        high_db: 3.5,
                    }),
                    preset_b: EffectPreset::Eq(EqPatch {
                        hpf_hz: 180.0,
                        low_db: -3.5,
                        mid_db: 6.0,
                        mid_hz: 2_600.0,
                        mid_q: 1.1,
                        high_db: -4.0,
                    }),
                    max_drain_sec: 1.5,
                });
            }
            EffectKindTag::Chorus => {
                cases.push(EffectCase {
                    name: "Chorus".into(),
                    tag,
                    preset_a: EffectPreset::Chorus(ChorusPatch {
                        rate_hz: 1.25,
                        depth: 0.65,
                        width: 0.95,
                        mix: 0.6,
                    }),
                    preset_b: EffectPreset::Chorus(ChorusPatch {
                        rate_hz: 3.4,
                        depth: 0.35,
                        width: 0.3,
                        mix: 0.85,
                    }),
                    max_drain_sec: 1.5,
                });
            }
            EffectKindTag::Drive => {
                for mode in DriveMode::ALL {
                    let other = alternate_drive_mode(mode);
                    cases.push(EffectCase {
                        name: format!("Drive::{mode:?}"),
                        tag,
                        preset_a: EffectPreset::Drive(DrivePatch {
                            mode,
                            drive: 0.68,
                            asymmetry: 0.35,
                            tone_hz: 5_200.0,
                            output_db: -1.5,
                            mix: 0.75,
                        }),
                        preset_b: EffectPreset::Drive(DrivePatch {
                            mode: other,
                            drive: 0.38,
                            asymmetry: -0.45,
                            tone_hz: 11_500.0,
                            output_db: 2.0,
                            mix: 0.95,
                        }),
                        max_drain_sec: 1.5,
                    });
                }
            }
            EffectKindTag::Filter => {
                for &mode in FilterMode::ALL {
                    let other = alternate_filter_mode(mode);
                    // Static (non-clocked) preset pair with non-zero resonance, drive, and mode switch.
                    cases.push(EffectCase {
                        name: format!("Filter::{mode:?}::static"),
                        tag,
                        preset_a: EffectPreset::Filter {
                            patch: FilterPatch {
                                mode,
                                cutoff_hz: 620.0,
                                resonance: 0.58,
                                drive: 0.45,
                                kerr_semitones: 4.5,
                                spread: 0.55,
                                mix: 0.8,
                                env_pattern: 0,
                                env_pattern_id: None,
                                env_amount: 0.0,
                                env_decay_ms: 140.0,
                            },
                            pattern: CompiledFilterPattern::neutral(),
                            bpm: 124.0,
                        },
                        preset_b: EffectPreset::Filter {
                            patch: FilterPatch {
                                mode: other,
                                cutoff_hz: 2_800.0,
                                resonance: 0.32,
                                drive: 0.15,
                                kerr_semitones: -3.5,
                                spread: 0.25,
                                mix: 1.0,
                                env_pattern: 0,
                                env_pattern_id: None,
                                env_amount: 0.0,
                                env_decay_ms: 260.0,
                            },
                            pattern: CompiledFilterPattern::neutral(),
                            bpm: 124.0,
                        },
                        max_drain_sec: 1.5,
                    });

                    // Clocked rhythmic-envelope preset pair with active bipolar octave sweeps.
                    cases.push(EffectCase {
                        name: format!("Filter::{mode:?}::clocked_env"),
                        tag,
                        preset_a: EffectPreset::Filter {
                            patch: FilterPatch {
                                mode,
                                cutoff_hz: 420.0,
                                resonance: 0.62,
                                drive: 0.35,
                                kerr_semitones: -5.0,
                                spread: 0.7,
                                mix: 0.9,
                                env_pattern: 1,
                                env_pattern_id: None,
                                env_amount: 3.0,
                                env_decay_ms: 45.0,
                            },
                            pattern: filter_pattern(false),
                            bpm: 128.0,
                        },
                        preset_b: EffectPreset::Filter {
                            patch: FilterPatch {
                                mode: other,
                                cutoff_hz: 3_200.0,
                                resonance: 0.4,
                                drive: 0.6,
                                kerr_semitones: 6.0,
                                spread: 0.3,
                                mix: 0.75,
                                env_pattern: 2,
                                env_pattern_id: None,
                                env_amount: -2.2,
                                env_decay_ms: 75.0,
                            },
                            pattern: filter_pattern(true),
                            bpm: 128.0,
                        },
                        max_drain_sec: 1.5,
                    });
                }
            }
            EffectKindTag::Compressor => {
                cases.push(EffectCase {
                    name: "Compressor".into(),
                    tag,
                    preset_a: EffectPreset::Compressor(CompressorPatch {
                        threshold_db: -22.0,
                        ratio: 8.0,
                        attack_ms: 1.0,
                        release_ms: 35.0,
                        makeup_db: 3.5,
                        mix: 0.85,
                    }),
                    preset_b: EffectPreset::Compressor(CompressorPatch {
                        threshold_db: -12.0,
                        ratio: 3.5,
                        attack_ms: 8.0,
                        release_ms: 75.0,
                        makeup_db: 1.0,
                        mix: 1.0,
                    }),
                    max_drain_sec: 2.0,
                });
            }
            EffectKindTag::Limiter => {
                cases.push(EffectCase {
                    name: "Limiter".into(),
                    tag,
                    preset_a: EffectPreset::Limiter(LimiterPatch {
                        ceiling_db: -4.5,
                        input_drive_db: 9.0,
                        release_ms: 35.0,
                        mix: 0.9,
                    }),
                    preset_b: EffectPreset::Limiter(LimiterPatch {
                        ceiling_db: -1.5,
                        input_drive_db: 3.0,
                        release_ms: 80.0,
                        mix: 1.0,
                    }),
                    max_drain_sec: 2.0,
                });
            }
        }
    }
    cases
}

fn stereo_stimulus(frames: usize, sr: f64, amp: f32) -> Vec<[f32; 2]> {
    (0..frames)
        .map(|i| {
            let t = i as f64 / sr;
            let l = ((TAU * 220.0 * t).sin() * 0.6 + (TAU * 1_170.0 * t).sin() * 0.4) as f32 * amp;
            let r =
                ((TAU * 330.0 * t).cos() * 0.55 + (TAU * 1_780.0 * t).sin() * 0.45) as f32 * amp;
            [l, r]
        })
        .collect()
}

fn step_clock(frame: usize, sr: f64, bpm: f64) -> f64 {
    frame as f64 * bpm / (15.0 * sr)
}

#[test]
fn all_effects_and_modes_satisfy_block_parity_across_bypass_and_param_swaps() {
    let cases = all_effect_cases();
    for &sr in &CONTRACT_SAMPLE_RATES {
        let fade_frames = (sr * 0.025) as usize;
        // Multi-stage scenario exercising fresh idle-bypass, un-bypass, active audio,
        // mid-stream preset+mode swap, active->bypass fade to idle, preset+mode swap
        // while idle-bypassed, and un-bypass back to active audio.
        let stages: &[(usize, bool, bool)] = &[
            (192, true, false),              // initial idle-bypassed with preset A
            (256, false, false),             // un-bypass fade-in + active preset A
            (256, false, true),              // swap A -> B while active
            (fade_frames + 128, true, true), // bypass fade-out + settle to idle
            (256, true, false),              // swap B -> A WHILE idle-bypassed!
            (256, false, false),             // un-bypass back into active audio
        ];
        let total_frames: usize = stages.iter().map(|s| s.0).sum();
        let stimulus = stereo_stimulus(total_frames, sr, 0.85);

        for case in &cases {
            for &block_size in &[1usize, 7, 13, AUDIO_SUB_BLOCK_FRAMES] {
                let mut frame_fx = case.build(sr);
                let mut block_fx = case.build(sr);
                let mut gr_buf = vec![0.0_f32; block_size];
                let mut cursor = 0usize;

                for &(len, bypassed, use_b) in stages {
                    let preset = if use_b { case.preset_b } else { case.preset_a };
                    let pos = step_clock(cursor, sr, 128.0);
                    frame_fx.apply(preset, pos);
                    block_fx.apply(preset, pos);

                    let stage_end = cursor + len;
                    while cursor < stage_end {
                        let n = (stage_end - cursor).min(block_size);
                        let pos = step_clock(cursor, sr, 128.0);
                        // Periodic clock sync matching engine control cadence for rhythmic filters.
                        frame_fx.apply(preset, pos);
                        block_fx.apply(preset, pos);

                        let mut ref_out = Vec::with_capacity(n);
                        let mut ref_gr = Vec::with_capacity(n);
                        for &inp in &stimulus[cursor..cursor + n] {
                            ref_out.push(frame_fx.process(inp, bypassed));
                            ref_gr.push(frame_fx.gain_reduction_db().unwrap_or(0.0));
                        }

                        let mut blk_out = stimulus[cursor..cursor + n].to_vec();
                        let has_gr = block_fx.process_block_with_gr(
                            &mut blk_out,
                            bypassed,
                            &mut gr_buf[..n],
                        );

                        for i in 0..n {
                            assert_eq!(
                                [blk_out[i][0].to_bits(), blk_out[i][1].to_bits()],
                                [ref_out[i][0].to_bits(), ref_out[i][1].to_bits()],
                                "{} sr={sr} block={block_size} frame={} bypassed={bypassed}: audio mismatch {:?} vs {:?}",
                                case.name,
                                cursor + i,
                                blk_out[i],
                                ref_out[i]
                            );
                            if has_gr {
                                assert_eq!(
                                    gr_buf[i].to_bits(),
                                    ref_gr[i].to_bits(),
                                    "{} sr={sr} block={block_size} frame={}: GR mismatch {} vs {}",
                                    case.name,
                                    cursor + i,
                                    gr_buf[i],
                                    ref_gr[i]
                                );
                            }
                        }
                        cursor += n;
                    }
                }
            }
        }
    }
}

#[test]
fn all_effects_and_modes_satisfy_silent_sleep_equivalence_under_param_mutations() {
    let cases = all_effect_cases();
    for &sr in &CONTRACT_SAMPLE_RATES {
        let burst = stereo_stimulus(128, sr, 0.9);
        let wake = stereo_stimulus(384, sr, 0.75);

        for case in &cases {
            // 1. Fresh-instance sleep (active AND bypassed) across parameter mutations.
            for &sleep_bypassed in &[false, true] {
                let mut awake = case.build(sr);
                let mut sleeper = case.build(sr);
                awake.apply(case.preset_a, 0.0);
                sleeper.apply(case.preset_a, 0.0);
                assert!(
                    sleeper.is_quiet(sleep_bypassed),
                    "{} sr={sr}: fresh instance must be quiet (bypassed={sleep_bypassed})",
                    case.name
                );

                let mut frame = 0usize;
                for (step_idx, &(n, use_b)) in [
                    (64usize, false),
                    (13, true),
                    (64, true),
                    (19, false),
                    (64, false),
                ]
                .iter()
                .enumerate()
                {
                    let preset = if use_b { case.preset_b } else { case.preset_a };
                    let pos = step_clock(frame, sr, 128.0);
                    awake.apply(preset, pos);
                    sleeper.apply(preset, pos);
                    assert!(
                        sleeper.is_quiet(sleep_bypassed),
                        "{} sr={sr} fresh sleep step {step_idx} bypassed={sleep_bypassed}: expected quiet",
                        case.name
                    );
                    let mut zeros = vec![[0.0_f32; 2]; n];
                    awake.process_block(&mut zeros, sleep_bypassed);
                    sleeper.advance_silent_frames(n, sleep_bypassed);
                    frame += n;
                }

                // Wake both with identical audio (active) and assert tight frame-by-frame parity.
                let pos = step_clock(frame, sr, 128.0);
                awake.apply(case.preset_a, pos);
                sleeper.apply(case.preset_a, pos);
                let mut out_a = wake.clone();
                let mut out_s = wake.clone();
                awake.process_block(&mut out_a, false);
                sleeper.process_block(&mut out_s, false);
                for (i, (&a, &s)) in out_a.iter().zip(out_s.iter()).enumerate() {
                    let err = (a[0] - s[0]).abs().max((a[1] - s[1]).abs());
                    assert!(
                        err < 1e-5,
                        "{} sr={sr} fresh sleep(bypassed={sleep_bypassed}) wake frame {i}: awake={a:?} sleeper={s:?} err={err}",
                        case.name
                    );
                }
            }

            // 2. Post-excitation drain -> sleep across A->B->A mutations & odd/even chunks -> wake.
            let mut awake = case.build(sr);
            let mut sleeper = case.build(sr);
            awake.apply(case.preset_a, 0.0);
            sleeper.apply(case.preset_a, 0.0);

            let mut burst_a = burst.clone();
            let mut burst_s = burst.clone();
            awake.process_block(&mut burst_a, false);
            sleeper.process_block(&mut burst_s, false);
            assert!(
                !sleeper.is_quiet(false),
                "{} sr={sr}: must not report quiet immediately after loud burst",
                case.name
            );

            // Drain both in lockstep until sleeper reports quiet.
            let max_drain = (sr * case.max_drain_sec) as usize;
            let mut frame = burst.len();
            let mut drained = false;
            while frame < burst.len() + max_drain {
                let pos = step_clock(frame, sr, 128.0);
                awake.apply(case.preset_a, pos);
                sleeper.apply(case.preset_a, pos);
                let mut z_a = [[0.0_f32; 2]; AUDIO_SUB_BLOCK_FRAMES];
                let mut z_s = [[0.0_f32; 2]; AUDIO_SUB_BLOCK_FRAMES];
                awake.process_block(&mut z_a, false);
                sleeper.process_block(&mut z_s, false);
                frame += AUDIO_SUB_BLOCK_FRAMES;
                if sleeper.is_quiet(false) {
                    let tail_peak = z_s
                        .iter()
                        .map(|f| f[0].abs().max(f[1].abs()))
                        .fold(0.0_f32, f32::max);
                    assert!(
                        tail_peak < 1e-5,
                        "{} sr={sr}: reported quiet while still emitting tail {tail_peak}",
                        case.name
                    );
                    drained = true;
                    break;
                }
            }
            assert!(
                drained,
                "{} sr={sr}: failed to drain to is_quiet within {:.1}s",
                case.name, case.max_drain_sec
            );

            // Sleep across mixed chunk sizes while mutating parameters A -> B -> A.
            for &(n, use_b) in &[
                (1usize, true),
                (7, true),
                (13, true),
                (64, true),
                (192, true),
                (11, false),
                (64, false),
            ] {
                let preset = if use_b { case.preset_b } else { case.preset_a };
                let pos = step_clock(frame, sr, 128.0);
                awake.apply(preset, pos);
                sleeper.apply(preset, pos);
                assert!(
                    sleeper.is_quiet(false),
                    "{} sr={sr}: must stay quiet across silent param mutations",
                    case.name
                );
                let mut zeros = vec![[0.0_f32; 2]; n];
                awake.process_block(&mut zeros, false);
                sleeper.advance_silent_frames(n, false);
                frame += n;
            }

            // Wake both with identical audio and assert audio + GR parity.
            let pos = step_clock(frame, sr, 128.0);
            awake.apply(case.preset_a, pos);
            sleeper.apply(case.preset_a, pos);
            let mut out_a = wake.clone();
            let mut out_s = wake.clone();
            let mut gr_a = vec![0.0_f32; wake.len()];
            let mut gr_s = vec![0.0_f32; wake.len()];
            let has_gr_a = awake.process_block_with_gr(&mut out_a, false, &mut gr_a);
            let has_gr_s = sleeper.process_block_with_gr(&mut out_s, false, &mut gr_s);
            assert_eq!(has_gr_a, has_gr_s);
            for i in 0..wake.len() {
                let err = (out_a[i][0] - out_s[i][0])
                    .abs()
                    .max((out_a[i][1] - out_s[i][1]).abs());
                assert!(
                    err < 1e-5,
                    "{} sr={sr} post-drain wake frame {i}: awake={:?} sleeper={:?} err={err}",
                    case.name,
                    out_a[i],
                    out_s[i]
                );
                if has_gr_a {
                    assert!(
                        (gr_a[i] - gr_s[i]).abs() < 1e-4,
                        "{} sr={sr} post-drain wake GR frame {i}: awake={} sleeper={}",
                        case.name,
                        gr_a[i],
                        gr_s[i]
                    );
                }
            }
        }
    }
}

#[test]
fn all_effects_and_modes_advance_parameters_while_idle_bypassed() {
    let cases = all_effect_cases();
    for &sr in &CONTRACT_SAMPLE_RATES {
        let burst = stereo_stimulus(256, sr, 0.85);
        let probe = stereo_stimulus(256, sr, 0.75);
        let settle_frames = (sr * 0.45) as usize;

        for case in &cases {
            // Mutate A -> B while idle-bypassed, wait for smoothers to settle while bypassed,
            // then un-bypass and verify behavior matches a peer already settled on B before un-bypassing.
            let mut mutated = case.build(sr);
            let mut baseline = case.build(sr);

            mutated.apply(case.preset_a, 0.0);
            baseline.apply(case.preset_b, 0.0);

            // Excite both so internal buffers were previously active, then bypass until idle.
            let mut b1 = burst.clone();
            let mut b2 = burst.clone();
            mutated.process_block(&mut b1, false);
            baseline.process_block(&mut b2, false);

            let max_drain = (sr * case.max_drain_sec) as usize;
            let mut frame = burst.len();
            let mut both_idle = false;
            while frame < burst.len() + max_drain {
                let pos = step_clock(frame, sr, 128.0);
                mutated.apply(case.preset_a, pos);
                baseline.apply(case.preset_b, pos);
                let mut z1 = [[0.0_f32; 2]; AUDIO_SUB_BLOCK_FRAMES];
                let mut z2 = [[0.0_f32; 2]; AUDIO_SUB_BLOCK_FRAMES];
                mutated.process_block(&mut z1, true);
                baseline.process_block(&mut z2, true);
                frame += AUDIO_SUB_BLOCK_FRAMES;
                if mutated.is_quiet(true) && baseline.is_quiet(true) {
                    both_idle = true;
                    break;
                }
            }
            assert!(
                both_idle,
                "{} sr={sr}: bypassed effects failed to settle to idle",
                case.name
            );

            // Now mutate `mutated` from A -> B while idle-bypassed and feed live dry audio
            // so smoothers have time to converge to B while dry audio passes cleanly.
            let mut rem = settle_frames;
            let mut dry_idx = 0usize;
            while rem > 0 {
                let n = rem.min(AUDIO_SUB_BLOCK_FRAMES);
                let pos = step_clock(frame, sr, 128.0);
                mutated.apply(case.preset_b, pos);
                baseline.apply(case.preset_b, pos);

                let dry: Vec<[f32; 2]> = (0..n)
                    .map(|k| {
                        let t = (dry_idx + k) as f64 / sr;
                        let s = (TAU * 310.0 * t).sin() as f32 * 0.4;
                        [s, -s]
                    })
                    .collect();
                let mut d1 = dry.clone();
                let mut d2 = dry.clone();
                mutated.process_block(&mut d1, true);
                baseline.process_block(&mut d2, true);
                if case.tag != EffectKindTag::Limiter {
                    assert_eq!(
                        d1, dry,
                        "{} sr={sr}: idle-bypass must pass exact dry",
                        case.name
                    );
                }
                assert_eq!(
                    d1, d2,
                    "{} sr={sr}: bypassed dry output diverged",
                    case.name
                );
                frame += n;
                dry_idx += n;
                rem -= n;
            }

            // Flush Limiter lookahead ring with silence before un-bypassing so audio starts clean,
            // then compare un-bypassed output of `mutated` vs `baseline`.
            if case.tag == EffectKindTag::Limiter {
                let mut flush1 = vec![[0.0_f32; 2]; mutated.latency_frames() + 8];
                let mut flush2 = flush1.clone();
                mutated.process_block(&mut flush1, true);
                baseline.process_block(&mut flush2, true);
                frame += flush1.len();
            }

            // Skip Chorus phase comparison across different initial LFO rates before bypass;
            // verify Chorus smoothers converged by checking finite & clean un-bypass separately.
            let pos = step_clock(frame, sr, 128.0);
            mutated.apply(case.preset_b, pos);
            baseline.apply(case.preset_b, pos);
            let mut out_m = probe.clone();
            let mut out_b = probe.clone();
            mutated.process_block(&mut out_m, false);
            baseline.process_block(&mut out_b, false);
            if case.tag == EffectKindTag::Chorus {
                // Before the earliest ~2 ms chorus delay tap arrives, wet is 0.0 so output
                // depends strictly on converged `mix` smoother (independent of pre-bypass LFO phase).
                let check_len = (sr * 0.0015) as usize;
                for (i, (&m, &b)) in out_m[..check_len]
                    .iter()
                    .zip(out_b[..check_len].iter())
                    .enumerate()
                {
                    let err = (m[0] - b[0]).abs().max((m[1] - b[1]).abs());
                    assert!(
                        err < 1e-4,
                        "{} sr={sr} post-idle-bypass Chorus smoother frame {i}: mutated={m:?} baseline={b:?} err={err}",
                        case.name
                    );
                }
                for &f in &out_m {
                    assert!(f[0].is_finite() && f[1].is_finite());
                }
            } else {
                for (i, (&m, &b)) in out_m.iter().zip(out_b.iter()).enumerate() {
                    let err = (m[0] - b[0]).abs().max((m[1] - b[1]).abs());
                    assert!(
                        err < 1e-4,
                        "{} sr={sr} post-idle-bypass un-bypass frame {i}: mutated={m:?} baseline={b:?} err={err}",
                        case.name
                    );
                }
            }
        }
    }
}

#[test]
fn all_effects_and_modes_satisfy_reset_determinism_hygiene_and_gr_contracts() {
    let cases = all_effect_cases();
    for &sr in &CONTRACT_SAMPLE_RATES {
        let stimulus = stereo_stimulus(256, sr, 0.9);
        let pathological: [[f32; 2]; 10] = [
            [f32::NAN, 0.5],
            [-0.5, f32::NAN],
            [f32::INFINITY, f32::NEG_INFINITY],
            [f32::NEG_INFINITY, f32::INFINITY],
            [1e-38, -1e-38],
            [16.0, -16.0], // +24 dBFS overdrive
            [-16.0, 16.0],
            [f32::MIN_POSITIVE * 0.25, -f32::MIN_POSITIVE * 0.25],
            [0.0, 0.0],
            [0.25, -0.25],
        ];

        for case in &cases {
            // 1. Reset determinism: dirty effect after reset() == fresh effect bit-for-bit.
            let mut dirty = case.build(sr);
            dirty.apply(case.preset_a, 0.0);
            let mut warm = stimulus.clone();
            dirty.process_block(&mut warm, false);
            dirty.apply(case.preset_b, 1.5);
            dirty.process_block(&mut warm, true);
            dirty.reset();
            dirty.apply(case.preset_b, 0.0);

            let mut fresh = case.build(sr);
            fresh.apply(case.preset_b, 0.0);

            let mut out_dirty = stimulus.clone();
            let mut out_fresh = stimulus.clone();
            dirty.process_block(&mut out_dirty, false);
            fresh.process_block(&mut out_fresh, false);
            for (i, (&d, &f)) in out_dirty.iter().zip(out_fresh.iter()).enumerate() {
                assert_eq!(
                    [d[0].to_bits(), d[1].to_bits()],
                    [f[0].to_bits(), f[1].to_bits()],
                    "{} sr={sr} reset determinism mismatch at frame {i}: dirty={d:?} fresh={f:?}",
                    case.name
                );
            }

            // 2. NaN / ±Inf / subnormal / +24 dBFS hygiene (both active and bypassed).
            let mut hygiene = case.build(sr);
            hygiene.apply(case.preset_a, 0.0);
            for &bypassed in &[false, true, false] {
                let mut buf = pathological;
                hygiene.process_block(&mut buf, bypassed);
                for (i, &frame) in buf.iter().enumerate() {
                    assert!(
                        frame[0].is_finite() && frame[1].is_finite(),
                        "{} sr={sr} bypassed={bypassed} pathological frame {i} emitted non-finite {frame:?}",
                        case.name
                    );
                }
            }
            let mut recovery = stimulus.clone();
            hygiene.process_block(&mut recovery, false);
            for (i, &frame) in recovery.iter().enumerate() {
                assert!(
                    frame[0].is_finite()
                        && frame[1].is_finite()
                        && frame[0].abs() < 64.0
                        && frame[1].abs() < 64.0,
                    "{} sr={sr} recovery frame {i} blew up: {frame:?}",
                    case.name
                );
            }

            // 3. Latency & dynamics GR contract.
            let mut fx = case.build(sr);
            fx.apply(case.preset_a, 0.0);
            let expected_latency = match case.tag {
                EffectKindTag::Limiter => limiter_lookahead_frames(sr),
                EffectKindTag::Delay
                | EffectKindTag::Reverb
                | EffectKindTag::Eq
                | EffectKindTag::Chorus
                | EffectKindTag::Drive
                | EffectKindTag::Filter
                | EffectKindTag::Compressor => 0,
            };
            assert_eq!(fx.latency_frames(), expected_latency, "{}", case.name);

            let expects_gr = matches!(case.tag, EffectKindTag::Compressor | EffectKindTag::Limiter);
            assert_eq!(
                fx.gain_reduction_db().is_some(),
                expects_gr,
                "{}",
                case.name
            );
            let mut probe = stereo_stimulus(512, sr, 0.95);
            let mut gr = vec![0.0_f32; probe.len()];
            let has_gr = fx.process_block_with_gr(&mut probe, false, &mut gr);
            assert_eq!(has_gr, expects_gr, "{}", case.name);
            if expects_gr {
                let max_gr = gr.iter().copied().fold(0.0_f32, f32::max);
                assert!(
                    max_gr > 1.0,
                    "{} sr={sr}: dynamics processor must register > 1 dB GR on hot stimulus, got {max_gr}",
                    case.name
                );
                assert!(gr.iter().all(|&g| g.is_finite() && g >= 0.0));
            }
        }
    }
}
