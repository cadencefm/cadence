//! Pure, composable, zero-allocation, zero-I/O DSP library for Cadence.
//!
//! Serves as the single mathematical source of truth shared by both real-time
//! audio synthesis/mixing (`cadence-engine`) and GPU visualizer curves
//! (`cadence-app::view`), making DSP-vs-UI drift and intra-engine duplication
//! structurally impossible.

pub mod biquad;
pub mod chorus;
pub mod delay;
pub mod dynamics;
pub mod env;
pub mod filter;
pub mod gain;
pub mod lofi;
pub mod math;
pub mod noise;
pub mod osc;
pub mod resample;
pub mod resonator;
pub mod reverb;
pub mod sampler_voice;
pub mod saturate;

pub use biquad::{
    eq_response_db, BiquadCoeffs, DcBlocker, Eq, StereoBiquad, TptOnePoleHpf, TptOnePoleLpf,
};
pub use chorus::{Chorus, QuadratureLfo};
pub use delay::{Delay, DelayLine};
pub use dynamics::{
    limiter_lookahead_frames, soft_knee_gr_db, soft_knee_gr_db_f64, Compressor, Limiter,
    COMPRESSOR_KNEE_DB, LIMITER_KNEE_DB, LIMITER_LOOKAHEAD_SEC,
};
pub use env::{
    eval_adsr_curve, eval_modular_env_curve, AdsrStage, AdsrState, EnvMode, HoldDecayEnv,
    ModularEnv, ModularEnvParams, RhythmicEnv,
};
pub use filter::{
    acid_accent_amount, acid_filter_decay_ms, acid_ladder_transfer, filter_magnitude,
    filter_magnitude_db, filter_mix_magnitude_db, filter_transfer, filter_velocity_mod,
    one_pole_lpf_transfer, svf_transfer, AcidAccentMod, AcidCoeffs, AcidLadder, RhythmicFilter,
    Svf, SvfCoeffs, ACID_ACCENT_THRESHOLD, ACID_FILTER_VEL_MAX,
};
pub use gain::{
    apply_fader_gain, apply_pan_balance, apply_strip_gain, channel_gain_to_norm, format_db_value,
    format_gain_db, norm_to_channel_gain, nudge_channel_gain_db, pan_mono_equal_power,
    pan_mono_equal_power_f64, pan_stereo_balance, pan_stereo_balance_f64, stereo_mid_f32,
    stereo_mid_f64, stereo_ms_decode_f64, stereo_ms_encode_f64, stereo_ms_width_uncorrelated,
    stereo_ms_width_uncorrelated_f64, CHANNEL_GAIN_UNITY_NORM, MAX_CHANNEL_GAIN,
    MAX_CHANNEL_GAIN_DB,
};
pub use lofi::{
    lofi_quantize, lofi_quantize_f64, lofi_transfer_sample, LoFiProcessor, LOFI_BYPASS_BITS,
    LOFI_MAX_BITS, LOFI_MAX_RATE_HZ, LOFI_MIN_BITS, LOFI_MIN_RATE_HZ,
};
pub use math::{
    clamp_freq_hz, control_stride_mask, db_to_lin, db_to_lin_f64, decay_coeff_60db_ms,
    flush_denormal_f32, flush_denormal_f64, glide_coeff_ms, hann_fade_in, hann_fade_out,
    hermite4_f32, hermite4_f64, lin_to_db, lin_to_db_f64, log_to_norm, midi_to_hz, mix_stereo_f32,
    mix_stereo_f64, norm_to_log, octaves_to_ratio_f32, octaves_to_ratio_f64, one_pole_coeff_ms,
    one_pole_decay_ms, sanitize_clamp_f32, sanitize_clamp_f64, sanitize_frame, sanitize_sample,
    sanitize_sample_f64, sanitize_sample_rate, semitones_to_ratio_f32, semitones_to_ratio_f64,
    smooth_step_f32, smooth_step_f64, BypassFader, BypassStep, LogSmoother, PeakHold, Smoother,
    AUDIO_SUB_BLOCK_FRAMES, BYPASS_FADE_MS, BYPASS_SETTLE_EPS, CHOKE_MS, DECAY_LN_60DB,
    FAST_RELEASE_MS, LN_100, NYQUIST_SAFE_RATIO, SMOOTH_SETTLE_EPS_F32, SMOOTH_SETTLE_EPS_F64,
    TAIL_SILENCE_FLOOR_F32, TAIL_SILENCE_FLOOR_F64,
};
pub use noise::{
    mix_seed, NoiseGen, PinkFilter, XorShift32, GOLDEN_RATIO_32, NOISE_REF_SAMPLE_RATE,
    SECONDARY_WEYL_SALT_32,
};
pub use osc::{
    fm_depth_from_amount, ideal_waveform, ideal_waveform_f64, oscillator_sample, phase_wrap,
    poly_blamp, poly_blep, step_vco_bank, step_vco_bank_fm_2x, unison_detune_ratio,
    unison_gain_comp, unison_offsets, UnisonVoiceOffset, VcoStepParams, MAX_UNISON,
};
pub use resample::resample_stereo;
pub use resonator::{
    resonator_mode_descriptors, ModalResonator, ResonatorModeDescriptor, RESONATOR_AUDIBLE_MAX_HZ,
    RESONATOR_MAX_DECAY_MS, RESONATOR_MAX_HZ, RESONATOR_MIN_DECAY_MS, RESONATOR_MIN_HZ,
    RESONATOR_MODES, RESONATOR_REF_SAMPLE_RATE,
};
pub use reverb::{reverb_hf_rt60_envelope, reverb_rt60_envelope, Reverb};
pub use sampler_voice::{sample_hermite_stereo_f64, SamplerCursor, SamplerRegion};
pub use saturate::{
    asymmetric_drive_shape, asymmetric_drive_shape_f64, drive_shape_mode, drive_shape_mode_asym,
    drive_shape_mode_asym_f64, drive_shape_mode_f64, drive_transfer_sample,
    shape_drive_oversampled_2x, shape_drive_oversampled_2x_coeffs, soft_clip_drive,
    soft_clip_drive_f64, soft_knee_guard_f64, voice_guard_f64, Drive, DriveShaperCoeffs,
    Halfband2x, Halfband2xDryAlign, SoftClipCoeffs, DRIVE_MAX_OUTPUT_DB, DRIVE_MAX_TONE_HZ,
    DRIVE_MIN_OUTPUT_DB, DRIVE_MIN_TONE_HZ, VOICE_GUARD_CEILING, VOICE_GUARD_HEADROOM,
    VOICE_GUARD_KNEE,
};

/// Uniform zero-allocation stereo insert effect contract implemented by all
/// native Cadence studio effects (`Eq`, `RhythmicFilter`, `Compressor`,
/// `Limiter`, `Drive`, `Chorus`, `Delay`, `Reverb`).
pub trait StereoEffect {
    /// Process one stereo sample frame; when `bypassed` is true, pass dry audio
    /// with click-free crossfade and consistent delay/tail handling.
    fn process(&mut self, input: [f32; 2], bypassed: bool) -> [f32; 2];

    /// Process a contiguous stereo sub-block in place with bit-exact parity to
    /// frame-by-frame [`StereoEffect::process`].
    #[inline]
    fn process_block(&mut self, buf: &mut [[f32; 2]], bypassed: bool) {
        for frame in buf.iter_mut() {
            *frame = self.process(*frame, bypassed);
        }
    }

    /// Process a contiguous stereo sub-block in place, additionally writing per-frame
    /// instantaneous gain reduction in dB into `gr_out` for dynamics processors.
    ///
    /// Returns `true` if this effect produces dynamics gain-reduction telemetry
    /// (`Compressor`, `Limiter`), or `false` otherwise.
    #[inline]
    fn process_block_with_gr(
        &mut self,
        buf: &mut [[f32; 2]],
        bypassed: bool,
        _gr_out: &mut [f32],
    ) -> bool {
        self.process_block(buf, bypassed);
        false
    }

    /// Clear internal filter/delay/envelope state without heap allocation.
    fn reset(&mut self);

    /// Fixed processing latency in sample frames reported for parallel PDC.
    fn latency_frames(&self) -> usize {
        0
    }

    /// Instantaneous dynamics gain reduction in dB (`>= 0.0`) for dynamics processors.
    fn gain_reduction_db(&self) -> Option<f32> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_stereo_effect<E: StereoEffect>(
        effect: &mut E,
        input: [f32; 2],
        bypassed: bool,
    ) -> [f32; 2] {
        effect.process(input, bypassed)
    }

    #[test]
    fn stereo_effect_trait_dispatches_across_all_eight_studio_effects() {
        let sr = 48_000.0;
        let sr_u32 = 48_000_u32;

        let mut eq = Eq::new(sr);
        let mut filter = RhythmicFilter::new(sr);
        let mut compressor = Compressor::new(sr);
        let mut limiter = Limiter::new(sr);
        let mut drive = Drive::new(sr);
        let mut chorus = Chorus::new(sr);
        let mut delay = Delay::with_max_seconds(sr_u32, 1.0);
        let mut reverb = Reverb::new(sr_u32);

        let effects: [&mut dyn StereoEffect; 8] = [
            &mut eq,
            &mut filter,
            &mut compressor,
            &mut limiter,
            &mut drive,
            &mut chorus,
            &mut delay,
            &mut reverb,
        ];

        for (idx, effect) in effects.into_iter().enumerate() {
            let expected_latency = if idx == 3 {
                limiter_lookahead_frames(sr)
            } else {
                0
            };
            assert_eq!(effect.latency_frames(), expected_latency);

            let expects_gr = idx == 2 || idx == 3;
            assert_eq!(effect.gain_reduction_db().is_some(), expects_gr);

            let out = effect.process([0.5, -0.25], false);
            assert!(out[0].is_finite() && out[1].is_finite());
            effect.reset();
        }

        // Verify static generic dispatch alongside dynamic dispatch.
        let out = run_stereo_effect(&mut eq, [0.25, -0.5], true);
        assert_eq!(out, [0.25, -0.5]);
        assert_eq!(
            StereoEffect::latency_frames(&limiter),
            limiter_lookahead_frames(sr)
        );
        assert_eq!(StereoEffect::gain_reduction_db(&compressor), Some(0.0));
        assert_eq!(StereoEffect::gain_reduction_db(&limiter), Some(0.0));
        assert_eq!(StereoEffect::gain_reduction_db(&eq), None);
    }

    #[test]
    fn stereo_effect_process_block_matches_frame_by_frame_bit_exact() {
        let sr = 48_000.0;
        let sr_u32 = 48_000_u32;
        let total = AUDIO_SUB_BLOCK_FRAMES * 6;
        let input: Vec<[f32; 2]> = (0..total)
            .map(|i| {
                let t = i as f64 / sr;
                let l = (std::f64::consts::TAU * 220.0 * t).sin() as f32 * 0.85;
                let r = (std::f64::consts::TAU * 330.0 * t).cos() as f32 * 0.75;
                [l, r]
            })
            .collect();

        fn verify_pair<E: StereoEffect>(mut a: E, mut b: E, input: &[[f32; 2]], name: &str) {
            let chunk = AUDIO_SUB_BLOCK_FRAMES;
            let mut gr_block = [0.0_f32; AUDIO_SUB_BLOCK_FRAMES];
            // Exercise initial bypassed idle -> active -> mid-fade -> bypassed -> active transitions.
            let schedule = [true, false, false, true, true, false];
            for (blk_idx, &bypassed) in schedule.iter().enumerate() {
                let start = blk_idx * chunk;
                let slice = &input[start..start + chunk];
                let mut ref_out = [[0.0_f32; 2]; AUDIO_SUB_BLOCK_FRAMES];
                let mut ref_gr = [0.0_f32; AUDIO_SUB_BLOCK_FRAMES];
                for i in 0..chunk {
                    ref_out[i] = a.process(slice[i], bypassed);
                    ref_gr[i] = a.gain_reduction_db().unwrap_or(0.0);
                }
                let mut blk_out = [[0.0_f32; 2]; AUDIO_SUB_BLOCK_FRAMES];
                blk_out.copy_from_slice(slice);
                let has_gr = b.process_block_with_gr(&mut blk_out, bypassed, &mut gr_block);
                for i in 0..chunk {
                    assert_eq!(
                        blk_out[i][0].to_bits(),
                        ref_out[i][0].to_bits(),
                        "{name} block {blk_idx} frame {i} L mismatch"
                    );
                    assert_eq!(
                        blk_out[i][1].to_bits(),
                        ref_out[i][1].to_bits(),
                        "{name} block {blk_idx} frame {i} R mismatch"
                    );
                    if has_gr {
                        assert_eq!(
                            gr_block[i].to_bits(),
                            ref_gr[i].to_bits(),
                            "{name} block {blk_idx} frame {i} GR mismatch"
                        );
                    }
                }
            }
        }

        verify_pair(Eq::new(sr), Eq::new(sr), &input, "Eq");
        verify_pair(
            RhythmicFilter::new(sr),
            RhythmicFilter::new(sr),
            &input,
            "RhythmicFilter",
        );
        verify_pair(
            Compressor::new(sr),
            Compressor::new(sr),
            &input,
            "Compressor",
        );
        verify_pair(Limiter::new(sr), Limiter::new(sr), &input, "Limiter");
        verify_pair(Drive::new(sr), Drive::new(sr), &input, "Drive");
        verify_pair(Chorus::new(sr), Chorus::new(sr), &input, "Chorus");
        verify_pair(
            Delay::with_max_seconds(sr_u32, 1.0),
            Delay::with_max_seconds(sr_u32, 1.0),
            &input,
            "Delay",
        );
        verify_pair(Reverb::new(sr_u32), Reverb::new(sr_u32), &input, "Reverb");
    }
}
