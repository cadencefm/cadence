//! 1-key MIDI Learn (`M`): focus target extraction, arm/cancel/unlearn, and mapping binding.
use crate::*;
use cadence_core::midi_map::{
    MidiFeedback, MidiMapping, MidiSource, MidiTarget, MidiTransportAction, MidiValueMode,
};

impl App {
    pub fn is_param_learning(&self, address: ParamAddress) -> bool {
        self.midi.learn == Some(MidiTarget::Param { address })
    }

    pub fn is_macro_learning(&self, slot: usize) -> bool {
        self.midi.learn == Some(MidiTarget::ProjectMacro { slot })
    }

    /// Determine the most natural `MidiTarget` for 1-key MIDI Learn (`M`) based on current UI focus.
    pub fn learn_target_from_focus(&self) -> Option<(MidiTarget, String)> {
        if self.macros_focused() {
            let slot = self
                .view_state
                .macro_cursor
                .min(cadence_core::macros::MAX_PROJECT_MACROS - 1);
            let name = self.doc.project().macro_control(slot as u8).map_or_else(
                || cadence_core::macros::MacroControl::default_name(slot as u8),
                |m| m.name.clone(),
            );
            return Some((
                MidiTarget::ProjectMacro { slot },
                format!("M{} · {name}", slot + 1),
            ));
        }
        if self.midi_focused() {
            if self.devices.tab == DevicesTab::Midi {
                let mappings = self.effective_midi_mappings();
                if let Some(map_idx) = self.midi.cursor.checked_sub(super::MidiDeckRow::COUNT) {
                    if let Some((m, _)) = mappings.get(map_idx) {
                        return Some((m.target, m.label.clone()));
                    }
                }
            }
            return None;
        }
        if self.deck_focused() {
            if let Some(ctrl) = self.control_descriptors().get(self.control_cursor) {
                if let ControlTarget::Param(address) = ctrl.target {
                    return address.is_automatable().then(|| {
                        (
                            MidiTarget::Param { address },
                            format!("{} · {}", ctrl.group, ctrl.label),
                        )
                    });
                }
            }
        }
        if self.mixer_canvas_focused() {
            let ch = self.selected_channel;
            match self.view_state.mixer_slot {
                MixerSlot::Pan => {
                    return Some((
                        MidiTarget::Param {
                            address: ParamAddress::Channel {
                                channel: ch,
                                param: ChannelParam::Pan,
                            },
                        },
                        format!("{} Pan", self.channel_name(ch)),
                    ));
                }
                MixerSlot::Send(idx) => {
                    if let Some((ch_id, target)) = self.mixer_focused_send(idx) {
                        return Some((
                            MidiTarget::Param {
                                address: ParamAddress::Send {
                                    channel: ChannelRef::Channel(ch_id),
                                    target,
                                },
                            },
                            format!(
                                "{} → {}",
                                self.channel_name(ch),
                                self.channel_name(ChannelRef::Channel(target))
                            ),
                        ));
                    }
                }
                _ => {}
            }
            return Some((
                MidiTarget::Param {
                    address: ParamAddress::Channel {
                        channel: ch,
                        param: ChannelParam::Gain,
                    },
                },
                format!("{} Gain", self.channel_name(ch)),
            ));
        }
        if self.bank_focused() && self.view_state.bank_mode == BankMode::Parts {
            let lane_index = self
                .view_state
                .bank_lane_idx
                .min(self.doc.project().lanes.len().saturating_sub(1));
            if let Some(lane) = self.doc.project().lanes.get(lane_index) {
                if !lane.parts.is_empty() {
                    let part_index = self.view_state.bank_part_idx.min(lane.parts.len() - 1);
                    let part_name = self
                        .part(lane.parts[part_index].part)
                        .map_or("Part", |p| p.name.as_str());
                    return Some((
                        MidiTarget::LaunchLanePart {
                            lane_index,
                            part_index,
                        },
                        format!("{} · {part_name}", lane.name),
                    ));
                }
            }
        }
        // Fallback: learn active Rack's first pinned control, first Filter cutoff, or Rack Cutoff.
        if let Some(id) = self.selected_instrument {
            if let Some(inst) = self.instrument(id) {
                if let Some(pin) = inst
                    .rack
                    .panel_pins
                    .iter()
                    .find(|p| inst.rack.supports_automatable_param(p.target))
                {
                    return Some((
                        MidiTarget::Param {
                            address: ParamAddress::from_instrument(id, pin.target),
                        },
                        format!("{} · {}", inst.name, pin.display_label()),
                    ));
                }
                return Some((
                    MidiTarget::Param {
                        address: ParamAddress::Rack {
                            instrument: id,
                            param: RackParam::Cutoff,
                        },
                    },
                    format!("{} · Cutoff", inst.name),
                ));
            }
        }
        None
    }

    /// Arm MIDI Learn for an explicit `(target, label)` pair and emit controller OLED feedback.
    pub(crate) fn arm_midi_learn_target(&mut self, target: MidiTarget, label: String) -> Outcome {
        let mut out = Outcome::default();
        self.midi.learn = Some(target);
        self.status =
            format!("MIDI LEARN armed → {label} · move a knob/fader/pad · x unlearn · Esc cancel");
        out.midi_feedback.push(MidiFeedback::Display {
            line1: "MIDI LEARN".into(),
            line2: label,
            value_bar: None,
        });
        out
    }

    /// Arm or toggle off 1-key MIDI Learn (`M`).
    pub fn toggle_midi_learn(&mut self) -> Outcome {
        let mut out = Outcome::default();
        if self.midi.learn.take().is_some() {
            self.status = "MIDI Learn cancelled".into();
            out.midi_feedback.push(MidiFeedback::Display {
                line1: "Cadence".into(),
                line2: "Learn cancelled".into(),
                value_bar: None,
            });
            return out;
        }
        let Some((target, label)) = self.learn_target_from_focus() else {
            self.status = if self.midi_focused() && self.devices.tab == DevicesTab::Audio {
                "Switch to MIDI tab (H/L) or focus a parameter to MIDI Learn".into()
            } else if self.midi_focused() {
                "Focus a mapping row to re-learn, or press a to add a MIDI mapping".into()
            } else {
                "Focus a parameter, mixer strip, or bank part to MIDI Learn".into()
            };
            return out;
        };
        self.arm_midi_learn_target(target, label)
    }

    /// Unlearn any user mapping bound to `target` (or currently armed/focused target).
    pub fn unlearn_midi_target(&mut self, target: Option<MidiTarget>) -> Outcome {
        let mut out = Outcome::default();
        let target = target
            .or_else(|| self.midi.learn.take())
            .or_else(|| self.learn_target_from_focus().map(|(t, _)| t));
        let Some(target) = target else {
            self.status = "No MIDI target selected to unlearn".into();
            return out;
        };
        let before = self.config.midi_mappings.len();
        self.config.midi_mappings.retain(|m| m.target != target);
        if self.config.midi_mappings.len() < before {
            self.settings_ui.config_dirty = true;
            self.maybe_persist_config();
            self.status = format!("Unlearned MIDI mapping for {}", target.default_label());
            out.midi_feedback.push(MidiFeedback::Display {
                line1: "MIDI Unlearned".into(),
                line2: target.default_label(),
                value_bar: None,
            });
        } else {
            self.status = format!("No user MIDI mapping on {}", target.default_label());
        }
        out
    }

    pub(super) fn next_midi_mapping_id(&self) -> u64 {
        self.config
            .midi_mappings
            .iter()
            .map(|m| m.id)
            .max()
            .unwrap_or(1000)
            + 1
    }

    pub(super) fn complete_midi_learn(
        &mut self,
        target: MidiTarget,
        source: MidiSource,
    ) -> Outcome {
        let mode = match (&source, &target) {
            (
                MidiSource::Note { .. },
                MidiTarget::LaunchSection { .. }
                | MidiTarget::LaunchLanePart { .. }
                | MidiTarget::Transport { .. },
            ) => MidiValueMode::Trigger,
            (
                MidiSource::Note { .. },
                MidiTarget::MuteLane { .. } | MidiTarget::SoloLane { .. },
            ) => MidiValueMode::Toggle,
            (
                MidiSource::Cc { .. },
                MidiTarget::Transport {
                    action:
                        MidiTransportAction::PlayPause
                        | MidiTransportAction::Stop
                        | MidiTransportAction::Record
                        | MidiTransportAction::LoopToggle
                        | MidiTransportAction::TapTempo
                        | MidiTransportAction::PrevSection
                        | MidiTransportAction::NextSection
                        | MidiTransportAction::ConfirmControl
                        | MidiTransportAction::KeepLive,
                },
            ) => MidiValueMode::Trigger,
            _ => MidiValueMode::Absolute,
        };
        // Replace any existing user mapping on the same target or same source so bindings stay unambiguous.
        self.config
            .midi_mappings
            .retain(|m| m.target != target && m.source != source);
        let id = self.next_midi_mapping_id();
        let mapping = MidiMapping::new(id, source, mode, target);
        let label = mapping.label.clone();
        self.config.midi_mappings.push(mapping);
        self.settings_ui.config_dirty = true;
        self.midi.pickup_latched.remove(&id);
        self.maybe_persist_config();
        self.status = format!("Learned {} → {label}", source.display_label());
        self.push_midi_log(
            source.short_badge(),
            "LEARN".into(),
            format!("Bound → {label}"),
        );
        let mut out = Outcome::default();
        out.midi_feedback.push(MidiFeedback::Display {
            line1: format!("Bound {}", source.short_badge()),
            line2: label,
            value_bar: None,
        });
        self.sync_midi_feedback(&mut out);
        out
    }
}
