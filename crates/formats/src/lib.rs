//! Feature-gated third-party song format importers and exporters for Cadence.
//!
//! Each format lives in its own isolated submodule behind an explicit Cargo
//! feature (e.g., `rbs` for clean-room ReBirth RB-338 `.rbs` import) and
//! constructs strictly public [`cadence_core::material::Project`] values with
//! zero runtime hooks into `cadence-core` or `cadence-engine`.
//!
//! Host and UI layers can route through the format-agnostic façade in this
//! module so adding future formats (such as Standard MIDI `.mid`) does not
//! require scattering per-format `#[cfg]` ladders across `cadence-app`.

#[cfg(feature = "rbs")]
pub mod rbs;

use anyhow::{bail, Result};
use cadence_core::material::Project;
use cadence_core::presets::PresetCatalog;
use std::path::Path;

/// Static metadata describing a supported external song import format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImportFormatSpec {
    pub id: &'static str,
    pub label: &'static str,
    pub badge: &'static str,
    pub extensions: &'static [&'static str],
    pub max_bytes: u64,
}

/// Result of importing an external format into a native Cadence [`Project`].
#[derive(Clone, Debug)]
pub struct ImportedProject {
    pub format_id: &'static str,
    pub project: Project,
    pub status_line: String,
    pub notices: Vec<String>,
}

#[cfg(feature = "rbs")]
const RBS_SPEC: ImportFormatSpec = ImportFormatSpec {
    id: "rbs",
    label: "ReBirth .rbs",
    badge: "RBS",
    extensions: &["rbs"],
    max_bytes: rbs::MAX_RBS_BYTES,
};

const SUPPORTED_IMPORT_FORMATS: &[ImportFormatSpec] = &[
    #[cfg(feature = "rbs")]
    RBS_SPEC,
];

/// Return all external song import formats enabled in this build.
pub fn supported_import_formats() -> &'static [ImportFormatSpec] {
    SUPPORTED_IMPORT_FORMATS
}

/// Look up an enabled import format by file extension (case-insensitive, without leading dot).
pub fn find_import_format_by_ext(ext: &str) -> Option<&'static ImportFormatSpec> {
    let trimmed = ext.trim_start_matches('.');
    SUPPORTED_IMPORT_FORMATS.iter().find(|spec| {
        spec.extensions
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(trimmed))
    })
}

/// Look up an enabled import format for `path` from its file extension.
pub fn find_import_format_for_path(path: &Path) -> Option<&'static ImportFormatSpec> {
    path.extension()
        .and_then(|e| e.to_str())
        .and_then(find_import_format_by_ext)
}

/// Returns true if `path` has a file extension belonging to a known external song
/// import format, even when that format's Cargo feature is disabled in this build
/// (allowing CLI/UI callers to surface actionable feature-enable diagnostics).
pub fn is_known_import_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("rbs"))
}

/// Maximum bounded read size for `path` if its extension matches an enabled import format.
pub fn max_import_bytes_for_path(path: &Path) -> Option<u64> {
    find_import_format_for_path(path).map(|spec| spec.max_bytes)
}

/// Format a concise human-readable header preview line for an external format payload.
pub fn peek_import_header_bytes(ext: &str, bytes: &[u8], fallback_title: &str) -> Result<String> {
    let ext_lower = ext.trim_start_matches('.').to_ascii_lowercase();
    match ext_lower.as_str() {
        #[cfg(feature = "rbs")]
        "rbs" => {
            let info = rbs::peek_rbs_header_bytes(bytes, fallback_title)?;
            Ok(format!(
                "ReBirth v{}.{} · \"{}\" · {} bars @ {:.1} BPM · {}",
                info.version.major,
                info.version.minor,
                info.title,
                info.bars,
                info.bpm,
                if info.song_mode {
                    "Song Mode"
                } else {
                    "Pattern Mode"
                }
            ))
        }
        #[cfg(not(feature = "rbs"))]
        "rbs" => {
            let _ = (bytes, fallback_title);
            bail!("ReBirth .rbs support is disabled in this build (compile with --features rbs)")
        }
        other => bail!("Unsupported import format '.{other}'"),
    }
}

/// Import an in-memory external song payload using `catalog` into a validated Cadence [`Project`].
pub fn import_bytes_with_presets(
    ext: &str,
    bytes: &[u8],
    fallback_title: &str,
    catalog: &PresetCatalog,
) -> Result<ImportedProject> {
    let ext_lower = ext.trim_start_matches('.').to_ascii_lowercase();
    match ext_lower.as_str() {
        #[cfg(feature = "rbs")]
        "rbs" => {
            let (project, summary) =
                rbs::import_rbs_bytes_with_presets(bytes, fallback_title, catalog)?;
            let status_line = summary.status_line();
            Ok(ImportedProject {
                format_id: "rbs",
                project,
                status_line,
                notices: summary.notices,
            })
        }
        #[cfg(not(feature = "rbs"))]
        "rbs" => {
            let _ = (bytes, fallback_title, catalog);
            bail!("ReBirth .rbs support is disabled in this build (compile with --features rbs)")
        }
        other => bail!("Unsupported import format '.{other}'"),
    }
}

/// Import an external song file from disk using `catalog` into a validated Cadence [`Project`].
pub fn import_file_with_presets(path: &Path, catalog: &PresetCatalog) -> Result<ImportedProject> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        #[cfg(feature = "rbs")]
        "rbs" => {
            let (project, summary) = rbs::import_rbs_file_with_presets(path, catalog)?;
            let status_line = summary.status_line();
            Ok(ImportedProject {
                format_id: "rbs",
                project,
                status_line,
                notices: summary.notices,
            })
        }
        #[cfg(not(feature = "rbs"))]
        "rbs" => {
            let _ = catalog;
            bail!("ReBirth .rbs support is disabled in this build (compile with --features rbs)")
        }
        other => bail!(
            "Unsupported import format '.{other}' for {}",
            path.display()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_registry_matches_enabled_features() {
        #[cfg(feature = "rbs")]
        {
            assert!(find_import_format_by_ext("rbs").is_some());
            assert!(find_import_format_by_ext(".RBS").is_some());
            assert!(find_import_format_for_path(Path::new("song.rbs")).is_some());
            assert_eq!(supported_import_formats().len(), 1);
        }
        #[cfg(not(feature = "rbs"))]
        {
            assert!(find_import_format_by_ext("rbs").is_none());
            assert!(supported_import_formats().is_empty());
            let err =
                import_bytes_with_presets("rbs", b"", "Test", &PresetCatalog::empty()).unwrap_err();
            assert!(err.to_string().contains("--features rbs"));
        }
    }
}
