//! Project structural validation, pattern type/period signature checks, and dependency queries.

use super::{
    Boundary, Consumer, Id, InstrumentInstance, LiveFlags, ModulationTarget, PatternKind,
    PatternType, Project, ValueUnit, MAX_BARS, MAX_DEPTH, MAX_DERIVED_OFFSET,
    MAX_GLOBAL_MODULATIONS, MAX_INSTRUMENTS, MAX_LANES, MAX_LINKS, MAX_LITERAL_STEPS, MAX_PARTS,
    MAX_PATTERNS, MAX_PLACEMENTS, MAX_PROJECT_MACROS, MAX_SECTIONS, VERSION,
};
use crate::mixer::{ChannelKind, ChannelRef, EffectKind};
use crate::music::{self, Step, StepCondition};
use crate::params::{InstrumentParam, ParamAddress, ValueMode};
use anyhow::{ensure, Context, Result};
use std::collections::{HashMap, HashSet};

impl Project {
    pub fn validate_macro_target(&self, target: ParamAddress) -> Result<()> {
        if let Some((instrument, param)) = target.as_instrument() {
            let inst = self
                .instrument(instrument)
                .context("Macro target instrument is missing")?;
            return valid_instrument_parameter(inst, param);
        }
        match target {
            ParamAddress::Channel { channel, .. } => {
                ensure!(
                    matches!(channel, ChannelRef::Master)
                        || matches!(channel, ChannelRef::Channel(id) if self.channel(id).is_some()),
                    "Macro target channel is missing"
                );
            }
            ParamAddress::Send { channel, target } => {
                ensure!(
                    matches!(
                        channel,
                        ChannelRef::Channel(id)
                            if self.channel(id).is_some_and(|c| c.kind != ChannelKind::Return)
                    ),
                    "Macro send source channel must be an existing Audio or Group channel"
                );
                ensure!(
                    self.channel(target)
                        .is_some_and(|c| c.kind == ChannelKind::Return),
                    "Macro send target must be an existing Return channel"
                );
            }
            ParamAddress::Effect { device, param } => {
                ensure!(
                    self.device(device)
                        .is_some_and(|(_, d)| d.read_normalized(param).is_some()),
                    "Macro target effect device or parameter is missing"
                );
            }
            ParamAddress::Rack { .. }
            | ParamAddress::Zone { .. }
            | ParamAddress::Module { .. }
            | ParamAddress::Cable { .. } => unreachable!(),
        }
        Ok(())
    }
    pub fn dependencies(&self, id: Id) -> Result<Vec<Id>> {
        Ok(self
            .pattern(id)
            .context("Pattern does not exist")?
            .kind
            .sources())
    }
    pub fn consumers(&self, id: Id) -> Vec<Consumer> {
        let mut consumers = Vec::new();
        for p in &self.patterns {
            if p.kind.sources().contains(&id) {
                consumers.push(Consumer {
                    kind: "pattern".into(),
                    id: p.id,
                    name: p.name.clone(),
                });
            }
        }
        for part in &self.parts {
            if part.pattern == id || part.parameter_links.iter().any(|l| l.pattern == id) {
                consumers.push(Consumer {
                    kind: "part".into(),
                    id: part.id,
                    name: part.name.clone(),
                });
            }
        }
        for m in &self.modulations {
            if m.pattern == id {
                consumers.push(Consumer {
                    kind: "modulation".into(),
                    id: m.id,
                    name: m.name.clone(),
                });
            }
        }
        for dev in self.all_inserts() {
            if let EffectKind::Filter(patch) = dev.kind {
                if patch.env_pattern_id == Some(id) {
                    consumers.push(Consumer {
                        kind: "effect".into(),
                        id: dev.id,
                        name: dev.name.clone(),
                    });
                }
            }
        }
        if matches!(self.timing.launch, Boundary::Cycle { pattern } if pattern == id) {
            consumers.push(Consumer {
                kind: "timing".into(),
                id: 0,
                name: "Launch cycle".into(),
            });
        }
        consumers
    }
    pub fn validate_flags(&self, flags: &LiveFlags) -> Result<()> {
        for &id in flags.muted_lanes.iter().chain(&flags.solo_lanes) {
            ensure!(
                self.lane(id).is_some(),
                "Live flag references missing lane {id}"
            );
        }
        for &id in flags.muted_channels.iter().chain(&flags.solo_channels) {
            ensure!(
                self.channel(id).is_some(),
                "Live flag references missing channel {id}"
            );
        }
        Ok(())
    }
    pub fn prune_flags(&self, flags: &mut LiveFlags) {
        flags.muted_lanes.retain(|id| self.lane(*id).is_some());
        flags.solo_lanes.retain(|id| self.lane(*id).is_some());
        flags
            .muted_channels
            .retain(|id| self.channel(*id).is_some());
        flags.solo_channels.retain(|id| self.channel(*id).is_some());
    }
    pub(super) fn signature(
        &self,
        id: Id,
        depth: usize,
        visiting: &mut HashSet<Id>,
        memo: &mut HashMap<(Id, usize), (PatternType, u64)>,
    ) -> Result<(PatternType, u64)> {
        ensure!(
            depth < MAX_DEPTH,
            "Pattern dependency depth exceeds {MAX_DEPTH}"
        );
        // Depth is part of the key: a shared subtree reached farther down must
        // still respect the root-to-leaf bound. Memoization prevents a compact
        // DAG with shared branches from causing exponential validation work.
        if let Some(result) = memo.get(&(id, depth)) {
            return Ok(*result);
        }
        ensure!(visiting.insert(id), "Pattern dependency cycle at {id}");
        let p = self
            .pattern(id)
            .with_context(|| format!("Missing pattern {id}"))?;
        let result = match &p.kind {
            PatternKind::Notes { steps } => {
                let turns = steps.iter().try_fold(1u64, |turns, step| {
                    lcm(turns, u64::from(step.condition.every))
                })?;
                let period = (steps.len() as u64)
                    .checked_mul(turns)
                    .context("Conditional pattern period exceeds clock range")?;
                (PatternType::Notes, period)
            }
            PatternKind::Rhythm { steps } => (PatternType::Rhythm, steps.len() as u64),
            PatternKind::Values { unit, period, .. }
            | PatternKind::SeededValues { unit, period, .. } => {
                (PatternType::Values(*unit), u64::from(*period))
            }
            PatternKind::Mini { code, steps } => {
                let ast = crate::mini::parse_mini(code)?;
                let cycles = crate::mini::mini_cycle_multiplier(&ast)?;
                let period = u64::from(*steps)
                    .checked_mul(cycles)
                    .context("Mini pattern period exceeds clock range")?;
                (PatternType::Notes, period)
            }
            PatternKind::Euclidean { steps, .. } => (PatternType::Rhythm, u64::from(*steps)),
            PatternKind::AddDegrees { notes, values } => {
                let (a, ap) = self.signature(*notes, depth + 1, visiting, memo)?;
                let (b, bp) = self.signature(*values, depth + 1, visiting, memo)?;
                ensure!(
                    a == PatternType::Notes && b == PatternType::Values(ValueUnit::Degrees),
                    "Degree-add needs Notes and degree Values"
                );
                (PatternType::Notes, lcm(ap, bp)?)
            }
            PatternKind::Mask { notes, rhythm } => {
                let (a, ap) = self.signature(*notes, depth + 1, visiting, memo)?;
                let (b, bp) = self.signature(*rhythm, depth + 1, visiting, memo)?;
                ensure!(
                    a == PatternType::Notes
                        && matches!(b, PatternType::Rhythm | PatternType::Notes),
                    "Mask needs Notes and a Rhythm/Notes source"
                );
                (PatternType::Notes, lcm(ap, bp)?)
            }
            PatternKind::Transpose { source, .. } => {
                let (kind, period) = self.signature(*source, depth + 1, visiting, memo)?;
                ensure!(kind == PatternType::Notes, "Transpose needs Notes");
                (kind, period)
            }
            PatternKind::NotesFromValues { values, rhythm, .. } => {
                let (a, ap) = self.signature(*values, depth + 1, visiting, memo)?;
                let (b, bp) = self.signature(*rhythm, depth + 1, visiting, memo)?;
                ensure!(
                    a == PatternType::Values(ValueUnit::Degrees)
                        && matches!(b, PatternType::Rhythm | PatternType::Notes),
                    "Notes-from-values needs degree Values and Rhythm/Notes"
                );
                (PatternType::Notes, lcm(ap, bp)?)
            }
            PatternKind::TimeScale { source, num, den } => {
                let (kind, period) = self.signature(*source, depth + 1, visiting, memo)?;
                let n = u64::from((*num).max(1));
                let d = u64::from((*den).max(1));
                let scaled = (lcm(period.saturating_mul(d), n)? / n).max(1);
                (kind, scaled)
            }
            PatternKind::Swing { source, .. } => {
                let (kind, period) = self.signature(*source, depth + 1, visiting, memo)?;
                ensure!(kind == PatternType::Notes, "Swing needs Notes");
                (kind, lcm(period, 2)?)
            }
            PatternKind::Ratchet { source, .. }
            | PatternKind::Arp { source, .. }
            | PatternKind::RangeFold { source, .. } => {
                let (kind, period) = self.signature(*source, depth + 1, visiting, memo)?;
                ensure!(kind == PatternType::Notes, "Operation needs Notes");
                (kind, period)
            }
            PatternKind::Shift { source, .. } | PatternKind::Reverse { source } => {
                self.signature(*source, depth + 1, visiting, memo)?
            }
            PatternKind::Chance { source, .. } => {
                let (kind, period) = self.signature(*source, depth + 1, visiting, memo)?;
                ensure!(
                    matches!(kind, PatternType::Notes | PatternType::Rhythm),
                    "Chance needs Notes or Rhythm"
                );
                (kind, period)
            }
            PatternKind::Every {
                interval,
                base,
                alt,
            } => {
                let (ka, pa) = self.signature(*base, depth + 1, visiting, memo)?;
                let (kb, pb) = self.signature(*alt, depth + 1, visiting, memo)?;
                ensure!(ka == kb, "Every requires base and alt of the same type");
                let base_span = pa
                    .checked_mul(u64::from((*interval).max(1)))
                    .context("Every period overflow")?;
                let total = lcm(base_span, pb)?;
                (ka, total)
            }
            PatternKind::Concat { parts } => {
                ensure!(!parts.is_empty(), "Concat needs at least one source");
                let (first_kind, mut total) =
                    self.signature(parts[0], depth + 1, visiting, memo)?;
                for &part in &parts[1..] {
                    let (k, p) = self.signature(part, depth + 1, visiting, memo)?;
                    ensure!(k == first_kind, "Concat sources must share the same type");
                    total = total.checked_add(p).context("Concat period overflow")?;
                }
                (first_kind, total)
            }
            PatternKind::Stack { parts } => {
                ensure!(!parts.is_empty(), "Stack needs at least one source");
                let (first_kind, mut period) =
                    self.signature(parts[0], depth + 1, visiting, memo)?;
                ensure!(
                    matches!(first_kind, PatternType::Notes | PatternType::Rhythm),
                    "Stack needs Notes or Rhythm sources"
                );
                for &part in &parts[1..] {
                    let (k, p) = self.signature(part, depth + 1, visiting, memo)?;
                    ensure!(k == first_kind, "Stack sources must share the same type");
                    period = lcm(period, p)?;
                }
                (first_kind, period)
            }
            PatternKind::Shape {
                source,
                velocity,
                gate,
            } => {
                let (ks, ps) = self.signature(*source, depth + 1, visiting, memo)?;
                let (kv, pv) = self.signature(*velocity, depth + 1, visiting, memo)?;
                ensure!(
                    ks == PatternType::Notes && kv == PatternType::Values(ValueUnit::Normalized),
                    "Shape needs Notes and normalized Values"
                );
                let mut period = lcm(ps, pv)?;
                if let Some(g) = gate {
                    let (kg, pg) = self.signature(*g, depth + 1, visiting, memo)?;
                    ensure!(
                        kg == PatternType::Values(ValueUnit::Normalized),
                        "Shape gate needs normalized Values"
                    );
                    period = lcm(period, pg)?;
                }
                (PatternType::Notes, period)
            }
        };
        visiting.remove(&id);
        ensure!(result.1 > 0, "Pattern periods must be positive");
        memo.insert((id, depth), result);
        Ok(result)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.version == VERSION,
            "Unsupported authoring project version {}; expected {VERSION}",
            self.version
        );
        valid_name(&self.title)?;
        ensure!(
            self.bpm.is_finite() && (20.0..=300.0).contains(&self.bpm),
            "Tempo must be 20..300 BPM"
        );
        ensure!(
            self.root_midi <= 127
                && !self.scale.is_empty()
                && self.scale.len() <= 12
                && self.scale[0] == 0
                && self.scale.iter().all(|v| (0..=11).contains(v))
                && self.scale.windows(2).all(|v| v[0] < v[1]),
            "Invalid musical key/scale"
        );
        ensure!(
            (1..=MAX_BARS).contains(&self.bars),
            "Arrangement must have 1..{MAX_BARS} bars"
        );
        ensure!(
            self.patterns.len() <= MAX_PATTERNS
                && self.instruments.len() <= MAX_INSTRUMENTS
                && self.parts.len() <= MAX_PARTS
                && self.lanes.len() <= MAX_LANES
                && self.sections.len() <= MAX_SECTIONS,
            "Authoring object count exceeds workspace bounds"
        );
        unique(
            self.patterns.iter().map(|p| (p.id, p.name.as_str())),
            "pattern",
        )?;
        unique(
            self.instruments.iter().map(|p| (p.id, p.name.as_str())),
            "instrument",
        )?;
        unique(self.parts.iter().map(|p| (p.id, p.name.as_str())), "part")?;
        unique(self.lanes.iter().map(|p| (p.id, p.name.as_str())), "lane")?;
        unique(
            self.sections.iter().map(|p| (p.id, p.name.as_str())),
            "section",
        )?;
        unique(
            self.modulations.iter().map(|p| (p.id, p.name.as_str())),
            "modulation",
        )?;
        for p in &self.patterns {
            match &p.kind {
                PatternKind::Notes { steps } => {
                    ensure!(
                        !steps.is_empty() && steps.len() <= MAX_LITERAL_STEPS,
                        "Notes must have 1..{MAX_LITERAL_STEPS} steps"
                    );
                    for step in steps {
                        validate_source_step(step)?;
                    }
                }
                PatternKind::Rhythm { steps } => ensure!(
                    !steps.is_empty() && steps.len() <= MAX_LITERAL_STEPS,
                    "Rhythm must have 1..{MAX_LITERAL_STEPS} steps"
                ),
                PatternKind::Values {
                    unit,
                    period,
                    mode,
                    points,
                } => {
                    ensure!(
                        (1..=(MAX_BARS * 16) as u32).contains(period),
                        "Value period exceeds the bounded timeline"
                    );
                    ensure!(
                        !points.is_empty() && points.len() <= *period as usize,
                        "Values require bounded nonempty points"
                    );
                    ensure!(
                        *mode == ValueMode::Hold || *unit == ValueUnit::Normalized,
                        "Degree values are held; normalized values may Hold or Ramp"
                    );
                    let mut previous = None;
                    for point in points {
                        ensure!(
                            point.step < *period && previous.is_none_or(|v| point.step > v),
                            "Value points must be sorted, unique and inside their period"
                        );
                        valid_value(*unit, point.value)?;
                        previous = Some(point.step);
                    }
                }
                PatternKind::SeededValues {
                    unit,
                    period,
                    min,
                    max,
                    ..
                } => {
                    ensure!(
                        (1..=(MAX_BARS * 16) as u32).contains(period),
                        "Seed period exceeds the bounded timeline"
                    );
                    valid_value(*unit, *min)?;
                    valid_value(*unit, *max)?;
                    ensure!(min <= max, "Seeded minimum exceeds maximum");
                }
                PatternKind::Mini { code, steps } => {
                    ensure!(
                        (1..=(MAX_LITERAL_STEPS as u16)).contains(steps),
                        "Mini steps must be 1..={MAX_LITERAL_STEPS}"
                    );
                    crate::mini::parse_mini(code)?;
                }
                PatternKind::Euclidean { pulses, steps, .. } => {
                    ensure!(
                        (1..=(MAX_LITERAL_STEPS as u16)).contains(steps) && pulses <= steps,
                        "Euclidean requires 0 <= pulses <= steps <= {MAX_LITERAL_STEPS}"
                    );
                }
                PatternKind::TimeScale { num, den, .. } => ensure!(
                    (1..=16).contains(num) && (1..=16).contains(den),
                    "TimeScale ratio num/den must be 1..=16"
                ),
                PatternKind::Swing { amount, .. } => ensure!(
                    amount.is_finite() && (0.0..=0.45).contains(amount),
                    "Swing amount must be within 0.0..=0.45"
                ),
                PatternKind::Ratchet { count, .. } => {
                    ensure!((1..=8).contains(count), "Ratchet count must be 1..=8")
                }
                PatternKind::Chance { probability, .. } => ensure!(
                    probability.is_finite() && (0.0..=1.0).contains(probability),
                    "Chance probability must be 0..=1"
                ),
                PatternKind::Every { interval, .. } => {
                    ensure!((2..=64).contains(interval), "Every interval must be 2..=64")
                }
                PatternKind::Arp { rate, .. } => ensure!(
                    (1..=4).contains(rate),
                    "Arp rate must be 1..=4 notes per step"
                ),
                PatternKind::RangeFold {
                    low_midi,
                    high_midi,
                    ..
                } => ensure!(
                    *low_midi <= *high_midi && *high_midi <= 127 && (*high_midi - *low_midi) >= 11,
                    "RangeFold requires at least one octave within 0..=127"
                ),
                PatternKind::Concat { parts } | PatternKind::Stack { parts } => ensure!(
                    !parts.is_empty() && parts.len() <= 16,
                    "Concat/Stack requires 1..=16 parts"
                ),
                PatternKind::Transpose { semitones, .. } => ensure!(
                    (-96..=96).contains(semitones),
                    "Transpose must be within ±96 semitones"
                ),
                PatternKind::NotesFromValues {
                    degree_offset,
                    velocity,
                    gate,
                    ..
                } => {
                    ensure!(
                        (-63..=63).contains(degree_offset),
                        "Degree offset is outside bounds"
                    );
                    validate_source_step(&Step {
                        active: true,
                        degree: 1,
                        velocity: *velocity,
                        gate: *gate,
                        ..Step::default()
                    })?;
                }
                _ => {}
            }
            self.pattern_type(p.id)
                .with_context(|| format!("Pattern {}", p.name))?;
        }
        for instrument in &self.instruments {
            instrument
                .rack
                .validate()
                .with_context(|| format!("Instrument {}", instrument.name))?;
            for (_, zone, _, smp) in instrument.rack.sampler_modules() {
                if let Some(path) = &smp.path {
                    ensure!(
                        !std::path::Path::new(path).is_absolute(),
                        "Instrument {} zone {} sample path must be project-relative or a catalog label, not absolute ({path})",
                        instrument.name,
                        zone.name
                    );
                }
            }
            ensure!(
                self.channel(instrument.channel)
                    .is_some_and(|c| c.kind == ChannelKind::Audio),
                "Instrument {} must output to an existing Audio channel",
                instrument.name
            );
        }
        for part in &self.parts {
            ensure!(
                self.pattern_type(part.pattern)? == PatternType::Notes,
                "Part {} needs a Notes pattern",
                part.name
            );
            let instrument = self
                .instrument(part.instrument)
                .with_context(|| format!("Part {} has a missing instrument", part.name))?;
            ensure!(
                (-96..=96).contains(&part.transpose)
                    && part.velocity_scale.is_finite()
                    && (0.0..=1.0).contains(&part.velocity_scale),
                "Part transpose/velocity is out of range"
            );
            ensure!(
                part.parameter_links.len() <= MAX_LINKS,
                "Too many per-note control links"
            );
            let mut targets = HashSet::new();
            for link in &part.parameter_links {
                ensure!(
                    targets.insert(link.target),
                    "Duplicate per-note parameter link"
                );
                ensure!(
                    self.pattern_type(link.pattern)? == PatternType::Values(ValueUnit::Normalized),
                    "Per-note controls need normalized Values"
                );
                valid_instrument_parameter(instrument, link.target)?;
            }
        }
        let mut placement_ids = HashSet::new();
        let mut occupied: HashMap<Id, Vec<(u32, u32, Id)>> = HashMap::new();
        for lane in &self.lanes {
            let mut ranges = Vec::new();
            for place in &lane.placements {
                ensure!(
                    placement_ids.insert(place.id),
                    "Duplicate placement ID {}",
                    place.id
                );
                ensure!(placement_ids.len() <= MAX_PLACEMENTS, "Too many placements");
                let end = place.end_step().context("Placement range overflow")?;
                ensure!(
                    place.length_steps > 0 && end <= self.arrangement_steps() as u32,
                    "Placement must fit the arrangement"
                );
                ensure!(
                    place.offset_steps.unsigned_abs() <= MAX_DERIVED_OFFSET,
                    "Phase offset exceeds bounds"
                );
                let part = self
                    .part(place.part)
                    .with_context(|| format!("Placement {} has a missing part", place.id))?;
                ranges.push((place.start_step, end, place.id));
                occupied.entry(part.instrument).or_default().push((
                    place.start_step,
                    end,
                    place.id,
                ));
                let mut seen_exceptions = HashSet::new();
                for local in &place.exceptions {
                    ensure!(
                        local.step < place.length_steps && seen_exceptions.insert(local.step),
                        "Placement exceptions must have unique relative positions inside their placement"
                    );
                    validate_source_step(&local.value)?;
                    // An exception addresses one occurrence, not another
                    // repeating source. Conditional repetition belongs to the
                    // reusable pattern that supplies the occurrence.
                    ensure!(
                        local.value.condition == StepCondition::ALWAYS,
                        "Placement-local exceptions must be unconditional; put repeating conditions on the source pattern"
                    );
                }
            }
            nonoverlap(
                &mut ranges,
                &format!("Lane {} has overlapping placements", lane.name),
            )?;
        }
        for (instrument, mut ranges) in occupied {
            nonoverlap(&mut ranges,&format!("Instrument {instrument} is shared by overlapping placements; create a separate instance for simultaneous parts"))?;
        }
        for section in &self.sections {
            ensure!(
                section.length_steps > 0
                    && section
                        .start_step
                        .checked_add(section.length_steps)
                        .is_some_and(|end| end <= self.arrangement_steps() as u32),
                "Section range exceeds the arrangement"
            );
        }
        ensure!(
            self.modulations.len() <= MAX_GLOBAL_MODULATIONS,
            "Too many global control relationships"
        );
        let mut global_targets = HashSet::new();
        for modulation in &self.modulations {
            ensure!(
                global_targets.insert(modulation.target),
                "A parameter has multiple global sources; compose its source explicitly"
            );
            ensure!(
                self.pattern_type(modulation.pattern)?
                    == PatternType::Values(ValueUnit::Normalized),
                "Global controls need normalized Values"
            );
            match modulation.target {
                ModulationTarget::Instrument {
                    instrument,
                    parameter,
                } => valid_instrument_parameter(
                    self.instrument(instrument)
                        .context("Modulated instrument is missing")?,
                    parameter,
                )?,
                ModulationTarget::Effect { device, parameter } => ensure!(
                    self.device(device)
                        .is_some_and(|(_, d)| d.read_normalized(parameter).is_some()),
                    "Effect parameter target is missing or unsupported"
                ),
                ModulationTarget::ChannelGain { channel } => ensure!(
                    matches!(channel, ChannelRef::Master)
                        || matches!(channel, ChannelRef::Channel(id) if self.channel(id).is_some()),
                    "Level-shape target is missing"
                ),
                ModulationTarget::ChannelPan { channel } => ensure!(
                    matches!(channel, ChannelRef::Master)
                        || matches!(channel, ChannelRef::Channel(id) if self.channel(id).is_some()),
                    "Pan-automation target is missing"
                ),
                ModulationTarget::Send { channel, target } => {
                    ensure!(
                        matches!(
                            channel,
                            ChannelRef::Channel(id)
                                if self.channel(id).is_some_and(|c| c.kind != ChannelKind::Return)
                        ),
                        "Send modulation source must be an existing Audio or Group channel"
                    );
                    ensure!(
                        self.channel(target)
                            .is_some_and(|c| c.kind == ChannelKind::Return),
                        "Send modulation target must be an existing Return channel"
                    );
                }
                ModulationTarget::Macro { index } => ensure!(
                    usize::from(index) < MAX_PROJECT_MACROS && self.macro_control(index).is_some(),
                    "Modulated Macro {} does not exist",
                    usize::from(index) + 1
                ),
            }
        }
        if !self.macros.is_empty() {
            ensure!(
                self.macros.len() <= MAX_PROJECT_MACROS,
                "At most {MAX_PROJECT_MACROS} project macros are supported"
            );
            let mut macro_indices = HashSet::with_capacity(self.macros.len());
            for mac in &self.macros {
                mac.validate_self()?;
                ensure!(
                    macro_indices.insert(mac.index),
                    "Duplicate project macro index {}",
                    mac.index + 1
                );
                for binding in &mac.bindings {
                    self.validate_macro_target(binding.target)?;
                }
            }
        }
        for duck in &self.ducks {
            ensure!(
                self.part(duck.source_part).is_some(),
                "Duck source part is missing"
            );
        }
        for dev in self.all_inserts() {
            if let EffectKind::Filter(patch) = dev.kind {
                if let Some(pat_id) = patch.env_pattern_id {
                    let ptype = self
                        .pattern_type(pat_id)
                        .with_context(|| format!("Filter envelope pattern {pat_id} is missing"))?;
                    ensure!(
                        !matches!(ptype, PatternType::Values(ValueUnit::Degrees)),
                        "Filter envelope pattern {pat_id} cannot use degree Values; use Notes, Rhythm, or normalized Values"
                    );
                }
            }
        }
        self.validate_live_setup()?;
        let mixer = self.performance_mixer(&LiveFlags::default());
        mixer.validate(self.arrangement_steps() as u32)?;
        Ok(())
    }
}

fn valid_name(name: &str) -> Result<()> {
    ensure!(
        !name.trim().is_empty()
            && name.chars().count() <= 120
            && !name.chars().any(char::is_control),
        "Names require 1..120 printable characters"
    );
    Ok(())
}
fn unique<'a>(items: impl Iterator<Item = (Id, &'a str)>, kind: &str) -> Result<()> {
    let mut ids = HashSet::new();
    let mut names = HashSet::new();
    for (id, name) in items {
        valid_name(name)?;
        ensure!(ids.insert(id), "Duplicate {kind} ID {id}");
        ensure!(names.insert(name), "Duplicate {kind} name {name}");
    }
    Ok(())
}
fn validate_source_step(step: &Step) -> Result<()> {
    music::validate_step(step)?;
    ensure!(
        step.source_part.is_none(),
        "Source notes cannot contain compiled part provenance"
    );
    Ok(())
}

fn valid_value(unit: ValueUnit, value: f32) -> Result<()> {
    ensure!(value.is_finite(), "Value must be finite");
    match unit {
        ValueUnit::Degrees => ensure!(
            value.fract() == 0.0 && (-63.0..=63.0).contains(&value),
            "Degree values must be integral and within ±63"
        ),
        ValueUnit::Normalized => ensure!(
            (0.0..=1.0).contains(&value),
            "Normalized values must be within 0..1"
        ),
    };
    Ok(())
}
impl InstrumentInstance {
    /// Validate that `target` is supported and automatable on this instrument.
    pub fn validate_parameter(&self, target: InstrumentParam) -> Result<()> {
        valid_instrument_parameter(self, target)
    }
}

fn valid_instrument_parameter(
    instrument: &InstrumentInstance,
    target: InstrumentParam,
) -> Result<()> {
    instrument
        .rack
        .validate_automatable_param(target)
        .with_context(|| format!("Instrument {}", instrument.name))
}
fn nonoverlap(ranges: &mut [(u32, u32, Id)], message: &str) -> Result<()> {
    ranges.sort_by_key(|r| (r.0, r.1, r.2));
    ensure!(
        ranges.windows(2).all(|pair| pair[0].1 <= pair[1].0),
        "{message}"
    );
    Ok(())
}
fn lcm(a: u64, b: u64) -> Result<u64> {
    ensure!(a > 0 && b > 0, "Pattern periods must be positive");
    let (mut x, mut y) = (a, b);
    while y != 0 {
        let r = x % y;
        x = y;
        y = r;
    }
    (a / x)
        .checked_mul(b)
        .context("Combined pattern period exceeds clock range")
}
