//! Pattern type/mode queries, step/value evaluation, previewing, and pitch conversion.

use super::{
    Id, Lane, LiveFlags, Part, PatternKind, PatternType, PatternValue, Placement, PlacementSample,
    PreviewStep, Project, ValuePoint, ValueUnit, MAX_DEPTH, MAX_LITERAL_STEPS,
};
use crate::evaluate::EvaluatedEvent;
use crate::music::{self, Step};
use crate::params::ValueMode;
use anyhow::{bail, ensure, Context, Result};
use std::collections::{HashMap, HashSet};

/// Evaluate a sorted `Values` breakpoint sequence (`Hold` or wrapped `Ramp`) at arbitrary step time.
///
/// Returns `None` when `period == 0` or `points` is empty so callers can either report an error
/// (`Project::value_inner`) or supply a domain fallback (e.g., `cadence-formats`).
pub fn eval_value_points(
    period: u32,
    mode: ValueMode,
    points: &[ValuePoint],
    time: f64,
) -> Option<f32> {
    if period == 0 || points.is_empty() {
        return None;
    }
    let at = time.rem_euclid(f64::from(period));
    let prior = points
        .iter()
        .rposition(|p| f64::from(p.step) <= at)
        .unwrap_or(points.len() - 1);
    if mode != ValueMode::Ramp || points.len() == 1 {
        return Some(points[prior].value);
    }
    let next = (prior + 1) % points.len();
    let a = points[prior];
    let b = points[next];
    let span = if next > prior {
        f64::from(b.step - a.step)
    } else {
        f64::from(period - a.step + b.step)
    };
    let elapsed = (at - f64::from(a.step)).rem_euclid(f64::from(period));
    Some(a.value + (b.value - a.value) * (elapsed / span.max(1.0)) as f32)
}

impl Project {
    pub fn pattern_type(&self, id: Id) -> Result<PatternType> {
        self.signature(id, 0, &mut HashSet::new(), &mut HashMap::new())
            .map(|x| x.0)
    }
    pub fn pattern_period(&self, id: Id) -> Result<u64> {
        self.signature(id, 0, &mut HashSet::new(), &mut HashMap::new())
            .map(|x| x.1)
    }
    /// Editor coordinate period: literal `Notes`/`Rhythm` use stored length while
    /// derived patterns use their full repeat period.
    pub(crate) fn editor_period(&self, id: Id) -> Result<usize> {
        let pattern = self.pattern(id).context("Source pattern missing")?;
        let period = match &pattern.kind {
            PatternKind::Notes { steps } => steps.len(),
            PatternKind::Rhythm { steps } => steps.len(),
            _ => usize::try_from(self.pattern_period(id)?)
                .context("Source period exceeds editor coordinates")?,
        };
        ensure!(period > 0, "Source period must be positive");
        Ok(period)
    }
    /// Effective interpolation mode for a `Values` pattern, preserving `ValueMode::Ramp`
    /// through derived `Shift`, `Reverse`, `Every`, and `Concat` wrappers.
    pub fn value_mode(&self, id: Id) -> Result<ValueMode> {
        self.value_mode_depth(id, 0)
    }
    fn value_mode_depth(&self, id: Id, depth: usize) -> Result<ValueMode> {
        ensure!(depth < MAX_DEPTH, "Pattern depth exceeds {MAX_DEPTH}");
        let p = self
            .pattern(id)
            .with_context(|| format!("Missing pattern {id}"))?;
        match &p.kind {
            PatternKind::Values { mode, .. } => Ok(*mode),
            PatternKind::SeededValues { .. } => Ok(ValueMode::Hold),
            PatternKind::Shift { source, .. }
            | PatternKind::Reverse { source }
            | PatternKind::TimeScale { source, .. } => self.value_mode_depth(*source, depth + 1),
            PatternKind::Every { base, alt, .. } => {
                if self.value_mode_depth(*base, depth + 1)? == ValueMode::Ramp
                    || self.value_mode_depth(*alt, depth + 1)? == ValueMode::Ramp
                {
                    Ok(ValueMode::Ramp)
                } else {
                    Ok(ValueMode::Hold)
                }
            }
            PatternKind::Concat { parts } => {
                for &part in parts {
                    if self.value_mode_depth(part, depth + 1)? == ValueMode::Ramp {
                        return Ok(ValueMode::Ramp);
                    }
                }
                Ok(ValueMode::Hold)
            }
            _ => Ok(ValueMode::Hold),
        }
    }
    pub fn evaluate_pattern(&self, id: Id, time: i64) -> Result<PatternValue> {
        self.pattern_type(id)?;
        self.evaluate_inner(id, time, 0)
    }
    pub(super) fn evaluate_inner(&self, id: Id, time: i64, depth: usize) -> Result<PatternValue> {
        self.evaluate_memoized(id, time, depth, &mut HashMap::new())
    }
    fn evaluate_memoized(
        &self,
        id: Id,
        time: i64,
        depth: usize,
        memo: &mut HashMap<(Id, i64), PatternValue>,
    ) -> Result<PatternValue> {
        ensure!(
            depth < MAX_DEPTH,
            "Pattern evaluation exceeds dependency bound"
        );
        // Key by `(id, time)` because temporal operators (`Shift`, `Reverse`,
        // `TimeScale`, `Concat`) sample shared DAG sources at different times.
        if let Some(value) = memo.get(&(id, time)) {
            return Ok(value.clone());
        }
        let earliest_step = |buf: Vec<EvaluatedEvent>| -> Step {
            buf.into_iter()
                .min_by(|a, b| a.offset.total_cmp(&b.offset))
                .map_or_else(Step::default, |e| e.step)
        };
        let p = self
            .pattern(id)
            .with_context(|| format!("Missing pattern {id}"))?;
        let value = match &p.kind {
            PatternKind::Notes { steps } => {
                ensure!(!steps.is_empty(), "Empty note source");
                let mut step = steps[time.rem_euclid(steps.len() as i64) as usize].clone();
                let cycle = time.div_euclid(steps.len() as i64);
                let condition = step.condition;
                step.active &= cycle.rem_euclid(i64::from(condition.every.max(1)))
                    == i64::from(condition.turn.saturating_sub(1));
                step.source_part = None;
                PatternValue::Notes(step)
            }
            PatternKind::Rhythm { steps } => {
                ensure!(!steps.is_empty(), "Empty rhythm source");
                PatternValue::Rhythm(steps[time.rem_euclid(steps.len() as i64) as usize])
            }
            PatternKind::Euclidean {
                pulses,
                steps,
                rotation,
            } => PatternValue::Rhythm(crate::mini::euclidean_hit(*pulses, *steps, *rotation, time)),
            PatternKind::Values { .. } | PatternKind::SeededValues { .. } => {
                PatternValue::Value(self.value_inner(id, time as f64)?)
            }
            PatternKind::Mini { .. }
            | PatternKind::Swing { .. }
            | PatternKind::Ratchet { .. }
            | PatternKind::Arp { .. }
            | PatternKind::RangeFold { .. }
            | PatternKind::Shape { .. } => {
                let mut buf = Vec::with_capacity(2);
                self.evaluate_notes_events_depth(id, time, depth, &mut buf)?;
                PatternValue::Notes(earliest_step(buf))
            }
            PatternKind::TimeScale { source, num, den } => match self.pattern_type(*source)? {
                PatternType::Rhythm => {
                    // Match `Notes` `TimeScale` onset-interval semantics `[time * rate, (time + 1) * rate)`.
                    let n = i128::from((*num).max(1));
                    let d = i128::from((*den).max(1));
                    let t = i128::from(time);
                    let first = (t * n + d - 1).div_euclid(d) as i64;
                    let last = ((t + 1) * n - 1).div_euclid(d) as i64;
                    let mut active = false;
                    for st in first..=last {
                        if self
                            .evaluate_memoized(*source, st, depth + 1, memo)?
                            .active()
                        {
                            active = true;
                            break;
                        }
                    }
                    PatternValue::Rhythm(active)
                }
                PatternType::Values(_) => PatternValue::Value(self.value_inner(id, time as f64)?),
                PatternType::Notes => {
                    let mut buf = Vec::with_capacity(2);
                    self.evaluate_notes_events_depth(id, time, depth, &mut buf)?;
                    PatternValue::Notes(earliest_step(buf))
                }
            },
            PatternKind::Shift { source, steps } => {
                let shifted = time - i64::from(*steps);
                self.evaluate_memoized(*source, shifted, depth + 1, memo)?
            }
            PatternKind::Reverse { source } => {
                if self.pattern_type(*source)? == PatternType::Notes {
                    let mut buf = Vec::with_capacity(2);
                    self.evaluate_notes_events_depth(id, time, depth, &mut buf)?;
                    PatternValue::Notes(earliest_step(buf))
                } else {
                    let period = self.pattern_period(*source)? as i64;
                    let cycle = time.div_euclid(period);
                    let idx = time.rem_euclid(period);
                    let rev = cycle * period + (period - 1 - idx);
                    self.evaluate_memoized(*source, rev, depth + 1, memo)?
                }
            }
            PatternKind::Chance {
                source,
                probability,
                seed,
            } => {
                let keep = crate::evaluate::det_hash(*seed, id, time, 0) < *probability;
                match self.evaluate_memoized(*source, time, depth + 1, memo)? {
                    PatternValue::Notes(mut s) => {
                        s.active &= keep;
                        PatternValue::Notes(s)
                    }
                    PatternValue::Rhythm(b) => PatternValue::Rhythm(b && keep),
                    other => other,
                }
            }
            PatternKind::Every {
                interval,
                base,
                alt,
            } => {
                let target = self.resolve_every_target(*interval, *base, *alt, time)?;
                self.evaluate_memoized(target, time, depth + 1, memo)?
            }
            PatternKind::Concat { parts } => {
                let (chosen, rem) = self.resolve_concat_step(parts, time)?;
                self.evaluate_memoized(chosen, rem, depth + 1, memo)?
            }
            PatternKind::Stack { parts } => {
                if self.pattern_type(parts[0])? == PatternType::Rhythm {
                    let mut any = false;
                    for &part in parts {
                        if self
                            .evaluate_memoized(part, time, depth + 1, memo)?
                            .active()
                        {
                            any = true;
                            break;
                        }
                    }
                    PatternValue::Rhythm(any)
                } else {
                    let mut buf = Vec::with_capacity(4);
                    self.evaluate_notes_events_depth(id, time, depth, &mut buf)?;
                    PatternValue::Notes(earliest_step(buf))
                }
            }
            PatternKind::AddDegrees { notes, values } => {
                let PatternValue::Notes(mut step) =
                    self.evaluate_memoized(*notes, time, depth + 1, memo)?
                else {
                    bail!("Degree-add source is not Notes")
                };
                let offset = self.value_inner(*values, time as f64)?.round() as i16;
                shift_degrees(&mut step, offset);
                PatternValue::Notes(step)
            }
            PatternKind::Mask { notes, rhythm } => {
                let PatternValue::Notes(mut step) =
                    self.evaluate_memoized(*notes, time, depth + 1, memo)?
                else {
                    bail!("Mask source is not Notes")
                };
                step.active &= self
                    .evaluate_memoized(*rhythm, time, depth + 1, memo)?
                    .active();
                PatternValue::Notes(step)
            }
            PatternKind::Transpose { source, semitones } => {
                let PatternValue::Notes(mut step) =
                    self.evaluate_memoized(*source, time, depth + 1, memo)?
                else {
                    bail!("Transpose source is not Notes")
                };
                self.transpose_step(&mut step, *semitones);
                PatternValue::Notes(step)
            }
            PatternKind::NotesFromValues {
                values,
                rhythm,
                degree_offset,
                velocity,
                gate,
            } => {
                let degree = (self.value_inner(*values, time as f64)?.round() as i32
                    + i32::from(*degree_offset)
                    + 1)
                .clamp(i32::from(music::MIN_DEGREE), i32::from(music::MAX_DEGREE))
                    as i16;
                PatternValue::Notes(Step {
                    active: self
                        .evaluate_memoized(*rhythm, time, depth + 1, memo)?
                        .active(),
                    degree,
                    velocity: *velocity,
                    gate: *gate,
                    ..Step::default()
                })
            }
        };
        memo.insert((id, time), value.clone());
        Ok(value)
    }
    pub fn value_at(&self, id: Id, time: f64) -> Result<f32> {
        ensure!(time.is_finite(), "Value-query time must be finite");
        ensure!(
            matches!(self.pattern_type(id)?, PatternType::Values(_)),
            "Expected a value pattern"
        );
        self.value_inner(id, time)
    }
    pub(crate) fn value_inner(&self, id: Id, time: f64) -> Result<f32> {
        let p = self.pattern(id).context("Value source is missing")?;
        match &p.kind {
            PatternKind::Values {
                period,
                mode,
                points,
                ..
            } => eval_value_points(*period, *mode, points, time).context("Value pattern is empty"),
            PatternKind::SeededValues {
                unit,
                period,
                seed,
                min,
                max,
            } => {
                ensure!(*period > 0, "Seed period is zero");
                let index = time.floor().rem_euclid(f64::from(*period)) as u64;
                let v = crate::evaluate::splitmix64(
                    seed.wrapping_add(index.wrapping_mul(crate::evaluate::GOLDEN_RATIO_64)),
                );
                let value = *min + (*max - *min) * ((v >> 40) as f32 / 16_777_215.0);
                Ok(if *unit == ValueUnit::Degrees {
                    value.round()
                } else {
                    value
                })
            }
            PatternKind::Shift { source, steps } => {
                self.value_inner(*source, time - f64::from(*steps))
            }
            PatternKind::TimeScale { source, num, den } => {
                let rate = f64::from((*num).max(1)) / f64::from((*den).max(1));
                self.value_inner(*source, time * rate)
            }
            PatternKind::Reverse { source } => {
                let period = self.pattern_period(*source)? as f64;
                let cycle = (time / period).floor();
                let idx = time.rem_euclid(period);
                let rev_in_period = if self.value_mode(*source)? == ValueMode::Ramp {
                    // Continuous period involution: exact on integers `[0..P) -> [P-1..0]`
                    // and smooth across fractional sub-steps without clamping the final step.
                    (period - 1.0 - idx).rem_euclid(period)
                } else {
                    // Step-held / seeded values: reflect step slot while keeping fractional
                    // position inside that reversed step `[rev_step, rev_step + 1)`.
                    let step = idx.floor();
                    let frac = idx - step;
                    (period - 1.0 - step).rem_euclid(period) + frac
                };
                self.value_inner(*source, cycle * period + rev_in_period)
            }
            PatternKind::Every {
                interval,
                base,
                alt,
            } => {
                let target =
                    self.resolve_every_target(*interval, *base, *alt, time.floor() as i64)?;
                self.value_inner(target, time)
            }
            PatternKind::Concat { parts } => {
                let (chosen, rem) = self.resolve_concat_time(parts, time)?;
                self.value_inner(chosen, rem)
            }
            _ => bail!("Expected a value pattern"),
        }
    }
    pub fn preview(&self, id: Id, start: i64, count: usize) -> Result<Vec<PreviewStep>> {
        ensure!(
            count <= MAX_LITERAL_STEPS,
            "Preview range exceeds {MAX_LITERAL_STEPS} steps"
        );
        self.pattern_type(id)?;
        (0..count)
            .map(|i| {
                let time = start
                    .checked_add(i as i64)
                    .context("Preview clock overflow")?;
                Ok(PreviewStep {
                    time,
                    value: self.evaluate_inner(id, time, 0)?,
                })
            })
            .collect()
    }
    pub fn preview_note_steps(&self, id: Id, start: i64, count: usize) -> Result<Vec<Step>> {
        ensure!(
            self.pattern_type(id)? == PatternType::Notes,
            "Expected Notes material"
        );
        self.preview(id, start, count)?
            .into_iter()
            .map(|p| match p.value {
                PatternValue::Notes(step) => Ok(step),
                _ => bail!("Expected Notes"),
            })
            .collect()
    }
    pub fn resolve_placement(&self, id: Id, local_step: u32) -> Result<PlacementSample> {
        self.resolve_placement_with_flags(id, local_step, &LiveFlags::default())
    }
    pub fn resolve_placement_with_flags(
        &self,
        id: Id,
        local_step: u32,
        flags: &LiveFlags,
    ) -> Result<PlacementSample> {
        let (lane, place) = self.placement(id).context("Placement does not exist")?;
        let part = self.part(place.part).context("Part is missing")?;
        ensure!(
            self.pattern_type(part.pattern)? == PatternType::Notes,
            "Part source is not Notes"
        );
        self.sample_placement(lane, place, part, local_step, flags)
    }
    fn sample_placement(
        &self,
        lane: &Lane,
        place: &Placement,
        part: &Part,
        local_step: u32,
        flags: &LiveFlags,
    ) -> Result<PlacementSample> {
        ensure!(
            local_step < place.length_steps,
            "Step is outside this placement"
        );
        let song_step = place
            .start_step
            .checked_add(local_step)
            .context("Placement clock overflow")?;
        let pattern_time = place.pattern_time(song_step);
        let local = place.exceptions.iter().find(|o| o.step == local_step);
        let mut value = if let Some(local) = local {
            local.value.clone()
        } else {
            match self.evaluate_inner(part.pattern, pattern_time, 0)? {
                PatternValue::Notes(step) => step,
                _ => bail!("Part source is not Notes"),
            }
        };
        self.interpret(part, &mut value);
        let eligible = value.active && flags.lane_enabled(lane.id);
        Ok(PlacementSample {
            lane: lane.id,
            placement: place.id,
            part: part.id,
            instrument: part.instrument,
            song_step,
            pattern_time,
            value,
            eligible,
            local_exception: local.is_some(),
        })
    }
    pub(super) fn resolve_every_target(
        &self,
        interval: u16,
        base: Id,
        alt: Id,
        step_time: i64,
    ) -> Result<Id> {
        let period = self.pattern_period(base)? as i64;
        let cycle = step_time.div_euclid(period);
        let iv = i64::from(interval.max(1));
        Ok(if cycle.rem_euclid(iv) == iv - 1 {
            alt
        } else {
            base
        })
    }

    pub(super) fn resolve_concat_step(&self, parts: &[Id], time: i64) -> Result<(Id, i64)> {
        let mut periods = Vec::with_capacity(parts.len());
        let mut total = 0i64;
        for &part in parts {
            let p = self.pattern_period(part)? as i64;
            periods.push(p);
            total += p;
        }
        let mut rem = time.rem_euclid(total.max(1));
        for (&part, &p) in parts.iter().zip(&periods) {
            if rem < p {
                return Ok((part, rem));
            }
            rem -= p;
        }
        Ok((parts[0], rem))
    }

    fn resolve_concat_time(&self, parts: &[Id], time: f64) -> Result<(Id, f64)> {
        let mut periods = Vec::with_capacity(parts.len());
        let mut total = 0.0_f64;
        for &part in parts {
            let p = self.pattern_period(part)? as f64;
            periods.push(p);
            total += p;
        }
        let mut rem = time.rem_euclid(total.max(1.0));
        for (&part, &p) in parts.iter().zip(&periods) {
            if rem < p {
                return Ok((part, rem));
            }
            rem -= p;
        }
        Ok((parts[0], rem))
    }

    pub fn pitch_midi(&self, degree: i16, accidental: i8, semitones: i32) -> u8 {
        music::pitch_midi(self.root_midi, &self.scale, degree, accidental, semitones)
    }
    pub fn midi_to_degree(&self, midi: u8) -> (i16, i8) {
        music::midi_to_degree(self.root_midi, &self.scale, midi, 0)
    }
    pub(super) fn transpose_step(&self, step: &mut Step, semitones: i16) {
        if semitones == 0 {
            return;
        }
        (step.degree, step.accidental) = self.midi_to_degree(self.pitch_midi(
            step.degree,
            step.accidental,
            i32::from(semitones),
        ));
        for tone in &mut step.chord {
            (tone.degree, tone.accidental) = self.midi_to_degree(self.pitch_midi(
                tone.degree,
                tone.accidental,
                i32::from(semitones),
            ));
        }
    }
}

pub(super) fn shift_degrees(step: &mut Step, offset: i16) {
    let clamp_deg = |deg: i16| -> i16 {
        (i32::from(deg) + i32::from(offset))
            .clamp(i32::from(music::MIN_DEGREE), i32::from(music::MAX_DEGREE)) as i16
    };
    step.degree = clamp_deg(step.degree);
    for tone in &mut step.chord {
        tone.degree = clamp_deg(tone.degree);
    }
}
