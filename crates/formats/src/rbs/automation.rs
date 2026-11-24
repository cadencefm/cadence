//! Song-Mode automation compilation: Master & channel gain/mute, Delay, TB-303, Compressor,
//! Distortion, TR-808/TR-909 per-zone drums, and PCF.

use super::calibration::{
    ensure_zone_filter, find_drum_zone_index, find_zone_filter, find_zone_noise,
    rbs_channel_gain_scale, rbs_delay_feedback, rbs_delay_steps, rbs_dist_drive,
    rbs_dist_output_db, rbs_dist_tone_hz, rbs_level, rbs_master_gain, rbs_pan,
    rbs_presence_cutoff_hz, resolve_tb303_targets, zone_is_sample, RBS_ECHO_RETURN_GAIN,
    RBS_ECHO_SEND_SCALE,
};
use super::controls::{
    mixer_ch_pan_ctrl, DrumSdPairSpec, DrumZoneControlSpec, COMP_CONTROLS, DELAY_CTRL_ENABLE,
    DELAY_CTRL_FEEDBACK, DELAY_CTRL_MODE, DELAY_CTRL_STEPS, PCF_CONTROLS, TB303_CONTROLS,
    TR808_SD_PAIR, TR808_ZONE_CONTROLS, TR909_SD_PAIR, TR909_ZONE_CONTROLS, TRACK_303_A, TRACK_808,
    TRACK_909, TRACK_DELAY, TRACK_MIXER,
};
use super::model::RbsSong;
use cadence_core::material::{
    InstrumentInstance, Modulation, ModulationTarget, Pattern, PatternKind, ValuePoint, ValueUnit,
    MAX_GLOBAL_MODULATIONS, MAX_PATTERNS,
};
use cadence_core::mixer::{Channel, ChannelParam, ChannelRef, EffectParam, MAX_MIXER_AUTOMATIONS};
use cadence_core::params::{InstrumentParam, ValueMode};
use cadence_core::rack::{drum_midi, ModuleKind, ModuleParam, RackPatch};
use std::collections::{BTreeMap, HashSet};

pub(super) fn series_val(map: &BTreeMap<u32, u8>, step: u32, fallback: u8) -> u8 {
    map.range(..=step).next_back().map_or(fallback, |(_, &v)| v)
}

pub(super) fn series_varies(map: &BTreeMap<u32, u8>) -> bool {
    let mut iter = map.values();
    let Some(&first) = iter.next() else {
        return false;
    };
    iter.any(|&v| v != first)
}

pub(super) fn union_step_keys(
    a: &BTreeMap<u32, u8>,
    b: &BTreeMap<u32, u8>,
    include_zero: bool,
) -> Vec<u32> {
    let mut steps: Vec<u32> = a.keys().chain(b.keys()).copied().collect();
    if include_zero {
        steps.push(0);
    }
    steps.sort_unstable();
    steps.dedup();
    steps
}

/// Classify whether a `(prev -> next)` breakpoint transition with step gap `dt` and normalized
/// jump `dv` represents a stepped jump (requiring a pre-step hold anchor at `next.step - 1`)
/// versus a continuous knob sweep (interpolating smoothly across the gap).
#[inline]
fn needs_hold_anchor(dt: u32, dv: f32) -> bool {
    if dt <= 1 {
        return false;
    }
    // Large instantaneous jump (>= 20% of full scale across >= 2 steps): stepped jump.
    if dv >= 0.20 {
        return true;
    }
    // Slow continuous sweep (small quantized ticks <= 5% within 1 bar): keep smooth ramp.
    if dv <= 0.05 && dt <= 16 {
        return false;
    }
    // Moderate step: smooth when dense (<= 4 steps), stepped jump when sparse (> 4 steps).
    dt > 4
}

/// Douglas-Peucker vertical-error simplification for `ValueMode::Ramp` breakpoints.
/// Drops redundant interior collinear points while guaranteeing every original point stays
/// within `tol` of the resulting piecewise-linear curve.
fn simplify_ramp_points(pts: &[ValuePoint], tol: f32) -> Vec<ValuePoint> {
    if pts.len() <= 2 {
        return pts.to_vec();
    }
    let mut keep = vec![false; pts.len()];
    keep[0] = true;
    keep[pts.len() - 1] = true;
    let mut stack = vec![(0usize, pts.len() - 1)];

    while let Some((lo, hi)) = stack.pop() {
        if hi <= lo + 1 {
            continue;
        }
        let a = pts[lo];
        let b = pts[hi];
        let span = f64::from(b.step.saturating_sub(a.step)).max(1.0);
        let mut worst_idx = None;
        let mut worst_err = tol;
        for (idx, pt) in pts.iter().enumerate().take(hi).skip(lo + 1) {
            let t = f64::from(pt.step.saturating_sub(a.step)) / span;
            let interp = a.value + (b.value - a.value) * (t as f32);
            let err = (pt.value - interp).abs();
            if err > worst_err {
                worst_err = err;
                worst_idx = Some(idx);
            }
        }
        if let Some(split) = worst_idx {
            keep[split] = true;
            stack.push((lo, split));
            stack.push((split, hi));
        }
    }

    pts.iter()
        .zip(keep)
        .filter_map(|(&pt, k)| k.then_some(pt))
        .collect()
}

/// Compress a step-indexed normalized automation timeline into Cadence `ValuePoint`s.
///
/// - `ValueMode::Hold`: removes redundant repeated values and emits exact step breakpoints.
/// - `ValueMode::Ramp`: distinguishes stepped jumps from slow continuous sweeps using both
///   `Δstep` and `|Δvalue|` (plus any explicit `forced_steps` such as channel mute toggles),
///   pins `total_steps - 1` so the final value never ramps back to step 0 on wrap, and prunes
///   redundant collinear interior ramp points within `1.5e-3` tolerance.
pub(super) fn compress_points(
    series: &BTreeMap<u32, f32>,
    mode: ValueMode,
    total_steps: u32,
    forced_steps: Option<&HashSet<u32>>,
) -> Vec<ValuePoint> {
    let mut raw: Vec<ValuePoint> = Vec::new();
    for (&step, &val) in series {
        if step >= total_steps {
            break;
        }
        let v = val.clamp(0.0, 1.0);
        if raw.last().is_none_or(|prev| (prev.value - v).abs() > 1e-4) {
            raw.push(ValuePoint { step, value: v });
        }
    }
    if raw.len() < 2 || mode == ValueMode::Hold {
        return raw;
    }

    let mut anchored: Vec<ValuePoint> = Vec::with_capacity(raw.len() + 4);
    anchored.push(raw[0]);
    for pt in &raw[1..] {
        let prev = *anchored.last().unwrap();
        let dt = pt.step.saturating_sub(prev.step);
        let dv = (pt.value - prev.value).abs();
        let is_forced = forced_steps.is_some_and(|set| set.contains(&pt.step));
        if dt > 1 && (is_forced || needs_hold_anchor(dt, dv)) {
            anchored.push(ValuePoint {
                step: pt.step - 1,
                value: prev.value,
            });
        }
        anchored.push(*pt);
    }
    if let Some(&last) = anchored.last() {
        if last.step + 1 < total_steps {
            anchored.push(ValuePoint {
                step: total_steps - 1,
                value: last.value,
            });
        }
    }
    simplify_ramp_points(&anchored, 1.5e-3)
}

type PushModFn<'a> =
    dyn FnMut(String, ModulationTarget, ValueMode, Vec<ValuePoint>, bool) -> bool + 'a;

/// Pre-extracted Song-Mode initial device state and FX/mixer control timelines shared between
/// channel insert instantiation (`convert.rs`) and Song-Mode modulation emission (`automation.rs`).
pub(super) struct FxTimelines {
    pub(super) total_steps: u32,
    pub(super) init_303: [[u8; 9]; 2],
    pub(super) init_808: [u8; 29],
    pub(super) init_909: [u8; 30],
    pub(super) init_delay: [u8; 5],
    pub(super) init_dist: [u8; 3],
    pub(super) init_pcf: [u8; 7],
    pub(super) init_comp: [u8; 3],
    pub(super) init_master_lvl: u8,
    pub(super) master_lvl_series: BTreeMap<u32, u8>,
    pub(super) init_comp_target: u8,
    pub(super) init_pcf_target: u8,
    pub(super) comp_tgt: BTreeMap<u32, u8>,
    pub(super) comp_en: BTreeMap<u32, u8>,
    pub(super) comp_params: [BTreeMap<u32, u8>; 2],
    pub(super) pcf_tgt: BTreeMap<u32, u8>,
    pub(super) pcf_en: BTreeMap<u32, u8>,
    pub(super) pcf_params: [BTreeMap<u32, u8>; 6],
    pub(super) dist_en: BTreeMap<u32, u8>,
    pub(super) dist_amt: BTreeMap<u32, u8>,
    pub(super) dist_shape: BTreeMap<u32, u8>,
    pub(super) ch_init_en: [u8; 4],
    pub(super) ch_en_series: [BTreeMap<u32, u8>; 4],
    pub(super) ch_init_lvl: [u8; 4],
    pub(super) ch_lvl_series: [BTreeMap<u32, u8>; 4],
    pub(super) ch_init_pan: [u8; 4],
    pub(super) ch_send_series: [BTreeMap<u32, u8>; 4],
    pub(super) ch_init_dist: [u8; 4],
    pub(super) ch_dist_series: [BTreeMap<u32, u8>; 4],
    pub(super) dist_activity_steps: [Vec<u32>; 4],
    pub(super) del_pan_series: BTreeMap<u32, u8>,
}

impl FxTimelines {
    pub(super) fn first_active_dist(&self, c: usize) -> Option<u32> {
        union_step_keys(&self.ch_dist_series[c], &self.dist_en, true)
            .into_iter()
            .find(|&s| {
                s < self.total_steps
                    && series_val(&self.ch_dist_series[c], s, self.ch_init_dist[c]) != 0
                    && series_val(&self.dist_en, s, self.init_dist[0]) != 0
            })
    }

    pub(super) fn first_active_pcf(&self, c: usize) -> Option<u32> {
        union_step_keys(&self.pcf_tgt, &self.pcf_en, true)
            .into_iter()
            .find(|&s| {
                s < self.total_steps
                    && series_val(&self.pcf_en, s, self.init_pcf[0]) != 0
                    && series_val(&self.pcf_tgt, s, self.init_pcf_target) == (c + 1) as u8
            })
    }

    pub(super) fn first_active_comp(&self, target_id: u8) -> Option<u32> {
        union_step_keys(&self.comp_tgt, &self.comp_en, true)
            .into_iter()
            .find(|&s| {
                s < self.total_steps
                    && series_val(&self.comp_en, s, self.init_comp[0]) != 0
                    && series_val(&self.comp_tgt, s, self.init_comp_target) == target_id
            })
    }
}

pub(super) struct AutomationRouting<'a> {
    pub(super) has_909: bool,
    pub(super) dev_count: usize,
    pub(super) prefixes: [&'static str; 4],
    pub(super) ch_ids: &'a [u64],
    pub(super) inst_ids: &'a [u64],
    pub(super) echo_id: u64,
    pub(super) echo_vec_idx: usize,
    pub(super) delay_dev_id: u64,
    pub(super) drive_dev_ids: [Option<u64>; 4],
    pub(super) pcf_dev_ids: [Option<u64>; 4],
    pub(super) comp_dev_ids: [Option<u64>; 4],
    pub(super) master_comp_id: Option<u64>,
}

impl RbsSong {
    /// Emit per-zone `InstrumentParam::{Zone, Module}` automation curves for TR-808 (track 3)
    /// or TR-909 (track 4) panel CCs from the declarative drum control tables.
    #[allow(clippy::too_many_arguments)]
    fn emit_drum_zone_automations(
        &self,
        track: usize,
        prefix: &str,
        inst_id: u64,
        init_params: &[u8],
        controls: &[DrumZoneControlSpec],
        sd_pair: DrumSdPairSpec,
        total_steps: u32,
        kit: &mut RackPatch,
        push_mod: &mut PushModFn<'_>,
    ) {
        // 1. Coupled Snare Tone + Snappy pair (shares spliced Filter.Cutoff via `min` on samples).
        if let Some(sd_idx) = find_drum_zone_index(kit, drum_midi::SD) {
            let tone_init = init_params
                .get(usize::from(sd_pair.tone_ctrl))
                .copied()
                .unwrap_or(64);
            let snap_init = init_params
                .get(usize::from(sd_pair.snappy_ctrl))
                .copied()
                .unwrap_or(64);
            let tone_series = self.ctrl_series(track, sd_pair.tone_ctrl, tone_init, total_steps);
            let snap_series = self.ctrl_series(track, sd_pair.snappy_ctrl, snap_init, total_steps);
            let tone_varies = series_varies(&tone_series);
            let snap_varies = series_varies(&snap_series);
            if tone_varies || snap_varies {
                let sd_zone_id = kit.zones[sd_idx].id;
                if !zone_is_sample(&kit.zones[sd_idx]) {
                    if tone_varies {
                        if let Some(flt_id) = find_zone_filter(&kit.zones[sd_idx]) {
                            let mapped: BTreeMap<u32, f32> = tone_series
                                .into_iter()
                                .map(|(s, v)| {
                                    (
                                        s,
                                        ModuleParam::Cutoff
                                            .normalize((sd_pair.cutoff_fn)(v, false)),
                                    )
                                })
                                .collect();
                            push_mod(
                                format!("{prefix} SD Cutoff"),
                                ModulationTarget::Instrument {
                                    instrument: inst_id,
                                    parameter: InstrumentParam::Module {
                                        zone: sd_zone_id,
                                        module: flt_id,
                                        param: ModuleParam::Cutoff,
                                    },
                                },
                                ValueMode::Ramp,
                                compress_points(&mapped, ValueMode::Ramp, total_steps, None),
                                false,
                            );
                        }
                    }
                    if snap_varies {
                        if let Some(noise_id) = find_zone_noise(&kit.zones[sd_idx]) {
                            let mapped: BTreeMap<u32, f32> = snap_series
                                .into_iter()
                                .map(|(s, v)| {
                                    (
                                        s,
                                        ModuleParam::Level.normalize((sd_pair.snappy_noise_fn)(v)),
                                    )
                                })
                                .collect();
                            push_mod(
                                format!("{prefix} SD Snappy"),
                                ModulationTarget::Instrument {
                                    instrument: inst_id,
                                    parameter: InstrumentParam::Module {
                                        zone: sd_zone_id,
                                        module: noise_id,
                                        param: ModuleParam::Level,
                                    },
                                },
                                ValueMode::Ramp,
                                compress_points(&mapped, ValueMode::Ramp, total_steps, None),
                                false,
                            );
                        }
                    }
                } else if let Some(flt_id) = ensure_zone_filter(&mut kit.zones[sd_idx]) {
                    let steps = union_step_keys(&tone_series, &snap_series, true);
                    let mut mapped = BTreeMap::new();
                    for s in steps {
                        if s >= total_steps {
                            break;
                        }
                        let vt = series_val(&tone_series, s, tone_init);
                        let vs = series_val(&snap_series, s, snap_init);
                        let hz = (sd_pair.cutoff_fn)(vt, true).min(rbs_presence_cutoff_hz(vs));
                        mapped.insert(s, ModuleParam::Cutoff.normalize(hz));
                    }
                    push_mod(
                        format!("{prefix} SD Cutoff"),
                        ModulationTarget::Instrument {
                            instrument: inst_id,
                            parameter: InstrumentParam::Module {
                                zone: sd_zone_id,
                                module: flt_id,
                                param: ModuleParam::Cutoff,
                            },
                        },
                        ValueMode::Ramp,
                        compress_points(&mapped, ValueMode::Ramp, total_steps, None),
                        false,
                    );
                }
            }
        }

        // 2. All single-knob per-zone controls in declarative priority order (including dynamic
        //    808 alt zones when present).
        let emit_one_zone = |kit: &mut RackPatch,
                             push_mod: &mut PushModFn<'_>,
                             spec: DrumZoneControlSpec,
                             midi: u8,
                             voice_label: &str| {
            let Some(zone_idx) = find_drum_zone_index(kit, midi) else {
                return;
            };
            let init = init_params
                .get(usize::from(spec.ctrl))
                .copied()
                .unwrap_or(64);
            let series = self.ctrl_series(track, spec.ctrl, init, total_steps);
            if !series_varies(&series) {
                return;
            }
            let is_sample = zone_is_sample(&kit.zones[zone_idx]);
            let Some(param) = spec.resolve_automation_target(&mut kit.zones[zone_idx], is_sample)
            else {
                return;
            };
            let mapped: BTreeMap<u32, f32> = series
                .iter()
                .map(|(&s, &v)| {
                    let native = spec.native_value(is_sample, v);
                    (s, DrumZoneControlSpec::normalize_param(kit, param, native))
                })
                .collect();
            let pts = compress_points(&mapped, ValueMode::Ramp, total_steps, None);
            if push_mod(
                format!("{prefix} {voice_label} {}", spec.param_tag),
                ModulationTarget::Instrument {
                    instrument: inst_id,
                    parameter: param,
                },
                ValueMode::Ramp,
                pts,
                false,
            ) {
                if let InstrumentParam::Module {
                    module,
                    param: ModuleParam::CoarseTune,
                    ..
                } = param
                {
                    // Clear static fine_cents so automated coarse pitch stays centered.
                    if let Some(ModuleKind::Sampler(smp)) = kit.zones[zone_idx]
                        .modules
                        .iter_mut()
                        .find(|m| m.id == module)
                        .map(|m| &mut m.kind)
                    {
                        smp.fine_cents = 0.0;
                    }
                }
            }
        };

        for &spec in controls {
            emit_one_zone(kit, push_mod, spec, spec.midi, spec.label);
            if let (Some(alt_midi), Some(alt_label)) = (spec.alt_midi, spec.alt_label) {
                emit_one_zone(kit, push_mod, spec, alt_midi, alt_label);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn build_song_automations(
        &self,
        fx: &FxTimelines,
        routing: &AutomationRouting<'_>,
        master: &mut cadence_core::mixer::MasterChannel,
        channels: &mut [Channel],
        instruments: &mut [InstrumentInstance],
        patterns: &mut Vec<Pattern>,
        fresh: &mut dyn FnMut() -> u64,
    ) -> Vec<Modulation> {
        let &FxTimelines {
            total_steps,
            ref init_303,
            ref init_808,
            ref init_909,
            ref init_delay,
            ref init_dist,
            ref init_pcf,
            ref init_comp,
            ref master_lvl_series,
            init_comp_target,
            init_pcf_target,
            ref comp_tgt,
            ref comp_en,
            ref comp_params,
            ref pcf_tgt,
            ref pcf_en,
            ref pcf_params,
            ref dist_en,
            ref dist_amt,
            ref dist_shape,
            ref ch_init_en,
            ref ch_en_series,
            ref ch_init_lvl,
            ref ch_lvl_series,
            ref ch_init_pan,
            ref ch_send_series,
            ref ch_init_dist,
            ref ch_dist_series,
            ref dist_activity_steps,
            ref del_pan_series,
            ..
        } = fx;
        let &AutomationRouting {
            has_909,
            dev_count,
            prefixes,
            ch_ids,
            inst_ids,
            echo_id,
            echo_vec_idx,
            delay_dev_id,
            drive_dev_ids,
            pcf_dev_ids,
            comp_dev_ids,
            master_comp_id,
        } = routing;
        let first_active_dist = |c: usize| fx.first_active_dist(c);
        let first_active_pcf = |c: usize| fx.first_active_pcf(c);
        let first_active_comp = |target_id: u8| fx.first_active_comp(target_id);

        if !self.song_mode {
            return Vec::new();
        }

        // Build Song-Mode Automation Modulations in priority order (only emit lanes that move!).
        let mut modulations: Vec<Modulation> = Vec::new();
        let mut mixer_mod_count = 0usize;

        let mut push_mod = |name: String,
                            target: ModulationTarget,
                            mode: ValueMode,
                            points: Vec<ValuePoint>,
                            is_mixer: bool|
         -> bool {
            if points.len() < 2
                || patterns.len() >= MAX_PATTERNS
                || modulations.len() >= MAX_GLOBAL_MODULATIONS
                || modulations.iter().any(|m| m.target == target)
            {
                return false;
            }
            if is_mixer && mixer_mod_count >= MAX_MIXER_AUTOMATIONS {
                return false;
            }
            let pat_id = fresh();
            patterns.push(Pattern {
                id: pat_id,
                name: name.clone(),
                kind: PatternKind::Values {
                    unit: ValueUnit::Normalized,
                    period: total_steps,
                    mode,
                    points,
                },
            });
            modulations.push(Modulation {
                id: fresh(),
                name,
                pattern: pat_id,
                target,
            });
            if is_mixer {
                mixer_mod_count += 1;
            }
            true
        };

        // Helper for active-window effect automation (records parameter moves while that
        // shared ReBirth FX unit is enabled and routed to this channel, PLUS rising-edge
        // snapshots whenever the FX re-activates on this channel after moving elsewhere!).
        let active_effect_points = |param_series: &BTreeMap<u32, u8>,
                                    param_init: u8,
                                    first_active: u32,
                                    mode: ValueMode,
                                    activity_steps: &[u32],
                                    active_at: &dyn Fn(u32) -> bool,
                                    map_fn: &dyn Fn(u8) -> f32|
         -> Vec<ValuePoint> {
            let mut mapped: BTreeMap<u32, f32> = BTreeMap::new();
            let init_v = series_val(param_series, first_active, param_init);
            mapped.insert(0, map_fn(init_v));
            let mut prev_active = active_at(0);
            for &s in activity_steps {
                if s >= total_steps {
                    break;
                }
                let is_active = active_at(s);
                if is_active && !prev_active {
                    let v = series_val(param_series, s, param_init);
                    mapped.insert(s, map_fn(v));
                }
                prev_active = is_active;
            }
            for (&s, &v) in param_series {
                if s < total_steps && active_at(s) {
                    mapped.insert(s, map_fn(v));
                }
            }
            compress_points(&mapped, mode, total_steps, None)
        };

        // 1. CRITICAL FIRST: Master Level (`ctrl 0`) & Mixer Channel Gain + v4.2 Enable/Mute
        //    (`ctrls 5,11,17,23` composed with `6,12,18,24`).
        //    Whenever a strip has `ChannelGain` automation (`GainScale`), set static `strip.gain = 1.0`
        //    so `mix_engine` (`strip.gain * gain_scale`) never squares levels or zero-locks fade-ins!
        if series_varies(master_lvl_series) {
            let master_map: BTreeMap<u32, f32> = master_lvl_series
                .iter()
                .map(|(&s, &v)| (s, rbs_master_gain(v)))
                .collect();
            if push_mod(
                "Master Gain".into(),
                ModulationTarget::ChannelGain {
                    channel: ChannelRef::Master,
                },
                ValueMode::Ramp,
                compress_points(&master_map, ValueMode::Ramp, total_steps, None),
                true,
            ) {
                master.strip.gain = 1.0;
            }
        }

        for c in 0..dev_count {
            // Statically muted for the entire song -> stays cleanly muted (`strip.gain = 0.0`).
            if ch_en_series[c].values().all(|&v| v == 0) {
                continue;
            }
            let en_varies = series_varies(&ch_en_series[c]);
            let lvl_varies = series_varies(&ch_lvl_series[c]);
            if !en_varies && !lvl_varies {
                continue;
            }
            let scale = rbs_channel_gain_scale(c);
            let steps = union_step_keys(&ch_en_series[c], &ch_lvl_series[c], true);
            let mut gain_map: BTreeMap<u32, f32> = BTreeMap::new();
            for s in steps {
                if s >= total_steps {
                    break;
                }
                let en = series_val(&ch_en_series[c], s, ch_init_en[c]) != 0;
                let lvl = series_val(&ch_lvl_series[c], s, ch_init_lvl[c]);
                let eff = if en {
                    (rbs_level(lvl) * scale).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                gain_map.insert(s, eff);
            }
            // Pure mute/unmute switches use exact Hold; combined level + mute curves use Ramp
            // with forced pre-step hold anchors on every enable transition so mutes snap cleanly!
            let (mode, pts) = if !lvl_varies {
                (
                    ValueMode::Hold,
                    compress_points(&gain_map, ValueMode::Hold, total_steps, None),
                )
            } else {
                let forced: HashSet<u32> = if en_varies {
                    let mut toggles = HashSet::new();
                    let mut prev_en = ch_init_en[c] != 0;
                    for (&s, &v) in &ch_en_series[c] {
                        let cur_en = v != 0;
                        if s > 0 && cur_en != prev_en {
                            toggles.insert(s);
                        }
                        prev_en = cur_en;
                    }
                    toggles
                } else {
                    HashSet::new()
                };
                let forced_ref = (!forced.is_empty()).then_some(&forced);
                (
                    ValueMode::Ramp,
                    compress_points(&gain_map, ValueMode::Ramp, total_steps, forced_ref),
                )
            };
            if push_mod(
                format!("{} Gain", prefixes[c]),
                ModulationTarget::ChannelGain {
                    channel: ChannelRef::Channel(ch_ids[c]),
                },
                mode,
                pts,
                true,
            ) {
                channels[c].strip.gain = 1.0;
            }
        }

        // 2. Delay Track 5 automation: Enabled (via Echo return ChannelGain), Steps/Mode Time,
        //    Feedback, and Pan.
        let del_en_raw =
            self.ctrl_series(TRACK_DELAY, DELAY_CTRL_ENABLE, init_delay[0], total_steps);
        let del_en_map: BTreeMap<u32, f32> = del_en_raw
            .into_iter()
            .map(|(s, v)| (s, if v != 0 { RBS_ECHO_RETURN_GAIN } else { 0.0 }))
            .collect();
        if push_mod(
            "Echo On".into(),
            ModulationTarget::ChannelGain {
                channel: ChannelRef::Channel(echo_id),
            },
            ValueMode::Hold,
            compress_points(&del_en_map, ValueMode::Hold, total_steps, None),
            true,
        ) {
            channels[echo_vec_idx].strip.gain = 1.0;
        }

        let del_steps_raw =
            self.ctrl_series(TRACK_DELAY, DELAY_CTRL_STEPS, init_delay[1], total_steps);
        let del_mode_raw =
            self.ctrl_series(TRACK_DELAY, DELAY_CTRL_MODE, init_delay[2], total_steps);
        let del_time_steps = union_step_keys(&del_steps_raw, &del_mode_raw, true);
        let mut del_time_map: BTreeMap<u32, f32> = BTreeMap::new();
        for s in del_time_steps {
            if s >= total_steps {
                break;
            }
            let st = series_val(&del_steps_raw, s, init_delay[1]);
            let md = series_val(&del_mode_raw, s, init_delay[2]);
            del_time_map.insert(s, EffectParam::Time.normalize(rbs_delay_steps(st, md)));
        }
        push_mod(
            "Echo Time".into(),
            ModulationTarget::Effect {
                device: delay_dev_id,
                parameter: EffectParam::Time,
            },
            ValueMode::Hold,
            compress_points(&del_time_map, ValueMode::Hold, total_steps, None),
            true,
        );

        let fb_raw = self.ctrl_series(TRACK_DELAY, DELAY_CTRL_FEEDBACK, init_delay[3], total_steps);
        let fb_map: BTreeMap<u32, f32> = fb_raw
            .into_iter()
            .map(|(s, v)| (s, EffectParam::Feedback.normalize(rbs_delay_feedback(v))))
            .collect();
        push_mod(
            "Echo Feedback".into(),
            ModulationTarget::Effect {
                device: delay_dev_id,
                parameter: EffectParam::Feedback,
            },
            ValueMode::Ramp,
            compress_points(&fb_map, ValueMode::Ramp, total_steps, None),
            true,
        );

        let del_pan_map: BTreeMap<u32, f32> = del_pan_series
            .iter()
            .map(|(&s, &v)| (s, ChannelParam::Pan.normalize(rbs_pan(v))))
            .collect();
        push_mod(
            "Echo Pan".into(),
            ModulationTarget::ChannelPan {
                channel: ChannelRef::Channel(echo_id),
            },
            ValueMode::Ramp,
            compress_points(&del_pan_map, ValueMode::Ramp, total_steps, None),
            true,
        );

        // 3. TB-303 A & B live knob automation driven directly from `TB303_CONTROLS`.
        for idx in 0..2 {
            let Some(targets) = resolve_tb303_targets(&instruments[idx].rack) else {
                continue;
            };
            let trk = TRACK_303_A + idx;
            let tag = prefixes[idx];
            for spec in TB303_CONTROLS {
                let raw = self.ctrl_series(
                    trk,
                    spec.ctrl,
                    init_303[idx][usize::from(spec.ctrl)],
                    total_steps,
                );
                let mapped: BTreeMap<u32, f32> = raw
                    .into_iter()
                    .map(|(s, v)| (s, spec.normalized(v, self.vintage)))
                    .collect();
                let pts = compress_points(&mapped, spec.mode, total_steps, None);
                push_mod(
                    format!("{tag} {}", spec.label),
                    ModulationTarget::Instrument {
                        instrument: inst_ids[idx],
                        parameter: spec.resolve_param(targets),
                    },
                    spec.mode,
                    pts,
                    false,
                );
            }
        }

        // 4. Mixer channel Pan (native ChannelPan) & post-insert Delay Send (`ModulationTarget::Send`).
        for c in 0..dev_count {
            let pan_raw = self.ctrl_series(
                TRACK_MIXER,
                mixer_ch_pan_ctrl(c),
                ch_init_pan[c],
                total_steps,
            );
            let pan_map: BTreeMap<u32, f32> = pan_raw
                .into_iter()
                .map(|(s, v)| (s, ChannelParam::Pan.normalize(rbs_pan(v))))
                .collect();
            if push_mod(
                format!("{} Pan", prefixes[c]),
                ModulationTarget::ChannelPan {
                    channel: ChannelRef::Channel(ch_ids[c]),
                },
                ValueMode::Ramp,
                compress_points(&pan_map, ValueMode::Ramp, total_steps, None),
                true,
            ) {
                channels[c].strip.pan = 0.0;
            }

            let snd_map: BTreeMap<u32, f32> = ch_send_series[c]
                .iter()
                .map(|(&s, &v)| (s, rbs_level(v) * RBS_ECHO_SEND_SCALE))
                .collect();
            push_mod(
                format!("{} Echo", prefixes[c]),
                ModulationTarget::Send {
                    channel: ChannelRef::Channel(ch_ids[c]),
                    target: echo_id,
                },
                ValueMode::Ramp,
                compress_points(&snd_map, ValueMode::Ramp, total_steps, None),
                true,
            );
        }

        // 5. Compressor Track 8 automation (Enabled, Target, Amount->Ratio, Threshold;
        //    zero auto makeup gain per official ReBirth 2.0.1 Owner's Manual p. 66-67).
        let comp_activity_steps = union_step_keys(comp_tgt, comp_en, false);
        let emit_comp_automations =
            |label: &str, cid: u64, target_id: u8, first_s: u32, push_mod: &mut PushModFn<'_>| {
                let mut mix_map: BTreeMap<u32, f32> = BTreeMap::new();
                for &s in &comp_activity_steps {
                    if s >= total_steps {
                        continue;
                    }
                    let t = series_val(comp_tgt, s, init_comp_target);
                    let e = series_val(comp_en, s, init_comp[0]);
                    mix_map.insert(s, if e != 0 && t == target_id { 1.0 } else { 0.0 });
                }
                push_mod(
                    format!("{label} Comp On"),
                    ModulationTarget::Effect {
                        device: cid,
                        parameter: EffectParam::Mix,
                    },
                    ValueMode::Hold,
                    compress_points(&mix_map, ValueMode::Hold, total_steps, None),
                    true,
                );

                let active_here = |s: u32| {
                    series_val(comp_en, s, init_comp[0]) != 0
                        && series_val(comp_tgt, s, init_comp_target) == target_id
                };
                for (i, spec) in COMP_CONTROLS.into_iter().enumerate() {
                    let pts = active_effect_points(
                        &comp_params[i],
                        init_comp[usize::from(spec.ctrl)],
                        first_s,
                        spec.mode,
                        &comp_activity_steps,
                        &active_here,
                        &|v| spec.normalized(v),
                    );
                    push_mod(
                        format!("{label} {}", spec.label),
                        ModulationTarget::Effect {
                            device: cid,
                            parameter: spec.param,
                        },
                        spec.mode,
                        pts,
                        true,
                    );
                }
            };
        if let (Some(cid), Some(first_s)) = (master_comp_id, first_active_comp(1)) {
            emit_comp_automations("Master", cid, 1, first_s, &mut push_mod);
        }
        for c in 0..dev_count {
            if let (Some(cid), Some(first_s)) = (comp_dev_ids[c], first_active_comp((c + 2) as u8))
            {
                emit_comp_automations(prefixes[c], cid, (c + 2) as u8, first_s, &mut push_mod);
            }
        }

        // 6. Distortion Track 6 automation (Channel Enable & Global Enable, Drive Amount & synced Output trim).
        for c in 0..dev_count {
            let (Some(did), Some(first_s)) = (drive_dev_ids[c], first_active_dist(c)) else {
                continue;
            };
            let mut steps_union: BTreeMap<u32, f32> = BTreeMap::new();
            for &s in &dist_activity_steps[c] {
                if s >= total_steps {
                    continue;
                }
                let ch_on = series_val(&ch_dist_series[c], s, ch_init_dist[c]) != 0;
                let gl_on = series_val(dist_en, s, init_dist[0]) != 0;
                steps_union.insert(s, if ch_on && gl_on { 1.0 } else { 0.0 });
            }
            push_mod(
                format!("{} Dist On", prefixes[c]),
                ModulationTarget::Effect {
                    device: did,
                    parameter: EffectParam::Mix,
                },
                ValueMode::Hold,
                compress_points(&steps_union, ValueMode::Hold, total_steps, None),
                true,
            );

            let active_here = |s: u32| {
                series_val(&ch_dist_series[c], s, ch_init_dist[c]) != 0
                    && series_val(dist_en, s, init_dist[0]) != 0
            };
            let mut drv_map: BTreeMap<u32, f32> = BTreeMap::new();
            let mut out_map: BTreeMap<u32, f32> = BTreeMap::new();
            let init_amt = series_val(dist_amt, first_s, init_dist[1]);
            let init_shp = series_val(dist_shape, first_s, init_dist[2]);
            let init_drv = rbs_dist_drive(init_amt, init_shp);
            drv_map.insert(0, init_drv);
            out_map.insert(
                0,
                EffectParam::Output.normalize(rbs_dist_output_db(init_drv)),
            );
            let drv_steps = {
                let mut steps = union_step_keys(dist_amt, dist_shape, false);
                steps.extend(&dist_activity_steps[c]);
                steps.sort_unstable();
                steps.dedup();
                steps
            };
            for s in drv_steps {
                if s >= total_steps {
                    break;
                }
                if active_here(s) {
                    let amt = series_val(dist_amt, s, init_dist[1]);
                    let shp = series_val(dist_shape, s, init_dist[2]);
                    let drv = rbs_dist_drive(amt, shp);
                    drv_map.insert(s, drv);
                    out_map.insert(s, EffectParam::Output.normalize(rbs_dist_output_db(drv)));
                }
            }
            let drv_pts = compress_points(&drv_map, ValueMode::Ramp, total_steps, None);
            push_mod(
                format!("{} Dist", prefixes[c]),
                ModulationTarget::Effect {
                    device: did,
                    parameter: EffectParam::Drive,
                },
                ValueMode::Ramp,
                drv_pts,
                true,
            );
            let out_pts = compress_points(&out_map, ValueMode::Ramp, total_steps, None);
            push_mod(
                format!("{} Dist Out", prefixes[c]),
                ModulationTarget::Effect {
                    device: did,
                    parameter: EffectParam::Output,
                },
                ValueMode::Ramp,
                out_pts,
                true,
            );
        }

        // 7. TR-808 (Track 3) & TR-909 (Track 4) Per-Zone Drum Voice CC Automation.
        struct DrumAutoTarget<'a> {
            dev_idx: usize,
            track: usize,
            init: &'a [u8],
            controls: &'static [DrumZoneControlSpec],
            sd_pair: DrumSdPairSpec,
        }
        let target_808 = DrumAutoTarget {
            dev_idx: 2,
            track: TRACK_808,
            init: init_808.as_slice(),
            controls: TR808_ZONE_CONTROLS,
            sd_pair: TR808_SD_PAIR,
        };
        let target_909 = DrumAutoTarget {
            dev_idx: 3,
            track: TRACK_909,
            init: init_909.as_slice(),
            controls: TR909_ZONE_CONTROLS,
            sd_pair: TR909_SD_PAIR,
        };
        let drum_targets: &[DrumAutoTarget<'_>] = if has_909 {
            &[target_808, target_909]
        } else {
            &[target_808]
        };
        for target in drum_targets {
            self.emit_drum_zone_automations(
                target.track,
                prefixes[target.dev_idx],
                inst_ids[target.dev_idx],
                target.init,
                target.controls,
                target.sd_pair,
                total_steps,
                &mut instruments[target.dev_idx].rack,
                &mut push_mod,
            );
        }

        // 8. PCF Track 7: native rhythmic Filter controls driven directly from `PCF_CONTROLS`.
        let pcf_activity_steps = union_step_keys(pcf_tgt, pcf_en, false);
        for c in 0..dev_count {
            let (Some(pid), Some(first_s)) = (pcf_dev_ids[c], first_active_pcf(c)) else {
                continue;
            };
            let mut mix_map: BTreeMap<u32, f32> = BTreeMap::new();
            for &s in &pcf_activity_steps {
                if s >= total_steps {
                    continue;
                }
                let t = series_val(pcf_tgt, s, init_pcf_target);
                let e = series_val(pcf_en, s, init_pcf[0]);
                mix_map.insert(
                    s,
                    if e != 0 && t == (c + 1) as u8 {
                        1.0
                    } else {
                        0.0
                    },
                );
            }
            push_mod(
                format!("{} PCF Mix", prefixes[c]),
                ModulationTarget::Effect {
                    device: pid,
                    parameter: EffectParam::Mix,
                },
                ValueMode::Hold,
                compress_points(&mix_map, ValueMode::Hold, total_steps, None),
                true,
            );

            let active_here = |s: u32| {
                series_val(pcf_en, s, init_pcf[0]) != 0
                    && series_val(pcf_tgt, s, init_pcf_target) == (c + 1) as u8
            };
            for (i, spec) in PCF_CONTROLS.into_iter().enumerate() {
                let pts = active_effect_points(
                    &pcf_params[i],
                    init_pcf[usize::from(spec.ctrl)],
                    first_s,
                    spec.mode,
                    &pcf_activity_steps,
                    &active_here,
                    &|v| spec.normalized(v),
                );
                push_mod(
                    format!("{} {}", prefixes[c], spec.label),
                    ModulationTarget::Effect {
                        device: pid,
                        parameter: spec.param,
                    },
                    spec.mode,
                    pts,
                    true,
                );
            }
        }

        // 9. Bonus headroom: Distortion Shape -> Drive Tone automation (Track 6 ctrl 2).
        for c in 0..dev_count {
            let (Some(did), Some(first_s)) = (drive_dev_ids[c], first_active_dist(c)) else {
                continue;
            };
            let active_here = |s: u32| {
                series_val(&ch_dist_series[c], s, ch_init_dist[c]) != 0
                    && series_val(dist_en, s, init_dist[0]) != 0
            };
            let tone_pts = active_effect_points(
                dist_shape,
                init_dist[2],
                first_s,
                ValueMode::Ramp,
                &dist_activity_steps[c],
                &active_here,
                &|v| EffectParam::Tone.normalize(rbs_dist_tone_hz(v)),
            );
            push_mod(
                format!("{} Dist Shape", prefixes[c]),
                ModulationTarget::Effect {
                    device: did,
                    parameter: EffectParam::Tone,
                },
                ValueMode::Ramp,
                tone_pts,
                true,
            );
        }

        drop(push_mod);
        modulations
    }
}
