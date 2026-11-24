//! Semantic Pattern deck view (Notes, Rhythm, Values, Derived sources, smooth multi-use playheads).
use crate::{kind_label, App, EditScope, Mode};
use cadence_core::material::{
    Boundary, Id, ModulationTarget, PatternKind, PatternType, PatternValue, ValueUnit,
};
use cadence_core::mixer::EffectKind;
use cadence_core::music::{note_name, scale_name};
use cadence_core::params::ValueMode;
use cadence_core::playback::{self, PlaybackClock, PlaybackMarker, PlaybackUse};
use cadence_core::runtime::AudioSnapshot;
use std::collections::HashSet;

#[derive(Clone, Debug)]
pub struct PatternDeckView {
    pub empty: bool,
    pub pattern_id: Option<Id>,
    pub name: String,
    pub kind_label: &'static str,
    pub scope_label: String,
    pub zoom: f32,
    pub period: usize,
    pub page_start: usize,
    pub page_steps: usize,
    pub cursor: usize,
    pub insert_mode: bool,
    pub selected_voice: usize,
    pub voice_readout: Option<String>,
    pub focused: bool,
    pub root_note: String,
    pub scale_name: &'static str,
    pub root_pc: u8,
    pub scale_pcs: [bool; 12],
    pub playback_status: String,
    pub playheads: Vec<PatternPlayhead>,
    pub sources: Vec<PatternSourceChip>,
    pub consumers: Vec<PatternConsumerChip>,
    pub source_previews: Vec<SourcePreviewStrip>,
    pub mini_code: Option<String>,
    pub content: PatternContentView,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatternConsumerChip {
    pub label: String,
    pub detail: String,
}

#[derive(Clone, Debug)]
pub struct SourcePreviewStrip {
    pub id: Id,
    pub name: String,
    pub kind_label: &'static str,
    pub period: usize,
    pub kind: SourcePreviewKind,
}

#[derive(Clone, Debug)]
pub enum SourcePreviewKind {
    /// Active hits across visible steps `(step_offset, normalized_height 0..1)`.
    Notes(Vec<Option<f32>>),
    /// Rhythm hits across visible steps.
    Rhythm(Vec<bool>),
    /// Normalized value curve `0..=1` across visible steps.
    Values(Vec<f32>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PatternPlayhead {
    /// Continuous step position inside the pattern period (smooth at 60/120fps).
    pub position: f64,
    pub kind: PlaybackUse,
    pub local_exception: bool,
}

#[derive(Clone, Debug)]
pub struct PatternSourceChip {
    pub id: Id,
    pub name: String,
    pub kind_label: &'static str,
    pub selected: bool,
}

#[derive(Clone, Debug)]
pub enum PatternContentView {
    Empty,
    Notes {
        steps: Vec<PatternNoteStep>,
        /// Sub-step resolved events across the visible page (for Mini, Arp, Ratchet, Swing, TimeScale, Stack).
        sub_notes: Vec<SubStepNoteView>,
        pitch_range: (u8, u8),
    },
    Rhythm {
        steps: Vec<PatternRhythmStep>,
    },
    Values {
        unit: ValueUnit,
        mode: ValueMode,
        range: (f32, f32),
        /// Dense sampled curve `(relative_step, value)` across `0..=page_steps`.
        curve: Vec<(f64, f32)>,
        /// Authored breakpoint diamonds `(relative_step, value)` inside the visible page.
        points: Vec<(f64, f32)>,
        cursor_value: f32,
        cursor_authored: bool,
    },
}

/// One resolved note event with fractional sub-step timing `offset ∈ [0.0, 1.0)`.
#[derive(Clone, Debug)]
pub struct SubStepNoteView {
    pub step_rel: usize,
    pub step_index: usize,
    pub offset: f32,
    pub midi: u8,
    pub note_name: String,
    pub velocity: f32,
    pub gate: f32,
    pub primary: bool,
    pub selected: bool,
}

/// One voice in a step's polyphonic note stack, ordered visually lowest-to-highest pitch.
#[derive(Clone, Debug)]
pub struct PatternVoiceView {
    pub midi: u8,
    pub note_name: String,
    pub velocity: f32,
    pub gate: f32,
    pub is_root: bool,
    pub selected: bool,
}

#[derive(Clone, Debug)]
pub struct PatternNoteStep {
    pub step_index: usize,
    pub active: bool,
    pub midi: u8,
    pub note_name: String,
    pub velocity: f32,
    pub gate: f32,
    pub chord: Vec<(u8, f32, f32)>,
    /// All active voices at this step sorted ascending by MIDI pitch (`0` = lowest).
    pub voices: Vec<PatternVoiceView>,
    pub overridden: bool,
    pub inherited: bool,
    pub conditional: bool,
    pub condition_label: String,
    pub selected: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct PatternRhythmStep {
    pub step_index: usize,
    pub active: bool,
    pub selected: bool,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct PatternViewTarget {
    pub pattern_id: Option<Id>,
    pub scope: EditScope,
    pub cursor: usize,
    pub source_cursor: usize,
    pub focused: bool,
}

impl App {
    pub(super) fn build_pattern_view(
        &self,
        target: PatternViewTarget,
        snapshot: &AudioSnapshot,
    ) -> PatternDeckView {
        let PatternViewTarget {
            pattern_id,
            scope,
            cursor,
            source_cursor,
            focused,
        } = target;
        let project = self.project();
        let root_pc = project.root_midi % 12;
        let mut scale_pcs = [false; 12];
        for &interval in &project.scale {
            let pc = (i16::from(project.root_midi) + i16::from(interval)).rem_euclid(12) as usize;
            scale_pcs[pc] = true;
        }
        let Some(pattern) = pattern_id.and_then(|id| self.pattern(id)) else {
            return PatternDeckView {
                empty: true,
                pattern_id: None,
                name: "PATTERN".into(),
                kind_label: "",
                scope_label: String::new(),
                zoom: self.view_state.pattern_zoom,
                period: 16,
                page_start: 0,
                page_steps: 16,
                cursor: 0,
                insert_mode: false,
                selected_voice: 0,
                voice_readout: None,
                focused,
                root_note: note_name(project.root_midi),
                scale_name: scale_name(&project.scale),
                root_pc,
                scale_pcs,
                playback_status: String::new(),
                playheads: Vec::new(),
                sources: Vec::new(),
                consumers: Vec::new(),
                source_previews: Vec::new(),
                mini_code: None,
                content: PatternContentView::Empty,
            };
        };

        let length = self.length_of(Some(pattern.id), scope).max(1);
        let visible_steps = self.pattern_visible_steps(length);
        let observed_step = self.observed_transport_step(snapshot);
        let request = self.playback_request_of(Some(pattern.id), scope, observed_step);
        let projection = request.and_then(|req| {
            playback::pattern_playback_live(
                project,
                self.flags(),
                self.live_projection(observed_step),
                req,
            )
            .ok()
        });
        let page = request.zip(projection.as_ref()).and_then(|(req, proj)| {
            self.follow
                .pattern_page_from_projection(req, proj, cursor, visible_steps)
                .ok()
        });
        let page_start = page
            .map_or((cursor / visible_steps) * visible_steps, |p| p.start)
            .min(length.saturating_sub(1));
        let page_steps = visible_steps.min(length.saturating_sub(page_start)).max(1);

        let markers = projection.map(|p| p.markers).unwrap_or_default();

        let sub_frac = if snapshot.playing {
            f64::from(snapshot.step_fraction.clamp(0.0, 0.9999))
        } else {
            0.0
        };
        let playheads: Vec<PatternPlayhead> = markers
            .iter()
            .map(|m| PatternPlayhead {
                position: m.step as f64 + (m.fraction + sub_frac).clamp(0.0, 0.9999),
                kind: m.kind,
                local_exception: m.local_exception,
            })
            .collect();

        let playback_status = self.pattern_playback_status(
            pattern.id,
            scope,
            request.map(|r| r.clock),
            &markers,
            snapshot.playing,
        );

        let raw_sources = pattern.kind.sources();
        let active_source = source_cursor.min(raw_sources.len().saturating_sub(1));
        let sources: Vec<PatternSourceChip> = raw_sources
            .into_iter()
            .enumerate()
            .map(|(i, id)| {
                let (name, kind) = self.pattern(id).map_or(("missing".into(), ""), |p| {
                    (p.name.clone(), kind_label(&p.kind))
                });
                PatternSourceChip {
                    id,
                    name,
                    kind_label: kind,
                    selected: i == active_source,
                }
            })
            .collect();

        let placement_exceptions: Vec<u32> = match scope {
            EditScope::Placement(pid) => self
                .placement(pid)
                .map(|(_, pl)| pl.exceptions.iter().map(|e| e.step).collect())
                .unwrap_or_default(),
            EditScope::Definition => Vec::new(),
        };

        let content = match &pattern.kind {
            PatternKind::Values {
                unit, mode, points, ..
            } => {
                let (low, high) = if *unit == ValueUnit::Degrees {
                    let mut lo = -7.0_f32;
                    let mut hi = 7.0_f32;
                    for pt in points {
                        lo = lo.min(pt.value - 1.0);
                        hi = hi.max(pt.value + 1.0);
                    }
                    (lo, hi)
                } else {
                    (0.0_f32, 1.0_f32)
                };
                let samples = (page_steps * 4).clamp(32, 256);
                let mut curve = Vec::with_capacity(samples + 1);
                for i in 0..=samples {
                    let rel = page_steps as f64 * (i as f64 / samples as f64);
                    let v = project
                        .value_at(pattern.id, page_start as f64 + rel)
                        .unwrap_or(0.0);
                    curve.push((rel, v));
                }
                let visible_points = points
                    .iter()
                    .filter(|p| {
                        let s = p.step as usize;
                        s >= page_start && s < page_start + page_steps
                    })
                    .map(|p| ((p.step as usize - page_start) as f64, p.value))
                    .collect();
                let cursor_value = project.value_at(pattern.id, cursor as f64).unwrap_or(0.0);
                let cursor_authored = points.iter().any(|p| p.step as usize == cursor);
                PatternContentView::Values {
                    unit: *unit,
                    mode: *mode,
                    range: (low, high),
                    curve,
                    points: visible_points,
                    cursor_value,
                    cursor_authored,
                }
            }
            _ => match self.value_of(Some(pattern.id), scope, page_start) {
                Ok(PatternValue::Rhythm(_)) => {
                    let mut steps = Vec::with_capacity(page_steps);
                    for step_index in page_start..page_start + page_steps {
                        let active = matches!(
                            self.value_of(Some(pattern.id), scope, step_index),
                            Ok(PatternValue::Rhythm(true))
                        );
                        steps.push(PatternRhythmStep {
                            step_index,
                            active,
                            selected: step_index == cursor,
                        });
                    }
                    PatternContentView::Rhythm { steps }
                }
                Ok(PatternValue::Value(_)) => {
                    let unit = match project.pattern_type(pattern.id) {
                        Ok(PatternType::Values(u)) => u,
                        _ => ValueUnit::Normalized,
                    };
                    let mode = project.value_mode(pattern.id).unwrap_or(ValueMode::Hold);
                    let mut curve = Vec::with_capacity(page_steps);
                    let mut points = Vec::with_capacity(page_steps);
                    let (mut lo, mut hi) = if unit == ValueUnit::Degrees {
                        (-7.0_f32, 7.0_f32)
                    } else {
                        (0.0_f32, 1.0_f32)
                    };
                    for offset in 0..page_steps {
                        let step_index = page_start + offset;
                        let v = match self.value_of(Some(pattern.id), scope, step_index) {
                            Ok(PatternValue::Value(v)) => v,
                            _ => 0.0,
                        };
                        if unit == ValueUnit::Degrees {
                            lo = lo.min(v - 1.0);
                            hi = hi.max(v + 1.0);
                        } else {
                            lo = lo.min(v);
                            hi = hi.max(v);
                        }
                        curve.push((offset as f64, v));
                        points.push((offset as f64, v));
                    }
                    if (hi - lo).abs() < 1e-4 {
                        hi = lo + 1.0;
                    }
                    let cursor_value = match self.value_of(Some(pattern.id), scope, cursor) {
                        Ok(PatternValue::Value(v)) => v,
                        _ => 0.0,
                    };
                    PatternContentView::Values {
                        unit,
                        mode,
                        range: (lo, hi),
                        curve,
                        points,
                        cursor_value,
                        cursor_authored: true,
                    }
                }
                _ => {
                    let mut steps = Vec::with_capacity(page_steps);
                    let mut sub_notes = Vec::new();
                    let mut has_substep_detail = false;
                    let mut ev_buf = Vec::with_capacity(4);
                    let mut min_midi = 127u8;
                    let mut max_midi = 0u8;
                    for (rel, step_index) in (page_start..page_start + page_steps).enumerate() {
                        let step = match self.value_of(Some(pattern.id), scope, step_index) {
                            Ok(PatternValue::Notes(s)) => s,
                            _ => cadence_core::music::Step::default(),
                        };
                        let midi = project.pitch_midi(step.degree, step.accidental, 0);
                        let mut chord = Vec::new();
                        let sorted_voices = crate::editing::step_voices_sorted(project, &step);
                        let clamped_voice = if sorted_voices.is_empty() {
                            0
                        } else {
                            self.note_voice.min(sorted_voices.len() - 1)
                        };
                        let voices: Vec<PatternVoiceView> = sorted_voices
                            .iter()
                            .enumerate()
                            .map(|(vi, v)| PatternVoiceView {
                                midi: v.midi,
                                note_name: note_name(v.midi),
                                velocity: v.velocity,
                                gate: v.gate,
                                is_root: v.slot == crate::editing::StepVoiceSlot::Root,
                                selected: step_index == cursor && vi == clamped_voice,
                            })
                            .collect();
                        if step.active {
                            min_midi = min_midi.min(midi);
                            max_midi = max_midi.max(midi);
                            for tone in &step.chord {
                                let cmidi = project.pitch_midi(tone.degree, tone.accidental, 0);
                                min_midi = min_midi.min(cmidi);
                                max_midi = max_midi.max(cmidi);
                                chord.push((cmidi, tone.velocity, tone.gate));
                            }
                        }
                        let overridden = matches!(scope, EditScope::Placement(_))
                            && placement_exceptions.contains(&(step_index as u32));
                        let inherited = matches!(scope, EditScope::Placement(_)) && !overridden;
                        if !overridden {
                            let eval_time = match scope {
                                EditScope::Definition => Some(step_index as i64),
                                EditScope::Placement(pid) => self.placement(pid).map(|(_, pl)| {
                                    pl.pattern_time(pl.start_step + step_index as u32)
                                }),
                            };
                            if let Some(t) = eval_time {
                                ev_buf.clear();
                                if cadence_core::evaluate::evaluate_notes_into(
                                    project,
                                    pattern.id,
                                    t,
                                    &mut ev_buf,
                                )
                                .is_ok()
                                {
                                    if ev_buf.len() > 1 || ev_buf.iter().any(|e| e.offset > 0.001) {
                                        has_substep_detail = true;
                                    }
                                    for ev in &ev_buf {
                                        let emidi = project.pitch_midi(
                                            ev.step.degree,
                                            ev.step.accidental,
                                            0,
                                        );
                                        min_midi = min_midi.min(emidi);
                                        max_midi = max_midi.max(emidi);
                                        sub_notes.push(SubStepNoteView {
                                            step_rel: rel,
                                            step_index,
                                            offset: ev.offset,
                                            midi: emidi,
                                            note_name: note_name(emidi),
                                            velocity: ev.step.velocity,
                                            gate: ev.step.gate,
                                            primary: true,
                                            selected: step_index == cursor,
                                        });
                                        for tone in &ev.step.chord {
                                            let cmidi =
                                                project.pitch_midi(tone.degree, tone.accidental, 0);
                                            min_midi = min_midi.min(cmidi);
                                            max_midi = max_midi.max(cmidi);
                                            sub_notes.push(SubStepNoteView {
                                                step_rel: rel,
                                                step_index,
                                                offset: ev.offset,
                                                midi: cmidi,
                                                note_name: note_name(cmidi),
                                                velocity: tone.velocity,
                                                gate: tone.gate,
                                                primary: false,
                                                selected: step_index == cursor,
                                            });
                                        }
                                    }
                                }
                            }
                        }
                        steps.push(PatternNoteStep {
                            step_index,
                            active: step.active,
                            midi,
                            note_name: note_name(midi),
                            velocity: step.velocity,
                            gate: step.gate,
                            chord,
                            voices,
                            overridden,
                            inherited,
                            conditional: !step.condition.is_always(),
                            condition_label: step.condition.label(),
                            selected: step_index == cursor,
                        });
                    }
                    if !has_substep_detail {
                        sub_notes.clear();
                    }
                    let pitch_range = if min_midi <= max_midi {
                        (
                            min_midi.saturating_sub(3),
                            max_midi.saturating_add(3).min(127),
                        )
                    } else {
                        (
                            project.root_midi.saturating_sub(5),
                            project.root_midi.saturating_add(14).min(127),
                        )
                    };
                    PatternContentView::Notes {
                        steps,
                        sub_notes,
                        pitch_range,
                    }
                }
            },
        };

        let scope_label = match scope {
            EditScope::Definition => "SHARED DEFINITION".into(),
            EditScope::Placement(id) => format!("PLACEMENT {id} · local steps"),
        };

        let source_previews: Vec<SourcePreviewStrip> = pattern
            .kind
            .sources()
            .into_iter()
            .take(3)
            .filter_map(|src_id| {
                let src = self.pattern(src_id)?;
                let src_period = self.pattern_period(src_id);
                let steps = page_steps.clamp(1, 64);
                let preview = match project.pattern_type(src_id).ok()? {
                    cadence_core::material::PatternType::Rhythm => {
                        let hits = (0..steps)
                            .map(|i| {
                                project
                                    .evaluate_pattern(src_id, (page_start + i) as i64)
                                    .is_ok_and(|v| v.active())
                            })
                            .collect();
                        SourcePreviewKind::Rhythm(hits)
                    }
                    cadence_core::material::PatternType::Values(_) => {
                        let raw: Vec<f32> = (0..steps)
                            .map(|i| {
                                match project.evaluate_pattern(src_id, (page_start + i) as i64) {
                                    Ok(PatternValue::Value(v)) => v,
                                    _ => 0.0,
                                }
                            })
                            .collect();
                        let lo = raw.iter().copied().fold(f32::INFINITY, f32::min);
                        let hi = raw.iter().copied().fold(f32::NEG_INFINITY, f32::max);
                        let span = (hi - lo).max(1e-4);
                        let norm = raw
                            .into_iter()
                            .map(|v| ((v - lo) / span).clamp(0.0, 1.0))
                            .collect();
                        SourcePreviewKind::Values(norm)
                    }
                    cadence_core::material::PatternType::Notes => {
                        let mut midis = Vec::with_capacity(steps);
                        let mut lo = 127u8;
                        let mut hi = 0u8;
                        for i in 0..steps {
                            let m = match project.evaluate_pattern(src_id, (page_start + i) as i64)
                            {
                                Ok(PatternValue::Notes(s)) if s.active => {
                                    let midi = project.pitch_midi(s.degree, s.accidental, 0);
                                    lo = lo.min(midi);
                                    hi = hi.max(midi);
                                    Some(midi)
                                }
                                _ => None,
                            };
                            midis.push(m);
                        }
                        let span = f32::from(hi.saturating_sub(lo).max(1));
                        let norm = midis
                            .into_iter()
                            .map(|opt| opt.map(|m| (f32::from(m - lo) / span).clamp(0.0, 1.0)))
                            .collect();
                        SourcePreviewKind::Notes(norm)
                    }
                };
                Some(SourcePreviewStrip {
                    id: src_id,
                    name: src.name.clone(),
                    kind_label: kind_label(&src.kind),
                    period: src_period,
                    kind: preview,
                })
            })
            .collect();

        let mini_code = match &pattern.kind {
            PatternKind::Mini { code, .. } => Some(code.clone()),
            _ => None,
        };

        let (selected_voice, voice_readout) = match self.value_of(Some(pattern.id), scope, cursor) {
            Ok(PatternValue::Notes(step)) if step.active => {
                let voices = crate::editing::step_voices_sorted(project, &step);
                let idx = if voices.is_empty() {
                    0
                } else {
                    self.note_voice.min(voices.len() - 1)
                };
                (idx, crate::editing::format_voice_readout(&voices, idx))
            }
            _ => (0, None),
        };

        let consumers = if matches!(content, PatternContentView::Values { .. }) {
            self.values_pattern_consumers(pattern.id)
        } else {
            Vec::new()
        };

        PatternDeckView {
            empty: false,
            pattern_id: Some(pattern.id),
            name: pattern.name.clone(),
            kind_label: kind_label(&pattern.kind),
            scope_label,
            zoom: self.pattern_effective_zoom(length),
            period: length,
            page_start,
            page_steps,
            cursor,
            insert_mode: focused && self.input.mode == Mode::Insert,
            selected_voice,
            voice_readout,
            focused,
            root_note: note_name(project.root_midi),
            scale_name: scale_name(&project.scale),
            root_pc,
            scale_pcs,
            playback_status,
            playheads,
            sources,
            consumers,
            source_previews,
            mini_code,
            content,
        }
    }

    fn pattern_playback_status(
        &self,
        pattern_id: Id,
        scope: EditScope,
        clock: Option<PlaybackClock>,
        markers: &[PlaybackMarker],
        playing: bool,
    ) -> String {
        if let Some(first) = markers.first() {
            let mode = if playing { "▶ PLAY" } else { "∥ PAUSED" };
            return format!(
                "{mode} step {}{}{}",
                first.step + 1,
                if markers.len() > 1 {
                    format!(" · {} uses", markers.len())
                } else {
                    String::new()
                },
                if first.local_exception {
                    " · local exception"
                } else {
                    ""
                }
            );
        }
        if self.is_solo_auditioning() {
            return "Audition uses other material".into();
        }
        let project = self.project();
        match scope {
            EditScope::Placement(placement_id) => {
                if matches!(clock, Some(PlaybackClock::PastEnd)) {
                    return "At song end".into();
                }
                let Some((lane, place)) = self.placement(placement_id) else {
                    return "Unused in song".into();
                };
                let end = place.end_step().unwrap_or(place.start_step);
                if let Some(PlaybackClock::Song { step }) = clock {
                    let song_time = step.rem_euclid(project.arrangement_steps().max(1) as f64);
                    if song_time >= f64::from(place.start_step) && song_time < f64::from(end) {
                        if !self.flags().lane_enabled(lane.id) {
                            return "Lane muted".into();
                        }
                        if self.live.lane_state(lane.id).is_some() {
                            return "Lane overridden".into();
                        }
                        return "Placed · idle here".into();
                    }
                }
                format_placed_bars(&[(place.start_step, end)])
            }
            EditScope::Definition => {
                if project.consumers(pattern_id).is_empty() {
                    return "Unused in song".into();
                }
                let roots = playback::consuming_patterns(project, pattern_id);
                let using_parts: HashSet<Id> = project
                    .parts
                    .iter()
                    .filter(|part| {
                        roots.contains(&part.pattern)
                            || part
                                .parameter_links
                                .iter()
                                .any(|link| roots.contains(&link.pattern))
                    })
                    .map(|part| part.id)
                    .collect();

                let mut placements: Vec<(Id, u32, u32)> = Vec::new();
                let mut in_lane_bank = false;
                for lane in &project.lanes {
                    if lane.parts.iter().any(|lp| using_parts.contains(&lp.part)) {
                        in_lane_bank = true;
                    }
                    for place in &lane.placements {
                        if using_parts.contains(&place.part) {
                            let end = place.end_step().unwrap_or(place.start_step);
                            placements.push((lane.id, place.start_step, end));
                        }
                    }
                }

                let modulations: Vec<&cadence_core::material::Modulation> = project
                    .modulations
                    .iter()
                    .filter(|m| roots.contains(&m.pattern))
                    .collect();
                let drives_filter_env = project
                    .mixer
                    .channels
                    .iter()
                    .flat_map(|c| c.inserts.iter())
                    .chain(project.mixer.master.inserts.iter())
                    .any(|dev| {
                        matches!(
                            dev.kind,
                            EffectKind::Filter(patch)
                                if patch.env_pattern_id.is_some_and(|id| roots.contains(&id))
                        )
                    });
                let drives_launch_cycle = matches!(
                    project.timing.launch,
                    Boundary::Cycle { pattern } if roots.contains(&pattern)
                );

                let macro_index = modulations.iter().find_map(|m| match m.target {
                    ModulationTarget::Macro { index } => Some(index),
                    _ => None,
                });

                if matches!(clock, Some(PlaybackClock::PastEnd)) {
                    if !placements.is_empty() {
                        return "At song end".into();
                    }
                    if let Some(idx) = macro_index {
                        return format!("At song end · drives M{}", usize::from(idx) + 1);
                    }
                    if !modulations.is_empty() {
                        return "At song end · modulation".into();
                    }
                    if drives_filter_env {
                        return "At song end · Filter Env".into();
                    }
                }

                if let Some(PlaybackClock::Song { step }) = clock {
                    let song_time = step.rem_euclid(project.arrangement_steps().max(1) as f64);
                    let covering: Vec<Id> = placements
                        .iter()
                        .filter(|&&(_, start, end)| {
                            song_time >= f64::from(start) && song_time < f64::from(end)
                        })
                        .map(|&(lane_id, _, _)| lane_id)
                        .collect();
                    if !covering.is_empty() {
                        if covering
                            .iter()
                            .all(|&lane_id| !self.flags().lane_enabled(lane_id))
                        {
                            return "Lane muted".into();
                        }
                        if covering
                            .iter()
                            .any(|&lane_id| self.live.lane_state(lane_id).is_some())
                        {
                            return "Lane overridden".into();
                        }
                        if covering
                            .iter()
                            .any(|&lane_id| !self.flags().lane_enabled(lane_id))
                        {
                            return "Lane muted".into();
                        }
                        return "Placed · idle here".into();
                    }
                    if self.live.active().iter().any(|ov| {
                        using_parts.contains(&ov.part.part) && !self.flags().lane_enabled(ov.lane)
                    }) {
                        return "Lane muted".into();
                    }
                }

                if !placements.is_empty() {
                    let spans: Vec<(u32, u32)> =
                        placements.iter().map(|&(_, s, e)| (s, e)).collect();
                    return format_placed_bars(&spans);
                }
                if let Some(idx) = macro_index {
                    return format!("Drives M{} · idle here", usize::from(idx) + 1);
                }
                if !modulations.is_empty() {
                    return "Modulation · idle here".into();
                }
                if drives_filter_env {
                    return "Filter Env · idle here".into();
                }
                if drives_launch_cycle {
                    return "Launch cycle clock".into();
                }
                if in_lane_bank {
                    return "In lane bank · idle".into();
                }
                "Used in song · idle here".into()
            }
        }
    }
}

fn format_placed_bars(spans: &[(u32, u32)]) -> String {
    let mut bars: Vec<(u32, u32)> = spans
        .iter()
        .filter(|&&(s, e)| e > s)
        .map(|&(s, e)| (s / 16 + 1, e.saturating_sub(1) / 16 + 1))
        .collect();
    if bars.is_empty() {
        return "Placed · idle here".into();
    }
    bars.sort_unstable();
    let mut merged: Vec<(u32, u32)> = Vec::with_capacity(bars.len());
    for (b0, b1) in bars {
        if let Some(last) = merged.last_mut() {
            if b0 <= last.1.saturating_add(1) {
                last.1 = last.1.max(b1);
                continue;
            }
        }
        merged.push((b0, b1));
    }
    if merged.len() == 1 {
        let (b0, b1) = merged[0];
        if b0 == b1 {
            format!("Placed (bar {b0})")
        } else {
            format!("Placed (bars {b0}–{b1})")
        }
    } else {
        "Placed · idle here".into()
    }
}
