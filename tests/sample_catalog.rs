use cadence_core::sampler::{load_sample_file, SampleLibrary};
use std::{fs, path::Path, sync::Arc};
fn tone(path: &Path, value: i16) {
    let mut w = hound::WavWriter::create(
        path,
        hound::WavSpec {
            channels: 1,
            sample_rate: 8000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )
    .unwrap();
    for _ in 0..80 {
        w.write_sample(value).unwrap();
    }
    w.finalize().unwrap();
}
#[test]
fn identical_labels_resolve_the_exact_selected_file_and_cache_identity() {
    let t = tempfile::tempdir().unwrap();
    let a = t.path().join("pack-a");
    let b = t.path().join("pack-b");
    fs::create_dir(&a).unwrap();
    fs::create_dir(&b).unwrap();
    tone(&a.join("tone.wav"), 12000);
    tone(&b.join("tone.wav"), -10000);
    let mut lib = SampleLibrary::from_roots(&[a.clone(), b.clone()], None);
    let entries = lib
        .visible_entries()
        .map(|(_, e)| e.clone())
        .collect::<Vec<_>>();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].label, entries[1].label);
    let first = lib.load_entry(&entries[0]).unwrap();
    let second = lib.load_entry(&entries[1]).unwrap();
    assert!(first.left[0] > 0.0 && second.left[0] < 0.0);
    assert_ne!(first.resolved_path, second.resolved_path);
    assert!(Arc::ptr_eq(
        &second,
        &lib.cached_path(&entries[1].path).unwrap()
    ));
    lib.set_query("pack-b".into());
    assert_eq!(lib.filtered_count(), 1);
    assert!(lib
        .selected_entry()
        .unwrap()
        .path
        .starts_with(b.canonicalize().unwrap()));
    lib.invalidate_decoded_cache();
    lib.cache_prepared(&entries[0].path, Arc::clone(&second));
    assert!(
        lib.cached_path(&entries[0].path).is_none(),
        "wrong prepared resource cannot poison selected path"
    );
    lib.cache_prepared(&entries[1].path, Arc::clone(&second));
    assert!(
        lib.load(entries[1].path.to_str().unwrap()).is_err(),
        "absolute paths must be rejected by portable locator load"
    );
    assert!(Arc::ptr_eq(&second, &lib.load_entry(&entries[1]).unwrap()));
    tone(&entries[1].path, 5000);
    let fresh = lib.reload_entry(&entries[1]).unwrap();
    assert!(fresh.left[0] > 0.0);
    assert!(!Arc::ptr_eq(&fresh, &second));
}
#[test]
fn explicit_roots_and_project_relative_assets_are_portable_and_isolated() {
    let t = tempfile::tempdir().unwrap();
    let samples = t.path().join("samples");
    fs::create_dir(&samples).unwrap();
    tone(&samples.join("tone.wav"), 1000);
    let mut lib = SampleLibrary::from_roots(&[], Some(&t.path().join("project.json")));
    assert_eq!(lib.total_count(), 1);
    assert!(!lib.index_truncated());
    assert!(lib.load("./samples/tone.wav").is_ok());
    fs::remove_file(samples.join("tone.wav")).unwrap();
    assert!(lib.load("./samples/tone.wav").is_err());
}
#[cfg(unix)]
#[test]
fn directory_symlink_cycles_are_skipped_but_audio_file_aliases_work() {
    let t = tempfile::tempdir().unwrap();
    tone(&t.path().join("tone.wav"), 1000);
    std::os::unix::fs::symlink(t.path(), t.path().join("cycle")).unwrap();
    std::os::unix::fs::symlink(t.path().join("tone.wav"), t.path().join("alias.wav")).unwrap();
    let mut lib = SampleLibrary::from_roots(&[t.path().into()], None);
    assert_eq!(lib.total_count(), 2);
    let e = lib
        .visible_entries()
        .find(|(_, e)| e.label == "alias.wav")
        .unwrap()
        .1
        .clone();
    let sample = lib.load_entry(&e).unwrap();
    assert_eq!(
        sample.resolved_path,
        t.path().join("tone.wav").canonicalize().unwrap()
    );
    let decoded = load_sample_file(&e.path, None).unwrap();
    lib.cache_prepared(&e.path, Arc::clone(&decoded));
    assert!(Arc::ptr_eq(
        &decoded,
        &lib.cached_path(&t.path().join("tone.wav")).unwrap()
    ));
}

#[test]
fn roots_only_catalog_excludes_implicit_project_assets_but_keeps_relative_loading() {
    let t = tempfile::tempdir().unwrap();
    let samples = t.path().join("samples");
    fs::create_dir(&samples).unwrap();
    tone(&samples.join("tone.wav"), 1000);
    let mut lib = SampleLibrary::from_roots_only(&[], Some(&t.path().join("project.json")));
    assert_eq!(lib.total_count(), 0);
    assert!(lib.load("./samples/tone.wav").is_ok());
    lib.set_project_path(&t.path().join("other.json"));
    lib.rescan();
    assert_eq!(lib.total_count(), 0);
}

#[test]
fn riff_inst_smpl_metadata_defaults_and_folder_import() {
    use cadence_app::{Action, App};
    use cadence_core::{material::Project, rack::ModuleKind, sampler::SampleLoopMode};
    use std::io::Cursor;

    let write_synthetic_wav = |path: &Path,
                               frames: usize,
                               freq_hz: f32,
                               inst: Option<[u8; 7]>,
                               smpl: Option<(u32, u32, u32, u32)>| {
        let mut cursor = Cursor::new(Vec::<u8>::new());
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        {
            let mut w = hound::WavWriter::new(&mut cursor, spec).unwrap();
            for n in 0..frames {
                let phase = n as f32 * freq_hz * std::f32::consts::TAU / 44_100.0;
                w.write_sample((phase.sin() * 14_000.0) as i16).unwrap();
            }
            w.finalize().unwrap();
        }
        let mut bytes = cursor.into_inner();
        if let Some(inst_bytes) = inst {
            bytes.extend_from_slice(b"inst");
            bytes.extend_from_slice(&7u32.to_le_bytes());
            bytes.extend_from_slice(&inst_bytes);
            bytes.push(0); // word-align padding
        }
        if let Some((root, loop_type, start, end)) = smpl {
            let mut body = vec![0u8; 36 + 24];
            body[12..16].copy_from_slice(&root.to_le_bytes());
            body[28..32].copy_from_slice(&1u32.to_le_bytes());
            body[40..44].copy_from_slice(&loop_type.to_le_bytes());
            body[44..48].copy_from_slice(&start.to_le_bytes());
            body[48..52].copy_from_slice(&end.to_le_bytes());
            bytes.extend_from_slice(b"smpl");
            bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
            bytes.extend_from_slice(&body);
        }
        let riff_size = (bytes.len() - 8) as u32;
        bytes[4..8].copy_from_slice(&riff_size.to_le_bytes());
        fs::write(path, bytes).unwrap();
    };

    let dir = tempfile::tempdir().unwrap();
    let samples_dir = dir.path().join("orch");
    fs::create_dir_all(&samples_dir).unwrap();

    // Looped WAV with both 7-byte inst (bounds 55..67, vel 32..120) and smpl (root 60, forward loop 100..900).
    let looped_wav = samples_dir.join("OSTR C4.wav");
    write_synthetic_wav(
        &looped_wav,
        1200,
        261.63,
        Some([60, (-12i8) as u8, 0, 55, 67, 32, 120]),
        Some((60, 0, 100, 900)),
    );
    // Unlooped filename-pitched WAVs (no inst/smpl) in a separate folder for midpoint splits.
    let pitched_dir = dir.path().join("violin");
    fs::create_dir_all(&pitched_dir).unwrap();
    write_synthetic_wav(&pitched_dir.join("Violin_C3.wav"), 800, 130.8, None, None);
    write_synthetic_wav(&pitched_dir.join("Violin_F3.wav"), 800, 174.6, None, None);
    write_synthetic_wav(&pitched_dir.join("Violin_C4.wav"), 800, 261.6, None, None);

    let save_path = dir.path().join("project.json");
    let lib = SampleLibrary::from_roots_only(&[], Some(&save_path));
    let mut app = App::new_with_library(Project::empty(), save_path, lib).unwrap();

    // 1. `:sampler` applies full smpl loop & root defaults.
    app.execute_command("sampler StrSolo \"./orch/OSTR C4.wav\"")
        .unwrap();
    let solo_id = app.selected_instrument.unwrap();
    let (_, _, _, p) = app
        .instrument(solo_id)
        .unwrap()
        .rack
        .sampler_modules()
        .next()
        .expect("Sampler module on StrSolo");
    assert_eq!(p.root_midi, 60);
    assert_eq!(p.loop_mode, SampleLoopMode::Forward);
    assert_eq!((p.loop_start, p.loop_end), (100, 901));

    // 2. `:sample` on blank Sampler applies identical RIFF defaults.
    app.execute_command("instrument StrTwo sampler").unwrap();
    let two_id = app.selected_instrument.unwrap();
    app.execute_command("sample \"./orch/OSTR C4.wav\"")
        .unwrap();
    let (_, _, _, p2) = app
        .instrument(two_id)
        .unwrap()
        .rack
        .sampler_modules()
        .next()
        .expect("Sampler module on StrTwo");
    assert_eq!(p2.root_midi, 60);
    assert_eq!(p2.loop_mode, SampleLoopMode::Forward);
    assert_eq!((p2.loop_start, p2.loop_end), (100, 901));

    // 3. `:zone sample` on Blank Kit Rack applies full 7-byte inst + smpl + one_shot=false.
    app.execute_command("instrument StrZones kit").unwrap();
    let kit_id = app.selected_instrument.unwrap();
    app.execute_command("zone sample \"./orch/OSTR C4.wav\"")
        .unwrap();
    {
        let k = &app.instrument(kit_id).unwrap().rack;
        let z = &k.zones[0];
        assert_eq!((z.low_midi, z.high_midi, z.root_midi), (55, 67, 60));
        assert!((z.low_vel - (32.0 / 127.0)).abs() < 1e-3);
        assert!((z.high_vel - (120.0 / 127.0)).abs() < 1e-3);
        let sp = z
            .modules
            .iter()
            .find_map(|m| match &m.kind {
                ModuleKind::Sampler(sp) => Some(sp),
                _ => None,
            })
            .expect("expected Sampler module in zone");
        assert!(!sp.one_shot, "looped sample zone must set one_shot = false");
        assert_eq!(sp.loop_mode, SampleLoopMode::Forward);
        assert_eq!((sp.loop_start, sp.loop_end), (100, 901));
    }

    // 4. `:zone import ./violin` builds sorted contiguous midpoint key splits spanning 0..=127 + undo.
    let before_import = app.project().clone();
    app.execute_command("zone import ./violin").unwrap();
    {
        let k = &app.instrument(kit_id).unwrap().rack;
        assert_eq!(k.zones.len(), 3);
        assert_eq!(
            (
                k.zones[0].root_midi,
                k.zones[0].low_midi,
                k.zones[0].high_midi
            ),
            (48, 0, 50)
        );
        assert_eq!(
            (
                k.zones[1].root_midi,
                k.zones[1].low_midi,
                k.zones[1].high_midi
            ),
            (53, 51, 56)
        );
        assert_eq!(
            (
                k.zones[2].root_midi,
                k.zones[2].low_midi,
                k.zones[2].high_midi
            ),
            (60, 57, 127)
        );
        for z in &k.zones {
            let sp = z
                .modules
                .iter()
                .find_map(|m| match &m.kind {
                    ModuleKind::Sampler(sp) => Some(sp),
                    _ => None,
                })
                .expect("expected Sampler module in imported zone");
            assert!(sp.one_shot);
            let path = sp.path.as_deref().unwrap();
            assert!(
                path.starts_with("./violin/"),
                "imported zone path must stay portable relative, got {path}"
            );
        }
    }
    assert!(
        app.ready(),
        "all imported folder samples must load into SampleBank"
    );

    // Atomic undo restores previous zones cleanly.
    app.apply(Action::Undo).unwrap();
    assert_eq!(app.project(), &before_import);
}
