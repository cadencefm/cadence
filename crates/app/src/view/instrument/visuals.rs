//! Pre-sampled DSP visualization builders for each Rack module kind and the pinned overview.
use super::{sanitize_wave, ModuleVisualView, RACK_ENV_POINTS};
use crate::App;
use cadence_core::dsp::cubic::modal::body_ratio;
use cadence_core::dsp::cubic::MAX_ATOMS;
use cadence_core::dsp::{
    drive_transfer_sample, eval_modular_env_curve, filter_magnitude_db, filter_mix_magnitude_db,
    fm_depth_from_amount, ideal_waveform, lfo_shape_sample, lofi_transfer_sample, log_to_norm,
    midi_to_hz, norm_to_log, phase_wrap, resonator_mode_descriptors, sanitize_sample_rate,
    semitones_to_ratio_f64, unison_detune_ratio, NYQUIST_SAFE_RATIO,
};
use cadence_core::material::Id;
use cadence_core::params::{format_hz, InstrumentParam, RackParam};
use cadence_core::rack::{
    module_sample_key, AeroMode, AeroModule, DriveModule, EnvMode, EnvModule, FilterMode,
    FilterModule, InPort, LfoModule, LfoShape, LoFiModule, ModuleKind, ModuleKindTag, NoiseColor,
    NoiseModule, OutModule, PitchTracking, RackModule, RackPatch, RackZone, ResonatorModule,
    SamplerModule, VcaModule, VcoModule,
};
use cadence_core::runtime::AudioSnapshot;

const RACK_WAVE_POINTS: usize = 96;
const RACK_FM_WAVE_POINTS: usize = 256;
const RACK_FILTER_POINTS: usize = 96;
const RACK_TRANSFER_POINTS: usize = 96;

struct VisualCtx<'a> {
    inst_id: Id,
    rack: &'a RackPatch,
    zone: &'a RackZone,
    sr: f64,
    high_hz: f32,
    query_time: f64,
}

pub(super) fn empty_pinned_overview(live: &[f32]) -> ModuleVisualView {
    ModuleVisualView::PinnedOverview {
        pin_count: 0,
        osc_label: None,
        osc_curve: Vec::new(),
        filter_label: None,
        filter_curve: Vec::new(),
        filter_cutoff_norm: 1.0,
        filter_mod_cutoff_norm: None,
        env_label: None,
        env_curve: Vec::new(),
        live_waveform: sanitize_wave(live),
    }
}

/// Sample 2 cycles (or FM sub-harmonic window) of a `VcoModule` strictly via `ideal_waveform`.
fn sample_vco_curve(vco: &VcoModule, fm_source: Option<(&VcoModule, f32)>) -> (Vec<f32>, bool) {
    let phase_a = f64::from(vco.phase_deg) / 360.0;
    if let Some((mod_vco, fm_amount)) = fm_source.filter(|(_, amt)| amt.abs() > 0.005) {
        let fm_abs = fm_amount.abs().clamp(0.0, 1.0);
        let sign = fm_amount.signum();
        let delta_cents = f64::from(mod_vco.tune_semitones - vco.tune_semitones) * 100.0
            + f64::from(mod_vco.fine_cents - vco.fine_cents);
        let fm_ratio = unison_detune_ratio(delta_cents).clamp(0.0625, 16.0);
        let fm_depth = fm_depth_from_amount(fm_abs) * f64::from(sign);
        let cycles = if fm_ratio < 0.5 {
            (1.0 / fm_ratio).clamp(2.0, 8.0)
        } else {
            2.0
        };
        let phase_b = f64::from(mod_vco.phase_deg) / 360.0;
        let curve = (0..RACK_FM_WAVE_POINTS)
            .map(|i| {
                let raw_t = (i as f64 / RACK_FM_WAVE_POINTS as f64) * cycles;
                let mod_sig = f64::from(ideal_waveform(
                    mod_vco.waveform,
                    phase_wrap(raw_t * fm_ratio + phase_b) as f32,
                    mod_vco.reach,
                    mod_vco.shape,
                    mod_vco.pulse_width,
                ));
                let phase = phase_wrap(raw_t + phase_a + mod_sig * fm_depth) as f32;
                ideal_waveform(vco.waveform, phase, vco.reach, vco.shape, vco.pulse_width)
            })
            .collect();
        (curve, true)
    } else {
        // Half-open [0, 2) sampling prevents right-edge phase == 1.0 wrap discontinuities.
        let curve = (0..RACK_WAVE_POINTS)
            .map(|i| {
                let phase = phase_wrap((i as f64 / RACK_WAVE_POINTS as f64) * 2.0 + phase_a) as f32;
                ideal_waveform(vco.waveform, phase, vco.reach, vco.shape, vco.pulse_width)
            })
            .collect();
        (curve, false)
    }
}

fn sample_lfo_curve(shape: LfoShape, bipolar: bool) -> Vec<f32> {
    // Deterministic pseudo-random table for Sample & Hold visualization across 8 steps (2 cycles).
    const SH_STEPS: [f32; 8] = [0.65, -0.45, 0.90, -0.15, -0.80, 0.35, 0.10, -0.60];
    (0..RACK_WAVE_POINTS)
        .map(|i| {
            let t = (i as f64 / RACK_WAVE_POINTS as f64) * 2.0;
            let phase = phase_wrap(t) as f32;
            let raw = match shape {
                LfoShape::SampleHold => {
                    let step = ((t * 4.0).floor() as usize).min(SH_STEPS.len() - 1);
                    SH_STEPS[step]
                }
                other => lfo_shape_sample(other, phase),
            };
            if bipolar {
                raw.clamp(-1.0, 1.0)
            } else {
                (raw * 0.5 + 0.5).clamp(0.0, 1.0)
            }
        })
        .collect()
}

fn sample_filter_curve(
    mode: FilterMode,
    cutoff_hz: f32,
    resonance: f32,
    mix: f32,
    sr: f64,
    high_hz: f32,
) -> Vec<f32> {
    (0..RACK_FILTER_POINTS)
        .map(|i| {
            let x = i as f32 / (RACK_FILTER_POINTS - 1) as f32;
            let freq = norm_to_log(x, 20.0, high_hz);
            let db = filter_mix_magnitude_db(mode, cutoff_hz, resonance, mix, f64::from(freq), sr);
            if db.is_finite() {
                db.clamp(-48.0, 18.0) as f32
            } else {
                -48.0
            }
        })
        .collect()
}

impl App {
    pub(super) fn build_module_visual(
        &self,
        inst_id: Id,
        rack: &RackPatch,
        zone: &RackZone,
        active_module_id: Option<u16>,
        snapshot: &AudioSnapshot,
        query_time: f64,
    ) -> ModuleVisualView {
        let sr = if snapshot.sample_rate < 8_000 {
            48_000.0
        } else {
            sanitize_sample_rate(f64::from(snapshot.sample_rate))
        };
        let high_hz = (sr * NYQUIST_SAFE_RATIO).clamp(4_000.0, 20_000.0) as f32;
        let live_waveform = sanitize_wave(snapshot.track_waveform(inst_id));
        let ctx = VisualCtx {
            inst_id,
            rack,
            zone,
            sr,
            high_hz,
            query_time,
        };

        let Some(mod_id) = active_module_id else {
            return self.build_pinned_overview_visual(&ctx, live_waveform);
        };
        let Some(base_mod) = zone.module(mod_id) else {
            return ModuleVisualView::Out {
                module_id: 0,
                name: "Out".into(),
                gain_db: 0.0,
                pan: 0.0,
                spread: 0.0,
                voice_summary: String::new(),
                live_waveform,
            };
        };
        let (eff_mod, modded) =
            self.live_effective_module(inst_id, rack, zone.id, base_mod, query_time);

        match (&base_mod.kind, &eff_mod.kind) {
            (ModuleKind::Vco(_), ModuleKind::Vco(v)) => {
                self.build_vco_visual(&ctx, base_mod, v, live_waveform)
            }
            (ModuleKind::Aero(_), ModuleKind::Aero(a)) => {
                build_aero_visual(&ctx, base_mod, a, live_waveform)
            }
            (ModuleKind::Sampler(_), ModuleKind::Sampler(s)) => {
                self.build_sampler_visual(&ctx, base_mod, s, live_waveform)
            }
            (ModuleKind::Noise(_), ModuleKind::Noise(n)) => {
                build_noise_visual(&ctx, base_mod, n, live_waveform)
            }
            (ModuleKind::Env(_), ModuleKind::Env(e)) => {
                build_env_visual(base_mod, e, live_waveform)
            }
            (ModuleKind::Lfo(_), ModuleKind::Lfo(l)) => {
                self.build_lfo_visual(base_mod, l, live_waveform)
            }
            (ModuleKind::Filter(base_f), ModuleKind::Filter(eff_f)) => {
                build_filter_visual(&ctx, base_mod, base_f, eff_f, modded, live_waveform)
            }
            (ModuleKind::Vca(_), ModuleKind::Vca(v)) => {
                build_vca_visual(&ctx, base_mod, v, live_waveform)
            }
            (ModuleKind::Drive(_), ModuleKind::Drive(d)) => {
                build_drive_visual(base_mod, d, live_waveform)
            }
            (ModuleKind::LoFi(_), ModuleKind::LoFi(l)) => {
                build_lofi_visual(base_mod, l, live_waveform)
            }
            (ModuleKind::Resonator(_), ModuleKind::Resonator(r)) => {
                build_resonator_visual(&ctx, base_mod, r, live_waveform)
            }
            (ModuleKind::Out(_), ModuleKind::Out(o)) => {
                build_out_visual(&ctx, base_mod, o, live_waveform)
            }
            _ => unreachable!("live_effective_module preserves module kind"),
        }
    }

    fn resolve_vco_fm_source(
        &self,
        ctx: &VisualCtx<'_>,
        target_module_id: u16,
    ) -> Option<(VcoModule, f32)> {
        ctx.zone.cables.iter().find_map(|c| {
            if c.to_module == target_module_id && c.to_port == InPort::FmIn {
                let eff_amt = self
                    .resolve_live_rack_param(
                        ctx.inst_id,
                        InstrumentParam::Cable {
                            zone: ctx.zone.id,
                            cable: c.id,
                        },
                        ctx.query_time,
                    )
                    .map(|norm| {
                        cadence_core::rack::denormalize_cable_amount(
                            ModuleKindTag::Vco,
                            InPort::FmIn,
                            norm,
                        )
                    })
                    .unwrap_or(c.amount);
                let src = ctx.zone.module(c.from_module)?;
                if !matches!(src.kind, ModuleKind::Vco(_)) {
                    return None;
                }
                let (eff_src, _) = self.live_effective_module(
                    ctx.inst_id,
                    ctx.rack,
                    ctx.zone.id,
                    src,
                    ctx.query_time,
                );
                match eff_src.kind {
                    ModuleKind::Vco(sv) => Some((sv, eff_amt)),
                    _ => None,
                }
            } else {
                None
            }
        })
    }

    fn build_pinned_overview_visual(
        &self,
        ctx: &VisualCtx<'_>,
        live_waveform: Vec<f32>,
    ) -> ModuleVisualView {
        let (osc_label, osc_curve) = ctx
            .zone
            .modules
            .iter()
            .find(|m| matches!(m.kind, ModuleKind::Vco(_)))
            .map(|m| {
                let (eff, _) = self.live_effective_module(
                    ctx.inst_id,
                    ctx.rack,
                    ctx.zone.id,
                    m,
                    ctx.query_time,
                );
                let ModuleKind::Vco(v) = &eff.kind else {
                    unreachable!()
                };
                let fm_src = self.resolve_vco_fm_source(ctx, m.id);
                let (curve, fm) = sample_vco_curve(v, fm_src.as_ref().map(|(sv, amt)| (sv, *amt)));
                let lbl = if fm {
                    format!("{} · {} · FM", m.name, v.waveform.label())
                } else {
                    format!("{} · {}", m.name, v.waveform.label())
                };
                (Some(lbl), curve)
            })
            .unwrap_or((None, Vec::new()));

        let (filter_label, filter_curve, filter_cutoff_norm, filter_mod_cutoff_norm) =
            if let Some(fm) = ctx
                .zone
                .modules
                .iter()
                .find(|m| matches!(m.kind, ModuleKind::Filter(_)))
            {
                let ModuleKind::Filter(base_f) = &fm.kind else {
                    unreachable!()
                };
                let (eff, modded) = self.live_effective_module(
                    ctx.inst_id,
                    ctx.rack,
                    ctx.zone.id,
                    fm,
                    ctx.query_time,
                );
                let ModuleKind::Filter(eff_f) = &eff.kind else {
                    unreachable!()
                };
                let base_norm = log_to_norm(base_f.cutoff_hz, 20.0, ctx.high_hz);
                let mod_norm = (modded && (eff_f.cutoff_hz - base_f.cutoff_hz).abs() > 0.5)
                    .then(|| log_to_norm(eff_f.cutoff_hz, 20.0, ctx.high_hz));
                let lbl = match mod_norm {
                    Some(_) => format!(
                        "{} · {} · {} → {}",
                        fm.name,
                        eff_f.mode.label(),
                        format_hz(base_f.cutoff_hz),
                        format_hz(eff_f.cutoff_hz)
                    ),
                    None => format!(
                        "{} · {} · {}",
                        fm.name,
                        eff_f.mode.label(),
                        format_hz(base_f.cutoff_hz)
                    ),
                };
                let curve = sample_filter_curve(
                    eff_f.mode,
                    eff_f.cutoff_hz,
                    eff_f.resonance,
                    eff_f.mix,
                    ctx.sr,
                    ctx.high_hz,
                );
                (Some(lbl), curve, base_norm, mod_norm)
            } else {
                // Fall back to Rack bus filter response.
                let live_cut = self
                    .resolve_live_rack_param(
                        ctx.inst_id,
                        InstrumentParam::Rack {
                            param: RackParam::Cutoff,
                        },
                        ctx.query_time,
                    )
                    .map(|n| {
                        let mut tmp = ctx.rack.clone();
                        let _ = tmp.write_param_normalized(
                            InstrumentParam::Rack {
                                param: RackParam::Cutoff,
                            },
                            n,
                            0,
                        );
                        tmp.cutoff_hz
                    });
                let eff_cut = live_cut.unwrap_or(ctx.rack.cutoff_hz);
                let base_norm = log_to_norm(ctx.rack.cutoff_hz, 20.0, ctx.high_hz);
                let mod_norm = live_cut.map(|hz| log_to_norm(hz, 20.0, ctx.high_hz));
                let lbl = format!("RACK FILTER · {}", format_hz(eff_cut));
                let curve = sample_filter_curve(
                    FilterMode::ImLpf2,
                    eff_cut,
                    ctx.rack.resonance,
                    1.0,
                    ctx.sr,
                    ctx.high_hz,
                );
                (Some(lbl), curve, base_norm, mod_norm)
            };

        let (env_label, env_curve) = ctx
            .zone
            .modules
            .iter()
            .find(|m| matches!(m.kind, ModuleKind::Env(_)))
            .map(|m| {
                let (eff, _) = self.live_effective_module(
                    ctx.inst_id,
                    ctx.rack,
                    ctx.zone.id,
                    m,
                    ctx.query_time,
                );
                let ModuleKind::Env(e) = &eff.kind else {
                    unreachable!()
                };
                let curve = eval_modular_env_curve(e.to_dsp_params(), RACK_ENV_POINTS);
                let lbl = match e.mode {
                    EnvMode::Capacitor => {
                        format!("{} · Cap {:.1}/{:.0}ms", m.name, e.attack_ms, e.decay_ms)
                    }
                    _ => format!(
                        "{} · {:.0}/{:.0}/{:.0}%/{:.0}",
                        m.name,
                        e.attack_ms,
                        e.decay_ms,
                        e.sustain * 100.0,
                        e.release_ms
                    ),
                };
                (Some(lbl), curve)
            })
            .unwrap_or((None, Vec::new()));

        ModuleVisualView::PinnedOverview {
            pin_count: ctx.rack.panel_pins.len(),
            osc_label,
            osc_curve,
            filter_label,
            filter_curve,
            filter_cutoff_norm,
            filter_mod_cutoff_norm,
            env_label,
            env_curve,
            live_waveform,
        }
    }

    fn build_vco_visual(
        &self,
        ctx: &VisualCtx<'_>,
        base_mod: &RackModule,
        v: &VcoModule,
        live_waveform: Vec<f32>,
    ) -> ModuleVisualView {
        let fm_src = self.resolve_vco_fm_source(ctx, base_mod.id);
        let (curve, fm_active) = sample_vco_curve(v, fm_src.as_ref().map(|(sv, amt)| (sv, *amt)));
        let label = if fm_active {
            format!("{} · {} · FM", base_mod.name, v.waveform.label())
        } else {
            format!(
                "{} · {} · {:+.1}st",
                base_mod.name,
                v.waveform.label(),
                v.tune_semitones
            )
        };
        ModuleVisualView::Vco {
            module_id: base_mod.id,
            name: base_mod.name.clone(),
            label,
            waveform: v.waveform,
            waveform_label: v.waveform.label(),
            reach: v.reach,
            shape: v.shape,
            chirality: v.chirality,
            pulse_width: v.pulse_width,
            curve,
            fm_active,
            unison_voices: v.unison_voices,
            active: v.level > 0.0,
            live_waveform,
        }
    }

    fn build_sampler_visual(
        &self,
        ctx: &VisualCtx<'_>,
        base_mod: &RackModule,
        s: &SamplerModule,
        live_waveform: Vec<f32>,
    ) -> ModuleVisualView {
        let key = module_sample_key(ctx.inst_id, ctx.zone.id, base_mod.id);
        if let Some(sample) = self.assets.samples.get(&key) {
            let frames = sample.frames.max(1);
            let start = s.sample_start.min(frames.saturating_sub(1));
            let end = if s.sample_end == 0 {
                frames
            } else {
                s.sample_end.clamp(start + 1, frames)
            };
            let loop_end = if s.loop_end == 0 {
                frames
            } else {
                s.loop_end.min(frames)
            };
            let loop_start = s.loop_start.min(loop_end.saturating_sub(1));
            ModuleVisualView::Sampler {
                module_id: base_mod.id,
                name: base_mod.name.clone(),
                sample_name: Some(sample.name.clone()),
                duration_sec: sample.duration_seconds(),
                peaks: sample.peaks.to_vec(),
                frames,
                sample_start: start as f32 / frames as f32,
                sample_end: end as f32 / frames as f32,
                loop_mode: s.loop_mode,
                loop_start: loop_start as f32 / frames as f32,
                loop_end: loop_end as f32 / frames as f32,
                live_waveform,
            }
        } else {
            ModuleVisualView::Sampler {
                module_id: base_mod.id,
                name: base_mod.name.clone(),
                sample_name: None,
                duration_sec: 0.0,
                peaks: Vec::new(),
                frames: 0,
                sample_start: 0.0,
                sample_end: 1.0,
                loop_mode: s.loop_mode,
                loop_start: 0.0,
                loop_end: 1.0,
                live_waveform,
            }
        }
    }

    fn build_lfo_visual(
        &self,
        base_mod: &RackModule,
        l: &LfoModule,
        live_waveform: Vec<f32>,
    ) -> ModuleVisualView {
        let rate_label = if l.sync {
            format!(
                "{:.1} steps ({:.2} Hz)",
                l.division_steps,
                l.effective_hz(self.project().bpm)
            )
        } else {
            format!("{:.2} Hz", l.rate_hz)
        };
        ModuleVisualView::Lfo {
            module_id: base_mod.id,
            name: base_mod.name.clone(),
            shape_label: l.shape.label(),
            bipolar: l.bipolar,
            sync: l.sync,
            rate_label,
            curve: sample_lfo_curve(l.shape, l.bipolar),
            live_waveform,
        }
    }
}

fn build_aero_visual(
    ctx: &VisualCtx<'_>,
    base_mod: &RackModule,
    a: &AeroModule,
    live_waveform: Vec<f32>,
) -> ModuleVisualView {
    let semi = f64::from(a.tune_semitones) + f64::from(a.fine_cents) * 0.01;
    let base_hz = match a.pitch_tracking {
        PitchTracking::Fixed => {
            (f64::from(a.fixed_hz.max(20.0)) * semitones_to_ratio_f64(semi)).clamp(20.0, 12_000.0)
        }
        PitchTracking::Chromatic => {
            midi_to_hz(f64::from(ctx.zone.root_midi) + semi).clamp(20.0, 12_000.0)
        }
    };
    let geom = a.geometry.to_cubic();
    let req_n = usize::from(a.atoms).clamp(1, MAX_ATOMS);
    let mut n = req_n;
    while n > 1 && base_hz * body_ratio(geom, n - 1, req_n) > 0.44 * ctx.sr {
        n -= 1;
    }
    let brightness = f64::from(a.brightness.clamp(0.0, 1.0));
    let absorption = f64::from(a.absorption.clamp(0.0, 0.5));
    let radiation = f64::from(a.radiation.clamp(0.0, 1.0));
    let tilt_exp = -1.45 + 2.55 * brightness;
    let denom_n = n.saturating_sub(1).max(1) as f64;
    let mut raw_modes = Vec::with_capacity(n);
    let mut max_amp = 1e-9_f64;
    for k in 0..n {
        let ratio = body_ratio(geom, k, req_n);
        let freq_hz = (base_hz * ratio).clamp(20.0, 20_000.0) as f32;
        let freq_norm = log_to_norm(freq_hz, 20.0, 20_000.0);
        let raw_amp = match a.mode {
            AeroMode::FlueJet => {
                let drv_w = ((k + 1) as f64).powf(-0.78);
                let rad_w = ratio.powf(tilt_exp);
                let damp = 1.0
                    + (0.4 + 2.0 * absorption) * 0.18 * (k as f64)
                    + 0.15 * radiation * ratio.powf(0.45);
                (drv_w * rad_w / damp).max(0.0)
            }
            AeroMode::BrassReed => {
                let mode_sc = 0.78_f64.powi(k as i32);
                let bright_shelf = 1.0 + 0.9 * brightness * (k as f64 / denom_n);
                let damp = 1.0
                    + (0.4 + 2.0 * absorption) * 0.15 * (k as f64)
                    + 0.12 * radiation * ratio.powf(0.45);
                (mode_sc * bright_shelf / damp).max(0.0)
            }
        };
        max_amp = max_amp.max(raw_amp);
        raw_modes.push((freq_norm, raw_amp));
    }
    let mode_bars = raw_modes
        .into_iter()
        .map(|(x, raw)| (x, (raw / max_amp).clamp(0.0, 1.0) as f32))
        .collect();

    ModuleVisualView::Aero {
        module_id: base_mod.id,
        name: base_mod.name.clone(),
        mode: a.mode,
        geometry: a.geometry,
        atoms: a.atoms,
        base_hz: base_hz as f32,
        drive: a.drive,
        embouchure: a.embouchure,
        p_beat_pa: a.p_beat_pa,
        brightness: a.brightness,
        absorption: a.absorption,
        radiation: a.radiation,
        kerr_semitones: a.kerr_semitones,
        triad_amount: a.triad_amount,
        spread: a.spread,
        mode_bars,
        live_waveform,
    }
}

fn build_noise_visual(
    ctx: &VisualCtx<'_>,
    base_mod: &RackModule,
    n: &NoiseModule,
    live_waveform: Vec<f32>,
) -> ModuleVisualView {
    let band_curve = (0..RACK_FILTER_POINTS)
        .map(|i| {
            let x = i as f32 / (RACK_FILTER_POINTS - 1) as f32;
            let freq = norm_to_log(x, 20.0, ctx.high_hz);
            let hp = filter_magnitude_db(
                FilterMode::HighPass,
                n.highpass_hz,
                0.0,
                f64::from(freq),
                ctx.sr,
            );
            let lp = filter_magnitude_db(
                FilterMode::ImLpf2,
                n.lowpass_hz,
                0.0,
                f64::from(freq),
                ctx.sr,
            );
            let pink_tilt = if matches!(n.color, NoiseColor::Pink) {
                -3.0 * (f64::from(freq) / 1000.0).max(1e-3).log2()
            } else {
                0.0
            };
            let db = hp + lp + pink_tilt;
            if db.is_finite() {
                db.clamp(-48.0, 12.0) as f32
            } else {
                -48.0
            }
        })
        .collect();
    ModuleVisualView::Noise {
        module_id: base_mod.id,
        name: base_mod.name.clone(),
        color_label: n.color.label(),
        highpass_hz: n.highpass_hz,
        lowpass_hz: n.lowpass_hz,
        band_curve,
        highpass_norm: log_to_norm(n.highpass_hz, 20.0, ctx.high_hz),
        lowpass_norm: log_to_norm(n.lowpass_hz, 20.0, ctx.high_hz),
        live_waveform,
    }
}

fn build_env_visual(
    base_mod: &RackModule,
    e: &EnvModule,
    live_waveform: Vec<f32>,
) -> ModuleVisualView {
    let curve = eval_modular_env_curve(e.to_dsp_params(), RACK_ENV_POINTS);
    let label = if e.mode == EnvMode::Capacitor {
        if e.bursts > 1 {
            format!(
                "{} · {}x · chg {:.1}ms · dec {:.0}ms",
                base_mod.name, e.bursts, e.attack_ms, e.decay_ms
            )
        } else {
            format!(
                "{} · chg {:.1}ms · dec {:.0}ms",
                base_mod.name, e.attack_ms, e.decay_ms
            )
        }
    } else if e.bursts > 1 {
        format!(
            "{} · {} · {}x · {:.0}/{:.0}ms",
            base_mod.name,
            e.mode.label(),
            e.bursts,
            e.attack_ms,
            e.decay_ms
        )
    } else {
        format!(
            "{} · {} · {:.0}/{:.0}/{:.0}%/{:.0}",
            base_mod.name,
            e.mode.label(),
            e.attack_ms,
            e.decay_ms,
            e.sustain * 100.0,
            e.release_ms
        )
    };
    ModuleVisualView::Env {
        module_id: base_mod.id,
        name: base_mod.name.clone(),
        label,
        mode: e.mode,
        mode_label: e.mode.label(),
        trigger_mode: e.trigger_mode,
        cubic_limit: e.cubic_limit,
        curve,
        bursts: e.bursts,
        live_waveform,
    }
}

fn build_filter_visual(
    ctx: &VisualCtx<'_>,
    base_mod: &RackModule,
    base_f: &FilterModule,
    eff_f: &FilterModule,
    modded: bool,
    live_waveform: Vec<f32>,
) -> ModuleVisualView {
    let cutoff_norm = log_to_norm(base_f.cutoff_hz, 20.0, ctx.high_hz);
    let mod_cutoff_norm = (modded && (eff_f.cutoff_hz - base_f.cutoff_hz).abs() > 0.5)
        .then(|| log_to_norm(eff_f.cutoff_hz, 20.0, ctx.high_hz));
    let label = match mod_cutoff_norm {
        Some(_) => format!(
            "{} · {} · {} → {}",
            base_mod.name,
            eff_f.mode.label(),
            format_hz(base_f.cutoff_hz),
            format_hz(eff_f.cutoff_hz)
        ),
        None => format!(
            "{} · {} · {}",
            base_mod.name,
            eff_f.mode.label(),
            format_hz(base_f.cutoff_hz)
        ),
    };
    let curve = sample_filter_curve(
        eff_f.mode,
        eff_f.cutoff_hz,
        eff_f.resonance,
        eff_f.mix,
        ctx.sr,
        ctx.high_hz,
    );
    ModuleVisualView::Filter {
        module_id: base_mod.id,
        name: base_mod.name.clone(),
        label,
        mode_label: eff_f.mode.label(),
        curve,
        cutoff_norm,
        mod_cutoff_norm,
        kerr_semitones: eff_f.kerr_semitones,
        spread: eff_f.spread,
        mix: eff_f.mix,
        live_waveform,
    }
}

fn build_vca_visual(
    ctx: &VisualCtx<'_>,
    base_mod: &RackModule,
    v: &VcaModule,
    live_waveform: Vec<f32>,
) -> ModuleVisualView {
    let cv_cables: Vec<&cadence_core::rack::PatchCable> = ctx
        .zone
        .cables
        .iter()
        .filter(|c| c.to_module == base_mod.id && c.to_port == InPort::Cv)
        .collect();
    let cv_patched = !cv_cables.is_empty();
    let ring_mod = cv_cables.iter().any(|c| {
        ctx.zone.module(c.from_module).is_some_and(|src| {
            matches!(
                src.kind,
                ModuleKind::Vco(_)
                    | ModuleKind::Aero(_)
                    | ModuleKind::Sampler(_)
                    | ModuleKind::Noise(_)
            )
        })
    });
    let transfer_curve = (0..RACK_TRANSFER_POINTS)
        .map(|i| {
            let t = i as f32 / (RACK_TRANSFER_POINTS - 1) as f32;
            let cv = if ring_mod { t * 2.0 - 1.0 } else { t };
            if cv_patched {
                (v.gain * (v.bias + cv)).clamp(-2.0, 2.0)
            } else {
                v.gain.clamp(0.0, 2.0)
            }
        })
        .collect();
    ModuleVisualView::Vca {
        module_id: base_mod.id,
        name: base_mod.name.clone(),
        gain: v.gain,
        bias: v.bias,
        cv_patched,
        ring_mod,
        transfer_curve,
        live_waveform,
    }
}

fn build_drive_visual(
    base_mod: &RackModule,
    d: &DriveModule,
    live_waveform: Vec<f32>,
) -> ModuleVisualView {
    let patch = d.to_drive_patch();
    let transfer_curve = (0..RACK_TRANSFER_POINTS)
        .map(|i| {
            let x = (i as f32 / (RACK_TRANSFER_POINTS - 1) as f32) * 2.0 - 1.0;
            drive_transfer_sample(x, &patch).clamp(-1.5, 1.5)
        })
        .collect();
    let label = if d.asymmetry.abs() >= 0.005 {
        format!(
            "{} · {} · drive {:.0}% · asym {:+.0}% · mix {:.0}%",
            base_mod.name,
            d.mode.label(),
            d.drive * 100.0,
            d.asymmetry * 100.0,
            d.mix * 100.0
        )
    } else {
        format!(
            "{} · {} · drive {:.0}% · mix {:.0}%",
            base_mod.name,
            d.mode.label(),
            d.drive * 100.0,
            d.mix * 100.0
        )
    };
    ModuleVisualView::Drive {
        module_id: base_mod.id,
        name: base_mod.name.clone(),
        label,
        mode: d.mode,
        transfer_curve,
        live_waveform,
    }
}

fn build_lofi_visual(
    base_mod: &RackModule,
    l: &LoFiModule,
    live_waveform: Vec<f32>,
) -> ModuleVisualView {
    let transfer_curve = (0..RACK_TRANSFER_POINTS)
        .map(|i| {
            let x = (i as f32 / (RACK_TRANSFER_POINTS - 1) as f32) * 2.0 - 1.0;
            lofi_transfer_sample(x, l.bits, l.mix).clamp(-1.2, 1.2)
        })
        .collect();
    ModuleVisualView::LoFi {
        module_id: base_mod.id,
        name: base_mod.name.clone(),
        bits: l.bits,
        rate_hz: l.rate_hz,
        mix: l.mix,
        transfer_curve,
        live_waveform,
    }
}

fn build_resonator_visual(
    ctx: &VisualCtx<'_>,
    base_mod: &RackModule,
    r: &ResonatorModule,
    live_waveform: Vec<f32>,
) -> ModuleVisualView {
    let semi = f64::from(r.tune_semitones) + f64::from(r.fine_cents) * 0.01;
    let base_hz = match r.pitch_tracking {
        PitchTracking::Fixed => {
            (f64::from(r.fixed_hz.max(20.0)) * semitones_to_ratio_f64(semi)).clamp(20.0, 12_000.0)
        }
        PitchTracking::Chromatic => {
            midi_to_hz(f64::from(ctx.zone.root_midi) + semi).clamp(20.0, 12_000.0)
        }
    };
    let descs = resonator_mode_descriptors(
        r.mode,
        base_hz,
        r.structure,
        r.decay_ms,
        r.brightness,
        r.position,
        ctx.sr,
    );
    let max_gain = descs
        .iter()
        .filter(|d| d.active)
        .map(|d| d.gain)
        .fold(1e-6_f64, f64::max);
    let max_decay = descs
        .iter()
        .filter(|d| d.active)
        .map(|d| d.decay_s)
        .fold(1e-3_f64, f64::max);
    let modes = descs
        .iter()
        .filter(|d| d.active && d.gain > 0.0)
        .map(|d| {
            let x = log_to_norm(d.freq_hz as f32, 20.0, 20_000.0);
            let h = (d.gain / max_gain).clamp(0.0, 1.0) as f32;
            let dec = (d.decay_s / max_decay).clamp(0.0, 1.0) as f32;
            (x, h, dec)
        })
        .collect();
    ModuleVisualView::Resonator {
        module_id: base_mod.id,
        name: base_mod.name.clone(),
        mode_label: r.mode.label(),
        base_hz: base_hz as f32,
        decay_ms: r.decay_ms,
        brightness: r.brightness,
        position: r.position,
        mix: r.mix,
        modes,
        live_waveform,
    }
}

fn build_out_visual(
    ctx: &VisualCtx<'_>,
    base_mod: &RackModule,
    o: &OutModule,
    live_waveform: Vec<f32>,
) -> ModuleVisualView {
    let v = &ctx.zone.voice;
    let voice_summary = format!(
        "{} · {} · glide {:.0}ms{}",
        v.voice_mode.label(),
        v.pitch_tracking.label(),
        v.glide_ms,
        if v.legato { " · legato" } else { "" }
    );
    ModuleVisualView::Out {
        module_id: base_mod.id,
        name: base_mod.name.clone(),
        gain_db: o.gain_db,
        pan: o.pan,
        spread: o.spread,
        voice_summary,
        live_waveform,
    }
}
