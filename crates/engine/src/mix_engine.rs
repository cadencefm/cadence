//! Bounded native mixer runtime operating directly on `ScoreWindow`.
//! Prepared storage and fixed routing tables keep processing zero-alloc and
//! independent of frontend, filesystem, and audio-device APIs.
use cadence_core::dsp::{
    apply_fader_gain, limiter_lookahead_frames, one_pole_coeff_ms, one_pole_decay_ms,
    pan_stereo_balance, sanitize_frame, sanitize_sample, smooth_step_f32, Chorus, Compressor,
    Delay, DelayLine, Drive, Eq, Limiter, PeakHold, Reverb, RhythmicFilter, StereoEffect,
    AUDIO_SUB_BLOCK_FRAMES, TAIL_SILENCE_FLOOR_F32,
};
use cadence_core::mixer::{
    ChannelKind, ChannelParam, ChannelRef, EffectKind, EffectKindTag, MixerParamTarget,
    MAX_CHANNELS, MAX_DUCKS, MAX_EFFECT_DEVICES, MAX_INSERTS, MAX_MIXER_CHANNELS, MAX_SENDS,
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

/// Collect `(device_id, EffectKindTag)` pairs for every insert device in `score`
/// (capped at [`MAX_EFFECT_DEVICES`]). Off-RT helper used for effect preparation.
pub(crate) fn score_effect_specs(score: &ScoreWindow) -> Vec<(u64, EffectKindTag)> {
    score
        .mixer
        .channels
        .iter()
        .take(MAX_MIXER_CHANNELS)
        .flat_map(|c| c.inserts.iter())
        .chain(score.mixer.master.inserts.iter())
        .take(MAX_EFFECT_DEVICES)
        .map(|d| (d.id, d.kind.tag()))
        .collect()
}

/// Off-RT helper: construct a [`PreparedEffect`] for every insert device in `score`.
pub(crate) fn prepare_effects_for_score(
    score: &ScoreWindow,
    sample_rate: u32,
) -> Vec<PreparedEffect> {
    score_effect_specs(score)
        .into_iter()
        .map(|(id, tag)| PreparedEffect::new(id, tag, sample_rate))
        .collect()
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
            let next_in = (self.input[channel] * decay).max(before);
            self.input[channel] = if next_in < TAIL_SILENCE_FLOOR_F32 {
                0.0
            } else {
                next_in
            };
            let next_out = (self.output[channel] * decay).max(after);
            self.output[channel] = if next_out < TAIL_SILENCE_FLOOR_F32 {
                0.0
            } else {
                next_out
            };

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

    #[inline]
    fn is_settled_zero(&self) -> bool {
        self.input[0] == 0.0
            && self.input[1] == 0.0
            && self.output[0] == 0.0
            && self.output[1] == 0.0
            && self.input_hold[0].value() == 0.0
            && self.input_hold[1].value() == 0.0
            && self.output_hold[0].value() == 0.0
            && self.output_hold[1].value() == 0.0
    }

    #[inline]
    fn observe_silent_block(
        &mut self,
        frames: usize,
        decay: f32,
        hold_frames: u32,
        hold_decay: f32,
    ) {
        if self.is_settled_zero() {
            return;
        }
        for _ in 0..frames {
            for channel in 0..2 {
                let next_in = self.input[channel] * decay;
                self.input[channel] = if next_in < TAIL_SILENCE_FLOOR_F32 {
                    0.0
                } else {
                    next_in
                };
                let next_out = self.output[channel] * decay;
                self.output[channel] = if next_out < TAIL_SILENCE_FLOOR_F32 {
                    0.0
                } else {
                    next_out
                };
                self.input_hold[channel].observe(
                    0.0,
                    self.input[channel],
                    hold_frames,
                    hold_decay,
                    PeakHold::DEFAULT_ZERO_FLOOR,
                );
                self.output_hold[channel].observe(
                    0.0,
                    self.output[channel],
                    hold_frames,
                    hold_decay,
                    PeakHold::DEFAULT_ZERO_FLOOR,
                );
            }
            if self.is_settled_zero() {
                break;
            }
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
    target_channel_id: u64,
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
    max_pdc_delay: usize,
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
            max_pdc_delay: 0,
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
    fn has_smoothers_unsettled(&self) -> bool {
        if self.gain != self.target_gain
            || self.pan != self.target_pan
            || self.scale != self.target_scale
        {
            return true;
        }
        for send in &self.sends {
            if send.target.is_none() {
                break;
            }
            if send.amount != send.target_amount {
                return true;
            }
        }
        false
    }

    #[inline]
    fn inserts_quiet(&self, effects: &[EffectSlot]) -> bool {
        for &slot in self.inserts.iter().flatten() {
            if let Some(effect) = effects.get(usize::from(slot)) {
                if !effect.is_quiet() {
                    return false;
                }
            }
        }
        true
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

macro_rules! dispatch_active_effect {
    (&$eff:expr, $method:ident ( $($arg:expr),* )) => {
        match &$eff {
            ActiveEffect::Delay(fx) => StereoEffect::$method(fx.as_ref(), $($arg),*),
            ActiveEffect::Reverb(fx) => StereoEffect::$method(fx.as_ref(), $($arg),*),
            ActiveEffect::Eq(fx) => StereoEffect::$method(fx.as_ref(), $($arg),*),
            ActiveEffect::Chorus(fx) => StereoEffect::$method(fx.as_ref(), $($arg),*),
            ActiveEffect::Drive(fx) => StereoEffect::$method(fx.as_ref(), $($arg),*),
            ActiveEffect::Filter(fx) => StereoEffect::$method(fx.as_ref(), $($arg),*),
            ActiveEffect::Compressor(fx) => StereoEffect::$method(fx.as_ref(), $($arg),*),
            ActiveEffect::Limiter(fx) => StereoEffect::$method(fx.as_ref(), $($arg),*),
        }
    };
    (&mut $eff:expr, $method:ident ( $($arg:expr),* )) => {
        match &mut $eff {
            ActiveEffect::Delay(fx) => StereoEffect::$method(fx.as_mut(), $($arg),*),
            ActiveEffect::Reverb(fx) => StereoEffect::$method(fx.as_mut(), $($arg),*),
            ActiveEffect::Eq(fx) => StereoEffect::$method(fx.as_mut(), $($arg),*),
            ActiveEffect::Chorus(fx) => StereoEffect::$method(fx.as_mut(), $($arg),*),
            ActiveEffect::Drive(fx) => StereoEffect::$method(fx.as_mut(), $($arg),*),
            ActiveEffect::Filter(fx) => StereoEffect::$method(fx.as_mut(), $($arg),*),
            ActiveEffect::Compressor(fx) => StereoEffect::$method(fx.as_mut(), $($arg),*),
            ActiveEffect::Limiter(fx) => StereoEffect::$method(fx.as_mut(), $($arg),*),
        }
    };
}

/// Heap-boxed active stereo insert processor owned by one [`EffectSlot`].
///
/// Unused mixer slots hold `None` (`0` bytes of idle effect buffers across all
/// `MAX_EFFECT_DEVICES` slots). Live additions and kind swaps construct new
/// instances off the real-time thread as [`PreparedEffect`] and retire replaced
/// instances into a pre-sized queue dropped off-RT, guaranteeing strictly zero
/// RT allocations and zero RT drops.
pub(crate) enum ActiveEffect {
    Delay(Box<Delay>),
    Reverb(Box<Reverb>),
    Eq(Box<Eq>),
    Chorus(Box<Chorus>),
    Drive(Box<Drive>),
    Filter(Box<RhythmicFilter>),
    Compressor(Box<Compressor>),
    Limiter(Box<Limiter>),
}

impl ActiveEffect {
    /// Off-RT constructor for a single boxed effect instance at `sample_rate` Hz.
    pub(crate) fn new(tag: EffectKindTag, sample_rate: u32) -> Self {
        let sr = f64::from(sample_rate);
        match tag {
            EffectKindTag::Delay => Self::Delay(Box::new(Delay::new(sample_rate))),
            EffectKindTag::Reverb => Self::Reverb(Box::new(Reverb::new(sample_rate))),
            EffectKindTag::Eq => Self::Eq(Box::new(Eq::new(sr))),
            EffectKindTag::Chorus => Self::Chorus(Box::new(Chorus::new(sr))),
            EffectKindTag::Drive => Self::Drive(Box::new(Drive::new(sr))),
            EffectKindTag::Filter => Self::Filter(Box::new(RhythmicFilter::new(sr))),
            EffectKindTag::Compressor => Self::Compressor(Box::new(Compressor::new(sr))),
            EffectKindTag::Limiter => Self::Limiter(Box::new(Limiter::new(sr))),
        }
    }

    #[inline]
    fn tag(&self) -> EffectKindTag {
        match self {
            Self::Delay(_) => EffectKindTag::Delay,
            Self::Reverb(_) => EffectKindTag::Reverb,
            Self::Eq(_) => EffectKindTag::Eq,
            Self::Chorus(_) => EffectKindTag::Chorus,
            Self::Drive(_) => EffectKindTag::Drive,
            Self::Filter(_) => EffectKindTag::Filter,
            Self::Compressor(_) => EffectKindTag::Compressor,
            Self::Limiter(_) => EffectKindTag::Limiter,
        }
    }

    #[inline]
    fn reset(&mut self) {
        dispatch_active_effect!(&mut *self, reset());
    }

    #[inline]
    fn process_block_with_gr(
        &mut self,
        buf: &mut [[f32; 2]],
        bypassed: bool,
        gr_out: &mut [f32],
    ) -> bool {
        dispatch_active_effect!(&mut *self, process_block_with_gr(buf, bypassed, gr_out))
    }

    #[inline]
    fn latency_frames(&self) -> usize {
        dispatch_active_effect!(&*self, latency_frames())
    }

    #[inline]
    fn gain_reduction_db(&self) -> Option<f32> {
        dispatch_active_effect!(&*self, gain_reduction_db())
    }

    #[inline]
    fn is_quiet(&self, bypassed: bool) -> bool {
        dispatch_active_effect!(&*self, is_quiet(bypassed))
    }

    #[inline]
    fn advance_silent_frames(&mut self, frames: usize, bypassed: bool) {
        dispatch_active_effect!(&mut *self, advance_silent_frames(frames, bypassed));
    }

    fn set_parameters(&mut self, id: u64, kind: EffectKind, score: &ScoreWindow, clock_pos: f64) {
        match (self, kind) {
            (Self::Delay(fx), EffectKind::Delay(patch)) => fx.set_parameters(patch, score.bpm),
            (Self::Reverb(fx), EffectKind::Reverb(patch)) => fx.set_parameters(patch),
            (Self::Eq(fx), EffectKind::Eq(patch)) => fx.set_parameters(patch),
            (Self::Chorus(fx), EffectKind::Chorus(patch)) => fx.set_parameters(patch),
            (Self::Drive(fx), EffectKind::Drive(patch)) => fx.set_parameters(patch),
            (Self::Filter(fx), EffectKind::Filter(patch)) => {
                let compiled = patch
                    .env_pattern_id
                    .and_then(|_| score.mixer.filter_env_trigger(id))
                    .unwrap_or_else(|| score.mixer.resolve_filter_groove(patch.env_pattern));
                fx.set_parameters_with_clock(patch, compiled, clock_pos, score.bpm);
            }
            (Self::Compressor(fx), EffectKind::Compressor(patch)) => fx.set_parameters(patch),
            (Self::Limiter(fx), EffectKind::Limiter(patch)) => fx.set_parameters(patch),
            _ => {}
        }
    }
}

/// Control-thread pre-allocated effect awaiting zero-alloc/zero-drop RT handoff
/// in [`MixerEngine::adopt_prepared`].
pub(crate) struct PreparedEffect {
    pub(crate) id: u64,
    pub(crate) tag: EffectKindTag,
    pub(crate) effect: Option<ActiveEffect>,
}

impl PreparedEffect {
    pub(crate) fn new(id: u64, tag: EffectKindTag, sample_rate: u32) -> Self {
        Self {
            id,
            tag,
            effect: Some(ActiveEffect::new(tag, sample_rate)),
        }
    }
}

struct EffectSlot {
    id: Option<u64>,
    effect: Option<ActiveEffect>,
    bypassed: bool,
    gr_hold: PeakHold,
    render_gr_max_db: f32,
    render_gr_sum_db: f64,
    render_gr_frames: u64,
}

impl EffectSlot {
    fn empty() -> Self {
        Self {
            id: None,
            effect: None,
            bypassed: false,
            gr_hold: PeakHold::new(),
            render_gr_max_db: 0.0,
            render_gr_sum_db: 0.0,
            render_gr_frames: 0,
        }
    }

    #[inline]
    fn tag(&self) -> Option<EffectKindTag> {
        self.effect.as_ref().map(ActiveEffect::tag)
    }

    fn reset(&mut self) {
        if let Some(effect) = self.effect.as_mut() {
            effect.reset();
        }
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
            self.effect.as_ref().map_or(0, ActiveEffect::latency_frames)
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
        let Some(effect) = self.effect.as_mut() else {
            return;
        };
        let n = buf.len();
        let has_gr = effect.process_block_with_gr(buf, self.bypassed, &mut gr_scratch[..n]);
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
        self.effect
            .as_ref()
            .and_then(ActiveEffect::gain_reduction_db)
            .unwrap_or(0.0)
    }

    #[inline]
    fn gr_hold_db(&self) -> f32 {
        if self.bypassed {
            0.0
        } else {
            self.gr_hold.value()
        }
    }

    #[inline]
    fn is_quiet(&self) -> bool {
        self.id.is_none()
            || self
                .effect
                .as_ref()
                .is_none_or(|fx| fx.is_quiet(self.bypassed))
    }

    #[inline]
    fn observe_silent_block(&mut self, frames: usize, hold_frames: u32, hold_decay: f32) {
        let Some(effect) = self.effect.as_mut() else {
            return;
        };
        if self.id.is_none() {
            return;
        }
        effect.advance_silent_frames(frames, self.bypassed);
        if self.bypassed {
            return;
        }
        if effect.gain_reduction_db().is_some() {
            if self.gr_hold.value() > 0.0 {
                for _ in 0..frames {
                    self.gr_hold
                        .observe(0.0, 0.0, hold_frames, hold_decay, GR_HOLD_ZERO_FLOOR_DB);
                    if self.gr_hold.value() == 0.0 {
                        break;
                    }
                }
            }
            self.render_gr_frames += frames as u64;
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
    pdc_active_frames: [usize; MAX_MIXER_CHANNELS],
    scratch: Box<MixerBlockScratch>,
    active_audio: [usize; MAX_MIXER_CHANNELS],
    active_audio_count: usize,
    active_returns: [usize; MAX_MIXER_CHANNELS],
    active_return_count: usize,
    automated_mixer_slots: [usize; MAX_MIXER_CHANNELS + 1],
    automated_mixer_slot_count: usize,
    group_order: [usize; MAX_MIXER_CHANNELS],
    group_count: usize,
    ducks: [DuckState; MAX_DUCKS],
    active_ducks: [usize; MAX_DUCKS],
    active_duck_count: usize,
    active_effects: [usize; MAX_EFFECT_DEVICES],
    active_effect_count: usize,
    dynamic_effects: [usize; MAX_EFFECT_DEVICES],
    dynamic_effect_count: usize,
    controls_dirty: bool,
    total_output_latency: usize,
    sample_rate: f64,
    envelope_smoothing: f32,
    wet_only: bool,
    smoothing: f32,
    meter_decay: f32,
    hold_frames: u32,
    hold_decay: f32,
    #[cfg(test)]
    disable_sleep: bool,
}

impl MixerEngine {
    pub(crate) fn new(score: &ScoreWindow, sample_rate: u32) -> Box<Self> {
        let sr = f64::from(sample_rate);
        // Accommodate stacked limiters across chained Group → Group → Master paths without truncating PDC.
        let pdc_cap = limiter_lookahead_frames(sr).saturating_mul(MAX_EFFECT_DEVICES);
        let effects = (0..MAX_EFFECT_DEVICES)
            .map(|_| EffectSlot::empty())
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
            pdc_active_frames: [0; MAX_MIXER_CHANNELS],
            scratch: MixerBlockScratch::new_boxed(),
            active_audio: [0; MAX_MIXER_CHANNELS],
            active_audio_count: 0,
            active_returns: [0; MAX_MIXER_CHANNELS],
            active_return_count: 0,
            automated_mixer_slots: [0; MAX_MIXER_CHANNELS + 1],
            automated_mixer_slot_count: 0,
            group_order: [0; MAX_MIXER_CHANNELS],
            group_count: 0,
            ducks: [DuckState::default(); MAX_DUCKS],
            active_ducks: [0; MAX_DUCKS],
            active_duck_count: 0,
            active_effects: [0; MAX_EFFECT_DEVICES],
            active_effect_count: 0,
            dynamic_effects: [0; MAX_EFFECT_DEVICES],
            dynamic_effect_count: 0,
            controls_dirty: true,
            total_output_latency: 0,
            sample_rate: sr,
            envelope_smoothing: one_pole_coeff_ms(1.0, sr) as f32,
            wet_only: false,
            smoothing: one_pole_coeff_ms(10.0, sr) as f32,
            meter_decay: one_pole_decay_ms(300.0, sr) as f32,
            hold_frames: (1.5 * sr).round().max(1.0) as u32,
            hold_decay: one_pole_decay_ms(300.0, sr) as f32,
            #[cfg(test)]
            disable_sleep: false,
        });
        let mut prepared = prepare_effects_for_score(score, sample_rate);
        let mut retired = Vec::new();
        mixer.adopt_prepared(score, true, &mut prepared, &mut retired);
        mixer
    }

    /// Total cumulative output Plugin Delay Compensation (PDC) latency in frames
    /// (`max_master_in + master_insert_latency`).
    #[inline]
    pub(crate) fn total_output_latency_frames(&self) -> usize {
        self.total_output_latency
    }

    #[cfg(test)]
    #[inline]
    pub(crate) fn set_disable_sleep(&mut self, disable: bool) {
        self.disable_sleep = disable;
    }

    #[inline]
    fn sleep_enabled(&self) -> bool {
        #[cfg(test)]
        {
            !self.disable_sleep
        }
        #[cfg(not(test))]
        {
            true
        }
    }

    #[inline]
    fn channel_inserts_quiet(&self, slot: usize) -> bool {
        self.sleep_enabled() && self.channels[slot].inserts_quiet(&self.effects)
    }

    /// Current Master decayed stereo output peaks (300 ms release, observing the exact
    /// final summed DAC/WAV output including audition preview and software input monitor).
    #[inline]
    pub(crate) fn master_output_peaks(&self) -> [f32; 2] {
        self.channels[MASTER].meter.output
    }

    /// Number of effect slots that currently own a live boxed [`ActiveEffect`].
    #[cfg(test)]
    pub(crate) fn allocated_effect_count(&self) -> usize {
        self.effects.iter().filter(|s| s.effect.is_some()).count()
    }

    #[cfg(test)]
    pub(crate) fn adopt(&mut self, score: &ScoreWindow, boundary: bool) {
        let evicting = self.effects.iter().any(|slot| {
            slot.id.is_some_and(|id| {
                !score
                    .mixer
                    .channels
                    .iter()
                    .take(MAX_MIXER_CHANNELS)
                    .flat_map(|c| c.inserts.iter())
                    .chain(score.mixer.master.inserts.iter())
                    .any(|d| d.id == id && slot.tag() == Some(d.kind.tag()))
            })
        });
        let mut retired = if evicting {
            Vec::with_capacity(MAX_EFFECT_DEVICES)
        } else {
            Vec::new()
        };
        self.adopt_prepared(score, boundary, &mut [], &mut retired);
    }

    /// Zero-alloc, zero-drop RT adoption of a new `ScoreWindow`: keeps unchanged
    /// live effects in place, moves newly prepared [`ActiveEffect`] instances out of
    /// `prepared`, and pushes evicted instances into `retired` (pre-sized off-RT to
    /// `>= MAX_EFFECT_DEVICES`) for off-RT deallocation.
    pub(crate) fn adopt_prepared(
        &mut self,
        score: &ScoreWindow,
        boundary: bool,
        prepared: &mut [PreparedEffect],
        retired: &mut Vec<ActiveEffect>,
    ) {
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

        // Pass 1: keep live matching `(id, tag)` slots unless a freshly prepared
        // replacement is provided (resetting on boundary), and retire removed,
        // re-typed, or freshly replaced slots.
        for slot in self.effects.iter_mut() {
            let Some(id) = slot.id else {
                continue;
            };
            let matches_wanted = wanted[..wanted_count]
                .iter()
                .any(|&(wid, wtag)| wid == Some(id) && slot.tag() == Some(wtag));
            let has_fresh_replacement = slot.tag().is_some_and(|tag| {
                prepared
                    .iter()
                    .any(|p| p.id == id && p.tag == tag && p.effect.is_some())
            });
            if matches_wanted && !has_fresh_replacement {
                if boundary {
                    slot.reset();
                }
            } else {
                if let Some(old) = slot.effect.take() {
                    debug_assert!(
                        retired.len() < retired.capacity(),
                        "retired_effects must be pre-sized off-RT to >= MAX_EFFECT_DEVICES"
                    );
                    retired.push(old);
                }
                slot.id = None;
                slot.bypassed = false;
                slot.clear_gr_meters();
            }
        }

        // Pass 2: install newly added, kind-swapped, or freshly replaced effects from `prepared`.
        for &(id_opt, tag) in &wanted[..wanted_count] {
            let Some(id) = id_opt else {
                continue;
            };
            if self.effects.iter().any(|slot| slot.id == Some(id)) {
                continue;
            }
            let Some(prep) = prepared
                .iter_mut()
                .find(|p| p.id == id && p.tag == tag && p.effect.is_some())
            else {
                continue;
            };
            let Some(idx) = self.effects.iter().position(|s| s.id.is_none()) else {
                break;
            };
            let slot = &mut self.effects[idx];
            slot.id = Some(id);
            slot.effect = prep.effect.take();
            slot.bypassed = false;
            slot.clear_gr_meters();
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
            let mut max_pdc = self.channels[c_slot].pdc_delay;
            for send in &self.channels[c_slot].sends {
                if send.target.is_some() {
                    max_pdc = max_pdc.max(send.pdc_delay);
                }
            }
            self.channels[c_slot].max_pdc_delay = max_pdc;
            let slot_changed = previous[c_slot].channel != self.channels[c_slot].channel;
            if boundary || slot_changed || max_pdc == 0 {
                self.pdc_lines[c_idx].reset();
                self.pdc_active_frames[c_idx] = 0;
            } else if max_pdc > previous[c_slot].max_pdc_delay {
                // Conservatively drain existing PDC history when delay grows without a reset.
                self.pdc_active_frames[c_idx] = self.pdc_active_frames[c_idx].max(max_pdc);
            }
        }
        for (c_idx, line) in self.pdc_lines.iter_mut().enumerate().skip(chan_count) {
            line.reset();
            self.pdc_active_frames[c_idx] = 0;
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
        self.active_duck_count = 0;
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
            self.active_ducks[self.active_duck_count] = index;
            self.active_duck_count += 1;
        }

        // Pre-build dense active channel, return, automation, and effect index lists so
        // real-time processing and control updates never scan unused fixed-capacity slots.
        self.active_audio_count = 0;
        self.active_return_count = 0;
        self.automated_mixer_slot_count = 0;
        for index in 0..chan_count {
            let slot = MAX_INSTRUMENTS + index;
            match self.channels[slot].role {
                ChannelRole::Audio => {
                    self.active_audio[self.active_audio_count] = index;
                    self.active_audio_count += 1;
                }
                ChannelRole::Return => {
                    self.active_returns[self.active_return_count] = index;
                    self.active_return_count += 1;
                }
                _ => {}
            }
            if Self::channel_has_automation(&self.channels[slot], score) {
                self.automated_mixer_slots[self.automated_mixer_slot_count] = slot;
                self.automated_mixer_slot_count += 1;
            }
        }
        if Self::channel_has_automation(&self.channels[MASTER], score) {
            self.automated_mixer_slots[self.automated_mixer_slot_count] = MASTER;
            self.automated_mixer_slot_count += 1;
        }

        self.active_effect_count = 0;
        self.dynamic_effect_count = 0;
        for (slot_idx, slot) in self.effects.iter().enumerate() {
            let Some(id) = slot.id else {
                continue;
            };
            self.active_effects[self.active_effect_count] = slot_idx;
            self.active_effect_count += 1;
            let is_dynamic = score.find_device(id).is_some_and(|(_, device)| {
                matches!(device.kind, EffectKind::Filter(_))
                    || device.params().iter().any(|&param| {
                        score.has_mixer_automation(MixerParamTarget::Device {
                            device_id: id,
                            param,
                        })
                    })
            });
            if is_dynamic {
                self.dynamic_effects[self.dynamic_effect_count] = slot_idx;
                self.dynamic_effect_count += 1;
            }
        }
        self.controls_dirty = true;

        // Ignore muted returns when deciding whether soloed returns isolate wet-only monitoring.
        self.wet_only = visible_channels.iter().any(|c| {
            let cref = ChannelRef::Channel(c.id);
            c.kind == ChannelKind::Return && flags.channel_solo(cref) && !flags.channel_muted(cref)
        });
    }

    fn channel_has_automation(plan: &ChannelPlan, score: &ScoreWindow) -> bool {
        let Some(channel) = plan.channel else {
            return false;
        };
        if score.has_mixer_automation(MixerParamTarget::Channel {
            channel,
            param: ChannelParam::Gain,
        }) || score.has_mixer_automation(MixerParamTarget::Channel {
            channel,
            param: ChannelParam::Pan,
        }) || score.has_mixer_automation(MixerParamTarget::ChannelGainScale { channel })
        {
            return true;
        }
        for send in &plan.sends {
            if send.target.is_none() {
                break;
            }
            if score.has_mixer_automation(MixerParamTarget::Send {
                channel,
                target: send.target_channel_id,
            }) {
                return true;
            }
        }
        false
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
                    target_channel_id: send.target,
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
        let auto_slots = self.automated_mixer_slots;
        for &slot in &auto_slots[..self.automated_mixer_slot_count] {
            let plan = &mut self.channels[slot];
            let Some(channel) = plan.channel else {
                continue;
            };
            let gain_target = MixerParamTarget::Channel {
                channel,
                param: ChannelParam::Gain,
            };
            if score.has_mixer_automation(gain_target) {
                plan.target_gain = ChannelParam::Gain
                    .denormalize(score.resolve_mixer_param(gain_target, auto_pos));
            }
            let pan_target = MixerParamTarget::Channel {
                channel,
                param: ChannelParam::Pan,
            };
            if score.has_mixer_automation(pan_target) {
                plan.target_pan =
                    ChannelParam::Pan.denormalize(score.resolve_mixer_param(pan_target, auto_pos));
            }
            let scale_target = MixerParamTarget::ChannelGainScale { channel };
            if score.has_mixer_automation(scale_target) {
                plan.target_scale = score.resolve_mixer_param(scale_target, auto_pos);
            }
            for send in &mut plan.sends {
                if send.target.is_none() {
                    break;
                }
                let send_target = MixerParamTarget::Send {
                    channel,
                    target: send.target_channel_id,
                };
                if score.has_mixer_automation(send_target) {
                    send.target_amount = score.resolve_mixer_param(send_target, auto_pos);
                }
            }
        }

        let (effect_indices, effect_count) = if self.controls_dirty {
            self.controls_dirty = false;
            (self.active_effects, self.active_effect_count)
        } else {
            (self.dynamic_effects, self.dynamic_effect_count)
        };
        for &slot_idx in &effect_indices[..effect_count] {
            let slot = &mut self.effects[slot_idx];
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
            if let Some(effect) = slot.effect.as_mut() {
                effect.set_parameters(id, kind, score, clock_pos);
            }
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
        self.pdc_active_frames.fill(0);
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
        self.active_effects[..self.active_effect_count]
            .iter()
            .filter_map(|&idx| {
                let slot = &self.effects[idx];
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
        self.active_effects[..self.active_effect_count]
            .iter()
            .filter_map(|&idx| {
                let slot = &self.effects[idx];
                let id = slot.id?;
                slot.effect.as_ref()?.gain_reduction_db()?;
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

#[inline]
fn slice_has_audio(buf: &[[f32; 2]]) -> bool {
    buf.iter().any(|&[l, r]| l != 0.0 || r != 0.0)
}

/// Advance unsettled strip & send smoothers in place for a silent/sleeping channel
/// without writing per-frame scratch buffers.
#[inline]
fn step_channel_smoothers_silent(channel: &mut ChannelPlan, sm: f32, frames: usize) {
    if !channel.has_smoothers_unsettled() {
        return;
    }
    if channel.gain != channel.target_gain
        || channel.pan != channel.target_pan
        || channel.scale != channel.target_scale
    {
        for _ in 0..frames {
            smooth_step_f32(&mut channel.gain, channel.target_gain, sm);
            smooth_step_f32(&mut channel.pan, channel.target_pan, sm);
            smooth_step_f32(&mut channel.scale, channel.target_scale, sm);
        }
        channel.refresh_pan_cache();
    }
    for send in &mut channel.sends {
        if send.target.is_none() {
            break;
        }
        if send.amount != send.target_amount {
            for _ in 0..frames {
                smooth_step_f32(&mut send.amount, send.target_amount, sm);
            }
        }
    }
}

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
    bus_active: &mut [bool; MAX_MIXER_CHANNELS],
    master: &mut [[f32; 2]; AUDIO_SUB_BLOCK_FRAMES],
    master_active: &mut bool,
) {
    let n = output.len();
    match plan.output_slot() {
        Some(MASTER) => {
            if !wet_only {
                if !*master_active {
                    master[..n].copy_from_slice(output);
                    *master_active = true;
                } else {
                    for (dst, &src) in master[..n].iter_mut().zip(output) {
                        dst[0] += src[0];
                        dst[1] += src[1];
                    }
                }
            }
        }
        Some(slot) if (MAX_INSTRUMENTS..MASTER).contains(&slot) => {
            let idx = slot - MAX_INSTRUMENTS;
            if idx < MAX_MIXER_CHANNELS {
                if !bus_active[idx] {
                    buses[idx][..n].copy_from_slice(output);
                    bus_active[idx] = true;
                } else {
                    for (dst, &src) in buses[idx][..n].iter_mut().zip(output) {
                        dst[0] += src[0];
                        dst[1] += src[1];
                    }
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
fn observe_silent_inserts(
    inserts: [Option<u8>; MAX_INSERTS],
    effects: &mut [EffectSlot],
    frames: usize,
    hold_frames: u32,
    hold_decay: f32,
) {
    for &slot in inserts.iter().flatten() {
        if let Some(effect) = effects.get_mut(usize::from(slot)) {
            effect.observe_silent_block(frames, hold_frames, hold_decay);
        }
    }
}

#[allow(clippy::too_many_arguments)]
#[inline]
fn process_channel_inserts_block(
    inserts: [Option<u8>; MAX_INSERTS],
    effects: &mut [EffectSlot],
    input: Option<&[[f32; 2]]>,
    inserts_quiet: bool,
    work: &mut [[f32; 2]],
    hold_frames: u32,
    hold_decay: f32,
    gr_scratch: &mut [f32; AUDIO_SUB_BLOCK_FRAMES],
) {
    if let Some(src) = input {
        work.copy_from_slice(src);
        run_inserts_block(inserts, effects, work, hold_frames, hold_decay, gr_scratch);
    } else if !inserts_quiet {
        work.fill([0.0; 2]);
        run_inserts_block(inserts, effects, work, hold_frames, hold_decay, gr_scratch);
    } else {
        work.fill([0.0; 2]);
        observe_silent_inserts(inserts, effects, work.len(), hold_frames, hold_decay);
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
        let active_ducks = self.active_ducks;
        for &duck_idx in &active_ducks[..self.active_duck_count] {
            let duck = &mut self.ducks[duck_idx];
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

    #[allow(clippy::too_many_arguments)]
    fn process_bus_channel_block(
        &mut self,
        index: usize,
        frames: usize,
        duck_dirty: &[bool; MAX_CHANNELS],
        bus_active: &mut [bool; MAX_MIXER_CHANNELS],
        return_active: &mut [bool; MAX_MIXER_CHANNELS],
        master_active: &mut bool,
    ) {
        let slot = MAX_INSTRUMENTS + index;
        let sm = self.smoothing;
        let decay = self.meter_decay;
        let hold_frames = self.hold_frames;
        let hold_decay = self.hold_decay;
        let wet_only = self.wet_only;

        let has_input = bus_active[index];
        let sleep_enabled = self.sleep_enabled();
        let inserts_quiet = self.channel_inserts_quiet(slot);
        let was_pdc_active = self.pdc_active_frames[index] > 0;
        let source_enabled = self.channels[slot].source_enabled;

        // Fast sleep: completely silent input, drained inserts, and drained PDC line.
        if !has_input && inserts_quiet && !was_pdc_active {
            step_channel_smoothers_silent(&mut self.channels[slot], sm, frames);
            let inserts = self.channels[slot].inserts;
            observe_silent_inserts(inserts, &mut self.effects, frames, hold_frames, hold_decay);
            self.channels[slot]
                .meter
                .observe_silent_block(frames, decay, hold_frames, hold_decay);
            return;
        }

        let scratch = &mut *self.scratch;
        // Muted / non-soloed channel: keep pre-insert input metering live, drain inserts
        // with silence until quiet, then skip insert DSP cleanly.
        if !source_enabled {
            let meter = &mut self.channels[slot].meter;
            if has_input {
                for &inp in &scratch.buses[index][..frames] {
                    meter.observe(inp, [0.0; 2], false, decay, hold_frames, hold_decay);
                }
            } else {
                meter.observe_silent_block(frames, decay, hold_frames, hold_decay);
            }
            let inserts = self.channels[slot].inserts;
            if inserts_quiet {
                observe_silent_inserts(inserts, &mut self.effects, frames, hold_frames, hold_decay);
            } else {
                scratch.work[..frames].fill([0.0; 2]);
                run_inserts_block(
                    inserts,
                    &mut self.effects,
                    &mut scratch.work[..frames],
                    hold_frames,
                    hold_decay,
                    &mut scratch.gr_scratch,
                );
            }
            if !was_pdc_active {
                step_channel_smoothers_silent(&mut self.channels[slot], sm, frames);
                return;
            }
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
            let const_pan = plan.cached_pan_lr;
            self.pdc_active_frames[index] = self.pdc_active_frames[index].saturating_sub(frames);
            if let Some(line) = self.pdc_lines.get_mut(index) {
                for i in 0..frames {
                    for (s_idx, send) in plan.sends.iter().enumerate() {
                        let Some(target) = send.target.map(usize::from) else {
                            break;
                        };
                        let amt = scratch.send_amt_buf[s_idx][i];
                        if target < MAX_MIXER_CHANNELS && amt > 0.0 && send.pdc_delay > 0 {
                            let tapped = line.read_integer(send.pdc_delay);
                            if tapped[0] != 0.0 || tapped[1] != 0.0 {
                                if !return_active[target] {
                                    scratch.returns[target][..frames].fill([0.0; 2]);
                                    return_active[target] = true;
                                }
                                scratch.returns[target][i][0] += tapped[0] * amt;
                                scratch.returns[target][i][1] += tapped[1] * amt;
                            }
                        }
                    }
                    let main_pf = if plan.pdc_delay == 0 {
                        [0.0; 2]
                    } else {
                        line.read_integer(plan.pdc_delay)
                    };
                    line.write([0.0; 2]);
                    let pan_lr = if strip_settled {
                        const_pan
                    } else {
                        scratch.pan_lr_buf[i]
                    };
                    scratch.work[i] = apply_pan_lr(main_pf, pan_lr);
                }
                if self.pdc_active_frames[index] == 0 {
                    line.reset();
                }
                if slice_has_audio(&scratch.work[..frames]) {
                    route_output_block(
                        wet_only,
                        plan,
                        &scratch.work[..frames],
                        &mut scratch.buses,
                        bus_active,
                        &mut scratch.master,
                        master_active,
                    );
                }
            }
            return;
        }

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
        process_channel_inserts_block(
            plan.inserts,
            &mut self.effects,
            has_input.then_some(&scratch.buses[index][..frames]),
            inserts_quiet,
            &mut scratch.work[..frames],
            hold_frames,
            hold_decay,
            &mut scratch.gr_scratch,
        );

        let has_duck = duck_dirty[slot];
        let mut inserts_non_finite = false;
        let mut post_has_audio = false;
        let meter = &mut self.channels[slot].meter;
        let const_gain = plan.gain;
        let const_scale = plan.scale;
        let const_pan = plan.cached_pan_lr;
        for i in 0..frames {
            let inp = if has_input {
                scratch.buses[index][i]
            } else {
                [0.0; 2]
            };
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
            let safe_pf = sanitize_frame(pf);
            if safe_pf[0] != 0.0 || safe_pf[1] != 0.0 {
                post_has_audio = true;
            }
            scratch.post_fader[i] = safe_pf;
            meter.observe(inp, out, false, decay, hold_frames, hold_decay);
        }
        if inserts_non_finite {
            meter.clipped = true;
        }

        // Reuse the channel's single PDC delay line (holding sanitized post-fader, pre-pan audio)
        // to tap both Return-aligned sends and main output before writing once per frame.
        let needs_pdc_write = plan.max_pdc_delay > 0;
        if needs_pdc_write {
            if !post_has_audio && !was_pdc_active && sleep_enabled {
                return;
            }
            if post_has_audio {
                self.pdc_active_frames[index] = plan.max_pdc_delay;
            } else {
                self.pdc_active_frames[index] =
                    self.pdc_active_frames[index].saturating_sub(frames);
            }
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
                            if tapped[0] != 0.0 || tapped[1] != 0.0 {
                                if !return_active[target] {
                                    scratch.returns[target][..frames].fill([0.0; 2]);
                                    return_active[target] = true;
                                }
                                scratch.returns[target][i][0] += tapped[0] * amt;
                                scratch.returns[target][i][1] += tapped[1] * amt;
                            }
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
                if self.pdc_active_frames[index] == 0 && !post_has_audio {
                    line.reset();
                }
                if slice_has_audio(&scratch.work[..frames]) {
                    route_output_block(
                        wet_only,
                        plan,
                        &scratch.work[..frames],
                        &mut scratch.buses,
                        bus_active,
                        &mut scratch.master,
                        master_active,
                    );
                }
                return;
            }
        }

        if !post_has_audio {
            return;
        }
        for (s_idx, send) in plan.sends.iter().enumerate() {
            let Some(target) = send.target.map(usize::from) else {
                break;
            };
            if target < MAX_MIXER_CHANNELS {
                let any_amt = scratch.send_amt_buf[s_idx][..frames]
                    .iter()
                    .any(|&a| a > 0.0);
                if any_amt {
                    if !return_active[target] {
                        scratch.returns[target][..frames].fill([0.0; 2]);
                        return_active[target] = true;
                    }
                    for i in 0..frames {
                        let amt = scratch.send_amt_buf[s_idx][i];
                        if amt > 0.0 {
                            scratch.returns[target][i][0] += scratch.post_fader[i][0] * amt;
                            scratch.returns[target][i][1] += scratch.post_fader[i][1] * amt;
                        }
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
        route_output_block(
            wet_only,
            plan,
            &scratch.work[..frames],
            &mut scratch.buses,
            bus_active,
            &mut scratch.master,
            master_active,
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
        let active_ducks = self.active_ducks;
        for &duck_idx in &active_ducks[..self.active_duck_count] {
            let duck = &mut self.ducks[duck_idx];
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
            let mut target_factors = target_slot.map(|ts| {
                if !duck_dirty[ts] {
                    self.scratch.factors[ts][..frames].fill(1.0);
                    duck_dirty[ts] = true;
                }
                self.scratch.factors[ts][..frames].iter_mut()
            });
            for _ in 0..frames {
                let x = (duck.elapsed / duck.recovery_frames).clamp(0.0, 1.0) as f32;
                let target = 1.0 - duck.depth * (1.0 - x).powi(2);
                smooth_step_f32(&mut duck.gain, target, env_sm);
                if duck.elapsed > duck.recovery_frames + settle_margin {
                    duck.gain = 1.0;
                }
                duck.elapsed += 1.0;
                if let Some(factor) = target_factors.as_mut().and_then(Iterator::next) {
                    *factor *= duck.gain;
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

        // Lazy bus/return/master activity flags avoid clearing unused scratch buffers
        // and let silent downstream channels skip insert DSP completely.
        let mut bus_active = [false; MAX_MIXER_CHANNELS];
        let mut return_active = [false; MAX_MIXER_CHANNELS];
        let mut master_active = false;

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
            let mut source_has_audio = false;
            let src = &sources[index][..frames];
            for (dst, &raw) in self.scratch.work[..frames].iter_mut().zip(src) {
                if !raw[0].is_finite() || !raw[1].is_finite() {
                    source_non_finite = true;
                }
                let safe = sanitize_frame(raw);
                if safe[0] != 0.0 || safe[1] != 0.0 {
                    source_has_audio = true;
                }
                *dst = safe;
            }
            if source_non_finite {
                if let Some(out_slot) = plan.output_slot().filter(|&s| s < MAX_CHANNELS) {
                    self.channels[out_slot].meter.clipped = true;
                }
            }
            if source_has_audio {
                route_output_block(
                    self.wet_only,
                    plan,
                    &self.scratch.work[..frames],
                    &mut self.scratch.buses,
                    &mut bus_active,
                    &mut self.scratch.master,
                    &mut master_active,
                );
            }
        }

        // Process active Audio channels (dense list precomputed in `adopt`).
        let active_audio = self.active_audio;
        for &index in &active_audio[..self.active_audio_count] {
            self.process_bus_channel_block(
                index,
                frames,
                &duck_dirty,
                &mut bus_active,
                &mut return_active,
                &mut master_active,
            );
        }

        // Process Group channels in causal topological order (supports chained groups).
        let group_order = self.group_order;
        for &index in group_order.iter().take(self.group_count) {
            self.process_bus_channel_block(
                index,
                frames,
                &duck_dirty,
                &mut bus_active,
                &mut return_active,
                &mut master_active,
            );
        }

        // Process active Return buses (dense list precomputed in `adopt`).
        let sm = self.smoothing;
        let decay = self.meter_decay;
        let hold_frames = self.hold_frames;
        let hold_decay = self.hold_decay;
        let active_returns = self.active_returns;
        for &index in &active_returns[..self.active_return_count] {
            let slot = MAX_INSTRUMENTS + index;
            let has_input = return_active[index];
            let sleep_enabled = self.sleep_enabled();
            let inserts_quiet = self.channel_inserts_quiet(slot);
            let was_pdc_active = self.pdc_active_frames[index] > 0;
            let enabled =
                !self.channels[slot].muted && (!self.wet_only || self.channels[slot].solo);

            if !has_input && inserts_quiet && !was_pdc_active {
                step_channel_smoothers_silent(&mut self.channels[slot], sm, frames);
                let inserts = self.channels[slot].inserts;
                observe_silent_inserts(inserts, &mut self.effects, frames, hold_frames, hold_decay);
                self.channels[slot].meter.observe_silent_block(
                    frames,
                    decay,
                    hold_frames,
                    hold_decay,
                );
                continue;
            }

            let scratch = &mut *self.scratch;
            if !enabled {
                step_channel_smoothers_silent(&mut self.channels[slot], sm, frames);
                let meter = &mut self.channels[slot].meter;
                if has_input {
                    for &inp in &scratch.returns[index][..frames] {
                        meter.observe(inp, [0.0; 2], false, decay, hold_frames, hold_decay);
                    }
                } else {
                    meter.observe_silent_block(frames, decay, hold_frames, hold_decay);
                }
                let inserts = self.channels[slot].inserts;
                if inserts_quiet {
                    observe_silent_inserts(
                        inserts,
                        &mut self.effects,
                        frames,
                        hold_frames,
                        hold_decay,
                    );
                } else {
                    scratch.work[..frames].fill([0.0; 2]);
                    run_inserts_block(
                        inserts,
                        &mut self.effects,
                        &mut scratch.work[..frames],
                        hold_frames,
                        hold_decay,
                        &mut scratch.gr_scratch,
                    );
                }
                if was_pdc_active {
                    let pdc_delay = self.channels[slot].pdc_delay;
                    self.pdc_active_frames[index] =
                        self.pdc_active_frames[index].saturating_sub(frames);
                    if let Some(line) = self.pdc_lines.get_mut(index) {
                        for i in 0..frames {
                            let aligned = line.read_integer(pdc_delay);
                            line.write([0.0; 2]);
                            if aligned[0] != 0.0 || aligned[1] != 0.0 {
                                if !master_active {
                                    scratch.master[..frames].fill([0.0; 2]);
                                    master_active = true;
                                }
                                scratch.master[i][0] += aligned[0];
                                scratch.master[i][1] += aligned[1];
                            }
                        }
                        if self.pdc_active_frames[index] == 0 {
                            line.reset();
                        }
                    }
                }
                continue;
            }

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
            process_channel_inserts_block(
                plan.inserts,
                &mut self.effects,
                has_input.then_some(&scratch.returns[index][..frames]),
                inserts_quiet,
                &mut scratch.work[..frames],
                hold_frames,
                hold_decay,
                &mut scratch.gr_scratch,
            );

            let has_duck = duck_dirty[slot];
            let mut inserts_non_finite = false;
            let mut post_has_audio = false;
            let meter = &mut self.channels[slot].meter;
            let const_gain = plan.gain;
            let const_scale = plan.scale;
            let const_pan = plan.cached_pan_lr;
            for i in 0..frames {
                let inp = if has_input {
                    scratch.returns[index][i]
                } else {
                    [0.0; 2]
                };
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
                let output = apply_pan_lr(apply_fader_gain(processed, g, sc, duck), pan_lr);
                meter.observe(inp, output, false, decay, hold_frames, hold_decay);
                let safe = sanitize_frame(output);
                if safe[0] != 0.0 || safe[1] != 0.0 {
                    post_has_audio = true;
                }
                scratch.post_fader[i] = safe;
            }
            if inserts_non_finite {
                meter.clipped = true;
            }
            if plan.pdc_delay > 0 {
                if !post_has_audio && !was_pdc_active && sleep_enabled {
                    continue;
                }
                if post_has_audio {
                    self.pdc_active_frames[index] = plan.pdc_delay;
                } else {
                    self.pdc_active_frames[index] =
                        self.pdc_active_frames[index].saturating_sub(frames);
                }
                if let Some(line) = self.pdc_lines.get_mut(index) {
                    for i in 0..frames {
                        let aligned = line.read_integer(plan.pdc_delay);
                        line.write(scratch.post_fader[i]);
                        if aligned[0] != 0.0 || aligned[1] != 0.0 {
                            if !master_active {
                                scratch.master[..frames].fill([0.0; 2]);
                                master_active = true;
                            }
                            scratch.master[i][0] += aligned[0];
                            scratch.master[i][1] += aligned[1];
                        }
                    }
                    if self.pdc_active_frames[index] == 0 && !post_has_audio {
                        line.reset();
                    }
                    continue;
                }
            }
            if post_has_audio {
                if !master_active {
                    scratch.master[..frames].copy_from_slice(&scratch.post_fader[..frames]);
                    master_active = true;
                } else {
                    for i in 0..frames {
                        scratch.master[i][0] += scratch.post_fader[i][0];
                        scratch.master[i][1] += scratch.post_fader[i][1];
                    }
                }
            }
        }

        // Process Master bus.
        let master_inserts_quiet = self.channel_inserts_quiet(MASTER);
        let has_preview = !preview.is_empty();
        let has_monitor = !monitor.is_empty();
        if !master_active && master_inserts_quiet && !has_preview && !has_monitor {
            step_channel_smoothers_silent(&mut self.channels[MASTER], sm, frames);
            let inserts = self.channels[MASTER].inserts;
            observe_silent_inserts(inserts, &mut self.effects, frames, hold_frames, hold_decay);
            self.channels[MASTER].meter.observe_silent_block(
                frames,
                decay,
                hold_frames,
                hold_decay,
            );
            out[..frames].fill([0.0; 2]);
            return;
        }

        let scratch = &mut *self.scratch;
        let master_muted = self.channels[MASTER].muted;
        if master_muted {
            step_channel_smoothers_silent(&mut self.channels[MASTER], sm, frames);
            let inserts = self.channels[MASTER].inserts;
            if master_inserts_quiet {
                observe_silent_inserts(inserts, &mut self.effects, frames, hold_frames, hold_decay);
            } else {
                scratch.work[..frames].fill([0.0; 2]);
                run_inserts_block(
                    inserts,
                    &mut self.effects,
                    &mut scratch.work[..frames],
                    hold_frames,
                    hold_decay,
                    &mut scratch.gr_scratch,
                );
            }
            let meter = &mut self.channels[MASTER].meter;
            if !master_active && !has_preview && !has_monitor {
                meter.observe_silent_block(frames, decay, hold_frames, hold_decay);
                out[..frames].fill([0.0; 2]);
                return;
            }
            let mut master_non_finite = false;
            for (i, dst) in out[..frames].iter_mut().enumerate() {
                let prev = preview.get(i).copied().unwrap_or([0.0; 2]);
                let mon = monitor.get(i).copied().unwrap_or([0.0; 2]);
                if !prev[0].is_finite()
                    || !prev[1].is_finite()
                    || !mon[0].is_finite()
                    || !mon[1].is_finite()
                {
                    master_non_finite = true;
                }
                let inp = if master_active {
                    scratch.master[i]
                } else {
                    [0.0; 2]
                };
                let final_raw = [
                    sanitize_sample(prev[0]) + sanitize_sample(mon[0]),
                    sanitize_sample(prev[1]) + sanitize_sample(mon[1]),
                ];
                meter.observe(inp, final_raw, true, decay, hold_frames, hold_decay);
                *dst = sanitize_frame(final_raw);
            }
            if master_non_finite {
                meter.clipped = true;
            }
            return;
        }

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
        process_channel_inserts_block(
            master_plan.inserts,
            &mut self.effects,
            master_active.then_some(&scratch.master[..frames]),
            master_inserts_quiet,
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
        for (i, dst) in out[..frames].iter_mut().enumerate() {
            let processed = scratch.work[i];
            if !processed[0].is_finite() || !processed[1].is_finite() {
                master_non_finite = true;
            }
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
            let strip_out = apply_pan_lr(apply_fader_gain(processed, g, sc, duck), pan_lr);
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
            let inp = if master_active {
                scratch.master[i]
            } else {
                [0.0; 2]
            };
            meter.observe(inp, final_raw, true, decay, hold_frames, hold_decay);
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
        let preview_slice: &[[f32; 2]] = if preview[0] != 0.0
            || preview[1] != 0.0
            || !preview[0].is_finite()
            || !preview[1].is_finite()
        {
            std::slice::from_ref(&preview)
        } else {
            &[]
        };
        let monitor_slice: &[[f32; 2]] = if monitor[0] != 0.0
            || monitor[1] != 0.0
            || !monitor[0].is_finite()
            || !monitor[1].is_finite()
        {
            std::slice::from_ref(&monitor)
        } else {
            &[]
        };
        let mut out = [[0.0_f32; 2]; 1];
        self.process_block(
            &src_block,
            active_mask,
            preview_slice,
            monitor_slice,
            &mut out,
        );
        out[0]
    }
}

#[cfg(test)]
pub(crate) mod tests;
