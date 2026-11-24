//! End-to-end integration tests for the Unified Eurorack Modular Rack architecture:
//! - Native single-Rack TR-909 Low Tom (dual fixed VCO `105 Hz + 70 Hz` -> LP12 + post-filter
//!   Noise -> LP6 -> Drive HardClip -> Out) proving distinct low body + post-filter stick click.
//! - Native single-Rack TR-909 Clap using multi-burst `Env` (`bursts: 4, burst_spacing_ms: 11.4`)
//!   + delayed reverb `Env` (`delay_ms: 34.0`) + `Filter(LowPass6)` with zero pattern hacks.
//! - Native `LoFi` bit/rate quantizer, `Vca` 4-quadrant audio-rate Ring Mod, `z-1` feedback cable,
//!   and `Sampler` pitched by `Env` via `PitchCv`.
//! - `ParamAddress::Cable` modulation / `MacroControl` binding + `Project` / `InstrumentPreset` JSON
//!   round-trip.
//! - `--render-kit` round-trip generating a `Rack` kit preset (`[Sampler -> Out]` per voice,
//!   RIFF `inst`/`smpl` root MIDI, choke group 1 on `CH`/`OH`) and loading/rendering it back.

use cadence::host::{render_kit, KitRenderOptions};
use cadence_core::assets::load_project_samples;
use cadence_core::config::UserConfig;
use cadence_core::dsp::env::EnvMode;
use cadence_core::macros::{MacroBinding, MacroControl};
use cadence_core::material::{
    parse_document_json, write_document_json, InstrumentInstance, Lane, LiveFlags, Modulation,
    ModulationTarget, Part, Pattern, PatternKind, PhaseMode, Placement, Project, ValuePoint,
    ValueUnit, VERSION,
};
use cadence_core::mixer::{Channel, ChannelKind};
use cadence_core::music::Step;
use cadence_core::params::{InstrumentParam, ParamAddress, ValueMode};
use cadence_core::presets::{InstrumentPreset, PresetCatalog, INSTRUMENT_PRESET_VERSION};
use cadence_core::rack::{
    compile_zone_graph, drum_midi, module_sample_key, normalize_cable_amount, DriveMode,
    DriveModule, EnvModule, FilterMode, FilterModule, InPort, LoFiModule, ModuleKind,
    ModuleKindTag, NoiseColor, NoiseModule, OutModule, PatchCable, PitchTracking, RackModule,
    RackPatch, RackZone, ResonatorMode, ResonatorModule, SamplerModule, TriggerMode, VcaModule,
    VcoModule, VoiceSettings, Waveform,
};
use cadence_core::sampler::{load_sample_file, SampleData, SampleLibrary};
use cadence_engine::audio::{render_schedule_f32, render_schedule_pcm};
use std::f64::consts::TAU;
use std::path::Path;
use std::sync::Arc;

/// Helper: wrap a single `RackPatch` into a 1-bar Project triggering a single note at step 0,
/// and render stereo `f32` frames at `sample_rate`.
fn render_single_hit(
    rack: RackPatch,
    midi: u8,
    gate_steps: f32,
    steps: u64,
    sample_rate: u32,
    bank: Vec<(u64, Arc<SampleData>)>,
) -> Vec<[f32; 2]> {
    let mut p = Project::empty();
    p.bpm = 120.0;
    p.bars = (steps.div_ceil(16) as usize).max(1);
    p.root_midi = 60;
    p.mixer.master.strip.gain = 1.0;
    p.mixer
        .channels
        .push(Channel::new(1, ChannelKind::Audio, "Voice"));
    let (degree, accidental) = p.midi_to_degree(midi);
    let mut pat_steps = vec![Step::default(); steps as usize];
    pat_steps[0] = Step {
        active: true,
        degree,
        accidental,
        velocity: 0.9,
        gate: gate_steps,
        ..Step::default()
    };
    p.patterns.push(Pattern {
        id: 1,
        name: "Hit".into(),
        kind: PatternKind::Notes { steps: pat_steps },
    });
    p.instruments.push(InstrumentInstance {
        id: 1,
        name: "Rack".into(),
        rack,
        channel: 1,
    });
    p.parts.push(Part::new(1, "Part", 1, 1));
    let mut lane = Lane::new(1, "Voice");
    lane.placements.push(Placement::new(1, 1, 0, steps as u32));
    p.lanes.push(lane);
    let score = p.compile().unwrap();
    let (report, frames) =
        render_schedule_f32(&[(0, &score)], 0, steps, bank, sample_rate).unwrap();
    assert!(
        report.channels.iter().all(|c| !c.clipped),
        "single-hit render clipped: {:?}",
        report.channels
    );
    assert!(
        frames.iter().all(|f| f[0].is_finite() && f[1].is_finite()),
        "non-finite sample in rendered output"
    );
    frames
}

/// Goertzel DFT magnitude at `freq_hz` over a mono slice `samples` at `sample_rate`.
fn goertzel_magnitude(samples: &[f64], sample_rate: f64, freq_hz: f64) -> f64 {
    let n = samples.len();
    if n == 0 {
        return 0.0;
    }
    let w = TAU * freq_hz / sample_rate;
    let coeff = 2.0 * w.cos();
    let mut s1 = 0.0_f64;
    let mut s2 = 0.0_f64;
    for &x in samples {
        let s0 = x + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    let power = (s1 * s1 + s2 * s2 - coeff * s1 * s2).max(0.0);
    2.0 * power.sqrt() / n as f64
}

fn rms_mono(frames: &[[f32; 2]]) -> f64 {
    if frames.is_empty() {
        return 0.0;
    }
    let sum: f64 = frames
        .iter()
        .map(|f| {
            let m = 0.5 * (f64::from(f[0]) + f64::from(f[1]));
            m * m
        })
        .sum();
    (sum / frames.len() as f64).sqrt()
}

fn peak_mono(frames: &[[f32; 2]]) -> f64 {
    frames
        .iter()
        .map(|f| f64::from(f[0]).abs().max(f64::from(f[1]).abs()))
        .fold(0.0_f64, f64::max)
}

fn build_tr909_low_tom_rack(noise_level: f32) -> RackPatch {
    // Dual fixed-pitch VCOs (105 Hz + 70 Hz) with pitch-drop envelope -> Body LP12 -> Body VCA,
    // summed with a post-filter Noise -> Stick LP6 -> Stick VCA into HardClip Drive -> Out.
    let zone = RackZone {
        id: 1,
        name: "TR-909 LT".into(),
        low_midi: 0,
        high_midi: 127,
        root_midi: drum_midi::LT,
        low_vel: 0.0,
        high_vel: 1.0,
        gain_db: 0.0,
        pan: 0.0,
        choke_group: None,
        voice: VoiceSettings {
            pitch_tracking: PitchTracking::Fixed,
            phase_reset: true,
            ..VoiceSettings::default()
        },
        modules: vec![
            RackModule::new(
                1,
                "Body VCO A",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Triangle,
                    pitch_tracking: PitchTracking::Fixed,
                    fixed_hz: 105.0,
                    level: 0.65,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                2,
                "Body VCO B",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Sine,
                    pitch_tracking: PitchTracking::Fixed,
                    fixed_hz: 70.0,
                    level: 0.55,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                3,
                "Pitch Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::HoldDecay,
                    attack_ms: 0.1,
                    hold_ms: 0.0,
                    decay_ms: 35.0,
                    sustain: 0.0,
                    release_ms: 10.0,
                    trigger_mode: TriggerMode::OneShot,
                    delay_ms: 0.0,
                    bursts: 1,
                    burst_spacing_ms: 10.0,
                }),
            ),
            RackModule::new(
                4,
                "Body LP12",
                ModuleKind::Filter(FilterModule {
                    mode: FilterMode::LowPass,
                    cutoff_hz: 320.0,
                    resonance: 0.15,
                    drive: 0.0,
                    key_tracking: 0.0,
                    vel_amount: 0.0,
                    mix: 1.0,
                }),
            ),
            RackModule::new(5, "Body VCA", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(
                6,
                "Body Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::HoldDecay,
                    attack_ms: 0.5,
                    hold_ms: 0.0,
                    decay_ms: 240.0,
                    sustain: 0.0,
                    release_ms: 20.0,
                    trigger_mode: TriggerMode::OneShot,
                    delay_ms: 0.0,
                    bursts: 1,
                    burst_spacing_ms: 10.0,
                }),
            ),
            RackModule::new(
                7,
                "Stick Noise",
                ModuleKind::Noise(NoiseModule {
                    color: NoiseColor::White,
                    highpass_hz: 1_800.0,
                    lowpass_hz: 14_000.0,
                    level: noise_level,
                }),
            ),
            RackModule::new(
                8,
                "Stick LP6",
                ModuleKind::Filter(FilterModule {
                    mode: FilterMode::LowPass6,
                    cutoff_hz: 6_500.0,
                    resonance: 0.0,
                    drive: 0.0,
                    key_tracking: 0.0,
                    vel_amount: 0.0,
                    mix: 1.0,
                }),
            ),
            RackModule::new(9, "Stick VCA", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(
                10,
                "Stick Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::HoldDecay,
                    attack_ms: 0.1,
                    hold_ms: 0.0,
                    decay_ms: 11.0,
                    sustain: 0.0,
                    release_ms: 5.0,
                    trigger_mode: TriggerMode::OneShot,
                    delay_ms: 0.0,
                    bursts: 1,
                    burst_spacing_ms: 10.0,
                }),
            ),
            RackModule::new(
                11,
                "HardClip",
                ModuleKind::Drive(DriveModule {
                    mode: DriveMode::HardClip,
                    drive: 0.35,
                    asymmetry: 0.0,
                    tone_hz: 16_000.0,
                    output_db: -3.0,
                    mix: 1.0,
                }),
            ),
            RackModule::new(12, "Out", ModuleKind::Out(OutModule::default())),
        ],
        cables: vec![
            PatchCable::new(1, 3, 1, InPort::PitchCv, 7.0),
            PatchCable::new(2, 3, 2, InPort::PitchCv, 5.0),
            PatchCable::new(3, 1, 4, InPort::In, 1.0),
            PatchCable::new(4, 2, 4, InPort::In, 1.0),
            PatchCable::new(5, 4, 5, InPort::In, 1.0),
            PatchCable::new(6, 6, 5, InPort::Cv, 1.0),
            PatchCable::new(7, 5, 11, InPort::In, 1.0),
            // Post-filter Noise branch bypasses Body LP12 completely!
            PatchCable::new(8, 7, 8, InPort::In, 1.0),
            PatchCable::new(9, 8, 9, InPort::In, 1.0),
            PatchCable::new(10, 10, 9, InPort::Cv, 1.0),
            PatchCable::new(11, 9, 11, InPort::In, 1.0),
            PatchCable::new(12, 11, 12, InPort::In, 1.0),
        ],
    };
    RackPatch {
        kit_name: String::new(),
        zones: vec![zone],
        gain_db: 0.0,
        pan: 0.0,
        tune_semitones: 0.0,
        cutoff_hz: 20_000.0,
        resonance: 0.0,
        panel_pins: Vec::new(),
    }
}

#[test]
fn native_single_rack_tr909_low_tom_has_distinct_low_body_and_post_filter_stick_transient() {
    let sr = 48_000u32;
    let full = render_single_hit(
        build_tr909_low_tom_rack(0.65),
        drum_midi::LT,
        2.0,
        8,
        sr,
        Vec::new(),
    );
    let body_only = render_single_hit(
        build_tr909_low_tom_rack(0.0),
        drum_midi::LT,
        2.0,
        8,
        sr,
        Vec::new(),
    );

    // 1. Body region (40..140 ms): strong low-frequency body around 70 Hz and 105 Hz!
    let body_start = (0.040 * f64::from(sr)) as usize;
    let body_end = (0.140 * f64::from(sr)) as usize;
    let body_mono: Vec<f64> = full[body_start..body_end]
        .iter()
        .map(|f| f64::from(f[0]))
        .collect();
    let mag_70 = goertzel_magnitude(&body_mono, f64::from(sr), 70.0);
    let mag_105 = goertzel_magnitude(&body_mono, f64::from(sr), 105.0);
    let mag_5k_late = goertzel_magnitude(&body_mono, f64::from(sr), 5_000.0);
    assert!(
        mag_70 > 0.02 && mag_105 > 0.02,
        "TR-909 LT body must contain both 70 Hz ({mag_70:.4}) and 105 Hz ({mag_105:.4}) modes"
    );
    assert!(
        (mag_70 + mag_105) > mag_5k_late * 10.0,
        "late tom tail must stay clean low-frequency body (low={:.4}, 5k={:.4})",
        mag_70 + mag_105,
        mag_5k_late
    );

    // 2. Attack transient (0..12 ms): post-filter noise adds substantial high-frequency energy
    //    bypassing the 320 Hz Body LP12 filter, while late tail (80..180 ms) stays virtually unchanged!
    let attack_len = (0.012 * f64::from(sr)) as usize;
    let attack_delta_hf_sq: f64 = (1..attack_len)
        .map(|i| {
            // First-difference high-pass emphasizes >2 kHz stick click energy.
            let d0 = f64::from(full[i][0] - body_only[i][0]);
            let d_prev = f64::from(full[i - 1][0] - body_only[i - 1][0]);
            let hp = d0 - d_prev;
            hp * hp
        })
        .sum();
    let attack_hf_rms = (attack_delta_hf_sq / attack_len as f64).sqrt();

    let late_start = (0.080 * f64::from(sr)) as usize;
    let late_end = (0.180 * f64::from(sr)) as usize;
    let late_delta_sq: f64 = (late_start..late_end)
        .map(|i| {
            let d = f64::from(full[i][0] - body_only[i][0]);
            d * d
        })
        .sum();
    let late_delta_rms = (late_delta_sq / (late_end - late_start) as f64).sqrt();

    assert!(
        attack_hf_rms > 0.02,
        "post-filter stick noise must produce strong high-frequency attack transient (rms={attack_hf_rms:.5})"
    );
    assert!(
        attack_hf_rms > late_delta_rms * 8.0,
        "stick transient must be sharply localized to attack (attack_hf={attack_hf_rms:.5}, late_delta={late_delta_rms:.5})"
    );
}

#[test]
fn native_single_rack_tr909_clap_renders_four_distinct_bursts_and_delayed_tail() {
    let sr = 48_000u32;
    let clap_zone = RackZone {
        id: 1,
        name: "TR-909 CP".into(),
        low_midi: 0,
        high_midi: 127,
        root_midi: drum_midi::CP,
        low_vel: 0.0,
        high_vel: 1.0,
        gain_db: 0.0,
        pan: 0.0,
        choke_group: None,
        voice: VoiceSettings::default(),
        modules: vec![
            RackModule::new(
                1,
                "Clap Noise",
                ModuleKind::Noise(NoiseModule {
                    color: NoiseColor::White,
                    highpass_hz: 900.0,
                    lowpass_hz: 3_200.0,
                    level: 0.9,
                }),
            ),
            RackModule::new(
                2,
                "Clap LP6",
                ModuleKind::Filter(FilterModule {
                    mode: FilterMode::LowPass6,
                    cutoff_hz: 2_400.0,
                    resonance: 0.0,
                    drive: 0.0,
                    key_tracking: 0.0,
                    vel_amount: 0.0,
                    mix: 1.0,
                }),
            ),
            RackModule::new(
                3,
                "Burst Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::HoldDecay,
                    attack_ms: 0.1,
                    hold_ms: 0.0,
                    decay_ms: 7.5,
                    sustain: 0.0,
                    release_ms: 5.0,
                    trigger_mode: TriggerMode::OneShot,
                    delay_ms: 0.0,
                    bursts: 4,
                    burst_spacing_ms: 11.4,
                }),
            ),
            RackModule::new(4, "Burst VCA", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(
                5,
                "Reverb Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::HoldDecay,
                    attack_ms: 0.5,
                    hold_ms: 0.0,
                    decay_ms: 260.0,
                    sustain: 0.0,
                    release_ms: 20.0,
                    trigger_mode: TriggerMode::OneShot,
                    delay_ms: 34.0,
                    bursts: 1,
                    burst_spacing_ms: 10.0,
                }),
            ),
            RackModule::new(6, "Reverb VCA", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(7, "Out", ModuleKind::Out(OutModule::default())),
        ],
        cables: vec![
            PatchCable::new(1, 1, 2, InPort::In, 1.0),
            PatchCable::new(2, 2, 4, InPort::In, 0.95),
            PatchCable::new(3, 3, 4, InPort::Cv, 1.0),
            PatchCable::new(4, 4, 7, InPort::In, 1.0),
            PatchCable::new(5, 2, 6, InPort::In, 0.45),
            PatchCable::new(6, 5, 6, InPort::Cv, 1.0),
            PatchCable::new(7, 6, 7, InPort::In, 1.0),
        ],
    };
    let clap_rack = RackPatch {
        kit_name: String::new(),
        zones: vec![clap_zone],
        gain_db: 0.0,
        pan: 0.0,
        tune_semitones: 0.0,
        cutoff_hz: 20_000.0,
        resonance: 0.0,
        panel_pins: Vec::new(),
    };
    clap_rack.validate().unwrap();

    // Render 8 steps (1.0 s at 120 BPM) from a SINGLE step-0 trigger (zero pattern hacks!).
    let frames = render_single_hit(clap_rack, drum_midi::CP, 1.0, 8, sr, Vec::new());

    // Measure short-window RMS around each of the 4 burst onsets (0.0, 11.4, 22.8, 34.2 ms)
    // and the valleys immediately preceding bursts 2, 3, and 4 (9.0, 20.4, 31.8 ms).
    let win_rms = |start_ms: f64, dur_ms: f64| -> f64 {
        let a = (start_ms * 0.001 * f64::from(sr)).round() as usize;
        let b = ((start_ms + dur_ms) * 0.001 * f64::from(sr)).round() as usize;
        rms_mono(&frames[a..b.min(frames.len())])
    };

    let burst_peaks: Vec<f64> = [0.0, 11.4, 22.8, 34.2]
        .iter()
        .map(|&t| win_rms(t + 0.2, 3.0))
        .collect();
    let valleys: Vec<f64> = [8.4, 19.8, 31.2].iter().map(|&t| win_rms(t, 2.2)).collect();

    for (i, &bp) in burst_peaks.iter().enumerate() {
        assert!(
            bp > 0.012,
            "clap burst {} must have crisp audible onset energy (rms={bp:.4})",
            i + 1
        );
    }
    for (i, &val) in valleys.iter().enumerate() {
        let prev = burst_peaks[i];
        let next = burst_peaks[i + 1];
        assert!(
            val < prev * 0.45 && val < next * 0.45,
            "clap must show clear trough between burst {} and {} (prev={prev:.4}, valley={val:.4}, next={next:.4})",
            i + 1,
            i + 2
        );
    }

    // Delayed reverb tail (60..180 ms) sustains smoothly and decays cleanly by 420..520 ms.
    let tail_mid = win_rms(60.0, 120.0);
    let tail_late = win_rms(420.0, 100.0);
    assert!(
        tail_mid > 0.003,
        "delayed clap reverb tail must be audible past 60 ms (rms={tail_mid:.5})"
    );
    assert!(
        tail_mid > tail_late * 6.0,
        "clap reverb tail must decay cleanly (mid={tail_mid:.5}, late={tail_late:.5})"
    );
}

#[test]
fn lofi_ring_mod_z1_feedback_and_env_pitched_sampler_work_end_to_end() {
    let sr = 48_000u32;

    // 1. LoFi bit/rate quantizer: crushed 4-bit / 3 kHz vs clean bypass.
    let make_lofi_rack = |bits: f32, rate_hz: f32, mix: f32| RackPatch {
        kit_name: String::new(),
        zones: vec![RackZone {
            id: 1,
            name: "LoFi".into(),
            low_midi: 0,
            high_midi: 127,
            root_midi: 60,
            low_vel: 0.0,
            high_vel: 1.0,
            gain_db: 0.0,
            pan: 0.0,
            choke_group: None,
            voice: VoiceSettings::default(),
            modules: vec![
                RackModule::new(
                    1,
                    "VCO",
                    ModuleKind::Vco(VcoModule {
                        waveform: Waveform::Sine,
                        pitch_tracking: PitchTracking::Fixed,
                        fixed_hz: 440.0,
                        level: 0.7,
                        ..VcoModule::default()
                    }),
                ),
                RackModule::new(
                    2,
                    "LoFi",
                    ModuleKind::LoFi(LoFiModule { bits, rate_hz, mix }),
                ),
                RackModule::new(3, "VCA", ModuleKind::Vca(VcaModule::default())),
                RackModule::new(
                    4,
                    "Env",
                    ModuleKind::Env(EnvModule {
                        attack_ms: 1.0,
                        sustain: 1.0,
                        ..EnvModule::default()
                    }),
                ),
                RackModule::new(5, "Out", ModuleKind::Out(OutModule::default())),
            ],
            cables: vec![
                PatchCable::new(1, 1, 2, InPort::In, 1.0),
                PatchCable::new(2, 2, 3, InPort::In, 1.0),
                PatchCable::new(3, 4, 3, InPort::Cv, 1.0),
                PatchCable::new(4, 3, 5, InPort::In, 1.0),
            ],
        }],
        gain_db: 0.0,
        pan: 0.0,
        tune_semitones: 0.0,
        cutoff_hz: 20_000.0,
        resonance: 0.0,
        panel_pins: Vec::new(),
    };
    let clean = render_single_hit(
        make_lofi_rack(24.0, 48_000.0, 0.0),
        60,
        4.0,
        4,
        sr,
        Vec::new(),
    );
    let crushed = render_single_hit(
        make_lofi_rack(4.0, 3_000.0, 1.0),
        60,
        4.0,
        4,
        sr,
        Vec::new(),
    );
    // At 3 kHz S&H on a 440 Hz sine, zero-order hold creates strong mirror images at
    // 3000 - 440 = 2560 Hz and 3000 + 440 = 3440 Hz (absent from clean bypassed sine)!
    let clean_mono: Vec<f64> = clean[2_400..16_800]
        .iter()
        .map(|f| f64::from(f[0]))
        .collect();
    let crushed_mono: Vec<f64> = crushed[2_400..16_800]
        .iter()
        .map(|f| f64::from(f[0]))
        .collect();
    let clean_alias = goertzel_magnitude(&clean_mono, f64::from(sr), 2_560.0)
        + goertzel_magnitude(&clean_mono, f64::from(sr), 3_440.0);
    let crushed_alias = goertzel_magnitude(&crushed_mono, f64::from(sr), 2_560.0)
        + goertzel_magnitude(&crushed_mono, f64::from(sr), 3_440.0);
    let crushed_harm3 = goertzel_magnitude(&crushed_mono, f64::from(sr), 1_320.0);
    let clean_harm3 = goertzel_magnitude(&clean_mono, f64::from(sr), 1_320.0);
    assert!(
        crushed_alias > 0.04 && crushed_alias > clean_alias * 20.0,
        "LoFi 3 kHz S&H must produce strong 2560/3440 Hz clock-decimation images (crushed={crushed_alias:.4}, clean={clean_alias:.5})"
    );
    assert!(
        crushed_harm3 > 0.003 && crushed_harm3 > clean_harm3 * 50.0,
        "LoFi 4-bit quantization must produce odd harmonic distortion (crushed_3rd={crushed_harm3:.4}, clean_3rd={clean_harm3:.5})"
    );

    // 2. VCA 4-quadrant audio-rate Ring Mod: 300 Hz * 120 Hz -> sidebands at 180 Hz & 420 Hz,
    //    with 300 Hz carrier strongly suppressed!
    let ring_rack = RackPatch {
        kit_name: String::new(),
        zones: vec![RackZone {
            id: 1,
            name: "RingMod".into(),
            low_midi: 0,
            high_midi: 127,
            root_midi: 60,
            low_vel: 0.0,
            high_vel: 1.0,
            gain_db: 0.0,
            pan: 0.0,
            choke_group: None,
            voice: VoiceSettings::default(),
            modules: vec![
                RackModule::new(
                    1,
                    "Carrier",
                    ModuleKind::Vco(VcoModule {
                        waveform: Waveform::Sine,
                        pitch_tracking: PitchTracking::Fixed,
                        fixed_hz: 300.0,
                        level: 0.8,
                        ..VcoModule::default()
                    }),
                ),
                RackModule::new(
                    2,
                    "Modulator",
                    ModuleKind::Vco(VcoModule {
                        waveform: Waveform::Sine,
                        pitch_tracking: PitchTracking::Fixed,
                        fixed_hz: 120.0,
                        level: 1.0,
                        ..VcoModule::default()
                    }),
                ),
                RackModule::new(
                    3,
                    "Ring VCA",
                    ModuleKind::Vca(VcaModule {
                        gain: 1.0,
                        bias: 0.0,
                    }),
                ),
                RackModule::new(4, "Amp VCA", ModuleKind::Vca(VcaModule::default())),
                RackModule::new(
                    5,
                    "Env",
                    ModuleKind::Env(EnvModule {
                        attack_ms: 1.0,
                        sustain: 1.0,
                        ..EnvModule::default()
                    }),
                ),
                RackModule::new(6, "Out", ModuleKind::Out(OutModule::default())),
            ],
            cables: vec![
                PatchCable::new(1, 1, 3, InPort::In, 1.0),
                PatchCable::new(2, 2, 3, InPort::Cv, 1.0),
                PatchCable::new(3, 3, 4, InPort::In, 1.0),
                PatchCable::new(4, 5, 4, InPort::Cv, 1.0),
                PatchCable::new(5, 4, 6, InPort::In, 1.0),
            ],
        }],
        gain_db: 0.0,
        pan: 0.0,
        tune_semitones: 0.0,
        cutoff_hz: 20_000.0,
        resonance: 0.0,
        panel_pins: Vec::new(),
    };
    let ring_frames = render_single_hit(ring_rack, 60, 4.0, 4, sr, Vec::new());
    let ring_mono: Vec<f64> = ring_frames[2_400..16_800]
        .iter()
        .map(|f| f64::from(f[0]))
        .collect();
    let sb_180 = goertzel_magnitude(&ring_mono, f64::from(sr), 180.0);
    let sb_420 = goertzel_magnitude(&ring_mono, f64::from(sr), 420.0);
    let carrier_300 = goertzel_magnitude(&ring_mono, f64::from(sr), 300.0);
    assert!(
        sb_180 > 0.15 && sb_420 > 0.15,
        "Ring Mod must generate strong 180 Hz ({sb_180:.4}) and 420 Hz ({sb_420:.4}) sidebands"
    );
    assert!(
        carrier_300 < sb_180 * 0.05,
        "balanced Ring Mod must suppress 300 Hz carrier ({carrier_300:.5} vs sideband {sb_180:.4})"
    );

    // 3. z-1 feedback cable: Drive -> Filter.In creates a deterministic 1-sample feedback edge!
    let make_fb_rack = |with_feedback: bool| {
        let mut cables = vec![
            PatchCable::new(1, 1, 2, InPort::In, 1.0),
            PatchCable::new(2, 2, 3, InPort::In, 1.0),
            PatchCable::new(3, 3, 4, InPort::In, 1.0),
            PatchCable::new(4, 5, 4, InPort::Cv, 1.0),
            PatchCable::new(5, 4, 6, InPort::In, 1.0),
        ];
        if with_feedback {
            cables.push(PatchCable::new(6, 3, 2, InPort::In, 0.55));
        }
        RackPatch {
            kit_name: String::new(),
            zones: vec![RackZone {
                id: 1,
                name: "Feedback".into(),
                low_midi: 0,
                high_midi: 127,
                root_midi: 60,
                low_vel: 0.0,
                high_vel: 1.0,
                gain_db: 0.0,
                pan: 0.0,
                choke_group: None,
                voice: VoiceSettings::default(),
                modules: vec![
                    RackModule::new(
                        1,
                        "VCO",
                        ModuleKind::Vco(VcoModule {
                            waveform: Waveform::Saw,
                            level: 0.55,
                            ..VcoModule::default()
                        }),
                    ),
                    RackModule::new(
                        2,
                        "Filter",
                        ModuleKind::Filter(FilterModule {
                            mode: FilterMode::LowPass,
                            cutoff_hz: 1_200.0,
                            resonance: 0.25,
                            drive: 0.0,
                            key_tracking: 0.0,
                            vel_amount: 0.0,
                            mix: 1.0,
                        }),
                    ),
                    RackModule::new(
                        3,
                        "Drive",
                        ModuleKind::Drive(DriveModule {
                            mode: DriveMode::Tape,
                            drive: 0.45,
                            asymmetry: 0.1,
                            tone_hz: 10_000.0,
                            output_db: -3.0,
                            mix: 1.0,
                        }),
                    ),
                    RackModule::new(4, "VCA", ModuleKind::Vca(VcaModule::default())),
                    RackModule::new(
                        5,
                        "Env",
                        ModuleKind::Env(EnvModule {
                            attack_ms: 2.0,
                            sustain: 0.8,
                            ..EnvModule::default()
                        }),
                    ),
                    RackModule::new(6, "Out", ModuleKind::Out(OutModule::default())),
                ],
                cables,
            }],
            gain_db: 0.0,
            pan: 0.0,
            tune_semitones: 0.0,
            cutoff_hz: 20_000.0,
            resonance: 0.0,
            panel_pins: Vec::new(),
        }
    };
    let fb_rack = make_fb_rack(true);
    let compiled_fb = compile_zone_graph(&fb_rack.zones[0]).unwrap();
    assert!(
        compiled_fb.has_feedback(),
        "Drive -> Filter.In cycle must be marked as z-1 feedback"
    );
    assert!(
        compiled_fb.cables.iter().any(|c| c.feedback),
        "at least one compiled cable must carry feedback == true"
    );
    let no_fb_frames = render_single_hit(make_fb_rack(false), 60, 4.0, 4, sr, Vec::new());
    let fb_frames = render_single_hit(fb_rack, 60, 4.0, 4, sr, Vec::new());
    let fb_diff: f64 = fb_frames
        .iter()
        .zip(&no_fb_frames)
        .map(|(a, b)| f64::from((a[0] - b[0]).abs()))
        .fold(0.0_f64, f64::max);
    assert!(
        fb_diff > 0.05 && peak_mono(&fb_frames) <= 1.0,
        "z-1 feedback must audibly shape signal while staying bounded (diff={fb_diff:.4}, peak={:.4})",
        peak_mono(&fb_frames)
    );

    // 4. Sampler pitched by Env via PitchCv: steady 220 Hz WAV sweeps +12 st (440 Hz) down to 220 Hz!
    let dir = tempfile::tempdir().unwrap();
    let wav_path = dir.path().join("tone220.wav");
    {
        let mut w = hound::WavWriter::create(
            &wav_path,
            hound::WavSpec {
                channels: 1,
                sample_rate: sr,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for i in 0..sr {
            let s = (TAU * 220.0 * f64::from(i) / f64::from(sr)).sin() * 0.65;
            w.write_sample((s * f64::from(i16::MAX)).round() as i16)
                .unwrap();
        }
        w.finalize().unwrap();
    }
    let sample = load_sample_file(&wav_path, Some("./tone220.wav".into())).unwrap();
    let pitched_smp_rack = RackPatch {
        kit_name: String::new(),
        zones: vec![RackZone {
            id: 1,
            name: "PitchedSmp".into(),
            low_midi: 0,
            high_midi: 127,
            root_midi: 60,
            low_vel: 0.0,
            high_vel: 1.0,
            gain_db: 0.0,
            pan: 0.0,
            choke_group: None,
            voice: VoiceSettings::default(),
            modules: vec![
                RackModule::new(
                    1,
                    "Sampler",
                    ModuleKind::Sampler(SamplerModule {
                        path: Some("./tone220.wav".into()),
                        root_midi: 60,
                        one_shot: true,
                        ..SamplerModule::default()
                    }),
                ),
                RackModule::new(
                    2,
                    "Pitch Env",
                    ModuleKind::Env(EnvModule {
                        mode: EnvMode::HoldDecay,
                        attack_ms: 0.1,
                        hold_ms: 25.0,
                        decay_ms: 50.0,
                        sustain: 0.0,
                        release_ms: 10.0,
                        trigger_mode: TriggerMode::OneShot,
                        delay_ms: 0.0,
                        bursts: 1,
                        burst_spacing_ms: 10.0,
                    }),
                ),
                RackModule::new(3, "Out", ModuleKind::Out(OutModule::default())),
            ],
            cables: vec![
                PatchCable::new(1, 2, 1, InPort::PitchCv, 12.0),
                PatchCable::new(2, 1, 3, InPort::In, 1.0),
            ],
        }],
        gain_db: 0.0,
        pan: 0.0,
        tune_semitones: 0.0,
        cutoff_hz: 20_000.0,
        resonance: 0.0,
        panel_pins: Vec::new(),
    };
    let smp_frames = render_single_hit(
        pitched_smp_rack,
        60,
        4.0,
        6,
        sr,
        vec![(module_sample_key(1, 1, 1), sample)],
    );
    // During the 25 ms hold window (2..22 ms), PitchCv=+12 st -> ~440 Hz!
    // After decay (200..260 ms), PitchCv~=0 -> ~220 Hz!
    let early_mono: Vec<f64> = smp_frames[96..1056]
        .iter()
        .map(|f| f64::from(f[0]))
        .collect();
    let late_mono: Vec<f64> = smp_frames[9_600..12_480]
        .iter()
        .map(|f| f64::from(f[0]))
        .collect();
    let early_440 = goertzel_magnitude(&early_mono, f64::from(sr), 440.0);
    let early_220 = goertzel_magnitude(&early_mono, f64::from(sr), 220.0);
    let late_220 = goertzel_magnitude(&late_mono, f64::from(sr), 220.0);
    let late_440 = goertzel_magnitude(&late_mono, f64::from(sr), 440.0);
    assert!(
        early_440 > early_220 * 4.0,
        "Sampler PitchCv=+12 st hold window must pitch 220 Hz sample to 440 Hz (440={early_440:.4}, 220={early_220:.4})"
    );
    assert!(
        late_220 > late_440 * 4.0,
        "Sampler PitchCv after decay must return cleanly to 220 Hz (220={late_220:.4}, 440={late_440:.4})"
    );
}

#[test]
fn cable_modulation_macro_binding_and_json_round_trip() {
    let sr = 24_000u32;
    let rack = RackPatch::acid_303();
    // Cable 5 is Filter Env -> Acid Filter.Cv (pinned as "Env Mod" in canonical acid_303).
    let cable_param = InstrumentParam::Cable { zone: 1, cable: 5 };
    assert!(
        rack.panel_pins.iter().any(|pin| pin.target == cable_param),
        "acid_303 must pin Filter Env -> Filter.Cv cable"
    );
    let cable_addr = ParamAddress::Cable {
        instrument: 1,
        zone: 1,
        cable: 5,
    };
    assert_eq!(cable_addr.as_instrument(), Some((1, cable_param)));
    assert!(cable_addr.is_automatable());

    let mut p = Project::empty();
    p.bpm = 120.0;
    p.bars = 2;
    p.mixer
        .channels
        .push(Channel::new(1, ChannelKind::Audio, "Acid"));
    p.instruments.push(InstrumentInstance {
        id: 1,
        name: "Acid303".into(),
        rack: rack.clone(),
        channel: 1,
    });
    let step = Step {
        active: true,
        degree: 1,
        velocity: 0.85,
        gate: 0.75,
        ..Step::default()
    };
    p.patterns.push(Pattern {
        id: 1,
        name: "Pulse".into(),
        kind: PatternKind::Notes {
            steps: vec![step.clone(), Step::default(), step, Step::default()],
        },
    });
    // Ramp normalized pattern sweeping Filter Env -> Filter.Cv cable depth from 0 to +5 octaves.
    let norm_lo = normalize_cable_amount(ModuleKindTag::Filter, InPort::Cv, 0.0);
    let norm_hi = normalize_cable_amount(ModuleKindTag::Filter, InPort::Cv, 5.0);
    p.patterns.push(Pattern {
        id: 2,
        name: "CableSweep".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 32,
            mode: ValueMode::Ramp,
            points: vec![
                ValuePoint {
                    step: 0,
                    value: norm_lo,
                },
                ValuePoint {
                    step: 31,
                    value: norm_hi,
                },
            ],
        },
    });
    p.parts.push(Part::new(1, "AcidPart", 1, 1));
    let mut lane = Lane::new(1, "Acid");
    lane.placements.push(Placement::new(1, 1, 0, 32));
    p.lanes.push(lane);

    // 1. Static MacroControl bound to ParamAddress::Cable changes rendered sound audibly!
    let mut mac = MacroControl::new(0, "Env Sweep");
    mac.value = 0.0;
    mac.bindings
        .push(MacroBinding::new(cable_addr, norm_lo, norm_hi));
    p.macros.push(mac);
    let score_lo = p.compile().unwrap();
    let (_, pcm_lo) = render_schedule_pcm(&[(0, &score_lo)], 0, 16, Vec::new(), sr).unwrap();

    p.macros[0].value = 1.0;
    let score_hi = p.compile().unwrap();
    let (_, pcm_hi) = render_schedule_pcm(&[(0, &score_hi)], 0, 16, Vec::new(), sr).unwrap();
    assert_ne!(
        pcm_lo, pcm_hi,
        "MacroControl bound to ParamAddress::Cable must audibly change rendered output"
    );

    // 2. Add a ModLane modulation targeting the exact same Cable parameter and verify JSON round-trip.
    p.modulations.push(Modulation {
        id: 1,
        name: "AutoEnvMod".into(),
        target: ModulationTarget::Instrument {
            instrument: 1,
            parameter: cable_param,
        },
        pattern: 2,
    });
    let mut json_buf = Vec::new();
    write_document_json(&mut json_buf, &p, &LiveFlags::default()).unwrap();
    let raw: serde_json::Value = serde_json::from_slice(&json_buf).unwrap();
    assert_eq!(raw["version"], VERSION);
    assert_eq!(VERSION, 1);
    let (reloaded_proj, _) = parse_document_json(&json_buf).unwrap();
    assert_eq!(reloaded_proj, p);
    let score_mod = reloaded_proj.compile().unwrap();
    assert!(
        score_mod.mod_lanes.iter().any(|m| m.target == cable_param),
        "compiled score must preserve ModLane targeting InstrumentParam::Cable"
    );
    let (rep_mod, _) = render_schedule_pcm(&[(0, &score_mod)], 0, 32, Vec::new(), sr).unwrap();
    assert!(rep_mod.output.peak[0] > 0.05 && rep_mod.channels.iter().all(|c| !c.clipped));

    // 3. InstrumentPreset JSON round-trip.
    let dir = tempfile::tempdir().unwrap();
    let preset_path = dir.path().join("acid-custom.preset.json");
    let preset = InstrumentPreset {
        version: INSTRUMENT_PRESET_VERSION,
        name: "Acid Custom".into(),
        category: Some("Modular".into()),
        patch: rack,
    };
    assert_eq!(INSTRUMENT_PRESET_VERSION, 1);
    preset.save(&preset_path).unwrap();
    let loaded_preset = InstrumentPreset::load(&preset_path).unwrap();
    assert_eq!(loaded_preset, preset);
}

#[test]
fn render_kit_round_trip_emits_rack_kit_and_plays_back_with_choke_groups() {
    let dir = tempfile::tempdir().unwrap();
    let src_path = dir.path().join("synth-kit-source.json");
    let out_samples = dir.path().join("rendered-kit");
    let out_preset = dir.path().join("presets/kits/mini-909.json");

    // Build a self-contained synthetic 3-voice drum source project: BD, CH, OH.
    let mut src = Project::empty();
    src.title = "Mini 909".into();
    src.bpm = 120.0;
    src.bars = 1;
    src.mixer.master.strip.gain = 1.0;

    let make_hat_rack = |decay_ms: f32| RackPatch {
        kit_name: String::new(),
        zones: vec![RackZone {
            id: 1,
            name: "Hat".into(),
            low_midi: 0,
            high_midi: 127,
            root_midi: 60,
            low_vel: 0.0,
            high_vel: 1.0,
            gain_db: 0.0,
            pan: 0.0,
            choke_group: None,
            voice: VoiceSettings::default(),
            modules: vec![
                RackModule::new(
                    1,
                    "Noise",
                    ModuleKind::Noise(NoiseModule {
                        color: NoiseColor::White,
                        highpass_hz: 6_000.0,
                        lowpass_hz: 16_000.0,
                        level: 0.65,
                    }),
                ),
                RackModule::new(2, "VCA", ModuleKind::Vca(VcaModule::default())),
                RackModule::new(
                    3,
                    "Env",
                    ModuleKind::Env(EnvModule {
                        mode: EnvMode::HoldDecay,
                        attack_ms: 0.2,
                        hold_ms: 0.0,
                        decay_ms,
                        sustain: 0.0,
                        release_ms: 10.0,
                        trigger_mode: TriggerMode::OneShot,
                        delay_ms: 0.0,
                        bursts: 1,
                        burst_spacing_ms: 10.0,
                    }),
                ),
                RackModule::new(4, "Out", ModuleKind::Out(OutModule::default())),
            ],
            cables: vec![
                PatchCable::new(1, 1, 2, InPort::In, 1.0),
                PatchCable::new(2, 3, 2, InPort::Cv, 1.0),
                PatchCable::new(3, 2, 4, InPort::In, 1.0),
            ],
        }],
        gain_db: 0.0,
        pan: 0.0,
        tune_semitones: 0.0,
        cutoff_hz: 20_000.0,
        resonance: 0.0,
        panel_pins: Vec::new(),
    };

    let voice_specs: [(&str, RackPatch); 3] = [
        ("BD", build_tr909_low_tom_rack(0.2)),
        ("CH", make_hat_rack(30.0)),
        ("OH", make_hat_rack(900.0)),
    ];
    let hit = Step {
        active: true,
        degree: 1,
        velocity: 0.85,
        gate: 1.0,
        ..Step::default()
    };
    let mut pat_steps = vec![Step::default(); 16];
    pat_steps[0] = hit;
    src.patterns.push(Pattern {
        id: 1,
        name: "SingleHit".into(),
        kind: PatternKind::Notes { steps: pat_steps },
    });
    for (idx, (name, rack)) in voice_specs.into_iter().enumerate() {
        let base = (idx as u64 + 1) * 10;
        src.mixer
            .channels
            .push(Channel::new(base, ChannelKind::Audio, name));
        src.instruments.push(InstrumentInstance {
            id: base + 1,
            name: name.into(),
            rack,
            channel: base,
        });
        src.parts
            .push(Part::new(base + 2, format!("{name}Part"), 1, base + 1));
        let mut lane = Lane::new(base + 3, name);
        lane.placements
            .push(Placement::new(base + 4, base + 2, 0, 16));
        src.lanes.push(lane);
    }
    let mut doc_bytes = Vec::new();
    write_document_json(&mut doc_bytes, &src, &LiveFlags::default()).unwrap();
    std::fs::write(&src_path, doc_bytes).unwrap();

    let out_synth_preset = dir.path().join("presets/kits/mini-909-synth.json");
    let out_voices_dir = dir.path().join("presets/drums");

    // Run `--render-kit` host pipeline emitting baked kit + live synth kit + single-voice presets!
    let summary = render_kit(&KitRenderOptions {
        source_project: &src_path,
        out_samples: Some(&out_samples),
        out_preset: Some(&out_preset),
        out_synth_preset: Some(&out_synth_preset),
        out_voices_dir: Some(&out_voices_dir),
        kit_name_override: Some("Mini 909 Kit"),
        sample_rate: 24_000,
        extra_sample_dirs: &[],
    })
    .unwrap();
    assert_eq!(summary.kit_name, "Mini 909 Kit");
    assert_eq!(summary.synth_kit_name, "Mini 909 Kit Synth");
    assert_eq!(summary.voices.len(), 3);
    assert_eq!(summary.synth_zone_count, 3);
    assert_eq!(summary.voice_presets.len(), 3);
    assert_eq!(summary.voices[0].midi, drum_midi::BD);
    assert_eq!(summary.voices[0].choke_group, None);
    assert_eq!(summary.voices[1].midi, drum_midi::CH);
    assert_eq!(summary.voices[1].choke_group, Some(1));
    assert_eq!(summary.voices[2].midi, drum_midi::OH);
    assert_eq!(summary.voices[2].choke_group, Some(1));

    // Verify emitted live synth multi-zone kit + individual 1-zone modular voice presets!
    let loaded_synth_kit = InstrumentPreset::load(&out_synth_preset).unwrap();
    assert_eq!(loaded_synth_kit.version, INSTRUMENT_PRESET_VERSION);
    assert_eq!(loaded_synth_kit.category.as_deref(), Some("Kit"));
    assert_eq!(loaded_synth_kit.name, "Mini 909 Kit Synth");
    assert_eq!(loaded_synth_kit.patch.zones.len(), 3);
    assert!(!loaded_synth_kit.patch.has_samplers());
    assert_eq!(loaded_synth_kit.patch.zones[0].root_midi, drum_midi::BD);
    assert_eq!(loaded_synth_kit.patch.zones[1].choke_group, Some(1));
    assert_eq!(loaded_synth_kit.patch.zones[2].choke_group, Some(1));

    for vp in &summary.voice_presets {
        assert!(
            vp.path.is_file(),
            "missing voice preset {}",
            vp.path.display()
        );
        let voice_preset = InstrumentPreset::load(&vp.path).unwrap();
        assert_eq!(voice_preset.category.as_deref(), Some("Drum"));
        assert_eq!(voice_preset.patch.zones.len(), 1);
        assert_eq!(voice_preset.patch.zones[0].low_midi, 0);
        assert_eq!(voice_preset.patch.zones[0].high_midi, 127);
        assert_eq!(voice_preset.patch.zones[0].root_midi, vp.midi);
        assert_eq!(
            voice_preset.patch.zones[0].gain_db, 0.0,
            "single-voice preset should lift bus trim onto rack.gain_db and keep zone.gain_db == 0.0"
        );
        assert!(!voice_preset.patch.has_samplers());
    }

    // Verify generated WAV files carry exact RIFF `inst`/`smpl` root MIDI metadata.
    for voice in &summary.voices {
        assert!(voice.wav_path.is_file());
        let loaded = load_sample_file(&voice.wav_path, Some(voice.locator.clone())).unwrap();
        assert!(loaded.meta.has_root);
        assert_eq!(loaded.meta.root_midi, voice.midi);
        assert_eq!(loaded.meta.low_midi, Some(voice.midi));
        assert_eq!(loaded.meta.high_midi, Some(voice.midi));
    }

    // Verify emitted Rack kit preset structure and reload via PresetCatalog.
    let loaded_preset = InstrumentPreset::load(&out_preset).unwrap();
    assert_eq!(loaded_preset.version, INSTRUMENT_PRESET_VERSION);
    assert_eq!(loaded_preset.category.as_deref(), Some("Kit"));
    assert_eq!(loaded_preset.patch.kit_name, "Mini 909 Kit");
    assert_eq!(loaded_preset.patch.zones.len(), 3);
    for (zone, voice) in loaded_preset.patch.zones.iter().zip(&summary.voices) {
        assert_eq!(zone.low_midi, voice.midi);
        assert_eq!(zone.high_midi, voice.midi);
        assert_eq!(zone.root_midi, voice.midi);
        assert_eq!(zone.choke_group, voice.choke_group);
        assert_eq!(
            zone.modules.len(),
            2,
            "each kit zone must be [Sampler -> Out]"
        );
        assert_eq!(zone.cables.len(), 1);
        assert!(matches!(zone.modules[0].kind, ModuleKind::Sampler(_)));
        assert!(matches!(zone.modules[1].kind, ModuleKind::Out(_)));
    }

    let catalog =
        PresetCatalog::discover(&[dir.path().join("presets")], None, &UserConfig::default());
    let discovered = catalog
        .find_by_category("Kit", "Mini 909 Kit")
        .expect("rendered kit preset must be discoverable via PresetCatalog");

    // Load the rendered Rack kit into a new Project and verify CH chokes OH cleanly.
    let render_kit_pattern = |include_choke_hit: bool| -> Vec<[f32; 2]> {
        let mut kit_proj = Project::empty();
        kit_proj.bpm = 120.0;
        kit_proj.bars = 1;
        kit_proj.mixer.master.strip.gain = 1.0;
        kit_proj
            .mixer
            .channels
            .push(Channel::new(1, ChannelKind::Audio, "Kit"));
        // Make kit sample paths relative to `dir.path()` for clean Project validation!
        let mut patch = discovered.patch.clone();
        for (_, _, smp) in patch.sampler_modules_mut() {
            if let Some(p) = &smp.path {
                let name = Path::new(p).file_name().unwrap().to_string_lossy();
                smp.path = Some(format!("rendered-kit/{name}"));
            }
        }
        kit_proj.instruments.push(InstrumentInstance {
            id: 1,
            name: "Mini909".into(),
            rack: patch,
            channel: 1,
        });
        let (oh_deg, oh_acc) = kit_proj.midi_to_degree(drum_midi::OH);
        let (ch_deg, ch_acc) = kit_proj.midi_to_degree(drum_midi::CH);
        let mut steps = vec![Step::default(); 16];
        steps[0] = Step {
            active: true,
            degree: oh_deg,
            accidental: oh_acc,
            velocity: 0.9,
            gate: 1.0,
            ..Step::default()
        };
        if include_choke_hit {
            steps[1] = Step {
                active: true,
                degree: ch_deg,
                accidental: ch_acc,
                velocity: 0.9,
                gate: 1.0,
                ..Step::default()
            };
        }
        kit_proj.patterns.push(Pattern {
            id: 1,
            name: "HatChoke".into(),
            kind: PatternKind::Notes { steps },
        });
        kit_proj.parts.push(Part::new(1, "KitPart", 1, 1));
        let mut lane = Lane::new(1, "Kit");
        lane.placements.push(Placement {
            id: 1,
            part: 1,
            start_step: 0,
            length_steps: 16,
            phase: PhaseMode::Song,
            offset_steps: 0,
            exceptions: Vec::new(),
        });
        kit_proj.lanes.push(lane);
        let score = kit_proj.compile().unwrap();
        let mut lib = SampleLibrary::from_roots_only(
            &[dir.path().to_path_buf()],
            Some(&dir.path().join("playback.json")),
        );
        let loaded = load_project_samples(&score, &mut lib);
        assert!(
            loaded.issues.is_empty(),
            "rendered kit samples must load cleanly: {:?}",
            loaded.issues
        );
        let (_, frames) = render_schedule_f32(&[(0, &score)], 0, 8, loaded.bank, 24_000).unwrap();
        frames
    };

    let unchoked = render_kit_pattern(false);
    let choked = render_kit_pattern(true);
    // At 120 BPM, 1 step = 125 ms. OH starts at 0 ms (900 ms decay), CH hits at 125 ms (30 ms decay)
    // and chokes OH via choke_group 1 -> by 180..320 ms, choked tail must be far quieter than unchoked!
    let tail_a = (0.180 * 24_000.0) as usize;
    let tail_b = (0.320 * 24_000.0) as usize;
    let unchoked_rms = rms_mono(&unchoked[tail_a..tail_b]);
    let choked_rms = rms_mono(&choked[tail_a..tail_b]);
    assert!(
        unchoked_rms > 0.01,
        "unchoked OH must ring into 180..320 ms (rms={unchoked_rms:.5})"
    );
    assert!(
        choked_rms < unchoked_rms * 0.2,
        "CH on choke_group 1 must silence OH tail (unchoked={unchoked_rms:.5}, choked={choked_rms:.5})"
    );

    // Verify rendered baked kit and live synth kit match 1:1 in playback level (BD @ vel 1.0).
    let render_bd = |patch: RackPatch| -> (f64, f64) {
        let mut p = Project::empty();
        p.bpm = 120.0;
        p.bars = 1;
        p.mixer.master.strip.gain = 1.0;
        p.mixer
            .channels
            .push(Channel::new(1, ChannelKind::Audio, "Kit"));
        p.instruments.push(InstrumentInstance {
            id: 1,
            name: "Kit".into(),
            rack: patch,
            channel: 1,
        });
        let (bd_deg, bd_acc) = p.midi_to_degree(drum_midi::BD);
        let mut steps = vec![Step::default(); 16];
        steps[0] = Step {
            active: true,
            degree: bd_deg,
            accidental: bd_acc,
            velocity: 1.0,
            gate: 1.0,
            ..Step::default()
        };
        p.patterns.push(Pattern {
            id: 1,
            name: "BD".into(),
            kind: PatternKind::Notes { steps },
        });
        p.parts.push(Part::new(1, "BDPart", 1, 1));
        let mut lane = Lane::new(1, "Kit");
        lane.placements.push(Placement {
            id: 1,
            part: 1,
            start_step: 0,
            length_steps: 16,
            phase: PhaseMode::Song,
            offset_steps: 0,
            exceptions: Vec::new(),
        });
        p.lanes.push(lane);
        let score = p.compile().unwrap();
        let mut lib = SampleLibrary::from_roots_only(
            &[dir.path().to_path_buf()],
            Some(&dir.path().join("playback.json")),
        );
        let loaded = load_project_samples(&score, &mut lib);
        assert!(loaded.issues.is_empty());
        let (_, frames) = render_schedule_f32(&[(0, &score)], 0, 8, loaded.bank, 24_000).unwrap();
        let peak = frames.iter().fold(0.0_f64, |m, f| {
            m.max(f64::from(f[0].abs())).max(f64::from(f[1].abs()))
        });
        (peak, rms_mono(&frames))
    };
    let mut baked_patch = discovered.patch.clone();
    for (_, _, smp) in baked_patch.sampler_modules_mut() {
        if let Some(p) = &smp.path {
            let name = Path::new(p).file_name().unwrap().to_string_lossy();
            smp.path = Some(format!("rendered-kit/{name}"));
        }
    }
    let (baked_peak, baked_rms) = render_bd(baked_patch);
    let (synth_peak, synth_rms) = render_bd(loaded_synth_kit.patch.clone());
    let rms_diff_db = (20.0 * (synth_rms / baked_rms).log10()).abs();
    let peak_diff_db = (20.0 * (synth_peak / baked_peak).log10()).abs();
    assert!(
        rms_diff_db < 0.5 && peak_diff_db < 1.5,
        "render-kit synth preset must match baked kit 1:1 (baked peak={baked_peak:.4}/rms={baked_rms:.4}, synth peak={synth_peak:.4}/rms={synth_rms:.4}, dRMS={rms_diff_db:.2} dB, dPeak={peak_diff_db:.2} dB)"
    );
}

#[test]
fn rack_resonator_string_and_nonlinear_plate_roundtrip_and_render() {
    let sr = 44_100u32;

    // 1. Chromatic plucked `ResonatorMode::String` (A4 = MIDI 69 -> 440 Hz + 880 Hz harmonics).
    let string_rack = RackPatch {
        kit_name: String::new(),
        zones: vec![RackZone {
            id: 1,
            name: "Pluck String".into(),
            low_midi: 0,
            high_midi: 127,
            root_midi: 60,
            low_vel: 0.0,
            high_vel: 1.0,
            gain_db: 0.0,
            pan: 0.0,
            choke_group: None,
            voice: VoiceSettings::default(),
            modules: vec![
                RackModule::new(
                    1,
                    "Pick",
                    ModuleKind::Noise(NoiseModule {
                        color: NoiseColor::White,
                        highpass_hz: 120.0,
                        lowpass_hz: 12_000.0,
                        level: 1.0,
                    }),
                ),
                RackModule::new(
                    2,
                    "Pick Env",
                    ModuleKind::Env(EnvModule {
                        mode: EnvMode::Adsr,
                        attack_ms: 0.1,
                        hold_ms: 0.0,
                        decay_ms: 3.0,
                        sustain: 0.0,
                        release_ms: 3.0,
                        trigger_mode: TriggerMode::OneShot,
                        delay_ms: 0.0,
                        bursts: 1,
                        burst_spacing_ms: 10.0,
                    }),
                ),
                RackModule::new(3, "Pick VCA", ModuleKind::Vca(VcaModule::default())),
                RackModule::new(
                    4,
                    "String",
                    ModuleKind::Resonator(ResonatorModule {
                        mode: ResonatorMode::String,
                        pitch_tracking: PitchTracking::Chromatic,
                        tune_semitones: 0.0,
                        fine_cents: 0.0,
                        fixed_hz: 440.0,
                        structure: 0.0,
                        decay_ms: 900.0,
                        brightness: 0.65,
                        position: 0.25,
                        mix: 1.0,
                    }),
                ),
                RackModule::new(5, "Out", ModuleKind::Out(OutModule::default())),
            ],
            cables: vec![
                PatchCable::new(1, 1, 3, InPort::In, 1.0),
                PatchCable::new(2, 2, 3, InPort::Cv, 1.0),
                PatchCable::new(3, 3, 4, InPort::In, 1.2),
                PatchCable::new(4, 4, 5, InPort::In, 0.8),
            ],
        }],
        gain_db: 0.0,
        pan: 0.0,
        tune_semitones: 0.0,
        cutoff_hz: 20_000.0,
        resonance: 0.0,
        panel_pins: Vec::new(),
    };
    let str_frames = render_single_hit(string_rack.clone(), 69, 1.0, 8, sr, Vec::new());
    let mono: Vec<f64> = str_frames[882..12_000]
        .iter()
        .map(|f| 0.5 * f64::from(f[0] + f[1]))
        .collect();
    let mag_440 = goertzel_magnitude(&mono, f64::from(sr), 440.0);
    let mag_880 = goertzel_magnitude(&mono, f64::from(sr), 880.0);
    let mag_620 = goertzel_magnitude(&mono, f64::from(sr), 620.0);
    assert!(
        mag_440 > mag_620 * 6.0 && mag_880 > mag_620 * 3.0,
        "String resonator at A4 must ring at 440/880 Hz (440={mag_440:.4}, 880={mag_880:.4}, 620={mag_620:.4})"
    );

    // 2. Nonlinear `z-1` `Drive(Tape)` feedback `ResonatorMode::Plate` -> `LoFi` cymbal path +
    //    Project JSON roundtrip.
    let plate_rack = RackPatch {
        kit_name: String::new(),
        zones: vec![RackZone {
            id: 1,
            name: "Bronze Plate".into(),
            low_midi: 0,
            high_midi: 127,
            root_midi: 60,
            low_vel: 0.0,
            high_vel: 1.0,
            gain_db: 0.0,
            pan: 0.0,
            choke_group: None,
            voice: VoiceSettings::default(),
            modules: vec![
                RackModule::new(
                    1,
                    "Strike",
                    ModuleKind::Noise(NoiseModule {
                        color: NoiseColor::White,
                        highpass_hz: 1200.0,
                        lowpass_hz: 16_000.0,
                        level: 1.0,
                    }),
                ),
                RackModule::new(
                    2,
                    "Strike Env",
                    ModuleKind::Env(EnvModule {
                        mode: EnvMode::Adsr,
                        attack_ms: 0.2,
                        hold_ms: 0.0,
                        decay_ms: 35.0,
                        sustain: 0.0,
                        release_ms: 10.0,
                        trigger_mode: TriggerMode::OneShot,
                        delay_ms: 0.0,
                        bursts: 1,
                        burst_spacing_ms: 10.0,
                    }),
                ),
                RackModule::new(3, "Strike VCA", ModuleKind::Vca(VcaModule::default())),
                RackModule::new(
                    4,
                    "Bronze Plate",
                    ModuleKind::Resonator(ResonatorModule {
                        mode: ResonatorMode::Plate,
                        pitch_tracking: PitchTracking::Fixed,
                        tune_semitones: 0.0,
                        fine_cents: 0.0,
                        fixed_hz: 560.0,
                        structure: 0.48,
                        decay_ms: 1_200.0,
                        brightness: 0.72,
                        position: 0.58,
                        mix: 1.0,
                    }),
                ),
                RackModule::new(
                    5,
                    "Plate NL",
                    ModuleKind::Drive(DriveModule {
                        mode: DriveMode::Tape,
                        drive: 0.45,
                        asymmetry: 0.0,
                        tone_hz: 14_000.0,
                        output_db: 0.0,
                        mix: 1.0,
                    }),
                ),
                RackModule::new(
                    6,
                    "10-Bit DAC",
                    ModuleKind::LoFi(LoFiModule {
                        bits: 10.0,
                        rate_hz: 26_000.0,
                        mix: 0.75,
                    }),
                ),
                RackModule::new(7, "Out", ModuleKind::Out(OutModule::default())),
            ],
            cables: vec![
                PatchCable::new(1, 1, 3, InPort::In, 1.0),
                PatchCable::new(2, 2, 3, InPort::Cv, 1.0),
                PatchCable::new(3, 3, 4, InPort::In, 1.8),
                PatchCable::new(4, 4, 5, InPort::In, 0.30),
                PatchCable::new(5, 5, 4, InPort::In, 0.025), // z-1 nonlinear feedback!
                PatchCable::new(6, 4, 6, InPort::In, 1.8),
                PatchCable::new(7, 6, 7, InPort::In, 0.9),
            ],
        }],
        gain_db: 0.0,
        pan: 0.0,
        tune_semitones: 0.0,
        cutoff_hz: 20_000.0,
        resonance: 0.0,
        panel_pins: Vec::new(),
    };
    let graph = compile_zone_graph(&plate_rack.zones[0]).unwrap();
    assert!(
        graph.cables.iter().any(|c| c.feedback),
        "Plate -> Drive -> Plate cycle must compile a deterministic z-1 feedback edge"
    );
    let plate_frames = render_single_hit(plate_rack.clone(), 60, 1.0, 8, sr, Vec::new());
    let early_rms = rms_mono(&plate_frames[0..4_410]);
    let tail_rms = rms_mono(&plate_frames[13_230..22_050]);
    assert!(
        early_rms > 0.05 && tail_rms > 0.001 && tail_rms < early_rms,
        "Plate resonator must strike loudly and decay smoothly past 300 ms (early={early_rms:.4}, tail={tail_rms:.4})"
    );

    // Project JSON roundtrip preserves Resonator params cleanly.
    let mut proj = Project::empty();
    proj.mixer
        .channels
        .push(Channel::new(1, ChannelKind::Audio, "Cymbal"));
    proj.instruments.push(InstrumentInstance {
        id: 1,
        name: "PlateCymbal".into(),
        rack: plate_rack.clone(),
        channel: 1,
    });
    let mut json = Vec::new();
    write_document_json(&mut json, &proj, &LiveFlags::default()).unwrap();
    let (reloaded, _) = parse_document_json(&json).unwrap();
    assert_eq!(reloaded.version, VERSION);
    assert_eq!(reloaded.instruments[0].rack, plate_rack);
}

#[test]
fn rack_param_descriptors_and_module_metadata_stay_aligned() {
    use cadence_core::mixer::EffectDevice;
    use cadence_core::rack::{ModuleParam, RackParam, ZoneParam};

    let patch = RackPatch::default_synth();
    let rack_descs = patch.descriptors();
    assert_eq!(rack_descs.len(), RackParam::ALL.len());
    for (idx, &param) in RackParam::ALL.iter().enumerate() {
        assert_eq!(param as usize, idx);
        assert_eq!(RackParam::from_slot(idx), Some(param));
        assert_eq!(rack_descs[idx].id, idx);
        assert_eq!(RackParam::parse(param.name()), Some(param));
    }

    for &zparam in &ZoneParam::ALL {
        assert_eq!(ZoneParam::parse(zparam.name()), Some(zparam));
    }

    assert_eq!(ModuleKindTag::ALL.len(), 11);
    for (idx, &mparam) in ModuleParam::ALL.iter().enumerate() {
        assert_eq!(mparam as usize, idx);
        assert_eq!(ModuleParam::from_slot(idx), Some(mparam));
    }
    for &tag in ModuleKindTag::ALL {
        assert!(
            !tag.params().is_empty(),
            "ModuleKindTag::{tag:?} must expose faceplate parameters"
        );
        for &param in tag.params() {
            assert_eq!(
                ModuleParam::parse_for_kind(param.name(), tag),
                Some(param),
                "ModuleParam::{param:?} name '{}' must round-trip for {tag:?}",
                param.name()
            );
        }
    }

    assert_eq!(
        ModuleKindTag::Filter.params(),
        &[
            ModuleParam::FilterMode,
            ModuleParam::Cutoff,
            ModuleParam::Resonance,
            ModuleParam::Drive,
            ModuleParam::KeyTrack,
            ModuleParam::VelAmount,
            ModuleParam::Mix,
        ]
    );

    for kind in [
        "eq", "filter", "comp", "limiter", "drive", "delay", "chorus", "reverb",
    ] {
        assert!(EffectDevice::is_kind_name(kind));
    }
}
