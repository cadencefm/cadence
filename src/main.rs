use anyhow::{bail, ensure, Context, Result};
use cadence::host::{
    self,
    ctl::{parse_listen_bars, resolve_ctl_socket, send_ctl_request},
    protocol::{HostRequest, ListenRequest, DEFAULT_LISTEN_SAMPLE_RATE},
};
use cadence_app::{load_document, Action, App, CoreHostIo};
use cadence_core::{material::Project, mixer::LiveFlags, sampler::SampleLibrary};
use cadence_engine::audio;
use clap::{ArgGroup, Parser, Subcommand};
use std::{
    fs::File,
    io::{self, BufReader},
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Parser, Debug)]
#[command(
    name = "cadence",
    version,
    about = "A GPU-accelerated, keyboard-driven modal musical instrument",
    long_about = "A modal musical workspace with independent patterns, retained derivations, parts, arrangement lanes, and a native mixer. Literal and composed material share the same editor and command interface. Run with no arguments for the demo, or --empty to build from scratch. Press ? for the guide."
)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,

    /// Open a retained-material project (or ReBirth .rbs song); otherwise start with the built-in demo.
    #[arg(short, long, conflicts_with_all = ["import_rbs", "empty", "write_demo", "render_kit", "list_midi"])]
    project: Option<PathBuf>,
    /// Import a ReBirth RB-338 .rbs song (v1.x or v2.0 / 4.0–4.2) directly into Cadence.
    #[arg(long = "import-rbs", conflicts_with_all = ["project", "empty", "write_demo", "render_kit", "list_midi"])]
    import_rbs: Option<PathBuf>,
    /// Start a genuinely empty authoring workspace.
    #[arg(long, conflicts_with_all = ["project", "import_rbs", "write_demo", "render_kit", "list_midi"])]
    empty: bool,
    /// Additional sample root directory for Sampler lookup (repeatable).
    #[arg(long = "sample-dir")]
    sample_dir: Vec<PathBuf>,
    /// Additional external preset root directory for InstrumentPreset & filter_grooves lookup (repeatable).
    #[arg(long = "preset-dir")]
    preset_dir: Vec<PathBuf>,
    /// Browse only explicit --sample-dir roots; project-relative loading still works.
    #[arg(long)]
    sample_roots_only: bool,
    /// Export each Audio channel of SOURCE_PROJECT into baked WAVs/kit preset, live synth kit preset, and/or individual voice presets and exit.
    #[arg(
        long = "render-kit",
        conflicts_with_all = ["project", "import_rbs", "empty", "write_demo", "render", "stems", "stdio", "script", "key_script", "output", "check", "recover", "ctl", "list_midi", "bars", "take", "midi_in", "midi_out"]
    )]
    render_kit: Option<PathBuf>,
    /// Output directory for baked kit WAV samples (with --render-kit; pair with --out-preset).
    #[arg(long = "out-samples", requires = "render_kit")]
    out_samples: Option<PathBuf>,
    /// Output path for generated baked sample-kit InstrumentPreset JSON (with --render-kit; pair with --out-samples).
    #[arg(long = "out-preset", requires = "render_kit")]
    out_preset: Option<PathBuf>,
    /// Output path for generated live multi-zone modular synth kit InstrumentPreset JSON (with --render-kit).
    #[arg(long = "out-synth-preset", requires = "render_kit")]
    out_synth_preset: Option<PathBuf>,
    /// Output directory for individual 1-zone modular synth drum voice presets (with --render-kit).
    #[arg(long = "out-voices-dir", requires = "render_kit")]
    out_voices_dir: Option<PathBuf>,
    /// Optional display kit name override (with --render-kit; defaults to source project title).
    #[arg(long = "kit-name", requires = "render_kit")]
    kit_name: Option<String>,
    /// Run the synth and transport without opening an audio device.
    #[arg(long, conflicts_with = "list_midi")]
    silent: bool,
    /// Preferred live audio output device name (substring match; overrides config for this session).
    #[arg(
        long = "audio-device",
        value_name = "NAME",
        conflicts_with_all = ["silent", "write_demo", "render_kit", "list_midi", "render", "stems", "check", "script", "key_script"]
    )]
    audio_device: Option<String>,
    /// Preferred live audio callback buffer size in frames (32..=8192; overrides config for this session).
    #[arg(
        long = "buffer-size",
        value_name = "FRAMES",
        conflicts_with_all = ["silent", "write_demo", "render_kit", "list_midi", "render", "stems", "check", "script", "key_script"]
    )]
    buffer_size: Option<u32>,
    /// Export the current project to a WAV file and exit (no window/device needed).
    #[arg(long, conflicts_with_all = ["stdio", "write_demo", "render_kit", "list_midi", "ctl", "midi_in", "midi_out"])]
    render: Option<PathBuf>,
    /// Export Master + isolated per-channel stereo WAV stems into DIR and exit.
    #[arg(long, conflicts_with_all = ["stdio", "write_demo", "render_kit", "list_midi", "ctl", "take", "midi_in", "midi_out"])]
    stems: Option<PathBuf>,
    /// Save channel overload and PCM/tail measurements for the offline render.
    #[arg(long, requires = "render")]
    render_report: Option<PathBuf>,
    /// Number of bars for offline rendering (defaults to project bar count; requires --render or --stems).
    #[arg(long, conflicts_with_all = ["stdio", "write_demo", "render_kit", "list_midi", "ctl"])]
    bars: Option<u32>,
    /// Render a recorded live take instead of the arrangement (with --render).
    #[arg(long, requires = "render", conflicts_with = "stems")]
    take: Option<PathBuf>,
    /// Bars of release after a take's last step.
    #[arg(long, default_value_t = 2, requires = "take")]
    take_tail_bars: u32,
    /// Output sample rate for offline rendering.
    #[arg(long, default_value_t = 48_000)]
    sample_rate: u32,
    /// Write the built-in demo as a reusable JSON project and exit.
    #[arg(
        long,
        conflicts_with_all = ["project", "import_rbs", "empty", "render_kit", "script", "key_script", "stdio", "output", "render", "stems", "check", "recover", "ctl", "list_midi", "bars", "take", "midi_in", "midi_out"]
    )]
    write_demo: Option<PathBuf>,
    /// Apply JSONL musical operations before rendering/saving; no window is opened.
    #[arg(long, conflicts_with_all = ["stdio", "key_script", "write_demo", "render_kit", "list_midi", "ctl", "midi_in", "midi_out"])]
    script: Option<PathBuf>,
    /// Replay JSONL key-sequence steps through the real controller, then exit.
    #[arg(long, conflicts_with_all = ["stdio", "script", "write_demo", "render_kit", "list_midi", "ctl", "midi_in", "midi_out"])]
    key_script: Option<PathBuf>,
    /// Run the live engine controlled by JSONL on stdin/stdout, without opening a window.
    #[arg(
        long,
        conflicts_with_all = ["render", "stems", "write_demo", "render_kit", "check", "output", "script", "key_script", "ctl", "list_midi", "bars", "take", "midi_in", "midi_out"]
    )]
    stdio: bool,
    /// Save the resulting score after a batch script, validation, render, or .rbs import.
    #[arg(long, conflicts_with_all = ["stdio", "write_demo", "render_kit", "list_midi", "ctl", "midi_in", "midi_out"])]
    output: Option<PathBuf>,
    /// Validate score and sample readiness; print a summary and fail if required audio is unavailable.
    #[arg(long, conflicts_with_all = ["stdio", "write_demo", "render_kit", "list_midi", "ctl", "midi_in", "midi_out"])]
    check: bool,
    /// Restore unsaved changes from the session crash-recovery snapshot (never overwrites disk).
    #[arg(long, conflicts_with_all = ["write_demo", "render_kit", "list_midi"])]
    recover: bool,
    /// Override the per-project session directory (locks, recovery autosave, auto-recorded takes, default ctl.sock).
    #[arg(long)]
    session_dir: Option<PathBuf>,
    /// Enable the live Unix control socket for `cadence ctl` (defaults to `<session_dir>/ctl.sock`).
    #[arg(
        long,
        value_name = "SOCK",
        num_args = 0..=1,
        default_missing_value = "",
        conflicts_with_all = ["stdio", "script", "key_script", "render", "stems", "check", "output", "write_demo", "render_kit", "list_midi", "bars", "take"]
    )]
    ctl: Option<String>,
    /// Play from a MIDI input port whose name contains PORT (any port if omitted).
    #[arg(long, num_args = 0..=1, default_missing_value = "", conflicts_with_all = ["stdio", "script", "key_script", "render", "stems", "check", "output", "write_demo", "render_kit", "list_midi"])]
    midi_in: Option<String>,
    /// Send LED/OLED feedback to a MIDI output port whose name contains PORT (any port if omitted).
    #[arg(long, num_args = 0..=1, default_missing_value = "", conflicts_with_all = ["stdio", "script", "key_script", "render", "stems", "check", "output", "write_demo", "render_kit", "list_midi"])]
    midi_out: Option<String>,
    /// List MIDI input and output ports and exit.
    #[arg(long, conflicts_with_all = ["project", "import_rbs", "empty", "write_demo", "render_kit", "render", "stems", "stdio", "script", "key_script", "output", "check", "recover", "ctl", "bars", "take", "midi_in", "midi_out", "silent"])]
    list_midi: bool,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Query or control a running Cadence GUI instance via its live `--ctl` Unix socket.
    Ctl(CtlArgs),
}

#[derive(Parser, Debug)]
struct CtlArgs {
    /// Path to the running Cadence Unix control socket (auto-discovers CADENCE_CTL_SOCK or <session_dir>/ctl.sock if omitted).
    #[arg(long, conflicts_with = "session_dir")]
    sock: Option<PathBuf>,
    /// Resolve `<SESSION_DIR>/ctl.sock` explicitly.
    #[arg(long = "session-dir", conflicts_with = "sock")]
    session_dir: Option<PathBuf>,
    #[command(subcommand)]
    op: CtlCommand,
}

#[derive(Subcommand, Debug)]
enum CtlCommand {
    /// Fetch the live session state (omit full project JSON with `--compact`).
    State {
        #[arg(long)]
        compact: bool,
    },
    /// Fetch live mixer channel peaks/headroom/clip flags and Compressor/Limiter GR meters.
    Meters,
    /// Execute a Cadence Ex command line on the live GUI session (pushed onto `u` undo stack).
    Cmd {
        /// Ex command line (e.g. `"bpm 132"`, `"param cutoff 1200"`, `"param TR-808.BD.cutoff 1200"`).
        line: String,
        /// Fail with a revision conflict if the live GUI revision does not match.
        #[arg(long = "expected-revision")]
        expected_revision: Option<u64>,
        /// Return compact telemetry instead of the full project JSON dump.
        #[arg(long)]
        compact: bool,
    },
    /// Execute a typed JSON `Action` on the live GUI session.
    Action {
        /// JSON object representing a typed `cadence_app::Action` (e.g. `'{"op":"play","playing":true}'`).
        json: String,
        /// Fail with a revision conflict if the live GUI revision does not match.
        #[arg(long = "expected-revision")]
        expected_revision: Option<u64>,
        /// Return compact telemetry instead of the full project JSON dump.
        #[arg(long)]
        compact: bool,
    },
    /// Capture stereo 16-bit WAV audio (offline bar window or live speaker tap) with telemetry.
    #[command(group(ArgGroup::new("listen_mode").required(true).args(["bars", "live"])))]
    Listen {
        /// Render 1-based half-open bar window `A..B` (or inclusive `A..=B`) from live in-memory state.
        #[arg(long, value_name = "A..B", conflicts_with = "live")]
        bars: Option<String>,
        /// Temporary solo channels/lanes for `--bars` (comma-separated or repeatable; never mutates GUI flags).
        #[arg(
            long,
            value_delimiter = ',',
            requires = "bars",
            conflicts_with = "live"
        )]
        solo: Vec<String>,
        /// Capture `SECONDS` (`0.25..=30.0`) of live speaker output from the lock-free audio tap ring.
        #[arg(long, value_name = "SECONDS", conflicts_with = "bars")]
        live: Option<f64>,
        /// Output WAV sample rate in Hz (default 24000).
        #[arg(long = "sr", alias = "sample-rate", default_value_t = DEFAULT_LISTEN_SAMPLE_RATE)]
        sr: u32,
        /// Destination WAV path (defaults to `<session_dir>/ear.wav`).
        #[arg(long = "out", alias = "output")]
        out: Option<PathBuf>,
    },
}

const MAX_RENDER_REPORT_BYTES: u64 = 16 * 1024 * 1024;

fn write_render_report(
    report_path: &Path,
    report: &cadence_engine::audio::RenderReport,
) -> Result<()> {
    cadence_core::store::write_atomic(report_path, MAX_RENDER_REPORT_BYTES, |buf| {
        serde_json::to_writer_pretty(&mut *buf, report)
            .context("Serializing render report JSON")?;
        buf.push(b'\n');
        Ok(())
    })
}

fn has_top_level_session_flags(args: &Args) -> bool {
    args.project.is_some()
        || args.import_rbs.is_some()
        || args.empty
        || !args.sample_dir.is_empty()
        || !args.preset_dir.is_empty()
        || args.sample_roots_only
        || args.render_kit.is_some()
        || args.silent
        || args.audio_device.is_some()
        || args.buffer_size.is_some()
        || args.render.is_some()
        || args.stems.is_some()
        || args.bars.is_some()
        || args.take.is_some()
        || args.write_demo.is_some()
        || args.script.is_some()
        || args.key_script.is_some()
        || args.stdio
        || args.output.is_some()
        || args.check
        || args.recover
        || args.session_dir.is_some()
        || args.ctl.is_some()
        || args.midi_in.is_some()
        || args.midi_out.is_some()
        || args.list_midi
}

fn run_ctl_client(ctl: CtlArgs) -> Result<()> {
    let sock = resolve_ctl_socket(ctl.sock.as_deref(), ctl.session_dir.as_deref())?;
    let request = match ctl.op {
        CtlCommand::State { compact } => HostRequest {
            compact,
            action: Some(Action::GetState),
            ..HostRequest::default()
        },
        CtlCommand::Meters => HostRequest {
            meters: true,
            ..HostRequest::default()
        },
        CtlCommand::Cmd {
            line,
            expected_revision,
            compact,
        } => HostRequest {
            expected_revision,
            compact,
            cmd: Some(line),
            ..HostRequest::default()
        },
        CtlCommand::Action {
            json,
            expected_revision,
            compact,
        } => {
            let action: Action = serde_json::from_str(&json)
                .with_context(|| format!("Parsing Action JSON '{json}'"))?;
            HostRequest {
                expected_revision,
                compact,
                action: Some(action),
                ..HostRequest::default()
            }
        }
        CtlCommand::Listen {
            bars,
            solo,
            live,
            sr,
            out,
        } => {
            let listen = if let Some(spec) = bars {
                let (start_bar, end_bar) = parse_listen_bars(&spec)?;
                ListenRequest::Bars {
                    start_bar,
                    end_bar,
                    solo,
                    sample_rate: sr,
                    out_path: out,
                }
            } else if let Some(seconds) = live {
                ListenRequest::Live {
                    seconds,
                    sample_rate: sr,
                    out_path: out,
                }
            } else {
                bail!("Specify either --bars A..B or --live SECONDS")
            };
            HostRequest {
                listen: Some(listen),
                ..HostRequest::default()
            }
        }
    };
    let response = send_ctl_request(&sock, &request)?;
    println!("{}", serde_json::to_string(&response)?);
    if response["ok"] != true {
        let err_msg = response["error"]
            .as_str()
            .or_else(|| response["audio_error"].as_str())
            .unwrap_or("Control request failed");
        bail!("{err_msg}");
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("cadence: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = Args::parse();
    if let Some(Command::Ctl(ctl_args)) = args.command.take() {
        ensure!(
            !has_top_level_session_flags(&args),
            "Top-level session flags cannot be combined with `cadence ctl`; pass --sock or --session-dir after `ctl`"
        );
        return run_ctl_client(ctl_args);
    }

    if args.bars.is_some() && args.render.is_none() && args.stems.is_none() {
        bail!("--bars requires --render or --stems");
    }
    if let Some(frames) = args.buffer_size {
        ensure!(
            (cadence_core::runtime::MIN_AUDIO_BUFFER_FRAMES
                ..=cadence_core::runtime::MAX_AUDIO_BUFFER_FRAMES)
                .contains(&frames),
            "--buffer-size must be between {} and {} frames",
            cadence_core::runtime::MIN_AUDIO_BUFFER_FRAMES,
            cadence_core::runtime::MAX_AUDIO_BUFFER_FRAMES
        );
    }

    let project_is_import = args
        .project
        .as_deref()
        .is_some_and(cadence_formats::is_known_import_path);
    let has_output_producer = args.script.is_some()
        || args.key_script.is_some()
        || args.render.is_some()
        || args.stems.is_some()
        || args.check
        || args.import_rbs.is_some()
        || project_is_import;
    if args.output.is_some() && !has_output_producer {
        bail!(
            "--output requires a batch operation (--script, --key-script, --check, --render, --stems, --import-rbs, or --project <file.rbs>); use --write-demo to export the built-in demo"
        );
    }

    if args.list_midi {
        #[cfg(feature = "midi")]
        {
            let inputs = cadence_engine::midi::list_ports()?;
            let outputs = cadence_engine::midi::list_output_ports()?;
            println!("MIDI Input Ports ({}):", inputs.len());
            if inputs.is_empty() {
                println!("  (none)");
            }
            for port in inputs {
                println!("  {port}");
            }
            println!("MIDI Output Ports ({}):", outputs.len());
            if outputs.is_empty() {
                println!("  (none)");
            }
            for port in outputs {
                println!("  {port}");
            }
            return Ok(());
        }
        #[cfg(not(feature = "midi"))]
        bail!("This build has no MIDI support; build with the midi feature");
    }
    if let Some(path) = &args.write_demo {
        let library = SampleLibrary::from_roots_only(&[], Some(path));
        App::with_host_io(
            Project::demo(),
            LiveFlags::default(),
            path.clone(),
            library,
            Arc::new(CoreHostIo),
        )?
        .save_project(path)?;
        println!("Wrote demo project to {}", path.display());
        return Ok(());
    }
    if let Some(source_project) = &args.render_kit {
        let summary = host::render_kit(&host::KitRenderOptions {
            source_project,
            out_samples: args.out_samples.as_deref(),
            out_preset: args.out_preset.as_deref(),
            out_synth_preset: args.out_synth_preset.as_deref(),
            out_voices_dir: args.out_voices_dir.as_deref(),
            kit_name_override: args.kit_name.as_deref(),
            sample_rate: args.sample_rate,
            extra_sample_dirs: &args.sample_dir,
        })?;
        if let Some(preset_path) = &summary.preset_path {
            println!(
                "Rendered baked kit '{}' ({} voices, {:.1} KiB total @ {} Hz) → {}",
                summary.kit_name,
                summary.voices.len(),
                summary.total_bytes as f64 / 1024.0,
                summary.sample_rate,
                preset_path.display()
            );
            for v in &summary.voices {
                println!(
                    "  {:02} {:<10} MIDI {:>3} {:>2}ch  {:>6.1} ms  peak {:>6.3}  dc {:>7.5}  {:>6.1} KiB  {}",
                    v.index,
                    v.name,
                    v.midi,
                    v.channels,
                    v.frames as f64 * 1000.0 / f64::from(summary.sample_rate),
                    v.peak,
                    v.dc_abs,
                    v.bytes as f64 / 1024.0,
                    v.locator
                );
            }
        }
        if let Some(synth_path) = &summary.synth_preset_path {
            println!(
                "Exported live synth kit '{}' ({} zones, {} pins) → {}",
                summary.synth_kit_name,
                summary.synth_zone_count,
                summary.synth_pin_count,
                synth_path.display()
            );
        }
        if !summary.voice_presets.is_empty() {
            println!(
                "Exported {} modular voice preset(s) for '{}':",
                summary.voice_presets.len(),
                summary.kit_name
            );
            for vp in &summary.voice_presets {
                println!(
                    "  {:02} {:<14} MIDI {:>3}  {:>2} mods  {:>2} cables  {:>2} pins  {}",
                    vp.index,
                    vp.name,
                    vp.midi,
                    vp.modules,
                    vp.cables,
                    vp.pins,
                    vp.path.display()
                );
            }
        }
        return Ok(());
    }
    let config_load = cadence_core::config::UserConfig::load_default_diagnostic();
    if let Some(warning) = &config_load.warning {
        eprintln!("cadence: warning: {warning}");
    }
    let user_config = config_load.config;
    let config_corrupt_on_disk = config_load.corrupt_on_disk;
    let mut startup_warning = config_load.warning;
    let rbs_presets = |rbs_path: &Path| {
        cadence_core::presets::PresetCatalog::discover(
            &args.preset_dir,
            Some(rbs_path),
            &user_config,
        )
    };
    let (project, flags, save_path, import_status) = if let Some(path) = &args.import_rbs {
        let catalog = rbs_presets(path);
        let imported = cadence_formats::import_file_with_presets(path, &catalog)?;
        (
            imported.project,
            LiveFlags::default(),
            path.with_extension("cadence.json"),
            Some(imported.status_line),
        )
    } else if let Some(path) = &args.project {
        if cadence_formats::find_import_format_for_path(path).is_some()
            || cadence_formats::is_known_import_path(path)
        {
            let catalog = rbs_presets(path);
            let imported = cadence_formats::import_file_with_presets(path, &catalog)?;
            (
                imported.project,
                LiveFlags::default(),
                path.with_extension("cadence.json"),
                Some(imported.status_line),
            )
        } else {
            let (project, flags) = load_document(path)?;
            (project, flags, path.clone(), None)
        }
    } else if args.empty {
        (
            Project::empty_with_config(&user_config),
            LiveFlags::default(),
            PathBuf::from("cadence-project.json"),
            None,
        )
    } else {
        (
            Project::demo(),
            LiveFlags::default(),
            PathBuf::from("cadence-project.json"),
            None,
        )
    };
    let library = if args.sample_roots_only {
        SampleLibrary::from_roots_only(&args.sample_dir, Some(&save_path))
    } else {
        SampleLibrary::discover_with_config(&args.sample_dir, Some(&save_path), &user_config)
    };
    let custom_presets = (!args.preset_dir.is_empty()).then(|| {
        let catalog = cadence_core::presets::PresetCatalog::discover(
            &args.preset_dir,
            Some(&save_path),
            &user_config,
        );
        let grooves =
            cadence_core::filter_patterns::FilterGrooveCatalog::load_from_roots(catalog.roots());
        (catalog, grooves)
    });
    let process_grooves = match &custom_presets {
        Some((_, grooves)) => grooves.clone(),
        None => cadence_core::filter_patterns::FilterGrooveCatalog::discover(
            Some(&save_path),
            &user_config,
        ),
    };
    cadence_core::filter_patterns::set_filter_grooves(process_grooves);
    let mut app = App::with_host_io(
        project,
        flags,
        save_path.clone(),
        library,
        Arc::new(CoreHostIo),
    )?;
    if let Some((catalog, grooves)) = custom_presets {
        app.set_filter_grooves(grooves);
        app.set_preset_catalog(catalog);
    }
    let mut has_custom_status = false;
    if let Some(status) = import_status {
        app.status = status;
        has_custom_status = true;
    }
    let batch_mode = args.script.is_some()
        || args.key_script.is_some()
        || args.render.is_some()
        || args.stems.is_some()
        || args.check
        || args.output.is_some();
    let mut stream_request = user_config.audio_stream_request();
    if let Some(dev) = &args.audio_device {
        let trimmed = dev.trim();
        if !trimmed.is_empty() {
            stream_request.output_device = Some(trimmed.to_owned());
        }
    }
    if let Some(frames) = args.buffer_size {
        stream_request.buffer_frames = Some(frames);
    }
    let interactive = !batch_mode && !args.stdio;
    if interactive {
        app.apply_config(user_config);
        app.set_config_persist_on_change(!config_corrupt_on_disk);
        if let Some(opened) = args.project.as_ref().or(args.import_rbs.as_ref()) {
            app.config.record_recent_project(opened);
            if !config_corrupt_on_disk {
                if let Err(err) = app.config.save_default() {
                    let msg = format!("Could not save recent project to config: {err:#}");
                    eprintln!("cadence: warning: {msg}");
                    if startup_warning.is_none() {
                        startup_warning = Some(msg);
                    }
                }
            }
        }
    }
    let mut session_lock = None;
    let mut session_dir = None;
    if interactive || args.recover || args.session_dir.is_some() {
        let dir = match &args.session_dir {
            Some(path) => cadence_core::store::SessionDir::at(path.clone()),
            None => cadence_core::store::SessionDir::for_project(&save_path)?,
        };
        session_lock = Some(dir.acquire_lock()?);
        app.set_session_takes_dir(dir.takes_dir());
        if args.recover {
            let (rec_project, rec_flags) = dir
                .load_recovery()?
                .context("No crash-recovery snapshot found for this project")?;
            app.restore_recovered_snapshot(rec_project, rec_flags)?;
        } else if dir.has_recovery() {
            app.status = "Crash-recovery snapshot found · restart with --recover to restore".into();
            has_custom_status = true;
        }
        session_dir = Some(dir);
    }
    if let Some(warning) = startup_warning.take() {
        if has_custom_status {
            app.status = format!("{warning} · {}", app.status);
        } else {
            app.status = warning;
            has_custom_status = true;
        }
    }
    let _session_lock = session_lock;
    if let Some(path) = &args.script {
        let reader = BufReader::new(
            File::open(path).with_context(|| format!("opening script {}", path.display()))?,
        );
        host::process_requests(&mut app, reader, io::stdout().lock(), None)?;
    }
    if let Some(path) = &args.key_script {
        let summary = host::replay_key_script(&mut app, path)?;
        println!("{summary}");
    }
    if args.check {
        app.project().validate()?;
        println!(
            "{}",
            serde_json::json!({
                "valid": true,
                "ready": app.ready(),
                "sample_issues": app.sample_issues(),
                "title": app.project().title,
                "bpm": app.project().bpm,
                "patterns": app.project().patterns.len(),
                "instruments": app.project().instruments.len(),
                "parts": app.project().parts.len(),
                "lanes": app.project().lanes.len(),
                "bars": app.project().bars,
                "arrangement_steps": app.project().bars * 16,
                "revision": app.revision(),
            })
        );
        app.ensure_samples_ready()?;
    }
    if let Some(path) = &args.output {
        app.save_project(path)?;
    }
    if let (Some(path), Some(take_path)) = (&args.render, &args.take) {
        let take = cadence_core::take::Take::load(take_path)?;
        let scores = take.compile(args.take_tail_bars)?;
        let bank = host::load_take_samples(&scores, &mut app.assets.library)?;
        let schedule: Vec<_> = scores.iter().map(|(at, c)| (*at, c)).collect();
        let tail_steps = u64::from(args.take_tail_bars).saturating_mul(16);
        let steps = take
            .steps()
            .checked_add(tail_steps)
            .context("Take render length overflowed")?;
        let report = audio::render_schedule_report(
            &schedule,
            take.header.start_step,
            steps,
            bank,
            path,
            args.sample_rate,
        )?;
        if let Some(report_path) = &args.render_report {
            write_render_report(report_path, &report)?;
        }
        eprintln!(
            "Rendered take of {} bars ({} changes) at {} Hz to {}",
            steps.div_ceil(16),
            take.events.len(),
            args.sample_rate,
            path.display()
        );
    } else if let Some(path) = &args.render {
        app.ensure_samples_ready()?;
        let bars = args.bars.unwrap_or(app.project().bars as u32);
        let report = audio::render_wav_with_samples_report(
            app.compiled_project(),
            app.sample_bank(),
            path,
            bars,
            args.sample_rate,
        )?;
        if let Some(report_path) = &args.render_report {
            write_render_report(report_path, &report)?;
        }
        eprintln!(
            "Rendered {} bars at {} Hz to {}",
            bars,
            args.sample_rate,
            path.display()
        );
    }
    if let Some(stems_dir) = &args.stems {
        app.ensure_samples_ready()?;
        let bars = args.bars.unwrap_or(app.project().bars as u32);
        let stems = audio::render_stems_with_samples_report(
            app.project(),
            app.flags(),
            app.sample_bank(),
            stems_dir,
            0,
            u64::from(bars) * 16,
            args.sample_rate,
        )?;
        eprintln!(
            "Rendered Master + {} channel stems ({} bars @ {} Hz) to {}",
            stems.stem_paths.len(),
            bars,
            args.sample_rate,
            stems_dir.display()
        );
    }
    if batch_mode {
        return Ok(());
    }

    if !app.sample_issues().is_empty() {
        eprintln!("{} sampler part(s) unavailable; affected parts are silent. Use :assets to inspect or repair.", app.sample_issues().len());
    }
    let (mut engine, warning) = host::start_audio_with_config(&app, args.silent, stream_request)?;
    app.set_audio_inventory(engine.enumerate_inventory());
    if args.stdio {
        if let Some(warning) = warning {
            eprintln!("{warning}");
        }
        eprintln!("Cadence JSONL engine: {}", engine.description());
        let result = host::process_requests(
            &mut app,
            io::BufReader::new(io::stdin()),
            io::stdout().lock(),
            Some(&mut engine),
        );
        engine.shutdown();
        return result;
    }
    if let Some(warning) = warning {
        if has_custom_status {
            app.status = format!("{warning} · {}", app.status);
        } else {
            app.status = warning;
        }
    }
    #[cfg(feature = "gui")]
    {
        let ctl_server = if let Some(ctl_arg) = &args.ctl {
            let trimmed = ctl_arg.trim();
            let (sock_path, ear_path) = if trimmed.is_empty() {
                let dir = session_dir
                    .as_ref()
                    .context("Session directory unavailable for default --ctl socket")?;
                (dir.ctl_socket_path(), dir.ear_wav_path())
            } else {
                let explicit = PathBuf::from(trimmed);
                let ear = session_dir
                    .as_ref()
                    .map(|d| d.ear_wav_path())
                    .unwrap_or_else(|| {
                        explicit
                            .parent()
                            .filter(|p| !p.as_os_str().is_empty())
                            .unwrap_or_else(|| Path::new("."))
                            .join("ear.wav")
                    });
                (explicit, ear)
            };
            let server = host::ctl::CtlServer::bind(sock_path, ear_path)?;
            eprintln!("Cadence control socket: {}", server.path().display());
            Some(server)
        } else {
            None
        };
        let result = host::run_gui(
            &mut app,
            &mut engine,
            args.midi_in.as_deref(),
            args.midi_out.as_deref(),
            session_dir,
            ctl_server,
        );
        engine.shutdown();
        result
    }
    #[cfg(not(feature = "gui"))]
    {
        let _ = (session_dir, args.ctl);
        engine.shutdown();
        bail!("This build has no GUI support; build with the gui feature or use --render / --stdio")
    }
}

#[cfg(test)]
mod cli_tests {
    use super::*;

    #[test]
    fn bare_ctl_flag_parses_with_and_without_explicit_socket() {
        let bare = Args::try_parse_from(["cadence", "--silent", "--ctl"]).unwrap();
        assert_eq!(bare.ctl.as_deref(), Some(""));

        let with_session =
            Args::try_parse_from(["cadence", "--silent", "--session-dir", "/tmp/s", "--ctl"])
                .unwrap();
        assert_eq!(with_session.ctl.as_deref(), Some(""));

        let explicit =
            Args::try_parse_from(["cadence", "--silent", "--ctl", "/tmp/cadence.sock"]).unwrap();
        assert_eq!(explicit.ctl.as_deref(), Some("/tmp/cadence.sock"));
    }

    #[test]
    fn audio_device_and_buffer_size_flags_parse_cleanly() {
        let parsed = Args::try_parse_from([
            "cadence",
            "--audio-device",
            "MacBook Pro Speakers",
            "--buffer-size",
            "512",
        ])
        .unwrap();
        assert_eq!(parsed.audio_device.as_deref(), Some("MacBook Pro Speakers"));
        assert_eq!(parsed.buffer_size, Some(512));
        assert!(has_top_level_session_flags(&parsed));

        assert!(
            Args::try_parse_from(["cadence", "--silent", "--buffer-size", "256"]).is_err(),
            "--buffer-size must conflict with --silent"
        );
    }

    #[test]
    fn render_kit_accepts_standalone_synth_preset_and_voices_dir() {
        let synth_only = Args::try_parse_from([
            "cadence",
            "--render-kit",
            "factory-sources/tr-808/project.json",
            "--out-synth-preset",
            "presets/kits/tr-808-synth.json",
        ])
        .unwrap();
        assert!(synth_only.out_samples.is_none());
        assert!(synth_only.out_preset.is_none());
        assert_eq!(
            synth_only.out_synth_preset.as_deref(),
            Some(Path::new("presets/kits/tr-808-synth.json"))
        );

        let voices_only = Args::try_parse_from([
            "cadence",
            "--render-kit",
            "factory-sources/tr-808/project.json",
            "--out-voices-dir",
            "presets/drums",
        ])
        .unwrap();
        assert_eq!(
            voices_only.out_voices_dir.as_deref(),
            Some(Path::new("presets/drums"))
        );
    }

    #[test]
    fn cli_rejects_mutually_exclusive_flag_combinations() {
        for bad_args in [
            vec!["cadence", "--stdio", "--check"],
            vec!["cadence", "--stdio", "--output", "ignored.json"],
            vec!["cadence", "--write-demo", "ignored.json", "--check"],
            vec![
                "cadence",
                "--write-demo",
                "ignored.json",
                "--script",
                "missing.jsonl",
            ],
            vec![
                "cadence",
                "--write-demo",
                "ignored.json",
                "--render",
                "ignored.wav",
            ],
            vec!["cadence", "--empty", "--project", "foo.json"],
            vec!["cadence", "--empty", "--import-rbs", "foo.rbs"],
            vec!["cadence", "--stdio", "--stems", "stems_dir"],
            vec!["cadence", "--stdio", "--ctl"],
            vec!["cadence", "--write-demo", "a.json", "--stems", "stems_dir"],
            vec!["cadence", "--list-midi", "--render", "a.wav"],
            vec!["cadence", "--ctl", "--render", "a.wav"],
            vec!["cadence", "--ctl", "--check"],
        ] {
            let err = Args::try_parse_from(&bad_args).unwrap_err();
            assert_eq!(
                err.exit_code(),
                2,
                "expected clap conflict exit code 2 for {bad_args:?}, got {err}"
            );
        }
    }
}
