//! Portable project bundle exporter: packages a `Project` + `LiveFlags` and copies
//! every referenced `SamplerModule` audio file across all Rack zones into
//! `<bundle_dir>/samples/`, rewriting all sample locators to portable `./samples/<filename>` paths.
use crate::material::{write_document_json, Project};
use crate::mixer::LiveFlags;
use crate::rack::module_sample_key;
use crate::sampler::SampleLibrary;
use crate::store::write_atomic;
use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

const MAX_BUNDLE_PROJECT_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleReport {
    pub project_path: PathBuf,
    pub samples_dir: PathBuf,
    pub samples_copied: usize,
    pub missing_samples: Vec<String>,
}

fn sanitize_filename(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
            out.push(ch);
        } else if ch.is_whitespace() {
            out.push('_');
        }
    }
    if out.is_empty() || out.starts_with('.') {
        format!("sample_{out}")
    } else {
        out
    }
}

fn unique_dest_name(source: &Path, used: &mut HashSet<String>) -> String {
    let raw_name = source
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("sample.wav");
    let clean = sanitize_filename(raw_name);
    if used.insert(clean.clone()) {
        return clean;
    }
    let stem = Path::new(&clean)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("sample");
    let ext = Path::new(&clean)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("wav");
    for n in 2..10_000 {
        let candidate = format!("{stem}_{n}.{ext}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
    }
    clean
}

fn resolve_sample_source(
    locator: &str,
    asset_base: &Path,
    library: &SampleLibrary,
    hint: Option<&PathBuf>,
) -> Option<PathBuf> {
    if let Some(p) = hint.filter(|p| p.is_file()) {
        return Some(p.clone());
    }
    if let Some(p) = library.resolve_path(locator).filter(|p| p.is_file()) {
        return Some(p);
    }
    let joined = asset_base.join(locator);
    if joined.is_file() {
        return Some(joined);
    }
    let direct = PathBuf::from(locator);
    if direct.is_file() {
        return Some(direct);
    }
    None
}

/// Export a self-contained portable Cadence project directory (`project.json` + `samples/`).
pub fn save_portable_bundle(
    project: &Project,
    flags: &LiveFlags,
    asset_base: &Path,
    library: &SampleLibrary,
    resolved_samplers: &HashMap<u64, PathBuf>,
    target_dir: &Path,
) -> Result<BundleReport> {
    std::fs::create_dir_all(target_dir)
        .with_context(|| format!("Creating bundle directory {}", target_dir.display()))?;
    let samples_dir = target_dir.join("samples");

    let mut bundled = project.clone();
    let mut copied_by_canonical: HashMap<PathBuf, String> = HashMap::new();
    let mut used_names: HashSet<String> = HashSet::new();
    let mut missing_samples: Vec<String> = Vec::new();
    let mut samples_copied = 0usize;

    let mut copy_one = |locator: &str, hint: Option<&PathBuf>| -> Result<Option<String>> {
        let Some(src) = resolve_sample_source(locator, asset_base, library, hint) else {
            missing_samples.push(locator.to_string());
            return Ok(None);
        };
        let canonical = src.canonicalize().unwrap_or_else(|_| src.clone());
        if let Some(rel) = copied_by_canonical.get(&canonical) {
            return Ok(Some(rel.clone()));
        }
        std::fs::create_dir_all(&samples_dir)
            .with_context(|| format!("Creating bundle samples dir {}", samples_dir.display()))?;
        let dest_name = unique_dest_name(&canonical, &mut used_names);
        let dest_path = samples_dir.join(&dest_name);
        if dest_path.canonicalize().ok().as_ref() != Some(&canonical) {
            std::fs::copy(&canonical, &dest_path).with_context(|| {
                format!(
                    "Copying sample {} to {}",
                    canonical.display(),
                    dest_path.display()
                )
            })?;
        }
        let rel = format!("./samples/{dest_name}");
        copied_by_canonical.insert(canonical, rel.clone());
        samples_copied += 1;
        Ok(Some(rel))
    };

    for inst in &mut bundled.instruments {
        let inst_id = inst.id;
        for (zone_id, mod_id, smp) in inst.rack.sampler_modules_mut() {
            if let Some(locator) = smp.path.clone() {
                let hint = resolved_samplers.get(&module_sample_key(inst_id, zone_id, mod_id));
                if let Some(rel) = copy_one(&locator, hint)? {
                    smp.path = Some(rel);
                }
            }
        }
    }

    bundled.validate()?;
    let project_path = target_dir.join("project.json");
    write_atomic(&project_path, MAX_BUNDLE_PROJECT_BYTES, |buf| {
        write_document_json(&mut *buf, &bundled, flags)
    })?;

    Ok(BundleReport {
        project_path,
        samples_dir,
        samples_copied,
        missing_samples,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::{parse_document_json, InstrumentInstance};
    use crate::rack::{RackPatch, RackZone};

    #[test]
    fn portable_bundle_copies_samples_and_rewrites_paths() {
        let src_dir = tempfile::tempdir().unwrap();
        let sample_file = src_dir.path().join("kick hit.wav");
        std::fs::write(&sample_file, b"RIFF0000WAVE").unwrap();

        let mut project = Project::demo();
        let ch_id = project.mixer.channels[0].id;
        let inst_id = 9999u64;
        let mut rack = RackPatch::default_sampler();
        if let Some((_, _, smp)) = rack.sampler_modules_mut().next() {
            smp.path = Some("./kick hit.wav".into());
        }
        project.instruments.push(InstrumentInstance {
            id: inst_id,
            name: "Bundled Kick".into(),
            channel: ch_id,
            rack,
        });

        let out_dir = tempfile::tempdir().unwrap();
        let bundle_dir = out_dir.path().join("my-song-bundle");
        let library = SampleLibrary::from_roots_only(&[], None);
        let report = save_portable_bundle(
            &project,
            &LiveFlags::default(),
            src_dir.path(),
            &library,
            &HashMap::new(),
            &bundle_dir,
        )
        .unwrap();

        assert_eq!(report.samples_copied, 1);
        assert!(report.missing_samples.is_empty());
        assert!(report.project_path.is_file());
        assert!(bundle_dir.join("samples/kick_hit.wav").is_file());

        let bytes = std::fs::read(&report.project_path).unwrap();
        assert!(
            bytes.ends_with(b"\n") && !bytes.ends_with(b"\n\n"),
            "bundle project.json should have exactly one trailing newline"
        );
        let (reloaded, _) = parse_document_json(&bytes).unwrap();
        let inst = reloaded
            .instruments
            .iter()
            .find(|i| i.id == inst_id)
            .unwrap();
        let (_, _, _, smp) = inst
            .rack
            .sampler_modules()
            .next()
            .expect("expected sampler");
        assert_eq!(smp.path.as_deref(), Some("./samples/kick_hit.wav"));
    }

    #[test]
    fn portable_bundle_uses_resolved_hint_for_multizone_rack_samplers() {
        let hidden_dir = tempfile::tempdir().unwrap();
        let zone_file = hidden_dir.path().join("snare_hint.wav");
        std::fs::write(&zone_file, b"RIFF0000WAVE").unwrap();

        let mut project = Project::demo();
        let ch_id = project.mixer.channels[0].id;
        let inst_id = 8888u64;
        let mut kit = RackPatch::default_kit();
        let zone_id = kit.next_zone_id();
        let snare_zone =
            RackZone::sample_pad(zone_id, "HintSnare", 38, "nonexistent_catalog_label.wav");
        let mod_id = snare_zone
            .modules
            .iter()
            .find(|m| matches!(m.kind, crate::rack::ModuleKind::Sampler(_)))
            .unwrap()
            .id;
        kit.zones.push(snare_zone);
        project.instruments.push(InstrumentInstance {
            id: inst_id,
            name: "HintKit".into(),
            channel: ch_id,
            rack: kit,
        });

        let mut hints = HashMap::new();
        hints.insert(module_sample_key(inst_id, zone_id, mod_id), zone_file);
        let empty_base = tempfile::tempdir().unwrap();
        let out_dir = tempfile::tempdir().unwrap();
        let bundle_dir = out_dir.path().join("kit-bundle");
        let library = SampleLibrary::from_roots_only(&[], None);

        let report = save_portable_bundle(
            &project,
            &LiveFlags::default(),
            empty_base.path(),
            &library,
            &hints,
            &bundle_dir,
        )
        .unwrap();
        assert_eq!(report.samples_copied, 1);
        assert!(report.missing_samples.is_empty());
        assert!(bundle_dir.join("samples/snare_hint.wav").is_file());
    }

    #[test]
    fn portable_bundle_suffixes_filename_collisions_from_distinct_sources() {
        let src_root = tempfile::tempdir().unwrap();
        let dir_a = src_root.path().join("pack_a");
        let dir_b = src_root.path().join("pack_b");
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        std::fs::write(dir_a.join("kick.wav"), b"RIFF_PACK_A_WAVE").unwrap();
        std::fs::write(dir_b.join("kick.wav"), b"RIFF_PACK_B_WAVE").unwrap();

        let mut project = Project::demo();
        let ch_id = project.mixer.channels[0].id;
        let id_a = project.max_id() + 1;
        let id_b = id_a + 1;
        for (inst_id, name, path) in [
            (id_a, "Kick A", "./pack_a/kick.wav"),
            (id_b, "Kick B", "./pack_b/kick.wav"),
        ] {
            let mut rack = RackPatch::default_sampler();
            if let Some((_, _, smp)) = rack.sampler_modules_mut().next() {
                smp.path = Some(path.into());
            }
            project.instruments.push(InstrumentInstance {
                id: inst_id,
                name: name.into(),
                channel: ch_id,
                rack,
            });
        }

        let out_dir = tempfile::tempdir().unwrap();
        let bundle_dir = out_dir.path().join("collision-bundle");
        let library = SampleLibrary::from_roots_only(&[], None);
        let report = save_portable_bundle(
            &project,
            &LiveFlags::default(),
            src_root.path(),
            &library,
            &HashMap::new(),
            &bundle_dir,
        )
        .unwrap();

        assert_eq!(report.samples_copied, 2);
        assert!(report.missing_samples.is_empty());
        assert_eq!(
            std::fs::read(bundle_dir.join("samples/kick.wav")).unwrap(),
            b"RIFF_PACK_A_WAVE"
        );
        assert_eq!(
            std::fs::read(bundle_dir.join("samples/kick_2.wav")).unwrap(),
            b"RIFF_PACK_B_WAVE"
        );

        let (reloaded, _) =
            parse_document_json(&std::fs::read(&report.project_path).unwrap()).unwrap();
        let path_for = |id: u64| -> String {
            reloaded
                .instruments
                .iter()
                .find(|i| i.id == id)
                .unwrap()
                .rack
                .sampler_modules()
                .next()
                .unwrap()
                .3
                .path
                .clone()
                .unwrap()
        };
        assert_eq!(path_for(id_a), "./samples/kick.wav");
        assert_eq!(path_for(id_b), "./samples/kick_2.wav");
    }

    #[test]
    fn portable_bundle_deduplicates_canonical_samples_across_zones() {
        let src_root = tempfile::tempdir().unwrap();
        let sub = src_root.path().join("drums");
        std::fs::create_dir_all(&sub).unwrap();
        let shared_wav = sub.join("hat.wav");
        std::fs::write(&shared_wav, b"RIFF_SHARED_HAT_WAVE").unwrap();

        let mut project = Project::demo();
        let ch_id = project.mixer.channels[0].id;
        let inst_id = project.max_id() + 1;
        let mut kit = RackPatch::default_kit();
        let z1 = kit.next_zone_id();
        kit.zones
            .push(RackZone::sample_pad(z1, "CH", 42, "./drums/hat.wav"));
        let z2 = kit.next_zone_id();
        kit.zones.push(RackZone::sample_pad(
            z2,
            "OH",
            46,
            "./drums/../drums/hat.wav",
        ));
        project.instruments.push(InstrumentInstance {
            id: inst_id,
            name: "SharedKit".into(),
            channel: ch_id,
            rack: kit,
        });

        let out_dir = tempfile::tempdir().unwrap();
        let bundle_dir = out_dir.path().join("dedup-bundle");
        let library = SampleLibrary::from_roots_only(&[], None);
        let report = save_portable_bundle(
            &project,
            &LiveFlags::default(),
            src_root.path(),
            &library,
            &HashMap::new(),
            &bundle_dir,
        )
        .unwrap();

        assert_eq!(report.samples_copied, 1);
        assert!(report.missing_samples.is_empty());
        assert!(bundle_dir.join("samples/hat.wav").is_file());
        assert!(!bundle_dir.join("samples/hat_2.wav").exists());

        let (reloaded, _) =
            parse_document_json(&std::fs::read(&report.project_path).unwrap()).unwrap();
        let kit_inst = reloaded
            .instruments
            .iter()
            .find(|i| i.id == inst_id)
            .unwrap();
        let paths: Vec<String> = kit_inst
            .rack
            .sampler_modules()
            .filter_map(|(_, _, _, smp)| smp.path.clone())
            .collect();
        assert_eq!(paths, vec!["./samples/hat.wav", "./samples/hat.wav"]);
    }

    #[test]
    fn portable_bundle_reports_partial_missing_samples_while_bundling_available_ones() {
        let src_root = tempfile::tempdir().unwrap();
        std::fs::write(src_root.path().join("present.wav"), b"RIFF_OK_WAVE").unwrap();

        let mut project = Project::demo();
        let ch_id = project.mixer.channels[0].id;
        let inst_id = project.max_id() + 1;
        let mut kit = RackPatch::default_kit();
        let z1 = kit.next_zone_id();
        kit.zones
            .push(RackZone::sample_pad(z1, "Present", 36, "./present.wav"));
        let z2 = kit.next_zone_id();
        kit.zones
            .push(RackZone::sample_pad(z2, "Ghost", 38, "./missing_snare.wav"));
        project.instruments.push(InstrumentInstance {
            id: inst_id,
            name: "PartialKit".into(),
            channel: ch_id,
            rack: kit,
        });

        let out_dir = tempfile::tempdir().unwrap();
        let bundle_dir = out_dir.path().join("partial-bundle");
        let library = SampleLibrary::from_roots_only(&[], None);
        let report = save_portable_bundle(
            &project,
            &LiveFlags::default(),
            src_root.path(),
            &library,
            &HashMap::new(),
            &bundle_dir,
        )
        .unwrap();

        assert_eq!(report.samples_copied, 1);
        assert_eq!(report.missing_samples, vec!["./missing_snare.wav"]);
        assert!(bundle_dir.join("samples/present.wav").is_file());
        let (reloaded, _) =
            parse_document_json(&std::fs::read(&report.project_path).unwrap()).unwrap();
        let inst = reloaded
            .instruments
            .iter()
            .find(|i| i.id == inst_id)
            .unwrap();
        let paths: Vec<String> = inst
            .rack
            .sampler_modules()
            .filter_map(|(_, _, _, smp)| smp.path.clone())
            .collect();
        assert_eq!(paths, vec!["./samples/present.wav", "./missing_snare.wav"]);
    }

    #[test]
    fn portable_bundle_in_place_rebundling_preserves_sample_contents() {
        let bundle_dir = tempfile::tempdir().unwrap();
        let samples_dir = bundle_dir.path().join("samples");
        std::fs::create_dir_all(&samples_dir).unwrap();
        let payload = b"RIFF_ORIGINAL_IN_PLACE_BYTES_1234567890";
        std::fs::write(samples_dir.join("loop.wav"), payload).unwrap();

        let mut project = Project::demo();
        let ch_id = project.mixer.channels[0].id;
        let inst_id = project.max_id() + 1;
        let mut rack = RackPatch::default_sampler();
        if let Some((_, _, smp)) = rack.sampler_modules_mut().next() {
            smp.path = Some("./samples/loop.wav".into());
        }
        project.instruments.push(InstrumentInstance {
            id: inst_id,
            name: "InPlaceSampler".into(),
            channel: ch_id,
            rack,
        });

        let library = SampleLibrary::from_roots_only(&[], None);
        let report = save_portable_bundle(
            &project,
            &LiveFlags::default(),
            bundle_dir.path(),
            &library,
            &HashMap::new(),
            bundle_dir.path(),
        )
        .unwrap();

        assert_eq!(report.samples_copied, 1);
        assert!(report.missing_samples.is_empty());
        assert_eq!(
            std::fs::read(samples_dir.join("loop.wav")).unwrap(),
            payload,
            "in-place re-bundling must not truncate or corrupt existing sample files"
        );
        let (reloaded, _) =
            parse_document_json(&std::fs::read(&report.project_path).unwrap()).unwrap();
        let inst = reloaded
            .instruments
            .iter()
            .find(|i| i.id == inst_id)
            .unwrap();
        assert_eq!(
            inst.rack
                .sampler_modules()
                .next()
                .unwrap()
                .3
                .path
                .as_deref(),
            Some("./samples/loop.wav")
        );
    }

    #[test]
    fn portable_bundle_sanitizes_dot_files_and_unsafe_characters() {
        let src_dir = tempfile::tempdir().unwrap();
        let hidden = src_dir.path().join(".secret hit!?.wav");
        std::fs::write(&hidden, b"RIFF_DOTFILE_WAVE").unwrap();

        let mut project = Project::demo();
        let ch_id = project.mixer.channels[0].id;
        let inst_id = project.max_id() + 1;
        let mut rack = RackPatch::default_sampler();
        if let Some((_, _, smp)) = rack.sampler_modules_mut().next() {
            smp.path = Some("./.secret hit!?.wav".into());
        }
        project.instruments.push(InstrumentInstance {
            id: inst_id,
            name: "DotSampler".into(),
            channel: ch_id,
            rack,
        });

        let out_dir = tempfile::tempdir().unwrap();
        let bundle_dir = out_dir.path().join("dot-bundle");
        let library = SampleLibrary::from_roots_only(&[], None);
        let report = save_portable_bundle(
            &project,
            &LiveFlags::default(),
            src_dir.path(),
            &library,
            &HashMap::new(),
            &bundle_dir,
        )
        .unwrap();

        assert_eq!(report.samples_copied, 1);
        assert!(report.missing_samples.is_empty());
        assert!(bundle_dir.join("samples/sample_.secret_hit.wav").is_file());
        let (reloaded, _) =
            parse_document_json(&std::fs::read(&report.project_path).unwrap()).unwrap();
        let inst = reloaded
            .instruments
            .iter()
            .find(|i| i.id == inst_id)
            .unwrap();
        let bundled_path = inst
            .rack
            .sampler_modules()
            .next()
            .unwrap()
            .3
            .path
            .as_deref()
            .unwrap();
        assert_eq!(bundled_path, "./samples/sample_.secret_hit.wav");
        assert!(!bundled_path
            .strip_prefix("./samples/")
            .unwrap()
            .starts_with('.'));
    }
}
