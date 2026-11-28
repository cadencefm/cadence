//! Conversion from parsed [`RbsSong`] into a validated native Cadence [`Project`].

use super::automation::{
    series_val, series_varies, union_step_keys, AutomationRouting, FxTimelines,
};
use super::calibration::{
    bank_slot_label, rack_patch_for_303, rbs_channel_gain_scale, rbs_comp_ratio,
    rbs_comp_threshold_db, rbs_delay_feedback, rbs_delay_steps, rbs_dist_drive, rbs_dist_output_db,
    rbs_dist_tone_hz, rbs_level, rbs_master_gain, rbs_pan, tr808_kit_from_params,
    tr909_kit_from_params, Tr808SelectorMode, RBS_ECHO_RETURN_GAIN, RBS_ECHO_SEND_SCALE,
};
use super::controls::{
    mixer_ch_dist_ctrl, mixer_ch_enable_ctrl, mixer_ch_level_ctrl, mixer_ch_pan_ctrl,
    mixer_ch_send_ctrl, COMP_CONTROLS, COMP_CTRL_ENABLE, DELAY_CTRL_PAN, DEV_CTRL_ENABLE,
    DEV_CTRL_PATTERN, DIST_CTRL_AMOUNT, DIST_CTRL_ENABLE, DIST_CTRL_SHAPE, DRUM_CTRL_ACCENT,
    MIXER_CTRL_COMP_TARGET, MIXER_CTRL_MASTER_LEVEL, MIXER_CTRL_PCF_TARGET, PCF_CONTROLS,
    PCF_CTRL_ENABLE, TR808_SELECTOR_SPECS, TR909_CTRL_FLAM, TRACK_808, TRACK_909, TRACK_COMP,
    TRACK_DELAY, TRACK_DIST, TRACK_MIXER, TRACK_PCF,
};
use super::model::RbsSong;
use super::patterns::{
    drum_pattern_has_accent, drum_pattern_non_empty, drum_pattern_to_steps,
    tb303_pattern_non_empty, tb303_pattern_to_steps, tb303_step_to_cadence, tr909_flam_mini_codes,
    PatternCatalog, Tb303NextStep,
};
use super::sanitize_title;
use anyhow::{Context, Result};
use cadence_core::config::UserConfig;
use cadence_core::material::{
    Exception, InstrumentInstance, Lane, LanePart, MixerGraph, Part, Pattern, PhaseMode, Placement,
    Project, MAX_GLOBAL_MODULATIONS, MAX_LANE_PARTS, MAX_PATTERNS, MAX_PLACEMENTS, STEPS_PER_BAR,
};
use cadence_core::mixer::{
    Channel, ChannelKind, ChannelRef, ChannelSend, ChannelStrip, CompressorPatch, DelayMode,
    DelayPatch, DriveMode, DrivePatch, EffectDevice, EffectKind, FilterPatch, MasterChannel,
};
use cadence_core::music::{EndRule, Step};
use cadence_core::presets::PresetCatalog;
use cadence_core::rack::drum_midi;
use std::collections::{BTreeMap, HashMap, HashSet};

/// Quantizes a varying discrete device parameter (drum Global Accent or 909 Flam Interval)
/// for Song-Mode segment variant keying:
/// - Preserves exact `u8` values whenever `<= 8` distinct values appear across the song.
/// - Snaps denser continuous knob sweeps into `<= 9` clean buckets so `MAX_PATTERNS` never blows up.
struct DiscreteVariantQuantizer {
    exact: bool,
    preserve_zero: bool,
}

impl DiscreteVariantQuantizer {
    fn from_series(series: &BTreeMap<u32, u8>, preserve_zero: bool) -> Self {
        let distinct: HashSet<u8> = series.values().copied().collect();
        Self {
            exact: distinct.len() <= 8,
            preserve_zero,
        }
    }

    fn quantize(&self, raw: u8) -> u8 {
        let v = raw.min(127);
        if self.exact || (self.preserve_zero && v == 0) {
            return v;
        }
        if self.preserve_zero {
            (((v - 1) / 16 + 1) * 16).min(127)
        } else {
            ((v / 16) * 16 + 8).min(127)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct SegmentState {
    p_idx: usize,
    /// TR-808 dynamic alt-voice bitmask (`0` for non-808).
    sel_mask: u8,
    /// Active TR-808 / TR-909 Global Accent (`0` for 303 or unaccented drum patterns).
    accent: u8,
    /// Active TR-909 flam interval (`0` when pattern has no flams or non-909).
    flam_iv: u8,
}

#[derive(Clone, Copy, Debug)]
struct ResolvedSegment {
    start_step: u32,
    end_step: u32,
    state: Option<SegmentState>,
    phase: PhaseMode,
    offset_steps: i32,
}

struct DevicePatternBuild {
    steps: Vec<Step>,
    overflow: Option<Vec<Step>>,
    shuffle: bool,
    flam_minis: Vec<String>,
}

#[inline]
fn pattern_time_at(phase: PhaseMode, start_step: u32, offset_steps: i32, song_step: u32) -> i64 {
    i64::from(match phase {
        PhaseMode::Song => song_step,
        PhaseMode::Restart => song_step.saturating_sub(start_step),
    }) + i64::from(offset_steps)
}

impl RbsSong {
    fn device_pattern_len(&self, dev: usize, p_idx: usize) -> usize {
        match dev {
            0 | 1 => usize::from(self.tb303[dev].patterns[p_idx].length.clamp(1, 16)),
            2 => usize::from(self.tr808.patterns[p_idx].length.clamp(1, 16)),
            _ => usize::from(self.tr909.patterns[p_idx].length.clamp(1, 16)),
        }
    }

    fn build_device_pattern(
        &self,
        dev: usize,
        state: SegmentState,
        song_bpm: f64,
    ) -> DevicePatternBuild {
        match dev {
            0 | 1 => {
                let pat = &self.tb303[dev].patterns[state.p_idx];
                DevicePatternBuild {
                    steps: tb303_pattern_to_steps(pat),
                    overflow: None,
                    shuffle: pat.shuffle,
                    flam_minis: Vec::new(),
                }
            }
            2 => {
                let pat = &self.tr808.patterns[state.p_idx];
                let decoded = drum_pattern_to_steps(pat, false, state.sel_mask, state.accent);
                DevicePatternBuild {
                    steps: decoded.primary,
                    overflow: decoded.overflow,
                    shuffle: pat.shuffle,
                    flam_minis: Vec::new(),
                }
            }
            _ => {
                let pat = &self.tr909.patterns[state.p_idx];
                let decoded = drum_pattern_to_steps(pat, true, 0, state.accent);
                DevicePatternBuild {
                    steps: decoded.primary,
                    overflow: decoded.overflow,
                    shuffle: pat.shuffle,
                    flam_minis: tr909_flam_mini_codes(pat, state.flam_iv, state.accent, song_bpm),
                }
            }
        }
    }

    /// Convert this parsed ReBirth song into a validated native Cadence `Project`
    /// using the default discovered external [`PresetCatalog`].
    pub fn to_cadence_project(&self, fallback_title: &str) -> Result<Project> {
        let catalog = PresetCatalog::discover(&[], None, &UserConfig::load_default());
        Ok(self
            .to_cadence_project_with_presets(fallback_title, &catalog)?
            .0)
    }

    /// Convert this parsed ReBirth song into a validated native Cadence `Project`
    /// resolving `acid-303`, `tr-808`, `tr-909`, and TR-808 alt voices from `presets`.
    /// Missing presets never fail — they fall back gracefully to bare engine defaults
    /// and record non-fatal human-readable notices in the returned `Vec<String>`.
    pub fn to_cadence_project_with_presets(
        &self,
        fallback_title: &str,
        presets: &PresetCatalog,
    ) -> Result<(Project, Vec<String>)> {
        let mut notices: Vec<String> = Vec::new();
        let mut next_id: u64 = 1;
        let mut fresh = || {
            let id = next_id;
            next_id += 1;
            id
        };

        // Determine song length & BPM.
        let bars = self.song_bars();
        let total_steps = (bars * STEPS_PER_BAR) as u32;
        let song_bpm = (f64::from(self.bpm_millis) / 1000.0).clamp(20.0, 300.0);

        // Gather effective t=0 state for all devices & mixer.
        let mut init_303 = [self.tb303[0].params, self.tb303[1].params];
        for (idx, arr) in init_303.iter_mut().enumerate() {
            for (c, slot) in arr.iter_mut().enumerate() {
                *slot = self.initial_ctrl(1 + idx, c as u8, *slot);
            }
        }
        let mut init_808 = self.tr808.params;
        for (c, slot) in init_808.iter_mut().enumerate() {
            *slot = self.initial_ctrl(TRACK_808, c as u8, *slot);
        }
        let mut init_909 = self.tr909.params;
        for (c, slot) in init_909.iter_mut().enumerate() {
            *slot = self.initial_ctrl(TRACK_909, c as u8, *slot);
        }
        let mut init_delay = self.delay;
        for (c, slot) in init_delay.iter_mut().enumerate() {
            *slot = self.initial_ctrl(TRACK_DELAY, c as u8, *slot);
        }
        let mut init_dist = self.dist;
        for (c, slot) in init_dist.iter_mut().enumerate() {
            *slot = self.initial_ctrl(TRACK_DIST, c as u8, *slot);
        }
        let mut init_pcf = self.pcf;
        for (c, slot) in init_pcf.iter_mut().enumerate() {
            *slot = self.initial_ctrl(TRACK_PCF, c as u8, *slot);
        }
        let mut init_comp = self.comp;
        for (c, slot) in init_comp.iter_mut().enumerate() {
            *slot = self.initial_ctrl(TRACK_COMP, c as u8, *slot);
        }

        // Gather TR-808 selector timelines (`TR808_SELECTOR_SPECS`: ctrls 11, 14, 17, 19, 21).
        // ReBirth polarity: 0 (UP) -> Alt (`LC/MC/HC/CL/MA`); != 0 (DN) -> Primary (`LT/MT/HT/RS/CP`).
        let sel_series_808: [BTreeMap<u32, u8>; 5] = std::array::from_fn(|i| {
            let ctrl = TR808_SELECTOR_SPECS[i].ctrl;
            self.ctrl_series(TRACK_808, ctrl, init_808[usize::from(ctrl)], total_steps)
        });
        let sel_modes_808: [Tr808SelectorMode; 5] = std::array::from_fn(|i| {
            let has_primary = sel_series_808[i].values().any(|&v| v != 0);
            let has_alt = sel_series_808[i].values().any(|&v| v == 0);
            match (has_primary, has_alt) {
                (false, true) => Tr808SelectorMode::AltOnly,
                (true, true) => Tr808SelectorMode::Dynamic,
                _ => Tr808SelectorMode::PrimaryOnly,
            }
        });

        // Gather TR-808 & TR-909 Global Accent (`ctrl 2`) and TR-909 flam interval (`ctrl 29`).
        let accent_series_808 =
            self.ctrl_series(TRACK_808, DRUM_CTRL_ACCENT, init_808[2], total_steps);
        let accent_quant_808 = DiscreteVariantQuantizer::from_series(&accent_series_808, true);
        let accent_moves_808 = series_varies(&accent_series_808);

        let accent_series_909 =
            self.ctrl_series(TRACK_909, DRUM_CTRL_ACCENT, init_909[2], total_steps);
        let accent_quant_909 = DiscreteVariantQuantizer::from_series(&accent_series_909, true);
        let accent_moves_909 = series_varies(&accent_series_909);

        let flam_series_909 =
            self.ctrl_series(TRACK_909, TR909_CTRL_FLAM, init_909[29], total_steps);
        let flam_quant_909 = DiscreteVariantQuantizer::from_series(&flam_series_909, false);
        let flam_moves_909 = series_varies(&flam_series_909);

        // Gather Mixer & shared FX routing/control timelines from declarative specs.
        let init_master_lvl = self.initial_ctrl(
            TRACK_MIXER,
            MIXER_CTRL_MASTER_LEVEL,
            self.mixer.master_level,
        );
        let master_lvl_series = self.ctrl_series(
            TRACK_MIXER,
            MIXER_CTRL_MASTER_LEVEL,
            init_master_lvl,
            total_steps,
        );
        let init_comp_target =
            self.initial_ctrl(TRACK_MIXER, MIXER_CTRL_COMP_TARGET, self.mixer.comp_target);
        let init_pcf_target =
            self.initial_ctrl(TRACK_MIXER, MIXER_CTRL_PCF_TARGET, self.mixer.pcf_target);
        let comp_tgt = self.ctrl_series(
            TRACK_MIXER,
            MIXER_CTRL_COMP_TARGET,
            init_comp_target,
            total_steps,
        );
        let comp_en = self.ctrl_series(TRACK_COMP, COMP_CTRL_ENABLE, init_comp[0], total_steps);
        let comp_params: [BTreeMap<u32, u8>; 2] = std::array::from_fn(|i| {
            let ctrl = COMP_CONTROLS[i].ctrl;
            self.ctrl_series(TRACK_COMP, ctrl, init_comp[usize::from(ctrl)], total_steps)
        });

        let pcf_tgt = self.ctrl_series(
            TRACK_MIXER,
            MIXER_CTRL_PCF_TARGET,
            init_pcf_target,
            total_steps,
        );
        let pcf_en = self.ctrl_series(TRACK_PCF, PCF_CTRL_ENABLE, init_pcf[0], total_steps);
        let pcf_params: [BTreeMap<u32, u8>; 6] = std::array::from_fn(|i| {
            let ctrl = PCF_CONTROLS[i].ctrl;
            self.ctrl_series(TRACK_PCF, ctrl, init_pcf[usize::from(ctrl)], total_steps)
        });

        let dist_en = self.ctrl_series(TRACK_DIST, DIST_CTRL_ENABLE, init_dist[0], total_steps);
        let dist_amt = self.ctrl_series(TRACK_DIST, DIST_CTRL_AMOUNT, init_dist[1], total_steps);
        let dist_shape = self.ctrl_series(TRACK_DIST, DIST_CTRL_SHAPE, init_dist[2], total_steps);

        // ReBirth 1.x (`version.major < 4`) had no TR-909 — omit ghost 909 instrument/channel/lane!
        let has_909 = self.version.major >= 4;
        let dev_count = if has_909 { 4 } else { 3 };

        let ch_init_en: [u8; 4] = std::array::from_fn(|c| {
            self.initial_ctrl(
                TRACK_MIXER,
                mixer_ch_enable_ctrl(c),
                u8::from(self.mixer.channels[c].enabled),
            )
        });
        let ch_en_series: [BTreeMap<u32, u8>; 4] = std::array::from_fn(|c| {
            self.ctrl_series(
                TRACK_MIXER,
                mixer_ch_enable_ctrl(c),
                ch_init_en[c],
                total_steps,
            )
        });
        let ch_init_lvl: [u8; 4] = std::array::from_fn(|c| {
            self.initial_ctrl(
                TRACK_MIXER,
                mixer_ch_level_ctrl(c),
                self.mixer.channels[c].level,
            )
        });
        let ch_lvl_series: [BTreeMap<u32, u8>; 4] = std::array::from_fn(|c| {
            self.ctrl_series(
                TRACK_MIXER,
                mixer_ch_level_ctrl(c),
                ch_init_lvl[c],
                total_steps,
            )
        });
        let ch_init_pan: [u8; 4] = std::array::from_fn(|c| {
            self.initial_ctrl(
                TRACK_MIXER,
                mixer_ch_pan_ctrl(c),
                self.mixer.channels[c].pan,
            )
        });
        let ch_init_send_u8: [u8; 4] = std::array::from_fn(|c| {
            self.initial_ctrl(
                TRACK_MIXER,
                mixer_ch_send_ctrl(c),
                self.mixer.channels[c].delay_send,
            )
        });
        let ch_send_series: [BTreeMap<u32, u8>; 4] = std::array::from_fn(|c| {
            self.ctrl_series(
                TRACK_MIXER,
                mixer_ch_send_ctrl(c),
                ch_init_send_u8[c],
                total_steps,
            )
        });
        let ch_init_dist: [u8; 4] = std::array::from_fn(|c| {
            self.initial_ctrl(
                TRACK_MIXER,
                mixer_ch_dist_ctrl(c),
                u8::from(self.mixer.channels[c].dist_on),
            )
        });
        let ch_dist_series: [BTreeMap<u32, u8>; 4] = std::array::from_fn(|c| {
            self.ctrl_series(
                TRACK_MIXER,
                mixer_ch_dist_ctrl(c),
                ch_init_dist[c],
                total_steps,
            )
        });
        let dist_activity_steps: [Vec<u32>; 4] =
            std::array::from_fn(|c| union_step_keys(&ch_dist_series[c], &dist_en, false));
        let del_pan_series =
            self.ctrl_series(TRACK_DELAY, DELAY_CTRL_PAN, init_delay[4], total_steps);

        let fx = FxTimelines {
            total_steps,
            init_303,
            init_808,
            init_909,
            init_delay,
            init_dist,
            init_pcf,
            init_comp,
            init_master_lvl,
            master_lvl_series,
            init_comp_target,
            init_pcf_target,
            comp_tgt,
            comp_en,
            comp_params,
            pcf_tgt,
            pcf_en,
            pcf_params,
            dist_en,
            dist_amt,
            dist_shape,
            ch_init_en,
            ch_en_series,
            ch_init_lvl,
            ch_lvl_series,
            ch_init_pan,
            ch_send_series,
            ch_init_dist,
            ch_dist_series,
            dist_activity_steps,
            del_pan_series,
        };
        let FxTimelines {
            init_303,
            init_808,
            init_909,
            init_delay,
            init_dist,
            init_pcf,
            init_comp,
            init_master_lvl,
            init_comp_target,
            init_pcf_target,
            comp_params,
            pcf_params,
            dist_amt,
            dist_shape,
            ch_init_en,
            ch_init_lvl,
            ch_init_pan,
            ch_send_series,
            ch_init_dist,
            ..
        } = &fx;
        let init_master_lvl = *init_master_lvl;
        let init_comp_target = *init_comp_target;
        let init_pcf_target = *init_pcf_target;

        // Build Mixer: 3 or 4 Audio Channels -> 2 Unity Group Busses (Synths, Drums) + 1 Echo Return -> Master.
        let ch_names = ["TB-303 A", "TB-303 B", "TR-808", "TR-909"];
        let ch_ids: Vec<u64> = (0..dev_count).map(|_| fresh()).collect();
        let synths_group_id = fresh();
        let drums_group_id = fresh();
        let echo_id = fresh();
        let delay_dev_id = fresh();

        let mut synths_ch = Channel::new(synths_group_id, ChannelKind::Group, "Synths");
        synths_ch.strip = ChannelStrip {
            gain: 1.0,
            pan: 0.0,
        };
        synths_ch.output = ChannelRef::Master;

        let mut drums_ch = Channel::new(drums_group_id, ChannelKind::Group, "Drums");
        drums_ch.strip = ChannelStrip {
            gain: 1.0,
            pan: 0.0,
        };
        drums_ch.output = ChannelRef::Master;

        let mut echo_ch = Channel::new(echo_id, ChannelKind::Return, "Echo");
        echo_ch.strip = ChannelStrip {
            gain: if init_delay[0] != 0 {
                RBS_ECHO_RETURN_GAIN
            } else {
                0.0
            },
            pan: rbs_pan(init_delay[4]),
        };
        echo_ch.output = ChannelRef::Master;
        let mut delay_dev = EffectDevice::delay(delay_dev_id);
        delay_dev.kind = EffectKind::Delay(DelayPatch {
            mode: DelayMode::Stereo,
            steps: rbs_delay_steps(init_delay[1], init_delay[2]),
            feedback: rbs_delay_feedback(init_delay[3]),
            mix: 1.0,
        });
        echo_ch.inserts.push(delay_dev);

        let build_comp_device = |cid: u64,
                                 first_s: u32,
                                 active_at_0: bool,
                                 attack_ms: f32,
                                 release_ms: f32|
         -> EffectDevice {
            let mut cmp = EffectDevice::compressor(cid);
            let amt0 = series_val(&comp_params[0], first_s, init_comp[1]);
            let thr0 = series_val(&comp_params[1], first_s, init_comp[2]);
            let thresh_db = rbs_comp_threshold_db(thr0);
            let ratio = rbs_comp_ratio(amt0);
            // Official ReBirth 2.0.1 Owner's Manual (`p. 66-67`): ReBirth's Compressor has zero
            // auto makeup gain (level reduction only, compensated via channel fader).
            cmp.kind = EffectKind::Compressor(CompressorPatch {
                threshold_db: thresh_db,
                ratio,
                attack_ms,
                release_ms,
                makeup_db: 0.0,
                mix: if active_at_0 { 1.0 } else { 0.0 },
            });
            cmp
        };

        let mut drive_dev_ids: [Option<u64>; 4] = [None; 4];
        let mut pcf_dev_ids: [Option<u64>; 4] = [None; 4];
        let mut comp_dev_ids: [Option<u64>; 4] = [None; 4];
        let mut channels = Vec::with_capacity(dev_count + 3);

        for c in 0..dev_count {
            let mut ch = Channel::new(ch_ids[c], ChannelKind::Audio, ch_names[c]);
            ch.output = ChannelRef::Channel(if c < 2 {
                synths_group_id
            } else {
                drums_group_id
            });
            ch.strip = ChannelStrip {
                gain: if ch_init_en[c] != 0 {
                    (rbs_level(ch_init_lvl[c]) * rbs_channel_gain_scale(c)).clamp(0.0, 1.0)
                } else {
                    0.0
                },
                pan: rbs_pan(ch_init_pan[c]),
            };
            let send_amt = rbs_level(ch_init_send_u8[c]) * RBS_ECHO_SEND_SCALE;
            if send_amt > 0.0 || (self.song_mode && series_varies(&ch_send_series[c])) {
                ch.sends.push(ChannelSend {
                    target: echo_id,
                    amount: send_amt,
                });
            }

            if let Some(first_s) = fx.first_active_dist(c) {
                let did = fresh();
                let mut drv = EffectDevice::drive(did);
                drv.name = "Dist".into();
                let amt0 = series_val(dist_amt, first_s, init_dist[1]);
                let shp0 = series_val(dist_shape, first_s, init_dist[2]);
                let active_at_0 = ch_init_dist[c] != 0 && init_dist[0] != 0;
                let drive0 = rbs_dist_drive(amt0, shp0);
                drv.kind = EffectKind::Drive(DrivePatch {
                    mode: DriveMode::ShockPort,
                    drive: drive0,
                    asymmetry: 0.0,
                    tone_hz: rbs_dist_tone_hz(shp0),
                    output_db: rbs_dist_output_db(drive0),
                    mix: if active_at_0 { 1.0 } else { 0.0 },
                });
                ch.inserts.push(drv);
                drive_dev_ids[c] = Some(did);
            }

            if let Some(first_s) = fx.first_active_pcf(c) {
                let pid = fresh();
                let mut flt = EffectDevice::filter(pid);
                flt.name = "PCF".into();
                let active_at_0 = init_pcf[0] != 0 && init_pcf_target == (c + 1) as u8;
                flt.kind = EffectKind::Filter(FilterPatch {
                    drive: 0.12,
                    mix: if active_at_0 { 1.0 } else { 0.0 },
                    env_pattern_id: None,
                    ..FilterPatch::default()
                });
                // Apply all 6 PCF knobs from the authoritative declarative control table.
                for (i, spec) in PCF_CONTROLS.into_iter().enumerate() {
                    let raw = series_val(&pcf_params[i], first_s, init_pcf[usize::from(spec.ctrl)]);
                    let _ = flt.write_native(spec.param, (spec.to_native)(raw));
                }
                ch.inserts.push(flt);
                pcf_dev_ids[c] = Some(pid);
            }

            if let Some(first_s) = fx.first_active_comp((c + 2) as u8) {
                let cid = fresh();
                let active_at_0 = init_comp[0] != 0 && init_comp_target == (c + 2) as u8;
                ch.inserts
                    .push(build_comp_device(cid, first_s, active_at_0, 1.2, 90.0));
                comp_dev_ids[c] = Some(cid);
            }

            channels.push(ch);
        }
        channels.push(synths_ch);
        channels.push(drums_ch);
        let echo_vec_idx = channels.len();
        channels.push(echo_ch);

        // Calibrate Master headroom cleanly for unity-gain Master summing (no hidden tanh clamp)
        // so full mixes stay below 0 dBFS while Master = 0 remains truly silent.
        let mut master = MasterChannel {
            strip: ChannelStrip {
                gain: rbs_master_gain(init_master_lvl),
                pan: 0.0,
            },
            inserts: Vec::new(),
        };
        let mut master_comp_id: Option<u64> = None;
        if let Some(first_s) = fx.first_active_comp(1) {
            let cid = fresh();
            let active_at_0 = init_comp[0] != 0 && init_comp_target == 1;
            master
                .inserts
                .push(build_comp_device(cid, first_s, active_at_0, 8.0, 95.0));
            master_comp_id = Some(cid);
        }

        // Build 3 Instruments (ReBirth 1.x) or 4 Instruments (ReBirth 2.0).
        let inst_ids: Vec<u64> = (0..dev_count).map(|_| fresh()).collect();
        let mut instruments = Vec::with_capacity(dev_count);
        instruments.push(InstrumentInstance {
            id: inst_ids[0],
            name: "TB-303 A".into(),
            rack: rack_patch_for_303(&init_303[0], self.vintage, presets, &mut notices),
            channel: ch_ids[0],
        });
        instruments.push(InstrumentInstance {
            id: inst_ids[1],
            name: "TB-303 B".into(),
            rack: rack_patch_for_303(&init_303[1], self.vintage, presets, &mut notices),
            channel: ch_ids[1],
        });
        instruments.push(InstrumentInstance {
            id: inst_ids[2],
            name: "TR-808".into(),
            rack: tr808_kit_from_params(init_808, sel_modes_808, presets, &mut notices),
            channel: ch_ids[2],
        });
        if has_909 {
            instruments.push(InstrumentInstance {
                id: inst_ids[3],
                name: "TR-909".into(),
                rack: tr909_kit_from_params(init_909, presets, &mut notices),
                channel: ch_ids[3],
            });
        }

        // Build Patterns, Parts, Lanes, and Event-Driven Sub-Bar Coalesced Placements.
        let mut patterns: Vec<Pattern> = Vec::new();
        let mut parts: Vec<Part> = Vec::new();
        let mut lanes: Vec<Lane> = Vec::with_capacity(dev_count);
        let mut total_placements = 0usize;
        let swing_amount = (f32::from(self.shuffle) / 127.0 * 0.36).clamp(0.0, 0.45);
        let mut catalog = PatternCatalog::new(swing_amount, MAX_PATTERNS - MAX_GLOBAL_MODULATIONS);

        let prefixes = ["303A", "303B", "808", "909"];
        let init_enables = [
            init_303[0][0] != 0,
            init_303[1][0] != 0,
            init_808[0] != 0,
            init_909[0] != 0,
        ];
        let init_pats = [
            init_303[0][1].min(31) as usize,
            init_303[1][1].min(31) as usize,
            init_808[1].min(31) as usize,
            init_909[1].min(31) as usize,
        ];

        let mut all_bar_states: [Vec<Option<(usize, u8)>>; 4] = std::array::from_fn(|_| Vec::new());
        let mut placed_p_idxs: [HashSet<usize>; 4] = std::array::from_fn(|_| HashSet::new());

        // Helper: default canonical variant state for a device's bank slot `p_idx` (used for
        // clean un-suffixed Part naming and Pass 2 unplaced launchpad slots).
        let default_state_for_slot = |dev: usize, p_idx: usize| -> SegmentState {
            match dev {
                0 | 1 => SegmentState {
                    p_idx,
                    sel_mask: 0,
                    accent: 0,
                    flam_iv: 0,
                },
                2 => {
                    let pat = &self.tr808.patterns[p_idx];
                    let pat_len = usize::from(pat.length.clamp(1, 16));
                    let mut mask = 0u8;
                    for (i, sel) in TR808_SELECTOR_SPECS.into_iter().enumerate() {
                        if sel_modes_808[i] == Tr808SelectorMode::Dynamic
                            && pat.steps[..pat_len]
                                .iter()
                                .any(|row| row[sel.pattern_col] != 0)
                            && init_808[usize::from(sel.ctrl)] == 0
                        {
                            mask |= 1 << i;
                        }
                    }
                    let accent = if drum_pattern_has_accent(pat, false) {
                        accent_quant_808.quantize(init_808[2])
                    } else {
                        0
                    };
                    SegmentState {
                        p_idx,
                        sel_mask: mask,
                        accent,
                        flam_iv: 0,
                    }
                }
                _ => {
                    let pat = &self.tr909.patterns[p_idx];
                    let pat_len = usize::from(pat.length.clamp(1, 16));
                    let has_flam = pat.steps[..pat_len]
                        .iter()
                        .any(|row| row[1..12].iter().any(|&v| v >= 3));
                    let flam_iv = if has_flam {
                        flam_quant_909.quantize(init_909[29])
                    } else {
                        0
                    };
                    let accent = if drum_pattern_has_accent(pat, true) {
                        accent_quant_909.quantize(init_909[2])
                    } else {
                        0
                    };
                    SegmentState {
                        p_idx,
                        sel_mask: 0,
                        accent,
                        flam_iv,
                    }
                }
            }
        };

        // Shared Pass 1 & Pass 2 materializer for any `(dev, state)` pair.
        let materialize_slot = |catalog: &mut PatternCatalog,
                                patterns: &mut Vec<Pattern>,
                                parts: &mut Vec<Part>,
                                lane: &mut Lane,
                                fresh: &mut dyn FnMut() -> u64,
                                dev: usize,
                                state: SegmentState,
                                pat_name: String|
         -> Option<u64> {
            let built = self.build_device_pattern(dev, state, song_bpm);
            let pid = catalog.materialize_part(
                dev < 2,
                pat_name,
                inst_ids[dev],
                built.steps,
                built.overflow,
                built.shuffle,
                built.flam_minis,
                patterns,
                parts,
                fresh,
            )?;
            if lane.parts.len() < MAX_LANE_PARTS {
                lane.parts.push(LanePart {
                    part: pid,
                    phase: PhaseMode::Restart,
                    offset_steps: 0,
                });
            }
            Some(pid)
        };

        // Pass 1: Materialize Song-Mode (or Pattern-Mode) placed segments first so they own
        // canonical base patterns, pattern budget priority, and leading `lane.parts` slots.
        for dev in 0..dev_count {
            let mut lane = Lane::new(fresh(), ch_names[dev]);
            let mut state_part_map: HashMap<SegmentState, u64> = HashMap::new();
            let mut slot_variant_counts: HashMap<usize, usize> = HashMap::new();

            let dev_track = 1 + dev;
            let en_series = self.ctrl_series(
                dev_track,
                DEV_CTRL_ENABLE,
                u8::from(init_enables[dev]),
                total_steps,
            );
            let pat_series = self.ctrl_series(
                dev_track,
                DEV_CTRL_PATTERN,
                init_pats[dev] as u8,
                total_steps,
            );

            // Event-driven change boundaries: split ONLY where enable, pattern, dynamic 808
            // voice selectors, 808/909 global accent, or 909 flam interval actually emit events.
            let mut boundaries: Vec<u32> = Vec::with_capacity(16);
            boundaries.push(0);
            boundaries.push(total_steps);
            boundaries.extend(en_series.keys().copied().filter(|&s| s < total_steps));
            boundaries.extend(pat_series.keys().copied().filter(|&s| s < total_steps));
            if dev == 2 {
                for i in 0..5 {
                    if sel_modes_808[i] == Tr808SelectorMode::Dynamic {
                        boundaries.extend(
                            sel_series_808[i]
                                .keys()
                                .copied()
                                .filter(|&s| s < total_steps),
                        );
                    }
                }
                if accent_moves_808 {
                    boundaries.extend(
                        accent_series_808
                            .keys()
                            .copied()
                            .filter(|&s| s < total_steps),
                    );
                }
            } else if dev == 3 {
                if flam_moves_909 {
                    boundaries.extend(flam_series_909.keys().copied().filter(|&s| s < total_steps));
                }
                if accent_moves_909 {
                    boundaries.extend(
                        accent_series_909
                            .keys()
                            .copied()
                            .filter(|&s| s < total_steps),
                    );
                }
            }
            boundaries.sort_unstable();
            boundaries.dedup();

            // Evaluate canonical state on each event interval and coalesce adjacent identical runs!
            let mut coalesced: Vec<(u32, u32, Option<SegmentState>)> = Vec::new();
            for win in boundaries.windows(2) {
                let start_step = win[0];
                let end_step = win[1];
                if start_step >= end_step {
                    continue;
                }
                let en = series_val(&en_series, start_step, u8::from(init_enables[dev])) != 0;
                let state = if !en {
                    None
                } else {
                    let p_idx =
                        series_val(&pat_series, start_step, init_pats[dev] as u8).min(31) as usize;
                    let seg = match dev {
                        0 | 1 => SegmentState {
                            p_idx,
                            sel_mask: 0,
                            accent: 0,
                            flam_iv: 0,
                        },
                        2 => {
                            let pat = &self.tr808.patterns[p_idx];
                            let pat_len = usize::from(pat.length.clamp(1, 16));
                            let mut mask = 0u8;
                            for (i, sel) in TR808_SELECTOR_SPECS.into_iter().enumerate() {
                                if sel_modes_808[i] == Tr808SelectorMode::Dynamic
                                    && pat.steps[..pat_len]
                                        .iter()
                                        .any(|row| row[sel.pattern_col] != 0)
                                    && series_val(
                                        &sel_series_808[i],
                                        start_step,
                                        init_808[usize::from(sel.ctrl)],
                                    ) == 0
                                {
                                    mask |= 1 << i;
                                }
                            }
                            let accent = if drum_pattern_has_accent(pat, false) {
                                let raw = series_val(&accent_series_808, start_step, init_808[2]);
                                accent_quant_808.quantize(raw)
                            } else {
                                0
                            };
                            SegmentState {
                                p_idx,
                                sel_mask: mask,
                                accent,
                                flam_iv: 0,
                            }
                        }
                        _ => {
                            let pat = &self.tr909.patterns[p_idx];
                            let pat_len = usize::from(pat.length.clamp(1, 16));
                            let has_flam = pat.steps[..pat_len]
                                .iter()
                                .any(|row| row[1..12].iter().any(|&v| v >= 3));
                            let flam_iv = if has_flam {
                                let raw = series_val(&flam_series_909, start_step, init_909[29]);
                                flam_quant_909.quantize(raw)
                            } else {
                                0
                            };
                            let accent = if drum_pattern_has_accent(pat, true) {
                                let raw = series_val(&accent_series_909, start_step, init_909[2]);
                                accent_quant_909.quantize(raw)
                            } else {
                                0
                            };
                            SegmentState {
                                p_idx,
                                sel_mask: 0,
                                accent,
                                flam_iv,
                            }
                        }
                    };
                    Some(seg)
                };
                if let Some(last) = coalesced.last_mut() {
                    if last.2 == state {
                        last.1 = end_step;
                        continue;
                    }
                }
                coalesced.push((start_step, end_step, state));
            }

            // Resolve exact phase & offset for every segment so mid-bar splits keep bar-locked
            // phase on 16-step patterns (`PhaseMode::Song`) and continuous polyrhythmic phase +
            // Swing parity across odd-length patterns (`PhaseMode::Restart` + `offset_steps`).
            let mut resolved: Vec<ResolvedSegment> = Vec::with_capacity(coalesced.len());
            let mut last_odd_p_idx: Option<usize> = None;
            let mut odd_anchor: u32 = 0;
            for (start_step, end_step, state) in coalesced {
                let (phase, offset_steps) = match state {
                    None => (PhaseMode::Song, 0),
                    Some(st) => {
                        let pat_len = self.device_pattern_len(dev, st.p_idx);
                        if pat_len == STEPS_PER_BAR {
                            last_odd_p_idx = None;
                            (PhaseMode::Song, 0)
                        } else {
                            let full_period = (if pat_len.is_multiple_of(2) {
                                pat_len
                            } else {
                                pat_len * 2
                            }) as u32;
                            if last_odd_p_idx != Some(st.p_idx) {
                                odd_anchor = start_step - (start_step % STEPS_PER_BAR as u32);
                                last_odd_p_idx = Some(st.p_idx);
                            }
                            let elapsed = start_step.saturating_sub(odd_anchor);
                            (PhaseMode::Restart, (elapsed % full_period) as i32)
                        }
                    }
                };
                resolved.push(ResolvedSegment {
                    start_step,
                    end_step,
                    state,
                    phase,
                    offset_steps,
                });
            }

            // Materialize Parts and emit coalesced Placements (with cross-segment TB-303 slide
            // lookahead whenever a segment ends on a slid note transitioning into a different
            // next step than single-pattern wrap assumed).
            for seg_idx in 0..resolved.len() {
                let seg = resolved[seg_idx];
                let Some(state) = seg.state else {
                    continue;
                };
                if total_placements >= MAX_PLACEMENTS {
                    break;
                }

                let part_id = if let Some(&existing) = state_part_map.get(&state) {
                    existing
                } else {
                    let slot_lbl = bank_slot_label(state.p_idx);
                    let seen = slot_variant_counts.entry(state.p_idx).or_insert(0);
                    let pat_name = if *seen == 0 {
                        format!("{} {slot_lbl}", prefixes[dev])
                    } else {
                        format!("{} {slot_lbl} v{}", prefixes[dev], *seen + 1)
                    };
                    let Some(pid) = materialize_slot(
                        &mut catalog,
                        &mut patterns,
                        &mut parts,
                        &mut lane,
                        &mut fresh,
                        dev,
                        state,
                        pat_name,
                    ) else {
                        continue;
                    };
                    *seen += 1;
                    state_part_map.insert(state, pid);
                    placed_p_idxs[dev].insert(state.p_idx);
                    pid
                };

                // Check cross-bar / cross-segment TB-303 slide boundary on the final step of this
                // placement.
                let mut slide_boundary_fix: Option<(usize, Step, bool)> = None;
                if dev < 2 && seg.end_step > seg.start_step {
                    let pat = &self.tb303[dev].patterns[state.p_idx];
                    let pat_len = usize::from(pat.length.clamp(1, 16));
                    let last_song_step = seg.end_step - 1;
                    let last_pat_time = pattern_time_at(
                        seg.phase,
                        seg.start_step,
                        seg.offset_steps,
                        last_song_step,
                    );
                    let pat_step_idx = last_pat_time.rem_euclid(pat_len as i64) as usize;
                    let last_303 = pat.steps[pat_step_idx];
                    if !last_303.rest && last_303.slide {
                        let assumed_next = if pat.steps[(pat_step_idx + 1) % pat_len].rest {
                            Tb303NextStep::Rest
                        } else {
                            Tb303NextStep::Note
                        };
                        let actual_next = if seg.end_step < total_steps {
                            let next_seg = resolved[seg_idx + 1];
                            match next_seg.state {
                                Some(ns) => {
                                    let npat = &self.tb303[dev].patterns[ns.p_idx];
                                    let nlen = usize::from(npat.length.clamp(1, 16));
                                    let ntime = pattern_time_at(
                                        next_seg.phase,
                                        next_seg.start_step,
                                        next_seg.offset_steps,
                                        seg.end_step,
                                    );
                                    if npat.steps[ntime.rem_euclid(nlen as i64) as usize].rest {
                                        Tb303NextStep::Rest
                                    } else {
                                        Tb303NextStep::Note
                                    }
                                }
                                None => Tb303NextStep::Stop,
                            }
                        } else if !self.song_mode || self.loop_on {
                            match resolved.first().and_then(|s0| s0.state.map(|st| (s0, st))) {
                                Some((s0, st0)) => {
                                    let pat0 = &self.tb303[dev].patterns[st0.p_idx];
                                    let len0 = usize::from(pat0.length.clamp(1, 16));
                                    let t0 = pattern_time_at(s0.phase, 0, s0.offset_steps, 0);
                                    if pat0.steps[t0.rem_euclid(len0 as i64) as usize].rest {
                                        Tb303NextStep::Rest
                                    } else {
                                        Tb303NextStep::Note
                                    }
                                }
                                None => Tb303NextStep::Stop,
                            }
                        } else {
                            Tb303NextStep::Stop
                        };
                        if actual_next != assumed_next {
                            let is_swung = pat.shuffle
                                && swing_amount > 0.01
                                && last_pat_time.rem_euclid(2) == 1;
                            let adjusted = tb303_step_to_cadence(last_303, actual_next);
                            slide_boundary_fix = Some((pat_step_idx, adjusted, is_swung));
                        }
                    }
                }

                let seg_len = seg.end_step - seg.start_step;
                match slide_boundary_fix {
                    Some((pat_step_idx, adjusted_step, true)) => {
                        // Swung odd boundary step: `Placement.exceptions` emits offset 0.0, so use
                        // a boundary-adjusted swung pattern variant for the final cycle to keep
                        // BOTH exact swing micro-timing AND exact cross-bar slide gate!
                        let pat = &self.tb303[dev].patterns[state.p_idx];
                        let pat_len = u32::from(pat.length.clamp(1, 16));
                        let mut adj_steps = tb303_pattern_to_steps(pat);
                        adj_steps[pat_step_idx] = adjusted_step;
                        let adj_name = format!(
                            "{} {} SlideEnd",
                            prefixes[dev],
                            bank_slot_label(state.p_idx)
                        );
                        if let Some(adj_part_id) = catalog.materialize_part(
                            true,
                            adj_name,
                            inst_ids[dev],
                            adj_steps,
                            None,
                            pat.shuffle,
                            Vec::new(),
                            &mut patterns,
                            &mut parts,
                            &mut fresh,
                        ) {
                            if seg_len > pat_len && total_placements + 1 < MAX_PLACEMENTS {
                                let split = seg.end_step - pat_len;
                                let mut head = Placement::new(
                                    fresh(),
                                    part_id,
                                    seg.start_step,
                                    split - seg.start_step,
                                );
                                head.phase = seg.phase;
                                head.offset_steps = seg.offset_steps;
                                lane.placements.push(head);
                                total_placements += 1;

                                let mut tail = Placement::new(fresh(), adj_part_id, split, pat_len);
                                tail.phase = seg.phase;
                                tail.offset_steps = match seg.phase {
                                    PhaseMode::Song => seg.offset_steps,
                                    PhaseMode::Restart => {
                                        let full_period = if pat_len.is_multiple_of(2) {
                                            pat_len
                                        } else {
                                            pat_len * 2
                                        };
                                        ((seg.offset_steps as u32 + (split - seg.start_step))
                                            % full_period)
                                            as i32
                                    }
                                };
                                lane.placements.push(tail);
                                total_placements += 1;
                            } else {
                                let mut pl =
                                    Placement::new(fresh(), adj_part_id, seg.start_step, seg_len);
                                pl.phase = seg.phase;
                                pl.offset_steps = seg.offset_steps;
                                lane.placements.push(pl);
                                total_placements += 1;
                            }
                            continue;
                        }
                        // Fallback if pattern budget exhausted: emit canonical placement.
                        let mut pl = Placement::new(fresh(), part_id, seg.start_step, seg_len);
                        pl.phase = seg.phase;
                        pl.offset_steps = seg.offset_steps;
                        lane.placements.push(pl);
                        total_placements += 1;
                    }
                    Some((_, mut adjusted_step, false)) => {
                        // Unswung boundary step: attach a single zero-overhead Placement Exception
                        // (adjusting degree back into untransposed base-pattern space so
                        // `Project::interpret` produces the exact pitch on transposed Parts!).
                        let part_transpose = parts
                            .iter()
                            .find(|p| p.id == part_id)
                            .map_or(0, |p| p.transpose);
                        adjusted_step.degree -= part_transpose;
                        let mut pl = Placement::new(fresh(), part_id, seg.start_step, seg_len);
                        pl.phase = seg.phase;
                        pl.offset_steps = seg.offset_steps;
                        pl.exceptions.push(Exception {
                            step: seg_len - 1,
                            value: adjusted_step,
                        });
                        lane.placements.push(pl);
                        total_placements += 1;
                    }
                    None => {
                        let mut pl = Placement::new(fresh(), part_id, seg.start_step, seg_len);
                        pl.phase = seg.phase;
                        pl.offset_steps = seg.offset_steps;
                        lane.placements.push(pl);
                        total_placements += 1;
                    }
                }
            }

            // Summarize dominant active state per bar for musical form section detection.
            let mut bar_state: Vec<Option<(usize, u8)>> = Vec::with_capacity(bars);
            for bar in 0..bars {
                let b_start = (bar * STEPS_PER_BAR) as u32;
                let b_end = b_start + STEPS_PER_BAR as u32;
                let mut best: Option<(u32, (usize, u8))> = None;
                for seg in &resolved {
                    if seg.end_step <= b_start {
                        continue;
                    }
                    if seg.start_step >= b_end {
                        break;
                    }
                    if let Some(st) = seg.state {
                        let overlap = seg.end_step.min(b_end) - seg.start_step.max(b_start);
                        if best.is_none_or(|(best_ov, _)| overlap > best_ov) {
                            best = Some((overlap, (st.p_idx, st.sel_mask)));
                        }
                    }
                }
                bar_state.push(best.map(|(_, key)| key));
            }
            all_bar_states[dev] = bar_state;
            lanes.push(lane);
        }

        // Pass 2: Populate remaining non-empty authored bank slots (`A1..D8`) into each lane's
        // live launchpad (`lane.parts`, up to `MAX_LANE_PARTS = 32`) via the exact same shared
        // materializer, without ever touching Song-Mode placements or rendered audio.
        for dev in 0..dev_count {
            let bank_len = match dev {
                0 => self.tb303[0].patterns.len(),
                1 => self.tb303[1].patterns.len(),
                2 => self.tr808.patterns.len(),
                _ => self.tr909.patterns.len(),
            }
            .min(32);
            for p_idx in 0..bank_len {
                if lanes[dev].parts.len() >= MAX_LANE_PARTS {
                    break;
                }
                if placed_p_idxs[dev].contains(&p_idx) {
                    continue;
                }
                let state = default_state_for_slot(dev, p_idx);
                let pat_name = format!("{} {}", prefixes[dev], bank_slot_label(p_idx));
                let _ = materialize_slot(
                    &mut catalog,
                    &mut patterns,
                    &mut parts,
                    &mut lanes[dev],
                    &mut fresh,
                    dev,
                    state,
                    pat_name,
                );
            }
        }

        let routing = AutomationRouting {
            has_909,
            dev_count,
            prefixes,
            ch_ids: &ch_ids,
            inst_ids: &inst_ids,
            echo_id,
            echo_vec_idx,
            delay_dev_id,
            drive_dev_ids,
            pcf_dev_ids,
            comp_dev_ids,
            master_comp_id,
        };
        let modulations = self.build_song_automations(
            &fx,
            &routing,
            &mut master,
            &mut channels,
            &mut instruments,
            &mut patterns,
            &mut fresh,
        );

        // Build musical navigation Sections from loop region + real arrangement transitions.
        let pat_non_empty: [Vec<bool>; 4] = [
            self.tb303[0]
                .patterns
                .iter()
                .map(tb303_pattern_non_empty)
                .collect(),
            self.tb303[1]
                .patterns
                .iter()
                .map(tb303_pattern_non_empty)
                .collect(),
            self.tr808
                .patterns
                .iter()
                .map(drum_pattern_non_empty)
                .collect(),
            self.tr909
                .patterns
                .iter()
                .map(drum_pattern_non_empty)
                .collect(),
        ];
        let sections = self.detect_musical_sections(
            bars,
            dev_count,
            &all_bar_states,
            &pat_non_empty,
            &channels,
            &modulations,
            &patterns,
            &mut fresh,
        );

        let project = Project {
            version: cadence_core::material::VERSION,
            title: sanitize_title(&self.title, fallback_title),
            bpm: song_bpm,
            root_midi: drum_midi::BD, // C2 (36)
            scale: (0..12).collect(), // Chromatic scale for exact 303 notes & drum pads
            bars,
            patterns,
            instruments,
            parts,
            lanes,
            sections,
            mixer: MixerGraph { master, channels },
            ducks: Vec::new(),
            modulations,
            // Leave Macros unbound on `.rbs` import so direct knob edits to 303A/B Cutoff/Res,
            // PCF Freq, Echo Feedback, and 808/909 Cutoff take effect immediately instead of
            // being shadowed by static macro evaluation (`apply_static_macros`).
            macros: Vec::new(),
            timing: Default::default(),
            end: if self.loop_on {
                EndRule::Loop
            } else {
                EndRule::Stop
            },
        };

        project
            .validate()
            .context("Converted ReBirth song failed Cadence project validation")?;
        Ok((project, notices))
    }
}
