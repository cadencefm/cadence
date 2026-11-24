//! Section keys: the number row in Jam, and Alt with a digit everywhere, move
//! the song (or its loop) to a section through `Change::Timeline` on `Performance`.
//!
//! Stopped: immediate remap at `now` + Arrange cursor move.
//! Playing: lands at the end of the current section or loop pass (or next launch
//! boundary outside any section); pressing the same key again brings it forward
//! to the next launch boundary; Backspace cancels; Home clears.
use super::*;
use cadence_core::live::Change;
use cadence_core::material::Section;
use cadence_core::timeline::Segment;

impl App {
    /// Whether the number row has sections to address.
    pub fn uses_sections(&self) -> bool {
        !self.doc.project().sections.is_empty()
    }

    /// The sections the number row addresses in the current bank.
    pub fn bank_sections(&self) -> Vec<Id> {
        self.doc
            .project()
            .sections
            .iter()
            .skip(self.input.bank * SECTION_KEYS.len())
            .take(SECTION_KEYS.len())
            .map(|s| s.id)
            .collect()
    }

    fn section(&self, id: Id) -> Option<&Section> {
        self.doc.project().sections.iter().find(|s| s.id == id)
    }

    /// The named section spanning the current Arrange cursor, if any.
    /// When sections overlap, prefers the most specific (latest-starting, then newest) section.
    pub(super) fn section_at_cursor(&self) -> Option<&Section> {
        let cursor = self.arrange_ui.step;
        self.doc
            .project()
            .sections
            .iter()
            .filter(|s| (s.start_step..s.start_step + s.length_steps).contains(&cursor))
            .max_by_key(|s| (s.start_step, s.id))
    }

    pub(super) fn section_key(&mut self, index: usize) -> Result<Outcome> {
        let bank = self.bank_sections();
        let id = bank
            .get(index)
            .copied()
            .or_else(|| self.doc.project().sections.get(index).map(|s| s.id))
            .with_context(|| match SECTION_KEYS.get(index) {
                Some(key) => format!("No section on {key}"),
                None => format!("No section {}", index + 1),
            })?;
        let target = self.section(id).context("Section missing")?.clone();
        let mut out = Outcome::default();
        if self.is_solo_auditioning() {
            let stopped = self.stop_audition()?;
            out.live_notes_off.extend(stopped.live_notes_off);
            out.changed |= stopped.changed;
        }
        let looping = self.playing_loop().is_some();
        let start = u64::from(target.start_step);
        let len = u64::from(target.length_steps);
        let region = (target.start_step, target.start_step + target.length_steps);
        if looping {
            self.transport.loop_region = Some(region);
        }
        let now = self.transport.next_step;
        if !self.transport.playing {
            self.live.cancel_pending_timeline();
            self.arrange_ui.step = target.start_step;
            self.select_at_cursor();
            let segment =
                Segment::new(now, start, looping.then_some((start, len)), self.end_rule());
            self.live.set_segment(segment);
            if let Some(take) = &mut self.takes.active {
                take.record_segment(now, segment, &format!("section {}", target.name));
            }
            self.transport.step = now;
            self.transport.next_step = now;
            self.live.next_step = now;
            self.refresh_audio()?;
            self.status = format!("{} · Space plays it", target.name);
            out.changed = true;
            out.transport.push(TransportIntent::Seek { step: now });
            return Ok(out);
        }

        let song_len = self.doc.project().arrangement_steps().max(1) as u64;
        let launch_at = self
            .live
            .boundary(self.doc.project(), self.doc.project().timing.launch)?
            .max(now);
        let again = self.live.pending_timeline().is_some_and(|(_, seg)| {
            seg.song == start && seg.region == looping.then_some((start, len))
        });
        let at = if again {
            launch_at
        } else if looping {
            self.live
                .timeline()
                .pass_end(now, song_len)
                .filter(|&end| end > now)
                .unwrap_or(launch_at)
        } else {
            self.live
                .timeline()
                .song_step(now, song_len)
                .and_then(|cur| {
                    self.doc
                        .project()
                        .sections
                        .iter()
                        .find(|s| {
                            (u64::from(s.start_step)..u64::from(s.start_step + s.length_steps))
                                .contains(&cur)
                        })
                        .map(|s| now + (u64::from(s.start_step + s.length_steps) - cur))
                })
                .filter(|&end| end > now)
                .unwrap_or(launch_at)
        };

        let segment = Segment::new(at, start, looping.then_some((start, len)), self.end_rule());
        self.live.enqueue(
            at,
            Change::Timeline { segment },
            format!("section {}", target.name),
        );
        let landed = self.live.land_due();
        self.record_landed(landed);
        self.refresh_audio()?;
        let bar = self.song_position(at) / 16 + 1;
        let next_bar = self.song_position(launch_at) / 16 + 1;
        self.status = if again {
            format!("{} at bar {bar}", target.name)
        } else {
            match SECTION_KEYS.get(index) {
                Some(key) => format!(
                    "{} at bar {bar} · {key} again for bar {next_bar}",
                    target.name,
                ),
                None => format!("{} at bar {bar} · repeat for bar {next_bar}", target.name,),
            }
        };
        out.changed = true;
        Ok(out)
    }

    /// `H`/`L` (and `w`/`b` in Placements) on Arrange: jump to previous or next Section
    /// (falling back to ±1 bar when no sections are defined).
    pub(super) fn jump_section(&mut self, forward: bool, count: usize) {
        self.suspend_follow();
        let mut starts: Vec<(u32, String)> = self
            .doc
            .project()
            .sections
            .iter()
            .map(|s| (s.start_step, s.name.clone()))
            .collect();
        starts.sort_by_key(|(step, _)| *step);
        starts.dedup_by_key(|(step, _)| *step);

        if starts.is_empty() {
            let max_step = (self.doc.project().bars as u32)
                .saturating_mul(16)
                .saturating_sub(1);
            let stride = (count.max(1).min(u32::MAX as usize / 16) as u32) * 16;
            self.arrange_ui.step = if forward {
                self.arrange_ui.step.saturating_add(stride).min(max_step)
            } else {
                self.arrange_ui.step.saturating_sub(stride)
            };
            self.select_at_cursor();
            self.status = format!("Bar {}", self.arrange_ui.step / 16 + 1);
            return;
        }

        let target = if forward {
            let after = starts.partition_point(|(s, _)| *s <= self.arrange_ui.step);
            if after < starts.len() {
                let idx = (after + count.saturating_sub(1)).min(starts.len() - 1);
                Some(&starts[idx])
            } else {
                starts.last()
            }
        } else {
            let before = starts.partition_point(|(s, _)| *s < self.arrange_ui.step);
            if before > 0 {
                let idx = before.saturating_sub(count.max(1));
                Some(&starts[idx])
            } else {
                starts.first()
            }
        };

        if let Some((step, name)) = target {
            self.arrange_ui.step = *step;
            self.select_at_cursor();
            self.status = format!("Section '{name}' · bar {}", step / 16 + 1);
        }
    }

    pub(crate) fn section_delete_summary(&self, id: Id) -> Result<String> {
        let sec = self.section(id).context("Section missing")?;
        let start_bar = sec.start_step / 16 + 1;
        let end_bar = (sec.start_step + sec.length_steps)
            .div_ceil(16)
            .max(start_bar);
        let range = if start_bar == end_bar {
            format!("bar {start_bar}")
        } else {
            format!("bars {start_bar}–{end_bar}")
        };
        Ok(format!("Delete section '{}' ({range})?", sec.name))
    }

    pub(crate) fn request_delete_section(&mut self, id: Id) -> Result<Outcome> {
        let msg = self.section_delete_summary(id)?;
        self.input.mode = Mode::Confirm(ConfirmAction::DeleteSection(id));
        self.status = format!("{msg} y confirm · Esc cancel");
        Ok(Outcome::default())
    }

    pub(crate) fn delete_section_confirmed(&mut self, id: Id) -> Result<Outcome> {
        let name = self.section(id).context("Section missing")?.name.clone();
        let out = self.apply(Action::Edit {
            edit: Edit::RemoveSection { id },
        })?;
        self.status = format!("Deleted section '{name}' · u restores it");
        Ok(out)
    }

    /// `tl`: loop the section under the Arrange cursor.
    pub(super) fn loop_section_at_cursor(&mut self) -> Result<Outcome> {
        let section = self
            .section_at_cursor()
            .cloned()
            .context("No section under the cursor · press nt to add one")?;
        let region = (
            section.start_step,
            section.start_step + section.length_steps,
        );
        let out = self.activate_loop(region)?;
        if self.live.pending_timeline().is_none() {
            self.status = format!("Looping {}", section.name);
        }
        Ok(out)
    }
}
