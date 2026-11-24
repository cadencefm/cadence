//! Bounded native mixer runtime operating directly on `ScoreWindow`.
//! Prepared storage and fixed routing tables keep processing zero-alloc and
//! independent of frontend, filesystem, and audio-device APIs.
use cadence_core::dsp::{
    apply_fader_gain, limiter_lookahead_frames, one_pole_coeff_ms, one_pole_decay_ms,
    pan_stereo_balance, sanitize_frame, sanitize_sample, smooth_step_f32, Chorus, Compressor,
    Delay, DelayLine, Drive, Eq, Limiter, PeakHold, Reverb, RhythmicFilter, StereoEffect,
    AUDIO_SUB_BLOCK_FRAMES,
};
use cadence_core::mixer::{
    ChannelKind, ChannelParam, ChannelRef, EffectKind, EffectKindTag, MixerParamTarget,
    MAX_CHANNELS, MAX_DELAY_SECONDS, MAX_DUCKS, MAX_EFFECT_DEVICES, MAX_INSERTS,
    MAX_MIXER_CHANNELS, MAX_SENDS,
};
use cadence_core::runtime::{ChannelMeter, EffectMeter};
use cadence_core::score::{ScoreWindow, MAX_INSTRUMENTS};

const MASTER: usize = MAX_CHANNELS - 1;
/// Gain-reduction meter hold snap floor in dB (`0.0001 dB`).
const GR_HOLD_ZERO_FLOOR_DB: f32 = 1e-4;

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
                PeakHold::DEFAULT_ZERO_FLOOR,
            );
            self.output_hold[channel].observe(
                after,
                self.output[channel],
                hold_frames,
                hold_decay,
                PeakHold::DEFAULT_ZERO_FLOOR,
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
    pdc_delay: usize,
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
    cached_pan_bits: u32,
    cached_pan_lr: [f32; 2],
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
            cached_pan_bits: 0.0_f32.to_bits(),
            cached_pan_lr: [1.0, 1.0],
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
    fn refresh_pan_cache(&mut self) {
        let bits = self.pan.to_bits();
        if bits != self.cached_pan_bits {
            self.cached_pan_bits = bits;
            self.cached_pan_lr = if self.pan == 0.0 {
                [1.0, 1.0]
            } else {
                pan_stereo_balance(self.pan)
            };
        }
    }

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
    fn process_block_with_gr(
        &mut self,
        active: EffectKindTag,
        buf: &mut [[f32; 2]],
        bypassed: bool,
        gr_out: &mut [f32],
    ) -> bool {
        dispatch_effect_bank!(
            &mut self,
            active,
            process_block_with_gr(buf, bypassed, gr_out)
        )
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

    fn process_block(
        &mut self,
        buf: &mut [[f32; 2]],
        hold_frames: u32,
        hold_decay: f32,
        gr_scratch: &mut [f32; AUDIO_SUB_BLOCK_FRAMES],
    ) {
        let n = buf.len();
        let has_gr =
            self.bank
                .process_block_with_gr(self.active, buf, self.bypassed, &mut gr_scratch[..n]);
        if has_gr {
            if self.bypassed {
                self.gr_hold.reset();
            } else {
                for &raw_gr in &gr_scratch[..n] {
                    let gr = if raw_gr.is_finite() {
                        raw_gr.max(0.0)
                    } else {
                        0.0
                    };
                    self.gr_hold
                        .observe(gr, gr, hold_frames, hold_decay, GR_HOLD_ZERO_FLOOR_DB);
                    self.render_gr_max_db = self.render_gr_max_db.max(gr);
                    self.render_gr_sum_db += f64::from(gr);
                    self.render_gr_frames += 1;
                }
            }
        }
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

/// Maximum concurrent pending PDC-aligned duck triggers per duck state (zero-alloc).
const DUCK_PENDING_CAPACITY: usize = 4;

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
    pdc_delay: u32,
    pending_delays: [u32; DUCK_PENDING_CAPACITY],
    pending_len: u8,
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
            pdc_delay: 0,
            pending_delays: [0; DUCK_PENDING_CAPACITY],
            pending_len: 0,
        }
    }
}

/// Heap-boxed sub-block scratch buffers owned by [`MixerEngine`] so debug stacks
/// stay compact and block mixing performs strictly zero heap allocations.
struct MixerBlockScratch {
    buses: [[[f32; 2]; AUDIO_SUB_BLOCK_FRAMES]; MAX_MIXER_CHANNELS],
    returns: [[[f32; 2]; AUDIO_SUB_BLOCK_FRAMES]; MAX_MIXER_CHANNELS],
    master: [[f32; 2]; AUDIO_SUB_BLOCK_FRAMES],
    work: [[f32; 2]; AUDIO_SUB_BLOCK_FRAMES],
    post_fader: [[f32; 2]; AUDIO_SUB_BLOCK_FRAMES],
    factors: [[f32; AUDIO_SUB_BLOCK_FRAMES]; MAX_CHANNELS],
    gr_scratch: [f32; AUDIO_SUB_BLOCK_FRAMES],
    gain_buf: [f32; AUDIO_SUB_BLOCK_FRAMES],
    scale_buf: [f32; AUDIO_SUB_BLOCK_FRAMES],
    pan_lr_buf: [[f32; 2]; AUDIO_SUB_BLOCK_FRAMES],
    send_amt_buf: [[f32; AUDIO_SUB_BLOCK_FRAMES]; MAX_SENDS],
}

impl MixerBlockScratch {
    fn new_boxed() -> Box<Self> {
        Box::new(Self {
            buses: [[[0.0; 2]; AUDIO_SUB_BLOCK_FRAMES]; MAX_MIXER_CHANNELS],
            returns: [[[0.0; 2]; AUDIO_SUB_BLOCK_FRAMES]; MAX_MIXER_CHANNELS],
            master: [[0.0; 2]; AUDIO_SUB_BLOCK_FRAMES],
            work: [[0.0; 2]; AUDIO_SUB_BLOCK_FRAMES],
            post_fader: [[0.0; 2]; AUDIO_SUB_BLOCK_FRAMES],
            factors: [[1.0; AUDIO_SUB_BLOCK_FRAMES]; MAX_CHANNELS],
            gr_scratch: [0.0; AUDIO_SUB_BLOCK_FRAMES],
            gain_buf: [0.0; AUDIO_SUB_BLOCK_FRAMES],
            scale_buf: [0.0; AUDIO_SUB_BLOCK_FRAMES],
            pan_lr_buf: [[1.0; 2]; AUDIO_SUB_BLOCK_FRAMES],
            send_amt_buf: [[0.0; AUDIO_SUB_BLOCK_FRAMES]; MAX_SENDS],
        })
    }
}

pub(crate) struct MixerEngine {
    channels: [ChannelPlan; MAX_CHANNELS],
    effects: Box<[EffectSlot]>,
    pdc_lines: Box<[DelayLine]>,
    scratch: Box<MixerBlockScratch>,
    group_order: [usize; MAX_MIXER_CHANNELS],
    group_count: usize,
    ducks: [DuckState; MAX_DUCKS],
    total_output_latency: usize,
    sample_rate: f64,
    envelope_smoothing: f32,
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
            scratch: MixerBlockScratch::new_boxed(),
            group_order: [0; MAX_MIXER_CHANNELS],
            group_count: 0,
            ducks: [DuckState::default(); MAX_DUCKS],
            total_output_latency: 0,
            sample_rate: sr,
            envelope_smoothing: one_pole_coeff_ms(1.0, sr) as f32,
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

    /// Total cumulative output Plugin Delay Compensation (PDC) latency in frames
    /// (`max_master_in + master_insert_latency`).
    #[inline]
    pub(crate) fn total_output_latency_frames(&self) -> usize {
        self.total_output_latency
    }

    /// Current Master decayed stereo output peaks (300 ms release, observing the exact
    /// final summed DAC/WAV output including audition preview and software input monitor).
    #[inline]
    pub(crate) fn master_output_peaks(&self) -> [f32; 2] {
        self.channels[MASTER].meter.output
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
        let mut return_max_send = [0usize; MAX_MIXER_CHANNELS];
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
            return_max_send[r_idx] = max_send;
            for (c_idx, &lat) in branch_lat.iter().enumerate().take(chan_count) {
                let c_slot = MAX_INSTRUMENTS + c_idx;
                if !self.channels[c_slot].role.is_bus_source() {
                    continue;
                }
                let max_cap = self.pdc_lines[c_idx].capacity();
                for send in &mut self.channels[c_slot].sends {
                    if send.target == r_tag {
                        send.pdc_delay = return_max_send[r_idx].saturating_sub(lat).min(max_cap);
                    }
                }
            }
            branch_lat[r_idx] = return_max_send[r_idx].saturating_add(insert_lat[r_idx]);
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
            let any_send_pdc = self.channels[c_slot]
                .sends
                .iter()
                .any(|s| s.target.is_some() && s.pdc_delay > 0);
            let slot_changed = previous[c_slot].channel != self.channels[c_slot].channel;
            if boundary || slot_changed || (self.channels[c_slot].pdc_delay == 0 && !any_send_pdc) {
                self.pdc_lines[c_idx].reset();
            }
        }
        for line in self.pdc_lines.iter_mut().skip(chan_count) {
            line.reset();
        }
        let mut master_insert_lat = 0usize;
        for &eff_slot in self.channels[MASTER].inserts.iter().flatten() {
            if let Some(effect) = self.effects.get(usize::from(eff_slot)) {
                master_insert_lat = master_insert_lat.saturating_add(effect.latency_frames());
            }
        }
        self.total_output_latency = max_master_in.saturating_add(master_insert_lat);

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
            let pdc_delay = if target == MASTER {
                self.total_output_latency
            } else if (MAX_INSTRUMENTS..MASTER).contains(&target) {
                branch_lat[target - MAX_INSTRUMENTS]
            } else {
                0
            };
            let mut state = DuckState {
                id: Some(definition.id),
                source_part: definition.source_part,
                target: Some(target as u8),
                depth: definition.depth,
                recovery_frames: (f64::from(definition.recovery_ms) * 0.001 * self.sample_rate)
                    .max(1.0),
                pdc_delay: pdc_delay.min(u32::MAX as usize) as u32,
                ..DuckState::default()
            };
            if !boundary {
                if let Some(old) = previous_ducks.iter().find(|e| e.id == state.id) {
                    state.elapsed = old.elapsed;
                    state.gain = old.gain;
                    state.pending_delays = old.pending_delays;
                    state.pending_len = old.pending_len;
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
        plan.refresh_pan_cache();
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
                    pdc_delay: 0,
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
            plan.refresh_pan_cache();
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
                            let (cur_amount, pdc_delay) = prev_sends
                                .iter()
                                .find(|s| s.target == target_tag)
                                .map_or((value, 0), |s| (s.amount, s.pdc_delay));
                            plan.sends[send_count] = SendPlan {
                                target: target_tag,
                                amount: cur_amount,
                                target_amount: value,
                                pdc_delay,
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
            duck.pending_delays = [0; DUCK_PENDING_CAPACITY];
            duck.pending_len = 0;
        }
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
}

/// Post-recovery grace window (`10 ms`) after which an idle duck envelope snaps to `1.0`.
const DUCK_TAIL_SETTLE_SEC: f64 = 0.01;

/// Step one channel's strip (`gain`, `pan`, `scale`) and active aux-send smoothers
/// across `frames` frames, returning `true` when strip smoothers were already settled.
#[inline]
fn step_channel_smoothers(
    channel: &mut ChannelPlan,
    sm: f32,
    frames: usize,
    gain_buf: &mut [f32; AUDIO_SUB_BLOCK_FRAMES],
    scale_buf: &mut [f32; AUDIO_SUB_BLOCK_FRAMES],
    pan_lr_buf: &mut [[f32; 2]; AUDIO_SUB_BLOCK_FRAMES],
    send_amt_buf: &mut [[f32; AUDIO_SUB_BLOCK_FRAMES]; MAX_SENDS],
) -> bool {
    let strip_settled = channel.gain == channel.target_gain
        && channel.pan == channel.target_pan
        && channel.scale == channel.target_scale;
    if strip_settled {
        channel.refresh_pan_cache();
    } else {
        for i in 0..frames {
            smooth_step_f32(&mut channel.gain, channel.target_gain, sm);
            smooth_step_f32(&mut channel.pan, channel.target_pan, sm);
            channel.refresh_pan_cache();
            smooth_step_f32(&mut channel.scale, channel.target_scale, sm);
            gain_buf[i] = channel.gain;
            scale_buf[i] = channel.scale;
            pan_lr_buf[i] = channel.cached_pan_lr;
        }
    }
    for (s_idx, send) in channel.sends.iter_mut().enumerate() {
        if send.target.is_none() {
            break;
        }
        if send.amount == send.target_amount {
            send_amt_buf[s_idx][..frames].fill(send.amount);
        } else {
            for amt in &mut send_amt_buf[s_idx][..frames] {
                smooth_step_f32(&mut send.amount, send.target_amount, sm);
                *amt = send.amount;
            }
        }
    }
    strip_settled
}

#[inline]
fn route_output_block(
    wet_only: bool,
    plan: ChannelPlan,
    output: &[[f32; 2]],
    buses: &mut [[[f32; 2]; AUDIO_SUB_BLOCK_FRAMES]; MAX_MIXER_CHANNELS],
    master: &mut [[f32; 2]; AUDIO_SUB_BLOCK_FRAMES],
) {
    match plan.output_slot() {
        Some(MASTER) => {
            if !wet_only {
                for (dst, &src) in master[..output.len()].iter_mut().zip(output) {
                    dst[0] += src[0];
                    dst[1] += src[1];
                }
            }
        }
        Some(slot) if (MAX_INSTRUMENTS..MASTER).contains(&slot) => {
            let idx = slot - MAX_INSTRUMENTS;
            if idx < MAX_MIXER_CHANNELS {
                for (dst, &src) in buses[idx][..output.len()].iter_mut().zip(output) {
                    dst[0] += src[0];
                    dst[1] += src[1];
                }
            }
        }
        _ => {}
    }
}

fn run_inserts_block(
    inserts: [Option<u8>; MAX_INSERTS],
    effects: &mut [EffectSlot],
    buf: &mut [[f32; 2]],
    hold_frames: u32,
    hold_decay: f32,
    gr_scratch: &mut [f32; AUDIO_SUB_BLOCK_FRAMES],
) {
    for &slot in inserts.iter().flatten() {
        if let Some(effect) = effects.get_mut(usize::from(slot)) {
            effect.process_block(buf, hold_frames, hold_decay, gr_scratch);
        }
    }
}

#[inline]
fn apply_pan_lr(input: [f32; 2], pan_lr: [f32; 2]) -> [f32; 2] {
    [
        sanitize_sample(input[0]) * pan_lr[0],
        sanitize_sample(input[1]) * pan_lr[1],
    ]
}

impl MixerEngine {
    pub(crate) fn trigger_part(&mut self, part: u64) {
        for duck in &mut self.ducks {
            if duck.id.is_some() && duck.source_part == part {
                if duck.pdc_delay == 0 {
                    duck.elapsed = 0.0;
                } else {
                    let len = usize::from(duck.pending_len).min(DUCK_PENDING_CAPACITY);
                    if len < DUCK_PENDING_CAPACITY {
                        duck.pending_delays[len] = duck.pdc_delay;
                        duck.pending_len = (len + 1) as u8;
                    } else {
                        duck.pending_delays.copy_within(1..DUCK_PENDING_CAPACITY, 0);
                        duck.pending_delays[DUCK_PENDING_CAPACITY - 1] = duck.pdc_delay;
                    }
                }
            }
        }
    }

    fn process_bus_channel_block(
        &mut self,
        index: usize,
        frames: usize,
        duck_dirty: &[bool; MAX_CHANNELS],
    ) {
        let slot = MAX_INSTRUMENTS + index;
        let sm = self.smoothing;
        let decay = self.meter_decay;
        let hold_frames = self.hold_frames;
        let hold_decay = self.hold_decay;
        let wet_only = self.wet_only;

        let scratch = &mut *self.scratch;
        let strip_settled = step_channel_smoothers(
            &mut self.channels[slot],
            sm,
            frames,
            &mut scratch.gain_buf,
            &mut scratch.scale_buf,
            &mut scratch.pan_lr_buf,
            &mut scratch.send_amt_buf,
        );
        let plan = self.channels[slot];
        let input_slice = &scratch.buses[index][..frames];
        scratch.work[..frames].copy_from_slice(input_slice);
        run_inserts_block(
            plan.inserts,
            &mut self.effects,
            &mut scratch.work[..frames],
            hold_frames,
            hold_decay,
            &mut scratch.gr_scratch,
        );

        let has_duck = duck_dirty[slot];
        let mut inserts_non_finite = false;
        let meter = &mut self.channels[slot].meter;
        if plan.source_enabled {
            let const_gain = plan.gain;
            let const_scale = plan.scale;
            let const_pan = plan.cached_pan_lr;
            for (i, &inp) in input_slice.iter().enumerate() {
                let processed = scratch.work[i];
                if !processed[0].is_finite() || !processed[1].is_finite() {
                    inserts_non_finite = true;
                }
                let duck = if has_duck {
                    scratch.factors[slot][i]
                } else {
                    1.0
                };
                let (g, sc, pan_lr) = if strip_settled {
                    (const_gain, const_scale, const_pan)
                } else {
                    (
                        scratch.gain_buf[i],
                        scratch.scale_buf[i],
                        scratch.pan_lr_buf[i],
                    )
                };
                let pf = apply_fader_gain(processed, g, sc, duck);
                let out = apply_pan_lr(pf, pan_lr);
                scratch.post_fader[i] = sanitize_frame(pf);
                meter.observe(inp, out, false, decay, hold_frames, hold_decay);
            }
        } else {
            for (i, &inp) in input_slice.iter().enumerate() {
                let processed = scratch.work[i];
                if !processed[0].is_finite() || !processed[1].is_finite() {
                    inserts_non_finite = true;
                }
                scratch.post_fader[i] = [0.0; 2];
                meter.observe(inp, [0.0; 2], false, decay, hold_frames, hold_decay);
            }
        }
        if inserts_non_finite {
            meter.clipped = true;
        }

        // Reuse the channel's single PDC delay line (holding sanitized post-fader, pre-pan audio)
        // to tap both Return-aligned sends and main output before writing once per frame.
        let any_send_pdc = plan
            .sends
            .iter()
            .any(|s| s.target.is_some() && s.pdc_delay > 0);
        let needs_pdc_write = plan.pdc_delay > 0 || any_send_pdc;
        let const_pan = plan.cached_pan_lr;

        if needs_pdc_write {
            if let Some(line) = self.pdc_lines.get_mut(index) {
                for i in 0..frames {
                    let safe_pf = scratch.post_fader[i];
                    for (s_idx, send) in plan.sends.iter().enumerate() {
                        let Some(target) = send.target.map(usize::from) else {
                            break;
                        };
                        let amt = scratch.send_amt_buf[s_idx][i];
                        if target < MAX_MIXER_CHANNELS && amt > 0.0 {
                            let tapped = if send.pdc_delay == 0 {
                                safe_pf
                            } else {
                                line.read_integer(send.pdc_delay)
                            };
                            scratch.returns[target][i][0] += tapped[0] * amt;
                            scratch.returns[target][i][1] += tapped[1] * amt;
                        }
                    }
                    let main_pf = if plan.pdc_delay == 0 {
                        safe_pf
                    } else {
                        line.read_integer(plan.pdc_delay)
                    };
                    line.write(safe_pf);
                    let pan_lr = if strip_settled {
                        const_pan
                    } else {
                        scratch.pan_lr_buf[i]
                    };
                    scratch.work[i] = apply_pan_lr(main_pf, pan_lr);
                }
            } else {
                for (s_idx, send) in plan.sends.iter().enumerate() {
                    let Some(target) = send.target.map(usize::from) else {
                        break;
                    };
                    if target < MAX_MIXER_CHANNELS {
                        for i in 0..frames {
                            let amt = scratch.send_amt_buf[s_idx][i];
                            if amt > 0.0 {
                                scratch.returns[target][i][0] += scratch.post_fader[i][0] * amt;
                                scratch.returns[target][i][1] += scratch.post_fader[i][1] * amt;
                            }
                        }
                    }
                }
                for i in 0..frames {
                    let pan_lr = if strip_settled {
                        const_pan
                    } else {
                        scratch.pan_lr_buf[i]
                    };
                    scratch.work[i] = apply_pan_lr(scratch.post_fader[i], pan_lr);
                }
            }
        } else {
            for (s_idx, send) in plan.sends.iter().enumerate() {
                let Some(target) = send.target.map(usize::from) else {
                    break;
                };
                if target < MAX_MIXER_CHANNELS {
                    for i in 0..frames {
                        let amt = scratch.send_amt_buf[s_idx][i];
                        if amt > 0.0 {
                            scratch.returns[target][i][0] += scratch.post_fader[i][0] * amt;
                            scratch.returns[target][i][1] += scratch.post_fader[i][1] * amt;
                        }
                    }
                }
            }
            if strip_settled {
                for i in 0..frames {
                    scratch.work[i] = apply_pan_lr(scratch.post_fader[i], const_pan);
                }
            } else {
                for i in 0..frames {
                    scratch.work[i] = apply_pan_lr(scratch.post_fader[i], scratch.pan_lr_buf[i]);
                }
            }
        }

        route_output_block(
            wet_only,
            plan,
            &scratch.work[..frames],
            &mut scratch.buses,
            &mut scratch.master,
        );
    }

    /// Process a contiguous sub-block (`1..=AUDIO_SUB_BLOCK_FRAMES`) through the full
    /// mixer routing DAG, inserts, PDC lines, sends, ducking, and metering.
    pub(crate) fn process_block(
        &mut self,
        sources: &[[[f32; 2]; AUDIO_SUB_BLOCK_FRAMES]; MAX_INSTRUMENTS],
        source_active_mask: u32,
        preview: &[[f32; 2]],
        monitor: &[[f32; 2]],
        out: &mut [[f32; 2]],
    ) {
        let frames = out.len().min(AUDIO_SUB_BLOCK_FRAMES);
        if frames == 0 {
            return;
        }

        let mut duck_dirty = [false; MAX_CHANNELS];
        let env_sm = self.envelope_smoothing;
        let settle_margin = self.sample_rate * DUCK_TAIL_SETTLE_SEC;
        for duck in &mut self.ducks {
            if duck.id.is_none() {
                continue;
            }
            if duck.pending_len == 0
                && duck.gain == 1.0
                && duck.elapsed > duck.recovery_frames + settle_margin
            {
                duck.elapsed += frames as f64;
                continue;
            }
            let target_slot = duck.target.map(usize::from).filter(|&s| s < MAX_CHANNELS);
            if let Some(ts) = target_slot {
                if !duck_dirty[ts] {
                    self.scratch.factors[ts][..frames].fill(1.0);
                    duck_dirty[ts] = true;
                }
            }
            for i in 0..frames {
                let x = (duck.elapsed / duck.recovery_frames).clamp(0.0, 1.0) as f32;
                let target = 1.0 - duck.depth * (1.0 - x).powi(2);
                smooth_step_f32(&mut duck.gain, target, env_sm);
                if duck.elapsed > duck.recovery_frames + settle_margin {
                    duck.gain = 1.0;
                }
                duck.elapsed += 1.0;
                if let Some(ts) = target_slot {
                    self.scratch.factors[ts][i] *= duck.gain;
                }
                // Advance pending PDC-aligned duck countdowns after evaluating this frame so
                // a delay of `N` frames arms `elapsed = 0.0` on exact frame `N`.
                let mut triggered = false;
                let mut j = 0usize;
                let mut len = usize::from(duck.pending_len).min(DUCK_PENDING_CAPACITY);
                while j < len {
                    if duck.pending_delays[j] <= 1 {
                        triggered = true;
                        len -= 1;
                        duck.pending_delays.copy_within(j + 1..len + 1, j);
                        duck.pending_delays[len] = 0;
                    } else {
                        duck.pending_delays[j] -= 1;
                        j += 1;
                    }
                }
                duck.pending_len = len as u8;
                if triggered {
                    duck.elapsed = 0.0;
                }
            }
        }

        // Clear bus, return, and master accumulators for active channels in this sub-block.
        for index in 0..MAX_MIXER_CHANNELS {
            let role = self.channels[MAX_INSTRUMENTS + index].role;
            match role {
                ChannelRole::Audio | ChannelRole::Group => {
                    self.scratch.buses[index][..frames].fill([0.0; 2]);
                }
                ChannelRole::Return => {
                    self.scratch.returns[index][..frames].fill([0.0; 2]);
                }
                _ => {}
            }
        }
        self.scratch.master[..frames].fill([0.0; 2]);

        // Route active HiddenSource instrument stems into target Audio/Group/Master buses.
        let mut mask = source_active_mask;
        while mask != 0 {
            let index = mask.trailing_zeros() as usize;
            mask &= mask - 1;
            if index >= MAX_INSTRUMENTS {
                break;
            }
            let plan = self.channels[index];
            if plan.role != ChannelRole::HiddenSource {
                continue;
            }
            let mut source_non_finite = false;
            let src = &sources[index][..frames];
            for (dst, &raw) in self.scratch.work[..frames].iter_mut().zip(src) {
                if !raw[0].is_finite() || !raw[1].is_finite() {
                    source_non_finite = true;
                }
                *dst = sanitize_frame(raw);
            }
            if source_non_finite {
                if let Some(out_slot) = plan.output_slot().filter(|&s| s < MAX_CHANNELS) {
                    self.channels[out_slot].meter.clipped = true;
                }
            }
            route_output_block(
                self.wet_only,
                plan,
                &self.scratch.work[..frames],
                &mut self.scratch.buses,
                &mut self.scratch.master,
            );
        }

        // Process Audio channels.
        for index in 0..MAX_MIXER_CHANNELS {
            if self.channels[MAX_INSTRUMENTS + index].role != ChannelRole::Audio {
                continue;
            }
            self.process_bus_channel_block(index, frames, &duck_dirty);
        }

        // Process Group channels in causal topological order (supports chained groups).
        let group_order = self.group_order;
        for &index in group_order.iter().take(self.group_count) {
            self.process_bus_channel_block(index, frames, &duck_dirty);
        }

        // Process Return buses.
        let sm = self.smoothing;
        let decay = self.meter_decay;
        let hold_frames = self.hold_frames;
        let hold_decay = self.hold_decay;
        for index in 0..MAX_MIXER_CHANNELS {
            let slot = MAX_INSTRUMENTS + index;
            if self.channels[slot].role != ChannelRole::Return {
                continue;
            }
            let scratch = &mut *self.scratch;
            let strip_settled = step_channel_smoothers(
                &mut self.channels[slot],
                sm,
                frames,
                &mut scratch.gain_buf,
                &mut scratch.scale_buf,
                &mut scratch.pan_lr_buf,
                &mut scratch.send_amt_buf,
            );
            let plan = self.channels[slot];
            let input_slice = &scratch.returns[index][..frames];
            scratch.work[..frames].copy_from_slice(input_slice);
            run_inserts_block(
                plan.inserts,
                &mut self.effects,
                &mut scratch.work[..frames],
                hold_frames,
                hold_decay,
                &mut scratch.gr_scratch,
            );

            let enabled = !plan.muted && (!self.wet_only || plan.solo);
            let has_duck = duck_dirty[slot];
            let mut inserts_non_finite = false;
            let meter = &mut self.channels[slot].meter;
            let const_gain = plan.gain;
            let const_scale = plan.scale;
            let const_pan = plan.cached_pan_lr;
            for (i, &inp) in input_slice.iter().enumerate() {
                let processed = scratch.work[i];
                if !processed[0].is_finite() || !processed[1].is_finite() {
                    inserts_non_finite = true;
                }
                let output = if enabled {
                    let duck = if has_duck {
                        scratch.factors[slot][i]
                    } else {
                        1.0
                    };
                    let (g, sc, pan_lr) = if strip_settled {
                        (const_gain, const_scale, const_pan)
                    } else {
                        (
                            scratch.gain_buf[i],
                            scratch.scale_buf[i],
                            scratch.pan_lr_buf[i],
                        )
                    };
                    apply_pan_lr(apply_fader_gain(processed, g, sc, duck), pan_lr)
                } else {
                    [0.0; 2]
                };
                meter.observe(inp, output, false, decay, hold_frames, hold_decay);
                scratch.post_fader[i] = sanitize_frame(output);
            }
            if inserts_non_finite {
                meter.clipped = true;
            }
            if plan.pdc_delay > 0 {
                if let Some(line) = self.pdc_lines.get_mut(index) {
                    for i in 0..frames {
                        let aligned = line.read_integer(plan.pdc_delay);
                        line.write(scratch.post_fader[i]);
                        scratch.master[i][0] += aligned[0];
                        scratch.master[i][1] += aligned[1];
                    }
                } else {
                    for i in 0..frames {
                        scratch.master[i][0] += scratch.post_fader[i][0];
                        scratch.master[i][1] += scratch.post_fader[i][1];
                    }
                }
            } else {
                for i in 0..frames {
                    scratch.master[i][0] += scratch.post_fader[i][0];
                    scratch.master[i][1] += scratch.post_fader[i][1];
                }
            }
        }

        // Process Master bus.
        let scratch = &mut *self.scratch;
        let strip_settled = step_channel_smoothers(
            &mut self.channels[MASTER],
            sm,
            frames,
            &mut scratch.gain_buf,
            &mut scratch.scale_buf,
            &mut scratch.pan_lr_buf,
            &mut scratch.send_amt_buf,
        );
        let master_plan = self.channels[MASTER];
        scratch.work[..frames].copy_from_slice(&scratch.master[..frames]);
        run_inserts_block(
            master_plan.inserts,
            &mut self.effects,
            &mut scratch.work[..frames],
            hold_frames,
            hold_decay,
            &mut scratch.gr_scratch,
        );

        let has_duck = duck_dirty[MASTER];
        let mut master_non_finite = false;
        let meter = &mut self.channels[MASTER].meter;
        let const_gain = master_plan.gain;
        let const_scale = master_plan.scale;
        let const_pan = master_plan.cached_pan_lr;
        let master_muted = master_plan.muted;
        for (i, dst) in out[..frames].iter_mut().enumerate() {
            let processed = scratch.work[i];
            if !processed[0].is_finite() || !processed[1].is_finite() {
                master_non_finite = true;
            }
            let strip_out = if master_muted {
                [0.0; 2]
            } else {
                let duck = if has_duck {
                    scratch.factors[MASTER][i]
                } else {
                    1.0
                };
                let (g, sc, pan_lr) = if strip_settled {
                    (const_gain, const_scale, const_pan)
                } else {
                    (
                        scratch.gain_buf[i],
                        scratch.scale_buf[i],
                        scratch.pan_lr_buf[i],
                    )
                };
                apply_pan_lr(apply_fader_gain(processed, g, sc, duck), pan_lr)
            };
            let prev = preview.get(i).copied().unwrap_or([0.0; 2]);
            let mon = monitor.get(i).copied().unwrap_or([0.0; 2]);
            if !prev[0].is_finite()
                || !prev[1].is_finite()
                || !mon[0].is_finite()
                || !mon[1].is_finite()
            {
                master_non_finite = true;
            }
            // Preview audition and live software input monitoring sum after Master inserts/mute
            // so muting Master never silences them, while Master meters observe the exact final
            // stereo frame sent to DAC/WAV.
            let final_raw = [
                strip_out[0] + sanitize_sample(prev[0]) + sanitize_sample(mon[0]),
                strip_out[1] + sanitize_sample(prev[1]) + sanitize_sample(mon[1]),
            ];
            meter.observe(
                scratch.master[i],
                final_raw,
                true,
                decay,
                hold_frames,
                hold_decay,
            );
            *dst = sanitize_frame(final_raw);
        }
        if master_non_finite {
            meter.clipped = true;
        }
    }

    #[cfg(test)]
    pub(crate) fn process(
        &mut self,
        sources: &[[f32; 2]; MAX_INSTRUMENTS],
        preview: [f32; 2],
        monitor: [f32; 2],
    ) -> [f32; 2] {
        let mut src_block = [[[0.0_f32; 2]; AUDIO_SUB_BLOCK_FRAMES]; MAX_INSTRUMENTS];
        let mut active_mask = 0u32;
        for (i, &frame) in sources.iter().enumerate() {
            if frame[0] != 0.0 || frame[1] != 0.0 || !frame[0].is_finite() || !frame[1].is_finite()
            {
                src_block[i][0] = frame;
                active_mask |= 1u32 << i;
            }
        }
        let mut out = [[0.0_f32; 2]; 1];
        self.process_block(&src_block, active_mask, &[preview], &[monitor], &mut out);
        out[0]
    }
}

#[cfg(test)]
pub(crate) mod tests;
