//! ReBirth RB-338 parsed song data model, version tags, and import summary types.

use super::{MAX_RBS_POS, POS_PER_BAR};
use cadence_core::material::MAX_BARS;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RbsVersion {
    pub major: u8,
    pub minor: u8,
    pub revision: u8,
}

impl RbsVersion {
    pub(super) fn key(self) -> u32 {
        (u32::from(self.major) << 16) | (u32::from(self.minor) << 8) | u32::from(self.revision)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Tb303Step {
    pub note: u8, // 0..=12 (C..C+12)
    pub slide: bool,
    pub accent: bool,
    pub up: bool,
    pub down: bool,
    pub rest: bool,
}

#[derive(Clone, Debug)]
pub struct Tb303Pattern {
    pub shuffle: bool,
    pub length: u8, // 1..=16
    pub steps: [Tb303Step; 16],
}

impl Default for Tb303Pattern {
    fn default() -> Self {
        Self {
            shuffle: false,
            length: 16,
            steps: [Tb303Step {
                rest: true,
                ..Default::default()
            }; 16],
        }
    }
}

#[derive(Clone, Debug)]
pub struct Tb303Device {
    /// `[enabled, pattern, tune, cutoff, res, envmod, decay, accent, waveform]`
    pub params: [u8; 9],
    pub patterns: Vec<Tb303Pattern>, // 32 patterns
}

impl Default for Tb303Device {
    fn default() -> Self {
        Self {
            params: [1, 0, 64, 20, 90, 20, 100, 64, 0],
            patterns: vec![Tb303Pattern::default(); 32],
        }
    }
}

#[derive(Clone, Debug)]
pub struct DrumPattern {
    pub shuffle: bool,
    pub length: u8,
    /// `steps[step][voice]`: 808 uses 0/1; 909 uses 0=off, 1=on, 2=accent, 3=flam.
    /// Column 0 is always Accent (`AC`).
    pub steps: [[u8; 12]; 16],
}

impl Default for DrumPattern {
    fn default() -> Self {
        Self {
            shuffle: false,
            length: 16,
            steps: [[0; 12]; 16],
        }
    }
}

#[derive(Clone, Debug)]
pub struct Tr808Device {
    pub params: [u8; 29],
    pub patterns: Vec<DrumPattern>,
}

impl Default for Tr808Device {
    fn default() -> Self {
        Self {
            params: [
                1, 0, 40, 100, 64, 64, 100, 64, 64, 100, 64, 0, 100, 64, 0, 100, 64, 0, 100, 0,
                100, 0, 100, 100, 0, 100, 100, 100, 100,
            ],
            patterns: vec![DrumPattern::default(); 32],
        }
    }
}

#[derive(Clone, Debug)]
pub struct Tr909Device {
    pub params: [u8; 30],
    pub patterns: Vec<DrumPattern>,
}

impl Default for Tr909Device {
    fn default() -> Self {
        Self {
            params: [
                1, 0, 32, 100, 64, 64, 127, 100, 64, 64, 64, 100, 64, 127, 100, 64, 127, 100, 64,
                127, 100, 100, 100, 32, 127, 100, 64, 100, 64, 64,
            ],
            patterns: vec![DrumPattern::default(); 32],
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MixerChannelInit {
    pub enabled: bool,
    pub level: u8,
    pub pan: u8,
    pub delay_send: u8,
    pub dist_on: bool,
}

impl Default for MixerChannelInit {
    fn default() -> Self {
        Self {
            enabled: true,
            level: 100,
            pan: 64,
            delay_send: 0,
            dist_on: false,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MixerInit {
    pub master_level: u8,
    /// `0=off, 1=master, 2=303#1, 3=303#2, 4=808, 5=909`
    pub comp_target: u8,
    /// `0=off, 1=303#1, 2=303#2, 3=808, 4=909`
    pub pcf_target: u8,
    pub channels: [MixerChannelInit; 4],
}

impl Default for MixerInit {
    fn default() -> Self {
        Self {
            master_level: 100,
            comp_target: 0,
            pcf_target: 0,
            channels: [MixerChannelInit::default(); 4],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrackEvent {
    /// Position in 32nd notes (`32` per bar, `2` per 16th-note step).
    pub pos: u32,
    pub ctrl: u8,
    pub value: u8,
}

/// Parsed ReBirth RB-338 song independent of binary container (v4.x IFF or v1.x SMF).
#[derive(Clone, Debug)]
pub struct RbsSong {
    pub version: RbsVersion,
    pub title: String,
    pub mod_name: String,
    pub song_mode: bool,
    pub loop_on: bool,
    pub bpm_millis: u32,
    pub loop_start_bar: u32,
    pub loop_end_bar: u32,
    pub shuffle: u8,
    pub vintage: bool,
    pub mixer: MixerInit,
    /// `[enabled, steps(1..=32), mode(0=triplets,1=16ths), feedback, pan]`
    pub delay: [u8; 5],
    /// `[enabled, freq, res, amount, wave, decay, mode(0=BP,1=LP)]`
    pub pcf: [u8; 7],
    /// `[enabled, amount, shape]`
    pub dist: [u8; 3],
    /// `[enabled, amount, threshold]`
    pub comp: [u8; 3],
    pub tb303: [Tb303Device; 2],
    pub tr808: Tr808Device,
    pub tr909: Tr909Device,
    /// 9 automation tracks: `0=Mixer, 1=303#1, 2=303#2, 3=808, 4=909, 5=Delay, 6=Dist, 7=PCF, 8=Comp`.
    pub tracks: [Vec<TrackEvent>; 9],
}

impl Default for RbsSong {
    fn default() -> Self {
        Self {
            version: RbsVersion {
                major: 4,
                minor: 2,
                revision: 0,
            },
            title: String::new(),
            mod_name: "Standard ReBirth".into(),
            song_mode: true,
            loop_on: true,
            bpm_millis: 120_000,
            loop_start_bar: 0,
            loop_end_bar: 4,
            shuffle: 0,
            vintage: false,
            mixer: MixerInit::default(),
            delay: [1, 3, 1, 80, 64],
            pcf: [1, 70, 64, 40, 24, 90, 1],
            dist: [1, 70, 0],
            comp: [1, 70, 64],
            tb303: [Tb303Device::default(), Tb303Device::default()],
            tr808: Tr808Device::default(),
            tr909: Tr909Device::default(),
            tracks: std::array::from_fn(|_| Vec::new()),
        }
    }
}

/// Summary metadata reported after importing an `.rbs` song.
#[derive(Clone, Debug)]
pub struct RbsImportSummary {
    pub title: String,
    pub version: RbsVersion,
    pub bpm: f64,
    pub bars: usize,
    pub song_mode: bool,
    pub patterns: usize,
    pub placements: usize,
    pub modulations: usize,
    /// Non-fatal preset fallback notices (e.g. when external factory presets were removed).
    pub notices: Vec<String>,
}

impl RbsImportSummary {
    /// Canonical user-facing status summary line shared by CLI (`src/main.rs`) and `App` (`apply/session.rs`).
    pub fn status_line(&self) -> String {
        let base = format!(
            "Imported ReBirth v{}.{} \"{}\" · {} bars @ {:.1} BPM · {} pat · {} mod",
            self.version.major,
            self.version.minor,
            self.title,
            self.bars,
            self.bpm,
            self.patterns,
            self.modulations,
        );
        if self.notices.is_empty() {
            base
        } else {
            format!("{base} · notice: {}", self.notices.join("; "))
        }
    }
}

/// Lightweight metadata preview for the File Hub (`gf`) without running full conversion.
#[derive(Clone, Debug)]
pub struct RbsHeaderPreview {
    pub title: String,
    pub version: RbsVersion,
    pub bpm: f64,
    pub bars: usize,
    pub song_mode: bool,
}

impl RbsSong {
    /// Retrieve the effective initial value of `(track, ctrl)` at tick 0 (song mode uses
    /// track tick-0 override if present, falling back to `fallback`).
    pub(super) fn initial_ctrl(&self, track: usize, ctrl: u8, fallback: u8) -> u8 {
        if !self.song_mode {
            return fallback;
        }
        let mut value = None;
        for ev in &self.tracks[track] {
            if ev.pos == 0 && ev.ctrl == ctrl {
                value = Some(ev.value);
            } else if ev.pos > 0 {
                break;
            }
        }
        value.unwrap_or(fallback)
    }

    /// Build a step-indexed breakpoint timeline (`step -> raw u8`) across `0..total_steps`.
    ///
    /// ReBirth `TRAK` / SMF positions run at 32nd-note resolution (`2` ticks per 16th-note step).
    /// Mapping `step = ev.pos.div_ceil(2)` satisfies `step <= s <==> ev.pos <= 2 * s`, so:
    /// - an on-beat event at `2 * s` always takes effect on step `s` and is never overwritten by
    ///   a mid-step event at `2 * s + 1`;
    /// - a mid-step event at `2 * s + 1` takes effect on step `s + 1` unless superseded by an
    ///   explicit on-beat event at `2 * (s + 1)` (which follows it in sorted `pos` order).
    pub(super) fn ctrl_series(
        &self,
        track: usize,
        ctrl: u8,
        init: u8,
        total_steps: u32,
    ) -> BTreeMap<u32, u8> {
        let mut map = BTreeMap::new();
        map.insert(0, init);
        if !self.song_mode {
            return map;
        }
        for ev in &self.tracks[track] {
            if ev.ctrl == ctrl {
                let step = ev.pos.div_ceil(2);
                if step < total_steps {
                    map.insert(step, ev.value);
                }
            }
        }
        map
    }

    /// Effective song length in bars shared by both `peek_rbs_header` and full import.
    pub(super) fn song_bars(&self) -> usize {
        if !self.song_mode {
            return 4;
        }
        if self.loop_on && self.loop_end_bar > 0 {
            return (self.loop_end_bar as usize).clamp(1, MAX_BARS);
        }
        let max_ev_bar = self
            .tracks
            .iter()
            .flat_map(|t| t.iter())
            .map(|ev| (ev.pos.min(MAX_RBS_POS) / POS_PER_BAR) + 1)
            .max()
            .unwrap_or(4);
        (max_ev_bar as usize).clamp(1, MAX_BARS)
    }
}
