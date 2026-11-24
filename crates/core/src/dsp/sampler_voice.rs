//! Pure, zero-allocation sample playback DSP: 4-point Catmull-Rom cubic Hermite
//! stereo sample lookup and [`SamplerCursor`] supporting One-Shot (`Off`),
//! crossfaded `Forward` looping, and crossfaded `PingPong` looping.

use crate::dsp::math::{hermite4_f64, sanitize_sample, sanitize_sample_f64, sanitize_sample_rate};
use crate::sampler::{SampleData, SampleLoopMode};

/// Pure playback window and loop configuration for [`SamplerCursor`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SamplerRegion {
    pub sample_start: usize,
    pub sample_end: usize,
    pub loop_mode: SampleLoopMode,
    pub loop_start: usize,
    pub loop_end: usize,
    pub loop_xfade_ms: f32,
}

impl Default for SamplerRegion {
    fn default() -> Self {
        Self {
            sample_start: 0,
            sample_end: 0,
            loop_mode: SampleLoopMode::Off,
            loop_start: 0,
            loop_end: 0,
            loop_xfade_ms: 0.0,
        }
    }
}

/// 4-point Catmull-Rom cubic Hermite stereo lookup into `sample` at fractional
/// frame `pos`, clamped strictly inside `[min_idx, max_excl)`.
#[inline]
pub fn sample_hermite_stereo_f64(
    sample: &SampleData,
    pos: f64,
    min_idx: usize,
    max_excl: usize,
) -> [f64; 2] {
    let bound_end = max_excl.min(sample.frames);
    if !pos.is_finite() || bound_end == 0 || min_idx >= bound_end {
        return [0.0, 0.0];
    }
    let last = bound_end - 1;
    let base = (pos.floor() as isize).clamp(min_idx as isize, last as isize) as usize;
    let frac = (pos - base as f64).clamp(0.0, 1.0);
    let i_m1 = base.saturating_sub(1).max(min_idx);
    let i_0 = base;
    let i_1 = (base + 1).min(last);
    let i_2 = (base + 2).min(last);
    let fm1 = sample.frame_stereo(i_m1);
    let f0 = sample.frame_stereo(i_0);
    let f1 = sample.frame_stereo(i_1);
    let f2 = sample.frame_stereo(i_2);
    [
        sanitize_sample_f64(hermite4_f64(
            f64::from(fm1[0]),
            f64::from(f0[0]),
            f64::from(f1[0]),
            f64::from(f2[0]),
            frac,
        )),
        sanitize_sample_f64(hermite4_f64(
            f64::from(fm1[1]),
            f64::from(f0[1]),
            f64::from(f1[1]),
            f64::from(f2[1]),
            frac,
        )),
    ]
}

#[inline]
fn resolve_loop_bounds(
    start: usize,
    end: usize,
    loop_start: usize,
    loop_end: usize,
) -> (usize, usize) {
    let l_start = loop_start.clamp(start, end.saturating_sub(1));
    let l_end = if loop_end == 0 {
        end
    } else {
        loop_end.clamp(l_start + 1, end)
    };
    (l_start, l_end)
}

/// Zero-allocation sample playback cursor supporting One-Shot (`Off`) with
/// click-free 1.5 ms end-of-region tail fade, crossfaded `Forward` looping, and
/// crossfaded `PingPong` looping.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SamplerCursor {
    pub pos: f64,
    pub pingpong_dir: f64,
    pub pingpong_entered: bool,
    pub playing: bool,
}

impl Default for SamplerCursor {
    fn default() -> Self {
        Self::new(0, false)
    }
}

impl SamplerCursor {
    #[inline]
    pub const fn new(sample_start: usize, playing: bool) -> Self {
        Self {
            pos: sample_start as f64,
            pingpong_dir: 1.0,
            pingpong_entered: false,
            playing,
        }
    }

    #[inline]
    pub fn retrigger(&mut self, sample_start: usize) {
        self.pos = sample_start as f64;
        self.pingpong_dir = 1.0;
        self.pingpong_entered = false;
        self.playing = true;
    }

    #[inline]
    pub const fn is_playing(self) -> bool {
        self.playing
    }

    #[inline]
    pub fn stop(&mut self) {
        self.playing = false;
    }

    /// Advance the playback head by one output frame at `step_ratio` source frames
    /// per output frame and return the interpolated stereo `[f64; 2]` sample.
    #[inline]
    pub fn step(
        &mut self,
        sample: &SampleData,
        region: SamplerRegion,
        step_ratio: f64,
        sample_rate: f64,
    ) -> [f64; 2] {
        if !self.playing || sample.frames == 0 {
            self.playing = false;
            return [0.0, 0.0];
        }
        let step_ratio = if step_ratio.is_finite() {
            step_ratio.max(0.0)
        } else {
            0.0
        };
        let sr = sanitize_sample_rate(sample_rate);
        let start = region.sample_start.min(sample.frames.saturating_sub(1));
        let end = if region.sample_end == 0 {
            sample.frames
        } else {
            region.sample_end.min(sample.frames).max(start + 1)
        };

        match region.loop_mode {
            SampleLoopMode::Off => {
                if self.pos >= end as f64 {
                    self.playing = false;
                    return [0.0, 0.0];
                }
                let raw = sample_hermite_stereo_f64(sample, self.pos, start, end);
                let remain_out = (end as f64 - self.pos).max(0.0) / step_ratio.max(1e-6);
                let fade_out = (0.0015 * sr).max(8.0);
                let tail = (remain_out / fade_out).clamp(0.0, 1.0);
                self.pos += step_ratio;
                [raw[0] * tail, raw[1] * tail]
            }
            SampleLoopMode::Forward => {
                let (l_start, l_end) =
                    resolve_loop_bounds(start, end, region.loop_start, region.loop_end);
                let xfade_ms = f64::from(sanitize_sample(region.loop_xfade_ms).max(0.0));
                let xfade_frames = ((xfade_ms * 0.001) * f64::from(sample.sample_rate)) as usize;
                let xf = xfade_frames.min((l_end - l_start) / 2);
                let loop_period = (l_end - l_start - xf).max(1) as f64;
                if self.pos >= l_end as f64 {
                    self.pos = (l_start + xf) as f64 + (self.pos - l_end as f64) % loop_period;
                }
                let mut out = sample_hermite_stereo_f64(sample, self.pos, start, l_end);
                if xf > 0 && self.pos >= (l_end - xf) as f64 {
                    let alpha = ((self.pos - (l_end - xf) as f64) / xf as f64).clamp(0.0, 1.0);
                    let mirror_pos = l_start as f64 + (self.pos - (l_end - xf) as f64);
                    let head = sample_hermite_stereo_f64(sample, mirror_pos, l_start, l_end);
                    out[0] = out[0] * (1.0 - alpha) + head[0] * alpha;
                    out[1] = out[1] * (1.0 - alpha) + head[1] * alpha;
                }
                self.pos += step_ratio;
                out
            }
            SampleLoopMode::PingPong => {
                let (l_start, l_end) =
                    resolve_loop_bounds(start, end, region.loop_start, region.loop_end);
                let lo = l_start as f64;
                let hi = (l_end - 1).max(l_start) as f64;
                let mut out = sample_hermite_stereo_f64(sample, self.pos, start, l_end);
                let xfade_ms = f64::from(sanitize_sample(region.loop_xfade_ms).max(0.0));
                let xfade_frames = ((xfade_ms * 0.001) * f64::from(sample.sample_rate)) as usize;
                let xf = xfade_frames.min((l_end - l_start) / 2);
                if xf > 0 && hi > lo {
                    let xf_f = xf as f64;
                    let d_hi = (hi - self.pos).max(0.0);
                    if d_hi < xf_f {
                        let w = 0.5 * (1.0 - d_hi / xf_f);
                        let mirror_pos = (hi - xf_f + d_hi).clamp(lo, hi);
                        let mirror = sample_hermite_stereo_f64(sample, mirror_pos, l_start, l_end);
                        out[0] = out[0] * (1.0 - w) + mirror[0] * w;
                        out[1] = out[1] * (1.0 - w) + mirror[1] * w;
                    } else if self.pingpong_entered {
                        let d_lo = (self.pos - lo).max(0.0);
                        if d_lo < xf_f {
                            let w = 0.5 * (1.0 - d_lo / xf_f);
                            let mirror_pos = (lo + xf_f - d_lo).clamp(lo, hi);
                            let mirror =
                                sample_hermite_stereo_f64(sample, mirror_pos, l_start, l_end);
                            out[0] = out[0] * (1.0 - w) + mirror[0] * w;
                            out[1] = out[1] * (1.0 - w) + mirror[1] * w;
                        }
                    }
                }
                if self.pos >= lo {
                    self.pingpong_entered = true;
                }
                self.pos += step_ratio * self.pingpong_dir;
                if hi <= lo {
                    self.pos = lo;
                } else if self.pos > hi || (self.pos < lo && self.pingpong_dir < 0.0) {
                    let span = hi - lo;
                    let two_span = 2.0 * span;
                    let offset = if self.pingpong_dir > 0.0 {
                        self.pos - lo
                    } else {
                        two_span - (self.pos - lo)
                    };
                    let cyc = offset.rem_euclid(two_span);
                    if cyc <= span {
                        self.pos = lo + cyc;
                        self.pingpong_dir = 1.0;
                    } else {
                        self.pos = lo + (two_span - cyc);
                        self.pingpong_dir = -1.0;
                    }
                    self.pingpong_entered = true;
                }
                out
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sampler::SampleMetadata;
    use std::path::PathBuf;

    fn make_sample(left: Vec<f32>, right: Option<Vec<f32>>, sample_rate: u32) -> SampleData {
        let frames = left.len();
        let channels = if right.is_some() { 2 } else { 1 };
        SampleData {
            path_label: "synthetic".into(),
            resolved_path: PathBuf::from("synthetic.wav"),
            name: "synthetic".into(),
            sample_rate,
            channels,
            frames,
            left: left.into_boxed_slice(),
            right: right.map(Vec::into_boxed_slice),
            peaks: Box::new([]),
            meta: SampleMetadata::default(),
        }
    }

    #[test]
    fn sample_hermite_interpolates_linear_ramp_exactly_and_guards_edges() {
        let left: Vec<f32> = (0..16).map(|i| i as f32 * 0.05).collect();
        let right: Vec<f32> = (0..16).map(|i| -(i as f32) * 0.05).collect();
        let sample = make_sample(left, Some(right), 48_000);

        let mid = sample_hermite_stereo_f64(&sample, 5.25, 0, 16);
        assert!((mid[0] - 5.25 * 0.05).abs() < 1e-6, "got {mid:?}");
        assert!((mid[1] + 5.25 * 0.05).abs() < 1e-6, "got {mid:?}");

        // Non-finite pos and empty bounds return silence cleanly.
        assert_eq!(
            sample_hermite_stereo_f64(&sample, f64::NAN, 0, 16),
            [0.0, 0.0]
        );
        assert_eq!(sample_hermite_stereo_f64(&sample, 2.0, 5, 5), [0.0, 0.0]);
        let empty = make_sample(vec![], None, 48_000);
        assert_eq!(sample_hermite_stereo_f64(&empty, 0.0, 0, 1), [0.0, 0.0]);
    }

    #[test]
    fn oneshot_fades_tail_cleanly_and_stops_at_region_end() {
        let sr = 48_000.0;
        let sample = make_sample(vec![0.8; 256], None, 48_000);
        let region = SamplerRegion {
            sample_start: 16,
            sample_end: 200,
            loop_mode: SampleLoopMode::Off,
            ..SamplerRegion::default()
        };
        let mut cursor = SamplerCursor::new(region.sample_start, true);
        let mut frames = Vec::new();
        while cursor.is_playing() {
            let y = cursor.step(&sample, region, 1.0, sr);
            if cursor.is_playing() {
                frames.push(y[0]);
            }
        }
        assert_eq!(frames.len(), 200 - 16);
        assert!((frames[0] - 0.8).abs() < 1e-6);
        // Final active frame should be faded down close to zero by the 1.5 ms tail fade.
        let last = *frames.last().unwrap();
        assert!(
            (0.0..0.05).contains(&last),
            "expected faded tail, got {last}"
        );
        assert_eq!(cursor.step(&sample, region, 1.0, sr), [0.0, 0.0]);
    }

    #[test]
    fn forward_loop_with_crossfade_smooths_step_discontinuity() {
        // Construct a sample with a sharp jump from +1.0 at loop end to -1.0 at loop start.
        let sr = 10_000_u32;
        let n = 200;
        let mut data = Vec::with_capacity(n);
        for i in 0..n {
            // Ramp from -1.0 to +1.0 across [0..200).
            data.push(-1.0 + 2.0 * (i as f32 / (n - 1) as f32));
        }
        let sample = make_sample(data, None, sr);

        let measure_max_jump = |xfade_ms: f32| -> f64 {
            let region = SamplerRegion {
                sample_start: 0,
                sample_end: n,
                loop_mode: SampleLoopMode::Forward,
                loop_start: 0,
                loop_end: n,
                loop_xfade_ms: xfade_ms,
            };
            let mut cursor = SamplerCursor::new(0, true);
            let mut prev = cursor.step(&sample, region, 1.0, f64::from(sr))[0];
            let mut max_jump = 0.0_f64;
            for _ in 0..(n * 3) {
                let y = cursor.step(&sample, region, 1.0, f64::from(sr))[0];
                max_jump = max_jump.max((y - prev).abs());
                prev = y;
            }
            assert!(cursor.is_playing());
            max_jump
        };

        let raw_jump = measure_max_jump(0.0);
        let xfade_jump = measure_max_jump(5.0); // 50 frames crossfade at 10 kHz
        assert!(
            raw_jump > 1.8,
            "expected sharp discontinuity without xfade, got {raw_jump}"
        );
        assert!(
            xfade_jump < 0.15,
            "expected smooth crossfaded loop boundary, got {xfade_jump}"
        );
    }

    #[test]
    fn pingpong_loop_bounces_continuously_with_crossfade_and_retrigger() {
        let sr = 10_000_u32;
        let n = 120;
        let data: Vec<f32> = (0..n).map(|i| i as f32 / (n - 1) as f32).collect();
        let sample = make_sample(data, None, sr);
        let region = SamplerRegion {
            sample_start: 0,
            sample_end: n,
            loop_mode: SampleLoopMode::PingPong,
            loop_start: 20,
            loop_end: 100,
            loop_xfade_ms: 2.0, // 20 frames
        };
        let mut cursor = SamplerCursor::new(0, true);
        let mut saw_reverse = false;
        let mut saw_forward_again = false;
        for _ in 0..400 {
            let y = cursor.step(&sample, region, 1.0, f64::from(sr));
            assert!(y[0].is_finite() && y[1].is_finite());
            if cursor.pingpong_dir < 0.0 {
                saw_reverse = true;
            } else if saw_reverse && cursor.pingpong_dir > 0.0 {
                saw_forward_again = true;
            }
        }
        assert!(
            saw_reverse && saw_forward_again,
            "expected full ping-pong roundtrip"
        );
        assert!(cursor.pingpong_entered);

        cursor.retrigger(10);
        assert_eq!(cursor.pos, 10.0);
        assert_eq!(cursor.pingpong_dir, 1.0);
        assert!(!cursor.pingpong_entered);
        assert!(cursor.is_playing());
    }
}
