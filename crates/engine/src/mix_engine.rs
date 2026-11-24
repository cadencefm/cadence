//! Bounded native mixer runtime operating directly on `ScoreWindow`.
//! Prepared storage and fixed routing tables keep processing zero-alloc and
//! independent of frontend, filesystem, and audio-device APIs.
use cadence_core::dsp::{
    apply_fader_gain, apply_pan_balance, apply_strip_gain, limiter_lookahead_frames,
    one_pole_coeff_ms, one_pole_decay_ms, sanitize_frame, sanitize_sample, Chorus, Compressor,
    Delay, DelayLine, Drive, Eq, Limiter, PeakHold, Reverb, RhythmicFilter, StereoEffect,
};
use cadence_core::mixer::{
    ChannelKind, ChannelParam, ChannelRef, EffectKind, EffectKindTag, MixerParamTarget,
    MAX_CHANNELS, MAX_DELAY_SECONDS, MAX_DUCKS, MAX_EFFECT_DEVICES, MAX_INSERTS,
    MAX_MIXER_CHANNELS, MAX_SENDS,
};
use cadence_core::runtime::{ChannelMeter, EffectMeter};
use cadence_core::score::{ScoreWindow, MAX_INSTRUMENTS};

const MASTER: usize = MAX_CHANNELS - 1;

const _: () = {
    assert!(MAX_CHANNELS <= u8::MAX as usize);
    assert!(MAX_EFFECT_DEVICES <= u8::MAX as usize);
};

/// Count how many `Delay` insert devices exist in `score` (capped at `MAX_EFFECT_DEVICES`).
pub(crate) fn count_delay_devices(score: &ScoreWindow) -> usize {
    score
        .mixer
        .channels
        .iter()
        .take(MAX_MIXER_CHANNELS)
        .flat_map(|c| c.inserts.iter())
        .chain(score.mixer.master.inserts.iter())
        .take(MAX_EFFECT_DEVICES)
        .filter(|d| matches!(d.kind, EffectKind::Delay(_)))
        .count()
}

#[derive(Clone, Copy, Default)]
struct MeterState {
    input: [f32; 2],
    input_hold: [PeakHold; 2],
    input_max: [f32; 2],
    output: [f32; 2],
    output_hold: [PeakHold; 2],
    output_max: [f32; 2],
    hot: bool,
    clipped: bool,
}

impl MeterState {
    fn observe(
        &mut self,
        input: [f32; 2],
        output: [f32; 2],
        is_master: bool,
        decay: f32,
        hold_frames: u32,
        hold_decay: f32,
    ) {
        for channel in 0..2 {
            let raw_in = input[channel];
            let raw_out = output[channel];
            let finite = raw_in.is_finite() && raw_out.is_finite();
            let before = if raw_in.is_finite() {
                raw_in.abs()
            } else {
                0.0
            };
            let after = if raw_out.is_finite() {
                raw_out.abs()
            } else {
                0.0
            };
            self.input[channel] = (self.input[channel] * decay).max(before);
            self.output[channel] = (self.output[channel] * decay).max(after);

            self.input_hold[channel].observe(
                before,
                self.input[channel],
                hold_frames,
                hold_decay,
                1e-6,
            );
            self.output_hold[channel].observe(
                after,
                self.output[channel],
                hold_frames,
                hold_decay,
                1e-6,
            );

            self.input_max[channel] = self.input_max[channel].max(before);
            self.output_max[channel] = self.output_max[channel].max(after);
            self.hot |= before > 1.0 || after > 1.0;
            self.clipped |= !finite || (is_master && after > 1.0);
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ChannelRole {
    #[default]
    Inactive,
    HiddenSource,
    Audio,
    Group,
    Return,
    Master,
}

impl ChannelRole {
    #[inline]
    fn is_bus_source(self) -> bool {
        matches!(self, Self::Audio | Self::Group)
    }
}

#[derive(Clone, Copy, Default)]
struct SendPlan {
    /// Target Return channel index in `0..MAX_MIXER_CHANNELS`, or `None` when unused.
    target: Option<u8>,
    amount: f32,
    target_amount: f32,
}

#[derive(Clone, Copy)]
struct ChannelPlan {
    channel: Option<ChannelRef>,
    role: ChannelRole,
    source_enabled: bool,
    muted: bool,
    solo: bool,
    /// Downstream channel slot index in `0..MAX_CHANNELS`, or `None` for Inactive / Master.
    output: Option<u8>,
    /// Packed indices into `MixerEngine::effects`, with unused trailing entries `None`.
    inserts: [Option<u8>; MAX_INSERTS],
    /// Packed send entries, with unused trailing entries having `target: None`.
    sends: [SendPlan; MAX_SENDS],
    gain: f32,
    pan: f32,
    target_gain: f32,
    target_pan: f32,
    scale: f32,
    target_scale: f32,
    pdc_delay: usize,
    meter: MeterState,
}

impl Default for ChannelPlan {
    fn default() -> Self {
        Self {
            channel: None,
            role: ChannelRole::Inactive,
            source_enabled: true,
            muted: false,
            solo: false,
            output: None,
            inserts: [None; MAX_INSERTS],
            sends: [SendPlan::default(); MAX_SENDS],
            gain: 1.0,
            pan: 0.0,
            target_gain: 1.0,
            target_pan: 0.0,
            scale: 1.0,
            target_scale: 1.0,
            pdc_delay: 0,
            meter: MeterState::default(),
        }
    }
}

impl ChannelPlan {
    #[inline]
    fn output_slot(self) -> Option<usize> {
        self.output.map(usize::from)
    }

    #[inline]
    fn output_mixer_index(self) -> Option<usize> {
        self.output_slot()
            .filter(|&slot| (MAX_INSTRUMENTS..MASTER).contains(&slot))
            .map(|slot| slot - MAX_INSTRUMENTS)
    }

    #[inline]
    fn routes_to_master(self) -> bool {
        self.output_slot() == Some(MASTER)
    }
}

macro_rules! dispatch_effect_bank {
    (&$bank:expr, $tag:expr, $method:ident ( $($arg:expr),* )) => {
        dispatch_effect_bank!(@arm $bank, $tag, $method, [ $($arg),* ], [])
    };
    (&mut $bank:expr, $tag:expr, $method:ident ( $($arg:expr),* )) => {
        dispatch_effect_bank!(@arm $bank, $tag, $method, [ $($arg),* ], [mut])
    };
    (@arm $bank:expr, $tag:expr, $method:ident, [ $($arg:expr),* ], [ $($m:tt)? ]) => {
        match $tag {
            EffectKindTag::Delay => StereoEffect::$method(& $($m)? $bank.delay, $($arg),*),
            EffectKindTag::Reverb => StereoEffect::$method(& $($m)? $bank.reverb, $($arg),*),
            EffectKindTag::Eq => StereoEffect::$method(& $($m)? $bank.eq, $($arg),*),
            EffectKindTag::Chorus => StereoEffect::$method(& $($m)? $bank.chorus, $($arg),*),
            EffectKindTag::Drive => StereoEffect::$method(& $($m)? $bank.drive, $($arg),*),
            EffectKindTag::Filter => StereoEffect::$method(& $($m)? $bank.filter, $($arg),*),
            EffectKindTag::Compressor => StereoEffect::$method(& $($m)? $bank.compressor, $($arg),*),
            EffectKindTag::Limiter => StereoEffect::$method(& $($m)? $bank.limiter, $($arg),*),
        }
    };
}

/// Pre-allocated bank of all 8 native [`StereoEffect`] processors owned by one
/// [`EffectSlot`]. Keeping every effect instance pre-allocated (with `Delay`
/// buffers lazily supplied off-RT) allows `adopt()` to hot-swap slot effect
/// kinds on the audio thread with zero allocations and zero drops.
struct EffectBank {
    delay: Delay,
    reverb: Reverb,
    eq: Eq,
    chorus: Chorus,
    drive: Drive,
    filter: RhythmicFilter,
    compressor: Compressor,
    limiter: Limiter,
}

impl EffectBank {
    fn new(sample_rate: u32) -> Self {
        let sr = f64::from(sample_rate);
        Self {
            delay: Delay::new_unallocated(sample_rate),
            reverb: Reverb::new(sample_rate),
            eq: Eq::new(sr),
            chorus: Chorus::new(sr),
            drive: Drive::new(sr),
            filter: RhythmicFilter::new(sr),
            compressor: Compressor::new(sr),
            limiter: Limiter::new(sr),
        }
    }

    fn reset_all(&mut self) {
        StereoEffect::reset(&mut self.delay);
        StereoEffect::reset(&mut self.reverb);
        StereoEffect::reset(&mut self.eq);
        StereoEffect::reset(&mut self.chorus);
        StereoEffect::reset(&mut self.drive);
        StereoEffect::reset(&mut self.filter);
        StereoEffect::reset(&mut self.compressor);
        StereoEffect::reset(&mut self.limiter);
    }

    #[inline]
    fn process(&mut self, active: EffectKindTag, input: [f32; 2], bypassed: bool) -> [f32; 2] {
        dispatch_effect_bank!(&mut self, active, process(input, bypassed))
    }

    #[inline]
    fn latency_frames(&self, active: EffectKindTag) -> usize {
        dispatch_effect_bank!(&self, active, latency_frames())
    }

    #[inline]
    fn gain_reduction_db(&self, active: EffectKindTag) -> Option<f32> {
        dispatch_effect_bank!(&self, active, gain_reduction_db())
    }

    fn set_parameters(&mut self, id: u64, kind: EffectKind, score: &ScoreWindow, clock_pos: f64) {
        match kind {
            EffectKind::Delay(patch) => self.delay.set_parameters(patch, score.bpm),
            EffectKind::Reverb(patch) => self.reverb.set_parameters(patch),
            EffectKind::Eq(patch) => self.eq.set_parameters(patch),
            EffectKind::Chorus(patch) => self.chorus.set_parameters(patch),
            EffectKind::Drive(patch) => self.drive.set_parameters(patch),
            EffectKind::Filter(patch) => {
                let compiled = patch
                    .env_pattern_id
                    .and_then(|_| score.mixer.filter_env_trigger(id))
                    .unwrap_or_else(|| score.mixer.resolve_filter_groove(patch.env_pattern));
                self.filter
                    .set_parameters_with_clock(patch, compiled, clock_pos, score.bpm);
            }
            EffectKind::Compressor(patch) => self.compressor.set_parameters(patch),
            EffectKind::Limiter(patch) => self.limiter.set_parameters(patch),
        }
    }
}

struct EffectSlot {
    id: Option<u64>,
    bank: EffectBank,
    active: EffectKindTag,
    bypassed: bool,
    gr_hold: PeakHold,
    render_gr_max_db: f32,
    render_gr_sum_db: f64,
    render_gr_frames: u64,
}

impl EffectSlot {
    fn new(sample_rate: u32) -> Self {
        Self {
            id: None,
            bank: EffectBank::new(sample_rate),
            active: EffectKindTag::Eq,
            bypassed: false,
            gr_hold: PeakHold::new(),
            render_gr_max_db: 0.0,
            render_gr_sum_db: 0.0,
            render_gr_frames: 0,
        }
    }

    fn reset(&mut self) {
        self.bank.reset_all();
        self.clear_gr_meters();
    }

    fn clear_gr_meters(&mut self) {
        self.gr_hold.reset();
        self.render_gr_max_db = 0.0;
        self.render_gr_sum_db = 0.0;
        self.render_gr_frames = 0;
    }

    #[inline]
    fn latency_frames(&self) -> usize {
        if self.id.is_some() {
            self.bank.latency_frames(self.active)
        } else {
            0
        }
    }

    fn process(&mut self, input: [f32; 2], hold_frames: u32, hold_decay: f32) -> [f32; 2] {
        let out = self.bank.process(self.active, input, self.bypassed);
        if let Some(raw_gr) = self.bank.gain_reduction_db(self.active) {
            if self.bypassed {
                self.gr_hold.reset();
            } else {
                let gr = if raw_gr.is_finite() {
                    raw_gr.max(0.0)
                } else {
                    0.0
                };
                self.gr_hold.observe(gr, gr, hold_frames, hold_decay, 1e-4);
                self.render_gr_max_db = self.render_gr_max_db.max(gr);
                self.render_gr_sum_db += f64::from(gr);
                self.render_gr_frames += 1;
            }
        }
        out
    }

    #[inline]
    fn gain_reduction_db(&self) -> f32 {
        if self.bypassed {
            return 0.0;
        }
        self.bank.gain_reduction_db(self.active).unwrap_or(0.0)
    }

    #[inline]
    fn gr_hold_db(&self) -> f32 {
        if self.bypassed {
            0.0
        } else {
            self.gr_hold.value()
        }
    }
}

#[derive(Clone, Copy)]
struct DuckState {
    id: Option<u64>,
    source_part: u64,
    /// Target channel slot index in `0..MAX_CHANNELS`, or `None` when inactive.
    target: Option<u8>,
    depth: f32,
    recovery_frames: f64,
    elapsed: f64,
    gain: f32,
}

impl Default for DuckState {
    fn default() -> Self {
        Self {
            id: None,
            source_part: 0,
            target: None,
            depth: 0.0,
            recovery_frames: 1.0,
            elapsed: f64::INFINITY,
            gain: 1.0,
        }
    }
}

pub(crate) struct MixerEngine {
    channels: [ChannelPlan; MAX_CHANNELS],
    effects: Box<[EffectSlot]>,
    pdc_lines: Box<[DelayLine]>,
    group_order: [usize; MAX_MIXER_CHANNELS],
    group_count: usize,
    ducks: [DuckState; MAX_DUCKS],
    sample_rate: f64,
    envelope_smoothing: f32,
    factors: [f32; MAX_CHANNELS],
    wet_only: bool,
    smoothing: f32,
    meter_decay: f32,
    hold_frames: u32,
    hold_decay: f32,
}

impl MixerEngine {
    pub(crate) fn new(score: &ScoreWindow, sample_rate: u32) -> Box<Self> {
        let sr = f64::from(sample_rate);
        // Accommodate stacked limiters across chained Group → Group → Master paths without truncating PDC.
        let pdc_cap = limiter_lookahead_frames(sr).saturating_mul(MAX_EFFECT_DEVICES);
        let effects = (0..MAX_EFFECT_DEVICES)
            .map(|_| EffectSlot::new(sample_rate))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let pdc_lines = (0..MAX_MIXER_CHANNELS)
            .map(|_| DelayLine::new(pdc_cap))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let mut mixer = Box::new(Self {
            channels: [ChannelPlan::default(); MAX_CHANNELS],
            effects,
            pdc_lines,
            group_order: [0; MAX_MIXER_CHANNELS],
            group_count: 0,
            ducks: [DuckState::default(); MAX_DUCKS],
            sample_rate: sr,
            envelope_smoothing: one_pole_coeff_ms(1.0, sr) as f32,
            factors: [1.0; MAX_CHANNELS],
            wet_only: false,
            smoothing: one_pole_coeff_ms(10.0, sr) as f32,
            meter_decay: one_pole_decay_ms(300.0, sr) as f32,
            hold_frames: (1.5 * sr).round().max(1.0) as u32,
            hold_decay: one_pole_decay_ms(300.0, sr) as f32,
        });
        mixer.prepare_delays(score);
        mixer.adopt(score, true);
        mixer
    }

    /// Number of effect slots that currently hold an allocated stereo `Delay` buffer.
    pub(crate) fn allocated_delay_count(&self) -> usize {
        self.effects
            .iter()
            .filter(|slot| slot.bank.delay.is_allocated())
            .count()
    }

    /// Off-RT helper: ensure at least as many `Delay` buffers are allocated as `score` needs.
    pub(crate) fn prepare_delays(&mut self, score: &ScoreWindow) {
        let needed = count_delay_devices(score);
        let mut have = self.allocated_delay_count();
        for slot in self.effects.iter_mut() {
            if have >= needed {
                break;
            }
            if !slot.bank.delay.is_allocated() {
                slot.bank.delay.allocate(MAX_DELAY_SECONDS);
                have += 1;
            }
        }
    }

    /// Zero-alloc / zero-drop RT handoff: swap control-thread pre-allocated `Delay` buffers
    /// into unallocated slots (leaving empty unallocated shells in `incoming` for off-RT drop).
    pub(crate) fn install_delay_buffers(&mut self, incoming: &mut [Delay]) {
        for delay in incoming.iter_mut() {
            if !delay.is_allocated() {
                continue;
            }
            if let Some(slot) = self
                .effects
                .iter_mut()
                .find(|s| !s.bank.delay.is_allocated())
            {
                std::mem::swap(&mut slot.bank.delay, delay);
                slot.bank.delay.reset();
            }
        }
    }

    pub(crate) fn adopt(&mut self, score: &ScoreWindow, boundary: bool) {
        let mut wanted = [(None::<u64>, EffectKindTag::Eq); MAX_EFFECT_DEVICES];
        let mut wanted_count = 0usize;
        let devices = score
            .mixer
            .channels
            .iter()
            .take(MAX_MIXER_CHANNELS)
            .flat_map(|c| c.inserts.iter())
            .chain(score.mixer.master.inserts.iter());
        for device in devices.take(MAX_EFFECT_DEVICES) {
            wanted[wanted_count] = (Some(device.id), device.kind.tag());
            wanted_count += 1;
        }
        for slot in self.effects.iter_mut() {
            let keep = !boundary
                && slot.id.is_some()
                && wanted[..wanted_count].iter().any(|(id, _)| *id == slot.id);
            if !keep {
                slot.id = None;
                slot.reset();
            }
        }
        for &(id_opt, active) in &wanted[..wanted_count] {
            let Some(id) = id_opt else {
                continue;
            };
            if let Some(slot) = self.effects.iter_mut().find(|slot| slot.id == Some(id)) {
                if slot.active != active {
                    slot.reset();
                    slot.active = active;
                }
                continue;
            }
            // Prefer an allocated Delay slot for Delay devices and unallocated slots for others.
            let target_idx = if active == EffectKindTag::Delay {
                self.effects
                    .iter()
                    .position(|s| s.id.is_none() && s.bank.delay.is_allocated())
                    .or_else(|| self.effects.iter().position(|s| s.id.is_none()))
            } else {
                self.effects
                    .iter()
                    .position(|s| s.id.is_none() && !s.bank.delay.is_allocated())
                    .or_else(|| self.effects.iter().position(|s| s.id.is_none()))
            };
            if let Some(idx) = target_idx {
                self.effects[idx].id = Some(id);
                self.effects[idx].active = active;
                self.effects[idx].reset();
            }
        }

        // Zero-alloc donor swap: ensure every active Delay device owns an allocated buffer
        // whenever any free or non-Delay slot holds one.
        for i in 0..self.effects.len() {
            let needs_buf = self.effects[i].id.is_some()
                && self.effects[i].active == EffectKindTag::Delay
                && !self.effects[i].bank.delay.is_allocated();
            if !needs_buf {
                continue;
            }
            let donor = self.effects.iter().position(|s| {
                s.bank.delay.is_allocated() && (s.id.is_none() || s.active != EffectKindTag::Delay)
            });
            if let Some(j) = donor {
                if i < j {
                    let (left, right) = self.effects.split_at_mut(j);
                    std::mem::swap(&mut left[i].bank.delay, &mut right[0].bank.delay);
                } else if j < i {
                    let (left, right) = self.effects.split_at_mut(i);
                    std::mem::swap(&mut right[0].bank.delay, &mut left[j].bank.delay);
                }
                self.effects[i].bank.delay.reset();
            }
        }

        let previous = self.channels;
        self.channels.fill(ChannelPlan::default());
        let flags = &score.mixer.flags;
        let visible_channels =
            &score.mixer.channels[..score.mixer.channels.len().min(MAX_MIXER_CHANNELS)];
        let channel_slot = |id: u64| {
            visible_channels
                .iter()
                .position(|c| c.id == id)
                .map_or(MASTER, |i| MAX_INSTRUMENTS + i)
        };
        let source_solo = visible_channels.iter().any(|c| {
            c.kind != ChannelKind::Return && flags.channel_solo(ChannelRef::Channel(c.id))
        });

        // Slots 0..MAX_INSTRUMENTS: unity hidden instrument feeds into their Audio channels.
        for (index, inst) in score.instruments.iter().take(MAX_INSTRUMENTS).enumerate() {
            let mut plan = ChannelPlan {
                channel: None,
                role: ChannelRole::HiddenSource,
                source_enabled: true,
                output: Some(channel_slot(inst.channel) as u8),
                gain: 1.0,
                pan: 0.0,
                target_gain: 1.0,
                target_pan: 0.0,
                ..ChannelPlan::default()
            };
            if !boundary && previous[index].role == ChannelRole::HiddenSource {
                plan.meter = previous[index].meter;
            }
            self.channels[index] = plan;
        }

        // First pass: configure basic strip, inserts, sends, and output links for each mixer bus.
        for (index, ch) in visible_channels.iter().enumerate() {
            let slot = MAX_INSTRUMENTS + index;
            let cref = ChannelRef::Channel(ch.id);
            let role = match ch.kind {
                ChannelKind::Audio => ChannelRole::Audio,
                ChannelKind::Group => ChannelRole::Group,
                ChannelKind::Return => ChannelRole::Return,
            };
            self.configure_channel(score, slot, cref, role, &previous, boundary);
            self.channels[slot].output = Some(match ch.output {
                ChannelRef::Channel(id) => {
                    let target = channel_slot(id);
                    if target == slot {
                        MASTER as u8
                    } else {
                        target as u8
                    }
                }
                ChannelRef::Master => MASTER as u8,
            });
        }
        self.configure_channel(
            score,
            MASTER,
            ChannelRef::Master,
            ChannelRole::Master,
            &previous,
            boundary,
        );

        // Compute topological ordering for Group buses so chained groups (Group A -> Group B)
        // always sum in causal order with zero allocation and cycle-safe fallback.
        let chan_count = visible_channels.len();
        let mut group_in_degree = [0usize; MAX_MIXER_CHANNELS];
        let mut is_group = [false; MAX_MIXER_CHANNELS];
        let mut total_groups = 0usize;
        for (index, is_g) in is_group.iter_mut().enumerate().take(chan_count) {
            let slot = MAX_INSTRUMENTS + index;
            if self.channels[slot].role == ChannelRole::Group {
                *is_g = true;
                total_groups += 1;
            }
        }
        for (index, &is_g) in is_group.iter().enumerate().take(chan_count) {
            if !is_g {
                continue;
            }
            if let Some(target_idx) = self.channels[MAX_INSTRUMENTS + index].output_mixer_index() {
                if target_idx < MAX_MIXER_CHANNELS && is_group[target_idx] {
                    group_in_degree[target_idx] += 1;
                }
            }
        }
        let mut visited = [false; MAX_MIXER_CHANNELS];
        self.group_count = 0;
        for _ in 0..total_groups {
            let next = (0..chan_count)
                .find(|&idx| is_group[idx] && !visited[idx] && group_in_degree[idx] == 0)
                .or_else(|| (0..chan_count).find(|&idx| is_group[idx] && !visited[idx]));
            let Some(idx) = next else {
                break;
            };
            visited[idx] = true;
            self.group_order[self.group_count] = idx;
            self.group_count += 1;
            if let Some(target_idx) = self.channels[MAX_INSTRUMENTS + idx].output_mixer_index() {
                if target_idx < MAX_MIXER_CHANNELS && is_group[target_idx] {
                    group_in_degree[target_idx] = group_in_degree[target_idx].saturating_sub(1);
                }
            }
        }

        // Compute ancestor-aware mute/solo `source_enabled` for Audio and Group buses.
        let mut group_has_solo_descendant = [false; MAX_MIXER_CHANNELS];
        for index in 0..chan_count {
            let slot = MAX_INSTRUMENTS + index;
            let plan = self.channels[slot];
            if !plan.role.is_bus_source() || !plan.solo {
                continue;
            }
            let mut cur = plan.output_slot();
            for _ in 0..MAX_MIXER_CHANNELS {
                let Some(cur_slot) = cur.filter(|&s| (MAX_INSTRUMENTS..MASTER).contains(&s)) else {
                    break;
                };
                if self.channels[cur_slot].role != ChannelRole::Group {
                    break;
                }
                group_has_solo_descendant[cur_slot - MAX_INSTRUMENTS] = true;
                cur = self.channels[cur_slot].output_slot();
            }
        }
        for (index, &has_solo_desc) in group_has_solo_descendant
            .iter()
            .enumerate()
            .take(chan_count)
        {
            let slot = MAX_INSTRUMENTS + index;
            let plan = self.channels[slot];
            if !plan.role.is_bus_source() {
                continue;
            }
            let mut ancestor_muted = false;
            let mut ancestor_solo = false;
            let mut cur = plan.output_slot();
            for _ in 0..MAX_MIXER_CHANNELS {
                let Some(cur_slot) = cur.filter(|&s| (MAX_INSTRUMENTS..MASTER).contains(&s)) else {
                    break;
                };
                if self.channels[cur_slot].role != ChannelRole::Group {
                    break;
                }
                ancestor_muted |= self.channels[cur_slot].muted;
                ancestor_solo |= self.channels[cur_slot].solo;
                cur = self.channels[cur_slot].output_slot();
            }
            let solo_ok = !source_solo
                || plan.solo
                || ancestor_solo
                || (plan.role == ChannelRole::Group && has_solo_desc);
            self.channels[slot].source_enabled = !plan.muted && !ancestor_muted && solo_ok;
        }

        // Compute parallel-path Plugin Delay Compensation (PDC) so Limiters on Audio/Group/Return
        // buses align phase-accurately without comb filtering. When no non-Master Limiter exists,
        // every `pdc_delay` is 0 (zero extra latency, zero DelayLine overhead).
        let mut insert_lat = [0usize; MAX_MIXER_CHANNELS];
        let mut branch_lat = [0usize; MAX_MIXER_CHANNELS];
        for index in 0..chan_count {
            let slot = MAX_INSTRUMENTS + index;
            let plan = self.channels[slot];
            let mut lat = 0usize;
            for &eff_slot in plan.inserts.iter().flatten() {
                if let Some(effect) = self.effects.get(usize::from(eff_slot)) {
                    lat = lat.saturating_add(effect.latency_frames());
                }
            }
            insert_lat[index] = lat;
            if plan.role == ChannelRole::Audio {
                branch_lat[index] = lat;
            }
        }
        let group_order = self.group_order;
        for &g_idx in group_order.iter().take(self.group_count) {
            let g_slot = MAX_INSTRUMENTS + g_idx;
            let mut max_in = 0usize;
            for (c_idx, &lat) in branch_lat.iter().enumerate().take(chan_count) {
                let c_slot = MAX_INSTRUMENTS + c_idx;
                let plan = self.channels[c_slot];
                if plan.role.is_bus_source() && plan.output_slot() == Some(g_slot) {
                    max_in = max_in.max(lat);
                }
            }
            for (c_idx, &lat) in branch_lat.iter().enumerate().take(chan_count) {
                let c_slot = MAX_INSTRUMENTS + c_idx;
                let plan = self.channels[c_slot];
                if plan.role.is_bus_source() && plan.output_slot() == Some(g_slot) {
                    self.channels[c_slot].pdc_delay = max_in.saturating_sub(lat);
                }
            }
            branch_lat[g_idx] = max_in.saturating_add(insert_lat[g_idx]);
        }
        for r_idx in 0..chan_count {
            let r_slot = MAX_INSTRUMENTS + r_idx;
            if self.channels[r_slot].role != ChannelRole::Return {
                continue;
            }
            let r_tag = Some(r_idx as u8);
            let mut max_send = 0usize;
            for (c_idx, &lat) in branch_lat.iter().enumerate().take(chan_count) {
                let c_slot = MAX_INSTRUMENTS + c_idx;
                let plan = self.channels[c_slot];
                if plan.role.is_bus_source() && plan.sends.iter().any(|s| s.target == r_tag) {
                    max_send = max_send.max(lat);
                }
            }
            branch_lat[r_idx] = max_send.saturating_add(insert_lat[r_idx]);
        }
        let mut max_master_in = 0usize;
        for (c_idx, &lat) in branch_lat.iter().enumerate().take(chan_count) {
            let c_slot = MAX_INSTRUMENTS + c_idx;
            let plan = self.channels[c_slot];
            if plan.role == ChannelRole::Return
                || (plan.role.is_bus_source() && plan.routes_to_master())
            {
                max_master_in = max_master_in.max(lat);
            }
        }
        for (c_idx, &lat) in branch_lat.iter().enumerate().take(chan_count) {
            let c_slot = MAX_INSTRUMENTS + c_idx;
            let plan = self.channels[c_slot];
            if plan.role == ChannelRole::Return
                || (plan.role.is_bus_source() && plan.routes_to_master())
            {
                self.channels[c_slot].pdc_delay = max_master_in.saturating_sub(lat);
            }
            let max_cap = self.pdc_lines[c_idx].capacity();
            self.channels[c_slot].pdc_delay = self.channels[c_slot].pdc_delay.min(max_cap);
            if boundary || self.channels[c_slot].pdc_delay == 0 {
                self.pdc_lines[c_idx].reset();
            }
        }

        let previous_ducks = self.ducks;
        self.ducks.fill(DuckState::default());
        for (index, definition) in score.mixer.ducks.iter().take(MAX_DUCKS).enumerate() {
            let Some(target) = self
                .channels
                .iter()
                .position(|p| p.channel == Some(definition.target))
            else {
                continue;
            };
            let mut state = DuckState {
                id: Some(definition.id),
                source_part: definition.source_part,
                target: Some(target as u8),
                depth: definition.depth,
                recovery_frames: (f64::from(definition.recovery_ms) * 0.001 * self.sample_rate)
                    .max(1.0),
                ..DuckState::default()
            };
            if !boundary {
                if let Some(old) = previous_ducks.iter().find(|e| e.id == state.id) {
                    state.elapsed = old.elapsed;
                    state.gain = old.gain;
                }
            }
            self.ducks[index] = state;
        }

        // Ignore muted returns when deciding whether soloed returns isolate wet-only monitoring.
        self.wet_only = visible_channels.iter().any(|c| {
            let cref = ChannelRef::Channel(c.id);
            c.kind == ChannelKind::Return && flags.channel_solo(cref) && !flags.channel_muted(cref)
        });
    }

    fn configure_channel(
        &mut self,
        score: &ScoreWindow,
        index: usize,
        channel: ChannelRef,
        role: ChannelRole,
        previous: &[ChannelPlan; MAX_CHANNELS],
        boundary: bool,
    ) {
        let (strip, inserts, sends_slice) = match channel {
            ChannelRef::Channel(id) => match score.channel(id) {
                Some(ch) => (ch.strip, ch.inserts.as_slice(), ch.sends.as_slice()),
                None => return,
            },
            ChannelRef::Master => (
                score.mixer.master.strip,
                score.mixer.master.inserts.as_slice(),
                &[][..],
            ),
        };
        let muted = score.mixer.flags.channel_muted(channel);
        let solo = score.mixer.flags.channel_solo(channel);
        let mut plan = ChannelPlan {
            channel: Some(channel),
            role,
            gain: strip.gain,
            pan: strip.pan,
            target_gain: strip.gain,
            target_pan: strip.pan,
            muted,
            solo,
            ..ChannelPlan::default()
        };
        let old_plan = if !boundary {
            previous.iter().find(|old| old.channel == Some(channel))
        } else {
            None
        };
        if let Some(old) = old_plan {
            plan.gain = old.gain;
            plan.pan = old.pan;
            plan.meter = old.meter;
            plan.scale = old.scale;
        }
        let mut insert_count = 0usize;
        for device in inserts.iter().take(MAX_INSERTS) {
            if let Some(slot) = self
                .effects
                .iter()
                .position(|slot| slot.id == Some(device.id))
            {
                if insert_count < MAX_INSERTS {
                    plan.inserts[insert_count] = Some(slot as u8);
                    insert_count += 1;
                }
            }
        }
        let visible_channels =
            &score.mixer.channels[..score.mixer.channels.len().min(MAX_MIXER_CHANNELS)];
        let mut send_count = 0usize;
        for send in sends_slice {
            if send_count >= MAX_SENDS {
                break;
            }
            if let Some(target) = visible_channels
                .iter()
                .position(|c| c.id == send.target && c.kind == ChannelKind::Return)
            {
                let target_tag = Some(target as u8);
                let prev_amount = old_plan
                    .and_then(|old| {
                        old.sends
                            .iter()
                            .find(|s| s.target == target_tag)
                            .map(|s| s.amount)
                    })
                    .unwrap_or(send.amount);
                plan.sends[send_count] = SendPlan {
                    target: target_tag,
                    amount: prev_amount,
                    target_amount: send.amount,
                };
                send_count += 1;
            }
        }
        self.channels[index] = plan;
    }

    /// Snap all channel strip, send, and effect smoothers immediately to their current targets
    /// (eliminates startup slew when starting a mid-song render at `start_step > 0`).
    pub(crate) fn snap_smoothers(&mut self) {
        for plan in &mut self.channels {
            plan.gain = plan.target_gain;
            plan.pan = plan.target_pan;
            plan.scale = plan.target_scale;
            for send in &mut plan.sends {
                if send.target.is_some() {
                    send.amount = send.target_amount;
                }
            }
        }
        for slot in self.effects.iter_mut() {
            if slot.id.is_some() {
                slot.reset();
            }
        }
    }

    pub(crate) fn update_controls(&mut self, score: &ScoreWindow, auto_pos: f64, clock_pos: f64) {
        let visible_channels =
            &score.mixer.channels[..score.mixer.channels.len().min(MAX_MIXER_CHANNELS)];
        for plan in &mut self.channels[MAX_INSTRUMENTS..] {
            let Some(channel) = plan.channel else {
                continue;
            };
            let gain_target = MixerParamTarget::Channel {
                channel,
                param: ChannelParam::Gain,
            };
            let pan_target = MixerParamTarget::Channel {
                channel,
                param: ChannelParam::Pan,
            };
            let scale_target = MixerParamTarget::ChannelGainScale { channel };
            let strip = match channel {
                ChannelRef::Channel(id) => score.channel(id).map(|c| c.strip),
                ChannelRef::Master => Some(score.mixer.master.strip),
            };
            plan.target_gain = if score.has_mixer_automation(gain_target) {
                ChannelParam::Gain.denormalize(score.resolve_mixer_param(gain_target, auto_pos))
            } else {
                strip.map_or(1.0, |s| s.gain)
            };
            plan.target_pan = if score.has_mixer_automation(pan_target) {
                ChannelParam::Pan.denormalize(score.resolve_mixer_param(pan_target, auto_pos))
            } else {
                strip.map_or(0.0, |s| s.pan)
            };
            plan.target_scale = if score.has_mixer_automation(scale_target) {
                score.resolve_mixer_param(scale_target, auto_pos)
            } else {
                1.0
            };
            if let ChannelRef::Channel(id) = channel {
                if let Some(ch) = score.channel(id) {
                    let prev_sends = plan.sends;
                    plan.sends = [SendPlan::default(); MAX_SENDS];
                    let mut send_count = 0usize;
                    for send in &ch.sends {
                        if send_count >= MAX_SENDS {
                            break;
                        }
                        if let Some(target) = visible_channels
                            .iter()
                            .position(|c| c.id == send.target && c.kind == ChannelKind::Return)
                        {
                            let target_tag = Some(target as u8);
                            let send_target = MixerParamTarget::Send {
                                channel,
                                target: send.target,
                            };
                            let value = if score.has_mixer_automation(send_target) {
                                score.resolve_mixer_param(send_target, auto_pos)
                            } else {
                                send.amount
                            };
                            let cur_amount = prev_sends
                                .iter()
                                .find(|s| s.target == target_tag)
                                .map_or(value, |s| s.amount);
                            plan.sends[send_count] = SendPlan {
                                target: target_tag,
                                amount: cur_amount,
                                target_amount: value,
                            };
                            send_count += 1;
                        }
                    }
                }
            }
        }
        for slot in self.effects.iter_mut() {
            let Some(id) = slot.id else {
                continue;
            };
            let Some((_, device)) = score.find_device(id) else {
                continue;
            };
            let mut kind = device.kind;
            for &param in device.params() {
                let target = MixerParamTarget::Device {
                    device_id: id,
                    param,
                };
                if score.has_mixer_automation(target) {
                    let value = score.resolve_mixer_param(target, auto_pos);
                    param.write_kind(&mut kind, value);
                }
            }
            let active = kind.tag();
            if slot.active != active {
                slot.reset();
                slot.active = active;
            }
            slot.bank.set_parameters(id, kind, score, clock_pos);
            slot.bypassed = device.bypassed;
            if slot.bypassed {
                slot.gr_hold.reset();
            }
        }
    }

    pub(crate) fn reset(&mut self) {
        for effect in self.effects.iter_mut() {
            effect.reset();
        }
        for line in self.pdc_lines.iter_mut() {
            line.reset();
        }
        for duck in &mut self.ducks {
            duck.elapsed = f64::INFINITY;
            duck.gain = 1.0;
        }
        self.factors.fill(1.0);
        self.clear_meters(None);
    }

    pub(crate) fn clear_meters(&mut self, channel: Option<ChannelRef>) {
        for plan in &mut self.channels {
            if channel.is_none() || plan.channel == channel {
                plan.meter = MeterState::default();
                if channel.is_some() {
                    for &slot in plan.inserts.iter().flatten() {
                        if let Some(effect) = self.effects.get_mut(usize::from(slot)) {
                            effect.clear_gr_meters();
                        }
                    }
                }
            }
        }
        if channel.is_none() {
            for effect in self.effects.iter_mut() {
                effect.clear_gr_meters();
            }
        }
    }

    pub(crate) fn meters(&self) -> impl Iterator<Item = ChannelMeter> + '_ {
        self.channels.iter().filter_map(|plan| {
            plan.channel.map(|channel| ChannelMeter {
                channel,
                input_peak: plan.meter.input,
                input_peak_hold: [
                    plan.meter.input_hold[0].value(),
                    plan.meter.input_hold[1].value(),
                ],
                output_peak: plan.meter.output,
                output_peak_hold: [
                    plan.meter.output_hold[0].value(),
                    plan.meter.output_hold[1].value(),
                ],
                hot: plan.meter.hot,
                clipped: plan.meter.clipped,
            })
        })
    }

    /// Session-wide exact maxima for offline `RenderReport` (never loses peaks to 1.5s hold decay).
    pub(crate) fn render_channel_meters(&self) -> impl Iterator<Item = ChannelMeter> + '_ {
        self.channels.iter().filter_map(|plan| {
            plan.channel.map(|channel| ChannelMeter {
                channel,
                input_peak: plan.meter.input,
                input_peak_hold: plan.meter.input_max,
                output_peak: plan.meter.output,
                output_peak_hold: plan.meter.output_max,
                hot: plan.meter.hot,
                clipped: plan.meter.clipped,
            })
        })
    }

    pub(crate) fn effect_meters(&self) -> impl Iterator<Item = EffectMeter> + '_ {
        self.effects.iter().filter_map(|slot| {
            slot.id.map(|device_id| EffectMeter {
                device_id,
                gain_reduction_db: slot.gain_reduction_db(),
                gr_hold_db: slot.gr_hold_db(),
            })
        })
    }

    /// Session-wide dynamics gain reduction stats `(device_id, max_gr_db, avg_gr_db)` for active
    /// insert devices that report gain reduction via [`StereoEffect::gain_reduction_db`].
    pub(crate) fn render_effect_stats(&self) -> impl Iterator<Item = (u64, f32, f32)> + '_ {
        self.effects.iter().filter_map(|slot| {
            let id = slot.id?;
            slot.bank.gain_reduction_db(slot.active)?;
            if slot.bypassed && slot.render_gr_frames == 0 {
                return None;
            }
            let avg_gr_db = if slot.render_gr_frames > 0 {
                (slot.render_gr_sum_db / slot.render_gr_frames as f64) as f32
            } else {
                0.0
            };
            Some((id, slot.render_gr_max_db, avg_gr_db))
        })
    }

    fn inserts(&mut self, channel: usize, mut input: [f32; 2]) -> [f32; 2] {
        let plan = self.channels[channel];
        let hold_frames = self.hold_frames;
        let hold_decay = self.hold_decay;
        for &slot in plan.inserts.iter().flatten() {
            if let Some(effect) = self.effects.get_mut(usize::from(slot)) {
                input = effect.process(input, hold_frames, hold_decay);
            }
        }
        input
    }

    #[inline]
    fn strip_fader(&self, channel: usize, input: [f32; 2]) -> [f32; 2] {
        let plan = self.channels[channel];
        apply_fader_gain(input, plan.gain, plan.scale, self.factors[channel])
    }

    #[inline]
    fn strip(&self, channel: usize, input: [f32; 2]) -> [f32; 2] {
        let plan = self.channels[channel];
        apply_strip_gain(
            input,
            plan.gain,
            plan.scale,
            self.factors[channel],
            plan.pan,
        )
    }

    #[inline]
    fn sends(
        &self,
        channel: usize,
        output: [f32; 2],
        returns: &mut [[f32; 2]; MAX_MIXER_CHANNELS],
    ) {
        let plan = self.channels[channel];
        for send in &plan.sends {
            if let Some(target) = send.target.map(usize::from) {
                if target < MAX_MIXER_CHANNELS && send.amount > 0.0 {
                    returns[target][0] += output[0] * send.amount;
                    returns[target][1] += output[1] * send.amount;
                }
            }
        }
    }

    #[inline]
    fn apply_pdc(&mut self, channel_idx: usize, delay_frames: usize, output: [f32; 2]) -> [f32; 2] {
        if delay_frames == 0 {
            return output;
        }
        if let Some(line) = self.pdc_lines.get_mut(channel_idx) {
            // Read before writing for exact `z^{-delay_frames}` sample alignment with Limiter lookahead.
            let delayed = line.read_integer(delay_frames);
            line.write(output);
            delayed
        } else {
            output
        }
    }

    pub(crate) fn trigger_part(&mut self, part: u64) {
        for duck in &mut self.ducks {
            if duck.id.is_some() && duck.source_part == part {
                duck.elapsed = 0.0;
            }
        }
    }

    #[inline]
    fn route_output(
        &self,
        plan: ChannelPlan,
        output: [f32; 2],
        channels: &mut [[f32; 2]; MAX_MIXER_CHANNELS],
        master: &mut [f32; 2],
    ) {
        match plan.output_slot() {
            Some(MASTER) => {
                if !self.wet_only {
                    master[0] += output[0];
                    master[1] += output[1];
                }
            }
            Some(slot) if (MAX_INSTRUMENTS..MASTER).contains(&slot) => {
                let idx = slot - MAX_INSTRUMENTS;
                if idx < MAX_MIXER_CHANNELS {
                    channels[idx][0] += output[0];
                    channels[idx][1] += output[1];
                }
            }
            _ => {}
        }
    }

    #[inline]
    fn process_bus_channel(
        &mut self,
        index: usize,
        buses: &mut [[f32; 2]; MAX_MIXER_CHANNELS],
        returns: &mut [[f32; 2]; MAX_MIXER_CHANNELS],
        master: &mut [f32; 2],
    ) {
        let slot = MAX_INSTRUMENTS + index;
        let plan = self.channels[slot];
        let input = buses[index];
        let processed = self.inserts(slot, input);
        // Aux sends tap post-fader, pre-strip-pan so stereo sources stay stereo
        // and channel strip panning never starves opposite-panned Returns.
        let (post_fader, output) = if plan.source_enabled {
            let pf = self.strip_fader(slot, processed);
            (pf, apply_pan_balance(pf, plan.pan))
        } else {
            ([0.0; 2], [0.0; 2])
        };
        self.channels[slot].meter.observe(
            input,
            output,
            false,
            self.meter_decay,
            self.hold_frames,
            self.hold_decay,
        );
        self.sends(slot, sanitize_frame(post_fader), returns);
        let safe_out = sanitize_frame(output);
        let aligned = self.apply_pdc(index, plan.pdc_delay, safe_out);
        self.route_output(plan, aligned, buses, master);
    }

    pub(crate) fn process(
        &mut self,
        sources: &[[f32; 2]; MAX_INSTRUMENTS],
        preview: [f32; 2],
    ) -> [f32; 2] {
        self.factors.fill(1.0);
        for duck in &mut self.ducks {
            if duck.id.is_none() {
                continue;
            }
            let x = (duck.elapsed / duck.recovery_frames).clamp(0.0, 1.0) as f32;
            let target = 1.0 - duck.depth * (1.0 - x).powi(2);
            duck.gain += (target - duck.gain) * self.envelope_smoothing;
            if duck.elapsed > duck.recovery_frames + self.sample_rate * 0.01 {
                duck.gain = 1.0;
            }
            duck.elapsed += 1.0;
            if let Some(target_slot) = duck.target.map(usize::from) {
                if target_slot < MAX_CHANNELS {
                    self.factors[target_slot] *= duck.gain;
                }
            }
        }
        let sm = self.smoothing;
        for channel in &mut self.channels {
            if matches!(
                channel.role,
                ChannelRole::Inactive | ChannelRole::HiddenSource
            ) {
                continue;
            }
            channel.gain += (channel.target_gain - channel.gain) * sm;
            channel.pan += (channel.target_pan - channel.pan) * sm;
            channel.scale += (channel.target_scale - channel.scale) * sm;
            for send in &mut channel.sends {
                if send.target.is_none() {
                    break;
                }
                send.amount += (send.target_amount - send.amount) * sm;
            }
        }
        let mut buses = [[0.0; 2]; MAX_MIXER_CHANNELS];
        let mut returns = [[0.0; 2]; MAX_MIXER_CHANNELS];
        let mut master = [0.0; 2];

        let decay = self.meter_decay;
        let hold_frames = self.hold_frames;
        let hold_decay = self.hold_decay;

        for (index, &raw_source) in sources.iter().enumerate() {
            let plan = self.channels[index];
            if plan.role != ChannelRole::HiddenSource {
                continue;
            }
            let source_non_finite = !raw_source[0].is_finite() || !raw_source[1].is_finite();
            let output = sanitize_frame(raw_source);
            if source_non_finite {
                if let Some(out_slot) = plan.output_slot().filter(|&s| s < MAX_CHANNELS) {
                    self.channels[out_slot].meter.clipped = true;
                }
            }
            self.route_output(plan, output, &mut buses, &mut master);
        }

        // Process Audio channels.
        for index in 0..MAX_MIXER_CHANNELS {
            let slot = MAX_INSTRUMENTS + index;
            if self.channels[slot].role != ChannelRole::Audio {
                continue;
            }
            self.process_bus_channel(index, &mut buses, &mut returns, &mut master);
        }

        // Process Group channels in causal topological order (supports chained groups).
        let group_order = self.group_order;
        for &index in group_order.iter().take(self.group_count) {
            self.process_bus_channel(index, &mut buses, &mut returns, &mut master);
        }

        // Process Return buses.
        for (index, &input) in returns.iter().enumerate() {
            let slot = MAX_INSTRUMENTS + index;
            let plan = self.channels[slot];
            if plan.role != ChannelRole::Return {
                continue;
            }
            let processed = self.inserts(slot, input);
            let output = if !plan.muted && (!self.wet_only || plan.solo) {
                self.strip(slot, processed)
            } else {
                [0.0; 2]
            };
            self.channels[slot]
                .meter
                .observe(input, output, false, decay, hold_frames, hold_decay);
            let safe_out = sanitize_frame(output);
            let aligned = self.apply_pdc(index, plan.pdc_delay, safe_out);
            master[0] += aligned[0];
            master[1] += aligned[1];
        }

        let processed = self.inserts(MASTER, master);
        let output = if self.channels[MASTER].muted {
            [0.0; 2]
        } else {
            self.strip(MASTER, processed)
        };
        self.channels[MASTER]
            .meter
            .observe(master, output, true, decay, hold_frames, hold_decay);
        if !preview[0].is_finite() || !preview[1].is_finite() {
            self.channels[MASTER].meter.clipped = true;
        }

        // Preview audition sums after Master mute/strip so muting Master never silences browser previews.
        [
            sanitize_sample(output[0] + preview[0]),
            sanitize_sample(output[1] + preview[1]),
        ]
    }
}

#[cfg(test)]
pub(crate) mod tests;
