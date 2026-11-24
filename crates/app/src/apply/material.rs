//! Material, arrangement, modulation, and ducking action handlers.
use crate::*;
use std::collections::HashSet;

pub(super) struct PlacedPartSpec {
    pub name: String,
    pub sound: workflow::SoundChoice,
    pub notes: Vec<Step>,
    pub start_step: u32,
    pub length_steps: Option<u32>,
    pub transpose: Option<i8>,
    pub velocity_scale: Option<f32>,
}

fn validate_part_scalars(transpose: Option<i8>, velocity_scale: Option<f32>) -> Result<(i16, f32)> {
    let transpose = i16::from(transpose.unwrap_or(0));
    ensure!(
        (-96..=96).contains(&transpose),
        "Part transpose must be -96..=96 semitones"
    );
    let velocity_scale = velocity_scale.unwrap_or(1.0);
    ensure!(
        velocity_scale.is_finite() && (0.0..=1.0).contains(&velocity_scale),
        "Part velocity_scale must be 0.0..=1.0"
    );
    Ok((transpose, velocity_scale))
}

impl App {
    pub(super) fn apply_inspect_arrangement_event(
        &self,
        event: Option<EventKey>,
    ) -> Result<Outcome> {
        let key = event
            .or_else(|| self.selected_arrangement_event().map(|n| n.key))
            .context("Select an evaluated note event")?;
        Ok(Outcome {
            inspection: Some(serde_json::to_value(self.inspect_arrangement_key(key)?)?),
            ..Outcome::default()
        })
    }

    pub(super) fn apply_select_lane(&mut self, lane: Id) -> Result<Outcome> {
        ensure!(self.lane(lane).is_some(), "Lane missing");
        self.suspend_follow();
        self.push_back();
        self.selected_lane = Some(lane);
        self.show_workspace(Workspace::Arrange);
        self.select_at_cursor();
        Ok(Outcome::default())
    }

    pub(super) fn apply_create_pattern(
        &mut self,
        name: String,
        kind: PatternKind,
    ) -> Result<Outcome> {
        let id = self.doc.fresh_ids(1)?[0];
        let out = self.commit(
            vec![Edit::PutPattern {
                pattern: Pattern { id, name, kind },
            }],
            format!("Created pattern {id}"),
        )?;
        self.select_pattern(id)?;
        Ok(out)
    }

    pub(super) fn apply_create_lane(&mut self, name: String) -> Result<Outcome> {
        let id = self.doc.fresh_ids(1)?[0];
        let out = self.commit(
            vec![Edit::PutLane {
                lane: Lane {
                    id,
                    name,
                    parts: Vec::new(),
                    placements: Vec::new(),
                },
            }],
            format!("Created lane {id}"),
        )?;
        self.selected_lane = Some(id);
        self.show_workspace(Workspace::Arrange);
        self.select_at_cursor();
        Ok(out)
    }

    pub(super) fn apply_create_part(
        &mut self,
        name: String,
        pattern: Id,
        instrument: Id,
        transpose: Option<i8>,
        velocity_scale: Option<f32>,
    ) -> Result<Outcome> {
        let (transpose, velocity_scale) = validate_part_scalars(transpose, velocity_scale)?;
        let id = self.doc.fresh_ids(1)?[0];
        let out = self.commit(
            vec![Edit::PutPart {
                part: Part {
                    id,
                    name,
                    pattern,
                    instrument,
                    transpose,
                    velocity_scale,
                    parameter_links: Vec::new(),
                },
            }],
            format!("Created part {id}"),
        )?;
        self.push_back();
        self.selected_part = Some(id);
        self.selected_pattern = Some(pattern);
        self.selected_instrument = Some(instrument);
        self.show_detail(Detail::Part(id));
        Ok(out)
    }

    pub(super) fn apply_place(
        &mut self,
        part: Id,
        lane: Id,
        start_step: u32,
        length_steps: u32,
        phase: PhaseMode,
        offset_steps: i32,
    ) -> Result<Outcome> {
        let id = self.doc.fresh_ids(1)?[0];
        let out = self.commit(
            vec![Edit::PutPlacement {
                lane,
                placement: Placement {
                    id,
                    part,
                    start_step,
                    length_steps,
                    phase,
                    offset_steps,
                    exceptions: Vec::new(),
                },
            }],
            format!("Placed part at bar {}", start_step / 16 + 1),
        )?;
        self.show_workspace(Workspace::Arrange);
        self.select_placement(id)?;
        Ok(out)
    }

    pub(super) fn apply_placed_part(&mut self, spec: PlacedPartSpec) -> Result<Outcome> {
        let PlacedPartSpec {
            name,
            sound,
            notes,
            start_step,
            length_steps,
            transpose,
            velocity_scale,
        } = spec;
        let (transpose, velocity_scale) = validate_part_scalars(transpose, velocity_scale)?;
        let rack = self.materialize_sound_choice(&sound, false)?;
        let remaining = (self.doc.project().bars as u32 * 16)
            .checked_sub(start_step)
            .context("Placement starts beyond the song")?;
        let length_steps = length_steps.unwrap_or(remaining);
        let ids = self.doc.fresh_ids(6)?;
        let (pattern, lane, instrument, channel, part, placement) =
            (ids[0], ids[1], ids[2], ids[3], ids[4], ids[5]);
        let edits = vec![
            Edit::PutPattern {
                pattern: Pattern {
                    id: pattern,
                    name: format!("{name} motif"),
                    kind: PatternKind::Notes { steps: notes },
                },
            },
            Edit::CreateChannel {
                channel: Channel::new(channel, ChannelKind::Audio, name.clone()),
            },
            Edit::CreateInstrument {
                instrument: InstrumentInstance {
                    id: instrument,
                    name: format!("{name} sound"),
                    rack,
                    channel,
                },
            },
            Edit::PutPart {
                part: Part {
                    id: part,
                    name: name.clone(),
                    pattern,
                    instrument,
                    transpose,
                    velocity_scale,
                    parameter_links: Vec::new(),
                },
            },
            Edit::PutLane {
                lane: Lane {
                    id: lane,
                    name: name.clone(),
                    parts: Vec::new(),
                    placements: Vec::new(),
                },
            },
            Edit::PutPlacement {
                lane,
                placement: Placement {
                    id: placement,
                    part,
                    start_step,
                    length_steps,
                    phase: PhaseMode::Song,
                    offset_steps: 0,
                    exceptions: Vec::new(),
                },
            },
        ];
        let out = self.commit(
            edits,
            format!("Created part {name} · one transaction · Space plays"),
        )?;
        self.show_workspace(Workspace::Arrange);
        self.select_placement(placement)?;
        Ok(out)
    }

    pub(super) fn apply_fork(&mut self, source: Id, name: String) -> Result<Outcome> {
        let id = self.doc.fresh_ids(1)?[0];
        let out = self.commit(
            vec![Edit::ForkPattern { source, id, name }],
            "Forked definition; referenced inputs remain shared".into(),
        )?;
        self.select_pattern(id)?;
        Ok(out)
    }

    pub(super) fn apply_freeze(
        &mut self,
        source: Id,
        name: String,
        start: i64,
        length: usize,
    ) -> Result<Outcome> {
        let id = self.doc.fresh_ids(1)?[0];
        let out = self.commit(
            vec![Edit::FreezePattern {
                source,
                id,
                name,
                start,
                length,
            }],
            "Froze finite range; generating dependencies removed".into(),
        )?;
        self.select_pattern(id)?;
        Ok(out)
    }

    pub(super) fn apply_create_section(
        &mut self,
        name: String,
        start_step: u32,
        length_steps: u32,
    ) -> Result<Outcome> {
        let id = self.doc.fresh_ids(1)?[0];
        self.commit(
            vec![Edit::PutSection {
                section: Section {
                    id,
                    name,
                    start_step,
                    length_steps,
                },
            }],
            "Created named section".into(),
        )
    }

    pub(super) fn apply_create_modulation(
        &mut self,
        name: String,
        target: ModulationTarget,
        pattern: Id,
    ) -> Result<Outcome> {
        let id = self.doc.fresh_ids(1)?[0];
        self.commit(
            vec![Edit::PutModulation {
                modulation: Modulation {
                    id,
                    name,
                    target,
                    pattern,
                },
            }],
            "Created retained control relationship".into(),
        )
    }

    pub(super) fn apply_create_duck(
        &mut self,
        name: String,
        source_part: Id,
        target: ChannelRef,
        depth: f32,
        recovery_ms: f32,
    ) -> Result<Outcome> {
        let id = self.doc.fresh_ids(1)?[0];
        self.commit(
            vec![Edit::PutDuck {
                duck: Duck {
                    id,
                    name,
                    source_part,
                    target,
                    depth,
                    recovery_ms,
                },
            }],
            "Created duck; channel trim remains independent".into(),
        )
    }

    pub(super) fn apply_rename_pattern(&mut self, pattern: Id, name: String) -> Result<Outcome> {
        let clean = crate::drafts::form::validate_printable_name("Pattern", &name, 120)?;
        let mut pat = self.pattern(pattern).context("Pattern missing")?.clone();
        ensure!(
            !self
                .doc
                .project()
                .patterns
                .iter()
                .any(|p| p.id != pattern && p.name.eq_ignore_ascii_case(&clean)),
            "Pattern '{clean}' already exists"
        );
        pat.name = clean.clone();
        self.commit(
            vec![Edit::PutPattern { pattern: pat }],
            format!("Renamed pattern to '{clean}'"),
        )
    }

    pub(super) fn apply_rename_part(&mut self, part: Id, name: String) -> Result<Outcome> {
        let clean = crate::drafts::form::validate_printable_name("Part", &name, 120)?;
        let mut target = self.part(part).context("Part missing")?.clone();
        ensure!(
            !self
                .doc
                .project()
                .parts
                .iter()
                .any(|p| p.id != part && p.name.eq_ignore_ascii_case(&clean)),
            "Part '{clean}' already exists"
        );
        target.name = clean.clone();
        self.commit(
            vec![Edit::PutPart { part: target }],
            format!("Renamed part to '{clean}'"),
        )
    }

    pub(super) fn apply_rename_lane(&mut self, lane: Id, name: String) -> Result<Outcome> {
        let clean = crate::drafts::form::validate_printable_name("Lane", &name, 120)?;
        let mut target = self.lane(lane).context("Lane missing")?.clone();
        ensure!(
            !self
                .doc
                .project()
                .lanes
                .iter()
                .any(|l| l.id != lane && l.name.eq_ignore_ascii_case(&clean)),
            "Lane '{clean}' already exists"
        );
        target.name = clean.clone();
        self.commit(
            vec![Edit::PutLane { lane: target }],
            format!("Renamed lane to '{clean}'"),
        )
    }

    pub(super) fn apply_reorder_lane(&mut self, lane: Id, target_index: usize) -> Result<Outcome> {
        let lane_name = self.lane(lane).context("Lane missing")?.name.clone();
        let out = self.commit(
            vec![Edit::ReorderLane { lane, target_index }],
            format!("Moved lane '{lane_name}' to row {}", target_index + 1),
        )?;
        self.selected_lane = Some(lane);
        if let Some(idx) = self.doc.project().lanes.iter().position(|l| l.id == lane) {
            self.view_state.bank_lane_idx = idx;
        }
        Ok(out)
    }

    pub(crate) fn pattern_delete_summary(&self, id: Id) -> Result<String> {
        let project = self.doc.project();
        let pat = project.pattern(id).context("Pattern missing")?;
        let (to_delete, dep_parts, dep_placements, dep_mods, dep_links, dep_filters, resets_cycle) =
            collect_pattern_delete_impact(project, id);
        let derived = to_delete.len().saturating_sub(1);
        let mut notes = Vec::new();
        if derived > 0 {
            notes.push(format!("{derived} derived"));
        }
        if !dep_parts.is_empty() {
            notes.push(format!(
                "{} part{}",
                dep_parts.len(),
                if dep_parts.len() == 1 { "" } else { "s" }
            ));
        }
        if dep_placements > 0 {
            notes.push(format!(
                "{dep_placements} placement{}",
                if dep_placements == 1 { "" } else { "s" }
            ));
        }
        if dep_mods > 0 {
            notes.push(format!(
                "{dep_mods} mod{}",
                if dep_mods == 1 { "" } else { "s" }
            ));
        }
        if dep_links > 0 {
            notes.push(format!(
                "{dep_links} link{}",
                if dep_links == 1 { "" } else { "s" }
            ));
        }
        if dep_filters > 0 {
            notes.push(format!(
                "{dep_filters} filter groove{}",
                if dep_filters == 1 { "" } else { "s" }
            ));
        }
        if resets_cycle {
            notes.push("cycle timing".into());
        }
        if notes.is_empty() {
            Ok(format!("Delete pattern '{}'?", pat.name))
        } else {
            Ok(format!(
                "Delete pattern '{}' (removes/clears {})?",
                pat.name,
                notes.join(", ")
            ))
        }
    }

    pub(crate) fn request_delete_pattern(&mut self, id: Id) -> Result<Outcome> {
        let msg = self.pattern_delete_summary(id)?;
        self.input.mode = Mode::Confirm(ConfirmAction::DeletePattern(id));
        self.status = format!("{msg} y confirm · Esc cancel");
        Ok(Outcome::default())
    }

    pub(crate) fn delete_pattern_safe(&mut self, id: Id) -> Result<Outcome> {
        let project = self.doc.project();
        let pat_name = project.pattern(id).context("Pattern missing")?.name.clone();
        let (to_delete, dep_parts, _, _, _, _, resets_cycle) =
            collect_pattern_delete_impact(project, id);
        let del_pats: HashSet<Id> = to_delete.iter().copied().collect();
        let del_parts: HashSet<Id> = dep_parts.iter().copied().collect();

        let mut edits = Vec::new();
        for duck in &project.ducks {
            if del_parts.contains(&duck.source_part) {
                edits.push(Edit::RemoveDuck { id: duck.id });
            }
        }
        for lane in &project.lanes {
            for pl in &lane.placements {
                if del_parts.contains(&pl.part) {
                    edits.push(Edit::RemovePlacement { id: pl.id });
                }
            }
        }
        for m in &project.modulations {
            if del_pats.contains(&m.pattern) {
                edits.push(Edit::RemoveModulation { id: m.id });
            }
        }
        for dev in project
            .mixer
            .master
            .inserts
            .iter()
            .chain(project.mixer.channels.iter().flat_map(|c| c.inserts.iter()))
        {
            if let cadence_core::mixer::EffectKind::Filter(mut patch) = dev.kind {
                if patch
                    .env_pattern_id
                    .is_some_and(|env_id| del_pats.contains(&env_id))
                {
                    patch.env_pattern_id = None;
                    edits.push(Edit::UpdateEffect {
                        device: dev.id,
                        name: None,
                        bypassed: None,
                        kind: Some(cadence_core::mixer::EffectKind::Filter(patch)),
                    });
                }
            }
        }
        for part in &project.parts {
            if del_parts.contains(&part.id) {
                continue;
            }
            if part
                .parameter_links
                .iter()
                .any(|link| del_pats.contains(&link.pattern))
            {
                let mut updated = part.clone();
                updated
                    .parameter_links
                    .retain(|link| !del_pats.contains(&link.pattern));
                edits.push(Edit::PutPart { part: updated });
            }
        }
        for part_id in &dep_parts {
            edits.push(Edit::RemovePart { id: *part_id });
        }
        if resets_cycle {
            let mut timing = project.timing;
            timing.launch = cadence_core::material::Boundary::Bar;
            edits.push(Edit::SetLiveTiming { timing });
        }
        // Remove dependent derived patterns before their sources.
        for pat_id in to_delete.iter().rev() {
            edits.push(Edit::RemovePattern { id: *pat_id });
        }

        let extra = to_delete.len().saturating_sub(1) + dep_parts.len();
        let label = if extra > 0 {
            format!("Deleted pattern '{pat_name}' (+{extra} dependent items) · u restores")
        } else {
            format!("Deleted pattern '{pat_name}' · u restores")
        };
        let out = self.commit(edits, label)?;
        if self
            .selected_pattern
            .is_some_and(|sid| del_pats.contains(&sid))
        {
            self.selected_pattern = self.doc.project().patterns.first().map(|p| p.id);
        }
        self.normalize_focus();
        Ok(out)
    }

    pub(crate) fn part_delete_summary(&self, id: Id) -> Result<String> {
        let project = self.doc.project();
        let part = project.part(id).context("Part missing")?;
        let placements: usize = project
            .lanes
            .iter()
            .flat_map(|l| &l.placements)
            .filter(|p| p.part == id)
            .count();
        let ducks = project.ducks.iter().filter(|d| d.source_part == id).count();
        let bank_slots = project
            .lanes
            .iter()
            .filter(|l| l.parts.iter().any(|e| e.part == id))
            .count();
        let mut notes = Vec::new();
        if placements > 0 {
            notes.push(format!(
                "{placements} placement{}",
                if placements == 1 { "" } else { "s" }
            ));
        }
        if ducks > 0 {
            notes.push(format!("{ducks} duck{}", if ducks == 1 { "" } else { "s" }));
        }
        if bank_slots > 0 {
            notes.push(format!(
                "{bank_slots} lane bank slot{}",
                if bank_slots == 1 { "" } else { "s" }
            ));
        }
        if notes.is_empty() {
            Ok(format!("Delete part '{}'?", part.name))
        } else {
            Ok(format!(
                "Delete part '{}' (removes {})?",
                part.name,
                notes.join(", ")
            ))
        }
    }

    pub(crate) fn request_delete_part(&mut self, id: Id) -> Result<Outcome> {
        let msg = self.part_delete_summary(id)?;
        self.input.mode = Mode::Confirm(ConfirmAction::DeletePart(id));
        self.status = format!("{msg} y confirm · Esc cancel");
        Ok(Outcome::default())
    }

    pub(crate) fn delete_part_safe(&mut self, id: Id) -> Result<Outcome> {
        let project = self.doc.project();
        let part_name = project.part(id).context("Part missing")?.name.clone();
        let mut edits = Vec::new();
        for duck in &project.ducks {
            if duck.source_part == id {
                edits.push(Edit::RemoveDuck { id: duck.id });
            }
        }
        for lane in &project.lanes {
            for pl in &lane.placements {
                if pl.part == id {
                    edits.push(Edit::RemovePlacement { id: pl.id });
                }
            }
        }
        edits.push(Edit::RemovePart { id });
        let out = self.commit(edits, format!("Deleted part '{part_name}' · u restores"))?;
        if self.detail == Detail::Part(id) {
            self.detail = Detail::None;
        }
        self.normalize_focus();
        Ok(out)
    }

    pub(crate) fn lane_delete_summary(&self, id: Id) -> Result<String> {
        let lane = self.lane(id).context("Lane missing")?;
        let placements = lane.placements.len();
        let bank_slots = lane.parts.len();
        let mut notes = Vec::new();
        if placements > 0 {
            notes.push(format!(
                "{placements} placement{}",
                if placements == 1 { "" } else { "s" }
            ));
        }
        if bank_slots > 0 {
            notes.push(format!(
                "{bank_slots} bank slot{}",
                if bank_slots == 1 { "" } else { "s" }
            ));
        }
        if notes.is_empty() {
            Ok(format!("Delete lane '{}'?", lane.name))
        } else {
            Ok(format!(
                "Delete lane '{}' (removes {})?",
                lane.name,
                notes.join(", ")
            ))
        }
    }

    pub(crate) fn request_delete_lane(&mut self, id: Id) -> Result<Outcome> {
        let msg = self.lane_delete_summary(id)?;
        self.input.mode = Mode::Confirm(ConfirmAction::DeleteLane(id));
        self.status = format!("{msg} y confirm · Esc cancel");
        Ok(Outcome::default())
    }

    pub(crate) fn delete_lane_safe(&mut self, id: Id) -> Result<Outcome> {
        let lane_name = self.lane(id).context("Lane missing")?.name.clone();
        let old_idx = self
            .doc
            .project()
            .lanes
            .iter()
            .position(|l| l.id == id)
            .unwrap_or(0);
        let out = self.commit(
            vec![Edit::RemoveLane { id }],
            format!("Deleted lane '{lane_name}' · u restores"),
        )?;
        let lanes = &self.doc.project().lanes;
        if lanes.is_empty() {
            self.selected_lane = None;
            self.view_state.bank_lane_idx = 0;
        } else {
            let next_idx = old_idx.min(lanes.len() - 1);
            self.selected_lane = Some(lanes[next_idx].id);
            self.view_state.bank_lane_idx = next_idx;
        }
        self.normalize_focus();
        self.select_at_cursor();
        Ok(out)
    }
}

/// Computes transitive deletion impact for removing pattern `root_id`:
/// returns `(ordered_patterns_to_delete, dependent_parts, placements_count, mods_count, links_count, filters_count, resets_cycle)`.
#[allow(clippy::type_complexity)]
fn collect_pattern_delete_impact(
    project: &Project,
    root_id: Id,
) -> (Vec<Id>, Vec<Id>, usize, usize, usize, usize, bool) {
    let mut ordered = vec![root_id];
    let mut visited: HashSet<Id> = HashSet::from([root_id]);
    let mut idx = 0;
    while idx < ordered.len() {
        let cur = ordered[idx];
        idx += 1;
        for pat in &project.patterns {
            if !visited.contains(&pat.id) && pat.kind.sources().contains(&cur) {
                visited.insert(pat.id);
                ordered.push(pat.id);
            }
        }
    }
    let dep_parts: Vec<Id> = project
        .parts
        .iter()
        .filter(|part| visited.contains(&part.pattern))
        .map(|part| part.id)
        .collect();
    let del_parts: HashSet<Id> = dep_parts.iter().copied().collect();
    let dep_placements = project
        .lanes
        .iter()
        .flat_map(|l| &l.placements)
        .filter(|pl| del_parts.contains(&pl.part))
        .count();
    let dep_mods = project
        .modulations
        .iter()
        .filter(|m| visited.contains(&m.pattern))
        .count();
    let dep_links: usize = project
        .parts
        .iter()
        .filter(|part| !del_parts.contains(&part.id))
        .map(|part| {
            part.parameter_links
                .iter()
                .filter(|link| visited.contains(&link.pattern))
                .count()
        })
        .sum();
    let dep_filters = project
        .mixer
        .master
        .inserts
        .iter()
        .chain(project.mixer.channels.iter().flat_map(|c| c.inserts.iter()))
        .filter(|dev| {
            matches!(
                dev.kind,
                cadence_core::mixer::EffectKind::Filter(patch)
                    if patch.env_pattern_id.is_some_and(|env_id| visited.contains(&env_id))
            )
        })
        .count();
    let resets_cycle = matches!(
        project.timing.launch,
        cadence_core::material::Boundary::Cycle { pattern } if visited.contains(&pattern)
    );
    (
        ordered,
        dep_parts,
        dep_placements,
        dep_mods,
        dep_links,
        dep_filters,
        resets_cycle,
    )
}
