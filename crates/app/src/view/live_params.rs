//! Unified live parameter & macro projection evaluator for semantic views.
//!
//! Mirrors the exact compilation precedence in `cadence_core::material`:
//! 1. **Static Macros** (`project.macros` sorted by ascending `index` `0..8`,
//!    skipping any macro driven by `ModulationTarget::Macro`).
//! 2. **Automated Macro fan-out** (`ModulationTarget::Macro` sorted by ascending
//!    `index` `0..8`; last-write-wins per macro in `project.modulations` order;
//!    skipping non-automatable targets exactly like the compiler).
//! 3. **Direct `Modulation`** (`project.modulations` last-write-wins, always
//!    overriding Macro fan-out independent of `project.modulations` ordering).
//!
//! All views share [`App::live_query_time`] so modulated curves and meters track
//! smooth sub-step song position (`song_position_smooth`) across loops and live
//! section jumps without desyncing on raw `absolute_step`.
use crate::App;
use cadence_core::macros::MacroControl;
use cadence_core::material::ModulationTarget;
use cadence_core::mixer::{ChannelParam, ChannelRef};
use cadence_core::params::ParamAddress;
use cadence_core::runtime::AudioSnapshot;

/// Map a [`ParamAddress`] to its direct [`ModulationTarget`], if direct
/// modulation writes the same parameter slot in the audio engine.
///
/// Note: Direct `ModulationTarget::ChannelGain` writes the internal multiplicative
/// `MixerParamTarget::ChannelGainScale` (which multiplies the channel's strip/macro
/// [`ChannelParam::Gain`]), so `ChannelParam::Gain` has no direct replacement target
/// here — evaluate `ChannelGain` via [`App::evaluate_channel_gain_scale_at`].
fn direct_mod_target(address: ParamAddress) -> Option<ModulationTarget> {
    if let Some((instrument, parameter)) = address.as_instrument() {
        return Some(ModulationTarget::Instrument {
            instrument,
            parameter,
        });
    }
    match address {
        ParamAddress::Effect { device, param } => Some(ModulationTarget::Effect {
            device,
            parameter: param,
        }),
        ParamAddress::Channel { channel, param } => match param {
            ChannelParam::Pan => Some(ModulationTarget::ChannelPan { channel }),
            ChannelParam::Gain => None,
        },
        ParamAddress::Send { channel, target } => Some(ModulationTarget::Send { channel, target }),
        ParamAddress::Rack { .. }
        | ParamAddress::Zone { .. }
        | ParamAddress::Module { .. }
        | ParamAddress::Cable { .. } => unreachable!(),
    }
}

impl App {
    /// Smooth song position in steps for projecting live automated curves and meters.
    ///
    /// While playing, uses [`App::song_position_smooth`] so loop regions and live
    /// section jumps stay locked to heard audio; falls back to sub-step solo audition
    /// position or last heard step if `song_position_smooth` returns `None`.
    /// While stopped, projects at the current `arrange_step` cursor.
    pub(crate) fn live_query_time(&self, snapshot: &AudioSnapshot) -> f64 {
        if snapshot.playing {
            self.song_position_smooth(snapshot).unwrap_or_else(|| {
                if self.is_solo_auditioning() {
                    snapshot.absolute_step as f64
                        + f64::from(snapshot.step_fraction.clamp(0.0, 0.9999))
                } else {
                    self.song_step_heard() as f64
                }
            })
        } else {
            self.arrange_ui.step as f64
        }
    }

    /// Evaluate the live automated position (`0.0..=1.0`) and source pattern name
    /// for Project Macro `index` at `query_time`, using last-write-wins semantics.
    pub(crate) fn live_macro_modulation_at(
        &self,
        index: u8,
        query_time: f64,
    ) -> Option<(f32, String)> {
        let project = self.project();
        let m = project
            .modulations
            .iter()
            .rev()
            .find(|m| m.target == ModulationTarget::Macro { index })?;
        let value = project
            .value_at(m.pattern, query_time)
            .ok()
            .filter(|v| v.is_finite())?
            .clamp(0.0, 1.0);
        let pat_name = project
            .pattern(m.pattern)
            .map_or_else(|| m.name.clone(), |p| p.name.clone());
        Some((value, pat_name))
    }

    /// Evaluate the live automated position (`0.0..=1.0`) and source pattern name
    /// for Project Macro `index` at the current smooth transport time.
    pub(crate) fn live_macro_modulation(
        &self,
        index: u8,
        snapshot: &AudioSnapshot,
    ) -> Option<(f32, String)> {
        self.live_macro_modulation_at(index, self.live_query_time(snapshot))
    }

    /// Evaluate the live normalized value (`0.0..=1.0`) of `address` at `query_time`
    /// if driven by any static Macro, automated Macro, or direct `Modulation`.
    ///
    /// Returns `None` when `address` is purely static (no Macro binding and no
    /// direct `Modulation` targets it).
    pub(crate) fn evaluate_live_param_at(
        &self,
        address: ParamAddress,
        query_time: f64,
    ) -> Option<f32> {
        let project = self.project();
        let mut resolved: Option<f32> = None;

        // Stage 1: Static Macros (ascending macro index 0..8, skipping automated macros).
        let mut ordered_macros: Vec<&MacroControl> = project.macros.iter().collect();
        ordered_macros.sort_by_key(|m| m.index);
        for mac in &ordered_macros {
            if project
                .modulations
                .iter()
                .any(|m| m.target == ModulationTarget::Macro { index: mac.index })
            {
                continue;
            }
            for binding in &mac.bindings {
                if binding.target == address {
                    resolved = Some(binding.evaluate(mac.value).clamp(0.0, 1.0));
                }
            }
        }

        // Stage 2: Automated Macro fan-out (ascending macro index 0..8; last-write-wins per macro).
        if address.is_automatable() {
            for mac in &ordered_macros {
                if !mac.bindings.iter().any(|b| b.target == address) {
                    continue;
                }
                if let Some((macro_val, _)) = self.live_macro_modulation_at(mac.index, query_time) {
                    for binding in &mac.bindings {
                        if binding.target == address {
                            resolved = Some(binding.evaluate(macro_val).clamp(0.0, 1.0));
                        }
                    }
                }
            }
        }

        // Stage 3: Direct Modulation (last-write-wins in `project.modulations` order,
        // always overriding Macro fan-out independent of Vec order).
        if let Some(target) = direct_mod_target(address) {
            if let Some(direct) = project
                .modulations
                .iter()
                .rev()
                .find(|m| m.target == target)
                .and_then(|m| {
                    project
                        .value_at(m.pattern, query_time)
                        .ok()
                        .filter(|v| v.is_finite())
                        .map(|v| v.clamp(0.0, 1.0))
                })
            {
                resolved = Some(direct);
            }
        }

        resolved
    }

    /// Evaluate the live multiplicative gain scale (`0.0..=1.0`) from direct
    /// `ModulationTarget::ChannelGain` (compiled internally as `MixerParamTarget::ChannelGainScale`),
    /// using last-write-wins semantics.
    pub(crate) fn evaluate_channel_gain_scale_at(
        &self,
        channel: ChannelRef,
        query_time: f64,
    ) -> Option<f32> {
        let project = self.project();
        project
            .modulations
            .iter()
            .rev()
            .find(|m| m.target == ModulationTarget::ChannelGain { channel })
            .and_then(|m| {
                project
                    .value_at(m.pattern, query_time)
                    .ok()
                    .filter(|v| v.is_finite())
                    .map(|v| v.clamp(0.0, 1.0))
            })
    }

    /// Return formatted Macro labels (`"M1 <name>"`) bound to `address`, in ascending macro index order.
    pub(crate) fn macro_modulator_labels(&self, address: ParamAddress) -> Vec<String> {
        let mut ordered: Vec<&MacroControl> = self
            .project()
            .macros
            .iter()
            .filter(|mac| mac.bindings.iter().any(|b| b.target == address))
            .collect();
        ordered.sort_by_key(|m| m.index);
        ordered
            .into_iter()
            .map(|mac| format!("M{} {}", mac.index + 1, mac.name))
            .collect()
    }

    /// Single point of truth for compact driver pills (`M1`, Mod name, `Link:Part`)
    /// surfaced on controlled parameter rows across Rack, Channel, and Effect decks.
    pub(crate) fn param_driver_pills(&self, address: ParamAddress) -> Vec<String> {
        let project = self.project();
        let mut pills = Vec::new();

        let mut ordered_macros: Vec<&MacroControl> = project
            .macros
            .iter()
            .filter(|mac| mac.bindings.iter().any(|b| b.target == address))
            .collect();
        ordered_macros.sort_by_key(|m| m.index);
        for mac in ordered_macros {
            pills.push(format!("M{}", usize::from(mac.index) + 1));
        }

        if let Some(target) = self.param_address_to_mod_target(address) {
            for m in project.modulations.iter().filter(|m| m.target == target) {
                let label = if m.name.trim().is_empty() {
                    project
                        .pattern(m.pattern)
                        .map_or_else(|| "Mod".into(), |p| p.name.clone())
                } else {
                    m.name.clone()
                };
                pills.push(label);
            }
        }

        if let Some((inst_id, inst_param)) = address.as_instrument() {
            for part in project.parts.iter().filter(|p| p.instrument == inst_id) {
                if part.parameter_links.iter().any(|l| l.target == inst_param) {
                    pills.push(format!("Link:{}", part.name));
                }
            }
        }

        pills
    }
}
