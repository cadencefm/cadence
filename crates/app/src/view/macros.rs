//! Semantic view model for the Assignable Project Macros Deck (`gk`).
use crate::App;
use cadence_core::macros::{MacroControl, MAX_PROJECT_MACROS};
use cadence_core::runtime::AudioSnapshot;

/// Typed parameter target category for [`MacroBindingView`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MacroTargetKind {
    Rack,
    Zone,
    Module,
    Cable,
    Effect,
    Channel,
    Send,
}

impl MacroTargetKind {
    /// All macro target kinds in canonical display order.
    pub const ALL: [Self; 7] = [
        Self::Rack,
        Self::Zone,
        Self::Module,
        Self::Cable,
        Self::Effect,
        Self::Channel,
        Self::Send,
    ];

    /// Short uppercase badge label rendered in the Macros deck and parameter prompts.
    pub const fn badge(self) -> &'static str {
        match self {
            Self::Rack => "RACK",
            Self::Zone => "ZONE",
            Self::Module => "MOD",
            Self::Cable => "CABLE",
            Self::Effect => "FX",
            Self::Channel => "CH",
            Self::Send => "SEND",
        }
    }
}

/// One bound target parameter row inside a [`MacroSlotView`].
#[derive(Clone, Debug)]
pub struct MacroBindingView {
    pub index: usize,
    pub target_kind: MacroTargetKind,
    pub target_name: String,
    pub min_norm: f32,
    pub max_norm: f32,
    pub inverted: bool,
    pub current_norm: f32,
    pub current_display: String,
    pub min_display: String,
    pub max_display: String,
    pub selected: bool,
}

/// One Project Macro slot (`M1..M8`) inside [`MacrosDeckView`].
#[derive(Clone, Debug)]
pub struct MacroSlotView {
    pub index: u8,
    pub configured: bool,
    pub name: String,
    pub value: f32,
    /// Live evaluated macro position (`0.0..=1.0`) when automated via `ModulationTarget::Macro`.
    pub effective_value: Option<f32>,
    pub mod_pattern_name: Option<String>,
    pub midi_badge: Option<String>,
    pub learning: bool,
    pub selected: bool,
    pub bindings: Vec<MacroBindingView>,
}

/// Complete semantic projection of the 8-slot Project Macros console (`gk`).
#[derive(Clone, Debug)]
pub struct MacrosDeckView {
    pub slots: Vec<MacroSlotView>,
    pub selected_index: usize,
    pub selected_binding: Option<usize>,
    pub active_count: usize,
    pub total_bindings: usize,
    pub recording: bool,
}

impl App {
    pub(super) fn build_macros_view(&self, snapshot: &AudioSnapshot) -> MacrosDeckView {
        let project = self.project();
        let query_time = self.live_query_time(snapshot);
        let selected_index = self.view_state.macro_cursor.min(MAX_PROJECT_MACROS - 1);
        let selected_binding_count = project
            .macro_control(selected_index as u8)
            .map_or(0, |m| m.bindings.len());
        let selected_binding = self
            .view_state
            .macro_binding_cursor
            .filter(|&b| b < selected_binding_count);
        let mut active_count = 0usize;
        let mut total_bindings = 0usize;
        let mut slots = Vec::with_capacity(MAX_PROJECT_MACROS);

        for slot_i in 0..MAX_PROJECT_MACROS {
            let index = slot_i as u8;
            let live_mod = self.live_macro_modulation_at(index, query_time);
            let effective_value = live_mod.as_ref().map(|(v, _)| *v);
            let mod_pattern_name = live_mod.map(|(_, name)| name);
            let midi_badge = self.midi_badge_for_macro(slot_i);
            let learning = self.is_macro_learning(slot_i);
            let selected = slot_i == selected_index;

            if let Some(mac) = project.macro_control(index) {
                active_count += 1;
                total_bindings += mac.bindings.len();
                let eval_pos = effective_value.unwrap_or(mac.value);
                let bindings = mac
                    .bindings
                    .iter()
                    .enumerate()
                    .map(|(b_idx, b)| {
                        let (target_kind, target_name) = self.format_macro_target(b.target);
                        let current_norm = b.evaluate(eval_pos);
                        MacroBindingView {
                            index: b_idx,
                            target_kind,
                            target_name,
                            min_norm: b.min,
                            max_norm: b.max,
                            inverted: b.is_inverted(),
                            current_norm,
                            current_display: self
                                .format_param_normalized_value(b.target, current_norm),
                            min_display: self.format_param_normalized_value(b.target, b.min),
                            max_display: self.format_param_normalized_value(b.target, b.max),
                            selected: selected && selected_binding == Some(b_idx),
                        }
                    })
                    .collect();
                slots.push(MacroSlotView {
                    index,
                    configured: true,
                    name: mac.name.clone(),
                    value: mac.value,
                    effective_value,
                    mod_pattern_name,
                    midi_badge,
                    learning,
                    selected,
                    bindings,
                });
            } else {
                slots.push(MacroSlotView {
                    index,
                    configured: false,
                    name: MacroControl::default_name(index),
                    value: 0.0,
                    effective_value,
                    mod_pattern_name,
                    midi_badge,
                    learning,
                    selected,
                    bindings: Vec::new(),
                });
            }
        }

        MacrosDeckView {
            slots,
            selected_index,
            selected_binding,
            active_count,
            total_bindings,
            recording: self.midi.record,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadence_core::material::Project;

    #[test]
    fn macro_binding_min_and_max_display_use_native_parameter_units() {
        let mut app = App::new_in_memory(Project::demo(), "macros-unit.json".into()).unwrap();
        let ch_name = app.project().mixer.channels[0].name.clone();
        app.execute_command("macro 1 bind instrument Saw cutoff 0.0 1.0")
            .unwrap();
        app.execute_command(&format!("macro 1 bind channel {ch_name} gain 0.0 1.0"))
            .unwrap();
        app.execute_command(&format!("macro 1 bind channel {ch_name} pan 0.0 1.0"))
            .unwrap();
        let view = app.build_macros_view(&AudioSnapshot::default());
        let bindings = &view.slots[0].bindings;
        assert_eq!(bindings.len(), 3);
        // Cutoff min/max format in Hz/kHz, not raw %.
        assert!(
            bindings[0].min_display.contains("Hz"),
            "expected Hz min_display, got {}",
            bindings[0].min_display
        );
        assert!(
            bindings[0].max_display.contains("kHz"),
            "expected kHz max_display, got {}",
            bindings[0].max_display
        );
        // Channel gain min/max format in dB (`-inf dB` / `+6.0 dB`).
        assert_eq!(bindings[1].min_display, "-inf dB");
        assert_eq!(bindings[1].max_display, "+6.0 dB");
        // Channel pan min/max format as `L100` / `R100`.
        assert_eq!(bindings[2].min_display, "L100");
        assert_eq!(bindings[2].max_display, "R100");
    }
}
