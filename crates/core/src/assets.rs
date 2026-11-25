//! Sample preparation and performance-readiness diagnostics.
//!
//! Structural project validity is independent of whether its external assets
//! can be loaded. Keep the score intact while reporting every unavailable part.

use crate::rack::module_sample_key;
use crate::runtime::SampleBank;
use crate::sampler::{load_sample_file, SampleData, SampleLibrary};
use crate::score::ScoreWindow;
use anyhow::{bail, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SampleIssue {
    pub instrument_id: u64,
    pub instrument_name: String,
    pub path: Option<String>,
    pub message: String,
}

#[derive(Debug)]
pub struct SampleLoadReport {
    pub bank: SampleBank,
    pub issues: Vec<SampleIssue>,
}

impl SampleLoadReport {
    pub fn ensure_ready(&self) -> Result<()> {
        ensure_samples_ready(&self.issues)
    }
}

fn issue_label(inst_name: &str, zone_count: usize, zone_name: &str) -> String {
    if zone_count <= 1 {
        inst_name.to_string()
    } else {
        format!("{inst_name} [{zone_name}]")
    }
}

/// Prepare all available sampler assets, retaining precise failures for repair.
/// An unloaded sampler module is a valid editable device, but is not render-ready.
pub fn load_project_samples(score: &ScoreWindow, library: &mut SampleLibrary) -> SampleLoadReport {
    load_project_samples_with(
        score,
        library,
        |p| p.is_file(),
        |p| Ok(p.canonicalize()?),
        |p, lbl| load_sample_file(p, lbl),
    )
}

/// Host-I/O-injectable variant of [`load_project_samples`] performing zero direct OS filesystem reads.
pub fn load_project_samples_with(
    score: &ScoreWindow,
    library: &mut SampleLibrary,
    is_file: impl Fn(&Path) -> bool,
    canonicalize: impl Fn(&Path) -> Result<PathBuf>,
    load_file: impl Fn(&Path, Option<String>) -> Result<Arc<SampleData>>,
) -> SampleLoadReport {
    let mut report = SampleLoadReport {
        bank: Vec::new(),
        issues: Vec::new(),
    };
    for inst in &score.instruments {
        let zone_count = inst.rack.zones.len();
        for (zone_id, zone, mod_id, smp) in inst.rack.sampler_modules() {
            let path = smp.path.as_deref();
            let message = match path {
                Some(path) => match library.load_with(path, &is_file, &canonicalize, &load_file) {
                    Ok(sample) => {
                        report
                            .bank
                            .push((module_sample_key(inst.id, zone_id, mod_id), sample));
                        continue;
                    }
                    Err(error) => format!("{error:#}"),
                },
                None => {
                    if zone_count <= 1 {
                        "No sample assigned; load a sample before rendering".to_string()
                    } else {
                        format!("Zone {} has no sample assigned", zone.name)
                    }
                }
            };
            report.issues.push(SampleIssue {
                instrument_id: inst.id,
                instrument_name: issue_label(&inst.name, zone_count, &zone.name),
                path: path.map(str::to_string),
                message,
            });
        }
    }
    report
}

/// Prepare and merge all sampler assets across a sequence of compiled [`ScoreWindow`]s
/// (such as every segment of a recorded [`crate::take::Take`]), preserving samples from
/// earlier segments whose sampler instruments were later replaced or removed.
pub fn load_schedule_samples<'a>(
    scores: impl IntoIterator<Item = &'a ScoreWindow>,
    library: &mut SampleLibrary,
) -> SampleLoadReport {
    let mut report = SampleLoadReport {
        bank: Vec::new(),
        issues: Vec::new(),
    };
    for score in scores {
        let segment = load_project_samples(score, library);
        for (id, sample) in segment.bank {
            if !report.bank.iter().any(|(existing, _)| *existing == id) {
                report.bank.push((id, sample));
            }
        }
        for issue in segment.issues {
            if !report.issues.contains(&issue) {
                report.issues.push(issue);
            }
        }
    }
    report
}

/// Defend public DSP/export APIs against incomplete caller-provided banks.
pub fn ensure_sample_bank_ready(score: &ScoreWindow, bank: &SampleBank) -> Result<()> {
    let mut issues = Vec::new();
    for inst in &score.instruments {
        if !score.required_instrument_ids.contains(&inst.id) {
            continue;
        }
        let zone_count = inst.rack.zones.len();
        for (zone_id, zone, mod_id, smp) in inst.rack.sampler_modules() {
            let key = module_sample_key(inst.id, zone_id, mod_id);
            let path = smp.path.clone();
            let message = if path.is_none() {
                if zone_count <= 1 {
                    Some("No sample assigned; load a sample before rendering")
                } else {
                    Some("Zone sample is unassigned; load a sample before rendering")
                }
            } else if !bank
                .iter()
                .find(|(id, _)| *id == key)
                .is_some_and(|(_, sample)| sample.frames > 0)
            {
                Some("Required sample has no prepared audio buffer; load or relink it before rendering")
            } else {
                None
            };
            if let Some(message) = message {
                issues.push(SampleIssue {
                    instrument_id: inst.id,
                    instrument_name: issue_label(&inst.name, zone_count, &zone.name),
                    path,
                    message: message.to_string(),
                });
            }
        }
    }
    ensure_samples_ready(&issues)
}

/// Reject an unready resource set with every part/path diagnostic intact.
pub fn ensure_samples_ready(issues: &[SampleIssue]) -> Result<()> {
    if issues.is_empty() {
        return Ok(());
    }
    let details = issues
        .iter()
        .map(|issue| {
            format!(
                "{} (instrument {}, {}): {}",
                issue.instrument_name,
                issue.instrument_id,
                issue.path.as_deref().unwrap_or("unassigned"),
                issue.message,
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    bail!("Project is not ready for audio rendering:\n{details}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::{InstrumentInstance, Project};
    use crate::rack::RackPatch;
    use std::fs;

    #[test]
    fn reports_all_missing_invalid_and_unassigned_assets_without_changing_the_score() {
        let folder = tempfile::tempdir().unwrap();
        let invalid = folder.path().join("broken.wav");
        fs::write(&invalid, b"not an audio file").unwrap();
        let mut project = Project::empty();
        project.mixer.channels.push(crate::mixer::Channel::new(
            1,
            crate::mixer::ChannelKind::Audio,
            "Kit",
        ));
        let ch = project.mixer.channels[0].id;
        for (id, path) in [
            (1, Some("./missing.wav")),
            (2, Some("./broken.wav")),
            (3, None),
        ] {
            let mut rack = RackPatch::default_sampler();
            if let Some((_, _, smp)) = rack.sampler_modules_mut().next() {
                smp.path = path.map(str::to_string);
            }
            project.instruments.push(InstrumentInstance {
                id,
                name: format!("Sampler {id}"),
                rack,
                channel: ch,
            });
        }
        let score = project.compile().unwrap();
        let before = score.clone();
        let mut library = SampleLibrary::from_roots(&[], Some(&folder.path().join("project.json")));
        let report = load_project_samples(&score, &mut library);
        assert!(report.bank.is_empty());
        assert_eq!(report.issues.len(), 3);
        assert!(report.issues[0].message.contains("not found"));
        assert!(report.issues[1]
            .path
            .as_ref()
            .unwrap()
            .contains("broken.wav"));
        assert!(report.issues[2].path.is_none());
        let error = report.ensure_ready().unwrap_err().to_string();
        assert!(
            error.contains("missing.wav")
                && error.contains("broken.wav")
                && error.contains("unassigned")
        );
        assert_eq!(score, before);
    }

    #[test]
    fn successful_sample_preparation_and_synth_only_projects_are_ready() {
        let project = Project::demo();
        let score = project.compile().unwrap();
        let mut library = SampleLibrary::discover(&[], None);
        let report = load_project_samples(&score, &mut library);
        assert!(report.issues.is_empty());
        assert!(report.ensure_ready().is_ok());
        assert!(ensure_sample_bank_ready(&score, &report.bank).is_ok());
    }
}
