//! Modal object workflows. All pending objects are materialized in one transaction.
use super::form::{DraftCandidate, DraftField, DraftFieldView, DraftInput, DraftView};
use super::pattern::Draft;
use super::plan::{
    ChannelChoice, InstrumentChoice, InstrumentSpec, LaneChoice, ModPatternChoice, PartChoice,
    PartSpec, PatternChoice, PlacementSpec, SoundChoice, WorkflowContext, WorkflowKind,
    WorkflowOutcome, WorkflowPlan,
};
use super::workflow::WorkflowDraft;
use crate::*;

const CREATE_CHOICES: &[(char, &str, &str)] = &[
    ('n', "Notes", "Literal note/chord source"),
    ('r', "Rhythm", "Trigger source"),
    ('d', "Degrees", "Held harmonic offsets"),
    ('v', "Values", "Normalized control points"),
    ('s', "Seeded values", "Deterministic normalized variation"),
    ('g', "Seeded degrees", "Deterministic scale-degree offsets"),
    ('m', "Mini notation", "Strudel/Tidal inline mini-notation"),
    ('e', "Euclidean", "Bjorklund rhythm E(pulses, steps, rot)"),
    ('a', "Derive pattern", "All 16 pattern-algebra operators"),
    ('i', "Instrument", "Synth, Sampler, Kit, or Modular Rack"),
    ('b', "Part", "Material → instrument"),
    ('l', "Lane", "Arrangement lane"),
    ('o', "Placement", "Schedule a part"),
    ('p', "Placed part", "Material + sound + lane + placement"),
    ('t', "Section", "Named timeline section (1–0 jump)"),
    ('c', "Channel", "Audio, Group, or Return mixer bus"),
    ('f', "Effect", "Insert any of the 8 studio FX devices"),
    ('w', "Send", "Route channel send to a Return bus"),
    ('u', "Duck", "Triggered sidechain duck envelope"),
    ('y', "Modulation", "Modulate Rack, FX, Channel, or Macro"),
];

#[derive(Default)]
struct CommittedSelection {
    pattern: Option<Id>,
    new_pattern: bool,
    instrument: Option<Id>,
    part: Option<Id>,
    lane: Option<Id>,
    placement: Option<Id>,
    channel: Option<ChannelRef>,
    section_start: Option<u32>,
}

struct PlanBuilder {
    ids: std::vec::IntoIter<Id>,
    edits: Vec<Edit>,
    prepared_rack: Option<RackPatch>,
    selected: CommittedSelection,
}

impl PlanBuilder {
    fn id(&mut self) -> Id {
        self.ids.next().expect("workflow ID count matches plan")
    }

    fn instrument(&mut self, spec: &InstrumentSpec) -> Id {
        let channel = match &spec.channel {
            ChannelChoice::Existing(id) => *id,
            ChannelChoice::New { name } => {
                let id = self.id();
                self.edits.push(Edit::CreateChannel {
                    channel: Channel::new(id, ChannelKind::Audio, name.clone()),
                });
                id
            }
        };
        let rack = self
            .prepared_rack
            .take()
            .expect("instrument rack prepared before IDs");
        let id = self.id();
        self.edits.push(Edit::CreateInstrument {
            instrument: InstrumentInstance {
                id,
                name: spec.name.clone(),
                rack,
                channel,
            },
        });
        self.selected.instrument = Some(id);
        id
    }

    fn part(&mut self, spec: &PartSpec) -> Id {
        let pattern = match &spec.pattern {
            PatternChoice::Existing(id) => *id,
            PatternChoice::NewNotes { name, length } => {
                let id = self.id();
                self.edits.push(Edit::PutPattern {
                    pattern: Pattern {
                        id,
                        name: name.clone(),
                        kind: PatternKind::Notes {
                            steps: vec![Step::default(); *length],
                        },
                    },
                });
                self.selected.new_pattern = true;
                id
            }
        };
        let instrument = match &spec.instrument {
            InstrumentChoice::Existing(id) => *id,
            InstrumentChoice::New(spec) => self.instrument(spec),
        };
        let id = self.id();
        self.edits.push(Edit::PutPart {
            part: Part {
                id,
                name: spec.name.clone(),
                pattern,
                instrument,
                transpose: spec.transpose,
                velocity_scale: spec.velocity_scale,
                parameter_links: Vec::new(),
            },
        });
        self.selected.pattern = Some(pattern);
        self.selected.instrument = Some(instrument);
        self.selected.part = Some(id);
        id
    }

    fn placement(&mut self, spec: &PlacementSpec) {
        let part = match &spec.part {
            PartChoice::Existing(id) => *id,
            PartChoice::New(spec) => self.part(spec),
        };
        let lane = match &spec.lane {
            LaneChoice::Existing(id) => *id,
            LaneChoice::New { name } => {
                let id = self.id();
                self.edits.push(Edit::PutLane {
                    lane: Lane {
                        id,
                        name: name.clone(),
                        parts: Vec::new(),
                        placements: Vec::new(),
                    },
                });
                id
            }
        };
        let id = self.id();
        self.edits.push(Edit::PutPlacement {
            lane,
            placement: Placement {
                id,
                part,
                start_step: spec.start_step,
                length_steps: spec.length_steps,
                phase: spec.phase,
                offset_steps: spec.offset_steps,
                exceptions: Vec::new(),
            },
        });
        self.selected.part = Some(part);
        self.selected.lane = Some(lane);
        self.selected.placement = Some(id);
    }
}

fn plan_sound_choice(plan: &WorkflowPlan) -> Option<&SoundChoice> {
    let instrument = match plan {
        WorkflowPlan::Instrument(i) => Some(i),
        WorkflowPlan::Part(b) | WorkflowPlan::BankPart { spec: b, .. } => match &b.instrument {
            InstrumentChoice::New(i) => Some(i),
            _ => None,
        },
        WorkflowPlan::Place(p) | WorkflowPlan::PlacedPart(p) => match &p.part {
            PartChoice::New(b) => match &b.instrument {
                InstrumentChoice::New(i) => Some(i),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    };
    instrument.map(|i| &i.sound)
}

impl App {
    pub(crate) fn workflow_context(&self) -> WorkflowContext {
        if self.graph_focused() {
            if let Some(target) = self.selected_graph_target() {
                let mut ctx = WorkflowContext {
                    pattern: self.selected_pattern,
                    instrument: self.active_instrument(),
                    part: self.selected_part,
                    lane: self.selected_lane,
                    add_part_to_lane: None,
                    channel: None,
                    mixer_channel: None,
                    effect: None,
                    macro_slot: None,
                    mod_target: None,
                    send_target: None,
                    arrange_step: self.arrange_ui.step,
                };
                match target {
                    crate::view::GraphTarget::Pattern(id) => {
                        ctx.pattern = Some(id);
                    }
                    crate::view::GraphTarget::Part(id) => {
                        ctx.part = Some(id);
                        if let Some(part) = self.part(id) {
                            ctx.pattern = Some(part.pattern);
                            ctx.instrument = Some(part.instrument);
                            if let Some(inst) = self.instrument(part.instrument) {
                                ctx.channel = Some(inst.channel);
                            }
                        }
                    }
                    crate::view::GraphTarget::Macro(slot) => {
                        ctx.macro_slot = Some(slot);
                        ctx.instrument = None;
                    }
                    crate::view::GraphTarget::Instrument(id) => {
                        ctx.instrument = Some(id);
                        if let Some(inst) = self.instrument(id) {
                            ctx.channel = Some(inst.channel);
                        }
                    }
                    crate::view::GraphTarget::Channel(ch) => {
                        ctx.instrument = None;
                        ctx.mixer_channel = Some(ch);
                        if let ChannelRef::Channel(id) = ch {
                            if let Some(c) = self.channel(id) {
                                if c.kind == ChannelKind::Audio {
                                    ctx.channel = Some(id);
                                } else if c.kind == ChannelKind::Return {
                                    ctx.send_target = Some(id);
                                }
                            }
                        }
                    }
                    crate::view::GraphTarget::Effect(dev_id) => {
                        ctx.effect = Some(dev_id);
                        ctx.instrument = None;
                        if let Some((owner, _)) = self.effect(dev_id) {
                            ctx.mixer_channel = Some(owner);
                            if let ChannelRef::Channel(id) = owner {
                                if self
                                    .channel(id)
                                    .is_some_and(|c| c.kind == ChannelKind::Audio)
                                {
                                    ctx.channel = Some(id);
                                }
                            }
                        }
                    }
                }
                return ctx;
            }
        }
        let mut mixer_channel = None;
        let mut effect = None;
        let mut mod_target = None;
        let mut send_target = None;
        let mut channel = match self.selected_channel {
            ChannelRef::Channel(id)
                if self
                    .channel(id)
                    .is_some_and(|c| c.kind == ChannelKind::Audio) =>
            {
                Some(id)
            }
            _ => None,
        };
        if self.mixer_canvas_focused() {
            mixer_channel = Some(self.selected_channel);
            match self.view_state.mixer_slot {
                MixerSlot::Fader => {
                    mod_target = Some(ModulationTarget::ChannelGain {
                        channel: self.selected_channel,
                    });
                }
                MixerSlot::Pan => {
                    mod_target = Some(ModulationTarget::ChannelPan {
                        channel: self.selected_channel,
                    });
                }
                MixerSlot::Send(idx) => {
                    if let Some((src, ret)) = self.mixer_focused_send(idx) {
                        send_target = Some(ret);
                        mod_target = Some(ModulationTarget::Send {
                            channel: ChannelRef::Channel(src),
                            target: ret,
                        });
                    }
                }
                MixerSlot::Insert(idx) => {
                    effect = self.mixer_focused_insert(idx);
                }
                MixerSlot::Output | MixerSlot::Duck(_) => {}
            }
        } else if self.deck_focused() {
            match self.detail {
                Detail::Channel(ch) => {
                    mixer_channel = Some(ch);
                    if let Some(ctrl) = self.selected_control() {
                        match ctrl.target {
                            ControlTarget::Param(addr) => {
                                mod_target = self.param_address_to_mod_target(addr);
                                if let ParamAddress::Send { target, .. } = addr {
                                    send_target = Some(target);
                                }
                            }
                            ControlTarget::Effect(dev) => {
                                effect = Some(dev);
                            }
                            _ => {}
                        }
                    }
                }
                Detail::Effect(dev) => {
                    effect = Some(dev);
                    mixer_channel = self.effect(dev).map(|(ch, _)| ch);
                    if let Some(ctrl) = self.selected_control() {
                        if let ControlTarget::Param(addr) = ctrl.target {
                            mod_target = self.param_address_to_mod_target(addr);
                        }
                    }
                }
                Detail::Instrument(inst_id) => {
                    if let Some(inst) = self.instrument(inst_id) {
                        channel = Some(inst.channel);
                    }
                    if self.rack_ui.tab == RackTab::Patchbay {
                        if let (Some(cid), Some(z)) = (
                            self.rack_ui.patchbay_cable_id,
                            self.active_rack_zone(inst_id),
                        ) {
                            mod_target = self.param_address_to_mod_target(ParamAddress::Cable {
                                instrument: inst_id,
                                zone: z.id,
                                cable: cid,
                            });
                        }
                    } else if let Some(ctrl) = self.selected_control() {
                        if let ControlTarget::Param(addr) = ctrl.target {
                            mod_target = self.param_address_to_mod_target(addr);
                        }
                    }
                }
                Detail::Part(part_id) => {
                    if let Some(inst_id) = self.part(part_id).map(|p| p.instrument) {
                        if let Some(inst) = self.instrument(inst_id) {
                            channel = Some(inst.channel);
                        }
                    }
                }
                Detail::Placement(_) | Detail::None => {}
            }
        }
        let macro_slot = self
            .macros_focused()
            .then_some(self.view_state.macro_cursor as u8);
        WorkflowContext {
            pattern: self.selected_pattern,
            instrument: self.active_instrument(),
            part: self.selected_part,
            lane: self.selected_lane,
            add_part_to_lane: None,
            channel,
            mixer_channel,
            effect,
            macro_slot,
            mod_target,
            send_target,
            arrange_step: self.arrange_ui.step,
        }
    }

    pub(crate) fn begin_create(&mut self) -> Result<Outcome> {
        self.clear_entry_state();
        let origin = Some(self.navigation());
        self.modal = ActiveModal::Create { cursor: 0, origin };
        self.input.mode = Mode::Create;
        Ok(self.release_held())
    }

    pub(crate) fn create_view(&self) -> DraftView {
        DraftView {
            title: "NEW · choose an object".into(),
            fields: Vec::<DraftFieldView>::new(),
            field: DraftField::Kind,
            field_label: "Create".into(),
            input: DraftInput::Choice,
            text: String::new(),
            replace_on_type: false,
            query: String::new(),
            searching: false,
            candidates: CREATE_CHOICES
                .iter()
                .map(|&(key, label, detail)| DraftCandidate {
                    id: None,
                    key: Some(key),
                    label: label.into(),
                    detail: detail.into(),
                })
                .collect(),
            selected: Some(self.modal.create_cursor().min(CREATE_CHOICES.len() - 1)),
            summary: "Choose a type; the pending form is cancellable and commits once.".into(),
            preview: Vec::new(),
            error: None,
            hints: vec![
                ("type", "choose shortcut"),
                ("j/k", "select"),
                ("Enter", "open"),
                ("Esc", "cancel"),
            ],
        }
    }

    pub(crate) fn handle_create(&mut self, key: KeyEvent) -> Result<Outcome> {
        if key.kind == KeyEventKind::Repeat {
            return Ok(Outcome::default());
        }
        if key.code == KeyCode::Esc
            || (key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c'))
        {
            self.cancel_workflow_origin();
            return Ok(Outcome::default());
        }
        let chosen = match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if let Some(c) = self.modal.create_cursor_mut() {
                    *c = (*c + 1).min(CREATE_CHOICES.len() - 1);
                }
                None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let Some(c) = self.modal.create_cursor_mut() {
                    *c = c.saturating_sub(1);
                }
                None
            }
            KeyCode::Enter => {
                Some(CREATE_CHOICES[self.modal.create_cursor().min(CREATE_CHOICES.len() - 1)].0)
            }
            KeyCode::Char(c) => {
                let lower = c.to_ascii_lowercase();
                CREATE_CHOICES
                    .iter()
                    .find(|&&(k, _, _)| k == lower)
                    .map(|&(k, _, _)| k)
            }
            _ => None,
        };
        if let Some(choice) = chosen {
            let origin = self.modal.take_origin();
            if let Some(draft) = Draft::new_pattern_for_key(choice, self.doc.project()) {
                self.modal = ActiveModal::Authoring {
                    draft: Box::new(draft),
                    origin,
                };
                self.input.mode = Mode::Authoring;
            } else if choice == 'a' {
                self.modal = ActiveModal::Authoring {
                    draft: Box::new(Draft::derive(self.selected_pattern)),
                    origin,
                };
                self.input.mode = Mode::Authoring;
            } else {
                let kind = match choice {
                    'i' => WorkflowKind::Instrument,
                    'b' => WorkflowKind::Part,
                    'l' => WorkflowKind::Lane,
                    'o' => WorkflowKind::Place,
                    't' => WorkflowKind::Section,
                    'c' => WorkflowKind::Channel,
                    'f' => WorkflowKind::Effect,
                    'w' => WorkflowKind::Send,
                    'u' => WorkflowKind::Duck,
                    'y' => WorkflowKind::Modulation,
                    _ => WorkflowKind::PlacedPart,
                };
                let draft = WorkflowDraft::new(
                    kind,
                    self.doc.project(),
                    self.workflow_context(),
                    &self.assets.presets,
                );
                self.modal = ActiveModal::Workflow {
                    draft: Box::new(draft),
                    origin,
                };
                self.input.mode = Mode::Workflow;
            }
        }
        Ok(Outcome::default())
    }

    pub(crate) fn begin_workflow(&mut self, kind: WorkflowKind) -> Result<Outcome> {
        let ctx = self.workflow_context();
        self.begin_workflow_with_context(kind, ctx)
    }

    pub(crate) fn begin_workflow_with_context(
        &mut self,
        kind: WorkflowKind,
        ctx: WorkflowContext,
    ) -> Result<Outcome> {
        self.clear_entry_state();
        let origin = Some(self.navigation());
        let draft = WorkflowDraft::new(kind, self.doc.project(), ctx, &self.assets.presets);
        self.modal = ActiveModal::Workflow {
            draft: Box::new(draft),
            origin,
        };
        self.input.mode = Mode::Workflow;
        Ok(self.release_held())
    }

    pub(crate) fn cancel_workflow_origin(&mut self) {
        let origin = self.modal.take_origin();
        self.modal = ActiveModal::None;
        self.input.count = None;
        self.input.mode = Mode::Normal;
        if let Some(origin) = origin {
            self.restore_navigation(origin);
        }
        self.status = "Creation cancelled · musical state unchanged".into();
    }

    pub(crate) fn handle_workflow(&mut self, key: KeyEvent) -> Result<Outcome> {
        let (mut draft, origin) = self.modal.take_workflow().context("Workflow missing")?;
        let outcome = draft.handle_key(self.doc.project(), key);
        self.modal = ActiveModal::Workflow { draft, origin };
        self.apply_workflow_outcome(outcome)
    }

    pub(crate) fn apply_workflow_outcome(&mut self, outcome: WorkflowOutcome) -> Result<Outcome> {
        match outcome {
            WorkflowOutcome::Continue => Ok(Outcome::default()),
            WorkflowOutcome::Cancel => {
                self.cancel_workflow_origin();
                Ok(Outcome {
                    stop_audition: true,
                    ..Outcome::default()
                })
            }
            WorkflowOutcome::ChooseSample => self.begin_sample_browser(BrowserTarget::Workflow),
            WorkflowOutcome::BrowseFolder => {
                self.status = "Opening native folder picker…".into();
                Ok(Outcome {
                    file_dialog: Some(FileDialogRequest::PickWorkflowSampleFolder),
                    ..Outcome::default()
                })
            }
            WorkflowOutcome::AuditionPreset(preset_id) => {
                let rack = preset_id
                    .as_deref()
                    .and_then(|id| self.assets.presets.find(id))
                    .map(|e| e.patch.clone())
                    .unwrap_or_else(RackPatch::default_synth);
                Ok(Outcome {
                    stop_audition: true,
                    rack_audition: Some((rack, self.doc.project().root_midi, 0.8)),
                    ..Outcome::default()
                })
            }
            WorkflowOutcome::Submit(plan) => match self.commit_workflow(&plan) {
                Ok(outcome) => Ok(outcome),
                Err(error) => {
                    let message = format!("{error:#}");
                    if let Some(draft) = self.modal.workflow_mut() {
                        draft.set_error(message.clone());
                    }
                    self.input.mode = Mode::Workflow;
                    self.status = message;
                    Ok(Outcome::default())
                }
            },
        }
    }

    /// Populate `DraftField::SampleFolder` on an active `Mode::Workflow` draft from a native OS
    /// folder dialog selection.
    pub fn apply_workflow_sample_folder(&mut self, path: PathBuf) -> Result<Outcome> {
        ensure!(
            self.input.mode == Mode::Workflow,
            "No active workflow draft awaiting a sample folder"
        );
        let display = path.to_string_lossy().into_owned();
        let draft = self
            .modal
            .workflow_mut()
            .context("Workflow draft missing")?;
        ensure!(
            draft.active_field() == DraftField::SampleFolder,
            "Active workflow field is not SampleFolder"
        );
        draft.set_sample_folder(display.clone());
        self.status = format!("Selected sample folder: {display} · press Enter to import");
        Ok(Outcome::default())
    }

    fn finish_workflow_action(&mut self, mut outcome: Outcome) -> Result<Outcome> {
        if self.is_auditioning() {
            let restored = self.stop_audition()?;
            outcome.merge(restored);
        }
        outcome.stop_audition = true;
        self.input.mode = Mode::Normal;
        self.modal = ActiveModal::None;
        Ok(outcome)
    }

    fn commit_workflow(&mut self, plan: &WorkflowPlan) -> Result<Outcome> {
        plan.validate(self.doc.project())?;
        // Delegate single-action mixer workflows directly to their authoritative SPOT actions.
        match plan {
            WorkflowPlan::RenameInstrument { instrument, name } => {
                if let Some(origin) = self.modal.take_origin() {
                    self.restore_navigation(origin);
                }
                let out = self.apply(Action::RenameInstrument {
                    instrument: *instrument,
                    name: name.clone(),
                })?;
                return self.finish_workflow_action(out);
            }
            WorkflowPlan::RenamePart { part, name } => {
                if let Some(origin) = self.modal.take_origin() {
                    self.restore_navigation(origin);
                }
                let out = self.apply(Action::RenamePart {
                    part: *part,
                    name: name.clone(),
                })?;
                return self.finish_workflow_action(out);
            }
            WorkflowPlan::RenameLane { lane, name } => {
                if let Some(origin) = self.modal.take_origin() {
                    self.restore_navigation(origin);
                }
                let out = self.apply(Action::RenameLane {
                    lane: *lane,
                    name: name.clone(),
                })?;
                return self.finish_workflow_action(out);
            }
            WorkflowPlan::EditLanePart { lane, entry } => {
                if let Some(origin) = self.modal.take_origin() {
                    self.restore_navigation(origin);
                }
                let out = self.apply(Action::AddLanePart {
                    lane: *lane,
                    entry: *entry,
                })?;
                let part_name = self
                    .part(entry.part)
                    .map_or("Part", |p| p.name.as_str())
                    .to_string();
                let lane_name = self
                    .lane(*lane)
                    .map_or("Lane", |l| l.name.as_str())
                    .to_string();
                self.status = format!(
                    "Updated '{part_name}' on '{lane_name}' ({:?}, {:+} steps)",
                    entry.phase, entry.offset_steps
                );
                return self.finish_workflow_action(out);
            }
            WorkflowPlan::FreezePattern {
                pattern,
                name,
                start,
                length,
            } => {
                if let Some(origin) = self.modal.take_origin() {
                    self.restore_navigation(origin);
                }
                let out = self.apply(Action::Freeze {
                    source: *pattern,
                    name: name.clone(),
                    start: *start,
                    length: *length,
                })?;
                return self.finish_workflow_action(out);
            }
            WorkflowPlan::RenameChannel { channel, name } => {
                if let Some(origin) = self.modal.take_origin() {
                    self.restore_navigation(origin);
                }
                let out = self.apply(Action::RenameChannel {
                    channel: *channel,
                    name: name.clone(),
                })?;
                return self.finish_workflow_action(out);
            }
            WorkflowPlan::RenameEffect { device, name } => {
                if let Some(origin) = self.modal.take_origin() {
                    self.restore_navigation(origin);
                }
                let out = self.apply(Action::RenameEffect {
                    device: *device,
                    name: name.clone(),
                })?;
                return self.finish_workflow_action(out);
            }
            WorkflowPlan::RenameMacro { index, name } => {
                if let Some(origin) = self.modal.take_origin() {
                    self.restore_navigation(origin);
                }
                let out = self.apply(Action::RenameMacro {
                    index: *index,
                    name: name.clone(),
                })?;
                return self.finish_workflow_action(out);
            }
            WorkflowPlan::Effect { channel, kind } => {
                if let Some(origin) = self.modal.take_origin() {
                    self.restore_navigation(origin);
                }
                let out = self.apply(Action::AddEffect {
                    channel: *channel,
                    kind: kind.clone(),
                })?;
                return self.finish_workflow_action(out);
            }
            WorkflowPlan::MoveEffect {
                device,
                channel,
                position,
            } => {
                if let Some(origin) = self.modal.take_origin() {
                    self.restore_navigation(origin);
                }
                let out = self.apply(Action::MoveEffect {
                    device: *device,
                    channel: *channel,
                    position: *position,
                })?;
                self.selected_channel = *channel;
                if matches!(self.detail, Detail::Effect(id) if id == *device) {
                    self.show_detail(Detail::Effect(*device));
                }
                return self.finish_workflow_action(out);
            }
            WorkflowPlan::Send {
                source,
                target: ChannelChoice::Existing(ret_id),
                amount,
            } if *amount <= 0.0001 => {
                if let Some(origin) = self.modal.take_origin() {
                    self.restore_navigation(origin);
                }
                let out = self.apply(Action::SetChannelSend {
                    channel: *source,
                    target: *ret_id,
                    amount: None,
                })?;
                return self.finish_workflow_action(out);
            }
            WorkflowPlan::ZoneImportFolder { instrument, folder } => {
                if let Some(origin) = self.modal.take_origin() {
                    self.restore_navigation(origin);
                }
                let out = self.apply_zone_import_folder(Some(*instrument), folder.clone())?;
                return self.finish_workflow_action(out);
            }
            WorkflowPlan::SavePreset {
                instrument,
                name,
                category,
            } => {
                if let Some(origin) = self.modal.take_origin() {
                    self.restore_navigation(origin);
                }
                let slug = crate::bounce::slugify_bounce(name);
                let path = self
                    .doc
                    .asset_base()
                    .join("presets")
                    .join(format!("{slug}.preset.json"));
                self.save_instrument_preset_to_path(
                    *instrument,
                    name,
                    Some(category.clone()),
                    &path,
                )?;
                return self.finish_workflow_action(Outcome::default());
            }
            WorkflowPlan::Bounce {
                scope,
                name,
                create_part,
                target,
            } => {
                if let Some(origin) = self.modal.take_origin() {
                    self.restore_navigation(origin);
                }
                let out = self.apply(Action::Bounce {
                    scope: Some(*scope),
                    name: name.clone(),
                    create_part: *create_part,
                    target: Some(target.clone()),
                })?;
                return self.finish_workflow_action(out);
            }
            WorkflowPlan::Record {
                mode,
                bars,
                name,
                place_in_arrange,
                target,
            } => {
                if let Some(origin) = self.modal.take_origin() {
                    self.restore_navigation(origin);
                }
                let out = self.apply(Action::StartAudioRecord {
                    mode: Some(*mode),
                    target: Some(target.clone()),
                    place_in_arrange: Some(*place_in_arrange),
                    bars: *bars,
                    name: name.clone(),
                })?;
                return self.finish_workflow_action(out);
            }
            _ => {}
        }

        // Prepare and verify exact sample / folder references before allocating authored IDs.
        let prepared_rack = plan_sound_choice(plan)
            .map(|sound| self.materialize_sound_choice(sound, true))
            .transpose()?;
        let count = plan.new_object_count();
        let mut builder = PlanBuilder {
            ids: self.doc.fresh_ids(count)?.into_iter(),
            edits: Vec::new(),
            prepared_rack,
            selected: CommittedSelection::default(),
        };
        let label: String = match plan {
            WorkflowPlan::Instrument(i) => {
                builder.instrument(i);
                "Created workflow objects in one transaction".into()
            }
            WorkflowPlan::Part(b) => {
                builder.part(b);
                "Created workflow objects in one transaction".into()
            }
            WorkflowPlan::BankPart { lane, spec } => {
                let part_id = builder.part(spec);
                builder.edits.push(Edit::AddLanePart {
                    lane: *lane,
                    entry: cadence_core::material::LanePart::new(part_id),
                });
                format!("Created part '{}' and added to launchpad", spec.name)
            }
            WorkflowPlan::Lane { name } => {
                let id = builder.id();
                builder.edits.push(Edit::PutLane {
                    lane: Lane {
                        id,
                        name: name.clone(),
                        parts: Vec::new(),
                        placements: Vec::new(),
                    },
                });
                builder.selected.lane = Some(id);
                "Created workflow objects in one transaction".into()
            }
            WorkflowPlan::Place(p) | WorkflowPlan::PlacedPart(p) => {
                builder.placement(p);
                "Created workflow objects in one transaction".into()
            }
            WorkflowPlan::Section {
                id,
                name,
                start_step,
                length_steps,
            } => {
                let sec_id = id.unwrap_or_else(|| builder.id());
                builder.edits.push(Edit::PutSection {
                    section: Section {
                        id: sec_id,
                        name: name.clone(),
                        start_step: *start_step,
                        length_steps: *length_steps,
                    },
                });
                builder.selected.section_start = Some(*start_step);
                if id.is_some() {
                    format!("Updated section {name}")
                } else {
                    format!("Created section {name}")
                }
            }
            WorkflowPlan::Channel { name, kind, output } => {
                let id = builder.id();
                let mut ch = Channel::new(id, *kind, name.clone());
                ch.output = *output;
                builder.edits.push(Edit::CreateChannel { channel: ch });
                builder.selected.channel = Some(ChannelRef::Channel(id));
                format!("Created {} channel {name}", kind.label())
            }
            WorkflowPlan::Send {
                source,
                target,
                amount,
            } => {
                let ret_id = match target {
                    ChannelChoice::Existing(id) => *id,
                    ChannelChoice::New { name } => {
                        let id = builder.id();
                        builder.edits.push(Edit::CreateChannel {
                            channel: Channel::new(id, ChannelKind::Return, name.clone()),
                        });
                        id
                    }
                };
                let mut sends = self
                    .channel(*source)
                    .context("Source channel missing")?
                    .sends
                    .clone();
                if let Some(existing) = sends.iter_mut().find(|s| s.target == ret_id) {
                    existing.amount = *amount;
                } else {
                    sends.push(ChannelSend {
                        target: ret_id,
                        amount: *amount,
                    });
                }
                builder.edits.push(Edit::UpdateChannelMeta {
                    id: *source,
                    name: None,
                    output: None,
                    sends: Some(sends),
                });
                builder.selected.channel = Some(ChannelRef::Channel(*source));
                "Updated channel send".into()
            }
            WorkflowPlan::Duck {
                id,
                name,
                source_part,
                target,
                depth,
                recovery_ms,
            } => {
                let duck_id = id.unwrap_or_else(|| builder.id());
                builder.edits.push(Edit::PutDuck {
                    duck: Duck {
                        id: duck_id,
                        name: name.clone(),
                        source_part: *source_part,
                        target: *target,
                        depth: *depth,
                        recovery_ms: *recovery_ms,
                    },
                });
                builder.selected.channel = Some(*target);
                if id.is_some() {
                    format!("Updated duck {name}")
                } else {
                    format!("Created duck {name}")
                }
            }
            WorkflowPlan::Modulation {
                id,
                name,
                pattern,
                target,
            } => {
                if let ModulationTarget::Macro { index } = *target {
                    if self.doc.project().macro_control(index).is_none() {
                        builder.edits.push(Edit::PutMacro {
                            macro_control: cadence_core::macros::MacroControl::new(
                                index,
                                cadence_core::macros::MacroControl::default_name(index),
                            ),
                        });
                    }
                }
                let pat_id = match pattern {
                    ModPatternChoice::Existing(pid) => *pid,
                    ModPatternChoice::NewValues { name: pat_name } => {
                        let pid = builder.id();
                        builder.edits.push(Edit::PutPattern {
                            pattern: Pattern {
                                id: pid,
                                name: pat_name.clone(),
                                kind: PatternKind::Values {
                                    unit: ValueUnit::Normalized,
                                    period: 16,
                                    mode: ValueMode::Ramp,
                                    points: vec![ValuePoint {
                                        step: 0,
                                        value: 0.5,
                                    }],
                                },
                            },
                        });
                        pid
                    }
                };
                let mod_id = id.unwrap_or_else(|| builder.id());
                builder.edits.push(Edit::PutModulation {
                    modulation: Modulation {
                        id: mod_id,
                        name: name.clone(),
                        pattern: pat_id,
                        target: *target,
                    },
                });
                if id.is_some() {
                    format!("Updated modulation {name}")
                } else {
                    format!("Created modulation {name}")
                }
            }
            WorkflowPlan::RenameInstrument { .. }
            | WorkflowPlan::RenamePart { .. }
            | WorkflowPlan::RenameLane { .. }
            | WorkflowPlan::EditLanePart { .. }
            | WorkflowPlan::FreezePattern { .. }
            | WorkflowPlan::RenameChannel { .. }
            | WorkflowPlan::RenameEffect { .. }
            | WorkflowPlan::RenameMacro { .. }
            | WorkflowPlan::Effect { .. }
            | WorkflowPlan::MoveEffect { .. }
            | WorkflowPlan::ZoneImportFolder { .. }
            | WorkflowPlan::SavePreset { .. }
            | WorkflowPlan::Bounce { .. }
            | WorkflowPlan::Record { .. } => unreachable!("handled above"),
        };
        let selected = builder.selected;
        let mut outcome = self.commit(builder.edits, label)?;
        if self.is_auditioning() {
            let restored = self.stop_audition()?;
            outcome.merge(restored);
        }
        outcome.stop_audition = true;
        let origin = self.modal.take_origin();
        self.modal = ActiveModal::None;
        self.input.mode = Mode::Normal;
        if let Some(origin) = origin {
            self.restore_navigation(origin);
        }
        let stay_in_graph = self.graph_focused()
            && matches!(
                plan,
                WorkflowPlan::Send { .. }
                    | WorkflowPlan::Duck { .. }
                    | WorkflowPlan::Modulation { .. }
            );
        if stay_in_graph {
            return Ok(outcome);
        }
        if let WorkflowPlan::BankPart { lane, .. } = plan {
            if let Some(part_id) = selected.part {
                if let Some(lane_idx) = self.doc.project().lanes.iter().position(|l| l.id == *lane)
                {
                    self.view_state.bank_lane_idx = lane_idx;
                    if let Some(slot_idx) = self.doc.project().lanes[lane_idx]
                        .parts
                        .iter()
                        .position(|e| e.part == part_id)
                    {
                        self.view_state.bank_part_idx = slot_idx;
                    }
                }
                self.sync_bank_part_selection();
            }
            return Ok(outcome);
        }
        if let Some(id) = selected.placement {
            self.show_workspace(Workspace::Arrange);
            self.select_placement(id)?;
            if selected.new_pattern && matches!(plan, WorkflowPlan::PlacedPart(_)) {
                let pattern = selected.pattern.unwrap();
                self.select_pattern(pattern)?;
                self.input.mode = Mode::Insert;
                self.status = "Part created · enter notes · Esc returns to Normal".into();
            }
        } else if let Some(id) = selected.part {
            let b = self.part(id).context("Created part missing")?.clone();
            self.selected_part = Some(id);
            self.selected_pattern = Some(b.pattern);
            self.selected_instrument = Some(b.instrument);
            self.push_back();
            self.show_detail(Detail::Part(id));
        } else if let Some(id) = selected.instrument {
            self.open_instrument(id)?;
        } else if let Some(id) = selected.lane {
            self.selected_lane = Some(id);
            self.show_workspace(Workspace::Arrange);
            self.select_at_cursor();
        } else if let Some(ch) = selected.channel {
            self.open_channel(ch)?;
        } else if let Some(start_step) = selected.section_start {
            self.arrange_ui.step = start_step;
            self.show_workspace(Workspace::Arrange);
            self.select_at_cursor();
        }
        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::CREATE_CHOICES;
    use std::collections::HashSet;

    #[test]
    fn create_menu_shortcuts_are_unique_lowercase_and_never_collide_with_navigation() {
        const RESERVED: [char; 3] = ['j', 'k', '/'];
        let mut seen = HashSet::new();
        for &(key, label, _) in CREATE_CHOICES {
            let lower = key.to_ascii_lowercase();
            assert_eq!(key, lower, "Create menu '{label}' key must be lowercase");
            assert!(
                !RESERVED.contains(&lower),
                "Create menu '{label}' key '{key}' collides with {RESERVED:?}"
            );
            assert!(
                seen.insert(lower),
                "Create menu has duplicate shortcut '{key}' on '{label}'"
            );
        }
    }
}
