use super::parse_iff::parse_single_trak;
use super::parse_smf::map_1x_cc;
use super::*;
use cadence_core::material::PatternKind;
use cadence_core::mixer::EffectKind;

fn chunk(tag: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + payload.len() + 1);
    out.extend_from_slice(tag);
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    if payload.len() % 2 == 1 {
        out.push(0);
    }
    out
}

fn cat(kind: &[u8; 4], children: &[Vec<u8>]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(kind);
    for c in children {
        body.extend_from_slice(c);
    }
    chunk(b"CAT ", &body)
}

fn build_synthetic_rbs_42() -> Vec<u8> {
    // HEAD (256)
    let mut head = vec![0u8; 256];
    head[0..9].copy_from_slice(&[0x5B, 0x54, 0x5B, 0x54, 0xBC, 0x04, 0x02, 0x00, 0x00]);

    // GLOB (512): song_mode=1, loop_on=1, 136.0 BPM, loop 0..4 bars, shuffle=48
    let mut glob = vec![0u8; 512];
    glob[0] = 1;
    glob[1] = 1;
    glob[2..6].copy_from_slice(&136_000u32.to_be_bytes());
    glob[6..10].copy_from_slice(&0u32.to_be_bytes());
    glob[10..14].copy_from_slice(&(4 * TICKS_PER_BAR).to_be_bytes());
    glob[14] = 48;
    glob[15..31].copy_from_slice(b"Standard ReBirth");

    // USRI (712)
    let mut usri = vec![0u8; 712];
    usri[0..14].copy_from_slice(b"Acid Test 1998");

    // MIXR (64): PCF on 303#1 (id=1), Comp on Master (id=1), Dist on 303#1
    let mut mixr = vec![0u8; 64];
    mixr[0] = 110;
    mixr[1] = 1; // master comp
    mixr[2] = 1; // PCF on 303#1
    for c in 0..4 {
        let b = 16 + 12 * c;
        mixr[b] = 1;
        mixr[b + 1] = 105;
        mixr[b + 2] = 64;
        mixr[b + 3] = if c == 0 { 60 } else { 0 };
        mixr[b + 4] = u8::from(c == 0);
    }

    let dely = [1u8, 3, 1, 70, 64, 0, 0, 0];
    let pcf = [1u8, 55, 85, 75, 2, 70, 1, 0, 0, 0, 0, 0];
    let dist = [1u8, 80, 20, 0, 0, 0, 0, 0];
    let comp = [1u8, 70, 50, 0, 0, 0, 0, 0];

    // TB-303 #1 (1097 bytes) with an acid bassline on pattern 0 (shuffle on!)
    let mut tb1 = vec![0u8; 1097];
    tb1[0..9].copy_from_slice(&[1, 0, 64, 48, 105, 95, 75, 110, 0]);
    tb1[9] = 1; // shuffle ON
    tb1[10] = 16; // length 16
    for s in 0..16 {
        let off = 11 + s * 2;
        tb1[off] = [0, 0, 12, 0, 3, 0, 7, 10, 0, 0, 12, 0, 5, 7, 3, 0][s];
        // bit 0=slide, bit 1=accent, bit 4=rest
        tb1[off + 1] = if s % 4 == 0 {
            0x02 // accent
        } else if s % 4 == 2 {
            0x01 // slide
        } else {
            0x00
        };
    }
    // TB-303 #2 silent
    let mut tb2 = vec![0u8; 1097];
    tb2[10] = 16;
    for s in 0..16 {
        tb2[11 + s * 2 + 1] = 0x10; // rest
    }

    // TR-808 (6238 bytes) 4-on-the-floor kick + offbeat open hat + closed hat
    let mut t808 = vec![0u8; 6238];
    t808[0] = 1;
    t808[3] = 115; // BD level
    t808[6] = 100; // SD
    t808[26] = 95; // OH
    t808[28] = 90; // CH
    t808[31] = 16; // pat 0 len
    for s in 0..16 {
        let off = 32 + s * 12;
        if s % 4 == 0 {
            t808[off + 1] = 1; // BD
        }
        if s % 8 == 4 {
            t808[off + 2] = 1; // SD
        }
        if s % 4 == 2 {
            t808[off + 10] = 1; // OH
        } else {
            t808[off + 11] = 1; // CH
        }
    }

    // TR-909 (6239 bytes)
    let mut t909 = vec![0u8; 6239];
    t909[0] = 0; // disabled
    t909[32] = 16;

    // TRAK chunks (9 tracks): automate 303#1 cutoff sweep!
    let make_trak = |events: &[(u32, u8, u8)]| -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&(events.len() as u32).to_be_bytes());
        let mut prev = 0u32;
        for &(pos, ctrl, val) in events {
            let d = pos - prev;
            prev = pos;
            // encode VLQ
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
        chunk(b"TRAK", &b)
    };

    let trkl = cat(
        b"TRKL",
        &[
            make_trak(&[(0, 1, 1), (0, 2, 1), (0, 6, 110), (0, 9, 1)]),
            make_trak(&[
                (0, 0, 1),
                (0, 1, 0),
                (0, 3, 30),
                (32, 3, 75),
                (64, 3, 120),
                (96, 3, 45),
            ]),
            make_trak(&[(0, 0, 0)]),
            make_trak(&[(0, 0, 1), (0, 1, 0)]),
            make_trak(&[(0, 0, 0)]),
            make_trak(&[(0, 0, 1)]),
            make_trak(&[(0, 0, 1), (0, 1, 80)]),
            make_trak(&[(0, 0, 1), (0, 1, 55), (0, 3, 75), (0, 4, 2)]),
            make_trak(&[(0, 0, 1)]),
        ],
    );

    let devl = cat(
        b"DEVL",
        &[
            chunk(b"MIXR", &mixr),
            chunk(b"DELY", &dely),
            chunk(b"PCF ", &pcf),
            chunk(b"DIST", &dist),
            chunk(b"COMP", &comp),
            chunk(b"303 ", &tb1),
            chunk(b"303 ", &tb2),
            chunk(b"808 ", &t808),
            chunk(b"909 ", &t909),
        ],
    );

    cat(
        b"RB40",
        &[
            chunk(b"HEAD", &head),
            chunk(b"GLOB", &glob),
            chunk(b"USRI", &usri),
            devl,
            trkl,
        ],
    )
}

fn assert_tb303_cubic_rack(
    rack: &cadence_core::rack::RackPatch,
    expected_wf: cadence_core::rack::Waveform,
) {
    use cadence_core::rack::{EnvMode, FilterMode, ModuleKind, TriggerMode};

    let targets = calibration::resolve_tb303_targets(rack).expect("TB-303 targets");
    let zone = &rack.zones[0];
    let vco = zone
        .modules
        .iter()
        .find_map(|m| match &m.kind {
            ModuleKind::Vco(v) if m.id == targets.vco_id => Some(v),
            _ => None,
        })
        .expect("TB-303 VCO");
    assert_eq!(vco.waveform, expected_wf);
    assert!((vco.reach - 0.94).abs() < 1e-6);
    assert!((vco.shape - 0.50).abs() < 1e-6);
    assert!((vco.pulse_width - 0.50).abs() < 1e-6);
    assert!((vco.level - 1.0).abs() < 1e-6);
    assert!(zone.modules.iter().any(|m| {
        matches!(
            m.kind,
            ModuleKind::Filter(f)
                if m.id == targets.filter_id && f.mode == FilterMode::DiodeLadder
        )
    }));
    let filter_env = zone
        .modules
        .iter()
        .find_map(|m| match &m.kind {
            ModuleKind::Env(e) if m.id == targets.filter_env_id => Some(e),
            _ => None,
        })
        .expect("TB-303 Filter Env");
    assert_eq!(filter_env.mode, EnvMode::Capacitor);
    assert!((filter_env.attack_ms - 2.2).abs() < 1e-6);
    assert!((filter_env.hold_ms - 4.0).abs() < 1e-6);
    assert!((filter_env.cubic_limit - 0.45).abs() < 1e-6);
    assert_eq!(filter_env.trigger_mode, TriggerMode::OneShot);
}

#[test]
fn synthetic_rbs_42_parses_converts_validates_and_compiles() {
    let bytes = build_synthetic_rbs_42();
    let (project, summary) = import_rbs_bytes(&bytes, "fallback").unwrap();
    assert_eq!(project.title, "Acid Test 1998");
    assert!((project.bpm - 136.0).abs() < 1e-6);
    assert_eq!(project.bars, 4);
    assert_eq!(summary.version.major, 4);
    assert_eq!(summary.version.minor, 2);
    assert_eq!(project.instruments.len(), 4);
    assert_eq!(project.lanes.len(), 4);
    // Shuffle should have wrapped 303A A1 in a derived Swing pattern!
    assert!(project
        .patterns
        .iter()
        .any(|p| matches!(p.kind, PatternKind::Swing { .. })));
    // Cutoff automation & native rhythmic PCF Filter patch!
    assert!(project
        .modulations
        .iter()
        .any(|m| m.name.contains("303A Cutoff")));
    let pcf_dev = project.mixer.channels[0]
        .inserts
        .iter()
        .find(|d| d.name == "PCF")
        .expect("PCF insert on TB-303 A");
    match pcf_dev.kind {
        EffectKind::Filter(p) => {
            assert_eq!(p.mode, cadence_core::rack::FilterMode::ImLpf2);
            assert_eq!(p.env_pattern, 2);
            assert!(p.env_amount > 1.5);
            assert!(p.env_decay_ms > 100.0);
        }
        _ => panic!("expected native Filter PCF device"),
    }
    let dist_dev = project.mixer.channels[0]
        .inserts
        .iter()
        .find(|d| d.name == "Dist")
        .expect("Dist insert on TB-303 A");
    assert!(matches!(
        dist_dev.kind,
        EffectKind::Drive(p) if p.mode == cadence_core::mixer::DriveMode::ShockPort
    ));
    assert_tb303_cubic_rack(
        &project.instruments[0].rack,
        cadence_core::rack::Waveform::MobiusJet,
    );
    // Must compile cleanly into a valid executable ScoreWindow!
    let score = project.compile().unwrap();
    assert_eq!(score.bars(), 4);
    assert!(!score.events.is_empty());
}

#[test]
fn synthetic_rebirth_1x_smf_parses_and_compiles() {
    // Build a minimal ReBirth 1.x Standard MIDI File (format 1) containing
    // a 7-bit-packed 0x4000-byte "TinY" SysEx block and track 2/4 CCs.
    let mut raw = vec![0u8; 0x4000];
    raw[0..4].copy_from_slice(&[0xBC, 0x03, 0x01, 0x00]);
    raw[5..9].copy_from_slice(b"TinY");
    raw[9] = 110; // master level
    raw[10..14].copy_from_slice(&0u32.to_be_bytes()); // start bar 0
    raw[14..18].copy_from_slice(&2u32.to_be_bytes()); // 2 bars
    raw[0x12] = 1; // song mode
    raw[0x2A..0x2E].copy_from_slice(&132_000u32.to_be_bytes()); // 132 BPM
    raw[0x2E] = 1; // loop on
    raw[0x2F] = 0x83; // delay: 3 steps, 16ths mode (bit 7)
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
    // Pattern 0 step 0..15: notes on steps 0, 4, 8, 12; rests elsewhere
    for s in 0..16 {
        let so = 352 + 30 + 4 * s;
        raw[so] = 0;
        raw[so + 2] = if s % 8 == 0 { 0x02 } else { 0x00 };
        raw[so + 3] = u8::from(s % 4 != 0); // rest
    }
    // 808 record at 0x140
    raw[0x140 + 2] = 110;
    raw[0x140 + 3] = 64;
    raw[0x140 + 7] = 0xFF;
    raw[0x140 + 0x17] = 1;
    raw[0x3160..0x3160 + 11].copy_from_slice(b"Retro 1x RB");

    // Pack 0x4000 bytes in 7-to-8 groups
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

    // Track 0 (version meta)
    let t0 = make_mtrk(&[0x00, 0xFF, 0x2F, 0x00]);
    // Track 1 (SysEx)
    let mut t1_body = vec![0x00, 0xF0];
    t1_body.extend_from_slice(&encode_vlq(sysex.len()));
    t1_body.extend_from_slice(&sysex);
    t1_body.extend_from_slice(&[0x00, 0xFF, 0x2F, 0x00]);
    let t1 = make_mtrk(&t1_body);
    // Track 2 (303#1 enable=1, pat=0, cutoff sweep)
    let t2 = make_mtrk(&[
        0x00, 0xB0, 15, 1, // enabled
        0x00, 0xB0, 14, 0, // pat 0
        0x00, 0xB0, 17, 30, // cutoff 30
        32, 0xB0, 17, 110, // bar 1 cutoff 110
        0x00, 0xFF, 0x2F, 0x00,
    ]);

    let mut smf = Vec::new();
    smf.extend_from_slice(b"MThd");
    smf.extend_from_slice(&6u32.to_be_bytes());
    smf.extend_from_slice(&1u16.to_be_bytes()); // format 1
    smf.extend_from_slice(&3u16.to_be_bytes()); // 3 tracks
    smf.extend_from_slice(&32u16.to_be_bytes()); // division
    smf.extend_from_slice(&t0);
    smf.extend_from_slice(&t1);
    smf.extend_from_slice(&t2);

    let (project, summary) = import_rbs_bytes(&smf, "fallback").unwrap();
    assert_eq!(project.title, "Retro 1x RB");
    assert!((project.bpm - 132.0).abs() < 1e-6);
    assert_eq!(summary.version.major, 3);
    assert_eq!(project.bars, 2);
    // ReBirth 1.x has no TR-909 -> exactly 3 instruments, 3 lanes, 6 mixer channels!
    assert_eq!(project.instruments.len(), 3);
    assert!(!project.instruments.iter().any(|i| i.name == "TR-909"));
    assert_eq!(project.lanes.len(), 3);
    assert_eq!(project.mixer.channels.len(), 6);
    assert!(project.macros.is_empty());
    assert!(summary.status_line().contains("ReBirth v3.1"));
    assert!(summary.status_line().contains("Retro 1x RB"));
    let preview = peek_rbs_header_bytes(&smf, "fallback").unwrap();
    assert_eq!(preview.title, project.title);
    assert_eq!(preview.bars, project.bars);
    assert_eq!(preview.version, summary.version);
    assert!((preview.bpm - project.bpm).abs() < 1e-6);
    let score = project.compile().unwrap();
    assert!(!score.events.is_empty());
}

#[test]
fn iff_trak_events_are_stable_sorted_by_pos() {
    // Construct a TRAK where delta wraps around u32::MAX -> pos 100 then pos 0!
    let mut payload = Vec::new();
    payload.extend_from_slice(&3u32.to_be_bytes());
    // Event 0: delta 100 -> pos 100, ctrl 3, val 99
    payload.extend_from_slice(&[100, 3, 99]);
    // Event 1: delta 0xFFFFFF9C (wraps 100 + delta == 0!) -> pos 0, ctrl 3, val 42
    // VLQ for 0xFFFFFF9C: 5 bytes
    let d: u32 = 0u32.wrapping_sub(100);
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
        payload.push(stack[i]);
    }
    payload.extend_from_slice(&[3, 42]);
    // Event 2: delta 0 -> pos 0, ctrl 3, val 55 (same pos as Event 1 -> stable sort preserves 42 then 55)
    payload.extend_from_slice(&[0, 3, 55]);

    let events = parse_single_trak(&payload, 1, false, false).unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(
        events[0],
        TrackEvent {
            pos: 0,
            ctrl: 3,
            value: 42
        }
    );
    assert_eq!(
        events[1],
        TrackEvent {
            pos: 0,
            ctrl: 3,
            value: 55
        }
    );
    assert_eq!(
        events[2],
        TrackEvent {
            pos: 100,
            ctrl: 3,
            value: 99
        }
    );
}

#[test]
fn v1x_808_tom_tune_cc_matches_official_rebirth_1x_loader() {
    let mut song = RbsSong::default();
    // MIDI track 4 = TR-808; CC 24 -> ctrl 10 (LT tune), CC 27 -> ctrl 13 (MT), CC 30 -> ctrl 16 (HT).
    // Track 4 TR-808 tom tuning CCs map signed offsets into `[0, 127]` via `(val as i8) / 2 + 64`,
    // matching `decode_1x_sysex` initial tom tuning.
    map_1x_cc(4, 0, 24, 0, &mut song);
    map_1x_cc(4, 2, 24, 64, &mut song);
    map_1x_cc(4, 4, 24, 127, &mut song);
    let vals: Vec<u8> = song.tracks[3].iter().map(|e| e.value).collect();
    assert_eq!(vals, vec![64, 96, 127]);
}

#[test]
fn rbs_title_sanitization_channel_gain_section_fade_comp_makeup_and_32_step_delay() {
    use cadence_core::material::{
        Modulation, ModulationTarget, Pattern, PatternKind, ValuePoint, ValueUnit,
    };
    use cadence_core::mixer::{Channel, ChannelKind, ChannelRef};
    use cadence_core::params::ValueMode;

    // 1. Title sanitization strips control bytes / escapes cleanly.
    assert_eq!(
        sanitize_title("Acid\x07\x1b[31m\nLine\r\tMix", "fallback"),
        "Acid[31mLineMix"
    );
    assert_eq!(sanitize_title_opt("\x00\x07\n"), "");
    assert_eq!(
        sanitize_title("\x00\x07\n", "Fallback Title"),
        "Fallback Title"
    );

    // 2. DIST output trim scales smoothly from 0 dB at drive=0 down to -2.5 dB at full ShockPort
    //    density so engaging ReBirth DIST preserves clean-vs-distorted mix balance.
    assert_eq!(calibration::rbs_dist_output_db(0.0), 0.0);
    assert!(calibration::rbs_dist_output_db(0.7) < -1.5);
    assert!(calibration::rbs_dist_output_db(1.0) <= -2.5);
    assert!(calibration::rbs_dist_tone_hz(0) >= 9_000.0);

    // 3. Delay steps map cleanly up to 32 steps (mode 1 = straight steps, mode 0 = dotted 4/3x).
    assert!((calibration::rbs_delay_steps(24, 1) - 24.0).abs() < 1e-4);
    assert!((calibration::rbs_delay_steps(32, 1) - 32.0).abs() < 1e-4);
    assert!((calibration::rbs_delay_steps(24, 0) - 32.0).abs() < 1e-4);

    // 4. detect_musical_sections accounts for ChannelGain fade-to-0 silence lanes!
    let mut song = RbsSong::default();
    song.mixer.channels[0].enabled = true;
    song.mixer.channels[1].enabled = true;
    let mut ch1 = Channel::new(1, ChannelKind::Audio, "TB-303 #1");
    ch1.strip.gain = 0.85;
    let mut ch2 = Channel::new(2, ChannelKind::Audio, "TB-303 #2");
    ch2.strip.gain = 0.85;
    let channels = vec![ch1, ch2];
    let patterns = vec![Pattern {
        id: 11,
        name: "FadeToZero".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 128,
            mode: ValueMode::Hold,
            points: vec![
                ValuePoint {
                    step: 0,
                    value: 1.0,
                },
                ValuePoint {
                    step: 64,
                    value: 0.0,
                },
            ],
        },
    }];
    let modulations = vec![Modulation {
        id: 50,
        name: "Lead Gain".into(),
        target: ModulationTarget::ChannelGain {
            channel: ChannelRef::Channel(1),
        },
        pattern: 11,
    }];
    let bar_states: [Vec<Option<(usize, u8)>>; 4] = [
        vec![Some((0, 0)); 8],
        vec![Some((0, 0)); 8],
        vec![None; 8],
        vec![None; 8],
    ];
    let pat_non_empty: [Vec<bool>; 4] = [vec![true], vec![true], vec![false], vec![false]];
    let mut next_id = 100u64;
    let mut fresh = || {
        let id = next_id;
        next_id += 1;
        id
    };
    let sections = song.detect_musical_sections(
        8,
        2,
        &bar_states,
        &pat_non_empty,
        &channels,
        &modulations,
        &patterns,
        &mut fresh,
    );
    assert!(
        sections.len() >= 2 && sections.iter().any(|s| s.start_step == 64),
        "ChannelGain fade-to-0 at bar 4 (step 64) must create a section boundary at step 64: {sections:?}"
    );
}

#[test]
fn per_zone_drum_ccs_automate_both_sample_and_synth_kits_with_calibrated_values() {
    use cadence_core::material::ModulationTarget;
    use cadence_core::params::{InstrumentParam, RackParam, ZoneParam};
    use cadence_core::presets::{InstrumentPreset, PresetCatalog};
    use cadence_core::rack::{
        drum_midi, InPort, ModuleKind, ModuleParam, NoiseModule, PatchCable, RackModule, RackPatch,
        RackZone,
    };

    let build_synth_drum_zone = |id: u16, name: &str, midi: u8, choke: Option<u8>| -> RackZone {
        let mut z = RackPatch::default_synth().zones.remove(0);
        z.id = id;
        z.name = name.into();
        z.low_midi = midi;
        z.high_midi = midi;
        z.set_root_midi(midi);
        z.choke_group = choke;
        // Add a Noise source patched into the Filter so Snappy/Attack map cleanly to Noise.Level.
        let flt_id = z
            .modules
            .iter()
            .find(|m| matches!(m.kind, ModuleKind::Filter(_)))
            .unwrap()
            .id;
        let noise_id = z.next_module_id();
        z.modules.push(RackModule::new(
            noise_id,
            "Noise",
            ModuleKind::Noise(NoiseModule::default()),
        ));
        let cid = z.next_cable_id();
        z.cables
            .push(PatchCable::new(cid, noise_id, flt_id, InPort::In, 1.0));
        z
    };

    let build_kit = |kit_name: &str, use_synth: bool| -> RackPatch {
        let pads: [(&str, u8, Option<u8>); 11] = [
            ("BD", drum_midi::BD, None),
            ("SD", drum_midi::SD, None),
            ("LT", drum_midi::LT, None),
            ("MT", drum_midi::MT, None),
            ("HT", drum_midi::HT, None),
            ("RS", drum_midi::RS, None),
            ("CH", drum_midi::CH, Some(1)),
            ("CP", drum_midi::CP, None),
            ("CB", drum_midi::CB_OR_CR, None),
            ("CY", drum_midi::CY_OR_RD, None),
            ("OH", drum_midi::OH, Some(1)),
        ];
        let zones = pads
            .into_iter()
            .enumerate()
            .map(|(i, (name, midi, choke))| {
                let zid = (i + 1) as u16;
                if use_synth {
                    build_synth_drum_zone(zid, name, midi, choke)
                } else {
                    let mut z =
                        RackZone::sample_pad(zid, name, midi, format!("synthetic/{name}.wav"));
                    z.choke_group = choke;
                    z
                }
            })
            .collect();
        RackPatch {
            kit_name: kit_name.into(),
            zones,
            ..RackPatch::default_kit()
        }
    };

    let build_catalog = |dir: &std::path::Path, use_synth: bool| -> PresetCatalog {
        let kits_dir = dir.join("kits");
        let drums_dir = dir.join("drums");
        let synth_dir = dir.join("synth");
        std::fs::create_dir_all(&kits_dir).unwrap();
        std::fs::create_dir_all(&drums_dir).unwrap();
        std::fs::create_dir_all(&synth_dir).unwrap();

        InstrumentPreset::new("Acid 303", Some("Bass".into()), RackPatch::acid_303())
            .save(&synth_dir.join("acid-303.json"))
            .unwrap();
        InstrumentPreset::new("TR-808", Some("Kit".into()), build_kit("TR-808", use_synth))
            .save(&kits_dir.join("tr-808.json"))
            .unwrap();
        InstrumentPreset::new("TR-909", Some("Kit".into()), build_kit("TR-909", use_synth))
            .save(&kits_dir.join("tr-909.json"))
            .unwrap();
        // Dynamic / alt 808 voice presets matching the kit's source kind.
        for (slug, label) in [
            ("tr-808-lc", "LC"),
            ("tr-808-mc", "MC"),
            ("tr-808-hc", "HC"),
            ("tr-808-cl", "CL"),
            ("tr-808-ma", "MA"),
        ] {
            let alt_patch = if use_synth {
                RackPatch {
                    kit_name: String::new(),
                    zones: vec![build_synth_drum_zone(1, label, 60, None)],
                    panel_pins: Vec::new(),
                    ..RackPatch::default_synth()
                }
            } else {
                RackPatch {
                    kit_name: String::new(),
                    zones: vec![RackZone::sample_pad(
                        1,
                        label,
                        60,
                        format!("synthetic/{label}.wav"),
                    )],
                    panel_pins: Vec::new(),
                    ..RackPatch::default_sampler()
                }
            };
            InstrumentPreset::new(format!("TR-808 {label}"), Some("Drum".into()), alt_patch)
                .save(&drums_dir.join(format!("{slug}.json")))
                .unwrap();
        }
        PresetCatalog::from_roots(&[dir.to_path_buf()])
    };

    // Build a synthetic Song Mode RbsSong with rich 808 & 909 drum CC sweeps, including:
    // - 808: BD level(3), BD tone(4), BD decay(5, starts at 127 then shortens to 20!),
    //        SD tone(7), SD snappy(8), LT/LC level(9) + tune(10) + dynamic selector(11),
    //        CY tone(24), OH decay(27)
    // - 909: BD level(3), BD tune(4), BD attack(5), BD decay(6),
    //        SD tune(8), SD tone(9), SD snappy(10),
    //        HH shared level(22 -> both CH & OH!), CH decay(23), OH decay(24), CR tune(26)
    let mut song = RbsSong {
        song_mode: true,
        loop_on: true,
        loop_start_bar: 0,
        loop_end_bar: 4,
        ..RbsSong::default()
    };
    song.tr808.params[0] = 1;
    song.tr808.params[5] = 127; // BD decay starts wide open -> Sample sustain starts at 1.0!
    song.tr808.params[11] = 1; // LT primary initially
    song.tr808.patterns[0].steps[0][1] = 1; // BD
    song.tr808.patterns[0].steps[4][2] = 1; // SD
    song.tr808.patterns[0].steps[8][3] = 1; // LT/LC
    song.tr808.patterns[0].steps[12][10] = 1; // OH

    song.tr909.params[0] = 1;
    song.tr909.patterns[0].steps[0][1] = 1; // BD
    song.tr909.patterns[0].steps[4][2] = 1; // SD
    song.tr909.patterns[0].steps[8][8] = 1; // CH
    song.tr909.patterns[0].steps[12][9] = 1; // OH

    song.tracks[3] = vec![
        TrackEvent {
            pos: 0,
            ctrl: 0,
            value: 1,
        },
        TrackEvent {
            pos: 0,
            ctrl: 1,
            value: 0,
        },
        TrackEvent {
            pos: 0,
            ctrl: 3,
            value: 115,
        }, // BD level
        TrackEvent {
            pos: 0,
            ctrl: 4,
            value: 25,
        }, // BD tone
        TrackEvent {
            pos: 0,
            ctrl: 5,
            value: 127,
        }, // BD decay (max at bar 0)
        TrackEvent {
            pos: 0,
            ctrl: 7,
            value: 30,
        }, // SD tone
        TrackEvent {
            pos: 0,
            ctrl: 8,
            value: 20,
        }, // SD snappy
        TrackEvent {
            pos: 0,
            ctrl: 9,
            value: 100,
        }, // LT/LC level
        TrackEvent {
            pos: 0,
            ctrl: 10,
            value: 40,
        }, // LT/LC tune (-1.5 st)
        TrackEvent {
            pos: 0,
            ctrl: 11,
            value: 1,
        }, // LT primary
        TrackEvent {
            pos: 0,
            ctrl: 24,
            value: 20,
        }, // CY tone
        TrackEvent {
            pos: 0,
            ctrl: 27,
            value: 25,
        }, // OH decay
        TrackEvent {
            pos: 64,
            ctrl: 3,
            value: 45,
        }, // bar 2 moves
        TrackEvent {
            pos: 64,
            ctrl: 4,
            value: 120,
        },
        TrackEvent {
            pos: 64,
            ctrl: 5,
            value: 20,
        }, // BD decay drops below 0.72!
        TrackEvent {
            pos: 64,
            ctrl: 7,
            value: 118,
        },
        TrackEvent {
            pos: 64,
            ctrl: 8,
            value: 120,
        },
        TrackEvent {
            pos: 64,
            ctrl: 9,
            value: 50,
        },
        TrackEvent {
            pos: 64,
            ctrl: 10,
            value: 112,
        }, // +3.0 st
        TrackEvent {
            pos: 64,
            ctrl: 11,
            value: 0,
        }, // dynamic switch to LC!
        TrackEvent {
            pos: 64,
            ctrl: 24,
            value: 115,
        },
        TrackEvent {
            pos: 64,
            ctrl: 27,
            value: 120,
        },
    ];
    song.tracks[4] = vec![
        TrackEvent {
            pos: 0,
            ctrl: 0,
            value: 1,
        },
        TrackEvent {
            pos: 0,
            ctrl: 1,
            value: 0,
        },
        TrackEvent {
            pos: 0,
            ctrl: 3,
            value: 120,
        }, // BD level
        TrackEvent {
            pos: 0,
            ctrl: 4,
            value: 32,
        }, // BD tune (-3.0 st)
        TrackEvent {
            pos: 0,
            ctrl: 5,
            value: 20,
        }, // BD attack
        TrackEvent {
            pos: 0,
            ctrl: 6,
            value: 30,
        }, // BD decay
        TrackEvent {
            pos: 0,
            ctrl: 8,
            value: 24,
        }, // SD tune
        TrackEvent {
            pos: 0,
            ctrl: 9,
            value: 25,
        }, // SD tone
        TrackEvent {
            pos: 0,
            ctrl: 10,
            value: 20,
        }, // SD snappy
        TrackEvent {
            pos: 0,
            ctrl: 22,
            value: 110,
        }, // shared HH level (CH + OH)
        TrackEvent {
            pos: 0,
            ctrl: 23,
            value: 20,
        }, // CH decay
        TrackEvent {
            pos: 0,
            ctrl: 24,
            value: 30,
        }, // OH decay
        TrackEvent {
            pos: 0,
            ctrl: 26,
            value: 40,
        }, // CR tune
        TrackEvent {
            pos: 64,
            ctrl: 3,
            value: 50,
        },
        TrackEvent {
            pos: 64,
            ctrl: 4,
            value: 112,
        }, // +4.5 st
        TrackEvent {
            pos: 64,
            ctrl: 5,
            value: 120,
        },
        TrackEvent {
            pos: 64,
            ctrl: 6,
            value: 120,
        },
        TrackEvent {
            pos: 64,
            ctrl: 8,
            value: 112,
        },
        TrackEvent {
            pos: 64,
            ctrl: 9,
            value: 120,
        },
        TrackEvent {
            pos: 64,
            ctrl: 10,
            value: 120,
        },
        TrackEvent {
            pos: 64,
            ctrl: 22,
            value: 40,
        },
        TrackEvent {
            pos: 64,
            ctrl: 23,
            value: 115,
        },
        TrackEvent {
            pos: 64,
            ctrl: 24,
            value: 120,
        },
        TrackEvent {
            pos: 64,
            ctrl: 26,
            value: 104,
        },
    ];

    for use_synth in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let catalog = build_catalog(dir.path(), use_synth);
        let (proj, notices) = song
            .to_cadence_project_with_presets("DrumPerZone", &catalog)
            .unwrap();
        assert!(notices.is_empty(), "unexpected preset notices: {notices:?}");

        let inst808 = &proj.instruments[2];
        let inst909 = &proj.instruments[3];
        let kit808 = &inst808.rack;
        let kit909 = &inst909.rack;

        // Never emit kit-wide RackParam automations for per-voice drum CCs.
        for inst in [inst808, inst909] {
            for rp in RackParam::ALL {
                assert!(
                    !proj.modulations.iter().any(|m| {
                        m.target
                            == ModulationTarget::Instrument {
                                instrument: inst.id,
                                parameter: InstrumentParam::Rack { param: rp },
                            }
                    }),
                    "must not emit kit-wide {rp:?} on {} (use_synth={use_synth})",
                    inst.name
                );
            }
        }

        let zone_id = |kit: &RackPatch, midi: u8| -> u16 {
            kit.zones
                .iter()
                .find(|z| z.low_midi <= midi && midi <= z.high_midi)
                .unwrap()
                .id
        };
        let find_zone_mod = |inst_id: u64, zone: u16, param: ZoneParam| {
            proj.modulations
                .iter()
                .find(|m| {
                    m.target
                        == ModulationTarget::Instrument {
                            instrument: inst_id,
                            parameter: InstrumentParam::Zone { zone, param },
                        }
                })
                .unwrap_or_else(|| {
                    panic!(
                        "missing Zone modulation for inst={inst_id} z{zone} {param:?} (use_synth={use_synth}); have: {:?}",
                        proj.modulations.iter().map(|m| (&m.name, m.target)).collect::<Vec<_>>()
                    )
                })
        };
        let find_mod_param = |inst_id: u64, zone: u16, param: ModuleParam| {
            proj.modulations
                .iter()
                .find(|m| {
                    matches!(
                        m.target,
                        ModulationTarget::Instrument {
                            instrument,
                            parameter: InstrumentParam::Module { zone: z, param: p, .. },
                        } if instrument == inst_id && z == zone && p == param
                    )
                })
                .unwrap_or_else(|| {
                    panic!(
                        "missing Module modulation for inst={inst_id} z{zone} {param:?} (use_synth={use_synth}); have: {:?}",
                        proj.modulations.iter().map(|m| (&m.name, m.target)).collect::<Vec<_>>()
                    )
                })
        };

        let z808_bd = zone_id(kit808, drum_midi::BD);
        let z808_sd = zone_id(kit808, drum_midi::SD);
        let z808_lt = zone_id(kit808, drum_midi::LT);
        let z808_lc = zone_id(kit808, drum_midi::LC);
        let z808_cy = zone_id(kit808, drum_midi::CY_OR_RD);
        let z808_oh = zone_id(kit808, drum_midi::OH);

        let z909_bd = zone_id(kit909, drum_midi::BD);
        let z909_sd = zone_id(kit909, drum_midi::SD);
        let z909_ch = zone_id(kit909, drum_midi::CH);
        let z909_oh = zone_id(kit909, drum_midi::OH);
        let z909_cr = zone_id(kit909, drum_midi::CB_OR_CR);

        // Verify calibrated BD Gain values drop from bar 0 (115) -> bar 2 (45).
        let bd808_gain = find_zone_mod(inst808.id, z808_bd, ZoneParam::Gain);
        let g0 = proj.value_at(bd808_gain.pattern, 0.0).unwrap();
        let g32 = proj.value_at(bd808_gain.pattern, 32.0).unwrap();
        let bd808_trim = controls::TR808_ZONE_CONTROLS
            .iter()
            .find_map(|s| match s.kind {
                controls::DrumZoneParamKind::Gain { trim_db } if s.midi == drum_midi::BD => {
                    Some(trim_db)
                }
                _ => None,
            })
            .unwrap();
        assert!(
            (g0 - ZoneParam::Gain.normalize(calibration::rbs_zone_level_db(115, bd808_trim))).abs()
                < 1e-3
        );
        assert!(
            (g32 - ZoneParam::Gain.normalize(calibration::rbs_zone_level_db(45, bd808_trim))).abs()
                < 1e-3
        );
        assert!(g0 > g32 + 0.1);

        // Verify 808 dynamic LT + LC alt zone BOTH receive Gain and Tune automations.
        find_zone_mod(inst808.id, z808_lt, ZoneParam::Gain);
        find_zone_mod(inst808.id, z808_lc, ZoneParam::Gain);

        // Verify 909 shared HH level ctrl 22 drives BOTH CH and OH Gain with distinct trims.
        let ch909_gain = find_zone_mod(inst909.id, z909_ch, ZoneParam::Gain);
        let oh909_gain = find_zone_mod(inst909.id, z909_oh, ZoneParam::Gain);
        assert!(
            proj.value_at(oh909_gain.pattern, 0.0).unwrap()
                > proj.value_at(ch909_gain.pattern, 0.0).unwrap()
        );

        if use_synth {
            // Synth zones target Filter/Env/Vco/Noise ModuleParams.
            let bd808_cut = find_mod_param(inst808.id, z808_bd, ModuleParam::Cutoff);
            let c0 = proj.value_at(bd808_cut.pattern, 0.0).unwrap();
            let c32 = proj.value_at(bd808_cut.pattern, 32.0).unwrap();
            assert!(
                (c0 - ModuleParam::Cutoff.normalize(calibration::rbs_808_bd_cutoff_hz(25, false)))
                    .abs()
                    < 1e-3
            );
            assert!(
                (c32 - ModuleParam::Cutoff
                    .normalize(calibration::rbs_808_bd_cutoff_hz(120, false)))
                .abs()
                    < 1e-3
            );
            assert!(c32 > c0 + 0.15);
            find_mod_param(inst808.id, z808_bd, ModuleParam::Decay);
            find_mod_param(inst808.id, z808_sd, ModuleParam::Cutoff);
            find_mod_param(inst808.id, z808_sd, ModuleParam::Level);
            find_mod_param(inst808.id, z808_lt, ModuleParam::Tune);
            find_mod_param(inst808.id, z808_lc, ModuleParam::Tune);
            find_mod_param(inst808.id, z808_cy, ModuleParam::Cutoff);
            find_mod_param(inst808.id, z808_oh, ModuleParam::Decay);

            find_mod_param(inst909.id, z909_bd, ModuleParam::Tune);
            find_mod_param(inst909.id, z909_bd, ModuleParam::Level);
            find_mod_param(inst909.id, z909_bd, ModuleParam::Decay);
            find_mod_param(inst909.id, z909_sd, ModuleParam::Tune);
            find_mod_param(inst909.id, z909_sd, ModuleParam::Cutoff);
            find_mod_param(inst909.id, z909_sd, ModuleParam::Level);
            find_mod_param(inst909.id, z909_ch, ModuleParam::Decay);
            find_mod_param(inst909.id, z909_oh, ModuleParam::Decay);
            find_mod_param(inst909.id, z909_cr, ModuleParam::Tune);
        } else {
            // Sampled zones splice Filter / OneShot Env -> Vca on demand and target Sampler.CoarseTune.
            let bd808_cut = find_mod_param(inst808.id, z808_bd, ModuleParam::Cutoff);
            let c0 = proj.value_at(bd808_cut.pattern, 0.0).unwrap();
            let c32 = proj.value_at(bd808_cut.pattern, 32.0).unwrap();
            assert!(c32 > c0 + 0.3);

            find_mod_param(inst808.id, z808_bd, ModuleParam::Decay);
            // BD started at decay=127 (unspliced) and dipped below 0.72 -> OneShot Decay Env spliced!
            let bd_zone = kit808.zone(z808_bd).unwrap();
            let spliced_filter = bd_zone
                .modules
                .iter()
                .find_map(|m| match &m.kind {
                    ModuleKind::Filter(flt) => Some(flt),
                    _ => None,
                })
                .expect("Sample zone with automated Cutoff must splice a neutral Tone Filter");
            assert_eq!(spliced_filter.mode, cadence_core::rack::FilterMode::ImLpf2);
            assert_eq!(spliced_filter.kerr_semitones, 0.0);
            assert_eq!(spliced_filter.spread, 0.0);
            let spliced_env = bd_zone
                .modules
                .iter()
                .find_map(|m| match &m.kind {
                    ModuleKind::Env(env) => Some(env),
                    _ => None,
                })
                .expect("Sample zone with automated Decay must splice a OneShot Decay Env");
            assert_eq!(spliced_env.sustain, 0.0);

            // LT started at tune=40 (-1.5 st -> fine_cents=50) and automated CoarseTune -> fine_cents cleared!
            find_mod_param(inst808.id, z808_lt, ModuleParam::CoarseTune);
            find_mod_param(inst808.id, z808_lc, ModuleParam::CoarseTune);
            let lt_smp = kit808
                .zone(z808_lt)
                .unwrap()
                .modules
                .iter()
                .find_map(|m| match &m.kind {
                    ModuleKind::Sampler(smp) => Some(smp),
                    _ => None,
                })
                .expect("expected Sampler module on LT");
            assert_eq!(
                lt_smp.fine_cents, 0.0,
                "Sample zone with automated CoarseTune must zero static fine_cents"
            );

            find_mod_param(inst808.id, z808_sd, ModuleParam::Cutoff);
            find_mod_param(inst808.id, z808_cy, ModuleParam::Cutoff);
            find_mod_param(inst808.id, z808_oh, ModuleParam::Decay);

            find_mod_param(inst909.id, z909_bd, ModuleParam::CoarseTune);
            find_mod_param(inst909.id, z909_bd, ModuleParam::Cutoff);
            find_mod_param(inst909.id, z909_bd, ModuleParam::Decay);
            find_mod_param(inst909.id, z909_sd, ModuleParam::CoarseTune);
            find_mod_param(inst909.id, z909_sd, ModuleParam::Cutoff);
            find_mod_param(inst909.id, z909_ch, ModuleParam::Decay);
            find_mod_param(inst909.id, z909_oh, ModuleParam::Decay);
            find_mod_param(inst909.id, z909_cr, ModuleParam::CoarseTune);
        }

        // Must validate and compile cleanly into executable ScoreWindow mod lanes!
        let score = proj.compile().unwrap();
        assert!(
            score
                .mod_lanes
                .iter()
                .any(|l| matches!(l.target, InstrumentParam::Zone { .. }))
                && score
                    .mod_lanes
                    .iter()
                    .any(|l| matches!(l.target, InstrumentParam::Module { .. })),
            "compiled ScoreWindow must carry InstrumentParam::Zone and Module mod_lanes"
        );
    }
}

#[test]
fn rbs_comp_zero_makeup_and_sample_drum_calibration_avoids_double_envelope() {
    use cadence_core::mixer::EffectKind;
    use cadence_core::presets::{InstrumentPreset, PresetCatalog};
    use cadence_core::rack::{drum_midi, ModuleKind, RackPatch, RackZone};

    // 1. ReBirth Compressor uses zero auto makeup gain even under deep threshold automation.
    let mut song = RbsSong {
        song_mode: true,
        loop_on: true,
        loop_start_bar: 0,
        loop_end_bar: 4,
        ..RbsSong::default()
    };
    song.tr808.params[0] = 1;
    song.tr808.patterns[0].steps[0][1] = 1;
    // Compressor ON, Amount=127 (ratio=12), Thresh=10 (-28 dB), Target=4 (TR-808).
    song.comp = [1, 127, 10];
    song.mixer.comp_target = 4; // TR-808
    song.tracks[8] = vec![
        TrackEvent {
            pos: 0,
            ctrl: 0,
            value: 1,
        },
        TrackEvent {
            pos: 0,
            ctrl: 1,
            value: 127,
        },
        TrackEvent {
            pos: 0,
            ctrl: 2,
            value: 10,
        },
        TrackEvent {
            pos: 16,
            ctrl: 2,
            value: 0,
        },
    ];

    let tmp = tempfile::tempdir().unwrap();
    let kits_dir = tmp.path().join("kits");
    let synth_dir = tmp.path().join("synth");
    std::fs::create_dir_all(&kits_dir).unwrap();
    std::fs::create_dir_all(&synth_dir).unwrap();
    InstrumentPreset::new("Acid 303", Some("Bass".into()), RackPatch::acid_303())
        .save(&synth_dir.join("acid-303.json"))
        .unwrap();
    let kit909 = RackPatch {
        kit_name: "TR-909".into(),
        zones: vec![RackZone::sample_pad(
            1,
            "BD",
            drum_midi::BD,
            "synthetic/909-bd.wav",
        )],
        ..RackPatch::default_kit()
    };
    InstrumentPreset::new("TR-909", Some("Kit".into()), kit909)
        .save(&kits_dir.join("tr-909.json"))
        .unwrap();
    let kit808 = RackPatch {
        kit_name: "TR-808".into(),
        gain_db: -4.84,
        zones: vec![RackZone::sample_pad(
            1,
            "BD",
            drum_midi::BD,
            "synthetic/808-bd.wav",
        )],
        ..RackPatch::default_kit()
    };
    InstrumentPreset::new("TR-808", Some("Kit".into()), kit808)
        .save(&kits_dir.join("tr-808.json"))
        .unwrap();
    let catalog = PresetCatalog::from_roots(&[tmp.path().to_path_buf()]);

    // TR-909 BD at center decay (64) & max tune (127).
    song.tr909.params[0] = 1;
    song.tr909.params[4] = 127; // BD Tune max -> +3.0 st (not +6.0 st)
    song.tr909.params[6] = 64; // BD Decay mid (~271 ms synth -> ~705 ms sample decay)
    song.tr909.patterns[0].steps[0][1] = 1;

    let (proj, _) = song
        .to_cadence_project_with_presets("Comp&SampleTest", &catalog)
        .unwrap();
    let ch_808 = proj
        .mixer
        .channels
        .iter()
        .find(|c| c.name == "TR-808")
        .unwrap();
    let comp_808 = ch_808
        .inserts
        .iter()
        .find_map(|d| match &d.kind {
            EffectKind::Compressor(cp) => Some(cp),
            _ => None,
        })
        .expect("TR-808 compressor must exist");
    assert_eq!(
        comp_808.makeup_db, 0.0,
        "ReBirth Compressor has zero auto makeup gain (p. 66-67 manual), got {}",
        comp_808.makeup_db
    );

    // 2. Imported TR-909 kit applies RBS_DRUM_KIT_GAIN_DB, scales spliced decay by 2.6x, and keeps BD tune <= +3.0 st.
    let inst303 = proj
        .instruments
        .iter()
        .find(|i| i.name == "TB-303 A")
        .unwrap();
    assert_eq!(inst303.rack.gain_db, calibration::RBS_303_RACK_GAIN_DB);
    let inst808 = proj
        .instruments
        .iter()
        .find(|i| i.name == "TR-808")
        .unwrap();
    assert!(
        (inst808.rack.gain_db - (-4.84 + calibration::RBS_DRUM_KIT_GAIN_DB)).abs() < 1e-4,
        "TR-808 headroom trim must preserve preset base gain_db, got {}",
        inst808.rack.gain_db
    );
    let inst909 = proj
        .instruments
        .iter()
        .find(|i| i.name == "TR-909")
        .unwrap();
    let k909 = &inst909.rack;
    assert_eq!(k909.gain_db, calibration::RBS_DRUM_KIT_GAIN_DB);
    let bd = &k909.zones[0];
    let smp = bd
        .modules
        .iter()
        .find_map(|m| match &m.kind {
            ModuleKind::Sampler(s) => Some(s),
            _ => None,
        })
        .expect("expected Sampler module on 909 BD");
    let decay_env = bd
        .modules
        .iter()
        .find_map(|m| match &m.kind {
            ModuleKind::Env(e) => Some(e),
            _ => None,
        })
        .expect("expected spliced Decay Env on 909 BD below 0.72");
    assert!(
        smp.coarse_semitones <= 3,
        "TR-909 max BD Tune should stay within ±3 st, got {}",
        smp.coarse_semitones
    );
    assert!(
        decay_env.decay_ms > 600.0,
        "Sampled 909 BD mid decay should scale up to avoid double-enveloping, got {} ms",
        decay_env.decay_ms
    );
}

#[test]
fn mixer_track_0_preserves_master_level_and_v42_channel_enables_and_1x_master_cc7() {
    // 1. v4.2 TRAK Track 0 accepts ctrl 0 (Master Level) and ctrls 5/11/17/23 (channel enable/mute),
    //    while still rejecting out-of-range values.
    let mut payload = Vec::new();
    let events: &[(u8, u8)] = &[
        (0, 110), // Master Level = 110 (valid)
        (0, 128), // Master Level = 128 (invalid -> dropped)
        (5, 0),   // 303A enable = 0 (valid)
        (11, 1),  // 303B enable = 1 (valid)
        (17, 2),  // 808 enable = 2 (invalid -> dropped)
        (23, 0),  // 909 enable = 0 (valid)
    ];
    payload.extend_from_slice(&(events.len() as u32).to_be_bytes());
    for &(ctrl, val) in events {
        payload.extend_from_slice(&[0, ctrl, val]);
    }
    let parsed_42 = parse_single_trak(&payload, 0, false, false).unwrap();
    let pairs_42: Vec<(u8, u8)> = parsed_42.iter().map(|e| (e.ctrl, e.value)).collect();
    assert_eq!(pairs_42, vec![(0, 110), (5, 0), (11, 1), (23, 0)]);

    // 2. Pre-4.2 (v4.1) accepts Master Level (ctrl 0) but drops v4.2 channel enable switches.
    let parsed_41 = parse_single_trak(&payload, 0, false, true).unwrap();
    let pairs_41: Vec<(u8, u8)> = parsed_41.iter().map(|e| (e.ctrl, e.value)).collect();
    assert_eq!(pairs_41, vec![(0, 110)]);

    // 3. v4.0 accepts Master Level (ctrl 0) while dropping old sub==0 enable byte (ctrl 4).
    let mut p40 = Vec::new();
    p40.extend_from_slice(&2u32.to_be_bytes());
    p40.extend_from_slice(&[0, 0, 95, 0, 4, 0]);
    let parsed_40 = parse_single_trak(&p40, 0, true, true).unwrap();
    assert_eq!(
        parsed_40,
        vec![TrackEvent {
            pos: 0,
            ctrl: 0,
            value: 95,
        }]
    );

    // 4. ReBirth 1.x SMF maps MIDI Track 5 CC 7 to Track 0 ctrl 0 (Master Level).
    let mut song = RbsSong::default();
    map_1x_cc(5, 32, 7, 88, &mut song);
    assert_eq!(
        song.tracks[0],
        vec![TrackEvent {
            pos: 32,
            ctrl: 0,
            value: 88,
        }]
    );
}

#[test]
fn tb303_velocity_threshold_vintage_drive_and_decay_ramp_mode() {
    use cadence_core::dsp::{acid_accent_amount, ACID_ACCENT_THRESHOLD};
    use cadence_core::material::ModulationTarget;
    use cadence_core::params::{InstrumentParam, ValueMode};
    use cadence_core::rack::{ModuleParam, Waveform};

    // TB-303 waveform selector maps Saw (0) -> MobiusJet and Square (!= 0) -> MobiusPwm.
    assert_eq!(
        calibration::rbs_303_waveform_f32(0),
        Waveform::MobiusJet.to_index() as f32
    );
    assert_eq!(
        calibration::rbs_303_waveform_f32(1),
        Waveform::MobiusPwm.to_index() as f32
    );

    // TB-303 normal velocity sits cleanly below DiodeLadder accent threshold; accent hits 1.0.
    assert!(f64::from(calibration::RBS_303_NORMAL_VEL) < ACID_ACCENT_THRESHOLD);
    assert_eq!(
        acid_accent_amount(f64::from(calibration::RBS_303_NORMAL_VEL)),
        0.0
    );
    assert!((acid_accent_amount(f64::from(calibration::RBS_303_ACCENT_VEL)) - 1.0).abs() < 1e-6);

    // Vintage mode scales TB-303 Capacitor filter decay cleanly on the Filter Env module;
    // Decay (ctrl 6) automates as smooth ValueMode::Ramp, Waveform (ctrl 8) as ValueMode::Hold.
    let mut song = RbsSong {
        song_mode: true,
        loop_on: true,
        loop_end_bar: 2,
        vintage: false,
        ..RbsSong::default()
    };
    song.tb303[0].patterns[0].steps[0].rest = false;
    song.tb303[1].params[8] = 1; // Square -> MobiusPwm
    song.tracks[1] = vec![
        TrackEvent {
            pos: 0,
            ctrl: 0,
            value: 1,
        },
        TrackEvent {
            pos: 0,
            ctrl: 1,
            value: 0,
        },
        TrackEvent {
            pos: 0,
            ctrl: 6,
            value: 20,
        },
        TrackEvent {
            pos: 0,
            ctrl: 8,
            value: 0,
        },
        TrackEvent {
            pos: 32,
            ctrl: 6,
            value: 115,
        },
        TrackEvent {
            pos: 32,
            ctrl: 8,
            value: 1,
        },
    ];
    let modern = song.to_cadence_project("Modern").unwrap();
    song.vintage = true;
    let vintage = song.to_cadence_project("Vintage").unwrap();

    // Verify TB-303 A (saw -> MobiusJet) and TB-303 B (square -> MobiusPwm) static settings
    // plus Capacitor filter envelope invariants.
    assert_tb303_cubic_rack(&modern.instruments[0].rack, Waveform::MobiusJet);
    assert_tb303_cubic_rack(&modern.instruments[1].rack, Waveform::MobiusPwm);

    // Verify IFF GLOB[482] = 1 parses `vintage = true` end-to-end through `import_rbs_bytes`.
    let mut iff_bytes = build_synthetic_rbs_42();
    // Locate GLOB chunk payload and flip byte 482 to 1.
    let glob_pos = iff_bytes
        .windows(4)
        .position(|w| w == b"GLOB")
        .expect("GLOB chunk");
    iff_bytes[glob_pos + 8 + 482] = 1;
    let (iff_vintage, _) = import_rbs_bytes(&iff_bytes, "IffVintage").unwrap();

    let decay_of = |p: &Project| {
        let targets = calibration::resolve_tb303_targets(&p.instruments[0].rack)
            .expect("expected Acid 303 Rack");
        p.instruments[0]
            .rack
            .read_param_native(InstrumentParam::Module {
                zone: targets.zone_id,
                module: targets.filter_env_id,
                param: ModuleParam::Decay,
            })
            .expect("303 filter env decay")
    };
    let expected_modern = calibration::rbs_303_decay_ms(20);
    let expected_vintage = calibration::rbs_303_decay_ms_for_mode(20, true);
    assert!((decay_of(&modern) - expected_modern).abs() < 1e-3);
    assert!((decay_of(&vintage) - expected_vintage).abs() < 1e-3);
    assert!(decay_of(&vintage) < decay_of(&modern) * 0.8);
    // Verify IFF GLOB[482]=1 parsed `vintage = true` and shortened TB-303 decay (ctrl 6 = 75).
    assert!(
        (decay_of(&iff_vintage) - calibration::rbs_303_decay_ms_for_mode(75, true)).abs() < 1e-3
    );

    let mod_targets = calibration::resolve_tb303_targets(&modern.instruments[0].rack).unwrap();
    let vin_targets = calibration::resolve_tb303_targets(&vintage.instruments[0].rack).unwrap();
    let decay_mod = modern
        .modulations
        .iter()
        .find(|m| {
            m.target
                == ModulationTarget::Instrument {
                    instrument: modern.instruments[0].id,
                    parameter: InstrumentParam::Module {
                        zone: mod_targets.zone_id,
                        module: mod_targets.filter_env_id,
                        param: ModuleParam::Decay,
                    },
                }
        })
        .expect("303A Decay modulation");
    let vintage_decay_mod = vintage
        .modulations
        .iter()
        .find(|m| {
            m.target
                == ModulationTarget::Instrument {
                    instrument: vintage.instruments[0].id,
                    parameter: InstrumentParam::Module {
                        zone: vin_targets.zone_id,
                        module: vin_targets.filter_env_id,
                        param: ModuleParam::Decay,
                    },
                }
        })
        .expect("Vintage 303A Decay modulation");
    assert_eq!(
        modern.value_mode(decay_mod.pattern).unwrap(),
        ValueMode::Ramp,
        "TB-303 Decay automation must use ValueMode::Ramp"
    );
    let PatternKind::Values {
        points: mod_pts, ..
    } = &modern.pattern(decay_mod.pattern).unwrap().kind
    else {
        panic!("expected Values pattern");
    };
    let PatternKind::Values {
        points: vin_pts, ..
    } = &vintage.pattern(vintage_decay_mod.pattern).unwrap().kind
    else {
        panic!("expected Values pattern");
    };
    assert!(
        vin_pts.last().unwrap().value < mod_pts.last().unwrap().value,
        "Vintage TB-303 automated Decay curve must stay shorter than Modern"
    );

    // Song-Mode Waveform switch (ctrl 8: 0 -> 1) emits Hold automation from MobiusJet to MobiusPwm.
    let wave_mod = modern
        .modulations
        .iter()
        .find(|m| {
            m.target
                == ModulationTarget::Instrument {
                    instrument: modern.instruments[0].id,
                    parameter: InstrumentParam::Module {
                        zone: mod_targets.zone_id,
                        module: mod_targets.vco_id,
                        param: ModuleParam::Waveform,
                    },
                }
        })
        .expect("303A Wave modulation");
    assert_eq!(
        modern.value_mode(wave_mod.pattern).unwrap(),
        ValueMode::Hold
    );
    let PatternKind::Values {
        points: wave_pts, ..
    } = &modern.pattern(wave_mod.pattern).unwrap().kind
    else {
        panic!("expected Values pattern");
    };
    assert_eq!(wave_pts.len(), 2);
    assert!(
        (wave_pts[0].value
            - ModuleParam::Waveform.normalize(Waveform::MobiusJet.to_index() as f32))
        .abs()
            < 1e-5
    );
    assert!(
        (wave_pts[1].value
            - ModuleParam::Waveform.normalize(Waveform::MobiusPwm.to_index() as f32))
        .abs()
            < 1e-5
    );

    // Custom catalog `acid-303` presets with non-default VCO reach/shape or ADSR filter env
    // are normalized to crisp Mobius VCO and Capacitor filter envelope invariants.
    let mut custom_303 = cadence_core::rack::RackPatch::acid_303();
    for m in &mut custom_303.zones[0].modules {
        match &mut m.kind {
            cadence_core::rack::ModuleKind::Vco(v) => {
                v.reach = 0.50;
                v.shape = 0.20;
            }
            cadence_core::rack::ModuleKind::Env(e) if m.name == "Filter Env" => {
                e.mode = cadence_core::rack::EnvMode::Adsr;
                e.trigger_mode = cadence_core::rack::TriggerMode::Gate;
                e.cubic_limit = 0.0;
            }
            _ => {}
        }
    }
    let tmp = tempfile::tempdir().unwrap();
    let synth_dir = tmp.path().join("synth");
    std::fs::create_dir_all(&synth_dir).unwrap();
    cadence_core::presets::InstrumentPreset::new("Acid 303", Some("Bass".into()), custom_303)
        .save(&synth_dir.join("acid-303.json"))
        .unwrap();
    let catalog = cadence_core::presets::PresetCatalog::from_roots(&[tmp.path().to_path_buf()]);
    let mut notices = Vec::new();
    let params = [1, 0, 64, 48, 90, 80, 64, 100, 1]; // Square -> MobiusPwm
    let calibrated = calibration::rack_patch_for_303(&params, false, &catalog, &mut notices);
    assert!(notices.is_empty(), "unexpected notices: {notices:?}");
    assert_tb303_cubic_rack(&calibrated, Waveform::MobiusPwm);
}

#[test]
fn smart_automation_compression_and_32nd_note_series_fidelity() {
    use cadence_core::material::eval_value_points;
    use cadence_core::params::ValueMode;
    use std::collections::BTreeMap;

    // 1. Sudden 3-step jump (dt=3 <= 4, dv=0.80 >= 0.20) MUST insert a pre-step hold anchor at step 2!
    let mut jump = BTreeMap::new();
    jump.insert(0, 0.90);
    jump.insert(3, 0.10);
    let jump_pts = automation::compress_points(&jump, ValueMode::Ramp, 32, None);
    assert!(
        (eval_value_points(32, ValueMode::Ramp, &jump_pts, 2.0).unwrap() - 0.90).abs() < 1e-4,
        "step 2 before a 3-step jump must hold 0.90 instead of blurring into a ramp: {jump_pts:?}"
    );
    assert!((eval_value_points(32, ValueMode::Ramp, &jump_pts, 3.0).unwrap() - 0.10).abs() < 1e-4);

    // 2. Slow quantized sweep (dt=6 > 4, dv=0.02 <= 0.05) MUST stay a smooth ramp with NO staircase!
    let mut sweep = BTreeMap::new();
    for i in 0..=4u32 {
        sweep.insert(i * 6, 0.10 + (i as f32) * 0.02);
    }
    let sweep_pts = automation::compress_points(&sweep, ValueMode::Ramp, 64, None);
    // Midpoint t=3.0 between (0, 0.10) and (6, 0.12) must smoothly evaluate to 0.11 (not flat 0.10!).
    let mid = eval_value_points(64, ValueMode::Ramp, &sweep_pts, 3.0).unwrap();
    assert!(
        (mid - 0.11).abs() < 1e-3,
        "slow quantized sweep must interpolate smoothly across dt=6 gaps, got {mid} ({sweep_pts:?})"
    );
    // Collinear pruning keeps the ramp concise (start + end + tail pin = 3 points).
    assert_eq!(
        sweep_pts.len(),
        3,
        "expected 3 simplified points: {sweep_pts:?}"
    );

    // 3. 32nd-note event handling: on-beat pos=0 is NEVER overwritten by mid-step pos=1,
    //    and a 32nd-note linear sweep preserves exact half-step midpoint values!
    let mut song = RbsSong {
        song_mode: true,
        ..RbsSong::default()
    };
    song.tracks[1] = vec![
        TrackEvent {
            pos: 0,
            ctrl: 3,
            value: 20,
        },
        TrackEvent {
            pos: 1,
            ctrl: 3,
            value: 100,
        },
        TrackEvent {
            pos: 4,
            ctrl: 4,
            value: 10,
        },
        TrackEvent {
            pos: 5,
            ctrl: 4,
            value: 20,
        },
        TrackEvent {
            pos: 6,
            ctrl: 4,
            value: 30,
        },
        TrackEvent {
            pos: 7,
            ctrl: 4,
            value: 40,
        },
        TrackEvent {
            pos: 8,
            ctrl: 4,
            value: 50,
        },
    ];
    let s3 = song.ctrl_series(1, 3, 0, 16);
    assert_eq!(
        s3.get(&0),
        Some(&20),
        "pos=0 must not be overwritten by pos=1"
    );
    assert_eq!(s3.get(&1), Some(&100), "pos=1 must take effect on step 1");

    let s4 = song.ctrl_series(1, 4, 0, 16);
    assert_eq!(s4.get(&2), Some(&10));
    assert_eq!(s4.get(&3), Some(&30));
    assert_eq!(s4.get(&4), Some(&50));
}

#[test]
fn drum_accent_calibration_bpm_flam_and_delay_triplet_math() {
    // 1. Global Accent=0 produces exact uniform velocity; Accent=64 separates; Accent=127 punches to 1.0.
    let v0 = calibration::rbs_drum_velocities(0);
    let v64 = calibration::rbs_drum_velocities(64);
    let v127 = calibration::rbs_drum_velocities(127);
    assert!((v0.normal - v0.accent).abs() < 1e-6);
    assert!((v0.flam_grace_normal - v0.flam_grace_accent).abs() < 1e-6);
    assert!(v64.accent > v64.normal + 0.14);
    assert!((v127.accent - 1.0).abs() < 1e-6 && v127.accent > v127.normal + 0.30);

    // 2. BPM-accurate TR-909 flam: same flam_interval at 180 BPM has ~2x the step-fraction offset of 90 BPM,
    //    preserving the real ~26 ms hardware spacing!
    let off_90 = calibration::rbs_909_flam_offset(64, 90.0);
    let off_180 = calibration::rbs_909_flam_offset(64, 180.0);
    let ms_90 = off_90 * (15_000.0 / 90.0);
    let ms_180 = off_180 * (15_000.0 / 180.0);
    assert!(
        (ms_90 - ms_180).abs() < 0.2 && (ms_90 - 26.12).abs() < 0.5,
        "909 flam should hold ~26 ms across tempos (90bpm={ms_90:.2}ms, 180bpm={ms_180:.2}ms)"
    );
    // Clamped safely at extreme tempos.
    assert!((0.035..=0.48).contains(&calibration::rbs_909_flam_offset(0, 20.0)));
    assert!((0.035..=0.48).contains(&calibration::rbs_909_flam_offset(127, 300.0)));

    // 3. ReBirth Delay eighth-note triplets (`mode == 0`): 3 eighth-note triplets = 4 sixteenth steps (1 beat).
    assert!((calibration::rbs_delay_steps(3, 0) - 4.0).abs() < 1e-4);
    assert!((calibration::rbs_delay_steps(12, 0) - 16.0).abs() < 1e-4);
    assert!((calibration::rbs_delay_steps(3, 1) - 3.0).abs() < 1e-4);
}
