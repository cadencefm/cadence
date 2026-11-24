//! Hardware controller LED/OLED feedback, inline MIDI badges, and Focused Smart Macro target resolution.
use crate::*;
use cadence_core::midi_map::{
    FeedbackProtocol, MidiFeedback, MidiPadMode, MidiTarget, TransportLedKind,
};
use cadence_core::music::note_name;

/// Pad note palette (`0..=127` per RGB channel) for `MidiPadMode::Notes`.
const NOTE_PAD_COLORS: [(u8, u8, u8); 8] = [
    (127, 28, 12),  // 1: Kick (warm red-orange)
    (127, 74, 8),   // 2: Rim (amber)
    (127, 114, 8),  // 3: Snare (gold)
    (96, 24, 127),  // 4: Clap (violet)
    (10, 118, 127), // 5: Closed Hat (cyan)
    (18, 74, 127),  // 6: Open Hat (sky blue)
    (48, 36, 127),  // 7: Tom (indigo)
    (14, 120, 62),  // 8: Cymbal (emerald)
];

impl App {
    /// Short inline badge text (`CC 74`, `M1·CC86`, etc.) for a parameter control row.
    pub fn midi_badge_for_param(&self, address: ParamAddress) -> Option<String> {
        let effective = self.effective_midi_mappings();
        // Direct explicit mapping wins.
        if let Some((m, _)) = effective
            .iter()
            .find(|(m, _)| m.target == MidiTarget::Param { address })
        {
            return Some(m.source.short_badge());
        }
        // Otherwise check if this parameter is currently bound to a Focused Smart Macro slot.
        let macros = self.focused_macro_targets();
        if let Some(slot) = macros.iter().position(|(_, addr)| *addr == address) {
            if let Some((m, _)) = effective
                .iter()
                .find(|(m, _)| m.target == MidiTarget::FocusedMacro { slot })
            {
                return Some(format!("M{}·{}", slot + 1, m.source.short_badge()));
            }
        }
        None
    }

    /// Whether a Smart Macro slot (`FocusedMacro { slot }`, e.g. MiniLab 3 encoders 1–8)
    /// currently routes to Project Macro `slot` (0..8) rather than a focused-object param.
    pub fn focused_macro_resolves_project_macro(&self, slot: usize) -> Option<u8> {
        if slot >= cadence_core::macros::MAX_PROJECT_MACROS {
            return None;
        }
        if self.macros_focused() {
            return Some(slot as u8);
        }
        let in_object_deck = self.deck_focused()
            && matches!(
                self.detail,
                Detail::Instrument(_) | Detail::Effect(_) | Detail::Channel(_)
            );
        if !in_object_deck && self.doc.project().macro_control(slot as u8).is_some() {
            return Some(slot as u8);
        }
        None
    }

    /// Resolve a `MidiTarget` to a Project Macro slot (`0..8`), if applicable.
    pub(crate) fn resolve_target_project_macro(&self, target: MidiTarget) -> Option<u8> {
        match target {
            MidiTarget::ProjectMacro { slot }
                if slot < cadence_core::macros::MAX_PROJECT_MACROS =>
            {
                Some(slot as u8)
            }
            MidiTarget::FocusedMacro { slot } => self.focused_macro_resolves_project_macro(slot),
            _ => None,
        }
    }

    /// Short inline MIDI CC badge text (`CC 86`, `M1·CC 86`, etc.) for Project Macro `slot` (`0..8`).
    pub fn midi_badge_for_macro(&self, slot: usize) -> Option<String> {
        let effective = self.effective_midi_mappings();
        if let Some((m, _)) = effective
            .iter()
            .find(|(m, _)| m.target == MidiTarget::ProjectMacro { slot })
        {
            return Some(m.source.short_badge());
        }
        if let Some((m, _)) = effective
            .iter()
            .find(|(m, _)| m.target == MidiTarget::FocusedMacro { slot })
        {
            return Some(m.source.short_badge());
        }
        None
    }

    /// Resolve up to 8 smart macro parameter targets for the user's current musical focus.
    pub fn focused_macro_targets(&self) -> Vec<(String, ParamAddress)> {
        // 1. If an Instrument, Effect, or Channel object deck is focused, map its visible Param controls!
        if self.deck_focused()
            && matches!(
                self.detail,
                Detail::Instrument(_) | Detail::Effect(_) | Detail::Channel(_)
            )
        {
            let mut out = Vec::new();
            for c in self.control_descriptors() {
                if let ControlTarget::Param(addr) = c.target {
                    if addr.is_automatable() {
                        out.push((c.label, addr));
                        if out.len() >= 8 {
                            break;
                        }
                    }
                }
            }
            if !out.is_empty() {
                return out;
            }
        }

        // 2. If Mixer is focused, map the selected channel's strip + insert params.
        if self.mixer_canvas_focused() {
            let mut out = Vec::new();
            let ch = self.selected_channel;
            let ch_name = self.channel_name(ch).to_string();
            out.push((
                format!("{ch_name} Gain"),
                ParamAddress::Channel {
                    channel: ch,
                    param: ChannelParam::Gain,
                },
            ));
            out.push((
                format!("{ch_name} Pan"),
                ParamAddress::Channel {
                    channel: ch,
                    param: ChannelParam::Pan,
                },
            ));
            let inserts = match ch {
                ChannelRef::Master => &self.doc.project().mixer.master.inserts,
                ChannelRef::Channel(id) => self
                    .channel(id)
                    .map(|c| &c.inserts)
                    .unwrap_or(&self.doc.project().mixer.master.inserts),
            };
            for dev in inserts {
                for &param in dev.params() {
                    out.push((
                        format!("{} {}", dev.name, param.label()),
                        ParamAddress::Effect {
                            device: dev.id,
                            param,
                        },
                    ));
                    if out.len() >= 8 {
                        return out;
                    }
                }
            }
            return out;
        }

        // 3. Otherwise map the active / selected instrument's flagship musical performance controls!
        let inst_id = self
            .jam_instrument()
            .or(self.selected_instrument)
            .or_else(|| self.doc.project().instruments.first().map(|i| i.id));
        let Some(id) = inst_id else {
            return Vec::new();
        };
        let Some(inst) = self.instrument(id) else {
            return Vec::new();
        };
        let is_continuous = |target: InstrumentParam| match target {
            InstrumentParam::Module { param, .. } => !param.is_discrete(),
            InstrumentParam::Rack { .. }
            | InstrumentParam::Zone { .. }
            | InstrumentParam::Cable { .. } => true,
        };
        let mut out: Vec<(String, ParamAddress)> = Vec::new();
        // Prefer continuous performance pins first so hardware rotary knobs sweep smoothly
        // (e.g. Cutoff, Resonance, Env Mod, Drive, ADSR) instead of snapping enum selectors.
        for pin in &inst.rack.panel_pins {
            if inst.rack.supports_automatable_param(pin.target) && is_continuous(pin.target) {
                let addr = ParamAddress::from_instrument(id, pin.target);
                out.push((format!("{} · {}", inst.name, pin.display_label()), addr));
                if out.len() >= 8 {
                    return out;
                }
            }
        }
        if let Some(zone) = inst.rack.zones.get(self.active_zone_index(&inst.rack)) {
            for m in &zone.modules {
                for &param in m.kind.tag().params() {
                    let ip = InstrumentParam::Module {
                        zone: zone.id,
                        module: m.id,
                        param,
                    };
                    if inst.rack.supports_automatable_param(ip) && !param.is_discrete() {
                        let addr = ParamAddress::from_instrument(id, ip);
                        if !out.iter().any(|(_, a)| *a == addr) {
                            out.push((
                                format!("{} · {} {}", inst.name, m.name, param.label()),
                                addr,
                            ));
                            if out.len() >= 8 {
                                return out;
                            }
                        }
                    }
                }
            }
        }
        for param in RackParam::ALL {
            let addr = ParamAddress::Rack {
                instrument: id,
                param,
            };
            if !out.iter().any(|(_, a)| *a == addr) {
                out.push((format!("{} · {}", inst.name, param.label()), addr));
                if out.len() >= 8 {
                    return out;
                }
            }
        }
        for pin in &inst.rack.panel_pins {
            if inst.rack.supports_automatable_param(pin.target) {
                let addr = ParamAddress::from_instrument(id, pin.target);
                if !out.iter().any(|(_, a)| *a == addr) {
                    out.push((format!("{} · {}", inst.name, pin.display_label()), addr));
                    if out.len() >= 8 {
                        break;
                    }
                }
            }
        }
        out
    }

    /// Resolve the `ParamAddress` for a `MidiTarget` (direct Param, FocusedMacro, or LaneGain).
    pub(crate) fn resolve_target_param(&self, target: MidiTarget) -> Option<ParamAddress> {
        match target {
            MidiTarget::Param { address } => Some(address),
            MidiTarget::FocusedMacro { slot } => {
                self.focused_macro_targets().get(slot).map(|(_, a)| *a)
            }
            MidiTarget::LaneGain { lane_index } => {
                self.lane_channel_ref(lane_index)
                    .map(|channel| ParamAddress::Channel {
                        channel,
                        param: ChannelParam::Gain,
                    })
            }
            _ => None,
        }
    }

    fn lane_channel_ref(&self, lane_index: usize) -> Option<ChannelRef> {
        let lane = self.doc.project().lanes.get(lane_index)?;
        // Follow lane -> first part -> instrument -> channel, falling back to mixer channel index.
        if let Some(lp) = lane.parts.first() {
            if let Some(part) = self.part(lp.part) {
                if let Some(inst) = self.instrument(part.instrument) {
                    return Some(ChannelRef::Channel(inst.channel));
                }
            }
        }
        self.doc
            .project()
            .mixer
            .channels
            .get(lane_index)
            .map(|c| ChannelRef::Channel(c.id))
    }

    pub(crate) fn current_section_index(&self) -> Option<usize> {
        let step = if self.transport.playing {
            self.song_step_heard() as u32
        } else {
            self.arrange_ui.step
        };
        self.doc
            .project()
            .sections
            .iter()
            .position(|s| (s.start_step..s.start_step + s.length_steps).contains(&step))
    }

    /// Compute the 8 RGB pad colors (`0..=127` per channel) for both hardware LEDs and the GPU MIDI Deck.
    pub fn compute_pad_states(&self) -> [(String, (u8, u8, u8), bool); 8] {
        let profile = self.active_midi_profile();
        let active_sec = self.current_section_index();
        let pending_sec_start = self.live.pending_timeline().map(|(_, seg)| seg.song as u32);
        std::array::from_fn(|i| match self.config.midi_pad_mode {
            MidiPadMode::Sections => {
                if let Some(sec) = self.doc.project().sections.get(i) {
                    let is_active = active_sec == Some(i);
                    let is_pending = pending_sec_start == Some(sec.start_step);
                    let rgb = if is_pending {
                        (127, 92, 0)
                    } else if is_active {
                        (0, 127, 48)
                    } else {
                        (14, 62, 92)
                    };
                    (sec.name.clone(), rgb, is_active || is_pending)
                } else {
                    (format!("Sec {}", i + 1), (6, 6, 10), false)
                }
            }
            MidiPadMode::Mutes => {
                if let Some(lane) = self.doc.project().lanes.get(i) {
                    let muted = self.effective_lane_muted(lane.id);
                    let solo = self.effective_lane_solo(lane.id);
                    let rgb = if muted {
                        (127, 16, 16)
                    } else if solo {
                        (127, 108, 0)
                    } else {
                        (10, 112, 96)
                    };
                    (lane.name.clone(), rgb, !muted)
                } else {
                    (format!("Lane {}", i + 1), (4, 4, 8), false)
                }
            }
            MidiPadMode::Notes => {
                let note = profile.pad_notes[i];
                let held = self.input.midi_held.iter().any(|&(n, _)| n == note);
                let rgb = if held {
                    (127, 127, 127)
                } else {
                    NOTE_PAD_COLORS[i]
                };
                // If active Rack has multiple zones (e.g. Drum Kit), show the matching zone name!
                let zone_name = self
                    .jam_instrument()
                    .or(self.selected_instrument)
                    .and_then(|id| self.instrument(id))
                    .filter(|inst| inst.rack.zones.len() > 1)
                    .and_then(|inst| {
                        inst.rack
                            .zones
                            .iter()
                            .find(|z| (z.low_midi..=z.high_midi).contains(&note))
                            .map(|z| z.name.clone())
                    })
                    .unwrap_or_else(|| note_name(note));
                (zone_name, rgb, held)
            }
        })
    }

    /// Push only changed Pad RGB & Transport LED feedback packets onto `outcome.midi_feedback`.
    pub fn sync_midi_feedback(&mut self, outcome: &mut Outcome) {
        if self.active_midi_feedback_protocol() == FeedbackProtocol::None {
            return;
        }
        let pads = self.compute_pad_states();
        for (i, (_, rgb, _)) in pads.iter().enumerate() {
            if self.midi.feedback_cache.pads[i] != Some(*rgb) {
                self.midi.feedback_cache.pads[i] = Some(*rgb);
                outcome.midi_feedback.push(MidiFeedback::PadRgb {
                    index: i as u8,
                    r: rgb.0,
                    g: rgb.1,
                    b: rgb.2,
                });
            }
        }
        let transport_states = [
            (TransportLedKind::Loop, self.playing_loop().is_some()),
            (TransportLedKind::Stop, !self.transport.playing),
            (TransportLedKind::Play, self.transport.playing),
            (TransportLedKind::Record, self.midi.record),
            (TransportLedKind::Tap, !self.midi.tap_instants.is_empty()),
        ];
        for (idx, (kind, active)) in transport_states.into_iter().enumerate() {
            if self.midi.feedback_cache.transport[idx] != Some(active) {
                self.midi.feedback_cache.transport[idx] = Some(active);
                outcome
                    .midi_feedback
                    .push(MidiFeedback::TransportLed { kind, active });
            }
        }
    }

    /// Reset LED diff cache and emit full DAW handshake + welcome OLED + every pad & transport LED.
    pub fn force_midi_feedback_sync(&mut self) -> Vec<MidiFeedback> {
        self.midi.feedback_cache.reset();
        let mut out = Outcome::default();
        if self.active_midi_feedback_protocol() != FeedbackProtocol::None {
            out.midi_feedback.push(MidiFeedback::InitDawMode(true));
            out.midi_feedback.push(MidiFeedback::Display {
                line1: "Cadence".into(),
                line2: self.doc.project().title.clone(),
                value_bar: None,
            });
            self.sync_midi_feedback(&mut out);
        }
        out.midi_feedback
    }
}
