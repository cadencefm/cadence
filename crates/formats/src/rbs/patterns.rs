//! Pattern builders, TR-909 flam mini-notation, dense-drum overflow stacking, deduplication,
//! and TB-303 transposition sharing.

use super::calibration::{
    rbs_909_flam_offset, rbs_drum_velocities, RBS_303_ACCENT_VEL, RBS_303_NORMAL_VEL,
};
use super::controls::TR808_SELECTOR_SPECS;
use super::model::{DrumPattern, Tb303Pattern, Tb303Step};
use cadence_core::material::{Part, Pattern, PatternKind, MAX_PARTS};
use cadence_core::music::{ChordTone, Step, MAX_CHORD_TONES};
use cadence_core::rack::drum_midi;

const MAX_STEP_HITS: usize = 1 + MAX_CHORD_TONES;

/// Next sounding boundary state following a slid TB-303 step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tb303NextStep {
    /// Followed by an active note on the next step: overlap past max Swing (`1.52`) for smooth Mono+Legato glide.
    Note,
    /// Followed by a rest step inside an active pattern (`F~ .`): authentic TB-303 2-step tie (`1.65`)
    /// sustaining across the rest step and releasing cleanly before step `i + 2`.
    Rest,
    /// Followed by device mute/disable or non-looped song stop: trim cleanly (`1.05`) so audio doesn't bleed.
    Stop,
}

/// Convert a single TB-303 step into a Cadence `Step`, given what follows on the next step.
pub(super) fn tb303_step_to_cadence(s: Tb303Step, next: Tb303NextStep) -> Step {
    if s.rest {
        return Step::default();
    }
    let mut semitones = i16::from(s.note);
    if s.up {
        semitones += 12;
    }
    if s.down {
        semitones -= 12;
    }
    // Chromatic scale from C2 (MIDI 36): `pitch_midi` subtracts 1 from every degree
    // (including negative/zero down-octave degrees), so `degree = semitones + 1`.
    let degree = semitones + 1;
    // On a slid step:
    // - into another note: extend past max Swing offset (0.45) for smooth Mono+Legato glide (`1.52`);
    // - into a rest (`Note+Slide` + `Rest`): sustain across the rest step (`1.65`, classic TB-303 tie)
    //   while releasing well before step `i + 2` (even when `i` is swung);
    // - into a device disable or song stop: release cleanly at `1.05`.
    // On a non-slide step, keep gate < 1.0 - max_swing (0.55) so consecutive swung notes retrigger cleanly.
    let gate = if s.slide {
        match next {
            Tb303NextStep::Note => 1.52,
            Tb303NextStep::Rest => 1.65,
            Tb303NextStep::Stop => 1.05,
        }
    } else {
        0.52
    };
    Step {
        active: true,
        degree,
        accidental: 0,
        velocity: if s.accent {
            RBS_303_ACCENT_VEL
        } else {
            RBS_303_NORMAL_VEL
        },
        gate,
        ..Step::default()
    }
}

pub(super) fn tb303_pattern_to_steps(pat: &Tb303Pattern) -> Vec<Step> {
    let len = usize::from(pat.length.clamp(1, 16));
    let mut steps = Vec::with_capacity(len);
    for (i, &s) in pat.steps[..len].iter().enumerate() {
        let next = if pat.steps[(i + 1) % len].rest {
            Tb303NextStep::Rest
        } else {
            Tb303NextStep::Note
        };
        steps.push(tb303_step_to_cadence(s, next));
    }
    steps
}

fn drum_col_to_degree(is_909: bool, sel_mask_808: u8) -> [(usize, i16); 11] {
    let deg = |midi: u8| i16::from(midi - drum_midi::BD) + 1;
    if is_909 {
        // 909 cols: 0=AC, 1=BD, 2=SD, 3=LT, 4=MT, 5=HT, 6=RS, 7=CP, 8=CH, 9=OH, 10=CC, 11=RC
        [
            (1, deg(drum_midi::BD)),
            (2, deg(drum_midi::SD)),
            (8, deg(drum_midi::CH)),
            (9, deg(drum_midi::OH)),
            (7, deg(drum_midi::CP)),
            (6, deg(drum_midi::RS)),
            (10, deg(drum_midi::CB_OR_CR)),
            (11, deg(drum_midi::CY_OR_RD)),
            (3, deg(drum_midi::LT)),
            (4, deg(drum_midi::MT)),
            (5, deg(drum_midi::HT)),
        ]
    } else {
        // 808 cols: 0=AC, 1=BD, 2=SD, 3=LT, 4=MT, 5=HT, 6=RS, 7=CP, 8=CB, 9=CY, 10=OH, 11=CH
        let sel_midi = |idx: usize| {
            let spec = TR808_SELECTOR_SPECS[idx];
            if sel_mask_808 & (1 << idx) != 0 {
                spec.alt_midi
            } else {
                spec.primary_midi
            }
        };
        [
            (1, deg(drum_midi::BD)),
            (2, deg(drum_midi::SD)),
            (11, deg(drum_midi::CH)),
            (10, deg(drum_midi::OH)),
            (7, deg(sel_midi(4))),
            (6, deg(sel_midi(3))),
            (8, deg(drum_midi::CB_OR_CR)),
            (9, deg(drum_midi::CY_OR_RD)),
            (3, deg(sel_midi(0))),
            (4, deg(sel_midi(1))),
            (5, deg(sel_midi(2))),
        ]
    }
}

fn hits_to_step(hits: &[(i16, f32)]) -> Step {
    let Some(&(first_deg, first_vel)) = hits.first() else {
        return Step::default();
    };
    let mut step = Step {
        active: true,
        degree: first_deg,
        accidental: 0,
        velocity: first_vel,
        gate: 0.5,
        ..Step::default()
    };
    for &(deg, vel) in hits.iter().skip(1).take(MAX_CHORD_TONES) {
        step.chord.push(ChordTone {
            degree: deg,
            accidental: 0,
            velocity: vel,
            gate: 0.5,
        });
    }
    step
}

/// Decoded drum pattern steps. Patterns with `<= 6` simultaneous hits per step leave `overflow`
/// as `None` (flat `PatternKind::Notes`); steps with `7..=11` simultaneous voices split hits
/// `6..len` into `overflow` so `PatternCatalog` combines them cleanly via `PatternKind::Stack`.
pub(super) struct DrumPatternSteps {
    pub primary: Vec<Step>,
    pub overflow: Option<Vec<Step>>,
}

pub(super) fn drum_pattern_has_accent(pat: &DrumPattern, is_909: bool) -> bool {
    let len = usize::from(pat.length.clamp(1, 16));
    pat.steps[..len].iter().any(|row| {
        let has_hit = row[1..12].iter().any(|&v| v != 0);
        (row[0] != 0 && has_hit) || (is_909 && row[1..12].contains(&2))
    })
}

pub(super) fn drum_pattern_to_steps(
    pat: &DrumPattern,
    is_909: bool,
    sel_mask_808: u8,
    accent_knob: u8,
) -> DrumPatternSteps {
    let col_to_degree = drum_col_to_degree(is_909, sel_mask_808);
    let vels = rbs_drum_velocities(accent_knob);
    let len = usize::from(pat.length.clamp(1, 16));
    let mut primary = Vec::with_capacity(len);
    let mut overflow_steps: Vec<Step> = Vec::with_capacity(len);
    let mut any_overflow = false;

    for row in &pat.steps[..len] {
        let global_accent = row[0] != 0;
        let mut hits: Vec<(i16, f32)> = Vec::new();
        for &(col, degree) in &col_to_degree {
            let v = row[col];
            if v != 0 {
                // On TR-909: v==1 normal, v==2 accent, v>=3 flam (grace hit on step, main hit
                // delayed by `flam_interval` via layered Mini sub-step pattern).
                let vel = if is_909 && v >= 3 {
                    if global_accent {
                        vels.flam_grace_accent
                    } else {
                        vels.flam_grace_normal
                    }
                } else if global_accent || (is_909 && v == 2) {
                    vels.accent
                } else {
                    vels.normal
                };
                hits.push((degree, vel));
            }
        }
        let split = hits.len().min(MAX_STEP_HITS);
        primary.push(hits_to_step(&hits[..split]));
        if hits.len() > MAX_STEP_HITS {
            any_overflow = true;
            overflow_steps.push(hits_to_step(&hits[split..]));
        } else {
            overflow_steps.push(Step::default());
        }
    }

    DrumPatternSteps {
        primary,
        overflow: any_overflow.then_some(overflow_steps),
    }
}

/// Build native Cadence `PatternKind::Mini` sub-step flam layer(s) for TR-909 steps carrying
/// flam triggers (`step[col] >= 3`), firing the delayed second hit at BPM-accurate micro-offset
/// `rbs_909_flam_offset(flam_interval, song_bpm)`. Returns an empty `Vec` when no flams exist,
/// 1 layer for normal patterns (`<= 6` simultaneous flams), or 2 layers if `7..=11` voices flam
/// simultaneously on the same step.
pub(super) fn tr909_flam_mini_codes(
    pat: &DrumPattern,
    flam_interval: u8,
    accent_knob: u8,
    song_bpm: f64,
) -> Vec<String> {
    let col_to_degree = drum_col_to_degree(true, 0);
    let len = usize::from(pat.length.clamp(1, 16));
    let offset = rbs_909_flam_offset(flam_interval, song_bpm);
    // In `[~@1 hit@W]`, `hit` fires at fractional step offset `1 / (1 + W)`.
    let hit_weight = ((1.0 / offset) - 1.0).clamp(1.08, 28.0);
    let mut primary_tokens = Vec::with_capacity(len);
    let mut overflow_tokens = Vec::with_capacity(len);
    let mut has_flam = false;
    let mut has_overflow_flam = false;

    let format_flam_token = |degs: &[i16], accent: bool| -> String {
        if degs.is_empty() {
            return "~".to_string();
        }
        let acc = if accent { "!" } else { "" };
        let target = if degs.len() == 1 {
            format!("{}", degs[0])
        } else {
            let joined = degs
                .iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
                .join(",");
            format!("[{joined}]")
        };
        format!("[~@1 {target}{acc}@{hit_weight:.2}]")
    };

    for row in &pat.steps[..len] {
        let global_accent = row[0] != 0 && accent_knob > 0;
        let mut flam_degs: Vec<i16> = Vec::new();
        for &(col, degree) in &col_to_degree {
            if row[col] >= 3 {
                flam_degs.push(degree);
            }
        }
        if flam_degs.is_empty() {
            primary_tokens.push("~".to_string());
            overflow_tokens.push("~".to_string());
        } else {
            has_flam = true;
            let split = flam_degs.len().min(MAX_STEP_HITS);
            primary_tokens.push(format_flam_token(&flam_degs[..split], global_accent));
            if flam_degs.len() > MAX_STEP_HITS {
                has_overflow_flam = true;
                overflow_tokens.push(format_flam_token(&flam_degs[split..], global_accent));
            } else {
                overflow_tokens.push("~".to_string());
            }
        }
    }

    let mut out = Vec::new();
    if has_flam {
        out.push(primary_tokens.join(" "));
    }
    if has_overflow_flam {
        out.push(overflow_tokens.join(" "));
    }
    out
}

pub(super) fn tb303_pattern_non_empty(pat: &Tb303Pattern) -> bool {
    let len = usize::from(pat.length.clamp(1, 16));
    pat.steps[..len].iter().any(|s| !s.rest)
}

pub(super) fn drum_pattern_non_empty(pat: &DrumPattern) -> bool {
    let len = usize::from(pat.length.clamp(1, 16));
    pat.steps[..len]
        .iter()
        .any(|row| row[1..12].iter().any(|&v| v != 0))
}

/// Detect whether `cand` is an exact constant-semitone transposition (`Δ ∈ -36..=36`, `Δ != 0`)
/// of an earlier TB-303 pattern `base` with identical length, rests, velocities, gates/slides,
/// conditions, and chords. Since `.rbs` projects use chromatic `0..12` degrees, `degree` delta
/// equals exact semitone MIDI transposition (`Part.transpose`).
fn find_303_transposition(base: &[Step], cand: &[Step]) -> Option<i16> {
    if base.len() != cand.len() {
        return None;
    }
    let mut delta: Option<i32> = None;
    for (a, b) in base.iter().zip(cand.iter()) {
        if a.active != b.active {
            return None;
        }
        if !a.active {
            continue;
        }
        if a.accidental != b.accidental
            || a.velocity != b.velocity
            || a.gate != b.gate
            || a.condition != b.condition
            || a.chord != b.chord
        {
            return None;
        }
        let d = i32::from(b.degree) - i32::from(a.degree);
        if d == 0 || !(-36..=36).contains(&d) {
            return None;
        }
        match delta {
            None => delta = Some(d),
            Some(prev) if prev != d => return None,
            Some(_) => {}
        }
    }
    delta.map(|d| d as i16)
}

struct MaterializedBase {
    is_303: bool,
    steps: Vec<Step>,
    overflow_steps: Option<Vec<Step>>,
    flam_minis: Vec<String>,
    combined_pat_id: u64,
    swing_pat_id: Option<u64>,
    base_name: String,
}

pub(super) struct PatternCatalog {
    bases: Vec<MaterializedBase>,
    swing_amount: f32,
    pattern_budget: usize,
}

impl PatternCatalog {
    pub(super) fn new(swing_amount: f32, pattern_budget: usize) -> Self {
        Self {
            bases: Vec::new(),
            swing_amount,
            pattern_budget,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn materialize_part(
        &mut self,
        is_303: bool,
        pat_name: String,
        inst_id: u64,
        steps: Vec<Step>,
        overflow_steps: Option<Vec<Step>>,
        pat_shuffle: bool,
        flam_minis: Vec<String>,
        patterns: &mut Vec<Pattern>,
        parts: &mut Vec<Part>,
        fresh: &mut dyn FnMut() -> u64,
    ) -> Option<u64> {
        // Skip completely silent patterns to keep the arrangement & launchpads clean.
        if steps.iter().all(|s| !s.active) || parts.len() >= MAX_PARTS {
            return None;
        }
        let need_swing = pat_shuffle && self.swing_amount > 0.01;

        // 1. Exact duplicate match (including across 303A <-> 303B).
        let exact_match = self
            .bases
            .iter()
            .position(|b| {
                b.steps == steps && b.overflow_steps == overflow_steps && b.flam_minis == flam_minis
            })
            .map(|idx| (idx, 0i16));

        // 2. Exact TB-303 constant-semitone transposition (`Part.transpose = Δ`).
        let matched = exact_match.or_else(|| {
            if !is_303 || overflow_steps.is_some() || !flam_minis.is_empty() {
                return None;
            }
            self.bases.iter().enumerate().find_map(|(idx, b)| {
                if !b.is_303 || b.overflow_steps.is_some() || !b.flam_minis.is_empty() {
                    return None;
                }
                find_303_transposition(&b.steps, &steps).map(|delta| (idx, delta))
            })
        });

        let (play_pat_id, transpose) = if let Some((base_idx, transpose)) = matched {
            let play_id = if need_swing {
                if let Some(sw_id) = self.bases[base_idx].swing_pat_id {
                    sw_id
                } else {
                    if patterns.len() + 1 > self.pattern_budget {
                        return None;
                    }
                    let sw_id = fresh();
                    let combined_id = self.bases[base_idx].combined_pat_id;
                    let sw_name = format!("{} Swing", self.bases[base_idx].base_name);
                    patterns.push(Pattern {
                        id: sw_id,
                        name: sw_name,
                        kind: PatternKind::Swing {
                            source: combined_id,
                            amount: self.swing_amount,
                        },
                    });
                    self.bases[base_idx].swing_pat_id = Some(sw_id);
                    sw_id
                }
            } else {
                self.bases[base_idx].combined_pat_id
            };
            (play_id, transpose)
        } else {
            let extra_layers = usize::from(overflow_steps.is_some()) + flam_minis.len();
            let need_stack = extra_layers > 0;
            let reserve = 1 + extra_layers + usize::from(need_stack) + usize::from(need_swing);
            if patterns.len() + reserve > self.pattern_budget {
                return None;
            }

            let base_pat_id = fresh();
            let pat_len = steps.len() as u16;
            patterns.push(Pattern {
                id: base_pat_id,
                name: if need_stack || need_swing {
                    format!("{pat_name} Base")
                } else {
                    pat_name.clone()
                },
                kind: PatternKind::Notes {
                    steps: steps.clone(),
                },
            });

            let combined_pat_id = if need_stack {
                let mut stack_parts = Vec::with_capacity(1 + extra_layers);
                stack_parts.push(base_pat_id);
                if let Some(ref ov_steps) = overflow_steps {
                    let ov_pat_id = fresh();
                    patterns.push(Pattern {
                        id: ov_pat_id,
                        name: format!("{pat_name} Extra"),
                        kind: PatternKind::Notes {
                            steps: ov_steps.clone(),
                        },
                    });
                    stack_parts.push(ov_pat_id);
                }
                for (i, code) in flam_minis.iter().enumerate() {
                    let flam_pat_id = fresh();
                    let flam_name = if i == 0 {
                        format!("{pat_name} Flam")
                    } else {
                        format!("{pat_name} Flam {}", i + 1)
                    };
                    patterns.push(Pattern {
                        id: flam_pat_id,
                        name: flam_name,
                        kind: PatternKind::Mini {
                            code: code.clone(),
                            steps: pat_len,
                        },
                    });
                    stack_parts.push(flam_pat_id);
                }
                let stack_id = fresh();
                patterns.push(Pattern {
                    id: stack_id,
                    name: if need_swing {
                        format!("{pat_name} Raw")
                    } else {
                        pat_name.clone()
                    },
                    kind: PatternKind::Stack { parts: stack_parts },
                });
                stack_id
            } else {
                base_pat_id
            };

            let (play_pat_id, swing_pat_id) = if need_swing {
                let sw_id = fresh();
                patterns.push(Pattern {
                    id: sw_id,
                    name: pat_name.clone(),
                    kind: PatternKind::Swing {
                        source: combined_pat_id,
                        amount: self.swing_amount,
                    },
                });
                (sw_id, Some(sw_id))
            } else {
                (combined_pat_id, None)
            };

            self.bases.push(MaterializedBase {
                is_303,
                steps,
                overflow_steps,
                flam_minis,
                combined_pat_id,
                swing_pat_id,
                base_name: pat_name.clone(),
            });
            (play_pat_id, 0i16)
        };

        let pid = fresh();
        let mut part = Part::new(pid, pat_name, play_pat_id, inst_id);
        part.transpose = transpose;
        parts.push(part);
        Some(pid)
    }
}
