//! Material, pattern algebra, part, and audition Ex commands.
use super::instruments::parse_single_sound_token;
use super::{require_args, MaterialVerb};
use crate::*;

impl App {
    pub(super) fn execute_material_verb(
        &mut self,
        verb: MaterialVerb,
        args: &[String],
    ) -> Result<Outcome> {
        match verb {
            MaterialVerb::Pattern => self.pattern_command(args),
            MaterialVerb::Mini => self.mini_command(args),
            MaterialVerb::Open => {
                require_args(args, 1, ":open PATTERN")?;
                let id = self.pattern_id(&args[0])?;
                self.apply(Action::SelectPattern { pattern: id })
            }
            MaterialVerb::Part => self.part_command(args),
            MaterialVerb::Derive => self.derive_command(args),
            MaterialVerb::Fork => {
                ensure!((1..=2).contains(&args.len()), "Usage: :fork NAME [SOURCE]");
                let source = args
                    .get(1)
                    .map(|s| self.pattern_id(s))
                    .transpose()?
                    .or(self.selected_pattern)
                    .context("Select a source")?;
                self.apply(Action::Fork {
                    source,
                    name: args[0].clone(),
                })
            }
            MaterialVerb::Freeze => {
                ensure!(
                    (3..=4).contains(&args.len()),
                    "Usage: :freeze NAME START_STEP LENGTH [SOURCE]"
                );
                let source = args
                    .get(3)
                    .map(|s| self.pattern_id(s))
                    .transpose()?
                    .or(self.selected_pattern)
                    .context("Select a source")?;
                self.apply(Action::Freeze {
                    source,
                    name: args[0].clone(),
                    start: args[1].parse()?,
                    length: args[2].parse()?,
                })
            }
            MaterialVerb::Len => {
                require_args(args, 1, ":len STEPS")?;
                self.apply(Action::Edit {
                    edit: Edit::ResizePattern {
                        pattern: self.selected_pattern.context("Select a literal source")?,
                        length: args[0].parse()?,
                    },
                })
            }
            MaterialVerb::Note => {
                ensure!(
                    (2..=4).contains(&args.len()),
                    "Usage: :note STEP DEGREE[,DEGREE] [VELOCITY] [GATE]"
                );
                let step = args[0]
                    .parse::<usize>()?
                    .checked_sub(1)
                    .context("Steps start at 1")?;
                let mut value = self.step_from_text(&args[1])?;
                if let Some(v) = args.get(2) {
                    value.velocity = parse_amount(v)?;
                    for c in &mut value.chord {
                        c.velocity = value.velocity;
                    }
                }
                if let Some(g) = args.get(3) {
                    value.gate = g.parse()?;
                    for c in &mut value.chord {
                        c.gate = value.gate;
                    }
                }
                let old = self.cursor;
                self.cursor = step;
                let edit = self.edits_for_current_note(value);
                self.cursor = old;
                self.apply(Action::Edit { edit: edit? })
            }
            MaterialVerb::Value => {
                // `:value STEP VALUE` edits the selected source; a leading
                // pattern name edits that source from anywhere, even mid-set.
                ensure!(
                    (2..=3).contains(&args.len()),
                    "Usage: :value [PATTERN] STEP VALUE|off"
                );
                let (pattern, args) = if args.len() == 3 {
                    (self.pattern_id(&args[0])?, &args[1..])
                } else {
                    (self.selected_pattern.context("Select value source")?, args)
                };
                let step = args[0]
                    .parse::<u32>()?
                    .checked_sub(1)
                    .context("Steps start at 1")?;
                self.apply(Action::Edit {
                    edit: Edit::SetValue {
                        pattern,
                        step,
                        value: if args[1] == "off" {
                            None
                        } else {
                            Some(args[1].parse()?)
                        },
                    },
                })
            }
            MaterialVerb::ValueMode => {
                ensure!(
                    (1..=2).contains(&args.len()),
                    "Usage: :value-mode [PATTERN] hold|ramp|toggle"
                );
                let (pattern, mode_arg) = if args.len() == 2 {
                    (self.pattern_id(&args[0])?, args[1].as_str())
                } else {
                    (
                        self.selected_pattern.context("Select a value pattern")?,
                        args[0].as_str(),
                    )
                };
                let mode = if mode_arg.eq_ignore_ascii_case("toggle") {
                    None
                } else {
                    Some(ValueMode::parse(mode_arg).context("Value modes: hold, ramp, toggle")?)
                };
                self.set_value_mode(pattern, mode)
            }
            MaterialVerb::Local => {
                ensure!(args.len() <= 1, "Usage: :local [PLACEMENT_ID]");
                let id = args
                    .first()
                    .map(|s| s.parse())
                    .transpose()?
                    .or(self.selected_placement)
                    .context("Select a placement")?;
                ensure!(self.placement(id).is_some(), "Placement missing");
                self.push_back();
                self.set_placement_context(id)?;
                self.show_workspace(Workspace::Pattern);
                self.scope = EditScope::Placement(id);
                self.cursor = 0;
                self.status = "Placement-relative exceptions · shared sources remain linked".into();
                Ok(Outcome::default())
            }
            MaterialVerb::Source => {
                let id = self.selected_pattern.context("Select material")?;
                self.select_pattern(id)?;
                Ok(Outcome::default())
            }
            MaterialVerb::Inherit => {
                let EditScope::Placement(id) = self.scope else {
                    bail!("Choose a placement-local edit scope first")
                };
                self.apply(Action::Edit {
                    edit: Edit::SetException {
                        placement: id,
                        step: self.cursor as u32,
                        value: None,
                    },
                })
            }
            MaterialVerb::Link => {
                require_args(args, 3, ":link PART PARAMETER VALUE_PATTERN")?;
                let id = self.part_id(&args[0])?;
                let mut part = self.part(id).context("Part missing")?.clone();
                let instrument = self
                    .instrument(part.instrument)
                    .context("Instrument missing")?;
                let target = instrument_parameter(&args[1], &instrument.rack)
                    .or_else(|| {
                        self.parse_dotted_zone_param(&args[1], true)
                            .and_then(|addr| addr.as_instrument())
                            .filter(|(inst_id, _)| *inst_id == part.instrument)
                            .map(|(_, p)| p)
                    })
                    .context("Unknown per-note parameter")?;
                let pattern = self.pattern_id(&args[2])?;
                part.parameter_links.retain(|l| l.target != target);
                part.parameter_links
                    .push(material::ParameterLink { target, pattern });
                self.apply(Action::Edit {
                    edit: Edit::PutPart { part },
                })
            }
            MaterialVerb::Unlink => {
                require_args(args, 2, ":unlink PART PARAMETER")?;
                let mut part = self
                    .part(self.part_id(&args[0])?)
                    .context("Part missing")?
                    .clone();
                let instrument = self
                    .instrument(part.instrument)
                    .context("Instrument missing")?;
                let target = instrument_parameter(&args[1], &instrument.rack)
                    .or_else(|| {
                        self.parse_dotted_zone_param(&args[1], true)
                            .and_then(|addr| addr.as_instrument())
                            .filter(|(inst_id, _)| *inst_id == part.instrument)
                            .map(|(_, p)| p)
                    })
                    .context("Unknown parameter")?;
                part.parameter_links.retain(|link| link.target != target);
                self.apply(Action::Edit {
                    edit: Edit::PutPart { part },
                })
            }
            MaterialVerb::Audition => match args.first().map(|s| s.to_ascii_lowercase()).as_deref()
            {
                Some("stop" | "off") => {
                    ensure!(
                        args.len() == 1,
                        "Usage: :audition [PATTERN|stop|off] [STEPS]"
                    );
                    self.apply(Action::StopAudition)
                }
                None => self.apply(Action::Audition {
                    pattern: self.selected_pattern.context("Select a note pattern")?,
                    length: None,
                }),
                Some(_) => {
                    ensure!(
                        args.len() <= 2,
                        "Usage: :audition [PATTERN|stop|off] [STEPS]"
                    );
                    if args.len() == 1 {
                        if let Ok(steps) = args[0].parse::<usize>() {
                            if let Some(pattern) = self.selected_pattern {
                                return self.apply(Action::Audition {
                                    pattern,
                                    length: Some(steps),
                                });
                            }
                        }
                        let pattern = self.pattern_id(&args[0])?;
                        self.apply(Action::Audition {
                            pattern,
                            length: None,
                        })
                    } else {
                        let pattern = self.pattern_id(&args[0])?;
                        let length = args[1]
                            .parse::<usize>()
                            .context("Audition length must be a step count")?;
                        self.apply(Action::Audition {
                            pattern,
                            length: Some(length),
                        })
                    }
                }
            },
        }
    }

    pub(crate) fn pattern_command(&mut self, args: &[String]) -> Result<Outcome> {
        ensure!(
            args.len() >= 2,
            "Usage: :pattern notes|rhythm|degrees|values|seed|euclid|mini NAME ..."
        );
        let name = args[1].clone();
        let kind = match args[0].as_str() {
            "notes" => {
                ensure!(args.len() >= 3, "Add degree tokens or . rests");
                PatternKind::Notes {
                    steps: args[2..]
                        .iter()
                        .map(|s| self.step_from_text(s))
                        .collect::<Result<_>>()?,
                }
            }
            "rhythm" => {
                let mut steps = Vec::new();
                for token in &args[2..] {
                    for c in token.chars() {
                        steps.push(match c {
                            'x' | 'X' | '1' | '+' => true,
                            '.' | '~' | '-' | '0' => false,
                            _ => bail!("Rhythm uses x and ."),
                        });
                    }
                }
                ensure!(!steps.is_empty(), "Enter rhythm steps");
                PatternKind::Rhythm { steps }
            }
            "degrees" | "values" => {
                ensure!(
                    args.len() >= 4,
                    "Usage: :pattern degrees|values NAME PERIOD [hold|ramp] STEP:VALUE..."
                );
                let unit = if args[0] == "degrees" {
                    ValueUnit::Degrees
                } else {
                    ValueUnit::Normalized
                };
                let period = args[2].parse()?;
                let (mode, start) = match ValueMode::parse(&args[3]) {
                    Some(parsed) => (parsed, 4),
                    None => (ValueMode::Hold, 3),
                };
                let mut points = args[start..]
                    .iter()
                    .map(|s| {
                        let (a, b) = s
                            .split_once(':')
                            .context("Points use zero-based STEP:VALUE")?;
                        Ok(ValuePoint {
                            step: a.parse()?,
                            value: b.parse()?,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                points.sort_by_key(|p| p.step);
                PatternKind::Values {
                    unit,
                    period,
                    mode,
                    points,
                }
            }
            "seed" | "seed-degrees" => {
                ensure!(
                    args.len() == 6,
                    "Usage: :pattern seed NAME PERIOD SEED MIN MAX"
                );
                PatternKind::SeededValues {
                    unit: if args[0] == "seed-degrees" {
                        ValueUnit::Degrees
                    } else {
                        ValueUnit::Normalized
                    },
                    period: args[2].parse()?,
                    seed: args[3].parse()?,
                    min: args[4].parse()?,
                    max: args[5].parse()?,
                }
            }
            "euclid" => {
                ensure!(
                    (4..=5).contains(&args.len()),
                    "Usage: :pattern euclid NAME PULSES STEPS [ROTATION]"
                );
                PatternKind::Euclidean {
                    pulses: args[2].parse()?,
                    steps: args[3].parse()?,
                    rotation: args.get(4).map(|s| s.parse()).transpose()?.unwrap_or(0),
                }
            }
            "mini" => {
                ensure!(args.len() >= 4, "Usage: :pattern mini NAME STEPS <CODE...>");
                let steps: u16 = args[2].parse()?;
                let code = args[3..].join(" ");
                cadence_core::mini::parse_mini(&code)?;
                PatternKind::Mini { code, steps }
            }
            _ => bail!("Pattern kinds: notes, rhythm, degrees, values, seed, euclid, mini"),
        };
        self.apply(Action::CreatePattern { name, kind })
    }

    pub(crate) fn mini_command(&mut self, args: &[String]) -> Result<Outcome> {
        ensure!(!args.is_empty(), "Usage: :mini [NAME [STEPS]] <CODE...>");
        // Fast path: if a Mini pattern is selected and the first arg isn't a new name+steps pair,
        // allow `:mini c4 [e4 g4] ~` or `:mini NAME [STEPS] <CODE...>`.
        if let Some(id) = self.selected_pattern {
            if let Some(existing) = self.pattern(id).cloned() {
                if let PatternKind::Mini { steps, .. } = existing.kind {
                    // Check if args[0] explicitly targets another pattern or if user is live-editing code
                    let looks_like_named = args.len() >= 2
                        && (args[1].parse::<u16>().is_ok()
                            || self
                                .doc
                                .project()
                                .patterns
                                .iter()
                                .any(|p| p.name.eq_ignore_ascii_case(&args[0])));
                    if !looks_like_named {
                        let code = args.join(" ");
                        cadence_core::mini::parse_mini(&code)?;
                        let mut updated = existing;
                        updated.kind = PatternKind::Mini { code, steps };
                        return self.apply(Action::Edit {
                            edit: Edit::PutPattern { pattern: updated },
                        });
                    }
                }
            }
        }
        ensure!(args.len() >= 2, "Usage: :mini NAME [STEPS] <CODE...>");
        let name = args[0].clone();
        let (steps, code_start) = if args.len() >= 3 {
            if let Ok(s) = args[1].parse::<u16>() {
                (s, 2)
            } else {
                (16, 1)
            }
        } else {
            (16, 1)
        };
        let code = args[code_start..].join(" ");
        cadence_core::mini::parse_mini(&code)?;
        // If a Mini pattern with this exact name exists, update it in place —
        // but never overwrite an existing non-Mini pattern!
        if let Some(existing) = self
            .doc
            .project()
            .patterns
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(&name))
            .cloned()
        {
            ensure!(
                matches!(existing.kind, PatternKind::Mini { .. }),
                "Pattern '{}' is {}, not a Mini pattern; choose a different name",
                existing.name,
                kind_label(&existing.kind)
            );
            let mut updated = existing;
            updated.kind = PatternKind::Mini { code, steps };
            return self.apply(Action::Edit {
                edit: Edit::PutPattern { pattern: updated },
            });
        }
        self.apply(Action::CreatePattern {
            name,
            kind: PatternKind::Mini { code, steps },
        })
    }

    pub(crate) fn derive_command(&mut self, args: &[String]) -> Result<Outcome> {
        ensure!(args.len() >= 3, "Usage: :derive NAME OP SOURCE [ARGS...]");
        let name = args[0].clone();
        let kind = match args[1].as_str() {
            "add" => {
                ensure!(args.len() == 4, "Usage: :derive NAME add NOTES DEGREES");
                PatternKind::AddDegrees {
                    notes: self.pattern_id(&args[2])?,
                    values: self.pattern_id(&args[3])?,
                }
            }
            "mask" => {
                ensure!(args.len() == 4, "Usage: :derive NAME mask NOTES RHYTHM");
                PatternKind::Mask {
                    notes: self.pattern_id(&args[2])?,
                    rhythm: self.pattern_id(&args[3])?,
                }
            }
            "transpose" => {
                ensure!(
                    args.len() == 4,
                    "Usage: :derive NAME transpose SOURCE SEMITONES"
                );
                PatternKind::Transpose {
                    source: self.pattern_id(&args[2])?,
                    semitones: args[3].parse()?,
                }
            }
            "from" => {
                ensure!(
                    (4..=7).contains(&args.len()),
                    "Usage: :derive NAME from DEGREES RHYTHM [DEGREE_OFFSET VELOCITY GATE]"
                );
                PatternKind::NotesFromValues {
                    values: self.pattern_id(&args[2])?,
                    rhythm: self.pattern_id(&args[3])?,
                    degree_offset: args.get(4).map(|s| s.parse()).transpose()?.unwrap_or(0),
                    velocity: args
                        .get(5)
                        .map(|s| parse_amount(s))
                        .transpose()?
                        .unwrap_or(0.8),
                    gate: args.get(6).map(|s| s.parse()).transpose()?.unwrap_or(0.75),
                }
            }
            "fast" => {
                ensure!(
                    (4..=5).contains(&args.len()),
                    "Usage: :derive NAME fast SOURCE NUM [DEN]"
                );
                PatternKind::TimeScale {
                    source: self.pattern_id(&args[2])?,
                    num: args[3].parse()?,
                    den: args.get(4).map(|s| s.parse()).transpose()?.unwrap_or(1),
                }
            }
            "slow" => {
                ensure!(
                    (4..=5).contains(&args.len()),
                    "Usage: :derive NAME slow SOURCE DEN [NUM]"
                );
                PatternKind::TimeScale {
                    source: self.pattern_id(&args[2])?,
                    num: args.get(4).map(|s| s.parse()).transpose()?.unwrap_or(1),
                    den: args[3].parse()?,
                }
            }
            "swing" => {
                ensure!(args.len() == 4, "Usage: :derive NAME swing SOURCE AMOUNT");
                PatternKind::Swing {
                    source: self.pattern_id(&args[2])?,
                    amount: parse_amount(&args[3])?,
                }
            }
            "ratchet" => {
                ensure!(args.len() == 4, "Usage: :derive NAME ratchet SOURCE COUNT");
                PatternKind::Ratchet {
                    source: self.pattern_id(&args[2])?,
                    count: args[3].parse()?,
                }
            }
            "shift" => {
                ensure!(args.len() == 4, "Usage: :derive NAME shift SOURCE STEPS");
                PatternKind::Shift {
                    source: self.pattern_id(&args[2])?,
                    steps: args[3].parse()?,
                }
            }
            "reverse" => {
                ensure!(args.len() == 3, "Usage: :derive NAME reverse SOURCE");
                PatternKind::Reverse {
                    source: self.pattern_id(&args[2])?,
                }
            }
            "chance" => {
                ensure!(
                    (4..=5).contains(&args.len()),
                    "Usage: :derive NAME chance SOURCE PROB [SEED]"
                );
                PatternKind::Chance {
                    source: self.pattern_id(&args[2])?,
                    probability: parse_amount(&args[3])?,
                    seed: args.get(4).map(|s| s.parse()).transpose()?.unwrap_or(1),
                }
            }
            "every" => {
                ensure!(
                    args.len() == 5,
                    "Usage: :derive NAME every INTERVAL BASE ALT"
                );
                PatternKind::Every {
                    interval: args[2].parse()?,
                    base: self.pattern_id(&args[3])?,
                    alt: self.pattern_id(&args[4])?,
                }
            }
            "arp" => {
                ensure!(
                    (4..=5).contains(&args.len()),
                    "Usage: :derive NAME arp SOURCE up|down|updown|random [RATE]"
                );
                let mode = match args[3].to_ascii_lowercase().as_str() {
                    "up" => cadence_core::material::ArpMode::Up,
                    "down" => cadence_core::material::ArpMode::Down,
                    "updown" | "up-down" | "pingpong" => cadence_core::material::ArpMode::UpDown,
                    "random" | "rand" => cadence_core::material::ArpMode::Random,
                    _ => bail!("Arp modes: up, down, updown, random"),
                };
                PatternKind::Arp {
                    source: self.pattern_id(&args[2])?,
                    mode,
                    rate: args.get(4).map(|s| s.parse()).transpose()?.unwrap_or(1),
                }
            }
            "fold" => {
                ensure!(
                    args.len() == 5,
                    "Usage: :derive NAME fold SOURCE LOW_MIDI HIGH_MIDI"
                );
                PatternKind::RangeFold {
                    source: self.pattern_id(&args[2])?,
                    low_midi: args[3].parse()?,
                    high_midi: args[4].parse()?,
                }
            }
            "concat" => {
                ensure!(args.len() >= 4, "Usage: :derive NAME concat P1 P2 ...");
                let parts = args[2..]
                    .iter()
                    .map(|s| self.pattern_id(s))
                    .collect::<Result<Vec<_>>>()?;
                PatternKind::Concat { parts }
            }
            "stack" => {
                ensure!(args.len() >= 4, "Usage: :derive NAME stack P1 P2 ...");
                let parts = args[2..]
                    .iter()
                    .map(|s| self.pattern_id(s))
                    .collect::<Result<Vec<_>>>()?;
                PatternKind::Stack { parts }
            }
            "shape" => {
                ensure!(
                    (4..=5).contains(&args.len()),
                    "Usage: :derive NAME shape SOURCE VELOCITY [GATE]"
                );
                PatternKind::Shape {
                    source: self.pattern_id(&args[2])?,
                    velocity: self.pattern_id(&args[3])?,
                    gate: args.get(4).map(|s| self.pattern_id(s)).transpose()?,
                }
            }
            _ => bail!(
                "Operations: add, mask, transpose, from, fast, slow, swing, ratchet, shift, reverse, chance, every, arp, fold, concat, stack, shape"
            ),
        };
        self.apply(Action::CreatePattern { name, kind })
    }

    pub(crate) fn part_command(&mut self, args: &[String]) -> Result<Outcome> {
        if args.is_empty() {
            return self.begin_workflow(WorkflowKind::Part);
        }
        if args[0].eq_ignore_ascii_case("new") {
            if args.len() == 1 {
                return self.begin_workflow(WorkflowKind::PlacedPart);
            }
            ensure!(
                args.len() >= 4,
                "Usage: :part new NAME synth|sampler|kit|modular|<PRESET> DEGREE..."
            );
            let sound = parse_single_sound_token(
                &self.assets.presets,
                self.doc.asset_base(),
                self.io.as_ref(),
                &args[2],
            )?;
            let notes = args[3..]
                .iter()
                .map(|s| self.step_from_text(s))
                .collect::<Result<Vec<_>>>()?;
            return self.apply(Action::PlacedPart {
                name: args[1].clone(),
                sound,
                notes,
                start_step: self.arrange_ui.step,
                length_steps: None,
                transpose: None,
                velocity_scale: None,
            });
        }
        if args.len() == 1 {
            let id = self.part_id(&args[0])?;
            self.push_back();
            self.show_detail(Detail::Part(id));
            self.control_cursor = 0;
            return Ok(Outcome::default());
        }
        ensure!(
            args.len() == 3,
            "Usage: :part NAME PATTERN INSTRUMENT | :part NAME [pattern|instrument|transpose|velocity VALUE] | :part new NAME PRESET DEGREE..."
        );
        if matches!(
            args[1].as_str(),
            "pattern" | "instrument" | "transpose" | "velocity"
        ) && self.part_id(&args[0]).is_ok()
        {
            let id = self.part_id(&args[0])?;
            let mut part = self.part(id).context("Part missing")?.clone();
            match args[1].as_str() {
                "pattern" => part.pattern = self.pattern_id(&args[2])?,
                "instrument" => part.instrument = self.instrument_id(&args[2])?,
                "transpose" => part.transpose = args[2].parse()?,
                "velocity" => part.velocity_scale = parse_amount(&args[2])?,
                _ => unreachable!(),
            }
            return self.apply(Action::Edit {
                edit: Edit::PutPart { part },
            });
        }
        self.apply(Action::CreatePart {
            name: args[0].clone(),
            pattern: self.pattern_id(&args[1])?,
            instrument: self.instrument_id(&args[2])?,
            transpose: None,
            velocity_scale: None,
        })
    }
}
