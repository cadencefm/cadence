//! Offline kit export: renders/exports each isolated Audio channel of a Cadence source
//! project into trimmed, click-free 16-bit WAV samples + a baked sample-kit preset,
//! a live multi-zone modular synth kit preset (`--out-synth-preset`), and/or individual
//! 1-zone modular drum voice presets (`--out-voices-dir`).
use anyhow::{bail, ensure, Context, Result};
use cadence_app::load_document;
use cadence_core::assets::load_project_samples;
use cadence_core::dsp::{hann_fade_out, lin_to_db};
use cadence_core::material::{InstrumentInstance, Project};
use cadence_core::mixer::{Channel, ChannelKind, LiveFlags};
use cadence_core::params::InstrumentParam;
use cadence_core::presets::{InstrumentPreset, INSTRUMENT_PRESET_VERSION};
use cadence_core::rack::{
    drum_midi, ModuleKind, ModuleParam, PanelPin, RackParam, RackPatch, RackZone, MAX_PANEL_PINS,
};
use cadence_core::sampler::{write_wav_with_metadata, SampleLibrary, SampleMetadata};
use cadence_engine::audio;
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

/// -72 dBFS linear threshold for trailing silence trimming (`10^(-72/20)`).
const TAIL_THRESHOLD: f32 = 0.000_251_188_64;
/// Sample-accurate onset threshold for leading silence trimming.
const ONSET_THRESHOLD: f32 = 1e-6;
/// Smooth half-cosine fade-out duration at the trimmed tail end (milliseconds).
const TAIL_FADE_MS: f64 = 2.0;
/// Stereo difference below which a voice is written as a compact mono WAV.
const MONO_EPSILON: f32 = 1e-5;

/// Configuration for [`render_kit`].
///
/// At least one output (`out_samples`+`out_preset`, `out_synth_preset`, or `out_voices_dir`)
/// must be provided. `out_samples` and `out_preset` must always be paired together.
#[derive(Clone, Debug)]
pub struct KitRenderOptions<'a> {
    pub source_project: &'a Path,
    pub out_samples: Option<&'a Path>,
    pub out_preset: Option<&'a Path>,
    pub out_synth_preset: Option<&'a Path>,
    pub out_voices_dir: Option<&'a Path>,
    pub kit_name_override: Option<&'a str>,
    pub sample_rate: u32,
    pub extra_sample_dirs: &'a [PathBuf],
}

#[derive(Clone, Debug)]
pub struct RenderedVoiceSummary {
    pub index: usize,
    pub name: String,
    pub midi: u8,
    pub choke_group: Option<u8>,
    pub channels: u16,
    pub frames: usize,
    pub peak: f32,
    pub dc_abs: f32,
    pub wav_path: PathBuf,
    pub locator: String,
    pub bytes: u64,
}

#[derive(Clone, Debug)]
pub struct ExportedVoicePreset {
    pub index: usize,
    pub name: String,
    pub midi: u8,
    pub path: PathBuf,
    pub modules: usize,
    pub cables: usize,
    pub pins: usize,
}

#[derive(Clone, Debug)]
pub struct KitRenderSummary {
    pub kit_name: String,
    pub synth_kit_name: String,
    pub preset_path: Option<PathBuf>,
    pub synth_preset_path: Option<PathBuf>,
    pub synth_zone_count: usize,
    pub synth_pin_count: usize,
    pub voice_presets: Vec<ExportedVoicePreset>,
    pub sample_rate: u32,
    pub voices: Vec<RenderedVoiceSummary>,
    pub total_bytes: u64,
}

fn slugify_voice(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if (ch.is_whitespace() || ch == '-' || ch == '_')
            && !out.ends_with('-')
            && !out.is_empty()
        {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "voice".into()
    } else {
        trimmed.to_string()
    }
}

fn remap_pin_to_zone(pin: &PanelPin, zone_id: u16, voice_name: &str) -> Option<PanelPin> {
    let target = match pin.target {
        InstrumentParam::Zone { param, .. } => InstrumentParam::Zone {
            zone: zone_id,
            param,
        },
        InstrumentParam::Module { module, param, .. } => InstrumentParam::Module {
            zone: zone_id,
            module,
            param,
        },
        InstrumentParam::Cable { cable, .. } => InstrumentParam::Cable {
            zone: zone_id,
            cable,
        },
        InstrumentParam::Rack { .. } => return None,
    };
    let raw_label = format!("{} {}", voice_name.trim(), pin.display_label());
    let label: String = raw_label
        .chars()
        .filter(|c| !c.is_control())
        .take(32)
        .collect();
    Some(PanelPin { target, label })
}

fn voice_reference_velocity(project: &Project, instrument_id: u64) -> f32 {
    let mut max_vel = 0.0_f32;
    for part in project
        .parts
        .iter()
        .filter(|p| p.instrument == instrument_id)
    {
        if let Some(pat) = project.patterns.iter().find(|p| p.id == part.pattern) {
            if let cadence_core::material::PatternKind::Notes { steps } = &pat.kind {
                for step in steps.iter().filter(|s| s.active) {
                    max_vel = max_vel.max((step.velocity * part.velocity_scale).clamp(0.0, 1.0));
                }
            }
        }
    }
    if max_vel > 0.05 {
        max_vel
    } else {
        1.0
    }
}

/// Fold source-project channel/master/velocity gain and pan into a live synth zone so
/// playback matches the baked `[Sampler -> Out(pan = 0)]` WAV kit 1:1.
///
/// Reference bake velocity is folded into `Out.gain_db` so playback at velocity `1.0`
/// drives `OutNode`'s soft safety guard identically to the baked WAV render; the remaining
/// post-`Out` gain (including `FRAC_1_SQRT_2` center-pan parity with baked sample playback)
/// is returned in dB for the caller to lift onto `RackPatch.gain_db` + relative `zone.gain_db`.
fn fold_channel_into_zone(
    zone: &mut RackZone,
    inst_rack: &RackPatch,
    ch: &Channel,
    master_gain: f32,
    ref_velocity: f32,
) -> f32 {
    let ref_vel_db = lin_to_db(ref_velocity.max(1e-6));
    let (out_lo, out_hi) = ModuleParam::GainDb.range();
    let mut rem_ref_db = ref_vel_db;
    for module in &mut zone.modules {
        if let ModuleKind::Out(out) = &mut module.kind {
            let target = out.gain_db + ref_vel_db;
            let clamped = target.clamp(out_lo, out_hi);
            out.gain_db = clamped;
            rem_ref_db = target - clamped;
        }
    }
    let post_db = zone.gain_db
        + inst_rack.gain_db
        + rem_ref_db
        + lin_to_db((ch.strip.gain * master_gain * std::f32::consts::FRAC_1_SQRT_2).max(1e-6));
    zone.gain_db = 0.0;
    zone.pan = (zone.pan + inst_rack.pan + ch.strip.pan).clamp(-1.0, 1.0);
    post_db
}

fn resolve_channel_voice<'a>(project: &'a Project, ch: &Channel) -> Result<&'a InstrumentInstance> {
    ensure!(
        ch.inserts.is_empty(),
        "Audio channel '{}' has {} mixer insert(s) that cannot be embedded into a RackZone; fold FX into the Rack before synth kit export",
        ch.name,
        ch.inserts.len()
    );
    let mut insts = project.instruments.iter().filter(|i| i.channel == ch.id);
    let inst = insts.next().with_context(|| {
        format!(
            "Audio channel '{}' has no routed instrument for synth kit export",
            ch.name
        )
    })?;
    ensure!(
        insts.next().is_none(),
        "Audio channel '{}' has multiple instruments; synth kit export requires 1 instrument per Audio channel",
        ch.name
    );
    ensure!(
        inst.rack.zones.len() == 1,
        "Instrument '{}' on channel '{}' has {} zones; synth kit export requires a 1-zone Rack per Audio channel",
        inst.name,
        ch.name,
        inst.rack.zones.len()
    );
    Ok(inst)
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PathAnchor {
    Relative,
    RootDir,
    Prefix(String),
}

struct NormalizedPath {
    anchor: PathAnchor,
    leading_parents: usize,
    segments: Vec<String>,
}

fn normalize_lexical(path: &Path) -> NormalizedPath {
    let mut anchor = PathAnchor::Relative;
    let mut leading_parents = 0usize;
    let mut segments = Vec::new();
    for comp in path.components() {
        match comp {
            Component::Prefix(prefix) => {
                anchor = PathAnchor::Prefix(prefix.as_os_str().to_string_lossy().into_owned());
            }
            Component::RootDir => {
                if anchor == PathAnchor::Relative {
                    anchor = PathAnchor::RootDir;
                }
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if segments.pop().is_none() && anchor == PathAnchor::Relative {
                    leading_parents += 1;
                }
            }
            Component::Normal(os) => {
                segments.push(os.to_string_lossy().into_owned());
            }
        }
    }
    NormalizedPath {
        anchor,
        leading_parents,
        segments,
    }
}

/// Build a portable forward-slashed locator for `wav_path`, anchoring at
/// `factory-samples/...` whenever that directory appears in the normalized path,
/// otherwise returning a clean relative path (either directly or relative to
/// `preset_dir`). Never emits a raw absolute host OS path.
fn portable_kit_locator(wav_path: &Path, preset_dir: Option<&Path>) -> Result<String> {
    let norm_wav = normalize_lexical(wav_path);
    ensure!(
        !norm_wav.segments.is_empty(),
        "Kit sample path '{}' has no filename",
        wav_path.display()
    );
    if let Some(pos) = norm_wav
        .segments
        .iter()
        .position(|p| p.eq_ignore_ascii_case("factory-samples"))
    {
        if pos + 1 < norm_wav.segments.len() {
            return Ok(norm_wav.segments[pos..].join("/"));
        }
    }
    if norm_wav.anchor == PathAnchor::Relative && norm_wav.leading_parents == 0 {
        return Ok(norm_wav.segments.join("/"));
    }
    if let Some(base) = preset_dir {
        let norm_base = normalize_lexical(base);
        if norm_wav.anchor == norm_base.anchor
            && norm_wav.leading_parents == norm_base.leading_parents
        {
            let common = norm_base
                .segments
                .iter()
                .zip(norm_wav.segments.iter())
                .take_while(|(a, b)| a == b)
                .count();
            // For absolute paths, require at least one shared non-root ancestor directory so
            // unrelated host directories (e.g. `/var/...` vs `/etc/...`) are rejected cleanly.
            if norm_wav.anchor == PathAnchor::Relative || common >= 1 {
                let mut rel = Vec::with_capacity(
                    norm_base.segments.len() - common + norm_wav.segments.len() - common,
                );
                rel.extend(std::iter::repeat_n("..", norm_base.segments.len() - common));
                for seg in &norm_wav.segments[common..] {
                    rel.push(seg.as_str());
                }
                let locator = rel.join("/");
                ensure!(
                    !locator.is_empty()
                        && !Path::new(&locator).is_absolute()
                        && !locator.starts_with('/'),
                    "Computed kit locator '{locator}' is not portable"
                );
                return Ok(locator);
            }
        }
    }
    bail!(
        "Kit sample path '{}' is outside factory-samples/ and cannot be expressed as a portable relative locator",
        wav_path.display()
    )
}

/// Export each active Audio channel of `opts.source_project` into any combination of:
/// - baked 16-bit WAV samples (`out_samples`) + `[Sampler -> Out]` `InstrumentPreset` (`out_preset`)
/// - live multi-zone modular synth kit `InstrumentPreset` (`out_synth_preset`)
/// - individual 1-zone modular synth drum voice `InstrumentPreset`s (`out_voices_dir`)
pub fn render_kit(opts: &KitRenderOptions<'_>) -> Result<KitRenderSummary> {
    ensure!(
        opts.out_samples.is_some() == opts.out_preset.is_some(),
        "--out-samples and --out-preset must be specified together"
    );
    ensure!(
        opts.out_preset.is_some()
            || opts.out_synth_preset.is_some()
            || opts.out_voices_dir.is_some(),
        "--render-kit requires at least one output: --out-samples + --out-preset, --out-synth-preset, or --out-voices-dir"
    );

    let (project, base_flags) = load_document(opts.source_project)?;
    project.validate()?;

    let audio_channels: Vec<_> = project
        .mixer
        .channels
        .iter()
        .filter(|c| c.kind == ChannelKind::Audio)
        .filter(|c| !base_flags.channel_muted(cadence_core::mixer::ChannelRef::Channel(c.id)))
        .collect();
    ensure!(
        !audio_channels.is_empty(),
        "Source project '{}' has no active Audio channels to export",
        opts.source_project.display()
    );

    let kit_name = opts
        .kit_name_override
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(project.title.trim())
        .to_string();
    let synth_kit_name = if kit_name.to_ascii_lowercase().ends_with("synth") {
        kit_name.clone()
    } else {
        format!("{kit_name} Synth")
    };

    // Resolve canonical MIDI note + choke group once so baked kit, live synth kit,
    // and single-voice presets share identical mappings.
    let mut used_notes = Vec::with_capacity(audio_channels.len());
    let mut mappings = Vec::with_capacity(audio_channels.len());
    for ch in &audio_channels {
        let (midi, choke) = drum_midi::infer_mapping(&ch.name, &used_notes);
        used_notes.push(midi);
        mappings.push((midi, choke));
    }

    let mut voices = Vec::new();
    let mut total_bytes = 0u64;
    let mut preset_path = None;

    if let (Some(out_samples), Some(out_preset)) = (opts.out_samples, opts.out_preset) {
        ensure!(
            (8_000..=192_000).contains(&opts.sample_rate),
            "Sample rate must be between 8000 and 192000 Hz"
        );
        let preset_dir = out_preset.parent().filter(|p| !p.as_os_str().is_empty());
        // Fail fast before rendering audio or creating output directories if sample locators would be non-portable.
        portable_kit_locator(&out_samples.join("01-voice.wav"), preset_dir)?;

        std::fs::create_dir_all(out_samples).with_context(|| {
            format!(
                "Creating kit sample output directory {}",
                out_samples.display()
            )
        })?;
        if let Some(parent) = preset_dir {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("Creating kit preset output directory {}", parent.display())
            })?;
        }

        let mut library =
            SampleLibrary::discover(opts.extra_sample_dirs, Some(opts.source_project));
        let full_score = project.compile_with_flags(&base_flags)?;
        let loaded_samples = load_project_samples(&full_score, &mut library);
        if !loaded_samples.issues.is_empty() {
            let first = &loaded_samples.issues[0];
            bail!(
                "Source project '{}' has unavailable sample '{}': {}",
                opts.source_project.display(),
                first.instrument_name,
                first.message
            );
        }

        let steps = (project.bars as u64) * 16;
        let fade_frames =
            ((opts.sample_rate as f64 * TAIL_FADE_MS / 1000.0).round() as usize).max(16);

        let mut zones = Vec::with_capacity(audio_channels.len());
        voices.reserve(audio_channels.len());

        for (idx, (ch, &(midi, choke_group))) in
            audio_channels.iter().zip(mappings.iter()).enumerate()
        {
            let mut solo_flags = LiveFlags::default();
            solo_flags.solo_channels.insert(ch.id);
            let score = project.compile_with_flags(&solo_flags)?;
            let (report, frames) = audio::render_schedule_f32(
                &[(0, &score)],
                0,
                steps,
                loaded_samples.bank.clone(),
                opts.sample_rate,
            )?;
            ensure!(
                report.channels.iter().all(|c| !c.clipped),
                "Voice '{}' clipped during render (reduce channel gain in source project)",
                ch.name
            );

            let onset = frames
                .iter()
                .position(|f| f[0].abs().max(f[1].abs()) > ONSET_THRESHOLD)
                .with_context(|| {
                    format!(
                        "Voice '{}' produced silence — check its pattern/placement in {}",
                        ch.name,
                        opts.source_project.display()
                    )
                })?;
            let last_audible = frames
                .iter()
                .rposition(|f| f[0].abs().max(f[1].abs()) > TAIL_THRESHOLD)
                .unwrap_or(onset);
            let end = (last_audible + 1 + fade_frames).min(frames.len());
            ensure!(
                end > onset,
                "Voice '{}' produced an empty trimmed slice",
                ch.name
            );

            let slice = &frames[onset..end];
            let mut left: Vec<f32> = Vec::with_capacity(slice.len());
            let mut right: Vec<f32> = Vec::with_capacity(slice.len());
            let mut peak = 0.0_f32;
            let mut max_lr_diff = 0.0_f32;
            let mut sum_l = 0.0_f64;
            let mut sum_r = 0.0_f64;
            for &frame in slice {
                ensure!(
                    frame[0].is_finite() && frame[1].is_finite(),
                    "Voice '{}' produced non-finite samples",
                    ch.name
                );
                left.push(frame[0]);
                right.push(frame[1]);
                peak = peak.max(frame[0].abs()).max(frame[1].abs());
                max_lr_diff = max_lr_diff.max((frame[0] - frame[1]).abs());
                sum_l += f64::from(frame[0]);
                sum_r += f64::from(frame[1]);
            }
            ensure!(
                peak <= 1.0,
                "Voice '{}' peak {:.4} exceeds 0 dBFS",
                ch.name,
                peak
            );

            // Apply 2 ms smooth half-cosine fade-out so the tail lands cleanly on 0.0 with zero click.
            let actual_fade = fade_frames.min(left.len());
            let fade_start = left.len() - actual_fade;
            for i in 0..actual_fade {
                let t = if actual_fade <= 1 {
                    1.0_f32
                } else {
                    i as f32 / (actual_fade - 1) as f32
                };
                let gain = hann_fade_out(t);
                left[fade_start + i] *= gain;
                right[fade_start + i] *= gain;
            }

            let dc_abs = (sum_l.abs().max(sum_r.abs()) / left.len().max(1) as f64) as f32;
            let filename = format!("{:02}-{}.wav", idx + 1, slugify_voice(&ch.name));
            let wav_path = out_samples.join(&filename);
            let locator = portable_kit_locator(&wav_path, preset_dir)?;
            let meta = SampleMetadata {
                root_midi: midi,
                has_root: true,
                low_midi: Some(midi),
                high_midi: Some(midi),
                low_vel: Some(0),
                high_vel: Some(127),
                ..SampleMetadata::default()
            };
            let is_mono = max_lr_diff < MONO_EPSILON;
            write_wav_with_metadata(
                &wav_path,
                &left,
                if is_mono { None } else { Some(&right) },
                opts.sample_rate,
                &meta,
            )?;
            let bytes = std::fs::metadata(&wav_path).map(|m| m.len()).unwrap_or(0);
            total_bytes += bytes;

            let mut zone =
                RackZone::sample_pad((idx + 1) as u16, ch.name.clone(), midi, locator.clone());
            zone.choke_group = choke_group;
            zones.push(zone);
            voices.push(RenderedVoiceSummary {
                index: idx + 1,
                name: ch.name.clone(),
                midi,
                choke_group,
                channels: if is_mono { 1 } else { 2 },
                frames: left.len(),
                peak,
                dc_abs,
                wav_path,
                locator,
                bytes,
            });
        }

        let patch = RackPatch {
            kit_name: kit_name.clone(),
            zones,
            gain_db: 0.0,
            pan: 0.0,
            tune_semitones: 0.0,
            cutoff_hz: 20_000.0,
            resonance: 0.0,
            panel_pins: Vec::new(),
        };
        patch.validate()?;
        let preset = InstrumentPreset {
            version: INSTRUMENT_PRESET_VERSION,
            name: kit_name.clone(),
            category: Some("Kit".into()),
            patch,
        };
        preset.save(out_preset)?;
        preset_path = Some(out_preset.to_path_buf());
    }

    let mut synth_preset_path = None;
    let mut synth_zone_count = 0usize;
    let mut synth_pin_count = 0usize;
    let mut voice_presets = Vec::new();

    if opts.out_synth_preset.is_some() || opts.out_voices_dir.is_some() {
        let mut resolved_voices = Vec::with_capacity(audio_channels.len());
        for (ch, &(midi, choke_group)) in audio_channels.iter().zip(mappings.iter()) {
            let inst = resolve_channel_voice(&project, ch)?;
            resolved_voices.push((ch, inst, midi, choke_group));
        }

        if let Some(out_synth_preset) = opts.out_synth_preset {
            if let Some(parent) = out_synth_preset
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
            {
                std::fs::create_dir_all(parent).with_context(|| {
                    format!(
                        "Creating synth kit preset output directory {}",
                        parent.display()
                    )
                })?;
            }
            let mut synth_zones = Vec::with_capacity(resolved_voices.len());
            let mut post_dbs = Vec::with_capacity(resolved_voices.len());
            let mut per_voice_pins: Vec<(u16, &str, &[PanelPin])> =
                Vec::with_capacity(resolved_voices.len());

            for (idx, (ch, inst, midi, choke_group)) in resolved_voices.iter().enumerate() {
                let zone_id = (idx + 1) as u16;
                let mut zone = inst.rack.zones[0].clone();
                zone.id = zone_id;
                zone.name = ch.name.clone();
                zone.low_midi = *midi;
                zone.high_midi = *midi;
                zone.set_root_midi(*midi);
                zone.choke_group = zone.choke_group.or(*choke_group);
                let ref_vel = voice_reference_velocity(&project, inst.id);
                let post_db = fold_channel_into_zone(
                    &mut zone,
                    &inst.rack,
                    ch,
                    project.mixer.master.strip.gain,
                    ref_vel,
                );
                synth_zones.push(zone);
                post_dbs.push(post_db);
                per_voice_pins.push((zone_id, ch.name.as_str(), &inst.rack.panel_pins));
            }

            let (rack_lo, rack_hi) = RackParam::Gain.range();
            let mean_post_db = if post_dbs.is_empty() {
                0.0
            } else {
                post_dbs.iter().copied().sum::<f32>() / post_dbs.len() as f32
            };
            let raw_kit_gain = mean_post_db.clamp(rack_lo, rack_hi);
            let kit_gain_db = if raw_kit_gain.abs() < 1e-4 {
                0.0
            } else {
                raw_kit_gain
            };
            for (zone, post_db) in synth_zones.iter_mut().zip(post_dbs) {
                let rem = post_db - kit_gain_db;
                zone.gain_db = if rem.abs() < 1e-4 {
                    0.0
                } else {
                    rem.clamp(RackZone::MIN_GAIN_DB, RackZone::MAX_GAIN_DB)
                };
            }

            // Round-robin pick up to MAX_PANEL_PINS curated pins across all voices.
            let max_pin_depth = per_voice_pins
                .iter()
                .map(|(_, _, pins)| pins.len())
                .max()
                .unwrap_or(0);
            let mut panel_pins = Vec::with_capacity(MAX_PANEL_PINS);
            let mut seen_targets = HashSet::with_capacity(MAX_PANEL_PINS);
            'outer: for slot in 0..max_pin_depth {
                for &(zone_id, voice_name, pins) in &per_voice_pins {
                    if let Some(pin) = pins.get(slot) {
                        if let Some(remapped) = remap_pin_to_zone(pin, zone_id, voice_name) {
                            if seen_targets.insert(remapped.target) {
                                panel_pins.push(remapped);
                                if panel_pins.len() >= MAX_PANEL_PINS {
                                    break 'outer;
                                }
                            }
                        }
                    }
                }
            }

            let mut patch = RackPatch {
                kit_name: synth_kit_name.clone(),
                zones: synth_zones,
                gain_db: kit_gain_db,
                pan: 0.0,
                tune_semitones: 0.0,
                cutoff_hz: 20_000.0,
                resonance: 0.0,
                panel_pins,
            };
            patch.prune_invalid_pins();
            patch.validate()?;
            synth_zone_count = patch.zones.len();
            synth_pin_count = patch.panel_pins.len();

            let preset = InstrumentPreset {
                version: INSTRUMENT_PRESET_VERSION,
                name: synth_kit_name.clone(),
                category: Some("Kit".into()),
                patch,
            };
            preset.save(out_synth_preset)?;
            synth_preset_path = Some(out_synth_preset.to_path_buf());
        }

        if let Some(out_voices_dir) = opts.out_voices_dir {
            std::fs::create_dir_all(out_voices_dir).with_context(|| {
                format!(
                    "Creating voice preset output directory {}",
                    out_voices_dir.display()
                )
            })?;
            let kit_slug = slugify_voice(&kit_name);
            voice_presets.reserve(resolved_voices.len());

            for (idx, (ch, inst, midi, _choke_group)) in resolved_voices.iter().enumerate() {
                let mut voice_rack = inst.rack.clone();
                let zone = &mut voice_rack.zones[0];
                zone.id = 1;
                zone.low_midi = 0;
                zone.high_midi = 127;
                zone.set_root_midi(*midi);
                zone.choke_group = None;
                let ref_vel = voice_reference_velocity(&project, inst.id);
                let post_db = fold_channel_into_zone(
                    zone,
                    &inst.rack,
                    ch,
                    project.mixer.master.strip.gain,
                    ref_vel,
                );
                let (rack_lo, rack_hi) = RackParam::Gain.range();
                let raw_rack_gain = post_db.clamp(rack_lo, rack_hi);
                let rack_gain_db = if raw_rack_gain.abs() < 1e-4 {
                    0.0
                } else {
                    raw_rack_gain
                };
                let rem = post_db - rack_gain_db;
                zone.gain_db = if rem.abs() < 1e-4 {
                    0.0
                } else {
                    rem.clamp(RackZone::MIN_GAIN_DB, RackZone::MAX_GAIN_DB)
                };
                voice_rack.kit_name.clear();
                voice_rack.gain_db = rack_gain_db;
                voice_rack.pan = 0.0;
                voice_rack.prune_invalid_pins();
                voice_rack.validate()?;

                let preset_name = format!("{kit_name} {}", ch.name);
                let filename = format!("{kit_slug}-{}.json", slugify_voice(&ch.name));
                let voice_path = out_voices_dir.join(filename);
                let modules = voice_rack.zones[0].modules.len();
                let cables = voice_rack.zones[0].cables.len();
                let pins = voice_rack.panel_pins.len();

                let preset = InstrumentPreset {
                    version: INSTRUMENT_PRESET_VERSION,
                    name: preset_name.clone(),
                    category: Some("Drum".into()),
                    patch: voice_rack,
                };
                preset.save(&voice_path)?;
                voice_presets.push(ExportedVoicePreset {
                    index: idx + 1,
                    name: preset_name,
                    midi: *midi,
                    path: voice_path,
                    modules,
                    cables,
                    pins,
                });
            }
        }
    }

    Ok(KitRenderSummary {
        kit_name,
        synth_kit_name,
        preset_path,
        synth_preset_path,
        synth_zone_count,
        synth_pin_count,
        voice_presets,
        sample_rate: opts.sample_rate,
        voices,
        total_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_kit_locator_anchors_factory_samples_relativizes_and_rejects_absolute_paths() {
        // Anchors at `factory-samples/...` regardless of host prefix.
        assert_eq!(
            portable_kit_locator(
                Path::new("/Users/alice/workspace/factory-samples/tr-808/01-bd.wav"),
                None,
            )
            .unwrap(),
            "factory-samples/tr-808/01-bd.wav"
        );
        assert_eq!(
            portable_kit_locator(Path::new("factory-samples/edm/02-sd.wav"), None).unwrap(),
            "factory-samples/edm/02-sd.wav"
        );

        // Traversal out of `factory-samples/` must NOT fool the anchor check.
        assert!(
            portable_kit_locator(
                Path::new("/Users/alice/factory-samples/../private/01-bd.wav"),
                None,
            )
            .is_err(),
            "path escaping factory-samples via .. without a shared base must fail"
        );

        // Clean relative paths outside `factory-samples/` stay portable forward-slashed.
        assert_eq!(
            portable_kit_locator(Path::new("./kits/my-kit/01-bd.wav"), None).unwrap(),
            "kits/my-kit/01-bd.wav"
        );

        // Absolute paths sharing a common ancestor with preset_dir produce clean relative paths.
        assert_eq!(
            portable_kit_locator(
                Path::new("/tmp/studio-kit/samples/01-bd.wav"),
                Some(Path::new("/tmp/studio-kit")),
            )
            .unwrap(),
            "samples/01-bd.wav"
        );
        assert_eq!(
            portable_kit_locator(
                Path::new("/tmp/studio-kit/rendered-kit/01-bd.wav"),
                Some(Path::new("/tmp/studio-kit/presets/kits")),
            )
            .unwrap(),
            "../../rendered-kit/01-bd.wav"
        );

        // Raw absolute paths outside `factory-samples/` with no portable base must be rejected.
        for (wav, base) in [
            (Path::new("/abs/outside/01-bd.wav"), None),
            (
                Path::new("/var/samples/01-bd.wav"),
                Some(Path::new("/etc/presets")),
            ),
            (
                Path::new("/abs/samples/01-bd.wav"),
                Some(Path::new("relative/presets")),
            ),
            (Path::new("../escaped/01-bd.wav"), None),
        ] {
            let err = portable_kit_locator(wav, base);
            assert!(
                err.is_err(),
                "expected non-portable rejection for wav={} base={base:?}, got {err:?}",
                wav.display()
            );
        }
    }
}
