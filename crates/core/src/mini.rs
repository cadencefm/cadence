//! Pure Strudel/Tidal-inspired inline mini-notation parser and sub-step evaluator.
//!
//! Supported syntax (space-separated sequence over `steps` sixteenth-steps per cycle):
//! - Pitches: note names (`c4`, `eb3`, `f#5`, `bb2`) or 1-based scale degrees (`1`, `3`, `-2`, `5#`, `3b`)
//! - Rests: `~` or `.`
//! - Subdivisions (recursive): `[c4 e4 g4]`, `[c4 [e4 g4]]`
//! - Chords (comma-separated): `[c4,e4,g4]` or `c4,e4,g4`
//! - Alternating cycles: `<c4 e4 g4 b4>` (advances one item per pattern cycle)
//! - Modifiers (chainable): `*N` repeat, `/N` slow, `@N` weight/elongate, `(k,n)` / `(k,n,r)` Euclidean,
//!   `?` / `?0.6` deterministic probability, `!` accent (`velocity = 0.98`).

use crate::evaluate::{det_hash, EvaluatedEvent};
use crate::material::Id;
use crate::music::{ChordTone, Step, MAX_CHORD_TONES, MAX_DEGREE, MIN_DEGREE};
use anyhow::{bail, ensure, Context, Result};

#[derive(Clone, Debug, PartialEq)]
pub enum PitchSpec {
    Midi(u8),
    Degree { degree: i16, accidental: i8 },
}

#[derive(Clone, Debug, PartialEq)]
pub enum MiniNode {
    Rest,
    Pitch(PitchSpec),
    Chord(Vec<PitchSpec>),
    Seq(Vec<WeightedNode>),
    Alt(Vec<WeightedNode>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct WeightedNode {
    pub node: MiniNode,
    pub weight: f32,
    pub repeat: u8,
    pub slow: u8,
    pub euclidean: Option<(u16, u16, i16)>,
    pub chance: f32,
    pub accent: bool,
}

impl WeightedNode {
    fn plain(node: MiniNode) -> Self {
        Self {
            node,
            weight: 1.0,
            repeat: 1,
            slow: 1,
            euclidean: None,
            chance: 1.0,
            accent: false,
        }
    }
}

/// Bjorklund / Bresenham Euclidean rhythm predicate: true when `step` has a pulse.
#[inline]
pub fn euclidean_hit(pulses: u16, steps: u16, rotation: i16, step: i64) -> bool {
    if steps == 0 || pulses == 0 {
        return false;
    }
    let k = pulses.min(steps) as i64;
    let n = steps as i64;
    let idx = (step - i64::from(rotation)).rem_euclid(n);
    ((idx * k) % n) < k
}

/// Parse a mini-notation string into a validated AST.
pub fn parse_mini(code: &str) -> Result<Vec<WeightedNode>> {
    let trimmed = code.trim();
    ensure!(!trimmed.is_empty(), "Mini-notation code cannot be empty");
    let chars: Vec<char> = trimmed.chars().collect();
    let mut pos = 0;
    let nodes = parse_seq(&chars, &mut pos, None)?;
    ensure!(
        pos == chars.len(),
        "Unexpected trailing character '{}' at column {}",
        chars[pos],
        pos + 1
    );
    ensure!(
        !nodes.is_empty(),
        "Mini-notation sequence must contain at least one token"
    );
    Ok(nodes)
}

fn skip_ws(chars: &[char], pos: &mut usize) {
    while *pos < chars.len() && chars[*pos].is_whitespace() {
        *pos += 1;
    }
}

fn parse_seq(chars: &[char], pos: &mut usize, stop: Option<char>) -> Result<Vec<WeightedNode>> {
    let mut out = Vec::new();
    loop {
        skip_ws(chars, pos);
        if *pos >= chars.len() {
            if let Some(ch) = stop {
                bail!("Unclosed delimiter, expected '{ch}'");
            }
            break;
        }
        if stop == Some(chars[*pos]) {
            *pos += 1;
            break;
        }
        out.push(parse_item(chars, pos, stop)?);
    }
    Ok(out)
}

fn parse_item(chars: &[char], pos: &mut usize, stop: Option<char>) -> Result<WeightedNode> {
    skip_ws(chars, pos);
    let ch = *chars
        .get(*pos)
        .ok_or_else(|| anyhow::anyhow!("Unexpected end of mini-notation"))?;
    let base = match ch {
        '[' => {
            *pos += 1;
            // Look ahead: comma-separated pitches without spaces inside [...] OR nested sequence?
            let start = *pos;
            if let Some(chord) = try_parse_bracket_chord(chars, pos) {
                ensure!(
                    chord.len() <= MAX_CHORD_TONES + 1,
                    "Mini-notation chord can stack at most {} notes (1 main + {} chord tones)",
                    MAX_CHORD_TONES + 1,
                    MAX_CHORD_TONES
                );
                MiniNode::Chord(chord)
            } else {
                *pos = start;
                let inner = parse_seq(chars, pos, Some(']'))?;
                ensure!(!inner.is_empty(), "Empty subdivision '[]' in mini-notation");
                MiniNode::Seq(inner)
            }
        }
        '<' => {
            *pos += 1;
            let inner = parse_seq(chars, pos, Some('>'))?;
            ensure!(!inner.is_empty(), "Empty alternation '<>' in mini-notation");
            MiniNode::Alt(inner)
        }
        '~' | '.' => {
            *pos += 1;
            MiniNode::Rest
        }
        ']' | '>' => bail!("Unexpected closing delimiter '{ch}' at column {}", *pos + 1),
        _ => parse_atom(chars, pos, stop)?,
    };

    let mut item = WeightedNode::plain(base);
    parse_modifiers(chars, pos, &mut item)?;
    Ok(item)
}

fn try_parse_bracket_chord(chars: &[char], pos: &mut usize) -> Option<Vec<PitchSpec>> {
    let mut i = *pos;
    let mut buf = String::new();
    let mut depth = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '[' || c == '<' {
            depth += 1;
        } else if c == ']' {
            if depth == 0 {
                break;
            }
            depth -= 1;
        }
        buf.push(c);
        i += 1;
    }
    if i >= chars.len() || chars[i] != ']' || !buf.contains(',') {
        return None;
    }
    let mut pitches = Vec::new();
    for part in buf.split(',') {
        let p = parse_pitch(part.trim()).ok()?;
        pitches.push(p);
    }
    if pitches.len() >= 2 {
        *pos = i + 1;
        Some(pitches)
    } else {
        None
    }
}

fn parse_atom(chars: &[char], pos: &mut usize, stop: Option<char>) -> Result<MiniNode> {
    let start = *pos;
    while *pos < chars.len() {
        let c = chars[*pos];
        if c.is_whitespace()
            || matches!(c, '[' | ']' | '<' | '>' | '*' | '/' | '@' | '(' | '?' | '!')
            || stop == Some(c)
        {
            break;
        }
        *pos += 1;
    }
    let token: String = chars[start..*pos].iter().collect();
    ensure!(
        !token.is_empty(),
        "Expected pitch or rest at column {}",
        start + 1
    );
    if token.contains(',') {
        let mut pitches = Vec::new();
        for part in token.split(',') {
            pitches.push(parse_pitch(part.trim())?);
        }
        ensure!(
            pitches.len() <= MAX_CHORD_TONES + 1,
            "Mini-notation chord can stack at most {} notes (1 main + {} chord tones)",
            MAX_CHORD_TONES + 1,
            MAX_CHORD_TONES
        );
        Ok(MiniNode::Chord(pitches))
    } else {
        Ok(MiniNode::Pitch(parse_pitch(&token)?))
    }
}

fn parse_modifiers(chars: &[char], pos: &mut usize, item: &mut WeightedNode) -> Result<()> {
    loop {
        if *pos >= chars.len() {
            break;
        }
        match chars[*pos] {
            '*' => {
                *pos += 1;
                let n = read_u16(chars, pos)?;
                ensure!((1..=16).contains(&n), "Repeat '*N' must be 1..=16");
                item.repeat = item.repeat.saturating_mul(n as u8).min(16);
            }
            '/' => {
                *pos += 1;
                let n = read_u16(chars, pos)?;
                ensure!((1..=16).contains(&n), "Slow '/N' must be 1..=16");
                item.slow = item.slow.saturating_mul(n as u8).min(16);
            }
            '@' => {
                *pos += 1;
                let w = read_f32(chars, pos)?;
                ensure!(
                    w.is_finite() && (0.125..=32.0).contains(&w),
                    "Weight '@N' must be 0.125..=32"
                );
                item.weight *= w;
            }
            '(' => {
                *pos += 1;
                let k = read_u16(chars, pos)?;
                expect_char(chars, pos, ',')?;
                let n = read_u16(chars, pos)?;
                let rot = if *pos < chars.len() && chars[*pos] == ',' {
                    *pos += 1;
                    read_i16(chars, pos)?
                } else {
                    0
                };
                expect_char(chars, pos, ')')?;
                ensure!(
                    n > 0 && n <= 64 && k <= n,
                    "Euclidean (k,n) requires 0 <= k <= n <= 64"
                );
                item.euclidean = Some((k, n, rot));
            }
            '?' => {
                *pos += 1;
                if *pos < chars.len() && (chars[*pos].is_ascii_digit() || chars[*pos] == '.') {
                    let p = read_f32(chars, pos)?;
                    ensure!(
                        p.is_finite() && (0.0..=1.0).contains(&p),
                        "Chance '?p' must be 0..=1"
                    );
                    item.chance *= p;
                } else {
                    item.chance *= 0.5;
                }
            }
            '!' => {
                *pos += 1;
                item.accent = true;
            }
            _ => break,
        }
    }
    Ok(())
}

fn expect_char(chars: &[char], pos: &mut usize, want: char) -> Result<()> {
    skip_ws(chars, pos);
    if chars.get(*pos) == Some(&want) {
        *pos += 1;
        Ok(())
    } else {
        bail!("Expected '{want}' at column {}", *pos + 1);
    }
}

fn read_u16(chars: &[char], pos: &mut usize) -> Result<u16> {
    skip_ws(chars, pos);
    let start = *pos;
    while *pos < chars.len() && chars[*pos].is_ascii_digit() {
        *pos += 1;
    }
    let s: String = chars[start..*pos].iter().collect();
    s.parse::<u16>()
        .map_err(|_| anyhow::anyhow!("Expected integer at column {}", start + 1))
}

fn read_i16(chars: &[char], pos: &mut usize) -> Result<i16> {
    skip_ws(chars, pos);
    let start = *pos;
    if *pos < chars.len() && matches!(chars[*pos], '+' | '-') {
        *pos += 1;
    }
    while *pos < chars.len() && chars[*pos].is_ascii_digit() {
        *pos += 1;
    }
    let s: String = chars[start..*pos].iter().collect();
    s.parse::<i16>()
        .map_err(|_| anyhow::anyhow!("Expected signed integer at column {}", start + 1))
}

fn read_f32(chars: &[char], pos: &mut usize) -> Result<f32> {
    skip_ws(chars, pos);
    let start = *pos;
    while *pos < chars.len() && (chars[*pos].is_ascii_digit() || chars[*pos] == '.') {
        *pos += 1;
    }
    let s: String = chars[start..*pos].iter().collect();
    s.parse::<f32>()
        .map_err(|_| anyhow::anyhow!("Expected number at column {}", start + 1))
}

/// Parse a note name (`c4`, `eb3`, `f#5`, `bb2`) OR scale degree (`1`, `3`, `-2`, `5#`, `3b`).
pub fn parse_pitch(raw: &str) -> Result<PitchSpec> {
    let s = raw.trim();
    ensure!(!s.is_empty(), "Empty pitch token");
    let bytes = s.as_bytes();
    let first = bytes[0].to_ascii_lowercase();
    if matches!(first, b'a'..=b'g') {
        let base = match first {
            b'c' => 0i32,
            b'd' => 2,
            b'e' => 4,
            b'f' => 5,
            b'g' => 7,
            b'a' => 9,
            b'b' => 11,
            _ => unreachable!(),
        };
        let mut idx = 1;
        let mut acc = 0i32;
        while idx < bytes.len() {
            match bytes[idx] {
                b'#' | b's' => {
                    acc += 1;
                    idx += 1;
                }
                b'b' => {
                    acc -= 1;
                    idx += 1;
                }
                _ => break,
            }
        }
        let oct_str = &s[idx..];
        let oct: i32 = if oct_str.is_empty() {
            4
        } else {
            oct_str
                .parse()
                .map_err(|_| anyhow::anyhow!("Invalid note pitch '{raw}'"))?
        };
        let midi = (oct + 1) * 12 + base + acc;
        ensure!(
            (0..=127).contains(&midi),
            "MIDI note '{raw}' out of range 0..=127"
        );
        return Ok(PitchSpec::Midi(midi as u8));
    }

    // Scale degree with optional trailing '#' or 'b' accidental (e.g. "1", "3", "-2", "4#", "7b").
    let (num_part, acc) = if let Some(stripped) = s.strip_suffix('#') {
        (stripped, 1i8)
    } else if let Some(stripped) = s.strip_suffix('b') {
        (stripped, -1i8)
    } else {
        (s, 0i8)
    };
    let degree: i16 = num_part
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid mini-notation token '{raw}'"))?;
    ensure!(
        (MIN_DEGREE..=MAX_DEGREE).contains(&degree),
        "Scale degree '{raw}' out of range ({MIN_DEGREE}..={MAX_DEGREE})"
    );
    Ok(PitchSpec::Degree {
        degree,
        accidental: acc,
    })
}

/// Number of pattern cycles required before `/N` slow and `<alt>` alternations repeat.
pub fn mini_cycle_multiplier(nodes: &[WeightedNode]) -> Result<u64> {
    let mut mult = 1u64;
    for item in nodes {
        mult = lcm_u64(mult, weighted_cycle_multiplier(item)?)?;
    }
    Ok(mult)
}

fn weighted_cycle_multiplier(item: &WeightedNode) -> Result<u64> {
    let slow = u64::from(item.slow.max(1));
    let inner = match &item.node {
        MiniNode::Rest | MiniNode::Pitch(_) | MiniNode::Chord(_) => 1,
        MiniNode::Seq(children) => mini_cycle_multiplier(children)?,
        MiniNode::Alt(choices) => {
            let mut m = choices.len().max(1) as u64;
            for choice in choices {
                m = lcm_u64(m, weighted_cycle_multiplier(choice)?)?;
            }
            m
        }
    };
    lcm_u64(slow, inner)
}

fn lcm_u64(a: u64, b: u64) -> Result<u64> {
    let (mut x, mut y) = (a.max(1), b.max(1));
    while y != 0 {
        let r = x % y;
        x = y;
        y = r;
    }
    (a.max(1) / x)
        .checked_mul(b.max(1))
        .context("Mini-notation cycle period exceeds clock range")
}

/// Evaluate a parsed mini-notation pattern (`steps` sixteenth-steps per cycle) at integer
/// 16th-note `time`, pushing all sub-step `EvaluatedEvent`s falling in `[time, time + 1)`.
pub fn evaluate_mini_into(
    nodes: &[WeightedNode],
    steps: u16,
    pattern_id: Id,
    time: i64,
    midi_to_degree: impl Fn(u8) -> (i16, i8) + Copy,
    out: &mut Vec<EvaluatedEvent>,
) {
    if nodes.is_empty() || steps == 0 {
        return;
    }
    let period = i64::from(steps);
    let cycle = time.div_euclid(period);
    let step_in_cycle = time.rem_euclid(period) as f64;
    let win_start = step_in_cycle;
    let win_end = step_in_cycle + 1.0;

    let mut ctx = EvalCtx {
        pattern_id,
        cycle,
        win_start,
        win_end,
        salt: 0,
        midi_to_degree,
        out,
    };
    eval_seq(nodes, 0.0, period as f64, false, 1.0, &mut ctx);
}

struct EvalCtx<'a, F: Fn(u8) -> (i16, i8) + Copy> {
    pattern_id: Id,
    cycle: i64,
    win_start: f64,
    win_end: f64,
    salt: u32,
    midi_to_degree: F,
    out: &'a mut Vec<EvaluatedEvent>,
}

fn eval_seq<F: Fn(u8) -> (i16, i8) + Copy>(
    nodes: &[WeightedNode],
    start: f64,
    dur: f64,
    accent: bool,
    chance: f32,
    ctx: &mut EvalCtx<'_, F>,
) {
    let total_weight: f64 = nodes.iter().map(|n| f64::from(n.weight)).sum();
    if total_weight <= 1e-9 {
        return;
    }
    let mut cursor = start;
    for item in nodes {
        let span = dur * (f64::from(item.weight) / total_weight);
        eval_weighted(item, cursor, span, accent, chance, ctx);
        cursor += span;
    }
}

const MINI_EPS: f64 = 1e-6;

fn eval_weighted<F: Fn(u8) -> (i16, i8) + Copy>(
    item: &WeightedNode,
    start: f64,
    dur: f64,
    parent_accent: bool,
    parent_chance: f32,
    ctx: &mut EvalCtx<'_, F>,
) {
    // Reject immediately if disjoint from [win_start - MINI_EPS, win_end - MINI_EPS).
    if start + dur <= ctx.win_start - MINI_EPS || start >= ctx.win_end - MINI_EPS {
        return;
    }
    if item.slow > 1 && ctx.cycle.rem_euclid(i64::from(item.slow)) != 0 {
        return;
    }
    let accent = parent_accent || item.accent;
    let chance = (parent_chance * item.chance).clamp(0.0, 1.0);
    let rep = usize::from(item.repeat.max(1));

    if let Some((k, n, rot)) = item.euclidean {
        let euc_dur = dur / f64::from(n);
        let sub_dur = euc_dur / rep as f64;
        for i in 0..n {
            if !euclidean_hit(k, n, rot, i64::from(i)) {
                continue;
            }
            let base = start + f64::from(i) * euc_dur;
            for r in 0..rep {
                let s = base + r as f64 * sub_dur;
                eval_node(&item.node, s, sub_dur, accent, chance, ctx);
            }
        }
        return;
    }

    let sub_dur = dur / rep as f64;
    for r in 0..rep {
        let s = start + r as f64 * sub_dur;
        eval_node(&item.node, s, sub_dur, accent, chance, ctx);
    }
}

fn eval_node<F: Fn(u8) -> (i16, i8) + Copy>(
    node: &MiniNode,
    start: f64,
    dur: f64,
    accent: bool,
    chance: f32,
    ctx: &mut EvalCtx<'_, F>,
) {
    match node {
        MiniNode::Rest => {}
        MiniNode::Seq(children) => eval_seq(children, start, dur, accent, chance, ctx),
        MiniNode::Alt(choices) => {
            if !choices.is_empty() {
                let idx = ctx.cycle.rem_euclid(choices.len() as i64) as usize;
                eval_weighted(&choices[idx], start, dur, accent, chance, ctx);
            }
        }
        MiniNode::Pitch(p) => {
            emit_hit(std::slice::from_ref(p), start, dur, accent, chance, ctx);
        }
        MiniNode::Chord(pitches) => {
            emit_hit(pitches, start, dur, accent, chance, ctx);
        }
    }
}

fn emit_hit<F: Fn(u8) -> (i16, i8) + Copy>(
    pitches: &[PitchSpec],
    start: f64,
    dur: f64,
    accent: bool,
    chance: f32,
    ctx: &mut EvalCtx<'_, F>,
) {
    if pitches.is_empty() {
        return;
    }
    // Emit only if onset `start` falls inside `[win_start, win_end)`.
    if start < ctx.win_start - MINI_EPS || start >= ctx.win_end - MINI_EPS {
        return;
    }
    let salt = ctx.salt;
    ctx.salt = ctx.salt.wrapping_add(1);
    if chance < 0.9999 {
        let roll = det_hash(
            0x4d49_4e49,
            ctx.pattern_id,
            ctx.cycle * 1024 + ctx.win_start as i64,
            salt,
        );
        if roll >= chance {
            return;
        }
    }
    let offset = (start - ctx.win_start).clamp(0.0, 0.9999) as f32;
    let vel = if accent { 0.98 } else { 0.80 };
    let gate = (dur as f32 * 0.85).clamp(0.08, 64.0);

    let resolve = |p: &PitchSpec| -> (i16, i8) {
        match *p {
            PitchSpec::Midi(m) => (ctx.midi_to_degree)(m),
            PitchSpec::Degree { degree, accidental } => (degree, accidental),
        }
    };
    let (degree, accidental) = resolve(&pitches[0]);
    let chord: Vec<ChordTone> = pitches[1..]
        .iter()
        .map(|p| {
            let (d, a) = resolve(p);
            ChordTone {
                degree: d,
                accidental: a,
                velocity: vel,
                gate,
            }
        })
        .collect();
    ctx.out.push(EvaluatedEvent {
        offset,
        step: Step {
            active: true,
            degree,
            accidental,
            velocity: vel,
            gate,
            chord,
            ..Step::default()
        },
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id_deg(m: u8) -> (i16, i8) {
        (i16::from(m) - 59, 0)
    }

    #[test]
    fn parses_subdivisions_chords_alternation_euclidean_and_chance() {
        // 4 top-level tokens over 16 steps -> each slot is 4 steps; [c4 e4 g4] fires at 0, 4/3, 8/3!
        let ast = parse_mini("[c4 e4 g4] ~ [c4,e4,g4]! <d4 a4>").unwrap();
        let mut s0 = Vec::new();
        evaluate_mini_into(&ast, 16, 1, 0, id_deg, &mut s0);
        assert_eq!(s0.len(), 1);
        assert!((s0[0].offset - 0.0).abs() < 1e-4);

        let mut s1 = Vec::new();
        evaluate_mini_into(&ast, 16, 1, 1, id_deg, &mut s1);
        assert_eq!(s1.len(), 1);
        assert!(
            (s1[0].offset - (1.0 / 3.0)).abs() < 1e-3,
            "triplet offset: {}",
            s1[0].offset
        );

        // Chord + accent at step 8
        let mut s8 = Vec::new();
        evaluate_mini_into(&ast, 16, 1, 8, id_deg, &mut s8);
        assert_eq!(s8.len(), 1);
        assert_eq!(s8[0].step.chord.len(), 2);
        assert!(s8[0].step.velocity > 0.95);

        // Alternating cycle at step 12 vs step 28 (cycle 0 vs cycle 1)
        let mut c0 = Vec::new();
        let mut c1 = Vec::new();
        evaluate_mini_into(&ast, 16, 1, 12, id_deg, &mut c0);
        evaluate_mini_into(&ast, 16, 1, 28, id_deg, &mut c1);
        assert_ne!(c0[0].step.degree, c1[0].step.degree);

        // Euclidean c4(3,8) over 8 steps -> 3 hits!
        let euc = parse_mini("c4(3,8)").unwrap();
        let hits: usize = (0..8)
            .map(|t| {
                let mut v = Vec::new();
                evaluate_mini_into(&euc, 8, 2, t, id_deg, &mut v);
                v.len()
            })
            .sum();
        assert_eq!(hits, 3);

        // Ratchet c4*4 over 1 step -> 4 sub-step events at 0.0, 0.25, 0.5, 0.75!
        let ratch = parse_mini("c4*4").unwrap();
        let mut v = Vec::new();
        evaluate_mini_into(&ratch, 1, 3, 0, id_deg, &mut v);
        assert_eq!(v.len(), 4);
        assert!((v[1].offset - 0.25).abs() < 1e-4);
        assert!((v[3].offset - 0.75).abs() < 1e-4);
    }

    #[test]
    fn mini_rejects_u8_truncation_oversized_chords_preserves_degree_zero_euc_repeat_and_period() {
        // Out-of-range multipliers (*257, /257, *0, /0) are rejected without u8 wrap.
        assert!(parse_mini("c4*257").is_err());
        assert!(parse_mini("c4/257").is_err());
        assert!(parse_mini("c4*0").is_err());
        assert!(parse_mini("c4/0").is_err());

        // Chords exceeding MAX_CHORD_TONES + 1 (6 notes) are rejected at parse time.
        assert!(parse_mini("[c4,d4,e4,f4,g4,a4]").is_ok());
        assert!(parse_mini("[c4,d4,e4,f4,g4,a4,b4]").is_err());
        assert!(parse_mini("c4,d4,e4,f4,g4,a4,b4").is_err());

        // Degree '0' stays scale degree 0 (leading tone below tonic 1).
        assert_eq!(
            parse_pitch("0").unwrap(),
            PitchSpec::Degree {
                degree: 0,
                accidental: 0
            }
        );

        // Euclidean combined with *N repeats every Euclidean pulse N times.
        let euc_rep = parse_mini("c4(3,8)*2").unwrap();
        let hits: usize = (0..8)
            .map(|t| {
                let mut v = Vec::new();
                evaluate_mini_into(&euc_rep, 8, 9, t, id_deg, &mut v);
                v.len()
            })
            .sum();
        assert_eq!(hits, 6, "c4(3,8)*2 must emit 3 * 2 = 6 hits");

        // Cycle multiplier accounts for <alt> and /slow.
        let alt = parse_mini("<c4 e4 g4>").unwrap();
        assert_eq!(mini_cycle_multiplier(&alt).unwrap(), 3);
        let slow = parse_mini("c4/2 e4/3").unwrap();
        assert_eq!(mini_cycle_multiplier(&slow).unwrap(), 6);
        let combined = parse_mini("<c4/3 e4>").unwrap();
        assert_eq!(mini_cycle_multiplier(&combined).unwrap(), 6);
    }
}
