//! TB-303, TR-808, TR-909, Mixer, and FX parameter calibration curves and preset builders.

use super::controls::{
    DrumSdPairSpec, DrumZoneControlSpec, DrumZoneParamKind, TB303_CONTROLS, TR808_SD_PAIR,
    TR808_SELECTOR_SPECS, TR808_ZONE_CONTROLS, TR909_SD_PAIR, TR909_ZONE_CONTROLS,
};
use cadence_core::dsp::{lin_to_db, norm_to_log};
use cadence_core::filter_patterns::FILTER_ENV_PATTERN_COUNT;
use cadence_core::presets::PresetCatalog;
use cadence_core::rack::{
    compile_zone_graph, drum_midi, EnvMode, EnvModule, FilterMode, FilterModule, InPort,
    ModuleKind, PatchCable, RackModule, RackParam, RackPatch, RackZone, TriggerMode, VcaModule,
    VoiceMode, Waveform, MAX_CABLES_PER_ZONE, MAX_MODULES_PER_ZONE,
};

// ---------------------------------------------------------------------------
// TB-303 Calibration
// ---------------------------------------------------------------------------

/// Normal TB-303 step velocity — sits cleanly below `cadence_core::dsp::ACID_ACCENT_THRESHOLD`
/// (`0.85`) so non-accented notes retain their full authored filter envelope decay.
pub(super) const RBS_303_NORMAL_VEL: f32 = 0.64;
/// Accented TB-303 step velocity — drives `acid_accent_amount(1.0) == 1.0` for crisp Acid Ladder
/// accent decay snap, resonance boost, and filter `vel_amount` octave sweep.
pub(super) const RBS_303_ACCENT_VEL: f32 = 1.0;

/// Subtle analog drive for TB-303 voices.
pub(super) const RBS_303_DRIVE: f32 = 0.30;
/// Official ReBirth 2.0.1 Owner's Manual (`p. 170`): enabling "Vintage ReBirth Synth Sound"
/// (`GLOB[482] != 0`, auto-set on ReBirth 1.x songs) shortens the TB-303 filter envelope decay
/// relative to ReBirth 2.0 mode.
pub(super) const RBS_303_VINTAGE_DECAY_SCALE: f32 = 0.72;

pub(super) fn rbs_303_tune(v: u8) -> f32 {
    // ReBirth TB-338 Tune quantizes 0..=127 via `tune >> 2` into 32 integer-semitone slots
    // with a 6-bucket zero deadzone at center (`52..=75` -> exact `0` st; `120..=127` -> `+12` st).
    const TUNE_TAB: [i8; 32] = [
        -12, -12, -11, -10, -9, -8, -7, -6, -5, -4, -3, -2, -1, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 5, 6,
        7, 8, 9, 10, 11, 12, 12,
    ];
    f32::from(TUNE_TAB[(v.min(127) >> 2) as usize])
}

pub(super) fn rbs_303_cutoff_hz(v: u8) -> f32 {
    let t = f32::from(v.min(127)) / 127.0;
    // Calibrated against real ReBirth 2.0 TB-303 sweeps (v=27 -> ~436 Hz, v=64 -> ~1110 Hz).
    norm_to_log(t, 220.0, 5600.0).clamp(20.0, 20_000.0)
}

pub(super) fn rbs_303_resonance(v: u8) -> f32 {
    (f32::from(v.min(127)) / 127.0 * 0.92).clamp(0.0, 0.95)
}

pub(super) fn rbs_303_env_mod(v: u8) -> f32 {
    (0.75 + (f32::from(v.min(127)) / 127.0) * 4.45).clamp(-6.0, 6.0)
}

pub(super) fn rbs_303_decay_ms(v: u8) -> f32 {
    let t = f32::from(v.min(127)) / 127.0;
    norm_to_log(t, 65.0, 1200.0).clamp(1.0, 20_000.0)
}

pub(super) fn rbs_303_decay_ms_for_mode(v: u8, vintage: bool) -> f32 {
    let base = rbs_303_decay_ms(v);
    if vintage {
        (base * RBS_303_VINTAGE_DECAY_SCALE).clamp(1.0, 20_000.0)
    } else {
        base
    }
}

pub(super) fn rbs_303_accent(v: u8) -> f32 {
    (f32::from(v.min(127)) / 127.0 * 3.2).clamp(0.0, 4.0)
}

pub(super) fn rbs_303_waveform_f32(v: u8) -> f32 {
    let wf = if v == 0 {
        Waveform::Saw
    } else {
        Waveform::Pulse
    };
    wf.to_index() as f32
}

// ---------------------------------------------------------------------------
// Mixer, Delay, PCF, Distortion, and Compressor Calibration
// ---------------------------------------------------------------------------

pub(super) fn rbs_pan(v: u8) -> f32 {
    ((f32::from(v.min(127)) - 64.0) / 63.0).clamp(-1.0, 1.0)
}

pub(super) fn rbs_level(v: u8) -> f32 {
    (f32::from(v.min(127)) / 127.0).clamp(0.0, 1.0)
}

pub(super) fn rbs_master_gain(v: u8) -> f32 {
    (rbs_level(v) * RBS_MASTER_GAIN_SCALE).clamp(0.0, RBS_MASTER_GAIN_MAX)
}

pub(super) fn rbs_delay_feedback(v: u8) -> f32 {
    (f32::from(v.min(127)) / 127.0 * 0.85).clamp(0.0, 0.88)
}

pub(super) fn rbs_pcf_cutoff_hz(v: u8) -> f32 {
    let t = f32::from(v.min(127)) / 127.0;
    // ReBirth PCF base cutoff extends down into sub-bass (~22 Hz at 0, ~98 Hz at 37,
    // ~570 Hz at 78, ~4.4 kHz at 127), keeping BandPass bass grooves centered on the fundamental.
    norm_to_log(t, 22.0, 4400.0).clamp(20.0, 18_000.0)
}

pub(super) fn rbs_pcf_resonance(v: u8) -> f32 {
    (f32::from(v.min(127)) / 127.0 * 0.82).clamp(0.0, 0.95)
}

pub(super) fn rbs_pcf_env_amount(v: u8) -> f32 {
    (f32::from(v.min(127)) / 127.0 * 3.8).clamp(0.0, 4.0)
}

pub(super) fn rbs_pcf_env_pattern(v: u8) -> u8 {
    v.min((FILTER_ENV_PATTERN_COUNT - 1) as u8)
}

pub(super) fn rbs_pcf_env_pattern_f32(v: u8) -> f32 {
    f32::from(rbs_pcf_env_pattern(v))
}

pub(super) fn rbs_pcf_decay_ms(v: u8) -> f32 {
    let t = f32::from(v.min(127)) / 127.0;
    norm_to_log(t, 25.0, 1200.0).clamp(15.0, 1500.0)
}

pub(super) fn rbs_pcf_filter_mode(v: u8) -> FilterMode {
    // ReBirth PCF mode: 0 = BandPass, 1 = LowPass.
    if v == 1 {
        FilterMode::LowPass
    } else {
        FilterMode::BandPass
    }
}

pub(super) fn rbs_pcf_filter_mode_f32(v: u8) -> f32 {
    rbs_pcf_filter_mode(v).to_index() as f32
}

pub(super) fn rbs_dist_drive(amount: u8, shape: u8) -> f32 {
    let amt = f32::from(amount.min(127)) / 127.0;
    let shp = f32::from(shape.min(127)) / 127.0;
    // ReBirth DIST `amount` drives clipping even at `shape == 0` (1.5 mode), while raising `shape`
    // pushes harder into aggressive odd-harmonic grit.
    (amt * (0.72 + 0.28 * shp.powf(0.55))).clamp(0.0, 1.0)
}

pub(super) fn rbs_dist_tone_hz(shape: u8) -> f32 {
    let t = f32::from(shape.min(127)) / 127.0;
    // ReBirth DIST `shape` is a waveshaping grit control (not a dark low-pass filter): keep upper
    // harmonics wide open even at `shape == 0`, brightening further up to 18 kHz.
    norm_to_log(t.powf(0.6), 9_500.0, 18_000.0).clamp(600.0, 18_000.0)
}

/// Output gain trim (dB) compensating `DriveMode::HardClip` overdrive so engaging ReBirth DIST
/// adds dense harmonic bite without jumping +12 dB louder than clean synths and drums.
pub(super) fn rbs_dist_output_db(drive: f32) -> f32 {
    (-11.0 * drive.clamp(0.0, 1.0).powf(0.65)).clamp(-12.0, 0.0)
}

pub(super) fn rbs_comp_threshold_db(v: u8) -> f32 {
    (-36.0 + (f32::from(v.min(127)) / 127.0) * 26.0).clamp(-48.0, 0.0)
}

pub(super) fn rbs_comp_ratio(v: u8) -> f32 {
    (1.5 + (f32::from(v.min(127)) / 127.0) * 10.5).clamp(1.0, 20.0)
}

pub(super) const RBS_SYNTH_CHANNEL_GAIN_SCALE: f32 = 1.0;
pub(super) const RBS_DRUM_CHANNEL_GAIN_SCALE: f32 = 0.94;
/// Unity Echo return strip paired with a headroom-calibrated post-fader send scale so
/// multi-channel sends and high tempo-synced feedback do not overload the Echo bus.
pub(super) const RBS_ECHO_RETURN_GAIN: f32 = 1.0;
pub(super) const RBS_ECHO_SEND_SCALE: f32 = 0.76;
pub(super) const RBS_MASTER_GAIN_SCALE: f32 = 0.88;
pub(super) const RBS_MASTER_GAIN_MAX: f32 = 0.96;
/// Nominal source headroom (dB) on imported TB-303 Racks so dual-303 mixes with automated
/// `0..1` channel faders balance cleanly without altering `AcidLowPass` internal drive.
pub(super) const RBS_303_RACK_GAIN_DB: f32 = -6.0;
/// Nominal source headroom (dB) on imported TR-808 / TR-909 drum Racks, compensating
/// full-scale factory kits plus per-voice `trim_db` and `level_to_zone_db` boosts so all
/// four ReBirth instruments balance evenly beneath automated channel and zone faders.
pub(super) const RBS_DRUM_KIT_GAIN_DB: f32 = -8.0;

pub(super) fn rbs_channel_gain_scale(dev_idx: usize) -> f32 {
    if dev_idx < 2 {
        RBS_SYNTH_CHANNEL_GAIN_SCALE
    } else {
        RBS_DRUM_CHANNEL_GAIN_SCALE
    }
}

/// Calibrated TR-808 / TR-909 step velocities derived from the Global Accent knob (`params[2]`).
///
/// Matches Roland hardware behavior: non-accented steps hold a steady base level (`0.66`), while
/// raising Global Accent (`0..=127`) boosts accented steps smoothly from `0.66` (uniform, 0 dB
/// boost at Accent=0) up to `1.00` (+3.6 dB punch at Accent=127).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct DrumVelocities {
    pub normal: f32,
    pub accent: f32,
    pub flam_grace_normal: f32,
    pub flam_grace_accent: f32,
}

pub(super) fn rbs_drum_velocities(accent_knob: u8) -> DrumVelocities {
    let a = f32::from(accent_knob.min(127)) / 127.0;
    let normal = 0.66_f32;
    let accent = (normal + a * 0.34).clamp(normal, 1.0);
    DrumVelocities {
        normal,
        accent,
        flam_grace_normal: (normal * 0.86).clamp(0.30, 0.95),
        flam_grace_accent: (accent * 0.86).clamp(0.30, 0.98),
    }
}

/// Compute TR-909 flam micro-offset as a fraction of one 16th-note step at `song_bpm`.
///
/// Hardware TR-909 flam interval spans `10..=42 ms` (`ctrl 29 = 0..=127`). Dividing by
/// `step_ms = 15_000 / bpm` preserves real-world millisecond spacing across slow and fast tempos
/// while staying inside safe sub-step bounds (`0.035..=0.48` of a step).
pub(super) fn rbs_909_flam_offset(flam_interval: u8, song_bpm: f64) -> f32 {
    let flam_ms = 10.0 + (f32::from(flam_interval.min(127)) / 127.0) * 32.0;
    let step_ms = 15_000.0 / (song_bpm.clamp(20.0, 300.0) as f32);
    (flam_ms / step_ms).clamp(0.035, 0.48)
}

/// Convert ReBirth Delay `(steps, mode)` into Cadence `DelayPatch.steps` (16th-note steps).
///
/// Per the Propellerhead ReBirth RB-338 manual, `mode == 1` selects straight 16th-note steps
/// (`1..=32`) and `mode == 0` selects **eighth-note triplets** (`1..=32` eighth-note triplets;
/// 3 eighth-note triplets = 1 quarter note = 4 sixteenth steps, so 1 eighth-note triplet = `4/3`
/// sixteenth steps, clamped to Cadence's `0.5..=32.0` step delay range).
pub(super) fn rbs_delay_steps(steps: u8, mode: u8) -> f32 {
    let s = f32::from(steps.clamp(1, 32));
    let raw = if mode == 0 { s * (4.0 / 3.0) } else { s };
    raw.clamp(0.5, 32.0)
}

pub(super) fn bank_slot_label(pattern_idx: usize) -> String {
    let bank = (b'A' + (pattern_idx / 8) as u8) as char;
    let slot = (pattern_idx % 8) + 1;
    format!("{bank}{slot}")
}

fn push_notice_once(notices: &mut Vec<String>, msg: String) {
    if !notices.iter().any(|existing| existing == &msg) {
        notices.push(msg);
    }
}

/// Resolved module and cable IDs for an Acid TB-303 [`RackPatch`] (supports both
/// `presets/synth/acid-303.json`, `RackPatch::acid_303()`, and customized 303 racks).
#[derive(Clone, Copy, Debug)]
pub(super) struct Tb303Targets {
    pub zone_id: u16,
    pub vco_id: u16,
    pub filter_id: u16,
    pub filter_env_id: u16,
    pub env_to_filter_cable_id: u16,
}

pub(super) fn resolve_tb303_targets(rack: &RackPatch) -> Option<Tb303Targets> {
    let zone = rack.zones.first()?;
    let vco_id = zone
        .modules
        .iter()
        .find(|m| matches!(m.kind, ModuleKind::Vco(_)))?
        .id;
    let filter_id = zone
        .modules
        .iter()
        .find(|m| matches!(m.kind, ModuleKind::Filter(_)))?
        .id;
    let (filter_env_id, env_to_filter_cable_id) = zone.cables.iter().find_map(|c| {
        if c.to_module == filter_id && c.to_port == InPort::Cv {
            let is_env = zone
                .modules
                .iter()
                .any(|m| m.id == c.from_module && matches!(m.kind, ModuleKind::Env(_)));
            if is_env {
                return Some((c.from_module, c.id));
            }
        }
        None
    })?;
    Some(Tb303Targets {
        zone_id: zone.id,
        vco_id,
        filter_id,
        filter_env_id,
        env_to_filter_cable_id,
    })
}

pub(super) fn rack_patch_for_303(
    params: &[u8; 9],
    vintage: bool,
    catalog: &PresetCatalog,
    notices: &mut Vec<String>,
) -> RackPatch {
    let mut rack = match catalog
        .find("acid-303")
        .filter(|e| resolve_tb303_targets(&e.patch).is_some())
    {
        Some(entry) => entry.patch.clone(),
        None => {
            push_notice_once(
                notices,
                "Preset 'acid-303' missing; using default Acid 303 Rack".into(),
            );
            RackPatch::acid_303()
        }
    };
    rack.gain_db = RBS_303_RACK_GAIN_DB;
    let Some(targets) = resolve_tb303_targets(&rack) else {
        return rack;
    };
    if let Some(zone) = rack.zones.first_mut() {
        zone.voice.voice_mode = VoiceMode::Mono;
        zone.voice.legato = true;
        zone.voice.glide_ms = 60.0;
        for m in &mut zone.modules {
            if m.id == targets.vco_id {
                if let ModuleKind::Vco(vco) = &mut m.kind {
                    vco.level = 1.0;
                    vco.pulse_width = 0.50;
                }
            } else if m.id == targets.filter_id {
                if let ModuleKind::Filter(flt) = &mut m.kind {
                    flt.mode = FilterMode::AcidLowPass;
                    flt.drive = RBS_303_DRIVE;
                }
            }
        }
    }
    // Apply all 7 panel knobs from the authoritative TB-303 declarative control table
    // (including official Vintage shorter filter decay when `vintage == true`).
    for spec in TB303_CONTROLS {
        let raw = params[usize::from(spec.ctrl)];
        let target_param = spec.resolve_param(targets);
        let _ = rack.write_param_native(target_param, spec.native(raw, vintage));
    }
    rack
}

// ---------------------------------------------------------------------------
// TR-808 & TR-909 Drum Zone Calibration Curves & Builders
// ---------------------------------------------------------------------------

/// Sampled drums already contain their natural acoustic/electronic decay.
/// Only shape shorter tails below `0.72` and scale exponential ADSR `decay_ms` by `2.6x` so the
/// envelope shapes the tail without double-enveloping (squaring) the recorded hit.
pub(super) const RBS_SAMPLE_DECAY_SUSTAIN_THRESHOLD: f32 = 0.72;
pub(super) const RBS_SAMPLE_DECAY_SCALE: f32 = 2.6;
pub(super) const TR808_CY_SYNTH_DECAY_SCALE: f32 = 0.82;
pub(super) const TR808_OH_SYNTH_DECAY_SCALE: f32 = 0.80;

pub(super) fn rbs_sample_decay_ms(synth_decay_ms: f32) -> f32 {
    if synth_decay_ms.is_finite() {
        (synth_decay_ms * RBS_SAMPLE_DECAY_SCALE).clamp(35.0, 10_000.0)
    } else {
        400.0
    }
}

fn level_to_zone_db(v: u8) -> f32 {
    if v == 0 {
        return RackZone::MIN_GAIN_DB;
    }
    let ratio = f32::from(v.min(127)) / 100.0;
    lin_to_db(ratio).clamp(RackZone::MIN_GAIN_DB, 6.0)
}

pub(super) fn rbs_zone_level_db(v: u8, trim_db: f32) -> f32 {
    if v == 0 {
        RackZone::MIN_GAIN_DB
    } else {
        (level_to_zone_db(v) + trim_db).clamp(RackZone::MIN_GAIN_DB, RackZone::MAX_GAIN_DB)
    }
}

pub(super) fn rbs_presence_cutoff_hz(v: u8) -> f32 {
    let t = (f32::from(v.min(127)) / 127.0).clamp(0.0, 1.0);
    if t < 0.85 {
        norm_to_log(t / 0.85, 3600.0, 20_000.0).clamp(30.0, 20_000.0)
    } else {
        20_000.0
    }
}

pub(super) fn rbs_808_bd_cutoff_hz(v: u8, is_sample: bool) -> f32 {
    let t = f32::from(v.min(127)) / 127.0;
    if is_sample {
        norm_to_log(t, 320.0, 20_000.0).clamp(30.0, 20_000.0)
    } else {
        (600.0 + t * 7000.0).clamp(30.0, 20_000.0)
    }
}

pub(super) fn rbs_808_bd_decay_ms(v: u8) -> f32 {
    140.0 + (f32::from(v.min(127)) / 127.0) * 780.0
}

pub(super) fn rbs_808_sd_cutoff_hz(v: u8, is_sample: bool) -> f32 {
    let t = f32::from(v.min(127)) / 127.0;
    if is_sample {
        norm_to_log(t, 2800.0, 20_000.0).clamp(30.0, 20_000.0)
    } else {
        (3200.0 + t * 10_500.0).clamp(30.0, 20_000.0)
    }
}

pub(super) fn rbs_808_sd_snappy_noise(v: u8) -> f32 {
    (0.15 + (f32::from(v.min(127)) / 127.0) * 0.78).clamp(0.0, 1.0)
}

pub(super) fn rbs_808_tom_tune(v: u8) -> f32 {
    ((f32::from(v.min(127)) - 64.0) / 16.0).clamp(-4.0, 4.0)
}

pub(super) fn rbs_808_cy_cutoff_hz(v: u8, is_sample: bool) -> f32 {
    let t = f32::from(v.min(127)) / 127.0;
    if is_sample {
        norm_to_log(t, 3600.0, 20_000.0).clamp(30.0, 20_000.0)
    } else {
        (3200.0 + t * 5800.0).clamp(30.0, 20_000.0)
    }
}

pub(super) fn rbs_808_cy_decay_ms(v: u8) -> f32 {
    240.0 + (f32::from(v.min(127)) / 127.0) * 1050.0
}

pub(super) fn rbs_808_oh_decay_ms(v: u8) -> f32 {
    90.0 + (f32::from(v.min(127)) / 127.0) * 480.0
}

pub(super) fn rbs_909_bd_tune(v: u8) -> f32 {
    ((f32::from(v.min(127)) - 64.0) / 64.0) * 3.0
}

pub(super) fn rbs_909_bd_attack_noise(v: u8) -> f32 {
    (0.05 + (f32::from(v.min(127)) / 127.0) * 0.34).clamp(0.0, 1.0)
}

pub(super) fn rbs_909_bd_decay_ms(v: u8) -> f32 {
    110.0 + (f32::from(v.min(127)) / 127.0) * 320.0
}

pub(super) fn rbs_909_sd_tune(v: u8) -> f32 {
    ((f32::from(v.min(127)) - 64.0) / 64.0) * 5.0
}

pub(super) fn rbs_909_sd_cutoff_hz(v: u8, is_sample: bool) -> f32 {
    let t = f32::from(v.min(127)) / 127.0;
    if is_sample {
        norm_to_log(t, 3200.0, 20_000.0).clamp(30.0, 20_000.0)
    } else {
        (4500.0 + t * 11_000.0).clamp(30.0, 20_000.0)
    }
}

pub(super) fn rbs_909_sd_snappy_noise(v: u8) -> f32 {
    (0.20 + (f32::from(v.min(127)) / 127.0) * 0.75).clamp(0.0, 1.0)
}

pub(super) fn rbs_909_pitch_tune(v: u8) -> f32 {
    ((f32::from(v.min(127)) - 64.0) / 12.0).clamp(-5.33, 5.33)
}

pub(super) fn rbs_909_tom_decay_ms(v: u8) -> f32 {
    75.0 + (f32::from(v.min(127)) / 127.0) * 260.0
}

pub(super) fn rbs_909_ch_decay_ms(v: u8) -> f32 {
    20.0 + (f32::from(v.min(127)) / 127.0) * 95.0
}

pub(super) fn rbs_909_oh_decay_ms(v: u8) -> f32 {
    90.0 + (f32::from(v.min(127)) / 127.0) * 420.0
}

/// Returns true when `zone` is backed by a `Sampler` module.
pub(super) fn zone_is_sample(zone: &RackZone) -> bool {
    zone.modules
        .iter()
        .any(|m| matches!(m.kind, ModuleKind::Sampler(_)))
}

pub(super) fn find_zone_sampler(zone: &RackZone) -> Option<u16> {
    zone.modules
        .iter()
        .find(|m| matches!(m.kind, ModuleKind::Sampler(_)))
        .map(|m| m.id)
}

pub(super) fn find_zone_vco(zone: &RackZone) -> Option<u16> {
    zone.modules
        .iter()
        .find(|m| matches!(m.kind, ModuleKind::Vco(_)))
        .map(|m| m.id)
}

pub(super) fn find_zone_filter(zone: &RackZone) -> Option<u16> {
    zone.modules
        .iter()
        .find(|m| matches!(m.kind, ModuleKind::Filter(_)))
        .map(|m| m.id)
}

pub(super) fn find_zone_noise(zone: &RackZone) -> Option<u16> {
    zone.modules
        .iter()
        .find(|m| matches!(m.kind, ModuleKind::Noise(_)))
        .map(|m| m.id)
}

pub(super) fn find_zone_primary_env(zone: &RackZone) -> Option<u16> {
    if let Ok(compiled) = compile_zone_graph(zone) {
        if let Some(&slot) = compiled.lifetime_envs.first() {
            if let Some(op) = compiled.ops.get(usize::from(slot)) {
                return Some(op.module_id);
            }
        }
    }
    zone.modules
        .iter()
        .find(|m| matches!(m.kind, ModuleKind::Env(_)))
        .map(|m| m.id)
}

fn find_zone_vca_env(zone: &RackZone) -> Option<u16> {
    zone.cables.iter().find_map(|c| {
        if c.to_port != InPort::Cv {
            return None;
        }
        let to_vca = zone
            .modules
            .iter()
            .any(|m| m.id == c.to_module && matches!(m.kind, ModuleKind::Vca(_)));
        let from_env = zone
            .modules
            .iter()
            .any(|m| m.id == c.from_module && matches!(m.kind, ModuleKind::Env(_)));
        (to_vca && from_env).then_some(c.from_module)
    })
}

fn find_zone_noise_env(zone: &RackZone) -> Option<u16> {
    let noise_id = find_zone_noise(zone)?;
    let noise_vca_id = zone.cables.iter().find_map(|c| {
        if c.from_module == noise_id && c.to_port == InPort::In {
            let is_vca = zone
                .modules
                .iter()
                .any(|m| m.id == c.to_module && matches!(m.kind, ModuleKind::Vca(_)));
            if is_vca {
                return Some(c.to_module);
            }
        }
        None
    })?;
    zone.cables.iter().find_map(|c| {
        if c.to_module == noise_vca_id && c.to_port == InPort::Cv {
            let is_env = zone
                .modules
                .iter()
                .any(|m| m.id == c.from_module && matches!(m.kind, ModuleKind::Env(_)));
            if is_env {
                return Some(c.from_module);
            }
        }
        None
    })
}

/// Splice `new_mod` inline immediately before `zone`'s `Out` module, rerouting all audio
/// cables entering `Out.In` into `new_mod.In` and patching `new_mod.Out -> Out.In`.
fn splice_before_out(zone: &mut RackZone, mut new_mod: RackModule) -> Option<u16> {
    if zone.modules.len() >= MAX_MODULES_PER_ZONE || zone.cables.len() >= MAX_CABLES_PER_ZONE {
        return None;
    }
    let out_pos = zone
        .modules
        .iter()
        .position(|m| matches!(m.kind, ModuleKind::Out(_)))?;
    let out_id = zone.modules[out_pos].id;
    let new_id = zone.next_module_id();
    new_mod.id = new_id;
    for cable in &mut zone.cables {
        if cable.to_module == out_id && cable.to_port == InPort::In {
            cable.to_module = new_id;
        }
    }
    let cable_id = zone.next_cable_id();
    zone.cables
        .push(PatchCable::new(cable_id, new_id, out_id, InPort::In, 1.0));
    zone.modules.insert(out_pos, new_mod);
    Some(new_id)
}

/// Return the zone's existing `Filter` module ID or splice a neutral wide-open `LowPass`
/// `Filter` inline before `Out` on sampled drum zones.
pub(super) fn ensure_zone_filter(zone: &mut RackZone) -> Option<u16> {
    if let Some(id) = find_zone_filter(zone) {
        return Some(id);
    }
    splice_before_out(
        zone,
        RackModule::new(
            0,
            "Tone Filter",
            ModuleKind::Filter(FilterModule {
                mode: FilterMode::LowPass,
                cutoff_hz: 20_000.0,
                resonance: 0.0,
                drive: 0.0,
                key_tracking: 0.0,
                vel_amount: 0.0,
                mix: 1.0,
            }),
        ),
    )
}

/// Return the zone's existing `Env -> Vca.Cv` decay envelope ID or splice a crisp OneShot
/// `Env -> Vca` pair inline before `Out` on sampled drum zones.
pub(super) fn ensure_zone_decay_env(zone: &mut RackZone) -> Option<u16> {
    if let Some(id) = find_zone_vca_env(zone) {
        return Some(id);
    }
    if zone.modules.len() + 2 > MAX_MODULES_PER_ZONE || zone.cables.len() + 2 > MAX_CABLES_PER_ZONE
    {
        return None;
    }
    let vca_id = splice_before_out(
        zone,
        RackModule::new(0, "Decay VCA", ModuleKind::Vca(VcaModule::default())),
    )?;
    let env_id = zone.next_module_id();
    let env_mod = RackModule::new(
        env_id,
        "Decay Env",
        ModuleKind::Env(EnvModule {
            mode: EnvMode::Adsr,
            attack_ms: 0.1,
            hold_ms: 0.0,
            decay_ms: 1000.0,
            sustain: 0.0,
            release_ms: 15.0,
            trigger_mode: TriggerMode::OneShot,
            delay_ms: 0.0,
            bursts: 1,
            burst_spacing_ms: 10.0,
        }),
    );
    let out_pos = zone
        .modules
        .iter()
        .position(|m| matches!(m.kind, ModuleKind::Out(_)))
        .unwrap_or(zone.modules.len());
    zone.modules.insert(out_pos, env_mod);
    let cv_cable_id = zone.next_cable_id();
    zone.cables.push(PatchCable::new(
        cv_cable_id,
        env_id,
        vca_id,
        InPort::Cv,
        1.0,
    ));
    Some(env_id)
}

/// Resolve the zone index matching `midi`, preferring an exact single-note drum pad over a
/// wider fallback range.
pub(super) fn find_drum_zone_index(kit: &RackPatch, midi: u8) -> Option<usize> {
    kit.zones
        .iter()
        .position(|z| z.low_midi == midi && z.high_midi == midi)
        .or_else(|| {
            kit.zones
                .iter()
                .position(|z| z.low_midi <= midi && midi <= z.high_midi)
        })
}

fn zone_by_midi_mut(kit: &mut RackPatch, midi: u8) -> Option<&mut RackZone> {
    let idx = find_drum_zone_index(kit, midi)?;
    kit.zones.get_mut(idx)
}

/// Apply pitch tuning cleanly to both sampled (`Sampler`) and synth (`Vco`) drum zones.
fn apply_zone_tune(zone: &mut RackZone, semitones: f32) {
    let st = if semitones.is_finite() {
        semitones.clamp(-24.0, 24.0)
    } else {
        0.0
    };
    if zone_is_sample(zone) {
        let coarse = st.round();
        let fine = ((st - coarse) * 100.0).clamp(-100.0, 100.0);
        for m in &mut zone.modules {
            if let ModuleKind::Sampler(smp) = &mut m.kind {
                smp.coarse_semitones = (coarse as i16).clamp(-48, 48);
                smp.fine_cents = fine;
            }
        }
    } else {
        for m in &mut zone.modules {
            if let ModuleKind::Vco(vco) = &mut m.kind {
                vco.tune_semitones = (vco.tune_semitones + st).clamp(-48.0, 48.0);
            }
        }
    }
}

/// Apply tone/brightness filter cutoff cleanly to both sampled and synth drum zones.
fn apply_zone_cutoff(zone: &mut RackZone, cutoff_hz: f32) {
    let hz = if cutoff_hz.is_finite() {
        cutoff_hz.clamp(30.0, 20_000.0)
    } else {
        20_000.0
    };
    let is_sample = zone_is_sample(zone);
    let flt_id = if is_sample {
        if hz >= 19_999.0 && find_zone_filter(zone).is_none() {
            return;
        }
        ensure_zone_filter(zone)
    } else {
        find_zone_filter(zone)
    };
    if let Some(id) = flt_id {
        if let Some(ModuleKind::Filter(flt)) = zone
            .modules
            .iter_mut()
            .find(|m| m.id == id)
            .map(|m| &mut m.kind)
        {
            flt.cutoff_hz = hz;
        }
    }
}

/// Apply decay time cleanly to both sampled (splicing `OneShot Env -> Vca` below `0.72`)
/// and synth drum zones.
fn apply_zone_decay(
    zone: &mut RackZone,
    decay_ms: f32,
    decay_norm: f32,
    synth_noise_scale: Option<f32>,
) {
    let ms = if decay_ms.is_finite() {
        decay_ms.clamp(15.0, 10_000.0)
    } else {
        250.0
    };
    if zone_is_sample(zone) {
        if decay_norm < RBS_SAMPLE_DECAY_SUSTAIN_THRESHOLD {
            if let Some(env_id) = ensure_zone_decay_env(zone) {
                if let Some(ModuleKind::Env(env)) = zone
                    .modules
                    .iter_mut()
                    .find(|m| m.id == env_id)
                    .map(|m| &mut m.kind)
                {
                    env.decay_ms = rbs_sample_decay_ms(ms);
                    env.sustain = 0.0;
                    env.trigger_mode = TriggerMode::OneShot;
                }
            }
        }
    } else {
        let primary_id = find_zone_primary_env(zone);
        let noise_env_id = synth_noise_scale.and_then(|_| find_zone_noise_env(zone));
        for m in &mut zone.modules {
            let ModuleKind::Env(env) = &mut m.kind else {
                continue;
            };
            if Some(m.id) == noise_env_id && Some(m.id) != primary_id {
                env.decay_ms = ms;
            } else if Some(m.id) == primary_id {
                env.decay_ms = match synth_noise_scale {
                    Some(scale) => (ms * scale).clamp(10.0, 10_000.0),
                    None => ms,
                };
            }
        }
    }
}

/// Apply snare/kick snappy/attack emphasis cleanly to both sampled and synth drum zones.
fn apply_zone_noise_or_presence(zone: &mut RackZone, synth_noise_level: f32, raw_ctrl: u8) {
    if zone_is_sample(zone) {
        let cap_hz = rbs_presence_cutoff_hz(raw_ctrl);
        if cap_hz >= 19_999.0 && find_zone_filter(zone).is_none() {
            return;
        }
        if let Some(id) = ensure_zone_filter(zone) {
            if let Some(ModuleKind::Filter(flt)) = zone
                .modules
                .iter_mut()
                .find(|m| m.id == id)
                .map(|m| &mut m.kind)
            {
                flt.cutoff_hz = flt.cutoff_hz.min(cap_hz);
            }
        }
    } else if let Some(noise_id) = find_zone_noise(zone) {
        if let Some(ModuleKind::Noise(noise)) = zone
            .modules
            .iter_mut()
            .find(|m| m.id == noise_id)
            .map(|m| &mut m.kind)
        {
            noise.level = synth_noise_level.clamp(0.0, 1.0);
        }
    }
}

/// Apply a single declarative drum zone control spec statically onto a kit zone.
fn apply_drum_control_static(kit: &mut RackPatch, params: &[u8], spec: DrumZoneControlSpec) {
    let Some(zone) = zone_by_midi_mut(kit, spec.midi) else {
        return;
    };
    let raw = params.get(usize::from(spec.ctrl)).copied().unwrap_or(64);
    let is_sample = zone_is_sample(zone);
    match spec.kind {
        DrumZoneParamKind::Gain { trim_db } => {
            zone.gain_db = rbs_zone_level_db(raw, trim_db);
        }
        DrumZoneParamKind::Cutoff { curve } => {
            apply_zone_cutoff(zone, curve(raw, is_sample));
        }
        DrumZoneParamKind::Decay { curve, synth_scale } => {
            let norm = f32::from(raw.min(127)) / 127.0;
            apply_zone_decay(zone, curve(raw), norm, synth_scale);
        }
        DrumZoneParamKind::Tune { curve } => {
            apply_zone_tune(zone, curve(raw));
        }
        DrumZoneParamKind::NoiseOrPresence { synth_noise } => {
            apply_zone_noise_or_presence(zone, synth_noise(raw), raw);
        }
    }
}

/// Apply coupled Snare Tone + Snappy knobs statically onto `drum_midi::SD`.
fn apply_drum_sd_pair_static(kit: &mut RackPatch, params: &[u8], sd_pair: DrumSdPairSpec) {
    let Some(sd) = zone_by_midi_mut(kit, drum_midi::SD) else {
        return;
    };
    let tone_raw = params
        .get(usize::from(sd_pair.tone_ctrl))
        .copied()
        .unwrap_or(64);
    let snap_raw = params
        .get(usize::from(sd_pair.snappy_ctrl))
        .copied()
        .unwrap_or(64);
    let tone_t = f32::from(tone_raw.min(127)) / 127.0;
    let is_sample = zone_is_sample(sd);

    if let Some(range) = sd_pair.static_tone_tune_range {
        apply_zone_tune(sd, (tone_t - 0.5) * range);
    }
    apply_zone_cutoff(sd, (sd_pair.cutoff_fn)(tone_raw, is_sample));
    if !is_sample {
        if let Some((base_ms, span_ms)) = sd_pair.static_synth_tone_decay {
            apply_zone_decay(sd, base_ms + tone_t * span_ms, tone_t, None);
        }
    }
    apply_zone_noise_or_presence(sd, (sd_pair.snappy_noise_fn)(snap_raw), snap_raw);
}

fn load_kit_preset_or_blank(
    catalog: &PresetCatalog,
    preset_id: &str,
    fallback_name: &str,
    notices: &mut Vec<String>,
) -> RackPatch {
    match catalog
        .find_by_category("Kit", preset_id)
        .or_else(|| catalog.find(preset_id).filter(|e| e.is_kit()))
    {
        Some(entry) => entry.patch.clone(),
        None => {
            push_notice_once(
                notices,
                format!("Preset '{preset_id}' missing; using default Kit Rack"),
            );
            let mut blank = RackPatch::default_kit();
            blank.kit_name = fallback_name.into();
            blank
        }
    }
}

fn resolve_alt_zone(
    builtin_alt: Option<RackZone>,
    catalog: &PresetCatalog,
    preset_id: &str,
    label: &str,
    target_midi: u8,
    notices: &mut Vec<String>,
) -> RackZone {
    if let Some(mut z) = builtin_alt {
        z.name = label.into();
        z.low_midi = target_midi;
        z.high_midi = target_midi;
        z.set_root_midi(target_midi);
        return z;
    }
    if let Some(entry) = catalog.find(preset_id) {
        if let Some(first) = entry.patch.zones.first() {
            let mut z = first.clone();
            z.name = label.into();
            z.low_midi = target_midi;
            z.high_midi = target_midi;
            z.set_root_midi(target_midi);
            return z;
        }
    }
    push_notice_once(
        notices,
        format!("Preset '{preset_id}' missing; using default Synth zone"),
    );
    let mut z = RackPatch::default_synth().zones.remove(0);
    z.name = label.into();
    z.low_midi = target_midi;
    z.high_midi = target_midi;
    z.set_root_midi(target_midi);
    z
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tr808SelectorMode {
    /// Always primary voice (`LT/MT/HT/RS/CP`) on standard MIDI note.
    PrimaryOnly,
    /// Always alternate voice (`LC/MC/HC/CL/MA`) replacing primary zone in-place.
    AltOnly,
    /// Switches mid-song: keep primary on standard MIDI + ensure alt zone on `LC..=MA`.
    Dynamic,
}

fn ensure_hat_choke_group(kit: &mut RackPatch) {
    for midi in [drum_midi::CH, drum_midi::OH] {
        if let Some(z) = zone_by_midi_mut(kit, midi) {
            if z.choke_group.is_none() {
                z.choke_group = Some(1);
            }
        }
    }
}

fn calibrate_drum_kit_headroom(kit: &mut RackPatch) {
    let (lo, hi) = RackParam::Gain.range();
    kit.gain_db = (kit.gain_db + RBS_DRUM_KIT_GAIN_DB).clamp(lo, hi);
}

pub(super) fn tr808_kit_from_params(
    params: &[u8; 29],
    selector_modes: [Tr808SelectorMode; 5],
    catalog: &PresetCatalog,
    notices: &mut Vec<String>,
) -> RackPatch {
    let mut kit = load_kit_preset_or_blank(catalog, "tr-808", "TR-808", notices);
    // Capture any built-in alt zones (`LC..=MA`) bundled inside `tr-808.json` before pruning
    // unused alt zones so static PrimaryOnly / AltOnly songs stay a clean 11-zone ReBirth layout.
    let mut builtin_alts: [Option<RackZone>; 5] = [None, None, None, None, None];
    for (sel_idx, sel) in TR808_SELECTOR_SPECS.into_iter().enumerate() {
        if let Some(pos) = kit
            .zones
            .iter()
            .position(|z| z.low_midi == sel.alt_midi && z.high_midi == sel.alt_midi)
        {
            builtin_alts[sel_idx] = Some(kit.zones.remove(pos));
        }
    }
    ensure_hat_choke_group(&mut kit);

    // Apply all single-knob TR-808 voice controls & coupled SD Tone/Snappy from declarative tables.
    for &spec in TR808_ZONE_CONTROLS {
        apply_drum_control_static(&mut kit, params, spec);
    }
    apply_drum_sd_pair_static(&mut kit, params, TR808_SD_PAIR);

    // Honor TR-808 Voice Selectors: params[11, 14, 17, 19, 21] (LT->LC, MT->MC, HT->HC, RS->CL, CP->MA).
    for (sel_idx, sel) in TR808_SELECTOR_SPECS.into_iter().enumerate() {
        let maybe_tune = sel
            .tune_ctrl
            .map(|c| rbs_808_tom_tune(params[usize::from(c)]));
        match selector_modes[sel_idx] {
            Tr808SelectorMode::PrimaryOnly => {}
            Tr808SelectorMode::AltOnly => {
                let alt = resolve_alt_zone(
                    builtin_alts[sel_idx].take(),
                    catalog,
                    sel.alt_preset_id,
                    sel.alt_label,
                    sel.primary_midi,
                    notices,
                );
                if let Some(z) = zone_by_midi_mut(&mut kit, sel.primary_midi) {
                    let preserved_id = z.id;
                    let preserved_gain = z.gain_db;
                    *z = alt;
                    z.id = preserved_id;
                    z.gain_db = preserved_gain;
                    if let Some(tune) = maybe_tune {
                        apply_zone_tune(z, tune);
                    }
                }
            }
            Tr808SelectorMode::Dynamic => {
                let gain_db =
                    zone_by_midi_mut(&mut kit, sel.primary_midi).map_or(0.0, |z| z.gain_db);
                let mut alt_zone = resolve_alt_zone(
                    builtin_alts[sel_idx].take(),
                    catalog,
                    sel.alt_preset_id,
                    sel.alt_label,
                    sel.alt_midi,
                    notices,
                );
                alt_zone.id = kit.next_zone_id();
                alt_zone.gain_db = gain_db;
                alt_zone.pan = 0.0;
                alt_zone.choke_group = None;
                if let Some(tune) = maybe_tune {
                    apply_zone_tune(&mut alt_zone, tune);
                }
                kit.zones.push(alt_zone);
            }
        }
    }

    kit.prune_invalid_pins();
    calibrate_drum_kit_headroom(&mut kit);
    kit
}

pub(super) fn tr909_kit_from_params(
    params: &[u8; 30],
    catalog: &PresetCatalog,
    notices: &mut Vec<String>,
) -> RackPatch {
    let mut kit = load_kit_preset_or_blank(catalog, "tr-909", "TR-909", notices);
    ensure_hat_choke_group(&mut kit);

    // Apply all single-knob TR-909 voice controls & coupled SD Tone/Snappy from declarative tables.
    for &spec in TR909_ZONE_CONTROLS {
        apply_drum_control_static(&mut kit, params, spec);
    }
    apply_drum_sd_pair_static(&mut kit, params, TR909_SD_PAIR);

    kit.prune_invalid_pins();
    calibrate_drum_kit_headroom(&mut kit);
    kit
}
