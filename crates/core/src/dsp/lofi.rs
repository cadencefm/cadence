//! Pure, zero-allocation stereo Lo-Fi bit-depth quantizer and sample-rate clock
//! decimator (`LoFiProcessor`) plus transfer-curve evaluation (`lofi_transfer_sample`)
//! shared by `cadence-engine` and `cadence-app` UI visualizers.

use crate::dsp::math::{sanitize_sample, sanitize_sample_f64, sanitize_sample_rate};

/// Minimum continuous bit-depth for [`LoFiProcessor`].
pub const LOFI_MIN_BITS: f32 = 2.0;
/// Maximum continuous bit-depth for [`LoFiProcessor`].
pub const LOFI_MAX_BITS: f32 = 24.0;
/// Threshold at or above which bit quantization is transparently bypassed.
pub const LOFI_BYPASS_BITS: f32 = 23.95;
/// Minimum sample-rate hold clock frequency (Hz).
pub const LOFI_MIN_RATE_HZ: f32 = 500.0;
/// Maximum sample-rate hold clock frequency (Hz).
pub const LOFI_MAX_RATE_HZ: f32 = 48_000.0;

const LOFI_BYPASS_BITS_F64: f64 = 23.95;

/// Continuous mid-tread bit quantizer in `f64`.
///
/// - Transparently bypassed when `bits >= 23.95` (or non-finite).
/// - Strictly odd-symmetric (`q(-x) == -q(x)`) and zero-centered (`q(0.0) == 0.0`),
///   preventing DC offset on silent passages.
/// - Supports continuous fractional bit depths (`2.0..=24.0`) without zipper clicks.
#[inline]
pub fn lofi_quantize_f64(x: f64, bits: f64) -> f64 {
    let x = sanitize_sample_f64(x);
    if !bits.is_finite() || bits >= LOFI_BYPASS_BITS_F64 {
        return x;
    }
    let b = bits.clamp(f64::from(LOFI_MIN_BITS), f64::from(LOFI_MAX_BITS));
    let half_levels = 2.0_f64.powf(b - 1.0);
    let q = (x * half_levels).round() / half_levels;
    if q == 0.0 {
        0.0
    } else {
        q
    }
}

/// Continuous mid-tread bit quantizer (`f32` wrapper over [`lofi_quantize_f64`]).
#[inline]
pub fn lofi_quantize(x: f32, bits: f32) -> f32 {
    lofi_quantize_f64(f64::from(x), f64::from(bits)) as f32
}

/// Pure transfer-curve sample for UI visualizers matching [`LoFiProcessor`] bit
/// quantization and wet/dry `mix` exactly (with clock decimation bypassed).
#[inline]
pub fn lofi_transfer_sample(x: f32, bits: f32, mix: f32) -> f32 {
    let dry = sanitize_sample(x);
    let m = if mix.is_finite() {
        mix.clamp(0.0, 1.0)
    } else {
        1.0
    };
    if m <= 0.0 {
        return dry;
    }
    let wet = lofi_quantize(dry, bits);
    dry * (1.0 - m) + wet * m
}

/// Zero-allocation stereo bit-depth quantizer and phase-accumulating sample-rate
/// zero-order hold decimator.
#[derive(Clone, Copy, Debug)]
pub struct LoFiProcessor {
    sample_rate: f64,
    phase: f64,
    held: [f64; 2],
    primed: bool,
}

impl Default for LoFiProcessor {
    fn default() -> Self {
        Self::new(48_000.0)
    }
}

impl LoFiProcessor {
    #[inline]
    pub fn new(sample_rate: f64) -> Self {
        Self {
            sample_rate: sanitize_sample_rate(sample_rate),
            phase: 0.0,
            held: [0.0; 2],
            primed: false,
        }
    }

    #[inline]
    pub fn reset(&mut self) {
        self.phase = 0.0;
        self.held = [0.0; 2];
        self.primed = false;
    }

    #[inline]
    pub fn set_sample_rate(&mut self, sample_rate: f64) {
        self.sample_rate = sanitize_sample_rate(sample_rate);
    }

    /// Process one stereo frame at `f64` precision.
    ///
    /// - Sample-rate hold is bypassed when `rate_hz >= sample_rate`.
    /// - Bit quantization is bypassed when `bits >= 23.95`.
    /// - `mix` blends linearly between dry input (`0.0`) and lo-fi processed output (`1.0`).
    #[inline]
    pub fn process_stereo(
        &mut self,
        input: [f64; 2],
        bits: f32,
        rate_hz: f32,
        mix: f32,
    ) -> [f64; 2] {
        let dry = [sanitize_sample_f64(input[0]), sanitize_sample_f64(input[1])];
        let m = if mix.is_finite() {
            f64::from(mix.clamp(0.0, 1.0))
        } else {
            1.0
        };

        let rate = if rate_hz.is_finite() {
            f64::from(rate_hz.clamp(LOFI_MIN_RATE_HZ, LOFI_MAX_RATE_HZ))
        } else {
            self.sample_rate
        };

        let decimated =
            if rate >= self.sample_rate - 1e-6 || rate >= f64::from(LOFI_MAX_RATE_HZ) - 1.0 {
                self.held = dry;
                self.phase = 0.0;
                self.primed = true;
                dry
            } else {
                let inc = (rate / self.sample_rate).clamp(1e-6, 1.0);
                if !self.primed {
                    self.held = dry;
                    self.phase = 0.0;
                    self.primed = true;
                } else {
                    self.phase += inc;
                    if self.phase >= 1.0 {
                        self.phase -= self.phase.floor();
                        self.held = dry;
                    }
                }
                self.held
            };

        if m <= 0.0 {
            return dry;
        }

        let b = if bits.is_finite() {
            f64::from(bits)
        } else {
            f64::from(LOFI_MAX_BITS)
        };
        let wet = [
            lofi_quantize_f64(decimated[0], b),
            lofi_quantize_f64(decimated[1], b),
        ];
        if m >= 1.0 {
            wet
        } else {
            let inv = 1.0 - m;
            [dry[0] * inv + wet[0] * m, dry[1] * inv + wet[1] * m]
        }
    }

    /// Process one mono sample at `f64` precision.
    #[inline]
    pub fn process_mono(&mut self, input: f64, bits: f32, rate_hz: f32, mix: f32) -> f64 {
        self.process_stereo([input, input], bits, rate_hz, mix)[0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bit_quantizer_is_bypassed_at_high_bits_odd_symmetric_and_zero_dc() {
        for &x in &[-1.0, -0.73, -0.123, 0.0, 0.123, 0.73, 1.0] {
            // Bypassed at >= 23.95 bits.
            assert_eq!(lofi_quantize_f64(x, 23.95), x);
            assert_eq!(lofi_quantize_f64(x, 24.0), x);
            assert_eq!(lofi_quantize_f64(x, f64::NAN), x);

            // Strictly odd-symmetric at every bit depth.
            for &bits in &[2.0, 3.5, 4.0, 8.0, 12.0, 16.0] {
                let pos = lofi_quantize_f64(x, bits);
                let neg = lofi_quantize_f64(-x, bits);
                assert!(
                    (pos + neg).abs() < 1e-12,
                    "bits={bits} x={x}: pos={pos} neg={neg}"
                );
            }
        }

        // Zero static DC offset at silence.
        for b in 2..=24 {
            assert_eq!(lofi_quantize_f64(0.0, f64::from(b)), 0.0);
        }

        // 2-bit mid-tread quantizer has exact 0.5 step spacing.
        assert_eq!(lofi_quantize_f64(0.10, 2.0), 0.0);
        assert_eq!(lofi_quantize_f64(0.35, 2.0), 0.5);
        assert_eq!(lofi_quantize_f64(0.85, 2.0), 1.0);
        assert_eq!(lofi_quantize_f64(-0.35, 2.0), -0.5);

        // NaN/Inf input sanitized to 0.0.
        assert_eq!(lofi_quantize_f64(f64::NAN, 8.0), 0.0);
        assert_eq!(lofi_quantize_f64(f64::INFINITY, 8.0), 0.0);
    }

    #[test]
    fn processor_matches_transfer_curve_and_holds_clock_accurately_across_sample_rates() {
        for sr in [44_100.0, 48_000.0, 96_000.0] {
            // 1. Full transparent bypass when bits >= 23.95 and rate_hz >= sr.
            let mut proc = LoFiProcessor::new(sr);
            let input = [0.314_159, -0.271_828];
            let bypassed = proc.process_stereo(input, 24.0, sr as f32, 1.0);
            if sr <= f64::from(LOFI_MAX_RATE_HZ) {
                assert_eq!(bypassed, input);
            }

            // 2. Mix == 0.0 is always exact dry identity regardless of bits/rate.
            proc.reset();
            for i in 0..32 {
                let frame = [i as f64 * 0.02, -(i as f64) * 0.015];
                assert_eq!(proc.process_stereo(frame, 2.0, 500.0, 0.0), frame);
            }

            // 3. With rate_hz >= sr (when sr <= 48k), LoFiProcessor matches lofi_transfer_sample.
            if sr <= f64::from(LOFI_MAX_RATE_HZ) {
                proc.reset();
                for i in -20..=20 {
                    let x = i as f32 / 20.0;
                    let dsp = proc.process_mono(f64::from(x), 4.0, 48_000.0, 0.75) as f32;
                    let ui = lofi_transfer_sample(x, 4.0, 0.75);
                    assert!(
                        (dsp - ui).abs() < 1e-6,
                        "sr={sr} x={x}: dsp={dsp} vs ui={ui}"
                    );
                }
            }

            // 4. Phase-accumulating zero-order hold steps at exact rate_hz cadence.
            proc.reset();
            let hold_rate = sr / 8.0; // exact 8-sample hold period
            if hold_rate >= f64::from(LOFI_MIN_RATE_HZ) && hold_rate <= f64::from(LOFI_MAX_RATE_HZ)
            {
                let mut transitions = 0usize;
                let mut prev = f64::NAN;
                for i in 0..80 {
                    let x = (i as f64) * 0.01;
                    let y = proc.process_mono(x, 24.0, hold_rate as f32, 1.0);
                    assert!(y.is_finite());
                    if i > 0 && (y - prev).abs() > 1e-12 {
                        transitions += 1;
                    }
                    prev = y;
                }
                assert!(
                    (9..=10).contains(&transitions),
                    "sr={sr} hold_rate={hold_rate}: expected ~9-10 hold updates in 80 samples, got {transitions}"
                );
            }
        }
    }
}
