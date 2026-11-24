//! Core musical primitives shared across material definitions, evaluation, and playback.
//!
//! Pure types only: notes, chord tones, step conditions, scales, pitch helpers, and end rules.
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};

pub const STEPS_PER_BAR: usize = 16;
pub const MAX_CHORD_TONES: usize = 5;
/// Inclusive bounds on scale degree values (`-127..=128` covers full MIDI `0..=127`
/// across any `root_midi` and any scale including 12-tone chromatic without clamping).
pub const MIN_DEGREE: i16 = -127;
pub const MAX_DEGREE: i16 = 128;

/// What song playback does at the end of the arrangement.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndRule {
    /// Return to the start.
    #[default]
    Loop,
    /// Stop and return to the start.
    Stop,
    /// Keep the clock running past the end; nothing authored plays there.
    Continue,
}

impl EndRule {
    pub fn is_loop(&self) -> bool {
        *self == EndRule::Loop
    }

    pub fn label(self) -> &'static str {
        match self {
            EndRule::Loop => "loop",
            EndRule::Stop => "stop",
            EndRule::Continue => "continue",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "loop" => Some(EndRule::Loop),
            "stop" => Some(EndRule::Stop),
            "continue" => Some(EndRule::Continue),
            _ => None,
        }
    }

    /// The next rule, for a key that cycles them.
    pub fn next(self) -> Self {
        match self {
            EndRule::Loop => EndRule::Stop,
            EndRule::Stop => EndRule::Continue,
            EndRule::Continue => EndRule::Loop,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepCondition {
    pub turn: u8,
    pub every: u8,
}

impl Default for StepCondition {
    fn default() -> Self {
        Self { turn: 1, every: 1 }
    }
}

impl StepCondition {
    pub const ALWAYS: Self = Self { turn: 1, every: 1 };

    pub fn is_always(self) -> bool {
        self.every <= 1
    }

    pub fn matches(self, pass_one_based: u64) -> bool {
        if self.every <= 1 {
            return true;
        }
        let every = u64::from(self.every);
        let turn = u64::from(self.turn.clamp(1, self.every));
        (pass_one_based.saturating_sub(1) % every) + 1 == turn
    }

    pub fn label(self) -> String {
        format!("{}:{}", self.turn, self.every)
    }

    pub fn cycle(self, delta: i8) -> Self {
        const PRESETS: [(u8, u8); 12] = [
            (1, 1),
            (1, 2),
            (2, 2),
            (1, 3),
            (2, 3),
            (3, 3),
            (1, 4),
            (2, 4),
            (3, 4),
            (4, 4),
            (4, 8),
            (8, 8),
        ];
        let pos = PRESETS
            .iter()
            .position(|&(t, e)| t == self.turn && e == self.every)
            .unwrap_or(0) as isize;
        let next = (pos + isize::from(delta)).rem_euclid(PRESETS.len() as isize) as usize;
        let (turn, every) = PRESETS[next];
        Self { turn, every }
    }
}

/// Additional stacked chord voice on a polyphonic `Step`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChordTone {
    pub degree: i16,
    pub accidental: i8,
    pub velocity: f32,
    pub gate: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub active: bool,
    pub degree: i16,
    pub accidental: i8,
    pub velocity: f32,
    pub gate: f32,
    pub condition: StepCondition,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chord: Vec<ChordTone>,
    /// Transient provenance for the prepared score; never persisted in files.
    #[serde(skip)]
    pub source_part: Option<u64>,
}

impl Default for Step {
    fn default() -> Self {
        Self {
            active: false,
            degree: 1,
            accidental: 0,
            velocity: 0.8,
            gate: 0.75,
            condition: StepCondition::ALWAYS,
            chord: Vec::new(),
            source_part: None,
        }
    }
}

pub fn pitch_midi(root_midi: u8, scale: &[i8], degree: i16, accidental: i8, semitones: i32) -> u8 {
    if scale.is_empty() {
        return 60;
    }
    let degree_zero = i32::from(degree) - 1;
    let scale_length = scale.len() as i32;
    let octave = degree_zero.div_euclid(scale_length);
    let offset = i32::from(scale[degree_zero.rem_euclid(scale_length) as usize]);
    let pitch = i32::from(root_midi) + offset + i32::from(accidental) + octave * 12 + semitones;
    pitch.clamp(0, 127) as u8
}

pub fn midi_to_degree(
    root_midi: u8,
    scale: &[i8],
    target_midi: u8,
    transpose_semitones: i32,
) -> (i16, i8) {
    if scale.is_empty() {
        return (1, 0);
    }
    let rel = i32::from(target_midi) - i32::from(root_midi) - transpose_semitones;
    let octave = rel.div_euclid(12);
    let pc = rel.rem_euclid(12);
    let scale_len = scale.len() as i32;

    // Pick the nearest unclamped scale tone (checking adjacent octaves for sparse scales),
    // preferring exact matches (`acc == 0`), then smallest |acc|, then sharps over flats.
    let mut best_oct_delta = 0i32;
    let mut best_idx = 0usize;
    let mut best_acc = i32::MAX;
    for oct_delta in -1..=1 {
        for (idx, &s) in scale.iter().enumerate() {
            let acc = pc - (i32::from(s) + oct_delta * 12);
            let better =
                acc.abs() < best_acc.abs() || (acc.abs() == best_acc.abs() && acc > best_acc);
            if better {
                best_oct_delta = oct_delta;
                best_idx = idx;
                best_acc = acc;
            }
        }
    }
    let degree = ((octave + best_oct_delta) * scale_len + best_idx as i32 + 1)
        .clamp(i32::from(MIN_DEGREE), i32::from(MAX_DEGREE)) as i16;
    let accidental = best_acc.clamp(-6, 6) as i8;
    (degree, accidental)
}

pub fn note_name(midi: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C♯", "D", "E♭", "E", "F", "F♯", "G", "A♭", "A", "B♭", "B",
    ];
    format!(
        "{}{}",
        NAMES[usize::from(midi % 12)],
        i16::from(midi) / 12 - 1
    )
}

/// Authoritative musical scale definition shared by `scale_name`, `scale_label`,
/// `named_scale`, `cycle_scale`, `:key`, and the Song Properties modal (`gP`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScaleSpec {
    pub id: &'static str,
    pub label: &'static str,
    pub aliases: &'static [&'static str],
    pub intervals: &'static [i8],
}

pub const NAMED_SCALES: &[ScaleSpec] = &[
    ScaleSpec {
        id: "major",
        label: "Major",
        aliases: &["major", "maj", "ionian"],
        intervals: &[0, 2, 4, 5, 7, 9, 11],
    },
    ScaleSpec {
        id: "minor",
        label: "Minor",
        aliases: &["minor", "min", "aeolian", "natural_minor"],
        intervals: &[0, 2, 3, 5, 7, 8, 10],
    },
    ScaleSpec {
        id: "dorian",
        label: "Dorian",
        aliases: &["dorian", "dor"],
        intervals: &[0, 2, 3, 5, 7, 9, 10],
    },
    ScaleSpec {
        id: "phrygian",
        label: "Phrygian",
        aliases: &["phrygian", "phr"],
        intervals: &[0, 1, 3, 5, 7, 8, 10],
    },
    ScaleSpec {
        id: "lydian",
        label: "Lydian",
        aliases: &["lydian", "lyd"],
        intervals: &[0, 2, 4, 6, 7, 9, 11],
    },
    ScaleSpec {
        id: "mixolydian",
        label: "Mixolydian",
        aliases: &["mixolydian", "mix"],
        intervals: &[0, 2, 4, 5, 7, 9, 10],
    },
    ScaleSpec {
        id: "harmonic_minor",
        label: "Harmonic Minor",
        aliases: &["harmonic_minor", "harm", "harmonic"],
        intervals: &[0, 2, 3, 5, 7, 8, 11],
    },
    ScaleSpec {
        id: "melodic_minor",
        label: "Melodic Minor",
        aliases: &["melodic_minor", "melodic", "mel"],
        intervals: &[0, 2, 3, 5, 7, 9, 11],
    },
    ScaleSpec {
        id: "major_pentatonic",
        label: "Pentatonic Major",
        aliases: &["major_pentatonic", "pentatonic_major", "majpent"],
        intervals: &[0, 2, 4, 7, 9],
    },
    ScaleSpec {
        id: "pentatonic",
        label: "Pentatonic Minor",
        aliases: &["pentatonic", "minor_pentatonic", "pentatonic_minor", "pent"],
        intervals: &[0, 3, 5, 7, 10],
    },
    ScaleSpec {
        id: "blues",
        label: "Blues",
        aliases: &["blues"],
        intervals: &[0, 3, 5, 6, 7, 10],
    },
    ScaleSpec {
        id: "whole_tone",
        label: "Whole Tone",
        aliases: &["whole_tone", "wholetone", "whole"],
        intervals: &[0, 2, 4, 6, 8, 10],
    },
    ScaleSpec {
        id: "chromatic",
        label: "Chromatic",
        aliases: &["chromatic", "chrom"],
        intervals: &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
    },
];

pub fn scale_spec(scale: &[i8]) -> Option<&'static ScaleSpec> {
    NAMED_SCALES.iter().find(|s| s.intervals == scale)
}

pub fn scale_name(scale: &[i8]) -> &'static str {
    scale_spec(scale).map_or("custom", |s| s.id)
}

pub fn scale_label(scale: &[i8]) -> &'static str {
    scale_spec(scale).map_or("Custom", |s| s.label)
}

pub fn named_scale(name: &str) -> Option<Vec<i8>> {
    let norm = name.trim().to_ascii_lowercase().replace([' ', '-'], "_");
    NAMED_SCALES
        .iter()
        .find(|spec| spec.aliases.contains(&norm.as_str()))
        .map(|spec| spec.intervals.to_vec())
}

pub fn cycle_scale(scale: &[i8], direction: i8) -> &'static ScaleSpec {
    let len = NAMED_SCALES.len() as isize;
    let cur = NAMED_SCALES
        .iter()
        .position(|s| s.intervals == scale)
        .map(|i| i as isize);
    let next = match cur {
        Some(idx) => {
            let step = if direction >= 0 { 1 } else { -1 };
            (idx + step).rem_euclid(len) as usize
        }
        None => {
            if direction >= 0 {
                0
            } else {
                NAMED_SCALES.len() - 1
            }
        }
    };
    &NAMED_SCALES[next]
}

pub fn parse_root_midi(input: &str) -> Option<u8> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut chars = trimmed.chars().peekable();
    let letter = chars.next()?.to_ascii_uppercase();
    let base: i32 = match letter {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let accidental: i32 = match chars.peek().copied() {
        Some('#' | '♯') => {
            chars.next();
            1
        }
        Some('b' | '♭') => {
            chars.next();
            -1
        }
        _ => 0,
    };
    let rest: String = chars.collect();
    let octave: i32 = if rest.is_empty() {
        4
    } else {
        rest.parse().ok()?
    };
    let midi = (octave + 1) * 12 + base + accidental;
    (0..=127).contains(&midi).then_some(midi as u8)
}

pub fn validate_step(step: &Step) -> Result<()> {
    ensure!(
        (MIN_DEGREE..=MAX_DEGREE).contains(&step.degree),
        "Scale degree must be between {MIN_DEGREE} and {MAX_DEGREE}"
    );
    ensure!(
        (-6..=6).contains(&step.accidental),
        "Accidental offset must be between −6 and 6 semitones"
    );
    ensure!(
        step.velocity.is_finite() && (0.0..=1.0).contains(&step.velocity),
        "Velocity must be between 0 and 1"
    );
    ensure!(
        step.gate.is_finite() && (0.05..=64.0).contains(&step.gate),
        "Gate length must be between 0.05 and 64 steps"
    );
    ensure!(
        (1..=16).contains(&step.condition.every)
            && (1..=step.condition.every).contains(&step.condition.turn),
        "Step condition {}:{} must satisfy 1 <= turn <= every <= 16",
        step.condition.turn,
        step.condition.every
    );
    ensure!(
        step.chord.len() <= MAX_CHORD_TONES,
        "A step can stack at most {MAX_CHORD_TONES} extra chord voices"
    );
    for tone in &step.chord {
        ensure!(
            (MIN_DEGREE..=MAX_DEGREE).contains(&tone.degree),
            "Chord tone scale degree must be between {MIN_DEGREE} and {MAX_DEGREE}"
        );
        ensure!(
            (-6..=6).contains(&tone.accidental),
            "Chord tone accidental must be between −6 and 6 semitones"
        );
        ensure!(
            tone.velocity.is_finite() && (0.0..=1.0).contains(&tone.velocity),
            "Chord tone velocity must be between 0 and 1"
        );
        ensure!(
            tone.gate.is_finite() && (0.05..=64.0).contains(&tone.gate),
            "Chord tone gate must be between 0.05 and 64 steps"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_scales_spot_covers_all_thirteen_scales_and_cycles_cleanly() {
        assert_eq!(NAMED_SCALES.len(), 13);
        for spec in NAMED_SCALES {
            assert_eq!(spec.intervals[0], 0);
            assert!(spec.intervals.windows(2).all(|w| w[0] < w[1]));
            assert_eq!(scale_name(spec.intervals), spec.id);
            assert_eq!(scale_label(spec.intervals), spec.label);
            assert_eq!(named_scale(spec.id).as_deref(), Some(spec.intervals));
            assert_eq!(named_scale(spec.label).as_deref(), Some(spec.intervals));
            for alias in spec.aliases {
                assert_eq!(named_scale(alias).as_deref(), Some(spec.intervals));
            }
        }
        let first = NAMED_SCALES[0].intervals;
        let last = NAMED_SCALES.last().unwrap().intervals;
        assert_eq!(cycle_scale(first, -1).intervals, last);
        assert_eq!(cycle_scale(last, 1).intervals, first);
        assert_eq!(cycle_scale(&[0, 1, 5], 1).intervals, first);
    }
}
