//! Project-to-`ScoreWindow` compilation, static/automated Macro resolution, and Filter patterns.

use super::{
    EventOrigin, Id, LiveFlags, MacroControl, Modulation, ModulationTarget, Part, PatternKind,
    PatternType, PatternValue, Project, ValueUnit, STEPS_PER_BAR,
};
use crate::filter_patterns::{CompiledFilterPattern, MAX_FILTER_ENV_STEPS};
use crate::mixer::{
    ChannelParam, ChannelRef, EffectKind, Mixer, MixerAutomation, MixerBreakpoint, MixerParamTarget,
};
use crate::music::{self, EndRule, Step, StepCondition};
use crate::params::{ParamAddress, ValueMode};
use crate::score::{ModLane, ParamLock, ScoreEvent, ScoreInstrument, ScoreWindow};
use anyhow::{Context, Result};
use std::collections::HashSet;

impl Project {
    /// Compile any authored project `Pattern` (`Notes`, `Rhythm`, `Euclidean`, `Mini`,
    /// normalized `Values`, etc.) into a fixed-size real-time trigger grid for a `Filter` envelope.
    pub fn compile_filter_pattern(&self, pattern_id: Id) -> Option<CompiledFilterPattern> {
        self.pattern(pattern_id)?;
        let ptype = self.pattern_type(pattern_id).ok()?;
        let period = self
            .pattern_period(pattern_id)
            .ok()
            .unwrap_or(16)
            .clamp(1, MAX_FILTER_ENV_STEPS as u64) as usize;
        let mut steps = [0u8; MAX_FILTER_ENV_STEPS];
        match ptype {
            PatternType::Notes => {
                let mut per_step: Vec<Vec<(f32, f32)>> = Vec::with_capacity(period);
                let mut has_substep = false;
                let mut eval_buf = Vec::with_capacity(4);
                for t in 0..period {
                    eval_buf.clear();
                    if crate::evaluate::evaluate_notes_into(
                        self,
                        pattern_id,
                        t as i64,
                        &mut eval_buf,
                    )
                    .is_ok()
                    {
                        let mut hits = Vec::new();
                        for ev in &eval_buf {
                            if ev.step.active {
                                if ev.offset >= 0.125 {
                                    has_substep = true;
                                }
                                hits.push((ev.offset, ev.step.velocity));
                            }
                        }
                        per_step.push(hits);
                    } else {
                        per_step.push(Vec::new());
                    }
                }
                if has_substep && period <= MAX_FILTER_ENV_STEPS / 2 {
                    let len = (period * 2).min(MAX_FILTER_ENV_STEPS);
                    for (t, hits) in per_step.into_iter().enumerate() {
                        for (offset, vel) in hits {
                            let sub = usize::from(offset >= 0.125);
                            let idx = (t * 2 + sub).min(len - 1);
                            let v = (vel.clamp(0.0, 1.0) * 127.0).round().clamp(1.0, 127.0) as u8;
                            steps[idx] = steps[idx].max(v);
                        }
                    }
                    Some(CompiledFilterPattern {
                        resolution: 32,
                        length: len.max(1) as u8,
                        smooth_attack: false,
                        steps,
                    })
                } else {
                    for (t, hits) in per_step.into_iter().enumerate() {
                        let max_vel = hits.into_iter().map(|(_, v)| v).fold(0.0_f32, f32::max);
                        if max_vel > 0.0 {
                            steps[t] =
                                (max_vel.clamp(0.0, 1.0) * 127.0).round().clamp(1.0, 127.0) as u8;
                        }
                    }
                    Some(CompiledFilterPattern {
                        resolution: 16,
                        length: period.max(1) as u8,
                        smooth_attack: false,
                        steps,
                    })
                }
            }
            PatternType::Rhythm => {
                for (t, slot) in steps[..period].iter_mut().enumerate() {
                    if matches!(
                        self.evaluate_pattern(pattern_id, t as i64),
                        Ok(PatternValue::Rhythm(true))
                    ) {
                        *slot = 127;
                    }
                }
                Some(CompiledFilterPattern {
                    resolution: 16,
                    length: period.max(1) as u8,
                    smooth_attack: false,
                    steps,
                })
            }
            PatternType::Values(ValueUnit::Degrees) => None,
            PatternType::Values(ValueUnit::Normalized) => {
                let smooth_attack = self.value_mode(pattern_id).ok() == Some(ValueMode::Ramp);
                // Preserve exact 1/8 (`TimeScale 1/2`) and 1/32 (`TimeScale 2/1`) resolutions
                // on normalized `Values` so forked multi-resolution filter grooves round-trip identically.
                let (eval_id, resolution, eval_len) = match &self.pattern(pattern_id)?.kind {
                    PatternKind::TimeScale { source, num, den }
                        if u32::from(*num) == u32::from(*den) * 2 =>
                    {
                        let src_len = self
                            .pattern_period(*source)
                            .ok()
                            .unwrap_or(16)
                            .clamp(1, MAX_FILTER_ENV_STEPS as u64)
                            as usize;
                        (*source, 32u8, src_len)
                    }
                    PatternKind::TimeScale { source, num, den }
                        if u32::from(*den) == u32::from(*num) * 2 =>
                    {
                        let src_len = self
                            .pattern_period(*source)
                            .ok()
                            .unwrap_or(16)
                            .clamp(1, MAX_FILTER_ENV_STEPS as u64)
                            as usize;
                        (*source, 8u8, src_len)
                    }
                    _ => (pattern_id, 16u8, period),
                };
                for (t, slot) in steps[..eval_len].iter_mut().enumerate() {
                    if let Ok(v) = self.value_inner(eval_id, t as f64) {
                        let c = v.clamp(0.0, 1.0);
                        if c > 0.005 {
                            *slot = (c * 127.0).round().clamp(1.0, 127.0) as u8;
                        }
                    }
                }
                Some(CompiledFilterPattern {
                    resolution,
                    length: eval_len.max(1) as u8,
                    smooth_attack,
                    steps,
                })
            }
        }
    }

    pub(super) fn performance_mixer(&self, flags: &LiveFlags) -> Mixer {
        let mut pruned = flags.clone();
        self.prune_flags(&mut pruned);
        let mut filter_env_triggers = Vec::new();
        for dev in self.all_inserts() {
            if let EffectKind::Filter(patch) = dev.kind {
                if let Some(pat_id) = patch.env_pattern_id {
                    if let Some(compiled) = self.compile_filter_pattern(pat_id) {
                        filter_env_triggers.push((dev.id, compiled));
                    }
                }
            }
        }
        Mixer {
            master: self.mixer.master.clone(),
            channels: self.mixer.channels.clone(),
            automations: Vec::new(),
            ducks: self.ducks.clone(),
            flags: pruned,
            filter_env_triggers,
            filter_grooves: crate::filter_patterns::current_compiled_filter_grooves(),
        }
    }

    pub fn compile(&self) -> Result<ScoreWindow> {
        self.compile_with_flags(&LiveFlags::default())
    }
    pub fn compile_with_flags(&self, flags: &LiveFlags) -> Result<ScoreWindow> {
        self.validate()?;
        self.compile_validated(flags)
    }
    pub(super) fn compile_validated(&self, flags: &LiveFlags) -> Result<ScoreWindow> {
        let mut out = self.score_builder(self.bars, flags)?;
        let mut eval_buf = Vec::with_capacity(4);
        for lane in &self.lanes {
            if !flags.lane_enabled(lane.id) {
                continue;
            }
            for place in &lane.placements {
                let part = self.part(place.part).expect("validated part");
                for local_step in 0..place.length_steps {
                    let song_step = place.start_step + local_step;
                    let pattern_time = place.pattern_time(song_step);
                    let exception = place.exceptions.iter().find(|o| o.step == local_step);
                    if let Some(ex) = exception {
                        let mut step = ex.value.clone();
                        let eligible = step.active;
                        if !eligible {
                            continue;
                        }
                        self.interpret(part, &mut step);
                        out.write_event(
                            self,
                            EventWrite {
                                at: song_step as usize,
                                offset: 0.0,
                                part,
                                step,
                                origin: EventOrigin {
                                    song_step,
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
                    } else {
                        eval_buf.clear();
                        crate::evaluate::evaluate_notes_into(
                            self,
                            part.pattern,
                            pattern_time,
                            &mut eval_buf,
                        )?;
                        for mut ev in eval_buf.drain(..) {
                            if !ev.step.active {
                                continue;
                            }
                            self.interpret(part, &mut ev.step);
                            out.write_event(
                                self,
                                EventWrite {
                                    at: song_step as usize,
                                    offset: ev.offset,
                                    part,
                                    step: ev.step,
                                    origin: EventOrigin {
                                        song_step,
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
                    }
                }
            }
        }
        out.write_modulations(self, |pattern, at| self.value_inner(pattern, at as f64))?;
        out.finish()
    }

    /// Build a native `ScoreWindow` builder sized to `bars`.
    pub(super) fn score_builder(&self, bars: usize, flags: &LiveFlags) -> Result<ScoreBuilder> {
        let length = bars * STEPS_PER_BAR;
        let mut mixer = self.performance_mixer(flags);
        let mut mod_lanes = Vec::new();
        let instruments = build_score_instruments(self, &mut mixer, &mut mod_lanes, length)?;
        Ok(ScoreBuilder {
            title: self.title.clone(),
            bpm: self.bpm,
            root_midi: self.root_midi,
            scale: self.scale.clone(),
            bars,
            end_rule: self.end,
            live_window: None,
            instruments,
            mixer,
            buckets: vec![Vec::new(); length],
            step_owners: vec![Vec::new(); length],
            locks: Vec::new(),
            mod_lanes,
            provenance: Vec::new(),
            required: HashSet::new(),
            length,
        })
    }

    /// Fast sound-only patch update on an existing `ScoreWindow` (skips pattern evaluation!).
    pub fn patch_score_sound(&self, base: &ScoreWindow, flags: &LiveFlags) -> Result<ScoreWindow> {
        let mut mixer = self.performance_mixer(flags);
        let mut mod_lanes = base.mod_lanes.clone();
        let instruments =
            build_score_instruments(self, &mut mixer, &mut mod_lanes, base.arrangement_steps())?;
        mixer.automations = base.mixer.automations.clone();
        // Re-synthesize missing ChannelSend entries for any automated MixerParamTarget::Send
        // (matching ScoreBuilder::write_modulations so automated Macro sends survive fast patches).
        for auto in &mixer.automations {
            if let MixerParamTarget::Send { channel, target } = auto.target {
                upsert_channel_send(
                    &mut mixer.channels,
                    channel,
                    target,
                    auto.points.first().map_or(0.0, |p| p.value),
                    false,
                );
            }
        }
        let mut next = base.with_sound_updates(self.bpm, instruments, mixer);
        next.mod_lanes = mod_lanes;
        Ok(next)
    }

    /// A part's interpretation of its source: transpose and velocity scale.
    pub(super) fn interpret(&self, part: &Part, value: &mut Step) {
        self.transpose_step(value, part.transpose);
        value.velocity = (value.velocity * part.velocity_scale).clamp(0.0, 1.0);
        for tone in &mut value.chord {
            tone.velocity = (tone.velocity * part.velocity_scale).clamp(0.0, 1.0);
        }
    }
}

/// Native `ScoreWindow` builder shared by song compilation and live sliding rings.
pub(super) struct ScoreBuilder {
    title: String,
    bpm: f64,
    root_midi: u8,
    scale: Vec<i8>,
    bars: usize,
    end_rule: EndRule,
    pub(super) live_window: Option<crate::score::LiveWindow>,
    instruments: Vec<ScoreInstrument>,
    mixer: Mixer,
    buckets: Vec<Vec<ScoreEvent>>,
    /// Per-step `(inst_idx, lane)` owners so one lane can emit multiple sub-step
    /// events per instrument per step while earlier lanes still win cross-lane ties.
    step_owners: Vec<Vec<(u8, Id)>>,
    locks: Vec<ParamLock>,
    mod_lanes: Vec<ModLane>,
    provenance: Vec<EventOrigin>,
    required: HashSet<Id>,
    length: usize,
}

pub(super) struct EventWrite<'a> {
    pub(super) at: usize,
    pub(super) offset: f32,
    pub(super) part: &'a Part,
    pub(super) step: Step,
    pub(super) origin: EventOrigin,
}

impl ScoreBuilder {
    pub(super) fn instrument_index(&self, instrument: Id) -> Option<usize> {
        self.instruments.iter().position(|i| i.id == instrument)
    }

    fn pitch_midi(&self, degree: i16, accidental: i8) -> u8 {
        music::pitch_midi(self.root_midi, &self.scale, degree, accidental, 0)
    }

    /// Writes one interpreted sub-step event at timeline slot `at` with fractional
    /// `offset ∈ [0.0, 1.0)`. Multiple sub-step events from the same lane are preserved;
    /// a different lane colliding on the same instrument at the same step is ignored.
    pub(super) fn write_event(&mut self, song: &Project, write: EventWrite<'_>) -> Result<bool> {
        let EventWrite {
            at,
            offset,
            part,
            mut step,
            origin,
        } = write;
        let index = self
            .instrument_index(part.instrument)
            .context("Part instrument is missing from the score")?;
        let inst_idx = index as u8;
        if let Some((_, owner_lane)) = self.step_owners[at]
            .iter()
            .find(|(idx, _)| *idx == inst_idx)
        {
            if *owner_lane != origin.lane {
                return Ok(false);
            }
        } else {
            self.step_owners[at].push((inst_idx, origin.lane));
        }
        step.active = true;
        step.condition = StepCondition::ALWAYS;
        music::validate_step(&step).with_context(|| {
            format!(
                "Resolved pattern {} at step {}",
                part.pattern, origin.song_step
            )
        })?;
        let clamped_offset = offset.clamp(0.0, 0.9999);
        let lock_start = self.locks.len() as u32;
        for link in &part.parameter_links {
            let value = song.value_inner(
                link.pattern,
                origin.pattern_time as f64 + f64::from(clamped_offset),
            )?;
            self.locks.push(ParamLock {
                target: link.target,
                value,
            });
        }
        let lock_len = (self.locks.len() as u32 - lock_start) as u16;
        let midi = self.pitch_midi(step.degree, step.accidental);
        let mut chord = [crate::score::ResolvedChordTone::default(); crate::music::MAX_CHORD_TONES];
        let chord_len = step.chord.len().min(crate::music::MAX_CHORD_TONES);
        for (i, tone) in step.chord.iter().take(chord_len).enumerate() {
            chord[i] = crate::score::ResolvedChordTone {
                midi: self.pitch_midi(tone.degree, tone.accidental),
                velocity: tone.velocity,
                gate: tone.gate,
            };
        }
        self.buckets[at].push(ScoreEvent {
            inst_idx,
            part_id: (part.id != 0).then_some(part.id),
            offset: clamped_offset,
            degree: step.degree,
            accidental: step.accidental,
            midi,
            velocity: step.velocity,
            gate: step.gate,
            chord,
            chord_len: chord_len as u8,
            lock_start,
            lock_len,
        });
        self.required.insert(part.instrument);
        self.provenance.push(origin);
        Ok(true)
    }

    /// Samples every global control relationship of `statics` once per step with
    /// explicit deterministic precedence: automated Macros (ascending index `0..8`)
    /// apply first, then direct Modulations always override Macro fan-out targets
    /// independent of `statics.modulations` ordering.
    pub(super) fn write_modulations(
        &mut self,
        statics: &Project,
        mut value_at: impl FnMut(Id, usize) -> Result<f32>,
    ) -> Result<()> {
        let mut macro_mods: Vec<(u8, &Modulation)> = statics
            .modulations
            .iter()
            .filter_map(|m| match m.target {
                ModulationTarget::Macro { index } => Some((index, m)),
                _ => None,
            })
            .collect();
        macro_mods.sort_by_key(|(idx, _)| *idx);
        let direct_mods: Vec<&Modulation> = statics
            .modulations
            .iter()
            .filter(|m| !matches!(m.target, ModulationTarget::Macro { .. }))
            .collect();

        for modulation in macro_mods.into_iter().map(|(_, m)| m).chain(direct_mods) {
            let mode = statics.value_mode(modulation.pattern)?;
            let values = (0..self.length)
                .map(|at| value_at(modulation.pattern, at))
                .collect::<Result<Vec<f32>>>()?;
            let mixer_points = || {
                values
                    .iter()
                    .enumerate()
                    .map(|(at, &value)| MixerBreakpoint {
                        step: at as u32,
                        value,
                    })
                    .collect()
            };
            match modulation.target {
                ModulationTarget::Instrument {
                    instrument,
                    parameter,
                } => {
                    let index = self
                        .instrument_index(instrument)
                        .context("Modulated instrument is missing from the score")?;
                    upsert_mod_lane(
                        &mut self.mod_lanes,
                        ModLane {
                            inst_idx: index as u8,
                            target: parameter,
                            mode,
                            values,
                        },
                    );
                }
                ModulationTarget::Effect { device, parameter } => upsert_mixer_automation(
                    &mut self.mixer.automations,
                    MixerAutomation {
                        target: MixerParamTarget::Device {
                            device_id: device,
                            param: parameter,
                        },
                        mode,
                        points: mixer_points(),
                    },
                ),
                ModulationTarget::ChannelGain { channel } => upsert_mixer_automation(
                    &mut self.mixer.automations,
                    MixerAutomation {
                        target: MixerParamTarget::ChannelGainScale { channel },
                        mode,
                        points: mixer_points(),
                    },
                ),
                ModulationTarget::ChannelPan { channel } => upsert_mixer_automation(
                    &mut self.mixer.automations,
                    MixerAutomation {
                        target: MixerParamTarget::Channel {
                            channel,
                            param: ChannelParam::Pan,
                        },
                        mode,
                        points: mixer_points(),
                    },
                ),
                ModulationTarget::Send { channel, target } => {
                    if matches!(channel, ChannelRef::Channel(_)) {
                        upsert_channel_send(
                            &mut self.mixer.channels,
                            channel,
                            target,
                            values.first().copied().unwrap_or(0.0),
                            false,
                        );
                        upsert_mixer_automation(
                            &mut self.mixer.automations,
                            MixerAutomation {
                                target: MixerParamTarget::Send { channel, target },
                                mode,
                                points: mixer_points(),
                            },
                        );
                    }
                }
                ModulationTarget::Macro { index } => {
                    if let Some(mac) = statics.macro_control(index) {
                        for binding in &mac.bindings {
                            let mapped: Vec<f32> =
                                values.iter().map(|&v| binding.evaluate(v)).collect();
                            if let Some((instrument, param)) = binding.target.as_instrument() {
                                if param.is_automatable() {
                                    if let Some(inst_idx) = self.instrument_index(instrument) {
                                        upsert_mod_lane(
                                            &mut self.mod_lanes,
                                            ModLane {
                                                inst_idx: inst_idx as u8,
                                                target: param,
                                                mode,
                                                values: mapped,
                                            },
                                        );
                                    }
                                }
                                continue;
                            }
                            match binding.target {
                                ParamAddress::Effect { device, param } => {
                                    let points = mapped
                                        .iter()
                                        .enumerate()
                                        .map(|(at, &value)| MixerBreakpoint {
                                            step: at as u32,
                                            value,
                                        })
                                        .collect();
                                    upsert_mixer_automation(
                                        &mut self.mixer.automations,
                                        MixerAutomation {
                                            target: MixerParamTarget::Device {
                                                device_id: device,
                                                param,
                                            },
                                            mode,
                                            points,
                                        },
                                    );
                                }
                                ParamAddress::Channel { channel, param } => {
                                    let points = mapped
                                        .iter()
                                        .enumerate()
                                        .map(|(at, &value)| MixerBreakpoint {
                                            step: at as u32,
                                            value,
                                        })
                                        .collect();
                                    upsert_mixer_automation(
                                        &mut self.mixer.automations,
                                        MixerAutomation {
                                            target: MixerParamTarget::Channel { channel, param },
                                            mode,
                                            points,
                                        },
                                    );
                                }
                                ParamAddress::Send { channel, target } => {
                                    if matches!(channel, ChannelRef::Channel(_)) {
                                        upsert_channel_send(
                                            &mut self.mixer.channels,
                                            channel,
                                            target,
                                            mapped.first().copied().unwrap_or(0.0),
                                            false,
                                        );
                                        let points = mapped
                                            .iter()
                                            .enumerate()
                                            .map(|(at, &value)| MixerBreakpoint {
                                                step: at as u32,
                                                value,
                                            })
                                            .collect();
                                        upsert_mixer_automation(
                                            &mut self.mixer.automations,
                                            MixerAutomation {
                                                target: MixerParamTarget::Send { channel, target },
                                                mode,
                                                points,
                                            },
                                        );
                                    }
                                }
                                ParamAddress::Rack { .. }
                                | ParamAddress::Zone { .. }
                                | ParamAddress::Module { .. }
                                | ParamAddress::Cable { .. } => unreachable!(),
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) fn finish(mut self) -> Result<ScoreWindow> {
        let mut events = Vec::new();
        let mut step_ranges = Vec::with_capacity(self.length);
        for mut bucket in self.buckets {
            bucket.sort_by(|a, b| {
                a.offset
                    .total_cmp(&b.offset)
                    .then_with(|| a.inst_idx.cmp(&b.inst_idx))
            });
            let start = events.len() as u32;
            events.extend(bucket);
            let end = events.len() as u32;
            step_ranges.push((start, end));
        }
        let mut required_instrument_ids: Vec<_> = self.required.into_iter().collect();
        required_instrument_ids.sort_unstable();
        self.provenance
            .sort_by_key(|p| (p.song_step, p.instrument, p.placement));
        let score = ScoreWindow {
            title: self.title,
            bpm: self.bpm,
            root_midi: self.root_midi,
            scale: self.scale,
            bars: self.bars,
            end_rule: self.end_rule,
            live_window: self.live_window,
            instruments: self.instruments,
            mixer: self.mixer,
            events,
            step_ranges,
            locks: self.locks,
            mod_lanes: self.mod_lanes,
            required_instrument_ids,
            provenance: self.provenance,
        };
        score.validate()?;
        Ok(score)
    }
}

fn build_score_instruments(
    project: &Project,
    mixer: &mut Mixer,
    mod_lanes: &mut Vec<ModLane>,
    timeline_steps: usize,
) -> Result<Vec<ScoreInstrument>> {
    let mut instruments: Vec<ScoreInstrument> = project
        .instruments
        .iter()
        .map(|inst| ScoreInstrument {
            id: inst.id,
            name: inst.name.clone(),
            rack: inst.rack.clone(),
            compiled_zones: Vec::new(),
            channel: inst.channel,
        })
        .collect();
    if !project.macros.is_empty() {
        apply_static_macros(
            &project.macros,
            &project.modulations,
            &mut instruments,
            mixer,
            mod_lanes,
            timeline_steps,
        );
    }
    for inst in &mut instruments {
        // Compile zone DAGs AFTER static macros mutate `inst.rack` so `CompiledCable.amount`
        // reflects any static macro overrides on patch cables.
        inst.compile_zones()?;
    }
    Ok(instruments)
}

fn target_has_automation(
    macros: &[MacroControl],
    modulations: &[Modulation],
    target: ParamAddress,
) -> bool {
    if let Some((instrument, parameter)) = target.as_instrument() {
        if modulations.iter().any(|m| {
            m.target
                == ModulationTarget::Instrument {
                    instrument,
                    parameter,
                }
        }) {
            return true;
        }
    }
    macros.iter().any(|mac| {
        modulations
            .iter()
            .any(|m| m.target == ModulationTarget::Macro { index: mac.index })
            && mac.bindings.iter().any(|b| b.target == target)
    })
}

fn apply_static_macros(
    macros: &[MacroControl],
    modulations: &[Modulation],
    instruments: &mut [ScoreInstrument],
    mixer: &mut Mixer,
    mod_lanes: &mut Vec<ModLane>,
    timeline_steps: usize,
) {
    let mut ordered: Vec<&MacroControl> = macros.iter().collect();
    ordered.sort_by_key(|m| m.index);
    for mac in ordered {
        if modulations
            .iter()
            .any(|m| m.target == ModulationTarget::Macro { index: mac.index })
        {
            continue;
        }
        for binding in &mac.bindings {
            let norm = binding.evaluate(mac.value);
            if let Some((instrument, param)) = binding.target.as_instrument() {
                if param.is_frame_based() {
                    // Frame-based Sampler controls must resolve against the loaded
                    // sample's real `sample.frames` on the audio engine, never `0`.
                    if !target_has_automation(macros, modulations, binding.target) {
                        if let Some(inst_idx) = instruments.iter().position(|i| i.id == instrument)
                        {
                            upsert_mod_lane(
                                mod_lanes,
                                ModLane {
                                    inst_idx: inst_idx as u8,
                                    target: param,
                                    mode: ValueMode::Hold,
                                    values: vec![norm.clamp(0.0, 1.0); timeline_steps.max(1)],
                                },
                            );
                        }
                    }
                } else if let Some(inst) = instruments.iter_mut().find(|i| i.id == instrument) {
                    inst.rack.write_param_normalized(param, norm, 0);
                }
                continue;
            }
            match binding.target {
                ParamAddress::Channel { channel, param } => {
                    let strip = match channel {
                        ChannelRef::Master => Some(&mut mixer.master.strip),
                        ChannelRef::Channel(id) => mixer
                            .channels
                            .iter_mut()
                            .find(|c| c.id == id)
                            .map(|c| &mut c.strip),
                    };
                    if let Some(strip) = strip {
                        match param {
                            ChannelParam::Gain => strip.gain = param.denormalize(norm),
                            ChannelParam::Pan => strip.pan = param.denormalize(norm),
                        }
                    }
                }
                ParamAddress::Send { channel, target } => {
                    upsert_channel_send(&mut mixer.channels, channel, target, norm, true);
                }
                ParamAddress::Effect { device, param } => {
                    if let Some(dev) = mixer.all_inserts_mut().find(|d| d.id == device) {
                        let _ = dev.write_normalized(param, norm);
                    }
                }
                ParamAddress::Rack { .. }
                | ParamAddress::Zone { .. }
                | ParamAddress::Module { .. }
                | ParamAddress::Cable { .. } => unreachable!(),
            }
        }
    }
}

/// Ensure a channel carries a [`ChannelSend`] for `target`, optionally overwriting an existing
/// send's static level (used by static Macro overrides, automated Send lanes, and fast patches).
fn upsert_channel_send(
    channels: &mut [crate::mixer::Channel],
    channel: ChannelRef,
    target: u64,
    amount: f32,
    overwrite_existing: bool,
) {
    let ChannelRef::Channel(id) = channel else {
        return;
    };
    let clamped = amount.clamp(0.0, 1.0);
    if let Some(ch) = channels.iter_mut().find(|c| c.id == id) {
        if let Some(send) = ch.sends.iter_mut().find(|s| s.target == target) {
            if overwrite_existing {
                send.amount = clamped;
            }
        } else {
            ch.sends.push(crate::mixer::ChannelSend {
                target,
                amount: clamped,
            });
        }
    }
}

fn upsert_mod_lane(lanes: &mut Vec<ModLane>, lane: ModLane) {
    if let Some(existing) = lanes
        .iter_mut()
        .find(|l| l.inst_idx == lane.inst_idx && l.target == lane.target)
    {
        *existing = lane;
    } else {
        lanes.push(lane);
    }
}

fn upsert_mixer_automation(automations: &mut Vec<MixerAutomation>, lane: MixerAutomation) {
    if let Some(existing) = automations.iter_mut().find(|l| l.target == lane.target) {
        *existing = lane;
    } else {
        automations.push(lane);
    }
}
