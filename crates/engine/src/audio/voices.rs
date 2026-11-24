//! Unified heap-boxed [`ModularVoice`] pool entry with Rack-bus SVF, stereo balance
//! pan, unclamped bus headroom gain, and live held-note stack.
use super::*;
use crate::modular_voice::ModularVoice;
use cadence_core::dsp::{
    control_stride_mask, flush_denormal_f32, one_pole_coeff_ms, pan_stereo_balance_f64,
    sanitize_sample, sanitize_sample_rate, stereo_mid_f32, LogSmoother, Smoother, Svf, SvfCoeffs,
    AUDIO_SUB_BLOCK_FRAMES,
};
use cadence_core::rack::FilterMode;
use cadence_core::sampler::SampleData;
use std::sync::Arc;

/// Target Rack-bus SVF coefficient update cadence (`6 kHz`:
/// `8` frames @ 44.1/48 kHz, `16` @ 96 kHz, `32` @ 192 kHz).
const RACK_SVF_CTRL_TARGET_HZ: f64 = 6_000.0;

/// Unified polyphonic/monophonic Rack voice slot.
///
/// Pre-allocates its `Box<ModularVoice>` once at `Engine::new` and reuses it
/// strictly in-place so real-time triggering, retargeting, and reset perform
/// zero heap operations and keep debug stacks compact.
pub(super) struct Voice {
    pub(super) active: bool,
    pub(super) track: u8,
    pub(super) track_id: u64,
    pub(super) zone_id: u16,
    pub(super) modular: Box<ModularVoice>,
    /// Bitmask of P-locked [`cadence_core::rack::RackParam`]s (`1 << param.id()`).
    pub(super) rack_locks: u8,
    /// Bitmask of P-locked [`cadence_core::rack::ZoneParam`]s (`1 << (param as u8)`).
    pub(super) zone_locks: u8,
    /// Trigger-time Rack bus snapshots preserved for locked parameters.
    pub(super) rack_gain_db: f32,
    pub(super) rack_pan: f32,
    pub(super) rack_tune: f32,
    pub(super) rack_cutoff_hz: f32,
    pub(super) rack_resonance: f32,
    /// Trigger-time Zone bus snapshots preserved for locked parameters.
    pub(super) zone_gain_db: f32,
    pub(super) zone_pan: f32,
    /// Smoothed unclamped bus gain (`db_to_lin(rack_gain_db + zone_gain_db)`).
    pub(super) gain_scale: Smoother,
    /// Smoothed Rack bus stereo balance pan (`pan_stereo_balance_f64`).
    pub(super) rack_pan_sm: Smoother,
    pub(super) cached_rack_pan_bits: u64,
    pub(super) cached_rack_pan_lr: [f64; 2],
    /// Dedicated Rack-bus LowPass SVF (transparent bypass at >= 19.9 kHz & res <= 0.001).
    pub(super) rack_svf: Svf,
    pub(super) rack_svf_coeffs: SvfCoeffs,
    pub(super) cached_rack_svf_key: (u64, u32),
    pub(super) rack_cutoff: LogSmoother,
    pub(super) rack_cutoff_target_hz: f64,
    pub(super) rack_cutoff_coeff: f64,
    pub(super) rack_res_sm: Smoother,
    pub(super) rack_svf_ctrl: u32,
    pub(super) rack_svf_mask: u32,
    pub(super) rack_svf_was_active: bool,
    pub(super) sample_rate: f64,
    pub(super) live_key: Option<u8>,
    pub(super) choke_group: u8,
    pub(super) age: u32,
    pub(super) gate: u32,
    pub(super) last_output: [f32; 2],
}

impl Voice {
    /// Allocate an inactive voice slot with its heap-boxed [`ModularVoice`].
    pub(super) fn new_inactive(sample_rate: f64) -> Self {
        let sr = sanitize_sample_rate(sample_rate);
        Self {
            active: false,
            track: u8::MAX,
            track_id: 0,
            zone_id: 0,
            modular: ModularVoice::new_boxed_inactive(sr),
            rack_locks: 0,
            zone_locks: 0,
            rack_gain_db: 0.0,
            rack_pan: 0.0,
            rack_tune: 0.0,
            rack_cutoff_hz: 20_000.0,
            rack_resonance: 0.0,
            zone_gain_db: 0.0,
            zone_pan: 0.0,
            gain_scale: Smoother::new(1.0, 4.0, sr),
            rack_pan_sm: Smoother::new(0.0, 4.0, sr),
            cached_rack_pan_bits: 0.0_f64.to_bits(),
            cached_rack_pan_lr: [1.0, 1.0],
            rack_svf: Svf::default(),
            rack_svf_coeffs: SvfCoeffs::new(sr, 20_000.0, 0.0),
            cached_rack_svf_key: (20_000.0_f64.to_bits(), 0.0_f32.to_bits()),
            rack_cutoff: LogSmoother::new_clamped(20_000.0, 20.0, 20_000.0),
            rack_cutoff_target_hz: 20_000.0,
            rack_cutoff_coeff: one_pole_coeff_ms(4.0, sr),
            rack_res_sm: Smoother::new(0.0, 4.0, sr),
            rack_svf_ctrl: 0,
            rack_svf_mask: control_stride_mask(sr, RACK_SVF_CTRL_TARGET_HZ),
            rack_svf_was_active: false,
            sample_rate: sr,
            live_key: None,
            choke_group: 0,
            age: 0,
            gate: 1,
            last_output: [0.0; 2],
        }
    }

    /// Zero-allocation deactivation preserving the heap-boxed [`ModularVoice`].
    #[inline]
    pub(super) fn deactivate(&mut self) {
        self.active = false;
        self.track = u8::MAX;
        self.track_id = 0;
        self.zone_id = 0;
        self.live_key = None;
        self.choke_group = 0;
        self.rack_locks = 0;
        self.zone_locks = 0;
        self.age = 0;
        self.last_output = [0.0; 2];
        self.rack_svf.reset();
        self.rack_svf_was_active = false;
        self.modular.deactivate();
    }

    #[inline]
    pub(super) fn is_releasing(&self) -> bool {
        !self.active || self.modular.is_releasing()
    }

    #[inline]
    pub(super) fn current_level(&self) -> f32 {
        let g = self.gain_scale.current() as f32;
        self.modular.current_level() * g
    }

    /// Snap Rack-bus gain, stereo balance pan, and bus SVF immediately on fresh trigger.
    #[inline]
    pub(super) fn snap_rack_bus(
        &mut self,
        gain_lin: f32,
        rack_pan: f32,
        cutoff_hz: f32,
        resonance: f32,
        sample_rate: f64,
    ) {
        let sr = sanitize_sample_rate(sample_rate);
        self.sample_rate = sr;
        let g = f64::from(sanitize_sample(gain_lin).max(0.0));
        let pan = f64::from(sanitize_sample(rack_pan).clamp(-1.0, 1.0));
        let fc = f64::from(sanitize_sample(cutoff_hz)).clamp(20.0, 20_000.0);
        let res = sanitize_sample(resonance).clamp(0.0, 0.95);
        self.gain_scale = Smoother::new(g, 4.0, sr);
        self.rack_pan_sm = Smoother::new(pan, 4.0, sr);
        self.cached_rack_pan_bits = pan.to_bits();
        self.cached_rack_pan_lr = if pan == 0.0 {
            [1.0, 1.0]
        } else {
            pan_stereo_balance_f64(pan)
        };
        self.rack_cutoff = LogSmoother::new_clamped(fc, 20.0, 20_000.0);
        self.rack_cutoff_target_hz = fc;
        self.rack_cutoff_coeff = one_pole_coeff_ms(4.0, sr);
        self.rack_res_sm = Smoother::new(f64::from(res), 4.0, sr);
        self.rack_svf.reset();
        self.rack_svf_ctrl = 0;
        self.rack_svf_mask = control_stride_mask(sr, RACK_SVF_CTRL_TARGET_HZ);
        self.rack_svf_was_active = fc < 19_900.0 || res > 0.001;
        if self.rack_svf_was_active {
            self.cached_rack_svf_key = (fc.to_bits(), res.to_bits());
            self.rack_svf_coeffs = SvfCoeffs::new(sr, fc, res);
        }
    }

    /// Smoothly retarget Rack-bus gain, stereo balance pan, and bus SVF on continuous updates
    /// or mono/legato retargets without resetting filter integrators.
    #[inline]
    pub(super) fn set_rack_bus_targets(
        &mut self,
        gain_lin: f32,
        rack_pan: f32,
        cutoff_hz: f32,
        resonance: f32,
    ) {
        let g = f64::from(sanitize_sample(gain_lin).max(0.0));
        let pan = f64::from(sanitize_sample(rack_pan).clamp(-1.0, 1.0));
        let fc = f64::from(sanitize_sample(cutoff_hz)).clamp(20.0, 20_000.0);
        let res = f64::from(sanitize_sample(resonance).clamp(0.0, 0.95));
        self.gain_scale.set_target(g);
        self.rack_pan_sm.set_target(pan);
        if (fc - self.rack_cutoff_target_hz).abs() > 0.0 {
            self.rack_cutoff_target_hz = fc;
            self.rack_cutoff.set_target(fc);
        }
        self.rack_res_sm.set_target(res);
    }

    #[inline]
    pub(super) fn begin_release(&mut self) {
        if self.active {
            self.modular.note_off();
        }
    }

    #[inline]
    pub(super) fn force_release(&mut self) {
        if self.active {
            self.modular.force_release();
        }
        self.live_key = None;
    }

    #[inline]
    pub(super) fn choke(&mut self, sample_rate: f64) {
        if self.active {
            self.modular.choke(sample_rate);
        }
        self.live_key = None;
    }

    /// Render up to `out_src.len()` (`<= AUDIO_SUB_BLOCK_FRAMES`) consecutive stereo frames
    /// for this voice while keeping its `Box<ModularVoice>` hot in L1D cache, accumulating
    /// into `out_src` and optional mono `out_scope`.
    pub(super) fn render_block(
        &mut self,
        bank: &SampleBank,
        preview: Option<&Arc<SampleData>>,
        out_src: &mut [[f32; 2]],
        mut out_scope: Option<&mut [f32]>,
    ) {
        let frames = out_src.len().min(AUDIO_SUB_BLOCK_FRAMES);
        if !self.active || frames == 0 {
            return;
        }

        let mut raw_buf = [[0.0_f32; 2]; AUDIO_SUB_BLOCK_FRAMES];
        let mod_frames = self
            .modular
            .render_block(bank, preview, &mut raw_buf[..frames]);

        let settled = self.gain_scale.is_settled()
            && self.rack_pan_sm.is_settled()
            && self.rack_cutoff.is_settled()
            && self.rack_res_sm.is_settled();

        if settled {
            let gain_scale = self.gain_scale.next();
            let rack_pan = self.rack_pan_sm.next();
            let fc = self.rack_cutoff.step(self.rack_cutoff_coeff);
            let res = self.rack_res_sm.next() as f32;
            let rack_pan_bits = rack_pan.to_bits();
            if rack_pan_bits != self.cached_rack_pan_bits {
                self.cached_rack_pan_bits = rack_pan_bits;
                self.cached_rack_pan_lr = if rack_pan == 0.0 {
                    [1.0, 1.0]
                } else {
                    pan_stereo_balance_f64(rack_pan)
                };
            }
            let [rpl, rpr] = self.cached_rack_pan_lr;
            let scale_l = rpl * gain_scale;
            let scale_r = rpr * gain_scale;
            let filter_active = fc < 19_900.0 || res > 0.001;

            if !filter_active {
                if self.rack_svf_was_active {
                    self.rack_svf.reset();
                    self.rack_svf_was_active = false;
                }
                let run_frames = if mod_frames == 0 { 1 } else { mod_frames };
                let mut last = self.last_output;
                for i in 0..run_frames {
                    let raw = raw_buf[i];
                    let output = [
                        flush_denormal_f32(sanitize_sample((f64::from(raw[0]) * scale_l) as f32)),
                        flush_denormal_f32(sanitize_sample((f64::from(raw[1]) * scale_r) as f32)),
                    ];
                    out_src[i][0] += output[0];
                    out_src[i][1] += output[1];
                    if let Some(scope) = out_scope.as_deref_mut() {
                        scope[i] += stereo_mid_f32(output);
                    }
                    last = output;
                }
                self.age = self.age.saturating_add(run_frames as u32);
                self.last_output = last;
                if !self.modular.is_active() {
                    self.active = false;
                }
                return;
            }

            let svf_key = (fc.to_bits(), res.to_bits());
            if self.rack_svf_was_active && svf_key == self.cached_rack_svf_key {
                let coeffs = self.rack_svf_coeffs;
                let mut processed = 0usize;
                let mut last = self.last_output;
                for i in 0..frames {
                    let raw = raw_buf[i];
                    let stereo = self.rack_svf.process_stereo(
                        [f64::from(raw[0]), f64::from(raw[1])],
                        coeffs,
                        FilterMode::LowPass,
                    );
                    let output = [
                        flush_denormal_f32(sanitize_sample((stereo[0] * scale_l) as f32)),
                        flush_denormal_f32(sanitize_sample((stereo[1] * scale_r) as f32)),
                    ];
                    out_src[i][0] += output[0];
                    out_src[i][1] += output[1];
                    if let Some(scope) = out_scope.as_deref_mut() {
                        scope[i] += stereo_mid_f32(output);
                    }
                    last = output;
                    processed += 1;
                    let mod_inactive = (i + 1 >= mod_frames) && !self.modular.is_active();
                    if mod_inactive && self.rack_svf.quiet() {
                        self.active = false;
                        break;
                    }
                }
                self.rack_svf_ctrl =
                    self.rack_svf_ctrl.wrapping_add(processed as u32) & self.rack_svf_mask;
                self.age = self.age.saturating_add(processed as u32);
                self.last_output = last;
                return;
            }
        }

        // General per-frame Rack-bus smoothing / SVF update path (bit-exact with `next_stereo`).
        for i in 0..frames {
            let raw = raw_buf[i];
            let gain_scale = self.gain_scale.next();
            let rack_pan = self.rack_pan_sm.next();
            let fc = self.rack_cutoff.step(self.rack_cutoff_coeff);
            let res = self.rack_res_sm.next() as f32;
            let filter_active = fc < 19_900.0 || res > 0.001;

            let mut stereo = [f64::from(raw[0]), f64::from(raw[1])];
            if filter_active {
                let svf_key = (fc.to_bits(), res.to_bits());
                let need_coeff = !self.rack_svf_was_active
                    || (self.rack_svf_ctrl == 0 && svf_key != self.cached_rack_svf_key);
                if need_coeff {
                    self.cached_rack_svf_key = svf_key;
                    self.rack_svf_coeffs = SvfCoeffs::new(self.sample_rate, fc, res);
                }
                self.rack_svf_ctrl = self.rack_svf_ctrl.wrapping_add(1) & self.rack_svf_mask;
                self.rack_svf_was_active = true;
                stereo =
                    self.rack_svf
                        .process_stereo(stereo, self.rack_svf_coeffs, FilterMode::LowPass);
            } else if self.rack_svf_was_active {
                self.rack_svf.reset();
                self.rack_svf_was_active = false;
            }

            let rack_pan_bits = rack_pan.to_bits();
            if rack_pan_bits != self.cached_rack_pan_bits {
                self.cached_rack_pan_bits = rack_pan_bits;
                self.cached_rack_pan_lr = if rack_pan == 0.0 {
                    [1.0, 1.0]
                } else {
                    pan_stereo_balance_f64(rack_pan)
                };
            }
            let [rpl, rpr] = self.cached_rack_pan_lr;
            let output = [
                flush_denormal_f32(sanitize_sample((stereo[0] * rpl * gain_scale) as f32)),
                flush_denormal_f32(sanitize_sample((stereo[1] * rpr * gain_scale) as f32)),
            ];
            out_src[i][0] += output[0];
            out_src[i][1] += output[1];
            if let Some(scope) = out_scope.as_deref_mut() {
                scope[i] += stereo_mid_f32(output);
            }
            self.age = self.age.saturating_add(1);
            self.last_output = output;
            let mod_inactive = (i + 1 >= mod_frames) && !self.modular.is_active();
            if mod_inactive && self.rack_svf.quiet() {
                self.active = false;
                break;
            }
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct HeldNotes {
    pub(super) track_id: Option<u64>,
    pub(super) keys: [u8; 128],
    pub(super) velocities: [f32; 128],
    pub(super) len: usize,
}

impl Default for HeldNotes {
    fn default() -> Self {
        Self {
            track_id: None,
            keys: [0; 128],
            velocities: [0.0; 128],
            len: 0,
        }
    }
}

impl HeldNotes {
    pub(super) fn remove(&mut self, midi: u8) -> bool {
        let Some(index) = self.keys[..self.len].iter().position(|&key| key == midi) else {
            return false;
        };
        let was_last = index + 1 == self.len;
        self.keys.copy_within(index + 1..self.len, index);
        self.velocities.copy_within(index + 1..self.len, index);
        self.len -= 1;
        was_last
    }

    pub(super) fn press(&mut self, midi: u8, velocity: f32) {
        self.remove(midi);
        if self.len < self.keys.len() {
            self.keys[self.len] = midi;
            self.velocities[self.len] = velocity;
            self.len += 1;
        }
    }

    pub(super) fn last_matching(&self, pred: impl Fn(u8, f32) -> bool) -> Option<(u8, f32)> {
        (0..self.len).rev().find_map(|index| {
            let key = self.keys[index];
            let velocity = self.velocities[index];
            pred(key, velocity).then_some((key, velocity))
        })
    }
}
