//! Performing: launching parts over the arrangement, live mute and solo,
//! staging, and live edits on the single performance clock.
use super::{Action, App, KeyCode, KeyEvent, Mode, Outcome};
use anyhow::{bail, ensure, Context, Result};
use cadence_core::live::{beats_until, position_label, Change, Landed, StagedChange};
use cadence_core::material::{Boundary, Edit, EditTiming, Id, LanePart, Project};
use cadence_core::mixer::{ChannelRef, LiveFlags};
use cadence_core::runtime::MidiMessage;
use std::sync::Arc;

/// Keys that open each lane's layer in Jam and on Alt, in lane order.
pub const LANE_KEYS: [char; 10] = ['q', 'w', 'e', 'r', 't', 'y', 'u', 'i', 'o', 'p'];
/// Keys that move the song to a section in Jam and on Alt, in section order.
pub const SECTION_KEYS: [char; 10] = ['1', '2', '3', '4', '5', '6', '7', '8', '9', '0'];

impl App {
    /// The ten lanes addressed by Play mode's lane keys.
    pub fn play_bank_lanes(&self) -> Vec<Id> {
        self.doc
            .project()
            .lanes
            .iter()
            .skip(self.clamped_bank() * LANE_KEYS.len())
            .take(LANE_KEYS.len())
            .map(|l| l.id)
            .collect()
    }

    fn edit_boundary(&self) -> Boundary {
        match self.doc.project().timing.edits {
            EditTiming::Now => Boundary::Now,
            EditTiming::Launch => self.doc.project().timing.launch,
        }
    }

    /// Ordered live-launch quantization choices: standard musical grid boundaries plus
    /// `Boundary::Cycle { pattern }` for every pattern in the current project.
    pub(crate) fn live_boundary_choices(&self) -> Vec<Boundary> {
        let mut choices = vec![
            Boundary::Now,
            Boundary::Beat,
            Boundary::Bar,
            Boundary::Bars { count: 2 },
            Boundary::Bars { count: 4 },
            Boundary::Bars { count: 8 },
        ];
        let current = self.doc.project().timing.launch;
        if matches!(current, Boundary::Bars { .. }) && !choices.contains(&current) {
            choices.push(current);
        }
        for pat in &self.doc.project().patterns {
            choices.push(Boundary::Cycle { pattern: pat.id });
        }
        choices
    }

    pub(crate) fn set_launch_boundary(&mut self, boundary: Boundary) -> Result<Outcome> {
        let mut timing = self.doc.project().timing;
        timing.launch = boundary;
        let label = boundary.label(self.doc.project());
        let out = self.apply(Action::Edit {
            edit: Edit::SetLiveTiming { timing },
        })?;
        self.status = format!("Launch quantize → {label}");
        Ok(out)
    }

    pub(crate) fn cycle_launch_boundary(&mut self, delta: isize) -> Result<Outcome> {
        let choices = self.live_boundary_choices();
        let current = self.doc.project().timing.launch;
        let idx = choices.iter().position(|&b| b == current).unwrap_or(0) as isize;
        let next = choices[(idx + delta).rem_euclid(choices.len() as isize) as usize];
        self.set_launch_boundary(next)
    }

    pub(crate) fn toggle_edit_timing(&mut self) -> Result<Outcome> {
        let mut timing = self.doc.project().timing;
        timing.edits = match timing.edits {
            EditTiming::Now => EditTiming::Launch,
            EditTiming::Launch => EditTiming::Now,
        };
        let label = match timing.edits {
            EditTiming::Now => "Now (next step)",
            EditTiming::Launch => "Follow Launch Quantize",
        };
        let out = self.apply(Action::Edit {
            edit: Edit::SetLiveTiming { timing },
        })?;
        self.status = format!("Live edit timing → {label}");
        Ok(out)
    }

    pub(crate) fn adjust_late_tolerance(&mut self, delta: isize) -> Result<Outcome> {
        let mut timing = self.doc.project().timing;
        let max = cadence_core::material::MAX_LATE_TOLERANCE as isize;
        let next = (timing.late_tolerance_steps as isize + delta).clamp(0, max) as u32;
        timing.late_tolerance_steps = next;
        let out = self.apply(Action::Edit {
            edit: Edit::SetLiveTiming { timing },
        })?;
        self.status = format!(
            "Late boundary tolerance → {next} step{}",
            if next == 1 { "" } else { "s" }
        );
        Ok(out)
    }

    /// Recompile the sliding ring window published to the audio engine.
    pub(super) fn refresh_audio(&mut self) -> Result<()> {
        self.audio_score = self.live.compile(
            self.doc.project(),
            self.doc.flags(),
            Some(self.preview_patch.clone()),
        )?;
        self.audio_score.mixer.filter_grooves = self.assets.filter_grooves.compiled_bank();
        Ok(())
    }

    pub(super) fn record_landed(&mut self, landed: Vec<Landed>) {
        if landed.is_empty() {
            return;
        }
        let labels: Vec<_> = landed.iter().map(|l| l.label.clone()).collect();
        if let Some(take) = self.takes.active.as_mut() {
            take.record(&landed);
            self.takes.saved = false;
        }
        self.status = format!(
            "Landed at {} · {}",
            position_label(landed.last().map_or(0, |l| l.at)),
            labels.join(" · ")
        );
    }

    /// Called after the document changed. The new document is heard from the
    /// change's boundary; changes to instruments, the mixer, tempo or key are
    /// heard immediately with everything before them.
    pub(super) fn live_song_changed(
        &mut self,
        previous: &Project,
        edits: Option<Vec<Edit>>,
        boundary: Boundary,
        label: &str,
    ) -> Result<()> {
        if !previous.timeline_compatible(self.doc.project()) {
            let landed = self.live.land_all();
            self.record_landed(landed);
            self.live.reset_heard(self.doc.project(), self.doc.flags());
            if let Some(take) = self.takes.active.as_mut() {
                take.record_song(self.live.next_step, edits, self.doc.project(), label);
                self.takes.saved = false;
            }
        } else {
            let at = self.live.boundary(self.doc.project(), boundary)?;
            self.live.enqueue(
                at,
                Change::Song {
                    project: Arc::new(self.doc.project().clone()),
                    edits,
                },
                label.to_string(),
            );
            let landed = self.live.land_due();
            self.record_landed(landed);
        }
        self.refresh_audio()
    }

    /// Song edits made through ordinary editing follow edit timing.
    pub(super) fn live_after_commit(
        &mut self,
        previous: &Project,
        edits: Option<Vec<Edit>>,
        label: &str,
    ) -> Result<()> {
        let boundary = self.edit_boundary();
        self.live_song_changed(previous, edits, boundary, label)
    }

    /// Propagate a live-flag change. Lane mutes/solos land on the boundary when
    /// playing; channel mutes/solos affect the static mixer and take effect now.
    fn live_flags_changed(
        &mut self,
        previous: &LiveFlags,
        boundary: Boundary,
        label: &str,
    ) -> Result<()> {
        if !previous.mixer_compatible(self.doc.flags()) {
            let landed = self.live.land_all();
            self.record_landed(landed);
            self.live.reset_heard(self.doc.project(), self.doc.flags());
            if let Some(take) = self.takes.active.as_mut() {
                take.record_flags(self.live.next_step, self.doc.flags().clone(), label);
                self.takes.saved = false;
            }
        } else {
            let at = self.live.boundary(self.doc.project(), boundary)?;
            self.live.enqueue(
                at,
                Change::Flags {
                    flags: self.doc.flags().clone(),
                },
                label.to_string(),
            );
            let landed = self.live.land_due();
            self.record_landed(landed);
        }
        self.refresh_audio()
    }

    /// Lane and channel mute/solo are live state: saved as how you left them,
    /// never undoable, and never a new song revision.
    fn set_live_flags(&mut self, next: LiveFlags, label: String) -> Result<Outcome> {
        let previous = self.doc.flags().clone();
        if !self.doc.set_flags(next)? {
            self.status = label;
            return Ok(Outcome::default());
        }
        self.live.sequence += 1;
        let boundary = self.doc.project().timing.launch;
        self.live_flags_changed(&previous, boundary, &label)?;
        self.status = self.describe_landing(&label);
        Ok(Outcome {
            changed: true,
            ..Outcome::default()
        })
    }

    pub(super) fn set_lane_flags(
        &mut self,
        lane: Id,
        muted: Option<bool>,
        solo: Option<bool>,
    ) -> Result<Outcome> {
        if self.staging.is_some() {
            if let Some(muted) = muted {
                return self.stage(StagedChange::Mute { lane, muted });
            }
            if let Some(solo) = solo {
                return self.stage(StagedChange::Solo { lane, solo });
            }
        }
        let name = self.lane(lane).context("Lane missing")?.name.clone();
        let mut next = self.doc.flags().clone();
        let mut words = Vec::new();
        if let Some(muted) = muted {
            next.set_lane_muted(lane, muted);
            words.push(if muted { "muted" } else { "unmuted" });
        }
        if let Some(solo) = solo {
            next.set_lane_solo(lane, solo);
            words.push(if solo { "soloed" } else { "unsoloed" });
        }
        let label = format!("{name} {}", words.join(", "));
        self.set_live_flags(next, label)
    }

    /// Clear mute or solo on every lane as one live-state change.
    pub(super) fn clear_every_lane_flag(&mut self, solo: bool, label: &str) -> Result<Outcome> {
        let lanes: Vec<Id> = self
            .doc
            .project()
            .lanes
            .iter()
            .filter(|l| {
                if solo {
                    self.doc.flags().lane_solo(l.id)
                } else {
                    self.doc.flags().lane_muted(l.id)
                }
            })
            .map(|l| l.id)
            .collect();
        if lanes.is_empty() {
            self.status = label.into();
            return Ok(Outcome::default());
        }
        if self.staging.is_some() {
            let mut out = Outcome::default();
            for lane in lanes {
                out = self.stage(if solo {
                    StagedChange::Solo { lane, solo: false }
                } else {
                    StagedChange::Mute { lane, muted: false }
                })?;
            }
            return Ok(out);
        }
        let mut next = self.doc.flags().clone();
        for lane in lanes {
            if solo {
                next.set_lane_solo(lane, false);
            } else {
                next.set_lane_muted(lane, false);
            }
        }
        self.set_live_flags(next, label.into())
    }

    pub(super) fn set_channel_flags(
        &mut self,
        channel: ChannelRef,
        muted: Option<bool>,
        solo: Option<bool>,
    ) -> Result<Outcome> {
        if let ChannelRef::Channel(id) = channel {
            ensure!(self.channel(id).is_some(), "Channel missing");
        }
        let mut next = self.doc.flags().clone();
        if let Some(muted) = muted {
            next.set_channel_muted(channel, muted)?;
        }
        if let Some(solo) = solo {
            next.set_channel_solo(channel, solo)?;
        }
        let name = self.channel_name(channel);
        let label = match (muted, solo) {
            (Some(m), _) => format!("Channel {name} {}", if m { "muted" } else { "unmuted" }),
            (_, Some(s)) => format!("Channel {name} {}", if s { "soloed" } else { "unsoloed" }),
            _ => format!("Channel {name}"),
        };
        self.set_live_flags(next, label)
    }

    pub(super) fn describe_landing(&self, label: &str) -> String {
        let Some(pending) = self.live.pending.last() else {
            return label.to_string();
        };
        if !self.transport.playing {
            return label.to_string();
        }
        format!(
            "{label} · lands at {} (in {} beats)",
            position_label(pending.at),
            beats_until(self.live.next_step, pending.at)
        )
    }

    /// Headless clock for scripts and tests: advance the transport as if it
    /// had played `steps` sixteenths, respecting `EndRule::Stop`.
    pub fn advance(&mut self, steps: u64) -> Outcome {
        if !self.transport.playing {
            return Outcome::default();
        }
        let target = self.transport.next_step + steps;
        let stop = if self.is_solo_auditioning() {
            None
        } else {
            let song_len = self.doc.project().arrangement_steps() as u64;
            self.live
                .timeline()
                .stop_at(self.transport.next_step, song_len)
        };
        let (step, next, playing) = match stop {
            Some(stop) if target >= stop => (stop, stop, false),
            _ => (target.saturating_sub(1), target, true),
        };
        let mut out = self.observe_transport(step, next, playing, false);
        if self.live.base != self.live.next_step.saturating_sub(1) / 16 * 16
            && self.refresh_audio().is_ok()
        {
            out.changed = true;
        }
        out
    }

    pub(super) fn launch(
        &mut self,
        lane: Id,
        part: LanePart,
        at: Option<Boundary>,
    ) -> Result<Outcome> {
        ensure!(self.lane(lane).is_some(), "Lane missing");
        ensure!(self.part(part.part).is_some(), "Part missing");
        if self.staging.is_some() {
            return self.stage(StagedChange::Launch { lane, part });
        }
        let label = self.lane_part_label(lane, part.part);
        let boundary = at.unwrap_or(self.doc.project().timing.launch);
        let at = self.live.boundary(self.doc.project(), boundary)?;
        self.live
            .enqueue(at, Change::Launch { lane, part }, label.clone());
        let landed = self.live.land_due();
        self.record_landed(landed);
        self.refresh_audio()?;
        self.status = self.describe_landing(&label);
        Ok(Outcome {
            changed: true,
            ..Outcome::default()
        })
    }

    pub(super) fn release_lane(&mut self, lane: Id, at: Option<Boundary>) -> Result<Outcome> {
        if self.staging.is_some() {
            return self.stage(StagedChange::Release { lane });
        }
        let name = self.lane(lane).context("Lane missing")?.name.clone();
        let label = format!("{name} ■");
        let has_override = self.live.lane_state(lane).is_some()
            || self.live.pending_for_lane(lane).next().is_some();
        if !has_override {
            self.status = format!("{name} has no override");
            return Ok(Outcome::default());
        }
        let at = self.live.boundary(
            self.doc.project(),
            at.unwrap_or(self.doc.project().timing.launch),
        )?;
        self.live
            .enqueue(at, Change::Release { lane }, label.clone());
        let landed = self.live.land_due();
        self.record_landed(landed);
        self.refresh_audio()?;
        self.status = self.describe_landing(&label);
        Ok(Outcome {
            changed: true,
            ..Outcome::default()
        })
    }

    pub(super) fn release_all(&mut self, at: Option<Boundary>) -> Result<Outcome> {
        if self.staging.is_some() {
            let lanes: Vec<Id> = self
                .doc
                .project()
                .lanes
                .iter()
                .filter(|l| {
                    self.live.lane_state(l.id).is_some()
                        || self
                            .live
                            .pending_for_lane(l.id)
                            .any(|p| matches!(p.change, Change::Launch { .. }))
                        || self.staged_launch_on_lane(l.id).is_some()
                })
                .map(|l| l.id)
                .collect();
            if lanes.is_empty() {
                self.status = "No overrides active".into();
                return Ok(Outcome::default());
            }
            let mut out = Outcome::default();
            for lane in lanes {
                out = self.stage(StagedChange::Release { lane })?;
            }
            return Ok(out);
        }
        let has_overrides = self.live.overriding()
            || self
                .live
                .pending
                .iter()
                .any(|p| matches!(p.change, Change::Launch { .. } | Change::Release { .. }));
        if !has_overrides {
            self.status = "No overrides active".into();
            return Ok(Outcome::default());
        }
        let at = self.live.boundary(
            self.doc.project(),
            at.unwrap_or(self.doc.project().timing.launch),
        )?;
        self.live
            .enqueue(at, Change::ReleaseAll, "release all".into());
        let landed = self.live.land_due();
        self.record_landed(landed);
        self.refresh_audio()?;
        self.status = self.describe_landing("Release all");
        Ok(Outcome {
            changed: true,
            ..Outcome::default()
        })
    }

    /// Re-derive `doc.flags()` from `live.heard_flags()` plus any remaining
    /// pending `Change::Flags` in scheduled order so UI mute/solo stays synced with audio.
    pub(super) fn sync_flags_from_live(&mut self) -> Result<()> {
        let mut ordered: Vec<&cadence_core::live::Pending> = self.live.pending.iter().collect();
        ordered.sort_by_key(|p| p.at);
        let mut effective = self.live.heard_flags().clone();
        for pending in ordered {
            if let Change::Flags { flags } = &pending.change {
                effective = flags.clone();
            }
        }
        self.doc.set_flags(effective)?;
        Ok(())
    }

    /// Toggle a lane: release it if it plays (or is about to), otherwise relaunch
    /// its last part, or its first.
    pub(super) fn toggle_lane(&mut self, lane: Id, at: Option<Boundary>) -> Result<Outcome> {
        let expected = self.live.expected();
        if expected.iter().any(|a| a.lane == lane) {
            return self.release_lane(lane, at);
        }
        let part = self
            .live
            .last_part(lane)
            .filter(|p| self.part(p.part).is_some())
            .or_else(|| self.lane(lane).and_then(|l| l.parts.first().copied()))
            .context("This lane has no parts; press a in Bank Parts (gb) to add one")?;
        self.launch(lane, part, at)
    }

    pub(super) fn cancel_pending(&mut self, all: bool) -> Result<Outcome> {
        let removable: Vec<_> = self
            .live
            .pending
            .iter()
            .filter(|p| !matches!(p.change, Change::Song { .. }))
            .map(|p| p.id)
            .collect();
        let cancel: Vec<_> = if all {
            removable
        } else {
            removable.last().copied().into_iter().collect()
        };
        if cancel.is_empty() {
            self.status = if self.live.pending.is_empty() {
                "Nothing pending".into()
            } else {
                "Pending song edits stay; use u to undo them".into()
            };
            return Ok(Outcome::default());
        }
        let labels: Vec<_> = self
            .live
            .pending
            .iter()
            .filter(|p| cancel.contains(&p.id))
            .map(|p| p.label.clone())
            .collect();
        let had_flags = self
            .live
            .pending
            .iter()
            .any(|p| cancel.contains(&p.id) && matches!(p.change, Change::Flags { .. }));
        self.live.remove_pending(&cancel);
        if had_flags {
            self.sync_flags_from_live()?;
        }
        self.refresh_audio()?;
        self.status = format!("Cancelled {}", labels.join(" · "));
        Ok(Outcome {
            changed: true,
            ..Outcome::default()
        })
    }

    /// Fire a staged change on one boundary: launches, releases, mutes, solos,
    /// and any staged section jump all land atomically on the same boundary step.
    fn fire_staged(&mut self, changes: Vec<StagedChange>, boundary: Boundary) -> Result<Outcome> {
        let mut out = Outcome::default();
        let has_section_jump = changes
            .iter()
            .any(|c| matches!(c, StagedChange::JumpSection { .. }));
        if has_section_jump && self.is_solo_auditioning() {
            let stopped = self.stop_audition()?;
            out.live_notes_off.extend(stopped.live_notes_off);
            out.changed |= stopped.changed;
        }

        let at = self.live.boundary(self.doc.project(), boundary)?;
        let mut next_flags = self.doc.flags().clone();
        for change in &changes {
            match change {
                StagedChange::Mute { lane, muted } if self.lane(*lane).is_some() => {
                    next_flags.set_lane_muted(*lane, *muted);
                }
                StagedChange::Solo { lane, solo } if self.lane(*lane).is_some() => {
                    next_flags.set_lane_solo(*lane, *solo);
                }
                _ => {}
            }
        }
        if self.doc.set_flags(next_flags)? {
            self.live.sequence += 1;
            self.live.enqueue(
                at,
                Change::Flags {
                    flags: self.doc.flags().clone(),
                },
                "staged change".into(),
            );
        }

        let mut landed_section_start: Option<u32> = None;
        for change in &changes {
            match change {
                StagedChange::Launch { lane, part } => {
                    let label = self.lane_part_label(*lane, part.part);
                    self.live.enqueue(
                        at,
                        Change::Launch {
                            lane: *lane,
                            part: *part,
                        },
                        label,
                    );
                }
                StagedChange::Release { lane } => {
                    let label = self
                        .lane(*lane)
                        .map_or("lane".into(), |l| format!("{} ■", l.name));
                    self.live
                        .enqueue(at, Change::Release { lane: *lane }, label);
                }
                StagedChange::JumpSection { section } => {
                    if let Some(sec) = self
                        .doc
                        .project()
                        .sections
                        .iter()
                        .find(|s| s.id == *section)
                        .cloned()
                    {
                        let looping = self.playing_loop().is_some();
                        let start = u64::from(sec.start_step);
                        let len = u64::from(sec.length_steps);
                        if looping {
                            self.transport.loop_region =
                                Some((sec.start_step, sec.start_step + sec.length_steps));
                        }
                        let segment = cadence_core::timeline::Segment::new(
                            at,
                            start,
                            looping.then_some((start, len)),
                            self.end_rule(),
                        );
                        self.live.enqueue(
                            at,
                            Change::Timeline { segment },
                            format!("section {}", sec.name),
                        );
                        landed_section_start = Some(sec.start_step);
                    }
                }
                _ => {}
            }
        }
        let landed = self.live.land_due();
        self.record_landed(landed);
        if !self.transport.playing {
            if let Some(start_step) = landed_section_start {
                self.arrange_ui.step = start_step;
                self.select_at_cursor();
                self.transport.step = at;
                self.transport.next_step = at;
                self.live.next_step = at;
                out.transport
                    .push(cadence_core::runtime::TransportIntent::Seek { step: at });
            }
        }
        self.refresh_audio()?;
        self.status = if self.transport.playing {
            format!(
                "The staged change lands at {} (in {} beats)",
                position_label(at),
                beats_until(self.live.next_step, at)
            )
        } else {
            "Applied the staged change".into()
        };
        out.changed = true;
        Ok(out)
    }

    pub fn staging(&self) -> Option<&[StagedChange]> {
        self.staging.as_deref()
    }

    pub(super) fn start_staging(&mut self) -> Result<Outcome> {
        ensure!(
            self.staging.is_none(),
            "Already staging · S fires · u pops · Esc discards"
        );
        self.staging = Some(Vec::new());
        self.status =
            "Staging · launches, releases, mutes, solos & sections collect · S fires · Esc discards"
                .into();
        Ok(Outcome::default())
    }

    /// `S` key in Normal and Jam modes:
    /// - First `S`: enters staging.
    /// - Second `S`: fires all staged changes atomically if non-empty,
    ///   or exits staging cleanly with status `"Exited staging"` if empty.
    pub(super) fn toggle_stage_key(&mut self) -> Result<Outcome> {
        match self.staging.as_deref() {
            None => self.apply(Action::Stage),
            Some([]) => {
                let out = self.apply(Action::FinishStaging {
                    choice: StageChoice::Discard,
                })?;
                self.status = "Exited staging".into();
                Ok(out)
            }
            Some(_) => self.apply(Action::FinishStaging {
                choice: StageChoice::Fire,
            }),
        }
    }

    /// Undo (`u`) while staging is non-empty pops the most recent staged change.
    pub(super) fn pop_staged(&mut self) -> Result<Outcome> {
        let Some(staging) = self.staging.as_mut() else {
            return Ok(Outcome::default());
        };
        let Some(popped) = staging.pop() else {
            self.status = "Nothing staged to undo".into();
            return Ok(Outcome::default());
        };
        let remaining = staging.len();
        let label = popped.label(self.doc.project());
        self.status = if remaining == 0 {
            format!("Unstaged {label} · 0 staged · S or Esc exits")
        } else {
            format!("Unstaged {label} · {remaining} staged · S fires · Esc discards")
        };
        Ok(Outcome::default())
    }

    /// Trigger a section jump for a number-row key slot (`0..10`) in the active bank.
    /// Never leaks Bank 1 sections onto empty slots of a partially populated Bank 2+.
    pub(super) fn trigger_bank_section_key(&mut self, slot: usize) -> Result<Outcome> {
        let bank = self.effective_bank_sections();
        let id = bank
            .get(slot)
            .copied()
            .with_context(|| match SECTION_KEYS.get(slot) {
                Some(key) => format!("No section on {key}"),
                None => format!("No section {}", slot + 1),
            })?;
        self.trigger_section_jump_by_id(id, SECTION_KEYS.get(slot).copied())
    }

    /// Trigger a section jump by absolute 0-based index across `project.sections`
    /// (used by MIDI pads, Program Change, and Next/Prev Section transport bindings).
    pub(super) fn trigger_section_jump(&mut self, index: usize) -> Result<Outcome> {
        let id = self
            .doc
            .project()
            .sections
            .get(index)
            .map(|s| s.id)
            .with_context(|| format!("No section {}", index + 1))?;
        let key_hint = self.section_key_char(id);
        self.trigger_section_jump_by_id(id, key_hint)
    }

    /// Trigger a section jump by explicit `Section` ID.
    /// When staging is active, queues `StagedChange::JumpSection` instead of mutating
    /// the timeline immediately.
    pub(super) fn trigger_section_jump_by_id(
        &mut self,
        id: Id,
        key_hint: Option<char>,
    ) -> Result<Outcome> {
        if self.staging.is_some() {
            return self.stage(StagedChange::JumpSection { section: id });
        }
        self.jump_to_section_id(id, key_hint)
    }

    /// Record a gesture in the staged change instead of letting it land.
    fn stage(&mut self, change: StagedChange) -> Result<Outcome> {
        let label = change.label(self.doc.project());
        let staging = self.staging.as_mut().context("Not staging")?;
        // A later gesture for the same lane (or a newer section jump) replaces an earlier one of its kind.
        let same = |a: &StagedChange, b: &StagedChange| match (a, b) {
            (StagedChange::Mute { lane: x, .. }, StagedChange::Mute { lane: y, .. })
            | (StagedChange::Solo { lane: x, .. }, StagedChange::Solo { lane: y, .. }) => x == y,
            (
                StagedChange::Launch { lane: x, .. } | StagedChange::Release { lane: x },
                StagedChange::Launch { lane: y, .. } | StagedChange::Release { lane: y },
            ) => x == y,
            (StagedChange::JumpSection { .. }, StagedChange::JumpSection { .. }) => true,
            _ => false,
        };
        staging.retain(|existing| !same(existing, &change));
        staging.push(change);
        let count = staging.len();
        self.status = format!(
            "Staged {label} · {count} change{} · S fires · u pop · Esc discards",
            if count == 1 { "" } else { "s" }
        );
        Ok(Outcome::default())
    }

    /// Fire, save or discard what has been staged.
    pub(super) fn finish_staging(&mut self, choice: StageChoice) -> Result<Outcome> {
        let changes = self.staging.take().context("Not staging")?;
        match choice {
            StageChoice::Discard => {
                self.status = format!(
                    "Discarded {} staged change{}",
                    changes.len(),
                    if changes.len() == 1 { "" } else { "s" }
                );
                Ok(Outcome::default())
            }
            _ if changes.is_empty() => {
                self.status = "Nothing was staged".into();
                Ok(Outcome::default())
            }
            StageChoice::Fire => {
                let boundary = self.doc.project().timing.launch;
                self.fire_staged(changes, boundary)
            }
        }
    }

    pub(super) fn add_lane_part(&mut self, lane: Id, entry: LanePart) -> Result<Outcome> {
        let lane_name = self.lane(lane).context("Lane missing")?.name.clone();
        let part_name = self.part(entry.part).context("Part missing")?.name.clone();
        let out = self.commit(
            vec![Edit::AddLanePart { lane, entry }],
            format!("Part {part_name} added to {lane_name}"),
        )?;
        self.selected_lane = Some(lane);
        Ok(out)
    }

    pub(super) fn drop_lane_part(&mut self, lane: Id, part: Id) -> Result<Outcome> {
        let lane_name = self.lane(lane).context("Lane missing")?.name.clone();
        let part_name = self
            .part(part)
            .map_or_else(|| format!("#{part}"), |p| p.name.clone());
        self.commit(
            vec![Edit::DropLanePart { lane, part }],
            format!("Dropped {part_name} from {lane_name}"),
        )
    }

    /// Arm or disarm live take recording.
    /// Arming while playing opens a fresh `TakeJournal` from the current step immediately;
    /// disarming while playing seals any active take cleanly.
    pub(crate) fn set_take_armed(&mut self, armed: bool) -> Result<Outcome> {
        self.takes.armed = armed;
        if armed {
            if self.transport.playing && !self.is_solo_auditioning() {
                self.ensure_take_open()?;
                self.status = "Take recording started · tr disarms & seals".into();
            } else {
                self.status = "Take armed · Space starts recording · tr disarms".into();
            }
        } else if self.takes.active.is_some() {
            self.finish_active_take()?;
            self.status = "Take disarmed & sealed · gt views take".into();
        } else {
            self.status = "Take disarmed".into();
        }
        Ok(Outcome::default())
    }

    /// Interactive `X` in Take deck: confirm before discarding an unsaved take with events.
    pub(crate) fn request_discard_take(&mut self) -> Result<Outcome> {
        let Some(take) = self.current_take() else {
            self.status = "No take to discard".into();
            return Ok(Outcome::default());
        };
        if !self.takes.saved && !take.events.is_empty() {
            self.input.mode = Mode::Confirm(super::ConfirmAction::DiscardTake);
            self.status = format!(
                "Discard unsaved take ({} event{})? y/Enter confirm · Esc cancel",
                take.events.len(),
                if take.events.len() == 1 { "" } else { "s" }
            );
            return Ok(Outcome::default());
        }
        self.apply(Action::DiscardTake)
    }

    /// Clear any active/loaded take, sealed path, visual anchor, and range marks,
    /// removing unkept session take journal files via `HostIo::remove_file`.
    pub(crate) fn discard_take_action(&mut self) -> Result<Outcome> {
        let active_journal = self.takes.active.take();
        let had_active = active_journal.is_some();
        let active_path = active_journal.as_ref().and_then(|t| t.path.clone());
        drop(active_journal);
        let had_last = self.takes.last.take().is_some();
        let sealed_path = self.takes.last_sealed_path.take();
        let was_saved = self.takes.saved;
        if let Some(dir) = &self.takes.session_dir {
            if let Some(path) = active_path.as_deref() {
                if path.starts_with(dir) {
                    let _ = self.io.remove_file(path);
                }
            }
            if !was_saved {
                if let Some(path) = sealed_path.as_deref() {
                    if path.starts_with(dir) {
                        let _ = self.io.remove_file(path);
                    }
                }
            }
        }
        self.takes.cursor_bar = 0;
        self.takes.mark_start = None;
        self.takes.mark_end = None;
        self.takes.visual_anchor = None;
        self.takes.saved = false;
        if had_active && self.transport.playing {
            self.takes.armed = false;
        }
        self.status = if had_active || had_last {
            "Discarded take".into()
        } else {
            "No take to discard".into()
        };
        Ok(Outcome::default())
    }

    /// Open a take journal when playback starts and take recording is armed.
    pub(super) fn ensure_take_open(&mut self) -> Result<()> {
        if !self.takes.armed || self.takes.active.is_some() {
            return Ok(());
        }
        let landed = self.live.land_due();
        self.record_landed(landed);
        let start = self.live.next_step;
        let (path, sink) = match &self.takes.session_dir {
            Some(dir) => {
                let p = self.io.next_session_take_path(dir)?;
                let s = self.io.open_take_sink(&p)?;
                (Some(p), Some(s))
            }
            None => (None, None),
        };
        self.takes.active = Some(cadence_core::take::TakeJournal::open_with_sink(
            path,
            sink,
            self.live.heard(),
            self.live.heard_flags(),
            self.live.active(),
            start,
            Some(self.live.heard_segment()),
        )?);
        self.takes.saved = false;
        Ok(())
    }

    /// Save the active (or most recently sealed) take to the project's `takes/`
    /// folder or an explicit path.
    pub(super) fn save_take_action(&mut self, path: Option<std::path::PathBuf>) -> Result<Outcome> {
        let target = match path {
            Some(p) if p.components().count() == 1 && !p.to_string_lossy().ends_with(".jsonl") => {
                let stem = p.to_string_lossy();
                self.doc
                    .asset_base()
                    .join("takes")
                    .join(format!("{stem}.take.jsonl"))
            }
            Some(p) => p,
            None => {
                let folder = self.doc.asset_base().join("takes");
                let stem = crate::slugify(&self.doc.project().title, "take", Some(64));
                (1..)
                    .map(|n| folder.join(format!("{stem}-{n:03}.take.jsonl")))
                    .find(|p| !self.io.exists(p))
                    .expect("free take name")
            }
        };
        let take = self.save_take(&target)?;
        self.takes.saved = true;
        self.status = format!(
            "Saved take: {} bars, {} changes → {}",
            take.steps().div_ceil(16),
            take.events.len(),
            target.display()
        );
        Ok(Outcome::default())
    }

    /// Load a `.take.jsonl` performance file via `self.io` into the Take deck (`gt`)
    /// so the user can inspect, mark bar ranges, keep into the arrangement, or export it.
    pub(super) fn load_take_action(&mut self, path: std::path::PathBuf) -> Result<Outcome> {
        let resolved = if path.is_relative() && !self.io.is_file(&path) {
            let rel = self.doc.asset_base().join(&path);
            if self.io.is_file(&rel) {
                rel
            } else {
                path
            }
        } else {
            path
        };
        let bytes = self
            .io
            .read_bounded(&resolved, cadence_core::take::MAX_TAKE_BYTES)
            .with_context(|| format!("Reading take {}", resolved.display()))?;
        let take = cadence_core::take::Take::from_jsonl_bytes(&bytes, true)
            .with_context(|| format!("Parsing take {}", resolved.display()))?;
        let bars = take.steps().div_ceil(16).max(1);
        let events = take.events.len();
        self.takes.active = None;
        self.takes.last = Some(take);
        self.takes.last_sealed_path = Some(resolved.clone());
        self.takes.cursor_bar = 0;
        self.takes.mark_start = None;
        self.takes.mark_end = None;
        self.takes.visual_anchor = None;
        self.takes.saved = true;
        self.go_to_deck(crate::layout::Deck::song(crate::layout::DeckKind::Take));
        self.status = format!(
            "Loaded take: {bars} bars, {events} changes ← {} · v mark · k keep",
            resolved.display()
        );
        Ok(Outcome::default())
    }

    /// Live state for structured clients and script expectations.
    pub fn live_state_json(&self) -> serde_json::Value {
        let live = &self.live;
        serde_json::json!({
            "playing": self.transport.playing,
            "step": self.transport.step,
            "next_step": self.transport.next_step,
            "sequence": live.sequence,
            "active": live.active(),
            "pending": live.pending.iter().map(|p| serde_json::json!({
                "id": p.id,
                "at": p.at,
                "label": p.label,
            })).collect::<Vec<_>>(),
            "audition": live.audition.map(|a| serde_json::json!({
                "pattern": a.pattern,
                "instrument": a.instrument,
                "anchor": a.anchor,
                "length": a.length,
                "solo": a.solo,
            })),
            "take_armed": self.takes.armed,
            "take": self.takes.active.as_ref().map(|t| serde_json::json!({
                "path": t.path,
                "start_step": t.start_step(),
                "changes": t.len(),
            })),
        })
    }

    /// A short, specific label for a single edit.
    pub(super) fn describe_edit(&self, edit: &Edit) -> String {
        let pattern = |id: Id| {
            self.doc
                .project()
                .pattern(id)
                .map_or_else(|| format!("pattern {id}"), |p| p.name.clone())
        };
        match edit {
            Edit::SetValue {
                pattern: id,
                step,
                value,
            } => match value {
                Some(value) => format!("{} step {} → {value}", pattern(*id), step + 1),
                None => format!("{} step {} cleared", pattern(*id), step + 1),
            },
            Edit::SetNote {
                pattern: id, step, ..
            } => {
                format!("{} step {} note", pattern(*id), step + 1)
            }
            Edit::SetRhythm {
                pattern: id,
                step,
                active,
            } => format!(
                "{} step {} {}",
                pattern(*id),
                step + 1,
                if *active { "on" } else { "off" }
            ),
            Edit::PutPattern { pattern: p } => format!("Edited {}", p.name),
            Edit::PutPart { part } => format!("Edited part {}", part.name),
            Edit::AddLanePart { .. } => "Added part to lane".into(),
            Edit::DropLanePart { .. } => "Dropped part from lane".into(),
            Edit::SetTempo { bpm } => format!("Tempo {bpm:.1} BPM"),
            Edit::SetLiveTiming { .. } => "Live timing".into(),
            _ => "Edited musical definition".into(),
        }
    }

    /// `Lane ▶ Part` for a lane part launch.
    pub fn lane_part_label(&self, lane: Id, part: Id) -> String {
        format!(
            "{} ▶ {}",
            self.lane(lane).map_or("?", |l| l.name.as_str()),
            self.part(part).map_or("?", |b| b.name.as_str())
        )
    }

    /// True while the active take journal has hit its bounded on-disk size cap (`MAX_TAKE_BYTES`).
    pub fn take_capped(&self) -> bool {
        self.takes
            .active
            .as_ref()
            .is_some_and(cadence_core::take::TakeJournal::is_capped)
    }

    pub fn take_recording(&self) -> Option<(u64, usize)> {
        self.takes
            .active
            .as_ref()
            .map(|t| (t.start_step(), t.len()))
    }

    /// The finishing prefix after `Alt+Enter` while changes are staged.
    pub(super) fn handle_stage_prefix(&mut self, key: KeyEvent) -> Result<Outcome> {
        self.input.mode = self.take_resume_mode();
        match key.code {
            KeyCode::Enter | KeyCode::Char('f' | 'S') => self.apply(Action::FinishStaging {
                choice: StageChoice::Fire,
            }),
            KeyCode::Char('x' | 'd') => self.apply(Action::FinishStaging {
                choice: StageChoice::Discard,
            }),
            _ => {
                self.status = "Still staging · Alt+Enter or S finishes".into();
                Ok(Outcome::default())
            }
        }
    }

    /// MIDI input: MIDI Learn, universal MMC transport, profile/user CC/Note mappings,
    /// performance pad modes (Notes/Sections/Mutes), live velocity-sensitive play,
    /// live MIDI note recording, and bi-directional hardware LED/OLED feedback.
    pub fn handle_midi(&mut self, message: MidiMessage) -> Result<Outcome> {
        self.handle_midi_event(message)
    }

    /// Release every note held by MIDI input.
    pub fn release_midi(&mut self) -> Outcome {
        Outcome {
            live_notes_off: self
                .input
                .midi_held
                .drain(..)
                .map(|(midi, instrument_id)| super::InstrumentNoteOff {
                    instrument_id,
                    midi,
                })
                .collect(),
            ..Outcome::default()
        }
    }

    /// Ex commands for live performance.
    pub(super) fn execute_live_verb(
        &mut self,
        verb: crate::commands::LiveVerb,
        args: &[String],
    ) -> Result<Outcome> {
        use crate::commands::LiveVerb;
        match verb {
            LiveVerb::Launch => self.launch_command(args),
            LiveVerb::Release => self.release_command(args),
            LiveVerb::Stage => match args.first().map(|s| s.to_ascii_lowercase()).as_deref() {
                None | Some("start") => self.apply(Action::Stage),
                Some("fire") => self.apply(Action::FinishStaging {
                    choice: StageChoice::Fire,
                }),
                Some("discard" | "cancel") => self.apply(Action::FinishStaging {
                    choice: StageChoice::Discard,
                }),
                Some(_) => Err(anyhow::anyhow!("Usage: :stage [start|fire|discard]")),
            },
            LiveVerb::Unstage => self.apply(Action::FinishStaging {
                choice: StageChoice::Discard,
            }),
            LiveVerb::Undo => self.apply(Action::Undo),
            LiveVerb::Redo => self.apply(Action::Redo),
            LiveVerb::Cancel => self.apply(Action::CancelPending {
                all: args.first().is_some_and(|a| a == "all"),
            }),
            LiveVerb::Quantize => self.quantize_command(args),
            LiveVerb::Tolerance => {
                let steps: u32 = args
                    .first()
                    .context("Usage: :tolerance STEPS")?
                    .parse()
                    .context("Tolerance is a whole number of steps")?;
                let mut timing = self.doc.project().timing;
                timing.late_tolerance_steps = steps;
                self.apply(Action::Edit {
                    edit: Edit::SetLiveTiming { timing },
                })
            }
            LiveVerb::Advance => {
                let steps = parse_duration(&args.join(" "))?;
                self.apply(Action::Advance { steps })
            }
            LiveVerb::Take => match args.first().map(|s| s.to_ascii_lowercase()).as_deref() {
                Some("arm" | "on") => self.apply(Action::SetTakeArmed { armed: true }),
                Some("disarm" | "off") => self.apply(Action::SetTakeArmed { armed: false }),
                Some("toggle") => self.apply(Action::ToggleTakeArmed),
                Some("discard" | "clear") => self.apply(Action::DiscardTake),
                Some("save") => self.apply(Action::SaveTake {
                    path: args.get(1).map(std::path::PathBuf::from),
                }),
                Some("load" | "open") => {
                    let path = args.get(1).context("Usage: :take load PATH.take.jsonl")?;
                    self.apply(Action::LoadTake {
                        path: std::path::PathBuf::from(path),
                    })
                }
                _ => Err(anyhow::anyhow!(
                    "Usage: :take arm|disarm|toggle|discard|save [NAME|PATH]|load PATH"
                )),
            },
            LiveVerb::Keep => match args {
                [] => self.keep_all_overrides(),
                [a, b] => {
                    let start: usize = a.parse().context("Usage: :keep START_BAR END_BAR")?;
                    let end: usize = b.parse().context("Usage: :keep START_BAR END_BAR")?;
                    self.apply(Action::KeepRange {
                        start_bar: start,
                        end_bar: end,
                    })
                }
                _ => Err(anyhow::anyhow!("Usage: :keep [START_BAR END_BAR]")),
            },
        }
    }

    fn lane_named(&self, name: &str) -> Result<Id> {
        self.doc
            .project()
            .lanes
            .iter()
            .find(|l| l.name.eq_ignore_ascii_case(name))
            .map(|l| l.id)
            .with_context(|| format!("No lane named {name}"))
    }

    /// `at BOUNDARY` (or a bare boundary) after a command's own arguments.
    fn parse_at(&self, args: &[String]) -> Result<Option<Boundary>> {
        let words: Vec<&str> = args
            .iter()
            .map(String::as_str)
            .skip_while(|w| w.eq_ignore_ascii_case("at"))
            .collect();
        if words.is_empty() {
            return Ok(None);
        }
        Boundary::parse(&words.join(" "), self.doc.project()).map(Some)
    }

    fn release_command(&mut self, args: &[String]) -> Result<Outcome> {
        match args.first().map(|s| s.to_ascii_lowercase()).as_deref() {
            None => self.apply(Action::ReleaseAll { at: None }),
            Some("all") => {
                let at = self.parse_at(&args[1..])?;
                self.apply(Action::ReleaseAll { at })
            }
            Some("at") => {
                let at = self.parse_at(args)?;
                self.apply(Action::ReleaseAll { at })
            }
            Some(_) => {
                let lane = self.lane_named(&args[0])?;
                let at = self.parse_at(&args[1..])?;
                self.apply(Action::Release { lane, at })
            }
        }
    }

    fn launch_command(&mut self, args: &[String]) -> Result<Outcome> {
        let lane = self.lane_named(
            args.first()
                .context("Usage: :launch LANE [PART] [at BOUNDARY]")?,
        )?;
        let lane_obj = self.lane(lane).context("Lane missing")?;
        let (part, rest) = match args.get(1) {
            None => (
                *lane_obj.parts.first().context("Lane has no parts")?,
                &args[1..],
            ),
            Some(arg) if arg.eq_ignore_ascii_case("at") => (
                *lane_obj.parts.first().context("Lane has no parts")?,
                &args[1..],
            ),
            Some(arg) => {
                if let Ok(n) = arg.parse::<usize>() {
                    ensure!(n > 0, "Part indices start at 1");
                    let entry = *lane_obj
                        .parts
                        .get(n - 1)
                        .with_context(|| format!("Lane has no part {n}"))?;
                    (entry, &args[2..])
                } else if let Ok(part_id) = self.part_id(arg) {
                    let entry = lane_obj
                        .parts
                        .iter()
                        .copied()
                        .find(|p| p.part == part_id)
                        .unwrap_or(LanePart {
                            part: part_id,
                            phase: cadence_core::material::PhaseMode::Song,
                            offset_steps: 0,
                        });
                    (entry, &args[2..])
                } else {
                    (
                        *lane_obj.parts.first().context("Lane has no parts")?,
                        &args[1..],
                    )
                }
            }
        };
        let at = self.parse_at(rest)?;
        self.apply(Action::Launch { lane, part, at })
    }

    fn quantize_command(&mut self, args: &[String]) -> Result<Outcome> {
        let mut timing = self.doc.project().timing;
        match args.first().map(|s| s.to_ascii_lowercase()).as_deref() {
            Some("edits") => {
                timing.edits = match args.get(1).map(|s| s.to_ascii_lowercase()).as_deref() {
                    Some("now") => EditTiming::Now,
                    Some("launch") => EditTiming::Launch,
                    _ => bail!("Usage: :quantize edits now|launch"),
                }
            }
            Some("launch") => {
                timing.launch = Boundary::parse(&args[1..].join(" "), self.doc.project())?;
            }
            Some(_) => {
                timing.launch = Boundary::parse(&args.join(" "), self.doc.project())?;
            }
            None => {
                self.status = format!(
                    "Launch timing {} · edits {} · late tolerance {} steps",
                    timing.launch.label(self.doc.project()),
                    match timing.edits {
                        EditTiming::Now => "now",
                        EditTiming::Launch => "follow launch timing",
                    },
                    timing.late_tolerance_steps
                );
                return Ok(Outcome::default());
            }
        }
        self.apply(Action::Edit {
            edit: Edit::SetLiveTiming { timing },
        })
    }
}

/// How a staged change finishes.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageChoice {
    Fire,
    Discard,
}

/// `16`, `16 steps`, `2 bars`, `3 beats`.
pub fn parse_duration(text: &str) -> Result<u64> {
    let text = text.trim().to_ascii_lowercase();
    let (number, unit) = text
        .split_once(char::is_whitespace)
        .map_or((text.as_str(), "steps"), |(n, u)| (n, u.trim()));
    let (number, unit) = match number.find(|c: char| !c.is_ascii_digit()) {
        Some(at) if unit == "steps" && at > 0 => (&number[..at], &number[at..]),
        _ => (number, unit),
    };
    let count: u64 = number
        .parse()
        .with_context(|| format!("{text} is not a duration; use N steps, beats or bars"))?;
    let per = match unit {
        "" | "s" | "step" | "steps" => 1,
        "beat" | "beats" => 4,
        "bar" | "bars" | "b" => 16,
        other => bail!("Unknown duration unit {other}"),
    };
    Ok(count * per)
}
