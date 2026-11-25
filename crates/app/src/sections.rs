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
    /// Active Jam/Alt bank index clamped to the project's current bank count.
    pub(crate) fn clamped_bank(&self) -> usize {
        self.input.bank.min(self.jam_bank_count().saturating_sub(1))
    }

    /// Whether the number row has sections to address.
    pub fn uses_sections(&self) -> bool {
        !self.doc.project().sections.is_empty()
    }

    /// The sections sliced strictly for the current bank (`10` per bank).
    pub fn bank_sections(&self) -> Vec<Id> {
        self.doc
            .project()
            .sections
            .iter()
            .skip(self.clamped_bank() * SECTION_KEYS.len())
            .take(SECTION_KEYS.len())
            .map(|s| s.id)
            .collect()
    }

    /// Sections bound to the number row (`1..0` / `Alt+1..0`) in the active bank.
    ///
    /// When the active bank contains sections, returns strictly that bank's slice
    /// (never leaking Bank 1 slots into a partially populated Bank 2+). When the
    /// user paged to Bank 2+ for lanes (`>10` lanes, `<=10` sections) so the active
    /// bank has no sections of its own, falls back to Bank 1's sections so `1..0`
    /// remain usable.
    pub fn effective_bank_sections(&self) -> Vec<Id> {
        let bank = self.bank_sections();
        if !bank.is_empty() {
            bank
        } else {
            self.doc
                .project()
                .sections
                .iter()
                .take(SECTION_KEYS.len())
                .map(|s| s.id)
                .collect()
        }
    }

    /// Number-row shortcut (`1..0`) currently bound to `section_id`, if active in this bank.
    pub(crate) fn section_key_char(&self, section_id: Id) -> Option<char> {
        self.effective_bank_sections()
            .iter()
            .position(|&id| id == section_id)
            .and_then(|pos| SECTION_KEYS.get(pos).copied())
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

    pub(super) fn jump_to_section_id(&mut self, id: Id, key_hint: Option<char>) -> Result<Outcome> {
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
            match key_hint {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::DeckView;
    use cadence_core::material::Project;
    use cadence_core::runtime::AudioSnapshot;

    fn app_with_sections_and_lanes(section_count: usize, lane_count: usize) -> App {
        let mut proj = Project::empty();
        proj.title = "Bank Test".into();
        proj.bars = section_count.max(4) * 2;
        for i in 0..lane_count {
            proj.lanes.push(cadence_core::material::Lane {
                id: 100 + i as Id,
                name: format!("Lane {}", i + 1),
                placements: Vec::new(),
                parts: Vec::new(),
            });
        }
        for i in 0..section_count {
            proj.sections.push(Section {
                id: 200 + i as Id,
                name: format!("Sec {}", i + 1),
                start_step: (i as u32) * 32,
                length_steps: 32,
            });
        }
        let io = std::sync::Arc::new(crate::MemoryHostIo::new("/workspace"));
        App::new_with_host_io(proj, PathBuf::from("/workspace/bank-test.json"), io).unwrap()
    }

    #[test]
    fn bank_2_sections_align_badges_keys_and_never_leak_bank_1_slots() {
        let mut app = app_with_sections_and_lanes(12, 2);
        app.press_keys("z2").unwrap(); // Arrange + Bank visible
        app.press_keys("gj").unwrap(); // Enter Jam mode

        let snap = AudioSnapshot::default();

        // Bank 1: Arrange badges 1..10 with '1'..='0', 11..12 with None; Bank/Jam HUD show 1..10.
        let frame1 = app.frame_view(&snap, "");
        let arr1 = frame1
            .decks
            .iter()
            .find_map(|d| match &d.view {
                DeckView::Arrange(v) => Some(v),
                _ => None,
            })
            .expect("Arrange deck");
        assert_eq!(arr1.sections.len(), 12);
        for (i, sec) in arr1.sections.iter().enumerate() {
            let expected = SECTION_KEYS.get(i).copied();
            assert_eq!(sec.key_char, expected, "Bank 1 Arrange badge at {i}");
        }
        let bank1 = frame1
            .decks
            .iter()
            .find_map(|d| match &d.view {
                DeckView::Bank(v) => Some(v),
                _ => None,
            })
            .expect("Bank deck");
        assert_eq!(bank1.sections.len(), 10);
        assert_eq!(bank1.sections[0].name, "Sec 1");
        assert_eq!(bank1.sections[0].key_char, Some('1'));

        // Page to Bank 2 (`Tab` in Jam).
        app.press_keys("<Tab>").unwrap();
        assert_eq!(app.clamped_bank(), 1);

        let frame2 = app.frame_view(&snap, "");
        let arr2 = frame2
            .decks
            .iter()
            .find_map(|d| match &d.view {
                DeckView::Arrange(v) => Some(v),
                _ => None,
            })
            .expect("Arrange deck");
        for (i, sec) in arr2.sections.iter().enumerate() {
            let expected = match i {
                10 => Some('1'),
                11 => Some('2'),
                _ => None,
            };
            assert_eq!(sec.key_char, expected, "Bank 2 Arrange badge at {i}");
        }
        let bank2 = frame2
            .decks
            .iter()
            .find_map(|d| match &d.view {
                DeckView::Bank(v) => Some(v),
                _ => None,
            })
            .expect("Bank deck");
        assert_eq!(bank2.sections.len(), 2);
        assert_eq!(bank2.sections[0].name, "Sec 11");
        assert_eq!(bank2.sections[0].key_char, Some('1'));
        assert_eq!(bank2.sections[1].name, "Sec 12");
        assert_eq!(bank2.sections[1].key_char, Some('2'));

        let hud2 = frame2.jam_hud.expect("Jam HUD on Bank 2");
        assert_eq!(hud2.bank, 1);
        assert_eq!(hud2.bank_count, 2);
        assert_eq!(hud2.sections.len(), 2);
        assert_eq!(hud2.sections[0].0, '1');
        assert_eq!(hud2.sections[0].1, "Sec 11");
        assert_eq!(hud2.sections[1].0, '2');
        assert_eq!(hud2.sections[1].1, "Sec 12");

        // Pressing '1' on Bank 2 jumps to Sec 11 (step 320); pressing '2' jumps to Sec 12 (step 352).
        app.press_keys("1").unwrap();
        assert_eq!(app.arrange_ui.step, 10 * 32);
        app.press_keys("2").unwrap();
        assert_eq!(app.arrange_ui.step, 11 * 32);

        // Pressing '3' on Bank 2 MUST fail cleanly instead of jumping to Sec 3 from Bank 1!
        let err = app
            .number_key('3')
            .expect_err("empty Bank 2 slot must not leak Bank 1 Sec 3");
        assert!(format!("{err:#}").contains("No section on 3"));
        assert_eq!(app.arrange_ui.step, 11 * 32);

        // Absolute MIDI/ProgramChange jump (index 0 = Sec 1, index 11 = Sec 12) stays bank-independent.
        app.trigger_section_jump(0).unwrap();
        assert_eq!(app.arrange_ui.step, 0);
        app.trigger_section_jump(11).unwrap();
        assert_eq!(app.arrange_ui.step, 11 * 32);
    }

    #[test]
    fn bank_2_for_many_lanes_with_few_sections_keeps_bank_1_sections_accessible() {
        let mut app = app_with_sections_and_lanes(4, 12);
        app.press_keys("gj<Tab>").unwrap();
        assert_eq!(app.clamped_bank(), 1);
        assert_eq!(app.effective_bank_sections().len(), 4);

        app.press_keys("4").unwrap();
        assert_eq!(app.arrange_ui.step, 3 * 32);

        let err = app
            .number_key('5')
            .expect_err("slot 5 beyond 4 sections must report missing");
        assert!(format!("{err:#}").contains("No section on 5"));
    }
}
