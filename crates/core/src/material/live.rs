//! Live timeline compilation: a sliding ring of steps compiled from the same
//! retained patterns, parts and lanes as an arrangement.
//!
//! Each live step evaluates either the lane's active override or, when a timeline
//! segment is present, the arrangement underneath. An optional [`Audition`]
//! either plays alone (when transport is parked) or layers cleanly into the live
//! mix on the launch boundary.
use super::{
    EventOrigin, Id, InstrumentInstance, LanePart, LiveFlags, Part, PhaseMode, Project,
    STEPS_PER_BAR,
};
use crate::mixer::{Channel, ChannelKind};
use crate::rack::RackPatch;
use crate::score::{LiveWindow, ScoreWindow};
use crate::timeline::Segment;
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

pub const LIVE_RING_BARS: usize = 64;
pub const LIVE_RING_STEPS: u64 = (LIVE_RING_BARS * STEPS_PER_BAR) as u64;
/// Synthetic instrument id used when auditioning in a project with no instruments.
pub const AUDITION_INSTRUMENT_ID: Id = u64::MAX - 1;

/// A lane playing a part by value since `anchor` (the transport step it landed on).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Override {
    pub lane: Id,
    pub part: LanePart,
    pub anchor: u64,
}

/// An in-context pattern audition on the live ring.
/// Never authored, never recorded in takes, never kept by Keep.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Audition {
    pub pattern: Id,
    pub instrument: Id,
    pub anchor: u64,
    pub length: usize,
    /// When true (transport was stopped), only the audition plays; when false
    /// (transport is playing), it joins the live mix on `instrument` while
    /// leaving every other instrument untouched.
    pub solo: bool,
}

/// What is heard from `from` until the next segment.
#[derive(Clone, Copy, Debug)]
pub struct LiveSegment<'a> {
    pub from: u64,
    pub song: &'a Project,
    pub flags: &'a LiveFlags,
    pub active: &'a [Override],
    /// Arrangement timeline segment playing under lanes with no override, if any.
    pub timeline: Option<Segment>,
    /// Transient pattern audition active during this segment, if any.
    pub audition: Option<Audition>,
}

impl Override {
    /// Source time for a transport step, or `None` before the override started.
    ///
    /// When a timeline segment is active, `PhaseMode::Song` follows the
    /// timeline's song step (repeating with any loop pass and continuing
    /// monotonically past a continuing end) so kept loop passes match what was
    /// heard.
    pub fn pattern_time(
        &self,
        step: u64,
        timeline: Option<Segment>,
        song_length: u64,
    ) -> Option<i64> {
        if step < self.anchor {
            return None;
        }
        let base = match self.part.phase {
            PhaseMode::Song => {
                let song = match timeline {
                    Some(seg) => seg
                        .song_step(step, song_length)
                        .or_else(|| seg.song_step_unbounded(step))?,
                    None => step,
                };
                i64::try_from(song).ok()?
            }
            PhaseMode::Restart => i64::try_from(step.checked_sub(self.anchor)?).ok()?,
        };
        Some(base + i64::from(self.part.offset_steps))
    }
}

fn segment_at<'s, 'a>(segments: &'s [LiveSegment<'a>], step: u64) -> &'s LiveSegment<'a> {
    let index = segments.partition_point(|s| s.from <= step);
    &segments[index.saturating_sub(1)]
}

fn ring_step_at(base: u64, ring: u64, at: usize) -> u64 {
    base + (at as u64 + ring - base % ring) % ring
}

fn window_stop_at(segments: &[LiveSegment<'_>], base: u64, end: u64) -> Option<u64> {
    for (index, segment) in segments.iter().enumerate() {
        let seg_start = segment.from.max(base);
        let next_from = segments.get(index + 1).map(|s| s.from);
        let seg_end = next_from.map_or(end, |next| next.min(end));
        if seg_end <= seg_start {
            continue;
        }
        let Some(tl) = segment.timeline else {
            continue;
        };
        let Some(stop) = tl.stop_at(segment.song.arrangement_steps() as u64) else {
            continue;
        };
        let effective_stop = stop.max(seg_start);
        let superseded = next_from.is_some_and(|next| next <= effective_stop);
        if !superseded && effective_stop <= end {
            return Some(effective_stop);
        }
    }
    None
}

struct ArrangementStepCtx<'a> {
    out: &'a mut super::ScoreBuilder,
    segment: Segment,
    step: u64,
    at: usize,
    skip_instrument: Option<Id>,
    overridden_instruments: &'a [Id],
    eval_buf: &'a mut Vec<crate::evaluate::EvaluatedEvent>,
}

impl Project {
    /// Compiles the live window `[base, base + LIVE_RING_STEPS)`. `self`
    /// supplies the instruments, patches and mixer that are heard now.
    pub fn compile_live(
        &self,
        flags: &LiveFlags,
        base: u64,
        segments: &[LiveSegment<'_>],
    ) -> Result<ScoreWindow> {
        self.compile_live_window_with_audition(flags, base, LIVE_RING_BARS, segments, None)
    }

    /// Compiles the live window with an optional fallback Rack patch for
    /// auditioning in projects without instruments.
    pub fn compile_live_with_audition(
        &self,
        flags: &LiveFlags,
        base: u64,
        segments: &[LiveSegment<'_>],
        audition_patch: Option<RackPatch>,
    ) -> Result<ScoreWindow> {
        self.compile_live_window_with_audition(
            flags,
            base,
            LIVE_RING_BARS,
            segments,
            audition_patch,
        )
    }

    /// The arrangement under a lane with no override, at one transport step.
    fn write_arrangement_step(
        &self,
        lane: &super::Lane,
        ctx: &mut ArrangementStepCtx<'_>,
    ) -> Result<()> {
        let Some(song_step) = ctx
            .segment
            .song_step(ctx.step, self.arrangement_steps() as u64)
        else {
            return Ok(());
        };
        let Some(place) = lane.placements.iter().find(|p| {
            (u64::from(p.start_step)..u64::from(p.start_step + p.length_steps)).contains(&song_step)
        }) else {
            return Ok(());
        };
        let Some(part) = self.part(place.part) else {
            return Ok(());
        };
        if ctx.skip_instrument == Some(part.instrument)
            || ctx.overridden_instruments.contains(&part.instrument)
            || ctx.out.instrument_index(part.instrument).is_none()
        {
            return Ok(());
        }
        let local = (song_step - u64::from(place.start_step)) as u32;
        let pattern_time = place.pattern_time(song_step as u32);
        if let Some(ex) = place.exceptions.iter().find(|o| o.step == local) {
            if !ex.value.active {
                return Ok(());
            }
            let mut val = ex.value.clone();
            self.interpret(part, &mut val);
            ctx.out.write_event(
                self,
                super::compile::EventWrite {
                    at: ctx.at,
                    offset: 0.0,
                    part,
                    step: val,
                    origin: EventOrigin {
                        song_step: u32::try_from(ctx.step).unwrap_or(u32::MAX),
                        lane: lane.id,
                        placement: place.id,
                        part: part.id,
                        pattern: part.pattern,
                        instrument: part.instrument,
                        pattern_time,
                        local_exception: true,
                    },
                },
            )?;
            return Ok(());
        }
        ctx.eval_buf.clear();
        crate::evaluate::evaluate_notes_into(self, part.pattern, pattern_time, ctx.eval_buf)?;
        for mut ev in ctx.eval_buf.drain(..) {
            if !ev.step.active {
                continue;
            }
            self.interpret(part, &mut ev.step);
            ctx.out.write_event(
                self,
                super::compile::EventWrite {
                    at: ctx.at,
                    offset: ev.offset,
                    part,
                    step: ev.step,
                    origin: EventOrigin {
                        song_step: u32::try_from(ctx.step).unwrap_or(u32::MAX),
                        lane: lane.id,
                        placement: place.id,
                        part: part.id,
                        pattern: part.pattern,
                        instrument: part.instrument,
                        pattern_time,
                        local_exception: false,
                    },
                },
            )?;
        }
        Ok(())
    }

    /// Compiles `[base, base + bars * 16)` into a ring of that length.
    pub fn compile_live_window(
        &self,
        flags: &LiveFlags,
        base: u64,
        bars: usize,
        segments: &[LiveSegment<'_>],
    ) -> Result<ScoreWindow> {
        self.compile_live_window_with_audition(flags, base, bars, segments, None)
    }

    /// Compiles `[base, base + bars * 16)` into a ring of that length, synthesizing
    /// a temporary audition instrument if an [`Audition`] targets an absent id.
    pub fn compile_live_window_with_audition(
        &self,
        flags: &LiveFlags,
        base: u64,
        bars: usize,
        segments: &[LiveSegment<'_>],
        audition_patch: Option<RackPatch>,
    ) -> Result<ScoreWindow> {
        ensure!(
            (1..=super::MAX_BARS).contains(&bars),
            "Live window must be 1..{} bars",
            super::MAX_BARS
        );
        let ring = (bars * STEPS_PER_BAR) as u64;
        ensure!(
            segments.first().is_some_and(|s| s.from <= base),
            "Live compile needs a segment covering its first step"
        );
        ensure!(
            segments.windows(2).all(|w| w[0].from <= w[1].from),
            "Live segments must be ordered"
        );
        let solo_audition_any = segments
            .iter()
            .any(|s| s.audition.is_some_and(|aud| aud.solo));
        let missing_audition_instrument = segments
            .iter()
            .filter_map(|s| s.audition.map(|aud| aud.instrument))
            .find(|&id| self.instrument(id).is_none());
        let statics: Cow<'_, Project> = if let Some(aud_id) = missing_audition_instrument {
            let mut cloned = self.clone();
            let ch_id = cloned
                .mixer
                .channels
                .iter()
                .map(|c| c.id)
                .max()
                .unwrap_or(0)
                + 1;
            cloned
                .mixer
                .channels
                .push(Channel::new(ch_id, ChannelKind::Audio, "Audition"));
            cloned.instruments.push(InstrumentInstance {
                id: aud_id,
                name: "Audition Init".into(),
                rack: audition_patch.unwrap_or_else(RackPatch::default_synth),
                channel: ch_id,
            });
            Cow::Owned(cloned)
        } else {
            Cow::Borrowed(self)
        };
        let empty_flags = LiveFlags::default();
        let builder_flags = if solo_audition_any {
            &empty_flags
        } else {
            flags
        };
        let mut out = statics.score_builder(bars, builder_flags)?;
        let end = base + ring;
        let stop_at = if solo_audition_any {
            None
        } else {
            window_stop_at(segments, base, end)
        };
        out.live_window = Some(LiveWindow {
            start: base,
            end,
            stop_at,
        });
        let mut eval_buf = Vec::with_capacity(4);
        let mut overridden_instruments = Vec::with_capacity(super::MAX_LANES);
        for step in base..end {
            let segment = segment_at(segments, step);
            let song = segment.song;
            let seg_flags = segment.flags;
            let at = (step % ring) as usize;
            let active_audition = segment.audition.filter(|aud| step >= aud.anchor);
            let solo_audition = segment.audition.is_some_and(|aud| aud.solo);
            let skip_instrument = active_audition.map(|aud| aud.instrument);

            if !solo_audition {
                // Pass 1: active live Overrides always claim priority over underlying
                // arrangement placements sharing the same instrument (including across rests).
                overridden_instruments.clear();
                for lane in &song.lanes {
                    if !seg_flags.lane_enabled(lane.id) {
                        continue;
                    }
                    let Some(active) = segment.active.iter().find(|a| a.lane == lane.id) else {
                        continue;
                    };
                    let Some(part) = song.part(active.part.part) else {
                        continue;
                    };
                    if skip_instrument == Some(part.instrument)
                        || statics.instrument(part.instrument).is_none()
                    {
                        continue;
                    }
                    let Some(pattern_time) = active.pattern_time(
                        step,
                        segment.timeline,
                        song.arrangement_steps() as u64,
                    ) else {
                        continue;
                    };
                    if !overridden_instruments.contains(&part.instrument) {
                        overridden_instruments.push(part.instrument);
                    }
                    eval_buf.clear();
                    crate::evaluate::evaluate_notes_into(
                        song,
                        part.pattern,
                        pattern_time,
                        &mut eval_buf,
                    )?;
                    for mut ev in eval_buf.drain(..) {
                        if !ev.step.active {
                            continue;
                        }
                        song.interpret(part, &mut ev.step);
                        out.write_event(
                            song,
                            super::compile::EventWrite {
                                at,
                                offset: ev.offset,
                                part,
                                step: ev.step,
                                origin: EventOrigin {
                                    song_step: u32::try_from(step).unwrap_or(u32::MAX),
                                    lane: lane.id,
                                    placement: 0,
                                    part: part.id,
                                    pattern: part.pattern,
                                    instrument: part.instrument,
                                    pattern_time,
                                    local_exception: false,
                                },
                            },
                        )?;
                    }
                }
                // Pass 2: underlying arrangement placements on lanes without an active Override.
                if let Some(tl) = segment.timeline {
                    let mut ctx = ArrangementStepCtx {
                        out: &mut out,
                        segment: tl,
                        step,
                        at,
                        skip_instrument,
                        overridden_instruments: &overridden_instruments,
                        eval_buf: &mut eval_buf,
                    };
                    for lane in &song.lanes {
                        if !seg_flags.lane_enabled(lane.id)
                            || segment.active.iter().any(|a| a.lane == lane.id)
                        {
                            continue;
                        }
                        song.write_arrangement_step(lane, &mut ctx)?;
                    }
                }
            }

            if let Some(aud) = active_audition {
                let source = if song.pattern(aud.pattern).is_some() {
                    song
                } else {
                    statics.as_ref()
                };
                if source.pattern(aud.pattern).is_some()
                    && statics.instrument(aud.instrument).is_some()
                {
                    let len = aud.length.max(1) as u64;
                    let pattern_time = ((step - aud.anchor) % len) as i64;
                    eval_buf.clear();
                    if crate::evaluate::evaluate_notes_into(
                        source,
                        aud.pattern,
                        pattern_time,
                        &mut eval_buf,
                    )
                    .is_ok()
                    {
                        let part = Part::new(0, "Audition", aud.pattern, aud.instrument);
                        for ev in eval_buf.drain(..) {
                            if ev.step.active {
                                out.write_event(
                                    source,
                                    super::compile::EventWrite {
                                        at,
                                        offset: ev.offset,
                                        part: &part,
                                        step: ev.step,
                                        origin: EventOrigin {
                                            song_step: u32::try_from(step).unwrap_or(u32::MAX),
                                            lane: 0,
                                            placement: 0,
                                            part: 0,
                                            pattern: aud.pattern,
                                            instrument: aud.instrument,
                                            pattern_time,
                                            local_exception: false,
                                        },
                                    },
                                )?;
                            }
                        }
                    }
                }
            }
        }
        if !solo_audition_any {
            out.write_modulations(&statics, |pattern, at| {
                let step = ring_step_at(base, ring, at);
                let segment = segment_at(segments, step);
                let song = segment.song;
                let source = if song.pattern(pattern).is_some() {
                    song
                } else {
                    statics.as_ref()
                };
                let song_len = source.arrangement_steps() as u64;
                let mod_step = match segment.timeline {
                    Some(tl) => tl
                        .song_step(step, song_len)
                        .unwrap_or_else(|| song_len.saturating_sub(1)),
                    None => step,
                };
                source.value_inner(pattern, mod_step as f64)
            })?;
        }
        out.finish()
    }

    /// Whether `next` differs from `self` only in what the timeline encodes:
    /// patterns, parts, lanes, sections and live timing. Instruments, mixer,
    /// tempo and key are heard immediately.
    pub fn timeline_compatible(&self, next: &Project) -> bool {
        self.instruments == next.instruments
            && self.mixer == next.mixer
            && self.ducks == next.ducks
            && self.macros == next.macros
            && self.bpm == next.bpm
            && self.root_midi == next.root_midi
            && self.scale == next.scale
            && self.modulations.len() == next.modulations.len()
            && self
                .modulations
                .iter()
                .zip(&next.modulations)
                .all(|(a, b)| a.id == b.id && a.target == b.target)
    }

    /// Whether a lane override's lane and part still exist in this song.
    pub fn override_valid(&self, state: &Override) -> bool {
        self.lane(state.lane).is_some() && self.part(state.part.part).is_some()
    }

    /// The instrument a part plays, if it resolves.
    pub fn part_instrument(&self, part: Id) -> Option<Id> {
        Some(self.part(part)?.instrument)
    }
}
