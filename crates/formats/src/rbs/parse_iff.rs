//! ReBirth 2.0 IFF (`CAT RB40`, file versions 4.0–4.2) binary parser and header-only peek.

use super::controls::{
    is_mixer_ch_enable_ctrl, valid_track_event, MIXER_CTRL_COMP_TARGET, MIXER_CTRL_PCF_TARGET,
    TRACK_808, TRACK_MIXER,
};
use super::model::{
    DrumPattern, MixerChannelInit, RbsHeaderPreview, RbsSong, RbsVersion, Tb303Pattern, Tb303Step,
    TrackEvent,
};
use super::{
    decode_zstring, read_u32_be, sanitize_title, sanitize_title_opt, MAX_RBS_POS, TICKS_PER_BAR,
};
use anyhow::{ensure, Result};
use cadence_core::material::MAX_BARS;

pub(super) fn peek_rbs_iff_header(bytes: &[u8], fallback: &str) -> Result<RbsHeaderPreview> {
    let total_size = read_u32_be(bytes, 4)? as usize;
    ensure!(
        total_size >= 4 && 8 + total_size <= bytes.len(),
        "Invalid IFF CAT chunk size ({total_size}) for {} byte file",
        bytes.len()
    );
    ensure!(
        &bytes[8..12] == b"RB40",
        "Expected IFF container type 'RB40'"
    );

    let mut version = RbsVersion::default();
    let mut title = String::new();
    let mut song_mode = true;
    let mut loop_on = true;
    let mut bpm_millis = 120_000u32;
    let mut loop_end_bar = 4u32;
    let mut saw_head = false;
    let mut saw_glob = false;
    let mut trkl_slice: Option<&[u8]> = None;

    let end = 8 + total_size;
    let mut off = 12;
    while off + 8 <= end {
        let tag = &bytes[off..off + 4];
        let sz = read_u32_be(bytes, off + 4)? as usize;
        off += 8;
        ensure!(off + sz <= end, "Chunk {:?} exceeds RB40 bounds", tag);
        let payload = &bytes[off..off + sz];
        match tag {
            b"HEAD" => {
                ensure!(payload.len() >= 10, "HEAD chunk truncated");
                ensure!(
                    &payload[0..4] == b"[T[T" && payload[4] == 0xBC,
                    "Invalid HEAD magic in .rbs file"
                );
                version = RbsVersion {
                    major: payload[5],
                    minor: payload[6],
                    revision: payload[7],
                };
                ensure!(
                    version.major == 4,
                    "Unsupported .rbs major version {}",
                    version.major
                );
                saw_head = true;
            }
            b"GLOB" => {
                ensure!(payload.len() >= 483, "GLOB chunk truncated");
                song_mode = payload[0] != 0;
                loop_on = payload[1] != 0;
                bpm_millis = read_u32_be(payload, 2)?;
                loop_end_bar = read_u32_be(payload, 10)? / TICKS_PER_BAR;
                saw_glob = true;
            }
            b"USRI" => {
                if payload.len() >= 41 {
                    title = decode_zstring(&payload[0..41]);
                }
            }
            // Skip `DEVL` (128 patterns + device chunks) completely; record `TRKL` only as
            // fallback if the song has no explicit loop_end_bar.
            b"CAT " if payload.len() >= 4 && &payload[0..4] == b"TRKL" => {
                trkl_slice = Some(&payload[4..]);
            }
            _ => {}
        }
        off += sz + (sz & 1);
    }

    ensure!(saw_head && saw_glob, "Missing required HEAD or GLOB chunk");
    let bars = if !song_mode {
        4
    } else if loop_on && loop_end_bar > 0 {
        (loop_end_bar as usize).clamp(1, MAX_BARS)
    } else if let Some(trkl) = trkl_slice {
        // Unlooped Song Mode: parse only TRKL timestamps (still skips DEVL patterns!).
        let mut tmp = RbsSong {
            version,
            song_mode,
            loop_on,
            loop_end_bar,
            ..RbsSong::default()
        };
        parse_trkl(trkl, &mut tmp)?;
        tmp.song_bars()
    } else {
        4
    };
    let title = sanitize_title(&title, fallback);
    Ok(RbsHeaderPreview {
        title,
        version,
        bpm: (f64::from(bpm_millis) / 1000.0).clamp(20.0, 300.0),
        bars,
        song_mode,
    })
}

pub(super) fn parse_rbs_iff(bytes: &[u8]) -> Result<RbsSong> {
    let total_size = read_u32_be(bytes, 4)? as usize;
    ensure!(
        total_size >= 4 && 8 + total_size <= bytes.len(),
        "Invalid IFF CAT chunk size ({total_size}) for {} byte file",
        bytes.len()
    );
    ensure!(
        &bytes[8..12] == b"RB40",
        "Expected IFF container type 'RB40'"
    );

    let mut song = RbsSong::default();
    let mut saw_head = false;
    let mut saw_glob = false;
    let mut devl_slice: Option<&[u8]> = None;
    let mut trkl_slice: Option<&[u8]> = None;

    let end = 8 + total_size;
    let mut off = 12;
    while off + 8 <= end {
        let tag = &bytes[off..off + 4];
        let sz = read_u32_be(bytes, off + 4)? as usize;
        off += 8;
        ensure!(off + sz <= end, "Chunk {:?} exceeds RB40 bounds", tag);
        let payload = &bytes[off..off + sz];
        match tag {
            b"HEAD" => {
                ensure!(payload.len() >= 10, "HEAD chunk truncated");
                ensure!(
                    &payload[0..4] == b"[T[T" && payload[4] == 0xBC,
                    "Invalid HEAD magic in .rbs file"
                );
                song.version = RbsVersion {
                    major: payload[5],
                    minor: payload[6],
                    revision: payload[7],
                };
                ensure!(
                    song.version.major == 4,
                    "Unsupported .rbs major version {}",
                    song.version.major
                );
                saw_head = true;
            }
            b"GLOB" => {
                ensure!(payload.len() >= 483, "GLOB chunk truncated");
                song.song_mode = payload[0] != 0;
                song.loop_on = payload[1] != 0;
                song.bpm_millis = read_u32_be(payload, 2)?;
                song.loop_start_bar = read_u32_be(payload, 6)? / TICKS_PER_BAR;
                song.loop_end_bar = read_u32_be(payload, 10)? / TICKS_PER_BAR;
                song.shuffle = payload[14].min(127);
                song.mod_name = decode_zstring(&payload[15..80]);
                song.vintage = payload[482] != 0;
                saw_glob = true;
            }
            b"USRI" => {
                if payload.len() >= 41 {
                    song.title = sanitize_title_opt(&decode_zstring(&payload[0..41]));
                }
            }
            b"CAT " => {
                ensure!(payload.len() >= 4, "Nested CAT chunk truncated");
                let kind = &payload[0..4];
                if kind == b"DEVL" {
                    devl_slice = Some(&payload[4..]);
                } else if kind == b"TRKL" {
                    trkl_slice = Some(&payload[4..]);
                }
            }
            _ => {}
        }
        off += sz + (sz & 1);
    }

    ensure!(saw_head && saw_glob, "Missing required HEAD or GLOB chunk");
    if song.version.key()
        < (RbsVersion {
            major: 4,
            minor: 2,
            revision: 0,
        })
        .key()
    {
        song.vintage = false;
    }

    if let Some(devl) = devl_slice {
        parse_devl(devl, &mut song)?;
    }
    if let Some(trkl) = trkl_slice {
        parse_trkl(trkl, &mut song)?;
    }

    Ok(song)
}

fn parse_devl(buf: &[u8], song: &mut RbsSong) -> Result<()> {
    let ver = song.version;
    let is_4_0 = ver
        == RbsVersion {
            major: 4,
            minor: 0,
            revision: 0,
        };
    let is_4_1 = ver
        == RbsVersion {
            major: 4,
            minor: 1,
            revision: 0,
        };
    let pre_4_2 = ver.key()
        < (RbsVersion {
            major: 4,
            minor: 2,
            revision: 0,
        })
        .key();

    let mut off = 0;
    let mut tb303_seen = 0usize;
    while off + 8 <= buf.len() {
        let tag = &buf[off..off + 4];
        let sz = read_u32_be(buf, off + 4)? as usize;
        off += 8;
        ensure!(off + sz <= buf.len(), "DEVL chunk exceeds bounds");
        let p = &buf[off..off + sz];

        match tag {
            b"MIXR" => {
                ensure!(p.len() >= 64, "MIXR chunk truncated");
                song.mixer.master_level = p[0].min(127);
                if is_4_0 {
                    song.mixer.comp_target = if p[1] != 0 { 1 } else { 0 };
                    song.mixer.pcf_target = 0;
                    for (c, ch) in song.mixer.channels.iter_mut().enumerate() {
                        let base = 16 + 12 * c;
                        *ch = MixerChannelInit {
                            enabled: true,
                            level: p[base + 1].min(127),
                            pan: p[base + 2].min(127),
                            delay_send: p[base + 3].min(127),
                            dist_on: p[base + 4] != 0,
                        };
                        if p[base + 5] != 0 {
                            song.mixer.pcf_target = (c + 1) as u8;
                        }
                        if p[base + 6] != 0 {
                            song.mixer.comp_target = (c + 2) as u8;
                        }
                    }
                } else {
                    song.mixer.comp_target = p[1].min(5);
                    song.mixer.pcf_target = p[2].min(4);
                    for (c, ch) in song.mixer.channels.iter_mut().enumerate() {
                        let base = 16 + 12 * c;
                        *ch = MixerChannelInit {
                            enabled: if is_4_1 { true } else { p[base] != 0 },
                            level: p[base + 1].min(127),
                            pan: p[base + 2].min(127),
                            delay_send: p[base + 3].min(127),
                            dist_on: p[base + 4] != 0,
                        };
                    }
                }
            }
            b"DELY" => {
                ensure!(p.len() >= 5, "DELY chunk truncated");
                song.delay = [p[0], p[1].clamp(1, 32), p[2], p[3].min(127), p[4].min(127)];
            }
            b"PCF " => {
                ensure!(p.len() >= 7, "PCF chunk truncated");
                song.pcf = [
                    p[0],
                    p[1].min(127),
                    p[2].min(127),
                    p[3].min(127),
                    p[4],
                    p[5].min(127),
                    p[6],
                ];
            }
            b"DIST" => {
                ensure!(p.len() >= 3, "DIST chunk truncated");
                song.dist = [p[0], p[1].min(127), p[2].min(127)];
            }
            b"COMP" => {
                ensure!(p.len() >= 3, "COMP chunk truncated");
                song.comp = [p[0], p[1].min(127), p[2].min(127)];
            }
            b"303 " => {
                ensure!(p.len() >= 1097, "303 chunk truncated");
                if tb303_seen < 2 {
                    let dev = &mut song.tb303[tb303_seen];
                    dev.params.copy_from_slice(&p[0..9]);
                    dev.patterns.clear();
                    for pat_idx in 0..32 {
                        let base = 9 + pat_idx * 34;
                        let mut pat = Tb303Pattern {
                            shuffle: p[base] != 0,
                            length: p[base + 1].clamp(1, 16),
                            steps: [Tb303Step::default(); 16],
                        };
                        for (step_idx, step) in pat.steps.iter_mut().enumerate() {
                            let s_off = base + 2 + step_idx * 2;
                            let note = p[s_off].min(12);
                            let flags = p[s_off + 1];
                            *step = Tb303Step {
                                note,
                                slide: flags & 0x01 != 0,
                                accent: flags & 0x02 != 0,
                                up: flags & 0x04 != 0,
                                down: flags & 0x08 != 0,
                                rest: flags & 0x10 != 0,
                            };
                        }
                        dev.patterns.push(pat);
                    }
                    tb303_seen += 1;
                }
            }
            b"808 " => {
                ensure!(p.len() >= 6238, "808 chunk truncated");
                song.tr808.params.copy_from_slice(&p[0..29]);
                if pre_4_2 {
                    for idx in [10, 13, 16] {
                        song.tr808.params[idx] = (song.tr808.params[idx] / 2).saturating_add(32);
                    }
                }
                song.tr808.patterns.clear();
                for pat_idx in 0..32 {
                    let base = 30 + pat_idx * 194;
                    let mut pat = DrumPattern {
                        shuffle: p[base] != 0,
                        length: p[base + 1].clamp(1, 16),
                        steps: [[0; 12]; 16],
                    };
                    for (step_idx, step_voices) in pat.steps.iter_mut().enumerate() {
                        let s_off = base + 2 + step_idx * 12;
                        for (voice, cell) in step_voices.iter_mut().enumerate() {
                            *cell = u8::from(p[s_off + voice] != 0);
                        }
                    }
                    song.tr808.patterns.push(pat);
                }
            }
            b"909 " => {
                ensure!(p.len() >= 6239, "909 chunk truncated");
                song.tr909.params.copy_from_slice(&p[0..30]);
                song.tr909.patterns.clear();
                for pat_idx in 0..32 {
                    // 30 param bytes + 1 reserved byte = patterns begin at offset 31.
                    let base = 31 + pat_idx * 194;
                    let mut pat = DrumPattern {
                        shuffle: p[base] != 0,
                        length: p[base + 1].clamp(1, 16),
                        steps: [[0; 12]; 16],
                    };
                    for (step_idx, step_voices) in pat.steps.iter_mut().enumerate() {
                        let s_off = base + 2 + step_idx * 12;
                        step_voices.copy_from_slice(&p[s_off..s_off + 12]);
                    }
                    song.tr909.patterns.push(pat);
                }
            }
            _ => {}
        }
        off += sz + (sz & 1);
    }
    Ok(())
}

fn parse_trkl(buf: &[u8], song: &mut RbsSong) -> Result<()> {
    let ver = song.version;
    let is_4_0 = ver
        == RbsVersion {
            major: 4,
            minor: 0,
            revision: 0,
        };
    let pre_4_2 = ver.key()
        < (RbsVersion {
            major: 4,
            minor: 2,
            revision: 0,
        })
        .key();

    let mut off = 0;
    let mut track_idx = 0usize;
    while off + 8 <= buf.len() && track_idx < 9 {
        let tag = &buf[off..off + 4];
        let sz = read_u32_be(buf, off + 4)? as usize;
        off += 8;
        ensure!(off + sz <= buf.len(), "TRAK chunk exceeds bounds");
        if tag == b"TRAK" {
            let events = parse_single_trak(&buf[off..off + sz], track_idx, is_4_0, pre_4_2)?;
            song.tracks[track_idx] = events;
            track_idx += 1;
        }
        off += sz + (sz & 1);
    }
    Ok(())
}

pub(super) fn parse_single_trak(
    payload: &[u8],
    track_idx: usize,
    is_4_0: bool,
    pre_4_2: bool,
) -> Result<Vec<TrackEvent>> {
    if payload.len() < 4 {
        return Ok(Vec::new());
    }
    let count = read_u32_be(payload, 0)? as usize;
    let mut cursor = 4usize;
    let mut abs_pos: u32 = 0;
    let mut last_comp_pos: i64 = 0;
    let mut last_pcf_pos: i64 = 0;
    let mut out: Vec<TrackEvent> = Vec::with_capacity(count.min(16_384));

    for _ in 0..count {
        if cursor >= payload.len() {
            break;
        }
        // Arbitrary-length MIDI VLQ keeping low 32 bits (handles historical >4-byte deltas cleanly).
        let mut delta: u32 = 0;
        loop {
            let Some(&byte) = payload.get(cursor) else {
                out.sort_by_key(|ev| ev.pos);
                return Ok(out);
            };
            cursor += 1;
            delta = (delta.wrapping_shl(7)) | u32::from(byte & 0x7F);
            if byte & 0x80 == 0 {
                break;
            }
        }
        if cursor + 2 > payload.len() {
            break;
        }
        let mut ctrl = payload[cursor];
        let mut val = payload[cursor + 1];
        cursor += 2;

        let candidate_pos = abs_pos.wrapping_add(delta);

        // Version 4.0 mixer track controller remap.
        if track_idx == TRACK_MIXER && is_4_0 {
            let mut drop_ev = false;
            match ctrl {
                1 => {
                    if val != 0 {
                        val = 1;
                        last_comp_pos = i64::from(candidate_pos);
                    } else if i64::from(candidate_pos) > last_comp_pos {
                        last_comp_pos = i64::from(candidate_pos);
                    } else {
                        drop_ev = true;
                    }
                }
                4..=34 => {
                    let c = (ctrl - 4) / 8;
                    let sub = (ctrl - 4) % 8;
                    match sub {
                        0 => drop_ev = true, // v4.0 had no per-channel mixer mute switches
                        1..=4 => ctrl = 5 + 6 * c + sub,
                        5 => {
                            // PCF flag
                            ctrl = MIXER_CTRL_PCF_TARGET;
                            if val != 0 {
                                val = c + 1;
                                last_pcf_pos = i64::from(candidate_pos);
                            } else if i64::from(candidate_pos) > last_pcf_pos {
                                last_pcf_pos = i64::from(candidate_pos);
                            } else {
                                drop_ev = true;
                            }
                        }
                        6 => {
                            // Compressor flag
                            ctrl = MIXER_CTRL_COMP_TARGET;
                            if val != 0 {
                                val = c + 2;
                                last_comp_pos = i64::from(candidate_pos);
                            } else if i64::from(candidate_pos) > last_comp_pos {
                                last_comp_pos = i64::from(candidate_pos);
                            } else {
                                drop_ev = true;
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
            if candidate_pos >= MAX_RBS_POS {
                continue;
            }
            abs_pos = candidate_pos;
            if drop_ev || !valid_track_event(track_idx, ctrl, val) {
                continue;
            }
            out.push(TrackEvent {
                pos: abs_pos,
                ctrl,
                value: val,
            });
            continue;
        }

        // Pre-4.2 808 tom tuning transform applied prior to range validation.
        if track_idx == TRACK_808 && pre_4_2 && matches!(ctrl, 10 | 13 | 16) {
            val = (val / 2).wrapping_add(32);
        }

        if candidate_pos >= MAX_RBS_POS {
            continue;
        }
        abs_pos = candidate_pos;
        // Per-channel mixer enable/mute switches (`ctrls 5, 11, 17, 23`) were introduced in v4.2.
        if track_idx == TRACK_MIXER && pre_4_2 && is_mixer_ch_enable_ctrl(ctrl) {
            continue;
        }
        if !valid_track_event(track_idx, ctrl, val) {
            continue;
        }
        out.push(TrackEvent {
            pos: abs_pos,
            ctrl,
            value: val,
        });
    }
    // Stable sort by `pos` so `initial_ctrl` (which breaks early at `ev.pos > 0`) never
    // breaks on unsorted or wrapping-delta IFF TRAK streams while preserving same-pos order.
    out.sort_by_key(|ev| ev.pos);
    Ok(out)
}
