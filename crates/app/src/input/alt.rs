//! The Alt live layer: performance controls that work the same in every mode,
//! including Insert, forms, and overlays. Jam (`gj` / `Ctrl+Shift+j`) is this layer
//! locked on, so there the section and lane keys need no Alt.
//!
//! `Alt+h/j/k/l`, `Alt+H/J/K/L`, `Alt+Arrows`, `Alt+Shift+Arrows`, and `Alt+Backspace`
//! are reserved for direct 2D manipulation and reset and return `None` here.
use super::*;

impl App {
    /// Handle an Alt live-performance chord, or return `None` for keys outside the live layer.
    pub(crate) fn handle_alt(&mut self, key: KeyEvent) -> Option<Result<Outcome>> {
        if key.code == KeyCode::Enter {
            return Some(if key.kind == KeyEventKind::Repeat {
                Ok(Outcome::default())
            } else {
                self.toggle_staging()
            });
        }
        let KeyCode::Char(c) = key.code else {
            return None;
        };
        let every = c.is_ascii_uppercase() || key.modifiers.contains(KeyModifiers::SHIFT);
        let c = c.to_ascii_lowercase();
        if !(SECTION_KEYS.contains(&c) || LANE_KEYS.contains(&c) || "msxc".contains(c)) {
            return None;
        }
        if key.kind == KeyEventKind::Repeat {
            return Some(Ok(Outcome::default()));
        }
        self.input.count = None;
        // Committing performance keeps while a live-preview browser (Preset or Filter Groove)
        // is open must restore the pre-preview project first so an uncommitted preview can never
        // leak or overwrite later edits on quit.
        let mut pre_out = Outcome::default();
        if self.has_preview_browser() && c == 'c' {
            match self.cancel_active_preview_browser() {
                Ok(out) => pre_out = out,
                Err(err) => return Some(Err(err)),
            }
        }
        let result = match c {
            'm' if every => self.clear_every_lane_flag(false, "Unmuted every lane"),
            's' if every => self.clear_every_lane_flag(true, "Cleared every solo"),
            'x' if every => self.apply(Action::ReleaseAll { at: None }),
            'c' if every => self.keep_all_overrides(),
            'c' if self.take_focused() => self.keep_take_selection(),
            'c' => self
                .focused_lane()
                .context("Name a lane to keep")
                .and_then(|lane| self.keep_override(lane)),
            'm' => self.toggle_focused_mute(),
            's' => self.toggle_focused_solo(),
            'x' => self
                .focused_lane()
                .context("Name a lane to release")
                .and_then(|lane| self.apply(Action::Release { lane, at: None })),
            c if SECTION_KEYS.contains(&c) => self.number_key(c),
            c => self.open_lane_layer(c),
        };
        Some(result.map(|mut out| {
            out.merge(pre_out);
            out
        }))
    }

    /// In the Mixer, mute acts on the selected channel until a lane is named.
    fn mixer_channel_target(&self) -> bool {
        self.mixer_canvas_focused() && self.input.focused_lane.is_none()
    }

    fn toggle_focused_mute(&mut self) -> Result<Outcome> {
        if self.mixer_channel_target() {
            ensure!(
                self.strip(self.selected_channel).is_some(),
                "Select a channel"
            );
            return self.apply(Action::SetChannelMuted {
                channel: self.selected_channel,
                muted: !self.doc.flags().channel_muted(self.selected_channel),
            });
        }
        let lane = self.focused_lane().context("Name a lane to mute")?;
        ensure!(self.lane(lane).is_some(), "Lane missing");
        let muted = !self.effective_lane_muted(lane);
        self.apply(Action::SetLaneMuted { lane, muted })
    }

    fn toggle_focused_solo(&mut self) -> Result<Outcome> {
        if self.mixer_channel_target() {
            ensure!(
                self.strip(self.selected_channel).is_some(),
                "Select a channel"
            );
            return self.apply(Action::SetChannelSolo {
                channel: self.selected_channel,
                solo: !self.doc.flags().channel_solo(self.selected_channel),
            });
        }
        let lane = self.focused_lane().context("Name a lane to solo")?;
        ensure!(self.lane(lane).is_some(), "Lane missing");
        let solo = !self.effective_lane_solo(lane);
        self.apply(Action::SetLaneSolo { lane, solo })
    }

    /// Alt+Enter starts staging; when staging is empty it exits cleanly (matching `S`);
    /// when changes are staged it opens the Stage finish prompt (and a repeat Alt+Enter fires).
    fn toggle_staging(&mut self) -> Result<Outcome> {
        match self.staging.as_deref() {
            None => self.apply(Action::Stage),
            Some([]) => {
                if self.input.mode == Mode::Stage {
                    self.input.mode = self.take_resume_mode();
                }
                let out = self.apply(Action::FinishStaging {
                    choice: StageChoice::Discard,
                })?;
                self.status = "Exited staging".into();
                Ok(out)
            }
            Some(_) if self.input.mode == Mode::Stage => {
                self.input.mode = self.take_resume_mode();
                self.apply(Action::FinishStaging {
                    choice: StageChoice::Fire,
                })
            }
            Some(_) => {
                self.input.resume_mode = self.is_jam().then_some(Mode::Jam);
                self.input.mode = Mode::Stage;
                self.status =
                    "Staged change · Enter/S fire on the boundary · x discard · Esc keep staging"
                        .into();
                Ok(Outcome::default())
            }
        }
    }
}
