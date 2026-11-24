//! Read-only views of the currently prepared finite song performance.
//!
//! The caller supplies a Project and its matching CompiledProject from one
//! accepted revision. Queries never compile, flatten, or edit musical material.
//! Notes are scheduled events, not measurements of audible voices or FX tails.
use crate::material::{
    EventOrigin, Id, PatternKind, PatternValue, Project, MAX_BARS, MAX_INSTRUMENTS, MAX_LANES,
    MAX_LINKS, MAX_PARTS, MAX_PATTERNS, MAX_PLACEMENTS,
};
use crate::mixer::{ChannelRef, LiveFlags};
use crate::music::MAX_CHORD_TONES;
use crate::score::{ScoreEvent, ScoreWindow};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Up to 128 visible bars (2048 sixteenth-steps) so wide zoom levels never truncate.
pub const MAX_VISIBLE_STEPS: usize = 2048;
/// Current authored note/chord gates are bounded to sixteen sixteenth steps.
pub const MAX_GATE_STEPS: u32 = 64;
/// Generous visible-note budget with ample headroom for wide zoom, chords, and sub-step ratchets/arps.
pub const MAX_VISIBLE_NOTES: usize =
    MAX_LANES * (MAX_VISIBLE_STEPS + MAX_GATE_STEPS as usize) * (MAX_CHORD_TONES + 1) * 3;

/// Identity of one evaluated chord voice in one placement occurrence.
/// A chord edit can change the pitch occupying a tone slot; this is not a saved ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventKey {
    pub placement: Id,
    pub song_step: u32,
    /// Zero-based sub-step occurrence index within `(placement, song_step)` sorted by offset.
    pub sub_step: u16,
    /// Zero is the main note; subsequent indices address the authored chord order.
    pub tone: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArrangementRequest {
    pub start_step: u32,
    pub step_count: usize,
    pub lane: Option<Id>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ArrangementNote {
    pub key: EventKey,
    /// Fractional sub-step onset inside `key.song_step` (`0.0 <= offset < 1.0`).
    pub offset: f32,
    pub lane: Id,
    pub part: Id,
    pub pattern: Id,
    pub instrument: Id,
    pub channel: Id,
    pub local_step: u32,
    pub pattern_time: i64,
    /// Wrapped coordinate of the root material's editor (literal or derived).
    pub source_step: usize,
    pub midi: u8,
    pub velocity: f32,
    /// Full musical gate, never clipped to a page or placement boundary.
    pub gate: f32,
    pub local_exception: bool,
    /// Direct Audio-channel mute only. It does not describe gain, sends or audibility.
    pub channel_muted: bool,
}
impl ArrangementNote {
    /// Exact fractional song-step onset (`key.song_step + offset`).
    pub fn start_step(&self) -> f64 {
        f64::from(self.key.song_step) + f64::from(self.offset)
    }

    pub fn end_step(&self) -> f64 {
        self.start_step() + f64::from(self.gate)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ArrangementLane {
    pub id: Id,
    pub muted: bool,
    pub solo: bool,
    /// Lane mute/solo eligibility; channel mix state does not affect this flag.
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ArrangementProjection {
    pub start_step: u32,
    /// Requested span clipped at the finite song's end.
    pub step_count: usize,
    pub lanes: Vec<ArrangementLane>,
    pub notes: Vec<ArrangementNote>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SourcePoint {
    pub pattern: Id,
    pub parent: Option<Id>,
    pub role: String,
    pub depth: usize,
    pub time: i64,
    pub step: usize,
    pub period: usize,
    pub value: PatternValue,
    /// A placement-local note replaces this note-source graph at the onset.
    /// Its underlying values remain inspectable but did not produce that note.
    pub displaced: bool,
    /// The shared definition was already expanded; retain this edge without
    /// recursively duplicating a shared DAG into an exponential tree.
    pub reference_only: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ArrangementInspection {
    pub note: ArrangementNote,
    pub sources: Vec<SourcePoint>,
}

/// A bounded song-time query. Incoming gates keep their actual earlier onset;
/// no prior-loop note is invented at song zero. Conditions and local overrides
/// are already resolved by the same compiler that prepares the audio score.
pub fn arrangement_notes(
    project: &Project,
    compiled: &ScoreWindow,
    flags: &LiveFlags,
    request: ArrangementRequest,
) -> Result<ArrangementProjection> {
    check_snapshot_shape(project, compiled)?;
    ensure!(
        (1..=MAX_VISIBLE_STEPS).contains(&request.step_count),
        "Arrangement view must request 1..={MAX_VISIBLE_STEPS} steps"
    );
    let horizon = project.arrangement_steps() as u32;
    ensure!(request.start_step < horizon, "View starts outside the song");
    if let Some(lane) = request.lane {
        project.lane(lane).context("View lane is missing")?;
    }
    let end = request
        .start_step
        .saturating_add(request.step_count as u32)
        .min(horizon);
    let lanes = project
        .lanes
        .iter()
        .filter(|lane| request.lane.is_none_or(|id| lane.id == id))
        .map(|lane| ArrangementLane {
            id: lane.id,
            muted: flags.lane_muted(lane.id),
            solo: flags.lane_solo(lane.id),
            enabled: flags.lane_enabled(lane.id),
        })
        .collect();
    let first = compiled.provenance.partition_point(|origin| {
        origin.song_step < request.start_step.saturating_sub(MAX_GATE_STEPS)
    });
    let last = compiled
        .provenance
        .partition_point(|origin| origin.song_step < end);
    let mut resolver = Resolver::new(project, compiled, flags);
    let mut notes = Vec::new();
    for origin in &compiled.provenance[first..last] {
        if request.lane.is_some_and(|id| origin.lane != id) {
            continue;
        }
        let (base, resolved) = resolver.onset(origin)?;
        for tone in 0..=resolved.chord_len {
            let note = chord_note(&base, &resolved, tone).expect("resolved chord index");
            if note.end_step() > f64::from(request.start_step) {
                ensure!(
                    notes.len() < MAX_VISIBLE_NOTES,
                    "Arrangement view exceeds note budget"
                );
                notes.push(note);
            }
        }
    }
    notes.sort_by(|a, b| {
        a.key
            .song_step
            .cmp(&b.key.song_step)
            .then_with(|| a.offset.total_cmp(&b.offset))
            .then_with(|| a.lane.cmp(&b.lane))
            .then_with(|| a.key.sub_step.cmp(&b.key.sub_step))
            .then_with(|| a.key.tone.cmp(&b.key.tone))
    });
    Ok(ArrangementProjection {
        start_step: request.start_step,
        step_count: (end - request.start_step) as usize,
        lanes,
        notes,
    })
}

/// Find an exact scheduled voice, rejecting stale placement/step/sub-step/tone selections.
pub fn event_by_key(
    project: &Project,
    compiled: &ScoreWindow,
    flags: &LiveFlags,
    key: EventKey,
) -> Result<Option<ArrangementNote>> {
    check_snapshot_shape(project, compiled)?;
    let start = compiled
        .provenance
        .partition_point(|origin| origin.song_step < key.song_step);
    let occurrence = usize::from(key.sub_step);
    let origin = compiled.provenance[start..]
        .iter()
        .take_while(|origin| origin.song_step == key.song_step)
        .filter(|origin| origin.placement == key.placement)
        .nth(occurrence);
    let Some(origin) = origin else {
        return Ok(None);
    };
    let (base, resolved) = Resolver::new(project, compiled, flags).onset_at(origin, occurrence)?;
    Ok(chord_note(&base, &resolved, key.tone))
}

/// Select a gate covering the cursor, preferring the most recent onset. A pitch
/// filters exactly; omitted pitch chooses the earliest sub-step and first authored chord voice.
pub fn event_at(
    project: &Project,
    compiled: &ScoreWindow,
    flags: &LiveFlags,
    lane: Id,
    song_step: u32,
    pitch: Option<u8>,
) -> Result<Option<ArrangementNote>> {
    let projection = arrangement_notes(
        project,
        compiled,
        flags,
        ArrangementRequest {
            start_step: song_step,
            step_count: 1,
            lane: Some(lane),
        },
    )?;
    Ok(projection
        .notes
        .into_iter()
        .filter(|note| pitch.is_none_or(|midi| note.midi == midi))
        .max_by_key(|note| {
            (
                note.key.song_step,
                std::cmp::Reverse(note.key.sub_step),
                std::cmp::Reverse(note.key.tone),
            )
        }))
}

/// Cursor position for stepping chronologically across arrangement events on a lane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventCursor {
    pub lane: Id,
    pub song_step: u32,
    pub sub_step: Option<u16>,
    pub tone: Option<u8>,
}

/// Search sorted prepared onsets without materializing the entire song view,
/// stepping cleanly across every `(song_step, sub_step, tone)` in chronological
/// or reverse order. `tone: None` includes notes at the cursor (first/last voice
/// by direction); `Some(tone)` moves strictly beyond the selected chord voice at
/// that onset.
pub fn adjacent_event(
    project: &Project,
    compiled: &ScoreWindow,
    flags: &LiveFlags,
    cursor: EventCursor,
    forward: bool,
) -> Result<Option<ArrangementNote>> {
    let EventCursor {
        lane,
        song_step,
        sub_step,
        tone,
    } = cursor;
    check_snapshot_shape(project, compiled)?;
    project.lane(lane).context("View lane is missing")?;
    ensure!(
        song_step < project.arrangement_steps() as u32,
        "Event cursor is outside song"
    );
    let mut resolver = Resolver::new(project, compiled, flags);
    if forward {
        let first = compiled
            .provenance
            .partition_point(|origin| origin.song_step < song_step);
        let mut cur_step: Option<u32> = None;
        let mut occ: usize = 0;
        for origin in &compiled.provenance[first..] {
            if origin.lane != lane {
                continue;
            }
            if cur_step != Some(origin.song_step) {
                cur_step = Some(origin.song_step);
                occ = 0;
            }
            let this_occ = occ;
            occ += 1;
            if origin.song_step == song_step {
                match (sub_step, tone) {
                    (Some(cur_sub), Some(cur_tone)) => {
                        let target = usize::from(cur_sub);
                        if this_occ < target {
                            continue;
                        }
                        let (base, resolved) = resolver.onset_at(origin, this_occ)?;
                        let first_tone = if this_occ == target {
                            usize::from(cur_tone) + 1
                        } else {
                            0
                        };
                        if first_tone <= usize::from(resolved.chord_len) {
                            return Ok(chord_note(&base, &resolved, first_tone as u8));
                        }
                    }
                    (Some(cur_sub), None) => {
                        if this_occ < usize::from(cur_sub) {
                            continue;
                        }
                        let (base, resolved) = resolver.onset_at(origin, this_occ)?;
                        return Ok(chord_note(&base, &resolved, 0));
                    }
                    (None, Some(cur_tone)) => {
                        if this_occ > 0 {
                            continue;
                        }
                        let (base, resolved) = resolver.onset_at(origin, 0)?;
                        let first_tone = usize::from(cur_tone) + 1;
                        if first_tone <= usize::from(resolved.chord_len) {
                            return Ok(chord_note(&base, &resolved, first_tone as u8));
                        }
                    }
                    (None, None) => {
                        let (base, resolved) = resolver.onset_at(origin, this_occ)?;
                        return Ok(chord_note(&base, &resolved, 0));
                    }
                }
            } else {
                let (base, resolved) = resolver.onset_at(origin, this_occ)?;
                return Ok(chord_note(&base, &resolved, 0));
            }
        }
    } else {
        let end = compiled
            .provenance
            .partition_point(|origin| origin.song_step <= song_step);
        let slice = &compiled.provenance[..end];
        let mut i = slice.len();
        let mut step_origins: Vec<&EventOrigin> = Vec::with_capacity(8);
        while i > 0 {
            let group_step = slice[i - 1].song_step;
            let mut group_start = i - 1;
            while group_start > 0 && slice[group_start - 1].song_step == group_step {
                group_start -= 1;
            }
            step_origins.clear();
            for origin in &slice[group_start..i] {
                if origin.lane == lane {
                    step_origins.push(origin);
                }
            }
            for (this_occ, &origin) in step_origins.iter().enumerate().rev() {
                if group_step == song_step {
                    match (sub_step, tone) {
                        (Some(cur_sub), Some(cur_tone)) => {
                            let target = usize::from(cur_sub);
                            if this_occ > target {
                                continue;
                            }
                            if this_occ == target {
                                if cur_tone == 0 {
                                    continue;
                                }
                                let (base, resolved) = resolver.onset_at(origin, this_occ)?;
                                let last_tone = (cur_tone - 1).min(resolved.chord_len);
                                return Ok(chord_note(&base, &resolved, last_tone));
                            }
                            let (base, resolved) = resolver.onset_at(origin, this_occ)?;
                            return Ok(chord_note(&base, &resolved, resolved.chord_len));
                        }
                        (Some(cur_sub), None) => {
                            if this_occ > usize::from(cur_sub) {
                                continue;
                            }
                            let (base, resolved) = resolver.onset_at(origin, this_occ)?;
                            return Ok(chord_note(&base, &resolved, resolved.chord_len));
                        }
                        (None, Some(cur_tone)) => {
                            if cur_tone == 0 {
                                break;
                            }
                            if this_occ > 0 {
                                continue;
                            }
                            let (base, resolved) = resolver.onset_at(origin, 0)?;
                            let last_tone = (cur_tone - 1).min(resolved.chord_len);
                            return Ok(chord_note(&base, &resolved, last_tone));
                        }
                        (None, None) => {
                            let (base, resolved) = resolver.onset_at(origin, this_occ)?;
                            return Ok(chord_note(&base, &resolved, resolved.chord_len));
                        }
                    }
                } else {
                    let (base, resolved) = resolver.onset_at(origin, this_occ)?;
                    return Ok(chord_note(&base, &resolved, resolved.chord_len));
                }
            }
            i = group_start;
        }
    }
    Ok(None)
}

/// Inspect the exact retained definitions and sampled source clocks behind a note.
/// Placement exceptions explicitly mark their displaced source graph; per-note links
/// still sample their own values in that same placement clock.
pub fn inspect_event(
    project: &Project,
    compiled: &ScoreWindow,
    flags: &LiveFlags,
    key: EventKey,
) -> Result<ArrangementInspection> {
    let note =
        event_by_key(project, compiled, flags, key)?.context("Scheduled event no longer exists")?;
    let part = project.part(note.part).context("Part missing")?;
    ensure!(
        part.parameter_links.len() <= MAX_LINKS,
        "Too many per-note source links"
    );
    let mut sources = Vec::new();
    SourceTracer::new(project, &mut sources).trace(TraceNode {
        pattern: note.pattern,
        parent: None,
        role: "Material",
        depth: 0,
        time: note.pattern_time,
        displaced: note.local_exception,
    })?;
    for link in &part.parameter_links {
        // Different uses remain explicit even if a value definition also helps
        // construct the note. The current parameter sources have no dependencies.
        let role = format!("Per-note {}", link.target.short_label());
        SourceTracer::new(project, &mut sources).trace(TraceNode {
            pattern: link.pattern,
            parent: None,
            role: &role,
            depth: 0,
            time: note.pattern_time,
            displaced: false,
        })?;
    }
    Ok(ArrangementInspection { note, sources })
}

fn check_snapshot_shape(project: &Project, compiled: &ScoreWindow) -> Result<()> {
    ensure!(
        (1..=MAX_BARS).contains(&project.bars),
        "Invalid song horizon"
    );
    ensure!(
        project.patterns.len() <= MAX_PATTERNS
            && project.parts.len() <= MAX_PARTS
            && project.instruments.len() <= MAX_INSTRUMENTS
            && project.lanes.len() <= MAX_LANES
            && project
                .lanes
                .iter()
                .map(|lane| lane.placements.len())
                .sum::<usize>()
                <= MAX_PLACEMENTS,
        "Arrangement projection exceeds authoring bounds"
    );
    ensure!(
        compiled.arrangement_steps() == project.arrangement_steps()
            && compiled.instruments.len() == project.instruments.len(),
        "Prepared arrangement does not match the authored snapshot"
    );
    Ok(())
}

struct Resolver<'a> {
    project: &'a Project,
    compiled: &'a ScoreWindow,
    flags: &'a LiveFlags,
    periods: HashMap<Id, usize>,
    /// Tracks how many sub-step `ScoreEvent`s have already been claimed for each
    /// `(song_step, inst_idx, part_id)` so `:mini`, `Ratchet`, `Arp`, and `Swing`
    /// sub-step hits each resolve to their own distinct `ScoreEvent` and `offset`.
    consumed: HashMap<(u32, usize, Id), usize>,
}
impl<'a> Resolver<'a> {
    fn new(project: &'a Project, compiled: &'a ScoreWindow, flags: &'a LiveFlags) -> Self {
        Self {
            project,
            compiled,
            flags,
            periods: HashMap::new(),
            consumed: HashMap::new(),
        }
    }
    fn onset(&mut self, origin: &EventOrigin) -> Result<(ArrangementNote, ScoreEvent)> {
        let index = self
            .compiled
            .instrument_index(origin.instrument)
            .context("Prepared instrument mapping missing")?;
        let slot_key = (origin.song_step, index, origin.part);
        let occurrence = *self.consumed.get(&slot_key).unwrap_or(&0);
        let res = self.onset_at(origin, occurrence)?;
        self.consumed.insert(slot_key, occurrence + 1);
        Ok(res)
    }
    fn onset_at(
        &mut self,
        origin: &EventOrigin,
        occurrence: usize,
    ) -> Result<(ArrangementNote, ScoreEvent)> {
        let lane = self
            .project
            .lane(origin.lane)
            .context("Prepared lane missing")?;
        let placement = lane
            .placements
            .iter()
            .find(|p| p.id == origin.placement)
            .context("Prepared placement missing")?;
        let part = self
            .project
            .part(origin.part)
            .context("Prepared part missing")?;
        let instrument = self
            .project
            .instrument(origin.instrument)
            .context("Prepared instrument missing")?;
        let channel = self
            .project
            .channel(instrument.channel)
            .context("Prepared channel missing")?;
        let local_step = origin
            .song_step
            .checked_sub(placement.start_step)
            .context("Prepared event precedes placement")?;
        ensure!(
            local_step < placement.length_steps
                && placement.part == part.id
                && part.pattern == origin.pattern
                && part.instrument == instrument.id
                && origin.pattern_time == placement.pattern_time(origin.song_step)
                && origin.local_exception
                    == placement.exceptions.iter().any(|o| o.step == local_step),
            "Prepared event does not match its authored placement"
        );
        let index = self
            .compiled
            .instrument_index(instrument.id)
            .context("Prepared instrument mapping missing")?;
        let resolved = self
            .compiled
            .step_events(u64::from(origin.song_step))
            .iter()
            .filter(|ev| usize::from(ev.inst_idx) == index && ev.part_id == Some(part.id))
            .nth(occurrence)
            .cloned()
            .context("Prepared onset does not match event provenance")?;
        let period = if let Some(&period) = self.periods.get(&part.pattern) {
            period
        } else {
            let period = self.project.editor_period(part.pattern)?;
            self.periods.insert(part.pattern, period);
            period
        };
        Ok((
            ArrangementNote {
                key: EventKey {
                    placement: placement.id,
                    song_step: origin.song_step,
                    sub_step: occurrence as u16,
                    tone: 0,
                },
                offset: resolved.offset.clamp(0.0, 0.9999),
                lane: lane.id,
                part: part.id,
                pattern: part.pattern,
                instrument: instrument.id,
                channel: instrument.channel,
                local_step,
                pattern_time: origin.pattern_time,
                source_step: wrapped_step(origin.pattern_time, period),
                midi: resolved.midi,
                velocity: resolved.velocity,
                gate: resolved.gate,
                local_exception: origin.local_exception,
                channel_muted: self.flags.channel_muted(ChannelRef::Channel(channel.id)),
            },
            resolved,
        ))
    }
}
fn chord_note(base: &ArrangementNote, resolved: &ScoreEvent, tone: u8) -> Option<ArrangementNote> {
    if tone == 0 {
        return Some(base.clone());
    }
    let voice = resolved.chord_tones().get(usize::from(tone - 1))?;
    let mut note = base.clone();
    note.key.tone = tone;
    note.midi = voice.midi;
    note.velocity = voice.velocity;
    note.gate = voice.gate;
    Some(note)
}
fn wrapped_step(time: i64, period: usize) -> usize {
    i128::from(time).rem_euclid(period as i128) as usize
}

struct TraceNode<'a> {
    pattern: Id,
    parent: Option<Id>,
    role: &'a str,
    depth: usize,
    time: i64,
    displaced: bool,
}

struct SourceTracer<'a> {
    project: &'a Project,
    visited: HashSet<Id>,
    sources: &'a mut Vec<SourcePoint>,
}

impl<'a> SourceTracer<'a> {
    fn new(project: &'a Project, sources: &'a mut Vec<SourcePoint>) -> Self {
        Self {
            project,
            visited: HashSet::new(),
            sources,
        }
    }

    fn trace(&mut self, node: TraceNode<'_>) -> Result<()> {
        let TraceNode {
            pattern,
            parent,
            role,
            depth,
            time,
            displaced,
        } = node;
        ensure!(
            depth < crate::material::MAX_DEPTH,
            "Source trace exceeds dependency depth"
        );
        ensure!(
            self.sources.len() < (MAX_PATTERNS * 2 + 1) * (MAX_LINKS + 1),
            "Source trace exceeds node budget"
        );
        let definition = self
            .project
            .pattern(pattern)
            .context("Source pattern missing")?;
        let period = self.project.editor_period(pattern)?;
        let reference_only = !self.visited.insert(pattern);
        self.sources.push(SourcePoint {
            pattern,
            parent,
            role: role.into(),
            depth,
            time,
            step: wrapped_step(time, period),
            period,
            value: self.project.evaluate_pattern(pattern, time)?,
            displaced,
            reference_only,
        });
        if reference_only {
            return Ok(());
        }
        match &definition.kind {
            PatternKind::Shift { source, steps } => {
                self.trace(TraceNode {
                    pattern: *source,
                    parent: Some(pattern),
                    role: "Shifted",
                    depth: depth + 1,
                    time: time - i64::from(*steps),
                    displaced,
                })?;
            }
            PatternKind::Reverse { source } => {
                let src_period = self.project.pattern_period(*source)? as i64;
                let cycle = time.div_euclid(src_period);
                let idx = time.rem_euclid(src_period);
                let rev = cycle * src_period + (src_period - 1 - idx);
                self.trace(TraceNode {
                    pattern: *source,
                    parent: Some(pattern),
                    role: "Reversed",
                    depth: depth + 1,
                    time: rev,
                    displaced,
                })?;
            }
            PatternKind::TimeScale { source, num, den } => {
                let scaled = (i128::from(time) * i128::from((*num).max(1)))
                    .div_euclid(i128::from((*den).max(1))) as i64;
                self.trace(TraceNode {
                    pattern: *source,
                    parent: Some(pattern),
                    role: "Time-scaled",
                    depth: depth + 1,
                    time: scaled,
                    displaced,
                })?;
            }
            PatternKind::Concat { parts } => {
                let mut periods = Vec::with_capacity(parts.len());
                let mut total = 0i64;
                for &part in parts {
                    let p = self.project.pattern_period(part)? as i64;
                    periods.push(p);
                    total += p;
                }
                let rem = time.rem_euclid(total.max(1));
                let mut acc = 0i64;
                for (&child, &p) in parts.iter().zip(&periods) {
                    let child_time = if rem >= acc && rem < acc + p {
                        rem - acc
                    } else {
                        time - acc
                    };
                    acc += p;
                    self.trace(TraceNode {
                        pattern: child,
                        parent: Some(pattern),
                        role: "Concat",
                        depth: depth + 1,
                        time: child_time,
                        displaced,
                    })?;
                }
            }
            other => {
                for (child, role) in other.source_roles() {
                    self.trace(TraceNode {
                        pattern: child,
                        parent: Some(pattern),
                        role,
                        depth: depth + 1,
                        time,
                        displaced,
                    })?;
                }
            }
        }
        Ok(())
    }
}
