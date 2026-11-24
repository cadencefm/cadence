//! Sub-step multi-event (`EvaluatedEvent`) and rhythm-offset pattern evaluation.

use super::eval::shift_degrees;
use super::{ArpMode, Id, PatternKind, PatternType, PatternValue, Project, MAX_DEPTH};
use crate::evaluate::EvaluatedEvent;
use crate::music::{self, Step};
use anyhow::{bail, ensure, Context, Result};

impl Project {
    pub(crate) fn evaluate_notes_events_into(
        &self,
        id: Id,
        time: i64,
        out: &mut Vec<EvaluatedEvent>,
    ) -> Result<()> {
        self.evaluate_notes_events_depth(id, time, 0, out)
    }

    fn evaluate_rhythm_offsets_depth(
        &self,
        id: Id,
        time: i64,
        depth: usize,
        out: &mut Vec<f32>,
    ) -> Result<()> {
        ensure!(depth < MAX_DEPTH, "Pattern depth exceeds {MAX_DEPTH}");
        let p = self
            .pattern(id)
            .with_context(|| format!("Missing pattern {id}"))?;
        if self.pattern_type(id)? == PatternType::Notes {
            let mut tmp = Vec::with_capacity(4);
            self.evaluate_notes_events_depth(id, time, depth, &mut tmp)?;
            for ev in tmp {
                if ev.step.active {
                    out.push(ev.offset);
                }
            }
            return Ok(());
        }
        match &p.kind {
            PatternKind::Rhythm { .. } | PatternKind::Euclidean { .. } => {
                if self.evaluate_inner(id, time, depth)?.active() {
                    out.push(0.0);
                }
            }
            PatternKind::TimeScale { source, num, den } => {
                let n = f64::from((*num).max(1));
                let d = f64::from((*den).max(1));
                let rate = n / d;
                let src_start = (time as f64 * n) / d;
                let src_end = ((time + 1) as f64 * n) / d;
                let first_step = (src_start - 1e-6).floor() as i64;
                let last_step = (src_end - 1e-6).floor() as i64;
                let mut tmp = Vec::with_capacity(4);
                for st in first_step..=last_step {
                    tmp.clear();
                    self.evaluate_rhythm_offsets_depth(*source, st, depth + 1, &mut tmp)?;
                    for off in tmp.drain(..) {
                        let exact_src = st as f64 + f64::from(off);
                        if exact_src >= src_start - 1e-6 && exact_src < src_end - 1e-6 {
                            let new_off =
                                ((exact_src - src_start) / rate).clamp(0.0, 0.9999) as f32;
                            out.push(new_off);
                        }
                    }
                }
            }
            PatternKind::Shift { source, steps } => {
                self.evaluate_rhythm_offsets_depth(
                    *source,
                    time - i64::from(*steps),
                    depth + 1,
                    out,
                )?;
            }
            PatternKind::Reverse { source } => {
                let period = self.pattern_period(*source)? as i64;
                let cycle = time.div_euclid(period);
                let idx = time.rem_euclid(period);
                let rev_zero = cycle * period + (period - 1 - idx);
                let rev_frac = cycle * period + (period - 2 - idx).rem_euclid(period);
                let mut tmp = Vec::with_capacity(4);
                if rev_zero == rev_frac {
                    self.evaluate_rhythm_offsets_depth(*source, rev_zero, depth + 1, &mut tmp)?;
                    for off in tmp {
                        out.push(if off <= 1e-6 {
                            0.0
                        } else {
                            (1.0 - off).clamp(0.0, 0.9999)
                        });
                    }
                } else {
                    self.evaluate_rhythm_offsets_depth(*source, rev_zero, depth + 1, &mut tmp)?;
                    for off in tmp.drain(..) {
                        if off <= 1e-6 {
                            out.push(0.0);
                        }
                    }
                    self.evaluate_rhythm_offsets_depth(*source, rev_frac, depth + 1, &mut tmp)?;
                    for off in tmp.drain(..) {
                        if off > 1e-6 {
                            out.push((1.0 - off).clamp(0.0, 0.9999));
                        }
                    }
                }
            }
            PatternKind::Chance {
                source,
                probability,
                seed,
            } => {
                let mut tmp = Vec::with_capacity(4);
                self.evaluate_rhythm_offsets_depth(*source, time, depth + 1, &mut tmp)?;
                tmp.sort_by(f32::total_cmp);
                for (i, off) in tmp.into_iter().enumerate() {
                    if crate::evaluate::det_hash(*seed, id, time, i as u32) < *probability {
                        out.push(off);
                    }
                }
            }
            PatternKind::Every {
                interval,
                base,
                alt,
            } => {
                let target = self.resolve_every_target(*interval, *base, *alt, time)?;
                self.evaluate_rhythm_offsets_depth(target, time, depth + 1, out)?;
            }
            PatternKind::Concat { parts } => {
                let (chosen, rem) = self.resolve_concat_step(parts, time)?;
                self.evaluate_rhythm_offsets_depth(chosen, rem, depth + 1, out)?;
            }
            PatternKind::Stack { parts } => {
                for &part in parts {
                    self.evaluate_rhythm_offsets_depth(part, time, depth + 1, out)?;
                }
            }
            _ => bail!("Expected Rhythm or Notes source"),
        }
        Ok(())
    }

    pub(super) fn evaluate_notes_events_depth(
        &self,
        id: Id,
        time: i64,
        depth: usize,
        out: &mut Vec<EvaluatedEvent>,
    ) -> Result<()> {
        ensure!(depth < MAX_DEPTH, "Pattern depth exceeds {MAX_DEPTH}");
        let p = self
            .pattern(id)
            .with_context(|| format!("Missing pattern {id}"))?;
        match &p.kind {
            PatternKind::Notes { .. } => {
                if let PatternValue::Notes(step) = self.evaluate_inner(id, time, depth)? {
                    if step.active {
                        out.push(EvaluatedEvent { offset: 0.0, step });
                    }
                }
            }
            PatternKind::NotesFromValues {
                values,
                rhythm,
                degree_offset,
                velocity,
                gate,
            } => {
                let mut offsets = Vec::with_capacity(4);
                self.evaluate_rhythm_offsets_depth(*rhythm, time, depth + 1, &mut offsets)?;
                for offset in offsets {
                    let degree = (self
                        .value_inner(*values, time as f64 + f64::from(offset))?
                        .round() as i32
                        + i32::from(*degree_offset)
                        + 1)
                    .clamp(i32::from(music::MIN_DEGREE), i32::from(music::MAX_DEGREE))
                        as i16;
                    out.push(EvaluatedEvent {
                        offset,
                        step: Step {
                            active: true,
                            degree,
                            velocity: *velocity,
                            gate: *gate,
                            ..Step::default()
                        },
                    });
                }
            }
            PatternKind::Mini { code, steps } => {
                let ast = crate::mini::parse_mini(code)?;
                crate::mini::evaluate_mini_into(
                    &ast,
                    *steps,
                    id,
                    time,
                    |m| self.midi_to_degree(m),
                    out,
                );
            }
            PatternKind::AddDegrees { notes, values } => {
                let start = out.len();
                self.evaluate_notes_events_depth(*notes, time, depth + 1, out)?;
                for ev in &mut out[start..] {
                    let offset = self
                        .value_inner(*values, time as f64 + f64::from(ev.offset))?
                        .round() as i16;
                    shift_degrees(&mut ev.step, offset);
                }
            }
            PatternKind::Mask { notes, rhythm } => {
                let active = self.evaluate_inner(*rhythm, time, depth + 1)?.active();
                if active {
                    self.evaluate_notes_events_depth(*notes, time, depth + 1, out)?;
                }
            }
            PatternKind::Transpose { source, semitones } => {
                let start = out.len();
                self.evaluate_notes_events_depth(*source, time, depth + 1, out)?;
                for ev in &mut out[start..] {
                    self.transpose_step(&mut ev.step, *semitones);
                }
            }
            PatternKind::TimeScale { source, num, den } => {
                let n = f64::from((*num).max(1));
                let d = f64::from((*den).max(1));
                let rate = n / d;
                let src_start = (time as f64 * n) / d;
                let src_end = ((time + 1) as f64 * n) / d;
                let first_step = (src_start - 1e-6).floor() as i64;
                let last_step = (src_end - 1e-6).floor() as i64;
                let mut tmp = Vec::with_capacity(4);
                for st in first_step..=last_step {
                    tmp.clear();
                    self.evaluate_notes_events_depth(*source, st, depth + 1, &mut tmp)?;
                    for mut ev in tmp.drain(..) {
                        let exact_src = st as f64 + f64::from(ev.offset);
                        if exact_src >= src_start - 1e-6 && exact_src < src_end - 1e-6 {
                            let new_off =
                                ((exact_src - src_start) / rate).clamp(0.0, 0.9999) as f32;
                            ev.offset = new_off;
                            ev.step.gate = (ev.step.gate / rate as f32).clamp(0.05, 64.0);
                            for tone in &mut ev.step.chord {
                                tone.gate = (tone.gate / rate as f32).clamp(0.05, 64.0);
                            }
                            out.push(ev);
                        }
                    }
                }
            }
            PatternKind::Swing { source, amount } => {
                let shift = amount.clamp(0.0, 0.45);
                if shift <= 0.0 {
                    self.evaluate_notes_events_depth(*source, time, depth + 1, out)?;
                } else if time.rem_euclid(2) == 1 {
                    // Odd step: delay hits by `shift`; hits carrying into `time + 1`
                    // (`offset + shift >= 1.0`) are picked up on the following even step.
                    let mut tmp = Vec::with_capacity(4);
                    self.evaluate_notes_events_depth(*source, time, depth + 1, &mut tmp)?;
                    for mut ev in tmp {
                        let shifted = f64::from(ev.offset) + f64::from(shift);
                        if shifted < 1.0 {
                            ev.offset = (shifted as f32).clamp(0.0, 0.9999);
                            out.push(ev);
                        }
                    }
                } else {
                    // Even step: pull carry-ins from the preceding odd step (`time - 1`)
                    // alongside unshifted hits on `time`.
                    let mut prev = Vec::with_capacity(4);
                    self.evaluate_notes_events_depth(*source, time - 1, depth + 1, &mut prev)?;
                    for mut ev in prev {
                        let shifted = f64::from(ev.offset) + f64::from(shift);
                        if shifted >= 1.0 {
                            ev.offset = ((shifted - 1.0) as f32).clamp(0.0, 0.9999);
                            out.push(ev);
                        }
                    }
                    self.evaluate_notes_events_depth(*source, time, depth + 1, out)?;
                }
            }
            PatternKind::Ratchet { source, count } => {
                let n = (*count).clamp(1, 8);
                let step_span = 1.0 / f32::from(n);
                let emit_ratchet =
                    |ev: EvaluatedEvent, carry_from_prev: bool, dest: &mut Vec<EvaluatedEvent>| {
                        for k in 0..n {
                            let raw = f64::from(ev.offset) + f64::from(k) / f64::from(n);
                            let off = if carry_from_prev {
                                if raw < 1.0 {
                                    continue;
                                }
                                ((raw - 1.0) as f32).clamp(0.0, 0.9999)
                            } else {
                                if raw >= 1.0 {
                                    continue;
                                }
                                (raw as f32).clamp(0.0, 0.9999)
                            };
                            let mut hit = ev.step.clone();
                            let scale = (1.0 - 0.05 * f32::from(k)).max(0.5);
                            hit.velocity = (hit.velocity * scale).clamp(0.05, 1.0);
                            hit.gate = (hit.gate * step_span).clamp(0.05, 64.0);
                            for tone in &mut hit.chord {
                                tone.velocity = (tone.velocity * scale).clamp(0.05, 1.0);
                                tone.gate = (tone.gate * step_span).clamp(0.05, 64.0);
                            }
                            dest.push(EvaluatedEvent {
                                offset: off,
                                step: hit,
                            });
                        }
                    };
                let mut tmp = Vec::with_capacity(4);
                if n > 1 {
                    self.evaluate_notes_events_depth(*source, time - 1, depth + 1, &mut tmp)?;
                    for ev in tmp.drain(..) {
                        emit_ratchet(ev, true, out);
                    }
                }
                self.evaluate_notes_events_depth(*source, time, depth + 1, &mut tmp)?;
                for ev in tmp.drain(..) {
                    emit_ratchet(ev, false, out);
                }
            }
            PatternKind::Shift { source, steps } => {
                self.evaluate_notes_events_depth(
                    *source,
                    time - i64::from(*steps),
                    depth + 1,
                    out,
                )?;
            }
            PatternKind::Reverse { source } => {
                // Exact period involution `t' = cycle * P + (P - 1 - t_in_cycle) mod P`:
                // on-beat hits (`offset == 0`) stay on beat at `rev_zero = P - 1 - idx`,
                // while fractional sub-step hits (`offset > 0`) mirror to `1 - offset`
                // from `rev_frac = (P - 2 - idx) mod P` so all inter-onset intervals reverse
                // cleanly and `Reverse(Reverse(x)) == x`.
                let period = self.pattern_period(*source)? as i64;
                let cycle = time.div_euclid(period);
                let idx = time.rem_euclid(period);
                let rev_zero = cycle * period + (period - 1 - idx);
                let rev_frac = cycle * period + (period - 2 - idx).rem_euclid(period);
                let mut tmp = Vec::with_capacity(4);
                if rev_zero == rev_frac {
                    self.evaluate_notes_events_depth(*source, rev_zero, depth + 1, &mut tmp)?;
                    for mut ev in tmp.drain(..) {
                        if ev.offset > 1e-6 {
                            ev.offset = (1.0 - ev.offset).clamp(0.0, 0.9999);
                        } else {
                            ev.offset = 0.0;
                        }
                        out.push(ev);
                    }
                } else {
                    self.evaluate_notes_events_depth(*source, rev_zero, depth + 1, &mut tmp)?;
                    for mut ev in tmp.drain(..) {
                        if ev.offset <= 1e-6 {
                            ev.offset = 0.0;
                            out.push(ev);
                        }
                    }
                    self.evaluate_notes_events_depth(*source, rev_frac, depth + 1, &mut tmp)?;
                    for mut ev in tmp.drain(..) {
                        if ev.offset > 1e-6 {
                            ev.offset = (1.0 - ev.offset).clamp(0.0, 0.9999);
                            out.push(ev);
                        }
                    }
                }
            }
            PatternKind::Chance {
                source,
                probability,
                seed,
            } => {
                let mut tmp = Vec::with_capacity(4);
                self.evaluate_notes_events_depth(*source, time, depth + 1, &mut tmp)?;
                tmp.sort_by(|a, b| a.offset.total_cmp(&b.offset));
                for (i, ev) in tmp.into_iter().enumerate() {
                    if crate::evaluate::det_hash(*seed, id, time, i as u32) < *probability {
                        out.push(ev);
                    }
                }
            }
            PatternKind::Every {
                interval,
                base,
                alt,
            } => {
                let target = self.resolve_every_target(*interval, *base, *alt, time)?;
                self.evaluate_notes_events_depth(target, time, depth + 1, out)?;
            }
            PatternKind::Arp { source, mode, rate } => {
                // Collect every note currently sounding at `time` (struck on `time` or held
                // across its gate from any prior step — do not stop lookback at a short note
                // when an earlier chord is still held underneath!).
                let period = self.pattern_period(*source)? as i64;
                let mut pitches: Vec<(i16, i8, f32)> = Vec::new();
                let mut probe = Vec::with_capacity(4);
                for back in 0..period.min(65) {
                    probe.clear();
                    self.evaluate_notes_events_depth(*source, time - back, depth + 1, &mut probe)?;
                    for ev in &probe {
                        let held = back == 0 || (ev.offset + ev.step.gate) > back as f32;
                        if held {
                            pitches.push((ev.step.degree, ev.step.accidental, ev.step.velocity));
                        }
                        for t in &ev.step.chord {
                            if back == 0 || (ev.offset + t.gate) > back as f32 {
                                pitches.push((t.degree, t.accidental, t.velocity));
                            }
                        }
                    }
                }
                if pitches.is_empty() {
                    return Ok(());
                }
                pitches.sort_by_key(|&(d, a, _)| self.pitch_midi(d, a, 0));
                pitches.dedup_by_key(|(d, a, _)| self.pitch_midi(*d, *a, 0));
                let r = (*rate).clamp(1, 4);
                let sub_span = 1.0 / f32::from(r);
                let len = pitches.len();
                for k in 0..r {
                    let tick = time * i64::from(r) + i64::from(k);
                    let idx = match mode {
                        ArpMode::Up => tick.rem_euclid(len as i64) as usize,
                        ArpMode::Down => (len - 1) - (tick.rem_euclid(len as i64) as usize),
                        ArpMode::UpDown => {
                            if len <= 1 {
                                0
                            } else {
                                let cycle = (2 * len - 2) as i64;
                                let p = tick.rem_euclid(cycle) as usize;
                                if p < len {
                                    p
                                } else {
                                    2 * len - 2 - p
                                }
                            }
                        }
                        ArpMode::Random => {
                            let h = crate::evaluate::det_hash(0x4152_5021, id, tick, 0);
                            ((h * len as f32) as usize).min(len - 1)
                        }
                    };
                    let (degree, accidental, vel) = pitches[idx];
                    out.push(EvaluatedEvent {
                        offset: (f32::from(k) * sub_span).clamp(0.0, 0.999),
                        step: Step {
                            active: true,
                            degree,
                            accidental,
                            velocity: vel,
                            gate: (0.72 * sub_span).clamp(0.08, 2.0),
                            ..Step::default()
                        },
                    });
                }
            }
            PatternKind::RangeFold {
                source,
                low_midi,
                high_midi,
            } => {
                let start = out.len();
                self.evaluate_notes_events_depth(*source, time, depth + 1, out)?;
                let fold = |midi: u8| -> u8 {
                    let mut m = i32::from(midi);
                    let lo = i32::from(*low_midi);
                    let hi = i32::from(*high_midi);
                    while m < lo {
                        m += 12;
                    }
                    while m > hi {
                        m -= 12;
                    }
                    m.clamp(lo, hi) as u8
                };
                for ev in &mut out[start..] {
                    let m = fold(self.pitch_midi(ev.step.degree, ev.step.accidental, 0));
                    (ev.step.degree, ev.step.accidental) = self.midi_to_degree(m);
                    for tone in &mut ev.step.chord {
                        let tm = fold(self.pitch_midi(tone.degree, tone.accidental, 0));
                        (tone.degree, tone.accidental) = self.midi_to_degree(tm);
                    }
                }
            }
            PatternKind::Concat { parts } => {
                let (chosen, rem) = self.resolve_concat_step(parts, time)?;
                self.evaluate_notes_events_depth(chosen, rem, depth + 1, out)?;
            }
            PatternKind::Stack { parts } => {
                for &part in parts {
                    self.evaluate_notes_events_depth(part, time, depth + 1, out)?;
                }
            }
            PatternKind::Shape {
                source,
                velocity,
                gate,
            } => {
                let start = out.len();
                self.evaluate_notes_events_depth(*source, time, depth + 1, out)?;
                for ev in &mut out[start..] {
                    let t = time as f64 + f64::from(ev.offset);
                    let v = self.value_inner(*velocity, t)?.clamp(0.0, 1.0);
                    ev.step.velocity = (ev.step.velocity * v).clamp(0.01, 1.0);
                    for tone in &mut ev.step.chord {
                        tone.velocity = (tone.velocity * v).clamp(0.01, 1.0);
                    }
                    if let Some(g_id) = gate {
                        let g = self.value_inner(*g_id, t)?.clamp(0.05, 1.0);
                        ev.step.gate = (ev.step.gate * g * 2.0).clamp(0.05, 64.0);
                        for tone in &mut ev.step.chord {
                            tone.gate = (tone.gate * g * 2.0).clamp(0.05, 64.0);
                        }
                    }
                }
            }
            PatternKind::Rhythm { .. }
            | PatternKind::Euclidean { .. }
            | PatternKind::Values { .. }
            | PatternKind::SeededValues { .. } => {
                bail!("Expected Notes material");
            }
        }
        Ok(())
    }
}
