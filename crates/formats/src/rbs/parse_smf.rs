//! ReBirth 1.x Standard MIDI File (`MThd` + 7-bit-packed `TinY` SysEx) parser and header peek.

use super::controls::{
    mixer_ch_dist_ctrl, mixer_ch_level_ctrl, mixer_ch_pan_ctrl, mixer_ch_send_ctrl,
    DELAY_CTRL_ENABLE, DELAY_CTRL_FEEDBACK, DELAY_CTRL_MODE, DELAY_CTRL_PAN, DELAY_CTRL_STEPS,
    DEV_CTRL_ENABLE, DEV_CTRL_PATTERN, DIST_CTRL_AMOUNT, DIST_CTRL_ENABLE, MIXER_CTRL_MASTER_LEVEL,
    MIXER_CTRL_PCF_TARGET, PCF_CTRL_ENABLE, TRACK_808, TRACK_DELAY, TRACK_DIST, TRACK_MIXER,
    TRACK_PCF,
};
use super::model::{
    DrumPattern, MixerChannelInit, RbsHeaderPreview, RbsSong, RbsVersion, Tb303Pattern, Tb303Step,
    TrackEvent,
};
use super::{
    decode_zstring, read_u16_be, read_u32_be, sanitize_title, sanitize_title_opt, MAX_RBS_POS,
    POS_PER_BAR,
};
use anyhow::{ensure, Context, Result};
use cadence_core::material::MAX_BARS;

pub(super) fn peek_rbs_1x_header(bytes: &[u8], fallback: &str) -> Result<RbsHeaderPreview> {
    ensure!(bytes.len() >= 14, "SMF header truncated");
    let hdr_len = read_u32_be(bytes, 4)? as usize;
    let format = read_u16_be(bytes, 8)?;
    let ntrks = read_u16_be(bytes, 10)? as usize;
    ensure!(format == 1, "ReBirth 1.x songs must be MIDI format 1");

    let mut off = 8 + hdr_len;
    let mut midi_tracks: Vec<&[u8]> = Vec::with_capacity(ntrks);
    for _ in 0..ntrks {
        ensure!(off + 8 <= bytes.len(), "Truncated MTrk header");
        ensure!(&bytes[off..off + 4] == b"MTrk", "Expected MTrk chunk");
        let len = read_u32_be(bytes, off + 4)? as usize;
        off += 8;
        ensure!(off + len <= bytes.len(), "Truncated MTrk payload");
        midi_tracks.push(&bytes[off..off + len]);
        off += len;
    }

    let mut version = RbsVersion {
        major: 3,
        minor: 1,
        revision: 0,
    };
    let mut title = String::new();
    let mut song_mode = true;
    let mut loop_on = true;
    let mut bpm_millis = 120_000u32;
    let mut loop_end_bar = 4u32;
    let mut saw_sysex = false;

    'tracks: for trk in midi_tracks.iter().skip(1) {
        let mut cursor = 0usize;
        let mut running_status: u8 = 0;
        while cursor < trk.len() {
            let _delta = read_smf_vlq(trk, &mut cursor)?;
            let Some(&first) = trk.get(cursor) else {
                break;
            };
            let status = if first & 0x80 != 0 {
                cursor += 1;
                if first < 0xF0 {
                    running_status = first;
                }
                first
            } else {
                running_status
            };
            match status {
                0xFF => {
                    if cursor >= trk.len() {
                        break;
                    }
                    let meta_type = trk[cursor];
                    cursor += 1;
                    let len = read_smf_vlq(trk, &mut cursor)? as usize;
                    cursor = cursor.saturating_add(len).min(trk.len());
                    if meta_type == 0x2F {
                        break;
                    }
                }
                0xF0 | 0xF7 => {
                    let len = read_smf_vlq(trk, &mut cursor)? as usize;
                    let end = cursor.saturating_add(len).min(trk.len());
                    let data = &trk[cursor..end];
                    cursor = end;
                    if status == 0xF0
                        && data.len() >= 7
                        && &data[0..3] == b"\x00\x00\x66"
                        && data[4] != 0
                        && data[5] == 1
                        && data[6] == 0
                    {
                        let block = unpack_1x_sysex_block(&data[7..])?;
                        if block[0] == 0xBC {
                            version = RbsVersion {
                                major: block[1],
                                minor: block[2],
                                revision: block[3],
                            };
                        }
                        song_mode = block[0x12] != 0;
                        loop_on = block[0x2E] != 0;
                        bpm_millis = read_u32_be(&block, 0x2A)?;
                        let start_bar = read_u32_be(&block, 10)?;
                        let len_bars = read_u32_be(&block, 14)?;
                        loop_end_bar = start_bar.saturating_add(len_bars);
                        title = decode_zstring(&block[0x3160..0x3160 + 41]);
                        saw_sysex = true;
                        break 'tracks;
                    }
                }
                s if matches!(s & 0xF0, 0xC0 | 0xD0) => {
                    cursor = (cursor + 1).min(trk.len());
                }
                s if matches!(s & 0xF0, 0x80 | 0x90 | 0xA0 | 0xB0 | 0xE0) => {
                    cursor = (cursor + 2).min(trk.len());
                }
                _ => break,
            }
        }
    }

    ensure!(
        saw_sysex,
        "ReBirth 1.x MIDI file is missing song SysEx block"
    );
    let bars = if !song_mode {
        4
    } else if loop_on && loop_end_bar > 0 {
        (loop_end_bar as usize).clamp(1, MAX_BARS)
    } else {
        // Unlooped 1.x Song Mode: scan only MIDI CC timestamps (no pattern decoding!).
        let mut max_pos: Option<u32> = None;
        for (trk_idx, trk) in midi_tracks.iter().enumerate().skip(1) {
            let mut pos: u32 = 0;
            let mut cursor = 0usize;
            let mut running_status: u8 = 0;
            while cursor < trk.len() {
                let delta = read_smf_vlq(trk, &mut cursor)?;
                pos = pos.wrapping_add(delta);
                let Some(&first) = trk.get(cursor) else {
                    break;
                };
                let status = if first & 0x80 != 0 {
                    cursor += 1;
                    if first < 0xF0 {
                        running_status = first;
                    }
                    first
                } else {
                    running_status
                };
                match status {
                    0xFF => {
                        if cursor >= trk.len() {
                            break;
                        }
                        let meta_type = trk[cursor];
                        cursor += 1;
                        let len = read_smf_vlq(trk, &mut cursor)? as usize;
                        cursor = cursor.saturating_add(len).min(trk.len());
                        if meta_type == 0x2F {
                            break;
                        }
                    }
                    0xF0 | 0xF7 => {
                        let len = read_smf_vlq(trk, &mut cursor)? as usize;
                        cursor = cursor.saturating_add(len).min(trk.len());
                    }
                    s if (s & 0xF0) == 0xB0 => {
                        if cursor + 2 > trk.len() {
                            break;
                        }
                        cursor += 2;
                        if pos < MAX_RBS_POS && (2..=5).contains(&trk_idx) {
                            max_pos = Some(max_pos.map_or(pos, |m| m.max(pos)));
                        }
                    }
                    s if matches!(s & 0xF0, 0xC0 | 0xD0) => {
                        cursor = (cursor + 1).min(trk.len());
                    }
                    s if matches!(s & 0xF0, 0x80 | 0x90 | 0xA0 | 0xE0) => {
                        cursor = (cursor + 2).min(trk.len());
                    }
                    _ => break,
                }
            }
        }
        max_pos
            .map_or(4, |p| ((p.min(MAX_RBS_POS) / POS_PER_BAR) + 1) as usize)
            .clamp(1, MAX_BARS)
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

pub(super) fn parse_rbs_1x_smf(bytes: &[u8]) -> Result<RbsSong> {
    ensure!(bytes.len() >= 14, "SMF header truncated");
    let hdr_len = read_u32_be(bytes, 4)? as usize;
    let format = read_u16_be(bytes, 8)?;
    let ntrks = read_u16_be(bytes, 10)? as usize;
    ensure!(format == 1, "ReBirth 1.x songs must be MIDI format 1");

    let mut off = 8 + hdr_len;
    let mut midi_tracks: Vec<&[u8]> = Vec::with_capacity(ntrks);
    for _ in 0..ntrks {
        ensure!(off + 8 <= bytes.len(), "Truncated MTrk header");
        ensure!(&bytes[off..off + 4] == b"MTrk", "Expected MTrk chunk");
        let len = read_u32_be(bytes, off + 4)? as usize;
        off += 8;
        ensure!(off + len <= bytes.len(), "Truncated MTrk payload");
        midi_tracks.push(&bytes[off..off + len]);
        off += len;
    }

    let mut song = RbsSong {
        version: RbsVersion {
            major: 3,
            minor: 1,
            revision: 0,
        },
        vintage: true,
        ..RbsSong::default()
    };
    // ReBirth 1.x has no TR-909 (omitted during conversion when `version.major < 4`).
    song.tr909.params[0] = 0;

    let mut saw_sysex = false;
    for (trk_idx, trk) in midi_tracks.iter().enumerate().skip(1) {
        let mut pos: u32 = 0;
        let mut cursor = 0usize;
        let mut running_status: u8 = 0;
        while cursor < trk.len() {
            let delta = read_smf_vlq(trk, &mut cursor)?;
            pos = pos.wrapping_add(delta);
            let Some(&first) = trk.get(cursor) else {
                break;
            };
            let status = if first & 0x80 != 0 {
                cursor += 1;
                if first < 0xF0 {
                    running_status = first;
                }
                first
            } else {
                running_status
            };
            match status {
                0xFF => {
                    // Meta event
                    if cursor >= trk.len() {
                        break;
                    }
                    let meta_type = trk[cursor];
                    cursor += 1;
                    let len = read_smf_vlq(trk, &mut cursor)? as usize;
                    cursor = cursor.saturating_add(len).min(trk.len());
                    if meta_type == 0x2F {
                        break;
                    }
                }
                0xF0 | 0xF7 => {
                    let len = read_smf_vlq(trk, &mut cursor)? as usize;
                    let end = cursor.saturating_add(len).min(trk.len());
                    let data = &trk[cursor..end];
                    cursor = end;
                    if status == 0xF0
                        && data.len() >= 7
                        && &data[0..3] == b"\x00\x00\x66"
                        && data[4] != 0
                        && data[5] == 1
                        && data[6] == 0
                    {
                        decode_1x_sysex(&data[7..], &mut song)?;
                        saw_sysex = true;
                    }
                }
                s if (s & 0xF0) == 0xB0 => {
                    if cursor + 2 > trk.len() {
                        break;
                    }
                    let cc = trk[cursor];
                    let val = trk[cursor + 1];
                    cursor += 2;
                    if pos < MAX_RBS_POS {
                        map_1x_cc(trk_idx, pos, cc, val, &mut song);
                    }
                }
                s if matches!(s & 0xF0, 0xC0 | 0xD0) => {
                    cursor = (cursor + 1).min(trk.len());
                }
                s if matches!(s & 0xF0, 0x80 | 0x90 | 0xA0 | 0xE0) => {
                    cursor = (cursor + 2).min(trk.len());
                }
                _ => break,
            }
        }
    }

    ensure!(
        saw_sysex,
        "ReBirth 1.x MIDI file is missing song SysEx block"
    );
    for track in &mut song.tracks {
        track.sort_by_key(|ev| ev.pos);
    }
    Ok(song)
}

fn read_smf_vlq(buf: &[u8], cursor: &mut usize) -> Result<u32> {
    let mut val: u32 = 0;
    for _ in 0..5 {
        let byte = *buf.get(*cursor).context("Truncated SMF VLQ")?;
        *cursor += 1;
        val = (val << 7) | u32::from(byte & 0x7F);
        if byte & 0x80 == 0 {
            return Ok(val);
        }
    }
    Ok(val)
}

fn unpack_1x_sysex_block(packed: &[u8]) -> Result<Vec<u8>> {
    let mut block = Vec::with_capacity(0x4000);
    let mut i = 0usize;
    while block.len() < 0x4000 && i + 8 <= packed.len() {
        let msbs = packed[i + 7];
        for k in 0..7 {
            if block.len() < 0x4000 {
                let bit7 = ((msbs >> (6 - k)) & 1) << 7;
                block.push((packed[i + k] & 0x7F) | bit7);
            }
        }
        i += 8;
    }
    ensure!(
        block.len() >= 0x3300 && &block[5..9] == b"TinY",
        "Invalid ReBirth 1.x SysEx payload (missing 'TinY' signature)"
    );
    Ok(block)
}

fn decode_1x_sysex(packed: &[u8], song: &mut RbsSong) -> Result<()> {
    let block = unpack_1x_sysex_block(packed)?;
    if block[0] == 0xBC {
        song.version = RbsVersion {
            major: block[1],
            minor: block[2],
            revision: block[3],
        };
    }

    song.song_mode = block[0x12] != 0;
    song.loop_on = block[0x2E] != 0;
    song.bpm_millis = read_u32_be(&block, 0x2A)?;
    let start_bar = read_u32_be(&block, 10)?;
    let len_bars = read_u32_be(&block, 14)?;
    song.loop_start_bar = start_bar;
    song.loop_end_bar = start_bar.saturating_add(len_bars);
    song.shuffle = 0; // ReBirth 1.x songs had no shuffle

    song.mixer.master_level = block[9].min(127);
    song.mixer.comp_target = 0;
    song.mixer.pcf_target = match block[0x40] {
        0 => 1,
        1 => 2,
        2 => 3,
        _ => 0,
    };
    let dist_chan = block[0x37];
    for (c, ch) in song.mixer.channels.iter_mut().take(3).enumerate() {
        let r = 0x100 + 32 * c;
        *ch = MixerChannelInit {
            enabled: block[r + 0x17] != 0,
            level: block[r + 2].min(127),
            pan: block[r + 3].min(127),
            delay_send: block[r + 4].min(127),
            dist_on: dist_chan == c as u8,
        };
    }
    song.mixer.channels[3] = MixerChannelInit {
        enabled: false,
        ..MixerChannelInit::default()
    };

    song.delay = [
        block[0x35],
        (block[0x2F] & 0x7F).clamp(1, 32),
        u8::from(block[0x2F] & 0x80 != 0),
        block[0x33].min(127),
        block[0x34].min(127),
    ];
    song.dist = [block[0x38], block[0x36].min(127), 0];
    song.pcf = [
        block[0x3F],
        block[0x39].min(127),
        block[0x3A].min(127),
        block[0x3B].min(127),
        block[0x3C],
        block[0x3D].min(127),
        block[0x3E],
    ];

    // Two TB-303 units
    for (j, dev) in song.tb303.iter_mut().enumerate() {
        let r = 0x100 + 32 * j;
        let p = 352 + 4096 * j;
        dev.params[0] = u8::from(block[r + 5] == 0);
        dev.params[1] = block[r + 1].min(31);
        dev.params[2..9].copy_from_slice(&block[p + 16..p + 23]);
        dev.patterns.clear();
        for q in 0..32 {
            let len_byte = block[r + 7 + (q / 2)];
            let len = if q % 2 == 0 {
                (len_byte >> 4) + 1
            } else {
                (len_byte & 0x0F) + 1
            }
            .clamp(1, 16);
            let mut pat = Tb303Pattern {
                shuffle: false,
                length: len,
                steps: [Tb303Step::default(); 16],
            };
            for (s, step) in pat.steps.iter_mut().enumerate() {
                let so = p + 30 + 68 * q + 4 * s;
                let mut note = block[so];
                let f = block[so + 2];
                let mut up = f & 0x04 != 0;
                if note > 12 {
                    note = note.saturating_sub(12).min(12);
                    up = true;
                }
                *step = Tb303Step {
                    note: note.min(12),
                    slide: f & 0x01 != 0,
                    accent: f & 0x02 != 0,
                    up,
                    down: f & 0x08 != 0,
                    rest: block[so + 3] != 0,
                };
            }
            dev.patterns.push(pat);
        }
    }

    // TR-808 unit
    let r = 0x140;
    let p = 8544;
    song.tr808.params[0] = u8::from(block[r + 5] == 0);
    song.tr808.params[1] = block[r + 1].min(31);
    song.tr808.params[2..29].copy_from_slice(&block[p + 16..p + 43]);
    for idx in [10, 13, 16] {
        let signed = song.tr808.params[idx] as i8;
        song.tr808.params[idx] = ((signed / 2) as i16 + 64).clamp(0, 127) as u8;
    }
    song.tr808.patterns.clear();
    for q in 0..32 {
        let len_byte = block[r + 7 + (q / 2)];
        let len = if q % 2 == 0 {
            (len_byte >> 4) + 1
        } else {
            (len_byte & 0x0F) + 1
        }
        .clamp(1, 16);
        let mut pat = DrumPattern {
            shuffle: false,
            length: len,
            steps: [[0; 12]; 16],
        };
        let po = p + 48 + 64 * q;
        for (s, step_voices) in pat.steps.iter_mut().enumerate() {
            for (voice, cell) in step_voices.iter_mut().enumerate() {
                let idx = s + 16 * voice;
                let byte = block[po + idx / 4];
                let shift = 6 - 2 * (idx % 4);
                let val = (byte >> shift) & 0x03;
                *cell = u8::from(val != 0);
            }
        }
        song.tr808.patterns.push(pat);
    }

    song.title = sanitize_title_opt(&decode_zstring(&block[0x3160..0x3160 + 41]));
    Ok(())
}

/// Map ReBirth 1.x SMF MIDI CCs onto the canonical 9-track [`RbsSong`] event layout.
///
/// ReBirth 1.x stores initial per-channel mixer enable flags statically in the SysEx block
/// (`block[r + 0x17]`), while Track 5 (Global FX / Master) standard MIDI CC 7 captures Master
/// Level automation when present.
pub(super) fn map_1x_cc(midi_track: usize, pos: u32, cc: u8, val: u8, song: &mut RbsSong) {
    match midi_track {
        2..=4 => {
            let c = midi_track - 2;
            let dev_track = midi_track - 1; // 1=303#1, 2=303#2, 3=808
            match cc {
                7 => song.tracks[TRACK_MIXER].push(TrackEvent {
                    pos,
                    ctrl: mixer_ch_level_ctrl(c),
                    value: val.min(127),
                }),
                10 => song.tracks[TRACK_MIXER].push(TrackEvent {
                    pos,
                    ctrl: mixer_ch_pan_ctrl(c),
                    value: val.min(127),
                }),
                12 => song.tracks[TRACK_MIXER].push(TrackEvent {
                    pos,
                    ctrl: mixer_ch_send_ctrl(c),
                    value: val.min(127),
                }),
                14 => song.tracks[dev_track].push(TrackEvent {
                    pos,
                    ctrl: DEV_CTRL_PATTERN,
                    value: val.min(31),
                }),
                15 => song.tracks[dev_track].push(TrackEvent {
                    pos,
                    ctrl: DEV_CTRL_ENABLE,
                    value: u8::from(val != 0),
                }),
                16..=22 if midi_track <= 3 => song.tracks[dev_track].push(TrackEvent {
                    pos,
                    ctrl: cc - 14,
                    value: val.min(127),
                }),
                16..=42 if midi_track == 4 => {
                    let ctrl = cc - 14;
                    let mapped = if matches!(ctrl, 10 | 13 | 16) {
                        // ReBirth 1.x Track 4 TR-808 tom tuning CCs encode a signed offset
                        // centered at 64 (`(val as i8) / 2 + 64`), matching initial tom tuning
                        // in `decode_1x_sysex`.
                        let signed = val as i8;
                        ((signed / 2) as i16 + 64).clamp(0, 127) as u8
                    } else {
                        val.min(127)
                    };
                    song.tracks[TRACK_808].push(TrackEvent {
                        pos,
                        ctrl,
                        value: mapped,
                    });
                }
                _ => {}
            }
        }
        5 => match cc {
            7 => song.tracks[TRACK_MIXER].push(TrackEvent {
                pos,
                ctrl: MIXER_CTRL_MASTER_LEVEL,
                value: val.min(127),
            }),
            16 => song.tracks[TRACK_DIST].push(TrackEvent {
                pos,
                ctrl: DIST_CTRL_AMOUNT,
                value: val.min(127),
            }),
            17 => {
                // ReBirth 1.x only has 3 channels (0=303A, 1=303B, 2=808; 3=off); TR-909 dist is always 0.
                for c in 0..4usize {
                    song.tracks[TRACK_MIXER].push(TrackEvent {
                        pos,
                        ctrl: mixer_ch_dist_ctrl(c),
                        value: u8::from(c < 3 && val == c as u8),
                    });
                }
            }
            18 => song.tracks[TRACK_DIST].push(TrackEvent {
                pos,
                ctrl: DIST_CTRL_ENABLE,
                value: u8::from(val != 0),
            }),
            19 => song.tracks[TRACK_DELAY].push(TrackEvent {
                pos,
                ctrl: DELAY_CTRL_STEPS,
                value: val.clamp(1, 32),
            }),
            20 => song.tracks[TRACK_DELAY].push(TrackEvent {
                pos,
                ctrl: DELAY_CTRL_FEEDBACK,
                value: val.min(127),
            }),
            21 => song.tracks[TRACK_DELAY].push(TrackEvent {
                pos,
                ctrl: DELAY_CTRL_PAN,
                value: val.min(127),
            }),
            22 => song.tracks[TRACK_DELAY].push(TrackEvent {
                pos,
                ctrl: DELAY_CTRL_ENABLE,
                value: u8::from(val != 0),
            }),
            23 => song.tracks[TRACK_DELAY].push(TrackEvent {
                pos,
                ctrl: DELAY_CTRL_MODE,
                value: u8::from(val != 0),
            }),
            24..=29 => song.tracks[TRACK_PCF].push(TrackEvent {
                pos,
                ctrl: cc - 23,
                value: val.min(127),
            }),
            30 => song.tracks[TRACK_PCF].push(TrackEvent {
                pos,
                ctrl: PCF_CTRL_ENABLE,
                value: u8::from(val != 0),
            }),
            31 => {
                let target = match val {
                    0 => 1,
                    1 => 2,
                    2 => 3,
                    _ => 0,
                };
                song.tracks[TRACK_MIXER].push(TrackEvent {
                    pos,
                    ctrl: MIXER_CTRL_PCF_TARGET,
                    value: target,
                });
            }
            _ => {}
        },
        _ => {}
    }
}
