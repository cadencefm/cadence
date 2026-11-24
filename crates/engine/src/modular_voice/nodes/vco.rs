//! Bandlimited PolyBLEP/PolyBLAMP VCO voice node with unison and 2x oversampled through-zero FM.

use super::{
    write_copy_module_normalized, NodeInputs, ProcessCtx, TriggerCtx, TriggerState, VoiceLocks,
    VoiceModule,
};
use crate::modular_voice::ModuleParams;
use cadence_core::dsp::{
    midi_to_hz, mix_seed, pan_mono_equal_power_f64, phase_wrap, semitones_to_ratio_f64,
    step_vco_bank, step_vco_bank_fm_2x, unison_gain_comp, unison_offsets, Halfband2x, LogSmoother,
    VcoStepParams, XorShift32, MAX_UNISON,
};
use cadence_core::rack::{ModuleKind, ModuleKindTag, ModuleParam, PitchTracking, VcoModule};
use std::f64::consts::SQRT_2;

/// Irregular phase offsets for unison copies `1..MAX_UNISON` so zero-detune
/// unison never cancels the fundamental.
const UNISON_PHASE_OFFSETS: [f64; MAX_UNISON - 1] = [0.071, 0.223, 0.467, 0.823, 0.341, 0.659];

#[derive(Clone, Copy, Debug)]
pub(in crate::modular_voice) struct VcoNode {
    current: VcoModule,
    pub target: VcoModule,
    phases: [f64; MAX_UNISON],
    fm_os: Halfband2x,
    prev_fm_2x: f64,
    unison_ratios: [f64; MAX_UNISON],
    unison_pan: [[f64; 2]; MAX_UNISON],
    fixed_hz: LogSmoother,
    has_fm_in: bool,
}

impl VcoNode {
    #[inline]
    pub(super) fn new(params: VcoModule) -> Self {
        let mut node = Self {
            current: params,
            target: params,
            phases: [0.0; MAX_UNISON],
            fm_os: Halfband2x::new(),
            prev_fm_2x: 0.0,
            unison_ratios: [1.0; MAX_UNISON],
            unison_pan: [[1.0; 2]; MAX_UNISON],
            fixed_hz: LogSmoother::new(f64::from(params.fixed_hz), 20.0),
            has_fm_in: false,
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
        let base_hz = match vco.pitch_tracking {
            PitchTracking::Fixed => {
                f64::from(vco.fixed_hz)
                    * semitones_to_ratio_f64(
                        ctx.target_rack_tune
                            + f64::from(vco.tune_semitones)
                            + f64::from(vco.fine_cents) * 0.01,
                    )
            }
            PitchTracking::Chromatic => midi_to_hz(
                ctx.target_note_pitch
                    + ctx.target_rack_tune
                    + f64::from(vco.tune_semitones)
                    + f64::from(vco.fine_cents) * 0.01,
            ),
        };
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
        let count = usize::from(self.current.unison_voices.clamp(1, MAX_UNISON as u8));
        let offsets = unison_offsets(
            count,
            self.current.unison_detune_cents,
            self.current.unison_width,
        );
        for (copy, off) in offsets.iter().enumerate().take(count) {
            self.unison_ratios[copy] = off.detune_ratio;
            let [pl, pr] = pan_mono_equal_power_f64(f64::from(off.pan_pos));
            // Scale by SQRT_2 so center pan (0.0) equals [1.0, 1.0] matching single-voice mono wires
            // while maintaining exact constant stereo power L² + R² == 2.0 across [-1, 1].
            self.unison_pan[copy] = [pl * SQRT_2, pr * SQRT_2];
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
        self.has_fm_in = ctx.has_fm_in;
        if !ctx.eff_retarget {
            self.phases = Self::compute_fresh_phases(target, ctx);
            self.fm_os = Halfband2x::new();
            self.prev_fm_2x = 0.0;
            self.fixed_hz = LogSmoother::new(f64::from(target.fixed_hz), 20.0);
        } else {
            self.fixed_hz.set_target(f64::from(target.fixed_hz));
            if !ctx.eff_legato {
                self.fixed_hz.snap_to_target();
            }
            if !keep_phase {
                self.phases = Self::compute_fresh_phases(target, ctx);
                self.fm_os.reset();
                self.prev_fm_2x = 0.0;
            }
        }
        self.update_unison();
    }

    #[inline]
    fn smooth_step(&mut self, s64: f64) {
        let s32 = s64 as f32;
        self.current.tune_semitones +=
            (self.target.tune_semitones - self.current.tune_semitones) * s32;
        self.current.fine_cents += (self.target.fine_cents - self.current.fine_cents) * s32;
        self.current.pulse_width += (self.target.pulse_width - self.current.pulse_width) * s32;
        self.current.level += (self.target.level - self.current.level) * s32;
        self.current.unison_detune_cents +=
            (self.target.unison_detune_cents - self.current.unison_detune_cents) * s32;
        self.current.unison_width += (self.target.unison_width - self.current.unison_width) * s32;
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
        let base_hz = match vco.pitch_tracking {
            PitchTracking::Fixed => {
                let semi = ctx.rack_tune
                    + f64::from(vco.tune_semitones)
                    + f64::from(vco.fine_cents) * 0.01
                    + inputs.pitch_cv;
                (self.fixed_hz.current() * semitones_to_ratio_f64(semi.clamp(-96.0, 96.0)))
                    .clamp(0.01, sr * 0.45)
            }
            PitchTracking::Chromatic => midi_to_hz(
                (ctx.note_pitch
                    + ctx.rack_tune
                    + f64::from(vco.tune_semitones)
                    + f64::from(vco.fine_cents) * 0.01
                    + inputs.pitch_cv)
                    .clamp(-64.0, 180.0),
            )
            .clamp(0.01, sr * 0.45),
        };
        let pw = (f64::from(vco.pulse_width) + inputs.pw_cv).clamp(0.02, 0.98);
        let unison_count = usize::from(vco.unison_voices.clamp(1, MAX_UNISON as u8));
        let level = f64::from(vco.level) * unison_gain_comp(unison_count);

        let step_params = VcoStepParams {
            waveform: vco.waveform,
            unison_count,
            base_hz,
            sample_rate: sr,
            pw,
            level,
        };
        if self.has_fm_in {
            step_vco_bank_fm_2x(
                step_params,
                &mut self.fm_os,
                &mut self.phases,
                &mut self.prev_fm_2x,
                &self.unison_ratios,
                &self.unison_pan,
                inputs.fm_cycles,
            )
        } else {
            step_vco_bank(
                step_params,
                &mut self.phases,
                &self.unison_ratios,
                &self.unison_pan,
            )
        }
    }
}
