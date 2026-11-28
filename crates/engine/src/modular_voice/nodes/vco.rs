//! Native `1x`-sample-rate cubic and Hardy-space VCO voice node with automatic pitch-aware
//! Nyquist reach guarding, M/S quadrature chirality, unison, and `1x` through-zero linear FM.

use super::{
    resolve_tracked_pitch_hz, tracked_pitch_arg, tracked_pitch_arg_to_hz,
    write_copy_module_normalized, NodeInputs, ProcessCtx, TrackedPitch, TriggerCtx, TriggerState,
    VoiceLocks, VoiceModule,
};
use crate::modular_voice::ModuleParams;
use cadence_core::dsp::{
    mix_seed, phase_wrap, sanitize_clamp_f64, smooth_step_f32, step_vco_bank, unison_gain_comp,
    unison_offsets, LogSmoother, VcoStepParams, XorShift32, MAX_UNISON, NYQUIST_SAFE_RATIO,
    VCO_MAX_REACH,
};
use cadence_core::rack::{
    ModuleKind, ModuleKindTag, ModuleParam, PitchTracking, VcoModule, Waveform,
};

const VCO_MIN_HZ: f64 = 0.01;
const VCO_FALLBACK_HZ: f64 = 440.0;

/// Irregular phase offsets for unison copies `1..MAX_UNISON` so zero-detune
/// unison never cancels the fundamental.
const UNISON_PHASE_OFFSETS: [f64; MAX_UNISON - 1] = [0.071, 0.223, 0.467, 0.823, 0.341, 0.659];

#[derive(Clone, Copy, Debug)]
pub(in crate::modular_voice) struct VcoNode {
    current: VcoModule,
    pub target: VcoModule,
    phases: [f64; MAX_UNISON],
    prev_fm_cycles: f64,
    fm_primed: bool,
    unison_ratios: [f64; MAX_UNISON],
    unison_pan: [[f64; 2]; MAX_UNISON],
    fixed_hz: LogSmoother,
    cached_unison_key: (u8, u32, u32),
    cached_pitch_key: (PitchTracking, u64, u64, u64),
    cached_base_hz: f64,
}

impl VcoNode {
    #[inline]
    pub(super) fn new(params: VcoModule) -> Self {
        let mut node = Self {
            current: params,
            target: params,
            phases: [0.0; MAX_UNISON],
            prev_fm_cycles: 0.0,
            fm_primed: false,
            unison_ratios: [1.0; MAX_UNISON],
            unison_pan: [[1.0; 2]; MAX_UNISON],
            fixed_hz: LogSmoother::new(f64::from(params.fixed_hz), 20.0),
            cached_unison_key: (0, u32::MAX, u32::MAX),
            cached_pitch_key: (PitchTracking::Chromatic, u64::MAX, u64::MAX, u64::MAX),
            cached_base_hz: 440.0,
        };
        node.update_unison();
        node
    }

    fn compute_fresh_phases(vco: VcoModule, ctx: &TriggerCtx) -> [f64; MAX_UNISON] {
        let mut seed = XorShift32::new(mix_seed(ctx.base_seed, u32::from(ctx.mod_id)));
        let mut unison_seed =
            XorShift32::new(ctx.base_seed.wrapping_add(0x69a4_2b17).wrapping_add(
                u32::from(ctx.mod_id).wrapping_mul(cadence_core::dsp::SECONDARY_WEYL_SALT_32),
            ));
        let phase0_deg = phase_wrap(f64::from(vco.phase_deg) / 360.0);
        let base_hz = resolve_tracked_pitch_hz(
            TrackedPitch {
                mode: vco.pitch_tracking,
                fixed_hz: f64::from(vco.fixed_hz),
                note_pitch: ctx.target_note_pitch,
                rack_tune: ctx.target_rack_tune,
                tune_semitones: vco.tune_semitones,
                fine_cents: vco.fine_cents,
                pitch_cv: 0.0,
            },
            VCO_MIN_HZ,
            ctx.sample_rate * NYQUIST_SAFE_RATIO,
            VCO_FALLBACK_HZ,
        );
        let p0 = if ctx.phase_reset {
            phase0_deg
        } else {
            phase_wrap(ctx.clock * base_hz + phase0_deg + seed.next_bipolar_f64() * 0.5)
        };
        let mut phases = [p0; MAX_UNISON];
        for copy in 1..MAX_UNISON {
            let offset = UNISON_PHASE_OFFSETS[copy - 1];
            phases[copy] = if ctx.phase_reset {
                phase_wrap(p0 + offset)
            } else {
                phase_wrap(p0 + offset + unison_seed.next_bipolar_f64() * 0.13)
            };
        }
        phases
    }

    #[inline]
    fn update_unison(&mut self) {
        let count_u8 = self.current.unison_voices.clamp(1, MAX_UNISON as u8);
        let key = (
            count_u8,
            self.current.unison_detune_cents.to_bits(),
            self.current.unison_width.to_bits(),
        );
        if key == self.cached_unison_key {
            return;
        }
        self.cached_unison_key = key;
        let count = usize::from(count_u8);
        let offsets = unison_offsets(
            count,
            self.current.unison_detune_cents,
            self.current.unison_width,
        );
        for (copy, off) in offsets.iter().enumerate().take(count) {
            self.unison_ratios[copy] = off.detune_ratio;
            self.unison_pan[copy] = off.pan;
        }
    }
}

impl VoiceModule for VcoNode {
    #[inline]
    fn tag(&self) -> ModuleKindTag {
        ModuleKindTag::Vco
    }

    #[inline]
    fn target_params(&self) -> ModuleParams {
        ModuleParams::Vco(self.target)
    }

    #[inline]
    fn write_target_normalized(
        &mut self,
        param: ModuleParam,
        norm: f32,
        total_frames: usize,
    ) -> bool {
        write_copy_module_normalized(
            &mut self.target,
            ModuleKind::Vco,
            |k| match k {
                ModuleKind::Vco(m) => Some(m),
                _ => None,
            },
            param,
            norm,
            total_frames,
        )
    }

    #[inline]
    fn sync_target_logs(&mut self) {
        self.fixed_hz.set_target(f64::from(self.target.fixed_hz));
    }

    #[inline]
    fn update_continuous(&mut self, src: &ModuleKind, module_id: u16, locks: VoiceLocks<'_>) {
        let ModuleKind::Vco(s) = src else {
            return;
        };
        if !locks.is_module_locked(module_id, ModuleParam::Reach) {
            self.target.reach = s.reach;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Shape) {
            self.target.shape = s.shape;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Chirality) {
            self.target.chirality = s.chirality;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Tune) {
            self.target.tune_semitones = s.tune_semitones;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Fine) {
            self.target.fine_cents = s.fine_cents;
        }
        if !locks.is_module_locked(module_id, ModuleParam::PulseWidth) {
            self.target.pulse_width = s.pulse_width;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Level) {
            self.target.level = s.level;
        }
        if !locks.is_module_locked(module_id, ModuleParam::FixedHz) {
            self.target.fixed_hz = s.fixed_hz;
        }
        if !locks.is_module_locked(module_id, ModuleParam::UnisonDetune) {
            self.target.unison_detune_cents = s.unison_detune_cents;
        }
        if !locks.is_module_locked(module_id, ModuleParam::UnisonWidth) {
            self.target.unison_width = s.unison_width;
        }
        self.sync_target_logs();
    }

    #[inline]
    fn on_trigger(&mut self, ctx: &TriggerCtx, _state: &mut TriggerState<'_>) {
        let target = self.target;
        if ctx.eff_legato {
            self.current.waveform = target.waveform;
            self.current.phase_deg = target.phase_deg;
            self.current.pitch_tracking = target.pitch_tracking;
            self.current.unison_voices = target.unison_voices;
        } else {
            self.current = target;
        }

        let keep_phase = ctx.eff_retarget && (ctx.eff_legato || !ctx.phase_reset);
        if !ctx.eff_retarget {
            self.phases = Self::compute_fresh_phases(target, ctx);
            self.prev_fm_cycles = 0.0;
            self.fm_primed = false;
            self.fixed_hz = LogSmoother::new(f64::from(target.fixed_hz), 20.0);
        } else {
            self.fixed_hz.set_target(f64::from(target.fixed_hz));
            if !ctx.eff_legato {
                self.fixed_hz.snap_to_target();
            }
            if !keep_phase {
                self.phases = Self::compute_fresh_phases(target, ctx);
                self.prev_fm_cycles = 0.0;
                self.fm_primed = false;
            }
        }
        self.update_unison();
    }

    #[inline]
    fn smooth_step(&mut self, s64: f64) {
        let s32 = s64 as f32;
        smooth_step_f32(&mut self.current.reach, self.target.reach, s32);
        smooth_step_f32(&mut self.current.shape, self.target.shape, s32);
        smooth_step_f32(&mut self.current.chirality, self.target.chirality, s32);
        smooth_step_f32(
            &mut self.current.tune_semitones,
            self.target.tune_semitones,
            s32,
        );
        smooth_step_f32(&mut self.current.fine_cents, self.target.fine_cents, s32);
        smooth_step_f32(&mut self.current.pulse_width, self.target.pulse_width, s32);
        smooth_step_f32(&mut self.current.level, self.target.level, s32);
        smooth_step_f32(
            &mut self.current.unison_detune_cents,
            self.target.unison_detune_cents,
            s32,
        );
        smooth_step_f32(
            &mut self.current.unison_width,
            self.target.unison_width,
            s32,
        );
        self.current.fixed_hz = self.fixed_hz.step(s64) as f32;
    }

    #[inline]
    fn control_tick(&mut self, _sample_rate: f64) {
        self.update_unison();
    }

    #[inline]
    fn process(&mut self, inputs: &NodeInputs, ctx: &mut ProcessCtx<'_>) -> [f64; 2] {
        let vco = self.current;
        let sr = ctx.sample_rate;
        let (pitch_arg, fixed_hz_val) = tracked_pitch_arg(TrackedPitch {
            mode: vco.pitch_tracking,
            fixed_hz: self.fixed_hz.current(),
            note_pitch: ctx.note_pitch,
            rack_tune: ctx.rack_tune,
            tune_semitones: vco.tune_semitones,
            fine_cents: vco.fine_cents,
            pitch_cv: inputs.pitch_cv,
        });
        let pitch_key = (
            vco.pitch_tracking,
            pitch_arg.to_bits(),
            fixed_hz_val.to_bits(),
            sr.to_bits(),
        );
        if pitch_key != self.cached_pitch_key {
            self.cached_pitch_key = pitch_key;
            self.cached_base_hz = tracked_pitch_arg_to_hz(
                vco.pitch_tracking,
                pitch_arg,
                fixed_hz_val,
                VCO_MIN_HZ,
                sr * NYQUIST_SAFE_RATIO,
                VCO_FALLBACK_HZ,
            );
        }
        let base_freq_hz = self.cached_base_hz;
        let (pw, reach) = if vco.waveform == Waveform::MobiusPwm {
            (
                sanitize_clamp_f64(f64::from(vco.pulse_width) + inputs.pw_cv, 0.05, 0.95, 0.5),
                sanitize_clamp_f64(f64::from(vco.reach), 0.0, VCO_MAX_REACH, 0.92),
            )
        } else {
            (
                sanitize_clamp_f64(f64::from(vco.pulse_width), 0.05, 0.95, 0.5),
                sanitize_clamp_f64(
                    f64::from(vco.reach) + inputs.pw_cv,
                    0.0,
                    VCO_MAX_REACH,
                    0.92,
                ),
            )
        };
        let shape = sanitize_clamp_f64(f64::from(vco.shape), 0.0, 1.0, 0.5);
        let chirality = sanitize_clamp_f64(f64::from(vco.chirality), -1.0, 1.0, 0.0);
        let voices = usize::from(vco.unison_voices.clamp(1, MAX_UNISON as u8));
        let level = f64::from(vco.level) * unison_gain_comp(voices);
        let fm_cycles = if self.fm_primed {
            inputs.fm_cycles - self.prev_fm_cycles
        } else {
            self.fm_primed = true;
            0.0
        };
        self.prev_fm_cycles = inputs.fm_cycles;

        step_vco_bank(
            VcoStepParams {
                waveform: vco.waveform,
                base_freq_hz,
                sample_rate: sr,
                reach,
                shape,
                chirality,
                pw,
                fm_cycles,
                level,
                voices,
            },
            &mut self.phases,
            &self.unison_ratios,
            &self.unison_pan,
        )
    }
}
