//! Assignable Project Macros (`MacroControl`, `MacroBinding`) mapping up to 8
//! song-level performance controls to multi-target normalized parameter ranges
//! (including inverted `min > max` mappings) via unified `ParamAddress`.
use crate::params::ParamAddress;
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Maximum number of assignable Project Macros per project (`M1..M8`, indexed `0..8`).
pub const MAX_PROJECT_MACROS: usize = 8;

/// Maximum number of target bindings per Project Macro.
pub const MAX_MACRO_BINDINGS: usize = 16;

/// Maximum display name length for a Project Macro.
pub const MAX_MACRO_NAME_LEN: usize = 32;

/// One normalized target binding inside a [`MacroControl`].
///
/// `min` and `max` are normalized `0.0..=1.0` bounds and explicitly allow
/// inverted mappings (`min > max`) so one macro sweep can open one parameter
/// while closing another simultaneously.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacroBinding {
    pub target: ParamAddress,
    pub min: f32,
    pub max: f32,
}

impl MacroBinding {
    pub fn new(target: ParamAddress, min: f32, max: f32) -> Self {
        Self { target, min, max }
    }

    /// Evaluate the bound target's normalized `0.0..=1.0` value at macro position `x` in `0.0..=1.0`.
    #[inline]
    pub fn evaluate(&self, x: f32) -> f32 {
        let t = if x.is_finite() {
            x.clamp(0.0, 1.0)
        } else {
            0.0
        };
        (self.min + (self.max - self.min) * t).clamp(0.0, 1.0)
    }

    /// Whether this binding sweeps downward as the macro increases (`min > max`).
    #[inline]
    pub fn is_inverted(&self) -> bool {
        self.min > self.max
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.min.is_finite() && (0.0..=1.0).contains(&self.min),
            "Macro binding min must be finite and within 0.0..=1.0"
        );
        ensure!(
            self.max.is_finite() && (0.0..=1.0).contains(&self.max),
            "Macro binding max must be finite and within 0.0..=1.0"
        );
        Ok(())
    }
}

/// One assignable Project Macro (`index` in `0..8`) with a normalized `0.0..=1.0`
/// knob position and up to [`MAX_MACRO_BINDINGS`] multi-target parameter mappings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacroControl {
    pub index: u8,
    pub name: String,
    pub value: f32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<MacroBinding>,
}

impl MacroControl {
    pub fn default_name(index: u8) -> String {
        format!("Macro {}", usize::from(index) + 1)
    }

    pub fn new(index: u8, name: impl Into<String>) -> Self {
        Self {
            index,
            name: name.into(),
            value: 0.0,
            bindings: Vec::new(),
        }
    }

    pub fn validate_self(&self) -> Result<()> {
        ensure!(
            usize::from(self.index) < MAX_PROJECT_MACROS,
            "Macro index {} exceeds 0..{}",
            self.index,
            MAX_PROJECT_MACROS
        );
        let trimmed = self.name.trim();
        ensure!(
            !trimmed.is_empty()
                && trimmed.chars().count() <= MAX_MACRO_NAME_LEN
                && !self.name.chars().any(char::is_control),
            "Macro name must be 1..={MAX_MACRO_NAME_LEN} printable characters"
        );
        ensure!(
            self.value.is_finite() && (0.0..=1.0).contains(&self.value),
            "Macro {} value must be finite and within 0.0..=1.0",
            self.index + 1
        );
        ensure!(
            self.bindings.len() <= MAX_MACRO_BINDINGS,
            "Macro {} exceeds maximum of {MAX_MACRO_BINDINGS} bindings",
            self.index + 1
        );
        let mut seen = HashSet::with_capacity(self.bindings.len());
        for binding in &self.bindings {
            binding.validate()?;
            ensure!(
                seen.insert(binding.target),
                "Macro {} has duplicate binding for the same parameter",
                self.index + 1
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rack::ModuleParam;

    #[test]
    fn macro_binding_evaluates_forward_and_inverted_ranges() {
        let target = ParamAddress::Module {
            instrument: 1,
            zone: 1,
            module: 2,
            param: ModuleParam::Cutoff,
        };
        let forward = MacroBinding::new(target, 0.2, 0.8);
        assert!(!forward.is_inverted());
        assert!((forward.evaluate(0.0) - 0.2).abs() < 1e-6);
        assert!((forward.evaluate(0.5) - 0.5).abs() < 1e-6);
        assert!((forward.evaluate(1.0) - 0.8).abs() < 1e-6);

        let inverted = MacroBinding::new(target, 0.9, 0.1);
        assert!(inverted.is_inverted());
        assert!((inverted.evaluate(0.0) - 0.9).abs() < 1e-6);
        assert!((inverted.evaluate(0.5) - 0.5).abs() < 1e-6);
        assert!((inverted.evaluate(1.0) - 0.1).abs() < 1e-6);
        assert!(inverted.validate().is_ok());
    }
}
