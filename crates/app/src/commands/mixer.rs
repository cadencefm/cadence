//! Mixer, routing, effects, modulation, macros, delete, and export Ex commands.
use super::{parse_ms, require_args, MixerVerb};
use crate::*;

impl App {
    pub(super) fn execute_mixer_verb(
        &mut self,
        verb: MixerVerb,
        args: &[String],
    ) -> Result<Outcome> {
        match verb {
            MixerVerb::Mod => self.mod_command(args),
            MixerVerb::Macro => self.execute_macro_command(args),
            MixerVerb::Duck => {
                if args.is_empty() {
                    return self.begin_workflow(WorkflowKind::Duck);
                }
                require_args(
                    args,
                    5,
                    ":duck [NAME|ID SOURCE_PART TARGET_CHANNEL DEPTH RECOVERY_MS]",
                )?;
                let source_part = self.part_id(&args[1])?;
                let target = self.channel_ref(&args[2])?;
                let depth = parse_amount(&args[3])?;
                let recovery_ms = parse_ms(&args[4])?;
                let by_id = args[0].trim().trim_start_matches('#').parse::<Id>().ok();
                let existing = self
                    .doc
                    .project()
                    .ducks
                    .iter()
                    .find(|d| by_id == Some(d.id) || d.name.eq_ignore_ascii_case(args[0].trim()))
                    .or_else(|| {
                        self.doc
                            .project()
                            .ducks
                            .iter()
                            .find(|d| d.source_part == source_part && d.target == target)
                    })
                    .cloned();
                if let Some(prev) = existing {
                    let name = if by_id == Some(prev.id) {
                        prev.name
                    } else {
                        args[0].clone()
                    };
                    let label = format!("Updated duck '{name}'");
                    return self.commit(
                        vec![Edit::PutDuck {
                            duck: Duck {
                                id: prev.id,
                                name,
                                source_part,
                                target,
                                depth,
                                recovery_ms,
                            },
                        }],
                        label,
                    );
                }
                self.apply(Action::CreateDuck {
                    name: args[0].clone(),
                    source_part,
                    target,
                    depth,
                    recovery_ms,
                })
            }
            MixerVerb::Param => {
                ensure!(args.len() >= 2, "Usage: :param NAME VALUE");
                self.set_parameter_text(&args[0], &args[1..].join(" "))
            }
            MixerVerb::Channel => self.channel_command(args),
            MixerVerb::Fx => self.fx_command(args),
            MixerVerb::Route => {
                require_args(args, 2, ":route CHANNEL MASTER_OR_GROUP")?;
                self.apply(Action::SetChannelOutput {
                    channel: self.channel_id(&args[0])?,
                    output: self.channel_ref(&args[1])?,
                })
            }
            MixerVerb::RouteInstrument => {
                require_args(args, 2, ":route-instrument INSTRUMENT AUDIO_CHANNEL")?;
                self.apply(Action::RouteInstrument {
                    instrument: self.instrument_id(&args[0])?,
                    channel: self.channel_id(&args[1])?,
                })
            }
            MixerVerb::Send => {
                if args.is_empty() {
                    return self.begin_workflow(WorkflowKind::Send);
                }
                require_args(args, 3, ":send [CHANNEL RETURN AMOUNT|off]")?;
                self.apply(Action::SetChannelSend {
                    channel: self.channel_id(&args[0])?,
                    target: self.channel_id(&args[1])?,
                    amount: if args[2].eq_ignore_ascii_case("off") {
                        None
                    } else {
                        Some(parse_amount(&args[2])?)
                    },
                })
            }
            MixerVerb::Clips => {
                ensure!(
                    args == ["clear"] || args == ["clear", "all"],
                    "Usage: :clips clear [all]"
                );
                self.apply(Action::ClearMeters {
                    channel: if args.len() == 2 {
                        None
                    } else {
                        Some(self.selected_channel)
                    },
                })
            }
            MixerVerb::Rename => {
                ensure!(
                    args.len() >= 3,
                    "Usage: :rename instrument|channel|fx|pattern|part|lane ID_OR_NAME NEW_NAME"
                );
                let name = args[2..].join(" ");
                match args[0].to_ascii_lowercase().as_str() {
                    "instrument" | "inst" => {
                        let instrument = self.instrument_id(&args[1])?;
                        self.apply(Action::RenameInstrument { instrument, name })
                    }
                    "channel" | "ch" => {
                        let channel = self.channel_id(&args[1])?;
                        self.apply(Action::RenameChannel { channel, name })
                    }
                    "fx" | "effect" => {
                        let device = self.effect_id(&args[1])?;
                        self.apply(Action::RenameEffect { device, name })
                    }
                    "pattern" | "pat" => {
                        let pattern = self.pattern_id(&args[1])?;
                        self.apply(Action::RenamePattern { pattern, name })
                    }
                    "part" => {
                        let part = self.part_id(&args[1])?;
                        self.apply(Action::RenamePart { part, name })
                    }
                    "lane" => {
                        let lane = self.lane_id(&args[1])?;
                        self.apply(Action::RenameLane { lane, name })
                    }
                    _ => bail!(
                        "Usage: :rename instrument|channel|fx|pattern|part|lane ID_OR_NAME NEW_NAME"
                    ),
                }
            }
            MixerVerb::Delete => self.delete_command(args),
            MixerVerb::Mixer => {
                self.show_workspace(Workspace::Mixer);
                self.follow_deck();
                Ok(Outcome::default())
            }
            MixerVerb::Wav => {
                ensure!(args.len() <= 2, "Usage: :wav [path] [bars]");
                self.apply(Action::Export {
                    path: args
                        .first()
                        .map_or_else(|| PathBuf::from("cadence-take.wav"), PathBuf::from),
                    bars: args.get(1).map(|s| s.parse()).transpose()?,
                })
            }
            MixerVerb::Bundle => {
                if let Some(dir) = args.first() {
                    self.save_portable_bundle_to(&PathBuf::from(dir))?;
                } else {
                    self.open_file_hub(FileHubTab::Save);
                }
                Ok(Outcome::default())
            }
            MixerVerb::Stems => {
                let dir = args
                    .first()
                    .context("Usage: :stems DIR [BARS] [SAMPLE_RATE]")?;
                let bars = args
                    .get(1)
                    .map(|s| s.parse::<u32>().context("Bars must be a positive integer"))
                    .transpose()?;
                let sr = args
                    .get(2)
                    .map(|s| {
                        s.parse::<u32>()
                            .context("Sample rate must be e.g. 44100, 48000, or 96000")
                    })
                    .transpose()?
                    .unwrap_or(48_000);
                let scope = if bars.is_some() {
                    ExportScopeKind::CustomBars
                } else {
                    ExportScopeKind::FullSong
                };
                let export = self.build_export_request(
                    PathBuf::from(dir),
                    ExportMode::MasterAndStems,
                    scope,
                    bars.unwrap_or(self.doc.project().bars as u32),
                    sr,
                )?;
                self.status = format!("Rendering Master + Stems → {}…", export.path.display());
                Ok(Outcome {
                    export: Some(export),
                    ..Outcome::default()
                })
            }
        }
    }

    pub(crate) fn mod_command(&mut self, args: &[String]) -> Result<Outcome> {
        if let Some(sub) = args.first() {
            let sub_low = sub.to_ascii_lowercase();
            let looks_like_global_mod = args.len() >= 4
                && matches!(
                    args[1].to_ascii_lowercase().as_str(),
                    "instrument" | "zone" | "effect" | "channel" | "send" | "macro"
                );
            if !looks_like_global_mod
                && matches!(sub_low.as_str(), "add" | "del" | "rm" | "remove" | "rename")
            {
                return self.rack_mod_subcommand(args);
            }
        }
        if args.len() == 3 {
            if let Some((instrument, parameter)) = self
                .parse_dotted_zone_param(&args[1], true)
                .and_then(|addr| addr.as_instrument())
            {
                let pattern = self.pattern_id(&args[2])?;
                return self.apply(Action::CreateModulation {
                    name: args[0].clone(),
                    target: ModulationTarget::Instrument {
                        instrument,
                        parameter,
                    },
                    pattern,
                });
            }
        }
        if args.len() == 4 && args[1].eq_ignore_ascii_case("macro") {
            let index = Self::parse_macro_slot(&args[2])?;
            let pattern = self.pattern_id(&args[3])?;
            return self.apply(Action::CreateModulation {
                name: args[0].clone(),
                target: ModulationTarget::Macro { index },
                pattern,
            });
        }
        if args.len() == 6
            && args[1].eq_ignore_ascii_case("channel")
            && args[3].eq_ignore_ascii_case("send")
        {
            let channel = self.channel_ref(&args[2])?;
            let target = self.channel_id(&args[4])?;
            let pattern = self.pattern_id(&args[5])?;
            return self.apply(Action::CreateModulation {
                name: args[0].clone(),
                target: ModulationTarget::Send { channel, target },
                pattern,
            });
        }
        ensure!(
            args.len() == 5,
            "Usage: :mod add|del|rename ... | :mod NAME instrument|zone|effect|channel|send TARGET PARAMETER VALUE_PATTERN | :mod NAME KIT.ZONE.PARAM VALUE_PATTERN | :mod NAME macro 1..8 VALUE_PATTERN"
        );
        let pattern = self.pattern_id(&args[4])?;
        let kind_lower = args[1].trim().to_ascii_lowercase();
        ensure!(
            matches!(
                kind_lower.as_str(),
                "instrument"
                    | "inst"
                    | "rack"
                    | "zone"
                    | "module"
                    | "mod"
                    | "cable"
                    | "effect"
                    | "fx"
                    | "channel"
                    | "ch"
                    | "send"
            ),
            "Modulation target is instrument, zone, effect, channel, send, or macro"
        );
        let address = self.parse_param_address(&args[1], &args[2], &args[3])?;
        let target = match address {
            ParamAddress::Send { channel, target } => ModulationTarget::Send { channel, target },
            other => self
                .param_address_to_mod_target(other)
                .context("Parameter is not automatable")?,
        };
        self.apply(Action::CreateModulation {
            name: args[0].clone(),
            target,
            pattern,
        })
    }

    pub(crate) fn channel_command(&mut self, args: &[String]) -> Result<Outcome> {
        if args.is_empty() {
            return self.begin_workflow(WorkflowKind::Channel);
        }
        if args.len() == 1 {
            let c = self.channel_ref(&args[0])?;
            return self.apply(Action::SelectChannel { channel: c });
        }
        if args.len() >= 3 && args[0].eq_ignore_ascii_case("rename") {
            let channel = self.channel_id(&args[1])?;
            let name = args[2..].join(" ");
            return self.apply(Action::RenameChannel { channel, name });
        }
        ensure!(
            args.len() == 2,
            "Usage: :channel | :channel NAME | :channel audio|group|return NAME | :channel rename CHANNEL NEW_NAME"
        );
        let kind =
            ChannelKind::parse(&args[0]).context("Channel kind is audio, group, or return")?;
        self.apply(Action::CreateChannel {
            name: args[1].clone(),
            kind,
        })
    }

    pub(crate) fn fx_command(&mut self, args: &[String]) -> Result<Outcome> {
        if args.is_empty() {
            return self.begin_workflow(WorkflowKind::Effect);
        }
        match args[0].as_str() {
            "add" => {
                ensure!(
                    (2..=3).contains(&args.len()),
                    "Usage: :fx add <{}> [CHANNEL]",
                    EffectDevice::kind_names_hint()
                );
                let channel = args
                    .get(2)
                    .map(|s| self.channel_ref(s))
                    .transpose()?
                    .unwrap_or(self.selected_channel);
                self.apply(Action::AddEffect {
                    channel,
                    kind: args[1].clone(),
                })
            }
            kind if EffectDevice::is_kind_name(kind) => {
                ensure!(
                    args.len() <= 2,
                    "Usage: :fx <{}> [CHANNEL]",
                    EffectDevice::kind_names_hint()
                );
                let channel = args
                    .get(1)
                    .map(|s| self.channel_ref(s))
                    .transpose()?
                    .unwrap_or(self.selected_channel);
                self.apply(Action::AddEffect {
                    channel,
                    kind: args[0].clone(),
                })
            }
            "rename" => {
                ensure!(args.len() >= 3, "Usage: :fx rename ID_OR_NAME NEW_NAME");
                let device = self.effect_id(&args[1])?;
                let name = args[2..].join(" ");
                self.apply(Action::RenameEffect { device, name })
            }
            "bypass" => {
                ensure!(args.len() == 2, "Usage: :fx bypass ID_OR_NAME");
                let id = self.effect_id(&args[1])?;
                let bypassed = !self.effect(id).context("Effect missing")?.1.bypassed;
                self.apply(Action::SetEffectBypass {
                    device: id,
                    bypassed,
                })
            }
            "delete" => {
                ensure!(args.len() == 2, "Usage: :fx delete ID_OR_NAME");
                let device = self.effect_id(&args[1])?;
                self.apply(Action::DeleteEffect { device })
            }
            "move" => {
                ensure!(
                    args.len() == 4,
                    "Usage: :fx move ID_OR_NAME CHANNEL ONE_BASED_POSITION"
                );
                let device = self.effect_id(&args[1])?;
                let channel = self.channel_ref(&args[2])?;
                let position = args[3]
                    .parse::<usize>()?
                    .checked_sub(1)
                    .context("Positions start at 1")?;
                self.apply(Action::MoveEffect {
                    device,
                    channel,
                    position,
                })
            }
            _ => {
                ensure!(args.len() == 1, "Usage: :fx ID_OR_NAME");
                let id = self.effect_id(&args[0])?;
                let (owner, _) = self.effect(id).context("Effect missing")?;
                self.push_back();
                self.selected_channel = owner;
                self.show_detail(Detail::Effect(id));
                self.control_cursor = 0;
                Ok(Outcome::default())
            }
        }
    }

    pub(crate) fn delete_command(&mut self, args: &[String]) -> Result<Outcome> {
        ensure!(
            args.len() == 2,
            "Usage: :delete pattern|part|instrument|lane|placement|channel|fx|section|mod|duck|macro ID_OR_NAME"
        );
        let kind = args[0].to_ascii_lowercase();
        match kind.as_str() {
            "fx" | "effect" => {
                let device = self.effect_id(&args[1])?;
                return self.apply(Action::DeleteEffect { device });
            }
            "macro" => {
                let index = Self::parse_macro_slot(&args[1])?;
                return self.apply(Action::ClearMacro { index });
            }
            "channel" => {
                let channel = self.channel_id(&args[1])?;
                return self.apply(Action::DeleteChannel { channel });
            }
            _ => {}
        }
        let edit = match kind.as_str() {
            "pattern" => Edit::RemovePattern {
                id: self.pattern_id(&args[1])?,
            },
            "part" => Edit::RemovePart {
                id: self.part_id(&args[1])?,
            },
            "instrument" => Edit::RemoveInstrument {
                id: self.instrument_id(&args[1])?,
            },
            "lane" => Edit::RemoveLane {
                id: self.lane_id(&args[1])?,
            },
            "placement" => Edit::RemovePlacement {
                id: args[1].parse()?,
            },
            "section" => Edit::RemoveSection {
                id: lookup(
                    &args[1],
                    self.doc
                        .project()
                        .sections
                        .iter()
                        .map(|s| (s.id, s.name.as_str())),
                    "section",
                )?,
            },
            "mod" => Edit::RemoveModulation {
                id: lookup(
                    &args[1],
                    self.doc
                        .project()
                        .modulations
                        .iter()
                        .map(|s| (s.id, s.name.as_str())),
                    "modulation",
                )?,
            },
            "duck" => Edit::RemoveDuck {
                id: lookup(
                    &args[1],
                    self.doc
                        .project()
                        .ducks
                        .iter()
                        .map(|s| (s.id, s.name.as_str())),
                    "duck",
                )?,
            },
            _ => bail!("Unknown object kind"),
        };
        self.apply(Action::Edit { edit })
    }
}

#[cfg(test)]
mod tests {
    use crate::{Action, App};
    use cadence_core::material::Project;

    #[test]
    fn rename_instrument_action_commands_and_undo_mirror_rename_channel() {
        let mut app = App::new_in_memory(Project::demo(), "rename-inst.json".into()).unwrap();
        let inst0 = app.doc.project().instruments[0].id;
        let orig_name = app.doc.project().instruments[0].name.clone();
        let inst1_name = app.doc.project().instruments[1].name.clone();

        // 1. Direct Action::RenameInstrument + Undo / Redo.
        app.apply(Action::RenameInstrument {
            instrument: inst0,
            name: "  Acid Lead  ".into(),
        })
        .unwrap();
        assert_eq!(app.instrument(inst0).unwrap().name, "Acid Lead");

        app.apply(Action::Undo).unwrap();
        assert_eq!(app.instrument(inst0).unwrap().name, orig_name);

        app.apply(Action::Redo).unwrap();
        assert_eq!(app.instrument(inst0).unwrap().name, "Acid Lead");

        // 2. `:instrument rename` Ex command (multi-word name) + Undo.
        app.execute_command(&format!("instrument rename {inst0} Warm Pulse"))
            .unwrap();
        assert_eq!(app.instrument(inst0).unwrap().name, "Warm Pulse");
        app.apply(Action::Undo).unwrap();
        assert_eq!(app.instrument(inst0).unwrap().name, "Acid Lead");

        // 3. `:rename instrument` and `:rename channel` Ex command peers.
        app.execute_command(&format!("rename instrument {inst0} Brass Lead"))
            .unwrap();
        assert_eq!(app.instrument(inst0).unwrap().name, "Brass Lead");

        let ch0 = app.doc.project().mixer.channels[0].id;
        app.execute_command(&format!("rename channel {ch0} Lead Bus"))
            .unwrap();
        assert_eq!(app.channel(ch0).unwrap().name, "Lead Bus");

        // 4. Reject empty and colliding instrument names (case-insensitive).
        assert!(app
            .apply(Action::RenameInstrument {
                instrument: inst0,
                name: "   ".into(),
            })
            .is_err());
        assert!(app
            .apply(Action::RenameInstrument {
                instrument: inst0,
                name: inst1_name.to_ascii_uppercase(),
            })
            .is_err());

        // 5. `:mod` fallback error lists `zone`.
        let pat_id = app.doc.project().patterns[6].id;
        let err = app
            .execute_command(&format!("mod badmod bogus 1 cutoff {pat_id}"))
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            "Modulation target is instrument, zone, effect, channel, send, or macro"
        );
    }
}
