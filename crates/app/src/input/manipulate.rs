//! Modeless direct manipulation (Tier 3: `Alt+hjkl`/`Alt+HJKL`/`Alt+Arrows`/`Alt+Shift+Arrows`),
//! one-hand scalar adjustment (Tier 4: `,`/`.` & `<`/`>`), and universal reset (`Alt+Backspace`).
//!
//! Every continuous adjustment supports key-repeat and coalesces repeats into a single undo step.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Axis {
    Horiz,
    Vert,
}

impl App {
    pub(crate) fn focused_effect_for_reorder(&self) -> Option<Id> {
        match self.detail {
            Detail::Effect(id) => Some(id),
            Detail::Channel(_) => self
                .control_descriptors()
                .get(self.control_cursor)
                .and_then(|c| match c.target {
                    ControlTarget::Effect(id) => Some(id),
                    _ => None,
                }),
            _ => None,
        }
    }

    /// Tier 3 direct 2D manipulation & vertical reordering (`Alt+hjkl/HJKL/Arrows`) and
    /// universal reset (`Alt+Backspace`).
    pub(super) fn handle_direct_manipulate(
        &mut self,
        key: KeyEvent,
        count: usize,
    ) -> Result<Outcome> {
        let repeat = key.kind == KeyEventKind::Repeat;
        if key.code == KeyCode::Backspace {
            if repeat {
                return Ok(Outcome::default());
            }
            return self.handle_reset_focused();
        }

        let shifted = key.modifiers.contains(KeyModifiers::SHIFT)
            || matches!(key.code, KeyCode::Char(c) if c.is_ascii_uppercase());
        let (axis, dir, coarse) = match key.code {
            KeyCode::Char('h' | 'H') | KeyCode::Left => (Axis::Horiz, -1i8, shifted),
            KeyCode::Char('l' | 'L') | KeyCode::Right => (Axis::Horiz, 1i8, shifted),
            KeyCode::Char('j' | 'J') | KeyCode::Down => (Axis::Vert, -1i8, shifted),
            KeyCode::Char('k' | 'K') | KeyCode::Up => (Axis::Vert, 1i8, shifted),
            _ => return Ok(Outcome::default()),
        };

        if self.macros_focused() {
            return match axis {
                Axis::Horiz => self.nudge_macro_focused(dir, coarse, repeat),
                Axis::Vert if !repeat && self.view_state.macro_binding_cursor.is_some() => {
                    // Up (`k` / dir > 0) moves row earlier (-1); Down (`j` / dir < 0) moves row later (+1).
                    self.reorder_macro_binding(if dir > 0 { -1 } else { 1 })
                }
                Axis::Vert => Ok(Outcome::default()),
            };
        }

        if self.midi_focused() {
            return match axis {
                Axis::Horiz if !repeat => {
                    if self.devices.tab == DevicesTab::Audio {
                        self.adjust_audio_deck_row(isize::from(dir))
                    } else {
                        self.adjust_midi_deck_row(isize::from(dir))
                    }
                }
                _ => Ok(Outcome::default()),
            };
        }

        if self.deck_focused() {
            if let Detail::Instrument(id) = self.detail {
                if self.rack_ui.tab == RackTab::Patchbay {
                    return match axis {
                        Axis::Horiz => self.nudge_patchbay_cable(id, dir, coarse, repeat),
                        Axis::Vert => Ok(Outcome::default()),
                    };
                }
            }
            return match axis {
                Axis::Horiz => self.nudge_control(dir, coarse, repeat),
                Axis::Vert if !repeat => {
                    let delta = if dir > 0 { -1 } else { 1 };
                    match self.detail {
                        Detail::Channel(_) | Detail::Effect(_) => {
                            if let Some(fx_id) = self.focused_effect_for_reorder() {
                                return self.reorder_effect_in_channel(fx_id, delta);
                            }
                            Ok(Outcome::default())
                        }
                        Detail::Instrument(id) if self.rack_ui.tab == RackTab::Zones => {
                            self.reorder_rack_zone(id, delta)
                        }
                        Detail::Instrument(id) if self.rack_ui.tab == RackTab::Panel => {
                            self.reorder_panel_pin(id, delta)
                        }
                        _ => Ok(Outcome::default()),
                    }
                }
                Axis::Vert => Ok(Outcome::default()),
            };
        }

        if self.mixer_canvas_focused() {
            self.clamp_mixer_slot();
            return match axis {
                Axis::Horiz => self.nudge_mixer_slot(dir, coarse, repeat),
                Axis::Vert => match self.view_state.mixer_slot {
                    MixerSlot::Fader => self.nudge_param(
                        ParamAddress::Channel {
                            channel: self.selected_channel,
                            param: ChannelParam::Gain,
                        },
                        dir,
                        coarse,
                        repeat,
                    ),
                    MixerSlot::Insert(idx) if !repeat => {
                        if let Some(fx_id) = self.mixer_focused_insert(idx) {
                            self.reorder_effect_in_channel(fx_id, if dir > 0 { -1 } else { 1 })
                        } else {
                            Ok(Outcome::default())
                        }
                    }
                    _ => Ok(Outcome::default()),
                },
            };
        }

        if self.bank_focused() && self.view_state.bank_mode == BankMode::Parts {
            return match axis {
                Axis::Vert if !repeat => {
                    let delta = if dir > 0 { -1 } else { 1 };
                    if let Some(lane) = self
                        .doc
                        .project()
                        .lanes
                        .get(self.view_state.bank_lane_idx)
                        .map(|l| l.id)
                    {
                        self.reorder_lane_by(lane, delta)
                    } else {
                        Ok(Outcome::default())
                    }
                }
                _ => Ok(Outcome::default()),
            };
        }

        if self.arrange_canvas_focused() {
            if self.arrange_ui.view == ArrangeView::Placements {
                if let Some(id) = self.selected_placement {
                    return match axis {
                        Axis::Horiz => {
                            let shift = i64::from(dir) * if coarse { 16 } else { 4 };
                            self.reshape_placement(id, shift, 0, repeat)
                        }
                        Axis::Vert => {
                            let lane_delta = if dir > 0 { -1 } else { 1 };
                            self.move_placement_lane(id, lane_delta, repeat)
                        }
                    };
                }
            }
            if axis == Axis::Vert && !repeat {
                if let Some(lane_id) = self.selected_lane {
                    let delta = if dir > 0 { -1 } else { 1 };
                    return self.reorder_lane_by(lane_id, delta);
                }
            }
            if self.arrange_ui.view == ArrangeView::Placements && self.selected_placement.is_none()
            {
                self.status = "No placement under cursor · Enter places a part".into();
            }
            return Ok(Outcome::default());
        }

        if self.pattern_canvas_focused() {
            if self.value_canvas() {
                return match axis {
                    Axis::Vert => self.nudge_value(dir, count, coarse, repeat),
                    Axis::Horiz => {
                        let delta = isize::from(dir) * if coarse { 4 } else { 1 };
                        self.slide_value_point(delta, repeat)
                    }
                };
            }
            if self.can_edit_current_pattern_notes() {
                return match axis {
                    Axis::Vert => {
                        let deg = i16::from(dir) * if coarse { 7 } else { 1 };
                        self.edit_selected_voice_pitch(deg, repeat)
                    }
                    Axis::Horiz => {
                        let gate = f32::from(dir) * if coarse { 1.0 } else { 0.25 };
                        self.edit_selected_voice_gate(gate, repeat)
                    }
                };
            }
        }

        Ok(Outcome::default())
    }

    /// Tier 4 one-hand 1D scalar adjustment (`,` / `.` fine, `<` / `>` coarse) with key-repeat
    /// and coalesced undo across every editable deck.
    pub(super) fn handle_scalar_adjust(
        &mut self,
        direction: i8,
        coarse: bool,
        repeat: bool,
        count: usize,
    ) -> Result<Outcome> {
        if self.macros_focused() {
            return self.nudge_macro_focused(direction, coarse, repeat);
        }
        if self.midi_focused() {
            if repeat {
                return Ok(Outcome::default());
            }
            return if self.devices.tab == DevicesTab::Audio {
                self.adjust_audio_deck_row(isize::from(direction))
            } else {
                self.adjust_midi_deck_row(isize::from(direction))
            };
        }
        if self.deck_focused() {
            if let Detail::Instrument(id) = self.detail {
                if self.rack_ui.tab == RackTab::Patchbay {
                    return self.nudge_patchbay_cable(id, direction, coarse, repeat);
                }
            }
            return self.nudge_control(direction, coarse, repeat);
        }
        if self.mixer_canvas_focused() {
            self.clamp_mixer_slot();
            return self.nudge_mixer_slot(direction, coarse, repeat);
        }
        if self.arrange_canvas_focused() && self.arrange_ui.view == ArrangeView::Placements {
            let Some(id) = self.selected_placement else {
                self.status = "No placement under cursor · Enter places a part".into();
                return Ok(Outcome::default());
            };
            let resize = i64::from(direction) * if coarse { 16 } else { 4 };
            return self.reshape_placement(id, 0, resize, repeat);
        }
        if self.pattern_canvas_focused() {
            if self.value_canvas() {
                return self.nudge_value(direction, count, coarse, repeat);
            }
            if self.can_edit_current_pattern_notes() {
                let delta = f32::from(direction) * if coarse { 0.15 } else { 0.05 };
                return self.edit_selected_voice_velocity(delta, repeat);
            }
        }
        Ok(Outcome::default())
    }

    /// Universal reset-to-default (`Alt+Backspace`) for the focused scalar parameter across
    /// control decks, Mixer, Macros, and Rack Patchbay.
    pub(super) fn handle_reset_focused(&mut self) -> Result<Outcome> {
        if self.macros_focused() {
            return self.reset_macro_focused();
        }
        if self.midi_focused() {
            return if self.devices.tab == DevicesTab::Audio {
                self.reset_audio_deck_row()
            } else {
                self.reset_midi_deck_row()
            };
        }
        if self.deck_focused() {
            if let Detail::Instrument(id) = self.detail {
                if self.rack_ui.tab == RackTab::Patchbay {
                    return self.reset_patchbay_cable(id);
                }
            }
            return self.reset_focused_control();
        }
        if self.mixer_canvas_focused() {
            self.clamp_mixer_slot();
            return match self.view_state.mixer_slot {
                MixerSlot::Fader => self.reset_param_default(ParamAddress::Channel {
                    channel: self.selected_channel,
                    param: ChannelParam::Gain,
                }),
                MixerSlot::Pan => self.reset_param_default(ParamAddress::Channel {
                    channel: self.selected_channel,
                    param: ChannelParam::Pan,
                }),
                MixerSlot::Send(idx) => {
                    if let Some((ch, target)) = self.mixer_focused_send(idx) {
                        self.reset_param_default(ParamAddress::Send {
                            channel: ChannelRef::Channel(ch),
                            target,
                        })
                    } else {
                        Ok(Outcome::default())
                    }
                }
                MixerSlot::Duck(idx) => {
                    if let Some(id) = self.mixer_focused_duck(idx) {
                        let mut duck = self
                            .doc
                            .project()
                            .ducks
                            .iter()
                            .find(|d| d.id == id)
                            .context("Duck missing")?
                            .clone();
                        duck.depth = 0.0;
                        let label = format!("Reset {} depth → 0%", duck.name);
                        self.commit(vec![Edit::PutDuck { duck }], label)
                    } else {
                        Ok(Outcome::default())
                    }
                }
                MixerSlot::Insert(_) | MixerSlot::Output => Ok(Outcome::default()),
            };
        }
        Ok(Outcome::default())
    }

    /// Single source of truth for adjusting the focused Mixer strip slot (step sizes aligned
    /// with `Detail::Channel` via `nudge_param`).
    fn nudge_mixer_slot(&mut self, direction: i8, coarse: bool, coalesce: bool) -> Result<Outcome> {
        match self.view_state.mixer_slot {
            MixerSlot::Fader => self.nudge_param(
                ParamAddress::Channel {
                    channel: self.selected_channel,
                    param: ChannelParam::Gain,
                },
                direction,
                coarse,
                coalesce,
            ),
            MixerSlot::Pan => self.nudge_param(
                ParamAddress::Channel {
                    channel: self.selected_channel,
                    param: ChannelParam::Pan,
                },
                direction,
                coarse,
                coalesce,
            ),
            MixerSlot::Send(idx) => {
                if let Some((ch, target)) = self.mixer_focused_send(idx) {
                    self.nudge_param(
                        ParamAddress::Send {
                            channel: ChannelRef::Channel(ch),
                            target,
                        },
                        direction,
                        coarse,
                        coalesce,
                    )
                } else {
                    Ok(Outcome::default())
                }
            }
            MixerSlot::Output => {
                if let ChannelRef::Channel(cid) = self.selected_channel {
                    self.cycle_channel_output(cid, direction)
                } else {
                    Ok(Outcome::default())
                }
            }
            MixerSlot::Duck(idx) => {
                if let Some(id) = self.mixer_focused_duck(idx) {
                    self.nudge_duck_depth(id, direction, coarse, coalesce)
                } else {
                    Ok(Outcome::default())
                }
            }
            MixerSlot::Insert(_) => Ok(Outcome::default()),
        }
    }
}
