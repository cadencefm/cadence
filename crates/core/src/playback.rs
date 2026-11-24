//! Observational playback coordinates for retained authored material.
//!
//! These are clocks of active uses, including rests, rather than a reconstruction
//! from note-on events. The UI supplies the engine's observed position; this module
//! never extrapolates wall time, moves an edit cursor, or changes authored data.
use crate::material::{
    Audition, Id, Override, PatternKind, PhaseMode, Project, MAX_BARS, MAX_DEPTH,
    MAX_GLOBAL_MODULATIONS, MAX_LANES, MAX_LINKS, MAX_PARTS, MAX_PATTERNS, MAX_PLACEMENTS,
    STEPS_PER_BAR,
};
use crate::mixer::{EffectKind, LiveFlags};
use crate::timeline::Segment;
use anyhow::{ensure, Context, Result};
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PlaybackClock {
    Song {
        step: f64,
    },
    /// Solo audition root and finite loop length. Arrangement uses
    /// and song-global modulations are not active in this clock domain.
    Audition {
        pattern: Id,
        step: f64,
        length: usize,
    },
    /// Transport is past a continuing or stopped song end: no arrangement uses wrap.
    PastEnd,
}

/// Optional live performance context layered onto pattern playback projection.
#[derive(Clone, Copy, Debug, Default)]
pub struct LiveProjection<'a> {
    pub transport_step: u64,
    pub segment: Option<Segment>,
    pub active: &'a [Override],
    pub audition: Option<Audition>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaybackScope {
    Definition,
    Placement(Id),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlaybackRequest {
    pub pattern: Id,
    pub scope: PlaybackScope,
    pub preferred_placement: Option<Id>,
    pub clock: PlaybackClock,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaybackUse {
    Placement,
    /// Active live lane override playing a part by value.
    Override,
    /// The placement clock at which per-note values are sampled on eligible
    /// onsets. It is not an effective-value cursor for already-sounding voices.
    ParameterLink,
    Modulation,
    Audition,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlaybackMarker {
    /// Wrapped source/editor coordinate, suitable for plotting at ordinary sizes.
    pub position: f64,
    /// Exact integer coordinate, retained for very long derived periods and labels.
    pub step: usize,
    pub fraction: f64,
    /// Unwrapped clock before the selected source's own period is applied.
    pub source_time: f64,
    /// Zero-based source pass; negative offsets may produce negative passes.
    pub cycle: i64,
    pub lane: Option<Id>,
    pub placement: Option<Id>,
    pub part: Option<Id>,
    pub modulation: Option<Id>,
    pub effect: Option<Id>,
    pub kind: PlaybackUse,
    pub preferred: bool,
    /// The placement replaces its source note at this position. Its underlying
    /// clock still progresses, but this badge prevents implying an unmodified use.
    pub local_exception: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PatternPlayback {
    /// Literal editors use stored length; derivations use their full repeat period.
    pub period: usize,
    /// Preferred active placement first, then other active uses in authored order.
    /// Coincident markers retain separate provenance rather than hiding uses.
    pub markers: Vec<PlaybackMarker>,
}

/// Project the observed transport into one selected authored pattern/editor.
///
/// Propagates incoming placement/override/modulation clocks through temporal
/// pattern operators (`Shift`, `Reverse`, `TimeScale`, `Concat`, `Every`) to
/// match their evaluation semantics.
pub fn pattern_playback(
    project: &Project,
    flags: &LiveFlags,
    request: PlaybackRequest,
) -> Result<PatternPlayback> {
    pattern_playback_live(project, flags, LiveProjection::default(), request)
}

/// Project observed transport plus active live overrides and layered audition
/// into one selected authored pattern/editor.
pub fn pattern_playback_live(
    project: &Project,
    flags: &LiveFlags,
    live: LiveProjection<'_>,
    request: PlaybackRequest,
) -> Result<PatternPlayback> {
    ensure!(
        project.patterns.len() <= MAX_PATTERNS
            && project.parts.len() <= MAX_PARTS
            && project.lanes.len() <= MAX_LANES
            && project
                .lanes
                .iter()
                .map(|l| l.placements.len())
                .sum::<usize>()
                <= MAX_PLACEMENTS
            && project.modulations.len() <= MAX_GLOBAL_MODULATIONS,
        "Playback projection exceeds authoring bounds"
    );
    ensure!(
        (1..=MAX_BARS).contains(&project.bars),
        "Invalid song horizon"
    );
    let period = match request.scope {
        PlaybackScope::Definition => project.editor_period(request.pattern)?,
        PlaybackScope::Placement(id) => {
            let (_, placement) = project.placement(id).context("Placement missing")?;
            let part = project.part(placement.part).context("Part missing")?;
            ensure!(
                part.pattern == request.pattern,
                "Local editor pattern does not match its placement"
            );
            ensure!(
                placement.length_steps > 0,
                "Placement length must be positive"
            );
            placement.length_steps as usize
        }
    };
    let mut result = PatternPlayback {
        period,
        markers: Vec::new(),
    };
    let roots = consuming_patterns(project, request.pattern);
    let mut projected = Vec::with_capacity(4);
    match request.clock {
        PlaybackClock::Audition {
            pattern,
            step,
            length,
        } => {
            ensure!(
                step.is_finite() && step >= 0.0,
                "Invalid observed audition position"
            );
            ensure!(
                length > 0 && length <= MAX_BARS * STEPS_PER_BAR,
                "Invalid audition length"
            );
            project
                .pattern(pattern)
                .context("Audition pattern missing")?;
            if request.scope == PlaybackScope::Definition && roots.contains(&pattern) {
                let time = step.rem_euclid(length as f64);
                projected.clear();
                project_clock_to_target(
                    project,
                    pattern,
                    request.pattern,
                    time,
                    0,
                    &roots,
                    &mut projected,
                )?;
                for &pt in &projected {
                    result
                        .markers
                        .push(marker(pt, period, PlaybackUse::Audition));
                }
            }
        }
        PlaybackClock::Song { .. } | PlaybackClock::PastEnd => {
            let (song_time, sub_frac) = match request.clock {
                PlaybackClock::Song { step } => {
                    ensure!(
                        step.is_finite() && step >= 0.0,
                        "Invalid observed song position"
                    );
                    let frac = step - step.floor();
                    (
                        Some(step.rem_euclid(project.arrangement_steps() as f64)),
                        frac,
                    )
                }
                PlaybackClock::PastEnd => (None, 0.0),
                PlaybackClock::Audition { .. } => unreachable!(),
            };
            let song_len = project.arrangement_steps() as u64;
            for lane in &project.lanes {
                if !flags.lane_enabled(lane.id) {
                    continue;
                }
                if let Some(ov) = live.active.iter().find(|a| a.lane == lane.id) {
                    if request.scope == PlaybackScope::Definition {
                        if let Some(part) = project.part(ov.part.part) {
                            if let Some(pattern_time) =
                                ov.pattern_time(live.transport_step, live.segment, song_len)
                            {
                                let pt = pattern_time as f64 + sub_frac;
                                if roots.contains(&part.pattern) {
                                    projected.clear();
                                    project_clock_to_target(
                                        project,
                                        part.pattern,
                                        request.pattern,
                                        pt,
                                        0,
                                        &roots,
                                        &mut projected,
                                    )?;
                                    for &clock in &projected {
                                        let mut point =
                                            marker(clock, period, PlaybackUse::Override);
                                        point.lane = Some(lane.id);
                                        point.part = Some(part.id);
                                        result.markers.push(point);
                                    }
                                }
                                projected.clear();
                                for link in &part.parameter_links {
                                    if roots.contains(&link.pattern) {
                                        project_clock_to_target(
                                            project,
                                            link.pattern,
                                            request.pattern,
                                            pt,
                                            0,
                                            &roots,
                                            &mut projected,
                                        )?;
                                    }
                                }
                                for &clock in &projected {
                                    let mut point =
                                        marker(clock, period, PlaybackUse::ParameterLink);
                                    point.lane = Some(lane.id);
                                    point.part = Some(part.id);
                                    result.markers.push(point);
                                }
                            }
                        }
                    }
                    continue;
                }
                let Some(time) = song_time else {
                    continue;
                };
                for place in &lane.placements {
                    let end = place.end_step().context("Placement clock overflow")?;
                    if time < f64::from(place.start_step) || time >= f64::from(end) {
                        continue;
                    }
                    if matches!(request.scope, PlaybackScope::Placement(id) if id != place.id) {
                        continue;
                    }
                    let part = project.part(place.part).context("Part missing")?;
                    ensure!(
                        part.parameter_links.len() <= MAX_LINKS,
                        "Too many part parameter links"
                    );
                    let local_time = time - f64::from(place.start_step);
                    let pattern_time = match place.phase {
                        PhaseMode::Song => time,
                        PhaseMode::Restart => local_time,
                    } + f64::from(place.offset_steps);
                    let local_exception = place
                        .exceptions
                        .iter()
                        .any(|o| f64::from(o.step) == local_time.floor());
                    let mut add_use = |kind, clock| {
                        let mut point = marker(clock, period, kind);
                        point.lane = Some(lane.id);
                        point.placement = Some(place.id);
                        point.part = Some(part.id);
                        point.preferred = request.preferred_placement == Some(place.id)
                            || matches!(request.scope, PlaybackScope::Placement(_));
                        point.local_exception = local_exception;
                        result.markers.push(point);
                    };
                    if matches!(request.scope, PlaybackScope::Placement(_)) {
                        add_use(PlaybackUse::Placement, local_time);
                    } else {
                        if roots.contains(&part.pattern) {
                            projected.clear();
                            project_clock_to_target(
                                project,
                                part.pattern,
                                request.pattern,
                                pattern_time,
                                0,
                                &roots,
                                &mut projected,
                            )?;
                            for &clock in &projected {
                                add_use(PlaybackUse::Placement, clock);
                            }
                        }
                        projected.clear();
                        for link in &part.parameter_links {
                            if roots.contains(&link.pattern) {
                                project_clock_to_target(
                                    project,
                                    link.pattern,
                                    request.pattern,
                                    pattern_time,
                                    0,
                                    &roots,
                                    &mut projected,
                                )?;
                            }
                        }
                        for &clock in &projected {
                            add_use(PlaybackUse::ParameterLink, clock);
                        }
                    }
                }
            }
            if request.scope == PlaybackScope::Definition {
                if let Some(aud) = live
                    .audition
                    .filter(|a| !a.solo && live.transport_step >= a.anchor)
                {
                    if roots.contains(&aud.pattern) {
                        let aud_time =
                            ((live.transport_step - aud.anchor) % aud.length.max(1) as u64) as f64
                                + sub_frac;
                        projected.clear();
                        project_clock_to_target(
                            project,
                            aud.pattern,
                            request.pattern,
                            aud_time,
                            0,
                            &roots,
                            &mut projected,
                        )?;
                        for &clock in &projected {
                            result
                                .markers
                                .push(marker(clock, period, PlaybackUse::Audition));
                        }
                    }
                }
                if let Some(time) = song_time {
                    for modulation in &project.modulations {
                        if roots.contains(&modulation.pattern) {
                            projected.clear();
                            project_clock_to_target(
                                project,
                                modulation.pattern,
                                request.pattern,
                                time,
                                0,
                                &roots,
                                &mut projected,
                            )?;
                            for &clock in &projected {
                                let mut point = marker(clock, period, PlaybackUse::Modulation);
                                point.modulation = Some(modulation.id);
                                result.markers.push(point);
                            }
                        }
                    }
                    for dev in project.all_inserts() {
                        if let EffectKind::Filter(patch) = dev.kind {
                            if let Some(env_pat) = patch.env_pattern_id {
                                if roots.contains(&env_pat) {
                                    projected.clear();
                                    project_clock_to_target(
                                        project,
                                        env_pat,
                                        request.pattern,
                                        time,
                                        0,
                                        &roots,
                                        &mut projected,
                                    )?;
                                    for &clock in &projected {
                                        let mut point =
                                            marker(clock, period, PlaybackUse::Modulation);
                                        point.effect = Some(dev.id);
                                        result.markers.push(point);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    // Stable ordering retains authored lane/use order among equal priorities.
    result.markers.sort_by_key(|point| !point.preferred);
    Ok(result)
}

/// Propagate an incoming fractional clock `time` from root pattern `from` down to
/// `target`, transforming through temporal operators (`Shift`, `Reverse`, `TimeScale`,
/// `Concat`, `Every`) and deduplicating identical projected clocks across diamond DAGs.
fn project_clock_to_target(
    project: &Project,
    from: Id,
    target: Id,
    time: f64,
    depth: usize,
    ancestors: &HashSet<Id>,
    out: &mut Vec<f64>,
) -> Result<()> {
    ensure!(
        depth < MAX_DEPTH,
        "Playback projection exceeds pattern depth"
    );
    if from == target {
        if !out.iter().any(|&existing| (existing - time).abs() < 1e-9) {
            out.push(time);
        }
        return Ok(());
    }
    if !ancestors.contains(&from) {
        return Ok(());
    }
    let pattern = project.pattern(from).context("Pattern missing")?;
    match &pattern.kind {
        PatternKind::Shift { source, steps } => {
            project_clock_to_target(
                project,
                *source,
                target,
                time - f64::from(*steps),
                depth + 1,
                ancestors,
                out,
            )?;
        }
        PatternKind::Reverse { source } => {
            let p = project.pattern_period(*source)? as f64;
            let cycle = (time / p).floor();
            let idx = time.rem_euclid(p);
            let step = idx.floor();
            let frac = idx - step;
            let rev_time = cycle * p + (p - 1.0 - step) + frac;
            project_clock_to_target(
                project,
                *source,
                target,
                rev_time,
                depth + 1,
                ancestors,
                out,
            )?;
        }
        PatternKind::TimeScale { source, num, den } => {
            let scaled = (time * f64::from((*num).max(1))) / f64::from((*den).max(1));
            project_clock_to_target(project, *source, target, scaled, depth + 1, ancestors, out)?;
        }
        PatternKind::Every {
            interval,
            base,
            alt,
        } => {
            let period = project.pattern_period(*base)? as i64;
            let cycle = (time.floor() as i64).div_euclid(period);
            let chosen = if cycle.rem_euclid(i64::from(*interval)) == i64::from(*interval) - 1 {
                *alt
            } else {
                *base
            };
            project_clock_to_target(project, chosen, target, time, depth + 1, ancestors, out)?;
        }
        PatternKind::Concat { parts } => {
            let mut periods = Vec::with_capacity(parts.len());
            let mut total = 0.0_f64;
            for &part in parts {
                let p = project.pattern_period(part)? as f64;
                periods.push(p);
                total += p;
            }
            let mut rem = time.rem_euclid(total.max(1.0));
            let mut chosen = parts[0];
            for (&part, &p) in parts.iter().zip(&periods) {
                if rem < p {
                    chosen = part;
                    break;
                }
                rem -= p;
            }
            project_clock_to_target(project, chosen, target, rem, depth + 1, ancestors, out)?;
        }
        other => {
            for source in other.sources() {
                if ancestors.contains(&source) {
                    project_clock_to_target(
                        project,
                        source,
                        target,
                        time,
                        depth + 1,
                        ancestors,
                        out,
                    )?;
                }
            }
        }
    }
    Ok(())
}

/// Reverse dependency traversal visits each definition once, including shared
/// DAG branches. Returns `selected` plus every derived pattern that transitively
/// consumes it. A validated Project has at most MAX_PATTERNS definitions.
pub fn consuming_patterns(project: &Project, selected: Id) -> HashSet<Id> {
    let mut consumers: HashMap<Id, Vec<Id>> = HashMap::new();
    for pattern in &project.patterns {
        for source in pattern.kind.sources() {
            consumers.entry(source).or_default().push(pattern.id);
        }
    }
    let mut roots = HashSet::from([selected]);
    let mut pending = VecDeque::from([selected]);
    while let Some(source) = pending.pop_front() {
        if let Some(parents) = consumers.get(&source) {
            for &parent in parents {
                if roots.insert(parent) {
                    pending.push_back(parent);
                }
            }
        }
    }
    roots
}

fn marker(time: f64, period: usize, kind: PlaybackUse) -> PlaybackMarker {
    let integer = time.floor() as i128;
    let fraction = time - time.floor();
    let step = integer.rem_euclid(period as i128) as usize;
    PlaybackMarker {
        position: step as f64 + fraction,
        step,
        fraction,
        source_time: time,
        cycle: integer.div_euclid(period as i128) as i64,
        lane: None,
        placement: None,
        part: None,
        modulation: None,
        effect: None,
        kind,
        preferred: false,
        local_exception: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::{LanePart, Modulation, ModulationTarget, Pattern, PatternKind};
    use crate::params::InstrumentParam;

    #[test]
    fn playback_accepts_over_32_modulations_projects_temporal_ops_and_substep_overrides() {
        let mut project = Project::demo();
        let flags = LiveFlags::default();
        // Add 40 global modulations across Rack module/cable params.
        let val_pat = project.patterns[6].id;
        let mut next_id = project.max_id() + 1;
        let targets: Vec<(Id, InstrumentParam)> = project
            .instruments
            .iter()
            .flat_map(|inst| {
                let inst_id = inst.id;
                inst.rack.zones.iter().flat_map(move |z| {
                    let z_id = z.id;
                    let mod_targets = z.modules.iter().flat_map(move |m| {
                        let m_id = m.id;
                        m.kind
                            .tag()
                            .params()
                            .iter()
                            .copied()
                            .filter(|p| p.is_automatable())
                            .map(move |param| {
                                (
                                    inst_id,
                                    InstrumentParam::Module {
                                        zone: z_id,
                                        module: m_id,
                                        param,
                                    },
                                )
                            })
                    });
                    let cable_targets = z.cables.iter().map(move |c| {
                        (
                            inst_id,
                            InstrumentParam::Cable {
                                zone: z_id,
                                cable: c.id,
                            },
                        )
                    });
                    mod_targets.chain(cable_targets)
                })
            })
            .collect();
        for (inst_id, parameter) in targets {
            if project.modulations.len() >= 40 {
                break;
            }
            let target = ModulationTarget::Instrument {
                instrument: inst_id,
                parameter,
            };
            if project.modulations.iter().any(|m| m.target == target) {
                continue;
            }
            project.modulations.push(Modulation {
                id: next_id,
                name: format!("Mod {next_id}"),
                target,
                pattern: val_pat,
            });
            next_id += 1;
        }
        assert!(project.modulations.len() >= 40);
        project.validate().unwrap();

        let res = pattern_playback(
            &project,
            &flags,
            PlaybackRequest {
                pattern: 1,
                scope: PlaybackScope::Definition,
                preferred_placement: None,
                clock: PlaybackClock::Song { step: 4.5 },
            },
        )
        .unwrap();
        assert_eq!(res.markers.len(), 1);

        // Temporal Shift/Reverse/TimeScale project exact transformed coordinates into source.
        let shift_id = next_id;
        next_id += 1;
        project.patterns.push(Pattern {
            id: shift_id,
            name: "ShiftedMotif".into(),
            kind: PatternKind::Shift {
                source: 1,
                steps: 2,
            },
        });
        let rev_id = next_id;
        next_id += 1;
        project.patterns.push(Pattern {
            id: rev_id,
            name: "ReversedMotif".into(),
            kind: PatternKind::Reverse { source: 1 },
        });
        let fast_id = next_id;
        project.patterns.push(Pattern {
            id: fast_id,
            name: "DoubleMotif".into(),
            kind: PatternKind::TimeScale {
                source: 1,
                num: 2,
                den: 1,
            },
        });
        project.parts[0].pattern = shift_id;
        let shifted = pattern_playback(
            &project,
            &flags,
            PlaybackRequest {
                pattern: 1,
                scope: PlaybackScope::Definition,
                preferred_placement: None,
                clock: PlaybackClock::Song { step: 4.25 },
            },
        )
        .unwrap();
        assert!((shifted.markers[0].position - 2.25).abs() < 1e-6);

        project.parts[0].pattern = rev_id;
        let reversed = pattern_playback(
            &project,
            &flags,
            PlaybackRequest {
                pattern: 1,
                scope: PlaybackScope::Definition,
                preferred_placement: None,
                clock: PlaybackClock::Song { step: 0.25 },
            },
        )
        .unwrap();
        // Reversing 5-step Motif at step 0.25 plays source step 4 (4.25)!
        assert!((reversed.markers[0].position - 4.25).abs() < 1e-6);

        project.parts[0].pattern = fast_id;
        let doubled = pattern_playback(
            &project,
            &flags,
            PlaybackRequest {
                pattern: 1,
                scope: PlaybackScope::Definition,
                preferred_placement: None,
                clock: PlaybackClock::Song { step: 1.25 },
            },
        )
        .unwrap();
        assert!((doubled.markers[0].position - 2.5).abs() < 1e-6);

        // Live Override preserves fractional sub-step playhead.
        project.parts[0].pattern = 1;
        let active = [Override {
            lane: 1,
            part: LanePart::new(1),
            anchor: 0,
        }];
        let ov_proj = pattern_playback_live(
            &project,
            &flags,
            LiveProjection {
                transport_step: 6,
                segment: None,
                active: &active,
                audition: None,
            },
            PlaybackRequest {
                pattern: 1,
                scope: PlaybackScope::Definition,
                preferred_placement: None,
                clock: PlaybackClock::Song { step: 6.375 },
            },
        )
        .unwrap();
        assert_eq!(ov_proj.markers.len(), 1);
        assert_eq!(ov_proj.markers[0].kind, PlaybackUse::Override);
        assert!((ov_proj.markers[0].fraction - 0.375).abs() < 1e-6);
    }

    #[test]
    fn filter_env_pattern_projects_modulation_markers_for_direct_and_derived_sources() {
        use crate::mixer::{EffectDevice, EffectKind};

        let mut project = Project::demo();
        let mut flags = LiveFlags::default();
        let mut next_id = project.max_id() + 1;

        // Add an unplaced Rhythm pattern and a double-speed derived TimeScale pattern.
        let base_rhythm_id = next_id;
        next_id += 1;
        project.patterns.push(Pattern {
            id: base_rhythm_id,
            name: "FilterTrigBase".into(),
            kind: PatternKind::Rhythm {
                steps: (0..16).map(|i| i % 4 == 0).collect(),
            },
        });
        let fast_rhythm_id = next_id;
        next_id += 1;
        project.patterns.push(Pattern {
            id: fast_rhythm_id,
            name: "FilterTrigFast".into(),
            kind: PatternKind::TimeScale {
                source: base_rhythm_id,
                num: 2,
                den: 1,
            },
        });

        // Attach channel Filter driven by direct pattern and Master Filter driven by derived pattern.
        let ch_filter_id = next_id;
        next_id += 1;
        let mut ch_filter = EffectDevice::filter(ch_filter_id);
        if let EffectKind::Filter(ref mut fp) = ch_filter.kind {
            fp.env_pattern_id = Some(base_rhythm_id);
        }
        project.mixer.channels[0].inserts.push(ch_filter);

        let master_filter_id = next_id;
        let mut master_filter = EffectDevice::filter(master_filter_id);
        if let EffectKind::Filter(ref mut fp) = master_filter.kind {
            fp.env_pattern_id = Some(fast_rhythm_id);
        }
        project.mixer.master.inserts.push(master_filter);
        project.validate().unwrap();

        // Mute all lanes to verify Filter envelope clock follows song time independently of lanes.
        for lane in &project.lanes {
            flags.set_lane_muted(lane.id, true);
        }

        // Direct pattern is consumed both directly by channel filter (step 3.25) and via 2x TimeScale
        // by master filter (step 6.5).
        let base_res = pattern_playback(
            &project,
            &flags,
            PlaybackRequest {
                pattern: base_rhythm_id,
                scope: PlaybackScope::Definition,
                preferred_placement: None,
                clock: PlaybackClock::Song { step: 3.25 },
            },
        )
        .unwrap();
        assert_eq!(base_res.markers.len(), 2);
        assert!(base_res
            .markers
            .iter()
            .all(|m| m.kind == PlaybackUse::Modulation));
        let ch_marker = base_res
            .markers
            .iter()
            .find(|m| m.effect == Some(ch_filter_id))
            .expect("channel filter marker");
        assert!((ch_marker.position - 3.25).abs() < 1e-6);
        let master_marker = base_res
            .markers
            .iter()
            .find(|m| m.effect == Some(master_filter_id))
            .expect("master filter derived marker");
        assert!((master_marker.position - 6.5).abs() < 1e-6);

        // PastEnd emits zero filter-envelope markers.
        let past_end = pattern_playback(
            &project,
            &flags,
            PlaybackRequest {
                pattern: base_rhythm_id,
                scope: PlaybackScope::Definition,
                preferred_placement: None,
                clock: PlaybackClock::PastEnd,
            },
        )
        .unwrap();
        assert!(past_end.markers.is_empty());
    }
}
