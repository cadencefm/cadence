//! Song / Project Properties modal (`gP` and Action Palette):
//! Title, Tempo (BPM), Key Root, Scale (13-scale `cadence_core::music::NAMED_SCALES` SPOT),
//! Length (Bars), End Rule, and Project Live Timing — all landing cleanly on the `u` undo stack.
use super::*;
use cadence_core::material::{EditTiming, MAX_BARS, MAX_LATE_TOLERANCE};
use cadence_core::music::{cycle_scale, note_name, scale_label, EndRule};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SongPropRow {
    Title,
    Tempo,
    KeyRoot,
    Scale,
    Bars,
    EndRule,
    LaunchQuantize,
    EditTiming,
    LateTolerance,
}

impl SongPropRow {
    pub(crate) const ALL: [Self; 9] = [
        Self::Title,
        Self::Tempo,
        Self::KeyRoot,
        Self::Scale,
        Self::Bars,
        Self::EndRule,
        Self::LaunchQuantize,
        Self::EditTiming,
        Self::LateTolerance,
    ];

    pub(crate) fn index(self) -> usize {
        Self::ALL.iter().position(|&r| r == self).unwrap_or(0)
    }

    pub(crate) fn from_index(index: usize) -> Self {
        Self::ALL[index.min(Self::ALL.len() - 1)]
    }

    pub(crate) fn group(self) -> &'static str {
        match self {
            Self::Title | Self::Tempo => "PROJECT",
            Self::KeyRoot | Self::Scale => "MUSICAL KEY",
            Self::Bars | Self::EndRule => "ARRANGEMENT",
            Self::LaunchQuantize | Self::EditTiming | Self::LateTolerance => "LIVE TIMING",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SongPropertiesState {
    pub cursor: usize,
    pub title_buf: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SongPropRowView {
    pub group: &'static str,
    pub label: &'static str,
    pub value: String,
    pub detail: &'static str,
    pub selected: bool,
    pub adjustable: bool,
}

#[derive(Clone, Debug)]
pub struct SongPropertiesView {
    pub rows: Vec<SongPropRowView>,
    pub title_editing: Option<String>,
    pub dirty: bool,
    pub summary: String,
}

impl App {
    pub(crate) fn open_song_properties(&mut self, initial_row: SongPropRow, edit_title: bool) {
        let mut state = SongPropertiesState {
            cursor: initial_row.index(),
            title_buf: None,
        };
        if edit_title && initial_row == SongPropRow::Title {
            state.title_buf = Some(self.doc.project().title.clone());
            self.status = "Rename Song · Enter saves · Ctrl+u clear · Esc cancels".into();
        } else {
            self.status =
                "Song Properties · j/k row · Alt+h/l or ,/. adjust · Enter exact · Alt+Bksp reset · Esc close"
                    .into();
        }
        self.modal = ActiveModal::SongProperties(state);
        self.input.count = None;
        self.input.mode = Mode::SongProperties;
    }

    pub fn song_properties_view(&self) -> Option<SongPropertiesView> {
        if self.input.mode != Mode::SongProperties {
            return None;
        }
        let state = self.modal.song_props()?;
        let p = self.doc.project();
        let rows = SongPropRow::ALL
            .into_iter()
            .enumerate()
            .map(|(i, row)| {
                let (label, value, detail, adjustable) = match row {
                    SongPropRow::Title => (
                        "Title",
                        p.title.clone(),
                        "Song title shown in chrome and saved project metadata (Enter edits inline)",
                        false,
                    ),
                    SongPropRow::Tempo => (
                        "Tempo (BPM)",
                        format!("{:.1} BPM", p.bpm),
                        "Song tempo (20–300 BPM · Alt+h/l or ,/. ±1 · Alt+H/L or </> ±10 · Enter exact)",
                        true,
                    ),
                    SongPropRow::KeyRoot => (
                        "Key Root",
                        format!("{} (MIDI {})", note_name(p.root_midi), p.root_midi),
                        "Tonic pitch class & octave (Alt+h/l or ,/. ±1 st · Alt+H/L or </> ±1 oct · Enter exact)",
                        true,
                    ),
                    SongPropRow::Scale => (
                        "Scale",
                        scale_label(&p.scale).to_string(),
                        "Musical scale across 13 named scales (Alt+h/l or ,/. cycle · Alt+H/L or </> jump)",
                        true,
                    ),
                    SongPropRow::Bars => (
                        "Length (Bars)",
                        format!("{} bars ({} steps)", p.bars, p.arrangement_steps()),
                        "Total song arrangement length (Alt+h/l or ,/. ±1 bar · Alt+H/L or </> ±8 bars · Enter exact)",
                        true,
                    ),
                    SongPropRow::EndRule => (
                        "End Rule",
                        match p.end {
                            EndRule::Loop => "Loop".into(),
                            EndRule::Stop => "Stop".into(),
                            EndRule::Continue => "Continue".into(),
                        },
                        "Behavior at the end of the song arrangement (Loop / Stop / Continue)",
                        true,
                    ),
                    SongPropRow::LaunchQuantize => (
                        "Launch Quantize",
                        p.timing.launch.label(p),
                        "Musical boundary for live part launches & cues (Alt+h/l or ,/. cycle · c pick Cycle)",
                        true,
                    ),
                    SongPropRow::EditTiming => (
                        "Edit Timing",
                        match p.timing.edits {
                            EditTiming::Now => "Immediate (next step)".into(),
                            EditTiming::Launch => "Follow Launch Quantize".into(),
                        },
                        "When live song edits land during playback",
                        true,
                    ),
                    SongPropRow::LateTolerance => (
                        "Late Tolerance",
                        format!(
                            "{} step{}",
                            p.timing.late_tolerance_steps,
                            if p.timing.late_tolerance_steps == 1 {
                                ""
                            } else {
                                "s"
                            }
                        ),
                        "Grace window (0–4 steps) after a boundary that still lands immediately · Enter exact",
                        true,
                    ),
                };
                SongPropRowView {
                    group: row.group(),
                    label,
                    value,
                    detail,
                    selected: i == state.cursor,
                    adjustable,
                }
            })
            .collect();
        let summary = format!(
            "{}  ·  {:.1} BPM  ·  {} {}  ·  {} bars",
            p.title,
            p.bpm,
            note_name(p.root_midi),
            scale_label(&p.scale),
            p.bars
        );
        Some(SongPropertiesView {
            rows,
            title_editing: state.title_buf.clone(),
            dirty: self.dirty(),
            summary,
        })
    }

    fn adjust_song_property(
        &mut self,
        direction: i8,
        coarse: bool,
        coalesce: bool,
    ) -> Result<Outcome> {
        let row = SongPropRow::from_index(self.modal.song_props().map_or(0, |sp| sp.cursor));
        let p = self.doc.project();
        match row {
            SongPropRow::Title => Ok(Outcome::default()),
            SongPropRow::Tempo => {
                let delta = f64::from(direction) * if coarse { 10.0 } else { 1.0 };
                let bpm = ((p.bpm + delta) * 10.0).round() / 10.0;
                let bpm = bpm.clamp(20.0, 300.0);
                self.commit_coalesced(
                    vec![Edit::SetTempo { bpm }],
                    format!("Tempo → {bpm:.1} BPM"),
                    document::CoalesceKey::SongTempo,
                    coalesce,
                )
            }
            SongPropRow::KeyRoot => {
                let delta = i16::from(direction) * if coarse { 12 } else { 1 };
                let root_midi = (i16::from(p.root_midi) + delta).clamp(0, 127) as u8;
                let scale = p.scale.clone();
                let label = format!("Key → {} {}", note_name(root_midi), scale_label(&scale));
                self.commit_coalesced(
                    vec![Edit::SetKey { root_midi, scale }],
                    label,
                    document::CoalesceKey::SongKey,
                    coalesce,
                )
            }
            SongPropRow::Scale => {
                let steps = if coarse { 3 } else { 1 };
                let mut current = p.scale.as_slice();
                let mut spec = cycle_scale(current, direction);
                for _ in 1..steps {
                    current = spec.intervals;
                    spec = cycle_scale(current, direction);
                }
                let root_midi = p.root_midi;
                let label = format!("Key → {} {}", note_name(root_midi), spec.label);
                self.commit_coalesced(
                    vec![Edit::SetKey {
                        root_midi,
                        scale: spec.intervals.to_vec(),
                    }],
                    label,
                    document::CoalesceKey::SongKey,
                    coalesce,
                )
            }
            SongPropRow::Bars => {
                let delta = isize::from(direction) * if coarse { 8 } else { 1 };
                let bars = (p.bars as isize + delta).clamp(1, MAX_BARS as isize) as usize;
                self.commit_coalesced(
                    vec![Edit::SetBars { bars }],
                    format!("Arrangement length → {bars} bars"),
                    document::CoalesceKey::SongBars,
                    coalesce,
                )
            }
            SongPropRow::EndRule => {
                let next = if direction >= 0 {
                    p.end.next()
                } else {
                    match p.end {
                        EndRule::Loop => EndRule::Continue,
                        EndRule::Stop => EndRule::Loop,
                        EndRule::Continue => EndRule::Stop,
                    }
                };
                self.set_end_rule(next)
            }
            SongPropRow::LaunchQuantize => {
                let choices = self.live_boundary_choices();
                let current = p.timing.launch;
                let idx = choices.iter().position(|&b| b == current).unwrap_or(0) as isize;
                let next = choices
                    [(idx + isize::from(direction)).rem_euclid(choices.len() as isize) as usize];
                let mut timing = p.timing;
                timing.launch = next;
                let label = format!("Launch quantize → {}", next.label(p));
                self.commit_coalesced(
                    vec![Edit::SetLiveTiming { timing }],
                    label,
                    document::CoalesceKey::SongTiming,
                    coalesce,
                )
            }
            SongPropRow::EditTiming => self.toggle_edit_timing(),
            SongPropRow::LateTolerance => {
                let mut timing = p.timing;
                let max = MAX_LATE_TOLERANCE as isize;
                let next = (timing.late_tolerance_steps as isize + isize::from(direction))
                    .clamp(0, max) as u32;
                timing.late_tolerance_steps = next;
                let label = format!(
                    "Late boundary tolerance → {next} step{}",
                    if next == 1 { "" } else { "s" }
                );
                self.commit_coalesced(
                    vec![Edit::SetLiveTiming { timing }],
                    label,
                    document::CoalesceKey::SongTiming,
                    coalesce,
                )
            }
        }
    }

    fn reset_song_property(&mut self) -> Result<Outcome> {
        let row = SongPropRow::from_index(self.modal.song_props().map_or(0, |sp| sp.cursor));
        let defaults = Project::default();
        let p = self.doc.project();
        match row {
            SongPropRow::Title => Ok(Outcome::default()),
            SongPropRow::Tempo => self.commit(
                vec![Edit::SetTempo { bpm: defaults.bpm }],
                format!("Reset Tempo → {:.1} BPM", defaults.bpm),
            ),
            SongPropRow::KeyRoot => {
                let scale = p.scale.clone();
                let label = format!(
                    "Reset Key Root → {} {}",
                    note_name(defaults.root_midi),
                    scale_label(&scale)
                );
                self.commit(
                    vec![Edit::SetKey {
                        root_midi: defaults.root_midi,
                        scale,
                    }],
                    label,
                )
            }
            SongPropRow::Scale => {
                let root_midi = p.root_midi;
                let label = format!(
                    "Reset Scale → {} {}",
                    note_name(root_midi),
                    scale_label(&defaults.scale)
                );
                self.commit(
                    vec![Edit::SetKey {
                        root_midi,
                        scale: defaults.scale,
                    }],
                    label,
                )
            }
            SongPropRow::Bars => self.commit(
                vec![Edit::SetBars {
                    bars: defaults.bars,
                }],
                format!("Reset Arrangement length → {} bars", defaults.bars),
            ),
            SongPropRow::EndRule => self.set_end_rule(defaults.end),
            SongPropRow::LaunchQuantize => {
                let mut timing = p.timing;
                timing.launch = defaults.timing.launch;
                let label = format!("Reset Launch quantize → {}", timing.launch.label(p));
                self.commit(vec![Edit::SetLiveTiming { timing }], label)
            }
            SongPropRow::EditTiming => {
                let mut timing = p.timing;
                timing.edits = defaults.timing.edits;
                self.commit(
                    vec![Edit::SetLiveTiming { timing }],
                    "Reset Edit timing → default".into(),
                )
            }
            SongPropRow::LateTolerance => {
                let mut timing = p.timing;
                timing.late_tolerance_steps = defaults.timing.late_tolerance_steps;
                self.commit(
                    vec![Edit::SetLiveTiming { timing }],
                    format!(
                        "Reset Late tolerance → {} steps",
                        timing.late_tolerance_steps
                    ),
                )
            }
        }
    }

    pub(super) fn handle_song_properties(&mut self, key: KeyEvent) -> Result<Outcome> {
        let repeat = key.kind == KeyEventKind::Repeat;
        if let Some(mut buf) = self
            .modal
            .song_props_mut()
            .and_then(|sp| sp.title_buf.take())
        {
            match key.code {
                KeyCode::Esc => {
                    self.status = "Song rename cancelled".into();
                    return Ok(Outcome::default());
                }
                KeyCode::Enter if !repeat => {
                    let trimmed =
                        match drafts::form::validate_printable_name("Song title", &buf, 120) {
                            Ok(valid) => valid,
                            Err(err) => {
                                if let Some(sp) = self.modal.song_props_mut() {
                                    sp.title_buf = Some(buf);
                                }
                                self.status = format!("{err:#}");
                                return Ok(Outcome::default());
                            }
                        };
                    let out = self.commit(
                        vec![Edit::RenameProject {
                            title: trimmed.clone(),
                        }],
                        format!("Renamed song → '{trimmed}'"),
                    )?;
                    return Ok(out);
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    buf.clear();
                }
                KeyCode::Backspace => {
                    buf.pop();
                }
                KeyCode::Char(c)
                    if !c.is_control()
                        && !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                        && buf.chars().count() < 120 =>
                {
                    buf.push(c);
                }
                _ => {}
            }
            if let Some(sp) = self.modal.song_props_mut() {
                sp.title_buf = Some(buf);
            }
            return Ok(Outcome::default());
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) {
            if repeat {
                return Ok(Outcome::default());
            }
            let shifted = key.modifiers.contains(KeyModifiers::SHIFT)
                || matches!(key.code, KeyCode::Char(c) if c.is_ascii_uppercase());
            return match key.code {
                KeyCode::Char('r') => self.apply(Action::Redo),
                KeyCode::Char('z' | 'Z') if shifted => self.apply(Action::Redo),
                KeyCode::Char('z') => self.apply(Action::Undo),
                _ => Ok(Outcome::default()),
            };
        }

        if key.code == KeyCode::Esc {
            self.modal = ActiveModal::None;
            self.input.mode = Mode::Normal;
            self.status = "Closed Song Properties".into();
            return Ok(Outcome::default());
        }

        if !repeat {
            match key.code {
                KeyCode::Char('u') => return self.apply(Action::Undo),
                KeyCode::Char('U') => return self.apply(Action::Redo),
                _ => {}
            }
        }

        let max_idx = SongPropRow::ALL.len() - 1;
        let groups = SongPropRow::ALL.map(SongPropRow::group);

        let Some(cursor) = self.modal.song_props().map(|sp| sp.cursor) else {
            return Ok(Outcome::default());
        };
        let active_row = SongPropRow::from_index(cursor);

        // Tier 3 (Alt+h/l/H/L / Alt+Arrows) and universal reset (Alt+Backspace)
        if key.modifiers.contains(KeyModifiers::ALT) {
            let shifted = key.modifiers.contains(KeyModifiers::SHIFT);
            return match key.code {
                KeyCode::Backspace if !repeat => self.reset_song_property(),
                KeyCode::Char('h') | KeyCode::Left if !shifted => {
                    self.adjust_song_property(-1, false, repeat)
                }
                KeyCode::Char('l') | KeyCode::Right if !shifted => {
                    self.adjust_song_property(1, false, repeat)
                }
                KeyCode::Char('H') | KeyCode::Char('h') | KeyCode::Left => {
                    self.adjust_song_property(-1, true, repeat)
                }
                KeyCode::Char('L') | KeyCode::Char('l') | KeyCode::Right => {
                    self.adjust_song_property(1, true, repeat)
                }
                _ => Ok(Outcome::default()),
            };
        }

        match key.code {
            // Tier 4 (,/. fine, </> coarse) direct 1D scalar adjustment
            KeyCode::Char(',') => return self.adjust_song_property(-1, false, repeat),
            KeyCode::Char('.') => return self.adjust_song_property(1, false, repeat),
            KeyCode::Char('<') => return self.adjust_song_property(-1, true, repeat),
            KeyCode::Char('>') => return self.adjust_song_property(1, true, repeat),
            // Tier 1 vertical navigation (bare h/l strictly no-op)
            KeyCode::Char('j') | KeyCode::Down => {
                if let Some(sp) = self.modal.song_props_mut() {
                    sp.cursor = (sp.cursor + 1).min(max_idx);
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let Some(sp) = self.modal.song_props_mut() {
                    sp.cursor = sp.cursor.saturating_sub(1);
                }
            }
            KeyCode::Char('w') => {
                if let Some(next) = crate::jump_group_index(&groups, cursor, true) {
                    if let Some(sp) = self.modal.song_props_mut() {
                        sp.cursor = next;
                    }
                }
            }
            KeyCode::Char('b') => {
                if let Some(next) = crate::jump_group_index(&groups, cursor, false) {
                    if let Some(sp) = self.modal.song_props_mut() {
                        sp.cursor = next;
                    }
                }
            }
            KeyCode::Char('0') => {
                if let Some(sp) = self.modal.song_props_mut() {
                    sp.cursor = 0;
                }
            }
            KeyCode::Char('$' | 'G') | KeyCode::End => {
                if let Some(sp) = self.modal.song_props_mut() {
                    sp.cursor = max_idx;
                }
            }
            KeyCode::PageUp => {
                if let Some(sp) = self.modal.song_props_mut() {
                    sp.cursor = sp.cursor.saturating_sub(6);
                }
            }
            KeyCode::PageDown => {
                if let Some(sp) = self.modal.song_props_mut() {
                    sp.cursor = (sp.cursor + 6).min(max_idx);
                }
            }
            KeyCode::Char('c') if !repeat && active_row == SongPropRow::LaunchQuantize => {
                return self.open_cycle_boundary_picker();
            }
            KeyCode::Enter if !repeat => {
                return match active_row {
                    SongPropRow::Title => {
                        let title = self.doc.project().title.clone();
                        if let Some(sp) = self.modal.song_props_mut() {
                            sp.title_buf = Some(title);
                        }
                        self.status =
                            "Rename Song · Enter saves · Ctrl+u clear · Esc cancels".into();
                        Ok(Outcome::default())
                    }
                    SongPropRow::Tempo => self.begin_song_bpm_value_input(String::new()),
                    SongPropRow::KeyRoot => self.begin_song_key_root_value_input(String::new()),
                    SongPropRow::Bars => self.begin_song_bars_value_input(String::new()),
                    SongPropRow::LateTolerance => {
                        self.begin_late_tolerance_value_input(String::new(), Mode::SongProperties)
                    }
                    _ => self.adjust_song_property(1, false, false),
                };
            }
            _ => {}
        }
        Ok(Outcome::default())
    }
}
