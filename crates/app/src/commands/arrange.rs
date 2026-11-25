//! Arrangement, transport, timeline, section, lane, and placement Ex commands.
use super::{parse_phase, require_args, ArrangeVerb};
use crate::*;

impl App {
    pub(super) fn execute_arrange_verb(
        &mut self,
        verb: ArrangeVerb,
        args: &[String],
    ) -> Result<Outcome> {
        match verb {
            ArrangeVerb::Stop => {
                require_args(args, 0, ":stop")?;
                self.apply(Action::Stop)
            }
            ArrangeVerb::Restart => {
                require_args(args, 0, ":restart")?;
                self.apply(Action::RestartLoop)
            }
            ArrangeVerb::Cue { play } => {
                ensure!(args.len() <= 1, "Usage: :cue [BAR] or :play [BAR]");
                if let Some(bar) = args.first() {
                    let bar = bar
                        .parse::<usize>()?
                        .checked_sub(1)
                        .context("Bars start at 1")?;
                    let step = bar.checked_mul(16).context("Bar overflow")?;
                    self.apply(Action::CueSong { step, play })
                } else {
                    self.apply(Action::CueSelection { play })
                }
            }
            ArrangeVerb::Monitor => {
                ensure!(args.len() <= 1, "Usage: :monitor [on|off|toggle]");
                let visible = match args.first().map(String::as_str) {
                    None | Some("toggle") => !self.view_state.monitor_visible,
                    Some("on") => true,
                    Some("off") => false,
                    _ => bail!("Usage: :monitor [on|off|toggle]"),
                };
                self.apply(Action::SetMonitor { visible })
            }
            ArrangeVerb::Follow => {
                ensure!(args.len() <= 1, "Usage: :follow [on|off|resume|toggle]");
                match args.first().map(String::as_str) {
                    None | Some("toggle") => self.apply(Action::ToggleFollow),
                    Some("on" | "resume") => self.apply(Action::SetFollow { enabled: true }),
                    Some("off") => self.apply(Action::SetFollow { enabled: false }),
                    _ => bail!("Usage: :follow [on|off|resume|toggle]"),
                }
            }
            ArrangeVerb::Bpm => {
                require_args(args, 1, ":bpm NUMBER")?;
                self.apply(Action::Edit {
                    edit: Edit::SetTempo {
                        bpm: args[0].parse()?,
                    },
                })
            }
            ArrangeVerb::Bars => {
                require_args(args, 1, ":bars COUNT")?;
                self.apply(Action::Edit {
                    edit: Edit::SetBars {
                        bars: args[0].parse()?,
                    },
                })
            }
            ArrangeVerb::Title => {
                ensure!(!args.is_empty(), "Usage: :title NAME");
                self.apply(Action::Edit {
                    edit: Edit::RenameProject {
                        title: args.join(" "),
                    },
                })
            }
            ArrangeVerb::Key => {
                ensure!(
                    (1..=3).contains(&args.len()),
                    "Usage: :key D4 [minor|major|dorian|phrygian|lydian|mixolydian|harmonic_minor|melodic_minor|pentatonic|major_pentatonic|blues|whole_tone|chromatic]"
                );
                let root_midi =
                    cadence_core::music::parse_root_midi(&args[0]).context("Invalid key note")?;
                let scale_raw = if args.len() > 1 {
                    args[1..].join(" ")
                } else {
                    "minor".into()
                };
                let scale =
                    cadence_core::music::named_scale(&scale_raw).context("Unknown scale")?;
                self.apply(Action::Edit {
                    edit: Edit::SetKey { root_midi, scale },
                })
            }
            ArrangeVerb::Lane => self.lane_command(args),
            ArrangeVerb::Place => {
                if args.is_empty() {
                    return self.begin_workflow(WorkflowKind::Place);
                }
                ensure!(
                    (4..=6).contains(&args.len()),
                    "Usage: :place PART LANE BAR BARS [song|restart] [OFFSET_STEPS]"
                );
                let bar = args[2].parse::<u32>()?;
                ensure!(bar > 0, "Bars start at 1");
                let length = args[3]
                    .parse::<u32>()?
                    .checked_mul(16)
                    .context("Length overflow")?;
                let phase = parse_phase(args.get(4).map_or("song", String::as_str))?;
                self.apply(Action::Place {
                    part: self.part_id(&args[0])?,
                    lane: self.lane_id(&args[1])?,
                    start_step: (bar - 1).checked_mul(16).context("Bar overflow")?,
                    length_steps: length,
                    phase,
                    offset_steps: args.get(5).map(|s| s.parse()).transpose()?.unwrap_or(0),
                })
            }
            ArrangeVerb::Placement => self.placement_command(args),
            ArrangeVerb::Section => {
                if args.is_empty() {
                    return self.begin_workflow(WorkflowKind::Section);
                }
                if args.len() == 1 {
                    let s = self
                        .doc
                        .project()
                        .sections
                        .iter()
                        .find(|s| {
                            s.name.eq_ignore_ascii_case(&args[0]) || s.id.to_string() == args[0]
                        })
                        .context("Section missing")?;
                    self.arrange_ui.step = s.start_step;
                    self.suspend_follow();
                    self.show_workspace(Workspace::Arrange);
                    self.select_at_cursor();
                    return Ok(Outcome::default());
                }
                if args.len() >= 3
                    && matches!(
                        args[1].to_ascii_lowercase().as_str(),
                        "rename" | "start" | "length"
                    )
                {
                    let mut section = self
                        .doc
                        .project()
                        .sections
                        .iter()
                        .find(|s| {
                            s.name.eq_ignore_ascii_case(&args[0]) || s.id.to_string() == args[0]
                        })
                        .cloned()
                        .context("Section missing")?;
                    match args[1].to_ascii_lowercase().as_str() {
                        "rename" => {
                            let next_name = args[2..].join(" ").trim().to_string();
                            ensure!(!next_name.is_empty(), "Section name cannot be empty");
                            section.name = next_name;
                        }
                        "start" => {
                            ensure!(args.len() == 3, "Usage: :section NAME start BAR");
                            let bar = args[2]
                                .parse::<u32>()?
                                .checked_sub(1)
                                .context("Bars start at 1")?;
                            section.start_step = bar.checked_mul(16).context("Bar overflow")?;
                        }
                        "length" => {
                            ensure!(args.len() == 3, "Usage: :section NAME length BARS");
                            let len = args[2]
                                .parse::<u32>()?
                                .checked_mul(16)
                                .context("Length overflow")?;
                            ensure!(len > 0, "Section length must be at least 1 bar");
                            section.length_steps = len;
                        }
                        _ => unreachable!(),
                    }
                    let end_bar = (section.start_step + section.length_steps).div_ceil(16) as usize;
                    let mut edits = Vec::new();
                    if end_bar > self.doc.project().bars {
                        edits.push(Edit::SetBars { bars: end_bar });
                    }
                    edits.push(Edit::PutSection { section });
                    return self.apply(Action::Transaction { edits });
                }
                ensure!(
                    args.len() == 3,
                    "Usage: :section | :section NAME [BAR BARS | rename NEW | start BAR | length BARS]"
                );
                let bar = args[1]
                    .parse::<u32>()?
                    .checked_sub(1)
                    .context("Bars start at 1")?;
                let start_step = bar.checked_mul(16).context("Bar overflow")?;
                let length_steps = args[2]
                    .parse::<u32>()?
                    .checked_mul(16)
                    .context("Length overflow")?;
                let end_bar = (start_step + length_steps).div_ceil(16) as usize;
                if end_bar > self.doc.project().bars {
                    let id = self.doc.fresh_ids(1)?[0];
                    self.apply(Action::Transaction {
                        edits: vec![
                            Edit::SetBars { bars: end_bar },
                            Edit::PutSection {
                                section: Section {
                                    id,
                                    name: args[0].clone(),
                                    start_step,
                                    length_steps,
                                },
                            },
                        ],
                    })
                } else {
                    self.apply(Action::CreateSection {
                        name: args[0].clone(),
                        start_step,
                        length_steps,
                    })
                }
            }
            ArrangeVerb::End => {
                require_args(args, 1, ":end loop|stop|continue")?;
                let end = cadence_core::music::EndRule::parse(&args[0])
                    .context("Usage: :end loop|stop|continue")?;
                self.set_end_rule(end)
            }
            ArrangeVerb::Loop => match args.first().map(|s| s.to_ascii_lowercase()).as_deref() {
                None => self.toggle_song_loop(),
                Some("song" | "off") => self.disable_loop(),
                Some("clear" | "none" | "reset") => self.clear_loop_region(),
                Some(_) => {
                    ensure!(args.len() == 1, "Usage: :loop [song|off|clear|SECTION]");
                    let s = self
                        .doc
                        .project()
                        .sections
                        .iter()
                        .find(|s| {
                            s.name.eq_ignore_ascii_case(&args[0]) || s.id.to_string() == args[0]
                        })
                        .context("Section missing")?;
                    self.activate_loop((s.start_step, s.start_step + s.length_steps))
                }
            },
            ArrangeVerb::Arrange => {
                ensure!(
                    args.len() <= 1,
                    "Usage: :arrange [placements|overview|roll]"
                );
                let view = match args.first().map(String::as_str) {
                    None => self.arrange_ui.view,
                    Some("placements") => ArrangeView::Placements,
                    Some("overview") => ArrangeView::Overview,
                    Some("roll") => ArrangeView::Roll,
                    _ => bail!("Use placements, overview or roll"),
                };
                self.apply(Action::SetArrangeView { view })
            }
            ArrangeVerb::InspectEvent => {
                require_args(args, 0, ":inspect-event")?;
                self.open_arrangement_inspection()
            }
        }
    }

    pub(crate) fn lane_command(&mut self, args: &[String]) -> Result<Outcome> {
        if args.is_empty() {
            return self.begin_workflow(WorkflowKind::Lane);
        }
        if args.len() == 1 {
            return self.apply(Action::CreateLane {
                name: args[0].clone(),
            });
        }
        let lane = self.lane_id(&args[0])?;
        match args[1].to_ascii_lowercase().as_str() {
            "add" => {
                ensure!(
                    (3..=5).contains(&args.len()),
                    "Usage: :lane LANE add PART [song|restart] [OFFSET]"
                );
                let part = self.part_id(&args[2])?;
                let mut phase = PhaseMode::Song;
                let mut offset_steps = 0;
                for word in &args[3..] {
                    match word.to_ascii_lowercase().as_str() {
                        "song" => phase = PhaseMode::Song,
                        "restart" => phase = PhaseMode::Restart,
                        other => {
                            offset_steps = other.parse().with_context(|| {
                                format!("{other} is not song, restart or an offset")
                            })?
                        }
                    }
                }
                self.apply(Action::AddLanePart {
                    lane,
                    entry: LanePart {
                        part,
                        phase,
                        offset_steps,
                    },
                })
            }
            "drop" => {
                ensure!(args.len() == 3, "Usage: :lane LANE drop PART");
                let part = self.part_id(&args[2])?;
                self.apply(Action::DropLanePart { lane, part })
            }
            _ => bail!("Usage: :lane NAME | :lane LANE add PART [song|restart] [OFFSET] | :lane LANE drop PART"),
        }
    }

    pub(crate) fn placement_command(&mut self, args: &[String]) -> Result<Outcome> {
        ensure!(
            !args.is_empty(),
            "Usage: :placement ID [start|length|phase|offset|part VALUE]"
        );
        let id = args[0].parse()?;
        if args.len() == 1 {
            return self.apply(Action::SelectPlacement { placement: id });
        }
        ensure!(args.len() == 3, "Usage: :placement ID FIELD VALUE");
        let (l, p) = self.placement(id).context("Placement missing")?;
        let lane = l.id;
        let mut p = p.clone();
        match args[1].as_str() {
            "start" => {
                let bar = args[2]
                    .parse::<u32>()?
                    .checked_sub(1)
                    .context("Bars start at 1")?;
                p.start_step = bar.checked_mul(16).context("Bar overflow")?;
            }
            "length" => {
                p.length_steps = args[2]
                    .parse::<u32>()?
                    .checked_mul(16)
                    .context("Length overflow")?
            }
            "phase" => p.phase = parse_phase(&args[2])?,
            "offset" => p.offset_steps = args[2].parse()?,
            "part" => p.part = self.part_id(&args[2])?,
            _ => bail!("Placement fields: start, length, phase, offset, part"),
        };
        self.apply(Action::Edit {
            edit: Edit::PutPlacement { lane, placement: p },
        })
    }
}
