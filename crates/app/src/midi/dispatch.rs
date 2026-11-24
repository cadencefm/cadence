//! MIDI mapping execution: transport, launches, mutes/solos, Project Macros, continuous params, and tap tempo.
use super::{PICKUP_TOLERANCE, RELATIVE_STEP};
use crate::*;
use cadence_core::midi_map::{
    MidiFeedback, MidiMapping, MidiSource, MidiTarget, MidiTransportAction, MidiValueMode,
};
use cadence_core::runtime::MidiMessage;
use std::time::Instant;

enum ContinuousMidiStep {
    Ignore,
    PickupWait(MidiFeedback, String),
    Apply(f32),
}

impl App {
    /// Shared continuous MIDI value decoder + soft-pickup takeover gate for both
    /// `ParamAddress` targets and Project Macro targets (single point of truth).
    fn resolve_midi_continuous_value(
        &mut self,
        mapping: &MidiMapping,
        raw_u8: u8,
        current: f32,
        pickup_hint_label: &str,
        pickup_display_label: &str,
    ) -> ContinuousMidiStep {
        match mapping.mode {
            MidiValueMode::RelativeSigned | MidiValueMode::RelativeOffset64 => {
                let Some(delta) = mapping.mode.decode_relative_delta(raw_u8) else {
                    return ContinuousMidiStep::Ignore;
                };
                let sign = if mapping.invert { -1.0 } else { 1.0 };
                let next = (current + f32::from(delta) * RELATIVE_STEP * sign)
                    .clamp(mapping.min.min(mapping.max), mapping.min.max(mapping.max))
                    .clamp(0.0, 1.0);
                ContinuousMidiStep::Apply(next)
            }
            MidiValueMode::Toggle => {
                if raw_u8 == 0 {
                    return ContinuousMidiStep::Ignore;
                }
                ContinuousMidiStep::Apply(if current < 0.5 {
                    mapping.max
                } else {
                    mapping.min
                })
            }
            MidiValueMode::Momentary => {
                ContinuousMidiStep::Apply(if raw_u8 > 0 { mapping.max } else { mapping.min })
            }
            MidiValueMode::Trigger => {
                if raw_u8 == 0 {
                    return ContinuousMidiStep::Ignore;
                }
                ContinuousMidiStep::Apply(mapping.max)
            }
            MidiValueMode::Absolute => {
                let raw_norm = f32::from(raw_u8) / 127.0;
                let mapped = mapping.map_normalized(raw_norm);
                if mapping.pickup && self.config.midi_pickup {
                    let latched = self
                        .midi
                        .pickup_latched
                        .get(&mapping.id)
                        .copied()
                        .unwrap_or(false);
                    if !latched {
                        if (mapped - current).abs() <= PICKUP_TOLERANCE {
                            self.midi.pickup_latched.insert(mapping.id, true);
                        } else {
                            let arrow = if mapped < current { "->" } else { "<-" };
                            let hint = format!(
                                "Pickup {pickup_hint_label} {arrow} {:.0}%",
                                current * 100.0
                            );
                            self.status = hint.clone();
                            let fb = MidiFeedback::Display {
                                line1: format!("Pickup {pickup_display_label}"),
                                line2: format!("{arrow} {:.0}%", current * 100.0),
                                value_bar: Some((current * 127.0).round() as u8),
                            };
                            return ContinuousMidiStep::PickupWait(fb, hint);
                        }
                    }
                }
                ContinuousMidiStep::Apply(mapped)
            }
        }
    }

    /// Write a normalized `0.0..=1.0` value to any `ParamAddress` via the validated coalescing path.
    /// Returns `(Outcome, short_name, formatted_value)`.
    fn apply_param_normalized(
        &mut self,
        address: ParamAddress,
        norm: f32,
    ) -> Result<(Outcome, String, String)> {
        let (native, short_name, value_str) =
            self.resolve_param_normalized_display(address, norm)?;
        let label = self.describe_param(address, native);
        let out = self.commit_param(address, native, label, true)?;
        Ok((out, short_name, value_str))
    }

    pub(super) fn dispatch_midi_mapping(
        &mut self,
        mapping: &MidiMapping,
        message: MidiMessage,
    ) -> Result<(Outcome, String)> {
        let raw_u8 = match message {
            MidiMessage::Control { value, .. } => value,
            MidiMessage::NoteOn { velocity, .. } => velocity,
            MidiMessage::NoteOff { .. } => 0,
            MidiMessage::PitchBend { value, .. } => {
                (((i32::from(value) + 8192) * 127) / 16383).clamp(0, 127) as u8
            }
            _ => return Ok((Outcome::default(), String::new())),
        };

        // Handle transport & discrete performance targets first.
        match mapping.target {
            MidiTarget::Transport { action } => {
                return self.dispatch_midi_transport(mapping, action, raw_u8);
            }
            MidiTarget::LaunchSection { index } => {
                if raw_u8 > 0 {
                    let mut out = self.trigger_section_jump(index)?;
                    let label = format!("Section {}", index + 1);
                    out.midi_feedback.push(MidiFeedback::Display {
                        line1: "Launch Section".into(),
                        line2: label.clone(),
                        value_bar: None,
                    });
                    return Ok((out, label));
                }
                return Ok((Outcome::default(), String::new()));
            }
            MidiTarget::LaunchLanePart {
                lane_index,
                part_index,
            } => {
                if raw_u8 > 0 {
                    if let Some(lane) = self.doc.project().lanes.get(lane_index) {
                        let lane_id = lane.id;
                        if let Some(&lp) = lane.parts.get(part_index) {
                            let label = self.lane_part_label(lane_id, lp.part);
                            let mut out = self.apply(Action::Launch {
                                lane: lane_id,
                                part: lp,
                                at: None,
                            })?;
                            out.midi_feedback.push(MidiFeedback::Display {
                                line1: "Launch Part".into(),
                                line2: label.clone(),
                                value_bar: None,
                            });
                            return Ok((out, label));
                        }
                    }
                }
                return Ok((Outcome::default(), String::new()));
            }
            MidiTarget::MuteLane { lane_index } => {
                let Some(lane) = self.doc.project().lanes.get(lane_index) else {
                    return Ok((Outcome::default(), String::new()));
                };
                let lane_id = lane.id;
                let lane_name = lane.name.clone();
                let next_muted = match mapping.mode {
                    MidiValueMode::Momentary => raw_u8 > 0,
                    _ if raw_u8 > 0 => !self.effective_lane_muted(lane_id),
                    _ => return Ok((Outcome::default(), String::new())),
                };
                let mut out = self.apply(Action::SetLaneMuted {
                    lane: lane_id,
                    muted: next_muted,
                })?;
                let label = format!(
                    "{lane_name} {}",
                    if next_muted { "MUTED" } else { "ACTIVE" }
                );
                out.midi_feedback.push(MidiFeedback::Display {
                    line1: lane_name,
                    line2: if next_muted { "Muted" } else { "Unmuted" }.into(),
                    value_bar: None,
                });
                return Ok((out, label));
            }
            MidiTarget::SoloLane { lane_index } => {
                let Some(lane) = self.doc.project().lanes.get(lane_index) else {
                    return Ok((Outcome::default(), String::new()));
                };
                let lane_id = lane.id;
                let lane_name = lane.name.clone();
                let next_solo = match mapping.mode {
                    MidiValueMode::Momentary => raw_u8 > 0,
                    _ if raw_u8 > 0 => !self.effective_lane_solo(lane_id),
                    _ => return Ok((Outcome::default(), String::new())),
                };
                let mut out = self.apply(Action::SetLaneSolo {
                    lane: lane_id,
                    solo: next_solo,
                })?;
                let label = format!("{lane_name} {}", if next_solo { "SOLO" } else { "OFF" });
                out.midi_feedback.push(MidiFeedback::Display {
                    line1: lane_name,
                    line2: if next_solo { "Solo ON" } else { "Solo OFF" }.into(),
                    value_bar: None,
                });
                return Ok((out, label));
            }
            _ => {}
        }

        // Project Macro targets (`MidiTarget::ProjectMacro` or `FocusedMacro` resolving to a Project Macro).
        if let Some(macro_idx) = self.resolve_target_project_macro(mapping.target) {
            return self.dispatch_midi_project_macro(mapping, message, raw_u8, macro_idx);
        }

        // Continuous parameter targets: Param, FocusedMacro, LaneGain.
        let Some(address) = self.resolve_target_param(mapping.target) else {
            return Ok((Outcome::default(), "no target".into()));
        };
        let current = self.read_param_normalized(address).unwrap_or(0.5);
        let next_norm = match self.resolve_midi_continuous_value(
            mapping,
            raw_u8,
            current,
            &mapping.label,
            &mapping.label,
        ) {
            ContinuousMidiStep::Ignore => return Ok((Outcome::default(), String::new())),
            ContinuousMidiStep::PickupWait(fb, hint) => {
                return Ok((
                    Outcome {
                        midi_feedback: vec![fb],
                        ..Outcome::default()
                    },
                    hint,
                ));
            }
            ContinuousMidiStep::Apply(v) => v,
        };

        let (mut out, short_name, val_str) = if self.midi.record
            && self.transport.playing
            && matches!(
                message,
                MidiMessage::Control { .. } | MidiMessage::PitchBend { .. }
            ) {
            match self.record_midi_cc_automation(address, next_norm)? {
                Some(recorded) => recorded,
                None => self.apply_param_normalized(address, next_norm)?,
            }
        } else {
            self.apply_param_normalized(address, next_norm)?
        };
        let bar = (next_norm.clamp(0.0, 1.0) * 127.0).round() as u8;
        out.midi_feedback.push(MidiFeedback::Display {
            line1: short_name.clone(),
            line2: val_str.clone(),
            value_bar: Some(bar),
        });
        if let MidiSource::Cc { channel, cc } = mapping.source {
            out.midi_feedback.push(MidiFeedback::CcValue {
                channel: channel.unwrap_or(0),
                cc,
                value: bar,
            });
        }
        Ok((out, format!("{short_name} = {val_str}")))
    }

    fn dispatch_midi_project_macro(
        &mut self,
        mapping: &MidiMapping,
        message: MidiMessage,
        raw_u8: u8,
        macro_idx: u8,
    ) -> Result<(Outcome, String)> {
        let current = self
            .doc
            .project()
            .macro_control(macro_idx)
            .map_or(0.0, |m| m.value);
        let macro_name = self.doc.project().macro_control(macro_idx).map_or_else(
            || cadence_core::macros::MacroControl::default_name(macro_idx),
            |m| m.name.clone(),
        );
        let hint_label = format!("M{} {macro_name}", usize::from(macro_idx) + 1);
        let next_norm = match self.resolve_midi_continuous_value(
            mapping,
            raw_u8,
            current,
            &hint_label,
            &macro_name,
        ) {
            ContinuousMidiStep::Ignore => return Ok((Outcome::default(), String::new())),
            ContinuousMidiStep::PickupWait(fb, hint) => {
                return Ok((
                    Outcome {
                        midi_feedback: vec![fb],
                        ..Outcome::default()
                    },
                    hint,
                ));
            }
            ContinuousMidiStep::Apply(v) => v,
        };

        let (mut out, short_name, val_str) = if self.midi.record
            && self.transport.playing
            && matches!(
                message,
                MidiMessage::Control { .. } | MidiMessage::PitchBend { .. }
            ) {
            self.record_midi_macro_automation(macro_idx, next_norm)?
        } else {
            self.apply_macro_value_midi(macro_idx, next_norm)?
        };
        let bar = (next_norm.clamp(0.0, 1.0) * 127.0).round() as u8;
        let line1 = format!("M{} {short_name}", usize::from(macro_idx) + 1);
        out.midi_feedback.push(MidiFeedback::Display {
            line1: line1.clone(),
            line2: val_str.clone(),
            value_bar: Some(bar),
        });
        if let MidiSource::Cc { channel, cc } = mapping.source {
            out.midi_feedback.push(MidiFeedback::CcValue {
                channel: channel.unwrap_or(0),
                cc,
                value: bar,
            });
        }
        Ok((out, format!("{line1} = {val_str}")))
    }

    fn dispatch_midi_transport(
        &mut self,
        mapping: &MidiMapping,
        action: MidiTransportAction,
        raw_u8: u8,
    ) -> Result<(Outcome, String)> {
        if action == MidiTransportAction::NavigateControl {
            let delta = mapping
                .mode
                .decode_relative_delta(raw_u8)
                .unwrap_or(if raw_u8 >= 64 { 1 } else { -1 });
            let steps = delta.unsigned_abs().max(1) as usize;
            let down = (delta > 0) ^ mapping.invert;
            let mut line1 = "Navigate".to_string();
            let mut line2 = String::new();
            if self.macros_focused() {
                let max = cadence_core::macros::MAX_PROJECT_MACROS - 1;
                self.view_state.macro_cursor = if down {
                    (self.view_state.macro_cursor + steps).min(max)
                } else {
                    self.view_state.macro_cursor.saturating_sub(steps)
                };
                let slot = self.view_state.macro_cursor as u8;
                let (name, val) = self.doc.project().macro_control(slot).map_or_else(
                    || (cadence_core::macros::MacroControl::default_name(slot), 0.0),
                    |m| (m.name.clone(), m.value),
                );
                line1 = format!("M{} {name}", self.view_state.macro_cursor + 1);
                line2 = format!("{:.0}%", val * 100.0);
            } else if self.midi_focused() && self.devices.tab == DevicesTab::Audio {
                let total = AudioDeckRow::ALL.len();
                self.devices.cursor = if down {
                    (self.devices.cursor + steps).min(total - 1)
                } else {
                    self.devices.cursor.saturating_sub(steps)
                };
                if let Some(row) = AudioDeckRow::from_index(self.devices.cursor) {
                    line1 = "Audio Device".into();
                    line2 = row.label().into();
                }
            } else if self.midi_focused() {
                let mappings = self.effective_midi_mappings();
                let total = super::MidiDeckRow::COUNT + mappings.len();
                self.midi.cursor = if down {
                    (self.midi.cursor + steps).min(total.saturating_sub(1))
                } else {
                    self.midi.cursor.saturating_sub(steps)
                };
                match super::MidiDeckRow::from_index(self.midi.cursor) {
                    Some(super::MidiDeckRow::In) => {
                        line1 = "MIDI IN".into();
                        line2 = self.midi.port.clone().unwrap_or_else(|| "Off".into());
                    }
                    Some(super::MidiDeckRow::Out) => {
                        line1 = "MIDI OUT".into();
                        line2 = self.midi.out_port.clone().unwrap_or_else(|| "Off".into());
                    }
                    Some(super::MidiDeckRow::Profile) => {
                        line1 = "Profile".into();
                        line2 = self.active_midi_profile().name;
                    }
                    Some(super::MidiDeckRow::Pads) => {
                        line1 = "Pad Mode".into();
                        line2 = self.config.midi_pad_mode.label().into();
                    }
                    Some(super::MidiDeckRow::Pickup) => {
                        line1 = "Soft Pickup".into();
                        line2 = if self.config.midi_pickup { "On" } else { "Off" }.into();
                    }
                    None => {
                        if let Some((m, _)) =
                            mappings.get(self.midi.cursor - super::MidiDeckRow::COUNT)
                        {
                            line1 = m.source.short_badge();
                            line2 = m.label.clone();
                        }
                    }
                }
            } else if self.deck_focused() {
                let controls = self.control_descriptors();
                if !controls.is_empty() {
                    self.control_cursor = if down {
                        (self.control_cursor + steps).min(controls.len() - 1)
                    } else {
                        self.control_cursor.saturating_sub(steps)
                    };
                    if let Some(c) = controls.get(self.control_cursor) {
                        line1 = c.label.clone();
                        line2 = c.display.clone();
                    }
                }
            } else {
                self.move_selection(if down {
                    steps as isize
                } else {
                    -(steps as isize)
                });
                line1 = self.deck_label(
                    self.layout
                        .focused()
                        .unwrap_or(&layout::Deck::song(layout::DeckKind::Arrange)),
                );
                line2 = self.status.clone();
            }
            let mut out = Outcome::default();
            out.midi_feedback.push(MidiFeedback::Display {
                line1: line1.clone(),
                line2: line2.clone(),
                value_bar: None,
            });
            return Ok((out, format!("{line1} · {line2}")));
        }

        if raw_u8 == 0 {
            return Ok((Outcome::default(), String::new()));
        }
        let mut out = match action {
            MidiTransportAction::PlayPause => self.apply(Action::Play {
                playing: !self.transport.playing,
            })?,
            MidiTransportAction::Stop => self.apply(Action::Stop)?,
            MidiTransportAction::Record => self.toggle_midi_record(),
            MidiTransportAction::LoopToggle => {
                if self.playing_loop().is_some() {
                    self.clear_loop_region()?
                } else {
                    self.loop_section_at_cursor()
                        .unwrap_or_else(|_| Outcome::default())
                }
            }
            MidiTransportAction::TapTempo => self.handle_tap_tempo()?,
            MidiTransportAction::PrevSection | MidiTransportAction::NextSection => {
                let count = self.doc.project().sections.len();
                if count > 0 {
                    let cur = self.current_section_index().unwrap_or(0);
                    let next = if action == MidiTransportAction::NextSection {
                        (cur + 1) % count
                    } else {
                        (cur + count - 1) % count
                    };
                    self.trigger_section_jump(next)?
                } else {
                    Outcome::default()
                }
            }
            MidiTransportAction::ConfirmControl => {
                if self.macros_focused() {
                    self.toggle_midi_learn()
                } else if self.midi_focused() {
                    if self.devices.tab == DevicesTab::Audio {
                        self.activate_audio_deck_row()?
                    } else {
                        self.activate_midi_deck_row()?
                    }
                } else if self.deck_focused() {
                    self.open_current()?
                } else {
                    self.apply(Action::Play {
                        playing: !self.transport.playing,
                    })?
                }
            }
            MidiTransportAction::KeepLive => self.keep_all_overrides()?,
            MidiTransportAction::NavigateControl => Outcome::default(),
        };
        let label = action.label().to_string();
        if out.midi_feedback.is_empty() {
            out.midi_feedback.push(MidiFeedback::Display {
                line1: label.clone(),
                line2: self.status.clone(),
                value_bar: None,
            });
        }
        Ok((out, label))
    }

    fn handle_tap_tempo(&mut self) -> Result<Outcome> {
        let now = Instant::now();
        if let Some(&prev) = self.midi.tap_instants.back() {
            if now.duration_since(prev).as_secs_f64() > 2.5 {
                self.midi.tap_instants.clear();
            }
        }
        self.midi.tap_instants.push_back(now);
        while self.midi.tap_instants.len() > 5 {
            self.midi.tap_instants.pop_front();
        }
        if self.midi.tap_instants.len() < 2 {
            self.status = "Tap Tempo: tap again…".into();
            return Ok(Outcome {
                midi_feedback: vec![MidiFeedback::Display {
                    line1: "Tap Tempo".into(),
                    line2: "Tap again...".into(),
                    value_bar: None,
                }],
                ..Outcome::default()
            });
        }
        let mut sum = 0.0;
        let mut count = 0usize;
        let taps: Vec<Instant> = self.midi.tap_instants.iter().copied().collect();
        for w in taps.windows(2) {
            let dt = w[1].duration_since(w[0]).as_secs_f64();
            if (0.2..=2.0).contains(&dt) {
                sum += dt;
                count += 1;
            }
        }
        if count == 0 {
            return Ok(Outcome::default());
        }
        let avg_dt = sum / count as f64;
        let bpm = ((60.0 / avg_dt) * 2.0).round() / 2.0;
        let bpm = bpm.clamp(20.0, 300.0);
        let mut out = self.apply(Action::Edit {
            edit: Edit::SetTempo { bpm },
        })?;
        self.status = format!("Tap Tempo → {bpm:.1} BPM");
        out.midi_feedback.push(MidiFeedback::Display {
            line1: "Tap Tempo".into(),
            line2: format!("{bpm:.1} BPM"),
            value_bar: Some((((bpm - 40.0) / 200.0).clamp(0.0, 1.0) * 127.0) as u8),
        });
        Ok(out)
    }
}
