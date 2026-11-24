use super::*;
use cadence_core::material::{InstrumentInstance, Project};
use cadence_core::mixer::{
    Channel, ChannelKind, ChannelSend, ChorusPatch, DelayPatch, DrivePatch, Duck, EffectDevice,
    EffectKind, EqPatch, MixerAutomation, MixerBreakpoint, ReverbPatch,
};
use cadence_core::params::ValueMode;
use cadence_core::rack::RackPatch;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static COUNT_HEAP: Cell<bool> = const { Cell::new(false) };
    static HEAP_OPS: Cell<usize> = const { Cell::new(0) };
}

struct CheckedAllocator;
#[global_allocator]
static ALLOCATOR: CheckedAllocator = CheckedAllocator;

fn count_heap() {
    if COUNT_HEAP.try_with(Cell::get).unwrap_or(false) {
        let _ = HEAP_OPS.try_with(|n| n.set(n.get() + 1));
    }
}

unsafe impl GlobalAlloc for CheckedAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count_heap();
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count_heap();
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        count_heap();
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count_heap();
        unsafe { System.realloc(ptr, layout, size) }
    }
}

pub(crate) fn assert_no_heap_operations(run: impl FnOnce()) {
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            COUNT_HEAP.with(|value| value.set(false));
        }
    }
    HEAP_OPS.with(|value| value.set(0));
    COUNT_HEAP.with(|value| value.set(true));
    let guard = Guard;
    run();
    drop(guard);
    assert_eq!(
        HEAP_OPS.with(Cell::get),
        0,
        "processing/adoption must neither allocate nor free"
    );
}

const RATE: u32 = 48_000;

fn close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 1e-3,
        "actual={actual} expected={expected}"
    );
}

fn stereo_score() -> ScoreWindow {
    let mut p = Project::empty();
    p.bars = 4;
    p.mixer.master.strip.gain = 1.0;
    p.mixer.channels = vec![
        Channel::new(1, ChannelKind::Audio, "Lead"),
        Channel::new(2, ChannelKind::Audio, "Bass"),
        Channel::new(3, ChannelKind::Group, "Music"),
        Channel::new(4, ChannelKind::Return, "Echo"),
    ];
    p.instruments = vec![
        InstrumentInstance {
            id: 1,
            name: "Lead".into(),
            rack: RackPatch::default_synth(),
            channel: 1,
        },
        InstrumentInstance {
            id: 2,
            name: "Bass".into(),
            rack: RackPatch::default_synth(),
            channel: 2,
        },
    ];
    p.compile().unwrap()
}

fn meter(mixer: &MixerEngine, channel: ChannelRef) -> ChannelMeter {
    mixer
        .meters()
        .find(|m| m.channel == channel)
        .expect("channel meter")
}

fn settle(mixer: &mut MixerEngine, sources: [[f32; 2]; MAX_INSTRUMENTS]) -> [f32; 2] {
    let mut out = [0.0; 2];
    for _ in 0..4000 {
        out = mixer.process(&sources, [0.0; 2]);
    }
    out
}

#[test]
fn audio_channel_gain_pan_group_routing_and_master_metering() {
    let mut score = stereo_score();
    score.mixer.channel_mut(1).unwrap().strip.gain = 0.5;
    score.mixer.channel_mut(1).unwrap().strip.pan = -0.5;
    score.mixer.channel_mut(1).unwrap().output = ChannelRef::Channel(3);
    score.mixer.channel_mut(3).unwrap().strip.gain = 0.8;

    let mut mix = MixerEngine::new(&score, RATE);
    mix.update_controls(&score, 0.0, 0.0);

    let mut sources = [[0.0; 2]; MAX_INSTRUMENTS];
    sources[0] = [1.0, 1.0];
    let out = settle(&mut mix, sources);
    // Unity-normalized equal-power stereo balance at pan = -0.5 -> L = 1.0, R = tan(pi/8) = sqrt(2) - 1.
    let [pl, pr] = cadence_core::dsp::pan_stereo_balance(-0.5);
    assert_eq!(pl, 1.0);
    close(out[0], 0.5 * 0.8);
    close(out[1], 0.5 * pr * 0.8);
    close(meter(&mix, ChannelRef::Channel(1)).output_peak[0], 0.5);
    close(meter(&mix, ChannelRef::Channel(3)).output_peak[0], 0.4);
    close(meter(&mix, ChannelRef::Master).output_peak[0], 0.4);
}

#[test]
fn channel_mute_solo_and_return_wet_only_isolation() {
    let mut score = stereo_score();
    score.mixer.channel_mut(1).unwrap().sends.push(ChannelSend {
        target: 4,
        amount: 0.5,
    });
    // Solo channel 1 -> silences channel 2
    score
        .mixer
        .flags
        .set_channel_solo(ChannelRef::Channel(1), true)
        .unwrap();
    let mut mix = MixerEngine::new(&score, RATE);
    mix.update_controls(&score, 0.0, 0.0);

    let mut sources = [[0.0; 2]; MAX_INSTRUMENTS];
    sources[0] = [0.4, 0.4];
    sources[1] = [0.6, 0.6];
    let out = settle(&mut mix, sources);
    // Channel 1 dry (0.4) + return 4 (0.4 * 0.5 = 0.2) = 0.6; Channel 2 muted by solo.
    close(out[0], 0.6);

    // Solo return 4 -> wet-only monitoring!
    score
        .mixer
        .flags
        .set_channel_solo(ChannelRef::Channel(4), true)
        .unwrap();
    mix.adopt(&score, true);
    mix.update_controls(&score, 0.0, 0.0);
    let wet = settle(&mut mix, sources);
    close(wet[0], 0.2);
}

#[test]
fn sidechain_ducking_attenuates_target_channel_and_recovers() {
    let mut score = stereo_score();
    score.mixer.ducks.push(Duck {
        id: 1,
        name: "Kick Pump".into(),
        source_part: 7,
        target: ChannelRef::Channel(2),
        depth: 0.8,
        recovery_ms: 80.0,
    });
    let mut mix = MixerEngine::new(&score, RATE);
    mix.update_controls(&score, 0.0, 0.0);
    let mut sources = [[0.0; 2]; MAX_INSTRUMENTS];
    sources[1] = [1.0, 1.0];

    let before = settle(&mut mix, sources);
    close(before[0], 1.0);

    mix.trigger_part(7);
    let mut min_out = 1.0_f32;
    for _ in 0..400 {
        let s = mix.process(&sources, [0.0; 2]);
        min_out = min_out.min(s[0]);
    }
    assert!(min_out < 0.35, "ducking must attenuate: min={min_out}");

    // Recover completely past 80ms + tail
    for _ in 0..8000 {
        mix.process(&sources, [0.0; 2]);
    }
    let after = mix.process(&sources, [0.0; 2]);
    close(after[0], 1.0);
}

#[test]
fn mixer_automation_ramps_channel_gain_and_device_parameters() {
    let mut score = stereo_score();
    let mut delay = EffectDevice::delay(10);
    delay.kind = EffectKind::Delay(DelayPatch {
        steps: 2.0,
        feedback: 0.0,
        mix: 0.0,
        ..DelayPatch::default()
    });
    score.mixer.channel_mut(1).unwrap().inserts.push(delay);
    score.mixer.automations.push(MixerAutomation {
        target: MixerParamTarget::Channel {
            channel: ChannelRef::Channel(1),
            param: ChannelParam::Gain,
        },
        mode: ValueMode::Ramp,
        points: vec![
            MixerBreakpoint {
                step: 0,
                value: 0.0,
            },
            MixerBreakpoint {
                step: 4,
                value: 1.0,
            },
        ],
    });
    let mut mix = MixerEngine::new(&score, RATE);
    mix.update_controls(&score, 2.0, 2.0);
    let mut sources = [[0.0; 2]; MAX_INSTRUMENTS];
    sources[0] = [1.0, 1.0];
    let out = settle(&mut mix, sources);
    close(out[0], cadence_core::dsp::norm_to_channel_gain(0.5));
    // At step 3.0 (norm = 0.75 unity detent) -> exact 1.0; at step 4.0 (norm = 1.0) -> +6.02 dB (2.0).
    mix.update_controls(&score, 3.0, 3.0);
    let out_unity = settle(&mut mix, sources);
    close(out_unity[0], 1.0);
    mix.update_controls(&score, 4.0, 4.0);
    let out_boost = settle(&mut mix, sources);
    close(out_boost[0], cadence_core::dsp::MAX_CHANNEL_GAIN);
}

#[test]
fn reverb_delay_eq_chorus_and_drive_inserts_process_audio_cleanly() {
    let mut score = stereo_score();
    let mut eq = EffectDevice::eq(11);
    eq.kind = EffectKind::Eq(EqPatch {
        hpf_hz: 250.0,
        low_db: -6.0,
        mid_db: 2.0,
        high_db: 4.0,
        ..EqPatch::default()
    });
    let mut chorus = EffectDevice::chorus(12);
    chorus.kind = EffectKind::Chorus(ChorusPatch {
        rate_hz: 1.0,
        depth: 0.6,
        width: 1.0,
        mix: 0.5,
    });
    let mut drive = EffectDevice::drive(13);
    drive.kind = EffectKind::Drive(DrivePatch {
        drive: 0.7,
        tone_hz: 6000.0,
        output_db: 0.0,
        mix: 1.0,
        ..DrivePatch::default()
    });
    let mut verb = EffectDevice::reverb(14);
    verb.kind = EffectKind::Reverb(ReverbPatch {
        mix: 0.4,
        ..ReverbPatch::default()
    });
    score.mixer.channel_mut(1).unwrap().inserts = vec![eq, chorus, drive, verb];

    let mut mix = MixerEngine::new(&score, RATE);
    mix.update_controls(&score, 0.0, 0.0);

    let mut stereo_diff = 0.0_f64;
    let mut peak = 0.0_f32;
    assert_no_heap_operations(|| {
        mix.update_controls(&score, 0.0, 0.0);
        for i in 0..4800 {
            let mut sources = [[0.0; 2]; MAX_INSTRUMENTS];
            if i < 2400 {
                let x = (std::f64::consts::TAU * 440.0 * i as f64 / RATE as f64).sin() as f32 * 0.5;
                sources[0] = [x, x];
            }
            let out = mix.process(&sources, [0.0; 2]);
            assert!(out[0].is_finite() && out[1].is_finite());
            peak = peak.max(out[0].abs()).max(out[1].abs());
            if i > 1000 && i < 2400 {
                stereo_diff += f64::from(out[0] - out[1]).powi(2);
            }
        }
    });
    assert!(peak > 0.1 && peak < 2.0, "peak={peak}");
    assert!(
        stereo_diff > 0.1,
        "chorus + reverb chain creates stereo spread: diff={stereo_diff}"
    );
    // Reverb tail rings past dry stop (sample 2400)
    let tail = mix.process(&[[0.0; 2]; MAX_INSTRUMENTS], [0.0; 2]);
    assert!(tail[0].abs() + tail[1].abs() > 1e-5, "reverb tail active");
}

#[test]
fn filter_and_compressor_inserts_process_with_zero_allocations() {
    let mut score = stereo_score();
    let mut flt = EffectDevice::filter(21);
    flt.kind = EffectKind::Filter(cadence_core::mixer::FilterPatch {
        mode: cadence_core::rack::FilterMode::AcidLowPass,
        cutoff_hz: 600.0,
        resonance: 0.72,
        drive: 0.25,
        mix: 1.0,
        ..Default::default()
    });
    let mut cmp = EffectDevice::compressor(22);
    cmp.kind = EffectKind::Compressor(cadence_core::mixer::CompressorPatch {
        threshold_db: -18.0,
        ratio: 6.0,
        attack_ms: 4.0,
        release_ms: 80.0,
        makeup_db: 2.0,
        mix: 1.0,
    });
    score.mixer.channel_mut(1).unwrap().inserts = vec![flt];
    score.mixer.master.inserts = vec![cmp];
    score.mixer.automations.push(MixerAutomation {
        target: MixerParamTarget::Device {
            device_id: 21,
            param: cadence_core::mixer::EffectParam::Cutoff,
        },
        mode: ValueMode::Ramp,
        points: vec![
            MixerBreakpoint {
                step: 0,
                value: 0.15,
            },
            MixerBreakpoint {
                step: 4,
                value: 0.85,
            },
        ],
    });

    let mut mix = MixerEngine::new(&score, RATE);
    let mut peak = 0.0_f32;
    assert_no_heap_operations(|| {
        for i in 0..4800 {
            if i % 64 == 0 {
                let pos = (i as f64 / 4800.0) * 4.0;
                mix.update_controls(&score, pos, pos);
            }
            let x = (std::f64::consts::TAU * 220.0 * i as f64 / RATE as f64).sin() as f32 * 0.8;
            let mut sources = [[0.0; 2]; MAX_INSTRUMENTS];
            sources[0] = [x, x];
            let out = mix.process(&sources, [0.0; 2]);
            assert!(out[0].is_finite() && out[1].is_finite());
            peak = peak.max(out[0].abs());
        }
    });
    assert!(peak > 0.05 && peak < 2.0, "peak={peak}");
}

#[test]
fn limiter_insert_processes_zero_alloc_enforces_ceiling_and_reports_gr_meter() {
    let mut score = stereo_score();
    let mut lim = EffectDevice::limiter(31);
    let ceiling_db = -4.0_f32;
    let ceiling_lin = cadence_core::dsp::db_to_lin(ceiling_db);
    lim.kind = EffectKind::Limiter(cadence_core::mixer::LimiterPatch {
        ceiling_db,
        input_drive_db: 12.0,
        release_ms: 50.0,
        mix: 1.0,
    });
    score.mixer.master.inserts = vec![lim];

    let mut mix = MixerEngine::new(&score, RATE);
    let mut peak = 0.0_f32;
    let mut max_gr = 0.0_f32;
    assert_no_heap_operations(|| {
        mix.update_controls(&score, 0.0, 0.0);
        for i in 0..4800 {
            let x = (std::f64::consts::TAU * 330.0 * i as f64 / RATE as f64).sin() as f32 * 0.9;
            let mut sources = [[0.0; 2]; MAX_INSTRUMENTS];
            sources[0] = [x, x];
            let out = mix.process(&sources, [0.0; 2]);
            assert!(out[0].is_finite() && out[1].is_finite());
            peak = peak.max(out[0].abs()).max(out[1].abs());
            if let Some(m) = mix.effect_meters().find(|m| m.device_id == 31) {
                max_gr = max_gr.max(m.gain_reduction_db);
            }
        }
    });
    assert!(
        peak <= ceiling_lin + 1e-4,
        "limiter ceiling violated in MixerEngine: peak={peak} > ceiling={ceiling_lin}"
    );
    assert!(
        max_gr > 8.0,
        "expected live limiter GR meter > 8 dB, got {max_gr}"
    );
}

#[test]
fn lazy_delay_allocation_topological_groups_muted_return_solo_preview_and_pdc() {
    // 1. Zero Delay devices -> 0 allocated Delay buffers.
    let mut score = stereo_score();
    let mut mix = MixerEngine::new(&score, RATE);
    assert_eq!(mix.allocated_delay_count(), 0);

    // Off-RT delay handoff installs buffer with ZERO heap operations on RT thread!
    let mut delay = EffectDevice::delay(99);
    delay.kind = EffectKind::Delay(DelayPatch {
        steps: 1.0,
        feedback: 0.0,
        mix: 1.0,
        ..DelayPatch::default()
    });
    score.mixer.channel_mut(4).unwrap().inserts.push(delay);
    let mut incoming = [Delay::new(RATE)];
    assert_no_heap_operations(|| {
        mix.install_delay_buffers(&mut incoming);
        mix.adopt(&score, false);
        mix.update_controls(&score, 0.0, 0.0);
    });
    assert_eq!(mix.allocated_delay_count(), 1);
    // Drop the unallocated shell off-RT after the zero-alloc block.
    drop(incoming);

    // 2. Reverse-ordered chained groups (Group B before Group A: Audio -> A -> B -> Master).
    let mut chained = stereo_score();
    chained.mixer.channels = vec![
        Channel::new(1, ChannelKind::Audio, "Lead"),
        Channel::new(10, ChannelKind::Group, "OuterBus"),
        Channel::new(11, ChannelKind::Group, "InnerBus"),
    ];
    chained.mixer.channel_mut(1).unwrap().output = ChannelRef::Channel(11);
    chained.mixer.channel_mut(11).unwrap().output = ChannelRef::Channel(10);
    chained.mixer.channel_mut(11).unwrap().strip.gain = 0.5;
    chained.mixer.channel_mut(10).unwrap().strip.gain = 0.5;
    let mut cmix = MixerEngine::new(&chained, RATE);
    cmix.update_controls(&chained, 0.0, 0.0);
    let mut sources = [[0.0; 2]; MAX_INSTRUMENTS];
    sources[0] = [0.8, 0.8];
    let out = settle(&mut cmix, sources);
    close(out[0], 0.2);

    // 3. Muted soloed return does not silence dry mix, and Master mute does not silence preview.
    let mut mscore = stereo_score();
    mscore
        .mixer
        .flags
        .set_channel_solo(ChannelRef::Channel(4), true)
        .unwrap();
    mscore
        .mixer
        .flags
        .set_channel_muted(ChannelRef::Channel(4), true)
        .unwrap();
    let mut mmix = MixerEngine::new(&mscore, RATE);
    mmix.update_controls(&mscore, 0.0, 0.0);
    let dry_out = settle(&mut mmix, sources);
    close(dry_out[0], 0.8);

    mscore
        .mixer
        .flags
        .set_channel_muted(ChannelRef::Master, true)
        .unwrap();
    mmix.adopt(&mscore, false);
    mmix.update_controls(&mscore, 0.0, 0.0);
    let audition_out = mmix.process(&sources, [0.35, -0.35]);
    close(audition_out[0], 0.35);
    close(audition_out[1], -0.35);

    // 4. Parallel Limiter PDC aligns impulse arrival across limited and dry channels.
    let mut pdc_score = stereo_score();
    pdc_score
        .mixer
        .channel_mut(1)
        .unwrap()
        .inserts
        .push(EffectDevice::limiter(55));
    let mut pmix = MixerEngine::new(&pdc_score, RATE);
    pmix.update_controls(&pdc_score, 0.0, 0.0);
    pmix.snap_smoothers();
    let mut peak_ch1 = None;
    let mut peak_ch2 = None;
    for frame in 0..256 {
        let mut s = [[0.0; 2]; MAX_INSTRUMENTS];
        if frame == 0 {
            s[0] = [0.25, 0.0]; // Left = limited channel 1
            s[1] = [0.0, 0.25]; // Right = dry channel 2 (PDC compensated!)
        }
        let y = pmix.process(&s, [0.0; 2]);
        if peak_ch1.is_none() && y[0].abs() > 0.1 {
            peak_ch1 = Some(frame);
        }
        if peak_ch2.is_none() && y[1].abs() > 0.1 {
            peak_ch2 = Some(frame);
        }
    }
    assert_eq!(
        peak_ch1, peak_ch2,
        "PDC must align parallel Limiter and dry channels to exact same frame: {peak_ch1:?} vs {peak_ch2:?}"
    );
    assert_eq!(peak_ch1, Some(limiter_lookahead_frames(f64::from(RATE))));
}

#[test]
fn float_metering_distinguishes_hot_headroom_from_digital_clipping_and_holds_gr() {
    use cadence_core::dsp::db_to_lin;
    use cadence_core::mixer::{CompressorPatch, LimiterPatch};

    let hot_lin = db_to_lin(4.0); // ~1.585 (+4 dBFS internal float signal)

    // 1. Hot internal channel (+4 dBFS pre-fader) attenuated by its fader -> channel is `hot`, NOT `clipped`; Master is clean!
    let mut score = stereo_score();
    score.mixer.channel_mut(1).unwrap().strip.gain = 0.4; // attenuates +4 dBFS -> ~0.634 (-4 dBFS)
    let mut mix = MixerEngine::new(&score, RATE);
    mix.update_controls(&score, 0.0, 0.0);
    mix.snap_smoothers();

    let mut sources = [[0.0; 2]; MAX_INSTRUMENTS];
    sources[0] = [hot_lin, hot_lin * 0.9];
    let out = settle(&mut mix, sources);
    assert!(out[0] < 0.7 && out[0] > 0.6);

    let ch1 = meter(&mix, ChannelRef::Channel(1));
    let mst = meter(&mix, ChannelRef::Master);
    assert!(
        ch1.hot,
        "internal channel above 0 dBFS must latch hot=true: {ch1:?}"
    );
    assert!(
        !ch1.clipped,
        "internal float bus must NEVER latch clipped on finite >0 dBFS: {ch1:?}"
    );
    assert!(
        ch1.input_peak_hold[0] > 1.5 && ch1.output_peak_hold[0] < 0.7,
        "pre-fader input_peak_hold must reflect +4 dBFS while post-fader output_peak_hold reflects fader attenuation: {ch1:?}"
    );
    assert!(!mst.hot && !mst.clipped, "clean master bus: {mst:?}");

    // 2. Hot internal channel routed at unity into Master with Master Limiter (-1 dBFS ceiling):
    //    channel is hot & unclipped, Master is hot (pre-limiter > 0 dBFS) & unclipped (post-limiter <= 1.0)!
    let mut lim_score = stereo_score();
    let comp_id = 61_u64;
    let lim_id = 62_u64;
    lim_score
        .mixer
        .channel_mut(1)
        .unwrap()
        .inserts
        .push(EffectDevice {
            id: comp_id,
            name: "Ch Comp".into(),
            bypassed: false,
            kind: EffectKind::Compressor(CompressorPatch {
                threshold_db: -12.0,
                ratio: 4.0,
                attack_ms: 1.0,
                release_ms: 50.0,
                makeup_db: 8.0,
                mix: 1.0,
            }),
        });
    lim_score.mixer.master.inserts.push(EffectDevice {
        id: lim_id,
        name: "Master Limiter".into(),
        bypassed: false,
        kind: EffectKind::Limiter(LimiterPatch {
            ceiling_db: -1.0,
            input_drive_db: 0.0,
            release_ms: 50.0,
            mix: 1.0,
        }),
    });
    let mut lmix = MixerEngine::new(&lim_score, RATE);
    lmix.update_controls(&lim_score, 0.0, 0.0);
    lmix.snap_smoothers();

    for i in 0..4800 {
        let x = (std::f64::consts::TAU * 220.0 * i as f64 / RATE as f64).sin() as f32 * hot_lin;
        let mut s = [[0.0; 2]; MAX_INSTRUMENTS];
        s[0] = [x, x];
        let y = lmix.process(&s, [0.0; 2]);
        assert!(y[0].abs() <= 1.0 && y[1].abs() <= 1.0);
    }
    let lch1 = meter(&lmix, ChannelRef::Channel(1));
    let lmst = meter(&lmix, ChannelRef::Master);
    assert!(lch1.hot && !lch1.clipped, "hot limited ch1: {lch1:?}");
    assert!(
        lmst.hot && !lmst.clipped,
        "master receiving >0 dBFS tamed by Master Limiter must be hot=true, clipped=false: {lmst:?}"
    );
    // Verify EffectMeter::gr_hold_db holds peak GR while signal drops to silence (< 1.5s hold),
    // and decays cleanly after 1.5s!
    let comp_m_hot = lmix
        .effect_meters()
        .find(|m| m.device_id == comp_id)
        .expect("comp meter");
    let lim_m_hot = lmix
        .effect_meters()
        .find(|m| m.device_id == lim_id)
        .expect("lim meter");
    assert!(
        comp_m_hot.gain_reduction_db > 2.0 && comp_m_hot.gr_hold_db >= comp_m_hot.gain_reduction_db,
        "comp GR hold: {comp_m_hot:?}"
    );
    assert!(
        lim_m_hot.gain_reduction_db > 1.0 && lim_m_hot.gr_hold_db >= lim_m_hot.gain_reduction_db,
        "lim GR hold: {lim_m_hot:?}"
    );
    let silence = [[0.0; 2]; MAX_INSTRUMENTS];
    // Run 0.5 s of silence: live GR releases toward 0, but 1.5 s gr_hold_db stays pinned!
    for _ in 0..(RATE / 2) {
        lmix.process(&silence, [0.0; 2]);
    }
    let lim_m_held = lmix
        .effect_meters()
        .find(|m| m.device_id == lim_id)
        .expect("lim meter held");
    assert!(
        lim_m_held.gain_reduction_db < 0.2,
        "live GR should release after 0.5 s silence: {lim_m_held:?}"
    );
    assert!(
        (lim_m_held.gr_hold_db - lim_m_hot.gr_hold_db).abs() < 0.25,
        "gr_hold_db must hold during 1.5 s window: hot={lim_m_hot:?} held={lim_m_held:?}"
    );
    // Run 2.5 s more silence: gr_hold_db decays cleanly below held peak, while render_effect_stats preserves session max!
    for _ in 0..(RATE * 5 / 2) {
        lmix.process(&silence, [0.0; 2]);
    }
    let lim_m_decayed = lmix
        .effect_meters()
        .find(|m| m.device_id == lim_id)
        .expect("lim meter decayed");
    assert!(
        lim_m_decayed.gr_hold_db < lim_m_hot.gr_hold_db * 0.25,
        "gr_hold_db must decay after 1.5 s hold expires: {lim_m_decayed:?}"
    );
    let (_, lim_render_max, lim_render_avg) = lmix
        .render_effect_stats()
        .find(|(id, _, _)| *id == lim_id)
        .expect("lim render stats");
    assert!(
        lim_render_max >= lim_m_hot.gr_hold_db - 0.05 && lim_render_avg > 0.0,
        "render_effect_stats must preserve full-session max GR: max={lim_render_max}, avg={lim_render_avg}"
    );

    // 3. Master post-fader > 1.0 sets `clipped = true` on Master (while internal channel remains `clipped = false`).
    let clip_score = stereo_score();
    let mut cmix = MixerEngine::new(&clip_score, RATE);
    cmix.update_controls(&clip_score, 0.0, 0.0);
    cmix.snap_smoothers();
    let mut hot_src = [[0.0; 2]; MAX_INSTRUMENTS];
    hot_src[0] = [hot_lin, hot_lin];
    settle(&mut cmix, hot_src);
    let c_ch1 = meter(&cmix, ChannelRef::Channel(1));
    let c_mst = meter(&cmix, ChannelRef::Master);
    assert!(c_ch1.hot && !c_ch1.clipped, "ch1 unclipped: {c_ch1:?}");
    assert!(
        c_mst.clipped && c_mst.output_peak_hold[0] > 1.5,
        "master post-fader > 1.0 must set clipped=true: {c_mst:?}"
    );

    // 4. Non-finite input sets `clipped = true` while keeping output finite.
    let nan_score = stereo_score();
    let mut nmix = MixerEngine::new(&nan_score, RATE);
    nmix.update_controls(&nan_score, 0.0, 0.0);
    let mut nan_src = [[0.0; 2]; MAX_INSTRUMENTS];
    nan_src[0] = [f32::NAN, 0.25];
    let nan_out = nmix.process(&nan_src, [0.0; 2]);
    assert!(nan_out[0].is_finite() && nan_out[1].is_finite());
    let n_ch1 = meter(&nmix, ChannelRef::Channel(1));
    assert!(
        n_ch1.clipped,
        "non-finite input must set clipped=true: {n_ch1:?}"
    );
}

#[test]
fn group_to_group_chained_routing_pdc_and_mute_solo_ancestry() {
    use cadence_core::mixer::LimiterPatch;

    // Topology:
    //   ch1 (Audio) -> ch3 (Group A, gain 0.5) -> ch4 (Group B, gain 0.5) -> Master
    //   ch2 (Audio) -> Master (direct parallel path)
    // Put ch4 BEFORE ch3 in `channels` to prove topological ordering does not depend on vec order!
    let mut p = Project::empty();
    p.bars = 4;
    p.mixer.master.strip.gain = 1.0;
    let mut ch1 = Channel::new(1, ChannelKind::Audio, "Drum");
    ch1.strip.gain = 1.0;
    ch1.output = ChannelRef::Channel(3);
    let mut ch2 = Channel::new(2, ChannelKind::Audio, "Direct");
    ch2.strip.gain = 1.0;
    ch2.output = ChannelRef::Master;
    let mut grp_b = Channel::new(4, ChannelKind::Group, "MasterSub");
    grp_b.strip.gain = 0.5;
    grp_b.output = ChannelRef::Master;
    let mut grp_a = Channel::new(3, ChannelKind::Group, "DrumsBus");
    grp_a.strip.gain = 0.5;
    grp_a.output = ChannelRef::Channel(4);
    p.mixer.channels = vec![ch1, ch2, grp_b, grp_a];
    p.instruments = vec![
        InstrumentInstance {
            id: 1,
            name: "Drum".into(),
            rack: RackPatch::default_synth(),
            channel: 1,
        },
        InstrumentInstance {
            id: 2,
            name: "Direct".into(),
            rack: RackPatch::default_synth(),
            channel: 2,
        },
    ];
    p.validate().unwrap();
    let score = p.compile().unwrap();
    let mut engine = MixerEngine::new(&score, RATE);
    engine.update_controls(&score, 0.0, 0.0);
    engine.snap_smoothers();

    // 1. Chained gain: 1.0 * 0.5 (Group A) * 0.5 (Group B) = 0.25 with zero heap ops!
    let mut src = [[0.0; 2]; MAX_INSTRUMENTS];
    src[0] = [0.8, 0.8];
    let mut out = [0.0; 2];
    assert_no_heap_operations(|| {
        for _ in 0..256 {
            out = engine.process(&src, [0.0; 2]);
        }
    });
    close(out[0], 0.2);
    close(out[1], 0.2);

    // 2. Mute on ancestor Group B silences ch1 while leaving ch2 untouched.
    let mut muted_score = score.clone();
    muted_score
        .mixer
        .flags
        .set_channel_muted(ChannelRef::Channel(4), true)
        .unwrap();
    engine.adopt(&muted_score, true);
    engine.update_controls(&muted_score, 0.0, 0.0);
    engine.snap_smoothers();
    src[0] = [0.8, 0.8];
    src[1] = [0.3, 0.3];
    let muted_out = settle(&mut engine, src);
    close(muted_out[0], 0.3);

    // 3. Solo on ancestor Group B passes ch1 through Group A -> Group B and mutes ch2.
    let mut solo_score = score.clone();
    solo_score
        .mixer
        .flags
        .set_channel_solo(ChannelRef::Channel(4), true)
        .unwrap();
    engine.adopt(&solo_score, true);
    engine.update_controls(&solo_score, 0.0, 0.0);
    engine.snap_smoothers();
    let solo_out = settle(&mut engine, src);
    close(solo_out[0], 0.2);

    // 4. Chained PDC: put a Limiter on Group A AND a Limiter on Group B (2x lookahead),
    // verify an impulse fed simultaneously into ch1 (via Group A -> Group B) and ch2 (direct)
    // lands on the EXACT same Master output frame!
    let mut pdc_proj = p.clone();
    for id in [3u64, 4u64] {
        let ch = pdc_proj
            .mixer
            .channels
            .iter_mut()
            .find(|c| c.id == id)
            .unwrap();
        ch.strip.gain = 1.0;
        ch.inserts.push(EffectDevice {
            id: 47 + id,
            name: format!("Lim {id}"),
            bypassed: false,
            kind: EffectKind::Limiter(LimiterPatch::default()),
        });
    }
    let pdc_score = pdc_proj.compile().unwrap();
    let mut pdc_eng = MixerEngine::new(&pdc_score, RATE);
    pdc_eng.update_controls(&pdc_score, 0.0, 0.0);
    pdc_eng.snap_smoothers();

    let mut ch1_peak_idx = None;
    let mut ch2_peak_idx = None;
    let lookahead = limiter_lookahead_frames(f64::from(RATE));
    // Measure ch1 (through Group A + Group B limiters) impulse arrival frame
    for i in 0..(lookahead * 4) {
        let mut s = [[0.0; 2]; MAX_INSTRUMENTS];
        if i == 0 {
            s[0] = [0.25, 0.25];
        }
        let y = pdc_eng.process(&s, [0.0; 2]);
        if ch1_peak_idx.is_none() && y[0].abs() > 0.1 {
            ch1_peak_idx = Some(i);
        }
    }
    // Reset and measure ch2 (direct path, compensated by PDC!) impulse arrival frame
    let mut pdc_eng2 = MixerEngine::new(&pdc_score, RATE);
    pdc_eng2.update_controls(&pdc_score, 0.0, 0.0);
    pdc_eng2.snap_smoothers();
    for i in 0..(lookahead * 4) {
        let mut s = [[0.0; 2]; MAX_INSTRUMENTS];
        if i == 0 {
            s[1] = [0.25, 0.25];
        }
        let y = pdc_eng2.process(&s, [0.0; 2]);
        if ch2_peak_idx.is_none() && y[0].abs() > 0.1 {
            ch2_peak_idx = Some(i);
        }
    }
    assert_eq!(
        ch1_peak_idx,
        Some(lookahead * 2),
        "chained Group A + Group B limiters must delay by 2x lookahead"
    );
    assert_eq!(
        ch2_peak_idx, ch1_peak_idx,
        "direct path PDC must align exactly with chained Group A -> Group B latency"
    );
}

#[test]
fn delay_stereo_and_drive_modes_process_and_automate_with_zero_rt_allocations() {
    use cadence_core::mixer::{DelayMode, DriveMode, EffectParam};
    let mut score = stereo_score();
    let mut delay = EffectDevice::delay(201);
    delay.kind = EffectKind::Delay(DelayPatch {
        mode: DelayMode::Stereo,
        steps: 1.0,
        feedback: 0.5,
        mix: 1.0,
    });
    let mut drive = EffectDevice::drive(202);
    drive.kind = EffectKind::Drive(DrivePatch {
        mode: DriveMode::HardClip,
        drive: 0.85,
        asymmetry: 0.0,
        tone_hz: 9_000.0,
        output_db: 0.0,
        mix: 1.0,
    });
    score.mixer.channel_mut(1).unwrap().inserts = vec![drive, delay];
    score.mixer.automations.push(MixerAutomation {
        target: MixerParamTarget::Device {
            device_id: 201,
            param: EffectParam::DelayMode,
        },
        mode: ValueMode::Hold,
        points: vec![
            MixerBreakpoint {
                step: 0,
                value: 1.0, // Stereo
            },
            MixerBreakpoint {
                step: 2,
                value: 0.0, // PingPong
            },
        ],
    });
    score.mixer.automations.push(MixerAutomation {
        target: MixerParamTarget::Device {
            device_id: 202,
            param: EffectParam::DriveMode,
        },
        mode: ValueMode::Hold,
        points: vec![
            MixerBreakpoint {
                step: 0,
                value: 0.5, // HardClip
            },
            MixerBreakpoint {
                step: 2,
                value: 1.0, // Fold
            },
        ],
    });

    let mut mix = MixerEngine::new(&score, RATE);
    let mut sources = [[0.0; 2]; MAX_INSTRUMENTS];
    assert_no_heap_operations(|| {
        for block in 0..4 {
            let step = f64::from(block);
            mix.update_controls(&score, step, step);
            for i in 0..128 {
                let x = ((block * 128 + i) as f32 * 0.04).sin() * 0.35;
                sources[0] = [x, x * 0.5];
                let y = mix.process(&sources, [0.0; 2]);
                assert!(y[0].is_finite() && y[1].is_finite());
            }
        }
    });
}

#[test]
fn fader_headroom_boost_channel_gain_scale_and_automated_macro_sweep() {
    use cadence_core::dsp::{db_to_lin, norm_to_channel_gain, MAX_CHANNEL_GAIN};
    use cadence_core::macros::{MacroBinding, MacroControl};
    use cadence_core::material::{
        Modulation, ModulationTarget, Pattern, PatternKind, Project, ValuePoint, ValueUnit,
    };
    use cadence_core::params::{ParamAddress, ValueMode};

    // 1. +6 dB boost doubles internal 32-bit float bus cleanly: Audio & Group are `hot` (never `clipped`),
    //    Master only sets `clipped` when Master post-fader output exceeds 1.0!
    let mut score = stereo_score();
    score.mixer.channel_mut(1).unwrap().strip.gain = MAX_CHANNEL_GAIN; // +6.02 dB (2.0x)
    score.mixer.channel_mut(1).unwrap().output = ChannelRef::Channel(3); // -> Group 3
    score.mixer.channel_mut(3).unwrap().strip.gain = 1.0;
    score.mixer.master.strip.gain = 0.5; // brings 1.6 cleanly back to 0.8 at DAC!
    score.validate().unwrap();

    let mut mix = MixerEngine::new(&score, RATE);
    mix.update_controls(&score, 0.0, 0.0);
    let mut sources = [[0.0; 2]; MAX_INSTRUMENTS];
    sources[0] = [0.8, 0.8];
    let out_tamed = settle(&mut mix, sources);
    close(out_tamed[0], 0.8);
    let m_ch1 = meter(&mix, ChannelRef::Channel(1));
    let m_grp = meter(&mix, ChannelRef::Channel(3));
    let m_mst = meter(&mix, ChannelRef::Master);
    close(m_ch1.output_peak[0], 1.6);
    assert!(
        m_ch1.hot && !m_ch1.clipped,
        "boosted Audio strip must be hot, not clipped"
    );
    close(m_grp.output_peak[0], 1.6);
    assert!(
        m_grp.hot && !m_grp.clipped,
        "hot Group bus must be hot, not clipped"
    );
    assert!(
        m_mst.hot && !m_mst.clipped,
        "tamed Master (in 1.6, out 0.8) must be hot, not clipped"
    );

    // Push Master to unity -> Master post-fader 1.6 > 1.0 fires `clipped`!
    score.mixer.master.strip.gain = 1.0;
    mix.adopt(&score, true);
    mix.update_controls(&score, 0.0, 0.0);
    let out_hot = settle(&mut mix, sources);
    close(out_hot[0], 1.6);
    assert!(meter(&mix, ChannelRef::Master).clipped);

    // 2. ChannelGainScale multiplies a boosted +3 dB channel smoothly without clamping at 1.0!
    let mut scaled_score = stereo_score();
    let boost_3db = db_to_lin(3.0);
    scaled_score.mixer.channel_mut(1).unwrap().strip.gain = boost_3db;
    scaled_score.mixer.automations.push(MixerAutomation {
        target: MixerParamTarget::ChannelGainScale {
            channel: ChannelRef::Channel(1),
        },
        mode: ValueMode::Hold,
        points: vec![
            MixerBreakpoint {
                step: 0,
                value: 0.9,
            },
            MixerBreakpoint {
                step: 4,
                value: 0.5,
            },
        ],
    });
    let mut scaled_mix = MixerEngine::new(&scaled_score, RATE);
    sources[0] = [1.0, 1.0];
    scaled_mix.update_controls(&scaled_score, 0.0, 0.0);
    let y_hi = settle(&mut scaled_mix, sources);
    assert!(
        y_hi[0] > 1.25,
        "boosted +3 dB * 0.9 must exceed 1.0 without clamping"
    );
    close(y_hi[0], boost_3db * 0.9);
    scaled_mix.update_controls(&scaled_score, 4.0, 4.0);
    let y_lo = settle(&mut scaled_mix, sources);
    close(y_lo[0], boost_3db * 0.5);

    // 3. Automated Project Macro targeting ChannelParam::Gain compiles and sweeps 0.0 -> +6.02 dB
    //    with zero RT allocations!
    let mut proj = Project::demo();
    let target_ch = proj.mixer.channels[0].id;
    let ch_ref = ChannelRef::Channel(target_ch);
    proj.mixer.master.strip.gain = 1.0;
    let ramp_id = proj.max_id() + 1;
    proj.patterns.push(Pattern {
        id: ramp_id,
        name: "FaderBoostRamp".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 16,
            mode: ValueMode::Ramp,
            points: vec![
                ValuePoint {
                    step: 0,
                    value: 0.0,
                },
                ValuePoint {
                    step: 8,
                    value: 1.0,
                },
            ],
        },
    });
    proj.macros.push(MacroControl {
        index: 0,
        name: "Boost".into(),
        value: 0.0,
        bindings: vec![MacroBinding::new(
            ParamAddress::Channel {
                channel: ch_ref,
                param: ChannelParam::Gain,
            },
            0.0,
            1.0,
        )],
    });
    let mod_id = proj.max_id() + 2;
    proj.modulations.push(Modulation {
        id: mod_id,
        name: "M1 Boost".into(),
        target: ModulationTarget::Macro { index: 0 },
        pattern: ramp_id,
    });
    let macro_score = proj.compile().unwrap();
    let mut macro_mix = MixerEngine::new(&macro_score, RATE);
    let inst_idx = macro_score
        .instruments
        .iter()
        .position(|i| i.channel == target_ch)
        .expect("instrument routed to target channel");
    let mut macro_sources = [[0.0; 2]; MAX_INSTRUMENTS];
    macro_sources[inst_idx] = [0.5, 0.5];

    // Step 6 -> norm 0.75 -> exact unity (1.0 * 0.5 = 0.5); Step 8 -> norm 1.0 -> +6.02 dB (2.0 * 0.5 = 1.0).
    let mut out_mid = [0.0; 2];
    let mut out_peak = [0.0; 2];
    assert_no_heap_operations(|| {
        macro_mix.update_controls(&macro_score, 6.0, 6.0);
        for _ in 0..4000 {
            out_mid = macro_mix.process(&macro_sources, [0.0; 2]);
        }
        macro_mix.update_controls(&macro_score, 8.0, 8.0);
        for _ in 0..4000 {
            out_peak = macro_mix.process(&macro_sources, [0.0; 2]);
        }
    });
    // Account for Audio channel routing through Group (if any) in demo project: verify exact 2x ratio!
    let ratio = out_peak[0] / out_mid[0].max(1e-6);
    assert!(
        (ratio - (norm_to_channel_gain(1.0) / norm_to_channel_gain(0.75))).abs() < 0.02,
        "automated Macro norm 0.75 -> 1.0 must double channel gain (+6.02 dB): mid={}, peak={}, ratio={ratio}",
        out_mid[0],
        out_peak[0]
    );
}

#[test]
fn aux_sends_are_post_fader_pre_strip_pan_so_opposite_panned_returns_never_cancel() {
    // Hard-right Audio channel (+1.0 pan) sending at unity to a hard-left Return (-1.0 pan)
    // must deliver dry signal strictly on R and full-strength wet Return strictly on L,
    // while preserving stereo spread and respecting channel fader attenuation & mute.
    let mut score = stereo_score();
    // Silence channel 2 so only channel 1 + return 4 are active.
    score.mixer.channel_mut(2).unwrap().strip.gain = 0.0;
    {
        let ch1 = score.mixer.channel_mut(1).unwrap();
        ch1.output = ChannelRef::Master;
        ch1.strip.gain = 0.5;
        ch1.strip.pan = 1.0; // hard right dry
        ch1.sends = vec![ChannelSend {
            target: 4,
            amount: 1.0,
        }];
    }
    {
        let ret = score.mixer.channel_mut(4).unwrap();
        ret.strip.gain = 1.0;
        ret.strip.pan = -1.0; // hard left return
        ret.inserts.clear();
    }

    let mut mix = MixerEngine::new(&score, RATE);
    let mut sources = [[0.0; 2]; MAX_INSTRUMENTS];
    // Centered mono-like stereo source [0.4, 0.4] at fader 0.5 -> dry R = 0.2, return L = 0.2!
    sources[0] = [0.4, 0.4];
    let mut out = [0.0; 2];
    for _ in 0..4000 {
        out = mix.process(&sources, [0.0; 2]);
    }
    assert!(
        (out[0] - 0.2).abs() < 0.01 && (out[1] - 0.2).abs() < 0.01,
        "hard-right channel -> hard-left return must stay balanced at [0.2, 0.2], got {out:?}"
    );

    // Stereo source [0.6, 0.2] into centered Return (pan 0.0) keeps full stereo L/R image!
    score.mixer.channel_mut(4).unwrap().strip.pan = 0.0;
    score.mixer.channel_mut(1).unwrap().strip.gain = 1.0;
    score.mixer.channel_mut(1).unwrap().strip.pan = 1.0;
    let mut mix_st = MixerEngine::new(&score, RATE);
    sources[0] = [0.6, 0.2];
    let mut out_st = [0.0; 2];
    for _ in 0..4000 {
        out_st = mix_st.process(&sources, [0.0; 2]);
    }
    // Dry (hard right) = [0.0, 0.2]; Return (pre-pan stereo) = [0.6, 0.2] -> Master = [0.6, 0.4]!
    assert!(
        (out_st[0] - 0.6).abs() < 0.01 && (out_st[1] - 0.4).abs() < 0.01,
        "pre-pan send must preserve stereo source into centered return [0.6, 0.4], got {out_st:?}"
    );
}

#[test]
fn adopt_effect_kind_hot_swap_across_all_eight_effects_performs_zero_heap_ops() {
    use cadence_core::mixer::EffectKindTag;

    let mut base = stereo_score();
    let device_id = 77_u64;
    base.mixer
        .channel_mut(1)
        .unwrap()
        .inserts
        .push(EffectDevice::delay(device_id));
    let mut mix = MixerEngine::new(&base, RATE);
    assert_eq!(mix.allocated_delay_count(), 1);

    // Pre-build one ScoreWindow per EffectKindTag off the RT thread.
    let scores: Vec<ScoreWindow> = EffectKindTag::ALL
        .iter()
        .map(|&tag| {
            let mut s = stereo_score();
            s.mixer
                .channel_mut(1)
                .unwrap()
                .inserts
                .push(tag.default_device(device_id));
            s
        })
        .collect();

    let mut sources = [[0.0_f32; 2]; MAX_INSTRUMENTS];
    sources[0] = [0.4, -0.3];

    assert_no_heap_operations(|| {
        for score in &scores {
            mix.adopt(score, false);
            mix.update_controls(score, 0.0, 0.0);
            for _ in 0..128 {
                let y = mix.process(&sources, [0.0; 2]);
                assert!(y[0].is_finite() && y[1].is_finite());
            }
        }
    });
}

#[test]
fn mixer_smoothers_settle_exactly_without_subnormal_stalls_and_cache_pan_weights() {
    use cadence_core::dsp::pan_stereo_balance;

    for sr in [44_100u32, 48_000, 96_000, 192_000] {
        let mut score = stereo_score();
        score.mixer.channel_mut(1).unwrap().strip.pan = -0.65;
        score.mixer.channel_mut(1).unwrap().sends = vec![ChannelSend {
            target: 4,
            amount: 0.75,
        }];
        score.validate().unwrap();

        let mut mix = MixerEngine::new(&score, sr);
        mix.update_controls(&score, 0.0, 0.0);
        mix.snap_smoothers();

        // Cached pan weights must match pan_stereo_balance immediately after snap_smoothers.
        let slot1 = MAX_INSTRUMENTS;
        assert_eq!(
            mix.channels[slot1].cached_pan_lr,
            pan_stereo_balance(-0.65),
            "sr={sr}: cached pan weights out of sync after snap"
        );

        let mut sources = [[0.0_f32; 2]; MAX_INSTRUMENTS];
        sources[0] = [0.5, 0.5];
        let y_panned = mix.process(&sources, [0.0; 2]);
        let [pl, pr] = pan_stereo_balance(-0.65);
        // Channel 1 + Return 4 (amount 0.75, pre-pan) both route to Master.
        close(y_panned[0], 0.5 * pl + 0.5 * 0.75);
        close(y_panned[1], 0.5 * pr + 0.5 * 0.75);

        // Return pan and send amount back to 0.0: smooth_step_f32 must settle to exact 0.0
        // (zero subnormal stall) and refresh cached_pan_lr back to [1.0, 1.0].
        score.mixer.channel_mut(1).unwrap().strip.pan = 0.0;
        score.mixer.channel_mut(1).unwrap().sends[0].amount = 0.0;
        mix.adopt(&score, false);
        mix.update_controls(&score, 0.0, 0.0);
        let settle_frames = (f64::from(sr) * 0.15) as usize;
        for _ in 0..settle_frames {
            let _ = mix.process(&sources, [0.0; 2]);
        }
        assert_eq!(
            mix.channels[slot1].pan, 0.0,
            "sr={sr}: channel pan stalled in subnormals"
        );
        assert_eq!(
            mix.channels[slot1].cached_pan_lr,
            [1.0, 1.0],
            "sr={sr}: cached pan weights did not reset to unity"
        );
        assert_eq!(
            mix.channels[slot1].sends[0].amount, 0.0,
            "sr={sr}: send amount stalled in subnormals"
        );
    }
}
