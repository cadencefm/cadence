//! Instrument, preset, sampler, and kit Ex commands plus sound-argument parsers.
use super::{require_args, InstrumentVerb};
use crate::apply::{load_preset_via_io, resolve_preset_file_path_with};
use crate::*;
use cadence_core::rack::RackTemplate;

impl App {
    pub(super) fn execute_instrument_verb(
        &mut self,
        verb: InstrumentVerb,
        args: &[String],
    ) -> Result<Outcome> {
        match verb {
            InstrumentVerb::Instrument => {
                if args.is_empty() {
                    return self.begin_workflow(WorkflowKind::Instrument);
                }
                if args.len() == 1 {
                    let id = self.instrument_id(&args[0])?;
                    return self.apply(Action::SelectInstrument { instrument: id });
                }
                if args[0].eq_ignore_ascii_case("rename") {
                    ensure!(
                        args.len() >= 3,
                        "Usage: :instrument rename INSTRUMENT NEW_NAME"
                    );
                    let instrument = self.instrument_id(&args[1])?;
                    let name = args[2..].join(" ");
                    return self.apply(Action::RenameInstrument { instrument, name });
                }
                let (sound, channel_arg) = parse_instrument_sound_args(
                    &self.assets.presets,
                    self.doc.asset_base(),
                    self.io.as_ref(),
                    &args[1..],
                )?;
                let channel = channel_arg.map(|s| self.channel_id(s)).transpose()?;
                self.apply(Action::CreateInstrument {
                    name: args[0].clone(),
                    sound,
                    channel,
                })
            }
            InstrumentVerb::Kit => {
                ensure!(
                    !args.is_empty(),
                    "Usage: :kit NAME [blank|{}|folder DIR [CHANNEL]]",
                    self.assets.presets.names_hint(Some("Kit"))
                );
                let (sound, channel_arg) = if args.len() == 1 {
                    let default_sound = if self.assets.presets.find("tr-808").is_some() {
                        workflow::SoundChoice::Preset("tr-808".into())
                    } else {
                        workflow::SoundChoice::Template(RackTemplate::Kit)
                    };
                    (default_sound, None)
                } else {
                    parse_kit_sound_args(
                        &self.assets.presets,
                        self.doc.asset_base(),
                        self.io.as_ref(),
                        &args[1..],
                    )?
                };
                let channel = channel_arg.map(|s| self.channel_id(s)).transpose()?;
                self.apply(Action::CreateInstrument {
                    name: args[0].clone(),
                    sound,
                    channel,
                })
            }
            InstrumentVerb::Sampler => {
                ensure!(
                    (1..=3).contains(&args.len()),
                    "Usage: :sampler NAME [PATH|PRESET|blank [CHANNEL]]"
                );
                let (sound, channel_arg) =
                    parse_sampler_sound_args(&self.assets.presets, &args[1..])?;
                let channel = channel_arg.map(|s| self.channel_id(s)).transpose()?;
                self.apply(Action::CreateInstrument {
                    name: args[0].clone(),
                    sound,
                    channel,
                })
            }
            InstrumentVerb::Preset => {
                require_args(args, 1, ":preset NAME|PATH")?;
                if let Some(id) = self.active_instrument() {
                    self.apply(Action::LoadPreset {
                        instrument: id,
                        preset: args[0].clone(),
                    })
                } else {
                    let loaded = self.resolve_instrument_preset(&args[0])?;
                    self.preview_patch = loaded.patch;
                    self.status = format!("Changed transient audition sound to '{}'", loaded.name);
                    Ok(Outcome::default())
                }
            }
            InstrumentVerb::PresetSave => {
                require_args(args, 1, ":preset-save PATH")?;
                self.save_active_instrument_preset(Path::new(&args[0]))?;
                Ok(Outcome::default())
            }
            InstrumentVerb::PresetLoad => {
                require_args(args, 1, ":preset-load PATH|NAME")?;
                let id = self
                    .active_instrument()
                    .or(self.selected_instrument)
                    .context("Select an instrument before loading a preset")?;
                self.load_instrument_preset(id, &args[0])
            }
            InstrumentVerb::Tab => {
                require_args(args, 1, ":tab panel|patchbay|zones")?;
                let tab = RackTab::parse(&args[0])
                    .context("Unknown Rack tab (use panel, patchbay, or zones)")?;
                self.rack_ui.tab = tab;
                self.control_cursor = 0;
                self.reset_midi_pickup();
                self.status = format!("Rack tab: {}", tab.label());
                Ok(Outcome::default())
            }
            InstrumentVerb::ReloadSamples => self.apply(Action::ReloadSamples),
            InstrumentVerb::ReloadPresets => {
                require_args(args, 0, ":reload-presets")?;
                self.reload_presets_with_status();
                Ok(Outcome::default())
            }
            InstrumentVerb::Samples => {
                require_args(args, 0, ":samples")?;
                self.browse_samples()
            }
            InstrumentVerb::Sample => {
                ensure!(!args.is_empty(), "Usage: :sample PATH | deck | write-meta");
                if args.len() == 1 && args[0].eq_ignore_ascii_case("deck") {
                    return self.open_sample_deck();
                }
                if args.len() == 1
                    && (args[0].eq_ignore_ascii_case("write-meta")
                        || args[0].eq_ignore_ascii_case("write-riff"))
                {
                    return self.execute_sample_write_meta_command();
                }
                require_args(args, 1, ":sample PATH | deck | write-meta")?;
                let id = self
                    .active_instrument()
                    .or(self.selected_instrument)
                    .context("Select an instrument first")?;
                let mut rack = self
                    .instrument(id)
                    .context("Instrument missing")?
                    .rack
                    .clone();
                ensure!(!rack.zones.is_empty(), "Rack has no zones");
                let z_idx = rack.clamp_zone_index(self.rack_ui.zone_cursor);
                let preferred = self.focused_rack_module_id(&rack);
                let mod_id = Self::ensure_zone_sampler_module(&mut rack.zones[z_idx], preferred)?;
                let fallback_midi = rack.zones[z_idx].root_midi;
                self.load_and_apply_zone_sample(
                    &mut rack.zones[z_idx],
                    Some(mod_id),
                    &args[0],
                    fallback_midi,
                    true,
                    false,
                )?;
                let mut out = self.apply(Action::Edit {
                    edit: Edit::UpdateRackPatch { id, rack },
                })?;
                if !out.samples_changed {
                    self.sync_loaded_samples();
                    out.samples_changed = true;
                }
                Ok(out)
            }
            InstrumentVerb::Slice => self.execute_slice_command(args),
            InstrumentVerb::Wave => {
                ensure!(args.len() <= 1, "Usage: :wave [INSTRUMENT]");
                if let Some(name) = args.first() {
                    let id = self.instrument_id(name)?;
                    let (inst, zone, module) = self
                        .sampler_target_for_instrument(id)
                        .with_context(|| format!("Instrument '{name}' has no Sampler module"))?;
                    self.open_sample_deck_for(inst, zone, module)
                } else {
                    self.open_sample_deck()
                }
            }
        }
    }
}

fn sample_stem_label(path: &str) -> String {
    Path::new(path.trim())
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("Sample")
        .to_string()
}

fn folder_name_label(dir: &str) -> String {
    Path::new(dir.trim())
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("Kit")
        .to_string()
}

fn parse_sampler_sound_args<'a>(
    presets: &PresetCatalog,
    args: &'a [String],
) -> Result<(workflow::SoundChoice, Option<&'a str>)> {
    ensure!(args.len() <= 2, "Usage: ... [PATH|PRESET|blank [CHANNEL]]");
    let sound = match args.first() {
        None => workflow::SoundChoice::Template(RackTemplate::Sampler),
        Some(tok)
            if PresetCatalog::is_bare_default_token(RackTemplate::Sampler, tok)
                || tok.eq_ignore_ascii_case("none") =>
        {
            workflow::SoundChoice::Template(RackTemplate::Sampler)
        }
        Some(tok) => {
            if let Some(entry) = presets.find(tok) {
                workflow::SoundChoice::Preset(entry.id.clone())
            } else {
                workflow::SoundChoice::Sampler {
                    path: Some(tok.clone()),
                    label: Some(sample_stem_label(tok)),
                }
            }
        }
    };
    Ok((sound, args.get(1).map(String::as_str)))
}

fn parse_kit_sound_args<'a>(
    presets: &PresetCatalog,
    asset_base: &Path,
    io: &dyn HostIo,
    args: &'a [String],
) -> Result<(workflow::SoundChoice, Option<&'a str>)> {
    ensure!(
        !args.is_empty(),
        "Expected blank, {}, or folder <DIR>",
        presets.names_hint(Some("Kit"))
    );
    let head = args[0].to_ascii_lowercase();
    if matches!(head.as_str(), "blank" | "empty") {
        ensure!(args.len() <= 2, "Usage: ... blank [CHANNEL]");
        return Ok((
            workflow::SoundChoice::Template(RackTemplate::Kit),
            args.get(1).map(String::as_str),
        ));
    }
    if head == "folder" {
        ensure!(
            (2..=3).contains(&args.len()),
            "Usage: ... folder <DIR> [CHANNEL]"
        );
        let dir = args[1].clone();
        let label = folder_name_label(&dir);
        return Ok((
            workflow::SoundChoice::Folder { path: dir, label },
            args.get(2).map(String::as_str),
        ));
    }
    if let Some(entry) = presets.find(&args[0]) {
        ensure!(args.len() <= 2, "Usage: ... <PRESET> [CHANNEL]");
        return Ok((
            workflow::SoundChoice::Preset(entry.id.clone()),
            args.get(1).map(String::as_str),
        ));
    }
    if let Some(path) = resolve_preset_file_path_with(io, asset_base, &args[0]) {
        load_preset_via_io(io, &path)?;
        ensure!(args.len() <= 2, "Usage: ... <PRESET> [CHANNEL]");
        return Ok((
            workflow::SoundChoice::Preset(args[0].clone()),
            args.get(1).map(String::as_str),
        ));
    }
    bail!(
        "Unknown Kit preset '{}' (use blank, {}, or folder <DIR>)",
        args[0],
        presets.names_hint(Some("Kit"))
    )
}

pub(super) fn parse_single_sound_token(
    presets: &PresetCatalog,
    asset_base: &Path,
    io: &dyn HostIo,
    token: &str,
) -> Result<workflow::SoundChoice> {
    if let Some(template) = RackTemplate::parse(token) {
        return Ok(workflow::SoundChoice::Template(template));
    }
    if let Some(entry) = presets.find(token) {
        return Ok(workflow::SoundChoice::Preset(entry.id.clone()));
    }
    if let Some(path) = resolve_preset_file_path_with(io, asset_base, token) {
        load_preset_via_io(io, &path)?;
        return Ok(workflow::SoundChoice::Preset(token.trim().to_string()));
    }
    bail!(
        "Unknown sound '{token}' (use synth, sampler, kit, modular, or a preset: {})",
        presets.names_hint(None)
    )
}

fn parse_instrument_sound_args<'a>(
    presets: &PresetCatalog,
    asset_base: &Path,
    io: &dyn HostIo,
    args: &'a [String],
) -> Result<(workflow::SoundChoice, Option<&'a str>)> {
    ensure!(
        !args.is_empty(),
        "Usage: :instrument NAME synth|sampler|kit|modular|folder|<PRESET> ..."
    );
    match args[0].to_ascii_lowercase().as_str() {
        "synth" => {
            ensure!(
                args.len() <= 3,
                "Usage: :instrument NAME synth [PRESET [CHANNEL]]"
            );
            let sound = match args.get(1) {
                None => workflow::SoundChoice::Template(RackTemplate::Synth),
                Some(tok) if PresetCatalog::is_bare_default_token(RackTemplate::Synth, tok) => {
                    workflow::SoundChoice::Template(RackTemplate::Synth)
                }
                Some(tok) => {
                    if let Some(entry) = presets.find(tok) {
                        workflow::SoundChoice::Preset(entry.id.clone())
                    } else if let Some(path) = resolve_preset_file_path_with(io, asset_base, tok) {
                        load_preset_via_io(io, &path)?;
                        workflow::SoundChoice::Preset(tok.clone())
                    } else {
                        bail!(
                            "Unknown preset '{}' (available: {}; bare default: init)",
                            tok,
                            presets.names_hint(None)
                        );
                    }
                }
            };
            Ok((sound, args.get(2).map(String::as_str)))
        }
        "sampler" => {
            ensure!(
                args.len() <= 3,
                "Usage: :instrument NAME sampler [PATH|PRESET|blank [CHANNEL]]"
            );
            parse_sampler_sound_args(presets, &args[1..])
        }
        "kit" => {
            if args.len() == 1 {
                return Ok((workflow::SoundChoice::Template(RackTemplate::Kit), None));
            }
            parse_kit_sound_args(presets, asset_base, io, &args[1..])
        }
        "modular" | "rack" => {
            ensure!(args.len() <= 2, "Usage: :instrument NAME modular [CHANNEL]");
            Ok((
                workflow::SoundChoice::Template(RackTemplate::Modular),
                args.get(1).map(String::as_str),
            ))
        }
        "folder" => parse_kit_sound_args(presets, asset_base, io, args),
        _ => {
            ensure!(
                args.len() <= 2,
                "Usage: :instrument NAME synth|sampler|kit|modular|folder|<PRESET> [CHANNEL]"
            );
            let sound = parse_single_sound_token(presets, asset_base, io, &args[0])?;
            Ok((sound, args.get(1).map(String::as_str)))
        }
    }
}
