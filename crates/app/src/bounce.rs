//! Pure resampling / bounce-to-sampler orchestration (`:bounce` / `BounceRequest`).
//!
//! `cadence-app` stays strictly pure (no `cadence-engine` dependency): `prepare_bounce`
//! compiles the isolated or range `ScoreWindow` into a `BounceRequest` on `Outcome`,
//! the host renders WAV audio via `cadence_engine::audio::render::render_bounce_wav`,
//! and `App::complete_bounce` ingests the rendered file as a playable `Sampler`.
use super::*;
use cadence_core::assets::ensure_sample_bank_ready;
use cadence_core::music::{note_name, parse_root_midi, EndRule};
use cadence_core::rack::{module_sample_key, ModuleKind, RackPatch, RackZone, MAX_ZONES};
use cadence_core::sampler::SampleLoopMode;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BounceScope {
    Part(Id),
    Pattern(Id),
    Range {
        /// 1-based inclusive start bar.
        start_bar: usize,
        /// 1-based inclusive end bar.
        end_bar: usize,
    },
}

/// Destination for a resampled WAV bounce.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BounceTarget {
    /// Create a brand-new Sampler Rack instrument (plus optional Audio channel / trigger Part).
    #[default]
    NewSampler,
    /// Load the bounced WAV directly into an existing or newly created Rack zone's Sampler module.
    RackZone {
        instrument: Id,
        #[serde(default)]
        zone_index: Option<usize>,
        #[serde(default)]
        midi: Option<u8>,
        #[serde(default)]
        zone_name: Option<String>,
    },
}

/// Pure render request emitted on `Outcome::bounce` for the host to render off-crate.
#[derive(Clone, Debug)]
pub struct BounceRequest {
    pub scope: BounceScope,
    pub target: BounceTarget,
    pub score: ScoreWindow,
    pub start_step: u64,
    pub steps: u64,
    pub sample_rate: u32,
    /// Portable project-relative path stored on the Sampler module (e.g. `"samples/lead-bounce.wav"`).
    pub relative_path: String,
    /// Absolute disk path where the host writes the stereo WAV.
    pub output_path: PathBuf,
    pub instrument_name: String,
    pub create_part: bool,
    pub source_lane: Option<Id>,
}

/// Pure specification for landing a newly written stereo WAV (from `:bounce` or live
/// audio recording) into a `Sampler` Rack, optional trigger `Part`, optional arranged
/// `Placement`, or target `RackZone` via ONE atomic `Document::commit`.
#[derive(Clone, Debug)]
pub(crate) struct SampleLandingSpec {
    pub target: BounceTarget,
    pub relative_path: String,
    pub output_path: PathBuf,
    pub instrument_name: String,
    pub steps: u64,
    pub create_part: bool,
    /// When `Some(start_step)`, places the created trigger `Part` on `source_lane` (or a new
    /// lane) at `start_step` with `length_steps = steps`, expanding `project.bars` if needed.
    pub place_in_arrange_at: Option<u32>,
    pub source_lane: Option<Id>,
    pub status_summary: String,
}

pub(crate) fn slugify_bounce(name: &str) -> String {
    crate::slugify(name, "bounce", Some(48))
}

pub(crate) fn parse_bar_range(args: &[String]) -> Result<(usize, usize, usize)> {
    ensure!(
        !args.is_empty(),
        "Usage: :bounce bars START..END [NAME] [--part]"
    );
    let token = args[0].trim();
    if let Some((a, b)) = token
        .split_once("..=")
        .or_else(|| token.split_once(".."))
        .or_else(|| token.split_once('-'))
    {
        let start_bar = a
            .trim()
            .parse::<usize>()
            .context("Start bar must be a positive integer")?;
        let end_bar = b
            .trim()
            .parse::<usize>()
            .context("End bar must be a positive integer")?;
        return Ok((start_bar, end_bar, 1));
    }
    let start_bar = token
        .parse::<usize>()
        .context("Bar range expects START..END (e.g. 1..2)")?;
    if let Some(second) = args.get(1).and_then(|s| s.parse::<usize>().ok()) {
        Ok((start_bar, second, 2))
    } else {
        Ok((start_bar, start_bar, 1))
    }
}

fn project_references_sample_path(project: &Project, rel: &str) -> bool {
    project.instruments.iter().any(|inst| {
        inst.rack
            .sampler_modules()
            .any(|(_, _, _, s)| s.path.as_deref() == Some(rel))
    })
}

pub(crate) fn parse_zone_midi_token(token: &str) -> Option<u8> {
    let trimmed = token.trim();
    if let Ok(n) = trimmed.parse::<u8>() {
        if n <= 127 {
            return Some(n);
        }
    }
    parse_root_midi(trimmed)
}

impl App {
    /// Default bounce destination for the current focus: active zone when focused on a multi-zone
    /// Rack or the Rack `Zones` tab, otherwise a brand-new Sampler Rack.
    pub(crate) fn default_bounce_target(&self) -> Option<BounceTarget> {
        if !self.deck_focused() {
            return None;
        }
        let Detail::Instrument(inst_id) = self.detail else {
            return None;
        };
        let inst = self.instrument(inst_id)?;
        if inst.rack.zones.len() > 1 || self.rack_ui.tab == RackTab::Zones {
            Some(BounceTarget::RackZone {
                instrument: inst_id,
                zone_index: Some(inst.rack.clamp_zone_index(self.rack_ui.zone_cursor)),
                midi: None,
                zone_name: None,
            })
        } else {
            None
        }
    }

    /// Open the pre-filled `WorkflowKind::Bounce` dialog (`tb`). Pressing `Enter` immediately
    /// commits with the smart defaults (`tb → Enter` = 2 keys).
    pub(crate) fn open_bounce_dialog(&mut self) -> Result<Outcome> {
        let scope = self.default_bounce_scope()?;
        let target = self.default_bounce_target().unwrap_or_default();
        self.clear_entry_state();
        let origin = Some(self.navigation());
        let ctx = self.workflow_context();
        let draft = drafts::WorkflowDraft::bounce(
            self.doc.project(),
            ctx,
            scope,
            target,
            &self.assets.presets,
        );
        self.modal = ActiveModal::Workflow {
            draft: Box::new(draft),
            origin,
        };
        self.input.mode = Mode::Workflow;
        self.status =
            "Bounce · Enter bounces now · s scope · r range · t target · n name · p trigger part · Esc cancel"
                .into();
        Ok(self.release_held())
    }

    /// Intelligently bounce the currently focused Part, Pattern, or Arrangement loop/bar range.
    /// When a multi-zone Rack or the Rack Zones tab has focus, targets its active zone directly.
    pub fn bounce_selection(&mut self) -> Result<Outcome> {
        let target = self.default_bounce_target();
        self.apply(Action::Bounce {
            scope: None,
            name: None,
            create_part: false,
            target,
        })
    }

    pub(crate) fn resolve_sample_zone_target(
        &self,
        zone_args: &[String],
        verb: &str,
        default_label: &str,
    ) -> Result<BounceTarget> {
        if zone_args.is_empty() {
            let inst_id = self
                .active_instrument()
                .or(self.selected_instrument)
                .with_context(|| format!("Select or name an instrument for :{verb} zone"))?;
            let inst = self.instrument(inst_id).context("Instrument missing")?;
            let zone_index = (!inst.rack.zones.is_empty())
                .then(|| inst.rack.clamp_zone_index(self.rack_ui.zone_cursor));
            return Ok(BounceTarget::RackZone {
                instrument: inst_id,
                zone_index,
                midi: zone_index.is_none().then_some(60),
                zone_name: None,
            });
        }
        let (inst_id, zone_tok, extra_name) = if zone_args.len() == 1 {
            let inst_id = self
                .active_instrument()
                .or(self.selected_instrument)
                .with_context(|| format!("Specify :{verb} zone <INSTRUMENT> <ZONE|NOTE|NAME>"))?;
            (inst_id, zone_args[0].as_str(), None)
        } else {
            let inst_id = self.instrument_id(&zone_args[0])?;
            let extra = (zone_args.len() > 2).then(|| zone_args[2..].join(" "));
            (inst_id, zone_args[1].as_str(), extra)
        };
        let inst = self.instrument(inst_id).context("Instrument missing")?;
        if let Some(idx) = inst.rack.find_zone(zone_tok) {
            return Ok(BounceTarget::RackZone {
                instrument: inst_id,
                zone_index: Some(idx),
                midi: None,
                zone_name: extra_name,
            });
        }
        if let Some(midi) = parse_zone_midi_token(zone_tok) {
            if let Some(idx) = inst
                .rack
                .zones
                .iter()
                .position(|z| z.low_midi <= midi && midi <= z.high_midi)
            {
                return Ok(BounceTarget::RackZone {
                    instrument: inst_id,
                    zone_index: Some(idx),
                    midi: Some(midi),
                    zone_name: extra_name,
                });
            }
            return Ok(BounceTarget::RackZone {
                instrument: inst_id,
                zone_index: None,
                midi: Some(midi),
                zone_name: extra_name
                    .or_else(|| Some(format!("{default_label} {}", note_name(midi)))),
            });
        }
        bail!(
            "Zone '{zone_tok}' not found on '{}' (use z1, 1..={}, zone name, or MIDI note e.g. C3)",
            inst.name,
            inst.rack.zones.len()
        )
    }

    fn resolve_bounce_zone_target(&self, zone_args: &[String]) -> Result<BounceTarget> {
        self.resolve_sample_zone_target(zone_args, "bounce", "Bounce")
    }

    /// Execute `:bounce [part|pattern|bars A..B|BARS] [zone <INSTRUMENT> <ZONE|NOTE|NAME>] [NAME] [--part]`.
    pub(super) fn bounce_command(&mut self, args: &[String]) -> Result<Outcome> {
        let mut create_part = false;
        let mut pos = Vec::with_capacity(args.len());
        for arg in args {
            if arg == "--part" || arg == "-p" {
                create_part = true;
            } else {
                pos.push(arg.clone());
            }
        }
        let (scope_pos, target) =
            if let Some(z_idx) = pos.iter().position(|a| a.eq_ignore_ascii_case("zone")) {
                let target = self.resolve_bounce_zone_target(&pos[z_idx + 1..])?;
                (&pos[..z_idx], Some(target))
            } else {
                (pos.as_slice(), None)
            };
        if scope_pos.is_empty() {
            return self.apply(Action::Bounce {
                scope: None,
                name: None,
                create_part,
                target,
            });
        }
        let first = scope_pos[0].to_ascii_lowercase();
        match first.as_str() {
            "part" => {
                let (part_id, name_slice) = if scope_pos.len() >= 2 {
                    if let Ok(id) = self.part_id(&scope_pos[1]) {
                        (id, &scope_pos[2..])
                    } else if let Some(id) = self.focused_part_id() {
                        (id, &scope_pos[1..])
                    } else {
                        bail!("Part '{}' not found (or select a part first)", scope_pos[1]);
                    }
                } else {
                    (
                        self.focused_part_id()
                            .context("Select or name a part to bounce")?,
                        &scope_pos[1..],
                    )
                };
                let name = (!name_slice.is_empty()).then(|| name_slice.join(" "));
                self.apply(Action::Bounce {
                    scope: Some(BounceScope::Part(part_id)),
                    name,
                    create_part,
                    target,
                })
            }
            "pattern" => {
                let (pattern_id, name_slice) = if scope_pos.len() >= 2 {
                    if let Ok(id) = self.pattern_id(&scope_pos[1]) {
                        (id, &scope_pos[2..])
                    } else if let Some(id) = self.selected_pattern {
                        (id, &scope_pos[1..])
                    } else {
                        bail!(
                            "Pattern '{}' not found (or select a pattern first)",
                            scope_pos[1]
                        );
                    }
                } else {
                    (
                        self.selected_pattern
                            .context("Select or name a pattern to bounce")?,
                        &scope_pos[1..],
                    )
                };
                let name = (!name_slice.is_empty()).then(|| name_slice.join(" "));
                self.apply(Action::Bounce {
                    scope: Some(BounceScope::Pattern(pattern_id)),
                    name,
                    create_part,
                    target,
                })
            }
            "bars" | "range" => {
                let (start_bar, end_bar, consumed) = parse_bar_range(&scope_pos[1..])?;
                let name_slice = &scope_pos[1 + consumed..];
                let name = (!name_slice.is_empty()).then(|| name_slice.join(" "));
                self.apply(Action::Bounce {
                    scope: Some(BounceScope::Range { start_bar, end_bar }),
                    name,
                    create_part,
                    target,
                })
            }
            _ if target.is_some() => {
                // Allow concise `:bounce 2 zone Kit BD` or `:bounce 1..4 zone Kit C2`.
                if let Ok(count) = scope_pos[0].parse::<usize>() {
                    ensure!(
                        scope_pos.len() == 1 && count >= 1,
                        "Usage: :bounce [BARS|START..END] zone <INSTRUMENT> <ZONE|NOTE|NAME>"
                    );
                    let start_bar = ((self.arrange_ui.step / 16) as usize + 1)
                        .clamp(1, self.doc.project().bars.max(1));
                    let end_bar = (start_bar + count - 1).min(self.doc.project().bars.max(1));
                    return self.apply(Action::Bounce {
                        scope: Some(BounceScope::Range { start_bar, end_bar }),
                        name: None,
                        create_part,
                        target,
                    });
                }
                if let Ok((start_bar, end_bar, consumed)) = parse_bar_range(scope_pos) {
                    let name_slice = &scope_pos[consumed..];
                    let name = (!name_slice.is_empty()).then(|| name_slice.join(" "));
                    return self.apply(Action::Bounce {
                        scope: Some(BounceScope::Range { start_bar, end_bar }),
                        name,
                        create_part,
                        target,
                    });
                }
                let name = Some(scope_pos.join(" "));
                self.apply(Action::Bounce {
                    scope: None,
                    name,
                    create_part,
                    target,
                })
            }
            _ => {
                let name = Some(scope_pos.join(" "));
                self.apply(Action::Bounce {
                    scope: None,
                    name,
                    create_part,
                    target,
                })
            }
        }
    }

    fn focused_part_id(&self) -> Option<Id> {
        match self.detail {
            Detail::Part(id) if self.part(id).is_some() => Some(id),
            Detail::Placement(pid) => self.placement(pid).map(|(_, p)| p.part),
            _ => self.selected_part.filter(|id| self.part(*id).is_some()),
        }
    }

    pub(crate) fn default_bounce_scope(&self) -> Result<BounceScope> {
        let project = self.doc.project();
        // Always honor an active Arrange loop range before focused deck/part context so
        // bouncing from `gi → Zones` (or any other deck) respects the user's loop selection.
        if let Some((start_step, end_step)) = self.playing_loop() {
            if end_step > start_step {
                let start_bar = (start_step / 16) as usize + 1;
                let end_bar = (end_step.div_ceil(16) as usize)
                    .max(start_bar)
                    .min(project.bars.max(1));
                return Ok(BounceScope::Range { start_bar, end_bar });
            }
        }
        if self.deck_focused() {
            if let Some(part_id) = self.focused_part_id() {
                return Ok(BounceScope::Part(part_id));
            }
        }
        match self.workspace() {
            Workspace::Pattern => {
                if let Some(pat_id) = self.selected_pattern {
                    if project
                        .pattern_type(pat_id)
                        .is_ok_and(|t| t == PatternType::Notes)
                    {
                        return Ok(BounceScope::Pattern(pat_id));
                    }
                }
            }
            Workspace::Arrange => {
                if let Some(part_id) = self.focused_part_id() {
                    return Ok(BounceScope::Part(part_id));
                }
                let bar = ((self.arrange_ui.step / 16) as usize + 1).clamp(1, project.bars.max(1));
                return Ok(BounceScope::Range {
                    start_bar: bar,
                    end_bar: bar,
                });
            }
            Workspace::Mixer => {}
        }
        if let Some(part_id) = self.focused_part_id() {
            return Ok(BounceScope::Part(part_id));
        }
        if let Some(pat_id) = self.selected_pattern {
            if project
                .pattern_type(pat_id)
                .is_ok_and(|t| t == PatternType::Notes)
            {
                return Ok(BounceScope::Pattern(pat_id));
            }
        }
        Ok(BounceScope::Range {
            start_bar: 1,
            end_bar: project.bars.max(1),
        })
    }

    fn lane_for_part(&self, part_id: Id) -> Option<Id> {
        let project = self.doc.project();
        if let Some(sel) = self.selected_lane {
            if project.lanes.iter().any(|l| {
                l.id == sel
                    && (l.parts.iter().any(|lp| lp.part == part_id)
                        || l.placements.iter().any(|p| p.part == part_id))
            }) {
                return Some(sel);
            }
        }
        project
            .lanes
            .iter()
            .find(|l| {
                l.parts.iter().any(|lp| lp.part == part_id)
                    || l.placements.iter().any(|p| p.part == part_id)
            })
            .map(|l| l.id)
            .or(self.selected_lane)
    }

    pub(crate) fn unique_named_entity(&self, requested: &str, fallback: &str) -> String {
        let base = requested.trim();
        let base = if base.is_empty() { fallback } else { base };
        let project = self.doc.project();
        crate::unique_name(
            base,
            project
                .instruments
                .iter()
                .map(|i| i.name.as_str())
                .chain(project.mixer.channels.iter().map(|c| c.name.as_str()))
                .chain(project.parts.iter().map(|p| p.name.as_str()))
                .chain(project.patterns.iter().map(|p| p.name.as_str()))
                .chain(project.lanes.iter().map(|l| l.name.as_str())),
            None,
        )
    }

    pub(crate) fn unique_sample_paths(
        &self,
        subdir: &str,
        instrument_name: &str,
        fallback_slug: &str,
    ) -> (String, PathBuf) {
        let slug = crate::slugify(instrument_name, fallback_slug, Some(48));
        let base = self.doc.asset_base();
        let project = self.doc.project();
        for suffix in 0usize..10_000 {
            let rel = if suffix == 0 {
                format!("{subdir}/{slug}.wav")
            } else {
                format!("{subdir}/{slug}-{}.wav", suffix + 1)
            };
            let abs = base.join(&rel);
            if !self.io.exists(&abs) && !project_references_sample_path(project, &rel) {
                return (rel, abs);
            }
        }
        let rel = format!("{subdir}/{slug}-{fallback_slug}.wav");
        let abs = base.join(&rel);
        (rel, abs)
    }

    pub(crate) fn validate_bounce_target(&self, target: &BounceTarget) -> Result<()> {
        let project = self.doc.project();
        match target {
            BounceTarget::NewSampler => {
                ensure!(
                    project.instruments.len() < material::MAX_INSTRUMENTS,
                    "Project already has the maximum {} instruments",
                    material::MAX_INSTRUMENTS
                );
            }
            BounceTarget::RackZone {
                instrument,
                zone_index,
                ..
            } => {
                let inst = project
                    .instrument(*instrument)
                    .context("Target Rack instrument missing")?;
                if let Some(idx) = *zone_index {
                    ensure!(
                        idx < inst.rack.zones.len(),
                        "Zone index z{} out of range on '{}'",
                        idx + 1,
                        inst.name
                    );
                } else {
                    ensure!(
                        inst.rack.zones.len() < MAX_ZONES,
                        "Rack '{}' already has the maximum {MAX_ZONES} zones",
                        inst.name
                    );
                }
            }
        }
        Ok(())
    }

    /// Build a pure `BounceRequest` on `Outcome` without touching audio rendering crates.
    pub(super) fn prepare_bounce(
        &mut self,
        scope: Option<BounceScope>,
        name: Option<String>,
        create_part: bool,
        target: Option<BounceTarget>,
    ) -> Result<Outcome> {
        let project = self.doc.project();
        let target = target.unwrap_or_default();
        self.validate_bounce_target(&target)?;
        let scope = match scope {
            Some(s) => s,
            None => self.default_bounce_scope()?,
        };
        let (score, start_step, steps, default_name, source_lane) = match scope {
            BounceScope::Part(part_id) => {
                let part = project.part(part_id).context("Part missing")?;
                ensure!(
                    project.pattern_type(part.pattern)? == PatternType::Notes,
                    "Part '{}' pattern does not produce notes",
                    part.name
                );
                let period = project
                    .pattern_period(part.pattern)?
                    .clamp(1, material::MAX_LITERAL_STEPS as u64)
                    as u32;
                let mut isolated = project.clone();
                let next_id = isolated.max_id() + 1;
                isolated.bars = (period as usize).div_ceil(16).max(1);
                isolated.end = EndRule::Stop;
                isolated.sections.clear();
                isolated.lanes = vec![Lane {
                    id: next_id,
                    name: "Bounce".into(),
                    parts: Vec::new(),
                    placements: vec![Placement {
                        id: next_id + 1,
                        part: part_id,
                        start_step: 0,
                        length_steps: period,
                        phase: PhaseMode::Restart,
                        offset_steps: 0,
                        exceptions: Vec::new(),
                    }],
                }];
                let score = isolated.compile_with_flags(&LiveFlags::default())?;
                (
                    score,
                    0u64,
                    u64::from(period),
                    format!("{} Bounce", part.name),
                    self.lane_for_part(part_id),
                )
            }
            BounceScope::Pattern(pattern_id) => {
                let pat = project.pattern(pattern_id).context("Pattern missing")?;
                ensure!(
                    project.pattern_type(pattern_id)? == PatternType::Notes,
                    "Pattern '{}' does not produce notes to bounce",
                    pat.name
                );
                let period = project
                    .pattern_period(pattern_id)?
                    .clamp(1, material::MAX_LITERAL_STEPS as u64)
                    as u32;
                let mut isolated = project.clone();
                let mut next_id = isolated.max_id() + 1;
                let matching_part = self
                    .selected_part
                    .and_then(|id| project.part(id))
                    .filter(|p| p.pattern == pattern_id)
                    .or_else(|| {
                        project.parts.iter().find(|p| {
                            p.pattern == pattern_id
                                && Some(p.instrument) == self.selected_instrument
                        })
                    })
                    .or_else(|| project.parts.iter().find(|p| p.pattern == pattern_id));
                let part_id = if let Some(existing) = matching_part {
                    existing.id
                } else {
                    let inst_id = self
                        .active_instrument()
                        .filter(|id| project.instrument(*id).is_some())
                        .or_else(|| project.instruments.first().map(|i| i.id));
                    let inst_id = if let Some(id) = inst_id {
                        id
                    } else {
                        let ch_id = next_id;
                        next_id += 1;
                        let synth_id = next_id;
                        next_id += 1;
                        isolated.mixer.channels.push(Channel::new(
                            ch_id,
                            ChannelKind::Audio,
                            "Bounce",
                        ));
                        isolated.instruments.push(InstrumentInstance {
                            id: synth_id,
                            name: "Bounce Preview".into(),
                            rack: self.preview_patch.clone(),
                            channel: ch_id,
                        });
                        synth_id
                    };
                    let tmp_part_id = next_id;
                    next_id += 1;
                    isolated.parts.push(Part {
                        id: tmp_part_id,
                        name: format!("{} Bounce", pat.name),
                        pattern: pattern_id,
                        instrument: inst_id,
                        transpose: 0,
                        velocity_scale: 1.0,
                        parameter_links: Vec::new(),
                    });
                    tmp_part_id
                };
                let lane_id = next_id;
                let placement_id = next_id + 1;
                isolated.bars = (period as usize).div_ceil(16).max(1);
                isolated.end = EndRule::Stop;
                isolated.sections.clear();
                isolated.lanes = vec![Lane {
                    id: lane_id,
                    name: "Bounce".into(),
                    parts: Vec::new(),
                    placements: vec![Placement {
                        id: placement_id,
                        part: part_id,
                        start_step: 0,
                        length_steps: period,
                        phase: PhaseMode::Restart,
                        offset_steps: 0,
                        exceptions: Vec::new(),
                    }],
                }];
                let score = isolated.compile_with_flags(&LiveFlags::default())?;
                let lane = matching_part
                    .and_then(|p| self.lane_for_part(p.id))
                    .or(self.selected_lane);
                (
                    score,
                    0u64,
                    u64::from(period),
                    format!("{} Bounce", pat.name),
                    lane,
                )
            }
            BounceScope::Range { start_bar, end_bar } => {
                ensure!(
                    start_bar >= 1 && start_bar <= end_bar,
                    "Bounce bar range must satisfy 1 <= start <= end"
                );
                ensure!(
                    end_bar <= project.bars,
                    "Bounce bar range {start_bar}..{end_bar} exceeds project length ({} bars)",
                    project.bars
                );
                let start_step = (start_bar as u64 - 1) * 16;
                let steps = (end_bar as u64 - start_bar as u64 + 1) * 16;
                let default_name = if start_bar == end_bar {
                    format!("Bar {start_bar} Bounce")
                } else {
                    format!("Bars {start_bar}-{end_bar} Bounce")
                };
                (
                    self.doc.compiled().clone(),
                    start_step,
                    steps,
                    default_name,
                    self.selected_lane,
                )
            }
        };

        ensure_sample_bank_ready(&score, &self.sample_bank())?;

        let requested_name = name
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or(default_name);
        let (instrument_name, file_slug_base) = match &target {
            BounceTarget::NewSampler => {
                let unique = self.unique_named_entity(&requested_name, "Bounce");
                (unique.clone(), unique)
            }
            BounceTarget::RackZone {
                instrument,
                zone_index,
                midi,
                zone_name,
            } => {
                let inst = self.instrument(*instrument).context("Instrument missing")?;
                let zlabel = match *zone_index {
                    Some(idx) => inst
                        .rack
                        .zones
                        .get(idx)
                        .map_or_else(|| format!("z{}", idx + 1), |z| z.name.clone()),
                    None => zone_name
                        .clone()
                        .unwrap_or_else(|| note_name(midi.unwrap_or(60))),
                };
                (
                    format!("{} [{}]", inst.name, zlabel),
                    format!("{}-{}-{}", inst.name, zlabel, requested_name),
                )
            }
        };
        let (relative_path, output_path) =
            self.unique_sample_paths("samples", &file_slug_base, "bounce");

        let request = BounceRequest {
            scope,
            target,
            score,
            start_step,
            steps,
            sample_rate: 48_000,
            relative_path: relative_path.clone(),
            output_path,
            instrument_name: instrument_name.clone(),
            create_part,
            source_lane,
        };
        self.status = format!("Bouncing {instrument_name} → {relative_path}…");
        Ok(Outcome {
            bounce: Some(request),
            ..Outcome::default()
        })
    }

    /// Complete a rendered bounce: incrementally register ONLY the newly written WAV (no full
    /// library rescan), create a playable `Sampler` OR load directly into the target
    /// Rack zone (pushing onto Undo), open it in `gi`, and signal audio sync.
    pub fn complete_bounce(
        &mut self,
        request: &BounceRequest,
        duration_sec: f64,
    ) -> Result<Outcome> {
        let status_summary = match &request.target {
            BounceTarget::RackZone { .. } => format!(
                "Bounced → {} ({}, {duration_sec:.2}s)",
                request.instrument_name, request.relative_path
            ),
            BounceTarget::NewSampler => format!(
                "Bounced {} → {} ({duration_sec:.2}s)",
                request.instrument_name, request.relative_path
            ),
        };
        self.land_recorded_sample(SampleLandingSpec {
            target: request.target.clone(),
            relative_path: request.relative_path.clone(),
            output_path: request.output_path.clone(),
            instrument_name: request.instrument_name.clone(),
            steps: request.steps,
            create_part: request.create_part,
            place_in_arrange_at: None,
            source_lane: request.source_lane,
            status_summary,
        })
    }

    /// Single Point of Truth for landing a newly written stereo WAV (from `:bounce` or live
    /// audio recording) into a target `RackZone` or a brand-new `Sampler` Rack + optional
    /// trigger `Part` + optional arranged `Placement` in ONE atomic undoable commit.
    pub(crate) fn land_recorded_sample(&mut self, spec: SampleLandingSpec) -> Result<Outcome> {
        self.assets
            .library
            .set_project_dir_if_unset(self.doc.asset_base());
        self.assets.library.register_file(&spec.output_path);

        let sample = self
            .assets
            .library
            .load(&spec.relative_path)
            .with_context(|| format!("Loading sample {}", spec.relative_path))?;

        if let BounceTarget::RackZone {
            instrument: inst_id,
            zone_index,
            midi,
            zone_name,
        } = &spec.target
        {
            let inst = self
                .instrument(*inst_id)
                .context("Target Rack instrument missing")?;
            let inst_name = inst.name.clone();
            let mut rack = inst.rack.clone();
            let (z_idx, z_id, mod_id) = if let Some(idx) = *zone_index {
                ensure!(
                    idx < rack.zones.len(),
                    "Zone index z{} out of range on '{inst_name}'",
                    idx + 1
                );
                let mid = Self::ensure_zone_sampler_module(&mut rack.zones[idx], None)?;
                let fallback = rack.zones[idx].root_midi;
                rack.zones[idx].apply_sample_metadata(
                    Some(mid),
                    spec.relative_path.clone(),
                    &sample.meta,
                    sample.frames,
                    fallback,
                );
                if let Some(m) = rack.zones[idx].module_mut(mid) {
                    if let ModuleKind::Sampler(s) = &mut m.kind {
                        s.one_shot = true;
                    }
                }
                for m in &mut rack.zones[idx].modules {
                    if let ModuleKind::Env(e) = &mut m.kind {
                        e.attack_ms = 0.5;
                        break;
                    }
                }
                if let Some(custom) = zone_name.as_deref().filter(|s| !s.trim().is_empty()) {
                    rack.zones[idx].name = custom.trim().chars().take(32).collect();
                }
                let zid = rack.zones[idx].id;
                (idx, zid, mid)
            } else {
                ensure!(
                    rack.zones.len() < MAX_ZONES,
                    "Rack '{inst_name}' already has the maximum {MAX_ZONES} zones"
                );
                let note = midi.unwrap_or(60).min(127);
                let zname = zone_name
                    .clone()
                    .unwrap_or_else(|| format!("Take {}", note_name(note)));
                let zid = rack.next_zone_id();
                let mut zone = RackZone::from_sample_metadata(
                    zid,
                    zname,
                    spec.relative_path.clone(),
                    &sample.meta,
                    sample.frames,
                    note,
                );
                zone.low_midi = note;
                zone.high_midi = note;
                zone.root_midi = note;
                let mid = Self::ensure_zone_sampler_module(&mut zone, None)?;
                if let Some(m) = zone.module_mut(mid) {
                    if let ModuleKind::Sampler(s) = &mut m.kind {
                        s.one_shot = true;
                    }
                }
                for m in &mut zone.modules {
                    if let ModuleKind::Env(e) = &mut m.kind {
                        e.attack_ms = 0.5;
                        break;
                    }
                }
                rack.zones.push(zone);
                (rack.zones.len() - 1, zid, mid)
            };
            let zlabel = rack.zones[z_idx].name.clone();
            let mut out = self.commit(
                vec![Edit::UpdateRackPatch { id: *inst_id, rack }],
                spec.status_summary.clone(),
            )?;
            self.assets
                .samples
                .insert(module_sample_key(*inst_id, z_id, mod_id), sample);
            self.assets.issues.retain(|issue| {
                !(issue.instrument_id == *inst_id
                    && issue.instrument_name == format!("{inst_name} [{zlabel}]"))
            });
            self.rack_ui.zone_cursor = z_idx;
            self.open_instrument(*inst_id)?;
            self.status = spec.status_summary;
            out.changed = true;
            out.samples_changed = true;
            return Ok(out);
        }

        let mut rack = RackPatch::default_sampler();
        let (z_id, mod_id) = if let Some(zone) = rack.zones.first_mut() {
            zone.root_midi = 60;
            let mid = Self::ensure_zone_sampler_module(zone, None)?;
            if let Some(m) = zone.module_mut(mid) {
                if let ModuleKind::Sampler(s) = &mut m.kind {
                    s.path = Some(spec.relative_path.clone());
                    s.sample_start = 0;
                    s.sample_end = sample.frames;
                    s.loop_mode = SampleLoopMode::Off;
                    s.loop_start = 0;
                    s.loop_end = sample.frames;
                    s.one_shot = true;
                }
            }
            for m in &mut zone.modules {
                if let ModuleKind::Env(e) = &mut m.kind {
                    e.attack_ms = 0.5;
                    break;
                }
            }
            (zone.id, mid)
        } else {
            (1, 1)
        };

        let want_part = spec.create_part || spec.place_in_arrange_at.is_some();
        let want_placement = spec.place_in_arrange_at.is_some();
        // When placing an arranged recording, prefer a dedicated lane so it never overlaps
        // existing lane clips (falling back to an existing lane only if at MAX_LANES).
        let target_existing_lane = if want_placement {
            if self.doc.project().lanes.len() < material::MAX_LANES {
                None
            } else {
                spec.source_lane
                    .and_then(|id| self.lane(id))
                    .filter(|l| l.parts.len() < material::MAX_LANE_PARTS)
                    .map(|l| l.id)
            }
        } else {
            spec.source_lane
                .and_then(|id| self.lane(id))
                .filter(|l| l.parts.len() < material::MAX_LANE_PARTS)
                .map(|l| l.id)
        };
        let need_new_lane = want_part
            && target_existing_lane.is_none()
            && self.doc.project().lanes.len() < material::MAX_LANES;
        let id_count = 2
            + if want_part { 2 } else { 0 }
            + usize::from(need_new_lane)
            + usize::from(want_placement);
        let ids = self.doc.fresh_ids(id_count)?;
        let ch_id = ids[0];
        let inst_id = ids[1];

        let mut edits = Vec::new();
        let length_steps = (spec.steps.max(1)).min(material::MAX_LITERAL_STEPS as u64) as u32;
        if let Some(start_step) = spec.place_in_arrange_at {
            let needed_bars = ((u64::from(start_step) + u64::from(length_steps)).div_ceil(16)
                as usize)
                .min(material::MAX_BARS);
            if needed_bars > self.doc.project().bars {
                edits.push(Edit::SetBars { bars: needed_bars });
            }
        }

        edits.push(Edit::CreateChannel {
            channel: Channel::new(ch_id, ChannelKind::Audio, spec.instrument_name.clone()),
        });
        edits.push(Edit::CreateInstrument {
            instrument: InstrumentInstance {
                id: inst_id,
                name: spec.instrument_name.clone(),
                rack,
                channel: ch_id,
            },
        });

        let mut created_part: Option<(Id, Id)> = None;
        let mut created_lane: Option<Id> = None;
        let mut created_placement: Option<Id> = None;
        if want_part {
            let mut cursor = 2usize;
            let pat_id = ids[cursor];
            cursor += 1;
            let part_id = ids[cursor];
            cursor += 1;
            let pat_len = (length_steps as usize).clamp(1, material::MAX_LITERAL_STEPS);
            let (degree, accidental) = self.doc.project().midi_to_degree(60);
            let mut steps = vec![Step::default(); pat_len];
            steps[0] = Step {
                active: true,
                degree,
                accidental,
                velocity: 1.0,
                gate: (length_steps as f32).clamp(1.0, 64.0),
                ..Step::default()
            };
            edits.push(Edit::PutPattern {
                pattern: Pattern {
                    id: pat_id,
                    name: format!("{} Trig", spec.instrument_name),
                    kind: PatternKind::Notes { steps },
                },
            });
            edits.push(Edit::PutPart {
                part: Part {
                    id: part_id,
                    name: spec.instrument_name.clone(),
                    pattern: pat_id,
                    instrument: inst_id,
                    transpose: 0,
                    velocity_scale: 1.0,
                    parameter_links: Vec::new(),
                },
            });
            let lane_entry = LanePart {
                part: part_id,
                phase: PhaseMode::Restart,
                offset_steps: 0,
            };
            let placement = spec.place_in_arrange_at.map(|start_step| {
                let placement_id = ids[cursor];
                cursor += 1;
                created_placement = Some(placement_id);
                Placement {
                    id: placement_id,
                    part: part_id,
                    start_step,
                    length_steps,
                    phase: PhaseMode::Restart,
                    offset_steps: 0,
                    exceptions: Vec::new(),
                }
            });
            if let Some(existing_id) = target_existing_lane {
                if let Some(mut lane) = self.lane(existing_id).cloned() {
                    lane.parts.push(lane_entry);
                    if let Some(p) = placement {
                        lane.placements.push(p);
                    }
                    created_lane = Some(lane.id);
                    edits.push(Edit::PutLane { lane });
                }
            } else if need_new_lane {
                let lane_id = ids[cursor];
                created_lane = Some(lane_id);
                edits.push(Edit::PutLane {
                    lane: Lane {
                        id: lane_id,
                        name: spec.instrument_name.clone(),
                        parts: vec![lane_entry],
                        placements: placement.into_iter().collect(),
                    },
                });
            }
            created_part = Some((pat_id, part_id));
        }

        let mut out = self.commit(edits, spec.status_summary.clone())?;
        self.assets
            .samples
            .insert(module_sample_key(inst_id, z_id, mod_id), sample);
        if let Some((pat_id, part_id)) = created_part {
            self.selected_pattern = Some(pat_id);
            self.selected_part = Some(part_id);
        }
        if let Some(lane_id) = created_lane {
            self.selected_lane = Some(lane_id);
        }
        if let Some(placement_id) = created_placement {
            self.selected_placement = Some(placement_id);
        }
        self.open_instrument(inst_id)?;
        self.status = spec.status_summary;
        out.changed = true;
        out.samples_changed = true;
        Ok(out)
    }
}
