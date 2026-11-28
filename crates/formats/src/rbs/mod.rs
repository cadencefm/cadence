//! Clean-room MIT ReBirth RB-338 `.rbs` binary parser and Cadence `Project` importer.
//!
//! Supports both:
//! - ReBirth 2.0 IFF songs (`CAT .... RB40`, file versions 4.0, 4.1, and 4.2)
//! - ReBirth 1.x Standard MIDI File songs (`MThd` with 7-bit-packed `TinY` SysEx block)
//!
//! Converts an `.rbs` song into a clean, validated native Cadence `Project`:
//! - 3 instruments & 3 lanes on ReBirth 1.x (`TB-303 A`, `TB-303 B`, `TR-808`), or 4 on
//!   ReBirth 2.0 (adding `TR-909`), built as `RackPatch` Acid TB-303 synths (`MobiusJet`/
//!   `MobiusPwm` VCO, `DiodeLadder` filter with velocity accent, `Capacitor` filter envelope,
//!   and `panel_pins`) and multi-zone drum `Rack` kits indexed by `drum_midi`
//! - Deduplicated shared patterns, derived `Part.transpose` for transposed 303 motifs,
//!   full non-empty bank `lane.parts` launchpads, dense `>6`-voice drum `PatternKind::Stack`
//!   overflow layers, BPM-accurate TR-909 flams, and Global Shuffle via `PatternKind::Swing`
//! - `Synths` and `Drums` Group busses, `Echo` Return bus driven via standard post-insert
//!   `ChannelSend` + `ModulationTarget::Send`, `Drive`, Pattern Controlled Filter (`Filter`),
//!   and Master/channel `Compressor`
//! - Musical section detection with loop-region preservation, event-driven sub-bar Song Mode
//!   placements with cross-bar TB-303 slide lookahead, and live Master/channel mute & gain,
//!   knob/`ChannelPan`/`Send`/FX `Values` automation curves (leaving `macros` empty so direct
//!   Instrument/Effect knob edits take effect immediately)

mod automation;
mod calibration;
mod controls;
mod convert;
mod model;
mod parse_iff;
mod parse_smf;
mod patterns;
mod sections;

#[cfg(test)]
mod tests;

pub use model::{
    DrumPattern, MixerChannelInit, MixerInit, RbsHeaderPreview, RbsImportSummary, RbsSong,
    RbsVersion, Tb303Device, Tb303Pattern, Tb303Step, Tr808Device, Tr909Device, TrackEvent,
};

use anyhow::{bail, ensure, Context, Result};
use cadence_core::config::UserConfig;
use cadence_core::material::Project;
use cadence_core::presets::PresetCatalog;
use parse_iff::{parse_rbs_iff, peek_rbs_iff_header};
use parse_smf::{parse_rbs_1x_smf, peek_rbs_1x_header};
use std::path::Path;

pub(super) const TICKS_PER_BAR: u32 = 768;
pub(super) const POS_PER_BAR: u32 = 32; // 32nd-note resolution in TRAK chunks (2 positions per 16th step)
pub(super) const MAX_RBS_POS: u32 = 32_000; // 1000 bars × 32

/// Maximum `.rbs` file size accepted when reading from disk (16 MiB).
pub const MAX_RBS_BYTES: u64 = 16 * 1024 * 1024;

/// Inspect an `.rbs` file's header metadata quickly for UI previews without decoding
/// device patterns (`DEVL`) or full automation tracks (`TRKL` / MIDI CCs).
pub fn peek_rbs_header(path: &Path) -> Result<RbsHeaderPreview> {
    let bytes = cadence_core::store::read_bounded(path, MAX_RBS_BYTES)
        .with_context(|| format!("Failed to read ReBirth song {}", path.display()))?;
    let fallback = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("ReBirth Song");
    peek_rbs_header_bytes(&bytes, fallback)
}

pub fn peek_rbs_header_bytes(bytes: &[u8], fallback: &str) -> Result<RbsHeaderPreview> {
    ensure!(
        bytes.len() >= 12,
        "File is too small to be a ReBirth .rbs song"
    );
    if &bytes[0..4] == b"CAT " {
        peek_rbs_iff_header(bytes, fallback)
    } else if &bytes[0..4] == b"MThd" {
        peek_rbs_1x_header(bytes, fallback)
    } else {
        bail!("Unsupported .rbs header (expected IFF 'CAT RB40' or ReBirth 1.x 'MThd')");
    }
}

pub fn import_rbs_file(path: &Path) -> Result<(Project, RbsImportSummary)> {
    let catalog = PresetCatalog::discover(&[], Some(path), &UserConfig::load_default());
    import_rbs_file_with_presets(path, &catalog)
}

pub fn import_rbs_file_with_presets(
    path: &Path,
    catalog: &PresetCatalog,
) -> Result<(Project, RbsImportSummary)> {
    let bytes = cadence_core::store::read_bounded(path, MAX_RBS_BYTES)
        .with_context(|| format!("Failed to read ReBirth song {}", path.display()))?;
    let fallback = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("ReBirth Song");
    import_rbs_bytes_with_presets(&bytes, fallback, catalog)
}

pub fn import_rbs_bytes(bytes: &[u8], fallback_title: &str) -> Result<(Project, RbsImportSummary)> {
    let catalog = PresetCatalog::discover(&[], None, &UserConfig::load_default());
    import_rbs_bytes_with_presets(bytes, fallback_title, &catalog)
}

pub fn import_rbs_bytes_with_presets(
    bytes: &[u8],
    fallback_title: &str,
    catalog: &PresetCatalog,
) -> Result<(Project, RbsImportSummary)> {
    let song = parse_rbs(bytes)?;
    let (project, notices) = song.to_cadence_project_with_presets(fallback_title, catalog)?;
    let placements: usize = project.lanes.iter().map(|l| l.placements.len()).sum();
    let summary = RbsImportSummary {
        title: project.title.clone(),
        version: song.version,
        bpm: project.bpm,
        bars: project.bars,
        song_mode: song.song_mode,
        patterns: project.patterns.len(),
        placements,
        modulations: project.modulations.len(),
        notices,
    };
    Ok((project, summary))
}

/// Parse an `.rbs` binary stream (ReBirth 2.0 IFF `CAT RB40` or ReBirth 1.x `MThd`).
pub fn parse_rbs(bytes: &[u8]) -> Result<RbsSong> {
    ensure!(
        bytes.len() >= 12,
        "File is too small to be a ReBirth .rbs song"
    );
    if &bytes[0..4] == b"CAT " {
        parse_rbs_iff(bytes)
    } else if &bytes[0..4] == b"MThd" {
        parse_rbs_1x_smf(bytes)
    } else {
        bail!("Unsupported .rbs header (expected IFF 'CAT RB40' or ReBirth 1.x 'MThd')");
    }
}

pub(super) fn read_u32_be(b: &[u8], off: usize) -> Result<u32> {
    let slice = b
        .get(off..off + 4)
        .context("Unexpected EOF reading big-endian u32")?;
    Ok(u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

pub(super) fn read_u16_be(b: &[u8], off: usize) -> Result<u16> {
    let slice = b
        .get(off..off + 2)
        .context("Unexpected EOF reading big-endian u16")?;
    Ok(u16::from_be_bytes([slice[0], slice[1]]))
}

pub(super) fn decode_zstring(slice: &[u8]) -> String {
    let end = slice.iter().position(|&c| c == 0).unwrap_or(slice.len());
    // ReBirth songs use Mac/Windows-1252 / Latin-1 characters (such as `©`).
    slice[..end]
        .iter()
        .map(|&b| char::from(b))
        .collect::<String>()
        .trim()
        .to_string()
}

pub(super) fn sanitize_title_opt(raw: &str) -> String {
    raw.chars()
        .filter(|c| !c.is_control())
        .take(64)
        .collect::<String>()
        .trim()
        .to_string()
}

pub(super) fn sanitize_title(raw: &str, fallback: &str) -> String {
    let cleaned = sanitize_title_opt(raw);
    if cleaned.is_empty() {
        let fb = sanitize_title_opt(fallback);
        if fb.is_empty() {
            "ReBirth Song".into()
        } else {
            fb
        }
    } else {
        cleaned
    }
}
