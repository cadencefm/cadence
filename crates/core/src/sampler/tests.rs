use super::*;

fn demo_sample(name: &str) -> PathBuf {
    Path::new(env!("CADENCE_WORKSPACE_DIR"))
        .join("examples/demo/samples")
        .join(name)
}

#[test]
fn library_scan_and_cache_have_independent_bounds() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..70 {
        fs::write(dir.path().join(format!("{i:02}.wav")), []).unwrap();
    }
    let mut library = SampleLibrary::from_roots(&[dir.path().into()], None);
    library.rescan_with_limits(5, 100);
    assert_eq!(library.total_count(), 5);
    assert!(library.index_truncated());
    library.rescan_with_limits(100, 10);
    assert_eq!(library.total_count(), 10);
    assert!(library.index_truncated());
    library.rescan_with_limits(100, 100);
    assert_eq!(library.total_count(), 70);
    assert!(!library.index_truncated());
    // A decoded resource remains usable by a voice after catalog eviction.
    let hat = demo_sample("cyber-hat.wav");
    let bytes = fs::read(&hat).unwrap();
    let original = load_sample_file(&hat, None).unwrap();
    for i in 0..65 {
        let path = dir.path().join(format!("{i:02}.wav"));
        let sample = decode_sample_bytes(
            &bytes,
            "Hat".into(),
            "Hat".into(),
            path.canonicalize().unwrap(),
        )
        .unwrap();
        library.cache_prepared(&path, sample);
    }
    assert_eq!(library.cache.len(), 64);
    assert!(library.cached_path(&dir.path().join("00.wav")).is_none());
    assert!(library.cached_path(&dir.path().join("64.wav")).is_some());
    assert!(original.frames > 0);
}

#[test]
fn decoded_pcm_limit_rejects_before_accumulating_the_whole_asset() {
    let bytes = fs::read(demo_sample("cyber-hat.wav")).unwrap();
    let error = decode_with_symphonia_limit(&bytes, Some("wav"), 8).unwrap_err();
    assert!(error.to_string().contains("PCM limit"));
    assert!(decode_with_symphonia(&bytes, Some("wav")).is_ok());
    // A tiny WavPack header can declare huge output: reject before decoding.
    let mut header = [0u8; 32];
    header[..4].copy_from_slice(b"wvpk");
    header[4..8].copy_from_slice(&24u32.to_le_bytes());
    header[8..10].copy_from_slice(&0x410u16.to_le_bytes());
    header[12..16].copy_from_slice(&500_000_000u32.to_le_bytes());
    header[20..24].copy_from_slice(&1u32.to_le_bytes());
    let error =
        decode_wavpack(&header, "Huge".into(), "huge.wv".into(), "huge.wv".into()).unwrap_err();
    assert!(error.to_string().contains("PCM limit"));
}

#[test]
fn loads_pcm_wav_with_smpl_loop_and_extracts_peaks() {
    let wav_path = demo_sample("reese-bass.wav");
    let sample = load_sample_file(&wav_path, None).unwrap();
    assert_eq!(sample.sample_rate, 48_000);
    assert_eq!(sample.channels, 2);
    assert!(sample.frames > 48_000);
    assert_eq!(sample.peaks.len(), PEAK_BINS);
    assert_eq!(sample.meta.root_midi, 36);
    assert_eq!(sample.meta.loop_mode, SampleLoopMode::Forward);
    assert_eq!(sample.meta.loop_start, 9_600);
    assert_eq!(sample.meta.loop_end, 67_576);
}

#[test]
fn indexes_supported_formats_in_a_temporary_library() {
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path().canonicalize().unwrap();
    let formats = ["wav", "wv", "flac", "ogg", "mp3", "aif", "aiff"];
    for extension in formats {
        fs::write(root.join(format!("test.{extension}")), []).unwrap();
    }
    fs::write(root.join("notes.txt"), b"not an audio asset").unwrap();
    let library = SampleLibrary::from_roots_only(std::slice::from_ref(&root), None);
    let entries: Vec<_> = library
        .entries
        .iter()
        .filter(|entry| entry.path.starts_with(&root))
        .collect();
    assert_eq!(entries.len(), formats.len());
    for extension in formats {
        assert!(entries
            .iter()
            .any(|entry| entry.path.extension().is_some_and(|ext| ext == extension)));
    }
}

#[test]
fn project_relative_assets_win_and_rebase_when_a_new_project_is_opened() {
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path().canonicalize().unwrap();
    let global = root.join("library");
    let first = root.join("first");
    let second = root.join("second");
    let hat = demo_sample("cyber-hat.wav");
    for dir in [&global, &first, &second] {
        fs::create_dir_all(dir).unwrap();
        fs::copy(&hat, dir.join("tone.wav")).unwrap();
    }
    fs::create_dir(first.join("samples")).unwrap();
    fs::copy(&hat, first.join("samples/old-only.wav")).unwrap();
    let mut library = SampleLibrary::from_roots(
        std::slice::from_ref(&global),
        Some(&first.join("score.json")),
    );
    assert_eq!(
        library.load("tone.wav").unwrap().resolved_path,
        first.join("tone.wav")
    );
    assert!(library.resolve_path("old-only.wav").is_some());
    library.set_project_path(&second.join("score.json"));
    assert_eq!(
        library.load("tone.wav").unwrap().resolved_path,
        second.join("tone.wav")
    );
    assert!(
        library.resolve_path("old-only.wav").is_none(),
        "old project roots must not leak into the new project"
    );
    assert_eq!(
        library.resolve_path("./tone.wav"),
        Some(second.join("./tone.wav"))
    );
    fs::remove_file(second.join("tone.wav")).unwrap();
    assert!(
        library.resolve_path("./tone.wav").is_none(),
        "explicit project paths must not substitute library files"
    );
    assert_eq!(
        library.load("tone.wav").unwrap().resolved_path,
        global.join("tone.wav")
    );
}

#[test]
fn removed_library_file_is_not_returned_from_the_decode_cache() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("temporary-asset.wav");
    fs::copy(demo_sample("cyber-hat.wav"), &path).unwrap();
    let mut library = SampleLibrary::from_roots_only(&[folder.path().to_path_buf()], None);
    library.load("temporary-asset.wav").unwrap();
    fs::remove_file(path).unwrap();
    assert!(library.load("temporary-asset.wav").is_err());
}
