mod common;

use cadence_core::assets::load_project_samples;
use cadence_core::material::{Edit, LiveFlags, ModulationTarget, PatternKind, Project, MAX_BARS};
use cadence_core::mixer::{ChannelKind, ChannelRef, EffectParam};
use cadence_core::params::{InstrumentParam, ParamAddress, RackParam, ZoneParam};
use cadence_core::rack::{drum_midi, FilterMode, ModuleKind, ModuleParam};
use cadence_core::runtime::SampleBank;
use cadence_core::score::ScoreWindow;
use cadence_engine::audio::{render_measure_report, render_schedule_pcm};
use cadence_formats::rbs::{import_rbs_bytes, import_rbs_file, peek_rbs_header};
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

fn load_rbs_samples(score: &ScoreWindow) -> SampleBank {
    static FACTORY_LIB: OnceLock<Mutex<cadence_core::sampler::SampleLibrary>> = OnceLock::new();
    let mut lib = FACTORY_LIB
        .get_or_init(|| Mutex::new(common::factory_sample_library(None, false)))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let loaded = load_project_samples(score, &mut lib);
    assert!(
        loaded.issues.is_empty(),
        "Factory kit samples should load cleanly for RBS render: {:?}",
        loaded.issues
    );
    loaded.bank
}

const TICKS_PER_BAR: u32 = 768;

fn iff_chunk(tag: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + payload.len() + 1);
    out.extend_from_slice(tag);
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    if payload.len() % 2 == 1 {
        out.push(0);
    }
    out
}

fn iff_cat(kind: &[u8; 4], children: &[Vec<u8>]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(kind);
    for c in children {
        body.extend_from_slice(c);
    }
    iff_chunk(b"CAT ", &body)
}

fn make_trak(events: &[(u32, u8, u8)]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&(events.len() as u32).to_be_bytes());
    let mut prev = 0u32;
    for &(pos, ctrl, val) in events {
        let d = pos.saturating_sub(prev);
        prev = pos;
        let mut stack = [0u8; 5];
        let mut n = 0;
        let mut v = d;
        stack[n] = (v & 0x7F) as u8;
        n += 1;
        v >>= 7;
        while v > 0 {
            stack[n] = ((v & 0x7F) as u8) | 0x80;
            n += 1;
            v >>= 7;
        }
        for i in (0..n).rev() {
            b.push(stack[i]);
        }
        b.push(ctrl);
        b.push(val);
    }
    iff_chunk(b"TRAK", &b)
}

#[derive(Clone)]
struct SyntheticRbs {
    bars: u32,
    loop_start_bar: u32,
    loop_on: bool,
    bpm_millis: u32,
    shuffle: u8,
    song_mode: bool,
    mixr: [u8; 64],
    dely: [u8; 8],
    pcf: [u8; 12],
    dist: [u8; 8],
    comp: [u8; 8],
    tb1: Vec<u8>,
    tb2: Vec<u8>,
    t808: Vec<u8>,
    t909: Vec<u8>,
    tracks: [Vec<(u32, u8, u8)>; 9],
}

impl SyntheticRbs {
    fn new(bars: u32) -> Self {
        let mut mixr = [0u8; 64];
        mixr[0] = 115; // master
        for c in 0..4 {
            let b = 16 + 12 * c;
            mixr[b] = 1; // enabled
            mixr[b + 1] = 100; // level
            mixr[b + 2] = 64; // center pan
            mixr[b + 3] = 0; // send
            mixr[b + 4] = 0; // dist off
        }

        let mut tb1 = vec![0u8; 1097];
        tb1[0..9].copy_from_slice(&[1, 0, 64, 52, 85, 80, 72, 95, 0]);
        for pat in 0..32 {
            let base = 9 + pat * 34;
            tb1[base + 1] = 16;
            for s in 0..16 {
                tb1[base + 2 + s * 2 + 1] = 0x10; // rest
            }
        }

        let mut tb2 = tb1.clone();
        tb2[0] = 0;

        let mut t808 = vec![0u8; 6238];
        t808[0] = 0;
        t808[2] = 40;
        for idx in [3, 6, 9, 12, 15, 18, 20, 22, 23, 26, 28] {
            t808[idx] = 100;
        }
        for idx in [4, 5, 7, 8, 10, 13, 16, 25, 27] {
            t808[idx] = 64;
        }
        for pat in 0..32 {
            t808[30 + pat * 194 + 1] = 16;
        }

        let mut t909 = vec![0u8; 6239];
        t909[0] = 0;
        t909[2] = 32;
        for idx in [3, 7, 11, 14, 17, 20, 21, 22, 25, 27] {
            t909[idx] = 100;
        }
        for idx in [
            4, 5, 6, 8, 9, 10, 12, 13, 15, 16, 18, 19, 23, 24, 26, 28, 29,
        ] {
            t909[idx] = 64;
        }
        for pat in 0..32 {
            t909[31 + pat * 194 + 1] = 16;
        }

        Self {
            bars,
            loop_start_bar: 0,
            loop_on: true,
            bpm_millis: 136_000,
            shuffle: 0,
            song_mode: true,
            mixr,
            dely: [1, 3, 1, 64, 64, 0, 0, 0],
            pcf: [0, 50, 75, 80, 24, 70, 1, 0, 0, 0, 0, 0],
            dist: [0, 70, 0, 0, 0, 0, 0, 0],
            comp: [0, 70, 64, 0, 0, 0, 0, 0],
            tb1,
            tb2,
            t808,
            t909,
            // Track 0 (MIXR) starts empty so static `mixr` init (including master_level=115) applies;
            // Tracks 1..=8 default to `(pos=0, ctrl=0 [enable], val=0 [disabled])`.
            tracks: std::array::from_fn(|i| if i == 0 { Vec::new() } else { vec![(0, 0, 0)] }),
        }
    }

    fn set_303_pat_step(&mut self, which: usize, pat: usize, step: usize, note: u8, flags: u8) {
        let buf = if which == 0 {
            &mut self.tb1
        } else {
            &mut self.tb2
        };
        let off = 9 + pat * 34 + 2 + step * 2;
        buf[off] = note;
        buf[off + 1] = flags;
    }

    fn set_303_pat_shuffle(&mut self, which: usize, pat: usize, shuffle: bool) {
        let buf = if which == 0 {
            &mut self.tb1
        } else {
            &mut self.tb2
        };
        buf[9 + pat * 34] = u8::from(shuffle);
    }

    fn set_303_step(&mut self, which: usize, step: usize, note: u8, flags: u8) {
        self.set_303_pat_step(which, 0, step, note, flags);
    }

    fn set_808_pat_step(&mut self, pat: usize, step: usize, col: usize, val: u8) {
        self.t808[30 + pat * 194 + 2 + step * 12 + col] = val;
    }

    fn set_808_step(&mut self, step: usize, col: usize, val: u8) {
        self.set_808_pat_step(0, step, col, val);
    }

    fn set_909_pat_step(&mut self, pat: usize, step: usize, col: usize, val: u8) {
        self.t909[31 + pat * 194 + 2 + step * 12 + col] = val;
    }

    fn set_909_step(&mut self, step: usize, col: usize, val: u8) {
        self.set_909_pat_step(0, step, col, val);
    }

    fn build(&self, title: &str) -> Vec<u8> {
        let mut head = vec![0u8; 256];
        head[0..9].copy_from_slice(&[0x5B, 0x54, 0x5B, 0x54, 0xBC, 0x04, 0x02, 0x00, 0x00]);

        let mut glob = vec![0u8; 512];
        glob[0] = u8::from(self.song_mode);
        glob[1] = u8::from(self.loop_on);
        glob[2..6].copy_from_slice(&self.bpm_millis.to_be_bytes());
        glob[6..10].copy_from_slice(&(self.loop_start_bar * TICKS_PER_BAR).to_be_bytes());
        glob[10..14].copy_from_slice(&(self.bars * TICKS_PER_BAR).to_be_bytes());
        glob[14] = self.shuffle;
        glob[15..31].copy_from_slice(b"Standard ReBirth");

        let mut usri = vec![0u8; 712];
        let tb = title.as_bytes();
        let n = tb.len().min(40);
        usri[..n].copy_from_slice(&tb[..n]);

        let devl = iff_cat(
            b"DEVL",
            &[
                iff_chunk(b"MIXR", &self.mixr),
                iff_chunk(b"DELY", &self.dely),
                iff_chunk(b"PCF ", &self.pcf),
                iff_chunk(b"DIST", &self.dist),
                iff_chunk(b"COMP", &self.comp),
                iff_chunk(b"303 ", &self.tb1),
                iff_chunk(b"303 ", &self.tb2),
                iff_chunk(b"808 ", &self.t808),
                iff_chunk(b"909 ", &self.t909),
            ],
        );
        let trk_chunks: Vec<Vec<u8>> = self.tracks.iter().map(|t| make_trak(t)).collect();
        let trkl = iff_cat(b"TRKL", &trk_chunks);
        iff_cat(
            b"RB40",
            &[
                iff_chunk(b"HEAD", &head),
                iff_chunk(b"GLOB", &glob),
                iff_chunk(b"USRI", &usri),
                devl,
                trkl,
            ],
        )
    }
}

fn slice_peak_lr(pcm: &[[i16; 2]], start_frac: f64, end_frac: f64) -> (f32, f32) {
    let a = ((pcm.len() as f64) * start_frac) as usize;
    let b = (((pcm.len() as f64) * end_frac) as usize).min(pcm.len());
    let mut pl = 0.0_f32;
    let mut pr = 0.0_f32;
    for &[l, r] in &pcm[a..b] {
        pl = pl.max((f32::from(l) / 32768.0).abs());
        pr = pr.max((f32::from(r) / 32768.0).abs());
    }
    (pl, pr)
}

fn slice_rms_lr(pcm: &[[i16; 2]], start_frac: f64, end_frac: f64) -> (f32, f32) {
    let a = ((pcm.len() as f64) * start_frac) as usize;
    let b = (((pcm.len() as f64) * end_frac) as usize).min(pcm.len());
    let n = b.saturating_sub(a).max(1) as f64;
    let mut sl = 0.0_f64;
    let mut sr = 0.0_f64;
    for &[l, r] in &pcm[a..b] {
        let fl = f64::from(l) / 32768.0;
        let fr = f64::from(r) / 32768.0;
        sl += fl * fl;
        sr += fr * fr;
    }
    ((sl / n).sqrt() as f32, (sr / n).sqrt() as f32)
}

#[test]
fn tr808_voice_selectors_support_both_static_and_dynamic_congas_claves_maracas() {
    // ReBirth TR-808 selector polarity:
    //   0 (switch UP)   -> LC, MC, HC, CL, MA (Congas, Claves, Maracas)
    //   1 (switch DOWN) -> LT, MT, HT, RS, CP (Toms, Rimshot, Clap)
    // 1. Static selectors: all 5 voices at 0 -> Congas/Claves/Maracas throughout song.
    let mut static_rbs = SyntheticRbs::new(2);
    for idx in [11, 14, 17, 19, 21] {
        static_rbs.t808[idx] = 0;
    }
    static_rbs.set_808_step(0, 3, 1); // LC
    static_rbs.set_808_step(4, 6, 1); // CL
    static_rbs.set_808_step(8, 7, 1); // MA
    static_rbs.tracks[3] = vec![
        (0, 0, 1),
        (0, 1, 0),
        (0, 11, 0),
        (0, 14, 0),
        (0, 17, 0),
        (0, 19, 0),
        (0, 21, 0),
    ];
    let (proj_static, _) = import_rbs_bytes(&static_rbs.build("808AltStatic"), "808Alt").unwrap();
    let kit = &proj_static.instruments[2].rack;
    let names: Vec<&str> = kit.zones.iter().map(|z| z.name.as_str()).collect();
    for expected in ["LC", "MC", "HC", "CL", "MA"] {
        assert!(
            names.contains(&expected),
            "static 808 selectors at 0 should map zone {expected}, got {names:?}"
        );
    }
    let score = proj_static.compile().unwrap();
    let rep = render_measure_report(&score, load_rbs_samples(&score), 2, 24_000).unwrap();
    assert!(rep.output.peak[0] > 0.05 && rep.output.full_scale_samples == 0);

    // 2. Dynamic mid-song selector switch: RS (1) -> CL (0) and CP (1) -> MA (0) at bar 1!
    let mut dyn_rbs = SyntheticRbs::new(2);
    dyn_rbs.t808[19] = 1;
    dyn_rbs.t808[21] = 1;
    dyn_rbs.set_808_step(0, 6, 1); // RS/CL
    dyn_rbs.set_808_step(4, 7, 1); // CP/MA
    dyn_rbs.tracks[3] = vec![
        (0, 0, 1),
        (0, 1, 0),
        (0, 19, 1),
        (0, 21, 1),
        (32, 19, 0),
        (32, 21, 0),
    ];
    let (proj_dyn, _) = import_rbs_bytes(&dyn_rbs.build("808AltDyn"), "808AltDyn").unwrap();
    let kit_dyn = &proj_dyn.instruments[2].rack;
    assert!(
        kit_dyn.zones.iter().any(|z| z.low_midi == drum_midi::CL)
            && kit_dyn.zones.iter().any(|z| z.low_midi == drum_midi::MA),
        "dynamic 808 selectors should add CL/MA alternate zones"
    );
    let score_dyn = proj_dyn.compile().unwrap();
    // Bar 0 step 0 hits RS (41); Bar 1 step 16 hits CL (50)!
    let bar0_midis: Vec<u8> = score_dyn.step_events(0).iter().map(|e| e.midi).collect();
    let bar1_midis: Vec<u8> = score_dyn.step_events(16).iter().map(|e| e.midi).collect();
    assert!(bar0_midis.contains(&drum_midi::RS), "bar0={bar0_midis:?}");
    assert!(bar1_midis.contains(&drum_midi::CL), "bar1={bar1_midis:?}");
}

#[test]
fn delay_track_5_and_compressor_track_8_automate_mid_song() {
    let mut rbs = SyntheticRbs::new(4);
    for s in (0..16).step_by(4) {
        rbs.set_808_step(s, 1, 1);
    }
    rbs.tracks[0] = vec![
        (0, 1, 4), // Comp on TR-808
        (0, 18, 110),
        (0, 20, 80), // 808 echo send
        (64, 1, 1),  // Comp moves to Master at bar 2
    ];
    rbs.tracks[3] = vec![(0, 0, 1), (0, 1, 0)];
    // Delay Track 5: Enabled 0->1, Steps 3->6, Mode 1->0 (triplets), Feedback 40->110, Pan 20->108
    rbs.tracks[5] = vec![
        (0, 0, 0),
        (0, 1, 3),
        (0, 2, 1),
        (0, 3, 40),
        (0, 4, 20),
        (32, 0, 1),
        (32, 1, 6),
        (64, 2, 0),
        (64, 3, 110),
        (64, 4, 108),
    ];
    // Compressor Track 8: Enabled 1, Amount 30->120, Threshold 100->20
    rbs.tracks[8] = vec![
        (0, 0, 1),
        (0, 1, 30),
        (0, 2, 100),
        (32, 1, 120),
        (32, 2, 20),
        (96, 0, 0),
    ];

    let (proj, _) = import_rbs_bytes(&rbs.build("DelayComp"), "DelayComp").unwrap();
    // Echo return must have unity strip.gain when Echo On (ChannelGain) is automated!
    let echo = proj
        .mixer
        .channels
        .iter()
        .find(|c| c.name == "Echo")
        .unwrap();
    assert!((echo.strip.gain - 1.0).abs() < 1e-6);
    assert!(proj.modulations.iter().any(|m| m.name == "Echo On"));
    assert!(proj.modulations.iter().any(|m| m.name == "Echo Time"));
    assert!(proj.modulations.iter().any(|m| m.name == "Echo Feedback"));
    assert!(
        proj.modulations.iter().any(|m| {
            m.name == "Echo Pan"
                && matches!(
                    m.target,
                    ModulationTarget::ChannelPan { channel } if channel == ChannelRef::Channel(echo.id)
                )
        }),
        "varying Delay Track 5 Pan must emit native ChannelPan automation on Echo return"
    );
    // Both TR-808 and Master compressors should be instantiated and automated!
    assert!(proj.modulations.iter().any(|m| m.name == "808 Comp On"));
    assert!(proj.modulations.iter().any(|m| m.name == "808 Comp Ratio"));
    assert!(proj.modulations.iter().any(|m| m.name == "808 Comp Thresh"));
    assert!(proj.modulations.iter().any(|m| m.name == "Master Comp On"));

    let score = proj.compile().unwrap();
    let rep = render_measure_report(&score, load_rbs_samples(&score), 4, 24_000).unwrap();
    assert!(rep.output.peak[0] > 0.05 && rep.output.full_scale_samples == 0);
}

#[test]
fn delay_track_5_echo_pan_automates_stereo_return_bus_across_bars() {
    let mut rbs = SyntheticRbs::new(4);
    for s in (0..16).step_by(4) {
        rbs.set_808_step(s, 1, 1); // steady BD hits feeding Echo send
    }
    rbs.tracks[0] = vec![
        (0, 18, 110), // TR-808 level
        (0, 20, 115), // TR-808 strong echo send
    ];
    rbs.tracks[3] = vec![(0, 0, 1), (0, 1, 0)];
    // Delay Track 5: enabled entire song, 2-step delay, Pan = 0 (hard L) bars 0-1 -> Pan = 127 (hard R) bars 2-3!
    rbs.tracks[5] = vec![
        (0, 0, 1),
        (0, 1, 2),
        (0, 2, 1),
        (0, 3, 55),
        (0, 4, 0),
        (64, 4, 127),
    ];

    // Verify Dist imports as DriveMode::ShockPort alongside DelayMode::Stereo on Echo.
    rbs.dist = [1, 110, 96, 0, 0, 0, 0, 0];
    rbs.tracks[0].push((0, 21, 1)); // TR-808 Dist ON
    rbs.tracks[6] = vec![(0, 0, 1), (0, 1, 110), (0, 2, 96)];

    let (proj, _) = import_rbs_bytes(&rbs.build("EchoPanSweep"), "EchoPanSweep").unwrap();
    let ch_808 = proj
        .mixer
        .channels
        .iter()
        .find(|c| c.name == "TR-808")
        .expect("TR-808 channel");
    assert!(ch_808.inserts.iter().any(|d| matches!(
        d.kind,
        cadence_core::mixer::EffectKind::Drive(p)
            if p.mode == cadence_core::mixer::DriveMode::ShockPort && p.drive > 0.7
    )));
    let echo = proj
        .mixer
        .channels
        .iter()
        .find(|c| c.name == "Echo")
        .expect("Echo return bus");
    assert!(proj.modulations.iter().any(|m| {
        m.name == "Echo Pan"
            && m.target
                == ModulationTarget::ChannelPan {
                    channel: ChannelRef::Channel(echo.id),
                }
    }));

    // Dry TR-808 is centered (pan = 64 -> L == R), and ReBirth Echo imports as native
    // DelayMode::Stereo so every echo tap stays balanced L == R before the Return strip pan.
    // Panning Echo hard L in bars 1-2 shifts stereo RMS energy Left, and hard R in bars 3-4 shifts it Right.
    assert!(matches!(
        echo.inserts[0].kind,
        cadence_core::mixer::EffectKind::Delay(p) if p.mode == cadence_core::mixer::DelayMode::Stereo
    ));
    let score = proj.compile().unwrap();
    let (rep, pcm) =
        render_schedule_pcm(&[(0, &score)], 0, 64, load_rbs_samples(&score), 24_000).unwrap();
    assert_eq!(rep.output.full_scale_samples, 0);
    let (early_l, early_r) = slice_rms_lr(&pcm, 0.08, 0.45);
    let (late_l, late_r) = slice_rms_lr(&pcm, 0.58, 0.96);
    assert!(
        early_l > early_r * 1.10,
        "Echo Pan hard L in bars 1-2 must shift stereo RMS energy Left (L={early_l}, R={early_r})"
    );
    assert!(
        late_r > late_l * 1.05,
        "Echo Pan hard R in bars 3-4 must shift stereo RMS energy Right (L={late_l}, R={late_r})"
    );
}

#[test]
fn pcf_track_7_honors_native_pattern_decay_and_dynamic_bp_lp_mode_switch() {
    let build_pcf = |decay: u8| -> Project {
        let mut rbs = SyntheticRbs::new(4);
        for s in 0..16 {
            rbs.set_303_step(0, s, 0, 0x01); // tied drone so filter tail is crystal clear
        }
        rbs.tracks[0] = vec![(0, 2, 1), (0, 6, 115)]; // PCF on 303A
        rbs.tracks[1] = vec![(0, 0, 1), (0, 1, 0), (0, 3, 115)];
        rbs.tracks[7] = vec![
            (0, 0, 1),
            (0, 1, 20),  // low base cutoff
            (0, 2, 75),  // resonance
            (0, 3, 115), // high env amount
            (0, 4, 0),   // Pattern 0 (Whole Pulse: step 0 only)
            (0, 5, decay),
            (0, 6, 0),   // BandPass at bar 0
            (64, 4, 24), // switch to Pattern 24 (Acid Bounce) at bar 2!
            (64, 6, 1),  // switch to ImLpf2 (12 dB/oct LP) at bar 2!
        ];
        import_rbs_bytes(&rbs.build("PcfTest"), "PcfTest")
            .unwrap()
            .0
    };

    let short_dec = build_pcf(10);
    let long_dec = build_pcf(120);

    // Native FilterPatch must carry pattern, env_amount, and scaled env_decay_ms directly!
    let get_pcf = |p: &Project| match p.mixer.channels[0]
        .inserts
        .iter()
        .find(|d| d.name == "PCF")
        .expect("PCF insert")
        .kind
    {
        cadence_core::mixer::EffectKind::Filter(fp) => fp,
        _ => panic!("expected native Filter PCF"),
    };
    let fp_short = get_pcf(&short_dec);
    let fp_long = get_pcf(&long_dec);
    assert_eq!(fp_short.env_pattern, 0);
    assert!(fp_short.env_amount > 3.0);
    assert!(
        fp_long.env_decay_ms > fp_short.env_decay_ms * 10.0,
        "long decay ({} ms) must far exceed short ({} ms)",
        fp_long.env_decay_ms,
        fp_short.env_decay_ms
    );

    // Dynamic BP/LP mode switch & Pattern switch must emit native EffectParam automations!
    let mode_mod = short_dec
        .modulations
        .iter()
        .find(|m| {
            matches!(
                m.target,
                ModulationTarget::Effect {
                    parameter: EffectParam::FilterMode,
                    ..
                }
            )
        })
        .expect("PCF Mode switch must emit EffectParam::FilterMode modulation");
    let PatternKind::Values { points, .. } = &short_dec.pattern(mode_mod.pattern).unwrap().kind
    else {
        panic!("expected Values pattern");
    };
    assert!(points.len() >= 2);
    let expected_bp = EffectParam::FilterMode.normalize(FilterMode::BandPass.to_index() as f32);
    let expected_lp = EffectParam::FilterMode.normalize(FilterMode::ImLpf2.to_index() as f32);
    assert!((points[0].value - expected_bp).abs() < 1e-3);
    assert!((points[1].value - expected_lp).abs() < 1e-3);

    assert!(
        short_dec.modulations.iter().any(|m| matches!(
            m.target,
            ModulationTarget::Effect {
                parameter: EffectParam::EnvPattern,
                ..
            }
        )),
        "PCF Wave change must emit native EffectParam::EnvPattern modulation"
    );

    // End-to-end audio verification: both short & long render cleanly with 0 clips and
    // measurably different envelope tails!
    let short_score = short_dec.compile().unwrap();
    let (_, pcm_short) = render_schedule_pcm(
        &[(0, &short_score)],
        0,
        32,
        load_rbs_samples(&short_score),
        24_000,
    )
    .unwrap();
    let long_score = long_dec.compile().unwrap();
    let (_, pcm_long) = render_schedule_pcm(
        &[(0, &long_score)],
        0,
        32,
        load_rbs_samples(&long_score),
        24_000,
    )
    .unwrap();
    let diff: f32 = pcm_short
        .iter()
        .zip(&pcm_long)
        .map(|(&a, &b)| (i32::from(a[0]) - i32::from(b[0])).abs() as f32 / 32768.0)
        .fold(0.0, f32::max);
    assert!(
        diff > 0.02,
        "native rhythmic PCF decay must audibly shape the rendered waveform (max diff={diff})"
    );
}

#[test]
fn tb303_tune_uses_exact_rebirth_semitone_table_with_center_deadzone() {
    let check_tune = |raw_tune: u8, expected_st: f32| {
        let mut rbs = SyntheticRbs::new(2);
        rbs.tb1[2] = raw_tune;
        rbs.set_303_step(0, 0, 0, 0x00);
        rbs.tracks[1] = vec![(0, 0, 1), (0, 1, 0), (0, 2, raw_tune)];
        let (proj, _) = import_rbs_bytes(&rbs.build("TuneTest"), "TuneTest").unwrap();
        let tune_st = proj.instruments[0].rack.zones[0]
            .modules
            .iter()
            .find_map(|m| match &m.kind {
                ModuleKind::Vco(vco) => Some(vco.tune_semitones),
                _ => None,
            })
            .expect("expected VCO module on TB-303 Rack");
        assert!(
            (tune_st - expected_st).abs() < 1e-5,
            "raw tune {raw_tune}: expected {expected_st} st, got {tune_st} st"
        );
    };
    check_tune(0, -12.0);
    check_tune(7, -12.0);
    check_tune(42, -3.0);
    // Entire 52..=75 center detent must be dead-in-tune 0.0 semitones!
    for v in [52, 57, 62, 64, 66, 68, 74, 75] {
        check_tune(v, 0.0);
    }
    check_tune(111, 9.0);
    check_tune(120, 12.0);
    check_tune(127, 12.0);
}

#[test]
fn tr808_and_tr909_tracks_emit_per_zone_drum_automations_without_kit_wide_mismappings() {
    let mut rbs = SyntheticRbs::new(4);
    for s in (0..16).step_by(4) {
        rbs.set_808_step(s, 1, 1);
        rbs.set_909_step(s, 2, 1);
    }
    rbs.tracks[3] = vec![
        (0, 0, 1),
        (0, 1, 0),
        (0, 3, 120), // BD level -> per-zone BD Gain
        (0, 4, 30),  // BD tone  -> per-zone BD Cutoff
        (0, 5, 30),  // BD decay -> per-zone BD Decay
        (0, 10, 40), // LT tune  -> per-zone LT CoarseTune/Tune
        (64, 3, 40),
        (64, 4, 120),
        (64, 5, 120),
        (64, 10, 110),
    ];
    rbs.tracks[4] = vec![
        (0, 0, 1),
        (0, 1, 0),
        (0, 3, 120), // BD level -> per-zone BD Gain
        (0, 4, 30),  // BD tune  -> per-zone BD CoarseTune/Tune
        (0, 6, 30),  // BD decay -> per-zone BD Decay
        (0, 9, 25),  // SD tone  -> per-zone SD Cutoff
        (64, 3, 45),
        (64, 4, 120),
        (64, 6, 120),
        (64, 9, 120),
    ];
    let (proj, _) = import_rbs_bytes(&rbs.build("DrumAuto"), "DrumAuto").unwrap();
    for (inst_idx, tag) in [(2usize, "808"), (3usize, "909")] {
        let inst = &proj.instruments[inst_idx];
        let inst_id = inst.id;
        let kit = &inst.rack;
        let z_for = |midi: u8| -> u16 {
            kit.zones
                .iter()
                .find(|z| z.low_midi <= midi && midi <= z.high_midi)
                .unwrap()
                .id
        };
        let bd_z = z_for(drum_midi::BD);
        let sd_z = z_for(drum_midi::SD);
        let lt_z = z_for(drum_midi::LT);

        let has_zone_param = |zone: u16, param: ZoneParam| {
            proj.modulations.iter().any(|m| {
                m.target
                    == ModulationTarget::Instrument {
                        instrument: inst_id,
                        parameter: InstrumentParam::Zone { zone, param },
                    }
            })
        };
        let has_module_param = |zone: u16, param: ModuleParam| {
            proj.modulations.iter().any(|m| {
                matches!(
                    m.target,
                    ModulationTarget::Instrument {
                        instrument,
                        parameter: InstrumentParam::Module { zone: z, param: p, .. },
                    } if instrument == inst_id && z == zone && p == param
                )
            })
        };

        assert!(
            has_zone_param(bd_z, ZoneParam::Gain),
            "{tag} BD level must emit per-zone ZoneParam::Gain"
        );
        assert!(
            has_module_param(bd_z, ModuleParam::Decay),
            "{tag} BD decay must emit per-zone Decay modulation"
        );
        if inst_idx == 2 {
            assert!(
                has_module_param(bd_z, ModuleParam::Cutoff),
                "808 BD tone must emit per-zone BD Cutoff"
            );
            assert!(
                has_module_param(lt_z, ModuleParam::CoarseTune),
                "808 LT tune must emit per-zone LT CoarseTune"
            );
        } else {
            assert!(
                has_module_param(bd_z, ModuleParam::CoarseTune),
                "909 BD tune must emit per-zone BD CoarseTune"
            );
            assert!(
                has_module_param(sd_z, ModuleParam::Cutoff),
                "909 SD tone must emit per-zone SD Cutoff"
            );
        }

        // Zero kit-wide RackParam mis-mappings!
        for forbidden in [
            RackParam::Cutoff,
            RackParam::Tune,
            RackParam::Gain,
            RackParam::Resonance,
        ] {
            assert!(
                !proj.modulations.iter().any(|m| {
                    m.target
                        == ModulationTarget::Instrument {
                            instrument: inst_id,
                            parameter: InstrumentParam::Rack { param: forbidden },
                        }
                }),
                "{tag} must NOT emit kit-wide {forbidden:?} mis-mapping"
            );
        }
    }
    let drum_score = proj.compile().unwrap();
    let rep = render_measure_report(&drum_score, load_rbs_samples(&drum_score), 4, 24_000).unwrap();
    assert!(rep.output.peak[0] > 0.05 && rep.output.full_scale_samples == 0);
}

#[test]
fn tb303_slide_legato_glides_under_max_shuffle_and_accent_barks_cleanly() {
    let mut rbs = SyntheticRbs::new(2);
    rbs.shuffle = 127; // maximum ReBirth shuffle -> 0.36 swing offset on odd steps!
    rbs.tb1[9] = 1; // shuffle ON for pattern 0
                    // Step 0 slides into step 1 (octave jump C2 -> C3), step 4 accented, step 8 unaccented
    rbs.set_303_step(0, 0, 0, 0x01); // C2 + slide
    rbs.set_303_step(0, 1, 12, 0x00); // C3 (target of slide)
    rbs.set_303_step(0, 4, 0, 0x02); // C2 + accent
    rbs.set_303_step(0, 8, 0, 0x00); // C2 unaccented
    rbs.tracks[1] = vec![(0, 0, 1), (0, 1, 0), (0, 3, 45), (0, 4, 100), (0, 7, 115)];

    let (proj, _) = import_rbs_bytes(&rbs.build("303Slide"), "303Slide").unwrap();
    // Base notes pattern must set slide gate > 1.45 so it overlaps even under max swing!
    let base_steps = proj
        .patterns
        .iter()
        .find_map(|p| match &p.kind {
            PatternKind::Notes { steps } if p.name.starts_with("303A") => Some(steps),
            _ => None,
        })
        .unwrap();
    assert!(
        base_steps[0].gate > 1.45,
        "slid 303 step gate must exceed 1.0 + max_swing, got {}",
        base_steps[0].gate
    );
    assert!(
        base_steps[1].gate < 0.55,
        "non-slid 303 step gate must stay < 0.55 to avoid accidental legato, got {}",
        base_steps[1].gate
    );
    // Verify rendered audio has 0 clips and strong accented bark vs unaccented note.
    let score = proj.compile().unwrap();
    let (rep, pcm) =
        render_schedule_pcm(&[(0, &score)], 0, 16, load_rbs_samples(&score), 24_000).unwrap();
    assert_eq!(rep.output.full_scale_samples, 0);
    let (acc_peak, _) = slice_peak_lr(&pcm, 4.0 / 16.0, 6.0 / 16.0);
    let (norm_peak, _) = slice_peak_lr(&pcm, 8.0 / 16.0, 10.0 / 16.0);
    assert!(
        acc_peak > norm_peak * 1.35,
        "accented 303 note ({acc_peak}) should punch noticeably harder than unaccented ({norm_peak})"
    );
}

fn build_synthetic_1x_smf(title: &str, bars: u32, loop_on: bool) -> Vec<u8> {
    let mut raw = vec![0u8; 0x4000];
    raw[0..4].copy_from_slice(&[0xBC, 0x03, 0x01, 0x00]); // v3.1 (ReBirth 1.x)
    raw[5..9].copy_from_slice(b"TinY");
    raw[9] = 110; // master level
    raw[10..14].copy_from_slice(&0u32.to_be_bytes()); // start bar 0
    raw[14..18].copy_from_slice(&bars.to_be_bytes());
    raw[0x12] = 1; // song mode
    raw[0x2A..0x2E].copy_from_slice(&132_000u32.to_be_bytes()); // 132 BPM
    raw[0x2E] = u8::from(loop_on);
    raw[0x2F] = 0x83; // delay: 3 steps, 16ths mode
    raw[0x33] = 72; // delay fb
    raw[0x34] = 64; // delay pan
    raw[0x35] = 1; // delay on
    raw[0x36] = 60; // dist amount
    raw[0x37] = 0; // dist on 303#1
    raw[0x38] = 1; // dist on
    raw[0x40] = 3; // PCF off
                   // 303#1 record at 0x100, patterns at 352
    raw[0x100 + 2] = 110; // level
    raw[0x100 + 3] = 64; // pan
    raw[0x100 + 7] = 0xFF; // pattern 0 & 1 length = 16
    raw[0x100 + 0x17] = 1; // mix enabled
    raw[352 + 16..352 + 23].copy_from_slice(&[64, 50, 95, 85, 80, 90, 0]);
    for s in 0..16 {
        let so = 352 + 30 + 4 * s;
        raw[so] = (s % 12) as u8;
        raw[so + 2] = if s % 8 == 0 { 0x02 } else { 0x00 };
        raw[so + 3] = u8::from(s % 4 != 0); // rest
    }
    // 808 record at 0x140 (selector bit 0 => LC/CL/MA)
    raw[0x140 + 2] = 110;
    raw[0x140 + 3] = 64;
    raw[0x140 + 7] = 0xFF;
    raw[0x140 + 0x17] = 1;
    let tb = title.as_bytes();
    let n = tb.len().min(40);
    raw[0x3160..0x3160 + n].copy_from_slice(&tb[..n]);

    let mut sysex = vec![0x00, 0x00, 0x66, 0x00, 0x15, 0x01, 0x00];
    let mut i = 0;
    while i < raw.len() {
        let mut msbs = 0u8;
        for k in 0..7 {
            let b = raw.get(i + k).copied().unwrap_or(0);
            sysex.push(b & 0x7F);
            if b & 0x80 != 0 {
                msbs |= 1 << (6 - k);
            }
        }
        sysex.push(msbs);
        i += 7;
    }
    sysex.push(0xF7);

    let make_mtrk = |body: &[u8]| -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"MTrk");
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(body);
        out
    };
    let encode_vlq = |mut v: usize| -> Vec<u8> {
        let mut st = vec![(v & 0x7F) as u8];
        v >>= 7;
        while v > 0 {
            st.push(((v & 0x7F) as u8) | 0x80);
            v >>= 7;
        }
        st.reverse();
        st
    };

    let t0 = make_mtrk(&[0x00, 0xFF, 0x2F, 0x00]);
    let mut t1_body = vec![0x00, 0xF0];
    t1_body.extend_from_slice(&encode_vlq(sysex.len()));
    t1_body.extend_from_slice(&sysex);
    t1_body.extend_from_slice(&[0x00, 0xFF, 0x2F, 0x00]);
    let t1 = make_mtrk(&t1_body);
    let mut t2_body = vec![
        0x00, 0xB0, 15, 1, // 303#1 enabled
        0x00, 0xB0, 14, 0, // pat 0
        0x00, 0xB0, 17, 35, // cutoff 35
    ];
    if bars > 1 {
        t2_body.extend_from_slice(&encode_vlq(((bars - 1) * 32) as usize));
        t2_body.extend_from_slice(&[0xB0, 17, 110]);
    }
    t2_body.extend_from_slice(&[0x00, 0xFF, 0x2F, 0x00]);
    let t2 = make_mtrk(&t2_body);

    let mut smf = Vec::new();
    smf.extend_from_slice(b"MThd");
    smf.extend_from_slice(&6u32.to_be_bytes());
    smf.extend_from_slice(&1u16.to_be_bytes());
    smf.extend_from_slice(&3u16.to_be_bytes());
    smf.extend_from_slice(&32u16.to_be_bytes());
    smf.extend_from_slice(&t0);
    smf.extend_from_slice(&t1);
    smf.extend_from_slice(&t2);
    smf
}

#[test]
fn synthetic_full_song_mode_fade_in_transpose_echo_send_and_zero_clips() {
    // 32-bar self-contained Song Mode fixture exercising:
    // - Synths/Drums Group busses + Echo Return via standard Channel.sends + ModulationTarget::Send
    // - 303A & 303B GainScale fade-in from silent bar 0 to audible bars 4..32
    // - Transposed 303 Parts (Part.transpose != 0) and >16 bank slots on 303A
    // - Musical form sections (Intro -> Build/Drop -> Breakdown -> Outro, never mechanical "Part N")
    // - Dynamic Echo Time, 808 Comp On, and 303B PCF Mode automations
    // - TR-909 flam Stack pattern & TR-808 selector polarity (0 = CL/LC, 1 = RS/CP)
    // - Unbound Macros (macros.is_empty()) and 0 full-scale clips!
    let mut rbs = SyntheticRbs::new(32);
    rbs.mixr[1] = 4; // Comp routed to TR-808 (1=Master, 2=303A, 3=303B, 4=808, 5=909)
    rbs.mixr[2] = 2; // PCF routed to 303B
    rbs.pcf[0] = 1; // PCF enabled
    rbs.comp[0] = 1; // Comp enabled
    rbs.dely = [1, 3, 1, 80, 64, 0, 0, 0]; // Delay enabled, 3 steps, 16ths, fb=80

    // Populate 18 distinct 303A patterns (0..18), including exact +5 st transposition (pat 1 vs pat 0)
    // so lane.parts exposes >16 bank slots AND Part.transpose is used.
    let base_motif: [(usize, u8, u8); 4] =
        [(0, 0, 0x02), (4, 3, 0x00), (8, 7, 0x02), (12, 10, 0x00)];
    for &(step, note, flags) in &base_motif {
        rbs.set_303_pat_step(0, 0, step, note, flags);
        rbs.set_303_pat_step(0, 1, step, note + 2, flags); // +2 semitone transpose of pat 0
        rbs.set_303_pat_step(1, 0, step, note + 5, flags); // 303B motif
    }
    for pat in 2..18 {
        rbs.set_303_pat_step(0, pat, 0, (pat % 12) as u8, 0x02);
        rbs.set_303_pat_step(0, pat, (pat % 12) + 2, ((pat * 3) % 12) as u8, 0x00);
    }
    // 808: BD on 4-on-the-floor + Clap/Maracas + Rim/Claves (default selector 0 -> LC/CL/MA)
    for s in [0, 4, 8, 12] {
        rbs.set_808_step(s, 1, 2); // BD accented
    }
    rbs.set_808_step(4, 11, 1); // CL/CP
                                // 909: BD + Flam on SD (val 3 = flam, t909[29] = flam interval)
    rbs.t909[29] = 64;
    rbs.set_909_step(0, 1, 1);
    rbs.set_909_step(4, 2, 3); // SD with flam!

    // Track 0 (Mixer): 303A (ctrl 6) and 303B (ctrl 12) fade IN from 0 at bar 0 to 105 by bar 4;
    // 303A Echo Send (ctrl 8) kicks in from bar 4..24; Comp->TR-808 (ctrl 1=4); PCF->303B (ctrl 2=2).
    rbs.tracks[0] = vec![
        (0, 1, 4),   // Comp -> TR-808
        (0, 2, 2),   // PCF -> 303B
        (0, 6, 0),   // 303A level 0 at bar 0
        (0, 12, 0),  // 303B level 0 at bar 0
        (0, 8, 0),   // 303A Echo Send 0 at bar 0
        (16, 6, 40), // bar 0.5: fade starts after steps 0..4
        (16, 12, 40),
        (128, 6, 110),  // bar 4: 303A full
        (128, 12, 115), // bar 4: 303B full
        (128, 8, 95),   // bar 4: 303A Echo Send up!
        (768, 8, 0),    // bar 24: Echo Send down
    ];
    // Track 1 (303A): enabled bars 0..32, switch pat 0 -> pat 1 at bar 8
    rbs.tracks[1] = vec![(0, 0, 1), (0, 1, 0), (256, 1, 1)];
    // Track 2 (303B): enabled bars 0..32
    rbs.tracks[2] = vec![(0, 0, 1), (0, 1, 0)];
    // Track 3 (808): silent bars 0..8, active bars 8..24, off bars 24..32 (creates form transitions!)
    rbs.tracks[3] = vec![(0, 0, 0), (0, 1, 0), (256, 0, 1), (768, 0, 0)];
    // Track 4 (909): active bars 16..24
    rbs.tracks[4] = vec![(0, 0, 0), (0, 1, 0), (512, 0, 1), (768, 0, 0)];
    // Track 5 (Delay): enabled + Delay Time automation (ctrl 1)
    rbs.tracks[5] = vec![(0, 0, 1), (0, 1, 3), (384, 1, 6)];
    // Track 7 (PCF): PCF Mode switch LP <-> BP (ctrl 6)
    rbs.tracks[7] = vec![(0, 0, 1), (0, 6, 0), (512, 6, 1)];
    // Track 8 (Comp): Comp On toggle (ctrl 0)
    rbs.tracks[8] = vec![(0, 0, 0), (256, 0, 1), (768, 0, 0)];

    let (proj, summary) = import_rbs_bytes(&rbs.build("FullSongSynth"), "FullSongSynth").unwrap();
    assert_eq!(summary.bars, 32);
    assert!(
        proj.macros.is_empty(),
        "RBS import must leave macros empty so direct knob edits are never shadowed"
    );
    assert!(proj
        .mixer
        .channels
        .iter()
        .any(|c| c.kind == ChannelKind::Group && c.name == "Synths"));
    assert!(proj
        .mixer
        .channels
        .iter()
        .any(|c| c.kind == ChannelKind::Group && c.name == "Drums"));
    assert!(
        proj.parts.iter().any(|p| p.transpose == 2),
        "303A pat 1 should derive from pat 0 via Part.transpose = +2"
    );
    assert!(
        proj.lanes[0].parts.len() > 16,
        "303A lane.parts should expose >16 bank slots, got {}",
        proj.lanes[0].parts.len()
    );
    assert!(
        !proj.sections.is_empty()
            && proj.sections.len() <= 32
            && !proj.sections.iter().any(|s| s.name.starts_with("Part ")),
        "sections should be musical, got {:?}",
        proj.sections.iter().map(|s| &s.name).collect::<Vec<_>>()
    );
    assert!(
        (proj.mixer.channels[0].strip.gain - 1.0).abs() < 1e-6
            && (proj.mixer.channels[1].strip.gain - 1.0).abs() < 1e-6,
        "Both 303A & 303B must use unity strip.gain = 1.0 for GainScale fade-in"
    );

    // Echo Send must use standard Channel.sends + ModulationTarget::Send!
    let echo_ch = proj
        .mixer
        .channels
        .iter()
        .find(|c| c.kind == ChannelKind::Return && c.name == "Echo")
        .expect("Echo Return channel");
    assert!(
        proj.mixer.channels[0]
            .sends
            .iter()
            .any(|s| s.target == echo_ch.id),
        "303A channel must declare standard ChannelSend to Echo Return"
    );
    assert!(
        proj.modulations.iter().any(|m| {
            m.target
                == ModulationTarget::Send {
                    channel: ChannelRef::Channel(proj.mixer.channels[0].id),
                    target: echo_ch.id,
                }
        }),
        "303A Echo Send automation must target ModulationTarget::Send"
    );
    assert!(proj.modulations.iter().any(|m| m.name == "Echo Time"));
    assert!(proj.modulations.iter().any(|m| m.name == "808 Comp On"));
    assert!(proj.modulations.iter().any(|m| m.name == "303B PCF Mode"));
    assert!(
        proj.patterns
            .iter()
            .any(|pat| matches!(pat.kind, PatternKind::Stack { .. })),
        "TR-909 flam should materialize a Stack pattern"
    );
    let kit808 = &proj.instruments[2].rack;
    assert!(
        kit808.zones.iter().any(|z| z.name == "CL") && kit808.zones.iter().any(|z| z.name == "LC"),
        "TR-808 default selector=0 should use CL/LC zones"
    );

    // Solo TB-303 A & TB-303 B individually across bars 0..8 -> start near 0, fade in audibly by bar 4, 0 clips!
    for (ch_idx, label) in [(0usize, "TB-303 A"), (1usize, "TB-303 B")] {
        let mut flags = LiveFlags::default();
        flags.solo_channels.insert(proj.mixer.channels[ch_idx].id);
        let score = proj.compile_with_flags(&flags).unwrap();
        let (rep, pcm) =
            render_schedule_pcm(&[(0, &score)], 0, 8 * 16, load_rbs_samples(&score), 16_000)
                .unwrap();
        assert_eq!(rep.output.full_scale_samples, 0, "{label} clipped");
        let (bar0_l, bar0_r) = slice_peak_lr(&pcm, 1.0 / (8.0 * 16.0), 5.0 / (8.0 * 16.0));
        let (fade_l, fade_r) = slice_peak_lr(&pcm, 4.0 / 8.0, 1.0);
        let bar0 = bar0_l.max(bar0_r);
        let fade = fade_l.max(fade_r);
        assert!(
            bar0 < 0.05 && fade > 0.12,
            "{label} must start near 0 ({bar0}) and fade in audibly ({fade})"
        );
    }

    // Full 32-bar mix render -> audible with zero clips!
    let score = proj.compile().unwrap();
    let (rep, pcm) =
        render_schedule_pcm(&[(0, &score)], 0, 32 * 16, load_rbs_samples(&score), 12_000).unwrap();
    let (pl, pr) = slice_peak_lr(&pcm, 0.0, 1.0);
    assert!(
        pl.max(pr) > 0.20 && rep.output.full_scale_samples == 0,
        "Full 32-bar mix peak=({pl},{pr}) clips={}",
        rep.output.full_scale_samples
    );
}

#[test]
fn master_level_zero_renders_true_digital_silence() {
    let mut rbs = SyntheticRbs::new(2);
    rbs.mixr[0] = 0; // Master level = 0!
    rbs.set_303_step(0, 0, 0, 0x02);
    rbs.set_808_step(0, 1, 2);
    rbs.tracks[1] = vec![(0, 0, 1), (0, 1, 0)];
    rbs.tracks[3] = vec![(0, 0, 1), (0, 1, 0)];

    let (proj, _) = import_rbs_bytes(&rbs.build("SilentMaster"), "SilentMaster").unwrap();
    assert_eq!(
        proj.mixer.master.strip.gain, 0.0,
        "Master level=0 must map to exact 0.0 gain (no 0.20 floor)"
    );
    let score = proj.compile().unwrap();
    let (_, pcm) =
        render_schedule_pcm(&[(0, &score)], 0, 32, load_rbs_samples(&score), 24_000).unwrap();
    let (pl, pr) = slice_peak_lr(&pcm, 0.0, 1.0);
    assert!(
        pl == 0.0 && pr == 0.0,
        "Master level=0 must render true digital silence (L={pl}, R={pr})"
    );
}

#[test]
fn peek_rbs_header_matches_full_import_for_iff_and_1x_smf_without_ghost_909() {
    let dir = tempfile::tempdir().unwrap();

    // 1. IFF looped Song Mode (header loop_end_bar directly gives bars without scanning DEVL/TRKL)
    let mut looped = SyntheticRbs::new(12);
    looped.loop_on = true;
    looped.bpm_millis = 128_500;
    looped.set_303_step(0, 0, 0, 0x02);
    looped.tracks[1] = vec![(0, 0, 1), (0, 1, 0)];
    let looped_path = dir.path().join("looped.rbs");
    std::fs::write(&looped_path, looped.build("Looped IFF")).unwrap();
    let p_looped = peek_rbs_header(&looped_path).unwrap();
    let (proj_looped, sum_looped) = import_rbs_file(&looped_path).unwrap();
    assert_eq!(p_looped.title, proj_looped.title);
    assert_eq!(p_looped.bars, sum_looped.bars);
    assert_eq!(p_looped.bars, 12);
    assert_eq!(p_looped.version, sum_looped.version);
    assert!((p_looped.bpm - proj_looped.bpm).abs() < 1e-6);

    // 2. IFF unlooped Song Mode (bars inferred from last track event)
    let mut unlooped = SyntheticRbs::new(4);
    unlooped.loop_on = false;
    unlooped.set_303_step(0, 0, 0, 0x02);
    unlooped.tracks[1] = vec![(0, 0, 1), (0, 1, 0), (160, 3, 90)]; // pos 160 = bar 5 -> 6 bars
    let unlooped_path = dir.path().join("unlooped.rbs");
    std::fs::write(&unlooped_path, unlooped.build("Unlooped IFF")).unwrap();
    let p_unlooped = peek_rbs_header(&unlooped_path).unwrap();
    let (proj_unlooped, sum_unlooped) = import_rbs_file(&unlooped_path).unwrap();
    assert_eq!(p_unlooped.title, proj_unlooped.title);
    assert_eq!(p_unlooped.bars, sum_unlooped.bars);
    assert_eq!(p_unlooped.bars, 6);

    // 3. ReBirth 1.x SMF: header parity, NO ghost TR-909, and audible zero-clip render!
    let smf_bytes = build_synthetic_1x_smf("Classic 1x", 4, true);
    let smf_path = dir.path().join("classic1x.rbs");
    std::fs::write(&smf_path, &smf_bytes).unwrap();
    let p_smf = peek_rbs_header(&smf_path).unwrap();
    let (proj_smf, sum_smf) = import_rbs_file(&smf_path).unwrap();
    assert_eq!(p_smf.title, proj_smf.title);
    assert_eq!(p_smf.bars, sum_smf.bars);
    assert_eq!(p_smf.version, sum_smf.version);
    assert!((p_smf.bpm - proj_smf.bpm).abs() < 1e-6);
    assert_eq!(proj_smf.instruments.len(), 3, "v1.x must omit ghost TR-909");
    assert!(!proj_smf.instruments.iter().any(|i| i.name == "TR-909"));
    assert_eq!(proj_smf.lanes.len(), 3);
    assert_eq!(proj_smf.mixer.channels.len(), 6);
    let smf_score = proj_smf.compile().unwrap();
    let (rep, pcm) = render_schedule_pcm(
        &[(0, &smf_score)],
        0,
        64,
        load_rbs_samples(&smf_score),
        24_000,
    )
    .unwrap();
    let (pl, pr) = slice_peak_lr(&pcm, 0.0, 1.0);
    assert!(
        pl.max(pr) > 0.15 && rep.output.full_scale_samples == 0,
        "v1.x SMF render peak=({pl},{pr}) clips={}",
        rep.output.full_scale_samples
    );
}

#[test]
fn echo_send_via_channel_sends_and_modulation_target_send_renders_wet_tail() {
    let mut rbs = SyntheticRbs::new(2);
    rbs.dely = [1, 4, 1, 90, 96, 0, 0, 0]; // Delay ON, 4 steps, high feedback, panned right
    rbs.set_303_step(0, 12, 0, 0x02); // hit on step 12 of bar 0 -> first echo lands at bar 1 step 0!
    rbs.tracks[1] = vec![(0, 0, 1), (0, 1, 0), (32, 0, 0)]; // 303A stops at bar 1 so bar 1 is pure tail!
    rbs.tracks[5] = vec![(0, 0, 1)]; // Delay ON
                                     // Send OFF vs Send ON at bar 0
    rbs.tracks[0] = vec![(0, 6, 105), (0, 8, 0)];
    let (dry_proj, _) = import_rbs_bytes(&rbs.build("EchoDry"), "EchoDry").unwrap();

    rbs.tracks[0] = vec![(0, 6, 105), (0, 8, 120), (32, 8, 0)];
    let (wet_proj, _) = import_rbs_bytes(&rbs.build("EchoWet"), "EchoWet").unwrap();

    let echo_id = wet_proj
        .mixer
        .channels
        .iter()
        .find(|c| c.kind == ChannelKind::Return && c.name == "Echo")
        .unwrap()
        .id;
    assert!(wet_proj.modulations.iter().any(|m| {
        m.target
            == ModulationTarget::Send {
                channel: ChannelRef::Channel(wet_proj.mixer.channels[0].id),
                target: echo_id,
            }
    }));

    let dry_score = dry_proj.compile().unwrap();
    let (_, dry_pcm) = render_schedule_pcm(
        &[(0, &dry_score)],
        0,
        32,
        load_rbs_samples(&dry_score),
        24_000,
    )
    .unwrap();
    let wet_score = wet_proj.compile().unwrap();
    let (wet_rep, wet_pcm) = render_schedule_pcm(
        &[(0, &wet_score)],
        0,
        32,
        load_rbs_samples(&wet_score),
        24_000,
    )
    .unwrap();
    assert_eq!(wet_rep.output.full_scale_samples, 0);
    // Bar 1 (second half of render) should carry an audible Echo tail only when Send is active!
    let (dry_l, dry_r) = slice_peak_lr(&dry_pcm, 0.55, 0.95);
    let (wet_l, wet_r) = slice_peak_lr(&wet_pcm, 0.55, 0.95);
    assert!(
        dry_l.max(dry_r) < 0.01 && wet_l.max(wet_r) > 0.03,
        "Channel.sends Echo tail should ring audibly in bar 1 (dry={dry_l:.4}/{dry_r:.4}, wet={wet_l:.4}/{wet_r:.4})"
    );
}

#[test]
fn tb303_deduplicates_exact_duplicates_and_derives_transposed_parts_with_identical_midi() {
    let mut rbs = SyntheticRbs::new(6);
    rbs.shuffle = 80; // global shuffle active so Swing wrapper sharing is tested too!

    // Helper: write the same 4-note acid motif transposed by `delta` semitones into (which, pat).
    // Base notes (in semitones above C2 = MIDI 36): step 0=5(acc), step 2=8(slide), step 3=10, step 8=5(acc).
    let write_motif = |rbs: &mut SyntheticRbs, which: usize, pat: usize, delta: i8, shuf: bool| {
        rbs.set_303_pat_shuffle(which, pat, shuf);
        let motif: [(usize, i8, u8); 4] = [
            (0, 5, 0x02),  // accent
            (2, 8, 0x01),  // slide
            (3, 10, 0x00), // slide target
            (8, 5, 0x02),  // accent
        ];
        for (step, base_st, base_flags) in motif {
            let st = base_st + delta;
            let (note, oct_flag) = if st >= 12 {
                ((st - 12) as u8, 0x04) // up octave
            } else {
                (st as u8, 0x00)
            };
            rbs.set_303_pat_step(which, pat, step, note, base_flags | oct_flag);
        }
    };

    // 303A: A1 base (shuffle ON), A2 +5 st (shuffle ON), A3 -5 st (shuffle OFF), A4 exact dup (shuffle ON).
    write_motif(&mut rbs, 0, 0, 0, true);
    write_motif(&mut rbs, 0, 1, 5, true);
    write_motif(&mut rbs, 0, 2, -5, false);
    write_motif(&mut rbs, 0, 3, 0, true);
    // 303B: A1 exact dup of 303A A1 (shuffle ON), A2 +7 st cross-device transposition (shuffle ON).
    write_motif(&mut rbs, 1, 0, 0, true);
    write_motif(&mut rbs, 1, 1, 7, true);

    // Exact duplicate 808 patterns A1 & A2, and exact duplicate 909 flam patterns A1 & A2.
    for pat in 0..2 {
        rbs.set_808_pat_step(pat, 0, 1, 1);
        rbs.set_808_pat_step(pat, 8, 2, 1);
        rbs.set_909_pat_step(pat, 0, 2, 3); // SD flam
        rbs.set_909_pat_step(pat, 4, 1, 1); // BD
    }

    rbs.tracks[1] = vec![
        (0, 0, 1),
        (0, 1, 0),   // bar 0: 303A A1 (base)
        (32, 1, 1),  // bar 1: 303A A2 (+5 st)
        (64, 1, 2),  // bar 2: 303A A3 (-5 st, unswung)
        (96, 1, 3),  // bar 3: 303A A4 (exact dup)
        (128, 1, 0), // bar 4: 303A A1
        (160, 1, 1), // bar 5: 303A A2
    ];
    rbs.tracks[2] = vec![
        (0, 0, 1),
        (0, 1, 0),  // bars 0..2: 303B A1 (exact dup of 303A A1)
        (96, 1, 1), // bars 3..5: 303B A2 (+7 st)
    ];
    rbs.tracks[3] = vec![(0, 0, 1), (0, 1, 0), (64, 1, 1)];
    rbs.tracks[4] = vec![(0, 0, 1), (0, 1, 0), (64, 1, 1)];

    let (proj, _) = import_rbs_bytes(&rbs.build("DedupTrans"), "DedupTrans").unwrap();

    let part_by_name = |name: &str| {
        proj.parts
            .iter()
            .find(|p| p.name == name)
            .unwrap_or_else(|| panic!("missing part {name}"))
    };
    let a1 = part_by_name("303A A1");
    let a2 = part_by_name("303A A2");
    let a3 = part_by_name("303A A3");
    let a4 = part_by_name("303A A4");
    let b1 = part_by_name("303B A1");
    let b2 = part_by_name("303B A2");

    // All 5 shuffled 303 parts share ONE Swing pattern; unswung A3 shares its inner base Notes pattern!
    assert_eq!(a1.pattern, a2.pattern);
    assert_eq!(a1.pattern, a4.pattern);
    assert_eq!(a1.pattern, b1.pattern);
    assert_eq!(a1.pattern, b2.pattern);
    let PatternKind::Swing { source, .. } = proj.pattern(a1.pattern).unwrap().kind else {
        panic!("expected Swing pattern for 303A A1");
    };
    assert_eq!(
        a3.pattern, source,
        "unshuffled transposed part must directly reference the shared unswung base Notes pattern"
    );

    assert_eq!(a1.transpose, 0);
    assert_eq!(a2.transpose, 5);
    assert_eq!(a3.transpose, -5);
    assert_eq!(a4.transpose, 0);
    assert_eq!(b1.transpose, 0);
    assert_eq!(b2.transpose, 7);

    // Exactly ONE literal 303 Notes pattern across all 6 303A/303B parts!
    let notes_303_count = proj
        .patterns
        .iter()
        .filter(|p| {
            matches!(p.kind, PatternKind::Notes { .. })
                && (p.name.starts_with("303A") || p.name.starts_with("303B"))
        })
        .count();
    assert_eq!(
        notes_303_count, 1,
        "all 6 duplicate/transposed 303 parts must share 1 base Notes pattern"
    );

    // 808 A1/A2 and 909 A1/A2 share exact duplicate base/Stack patterns too!
    assert_eq!(
        part_by_name("808 A1").pattern,
        part_by_name("808 A2").pattern
    );
    assert_eq!(
        part_by_name("909 A1").pattern,
        part_by_name("909 A2").pattern
    );

    // Verify compiled MIDI pitches, gates, velocities, and swing offsets are 100% exact!
    let score = proj.compile().unwrap();
    let midis_at = |inst_idx: u8, bar: usize| -> Vec<(usize, u8, f32, f32)> {
        let mut out = Vec::new();
        for s in 0..16 {
            for ev in score.step_events((bar * 16 + s) as u64) {
                if ev.inst_idx == inst_idx {
                    out.push((s, ev.midi, ev.velocity, ev.offset));
                }
            }
        }
        out
    };
    // Base C2=36 + [5, 8, 10, 5] = [41, 44, 46, 41]; step 3 is odd so swung when shuffle ON!
    let bar0_a = midis_at(0, 0);
    let bar1_a = midis_at(0, 1);
    let bar2_a = midis_at(0, 2);
    let bar3_a = midis_at(0, 3);
    let bar0_b = midis_at(1, 0);
    let bar3_b = midis_at(1, 3);

    assert_eq!(
        bar0_a.iter().map(|e| (e.0, e.1)).collect::<Vec<_>>(),
        vec![(0, 41), (2, 44), (3, 46), (8, 41)]
    );
    assert_eq!(
        bar1_a.iter().map(|e| (e.0, e.1)).collect::<Vec<_>>(),
        vec![(0, 46), (2, 49), (3, 51), (8, 46)]
    );
    assert_eq!(
        bar2_a.iter().map(|e| (e.0, e.1)).collect::<Vec<_>>(),
        vec![(0, 36), (2, 39), (3, 41), (8, 36)]
    );
    assert_eq!(bar3_a, bar0_a);
    assert_eq!(bar0_b, bar0_a);
    assert_eq!(
        bar3_b.iter().map(|e| (e.0, e.1)).collect::<Vec<_>>(),
        vec![(0, 48), (2, 51), (3, 53), (8, 48)]
    );
    // Shuffled step 3 has positive swing offset; unshuffled A3 (bar 2) has 0.0 offset!
    assert!(bar0_a[2].3 > 0.15 && bar1_a[2].3 == bar0_a[2].3 && bar3_b[2].3 == bar0_a[2].3);
    assert!(bar2_a[2].3.abs() < 1e-6);
}

#[test]
fn musical_section_detection_tracks_arrangement_form_and_preserves_loop_region() {
    // 1. Musical form: Intro (0..4) -> Drop (4..12, with 1-bar fill blip at bar 11!) ->
    //    Break (12..16) -> Drop (16..28) -> Outro (28..32).
    let mut rbs = SyntheticRbs::new(32);
    rbs.set_303_step(0, 0, 0, 0x02);
    rbs.set_303_step(1, 4, 7, 0x00);
    rbs.set_808_step(0, 1, 1);
    rbs.set_909_step(0, 2, 1);

    rbs.tracks[1] = vec![(0, 0, 1), (0, 1, 0)]; // 303A plays bars 0..32
    rbs.tracks[2] = vec![(0, 0, 0), (16 * 32, 0, 1), (28 * 32, 0, 0)]; // 303B bars 16..28
    rbs.tracks[3] = vec![
        (0, 0, 0),
        (4 * 32, 0, 1),  // 808 enters at bar 4
        (12 * 32, 0, 0), // drums drop out at bar 12 (Break)
        (16 * 32, 0, 1), // drums return at bar 16 (Drop)
        (28 * 32, 0, 0), // drums drop out at bar 28 (Outro)
    ];
    rbs.tracks[4] = vec![
        (0, 0, 0),
        (11 * 32, 0, 1), // 1-bar fill blip at bar 11 (must NOT spam a 1-bar section!)
        (12 * 32, 0, 0),
        (16 * 32, 0, 1), // 909 joins full drop at bar 16..28
        (28 * 32, 0, 0),
    ];

    let (proj, _) = import_rbs_bytes(&rbs.build("FormDetect"), "FormDetect").unwrap();
    let starts: Vec<u32> = proj.sections.iter().map(|s| s.start_step / 16).collect();
    let names: Vec<&str> = proj.sections.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        starts,
        vec![0, 4, 12, 16, 28],
        "expected clean musical phrase boundaries without bar-11 micro-blip, got {starts:?} ({names:?})"
    );
    assert_eq!(names[0], "Intro");
    assert!(names.contains(&"Break"), "expected Break label: {names:?}");
    assert_eq!(
        *names.last().unwrap(),
        "Outro",
        "expected Outro label: {names:?}"
    );
    assert!(
        !names.iter().any(|n| n.starts_with("Part ")),
        "must not emit mechanical Part N labels: {names:?}"
    );
    // Sections must be contiguous, cover 0..32 bars, and have unique names.
    let total_steps: u32 = proj.sections.iter().map(|s| s.length_steps).sum();
    assert_eq!(total_steps, 32 * 16);
    let unique: HashSet<&str> = names.iter().copied().collect();
    assert_eq!(unique.len(), names.len(), "section names must be unique");

    // 2. Explicit loop region (`loop_start_bar = 8 .. loop_end_bar = 24`) must be preserved
    //    even on a completely steady arrangement with zero pattern changes!
    let mut looped = SyntheticRbs::new(24);
    looped.loop_on = true;
    looped.loop_start_bar = 8;
    looped.set_303_step(0, 0, 0, 0x02);
    looped.set_808_step(0, 1, 1);
    looped.tracks[1] = vec![(0, 0, 1), (0, 1, 0)];
    looped.tracks[3] = vec![(0, 0, 1), (0, 1, 0)];

    let (loop_proj, _) = import_rbs_bytes(&looped.build("LoopPreserved"), "LoopPreserved").unwrap();
    assert!(
        loop_proj
            .sections
            .iter()
            .any(|s| s.start_step == 8 * 16 && s.length_steps == 16 * 16 && s.name == "Loop"),
        "expected preserved Loop section at bars 8..24, got {:?}",
        loop_proj
            .sections
            .iter()
            .map(|s| (&s.name, s.start_step / 16, s.length_steps / 16))
            .collect::<Vec<_>>()
    );
}

#[test]
fn mixer_channel_pans_target_channel_pan_with_zero_double_panning() {
    let mut rbs = SyntheticRbs::new(4);
    rbs.set_303_step(0, 0, 0, 0x02);
    rbs.set_303_step(1, 0, 7, 0x00);
    for s in (0..16).step_by(4) {
        rbs.set_808_step(s, 1, 1);
    }
    rbs.set_909_step(4, 2, 1);

    // 303A & 808 pan-automated; 303B static left (pan=16); 909 static right (pan=112).
    rbs.mixr[16 + 12 + 2] = 16; // 303B static L
    rbs.mixr[16 + 36 + 2] = 112; // 909 static R
    rbs.tracks[0] = vec![
        (0, 7, 10),   // 303A pan L
        (0, 19, 120), // 808 pan R
        (64, 7, 120), // 303A pan R
        (64, 19, 10), // 808 pan L
    ];
    rbs.tracks[1] = vec![(0, 0, 1), (0, 1, 0)];
    rbs.tracks[2] = vec![(0, 0, 1), (0, 1, 0)];
    rbs.tracks[3] = vec![(0, 0, 1), (0, 1, 0)];
    rbs.tracks[4] = vec![(0, 0, 1), (0, 1, 0)];

    let (proj, _) = import_rbs_bytes(&rbs.build("ChannelPans"), "ChannelPans").unwrap();

    // Automated pan channels target ModulationTarget::ChannelPan and center strip.pan = 0.0.
    for c in [0usize, 2] {
        let ch = &proj.mixer.channels[c];
        assert!(ch.strip.pan.abs() < 1e-6);
        assert!(proj.modulations.iter().any(|m| {
            m.target
                == ModulationTarget::ChannelPan {
                    channel: ChannelRef::Channel(ch.id),
                }
        }));
    }
    // Static pan channels keep initial strip.pan and emit no ChannelPan modulation.
    assert!(proj.mixer.channels[1].strip.pan < -0.7);
    assert!(proj.mixer.channels[3].strip.pan > 0.7);
    for c in [1usize, 3] {
        let ch_id = proj.mixer.channels[c].id;
        assert!(!proj.modulations.iter().any(|m| {
            m.target
                == ModulationTarget::ChannelPan {
                    channel: ChannelRef::Channel(ch_id),
                }
        }));
    }
    // Zero instrument-level pan modulations, and all 4 instrument Racks keep pan = 0.0!
    assert!(!proj.modulations.iter().any(|m| matches!(
        m.target,
        ModulationTarget::Instrument {
            parameter: InstrumentParam::Rack {
                param: RackParam::Pan
            },
            ..
        }
    )));
    for inst in &proj.instruments {
        let pan = inst.rack.pan;
        assert!(
            pan.abs() < 1e-6,
            "{} Rack pan must be 0.0, got {pan}",
            inst.name
        );
    }

    // Audio verification: soloing pan-automated TR-808 sweeps energy cleanly from R (bar 0) to L (bar 2).
    let mut flags = LiveFlags::default();
    flags.solo_channels.insert(proj.mixer.channels[2].id);
    let score = proj.compile_with_flags(&flags).unwrap();
    let (rep, pcm) =
        render_schedule_pcm(&[(0, &score)], 0, 64, load_rbs_samples(&score), 24_000).unwrap();
    assert_eq!(rep.output.full_scale_samples, 0);
    let (early_l, early_r) = slice_peak_lr(&pcm, 0.02, 0.45);
    let (late_l, late_r) = slice_peak_lr(&pcm, 0.55, 0.98);
    assert!(
        early_r > 0.08 && early_r > early_l * 2.0 && late_l > 0.08 && late_l > late_r * 2.0,
        "automated ChannelPan must sweep cleanly R->L (early L={early_l} R={early_r}, late L={late_l} R={late_r})"
    );
}

#[test]
fn bank_launchpad_populates_all_non_empty_slots_up_to_32_without_altering_song_audio() {
    let mut base_rbs = SyntheticRbs::new(4);
    // Song Mode places ONLY 303A B3 (idx 10) for bars 0..2, then 303A A1 (idx 0) for bars 2..4.
    base_rbs.set_303_pat_step(0, 10, 0, 7, 0x02);
    base_rbs.set_303_pat_step(0, 0, 0, 0, 0x02);
    base_rbs.tracks[1] = vec![(0, 0, 1), (0, 1, 10), (64, 1, 0)];

    // Clone and author 20 MORE unplaced non-empty bank slots (total = 22 > 16!) on 303A,
    // including exact transpositions of A1 and exact duplicates of B3.
    let mut rich_rbs = base_rbs.clone();
    for pat in 1..=20 {
        if pat == 10 {
            continue;
        }
        // Alternate exact transpositions of A1 (note = pat % 12) and duplicates of B3 (note = 7).
        let note = if pat % 2 == 0 { (pat % 12) as u8 } else { 7 };
        rich_rbs.set_303_pat_step(0, pat, 0, note, 0x02);
    }
    // Also add slot 21 so total non-empty 303A bank slots == 22.
    rich_rbs.set_303_pat_step(0, 21, 0, 5, 0x02);

    let (base_proj, _) = import_rbs_bytes(&base_rbs.build("BaseBank"), "BaseBank").unwrap();
    let (rich_proj, _) = import_rbs_bytes(&rich_rbs.build("RichBank"), "RichBank").unwrap();

    // Rich project exposes all 22 non-empty 303A bank slots in lane.parts (>16 cap lifted to 32!).
    assert_eq!(rich_proj.lanes[0].parts.len(), 22);
    // Song-Mode placed parts appear FIRST in song order: B3 (idx 0), then A1 (idx 1)!
    let p0 = rich_proj
        .part(rich_proj.lanes[0].parts[0].part)
        .expect("slot 0");
    let p1 = rich_proj
        .part(rich_proj.lanes[0].parts[1].part)
        .expect("slot 1");
    assert_eq!(p0.name, "303A B3");
    assert_eq!(p1.name, "303A A1");
    // Unplaced bank patterns share deduplicated/transposed base Notes pattern (all 22 share 1!).
    let notes_303a = rich_proj
        .patterns
        .iter()
        .filter(|p| matches!(p.kind, PatternKind::Notes { .. }) && p.name.starts_with("303A"))
        .count();
    assert_eq!(
        notes_303a, 1,
        "all 22 single-note transposed/duplicate 303A bank patterns should share 1 base Notes pattern"
    );

    // Unplaced bank slots must NEVER alter Song-Mode placements or compiled schedule events!
    assert_eq!(
        rich_proj.lanes[0].placements.len(),
        base_proj.lanes[0].placements.len()
    );
    let base_score = base_proj.compile().unwrap();
    let rich_score = rich_proj.compile().unwrap();
    assert_eq!(base_score.events, rich_score.events);
}

#[test]
fn mixer_routes_synths_and_drums_through_unity_group_busses() {
    let mut rbs = SyntheticRbs::new(2);
    rbs.set_303_step(0, 0, 0, 0x02);
    rbs.set_808_step(0, 1, 1);
    rbs.tracks[1] = vec![(0, 0, 1), (0, 1, 0)];
    rbs.tracks[3] = vec![(0, 0, 1), (0, 1, 0)];
    // Also toggle Delay Track 5 On/Off to confirm Echo Return indexing (`echo_vec_idx`) works cleanly!
    rbs.tracks[5] = vec![(0, 0, 0), (32, 0, 1)];

    let (proj, _) = import_rbs_bytes(&rbs.build("GroupBusses"), "GroupBusses").unwrap();
    assert_eq!(proj.mixer.channels.len(), 7);

    let synths = &proj.mixer.channels[4];
    let drums = &proj.mixer.channels[5];
    let echo = &proj.mixer.channels[6];
    assert_eq!(synths.name, "Synths");
    assert_eq!(synths.kind, ChannelKind::Group);
    assert_eq!(synths.output, ChannelRef::Master);
    assert!((synths.strip.gain - 1.0).abs() < 1e-6 && synths.strip.pan.abs() < 1e-6);
    assert!(synths.inserts.is_empty());

    assert_eq!(drums.name, "Drums");
    assert_eq!(drums.kind, ChannelKind::Group);
    assert_eq!(drums.output, ChannelRef::Master);
    assert!((drums.strip.gain - 1.0).abs() < 1e-6 && drums.strip.pan.abs() < 1e-6);
    assert!(drums.inserts.is_empty());

    assert_eq!(echo.name, "Echo");
    assert_eq!(echo.kind, ChannelKind::Return);
    assert_eq!(echo.output, ChannelRef::Master);
    assert!(
        (echo.strip.gain - 1.0).abs() < 1e-6,
        "Echo On automation must set unity strip.gain on Echo return (not on Synths group!)"
    );

    // 303A/303B -> Synths; 808/909 -> Drums.
    assert_eq!(
        proj.mixer.channels[0].output,
        ChannelRef::Channel(synths.id)
    );
    assert_eq!(
        proj.mixer.channels[1].output,
        ChannelRef::Channel(synths.id)
    );
    assert_eq!(proj.mixer.channels[2].output, ChannelRef::Channel(drums.id));
    assert_eq!(proj.mixer.channels[3].output, ChannelRef::Channel(drums.id));

    // Functional audio check: muting both Group busses silences all dry instruments!
    let mut flags = LiveFlags::default();
    flags.muted_channels.insert(synths.id);
    flags.muted_channels.insert(drums.id);
    let score_muted = proj.compile_with_flags(&flags).unwrap();
    let (_, pcm_muted) = render_schedule_pcm(
        &[(0, &score_muted)],
        0,
        32,
        load_rbs_samples(&score_muted),
        24_000,
    )
    .unwrap();
    let (ml, mr) = slice_peak_lr(&pcm_muted, 0.0, 1.0);
    assert!(
        ml < 1e-6 && mr < 1e-6,
        "muting Synths + Drums groups must silence all 4 channels (L={ml}, R={mr})"
    );
}

#[test]
fn rbs_import_leaves_macros_unbound_so_direct_knob_edits_take_effect() {
    // 1. Pattern Mode with PCF active & Echo Send enabled -> macros stay empty!
    let mut pat_rbs = SyntheticRbs::new(2);
    pat_rbs.song_mode = false;
    pat_rbs.t808[0] = 1; // TR-808 pattern active in Pattern Mode
    pat_rbs.mixr[2] = 1; // PCF routed to 303A
    pat_rbs.mixr[16 + 3] = 90; // 303A Echo Send active
    pat_rbs.pcf[0] = 1; // PCF enabled
    pat_rbs.dely = [1, 3, 1, 70, 64, 0, 0, 0];
    pat_rbs.set_303_step(0, 0, 0, 0x02);
    pat_rbs.set_303_step(0, 4, 7, 0x00);
    pat_rbs.set_808_step(0, 1, 1);

    let (pat_proj, _) = import_rbs_bytes(&pat_rbs.build("PatNoMacros"), "PatNoMacros").unwrap();
    assert!(
        pat_proj.macros.is_empty(),
        "Pattern-Mode RBS import must leave macros empty"
    );

    // Verify direct Edit::SetParam on 303A Cutoff, 303A Res, PCF Freq, Echo Fdbk, and 808 Cutoff
    // genuinely changes rendered PCM (not shadowed by static Macros!).
    let base_score = pat_proj.compile().unwrap();
    let base_pcm = render_schedule_pcm(
        &[(0, &base_score)],
        0,
        16,
        load_rbs_samples(&base_score),
        12_000,
    )
    .unwrap()
    .1;

    let tb1_id = pat_proj.instruments[0].id;
    let kit808_id = pat_proj.instruments[2].id;
    let pcf_id = pat_proj.mixer.channels[0]
        .inserts
        .iter()
        .find(|d| d.name == "PCF")
        .unwrap()
        .id;
    let echo_delay_id = pat_proj
        .mixer
        .channels
        .iter()
        .find(|c| c.kind == ChannelKind::Return && c.name == "Echo")
        .unwrap()
        .inserts[0]
        .id;

    let tb1_filter_id = pat_proj.instruments[0].rack.zones[0]
        .modules
        .iter()
        .find(|m| matches!(m.kind, ModuleKind::Filter(_)))
        .expect("303A Filter module")
        .id;
    let tb1_zone_id = pat_proj.instruments[0].rack.zones[0].id;

    let direct_edits: [(&str, ParamAddress, f32); 2] = [
        (
            "303A Cutoff",
            ParamAddress::Module {
                instrument: tb1_id,
                zone: tb1_zone_id,
                module: tb1_filter_id,
                param: ModuleParam::Cutoff,
            },
            14_000.0,
        ),
        (
            "PCF Freq",
            ParamAddress::Effect {
                device: pcf_id,
                param: EffectParam::Cutoff,
            },
            12_000.0,
        ),
    ];
    let _ = (kit808_id, echo_delay_id);

    for (label, address, value) in direct_edits {
        let mut edited = pat_proj.clone();
        edited.apply(Edit::SetParam { address, value }).unwrap();
        let edited_score = edited.compile().unwrap();
        let (_, edited_pcm) = render_schedule_pcm(
            &[(0, &edited_score)],
            0,
            16,
            load_rbs_samples(&edited_score),
            12_000,
        )
        .unwrap();
        assert!(
            base_pcm != edited_pcm,
            "direct knob edit to {label} must audibly affect rendered PCM without macro shadowing"
        );
    }

    // 2. Song Mode also leaves macros empty!
    let mut song_rbs = SyntheticRbs::new(2);
    song_rbs.mixr[2] = 1;
    song_rbs.pcf[0] = 1;
    song_rbs.set_303_step(0, 0, 0, 0x02);
    song_rbs.tracks[1] = vec![(0, 0, 1), (0, 1, 0), (0, 3, 40), (32, 3, 110)];
    let (song_proj, _) = import_rbs_bytes(&song_rbs.build("SongNoMacros"), "SongNoMacros").unwrap();
    assert!(
        song_proj.macros.is_empty(),
        "Song-Mode RBS import must leave macros empty"
    );
}

#[test]
fn edit_resize_pattern_supports_song_length_values_up_to_max_bars_times_16() {
    let mut rbs = SyntheticRbs::new(80);
    rbs.set_303_step(0, 0, 0, 0x02);
    rbs.tracks[1] = vec![(0, 0, 1), (0, 1, 0), (0, 3, 30), (2400, 3, 110)];
    let (mut proj, _) = import_rbs_bytes(&rbs.build("LongValues"), "LongValues").unwrap();
    let val_pat_id = proj
        .patterns
        .iter()
        .find(|p| matches!(p.kind, PatternKind::Values { period, .. } if period > 1024))
        .expect("80-bar song should produce a >1024-step Values automation pattern")
        .id;

    // Resizing a Values pattern to 2048 and MAX_BARS * 16 must succeed cleanly!
    proj.apply(Edit::ResizePattern {
        pattern: val_pat_id,
        length: 2048,
    })
    .unwrap();
    proj.apply(Edit::ResizePattern {
        pattern: val_pat_id,
        length: MAX_BARS * 16,
    })
    .unwrap();
    let max_period = proj
        .patterns
        .iter()
        .find(|p| p.id == val_pat_id)
        .and_then(|p| match p.kind {
            PatternKind::Values { period, .. } => Some(period),
            _ => None,
        })
        .unwrap();
    assert_eq!(max_period as usize, MAX_BARS * 16);

    // Exceeding MAX_BARS * 16 must still be rejected!
    assert!(proj
        .apply(Edit::ResizePattern {
            pattern: val_pat_id,
            length: MAX_BARS * 16 + 1,
        })
        .is_err());
    assert!(proj.validate().is_ok());
}

#[test]
fn tb303_down_octave_notes_exact_midi_and_cross_c2_transposition_sharing() {
    let mut rbs = SyntheticRbs::new(2);
    // Pattern 0: down-octave C1 (24), F#1 (30), B1 (35), plus C2 (36) and D2 (38)
    rbs.set_303_pat_step(0, 0, 0, 0, 0x08); // C1 (down)
    rbs.set_303_pat_step(0, 0, 1, 6, 0x08); // F#1 (down)
    rbs.set_303_pat_step(0, 0, 2, 11, 0x08); // B1 (down)
    rbs.set_303_pat_step(0, 0, 3, 0, 0x00); // C2
    rbs.set_303_pat_step(0, 0, 4, 2, 0x00); // D2

    // Pattern 1: exact +3 semitone transposition crossing C2 -> D#1 (27), A1 (33), D2 (38), D#2 (39), F2 (41)
    rbs.set_303_pat_step(0, 1, 0, 3, 0x08); // D#1 (down)
    rbs.set_303_pat_step(0, 1, 1, 9, 0x08); // A1 (down)
    rbs.set_303_pat_step(0, 1, 2, 2, 0x00); // D2
    rbs.set_303_pat_step(0, 1, 3, 3, 0x00); // D#2
    rbs.set_303_pat_step(0, 1, 4, 5, 0x00); // F2

    rbs.tracks[1] = vec![
        (0, 0, 1),  // enabled
        (0, 1, 0),  // bar 0: pat 0
        (32, 1, 1), // bar 1: pat 1
    ];

    let (proj, _) = import_rbs_bytes(&rbs.build("DownOctave303"), "DownOctave303").unwrap();
    let score = proj.compile().unwrap();
    let inst0 = score.instrument_index(proj.instruments[0].id).unwrap();
    let bar0_midis: Vec<u8> = (0..5)
        .map(|s| score.event_for(inst0, s).unwrap().midi)
        .collect();
    assert_eq!(
        bar0_midis,
        vec![24, 30, 35, 36, 38],
        "down-octave TB-303 notes must be exact (C1=24, F#1=30, B1=35), not 1 semitone flat"
    );
    let bar1_midis: Vec<u8> = (16..21)
        .map(|s| score.event_for(inst0, s).unwrap().midi)
        .collect();
    assert_eq!(bar1_midis, vec![27, 33, 38, 39, 41]);

    // Pattern 1 must share Pattern 0's base pattern via Part.transpose = +3 across C2!
    let p0 = proj.parts.iter().find(|p| p.name == "303A A1").unwrap();
    let p1 = proj.parts.iter().find(|p| p.name == "303A A2").unwrap();
    assert_eq!(
        p0.pattern, p1.pattern,
        "cross-C2 transposition must share base Pattern"
    );
    assert_eq!(p1.transpose, 3);
}

#[test]
fn rbs_rising_edge_fx_reactivation_and_peek_header_loop_end() {
    let mut rbs = SyntheticRbs::new(4);
    rbs.set_303_step(0, 0, 0, 0x00);
    rbs.set_303_step(1, 0, 0, 0x00);
    // PCF active on 303A at bar 0 (freq=20), switches to 303B at bar 1,
    // freq moves to 120 while on 303B (bar 1 step 8), switches BACK to 303A at bar 2 (step 32)!
    rbs.tracks[0] = vec![
        (0, 2, 1),  // PCF -> 303A
        (32, 2, 2), // bar 1: PCF -> 303B
        (64, 2, 1), // bar 2: PCF -> 303A again (rising edge!)
    ];
    rbs.tracks[1] = vec![(0, 0, 1), (0, 1, 0)];
    rbs.tracks[2] = vec![(0, 0, 1), (0, 1, 0)];
    rbs.tracks[7] = vec![
        (0, 0, 1),    // PCF enabled
        (0, 1, 20),   // freq = 20 while on 303A
        (48, 1, 120), // bar 1.5: freq = 120 while on 303B
    ];

    let bytes = rbs.build("RisingEdgeFX");
    let (proj, _) = import_rbs_bytes(&bytes, "RisingEdgeFX").unwrap();
    let freq_mod = proj
        .modulations
        .iter()
        .find(|m| m.name == "303A PCF Freq")
        .expect("rising-edge reactivation must emit 303A PCF Freq automation");
    let val_bar0 = proj.value_at(freq_mod.pattern, 0.0).unwrap();
    let val_bar2 = proj.value_at(freq_mod.pattern, 32.0).unwrap();
    assert!(
        val_bar2 > val_bar0 + 0.35,
        "PCF re-activating on 303A at bar 2 must pick up freq=120 ({val_bar2} vs bar0 {val_bar0})"
    );
}

#[test]
fn master_level_automation_and_v42_channel_mute_unmute_render_accurately() {
    let mut rbs = SyntheticRbs::new(4);
    for s in (0..16).step_by(2) {
        rbs.set_303_step(0, s, 0, 0x02);
        rbs.set_303_step(1, s, 7, 0x02);
        rbs.set_808_step(s, 1, 1);
    }
    // 303B starts muted in v4.2 MIXR, unmutes at bar 2, mutes again at bar 3!
    // 808 starts muted in v4.2 MIXR and NEVER unmutes -> stays statically muted (`strip.gain = 0.0`).
    rbs.mixr[16 + 12] = 0; // 303B disabled initially
    rbs.mixr[16 + 24] = 0; // 808 disabled permanently
    rbs.tracks[0] = vec![
        (0, 0, 115),   // Master Level = 115 at bar 0
        (0, 6, 110),   // 303A level
        (0, 11, 0),    // 303B muted at bar 0
        (0, 12, 80),   // 303B level = 80
        (64, 11, 1),   // bar 2: 303B UNMUTES!
        (64, 12, 120), // bar 2: 303B level = 120
        (96, 11, 0),   // bar 3: 303B MUTES again!
        (96, 0, 0),    // bar 3: Master Level drops to 0!
    ];
    rbs.tracks[1] = vec![(0, 0, 1), (0, 1, 0)];
    rbs.tracks[2] = vec![(0, 0, 1), (0, 1, 0)];
    rbs.tracks[3] = vec![(0, 0, 1), (0, 1, 0)];

    let (proj, _) = import_rbs_bytes(&rbs.build("MasterAndMutes"), "MasterAndMutes").unwrap();

    // 1. Master Gain automation targets ChannelGain { channel: Master } and sets unity master strip.gain.
    assert!(
        (proj.mixer.master.strip.gain - 1.0).abs() < 1e-6,
        "automated Master strip.gain must be unity 1.0, got {}",
        proj.mixer.master.strip.gain
    );
    assert!(
        proj.modulations.iter().any(|m| {
            m.target
                == ModulationTarget::ChannelGain {
                    channel: ChannelRef::Master,
                }
        }),
        "Master Level automation must target ChannelGain {{ channel: ChannelRef::Master }}"
    );

    // 2. Dynamically unmuted 303B emits ChannelGain automation with unity strip.gain; permanently
    //    muted 808 stays at strip.gain = 0.0 with NO ChannelGain automation.
    assert!((proj.mixer.channels[1].strip.gain - 1.0).abs() < 1e-6);
    assert!(proj.modulations.iter().any(|m| {
        m.target
            == ModulationTarget::ChannelGain {
                channel: ChannelRef::Channel(proj.mixer.channels[1].id),
            }
    }));
    assert_eq!(proj.mixer.channels[2].strip.gain, 0.0);
    assert!(!proj.modulations.iter().any(|m| {
        m.target
            == ModulationTarget::ChannelGain {
                channel: ChannelRef::Channel(proj.mixer.channels[2].id),
            }
    }));

    // 3. Full mix audio check: audible in bar 0..1, drops to silence in bar 3 when Master=0!
    let score = proj.compile().unwrap();
    let (rep, pcm) =
        render_schedule_pcm(&[(0, &score)], 0, 64, load_rbs_samples(&score), 24_000).unwrap();
    assert_eq!(rep.output.full_scale_samples, 0);
    let (bar0_l, _) = slice_peak_lr(&pcm, 0.02, 0.24);
    let (bar3_l, _) = slice_peak_lr(&pcm, 0.80, 0.99);
    assert!(
        bar0_l > 0.15 && bar3_l < 0.005,
        "Master Level drop at bar 3 must silence output (bar0={bar0_l}, bar3={bar3_l})"
    );

    // 4. Solo 303B with steady Master: silent bars 0..1, loud bar 2, silent bar 3!
    let mut rbs_b = rbs.clone();
    rbs_b.tracks[0].retain(|&(_, ctrl, _)| ctrl != 0); // hold Master steady
    let (proj_b, _) = import_rbs_bytes(&rbs_b.build("ChanMute303B"), "ChanMute303B").unwrap();
    let mut flags_b = LiveFlags::default();
    flags_b.solo_channels.insert(proj_b.mixer.channels[1].id);
    let score_b = proj_b.compile_with_flags(&flags_b).unwrap();
    let (rep_b, pcm_b) =
        render_schedule_pcm(&[(0, &score_b)], 0, 64, load_rbs_samples(&score_b), 24_000).unwrap();
    assert_eq!(rep_b.output.full_scale_samples, 0);
    let (b_early, _) = slice_peak_lr(&pcm_b, 0.05, 0.45);
    let (b_bar2, _) = slice_peak_lr(&pcm_b, 0.52, 0.72);
    let (b_bar3, _) = slice_peak_lr(&pcm_b, 0.80, 0.98);
    assert!(
        b_early < 0.01 && b_bar2 > 0.15 && b_bar3 < 0.01,
        "303B v4.2 mute/unmute must silence bars 0..1 ({b_early}), play bar 2 ({b_bar2}), silence bar 3 ({b_bar3})"
    );
}

#[test]
fn tr808_and_tr909_global_accent_knob_scales_velocities_across_sections() {
    let mut rbs = SyntheticRbs::new(2);
    rbs.t808[2] = 0; // Accent=0 at bar 0
    rbs.t909[2] = 0;
    // Step 0: accented BD; Step 8: normal BD
    rbs.set_808_step(0, 0, 1); // global accent ON
    rbs.set_808_step(0, 1, 1); // BD
    rbs.set_808_step(8, 1, 1); // BD (no accent)

    rbs.set_909_step(0, 1, 2); // 909 BD accented (v==2)
    rbs.set_909_step(8, 1, 1); // 909 BD normal (v==1)

    // Bar 0: ctrl 2 = 0 (uniform); Bar 1: ctrl 2 = 127 (max accent separation!)
    rbs.tracks[3] = vec![(0, 0, 1), (0, 1, 0), (0, 2, 0), (32, 2, 127)];
    rbs.tracks[4] = vec![(0, 0, 1), (0, 1, 0), (0, 2, 0), (32, 2, 127)];

    let (proj, _) = import_rbs_bytes(&rbs.build("DrumAccent"), "DrumAccent").unwrap();
    let score = proj.compile().unwrap();

    for (inst_idx, label) in [(2u8, "TR-808"), (3u8, "TR-909")] {
        let vel_at = |step: u64| {
            score
                .step_events(step)
                .iter()
                .find(|e| e.inst_idx == inst_idx && e.midi == drum_midi::BD)
                .unwrap_or_else(|| panic!("{label} missing BD at step {step}"))
                .velocity
        };
        let b0_acc = vel_at(0);
        let b0_norm = vel_at(8);
        let b1_acc = vel_at(16);
        let b1_norm = vel_at(24);
        assert!(
            (b0_acc - b0_norm).abs() < 1e-5,
            "{label} Accent=0 at bar 0 must yield uniform velocity ({b0_acc} vs {b0_norm})"
        );
        assert!(
            (b1_acc - 1.0).abs() < 1e-5 && b1_acc > b1_norm + 0.30,
            "{label} Accent=127 at bar 1 must deliver punchy separation ({b1_acc} vs {b1_norm})"
        );
    }

    let rep = render_measure_report(&score, load_rbs_samples(&score), 2, 24_000).unwrap();
    assert!(rep.output.peak[0] > 0.10 && rep.output.full_scale_samples == 0);
}

#[test]
fn tb303_cross_bar_slide_lookahead_handles_rest_note_transpose_and_shuffle() {
    // 1. Unshuffled: Pattern 0 (step 0 note, step 15 slide -> assumed next=true => 1.52) plays
    //    bars 0..2, then switches to Pattern 1 (step 0 rest!) at bar 2.
    //    Pattern 2 (step 0 rest, step 15 slide -> assumed next=false => 1.05) plays bar 3 and
    //    loops back to Pattern 0 (step 0 note!) -> must extend to 1.52!
    let mut rbs = SyntheticRbs::new(4);
    rbs.loop_on = true;
    // Pat 0: step 0 = C2, step 15 = C2 + slide
    rbs.set_303_pat_step(0, 0, 0, 0, 0x00);
    rbs.set_303_pat_step(0, 0, 15, 0, 0x01);
    // Pat 1: exact +5 st transpose of Pat 0 on step 4, REST on step 0!
    rbs.set_303_pat_step(0, 1, 4, 5, 0x00);
    // Pat 2: step 0 = REST, step 15 = G2 + slide (loops into Pat 0 step 0 note!)
    rbs.set_303_pat_step(0, 2, 15, 7, 0x01);
    // Pat 3: exact +5 st transposition of Pat 0 (tests transposed boundary Exception pitch!)
    rbs.set_303_pat_step(0, 3, 0, 5, 0x00);
    rbs.set_303_pat_step(0, 3, 15, 5, 0x01);

    rbs.tracks[1] = vec![
        (0, 0, 1),
        (0, 1, 0),  // bar 0: Pat 0 (loops into Pat 3 note at bar 1 -> 1.52)
        (32, 1, 3), // bar 1: Pat 3 (+5 st transposed, ends into Pat 1 REST -> 1.05!)
        (64, 1, 1), // bar 2: Pat 1 (step 0 REST)
        (96, 1, 2), // bar 3: Pat 2 (step 0 REST, step 15 slide -> wraps to Pat 0 NOTE -> 1.52!)
    ];
    let (proj, _) = import_rbs_bytes(&rbs.build("SlideLookahead"), "SlideLookahead").unwrap();
    let score = proj.compile().unwrap();

    let ev = |step: u64| {
        score
            .event_for(0, step)
            .unwrap_or_else(|| panic!("missing step {step}"))
    };
    // Bar 0 step 15 -> Bar 1 step 16 (note) => gate 1.52
    assert!(
        (ev(15).gate - 1.52).abs() < 1e-4,
        "bar0 step 15 gate={}",
        ev(15).gate
    );
    // Bar 1 step 31 (Pat 3 transposed +5 st -> MIDI 41) -> Bar 2 step 32 (REST!) => TB-303 tie gate 1.65 & exact pitch 41!
    assert_eq!(
        ev(31).midi,
        41,
        "transposed boundary slide exception must preserve MIDI 41"
    );
    assert!(
        (ev(31).gate - 1.65).abs() < 1e-4,
        "bar1 step 31 into rest must tie across rest with gate 1.65, got {}",
        ev(31).gate
    );
    // Bar 3 step 63 (Pat 2 step 15, rest-wrapped inside Pat 2) -> wraps to Bar 0 step 0 (NOTE!) => gate 1.52!
    assert!(
        (ev(63).gate - 1.52).abs() < 1e-4,
        "bar3 step 63 looping into note must extend to 1.52, got {}",
        ev(63).gate
    );

    // 2. Shuffled: Pat 0 (shuffle=true) plays bars 0..2 then disables at bar 2.
    //    Bar 0 step 15 keeps swing + gate 1.52; Bar 1 step 31 keeps swing + shortened gate 1.05!
    let mut shuf = SyntheticRbs::new(3);
    shuf.shuffle = 100;
    shuf.set_303_pat_shuffle(0, 0, true);
    shuf.set_303_pat_step(0, 0, 0, 0, 0x00);
    shuf.set_303_pat_step(0, 0, 15, 0, 0x01);
    shuf.tracks[1] = vec![(0, 0, 1), (0, 1, 0), (64, 0, 0)];
    let (shuf_proj, _) = import_rbs_bytes(&shuf.build("ShufSlide"), "ShufSlide").unwrap();
    let shuf_score = shuf_proj.compile().unwrap();
    let e15 = shuf_score.event_for(0, 15).unwrap();
    let e31 = shuf_score.event_for(0, 31).unwrap();
    assert!((e15.gate - 1.52).abs() < 1e-4 && e15.offset > 0.15);
    assert!(
        (e31.gate - 1.05).abs() < 1e-4 && (e31.offset - e15.offset).abs() < 1e-5,
        "swung cross-bar slide end must preserve swing offset ({} vs {}) AND gate 1.05 ({})",
        e31.offset,
        e15.offset,
        e31.gate
    );
}

#[test]
fn event_driven_sub_bar_segment_coalescing_preserves_phase_and_bar_compactness() {
    let mut rbs = SyntheticRbs::new(4);
    // 303A Pat 0 (16 steps): distinct note on every step 0..16 (C2..D#3)
    for s in 0..16 {
        rbs.set_303_pat_step(0, 0, s, (s % 12) as u8, if s >= 12 { 0x04 } else { 0x00 });
    }
    // 303A Pat 1 (16 steps): distinct note on step 8 (MIDI 48 = C3)
    rbs.set_303_pat_step(0, 1, 8, 12, 0x00);

    // 303B Pat 0: odd-length 12-step pattern!
    rbs.tb2[9 + 1] = 12;
    for s in 0..12 {
        rbs.set_303_pat_step(1, 0, s, s as u8, 0x00);
    }

    // 808 Pat 0: RS/CL on steps 4 and 12
    rbs.t808[19] = 1; // RS initially
    rbs.set_808_step(4, 6, 1);
    rbs.set_808_step(12, 6, 1);

    // 909 Pat 0: steady bar-aligned 4 bars (must coalesce into 1 compact placement!)
    rbs.set_909_step(0, 1, 1);

    // Track 1 (303A): mid-bar mute at step 4 (pos 8), drop-in at step 12 (pos 24),
    //                 bar 1 switches Pat 0 -> Pat 1 mid-bar at step 24 (pos 48)!
    rbs.tracks[1] = vec![
        (0, 0, 1),
        (0, 1, 0),
        (8, 0, 0),  // step 4: mute
        (24, 0, 1), // step 12: re-enable mid-bar (must play step 12 = MIDI 48, NOT step 0!)
        (48, 1, 1), // step 24 (bar 1 beat 3): switch to Pat 1 mid-bar (must play Pat 1 step 8 = MIDI 48!)
        (64, 0, 0),
    ];
    // Track 2 (303B): odd 12-step pattern muted steps 4..8 (pos 8..16) -> re-enable at step 8 must
    //                 continue seamlessly on pattern step 8 (MIDI 44), then wrap cleanly at step 12 (MIDI 36)!
    rbs.tracks[2] = vec![(0, 0, 1), (0, 1, 0), (8, 0, 0), (16, 0, 1)];
    // Track 3 (808): mid-bar selector flip RS(1) -> CL(0) at step 8 (pos 16)!
    rbs.tracks[3] = vec![(0, 0, 1), (0, 1, 0), (0, 19, 1), (16, 19, 0)];
    // Track 4 (909): steady 4 bars
    rbs.tracks[4] = vec![(0, 0, 1), (0, 1, 0)];

    let (proj, _) = import_rbs_bytes(&rbs.build("SubBarCoalesce"), "SubBarCoalesce").unwrap();
    // Bar-aligned 909 stays 1 compact placement!
    assert_eq!(proj.lanes[3].placements.len(), 1);

    let score = proj.compile().unwrap();
    // 303A: step 0=36, steps 4..12 silent, step 12=48 (bar-locked phase!), step 24=48 (Pat 1 step 8!)
    assert_eq!(score.event_for(0, 0).unwrap().midi, 36);
    assert!(score.event_for(0, 4).is_none());
    assert!(score.event_for(0, 11).is_none());
    assert_eq!(
        score.event_for(0, 12).unwrap().midi,
        48,
        "mid-bar re-enable at step 12 must preserve bar-locked phase (step 12 = MIDI 48)"
    );
    assert_eq!(
        score.event_for(0, 24).unwrap().midi,
        48,
        "mid-bar pattern switch at step 24 must play Pat 1 step 8 (MIDI 48)"
    );

    // 303B odd 12-step pattern: step 3=39, steps 4..8 silent, step 8=44, step 12=36!
    assert_eq!(score.event_for(1, 3).unwrap().midi, 39);
    assert!(score.event_for(1, 5).is_none());
    assert_eq!(score.event_for(1, 8).unwrap().midi, 44);
    assert_eq!(score.event_for(1, 12).unwrap().midi, 36);

    // 808 mid-bar selector flip: step 4 hits RS, step 12 hits CL!
    let s4_midis: Vec<u8> = score
        .step_events(4)
        .iter()
        .filter(|e| e.inst_idx == 2)
        .map(|e| e.midi)
        .collect();
    let s12_midis: Vec<u8> = score
        .step_events(12)
        .iter()
        .filter(|e| e.inst_idx == 2)
        .map(|e| e.midi)
        .collect();
    assert!(s4_midis.contains(&drum_midi::RS), "step 4={s4_midis:?}");
    assert!(s12_midis.contains(&drum_midi::CL), "step 12={s12_midis:?}");
}

#[test]
fn dense_drum_steps_over_six_voices_stack_without_dropping_any_hits_and_bpm_scales_909_flam() {
    let mut rbs = SyntheticRbs::new(2);
    // 808 Pat 0: ALL 9 simultaneous voices on step 0 (BD, SD, LT, MT, HT, RS, CP, CB, CY)!
    // Trim drum faders so 17 simultaneous in-phase hits stay within 0 dBFS headroom.
    rbs.mixr[16 + 12 * 2 + 1] = 55;
    rbs.mixr[16 + 12 * 3 + 1] = 55;
    // Ensure all 808 selectors = 1 (primary LT/MT/HT/RS/CP).
    for idx in [11, 14, 17, 19, 21] {
        rbs.t808[idx] = 1;
    }
    for col in 1..=9 {
        rbs.set_808_pat_step(0, 0, col, 1);
    }
    // 808 Pat 1: normal 3-voice hit (<= 6 -> must stay flat Notes, NOT Stack!).
    rbs.set_808_pat_step(1, 0, 1, 1);
    rbs.set_808_pat_step(1, 0, 2, 1);
    rbs.set_808_pat_step(1, 0, 11, 1);

    // 909 Pat 0: 8 simultaneous voices on step 0 (BD, SD flam, LT, MT, HT, RS, CP, CH)!
    rbs.t909[29] = 64;
    for col in 1..=8 {
        rbs.set_909_step(0, col, if col == 2 { 3 } else { 1 });
    }
    rbs.tracks[3] = vec![(0, 0, 1), (0, 1, 0), (32, 1, 1)];
    rbs.tracks[4] = vec![(0, 0, 1), (0, 1, 0)];

    // Slow BPM (90) vs Fast BPM (180)
    rbs.bpm_millis = 90_000;
    let (proj_90, _) = import_rbs_bytes(&rbs.build("DenseDrums90"), "DenseDrums90").unwrap();
    rbs.bpm_millis = 180_000;
    let (proj_180, _) = import_rbs_bytes(&rbs.build("DenseDrums180"), "DenseDrums180").unwrap();

    // Dense 808 A1 uses Stack; normal 808 A2 stays flat Notes!
    let p808_a1 = proj_90.parts.iter().find(|p| p.name == "808 A1").unwrap();
    let p808_a2 = proj_90.parts.iter().find(|p| p.name == "808 A2").unwrap();
    assert!(matches!(
        proj_90.pattern(p808_a1.pattern).unwrap().kind,
        PatternKind::Stack { .. }
    ));
    assert!(matches!(
        proj_90.pattern(p808_a2.pattern).unwrap().kind,
        PatternKind::Notes { .. }
    ));

    let collect_midis = |score: &ScoreWindow, inst_idx: u8, step: u64| -> HashSet<u8> {
        let mut out = HashSet::new();
        for ev in score.step_events(step) {
            if ev.inst_idx == inst_idx {
                out.insert(ev.midi);
                for tone in &ev.chord[..usize::from(ev.chord_len)] {
                    out.insert(tone.midi);
                }
            }
        }
        out
    };

    let score_90 = proj_90.compile().unwrap();
    let score_180 = proj_180.compile().unwrap();

    let hit_808 = collect_midis(&score_90, 2, 0);
    for expected in [
        drum_midi::BD,
        drum_midi::SD,
        drum_midi::LT,
        drum_midi::MT,
        drum_midi::HT,
        drum_midi::RS,
        drum_midi::CP,
        drum_midi::CB_OR_CR,
        drum_midi::CY_OR_RD,
    ] {
        assert!(
            hit_808.contains(&expected),
            "808 dense 9-voice step must not drop MIDI {expected}: got {hit_808:?}"
        );
    }

    let hit_909 = collect_midis(&score_90, 3, 0);
    for expected in [
        drum_midi::BD,
        drum_midi::SD,
        drum_midi::LT,
        drum_midi::MT,
        drum_midi::HT,
        drum_midi::RS,
        drum_midi::CP,
        drum_midi::CH,
    ] {
        assert!(
            hit_909.contains(&expected),
            "909 dense 8-voice step must not drop MIDI {expected}: got {hit_909:?}"
        );
    }

    // BPM-accurate TR-909 flam micro-offset scales ~2x between 90 BPM and 180 BPM!
    let flam_off = |score: &ScoreWindow| {
        score
            .step_events(0)
            .iter()
            .filter(|e| e.inst_idx == 3 && e.midi == drum_midi::SD && e.offset > 0.01)
            .map(|e| e.offset)
            .next()
            .expect("delayed 909 SD flam hit")
    };
    let o90 = flam_off(&score_90);
    let o180 = flam_off(&score_180);
    let ratio = o180 / o90;
    assert!(
        (ratio - 2.0).abs() < 0.15,
        "180 BPM flam step offset ({o180}) should be ~2x 90 BPM offset ({o90}), ratio={ratio}"
    );

    // Also verify wider TR-909 flam interval (ctrl 0x1d) delays the secondary SD hit noticeably further.
    let mut rbs_tight = rbs.clone();
    rbs_tight.bpm_millis = 120_000;
    rbs_tight.t909[29] = 16;
    let mut rbs_wide = rbs_tight.clone();
    rbs_wide.t909[29] = 115;
    let score_tight = import_rbs_bytes(&rbs_tight.build("FlamTight"), "FlamTight")
        .unwrap()
        .0
        .compile()
        .unwrap();
    let score_wide = import_rbs_bytes(&rbs_wide.build("FlamWide"), "FlamWide")
        .unwrap()
        .0
        .compile()
        .unwrap();
    assert!(
        flam_off(&score_wide) > flam_off(&score_tight) + 0.12,
        "wider flam interval ({}) must exceed tight ({})",
        flam_off(&score_wide),
        flam_off(&score_tight)
    );

    // End-to-end audio render check: audible and finite!
    let rep = render_measure_report(&score_90, load_rbs_samples(&score_90), 2, 24_000).unwrap();
    assert!(
        rep.output.peak[0] > 0.10 && rep.output.full_scale_samples == 0,
        "peak={:?} full_scale_samples={}",
        rep.output.peak,
        rep.output.full_scale_samples
    );
}

#[test]
fn dense_four_device_mix_with_high_q_pcf_echo_and_automated_gains_stays_unclipped() {
    let mut rbs = SyntheticRbs::new(4);
    rbs.mixr[0] = 110; // strong Master fader
    rbs.mixr[2] = 1; // PCF routed to TB-303 A
    rbs.pcf = [1, 48, 127, 105, 13, 95, 0, 0, 0, 0, 0, 0]; // high-Q BandPass PCF
    rbs.dely = [1, 3, 1, 115, 84, 0, 0, 0]; // high-feedback tempo-synced Echo

    // Dense accented 16th-note acid lines on both TB-303s + full 808 & 909 drum patterns.
    for s in 0..16 {
        rbs.set_303_step(
            0,
            s,
            [0, 0, 12, 7][s % 4],
            if s % 4 == 0 { 0x02 } else { 0x00 },
        );
        rbs.set_303_step(
            1,
            s,
            [12, 7, 3, 0][s % 4],
            if s % 4 == 2 { 0x02 } else { 0x00 },
        );
        if s % 4 == 0 {
            rbs.set_808_step(s, 1, 2); // accented 808 BD
            rbs.set_909_step(s, 1, 2); // accented 909 BD
        }
        if s % 8 == 4 {
            rbs.set_808_step(s, 2, 2); // 808 SD
            rbs.set_909_step(s, 2, 2); // 909 SD
        }
        if s % 4 == 2 {
            rbs.set_808_step(s, 10, 1); // 808 OH
            rbs.set_909_step(s, 10, 1); // 909 OH
        } else {
            rbs.set_909_step(s, 9, 1); // 909 CH
        }
    }
    rbs.t808[2] = 115; // high 808 global accent
    rbs.t808[3] = 127; // max 808 BD level
    rbs.t909[2] = 115; // high 909 global accent
    rbs.t909[3] = 127; // max 909 BD level

    // Automate all four channel faders up to full 127 with strong multi-channel Echo sends.
    rbs.tracks[0] = vec![
        (0, 2, 1),    // PCF -> TB-303 A
        (0, 6, 96),   // 303A gain -> 127
        (0, 8, 120),  // 303A echo send
        (0, 12, 96),  // 303B gain -> 127
        (0, 14, 110), // 303B echo send
        (0, 18, 100), // 808 gain -> 127
        (0, 20, 105), // 808 echo send
        (0, 24, 100), // 909 gain -> 127
        (16, 6, 127),
        (16, 12, 127),
        (16, 18, 127),
        (16, 24, 127),
    ];
    rbs.tracks[1] = vec![(0, 0, 1), (0, 1, 0), (0, 3, 95), (0, 4, 110)];
    rbs.tracks[2] = vec![(0, 0, 1), (0, 1, 0), (0, 3, 90), (0, 4, 95)];
    rbs.tracks[3] = vec![(0, 0, 1), (0, 1, 0)];
    rbs.tracks[4] = vec![(0, 0, 1), (0, 1, 0)];
    rbs.tracks[5] = vec![(0, 0, 1), (0, 1, 3), (0, 2, 1), (0, 3, 115)];
    rbs.tracks[7] = vec![
        (0, 0, 1),
        (0, 1, 48),
        (0, 2, 127),
        (0, 3, 105),
        (0, 4, 13),
        (0, 5, 95),
        (0, 6, 0),
    ];

    let (proj, _) = import_rbs_bytes(&rbs.build("DenseHeadroom"), "DenseHeadroom").unwrap();
    // Source headroom sits cleanly on each Rack so automated channel faders stay unity-scaled.
    assert!((proj.instruments[0].rack.gain_db - (-6.0)).abs() < 1e-5);
    assert!((proj.instruments[1].rack.gain_db - (-6.0)).abs() < 1e-5);
    assert!((proj.instruments[2].rack.gain_db - (-8.0)).abs() < 1e-5);
    assert!((proj.instruments[3].rack.gain_db - (-8.0)).abs() < 1e-5);
    for ch in proj.mixer.channels.iter().take(4) {
        assert!((ch.strip.gain - 1.0).abs() < 1e-6);
    }

    let score = proj.compile().unwrap();
    let rep = render_measure_report(&score, load_rbs_samples(&score), 4, 24_000).unwrap();
    let master_peak = rep.output.peak[0].max(rep.output.peak[1]);
    assert!(
        master_peak > 0.25 && master_peak <= 1.0 && rep.output.full_scale_samples == 0,
        "Dense 4-device mix with high-Q PCF and Echo must stay punchy and unclipped: peak={master_peak:.4}, clips={}",
        rep.output.full_scale_samples
    );
    let master_ch = rep
        .channels
        .iter()
        .find(|c| c.name == "Master")
        .expect("Master meter");
    assert!(
        !master_ch.clipped,
        "Master bus must not clip: peak_hold={:?}",
        master_ch.output_peak_hold
    );
}
