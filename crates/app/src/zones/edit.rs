//! Interactive Rack zone nudging, inline editing, and `:zone` command parser.
use super::{format_midi_note, ZoneControl};
use crate::*;
use cadence_core::music::parse_root_midi;
use cadence_core::params::{InstrumentParam, ParamAddress};
use cadence_core::rack::{module_sample_key, ModuleKind, MAX_CHOKE_GROUPS};

fn parse_midi_token(raw: &str) -> Result<u8> {
    let trimmed = raw.trim();
    if let Ok(midi) = trimmed.parse::<u8>() {
        ensure!(midi <= 127, "MIDI note must be within 0..=127");
        return Ok(midi);
    }
    let midi =
        parse_root_midi(trimmed).context("Expected MIDI note 0..127 or note name such as C2")?;
    ensure!(midi <= 127, "MIDI note must be within 0..=127");
    Ok(midi)
}

fn parse_velocity_token(raw: &str) -> Result<f32> {
    param_ops::parse_normalized(raw)
}

fn parse_choke_token(raw: &str) -> Result<Option<u8>> {
    let lower = raw.trim().to_ascii_lowercase();
    if matches!(lower.as_str(), "none" | "off" | "0" | "-") {
        return Ok(None);
    }
    let digits = lower
        .strip_prefix("group")
        .or_else(|| lower.strip_prefix('c'))
        .unwrap_or(&lower)
        .trim();
    let group: u8 = digits
        .parse()
        .with_context(|| format!("Choke group must be none or 1..={MAX_CHOKE_GROUPS}"))?;
    ensure!(
        (1..=MAX_CHOKE_GROUPS).contains(&group),
        "Choke group must be none or 1..={MAX_CHOKE_GROUPS}"
    );
    Ok(Some(group))
}

impl App {
    /// Open inline command entry (`Enter`) — or Sample Browser on `SamplePath` — for a Rack zone control.
    pub(crate) fn open_zone_control(&mut self, id: Id, field: ZoneControl) -> Result<Outcome> {
        if field == ZoneControl::SamplePath {
            if let Some((zone_id, mod_id)) = self.instrument(id).and_then(|i| {
                let idx = i.rack.clamp_zone_index(self.rack_ui.zone_cursor);
                let z = i.rack.zones.get(idx)?;
                let mid = z
                    .modules
                    .iter()
                    .find(|m| matches!(m.kind, ModuleKind::Sampler(_)))
                    .map(|m| m.id);
                Some((z.id, mid))
            }) {
                return self.begin_sample_browser(browser::BrowserTarget::AssignModule {
                    instrument: id,
                    zone_id,
                    module_id: mod_id,
                });
            }
            return Ok(Outcome::default());
        }
        if field.units_hint().is_some() {
            return self.begin_zone_field_value_input(id, field, String::new());
        }
        Ok(Outcome::default())
    }

    /// Preview/validate text input for a structural Rack zone field (SPOT for `Mode::ValueInput`).
    pub(crate) fn preview_zone_field_text(
        &self,
        id: Id,
        field: ZoneControl,
        raw: &str,
    ) -> Result<String> {
        let trimmed = raw.trim();
        ensure!(self.instrument(id).is_some(), "Instrument missing");
        match field {
            ZoneControl::Name => {
                ensure!(
                    !trimmed.is_empty()
                        && trimmed.chars().count() <= 32
                        && !trimmed.chars().any(char::is_control),
                    "Zone name must be 1–32 printable characters"
                );
                Ok(trimmed.to_string())
            }
            ZoneControl::LowMidi | ZoneControl::HighMidi => {
                let midi = parse_midi_token(trimmed)?;
                Ok(format_midi_note(midi))
            }
            ZoneControl::LowVel | ZoneControl::HighVel => {
                let vel = parse_velocity_token(trimmed)?;
                Ok(format!("{:.0}%", vel * 100.0))
            }
            ZoneControl::ChokeGroup => {
                let group = parse_choke_token(trimmed)?;
                Ok(match group {
                    None => "None".into(),
                    Some(g) => format!("Group {g}"),
                })
            }
            ZoneControl::SamplePath => {
                bail!("Control does not accept direct text entry")
            }
        }
    }

    /// Commit validated text input for a structural Rack zone field (SPOT for `Mode::ValueInput`).
    pub(crate) fn apply_zone_field_text(
        &mut self,
        id: Id,
        field: ZoneControl,
        raw: &str,
    ) -> Result<Outcome> {
        let trimmed = raw.trim();
        let mut rack = self.clone_rack_patch(id)?;
        ensure!(!rack.zones.is_empty(), "Rack has no zones");
        let idx = rack.clamp_zone_index(self.rack_ui.zone_cursor);
        let zone = &mut rack.zones[idx];
        let label = match field {
            ZoneControl::Name => {
                ensure!(
                    !trimmed.is_empty()
                        && trimmed.chars().count() <= 32
                        && !trimmed.chars().any(char::is_control),
                    "Zone name must be 1–32 printable characters"
                );
                zone.name = trimmed.to_string();
                format!("Renamed zone z{} → {}", zone.id, zone.name)
            }
            ZoneControl::LowMidi => {
                let midi = parse_midi_token(trimmed)?;
                zone.set_low_midi(midi);
                format!(
                    "Zone {} MIDI Low → {}",
                    zone.name,
                    format_midi_note(zone.low_midi)
                )
            }
            ZoneControl::HighMidi => {
                let midi = parse_midi_token(trimmed)?;
                zone.set_high_midi(midi);
                format!(
                    "Zone {} MIDI High → {}",
                    zone.name,
                    format_midi_note(zone.high_midi)
                )
            }
            ZoneControl::LowVel => {
                let vel = parse_velocity_token(trimmed)?;
                zone.set_low_vel(vel);
                format!("Zone {} Vel Low → {:.0}%", zone.name, zone.low_vel * 100.0)
            }
            ZoneControl::HighVel => {
                let vel = parse_velocity_token(trimmed)?;
                zone.set_high_vel(vel);
                format!(
                    "Zone {} Vel High → {:.0}%",
                    zone.name,
                    zone.high_vel * 100.0
                )
            }
            ZoneControl::ChokeGroup => {
                let group = parse_choke_token(trimmed)?;
                zone.set_choke_group(group)?;
                format!(
                    "Zone {} Choke → {}",
                    zone.name,
                    match group {
                        None => "None".into(),
                        Some(g) => format!("Group {g}"),
                    }
                )
            }
            ZoneControl::SamplePath => unreachable!(),
        };
        zone.validate()?;
        self.commit(vec![Edit::UpdateRackPatch { id, rack }], label)
    }

    /// Nudge a focused Rack structural zone control via `Alt+h`/`l` or `,`/`.` (`Alt+H`/`L` or `<`/`>`).
    pub(crate) fn nudge_zone_control(
        &mut self,
        id: Id,
        field: ZoneControl,
        direction: i8,
        coarse: bool,
        coalesce: bool,
    ) -> Result<Outcome> {
        if !field.is_adjustable() {
            return Ok(Outcome::default());
        }
        let mut rack = self.clone_rack_patch(id)?;
        if rack.zones.is_empty() {
            return Ok(Outcome::default());
        }
        let idx = rack.clamp_zone_index(self.rack_ui.zone_cursor);
        let zone = &mut rack.zones[idx];
        let label = match field {
            ZoneControl::LowMidi => {
                let step = if coarse { 12 } else { 1 };
                let next =
                    (i16::from(zone.low_midi) + i16::from(direction) * step).clamp(0, 127) as u8;
                zone.set_low_midi(next);
                format!(
                    "Zone {} MIDI Low → {}",
                    zone.name,
                    format_midi_note(zone.low_midi)
                )
            }
            ZoneControl::HighMidi => {
                let step = if coarse { 12 } else { 1 };
                let next =
                    (i16::from(zone.high_midi) + i16::from(direction) * step).clamp(0, 127) as u8;
                zone.set_high_midi(next);
                format!(
                    "Zone {} MIDI High → {}",
                    zone.name,
                    format_midi_note(zone.high_midi)
                )
            }
            ZoneControl::LowVel => {
                let step_pct = if coarse { 5.0 } else { 1.0 };
                let next = (((zone.low_vel * 100.0).round() + f32::from(direction) * step_pct)
                    / 100.0)
                    .clamp(0.0, 1.0);
                zone.set_low_vel(next);
                format!("Zone {} Vel Low → {:.0}%", zone.name, zone.low_vel * 100.0)
            }
            ZoneControl::HighVel => {
                let step_pct = if coarse { 5.0 } else { 1.0 };
                let next = (((zone.high_vel * 100.0).round() + f32::from(direction) * step_pct)
                    / 100.0)
                    .clamp(0.0, 1.0);
                zone.set_high_vel(next);
                format!(
                    "Zone {} Vel High → {:.0}%",
                    zone.name,
                    zone.high_vel * 100.0
                )
            }
            ZoneControl::ChokeGroup => {
                let cur = i16::from(zone.choke_group.unwrap_or(0));
                let slots = i16::from(MAX_CHOKE_GROUPS) + 1;
                let next = (cur + i16::from(direction)).rem_euclid(slots) as u8;
                let group = (next > 0).then_some(next);
                zone.set_choke_group(group)?;
                format!(
                    "Zone {} Choke → {}",
                    zone.name,
                    match group {
                        None => "None".into(),
                        Some(g) => format!("Group {g}"),
                    }
                )
            }
            ZoneControl::Name | ZoneControl::SamplePath => {
                return Ok(Outcome::default());
            }
        };
        self.commit_coalesced(
            vec![Edit::UpdateRackPatch { id, rack }],
            label,
            document::CoalesceKey::Zone {
                instrument: id,
                zone_idx: idx,
                field,
            },
            coalesce,
        )
    }

    /// Execute `:zone ...` Ex commands for Rack zone editing.
    pub(crate) fn zone_command(&mut self, args: &[String]) -> Result<Outcome> {
        ensure!(
            !args.is_empty(),
            "Usage: :zone next|prev|select|add|dup|del|sample|preset|import|name|range|vel|choke|set ..."
        );
        match args[0].to_ascii_lowercase().as_str() {
            "next" => {
                ensure!(args.len() == 1, "Usage: :zone next");
                self.apply(Action::ZoneStep {
                    instrument: None,
                    delta: 1,
                })
            }
            "prev" | "previous" => {
                ensure!(args.len() == 1, "Usage: :zone prev");
                self.apply(Action::ZoneStep {
                    instrument: None,
                    delta: -1,
                })
            }
            "select" | "sel" => {
                ensure!(args.len() >= 2, "Usage: :zone select ID|N|NAME");
                self.apply(Action::ZoneSelect {
                    instrument: None,
                    zone: args[1..].join(" "),
                })
            }
            "add" => {
                // :zone add [NAME] [MIDI] [synth|sample PATH]
                let rest = &args[1..];
                let mut name: Option<String> = None;
                let mut midi: Option<u8> = None;
                let mut sample_path: Option<String> = None;
                let mut i = 0usize;
                while i < rest.len() {
                    let tok = rest[i].as_str();
                    if tok.eq_ignore_ascii_case("synth") {
                        sample_path = None;
                        i += 1;
                    } else if tok.eq_ignore_ascii_case("sample") {
                        ensure!(
                            i + 1 < rest.len(),
                            "Usage: :zone add [NAME] [MIDI] [synth|sample PATH]"
                        );
                        sample_path = Some(rest[i + 1..].join(" "));
                        break;
                    } else if name.is_none() && midi.is_none() {
                        if rest.len() == 1 && parse_midi_token(tok).is_ok() {
                            midi = Some(parse_midi_token(tok)?);
                        } else {
                            name = Some(tok.to_string());
                        }
                        i += 1;
                    } else if midi.is_none() {
                        midi = Some(parse_midi_token(tok)?);
                        i += 1;
                    } else {
                        bail!("Usage: :zone add [NAME] [MIDI] [synth|sample PATH]");
                    }
                }
                self.apply(Action::ZoneAdd {
                    instrument: None,
                    name,
                    midi,
                    sample_path,
                })
            }
            "dup" | "duplicate" => {
                let name = if args.len() > 1 {
                    Some(args[1..].join(" "))
                } else {
                    None
                };
                self.apply(Action::ZoneDuplicate {
                    instrument: None,
                    name,
                })
            }
            "del" | "delete" | "remove" => {
                ensure!(args.len() == 1, "Usage: :zone del");
                self.apply(Action::ZoneRemove { instrument: None })
            }
            "sample" => {
                ensure!(args.len() >= 2, "Usage: :zone sample PATH");
                self.apply(Action::ZoneSetSample {
                    instrument: None,
                    path: args[1..].join(" "),
                })
            }
            "preset" => {
                ensure!(args.len() >= 2, "Usage: :zone preset NAME|PATH");
                self.apply(Action::ZoneLoadPreset {
                    instrument: None,
                    preset: args[1..].join(" "),
                })
            }
            "import" => {
                ensure!(args.len() >= 2, "Usage: :zone import DIR");
                self.apply(Action::ZoneImportFolder {
                    instrument: None,
                    folder: args[1..].join(" "),
                })
            }
            "name" | "rename" => {
                ensure!(args.len() >= 2, "Usage: :zone name NAME");
                self.apply(Action::ZoneRename {
                    instrument: None,
                    name: args[1..].join(" "),
                })
            }
            "range" => {
                ensure!(
                    (3..=4).contains(&args.len()),
                    "Usage: :zone range LOW HIGH [ROOT]"
                );
                let low_midi = parse_midi_token(&args[1])?;
                let high_midi = parse_midi_token(&args[2])?;
                let root_midi = args.get(3).map(|s| parse_midi_token(s)).transpose()?;
                self.apply(Action::ZoneSetRange {
                    instrument: None,
                    low_midi,
                    high_midi,
                    root_midi,
                })
            }
            "vel" | "velocity" => {
                ensure!(args.len() == 3, "Usage: :zone vel LOW HIGH");
                let low_vel = parse_velocity_token(&args[1])?;
                let high_vel = parse_velocity_token(&args[2])?;
                self.apply(Action::ZoneSetVelocity {
                    instrument: None,
                    low_vel,
                    high_vel,
                })
            }
            "choke" => {
                ensure!(args.len() == 2, "Usage: :zone choke none|1..8");
                let choke_group = parse_choke_token(&args[1])?;
                self.apply(Action::ZoneSetChoke {
                    instrument: None,
                    choke_group,
                })
            }
            "set" => {
                ensure!(args.len() >= 3, "Usage: :zone set FIELD VALUE");
                self.zone_set_field(&args[1], &args[2..].join(" "))
            }
            other => bail!(
                "Unknown :zone subcommand '{other}' · use next|prev|select|add|dup|del|sample|preset|import|name|range|vel|choke|set"
            ),
        }
    }

    pub(crate) fn zone_set_field(&mut self, field_name: &str, value: &str) -> Result<Outcome> {
        let key = field_name
            .trim()
            .to_ascii_lowercase()
            .replace([' ', '-'], "_");
        match key.as_str() {
            "name" => {
                return self.apply(Action::ZoneRename {
                    instrument: None,
                    name: value.to_string(),
                });
            }
            "sample" | "sample_path" | "path" => {
                return self.apply(Action::ZoneSetSample {
                    instrument: None,
                    path: value.to_string(),
                });
            }
            "choke" | "choke_group" => {
                return self.apply(Action::ZoneSetChoke {
                    instrument: None,
                    choke_group: parse_choke_token(value)?,
                });
            }
            _ => {}
        }

        let id = self.resolve_rack_instrument_id(None)?;
        let (idx, zone_id, rack_clone) = {
            let rack = &self.instrument(id).context("Instrument missing")?.rack;
            ensure!(!rack.zones.is_empty(), "Rack has no zones");
            let idx = rack.clamp_zone_index(self.rack_ui.zone_cursor);
            (idx, rack.zones[idx].id, rack.clone())
        };

        // Route continuous/automatable zone/module/cable parameters through the unified ParamAddress path!
        if !matches!(
            key.as_str(),
            "low_midi"
                | "low"
                | "midi_low"
                | "high_midi"
                | "high"
                | "midi_high"
                | "root_midi"
                | "root"
                | "midi_root"
                | "low_vel"
                | "vel_low"
                | "high_vel"
                | "vel_high"
        ) {
            let scoped = format!("z{zone_id}.{}", field_name.trim());
            if let Some(ip) = InstrumentParam::parse_for_rack(&scoped, &rack_clone, false) {
                let address = ParamAddress::from_instrument(id, ip);
                let native = self.parse_param_native(address, value)?;
                return self.apply(Action::SetParam {
                    address,
                    value: native,
                });
            }
        }

        let mut rack = rack_clone;
        let zone = &mut rack.zones[idx];
        let label = match key.as_str() {
            "low_midi" | "low" | "midi_low" => {
                let midi = parse_midi_token(value)?;
                zone.set_low_midi(midi);
                format!(
                    "Zone {} MIDI Low → {}",
                    zone.name,
                    format_midi_note(zone.low_midi)
                )
            }
            "high_midi" | "high" | "midi_high" => {
                let midi = parse_midi_token(value)?;
                zone.set_high_midi(midi);
                format!(
                    "Zone {} MIDI High → {}",
                    zone.name,
                    format_midi_note(zone.high_midi)
                )
            }
            "root_midi" | "root" | "midi_root" => {
                let midi = parse_midi_token(value)?;
                zone.set_root_midi(midi);
                format!(
                    "Zone {} Root MIDI → {}",
                    zone.name,
                    format_midi_note(zone.root_midi)
                )
            }
            "low_vel" | "vel_low" => {
                let v = parse_velocity_token(value)?;
                zone.set_low_vel(v);
                format!("Zone {} Vel Low → {:.0}%", zone.name, zone.low_vel * 100.0)
            }
            "high_vel" | "vel_high" => {
                let v = parse_velocity_token(value)?;
                zone.set_high_vel(v);
                format!(
                    "Zone {} Vel High → {:.0}%",
                    zone.name,
                    zone.high_vel * 100.0
                )
            }
            _ => bail!("Unknown zone field '{field_name}'"),
        };
        self.commit(vec![Edit::UpdateRackPatch { id, rack }], label)
    }

    /// Audition the currently focused Rack zone at its root MIDI note.
    pub(crate) fn audition_focused_zone(&mut self, id: Id) -> Result<Outcome> {
        let Some((zone_name, midi, zone_id, samplers, has_synth)) =
            self.instrument(id).and_then(|inst| {
                let idx = inst.rack.clamp_zone_index(self.rack_ui.zone_cursor);
                let z = inst.rack.zones.get(idx)?;
                let midi = z.root_midi.clamp(z.low_midi, z.high_midi);
                let samplers: Vec<u16> = z
                    .modules
                    .iter()
                    .filter_map(|m| matches!(m.kind, ModuleKind::Sampler(_)).then_some(m.id))
                    .collect();
                let has_synth = z
                    .modules
                    .iter()
                    .any(|m| matches!(m.kind, ModuleKind::Vco(_) | ModuleKind::Noise(_)));
                Some((z.name.clone(), midi, z.id, samplers, has_synth))
            })
        else {
            return Ok(Outcome::default());
        };
        if !samplers.is_empty() && !has_synth {
            let any_loaded = samplers.iter().any(|&mid| {
                self.assets
                    .samples
                    .contains_key(&module_sample_key(id, zone_id, mid))
            });
            ensure!(
                any_loaded,
                "Zone '{}' sample is unavailable · press B to assign a WAV",
                zone_name
            );
        }
        self.status = format!("Audition zone {zone_name} ({})", format_midi_note(midi));
        Ok(Outcome {
            audition_instrument: Some((id, midi, 0.85)),
            ..Outcome::default()
        })
    }
}
